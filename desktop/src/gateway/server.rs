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

use std::io::{BufRead as _, Read, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
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
                    // Windows 会把非阻塞 listen 的状态继承到 accept 出的套接字。
                    // 不改回阻塞的话,引擎 1MB 对话体还没写完,read_exact 就会
                    // WouldBlock → Abandoned → RST,表现成「标题成功、任务无响应」。
                    let _ = conn.set_nonblocking(false);
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
    // 头阶段用 take(32KB) 限幅防无界头;**读体必须脱离 take**:
    // take 是包在 reader 外的适配器,沿用 head 读 119KB 的引擎请求体会在
    // 32KB 处假 EOF → Abandoned → 静默关连接(实测:引擎 41 条消息 + 82
    // 工具的请求 14ms 内被重置,引擎重试 12 次全挂)。头解析完即 drop
    // take 适配器,体从裸 reader 读(BufReader 缓冲里的预读不丢——同一条
    // reader 链)。
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
    // drop take 适配器(head 移动进 drop 就此结束),后续读体不再受限幅
    drop(head);
    // 许多客户端在长请求体无法预先知道长度时会自动使用
    // Transfer-Encoding: chunked。ohmyagent 的长上下文请求正是这种形态;
    // 直接回 411 会让客户端还在发送 body 时看到连接被重置,表现成
    // 「第一条请求成功,第二条请求永远不再继续」。这里在同一个
    // BufReader 上解码分块体,否则头阶段的预读字节也会被丢掉。
    if chunked && content_length.is_some() {
        return Err(ReadError::Status(400, "请求同时包含 Content-Length 和 chunked 编码"));
    }
    if expect_continue && chunked {
        let _ = conn.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    if chunked {
        let body = read_chunked_body(&mut reader)?;
        return Ok(HttpReq { method, path, bearer, body });
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
    read_exact_all(&mut reader, &mut body)?;
    Ok(HttpReq { method, path, bearer, body })
}

fn read_exact_all(reader: &mut impl Read, buf: &mut [u8]) -> Result<(), ReadError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut off = 0;
    while off < buf.len() {
        match reader.read(&mut buf[off..]) {
            Ok(0) => return Err(ReadError::Abandoned),
            Ok(n) => off += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err(ReadError::Abandoned);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return Err(ReadError::Abandoned),
        }
    }
    Ok(())
}

/// 解码 HTTP/1.1 chunked 请求体。每个 chunk 的尺寸行与 trailer 都设有
/// 限长,累计 body 也受 MAX_BODY_BYTES 约束,避免把分块请求当成无限流读取。
fn read_chunked_body(reader: &mut impl std::io::BufRead) -> Result<Vec<u8>, ReadError> {
    fn next_line(reader: &mut impl std::io::BufRead) -> Result<String, ReadError> {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|_| ReadError::Abandoned)?;
        if n == 0 {
            return Err(ReadError::Abandoned);
        }
        if n > MAX_HEADER_BYTES {
            return Err(ReadError::Status(400, "分块请求体的尺寸行或尾部过长"));
        }
        Ok(line)
    }

    let mut body = Vec::new();
    loop {
        let line = next_line(reader)?;
        let size_text = line
            .trim_end_matches(&['\r', '\n'][..])
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = u64::from_str_radix(size_text, 16)
            .map_err(|_| ReadError::Status(400, "分块请求体的尺寸无效"))?;
        if size == 0 {
            // 最后一块后可以跟 trailer headers,直到空行结束。
            for _ in 0..MAX_HEADERS {
                if next_line(reader)?.trim().is_empty() {
                    return Ok(body);
                }
            }
            return Err(ReadError::Status(400, "分块请求体的尾部过多"));
        }
        let size = usize::try_from(size)
            .ok()
            .and_then(|n| body.len().checked_add(n).map(|end| (n, end)))
            .filter(|(_, end)| *end <= MAX_BODY_BYTES)
            .ok_or(ReadError::Status(413, "请求体过大"))?;
        let start = body.len();
        body.resize(size.1, 0);
        reader
            .read_exact(&mut body[start..size.1])
            .map_err(|_| ReadError::Abandoned)?;
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).map_err(|_| ReadError::Abandoned)?;
        if crlf != *b"\r\n" {
            return Err(ReadError::Status(400, "分块请求体缺少 CRLF"));
        }
    }
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
    let _ = conn.set_nonblocking(false);
    let _ = conn.set_nodelay(true);
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
    request_content: Option<String>,
    response_content: Option<String>,
) {
    host.push_log(LogEntry {
        request_id: 0,
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
        request_content: request_content.map(|s| s.chars().take(500).collect()),
        response_content: response_content.map(|s| s.chars().take(500).collect()),
        pending: false,
    });
}

/// 从 incoming 请求体提取用户最后一条消息内容(截断 500 字符)。
fn extract_content(incoming: &Value) -> String {
    incoming
        .pointer("/messages")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.iter().rev().find(|m| m.pointer("/role").and_then(|r| r.as_str()) == Some("user")))
        .and_then(|m| m.pointer("/content").and_then(|c| c.as_str()))
        .unwrap_or("")
        .chars()
        .take(500)
        .collect()
}

/// 从非流式响应体提取回答内容(截断 500 字符)。
fn extract_response_content(body: &Value) -> String {
    body.pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(500)
        .collect()
}

/// 请求开始时推一条 pending 日志(2026-09-14):
/// ok=false/pending=true/latency=0,前端展示"请求中"态。
fn push_pending_log(host: &GatewayHost, group: &RuntimeGroup, stream: bool, request_content: Option<String>) -> u64 {
    let request_id = host.next_log_seq();
    let initial_model = group
        .candidates
        .iter()
        .find(|c| c.unavailable.is_none())
        .map(|c| if c.model.is_empty() { c.label.clone() } else { c.model.clone() })
        .unwrap_or_default();
    host.push_log(LogEntry {
        request_id,
        ts_ms: super::now_ms(),
        group_id: group.group.id.clone(),
        group_name: group.group.name.clone(),
        stream,
        ok: false,
        status: None,
        latency_ms: 0,
        model: initial_model,
        attempts: 0,
        prompt_tokens: None,
        completion_tokens: None,
        error: None,
        request_content: request_content.map(|s| s.chars().take(500).collect()),
        response_content: None,
        pending: true,
    });
    request_id
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
    // 立即写一条 pending 日志(2026-09-14):前端可实时看到"请求中"态。
    let request_id = push_pending_log(host, group, false, Some(extract_content(&incoming)));
    let ctx = GroupCtx::of(&group.group);
    let timeout = group.group.effective_timeout();
    let mut attempts = unavailable_notes(group);
    let prompt_est = upstream::estimate_tokens(upstream::incoming_prompt_chars(&incoming));

    let mut rng = host.rng_next();
    let plan = {
        let health = host.health_map();
        let lats = host.latencies();
        let strat = group.group.effective_strategy();
        let rr_start = if strat == super::STRATEGY_BALANCED {
            Some(host.rr_next(&group.group.id, group.candidates.len()))
        } else { None };
        sched::plan(
            strat,
            &group.group.id,
            &group.candidates,
            &health,
            super::now_ms(),
            &mut rng,
            &lats,
            rr_start,
        )
    };
    let mut last_error: Option<UpstreamError> = None;
    let mut usage_out = Usage { prompt_tokens: Some(prompt_est), completion_tokens: None };
    for cand in &plan {
        host.update_pending_model(request_id, &cand.model);
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
                // 更新 pending 条目(而不是追加新条目),使前端"请求中"态翻为完成
                // Use complete_and_persist to also write to JSONL with full bodies.
                host.complete_and_persist(request_id, &group.group.id, &group.group.name, false, true, Some(200), started.elapsed().as_millis() as u64,
                    &reply.model, attempts.len() as u32, &usage_out, None,
                    Some(extract_content(&incoming)), Some(extract_response_content(&reply.body)),
                    Some(incoming.to_string()), Some(reply.body.to_string()));
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
    // 更新 pending 条目(而不是追加新条目)
    // Use complete_and_persist to also write to JSONL (no response body on failure).
    host.complete_and_persist(request_id, &group.group.id, &group.group.name, false, false, Some(status), started.elapsed().as_millis() as u64,
        &model, attempts_n, &usage_out, Some(summary.clone()),
        Some(extract_content(&incoming)), None,
        Some(incoming.to_string()), None);
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
    response_content: String,
    raw_response: String,
}

fn handle_streaming(
    conn: &mut TcpStream,
    host: &GatewayHost,
    group: &RuntimeGroup,
    incoming: Value,
    started: std::time::Instant,
) {
    // 立即写一条 pending 日志(2026-09-14)。
    let request_id = push_pending_log(host, group, true, Some(extract_content(&incoming)));
    let ctx = GroupCtx::of(&group.group);
    let timeout = group.group.effective_timeout();
    let mut attempts = unavailable_notes(group);
    let prompt_est = upstream::estimate_tokens(upstream::incoming_prompt_chars(&incoming));

    let outcome = tauri::async_runtime::block_on(async {
        let mut rng = host.rng_next();
        let plan = {
            let health = host.health_map();
            let lats = host.latencies();
            let strat = group.group.effective_strategy();
            let rr_start = if strat == super::STRATEGY_BALANCED {
                Some(host.rr_next(&group.group.id, group.candidates.len()))
            } else { None };
            sched::plan(
                strat,
                &group.group.id,
                &group.candidates,
                &health,
                super::now_ms(),
                &mut rng,
                &lats,
                rr_start,
            )
        };
        let mut last_error: Option<UpstreamError> = None;
        for cand in &plan {
            host.update_pending_model(
                request_id,
                if cand.model.is_empty() { &cand.label } else { &cand.model },
            );
            match upstream::open_stream(host.client(), cand, &incoming, &ctx, timeout).await {
                Ok((reply, model)) => {
                    let display_model = if model.is_empty() { cand.label.clone() } else { model.clone() };
                    host.record_attempt(&group.group.id, &cand.id, true);
                    attempts.push(AttemptNote {
                        label: cand.label.clone(),
                        model: if model.is_empty() { cand.label.clone() } else { model.clone() },
                        ok: true,
                        message: String::new(),
                    });
                    // 首字节前是最后一次换模型的机会:SSE 头现在落笔
                    write_sse_head(
                        conn,
                        &[
                            ("X-Gateway-Group", group.group.name.clone()),
                            ("X-Gateway-Model", display_model.clone()),
                        ],
                    );
                    let mut write_conn = match conn.try_clone() {
                        Ok(c) => c,
                        Err(e) => {
                            return StreamOutcome {
                                ok: false,
                                status: Some(200),
                                model: display_model.clone(),
                                attempts: attempts.len(),
                                usage: Usage { prompt_tokens: Some(prompt_est), completion_tokens: None },
                                error: Some(format!("客户端连接不可写: {e}")),
                                response_content: String::new(),
                                raw_response: String::new(),
                            };
                        }
                    };
                    let write = move |bytes: &[u8]| -> std::io::Result<()> {
                        write_conn.write_all(bytes)?;
                        write_conn.flush()
                    };
                    let relay = upstream::relay_stream(reply, display_model.clone(), write, timeout.max(Duration::from_secs(60))).await;
                    return match relay {
                        Ok(summary) => StreamOutcome {
                            ok: summary.error.is_none(),
                            status: Some(200),
                            model: summary.response_model.clone().unwrap_or_else(|| {
                                display_model.clone()
                            }),
                            attempts: attempts.len(),
                            usage: Usage {
                                prompt_tokens: Some(prompt_est),
                                completion_tokens: summary
                                    .usage
                                    .completion_tokens
                                    .or_else(|| Some(upstream::estimate_tokens(summary.completion_chars))),
                            },
                            error: summary.error,
                            response_content: summary.response_content,
                            raw_response: summary.raw_response,
                        },
                        Err(e) => StreamOutcome {
                            ok: false,
                            status: Some(200),
                            model: display_model,
                            attempts: attempts.len(),
                            usage: Usage { prompt_tokens: Some(prompt_est), completion_tokens: None },
                            error: Some(e),
                            response_content: String::new(),
                            raw_response: String::new(),
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
            response_content: String::new(),
            raw_response: String::new(),
        }
    });
    // 流式响应以 Connection: close 定界。引擎在 complete_and_persist
    // 落盘之前就必须看到 EOF，否则会一直等下一个 delta。
    let _ = conn.flush();
    let _ = conn.shutdown(Shutdown::Write);
    // 更新 pending 条目(而不是追加新条目),与 run_buffered 同款
    // Use complete_and_persist to also write to JSONL. For streaming, the
    // Store the captured stream content in the same detail fields as buffered
    // requests so streamed work is inspectable too.
    host.complete_and_persist(
        request_id,
        &group.group.id,
        &group.group.name,
        true,
        outcome.ok,
        outcome.status,
        started.elapsed().as_millis() as u64,
        &outcome.model,
        outcome.attempts as u32,
        &outcome.usage,
        outcome.error.clone(),
        Some(extract_content(&incoming)),
        (!outcome.response_content.is_empty()).then_some(outcome.response_content),
        Some(incoming.to_string()),
        (!outcome.raw_response.is_empty()).then_some(outcome.raw_response),
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
            log_enabled: true,
            models,
        }
    }

    /// 建一个带快照的宿主,返回(宿主, 解析后的组)。
    fn make_host(group: ModelGroup) -> (GatewayHost, RuntimeGroup) {
        let host = GatewayHost::new();
        let cfg = DesktopConfig {
            gateway: crate::gateway::GatewaySettings { enabled: true, port: 0, groups: vec![group], vendor_presets: vec![] },
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

        // 大请求体(> MAX_HEADER_BYTES 的 take 限幅,回归 2026-09-07 引擎
        // 119KB 请求被网关静默重置的 bug):体读取必须不受头限幅影响
        let mut big = incoming_body(false);
        let filler = "x".repeat(64 * 1024);
        big["messages"][0]["content"] = Value::String(format!("长上下文 {filler}"));
        let big_bytes = big.to_string().into_bytes();
        assert!(big_bytes.len() > MAX_HEADER_BYTES);
        let (status, _, body) = call("POST", "/v1/chat/completions", Some("tgk-e2e"), Some(big_bytes));
        assert_eq!(status, 200, "大 body 请求必须完整读取并转发(不再 14ms 重置)");
        assert!(String::from_utf8_lossy(&body).contains("hello"));

        // 长上下文客户端可能使用 Transfer-Encoding: chunked;网关必须
        // 解码后继续走正常鉴权与转发,不能在客户端发送 body 时重置连接。
        let chunked_body = incoming_body(false).to_string().into_bytes();
        let mut chunked_conn = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let chunked_head = "POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\n\
Authorization: Bearer tgk-e2e\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        chunked_conn.write_all(chunked_head.as_bytes()).unwrap();
        chunked_conn
            .write_all(format!("{:X}\r\n", chunked_body.len()).as_bytes())
            .unwrap();
        chunked_conn.write_all(&chunked_body).unwrap();
        chunked_conn.write_all(b"\r\n0\r\n\r\n").unwrap();
        let mut chunked_resp = Vec::new();
        chunked_conn.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        let _ = chunked_conn.read_to_end(&mut chunked_resp);
        let chunked_text = String::from_utf8_lossy(&chunked_resp);
        assert!(chunked_text.starts_with("HTTP/1.1 200 OK"), "chunked body 应正常转发: {chunked_text}");
        assert!(chunked_text.contains("hello"));

        // 引擎真实对话约 1MB,且 Go 会先发 Expect: 100-continue 再慢慢写 body。
        // 头到齐、体尚未到达时绝不能 Abandoned/RST。
        let mut slow_json = incoming_body(false);
        slow_json["messages"][0]["content"] = Value::String("x".repeat(256 * 1024));
        let slow_body = slow_json.to_string().into_bytes();
        let mut slow = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let slow_head = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\n\
Authorization: Bearer tgk-e2e\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
            slow_body.len()
        );
        slow.write_all(slow_head.as_bytes()).unwrap();
        slow.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        for chunk in slow_body.chunks(8 * 1024) {
            slow.write_all(chunk).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut slow_resp = Vec::new();
        let _ = slow.read_to_end(&mut slow_resp);
        let slow_text = String::from_utf8_lossy(&slow_resp);
        assert!(
            slow_text.contains("HTTP/1.1 200 OK"),
            "分片大 body 应转发成功,不能 RST: {slow_text}"
        );
        assert!(slow_text.contains("hello"));

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

    /// 自定义上游常忽略 stream=true，直接回一条带 tool_calls 的
    /// chat.completion。必须改写成 chunk，否则引擎空等增量、工作区无输出。
    #[test]
    fn streaming_json_tool_completion_is_rewritten_as_chunks() {
        let good = spawn_upstream(Arc::new(|_, _| {
            let body = json!({
                "id": "cmpl-1",
                "object": "chat.completion",
                "model": "up-model",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call-1",
                            "type": "function",
                            "function": { "name": "read_file", "arguments": "{\"path\":\"a.txt\"}" }
                        }]
                    },
                    "finish_reason": "stop"
                }]
            });
            (200, "application/json".into(), body.to_string().into_bytes())
        }));
        let (host, _rt) = make_host(group_with("mg-json", "json组", vec![custom(&good, "up-model", 1)]));
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
        assert!(text.contains("text/event-stream"), "{text}");
        assert!(text.contains("chat.completion.chunk"), "应改写成 chunk: {text}");
        assert!(text.contains("\"tool_calls\""), "必须带上工具调用: {text}");
        assert!(text.contains("call-1"));
        assert!(text.contains("\"finish_reason\":\"tool_calls\"") || text.contains("\"finish_reason\": \"tool_calls\""), "{text}");
        assert!(text.contains("[DONE]"));
        handle.stop();
    }
}
