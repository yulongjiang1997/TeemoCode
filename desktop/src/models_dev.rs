// models.dev 模型目录集成(2026-09-16)
//
// 从 https://models.dev/api.json 拉取全量模型目录,三级缓存:
//   内存 → 磁盘(config_dir/models-dev-cache.json) → 网络(24h TTL)
//
// 用途:厂商导入模型时自动填充 context_window / max_output / vision / think,
// 无需用户手填。未命中的模型回退到厂商预设,再回退到默认值。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE_URL: &str = "https://models.dev/api.json";
const CACHE_TTL_SECS: u64 = 24 * 3600; // 24 小时

/// 单个模型的目录条目(从 models.dev 提取的字段)。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ModelDevEntry {
    /// 上下文窗口(token);0 = 未知。
    pub context_window: u64,
    /// 最大输出(token);0 = 未知。
    pub max_output: u64,
    /// 是否支持图片输入(vision)。
    pub vision: bool,
    /// 思考档位列表(如 ["low","medium","high"]);空 = 未知/不支持。
    pub reasoning_effort_values: Vec<String>,
}

/// 磁盘缓存格式。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct CacheFile {
    /// Unix 时间戳(秒)。
    fetched_at: u64,
    /// provider key → { model_id → ModelDevEntry }
    providers: HashMap<String, HashMap<String, ModelDevEntry>>,
}

/// 全局内存缓存。
static CACHE: Mutex<Option<CacheFile>> = Mutex::new(None);

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 缓存文件路径(config_dir/models-dev-cache.json)。
fn cache_path(config_dir: &PathBuf) -> PathBuf {
    config_dir.join("models-dev-cache.json")
}

/// 从 JSON 解析 models.dev 数据。
fn parse_registry(data: &serde_json::Value) -> CacheFile {
    let mut providers: HashMap<String, HashMap<String, ModelDevEntry>> = HashMap::new();
    if let Some(obj) = data.as_object() {
        for (prov_key, prov_val) in obj {
            if let Some(models) = prov_val.get("models").and_then(|m| m.as_object()) {
                let mut model_map: HashMap<String, ModelDevEntry> = HashMap::new();
                for (model_id, model_val) in models {
                    let entry = ModelDevEntry {
                        context_window: model_val
                            .get("limit")
                            .and_then(|l| l.get("context"))
                            .and_then(|c| c.as_u64())
                            .unwrap_or(0),
                        max_output: model_val
                            .get("limit")
                            .and_then(|l| l.get("output"))
                            .and_then(|o| o.as_u64())
                            .unwrap_or(0),
                        vision: model_val
                            .get("modalities")
                            .and_then(|m| m.get("input"))
                            .and_then(|i| i.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .any(|v| v.as_str() == Some("image"))
                            })
                            .unwrap_or(false),
                        reasoning_effort_values: model_val
                            .get("reasoning_options")
                            .and_then(|ro| ro.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|opt| {
                                        if opt.get("type").and_then(|t| t.as_str()) == Some("effort") {
                                            opt.get("values")
                                                .and_then(|v| v.as_array())
                                                .map(|vals| {
                                                    vals.iter()
                                                        .filter_map(|v| v.as_str().map(String::from))
                                                        .collect::<Vec<String>>()
                                                })
                                        } else {
                                            None
                                        }
                                    })
                                    .flatten()
                                    .collect::<Vec<String>>()
                            })
                            .unwrap_or_default(),
                    };
                    model_map.insert(model_id.clone(), entry);
                }
                providers.insert(prov_key.clone(), model_map);
            }
        }
    }
    CacheFile {
        fetched_at: now_secs(),
        providers,
    }
}

/// 异步加载缓存(三级:内存 → 磁盘 → 网络)。
pub async fn load_cache(config_dir: &PathBuf) -> Result<CacheFile, String> {
    // 1. 内存缓存(未过期)
    {
        let guard = CACHE.lock().map_err(|e| format!("锁失败: {e}"))?;
        if let Some(ref cached) = *guard {
            if now_secs().saturating_sub(cached.fetched_at) < CACHE_TTL_SECS {
                return Ok(cached.clone());
            }
        }
    }

    // 2. 磁盘缓存
    let path = cache_path(config_dir);
    if path.exists() {
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(disk) = serde_json::from_str::<CacheFile>(&data) {
                if let Ok(mut guard) = CACHE.lock() {
                    *guard = Some(disk.clone());
                }
                if now_secs().saturating_sub(disk.fetched_at) < CACHE_TTL_SECS {
                    return Ok(disk);
                }
                // 过期:后台刷新,先返回旧数据
                let dir = config_dir.clone();
                tokio::spawn(async move {
                    let _ = refresh_from_network(&dir).await;
                });
                return Ok(disk);
            }
        }
    }

    // 3. 网络(首次或磁盘无缓存时)
    refresh_from_network(config_dir).await
}

/// 从网络拉取并写入缓存。
async fn refresh_from_network(config_dir: &PathBuf) -> Result<CacheFile, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let resp = client
        .get(SOURCE_URL)
        .send()
        .await
        .map_err(|e| format!("请求 models.dev 失败: {e}"))?;
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 models.dev 响应失败: {e}"))?;
    let parsed = parse_registry(&body);

    // 写磁盘
    if let Ok(json) = serde_json::to_string(&parsed) {
        let _ = std::fs::write(cache_path(config_dir), json);
    }
    // 写内存
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(parsed.clone());
    }
    Ok(parsed)
}

/// 按 base_url 匹配 provider key。
fn match_provider_key(cache: &CacheFile, base_url: &str) -> Option<String> {
    let normalized = base_url.trim().trim_end_matches('/').trim_end_matches("/v1");
    let input_host = extract_host(normalized)?;

    let key_by_host: &[(&str, &str)] = &[
        ("api.openai.com", "openai"),
        ("api.anthropic.com", "anthropic"),
        ("generativelanguage.googleapis.com", "google"),
        ("api.deepseek.com", "deepseek"),
        ("api.moonshot.cn", "moonshot"),
        ("api.moonshot.ai", "moonshot"),
        ("open.bigmodel.cn", "zhipuai"),
        ("api.siliconflow.cn", "siliconflow"),
        ("api.minimaxi.com", "minimax"),
        ("api.minimax.chat", "minimax"),
        ("api.together.xyz", "together"),
        ("api.groq.com", "groq"),
        ("api.mistral.ai", "mistral"),
        ("api.x.ai", "xai"),
        ("api.openrouter.ai", "openrouter"),
        ("api.cloudflare.com", "cloudflare"),
    ];

    for (host, key) in key_by_host {
        if input_host == *host && cache.providers.contains_key(*key) {
            return Some(key.to_string());
        }
    }

    // 模糊匹配:provider key 包含 hostname 的核心词
    let host_core = input_host.strip_prefix("www.").unwrap_or(input_host);
    let host_part = host_core.split('.').next().unwrap_or("");
    if !host_part.is_empty() {
        for key in cache.providers.keys() {
            if key.contains(host_part) {
                return Some(key.clone());
            }
        }
    }

    None
}

fn extract_host(url: &str) -> Option<&str> {
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    stripped.split('/').next()
}

/// 查找单个模型在 models.dev 中的参数。
pub fn lookup_model(cache: &CacheFile, base_url: &str, model_id: &str) -> Option<ModelDevEntry> {
    // Phase 1: 按 base_url 匹配 provider
    if let Some(prov_key) = match_provider_key(cache, base_url) {
        if let Some(prov_models) = cache.providers.get(&prov_key) {
            if let Some(entry) = prov_models.get(model_id) {
                return Some(entry.clone());
            }
        }
    }

    // Phase 2: 全局扫描,优先取有 reasoning_effort_values 的条目
    let mut best: Option<ModelDevEntry> = None;
    let mut fallback: Option<ModelDevEntry> = None;
    for (_, models) in &cache.providers {
        if let Some(entry) = models.get(model_id) {
            if !entry.reasoning_effort_values.is_empty() {
                best = Some(entry.clone());
                break;
            }
            if fallback.is_none() {
                fallback = Some(entry.clone());
            }
        }
    }
    best.or(fallback)
}

/// 批量查找模型参数(返回 JSON,供 IPC 命令使用)。
pub async fn enrich_models(
    config_dir: &PathBuf,
    base_url: &str,
    model_ids: &[String],
) -> Result<serde_json::Value, String> {
    let cache = load_cache(config_dir).await?;
    let mut result = serde_json::Map::new();
    for id in model_ids {
        if let Some(entry) = lookup_model(&cache, base_url, id) {
            result.insert(
                id.clone(),
                serde_json::json!({
                    "context_window": entry.context_window,
                    "max_output": entry.max_output,
                    "vision": entry.vision,
                    "reasoning_effort_values": entry.reasoning_effort_values,
                }),
            );
        }
    }
    Ok(serde_json::Value::Object(result))
}
