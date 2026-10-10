//! HITL 凭证区域探测与临时放行(第 144 轮)。
//!
//! 需求:Agent 用 MCP_Web_Use 发现网站需要输入**账号/密码/验证码**等必须人工
//! 输入的信息时,相关区域应是 partial 部分屏蔽模式的**非屏蔽区域**(白名单挖洞),
//! 而不是整页放行。本模块提供三件事:
//!
//! 1. [`CREDENTIAL_ZONE_PROBE_JS`]:页面内一次 eval_js 探测凭证输入区,产出主文档
//!    CSS 选择器(partial `allow_selectors` 直接可用);规则与优先级见 doc 08 §4.2;
//! 2. [`spawn_unlock_reassert`]:提问期间的导航重放协程 —— 人工提交登录后页面导航
//!    (登录→2FA),新文档按 `new_page` 烘焙态回锁会打断多步人工流程,协程订阅
//!    主 frame 导航持续重放临时放行态,收口时 abort;
//! 3. [`hitl_scope_enabled`]:杀开关 `LAEW_WEB_HITL_SCOPE=off` 跳过探测回退整页
//!    open(第 143 轮行为),与 `LAEW_WEB_OVERLAY` 等开关同构。
//!
//! 设计:`docs/MCP_Web_Use/08-浏览器进程保护与凭证区域部分放行.md` §4。

use serde_json::Value;

/// 杀开关环境变量:off 系 → HITL 解锁跳过凭证区探测,恒整页 open。
const HITL_SCOPE_ENV: &str = "LAEW_WEB_HITL_SCOPE";

/// HITL 凭证区域放行是否启用(默认开;`LAEW_WEB_HITL_SCOPE=off/0/false/no` 关)。
pub(super) fn hitl_scope_enabled() -> bool {
    !matches!(
        std::env::var(HITL_SCOPE_ENV)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 凭证区域探测 JS(主文档执行;输出 `[{kind, selector}]`,JS 侧已去重 + ≤8 条)。
///
/// 探测优先级:① 密码框 → 就近 form/语义祖先容器(提交按钮落在放行洞内);
/// ② 短信/OTP/验证码文本框(属性命中关键词)→ 同款容器化;③ 验证码/滑块
/// iframe(取 iframe 元素本身,主文档挖洞覆盖其矩形);④ 验证码/滑块容器
/// div/section(矩形 ≤40% 视口,防整页语义容器误命中)。
///
/// 选择器生成:`#id`(CSS.escape)→ `tag[name="…"]`(唯一时)→ `tag:nth-of-type`
/// 路径(≤4 层),生成即 `querySelectorAll` 自验命中;单条 ≤150 字符(<160 上限)。
/// 不穿透 shadow DOM(探测不到即回退整页 open,不误伤)。
pub(super) const CREDENTIAL_ZONE_PROBE_JS: &str = r#"(() => {
    const KW = {
        zone: /captcha|verif|recaptcha|geetest|tcaptcha|dingxiang|yidun|slider|challenge|login|signin|password|passwd|sms|otp|one[-_ ]?time|2fa|two[-_ ]?factor|验证码|校验|滑|短信|动态码|登录|登陆|密码|口令/i,
        input: /otp|one[-_ ]?time|sms|captcha|verif|code|短信|动态码|验证码|校验/i
    };
    const hit = (s, re) => !!s && re.test(s);
    const esc = (s) => (window.CSS && CSS.escape) ? CSS.escape(s) : String(s).replace(/([^\w-])/g, '\\$1');
    const visible = (el) => {
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) return false;
        const st = getComputedStyle(el);
        return st.visibility !== 'hidden' && st.display !== 'none';
    };
    const selOf = (el) => {
        if (el.id) return '#' + esc(el.id);
        const name = el.getAttribute('name');
        if (name) {
            const s = el.tagName.toLowerCase() + '[name=' + JSON.stringify(name) + ']';
            try { if (document.querySelectorAll(s).length === 1) return s; } catch (e) {}
        }
        const parts = [];
        let cur = el;
        for (let depth = 0; cur && cur.nodeType === 1 && depth < 4; depth++) {
            if (cur.id) { parts.unshift('#' + esc(cur.id)); break; }
            let part = cur.tagName.toLowerCase();
            const parent = cur.parentElement;
            if (parent) {
                const same = Array.prototype.filter.call(parent.children, (c) => c.tagName === cur.tagName);
                if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(cur) + 1) + ')';
            }
            parts.unshift(part);
            cur = parent;
        }
        const s = parts.join(' > ');
        try { return document.querySelectorAll(s).length >= 1 ? s : ''; } catch (e) { return ''; }
    };
    const out = [];
    const seen = new Set();
    const push = (el, kind) => {
        if (!el || !visible(el)) return;
        const s = selOf(el);
        if (!s || s.length > 150 || seen.has(s)) return;
        seen.add(s);
        out.push({ kind: kind, selector: s });
    };
    const metaOf = (el) => ((el.getAttribute('class') || '') + ' ' + (el.id || '') + ' '
        + (el.getAttribute('name') || '') + ' ' + (el.getAttribute('title') || '')
        + ' ' + (el.getAttribute('src') || ''));
    // 容器化:输入框 → 就近 form(≤60% 视口)→ 语义祖先(≤4 层)→ 父节点兜底
    const containerOf = (input) => {
        const va = Math.max(1, innerWidth * innerHeight);
        const form = input.closest ? input.closest('form') : null;
        if (form) {
            const r = form.getBoundingClientRect();
            if (r.width * r.height <= va * 0.6) return { el: form, kind: 'form' };
        }
        let cur = input.parentElement;
        for (let up = 0; cur && up < 4; up++) {
            const meta = (cur.getAttribute('class') || '') + ' ' + (cur.id || '');
            if (hit(meta, KW.zone)) {
                const r = cur.getBoundingClientRect();
                if (r.width * r.height <= va * 0.6) return { el: cur, kind: 'container' };
            }
            cur = cur.parentElement;
        }
        return { el: input.parentElement || input, kind: 'field-parent' };
    };
    // ① 密码框(最典型的「必须人工输入」)
    Array.prototype.forEach.call(document.querySelectorAll('input[type=password]'), (el) => {
        if (!visible(el)) return;
        const c = containerOf(el);
        push(c.el, c.kind);
    });
    // ② 短信/OTP/验证码文本框(非密码,属性命中关键词)
    Array.prototype.forEach.call(
        document.querySelectorAll('input:not([type=password]):not([type=hidden])'),
        (el) => {
            if (!visible(el)) return;
            const t = (el.getAttribute('type') || 'text').toLowerCase();
            if (t !== 'text' && t !== 'tel' && t !== 'number' && t !== '') return;
            const attr = [el.getAttribute('name'), el.id, el.getAttribute('placeholder'),
                el.getAttribute('autocomplete'), el.getAttribute('aria-label')].join(' ');
            if (!hit(attr, KW.input)) return;
            const c = containerOf(el);
            push(c.el, c.kind);
        });
    // ③ 验证码/滑块 iframe(取 iframe 元素本身,主文档挖洞覆盖其矩形;
    //    iframe 内部由盾区 JS 的子 frame 白名单 fail-open 放行,见 browser_overlay.rs)
    Array.prototype.forEach.call(document.querySelectorAll('iframe'), (el) => {
        if (!hit(metaOf(el), KW.zone)) return;
        push(el, 'captcha-iframe');
    });
    // ④ 验证码/滑块容器 div/section(矩形 ≤40% 视口)
    Array.prototype.forEach.call(document.querySelectorAll('div,section'), (el) => {
        if (!hit(metaOf(el), KW.zone)) return;
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) return;
        if (r.width * r.height > innerWidth * innerHeight * 0.4) return;
        push(el, 'captcha-zone');
    });
    // ⑤ 人工核验挑战弹层(第 152 轮)—— 与 blocker_probe 的弹层选取同构:
    // 「高 z-index + 固定/绝对定位 + 面积够大」,不看类名(现代前端多用 BEM/哈希
    // 类名,关键词通道必然落空)。这一条直接对应实测事故:豆包的**拖拽拼图**弹层
    // 类名不含 captcha/verify,①②③④ 全部探测不到 → 回退整页 open;而一旦页面
    // 上恰好有个含 zone 关键词的容器命中了 ④,就会走 partial 挖洞路径,盾罩把
    // 整块挑战弹层(拖拽区、九宫格)罩住 —— 人工看得见弹窗却拖不动。整层放行是
    // 唯一正确解:挑战弹层内部的每一个像素都是人工必操作区。
    // 面积上限放宽到 60% 视口(挑战弹层通常比登录表单大)。
    let bestLayer = null;
    const cand = 'dialog[open], [role="dialog"], [aria-modal="true"], body div, body section, body main';
    Array.prototype.forEach.call(document.querySelectorAll(cand), (el) => {
        if (el.closest('[data-laew-agent]')) return;
        const cs = getComputedStyle(el);
        if (cs.display === 'none' || cs.visibility === 'hidden' || parseFloat(cs.opacity) < 0.05) return;
        if (cs.position !== 'fixed' && cs.position !== 'absolute') return;
        const r = el.getBoundingClientRect();
        if (r.width < 200 || r.height < 120) return;
        const area = r.width * r.height;
        if (area > innerWidth * innerHeight * 0.6) return;
        const z = parseInt(cs.zIndex, 10) || 0;
        if (!bestLayer || z > bestLayer.z) bestLayer = { el: el, z: z };
    });
    if (bestLayer) push(bestLayer.el, 'challenge-overlay');
    return out.slice(0, 8);
})()"#;

/// 选择器清单净化(纯函数,显式传参与探测结果共用):trim / 去空 / 去重 /
/// 单条 ≤160 字符 / 总数 ≤8(对齐 `MAX_GUARD_SELECTORS`)。
pub(super) fn sanitize_selector_list(list: &[String]) -> Vec<String> {
    use crate::agent::browser_overlay::{MAX_GUARD_SELECTOR_LEN, MAX_GUARD_SELECTORS};
    let mut out: Vec<String> = Vec::new();
    for raw in list {
        let s = raw.trim();
        if s.is_empty() || s.chars().count() > MAX_GUARD_SELECTOR_LEN {
            continue;
        }
        if out.iter().any(|x| x == s) {
            continue;
        }
        out.push(s.to_string());
        if out.len() >= MAX_GUARD_SELECTORS {
            break;
        }
    }
    out
}

/// 解析探测 JS 的返回值(`[{kind, selector}]` → 选择器列表,净化同上)。
pub(super) fn parse_probe_result(v: Value) -> Vec<String> {
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    let raw: Vec<String> = arr
        .iter()
        .filter_map(|item| {
            item.get("selector")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    sanitize_selector_list(&raw)
}

/// 对页面执行凭证区域探测(fail-open:开关关闭/执行失败/解析失败 → 空列表,
/// 调用方回退整页 open,绝不阻塞提问)。
pub(super) async fn probe_credential_zones(page: &chromiumoxide::Page) -> Vec<String> {
    if !hitl_scope_enabled() {
        return Vec::new();
    }
    match super::eval_js_string(page, CREDENTIAL_ZONE_PROBE_JS).await {
        Ok(v) => parse_probe_result(v),
        Err(_) => Vec::new(),
    }
}

/// 提问期间的导航重放协程(第 144 轮):人工提交登录 → 新文档按烘焙态(locked)
/// 回锁,会把人工锁死在多步流程(登录→2FA)半路。协程订阅主 frame 导航,重放
/// 临时放行态(partial 挖洞或 open),与 Agent 导航动作收尾的 re-assert 同构;
/// 调用方在 HITL 收口(应答/超时/取消/不可用)时 `abort()`。
///
/// 容错:新文档执行上下文就绪有竞态,`__laewGuardSet` 未就绪时间隔 400ms 重试
/// (≤3 次);页面关闭 → 事件流结束,协程自然退出。
pub(super) fn spawn_unlock_reassert(
    page: chromiumoxide::Page,
    cfg: crate::agent::browser_overlay::PageGuardConfig,
) -> tokio::task::JoinHandle<()> {
    use chromiumoxide::cdp::browser_protocol::page::EventFrameNavigated;
    use futures::StreamExt;
    tokio::spawn(async move {
        let Ok(mut stream) = page.event_listener::<EventFrameNavigated>().await else {
            return;
        };
        while let Some(ev) = stream.next().await {
            // 仅主 frame 导航重建主文档管控;子 frame 加载不动主文档状态。
            if ev.frame.parent_id.is_some() {
                continue;
            }
            for attempt in 0..3 {
                if attempt > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
                let installed = page
                    .evaluate("!!(window.__laewGuardSet)")
                    .await
                    .ok()
                    .and_then(|r| r.value().cloned())
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if installed {
                    let _ =
                        crate::agent::browser_overlay::apply_page_guard(&page, &cfg).await;
                    break;
                }
            }
        }
    })
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
    fn hitl_scope_env_switch() {
        let _g = env_lock();
        unsafe { std::env::remove_var(HITL_SCOPE_ENV) };
        assert!(hitl_scope_enabled(), "缺省应启用");
        for v in ["off", "0", "false", "no", "OFF", " No "] {
            unsafe { std::env::set_var(HITL_SCOPE_ENV, v) };
            assert!(!hitl_scope_enabled(), "{v} 应视为关闭");
        }
        for v in ["on", "1", "garbage"] {
            unsafe { std::env::set_var(HITL_SCOPE_ENV, v) };
            assert!(hitl_scope_enabled(), "{v} 不应误杀(只认 off 系)");
        }
        unsafe { std::env::remove_var(HITL_SCOPE_ENV) };
    }

    #[test]
    fn sanitize_dedupes_and_caps() {
        let list = vec![
            "  .login  ".to_string(),
            ".login".to_string(), // 去重(trim 后相同)
            "".to_string(),       // 空剔除
            " ".to_string(),
            "a".repeat(161), // 超长剔除
            "#ok".to_string(),
        ];
        let out = sanitize_selector_list(&list);
        assert_eq!(out, vec![".login".to_string(), "#ok".to_string()]);
        // 总数上限 8(对齐 MAX_GUARD_SELECTORS)
        let many: Vec<String> = (0..20).map(|i| format!("s{i}")).collect();
        assert_eq!(sanitize_selector_list(&many).len(), 8);
    }

    #[test]
    fn parse_probe_result_extracts_selectors() {
        let v = serde_json::json!([
            {"kind": "form", "selector": "#login-form"},
            {"kind": "captcha-iframe", "selector": "iframe[title=verify]"},
            {"kind": "container"},           // 缺 selector 字段 → 跳过
            {"kind": "zone", "selector": ""} // 空串 → 跳过
        ]);
        assert_eq!(
            parse_probe_result(v),
            vec!["#login-form".to_string(), "iframe[title=verify]".to_string()]
        );
        // 非数组返回空(fail-open)
        assert!(parse_probe_result(serde_json::json!("oops")).is_empty());
        assert!(parse_probe_result(serde_json::Value::Null).is_empty());
    }

    /// 探测 JS 关键标记锁死:四类候选 / 关键词 / 容器化规则 / 上限 8。
    #[test]
    fn probe_js_markers_locked() {
        let js = CREDENTIAL_ZONE_PROBE_JS;
        assert!(js.contains("input[type=password]"), "① 密码框");
        assert!(js.contains("autocomplete"), "② OTP/短信输入框属性");
        assert!(js.contains("'iframe'"), "③ 验证码 iframe");
        assert!(js.contains("'div,section'"), "④ 验证码容器");
        // 关键词覆盖(中英双语)
        assert!(js.contains("captcha"));
        assert!(js.contains("验证码"));
        assert!(js.contains("短信"));
        assert!(js.contains("动态码"));
        // 容器化:form 优先 + 60% 视口上限 + 4 层祖先
        assert!(js.contains("closest('form')"));
        assert!(js.contains("va * 0.6"));
        assert!(js.contains("up < 4"));
        // 选择器自验 + 长度上限 + 总数上限
        assert!(js.contains("querySelectorAll(s).length"));
        assert!(js.contains("s.length > 150"));
        assert!(js.contains("slice(0, 8)"));
        // ⑤ 挑战弹层整层放行(不看类名,只看几何与 z-index)
        assert!(js.contains("challenge-overlay"));
        assert!(js.contains("bestLayer"));
        assert!(js.contains("innerHeight * 0.6"));
        // 无裸控制字节风险锚点:转义走标准 JS 字面量
        assert!(js.contains("CSS.escape"));
    }
}
