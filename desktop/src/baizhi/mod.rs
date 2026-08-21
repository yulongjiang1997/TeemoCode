// ç¾æºäºè´¦å· + MonkeyCode äºç«¯(agent/internal/baizhi ç Rust ç§»æ¤)ã
// å£³çº§åä¾,ä¸ agent å¼ææ å³(åå° ohmyagent äºç«¯åè½ç§å¸¸)ã
// å­è¯(cookie)åªå¨å£³è¿ç¨å,UI ç» Tauri IPC é©±å¨ã

pub mod cookies;
pub mod monkeycode;
pub mod pow;
pub mod sync;
pub mod wechat;

#[cfg(test)]
mod tests;

use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::State;

use cookies::CookieStore;

const DEFAULT_MODEL_GATEWAY: &str = "https://ai-models.app.baizhi.cloud";
const DEFAULT_MCP_GATEWAY: &str = "https://agent-toolkit.app.baizhi.cloud";
/// MonkeyCode å®æ¹äºå°å(config.rs ç Basic Auth ä½ç¨åå¤å®ä¹ç¨å®)ã
pub(crate) const DEFAULT_MONKEYCODE_URL: &str = "https://monkeycode-ai.com";
/// å®æ¹äºçæ¨¡åè¯·æ±å°å:llmproxy èµ°ç¬ç«å­å,ä¸ä¸ä¸»æå¡ååã
pub(crate) const DEFAULT_MONKEYCODE_LLM_URL: &str = "https://proxy.monkeycode-ai.com/v1";

/// æ¨¡åè¯·æ±å°å(llmproxy)çåä¸åºå¤(2026-08-07 ç¨æ·å®æ¡):
/// - è®¾ç½®éæ¾å¼å¡«äºãæ¨¡åè¯·æ±å°åãâ åæ ·ç¨å®(æåé¨ç½²çéçé¨,ç«å³çæ);
/// - å¦åæå¡å°åå°±æ¯å®æ¹äº â ç¬ç«ä»£çå­å proxy.monkeycode-ai.com/v1;
/// - å¦å(èªå»º/ç§æå/èè°)â è·éæå¡å°å {æå¡å°å}/v1:å¼æºåç«¯æ
///   llmproxy æå¨ä¸»æå¡ç /v1 ä¸(backend/biz/llmproxy/register.go),ååå³è¾¾ã
///
/// `monkeycode` ä¼ å·²è§£æçæå¡å°å(Endpoints::resolve ä¹å,å«ç¯å¢åéè¦ç):
/// ç¯å¢åéæåèè°ç¯å¢æ¶æèªå»ºå¤ç,ä¸ä¼è¯¯åå®æ¹ä»£çåã
pub(crate) fn resolve_mc_llm(mc_llm_base_url: &str, monkeycode: &str) -> String {
    let explicit = mc_llm_base_url.trim().trim_end_matches('/');
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    let server = monkeycode.trim().trim_end_matches('/');
    if server == DEFAULT_MONKEYCODE_URL {
        return DEFAULT_MONKEYCODE_LLM_URL.to_string();
    }
    format!("{server}/v1")
}

/// ç¾æºäºæå¡å°åãæ¨¡åä¸ MCP æå¡åºå®èµ°å®æ¹äº;è´¦å·å MonkeyCode å°åå¯è¦çã
pub struct Endpoints {
    /// è´¦å·/ç»å½å(éªè¯ç ãææºå·/å¾®ä¿¡ç»å½ãprofile)
    pub account: String,
    /// æ¨¡åæå¡:/api/console/* å key ä¸æ¨¡ååè¡¨;/api/openaiã/api/anthropic æ¨ç
    pub model_gateway: String,
    /// Agent å·¥å·å(MCP æå¡)
    pub mcp_gateway: String,
    /// MonkeyCode äºç«¯(è´¦å·æ¡¥æ¥ç»å½ + äºç«¯ä»»å¡)
    pub monkeycode: String,
}

fn env_or(env: &str, def: &str) -> String {
    std::env::var(env)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| def.to_string())
        .trim_end_matches('/')
        .to_string()
}

impl Endpoints {
    /// mc_base_url æ¥èªè®¾ç½®(config.json,èªå»º/ç§æåé¨ç½²å°å;ç©º = å®æ¹äº)ã
    /// ä¼åçº§:ç¯å¢åé(å¼å/èè°éçé¨)> è®¾ç½®å¼ > å®æ¹äºé»è®¤ã
    pub fn resolve(mc_base_url: &str) -> Self {
        let mc_default = mc_base_url.trim();
        let mc_default = if mc_default.is_empty() { DEFAULT_MONKEYCODE_URL } else { mc_default };
        Self {
            account: env_or("MC_DESKTOP_BAIZHI_URL", "https://baizhi.cloud"),
            model_gateway: DEFAULT_MODEL_GATEWAY.to_string(),
            mcp_gateway: DEFAULT_MCP_GATEWAY.to_string(),
            monkeycode: env_or("MC_DESKTOP_MONKEYCODE_URL", mc_default),
        }
    }
}

/// ä¼è¯å¤±æå¨åµ:Status ç±»æ¥å£è½¬æ"æªç»å½"èéæ¥é;éè¯¯ä¿¡æ¯éä¼  UIã
pub enum BzErr {
    Unauthorized(String),
    Other(String),
}

impl BzErr {
    pub fn msg(self) -> String {
        match self {
            BzErr::Unauthorized(m) | BzErr::Other(m) => m,
        }
    }
}

pub type BzResult<T> = Result<T, BzErr>;

pub fn other(m: impl Into<String>) -> BzErr {
    BzErr::Other(m.into())
}

/// ç¾æºäºè´¦å·æå¡ãcookie ååç½:ç¾æºä¼è¯(store)ä¸ MonkeyCode ä¼è¯(mc),
/// å­è¯è¯­ä¹ä¸å,ä¸æ¹ç»åºä¸çµè¿å¦ä¸æ¹ã
pub struct Service {
    pub ep: Endpoints,
    /// API ç­è¯·æ±(30s;ä¸èªå¨è·ééå®åââå¾®ä¿¡åè°ç­ 302 ç Set-Cookie
    /// è¦å¨é¦ååºå°±å¸æ¶,è·éä¼ä¸¢ä¸­é´ååºç cookie)
    ///
    /// Option èéç´æ¥ Client:æå»ºåªå¨ TLS åç«¯èµ·ä¸æ¥æ¶å¤±è´¥,æ­¤åæ¯
    /// `.expect()`ââè Service::new å¨ setup éè·,GUI å­ç³»ç»ä¸(Windows
    /// æ æ§å¶å°)panic å°±æ¯åå»æ²¡ååºãé¶çº¿ç´¢ãäºç«¯/è´¦å·æ¯å¯éçº§åè½é¢,
    /// æ¬å°å¼æä¼è¯å¹¶ä¸ä¾èµå®,ä¸è¯¥è¢«å®æçä¸èµ·æä¸å¼ã
    http: Option<reqwest::Client>,
    /// å¾®ä¿¡ææé¡µ/äºç»´ç /é¿è½®è¯¢(é¿è½®è¯¢æé¿æ ~25s)
    lp: Option<reqwest::Client>,
    pub store: CookieStore,
    pub mc: CookieStore,
    /// æµè¯ç¯å¢ååä»£çç Basic Auth å¤´å¼(é¢è®¡ç®ç "Basic <b64>";None =
    /// æªéç½®)ãä»å¯¹ MonkeyCode åçè¯·æ±éå ,è§ mc_basic_headerã
    pub(crate) mc_basic: Option<String>,
    /// æ¨¡åè¯·æ±å°å(llmproxy):è®¾ç½®éåç¬æå®çå¼,æé»è®¤ {æå¡å°å}/v1ã
    /// ä¼åæ¨¡åæ¡ç®ç base_url å¿«ç§ä¸ç©åæ³¨å¥é½ä»¥å®ä¸ºåã
    pub(crate) mc_llm: String,
    /// èªå»º/ç§æåé¨ç½²ç¨èªç­¾è¯ä¹¦æ¶è·³è¿ TLS æ ¡éª(ä»å¯¹ MonkeyCode åçæ)ã
    pub(crate) mc_skip_tls: bool,
    /// è¿è¡ä¸­çæ«ç ä¼è¯(åªä¿çææ°)
    pub wx: StdMutex<Option<wechat::WechatLogin>>,
}

/// "user:pass" â é¢è®¡ç®ç Basic Auth å¤´å¼(ç©ºç½ = æªéç½®)ã
fn basic_header_value(user_pass: &str) -> Option<String> {
    use base64::Engine as _;
    let v = user_pass.trim();
    (!v.is_empty()).then(|| format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(v.as_bytes())))
}

impl Service {
    /// æµè¯æé :ç«¯ç¹å¯æ³¨å¥,cookie ä»åå­ã
    #[cfg(test)]
    pub fn test_service(ep: Endpoints) -> Self {
        let mk = |timeout: u64| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(timeout))
                .build()
                .expect("æå»º HTTP å®¢æ·ç«¯å¤±è´¥")
        };
        let mc_llm = resolve_mc_llm("", &ep.monkeycode);
        Self {
            ep,
            http: Some(mk(10)),
            lp: Some(mk(10)),
            store: CookieStore::new(None),
            mc: CookieStore::new(None),
            mc_basic: None,
            mc_llm,
            mc_skip_tls: false,
            wx: StdMutex::new(None),
        }
    }

    pub fn new(config_dir: std::path::PathBuf, cfg: &crate::config::DesktopConfig) -> Self {
        // æå»ºå¤±è´¥åªåçå¨ TLS åç«¯åå§åä¸äºæ¶ãä¸ panic:å£³å¨ setup é
        // æé æ¬æå¡,GUI å­ç³»ç»ä¸ panic = åå»æ²¡ååºãæ ä»»ä½çº¿ç´¢ãéçº§ä¸º
        // äºç«¯/è´¦å·å½ä»¤éæ¡æ¥é,æ¬å°å¼æä¼è¯ä¸åå½±åã
        // mc_skip_tls_verify:èªå»º/ç§æåç¨èªç­¾è¯ä¹¦æ¶è·³è¿ TLS æ ¡éª;å®æ¹äº
        // æ°¸ä¸è·³è¿(ä»å½æå¡å°åç¡®å®æ¯å®æ¹ååæ¶æä¸º false,è§ä¸æ¹å¤å®)ã
        let skip_tls = cfg.mc_skip_tls_verify && cfg.mc_base_url.trim() != DEFAULT_MONKEYCODE_URL;
        let mk = |timeout: u64| {
            let mut b = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(timeout));
            if skip_tls {
                // è­¦å:èªç­¾è¯ä¹¦åºæ¯ä¸çéçé¨,éå®æ¹äºä¸å¼å¯ã
                b = b.danger_accept_invalid_certs(true);
            }
            b.build()
                .inspect_err(|e| eprintln!("[desktop] HTTP å®¢æ·ç«¯æå»ºå¤±è´¥(äºç«¯/è´¦å·åè½ä¸å¯ç¨): {e}"))
                .ok()
        };
        let ep = Endpoints::resolve(&cfg.mc_base_url);
        let mc_llm = resolve_mc_llm(&cfg.mc_llm_base_url, &ep.monkeycode);
        Self {
            ep,
            http: mk(30),
            lp: mk(40),
            store: CookieStore::new(Some(config_dir.join("baizhi-cookies.json"))),
            mc: CookieStore::new(Some(config_dir.join("monkeycode-cookies.json"))),
            mc_basic: basic_header_value(&cfg.mc_basic_auth),
            mc_llm,
            mc_skip_tls: skip_tls,
            wx: StdMutex::new(None),
        }
    }

    /// æµè¯ç¯å¢ååä»£çç Basic Auth å¤´(ä»å½ url è½å¨ MonkeyCode åæ¶è¿å;
    /// ä¸å¡é´æèµ° cookie,REST/WS é¾è·¯ä¸ Authorization å¤´æ¯ç©ºé²ç,å¯¹é½
    /// mobile ç authHeaders)ãä¼åæ¨¡åç LLM è°ç¨ä¸èµ°è¿é:å¼æä¾§ç±ç©å
    /// æ Basic åµè¿æ¡ç® base_url ç userinfo(config.rs with_basic_userinfo,
    /// anthropic åè®®å¯ç¨;openai ç³»åè®®å å¼æå ç¨ Authorization ä»åé)ã
    pub(crate) fn mc_basic_header(&self, url: &reqwest::Url) -> Option<&str> {
        let basic = self.mc_basic.as_deref()?;
        let mc = reqwest::Url::parse(&self.ep.monkeycode).ok()?;
        (url.host_str() == mc.host_str() && url.port_or_known_default() == mc.port_or_known_default())
            .then_some(basic)
    }

    /// å API å®¢æ·ç«¯;TLS åç«¯èµ·ä¸æ¥æ¶ç»åºå¯è¡å¨éè¯¯èä¸æ¯ panicã
    fn http(&self) -> BzResult<&reqwest::Client> {
        self.http
            .as_ref()
            .ok_or_else(|| other("HTTP å®¢æ·ç«¯åå§åå¤±è´¥(ç³»ç» TLS ä¸å¯ç¨),äºç«¯ä¸è´¦å·åè½æä¸å¯ç¨"))
    }

    /// åé¿è½®è¯¢å®¢æ·ç«¯(åä¸)ã
    fn lp(&self) -> BzResult<&reqwest::Client> {
        self.lp
            .as_ref()
            .ok_or_else(|| other("HTTP å®¢æ·ç«¯åå§åå¤±è´¥(ç³»ç» TLS ä¸å¯ç¨),äºç«¯ä¸è´¦å·åè½æä¸å¯ç¨"))
    }

    // ==================== HTTP åºåº§ ====================

    /// åè¯·æ±:æºå¸¦æå®ç½ç cookie,å¸æ¶ååºç Set-Cookieã
    /// è¿å (body, status, Location å¤´)ââæ¡¥æ¥ç»å½æå¨è·ééå®åéè¦ Locationã
    pub async fn do_store_full(
        &self,
        store: &CookieStore,
        method: reqwest::Method,
        target: &str,
        body: Option<&Value>,
    ) -> BzResult<(Vec<u8>, u16, Option<String>)> {
        let url = reqwest::Url::parse(target).map_err(|e| other(format!("å°åå¼å¸¸: {e}")))?;
        let host = url.host_str().unwrap_or("").to_string();
        let mut req = self.http()?.request(method, url.clone());
        if let Some(b) = body {
            req = req.json(b);
        }
        if let Some(h) = store.header(&url) {
            req = req.header(reqwest::header::COOKIE, h);
        }
        if let Some(b) = self.mc_basic_header(&url) {
            req = req.header(reqwest::header::AUTHORIZATION, b);
        }
        let resp = req.send().await.map_err(|e| other(format!("è¯·æ± {host} å¤±è´¥: {e}")))?;
        let status = resp.status().as_u16();
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let set_cookies: Vec<String> = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect();
        store.update(resp.url(), &set_cookies);
        let data = resp.bytes().await.map_err(|e| other(format!("è¯»åååºå¤±è´¥: {e}")))?;
        Ok((data.to_vec(), status, location))
    }

    /// do_store_full çå¸¸ç¨å½¢æ(ä¸å³å¿ Location)ã
    pub async fn do_store(
        &self,
        store: &CookieStore,
        method: reqwest::Method,
        target: &str,
        body: Option<&Value>,
    ) -> BzResult<(Vec<u8>, u16)> {
        let (data, status, _) = self.do_store_full(store, method, target, body).await?;
        Ok((data, status))
    }

    /// è´¦å·åç¸å¯¹è·¯å¾è¯·æ±(ç»å¯¹ URL ç´æ¥ç¨)ã
    async fn account_do(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> BzResult<(Vec<u8>, u16)> {
        let target = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{}", self.ep.account, path)
        };
        self.do_store(&self.store, method, &target, body).await
    }

    /// GET ä»»æ URL(ç¾æºç½;å¾®ä¿¡é¡µé¢/å¾ç/é¿è½®è¯¢èµ°è¿é,è¶æ¶ 40s)ã
    pub async fn fetch(&self, raw_url: &str) -> BzResult<Vec<u8>> {
        let url = reqwest::Url::parse(raw_url).map_err(|e| other(format!("å°åå¼å¸¸: {e}")))?;
        let mut req = self.lp()?.get(url.clone());
        if let Some(h) = self.store.header(&url) {
            req = req.header(reqwest::header::COOKIE, h);
        }
        let resp = req.send().await.map_err(|e| other(format!("è¯·æ±å¤±è´¥: {e}")))?;
        let status = resp.status().as_u16();
        let set_cookies: Vec<String> = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect();
        self.store.update(resp.url(), &set_cookies);
        if status >= 400 {
            return Err(other(format!("HTTP {status}")));
        }
        resp.bytes().await.map(|b| b.to_vec()).map_err(|e| other(format!("è¯»åååºå¤±è´¥: {e}")))
    }

    /// è¯·æ±ç¾æºäºä¸å¡æ¥å£å¹¶è§£å¼ {code,message,success,data} åå£³ã
    /// è¿å data(ç¼º data å­æ®µæ¶è¿åæ´ä¸ªååºä½,å¯¹é½ç§»å¨ç«¯è¯­ä¹)ã
    pub async fn call(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> BzResult<Value> {
        let (data, status) = self.account_do(method, path, body).await?;
        unwrap_envelope(&data, status, &ENV_BAIZHI)
    }

    /// è£¸ç»æç«¯ç¹è¯·æ±(éªè¯ç  challenge/redeem ä¸å¥ {code,data} åå£³,
    /// 2xx å³æå):ç½ä¸éè¯¯æ ç­¾ç±è°ç¨æ¹æå®ãç½åæ°å³å®çæ¯
    /// ååº Set-Cookie ç**å¸æ¶æ¹å**(è£¸ç»æç«¯ç¹æ¬èº«å¤ä¸ºåé´æ)ââ
    /// MonkeyCode åå¿é¡»ä¼  mc ç½,ç¨ç¾æºç½ä¼æ mc å cookie æ··è¿ç¾æºç½,
    /// ç ´ååç½éç¦»ã
    async fn raw_at(
        &self,
        store: &CookieStore,
        method: reqwest::Method,
        target: &str,
        body: Option<&Value>,
        label: &str,
    ) -> BzResult<Value> {
        let (data, status) = self.do_store(store, method, target, body).await?;
        if !(200..300).contains(&status) {
            if let Ok(v) = serde_json::from_slice::<Value>(&data) {
                if let Some(m) = v.get("message").and_then(|m| m.as_str()) {
                    return Err(other(clean_message(m)));
                }
            }
            return Err(http_error(status, &data, label));
        }
        serde_json::from_slice(&data).map_err(|e| other(format!("{label}ååºè§£æå¤±è´¥: {e}")))
    }

    // ==================== ç»å½/ç¶æ ====================

    /// ç¾æºäºåç PoW éªè¯ç (ææºå·åç /ç»å½ç¨)ã
    async fn obtain_captcha_token(&self) -> BzResult<String> {
        self.captcha_token_at(&self.ep.account, &self.store, "ç¾æºäº").await
    }

    /// å®æ´è·ä¸é PoW éªè¯ç ,è¿åç»å½æ¥å£æé captcha_tokenãç¾æºäºä¸
    /// MonkeyCode æå¡ç«¯ç¨åä¸å¥ go-cap åè®®(challenge 201 è£¸ç»æ â æ¬å°
    /// çç ´ â redeem æ¢ token),å·®å¼åªå¨åãcookie ç½ä¸éè¯¯æ ç­¾ã
    pub(crate) async fn captcha_token_at(&self, base: &str, store: &CookieStore, label: &str) -> BzResult<String> {
        let ch = self
            .raw_at(
                store,
                reqwest::Method::POST,
                &format!("{base}/api/v1/public/captcha/challenge"),
                None,
                label,
            )
            .await
            .map_err(|e| other(format!("è·åéªè¯ç è´¨è¯¢å¤±è´¥: {}", e.msg())))?;
        let token = ch.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let challenge: pow::Challenge = serde_json::from_value(ch.get("challenge").cloned().unwrap_or(Value::Null))
            .map_err(|_| other("éªè¯ç è´¨è¯¢ååºæ ¼å¼å¼å¸¸"))?;
        if token.is_empty() || challenge.c == 0 {
            return Err(other("éªè¯ç è´¨è¯¢ååºæ ¼å¼å¼å¸¸"));
        }
        // SHA-256 çç ´æ¯ CPU å¯é,ä¸¢ blocking æ± 
        let tk = token.clone();
        let solutions = tauri::async_runtime::spawn_blocking(move || pow::solve_challenges(&tk, challenge))
            .await
            .map_err(|e| other(format!("éªè¯ç æ±è§£å¤±è´¥: {e}")))?
            .map_err(other)?;
        let rd = self
            .raw_at(
                store,
                reqwest::Method::POST,
                &format!("{base}/api/v1/public/captcha/redeem"),
                Some(&json!({ "token": token, "solutions": solutions })),
                label,
            )
            .await
            .map_err(|e| other(format!("éªè¯ç æ ¡éªå¤±è´¥: {}", e.msg())))?;
        let ok = rd.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        let rd_token = rd.get("token").and_then(|v| v.as_str()).unwrap_or("");
        if !ok || rd_token.is_empty() {
            let msg = rd.get("message").and_then(|v| v.as_str()).unwrap_or("éªè¯ç æ ¡éªæªéè¿");
            return Err(other(clean_message(msg)));
        }
        Ok(rd_token.to_string())
    }

    /// åéç»å½ç­ä¿¡éªè¯ç (åé¨åå®æ PoW éªè¯ç )ã
    pub async fn send_phone_code(&self, phone: &str) -> BzResult<()> {
        let captcha = self.obtain_captcha_token().await?;
        self.call(
            reqwest::Method::POST,
            "/api/v1/user/phone_code",
            Some(&json!({ "phone": phone, "kind": "login", "captcha_token": captcha })),
        )
        .await
        .map(|_| ())
    }

    /// ææºå· + ç­ä¿¡éªè¯ç ç»å½;æååä¼è¯ cookie å·²æä¹åã
    pub async fn login_phone(&self, phone: &str, code: &str) -> BzResult<()> {
        let captcha = self.obtain_captcha_token().await?;
        self.call(
            reqwest::Method::POST,
            "/api/v1/user/login/phone",
            Some(&json!({ "phone": phone, "code": code, "captcha_token": captcha })),
        )
        .await
        .map(|_| ())
    }

    /// ä¼è¯ç¶æ:æ cookie æ¶æ¢æµ profile,200 è§ä¸ºå·²ç»å½å¹¶è¿ååæ · profileã
    pub async fn status(&self) -> BzResult<(bool, Value)> {
        if self.store.is_empty() {
            return Ok((false, Value::Null));
        }
        match self.call(reqwest::Method::GET, "/api/v1/user/profile", None).await {
            Ok(profile) => Ok((true, profile)),
            Err(BzErr::Unauthorized(_)) => Ok((false, Value::Null)),
            Err(e) => Err(e),
        }
    }

    /// è´¦å·åä¸»æºå(è¯æ­å±ç¤ºç¨)ã
    pub fn base_host(&self) -> String {
        reqwest::Url::parse(&self.ep.account)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| self.ep.account.clone())
    }
}

// ==================== åå£³/éè¯¯è¾å© ====================

/// åå£³è§£åç­ç¥ãåä¸ªåç«¯(ç¾æºäºè´¦å·å/æ¨¡åç½å³/MCP ç½å³/MonkeyCode)ç
/// {code,message,(success),data} åå£³ç»æç¸å,å·®å¼åªå¨ code åæ³å¼éåã
/// 3xx/401 å¤çä¸ data ç¼ºå¤±ååºââç¨åæ°éä½å·®å¼,é²æ­¢åèªæ·è´åè¯­ä¹æ¼ç§»ã
pub(crate) struct Envelope {
    /// http_error çåç¼æ ç­¾(æä¸è¯æ ç­¾èªå¸¦å°¾é¨ç©ºæ ¼,ä¸ä¸­ææ¼æ¥æ¶çæçé´é)
    pub label: &'static str,
    /// code å­æ®µåæ³å¼å¤å®(æ¶å°çæ¯ `v.get("code")` åå¼,å«ç¼ºå¤±/éæ°å­æå½¢)
    pub code_ok: fn(Option<&Value>) -> bool,
    /// æ¯å¦é¢å¤æ£æ¥ success å¸å°å­æ®µ(ç¾æºäºè´¦å·ååå£³å¸¦ success)
    pub check_success: bool,
    /// Some(ææ¡):3xx ç´æ¥ä»¥è¯¥ææ¡å¤å¤±è´¥(MCP ç½å³æªå¼éæ¶ä¸éå®åå³ 302)
    pub redirect_msg: Option<&'static str>,
    /// Some(ææ¡):401 ä¸çååºä½,ç´æ¥è¿ååºå® Unauthorized
    /// (MonkeyCode é¾è·¯ç 401 æ¢å¤å¨ä½æ¯"å°è®¾ç½®ä¸­éæ°è¿æ¥"ââæ¡¥æ¥æè´¦å¯çå¯)
    pub fixed_401: Option<&'static str>,
    /// data ç¼ºå¤±/ä¸º null æ¶:true è¿åæ´ä¸ªååºä½(ç¾æºäº,å¯¹é½ç§»å¨ç«¯),false è¿å Null
    pub whole_body_fallback: bool,
}

/// å¸¸è§ code å¤å®:æ´æ° 0 åæ³;ç¼ºå¤±æéæ°å­ä¸è§ä¸ºå¤±è´¥(ä¸åé¾è·¯åè¯­ä¹ä¸è´)ã
pub(crate) fn code_is_zero(c: Option<&Value>) -> bool {
    c.and_then(Value::as_i64).map(|x| x == 0).unwrap_or(true)
}

/// ç¾æºäºè´¦å·å:åå£³å¸¦ success å¸å°;ç¼º data åæ´ä¸ªååºä½(å¯¹é½ç§»å¨ç«¯)ã
pub(crate) const ENV_BAIZHI: Envelope = Envelope {
    label: "ç¾æºäº",
    code_ok: code_is_zero,
    check_success: true,
    redirect_msg: None,
    fixed_401: None,
    whole_body_fallback: true,
};

/// æç­ç¥è§£å¼åå£³:é 2xx æ code/success å¤å¤±è´¥;å¤±è´¥ä¿¡æ¯ç» clean_message,
/// 401 è½¬ Unauthorized å¨åµ;æåå dataã
pub(crate) fn unwrap_envelope(data: &[u8], status: u16, p: &Envelope) -> BzResult<Value> {
    if let Some(msg) = p.fixed_401 {
        if status == 401 {
            return Err(BzErr::Unauthorized(msg.into()));
        }
    }
    if let Some(msg) = p.redirect_msg {
        if (300..400).contains(&status) {
            return Err(other(msg));
        }
    }
    let is2xx = (200..300).contains(&status);
    let Ok(v) = serde_json::from_slice::<Value>(data) else {
        if is2xx {
            return Ok(Value::Null); // é JSON ä½ 2xx,è§ä¸ºæåæ æ°æ®
        }
        return Err(http_error(status, data, p.label));
    };
    let code_fail = !(p.code_ok)(v.get("code"));
    let success_fail = p.check_success
        && v.get("success").and_then(|s| s.as_bool()).map(|s| !s).unwrap_or(false);
    if !is2xx || code_fail || success_fail {
        let msg = clean_message(v.get("message").and_then(|m| m.as_str()).unwrap_or(""));
        if msg.is_empty() {
            return Err(http_error(status, &[], p.label));
        }
        if status == 401 {
            return Err(BzErr::Unauthorized(msg));
        }
        return Err(other(msg));
    }
    match v.get("data") {
        Some(d) if !d.is_null() => Ok(d.clone()),
        _ if p.whole_body_fallback => Ok(v),
        _ => Ok(Value::Null),
    }
}

/// trace_id å¥ç¦»æ­£åãOnceLock:clean_message å¨æ¯æ¡éè¯¯è·¯å¾ä¸è¢«è°,
/// ç°ç¼è¯æ­£å(å¾®ç§çº§ä½åå¤åç)çº¯å±æµªè´¹,è¿ç¨åç¼è¯ä¸æ¬¡å³å¯ã
static TRACE_ID_RE: OnceLock<regex::Regex> = OnceLock::new();

/// å»ææå¡ç«¯ message å°¾é¨ç trace_id æ æ³¨(å¯¹é½ç§»å¨ç«¯)ã
pub fn clean_message(msg: &str) -> String {
    TRACE_ID_RE
        .get_or_init(|| regex::Regex::new(r"(?i)\s*\[trace_id:[^\]]+\]\s*$").unwrap())
        .replace(msg, "")
        .trim()
        .to_string()
}

/// è¯æ­çæ®µ:éé¢æååºçåææªæ­(å¥çº¦æ¼ç§»/ç»å½é¡µåæ¢çæ¥éä¸æ¥å¿ç¨),
/// æå­ç¬¦æªæ­é¿åæè£å¤å­èã
pub(crate) fn snippet(text: &str, max_chars: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max_chars {
        t.to_string()
    } else {
        let cut: String = t.chars().take(max_chars).collect();
        format!("{cut}â¦")
    }
}

pub fn http_error(status: u16, body: &[u8], label: &str) -> BzErr {
    if status == 401 {
        return BzErr::Unauthorized(format!("{label}ä¼è¯å·²å¤±æ,è¯·éæ°ç»å½"));
    }
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if !text.is_empty() && text.len() <= 200 && !text.starts_with('<') {
        other(format!("{label}è¯·æ±å¤±è´¥(HTTP {status}): {text}"))
    } else {
        other(format!("{label}è¯·æ±å¤±è´¥(HTTP {status})"))
    }
}

// ==================== Tauri å½ä»¤ ====================

/// å£³çº§è´¦å·åä¾ãä¿ç `pub Arc<Service>` è®¿é®(æ¢æå½ä»¤ç» bz.service ç´è¯»,é¿å
/// å¤§é¢ç§¯éå);å¦å  transport_generation ä»£æ¬¡:è®¾ç½®é¡µåæ¢æå¡å°å/ Basic
/// Auth æ¶æ¨è¿,ä¾ mc_disconnect / mc_models_sync|revoke æ ¡éª"åæç«æ"ââ
/// å¼æ­¥ç­å¾æé´è¥åäºæå¡,æç»ææ§æå¡çç»æè½å°æ°ä¼è¯ä¸ã
pub struct BaizhiState {
    pub service: std::sync::Arc<Service>,
    transport_generation: std::sync::atomic::AtomicU64,
}

impl BaizhiState {
    pub fn new(service: Service) -> Self {
        Self {
            service: std::sync::Arc::new(service),
            transport_generation: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// UI åèµ·æä½æ¶æºå¸¦å®çå°çä»£æ¬¡ãè¥ä¸å½åä¸ç¬¦(æé´åäºæ)è¿å None,
    /// è°ç¨æ¹æ®æ­¤åæ¶æ¬æ¬¡æä½å¹¶éè¯ââç­ä»·äºå®æ¹ service_snapshot_if_currentã
    pub(crate) fn service_snapshot_if_current(
        &self,
        expected_generation: Option<u64>,
    ) -> Option<(std::sync::Arc<Service>, u64)> {
        let generation = self.transport_generation.load(std::sync::atomic::Ordering::SeqCst);
        if expected_generation.is_some_and(|e| e != generation) {
            return None;
        }
        Some((std::sync::Arc::clone(&self.service), generation))
    }

    pub(crate) fn is_current(&self, expected_generation: u64) -> bool {
        self.transport_generation.load(std::sync::atomic::Ordering::SeqCst) == expected_generation
    }

    /// è®¾ç½®é¡µä¿å­ä¸æå¡å°å/ Basic ååæ¶æ¨è¿ä»£æ¬¡(ç± main.rs save_config è°ç¨)ã
    pub fn bump_transport_generation(&self) {
        self.transport_generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// å½åä»£æ¬¡,ä¾ UI è¯»åååä¼ (å®ç°å®æ¹ monkeycode-transport-changed è¯­ä¹)ã
    pub fn transport_generation(&self) -> u64 {
        self.transport_generation.load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn valid_phone(p: &str) -> bool {
    p.len() == 11 && p.starts_with('1') && p.bytes().all(|b| b.is_ascii_digit()) && (b'3'..=b'9').contains(&p.as_bytes()[1])
}

fn valid_code(c: &str) -> bool {
    (4..=6).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_digit())
}

#[tauri::command]
pub async fn baizhi_status(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    let (logged_in, profile) = bz.service.status().await.map_err(BzErr::msg)?;
    let mut resp = json!({ "logged_in": logged_in, "host": bz.service.base_host() });
    if !profile.is_null() {
        resp["profile"] = profile;
    }
    Ok(resp)
}

#[tauri::command]
pub async fn baizhi_send_code(bz: State<'_, BaizhiState>, phone: String) -> Result<Value, String> {
    if !valid_phone(&phone) {
        return Err("è¯·è¾å¥ææçææºå·".into());
    }
    bz.service.send_phone_code(&phone).await.map_err(BzErr::msg)?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub async fn baizhi_login(bz: State<'_, BaizhiState>, phone: String, code: String) -> Result<Value, String> {
    if !valid_phone(&phone) || !valid_code(&code) {
        return Err("è¯·è¾å¥ææçææºå·åç­ä¿¡éªè¯ç ".into());
    }
    bz.service.login_phone(&phone, &code).await.map_err(BzErr::msg)?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub async fn baizhi_logout(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    bz.service.store.clear();
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub async fn baizhi_wechat_start(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    let qr = wechat::start_wechat_login(&bz.service).await.map_err(BzErr::msg)?;
    Ok(json!({ "qr": qr }))
}

#[tauri::command]
pub async fn baizhi_wechat_poll(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    let status = wechat::poll_wechat_login(&bz.service).await.map_err(BzErr::msg)?;
    Ok(json!({ "status": status }))
}

#[tauri::command]
pub async fn baizhi_sync(bz: State<'_, BaizhiState>, known_keys: Option<Vec<String>>) -> Result<Value, String> {
    sync::sync(&bz.service, &known_keys.unwrap_or_default()).await.map_err(BzErr::msg)
}

#[tauri::command]
pub async fn mc_status(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    let (logged_in, user) = monkeycode::mc_status(&bz.service).await.map_err(BzErr::msg)?;
    let mut resp = json!({
        "logged_in": logged_in,
        "host": monkeycode::mc_host(&bz.service),
        "base_url": bz.service.ep.monkeycode,
    });
    if !user.is_null() {
        resp["user"] = user;
    }
    Ok(resp)
}

#[tauri::command]
pub async fn mc_login(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    let user = monkeycode::login_monkeycode(&bz.service).await.map_err(BzErr::msg)?;
    let mut resp = json!({ "ok": true });
    if !user.is_null() {
        resp["user"] = user;
    }
    Ok(resp)
}

/// MonkeyCode è´¦å·å¯ç ç´è¿ç»å½(ä¸ç»ç¾æºäº;å£³åèªå¨å®æ PoW éªè¯ç )ã
/// æ ¡éªå¯¹é½ mobile çå¼±æ ¡éª:ä»éç©º;password **ä¸ trim**ââé¦å°¾ç©ºæ ¼æ¯
/// å¯ç çä¸é¨å,trim ä¼ä¸ mobile/web è¡ä¸ºååã
#[tauri::command]
pub async fn mc_password_login(bz: State<'_, BaizhiState>, email: String, password: String) -> Result<Value, String> {
    let email = email.trim();
    if email.is_empty() || password.is_empty() {
        return Err("è¯·è¾å¥é®ç®±åå¯ç ".into());
    }
    let user = monkeycode::login_monkeycode_password(&bz.service, email, &password)
        .await
        .map_err(BzErr::msg)?;
    let mut resp = json!({ "ok": true });
    if !user.is_null() {
        resp["user"] = user;
    }
    Ok(resp)
}

#[tauri::command]
pub async fn mc_logout(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    bz.service.mc.clear();
    Ok(json!({ "ok": true }))
}

/// è´¦å·æç(é¢åº¦/ä¼å/ç­¾å°æ/éè¯·ä¸æ¬¡åå;åè·¯ç¼ºå¸­æ null éçº§,è§ mc_usage)ã
#[tauri::command]
pub async fn mc_usage(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    monkeycode::mc_usage(&bz.service).await.map_err(BzErr::msg)
}

/// æ¯æ¥ç­¾å°(å£³åèªå¨å®æ PoW éªè¯ç )ãæåå UI éæ mc_usage å·æ°ä½é¢ã
#[tauri::command]
pub async fn mc_checkin(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    monkeycode::mc_checkin(&bz.service).await.map_err(BzErr::msg)?;
    Ok(json!({ "ok": true }))
}

// ==================== ä¼åæ¨¡åæ¬å°åæ­¥ ====================

pub(crate) const OHMYAGENT_KEY_FILE: &str = "monkeycode-ohmyagent-key.json";

/// å·²è½ççè®°å½(è¦æ± id ä¸ api_key é½å¨,æåè§ä¸ºæ )ã
pub(crate) fn stored_ohmyagent_key(cfg_dir: &std::path::Path) -> Option<Value> {
    let data = std::fs::read(cfg_dir.join(OHMYAGENT_KEY_FILE)).ok()?;
    let v: Value = serde_json::from_slice(&data).ok()?;
    let has = |k: &str| v.get(k).and_then(Value::as_str).map(|s| !s.is_empty()).unwrap_or(false);
    (has("id") && has("api_key")).then_some(v)
}

/// åæ¬æºè®°å½,æ²¡æå°±åå»ºå¹¶ç«å»è½çã`server` åå(åæ¢äºæå¡å°å)ä½åº
/// éå»º,ä» `base_url` ååååå°å·æ°;æ§æä»¶ç¼º `server` æ¶æ base_url å®½æ¾å¤å®ã
async fn ensure_ohmyagent_key(svc: &Service, cfg_dir: &std::path::Path) -> BzResult<Value> {
    let server = svc.ep.monkeycode.as_str();
    let llm = svc.mc_llm.as_str();
    let persist = |k: &Value| {
        crate::config::atomic_write_private(&cfg_dir.join(OHMYAGENT_KEY_FILE), k.to_string().as_bytes())
            .map_err(other)
    };
    if let Some(mut k) = stored_ohmyagent_key(cfg_dir) {
        let stored_server = k.get("server").and_then(Value::as_str).unwrap_or("").to_string();
        let stored_llm = k.get("base_url").and_then(Value::as_str).unwrap_or("").to_string();
        let same_server = if stored_server.is_empty() {
            stored_llm.is_empty() || stored_llm == llm // æ§æä»¶:å­ base_url å®½æ¾å¤å®
        } else {
            stored_server == server
        };
        if same_server {
            if stored_server != server || stored_llm != llm {
                k["server"] = json!(server);
                k["base_url"] = json!(llm);
                persist(&k)?;
            }
            return Ok(k);
        }
        // æå¡å¨å·²åæ¢,è½å°ä¸æ¹éå»º
    }
    let mut k = monkeycode::mc_ohmyagent_key_create(svc).await?;
    k["server"] = json!(server);
    k["base_url"] = json!(llm);
    persist(&k)?;
    Ok(k)
}

/// åæ­¥ä¼ååç½®æ¨¡å(å½ä»¤çå¯æµåæ ¸:tests.rs ä»¥ TempDir ç´è°)ã
pub(crate) async fn sync_member_models(svc: &Service, cfg_dir: &std::path::Path) -> BzResult<Value> {
    ensure_ohmyagent_key(svc, cfg_dir).await?;
    monkeycode::mc_member_models_sync(svc).await
}

/// å æåæç§»é¤æ¬å°è®°å½;å å¤±è´¥(å¦æ­ç½)ä¿çè®°å½,ä¸æ¬¡æ­å¼éè¯å³æ¶æã
pub(crate) async fn revoke_member_models(svc: &Service, cfg_dir: &std::path::Path) -> BzResult<()> {
    let Some(key) = stored_ohmyagent_key(cfg_dir) else {
        return Ok(()); // ä»æªåæ­¥è¿,æ å¯å 
    };
    let id = key.get("id").and_then(Value::as_str).unwrap_or("");
    monkeycode::mc_ohmyagent_key_delete(svc, id).await?;
    let _ = std::fs::remove_file(cfg_dir.join(OHMYAGENT_KEY_FILE));
    Ok(())
}

/// åæ­¥ MonkeyCode ä¼ååç½®æ¨¡åä¸ºæ¬å°æ¡ç®(source="monkeycode")ãä¸
/// baizhi_sync åæ¬¾è¯­ä¹:ä¸ç¢° config.json,çº¯è¿å {models, notes}ã
/// expected_generation:UI çå°çå£³ä»£æ¬¡;è¥ä¿å­æé´åäºæå¡å°åååæ¶,é¿å
/// ææ§æå¡çç»æè½å°æ°ä¼è¯ä¸(ååå¼å®¹:ä¸ä¼  = ä¸æ ¡éª)ã
#[tauri::command]
pub async fn mc_models_sync(
    app: tauri::AppHandle,
    bz: State<'_, BaizhiState>,
    expected_generation: Option<u64>,
) -> Result<Value, String> {
    let cfg_dir = crate::config::config_dir(&app)?;
    if bz.service_snapshot_if_current(expected_generation).is_none() {
        return Err("æå¡éç½®å·²åæ¢,æ¬æ¬¡ä¼åæ¨¡ååæ­¥å·²åæ¶,è¯·éè¯".into());
    }
    sync_member_models(&bz.service, &cfg_dir).await.map_err(BzErr::msg)
}

/// æ­å¼ MonkeyCode è´¦å·æ¶è°ç¨(ä»æªåæ­¥è¿ç´æ¥æå)ãé¡»å¨æ¸é¤ mc ä¼è¯
/// ä¹åè°ç¨ââè¯·æ±èµ° mc ä¼è¯è®¤è¯ã
/// expected_generation:åä¸,åæååæ¶(ååå¼å®¹:ä¸ä¼  = ä¸æ ¡éª)ã
#[tauri::command]
pub async fn mc_models_revoke(
    app: tauri::AppHandle,
    bz: State<'_, BaizhiState>,
    expected_generation: Option<u64>,
) -> Result<Value, String> {
    let cfg_dir = crate::config::config_dir(&app)?;
    if bz.service_snapshot_if_current(expected_generation).is_none() {
        return Err("æå¡éç½®å·²åæ¢,æ¬æ¬¡ä¼åå¯é¥åéå·²åæ¶,è¯·éè¯".into());
    }
    revoke_member_models(&bz.service, &cfg_dir).await.map_err(BzErr::msg)?;
    Ok(json!({ "ok": true }))
}

/// åéä¼å Key + æ¸ä¼è¯çä¸ä½åæ­å¼å½ä»¤ãæ´ä¸ªæµç¨æè·åä¸ transport ä»£æ¬¡;
/// è¥ç­å¾åéæé´åäºæå¡,æåçåå­æ ¡éªä¼æç»æ¸ææ°æå¡ Cookie(å®æ¹
/// mc_disconnect è¯­ä¹,æ­¤å¤ä»¥ generation æ¯å¯¹å®ç°æå°å¯ç¨çæ¬)ã
#[tauri::command]
pub async fn mc_disconnect(
    app: tauri::AppHandle,
    bz: State<'_, BaizhiState>,
    expected_generation: u64,
) -> Result<Value, String> {
    let cfg_dir = crate::config::config_dir(&app)?;
    // ä»£æ¬¡ä¸ç¬¦(æé´åäºæå¡)â åæ¶,ä¸è§¦ç¢°ä»»ä½ä¼è¯ã
    let Some((svc, generation)) = bz.service_snapshot_if_current(Some(expected_generation)) else {
        return Ok(json!({ "ok": false, "cancelled": true }));
    };
    // ååéä¼å Key(èµ° mc ä¼è¯è®¤è¯),å¤±è´¥ä»ä½ warning,ä¸é»æ­æ¸ä¼è¯ã
    let warning = revoke_member_models(&svc, &cfg_dir).await.err().map(BzErr::msg);
    // åæ¸ mc ä¼è¯;äºæ¬¡æ ¡éªä»£æ¬¡,é²æ­¢åéçå¼æ­¥ç­å¾æé´åè¢«åæã
    let current = bz.is_current(generation);
    if current {
        svc.mc.clear();
    }
    Ok(json!({
        "ok": current,
        "cancelled": !current,
        "warning": warning,
    }))
}

#[tauri::command]
pub async fn mc_tasks(
    bz: State<'_, BaizhiState>,
    page: u32,
    size: u32,
    status: Option<String>,
    project_id: Option<String>,
    quick_start: Option<bool>,
) -> Result<Value, String> {
    let size = size.clamp(1, 50);
    let page = page.max(1);
    monkeycode::mc_tasks(
        &bz.service,
        page,
        size,
        status.as_deref().unwrap_or(""),
        project_id.as_deref().unwrap_or(""),
        quick_start,
    )
    .await
    .map_err(BzErr::msg)
}

#[tauri::command]
pub async fn mc_projects(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    monkeycode::mc_projects(&bz.service).await.map_err(BzErr::msg)
}

#[tauri::command]
pub async fn mc_task_info(bz: State<'_, BaizhiState>, id: String) -> Result<Value, String> {
    monkeycode::mc_task_info(&bz.service, &id).await.map_err(BzErr::msg)
}

#[tauri::command]
pub async fn mc_task_rounds(
    bz: State<'_, BaizhiState>,
    id: String,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<Value, String> {
    let limit = limit.unwrap_or(1).clamp(1, 10);
    monkeycode::mc_task_rounds(&bz.service, &id, cursor.as_deref().unwrap_or(""), limit)
        .await
        .map_err(BzErr::msg)
}

#[tauri::command]
pub async fn mc_task_user_inputs(
    bz: State<'_, BaizhiState>,
    id: String,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<Value, String> {
    // åç«¯ä¸é 100;å¤§çº²ä¸æ¬¡å¤æ¿äº,åå°å¨éæåçå¾è¿æ°
    let limit = limit.unwrap_or(100).clamp(1, 100);
    monkeycode::mc_task_user_inputs(&bz.service, &id, cursor.as_deref().unwrap_or(""), limit)
        .await
        .map_err(BzErr::msg)
}

#[tauri::command]
pub async fn mc_task_stop(bz: State<'_, BaizhiState>, id: String) -> Result<Value, String> {
    monkeycode::mc_task_stop(&bz.service, &id).await.map_err(BzErr::msg)?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub async fn mc_task_delete(bz: State<'_, BaizhiState>, id: String) -> Result<Value, String> {
    monkeycode::mc_task_delete(&bz.service, &id).await.map_err(BzErr::msg)?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub async fn mc_task_create(bz: State<'_, BaizhiState>, req: Value) -> Result<Value, String> {
    monkeycode::mc_task_create(&bz.service, &req).await.map_err(BzErr::msg)
}

#[tauri::command]
pub async fn mc_task_options(bz: State<'_, BaizhiState>) -> Result<Value, String> {
    monkeycode::mc_task_options(&bz.service).await.map_err(BzErr::msg)
}

/// äºç«¯èå¤©éä»¶ä¸ä¼ (data = base64 æä»¶å­è);è¿å {access_url}ã
#[tauri::command]
pub async fn mc_upload(bz: State<'_, BaizhiState>, filename: String, data: String) -> Result<Value, String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|e| format!("éä»¶æ°æ®è§£ç å¤±è´¥: {e}"))?;
    let access_url = monkeycode::mc_upload(&bz.service, &filename, bytes).await.map_err(BzErr::msg)?;
    Ok(json!({ "access_url": access_url }))
}

/// èææºç»ç«¯ session åè¡¨(ç»ç«¯é¢æ¿å¤ç¨å·²æä¼è¯ç¨;è¿å {terminals})ã
#[tauri::command]
pub async fn mc_terminal_list(bz: State<'_, BaizhiState>, vm_id: String) -> Result<Value, String> {
    monkeycode::mc_terminal_list(&bz.service, &vm_id).await.map_err(BzErr::msg)
}

/// ä»äºç«¯ä»»å¡ VM å·¥ä½åºä¸è½½æä»¶/ç®å½å°æ¬å°(dest ä¸º UI ç»ä¿å­å¯¹è¯æ¡
/// éå®çæ¬å°è·¯å¾;ç®å½ç±æå¡ç«¯ææ zip)ãdl_id ç± UI çæ,è¿åº¦ç»
/// `dl-progress:{dl_id}` äºä»¶ä¸æ¥,åæ¶èµ° mc_file_download_cancelã
/// è¿å {ok, bytes}ã
#[tauri::command]
pub async fn mc_file_download(
    app: tauri::AppHandle,
    bz: State<'_, BaizhiState>,
    ctl: State<'_, monkeycode::DownloadCtl>,
    dl_id: String,
    vm_id: String,
    path: String,
    filename: String,
    dest: String,
) -> Result<Value, String> {
    let bytes = monkeycode::mc_file_download(&app, &ctl, &bz.service, &dl_id, &vm_id, &path, &filename, &dest)
        .await
        .map_err(BzErr::msg)?;
    Ok(json!({ "ok": true, "bytes": bytes }))
}

/// åæ¶è¿è¡ä¸­çä¸è½½(ç½®æ,ç±ä¸è½½å¾ªç¯å¨åé´æ¶æå¹¶æ¸æ®ä»¶;å·²å®æ/ä¸å­å¨
/// éé»ââåæ¶ä¸å®æå¤©ç¶èµè·)ã
#[tauri::command]
pub async fn mc_file_download_cancel(
    ctl: State<'_, monkeycode::DownloadCtl>,
    dl_id: String,
) -> Result<Value, String> {
    ctl.cancel(&dl_id);
    Ok(json!({ "ok": true }))
}

/// ä¸ä¼ æä»¶å°äºç«¯ä»»å¡ VM å·¥ä½åº(path ä¸º VM åç»å¯¹è·¯å¾,data = base64 æä»¶å­è)ã
#[tauri::command]
pub async fn mc_file_upload(
    bz: State<'_, BaizhiState>,
    vm_id: String,
    path: String,
    data: String,
) -> Result<Value, String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|e| format!("æä»¶æ°æ®è§£ç å¤±è´¥: {e}"))?;
    monkeycode::mc_file_upload(&bz.service, &vm_id, &path, bytes).await.map_err(BzErr::msg)?;
    Ok(json!({ "ok": true }))
}
