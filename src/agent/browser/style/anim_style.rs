//! CSS 动画/过渡声明解析（第 57 轮）：`animation`/`transition` 简写与 longhand、
//! `@keyframes` 步进块、timing function。
//!
//! 只做「字符串 → `AnimDecl` 结构」的纯解析；时间轴插值与帧推进在
//! `browser/anim.rs` 动画引擎。设计见
//! `docs/MCP_Web_LsmAgentBrowser_Use/03-布局与渲染引擎/57-CSS过渡与关键帧动画帧同步方案.md`。

/// 缓动函数（`animation-timing-function` / `transition-timing-function`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Easing {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// `cubic-bezier(x1, y1, x2, y2)`。
    CubicBezier(f64, f64, f64, f64),
    /// `step-end`（近似为终点跳变）。
    StepEnd,
    /// `step-start`（近似为起点跳变）。
    StepStart,
}

impl Default for Easing {
    fn default() -> Self {
        // CSS 初始值 `ease`（过渡/动画共用）。
        Easing::Ease
    }
}

impl Easing {
    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim().to_ascii_lowercase();
        match v.as_str() {
            "linear" => Some(Easing::Linear),
            "ease" => Some(Easing::Ease),
            "ease-in" => Some(Easing::EaseIn),
            "ease-out" => Some(Easing::EaseOut),
            "ease-in-out" => Some(Easing::EaseInOut),
            "step-end" | "steps(1, end)" | "steps(1,end)" => Some(Easing::StepEnd),
            "step-start" | "steps(1, start)" | "steps(1,start)" => Some(Easing::StepStart),
            _ => {
                let rest = v.strip_prefix("cubic-bezier(")?;
                let rest = rest.trim_end_matches(')');
                let parts: Vec<f64> = rest
                    .split(',')
                    .filter_map(|p| p.trim().parse::<f64>().ok())
                    .collect();
                if parts.len() == 4 {
                    Some(Easing::CubicBezier(parts[0], parts[1], parts[2], parts[3]))
                } else {
                    None
                }
            }
        }
    }

    /// 求映射值：输入进度 `t`（0..1），输出补间进度（可越界，弹簧式 y 控制点允许）。
    pub fn eval(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::Ease => cubic_bez_eval(0.25, 0.1, 0.25, 1.0, t),
            Easing::EaseIn => cubic_bez_eval(0.42, 0.0, 1.0, 1.0, t),
            Easing::EaseOut => cubic_bez_eval(0.0, 0.0, 0.58, 1.0, t),
            Easing::EaseInOut => cubic_bez_eval(0.42, 0.0, 0.58, 1.0, t),
            Easing::CubicBezier(a, b, c, d) => cubic_bez_eval(a, b, c, d, t),
            Easing::StepEnd => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Easing::StepStart => {
                if t <= 0.0 {
                    0.0
                } else {
                    1.0
                }
            }
        }
    }
}

/// 解 `x(s) = t` 求 `y(s)`：x 分量对 s 单调，二分求 s 后代入 y。
fn cubic_bez_eval(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let bx = |s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * x1 + 3.0 * u * s * s * x2 + s * s * s
    };
    let by = |s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * y1 + 3.0 * u * s * s * y2 + s * s * s
    };
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        if bx(mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    by((lo + hi) * 0.5)
}

/// 迭代次数：有限次数或 `infinite`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IterationCount {
    Num(f64),
    Infinite,
}

impl Default for IterationCount {
    fn default() -> Self {
        IterationCount::Num(1.0)
    }
}

/// `animation-direction`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimDirection {
    #[default]
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

/// `animation-fill-mode`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillMode {
    #[default]
    None,
    Forwards,
    Backwards,
    Both,
}

/// `animation-play-state`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayState {
    #[default]
    Running,
    Paused,
}

/// 单条 `animation` 的 longhand 解析结果。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnimSpec {
    pub name: String,
    pub duration_ms: f64,
    pub easing: Easing,
    pub delay_ms: f64,
    pub iteration: IterationCount,
    pub direction: AnimDirection,
    pub fill: FillMode,
    pub play_state: PlayState,
}

/// 单条 `transition` 的 longhand 解析结果。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransitionSpec {
    /// 属性名（kebab-case）或 `"all"`。
    pub property: String,
    pub duration_ms: f64,
    pub easing: Easing,
    pub delay_ms: f64,
}

/// 元素的动画/过渡声明总和（`ComputedStyle.anim`；全部非继承）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnimDecl {
    pub animations: Vec<AnimSpec>,
    pub transitions: Vec<TransitionSpec>,
}

impl AnimDecl {
    pub fn is_empty(&self) -> bool {
        self.animations.is_empty() && self.transitions.is_empty()
    }

    /// 是否需要参与动画引擎跟踪（有运行语义的声明；`animation-name:none` 不算）。
    pub fn is_trackable(&self) -> bool {
        self.transitions
            .iter()
            .any(|t| t.duration_ms > 0.0 && !t.property.is_empty())
            || self
                .animations
                .iter()
                .any(|a| !a.name.is_empty() && a.name != "none" && a.duration_ms > 0.0)
    }
}

/// `@keyframes` 单个步进：`(步进位置 0..1, 声明 [(key, value)])`。
pub type KeyframeStep = (f64, Vec<(String, String)>);

/// `@keyframes name { ... }` 解析结果（步进按位置升序）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyframesDef {
    pub steps: Vec<KeyframeStep>,
}

/// 全文档 `@keyframes` 表。
pub type KeyframesMap = std::collections::HashMap<String, KeyframesDef>;

/// 解析 `@keyframes` 主体：`from {...} 40% {...} to {...}`。
pub fn parse_keyframes_body(body: &str) -> KeyframesDef {
    let mut steps: Vec<KeyframeStep> = Vec::new();
    let mut rest = body;
    while let Some((header, block, consumed)) = next_rule_block(rest) {
        rest = &rest[consumed..];
        for part in header.split(',') {
            let pos = match part.trim().to_ascii_lowercase().as_str() {
                "from" => Some(0.0),
                "to" => Some(100.0),
                p => p
                    .trim_end_matches('%')
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .map(|v| v.clamp(0.0, 100.0)),
            };
            let Some(pct) = pos else { continue };
            let decls = parse_simple_decls(&block);
            if decls.is_empty() {
                continue;
            }
            // 同一步进多块合并（后者覆盖同名属性）。
            if let Some(slot) = steps.iter_mut().find(|(p, _)| (*p - pct).abs() < 1e-6) {
                for (k, v) in decls {
                    if let Some(e) = slot.1.iter_mut().find(|(ek, _)| *ek == k) {
                        e.1 = v;
                    } else {
                        slot.1.push((k, v));
                    }
                }
            } else {
                steps.push((pct / 100.0, decls));
            }
        }
    }
    steps.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    KeyframesDef { steps }
}

/// 应用一条动画/过渡声明到 `AnimDecl`（简写与 longhand 统一入口）。
///
/// longhand 的逗号列表按位置下标逐条覆盖（列表长度 1 且目标已存在时铺满全部条目，
/// 覆盖绝大多数站点写法）；简写整体重建对应列表。
pub fn apply_anim_declaration(decl: &mut AnimDecl, key: &str, value: &str) {
    let v = value.trim();
    if v.is_empty() {
        return;
    }
    // 全局关键字:回到初始值。
    if matches!(
        v.to_ascii_lowercase().as_str(),
        "initial" | "unset" | "revert" | "revert-layer"
    ) {
        match key {
            "animation" | "animation-name" | "animation-duration" | "animation-timing-function"
            | "animation-delay" | "animation-iteration-count" | "animation-direction"
            | "animation-fill-mode" | "animation-play-state" => decl.animations.clear(),
            _ => decl.transitions.clear(),
        }
        return;
    }
    if key == "inherit" {
        return;
    }
    match key {
        "animation" => {
            decl.animations = split_top_commas(v)
                .iter()
                .map(|seg| parse_animation_segment(seg))
                .collect();
            if decl.animations.len() > 8 {
                decl.animations.truncate(8);
            }
        }
        "transition" => {
            decl.transitions = split_top_commas(v)
                .iter()
                .map(|seg| parse_transition_segment(seg))
                .collect();
            if decl.transitions.len() > 8 {
                decl.transitions.truncate(8);
            }
        }
        "animation-name" => {
            set_anim_field(&mut decl.animations, v, |s, tok| s.name = tok.to_string());
        }
        "animation-duration" | "animation-delay" => {
            let is_delay = key.ends_with("delay");
            set_anim_field(&mut decl.animations, v, |s, tok| {
                if let Some(ms) = parse_time_ms(&tok) {
                    if is_delay {
                        s.delay_ms = ms;
                    } else {
                        s.duration_ms = ms;
                    }
                }
            });
        }
        "animation-timing-function" => {
            set_anim_field(&mut decl.animations, v, |s, tok| {
                if let Some(e) = Easing::parse(tok) {
                    s.easing = e;
                }
            });
        }
        "animation-iteration-count" => {
            set_anim_field(&mut decl.animations, v, |s, tok| {
                let t = tok.trim().to_ascii_lowercase();
                if t == "infinite" {
                    s.iteration = IterationCount::Infinite;
                } else if let Ok(n) = t.parse::<f64>() {
                    s.iteration = IterationCount::Num(n.max(0.0));
                }
            });
        }
        "animation-direction" => {
            set_anim_field(&mut decl.animations, v, |s, tok| {
                s.direction = match tok.trim().to_ascii_lowercase().as_str() {
                    "reverse" => AnimDirection::Reverse,
                    "alternate" => AnimDirection::Alternate,
                    "alternate-reverse" => AnimDirection::AlternateReverse,
                    _ => AnimDirection::Normal,
                };
            });
        }
        "animation-fill-mode" => {
            set_anim_field(&mut decl.animations, v, |s, tok| {
                s.fill = match tok.trim().to_ascii_lowercase().as_str() {
                    "forwards" | "forward" => FillMode::Forwards,
                    "backwards" | "backward" => FillMode::Backwards,
                    "both" => FillMode::Both,
                    _ => FillMode::None,
                };
            });
        }
        "animation-play-state" => {
            set_anim_field(&mut decl.animations, v, |s, tok| {
                s.play_state = if tok.trim().eq_ignore_ascii_case("paused") {
                    PlayState::Paused
                } else {
                    PlayState::Running
                };
            });
        }
        "transition-property" => {
            // 属性列表 × 现有（或缺省）时间参数,按位置铺开。
            let props = split_top_commas(v);
            let old = decl.transitions.clone();
            decl.transitions = props
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let base = old.get(i).cloned().or_else(|| old.first().cloned());
                    let mut t = base.unwrap_or_default();
                    t.property = p.trim().to_ascii_lowercase();
                    t
                })
                .collect();
        }
        "transition-duration" | "transition-delay" => {
            let is_delay = key.ends_with("delay");
            set_trans_field(&mut decl.transitions, v, |t, tok| {
                if let Some(ms) = parse_time_ms(&tok) {
                    if is_delay {
                        t.delay_ms = ms;
                    } else {
                        t.duration_ms = ms;
                    }
                }
            });
        }
        "transition-timing-function" => {
            set_trans_field(&mut decl.transitions, v, |t, tok| {
                if let Some(e) = Easing::parse(tok) {
                    t.easing = e;
                }
            });
        }
        _ => {}
    }
}

/// longhand 逗号列表按位置回写;单项值铺满已有条目（无条目时建一条缺省）。
fn set_anim_field(list: &mut Vec<AnimSpec>, value: &str, mut f: impl FnMut(&mut AnimSpec, &str)) {
    if list.is_empty() {
        list.push(AnimSpec::default());
    }
    let toks = split_top_commas(value);
    if toks.len() == 1 {
        for s in list.iter_mut() {
            f(s, toks[0].trim());
        }
    } else {
        for (i, tok) in toks.iter().enumerate() {
            if i >= list.len() {
                list.push(AnimSpec::default());
            }
            f(&mut list[i], tok.trim());
        }
    }
}

fn set_trans_field(
    list: &mut Vec<TransitionSpec>,
    value: &str,
    mut f: impl FnMut(&mut TransitionSpec, &str),
) {
    if list.is_empty() {
        list.push(TransitionSpec::default());
    }
    let toks = split_top_commas(value);
    if toks.len() == 1 {
        for t in list.iter_mut() {
            f(t, toks[0].trim());
        }
    } else {
        for (i, tok) in toks.iter().enumerate() {
            if i >= list.len() {
                list.push(TransitionSpec::default());
            }
            f(&mut list[i], tok.trim());
        }
    }
}

/// `animation` 简写单段:无序 token 流按类型识别。
fn parse_animation_segment(seg: &str) -> AnimSpec {
    let mut spec = AnimSpec::default();
    let mut time_seen = 0usize;
    let mut name_seen = false;
    for tok in split_ws_respect_parens(seg) {
        let lower = tok.to_ascii_lowercase();
        if let Some(ms) = parse_time_ms(&tok) {
            match time_seen {
                0 => spec.duration_ms = ms,
                1 => spec.delay_ms = ms,
                _ => {}
            }
            time_seen += 1;
            continue;
        }
        if lower == "infinite" {
            spec.iteration = IterationCount::Infinite;
            continue;
        }
        if let Ok(n) = lower.parse::<f64>() {
            spec.iteration = IterationCount::Num(n.max(0.0));
            continue;
        }
        if lower.contains("cubic-bezier(") || Easing::parse(&tok).is_some() {
            // `none` 也是合法 fill-mode;缓动关键字先于 fill 判定。
            if lower != "none" {
                spec.easing = Easing::parse(&tok).unwrap_or_default();
                continue;
            }
        }
        match lower.as_str() {
            "normal" => {
                spec.direction = AnimDirection::Normal;
                continue;
            }
            "reverse" => {
                spec.direction = AnimDirection::Reverse;
                continue;
            }
            "alternate" => {
                spec.direction = AnimDirection::Alternate;
                continue;
            }
            "alternate-reverse" => {
                spec.direction = AnimDirection::AlternateReverse;
                continue;
            }
            "forwards" | "forward" => {
                spec.fill = FillMode::Forwards;
                continue;
            }
            "backwards" | "backward" => {
                spec.fill = FillMode::Backwards;
                continue;
            }
            "both" => {
                spec.fill = FillMode::Both;
                continue;
            }
            "paused" => {
                spec.play_state = PlayState::Paused;
                continue;
            }
            "running" => {
                spec.play_state = PlayState::Running;
                continue;
            }
            _ => {}
        }
        if lower == "none" {
            // `none` 歧义(name 与 fill-mode 都合法):先归 name,fill 交给后续 token。
            // 视觉结果一致——name=none 即不运行动画。
            if !name_seen {
                name_seen = true;
                spec.name = "none".to_string();
            }
            continue;
        }
        if !name_seen {
            name_seen = true;
            spec.name = tok.to_string();
        }
    }
    spec
}

/// `transition` 简写单段:第一个未识别 ident = 属性名。
fn parse_transition_segment(seg: &str) -> TransitionSpec {
    let mut spec = TransitionSpec::default();
    let mut time_seen = 0usize;
    let mut prop_seen = false;
    for tok in split_ws_respect_parens(seg) {
        if let Some(ms) = parse_time_ms(&tok) {
            match time_seen {
                0 => spec.duration_ms = ms,
                1 => spec.delay_ms = ms,
                _ => {}
            }
            time_seen += 1;
            continue;
        }
        if let Some(e) = Easing::parse(&tok) {
            spec.easing = e;
            continue;
        }
        if !prop_seen {
            prop_seen = true;
            spec.property = tok.to_ascii_lowercase();
        }
    }
    spec
}

/// `1s` → 1000、`250ms` → 250;裸数字/其他返回 None。
pub fn parse_time_ms(tok: &str) -> Option<f64> {
    let t = tok.trim().to_ascii_lowercase();
    if let Some(ms) = t.strip_suffix("ms") {
        return ms.trim().parse::<f64>().ok().map(|v| v.max(0.0));
    }
    if let Some(s) = t.strip_suffix('s') {
        return s.trim().parse::<f64>().ok().map(|v| (v * 1000.0).max(0.0));
    }
    None
}

/// 顶层逗号切分（尊重括号,`cubic-bezier(a, b)` 不切）。
fn split_top_commas(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in input.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// 空白切分（尊重括号,`cubic-bezier(0.1, 0.2, 0.3, 0.4)` 整段保留）。
fn split_ws_respect_parens(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in input.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            _ if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 简单声明体解析 `key: value; …`（关键帧步进块内用;不处理嵌套）。
fn parse_simple_decls(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in body.split(';') {
        let Some((k, v)) = part.split_once(':') else { continue };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim();
        if k.is_empty() || v.is_empty() || k.starts_with("--") {
            continue;
        }
        // `!important` 在关键帧步进里无级联意义,截掉。
        let v = v.strip_suffix("!important").map(str::trim).unwrap_or(v);
        out.push((k, v.to_string()));
    }
    out
}

/// 读取下一个 `header { body }`（`@keyframes` 步进块切分用）。
fn next_rule_block(input: &str) -> Option<(String, String, usize)> {
    let open = input.find('{')?;
    let header = input[..open].trim().to_string();
    let mut depth = 0i32;
    let mut end = None;
    for (idx, c) in input[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + idx);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    let body = input[open + 1..end].to_string();
    Some((header, body, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 缓动函数求值单调过端点() {
        assert!((Easing::Linear.eval(0.5) - 0.5).abs() < 1e-9);
        assert!((Easing::Ease.eval(0.0)).abs() < 1e-9);
        assert!((Easing::Ease.eval(1.0) - 1.0).abs() < 1e-6);
        // ease 前段快后段慢:中点值高于线性。
        assert!(Easing::Ease.eval(0.5) > 0.5);
        let cb = Easing::parse("cubic-bezier(0, 0, 1, 1)").unwrap();
        assert!((cb.eval(0.5) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn 时间解析毫秒与秒() {
        assert_eq!(parse_time_ms("1s"), Some(1000.0));
        assert_eq!(parse_time_ms("250ms"), Some(250.0));
        assert_eq!(parse_time_ms("0.3s"), Some(300.0));
        assert_eq!(parse_time_ms("3"), None);
    }

    #[test]
    fn 动画简写无序token流() {
        let spec = parse_animation_segment("fade 1s ease-in 200ms 3 reverse both paused");
        assert_eq!(spec.name, "fade");
        assert_eq!(spec.duration_ms, 1000.0);
        assert_eq!(spec.delay_ms, 200.0);
        assert_eq!(spec.iteration, IterationCount::Num(3.0));
        assert_eq!(spec.direction, AnimDirection::Reverse);
        assert_eq!(spec.fill, FillMode::Both);
        assert_eq!(spec.play_state, PlayState::Paused);
    }

    #[test]
    fn 动画简写识别cubic_bezier与alternate_reverse() {
        let spec = parse_animation_segment("spin 2s cubic-bezier(0.1, 0.2, 0.3, 0.4) infinite alternate-reverse");
        assert_eq!(spec.name, "spin");
        assert_eq!(
            spec.easing,
            Easing::CubicBezier(0.1, 0.2, 0.3, 0.4)
        );
        assert_eq!(spec.iteration, IterationCount::Infinite);
        assert_eq!(spec.direction, AnimDirection::AlternateReverse);
    }

    #[test]
    fn 过渡简写与逗号多段() {
        let mut decl = AnimDecl::default();
        apply_anim_declaration(&mut decl, "transition", "width 0.3s linear, opacity 200ms ease-out 50ms");
        assert_eq!(decl.transitions.len(), 2);
        assert_eq!(decl.transitions[0].property, "width");
        assert_eq!(decl.transitions[0].duration_ms, 300.0);
        assert_eq!(decl.transitions[0].easing, Easing::Linear);
        assert_eq!(decl.transitions[1].property, "opacity");
        assert_eq!(decl.transitions[1].delay_ms, 50.0);
    }

    #[test]
    fn longhand单项铺满多条目() {
        let mut decl = AnimDecl::default();
        apply_anim_declaration(&mut decl, "animation", "a 1s, b 2s");
        apply_anim_declaration(&mut decl, "animation-duration", "500ms");
        assert_eq!(decl.animations.len(), 2);
        assert_eq!(decl.animations[0].duration_ms, 500.0);
        assert_eq!(decl.animations[1].duration_ms, 500.0);
        assert_eq!(decl.animations[1].name, "b");
    }

    #[test]
    fn 关键帧步进收集与合并() {
        let def = parse_keyframes_body("from { opacity: 0 } 50% { opacity: 0.5; transform: scale(1.2) } to { opacity: 1 }");
        assert_eq!(def.steps.len(), 3);
        assert_eq!(def.steps[0].0, 0.0);
        assert_eq!(def.steps[1].0, 0.5);
        assert_eq!(def.steps[2].0, 1.0);
        // 同步进多块合并。
        let def2 = parse_keyframes_body("0% { opacity: 0 } 0% { transform: none } 100% { opacity: 1 }");
        assert_eq!(def2.steps.len(), 2);
        assert_eq!(def2.steps[0].1.len(), 2);
    }

    #[test]
    fn is_trackable门槛建议() {
        let mut decl = AnimDecl::default();
        assert!(!decl.is_trackable());
        apply_anim_declaration(&mut decl, "transition", "color 0s");
        assert!(!decl.is_trackable()); // 0 时长过渡不跟踪
        apply_anim_declaration(&mut decl, "transition", "color 1s");
        assert!(decl.is_trackable());
    }
}
