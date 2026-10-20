//! LsmAgentBrowser 网络栈:主文档获取、重定向、Cookie、字符集、大小上限。
//!
//! 不使用任何外部浏览器/CDP。所有请求由本进程内的 `reqwest` 发出。

use std::time::{Duration, Instant};

use crate::agent::browser::storage::CookieJar;
use crate::agent::browser::urlutil;

/// 单次请求参数。
#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: u64,
    /// 响应体上限(字节)。主文档默认 32MB,下载可放宽。
    pub max_bytes: usize,
}

impl FetchRequest {
    pub fn get(url: &str, timeout_ms: u64) -> Self {
        Self {
            url: url.to_string(),
            method: "GET".to_string(),
            headers: Vec::new(),
            body: None,
            timeout_ms,
            max_bytes: 32 * 1024 * 1024,
        }
    }
}

/// 单次请求结果。
#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub final_url: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub bytes: Vec<u8>,
    pub redirect_chain: Vec<String>,
    pub elapsed_ms: u64,
}

impl FetchResponse {
    pub fn content_type(&self) -> String {
        header_value(&self.headers, "content-type").unwrap_or_default()
    }

    pub fn is_html(&self) -> bool {
        let ct = self.content_type().to_ascii_lowercase();
        ct.is_empty() || ct.contains("html") || ct.contains("xml")
    }

    pub fn text(&self) -> String {
        urlutil::decode_body(&self.bytes, &self.content_type())
    }
}

pub fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

/// HTTP 客户端(严格 + 宽松 TLS 两份)。
pub struct HttpClient {
    strict: reqwest::Client,
    loose: reqwest::Client,
    user_agent: String,
}

impl HttpClient {
    pub fn new(user_agent: &str) -> Self {
        let strict = build_client(user_agent, false);
        let loose = build_client(user_agent, true);
        Self {
            strict,
            loose,
            user_agent: user_agent.to_string(),
        }
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    fn client_for(&self, url: &str) -> &reqwest::Client {
        // 与 LLM 端点共用 TLS 策略:IP 主机自动放宽,LAEW_TLS_INSECURE 可全局覆盖。
        if crate::llm::tls_insecure_for(url) {
            &self.loose
        } else {
            &self.strict
        }
    }

    /// 获取主文档/资源,自动跟随重定向。
    pub async fn fetch(
        &self,
        req: FetchRequest,
        jar: &mut CookieJar,
    ) -> Result<FetchResponse, String> {
        let started = Instant::now();
        let mut current = normalize_url(&req.url)?;
        let mut method = req.method.to_ascii_uppercase();
        let mut body = req.body.clone();
        let mut chain = vec![current.clone()];
        let headers = req.headers.clone();

        for hop in 0..=10 {
            // 本地协议:不走网络。
            if let Some(local) = local_fetch(&current) {
                return Ok(FetchResponse {
                    final_url: current,
                    status: local.0,
                    headers: local.1,
                    bytes: local.2,
                    redirect_chain: chain,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                });
            }
            if let Err(e) = crate::agent::safety::url_safety::is_safe_endpoint(&current) {
                return Err(format!(
                    "目标地址被 SSRF 防护拦截:{e}(本地/内网测试可设 LAEW_ALLOW_PRIVATE_ENDPOINT=1)"
                ));
            }

            let client = self.client_for(&current);
            let mut builder = client
                .request(
                    reqwest::Method::from_bytes(method.as_bytes())
                        .map_err(|e| format!("非法 HTTP 方法 {method}: {e}"))?,
                    &current,
                )
                .timeout(Duration::from_millis(req.timeout_ms.max(1_000)));
            for (k, v) in &headers {
                if let (Ok(name), Ok(value)) = (
                    reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                    reqwest::header::HeaderValue::from_str(v),
                ) {
                    builder = builder.header(name, value);
                }
            }
            if let Some(cookie) = jar.header_for(&current) {
                builder = builder.header(reqwest::header::COOKIE, cookie);
            }
            if let Some(b) = &body {
                builder = builder.body(b.clone());
            }
            let resp = builder
                .send()
                .await
                .map_err(|e| format!("请求失败 {}:{e}", short_url(&current)))?;

            let status = resp.status().as_u16();
            let resp_headers: Vec<(String, String)> = resp
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        v.to_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            jar.set_from_headers(&current, &resp_headers);

            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                if let Some(loc) = header_value(&resp_headers, "location") {
                    if hop >= 10 {
                        return Err("重定向次数超过上限(10)".to_string());
                    }
                    let next = urlutil::resolve(&current, &loc)
                        .ok_or_else(|| format!("无法解析重定向地址:{loc}"))?;
                    // 浏览器语义:303 一律 GET;301/302 的 POST 降为 GET;307/308 保持。
                    if status == 303 || (matches!(status, 301 | 302) && method == "POST") {
                        method = "GET".to_string();
                        body = None;
                    }
                    current = normalize_url(&next)?;
                    chain.push(current.clone());
                    continue;
                }
            }

            let declared = resp.content_length().unwrap_or(0) as usize;
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| format!("读取响应失败:{e}"))?
                .to_vec();
            if bytes.len() > req.max_bytes || declared > req.max_bytes {
                return Err(format!(
                    "响应体超过上限({} MB),已取消",
                    req.max_bytes / 1024 / 1024
                ));
            }
            return Ok(FetchResponse {
                final_url: current,
                status,
                headers: resp_headers,
                bytes,
                redirect_chain: chain,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }
        Err("重定向次数超过上限(10)".to_string())
    }
}

fn build_client(ua: &str, insecure: bool) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(20))
        .user_agent(ua);
    if insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().unwrap_or_else(|_| reqwest::Client::new())
}

fn normalize_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("URL 为空".to_string());
    }
    if let Ok(u) = url::Url::parse(raw) {
        return Ok(u.to_string());
    }
    url::Url::parse(&format!("https://{raw}"))
        .map(|u| u.to_string())
        .map_err(|e| format!("URL 非法:{raw}({e})"))
}

/// 本地协议处理:`about:blank`、`data:`、`file:`。
fn local_fetch(url: &str) -> Option<(u16, Vec<(String, String)>, Vec<u8>)> {
    if url == "about:blank" || url.starts_with("about:") {
        return Some((
            200,
            vec![("content-type".into(), "text/html; charset=utf-8".into())],
            b"<!doctype html><html><head><title>about:blank</title></head><body></body></html>"
                .to_vec(),
        ));
    }
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, payload) = rest.split_once(',').unwrap_or((rest, ""));
        let base64 = meta.ends_with(";base64");
        let bytes = if base64 {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(payload)
                .unwrap_or_default()
        } else {
            percent_decode(payload).into_bytes()
        };
        let ctype = meta
            .split(';')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("text/plain");
        return Some((
            200,
            vec![("content-type".into(), format!("{ctype}; charset=utf-8"))],
            bytes,
        ));
    }
    if let Some(path) = url.strip_prefix("file://") {
        let path = percent_decode(path);
        let path = std::path::PathBuf::from(path);
        // 只允许读取启动工作目录内的文件,防止越界读取。
        let cwd = std::env::current_dir().ok()?;
        let canonical = path.canonicalize().ok()?;
        if !canonical.starts_with(cwd.canonicalize().ok()?) {
            return Some((
                403,
                vec![("content-type".into(), "text/plain; charset=utf-8".into())],
                b"file:// access outside working directory is denied".to_vec(),
            ));
        }
        let bytes = std::fs::read(&canonical).ok()?;
        let ctype = guess_content_type(&canonical);
        return Some((200, vec![("content-type".into(), ctype)], bytes));
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn guess_content_type(path: &std::path::Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8".into(),
        "css" => "text/css; charset=utf-8".into(),
        "js" => "text/javascript; charset=utf-8".into(),
        "json" => "application/json".into(),
        "png" => "image/png".into(),
        "jpg" | "jpeg" => "image/jpeg".into(),
        "gif" => "image/gif".into(),
        "svg" => "image/svg+xml".into(),
        "txt" => "text/plain; charset=utf-8".into(),
        _ => "application/octet-stream".into(),
    }
}

fn short_url(url: &str) -> String {
    url.chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_url_解析() {
        let out = local_fetch("data:text/html,<h1>hi</h1>").unwrap();
        assert_eq!(out.0, 200);
        assert!(String::from_utf8_lossy(&out.2).contains("hi"));
    }

    #[test]
    fn data_url_base64() {
        let out = local_fetch("data:text/plain;base64,aGVsbG8=").unwrap();
        assert_eq!(out.2, b"hello");
    }

    #[test]
    fn about_blank_返回空页() {
        let out = local_fetch("about:blank").unwrap();
        assert!(String::from_utf8_lossy(&out.2).contains("<html>"));
    }

    #[test]
    fn 裸域名补协议() {
        assert_eq!(normalize_url("example.com").unwrap(), "https://example.com/");
    }
}
