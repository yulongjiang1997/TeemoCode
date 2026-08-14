// 原版 MonkeyCode 本地任务一键导入。
//
// 数据跨 3 个目录(都在配置目录下),sid 集合各不相同:
//   ohmy-sessions/<sid>/      壳侧会话(meta.json + replay.jsonl)
//   ohmyagent/sessions/<sid>/ 引擎对话(messages.jsonl)
//   ohmyagent/tasks/<sid>/    任务状态(tasks.json + notifications.json)
// 导入 = 选中 sid 的三处一并复制到本应用(com.teemocode.desktop)对应目录。
// 配置目录探测:Roaming/com.chaitin.baizhi.monkeycode;手动选目录时向上找
// 到含 ohmy-sessions/ohmyagent 的层作为根(选 ohmy-sessions、ohmyagent、
// ohmyagent/sessions 或配置目录本身都行)。
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::config::config_dir;

const ORIG_IDENT: &str = "com.chaitin.baizhi.monkeycode";

fn original_mc_dir() -> Option<std::path::PathBuf> {
    let roaming = std::env::var_os("APPDATA")?;
    let dir = std::path::PathBuf::from(&roaming).join(ORIG_IDENT);
    dir.exists().then_some(dir)
}

/// 向上找到含 ohmy-sessions 或 ohmyagent 的目录作为配置根。
fn find_config_root(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cur = Some(dir.to_path_buf());
    while let Some(d) = cur {
        if d.join("ohmy-sessions").is_dir() || d.join("ohmyagent").is_dir() {
            return Some(d);
        }
        cur = d.parent().map(std::path::Path::to_path_buf);
    }
    None
}

fn valid_sid(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 收集目录下的 sid 集合(目录名合法即算)。
fn sids_in(dir: &std::path::Path) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let sid = e.file_name().to_string_lossy().into_owned();
            if valid_sid(&sid) && e.path().is_dir() {
                out.insert(sid);
            }
        }
    }
    out
}

/// 扫描配置根下的全部会话(三个目录的并集;title/workdir 取 ohmy-sessions 的 meta)。
fn scan_all(root: &std::path::Path) -> Value {
    let mut ids = sids_in(&root.join("ohmy-sessions"));
    ids.extend(sids_in(&root.join("ohmyagent").join("sessions")));
    ids.extend(sids_in(&root.join("ohmyagent").join("tasks")));
    let mut sessions: Vec<Value> = Vec::new();
    for sid in ids {
        let mut title = String::new();
        let mut workdir = String::new();
        let mut archived = false;
        if let Ok(meta_text) = std::fs::read_to_string(root.join("ohmy-sessions").join(&sid).join("meta.json")) {
            if let Ok(meta) = serde_json::from_str::<Value>(&meta_text) {
                title = meta.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                workdir = meta.get("workdir").and_then(|v| v.as_str()).unwrap_or("").to_string();
                archived = meta.get("archived").and_then(|v| v.as_bool()).unwrap_or(false);
            }
        }
        sessions.push(json!({
            "sid": sid,
            "title": title,
            "workdir": workdir,
            "archived": archived,
        }));
    }
    Value::Array(sessions)
}

fn default_locations() -> Value {
    let mut list: Vec<Value> = Vec::new();
    if let Some(roaming) = std::env::var_os("APPDATA") {
        let cfg = std::path::PathBuf::from(&roaming).join(ORIG_IDENT);
        for sub in ["ohmy-sessions", "ohmyagent/sessions", "ohmyagent/tasks"] {
            list.push(json!({ "kind": "data", "path": cfg.join(sub).display().to_string() }));
        }
    }
    Value::Array(list)
}

/// 自动探测并扫描(标准安装路径)。
#[tauri::command]
pub async fn import_mc_scan(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let Some(root) = original_mc_dir().and_then(|d| find_config_root(&d)) else {
            return Ok(json!({ "found": false, "sessions": [], "candidates": default_locations() }));
        };
        Ok(json!({ "found": true, "sessions": scan_all(&root), "candidates": default_locations() }))
    })
    .await
    .map_err(|e| format!("扫描任务失败: {e}"))?
}

/// 用户手动指定的目录扫描(自动路径落空时的兜底)。
#[tauri::command]
pub async fn import_mc_scan_dir(app: AppHandle, path: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let dir = std::path::PathBuf::from(&path);
        if !dir.is_dir() {
            return Err(format!("目录不存在:{path}"));
        }
        let Some(root) = find_config_root(&dir) else {
            return Ok(json!({ "found": false, "sessions": [], "candidates": [] }));
        };
        Ok(json!({ "found": true, "sessions": scan_all(&root), "source_dir": root.display().to_string() }))
    })
    .await
    .map_err(|e| format!("扫描任务失败: {e}"))?
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<u64> {
    if !src.exists() {
        return Ok(0);
    }
    std::fs::create_dir_all(dst)?;
    let mut n = 0;
    for entry in std::fs::read_dir(src)?.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            n += copy_dir(&from, &to)?;
        } else if entry.file_type()?.is_file() {
            std::fs::create_dir_all(to.parent().unwrap_or(dst))?;
            std::fs::copy(&from, &to)?;
            n += 1;
        }
    }
    Ok(n)
}

/// 把选中的 sid 从原版配置目录复制进本应用,三处目录一并复制。
#[tauri::command]
pub async fn import_mc_apply(app: AppHandle, sids: Vec<String>, source_dir: Option<String>) -> Result<Value, String> {
    let teemo_cfg = config_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let root = source_dir
            .as_ref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
            .and_then(|d| find_config_root(&d))
            .or_else(|| original_mc_dir().and_then(|d| find_config_root(&d)));
        let Some(root) = root else {
            return Err("未找到原版 MonkeyCode 的配置目录".into());
        };
        let mut imported = 0;
        let mut skipped = 0;
        for sid in &sids {
            if !valid_sid(sid) {
                continue;
            }
            let src = root.join("ohmy-sessions").join(sid);
            if !src.join("meta.json").exists() && !root.join("ohmyagent").join("sessions").join(sid).exists() {
                skipped += 1;
                continue;
            }
            // 壳侧会话
            copy_dir(&src, &teemo_cfg.join("ohmy-sessions").join(sid)).map_err(|e| format!("复制会话 {sid} 失败: {e}"))?;
            // 引擎对话
            copy_dir(
                &root.join("ohmyagent").join("sessions").join(sid),
                &teemo_cfg.join("ohmyagent").join("sessions").join(sid),
            )
            .ok();
            // 任务状态
            copy_dir(
                &root.join("ohmyagent").join("tasks").join(sid),
                &teemo_cfg.join("ohmyagent").join("tasks").join(sid),
            )
            .ok();
            imported += 1;
        }
        Ok(json!({ "imported": imported, "skipped": skipped }))
    })
    .await
    .map_err(|e| format!("导入任务失败: {e}"))?
}
