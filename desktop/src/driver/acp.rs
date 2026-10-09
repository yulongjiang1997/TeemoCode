// Agent Client Protocol 客户端:JSON-RPC over stdio。
// 写出 LSP Content-Length 帧;读入同时认 LSP 头与 NDJSON(Codex/Goose 两端都见过)。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

pub enum Incoming {
    Notification { method: String, params: Value },
    Request { id: Value, method: String, params: Value },
}

pub struct AcpClient {
    child: StdMutex<Option<Child>>,
    stdin: StdMutex<Option<ChildStdin>>,
    pending: StdMutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>,
    next_id: AtomicI64,
    last_err: StdMutex<String>,
    ndjson: bool,
}

/// Windows 上 npm/npx 常是 .cmd,CreateProcess 不能直接起,必须经 cmd.exe。
/// 后续参数必须和脚本路径一起塞进 `/C` 的那一条命令,否则 `cmd /C script.cmd acp`
/// 会把 `acp` 丢掉(只跑脚本、不带参数)。
pub(super) fn command_with_args(bin: &Path, args: &[&str]) -> Command {
    #[cfg(windows)]
    {
        let ext = bin.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat") {
            let mut c = Command::new("cmd.exe");
            c.arg("/D").arg("/S").arg("/C");
            let line = windows_cmd_line(bin, args);
            use std::os::windows::process::CommandExt;
            c.raw_arg(format!("\"{line}\""));
            return c;
        }
    }
    let mut c = Command::new(bin);
    c.args(args);
    c
}

#[cfg(windows)]
fn windows_cmd_line(bin: &Path, args: &[&str]) -> String {
    fn quote(s: &str) -> String {
        if s.is_empty() || s.bytes().any(|b| b.is_ascii_whitespace() || b == b'"') {
            format!("\"{}\"", s.replace('"', "\\\""))
        } else {
            s.to_string()
        }
    }
    let mut out = quote(&bin.to_string_lossy());
    for a in args {
        out.push(' ');
        out.push_str(&quote(a));
    }
    out
}

impl AcpClient {
    pub fn spawn_with(
        bin: &Path,
        args: &[&str],
        cwd: &str,
        extra_env: &[(&str, String)],
        env_remove: &[&str],
        ndjson: bool,
    ) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<Incoming>), String> {
        let mut cmd = command_with_args(bin, args);
        cmd.current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for k in env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        crate::wsl::no_console(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("启动 ACP 引擎失败: {e}"))?;
        let stdin = child.stdin.take().ok_or("ACP 引擎未提供 stdin")?;
        let stdout = child.stdout.take().ok_or("ACP 引擎未提供 stdout")?;
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::unbounded_channel();
        let client = Arc::new(Self {
            child: StdMutex::new(Some(child)),
            stdin: StdMutex::new(Some(stdin)),
            pending: StdMutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            last_err: StdMutex::new(String::new()),
            ndjson,
        });
        let reader = client.clone();
        std::thread::Builder::new()
            .name("acp-stdout".into())
            .spawn(move || reader_loop(reader, stdout, tx))
            .map_err(|e| format!("ACP 读线程失败: {e}"))?;
        if let Some(err) = stderr {
            let err_client = client.clone();
            std::thread::Builder::new()
                .name("acp-stderr".into())
                .spawn(move || {
                    for line in BufReader::new(err).lines().flatten() {
                        if line.trim().is_empty() {
                            continue;
                        }
                        eprintln!("[codex-acp] {line}");
                        if let Ok(mut g) = err_client.last_err.lock() {
                            if g.len() >= 4096 {
                                continue;
                            }
                            if !g.is_empty() {
                                g.push('\n');
                            }
                            g.push_str(&line);
                        }
                    }
                })
                .ok();
        }
        Ok((client, rx))
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().map_err(|e| e.to_string())?.insert(id, tx);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write_msg(&msg)?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(format!("{method} 通道关闭")),
            Err(_) => {
                self.pending.lock().ok().map(|mut p| p.remove(&id));
                Err(format!("{method} 超时"))
            }
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write_msg(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    pub fn last_err_text(&self) -> String {
        self.last_err
            .lock()
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default()
    }

    pub fn respond(&self, id: Value, result: Value) -> Result<(), String> {
        self.write_msg(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    pub fn respond_err(&self, id: Value, message: &str) -> Result<(), String> {
        self.write_msg(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32603, "message": message }
        }))
    }

    fn write_msg(&self, msg: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(msg).map_err(|e| format!("ACP 编码失败: {e}"))?;
        let frame = if self.ndjson {
            let mut b = body;
            b.push(b'\n');
            b
        } else {
            let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
            frame.extend_from_slice(&body);
            frame
        };
        let mut guard = self.stdin.lock().map_err(|e| e.to_string())?;
        let stdin = guard.as_mut().ok_or("ACP stdin 已关闭")?;
        stdin.write_all(&frame).map_err(|e| format!("ACP 写入失败: {e}"))?;
        stdin.flush().map_err(|e| format!("ACP flush 失败: {e}"))?;
        Ok(())
    }

    pub fn kill(&self) {
        self.shutdown(None);
    }

    fn shutdown(&self, reason: Option<&str>) {
        if let Ok(mut g) = self.child.lock() {
            if let Some(child) = g.as_mut() {
                let _ = child.kill();
            }
            *g = None;
        }
        *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let detail = self
            .last_err
            .lock()
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let msg = match (reason, detail) {
            (Some(r), Some(d)) => format!("{r}: {d}"),
            (Some(r), None) => r.to_string(),
            (None, Some(d)) => format!("ACP 进程已结束: {d}"),
            (None, None) => "ACP 进程已结束".into(),
        };
        if let Ok(mut p) = self.pending.lock() {
            for (_, tx) in p.drain() {
                let _ = tx.send(Err(msg.clone()));
            }
        }
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        self.kill();
    }
}

fn reader_loop(client: Arc<AcpClient>, stdout: impl Read, tx: mpsc::UnboundedSender<Incoming>) {
    let mut reader = BufReader::new(stdout);
    loop {
        match read_message(&mut reader) {
            Ok(Some(v)) => dispatch(&client, v, &tx),
            Ok(None) => {
                client.shutdown(None);
                return;
            }
            Err(e) => {
                eprintln!("[acp] 读失败: {e}");
                client.shutdown(Some(&format!("ACP 读失败: {e}")));
                return;
            }
        }
    }
}

fn dispatch(client: &AcpClient, v: Value, tx: &mpsc::UnboundedSender<Incoming>) {
    let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = v.get("id").cloned();
    if method.is_empty() {
        if let Some(id) = id.as_ref().and_then(rpc_id) {
            let result = if let Some(err) = v.get("error") {
                Err(err.get("message").and_then(|m| m.as_str()).unwrap_or("ACP 错误").to_string())
            } else {
                Ok(v.get("result").cloned().unwrap_or(Value::Null))
            };
            if let Ok(mut p) = client.pending.lock() {
                if let Some(s) = p.remove(&id) {
                    let _ = s.send(result);
                }
            }
        }
        return;
    }
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    let incoming = if let Some(id) = id {
        Incoming::Request { id, method: method.to_string(), params }
    } else {
        Incoming::Notification { method: method.to_string(), params }
    };
    let _ = tx.send(incoming);
}

fn rpc_id(id: &Value) -> Option<i64> {
    match id {
        Value::Number(n) => n.as_i64().or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok())),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn read_message(reader: &mut BufReader<impl Read>) -> Result<Option<Value>, String> {
    let mut header = String::new();
    let mut first = String::new();
    loop {
        first.clear();
        let n = reader.read_line(&mut first).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = first.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('{') {
            return serde_json::from_str(trimmed.trim_end()).map(Some).map_err(|e| e.to_string());
        }
        if trimmed.to_ascii_lowercase().starts_with("content-length") {
            break;
        }
        eprintln!("[acp] skip: {}", trimmed.trim_end());
    }
    header.push_str(&first);
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None);
        }
        header.push_str(&line);
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let len = header
        .lines()
        .find_map(|l| {
            let l = l.trim();
            l.split_once(':').and_then(|(k, v)| {
                if k.eq_ignore_ascii_case("content-length") {
                    v.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
        })
        .ok_or("ACP 帧缺少 Content-Length")?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map(Some).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn ndjson_message_parses() {
        let raw = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n";
        let mut r = BufReader::new(Cursor::new(&raw[..]));
        let v = read_message(&mut r).unwrap().unwrap();
        assert_eq!(v["result"]["ok"], true);
    }

    #[test]
    fn lsp_frame_parses() {
        let body = br#"{"jsonrpc":"2.0","method":"session/update","params":{"x":1}}"#;
        let mut raw = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        raw.extend_from_slice(body);
        let mut r = BufReader::new(Cursor::new(raw));
        let v = read_message(&mut r).unwrap().unwrap();
        assert_eq!(v["method"], "session/update");
    }

    #[test]
    fn skips_non_protocol_banner_then_parses_ndjson() {
        let raw = b"Codex starting\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n";
        let mut r = BufReader::new(Cursor::new(&raw[..]));
        let v = read_message(&mut r).unwrap().unwrap();
        assert_eq!(v["result"]["ok"], true);
    }

    #[test]
    fn rpc_id_accepts_number_and_numeric_string() {
        assert_eq!(rpc_id(&json!(1)), Some(1));
        assert_eq!(rpc_id(&json!("2")), Some(2));
        assert_eq!(rpc_id(&json!("x")), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_cmd_line_keeps_args_with_the_script() {
        let line = windows_cmd_line(Path::new(r"C:\Users\me\npx.cmd"), &["-y", "@openai/codex", "acp"]);
        assert!(line.contains("npx.cmd"));
        assert!(line.contains("-y"));
        assert!(line.contains("acp"));
    }
}
