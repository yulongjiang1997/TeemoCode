// 上游适配层:网关对外只说 OpenAI Chat Completions,组内模型却可能是三种
// 协议(openai / anthropic / openai_responses)——这里做双向翻译:
//   请求:  OpenAI 入参 → 各协议请求体(钳 max_tokens、套组级温度/系统提示词)
//   应答:  各协议应答 → OpenAI 应答(非流式 JSON / 流式 SSE 事件翻译)
// openai 协议走**原样中继**(不改写应答字节,旁路嗅探 usage 记账)。
//
// 纯函数(body 构建、事件翻译)与 IO(send/relay)分离:前者单测对表,
// 后者由 server.rs 的集成测试用假上游覆盖。
//
// 超时语义:单次尝试的 tokio timeout 由调用方施加于「send + 读头」;
// 流式中继按 idle 超时逐块守——上游整体不限时(长生成是常态)。

use std::io;
use std::time::Duration;

use serde_json::{json, Value};

use super::ResolvedCandidate;

/// 组级共享上下文(生效值,由 ModelGroup 的 effective_* 派生)。
#[derive(Clone, Debug)]
pub(crate) struct GroupCtx {
    pub max_output: i64,
    pub temperature: Option<f64>,
    pub system_prompt: String,
}

impl GroupCtx {
    pub fn of(group: &super::ModelGroup) -> Self {
        Self {
            max_output: group.effective_max_output(),
            temperature: group.temperature,
            system_prompt: group.system_prompt.trim().to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Protocol {
    OpenAi,
    Anthropic,
    Responses,
}

pub(crate) fn normalize_provider(p: &str) -> Protocol {
    match p {
        "anthropic" => Protocol::Anthropic,
        "openai_responses" => Protocol::Responses,
        _ => Protocol::OpenAi,
    }
}

/// 粗估 token 数(中英混合折中 ≈ 3 字符/token)。仅用于日志展示与
/// 请求规模记录,不参与任何截断决策。
pub(crate) fn estimate_tokens(chars: usize) -> i64 {
    ((chars / 3) as i64).max(1)
}

// ==================== 错误与用量 ====================

#[derive(Debug, Clone)]
pub(crate) enum UpstreamError {
    /// 连接/请求层失败。
    Connect(String),
    /// 尝试超时。
    Timeout,
    /// 上游返回非 2xx。
    Status { code: u16, message: String },
    /// 请求体无法构造(如 incoming 非法)。
    Invalid(String),
}

impl UpstreamError {
    pub fn message(&self) -> String {
        match self {
            UpstreamError::Connect(e) => format!("无法连接上游: {e}"),
            UpstreamError::Timeout => "上游尝试超时".to_string(),
            UpstreamError::Status { code, message } => format!("HTTP {code}: {message}"),
            UpstreamError::Invalid(m) => m.clone(),
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            UpstreamError::Status { code, .. } => Some(*code),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Usage {
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
}

impl Usage {
    pub fn merge(&mut self, other: &Usage) {
        if other.prompt_tokens.is_some() {
            self.prompt_tokens = other.prompt_tokens;
        }
        if other.completion_tokens.is_some() {
            self.completion_tokens = other.completion_tokens;
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

/// 从上游错误响应体提取人读信息(openai/anthropic/responses 三种形态)。
pub(crate) fn extract_error_message(body: &Value, raw: &str) -> String {
    let msg = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .or_else(|| body.get("message").and_then(Value::as_str))
        .unwrap_or("");
    if !msg.is_empty() {
        return truncate(msg, 300);
    }
    let compact = raw.trim();
    if compact.is_empty() {
        "上游无错误详情".to_string()
    } else {
        truncate(compact, 200)
    }
}

// ==================== base_url 与端点 ====================

/// 与 model_test/models_fetch 同口径:尾斜杠归一、剥习惯性 /v1;base 末段
/// 已是版本段(如智谱 /api/coding/paas/v4)时不重复补 /v1。
pub(crate) fn endpoint_url(base_url: &str, path: &str) -> String {
    let base = base_url.trim().trim_end_matches('/').trim_end_matches("/v1").trim_end_matches('/');
    let seg = base.rsplit('/').next().unwrap_or("");
    let versioned = seg.len() >= 2 && seg.starts_with('v') && seg[1..].bytes().all(|c| c.is_ascii_digit());
    if versioned {
        format!("{base}/{path}")
    } else {
        format!("{base}/v1/{path}")
    }
}

// ==================== 入参解析(OpenAI Chat Completions 形态) ====================

#[derive(Clone, Debug)]
pub(crate) struct ConvMessage {
    pub role: String,
    /// string 或 parts 数组(原样保留,转换器各自解释)。
    pub content: Value,
}

pub(crate) fn incoming_messages(incoming: &Value) -> Result<Vec<ConvMessage>, UpstreamError> {
    let arr = incoming
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| UpstreamError::Invalid("请求缺少 messages 数组".into()))?;
    let mut out = Vec::with_capacity(arr.len());
    for m in arr {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("").to_string();
        if role.is_empty() {
            return Err(UpstreamError::Invalid("message 缺少 role".into()));
        }
        let content = m.get("content").cloned().unwrap_or(Value::Null);
        out.push(ConvMessage { role, content });
    }
    Ok(out)
}

/// OpenAI content(string 或 parts)的文本总量(字符)——日志估算用。
pub(crate) fn incoming_prompt_chars(incoming: &Value) -> usize {
    incoming_messages(incoming)
        .map(|msgs| {
            msgs.iter()
                .map(|m| match &m.content {
                    Value::String(s) => s.chars().count(),
                    Value::Array(parts) => parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .map(str::chars)
                        .map(Iterator::count)
                        .sum::<usize>(),
                    _ => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

/// 抽取系统提示:ctx.system_prompt(在前) + 请求里前导 system 消息,合并。
/// 返回(剩余消息, 系统文本)。
fn split_system(
    mut msgs: Vec<ConvMessage>,
    extra_system: &str,
) -> (Vec<ConvMessage>, Option<String>) {
    let mut system_parts: Vec<String> = vec![];
    if !extra_system.is_empty() {
        system_parts.push(extra_system.to_string());
    }
    let mut rest = Vec::with_capacity(msgs.len());
    let mut leading = true;
    for m in msgs.drain(..) {
        if leading && m.role == "system" {
            match &m.content {
                Value::String(s) => system_parts.push(s.clone()),
                Value::Array(parts) => {
                    let text: String = parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        system_parts.push(text);
                    }
                }
                _ => {}
            }
            continue;
        }
        leading = false;
        rest.push(m);
    }
    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };
    (rest, system)
}

/// 钳制整型字段到 max_output(仅当超出时改写)。
fn clamp_field(obj: &mut serde_json::Map<String, Value>, key: &str, max_output: i64) {
    if let Some(v) = obj.get(key).and_then(Value::as_i64) {
        if v > max_output {
            obj.insert(key.to_string(), json!(max_output));
        }
    }
}

fn text_of_openai_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

// ==================== 请求体构建 ====================

/// openai 协议:原样透传 + 钳制 + 组级注入。
pub(crate) fn openai_chat_body(
    incoming: &Value,
    target_model: &str,
    ctx: &GroupCtx,
    stream: bool,
) -> Result<Value, UpstreamError> {
    let msgs = incoming_messages(incoming)?;
    let mut body = incoming.clone();
    let obj = body
        .as_object_mut()
        .ok_or_else(|| UpstreamError::Invalid("请求体必须是 JSON 对象".into()))?;
    obj.insert("model".into(), json!(target_model));
    obj.insert("stream".into(), json!(stream));
    if !stream {
        // stream_options 只在流式下合法;非流式请求带着它会被部分网关拒绝
        obj.remove("stream_options");
    }
    clamp_field(obj, "max_tokens", ctx.max_output);
    clamp_field(obj, "max_completion_tokens", ctx.max_output);
    if let Some(t) = ctx.temperature {
        obj.entry("temperature".to_string()).or_insert(json!(t));
    }
    // 组级系统提示词前置:并入前导 system 消息,不破坏其余消息
    if !ctx.system_prompt.is_empty() {
        let (rest, system) = split_system(msgs, &ctx.system_prompt);
        let mut out_msgs = vec![];
        if let Some(s) = system {
            out_msgs.push(json!({ "role": "system", "content": s }));
        }
        for m in rest {
            out_msgs.push(json!({ "role": m.role, "content": m.content }));
        }
        obj.insert("messages".into(), Value::Array(out_msgs));
    }
    Ok(body)
}

/// data:image/...;base64,DATA → (media_type, data)。
fn parse_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mt = meta.strip_suffix(";base64")?;
    Some((mt.to_string(), data.to_string()))
}

fn openai_image_url(part: &Value) -> Option<String> {
    match part.get("image_url") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(v) => v.get("url").and_then(Value::as_str).map(str::to_string),
        None => None,
    }
}

/// OpenAI parts → Anthropic content blocks(文本/图片;data URL 转 base64
/// source,http URL 用 anthropic 的 url source)。
fn anthropic_blocks(content: &Value) -> Vec<Value> {
    let mut blocks: Vec<Value> = vec![];
    match content {
        Value::String(s) => blocks.push(json!({ "type": "text", "text": s })),
        Value::Array(parts) => {
            for p in parts {
                match p.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(t) = p.get("text").and_then(Value::as_str) {
                            blocks.push(json!({ "type": "text", "text": t }));
                        }
                    }
                    "image_url" => {
                        if let Some(url) = openai_image_url(p) {
                            if let Some((mt, data)) = parse_data_url(&url) {
                                blocks.push(json!({
                                    "type": "image",
                                    "source": { "type": "base64", "media_type": mt, "data": data }
                                }));
                            } else {
                                blocks.push(json!({
                                    "type": "image",
                                    "source": { "type": "url", "url": url }
                                }));
                            }
                        }
                    }
                    // audio 等暂不支持的部件退化为占位文本,不静默丢弃
                    other => {
                        blocks.push(json!({ "type": "text", "text": format!("[不支持的内容类型: {other}]") }));
                    }
                }
            }
        }
        Value::Null => {}
        other => blocks.push(json!({ "type": "text", "text": text_of_openai_content(other) })),
    }
    if blocks.is_empty() {
        blocks.push(json!({ "type": "text", "text": "" }));
    }
    blocks
}

/// OpenAI messages → Anthropic /v1/messages 请求体。
pub(crate) fn anthropic_messages_body(
    incoming: &Value,
    target_model: &str,
    ctx: &GroupCtx,
    stream: bool,
) -> Result<Value, UpstreamError> {
    let msgs = incoming_messages(incoming)?;
    let (rest, system) = split_system(msgs, &ctx.system_prompt);
    let mut messages: Vec<Value> = vec![];
    for m in rest {
        // tool 角色退化成 user 文本(工具语义不经网关透传,保对话不崩)
        let role = match m.role.as_str() {
            "assistant" => "assistant",
            _ => "user",
        };
        let mut blocks = anthropic_blocks(&m.content);
        if m.role == "tool" {
            blocks.insert(
                0,
                json!({ "type": "text", "text": "[工具结果]" }),
            );
        }
        // anthropic 要求相邻消息角色交替;连续同角色合并进上一条
        if let Some(prev) = messages.last_mut() {
            if prev.get("role").and_then(Value::as_str) == Some(role) {
                if let Some(arr) = prev.get_mut("content").and_then(Value::as_array_mut) {
                    arr.extend(blocks);
                    continue;
                }
            }
        }
        messages.push(json!({ "role": role, "content": blocks }));
    }
    // 请求里显式的 max_tokens 优先(钳制),否则用组级 max_output
    let requested = incoming.get("max_tokens").and_then(Value::as_i64);
    let max_tokens = requested.map(|v| v.min(ctx.max_output)).unwrap_or(ctx.max_output).max(1);
    let mut body = serde_json::Map::new();
    body.insert("model".into(), json!(target_model));
    body.insert("max_tokens".into(), json!(max_tokens));
    body.insert("stream".into(), json!(stream));
    body.insert("messages".into(), Value::Array(messages));
    if let Some(s) = system {
        body.insert("system".into(), json!(s));
    }
    if let Some(t) = ctx.temperature {
        if incoming.get("temperature").map(|v| v.is_null()).unwrap_or(true) {
            body.insert("temperature".into(), json!(t));
        }
    }
    Ok(Value::Object(body))
}

/// OpenAI messages → Responses /v1/responses 请求体。
pub(crate) fn responses_body(
    incoming: &Value,
    target_model: &str,
    ctx: &GroupCtx,
    stream: bool,
) -> Result<Value, UpstreamError> {
    let msgs = incoming_messages(incoming)?;
    let (rest, system) = split_system(msgs, &ctx.system_prompt);
    let mut input: Vec<Value> = vec![];
    for m in rest {
        let role = if m.role == "assistant" { "assistant" } else { "user" };
        let mut parts: Vec<Value> = vec![];
        match &m.content {
            Value::String(s) => parts.push(json!({ "type": "input_text", "text": s })),
            Value::Array(items) => {
                for p in items {
                    match p.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text" => {
                            if let Some(t) = p.get("text").and_then(Value::as_str) {
                                parts.push(json!({ "type": "input_text", "text": t }));
                            }
                        }
                        "image_url" => {
                            if let Some(url) = openai_image_url(p) {
                                parts.push(json!({ "type": "input_image", "image_url": url }));
                            }
                        }
                        other => parts.push(json!({ "type": "input_text", "text": format!("[不支持的内容类型: {other}]") })),
                    }
                }
            }
            Value::Null => {}
            other => parts.push(json!({ "type": "input_text", "text": text_of_openai_content(other) })),
        }
        if parts.is_empty() {
            parts.push(json!({ "type": "input_text", "text": "" }));
        }
        // 相邻同角色合并(responses 的 input 无此硬约束,但保持一致形态)
        if let Some(prev) = input.last_mut() {
            if prev.get("role").and_then(Value::as_str) == Some(role) {
                if let Some(arr) = prev.get_mut("content").and_then(Value::as_array_mut) {
                    arr.extend(parts);
                    continue;
                }
            }
        }
        input.push(json!({ "role": role, "content": parts }));
    }
    let requested = incoming.get("max_tokens").and_then(Value::as_i64).or_else(|| incoming.get("max_completion_tokens").and_then(Value::as_i64));
    let max_out = requested.map(|v| v.min(ctx.max_output)).unwrap_or(ctx.max_output).max(16);
    let mut body = serde_json::Map::new();
    body.insert("model".into(), json!(target_model));
    body.insert("input".into(), Value::Array(input));
    body.insert("stream".into(), json!(stream));
    body.insert("max_output_tokens".into(), json!(max_out));
    if let Some(s) = system {
        body.insert("instructions".into(), json!(s));
    }
    if let Some(t) = ctx.temperature {
        if incoming.get("temperature").map(|v| v.is_null()).unwrap_or(true) {
            body.insert("temperature".into(), json!(t));
        }
    }
    Ok(Value::Object(body))
}

pub(crate) fn build_upstream_body(
    protocol: Protocol,
    incoming: &Value,
    target_model: &str,
    ctx: &GroupCtx,
    stream: bool,
) -> Result<Value, UpstreamError> {
    match protocol {
        Protocol::OpenAi => openai_chat_body(incoming, target_model, ctx, stream),
        Protocol::Anthropic => anthropic_messages_body(incoming, target_model, ctx, stream),
        Protocol::Responses => responses_body(incoming, target_model, ctx, stream),
    }
}

// ==================== 响应解析(非流式) ====================

fn anthropic_stop_to_finish(reason: Option<&str>) -> &'static str {
    match reason {
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        _ => "stop",
    }
}

fn responses_status_to_finish(status: Option<&str>) -> &'static str {
    match status {
        Some("incomplete") => "length",
        _ => "stop",
    }
}

fn completion_body(id: &str, model: &str, text: String, finish: &str, usage: &Usage) -> Value {
    let mut body = json!({
        "id": id,
        "object": "chat.completion",
        "created": super::now_ms() / 1000,
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": text },
            "finish_reason": finish,
        }],
    });
    if usage.prompt_tokens.is_some() || usage.completion_tokens.is_some() {
        body["usage"] = json!({ "prompt_tokens": usage.prompt_tokens.unwrap_or(0), "completion_tokens": usage.completion_tokens.unwrap_or(0) });
    }
    body
}

/// 上游非流式应答 → OpenAI chat.completion。
pub(crate) fn parse_buffered_response(
    protocol: Protocol,
    body: &Value,
) -> (String /*model*/, String /*text*/, &'static str /*finish*/, Usage) {
    match protocol {
        Protocol::OpenAi => {
            let model = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
            let text = body
                .pointer("/choices/0/message/content")
                .map(text_of_openai_content)
                .unwrap_or_default();
            let finish = body
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                .map(|r| match r {
                    "length" => "length",
                    "tool_calls" | "function_call" => "tool_calls",
                    _ => "stop",
                })
                .unwrap_or("stop");
            let usage = Usage {
                prompt_tokens: body.pointer("/usage/prompt_tokens").and_then(Value::as_i64),
                completion_tokens: body.pointer("/usage/completion_tokens").and_then(Value::as_i64),
            };
            (model, text, finish, usage)
        }
        Protocol::Anthropic => {
            let model = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
            let text = body
                .get("content")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            let finish = anthropic_stop_to_finish(body.get("stop_reason").and_then(Value::as_str));
            let usage = Usage {
                prompt_tokens: body.pointer("/usage/input_tokens").and_then(Value::as_i64),
                completion_tokens: body.pointer("/usage/output_tokens").and_then(Value::as_i64),
            };
            (model, text, finish, usage)
        }
        Protocol::Responses => {
            let model = body
                .pointer("/response/model")
                .or_else(|| body.get("model"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let text = body
                .get("output")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter(|s| s.get("type").and_then(Value::as_str) == Some("message"))
                        .filter_map(|s| s.get("content").and_then(Value::as_array))
                        .flatten()
                        .filter(|p| p.get("type").and_then(Value::as_str) == Some("output_text"))
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            let status = body.get("status").and_then(Value::as_str);
            let finish = responses_status_to_finish(status);
            let usage = Usage {
                prompt_tokens: body.pointer("/usage/input_tokens").and_then(Value::as_i64),
                completion_tokens: body.pointer("/usage/output_tokens").and_then(Value::as_i64),
            };
            (model, text, finish, usage)
        }
    }
}

pub(crate) fn to_openai_completion(id: &str, parsed: &(String, String, &'static str, Usage)) -> Value {
    completion_body(id, &parsed.0, parsed.1.clone(), parsed.2, &parsed.3)
}

// ==================== 流式事件翻译 ====================

/// 增量 SSE 解析:按空行分事件,抽出 data 行载荷(多行 data 以 \n 连接)。
#[derive(Default)]
pub(crate) struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = vec![];
        loop {
            let end = match find_event_end(&self.buf) {
                Some(end) => end,
                None => break,
            };
            let event: Vec<u8> = self.buf.drain(..end).collect();
            let data = event_data(&event);
            if !data.is_empty() {
                out.push(data);
            }
        }
        out
    }
}

fn find_event_end(buf: &[u8]) -> Option<usize> {
    // 事件边界 = 空行;三种行尾形态都要认(\r\n\r\n、\n\r\n、\n\n)。
    // 从左向右扫,同位置先试长模式:最早的完整边界胜出。
    for i in 0..buf.len() {
        if buf[i..].starts_with(b"\r\n\r\n") {
            return Some(i + 4);
        }
        if buf[i..].starts_with(b"\n\r\n") {
            return Some(i + 3);
        }
        if buf[i..].starts_with(b"\n\n") {
            return Some(i + 2);
        }
    }
    None
}

fn event_data(event: &[u8]) -> String {
    let text = String::from_utf8_lossy(event);
    let mut lines: Vec<&str> = vec![];
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(v) = line.strip_prefix("data:") {
            lines.push(v.strip_prefix(' ').unwrap_or(v));
        }
    }
    lines.join("\n")
}

/// 流式翻译会话:chunk 元信息(id/model/created)与会话态。
pub(crate) struct StreamSession {
    pub id: String,
    pub model: String,
    pub created: i64,
    role_sent: bool,
}

impl StreamSession {
    pub fn new(id: String, model: String) -> Self {
        Self { id, model, created: (super::now_ms() / 1000) as i64, role_sent: false }
    }

    fn chunk(&self, delta: Value, finish: Option<&str>, usage: Option<Usage>) -> String {
        let mut c = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
        });
        if let Some(u) = usage {
            c["usage"] = json!({
                "prompt_tokens": u.prompt_tokens.unwrap_or(0),
                "completion_tokens": u.completion_tokens.unwrap_or(0),
            });
        }
        c.to_string()
    }

    fn ensure_role(&mut self) -> Option<String> {
        if self.role_sent {
            return None;
        }
        self.role_sent = true;
        Some(self.chunk(json!({ "role": "assistant", "content": "" }), None, None))
    }

    fn error_payload(message: &str) -> String {
        json!({ "error": { "message": truncate(message, 300), "type": "upstream_error" } }).to_string()
    }

    /// 翻译一个上游事件(anthropic / responses 协议)为若干待发 data 载荷。
    /// 返回 (载荷列表, usage 增量, done, 错误)。
    pub fn translate_event(&mut self, protocol: Protocol, data: &str) -> (Vec<String>, Usage, bool, Option<String>) {
        let mut usage = Usage::default();
        let mut done = false;
        let mut err = None;
        let Ok(ev) = serde_json::from_str::<Value>(data) else {
            return (vec![], usage, done, None); // 非 JSON 事件忽略
        };
        let mut out: Vec<String> = vec![];
        match protocol {
            Protocol::Anthropic => match ev.get("type").and_then(Value::as_str).unwrap_or("") {
                "message_start" => {
                    usage.prompt_tokens = ev.pointer("/message/usage/input_tokens").and_then(Value::as_i64);
                    if let Some(role) = self.ensure_role() {
                        out.push(role);
                    }
                }
                "content_block_delta" => {
                    let delta = ev.get("delta").unwrap_or(&Value::Null);
                    if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
                        if let Some(t) = delta.get("text").and_then(Value::as_str) {
                            if let Some(role) = self.ensure_role() {
                                out.push(role);
                            }
                            out.push(self.chunk(json!({ "content": t }), None, None));
                        }
                    }
                }
                "message_delta" => {
                    let finish = anthropic_stop_to_finish(ev.pointer("/delta/stop_reason").and_then(Value::as_str));
                    usage.completion_tokens = ev.pointer("/usage/output_tokens").and_then(Value::as_i64);
                    out.push(self.chunk(json!({}), Some(finish), Some(usage)));
                }
                "message_stop" => done = true,
                "error" => {
                    err = Some(
                        ev.pointer("/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("上游流式错误")
                            .to_string(),
                    );
                }
                _ => {}
            },
            Protocol::Responses => match ev.get("type").and_then(Value::as_str).unwrap_or("") {
                "response.created" | "response.in_progress" => {
                    if let Some(role) = self.ensure_role() {
                        out.push(role);
                    }
                }
                "response.output_text.delta" => {
                    if let Some(t) = ev.get("delta").and_then(Value::as_str) {
                        if let Some(role) = self.ensure_role() {
                            out.push(role);
                        }
                        out.push(self.chunk(json!({ "content": t }), None, None));
                    }
                }
                "response.completed" | "response.incomplete" => {
                    let status = ev.pointer("/response/status").and_then(Value::as_str);
                    let finish = responses_status_to_finish(status);
                    usage.prompt_tokens = ev.pointer("/response/usage/input_tokens").and_then(Value::as_i64);
                    usage.completion_tokens = ev.pointer("/response/usage/output_tokens").and_then(Value::as_i64);
                    out.push(self.chunk(json!({}), Some(finish), Some(usage)));
                    done = true;
                }
                "response.failed" | "error" => {
                    err = Some(
                        ev.pointer("/response/error/message")
                            .or_else(|| ev.pointer("/error/message"))
                            .and_then(Value::as_str)
                            .unwrap_or("上游流式失败")
                            .to_string(),
                    );
                    done = true;
                }
                _ => {}
            },
            Protocol::OpenAi => unreachable!("openai 协议走原样中继,不经翻译"),
        }
        if let Some(e) = &err {
            out.push(StreamSession::error_payload(e));
        }
        (out, usage, done, err)
    }
}

// ==================== IO(send / relay) ====================

#[derive(Debug)]
pub(crate) struct BufferedReply {
    pub model: String,
    pub body: Value,
    pub usage: Usage,
}

pub(crate) enum StreamReply {
    /// openai 协议:上游字节流原样中继(SSE)。
    RawPass(reqwest::Response),
    /// anthropic/responses:事件翻译成 openai chunk。
    Translated { resp: reqwest::Response, protocol: Protocol },
}

fn build_request(
    client: &reqwest::Client,
    cand: &ResolvedCandidate,
    protocol: Protocol,
    body: &Value,
) -> reqwest::RequestBuilder {
    let url = endpoint_url(&cand.base_url, match protocol {
        Protocol::Anthropic => "messages",
        Protocol::Responses => "responses",
        Protocol::OpenAi => "chat/completions",
    });
    let req = client.post(url).json(body);
    match protocol {
        Protocol::Anthropic => req
            .header("x-api-key", &cand.api_key)
            .header("anthropic-version", "2023-06-01"),
        _ => req.bearer_auth(&cand.api_key),
    }
}

/// 单次尝试(非流式):构造请求 → 发送 → 状态检查 → 解析。
pub(crate) async fn call_buffered(
    client: &reqwest::Client,
    cand: &ResolvedCandidate,
    incoming: &Value,
    ctx: &GroupCtx,
    timeout: Duration,
) -> Result<BufferedReply, UpstreamError> {
    let protocol = normalize_provider(&cand.provider);
    let body = build_upstream_body(protocol, incoming, &cand.model, ctx, false)?;
    let req = build_request(client, cand, protocol, &body);
    let resp = tokio::time::timeout(timeout, req.send())
        .await
        .map_err(|_| UpstreamError::Timeout)?
        .map_err(|e| UpstreamError::Connect(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let code = status.as_u16();
        let raw = tokio::time::timeout(timeout, resp.text())
            .await
            .map_err(|_| UpstreamError::Timeout)?
            .unwrap_or_default();
        let parsed: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        return Err(UpstreamError::Status { code, message: extract_error_message(&parsed, &raw) });
    }
    let raw = tokio::time::timeout(timeout, resp.text())
        .await
        .map_err(|_| UpstreamError::Timeout)?
        .map_err(|e| UpstreamError::Connect(e.to_string()))?;
    let parsed: Value =
        serde_json::from_str(&raw).map_err(|_| UpstreamError::Invalid("上游响应不是有效 JSON".into()))?;
    let (model, text, finish, usage) = parse_buffered_response(protocol, &parsed);
    let model = if model.is_empty() { cand.model.clone() } else { model };
    Ok(BufferedReply {
        usage,
        body: to_openai_completion(&new_completion_id(), &(model.clone(), text, finish, usage)),
        model,
    })
}

pub(crate) fn new_completion_id() -> String {
    format!("chatcmpl-gw-{}", super::new_hex(6))
}

/// 打开流式尝试:成功返回已就绪(头已到)的流,失败返回可切换的错误。
pub(crate) async fn open_stream(
    client: &reqwest::Client,
    cand: &ResolvedCandidate,
    incoming: &Value,
    ctx: &GroupCtx,
    timeout: Duration,
) -> Result<(StreamReply, String), UpstreamError> {
    let protocol = normalize_provider(&cand.provider);
    let body = build_upstream_body(protocol, incoming, &cand.model, ctx, true)?;
    let req = build_request(client, cand, protocol, &body);
    let resp = tokio::time::timeout(timeout, req.send())
        .await
        .map_err(|_| UpstreamError::Timeout)?
        .map_err(|e| UpstreamError::Connect(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let code = status.as_u16();
        let raw = tokio::time::timeout(timeout, resp.text())
            .await
            .map_err(|_| UpstreamError::Timeout)?
            .unwrap_or_default();
        let parsed: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        return Err(UpstreamError::Status { code, message: extract_error_message(&parsed, &raw) });
    }
    Ok((match protocol {
        Protocol::OpenAi => StreamReply::RawPass(resp),
        p => StreamReply::Translated { resp, protocol: p },
    }, cand.model.clone()))
}

/// 流式中继结果。
pub(crate) struct StreamSummary {
    pub usage: Usage,
    /// 翻译路径下累计的文本字符(raw 路径不统计,交由 usage 嗅探)。
    pub completion_chars: usize,
    /// 上游中途报错(已向客户端发过 SSE error 事件)。
    pub error: Option<String>,
}

/// 中继/翻译上游流。write 是到客户端的落笔(失败即中止,取消上游)。
/// idle 超时逐块施加,防止上游僵死把连接占住。model 用于翻译路径的
/// chunk 元信息(对外展示的应答模型)。
pub(crate) async fn relay_stream(
    reply: StreamReply,
    model: String,
    mut write: impl FnMut(&[u8]) -> io::Result<()>,
    idle: Duration,
) -> Result<StreamSummary, String> {
    use futures_util::StreamExt;
    let mut summary = StreamSummary { usage: Usage::default(), completion_chars: 0, error: None };
    let write_sse = |write: &mut dyn FnMut(&[u8]) -> io::Result<()>, payload: &str| -> Result<(), String> {
        write(format!("data: {payload}\n\n").as_bytes()).map_err(|e| format!("客户端写入失败: {e}"))
    };
    match reply {
        StreamReply::RawPass(resp) => {
            let mut stream = resp.bytes_stream();
            let mut parser = SseParser::default();
            loop {
                let chunk = match tokio::time::timeout(idle, stream.next()).await {
                    Err(_) => return Err("上游流式空闲超时".into()),
                    Ok(None) => break,
                    Ok(Some(Err(e))) => return Err(format!("上游流中断: {e}")),
                    Ok(Some(Ok(bytes))) => bytes,
                };
                // 旁路嗅探 usage(不改写字节)
                for data in parser.feed(&chunk) {
                    if data == "[DONE]" {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(&data) {
                        if v.pointer("/usage/prompt_tokens").is_some() || v.pointer("/usage/completion_tokens").is_some() {
                            summary.usage.merge(&Usage {
                                prompt_tokens: v.pointer("/usage/prompt_tokens").and_then(Value::as_i64),
                                completion_tokens: v.pointer("/usage/completion_tokens").and_then(Value::as_i64),
                            });
                        }
                        if v.get("error").is_some() && summary.error.is_none() {
                            summary.error = Some(extract_error_message(&v, &data));
                        }
                    }
                }
                write(&chunk).map_err(|e| format!("客户端写入失败: {e}"))?;
            }
        }
        StreamReply::Translated { resp, protocol } => {
            let mut stream = resp.bytes_stream();
            let mut parser = SseParser::default();
            let id = new_completion_id();
            let mut session: Option<StreamSession> = None;
            let mut done = false;
            'outer: loop {
                let chunk = match tokio::time::timeout(idle, stream.next()).await {
                    Err(_) => return Err("上游流式空闲超时".into()),
                    Ok(None) => break,
                    Ok(Some(Err(e))) => return Err(format!("上游流中断: {e}")),
                    Ok(Some(Ok(bytes))) => bytes,
                };
                for data in parser.feed(&chunk) {
                    if done {
                        break 'outer;
                    }
                    let sess =
                        session.get_or_insert_with(|| StreamSession::new(id.clone(), model.clone()));
                    let (payloads, usage, ev_done, err) = sess.translate_event(protocol, &data);
                    summary.usage.merge(&usage);
                    if let Some(e) = err {
                        summary.error = Some(e);
                    }
                    for p in &payloads {
                        write_sse(&mut write, p)?;
                        // 记录翻译产出的文本量(估算口径与 buffered 一致)
                        if p.starts_with('{') {
                            if let Ok(v) = serde_json::from_str::<Value>(p) {
                                if let Some(t) = v.pointer("/choices/0/delta/content").and_then(Value::as_str) {
                                    summary.completion_chars += t.chars().count();
                                }
                            }
                        }
                    }
                    if ev_done {
                        done = true;
                        break;
                    }
                }
            }
            if !done {
                // 上游流未给终止事件就断了:补一个收尾,客户端才不会吊着
                write_sse(&mut write, "[DONE]")?;
            }
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> GroupCtx {
        GroupCtx { max_output: 1000, temperature: Some(0.3), system_prompt: "你是测试助手".into() }
    }

    fn incoming() -> Value {
        json!({
            "model": "whatever",
            "messages": [
                { "role": "system", "content": "旧系统词" },
                { "role": "user", "content": "你好" }
            ],
            "max_tokens": 99999
        })
    }

    #[test]
    fn openai_body_clamps_and_injects() {
        let body = openai_chat_body(&incoming(), "m1", &ctx(), false).unwrap();
        assert_eq!(body["model"], "m1");
        assert_eq!(body["stream"], false);
        assert_eq!(body["max_tokens"], 1000, "超出的 max_tokens 被钳制");
        assert_eq!(body["temperature"], 0.3);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "你是测试助手\n\n旧系统词");
        // 非流式请求剥掉 stream_options
        let mut inc = incoming();
        inc["stream_options"] = json!({ "include_usage": true });
        let body = openai_chat_body(&inc, "m1", &ctx(), false).unwrap();
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn anthropic_body_converts_messages_and_system() {
        let body = anthropic_messages_body(&incoming(), "claude-x", &ctx(), true).unwrap();
        assert_eq!(body["model"], "claude-x");
        assert_eq!(body["max_tokens"], 1000, "请求的 max_tokens 被钳制");
        assert_eq!(body["stream"], true);
        assert_eq!(body["system"], "你是测试助手\n\n旧系统词");
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"][0]["type"], "text");
        assert_eq!(msgs[0]["content"][0]["text"], "你好");
        // 未带 max_tokens 的请求用组级缺省
        let mut inc = incoming();
        inc.as_object_mut().unwrap().remove("max_tokens");
        let body = anthropic_messages_body(&inc, "claude-x", &ctx(), false).unwrap();
        assert_eq!(body["max_tokens"], 1000);
    }

    #[test]
    fn anthropic_body_converts_data_url_images() {
        let inc = json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "text", "text": "看图" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,QUJD" } }
                ]}
            ]
        });
        let body = anthropic_messages_body(&inc, "claude-x", &ctx(), false).unwrap();
        let block = body["messages"][0]["content"][1].clone();
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "base64");
        assert_eq!(block["source"]["media_type"], "image/png");
        assert_eq!(block["source"]["data"], "QUJD");
        // http URL 用 anthropic url source
        let inc2 = json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "image_url", "image_url": { "url": "https://img.example.com/a.png" } }
                ]}
            ]
        });
        let body = anthropic_messages_body(&inc2, "c", &ctx(), false).unwrap();
        assert_eq!(body["messages"][0]["content"][0]["source"]["type"], "url");
    }

    #[test]
    fn responses_body_converts() {
        let body = responses_body(&incoming(), "gpt-x", &ctx(), true).unwrap();
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["max_output_tokens"], 1000);
        assert_eq!(body["instructions"], "你是测试助手\n\n旧系统词");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn endpoint_url_handles_versioned_bases() {
        assert_eq!(endpoint_url("https://a.com/v1", "chat/completions"), "https://a.com/v1/chat/completions");
        assert_eq!(endpoint_url("https://a.com", "chat/completions"), "https://a.com/v1/chat/completions");
        assert_eq!(endpoint_url("https://a.com/api/coding/paas/v4", "chat/completions"), "https://a.com/api/coding/paas/v4/chat/completions");
        assert_eq!(endpoint_url("https://a.com/", "chat/completions"), "https://a.com/v1/chat/completions");
    }

    #[test]
    fn parse_anthropic_response() {
        let body = json!({
            "id": "msg_1", "model": "claude-x", "stop_reason": "end_turn",
            "content": [ { "type": "text", "text": "Hello " }, { "type": "text", "text": "world" } ],
            "usage": { "input_tokens": 12, "output_tokens": 34 }
        });
        let (model, text, finish, usage) = parse_buffered_response(Protocol::Anthropic, &body);
        assert_eq!(model, "claude-x");
        assert_eq!(text, "Hello world");
        assert_eq!(finish, "stop");
        assert_eq!(usage.prompt_tokens, Some(12));
        assert_eq!(usage.completion_tokens, Some(34));
    }

    #[test]
    fn parse_responses_response() {
        let body = json!({
            "id": "resp_1", "status": "completed", "model": "gpt-x",
            "output": [ { "type": "message", "content": [ { "type": "output_text", "text": "答案" } ] } ],
            "usage": { "input_tokens": 5, "output_tokens": 6 }
        });
        let (_, text, finish, usage) = parse_buffered_response(Protocol::Responses, &body);
        assert_eq!(text, "答案");
        assert_eq!(finish, "stop");
        assert_eq!(usage.completion_tokens, Some(6));
    }

    #[test]
    fn sse_parser_splits_events() {
        let mut p = SseParser::default();
        let events = p.feed(b"event: a\ndata: {\"x\":1}\n\ndata: partial");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], "{\"x\":1}");
        let events = p.feed(b" continue\n\r\ndata: [DONE]\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], "partial continue");
        assert_eq!(events[1], "[DONE]");
    }

    #[test]
    fn anthropic_stream_translation_produces_openai_chunks() {
        let mut s = StreamSession::new("id1".into(), "claude-x".into());
        let (out, usage, done, err) = s.translate_event(
            Protocol::Anthropic,
            r#"{"type":"message_start","message":{"usage":{"input_tokens":7}}}"#,
        );
        assert_eq!(out.len(), 1, "role chunk");
        assert_eq!(usage.prompt_tokens, Some(7));
        assert!(!done);
        let (out, _, _, _) = s.translate_event(Protocol::Anthropic, r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}"#);
        let chunk: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(chunk["object"], "chat.completion.chunk");
        assert_eq!(chunk["choices"][0]["delta"]["content"], "hi");
        let (out, usage2, done, _) = s.translate_event(
            Protocol::Anthropic,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}"#,
        );
        let chunk: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(chunk["choices"][0]["finish_reason"], "stop");
        assert_eq!(chunk["usage"]["completion_tokens"], 9);
        assert_eq!(usage2.completion_tokens, Some(9));
        let (_, _, done2, _) = s.translate_event(Protocol::Anthropic, r#"{"type":"message_stop"}"#);
        assert!(done2 || done, "message_stop 触发 done");
        assert!(err.is_none());
    }

    #[test]
    fn responses_stream_translation() {
        let mut s = StreamSession::new("id2".into(), "gpt-x".into());
        let (_, _, _, _) = s.translate_event(Protocol::Responses, r#"{"type":"response.created"}"#);
        let (out, _, _, _) = s.translate_event(Protocol::Responses, r#"{"type":"response.output_text.delta","delta":"你好"}"#);
        let chunk: Value = serde_json::from_str(&out.last().unwrap()).unwrap();
        assert_eq!(chunk["choices"][0]["delta"]["content"], "你好");
        let (out, usage, done, _) = s.translate_event(
            Protocol::Responses,
            r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":3,"output_tokens":4}}}"#,
        );
        let chunk: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(chunk["choices"][0]["finish_reason"], "stop");
        assert_eq!(usage.completion_tokens, Some(4));
        assert!(done);
    }

    #[test]
    fn upstream_error_extraction() {
        let openai = json!({ "error": { "message": "quota exceeded" } });
        assert_eq!(extract_error_message(&openai, ""), "quota exceeded");
        let anthropic = json!({ "type": "error", "error": { "type": "overloaded", "message": "忙" } });
        assert_eq!(extract_error_message(&anthropic, ""), "忙");
        let garbage = json!(null);
        assert_eq!(extract_error_message(&garbage, "Bad Gateway"), "Bad Gateway");
    }
}
