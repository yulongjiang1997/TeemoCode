// 网关 HTTP 服务:手写最小面(browser/mcp.rs 同款风格,不引 HTTP 框架)。
//
// 对外契约(见 spec):GET /v1/models、POST /v1/chat/completions(流式
// close-delimited SSE)、GET /health;Bearer = 组 Key → 组即调度与鉴权边界。
//
// 与 mcp server 的两点差异:
//   - 请求体上限 32MB(长上下文对话是真需求,不是异常);
//   - 支持流式应答:响应不写 Content-Length,以 Connection: close 收尾
//     (HTTP/1.1 允许,OpenAI 各 SDK 的 HTTP 栈都按读到 EOF 结束处理)。
//
// 故障切换纪律:**首字节发出前**才允许换模型——SSE 头一旦落笔,客户端侧
// 已经"成功",此后上游出错只能补发一条 SSE error 事件并收尾。
//
// 请求 Expect: 100-continue 必须应答:curl 等客户端对大 body 会先探,
// 不应答会让每次请求白等 1 秒超时窗口。头与体必须经**同一个** BufReader
// 读——换回裸 conn 读体时,BufReader 的预读缓冲会把体前段吃掉。

use std::io::{BufRead as _, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::upstream::{self, GroupCtx, UpstreamError, Usage};
use super::{sched, LogEntry, RuntimeGroup, GatewayHost};

const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_HEADERS: usize = 100;
/// 请求体上限:长上下文对话按 ~8 token/KB 估算,32MB 远超任何真实场景。
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_INFLIGHT_CONNS: usize = 32;

// ==================== 服务生命周期 ====================

#[derive(Clone)]
pub struct ServerHandle {
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    pub port: u16,
}

impl ServerHandle {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst) && !self.stop.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// 启动监听线程。port=0 时绑定临时端口(测试用),handle.port 报实际值。
pub fn start(host: GatewayHost, port: u16) -> Result<ServerHandle, String> {
    let listener =
        TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("端口 {port} 监听失败: {e}"))?;
    let actual = listener.local_addr().map_err(|e| e.to_string())?.port();
    let handle = ServerHandle {
        stop: Arc::new(AtomicBool::new(false)),
        running: Arc::new(AtomicBool::new(true)),
        port: actual,
    };
    let stop = handle.stop.clone();
    let running = handle.running.clone();
    let inflight = Arc::new(AtomicUsize::new(0));
    let _ = listener.set_nonblocking(true);
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut conn, _)) => {
                    if inflight.load(Ordering::SeqCst) >= MAX_INFLIGHT_CONNS {
                        let _ = write_json(
                            &mut conn,
                            503,
                            &openai_error("网关繁忙(在途连接达上限)", "gateway_error", ""),
                            &[],
                        );
                        continue;
                    }
                    inflight.fetch_add(1, Ordering::SeqCst);
                    let host = host.clone();
                    let gauge = inflight.clone();
                    std::thread::spawn(move || {
                        handle_conn(&mut conn, &host);
                        gauge.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => {
                    // 监听套接字级异常:退出循环,running 落 false 让 reload 有机会重建
                    break;
                }
            }
        }
        running.store(false, Ordering::SeqCst);
    });
    Ok(handle)
}

// ==================== 常时比较(browser/bridge.rs 同语义) ====================

pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ==================== 最小 HTTP 读 ====================

struct HttpReq {
    method: String,
    path: String,
    bearer: Option<String>,
    body: Vec<u8>,
}

enum ReadError {
    /// 应写回的状态码与提示。
    Status(u16, &'static str),
    /// 连接异常,直接放弃。
    Abandoned,
}

fn read_request(conn: &mut TcpStream) -> Result<HttpReq, ReadError> {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(30)));
    let mut reader = std::io::BufReader::new(conn.try_clone().map_err(|_| ReadError::Abandoned)?);
    let mut head = (&mut reader).take(MAX_HEADER_BYTES as u64);
    let mut line = String::new();
    head.read_line(&mut line).map_err(|_| ReadError::Abandoned)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or(ReadError::Abandoned)?.to_string();
    let path = parts
        .next()
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string();
    let mut bearer = None;
    let mut expect_continue = false;
    let mut content_length = None;
    let mut chunked = false;
    for _ in 0..MAX_HEADERS {
        let mut h = String::new();
        if head.read_line(&mut h).is_err() || h.trim().is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if lower.starts_with("authorization:") {
            let v = h.split_once(':').map(|(_, value)| value).unwrap_or("").trim();
            bearer = v.strip_prefix("Bearer ").map(str::to_string);
        }
        if lower.starts_with("content-length:") {
            content_length = lower.split_once(':').and_then(|(_, v)| v.trim().parse().ok());
        }
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
        if lower.starts_with("expect:") && lower.contains("100-continue") {
            expect_continue = true;
        }
    }
    if chunked {
        return Err(ReadError::Status(411, "暂不支持 chunked 请求体"));
    }
    let len = match content_length {
        Some(len) if len <= MAX_BODY_BYTES => len,
        Some(_) => return Err(ReadError::Status(413, "请求体过大")),
        None => 0,
    };
    if expect_continue && len > 0 {
        let _ = conn.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    let mut body = vec![0u8; len];
    // 体必须从**同一个** reader 读(头读多了会吃掉体的前段)
    head.read_exact(&mut body).map_err(|_| ReadError::Abandoned)?;
    Ok(HttpReq { method, path, bearer, body })
}

// ==================== 最小 HTTP 写 ====================

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn write_json(conn: &mut TcpStream, status: u16, body: &Value, extra: &[(&str, String)]) {
    let payload = body.to_string();
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        reason(status),
        payload.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = conn.write_all(head.as_bytes());
    let _ = conn.write_all(payload.as_bytes());
    let _ = conn.flush();
}

fn write_sse_head(conn: &mut TcpStream, extra: &[(&str, String)]) {
    let mut head = String::from(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n",
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = conn.write_all(head.as_bytes());
    let _ = conn.flush();
}

fn openai_error(message: &str, err_type: &str, code: &str) -> Value {
    json!({ "error": { "message": message, "type": err_type, "code": code } })
}

// ==================== 请求分发 ====================

fn handle_conn(conn: &mut TcpStream, host: &GatewayHost) {
    let req = match read_request(conn) {
        Ok(req) => req,
        Err(ReadError::Status(status, msg)) => {
            write_json(conn, status, &openai_error(msg, "invalid_request_error", ""), &[]);
            return;
        }
        Err(ReadError::Abandoned) => return,
    };
    // 健康探针免鉴权(探活不走 Key)
    if req.path == "/health" {
        write_json(conn, 200, &json!({ "ok": true, "service": "teemo-gateway" }), &[]);
        return;
    }

    let snapshot = host.snapshot();
    let key = req.bearer.clone().unwrap_or_default();
    let key_valid =
        snapshot.groups.iter().any(|g| g.group.enabled && ct_eq(key.as_bytes(), g.group.key.as_bytes()));
    let authed_group = if key_valid { snapshot.group_by_key(&key).cloned() } else { None };

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/v1/models") => {
            if !key_valid {
                write_json(conn, 401, &openai_error("无效的 API Key(模型组 Key 不匹配)", "invalid_request_error", "invalid_api_key"), &[]);
                return;
            }
            let created = (super::now_ms() / 1000) as i64;
            let data: Vec<Value> = snapshot
                .groups
                .iter()
                .filter(|g| g.group.enabled)
                .map(|g| {
                    json!({
                        "id": g.group.name,
                        "object": "model",
                        "created": created,
                        "owned_by": "teemo-gateway",
                        "context_length": g.group.effective_context_window(),
                        "max_output_tokens": g.group.effective_max_output(),
                    })
                })
                .collect();
            write_json(conn, 200, &json!({ "object": "list", "data": data }), &[]);
        }
        ("POST", "/v1/chat/completions") => {
            let Some(group) = authed_group else {
                write_json(conn, 401, &openai_error("无效的 API Key(模型组 Key 不匹配)", "invalid_request_error", "invalid_api_key"), &[]);
                return;
            };
            let Ok(incoming) = serde_json::from_slice::<Value>(&req.body) else {
                write_json(conn, 400, &openai_error("请求体不是有效 JSON", "invalid_request_error", ""), &[]);
                return;
            };
            if incoming.get("messages").and_then(Value::as_array).is_none() {
                write_json(conn, 400, &openai_error("请求缺少 messages 数组", "invalid_request_error", ""), &[]);
                return;
            }
            let stream = incoming.get("stream").and_then(Value::as_bool).unwrap_or(false);
            let started = std::time::Instant::now();
            if stream {
                handle_streaming(conn, host, &group, incoming, started);
            } else {
                handle_buffered(conn, host, &group, incoming);
            }
        }
        ("POST", _) | ("GET", "/v1/chat/completions") => {
            write_json(conn, 405, &openai_error("方法或路径不支持", "invalid_request_error", ""), &[]);
        }
        _ => {
            write_json(conn, 404, &openai_error("未知路径(支持 /v1/models 与 /v1/chat/completions)", "invalid_request_error", ""), &[]);
        }
    }
}

// ==================== 尝试记录与摘要 ====================

struct AttemptNote {
    label: String,
    model: String,
    ok: bool,
    message: String,
}

fn unavailable_notes(group: &RuntimeGroup) -> Vec<AttemptNote> {
    group
        .candidates
        .iter()
        .filter(|c| c.unavailable.is_some())
        .map(|c| AttemptNote {
            label: c.label.clone(),
            model: c.model.clone(),
            ok: false,
            message: c.unavailable.clone().unwrap_or_default(),
        })
        .collect()
}

fn summarize_attempts(attempts: &[AttemptNote]) -> String {
    let parts: Vec<String> = attempts
        .iter()
        .map(|a| {
            if a.ok {
                format!("{}({}) 成功", a.label, a.model)
            } else {
                format!("{}({}) {}", a.label, a.model, a.message)
            }
        })
        .collect();
    format!("共尝试 {} 个模型: {}", attempts.len(), parts.join("; "))
}

fn push_log(
    host: &GatewayHost,
    group: &RuntimeGroup,
    stream: bool,
    started: std::time::Instant,
    ok: bool,
    status: Option<u16>,
    model: String,
    attempts: u32,
    usage: &Usage,
    error: Option<String>,
) {
    host.push_log(LogEntry {
        ts_ms: super::now_ms(),
        group_id: group.group.id.clone(),
        group_name: group.group.name.clone(),
        stream,
        ok,
        status,
        latency_ms: started.elapsed().as_millis() as u64,
        model,
        attempts,
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        error: error.map(|e| e.chars().take(200).collect::<String>()),
    });
}

// ==================== 非流式调度(服务与测试命令共用) ====================

pub(crate) struct BufferedOk {
    pub body: Value,
    pub model: String,
    pub attempts: usize,
}

pub(crate) struct BufferedFail {
    pub status: u16,
    pub body: Value,
    pub summary: String,
    pub attempts: usize,
}

/// 非流式调度:按 plan 逐个尝试,成功即返回;全失败返回最后一个错误。
/// 日志与健康簿在此落账(gateway_test_group 同路径,可观测一致)。
pub(crate) async fn run_buffered(
    host: &GatewayHost,
    group: &RuntimeGroup,
    incoming: Value,
) -> Result<BufferedOk, BufferedFail> {
    let started = std::time::Instant::now();
    let ctx = GroupCtx::of(&group.group);
    let timeout = group.group.effective_timeout();
    let mut attempts = unavailable_notes(group);
    let prompt_est = upstream::estimate_tokens(upstream::incoming_prompt_chars(&incoming));

    let mut rng = host.rng_next();
    let plan = {
        let health = host.health_map();
        sched::plan(
            group.group.effective_strategy(),
            &group.group.id,
            &group.candidates,
            &health,
            super::now_ms(),
            &mut rng,
        )
    };
    let mut last_error: Option<UpstreamError> = None;
    let mut usage_out = Usage { prompt_tokens: Some(prompt_est), completion_tokens: None };
    for cand in &plan {
        match upstream::call_buffered(host.client(), cand, &incoming, &ctx, timeout).await {
            Ok(reply) => {
                host.record_attempt(&group.group.id, &cand.id, true);
                attempts.push(AttemptNote {
                    label: cand.label.clone(),
                    model: reply.model.clone(),
                    ok: true,
                    message: String::new(),
                });
                usage_out.merge(&reply.usage);
                push_log(host, group, false, started, true, Some(200), reply.model.clone(), attempts.len() as u32, &usage_out, None);
                return Ok(BufferedOk { body: reply.body, model: reply.model, attempts: attempts.len() });
            }
            Err(e) => {
                host.record_attempt(&group.group.id, &cand.id, false);
                attempts.push(AttemptNote {
                    label: cand.label.clone(),
                    model: cand.model.clone(),
                    ok: false,
                    message: e.message(),
                });
                last_error = Some(e);
            }
        }
    }
    let attempts_n = attempts.len() as u32;
    let summary = if last_error.is_some() {
        format!("模型组「{}」全部候选失败({})", group.group.name, summarize_attempts(&attempts))
    } else {
        format!("模型组「{}」没有可用模型(组内无启用条目或全部被熔断)", group.group.name)
    };
    let status = last_error.as_ref().and_then(|e| e.status()).unwrap_or(502);
    let model = attempts.last().map(|a| a.model.clone()).unwrap_or_default();
    push_log(
        host,
        group,
        false,
        started,
        false,
        Some(status),
        model.clone(),
        attempts_n,
        &usage_out,
        Some(summary.clone()),
    );
    Err(BufferedFail {
        status,
        body: openai_error(&summary, "gateway_error", "all_models_failed"),
        summary,
        attempts: attempts.len(),
    })
}

fn handle_buffered(conn: &mut TcpStream, host: &GatewayHost, group: &RuntimeGroup, incoming: Value) {
    match tauri::async_runtime::block_on(run_buffered(host, group, incoming)) {
        Ok(ok) => {
            let extra = [
                ("X-Gateway-Group", group.group.name.clone()),
                ("X-Gateway-Model", ok.model.clone()),
            ];
            write_json(conn, 200, &ok.body, &extra);
        }
        Err(fail) => {
            let extra = [("X-Gateway-Group", group.group.name.clone())];
            write_json(conn, fail.status, &fail.body, &extra);
        }
    }
}

// ==================== 流式调度 ====================

struct StreamOutcome {
    ok: bool,
    status: Option<u16>,
    model: String,
    attempts: usize,
    usage: Usage,
    error: Option<String>,
}

fn handle_streaming(
    conn: &mut TcpStream,
    host: &GatewayHost,
    group: &RuntimeGroup,
    incoming: Value,
    started: std::time::Instant,
) {
    let ctx = GroupCtx::of(&group.group);
    let timeout = group.group.effective_timeout();
    let mut attempts = unavailable_notes(group);
    let prompt_est = upstream::estimate_tokens(upstream::incoming_prompt_chars(&incoming));

    let outcome = tauri::async_runtime::block_on(async {
        let mut rng = host.rng_next();
        let plan = {
            let health = host.health_map();
            sched::plan(
                group.group.effective_strategy(),
                &group.group.id,
                &group.candidates,
                &health,
                super::now_ms(),
                &mut rng,
            )
        };
        let mut last_error: Option<UpstreamError> = None;
        for cand in &plan {
            match upstream::open_stream(host.client(), cand, &incoming, &ctx, timeout).await {
                Ok((reply, model)) => {
                    host.record_attempt(&group.group.id, &cand.id, true);
                    attempts.push(AttemptNote {
                        label: cand.label.clone(),
                        model: model.clone(),
                        ok: true,
                        message: String::new(),
                    });
                    // 首字节前是最后一次换模型的机会:SSE 头现在落笔
                    write_sse_head(
                        conn,
                        &[
                            ("X-Gateway-Group", group.group.name.clone()),
                            ("X-Gateway-Model", model.clone()),
                        ],
                    );
                    let mut write_conn = match conn.try_clone() {
                        Ok(c) => c,
                        Err(e) => {
                            return StreamOutcome {
                                ok: false,
                                status: Some(200),
                                model,
                                attempts: attempts.len(),
                                usage: Usage { prompt_tokens: Some(prompt_est), completion_tokens: None },
                                error: Some(format!("客户端连接不可写: {e}")),
                            };
                        }
                    };
                    let write = move |bytes: &[u8]| -> std::io::Result<()> { write_conn.write_all(bytes) };
                    let relay = upstream::relay_stream(reply, model.clone(), write, timeout.max(Duration::from_secs(60))).await;
                    return match relay {
                        Ok(summary) => StreamOutcome {
                            ok: summary.error.is_none(),
                            status: Some(200),
                            model,
                            attempts: attempts.len(),
                            usage: Usage {
                                prompt_tokens: Some(prompt_est),
                                completion_tokens: summary
                                    .usage
                                    .completion_tokens
                                    .or_else(|| Some(upstream::estimate_tokens(summary.completion_chars))),
                            },
                            error: summary.error,
                        },
                        Err(e) => StreamOutcome {
                            ok: false,
                            status: Some(200),
                            model,
                            attempts: attempts.len(),
                            usage: Usage { prompt_tokens: Some(prompt_est), completion_tokens: None },
                            error: Some(e),
                        },
                    };
                }
                Err(e) => {
                    host.record_attempt(&group.group.id, &cand.id, false);
                    attempts.push(AttemptNote {
                        label: cand.label.clone(),
                        model: cand.model.clone(),
                        ok: false,
                        message: e.message(),
                    });
                    last_error = Some(e);
                }
            }
        }
        // 全部候选失败:头还没发,按非流式错误形态应答
        let summary = format!(
            "模型组「{}」全部候选失败({})",
            group.group.name,
            summarize_attempts(&attempts)
        );
        let status = last_error.as_ref().and_then(|e| e.status()).unwrap_or(502);
        write_json(conn, status, &openai_error(&summary, "gateway_error", "all_models_failed"), &[]);
        StreamOutcome {
            ok: false,
            status: Some(status),
            model: String::new(),
            attempts: attempts.len(),
            usage: Usage { prompt_tokens: Some(prompt_est), completion_tokens: None },
            error: Some(summary),
        }
    });
    push_log(
        host,
        group,
        true,
        started,
        outcome.ok,
        outcome.status,
        outcome.model.clone(),
        outcome.attempts as u32,
        &outcome.usage,
        outcome.error.clone(),
    );
}

// ==================== 集成测试(假上游 + 真服务) ====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DesktopConfig;
    use crate::gateway::{build_snapshot, GroupModel, ModelGroup, RuntimeSnapshot};
    use crate::util::LockExt;

    /// 假上游:每个连接读一个请求,按(脚本序号, 请求体 JSON)回应。
    fn spawn_upstream(script: Arc<dyn Fn(usize, &Value) -> (u16, String, Vec<u8>) + Send + Sync>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut seen = 0usize;
            for conn in listener.incoming().flatten() {
                let script = script.clone();
                let n = seen;
                seen += 1;
                std::thread::spawn(move || {
                    let mut conn = conn;
                    let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
                    let req_body = read_one_request(&mut conn);
                    let parsed: Value = req_body
                        .as_deref()
                        .and_then(|b| serde_json::from_str(b).ok())
                        .unwrap_or(Value::Null);
                    let (status, ctype, body) = script(n, &parsed);
                    let head = format!(
                        "HTTP/1.1 {} {}
Content-Type: {ctype}
Content-Length: {}
Connection: close

",
                        status,
                        if status == 200 { "OK" } else { "Error" },
                        body.len()
                    );
                    let _ = conn.write_all(head.as_bytes());
                    let _ = conn.write_all(&body);
                });
            }
        });
        format!("http://{addr}")
    }

    /// 简化读请求(测试上游不需要 Expect/大 body 处理),返回请求体文本。
    fn read_one_request(conn: &mut TcpStream) -> Option<String> {
        let mut reader = std::io::BufReader::new(conn.try_clone().ok()?);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let mut len = 0usize;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                break;
            }
            let lower = h.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
        if len == 0 {
            return None;
        }
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).ok()?;
        Some(String::from_utf8_lossy(&body).into_owned())
    }

    fn ok_json_body(model: &str, text: &str) -> Vec<u8> {
        json!({
            "id": "chatcmpl-upstream", "object": "chat.completion", "model": model,
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": text }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 3, "completion_tokens": 4 }
        })
        .to_string()
        .into_bytes()
    }

    fn sse_body() -> Vec<u8> {
        let mut out = String::new();
        out.push_str("data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"up-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}

");
        out.push_str("data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"up-model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"你好\"},\"finish_reason\":null}]}

");
        out.push_str("data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"up-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3}}

");
        out.push_str("data: [DONE]

");
        out.into_bytes()
    }

    fn custom(base: &str, model: &str, weight: u32) -> GroupModel {
        GroupModel {
            id: format!("gm-{model}"),
            enabled: true,
            weight,
            alias: String::new(),
            provider: "openai".into(),
            base_url: base.into(),
            api_key: "upstream-key".into(),
            model: model.into(),
        }
    }

    fn group_with(id: &str, name: &str, models: Vec<GroupModel>) -> ModelGroup {
        ModelGroup {
            id: id.into(),
            name: name.into(),
            enabled: true,
            key: "tgk-e2e".into(),
            strategy: "priority".into(),
            context_window: 4096,
            max_output: 512,
            temperature: None,
            system_prompt: String::new(),
            timeout_seconds: 10,
            models,
        }
    }

    /// 建一个带快照的宿主,返回(宿主, 解析后的组)。
    fn make_host(group: ModelGroup) -> (GatewayHost, RuntimeGroup) {
        let host = GatewayHost::new();
        let cfg = DesktopConfig {
            gateway: crate::gateway::GatewaySettings { enabled: true, port: 0, groups: vec![group] },
            models: serde_json::json!([]),
            ..Default::default()
        };
        let full = build_snapshot(&cfg, std::path::Path::new(""));
        let rg = full.groups[0].clone();
        *host.0.snapshot.lock_ok() = Arc::new(RuntimeSnapshot { settings: full.settings.clone(), groups: vec![rg.clone()] });
        (host, rg)
    }

    fn incoming_body(stream: bool) -> Value {
        json!({
            "model": "any",
            "stream": stream,
            "messages": [{ "role": "user", "content": "ping" }]
        })
    }

    #[test]
    fn failover_moves_to_next_candidate_and_logs() {
        // 权重 9 的上游恒 500;权重 5 的正常应答 → 请求应成功且记录 2 次尝试
        let bad = spawn_upstream(Arc::new(|_, _| {
            (500, "application/json".into(), br#"{"error":{"message":"boom"}}"#.to_vec())
        }));
        let good = spawn_upstream(Arc::new(|_, _| (200, "application/json".into(), ok_json_body("good-model", "pong"))));
        let (host, rt) = make_host(group_with(
            "mg-t1",
            "组T",
            vec![custom(&bad, "bad-model", 9), custom(&good, "good-model", 5)],
        ));
        let result = tauri::async_runtime::block_on(run_buffered(&host, &rt, incoming_body(false)));
        let ok = result.ok().expect("故障切换后应成功");
        assert_eq!(ok.model, "good-model");
        assert_eq!(ok.attempts, 2);
        assert_eq!(ok.body.pointer("/choices/0/message/content").and_then(Value::as_str), Some("pong"));
        // 日志:1 条,2 次尝试,成功;prompt 估算被上游真实 usage 覆盖(上游为准)
        let log = host.0.log.lock_ok();
        assert_eq!(log.len(), 1);
        let entry = &log[0];
        assert!(entry.ok);
        assert_eq!(entry.attempts, 2);
        assert_eq!(entry.prompt_tokens, Some(3), "上游 usage.prompt_tokens 优先于字符估算");
        assert_eq!(entry.completion_tokens, Some(4), "上游 usage 透传");
        // 计数
        let counters = host.0.counters.lock_ok();
        let c = counters.get("mg-t1").unwrap();
        assert_eq!((c.total, c.ok, c.fail, c.failovers), (1, 1, 0, 1));
        // 健康簿:bad-model 连败 1;good-model 成功无记录
        let health = host.0.health.lock_ok();
        let bad = health.get("mg-t1/gm-bad-model").unwrap();
        assert_eq!(bad.consecutive_failures, 1);
        assert!(health.get("mg-t1/gm-good-model").is_none());
    }

    #[test]
    fn all_candidates_failing_returns_last_upstream_status() {
        let bad1 = spawn_upstream(Arc::new(|_, _| {
            (429, "application/json".into(), br#"{"error":{"message":"rate limited"}}"#.to_vec())
        }));
        let bad2 = spawn_upstream(Arc::new(|_, _| {
            (500, "application/json".into(), br#"{"error":{"message":"boom"}}"#.to_vec())
        }));
        let (host, rt) = make_host(group_with("mg-t2", "组T2", vec![custom(&bad1, "b1", 9), custom(&bad2, "b2", 5)]));
        let result = tauri::async_runtime::block_on(run_buffered(&host, &rt, incoming_body(false)));
        let fail = result.err().expect("全部失败应返回 Err");
        assert_eq!(fail.status, 500, "保留最后一个上游状态; summary={}", fail.summary);
        assert!(fail.summary.contains("rate limited"), "摘要含首次尝试错误: {}", fail.summary);
        assert!(fail.summary.contains("boom"));
        assert_eq!(fail.attempts, 2);
    }

    /// 端到端:真服务 + 假上游,覆盖 HTTP 解析/鉴权/models/非流式/流式。
    #[test]
    fn end_to_end_http_and_sse() {
        let good = spawn_upstream(Arc::new(|_, req| {
            let stream = req.get("stream").and_then(Value::as_bool).unwrap_or(false);
            if stream {
                (200, "text/event-stream".into(), sse_body())
            } else {
                (200, "application/json".into(), ok_json_body("up-model", "hello"))
            }
        }));
        let (host, _rt) = make_host(group_with("mg-e2e", "e2e组", vec![custom(&good, "up-model", 1)]));
        let handle = start(host.clone(), 0).expect("启动服务");
        let port = handle.port;

        let call = |method: &str, path: &str, key: Option<&str>, body: Option<Vec<u8>>| -> (u16, String, Vec<u8>) {
            let mut conn = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            let mut req = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
            if let Some(k) = key {
                req.push_str(&format!("Authorization: Bearer {k}\r\n"));
            }
            if let Some(b) = &body {
                req.push_str(&format!("Content-Length: {}\r\n", b.len()));
            }
            req.push_str("\r\n");
            conn.write_all(req.as_bytes()).unwrap();
            if let Some(b) = body {
                conn.write_all(&b).unwrap();
            }
            let mut resp = Vec::new();
            conn.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
            let _ = conn.read_to_end(&mut resp);
            let split = resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(0);
            let head = String::from_utf8_lossy(&resp[..split]).to_string();
            let status: u16 = head
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            (status, head, resp[split + 4..].to_vec())
        };

        // /health 免鉴权
        let (status, _, body) = call("GET", "/health", None, None);
        assert_eq!(status, 200);
        assert!(String::from_utf8_lossy(&body).contains("\"ok\":true"));

        // 鉴权:错 Key 一律 401(models 与对话皆是)
        let (status, _, _) = call("GET", "/v1/models", Some("tgk-wrong"), None);
        assert_eq!(status, 401);
        let (status, _, _) = call(
            "POST",
            "/v1/chat/completions",
            Some("tgk-wrong"),
            Some(incoming_body(false).to_string().into_bytes()),
        );
        assert_eq!(status, 401);

        // /v1/models 列出启用组(含组级上下文)
        let (status, _, body) = call("GET", "/v1/models", Some("tgk-e2e"), None);
        assert_eq!(status, 200);
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("e2e组"), "组名即模型 id: {text}");
        assert!(text.contains("4096"), "context_length 外显组级上下文: {text}");

        // 非流式对话
        let (status, _, body) = call(
            "POST",
            "/v1/chat/completions",
            Some("tgk-e2e"),
            Some(incoming_body(false).to_string().into_bytes()),
        );
        assert_eq!(status, 200);
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("hello"), "上游应答透传: {text}");
        assert!(text.contains("chat.completion"));

        // 流式对话:SSE chunk 词汇 + [DONE]
        let (status, _, body) = call(
            "POST",
            "/v1/chat/completions",
            Some("tgk-e2e"),
            Some(incoming_body(true).to_string().into_bytes()),
        );
        assert_eq!(status, 200);
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("chat.completion.chunk"), "流式 chunk: {text}");
        assert!(text.contains("你好"));
        assert!(text.contains("[DONE]"));

        // 非法请求体
        let (status, _, _) = call("POST", "/v1/chat/completions", Some("tgk-e2e"), Some(b"{broken".to_vec()));
        assert_eq!(status, 400);
        let (status, _, _) = call("POST", "/v1/chat/completions", Some("tgk-e2e"), Some(br#"{"model":"x"}"#.to_vec()));
        assert_eq!(status, 400, "缺 messages 数组");

        // 未知路径
        let (status, _, _) = call("GET", "/v1/embeddings", Some("tgk-e2e"), None);
        assert_eq!(status, 404);

        handle.stop();
    }

    /// 流式 + 故障切换:权重高的上游恒 500,流式请求应切换到健康的上游。
    #[test]
    fn streaming_fails_over_before_first_byte() {
        let bad = spawn_upstream(Arc::new(|_, _| {
            (500, "application/json".into(), br#"{"error":{"message":"boom"}}"#.to_vec())
        }));
        let good = spawn_upstream(Arc::new(|_, _| (200, "text/event-stream".into(), sse_body())));
        let (host, _rt) = make_host(group_with(
            "mg-sse",
            "流式组",
            vec![custom(&bad, "bad-model", 9), custom(&good, "up-model", 5)],
        ));
        let handle = start(host.clone(), 0).unwrap();
        let mut conn = std::net::TcpStream::connect(("127.0.0.1", handle.port)).unwrap();
        let body = incoming_body(true).to_string().into_bytes();
        let req = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer tgk-e2e\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        conn.write_all(req.as_bytes()).unwrap();
        conn.write_all(&body).unwrap();
        let mut resp = Vec::new();
        conn.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        let _ = conn.read_to_end(&mut resp);
        let text = String::from_utf8_lossy(&resp);
        assert!(text.starts_with("HTTP/1.1 200 OK"), "切换后应成功: {}", text.lines().next().unwrap_or(""));
        assert!(text.contains("text/event-stream"));
        assert!(text.contains("chat.completion.chunk"));
        assert!(text.contains("[DONE]"));
        handle.stop();
    }
}
