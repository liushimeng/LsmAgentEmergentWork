//! 可视化模式页面蒙层与人工操作拦截(第 141 轮)。
//!
//! 需求:headed 可视化浏览器中,人工**可实时观看**页面变化但**不可点击/操作**
//! (防人工与 Agent 交叉操作);人工与页面的交互一律走「人工介入」弹窗
//! (`request_human`,提问期间默认自动解锁页面)。
//!
//! 双层设计(详案 `docs/MCP_Web_Use/05-可视化模式蒙层与人工操作拦截.md`):
//! - **L1 输入拦截(CDP 硬闸门)**:`Input.setIgnoreInputEvents(ignore=true)`,
//!   浏览器级丢弃该页面全部真实用户输入(鼠标/滚轮/键盘/触摸),target 级全 frame
//!   一致、跨导航持久。**实测同时拦截 CDP 合成输入**(`Input.dispatch*`),因此
//!   Agent 自己的输入类动作必须「先解后锁」([`action_dispatches_input`] 白名单,
//!   `mcp_web_use::control::run` 统一包裹)。
//! - **L2 视觉蒙层(JS 纯视觉)**:全屏半透明遮罩 + 锁定提示条,
//!   `pointer-events:none` 不参与 hit-test(Agent 点击/跨域 iframe 零影响),
//!   `data-laew-agent="1"`(blockers/elements/dom 提取过滤),
//!   `Page.addScriptToEvaluateOnNewDocument` 挂载(导航自动重注入,全 frame 生效)。
//!
//! 为什么不用 DOM 蒙层(`pointer-events:auto`)拦输入:CDP 合成输入与真实输入在
//! hit-test 层不可区分(`isTrusted` 均为 true),会把 Agent 自己的点击一并吃掉;
//! 且「挂起标志」跨域 iframe 读不到,Agent 点跨域 iframe 区域会被误拦且无法解除。

use chromiumoxide::cdp::browser_protocol::input::SetIgnoreInputEventsParams;
use chromiumoxide::Page;
use serde_json::{json, Value};

/// 蒙层遮罩元素 id(测试断言锚点;`inspect(elements/dom)` 经 data-laew-agent 过滤)。
pub const OVERLAY_ELEMENT_ID: &str = "__laew_overlay__";

/// 蒙层提示条 id(仅主文档注入,iframe 只铺蒙层不重复文案)。
pub const OVERLAY_HINT_ID: &str = "__laew_overlay_hint__";

/// 蒙层 JS 模板:`{INITIAL}` 占位符由 [`overlay_script`] 烘焙为 true/false,
/// 使运行期 `set_overlay(false)` 后发生的导航按「期望态」重建(而非恒 true)。
///
/// 设计要点:
/// - `pointer-events:none` —— 纯视觉,零 hit-test 参与(见模块文档);
/// - `z-index:2147483646` —— 恰低于高亮蓝框(2147483647),徽标不被蒙层盖住;
/// - 提示条文本含「人工介入」引导,人工点不动页面时知道去哪交互;
/// - `data-laew-agent="1"` —— 与高亮蓝框同款标记,`blockers` 正文扫描
///   (克隆 body 后剥 `[data-laew-agent]`)天然排除提示条文本;
/// - `document.body` 未就绪(极早期文档)时挂 documentElement,DOMContentLoaded 补挂。
pub const AGENT_OVERLAY_JS_TEMPLATE: &str = r#"(() => {
    if (window.__laewOverlayInstalled) return;
    window.__laewOverlayInstalled = true;
    window.__laewOverlayOn = {INITIAL};
    const ensure = () => {
        let mask = document.getElementById('__laew_overlay__');
        if (!mask) {
            mask = document.createElement('div');
            mask.id = '__laew_overlay__';
            mask.setAttribute('data-laew-agent', '1');
            mask.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483646;'
                + 'background:rgba(15,23,42,0.22);display:flex;align-items:flex-end;justify-content:flex-start;';
            if (window.top === window) {
                const chip = document.createElement('div');
                chip.id = '__laew_overlay_hint__';
                chip.setAttribute('data-laew-agent', '1');
                chip.style.cssText = 'margin:0 0 18px 18px;padding:8px 14px;background:rgba(15,23,42,0.85);'
                    + 'color:#e2e8f0;font:600 13px/1.7 system-ui,-apple-system,"PingFang SC","Microsoft YaHei",sans-serif;'
                    + 'border-radius:10px;box-shadow:0 4px 16px rgba(0,0,0,0.35);max-width:72%;pointer-events:none;';
                chip.textContent = '🔒 LAEW Agent 控制中 · 页面已锁定(仅观看)';
                const sub = document.createElement('div');
                sub.setAttribute('data-laew-agent', '1');
                sub.style.cssText = 'font-weight:400;color:#94a3b8;font-size:12px;';
                sub.textContent = '人工操作已拦截;如需介入请应答「人工介入」弹窗';
                chip.appendChild(sub);
                mask.appendChild(chip);
            }
            (document.body || document.documentElement).appendChild(mask);
        }
        mask.style.display = window.__laewOverlayOn ? '' : 'none';
    };
    window.__laewOverlaySet = (on) => {
        window.__laewOverlayOn = !!on;
        ensure();
        return !!on;
    };
    ensure();
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', () => ensure(), { once: true });
    }
})()"#;

/// 按期望态生成注入脚本(烘焙初始显示态)。
pub fn overlay_script(initial_on: bool) -> String {
    AGENT_OVERLAY_JS_TEMPLATE.replace("{INITIAL}", if initial_on { "true" } else { "false" })
}

/// `LAEW_WEB_OVERLAY` 缺省开关(第 141 轮):
/// `off/0/false/no` → 关闭(严格回退到第 140 轮行为);未设置/其它 → 开启。
pub fn overlay_default_from_env() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_OVERLAY")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 给页面注入蒙层脚本(新文档自动重挂 + 当前文档立即执行,全 frame 生效)。
///
/// 与 `browser.rs::inject_agent_highlight` 同款通道;`initial_on` 为实例期望态,
/// 导航后新文档按它重建蒙层,配合导航类动作收尾的 re-assert 使两层收敛。
pub async fn inject_agent_overlay(page: &Page, initial_on: bool) {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
    let _ = page
        .execute(AddScriptToEvaluateOnNewDocumentParams {
            source: overlay_script(initial_on),
            world_name: None,
            include_command_line_api: None,
            run_immediately: Some(true),
        })
        .await;
}

/// 该 control_action 是否会经 CDP `Input.dispatch*`(或 chromiumoxide
/// `Element::click()/type_str()`,内部同为 Input 域)注入输入事件。
///
/// 命中者由 `mcp_web_use::control::run` 统一「先解后锁」:
/// 动作前 `setIgnoreInputEvents(false)` → 动作 → `setIgnoreInputEvents(true)`。
/// 保守圈定:部分动作默认走 JS 路径(如 `input_text(use_js=true)`),多解锁一次
/// 无害;漏掉真注入输入的动作则会被蒙层吃掉,宁可多不可少。
/// 不注入输入事件的动作(`wait/eval_js/screenshot/set_*/navigate 类`)不解锁 ——
/// 长动作(如 wait 30s)期间保持锁定,避免页面裸奔。
pub fn action_dispatches_input(action: &str) -> bool {
    matches!(
        action,
        "click"
            | "human_click"
            | "right_click"
            | "double_click"
            | "hover"
            | "scroll"
            | "scroll_to"
            | "key_press"
            | "press_sequence"
            | "input_text"
            | "human_input"
            | "clear_input"
            | "upload_file"
            | "select_option"
            | "download"
            | "drag"
            | "focus"
            | "blur"
            | "mouse_move"
            | "dispatch_event"
    )
}

/// 该 control_action 完成后是否需要 re-assert 蒙层(导航类,第 141 轮)。
///
/// CDP 输入拦截标志实测跨导航持久;JS 蒙层新文档按注入时烘焙的初始态重建,
/// 若运行期 `set_overlay(false)` 后发生导航,蒙层会以旧态回归 —— re-assert
/// 用实例期望态重新应用两层,保证收敛。
pub fn action_needs_overlay_reassert(action: &str) -> bool {
    matches!(action, "navigate" | "back" | "forward" | "reload" | "new_tab")
}

/// 应用/解除页面蒙层(双层:L1 CDP 输入拦截 + L2 JS 视觉蒙层)。
///
/// `active=true`:`setIgnoreInputEvents(true)` + `__laewOverlaySet(true)`;
/// `active=false` 同理反向(人工介入期间解锁、运行时关闭均走这里)。
/// 返回两层应用结果(fail-open:单层失败不阻断另一层,如实回报)。
pub async fn apply_page_overlay(page: &Page, active: bool) -> std::result::Result<Value, String> {
    // L1:CDP 输入拦截(实测同时拦 CDP 合成输入 → 输入动作须先解后锁)
    let input_locked = page
        .execute(SetIgnoreInputEventsParams::new(active))
        .await
        .is_ok();
    // L2:JS 视觉蒙层(未注入脚本的页面返回 null → applied=false,如实上报)
    let js = format!("window.__laewOverlaySet ? window.__laewOverlaySet({active}) : null");
    let r = page
        .evaluate(js.as_str())
        .await
        .map_err(|e| format!("切换蒙层失败:{e}"))?;
    let visual_applied = !r.value().map(|v| v.is_null()).unwrap_or(true);
    Ok(json!({
        "input_locked": active && input_locked,
        "input_lock_applied": input_locked,
        "visual_mask": active && visual_applied,
    }))
}

/// 仅切换视觉蒙层可见性(截图避让用:**拍前隐藏 → 拍完恢复**,蒙层不进证据链)。
///
/// 不动 L1 输入拦截(截图期间无需放行输入);返回是否实际应用(未注入脚本 = false)。
pub async fn mask_set_visible(page: &Page, visible: bool) -> bool {
    let js = format!("window.__laewOverlaySet ? window.__laewOverlaySet({visible}) : null");
    match page.evaluate(js.as_str()).await {
        Ok(r) => !r.value().map(|v| v.is_null()).unwrap_or(true),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量类测试互斥(browser_mode.rs 同款,防并行 set_var 互踩)。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn env_default_on_and_off_values() {
        let _g = env_lock();
        unsafe {
            std::env::remove_var("LAEW_WEB_OVERLAY");
        }
        assert!(overlay_default_from_env());
        for v in ["off", "0", "false", "no", "OFF", " No "] {
            unsafe { std::env::set_var("LAEW_WEB_OVERLAY", v) };
            assert!(!overlay_default_from_env(), "{v} 应视为关闭");
        }
        // 非关闭语义的值不误杀(与 LAEW_TARGET_ANCHOR 等开关同款只认 off 系)
        for v in ["on", "1", "true", "garbage"] {
            unsafe { std::env::set_var("LAEW_WEB_OVERLAY", v) };
            assert!(overlay_default_from_env(), "{v} 应视为开启");
        }
        unsafe {
            std::env::remove_var("LAEW_WEB_OVERLAY");
        }
    }

    #[test]
    fn script_bakes_initial_state() {
        assert!(overlay_script(true).contains("window.__laewOverlayOn = true;"));
        assert!(overlay_script(false).contains("window.__laewOverlayOn = false;"));
        assert!(!overlay_script(true).contains("{INITIAL}"));
    }

    /// 蒙层关键标记锁死:纯视觉(pointer-events:none)/ 提取过滤(data-laew-agent)/
    /// 提示文案 / 恰低于蓝框的 z-index / 挂载函数。
    #[test]
    fn script_markers_locked() {
        let js = overlay_script(true);
        assert!(js.contains(OVERLAY_ELEMENT_ID), "蒙层元素 id 应出现在脚本中");
        assert!(js.contains(OVERLAY_HINT_ID), "提示条 id 应出现在脚本中");
        assert!(js.contains("pointer-events:none"));
        assert!(js.contains("data-laew-agent"));
        assert!(js.contains("z-index:2147483646"));
        assert!(js.contains("LAEW Agent 控制中"));
        assert!(js.contains("人工介入"));
        assert!(js.contains("__laewOverlaySet"));
        assert!(js.contains("window.top === window"));
    }

    #[test]
    fn input_action_whitelist() {
        // 注入输入的动作必须解锁(含 chromiumoxide Element click/type 路径)
        for a in [
            "click", "human_click", "right_click", "double_click", "hover", "scroll",
            "scroll_to", "key_press", "press_sequence", "input_text", "human_input",
            "clear_input", "upload_file", "select_option", "download", "drag", "focus",
            "blur", "mouse_move", "dispatch_event",
        ] {
            assert!(action_dispatches_input(a), "{a} 应解锁");
        }
        // 长动作/纯读/状态设置不解锁(wait 30s 期间不能裸奔)
        for a in [
            "wait", "eval_js", "screenshot", "heartbeat", "navigate", "back", "forward",
            "reload", "new_tab", "close_tab", "set_cookie", "set_storage", "set_viewport",
            "set_window", "sync_viewport", "set_highlight", "set_overlay", "request_human",
        ] {
            assert!(!action_dispatches_input(a), "{a} 不应解锁");
        }
    }

    #[test]
    fn nav_actions_need_reassert() {
        for a in ["navigate", "back", "forward", "reload", "new_tab"] {
            assert!(action_needs_overlay_reassert(a), "{a} 应回收 re-assert");
        }
        for a in ["click", "wait", "eval_js", "set_overlay", "request_human"] {
            assert!(!action_needs_overlay_reassert(a), "{a} 无需 re-assert");
        }
    }

    // =================== 真浏览器集成验证(#[ignore],本地 --ignored 跑) ===================
    //
    // 覆盖第 141 轮三条关键链路(详案 docs/MCP_Web_Use/05 §10):
    // 1. 蒙层元素注入存在性(headed + overlay=on);
    // 2. 输入锁语义:锁定时 CDP 合成点击被拦(实测 setIgnoreInputEvents 连合成
    //    输入一起拦)→ 解锁后恢复 —— 即「先解后锁」包装的必要性回归锁;
    // 3. 蒙层隐藏/恢复(截图避让通道)。
    // 需要本机安装 Chrome/Edge;跑 `cargo test --lib browser_overlay -- --ignored --nocapture`。
    #[tokio::test]
    #[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
    async fn real_browser_overlay_lock_and_mask() {
        use crate::agent::browser::{BrowserManager, BrowserMode};
        let mgr = BrowserManager::global();
        let url = "data:text/html,<button id='b' onclick='this.dataset.c=(+this.dataset.c||0)+1' \
                   style='position:fixed;left:0;top:0;width:200px;height:100px'>x</button>";
        let (pid, _, _) = mgr
            .new_page(url, BrowserMode::Headed, None, None, Some((800, 600)), true, true)
            .await
            .expect("启动有头浏览器失败(本机是否装有 Chrome?)");
        let page = mgr.page(&pid).await.expect("page 句柄");

        let count = |page: &Page| {
            let p = page.clone();
            async move {
                p.evaluate(
                    "document.getElementById('b') ? (+document.getElementById('b').dataset.c || 0) : -1",
                )
                .await
                .ok()
                .and_then(|r| r.value().cloned())
                .and_then(|v| v.as_i64())
                .unwrap_or(-1)
            }
        };
        let cdp_click = |page: &Page| {
            let p = page.clone();
            async move {
                use chromiumoxide::cdp::browser_protocol::input::{
                    DispatchMouseEventParams, DispatchMouseEventType,
                };
                for t in [DispatchMouseEventType::MousePressed, DispatchMouseEventType::MouseReleased] {
                    let _ = p
                        .execute(
                            DispatchMouseEventParams::builder()
                                .r#type(t)
                                .x(50.0)
                                .y(30.0)
                                .button(chromiumoxide::cdp::browser_protocol::input::MouseButton::Left)
                                .click_count(1)
                                .build()
                                .unwrap(),
                        )
                        .await;
                }
            }
        };

        // ① 蒙层元素存在 + 提示条在主文档
        let mask_probe = page
            .evaluate(
                "!!document.getElementById('__laew_overlay__') && !!document.getElementById('__laew_overlay_hint__')",
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(mask_probe, "蒙层/提示条未注入");

        // ② 输入锁:new_page 已 apply(true) → CDP 点击应被拦
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 0, "锁定时 CDP 点击不应生效");
        // 解锁 → 点击生效
        apply_page_overlay(&page, false).await.unwrap();
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 1, "解锁后 CDP 点击应生效");
        // 复锁 → 再拦(「先解后锁」回归锁)
        apply_page_overlay(&page, true).await.unwrap();
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 1, "复锁后 CDP 点击应再次被拦");

        // ③ 蒙层隐藏/恢复(截图避让通道)
        assert!(mask_set_visible(&page, false).await, "蒙层隐藏应实际应用");
        let hidden = page
            .evaluate("getComputedStyle(document.getElementById('__laew_overlay__')).display")
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        assert_eq!(hidden, "none", "隐藏后 display 应为 none");
        assert!(mask_set_visible(&page, true).await);

        // 收尾:回收浏览器(全局单例,必须清干净防污染其它测试)
        mgr.shutdown().await;
    }
}
