// 原版 MonkeyCode 本地资源一键导入:
// - 默认探测原版配置/数据目录(Roaming|Local/com.chaitin.baizhi.monkeycode);
//   未找到时前端可让用户手动选择目录(import_mc_scan_dir),导入同样带源目录;
// - 壳 sidecar(<sid>/meta.json 与帧)、chat-workspaces/<sid>/ 工作区、
//   引擎对话(ohmyagent/sessions/<engine_id>/messages.jsonl)一起复制;
// - 壳 sessions_list 以 data_dir 扫描为权威索引,复制完成后前端重拉即见。
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::config::{config_dir, local_data_dir};

/// 原版 MonkeyCode 的配置目录与本地数据目录(探测存在即返回)。
fn original_mc_dirs() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let roaming = std::env::var_os("APPDATA")?;
    let local = std::env::var_os("LOCALAPPDATA")?;
    let cfg = std::path::PathBuf::from(&roaming).join("com.chaitin.baizhi.monkeycode");
    let data = std::path::PathBuf::from(&local).join("com.chaitin.baizhi.monkeycode");
    if cfg.exists() || data.exists() {
        Some((cfg, data))
    } else {
        None
    }
}

fn valid_sid(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 扫描一个目录里的原版会话(<dir>/<sid>/meta.json 即视为一个会话)。
fn scan_sessions(data: &std::path::Path) -> Value {
    let mut sessions: Vec<Value> = Vec::new();
    let entries = std::fs::read_dir(data).map(|it| it.flatten().collect::<Vec<_>>()).unwrap_or_default();
    for entry in entries {
        let sid = entry.file_name().to_string_lossy().into_owned();
        if !valid_sid(&sid) || !entry.path().is_dir() {
            continue;
        }
        let meta_path = entry.path().join("meta.json");
        let Ok(meta_text) = std::fs::read_to_string(&meta_path) else { continue };
        let Ok(meta) = serde_json::from_str::<Value>(&meta_text) else { continue };
        let title = meta.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let workdir = meta.get("workdir").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let engine_id = meta
            .get("engine_id")
            .and_then(|v| v.as_str())
            .filter(|e| !e.is_empty())
            .unwrap_or(&sid)
            .to_string();
        let archived = meta.get("archived").and_then(|v| v.as_bool()).unwrap_or(false);
        sessions.push(json!({
            "sid": sid,
            "engine_id": engine_id,
            "title": title,
            "workdir": workdir,
            "archived": archived,
        }));
    }
    Value::Array(sessions)
}

/// 自动探测并扫描(标准安装路径)。
#[tauri::command]
pub async fn import_mc_scan(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let Some((_cfg, data)) = original_mc_dirs() else {
            return Ok(json!({ "found": false, "sessions": [], "candidates": default_locations() }));
        };
        Ok(json!({ "found": true, "sessions": scan_sessions(&data), "candidates": default_locations() }))
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
        Ok(json!({ "found": true, "sessions": scan_sessions(&dir), "source_dir": path }))
    })
    .await
    .map_err(|e| format!("扫描任务失败: {e}"))?
}

fn default_locations() -> Value {
    let mut list: Vec<Value> = Vec::new();
    if let Some(roaming) = std::env::var_os("APPDATA") {
        list.push(json!({ "kind": "config", "path": std::path::PathBuf::from(&roaming).join("com.chaitin.baizhi.monkeycode").display().to_string() }));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        list.push(json!({ "kind": "data", "path": std::path::PathBuf::from(&local).join("com.chaitin.baizhi.monkeycode").display().to_string() }));
    }
    Value::Array(list)
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

/// 把选中的 sid 复制进本应用。source_dir 为手动选择的目录时用它作为
/// 壳 sidecar/工作区来源;引擎消息优先看 source_dir/ohmyagent,回退标准配置目录。
#[tauri::command]
pub async fn import_mc_apply(app: AppHandle, sids: Vec<String>, source_dir: Option<String>) -> Result<Value, String> {
    let teemo_cfg = config_dir(&app)?;
    let teemo_data = local_data_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let orig_cfg = source_dir
            .as_ref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| original_mc_dirs().map(|(c, _)| c));
        let orig_data = source_dir
            .as_ref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| original_mc_dirs().map(|(_, d)| d));
        let (Some(orig_data), Some(orig_cfg)) = (orig_data, orig_cfg) else {
            return Err("未找到原版 MonkeyCode 的数据目录".into());
        };
        let mut imported = 0;
        let mut skipped = 0;
        for sid in &sids {
            if !valid_sid(sid) {
                continue;
            }
            let orig_sidecar = orig_data.join(sid);
            if !orig_sidecar.join("meta.json").exists() {
                skipped += 1;
                continue;
            }
            copy_dir(&orig_sidecar, &teemo_data.join(sid)).map_err(|e| format!("复制会话 {sid} 失败: {e}"))?;
            copy_dir(
                &orig_data.join("chat-workspaces").join(sid),
                &teemo_data.join("chat-workspaces").join(sid),
            )
            .ok();
            let engine_id = {
                let meta_text = std::fs::read_to_string(orig_sidecar.join("meta.json")).unwrap_or_default();
                serde_json::from_str::<Value>(&meta_text)
                    .ok()
                    .and_then(|m| m.get("engine_id").and_then(|v| v.as_str()).map(String::from))
                    .filter(|e| valid_sid(e))
                    .unwrap_or_else(|| sid.clone())
            };
            // 引擎消息:先试 source_dir/ohmyagent/sessions,再试标准配置目录
            let engine_src = orig_cfg.join("ohmyagent").join("sessions").join(&engine_id);
            copy_dir(&engine_src, &teemo_cfg.join("ohmyagent").join("sessions").join(&engine_id)).ok();
            imported += 1;
        }
        Ok(json!({ "imported": imported, "skipped": skipped }))
    })
    .await
    .map_err(|e| format!("导入任务失败: {e}"))?
}
