//! 阶段二:页面脚本执行管线与动态 DOM。
//!
//! 每个标签页一条专用 JS 线程持有持久 Boa Context(全局变量跨 `eval_js` 保留);
//! DOM 仍在宿主侧(kuchikiki),结构性修改以 action 回传,由 `dom_ops` 应用后
//! 重新序列化回 `tab.html`。设计见 `docs/自定义浏览器和V8引擎/05-阶段二页面脚本与动态DOM设计.md`。

pub mod amd_fill;
pub mod diag;
pub mod dom_ops;
pub mod env_js;
pub mod for_in_guard;
pub mod prelude;
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
    /// `prefers-color-scheme: dark` 求值结果(matchMedia 用)。
    pub prefers_dark: bool,
    /// 当前页 CSP 快照（阶段四第二步：fetch/XHR connect-src 门禁）。
    pub csp: Option<super::security::CspPolicy>,
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
        /// JS 侧墙钟预算(0 = 用 `budget_ms()` 默认值)。页面首屏脚本总量
        /// 可能远超 eval_js 的 10s;若沿用默认值,`event_loop` 在脚本执行完
        /// 时已过 deadline,懒加载定时器(百度地图 `cG.load`)全部不触发。
        budget_ms: u64,
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
    /// 语句级错误定位(调试用):在持久 Context 上对给定源码做前缀重放二分。
    Diag {
        source: String,
        origin: String,
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
        // 页面脚本线程需要较大的栈:Boa 0.22 解析/求值 20 万字符级压缩脚本
        // (百度地图 initmap/反爬脚本)时递归深度显著高于默认 2MB,实测会
        // stack overflow 直接 abort 整个进程(不可 catch)。
        let builder = std::thread::Builder::new()
            .name("lsm-page-js".to_string())
            .stack_size(32 * 1024 * 1024);
        let _ = builder.spawn(move || runtime::thread_main(rx));
        Self { tx }
    }

    /// 同步请求(调用方须在 `spawn_blocking` 中)。超时返回 Err,句柄应视为失效。
    pub fn request(
        &self,
        make: impl FnOnce(std::sync::mpsc::Sender<JsReply>) -> JsRequest,
    ) -> Result<JsReply, String> {
        self.request_with_budget(make, budget_ms())
    }

    /// 带显式等待预算的请求。页面脚本管线把它绑定到**管线剩余墙钟**:
    /// 重站点(百度地图首屏 60+ 脚本)单轮执行远超 eval_js 的 10s 默认预算,
    /// 旧实现用同一默认值等 RunScripts,导致管线在脚本中途被判定超时,
    /// 装载器/反爬脚本链断在中间(页面只剩外壳)。eval_js 交互仍走默认预算,
    /// 不让单次页面求值把工具调用拖到分钟级。
    pub fn request_with_budget(
        &self,
        make: impl FnOnce(std::sync::mpsc::Sender<JsReply>) -> JsRequest,
        wait_ms: u64,
    ) -> Result<JsReply, String> {
        let (tx_reply, rx) = std::sync::mpsc::channel::<JsReply>();
        self.tx
            .send(make(tx_reply))
            .map_err(|_| "JS 线程已退出".to_string())?;
        rx.recv_timeout(Duration::from_millis(wait_ms + 2_000))
            .map_err(|_| "JS 执行超时(线程预算耗尽)".to_string())
    }

    /// 对给定源码做语句级定位(需在 `spawn_blocking` 中调用)。
    /// 诊断是离线分析,前缀重放次数远高于常规请求,故单独放宽墙钟。
    pub fn diag(&self, source: String, origin: String) -> Result<Vec<String>, String> {
        let (tx_reply, rx_reply) = std::sync::mpsc::channel::<JsReply>();
        self.tx
            .send(JsRequest::Diag {
                source,
                origin,
                reply: tx_reply,
            })
            .map_err(|_| "JS 线程已退出".to_string())?;
        let reply = rx_reply
            .recv_timeout(Duration::from_secs(600))
            .map_err(|_| "JS 诊断超时(600s)".to_string())?;
        serde_json::from_str::<Vec<String>>(&reply.outcome.value)
            .map_err(|e| format!("diag 结果非法:{e}"))
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

/// AMD 依赖数组静态补齐开关。`LAEW_AMD_FILL=off` 关闭。
///
/// 补齐是为了绕开 Boa `fn.toString()` 只返回壳、装载器源码级依赖解析落空的缺陷;
/// 但 FIS 系装载器(`define(id, factory)` 同步注册)只认两参形态,补成三参会被
/// 转交给 RequireJS 分支,模块进 globalDefQueue 而永远进不了同步注册表
/// (实测百度地图 pano 模块 "has not been loaded yet")。排查时可用本开关二分。
pub fn amd_fill_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_AMD_FILL").unwrap_or_default().trim(),
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
    /// 已执行脚本指纹(内联内容/外链 URL)。交互后补齐懒加载脚本时作为种子,
    /// 避免把首屏脚本整批重跑一遍。
    pub executed: std::collections::HashSet<u64>,
    /// 页面申请过 WebGL 上下文的次数(>0 = 站点依赖 WebGL 绘制,内核无 WebGL
    /// → 该区域画不出,须如实告知调用方,别让它反复重试)。
    pub webgl_asked: u32,
}

/// 管线续跑种子。交互(eval_js / click 等)后页面可能注入新的 `<script>`
/// (FIS `require.async` 懒加载 widget 模块),必须沿用**同一个 JS 线程**与
/// 已执行指纹集合继续跑,否则要么重复执行首屏脚本、要么新模块永远不加载。
#[derive(Default)]
pub(crate) struct PipelineSeed {
    pub handle: Option<JsHandle>,
    pub executed: std::collections::HashSet<u64>,
    /// 0 表示使用默认 `MAX_ROUNDS`。
    pub max_rounds: usize,
    /// 0 表示使用默认 `pipeline_budget_ms()`。
    pub budget_ms: u64,
}

/// 脚本管线轮次上限。
///
/// 每一轮把"上一轮注入的 `<script>`"取回并执行;AMD 装载器会连续注入多个模块
/// 脚本(每个模块一轮),3 轮远远不够,页面 widget 层会卡在"等模块"状态。
/// 轮次本身很便宜(`collect_scripts` 按内容指纹/URL 去重,已执行过的脚本不再取回),
/// 配合墙钟预算即可。真实大站(百度地图)的 widget 层注入量实测超过 24 轮,
/// 放宽到 64。
const MAX_ROUNDS: usize = 64;
/// 脚本采完后允许的「空轮」上限:用来把页面定时器/rAF 排空(百度地图瓦片淡入
/// 依赖 img load 后的定时器)。连续这么多轮既无新脚本也无新动作即收工。
const MAX_IDLE_ROUNDS: u32 = 8;
/// 整条脚本管线的墙钟预算(毫秒),可用 `LAEW_WEB_SCRIPT_PIPELINE_MS` 覆盖。
// 整条脚本管线(多轮取回执行)的墙钟预算。40s 对百度地图这类首屏 60+ 脚本、
// 含 195KB 反爬脚本的站点不够:实测在脚本链中途被截断,地图内核不启动。
const PIPELINE_BUDGET_MS: u64 = 120_000;

/// 管线墙钟预算(环境变量覆盖,下限 2000ms)。
fn pipeline_budget_ms() -> u64 {
    std::env::var("LAEW_WEB_SCRIPT_PIPELINE_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v >= 2_000)
        .unwrap_or(PIPELINE_BUDGET_MS)
}
const MAX_EXTERNAL_BYTES: usize = 2 * 1024 * 1024;
const SCRIPT_FETCH_TIMEOUT_MS: u64 = 15_000;

/// 页面加载脚本管线:收集(内联+外链预取)→ 线程执行 → 动作应用 → 回写 HTML。
///
/// `rects`:调用方预布局产生的矩形映射(open/navigate 在管线前算一次),
/// 注入快照供 JS 桥暴露 clientWidth 等几何属性;容器定宽场景(BMap 初始化)
/// 不依赖图片固有尺寸,预布局足以供值。
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
    csp: Option<&super::security::CspPolicy>,
    rects: &HashMap<String, (f64, f64, f64, f64)>,
    seed: PipelineSeed,
) -> PipelineOut {
    let mut out = PipelineOut {
        html: raw_html.to_string(),
        ..Default::default()
    };
    if std::env::var("LAEW_DOM_TRACE").is_ok() {
        eprintln!(
            "[domtrace] pipeline start raw_html_len={} scripts={}",
            raw_html.len(),
            raw_html.matches("<script").count(),
        );
    }
    if !scripts_enabled() {
        return out;
    }
    let mut executed = seed.executed;
    // 连续「无新脚本且无新动作」的轮次计数:脚本采完后仍需把页面定时器排空
    // (见下方 collected.items.is_empty() 分支的说明)。
    let mut idle_rounds: u32 = 0;
    // 外链取回失败次数(URL → 次数):第一次失败允许重试,第二次起不再取回。
    let mut fetch_failures: std::collections::HashMap<String, u8> = std::collections::HashMap::new();
    let mut handle: Option<JsHandle> = seed.handle;
    let max_rounds = if seed.max_rounds == 0 { MAX_ROUNDS } else { seed.max_rounds };
    let budget = if seed.budget_ms == 0 { pipeline_budget_ms() } else { seed.budget_ms };
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(budget);
    let title_now = {
        let doc = super::document::Document::parse(raw_html, base_url);
        doc.title()
    };
    for round in 0..max_rounds {
        if std::time::Instant::now() >= deadline {
            out.console.push(super::ConsoleRecord {
                level: "warn".to_string(),
                text: format!("脚本管线达到墙钟预算({}ms),已停止继续取回模块", budget),
                at_ms: super::now_ms(),
            });
            break;
        }
        let mut collected = {
            // Document(NodeRef)非 Send,收集完立即销毁,不跨 await 存活。
            let doc = super::document::Document::parse(&out.html, base_url);
            dom_ops::collect_scripts(&doc, base_url, &mut executed)
        };
        // defer 语义:阻塞组先执行、defer 组整组后置(同组内保持文档序),
        // 与真实浏览器「解析完 → defer 队列 → DOMContentLoaded」一致。
        let deferred_items = std::mem::take(&mut collected.deferred);
        collected.items.extend(deferred_items);
        // 本轮是否有脚本待执行(collected.items 稍后会被 move,先留底)。
        let had_scripts = !collected.items.is_empty();
        if std::env::var("LAEW_DOM_TRACE").is_ok() {
            let ext = collected
                .items
                .iter()
                .filter(|it| matches!(it, dom_ops::CollectedItem::External(_)))
                .count();
            eprintln!(
                "[domtrace] round={} collected={} external={} executed={} html_len={}",
                round,
                collected.items.len(),
                ext,
                executed.len(),
                out.html.len()
            );
        }
        if collected.items.is_empty() {
            // 无新脚本可取,但页面可能还有**定时器/rAF** 待跑:百度地图瓦片淡入
            // 是「img load → setTimeout → opacity 0→1」,而事件循环每轮最多只跑
            // 64 个回调(见 runtime.rs::MAX_TIMER_CALLBACKS),40+ 张瓦片 + 地图自身
            // 的定时器一次排不完。旧实现此处直接 break,页面永远停在入场前状态
            // → 瓦片全透明 → 地图区域整片空白(实测 80/81 张瓦片 opacity:0)。
            // 追加「空脚本」轮次把事件循环排空,直到连续若干轮既无新脚本也无新动作。
            if idle_rounds >= MAX_IDLE_ROUNDS {
                break;
            }
        }
        // 本轮**收集到**的外链 URL 先留底:执行完成后记账进 executed,
        // 下一轮 collect_scripts 不再取回(真实浏览器语义:script 只执行一次)。
        //
        // 记账必须覆盖「取回但内容为空」的脚本:站点会注入 200 但零字节的
        // 占位脚本(百度地图的 `com-conf_d41b8cd.js`,d41b8cd 即空内容 MD5),
        // 若不记账,它们每轮都会被重新收集,管线反复空转到墙钟预算耗尽,
        // 后面的地图模块脚本永远轮不到。
        let collected_externals: Vec<String> = collected
            .items
            .iter()
            .filter_map(|it| match it {
                dom_ops::CollectedItem::External(u) => Some(u.clone()),
                _ => None,
            })
            .collect();
        // 按 DOM 文档序执行(内联/外链混排),不得拆成两组重排:
        // 装载器初始化链(`mod.js` → `require.resourceMap` → pkg)依赖真实顺序。
        let mut scripts = Vec::new();
        // 本轮取回失败的外链(网络错误):允许重试一次再放弃,避免一次抖动
        // 就永久丢掉站点模块。
        let mut failed_fetches: Vec<String> = Vec::new();
        for item in collected.items {
            let url = match item {
                dom_ops::CollectedItem::Inline(s) => {
                    scripts.push(s);
                    continue;
                }
                dom_ops::CollectedItem::External(u) => u,
            };
            let url = &url;
            // 阶段四第二步：CSP script-src 门禁（外链脚本 URL 白名单）。
            if let Some(policy) = csp {
                if !policy.is_script_allowed(url, false) {
                    out.console.push(super::ConsoleRecord {
                        level: "warn".to_string(),
                        text: format!("CSP script-src 阻止脚本 {url}"),
                        at_ms: super::now_ms(),
                    });
                    continue;
                }
            }
            // 阶段四第二步：先查全内存 HTTP 缓存（强缓存命中跳过网络）。
            if super::cache_enabled() {
                if let Some(entry) = inner.http_cache.get(url) {
                    let text = super::urlutil::decode_body(&entry.bytes, &super::net::header_value(&entry.headers, "content-type").unwrap_or_default());
                    if text.len() <= MAX_EXTERNAL_BYTES && !text.trim().is_empty() {
                        out.network.push(super::NetworkRecord {
                            url: url.clone(),
                            method: "GET".to_string(),
                            status: entry.status,
                            content_type: super::net::header_value(&entry.headers, "content-type").unwrap_or_default(),
                            bytes: text.len(),
                            elapsed_ms: 0,
                            redirect: false,
                            from_cache: true,
                            at_ms: super::now_ms(),
                        });
                        scripts.push(ScriptSource {
                            kind: "external",
                            content: text,
                            src_url: url.clone(),
                        });
                    }
                    continue;
                }
            }
            let mut req = super::net::FetchRequest::get(url, SCRIPT_FETCH_TIMEOUT_MS);
            req.max_bytes = MAX_EXTERNAL_BYTES;
            // 外链脚本:浏览器语义带 Referer + Sec-Fetch-*(script) 指纹头。
            // 缺 Referer 会被站点风控(百度 heicha/acs 等)判为异常请求。
            req.context = Some(super::reqctx::RequestContext::subresource(
                base_url,
                super::reqctx::Dest::Script,
                ua,
            ));
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
                            from_cache: false,
                            at_ms: super::now_ms(),
                        });
                        scripts.push(ScriptSource {
                            kind: "external",
                            content: text,
                            src_url: url.clone(),
                        });
                        // 阶段四第二步：可缓存响应写入全内存缓存。
                        super::cache_put(&mut inner.http_cache, url, &resp);
                    }
                }
                Err(e) => {
                    out.console.push(super::ConsoleRecord {
                        level: "warn".to_string(),
                        text: format!("外链脚本加载失败 {url}:{e}"),
                        at_ms: super::now_ms(),
                    });
                    failed_fetches.push(url.clone());
                }
            }
        }
        // 阶段四第二步：CSP 内联脚本门禁（无 'unsafe-inline' 时拒绝内联脚本）。
        if let Some(policy) = csp {
            if !scripts.is_empty() && scripts.iter().any(|s| s.kind == "inline") {
                if !policy.is_script_allowed("inline", true) {
                    scripts.retain(|s| s.kind == "external");
                    out.console.push(super::ConsoleRecord {
                        level: "warn".to_string(),
                        text: "CSP script-src 阻止内联脚本(缺少 'unsafe-inline')".to_string(),
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
        let env = build_env(inner, base_url, &title_now, ua, viewport, session_storage, csp);
        // 快照后立即销毁 Document(NodeRef 非 Send,不能跨 await 存活)。
        let nodes = {
            let doc = super::document::Document::parse(&out.html, base_url);
            doc.snapshot_nodes_with_rects(viewport, rects)
        };
        // 等待预算 = 管线剩余墙钟(至少 2s)。不得沿用 eval_js 的 10s 默认值:
        // 重站点首轮脚本总量常超过 10s,旧行为会把「还在正常执行」误判为超时。
        let wait_ms = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .max(2_000) as u64;
        let reply: Result<JsReply, String> = match tokio::task::spawn_blocking(move || {
            h.request_with_budget(
                |tx| JsRequest::RunScripts {
                    scripts,
                    env,
                    nodes,
                    reply: tx,
                    budget_ms: wait_ms,
                },
                wait_ms,
            )
        })
        .await
        {
            Ok(Ok(r)) => Ok(r),
            _ => Err("脚本执行超时或线程不可用".to_string()),
        };
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                if std::env::var("LAEW_DOM_TRACE").is_ok() {
                    eprintln!("[domtrace] round={} 管线中断:{e}", round);
                }
                out.console.push(super::ConsoleRecord {
                    level: "error".to_string(),
                    text: format!("页面脚本管线中断:{e}"),
                    at_ms: super::now_ms(),
                });
                // 超时不清线程句柄:线程仍在后台跑完本轮(脚本/定时器),
                // 页面全局状态(webpack 模块表、Vue 实例)都在它身上;清掉
                // 等于整页状态归零,后续 eval 全在新 Context 上白手起家
                // (实测 zhipin:超时丢句柄后 app 挂载链彻底死亡)。
                // 保留句柄,后续 run_pending_scripts/eval 复用同一线程。
                break;
            }
        };
        let doc = super::document::Document::parse(&out.html, base_url);
        append_reply(&mut out, inner, base_url, &reply, &doc);
        out.html = doc.serialize_full();
        if std::env::var("LAEW_DOM_TRACE").is_ok() {
            eprintln!(
                "[domtrace] round={} 回包 actions={} idle_rounds={}",
                round,
                reply.outcome.actions.len(),
                idle_rounds
            );
        }
        // 静默轮次记账:既无新脚本又无新动作 = 页面已无待办(定时器排空了)。
        if reply.outcome.actions.is_empty() && !had_scripts {
            idle_rounds += 1;
        } else {
            idle_rounds = 0;
        }
        if std::env::var("LAEW_DOM_TRACE").is_ok() {
            eprintln!(
                "[domtrace] round={} html_len={} has_app={} has_MapHolder={} opacity1={} opacity0={}",
                round,
                out.html.len(),
                out.html.contains("id=\"app\""),
                out.html.contains("MapHolder"),
                out.html.matches("opacity:1").count(),
                out.html.matches("opacity:0").count(),
            );
        }
        // 外链执行记账(含执行抛错的:真实浏览器里抛错的 script 同样不会重跑)。
        // 记账覆盖「取回但内容为空」的脚本(站点注入的零字节占位脚本),
        // 否则它们每轮都被重新收集,管线空转到预算耗尽。
        for url in &collected_externals {
            if failed_fetches.iter().any(|u| u == url) {
                let n = fetch_failures.entry(url.clone()).or_insert(0);
                *n += 1;
                if *n < 2 {
                    continue; // 允许重试一次
                }
            }
            executed.insert(dom_ops::fingerprint(url));
        }
        if round + 1 < max_rounds {
            // 下一轮 collect_scripts 会检测是否有新注入的脚本。
            continue;
        }
        break;
    }
    out.js = handle;
    out.executed = executed;
    out
}

/// 组装线程请求用的 PageEnv(读 inner 的 Cookie/存储快照)。
/// `csp` 为主文档响应解析出的当前页 CSP 策略（阶段四第二步）。
pub(crate) fn build_env(
    inner: &super::Inner,
    url: &str,
    title: &str,
    ua: &str,
    viewport: (u32, u32),
    session_storage: &HashMap<String, String>,
    csp: Option<&super::security::CspPolicy>,
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
        prefers_dark: matches!(
            std::env::var("LAEW_WEB_PREFERS_DARK")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        ),
        csp: csp.cloned(),
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
    if reply.outcome.webgl_asked > out.webgl_asked {
        out.webgl_asked = reply.outcome.webgl_asked;
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
            from_cache: false,
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
