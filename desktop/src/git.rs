use serde_json::json;
use std::process::Command;

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
pub async fn git_push(workdir: String, remote_url: Option<String>) -> Result<serde_json::Value, String> {
    let dir = workdir.trim();
    if dir.is_empty() {
        return Err("工作目录为空".into());
    }
    if !std::path::Path::new(dir).exists() {
        return Err(format!("工作目录不存在: {dir}"));
    }
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

/// 导入:按 git 地址加载到工作目录并拉取代码。
/// 目录空 → clone;已有 .git → pull;有文件无 .git → init + 远程 + pull。
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
