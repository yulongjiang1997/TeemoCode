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

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::util::LockExt;

pub const DEFAULT_PORT: u16 = 8317;

/// 统计日志落盘目录缓存:`manage` 启动早期写入 app_config_dir,之后
/// `GatewayHost::new()`(服务线程、单测)都能取到同一份路径;取不到时
/// 退回当前目录(测试场景无 AppHandle,不污染真实配置)。
static CONFIG_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// 由 `manage` 在启动早期写入;测试不调用则退回当前目录。
pub(crate) fn init_config_dir(dir: PathBuf) {
    let _ = CONFIG_DIR.set(dir);
}

/// 统计日志落盘目录。
pub(crate) fn log_stats_dir() -> PathBuf {
    CONFIG_DIR.get().cloned().unwrap_or_else(|| PathBuf::from("."))
}

/// 构造统计仓库:仅当 `manage` 已初始化目录(生产)才挂真实仓库;
/// 单测路径(无 AppHandle)不挂,避免往测试 cwd 落文件。
fn build_log_store() -> Option<GatewayLogStore> {
    if CONFIG_DIR.get().is_some() {
        Some(GatewayLogStore::new(&log_stats_dir()))
    } else {
        None
    }
}

pub const DEFAULT_CONTEXT_WINDOW: i64 = 128_000;
pub const DEFAULT_MAX_OUTPUT: i64 = 32_768;
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
/// 单组模型条目上限(权重顺序列表,再多调度意义与可读性都崩)。
pub const MAX_GROUP_MODELS: usize = 256;
/// 模型组数量上限。
pub const MAX_GROUPS: usize = 32;
pub const MAX_NAME_LEN: usize = 64;
/// 调度策略词汇(对外契约,UI 下拉与持久化同源)。
pub const STRATEGY_PRIORITY: &str = "priority";
pub const STRATEGY_WEIGHTED: &str = "weighted";
/// 最快模式:自动定时探测模型延迟,优选延迟最低的模型。
pub const STRATEGY_FASTEST: &str = "fastest";
/// 负载均衡:顺序轮转(round-robin),不按权重。
pub const STRATEGY_BALANCED: &str = "balanced";

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
    /// 厂商预设(2026-09-14):用户自建的厂商接入预设,存 config.json。
    /// 添加模型时选一个预设即可自动填 provider+base_url+api_key,
    /// 然后直接获取模型列表,无需重复手填。
    #[serde(default)]
    pub vendor_presets: Vec<VendorPreset>,
}

/// 厂商预设(用户自建)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VendorPreset {
    /// 唯一标识(uuid 截断)。
    pub id: String,
    /// 展示名(如 "我的 OpenAI"、"DeepSeek 账号 A")。
    pub name: String,
    /// 协议:openai | anthropic | openai_responses
    pub provider: String,
    /// 接口地址(如 https://api.openai.com)
    pub base_url: String,
    /// API Key(明文存 config.json,与模型组同款)。
    pub api_key: String,
    /// 上下文窗口(token);0 = 缺省。
    #[serde(default)]
    pub context_window: u64,
    /// 最大输出(token);0 = 缺省。
    #[serde(default)]
    pub max_output: u64,
    /// 是否支持图片输入。
    #[serde(default)]
    pub vision: bool,
    /// 思考模式:off | low | medium | high | max;空串 = 缺省。
    #[serde(default)]
    pub think: String,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self { enabled: false, port: default_port(), groups: vec![], vendor_presets: vec![] }
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
    /// 是否记录探测/调用日志(2026-09-14);false 时探测不写日志。
    #[serde(default = "default_true")]
    pub log_enabled: bool,
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
        match self.strategy.as_str() {
            STRATEGY_WEIGHTED => STRATEGY_WEIGHTED,
            STRATEGY_FASTEST => STRATEGY_FASTEST,
            STRATEGY_BALANCED => STRATEGY_BALANCED,
            _ => STRATEGY_PRIORITY,
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
    /// 仅用于把 pending 条目与同一个请求的完成回调关联起来；不对外暴露。
    #[serde(skip)]
    pub(crate) request_id: u64,
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
    /// 请求/响应内容(2026-09-14):pending 时不填;完成后写入。
    /// 截断到 500 字符避免内存膨胀(环形缓冲只留 LOG_CAP 条)。
    pub request_content: Option<String>,
    pub response_content: Option<String>,
    /// 请求是否仍在进行中(2026-09-14):true = 请求已发出但未收到响应。
    /// 前端展示"请求中"态;完成后翻为 false。
    pub pending: bool,
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

// ==================== 持久化调用日志(按天 × 模型聚合) ====================
//
// 2026-09-12 需求:本地大模型网关独立成左侧空间并新增 token 统计面板
// (按模型分类 / 时间范围 / 热力图 / 调用总时长)。内存日志只保 LOG_CAP 条
// 供「最近请求」表用,跨会话的时间维度统计需要落盘聚合。
//
// 落盘:`<app_config>/gateway-log-stats.json`,结构
// `{ "<YYYY-MM-DD>": { "<模型展示名>": GatewayDayRecord } }`。与
// stats.rs(本地会话用量)同为「聚合态落盘」而非逐请求流水:每次请求
// 累加到对应 天/模型 桶再整体原子写,请求级 token 明细仍以 LogEntry 保留。
// 保留 KEEP_DAYS 天,与本地会话统计口径一致。

/// 网关统计日志保留天数(2026-09-12 统计面板「累计」口径)。
const LOG_STATS_KEEP_DAYS: i64 = 366;

/// 某天某模型的聚合调用记录。
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct GatewayDayRecord {
    /// 调用次数(含失败;失败请求也计入「调用」与「总时长」)。
    pub calls: u64,
    /// 成功次数。
    pub ok_calls: u64,
    /// 失败次数。
    pub fail_calls: u64,
    /// prompt tokens 累计(null 不计入,与 LogEntry 口径一致)。
    pub input_tokens: u64,
    /// completion tokens 累计。
    pub output_tokens: u64,
    /// 请求耗时累计(毫秒,取 LogEntry.latency_ms)。面板换算秒展示。
    pub duration_ms: u64,
}

/// 聚合范围:today=当日,day7=近7日(含当日),all=全部留存。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GatewayRangeKind {
    #[default]
    Today,
    Day7,
    All,
}

/// UI 传来的范围字符串归一化(`today`/`7d`/`all` 均可,容错大小写与点号)。
pub(crate) fn parse_range_kind(s: &str) -> GatewayRangeKind {
    match s.trim().to_ascii_lowercase().replace('.', "").as_str() {
        "day7" | "7d" | "week" => GatewayRangeKind::Day7,
        "all" | "total" | "lifetime" => GatewayRangeKind::All,
        _ => GatewayRangeKind::Today,
    }
}

/// 单个模型在所选范围内的统计。
#[derive(Default, Clone, Debug, Serialize)]
pub struct GatewayModelStats {
    pub model: String,
    pub calls: u64,
    pub ok_calls: u64,
    pub fail_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    /// 请求耗时累计(毫秒)。
    pub duration_ms: u64,
}

/// 单日热力图数据点。
#[derive(Default, Clone, Debug, Serialize)]
pub struct GatewayHeatDay {
    /// `YYYY-MM-DD`。
    pub date: String,
    pub total_tokens: u64,
    pub calls: u64,
}

/// 统计查询响应(对表前端 GatewayLogStats)。
#[derive(Default, Clone, Debug, Serialize)]
pub struct GatewayLogStats {
    /// 请求的聚合范围(原样回传,便于 UI 校验)。
    pub range: GatewayRangeKind,
    /// 范围内总输入 tokens。
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_tokens: u64,
    /// 范围内总调用次数。
    pub total_calls: u64,
    /// 范围内请求耗时累计(毫秒,面板换算秒)。
    pub total_duration_ms: u64,
    /// 按 total_tokens 降序;total_tokens 相同按模型名升序(稳定排序)。
    pub models: Vec<GatewayModelStats>,
    /// 全部留存天数(热力图,按日期升序)。
    pub heatmap: Vec<GatewayHeatDay>,
}

/// 跨会话的网关调用统计仓库(与 UsageStats 同款聚合落盘)。
/// 挂在 `GatewayInner.log_stats` 上,push_log 与服务线程共用同一份内存态。
pub struct GatewayLogStore {
    path: PathBuf,
    days: StdMutex<BTreeMap<String, BTreeMap<String, GatewayDayRecord>>>,
}

impl GatewayLogStore {
    fn new(config_dir: &Path) -> Self {
        let path = config_dir.join("gateway-log-stats.json");
        let days = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<BTreeMap<String, BTreeMap<String, GatewayDayRecord>>>(&s).ok())
            .unwrap_or_default();
        let store = Self { path, days: StdMutex::new(days) };
        store.prune();
        store
    }

    /// 记一次请求:按 天 × 模型 聚合累加,然后整体原子落盘。
    pub fn record(&self, entry: &LogEntry) {
        let date = crate::stats::today_str();
        let model: String = if entry.model.is_empty() { "<unknown>".into() } else { entry.model.clone() };
        {
            let mut days = self.days.lock_ok();
            let rec = days
                .entry(date)
                .or_default()
                .entry(model)
                .or_default();
            rec.calls += 1;
            if entry.ok {
                rec.ok_calls += 1;
            } else {
                rec.fail_calls += 1;
            }
            if let Some(t) = entry.prompt_tokens {
                rec.input_tokens = rec.input_tokens.saturating_add(t.max(0) as u64);
            }
            if let Some(t) = entry.completion_tokens {
                rec.output_tokens = rec.output_tokens.saturating_add(t.max(0) as u64);
            }
            rec.duration_ms = rec.duration_ms.saturating_add(entry.latency_ms);
        }
        // 落盘在锁外做;统计面板不要求零丢失,失败静默(stats.rs 同款约定)。
        self.prune();
        let _ = serde_json::to_vec_pretty(&*self.days.lock_ok())
            .map(|data| crate::config::atomic_write_private(&self.path, &data));
    }

    /// 丢掉超过保留期的旧天。
    fn prune(&self) {
        let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        let cutoff = now.date().checked_sub(time::Duration::days(LOG_STATS_KEEP_DAYS)).unwrap_or(now.date());
        let cutoff_s = format!("{:04}-{:02}-{:02}", cutoff.year(), cutoff.month() as u8, cutoff.day());
        let mut days = self.days.lock_ok();
        days.retain(|date, _| date.as_str() >= cutoff_s.as_str());
    }

    /// 按范围聚合出统计面板所需数据。
    pub fn query(&self, range: GatewayRangeKind) -> GatewayLogStats {
        let days = self.days.lock_ok();
        let today = crate::stats::today_str();
        let mut lo = String::new();
        match range {
            GatewayRangeKind::Today => lo = today.clone(),
            GatewayRangeKind::Day7 => {
                let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
                let d = now.date().checked_sub(time::Duration::days(6)).unwrap_or(now.date());
                lo = format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day());
            }
            GatewayRangeKind::All => lo = String::new(),
        }
        let mut totals = (0u64, 0u64, 0u64, 0u64, 0u64); // input, output, calls, ok, fail
        let mut by_model: HashMap<String, GatewayModelStats> = HashMap::new();
        let mut heat = BTreeMap::new(); // date -> (tokens, calls)
        for (date, models) in days.iter() {
            let in_range = date.as_str() >= lo.as_str();
            for (model, rec) in models.iter() {
                let tokens = rec.input_tokens.saturating_add(rec.output_tokens);
                let h = heat.entry(date.clone()).or_insert((0u64, 0u64));
                h.0 = h.0.saturating_add(tokens);
                h.1 = h.1.saturating_add(rec.calls);
                if !in_range {
                    continue;
                }
                totals.0 = totals.0.saturating_add(rec.input_tokens);
                totals.1 = totals.1.saturating_add(rec.output_tokens);
                totals.2 = totals.2.saturating_add(rec.calls);
                totals.3 = totals.3.saturating_add(rec.ok_calls);
                totals.4 = totals.4.saturating_add(rec.fail_calls);
                let m = by_model.entry(model.clone()).or_default();
                m.model = model.clone();
                m.calls = m.calls.saturating_add(rec.calls);
                m.ok_calls = m.ok_calls.saturating_add(rec.ok_calls);
                m.fail_calls = m.fail_calls.saturating_add(rec.fail_calls);
                m.input_tokens = m.input_tokens.saturating_add(rec.input_tokens);
                m.output_tokens = m.output_tokens.saturating_add(rec.output_tokens);
                m.total_tokens = m.total_tokens.saturating_add(tokens);
                m.duration_ms = m.duration_ms.saturating_add(rec.duration_ms);
            }
        }
        // 模型列表稳定排序:总 tokens 降序 → 调用数降序 → 名称升序
        let mut models: Vec<GatewayModelStats> = by_model.into_values().collect();
        models.sort_by(|a, b| {
            b.total_tokens.cmp(&a.total_tokens)
                .then(b.calls.cmp(&a.calls))
                .then(a.model.cmp(&b.model))
        });
        let total_duration_ms: u64 = models.iter().map(|m| m.duration_ms).sum();
        let heatmap: Vec<GatewayHeatDay> = heat.into_iter().map(|(date, (tok, calls))| GatewayHeatDay {
            date,
            total_tokens: tok,
            calls,
        }).collect();
        GatewayLogStats {
            range,
            total_input_tokens: totals.0,
            total_output_tokens: totals.1,
            total_tokens: totals.0.saturating_add(totals.1),
            total_calls: totals.2,
            total_duration_ms,
            models,
            heatmap,
        }
    }
}

// ==================== Per-request log persistence (JSONL + sidecar) ====================
//
// 完成的请求写入 `gateway-requests.jsonl`(元数据 + 短摘要)。完整请求/响应体
// 另存 `gateway-request-bodies/{id}.json`,详情打开时再读,避免列表轮询拖 1MB
// 级 body。内存环(LOG_CAP=100)只给「请求中」占位;重启后列表一律走磁盘。

/// Maximum entries kept on disk before rotation trims oldest.
const PERSISTED_LOG_CAP: usize = 2_000;
/// Sidecar 中原始请求/响应体上限(字节)。超出会截断并标注原长度。
const RAW_BODY_CAP: usize = 512 * 1024;
/// JSONL 里留给搜索用的 body 摘要。
const RAW_SNIPPET_CAP: usize = 4_096;
/// Truncation limit for request/response content in persisted entries (2000 chars).
const PERSISTED_CONTENT_CAP: usize = 2_000;

fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…(truncated, {} bytes)", &s[..end], s.len())
}

fn persisted_entry_key(entry: &PersistedLogEntry) -> String {
    if entry.id.is_empty() {
        format!("{}-{}", entry.ts_ms, entry.group_id)
    } else {
        entry.id.clone()
    }
}

fn safe_body_id(id: &str) -> Option<&str> {
    if id.is_empty() || id.len() > 96 {
        return None;
    }
    if id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        Some(id)
    } else {
        None
    }
}

/// A complete log entry persisted to the JSONL file. Extends LogEntry with
/// request/response snippets; full bodies live in a sidecar keyed by `id`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedLogEntry {
    /// `{ts_ms}-{request_id}`; 旧文件缺省时由 ts_ms+group_id 合成。
    #[serde(default)]
    pub id: String,
    // --- same fields as LogEntry ---
    pub ts_ms: u64,
    pub group_id: String,
    pub group_name: String,
    pub stream: bool,
    pub ok: bool,
    pub status: Option<u16>,
    pub latency_ms: u64,
    pub model: String,
    pub attempts: u32,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub error: Option<String>,
    /// Truncated to 2000 chars for persisted entries (vs 500 in memory).
    pub request_content: Option<String>,
    /// Truncated to 2000 chars for persisted entries (vs 500 in memory).
    pub response_content: Option<String>,
    /// Persisted entries are always complete; retained for a uniform UI shape.
    #[serde(default)]
    pub pending: bool,
    // --- extra fields only in persisted entries ---
    /// Full request body (truncated to 10KB). Only in JSONL, not in memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_request: Option<String>,
    /// Full response body (truncated to 10KB). Only in JSONL, not in memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_response: Option<String>,
}

/// Filter parameters for querying persisted log entries.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct LogQuery {
    /// Filter by group id (exact match).
    #[serde(default)]
    pub group_id: Option<String>,
    /// Filter by model name (substring, case-insensitive).
    #[serde(default)]
    pub model: Option<String>,
    /// Filter by success/failure: true = only success, false = only fail.
    #[serde(default)]
    pub ok: Option<bool>,
    /// 搜索组名/模型/错误/摘要/body 片段(大小写不敏感)。
    #[serde(default)]
    pub search: Option<String>,
    /// Max entries to return (default 200).
    #[serde(default = "default_log_limit")]
    pub limit: usize,
    /// Pagination offset.
    #[serde(default)]
    pub offset: usize,
}

fn default_log_limit() -> usize {
    200
}

/// Manages the `gateway-requests.jsonl` file: append-only writes with rotation.
pub struct GatewayLogPersistence {
    path: PathBuf,
    bodies_dir: PathBuf,
    io: StdMutex<()>,
}

impl GatewayLogPersistence {
    fn new(config_dir: &Path) -> Self {
        let path = config_dir.join("gateway-requests.jsonl");
        let bodies_dir = config_dir.join("gateway-request-bodies");
        let _ = std::fs::create_dir_all(&bodies_dir);
        Self { path, bodies_dir, io: StdMutex::new(()) }
    }

    fn body_path(&self, id: &str) -> Option<PathBuf> {
        safe_body_id(id).map(|id| self.bodies_dir.join(format!("{id}.json")))
    }

    fn write_sidecar_locked(&self, id: &str, raw_request: Option<&str>, raw_response: Option<&str>) {
        let Some(path) = self.body_path(id) else { return };
        if raw_request.is_none() && raw_response.is_none() {
            return;
        }
        let body = serde_json::json!({
            "request": raw_request.map(|s| truncate_bytes(s, RAW_BODY_CAP)),
            "response": raw_response.map(|s| truncate_bytes(s, RAW_BODY_CAP)),
        });
        if let Ok(data) = serde_json::to_vec(&body) {
            let _ = crate::config::atomic_write_private(&path, &data);
        }
    }

    fn read_sidecar_locked(&self, id: &str) -> Option<(Option<String>, Option<String>)> {
        let path = self.body_path(id)?;
        let text = std::fs::read_to_string(path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let req = v.get("request").and_then(|x| x.as_str()).map(|s| s.to_string());
        let resp = v.get("response").and_then(|x| x.as_str()).map(|s| s.to_string());
        Some((req, resp))
    }

    /// Append a completed entry to the JSONL file. After appending, if the file
    /// exceeds PERSISTED_LOG_CAP lines, rewrite it with only the last
    /// PERSISTED_LOG_CAP entries (trim oldest).
    fn append(&self, mut entry: PersistedLogEntry, raw_request: Option<&str>, raw_response: Option<&str>) {
        let _guard = self.io.lock_ok();
        if entry.id.is_empty() {
            entry.id = persisted_entry_key(&entry);
        }
        if entry.raw_request.is_none() {
            entry.raw_request = raw_request.map(|s| truncate_bytes(s, RAW_SNIPPET_CAP));
        }
        if entry.raw_response.is_none() {
            entry.raw_response = raw_response.map(|s| truncate_bytes(s, RAW_SNIPPET_CAP));
        }
        self.write_sidecar_locked(&entry.id, raw_request, raw_response);
        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(_) => return,
        };
        let append_result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| {
                use std::io::Write as _;
                writeln!(f, "{line}")
            });
        if append_result.is_err() {
            return;
        }
        self.rotate_if_needed_locked();
    }

    fn rotate_if_needed_locked(&self) {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        if lines.len() <= PERSISTED_LOG_CAP {
            return;
        }
        let drop_n = lines.len() - PERSISTED_LOG_CAP;
        for line in &lines[..drop_n] {
            if let Ok(entry) = serde_json::from_str::<PersistedLogEntry>(line) {
                if let Some(path) = self.body_path(&entry.id) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        let keep = &lines[drop_n..];
        let rewritten = keep.join("\n") + "\n";
        let _ = crate::config::atomic_write_private(&self.path, rewritten.as_bytes());
    }

    /// Query persisted entries with filters. Returns (matching entries, total
    /// count). Entries are returned newest-first (reverse file order).
    fn query(&self, q: &LogQuery) -> (Vec<PersistedLogEntry>, usize) {
        let _guard = self.io.lock_ok();
        let content = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(_) => return (vec![], 0),
        };
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        let mut all: Vec<PersistedLogEntry> = Vec::with_capacity(lines.len());
        for line in lines.iter().rev() {
            if let Ok(mut entry) = serde_json::from_str::<PersistedLogEntry>(line) {
                if Self::matches(&entry, q) {
                    if entry.id.is_empty() {
                        entry.id = persisted_entry_key(&entry);
                    }
                    all.push(entry);
                }
            }
        }
        let total = all.len();
        let offset = q.offset.min(total);
        let end = offset.saturating_add(q.limit).min(total);
        let page = all[offset..end].to_vec();
        (page, total)
    }

    fn get(&self, id: &str) -> Option<PersistedLogEntry> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        let _guard = self.io.lock_ok();
        let content = std::fs::read_to_string(&self.path).ok()?;
        for line in content.lines().rev() {
            if line.is_empty() {
                continue;
            }
            let Ok(mut entry) = serde_json::from_str::<PersistedLogEntry>(line) else {
                continue;
            };
            if persisted_entry_key(&entry) != id {
                continue;
            }
            if entry.id.is_empty() {
                entry.id = id.to_string();
            }
            if let Some((req, resp)) = self.read_sidecar_locked(&entry.id) {
                if req.as_ref().is_some_and(|s| !s.is_empty()) {
                    entry.raw_request = req;
                }
                if resp.as_ref().is_some_and(|s| !s.is_empty()) {
                    entry.raw_response = resp;
                }
            }
            return Some(entry);
        }
        None
    }

    /// Check if an entry matches the query filters.
    fn matches(entry: &PersistedLogEntry, q: &LogQuery) -> bool {
        if let Some(ref gid) = q.group_id {
            if entry.group_id != *gid {
                return false;
            }
        }
        if let Some(ref model) = q.model {
            let needle = model.to_lowercase();
            if !entry.model.to_lowercase().contains(&needle) {
                return false;
            }
        }
        if let Some(ok) = q.ok {
            if entry.ok != ok {
                return false;
            }
        }
        if let Some(ref search) = q.search {
            let needle = search.to_lowercase();
            let hay = [
                entry.group_name.as_str(),
                entry.model.as_str(),
                entry.error.as_deref().unwrap_or(""),
                entry.request_content.as_deref().unwrap_or(""),
                entry.response_content.as_deref().unwrap_or(""),
                entry.raw_request.as_deref().unwrap_or(""),
                entry.raw_response.as_deref().unwrap_or(""),
            ];
            if !hay.iter().any(|s| s.to_lowercase().contains(&needle)) {
                return false;
            }
        }
        true
    }
}

/// Build a PersistedLogEntry from a LogEntry + optional full raw bodies.
/// JSONL 只留摘要;完整 body 由 append 写入 sidecar。
fn to_persisted(
    entry: &LogEntry,
    raw_request: Option<&str>,
    raw_response: Option<&str>,
) -> PersistedLogEntry {
    PersistedLogEntry {
        id: format!("{}-{}", entry.ts_ms, entry.request_id),
        ts_ms: entry.ts_ms,
        group_id: entry.group_id.clone(),
        group_name: entry.group_name.clone(),
        stream: entry.stream,
        ok: entry.ok,
        status: entry.status,
        latency_ms: entry.latency_ms,
        model: entry.model.clone(),
        attempts: entry.attempts,
        prompt_tokens: entry.prompt_tokens,
        completion_tokens: entry.completion_tokens,
        error: entry.error.clone(),
        request_content: entry
            .request_content
            .as_ref()
            .map(|s| s.chars().take(PERSISTED_CONTENT_CAP).collect()),
        response_content: entry
            .response_content
            .as_ref()
            .map(|s| s.chars().take(PERSISTED_CONTENT_CAP).collect()),
        pending: false,
        raw_request: raw_request.map(|s| truncate_bytes(s, RAW_SNIPPET_CAP)),
        raw_response: raw_response.map(|s| truncate_bytes(s, RAW_SNIPPET_CAP)),
    }
}

/// Build persistence store only when config dir is initialized (production).
fn build_log_persistence() -> Option<GatewayLogPersistence> {
    if CONFIG_DIR.get().is_some() {
        Some(GatewayLogPersistence::new(&log_stats_dir()))
    } else {
        None
    }
}

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
    /// 请求序号(用于 pending→complete 匹配)。
    log_seq: StdMutex<u64>,
    /// 跨会话调用统计(按天 × 模型聚合落盘);生产由 `manage` 初始化,
    /// 单测路径(无 AppHandle)为 None,push_log 跳过持久化。
    log_stats: Option<Arc<GatewayLogStore>>,
    /// Per-request log persistence (JSONL); None in test path.
    log_persistence: Option<Arc<GatewayLogPersistence>>,
    /// fastest 模式:各模型最近一次探测延迟(毫秒),key = group_id/model_id。
    /// 由后台探测线程定期更新;plan() 读取此表排序。
    latencies: StdMutex<HashMap<String, u64>>,
    /// balanced 模式:round-robin 计数器,key = group_id。每次取
    /// counter % candidates.len() 作为起始下标,成功后自增。
    rr_counters: StdMutex<HashMap<String, u64>>,
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
            log_stats: build_log_store().map(Arc::new),
            log_persistence: build_log_persistence().map(Arc::new),
            log_seq: StdMutex::new(0),
            latencies: StdMutex::new(HashMap::new()),
            rr_counters: StdMutex::new(HashMap::new()),
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

    /// fastest 模式:读取各模型最近探测延迟(毫秒)。key = group_id/model_id。
    pub(crate) fn latencies(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.0.latencies.lock_ok()
    }

    /// 写入一次延迟探测结果(后台探测线程调用)。
    pub(crate) fn record_latency(&self, group_id: &str, model_id: &str, ms: u64) {
        self.0.latencies.lock_ok().insert(format!("{group_id}/{model_id}"), ms);
    }

    /// balanced 模式:取当前 round-robin 起始下标并自增计数器。
    /// 返回值 = counter % len;若 len=0 返回 0。每次请求规划时调用,
    /// 使后续请求从下一个候选开始轮转。
    pub(crate) fn rr_next(&self, group_id: &str, len: usize) -> usize {
        if len == 0 { return 0; }
        let mut counters = self.0.rr_counters.lock_ok();
        let c = counters.entry(group_id.to_string()).or_insert(0);
        let idx = (*c % len as u64) as usize;
        *c = c.saturating_add(1);
        idx
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

    /// 人工解除模型弃用状态(2026-09-15):复位健康簿,模型恢复可用。
    pub(crate) fn reset_model_health(&self, group_id: &str, model_id: &str) {
        let key = format!("{group_id}/{model_id}");
        let mut health = self.0.health.lock_ok();
        if let Some(h) = health.get_mut(&key) {
            h.reset_abandoned();
        }
    }

    /// 探测专用:只更新延迟,不影响 health(熔断器)。
    /// 探测失败不写 record_attempt,避免与实际请求互相干扰——
    /// 探测失败可能只是 max_tokens=1 的 ping 被拒绝,不代表模型真正不可用。
    /// 探测成功时复位 health(让临时熔断的模型提前恢复)。
    pub(crate) fn record_probe_result(&self, group_id: &str, model_id: &str, ok: bool, latency_ms: u64) {
        self.record_latency(group_id, model_id, latency_ms);
        if ok {
            // 探测成功:复位健康簿(让临时熔断的模型提前恢复)
            let key = format!("{group_id}/{model_id}");
            let mut health = self.0.health.lock_ok();
            if let Some(h) = health.get_mut(&key) {
                h.record_success();
            }
        }
        // 探测失败:只记延迟(u64::MAX),不写 record_attempt,
        // 避免探测失败累计 consecutive_failures 导致熔断/弃用
    }

    pub(crate) fn push_log(&self, entry: LogEntry) {
        let group_id = entry.group_id.clone();
        let ok = entry.ok;
        let failovers = entry.attempts.saturating_sub(1) as u64;
        {
            let mut log = self.0.log.lock_ok();
            log.push_back(entry.clone());
            while log.len() > LOG_CAP {
                log.pop_front();
            }
        }
        // pending 只是占位，不应提前计入成功/失败/统计；完成时由
        // complete_and_persist 统一记一次，避免 pending + completed 双计数。
        if !entry.pending {
            // 跨会话持久化聚合(2026-09-12 统计面板数据源)。与内存日志解耦:
            // 内存只留 LOG_CAP 条供「最近请求」表,persist 按 天×模型 累加落盘。
            // 单测路径(manage 未初始化)log_stats=None,跳过持久化不落盘文件。
            if let Some(store) = &self.0.log_stats {
                store.record(&entry);
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
    }

    /// 下一个请求序号(2026-09-14:pending→complete 日志匹配用)。
    pub(crate) fn next_log_seq(&self) -> u64 {
        let mut s = self.0.log_seq.lock_ok();
        *s += 1;
        *s
    }

    /// 请求收到响应后更新 pending 日志条目(2026-09-14)。
    /// 按 seq 在环形缓冲中找到对应条目,原地更新状态/延迟/内容。
    /// 找不到(已被挤出 LOG_CAP)则静默跳过——pending 只活几秒,极少被挤。
    pub(crate) fn update_log(&self, seq: u64, ok: bool, status: Option<u16>, latency_ms: u64,
        model: &str, attempts: u32, prompt_tokens: Option<i64>, completion_tokens: Option<i64>,
        error: Option<String>, request_content: Option<String>, response_content: Option<String>,
    ) {
        let mut log = self.0.log.lock_ok();
        // 从后往前找(pending 是最近推入的,逆序扫最快)
        for entry in log.iter_mut().rev() {
            if entry.pending {
                entry.ok = ok;
                entry.status = status;
                entry.latency_ms = latency_ms;
                if !model.is_empty() { entry.model = model.to_string(); }
                entry.attempts = attempts;
                entry.prompt_tokens = prompt_tokens;
                entry.completion_tokens = completion_tokens;
                entry.error = error;
                entry.request_content = request_content;
                entry.response_content = response_content;
                entry.pending = false;
                break;
            }
        }
    }

    /// 内部版:直接接收 Usage 结构(server.rs 调用路径)。
    /// 找最后一个 pending=true 的条目原地更新。找不到则静默跳过。
    pub(crate) fn update_log_inner(
        &self,
        stream: bool,
        ok: bool,
        status: Option<u16>,
        latency_ms: u64,
        model: &str,
        attempts: u32,
        usage: &upstream::Usage,
        error: Option<String>,
        request_content: Option<String>,
        response_content: Option<String>,
    ) {
        let mut log = self.0.log.lock_ok();
        for entry in log.iter_mut().rev() {
            if entry.pending {
                entry.stream = stream;
                entry.ok = ok;
                entry.status = status;
                entry.latency_ms = latency_ms;
                if !model.is_empty() { entry.model = model.to_string(); }
                entry.attempts = attempts;
                entry.prompt_tokens = usage.prompt_tokens;
                entry.completion_tokens = usage.completion_tokens;
                entry.error = error;
                entry.request_content = request_content;
                entry.response_content = response_content;
                entry.pending = false;
                break;
            }
        }
    }

    /// Like update_log_inner but also persists the completed entry to JSONL + sidecar.
    /// Called from server.rs when a real gateway request completes.
    /// For probe requests (which don't have full bodies), use update_log_inner
    /// or update_pending_for_model instead — probes are not persisted.
    pub(crate) fn complete_and_persist(
        &self,
        request_id: u64,
        group_id: &str,
        group_name: &str,
        stream: bool,
        ok: bool,
        status: Option<u16>,
        latency_ms: u64,
        model: &str,
        attempts: u32,
        usage: &upstream::Usage,
        error: Option<String>,
        request_content: Option<String>,
        response_content: Option<String>,
        raw_request: Option<String>,
        raw_response: Option<String>,
    ) {
        let mut persisted_entry: Option<PersistedLogEntry> = None;
        {
            let mut log = self.0.log.lock_ok();
            for entry in log.iter_mut().rev() {
                if entry.pending && entry.request_id == request_id {
                    entry.stream = stream;
                    entry.ok = ok;
                    entry.status = status;
                    entry.latency_ms = latency_ms;
                    if !model.is_empty() { entry.model = model.to_string(); }
                    entry.attempts = attempts;
                    entry.prompt_tokens = usage.prompt_tokens;
                    entry.completion_tokens = usage.completion_tokens;
                    entry.error = error.clone();
                    entry.request_content = request_content.clone();
                    entry.response_content = response_content.clone();
                    entry.pending = false;
                    persisted_entry = Some(to_persisted(
                        entry,
                        raw_request.as_deref(),
                        raw_response.as_deref(),
                    ));
                    break;
                }
            }
        }
        let persisted = persisted_entry.unwrap_or_else(|| PersistedLogEntry {
            id: format!("{}-{}", now_ms(), request_id),
            ts_ms: now_ms(),
            group_id: group_id.to_string(),
            group_name: group_name.to_string(),
            stream,
            ok,
            status,
            latency_ms,
            model: model.to_string(),
            attempts,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            error: error.clone(),
            request_content: request_content.as_ref().map(|s| s.chars().take(PERSISTED_CONTENT_CAP).collect()),
            response_content: response_content.as_ref().map(|s| s.chars().take(PERSISTED_CONTENT_CAP).collect()),
            pending: false,
            raw_request: raw_request.as_deref().map(|s| truncate_bytes(s, RAW_SNIPPET_CAP)),
            raw_response: raw_response.as_deref().map(|s| truncate_bytes(s, RAW_SNIPPET_CAP)),
        });
        if let Some(persistence) = &self.0.log_persistence {
            persistence.append(persisted.clone(), raw_request.as_deref(), raw_response.as_deref());
        }
        if let Some(store) = &self.0.log_stats {
            store.record(&LogEntry {
                request_id: 0,
                ts_ms: persisted.ts_ms,
                group_id: persisted.group_id.clone(),
                group_name: persisted.group_name.clone(),
                stream: persisted.stream,
                ok: persisted.ok,
                status: persisted.status,
                latency_ms: persisted.latency_ms,
                model: persisted.model.clone(),
                attempts: persisted.attempts,
                prompt_tokens: persisted.prompt_tokens,
                completion_tokens: persisted.completion_tokens,
                error: persisted.error.clone(),
                request_content: None,
                response_content: None,
                pending: false,
            });
        }
        let mut counters = self.0.counters.lock_ok();
        let c = counters.entry(persisted.group_id.clone()).or_default();
        c.total += 1;
        if persisted.ok {
            c.ok += 1;
        } else {
            c.fail += 1;
        }
        c.failovers += persisted.attempts.saturating_sub(1) as u64;
    }

    /// Query persisted log entries with filters. Returns (entries, total count).
    pub(crate) fn query_persisted(&self, q: &LogQuery) -> Option<(Vec<PersistedLogEntry>, usize)> {
        self.0.log_persistence.as_ref().map(|p| p.query(q))
    }

    pub(crate) fn get_persisted(&self, id: &str) -> Option<PersistedLogEntry> {
        self.0.log_persistence.as_ref().and_then(|p| p.get(id))
    }

    /// Check whether log persistence is available (config dir initialized).
    pub(crate) fn has_persistence(&self) -> bool {
        self.0.log_persistence.is_some()
    }

    /// 按 model 字段精确匹配 pending 条目(2026-09-14:并行探测时
    /// 每个模型有自己的 pending 条目,不能用"找最后一个 pending"逻辑)。
    pub(crate) fn update_pending_for_model(
        &self,
        model: &str,
        ok: bool,
        status: Option<u16>,
        latency_ms: u64,
        error: Option<String>,
    ) {
        let mut log = self.0.log.lock_ok();
        for entry in log.iter_mut().rev() {
            if entry.pending && entry.model == model {
                entry.ok = ok;
                entry.status = status;
                entry.latency_ms = latency_ms;
                entry.error = error;
                entry.pending = false;
                entry.attempts = 1;
                break;
            }
        }
    }

    /// 更新真实请求当前尝试的模型，供请求进行中日志即时展示。
    pub(crate) fn update_pending_model(&self, request_id: u64, model: &str) {
        if model.is_empty() {
            return;
        }
        let mut log = self.0.log.lock_ok();
        if let Some(entry) = log.iter_mut().rev().find(|e| e.pending && e.request_id == request_id) {
            entry.model = model.to_string();
        }
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

/// 应用退出时停掉网关监听(2026-09-07 用户报障:关了程序端口还被占,
/// 下次启动"端口占用")。监听线程是阻塞 accept 的普通线程,进程不退
/// 它就不退;显式置 stop 让线程自然结束。
pub fn shutdown_all(app: &AppHandle) {
    let host: GatewayHost = app.state::<GatewayHost>().inner().clone();
    let taken = host.0.server.lock_ok().take();
    if let Some(h) = taken {
        h.stop();
    }
}

/// 幂等自愈:配置 enabled 但监听没在跑(启动竞态失败/意外停止)时按
/// 当前配置重建。已在跑或未启用则原样返回。工作区拉模型列表时调用,
/// 避免用户在设置页之外无从发现"网关停了"。
pub fn ensure_running(app: &AppHandle) {
    let host: GatewayHost = app.state::<GatewayHost>().inner().clone();
    let snapshot = host.snapshot();
    if !snapshot.settings.enabled {
        return;
    }
    let up = host
        .0
        .server
        .lock_ok()
        .as_ref()
        .map(|h| h.port == snapshot.settings.port && h.is_running())
        .unwrap_or(false);
    if !up {
        reload(app);
    }
}

/// 壳启动时挂载 managed state 并按配置起服务。挂在配置加载之后。
pub fn manage(app: &AppHandle) {
    init_config_dir(
        app.path().app_config_dir().unwrap_or_else(|_| PathBuf::from(".")),
    );
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
        (true, None) => {
            // 启动失败重试 3 次(间隔 300ms):上次进程的监听线程退出
            // 与本次绑定存在竞态(退出钩子 stop 后线程最多 100ms 才真正
            // 释放端口),重试兜住"重启即端口占用"的窗口期
            let mut started = None;
            let mut last_err = String::new();
            for attempt in 0..3 {
                match server::start(host.clone(), port) {
                    Ok(h) => {
                        started = Some(h);
                        break;
                    }
                    Err(e) => {
                        last_err = e;
                        if attempt < 2 {
                            std::thread::sleep(std::time::Duration::from_millis(300));
                        }
                    }
                }
            }
            match started {
                Some(h) => Some(h),
                None => {
                    eprintln!("[desktop] 模型网关启动失败(重试 3 次): {last_err}");
                    *host.0.server_error.lock_ok() = Some(last_err);
                    None
                }
            }
        }
        (false, _) => None,
    };
    *server_slot = handle;
    *host.0.snapshot.lock_ok() = Arc::new(snapshot.clone());

    // fastest 模式需要后台延迟探测:有 fastest 组就 spawn 线程定期 ping。
    spawn_latency_probe(host.clone(), &snapshot);
}

/// 正常模型探测间隔(秒)。
const PROBE_INTERVAL_NORMAL_SECS: u64 = 30;
/// 异常模型探测间隔(秒)——给故障模型更多恢复时间。
const PROBE_INTERVAL_DEGRADED_SECS: u64 = 120;
/// 连续失败多少次后永久弃用(不再自动探测)。
use sched::ABANDON_THRESHOLD;

/// fastest 模式后台探测线程(2026-09-14):遍历所有 strategy=fastest 的组,
/// 对每个候选**并行**发 call_buffered 真实请求,记录延迟 + 写日志。
/// 正常模型 30s 探测一次,异常模型 2min 探测一次;
/// 连续失败超过 10 次(ABANDON_THRESHOLD)永久弃用,不再探测,需人工解除。
fn spawn_latency_probe(host: GatewayHost, snapshot: &RuntimeSnapshot) {
    let has_fastest = snapshot.groups.iter().any(|g| g.group.effective_strategy() == STRATEGY_FASTEST);
    if !has_fastest {
        return;
    }
    tauri::async_runtime::spawn(async move {
        // 记录每个模型下次探测时间(key = group_id/model_id)
        let mut next_probe: HashMap<String, u64> = HashMap::new();
        loop {
            let now = now_ms();
            let snap = host.snapshot();
            let mut tasks: Vec<_> = vec![];
            for rg in &snap.groups {
                if rg.group.effective_strategy() != STRATEGY_FASTEST {
                    continue;
                }
                let ctx = upstream::GroupCtx::of(&rg.group);
                let client = host.client().clone();
                let group_id = rg.group.id.clone();
                let group_name = rg.group.name.clone();
                for cand in &rg.candidates {
                    if cand.unavailable.is_some() {
                        continue;
                    }
                    let key = format!("{}/{}", rg.group.id, cand.id);
                    // 检查健康状态:abandoned 和 Open(熔断中)都跳过探测
                    let health_guard = host.health_map();
                    let h = health_guard.get(&key).copied();
                    drop(health_guard);
                    if h.map(|h| h.abandoned).unwrap_or(false) {
                        continue; // 永久弃用,跳过
                    }
                    // 熔断中(Open)的模型也跳过:避免探测失败继续累计,
                    // 导致从 3 次很快到 10 次永久弃用
                    if h.map(|h| !h.is_available(now)).unwrap_or(false) {
                        continue; // 熔断中,跳过
                    }
                    // 检查是否到了探测时间
                    let next = next_probe.get(&key).copied().unwrap_or(0);
                    if now < next {
                        continue; // 还没到探测时间
                    }
                    let host = host.clone();
                    let cand = cand.clone();
                    let ctx = ctx.clone();
                    let client = client.clone();
                    let group_id = group_id.clone();
                    let group_name = group_name.clone();
                    let key_clone = key.clone();
                    tasks.push(async move {
                        let body = serde_json::json!({
                            "model": &cand.model,
                            "messages": [{ "role": "user", "content": "ping" }],
                            "max_tokens": 1,
                        });
                        let timeout = std::time::Duration::from_secs(15);
                        let started = std::time::Instant::now();
                        if rg.group.log_enabled {
                            host.push_log(LogEntry {
                                request_id: 0,
                                ts_ms: now_ms(),
                                group_id: group_id.clone(),
                                group_name: group_name.clone(),
                                stream: false, ok: false, status: None,
                                latency_ms: 0, model: cand.model.clone(),
                                attempts: 0, prompt_tokens: None,
                                completion_tokens: None, error: None,
                                request_content: Some("[fastest 探测]".to_string()),
                                response_content: None, pending: true,
                            });
                        }
                        let result = tokio::time::timeout(timeout, {
                            let client = client.clone();
                            let cand = cand.clone();
                            let body = body.clone();
                            let ctx = ctx.clone();
                            async move { upstream::call_buffered(&client, &cand, &body, &ctx, timeout).await }
                        }).await;
                        let elapsed = started.elapsed().as_millis() as u64;
                        let is_fail;
                        match result {
                            Err(_) => {
                                is_fail = true;
                                host.update_pending_for_model(&cand.model, false, Some(408), elapsed,
                                    Some(format!("fastest 探测超时")));
                                // 探测只更新延迟,不写 health(避免与实际请求互相干扰)
                                host.record_probe_result(&group_id, &cand.id, false, u64::MAX);
                            }
                            Ok(Ok(_)) => {
                                is_fail = false;
                                host.update_pending_for_model(&cand.model, true, Some(200), elapsed, None);
                                host.record_probe_result(&group_id, &cand.id, true, elapsed);
                            }
                            Ok(Err(e)) => {
                                is_fail = true;
                                let status = e.status().unwrap_or(502);
                                host.update_pending_for_model(&cand.model, false, Some(status), elapsed,
                                    Some(e.message()));
                                host.record_probe_result(&group_id, &cand.id, false, u64::MAX);
                            }
                        }
                        // 返回 (key, is_fail) 供主循环设置下次探测时间
                        (key_clone, is_fail)
                    });
                }
            }
            if !tasks.is_empty() {
                let results = futures_util::future::join_all(tasks).await;
                // 根据探测结果设置下次探测时间:
                // 成功 → 30s 后;失败 → 120s 后
                for (key, is_fail) in results {
                    let interval = if is_fail { PROBE_INTERVAL_DEGRADED_SECS } else { PROBE_INTERVAL_NORMAL_SECS };
                    next_probe.insert(key, now_ms() + interval * 1000);
                }
            } else {
                // 没有任务(全弃用或未到时间)→ 等 5s 再检查
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
            // 短暂睡眠后进入下一轮检查(可能有模型到了探测时间)
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}

/// 对单个候选发一次最小 ping,返回延迟(毫秒)。失败返 u64::MAX。
fn probe_one(client: &reqwest::Client, cand: &ResolvedCandidate) -> u64 {
    let body = serde_json::json!({
        "model": cand.model,
        "messages": [{ "role": "user", "content": "hi" }],
        "max_tokens": 1,
    });
    let started = std::time::Instant::now();
    let result = tauri::async_runtime::block_on(async {
        client
            .post(format!("{}/v1/chat/completions", cand.base_url.trim_end_matches('/')))
            .bearer_auth(&cand.api_key)
            .json(&body)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
    });
    match result {
        Ok(resp) => {
            if resp.status().is_success() {
                started.elapsed().as_millis() as u64
            } else {
                u64::MAX
            }
        }
        Err(_) => u64::MAX,
    }
}

/// async 版 ping 探测(供 gateway_probe_group 并行调用)。
/// 用 tokio::time::timeout 包裹整个请求(send + 读 body),确保超时精确生效——
/// reqwest 的 .timeout() 只管 connect+send,不含读 body;且 Client 构建时的
/// 全局 timeout 可能覆盖单请求值。tokio::time::timeout 是硬超时,到点即返回 Err。
async fn probe_one_async(client: &reqwest::Client, cand: &ResolvedCandidate, timeout_ms: u64) -> (String, u64) {
    let body = serde_json::json!({
        "model": cand.model,
        "messages": [{ "role": "user", "content": "hi" }],
        "max_tokens": 1,
    });
    let started = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms.max(500));
    let result = tokio::time::timeout(timeout, async {
        client
            .post(format!("{}/v1/chat/completions", cand.base_url.trim_end_matches('/')))
            .bearer_auth(&cand.api_key)
            .json(&body)
            .send()
            .await
    })
    .await;
    let latency = match result {
        // tokio::time::timeout 超时 → Elapsed error → u64::MAX
        Err(_) => u64::MAX,
        // 请求成功发出,但 HTTP 返回了错误 → u64::MAX
        Ok(Err(_)) => u64::MAX,
        // 请求成功且 HTTP 200 → 记录延迟
        Ok(Ok(resp)) if resp.status().is_success() => started.elapsed().as_millis() as u64,
        // HTTP 非 200 → u64::MAX
        Ok(Ok(_)) => u64::MAX,
    };
    (cand.id.clone(), latency)
}

/// 逐模型探测延迟(2026-09-13):对组内每个候选**并行**发一次真实请求,
/// 用 call_buffered(正确的 URL/协议/header),手动写日志。
/// 每个模型用 tokio::time::timeout 硬超时,超时标记为 null(异常 ✕)。
#[tauri::command]
pub async fn gateway_probe_group(app: AppHandle, id: String, timeout_ms: Option<u64>) -> Result<serde_json::Value, String> {
    let host = app.state::<GatewayHost>().inner().clone();
    let snapshot = host.snapshot();
    let group = snapshot
        .group_by_id(&id)
        .ok_or_else(|| format!("模型组不存在: {id}"))?
        .clone();
    let probe_timeout_ms = timeout_ms.unwrap_or(15000);
    let probe_timeout = std::time::Duration::from_millis(probe_timeout_ms.max(500));
    let ctx = upstream::GroupCtx::of(&group.group);
    let client = host.client().clone();
    // 并行探测:每个候选独立 call_buffered + 独立日志
    let tasks: Vec<_> = group.candidates.iter().map(|cand| {
        let host = host.clone();
        let cand = cand.clone();
        let client = client.clone();
        let ctx = ctx.clone();
        let group_id = group.group.id.clone();
        let group_name = group.group.name.clone();
        async move {
            if cand.unavailable.is_some() {
                return (cand.id.clone(), None);
            }
            let body = serde_json::json!({
                "model": &cand.model,
                "messages": [{ "role": "user", "content": "hi" }],
                "max_tokens": 1,
            });
            let started = std::time::Instant::now();
            // 写 pending 日志(log_enabled=false 时跳过)
            if group.group.log_enabled {
                host.push_log(LogEntry {
                    request_id: 0,
                    ts_ms: now_ms(),
                group_id: group_id.clone(),
                group_name: group_name.clone(),
                stream: false,
                ok: false,
                status: None,
                latency_ms: 0,
                model: cand.model.clone(),
                attempts: 0,
                prompt_tokens: None,
                completion_tokens: None,
                error: None,
                request_content: Some("hi".to_string()),
                response_content: None,
                pending: true,
            });
            }
            // 用 tokio::time::timeout 硬超时包裹 call_buffered
            let result = tokio::time::timeout(
                probe_timeout,
                upstream::call_buffered(&client, &cand, &body, &ctx, probe_timeout),
            ).await;
            let elapsed = started.elapsed().as_millis() as u64;
            match result {
                // tokio 超时
                Err(_) => {
                    host.update_pending_for_model(
                        &cand.model, false, Some(408), elapsed,
                        Some(format!("探测超时({probe_timeout_ms}ms)")),
                    );
                    host.record_latency(&group_id, &cand.id, u64::MAX);
                    host.record_attempt(&group_id, &cand.id, false);
                    (cand.id.clone(), Some(u64::MAX))
                }
                Ok(Ok(_reply)) => {
                    host.update_pending_for_model(
                        &cand.model, true, Some(200), elapsed,
                        None,
                    );
                    host.record_latency(&group_id, &cand.id, elapsed);
                    host.record_attempt(&group_id, &cand.id, true);
                    (cand.id.clone(), Some(elapsed))
                }
                Ok(Err(e)) => {
                    let status = e.status().unwrap_or(502);
                    host.update_pending_for_model(
                        &cand.model, false, Some(status), elapsed,
                        Some(e.message()),
                    );
                    host.record_latency(&group_id, &cand.id, u64::MAX);
                    host.record_attempt(&group_id, &cand.id, false);
                    (cand.id.clone(), Some(u64::MAX))
                }
            }
        }
    }).collect();
    let results = futures_util::future::join_all(tasks).await;
    let arr: Vec<serde_json::Value> = results.iter().map(|(cid, ms)| {
        serde_json::json!({
            "id": cid,
            "latency_ms": match ms { Some(v) if *v != u64::MAX => v.to_string().into(), _ => serde_json::Value::Null },
        })
    }).collect();
    Ok(serde_json::json!({ "id": id, "models": arr }))
}

/// 网关配置变更后同步物化引擎模型条目(组名 → 引擎 settings.models):
/// 引擎在跑时 settings 不随 config.json 自动刷新,不物化则工作区切到
/// 新组报「未知模型」。失败仅记日志——网关自身照常工作,模型条目下次
/// 保存设置/重启时补齐。
fn rematerialize_engine_models(app: &AppHandle) {
    let Ok(cfg) = crate::config::load_config(app) else { return };
    if let Err(e) = crate::config::materialize_engine_config(app, &cfg, crate::browser::mcp_endpoint(app)) {
        eprintln!("[desktop] 网关变更后物化引擎配置失败: {e}");
    }
}

// ==================== IPC 命令 ====================

fn status_payload(host: &GatewayHost) -> serde_json::Value {
    let snapshot = host.snapshot();
    let health = host.0.health.lock_ok();
    let counters = host.0.counters.lock_ok();
    let lats = host.0.latencies.lock_ok();
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
                    let latency = lats.get(&key).copied().filter(|&v| v != u64::MAX);
                    serde_json::json!({
                        "id": m.id, "enabled": m.enabled, "weight": m.weight, "alias": m.alias,
                        "provider": m.provider, "base_url": m.base_url, "api_key": m.api_key, "model": m.model,
                        "label": cand.map(|c| c.label.clone()).unwrap_or_else(|| m.alias.clone()),
                        "upstream_model": cand.map(|c| c.model.clone()).unwrap_or_default(),
                        "unavailable": cand.and_then(|c| c.unavailable.clone()),
                        "health": state.as_str(),
                        "latency_ms": latency,
                    })
                })
                .collect();
            serde_json::json!({
                "id": rg.group.id, "name": rg.group.name, "enabled": rg.group.enabled,
                "key": rg.group.key, "strategy": rg.group.effective_strategy(),
                "context_window": rg.group.effective_context_window(),
                "max_output": rg.group.effective_max_output(),
                "temperature": rg.group.temperature, "system_prompt": rg.group.system_prompt,
                "timeout_seconds": rg.group.timeout_seconds, "log_enabled": rg.group.log_enabled,
                "models": models,
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

fn log_query(
    group_id: Option<String>,
    model: Option<String>,
    ok: Option<bool>,
    search: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> LogQuery {
    LogQuery {
        group_id: non_empty_filter(group_id),
        model: non_empty_filter(model),
        ok,
        search: non_empty_filter(search),
        limit: limit.map(|n| n as usize).unwrap_or(50),
        offset: offset.map(|n| n as usize).unwrap_or(0),
    }
}

fn memory_matches(entry: &LogEntry, q: &LogQuery) -> bool {
    if let Some(ref gid) = q.group_id {
        if entry.group_id != *gid {
            return false;
        }
    }
    if let Some(ref model) = q.model {
        if !entry.model.to_lowercase().contains(&model.to_lowercase()) {
            return false;
        }
    }
    if let Some(ok) = q.ok {
        if entry.ok != ok {
            return false;
        }
    }
    if let Some(ref search) = q.search {
        let needle = search.to_lowercase();
        let hay = [
            entry.group_name.as_str(),
            entry.model.as_str(),
            entry.error.as_deref().unwrap_or(""),
            entry.request_content.as_deref().unwrap_or(""),
            entry.response_content.as_deref().unwrap_or(""),
        ];
        if !hay.iter().any(|s| s.to_lowercase().contains(&needle)) {
            return false;
        }
    }
    true
}

fn strip_raw_for_list(mut entry: PersistedLogEntry) -> PersistedLogEntry {
    entry.raw_request = None;
    entry.raw_response = None;
    entry
}

fn pending_log_values(host: &GatewayHost, q: &LogQuery) -> Vec<serde_json::Value> {
    if q.ok.is_some() {
        return vec![];
    }
    let log = host.0.log.lock_ok();
    log.iter()
        .rev()
        .filter(|e| e.pending && memory_matches(e, q))
        .filter_map(|e| serde_json::to_value(e).ok())
        .collect()
}

/// 最近请求日志:默认读磁盘(重启不丢)。offset=0 时把内存里的 pending 插到最前。
/// 列表不带完整 body,点详情走 gateway_log_detail。
#[tauri::command]
pub fn gateway_log(
    app: AppHandle,
    limit: Option<u32>,
    group_id: Option<String>,
    model: Option<String>,
    ok: Option<bool>,
    search: Option<String>,
    offset: Option<u32>,
) -> Result<Vec<serde_json::Value>, String> {
    let host = app.state::<GatewayHost>();
    let q = log_query(group_id, model, ok, search, limit, offset);
    if let Some((entries, _)) = host.query_persisted(&q) {
        let mut out: Vec<serde_json::Value> = entries
            .into_iter()
            .filter_map(|e| serde_json::to_value(strip_raw_for_list(e)).ok())
            .collect();
        if q.offset == 0 {
            let mut pending = pending_log_values(&host, &q);
            pending.append(&mut out);
            out = pending;
        }
        return Ok(out);
    }
    let log = host.0.log.lock_ok();
    let matched: Vec<&LogEntry> = log.iter().rev().filter(|e| memory_matches(e, &q)).collect();
    let start = q.offset.min(matched.len());
    let end = start.saturating_add(q.limit).min(matched.len());
    Ok(matched[start..end]
        .iter()
        .filter_map(|e| serde_json::to_value(*e).ok())
        .collect())
}

/// Total count of persisted log entries matching filters (for pagination).
#[tauri::command]
pub fn gateway_log_count(
    app: AppHandle,
    group_id: Option<String>,
    model: Option<String>,
    ok: Option<bool>,
    search: Option<String>,
) -> Result<u64, String> {
    let host = app.state::<GatewayHost>();
    let q = log_query(group_id, model, ok, search, Some(0), Some(0));
    if let Some((_, total)) = host.query_persisted(&q) {
        let extra = if q.ok.is_none() {
            pending_log_values(&host, &q).len()
        } else {
            0
        };
        return Ok((total + extra) as u64);
    }
    let log = host.0.log.lock_ok();
    Ok(log.iter().filter(|e| memory_matches(e, &q)).count() as u64)
}

/// 单条请求的完整请求/响应体(sidecar,最多 512KB)。
#[tauri::command]
pub fn gateway_log_detail(app: AppHandle, id: String) -> Result<serde_json::Value, String> {
    let host = app.state::<GatewayHost>();
    host.get_persisted(&id)
        .and_then(|e| serde_json::to_value(e).ok())
        .ok_or_else(|| "记录不存在".into())
}

fn non_empty_filter(value: Option<String>) -> Option<String> {
    value.and_then(|s| if s.trim().is_empty() { None } else { Some(s) })
}

/// 跨会话调用统计(2026-09-12 需求):按模型分类 + 范围(today/day7/all)聚合
/// tokens/调用数/总时长,并附带全量热力图。数据源是 `GatewayLogStore`
/// (push_log 记账时的聚合落盘),不是内存环形日志。
#[tauri::command]
pub fn gateway_log_stats(app: AppHandle, range: Option<String>) -> Result<GatewayLogStats, String> {
    let host = app.state::<GatewayHost>();
    let kind = match range.as_deref() {
        Some(s) => parse_range_kind(s),
        None => GatewayRangeKind::default(),
    };
    // 单测路径(manage 未初始化)log_stats=None → 返回空态,不报错
    Ok(match &host.0.log_stats {
        Some(store) => store.query(kind),
        None => GatewayLogStats { range: kind, ..Default::default() },
    })
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
        // 组变更要同步进引擎 settings(write_ohmyagent_config 把启用的组物化为
        // 引擎模型条目),否则工作区模型菜单能看到(实时读 config.json)但
        // 切换报「未知模型」
        rematerialize_engine_models(&app);
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

/// 幂等自愈:enabled 但没在跑时按当前配置重建(工作区拉模型时调用)。
#[tauri::command]
pub async fn gateway_ensure_running(app: AppHandle) -> Result<(), String> {
    ensure_running(&app);
    Ok(())
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
        rematerialize_engine_models(&app);
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
        rematerialize_engine_models(&app);
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
        rematerialize_engine_models(&app);
        reload(&app);
        Ok(key)
    })
    .await
    .map_err(|e| format!("重置失败: {e}"))?
}

/// 组连通性测试:走**真实调度链路**(含故障切换与熔断),发一条最小对话,
/// 报告最终由哪个模型应答、耗时与失败摘要。
#[tauri::command]
pub async fn gateway_test_group(app: AppHandle, id: String, timeout_ms: Option<u64>) -> Result<serde_json::Value, String> {
    let host = app.state::<GatewayHost>().inner().clone();
    let snapshot = host.snapshot();
    let mut group = snapshot
        .group_by_id(&id)
        .ok_or_else(|| format!("模型组不存在: {id}"))?
        .clone();
    let started = std::time::Instant::now();
    let body = serde_json::json!({
        "model": id,
        "messages": [{ "role": "user", "content": "ping" }],
        "max_tokens": 16,
    });
    // 弹窗设的超时(2026-09-14):覆盖组级 timeout,使 run_buffered 内部
    // 每个模型的上游调用都用此超时——模型1超时则切换试模型2,
    // 不是整组总超时。
    if let Some(ms) = timeout_ms {
        if ms > 0 {
            // 把毫秒转秒(向上取整,至少 1 秒)
            group.group.timeout_seconds = ((ms + 999) / 1000) as u64;
        }
    }
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

// ==================== 厂商预设 CRUD(2026-09-14) ====================

/// 保存厂商预设列表(全量替换)。
#[tauri::command]
pub fn gateway_save_vendors(app: AppHandle, vendors: Vec<VendorPreset>) -> Result<Vec<VendorPreset>, String> {
    // 校验
    let mut seen = std::collections::HashSet::new();
    for v in &vendors {
        let name = v.name.trim();
        if name.is_empty() {
            return Err("厂商预设名称不能为空".to_string());
        }
        if !seen.insert(name.to_lowercase()) {
            return Err(format!("厂商预设名称重复: {name}"));
        }
    }
    let mut vendors = vendors;
    // 确保 id 非空(新建时 UI 传空 id,此处补 uuid)
    for v in &mut vendors {
        if v.id.is_empty() {
            v.id = crate::util::short_uuid();
        }
    }
    crate::config::update_config_json(&app, |cfg| {
        cfg.gateway.vendor_presets = vendors.clone();
    })?;
    Ok(vendors)
}

/// 人工解除模型弃用状态(2026-09-15):连续失败超阈值被永久弃用后,
/// 用户确认问题已修复可手动解除,模型恢复可用并重新探测。
#[tauri::command]
pub fn gateway_reset_model_health(app: AppHandle, group_id: String, model_id: String) -> Result<(), String> {
    let host = app.state::<GatewayHost>();
    host.reset_model_health(&group_id, &model_id);
    Ok(())
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
            log_enabled: true,
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
            gateway: GatewaySettings { enabled: true, port: 1000, groups: vec![g], vendor_presets: vec![] },
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
            gateway: GatewaySettings { enabled: true, port: 1000, groups: vec![g2], vendor_presets: vec![] },
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

    fn sample_persisted(id: &str, group_id: &str, group_name: &str, model: &str, ok: bool, ts_ms: u64) -> PersistedLogEntry {
        PersistedLogEntry {
            id: id.into(),
            ts_ms,
            group_id: group_id.into(),
            group_name: group_name.into(),
            stream: false,
            ok,
            status: Some(if ok { 200 } else { 502 }),
            latency_ms: 10,
            model: model.into(),
            attempts: 1,
            prompt_tokens: Some(1),
            completion_tokens: Some(2),
            error: None,
            request_content: Some("hello user".into()),
            response_content: Some("hi".into()),
            pending: false,
            raw_request: None,
            raw_response: None,
        }
    }

    #[test]
    fn persisted_logs_query_without_filters_and_load_full_body() {
        let dir = std::env::temp_dir().join(format!("mc-gw-log-{}-{}", std::process::id(), now_ms()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = GatewayLogPersistence::new(&dir);
        p.append(
            sample_persisted("100-1", "mg-free", "免费组", "m1", true, 100),
            Some(r#"{"messages":[{"role":"user","content":"ping"}]}"#),
            Some(r#"{"id":"x"}"#),
        );
        p.append(
            sample_persisted("200-2", "mg-nebula", "星云", "m2", false, 200),
            Some("req-b"),
            Some("data: {\"delta\":1}\ndata: [DONE]"),
        );
        let (rows, total) = p.query(&LogQuery { limit: 50, ..Default::default() });
        assert_eq!(total, 2, "无筛选也应读出磁盘记录");
        assert_eq!(rows[0].group_name, "星云");
        assert_eq!(rows[1].group_name, "免费组");

        let (rows, total) = p.query(&LogQuery {
            group_id: Some("mg-free".into()),
            limit: 50,
            ..Default::default()
        });
        assert_eq!(total, 1);
        assert_eq!(rows[0].model, "m1");

        let (rows, total) = p.query(&LogQuery {
            search: Some("ping".into()),
            limit: 50,
            ..Default::default()
        });
        assert_eq!(total, 1);
        assert_eq!(rows[0].group_name, "免费组");

        let (rows, _) = p.query(&LogQuery {
            search: Some("免费".into()),
            limit: 50,
            ..Default::default()
        });
        assert_eq!(rows[0].group_name, "免费组", "搜索应匹配组名");

        let (rows, total) = p.query(&LogQuery {
            ok: Some(false),
            limit: 50,
            ..Default::default()
        });
        assert_eq!(total, 1);
        assert_eq!(rows[0].group_name, "星云");

        let detail = p.get("100-1").expect("详情应按 id 命中");
        assert!(detail.raw_request.unwrap().contains("ping"));

        let big = "x".repeat(20_000);
        p.append(
            sample_persisted("300-3", "mg-big", "大包", "m3", true, 300),
            Some(&format!(r#"{{"k":"{big}"}}"#)),
            None,
        );
        let detail = p.get("300-3").unwrap();
        assert!(detail.raw_request.unwrap().contains(&big), "sidecar 应保留完整 body");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
