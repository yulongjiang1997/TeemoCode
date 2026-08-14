// 原版 MonkeyCode 本地资源一键导入:
// - 原版会话在配置目录 Roaming/com.chaitin.baizhi.monkeycode/ohmy-sessions/<sid>/
//   (meta.json + replay.jsonl);本应用(com.teemocode.desktop)同结构;
// - 默认探测该标准位置;未找到时前端可手动选目录(import_mc_scan_dir),支持
//   直接选 ohmy-sessions 目录或选上层配置目录(自动补 /ohmy-sessions);
// - 复制 = 拷贝每个会话目录(meta + replay)到本应用 ohmy-sessions;
//   工作目录是外部路径(meta.workdir),不随会话复制;
// - 壳 sessions_list 以 ohmy-sessions 扫描为权威索引,复制后前端重拉即见。
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::config::config_dir;

const ORIG_IDENT: &str = "com.chaitin.baizhi.monkeycode";

fn original_mc_dir() -> Option<std::path::PathBuf> {
    let roaming = std::env::var_os("APPDATA")?;
    let dir = std::path::PathBuf::from(&roaming).join(ORIG_IDENT);
    dir.exists().then_some(dir)
}

fn valid_sid(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 会话根目录:手动选中的目录可能是 ohmy-sessions 本身,也可能是它的上层
/// (配置目录),统一归一到含 <sid>/meta.json 的那一层。
fn sessions_root(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    if dir.join("ohmy-sessions").join("meta.json").is_file() {
        return Some(dir.join("ohmy-sessions"));
    }
    // ohmy-sessions 下有 <sid>/meta.json
    if dir.join("ohmy-sessions").is_dir()
        && std::fs::read_dir(dir.join("ohmy-sessions"))
            .ok()
            .is_some_and(|it| it.flatten().any(|e| e.path().join("meta.json").is_file()))
    {
        return Some(dir.join("ohmy-sessions"));
    }
    // 直接就是会话目录层(<sid>/meta.json)
    if std::fs::read_dir(dir)
        .ok()
        .is_some_and(|it| it.flatten().any(|e| e.path().join("meta.json").is_file()))
    {
        return Some(dir.to_path_buf());
    }
    None
}

fn scan_sessions(root: &std::path::Path) -> Value {
    let mut sessions: Vec<Value> = Vec::new();
    let entries = std::fs::read_dir(root).map(|it| it.flatten().collect::<Vec<_>>()).unwrap_or_default();
    for entry in entries {
        let sid = entry.file_name().to_string_lossy().into_owned();
        if !valid_sid(&sid) || !entry.path().is_dir() {
            continue;
        }
        let meta_path = entry.path().join("meta.json");
        let Ok(meta_text) = std::fs::read_to_string(&meta_path) else { continue };
        let Ok(meta) = serde_json::from_str::<Value>(&meta_text) else { continue };
        sessions.push(json!({
            "sid": sid,
            "title": meta.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            "workdir": meta.get("workdir").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            "archived": meta.get("archived").and_then(|v| v.as_bool()).unwrap_or(false),
        }));
    }
    Value::Array(sessions)
}

fn default_locations() -> Value {
    let mut list: Vec<Value> = Vec::new();
    if let Some(roaming) = std::env::var_os("APPDATA") {
        let cfg = std::path::PathBuf::from(&roaming).join(ORIG_IDENT);
        list.push(json!({ "kind": "sessions", "path": cfg.join("ohmy-sessions").display().to_string() }));
        list.push(json!({ "kind": "config", "path": cfg.display().to_string() }));
    }
    Value::Array(list)
}

/// 自动探测并扫描(标准安装路径)。
#[tauri::command]
pub async fn import_mc_scan(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let Some(orig) = original_mc_dir() else {
            return Ok(json!({ "found": false, "sessions": [], "candidates": default_locations() }));
        };
        let Some(root) = sessions_root(&orig) else {
            return Ok(json!({ "found": false, "sessions": [], "candidates": default_locations() }));
        };
        Ok(json!({ "found": true, "sessions": scan_sessions(&root), "candidates": default_locations() }))
    })
    .await
    .map_err(|e| format!("扫描任务失败: {e}"))?
}

/// 用户手动指定的目录扫描(找不到自动路径时的兜底)。
#[tauri::command]
pub async fn import_mc_scan_dir(app: AppHandle, path: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let dir = std::path::PathBuf::from(&path);
        if !dir.is_dir() {
            return Err(format!("目录不存在:{path}"));
        }
        let Some(root) = sessions_root(&dir) else {
            return Ok(json!({ "found": false, "sessions": [], "candidates": [] }));
        };
        Ok(json!({ "found": true, "sessions": scan_sessions(&root), "source_dir": root.display().to_string() }))
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

/// 把选中的 sid 复制进本应用 ohmy-sessions。source_dir 为手动选择的目录
/// (可能是 ohmy-sessions 或其上层)。
#[tauri::command]
pub async fn import_mc_apply(app: AppHandle, sids: Vec<String>, source_dir: Option<String>) -> Result<Value, String> {
    let teemo_root = config_dir(&app)?.join("ohmy-sessions");
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let source = source_dir
            .as_ref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
            .and_then(|d| sessions_root(&d))
            .or_else(|| original_mc_dir().and_then(|d| sessions_root(&d)));
        let Some(root) = source else {
            return Err("未找到原版 MonkeyCode 的会话目录".into());
        };
        let mut imported = 0;
        let mut skipped = 0;
        for sid in &sids {
            if !valid_sid(sid) {
                continue;
            }
            let src = root.join(sid);
            if !src.join("meta.json").exists() {
                skipped += 1;
                continue;
            }
            copy_dir(&src, &teemo_root.join(sid)).map_err(|e| format!("复制会话 {sid} 失败: {e}"))?;
            imported += 1;
        }
        Ok(json!({ "imported": imported, "skipped": skipped }))
    })
    .await
    .map_err(|e| format!("导入任务失败: {e}"))?
}
