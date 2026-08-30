// 模型网关:壳内置的统一大模型调度平台(OpenAI Chat Completions 兼容)。
//
// 与 browser/(扩展桥)同级的壳原生服务,与引擎零耦合:
//   - 外部调用方把 base_url 指到 http://127.0.0.1:{port}/v1,以**组 Key**
//     作 Bearer——组即调度与鉴权的统一边界;
//   - sched.rs 在组内按权重顺序调度并做故障切换/熔断;
//   - upstream.rs 把组内任意协议(openai/anthropic/openai_responses)的
//     模型翻译成对外统一的 OpenAI 协议;
//   - server.rs 手写最小 HTTP(browser/mcp.rs 同款风格,不引 HTTP 框架)。
//
// 配置权威是 config.json 的 DesktopConfig.gateway:组的增删改走 gateway_*
// 独立命令(update_config_json 事务),**不进设置页表单**;merge_shell_prefs
// 以磁盘值保全该字段。运行期请求只用内存快照(RuntimeSnapshot,含引用条目
// 的连接信息解析),不碰磁盘。
//
// 并发模型:监听线程 + 每连接一线程(server.rs);快照/健康/日志各持 StdMutex
// 短临界区;上游调用经 tauri::async_runtime::block_on(reqwest stream)。

pub mod sched;
pub mod server;
pub mod upstream;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::util::LockExt;

pub const DEFAULT_PORT: u16 = 8317;
pub const DEFAULT_CONTEXT_WINDOW: i64 = 128_000;
pub const DEFAULT_MAX_OUTPUT: i64 = 32_768;
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
/// 单组模型条目上限(权重顺序列表,再多调度意义与可读性都崩)。
pub const MAX_GROUP_MODELS: usize = 16;
/// 模型组数量上限。
pub const MAX_GROUPS: usize = 32;
pub const MAX_NAME_LEN: usize = 64;
/// 调度策略词汇(对外契约,UI 下拉与持久化同源)。
pub const STRATEGY_PRIORITY: &str = "priority";
pub const STRATEGY_WEIGHTED: &str = "weighted";

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn default_true() -> bool {
    true
}
fn default_port() -> u16 {
    DEFAULT_PORT
}
fn default_context_window() -> i64 {
    DEFAULT_CONTEXT_WINDOW
}
fn default_max_output() -> i64 {
    DEFAULT_MAX_OUTPUT
}
fn default_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}
fn default_weight() -> u32 {
    1
}

// ==================== 配置类型(config.json 权威的一部分) ====================

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GatewaySettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub groups: Vec<ModelGroup>,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self { enabled: false, port: default_port(), groups: vec![] }
    }
}

/// 模型组:对外暴露一个 OpenAI 兼容模型条目(id=组名),组内模型按权重调度。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelGroup {
    /// "mg-<hex>";新建时为空,壳生成。
    #[serde(default)]
    pub id: String,
    /// 展示名,同时是对外 /v1/models 的模型 id。
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 组 Key("tgk-<32hex>"),对外 Bearer;新建为空时壳生成,可重置。
    #[serde(default)]
    pub key: String,
    /// priority(顺序优先,权重高者恒先) | weighted(健康模型间加权随机)。
    #[serde(default)]
    pub strategy: String,
    /// 组级共享上下文:全组模型共用, /v1/models 外显 context_length。
    #[serde(default = "default_context_window")]
    pub context_window: i64,
    /// 组级共享最大输出;钳制请求 max_tokens,anthropic 缺省时的必填值。
    #[serde(default = "default_max_output")]
    pub max_output: i64,
    /// 组级默认温度;请求未指定时套用。
    #[serde(default)]
    pub temperature: Option<f64>,
    /// 组级系统提示词;非空时前置到每个请求。
    #[serde(default)]
    pub system_prompt: String,
    /// 单次上游尝试的超时(秒),含连接与(非流式)完整应答。
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub models: Vec<GroupModel>,
}

/// 组内模型条目。alias 非空 = 引用桌面端模型库(config.models,name 匹配,
/// 会员条目凭据注入同引擎物化口径);alias 空 = 独立自定义条目,四连接字段生效。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupModel {
    /// "gm-<hex>";新建时为空,壳生成。
    #[serde(default)]
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 权重 1..=100:priority 下大者先行;weighted 下为分流比例。
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// 引用的模型库名(config.models.name);空 = 自定义条目。
    #[serde(default)]
    pub alias: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
}

pub(crate) fn new_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("系统随机源不可用");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn new_group_id() -> String {
    format!("mg-{}", new_hex(6))
}

pub(crate) fn new_model_id() -> String {
    format!("gm-{}", new_hex(6))
}

pub(crate) fn new_group_key() -> String {
    format!("tgk-{}", new_hex(16))
}

impl GroupModel {
    /// 保存前的字段归一:补 id、钳权重、引用条目丢弃手填连接字段(以模型库
    /// 为准,防两处真值)、自定义条目裁空白。返回 Err = 字段不合法。
    pub fn normalized(mut self) -> Result<Self, String> {
        if self.id.is_empty() {
            self.id = new_model_id();
        }
        self.weight = self.weight.clamp(1, 100);
        self.alias = self.alias.trim().to_string();
        if self.alias.is_empty() {
            self.provider = match self.provider.trim() {
                "anthropic" | "openai_responses" => self.provider.trim().to_string(),
                _ => "openai".to_string(),
            };
            self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
            self.api_key = self.api_key.trim().to_string();
            self.model = self.model.trim().to_string();
            if self.base_url.is_empty() {
                return Err(format!("模型「{}」缺少接口地址", self.model));
            }
            if self.model.is_empty() {
                return Err("自定义模型缺少模型标识".to_string());
            }
        }
        Ok(self)
    }
}

impl ModelGroup {
    pub fn effective_strategy(&self) -> &'static str {
        if self.strategy == STRATEGY_WEIGHTED {
            STRATEGY_WEIGHTED
        } else {
            STRATEGY_PRIORITY
        }
    }

    pub fn effective_context_window(&self) -> i64 {
        if self.context_window > 0 {
            self.context_window
        } else {
            DEFAULT_CONTEXT_WINDOW
        }
    }

    pub fn effective_max_output(&self) -> i64 {
        if self.max_output > 0 {
            self.max_output
        } else {
            DEFAULT_MAX_OUTPUT
        }
    }

    pub fn effective_timeout(&self) -> std::time::Duration {
        let secs = if self.timeout_seconds > 0 { self.timeout_seconds } else { DEFAULT_TIMEOUT_SECONDS };
        std::time::Duration::from_secs(secs)
    }

    /// 保存前归一 + 跨组校验。other_names 是除本组外现存组的名字集合,
    /// 用于重名判定。
    pub fn normalized_for_save(mut self, other_names: &[String]) -> Result<Self, String> {
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            return Err("模型组名称不能为空".to_string());
        }
        if self.name.len() > MAX_NAME_LEN {
            return Err(format!("模型组名称过长(≤{MAX_NAME_LEN} 字符)"));
        }
        if other_names.iter().any(|n| n == &self.name) {
            return Err(format!("模型组名称已存在: {}", self.name));
        }
        if self.id.is_empty() {
            self.id = new_group_id();
        }
        if self.key.is_empty() {
            self.key = new_group_key();
        }
        if self.models.len() > MAX_GROUP_MODELS {
            return Err(format!("模型组内模型过多(≤{MAX_GROUP_MODELS})"));
        }
        let mut models = Vec::with_capacity(self.models.len());
        for m in self.models {
            models.push(m.normalized()?);
        }
        self.models = models;
        Ok(self)
    }
}

// ==================== 运行时快照(请求期只读) ====================

/// 解析后的组内候选。unavailable = 引用条目解析失败(模型库无此名/缺凭据),
/// 调度时恒失败并带原因,不发起网络请求。
#[derive(Clone, Debug)]
pub struct ResolvedCandidate {
    pub id: String,
    pub label: String,
    pub weight: u32,
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub unavailable: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RuntimeGroup {
    pub group: ModelGroup,
    pub candidates: Vec<ResolvedCandidate>,
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeSnapshot {
    pub settings: GatewaySettings,
    pub groups: Vec<RuntimeGroup>,
}

impl RuntimeSnapshot {
    pub fn group_by_key(&self, key: &str) -> Option<&RuntimeGroup> {
        self.groups.iter().find(|g| g.group.enabled && server::ct_eq(key.as_bytes(), g.group.key.as_bytes()))
    }

    pub fn group_by_id(&self, id: &str) -> Option<&RuntimeGroup> {
        self.groups.iter().find(|g| g.group.id == id)
    }
}

/// 从权威配置构建快照。引用条目在此解析成连接信息(含会员凭据注入),
/// 请求路径不再触碰磁盘与配置结构。
pub(crate) fn build_snapshot(
    cfg: &crate::config::DesktopConfig,
    cfg_dir: &std::path::Path,
) -> RuntimeSnapshot {
    let settings = &cfg.gateway;
    let models = &cfg.models;
    let model_index: HashMap<String, &serde_json::Value> = models
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|v| v.as_str()).map(|n| (n.to_string(), m)))
                .collect()
        })
        .unwrap_or_default();
    let mut groups = Vec::new();
    for group in &settings.groups {
        let mut candidates = Vec::new();
        for m in &group.models {
            if !m.enabled {
                continue;
            }
            let (label, provider, base_url, api_key, model, unavailable) = if m.alias.is_empty() {
                (
                    if m.model.is_empty() { "自定义".to_string() } else { m.model.clone() },
                    m.provider.clone(),
                    m.base_url.clone(),
                    m.api_key.clone(),
                    m.model.clone(),
                    None,
                )
            } else {
                match model_index.get(m.alias.as_str()) {
                    None => (
                        m.alias.clone(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        Some(format!("模型库中不存在「{}」(可能已删除或改名)", m.alias)),
                    ),
                    Some(entry) => {
                        let get = |k: &str| entry.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let provider = get("provider");
                        let model = get("model");
                        let is_monkeycode =
                            entry.get("source").and_then(|v| v.as_str()) == Some(crate::baizhi::monkeycode::SOURCE_MONKEYCODE);
                        let (base_url, api_key) = if is_monkeycode {
                            (
                                crate::config::resolve_monkeycode_base_url(cfg),
                                crate::config::resolve_monkeycode_api_key(cfg_dir, cfg),
                            )
                        } else {
                            (get("base_url"), get("api_key"))
                        };
                        let unavailable =
                            if model.is_empty() { Some(format!("模型库条目「{}」缺少模型标识", m.alias)) } else { None };
                        (m.alias.clone(), provider, base_url, api_key, model, unavailable)
                    }
                }
            };
            candidates.push(ResolvedCandidate {
                id: m.id.clone(),
                label,
                weight: m.weight,
                provider,
                base_url,
                api_key,
                model,
                unavailable,
            });
        }
        groups.push(RuntimeGroup { group: group.clone(), candidates });
    }
    RuntimeSnapshot { settings: settings.clone(), groups }
}

// ==================== 日志与计数(会话期内存) ====================

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    pub ts_ms: u64,
    pub group_id: String,
    pub group_name: String,
    pub stream: bool,
    pub ok: bool,
    pub status: Option<u16>,
    pub latency_ms: u64,
    /// 最终应答(或最后尝试)的模型展示名。
    pub model: String,
    pub attempts: u32,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    /// 失败原因/尝试摘要(截断)。
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct GroupCounter {
    pub total: u64,
    pub ok: u64,
    pub fail: u64,
    /// 触发的故障切换次数(尝试数-1 的累计)。
    pub failovers: u64,
}

const LOG_CAP: usize = 100;

// ==================== 壳侧运行时(managed state) ====================

#[derive(Clone)]
pub struct GatewayHost(pub(crate) Arc<GatewayInner>);

pub(crate) struct GatewayInner {
    snapshot: StdMutex<Arc<RuntimeSnapshot>>,
    /// key = "<group_id>/<model_id>"。
    health: StdMutex<HashMap<String, sched::ModelHealth>>,
    log: StdMutex<VecDeque<LogEntry>>,
    counters: StdMutex<HashMap<String, GroupCounter>>,
    server: StdMutex<Option<server::ServerHandle>>,
    /// 服务侧错误(端口被占等),UI 外显;服务正常时为 None。
    server_error: StdMutex<Option<String>>,
    client: reqwest::Client,
    /// reload 串行闸(保存/重启并发时避免两个线程同时折腾监听线程)。
    reload_gate: StdMutex<()>,
}

impl GatewayHost {
    pub(crate) fn new() -> Self {
        let client = reqwest::Client::builder()
            // **不设**总超时(reqwest 的 timeout(0) 是"立即超时"不是"不限时",
            // 且 30s 默认会掐断长流式生成):请求级超时由网关按组配置自己
            // 施加(tokio::time::timeout),流式另有逐块 idle 守护。
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self(Arc::new(GatewayInner {
            snapshot: StdMutex::new(Arc::new(RuntimeSnapshot::default())),
            health: StdMutex::new(HashMap::new()),
            log: StdMutex::new(VecDeque::new()),
            counters: StdMutex::new(HashMap::new()),
            server: StdMutex::new(None),
            server_error: StdMutex::new(None),
            client,
            reload_gate: StdMutex::new(()),
        }))
    }

    pub(crate) fn snapshot(&self) -> Arc<RuntimeSnapshot> {
        self.0.snapshot.lock_ok().clone()
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.0.client
    }

    pub(crate) fn health_map(&self) -> std::sync::MutexGuard<'_, HashMap<String, sched::ModelHealth>> {
        self.0.health.lock_ok()
    }

    /// 单次尝试结果落健康簿。成功不新建记录(无历史的模型成功后仍无记录,
    /// 避免健康簿被"全绿条目"灌满);失败才落键。
    pub(crate) fn record_attempt(&self, group_id: &str, model_id: &str, ok: bool) {
        let key = format!("{group_id}/{model_id}");
        let now = now_ms();
        let mut health = self.0.health.lock_ok();
        match health.get_mut(&key) {
            Some(entry) => {
                if ok {
                    entry.record_success();
                } else {
                    entry.record_failure(now);
                }
            }
            None => {
                if !ok {
                    let mut h = sched::ModelHealth::default();
                    h.record_failure(now);
                    health.insert(key, h);
                }
            }
        }
    }

    pub(crate) fn push_log(&self, entry: LogEntry) {
        let group_id = entry.group_id.clone();
        let ok = entry.ok;
        let failovers = entry.attempts.saturating_sub(1) as u64;
        {
            let mut log = self.0.log.lock_ok();
            log.push_back(entry);
            while log.len() > LOG_CAP {
                log.pop_front();
            }
        }
        let mut counters = self.0.counters.lock_ok();
        let c = counters.entry(group_id).or_default();
        c.total += 1;
        if ok {
            c.ok += 1;
        } else {
            c.fail += 1;
        }
        c.failovers += failovers;
    }

    /// 调度用的随机数(进程级 xorshift 状态)。
    pub(crate) fn rng_next(&self) -> u64 {
        static SEED: std::sync::OnceLock<StdMutex<u64>> = std::sync::OnceLock::new();
        let cell = SEED.get_or_init(|| {
            let mut buf = [0u8; 8];
            getrandom::getrandom(&mut buf).ok();
            let mut seed = u64::from_le_bytes(buf);
            if seed == 0 {
                seed = 0x9E37_79B9_7F4A_7C15;
            }
            StdMutex::new(seed)
        });
        let mut state = cell.lock_ok();
        sched::next_u64(&mut state)
    }
}

/// 壳启动时挂载 managed state 并按配置起服务。挂在配置加载之后。
pub fn manage(app: &AppHandle) {
    app.manage(GatewayHost::new());
    reload(app);
}

/// 从盘上权威配置重建快照并按需启停监听。所有 gateway_* 变更命令与
/// save_config 之后都必须调用,保证请求期看到的就是盘上的配置。
pub fn reload(app: &AppHandle) {
    let host: GatewayHost = app.state::<GatewayHost>().inner().clone();
    let _gate = host.0.reload_gate.lock_ok();
    let cfg = match crate::config::load_config(app) {
        Ok(cfg) => cfg,
        Err(e) => {
            // 配置损坏已在别处外显;网关保持当前快照不动,仅记错误
            *host.0.server_error.lock_ok() = Some(format!("配置读取失败: {e}"));
            return;
        }
    };
    let snapshot = match crate::config::config_dir(app) {
        Ok(dir) => build_snapshot(&cfg, &dir),
        Err(_) => RuntimeSnapshot { settings: cfg.gateway.clone(), groups: vec![] },
    };
    let enabled = snapshot.settings.enabled;
    let port = snapshot.settings.port;

    let mut server_slot = host.0.server.lock_ok();
    let current = server_slot.take();
    let keep: Option<server::ServerHandle> = match &current {
        Some(h) if enabled && h.port == port && h.is_running() => Some(h.clone()),
        Some(h) => {
            h.stop();
            None
        }
        None => None,
    };
    *host.0.server_error.lock_ok() = None;
    let handle = match (enabled, keep) {
        (true, Some(h)) => Some(h),
        (true, None) => match server::start(host.clone(), port) {
            Ok(h) => Some(h),
            Err(e) => {
                eprintln!("[desktop] 模型网关启动失败: {e}");
                *host.0.server_error.lock_ok() = Some(e);
                None
            }
        },
        (false, _) => None,
    };
    *server_slot = handle;
    *host.0.snapshot.lock_ok() = Arc::new(snapshot);
}

// ==================== IPC 命令 ====================

fn status_payload(host: &GatewayHost) -> serde_json::Value {
    let snapshot = host.snapshot();
    let health = host.0.health.lock_ok();
    let counters = host.0.counters.lock_ok();
    let server = host.0.server.lock_ok();
    let server_error = host.0.server_error.lock_ok().clone();
    let now = now_ms();
    let groups: Vec<serde_json::Value> = snapshot
        .groups
        .iter()
        .map(|rg| {
            let models: Vec<serde_json::Value> = rg
                .group
                .models
                .iter()
                .map(|m| {
                    let cand = rg.candidates.iter().find(|c| c.id == m.id);
                    let key = format!("{}/{}", rg.group.id, m.id);
                    let state = health.get(&key).map(|h| h.state(now)).unwrap_or(sched::HealthState::Healthy);
                    serde_json::json!({
                        "id": m.id, "enabled": m.enabled, "weight": m.weight, "alias": m.alias,
                        "provider": m.provider, "base_url": m.base_url, "model": m.model,
                        "label": cand.map(|c| c.label.clone()).unwrap_or_else(|| m.alias.clone()),
                        "upstream_model": cand.map(|c| c.model.clone()).unwrap_or_default(),
                        "unavailable": cand.and_then(|c| c.unavailable.clone()),
                        "health": state.as_str(),
                    })
                })
                .collect();
            serde_json::json!({
                "id": rg.group.id, "name": rg.group.name, "enabled": rg.group.enabled,
                "key": rg.group.key, "strategy": rg.group.effective_strategy(),
                "context_window": rg.group.effective_context_window(),
                "max_output": rg.group.effective_max_output(),
                "temperature": rg.group.temperature, "system_prompt": rg.group.system_prompt,
                "timeout_seconds": rg.group.timeout_seconds, "models": models,
                "counters": counters.get(&rg.group.id).copied().unwrap_or_default(),
            })
        })
        .collect();
    serde_json::json!({
        "running": server.as_ref().map(|h| h.is_running()).unwrap_or(false),
        "port": snapshot.settings.port,
        "enabled": snapshot.settings.enabled,
        "error": server_error,
        "groups": groups,
    })
}

/// 网关运行态:服务状态 + 全部组(含每个模型的健康与不可用原因) + 计数。
#[tauri::command]
pub fn gateway_status(app: AppHandle) -> Result<serde_json::Value, String> {
    let host = app.state::<GatewayHost>();
    Ok(status_payload(&host))
}

/// 最近请求日志(环形缓冲,默认全部,limit 截尾取最新)。
#[tauri::command]
pub fn gateway_log(app: AppHandle, limit: Option<u32>) -> Result<Vec<LogEntry>, String> {
    let host = app.state::<GatewayHost>();
    let log = host.0.log.lock_ok();
    let take = limit.map(|n| n as usize).unwrap_or(usize::MAX);
    let start = log.len().saturating_sub(take);
    Ok(log.iter().skip(start).cloned().collect())
}

/// 保存事务体:校验失败经 err_slot 透出且不改配置;成功则原位替换/追加。
/// 保存事务体:校验失败经 err_slot 透出且不改配置;成功则原位替换/追加,
/// 并把**归一化后的组**(含新生成的 id/key)经 saved_slot 交还调用方——
/// 新建时 UI 传来的 id 是空串,落盘后只有归一化 id 可用于回查。
fn upsert_group(
    cfg: &mut crate::config::DesktopConfig,
    group: ModelGroup,
    err_slot: &mut Option<String>,
    saved_slot: &mut Option<ModelGroup>,
) {
    let other_names: Vec<String> =
        cfg.gateway.groups.iter().filter(|g| g.id != group.id).map(|g| g.name.clone()).collect();
    let normalized = match group.clone().normalized_for_save(&other_names) {
        Ok(g) => g,
        Err(e) => {
            *err_slot = Some(e);
            return;
        }
    };
    match cfg.gateway.groups.iter().position(|g| g.id == normalized.id) {
        Some(i) => cfg.gateway.groups[i] = normalized.clone(),
        None => {
            if cfg.gateway.groups.len() >= MAX_GROUPS {
                *err_slot = Some(format!("模型组数量已达上限({MAX_GROUPS})"));
                return;
            }
            cfg.gateway.groups.push(normalized.clone());
        }
    }
    *saved_slot = Some(normalized);
}

/// 新建或更新模型组(id 为空 = 新建)。返回归一化后的组(含生成的 id/key)。
#[tauri::command]
pub async fn gateway_save_group(app: AppHandle, group: ModelGroup) -> Result<ModelGroup, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut err: Option<String> = None;
        let mut saved: Option<ModelGroup> = None;
        crate::config::update_config_json(&app, |cfg| upsert_group(cfg, group.clone(), &mut err, &mut saved))?;
        if let Some(e) = err {
            return Err(e);
        }
        // 用归一化 id 回查(新建组的 UI 侧 id 为空,拿它找必然扑空——
        // 那正是"保存后未找到模型组"的根因)
        let saved = saved.ok_or_else(|| "保存后未找到模型组(内部错误)".to_string())?;
        reload(&app);
        let host = app.state::<GatewayHost>();
        let snapshot = host.snapshot();
        snapshot
            .groups
            .iter()
            .find(|rg| rg.group.id == saved.id)
            .map(|rg| rg.group.clone())
            .ok_or_else(|| "保存后未找到模型组(内部错误)".to_string())
    })
    .await
    .map_err(|e| format!("保存失败: {e}"))?
}

/// 删除模型组。
#[tauri::command]
pub async fn gateway_delete_group(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut err: Option<String> = None;
        crate::config::update_config_json(&app, |cfg| {
            let before = cfg.gateway.groups.len();
            cfg.gateway.groups.retain(|g| g.id != id);
            if cfg.gateway.groups.len() == before {
                err = Some(format!("模型组不存在: {id}"));
            }
        })?;
        if let Some(e) = err {
            return Err(e);
        }
        reload(&app);
        Ok(())
    })
    .await
    .map_err(|e| format!("删除失败: {e}"))?
}

/// 网关总开关与端口(变更即重启/停掉监听)。
#[tauri::command]
pub async fn gateway_update_settings(app: AppHandle, enabled: bool, port: u16) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !(1024..=65535).contains(&port) {
            return Err("端口需在 1024-65535 之间".to_string());
        }
        crate::config::update_config_json(&app, |cfg| {
            cfg.gateway.enabled = enabled;
            cfg.gateway.port = port;
        })?;
        reload(&app);
        Ok(())
    })
    .await
    .map_err(|e| format!("保存失败: {e}"))?
}

/// 重置组 Key(旧 Key 立即失效)。
#[tauri::command]
pub async fn gateway_regen_key(app: AppHandle, id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let key = new_group_key();
        let mut err: Option<String> = None;
        crate::config::update_config_json(&app, |cfg| match cfg.gateway.groups.iter_mut().find(|g| g.id == id) {
            Some(g) => g.key = key.clone(),
            None => err = Some(format!("模型组不存在: {id}")),
        })?;
        if let Some(e) = err {
            return Err(e);
        }
        reload(&app);
        Ok(key)
    })
    .await
    .map_err(|e| format!("重置失败: {e}"))?
}

/// 组连通性测试:走**真实调度链路**(含故障切换与熔断),发一条最小对话,
/// 报告最终由哪个模型应答、耗时与失败摘要。
#[tauri::command]
pub async fn gateway_test_group(app: AppHandle, id: String) -> Result<serde_json::Value, String> {
    let host = app.state::<GatewayHost>().inner().clone();
    let snapshot = host.snapshot();
    let group = snapshot
        .group_by_id(&id)
        .ok_or_else(|| format!("模型组不存在: {id}"))?
        .clone();
    let started = std::time::Instant::now();
    let body = serde_json::json!({
        "model": id,
        "messages": [{ "role": "user", "content": "ping" }],
        "max_tokens": 16,
    });
    let result = server::run_buffered(&host, &group, body).await;
    let latency = started.elapsed().as_millis() as u64;
    Ok(match result {
        Ok(reply) => serde_json::json!({
            "ok": true, "model": reply.model, "latency_ms": latency,
            "status": 200, "attempts": reply.attempts,
            "content": reply.body.pointer("/choices/0/message/content").and_then(|v| v.as_str()).unwrap_or(""),
        }),
        Err(failed) => serde_json::json!({
            "ok": false, "latency_ms": latency, "status": failed.status,
            "attempts": failed.attempts, "error": failed.summary,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(name: &str, models: Vec<GroupModel>) -> ModelGroup {
        ModelGroup {
            id: String::new(),
            name: name.into(),
            enabled: true,
            key: new_group_key(),
            strategy: STRATEGY_PRIORITY.into(),
            context_window: 0, // 触发缺省值
            max_output: 0,
            temperature: None,
            system_prompt: String::new(),
            timeout_seconds: 0,
            models,
        }
    }

    fn custom(url: &str, model: &str, weight: u32) -> GroupModel {
        GroupModel {
            id: String::new(),
            enabled: true,
            weight,
            alias: String::new(),
            provider: "openai".into(),
            base_url: url.into(),
            api_key: "k".into(),
            model: model.into(),
        }
    }

    #[test]
    fn normalized_group_fills_ids_keys_and_defaults() {
        let g = group("测试组", vec![custom("https://a.example.com", "m1", 0)]).normalized_for_save(&[]).unwrap();
        assert!(g.id.starts_with("mg-"));
        assert!(g.key.starts_with("tgk-"));
        assert_eq!(g.effective_context_window(), DEFAULT_CONTEXT_WINDOW);
        assert_eq!(g.effective_max_output(), DEFAULT_MAX_OUTPUT);
        assert_eq!(g.effective_timeout(), std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECONDS));
        assert!(g.models[0].id.len() > 3);
        assert_eq!(g.models[0].weight, 1, "权重 0 应钳到 1");
    }
    #[test]
    fn save_rejects_empty_duplicate_and_long_names() {
        let g = group("  ", vec![]).normalized_for_save(&[]);
        assert!(g.err().unwrap().contains("名称不能为空"));
        let g = group("dup", vec![]).normalized_for_save(&["dup".to_string()]);
        assert!(g.err().unwrap().contains("已存在"));
        let long = "x".repeat(MAX_NAME_LEN + 1);
        let g = group(&long, vec![]).normalized_for_save(&[]);
        assert!(g.err().unwrap().contains("过长"));
    }

    #[test]
    fn custom_model_requires_base_url_and_model() {
        let m = GroupModel { base_url: "".into(), ..custom("", "", 1) };
        assert!(m.normalized().err().unwrap().contains("接口地址"));
        let m = GroupModel { base_url: "https://x".into(), model: "".into(), ..custom("", "", 1) };
        assert!(m.normalized().err().unwrap().contains("模型标识"));
        // 引用条目不需要连接字段
        let m = GroupModel { base_url: "".into(), model: "".into(), api_key: "".into(), provider: "".into(), alias: "库模型".into(), ..custom("", "", 1) };
        let normalized = m.clone().normalized().unwrap();
        assert!(normalized.base_url.is_empty(), "引用条目不应携带手填连接字段");
        assert!(normalized.id.starts_with("gm-"), "归一化补 id");
    }

    /// 快照构建:引用条目按 name 解析;缺失引用 → unavailable;
    /// 会员条目按当前配置注入地址。
    #[test]
    fn build_snapshot_resolves_aliases_and_marks_missing() {
        let dir = std::env::temp_dir().join(format!("mc-gw-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let models = serde_json::json!([
            { "name": "库模型", "provider": "openai", "base_url": "https://lib.example.com", "api_key": "k1", "model": "m-lib" },
            { "name": "会员", "provider": "anthropic", "base_url": "", "api_key": "", "model": "m-mc", "source": "monkeycode" }
        ]);
        let mut g = group("组A", vec![]);
        g.models = vec![
            GroupModel { alias: "库模型".into(), ..custom("", "", 5) },
            GroupModel { alias: "不存在".into(), ..custom("", "", 3) },
            custom("https://direct.example.com", "m-direct", 1),
        ];
        let cfg = crate::config::DesktopConfig {
            models,
            gateway: GatewaySettings { enabled: true, port: 1000, groups: vec![g] },
            ..Default::default()
        };
        let snap = build_snapshot(&cfg, &dir);
        let rg = &snap.groups[0];
        assert_eq!(rg.candidates.len(), 3);
        let lib = &rg.candidates[0];
        assert_eq!(lib.base_url, "https://lib.example.com");
        assert_eq!(lib.model, "m-lib");
        assert_eq!(lib.weight, 5);
        let missing = &rg.candidates[1];
        assert!(missing.unavailable.as_deref().unwrap().contains("不存在"));
        let direct = &rg.candidates[2];
        assert_eq!(direct.base_url, "https://direct.example.com");
        // 会员条目:官方云默认地址(无本地 Key 记录 → key 为空,请求时报错外显)
        let mut g2 = group("组B", vec![]);
        g2.models = vec![GroupModel { alias: "会员".into(), ..custom("", "", 1) }];
        let cfg2 = crate::config::DesktopConfig {
            models: serde_json::json!([
                { "name": "会员", "provider": "anthropic", "base_url": "", "api_key": "", "model": "m-mc", "source": "monkeycode" }
            ]),
            gateway: GatewaySettings { enabled: true, port: 1000, groups: vec![g2] },
            ..Default::default()
        };
        let snap2 = build_snapshot(&cfg2, &dir);
        let mc = &snap2.groups[0].candidates[0];
        assert_eq!(mc.base_url, crate::baizhi::DEFAULT_MONKEYCODE_LLM_URL);
        assert_eq!(mc.provider, "anthropic");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 保存事务回归(2026-08-30 用户报障):新建组 UI 传空 id,保存后必须
    /// 能拿到归一化 id 回查;二次保存原位替换不重复;空 id 重试撞名报错。
    #[test]
    fn upsert_assigns_id_and_second_save_replaces() {
        let mut cfg = crate::config::DesktopConfig::default();
        let mut err: Option<String> = None;
        let mut saved: Option<ModelGroup> = None;
        upsert_group(&mut cfg, group("组X", vec![custom("https://a", "m1", 1)]), &mut err, &mut saved);
        assert!(err.is_none());
        let first = saved.clone().expect("归一化组必须交回");
        assert!(first.id.starts_with("mg-"));
        assert!(!first.key.is_empty());
        assert_eq!(cfg.gateway.groups.len(), 1);
        assert_eq!(cfg.gateway.groups[0].models.len(), 1, "模型随组落盘");

        // 二次保存(带归一化 id,新增一个模型):原位替换,不重复建组
        let mut updated = first.clone();
        updated.models.push(custom("https://b", "m2", 2));
        upsert_group(&mut cfg, updated, &mut err, &mut saved);
        assert!(err.is_none());
        assert_eq!(cfg.gateway.groups.len(), 1);
        assert_eq!(cfg.gateway.groups[0].models.len(), 2, "新增模型已写入");

        // 用户在报错后原表单重试的形态(仍空 id + 同名):撞名报错,不产生第二组
        let mut err2: Option<String> = None;
        let mut saved2: Option<ModelGroup> = None;
        upsert_group(&mut cfg, group("组X", vec![]), &mut err2, &mut saved2);
        assert!(err2.unwrap().contains("已存在"));
        assert_eq!(cfg.gateway.groups.len(), 1);
    }

    /// group_by_key 是鉴权唯一入口:只匹配启用组的 Key,且常时比较。
    #[test]
    fn group_lookup_by_key_respects_enabled() {
        let mut g = group("A", vec![]);
        g.key = "tgk-aaa".into();
        let mut off = group("B", vec![]);
        off.key = "tgk-bbb".into();
        off.enabled = false;
        let snap = RuntimeSnapshot {
            settings: GatewaySettings::default(),
            groups: vec![
                RuntimeGroup { group: g, candidates: vec![] },
                RuntimeGroup { group: off, candidates: vec![] },
            ],
        };
        assert!(snap.group_by_key("tgk-aaa").is_some());
        assert!(snap.group_by_key("tgk-bbb").is_none(), "停用组的 Key 不得鉴权");
        assert!(snap.group_by_key("tgk-ccc").is_none());
    }
}
