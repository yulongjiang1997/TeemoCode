//! 系统通知:子代理完成/中断时弹出 OS 原生通知。
//! 通过 tauri_plugin_notification 实现,跨平台(macOS/Windows/Linux)均原生支持。

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::util::LockExt;

/// 通知开关状态共享(线程安全)。
pub struct NotificationState {
    pub enabled: bool,
}

impl NotificationState {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

/// 显示系统通知(后台静默;权限未授予时不报错)。
pub fn show(app: &AppHandle, title: &str, body: &str) {
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}

/// 切换通知开关并持久化到 config.json。
#[tauri::command]
pub fn set_notification_enabled(enabled: bool, app: AppHandle) -> Result<(), String> {
    // 写 config:读取 → 改字段 → 写回
    let dir = crate::config::config_dir(&app)?;
    let path = dir.join("config.json");
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("无法读取配置: {e}"))?;
    let mut cfg: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("配置解析失败: {e}"))?;
    cfg["notification_enabled"] = serde_json::json!(enabled);
    // 原子写
    let tmp = dir.join(".config.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&cfg).map_err(|e| format!("配置序列化失败: {e}"))? + "\n")
        .map_err(|e| format!("无法写入配置: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("无法保存配置: {e}"))?;
    // 广播给 UI(其他窗口同步)
    let _ = app.emit("notification-enabled", enabled);
    Ok(())
}

/// 读取当前通知开关状态。
#[tauri::command]
pub fn get_notification_enabled(app: AppHandle) -> bool {
    app.state::<Arc<Mutex<NotificationState>>>().lock_ok().enabled
}

/// 注册 NotificationState 并返回初始化值(从 config 读取)。
pub fn init(app: &AppHandle) -> bool {
    let mut enabled = true;
    if let Ok(dir) = crate::config::config_dir(app) {
        if let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(v) = cfg.get("notification_enabled").and_then(|v| v.as_bool()) {
                    enabled = v;
                }
            }
        }
    }
    app.manage(Arc::new(Mutex::new(NotificationState::new(enabled))));
    enabled
}
