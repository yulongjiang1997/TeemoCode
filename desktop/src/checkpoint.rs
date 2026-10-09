// 轮次 checkpoint:旁路 git 目录 `.ohmyagent/ckpt.git`,不写入用户提交历史。
// GIT_DIR 与用户 `.git` 分离;工作树仍是会话 workdir。WSL 走 RepoCtx 的 guest git。
// 隔离 worktree 也放在本模块:`.ohmyagent/worktrees/<name>`。

use serde_json::{json, Value};

use crate::repo::RepoCtx;

const GIT_DIR_FLAG: &str = "--git-dir=.ohmyagent/ckpt.git";
const WORK_TREE_FLAG: &str = "--work-tree=.";

fn ckpt_args<'a>(extra: &'a [&'a str]) -> Vec<&'a str> {
    let mut v = vec![GIT_DIR_FLAG, WORK_TREE_FLAG];
    v.extend_from_slice(extra);
    v
}

fn ensure_ckpt(ctx: &RepoCtx) -> Result<(), String> {
    let git_dir = ctx.resolve(".ohmyagent/ckpt.git")?;
    std::fs::create_dir_all(&git_dir).map_err(|e| format!("创建 checkpoint 目录失败: {e}"))?;
    let marker = ctx.git_cmd(&[GIT_DIR_FLAG, "rev-parse", "--git-dir"]);
    if marker.map(|s| s.trim().contains("ckpt.git")).unwrap_or(false) {
        return Ok(());
    }
    ctx.git_cmd(&[GIT_DIR_FLAG, "init"])?;
    let _ = ctx.git_cmd(&[GIT_DIR_FLAG, "config", "user.email", "teemo@local"]);
    let _ = ctx.git_cmd(&[GIT_DIR_FLAG, "config", "user.name", "TeemoCode"]);
    let exclude = [
        ".ohmyagent/",
        ".git/",
        "node_modules/",
        "target/",
        "dist/",
        ".venv/",
        "__pycache__/",
    ]
    .join("\n");
    let rel = ".ohmyagent/ckpt.git/info/exclude";
    let path = ctx.resolve(rel).unwrap_or_else(|_| {
        std::path::Path::new(&ctx.workdir).join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, exclude);
    Ok(())
}

/// 拍一张工作区快照,返回 commit hash(短)。空仓库允许 --allow-empty。
pub fn save(ctx: &RepoCtx, message: &str) -> Result<String, String> {
    ensure_ckpt(ctx)?;
    let add = ckpt_args(&["add", "-A"]);
    ctx.git_cmd(&add)?;
    let msg = if message.trim().is_empty() { "checkpoint" } else { message };
    let commit_args = ["-c", "user.email=teemo@local", "-c", "user.name=TeemoCode", "commit", "-m", msg, "--allow-empty"];
    let commit = ckpt_args(&commit_args);
    let _ = ctx.git_cmd(&commit);
    let hash = ctx.git_cmd(&ckpt_args(&["rev-parse", "--short", "HEAD"]))?;
    Ok(hash.trim().to_string())
}

/// 列出最近快照(最多 40)。
pub fn list(ctx: &RepoCtx) -> Result<Value, String> {
    if ctx.git_cmd(&[GIT_DIR_FLAG, "rev-parse", "--git-dir"]).is_err() {
        return Ok(json!([]));
    }
    let log = ctx.git_cmd(&ckpt_args(&[
        "log",
        "-n",
        "40",
        "--format=%h%x09%s%x09%ct",
    ]))?;
    let mut out = Vec::new();
    for line in log.lines() {
        let mut parts = line.splitn(3, '\t');
        let hash = parts.next().unwrap_or("").trim();
        let subject = parts.next().unwrap_or("").trim();
        let ts = parts.next().unwrap_or("").trim();
        if hash.is_empty() {
            continue;
        }
        out.push(json!({ "hash": hash, "message": subject, "ts": ts }));
    }
    Ok(json!(out))
}

/// 整树恢复到某次快照(空 hash = HEAD)。不碰用户 `.git`。
pub fn restore(ctx: &RepoCtx, commit: &str) -> Result<(), String> {
    ensure_ckpt(ctx)?;
    let target = if commit.trim().is_empty() { "HEAD" } else { commit.trim() };
    ctx.git_cmd(&ckpt_args(&["checkout", "-f", target]))?;
    let _ = ctx.git_cmd(&ckpt_args(&["clean", "-fd", "-e", ".ohmyagent"]));
    Ok(())
}

/// 单文件恢复到最新快照。快照里没有该文件则删除工作区文件。
pub fn restore_file(ctx: &RepoCtx, rel: &str) -> Result<(), String> {
    if rel.trim().is_empty() {
        return Err("非法路径".into());
    }
    let path = ctx.resolve(rel)?;
    ensure_ckpt(ctx)?;
    if ctx.git_cmd(&ckpt_args(&["rev-parse", "--verify", "HEAD"])).is_err() {
        return Err("暂无检查点".into());
    }
    match ctx.git_cmd(&ckpt_args(&["checkout", "-f", "HEAD", "--", rel])) {
        Ok(_) => Ok(()),
        Err(_) => {
            if path.is_file() {
                std::fs::remove_file(&path).map_err(|e| format!("删除文件失败: {e}"))?;
            }
            Ok(())
        }
    }
}

/// 在仓库内建隔离 worktree,返回新 workdir(与 ctx.workdir 同风格的路径)。
pub fn add_worktree(ctx: &RepoCtx, name: &str) -> Result<String, String> {
    if !ctx.is_git() {
        return Err("隔离工作区需要 git 仓库".into());
    }
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("非法隔离名".into());
    }
    let rel = format!(".ohmyagent/worktrees/{name}");
    let branch = format!("teemo/{name}");
    let _ = ctx.git_cmd(&["worktree", "prune"]);
    match ctx.git_cmd(&["worktree", "add", "-b", &branch, &rel]) {
        Ok(_) => {}
        Err(e) => {
            if e.to_lowercase().contains("already exists") || e.contains("已经") {
                ctx.git_cmd(&["worktree", "add", &rel, &branch]).map_err(|e2| format!("{e}; {e2}"))?;
            } else {
                return Err(e);
            }
        }
    }
    let base = ctx.workdir.replace('\\', "/").trim_end_matches('/').to_string();
    Ok(format!("{base}/.ohmyagent/worktrees/{name}"))
}

/// 从主仓拆除隔离 worktree。ctx.workdir 必须是主仓。
pub fn remove_worktree(ctx: &RepoCtx, path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Ok(());
    }
    let _ = ctx.git_cmd(&["worktree", "remove", "--force", path]);
    let _ = ctx.git_cmd(&["worktree", "prune"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_git(label: &str) -> (String, RepoCtx) {
        let dir = std::env::temp_dir().join(format!(
            "mc-ckpt-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let workdir = dir.to_string_lossy().into_owned();
        let ctx = RepoCtx { workdir: workdir.clone(), wsl_distro: None };
        ctx.git_cmd(&["init"]).unwrap();
        ctx.git_cmd(&["config", "user.email", "t@t.t"]).unwrap();
        ctx.git_cmd(&["config", "user.name", "t"]).unwrap();
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        ctx.git_cmd(&["add", "a.txt"]).unwrap();
        ctx.git_cmd(&["commit", "-m", "init"]).unwrap();
        (workdir, ctx)
    }

    #[test]
    fn checkpoint_save_restore_does_not_touch_user_log() {
        let (dir, ctx) = tmp_git("roundtrip");
        let before = ctx.git_cmd(&["rev-parse", "HEAD"]).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("a.txt"), "two").unwrap();
        let hash = save(&ctx, "turn-1").unwrap();
        assert!(!hash.is_empty());
        std::fs::write(std::path::Path::new(&dir).join("a.txt"), "three").unwrap();
        restore(&ctx, "").unwrap();
        let text = std::fs::read_to_string(std::path::Path::new(&dir).join("a.txt")).unwrap();
        assert_eq!(text, "two");
        let after = ctx.git_cmd(&["rev-parse", "HEAD"]).unwrap();
        assert_eq!(before.trim(), after.trim());
        let log = ctx.git_cmd(&["log", "--oneline"]).unwrap();
        assert!(!log.contains("turn-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_file_reverts_one_path() {
        let (dir, ctx) = tmp_git("file");
        save(&ctx, "base").unwrap();
        std::fs::write(std::path::Path::new(&dir).join("a.txt"), "changed").unwrap();
        std::fs::write(std::path::Path::new(&dir).join("b.txt"), "new").unwrap();
        restore_file(&ctx, "a.txt").unwrap();
        assert_eq!(std::fs::read_to_string(std::path::Path::new(&dir).join("a.txt")).unwrap(), "one");
        assert!(std::path::Path::new(&dir).join("b.txt").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_file_without_checkpoint_does_not_delete() {
        let (dir, ctx) = tmp_git("nocheckpoint");
        let target = std::path::Path::new(&dir).join("keep.txt");
        std::fs::write(&target, "keep").unwrap();
        let err = restore_file(&ctx, "keep.txt").unwrap_err();
        assert!(err.contains("暂无检查点"), "{err}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
