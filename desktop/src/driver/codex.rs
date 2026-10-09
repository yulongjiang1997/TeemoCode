// Codex ACP 会话:每会话一个 `codex acp` 进程。不替换 ohmy 默认引擎。
// 每会话一条 Codex 通道,不替换 ohmy 默认引擎。
// Codex 在这里是代理运行时:隔离 CODEX_HOME,注入 TeemoCode 已配置的
// OpenAI 兼容模型,不使用 ~/.codex 的 ChatGPT 登录态。
// 优先 `codex app-server --listen stdio://`;帧仍走 ACP sessionUpdate 形状,UI 零改。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::acp::{AcpClient, Incoming};
use super::external::{find_in_dir, find_in_path};
use super::frame;
use super::ohmy::Inner;
use crate::repo::RepoCtx;

const INIT_TIMEOUT: Duration = Duration::from_secs(20);
const PROMPT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Codex 只打本机网关。Windows 开了系统代理时,reqwest 默认会把 127.0.0.1
/// 送进 Clash 之类的本地代理,对方再回空 body 的 502。
const LOOPBACK_NO_PROXY: &str = "127.0.0.1,localhost,::1";
const PROXY_ENV_KEYS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

enum CodexEngine {
    AppServer {
        client: Arc<AcpClient>,
        thread_id: String,
        turn_id: StdMutex<Option<String>>,
        pending_perm: StdMutex<HashMap<String, Value>>,
        turn_wait: StdMutex<Option<oneshot::Sender<Result<(), String>>>>,
        streamed: AtomicBool,
    },
    Exec {
        launch: CodexLaunch,
        thread_id: StdMutex<Option<String>>,
        child_pid: StdMutex<Option<u32>>,
    },
}

pub(super) struct CodexHandle {
    engine: CodexEngine,
    workdir: String,
    home: PathBuf,
    endpoint: StdMutex<crate::config::CodexEndpoint>,
    cancel: Arc<AtomicBool>,
}

pub enum Started {
    AppServer {
        handle: Arc<CodexHandle>,
        rx: mpsc::UnboundedReceiver<Incoming>,
        thread_id: String,
    },
    Exec {
        handle: Arc<CodexHandle>,
    },
}

pub struct CodexStart {
    pub workdir: String,
    pub resume_thread: Option<String>,
    pub home: PathBuf,
    pub endpoint: crate::config::CodexEndpoint,
}

#[derive(Default)]
pub struct CodexRegistry {
    sessions: StdMutex<HashMap<String, Arc<CodexHandle>>>,
}

impl CodexRegistry {
    pub fn insert(&self, sid: &str, handle: Arc<CodexHandle>) {
        if let Ok(mut g) = self.sessions.lock() {
            g.insert(sid.to_string(), handle);
        }
    }

    pub fn drop_session(&self, sid: &str) {
        if let Ok(mut g) = self.sessions.lock() {
            if let Some(h) = g.remove(sid) {
                h.cancel.store(true, Ordering::SeqCst);
                stop_handle(&h);
            }
        }
    }

    pub fn is_live(&self, sid: &str) -> bool {
        self.sessions.lock().ok().map(|g| g.contains_key(sid)).unwrap_or(false)
    }

    fn get(&self, sid: &str) -> Result<Arc<CodexHandle>, String> {
        self.sessions
            .lock()
            .map_err(|e| e.to_string())?
            .get(sid)
            .cloned()
            .ok_or_else(|| format!("Codex 会话未启动: {sid}"))
    }

    pub fn cancel(&self, sid: &str) {
        if let Ok(h) = self.get(sid) {
            h.cancel.store(true, Ordering::SeqCst);
            match &h.engine {
                CodexEngine::AppServer { client, thread_id, turn_id, .. } => {
                    let turn = turn_id.lock().ok().and_then(|g| g.clone());
                    if let Some(turn) = turn {
                        let _ = client.notify(
                            "turn/interrupt",
                            json!({ "threadId": thread_id, "turnId": turn }),
                        );
                    }
                    client.kill();
                }
                CodexEngine::Exec { child_pid, .. } => {
                    if let Ok(g) = child_pid.lock() {
                        if let Some(pid) = *g {
                            kill_pid(pid);
                        }
                    }
                }
            }
        }
    }

    pub fn was_cancelled(&self, sid: &str) -> bool {
        self.get(sid).map(|h| h.cancel.load(Ordering::SeqCst)).unwrap_or(false)
    }

    pub fn set_endpoint(&self, sid: &str, endpoint: crate::config::CodexEndpoint) -> Result<(), String> {
        let h = self.get(sid)?;
        *h.endpoint.lock().map_err(|e| e.to_string())? = endpoint;
        Ok(())
    }

    pub fn respond_perm(&self, sid: &str, req_id: &str, approved: bool, remember: bool) -> Result<(), String> {
        let h = self.get(sid)?;
        let CodexEngine::AppServer { client, pending_perm, .. } = &h.engine else {
            return Ok(());
        };
        let rpc_id = pending_perm
            .lock()
            .map_err(|e| e.to_string())?
            .remove(req_id)
            .ok_or_else(|| format!("Codex 审批已过期: {req_id}"))?;
        let option_id = if !approved {
            "reject-once"
        } else if remember {
            "allow-always"
        } else {
            "allow-once"
        };
        client.respond(
            rpc_id,
            json!({ "outcome": { "outcome": "selected", "optionId": option_id } }),
        )
    }

    pub async fn prompt(&self, inner: Arc<Inner>, sid: &str, text: &str) -> Result<(), String> {
        let h = self.get(sid)?;
        h.cancel.store(false, Ordering::SeqCst);
        match &h.engine {
            CodexEngine::AppServer { client, thread_id, turn_id, turn_wait, streamed, .. } => {
                // turn/start 的 JSON-RPC 响应只表示回合已受理(status=inProgress),
                // 真正结束靠 turn/completed 通知。以前 select/return 在 start 上,
                // UI 会闪一下 loading 然后空闲,后续流式帧被当成轮外噪声丢掉。
                streamed.store(false, Ordering::SeqCst);
                let (tx, rx) = oneshot::channel();
                if let Ok(mut g) = turn_wait.lock() {
                    *g = Some(tx);
                }
                let abort_wait = || {
                    if let Ok(mut g) = turn_wait.lock() {
                        let _ = g.take();
                    }
                };
                let model = h
                    .endpoint
                    .lock()
                    .ok()
                    .map(|g| g.model.clone())
                    .unwrap_or_default();
                let result = client
                    .request(
                        "turn/start",
                        json!({
                            "threadId": thread_id,
                            "cwd": h.workdir,
                            "model": model,
                            "input": [{ "type": "text", "text": text }]
                        }),
                        PROMPT_TIMEOUT,
                    )
                    .await;
                if h.cancel.load(Ordering::SeqCst) {
                    abort_wait();
                    return Err("cancelled".into());
                }
                match result {
                    Err(e) => {
                        abort_wait();
                        return Err(e);
                    }
                    Ok(v) => {
                        if let Some(tid) = v
                            .get("turn")
                            .and_then(|t| t.get("id"))
                            .or_else(|| v.get("turnId"))
                            .and_then(Value::as_str)
                        {
                            if let Ok(mut g) = turn_id.lock() {
                                *g = Some(tid.to_string());
                            }
                        }
                        match classify_turn(&v) {
                            Err(e) => {
                                abort_wait();
                                return Err(e);
                            }
                            Ok(true) => {
                                abort_wait();
                                return Ok(());
                            }
                            Ok(false) => {}
                        }
                    }
                }
                tokio::select! {
                    r = rx => {
                        if h.cancel.load(Ordering::SeqCst) {
                            return Err("cancelled".into());
                        }
                        r.map_err(|_| "Codex 通道关闭".to_string())?
                    }
                    _ = tokio::time::sleep(PROMPT_TIMEOUT) => {
                        abort_wait();
                        Err("Codex 回合超时".into())
                    }
                }
            }
            CodexEngine::Exec { .. } => {
                let sid = sid.to_string();
                let text = text.to_string();
                tokio::task::spawn_blocking(move || exec_turn(inner, &sid, &h, &text))
                    .await
                    .map_err(|e| format!("Codex exec 失败: {e}"))?
            }
        }
    }
}

fn stop_handle(h: &CodexHandle) {
    match &h.engine {
        CodexEngine::AppServer { client, .. } => client.kill(),
        CodexEngine::Exec { child_pid, .. } => {
            if let Ok(g) = child_pid.lock() {
                if let Some(pid) = *g {
                    kill_pid(pid);
                }
            }
        }
    }
}

fn kill_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).spawn();
    }
}

const NPX_PACKAGE: &str = "@openai/codex";

#[derive(Clone)]
struct CodexLaunch {
    bin: PathBuf,
    prefix: Vec<String>,
}

fn cache_dir() -> PathBuf {
    #[cfg(windows)]
    if let Ok(lad) = std::env::var("LOCALAPPDATA") {
        if !lad.trim().is_empty() {
            return PathBuf::from(lad).join("TeemoCode").join("codex-cli");
        }
    }
    crate::config::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".teemocode")
        .join("codex-cli")
}

fn cached_codex_bin() -> Option<PathBuf> {
    let dest = cache_dir();
    find_in_dir(&dest.join("node_modules").join(".bin"), "codex").or_else(|| find_in_dir(&dest, "codex"))
}

fn exe_sidecar_codex() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    find_in_dir(exe.parent()?, "codex")
}

/// 已经在盘上的 Codex:环境变量 → 应用同目录(可随包装发) → PATH/桌面版落点 → 本地缓存。
fn existing_codex_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MC_CODEX_BIN") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Some(p) = exe_sidecar_codex() {
        return Some(p);
    }
    if let Some(p) = find_in_path("codex") {
        return Some(PathBuf::from(p));
    }
    cached_codex_bin()
}

fn install_with_npm(npm: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("创建 Codex 缓存目录失败: {e}"))?;
    let dest_s = dest.to_string_lossy();
    let args = [
        "install",
        "--omit=dev",
        "--no-fund",
        "--prefix",
        dest_s.as_ref(),
        NPX_PACKAGE,
    ];
    let mut cmd = super::acp::command_with_args(npm, &args);
    cmd.current_dir(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::wsl::no_console(&mut cmd);
    let out = cmd.output().map_err(|e| format!("安装 Codex CLI 失败: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        return Err(if err.is_empty() {
            "安装 Codex CLI 失败".into()
        } else {
            format!("安装 Codex CLI 失败: {err}")
        });
    }
    Ok(())
}

fn npx_launch(npx: PathBuf) -> CodexLaunch {
    CodexLaunch {
        bin: npx,
        prefix: vec!["-y".into(), NPX_PACKAGE.into()],
    }
}

/// 本机没有 Codex 时自动装进应用缓存(不污染全局 npm),再不行才退到 npx。
fn ensure_codex_launch() -> Result<CodexLaunch, String> {
    if let Some(bin) = existing_codex_bin() {
        return Ok(CodexLaunch { bin, prefix: vec![] });
    }
    static INSTALL_LOCK: StdMutex<()> = StdMutex::new(());
    let _guard = INSTALL_LOCK.lock().map_err(|e| e.to_string())?;
    if let Some(bin) = cached_codex_bin() {
        return Ok(CodexLaunch { bin, prefix: vec![] });
    }
    if let Some(npm) = find_in_path("npm") {
        let dest = cache_dir();
        install_with_npm(Path::new(&npm), &dest)?;
        if let Some(bin) = cached_codex_bin() {
            return Ok(CodexLaunch { bin, prefix: vec![] });
        }
        return Err("已尝试安装 Codex CLI,但未找到可执行文件".into());
    }
    if let Some(npx) = find_in_path("npx") {
        return Ok(npx_launch(PathBuf::from(npx)));
    }
    Err(
        "无法自动获取 Codex CLI。请安装 Node.js 后重试,或把 Codex 放到应用目录,或设置 MC_CODEX_BIN"
            .into(),
    )
}

fn supports_app_server(launch: &CodexLaunch) -> bool {
    let mut args: Vec<&str> = launch.prefix.iter().map(String::as_str).collect();
    args.push("help");
    args.push("app-server");
    let mut cmd = super::acp::command_with_args(&launch.bin, &args);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::wsl::no_console(&mut cmd);
    match cmd.output() {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

fn toml_basic(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"))
}

pub(super) fn write_isolated_codex_home(home: &Path, endpoint: &crate::config::CodexEndpoint) -> Result<(), String> {
    std::fs::create_dir_all(home).map_err(|e| format!("创建 Codex 数据目录失败: {e}"))?;
    let body = format!(
        "# teemocode_alias = {}\nmodel = {}\nmodel_provider = \"teemocode\"\napproval_policy = \"never\"\nsandbox_mode = \"workspace-write\"\n\n[model_providers.teemocode]\nname = \"TeemoCode\"\nbase_url = {}\nenv_key = \"CODEX_API_KEY\"\nwire_api = \"responses\"\nrequires_openai_auth = false\nsupports_websockets = false\n",
        toml_basic(&endpoint.alias),
        toml_basic(&endpoint.model),
        toml_basic(&endpoint.base_url),
    );
    crate::config::atomic_write_private(&home.join("config.toml"), body.as_bytes())
}

fn launch_env(home: &Path, endpoint: &crate::config::CodexEndpoint) -> Vec<(String, String)> {
    vec![
        ("CODEX_HOME".into(), home.to_string_lossy().into_owned()),
        ("CODEX_API_KEY".into(), endpoint.api_key.clone()),
        ("NO_PROXY".into(), LOOPBACK_NO_PROXY.into()),
        ("no_proxy".into(), LOOPBACK_NO_PROXY.into()),
    ]
}

fn apply_codex_process_env(cmd: &mut Command, home: &Path, endpoint: &crate::config::CodexEndpoint) {
    for key in PROXY_ENV_KEYS {
        cmd.env_remove(key);
    }
    for (k, v) in launch_env(home, endpoint) {
        cmd.env(k, v);
    }
}

pub async fn start_session(opts: CodexStart) -> Result<Started, String> {
    write_isolated_codex_home(&opts.home, &opts.endpoint)?;
    let launch = tokio::task::spawn_blocking(ensure_codex_launch)
        .await
        .map_err(|e| format!("准备 Codex 失败: {e}"))??;
    let launch_probe = launch.clone();
    let has_app = tokio::task::spawn_blocking(move || supports_app_server(&launch_probe))
        .await
        .unwrap_or(false);
    if has_app {
        match connect_app_server(&launch, &opts).await {
            Ok(started) => return Ok(started),
            Err(e) => {
                eprintln!("[codex] app-server 握手失败,退到 exec: {e}");
            }
        }
    }
    Ok(Started::Exec {
        handle: Arc::new(CodexHandle {
            engine: CodexEngine::Exec {
                launch,
                thread_id: StdMutex::new(opts.resume_thread),
                child_pid: StdMutex::new(None),
            },
            workdir: opts.workdir,
            home: opts.home,
            endpoint: StdMutex::new(opts.endpoint),
            cancel: Arc::new(AtomicBool::new(false)),
        }),
    })
}

async fn connect_app_server(launch: &CodexLaunch, opts: &CodexStart) -> Result<Started, String> {
    let mut args: Vec<&str> = launch.prefix.iter().map(String::as_str).collect();
    args.push("app-server");
    args.push("--listen");
    args.push("stdio://");
    let env = launch_env(&opts.home, &opts.endpoint);
    let env_refs: Vec<(&str, String)> = env.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let (client, rx) = AcpClient::spawn_with(
        &launch.bin,
        &args,
        &opts.workdir,
        &env_refs,
        PROXY_ENV_KEYS,
        true,
    )?;
    client
        .request(
            "initialize",
            json!({
                "clientInfo": { "name": "TeemoCode", "title": "TeemoCode", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "experimentalApi": true }
            }),
            INIT_TIMEOUT,
        )
        .await?;
    let created = if let Some(tid) = opts.resume_thread.as_deref().filter(|s| !s.is_empty()) {
        client
            .request("thread/resume", json!({ "threadId": tid }), INIT_TIMEOUT)
            .await?
    } else {
        client
            .request(
                "thread/start",
                json!({
                    "cwd": opts.workdir,
                    "model": opts.endpoint.model,
                    "modelProvider": "teemocode",
                    "sandbox": "workspace-write",
                    "approvalPolicy": "never"
                }),
                INIT_TIMEOUT,
            )
            .await?
    };
    let thread_id = created
        .get("thread")
        .and_then(|t| t.get("id"))
        .or_else(|| created.get("threadId"))
        .or_else(|| created.get("id"))
        .and_then(|v| v.as_str())
        .ok_or("Codex thread/start 未返回 thread id")?
        .to_string();
    Ok(Started::AppServer {
        handle: Arc::new(CodexHandle {
            engine: CodexEngine::AppServer {
                client,
                thread_id: thread_id.clone(),
                turn_id: StdMutex::new(None),
                pending_perm: StdMutex::new(HashMap::new()),
                turn_wait: StdMutex::new(None),
                streamed: AtomicBool::new(false),
            },
            workdir: opts.workdir.clone(),
            home: opts.home.clone(),
            endpoint: StdMutex::new(opts.endpoint.clone()),
            cancel: Arc::new(AtomicBool::new(false)),
        }),
        rx,
        thread_id,
    })
}

fn signal_turn_wait(handle: &CodexHandle, result: Result<(), String>) {
    if let CodexEngine::AppServer { turn_wait, .. } = &handle.engine {
        if let Ok(mut g) = turn_wait.lock() {
            if let Some(tx) = g.take() {
                let _ = tx.send(result);
            }
        }
    }
}

fn json_turn_error(v: &Value) -> String {
    let err = v.get("turn").and_then(|t| t.get("error")).or_else(|| v.get("error"));
    let from_err = match err {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(obj) => obj
            .get("message")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        None => None,
    };
    from_err
        .or_else(|| {
            v.get("message")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Codex 回合失败".into())
}

fn will_retry_note(v: &Value) -> bool {
    v.get("willRetry")
        .or_else(|| v.get("will_retry"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Codex `error` 通知: `params.error.message`(不是顶层 message)。
/// `willRetry=true` 是 StreamError 瞬时失败,回合还在自动重试,不要打断。
fn classify_error_note(v: &Value) -> Option<String> {
    if will_retry_note(v) {
        None
    } else {
        Some(json_turn_error(v))
    }
}

fn turn_status(v: &Value) -> String {
    v.get("turn")
        .and_then(|t| t.get("status"))
        .or_else(|| v.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Ok(true)=已结束; Ok(false)=仍在进行; Err=失败原因。
fn classify_turn(v: &Value) -> Result<bool, String> {
    let status = turn_status(v);
    if status == "failed" {
        return Err(json_turn_error(v));
    }
    Ok(status == "completed" || status == "interrupted")
}

pub async fn pump_incoming(inner: Arc<Inner>, sid: String, handle: Arc<CodexHandle>, mut rx: mpsc::UnboundedReceiver<Incoming>) {
    while let Some(msg) = rx.recv().await {
        match msg {
            Incoming::Notification { method, params } => {
                map_app_server_note(&inner, &sid, &handle, &method, &params);
            }
            Incoming::Request { id, method, params } => {
                let CodexEngine::AppServer { client, pending_perm, .. } = &handle.engine else {
                    continue;
                };
                if method.ends_with("requestApproval") {
                    let _ = client.respond(id, json!({ "decision": "accept" }));
                    continue;
                }
                if method == "session/request_permission" || method.ends_with("request_permission") {
                    let req_id = match &id {
                        Value::Number(n) => n.to_string(),
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if let Ok(mut p) = pending_perm.lock() {
                        p.insert(req_id.clone(), id);
                    }
                    let tool = params
                        .get("toolCall")
                        .and_then(|t| t.get("kind").or_else(|| t.get("title")).or_else(|| t.get("toolCallId")))
                        .and_then(|v| v.as_str())
                        .unwrap_or("codex");
                    let title = params
                        .get("toolCall")
                        .and_then(|t| t.get("title").or_else(|| t.get("kind")))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Codex 请求权限");
                    let tc_id = params
                        .get("toolCall")
                        .and_then(|t| t.get("toolCallId").or_else(|| t.get("id")))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    inner.push_frame(&sid, |seq| frame::permission_req(&req_id, tool, title, tc_id, seq));
                    inner.emit_session_ask(&sid, true);
                } else if method == "fs/read_text_file" {
                    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    match read_sandboxed(&handle.workdir, path, inner.wsl.as_ref().map(|w| w.distro.as_str())) {
                        Ok(content) => {
                            let _ = client.respond(id, json!({ "content": content }));
                        }
                        Err(e) => {
                            let _ = client.respond_err(id, &e);
                        }
                    }
                } else if method == "fs/write_text_file" {
                    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    match write_sandboxed(&handle.workdir, path, content, inner.wsl.as_ref().map(|w| w.distro.as_str())) {
                        Ok(()) => {
                            let _ = client.respond(id, json!({}));
                        }
                        Err(e) => {
                            let _ = client.respond_err(id, &e);
                        }
                    }
                } else {
                    let _ = client.respond_err(id, "unsupported method");
                }
            }
        }
    }
    let detail = if let CodexEngine::AppServer { client, .. } = &handle.engine {
        client.last_err_text()
    } else {
        String::new()
    };
    let msg = if detail.is_empty() {
        "Codex app-server 已断开".to_string()
    } else {
        format!("Codex app-server 已断开: {detail}")
    };
    signal_turn_wait(&handle, Err(msg));
}

fn map_app_server_note(inner: &Inner, sid: &str, handle: &CodexHandle, method: &str, params: &Value) {
    match method {
        "session/update" => {
            let update = params.get("update").cloned().unwrap_or_else(|| params.clone());
            inner.push_frame(sid, |seq| frame::session_update(update, seq));
        }
        "item/agentMessage/delta" => {
            if let Some(d) = params.get("delta").and_then(Value::as_str) {
                if let CodexEngine::AppServer { streamed, .. } = &handle.engine {
                    streamed.store(true, Ordering::SeqCst);
                }
                inner.push_frame(sid, |seq| frame::agent_text(d, seq));
            }
        }
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
            if let Some(d) = params.get("delta").and_then(Value::as_str) {
                inner.push_frame(sid, |seq| frame::agent_thought(d, seq));
            }
        }
        "item/started" => {
            if let Some(item) = params.get("item") {
                let ty = item.get("type").and_then(Value::as_str).unwrap_or("");
                let id = item.get("id").and_then(Value::as_str).unwrap_or("");
                if matches!(ty, "commandExecution" | "fileChange" | "mcpToolCall") {
                    let title = item
                        .get("command")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("server").and_then(Value::as_str))
                        .unwrap_or(ty);
                    inner.push_frame(sid, |seq| frame::tool_call(id, title, item, seq));
                }
            }
        }
        "item/completed" => {
            if let Some(item) = params.get("item") {
                let ty = item.get("type").and_then(Value::as_str).unwrap_or("");
                let id = item.get("id").and_then(Value::as_str).unwrap_or("");
                if matches!(ty, "commandExecution" | "fileChange" | "mcpToolCall") {
                    let out = item
                        .get("aggregatedOutput")
                        .or_else(|| item.get("output"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    inner.push_frame(sid, |seq| frame::tool_call_completed(id, out, &[], seq));
                } else if matches!(ty, "agentMessage" | "agent_message") {
                    let already = match &handle.engine {
                        CodexEngine::AppServer { streamed, .. } => streamed.load(Ordering::SeqCst),
                        _ => true,
                    };
                    if !already {
                        if let Some(text) = item.get("text").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                            if let CodexEngine::AppServer { streamed, .. } = &handle.engine {
                                streamed.store(true, Ordering::SeqCst);
                            }
                            inner.push_frame(sid, |seq| frame::agent_text(text, seq));
                        }
                    }
                }
            }
        }
        "turn/started" => {
            if let CodexEngine::AppServer { turn_id, .. } = &handle.engine {
                let tid = params
                    .get("turn")
                    .and_then(|t| t.get("id"))
                    .or_else(|| params.get("turnId"))
                    .and_then(Value::as_str);
                if let Some(tid) = tid {
                    if let Ok(mut g) = turn_id.lock() {
                        *g = Some(tid.to_string());
                    }
                }
            }
        }
        "turn/completed" => match classify_turn(params) {
            Ok(true) => signal_turn_wait(handle, Ok(())),
            Ok(false) => {}
            Err(e) => signal_turn_wait(handle, Err(e)),
        },
        "error" => {
            if let Some(msg) = classify_error_note(params) {
                signal_turn_wait(handle, Err(msg));
            }
        }
        _ => {}
    }
}

/// 审批/沙箱写在隔离 CODEX_HOME 的 config.toml，不在 CLI 上传 `-s` 或
/// `--approve-for-me`（二者互斥）。禁止 `--dangerously-bypass-approvals-and-sandbox`。
fn exec_cli_args(prefix: &[String], workdir: &str, resume: Option<&str>) -> Vec<String> {
    let mut args = prefix.to_vec();
    args.push("exec".into());
    if let Some(id) = resume {
        args.extend([
            "resume".into(),
            "--skip-git-repo-check".into(),
            "--json".into(),
            id.to_string(),
            "-".into(),
        ]);
    } else {
        args.extend([
            "--skip-git-repo-check".into(),
            "--json".into(),
            "--color".into(),
            "never".into(),
            "-C".into(),
            workdir.to_string(),
            "-".into(),
        ]);
    }
    args
}

fn exec_turn(inner: Arc<Inner>, sid: &str, h: &CodexHandle, text: &str) -> Result<(), String> {
    let CodexEngine::Exec { launch, thread_id, child_pid } = &h.engine else {
        return Err("Codex 引擎类型异常".into());
    };
    let endpoint = h.endpoint.lock().map_err(|e| e.to_string())?.clone();
    let tid = thread_id.lock().ok().and_then(|g| g.clone());
    let mut args = exec_cli_args(&launch.prefix, &h.workdir, tid.as_deref());
    if let Some(dash) = args.iter().rposition(|a| a == "-") {
        args.splice(
            dash..dash,
            [
                format!("-c"),
                format!("model={}", endpoint.model),
                format!("-c"),
                format!("model_provider=teemocode"),
            ],
        );
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut cmd = super::acp::command_with_args(&launch.bin, &arg_refs);
    cmd.current_dir(&h.workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_codex_process_env(&mut cmd, &h.home, &endpoint);
    crate::wsl::no_console(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("启动 Codex 失败: {e}"))?;
    let pid = child.id();
    if let Ok(mut g) = child_pid.lock() {
        *g = Some(pid);
    }
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
        if !text.ends_with('\n') {
            let _ = stdin.write_all(b"\n");
        }
        drop(stdin);
    }
    let stdout = child.stdout.take().ok_or("Codex 未提供 stdout")?;
    let stderr = child.stderr.take();
    let err_join = std::thread::spawn(move || {
        let mut t = String::new();
        if let Some(err) = stderr {
            for l in BufReader::new(err).lines().flatten() {
                if t.len() >= 4096 {
                    break;
                }
                if !t.is_empty() {
                    t.push('\n');
                }
                t.push_str(&l);
            }
        }
        t
    });
    let mut state = ExecParseState::default();
    for line in BufReader::new(stdout).lines().flatten() {
        if h.cancel.load(Ordering::SeqCst) {
            kill_pid(pid);
            let _ = child.kill();
            let _ = child.wait();
            if let Ok(mut g) = child_pid.lock() {
                *g = None;
            }
            return Err("cancelled".into());
        }
        apply_exec_line(&mut state, &line, |delta, thought| {
            if thought {
                inner.push_frame(sid, |seq| frame::agent_thought(delta, seq));
            } else {
                inner.push_frame(sid, |seq| frame::agent_text(delta, seq));
            }
        });
    }
    let status = child.wait().map_err(|e| format!("等待 Codex 退出失败: {e}"))?;
    if let Ok(mut g) = child_pid.lock() {
        *g = None;
    }
    let err_tail = err_join.join().unwrap_or_default();
    if let Some(tid) = state.thread_id.clone() {
        if let Ok(mut g) = thread_id.lock() {
            *g = Some(tid.clone());
        }
        inner.write_sidecar(sid, |m| {
            m["codex_thread_id"] = json!(tid);
            m["codex_transport"] = json!("exec");
        });
    }
    if h.cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    if !state.saw_json {
        let plain = state.pending_plain.trim();
        if !plain.is_empty() {
            inner.push_frame(sid, |seq| frame::agent_text(plain, seq));
            state.streamed = true;
        }
    }
    if !status.success() {
        return Err(if err_tail.trim().is_empty() {
            format!("Codex 退出码 {status}")
        } else {
            format!("Codex 退出: {}", err_tail.chars().take(500).collect::<String>())
        });
    }
    if let Some(msg) = state.error_msg.as_deref().filter(|s| !s.is_empty()) {
        return Err(msg.to_string());
    }
    if state.had_error {
        return Err("Codex 回合失败".into());
    }
    if !state.streamed {
        return Err(if err_tail.trim().is_empty() {
            "Codex 没有返回内容".into()
        } else {
            format!("Codex 退出: {}", err_tail.chars().take(500).collect::<String>())
        });
    }
    Ok(())
}

#[derive(Default)]
struct ExecParseState {
    saw_json: bool,
    streamed: bool,
    had_error: bool,
    thread_id: Option<String>,
    pending_plain: String,
    error_msg: Option<String>,
}

fn json_str(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn item_text(item: &Value) -> Option<String> {
    if let Some(t) = json_str(item, &["text", "message"]) {
        return Some(t);
    }
    item.get("content")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|c| json_str(c, &["text"]))
}

fn apply_exec_value(state: &mut ExecParseState, v: &Value, emit: &mut impl FnMut(&str, bool)) {
    if let Some(id) = json_str(v, &["thread_id", "threadId"]) {
        state.thread_id = Some(id);
    }
    if let Some(th) = v.get("thread") {
        if let Some(id) = json_str(th, &["id", "thread_id", "threadId"]) {
            state.thread_id = Some(id);
        }
    }
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    if ty == "error" || ty == "turn.failed" || ty.ends_with(".error") {
        state.had_error = true;
        if let Some(msg) = json_str(v, &["message"]).or_else(|| {
            v.get("error")
                .and_then(|e| json_str(e, &["message"]))
                .or_else(|| json_str(v, &["error"]))
        }) {
            state.error_msg = Some(msg);
        }
        return;
    }
    let thought = ty.contains("thought") || ty.contains("reasoning");
    if ty.contains("delta") && (ty.contains("agent_message") || ty.contains("message") || thought) {
        if let Some(d) = json_str(v, &["delta", "text"]) {
            state.streamed = true;
            emit(&d, thought);
        }
        return;
    }
    if ty == "item.completed" || ty == "item_completed" {
        if state.streamed {
            return;
        }
        if let Some(item) = v.get("item") {
            let ity = item.get("type").and_then(Value::as_str).unwrap_or("");
            if ity == "agent_message" || ity == "message" {
                if let Some(t) = item_text(item) {
                    state.streamed = true;
                    emit(&t, false);
                }
            }
        }
        return;
    }
    if (ty == "agent_message" || ty == "agent_message_event") && !state.streamed {
        if let Some(t) = json_str(v, &["message", "text"]) {
            state.streamed = true;
            emit(&t, false);
        }
    }
    if let Some(inner) = v.get("msg").or_else(|| v.get("event")).filter(|x| x.is_object()) {
        apply_exec_value(state, inner, emit);
    }
}

fn apply_exec_line(state: &mut ExecParseState, line: &str, mut emit: impl FnMut(&str, bool)) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        if !state.saw_json {
            if !state.pending_plain.is_empty() {
                state.pending_plain.push('\n');
            }
            state.pending_plain.push_str(line);
        }
        return;
    };
    state.saw_json = true;
    apply_exec_value(state, &v, &mut emit);
}

fn read_sandboxed(workdir: &str, path: &str, wsl: Option<&str>) -> Result<String, String> {
    let ctx = RepoCtx { workdir: workdir.to_string(), wsl_distro: wsl.map(|s| s.to_string()) };
    let rel = rel_of(workdir, path)?;
    let abs = ctx.resolve(&rel)?;
    std::fs::read_to_string(&abs).map_err(|e| format!("读文件失败: {e}"))
}

fn write_sandboxed(workdir: &str, path: &str, content: &str, wsl: Option<&str>) -> Result<(), String> {
    let ctx = RepoCtx { workdir: workdir.to_string(), wsl_distro: wsl.map(|s| s.to_string()) };
    let rel = rel_of(workdir, path)?;
    let abs = ctx.resolve(&rel)?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    std::fs::write(&abs, content).map_err(|e| format!("写文件失败: {e}"))
}

fn rel_of(workdir: &str, path: &str) -> Result<String, String> {
    if path.is_empty() {
        return Err("空路径".into());
    }
    let p = std::path::Path::new(path);
    if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("路径越出工作区".into());
    }
    if p.is_absolute() {
        let root = std::path::Path::new(workdir)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(workdir));
        let real = p.canonicalize().map_err(|e| format!("路径无效: {e}"))?;
        real.strip_prefix(&root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .map_err(|_| "路径越出工作区".into())
    } else if p.components().any(|c| matches!(c, std::path::Component::Prefix(_))) {
        Err("路径越出工作区".into())
    } else {
        Ok(path.replace('\\', "/"))
    }
}

pub fn new_session_id() -> String {
    let mut random = [0u8; 8];
    let _ = getrandom::getrandom(&mut random);
    format!("c{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::{npx_launch, rel_of, NPX_PACKAGE};
    use std::path::PathBuf;

    #[test]
    fn rel_of_rejects_parent_dir() {
        assert!(rel_of("/tmp/ws", "../secret").is_err());
        assert!(rel_of("/tmp/ws", "foo/../../etc/passwd").is_err());
    }

    #[test]
    fn rel_of_keeps_relative() {
        assert_eq!(rel_of("/tmp/ws", "src/main.rs").unwrap(), "src/main.rs");
    }

    #[test]
    fn npx_fallback_invokes_official_package() {
        let launch = npx_launch(PathBuf::from("npx"));
        assert_eq!(launch.prefix, ["-y", NPX_PACKAGE]);
    }

    #[test]
    fn exec_jsonl_streams_delta_and_captures_thread() {
        let mut state = super::ExecParseState::default();
        let mut out = Vec::new();
        super::apply_exec_line(
            &mut state,
            r#"{"type":"thread.started","thread_id":"t-1"}"#,
            |d, thought| out.push((d.to_string(), thought)),
        );
        super::apply_exec_line(
            &mut state,
            r#"{"type":"item.agent_message.delta","delta":"Hello"}"#,
            |d, thought| out.push((d.to_string(), thought)),
        );
        super::apply_exec_line(
            &mut state,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Hello world"}}"#,
            |d, thought| out.push((d.to_string(), thought)),
        );
        assert_eq!(state.thread_id.as_deref(), Some("t-1"));
        assert_eq!(out, vec![("Hello".to_string(), false)]);
    }

    #[test]
    fn exec_new_session_flags_match_current_cli() {
        let args = super::exec_cli_args(&["-y".into(), "@openai/codex".into()], "/tmp/ws", None);
        assert_eq!(
            args,
            [
                "-y",
                "@openai/codex",
                "exec",
                "--skip-git-repo-check",
                "--json",
                "--color",
                "never",
                "-C",
                "/tmp/ws",
                "-",
            ]
        );
        assert!(!args.iter().any(|a| a == "-a" || a == "--ask" || a == "-s" || a == "--sandbox" || a == "--approve-for-me"));
        assert!(!args.iter().any(|a| a.contains("dangerously-bypass")));
    }

    #[test]
    fn exec_resume_flags_omit_new_session_only_options() {
        let args = super::exec_cli_args(&[], "/tmp/ws", Some("t-1"));
        assert_eq!(
            args,
            ["exec", "resume", "--skip-git-repo-check", "--json", "t-1", "-"]
        );
    }

    #[test]
    fn exec_jsonl_falls_back_to_completed_when_no_delta() {
        let mut state = super::ExecParseState::default();
        let mut out = Vec::new();
        super::apply_exec_line(
            &mut state,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}"#,
            |d, thought| out.push((d.to_string(), thought)),
        );
        assert_eq!(out, vec![("done".to_string(), false)]);
    }

    #[test]
    fn exec_jsonl_error_marks_failure_without_agent_text() {
        let mut state = super::ExecParseState::default();
        let mut out = Vec::new();
        super::apply_exec_line(
            &mut state,
            r#"{"type":"error","message":"boom"}"#,
            |d, thought| out.push((d.to_string(), thought)),
        );
        assert!(out.is_empty());
        assert!(state.had_error);
        assert_eq!(state.error_msg.as_deref(), Some("boom"));
        assert!(!state.streamed);
    }

    #[test]
    fn classify_turn_waits_for_in_progress_and_surfaces_failed() {
        assert_eq!(
            super::classify_turn(&serde_json::json!({"turn":{"id":"t1","status":"inProgress"}})).unwrap(),
            false
        );
        assert_eq!(
            super::classify_turn(&serde_json::json!({"turn":{"id":"t1","status":"completed"}})).unwrap(),
            true
        );
        assert_eq!(
            super::classify_turn(&serde_json::json!({
                "turn":{"id":"t1","status":"failed","error":{"message":"nope"}}
            }))
            .unwrap_err(),
            "nope"
        );
    }

    #[test]
    fn app_server_error_note_uses_nested_message_and_ignores_retry() {
        assert_eq!(
            super::classify_error_note(&serde_json::json!({
                "error": { "message": "unexpected status 502 Bad Gateway: Unknown error" },
                "willRetry": false,
                "threadId": "t",
                "turnId": "u"
            }))
            .as_deref(),
            Some("unexpected status 502 Bad Gateway: Unknown error")
        );
        assert_eq!(
            super::classify_error_note(&serde_json::json!({
                "error": { "message": "transient" },
                "willRetry": true
            })),
            None
        );
        assert_eq!(
            super::classify_error_note(&serde_json::json!({
                "error": { "message": "still retrying" },
                "will_retry": true
            })),
            None
        );
        assert_eq!(
            super::classify_error_note(&serde_json::json!({ "message": "legacy flat" }))
                .as_deref(),
            Some("legacy flat")
        );
    }

    #[test]
    fn isolated_home_config_does_not_require_openai_login() {
        let dir = std::env::temp_dir().join(format!("teemo-codex-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ep = crate::config::CodexEndpoint {
            alias: "qwen".into(),
            model: "qwen-plus".into(),
            base_url: "https://example.com/v1".into(),
            api_key: "sk-test".into(),
        };
        super::write_isolated_codex_home(&dir, &ep).unwrap();
        let toml = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(toml.contains("model_provider = \"teemocode\""));
        assert!(toml.contains("requires_openai_auth = false"));
        assert!(toml.contains("wire_api = \"responses\""));
        assert!(!toml.contains("wire_api = \"chat\""));
        assert!(!toml.contains("chatgpt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn launch_env_bypasses_loopback_proxy() {
        let dir = std::env::temp_dir().join(format!("teemo-codex-env-{}", std::process::id()));
        let ep = crate::config::CodexEndpoint {
            alias: "免费组".into(),
            model: "免费组".into(),
            base_url: "http://localhost:8317/v1".into(),
            api_key: "tgk-test".into(),
        };
        let env = super::launch_env(&dir, &ep);
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();
        assert_eq!(map.get("CODEX_API_KEY").map(String::as_str), Some("tgk-test"));
        let no_proxy = map.get("NO_PROXY").expect("NO_PROXY");
        assert!(no_proxy.contains("127.0.0.1"), "{no_proxy}");
        assert!(no_proxy.contains("localhost"), "{no_proxy}");
        assert_eq!(map.get("no_proxy"), map.get("NO_PROXY"));
    }

    #[test]
    fn isolated_home_is_accepted_by_local_codex_cli() {
        use std::io::{Read, Write};
        use std::process::Stdio;
        let Some(bin) = super::existing_codex_bin() else {
            eprintln!("skip: 本机没有 Codex CLI,无法做配置加载探针");
            return;
        };
        let home = std::env::temp_dir().join(format!("teemo-codex-cli-home-{}", std::process::id()));
        let work = std::env::temp_dir().join(format!("teemo-codex-cli-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let ep = crate::config::CodexEndpoint {
            alias: "qwen".into(),
            model: "qwen-plus".into(),
            base_url: "http://127.0.0.1:9/v1".into(),
            api_key: "sk-test".into(),
        };
        super::write_isolated_codex_home(&home, &ep).unwrap();
        let args = super::exec_cli_args(&[], work.to_str().unwrap(), None);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut cmd = super::super::acp::command_with_args(&bin, &arg_refs);
        cmd.current_dir(&work)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        super::apply_codex_process_env(&mut cmd, &home, &ep);
        crate::wsl::no_console(&mut cmd);
        let mut child = cmd.spawn().expect("spawn local codex");
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b"ping\n");
            let _ = stdin.flush();
            drop(stdin);
        }
        let mut err_pipe = child.stderr.take();
        let started = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() > std::time::Duration::from_secs(12) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(e) => panic!("wait local codex: {e}"),
            }
        }
        let mut stderr = String::new();
        if let Some(mut err) = err_pipe.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&work);
        assert!(
            !stderr.contains("Error loading config.toml") && !stderr.contains("wire_api = \"chat\""),
            "Codex 拒绝隔离 config.toml: {stderr}"
        );
    }
}
