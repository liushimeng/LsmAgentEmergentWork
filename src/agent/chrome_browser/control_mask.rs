//! Chrome 侧**控制权蒙层**(双浏览器工具架构 · 控制权状态机)。
//!
//! 与 `status.rs`(两条告知栏)配套,回答的是另一个问题:
//!
//! | 模块 | 回答的问题 | 产物 |
//! | ---- | ---------- | ---- |
//! | [`super::status`] | 「谁在控制 / 正在干什么」 | 右上状态栏 + 左下进度栏(`pointer-events:none`,纯告知) |
//! | **本模块** | **「这块页面现在归谁管」** | **全屏半透明蒙层 + 输入闸** |
//!
//! ## 两层结构
//!
//! - **L1 视觉蒙层(JS)**:全屏半透明遮罩,`pointer-events:none` **不参与 hit-test**
//!   —— 页面脚本枚举不到、Agent 的 JS 点击(`el.click()`)不受影响;纯视觉声明
//!   「现在是 Agent 控制状态」。沿用归档实测教训:不用 `pointer-events:auto`
//!   的整页 DOM 蒙层拦输入(CDP 合成输入与真实输入在 hit-test 层不可区分,
//!   会把 Agent 自己的点击一并吃掉)。
//! - **L2 输入闸(CDP)**:`Input.setIgnoreInputEvents(ignore=true)`,浏览器级丢弃
//!   该 target 的全部真实用户输入(鼠标/滚轮/键盘/触摸),target 级全 frame 一致、
//!   跨导航持久。**实测同时拦截 CDP 合成输入**(`Input.dispatch*`),因此 Agent
//!   自己的输入类动作必须「先解后锁」(见 [`input_actions_need_unlock`] 白名单,
//!   工具层统一包裹,毫秒级窗口)。
//!
//! ## 控制权两态(与 `crate::agent::web_control` 同一真源)
//!
//! | 状态 | 蒙层 | 输入闸 | 人工 |
//! | ---- | ---- | ------ | ---- |
//! | `agent` Agent 控制状态 | **在位** | **拦截** | 页面点不动(浏览器自身 chrome 仍可用) |
//! | `user` 用户控制状态 | 撤除 | 放行 | 可自由点击/输入/滚动 |
//!
//! 杀开关:`LAEW_WEB_MASK=off|0|false|no` 只关蒙层与输入闸(闸门与状态栏文案保留);
//! `LAEW_WEB_CONTROL=off` 连控制权状态机一起关(见 `web_control.rs`)。
//!
//! 设计详案 `docs/双浏览器工具架构/03-浏览器控制权状态机与视口自适应.md`。

use chromiumoxide::Page;
use serde_json::Value;

use crate::agent::web_control::{self, ControlState, Engine};

/// 蒙层是否启用(`LAEW_WEB_MASK` 未关 + 控制权状态机未关)。
pub fn mask_enabled() -> bool {
    web_control::mask_enabled()
}

/// 需要「先解后锁」的 Agent 动作白名单。
///
/// 这些动作会派发 **CDP 合成输入**(`Input.dispatchMouseEvent` / `dispatchKeyEvent`),
/// 被 `Input.setIgnoreInputEvents(true)` 一并拦下,必须临时解锁再动作。
///
/// **不在白名单里的动作**(如默认的 `click` 走 JS `el.click()`、`eval_js` 走脚本)
/// 不经过浏览器输入管线,锁着也能照常执行 —— 不为它们白白开窗。
pub fn input_actions_need_unlock() -> &'static [&'static str] {
    &[
        "click_real",
        "hover",
        "double_click",
        "right_click",
        "key_press",
        "type_real",
        "scroll",
        "drag",
    ]
}

/// 某动作是否需要临时解锁输入闸。
pub fn action_needs_unlock(action: &str) -> bool {
    input_actions_need_unlock().contains(&action.trim())
}

// ===================== JS 模板(视觉蒙层) =====================

/// 注入脚本:全屏半透明蒙层 + 页面侧开关 API。
///
/// 占位符:`%NS%`(进程级随机命名空间,与 `status.rs` 共用同一个 symbol 键对象,
/// 但用独立的 `Mask*` 键,互不干扰)。
///
/// - **顶层 frame 门**:注入会进每个新文档(含跨域 iframe),子 frame 一律不渲染
///   —— 天猫 baxia 滑块 iframe 内的蒙层会把挑战控件罩死(归档实测教训)。
/// - **纯视觉**:`pointer-events:none`,不参与 hit-test;文本一律不放蒙层里
///   (文案归 `status.rs` 的状态栏,归档第 171 轮「告知性 UI 外迁」原则)。
pub const AGENT_MASK_JS_TEMPLATE: &str = r#"(() => {
    const NS = window[Symbol.for('%NS%')] || (window[Symbol.for('%NS%')] = {});
    const REV = 1;
    if (NS.MaskRev >= REV) {
        try { if (NS.MaskEnsure) NS.MaskEnsure(); } catch (e) {}
        return;
    }
    NS.MaskRev = REV;
    const MASK_ID = '%NS%agent_mask__';
    let host = null;
    let active = false;
    let suspended = false;
    const root = () => document.documentElement || document.body;
    const STYLE_CSS = [
        ':host{all:initial;}',
        '.veil{position:absolute;inset:0;pointer-events:none;',
        'background:rgba(15,23,42,0.30);}',
        '.edge{position:absolute;left:0;right:0;height:4px;pointer-events:none;',
        'background:linear-gradient(90deg,rgba(37,99,235,0.85),rgba(14,165,233,0.85));}',
        '.edge.top{top:0;}.edge.bottom{bottom:0;}',
    ].join('');
    const ensure = () => {
        if (!root()) return;
        // 顶层门:子 frame(含跨域挑战 iframe)不渲染蒙层。
        if (window.top !== window) return;
        let h = document.getElementById(MASK_ID);
        if (!h) {
            h = document.createElement('div');
            h.id = MASK_ID;
            h.setAttribute('data-%NS%agent', '1');
            // 全屏宿主用 inset:0 定位在 root 自身的盒子里;root 若被页面
            // transform 劫持会偏移,故同时给出 100vw/100vh 作为兜底尺寸。
            h.style.cssText = 'position:fixed;left:0;top:0;width:100vw;height:100vh;'
                + 'margin:0;padding:0;pointer-events:none;z-index:2147483645;'
                + 'display:none;';
            root().appendChild(h);
        }
        if (!h.__%NS%sh) {
            try { h.__%NS%sh = h.attachShadow ? h.attachShadow({ mode: 'closed' }) : null; } catch (e) { h.__%NS%sh = null; }
        }
        const sh = h.__%NS%sh;
        if (sh && !sh.querySelector('style')) {
            const st = document.createElement('style');
            st.setAttribute('data-%NS%agent', '1');
            st.textContent = STYLE_CSS;
            sh.appendChild(st);
        }
        if (sh && !sh.querySelector('.veil')) {
            const v = document.createElement('div');
            v.className = 'veil';
            v.setAttribute('data-%NS%agent', '1');
            sh.appendChild(v);
            const t = document.createElement('div');
            t.className = 'edge top';
            t.setAttribute('data-%NS%agent', '1');
            sh.appendChild(t);
            const b = document.createElement('div');
            b.className = 'edge bottom';
            b.setAttribute('data-%NS%agent', '1');
            sh.appendChild(b);
        }
        host = h;
        render();
    };
    const render = () => {
        if (!host) return;
        const on = active && !suspended;
        try { host.style.display = on ? 'block' : 'none'; } catch (e) {}
    };
    NS.MaskSet = (v) => { active = !!v; try { ensure(); } catch (e) {} render(); return active; };
    NS.MaskSuspend = (v) => { suspended = !!v; render(); return suspended; };
    NS.MaskEnsure = () => { ensure(); return true; };
    NS.MaskProbe = () => ({
        present: !!document.getElementById(MASK_ID),
        active: active,
        suspended: suspended,
        top_frame: window.top === window,
    });
    try {
        ['MaskSet', 'MaskSuspend', 'MaskEnsure', 'MaskProbe', 'MaskRev']
            .forEach((k) => {
                const d = Object.getOwnPropertyDescriptor(NS, k);
                if (!d) return;
                Object.defineProperty(NS, k,
                    { enumerable: false, configurable: true, writable: true, value: d.value });
            });
    } catch (e) {}
    try {
        if (!NS.MaskMO && document.documentElement) {
            NS.MaskMO = new MutationObserver(() => {
                try {
                    if (!document.getElementById(MASK_ID)) { ensure(); }
                } catch (e) {}
            });
            NS.MaskMO.observe(document.documentElement, { childList: true, subtree: true });
        }
    } catch (e) {}
    const boot = () => { try { ensure(); render(); } catch (e) {} };
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', boot, { once: true });
        window.addEventListener('load', boot, { once: true });
    }
    boot();
})()"#;

// ===================== 注入与切换 =====================

/// 向页面注入蒙层脚本(新文档自动重挂 + 当前文档立即执行)。
///
/// 仅 headed 且蒙层启用时调用(hidden 无人工观看,零注入零开销)。
pub async fn ensure(page: &Page) {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
    let _ = page
        .execute(AddScriptToEvaluateOnNewDocumentParams {
            source: super::status::render_ns(AGENT_MASK_JS_TEMPLATE),
            world_name: None,
            include_command_line_api: None,
            run_immediately: Some(true),
        })
        .await;
}

/// 挂/摘视觉蒙层(幂等)。
pub async fn set_visual(page: &Page, on: bool) {
    let api = super::status::ns_member_expr("MaskSet");
    let js = format!("{api} ? {api}({}) : false", if on { "true" } else { "false" });
    let _ = page.evaluate(js.as_str()).await;
}

/// 截图取证避让:临时隐藏蒙层(拍完恢复),证据图不蒙。
pub async fn set_visual_suspended(page: &Page, suspend: bool) {
    let api = super::status::ns_member_expr("MaskSuspend");
    let js = format!("{api} ? {api}({}) : false", if suspend { "true" } else { "false" });
    let _ = page.evaluate(js.as_str()).await;
}

/// 设置浏览器级输入闸(`Input.setIgnoreInputEvents`)。
///
/// 返回是否设置成功(CDP 调用失败 → false,调用方据此 fail-open,
/// 保证「蒙层在但页面仍可点」不会让任务卡死)。
pub async fn set_input_ignored(page: &Page, ignore: bool) -> bool {
    use chromiumoxide::cdp::browser_protocol::input::SetIgnoreInputEventsParams;
    let params = match SetIgnoreInputEventsParams::builder().ignore(ignore).build() {
        Ok(p) => p,
        Err(_) => return false,
    };
    page.execute(params).await.is_ok()
}

/// 按控制权状态把页面收敛到目标形态(幂等,可在导航后重放)。
///
/// - `agent` 态:蒙层在位 + 输入闸拦截;
/// - `user` 态:蒙层撤除 + 输入闸放行。
///
/// **只在 headed 模式生效**:hidden(无头/管道/CI)没有人工在看页面,蒙层没有意义;
/// 更要紧的是 `Input.setIgnoreInputEvents` 会把 **Agent 自己的 CDP 合成输入**
/// 一起拦下 —— 无头模式下没有人工需要保护,却把 Agent 自己锁死,任务全废。
pub async fn apply_state(page: &Page, state: ControlState) {
    let headed = super::ChromeManager::global().mode().is_headed();
    let want_mask = headed && mask_enabled() && state == ControlState::Agent;
    if want_mask {
        ensure(page).await;
    }
    set_visual(page, want_mask).await;
    if headed {
        set_input_ignored(page, want_mask).await;
    }
}

/// 页面当前是否处于「Agent 控制状态」(供工具层回写真值)。
pub fn state_of(page_id: &str) -> ControlState {
    web_control::state_of(Engine::Chrome, page_id)
}

/// 把控制权记录回写成信封字段。
pub fn state_fields(page_id: &str) -> Value {
    let rec = web_control::record_of(Engine::Chrome, page_id);
    let state = rec.as_ref().map(|r| r.state).unwrap_or(ControlState::Agent);
    let mut obj = serde_json::json!({
        "control_state": state.as_str(),
        "control_state_label": state.label(),
    });
    if let Some(r) = rec {
        if let Some(n) = r.note {
            obj["note"] = serde_json::Value::String(n);
        }
    }
    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 蒙层脚本纯视觉不参与hit_test() {
        let js = super::super::status::render_ns(AGENT_MASK_JS_TEMPLATE);
        assert!(js.contains("pointer-events:none"), "蒙层必须纯视觉,不得参与 hit-test");
        assert!(!js.contains("pointer-events:auto"), "整页 DOM 蒙层不得用 pointer-events:auto");
        assert!(js.contains("MaskSet"), "应暴露开关 API");
        assert!(js.contains("MaskSuspend"), "应暴露截图避让 API");
        assert!(js.contains("MaskEnsure"), "应暴露 DOM 自愈入口");
        assert!(!js.contains("%NS%"), "占位符应全部渲染");
    }

    #[test]
    fn 蒙层只在顶层frame渲染() {
        let js = super::super::status::render_ns(AGENT_MASK_JS_TEMPLATE);
        assert!(
            js.contains("if (window.top !== window) return;"),
            "必须有顶层早退门(子 frame 不渲染蒙层)"
        );
    }

    #[test]
    fn 蒙层用closed_shadow且带agent标记() {
        let js = super::super::status::render_ns(AGENT_MASK_JS_TEMPLATE);
        assert!(js.contains("attachShadow"));
        assert!(js.contains("mode: 'closed'"));
        let ns = super::super::status::dom_ns();
        assert!(
            js.contains(&format!("data-{ns}agent")),
            "须带 agent 标记供提取过滤(渲染后带随机 ns)"
        );
    }

    #[test]
    fn 蒙层z序低于两条告知栏() {
        let js = super::super::status::render_ns(AGENT_MASK_JS_TEMPLATE);
        assert!(js.contains("z-index:2147483645"), "蒙层 z-index 应低于告知栏(2147483646)");
        assert!(!js.contains("z-index:2147483646"), "不得与告知栏同层");
    }

    #[test]
    fn 输入解锁白名单覆盖cdp合成输入动作() {
        for a in ["click_real", "hover", "double_click", "right_click", "key_press", "scroll"] {
            assert!(action_needs_unlock(a), "{a} 走 CDP 合成输入,必须先解后锁");
        }
        // JS 通道动作不经过浏览器输入管线,锁着也能跑,不必开窗
        assert!(!action_needs_unlock("click"));
        assert!(!action_needs_unlock("eval_js"));
        assert!(!action_needs_unlock("screenshot"));
    }
}
