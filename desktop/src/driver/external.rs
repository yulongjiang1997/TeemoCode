// 外部 CLI 子代理(2026-09-09 用户需求:把本机外部 CLI agent 当子代理):
// spawn 本机安装的 CLI(Claude Code / Codex CLI)跑一条任务,stdout 流式
// 归约成壳侧子会话帧,渲染零新增——复用引擎子代理的卡片/子会话浮层体系。
//
// 设计边界:
// - 不走引擎(ohmyagent 二进制不可改),派发入口 = composer「外部代理」菜单
// - 子会话 = 壳侧会话(created=true,无引擎实体,与 claim_subagent 同口径),
//   journal/sidecar 完整可回放;父会话挂合成 tool_call + child_session progress
// - CLI 探测/spawn/流式读/收尾全在本模块;Tauri 命令注册在 driver/mod.rs
//
// 生命周期:
// - run_external_agent 同步做完物化+spawn,把「读 stdout + 等进程 + 收尾」
//   交给一条 detached 线程,命令立即返回 run_id(前端据此可取消/查在跑列表)
// - external_agent_cancel 置 cancel 标志;worker 线程下一轮 100ms 轮询检测后
//   kill 进程树(taskkill /T /F 或负 pid 进程组),按 interrupted 收尾
// - 超时:worker 到 deadline 主动 kill,按 failed 收尾

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use serde_json::{json, Value};

use super::frame;
use super::ohmy::Inner;
use super::session::{SessionState, valid_session_id};
use crate::util::LockExt;

/// 单个运行中的外部子代理(运行注册表条目)。
pub(crate) struct ExtRun {
    pub(crate) run_id: String,
    pub(crate) child_sid: String,
    pub(crate) parent_sid: String,
    pub(crate) tc_id: String,
    /// 取消标志:cancel 命令置位,worker 线程轮询检测
    pub(crate) cancel: Arc<AtomicBool>,
}

/// 外部子代理运行注册表(Tauri State 挂 app.manage,包 Arc 供 worker 克隆)。
#[derive(Default)]
pub(crate) struct ExternalAgentHost {
    pub(crate) runs: StdMutex<HashMap<String, ExtRun>>,
}

impl ExternalAgentHost {
    pub fn new() -> Self {
        Self::default()
    }
}

/// CLI 预设(首批:Claude Code + Codex;设置里可扩展自定义命令)。
pub(crate) enum AgentPreset {
    Claude,
    Codex,
}

impl AgentPreset {
    pub fn of(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn bin(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn args(&self, prompt: &str) -> Vec<String> {
        match self {
            // -p 非交互;stream-json 逐行 JSON(assistant/result);--verbose 是
            // stream-json 的 CLI 前置要求(缺它会直接报错,reader 自动退纯文本)
            Self::Claude => vec![
                "-p".into(),
                prompt.into(),
                "--output-format".into(),
                "stream-json".into(),
                "--verbose".into(),
            ],
            // codex exec:非交互单任务,stdout 直接是 agent 文本。
            // --skip-git-repo-check 必带:会话工作区常不是 git 仓库(或
            // 不在 codex 受信任列表),缺它 exec 直接报 "Not inside a
            // trusted directory" 退出,任务根本跑不起来(2026-09-11
            // 用户报障"调用不了 codex")。
            Self::Codex => vec!["exec".into(), "--skip-git-repo-check".into(), prompt.into()],
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
        }
    }
}

/// 在候选目录里找可执行文件(Windows 带 .exe/.cmd/.bat 后缀)。
fn find_in_dir(dir: &std::path::Path, bin: &str) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        for ext in ["", ".exe", ".cmd", ".bat"] {
            let p = dir.join(format!("{bin}{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        let p = dir.join(bin);
        if p.is_file() {
            Some(p)
        } else {
            None
        }
    }
}

/// CLI 探测:先扫 PATH,失败后查**已知安装位置**兜底——桌面版/原生
/// 安装器的 CLI 不一定进系统 PATH(实测 Codex 桌面版把 codex.exe 放
/// ~/.codex/.sandbox-bin/,不在 PATH,只扫 PATH 会误报"未安装"→
/// 菜单置灰,2026-09-10 用户报障)。
pub(crate) fn find_in_path(bin: &str) -> Option<String> {
    let home = std::env::var("HOME").ok().map(std::path::PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(std::path::PathBuf::from))?;
    find_in_path_with_home(bin, &home)
}

/// 可注入 home 的探测体(单测用:建临时目录放假 CLI,不碰真实环境变量)。
pub(crate) fn find_in_path_with_home(bin: &str, home: &std::path::Path) -> Option<String> {
    // 1. PATH
    if let Ok(path_env) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_env) {
            if let Some(p) = find_in_dir(&dir, bin) {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    // 2. 已知安装位置(用户目录下的桌面版/原生安装器落点)
    let candidates = [
        // Codex 桌面版(sandbox 内嵌 CLI;多版本并存,取最新)
        home.join(".codex").join(".sandbox-bin"),
        home.join(".codex").join("bin"),
        // Claude Code 原生安装器
        home.join(".claude").join("local"),
        home.join(".claude"),
        // 常见全局 bin
        home.join(".local").join("bin"),
        home.join(".npm-global"),
    ];
    for dir in &candidates {
        if let Some(p) = find_in_dir(dir, bin) {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

/// 外部 CLI 安装探测(PATH 能否找到;前端菜单据此置灰)。
pub(crate) fn probe_external_agents() -> Value {
    json!({
        "claude": find_in_path("claude").is_some(),
        "codex": find_in_path("codex").is_some(),
    })
}

/// 杀进程树:Windows taskkill /T /F(连子进程);POSIX 负 pid(进程组)。
fn kill_tree(pid: u32) {
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
        let _ = Command::new("sh")
            .arg("-c")
            .arg(format!(
                "kill -TERM -{pid} 2>/dev/null; sleep 1; kill -KILL -{pid} 2>/dev/null"
            ))
            .spawn();
    }
}

fn truncate(s: &str, n: usize) -> String {
    let c: String = s.chars().take(n).collect();
    if s.chars().count() <= n {
        c
    } else {
        format!("{c}…")
    }
}

/// 生成 8 位十六进制随机段(会话 id 用;同 session.rs chat- 工作区口径,
/// 外部子会话用 ext- 前缀隔离)。
fn ext_hex8() -> String {
    let mut random = [0u8; 4];
    getrandom::getrandom(&mut random).ok();
    random.iter().map(|b| format!("{b:02x}")).collect()
}

/// 派发外部子代理:同步做完物化 + spawn,worker 线程读 stdout/等进程/收尾,
/// 立即返回 run_id(前端据此可取消/查在跑列表)。CLI 找不到/起不来立即 Err。
pub(crate) fn run_external_agent(
    inner: &Arc<Inner>,
    host: &Arc<ExternalAgentHost>,
    parent_sid: &str,
    agent: &AgentPreset,
    prompt: &str,
    workdir: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    if prompt.trim().is_empty() {
        return Err("任务描述为空".into());
    }
    let exe = find_in_path(agent.bin())
        .ok_or_else(|| format!("未找到 {},检查 PATH 或先安装 CLI", agent.label()))?;
    let workdir = workdir.trim();

    // 子会话 id:ext- 前缀 + 8 hex(过 valid_session_id,与引擎子会话口径一致)
    let hex8 = ext_hex8();
    let child_sid = format!("ext-{hex8}");
    if !valid_session_id(&child_sid) {
        return Err("子会话 id 生成异常".into());
    }
    let tc_id = format!("exttc-{hex8}");
    let run_id = format!("extrun-{hex8}");

    let cancel = Arc::new(AtomicBool::new(false));
    host.runs.lock_ok().insert(
        run_id.clone(),
        ExtRun {
            run_id: run_id.clone(),
            child_sid: child_sid.clone(),
            parent_sid: parent_sid.to_string(),
            tc_id: tc_id.clone(),
            cancel: cancel.clone(),
        },
    );

    // ---- 物化:子会话 + 父卡(与 claim_subagent 的回放形状一致) ----
    let title = format!("{} · {}", agent.label(), truncate(prompt.trim(), 40));
    inner.sess.sessions.lock_ok().insert(
        child_sid.clone(),
        SessionState {
            seq: 0,
            running: true,
            compacting: false,
            manual_compact: false,
            terminal_error_seen: false,
            turn: 1,
            cancel_requested_turn: None,
            created: true,
            engine_id: child_sid.clone(),
            opened: false,
            open_tools: HashMap::new(),
            model_text: String::new(),
            last_event_seq: 0,
            context_usage: None,
            last_billed_usage: None,
            workdir: workdir.to_string(),
            model_name: agent.label().to_string(),
            mode: "default".into(),
            title: title.clone(),
            fold: Default::default(),
        },
    );
    inner.write_sidecar(&child_sid, |m| {
        m["parent"] = json!(parent_sid);
        m["workdir"] = json!(workdir);
        m["model_name"] = json!(agent.label());
        m["title"] = json!(title);
        m["status"] = json!("running");
        m["external"] = json!(agent.label()); // 子会话浮层显示「外部 CLI」徽记
    });
    inner.push_frame(&child_sid, |seq| frame::user_input(prompt, seq));
    inner.push_frame(&child_sid, frame::task_started);
    inner.push_frame(parent_sid, |seq| {
        frame::tool_call(
            &tc_id,
            agent.label(),
            &json!({ "agent": agent.label(), "prompt": prompt, "workdir": workdir }),
            seq,
        )
    });
    inner.push_frame(parent_sid, |seq| {
        frame::tool_call_progress(
            &tc_id,
            json!({ "kind": "child_session", "childSessionId": child_sid, "external": true }),
            seq,
        )
    });

    // ---- spawn CLI(stdout 逐行;Windows 不闪控制台) ----
    let mut cmd = Command::new(&exe);
    cmd.args(agent.args(prompt))
        .current_dir(if workdir.is_empty() { ".".into() } else { workdir.to_string() })
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::wsl::no_console(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            host.runs.lock_ok().remove(&run_id);
            close_ext_child(inner, &child_sid, parent_sid, &tc_id, "failed", Some(&format!("启动失败: {e}")));
            return Err(format!("启动 {exe} 失败: {e}"));
        }
    };

    // ---- worker 线程:读 stdout + 等进程 + 收尾(detached,命令立即返回) ----
    let out = child.stdout.take().expect("stdout piped");
    let err_pipe = child.stderr.take().expect("stderr piped");
    let w_inner = Arc::clone(inner);
    let w_host = Arc::clone(host);
    let w_cancel = Arc::clone(&cancel);
    let w_child_sid = child_sid.clone();
    let w_parent = parent_sid.to_string();
    let w_tc = tc_id.clone();
    let w_run = run_id.clone();
    // 0 = 缺省 10 分钟(外部 CLI 任务通常几分钟;超时后 kill 进程树)
    let w_timeout = if timeout_secs > 0 { timeout_secs } else { 600 };
    let mut w_child = child;

    let _worker = std::thread::spawn(move || {
        // stdout 逐行归约
        let mut json_mode: Option<bool> = None; // None=未定;Some(true)=stream-json;Some(false)=纯文本
        let mut buf = String::new();
        let mut finished_by_result = false;
        for line in BufReader::new(out).lines().flatten() {
            if w_cancel.load(Ordering::SeqCst) {
                break;
            }
            match json_mode {
                Some(true) => {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    if handle_stream_json_line(&w_inner, &w_child_sid, &v) {
                        finished_by_result = true;
                    }
                }
                Some(false) => {
                    buf.push_str(&line);
                    buf.push('\n');
                    if buf.len() >= 4096 {
                        let chunk = std::mem::take(&mut buf);
                        w_inner.push_frame(&w_child_sid, |seq| frame::agent_text(&chunk, seq));
                    }
                }
                None => {
                    // 首行探测:以 '{' 开头且 parse 成 JSON = stream-json;否则纯文本
                    if line.starts_with('{') {
                        if let Ok(v) = serde_json::from_str::<Value>(&line) {
                            json_mode = Some(true);
                            let _ = handle_stream_json_line(&w_inner, &w_child_sid, &v);
                        } else {
                            json_mode = Some(false);
                            buf.push_str(&line);
                            buf.push('\n');
                        }
                    } else {
                        json_mode = Some(false);
                        buf.push_str(&line);
                        buf.push('\n');
                    }
                }
            }
        }
        if !buf.is_empty() && !finished_by_result {
            w_inner.push_frame(&w_child_sid, |seq| frame::agent_text(&buf, seq));
        }

        // 等进程(取消/超时 → kill 树)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(w_timeout);
        let outcome = loop {
            if w_cancel.load(Ordering::SeqCst) {
                kill_tree(w_child.id());
                let _ = w_child.wait();
                break Err("已取消".into());
            }
            if std::time::Instant::now() >= deadline {
                w_cancel.store(true, Ordering::SeqCst);
                kill_tree(w_child.id());
                let _ = w_child.wait();
                break Err(format!("超时(>{w_timeout}s),已终止进程树").into());
            }
            match w_child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        break Ok(());
                    }
                    break Err(format!("退出码 {status}"));
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                Err(e) => break Err(format!("进程读取失败: {e}")),
            }
        };

        // stderr 尾段(诊断):等进程结束后再读(避免管道写满阻塞 CLI)
        let err_tail = {
            let mut t = String::new();
            for l in BufReader::new(err_pipe).lines().flatten() {
                t.push_str(&l);
                t.push('\n');
                if t.len() > 4096 {
                    t = t[t.len() - 4096..].to_string();
                }
            }
            t
        };

        // 收尾:子会话终帧 + sidecar + 父卡 completed/failed
        match &outcome {
            Ok(_) => {
                close_ext_child(&w_inner, &w_child_sid, &w_parent, &w_tc, "finished", None)
            }
            Err(reason) if w_cancel.load(Ordering::SeqCst) => {
                close_ext_child(&w_inner, &w_child_sid, &w_parent, &w_tc, "interrupted", Some(reason))
            }
            Err(reason) => {
                let detail = if err_tail.trim().is_empty() {
                    reason.clone()
                } else {
                    format!("{reason}: {}", truncate(&err_tail, 500))
                };
                close_ext_child(&w_inner, &w_child_sid, &w_parent, &w_tc, "failed", Some(&detail));
            }
        }
        // 清运行注册表
        w_host.runs.lock_ok().remove(&w_run);
    });

    Ok(run_id)
}

/// 取消运行:置 cancel,worker 线程下一轮 100ms 轮询检测后 kill 树。
pub(crate) fn cancel_external_agent(host: &ExternalAgentHost, run_id: &str) -> Result<(), String> {
    let guard = host.runs.lock_ok();
    let run = guard.get(run_id).ok_or_else(|| format!("运行不存在: {run_id}"))?;
    run.cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// 在跑列表(前端显示在跑的外部代理;含 run_id/子会话/父会话)。
pub(crate) fn list_external_runs(host: &ExternalAgentHost) -> Vec<Value> {
    host.runs
        .lock_ok()
        .values()
        .map(|r| {
            json!({
                "run_id": r.run_id,
                "child_sid": r.child_sid,
                "parent_sid": r.parent_sid,
            })
        })
        .collect()
}

/// 收尾漏斗:子会话终帧 + sidecar 状态 + 父卡工具帧(completed/failed)。
/// 与 subagent.rs::close_child 同构,外部子会话按 status 三态收尾。
pub(crate) fn close_ext_child(
    inner: &Inner,
    child_sid: &str,
    parent_sid: &str,
    tc_id: &str,
    status: &str,
    reason: Option<&str>,
) {
    let was = {
        let mut sessions = inner.sess.sessions.lock_ok();
        match sessions.get_mut(child_sid) {
            Some(s) if s.running => {
                s.running = false;
                s.compacting = false;
                s.manual_compact = false;
                s.terminal_error_seen = false;
                s.cancel_requested_turn = None;
                true
            }
            _ => false,
        }
    };
    if !was {
        return;
    }
    match status {
        "interrupted" => {
            inner.push_frame(child_sid, frame::task_ended);
            inner.write_sidecar(child_sid, |m| m["status"] = json!("interrupted"));
            if let Some(r) = reason {
                inner.write_sidecar(child_sid, |m| m["error"] = json!(r));
            }
            inner.push_frame(parent_sid, |seq| {
                frame::tool_call_failed(tc_id, reason.unwrap_or("已取消"), seq)
            });
        }
        "failed" => {
            let msg = reason.unwrap_or("外部 CLI 执行失败");
            inner.push_frame(child_sid, |seq| frame::task_error(msg, seq));
            inner.write_sidecar(child_sid, |m| m["status"] = json!("error"));
            inner.write_sidecar(child_sid, |m| m["error"] = json!(msg));
            inner.push_frame(parent_sid, |seq| frame::tool_call_failed(tc_id, msg, seq));
        }
        _ => {
            inner.push_frame(child_sid, frame::task_ended);
            inner.write_sidecar(child_sid, |m| m["status"] = json!("finished"));
            inner.push_frame(parent_sid, |seq| {
                frame::tool_call_completed(tc_id, "已完成(外部 CLI)", &[], seq)
            });
        }
    }
}

/// stream-json(Claude Code)一行 → 子会话帧。返回 true = 已出终局(result 行)。
fn handle_stream_json_line(inner: &Arc<Inner>, child_sid: &str, v: &Value) -> bool {
    match v.get("type").and_then(|x| x.as_str()).unwrap_or("") {
        "assistant" => {
            if let Some(blocks) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                for b in blocks {
                    match b.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                        "text" => {
                            let t = b.get("text").and_then(|x| x.as_str()).unwrap_or("");
                            if !t.is_empty() {
                                inner.push_frame(child_sid, |seq| frame::agent_text(t, seq));
                            }
                        }
                        "thinking" => {
                            let t = b.get("thinking").and_then(|x| x.as_str()).unwrap_or("");
                            if !t.is_empty() {
                                inner.push_frame(child_sid, |seq| frame::agent_thought(t, seq));
                            }
                        }
                        "tool_use" => {
                            let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("工具");
                            inner.push_frame(child_sid, |seq| {
                                frame::agent_text(&format!("\n🔧 {name}\n"), seq)
                            });
                        }
                        _ => {}
                    }
                }
            }
            false
        }
        "result" => {
            let subtype = v.get("subtype").and_then(|x| x.as_str()).unwrap_or("");
            let result = v.get("result").and_then(|x| x.as_str()).unwrap_or("");
            if !result.is_empty() {
                if subtype == "success" {
                    inner.push_frame(child_sid, |seq| frame::agent_text(result, seq));
                } else {
                    inner.push_frame(child_sid, |seq| frame::task_error(result, seq));
                }
            }
            true
        }
        // system/init、user(工具结果回显)、permission_denied(诊断级)等:忽略
        _ => false,
    }
}

#[cfg(test)]
mod external_tests {
    use super::*;

    /// 手工临时目录(tempfile 不在依赖,避免为测试扩依赖面):
    /// 建唯一目录,测试结束由调用方/进程退出清。返回 home 路径。
    fn tmp_home() -> std::path::PathBuf {
        let n = std::sync::atomic::AtomicU64::new(0);
        let _ = &n;
        let dir = std::env::temp_dir().join(format!(
            "ext-probe-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("mkdir tmp home");
        dir
    }

    #[test]
    fn sandbox_bin_fallback_finds_codex() {
        // 模拟 Codex 桌面版:~/.codex/.sandbox-bin/codex.exe 不在 PATH
        let home = tmp_home();
        let sb = home.join(".codex").join(".sandbox-bin");
        std::fs::create_dir_all(&sb).expect("mkdir sandbox-bin");
        let exe = sb.join(if cfg!(windows) { "codex.exe" } else { "codex" });
        std::fs::write(&exe, b"").expect("write fake cli");
        let found = find_in_path_with_home("codex", &home);
        assert!(found.is_some(), "sandbox-bin 兜底应命中 codex");
        assert!(found.unwrap().starts_with(sb.to_string_lossy().as_ref()));
    }

    #[test]
    fn nothing_installed_returns_none() {
        let home = tmp_home();
        // 空 home + PATH 里没有这个假 CLI → None
        assert!(find_in_path_with_home("definitely_not_a_cli_xyz", &home).is_none());
    }
}
