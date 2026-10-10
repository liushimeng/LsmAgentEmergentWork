//! 自动阻断感知(第 147 轮):推送式 blocker 探测 + 弹层语义挑战检测。
//!
//! 实测事故(2026-10-10 豆包任务,`llaew_20261010_142603.log`):豆包网页中途弹出
//! **图片选择类人机验证**「在公园能看到的事物或动物」,题面没有「验证码」等任何关键词,
//! [`super::inspect`] 的关键词表扫不出来;且 blockers 是拉取式 inspect,Agent 只在
//! 流程开头探过一次,弹窗出现后 20+ 轮循环发消息零感知,直到用户 Ctrl-C 手动终止。
//!
//! 两条互补通道:
//! 1. **弹层语义挑战检测**(`detect_challenge`):纯关键词失配的图片选择题/滑块/语义题,
//!    靠弹层结构识别 —— 高 z-index 模态覆盖层 + 九宫格小图 / 验证码尺寸 iframe /
//!    验证类文案,打分制判定(Rust 纯函数可单测);
//! 2. **推送式自动感知**(`attach_if_blocked`):输入类动作 / wait / 导航类动作完成后
//!    工具层自动跑一次探针,命中才把 `data.blocker_alert` + `human_assist` 载荷附进
//!    响应(干净零噪声,验证弹窗出现后持续存在,循环里下一个动作必然探测命中)。
//!
//! 杀开关 `LAEW_WEB_BLOCKER_PROBE=off/0/false/no`:自动探测关闭,`inspect(blockers)`
//! 回退第 100 轮纯关键词行为(严格向后兼容)。

use serde_json::{json, Value};

use super::*;

// ===================== 开关与动作集合 =====================

/// 自动阻断感知总开关(默认开;`LAEW_WEB_BLOCKER_PROBE=off` 系值关闭)。
pub(super) fn enabled() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_BLOCKER_PROBE").ok().as_deref(),
        Some("off") | Some("0") | Some("false") | Some("no")
    )
}

/// 该 control_action 完成后是否自动探测人工阻断(第 147 轮)。
///
/// - 输入类动作(复用 [`crate::agent::browser_overlay::action_dispatches_input` 白名单]):
///   验证弹窗**出现后持续存在**,循环里下一个输入动作必然探测命中;
/// - `wait`:等待结束(含超时失败)恰是「页面该响应而没响应」的检查点 —— 验证弹窗
///   挡住回复时 wait 超时 + 探测命中是最强复合信号;
/// - 导航类:登录墙/验证墙常在导航落地页出现。
/// 观察类与纯状态设置类(screenshot/eval_js/set_*)不探测 —— eval_js 是高频只读通道,
/// 逐次探测只会翻倍 CDP 往返而不增信号。
pub(super) fn action_needs_probe(action: &str) -> bool {
    crate::agent::browser_overlay::action_dispatches_input(action)
        || matches!(action, "wait" | "navigate" | "back" | "forward" | "reload")
}

// ===================== 弹层语义挑战检测(纯函数,可单测) =====================

/// 弹层文本强验证词:任一命中 +4(单独即过阈值)。
const CHALLENGE_STRONG_WORDS: &[&str] = &[
    "验证码", "人机验证", "安全验证", "身份验证", "滑动验证", "拖动滑块",
    "请完成验证", "完成下方验证", "完成以下验证", "真人", "robot",
    "human verification", "captcha",
];
/// 弹层文本弱指令词:合计命中 +1(图片选择/语义题的题面动词,需与结构信号叠加)。
const CHALLENGE_WEAK_WORDS: &[&str] = &[
    "点击", "选择", "选出", "按顺序", "依次", "包含", "拖动", "滑动", "拼图",
    "图片", "图中", "下方图片", "符合", "哪一个", "哪个",
];
/// 刷新类按钮/文案词:合计命中 +1(验证码组件的标配控件)。
const CHALLENGE_REFRESH_WORDS: &[&str] = &["换一张", "刷新", "看不清", "click to refresh"];
/// 广告负信号:合计命中 −3(弹窗广告与验证弹窗结构同形,靠文案区分)。
const CHALLENGE_AD_WORDS: &[&str] = &["广告", "跳过", "skip", "sponsored", "advertisement"];
/// 判定阈值:≥3 分认为是语义挑战。
///
/// 设计权衡:iframe 形态(+2)与九宫格(+2)单独都不够 —— 防广告位/图库弹窗误报;
/// 强验证词(+4)单独即命中;**任一结构信号 + 任一文案信号**是最小可靠组合
/// (豆包题面在跨域 iframe 内主文档读不到,靠「验证码尺寸 iframe + 壳层验证文案」命中;
/// 九宫格渲染在主文档时靠「小图网格 + 选出/点击类动词」命中)。
const CHALLENGE_THRESHOLD: i32 = 3;

fn contains_any_lower(haystack: &str, words: &[&str]) -> bool {
    let lower = haystack.to_lowercase();
    words.iter().any(|w| lower.contains(&w.to_lowercase()))
}

/// 弹层元数据是否构成语义挑战;命中返回 blocker 条目(含结构证据)。
///
/// 入参是探针 JS 产出的 `overlay` 对象(见 [`PROBE_JS`]);`None` = 无弹层或分数不足。
pub(super) fn detect_challenge(overlay: &Value) -> Option<Value> {
    if overlay.get("found").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let text = overlay.get("text").and_then(Value::as_str).unwrap_or("");
    let buttons: Vec<&str> = overlay
        .get("buttons")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let buttons_text = buttons.join(" ");
    let grid_imgs = overlay.get("grid_imgs").and_then(Value::as_i64).unwrap_or(0);
    let iframe_like = overlay.get("iframe_like").and_then(Value::as_bool) == Some(true);

    let mut score: i32 = 0;
    if grid_imgs >= 4 {
        score += 2;
    }
    if iframe_like {
        score += 2;
    }
    if contains_any_lower(text, CHALLENGE_STRONG_WORDS) {
        score += 4;
    }
    if contains_any_lower(text, CHALLENGE_WEAK_WORDS) {
        score += 1;
    }
    if contains_any_lower(text, CHALLENGE_REFRESH_WORDS)
        || contains_any_lower(&buttons_text, CHALLENGE_REFRESH_WORDS)
    {
        score += 1;
    }
    if contains_any_lower(text, CHALLENGE_AD_WORDS)
        || contains_any_lower(&buttons_text, CHALLENGE_AD_WORDS)
    {
        score -= 3;
    }
    if score < CHALLENGE_THRESHOLD {
        return None;
    }
    let snippet: String = text.chars().take(120).collect();
    Some(json!({
        "kind": "captcha",
        "matched": "challenge_overlay",
        "snippet": snippet,
        "evidence": {
            "score": score,
            "grid_imgs": grid_imgs,
            "iframe_like": iframe_like,
            "buttons": buttons,
            "area_ratio": overlay.get("area_ratio").cloned().unwrap_or(Value::Null),
        },
    }))
}

/// 综合分析探针数据:全文关键词(第 100 轮语义,复用 [`super::inspect::detect_blockers`])
/// ∪ 弹层语义挑战;同 kind 去重,挑战条目优先(带结构证据)。
pub(super) fn analyze(probe: &Value) -> Vec<Value> {
    let mut out = super::inspect::detect_blockers(
        probe.get("text").and_then(Value::as_str).unwrap_or(""),
    );
    if let Some(challenge) = detect_challenge(probe.get("overlay").unwrap_or(&Value::Null)) {
        // 关键词通道可能已产出同 kind 条目(captcha),保留带 evidence 的挑战版本
        out.retain(|b| b.get("kind").and_then(Value::as_str) != Some("captcha"));
        out.insert(0, challenge);
    }
    out
}

// ===================== 探针 JS 与采集 =====================

/// 一次 eval 采集:全文(body 克隆剥离 `[data-laew-agent]`,Agent 蒙层/徽标不参与判定)
/// + 最高层弹层候选的结构元数据。字符串常量(不经 format!),花括号安全。
const PROBE_JS: &str = r#"(() => {
  try {
    if (!document.body) return {text: '', overlay: null};
    const clone = document.body.cloneNode(true);
    clone.querySelectorAll('[data-laew-agent]').forEach(e => e.remove());
    const text = (clone.innerText || '').slice(0, 20000);
    const vw = window.innerWidth, vh = window.innerHeight;
    const SEL = 'dialog[open], [role="dialog"], [aria-modal="true"], [class*="modal" i], [class*="dialog" i], [class*="popup" i], [class*="mask" i], [class*="overlay" i], [class*="captcha" i], [class*="verify" i], [class*="challenge" i]';
    let best = null;
    for (const el of document.querySelectorAll(SEL)) {
      if (el.closest('[data-laew-agent]')) continue;
      const cs = window.getComputedStyle(el);
      if (cs.display === 'none' || cs.visibility === 'hidden' || parseFloat(cs.opacity) < 0.05) continue;
      if (cs.position !== 'fixed' && cs.position !== 'absolute') continue;
      const r = el.getBoundingClientRect();
      if (r.width < 200 || r.height < 120) continue;
      if ((r.width * r.height) / (vw * vh) < 0.06) continue;
      const z = parseInt(cs.zIndex, 10) || 0;
      let depth = 0, n = el;
      while (n.parentElement) { depth++; n = n.parentElement; }
      if (!best || z > best.z || (z === best.z && depth > best.depth)) best = {el, z, depth};
    }
    if (!best) return {text: text, overlay: null};
    const el = best.el, r = el.getBoundingClientRect();
    let img_count = 0, grid_imgs = 0, canvas_count = 0, iframe_count = 0,
        iframe_like = false, input_count = 0;
    for (const img of el.querySelectorAll('img')) {
      const ir = img.getBoundingClientRect();
      if (ir.width <= 0 || ir.height <= 0) continue;
      img_count++;
      const ratio = ir.width / ir.height;
      if (ir.width >= 48 && ir.width <= 240 && ratio >= 0.5 && ratio <= 1.6) grid_imgs++;
    }
    canvas_count = el.querySelectorAll('canvas').length;
    input_count = el.querySelectorAll('input, textarea').length;
    for (const f of el.querySelectorAll('iframe')) {
      const fr = f.getBoundingClientRect();
      if (fr.width <= 0 || fr.height <= 0) continue;
      iframe_count++;
      const ratio = fr.width / fr.height;
      if (fr.width >= 160 && fr.width <= 480 && fr.height >= 120 && fr.height <= 640
          && ratio >= 0.5 && ratio <= 1.9) iframe_like = true;
    }
    const buttons = Array.from(el.querySelectorAll('button, [role="button"]'))
      .map(b => (b.innerText || '').replace(/\s+/g, ' ').trim().slice(0, 30))
      .filter(Boolean).slice(0, 6);
    return {
      text: text,
      overlay: {
        found: true,
        text: (el.innerText || '').replace(/\s+/g, ' ').slice(0, 600),
        img_count: img_count, grid_imgs: grid_imgs, canvas_count: canvas_count,
        iframe_count: iframe_count, iframe_like: iframe_like, input_count: input_count,
        buttons: buttons,
        rect: {w: Math.round(r.width), h: Math.round(r.height)},
        viewport: {w: vw, h: vh},
        area_ratio: Math.round(((r.width * r.height) / (vw * vh)) * 1000) / 1000,
        z_index: best.z,
      },
    };
  } catch (e) { return {text: '', overlay: null, error: String(e)}; }
})()"#;

/// 跑探针采集(fail-open:2s 超时 / 页面失效 / JS 异常一律返回空探针 = 干净)。
/// `inspect(info=blockers)` 与推送式探测(`probe_alert`)共用同一探针,判定口径一致。
pub(super) async fn collect_for_inspect(page: &chromiumoxide::Page) -> Value {
    match tokio::time::timeout(std::time::Duration::from_secs(2), eval_js_string(page, PROBE_JS))
        .await
    {
        Ok(Ok(v)) if v.is_object() => v,
        _ => json!({"text": "", "overlay": null}),
    }
}

// ===================== 响应附加(推送式) =====================

/// blocker_alert 的人类可读处置提示。
const ALERT_HINT: &str = "页面疑似弹出人工验证挑战(图片选择/语义题/滑块等,无法自动完成);\
立即停止继续输入或重试;按 human_assist 载荷发起 request_human(reason=captcha) 让人工完成验证,\
不确定弹窗内容时可先 control(screenshot, params.ocr=true) 确认";

/// 探测并生成告警载荷(命中才 Some):`{blocker_alert, next_action, human_assist}` 三键,
/// 供调用方并入响应 data(Ok 路走 [`attach_if_blocked`],wait 超时 Err 路由调用方手插)。
pub(super) async fn probe_alert(page_id: &str) -> Option<Value> {
    if !enabled() {
        return None;
    }
    let page = BrowserManager::global().page(page_id).await?;
    let probe = collect_for_inspect(&page).await;
    let alerts = analyze(&probe);
    let first = alerts.first()?;
    let kind = first
        .get("kind")
        .and_then(Value::as_str)
        .filter(|k| super::control::HUMAN_ASSIST_ALLOWED_REASONS.contains(k))
        .unwrap_or("manual_verify")
        .to_string();
    let mut alert = first.clone();
    if let Some(obj) = alert.as_object_mut() {
        obj.insert("hint".into(), json!(ALERT_HINT));
    }
    let hint = super::human_assist_hint(
        &kind,
        "页面出现人工验证挑战(见 data.blocker_alert),请人工完成验证后继续",
        &["我已完成验证,继续", "取消任务"],
    );
    Some(json!({
        "blocker_alert": alert,
        "next_action": hint.get("next_action").cloned().unwrap_or(json!("request_human")),
        "human_assist": hint.get("human_assist").cloned().unwrap_or(Value::Null),
    }))
}

/// 动作成功后把告警并入响应 data(干净时零改动;已有 next_action 不覆盖,`merge_hint` 惯例)。
pub(super) async fn attach_if_blocked(page_id: &str, data: &mut Value) {
    if let Some(alert) = probe_alert(page_id).await {
        if let (Some(dst), Some(src)) = (data.as_object_mut(), alert.as_object()) {
            for (k, v) in src {
                dst.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
    }
}
