//! 屏蔽页面状态栏 / 屏蔽页面进度栏（第 171 轮）：headed 页面右上角与左下角
//! 两条常驻告知栏的注入与推送。
//!
//! 为什么独立于 `browser_overlay.rs`（guard 管控脚本）：
//! - guard 脚本有**版本化重装**机制（`%NS%GuardRev` + `%NS%GuardOff`，高版本先
//!   卸载旧实例再重装），视觉层随重装一起被拆掉重建；两条栏必须**跨重装存活**
//!   （重装期间人工正在看的就是这两条栏，闪一下就被拆掉等于信息丢失）。
//! - 语义不同：guard 管「能不能操作」，本模块管「谁在控制 / 正在干什么」。
//! - 行数约束：`browser_overlay.rs` 已接近 1800 行上限（见 AGENTS.md 约定）。
//!
//! 两条栏的职责（人工看浏览器时最需要的三个问题）：
//!
//! | 栏 | 位置 | 回答 | 内容 |
//! | -- | ---- | ---- | ---- |
//! | **屏蔽页面状态栏** | 右上角 | 谁在控制 / 控制成什么样 / 我该怎么配合 | 具体 Agent 名 + 三档档位 + 静态注意事项 + `guard_note` |
//! | **屏蔽页面进度栏** | 左下角 | 正在干什么 / 进度到哪 / 刚发生了什么 | 当前动作 + 迭代进度与耗时 + 任务摘要 + 最近事件时间线 |
//!
//! 隐身铁律（第 169 轮）：两条栏整体挂 **closed Shadow DOM** —— 品牌文本留在
//! light DOM 会被页面一行 `document.body.innerText` 采集（实测京东风控 JS 据此把
//! 设备判为自动化，返回「当前页面异常」）。attachShadow 不可用时 fail-open 回退
//! light DOM（只损隐身，不损功能）。
//!
//! 命中测试：`pointer-events:none` + z-index 低于 guard 蒙层（2147483646），
//! 既不挡人工点击，也不影响 partial 盾区（2147483645）与页面元素。
//!
//! 两条铁律（2026-10-14 天猫 baxia 实测事故）：
//! - **只在顶层 frame 渲染**：`addScriptToEvaluateOnNewDocument` 会注入每个新文档（含
//!   跨域 iframe），baxia 滑块挑战 iframe（~390px 宽）内也长出一条
//!   `max-width:min(50vw,620px)` 的状态栏，把挑战控件罩死 —— 人工核验窗口被屏蔽、
//!   滑块没法操作。状态栏与进度栏同样必须 `window.top === window` 门控（REV 2）。
//! - **HITL 解锁期间挂起**：`request_human(unlock_page=true)` 人工操作期间两条栏
//!   整体 `set_suspended(true)`（挂起点在 `hitl_ctl::apply_hitl_unlock` 与导航重放
//!   协程，收口恢复）—— 人工核验期间页面里不该有任何 Agent 视觉元素叠在挑战层上，
//!   「页面已解锁」等信息由原生 HITL 弹窗承担。
//!
//! 设计详案 `docs/MCP_Web_Use/24-屏蔽页面状态栏与进度栏.md`。

use chromiumoxide::Page;
use serde_json::{json, Value};

use crate::agent::browser_ns::{inject_ns, ns_global, render_ns};
use crate::agent::browser_overlay::PageGuardConfig;

/// 两条栏的 closed Shadow DOM host id（主文档 light DOM 只留这个无文本 host）。
pub fn status_host_id() -> String {
    format!("{}status_ui__", inject_ns())
}

/// 屏蔽页面状态栏元素 id（右上角）。
pub fn status_bar_id() -> String {
    format!("{}guard_status__", inject_ns())
}

/// 屏蔽页面进度栏元素 id（左下角）。
pub fn progress_bar_id() -> String {
    format!("{}guard_progress__", inject_ns())
}

/// 事件时间线在页面侧的保留条数（JS `MAX_EVENTS` 同步值，见下方模板；超出后
/// 滚动丢弃，人工只看最近动态）。
#[cfg(test)]
const EVENT_WINDOW: usize = 6;

/// 三档档位与静态注意事项文案（**原左下角状态条整体迁入状态栏**，第 171 轮）。
///
/// `scroll_watch=true` 为 locked 档缺省（`LAEW_WEB_GUARD` 未关 + DOM 输入闸在场），
/// 此时人工可滚动观看、点击/键盘被拦；为 false（杀开关关闭 / 闸不在场回退 CDP
/// 全量拦截）时滚轮一并被拦，文案必须说清，否则人工会反复尝试滚动。
pub fn guard_text(mode: &str, scroll_watch: bool, human_unlocked: bool) -> (String, String) {
    if human_unlocked {
        return (
            "🔓 人工介入中 · 页面已全面解锁".to_string(),
            "可自由操作页面;完成后回弹窗点「提交 / 继续」交还控制权".to_string(),
        );
    }
    match mode {
        "open" => (
            "🔓 页面开放 · 人工可直接操作".to_string(),
            "Agent 协同中;人工完成操作后应答「人工介入」弹窗告知".to_string(),
        ),
        "partial" => (
            "🛡 部分锁定 · 盾区不可操作".to_string(),
            "其余区域人工可直接操作;需要调整范围请告知 Agent".to_string(),
        ),
        _ if scroll_watch => (
            "🔒 页面已锁定(仅观看·可滚动)".to_string(),
            "人工可滚动浏览;点击/键盘/选中已拦截,如需操作请应答「人工介入」弹窗".to_string(),
        ),
        _ => (
            "🔒 页面已锁定(仅观看)".to_string(),
            "人工点击/键盘/滚动/选中已拦截;如需介入请应答「人工介入」弹窗".to_string(),
        ),
    }
}

/// 注入脚本：两条栏 + 页面侧 API。
///
/// 占位符：`%NS%`（命名空间，见 `browser_ns.rs`）；其余全部由页面侧状态驱动，
/// Rust 侧只推 JSON —— **不在模板里烘焙任何文案**，保证「档位/动作/Agent」三类
/// 信息只有一个真源（Rust 侧 `sync` / `broadcast`）。
pub const AGENT_STATUS_JS_TEMPLATE: &str = r#"(() => {
    // 第 171 轮:版本化单实例 —— 本脚本会被多次注入(highlight 与 guard 两条通道
    // 各注入一次,派生页/重注入再来),而事件时间线/耗时起点/显隐标志都是**闭包
    // 状态**。旧实现每次注入都新建一份闭包并覆盖 window 上的 API,后注入者把
    // `last/events/visible` 全部重置(实测:`set_highlight(false)` 隐藏后又被下一
    // 次注入的实例渲染回来;事件时间线清零)。改成与 guard 同款的版本门:
    // 同版本直接返回(仅补一次 DOM 自愈),高版本才重建。
    // REV 2(2026-10-14):状态栏加 `window.top === window` 顶层门 —— 本脚本会注入
    // 每个新文档(含跨域 iframe),baxia 滑块挑战 iframe 内的状态栏把人工核验控件
    // 罩死(实测天猫任务);子 frame 一律不渲染任何一条栏。
    const REV = 2;
    if (window.%NS%StatusRev >= REV) {
        try { if (window.%NS%StatusEnsure) window.%NS%StatusEnsure(); } catch (e) {}
        return;
    }
    window.%NS%StatusRev = REV;
    const HOST_ID = '%NS%status_ui__';
    const STATUS_ID = '%NS%guard_status__';
    const PROGRESS_ID = '%NS%guard_progress__';
    const FONT = 'system-ui,-apple-system,"PingFang SC","Microsoft YaHei",sans-serif';
    const MAX_EVENTS = 6;
    let SH = null;
    let events = [];
    let last = null;
    let visible = true;
    let suspended = false;
    const root = () => document.documentElement || document.body;
    const el = (id) => (SH && SH.getElementById(id)) || document.getElementById(id);
    const row = (parent, cls, color) => {
        const d = document.createElement('div');
        d.setAttribute('data-%NS%agent', '1');
        d.className = cls;
        d.style.cssText = 'font:' + (cls.indexOf('title') >= 0 ? '700 13px/1.6 ' : '400 12px/1.6 ')
            + FONT + ';color:' + color
            + ';white-space:nowrap;overflow:hidden;text-overflow:ellipsis;max-width:min(46vw,560px);';
        parent.appendChild(d);
        return d;
    };
    const ensure = () => {
        const r = root();
        if (!r) return;
        if (!document.getElementById(HOST_ID) || !SH) {
            let h = document.getElementById(HOST_ID);
            if (!h) {
                h = document.createElement('div');
                h.id = HOST_ID;
                h.setAttribute('data-%NS%agent', '1');
                h.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483644;';
                r.appendChild(h);
            }
            try { SH = h.attachShadow ? h.attachShadow({ mode: 'closed' }) : null; } catch (e) { SH = null; }
        }
        // 顶层门(与下方进度栏同款):子 frame(含跨域挑战 iframe)不渲染状态栏。
        if (window.top === window && !el(STATUS_ID)) {
            const c = document.createElement('div');
            c.id = STATUS_ID;
            c.setAttribute('data-%NS%agent', '1');
            c.style.cssText = 'position:fixed;top:12px;right:12px;margin:0;padding:8px 14px;'
                + 'background:rgba(15,23,42,0.86);color:#e2e8f0;border-radius:10px;'
                + 'box-shadow:0 4px 16px rgba(0,0,0,0.35);pointer-events:none;z-index:2147483646;'
                + 'max-width:min(50vw,620px);';
            // ①谁在控制（蓝底强调）②管控档位 ③静态注意事项 ④guard_note
            const t = row(c, '%NS%sb_title__', '#60a5fa');
            t.style.cssText += 'font-weight:700;';
            const m = row(c, '%NS%sb_mode__', '#e2e8f0');
            const n = row(c, '%NS%sb_notice__', '#94a3b8');
            const o = row(c, '%NS%sb_note__', '#cbd5e1');
            mount(c);
        }
        if (window.top === window && !el(PROGRESS_ID)) {
            const c = document.createElement('div');
            c.id = PROGRESS_ID;
            c.setAttribute('data-%NS%agent', '1');
            c.style.cssText = 'position:fixed;left:18px;bottom:18px;margin:0;padding:8px 14px;'
                + 'background:rgba(15,23,42,0.86);color:#e2e8f0;border-radius:10px;'
                + 'box-shadow:0 4px 16px rgba(0,0,0,0.35);pointer-events:none;z-index:2147483646;'
                + 'max-width:min(46vw,560px);';
            // ①当前动作 ②迭代/耗时/进度条 ③任务摘要 ④事件时间线
            const a = row(c, '%NS%pb_action__', '#a7f3d0');
            a.style.cssText += 'font-weight:700;';
            const p = row(c, '%NS%pb_progress__', '#94a3b8');
            const bar = document.createElement('div');
            bar.setAttribute('data-%NS%agent', '1');
            bar.style.cssText = 'height:3px;border-radius:2px;background:rgba(148,163,184,0.35);margin:4px 0 3px;overflow:hidden;';
            const fill = document.createElement('div');
            fill.setAttribute('data-%NS%agent', '1');
            fill.className = '%NS%pb_fill__';
            fill.style.cssText = 'height:100%;width:0%;background:#60a5fa;transition:width .4s ease;';
            bar.appendChild(fill);
            const k = row(c, '%NS%pb_task__', '#cbd5e1');
            const e = row(c, '%NS%pb_events__', '#94a3b8');
            c.appendChild(a); c.appendChild(p); c.appendChild(bar); c.appendChild(k); c.appendChild(e);
            mount(c);
        }
        if (last) render();
    };
    const mount = (node) => (SH || root()).appendChild(node);
    const findIn = (id, cls) => {
        const box = el(id);
        if (!box) return null;
        return box.querySelector('.' + cls) || null;
    };
    const hhmmss = (ms) => {
        const d = new Date(ms);
        const p2 = (n) => (n < 10 ? '0' + n : '' + n);
        return p2(d.getHours()) + ':' + p2(d.getMinutes()) + ':' + p2(d.getSeconds());
    };
    const dur = (ms) => {
        if (!(ms > 0)) return '';
        const s = Math.floor(ms / 1000);
        if (s < 60) return s + '秒';
        const m = Math.floor(s / 60);
        if (m < 60) return m + '分' + (s % 60) + '秒';
        return Math.floor(m / 60) + '时' + (m % 60) + '分';
    };
    const render = () => {
        const d = last || {};
        // ---- 屏蔽页面状态栏(右上角) ----
        const t = findIn(STATUS_ID, '%NS%sb_title__');
        const m = findIn(STATUS_ID, '%NS%sb_mode__');
        const n = findIn(STATUS_ID, '%NS%sb_notice__');
        const o = findIn(STATUS_ID, '%NS%sb_note__');
        if (t) {
            t.textContent = '🔵 LAEW Agent 控制中 · ' + (d.agent || 'LAEW Agent');
            t.title = d.agent || '';
        }
        if (m) { m.textContent = d.mode_text || ''; m.style.display = m.textContent ? '' : 'none'; }
        if (n) { n.textContent = d.notice || ''; n.style.display = n.textContent ? '' : 'none'; }
        if (o) { o.textContent = d.note || ''; o.style.display = o.textContent ? '' : 'none'; }
        // ---- 屏蔽页面进度栏(左下角) ----
        const a = findIn(PROGRESS_ID, '%NS%pb_action__');
        const p = findIn(PROGRESS_ID, '%NS%pb_progress__');
        const f = findIn(PROGRESS_ID, '%NS%pb_fill__');
        const k = findIn(PROGRESS_ID, '%NS%pb_task__');
        const e = findIn(PROGRESS_ID, '%NS%pb_events__');
        if (a) {
            // 动作标签(状态推进)+ 工具标签(Agent 循环埋点)都在时一并显示,
            // 任一为空不产生多余分隔符。
            const parts = [d.phase || '', d.action || '', d.tool || ''].filter(Boolean);
            // 全空(尚未有 Agent 活动/动作,如刚 open 完):给一行占位提示,
            // 免得进度栏塌成空盒子让人工以为页面没被接管。
            a.textContent = parts.length ? ('▶ ' + parts.join(' · ')) : '待命中 · 等待下一次浏览器动作';
            a.title = a.textContent;
        }
        if (p) {
            const it = d.max_iter ? ('迭代 ' + (d.iter || 0) + '/' + d.max_iter) : '';
            const se = d.session ? ('会话 ' + String(d.session).slice(0, 6)) : '';
            const el2 = d.started_ms ? ('已用 ' + dur(Date.now() - Number(d.started_ms))) : '';
            p.textContent = [it, se, el2].filter(Boolean).join(' · ')
                || (d.agent ? '浏览器已接管 · ' + d.agent : '');
            p.title = p.textContent;
        }
        if (f) {
            const pct = d.max_iter ? Math.min(100, Math.round(((d.iter || 0) / d.max_iter) * 100)) : 0;
            f.style.width = pct + '%';
        }
        if (k) {
            k.textContent = d.task ? ('任务：' + d.task) : '';
            k.style.display = k.textContent ? '' : 'none';
            k.title = d.task || '';
        }
        if (e) {
            e.textContent = events.map((x) => hhmmss(x.t) + ' ' + (x.ok ? '✓' : '✗') + ' ' + x.label
                + (x.ms ? (' ' + dur(x.ms)) : '')).join(' · ');
            e.style.display = e.textContent ? '' : 'none';
            e.title = e.textContent;
        }
        const on = visible && !suspended;
        const sb = el(STATUS_ID), pb = el(PROGRESS_ID);
        if (sb) sb.style.display = on ? '' : 'none';
        if (pb) pb.style.display = on ? '' : 'none';
    };
    // ---- 页面侧 API(全部 ns 随机化 + 不可枚举化,与 guard/highlight 同款) ----
    window.%NS%StatusUpdate = (payload) => {
        if (!payload) return false;
        last = Object.assign({}, last || {}, payload);
        if (payload.event) {
            events.push(payload.event);
            while (events.length > MAX_EVENTS) events.shift();
        }
        ensure();
        render();
        return true;
    };
    window.%NS%StatusGuard = (modeText, notice, note) => {
        last = Object.assign({}, last || {}, { mode_text: modeText || '', notice: notice || '', note: note || '' });
        ensure();
        render();
        return true;
    };
    window.%NS%StatusVisible = (v) => { visible = !!v; render(); return visible; };
    window.%NS%StatusSuspend = (v) => { suspended = !!v; render(); return suspended; };
    // DOM 自愈入口(SPA 整树替换把 host 冲掉时重建两条栏并重绘当前内容)
    window.%NS%StatusEnsure = () => { ensure(); render(); return true; };
    // 影子树自探(非枚举,与 `%NS%GuardUiProbe` 同款):closed shadow 对页面/evaluate
    // 都不可读,真浏览器回归测试经此读两条栏的在场性与显示态。
    window.%NS%StatusProbe = () => ({
        host: !!document.getElementById(HOST_ID),
        status: !!el(STATUS_ID),
        progress: !!el(PROGRESS_ID),
        status_display: el(STATUS_ID) ? (el(STATUS_ID).style.display || '') : 'missing',
        progress_display: el(PROGRESS_ID) ? (el(PROGRESS_ID).style.display || '') : 'missing',
        agent: (last && last.agent) || '',
        action: (last && last.action) || '',
        events: events.length,
        // 几何:两条栏盒子与各行的包围盒(排版回归锁 —— 曾出现行高塌成一条)
        geom: [STATUS_ID, PROGRESS_ID].map((id) => {
            const b = el(id);
            if (!b) return null;
            const r = b.getBoundingClientRect();
            return {
                id: id.slice(-18),
                x: Math.round(r.left), y: Math.round(r.top),
                w: Math.round(r.width), h: Math.round(r.height),
                rows: Array.prototype.map.call(b.children, (c) => ({
                    tag: c.tagName,
                    h: Math.round(c.getBoundingClientRect().height),
                    w: Math.round(c.getBoundingClientRect().width),
                    text: (c.textContent || '').slice(0, 24),
                })),
            };
        }),
    });
    // 动作执行期耗时自刷新(0.5s),避免 Rust 侧为「秒数跳动」反复推送
    try {
        if (!window.%NS%StatusTimer) {
            window.%NS%StatusTimer = setInterval(() => { if (last) render(); }, 500);
        }
    } catch (e) {}
    try {
        ['%NS%StatusUpdate', '%NS%StatusGuard', '%NS%StatusVisible', '%NS%StatusSuspend', '%NS%StatusEnsure', '%NS%StatusProbe', '%NS%StatusTimer', '%NS%StatusRev']
            .forEach((k) => {
                if (window[k] !== undefined) {
                    Object.defineProperty(window, k,
                        { enumerable: false, configurable: true, writable: true, value: window[k] });
                }
            });
    } catch (e) {}
    // 就绪重试:document-start 时 documentElement/body 可能皆 null(同 guard 门)
    const boot = () => { try { ensure(); render(); } catch (e) {} };
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', boot, { once: true });
        window.addEventListener('load', boot, { once: true });
    }
    boot();
})()"#;

/// 把动作参数渲染成人读标签（进度栏第一行），如
/// `open https://a.example/x` / `control click #buy` / `inspect elements` /
/// `sequence 5 步`。
pub fn describe_web_action(args: &Value) -> String {
    let s = |k: &str| args.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
    let action = s("action").unwrap_or("?");
    let mut out = action.to_string();
    match action {
        "open" => {
            if let Some(u) = s("url") {
                out.push(' ');
                out.push_str(u);
            }
            if args.get("new_window").and_then(Value::as_bool).unwrap_or(false) {
                out.push_str(" [新窗口]");
            }
        }
        "control" => {
            if let Some(ca) = s("control_action") {
                out.push(' ');
                out.push_str(ca);
            }
            for key in ["selector", "url", "key", "text"] {
                if let Some(v) = args.get("params").and_then(|p| p.get(key)).and_then(Value::as_str) {
                    let v = crate::agent::web_activity::clip_chars(v, 40);
                    out.push(' ');
                    out.push_str(key);
                    out.push('=');
                    out.push_str(&v);
                    break;
                }
            }
        }
        "inspect" => {
            if let Some(i) = s("info") {
                out.push(' ');
                out.push_str(i);
            }
        }
        "sequence" | "batch" => {
            let n = args.get("steps").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
            out.push_str(&format!(" {n} 步"));
        }
        "explore" => {
            let n = args.get("queries").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
            out.push_str(&format!(" {n} 个维度"));
        }
        _ => {}
    }
    crate::agent::web_activity::clip_chars(&out, 90)
}

/// 组装两条栏的全量载荷（状态栏 + 进度栏一次推完，页面侧幂等渲染）。
pub fn build_payload(cfg: Option<&PageGuardConfig>, action: Option<&str>, event: Option<Value>) -> Value {
    let act = crate::agent::web_activity::current_snapshot();
    let (agent, session, iter, max_iter, task, phase, started_ms, tool) = match act {
        Some(a) => (
            a.agent,
            a.session,
            a.iter,
            a.max_iter,
            a.task,
            if a.phase.is_empty() { String::new() } else { a.phase },
            a.started_ms,
            a.tool,
        ),
        None => (
            "LAEW Agent".to_string(),
            String::new(),
            0,
            0,
            String::new(),
            String::new(),
            0,
            String::new(),
        ),
    };
    // `cfg = None` 的推送(纯进度刷新)**不携带档位字段**:页面侧 `Object.assign`
    // 是浅合并,带空串会把上一次 `apply_page_guard` 推来的档位/注意事项覆盖成空
    // (实测状态栏只剩一行标题,档位行与注意事项行消失)。
    let mut payload = json!({
        "agent": agent,
        "session": session,
        "iter": iter,
        "max_iter": max_iter,
        "task": task,
        "phase": phase,
        "started_ms": started_ms,
        "tool": tool,
        "action": action.unwrap_or(""),
        "event": event,
    });
    if let Some(c) = cfg {
        let (m, n) = guard_text(
            c.mode.as_str(),
            crate::agent::browser_overlay::locked_scroll_watch_enabled(),
            false,
        );
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("mode".into(), json!(c.mode.as_str()));
            obj.insert("mode_text".into(), json!(m));
            obj.insert("notice".into(), json!(n));
            obj.insert("note".into(), json!(c.note.clone().unwrap_or_default()));
        }
    }
    payload
}

/// 向页面注入两条栏脚本（新文档自动重挂 + 当前文档立即执行）。
pub async fn ensure(page: &Page) {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
    let _ = page
        .execute(AddScriptToEvaluateOnNewDocumentParams {
            source: render_ns(AGENT_STATUS_JS_TEMPLATE),
            world_name: None,
            include_command_line_api: None,
            run_immediately: Some(true),
        })
        .await;
}

/// 推一次全量刷新（导航后新文档 / 档位变更 / 动作前后共用）。
pub async fn push(page: &Page, cfg: Option<&PageGuardConfig>, action: Option<&str>, event: Option<Value>) {
    let payload = build_payload(cfg, action, event);
    let js = format!(
        "window.{} ? window.{}({}) : false",
        ns_global("StatusUpdate"),
        ns_global("StatusUpdate"),
        payload
    );
    let _ = page.evaluate(js.as_str()).await;
}

/// 显隐两条栏（`set_highlight(false)` 之外的显式开关通道，fail-open）。
pub async fn set_visible(page: &Page, visible: bool) {
    let js = format!(
        "window.{} ? window.{}({}) : false",
        ns_global("StatusVisible"),
        ns_global("StatusVisible"),
        if visible { "true" } else { "false" }
    );
    let _ = page.evaluate(js.as_str()).await;
}

/// 挂起/恢复两条栏（截图/OCR/HITL 附图「拍前隐藏 → 拍完恢复」，证据链干净）。
pub async fn set_suspended(page: &Page, suspend: bool) {
    let js = format!(
        "window.{} ? window.{}({}) : false",
        ns_global("StatusSuspend"),
        ns_global("StatusSuspend"),
        if suspend { "true" } else { "false" }
    );
    let _ = page.evaluate(js.as_str()).await;
}

// ===================== 挑战层驱动避让（docs/MCP_Web_Use/25） =====================

/// 本机制挂起记账：`page_id → 由 blocker_probe 探测驱动挂起中`。
///
/// 为什么单独记账：挂起/复显的触发源有多个（HITL 解锁、截图取证、
/// `set_highlight(false)`、挑战探测），互不隶属。探测驱动这条通道必须只恢复
/// **自己挂起**的页 —— 否则挑战转干净时的复显会把 HITL 提问期间人工正在看的
/// 页面栏提前唤回（或反过来）。幂等：重复挂起/重复清除均无副作用。
static CHALLENGE_SUSPENDED: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);

/// 该页当前是否由挑战探测机制挂起中。
pub fn challenge_suspended(page_id: &str) -> bool {
    match CHALLENGE_SUSPENDED.lock() {
        Ok(mut g) => {
            let set = g.get_or_insert_with(std::collections::HashSet::new);
            set.contains(page_id)
        }
        Err(_) => false,
    }
}

/// 挑战探测驱动的挂起/复显（`blocker_probe::sync_challenge_suspend` 调用）：
/// - `suspend=true`：记账并挂起两条栏（挑战弹窗在场，任何触发源都不该把
///   Agent 视觉元素叠在挑战层上）。
/// - `suspend=false`：仅当本机制挂起中才复显并销账；不是本机制挂起的页
///   （如 HITL 挂起）零动作，不抢其它通道的主导权。
pub async fn challenge_suspend(page: &Page, page_id: &str, suspend: bool) {
    {
        let Ok(mut g) = CHALLENGE_SUSPENDED.lock() else {
            return;
        };
        let set = g.get_or_insert_with(std::collections::HashSet::new);
        if suspend {
            set.insert(page_id.to_string());
        } else if !set.remove(page_id) {
            return; // 非本机制挂起：不复显
        }
    }
    set_suspended(page, suspend).await;
}

/// 测试辅助：清空记账（进程内单测互斥）。
#[cfg(test)]
pub(crate) fn reset_challenge_suspended_for_test() {
    if let Ok(mut g) = CHALLENGE_SUSPENDED.lock() {
        *g = None;
    }
}

/// 向全部存活页面广播「动作开始」（无浏览器实例时零开销返回）。
pub async fn broadcast_start(action: &str) {
    broadcast(action, None).await;
}

/// 向全部存活页面广播「动作结束」（附事件条目，进度栏时间线滚动）。
pub async fn broadcast_done(action: &str, ok: bool, elapsed_ms: u64) {
    let event = json!({
        "t": crate::agent::web_activity::now_ms(),
        "label": crate::agent::web_activity::clip_chars(action, 60),
        "ok": ok,
        "ms": elapsed_ms,
    });
    broadcast(action, Some(event)).await;
}

/// 广播实现：遍历存活页面逐个推（浏览器是进程级单实例，页面即全部现场）。
async fn broadcast(action: &str, event: Option<Value>) {
    let mgr = crate::agent::browser::BrowserManager::global();
    if !mgr.has_browser().await {
        return;
    }
    let cfg = mgr.current_guard().await;
    for (page_id, ..) in mgr.list_pages().await {
        if let Some(page) = mgr.page(&page_id).await {
            push(&page, Some(&cfg), Some(action), event.clone()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 两条栏元素id含命名空间且互不相同() {
        assert!(status_host_id().ends_with("status_ui__"));
        assert!(status_bar_id().ends_with("guard_status__"));
        assert!(progress_bar_id().ends_with("guard_progress__"));
        assert_ne!(status_bar_id(), progress_bar_id());
        assert!(!status_bar_id().contains("laew"), "不得含固定签名");
    }

    #[test]
    fn 注入脚本含两条栏与页面侧api() {
        let js = render_ns(AGENT_STATUS_JS_TEMPLATE);
        assert!(js.contains(&status_bar_id()), "应含状态栏 id");
        assert!(js.contains(&progress_bar_id()), "应含进度栏 id");
        assert!(js.contains("StatusUpdate"));
        assert!(js.contains("StatusGuard"));
        assert!(js.contains("StatusSuspend"));
        assert!(js.contains("LAEW Agent 控制中"));
        assert!(!js.contains("%NS%"), "占位符应全部渲染");
        assert!(js.contains("attachShadow"), "两条栏必须挂 closed shadow");
        assert!(js.contains("mode: 'closed'"));
        assert!(js.contains("pointer-events:none"), "不得挡人工点击");
        assert!(
            js.contains(&format!("const MAX_EVENTS = {EVENT_WINDOW};")),
            "事件时间线窗口应与 Rust 侧常量一致"
        );
        assert!(js.contains("text-overflow:ellipsis"), "长文本应截断显示");
    }

    #[test]
    fn 三档档位与静态注意事项文案齐全() {
        let (m, n) = guard_text("locked", true, false);
        assert_eq!(m, "🔒 页面已锁定(仅观看·可滚动)");
        assert!(n.contains("已拦截"));
        let (m2, n2) = guard_text("locked", false, false);
        assert_eq!(m2, "🔒 页面已锁定(仅观看)");
        assert!(n2.contains("滚动"));
        let (m3, _) = guard_text("open", true, false);
        assert!(m3.contains("页面开放"));
        let (m4, _) = guard_text("partial", true, false);
        assert!(m4.contains("部分锁定"));
        let (m5, n5) = guard_text("locked", true, true);
        assert!(m5.contains("人工介入中"));
        assert!(n5.contains("交还控制权"));
    }

    #[test]
    fn 动作标签覆盖八种action() {
        let cases = [
            (json!({"action":"open","url":"https://a.example/x"}), "open https://a.example/x"),
            (json!({"action":"list"}), "list"),
            (json!({"action":"control","control_action":"click","params":{"selector":"#buy"}}), "control click selector=#buy"),
            (json!({"action":"inspect","info":"elements"}), "inspect elements"),
            (json!({"action":"sequence","steps":[{"a":1},{"a":2}]}), "sequence 2 步"),
            (json!({"action":"batch","steps":[{"a":1}]}), "batch 1 步"),
            (json!({"action":"explore","queries":[{"info":"elements"}]}), "explore 1 个维度"),
            (json!({"action":"close","page_id":"all"}), "close"),
        ];
        for (args, want) in cases {
            assert_eq!(describe_web_action(&args), want);
        }
    }

    #[test]
    fn 动作标签超长截断() {
        let args = json!({"action":"open","url":format!("https://a.example/{}", "x".repeat(300))});
        let out = describe_web_action(&args);
        assert!(out.chars().count() <= 90, "应截断:{}", out.chars().count());
        assert!(out.ends_with('…'));
    }

    #[test]
    fn 纯进度推送不带档位字段不覆盖状态栏() {
        let cfg = PageGuardConfig::locked();
        let full = build_payload(Some(&cfg), None, None);
        assert!(full["mode_text"].as_str().unwrap().contains("页面已锁定"));
        let bare = build_payload(None, Some("inspect elements"), None);
        assert!(bare.get("mode_text").is_none(), "cfg=None 不得下发档位字段(否则浅合并覆盖空)");
        assert!(bare.get("notice").is_none());
        assert!(bare.get("note").is_none());
        assert_eq!(bare["action"], "inspect elements");
    }

    #[test]
    fn 注入脚本版本门单实例且带dom自愈入口() {
        let js = render_ns(AGENT_STATUS_JS_TEMPLATE);
        assert!(js.contains("const REV = 2;"));
        assert!(js.contains("StatusRev >= REV"), "重注入必须走版本门(否则闭包状态被重置)");
        assert!(js.contains("StatusEnsure"), "应暴露 DOM 自愈入口");
    }

    #[test]
    fn 两条栏都只在顶层frame渲染() {
        // 2026-10-14 天猫 baxia 实测:注入脚本会进每个新文档(含跨域挑战 iframe),
        // 状态栏在小 iframe 里罩死人工核验控件 —— 两条栏都必须有顶层门。
        let js = render_ns(AGENT_STATUS_JS_TEMPLATE);
        assert!(
            js.matches("window.top === window").count() >= 2,
            "状态栏与进度栏都需要 window.top === window 顶层门"
        );
        // 状态栏创建点必须被顶层门包裹(进度栏本就有;逐条核对创建语句)。
        assert!(
            js.contains("if (window.top === window && !el(STATUS_ID))"),
            "状态栏创建必须带顶层门"
        );
        assert!(
            js.contains("if (window.top === window && !el(PROGRESS_ID))"),
            "进度栏创建必须带顶层门"
        );
    }

    #[test]
    fn 载荷含状态栏与进度栏全部字段() {
        let cfg = PageGuardConfig::locked();
        let p = build_payload(Some(&cfg), Some("open https://a"), None);
        assert_eq!(p["mode"], "locked");
        assert!(p["mode_text"].as_str().unwrap().contains("页面已锁定"));
        assert!(!p["notice"].as_str().unwrap().is_empty(), "静态注意事项应下发");
        assert_eq!(p["action"], "open https://a");
        assert_eq!(p["tool"], "", "无活动台账时工具标签为空");
        // 无活动台账时回退品牌名,不报错
        assert_eq!(p["agent"], "LAEW Agent");
        let e = build_payload(None, None, Some(json!({"t":1,"label":"x","ok":true,"ms":5})));
        assert!(e["event"].is_object());
    }

    #[tokio::test]
    async fn 台账作用域内载荷带具体Agent名与迭代() {
        let a = crate::agent::web_activity::begin(
            "LsmAgentEmergentWork-SubAgent-Work",
            "sess-abcdef",
            "调研双站价格",
            12,
        );
        a.set_iter(3, "第 3/12 轮 · 执行工具");
        a.set_tool("MCP_Web_Use control click #buy");
        let p = crate::agent::web_activity::scope(a, async {
            build_payload(None, Some("control click #buy"), None)
        })
        .await;
        assert_eq!(p["agent"], "LsmAgentEmergentWork-SubAgent-Work");
        assert_eq!(p["session"], "sess-abcdef");
        assert_eq!(p["iter"], 3);
        assert_eq!(p["max_iter"], 12);
        assert_eq!(p["task"], "调研双站价格");
        assert_eq!(p["phase"], "第 3/12 轮 · 执行工具");
        assert_eq!(p["tool"], "MCP_Web_Use control click #buy");
    }
}