// 自动化(定时任务,对标 ZCode 的 Scheduled Automations):到点把提示词
// 发给 agent 的持久任务。调度线程在壳内(telemetry 同款常驻),触发直接
// 复用驱动层(session_create_with_kind → session_open → session_send),
// UI 不在线也能跑(帧落盘,打开即回放)。
//
// 计划语义:
//   - cron = 5 字段(分 时 日 月 周,本地时间),分钟粒度;同分钟去重靠
//     last_fire_ms(调度线程 20s 一扫,同一分钟内不会重复触发);
//   - once = 到 fire_at_ms 触发一次即消费(无论成败,失败原因记 last_result);
//   - 不跨进程补跑:应用没开就错过,一次性任务标"已过期跳过"。
//
// cron 匹配与 due 判定都是纯函数,单测表驱动。

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::gateway::new_hex;

pub const MAX_PROMPT_BYTES: usize = 64 * 1024;
pub const MAX_AUTOMATIONS: usize = 32;
/// 调度扫描周期(秒)。
pub const TICK_SECS: u64 = 20;

// ==================== 类型(config.json 权威的一部分) ====================

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Automation {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "crate::config::default_true")]
    pub enabled: bool,
    /// "once" | "cron"
    #[serde(default)]
    pub kind: String,
    /// 5 字段 cron(分 时 日 月 周),kind=cron 时生效。
    #[serde(default)]
    pub cron: String,
    /// 一次性任务的触发时刻(unix ms),kind=once 时生效。
    #[serde(default)]
    pub fire_at_ms: u64,
    /// 触发时发给 agent 的提示词(user-input)。
    #[serde(default)]
    pub prompt: String,
    /// "local"(workdir 生效) | "chat"(自动开新对话工作区)。
    #[serde(default = "default_session_kind")]
    pub kind_session: String,
    /// 本地项目工作目录;chat 形态忽略。
    #[serde(default)]
    pub workdir: String,
    /// 模型别名;空 = 会话默认模型。
    #[serde(default)]
    pub model: String,
    /// 上次触发时刻(unix ms;0 = 从未)。
    #[serde(default)]
    pub last_fire_ms: u64,
    /// 上次触发结果(ok / skipped:… / error:…),UI 列表外显。
    #[serde(default)]
    pub last_result: String,
}

fn default_session_kind() -> String {
    "chat".to_string()
}

impl Automation {
    /// 保存前归一 + 校验。Err = 人读错误;成功补 id、裁空白。
    pub fn normalized_for_save(mut self) -> Result<Self, String> {
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            return Err("名称不能为空".to_string());
        }
        if self.prompt.trim().is_empty() {
            return Err("提示词不能为空".to_string());
        }
        if self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(format!("提示词过大(上限 {}KB)", MAX_PROMPT_BYTES / 1024));
        }
        self.kind_session = match self.kind_session.as_str() {
            "local" => "local".to_string(),
            _ => "chat".to_string(),
        };
        if self.kind_session == "local" && self.workdir.trim().is_empty() {
            return Err("本地项目形态必须填写工作目录".to_string());
        }
        match self.kind.as_str() {
            "once" => {
                if self.fire_at_ms == 0 {
                    return Err("一次性任务缺少触发时间".to_string());
                }
            }
            "cron" => {
                self.cron = self.cron.trim().to_string();
                parse_cron(&self.cron)?;
            }
            _ => return Err(format!("不支持的计划类型: {}", self.kind)),
        }
        if self.id.is_empty() {
            self.id = format!("auto-{}", new_hex(6));
        }
        Ok(self)
    }
}

// ==================== cron 匹配器(5 字段,分钟粒度) ====================

/// 一分钟内的本地时间字段,来源由调用方决定(生产=now_local,测试=构造)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CronFields {
    pub minute: u32,  // 0-59
    pub hour: u32,    // 0-23
    pub day: u32,     // 1-31
    pub month: u32,   // 1-12
    /// 0-6 = 周日..周六(ZCode/标准 cron 语义;表达式里的 7 视作 0)
    pub weekday: u32,
}

/// 解析单个域:支持 `*`、`*/n`、`a-b`、`a,b,c` 与它们的组合(逗号分隔多项)。
fn parse_field(field: &str, min: u32, max: u32) -> Result<Vec<u32>, String> {
    let mut out = vec![];
    for part in field.split(',') {
        let part = part.trim();
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s.trim().parse().map_err(|_| format!("cron 步长非法: {s}"))?;
                if step == 0 {
                    return Err("cron 步长不能为 0".into());
                }
                (r, step)
            }
            None => (part, 1u32),
        };
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            let a: u32 = a.trim().parse().map_err(|_| format!("cron 数值非法: {a}"))?;
            let b: u32 = b.trim().parse().map_err(|_| format!("cron 数值非法: {b}"))?;
            (a, b)
        } else {
            let v: u32 = range.trim().parse().map_err(|_| format!("cron 数值非法: {range}"))?;
            (v, v)
        };
        if lo < min || hi > max || lo > hi {
            return Err(format!("cron 域超出范围({min}-{max}): {part}"));
        }
        let mut v = lo;
        while v <= hi {
            if !out.contains(&v) {
                out.push(v);
            }
            v += step;
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// 解析并校验 5 字段 cron 表达式;返回各域的允许值集合。
pub fn parse_cron(expr: &str) -> Result<[Vec<u32>; 5], String> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!("cron 需要 5 个字段(分 时 日 月 周),实际 {}: {expr}", fields.len()));
    }
    Ok([
        parse_field(fields[0], 0, 59)?,
        parse_field(fields[1], 0, 23)?,
        parse_field(fields[2], 1, 31)?,
        parse_field(fields[3], 1, 12)?,
        parse_field(fields[4], 0, 7)?,
    ])
}

/// 当前分钟是否命中。日(dom)与周(dow)同时受限时按标准 cron 语义取"或"
/// (两者都不是 `*` 时,命中其一即可);否则取"与"。
pub fn cron_matches(fields: &CronFields, expr: &str) -> Result<bool, String> {
    let parsed = parse_cron(expr)?;
    cron_matches_parsed(fields, &parsed)
}

pub fn cron_matches_parsed(fields: &CronFields, parsed: &[Vec<u32>; 5]) -> Result<bool, String> {
    let dow = if fields.weekday == 7 { 0 } else { fields.weekday };
    let day_hit = parsed[2].contains(&fields.day);
    let dow_hit = parsed[4].contains(&dow);
    let day_star = parsed[2].len() == 31;
    let dow_star = parsed[4].len() == 8;
    let date_hit = if day_star && dow_star {
        true
    } else if day_star {
        dow_hit
    } else if dow_star {
        day_hit
    } else {
        day_hit || dow_hit
    };
    Ok(parsed[0].contains(&fields.minute)
        && parsed[1].contains(&fields.hour)
        && parsed[3].contains(&fields.month)
        && date_hit)
}

// ==================== due 判定(纯函数) ====================

/// 调度判定:返回 Some(reason_label) = 应触发。label 用于 last_result 记账
/// ("cron"/"once")。now_ms 与 last_fire_ms 均为 unix 毫秒。
/// now_fields 是 now_ms 对应的本地时间字段(由调用方提供,便于单测)。
pub fn due(auto: &Automation, now_ms: u64, now_fields: &CronFields) -> Option<&'static str> {
    if !auto.enabled {
        return None;
    }
    match auto.kind.as_str() {
        "once" => {
            // 到点且从未触发过即消费(触发后 last_fire_ms 落值)
            (auto.fire_at_ms <= now_ms && auto.last_fire_ms == 0).then_some("once")
        }
        "cron" => {
            // 同分钟去重:last_fire_ms 落在当前分钟内则不再触发
            let same_minute = auto.last_fire_ms > 0 && (now_ms - auto.last_fire_ms) < 60_000 && now_ms >= auto.last_fire_ms;
            (!same_minute && cron_matches(now_fields, &auto.cron).unwrap_or(false)).then_some("cron")
        }
        _ => None,
    }
}

/// 一次性任务是否已错过(应用离线期间到点):仅用于 UI/list 的状态标注。
pub fn once_expired(auto: &Automation, now_ms: u64) -> bool {
    auto.kind == "once" && auto.enabled && auto.last_fire_ms == 0 && auto.fire_at_ms <= now_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(min: u32, hour: u32, day: u32, month: u32, weekday: u32) -> CronFields {
        CronFields { minute: min, hour, day, month, weekday }
    }

    #[test]
    fn cron_field_parsing() {
        let p = parse_cron("*/15 9-17 1,15 * 1-5").unwrap();
        assert_eq!(p[0], vec![0, 15, 30, 45]);
        assert_eq!(p[1], (9..=17).collect::<Vec<_>>());
        assert_eq!(p[2], vec![1, 15]);
        assert_eq!(p[3], (1..=12).collect::<Vec<_>>());
        assert_eq!(p[4], vec![1, 2, 3, 4, 5]);
        // 7 是合法值(与 0 都表示周日),归一到 0 发生在 cron_matches 里
        let p2 = parse_cron("0 12 * * 7").unwrap();
        assert!(p2[4].contains(&7));
        assert!(parse_cron("0 12 * *").is_err(), "字段数不足");
        assert!(parse_cron("60 * * * *").is_err(), "分钟越界");
        assert!(parse_cron("*/0 * * * *").is_err(), "步长 0");
        assert!(parse_cron("a * * * *").is_err(), "非数字");
    }

    #[test]
    fn cron_matches_basic() {
        // 每天 09:30
        assert!(cron_matches(&fields(30, 9, 12, 8, 3), "30 9 * * *").unwrap());
        assert!(!cron_matches(&fields(31, 9, 12, 8, 3), "30 9 * * *").unwrap());
        // 工作日(周一=1):2026-08-31 是周一
        assert!(cron_matches(&fields(0, 9, 31, 8, 1), "0 9 * * 1-5").unwrap());
        // 周六不命中
        assert!(!cron_matches(&fields(0, 9, 29, 8, 6), "0 9 * * 1-5").unwrap());
        // dom 与 dow 同时受限:标准 cron 取或
        assert!(cron_matches(&fields(0, 0, 1, 9, 1), "0 0 1 9 1").unwrap(), "2026-09-01 是周二,但 dom=1 命中");
        // 仅 dom 受限
        assert!(!cron_matches(&fields(0, 0, 2, 9, 2), "0 0 1 9 *").unwrap());
    }

    fn auto(kind: &str, cron: &str, fire_at: u64, last: u64, enabled: bool) -> Automation {
        Automation {
            id: "auto-1".into(),
            name: "n".into(),
            enabled,
            kind: kind.into(),
            cron: cron.into(),
            fire_at_ms: fire_at,
            prompt: "p".into(),
            kind_session: "chat".into(),
            workdir: "".into(),
            model: "".into(),
            last_fire_ms: last,
            last_result: "".into(),
        }
    }

    #[test]
    fn due_semantics() {
        let now = 1_800_000_000_000u64; // 某整分钟
        let f = fields(0, 12, 1, 1, 1);
        // cron:命中且未在本分钟触发过
        assert_eq!(due(&auto("cron", "0 12 * * *", 0, 0, true), now, &f), Some("cron"));
        // 同分钟已触发(30 秒前)→ 不重复
        assert_eq!(due(&auto("cron", "0 12 * * *", 0, now - 30_000, true), now, &f), None);
        // 禁用不触发
        assert_eq!(due(&auto("cron", "0 12 * * *", 0, 0, false), now, &f), None);
        // once:到点且未消费
        assert_eq!(due(&auto("once", "", now - 1_000, 0, true), now, &f), Some("once"));
        // once:已消费(last_fire 落值)
        assert_eq!(due(&auto("once", "", now - 1_000, now, true), now, &f), None);
        // once:未到点
        assert_eq!(due(&auto("once", "", now + 60_000, 0, true), now, &f), None);
        // once 过期判定(应用离线错过)
        assert!(once_expired(&auto("once", "", now - 60_000, 0, true), now));
        assert!(!once_expired(&auto("once", "", now - 60_000, now, true), now), "已触发不算过期");
    }

    #[test]
    fn normalized_save_validates() {
        let base = auto("cron", "", 0, 0, true);

        let mut a = base.clone();
        a.name = "每日检查".into();
        a.prompt = "跑一遍测试并汇报".into();
        a.cron = "0 9 * * *".into();
        assert!(a.normalized_for_save().is_ok());

        let mut a = base.clone();
        a.name = "  ".into();
        assert!(a.normalized_for_save().unwrap_err().contains("名称"));

        let mut a = base.clone();
        a.name = "x".into();
        a.prompt = "  ".into();
        assert!(a.normalized_for_save().unwrap_err().contains("提示词"));

        let mut a = base.clone();
        a.name = "x".into();
        a.prompt = "p".into();
        a.cron = "0 9 * *".into();
        assert!(a.normalized_for_save().unwrap_err().contains("cron"));

        let mut a = base.clone();
        a.name = "x".into();
        a.prompt = "p".into();
        a.kind = "once".into();
        a.fire_at_ms = 0;
        assert!(a.normalized_for_save().unwrap_err().contains("触发时间"));

        let mut a = base.clone();
        a.name = "x".into();
        a.prompt = "p".into();
        a.kind = "once".into();
        a.fire_at_ms = 1;
        a.kind_session = "local".into();
        a.workdir = " ".into();
        assert!(a.normalized_for_save().unwrap_err().contains("工作目录"));
    }
}

// ==================== 壳侧调度器与 IPC ====================

use tauri::{AppHandle, Manager as _};

use crate::config::{load_config, update_config_json};
use crate::driver::DriverHost;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 当前本地时间的 cron 字段。time 的 local-offset 在多线程 Unix 上可能
/// 失败(Windows 主平台无此问题);失败返回 None,本轮跳过。
fn now_fields() -> Option<super::automation::CronFields> {
    let now = time::OffsetDateTime::now_local().ok()?;
    Some(super::automation::CronFields {
        minute: u32::from(now.minute()),
        hour: u32::from(now.hour()),
        day: u32::from(now.day()),
        month: u32::from(u8::from(now.month())),
        weekday: u32::from(now.weekday().number_days_from_sunday()),
    })
}

/// 触发一次:建会话 → 打开 → 发提示词。返回会话 id。
/// (引擎不在位时 host.get() 报错,由调用方记 last_result。)
async fn fire_once(app: &AppHandle, auto: &Automation) -> Result<String, String> {
    let host = app.state::<DriverHost>();
    let engine = host.get()?;
    let created = engine
        .session_create_with_kind(&auto.workdir, &auto.model, true, &auto.kind_session, "")
        .await?;
    let sid = created
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "创建会话缺少 id".to_string())?
        .to_string();
    engine.session_open(&sid, None).await?;
    let content = base64::engine::general_purpose::STANDARD.encode(auto.prompt.as_bytes());
    engine
        .session_send(&sid, "user-input", serde_json::json!({ "content": content }))
        .await?;
    Ok(sid)
}

/// 触发并记账:last_fire_ms 落值消费(cron 同分钟去重/once 消费),
/// last_result 记录 ok / skipped(引擎不在位) / error。
fn fire_and_record(app: &AppHandle, auto: &Automation, now: u64) {
    let id = auto.id.clone();
    let result = match tauri::async_runtime::block_on(fire_once(app, auto)) {
        Ok(sid) => {
            eprintln!("[desktop] 自动化「{}」已触发 → 会话 {sid}", auto.name);
            format!("ok:{sid}")
        }
        Err(e) if e.contains("引擎") => format!("skipped:{e}"),
        Err(e) => format!("error:{}", e.chars().take(160).collect::<String>()),
    };
    let _ = update_config_json(app, |cfg| {
        if let Some(a) = cfg.automations.iter_mut().find(|a| a.id == id) {
            a.last_fire_ms = now;
            a.last_result = result;
        }
    });
}

fn tick(app: &AppHandle) {
    let now = now_ms();
    let Some(fields) = now_fields() else { return };
    let Ok(cfg) = load_config(app) else { return };
    for auto in &cfg.automations {
        let Some(label) = crate::automation::due(auto, now, &fields) else { continue };
        eprintln!("[desktop] 自动化「{}」到点({label}),开始触发", auto.name);
        fire_and_record(app, auto, now);
    }
}

/// 壳启动时挂载调度线程(20s 一扫;串行触发,长任务不会并发挤兑)。
pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(TICK_SECS));
        tick(&app);
    });
}

// ==================== IPC 命令 ====================

#[tauri::command]
pub fn automation_list(app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    let now = now_ms();
    let automations = load_config(&app)?.automations;
    // once_expired 标注进列表(UI 显示"已过期跳过"),其余字段原样透出
    Ok(automations
        .iter()
        .map(|a| {
            let mut v = serde_json::to_value(a).unwrap_or(serde_json::Value::Null);
            if once_expired(a, now) {
                v["expired"] = serde_json::json!(true);
            }
            v
        })
        .collect())
}

fn upsert(cfg: &mut crate::config::DesktopConfig, automation: Automation, err_slot: &mut Option<String>) -> Option<Automation> {
    let normalized = match automation.clone().normalized_for_save() {
        Ok(a) => a,
        Err(e) => {
            *err_slot = Some(e);
            return None;
        }
    };
    let saved = normalized.clone();
    match cfg.automations.iter().position(|a| a.id == normalized.id) {
        Some(i) => cfg.automations[i] = normalized,
        None => {
            if cfg.automations.len() >= MAX_AUTOMATIONS {
                *err_slot = Some(format!("自动化数量已达上限({MAX_AUTOMATIONS})"));
                return None;
            }
            cfg.automations.push(normalized);
        }
    }
    Some(saved)
}

/// 新建或更新自动化(id 空 = 新建)。返回归一化后的记录(含生成 id)。
#[tauri::command]
pub async fn automation_save(app: AppHandle, automation: Automation) -> Result<Automation, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut err: Option<String> = None;
        let mut saved: Option<Automation> = None;
        crate::config::update_config_json(&app, |cfg| {
            saved = upsert(cfg, automation.clone(), &mut err);
        })?;
        if let Some(e) = err {
            return Err(e);
        }
        saved.ok_or_else(|| "保存失败(内部错误)".to_string())
    })
    .await
    .map_err(|e| format!("保存失败: {e}"))?
}

#[tauri::command]
pub async fn automation_delete(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut err: Option<String> = None;
        crate::config::update_config_json(&app, |cfg| {
            let before = cfg.automations.len();
            cfg.automations.retain(|a| a.id != id);
            if cfg.automations.len() == before {
                err = Some(format!("自动化不存在: {id}"));
            }
        })?;
        if let Some(e) = err {
            return Err(e);
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("删除失败: {e}"))?
}

/// 立即运行一次(不走计划,用于测试;不改 last_fire_ms,只记 last_result)。
#[tauri::command]
pub async fn automation_run_now(app: AppHandle, id: String) -> Result<serde_json::Value, String> {
    let auto = {
        let cfg = load_config(&app)?;
        cfg.automations
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("自动化不存在: {id}"))?
    };
    let started = std::time::Instant::now();
    let (ok, detail) = match tauri::async_runtime::block_on(fire_once(&app, &auto)) {
        Ok(sid) => (true, sid),
        Err(e) => (false, e),
    };
    let detail_short: String = detail.chars().take(120).collect();
    let _ = update_config_json(&app, |cfg| {
        if let Some(a) = cfg.automations.iter_mut().find(|a| a.id == id) {
            a.last_fire_ms = now_ms();
            a.last_result = if ok { format!("manual:{detail_short}") } else { format!("error:{detail_short}") };
        }
    });
    Ok(serde_json::json!({ "ok": ok, "detail": detail_short, "latency_ms": started.elapsed().as_millis() as u64 }))
}
