//! CSS 过渡与 @keyframes 动画引擎（第 57 轮）：时间轴插值 + 布局钩子 + 帧同步判定。
//!
//! 数据流：样式层把 `animation`/`transition` 解析进 `ComputedStyle.anim`（见
//! `style/anim_style.rs`），布局 `LayoutCtx::style_for` 在计算样式后调
//! [`AnimEngine::track_and_apply`]——先与上一布局快照 diff 触发/重定向过渡，再用
//! 当前时刻的动画值覆盖计算样式。截图 / 查看器帧 / inspect 几何 / JS
//! `getComputedStyle`（经 `apply_overrides`）因此同源。
//! 设计见 `docs/MCP_Web_LsmAgentBrowser_Use/03-布局与渲染引擎/57-CSS过渡与关键帧动画帧同步方案.md`。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use super::style::anim_style::{
    AnimDecl, AnimDirection, AnimSpec, Easing, FillMode, IterationCount, KeyframesDef,
    KeyframesMap, PlayState,
};
use super::style::transform::{parse_transform, Length, TransformOp};
use super::style::values::{parse_color, parse_length, ValueCtx};
use super::style::{ComputedStyle, Transform2D};

/// 第 57 轮可动画属性集（超出集的属性走离散跳变/不跟踪）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimProp {
    Opacity,
    Transform,
    Color,
    BackgroundColor,
    Width,
    Height,
    Top,
    Right,
    Bottom,
    Left,
    MarginTop,
    MarginRight,
    MarginBottom,
    MarginLeft,
    PaddingTop,
    PaddingRight,
    PaddingBottom,
    PaddingLeft,
}

impl AnimProp {
    /// 属性名 → 可动画属性（`transition-property` 用）。
    pub fn from_name(name: &str) -> Option<Self> {
        use AnimProp::*;
        Some(match name {
            "opacity" => Opacity,
            "transform" => Transform,
            "color" => Color,
            "background-color" => BackgroundColor,
            "width" => Width,
            "height" => Height,
            "top" => Top,
            "right" => Right,
            "bottom" => Bottom,
            "left" => Left,
            "margin-top" => MarginTop,
            "margin-right" => MarginRight,
            "margin-bottom" => MarginBottom,
            "margin-left" => MarginLeft,
            "padding-top" => PaddingTop,
            "padding-right" => PaddingRight,
            "padding-bottom" => PaddingBottom,
            "padding-left" => PaddingLeft,
            _ => return None,
        })
    }

    fn all() -> &'static [AnimProp] {
        use AnimProp::*;
        &[
            Opacity,
            Transform,
            Color,
            BackgroundColor,
            Width,
            Height,
            Top,
            Right,
            Bottom,
            Left,
            MarginTop,
            MarginRight,
            MarginBottom,
            MarginLeft,
            PaddingTop,
            PaddingRight,
            PaddingBottom,
            PaddingLeft,
        ]
    }
}

/// 动画插值单位值。
#[derive(Debug, Clone, PartialEq)]
pub enum AnimValue {
    /// 无单位数（opacity）。
    Num(f64),
    /// 像素长度。
    Px(f64),
    /// 颜色（含 alpha,插值走预乘 alpha,避免透明↔彩色中间帧发黑）。
    Color { rgb: [u8; 3], a: u8 },
    /// transform 操作序列（布局期按盒尺寸求解;插值在操作分量级进行）。
    Transform(Vec<TransformOp>),
}

/// transform 单操作归一为「族 + 分量」,族内可线性插值。
enum OpFam {
    Translate(Length, Length),
    Rotate(f64),
    Scale(f64, f64),
    Skew(f64, f64),
    Matrix(f64, f64, f64, f64, Length, Length),
}

fn op_family(op: &TransformOp) -> OpFam {
    match op {
        TransformOp::Translate(x, y) => OpFam::Translate(*x, *y),
        TransformOp::TranslateX(x) => OpFam::Translate(*x, Length::Px(0.0)),
        TransformOp::TranslateY(y) => OpFam::Translate(Length::Px(0.0), *y),
        TransformOp::Rotate(deg) => OpFam::Rotate(*deg),
        TransformOp::Scale(sx, sy) => OpFam::Scale(*sx, *sy),
        TransformOp::ScaleX(sx) => OpFam::Scale(*sx, 1.0),
        TransformOp::ScaleY(sy) => OpFam::Scale(1.0, *sy),
        TransformOp::Skew(ax, ay) => OpFam::Skew(*ax, *ay),
        TransformOp::SkewX(ax) => OpFam::Skew(*ax, 0.0),
        TransformOp::SkewY(ay) => OpFam::Skew(0.0, *ay),
        &TransformOp::Matrix(a, b, c, d, e, f) => OpFam::Matrix(a, b, c, d, e, f),
    }
}

fn fam_to_op(fam: OpFam) -> TransformOp {
    match fam {
        OpFam::Translate(x, y) => TransformOp::Translate(x, y),
        OpFam::Rotate(deg) => TransformOp::Rotate(deg),
        OpFam::Scale(sx, sy) => TransformOp::Scale(sx, sy),
        OpFam::Skew(ax, ay) => TransformOp::Skew(ax, ay),
        OpFam::Matrix(a, b, c, d, e, f) => TransformOp::Matrix(a, b, c, d, e, f),
    }
}

fn lerp_len(a: Length, b: Length, t: f64) -> Option<Length> {
    match (a, b) {
        (Length::Px(x), Length::Px(y)) => Some(Length::Px(x + (y - x) * t)),
        (Length::Pct(x), Length::Pct(y)) => Some(Length::Pct(x + (y - x) * t)),
        _ => None,
    }
}

fn lerp_f(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// 单值插值。种类不匹配或 transform 族不匹配时 50% 处离散跳变（边界见 57 号文档）。
pub fn lerp_value(a: &AnimValue, b: &AnimValue, t: f64) -> AnimValue {
    match (a, b) {
        (AnimValue::Num(x), AnimValue::Num(y)) => AnimValue::Num(lerp_f(*x, *y, t)),
        (AnimValue::Px(x), AnimValue::Px(y)) => AnimValue::Px(lerp_f(*x, *y, t)),
        (
            AnimValue::Color {
                rgb: c1,
                a: a1,
            },
            AnimValue::Color {
                rgb: c2,
                a: a2,
            },
        ) => {
            // 预乘 alpha 插值。
            let (p1, p2) = (*a1 as f64 / 255.0, *a2 as f64 / 255.0);
            let a_out = lerp_f(p1, p2, t).clamp(0.0, 1.0);
            let mut rgb = [0u8; 3];
            for i in 0..3 {
                let prem = lerp_f(c1[i] as f64 * p1, c2[i] as f64 * p2, t);
                rgb[i] = if a_out > 1e-6 {
                    (prem / a_out).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                };
            }
            AnimValue::Color {
                rgb,
                a: (a_out * 255.0).round() as u8,
            }
        }
        (AnimValue::Transform(ops1), AnimValue::Transform(ops2)) => {
            if ops1.len() != ops2.len() {
                return if t < 0.5 { a.clone() } else { b.clone() };
            }
            let mut out = Vec::with_capacity(ops1.len());
            for (o1, o2) in ops1.iter().zip(ops2.iter()) {
                let (f1, f2) = (op_family(o1), op_family(o2));
                let mixed = match (f1, f2) {
                    (OpFam::Translate(x1, y1), OpFam::Translate(x2, y2)) => {
                        Some(lerp_len(x1, x2, t).zip(lerp_len(y1, y2, t)).map(
                            |(x, y)| OpFam::Translate(x, y),
                        ))
                    }
                    (OpFam::Rotate(d1), OpFam::Rotate(d2)) => {
                        Some(Some(OpFam::Rotate(lerp_f(d1, d2, t))))
                    }
                    (OpFam::Scale(x1, y1), OpFam::Scale(x2, y2)) => {
                        Some(Some(OpFam::Scale(lerp_f(x1, x2, t), lerp_f(y1, y2, t))))
                    }
                    (OpFam::Skew(x1, y1), OpFam::Skew(x2, y2)) => {
                        Some(Some(OpFam::Skew(lerp_f(x1, x2, t), lerp_f(y1, y2, t))))
                    }
                    (
                        OpFam::Matrix(a1, b1, c1, d1, e1, f1),
                        OpFam::Matrix(a2, b2, c2, d2, e2, f2),
                    ) => Some(
                        lerp_len(e1, e2, t).zip(lerp_len(f1, f2, t)).map(|(e, f)| {
                            OpFam::Matrix(
                                lerp_f(a1, a2, t),
                                lerp_f(b1, b2, t),
                                lerp_f(c1, c2, t),
                                lerp_f(d1, d2, t),
                                e,
                                f,
                            )
                        }),
                    ),
                    _ => Some(None), // 族不匹配:离散
                };
                match mixed {
                    Some(Some(fam)) => out.push(fam_to_op(fam)),
                    Some(None) => {
                        return if t < 0.5 { a.clone() } else { b.clone() };
                    }
                    None => {
                        return if t < 0.5 { a.clone() } else { b.clone() };
                    }
                }
            }
            AnimValue::Transform(out)
        }
        _ => {
            if t < 0.5 {
                a.clone()
            } else {
                b.clone()
            }
        }
    }
}

/// 从计算样式提取可动画属性值;`None` = 该属性当前不可插值（auto/百分比/缺省）。
pub fn extract_prop(style: &ComputedStyle, prop: AnimProp) -> Option<AnimValue> {
    use AnimProp::*;
    Some(match prop {
        Opacity => AnimValue::Num(style.opacity),
        Transform => AnimValue::Transform(style.transform.clone().map(|t| t.ops).unwrap_or_default()),
        Color => AnimValue::Color {
            rgb: style.color,
            a: 255,
        },
        BackgroundColor => AnimValue::Color {
            rgb: style.background.unwrap_or([0, 0, 0]),
            a: if style.background.is_some() {
                style.background_alpha
            } else {
                0
            },
        },
        Width => AnimValue::Px(style.width?),
        Height => AnimValue::Px(style.height?),
        Top => AnimValue::Px(style.pos.top?),
        Right => AnimValue::Px(style.pos.right?),
        Bottom => AnimValue::Px(style.pos.bottom?),
        Left => AnimValue::Px(style.pos.left?),
        MarginTop => AnimValue::Px(style.margin_top),
        MarginRight => {
            if style.margin_right_auto {
                return None;
            }
            AnimValue::Px(style.margin_right)
        }
        MarginBottom => AnimValue::Px(style.margin_bottom),
        MarginLeft => {
            if style.margin_left_auto {
                return None;
            }
            AnimValue::Px(style.margin_left)
        }
        PaddingTop => AnimValue::Px(style.padding_top),
        PaddingRight => AnimValue::Px(style.padding_right),
        PaddingBottom => AnimValue::Px(style.padding_bottom),
        PaddingLeft => AnimValue::Px(style.padding_left),
    })
}

/// 把动画值写回计算样式（布局/渲染/快照共用）。
pub fn apply_prop(style: &mut ComputedStyle, prop: AnimProp, v: &AnimValue) {
    use AnimProp::*;
    match (prop, v) {
        (Opacity, AnimValue::Num(n)) => style.opacity = n.clamp(0.0, 1.0),
        (Transform, AnimValue::Transform(ops)) => {
            style.transform = if ops.is_empty() {
                None
            } else {
                Some(Transform2D { ops: ops.clone() })
            };
        }
        (Color, AnimValue::Color { rgb, .. }) => style.color = *rgb,
        (BackgroundColor, AnimValue::Color { rgb, a }) => {
            if *a == 0 && *rgb == [0, 0, 0] {
                style.background = None;
                style.background_alpha = 255;
            } else {
                style.background = Some(*rgb);
                style.background_alpha = *a;
            }
        }
        (Width, AnimValue::Px(px)) => {
            style.width = Some(*px);
            style.width_percent = None;
        }
        (Height, AnimValue::Px(px)) => {
            style.height = Some(*px);
            style.height_percent = None;
        }
        (Top, AnimValue::Px(px)) => {
            style.pos.top = Some(*px);
            style.pos.top_percent = None;
        }
        (Right, AnimValue::Px(px)) => {
            style.pos.right = Some(*px);
            style.pos.right_percent = None;
        }
        (Bottom, AnimValue::Px(px)) => {
            style.pos.bottom = Some(*px);
            style.pos.bottom_percent = None;
        }
        (Left, AnimValue::Px(px)) => {
            style.pos.left = Some(*px);
            style.pos.left_percent = None;
        }
        (MarginTop, AnimValue::Px(px)) => style.margin_top = *px,
        (MarginRight, AnimValue::Px(px)) => {
            style.margin_right = *px;
            style.margin_right_auto = false;
        }
        (MarginBottom, AnimValue::Px(px)) => style.margin_bottom = *px,
        (MarginLeft, AnimValue::Px(px)) => {
            style.margin_left = *px;
            style.margin_left_auto = false;
        }
        (PaddingTop, AnimValue::Px(px)) => {
            style.padding_top = *px;
            style.padding_percent[0] = None;
        }
        (PaddingRight, AnimValue::Px(px)) => {
            style.padding_right = *px;
            style.padding_percent[1] = None;
        }
        (PaddingBottom, AnimValue::Px(px)) => {
            style.padding_bottom = *px;
            style.padding_percent[2] = None;
        }
        (PaddingLeft, AnimValue::Px(px)) => {
            style.padding_left = *px;
            style.padding_percent[3] = None;
        }
        _ => {}
    }
}

/// 解析关键帧步进里的属性原值（`opacity: 0` / `transform: scale(2)` / …）。
fn parse_anim_value(
    prop: AnimProp,
    raw: &str,
    font_size: f64,
    ctx: &ValueCtx,
) -> Option<AnimValue> {
    use AnimProp::*;
    match prop {
        Opacity => {
            let r = raw.trim().trim_end_matches('%');
            let n: f64 = r.trim().parse().ok()?;
            Some(AnimValue::Num(
                if raw.trim().ends_with('%') {
                    n / 100.0
                } else {
                    n
                }
                .clamp(0.0, 1.0),
            ))
        }
        Transform => Some(AnimValue::Transform(
            parse_transform(raw).map(|t| t.ops).unwrap_or_default(),
        )),
        Color | BackgroundColor => {
            // `parse_color` 外层 None=无法识别;内层 None=`transparent`(alpha 0)。
            let c = parse_color(raw)?.unwrap_or(([0, 0, 0], 0));
            Some(AnimValue::Color { rgb: c.0, a: c.1 })
        }
        _ => parse_length(raw, font_size, ctx).map(AnimValue::Px),
    }
}

/// 运行中的关键帧动画。
#[derive(Debug, Clone)]
struct RunningAnim {
    name: String,
    spec: AnimSpec,
    /// 声明开始时刻（含 delay 起点;时钟毫秒）。
    start_ms: u128,
    /// 已消耗时长（pause 冻结值;未暂停时为 0,靠 start_ms 平移补偿）。
    frozen: Option<u128>,
    /// 步进（升序 0..1,端点已用底层值补齐）,按属性并联。
    steps: Vec<(f64, HashMap<AnimProp, AnimValue>)>,
}

/// 运行中的过渡。
#[derive(Debug, Clone)]
struct RunningTransition {
    from: AnimValue,
    to: AnimValue,
    start_ms: u128,
    delay_ms: f64,
    duration_ms: f64,
    easing: Easing,
}

impl RunningTransition {
    fn value_at(&self, now: u128) -> AnimValue {
        let elapsed = now.saturating_sub(self.start_ms) as f64 - self.delay_ms;
        if elapsed <= 0.0 {
            return self.from.clone();
        }
        let t = (elapsed / self.duration_ms.max(1.0)).clamp(0.0, 1.0);
        if t >= 1.0 {
            return self.to.clone();
        }
        lerp_value(&self.from, &self.to, self.easing.eval(t))
    }

    fn finished(&self, now: u128) -> bool {
        now.saturating_sub(self.start_ms) as f64 >= self.delay_ms + self.duration_ms
    }
}

/// 单节点动画状态。
#[derive(Debug, Clone, Default)]
struct NodeAnim {
    anims: Vec<RunningAnim>,
    trans: HashMap<AnimProp, RunningTransition>,
    /// 上一布局的 CSS 计算值（过渡 diff 基线;不含动画覆盖）。
    last_values: HashMap<AnimProp, AnimValue>,
    last_seen_pass: u64,
}

/// 动画覆盖快照:节点稳定键 → 属性 → 当前动画值(纯数据,JS `getComputedStyle`
/// 与布局同源用;可安全跨线程传递)。
pub type AnimOverrides = HashMap<String, HashMap<AnimProp, AnimValue>>;

/// 把覆盖快照应用到计算样式(脚本快照路径共用)。
pub fn apply_overrides_map(ov: &AnimOverrides, key: &str, style: &mut ComputedStyle) {
    let Some(eff) = ov.get(key) else {
        return;
    };
    for (prop, v) in eff {
        apply_prop(style, *prop, v);
    }
}

/// 动画引擎（全局单实例挂在 `TabState.anim`;内部可变,布局钩子只借 `&self`）。
#[derive(Debug, Clone)]
pub struct AnimEngine {
    nodes: RefCell<HashMap<String, NodeAnim>>,
    clock_override: Cell<Option<u128>>,
    pass: Cell<u64>,
}

impl Default for AnimEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AnimEngine {
    pub fn new() -> Self {
        Self {
            nodes: RefCell::new(HashMap::new()),
            clock_override: Cell::new(None),
            pass: Cell::new(0),
        }
    }

    /// 测试注入时钟;`None` 恢复墙钟。
    pub fn set_clock_override(&self, t: Option<u128>) {
        self.clock_override.set(t);
    }

    /// 当前时刻（毫秒）。测试走 override,生产走墙钟。
    pub fn now(&self) -> u128 {
        self.clock_override.get().unwrap_or_else(super::now_ms)
    }

    /// 是否有「活跃」动画状态(含 fill:forwards/both 的终帧动画,以及
    /// **已到时但尚未落终值**的过渡)。
    ///
    /// `ensure_layout_locked` 的重排守卫用:终帧动画仍持续覆盖计算值;过渡结束
    /// 后必须再重排一次把终值落进布局(否则布局冻结在结束前最后一次补间值——
    /// 实测 e2e:过渡跑到 220/300 后 html 不再变,守卫跳过重排,画面停格)。
    /// 运行中条目由 `track_and_apply` 在落终值的那一次调用里回收。
    pub fn has_active(&self) -> bool {
        self.nodes
            .borrow()
            .values()
            .any(|n| !n.anims.is_empty() || !n.trans.is_empty())
    }

    /// 是否有**仍在变化**的动画/过渡(不含已到终帧/已暂停)——ticker 节拍用:
    /// 为真 40ms 快节拍,为假按 500ms 低频(终帧/暂停值稳定,无需快拍)。
    pub fn is_animating(&self) -> bool {
        let now = self.now();
        self.nodes.borrow().values().any(|n| {
            n.anims
                .iter()
                .any(|ra| !ra.finished(now) && ra.spec.play_state != PlayState::Paused)
                || n.trans.values().any(|t| !t.finished(now))
        })
    }

    /// 布局回合开始（GC 记账;`compute_full` 入口调用）。
    pub fn begin_pass(&self) {
        self.pass.set(self.pass.get() + 1);
    }

    /// 布局回合结束:连续两回合未见的节点整体回收（防 display:none 子树误杀,
    /// 阈值 2 回合;重新出现时动画自然重启,边界见 57 号文档）。
    pub fn end_pass(&self) {
        let pass = self.pass.get();
        self.nodes
            .borrow_mut()
            .retain(|_, n| pass.saturating_sub(n.last_seen_pass) <= 1);
    }

    /// 布局钩子:观察计算值(触发/重定向过渡、启停关键帧动画),再用当前时刻的
    /// 动画值覆盖 `style`。同一布局内多次调用幂等(值未变不触发新过渡)。
    pub fn track_and_apply(
        &self,
        key: &str,
        style: &mut ComputedStyle,
        kfs: &KeyframesMap,
        ctx: &ValueCtx,
    ) {
        let decl: AnimDecl = style.anim.clone();
        if !decl.is_trackable() {
            return;
        }
        let now = self.now();
        let pass = self.pass.get();
        let mut nodes = self.nodes.borrow_mut();
        let node = nodes.entry(key.to_string()).or_default();
        node.last_seen_pass = pass;

        // 1. 当前 CSS 计算值快照(插值不可行的属性不进 diff)。
        let mut current: HashMap<AnimProp, AnimValue> = HashMap::new();
        for prop in AnimProp::all() {
            if let Some(v) = extract_prop(style, *prop) {
                current.insert(*prop, v);
            }
        }

        // 2. 过渡:与上一布局基线 diff,变更即启动/重定向。
        for spec in &decl.transitions {
            if spec.duration_ms <= 0.0 {
                continue;
            }
            let props: Vec<AnimProp> = if spec.property == "all" {
                AnimProp::all().to_vec()
            } else {
                AnimProp::from_name(&spec.property).into_iter().collect()
            };
            for prop in props {
                let (Some(cur), Some(prev)) = (current.get(&prop), node.last_values.get(&prop))
                else {
                    continue;
                };
                if cur == prev {
                    continue;
                }
                // 已有同属性过渡:从当前插值位置重定向(retarget)。
                let from = node
                    .trans
                    .get(&prop)
                    .map(|t| t.value_at(now))
                    .unwrap_or_else(|| prev.clone());
                node.trans.insert(
                    prop,
                    RunningTransition {
                        from,
                        to: cur.clone(),
                        start_ms: now,
                        delay_ms: spec.delay_ms,
                        duration_ms: spec.duration_ms,
                        easing: spec.easing,
                    },
                );
            }
        }

        // 3. 关键帧动画:声明里出现的新名字启动;消失的名字取消。
        let want: Vec<&AnimSpec> = decl
            .animations
            .iter()
            .filter(|a| !a.name.is_empty() && a.name != "none" && a.duration_ms > 0.0)
            .collect();
        // 名字从声明里消失 = 动画取消(CSS 语义);名字仍在的保持运行。
        node.anims.retain(|ra| want.iter().any(|s| s.name == ra.name));
        for spec in &want {
            if node.anims.iter().any(|ra| ra.name == spec.name) {
                continue;
            }
            let Some(def) = kfs.get(&spec.name) else {
                continue;
            };
            let steps = resolve_keyframe_steps(def, &current, style.font_size, ctx);
            if steps.is_empty() {
                continue;
            }
            node.anims.push(RunningAnim {
                name: spec.name.clone(),
                spec: (*spec).clone(),
                start_ms: now,
                frozen: if spec.play_state == PlayState::Paused {
                    Some(now)
                } else {
                    None
                },
                steps,
            });
        }

        // 4. 暂停/恢复(play-state 切换平移 start_ms,冻结已消耗时长)。
        for ra in node.anims.iter_mut() {
            let want_paused = want
                .iter()
                .find(|s| s.name == ra.name)
                .map(|s| s.play_state == PlayState::Paused)
                .unwrap_or(false);
            match (ra.frozen, want_paused) {
                (None, true) => {
                    ra.frozen = Some(now);
                    ra.spec.play_state = PlayState::Paused;
                }
                (Some(f), false) => {
                    ra.start_ms += now.saturating_sub(f);
                    ra.frozen = None;
                    ra.spec.play_state = PlayState::Running;
                }
                _ => {}
            }
        }

        // 5. 叠加合成:过渡先铺,关键帧动画后盖(后者胜,与 CSS 分层一致);
        //    同类多动画取最后启动者(无 additive,边界见 57 号文档)。
        let mut effective: HashMap<AnimProp, AnimValue> = HashMap::new();
        for (prop, tr) in node.trans.iter() {
            effective.insert(*prop, tr.value_at(now));
        }
        node.trans.retain(|_, t| !t.finished(now));
        for ra in node.anims.iter() {
            if let Some(v) = ra.value_at(now, &current) {
                for (prop, val) in v {
                    effective.insert(prop, val);
                }
            }
        }
        // 结束即回收,但 fill:forwards/both 的终帧语义要保持到名字变化/节点 GC
        // (CSS 里 fill forwards 永久保留计算值),故这两档不随结束回收。
        node.anims.retain(|ra| {
            !(ra.finished(now) && matches!(ra.spec.fill, FillMode::None | FillMode::Backwards))
        });

        // 6. 基线推进 + 覆盖应用。
        node.last_values = current;
        for (prop, v) in &effective {
            apply_prop(style, *prop, v);
        }
    }

    /// 导出当前时刻的覆盖快照(供脚本管线/JS `getComputedStyle`;不推进状态)。
    ///
    /// 快照是纯数据:脚本管线多轮之间不再复读引擎(管线期间动画值按入口时刻
    /// 冻结,边界见 57 号文档)。
    pub fn override_snapshot(&self) -> AnimOverrides {
        let now = self.now();
        let nodes = self.nodes.borrow();
        let mut out = AnimOverrides::new();
        for (key, node) in nodes.iter() {
            let mut eff: HashMap<AnimProp, AnimValue> = HashMap::new();
            for (prop, tr) in node.trans.iter() {
                if !tr.finished(now) {
                    eff.insert(*prop, tr.value_at(now));
                }
            }
            for ra in node.anims.iter() {
                if let Some(v) = ra.value_at(now, &HashMap::new()) {
                    for (prop, val) in v {
                        eff.insert(prop, val);
                    }
                }
            }
            if !eff.is_empty() {
                out.insert(key.clone(), eff);
            }
        }
        out
    }

    /// 只读覆盖(供 JS `getComputedStyle` 快照;不推进任何状态)。
    pub fn apply_overrides(&self, key: &str, style: &mut ComputedStyle) {
        let now = self.now();
        let nodes = self.nodes.borrow();
        let Some(node) = nodes.get(key) else {
            return;
        };
        let mut effective: HashMap<AnimProp, AnimValue> = HashMap::new();
        for (prop, tr) in node.trans.iter() {
            effective.insert(*prop, tr.value_at(now));
        }
        for ra in node.anims.iter() {
            if let Some(v) = ra.value_at(now, &HashMap::new()) {
                for (prop, val) in v {
                    effective.insert(prop, val);
                }
            }
        }
        for (prop, v) in &effective {
            apply_prop(style, *prop, v);
        }
    }
}

/// 关键帧步进解析:每个步进按属性并联,缺失的 0%/100% 端点用底层值补齐。
fn resolve_keyframe_steps(
    def: &KeyframesDef,
    underlying: &HashMap<AnimProp, AnimValue>,
    font_size: f64,
    ctx: &ValueCtx,
) -> Vec<(f64, HashMap<AnimProp, AnimValue>)> {
    // 收集每个属性的 (位置, 值) 序列。
    let mut per_prop: HashMap<AnimProp, Vec<(f64, AnimValue)>> = HashMap::new();
    for (pos, decls) in &def.steps {
        for (k, raw) in decls {
            let Some(prop) = AnimProp::from_name(k) else { continue };
            let Some(v) = parse_anim_value(prop, raw, font_size, ctx) else {
                continue;
            };
            per_prop.entry(prop).or_default().push((*pos, v));
        }
    }
    // 端点补齐(缺失用底层值)。
    for (_prop, series) in per_prop.iter_mut() {
        series.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        if series.first().map(|(p, _)| *p > 0.0).unwrap_or(true) {
            // 0% 缺失:底层值。
            if let Some(base) = underlying.get(_prop) {
                series.insert(0, (0.0, base.clone()));
            }
        }
        if series.last().map(|(p, _)| *p < 1.0).unwrap_or(true) {
            if let Some(base) = underlying.get(_prop) {
                series.push((1.0, base.clone()));
            }
        }
    }
    // 合并为统一步进轴。
    let mut axis: Vec<f64> = per_prop.values().map(|s| s.iter().map(|(p, _)| *p)).flatten().collect();
    axis.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    axis.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let mut out: Vec<(f64, HashMap<AnimProp, AnimValue>)> = Vec::new();
    for pos in axis {
        let mut row: HashMap<AnimProp, AnimValue> = HashMap::new();
        for (prop, series) in &per_prop {
            // 该位置的值 = 系列中 ≤ pos 的最后一点(占位;真正的插值在 value_at 做)。
            let val = series
                .iter()
                .rev()
                .find(|(p, _)| *p <= pos + 1e-9)
                .map(|(_, v)| v.clone());
            if let Some(v) = val {
                row.insert(*prop, v);
            }
        }
        if !row.is_empty() {
            out.push((pos, row));
        }
    }
    out
}

impl RunningAnim {
    /// 当前时刻的动画值(属性 → 值);动画不激活(未开始/已结束无 fill)返回 None。
    fn value_at(
        &self,
        now: u128,
        current: &HashMap<AnimProp, AnimValue>,
    ) -> Option<HashMap<AnimProp, AnimValue>> {
        let elapsed = match self.frozen {
            Some(f) => f.saturating_sub(self.start_ms),
            None => now.saturating_sub(self.start_ms),
        } as f64;
        let active = elapsed - self.spec.delay_ms;
        let total = match self.spec.iteration {
            IterationCount::Num(n) => Some(self.spec.duration_ms * n.max(0.0)),
            IterationCount::Infinite => None,
        };
        // 属性集合 = 步进并集。
        let mut props: Vec<AnimProp> = Vec::new();
        for (_, row) in &self.steps {
            for p in row.keys() {
                if !props.contains(p) {
                    props.push(*p);
                }
            }
        }
        let mut out = HashMap::new();
        for prop in props {
            let val = if active < 0.0 {
                // delay 内:backwards/both 显首帧,否则底层值。
                match self.spec.fill {
                    FillMode::Backwards | FillMode::Both => {
                        self.value_of(prop, 0.0, current).or_else(|| current.get(&prop).cloned())
                    }
                    _ => current.get(&prop).cloned(),
                }
            } else if total.map(|t| active >= t).unwrap_or(false) {
                // 结束:forwards/both 显末帧,否则底层值。
                match self.spec.fill {
                    FillMode::Forwards | FillMode::Both => {
                        self.value_of(prop, 1.0, current).or_else(|| current.get(&prop).cloned())
                    }
                    _ => current.get(&prop).cloned(),
                }
            } else {
                let dur = self.spec.duration_ms.max(1.0);
                let iter_f = active / dur;
                let iter_i = iter_f.floor();
                let frac = iter_f - iter_i;
                let p = match self.spec.direction {
                    AnimDirection::Normal => frac,
                    AnimDirection::Reverse => 1.0 - frac,
                    AnimDirection::Alternate => {
                        if (iter_i as u64) % 2 == 0 {
                            frac
                        } else {
                            1.0 - frac
                        }
                    }
                    AnimDirection::AlternateReverse => {
                        if (iter_i as u64) % 2 == 0 {
                            1.0 - frac
                        } else {
                            frac
                        }
                    }
                };
                self.value_of(prop, p, current)
            };
            if let Some(v) = val {
                out.insert(prop, v);
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// 步进位置 p(0..1) 处属性的插值(段间用动画 easing;段内端点缺失已补底层值)。
    fn value_of(
        &self,
        prop: AnimProp,
        p: f64,
        current: &HashMap<AnimProp, AnimValue>,
    ) -> Option<AnimValue> {
        // 属性专属序列。
        let series: Vec<(f64, &AnimValue)> = self
            .steps
            .iter()
            .filter_map(|(pos, row)| row.get(&prop).map(|v| (*pos, v)))
            .collect();
        if series.is_empty() {
            return current.get(&prop).cloned();
        }
        if p <= series[0].0 {
            return Some(series[0].1.clone());
        }
        if p >= series[series.len() - 1].0 {
            return Some(series[series.len() - 1].1.clone());
        }
        for w in series.windows(2) {
            let (p0, v0) = w[0];
            let (p1, v1) = w[1];
            if p >= p0 && p <= p1 {
                let local = if (p1 - p0).abs() < 1e-9 {
                    1.0
                } else {
                    (p - p0) / (p1 - p0)
                };
                return Some(lerp_value(v0, v1, self.spec.easing.eval(local)));
            }
        }
        Some(series[series.len() - 1].1.clone())
    }

    fn finished(&self, now: u128) -> bool {
        if let IterationCount::Num(n) = self.spec.iteration {
            let elapsed = match self.frozen {
                Some(f) => f.saturating_sub(self.start_ms),
                None => now.saturating_sub(self.start_ms),
            } as f64;
            let total = self.spec.delay_ms + self.spec.duration_ms * n.max(0.0);
            return elapsed >= total;
        }
        false
    }
}

// ===================== 帧同步 ticker（管理器接线） =====================

impl super::LsmBrowserManager {
    /// 启动动画帧同步 ticker(全进程单实例;`ensure_view` 时挂上)。
    ///
    /// 查看器存活且有活跃动画/过渡时以 40ms 节拍强制重排 + 给跟随页推帧
    /// (人眼看得到动);无活跃动画 500ms 低频探测;hidden 模式(无查看器)
    /// 不渲染不重排,动作/截图路径按当前时钟取值,零额外开销。
    pub(crate) fn ensure_anim_ticker(&self) {
        use std::sync::atomic::Ordering;
        if self.anim_ticker_started.swap(true, Ordering::Relaxed) {
            return;
        }
        let weak = self
            .self_weak
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| std::sync::Weak::new());
        tokio::spawn(async move {
            let mut active = false;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(if active {
                    40
                } else {
                    500
                }))
                .await;
                let Some(mgr) = weak.upgrade() else { break };
                active = mgr.anim_tick().await;
            }
        });
    }

    /// 动画一拍:活跃动画页强制重排(布局取当前时刻值),跟随页推帧。
    ///
    /// 推帧只给「跟随页」(`ViewFollow` 语义):后台侦察页的动画不能劫持人工
    /// 视口(见 38 号文档)。返回「是否仍有**变化中**动画」供节拍调速
    /// (终帧/暂停状态按低频节拍,值已稳定)。
    async fn anim_tick(&self) -> bool {
        let alive = self.view.lock().map(|g| g.is_some()).unwrap_or(false);
        let mut inner = self.inner.lock().await;
        let mut any_active = false;
        let mut any_changing = false;
        for tab in inner.tabs.values_mut() {
            if tab.anim.has_active() {
                any_active = true;
                any_changing |= tab.anim.is_animating();
                if !alive {
                    continue;
                }
                // `ensure_layout_locked` 在活跃动画时无条件重排(57 轮守卫)。
                self.ensure_layout_locked(tab);
            }
        }
        if !alive || !any_active {
            return any_changing && alive;
        }
        let follow = self.view_follow();
        let summaries: Vec<super::page_bars::TabSummary> = inner
            .tabs
            .values()
            .map(|t| super::page_bars::TabSummary {
                id: t.id.clone(),
                foreground: t.foreground,
                headed: t.mode.is_headed(),
            })
            .collect();
        if let Some(id) =
            super::page_bars::pick_display_tab(&summaries, follow.as_ref(), super::now_ms())
        {
            if let Some(tab) = inner.tabs.get(id) {
                if tab.anim.has_active() {
                    self.push_frame(tab);
                }
            }
        }
        any_changing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::browser::style::anim_style::{apply_anim_declaration, AnimDecl};

    fn ctx() -> ValueCtx {
        ValueCtx {
            viewport_width: 1280.0,
            viewport_height: 720.0,
            root_font_size: 16.0,
        }
    }

    fn style_with(decl_src: &str) -> ComputedStyle {
        let mut st = ComputedStyle::default();
        for seg in decl_src.split(';') {
            if let Some((k, v)) = seg.split_once(':') {
                apply_anim_declaration(&mut st.anim, k.trim(), v.trim());
            }
        }
        st
    }

    fn kf(name: &str, body: &str) -> KeyframesMap {
        let mut m = KeyframesMap::new();
        m.insert(name.to_string(), super::super::style::anim_style::parse_keyframes_body(body));
        m
    }

    #[test]
    fn 线性插值透明度与颜色() {
        let a = AnimValue::Num(0.0);
        let b = AnimValue::Num(1.0);
        assert_eq!(lerp_value(&a, &b, 0.5), AnimValue::Num(0.5));
        let c = lerp_value(
            &AnimValue::Color { rgb: [0, 0, 0], a: 255 },
            &AnimValue::Color { rgb: [255, 255, 255], a: 255 },
            0.5,
        );
        assert_eq!(c, AnimValue::Color { rgb: [128, 128, 128], a: 255 });
    }

    #[test]
    fn 透明色预乘插值不发黑() {
        // transparent(预乘为 0)→红:中间帧亮度跟随 alpha,不落到黑。
        let mid = lerp_value(
            &AnimValue::Color { rgb: [0, 0, 0], a: 0 },
            &AnimValue::Color { rgb: [255, 0, 0], a: 255 },
            0.5,
        );
        match mid {
            AnimValue::Color { rgb, a } => {
                assert_eq!(a, 128);
                assert!(rgb[0] > 240, "红通道应保持饱和,实际 {:?}", rgb);
            }
            _ => panic!("类型错"),
        }
    }

    #[test]
    fn transform操作族内线性() {
        let a = AnimValue::Transform(vec![TransformOp::Scale(1.0, 1.0)]);
        let b = AnimValue::Transform(vec![TransformOp::Scale(2.0, 2.0)]);
        match lerp_value(&a, &b, 0.5) {
            AnimValue::Transform(ops) => match &ops[0] {
                TransformOp::Scale(x, y) => {
                    assert!((x - 1.5).abs() < 1e-9 && (y - 1.5).abs() < 1e-9)
                }
                _ => panic!("族错"),
            },
            _ => panic!("类型错"),
        }
        // 族不匹配:50% 离散。
        let c = AnimValue::Transform(vec![TransformOp::Rotate(90.0)]);
        assert_eq!(lerp_value(&a, &c, 0.4), a);
        assert_eq!(lerp_value(&a, &c, 0.6), c);
    }

    #[test]
    fn 关键帧动画中途取值与终值fill() {
        let eng = AnimEngine::new();
        eng.set_clock_override(Some(1000));
        let mut st = style_with("animation: fade 1s linear both");
        st.opacity = 0.3; // 底层值
        let kfs = kf("fade", "from { opacity: 0 } to { opacity: 1 }");
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        // t=0:0%
        assert!((s.opacity - 0.0).abs() < 1e-6, "t0={}", s.opacity);
        eng.set_clock_override(Some(1500));
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        assert!((s.opacity - 0.5).abs() < 1e-3, "t500={}", s.opacity);
        // 结束 + fill forwards → 终值;fill 为 both。
        eng.set_clock_override(Some(2100));
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        assert!((s.opacity - 1.0).abs() < 1e-6, "t_end={}", s.opacity);
    }

    #[test]
    fn 无fill结束后回落底层值() {
        let eng = AnimEngine::new();
        eng.set_clock_override(Some(0));
        let mut st = style_with("animation: fade 1s linear");
        st.opacity = 0.3;
        let kfs = kf("fade", "from { opacity: 0 } to { opacity: 1 }");
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx()); // 启动
        eng.set_clock_override(Some(1500));
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        assert!((s.opacity - 0.3).abs() < 1e-6, "回落值={}", s.opacity);
    }

    #[test]
    fn 交错方向与迭代折返() {
        let eng = AnimEngine::new();
        eng.set_clock_override(Some(0));
        let mut st = style_with("animation: fade 1s linear 2 alternate");
        let kfs = kf("fade", "from { opacity: 0 } to { opacity: 1 }");
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx()); // 启动
        // 第二轮后半:alternate 反向,位置 1-frac。
        eng.set_clock_override(Some(1750)); // active=1750ms → iter=1, frac=0.75 → p=0.25
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        assert!((s.opacity - 0.25).abs() < 1e-3, "交错值={}", s.opacity);
    }

    #[test]
    fn 过渡触发与重定向() {
        let eng = AnimEngine::new();
        eng.set_clock_override(Some(0));
        let kfs = KeyframesMap::new();
        let mut st = style_with("transition: opacity 1s linear");
        st.opacity = 0.0;
        eng.track_and_apply("x", &mut st, &kfs, &ctx());
        // 变更到 1.0:同刻启动,值仍 0。
        st.opacity = 1.0;
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        assert!((s.opacity - 0.0).abs() < 1e-6);
        // 半程 0.5。
        eng.set_clock_override(Some(500));
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        assert!((s.opacity - 0.5).abs() < 1e-3, "中途={}", s.opacity);
        // 中途改目标到 0.2 → 从 0.5 重定向,新目标 0.2。
        st.opacity = 0.2;
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        assert!((s.opacity - 0.5).abs() < 1e-3);
        eng.set_clock_override(Some(1000));
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        assert!((s.opacity - 0.35).abs() < 1e-3, "重定向中点={}", s.opacity);
    }

    #[test]
    fn 暂停冻结与恢复平移() {
        let eng = AnimEngine::new();
        eng.set_clock_override(Some(0));
        let mut st = style_with("animation: fade 1s linear paused");
        st.opacity = 0.0;
        let kfs = kf("fade", "from { opacity: 0 } to { opacity: 1 }");
        eng.set_clock_override(Some(400));
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        // paused:显示 delay 后?delay=0,active=400 但 paused 冻结在 0 → 值 0。
        // 冻结语义:从启动起冻结,值停在起点。
        let frozen_val = s.opacity;
        eng.set_clock_override(Some(900));
        let mut s2 = st.clone();
        eng.track_and_apply("x", &mut s2, &kfs, &ctx());
        assert!((s2.opacity - frozen_val).abs() < 1e-6, "暂停应冻结");
        // 恢复:时长继续累进。
        st.anim.animations[0].play_state = PlayState::Running;
        let mut s3 = st.clone();
        eng.track_and_apply("x", &mut s3, &kfs, &ctx());
        eng.set_clock_override(Some(1400));
        let mut s4 = st.clone();
        eng.track_and_apply("x", &mut s4, &kfs, &ctx());
        // 恢复后过了 500ms → 0.5。
        assert!((s4.opacity - 0.5).abs() < 1e-3, "恢复后={}", s4.opacity);
    }

    #[test]
    fn 关键帧缺端点用底层值补齐() {
        let eng = AnimEngine::new();
        eng.set_clock_override(Some(0));
        let mut st = style_with("animation: half 1s linear both");
        st.opacity = 0.8;
        // 只有 to:{opacity:1},from 缺 → 底层 0.8。
        let kfs = kf("half", "to { opacity: 1 }");
        eng.set_clock_override(Some(0));
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        assert!((s.opacity - 0.8).abs() < 1e-6, "起端={}", s.opacity);
        eng.set_clock_override(Some(500));
        let mut s = st.clone();
        eng.track_and_apply("x", &mut s, &kfs, &ctx());
        assert!((s.opacity - 0.9).abs() < 1e-3, "中点={}", s.opacity);
    }

    #[test]
    fn 只读覆盖不推进状态() {
        let eng = AnimEngine::new();
        eng.set_clock_override(Some(0));
        let mut st = style_with("animation: fade 1s linear both");
        st.opacity = 0.0;
        let kfs = kf("fade", "from { opacity: 0 } to { opacity: 1 }");
        eng.track_and_apply("x", &mut st, &kfs, &ctx());
        eng.set_clock_override(Some(750));
        let mut s = st.clone();
        eng.apply_overrides("x", &mut s);
        assert!((s.opacity - 0.75).abs() < 1e-3);
        // 再次观察:值不因 apply_overrides 漂移。
        let mut s2 = st.clone();
        eng.track_and_apply("x", &mut s2, &kfs, &ctx());
        assert!((s2.opacity - 0.75).abs() < 1e-3);
    }
}
