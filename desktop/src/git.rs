use serde_json::json;
use std::path::Path;
use std::process::Command;
use tauri::Manager;

use crate::config::config_dir;

/// Git 上传/导入(本地任务工作目录 ↔ 远程仓库)。
/// 依赖系统 git 命令;身份统一用 TeemoCode 本地配置(不改用户全局 git 配置)。

fn git(dir: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("无法执行 git: {e}"))?;
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
    std::path::Path::new(dir).join(".git").exists()
}

/// 当前仓库的远程地址(第一个);无则 None。
fn current_remote(dir: &str) -> Option<String> {
    git(dir, &["remote", "get-url", "origin"]).ok().filter(|s| !s.is_empty())
}

fn commit_identity(dir: &str) -> Result<(), String> {
    git(dir, &["-c", "user.name=TeemoCode", "-c", "user.email=teemocode@local", "commit", "-m", "TeemoCode: 本地任务更新"]).map(|_| ())
}

/// 上传:工作目录有 .git → 直接提交推送;没有 → init + 提交 + (给远程地址则)推送。
/// remote_url 可选:目录已有 origin 就用它,否则用传入地址;都没有则只提交不推送。
#[tauri::command]
pub async fn git_push(app: tauri::AppHandle, workdir: String, remote_url: Option<String>) -> Result<serde_json::Value, String> {
    let dir = workdir.trim();
    if dir.is_empty() {
        return Err("工作目录为空".into());
    }
    if !std::path::Path::new(dir).exists() {
        return Err(format!("工作目录不存在: {dir}"));
    }
    // 把绑定当前工作目录的任务会话数据导出到项目 .teemocode/,随代码一起提交推送
    let _ = export_task_data(&app, dir);
    if !has_git(dir) {
        git(dir, &["init"]).map_err(|e| format!("git init 失败: {e}"))?;
    }
    git(dir, &["add", "-A"]).map_err(|e| format!("git add 失败: {e}"))?;
    // 有改动才提交(避免"nothing to commit"报错)
    let changed = git(dir, &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(true);
    if changed {
        commit_identity(dir).map_err(|e| format!("git commit 失败: {e}"))?;
    }
    // 远程:已有 origin 优先;否则用传入地址
    let remote = current_remote(dir).or(remote_url.filter(|r| !r.trim().is_empty()));
    let mut pushed = false;
    let branch = git(dir, &["branch", "--show-current"]).unwrap_or_default();
    if let Some(url) = remote.as_deref() {
        if git(dir, &["remote", "get-url", "origin"]).is_err() {
            git(dir, &["remote", "add", "origin", url]).map_err(|e| format!("添加远程失败: {e}"))?;
        }
        let push_branch = if branch.is_empty() { "master" } else { &branch };
        git(dir, &["push", "-u", "origin", push_branch]).map_err(|e| format!("git push 失败: {e}"))?;
        pushed = true;
    }
    let hash = git(dir, &["rev-parse", "--short", "HEAD"]).ok();
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
fn export_task_data(app: &tauri::AppHandle, workdir: &str) -> Result<usize, String> {
    let root = config_dir(app)?;
    let sid_list = bound_sids(app, workdir);
    let mut exported = 0usize;
    for sid in &sid_list {
        for rel in ["ohmy-sessions", "ohmyagent/sessions", "ohmyagent/tasks"] {
            let src = root.join(rel).join(sid);
            if !src.is_dir() {
                continue;
            }
            let dest = Path::new(workdir).join(".teemocode").join(rel).join(sid);
            if !dest.is_dir() {
                std::fs::create_dir_all(&dest).map_err(|e| format!("创建导出目录失败: {e}"))?;
            }
            if let Ok(files) = std::fs::read_dir(&src) {
                for f in files.flatten() {
                    let name = f.file_name().to_string_lossy().to_string();
                    let _ = std::fs::copy(f.path(), dest.join(&name));
                }
            }
        }
        exported += 1;
    }
    Ok(exported)
}

/// 绑定某工作目录的会话 sid 集合(壳侧 ohmy-sessions 的 meta.workdir 匹配)。
fn bound_sids(app: &tauri::AppHandle, workdir: &str) -> Vec<String> {
    let Ok(root) = config_dir(app) else { return vec![] };
    let shell = root.join("ohmy-sessions");
    let mut out = vec![];
    let Ok(entries) = std::fs::read_dir(&shell) else { return out };
    for e in entries.flatten() {
        let sid = e.file_name().to_string_lossy().to_string();
        let meta_p = e.path().join("meta.json");
        if !meta_p.is_file() {
            continue;
        }
        if let Ok(meta) = read_json(&meta_p) {
            let wd = meta.get("workdir").and_then(|v| v.as_str()).unwrap_or("");
            if wd == workdir {
                out.push(sid);
            }
        }
    }
    out
}

/// 导入项目后检测任务数据:项目内 `.teemocode/` 三目录视为导出的本地任务
/// 会话数据,迁移到配置目录(本地任务工作区)并把 workdir 改成当前目录。
/// 导入:按 git 地址加载到工作目录并拉取代码。
#[tauri::command]
pub async fn git_import(workdir: String, url: String) -> Result<serde_json::Value, String> {
    let dir = workdir.trim();
    let url = url.trim();
    if dir.is_empty() {
        return Err("工作目录为空".into());
    }
    if url.is_empty() {
        return Err("git 地址为空".into());
    }
    if !std::path::Path::new(dir).exists() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建工作目录失败: {e}"))?;
    }
    let empty = std::fs::read_dir(dir).map(|mut it| it.next().is_none()).unwrap_or(true);
    if has_git(dir) {
        git(dir, &["pull"]).map_err(|e| format!("git pull 失败: {e}"))?;
    } else if empty {
        git(dir, &["clone", url, "."]).map_err(|e| format!("git clone 失败: {e}"))?;
    } else {
        git(dir, &["init"]).map_err(|e| format!("git init 失败: {e}"))?;
        git(dir, &["remote", "add", "origin", url]).map_err(|e| format!("添加远程失败: {e}"))?;
        // 先试默认分支拉取
        let pulled = git(dir, &["pull", "origin", "main"])
            .or_else(|_| git(dir, &["pull", "origin", "master"]));
        if let Err(e) = pulled {
            return Err(format!("git pull 失败: {e}"));
        }
    }
    let branch = git(dir, &["branch", "--show-current"]).unwrap_or_default();
    Ok(json!({
        "ok": true,
        "branch": if branch.is_empty() { "main".to_string() } else { branch },
        "remote": url,
    }))
}

/// 导入项目后检测任务数据:项目内 `.teemocode/<sid>/meta.json` 视为导出的
/// 本地任务会话数据,迁移到应用数据目录(本地任务工作区)并把 workdir 改成
/// 当前选择的目录。返回迁移数量,前端提示重启重新加载任务数据。
#[tauri::command]
pub async fn import_task_data(app: tauri::AppHandle, workdir: String) -> Result<serde_json::Value, String> {
    let src = Path::new(&workdir).join(".teemocode");
    if !src.is_dir() {
        return Ok(json!({ "migrated": 0, "sessions": [] }));
    }
    let data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("取应用数据目录失败: {e}"))?;
    let mut migrated = 0usize;
    let mut sessions: Vec<String> = vec![];
    let entries = std::fs::read_dir(&src).map_err(|e| format!("读取 .teemocode 失败: {e}"))?;
    for entry in entries.flatten() {
        let sid = entry.file_name().to_string_lossy().to_string();
        let meta_src = entry.path().join("meta.json");
        if !meta_src.is_file() {
            continue;
        }
        let dest = data.join(&sid);
        if !dest.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| format!("创建会话目录失败: {e}"))?;
        }
        if let Ok(mut meta) = read_json(&meta_src) {
            meta["workdir"] = json!(workdir);
            write_json(&dest.join("meta.json"), &meta)?;
            if let Ok(files) = std::fs::read_dir(entry.path()) {
                for f in files.flatten() {
                    let name = f.file_name().to_string_lossy().to_string();
                    if name == "meta.json" {
                        continue;
                    }
                    let _ = std::fs::copy(f.path(), dest.join(&name));
                }
            }
            migrated += 1;
            sessions.push(sid);
        }
    }
    Ok(json!({ "migrated": migrated, "sessions": sessions }))
}

fn read_json(p: &Path) -> Result<serde_json::Value, String> {
    let s = std::fs::read_to_string(p).map_err(|e| format!("读取 {} 失败: {e}", p.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("解析 {} 失败: {e}", p.display()))
}

fn write_json(p: &Path, v: &serde_json::Value) -> Result<(), String> {
    let s = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    std::fs::write(p, s).map_err(|e| format!("写入 {} 失败: {e}", p.display()))
}

/// 重启应用(任务数据迁移后让用户重启重新加载)。
#[tauri::command]
pub fn relaunch_app(app: tauri::AppHandle) {
    app.restart();
}
