// 原版 MonkeyCode 本地资源一键导入:
// - 原版配置目录(默认 Roaming/com.chaitin.baizhi.monkeycode)里的引擎会话
//   (ohmyagent/sessions/<engine_id>/messages.jsonl)与本地数据目录
//   (Local/com.chaitin.baizhi.monkeycode)里的壳 sidecar(<sid>/meta.json
//   与帧)、chat-workspaces/<sid>/ 工作区,一起复制进本应用对应目录;
// - 壳 sessions_list 以 data_dir 扫描为权威索引,复制完成后前端重拉即见;
// - 支持按 sid 选择导入(前端按项目分组勾选)。
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

/// 扫描原版 MonkeyCode 的本地会话,供前端按项目勾选。
#[tauri::command]
pub async fn import_mc_scan(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let Some((_cfg, data)) = original_mc_dirs() else {
            return Ok(json!({ "found": false, "sessions": [] }));
        };
        let mut sessions: Vec<Value> = Vec::new();
        let entries = std::fs::read_dir(&data).map(|it| it.flatten().collect::<Vec<_>>()).unwrap_or_default();
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
        Ok(json!({ "found": true, "sessions": sessions }))
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

/// 把选中的 sid 从原版 MonkeyCode 复制进本应用(壳 sidecar + 工作区 + 引擎消息)。
/// 复制过程只增不删:已存在同名 sid 的会话文件会被覆盖为该次导入的内容。
#[tauri::command]
pub async fn import_mc_apply(app: AppHandle, sids: Vec<String>) -> Result<Value, String> {
    let teemo_cfg = config_dir(&app)?;
    let teemo_data = local_data_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let Some((orig_cfg, orig_data)) = original_mc_dirs() else {
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
            // 壳 sidecar(meta + 帧)
            copy_dir(&orig_sidecar, &teemo_data.join(sid)).map_err(|e| format!("复制会话 {sid} 失败: {e}"))?;
            // 工作区
            copy_dir(
                &orig_data.join("chat-workspaces").join(sid),
                &teemo_data.join("chat-workspaces").join(sid),
            )
            .ok();
            // 引擎消息(messages.jsonl):engine_id 取 sidecar meta,缺省同 sid
            let engine_id = {
                let meta_text = std::fs::read_to_string(orig_sidecar.join("meta.json")).unwrap_or_default();
                serde_json::from_str::<Value>(&meta_text)
                    .ok()
                    .and_then(|m| m.get("engine_id").and_then(|v| v.as_str()).map(String::from))
                    .filter(|e| valid_sid(e))
                    .unwrap_or_else(|| sid.clone())
            };
            copy_dir(
                &orig_cfg.join("ohmyagent").join("sessions").join(&engine_id),
                &teemo_cfg.join("ohmyagent").join("sessions").join(&engine_id),
            )
            .ok();
            imported += 1;
        }
        Ok(json!({ "imported": imported, "skipped": skipped }))
    })
    .await
    .map_err(|e| format!("导入任务失败: {e}"))?
}
