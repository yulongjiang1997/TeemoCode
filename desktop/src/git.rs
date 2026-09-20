use serde_json::json;
use std::path::Path;
use std::process::Command;
use tauri::AppHandle;

use crate::config::{config_dir, load_config};

/// 与 export/import_mc 一致的三处任务数据相对路径。
const TASK_DATA_RELS: [&str; 3] = ["ohmy-sessions", "ohmyagent/sessions", "ohmyagent/tasks"];

/// 首次 `git init` 且目录还没有 .gitignore 时写入,避免把密钥和构建产物推进远程。
/// 已有 .gitignore 绝不覆盖。
const DEFAULT_GITIGNORE: &str = "\
.env\n\
.env.*\n\
!.env.example\n\
!.env.sample\n\
node_modules/\n\
target/\n\
dist/\n\
build/\n\
__pycache__/\n\
*.pem\n\
*.key\n\
*.p12\n\
.DS_Store\n\
Thumbs.db\n\
";

/// Git 上传/导入(本地任务工作目录 ↔ 远程仓库)。
/// 依赖系统 git 命令;身份统一用 TeemoCode 本地配置(不改用户全局 git 配置)。

fn git(dir: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0");
    // Windows 下不闪 cmd 控制台窗口(切任务刷新分支每次都会 spawn git)
    crate::wsl::no_console(&mut cmd);
    let out = cmd.output().map_err(|e| format!("无法执行 git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            err
        })
    }
}

fn has_git(dir: &str) -> bool {
    Path::new(dir).join(".git").exists()
}

/// 把会话 sidecar 里的 guest 路径翻成宿主 git 能 `current_dir` 的路径。
/// 已经是存在的 Windows/UNC 路径则原样返回(新建任务目录选择器走这条)。
fn git_host_dir(app: &AppHandle, workdir: &str) -> Result<String, String> {
    let raw = workdir.trim();
    if raw.is_empty() {
        return Err("工作目录为空".into());
    }
    if Path::new(raw).exists() {
        return Ok(raw.to_string());
    }
    // 只有 guest 绝对路径才翻 \\wsl$ ;盘符路径不存在就是不存在,
    // 不能拿 "D:\proj" 去拼 \\wsl$\Ubuntu\D:\proj。
    if raw.starts_with('/') {
        if let Some(distro) = load_config(app)
            .ok()
            .and_then(|c| crate::wsl::distro_of(&c.kernel_env).map(str::to_string))
        {
            let host = crate::wsl::host_fs_view(&distro, raw);
            if host.exists() {
                return Ok(host.to_string_lossy().into_owned());
            }
            return Err(format!("工作目录不存在: {raw} (WSL {distro})"));
        }
    }
    Err(format!("工作目录不存在: {raw}"))
}

fn normalize_workdir(s: &str) -> String {
    let s = s.trim().replace('\\', "/");
    let s = s.trim_end_matches('/');
    #[cfg(windows)]
    {
        s.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        s.to_string()
    }
}

fn workdir_eq(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && normalize_workdir(a) == normalize_workdir(b)
}

fn workdir_matches(meta_wd: &str, raw: &str, host: &str, distro: Option<&str>) -> bool {
    if workdir_eq(meta_wd, raw) || workdir_eq(meta_wd, host) {
        return true;
    }
    if let Some(d) = distro {
        let meta_host = crate::wsl::host_fs_view(d, meta_wd);
        if workdir_eq(&meta_host.to_string_lossy(), host) {
            return true;
        }
    }
    false
}

fn copy_tree(src: &Path, dst: &Path) -> Result<u64, String> {
    if !src.exists() {
        return Ok(0);
    }
    if src.is_file() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
        }
        std::fs::copy(src, dst).map_err(|e| format!("复制 {} 失败: {e}", src.display()))?;
        return Ok(1);
    }
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目录 {} 失败: {e}", dst.display()))?;
    let mut n = 0u64;
    let entries = std::fs::read_dir(src).map_err(|e| format!("读取 {} 失败: {e}", src.display()))?;
    for entry in entries.flatten() {
        n += copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
    }
    Ok(n)
}

fn ensure_default_gitignore(dir: &str) -> Result<(), String> {
    let p = Path::new(dir).join(".gitignore");
    if p.exists() {
        return Ok(());
    }
    std::fs::write(&p, DEFAULT_GITIGNORE).map_err(|e| format!("写入 .gitignore 失败: {e}"))
}

fn stage_for_push(dir: &str, new_repo: bool) -> Result<(), String> {
    if new_repo {
        ensure_default_gitignore(dir)?;
        git(dir, &["add", "-A"]).map_err(|e| format!("git add 失败: {e}"))?;
    } else {
        // 已有仓库:只提交已跟踪变更 + 任务数据,不把未跟踪的 .env / 构建产物顺手推进远程。
        git(dir, &["add", "-u"]).map_err(|e| format!("git add 失败: {e}"))?;
        if Path::new(dir).join(".teemocode").is_dir() {
            git(dir, &["add", "--", ".teemocode"]).map_err(|e| format!("git add .teemocode 失败: {e}"))?;
        }
    }
    Ok(())
}

fn ensure_origin(dir: &str, url: &str) -> Result<(), String> {
    match git(dir, &["remote", "get-url", "origin"]) {
        Ok(existing) if existing.trim() == url => Ok(()),
        Ok(_) => git(dir, &["remote", "set-url", "origin", url])
            .map(|_| ())
            .map_err(|e| format!("更新远程失败: {e}")),
        Err(_) => git(dir, &["remote", "add", "origin", url])
            .map(|_| ())
            .map_err(|e| format!("添加远程失败: {e}")),
    }
}

/// 当前仓库的远程地址(第一个);无则 None。
fn current_remote(dir: &str) -> Option<String> {
    git(dir, &["remote", "get-url", "origin"]).ok().filter(|s| !s.is_empty())
}

fn commit_identity(dir: &str) -> Result<(), String> {
    git(dir, &["-c", "user.name=TeemoCode", "-c", "user.email=teemocode@local", "commit", "-m", "TeemoCode: 本地任务更新"]).map(|_| ())
}

/// 上传:工作目录有 .git → 提交推送;没有 → init + 提交 + (给远程地址则)推送。
/// remote_url 可选:留空则用已有 origin;填写则把 origin 设成该地址再推。
/// 都没有则只提交不推送。
#[tauri::command]
pub async fn git_push(app: AppHandle, workdir: String, remote_url: Option<String>) -> Result<serde_json::Value, String> {
    let raw = workdir.trim().to_string();
    let dir = git_host_dir(&app, &raw)?;
    // 任务数据必须进仓库;失败就停,避免用户以为会话已随代码推走。
    export_task_data(&app, &raw, &dir)?;
    let new_repo = !has_git(&dir);
    if new_repo {
        git(&dir, &["init"]).map_err(|e| format!("git init 失败: {e}"))?;
    }
    stage_for_push(&dir, new_repo)?;
    let status = git(&dir, &["status", "--porcelain"]).map_err(|e| format!("git status 失败: {e}"))?;
    if !status.is_empty() {
        commit_identity(&dir).map_err(|e| format!("git commit 失败: {e}"))?;
    }
    let provided = remote_url
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string);
    if let Some(url) = provided.as_deref() {
        ensure_origin(&dir, url)?;
    }
    let remote = current_remote(&dir).or(provided);
    let mut pushed = false;
    let branch = git(&dir, &["branch", "--show-current"]).unwrap_or_default();
    if let Some(url) = remote.as_deref() {
        ensure_origin(&dir, url)?;
        let push_branch = if branch.is_empty() { "master" } else { &branch };
        git(&dir, &["push", "-u", "origin", push_branch]).map_err(|e| format!("git push 失败: {e}"))?;
        pushed = true;
    }
    let hash = git(&dir, &["rev-parse", "--short", "HEAD"]).ok();
    Ok(json!({
        "ok": true,
        "pushed": pushed,
        "remote": remote,
        "branch": if branch.is_empty() { "master".to_string() } else { branch },
        "commit": hash,
    }))
}

/// 把绑定该工作目录的任务会话数据导出到项目 `.teemocode/`,供 git 一起提交。
/// 覆盖三处数据目录(与导入原版 monkeycode 一致):
///   ohmy-sessions/<sid>/       壳侧会话(meta.json + events.jsonl)
///   ohmyagent/sessions/<sid>/  引擎对话(messages.jsonl)
///   ohmyagent/tasks/<sid>/     任务状态(tasks.json + notifications.json)
fn export_task_data(app: &AppHandle, raw: &str, host: &str) -> Result<usize, String> {
    let root = config_dir(app)?;
    let distro = load_config(app)
        .ok()
        .and_then(|c| crate::wsl::distro_of(&c.kernel_env).map(str::to_string));
    let sid_list = bound_sids(&root, raw, host, distro.as_deref());
    export_task_data_from(&root, host, &sid_list)
}

fn export_task_data_from(root: &Path, host: &str, sid_list: &[String]) -> Result<usize, String> {
    let mut exported = 0usize;
    for sid in sid_list {
        if !crate::driver::valid_session_id(sid) {
            continue;
        }
        let mut any = false;
        for rel in TASK_DATA_RELS {
            let src = root.join(rel).join(sid);
            if !src.is_dir() {
                continue;
            }
            let dest = Path::new(host).join(".teemocode").join(rel).join(sid);
            copy_tree(&src, &dest)?;
            any = true;
        }
        if any {
            exported += 1;
        }
    }
    Ok(exported)
}

/// 绑定某工作目录的会话 sid 集合(壳侧 ohmy-sessions 的 meta.workdir 匹配)。
/// raw = 会话逻辑路径(WSL 下常为 guest),host = 宿主 git 工作目录。
fn bound_sids(root: &Path, raw: &str, host: &str, distro: Option<&str>) -> Vec<String> {
    let shell = root.join("ohmy-sessions");
    let mut out = vec![];
    let Ok(entries) = std::fs::read_dir(&shell) else {
        return out;
    };
    for e in entries.flatten() {
        let sid = e.file_name().to_string_lossy().to_string();
        if !crate::driver::valid_session_id(&sid) {
            continue;
        }
        let meta_p = e.path().join("meta.json");
        if !meta_p.is_file() {
            continue;
        }
        if let Ok(meta) = read_json(&meta_p) {
            let wd = meta.get("workdir").and_then(|v| v.as_str()).unwrap_or("");
            if workdir_matches(wd, raw, host, distro) {
                out.push(sid);
            }
        }
    }
    out
}

/// 导入:按 git 地址加载到工作目录并拉取代码。
/// 目录已是 git 仓库时,把 origin 设成传入地址再 pull(不再忽略 URL)。
#[tauri::command]
pub async fn git_import(app: AppHandle, workdir: String, url: String) -> Result<serde_json::Value, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("git 地址为空".into());
    }
    let raw = workdir.trim();
    if raw.is_empty() {
        return Err("工作目录为空".into());
    }
    let dir = match git_host_dir(&app, raw) {
        Ok(d) => d,
        Err(e) => {
            // 宿主路径还不存在时创建(新建任务选了空目录);guest 路径(/mnt/...)
            // 在 Windows 上 create_dir_all 会在盘符根下造出错误目录,直接失败。
            if raw.starts_with('/') || raw.starts_with('\\') {
                return Err(e);
            }
            std::fs::create_dir_all(raw).map_err(|err| format!("创建工作目录失败: {err}"))?;
            raw.to_string()
        }
    };
    if !Path::new(&dir).exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建工作目录失败: {e}"))?;
    }
    let empty = match std::fs::read_dir(&dir) {
        Ok(mut it) => it.next().is_none(),
        Err(e) => return Err(format!("读取工作目录失败: {e}")),
    };
    if has_git(&dir) {
        ensure_origin(&dir, url)?;
        if git(&dir, &["pull"]).is_err() {
            git(&dir, &["pull", "origin", "main"])
                .or_else(|_| git(&dir, &["pull", "origin", "master"]))
                .map_err(|e| format!("git pull 失败: {e}"))?;
        }
    } else if empty {
        git(&dir, &["clone", url, "."]).map_err(|e| format!("git clone 失败: {e}"))?;
    } else {
        git(&dir, &["init"]).map_err(|e| format!("git init 失败: {e}"))?;
        git(&dir, &["remote", "add", "origin", url]).map_err(|e| format!("添加远程失败: {e}"))?;
        let pulled = git(&dir, &["pull", "origin", "main"])
            .or_else(|_| git(&dir, &["pull", "origin", "master"]));
        if let Err(e) = pulled {
            return Err(format!("git pull 失败: {e}"));
        }
    }
    let branch = git(&dir, &["branch", "--show-current"]).unwrap_or_default();
    Ok(json!({
        "ok": true,
        "branch": if branch.is_empty() { "main".to_string() } else { branch },
        "remote": url,
    }))
}

/// 导入项目后检测 `.teemocode/` 任务数据,按导出的三目录迁到配置目录
/// (`ohmy-sessions` / `ohmyagent/sessions` / `ohmyagent/tasks`),并把 sidecar
/// workdir 改成当前目录。兼容旧扁平 `.teemocode/<sid>/meta.json`。
#[tauri::command]
pub async fn import_task_data(app: AppHandle, workdir: String) -> Result<serde_json::Value, String> {
    let raw = workdir.trim();
    if raw.is_empty() {
        return Err("工作目录为空".into());
    }
    let host = git_host_dir(&app, raw).unwrap_or_else(|_| raw.to_string());
    let src = Path::new(&host).join(".teemocode");
    if !src.is_dir() {
        return Ok(json!({ "migrated": 0, "sessions": [] }));
    }
    let cfg = config_dir(&app)?;
    let (migrated, sessions) = import_task_data_into(&cfg, raw, &src)?;
    Ok(json!({ "migrated": migrated, "sessions": sessions }))
}

fn collect_import_sids(src: &Path) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut sids = BTreeSet::new();
    let take_dir = |dir: &Path, require_meta: bool, out: &mut BTreeSet<String>| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            let sid = e.file_name().to_string_lossy().to_string();
            if !crate::driver::valid_session_id(&sid) {
                continue;
            }
            if require_meta && !e.path().join("meta.json").is_file() {
                continue;
            }
            out.insert(sid);
        }
    };
    take_dir(&src.join("ohmy-sessions"), true, &mut sids);
    take_dir(&src.join("ohmyagent").join("sessions"), false, &mut sids);
    take_dir(&src.join("ohmyagent").join("tasks"), false, &mut sids);
    // 旧扁平: .teemocode/<sid>/meta.json(不要把 ohmy-sessions / ohmyagent 当 sid)
    if let Ok(entries) = std::fs::read_dir(src) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name == "ohmy-sessions" || name == "ohmyagent" {
                continue;
            }
            if !crate::driver::valid_session_id(&name) {
                continue;
            }
            if e.path().join("meta.json").is_file() {
                sids.insert(name);
            }
        }
    }
    sids.into_iter().collect()
}

fn import_task_data_into(cfg_root: &Path, session_workdir: &str, src: &Path) -> Result<(usize, Vec<String>), String> {
    if !src.is_dir() {
        return Ok((0, vec![]));
    }
    let sids = collect_import_sids(src);
    let mut migrated = 0usize;
    let mut sessions = Vec::new();
    for sid in sids {
        let nested = src.join("ohmy-sessions").join(&sid);
        let flat = src.join(&sid);
        let shell_src = if nested.join("meta.json").is_file() {
            nested
        } else {
            flat
        };
        if shell_src.join("meta.json").is_file() {
            let dest = cfg_root.join("ohmy-sessions").join(&sid);
            copy_tree(&shell_src, &dest)?;
            let meta_p = dest.join("meta.json");
            if let Ok(mut meta) = read_json(&meta_p) {
                meta["workdir"] = json!(session_workdir);
                write_json(&meta_p, &meta)?;
            }
        }
        for rel in ["ohmyagent/sessions", "ohmyagent/tasks"] {
            let from = src.join(rel).join(&sid);
            if from.is_dir() {
                copy_tree(&from, &cfg_root.join(rel).join(&sid))?;
            }
        }
        migrated += 1;
        sessions.push(sid);
    }
    Ok((migrated, sessions))
}

fn read_json(p: &Path) -> Result<serde_json::Value, String> {
    let s = std::fs::read_to_string(p).map_err(|e| format!("读取 {} 失败: {e}", p.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("解析 {} 失败: {e}", p.display()))
}

fn write_json(p: &Path, v: &serde_json::Value) -> Result<(), String> {
    let s = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    std::fs::write(p, s).map_err(|e| format!("写入 {} 失败: {e}", p.display()))
}

/// 查询当前分支名。返回 branch name 或空字符串(无 git 仓库)。
/// ⚠ async + spawn_blocking:git 子进程是阻塞重活,同步命令会在主线程
/// 执行并冻结 WebView(切任务时此命令必触发,大仓库/杀慢时数秒卡死)。
#[tauri::command]
pub async fn git_branch(app: AppHandle, workdir: String) -> String {
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(dir) = git_host_dir(&app, &workdir) else {
            return String::new();
        };
        match git(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]) {
            Ok(s) => s.trim().to_string(),
            Err(_) => String::new(),
        }
    })
    .await
    .unwrap_or_default()
}

/// 列出所有本地分支名。返回 Vec<String>。
#[tauri::command]
pub async fn git_branch_list(app: AppHandle, workdir: String) -> Vec<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(dir) = git_host_dir(&app, &workdir) else {
            return Vec::new();
        };
        // --format 在部分 Windows git 上不稳定;用 --list + 逐行 trim 最稳妥
        match git(&dir, &["branch", "--list"]) {
            Ok(s) => s
                .lines()
                .map(|l| l.trim_start_matches("* ").trim().trim_start_matches(' '))
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect(),
            Err(_) => Vec::new(),
        }
    })
    .await
    .unwrap_or_default()
}

/// 检查工作区是否干净(无未提交更改)。
#[tauri::command]
pub async fn git_is_clean(app: AppHandle, workdir: String) -> bool {
    tauri::async_runtime::spawn_blocking(move || {
        if workdir.trim().is_empty() {
            return true;
        }
        let Ok(dir) = git_host_dir(&app, &workdir) else {
            return false;
        };
        // 查不出来就当不干净,挡住切分支(旧实现失败当干净,可能带着未提交改动切走)
        git(&dir, &["status", "--porcelain"])
            .map(|s| s.trim().is_empty())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// 切换分支。失败返回错误信息。
#[tauri::command]
pub async fn git_checkout(app: AppHandle, workdir: String, branch: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = git_host_dir(&app, &workdir)?;
        let status = git(&dir, &["status", "--porcelain"]).map_err(|e| format!("无法检查工作区: {e}"))?;
        if !status.trim().is_empty() {
            return Err("工作区有未提交的更改,请先提交或丢弃后再切换分支".into());
        }
        git(&dir, &["checkout", &branch]).map(|_| ()).map_err(|e| format!("切换失败: {e}"))
    })
    .await
    .map_err(|e| format!("切换任务失败: {e}"))?
}

/// 重启应用(任务数据迁移后让用户重启重新加载)。
#[tauri::command]
pub fn relaunch_app(app: tauri::AppHandle) {
    app.restart();
}

/// 克隆 git 仓库到临时目录并扫描 SKILL.md 技能文件。
/// 返回技能原始信息列表(name/description/content/相对路径),供大模型解析。
#[tauri::command]
pub async fn skills_import_git(url: String) -> Result<serde_json::Value, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("git 地址为空".into());
    }
    let rand: u128 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("skills-import-{rand}"));
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    std::fs::create_dir_all(&tmp).map_err(|e| format!("创建临时目录失败: {e}"))?;
    let tmp_s = tmp.to_string_lossy().to_string();
    if let Err(e) = git(&tmp_s, &["clone", "--depth", "1", url, "."]) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("git clone 失败: {e}"));
    }
    let mut skills: Vec<serde_json::Value> = Vec::new();
    scan_skill_dirs(&tmp, &tmp, &mut skills);
    if skills.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("仓库中未找到 SKILL.md 技能文件(需形如 <skill-name>/SKILL.md)".into());
    }

    // 扫描仓库根 mcp.json(引擎同构形态 {"mcpServers":{...}}),若存在
    // 一并返回,供技能市场的 MCP 一键装功能消费。
    let mcp_value = std::fs::read_to_string(tmp.join("mcp.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("mcpServers").cloned())
        .filter(|v| v.is_object());
    // 技能正文已读进内存,临时克隆不再需要;成功路径也必须清掉。
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(json!({ "skills": skills, "mcp": mcp_value }))
}

/// 递归扫描目录下所有 SKILL.md(最多 5 层深),读取 name/description/content。
fn scan_skill_dirs(root: &Path, dir: &Path, out: &mut Vec<serde_json::Value>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().map(|n| n == ".git").unwrap_or(false) {
                continue;
            }
            let depth = p.strip_prefix(root).map(|r| r.components().count()).unwrap_or(0);
            if depth <= 5 {
                scan_skill_dirs(root, &p, out);
            }
        } else if p.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
            let Ok(content) = std::fs::read_to_string(&p) else { continue };
            let rel = p
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let dir_name = p
                .parent()
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "skill".into());
            let mut name = dir_name.clone();
            let mut desc = String::new();
            let mut in_fm = false;
            for line in content.lines() {
                let t = line.trim();
                if t == "---" {
                    if in_fm {
                        break;
                    }
                    in_fm = true;
                    continue;
                }
                if in_fm {
                    if let Some(v) = t.strip_prefix("name:") {
                        let v = v.trim().trim_matches('"');
                        if !v.is_empty() {
                            name = v.to_string();
                        }
                    } else if let Some(v) = t.strip_prefix("description:") {
                        desc = v.trim().trim_matches('"').to_string();
                    }
                }
            }
            if desc.is_empty() {
                desc = content
                    .lines()
                    .skip_while(|l| l.trim().is_empty() || l.trim() == "---")
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(120)
                    .collect();
            }
            out.push(json!({
                "dir_name": dir_name,
                "name": name,
                "description": desc,
                "rel_path": rel,
                "content": content,
            }));
        }
    }
}

/// 用大模型解析单个 SKILL.md 内容,提取结构化摘要(名称/作用/关键信息/适用场景)。
/// 复用 model_test 的 provider/base/key 口径(openai/anthropic 两种协议)。
#[tauri::command]
pub async fn skill_analyze(
    provider: String,
    base_url: String,
    api_key: String,
    model: String,
    content: String,
) -> Result<String, String> {
    let base = base_url.trim().trim_end_matches('/').trim_end_matches("/v1").to_string();
    if base.is_empty() {
        return Err("请先填写接口地址".into());
    }
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return Err("请先填写 API Key".into());
    }
    let model = model.trim().to_string();
    if model.is_empty() {
        return Err("请先填写模型标识".into());
    }
    let prompt = format!(
        "你是技能库分析师。以下是一个 Agent 技能(SKILL.md)的完整内容。请用中文输出严格的 JSON(不要 markdown 代码块包裹),格式:\n{{\"summary\": \"一句话作用概括(50字内)\", \"details\": \"详细说明(功能、用法、前置条件,200字内)\", \"keywords\": [\"关键词1\", ...], \"scenarios\": [\"适用场景1\", ...]}}\n\n技能名与 description 已解析,重点分析正文。\n\n--- SKILL.md 内容 ---\n{content}"
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    // Responses API(openai_responses)的请求/响应结构都不同:
    // 请求 POST {base}/responses {model, input};回复 output_text 串联。
    // base 已带版本段(如智谱 /api/coding/paas/v4)时不再补 /v1,防 /v4/v1/... 404。
let seg = base.rsplit('/').next().unwrap_or("");
let versioned = seg.len() >= 2 && seg.starts_with('v') && seg[1..].bytes().all(|c| c.is_ascii_digit());
    let vpath = |p: &str| if versioned { format!("{base}/{p}") } else { format!("{base}/v1/{p}") };
    let (url, req) = match provider.as_str() {
        "anthropic" => (
            vpath("messages"),
            client
                .post(vpath("messages"))
                .header("x-api-key", &key)
                .header("anthropic-version", "2023-06-01")
                .json(&serde_json::json!({
                    "model": model,
                    "max_tokens": 1024,
                    "messages": [{ "role": "user", "content": prompt }]
                })),
        ),
        "openai_responses" => (
            vpath("responses"),
            client
                .post(vpath("responses"))
                .bearer_auth(&key)
                .json(&serde_json::json!({
                    "model": model,
                    "input": prompt,
                })),
        ),
        _ => (
            vpath("chat/completions"),
            client
                .post(vpath("chat/completions"))
                .bearer_auth(&key)
                .json(&serde_json::json!({
                    "model": model,
                    "messages": [{ "role": "user", "content": prompt }]
                })),
        ),
    };
    let resp = req.send().await.map_err(|e| {
        if e.is_timeout() { "请求超时(120s)".to_string() } else { format!("无法连接: {e}") }
    })?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|_| format!("响应不是有效 JSON (HTTP {status})"))?;
    if !status.is_success() {
        let msg = body.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()).unwrap_or("");
        return Err(format!("HTTP {status} @ {url}{}", if msg.is_empty() { String::new() } else { format!(": {msg}") }));
    }
    // 提取回复文本(openai: choices[0].message.content;anthropic: content[0].text;
    // responses: output_text 或 output[].content[].text)
    let text = body
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("output_text").and_then(|v| v.as_str()))
        .or_else(|| {
            body.get("content")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("text"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            // responses API: output[] 里 type=message 的 content[] 里 type=output_text 的 text
            body.get("output")
                .and_then(|o| o.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .filter_map(|step| step.get("content").and_then(|c| c.as_array()))
                        .flatten()
                        .filter_map(|part| part.get("text").and_then(|v| v.as_str()))
                        .next()
                })
        })
        .unwrap_or("")
        .to_string();
    if text.is_empty() {
        return Err("模型返回为空".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mc-git-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(p: &Path, s: &str) {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn workdir_matches_slash_and_guest_host() {
        assert!(workdir_eq(r"D:\proj", "D:/proj"));
        assert!(workdir_matches("/mnt/c/proj", "/mnt/c/proj", r"\\wsl$\U\mnt\c\proj", None));
        assert!(!workdir_matches("/mnt/c/other", "/mnt/c/proj", r"D:\proj", None));
    }

    #[test]
    fn import_roundtrip_three_dir_layout() {
        let root = tmp("round");
        let cfg = root.join("cfg");
        let work = root.join("work");
        let sid = "abcd1234";
        write(&cfg.join("ohmy-sessions").join(sid).join("meta.json"), r#"{"workdir":"/mnt/c/old","title":"t"}"#);
        write(&cfg.join("ohmy-sessions").join(sid).join("events.jsonl"), "e\n");
        write(&cfg.join("ohmyagent").join("sessions").join(sid).join("messages.jsonl"), "{}\n");
        write(&cfg.join("ohmyagent").join("sessions").join(sid).join("nested").join("x.txt"), "x");
        write(&cfg.join("ohmyagent").join("tasks").join(sid).join("tasks.json"), "[]");

        export_task_data_from(&cfg, work.to_str().unwrap(), &[sid.to_string()]).unwrap();
        let packed = work.join(".teemocode");
        assert!(packed.join("ohmy-sessions").join(sid).join("meta.json").is_file());
        assert!(packed.join("ohmyagent").join("sessions").join(sid).join("nested").join("x.txt").is_file());

        let dest = root.join("dest");
        let (n, sessions) = import_task_data_into(&dest, r"D:\now", &packed).unwrap();
        assert_eq!(n, 1);
        assert_eq!(sessions, vec![sid.to_string()]);
        let meta: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dest.join("ohmy-sessions").join(sid).join("meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["workdir"], json!(r"D:\now"));
        assert!(dest.join("ohmyagent").join("sessions").join(sid).join("nested").join("x.txt").is_file());
        assert!(dest.join("ohmyagent").join("tasks").join(sid).join("tasks.json").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn import_flat_legacy_layout() {
        let root = tmp("flat");
        let src = root.join(".teemocode");
        let sid = "deadbeef";
        write(&src.join(sid).join("meta.json"), r#"{"title":"old"}"#);
        write(&src.join(sid).join("events.jsonl"), "e\n");
        let dest = root.join("cfg");
        let (n, _) = import_task_data_into(&dest, "/tmp/w", &src).unwrap();
        assert_eq!(n, 1);
        assert!(dest.join("ohmy-sessions").join(sid).join("events.jsonl").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn import_ignores_top_level_files() {
        let root = tmp("file");
        let src = root.join(".teemocode");
        write(&src.join("meta.json"), "{}");
        write(&src.join("ohmy-sessions").join("readme.txt"), "no");
        let dest = root.join("cfg");
        let (n, sessions) = import_task_data_into(&dest, "/w", &src).unwrap();
        assert_eq!(n, 0);
        assert!(sessions.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_sids_skips_layout_dir_names() {
        let root = tmp("layout");
        let src = root.join(".teemocode");
        std::fs::create_dir_all(src.join("ohmy-sessions")).unwrap();
        std::fs::create_dir_all(src.join("ohmyagent")).unwrap();
        assert!(collect_import_sids(&src).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
