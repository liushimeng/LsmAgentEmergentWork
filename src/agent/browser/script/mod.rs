//! 阶段二:页面脚本执行管线与动态 DOM。
//!
//! 每个标签页一条专用 JS 线程持有持久 Boa Context(全局变量跨 `eval_js` 保留);
//! DOM 仍在宿主侧(kuchikiki),结构性修改以 action 回传,由 `dom_ops` 应用后
//! 重新序列化回 `tab.html`。设计见 `docs/自定义浏览器和V8引擎/05-阶段二页面脚本与动态DOM设计.md`。

pub mod dom_ops;
mod prelude;
pub mod runtime;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::time::Duration;

pub use super::js::{JsAction, JsLog, JsOutcome};

/// 页面环境(owned 版 `js::JsPageMeta`),随每次请求注入线程。
#[derive(Debug, Clone)]
pub struct PageEnv {
    pub url: String,
    pub title: String,
    pub cookies: String,
    pub local_storage: HashMap<String, String>,
    pub session_storage: HashMap<String, String>,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub user_agent: String,
}

/// 脚本来源:inline 文本或宿主预取的外链内容。
#[derive(Debug, Clone)]
pub struct ScriptSource {
    /// `inline` | `external`
    pub kind: &'static str,
    pub content: String,
    pub src_url: String,
}

/// JS 线程发起的网络请求记录(回传宿主进台账)。
#[derive(Debug, Clone, Default)]
pub struct NetRecordLite {
    pub url: String,
    pub method: String,
    pub status: u16,
    pub content_type: String,
    pub bytes: usize,
    pub elapsed_ms: u64,
    pub redirect: bool,
}

/// 线程回复。
#[derive(Debug, Clone)]
pub struct JsReply {
    pub outcome: JsOutcome,
    pub network: Vec<NetRecordLite>,
    /// Dispatch 请求:是否有 listener 命中。
    pub handled: bool,
    /// Dispatch 请求:是否 preventDefault。
    pub default_prevented: bool,
}

impl JsReply {
    fn from_outcome(outcome: JsOutcome) -> Self {
        Self {
            outcome,
            network: Vec::new(),
            handled: false,
            default_prevented: false,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        let mut outcome = JsOutcome::default();
        outcome.ok = false;
        outcome.error = Some(message.into());
        Self::from_outcome(outcome)
    }
}

/// 发给 JS 线程的请求。
pub enum JsRequest {
    /// 页面加载:依序执行脚本 + 派发 DOMContentLoaded/load + 跑事件循环。
    RunScripts {
        scripts: Vec<ScriptSource>,
        env: PageEnv,
        nodes: Vec<super::document::SnapshotNode>,
        reply: std::sync::mpsc::Sender<JsReply>,
    },
    /// Agent 的 eval_js:在持久 Context 上执行用户表达式。
    Eval {
        script: String,
        env: PageEnv,
        nodes: Vec<super::document::SnapshotNode>,
        reply: std::sync::mpsc::Sender<JsReply>,
    },
    /// 宿主交互事件(click/input/change/submit)分发到页面 listener。
    Dispatch {
        selector: String,
        event: String,
        env: PageEnv,
        nodes: Vec<super::document::SnapshotNode>,
        reply: std::sync::mpsc::Sender<JsReply>,
    },
    Shutdown,
}

/// 标签页持有的 JS 线程句柄。`mpsc::Sender` 本身 Clone + Debug。
#[derive(Debug, Clone)]
pub struct JsHandle {
    tx: std::sync::mpsc::Sender<JsRequest>,
}

impl JsHandle {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<JsRequest>();
        let builder = std::thread::Builder::new().name("lsm-page-js".to_string());
        let _ = builder.spawn(move || runtime::thread_main(rx));
        Self { tx }
    }

    /// 同步请求(调用方须在 `spawn_blocking` 中)。超时返回 Err,句柄应视为失效。
    pub fn request(
        &self,
        make: impl FnOnce(std::sync::mpsc::Sender<JsReply>) -> JsRequest,
    ) -> Result<JsReply, String> {
        let (tx_reply, rx) = std::sync::mpsc::channel::<JsReply>();
        self.tx
            .send(make(tx_reply))
            .map_err(|_| "JS 线程已退出".to_string())?;
        rx.recv_timeout(Duration::from_millis(budget_ms() + 2_000))
            .map_err(|_| "JS 执行超时(线程预算耗尽)".to_string())
    }

    /// 请求线程退出(导航换页/关页)。
    pub fn shutdown(&self) {
        let _ = self.tx.send(JsRequest::Shutdown);
    }
}

/// 页面脚本总开关。`LAEW_WEB_SCRIPT=off` 关闭后 eval_js 回退阶段一快照桥。
pub fn scripts_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_SCRIPT").unwrap_or_default().trim(),
        "off" | "0" | "false" | "no"
    )
}

/// 单次脚本管线墙钟预算(毫秒)。
pub fn budget_ms() -> u64 {
    std::env::var("LAEW_WEB_SCRIPT_BUDGET_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v >= 500)
        .unwrap_or(10_000)
}

/// 脚本管线产物:回写宿主前的中间结果。
#[derive(Debug, Clone, Default)]
pub struct PipelineOut {
    pub html: String,
    pub title: Option<String>,
    pub console: Vec<super::ConsoleRecord>,
    pub network: Vec<super::NetworkRecord>,
    pub js: Option<JsHandle>,
    pub form_values: HashMap<String, String>,
}

const MAX_ROUNDS: usize = 3;
const MAX_EXTERNAL_BYTES: usize = 2 * 1024 * 1024;
const SCRIPT_FETCH_TIMEOUT_MS: u64 = 15_000;

/// 页面加载脚本管线:收集(内联+外链预取)→ 线程执行 → 动作应用 → 回写 HTML。
///
/// 多轮上限 `MAX_ROUNDS`:脚本注入的新 `<script>`(按内容指纹去重)继续执行。
/// 持有 `inner` 锁贯穿全程(与主文档取回的既有行为一致)。
pub(crate) async fn run_pipeline(
    inner: &mut super::Inner,
    http: &super::net::HttpClient,
    raw_html: &str,
    base_url: &str,
    ua: &str,
    viewport: (u32, u32),
    session_storage: &HashMap<String, String>,
) -> PipelineOut {
    let mut out = PipelineOut {
        html: raw_html.to_string(),
        ..Default::default()
    };
    if !scripts_enabled() {
        return out;
    }
    let mut executed = std::collections::HashSet::new();
    let mut handle: Option<JsHandle> = None;
    let title_now = {
        let doc = super::document::Document::parse(raw_html, base_url);
        doc.title()
    };
    for round in 0..MAX_ROUNDS {
        let collected = {
            // Document(NodeRef)非 Send,收集完立即销毁,不跨 await 存活。
            let doc = super::document::Document::parse(&out.html, base_url);
            dom_ops::collect_scripts(&doc, base_url, &mut executed)
        };
        if collected.inline.is_empty() && collected.external_urls.is_empty() {
            break;
        }
        let mut scripts = collected.inline;
        for url in &collected.external_urls {
            let mut req = super::net::FetchRequest::get(url, SCRIPT_FETCH_TIMEOUT_MS);
            req.max_bytes = MAX_EXTERNAL_BYTES;
            if ua != super::DEFAULT_UA {
                req.headers.push(("user-agent".to_string(), ua.to_string()));
            }
            match http.fetch(req, &mut inner.cookies).await {
                Ok(resp) => {
                    let text = resp.text();
                    if text.len() <= MAX_EXTERNAL_BYTES && !text.trim().is_empty() {
                        out.network.push(super::NetworkRecord {
                            url: url.clone(),
                            method: "GET".to_string(),
                            status: resp.status,
                            content_type: resp.content_type(),
                            bytes: resp.bytes.len(),
                            elapsed_ms: resp.elapsed_ms,
                            redirect: resp.redirect_chain.len() > 1,
                            at_ms: super::now_ms(),
                        });
                        scripts.push(ScriptSource {
                            kind: "external",
                            content: text,
                            src_url: url.clone(),
                        });
                    }
                }
                Err(e) => {
                    out.console.push(super::ConsoleRecord {
                        level: "warn".to_string(),
                        text: format!("外链脚本加载失败 {url}:{e}"),
                        at_ms: super::now_ms(),
                    });
                }
            }
        }
        if scripts.is_empty() {
            break;
        }
        if handle.is_none() {
            handle = Some(JsHandle::spawn());
        }
        let Some(h) = handle.clone() else { break };
        let env = build_env(inner, base_url, &title_now, ua, viewport, session_storage);
        // 快照后立即销毁 Document(NodeRef 非 Send,不能跨 await 存活)。
        let nodes = {
            let doc = super::document::Document::parse(&out.html, base_url);
            doc.snapshot_nodes()
        };
        let reply: Result<JsReply, String> = match tokio::task::spawn_blocking(move || {
            h.request(|tx| JsRequest::RunScripts {
                scripts,
                env,
                nodes,
                reply: tx,
            })
        })
        .await
        {
            Ok(Ok(r)) => Ok(r),
            _ => Err("脚本执行超时或线程不可用".to_string()),
        };
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                out.console.push(super::ConsoleRecord {
                    level: "error".to_string(),
                    text: format!("页面脚本管线中断:{e}"),
                    at_ms: super::now_ms(),
                });
                handle = None;
                break;
            }
        };
        let doc = super::document::Document::parse(&out.html, base_url);
        append_reply(&mut out, inner, base_url, &reply, &doc);
        out.html = doc.serialize_full();
        if round + 1 < MAX_ROUNDS {
            // 下一轮 collect_scripts 会检测是否有新注入的脚本。
            continue;
        }
        break;
    }
    out.js = handle;
    out
}

/// 组装线程请求用的 PageEnv(读 inner 的 Cookie/存储快照)。
pub(crate) fn build_env(
    inner: &super::Inner,
    url: &str,
    title: &str,
    ua: &str,
    viewport: (u32, u32),
    session_storage: &HashMap<String, String>,
) -> PageEnv {
    PageEnv {
        url: url.to_string(),
        title: title.to_string(),
        cookies: inner.cookies.header_for(url).unwrap_or_default(),
        local_storage: inner
            .local_storage
            .get(&super::urlutil::storage_origin(url))
            .cloned()
            .unwrap_or_default(),
        session_storage: session_storage.clone(),
        viewport_width: viewport.0,
        viewport_height: viewport.1,
        scroll_x: 0.0,
        scroll_y: 0.0,
        user_agent: ua.to_string(),
    }
}

/// 把线程回复中的动作/日志/网络合并进管线产物与 inner 存储;返回导航类待执行动作。
pub(crate) fn append_reply(
    out: &mut PipelineOut,
    inner: &mut super::Inner,
    base_url: &str,
    reply: &JsReply,
    doc: &super::document::Document,
) -> Vec<JsAction> {
    let summary = dom_ops::apply(doc, &reply.outcome.actions, base_url);
    for line in &summary.cookies {
        inner
            .cookies
            .set_from_headers(base_url, &[("set-cookie".to_string(), line.clone())]);
    }
    let origin = super::urlutil::storage_origin(base_url);
    for (k, v) in &summary.storage_local {
        inner
            .local_storage
            .entry(origin.clone())
            .or_default()
            .insert(k.clone(), v.clone());
    }
    for k in &summary.storage_local_remove {
        if let Some(m) = inner.local_storage.get_mut(&origin) {
            m.remove(k);
        }
    }
    if summary.storage_local_clear {
        if let Some(m) = inner.local_storage.get_mut(&origin) {
            m.clear();
        }
    }
    for log in &reply.outcome.logs {
        out.console.push(super::ConsoleRecord {
            level: log.level.clone(),
            text: log.text.clone(),
            at_ms: super::now_ms(),
        });
        if out.console.len() > 400 {
            out.console.remove(0);
        }
    }
    if let Some(title) = &summary.title {
        out.title = Some(title.clone());
    }
    out.form_values.extend(summary.form_values);
    for rec in &reply.network {
        out.network.push(super::NetworkRecord {
            url: rec.url.clone(),
            method: if rec.method.is_empty() { "FETCH".to_string() } else { rec.method.clone() },
            status: rec.status,
            content_type: rec.content_type.clone(),
            bytes: rec.bytes,
            elapsed_ms: rec.elapsed_ms,
            redirect: rec.redirect,
            at_ms: super::now_ms(),
        });
    }
    reply
        .outcome
        .actions
        .iter()
        .filter(|a| {
            matches!(
                a.kind.as_str(),
                "click" | "navigate" | "new_tab" | "submit" | "back" | "forward" | "reload"
            )
        })
        .cloned()
        .collect()
}
