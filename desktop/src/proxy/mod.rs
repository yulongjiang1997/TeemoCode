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
use tauri::{AppHandle, Manager};

use crate::config::{config_dir, load_config};

/// mihomo 内核版本(下载 URL 中的版本号)
const MIHOMO_VERSION: &str = "1.18.10";
/// 代理监听端口(订阅配置中 mixed-port 优先,否则用此默认值)
const DEFAULT_PROXY_PORT: u16 = 7897;
/// mihomo RESTful API 端口(查代理列表/切换节点)
const MIHOMO_API_PORT: u16 = 12090;
/// mihomo API secret(简单防误访)
const MIHOMO_API_SECRET: &str = "mc-proxy";
/// API 端口候选:系统 Clash 可能已占用 12090,逐个尝试直到找到空闲端口。
const API_PORT_CANDIDATES: [u16; 4] = [12090, 12091, 12092, 12093];
/// 代理端口候选(mixed-port):7890/7897 通常被系统 Clash 占用,逐个尝试。
const PROXY_PORT_CANDIDATES: [u16; 6] = [7897, 7890, 17897, 17898, 17899, 17900];

/// 检测 TCP 端口是否空闲(未被监听)。true = 可用。
/// 在 0.0.0.0 上探测:与 mihomo 的默认监听地址一致(allow-lan 模式),
/// 比只探测 127.0.0.1 更严格 —— 系统 Clash 若已监听 0.0.0.0:7890,
/// 探测 127.0.0.1 可能漏检,导致 mihomo 启动时仍绑定失败。
fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}

/// 从候选端口中选第一个空闲的;全都占用则随机分配一个高位端口。
fn pick_free_port(candidates: &[u16]) -> u16 {
    for &p in candidates {
        if port_is_free(p) {
            return p;
        }
    }
    // 兜底:让操作系统分配一个随机空闲端口
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(0)
}

/// URL path 段的百分号编码(节点名常含中文/emoji/空格,直接拼 path 会被
/// reqwest 原样发送导致 mihomo 解析 404)。保留安全字符,其余按 UTF-8 字节编码。
fn pct_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

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
    /// 已保存的订阅地址(2026-10-31):前端回显用。地址含 token 已在本机,
    /// 回显到同一台机器的设置页是安全的。
    #[serde(default)]
    pub subscription_url: String,
    /// 错误信息(启动失败等)
    pub error: Option<String>,
}

/// mihomo API:代理组
/// 注意:mihomo API 返回字段是 `all`(节点列表),这里用 alias 兼容;
/// 序列化给前端仍用 `options`(前端契约)。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProxyGroup {
    pub name: String,
    pub r#type: String,
    #[serde(default)]
    pub now: String,
    #[serde(default, alias = "all")]
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
    /// 实际使用的 mihomo API 端口(external-controller,启动时选定)
    api_port: u16,
    /// 错误信息
    error: Option<String>,
    /// 内核是否已下载
    kernel_ready: bool,
    /// 实际使用的内核路径(缓存/系统复用/下载)。
    /// 优先于 kernel_path():系统内核可能因文件锁复制失败,直接 spawn 原路径。
    kernel_used: Option<PathBuf>,
}

#[derive(Clone)]
pub struct ProxyManager(pub Arc<ProxyInner_>);

pub struct ProxyInner_ {
    inner: Mutex<ProxyInner>,
    pub(crate) app: AppHandle,
}

impl ProxyManager {
    pub fn new(app: AppHandle) -> Self {
        Self(Arc::new(ProxyInner_ {
            inner: Mutex::new(ProxyInner {
                child: None,
                proxy_port: DEFAULT_PROXY_PORT,
                api_port: MIHOMO_API_PORT,
                error: None,
                kernel_ready: false,
                kernel_used: None,
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
        let mut inner = self.0.inner.lock();
        let cfg = Self::read_config(&self.0.app);
        // 检查子进程是否仍存活:child 为 Some 但进程已退出时(外部 Clash
        // kill、内核崩溃),running 会虚假为 true,这里同步清理
        let alive = match inner.child.as_mut() {
            Some(child) => child.try_wait().map(|st| st.is_none()).unwrap_or(false),
            None => false,
        };
        if !alive {
            // 进程已退出:清理 child,补一条错误(若此前无错误)
            if inner.child.is_some() && inner.error.is_none() {
                inner.error = Some("内核进程已停止".to_string());
            }
            inner.child = None;
        }
        ProxyStatus {
            running: inner.child.is_some(),
            enabled: cfg.enabled,
            proxy_port: inner.proxy_port,
            api_port: inner.api_port,
            kernel_ready: inner.kernel_ready,
            has_subscription: !cfg.subscription_url.is_empty(),
            subscription_url: cfg.subscription_url.clone(),
            error: inner.error.clone(),
        }
    }

/// 检测系统已有的 mihomo 内核(仿 plantHelper findCore)。
/// 复用 Clash Verge / Clash Party 自带的 mihomo 二进制,避免从 GitHub 下载
/// (内地网络通常无法直连 GitHub)。找不到返回 None。
fn find_system_core() -> Option<PathBuf> {
    // Windows 常见安装路径(Clash Verge / Clash Party / Clash Verge Rev)
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let candidates = vec![
            "C:\\Program Files\\Clash Verge\\verge-mihomo.exe".to_string(),
            "C:\\Program Files\\Clash Party\\resources\\sidecar\\mihomo.exe".to_string(),
            format!("{}\\io.github.clash-verge-rev.clash-verge-rev\\verge-mihomo.exe", local),
        ];
        for c in candidates {
            let p = PathBuf::from(&c);
            if p.exists() {
                eprintln!("[proxy] 复用系统内核: {}", p.display());
                return Some(p);
            }
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        // 非 Windows:查找 PATH 中的 mihomo / clash
        let path = std::env::var("PATH").unwrap_or_default();
        let sep = if cfg!(target_os = "windows") { ';' } else { ':' };
        for dir in path.split(sep) {
            let p = PathBuf::from(dir).join("mihomo");
            if p.exists() {
                return Some(p);
            }
            let p = PathBuf::from(dir).join("clash");
            if p.exists() {
                return Some(p);
            }
        }
        None
    }
}

/// 下载 mihomo 内核(首次启动时自动调用)。
///
/// 获取顺序:
/// 1. 数据目录已有缓存内核(之前下载/复制的)→ 直接用;
/// 2. 系统已有 mihomo 内核(Clash Verge / Clash Party 自带的)→ 复制到数据目录;
/// 3. 都没有 → 从 GitHub 下载(大陆网络通常无法直连,因此:
///    - 优先走系统代理(Windows 注册表 ProxyServer);
///    - 多镜像源:官方 GitHub + ghproxy 镜像,逐个尝试;
///    - 启用 gzip 解码(GitHub 可能返回压缩响应))。
async fn ensure_kernel(&self) -> Result<(), String> {
    let path = Self::kernel_path(&self.0.app);
    // 1. 数据目录已有缓存内核(之前下载/复制的)→ 直接用
    if path.exists() {
        self.0.inner.lock().kernel_ready = true;
        eprintln!("[proxy] 内核已缓存: {}", path.display());
        // 缓存路径可能来自之前的下载;若数据目录内核版本过旧且系统有更新版,
        // 用户仍可手动清理,这里不强制覆盖
        self.0.inner.lock().kernel_used = Some(path.clone());
        return Ok(());
    }
    // 2. 系统已有 mihomo 内核 → 直接用原路径 spawn(不复制,避免文件锁失败;
    //    plantHelper 同款做法:复用 Clash Verge / Clash Party 自带内核)
    if let Some(system_core) = Self::find_system_core() {
        self.0.inner.lock().kernel_ready = true;
        self.0.inner.lock().kernel_used = Some(system_core.clone());
        eprintln!("[proxy] 复用系统内核: {}", system_core.display());
        return Ok(());
    }
    // 3. 都没有 → 从 GitHub 下载
    let url = mihomo_download_url();
    // 候选下载源:官方 + ghproxy 镜像(内地常用)
    let sources = vec![
        url.clone(),
        format!("https://mirror.ghproxy.com/{url}"),
        format!("https://ghfast.top/{url}"),
        format!("https://ghproxy.net/{url}"),
    ];
    let dir = Self::data_dir(&self.0.app);

    // 直连客户端(镜像站国内可直连)
    let direct = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建下载客户端失败: {e}"))?;
    // 代理客户端(官方 GitHub 需代理;走系统代理)
    let proxy_client = if let Some(proxy) = system_proxy() {
        let built: Option<Result<reqwest::Client, reqwest::Error>> =
            match reqwest::Proxy::all(&proxy) {
                Ok(p) => Some(
                    reqwest::Client::builder()
                        .timeout(Duration::from_secs(180))
                        .connect_timeout(Duration::from_secs(15))
                        .proxy(p)
                        .build(),
                ),
                Err(e) => {
                    eprintln!("[proxy] 系统代理 {proxy} 配置失败: {e}");
                    None
                }
            };
        match built {
            Some(Ok(c)) => {
                eprintln!("[proxy] 内核下载可用系统代理: {proxy}");
                Some(c)
            }
            Some(Err(e)) => {
                eprintln!("[proxy] 系统代理 {proxy} 不可用: {e}");
                None
            }
            None => None,
        }
    } else {
        None
    };

    let mut last_err = String::new();
    for (i, src) in sources.iter().enumerate() {
        // 先直连,再代理(逐源尝试)
        let mut clients: Vec<(&reqwest::Client, &str)> = vec![(&direct, "直连")];
        if let Some(pc) = &proxy_client {
            clients.push((pc, "代理"));
        }
        for (client, mode) in clients.iter() {
            eprintln!("[proxy] 尝试内核[{}/{}]{}: {src}", i + 1, sources.len(), mode);
            match download_and_extract(client, src, &dir, cfg!(target_os = "windows")).await {
                Ok(()) => {
                    self.0.inner.lock().kernel_ready = true;
                    self.0.inner.lock().kernel_used = Some(path.clone());
                    eprintln!("[proxy] 内核下载成功({mode}): {}", path.display());
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("[proxy] 源 {src} {mode} 失败: {e}");
                    last_err = e;
                }
            }
        }
    }
    Err(format!("内核下载失败(已试 {} 个源 × 直连/代理): {last_err}", sources.len()))
}
}

/// 读取 Windows 系统代理(注册表 ProxyServer)。非 Windows 或未设置返回 None。
fn system_proxy() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
            .ok()?;
        let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
        if enabled == 0 {
            return None;
        }
        let server: String = key.get_value("ProxyServer").ok()?;
        // 注册表可能返回 "host:port" 或 "http=host:port;https=host:port"
        let server = server.split(';').next().unwrap_or(&server).trim().to_string();
        if server.is_empty() {
            return None;
        }
        if server.starts_with("http://") || server.starts_with("https://") || server.starts_with("socks") {
            return Some(server);
        }
        return Some(format!("http://{server}"));
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// 下载并解压内核。windows = 处理 .zip,否则处理 .gz。
async fn download_and_extract(
    client: &reqwest::Client,
    url: &str,
    dir: &std::path::Path,
    windows: bool,
) -> Result<(), String> {
    let resp = client
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取响应体失败: {e}"))?;
    if bytes.len() < 100_000 {
        return Err(format!("响应体过小({}B),可能被拦截", bytes.len()));
    }

    if windows {
        // .zip:提取可执行文件
        let cursor = std::io::Cursor::new(&bytes[..]);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| format!("解压 zip 失败: {e}"))?;
        let mut extracted = false;
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| format!("读取压缩包条目失败: {e}"))?;
            let name = file.name().to_string();
            if name.ends_with(".exe") {
                let out_path = dir.join("mihomo.exe");
                let mut out = std::fs::File::create(&out_path)
                    .map_err(|e| format!("写入内核文件失败: {e}"))?;
                std::io::copy(&mut file, &mut out)
                    .map_err(|e| format!("写入内核数据失败: {e}"))?;
                extracted = true;
                break;
            }
        }
        if !extracted {
            return Err("zip 中未找到 .exe 文件".to_string());
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

impl ProxyManager {
    /// 拉取订阅并生成 mihomo 配置。
    /// `proxy_port`/`api_port`:启动前选定的空闲端口,强制写入配置,
    /// 覆盖订阅自带的 port/external-controller(可能已被本机系统 Clash 占用)。
    async fn fetch_subscription(
        &self,
        url: &str,
        proxy_port: u16,
        api_port: u16,
    ) -> Result<String, String> {
        // 订阅链接可能在境外,同样走系统代理提高成功率
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(15));
        if let Some(proxy) = system_proxy() {
            if let Ok(p) = reqwest::Proxy::all(&proxy) {
                builder = builder.proxy(p);
            }
        }
        let client = builder
            .build()
            .map_err(|e| format!("创建订阅拉取客户端失败: {e}"))?;
        // 很多订阅站按 User-Agent 分流,固定一个标识会被拒绝(403/426/502)。
        // 仿 plantHelper:多个 UA 轮流重试,命中一个能用的即可。
        let agents = [
            "clash-verge/v2.4.2",
            "clash.meta",
            "mihomo/1.19.31",
            "ClashForWindows/0.20.39",
        ];
        let mut last_err = String::new();
        let mut text = String::new();
        for agent in agents {
            let resp = client
                .get(url)
                .header("User-Agent", agent)
                .header("Accept", "*/*")
                .send()
                .await
                .map_err(|e| format!("拉取订阅失败: {e}"))?;
            if !resp.status().is_success() {
                last_err = format!("HTTP {} (UA={})", resp.status(), agent);
                eprintln!("[proxy] 订阅 UA={agent} 被拒: HTTP {}", resp.status());
                continue;
            }
            text = resp
                .text()
                .await
                .map_err(|e| format!("读取订阅内容失败: {e}"))?;
            if text.trim().is_empty() {
                last_err = format!("空响应 (UA={agent})",);
                eprintln!("[proxy] 订阅 UA={agent} 返回空");
                continue;
            }
            eprintln!("[proxy] 订阅拉取成功 (UA={agent})");
            break;
        }
        if text.trim().is_empty() {
            return Err(format!("订阅拉取失败: 所有 User-Agent 均被拒绝。{last_err}"));
        }
        // 订阅返回 Clash 配置 YAML。订阅自带的 port/external-controller 可能
        // 与本机系统 Clash 冲突(如 port:7890、external-controller:9099),
        // 因此这里强制覆盖为我们选定的空闲端口,而不是沿用订阅里的值。
        let mut yaml: serde_yaml::Value =
            serde_yaml::from_str(&text).map_err(|e| format!("解析订阅配置失败: {e}"))?;

        if let serde_yaml::Value::Mapping(ref mut m) = yaml {
            // 强制覆盖 external-controller + secret(UI 查询/切换节点用)
            m.insert(
                serde_yaml::Value::String("external-controller".into()),
                serde_yaml::Value::String(format!("127.0.0.1:{api_port}")),
            );
            m.insert(
                serde_yaml::Value::String("secret".into()),
                serde_yaml::Value::String(MIHOMO_API_SECRET.into()),
            );
            // 强制设定 mixed-port 为选定的空闲端口(覆盖订阅自带值,避免冲突)
            m.insert(
                serde_yaml::Value::String("mixed-port".into()),
                serde_yaml::Value::Number(serde_yaml::Number::from(proxy_port)),
            );
            // 移除订阅自带的 port/socks-port/redir-port/tproxy-port:
            // 只保留 mixed-port 一个入口,防止端口定义互相打架
            for key in ["port", "socks-port", "redir-port", "tproxy-port"] {
                m.remove(serde_yaml::Value::String(key.into()));
            }
            // 关闭 allow-lan:网关只需本机回环访问,没必要监听局域网
            // (订阅常带 allow-lan: true,会让 mihomo 监听 0.0.0.0,更易与
            // 本机其他 Clash 实例的 0.0.0.0 监听冲突)
            m.insert(
                serde_yaml::Value::String("allow-lan".into()),
                serde_yaml::Value::Bool(false),
            );
            // 禁止 mihomo 的 DNS 劫持(避免干扰本机 DNS)
            m.insert(
                serde_yaml::Value::String("dns".into()),
                serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
            );
        }

        let config_text = serde_yaml::to_string(&yaml).map_err(|e| format!("序列化配置失败: {e}"))?;

        // 写入配置文件
        let config_path = Self::config_path(&self.0.app);
        std::fs::write(&config_path, &config_text)
            .map_err(|e| format!("写入配置文件失败: {e}"))?;

        self.0.inner.lock().proxy_port = proxy_port;
        self.0.inner.lock().api_port = api_port;
        Ok(config_text)
    }

    /// 启动代理
    pub async fn start(&self) -> Result<(), String> {
        let cfg = Self::read_config(&self.0.app);
        if cfg.subscription_url.is_empty() {
            return Err("未配置订阅链接".to_string());
        }

        // 1. 确保内核已下载
        self.ensure_kernel()
            .await
            .map_err(|e| self.set_error(e))?;

        // 2. 选定空闲端口(写配置前先探测占用,避免与系统 Clash 冲突)。
        //    订阅自带的 port(如 7890)和我们的默认端口都可能已被本机
        //    Clash Verge 占用 —— 不探测直接启动必然绑定失败。
        let proxy_port = pick_free_port(&PROXY_PORT_CANDIDATES);
        let api_port = pick_free_port(&API_PORT_CANDIDATES);
        if proxy_port == 0 || api_port == 0 {
            let msg = "无可用空闲端口(代理端口或 API 端口分配失败)".to_string();
            self.0.inner.lock().error = Some(msg.clone());
            return Err(msg);
        }
        eprintln!("[proxy] 选定端口: mixed-port={proxy_port}, api={api_port}");

        // 3. 拉取订阅(每次启动都刷新,获取最新节点),写入选定的端口
        self.fetch_subscription(&cfg.subscription_url, proxy_port, api_port)
            .await
            .map_err(|e| self.set_error(e))?;

        // 4. 先停掉旧进程
        self.stop_sync();

        // 5. 启动 mihomo(优先用 ensure_kernel 确定的内核路径:缓存/系统复用/下载)
        let kernel = self
            .0
            .inner
            .lock()
            .kernel_used
            .clone()
            .unwrap_or_else(|| Self::kernel_path(&self.0.app));
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
            // (tokio::process::Command 在 Windows 上提供 inherent creation_flags)
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command
            .spawn()
            .map_err(|e| {
                let msg = format!("启动内核失败: {e}");
                self.0.inner.lock().error = Some(msg.clone());
                msg
            })?;

        // 6. 等待内核初始化,并确认进程没有立即崩溃(端口冲突/配置错误会秒退)
        tokio::time::sleep(Duration::from_millis(1500)).await;
        match child.try_wait() {
            Ok(None) => { /* 进程仍在运行 */ }
            Ok(Some(status)) => {
                // 进程已退出 —— 读 stderr 拿到真实原因
                let stderr = match child.stderr.take() {
                    Some(mut s) => {
                        use tokio::io::AsyncReadExt;
                        let mut buf = String::new();
                        let _ = s.read_to_string(&mut buf).await;
                        buf
                    }
                    None => String::new(),
                };
                let tail = stderr.trim().lines().last().unwrap_or("").to_string();
                let msg = if tail.is_empty() {
                    format!("内核启动后立即退出(status={status}),可能端口冲突或配置错误")
                } else {
                    format!("内核启动失败: {tail}")
                };
                eprintln!("[proxy] {msg}");
                self.0.inner.lock().error = Some(msg.clone());
                self.0.inner.lock().child = None;
                return Err(msg);
            }
            Err(e) => {
                let msg = format!("检查内核进程失败: {e}");
                self.0.inner.lock().error = Some(msg.clone());
                return Err(msg);
            }
        }

        self.0.inner.lock().child = Some(child);
        self.0.inner.lock().error = None;

        // 7. 轮询等待 mihomo API 就绪(内核起来到 API 可用有延迟),
        //    API 通了才说明节点列表能查;超时不视为致命错误(代理本身可能能用)
        let api_ready = Self::wait_api_ready(api_port, Duration::from_secs(8)).await;
        if !api_ready {
            let msg = format!("代理已启动(端口 {proxy_port}),但管理 API({api_port})响应超时");
            eprintln!("[proxy] {msg}");
            self.0.inner.lock().error = Some(msg);
        }

        Ok(())
    }

    /// 轮询检查 mihomo API 是否就绪(GET /version 返回 200)。
    async fn wait_api_ready(api_port: u16, timeout: Duration) -> bool {
        let url = format!("http://127.0.0.1:{api_port}/version");
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            match client
                .get(&url)
                .header("Authorization", format!("Bearer {MIHOMO_API_SECRET}"))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => return true,
                _ => tokio::time::sleep(Duration::from_millis(300)).await,
            }
        }
    }

    /// 记录错误到状态(前端 proxy_status 可见),并原样返回该错误。
    /// 用于 start() 内部各步骤的 `map_err`,保证失败原因不丢失。
    fn set_error(&self, msg: String) -> String {
        self.0.inner.lock().error = Some(msg.clone());
        msg
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
        let api_port = self.0.inner.lock().api_port;
        let url = format!("http://127.0.0.1:{api_port}/proxies");
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

    /// 切换节点(注意:mihomo 的 API 是 PUT /proxies/{组名},body={"name": 节点})
    pub async fn select_proxy(&self, group: &str, name: &str) -> Result<(), String> {
        let api_port = self.0.inner.lock().api_port;
        let url = format!("http://127.0.0.1:{api_port}/proxies/{}", pct_encode_path(group));
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

    /// 测速:对指定节点发起延迟测试,返回毫秒。
    /// mihomo API: GET /proxies/{name}/delay?url=...&timeout=...
    pub async fn test_delay(&self, name: &str) -> Result<u64, String> {
        let api_port = self.0.inner.lock().api_port;
        let url = format!(
            "http://127.0.0.1:{api_port}/proxies/{}/delay",
            pct_encode_path(name)
        );
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("创建测速客户端失败: {e}"))?;
        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {MIHOMO_API_SECRET}"))
            .query(&[
                ("url", "http://www.gstatic.com/generate_204"),
                ("timeout", "5000"),
            ])
            .send()
            .await
            .map_err(|e| format!("测速请求失败: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("测速失败: HTTP {status} {body}"));
        }
        // mihomo 返回 {"delay": 123} 或 {"message": "..."} 表示失败
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("解析测速结果失败: {e}"))?;
        if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
            return Err(msg.to_string());
        }
        let delay = v
            .get("delay")
            .and_then(|d| d.as_u64())
            .unwrap_or(0);
        Ok(delay)
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

/// 节点测速(2026-11-03):对指定节点发起延迟测试,返回毫秒数。
#[tauri::command]
pub async fn proxy_delay(app: AppHandle, name: String) -> Result<u64, String> {
    let mgr = app.state::<ProxyManager>().inner().clone();
    mgr.test_delay(&name).await
}
