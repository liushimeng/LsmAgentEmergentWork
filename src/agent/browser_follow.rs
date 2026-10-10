//! 页面跟随滚动与可视性诊断(第 152 轮)。
//!
//! 实测事故(2026-10-10 豆包多轮对话,`llaew_20261010_173332.log`):Agent 用
//! `eval_js` 读 DOM 拿到了第 N 轮豆包的完整回答,**用户/人工在浏览器窗口里却只
//! 看得到第一屏** —— 多轮对话把消息栈越堆越高,视口在 `open` 时按当时内容高度
//! 定过一次就再没变过,页面既不自动滚到底,人工想自己滚也滚不动:
//!
//! - locked 档的 L1 是 CDP `Input.setIgnoreInputEvents(true)`,**浏览器级一刀切,
//!   连滚轮一起拦**(第 141 轮实测:setIgnoreInputEvents 对真实输入与 CDP 合成
//!   输入一视同仁)。所以「人工自己滚一下」这条路在默认档位下根本不存在。
//!
//! 结论:不能靠「让人工滚」解决,只能靠**让页面自己停在最新**。本模块提供:
//!
//! - [`AUTO_FOLLOW_JS`]:页面内跟随器,每 1.2s 检查一次,**仅当用户本来就在底部**
//!   才吸底 —— 人工往上翻历史时绝不会被强行拽回来(这是敢自动跟随的前提);
//! - [`VISIBILITY_JS`]:一次性可视性诊断,回答「人工此刻能看到多少内容」,
//!   供 Agent 决定要不要先滚再截图 / 再截图给用户看。
//!
//! 注入通道与 [`crate::agent::browser_overlay`] 同款:
//! `Page.addScriptToEvaluateOnNewDocument`(导航自动重挂)+ 当前文档立即执行。

use serde_json::{json, Value};

/// 自动跟随的默认检查间隔(毫秒)。
pub const FOLLOW_INTERVAL_MS: i64 = 1200;
/// 「用户本来就在底部」的判定阈值(像素):`scrollHeight - scrollTop - clientHeight`
/// 不超过该值即认为贴着底部。放宽到 96 是为了容忍最后一屏的半行/子像素抖动。
pub const AT_BOTTOM_TOLERANCE_PX: f64 = 96.0;

/// 滚动容器与跟随判定 JS(不依赖 format!,花括号安全)。
///
/// 滚动容器的识别是整块的关键:AI 聊天站几乎都是「内层 div 滚动、外层页面不滚」
/// (`overflow:auto` 的消息列表),直接 `window.scrollTo` 什么都不会发生。
/// 这里取**视口内面积最大且确实可滚动(scrollHeight - clientHeight ≥ 40)**的
/// 元素,退化到 `document.scrollingElement`。
pub const AUTO_FOLLOW_JS: &str = r#"(() => {
  try {
    if (window.__laewAutoFollow) { return true; }
    const TOL = 96;
    const state = { on: false, timer: null, lastH: -1, lastAtBottom: true };
    const scroller = () => {
      let best = null, area = 0;
      const vw = window.innerWidth || 1, vh = window.innerHeight || 1;
      let nodes;
      try { nodes = document.querySelectorAll('div,main,section,ul,ol,article'); } catch (e) { nodes = []; }
      for (const el of nodes) {
        if (el.scrollHeight - el.clientHeight < 40) continue;
        const r = el.getBoundingClientRect();
        if (r.width < 120 || r.height < 120) continue;
        if (r.bottom < 0 || r.top > vh) continue;
        const a = Math.min(r.width, vw) * Math.min(r.height, vh);
        if (a > area) { area = a; best = el; }
      }
      return best || document.scrollingElement || document.documentElement;
    };
    const atBottom = (el) => (el.scrollHeight - el.scrollTop - el.clientHeight) <= TOL;
    const tick = () => {
      if (!state.on || !document.body) return;
      const el = scroller();
      if (!el) return;
      state.lastAtBottom = atBottom(el);
      if (state.lastAtBottom) { el.scrollTop = el.scrollHeight; }
      state.lastH = el.scrollHeight;
    };
    window.__laewAutoFollowStatus = () => {
      const el = scroller();
      if (!el) return { ready: false };
      return {
        ready: true,
        on: state.on,
        scroll_top: el.scrollTop,
        scroll_height: el.scrollHeight,
        client_height: el.clientHeight,
        at_bottom: state.lastAtBottom,
      };
    };
    window.__laewAutoFollow = function (on, intervalMs) {
      state.on = !!on;
      if (state.timer) { clearInterval(state.timer); state.timer = null; }
      if (state.on) {
        state.timer = setInterval(tick, intervalMs || 1200);
        tick();
      }
      return state.on;
    };
    return true;
  } catch (e) { return false; }
})()"#;

/// 可视性诊断 JS(一次性 eval,返回结构化诊断)。
pub const VISIBILITY_JS: &str = r#"(() => {
  try {
    const vw = window.innerWidth || 0, vh = window.innerHeight || 0;
    let best = null, area = 0;
    const nodes = document.querySelectorAll('div,main,section,ul,ol,article');
    for (const el of nodes) {
      if (el.scrollHeight - el.clientHeight < 40) continue;
      const r = el.getBoundingClientRect();
      if (r.width < 120 || r.height < 120) continue;
      if (r.bottom < 0 || r.top > vh) continue;
      const a = Math.min(r.width, vw) * Math.min(r.height, vh);
      if (a > area) { area = a; best = el; }
    }
    const el = best || document.scrollingElement || document.documentElement;
    if (!el) return { ok: false };
    const scrollTop = el.scrollTop, scrollHeight = el.scrollHeight, clientHeight = el.clientHeight;
    const at_bottom = (scrollHeight - scrollTop - clientHeight) <= 96;
    const below_px = Math.max(0, scrollHeight - scrollTop - clientHeight);
    // 人工在 locked 档看不到视口以下的内容;below_px 越大,「Agent 看得到、人看不到」
    // 的落差越大。
    const hidden_ratio = scrollHeight > 0
      ? Math.round((below_px / scrollHeight) * 1000) / 1000 : 0;
    const esc = (s) => (window.CSS && CSS.escape) ? CSS.escape(s) : String(s).replace(/([^\w-])/g, '\\$1');
    let selector = '';
    try {
      if (el.id) selector = '#' + esc(el.id);
      else if (el.className && typeof el.className === 'string') {
        const cls = el.className.trim().split(/\s+/).filter(Boolean)
          .filter(c => /^[A-Za-z_][\w-]*$/.test(c)).slice(0, 2);
        if (cls.length) {
          const s = cls.map(c => '.' + esc(c)).join('');
          if (document.querySelectorAll(s).length === 1) selector = s;
        }
      }
    } catch (e) {}
    return {
      ok: true, selector: selector,
      scrollable: scrollHeight - clientHeight > 40,
      at_bottom: at_bottom,
      below_fold_px: below_px,
      hidden_ratio: hidden_ratio,
      viewport: { w: vw, h: vh },
      scroll: { top: scrollTop, height: scrollHeight, client: clientHeight },
    };
  } catch (e) { return { ok: false, error: String(e) }; }
})()"#;

/// 跟随判定纯函数(Rust 侧镜像页面内逻辑,便于单测锁死语义)。
///
/// `at_bottom=false` 时**不跟随** —— 人工正在往上翻历史,强行吸底会把人从
/// 阅读位置拽走,这是「自动跟随」敢默认开启的唯一理由。
pub fn should_follow(enabled: bool, at_bottom: bool) -> bool {
    enabled && at_bottom
}

/// 给导航后的新文档执行跟随设置(配合 `addScriptToEvaluateOnNewDocument`)。
pub async fn install(page: &chromiumoxide::Page, enabled: bool) {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
    let _ = page
        .execute(AddScriptToEvaluateOnNewDocumentParams {
            source: format!("{AUTO_FOLLOW_JS}\nwindow.__laewAutoFollow && window.__laewAutoFollow({enabled}, {FOLLOW_INTERVAL_MS});"),
            world_name: None,
            include_command_line_api: None,
            run_immediately: Some(true),
        })
        .await;
}

/// 在当前文档上开关跟随(导航后新文档按 [`install`] 的期望态自动重挂)。
pub async fn set(page: &chromiumoxide::Page, enabled: bool) -> bool {
    match page
        .evaluate(format!(
            "window.__laewAutoFollow ? window.__laewAutoFollow({enabled}, {FOLLOW_INTERVAL_MS}) : null"
        ))
        .await
    {
        Ok(r) => !r.value().map(|v| v.is_null()).unwrap_or(true),
        Err(_) => false,
    }
}

/// 立刻吸底一次(不等 tick)—— `scroll_to(最新)` 的语义载体。
pub async fn scroll_to_bottom(page: &chromiumoxide::Page) -> bool {
    match page
        .evaluate(
            "(function(){ try { var el=null,a=0,vh=innerHeight||1; \
             document.querySelectorAll('div,main,section,ul,ol,article').forEach(function(n){ \
               if (n.scrollHeight-n.clientHeight<40) return; var r=n.getBoundingClientRect(); \
               if (r.width<120||r.height<120||r.bottom<0||r.top>vh) return; \
               var x=Math.min(r.width,innerWidth||1)*Math.min(r.height,vh); if (x>a){a=x;el=n;} }); \
             el = el || document.scrollingElement || document.documentElement; \
             if (!el) return false; el.scrollTop = el.scrollHeight; return true; } catch(e){ return false; } })()",
        )
        .await
    {
        Ok(r) => r.value().and_then(|v| v.as_bool()).unwrap_or(false),
        Err(_) => false,
    }
}

/// 可视性诊断(fail-open:执行失败返回 `{ok:false}` 形状,不抛)。
pub async fn visibility(page: &chromiumoxide::Page) -> Value {
    match page.evaluate(VISIBILITY_JS).await {
        Ok(r) => match r.value().cloned() {
            Some(v) if v.is_object() => v,
            _ => json!({"ok": false}),
        },
        Err(_) => json!({"ok": false}),
    }
}

/// 诊断 → 人类/模型可读的处置提示(纯函数,可单测)。
///
/// 关键点:诊断出「人工看不到」时,**给的是 Agent 能立即执行的动作**
/// (`scroll_to` / `auto_follow`),而不是一句「建议用户滚动」—— locked 档下
/// 用户滚不了,那句话等于把问题原样退回给用户。
pub fn visibility_note(v: &Value) -> Option<String> {
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    if v.get("scrollable").and_then(Value::as_bool) != Some(true) {
        return Some("页面无可滚动区域,内容一屏可见,无需滚动".into());
    }
    if v.get("at_bottom").and_then(Value::as_bool) == Some(true) {
        return Some("已停在最新位置,人工可见全部最新内容".into());
    }
    let below = v.get("below_fold_px").and_then(Value::as_i64).unwrap_or(0);
    Some(format!(
        "最新内容在视口下方约 {below}px 处,人工在 locked 档无法自行滚动(CDP 输入拦截 \
         连滚轮一并拦)。请先 control(scroll_to, params={{position:'bottom'}}) 或 \
         control(auto_follow, params={{enabled:true}}) 把页面带到最新,再截图/再让用户确认"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_only_when_user_already_at_bottom() {
        assert!(should_follow(true, true));
        assert!(!should_follow(true, false), "人工在翻历史时不得拽回底部");
        assert!(!should_follow(false, true), "关闭时一律不跟随");
        assert!(!should_follow(false, false));
    }

    #[test]
    fn visibility_note_cases() {
        // 未成功诊断 → 不给提示(fail-open,零噪声)
        assert_eq!(visibility_note(&json!({"ok": false})), None);
        // 不可滚动 → 明确「无需滚动」
        assert!(visibility_note(&json!({"ok": true, "scrollable": false}))
            .unwrap()
            .contains("无需滚动"));
        // 已在底部 → 明确「人工可见」
        assert!(visibility_note(&json!({"ok": true, "scrollable": true, "at_bottom": true}))
            .unwrap()
            .contains("已停在最新"));
        // 有隐藏内容 → 必须给出 Agent 可执行动作,且不得让用户自己滚
        let n = visibility_note(&json!({
            "ok": true, "scrollable": true, "at_bottom": false, "below_fold_px": 2145
        }))
        .unwrap();
        assert!(n.contains("2145"));
        assert!(n.contains("scroll_to"));
        assert!(n.contains("auto_follow"));
    }

    #[test]
    fn js_anchors_locked() {
        // 跟随器幂等重挂 + 状态查询 + 容差常量
        assert!(AUTO_FOLLOW_JS.contains("__laewAutoFollow"));
        assert!(AUTO_FOLLOW_JS.contains("__laewAutoFollowStatus"));
        assert!(AUTO_FOLLOW_JS.contains("const TOL = 96"));
        assert!(AUTO_FOLLOW_JS.contains("atBottom(el)"));
        assert!(AUTO_FOLLOW_JS.contains("el.scrollTop = el.scrollHeight"));
        // 诊断字段
        for k in [
            "below_fold_px",
            "hidden_ratio",
            "at_bottom",
            "scrollable",
            "selector",
        ] {
            assert!(VISIBILITY_JS.contains(k), "VISIBILITY_JS 缺字段 {k}");
        }
        // 纯 ASCII 正则转义正确(不能是 \\w)
        assert!(VISIBILITY_JS.contains(r"/^[A-Za-z_][\w-]*$/"));
        assert!(VISIBILITY_JS.contains(r"replace(/([^\w-])/g, '\\$1')"));
    }
}
