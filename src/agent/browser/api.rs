//! 自动化控制 API:`MCP_Web_LsmAgentBrowser_Use(control, control_action=...)` 的内核实现。
//!
//! 所有动作在进程内完成:DOM 查询、表单序列化、点击/输入语义、滚动、JS 执行。
//! 需要网络的动作(链接跳转/表单提交)复用 [`super::LsmBrowserManager`] 的导航能力。

use serde_json::{json, Value};

use super::document::{self, Document};
use super::js::{self, JsPageMeta};
use super::script;
use super::{EngineError, EngineResult, LsmBrowserManager};
use crate::agent::browser::urlutil;

// ========== 第 41 轮:运行时伪类状态(hover/focus/active) ==========

/// 伪类状态维度。同一时刻**只有一个**元素持有某个维度的状态位
/// (浏览器语义:一个指针、一个焦点)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PseudoDim {
    Hover,
    Focus,
    Active,
}

/// 置/清某元素的运行时伪类状态位。
///
/// 状态落进 `TabState.pseudo_states`,布局/样式阶段由
/// `Document::apply_widget_state` 写成 `data-lsm-*` 属性,样式引擎的
/// `:hover`/`:focus`/`:focus-visible`/`:active` 据此命中。
///
/// **状态只影响绘制,不影响占位**:控件固有尺寸不读任何状态标记,
/// 状态切换不会让相邻元素抖动。
fn set_pseudo_state(
    tab: &mut super::TabState,
    stable: &str,
    dim: PseudoDim,
    on: bool,
) {
    for flags in tab.pseudo_states.values_mut() {
        match dim {
            // 指针移动 → 旧的 hover 失效;同时「按键松开」,active 一并失效
            // (内核无 mousedown/mouseup,移动指针即视为松开)。
            PseudoDim::Hover => {
                flags.hover = false;
                flags.active = false;
            }
            PseudoDim::Focus => {
                flags.focus = false;
                flags.focus_visible = false;
            }
            PseudoDim::Active => flags.active = false,
        }
    }
    if on {
        let entry = tab.pseudo_states.entry(stable.to_string()).or_default();
        match dim {
            PseudoDim::Hover => entry.hover = true,
            PseudoDim::Focus => {
                entry.focus = true;
                entry.focus_visible = true;
            }
            PseudoDim::Active => entry.active = true,
        }
    }
    tab.pseudo_states.retain(|_, f| f.any());
}

/// 目标到根的元素链 stable selector(目标在前)。
///
/// CSS `:hover` 同时命中目标与全部祖先(`.parent:hover .child` 是菜单/下拉
/// 组件的最常见写法)。DOM 句柄非 Send,必须在持锁块内先收敛成 selector。
fn hover_chain_selectors(node: &kuchikiki::NodeRef) -> Vec<String> {
    let mut chain = vec![Document::stable_selector(node)];
    let mut cur = node.parent();
    while let Some(p) = cur {
        if p.as_element().is_some() {
            chain.push(Document::stable_selector(&p));
        }
        cur = p.parent();
    }
    chain
}

/// 悬停目标 + 祖先链;指针移动语义下旧 hover 与 active 一并释放。
fn set_hover_chain(tab: &mut super::TabState, chain: &[String]) {
    // 先按既有 Hover 维度清空旧指针态(on=false 不再写入目标),再逐祖先置位。
    set_pseudo_state(tab, "", PseudoDim::Hover, false);
    for stable in chain {
        tab.pseudo_states
            .entry(stable.clone())
            .or_default()
            .hover = true;
    }
    tab.pseudo_states.retain(|_, f| f.any());
}

/// 释放按下态。内核动作是「点击已完成」语义,mouseup 后不应滞留 `:active`。
fn release_active(tab: &mut super::TabState) {
    for flags in tab.pseudo_states.values_mut() {
        flags.active = false;
    }
    tab.pseudo_states.retain(|_, f| f.any());
}

fn p_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn p_u64(params: &Value, key: &str) -> Option<u64> {
    params.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

fn p_f64(params: &Value, key: &str) -> Option<f64> {
    params.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

fn p_bool(params: &Value, key: &str, default: bool) -> bool {
    match params.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(default),
        _ => default,
    }
}

fn require_selector<'a>(params: &'a Value) -> EngineResult<&'a str> {
    p_str(params, "selector").ok_or_else(|| EngineError::param("缺少 params.selector"))
}

fn nth(params: &Value) -> usize {
    p_u64(params, "nth").unwrap_or(0) as usize
}

fn timeout_ms(params: &Value, top: &Value, default: u64) -> u64 {
    p_u64(params, "timeout_ms")
        .or_else(|| p_u64(top, "timeout_ms"))
        .unwrap_or(default)
}

/// 解析当前 HTML 并落入运行时控件覆盖，保证 inspect/布局/截图共用一份状态。
fn tab_document(tab: &super::TabState) -> Document {
    tab.document_with_state()
}

/// Esc 关闭最上层打开的 `<dialog>`(第 44 轮)。
///
/// 浏览器语义:顶层浮层优先 —— 文档序**最后一个**带 `open` 的 dialog 即顶层。
/// 关闭走 DOM 属性改写(`open` / `data-lsm-modal` 移除),与页面脚本
/// `dialog.close()` 落到同一条路径。
async fn close_top_dialog(mgr: &LsmBrowserManager, pid: &str) -> EngineResult<Option<String>> {
    let (html, base_url, actions) = {
        let inner = mgr.inner.lock().await;
        let Some(tab) = inner.tabs.get(pid) else {
            return Ok(None);
        };
        let doc = tab_document(tab);
        let Ok(dialogs) = doc.select("dialog") else {
            return Ok(None);
        };
        let mut target: Option<String> = None;
        for d in dialogs {
            if document::attr(&d, "open").is_some() {
                target = Some(Document::stable_selector(&d));
            }
        }
        let Some(sel) = target else {
            return Ok(None);
        };
        let actions = vec![
            script::JsAction {
                kind: "remove_attr".to_string(),
                selector: sel.clone(),
                key: "open".to_string(),
                value: String::new(),
                url: String::new(),
                event: String::new(),
            },
            script::JsAction {
                kind: "remove_attr".to_string(),
                selector: sel.clone(),
                key: "data-lsm-modal".to_string(),
                value: String::new(),
                url: String::new(),
                event: String::new(),
            },
        ];
        (tab.html.clone(), tab.url.clone(), actions)
    };
    {
        let mut inner = mgr.inner.lock().await;
        let Some(tab) = inner.tabs.get_mut(pid) else {
            return Ok(None);
        };
        let (next_html, _summary) = apply_dom_actions(&html, &base_url, &actions);
        tab.html = next_html;
    }
    Ok(actions.first().map(|a| a.selector.clone()))
}

/// 本地表单重置(第 44 轮):清掉表单内控件的运行时值,恢复 HTML 默认值。
///
/// 运行时值只落在 `TabState::form_values`,清掉后重新解析的文档即回到
/// `value`/`checked`/`selected` 等 HTML 默认属性。
async fn reset_form_controls(mgr: &LsmBrowserManager, pid: &str, form_sel: &str) -> usize {
    let mut inner = mgr.inner.lock().await;
    let Some(tab) = inner.tabs.get_mut(pid) else {
        return 0;
    };
    let doc = tab_document(tab);
    let Ok(forms) = doc.select(form_sel) else {
        return 0;
    };
    let Some(form) = forms.into_iter().next() else {
        return 0;
    };
    let mut cleared = 0usize;
    for tag in ["input", "select", "textarea"] {
        let Ok(nodes) = form.select(tag) else {
            continue;
        };
        for n in nodes {
            let sel = Document::stable_selector(n.as_node());
            if tab.form_values.remove(&sel).is_some() {
                cleared += 1;
            }
        }
    }
    cleared
}

/// 运行时控件状态变化后重算布局并推帧。
async fn relayout_and_push(mgr: &LsmBrowserManager, pid: &str) {
    let mut inner = mgr.inner.lock().await;
    let Some(tab) = inner.tabs.get_mut(pid) else {
        return;
    };
    let doc = tab.document_with_state();
    let media = super::style::MediaCtx::new(tab.viewport.width as f64, tab.viewport.height as f64);
    let sizes = super::images::ImageSizes::from_store(&tab.images);
    tab.layout = super::layout::compute_full(
        &doc,
        tab.viewport.width as f64,
        mgr.font(),
        &media,
        &sizes,
    );
    tab.layout_rev = tab.html_rev;
    if tab.mode.is_headed() {
        mgr.push_frame(tab);
    }
}

// ============ 阶段二:页面 JS 线程交互 ============

/// 若页面有 JS 线程,把事件分发到页面 listener。无线程/失败返回 None(并清理失效句柄)。
async fn js_dispatch(
    mgr: &LsmBrowserManager,
    pid: &str,
    selector: &str,
    event: &str,
) -> Option<script::JsReply> {
    let (handle, env, nodes) = {
        let mut inner = mgr.inner.lock().await;
        // 「快照×布局」一致性守卫(与 js_eval_thread 同因):需要 &mut,放独立作用域。
        if let Some(tab) = inner.tabs.get_mut(pid) {
            mgr.ensure_layout_locked(tab);
        }
        let inner = &*inner;
        let Some(tab) = inner.tabs.get(pid) else {
            return None;
        };
        let handle = tab.js.clone()?;
        let mut env = script::build_env(
            inner,
            &tab.url,
            &tab.title,
            &tab.user_agent,
            (tab.viewport.width, tab.viewport.height),
            &tab.session_storage,
            tab.security.get_csp(&tab.url),
        );
        env.scroll_x = tab.scroll_x;
        env.scroll_y = tab.scroll_y;
        // 资源时序注入(第 174 轮):取最近 120 条,足够探测「脚本是否加载」。
        env.resources = tab.network.iter().rev().take(120).rev().cloned().collect();
        let doc = tab_document(tab);
        let rects = tab.layout.rect_map();
        let nodes = doc.snapshot_nodes_with_rects((tab.viewport.width, tab.viewport.height), &rects);
        (handle, env, nodes)
    };
    let sel = selector.to_string();
    let evt = event.to_string();
    let result = tokio::task::spawn_blocking(move || {
        handle.request(|tx| script::JsRequest::Dispatch {
            selector: sel,
            event: evt,
            env,
            nodes,
            reply: tx,
        })
    })
    .await;
    match result {
        Ok(Ok(reply)) => Some(reply),
        // 线程真退出(sender 已关)才清句柄;**忙碌超时不清**——线程还在跑
        // 页面的挂载/定时器链(Boa 下重站单轮可达数十秒),清掉等于把页面
        // 全局状态(webpack 模块表/Vue 实例)整页核爆,后续 eval 全在新
        // Context 上白手起家(实测 zhipin:超时清句柄后 app 永久停在加载中)。
        Ok(Err(e)) if e.contains("已退出") => {
            mgr.clear_js(pid).await;
            None
        }
        _ => None,
    }
}

/// JS 事件分发(带上限):线程忙/无响应时返回 None，保底让原生默认动作继续。
async fn js_dispatch_bounded(
    mgr: &LsmBrowserManager,
    pid: &str,
    selector: &str,
    event: &str,
) -> Option<script::JsReply> {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        js_dispatch(mgr, pid, selector, event),
    )
    .await
    .unwrap_or(None)
}

/// 应用 JS 回复:DOM 动作落到 `tab.html` 并重算布局;存储/Cookie/标题/滚动/台账合并。
/// 返回导航类待执行动作(click/navigate/submit/new_tab/back/forward/reload)。
async fn apply_js_reply(
    mgr: &LsmBrowserManager,
    pid: &str,
    reply: &script::JsReply,
) -> Vec<script::JsAction> {
    let mut inner = mgr.inner.lock().await;
    let (tab_url, tab_html) = {
        let Some(tab) = inner.tabs.get(pid) else {
            return Vec::new();
        };
        (tab.url.clone(), tab.html.clone())
    };
    // DOM 句柄不可跨 await(非 Send):DOM 应用与序列化放在同步辅助函数内完成。
    let (final_html, summary) =
        apply_dom_actions(&tab_html, &tab_url, &reply.outcome.actions);
    for line in &summary.cookies {
        inner
            .cookies
            .set_from_headers(&tab_url, &[("set-cookie".to_string(), line.clone())]);
    }
    let origin = urlutil::storage_origin(&tab_url);
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
    let Some(t) = inner.tabs.get_mut(pid) else {
        return Vec::new();
    };
    // session 存储合并
    for (k, v) in &summary.storage_session {
        t.session_storage.insert(k.clone(), v.clone());
    }
    for k in &summary.storage_session_remove {
        t.session_storage.remove(k);
    }
    if summary.storage_session_clear {
        t.session_storage.clear();
    }
    t.set_html(final_html.clone());
    if let Some(title) = &summary.title {
        t.title = title.clone();
    }
    let state_changed =
        !summary.form_values.is_empty() || summary.focus.is_some() || summary.blur;
    for (k, v) in summary.form_values {
        t.form_values.insert(k, v);
    }
    if let Some(s) = summary.scroll_to {
        t.scroll_y = s.max(0.0);
    }
    if let Some(s) = summary.scroll_by {
        t.scroll_y = (t.scroll_y + s).max(0.0);
    }
    if let Some(f) = summary.focus {
        set_pseudo_state(t, &f, PseudoDim::Focus, true);
        t.focused = Some(f);
    }
    if summary.blur {
        if let Some(focused) = t.focused.clone() {
            set_pseudo_state(t, &focused, PseudoDim::Focus, false);
        }
        t.focused = None;
    }
    for log in &reply.outcome.logs {
        if t.console.len() > 400 {
            t.console.remove(0);
        }
        t.console.push(super::ConsoleRecord {
            level: log.level.clone(),
            text: log.text.clone(),
            at_ms: super::now_ms(),
        });
    }
    if reply.outcome.webgl_asked > t.webgl_asked {
        t.webgl_asked = reply.outcome.webgl_asked;
    }
    for rec in &reply.network {
        if t.network.len() > 200 {
            t.network.remove(0);
        }
        t.network.push(super::NetworkRecord {
            url: rec.url.clone(),
            method: if rec.method.is_empty() {
                "FETCH".to_string()
            } else {
                rec.method.clone()
            },
            status: rec.status,
            content_type: rec.content_type.clone(),
            bytes: rec.bytes,
            elapsed_ms: rec.elapsed_ms,
            redirect: rec.redirect,
            from_cache: false,
            at_ms: super::now_ms(),
        });
    }
    // 阶段三第三步:补齐脚本新增的图片并重排(JS 动态插入 <img> 也能出图)。
    // 第 174 轮性能:DOM 未变化的 eval(纯读取探测占大头)跳过图片扫描与
    // 全量重排 —— 同一 html 的图片集与布局完全确定,重算是纯浪费;重页单条
    // eval 因此从 25-35s 降到亚秒级。脚本补齐(run_pending_scripts)仍照常,
    // 由它自己的「无待执行脚本」预检兜底。
    let dom_changed = final_html != tab_html;
    if !dom_changed && !state_changed {
        drop(inner);
        run_pending_scripts(mgr, pid, interaction_budget_ms()).await;
        return reply
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
            .collect();
    }
    let (page_url, user_agent, existing_images) = {
        let Some(tab) = inner.tabs.get(pid) else {
            return Vec::new();
        };
        (tab.url.clone(), tab.user_agent.clone(), tab.images.clone())
    };
    let images_out = super::images::load_images(
        &mut inner,
        &mgr.http,
        &final_html,
        &page_url,
        &user_agent,
        15_000,
        &existing_images,
    )
    .await;
    if let Some(tab) = inner.tabs.get_mut(pid) {
        tab.images = images_out.store;
        let dom = tab_document(tab);
        let media = super::style::MediaCtx::new(tab.viewport.width as f64, tab.viewport.height as f64);
        let sizes = super::images::ImageSizes::from_store(&tab.images);
        tab.layout = super::layout::compute_full(
            &dom,
            tab.viewport.width as f64,
            mgr.font(),
            &media,
            &sizes,
        );
        // 有头:JS 动作应用后重排完成,推帧给查看器。
        if tab.mode.is_headed() {
            mgr.push_frame(tab);
        }
        for rec in images_out.network {
            if tab.network.len() > 200 {
                tab.network.remove(0);
            }
            tab.network.push(super::NetworkRecord {
                url: rec.url,
                method: rec.method,
                status: rec.status,
                content_type: rec.content_type,
                bytes: rec.bytes,
                elapsed_ms: rec.elapsed_ms,
                redirect: rec.redirect,
                from_cache: false,
                at_ms: super::now_ms(),
            });
        }
        for log in images_out.console {
            if tab.console.len() > 400 {
                tab.console.remove(0);
            }
            tab.console.push(super::ConsoleRecord {
                level: log.level,
                text: log.text,
                at_ms: super::now_ms(),
            });
        }
    }
    drop(inner);
    // 交互后补齐懒加载脚本:真实浏览器里 `require.async`/动态 <script> 会在
    // 点击后立即取回执行;宿主若只在 open/navigate 跑管线,路线面板这类
    // 懒加载 widget 会永远停在 loading 态(实测百度地图路线按钮)。
    run_pending_scripts(mgr, pid, interaction_budget_ms()).await;
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

/// 交互后单次脚本补齐墙钟预算(毫秒)。
///
/// 第 174 轮(猎聘):8s 装不完重站懒加载链 —— 首屏 3.3MB common.js 之后,
/// 登录弹层/hero 组件 chunk 在交互注入,8s 窗口反复被截断,组件永远差一口
/// 气装不上。默认提到 30s,`LAEW_WEB_SCRIPT_INTERACT_MS` 可覆盖(下限 2000)。
fn interaction_budget_ms() -> u64 {
    std::env::var("LAEW_WEB_SCRIPT_INTERACT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v >= 2_000)
        .unwrap_or(30_000)
}
/// 交互后脚本补齐轮次上限(懒加载链通常 1-3 轮;重站 chunk 链 12 轮不够)。
const INTERACTION_SCRIPT_ROUNDS: usize = 24;

/// 用页面当前 DOM 续跑脚本管线,只执行尚未执行过的 `<script>`。
///
/// 复用标签页持久 JS 线程(`tab.js`)与已执行指纹(`tab.executed_scripts`),
/// 因此页面全局状态不丢、首屏脚本也不会重复初始化。
async fn run_pending_scripts(mgr: &LsmBrowserManager, pid: &str, budget_ms: u64) {
    let mut inner = mgr.inner.lock().await;
    let (html, url, ua, viewport, session, rects, seed, has_pending) = {
        let Some(tab) = inner.tabs.get(pid) else {
            return;
        };
        // 廉价预检(第 174 轮性能):无待执行脚本就不进管线。此前每条 eval/事件
        // 都空跑一遍「解析+收集+排空」,重页(猎聘 372KB DOM)单条 eval
        // 25-35s 的大头就是它;页面无变化时这里直接归零。
        let mut probe = tab.executed_scripts.clone();
        let doc = tab_document(tab);
        let mut collected = script::dom_ops::collect_scripts(&doc, &tab.url, &mut probe);
        let deferred_left = std::mem::take(&mut collected.deferred);
        let has_pending = !collected.items.is_empty() || !deferred_left.is_empty();
        (
            tab.html.clone(),
            tab.url.clone(),
            tab.user_agent.clone(),
            (tab.viewport.width, tab.viewport.height),
            tab.session_storage.clone(),
            tab.layout.rect_map(),
            script::PipelineSeed {
                handle: tab.js.clone(),
                executed: tab.executed_scripts.clone(),
                max_rounds: INTERACTION_SCRIPT_ROUNDS,
                budget_ms,
            },
            has_pending,
        )
    };
    if !has_pending {
        return;
    }
    let pipeline = script::run_pipeline(
        &mut inner,
        &mgr.http,
        &html,
        &url,
        &ua,
        viewport,
        &session,
        None,
        &rects,
        seed,
    )
    .await;
    let Some(tab) = inner.tabs.get_mut(pid) else {
        return;
    };
    let changed = pipeline.html != html;
    if changed {
        tab.set_html(pipeline.html);
    }
    // 截断观测字段(第 174 轮):管线收敛则清零,截断则带出剩余数。
    tab.script_budget_cut = pipeline.budget_cut;
    tab.script_pending = pipeline.pending_scripts;
    if let Some(h) = pipeline.js {
        tab.js = Some(h);
    }
    if !pipeline.executed.is_empty() {
        tab.executed_scripts = pipeline.executed;
    }
    for (k, v) in pipeline.form_values {
        tab.form_values.insert(k, v);
    }
    for log in pipeline.console {
        if tab.console.len() > 400 {
            tab.console.remove(0);
        }
        tab.console.push(super::ConsoleRecord {
            level: log.level,
            text: log.text,
            at_ms: super::now_ms(),
        });
    }
    for rec in pipeline.network {
        if tab.network.len() > 200 {
            tab.network.remove(0);
        }
        tab.network.push(super::NetworkRecord {
            url: rec.url,
            method: if rec.method.is_empty() { "GET".to_string() } else { rec.method },
            status: rec.status,
            content_type: rec.content_type,
            bytes: rec.bytes,
            elapsed_ms: rec.elapsed_ms,
            redirect: rec.redirect,
            from_cache: false,
            at_ms: super::now_ms(),
        });
    }
    if changed {
        let dom = tab_document(tab);
        let media = super::style::MediaCtx::new(tab.viewport.width as f64, tab.viewport.height as f64);
        let sizes = super::images::ImageSizes::from_store(&tab.images);
        tab.layout = super::layout::compute_full(
            &dom,
            tab.viewport.width as f64,
            mgr.font(),
            &media,
            &sizes,
        );
        tab.layout_rev = tab.html_rev;
        if tab.mode.is_headed() {
            mgr.push_frame(tab);
        }
    }
}

/// 应用 DOM 动作并序列化(同步辅助函数)。
///
/// DOM 句柄(`Rc` 树)不可跨 await 存活,故把"解析 → 应用 → 序列化"整体收在
/// 同步函数内,调用方只拿纯数据(`String` + `ApplySummary`)。
fn apply_dom_actions(
    html: &str,
    base_url: &str,
    actions: &[script::JsAction],
) -> (String, script::dom_ops::ApplySummary) {
    let doc = Document::parse(html, base_url);
    let summary = script::dom_ops::apply(&doc, actions, base_url);
    let final_html = doc.serialize_full();
    (final_html, summary)
}

/// eval_js 走页面 JS 线程的三种结局。
enum ThreadEval {
    /// 线程正常回复。
    Reply(script::JsReply),
    /// 无 JS 线程可用(`LAEW_WEB_SCRIPT=off` / 线程真退出):调用方可回退快照桥。
    NoThread,
    /// 线程在位但本轮预算内没回(事件循环在排定时器/微任务):**不要静默回退**。
    /// 阶段一桥是另一套语义环境(独立 Context、独立 document),静默回退会让
    /// 调用方拿到「同一段脚本在另一个世界跑出来的结果」——几何/全局状态全部
    /// 失真(实测猎聘:eval 超时回退后 `getBoundingClientRect` 全 0)。
    Busy,
}

/// eval_js 走页面 JS 线程(持久全局)。线程不可用时返回 NoThread,由调用方回退快照桥。
async fn js_eval_thread(
    mgr: &LsmBrowserManager,
    pid: &str,
    script_text: &str,
) -> ThreadEval {
    if !script::scripts_enabled() {
        return ThreadEval::NoThread;
    }
    {
        // 懒启动:页面无脚本时首次 eval_js 建线程,获得持久会话。
        let mut inner = mgr.inner.lock().await;
        if let Some(tab) = inner.tabs.get_mut(pid) {
            if tab.js.is_none() {
                tab.js = Some(script::JsHandle::spawn());
            }
        }
    }
    let (handle, env, nodes) = {
        let mut inner = mgr.inner.lock().await;
        // 「快照×布局」一致性守卫:html 在上次布局后被脚本改写时先重排,
        // 否则 rect_map(旧布局)× 快照(新 html)的位置键错位,几何全 0。
        // 需要 &mut,放独立作用域,之后降级为不可变借用。
        if let Some(tab) = inner.tabs.get_mut(pid) {
            mgr.ensure_layout_locked(tab);
        }
        let inner = &*inner;
        let Some(tab) = inner.tabs.get(pid) else {
            return ThreadEval::NoThread;
        };
        let Some(handle) = tab.js.clone() else {
            return ThreadEval::NoThread;
        };
        let mut env = script::build_env(
            inner,
            &tab.url,
            &tab.title,
            &tab.user_agent,
            (tab.viewport.width, tab.viewport.height),
            &tab.session_storage,
            tab.security.get_csp(&tab.url),
        );
        env.scroll_x = tab.scroll_x;
        env.scroll_y = tab.scroll_y;
        // 资源时序注入(第 174 轮):取最近 120 条,足够探测「脚本是否加载」。
        env.resources = tab.network.iter().rev().take(120).rev().cloned().collect();
        let doc = tab_document(tab);
        let rects = tab.layout.rect_map();
        let nodes = doc.snapshot_nodes_with_rects((tab.viewport.width, tab.viewport.height), &rects);
        (handle, env, nodes)
    };
    let script_text = script_text.to_string();
    let result = tokio::task::spawn_blocking(move || {
        // eval 也走宽限续等:重页面(猎聘/百度地图)事件循环排定时器单轮可到
        // 数十秒,常规 10s 预算内回不来就报「忙碌」,调用方还得再烧一轮迭代
        // 重试。宽限 30s(与管线宽限同机制),仍超时才判忙碌(2002)。
        handle.request_with_grace(
            |tx| script::JsRequest::Eval {
                script: script_text,
                env,
                nodes,
                reply: tx,
            },
            script::budget_ms(),
            script::eval_grace_ms(),
        )
    })
    .await;
    match result {
        Ok(Ok(reply)) => ThreadEval::Reply(reply),
        // 同 js_dispatch_thread:忙碌超时保留句柄,只有线程真退出才清。
        Ok(Err(e)) if e.contains("已退出") => {
            mgr.clear_js(pid).await;
            ThreadEval::NoThread
        }
        Ok(Err(_)) => ThreadEval::Busy,
        Err(_) => ThreadEval::Busy,
    }
}

/// 截图/等待前同步页面:复用持久 JS Context 推进微任务与定时器,应用 DOM/Canvas
/// 动作并按需重排。无脚本页面零成本;绝不为了取一帧重建页面全局状态。
pub(crate) async fn sync_page(mgr: &LsmBrowserManager, pid: &str) -> bool {
    let has_js = {
        let inner = mgr.inner.lock().await;
        inner.tabs.get(pid).and_then(|tab| tab.js.as_ref()).is_some()
    };
    if !has_js {
        return false;
    }
    match js_eval_thread(mgr, pid, "undefined").await {
        ThreadEval::Reply(reply) => {
            apply_js_reply(mgr, pid, &reply).await;
            true
        }
        _ => false,
    }
}

/// 执行一个 control_action。
pub async fn control(
    mgr: &LsmBrowserManager,
    page_id: Option<&str>,
    action: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    let pid = page_id.unwrap_or("");
    match action {
        "click" | "human_click" => act_click(mgr, pid, params, action == "human_click").await,
        "right_click" => act_click_kind(mgr, pid, params, "right").await,
        "double_click" => act_click_kind(mgr, pid, params, "double").await,
        "hover" => act_hover(mgr, pid, params).await,
        "mouse_move" => act_mouse_move(mgr, pid, params).await,
        "input_text" | "human_input" => {
            let out = act_input(mgr, pid, params).await?;
            // params.submit=true:输入后回车(与 Chrome 侧同名同义)。
            // 站点搜索框输入后通常弹「联想下拉」,回车才会把下拉里的规范 POI
            // 提交出去;只输入不回车,站点会用**原始文本**做地理编码,
            // 结果就是「未找到相关地点」(实测百度地图路线规划:2026-10-26)。
            let submit = params.get("submit").and_then(Value::as_bool).unwrap_or(false)
                || params.get("pick_suggestion").and_then(Value::as_bool).unwrap_or(false);
            if !submit {
                return Ok(out);
            }
            let mut key_params = params.clone();
            key_params["key"] = json!("Enter");
            let pressed = act_key_press(mgr, pid, &key_params, top).await?;
            let mut o = out.as_object().cloned().unwrap_or_default();
            o.insert("submitted".into(), json!(true));
            o.insert("submit_key".into(), json!("Enter"));
            o.insert("key_result".into(), pressed);
            Ok(Value::Object(o))
        }
        "clear_input" => act_clear(mgr, pid, params).await,
        "select_option" => act_select_option(mgr, pid, params).await,
        "select_dropdown" => act_select_dropdown(mgr, pid, params).await,
        // 第 41 轮:select 展开/收起浮层面板。
        "open_dropdown" => act_toggle_dropdown(mgr, pid, params, true).await,
        "close_dropdown" => act_toggle_dropdown(mgr, pid, params, false).await,
        "key_press" => act_key_press(mgr, pid, params, top).await,
        "press_sequence" => act_press_sequence(mgr, pid, params, top).await,
        "scroll" => act_scroll(mgr, pid, params).await,
        "scroll_to" => act_scroll_to(mgr, pid, params).await,
        "scroll_to_text" => act_scroll_to_text(mgr, pid, params).await,
        "navigate" => {
            require_page(pid)?;
            let url = p_str(params, "url")
                .or_else(|| p_str(top, "url"))
                .ok_or_else(|| EngineError::param("navigate 需要 params.url"))?;
            let t = timeout_ms(params, top, 60_000);
            let nav = mgr.navigate(pid, url, t).await?;
            Ok(nav_json(&nav))
        }
        "back" | "forward" | "reload" => {
            require_page(pid)?;
            let nav = mgr.history_nav(pid, action, timeout_ms(params, top, 60_000)).await?;
            Ok(nav_json(&nav))
        }
        "new_tab" => {
            require_page(pid)?;
            let url = p_str(params, "url").ok_or_else(|| EngineError::param("new_tab 需要 params.url"))?;
            let out = mgr.new_tab(pid, url, timeout_ms(params, top, 60_000)).await?;
            Ok(json!({
                "opened": true,
                "page_id": out.page_id,
                "spawned_page_id": out.page_id,
                "title": out.title,
                "final_url": out.final_url,
            }))
        }
        "new_window" => {
            let url = p_str(params, "url").ok_or_else(|| EngineError::param("new_window 需要 params.url"))?;
            let out = mgr
                .new_page(
                    url,
                    super::BrowserMode::Hidden,
                    None,
                    None,
                    None,
                    true,
                    false,
                    timeout_ms(params, top, 60_000),
                )
                .await?;
            Ok(json!({
                "opened": true,
                "page_id": out.page_id,
                "spawned_page_id": out.page_id,
                "title": out.title,
                "final_url": out.final_url,
                "window_id": out.window_id,
            }))
        }
        "close_tab" => {
            require_page(pid)?;
            let out = mgr.close_page(pid).await;
            Ok(json!({"closed": out.closed.contains(&pid.to_string())}))
        }
        "bring_to_front" => {
            require_page(pid)?;
            Ok(json!({"brought_to_front": mgr.bring_to_front(pid).await}))
        }
        "wait" => act_wait(mgr, pid, params, top).await,
        "eval_js" => act_eval(mgr, pid, params, top).await,
        "set_cookie" => {
            let name = p_str(params, "name").ok_or_else(|| EngineError::param("set_cookie 需要 name"))?;
            let value = params.get("value").and_then(Value::as_str).unwrap_or("");
            mgr.set_cookie(
                if pid.is_empty() { None } else { Some(pid) },
                name,
                value,
                p_str(params, "domain"),
                p_str(params, "path"),
                p_str(params, "url"),
            )
            .await
        }
        "delete_cookie" => {
            let name = p_str(params, "name").ok_or_else(|| EngineError::param("delete_cookie 需要 name"))?;
            require_page(pid)?;
            mgr.delete_cookie(Some(pid), name, p_str(params, "domain")).await
        }
        "set_storage" => {
            require_page(pid)?;
            let kind = if p_str(params, "kind") == Some("session") { "session" } else { "local" };
            let items = storage_items(params)?;
            mgr.storage_set(pid, kind, &items).await
        }
        "clear_storage" => {
            require_page(pid)?;
            let kind = if p_str(params, "kind") == Some("session") { "session" } else { "local" };
            let keys: Vec<String> = params
                .get("keys")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            mgr.storage_clear(pid, kind, &keys).await
        }
        "set_viewport" => {
            require_page(pid)?;
            let width = p_u64(params, "width").ok_or_else(|| EngineError::param("set_viewport 需要 width"))? as u32;
            let height = p_u64(params, "height").ok_or_else(|| EngineError::param("set_viewport 需要 height"))? as u32;
            mgr.set_viewport(pid, width, height).await
        }
        "screenshot" => {
            require_page(pid)?;
            mgr.screenshot(pid, params).await
        }
        "download" => {
            require_page(pid)?;
            mgr.download(pid, params).await
        }
        "set_window" => {
            require_page(pid)?;
            let mut inner = mgr.inner.lock().await;
            let tab = inner
                .tabs
                .get_mut(pid)
                .ok_or_else(|| EngineError::page_not_found(pid))?;
            if let Some(w) = p_u64(params, "width") {
                tab.viewport.width = (w as u32).clamp(320, 7680);
            }
            if let Some(h) = p_u64(params, "height") {
                tab.viewport.height = (h as u32).clamp(240, 4320);
            }
            let doc = tab_document(tab);
            // 阶段三第三步:视口变化复用已解码图片的固有尺寸重排。
            let media = super::style::MediaCtx::new(tab.viewport.width as f64, tab.viewport.height as f64);
            let sizes = super::images::ImageSizes::from_store(&tab.images);
            tab.layout = super::layout::compute_full(
                &doc,
                tab.viewport.width as f64,
                mgr.font(),
                &media,
                &sizes,
            );
            // 有头:视口/窗口变更后重排完成,推帧给查看器。
            if tab.mode.is_headed() {
                mgr.push_frame(tab);
            }
            Ok(json!({
                "width": tab.viewport.width,
                "height": tab.viewport.height,
                "left": p_u64(params, "left"),
                "top": p_u64(params, "top"),
                "window_state": p_str(params, "window_state").unwrap_or("normal"),
                "note": "LsmAgentBrowser 为进程内逻辑窗口;headed 模式下由独立查看器子进程(laew __browser-view)以原生窗口显示渲染帧,工具栏(后退/前进/刷新/首页 + 地址栏)经命令通道驱动本引擎",
            }))
        }
        "sync_viewport" => {
            require_page(pid)?;
            super::tab_view(mgr, pid).await.map(|(_, _, _, _, vp, _, _)| {
                json!({"width": vp.width, "height": vp.height, "synced": true})
            })
        }
        "set_highlight" => {
            require_page(pid)?;
            let enabled = p_bool(params, "enabled", true);
            let mut inner = mgr.inner.lock().await;
            let tab = inner
                .tabs
                .get_mut(pid)
                .ok_or_else(|| EngineError::page_not_found(pid))?;
            tab.highlight = enabled;
            Ok(json!({"highlight": enabled, "supported": false, "note": "阶段一无原生窗口,高亮不绘制"}))
        }
        "set_guard" => act_set_guard(mgr, pid, params).await,
        "set_overlay" => {
            require_page(pid)?;
            let enabled = p_bool(params, "enabled", true);
            act_set_guard(
                mgr,
                pid,
                &json!({"mode": if enabled { "locked" } else { "open" }}),
            )
            .await
        }
        "auto_follow" => {
            require_page(pid)?;
            let enabled = p_bool(params, "enabled", true);
            let mut inner = mgr.inner.lock().await;
            let tab = inner
                .tabs
                .get_mut(pid)
                .ok_or_else(|| EngineError::page_not_found(pid))?;
            tab.auto_follow = enabled;
            if p_bool(params, "scroll_to_bottom", false) {
                tab.scroll_y = tab.layout.page_height();
            }
            Ok(json!({"auto_follow": enabled, "visibility": "visible", "note": "阶段一无窗口滚动跟随"}))
        }
        "set_stealth" => Ok(json!({
            "mode": p_str(params, "mode").unwrap_or("off"),
            "applied": false,
            "note": "LsmAgentBrowser 原生内核不注入任何伪装脚本;该参数仅为兼容保留",
        })),
        "heartbeat" => {
            require_page(pid)?;
            let summary = mgr
                .page_summary(pid)
                .await
                .ok_or_else(|| EngineError::page_not_found(pid))?;
            Ok(json!({"ok": true, "title": summary.title}))
        }
        "focus" | "blur" => act_focus(mgr, pid, params, action).await,
        "dispatch_event" => act_dispatch_event(mgr, pid, params, top).await,
        "upload_file" => act_upload(mgr, pid, params).await,
        "drag" => act_drag(mgr, pid, params).await,
        other => Err(EngineError::param(format!("未知 control_action:{other}"))),
    }
}

fn require_page(pid: &str) -> EngineResult<()> {
    if pid.is_empty() {
        Err(EngineError::param("缺少 page_id"))
    } else {
        Ok(())
    }
}

fn storage_items(params: &Value) -> EngineResult<Vec<(String, String)>> {
    if let Some(obj) = params.get("items").and_then(Value::as_object) {
        return Ok(obj
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()),
                )
            })
            .collect());
    }
    if let Some(key) = p_str(params, "key") {
        let value = params
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        return Ok(vec![(key.to_string(), value)]);
    }
    Err(EngineError::param("set_storage 需要 items 或 key/value"))
}

fn nav_json(nav: &super::NavOutcome) -> Value {
    json!({
        "page_id": nav.page_id,
        "url": nav.url,
        "requested_url": nav.requested_url,
        "final_url": nav.url,
        "title": nav.title,
        "status": nav.status,
        "redirected": nav.redirect_chain.len() > 1,
        "redirect_chain": nav.redirect_chain,
        "content_type": nav.content_type,
        "byte_size": nav.source_bytes,
    })
}

enum NavPlan {
    Same { url: String, method: String, body: Option<Vec<u8>> },
    NewTab { url: String },
}

async fn act_click(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    human: bool,
) -> EngineResult<Value> {
    require_page(pid)?;
    click_inner(mgr, pid, params, if human { "human" } else { "click" }).await
}

async fn act_click_kind(mgr: &LsmBrowserManager, pid: &str, params: &Value, kind: &str) -> EngineResult<Value> {
    require_page(pid)?;
    click_inner(mgr, pid, params, kind).await
}

async fn click_inner(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    kind: &str,
) -> EngineResult<Value> {
    let selector = require_selector(params)?;
    let idx = nth(params);
    let timeout = timeout_ms(params, params, 60_000);
    let (
        plan,
        rect,
        stable,
        hover_chain,
        disabled,
        toggled,
        laid_out,
        radio_scope,
        summary_details,
        reset_form,
        select_toggle,
        option_pick,
        open_selects,
    ) = {
        let inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = tab_document(tab);
        let nodes = doc
            .select(selector)
            .map_err(EngineError::action_failed)?;
        let node = nodes.get(idx).ok_or_else(|| {
            EngineError::action_failed(format!("选择器未命中或索引越界:{selector}[{idx}]"))
        })?;
        let stable = Document::stable_selector(node);
        let rect_opt = tab.layout.rect_for(&stable);
        let rect = rect_opt.unwrap_or((0.0, 0.0, 0.0, 0.0));
        let tag = document::tag_name(node);
        let typ = document::attr(node, "type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let hover_chain = hover_chain_selectors(node);
        let disabled = matches!(
            tag.as_str(),
            "input" | "button" | "select" | "textarea" | "option"
        ) && document::attr(node, "disabled").is_some();

        // 当前展开的 select 面板:点击面板外任何元素都应收起(light dismiss 近似)。
        let open_selects: Vec<String> = doc
            .select("select")
            .map(|v| {
                v.iter()
                    .map(|n| Document::stable_selector(n))
                    .filter(|s| tab.widget_open.get(s).copied().unwrap_or(false))
                    .collect()
            })
            .unwrap_or_default();
        let mut select_toggle = None;
        let mut option_pick = None;
        if tag == "select"
            && document::attr(node, "multiple").is_none()
            && !super::layout::select_is_list(node)
        {
            let open = tab.widget_open.get(&stable).copied().unwrap_or(false);
            select_toggle = Some((stable.clone(), !open));
        } else if tag == "option" {
            if let Some(select) = document::closest_tag(node, "select") {
                let select_sel = Document::stable_selector(&select);
                // 第 44 轮:列表形态 select(`size>1`/`multiple`)的 option 常驻可见,
                // 无需展开面板即可点选。
                if tab.widget_open.get(&select_sel).copied().unwrap_or(false)
                    || super::layout::select_is_list(&select)
                {
                    if let Ok(options) = select.select("option") {
                        for (i, opt) in options.into_iter().enumerate() {
                            if Document::stable_selector(opt.as_node()) == stable {
                                let label = document::collapse_ws(&opt.as_node().text_contents());
                                let value = document::attr(opt.as_node(), "value")
                                    .unwrap_or_else(|| label.clone());
                                option_pick = Some((select_sel, value, label, i));
                                break;
                            }
                        }
                    }
                }
            }
        }

        let mut toggled = None;
        let mut radio_scope = None;
        let mut summary_details = None;
        // 第 44 轮:`reset` 按钮的默认行为是**本地表单重置**(不提交)。
        let mut reset_form = None;
        if tag == "summary" {
            let mut cur = node.parent();
            while let Some(p) = cur {
                if document::tag_name(&p) == "details" {
                    summary_details = Some(Document::stable_selector(&p));
                    break;
                }
                cur = p.parent();
            }
        }
        if matches!(tag.as_str(), "button" | "input") && typ == "reset" {
            let mut cur = node.parent();
            while let Some(p) = cur {
                if document::tag_name(&p) == "form" {
                    reset_form = Some(Document::stable_selector(&p));
                    break;
                }
                cur = p.parent();
            }
        }
        if tag == "input" && matches!(typ.as_str(), "checkbox" | "radio") {
            let cur = tab
                .form_values
                .get(&stable)
                .map(|v| v == "true")
                .unwrap_or_else(|| document::attr(node, "checked").is_some());
            toggled = Some((stable.clone(), (!cur).to_string()));
            if typ == "radio" {
                if let Some(name) = document::attr(node, "name").filter(|n| !n.trim().is_empty()) {
                    let mut form_key = String::new();
                    let mut cur = node.parent();
                    while let Some(p) = cur {
                        if document::tag_name(&p) == "form" {
                            form_key = Document::stable_selector(&p);
                            break;
                        }
                        cur = p.parent();
                    }
                    radio_scope = Some((form_key, name));
                }
            }
        }
        let mut plan = None;
        if toggled.is_none() && !matches!(kind, "right" | "double") {
            if let Some(href) = document::attr(node, "href") {
                if let Some(url) = urlutil::resolve(&tab.url, &href) {
                    let blank = document::attr(node, "target")
                        .map(|t| t.eq_ignore_ascii_case("_blank"))
                        .unwrap_or(false);
                    plan = Some(if blank {
                        NavPlan::NewTab { url }
                    } else {
                        NavPlan::Same {
                            url,
                            method: "GET".to_string(),
                            body: None,
                        }
                    });
                }
            } else {
                // 默认提交只属于 submit/image;button/reset 型按钮不触发表单。
                let submitter = match tag.as_str() {
                    "button" => !matches!(typ.as_str(), "button" | "reset"),
                    "input" => matches!(typ.as_str(), "submit" | "image"),
                    _ => document::attr(node, "role").as_deref() == Some("button"),
                };
                if submitter {
                    if let Some(form) = doc.form_request(node, &tab.form_values) {
                        let (url, body) = form.to_request();
                        plan = Some(NavPlan::Same {
                            url,
                            method: form.method,
                            body,
                        });
                    }
                }
            }
        }
        (
            plan,
            rect,
            stable,
            hover_chain,
            disabled,
            toggled,
            rect_opt.is_some(),
            radio_scope,
            summary_details,
            reset_form,
            select_toggle,
            option_pick,
            open_selects,
        )
    };

    if disabled {
        return Ok(json!({
            "clicked": false,
            "disabled": true,
            "selector": stable,
            "x": rect.0,
            "y": rect.1,
            "mode": kind,
            "laid_out": laid_out,
            "note": "目标带 disabled 属性,不分发事件也不执行默认行为",
        }));
    }

    // 第 41 轮:点击语义 —— 目标与祖先进入 hover,目标进入 active。
    // 状态只影响绘制,不改控件占位;动作收尾统一释放 active(mouseup 语义)。
    {
        let mut inner = mgr.inner.lock().await;
        if let Some(tab) = inner.tabs.get_mut(pid) {
            set_hover_chain(tab, &hover_chain);
            set_pseudo_state(tab, &stable, PseudoDim::Active, true);
        }
    }

    let mut out = json!({
        "clicked": true,
        "x": rect.0,
        "y": rect.1,
        "mode": kind,
        "selector": stable,
        "laid_out": laid_out,
    });
    // 阶段二:页面 listener 分发。preventDefault 或 JS 产生导航动作时优先按 JS 语义执行;
    // listener 未拦截时继续默认行为(handler 与默认导航并存,贴近真实浏览器)。
    if !matches!(kind, "right" | "double" | "js") {
        // JS 先行但不能无限等:页面线程忙/无响应时按未拦截处理,保底推进默认动作。
        let dispatch = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            js_dispatch(mgr, pid, &stable, "click"),
        )
        .await
        .unwrap_or(None);
        if let Some(reply) = dispatch {
            let pending = apply_js_reply(mgr, pid, &reply).await;
            if reply.handled {
                out["js_handled"] = json!(true);
            }
            if reply.default_prevented {
                out["default_prevented"] = json!(true);
                {
                    let mut inner = mgr.inner.lock().await;
                    if let Some(tab) = inner.tabs.get_mut(pid) {
                        release_active(tab);
                    }
                }
                return Ok(out);
            }
            if let Some(action) = pending.into_iter().next() {
                let nav_result: EngineResult<Option<(String, String)>> = match action.kind.as_str() {
                    "navigate" => mgr.navigate(pid, &action.url, timeout).await.map(|n| Some((n.url, n.title))),
                    "new_tab" => mgr.new_tab(pid, &action.url, timeout).await.map(|n| Some((n.final_url, n.title))),
                    "back" | "forward" | "reload" => mgr.history_nav(pid, &action.kind, timeout).await.map(|n| Some((n.url, n.title))),
                    "click" | "submit" => {
                        let p = json!({"selector": action.selector});
                        // Box::pin 打断 async 递归(click 动作 → click_inner → 再分发)。
                        Box::pin(click_inner(mgr, pid, &p, "js")).await?;
                        Ok(None)
                    }
                    _ => Ok(None),
                };
                {
                    let mut inner = mgr.inner.lock().await;
                    if let Some(tab) = inner.tabs.get_mut(pid) {
                        release_active(tab);
                    }
                }
                match nav_result {
                    Ok(Some((url, title))) => {
                        out["final_url"] = json!(url);
                        out["title"] = json!(title);
                        return Ok(out);
                    }
                    Ok(None) => return Ok(out),
                    Err(e) => return Err(e),
                }
            }
        }
    }
    // click 动作已完成(mouseup):释放按下态,只保留 hover 链。
    {
        let mut inner = mgr.inner.lock().await;
        if let Some(tab) = inner.tabs.get_mut(pid) {
            release_active(tab);
        }
    }
    // option 点击 = 选中所属 select 并收起面板;change 事件按浏览器语义发给 select。
    if let Some((select_sel, value, label, index)) = option_pick {
        {
            let mut inner = mgr.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                tab.form_values.insert(select_sel.clone(), value.clone());
                for opened in &open_selects {
                    tab.widget_open.remove(opened);
                }
            }
        }
        let mut js_events = false;
        if let Some(reply) = js_dispatch_bounded(mgr, pid, &select_sel, "change").await {
            apply_js_reply(mgr, pid, &reply).await;
            js_events = reply.handled;
        }
        out["selected"] = json!(true);
        out["selector"] = json!(select_sel);
        out["value"] = json!(value);
        out["text"] = json!(label);
        out["index"] = json!(index);
        out["dropdown_closed"] = json!(true);
        out["js_events"] = json!(js_events);
        relayout_and_push(mgr, pid).await;
        return Ok(out);
    }

    // 点击 select 本体 = 展开/收起浮层面板;其它已开面板一并收起。
    if let Some((select_sel, next_open)) = select_toggle {
        {
            let mut inner = mgr.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                for opened in &open_selects {
                    if opened != &select_sel {
                        tab.widget_open.remove(opened);
                    }
                }
                if next_open {
                    tab.widget_open.insert(select_sel.clone(), true);
                } else {
                    tab.widget_open.remove(&select_sel);
                }
            }
        }
        out["dropdown_open"] = json!(next_open);
        out["selector"] = json!(select_sel);
        relayout_and_push(mgr, pid).await;
        return Ok(out);
    }

    // 点击 select 面板之外 = light dismiss,收起所有已展开面板。
    if !open_selects.is_empty() {
        {
            let mut inner = mgr.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                for opened in &open_selects {
                    tab.widget_open.remove(opened);
                }
            }
        }
        out["dropdown_closed"] = json!(true);
    }

    // 第 44 轮:`reset` 按钮执行**本地表单重置** —— 清掉表单内控件的运行时值,
    // 恢复 HTML 默认值(不提交表单)。`reset` 事件先发给表单,页面可拦截。
    if let Some(form_sel) = reset_form {
        let mut prevented = false;
        if let Some(reply) = js_dispatch_bounded(mgr, pid, &form_sel, "reset").await {
            apply_js_reply(mgr, pid, &reply).await;
            prevented = reply.default_prevented;
        }
        if prevented {
            out["form_reset"] = json!(false);
            out["default_prevented"] = json!(true);
        } else {
            let cleared = reset_form_controls(mgr, pid, &form_sel).await;
            out["form_reset"] = json!(true);
            out["reset_fields"] = json!(cleared);
            out["form"] = json!(form_sel);
        }
        relayout_and_push(mgr, pid).await;
        return Ok(out);
    }

    // summary 是 details 的原生展开开关:JS 未拦截时切换开合覆盖并重排。
    if let Some(details_sel) = summary_details {
        let next_open = {
            let mut inner = mgr.inner.lock().await;
            let Some(tab) = inner.tabs.get_mut(pid) else {
                return Ok(out);
            };
            let current = tab.widget_open.get(&details_sel).copied().unwrap_or_else(|| {
                tab_document(tab)
                    .select(&details_sel)
                    .ok()
                    .and_then(|mut v| v.pop())
                    .map(|n| document::attr(&n, "open").is_some())
                    .unwrap_or(false)
            });
            tab.widget_open.insert(details_sel.clone(), !current);
            !current
        };
        out["toggled_open"] = json!(next_open);
        out["details"] = json!(details_sel);
        relayout_and_push(mgr, pid).await;
        return Ok(out);
    }

    // checkbox/radio 默认切换:JS 先行且未 preventDefault 时才改运行时状态,
    // 与真实浏览器的 click 事件语义一致;radio 同 form 同 name 互斥。
    if let Some((sel, value)) = toggled {
        let mut inner = mgr.inner.lock().await;
        if let Some(tab) = inner.tabs.get_mut(pid) {
            if value == "true" {
                if let Some((form_key, name)) = &radio_scope {
                    let doc = tab_document(tab);
                    if let Ok(radios) = doc.select("input[type=\"radio\"]") {
                        for radio in radios {
                            if document::attr(&radio, "name").as_deref() != Some(name.as_str()) {
                                continue;
                            }
                            let mut same_form = form_key.is_empty();
                            if !same_form {
                                let mut cur = radio.parent();
                                while let Some(p) = cur {
                                    if document::tag_name(&p) == "form" {
                                        same_form = Document::stable_selector(&p) == *form_key;
                                        break;
                                    }
                                    cur = p.parent();
                                }
                            }
                            if same_form {
                                let rs = Document::stable_selector(&radio);
                                if rs != sel {
                                    tab.form_values.insert(rs, "false".to_string());
                                }
                            }
                        }
                    }
                }
            }
            tab.form_values.insert(sel, value);
        }
        drop(inner);
        out["toggled"] = json!(true);
        relayout_and_push(mgr, pid).await;
        return Ok(out);
    }

    let navigated = plan.is_some();
    match plan {
        Some(NavPlan::NewTab { url }) => {
            let spawned = mgr.new_tab(pid, &url, timeout).await?;
            out["spawned_page_id"] = json!(spawned.page_id);
            out["final_url"] = json!(spawned.final_url);
            out["title"] = json!(spawned.title);
        }
        Some(NavPlan::Same { url, method, body }) => {
            let nav = mgr.submit_form(pid, &url, &method, body, timeout).await?;
            out["final_url"] = json!(nav.url);
            out["title"] = json!(nav.title);
        }
        None => {}
    }
    // 第 41 轮:无导航时按新状态重排重绘(导航分支由导航自身重建布局)。
    if !navigated {
        relayout_and_push(mgr, pid).await;
    }
    Ok(out)
}

async fn act_hover(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let (stable, hover_chain, laid_out, rect) = {
        let inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = tab_document(tab);
        let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("选择器未命中"))?;
        let stable = Document::stable_selector(node);
        let hover_chain = hover_chain_selectors(node);
        let laid_out = tab.layout.rect_for(&stable).is_some();
        let rect = tab.layout.rect_for(&stable).unwrap_or((0.0, 0.0, 0.0, 0.0));
        (stable, hover_chain, laid_out, rect)
    };
    // 第 42 轮:hover 命中目标与全部祖先(`.parent:hover .child`)。
    {
        let mut inner = mgr.inner.lock().await;
        if let Some(tab) = inner.tabs.get_mut(pid) {
            set_hover_chain(tab, &hover_chain);
        }
    }
    relayout_and_push(mgr, pid).await;
    Ok(json!({
        "hovered": true, "selector": stable, "x": rect.0, "y": rect.1,
        "laid_out": laid_out,
        "note": if laid_out { Value::Null } else { json!("元素无布局盒(可能被祖先 display:none 隐藏或为非寻址行内元素),坐标不可靠") },
    }))
}

async fn act_mouse_move(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let mut hovered: Option<String> = None;
    let mut hover_chain: Vec<String> = Vec::new();
    let (x, y) = if let Some(sel) = p_str(params, "selector") {
        let doc = tab_document(tab);
        let nodes = doc.select(sel).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("选择器未命中"))?;
        let stable = Document::stable_selector(node);
        let r = tab.layout.rect_for(&stable).unwrap_or((0.0, 0.0, 0.0, 0.0));
        hover_chain = hover_chain_selectors(node);
        hovered = Some(stable);
        (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0)
    } else {
        (p_f64(params, "x").unwrap_or(0.0), p_f64(params, "y").unwrap_or(0.0))
    };
    tab.mouse = Some((x, y));
    // 第 42 轮:按选择器移动指针等价于悬停目标与祖先链。
    if !hover_chain.is_empty() {
        set_hover_chain(tab, &hover_chain);
    }
    drop(inner);
    if hovered.is_some() {
        relayout_and_push(mgr, pid).await;
    }
    Ok(json!({"x": x, "y": y, "note": "指针坐标为布局近似值(非真实设备像素)" }))
}

async fn act_input(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let text = params.get("text").and_then(Value::as_str).unwrap_or("");
    let stable = {
        let mut inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = tab_document(tab);
        let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| {
            EngineError::action_failed(format!("输入目标未命中:{selector}"))
        })?;
        let stable = Document::stable_selector(node);
        if !document::is_visible(node) {
            return Err(EngineError::action_failed("输入目标不可见").with_data(json!({"selector": stable})));
        }
        let tag = document::tag_name(node);
        if document::attr(node, "disabled").is_some() {
            return Err(EngineError::action_failed("输入目标已禁用").with_data(json!({"selector": stable, "disabled": true})));
        }
        if matches!(tag.as_str(), "input" | "textarea")
            && document::attr(node, "readonly").is_some()
        {
            return Err(EngineError::action_failed("输入目标只读").with_data(json!({"selector": stable, "readonly": true})));
        }
        tab.form_values.insert(stable.clone(), text.to_string());
        stable
    };
    // 阶段二:input + change 事件分发到页面 listener(锁外执行)。
    let mut js_events = false;
    for evt in ["input", "change"] {
        if let Some(reply) = js_dispatch_bounded(mgr, pid, &stable, evt).await {
            apply_js_reply(mgr, pid, &reply).await;
            js_events = js_events || reply.handled;
        }
    }
    relayout_and_push(mgr, pid).await;
    Ok(json!({
        "selector": stable,
        "text": text,
        "mode": "native",
        "applied": true,
        "value_after": {"len": text.chars().count(), "matched": true, "head": text.chars().take(40).collect::<String>()},
        "fallbacks": 0,
        "js_events": js_events,
    }))
}

async fn act_clear(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let stable = {
        let mut inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = tab_document(tab);
        let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("清空目标未命中"))?;
        let stable = Document::stable_selector(node);
        let tag = document::tag_name(node);
        if document::attr(node, "disabled").is_some() {
            return Err(EngineError::action_failed("清空目标已禁用").with_data(json!({"selector": stable, "disabled": true})));
        }
        if matches!(tag.as_str(), "input" | "textarea")
            && document::attr(node, "readonly").is_some()
        {
            return Err(EngineError::action_failed("清空目标只读").with_data(json!({"selector": stable, "readonly": true})));
        }
        tab.form_values.insert(stable.clone(), String::new());
        stable
    };
    relayout_and_push(mgr, pid).await;
    Ok(json!({"cleared": true, "selector": stable, "len_after": 0}))
}

async fn act_select_option(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    // 非 Send 的 DOM 句柄全部收在块内,await 前销毁。
    let (select_selector, value, idx, label, mode) = {
        let mut inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = tab_document(tab);
        let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("select 未命中"))?;
        let select = if document::tag_name(node) == "select" {
            node.clone()
        } else {
            document::closest_tag(node, "select")
                .ok_or_else(|| EngineError::action_failed("目标不是 select 且不在 select 内"))?
        };
        let select_selector = Document::stable_selector(&select);
        if document::attr(&select, "disabled").is_some() {
            return Err(EngineError::action_failed("目标 select 已禁用").with_data(json!({
                "selector": select_selector,
                "disabled": true,
            })));
        }
        let options: Vec<_> = select
            .select("option")
            .map_err(|_| EngineError::action_failed("非法选择器"))?
            .into_iter()
            .collect();
    let value_filter = p_str(params, "value").map(str::to_string);
    let values_filter: Vec<String> = params
        .get("values")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let text_filter = p_str(params, "text").map(str::to_string);
    let index_filter = p_u64(params, "index").map(|v| v as usize);

    let mut chosen: Option<(String, usize, String)> = None;
    for (i, opt) in options.iter().enumerate() {
        if document::attr(opt.as_node(), "disabled").is_some() {
            continue;
        }
        let value = document::attr(opt.as_node(), "value")
            .unwrap_or_else(|| document::collapse_ws(&opt.text_contents()));
        let label = document::collapse_ws(&opt.text_contents());
        let hit = if let Some(v) = &value_filter {
            &value == v
        } else if !values_filter.is_empty() {
            values_filter.contains(&value)
        } else if let Some(t) = &text_filter {
            &label == t || label.contains(t.as_str())
        } else if let Some(idx) = index_filter {
            i == idx
        } else {
            false
        };
        if hit {
            chosen = Some((value, i, label));
            break;
        }
    }
    if value_filter.is_none() && values_filter.is_empty() && text_filter.is_none() && index_filter.is_none() {
        return Err(EngineError::param("select_option 需要 value/values/text/index 之一"));
    }
    let (value, idx, label) = chosen.ok_or_else(|| EngineError::action_failed("没有匹配的 option"))?;
        tab.form_values.insert(select_selector.clone(), value.clone());
        // 显式选择完成后浮层面板收起(浏览器点选 option 的默认收口语义)。
        tab.widget_open.remove(&select_selector);
        let mode = if value_filter.is_some() {
            "value"
        } else if text_filter.is_some() {
            "text"
        } else if index_filter.is_some() {
            "index"
        } else {
            "value"
        };
        (select_selector, value, idx, label, mode)
    };
    // 阶段二:change 事件分发到页面 listener。
    let mut js_events = false;
    if let Some(reply) = js_dispatch_bounded(mgr, pid, &select_selector, "change").await {
        apply_js_reply(mgr, pid, &reply).await;
        js_events = reply.handled;
    }
    relayout_and_push(mgr, pid).await;
    Ok(json!({
        "selected": true,
        "selector": select_selector,
        "value": value,
        "text": label,
        "index": idx,
        "mode": mode,
        "js_events": js_events,
    }))
}

async fn act_select_dropdown(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let option_text = p_str(params, "option_text")
        .ok_or_else(|| EngineError::param("select_dropdown 需要 option_text"))?;
    let selector = p_str(params, "selector");
    let hit = {
        let mut inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = tab_document(tab);
        let selects = if let Some(sel) = selector {
            doc.select(sel).map_err(EngineError::action_failed)?
        } else {
            doc.select("select").map_err(EngineError::action_failed)?
        };
        let mut hit = None;
        for sel_node in selects {
            let options = sel_node
                .select("option")
                .map_err(|_| EngineError::action_failed("非法选择器"))?;
            for opt in options {
                let label = document::collapse_ws(&opt.text_contents());
                if label == option_text || label.contains(option_text) {
                    let value = document::attr(opt.as_node(), "value").unwrap_or_else(|| label.clone());
                    let select_selector = Document::stable_selector(&sel_node);
                    tab.form_values.insert(select_selector.clone(), value.clone());
                    hit = Some((select_selector, value));
                    break;
                }
            }
            if hit.is_some() {
                break;
            }
        }
        hit
    };
    let Some((select_selector, value)) = hit else {
        return Err(EngineError::action_failed(format!(
            "未找到包含「{option_text}」的下拉选项"
        )));
    };
    relayout_and_push(mgr, pid).await;
    Ok(json!({
        "selected": true,
        "option_text": option_text,
        "value_after": value,
        "selector": select_selector,
        "mode": "custom_dropdown",
        "note": "原生 select 已直接选中;非原生下拉在阶段二接入页面脚本后可展开",
    }))
}

/// 第 41 轮:展开/收起 `select` 的浮层 option 面板。
///
/// 展开态写入 `TabState.widget_open`,布局层据此铺出顶层浮层面板
/// (不受祖先 overflow 裁剪,画在定位层之上)。
async fn act_toggle_dropdown(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    open: bool,
) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let stable = {
        let inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = tab_document(tab);
        let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
        let node = nodes
            .get(nth(params))
            .ok_or_else(|| EngineError::action_failed("下拉目标未命中"))?;
        if document::tag_name(node).to_ascii_lowercase() != "select" {
            return Err(EngineError::action_failed("下拉目标不是 <select>"));
        }
        if document::attr(node, "multiple").is_some() {
            return Err(EngineError::action_failed("multiple select 暂不支持浮层面板"));
        }
        if document::attr(node, "disabled").is_some() {
            return Err(EngineError::action_failed("下拉目标已禁用"));
        }
        Document::stable_selector(node)
    };
    {
        let mut inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        tab.widget_open.insert(stable.clone(), open);
    }
    relayout_and_push(mgr, pid).await;
    Ok(json!({"selector": stable, "open": open}))
}

async fn act_key_press(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    require_page(pid)?;
    let key = p_str(params, "key").ok_or_else(|| EngineError::param("key_press 需要 key"))?;
    let selector = p_str(params, "selector");
    let timeout = timeout_ms(params, top, 60_000);
    // 第 44 轮:Esc 关闭**最上层打开的 dialog**(浏览器语义:顶层浮层优先)。
    if key.eq_ignore_ascii_case("escape") || key.eq_ignore_ascii_case("esc") {
        if let Some(sel) = close_top_dialog(mgr, pid).await? {
            relayout_and_push(mgr, pid).await;
            return Ok(json!({
                "key": key, "mode": "keyevent", "submitted": false,
                "closed": true, "selector": sel,
            }));
        }
    }
    if key.eq_ignore_ascii_case("enter") {
        if let Some((url, method, body)) = resolve_submit(mgr, pid, selector).await? {
            // 阶段二:submit 事件先分发,preventDefault 可拦截提交(冒泡命中 form listener)。
            if let Some(dispatch_sel) = selector {
                if let Some(reply) = js_dispatch(mgr, pid, dispatch_sel, "submit").await {
                    apply_js_reply(mgr, pid, &reply).await;
                    if reply.default_prevented {
                        return Ok(json!({
                            "key": key, "mode": "keyevent", "submitted": false,
                            "default_prevented": true,
                        }));
                    }
                }
            }
            let nav = mgr.submit_form(pid, &url, &method, body, timeout).await?;
            return Ok(json!({
                "key": key, "mode": "keyevent", "submitted": true,
                "final_url": nav.url, "title": nav.title,
            }));
        }
    }
    Ok(json!({"key": key, "mode": "keyevent", "submitted": false}))
}

async fn act_press_sequence(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    require_page(pid)?;
    let keys: Vec<String> = match params.get("keys") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(s)) => s.split_whitespace().map(str::to_string).collect(),
        _ => return Err(EngineError::param("press_sequence 需要 keys")),
    };
    let selector = p_str(params, "selector");
    let timeout = timeout_ms(params, top, 60_000);
    let mut submitted = false;
    for key in &keys {
        if key.eq_ignore_ascii_case("enter") {
            if let Some((url, method, body)) = resolve_submit(mgr, pid, selector).await? {
                if let Some(dispatch_sel) = selector {
                    if let Some(reply) = js_dispatch(mgr, pid, dispatch_sel, "submit").await {
                        apply_js_reply(mgr, pid, &reply).await;
                        if reply.default_prevented {
                            return Ok(json!({
                                "pressed": true, "keys": keys, "focused": true,
                                "submitted": false, "default_prevented": true,
                            }));
                        }
                    }
                }
                let nav = mgr.submit_form(pid, &url, &method, body, timeout).await?;
                submitted = true;
                return Ok(json!({
                    "pressed": true, "keys": keys, "focused": selector.is_some(),
                    "submitted": submitted, "final_url": nav.url,
                }));
            }
        }
    }
    Ok(json!({"pressed": true, "keys": keys, "focused": selector.is_some(), "submitted": submitted}))
}

async fn resolve_submit(
    mgr: &LsmBrowserManager,
    pid: &str,
    selector: Option<&str>,
) -> EngineResult<Option<(String, String, Option<Vec<u8>>)>> {
    let inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = tab_document(tab);
    let target = if let Some(sel) = selector {
        doc.select(sel)
            .map_err(EngineError::action_failed)?
            .into_iter()
            .next()
    } else if let Some(focused) = &tab.focused {
        doc.select(focused)
            .ok()
            .and_then(|nodes| nodes.into_iter().next())
    } else {
        None
    };
    let Some(node) = target else {
        return Ok(None);
    };
    let form = doc.form_request(&node, &tab.form_values);
    Ok(form.map(|f| {
        let (url, body) = f.to_request();
        (url, f.method, body)
    }))
}

async fn act_scroll(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    if let Some(sel) = p_str(params, "selector") {
        let doc = tab_document(tab);
        let nodes = doc.select(sel).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("滚动目标未命中"))?;
        let stable = Document::stable_selector(node);
        if let Some((_, y, _, h)) = tab.layout.rect_for(&stable) {
            tab.scroll_y = (y - tab.viewport.height as f64 / 3.0 + h / 2.0).max(0.0);
            if tab.mode.is_headed() {
                mgr.push_frame(tab);
            }
            return Ok(json!({"mode": "selector_into_view", "found": true, "selector": stable, "scrollY": tab.scroll_y}));
        }
        return Ok(json!({"mode": "selector_into_view", "found": false, "selector": stable}));
    }
    let dx = p_f64(params, "delta_x").unwrap_or(0.0);
    let dy = p_f64(params, "delta_y")
        .or_else(|| p_f64(params, "y"))
        .unwrap_or(0.0);
    tab.scroll_x = (tab.scroll_x + dx).max(0.0);
    tab.scroll_y = (tab.scroll_y + dy).max(0.0);
    if tab.mode.is_headed() {
        mgr.push_frame(tab);
    }
    Ok(json!({"mode": "scrollBy", "delta_x": dx, "delta_y": dy, "scrollX": tab.scroll_x, "scrollY": tab.scroll_y}))
}

async fn act_scroll_to(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = tab_document(tab);
    let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
    let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("滚动目标未命中"))?;
    let stable = Document::stable_selector(node);
    let rect = tab.layout.rect_for(&stable);
    if let Some(r) = rect {
        tab.scroll_y = (r.1 - tab.viewport.height as f64 / 3.0).max(0.0);
    }
    if tab.mode.is_headed() {
        mgr.push_frame(tab);
    }
    Ok(json!({"selector": stable, "block": p_str(params, "block").unwrap_or("center"), "found": rect.is_some(), "scrollY": tab.scroll_y, "rect": rect.map(|r| json!({"x": r.0, "y": r.1, "width": r.2, "height": r.3}))}))
}

async fn act_scroll_to_text(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let text = p_str(params, "text").ok_or_else(|| EngineError::param("scroll_to_text 需要 text"))?;
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = tab_document(tab);
    let node = doc
        .find_by_text(text)
        .ok_or_else(|| EngineError::action_failed(format!("页面文本未找到:{text}")))?;
    let stable = Document::stable_selector(&node);
    let rect = tab.layout.rect_for(&stable).unwrap_or((0.0, 0.0, 0.0, 0.0));
    tab.scroll_y = (rect.1 - tab.viewport.height as f64 / 3.0).max(0.0);
    Ok(json!({
        "text": text, "block": p_str(params, "block").unwrap_or("center"),
        "found": true, "tag": document::tag_name(&node), "selector": stable,
        "text_len": node.text_contents().chars().count(),
        "head": document::truncate_chars(&document::collapse_ws(&node.text_contents()), 120),
        "scrollY": tab.scroll_y,
    }))
}

async fn act_focus(mgr: &LsmBrowserManager, pid: &str, params: &Value, mode: &str) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let stable = {
        let inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = tab_document(tab);
        let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("目标未命中"))?;
        Document::stable_selector(node)
    };
    {
        let mut inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        if mode == "focus" {
            tab.focused = Some(stable.clone());
            // 第 41 轮:焦点进入运行时伪类状态 → `:focus`/`:focus-visible` 命中。
            set_pseudo_state(tab, &stable, PseudoDim::Focus, true);
        } else {
            tab.focused = None;
            set_pseudo_state(tab, &stable, PseudoDim::Focus, false);
        }
    }
    relayout_and_push(mgr, pid).await;
    Ok(json!({"selector": stable, "mode": mode, "active": mode == "focus"}))
}

async fn act_dispatch_event(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    // `event` 兼容两种形状:事件名字符串("mousedown")或事件对象({"type":...}),
    // 并接受 `params.type` 兜底。模型常按 DOM `dispatchEvent(new Event(...))`
    // 的直觉传对象,此前一律 1001,连续操作被打断(实测百度地图路线按钮)。
    let event = p_str(params, "event")
        .map(str::to_string)
        .or_else(|| {
            params
                .get("event")
                .and_then(|e| e.get("type"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| p_str(params, "type").map(str::to_string))
        .ok_or_else(|| EngineError::param("dispatch_event 需要 event(事件名字符串或 {type:...} 对象)"))?;
    let ev = event.to_ascii_lowercase();
    if ev == "click" {
        let mut out = click_inner(mgr, pid, params, "event").await?;
        out["event"] = json!(event);
        out["dispatched"] = json!(true);
        return Ok(out);
    }
    if ev == "submit" {
        if let Some((url, method, body)) = resolve_submit(mgr, pid, Some(selector)).await? {
            if let Some(reply) = js_dispatch(mgr, pid, selector, "submit").await {
                apply_js_reply(mgr, pid, &reply).await;
                if reply.default_prevented {
                    return Ok(json!({"selector": selector, "event": event, "dispatched": true, "default_prevented": true}));
                }
            }
            let nav = mgr
                .submit_form(pid, &url, &method, body, timeout_ms(params, top, 60_000))
                .await?;
            return Ok(json!({"selector": selector, "event": event, "dispatched": true, "final_url": nav.url}));
        }
    }
    // 其余事件类型:优先分发到页面 listener(阶段二),无 listener 时保持记录语义。
    if let Some(reply) = js_dispatch(mgr, pid, selector, &ev).await {
        apply_js_reply(mgr, pid, &reply).await;
        return Ok(json!({
            "selector": selector, "event": event, "dispatched": true,
            "js_handled": reply.handled,
        }));
    }
    Ok(json!({"selector": selector, "event": event, "dispatched": true}))
}

async fn act_upload(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let files: Vec<String> = match params.get("file_paths") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => p_str(params, "file_path").map(|s| vec![s.to_string()]).unwrap_or_default(),
    };
    if files.is_empty() {
        return Err(EngineError::param("upload_file 需要 file_paths 或 file_path"));
    }
    let stable = {
        let mut inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = tab_document(tab);
        let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("上传控件未命中"))?;
        if document::tag_name(node) != "input"
            || document::attr(node, "type")
                .unwrap_or_default()
                .to_ascii_lowercase()
                != "file"
        {
            return Err(EngineError::action_failed("上传目标不是 input[type=file]"));
        }
        if document::attr(node, "disabled").is_some() {
            return Err(EngineError::action_failed("上传目标已禁用"));
        }
        let stable = Document::stable_selector(node);
        tab.form_values.insert(stable.clone(), files.join(","));
        stable
    };
    // 文件名进入控件内置 UI,必须立即重排推帧;否则截图继续显示「选择文件」。
    relayout_and_push(mgr, pid).await;
    Ok(json!({"selector": stable, "files": files, "count": files.len(), "note": "阶段一只记录文件路径;真实文件读取在阶段四接入"}))
}

async fn act_drag(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = tab_document(tab);
    let point = |sel_key: &str, x_key: &str, y_key: &str| -> Option<(f64, f64)> {
        if let Some(sel) = p_str(params, sel_key) {
            let nodes = doc.select(sel).ok()?;
            let node = nodes.first()?;
            let stable = Document::stable_selector(node);
            let r = tab.layout.rect_for(&stable).unwrap_or((0.0, 0.0, 0.0, 0.0));
            Some((r.0 + r.2 / 2.0, r.1 + r.3 / 2.0))
        } else {
            Some((p_f64(params, x_key)?, p_f64(params, y_key)?))
        }
    };
    let from = point("source_selector", "source_x", "source_y")
        .ok_or_else(|| EngineError::param("drag 需要 source_selector 或 source_x/source_y"))?;
    let to = point("target_selector", "target_x", "target_y")
        .ok_or_else(|| EngineError::param("drag 需要 target_selector 或 target_x/target_y"))?;
    Ok(json!({"dragged_from": [from.0, from.1], "to": [to.0, to.1], "note": "阶段一只记录拖拽轨迹"}))
}

async fn act_set_guard(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let mode = p_str(params, "mode").unwrap_or("locked");
    if !matches!(mode, "locked" | "open" | "partial") {
        return Err(EngineError::param("guard.mode 允许 locked/open/partial"));
    }
    let allow: Vec<String> = params
        .get("allow_selectors")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let block: Vec<String> = params
        .get("block_selectors")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if mode == "partial" && allow.is_empty() && block.is_empty() {
        return Err(EngineError::param("partial 需要 allow_selectors 或 block_selectors"));
    }
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    tab.guard = super::GuardState {
        mode: mode.to_string(),
        allow_selectors: allow.clone(),
        block_selectors: block.clone(),
        note: p_str(params, "note").map(str::to_string),
    };
    Ok(json!({
        "mode": mode,
        "allow_selectors": allow,
        "block_selectors": block,
        "note": p_str(params, "note"),
        "supported": false,
        "hint": "阶段一无原生窗口;管控状态仅记录,不影响进程内 DOM 自动化",
    }))
}

/// 就绪判定:元素状态/文本、页面 JS 表达式、网络静默(可组合,取 AND)。
///
/// 阶段五第一步。旧实现只支持 `selector` + `state`,且状态语义有 bug
/// (`hidden`/`detached` 恒为真)。现在:
/// - `state`:visible / hidden / attached / detached,语义按标准;
/// - `text`:元素文本需包含该子串;
/// - `js`:页面 JS 表达式求值为真(走页面 JS 线程,回退阶段一快照桥);
/// - `network_idle`/`network_idle_ms`:网络台账静默窗口(毫秒)。
async fn act_wait(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = p_str(params, "selector").map(str::to_string);
    let state = p_str(params, "state")
        .unwrap_or("visible")
        .trim()
        .to_ascii_lowercase();
    let text_cond = p_str(params, "text").map(str::to_string);
    let js_cond = p_str(params, "js")
        .or_else(|| p_str(params, "expression"))
        .map(str::to_string);
    // `network_idle` 允许数字(毫秒)或布尔(true = 默认 800ms)。
    let idle_ms = p_u64(params, "network_idle_ms")
        .or_else(|| p_u64(params, "idle_ms"))
        .or_else(|| p_u64(params, "network_idle"))
        .or_else(|| match params.get("network_idle") {
            Some(Value::Bool(true)) => Some(800),
            _ => None,
        });
    let total = timeout_ms(params, top, 30_000).min(120_000);

    // 无任何条件:等待期间仍推进页面事件循环;纯 sleep 会让动态弹层/验证组件
    // 在没有任何新 <script> 可装载时永远停在旧 DOM。
    if selector.is_none() && js_cond.is_none() && idle_ms.is_none() {
        let ms = p_u64(params, "duration_ms")
            .or_else(|| p_u64(params, "ms"))
            .or_else(|| p_u64(params, "wait_ms"))
            .unwrap_or(500)
            .min(30_000);
        let started = std::time::Instant::now();
        loop {
            sync_page(mgr, pid).await;
            let elapsed = started.elapsed().as_millis() as u64;
            if elapsed >= ms {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis((ms - elapsed).min(100))).await;
        }
        return Ok(json!({"slept_ms": ms}));
    }

    let started = std::time::Instant::now();
    let mut last = json!({});
    loop {
        sync_page(mgr, pid).await;
        let mut cond = serde_json::Map::new();
        let mut ok = true;
        if let Some(sel) = &selector {
            let (hit, detail) = check_element_state(mgr, pid, sel, &state, text_cond.as_deref()).await;
            cond.insert(
                "element".to_string(),
                json!({"selector": sel, "state": state, "ok": hit, "detail": detail}),
            );
            ok &= hit;
        }
        if let Some(expr) = &js_cond {
            let hit = js_condition(mgr, pid, expr).await;
            cond.insert("js".to_string(), json!({"expression": expr, "ok": hit}));
            ok &= hit;
        }
        if let Some(idle) = idle_ms {
            let (hit, ago) = network_idle_state(mgr, pid, idle).await;
            cond.insert(
                "network_idle".to_string(),
                json!({"idle_ms": idle, "ok": hit, "last_request_ms_ago": ago}),
            );
            ok &= hit;
        }
        if ok {
            return Ok(json!({
                "waited_for": selector.clone().unwrap_or_default(),
                "state": state,
                "conditions": Value::Object(cond),
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }));
        }
        last = Value::Object(cond);
        if started.elapsed().as_millis() as u64 >= total {
            return Err(
                EngineError::action_failed(format!("等待超时({total}ms)")).with_data(last)
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// 元素状态 + 文本条件判定。
///
/// `attached`=存在;`detached`=不存在;`visible`=存在且可见;`hidden`=存在且不可见。
async fn check_element_state(
    mgr: &LsmBrowserManager,
    pid: &str,
    selector: &str,
    state: &str,
    text_cond: Option<&str>,
) -> (bool, Value) {
    let (html, url) = {
        let inner = mgr.inner.lock().await;
        let Some(tab) = inner.tabs.get(pid) else {
            return (false, json!("页面不存在"));
        };
        (tab.html.clone(), tab.url.clone())
    };
    let doc = Document::parse(&html, &url);
    let nodes = match doc.select(selector) {
        Ok(n) => n,
        Err(e) => return (false, json!(e)),
    };
    let first = nodes.first();
    let exists = first.is_some();
    let visible = first.map(|n| document::is_visible(n)).unwrap_or(false);
    let matched = match state {
        "attached" => exists,
        "detached" => !exists,
        "hidden" => exists && !visible,
        "visible" => exists && visible,
        // 未知状态按 attached 处理(不静默判真)。
        _ => exists,
    };
    let text_ok = match (text_cond, first) {
        (Some(needle), Some(node)) => document::node_text(node).contains(needle),
        (Some(_), None) => false,
        (None, _) => true,
    };
    (matched && text_ok, json!({"exists": exists, "visible": visible, "text_ok": text_ok}))
}

/// JS 条件判定:表达式包一层 `!!(...)` 取真值。
///
/// 优先走页面 JS 线程(持久上下文,能看到页面真实全局态);线程不可用
/// (`LAEW_WEB_SCRIPT=off` / 超时)回退阶段一快照桥。
async fn js_condition(mgr: &LsmBrowserManager, pid: &str, expr: &str) -> bool {
    let script = format!("!!({expr})");
    match js_eval_thread(mgr, pid, &script).await {
        ThreadEval::Reply(reply) => return reply.outcome.value.trim() == "true",
        // 忙碌超时:条件判定宁可判「未就绪」(调用方会继续等/超时收口),
        // 也不要落到阶段一桥上拿另一套语义环境的结果。
        ThreadEval::Busy => return false,
        ThreadEval::NoThread => {}
    }
    let inner = mgr.inner.lock().await;
    let Some(tab) = inner.tabs.get(pid) else {
        return false;
    };
    let doc = tab_document(tab);
    let nodes = doc.snapshot_nodes();
    let empty = empty_map();
    let meta = JsPageMeta {
        url: &tab.url,
        title: &tab.title,
        cookies: &inner.cookies.header_for(&tab.url).unwrap_or_default(),
        local_storage: inner
            .local_storage
            .get(&urlutil::storage_origin(&tab.url))
            .unwrap_or(&empty),
        session_storage: &tab.session_storage,
        viewport_width: tab.viewport.width,
        viewport_height: tab.viewport.height,
        scroll_x: tab.scroll_x,
        scroll_y: tab.scroll_y,
        user_agent: &tab.user_agent,
    };
    js::eval(&script, &nodes, &meta)
        .map(|o| o.value.trim() == "true")
        .unwrap_or(false)
}

/// 网络静默判定:台账末条请求距今是否已超过静默窗口。
async fn network_idle_state(
    mgr: &LsmBrowserManager,
    pid: &str,
    idle_ms: u64,
) -> (bool, Option<u128>) {
    let inner = mgr.inner.lock().await;
    let Some(tab) = inner.tabs.get(pid) else {
        return (false, None);
    };
    let now = super::now_ms();
    match tab.network.last() {
        Some(rec) => {
            let ago = now.saturating_sub(rec.at_ms);
            (ago >= idle_ms as u128, Some(ago))
        }
        // 从未发起过请求:无待完成网络,视为已静默。
        None => (true, None),
    }
}

async fn act_eval(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    require_page(pid)?;
    let script = ["expression", "script", "function", "js", "code"]
        .iter()
        .find_map(|k| p_str(params, k))
        .ok_or_else(|| EngineError::param("eval_js 需要 expression"))?
        .to_string();
    // 第 181 轮:顶层 return 自动 IIFE 包裹(Boa 同样不允许顶层 return;
    // 与 Chrome/CDP 侧共用 js_preprocess,详见该模块文档)。
    let script = crate::agent::js_preprocess::wrap_top_level_return(&script);
    let await_promise = p_bool(params, "await_promise", false);
    let _ = timeout_ms(params, top, 30_000);

    // 阶段二:优先走页面 JS 线程(持久全局,DOM 修改落回文档);
    // 线程不可用(LAEW_WEB_SCRIPT=off / 线程退出)回退阶段一快照桥。
    // **线程忙碌超时不再静默回退** —— 阶段一桥是另一套语义环境,静默回退
    // 会让调用方拿到失真结果(几何全 0 / 全局状态另一份);显式报忙,
    // 调用方 wait 后重试即可(句柄保留,页面全局状态无损)。
    match js_eval_thread(mgr, pid, &script).await {
        ThreadEval::Reply(reply) => {
            let pending = apply_js_reply(mgr, pid, &reply).await;
            return finish_eval(mgr, pid, reply.outcome, pending, await_promise).await;
        }
        ThreadEval::Busy => {
            return Err(EngineError::new(
                2002,
                "页面 JS 线程忙碌(事件循环仍在排定时器/微任务),本次 eval 未执行;\
                 请 control(wait) 1-3 秒后重试;线程与页面全局状态保持无损",
            ));
        }
        ThreadEval::NoThread => {}
    }

    let (outcome, pending, runtime_state_changed) = {
        let mut inner = mgr.inner.lock().await;
        let some_tab = inner.tabs.get(pid).cloned();
        let Some(mut tab) = some_tab else {
            return Err(EngineError::page_not_found(pid));
        };
        let doc = tab_document(&tab);
        let rects = tab.layout.rect_map();
        let nodes = doc.snapshot_nodes_with_rects((tab.viewport.width, tab.viewport.height), &rects);
        let cookie_header = inner.cookies.header_for(&tab.url).unwrap_or_default();
        let empty = empty_map();
        let meta = JsPageMeta {
            url: &tab.url,
            title: &tab.title,
            cookies: &cookie_header,
            local_storage: inner
                .local_storage
                .get(&urlutil::storage_origin(&tab.url))
                .unwrap_or(&empty),
            session_storage: &tab.session_storage,
            viewport_width: tab.viewport.width,
            viewport_height: tab.viewport.height,
            scroll_x: tab.scroll_x,
            scroll_y: tab.scroll_y,
            user_agent: &tab.user_agent,
        };
        let outcome = js::eval(&script, &nodes, &meta).map_err(EngineError::action_failed)?;

        // 非导航动作直接落到标签页/存储。
        let mut pending: Vec<js::JsAction> = Vec::new();
        let mut runtime_state_changed = false;
        for action in &outcome.actions {
            match action.kind.as_str() {
                "set_value" => {
                    tab.form_values.insert(action.selector.clone(), action.value.clone());
                    runtime_state_changed = true;
                }
                "set_checked" => {
                    tab.form_values.insert(action.selector.clone(), action.value.clone());
                    runtime_state_changed = true;
                }
                "set_cookie" => {
                    if let Some(t) = inner.tabs.get(pid) {
                        let url = t.url.clone();
                        inner.cookies.set_from_headers(
                            &url,
                            &[("set-cookie".to_string(), action.value.clone())],
                        );
                    }
                }
                "storage_local" => {
                    let origin = urlutil::storage_origin(&tab.url);
                    inner
                        .local_storage
                        .entry(origin)
                        .or_default()
                        .insert(action.key.clone(), action.value.clone());
                }
                "storage_local_remove" => {
                    let origin = urlutil::storage_origin(&tab.url);
                    if let Some(m) = inner.local_storage.get_mut(&origin) {
                        m.remove(&action.key);
                    }
                }
                "storage_local_clear" => {
                    let origin = urlutil::storage_origin(&tab.url);
                    if let Some(m) = inner.local_storage.get_mut(&origin) {
                        m.clear();
                    }
                }
                "storage_session" => {
                    tab.session_storage.insert(action.key.clone(), action.value.clone());
                }
                "storage_session_remove" => {
                    tab.session_storage.remove(&action.key);
                }
                "storage_session_clear" => tab.session_storage.clear(),
                "set_title" => tab.title = action.value.clone(),
                "scroll" => {
                    if let Ok(v) = action.value.parse::<f64>() {
                        tab.scroll_y = v.max(0.0);
                    }
                }
                "scroll_by" => {
                    if let Ok(v) = action.value.parse::<f64>() {
                        tab.scroll_y = (tab.scroll_y + v).max(0.0);
                    }
                }
                "focus" => {
                    set_pseudo_state(&mut tab, &action.selector, PseudoDim::Focus, true);
                    tab.focused = Some(action.selector.clone());
                    runtime_state_changed = true;
                }
                "blur" => {
                    if let Some(prev) = tab.focused.clone() {
                        set_pseudo_state(&mut tab, &prev, PseudoDim::Focus, false);
                    }
                    tab.focused = None;
                    runtime_state_changed = true;
                }
                "fetch" => {
                    tab.network.push(super::NetworkRecord {
                        url: action.url.clone(),
                        method: "FETCH".to_string(),
                        status: 0,
                        content_type: String::new(),
                        bytes: 0,
                        elapsed_ms: 0,
                        redirect: false,
                        from_cache: false,
                        at_ms: super::now_ms(),
                    });
                }
                "click" | "navigate" | "new_tab" | "back" | "forward" | "reload" | "submit" => {
                    pending.push(action.clone());
                }
                _ => {}
            }
        }
        // 把非导航状态写回。
        if let Some(t) = inner.tabs.get_mut(pid) {
            t.form_values = tab.form_values;
            t.session_storage = tab.session_storage;
            t.title = tab.title;
            t.scroll_y = tab.scroll_y;
            t.focused = tab.focused;
            t.pseudo_states = tab.pseudo_states;
            if tab.network.len() > t.network.len() {
                t.network = tab.network;
            }
        }
        (outcome, pending, runtime_state_changed)
    };

    // 回退桥没有 apply_js_reply 的统一重排入口;运行时状态变化后必须显式刷新,
    // 否则 el.focus() 写进了状态但 :focus 样式与截图仍停留在旧布局。
    if runtime_state_changed {
        relayout_and_push(mgr, pid).await;
    }

    // 导航/点击动作在锁外执行。
    finish_eval(mgr, pid, outcome, pending, await_promise).await
}

/// eval_js 收尾:执行导航类动作并组装响应信封。
async fn finish_eval(
    mgr: &LsmBrowserManager,
    pid: &str,
    outcome: js::JsOutcome,
    pending: Vec<js::JsAction>,
    await_promise: bool,
) -> EngineResult<Value> {
    let mut applied = 0usize;
    for action in &pending {
        let result: EngineResult<()> = match action.kind.as_str() {
            "navigate" => mgr.navigate(pid, &action.url, 60_000).await.map(|_| ()),
            "click" | "submit" => {
                let p = json!({"selector": action.selector});
                click_inner(mgr, pid, &p, "js").await.map(|_| ())
            }
            "new_tab" => mgr.new_tab(pid, &action.url, 60_000).await.map(|_| ()),
            "back" | "forward" | "reload" => mgr
                .history_nav(pid, action.kind.as_str(), 60_000)
                .await
                .map(|_| ()),
            _ => Ok(()),
        };
        if result.is_ok() {
            applied += 1;
        }
    }
    // 第 174 轮观测性:带上脚本管线截断状态。此前「登录弹层差一口气装不上」
    // 只在 console 里有条 warn,工具结果毫无感知,Agent 只能盲探测;
    // 现在结果直接给 script_pending/script_budget_cut,Agent 可据此 wait/重试。
    let (script_pending, script_budget_cut) = {
        let inner = mgr.inner.lock().await;
        inner
            .tabs
            .get(pid)
            .map(|t| (t.script_pending, t.script_budget_cut))
            .unwrap_or((0, false))
    };
    let mut data = json!({
        "result": outcome.value,
        "value_type": outcome.value_type,
        "await_promise": await_promise,
        "ok": outcome.ok,
        "actions_applied": applied,
        "script_pending": script_pending,
        "script_budget_cut": script_budget_cut,
        "console": outcome.logs.iter().map(|l| json!({"level": l.level, "text": l.text})).collect::<Vec<_>>(),
    });
    if let Some(err) = outcome.error {
        data["js_error"] = json!(err);
    }
    if outcome.truncated {
        data["truncated"] = json!(true);
        data["truncate_hint"] = json!("返回值超过 256KB,已截断;可拆分表达式或分页读取");
    }
    if await_promise {
        data["promise_note"] = json!("阶段二 JS 线程在脚本结束后驱动微任务;已 resolve 的 Promise/await 续体会执行");
    }
    Ok(data)
}

fn empty_map() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::new()
}
