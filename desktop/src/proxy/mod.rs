// 内置代理(2026-10-30):app 自带 mihomo(Clash Meta)内核,用户填入
// 订阅链接后自动下载内核、拉取订阅、启动本地代理服务。网关的 reqwest
// client 自动走此代理,无需额外配置 Clash Verge 等外部工具。
//
// 架构:
//   ProxyManager(managed state)
//     ├─ kernel_path: 已下载的 mihomo 二进制路径(首次启动时自动下载)
//     ├─ subscription_url: 用户填入的订阅链接
//     ├─ child: Option<tokio::process::Child>  运行中的 mihomo 子进程
//     └─ api_port: mihomo 的 RESTful API 端口(查代理列表/切换节点)
//
//   mihomo 启动参数:
//     mihomo -d <data_dir> -f <config_path>
//   data_dir 存放 GeoIP/Country.mmdb 等数据文件(内核自动下载)
//   config_path 是从订阅拉取的 YAML(首次拉取后本地缓存)
//
//   代理端口:从订阅配置中读取 mixed-port/http-port,若无则用默认 7890
//   API 端口:固定 9090(mihomo 的 external-controller)
//
//   网关接入:GatewayHost 重建 reqwest client 时,若代理在运行则
//   .proxy(reqwest::Proxy::all("http://127.0.0.1:7890"))

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::config::{config_dir, load_config};

/// mihomo 内核版本(下载 URL 中的版本号)
const MIHOMO_VERSION: &str = "1.18.10";
/// 代理监听端口(订阅配置中 mixed-port 优先,否则用此默认值)
const DEFAULT_PROXY_PORT: u16 = 7897;
/// mihomo RESTful API 端口(查代理列表/切换节点)
const MIHOMO_API_PORT: u16 = 12090;
/// mihomo API secret(简单防误访)
const MIHOMO_API_SECRET: &str = "mc-proxy";

/// 下载 URL(mihomo releases):
/// Windows: mihomo-windows-amd64-v{ver}.zip
/// macOS:   mihomo-darwin-amd64-v{ver}.zip 或 arm64
fn mihomo_download_url() -> String {
    let ext = if cfg!(target_os = "windows") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    };
    let suffix = if cfg!(target_os = "windows") {
        ".zip"
    } else {
        ".gz"
    };
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    format!(
        "https://github.com/MetaCubeX/mihomo/releases/download/v{MIHOMO_VERSION}/mihomo-{os}-{ext}-v{MIHOMO_VERSION}{suffix}"
    )
}

/// 用户填入的订阅链接(存 config.json)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// 订阅链接(空 = 未配置)
    #[serde(default)]
    pub subscription_url: String,
    /// 是否启用代理
    #[serde(default)]
    pub enabled: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            subscription_url: String::new(),
            enabled: false,
        }
    }
}

/// mihomo 运行状态(前端展示)。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProxyStatus {
    pub running: bool,
    pub enabled: bool,
    /// 代理端口(网关流量打这里)
    pub proxy_port: u16,
    /// mihomo API 端口
    pub api_port: u16,
    /// 内核是否已下载
    pub kernel_ready: bool,
    /// 订阅链接(已配置)
    pub has_subscription: bool,
    /// 错误信息(启动失败等)
    pub error: Option<String>,
}

/// mihomo API:代理组
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProxyGroup {
    pub name: String,
    pub r#type: String,
    pub now: String,
    pub options: Vec<String>,
}

/// mihomo API:代理列表
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProxyListResponse {
    pub proxies: std::collections::HashMap<String, ProxyGroup>,
}

struct ProxyInner {
    /// 运行中的 mihomo 子进程
    child: Option<tokio::process::Child>,
    /// 当前代理端口(从订阅配置解析或默认值)
    proxy_port: u16,
    /// 错误信息
    error: Option<String>,
    /// 内核是否已下载
    kernel_ready: bool,
}

#[derive(Clone)]
pub struct ProxyManager(pub Arc<ProxyInner_>);

pub struct ProxyInner_ {
    inner: Mutex<ProxyInner>,
    app: AppHandle,
}

impl ProxyManager {
    pub fn new(app: AppHandle) -> Self {
        Self(Arc::new(ProxyInner_ {
            inner: Mutex::new(ProxyInner {
                child: None,
                proxy_port: DEFAULT_PROXY_PORT,
                error: None,
                kernel_ready: false,
            }),
            app,
        }))
    }

    /// proxy 工作目录(内核二进制 + 配置 + 数据)
    fn data_dir(app: &AppHandle) -> PathBuf {
        let mut p = config_dir(app).unwrap_or_else(|_| PathBuf::from("."));
        p.push("proxy");
        let _ = std::fs::create_dir_all(&p);
        p
    }

    /// mihomo 二进制路径
    fn kernel_path(app: &AppHandle) -> PathBuf {
        let dir = Self::data_dir(app);
        let name = if cfg!(target_os = "windows") {
            "mihomo.exe"
        } else {
            "mihomo"
        };
        dir.join(name)
    }

    /// mihomo 配置文件路径
    fn config_path(app: &AppHandle) -> PathBuf {
        Self::data_dir(app).join("config.yaml")
    }

    /// 从 config.json 读取代理配置
    fn read_config(app: &AppHandle) -> ProxyConfig {
        match load_config(app) {
            Ok(cfg) => cfg.gateway.proxy.clone(),
            Err(_) => ProxyConfig::default(),
        }
    }

    /// 当前代理 URL(网关重建 client 时用)
    pub fn proxy_url(&self) -> Option<String> {
        let inner = self.0.inner.lock();
        if inner.child.is_some() {
            Some(format!("http://127.0.0.1:{}", inner.proxy_port))
        } else {
            None
        }
    }

    /// 查询当前状态
    pub fn status(&self) -> ProxyStatus {
        let inner = self.0.inner.lock();
        let cfg = Self::read_config(&self.0.app);
        ProxyStatus {
            running: inner.child.is_some(),
            enabled: cfg.enabled,
            proxy_port: inner.proxy_port,
            api_port: MIHOMO_API_PORT,
            kernel_ready: inner.kernel_ready,
            has_subscription: !cfg.subscription_url.is_empty(),
            error: inner.error.clone(),
        }
    }

    /// 下载 mihomo 内核(首次启动时自动调用)
    async fn ensure_kernel(&self) -> Result<(), String> {
        let path = Self::kernel_path(&self.0.app);
        if path.exists() {
            self.0.inner.lock().kernel_ready = true;
            return Ok(());
        }
        let url = mihomo_download_url();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| format!("创建下载客户端失败: {e}"))?;
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("下载内核失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("下载内核失败: HTTP {}", resp.status()));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("读取内核数据失败: {e}"))?;

        let dir = Self::data_dir(&self.0.app);
        if cfg!(target_os = "windows") {
            // .zip:提取 mihomo.exe
            let cursor = std::io::Cursor::new(&bytes[..]);
            let mut archive = zip::ZipArchive::new(cursor)
                .map_err(|e| format!("解压内核失败: {e}"))?;
            for i in 0..archive.len() {
                let mut file = archive
                    .by_index(i)
                    .map_err(|e| format!("读取压缩包条目失败: {e}"))?;
                let name = file.name().to_string();
                // 取第一个可执行文件名(可能是 mihomo-windows-amd64-vX.Y.Z.exe)
                if name.ends_with(".exe") {
                    let out_path = dir.join("mihomo.exe");
                    let mut out = std::fs::File::create(&out_path)
                        .map_err(|e| format!("写入内核文件失败: {e}"))?;
                    std::io::copy(&mut file, &mut out)
                        .map_err(|e| format!("写入内核数据失败: {e}"))?;
                    break;
                }
            }
        } else {
            // .gz:直接解压
            let cursor = std::io::Cursor::new(&bytes[..]);
            let mut decoder = flate2::read::GzDecoder::new(cursor);
            let out_path = dir.join("mihomo");
            let mut out = std::fs::File::create(&out_path)
                .map_err(|e| format!("写入内核文件失败: {e}"))?;
            std::io::copy(&mut decoder, &mut out)
                .map_err(|e| format!("写入内核数据失败: {e}"))?;
            // 设可执行权限
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755));
            }
        }
        if !path.exists() {
            return Err("内核下载后文件不存在".to_string());
        }
        self.0.inner.lock().kernel_ready = true;
        Ok(())
    }

    /// 拉取订阅并生成 mihomo 配置
    async fn fetch_subscription(&self, url: &str) -> Result<String, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("创建订阅拉取客户端失败: {e}"))?;
        let resp = client
            .get(url)
            .header("User-Agent", "clash-verge/1.5")
            .send()
            .await
            .map_err(|e| format!("拉取订阅失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("拉取订阅失败: HTTP {}", resp.status()));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取订阅内容失败: {e}"))?;
        // 订阅返回 Clash 配置 YAML,我们只注入 external-controller 让 UI 能查询
        // 和切换节点。直接在原有配置上追加/覆盖必要字段。
        let mut yaml: serde_yaml::Value =
            serde_yaml::from_str(&text).map_err(|e| format!("解析订阅配置失败: {e}"))?;

        // 注入 external-controller + secret(UI 查询/切换节点用)
        if let serde_yaml::Value::Mapping(ref mut m) = yaml {
            m.insert(
                serde_yaml::Value::String("external-controller".into()),
                serde_yaml::Value::String(format!("127.0.0.1:{MIHOMO_API_PORT}")),
            );
            m.insert(
                serde_yaml::Value::String("secret".into()),
                serde_yaml::Value::String(MIHOMO_API_SECRET.into()),
            );
            // 确保 mixed-port 存在(若订阅未指定则用默认值)
            if !m.contains_key(serde_yaml::Value::String("mixed-port".into())) {
                m.insert(
                    serde_yaml::Value::String("mixed-port".into()),
                    serde_yaml::Value::Number(serde_yaml::Number::from(DEFAULT_PROXY_PORT)),
                );
            }
            // 禁止 mihomo 的 DNS 劫持(避免干扰本机 DNS)
            m.insert(
                serde_yaml::Value::String("dns".into()),
                serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
            );
        }

        let config_text = serde_yaml::to_string(&yaml).map_err(|e| format!("序列化配置失败: {e}"))?;

        // 解析代理端口
        let port = if let serde_yaml::Value::Mapping(ref m) = yaml {
            m.get(serde_yaml::Value::String("mixed-port".into()))
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .unwrap_or(DEFAULT_PROXY_PORT)
        } else {
            DEFAULT_PROXY_PORT
        };

        // 写入配置文件
        let config_path = Self::config_path(&self.0.app);
        std::fs::write(&config_path, &config_text)
            .map_err(|e| format!("写入配置文件失败: {e}"))?;

        self.0.inner.lock().proxy_port = port;
        Ok(config_text)
    }

    /// 启动代理
    pub async fn start(&self) -> Result<(), String> {
        let cfg = Self::read_config(&self.0.app);
        if cfg.subscription_url.is_empty() {
            return Err("未配置订阅链接".to_string());
        }

        // 1. 确保内核已下载
        self.ensure_kernel().await?;

        // 2. 拉取订阅(每次启动都刷新,获取最新节点)
        self.fetch_subscription(&cfg.subscription_url).await?;

        // 3. 先停掉旧进程
        self.stop_sync();

        // 4. 启动 mihomo
        let kernel = Self::kernel_path(&self.0.app);
        let data_dir = Self::data_dir(&self.0.app);
        let config_path = Self::config_path(&self.0.app);

        let mut command = tokio::process::Command::new(&kernel);
        command
            .arg("-d")
            .arg(&data_dir)
            .arg("-f")
            .arg(&config_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());

        #[cfg(target_os = "windows")]
        {
            // Windows:用 CREATE_NO_WINDOW 避免黑框
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let child = command
            .spawn()
            .map_err(|e| format!("启动内核失败: {e}"))?;

        self.0.inner.lock().child = Some(child);
        self.0.inner.lock().error = None;

        // 等待内核初始化(最多 3 秒)
        tokio::time::sleep(Duration::from_secs(2)).await;

        // 验证代理端口可达
        let port = self.0.inner.lock().proxy_port;
        let test_url = format!("http://127.0.0.1:{port}");
        let test_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .proxy(reqwest::Proxy::all(&test_url).map_err(|e| format!("配置代理失败: {e}"))?)
            .build()
            .map_err(|e| format!("创建测试客户端失败: {e}"))?;
        // 尝试通过代理访问一个轻量端点验证连通性(不阻塞流程)
        let _ = test_client
            .get("https://www.google.com/generate_204")
            .send()
            .await;

        Ok(())
    }

    /// 停止代理(同步,用于 start 内部重启)
    fn stop_sync(&self) {
        let mut inner = self.0.inner.lock();
        if let Some(mut child) = inner.child.take() {
            // 尝试 kill 子进程
            let _ = child.start_kill();
        }
    }

    /// 停止代理
    pub fn stop(&self) -> Result<(), String> {
        self.stop_sync();
        Ok(())
    }

    /// 查询代理节点列表(mihomo API)
    pub async fn list_proxies(&self) -> Result<Vec<ProxyGroup>, String> {
        let url = format!("http://127.0.0.1:{MIHOMO_API_PORT}/proxies");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("创建 API 客户端失败: {e}"))?;
        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {MIHOMO_API_SECRET}"))
            .send()
            .await
            .map_err(|e| format!("查询代理列表失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("查询代理列表失败: HTTP {}", resp.status()));
        }
        let body: ProxyListResponse = resp
            .json()
            .await
            .map_err(|e| format!("解析代理列表失败: {e}"))?;
        // 只返回 Selector 和 URLTest 类型的组(有 options 的)
        let mut groups: Vec<ProxyGroup> = body
            .proxies
            .into_iter()
            .filter(|(_, g)| g.r#type == "Selector" || g.r#type == "URLTest" || g.r#type == "Fallback")
            .map(|(_, g)| g)
            .collect();
        groups.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(groups)
    }

    /// 切换节点
    pub async fn select_proxy(&self, group: &str, name: &str) -> Result<(), String> {
        let url = format!("http://127.0.0.1:{MIHOMO_API_PORT}/proxies/{group}");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("创建 API 客户端失败: {e}"))?;
        let resp = client
            .put(&url)
            .header("Authorization", format!("Bearer {MIHOMO_API_SECRET}"))
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await
            .map_err(|e| format!("切换节点失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("切换节点失败: HTTP {}", resp.status()));
        }
        Ok(())
    }
}

// ==================== IPC 命令 ====================

#[tauri::command]
pub async fn proxy_status(app: AppHandle) -> Result<ProxyStatus, String> {
    let mgr = app.state::<ProxyManager>().inner().clone();
    Ok(mgr.status())
}

#[tauri::command]
pub async fn proxy_start(app: AppHandle) -> Result<ProxyStatus, String> {
    let mgr = app.state::<ProxyManager>().inner().clone();
    mgr.start().await?;
    // 重建网关 client 走代理
    if let Some(host) = app.try_state::<crate::gateway::GatewayHost>() {
        host.rebuild_client(mgr.proxy_url().as_deref());
    }
    Ok(mgr.status())
}

#[tauri::command]
pub async fn proxy_stop(app: AppHandle) -> Result<ProxyStatus, String> {
    let mgr = app.state::<ProxyManager>().inner().clone();
    mgr.stop()?;
    // 重建网关 client 去掉代理
    if let Some(host) = app.try_state::<crate::gateway::GatewayHost>() {
        host.rebuild_client(None);
    }
    Ok(mgr.status())
}

/// 保存代理配置(订阅链接 + enabled),并写入 config.json。
#[tauri::command]
pub async fn proxy_save_config(
    app: AppHandle,
    subscription_url: String,
    enabled: bool,
) -> Result<(), String> {
    crate::config::update_config_json(&app, |cfg| {
        cfg.gateway.proxy.subscription_url = subscription_url.clone();
        cfg.gateway.proxy.enabled = enabled;
    })?;
    Ok(())
}

#[tauri::command]
pub async fn proxy_proxies(app: AppHandle) -> Result<Vec<ProxyGroup>, String> {
    let mgr = app.state::<ProxyManager>().inner().clone();
    mgr.list_proxies().await
}

#[tauri::command]
pub async fn proxy_select(
    app: AppHandle,
    group: String,
    name: String,
) -> Result<(), String> {
    let mgr = app.state::<ProxyManager>().inner().clone();
    mgr.select_proxy(&group, &name).await
}
