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

/// 该 control_action 完成后是否自动探测人工阻断(第 147 轮;第 151 轮补 eval_js)。
///
/// - 输入类动作(复用 [`crate::agent::browser_overlay::action_dispatches_input` 白名单]):
///   验证弹窗**出现后持续存在**,循环里下一个输入动作必然探测命中;
/// - `wait`:等待结束(含超时失败)恰是「页面该响应而没响应」的检查点 —— 验证弹窗
///   挡住回复时 wait 超时 + 探测命中是最强复合信号;
/// - 导航类:登录墙/验证墙常在导航落地页出现;
/// - `eval_js`(第 151 轮):实测 LLM 在 input_text 路径碰壁后会把**全部输入行为迁移到
///   eval_js**(execCommand insertText + 合成 KeyboardEvent Enter + 自己 btn.click()),
///   「只读通道」假设不成立;且 await_promise 长轮询(45~55s)恰是「发出消息 → 风控
///   弹窗」的时序窗口,eval 返回时刻就是最该探测的检查点。成本 = 一次 eval 往返
///   (健康页面 ~10-50ms,fail-open 2s 超时语义不变)。
/// 观察类与纯状态设置类(screenshot/set_*)仍不探测。
pub(super) fn action_needs_probe(action: &str) -> bool {
    crate::agent::browser_overlay::action_dispatches_input(action)
        || matches!(
            action,
            "wait" | "navigate" | "back" | "forward" | "reload" | "eval_js"
        )
}

/// eval_js 表达式是否可能变更页面状态(第 151 轮,纯函数可单测)。
///
/// 供输入硬闸(`challenge_gate`)判定「可变更 eval」是否需要执行前探针 —— 真实事故中
/// LLM 正是用这些 API 对着验证弹窗继续发消息。标记词按小写包含匹配。
const EVAL_MUTATION_MARKERS: &[&str] = &[
    "execcommand",
    "inserttext",
    "dispatchevent",
    "keyboardevent",
    "mouseevent",
    ".click(",
    ".focus(",
    ".submit(",
    "requestsubmit(",
    "location.href",
    "location.assign",
    "location.replace",
    // 第 152 轮补:原表只认 `location.href` 一种赋值形态,`location = url` /
    // `window.location = url` / `self.location=` / `top.location=` 全部漏网,
    // 而这几形态与 `location.href` 的副作用完全等价(都是离开当前页)。
    "location=",
    "location =",
    "window.location",
    "self.location",
    "top.location",
    "parent.location",
    // 第 152 轮补:单页应用路由跳转 / Worker 动态导入同样改变页面状态。
    "pushstate(",
    "replacestate(",
    "window.open",
    "document.write",
];
pub(super) fn eval_expression_mutates(expr: &str) -> bool {
    let lower = expr.to_lowercase();
    EVAL_MUTATION_MARKERS.iter().any(|m| lower.contains(m))
}

// ===================== 弹层语义挑战检测(纯函数,可单测) =====================

/// 弹层文本强验证词:任一命中 +4(单独即过阈值)。
const CHALLENGE_STRONG_WORDS: &[&str] = &[
    "验证码", "人机验证", "安全验证", "身份验证", "滑动验证", "拖动滑块",
    "请完成验证", "完成下方验证", "完成以下验证", "真人", "robot",
    "human verification", "captcha",
];
/// 弹层文本弱指令词:合计命中 +1(图片选择/语义题的题面动词,需与结构信号叠加)。
/// 第 151 轮补「翻转」「找出」「图标」(点选翻转/找出类语义题的常见题面动词/名词)。
const CHALLENGE_WEAK_WORDS: &[&str] = &[
    "点击", "选择", "选出", "按顺序", "依次", "包含", "拖动", "拖拽", "滑动", "拼图",
    "图片", "图中", "下方图片", "符合", "哪一个", "哪个", "翻转", "找出", "图标",
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

/// 人工核验挑战的子类(第 152 轮)。
///
/// 第 151 轮之前 [`detect_challenge`] 对任何形态一律写死 `"kind": "captcha"`,
/// 于是豆包的**拖拽拼图**被贴上「图形/滑块验证码」标签:弹窗标题错、人工按
/// 验证码思路去找输入框(根本没有)、`hitl_answer_hint` 还提示「立即 input_text
/// 填码」把人引向不存在的动作。子类判定让 reason / 文案 / 后续动作三处对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChallengeType {
    /// 拖拽拼图:把碎片拖进缺口 / 把图拖到指定位置
    DragPuzzle,
    /// 滑块验证:按住滑块拖到最右
    Slider,
    /// 九宫格点选:按题面点选符合条件的图片
    ImageSelect,
    /// 文字验证码:输入框里填图形/文字码
    TextCaptcha,
    /// 跨域 iframe 内嵌的第三方验证码(geetest / 极验 / 顶象…)
    EmbeddedCaptcha,
    /// 结构判定为挑战但形态不明
    Unknown,
}

impl ChallengeType {
    /// 稳定标识(进 JSON,便于日志对账与单测锁死)。
    pub(super) fn as_str(self) -> &'static str {
        match self {
            ChallengeType::DragPuzzle => "drag_puzzle",
            ChallengeType::Slider => "slider",
            ChallengeType::ImageSelect => "image_select",
            ChallengeType::TextCaptcha => "text_captcha",
            ChallengeType::EmbeddedCaptcha => "embedded_captcha",
            ChallengeType::Unknown => "unknown",
        }
    }

    /// 人工完成该挑战所需的**交互方式**(进 HITL 弹窗文案,直接告诉人工怎么操作)。
    pub(super) fn interaction(self) -> &'static str {
        match self {
            ChallengeType::DragPuzzle => "鼠标按住图片拖到指定位置",
            ChallengeType::Slider => "按住滑块一路拖到最右",
            ChallengeType::ImageSelect => "按题面点选符合条件的图片",
            ChallengeType::TextCaptcha => "读出图片里的字符并在输入框填入",
            ChallengeType::EmbeddedCaptcha => "在页面内嵌的验证框中完成(第三方验证码)",
            ChallengeType::Unknown => "在浏览器窗口内完成页面上的验证",
        }
    }

    /// 映射到 `request_human` 的 reason。
    ///
    /// 只有**真能填码**的形态才叫 `captcha`(会触发自动截图 + 「立即 input_text
    /// 填码」的 next_hint);拖拽/滑块/点选一律 `manual_verify`(「人工核验」),
    /// 标签与文案不再误导。
    pub(super) fn reason(self) -> &'static str {
        match self {
            ChallengeType::TextCaptcha | ChallengeType::EmbeddedCaptcha => "captcha",
            _ => "manual_verify",
        }
    }

    /// 该子类是否需要**人工在浏览器窗口里动手**(false = 只需读码/输码)。
    /// 决定 HITL 弹窗该强调「去浏览器操作」还是「把码填进弹窗」。
    pub(super) fn needs_browser_action(self) -> bool {
        !matches!(self, ChallengeType::TextCaptcha)
    }
}

impl ChallengeType {
    /// 由 `as_str` 反查(供工具层从 JSON 回读,避免重复匹配表)。
    pub(super) fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "drag_puzzle" => ChallengeType::DragPuzzle,
            "slider" => ChallengeType::Slider,
            "image_select" => ChallengeType::ImageSelect,
            "text_captcha" => ChallengeType::TextCaptcha,
            "embedded_captcha" => ChallengeType::EmbeddedCaptcha,
            "unknown" => ChallengeType::Unknown,
            _ => return None,
        })
    }
}

/// 拖拽类词(与点选/滑块/填码区分开)。
const CHALLENGE_DRAG_WORDS: &[&str] = &["拖动", "拖拽", "拖到", "拖至", "拼图", "拼合", "移入", "移动到"];
/// 滑块类词。
const CHALLENGE_SLIDER_WORDS: &[&str] = &["滑块", "滑动", "拖动滑块", "向右", "最右", "最右侧"];

/// 按结构 + 文案判定挑战子类(纯函数,可单测)。
///
/// 判据优先级(从「最具体的形态」往回退):
/// 1. 文字码:弹层内有可见输入框 + 命中强验证词 → 唯一能填码的形态;
/// 2. 跨域 iframe:验证码尺寸 iframe → 主文档读不到内部结构,只能按 iframe 判;
/// 3. 滑块:探针采到的窄长条「滑杆」元素(宽 30~120px、高 20~90px)+ 滑块类文案;
/// 4. 拖拽拼图:canvas + 拖拽类文案(豆包实测形态:主文档 canvas 上渲染);
/// 5. 点选:九宫格 / CSS 背景图网格 ≥4;
/// 6. 其余 → Unknown。
pub(super) fn classify_challenge(overlay: &Value) -> ChallengeType {
    let text = overlay.get("text").and_then(Value::as_str).unwrap_or("");
    let buttons = overlay
        .get("buttons")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let hay = format!("{text} {buttons}");
    let grid = overlay.get("grid_imgs").and_then(Value::as_i64).unwrap_or(0)
        + overlay.get("bg_imgs").and_then(Value::as_i64).unwrap_or(0);
    let canvas = overlay.get("canvas_count").and_then(Value::as_i64).unwrap_or(0);
    let iframe_like = overlay.get("iframe_like").and_then(Value::as_bool) == Some(true);
    let inputs = overlay.get("input_count").and_then(Value::as_i64).unwrap_or(0);
    let slider_like = overlay.get("slider_like").and_then(Value::as_bool) == Some(true);
    let strong = contains_any_lower(&hay, CHALLENGE_STRONG_WORDS);
    let drag_word = contains_any_lower(&hay, CHALLENGE_DRAG_WORDS);
    let slider_word = contains_any_lower(&hay, CHALLENGE_SLIDER_WORDS);

    if inputs >= 1 && strong {
        return ChallengeType::TextCaptcha;
    }
    if iframe_like && canvas == 0 && grid < 4 && !(slider_like || drag_word) {
        return ChallengeType::EmbeddedCaptcha;
    }
    if slider_like || (slider_word && canvas == 0 && grid < 4) {
        return ChallengeType::Slider;
    }
    if canvas >= 1 && drag_word {
        return ChallengeType::DragPuzzle;
    }
    if canvas >= 1 && !slider_word && grid < 4 && !drag_word {
        // 无文案的 canvas 挑战:九成是滑块/拖拽,归 Unknown 而非 captcha 更安全
        return ChallengeType::Unknown;
    }
    if grid >= 4 {
        return ChallengeType::ImageSelect;
    }
    ChallengeType::Unknown
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
    // 第 151 轮:CSS 背景图九宫格信号 —— 部分挑战格不用 <img> 而用 background-image
    // 渲染(实测豆包风控组件形态之一),只数 img 会漏检。
    let bg_imgs = overlay.get("bg_imgs").and_then(Value::as_i64).unwrap_or(0);
    let iframe_like = overlay.get("iframe_like").and_then(Value::as_bool) == Some(true);

    let mut score: i32 = 0;
    if grid_imgs + bg_imgs >= 4 {
        score += 2;
    }
    // 第 150 轮:canvas 结构信号(拖拽拼图/滑块类验证常以 canvas 渲染在主文档,
    // 既无九宫格 img 也无验证码尺寸 iframe,旧打分制漏检豆包「拖拽图片到框中」)。
    let canvas_count = overlay.get("canvas_count").and_then(Value::as_i64).unwrap_or(0);
    if canvas_count >= 1 {
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
    // 第 152 轮:子类决定 kind —— 拖拽/滑块/点选不再被叫「验证码」。
    let ctype = classify_challenge(overlay);
    Some(json!({
        "kind": ctype.reason(),
        "matched": "challenge_overlay",
        "snippet": snippet,
        "challenge_type": ctype.as_str(),
        "interaction": ctype.interaction(),
        "needs_browser_action": ctype.needs_browser_action(),
        "evidence": {
            "score": score,
            "grid_imgs": grid_imgs,
            "bg_imgs": bg_imgs,
            "canvas_count": canvas_count,
            "iframe_like": iframe_like,
            "slider_like": overlay.get("slider_like").cloned().unwrap_or(Value::Null),
            "input_count": overlay.get("input_count").and_then(Value::as_i64).unwrap_or(0),
            "challenge_selector": overlay.get("selector").cloned().unwrap_or(Value::Null),
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
    // 词表候选:命中 modal/dialog/captcha/verify 等**语义类名**的模态。
    const SEL = 'dialog[open], [role="dialog"], [aria-modal="true"], [class*="modal" i], [class*="dialog" i], [class*="popup" i], [class*="mask" i], [class*="overlay" i], [class*="captcha" i], [class*="verify" i], [class*="challenge" i]';
    // 第 152 轮:通用候选 —— 现代前端大量使用 BEM / 哈希类名(如 `.css-1x2y3z`),
    // 词表一条都命中不了,整层弹窗直接漏检。这里补一条「不看类名、只看几何与
    // 层叠」的兜底通道:凡是 fixed/absolute 且面积够大的容器都进候选池,再走
    // 与词表候选完全相同的可见性/面积/层级筛选。为控制开销,只取前 600 个。
    const SEL_GENERIC = 'body div, body section, body main, body aside, body form';
    let best = null;
    const consider = (el) => {
      if (!el || el.closest('[data-laew-agent]')) return;
      const cs = window.getComputedStyle(el);
      if (cs.display === 'none' || cs.visibility === 'hidden' || parseFloat(cs.opacity) < 0.05) return;
      if (cs.position !== 'fixed' && cs.position !== 'absolute') return;
      const r = el.getBoundingClientRect();
      if (r.width < 200 || r.height < 120) return;
      if ((r.width * r.height) / (vw * vh) < 0.06) return;
      const z = parseInt(cs.zIndex, 10) || 0;
      let depth = 0, n = el;
      while (n.parentElement) { depth++; n = n.parentElement; }
      if (!best || z > best.z || (z === best.z && depth > best.depth)) best = {el, z, depth};
    };
    document.querySelectorAll(SEL).forEach(consider);
    if (!best) {
      let seen = 0;
      for (const el of document.querySelectorAll(SEL_GENERIC)) {
        if (++seen > 600) break;
        consider(el);
      }
    }
    if (!best) return {text: text, overlay: null};
    const el = best.el, r = el.getBoundingClientRect();
    // 第 152 轮:挑战层选择器(供 HITL 放行白名单与「滚动到验证弹窗」复用)
    const selOf = (node) => {
      const esc = (s) => (window.CSS && CSS.escape) ? CSS.escape(s) : String(s).replace(/([^\w-])/g, '\\$1');
      if (node.id) return '#' + esc(node.id);
      const cls = (node.getAttribute('class') || '').trim().split(/\s+/).filter(Boolean)
        .filter(c => /^[A-Za-z_][\w-]*$/.test(c)).slice(0, 2);
      if (cls.length) {
        const s = cls.map(c => '.' + esc(c)).join('');
        try { if (document.querySelectorAll(s).length === 1) return s; } catch (e) {}
      }
      const parts = [];
      let cur = node;
      for (let d = 0; cur && cur.nodeType === 1 && d < 4; d++) {
        let part = cur.tagName.toLowerCase();
        if (cur.id) { parts.unshift('#' + esc(cur.id)); break; }
        const parent = cur.parentElement;
        if (parent) {
          const same = Array.prototype.filter.call(parent.children, (c) => c.tagName === cur.tagName);
          if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(cur) + 1) + ')';
        }
        parts.unshift(part);
        cur = parent;
      }
      const s = parts.join(' > ');
      try { return document.querySelectorAll(s).length >= 1 ? s.slice(0, 150) : ''; } catch (e) { return ''; }
    };
    let img_count = 0, grid_imgs = 0, canvas_count = 0, iframe_count = 0,
        iframe_like = false, input_count = 0, bg_imgs = 0;
    for (const img of el.querySelectorAll('img')) {
      const ir = img.getBoundingClientRect();
      if (ir.width <= 0 || ir.height <= 0) continue;
      img_count++;
      const ratio = ir.width / ir.height;
      if (ir.width >= 48 && ir.width <= 240 && ratio >= 0.5 && ratio <= 1.6) grid_imgs++;
    }
    // 第 151 轮:CSS 背景图渲染的挑战格(background-image 而非 <img>),按九宫格同尺寸判据计数
    for (const node of el.querySelectorAll('*')) {
      if (bg_imgs >= 36) break;
      const nb = node.getBoundingClientRect();
      if (nb.width <= 0 || nb.height <= 0) continue;
      if (nb.width > 240) continue;
      const bs = window.getComputedStyle(node);
      if (!bs.backgroundImage || bs.backgroundImage === 'none') continue;
      const ratio = nb.width / nb.height;
      if (nb.width >= 48 && ratio >= 0.5 && ratio <= 1.6) bg_imgs++;
    }
    canvas_count = el.querySelectorAll('canvas').length;
    input_count = el.querySelectorAll('input, textarea').length;
    // 第 152 轮:滑杆信号 —— 滑块验证的把手是「窄长条」:宽 30~120px、高 20~90px。
    // 只看文案会漏掉无文案的滑块(豆包部分风控组件只有图形提示)。
    let slider_like = false;
    for (const node of el.querySelectorAll('div,span,button,i')) {
      const nb = node.getBoundingClientRect();
      if (nb.width < 30 || nb.width > 120 || nb.height < 20 || nb.height > 90) continue;
      const sb = window.getComputedStyle(node);
      if (sb.display === 'none' || sb.visibility === 'hidden') continue;
      const bg = sb.backgroundColor + ' ' + sb.backgroundImage + ' ' + sb.borderRadius;
      if (bg.includes('gradient') || bg.includes('url(')) { slider_like = true; break; }
    }
    // 第 152 轮:跨域 iframe 内部读不到(不同源),把 iframe 自身的 class/id/src/
    // title 拼进文本参与关键词判定 —— 第三方验证码组件的 src/name 几乎必然
    // 带 captcha/geetest/verify 之类标识,这是主文档侧唯一能拿到的强信号。
    let frame_meta = '';
    for (const f of el.querySelectorAll('iframe')) {
      const fr = f.getBoundingClientRect();
      if (fr.width <= 0 || fr.height <= 0) continue;
      iframe_count++;
      frame_meta += ' ' + (f.getAttribute('src') || '') + ' ' + (f.getAttribute('title') || '')
        + ' ' + (f.getAttribute('class') || '') + ' ' + (f.getAttribute('id') || '')
        + ' ' + (f.getAttribute('name') || '');
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
        text: ((el.innerText || '') + frame_meta).replace(/\s+/g, ' ').slice(0, 600),
        img_count: img_count, grid_imgs: grid_imgs, canvas_count: canvas_count,
        iframe_count: iframe_count, iframe_like: iframe_like, input_count: input_count,
        bg_imgs: bg_imgs,
        slider_like: slider_like,
        selector: selOf(el),
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

/// 把人工核验挑战弹层滚动到视口中央,返回命中的选择器(无挑战层 → `None`)。
///
/// 实测事故:验证弹窗出现在页面**下半屏或视口之外**时,HITL 弹窗里那张自动附图
/// 截到的是空白/无关区域,人工看不出「到底要我做什么」;而页面本身又被
/// locked 档的 `Input.setIgnoreInputEvents` 拦着滚轮,人工想自己滚过去也不行。
/// 提问前先把挑战层滚进视野,是让附图与页面同时「说人话」的最短路径。
///
/// fail-open:探针超时 / 无挑战层 / JS 异常一律 `None`,绝不阻断提问。
pub(super) async fn scroll_challenge_into_view(page: &chromiumoxide::Page) -> Option<String> {
    let probe = collect_for_inspect(page).await;
    let overlay = probe.get("overlay")?;
    if overlay.get("found").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let selector = overlay
        .get("selector")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?
        .to_string();
    let js = format!(
        "(function(){{ try {{ var el = document.querySelector({sel}); \
         if (!el) return false; el.scrollIntoView({{block:'center', inline:'nearest'}}); \
         return true; }} catch (e) {{ return false; }} }})()",
        sel = serde_json::to_string(&selector).unwrap_or_else(|_| "\"\"".into())
    );
    match eval_js_string(page, &js).await {
        Ok(v) if v.as_bool() == Some(true) => Some(selector),
        _ => None,
    }
}

// ===================== 响应附加(推送式) =====================

// ===================== 同指纹告警去重(第 152 轮) =====================

/// 进程内告警指纹表:`page_id -> (指纹, 连续命中次数)`。
///
/// 实测事故(2026-10-10 豆包):同一个误报(`2fa` 命中 CDN 图片 URL)在 iter
/// 7/9/11/12/14/15/17 连续播报 7 次,每次都拖一份 1~2 万 token 的
/// `human_assist` 载荷进上下文 —— 「狼来了」把 `next_action=request_human`
/// 训练成了噪声,既烧 token 又钝化模型的真实响应。
///
/// 去重口径:**只压重复的载荷,不压重复的事实**。同一指纹再次命中时仍返回
/// `blocker_alert`(带 `repeat`/`repeat_count`),只是不再重复 `human_assist`
/// 与 `next_action` —— Agent 已经知道该怎么做了,不必每次重读一遍长文案。
///
/// 硬闸 [`challenge_gate`] **不走**本表:拦截语义要持续生效,去重只针对播报。
fn fingerprint_store() -> &'static std::sync::Mutex<std::collections::HashMap<String, (String, u32)>> {
    static STORE: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, (String, u32)>>,
    > = std::sync::LazyLock::new(|| {
        std::sync::Mutex::new(std::collections::HashMap::new())
    });
    &STORE
}

/// 告警指纹(纯函数,可单测):`(kind, matched, snippet 前 40 字)`。
///
/// 只取稳定成分 —— 弹层每轮渲染出的**按钮文案/时间戳**会变,若纳入指纹则
/// 每次都算「新告警」,去重形同虚设(实测:豆包验证弹窗的按钮与倒计时逐轮变化)。
pub(super) fn fingerprint_of(alert: &Value) -> String {
    let kind = alert.get("kind").and_then(Value::as_str).unwrap_or("");
    let matched = alert.get("matched").and_then(Value::as_str).unwrap_or("");
    let snippet: String = alert
        .get("snippet")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(40)
        .collect();
    format!("{kind}|{matched}|{snippet}")
}

/// 登记一次告警指纹,返回是否为**重复**(纯逻辑收在闭包里便于单测)。
fn register_fingerprint(page_id: &str, fp: &str) -> Option<u32> {
    let mut guard = fingerprint_store().lock().unwrap_or_else(|e| e.into_inner());
    let entry = guard.entry(page_id.to_string()).or_insert_with(|| (fp.to_string(), 0));
    if entry.0 == fp {
        entry.1 += 1;
        Some(entry.1)
    } else {
        // 指纹变了(弹窗换了题面/换了形态)→ 重新计数
        *entry = (fp.to_string(), 1);
        None
    }
}

/// 清除某页的指纹记忆(人工介入发起 / 页面关闭 / guard 切换时调用)。
///
/// 人工介入是「页面状态已被人改变」的新事实 —— 之后出现的阻断应当重新完整播报,
/// 否则会出现「弹窗还在,但 Agent 因为上次已告警过而不再提醒」的漏报。
pub(super) fn forget_fingerprints(page_id: &str) {
    fingerprint_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(page_id);
}

/// blocker_alert 的人类可读处置提示。
const ALERT_HINT: &str = "页面弹出人工验证挑战(拖拽拼图/滑块/图片点选/文字码等,无法自动完成);\
立即停止继续输入或重试;按 data.challenge_type 选 reason 发起 request_human 让人工完成验证\
(拖拽/滑块/点选用 manual_verify,文字码用 captcha),\
不确定弹窗内容时可先 control(screenshot, params.ocr=true) 确认";

/// 挑战类 HITL 的选项文案(P11 对齐:与 `act_request_human` 的缺省 options
/// 保持一致,避免同一个「继续」在两处叫不同名字)。
pub(super) const HITL_CHALLENGE_OPTIONS: [&str; 2] = ["我已完成人工操作,继续", "取消任务"];

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
    // 第 152 轮:同类告警连续命中时只回事实、不重复长载荷(见 fingerprint_store 注释)
    let repeat_count = register_fingerprint(page_id, &fingerprint_of(first));
    let mut alert = first.clone();
    if let (Some(obj), Some(n)) = (alert.as_object_mut(), repeat_count) {
        obj.insert("hint".into(), json!(ALERT_HINT));
        obj.insert("repeat".into(), json!(true));
        obj.insert("repeat_count".into(), json!(n));
    } else if let Some(obj) = alert.as_object_mut() {
        obj.insert("hint".into(), json!(ALERT_HINT));
    }
    // 第 151 轮:message 带题面 snippet(≤60 字)—— 人工在 HITL 弹窗直接看到
    // 「要验证什么」,不用猜;空 snippet(关键词命中无上下文)回退通用文案。
    let snippet: String = alert
        .get("snippet")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(60)
        .collect();
    let message = if snippet.is_empty() {
        "页面出现人工验证挑战(见 data.blocker_alert),请人工完成验证后继续".to_string()
    } else {
        format!("页面弹出人工验证挑战「{snippet}」;请在浏览器窗口完成验证,完成后回弹窗点「提交 / 继续」(也可在输入框输入文字与 Agent 交互)")
    };
    // 第 152 轮:重复告警不再重复长载荷 —— Agent 已经知道该干什么,只回事实
    if repeat_count.is_some() {
        return Some(json!({"blocker_alert": alert}));
    }
    let hint = super::human_assist_hint(&kind, &message, &HITL_CHALLENGE_OPTIONS);
    let mut out = json!({
        "blocker_alert": alert,
        "next_action": hint.get("next_action").cloned().unwrap_or(json!("request_human")),
        "human_assist": hint.get("human_assist").cloned().unwrap_or(Value::Null),
    });
    // 第 152 轮:子类信息顶层也带一份,Agent 无需解析 evidence 即可决定动作
    if let (Some(dst), Some(src)) = (out.as_object_mut(), first.as_object()) {
        for k in ["challenge_type", "interaction", "needs_browser_action"] {
            if let Some(v) = src.get(k) {
                dst.insert(k.to_string(), v.clone());
            }
        }
    }
    Some(out)
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

// ===================== 输入前置硬闸(第 151 轮) =====================

/// 输入前置硬闸接线(control::run 在 guard_lift 之前调用,条件判断收在本模块
/// —— control.rs 已处 1800 行临界,新增代码按约定落职责子模块)。
///
/// 覆盖动作面:输入类 20 动作 + **可变更 eval_js**(实测 LLM 在 input_text 碰壁后
/// 把输入行为迁移到 execCommand insertText / 合成 KeyboardEvent 绕过探针);
/// `params.force=true` 逃生口跳过本次硬闸。`Some` = 6002 拦截信封(动作不执行)。
pub(super) async fn input_gate(
    page_id: &str,
    action: &str,
    params: &Value,
) -> Option<crate::error::Result<String>> {
    if !enabled()
        || params.get("force").and_then(Value::as_bool).unwrap_or(false)
        || !(crate::agent::browser_overlay::action_dispatches_input(action)
            || (action == "eval_js"
                && params
                    .get("expression")
                    .and_then(Value::as_str)
                    .map(eval_expression_mutates)
                    .unwrap_or(false)))
    {
        return None;
    }
    challenge_gate(page_id).await
}

/// 硬闸拦截的人类可读处置提示(附在 blocker_alert.hint)。
const GATE_HINT: &str = "输入动作已拦截:页面存在结构判定的人工验证挑战弹窗(图片选择/滑块/\
拖拽/语义题),继续输入只会把消息灌进弹窗后面的黑洞;按 human_assist 载荷发起 \
request_human(reason=captcha) 让人工完成验证,应答后再继续;确认是误判时可加 \
params.force=true 强制执行本动作";

/// 输入前置硬闸:输入类动作 / 可变更 eval_js 执行**前**探测**结构判定**的验证挑战。
///
/// 与推送式告警(`probe_alert`)的两点关键差异:
/// 1. **只认 [`detect_challenge`] 的弹层结构证据,不认关键词通道** —— 关键词命中
///    (如页面常驻的「请登录」文案)可能长期在场,作硬闸会把输入永久锁死;
/// 2. **命中即不执行动作**,返回 6002 信封(附 blocker_alert + human_assist 载荷 +
///    force 逃生口说明),从工具层根治「LLM 无视 next_action 继续对着弹窗输入」
///    (实测事故:Agent 对着豆包验证弹窗连发 2 条消息零送达)。
/// 干净/开关关闭/探针失败一律 None(照常执行,fail-open 不误伤)。
pub(super) async fn challenge_gate(
    page_id: &str,
) -> Option<crate::error::Result<String>> {
    if !enabled() {
        return None;
    }
    let page = BrowserManager::global().page(page_id).await?;
    let probe = collect_for_inspect(&page).await;
    let challenge = detect_challenge(probe.get("overlay").unwrap_or(&Value::Null))?;
    let snippet: String = challenge
        .get("snippet")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(60)
        .collect();
    // 第 152 轮:子类推出的 reason(拖拽 → manual_verify),不再一律 captcha
    let ctype = challenge
        .get("challenge_type")
        .and_then(Value::as_str)
        .and_then(ChallengeType::from_str)
        .unwrap_or(ChallengeType::Unknown);
    let mut alert = challenge;
    if let Some(obj) = alert.as_object_mut() {
        obj.insert("hint".into(), json!(GATE_HINT));
    }
    let message = if snippet.is_empty() {
        "页面弹出人工验证挑战,输入动作已被拦截;请在浏览器窗口完成验证".to_string()
    } else {
        format!("页面弹出人工验证挑战「{snippet}」,输入动作已被拦截;请人工在浏览器窗口完成验证,完成后回弹窗点「提交 / 继续」")
    };
    let hint = super::human_assist_hint(ctype.reason(), &message, &HITL_CHALLENGE_OPTIONS);
    let mut data = json!({
        "blocker_alert": alert,
        "force_hint": "确认页面无真实验证弹窗(结构误判)时,可在 params 加 force=true 跳过硬闸强制执行",
    });
    if let (Some(dst), Some(src)) = (data.as_object_mut(), hint.as_object()) {
        for (k, v) in src {
            dst.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    Some(super::envelope(
        super::CODE_BLOCKER_INPUT_GATE,
        "页面存在人工验证挑战,输入动作已拦截(见 data.blocker_alert;发起 request_human 让人工完成,误判可 params.force=true)",
        data,
    ))
}
