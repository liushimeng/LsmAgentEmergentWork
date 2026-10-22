//! JS 线程主体:持有持久 Boa Context,处理宿主的 RunScripts/Eval/Dispatch 请求。
//!
//! 线程内纯同步:`fetch`/XHR 走 `reqwest::blocking`;微任务由带预算的
//! JobQueue 限流;定时器由 PRELUDE 的虚拟时钟驱动(见 `prelude.rs`)。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use boa_engine::context::ContextBuilder;
use boa_engine::job::{FutureJob, JobQueue, NativeJob, SimpleJobQueue};
use boa_engine::{
    js_string, Context, JsArgs, JsResult, JsString, JsValue, NativeFunction, Source,
};
use serde_json::{json, Value};

use super::prelude::PRELUDE;
use super::{
    JsReply, JsRequest, NetRecordLite, PageEnv, ScriptSource, JsAction, JsLog, JsOutcome,
};
use crate::agent::browser::document::SnapshotNode;

const MAX_LOOP_ITERATIONS: u64 = 20_000_000;
const MAX_RECURSION: usize = 256;
const MAX_JOB_ENQUEUE: u64 = 20_000;
const MAX_TIMER_CALLBACKS: u64 = 256;
const MAX_FETCH_BODY: usize = 2 * 1024 * 1024;
const MAX_VALUE_CHARS: usize = 256 * 1024;

/// 带预算的微任务队列:超出上限的 Promise job 直接丢弃,
/// 防止 `function f(){ Promise.resolve().then(f) }` 式无限链拖死线程。
struct BudgetedJobQueue {
    inner: SimpleJobQueue,
    enqueued: Cell<u64>,
    cap: u64,
}

impl JobQueue for BudgetedJobQueue {
    fn enqueue_promise_job(&self, job: NativeJob, context: &mut Context) {
        if self.enqueued.get() >= self.cap {
            return;
        }
        self.enqueued.set(self.enqueued.get() + 1);
        self.inner.enqueue_promise_job(job, context);
    }

    fn run_jobs(&self, context: &mut Context) {
        self.inner.run_jobs(context);
    }

    fn enqueue_future_job(&self, _future: FutureJob, _context: &mut Context) {
        // 本内核不使用 Boa 的 future job 通道,丢弃。
    }
}

/// 每请求线程态(供 native fetch 读取)。
struct ReqEnv {
    base_url: String,
    cookies: String,
    network: Vec<NetRecordLite>,
    deadline: Instant,
    /// 当前页 CSP 快照（阶段四第二步：connect-src 门禁）。
    csp: Option<super::super::security::CspPolicy>,
}

thread_local! {
    static REQ_ENV: RefCell<ReqEnv> = RefCell::new(ReqEnv {
        base_url: String::new(),
        cookies: String::new(),
        network: Vec::new(),
        deadline: Instant::now(),
        csp: None,
    });
}

/// 线程入口:循环处理请求直到 Shutdown。
pub(super) fn thread_main(rx: std::sync::mpsc::Receiver<JsRequest>) {
    let mut rt = match ThreadRuntime::new() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::warn!(error = %e, "JS 线程 Context 初始化失败");
            for mut req in rx.iter() {
                if matches!(req, JsRequest::Shutdown) {
                    break;
                }
                if let Some(tx) = take_reply(&mut req) {
                    let _ = tx.send(JsReply::error(format!("JS Context 不可用:{e}")));
                }
            }
            return;
        }
    };
    for mut req in rx.iter() {
        if matches!(req, JsRequest::Shutdown) {
            break;
        }
        let reply_tx = take_reply(&mut req);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rt.handle(req)));
        match result {
            Ok(reply) => {
                if let (Some(tx), Some(out)) = (reply_tx, reply) {
                    let _ = tx.send(out);
                }
            }
            Err(_) => {
                tracing::warn!("JS 线程 panic,重建 Context");
                if let Some(tx) = reply_tx {
                    let _ = tx.send(JsReply::error("JS 线程 panic,已重建 Context"));
                }
                match ThreadRuntime::new() {
                    Ok(new_rt) => rt = new_rt,
                    Err(_) => break,
                }
            }
        }
    }
}

fn take_reply(req: &mut JsRequest) -> Option<std::sync::mpsc::Sender<JsReply>> {
    match req {
        JsRequest::RunScripts { reply, .. }
        | JsRequest::Eval { reply, .. }
        | JsRequest::Dispatch { reply, .. }
        | JsRequest::Diag { reply, .. } => Some(reply.clone()),
        JsRequest::Shutdown => None,
    }
}

struct ThreadRuntime {
    ctx: Context,
    queue: Rc<BudgetedJobQueue>,
}

impl ThreadRuntime {
    fn new() -> Result<Self, String> {
        let queue = Rc::new(BudgetedJobQueue {
            inner: SimpleJobQueue::new(),
            enqueued: Cell::new(0),
            cap: MAX_JOB_ENQUEUE,
        });
        let mut ctx = ContextBuilder::new()
            .job_queue(queue.clone())
            .build()
            .map_err(|e| format!("Boa Context 构建失败:{e}"))?;
        ctx.runtime_limits_mut()
            .set_loop_iteration_limit(MAX_LOOP_ITERATIONS);
        ctx.runtime_limits_mut().set_recursion_limit(MAX_RECURSION);
        ctx.register_global_callable(
            js_string!("__lsmFetchNative"),
            2,
            NativeFunction::from_fn_ptr(native_fetch),
        )
        .map_err(|e| format!("注册 fetch 原生函数失败:{e}"))?;
        ctx.register_global_callable(
            js_string!("atob"),
            1,
            NativeFunction::from_fn_ptr(native_atob),
        )
        .map_err(|e| format!("注册 atob 失败:{e}"))?;
        ctx.register_global_callable(
            js_string!("btoa"),
            1,
            NativeFunction::from_fn_ptr(native_btoa),
        )
        .map_err(|e| format!("注册 btoa 失败:{e}"))?;
        // ToObject 调试探针必须在 PRELUDE 求值前决定是否安装(默认不安装,
        // 包装内建方法会改变属性身份,不能常驻)。
        let trace_to_obj = std::env::var("LAEW_JS_TRACE_TOOBJ")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        ctx.eval(Source::from_bytes(
            format!("globalThis.__lsmTraceToObjInstall = {trace_to_obj};").as_bytes(),
        ))
        .map_err(|e| format!("调试开关初始化失败:{e}"))?;
        ctx.eval(Source::from_bytes(PRELUDE.as_bytes()))
            .map_err(|e| format!("PRELUDE 初始化失败:{e}"))?;
        // LAEW_JS_DIAG=1 时装 define 陷阱:站点装载器安装全局 define 时被包裹,
        // 原始首次执行(站点装载器调用约定)即被记录——重放不复现的根因是
        // 装载器注册表跳过重复 define,只有首跑能观测。
        if super::diag::diag_enabled() {
            if let Err(e) = ctx.eval(Source::from_bytes(
                super::diag::DEFINE_TRAP_JS.as_bytes(),
            )) {
                tracing::warn!(error = %e, "define 陷阱安装失败");
            }
        }
        Ok(Self { ctx, queue })
    }

    fn handle(&mut self, req: JsRequest) -> Option<JsReply> {
        match req {
            JsRequest::RunScripts {
                scripts,
                env,
                nodes,
                ..
            } => Some(self.run_scripts(&scripts, &env, &nodes)),
            JsRequest::Eval {
                script, env, nodes, ..
            } => Some(self.eval_user(&script, &env, &nodes)),
            JsRequest::Diag {
                source, origin, ..
            } => Some(self.diag_source(&source, &origin)),
            JsRequest::Dispatch {
                selector,
                event,
                env,
                nodes,
                ..
            } => Some(self.dispatch(&selector, &event, &env, &nodes)),
            JsRequest::Shutdown => None,
        }
    }

    fn eval_src(&mut self, src: &str) -> Result<JsValue, String> {
        self.ctx
            .eval(Source::from_bytes(src))
            .map_err(|e| format!("JS 执行失败:{e}"))
    }

    fn eval_string(&mut self, src: &str) -> Result<String, String> {
        let v = self.eval_src(src)?;
        v.to_string(&mut self.ctx)
            .map(|s| s.to_std_string_escaped())
            .map_err(|e| format!("JS 结果转换失败:{e}"))
    }

    /// 调试:在持久 Context 上做语句级错误定位(不改变页面状态,仅前缀重放)。
    fn diag_source(&mut self, src: &str, origin: &str) -> JsReply {
        let lines = super::diag::localize(&mut self.ctx, src, origin);
        let mut outcome = JsOutcome::default();
        outcome.ok = true;
        outcome.value = serde_json::to_string(&lines).unwrap_or_else(|_| "[]".into());
        JsReply::from_outcome(outcome)
    }

    fn eval_quiet(&mut self, src: &str) {
        if let Err(e) = self.eval_src(src) {
            self.eval_src(&format!(
                "__cerror({})",
                serde_json::to_string(&e).unwrap_or_else(|_| "\"error\"".into())
            ))
            .ok();
        }
    }

    fn begin_request(&mut self, env: &PageEnv) {
        self.queue.enqueued.set(0);
        let deadline = Instant::now() + Duration::from_millis(super::budget_ms());
        REQ_ENV.with(|c| {
            let mut env_cell = c.borrow_mut();
            env_cell.base_url = env.url.clone();
            env_cell.cookies = env.cookies.clone();
            env_cell.network.clear();
            env_cell.deadline = deadline;
            env_cell.csp = env.csp.clone();
        });
        self.eval_quiet("__lsmBegin()");
    }

    fn refresh(&mut self, nodes: &[SnapshotNode], env: &PageEnv) -> Result<(), String> {
        let nodes_json = serde_json::to_string(
            &nodes.iter().map(|n| n.to_json()).collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string());
        let meta_json = build_meta_json(env);
        let src = format!("__lsmRefresh({nodes_json}, {meta_json})");
        self.eval_src(&src).map(|_| ()).map_err(|e| e)
    }

    fn drain_jobs(&mut self) {
        self.ctx.run_jobs();
    }

    /// 微任务 + 虚拟时钟定时器,直至无待处理/超上限/超预算。
    fn event_loop(&mut self) {
        let deadline = REQ_ENV.with(|c| c.borrow().deadline);
        let mut total_ran: u64 = 0;
        loop {
            self.drain_jobs();
            let tick = self
                .eval_string("__lsmTick()")
                .unwrap_or_else(|_| "{\"pending\":0,\"ran\":0}".to_string());
            let (pending, ran) = parse_tick(&tick);
            total_ran += ran;
            if pending == 0 || total_ran > MAX_TIMER_CALLBACKS {
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
        }
    }

    fn run_scripts(&mut self, scripts: &[ScriptSource], env: &PageEnv, nodes: &[SnapshotNode]) -> JsReply {
        self.begin_request(env);
        if let Err(e) = self.refresh(nodes, env) {
            return JsReply::error(e);
        }
        for s in scripts {
            // 经 `__lsmRunScript` 间接求值(全局作用域,与浏览器 <script> 一致):
            // 失败时 JS 侧捕获 e.stack 回传,压缩大脚本也能定位到行列;
            // 此前直接 ctx.eval 只拿到 "TypeError: xxx" 一句,无从定位。
            let origin = if s.src_url.is_empty() { "(inline)".to_string() } else { s.src_url.clone() };
            // AMD 依赖数组静态补齐:无 deps 数组的 define(id, factory) 注入
            // ['require','exports','module', ...静态 require('lit')] —— Boa 的
            // fn.toString 壳让装载器源码级依赖解析落空,依赖不预载,工厂在
            // 半成品上同步执行(实测百度地图 tangram 工厂 to_object 穿透)。
            // 补齐与否取决于「此刻」页面上装的是哪种 AMD 装载器:脚本可以自己
            // 替换全局 require/define(百度地图 mod.js 把内置 RequireJS 换成 FIS
            // 同步装载器),因此必须逐个脚本在运行时判定,不能整批预设。
            let amd_kind = self
                .eval_string("__lsmAmdKind()")
                .unwrap_or_else(|_| "none".to_string());
            let filled = if super::amd_fill_enabled() && amd_kind != "fis" {
                super::amd_fill::amd_fill_deps(&s.content)
            } else {
                s.content.clone()
            };
            // Boa 0.20 for-in 规范偏差守卫:遍历 null/undefined 会抛错(规范应零次迭代),
            // `EXPR||{}` 改写在一切取值下与规范等价(见 for_in_guard.rs)。
            let mut guarded = super::for_in_guard::guard_for_in(&filled);
            // 调试:逐条语句标记 define 工厂体(LAEW_JS_TRACE_STMT=1)。
            if std::env::var("LAEW_JS_TRACE_STMT")
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false)
            {
                guarded = super::diag::instrument_amd_factories(&guarded);
            }
            self.eval_quiet(&format!(
                "__lsmSetCurrentScript({})",
                serde_json::to_string(&origin).unwrap_or_else(|_| "\"\"".into())
            ));
            let payload = serde_json::to_string(&guarded).unwrap_or_else(|_| "\"\"".into());
            let res = self.eval_string(&format!("__lsmRunScript({payload})"));
            if let Ok(body) = res {
                let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                if !parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let stack = parsed
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(无栈)")
                        .to_string();
                    let msg = format!("脚本执行失败:{stack} @ {origin}");
                    self.eval_quiet(&format!(
                        "__cerror({})",
                        serde_json::to_string(&msg).unwrap_or_else(|_| "\"error\"".into())
                    ));
                    // LAEW_JS_DIAG=1 时做语句级定位：Boa 0.20 运行时错误不带源码位置，
                    // 压缩脚本只剩一句 TypeError 无从排查（实测百度地图 tangram 工厂），
                    // 定位报告经 __cerror 进 console（Agent inspect 可见）+ 运行日志全文。
                    if super::diag::diag_enabled() {
                        let report = super::diag::localize(&mut self.ctx, &guarded, &origin);
                        for line in report {
                            tracing::warn!(origin = %origin, "{line}");
                            self.eval_quiet(&format!(
                                "__cerror({})",
                                serde_json::to_string(&line).unwrap_or_else(|_| "\"diag\"".into())
                            ));
                        }
                    }
                }
            }
            // 外链脚本执行完要触发脚本元素的 load 事件:AMD 装载器靠它推进
            // 模块解析,不触发就永远停在"等模块"状态(实测百度地图 widget 层)。
            if !s.src_url.is_empty() {
                let fire = format!(
                    "__lsmFireScriptLoad({})",
                    serde_json::to_string(&s.src_url).unwrap_or_else(|_| "\"\"".into())
                );
                self.eval_quiet(&fire);
            }
            // 脚本间跑一次微任务,常见「立即 resolve 后继续」的链式逻辑可以衔接。
            self.drain_jobs();
        }
        self.eval_quiet("__lsmFlushWrites()");
        self.eval_quiet("__lsmReady('interactive')");
        self.eval_quiet("__lsmDispatchDoc('DOMContentLoaded')");
        self.drain_jobs();
        self.eval_quiet("__lsmReady('complete')");
        self.eval_quiet("__lsmDispatchDoc('load')");
        self.event_loop();
        self.collect(None)
    }

    fn eval_user(&mut self, script: &str, env: &PageEnv, nodes: &[SnapshotNode]) -> JsReply {
        self.begin_request(env);
        if let Err(e) = self.refresh(nodes, env) {
            return JsReply::error(e);
        }
        let src = format!(
            "__lsmEvalUser({})",
            serde_json::to_string(script).unwrap_or_else(|_| "\"\"".into())
        );
        let res = self.eval_string(&src).unwrap_or_else(|e| {
            json!({"ok": false, "value": null, "type": "undefined", "error": e}).to_string()
        });
        let parsed: Value = serde_json::from_str(&res).unwrap_or_else(|_| {
            json!({"ok": false, "value": null, "type": "undefined", "error": "eval 结果非法"})
        });
        self.event_loop();
        let ov = json!({
            "ok": parsed.get("ok").and_then(Value::as_bool).unwrap_or(false),
            "value": parsed.get("value").cloned().unwrap_or(Value::Null),
            "type": parsed.get("type").and_then(Value::as_str).unwrap_or("undefined"),
            "error": parsed.get("error").cloned().unwrap_or(Value::Null),
        });
        self.collect(Some(ov))
    }

    fn dispatch(&mut self, selector: &str, event: &str, env: &PageEnv, nodes: &[SnapshotNode]) -> JsReply {
        self.begin_request(env);
        if let Err(e) = self.refresh(nodes, env) {
            return JsReply::error(e);
        }
        let src = format!(
            "__lsmDispatch({}, {})",
            serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(event).unwrap_or_else(|_| "\"\"".into())
        );
        self.eval_quiet(&src);
        self.event_loop();
        self.collect(None)
    }

    fn collect(&mut self, override_value: Option<Value>) -> JsReply {
        let src = match override_value {
            Some(ov) => format!("__lsmCollect({ov})"),
            None => "__lsmCollect()".to_string(),
        };
        let raw = self
            .eval_string(&src)
            .unwrap_or_else(|e| json!({"ok": false, "error": e}).to_string());
        let parsed: Value = serde_json::from_str(&raw).unwrap_or_else(|_| {
            json!({"ok": false, "error": format!("collect 结果非法:{}", raw.chars().take(200).collect::<String>())})
        });
        let network = REQ_ENV.with(|c| std::mem::take(&mut c.borrow_mut().network));
        let mut outcome = JsOutcome {
            ok: parsed.get("ok").and_then(Value::as_bool).unwrap_or(false),
            value: value_to_string(parsed.get("value")),
            value_type: parsed
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("undefined")
                .to_string(),
            error: parsed
                .get("error")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            actions: parsed
                .get("actions")
                .cloned()
                .and_then(|v| serde_json::from_value::<Vec<JsAction>>(v).ok())
                .unwrap_or_default(),
            logs: parsed
                .get("console")
                .cloned()
                .and_then(|v| serde_json::from_value::<Vec<JsLog>>(v).ok())
                .unwrap_or_default(),
            truncated: false,
        };
        let (clipped, truncated) = clip_owned(outcome.value, MAX_VALUE_CHARS);
        outcome.value = clipped;
        outcome.truncated = truncated;
        JsReply {
            outcome,
            network: network
                .into_iter()
                .map(|n| super::NetRecordLite {
                    url: n.url,
                    method: n.method,
                    status: n.status,
                    content_type: n.content_type,
                    bytes: n.bytes,
                    elapsed_ms: n.elapsed_ms,
                    redirect: n.redirect,
                })
                .collect(),
            handled: parsed.get("handled").and_then(Value::as_bool).unwrap_or(false),
            default_prevented: parsed
                .get("defaultPrevented")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }
}

fn parse_tick(s: &str) -> (u64, u64) {
    let v: Value = serde_json::from_str(s).unwrap_or(json!({}));
    (
        v.get("pending").and_then(Value::as_u64).unwrap_or(0),
        v.get("ran").and_then(Value::as_u64).unwrap_or(0),
    )
}

fn value_to_string(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "null".to_string(),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s)
            .map(|inner| match inner {
                Value::String(text) => text,
                other => other.to_string(),
            })
            .unwrap_or_else(|_| s.clone()),
        Some(other) => other.to_string(),
    }
}

fn clip_owned(s: String, n: usize) -> (String, bool) {
    if s.chars().count() <= n {
        return (s, false);
    }
    (s.chars().take(n).collect(), true)
}

fn build_meta_json(env: &PageEnv) -> String {
    let (protocol, host, hostname, pathname, search, hash, origin) = split_url(&env.url);
    json!({
        "url": env.url,
        "title": env.title,
        "cookies": env.cookies,
        "local": env.local_storage,
        "session": env.session_storage,
        "width": env.viewport_width,
        "height": env.viewport_height,
        "scrollX": env.scroll_x,
        "scrollY": env.scroll_y,
        "ua": env.user_agent,
        "prefersDark": env.prefers_dark,
        "protocol": protocol,
        "host": host,
        "hostname": hostname,
        "pathname": pathname,
        "search": search,
        "hash": hash,
        "origin": origin,
        // 调试开关:把 null/undefined 传入 ToObject 类内建调用的点记进 console
        // (定位 "cannot convert 'null' or 'undefined' to object" 的调用方)。
        "traceToObj": std::env::var("LAEW_JS_TRACE_TOOBJ")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false),
        "traceStmt": std::env::var("LAEW_JS_TRACE_STMT")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false),
    })
    .to_string()
}

fn split_url(url: &str) -> (String, String, String, String, String, String, String) {
    match url::Url::parse(url) {
        Ok(u) => (
            u.scheme().to_string(),
            u.host_str()
                .map(|h| match u.port() {
                    Some(p) => format!("{h}:{p}"),
                    None => h.to_string(),
                })
                .unwrap_or_default(),
            u.host_str().unwrap_or_default().to_string(),
            u.path().to_string(),
            u.query().map(|q| format!("?{q}")).unwrap_or_default(),
            u.fragment().map(|f| format!("#{f}")).unwrap_or_default(),
            format!("{}://{}", u.scheme(), u.host_str().unwrap_or_default()),
        ),
        Err(_) => (
            "https".into(),
            String::new(),
            String::new(),
            "/".into(),
            String::new(),
            String::new(),
            String::new(),
        ),
    }
}

// ---------- 原生函数 ----------

fn native_fetch(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let url = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let opts = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let result = REQ_ENV.with(|c| {
        let env = c.borrow();
        let base = env.base_url.clone();
        let cookies = env.cookies.clone();
        let deadline = env.deadline;
        drop(env);
        blocking_fetch(&url, &opts, &base, &cookies, deadline)
    });
    Ok(JsValue::from(JsString::from(result.to_string())))
}

fn native_atob(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use base64::Engine as _;
    let s = args.get_or_undefined(0).to_string(ctx)?.to_std_string_escaped();
    let out = base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| {
            boa_engine::JsError::from_native(
                boa_engine::JsNativeError::error().with_message(format!("atob 失败:{e}")),
            )
        })?;
    Ok(JsValue::from(JsString::from(out)))
}

fn native_btoa(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use base64::Engine as _;
    let s = args.get_or_undefined(0).to_string(ctx)?.to_std_string_escaped();
    let out = base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
    Ok(JsValue::from(JsString::from(out)))
}

/// 线程内阻塞 HTTP(页面 fetch/XHR 通道)。SSRF/TLS 策略与主网络栈一致。
fn blocking_fetch(
    url: &str,
    opts_json: &str,
    base_url: &str,
    cookies: &str,
    deadline: Instant,
) -> Value {
    if Instant::now() >= deadline {
        return json!({"error": "脚本预算已耗尽,请求取消"});
    }
    let opts: Value = serde_json::from_str(opts_json).unwrap_or(json!({}));
    let resolved = url::Url::parse(base_url)
        .ok()
        .and_then(|base| base.join(url).ok())
        .or_else(|| url::Url::parse(url).ok());
    let Some(target) = resolved else {
        return json!({"error": format!("URL 非法:{url}")});
    };
    if !matches!(target.scheme(), "http" | "https") {
        return json!({"error": format!("协议不支持:{}", target.scheme())});
    }
    if let Err(e) = crate::agent::safety::url_safety::is_safe_endpoint(target.as_str()) {
        return json!({"error": format!("SSRF 防护拦截:{e}")});
    }
    let method = opts
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let started = Instant::now();
    let client = build_blocking_client(crate::llm::tls_insecure_for(target.as_str()));
    let mut builder = client
        .request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET),
            target.as_str(),
        )
        .timeout(Duration::from_millis(20_000));
    if !cookies.is_empty() {
        builder = builder.header(reqwest::header::COOKIE, cookies);
    }
    if let Some(headers) = opts.get("headers").and_then(Value::as_object) {
        for (k, v) in headers {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(&v.to_string().trim_matches('"')),
            ) {
                builder = builder.header(name, val);
            }
        }
    }
    if let Some(body) = opts.get("body").and_then(Value::as_str) {
        builder = builder.body(body.to_string());
    }
    let resp = match builder.send() {
        Ok(r) => r,
        Err(e) => {
            record_network(url, &method, 0, "", 0, started);
            return json!({"error": format!("请求失败:{e}")});
        }
    };
    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or_default().to_string()))
        .collect();
    let set_cookie: Vec<String> = headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, v)| v.clone())
        .collect();
    let content_type = crate::agent::browser::net::header_value(&headers, "content-type").unwrap_or_default();
    let declared = resp.content_length().unwrap_or(0) as usize;
    if declared > MAX_FETCH_BODY {
        record_network(url, &method, status, &content_type, 0, started);
        return json!({"error": format!("响应体超过上限({} MB)", MAX_FETCH_BODY / 1024 / 1024)});
    }
    let bytes = match resp.bytes() {
        Ok(b) => b.to_vec(),
        Err(e) => {
            record_network(url, &method, status, &content_type, 0, started);
            return json!({"error": format!("读取响应失败:{e}")});
        }
    };
    let truncated = bytes.len() > MAX_FETCH_BODY;
    let bytes = if truncated { bytes[..MAX_FETCH_BODY].to_vec() } else { bytes };
    let body = crate::agent::browser::urlutil::decode_body(&bytes, &content_type);
    // 阶段四第二步：CORS 校验（跨域响应必须有合法 Access-Control-Allow-* 头）。
    if let Some(err) = check_cors(&base_url, target.as_str(), &method, &headers) {
        record_network(url, &method, status, &content_type, 0, started);
        return json!({"error": err});
    }
    // 阶段四第二步：CSP connect-src 门禁。
    if let Some(err) = check_connect_csp(&base_url, target.as_str()) {
        record_network(url, &method, status, &content_type, 0, started);
        return json!({"error": err});
    }
    record_network(url, &method, status, &content_type, bytes.len(), started);
    // JS 响应体套 for-in 守卫:AMD 装载器在线程内同步 eval 模块源码,
    // 模块里的 `for(x in nullable)` 同样会被 Boa 规范偏差打断。
    let body = if content_type.contains("javascript") || content_type.contains("ecmascript") {
        super::for_in_guard::guard_for_in(&body)
    } else {
        body
    };
    let mut header_map = serde_json::Map::new();
    for (k, v) in &headers {
        if k.eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        let key = k.to_ascii_lowercase();
        // 多值头以首个为准,避免 Map 覆盖关键头。
        header_map.entry(key).or_insert_with(|| Value::String(v.clone()));
    }
    json!({
        "ok": (200..300).contains(&status),
        "status": status,
        "statusText": "",
        "url": target.as_str(),
        "headers": Value::Object(header_map),
        "body": body,
        "setCookie": set_cookie,
        "redirect": false,
    })
}

fn record_network(url: &str, method: &str, status: u16, content_type: &str, bytes: usize, started: Instant) {
    REQ_ENV.with(|c| {
        let mut env = c.borrow_mut();
        if env.network.len() < 200 {
            env.network.push(NetRecordLite {
                url: url.to_string(),
                method: method.to_string(),
                status,
                content_type: content_type.to_string(),
                bytes,
                elapsed_ms: started.elapsed().as_millis() as u64,
                redirect: false,
            });
        }
    });
}

fn build_blocking_client(insecure: bool) -> reqwest::blocking::Client {
    let mut builder = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10));
    if insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().unwrap_or_else(|_| reqwest::blocking::Client::new())
}

/// CORS 校验（阶段四第二步）：跨域响应必须有合法 `Access-Control-Allow-*` 头。
/// 返回 None 表示放行；Some(原因) 表示按浏览器语义拒绝（fetch 走 error 通道）。
fn check_cors(
    base_url: &str,
    target: &str,
    method: &str,
    headers: &[(String, String)],
) -> Option<String> {
    let base_origin = url::Url::parse(base_url)
        .ok()
        .map(|u| u.origin().ascii_serialization())?;
    let target_origin = url::Url::parse(target)
        .ok()
        .map(|u| u.origin().ascii_serialization())?;
    if base_origin == target_origin {
        return None;
    }
    let cors = super::super::security::CorsHeaders::from_headers(headers);
    match super::super::security::check_simple_request(&base_origin, method, &cors, false) {
        super::super::security::CorsResult::Allowed
        | super::super::security::CorsResult::SameOrigin => None,
        super::super::security::CorsResult::Denied(why) => {
            Some(format!("CORS 策略阻止:{why}"))
        }
    }
}

/// CSP connect-src 门禁（阶段四第二步）：页面 CSP 存在时，fetch/XHR 目标必须在
/// connect-src（或 default-src 回退）允许范围内。
fn check_connect_csp(base_url: &str, target: &str) -> Option<String> {
    let _ = base_url;
    REQ_ENV.with(|c| {
        let env = c.borrow();
        env.csp.as_ref().and_then(|p| {
            if p.is_connect_allowed(target) {
                None
            } else {
                Some("CSP connect-src 阻止请求".to_string())
            }
        })
    })
}

#[cfg(test)]
mod diag_probe {
    //! 真实站点脚本语句级定位探针（离线，模块请求全部落空 = 前序会话已证仍可复现）。
    //!
    //! 用法：`LAEW_DIAG_LIB=/path/to/libs_b18b356.js cargo test --lib script::runtime::diag_probe -- --nocapture`
    //! 缺省（env 未设置）时自动跳过——真实文件不入库（版本哈希 URL 会漂移）。

    use super::*;

    fn probe(path: &str) -> Vec<String> {
        let src = std::fs::read_to_string(path).expect("读取真实脚本文件");
        let mut rt = ThreadRuntime::new().expect("ThreadRuntime 构建");
        // 与生产同条件：先跑一遍生产路径确认失败，再走定位器。
        let env = PageEnv {
            url: "https://map.baidu.com/".to_string(),
            title: "百度地图".to_string(),
            cookies: String::new(),
            local_storage: Default::default(),
            session_storage: Default::default(),
            viewport_width: 1280,
            viewport_height: 800,
            scroll_x: 0.0,
            scroll_y: 0.0,
            user_agent: "Mozilla/5.0".to_string(),
            prefers_dark: false,
            csp: None,
        };
        let reply = rt.run_scripts(
            &[ScriptSource {
                kind: "external",
                content: src.clone(),
                src_url: "https://map.baidu.com/wolfman/static/common/pkg/libs_b18b356.js".to_string(),
            }],
            &env,
            &[],
        );
        eprintln!("生产路径 outcome.error = {:?}", reply.outcome.error);
        let guarded = super::super::for_in_guard::guard_for_in(&src);
        super::super::diag::localize(&mut rt.ctx, &guarded, "libs_b18b356.js")
    }

    #[test]
    fn 真实tangram工厂语句级定位() {
        let Ok(path) = std::env::var("LAEW_DIAG_LIB") else {
            eprintln!("skip: 未设置 LAEW_DIAG_LIB");
            return;
        };
        if path.is_empty() {
            eprintln!("skip: LAEW_DIAG_LIB 为空");
            return;
        }
        let report = probe(&path);
        for line in &report {
            eprintln!("{line}");
        }
        // 探针断言：要么定位到具体语句，要么如实报告未复现（两种都是有效结果）。
        assert!(report.iter().any(|l| l.contains("culprit") || l.contains("未能复现")));
    }
}
