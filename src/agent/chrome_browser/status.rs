//! Chrome 侧**屏蔽页面状态栏 / 屏蔽页面进度栏**(双浏览器工具架构)。
//!
//! headed 页面右上角与左下角两条常驻告知栏的注入与推送:
//!
//! | 栏 | 位置 | 回答 | 内容 |
//! | -- | ---- | ---- | ---- |
//! | **屏蔽页面状态栏** | 右上角 | 谁在控制 / 控制成什么样 / 我该怎么配合 | 具体 Agent 名 + 档位行 + 静态注意事项 + note |
//! | **屏蔽页面进度栏** | 左下角 | 正在干什么 / 进度到哪 / 刚发生了什么 | 当前动作 + 迭代进度与耗时 + 任务摘要 + 事件时间线 |
//!
//! 数据源 = `crate::agent::web_activity`(task_local 活动台账);本模块只负责
//! 注入 JS、组装载荷、动作前后广播。Lsm 内核侧的姊妹实现在
//! `src/agent/browser/page_bars.rs`(查看器帧合成,不走页面注入)。
//!
//! 从归档实现(`docs/MCP_Web_Chrome_Use/reference实现归档/src/agent/browser_status.rs`)
//! 裁剪移植,保留全部实测教训:
//! - **closed Shadow DOM**:品牌文本留在 light DOM 会被页面 `document.body.innerText`
//!   采集(实测京东风控据此判自动化);`attachShadow` 不可用 fail-open 回退。
//! - **只在顶层 frame 渲染**:`addScriptToEvaluateOnNewDocument` 注入每个新文档(含跨域
//!   iframe),天猫 baxia 滑块挑战 iframe(~390px)内的状态栏会把挑战控件罩死。
//! - **版本化单实例**:多通道重注入不重置闭包状态(events/visible)。
//! - **独立 fixed 角落 host**:不共用全屏 inset:0 宿主(会被页面 transform 劫持
//!   position:fixed 包含块,Windows 1080p 实测「显示不出来」)。
//! - **pointer-events:none** + 长文本 ellipsis 截断 + 页面侧 500ms 耗时自刷新。
//!
//! 设计详案 `docs/双浏览器工具架构/02-屏蔽页面状态栏与进度栏.md`。

use chromiumoxide::Page;
use serde_json::{json, Value};

use crate::agent::web_activity;
use crate::agent::web_control::ControlState;

/// 事件时间线在页面侧的保留条数(JS `MAX_EVENTS` 同步值;超出滚动丢弃)。
pub const EVENT_WINDOW: usize = 6;

/// `LAEW_PAGE_BARS=off|0|false|no` 关闭两条告知栏(两工具同参,默认开启)。
pub fn bars_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_PAGE_BARS")
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

// ===================== 命名空间随机化 =====================

/// 进程级随机命名空间(与归档 `browser_ns.rs` 同款):两条栏的 host id / 页面侧 API
/// 都拼在随机 ns 后面,页面脚本无法按固定签名枚举「LAEW」痕迹。
pub(crate) fn dom_ns() -> &'static str {
    use std::sync::OnceLock;
    static NS: OnceLock<String> = OnceLock::new();
    NS.get_or_init(|| {
        let mut b = [0u8; 4];
        rand::Rng::fill(&mut rand::thread_rng(), &mut b);
        format!("l{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
    })
}

/// 渲染 JS 模板:把 `%NS%` 占位符替换为随机 ns。
pub fn render_ns(template: &str) -> String {
    template.replace("%NS%", dom_ns())
}

/// 页面侧 API 的安全调用表达式:符号命名空间对象未建立(脚本未注入)时求值为
/// `undefined` 而不是抛 TypeError。形如:
/// `((window[Symbol.for('l…')] || {}).StatusUpdate)`
pub(crate) fn ns_member_expr(api: &str) -> String {
    format!("(window[Symbol.for('{}')] || {{}}).{}", dom_ns(), api)
}

/// 影子树自探表达式(真浏览器回归/冒烟用):求值两条栏在场性与显示态,
/// 脚本未注入时返回 `null`(不抛错)。
pub fn probe_expr() -> String {
    let api = ns_member_expr("StatusProbe");
    format!("{api} ? {api}() : null")
}

// ===================== 文案(档位行 + 静态注意事项) =====================

/// 状态栏第②行(档位)与第③行(静态注意事项)文案。
///
/// 档位**如实反映控制权状态机**(`crate::agent::web_control`):
/// - `agent` Agent 控制状态:蒙层在位 + 输入闸拦截,人工点不动页面;
/// - `user` 用户控制状态:蒙层撤除,控制权交还人工,Agent 写操作被机械拒绝。
///
/// 不虚标档位:控制权状态由工具层如实记账,状态栏只是它的投影。
pub fn chrome_mode_text(state: ControlState) -> (&'static str, &'static str) {
    match state {
        ControlState::Agent => (
            "🔒 Agent 控制中 · 人工请勿操作页面",
            "Chrome 由 LAEW Agent 驱动;页面处于 Agent 控制状态,人工点击已被拦截;如需人工介入请用 request_human",
        ),
        ControlState::User => (
            "🔓 用户控制中 · 人工可直接操作",
            "Chrome 由 LAEW Agent 驱动;控制权已交还人工,Agent 的页面写操作会被拒绝;人工完成后请用 set_control_state 取回控制权",
        ),
    }
}

// ===================== JS 模板 =====================

/// 注入脚本:两条栏 + 页面侧 API。
///
/// 占位符:`%NS%`(命名空间);其余全部由页面侧状态驱动,Rust 侧只推 JSON ——
/// 不在模板里烘焙任何动态文案,保证「档位/动作/Agent」只有一个真源。
pub const AGENT_STATUS_JS_TEMPLATE: &str = r#"(() => {
    // 命名空间收敛到一个 symbol 键对象(零 window 字符串键,防枚举)。
    const NS = window[Symbol.for('%NS%')] || (window[Symbol.for('%NS%')] = {});
    // 版本化单实例:本脚本会被多次注入(open 注入 + 新文档重挂),而事件时间线/
    // 耗时起点/显隐标志都是闭包状态,后注入者不得重置。同版本仅补一次 DOM 自愈。
    // REV 2:顶层 frame 门 —— 注入会进每个新文档(含跨域挑战 iframe),子 frame
    // 一律不渲染任何一条栏(baxia 滑块实测教训)。
    const REV = 2;
    if (NS.StatusRev >= REV) {
        try { if (NS.StatusEnsure) NS.StatusEnsure(); } catch (e) {}
        return;
    }
    NS.StatusRev = REV;
    const STATUS_ID = '%NS%guard_status__';
    const PROGRESS_ID = '%NS%guard_progress__';
    // 两条栏各自独立 position:fixed 角落锚定 host:共用全屏 inset:0 宿主会被页面
    // html/body 的 transform 劫持 position:fixed 包含块(Windows 1080p 实测显示不出来)。
    const STATUS_HOST_ID = '%NS%status_host__';
    const PROGRESS_HOST_ID = '%NS%progress_host__';
    const FONT = 'system-ui,-apple-system,"PingFang SC","Microsoft YaHei",sans-serif';
    const MAX_EVENTS = 6;
    let SHS = null;   // 状态栏 host 的 closed shadow
    let SHP = null;   // 进度栏 host 的 closed shadow
    let events = [];
    let last = null;
    let visible = true;
    let suspended = false;
    // 挂起来源记账:截图避让若中途被打断,suspended 会永久留在页面侧导致两条栏
    // 隐身;超时自愈(见 onTick)。
    let suspend_since = 0;
    const root = () => document.documentElement || document.body;
    const el = (id) => {
        if (SHS && SHS.getElementById) { const e = SHS.getElementById(id); if (e) return e; }
        if (SHP && SHP.getElementById) { const e = SHP.getElementById(id); if (e) return e; }
        return document.getElementById(id);
    };
    const STYLE_CSS = [
        ':host{all:initial;}',
        '.bar{box-sizing:border-box;margin:0;pointer-events:none;',
        'background:rgba(15,23,42,0.88);color:#e2e8f0;border-radius:10px;',
        'box-shadow:0 4px 16px rgba(0,0,0,0.35);z-index:2147483646;padding:8px 14px;}',
        '.row{font:400 12px/1.6 ' + FONT + ';color:inherit;white-space:nowrap;',
        'overflow:hidden;text-overflow:ellipsis;max-width:min(46vw,560px);margin:0;}',
        '.row.title{font:700 13px/1.6 ' + FONT + ';}',
        '@media (max-width:1280px){.bar{padding:6px 10px;border-radius:8px;}',
        '.row{font-size:11px;max-width:min(52vw,420px);}.row.title{font-size:12px;}}',
        '@media (max-width:820px){.bar{padding:5px 8px;}.row{font-size:10px;',
        'max-width:min(58vw,320px);}.row.title{font-size:11px;}}',
    ].join('');
    const row = (parent, cls, color) => {
        const d = document.createElement('div');
        d.setAttribute('data-%NS%agent', '1');
        d.className = 'row ' + cls;
        d.style.color = color;
        parent.appendChild(d);
        return d;
    };
    const makeHost = (hostId, cssText) => {
        let h = document.getElementById(hostId);
        if (h && h.__%NS%sh) return h;
        if (!h) {
            h = document.createElement('div');
            h.id = hostId;
            h.setAttribute('data-%NS%agent', '1');
            h.style.cssText = cssText;
            const r = root();
            if (!r) return null;
            r.appendChild(h);
        }
        if (!h.__%NS%sh) {
            try { h.__%NS%sh = h.attachShadow ? h.attachShadow({ mode: 'closed' }) : null; } catch (e) { h.__%NS%sh = null; }
        }
        return h;
    };
    const ensure = () => {
        if (!root()) return;
        // 顶层门:子 frame(含跨域挑战 iframe)不渲染任何一条栏。
        if (window.top !== window) return;
        const stHost = makeHost(STATUS_HOST_ID,
            'position:fixed;top:12px;right:12px;margin:0;padding:0;pointer-events:none;'
            + 'z-index:2147483646;max-width:min(50vw,620px);');
        const pgHost = makeHost(PROGRESS_HOST_ID,
            'position:fixed;left:18px;bottom:18px;margin:0;padding:0;pointer-events:none;'
            + 'z-index:2147483646;max-width:min(46vw,560px);');
        SHS = (stHost && stHost.__%NS%sh) || SHS;
        SHP = (pgHost && pgHost.__%NS%sh) || SHP;
        const mountStyle = (sh) => {
            if (!sh) return;
            if (!sh.querySelector('style')) {
                const st = document.createElement('style');
                st.setAttribute('data-%NS%agent', '1');
                st.textContent = STYLE_CSS;
                sh.appendChild(st);
            }
        };
        mountStyle(SHS);
        mountStyle(SHP);
        // ---- 屏蔽页面状态栏(右上角):①谁在控制 ②档位 ③静态注意事项 ④note ----
        if (!el(STATUS_ID)) {
            const c = document.createElement('div');
            c.id = STATUS_ID;
            c.setAttribute('data-%NS%agent', '1');
            c.className = 'bar';
            row(c, '%NS%sb_title__', '#60a5fa');
            row(c, '%NS%sb_mode__', '#e2e8f0');
            row(c, '%NS%sb_notice__', '#94a3b8');
            row(c, '%NS%sb_note__', '#cbd5e1');
            (SHS || root()).appendChild(c);
        }
        // ---- 屏蔽页面进度栏(左下角):①动作 ②迭代/耗时 ③任务 ④事件时间线 ----
        if (!el(PROGRESS_ID)) {
            const c = document.createElement('div');
            c.id = PROGRESS_ID;
            c.setAttribute('data-%NS%agent', '1');
            c.className = 'bar';
            row(c, '%NS%pb_action__', '#a7f3d0');
            row(c, '%NS%pb_progress__', '#94a3b8');
            const bar = document.createElement('div');
            bar.setAttribute('data-%NS%agent', '1');
            bar.style.cssText = 'height:3px;border-radius:2px;background:rgba(148,163,184,0.35);margin:4px 0 3px;overflow:hidden;';
            const fill = document.createElement('div');
            fill.setAttribute('data-%NS%agent', '1');
            fill.className = '%NS%pb_fill__';
            fill.style.cssText = 'height:100%;width:0%;background:#60a5fa;transition:width .4s ease;';
            bar.appendChild(fill);
            c.appendChild(bar);
            row(c, '%NS%pb_task__', '#cbd5e1');
            row(c, '%NS%pb_events__', '#94a3b8');
            (SHP || root()).appendChild(c);
        }
        if (last) render();
    };
    const findIn = (id, cls) => {
        const box = el(id);
        return box ? (box.querySelector('.' + cls) || null) : null;
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
            const parts = [d.phase || '', d.action || '', d.tool || ''].filter(Boolean);
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
            k.textContent = d.task ? ('任务:' + d.task) : '';
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
        const sbHost = document.getElementById(STATUS_HOST_ID);
        const pgHost = document.getElementById(PROGRESS_HOST_ID);
        if (sbHost) sbHost.style.display = on ? '' : 'none';
        if (pgHost) pgHost.style.display = on ? '' : 'none';
    };
    // ---- 页面侧 API(挂 NS symbol 对象,成员一律不可枚举) ----
    NS.StatusUpdate = (payload) => {
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
    NS.StatusVisible = (v) => { visible = !!v; render(); return visible; };
    NS.StatusSuspend = (v) => {
        suspended = !!v;
        suspend_since = suspended ? Date.now() : 0;
        render();
        return suspended;
    };
    // DOM 自愈入口(SPA 整树替换把 host 冲掉时重建两条栏并重绘当前内容)。
    NS.StatusEnsure = () => { ensure(); render(); return true; };
    // 影子树自探:closed shadow 对页面/evaluate 都不可读,回归测试经此读在场性。
    NS.StatusProbe = () => ({
        host: !!document.getElementById(STATUS_HOST_ID),
        host_progress: !!document.getElementById(PROGRESS_HOST_ID),
        status: !!el(STATUS_ID),
        progress: !!el(PROGRESS_ID),
        agent: (last && last.agent) || '',
        action: (last && last.action) || '',
        events: events.length,
        visible: visible,
        suspended: suspended,
        top_frame: window.top === window,
    });
    // 耗时行自刷新(0.5s)+ DOM 自愈 + 挂起超时自愈(截图避让被打断时 3 分钟自动复显)。
    try {
        if (!NS.StatusTimer) {
            NS.StatusTimer = setInterval(() => {
                try { ensure(); } catch (e) {}
                if (suspended && suspend_since && (Date.now() - suspend_since) > 180000) {
                    suspended = false;
                    suspend_since = 0;
                }
                render();
            }, 500);
        }
    } catch (e) {}
    try {
        if (!NS.StatusMO && document.documentElement) {
            NS.StatusMO = new MutationObserver(() => {
                try {
                    if (!document.getElementById(STATUS_HOST_ID) || !document.getElementById(PROGRESS_HOST_ID)) {
                        ensure();
                        render();
                    }
                } catch (e) {}
            });
            NS.StatusMO.observe(document.documentElement, { childList: true, subtree: true });
        }
    } catch (e) {}
    try {
        ['StatusUpdate', 'StatusVisible', 'StatusSuspend', 'StatusEnsure', 'StatusProbe', 'StatusTimer', 'StatusMO', 'StatusRev']
            .forEach((k) => {
                const d = Object.getOwnPropertyDescriptor(NS, k);
                if (!d) return;
                Object.defineProperty(NS, k,
                    { enumerable: false, configurable: true, writable: true, value: d.value });
            });
    } catch (e) {}
    const boot = () => { try { ensure(); render(); } catch (e) {} };
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', boot, { once: true });
        window.addEventListener('load', boot, { once: true });
    }
    boot();
})()"#;

// ===================== 动作标签与载荷 =====================

/// 把动作参数渲染成人读标签(进度栏第一行),如 `open https://a.example/x` /
/// `control click selector=#buy` / `inspect elements`。
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
        }
        "control" => {
            if let Some(ca) = s("control_action") {
                out.push(' ');
                out.push_str(ca);
            }
            for key in ["selector", "url", "key", "text", "script"] {
                if let Some(v) = args.get("params").and_then(|p| p.get(key)).and_then(Value::as_str) {
                    let v = web_activity::clip_chars(v, 40);
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
        "close" => {
            if let Some(p) = s("page_id") {
                out.push(' ');
                out.push_str(p);
            }
        }
        _ => {}
    }
    web_activity::clip_chars(&out, 90)
}

/// 组装两条栏的全量载荷(状态栏 + 进度栏一次推完,页面侧浅合并幂等渲染)。
///
/// 档位/注意事项/note 字段**只在 `with_mode=true` 时下发**(浅合并语义:纯进度刷新
/// 不得携带空档位字段把上一次推来的档位覆盖成空)。`note` 为 `open(guard_note=…)`
/// 自定义提示。
pub fn build_payload(
    action: Option<&str>,
    event: Option<Value>,
    note: Option<&str>,
    with_mode: bool,
    control_state: ControlState,
) -> Value {
    let (agent, session, iter, max_iter, task, phase, started_ms, tool) =
        match web_activity::current_snapshot() {
            Some(a) => (
                a.agent, a.session, a.iter, a.max_iter, a.task, a.phase, a.started_ms, a.tool,
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
    if with_mode {
        let (mode_text, notice) = chrome_mode_text(control_state);
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("mode".into(), json!(control_state.as_str()));
            obj.insert("mode_text".into(), json!(mode_text));
            obj.insert("notice".into(), json!(notice));
            obj.insert("note".into(), json!(note.unwrap_or_default()));
        }
    }
    payload
}

// ===================== 注入与推送 =====================

/// 向页面注入两条栏脚本(新文档自动重挂 + 当前文档立即执行)。
/// 仅 headed 且 `LAEW_PAGE_BARS` 未关时调用(hidden 无人工观看,零注入)。
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

/// 推一次全量刷新(open 后首推 / 动作前后共用)。
pub async fn push(
    page: &Page,
    action: Option<&str>,
    event: Option<Value>,
    note: Option<&str>,
    with_mode: bool,
    control_state: ControlState,
) {
    let payload = build_payload(action, event, note, with_mode, control_state);
    let api = ns_member_expr("StatusUpdate");
    let js = format!("{api} ? {api}({payload}) : false");
    let _ = page.evaluate(js.as_str()).await;
}

/// 挂起/恢复两条栏(截图取证「拍前隐藏 → 拍完恢复」,证据图不入镜)。
pub async fn set_suspended(page: &Page, suspend: bool) {
    let api = ns_member_expr("StatusSuspend");
    let js = format!("{api} ? {api}({}) : false", if suspend { "true" } else { "false" });
    let _ = page.evaluate(js.as_str()).await;
}

/// 是否应该注入/推送(headed + 未关)。
pub fn should_inject(headed: bool) -> bool {
    headed && bars_enabled()
}

/// 向全部存活页面广播「动作开始」(无浏览器实例时零开销返回)。
pub async fn broadcast_start(action: &str) {
    broadcast(action, None, false).await;
}

/// 向全部存活页面广播「动作结束」(附事件条目,进度栏时间线滚动)。
pub async fn broadcast_done(action: &str, ok: bool, elapsed_ms: u64) {
    let event = json!({
        "t": web_activity::now_ms(),
        "label": web_activity::clip_chars(action, 60),
        "ok": ok,
        "ms": elapsed_ms,
    });
    broadcast(action, Some(event), false).await;
}

/// 广播实现:遍历存活页面逐个推(浏览器是进程级单实例,页面即全部现场)。
/// `with_mode` 仅首推(open 后)为 true —— 档位字段不随纯进度刷新反复下发。
/// 档位行取**该页自己的**控制权状态(并行单元各控各页,不能用全局值)。
async fn broadcast(action: &str, event: Option<Value>, with_mode: bool) {
    let mgr = super::ChromeManager::global();
    if !should_inject(mgr.mode().is_headed()) {
        return;
    }
    for (page_id, _) in mgr.list_pages().await {
        if let Some(page) = mgr.get_page(&page_id) {
            let state = crate::agent::web_control::state_of(
                crate::agent::web_control::Engine::Chrome,
                &page_id,
            );
            push(&page, Some(action), event.clone(), None, with_mode, state).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 注入脚本含两条栏与页面侧api() {
        let js = render_ns(AGENT_STATUS_JS_TEMPLATE);
        assert!(js.contains("guard_status__"), "应含状态栏 id");
        assert!(js.contains("guard_progress__"), "应含进度栏 id");
        assert!(js.contains("StatusUpdate"));
        assert!(js.contains("StatusSuspend"));
        assert!(js.contains("StatusProbe"));
        assert!(js.contains("LAEW Agent 控制中"), "状态栏标题必须带品牌与具体 Agent 位");
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
    fn 两条栏只在顶层frame渲染且独立fixed锚定() {
        let js = render_ns(AGENT_STATUS_JS_TEMPLATE);
        assert!(
            js.contains("if (window.top !== window) return;"),
            "ensure() 入口必须有顶层早退门(子 frame 不渲染任何一条栏)"
        );
        assert!(js.contains("makeHost(STATUS_HOST_ID"), "状态栏创建须在顶层门之后");
        assert!(js.contains("makeHost(PROGRESS_HOST_ID"), "进度栏创建须在顶层门之后");
        assert!(js.contains("top:12px;right:12px"), "状态栏须右上角视口锚定");
        assert!(js.contains("left:18px;bottom:18px"), "进度栏须左下角视口锚定");
        assert!(!js.contains("position:fixed;inset:0"), "不得用全屏 inset:0 宿主");
    }

    #[test]
    fn 注入脚本版本门单实例且带dom自愈入口() {
        let js = render_ns(AGENT_STATUS_JS_TEMPLATE);
        assert!(js.contains("const REV = 2;"));
        assert!(js.contains("StatusRev >= REV"), "重注入必须走版本门(否则闭包状态被重置)");
        assert!(js.contains("StatusEnsure"), "应暴露 DOM 自愈入口");
        assert!(js.contains("MutationObserver"), "SPA 整树替换需要 MutationObserver 自愈");
    }

    #[test]
    fn 命名空间随机化且api走symbol对象() {
        let js = render_ns(AGENT_STATUS_JS_TEMPLATE);
        let ns = dom_ns();
        assert!(ns.starts_with('l') && ns.len() == 9, "ns 应为 l + 8 位十六进制");
        assert!(js.contains(&format!("Symbol.for('{ns}')")), "API 应挂 symbol 命名空间对象");
        assert!(!js.contains("window.laew"), "不得有固定签名");
        // ns_member_expr 生成的是 window[Symbol.for('…').X] 形式
        assert!(ns_member_expr("StatusUpdate").contains("Symbol.for("));
    }

    #[test]
    fn chrome档位文案如实反映控制权两态() {
        // Agent 控制状态:蒙层在位,人工点不动。
        let (mode, notice) = chrome_mode_text(ControlState::Agent);
        assert!(mode.contains("Agent 控制中"), "Agent 态档位应写明 Agent 控制中");
        assert!(mode.contains("请勿操作"), "Agent 态应提示人工不要操作页面");
        assert!(!notice.is_empty(), "静态注意事项应随档位下发(状态栏第③行)");
        // 用户控制状态:控制权交还人工。
        let (umode, unotice) = chrome_mode_text(ControlState::User);
        assert!(umode.contains("用户控制中"), "User 态档位应写明用户控制中");
        assert!(umode.contains("可直接操作"), "User 态应提示人工可操作");
        assert!(!unotice.is_empty());
    }

    #[test]
    fn 动作标签覆盖各action() {
        let cases = [
            (json!({"action":"open","url":"https://a.example/x"}), "open https://a.example/x"),
            (json!({"action":"list"}), "list"),
            (json!({"action":"control","control_action":"click","params":{"selector":"#buy"}}), "control click selector=#buy"),
            (json!({"action":"control","control_action":"eval_js","params":{"script":"1+1"}}), "control eval_js script=1+1"),
            (json!({"action":"inspect","info":"elements"}), "inspect elements"),
            (json!({"action":"close","page_id":"all"}), "close all"),
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
        let full = build_payload(Some("open https://a"), None, Some("自定义提示"), true, ControlState::Agent);
        assert!(full["mode_text"].as_str().unwrap().contains("Agent 控制中"));
        assert_eq!(full["mode"], "agent", "档位字段应如实反映控制权状态");
        assert!(!full["notice"].as_str().unwrap().is_empty(), "静态注意事项应下发");
        assert_eq!(full["note"], "自定义提示");
        let bare = build_payload(Some("inspect elements"), None, None, false, ControlState::Agent);
        assert!(bare.get("mode_text").is_none(), "with_mode=false 不得下发档位字段(浅合并覆盖空)");
        assert!(bare.get("notice").is_none());
        assert!(bare.get("note").is_none());
        assert_eq!(bare["action"], "inspect elements");
    }

    #[test]
    fn 载荷无台账时回退品牌名() {
        let p = build_payload(None, None, None, false, ControlState::Agent);
        assert_eq!(p["agent"], "LAEW Agent");
        assert_eq!(p["tool"], "", "无活动台账时工具标签为空");
        let e = build_payload(None, Some(json!({"t":1,"label":"x","ok":true,"ms":5})), None, false, ControlState::Agent);
        assert!(e["event"].is_object());
    }

    #[tokio::test]
    async fn 台账作用域内载荷带具体Agent名与迭代() {
        let a = web_activity::begin(
            "LsmAgentEmergentWork-SubAgent-Work",
            "sess-abcdef",
            "调研双站价格",
            12,
        );
        a.set_iter(3, "第 3/12 轮 · 执行工具");
        a.set_tool("MCP_Web_Chrome_Use control click #buy");
        let p = web_activity::scope(a, async {
            build_payload(Some("control click #buy"), None, None, false, ControlState::Agent)
        })
        .await;
        assert_eq!(p["agent"], "LsmAgentEmergentWork-SubAgent-Work");
        assert_eq!(p["session"], "sess-abcdef");
        assert_eq!(p["iter"], 3);
        assert_eq!(p["max_iter"], 12);
        assert_eq!(p["task"], "调研双站价格");
        assert_eq!(p["phase"], "第 3/12 轮 · 执行工具");
        assert_eq!(p["tool"], "MCP_Web_Chrome_Use control click #buy");
    }
}
