//! CSS 2D transform 解析:仿射矩阵 + transform-origin。
//!
//! `transform` 的百分比平移相对元素**自身盒尺寸**,样式层无从得知,故解析为
//! **操作序列**(`TransformOp`),布局期按盒尺寸求解为 `Affine`;`transform-origin`
//! 同理留到布局期求解。设计见 `docs/MCP_Web_LsmAgentBrowser_Use/03-布局与渲染引擎/23-CSS2D变换与文本阴影与多背景层设计.md`。

use super::values::{parse_length, parse_percent, ValueCtx};

/// 长度:像素或百分比(百分比相对元素自身盒尺寸,布局期求解)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Px(f64),
    Pct(f64),
}

impl Length {
    /// 按盒尺寸求解为像素。
    pub fn resolve(self, box_size: f64) -> f64 {
        match self {
            Length::Px(v) => v,
            Length::Pct(p) => p / 100.0 * box_size,
        }
    }
}

/// 2×3 仿射矩阵(CSS `matrix(a,b,c,d,e,f)` 同构)。
///
/// 列向量约定:`x' = a*x + c*y + e` ; `y' = b*x + d*y + f`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const fn identity() -> Self {
        Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    /// 逆变换;行列式近零(退化)返回 `None`。
    pub fn invert(&self) -> Option<Self> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        Some(Self {
            a: self.d / det,
            b: -self.b / det,
            c: -self.c / det,
            d: self.a / det,
            e: (self.c * self.f - self.d * self.e) / det,
            f: (self.b * self.e - self.a * self.f) / det,
        })
    }

    /// 合成 `self ∘ other`(先作用 `other`,再作用 `self`)。
    pub fn compose(&self, other: &Self) -> Self {
        Self {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn translate(x: f64, y: f64) -> Self {
        Self { e: x, f: y, ..Self::identity() }
    }

    pub fn scale(sx: f64, sy: f64) -> Self {
        Self { a: sx, d: sy, ..Self::identity() }
    }

    /// CSS `rotate(deg)`:顺时针为正。
    pub fn rotate_deg(deg: f64) -> Self {
        let r = deg.to_radians();
        let (s, c) = (r.sin(), r.cos());
        Self { a: c, b: s, c: -s, d: c, ..Self::identity() }
    }

    /// CSS `skew(ax, ay)`:`matrix(1, tan(ay), tan(ax), 1, 0, 0)`。
    pub fn skew_deg(ax: f64, ay: f64) -> Self {
        let (tx, ty) = (ax.to_radians().tan(), ay.to_radians().tan());
        Self { b: ty, c: tx, ..Self::identity() }
    }
}

/// 单条 transform 操作(数值已解析,长度待布局期按盒尺寸求解)。
#[derive(Debug, Clone, PartialEq)]
pub enum TransformOp {
    Translate(Length, Length),
    TranslateX(Length),
    TranslateY(Length),
    /// 角度(度)。
    Rotate(f64),
    Scale(f64, f64),
    ScaleX(f64),
    ScaleY(f64),
    /// 角度(度)。
    Skew(f64, f64),
    SkewX(f64),
    SkewY(f64),
    Matrix(f64, f64, f64, f64, Length, Length),
}

impl TransformOp {
    fn to_affine(self, box_w: f64, box_h: f64) -> Affine {
        match self {
            TransformOp::Translate(x, y) => {
                Affine::translate(x.resolve(box_w), y.resolve(box_h))
            }
            TransformOp::TranslateX(x) => Affine::translate(x.resolve(box_w), 0.0),
            TransformOp::TranslateY(y) => Affine::translate(0.0, y.resolve(box_h)),
            TransformOp::Rotate(deg) => Affine::rotate_deg(deg),
            TransformOp::Scale(sx, sy) => Affine::scale(sx, sy),
            TransformOp::ScaleX(sx) => Affine::scale(sx, 1.0),
            TransformOp::ScaleY(sy) => Affine::scale(1.0, sy),
            TransformOp::Skew(ax, ay) => Affine::skew_deg(ax, ay),
            TransformOp::SkewX(ax) => Affine::skew_deg(ax, 0.0),
            TransformOp::SkewY(ay) => Affine::skew_deg(0.0, ay),
            TransformOp::Matrix(a, b, c, d, e, f) => Affine {
                a,
                b,
                c,
                d,
                e: e.resolve(box_w),
                f: f.resolve(box_h),
            },
        }
    }
}

/// `transform` 计算值:操作序列。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transform2D {
    pub ops: Vec<TransformOp>,
}

impl Transform2D {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// 按盒尺寸求解为仿射矩阵(**不含** transform-origin)。
    ///
    /// 操作按声明序左乘:`translate(...) rotate(...)` ⇒ 先平移后旋转。
    pub fn resolve(&self, box_w: f64, box_h: f64) -> Affine {
        let mut m = Affine::identity();
        for op in &self.ops {
            let om = op.clone().to_affine(box_w, box_h);
            m = om.compose(&m);
        }
        m
    }

    /// 含 transform-origin 的完整页坐标变换。
    ///
    /// `T(p) = origin + M·(p - origin)`,即 `translate(origin) ∘ M ∘ translate(-origin)`。
    pub fn resolve_with_origin(
        &self,
        box_x: f64,
        box_y: f64,
        box_w: f64,
        box_h: f64,
        origin: TransformOrigin,
    ) -> Affine {
        let m = self.resolve(box_w, box_h);
        let ox = box_x + origin.x.resolve(box_w);
        let oy = box_y + origin.y.resolve(box_h);
        // T(p) = origin + M·(p - origin) = A·p + (origin - A·origin + t)
        // 其中 M = (A | t):线性部分取自 M,平移项须**保留** M 自身平移 t。
        Affine {
            a: m.a,
            b: m.b,
            c: m.c,
            d: m.d,
            e: ox - (m.a * ox + m.c * oy) + m.e,
            f: oy - (m.b * ox + m.d * oy) + m.f,
        }
    }

    /// 序列化回 CSS 函数串(诊断/inspect 用;百分比保持原样)。
    pub fn to_css(&self) -> String {
        fn len(l: Length) -> String {
            match l {
                Length::Px(v) => format!("{}px", crate::agent::browser::style::values::num(v)),
                Length::Pct(p) => format!("{}%", crate::agent::browser::style::values::num(p)),
            }
        }
        let mut parts = Vec::with_capacity(self.ops.len());
        for op in &self.ops {
            parts.push(match op {
                TransformOp::Translate(x, y) => format!("translate({}, {})", len(*x), len(*y)),
                TransformOp::TranslateX(x) => format!("translateX({})", len(*x)),
                TransformOp::TranslateY(y) => format!("translateY({})", len(*y)),
                TransformOp::Rotate(d) => format!("rotate({}deg)", crate::agent::browser::style::values::num(*d)),
                TransformOp::Scale(x, y) => format!("scale({}, {})", crate::agent::browser::style::values::num(*x), crate::agent::browser::style::values::num(*y)),
                TransformOp::ScaleX(x) => format!("scaleX({})", crate::agent::browser::style::values::num(*x)),
                TransformOp::ScaleY(y) => format!("scaleY({})", crate::agent::browser::style::values::num(*y)),
                TransformOp::Skew(x, y) => format!("skew({}deg, {}deg)", crate::agent::browser::style::values::num(*x), crate::agent::browser::style::values::num(*y)),
                TransformOp::SkewX(x) => format!("skewX({}deg)", crate::agent::browser::style::values::num(*x)),
                TransformOp::SkewY(y) => format!("skewY({}deg)", crate::agent::browser::style::values::num(*y)),
                TransformOp::Matrix(a, b, c, d, e, f) => format!(
                    "matrix({}, {}, {}, {}, {}, {})",
                    crate::agent::browser::style::values::num(*a),
                    crate::agent::browser::style::values::num(*b),
                    crate::agent::browser::style::values::num(*c),
                    crate::agent::browser::style::values::num(*d),
                    len(*e),
                    len(*f)
                ),
            });
        }
        parts.join(" ")
    }
}

/// `transform-origin` 计算值(默认 `50% 50%`)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransformOrigin {
    pub x: Length,
    pub y: Length,
}

impl Default for TransformOrigin {
    fn default() -> Self {
        Self { x: Length::Pct(50.0), y: Length::Pct(50.0) }
    }
}

// ===================== 解析 =====================

/// 解析 `transform`:`none`/空返回 `None`;无法识别的函数跳过(不整条作废)。
pub fn parse_transform(value: &str) -> Option<Transform2D> {
    let v = value.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("none") {
        return None;
    }
    let mut ops = Vec::new();
    let mut rest = v;
    while !rest.is_empty() {
        let Some(open) = rest.find('(') else { break };
        let name = rest[..open].trim().to_ascii_lowercase();
        if name.is_empty() {
            break;
        }
        let Some(close) = matching_paren(rest, open) else { break };
        let args = &rest[open + 1..close];
        if let Some(op) = parse_op(&name, args) {
            ops.push(op);
        }
        rest = rest[close + 1..].trim_start();
    }
    if ops.is_empty() {
        None
    } else {
        Some(Transform2D { ops })
    }
}

/// 解析 `transform-origin`:1~2 值(关键字 / 长度 / 百分比),缺省 `50% 50%`。
pub fn parse_transform_origin(value: &str, _font_size: f64, _ctx: &ValueCtx) -> TransformOrigin {
    let toks = split_top_ws(value);
    let keyword = |t: &str| -> Option<Length> {
        match t.trim().to_ascii_lowercase().as_str() {
            "left" | "top" => Some(Length::Pct(0.0)),
            "right" | "bottom" => Some(Length::Pct(100.0)),
            "center" => Some(Length::Pct(50.0)),
            _ => None,
        }
    };
    let one = |t: &str| -> Length { keyword(t).or_else(|| parse_length_token(t)).unwrap_or(Length::Pct(50.0)) };
    match toks.len() {
        0 => TransformOrigin::default(),
        1 => {
            let v = &toks[0];
            // 单值:垂直关键字(top/bottom)作用于 y、水平关键字只作用于 x;
            // 其余(长度/百分比/center)作用于 x,y 取 50%。
            match keyword(v) {
                Some(k) if matches!(v.trim().to_ascii_lowercase().as_str(), "top" | "bottom") => {
                    TransformOrigin { x: Length::Pct(50.0), y: k }
                }
                Some(k) => TransformOrigin { x: k, y: Length::Pct(50.0) },
                None => TransformOrigin { x: one(v), y: Length::Pct(50.0) },
            }
        }
        _ => TransformOrigin { x: one(&toks[0]), y: one(&toks[1]) },
    }
}

fn parse_op(name: &str, args: &str) -> Option<TransformOp> {
    match name {
        "translate" => {
            let toks = split_top_ws(args);
            if toks.is_empty() {
                return None;
            }
            let x = parse_length_token(&toks[0])?;
            let y = toks.get(1).and_then(|t| parse_length_token(t)).unwrap_or(Length::Px(0.0));
            Some(TransformOp::Translate(x, y))
        }
        "translatex" => split_top_ws(args).first().and_then(|t| parse_length_token(t)).map(TransformOp::TranslateX),
        "translatey" => split_top_ws(args).first().and_then(|t| parse_length_token(t)).map(TransformOp::TranslateY),
        "rotate" => split_top_ws(args).first().and_then(|t| parse_angle(t)).map(TransformOp::Rotate),
        "scale" => {
            let toks = split_top_ws(args);
            let sx = toks.first()?.trim().parse::<f64>().ok()?;
            let sy = toks.get(1).and_then(|t| t.trim().parse::<f64>().ok()).unwrap_or(sx);
            Some(TransformOp::Scale(sx, sy))
        }
        "scalex" => split_top_ws(args).first()?.trim().parse::<f64>().ok().map(TransformOp::ScaleX),
        "scaley" => split_top_ws(args).first()?.trim().parse::<f64>().ok().map(TransformOp::ScaleY),
        "skew" => {
            let toks = split_top_ws(args);
            let ax = toks.first().and_then(|t| parse_angle(t))?;
            let ay = toks.get(1).and_then(|t| parse_angle(t)).unwrap_or(0.0);
            Some(TransformOp::Skew(ax, ay))
        }
        "skewx" => split_top_ws(args).first().and_then(|t| parse_angle(t)).map(TransformOp::SkewX),
        "skewy" => split_top_ws(args).first().and_then(|t| parse_angle(t)).map(TransformOp::SkewY),
        "matrix" => {
            let toks = split_top_ws(args);
            if toks.len() < 6 {
                return None;
            }
            let nums: Vec<f64> = toks.iter().filter_map(|t| t.trim().parse::<f64>().ok()).collect();
            if nums.len() < 6 {
                return None;
            }
            let e = parse_length_token(&toks[4]).unwrap_or(Length::Px(0.0));
            let f = parse_length_token(&toks[5]).unwrap_or(Length::Px(0.0));
            Some(TransformOp::Matrix(nums[0], nums[1], nums[2], nums[3], e, f))
        }
        _ => None,
    }
}

/// 解析角度:`deg` / `grad` / `rad` / `turn`,统一为度;无单位返回 `None`。
fn parse_angle(t: &str) -> Option<f64> {
    let t = t.trim().to_ascii_lowercase();
    if let Some(v) = t.strip_suffix("deg") {
        return v.trim().parse().ok();
    }
    if let Some(v) = t.strip_suffix("grad") {
        return v.trim().parse::<f64>().ok().map(|v| v * 360.0 / 400.0);
    }
    if let Some(v) = t.strip_suffix("rad") {
        return v.trim().parse::<f64>().ok().map(|v| v.to_degrees());
    }
    if let Some(v) = t.strip_suffix("turn") {
        return v.trim().parse::<f64>().ok().map(|v| v * 360.0);
    }
    None
}

/// 长度/百分比分量:`50%` → `Pct`;否则按像素长度解析。
fn parse_length_token(t: &str) -> Option<Length> {
    let t = t.trim();
    if let Some(p) = parse_percent(t) {
        return Some(Length::Pct(p));
    }
    parse_length(t, 16.0, &ValueCtx::default()).map(Length::Px)
}

/// `open` 处括号的配对下标;未闭合返回 `None`。
fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// 顶层参数切分:空白与逗号均可分隔(括号内不切开)。
///
/// transform 函数参数既可用逗号(`matrix(1,0,0,1,5,5)`)也可用空白
/// (`translate(10px 20px)`),故两者都作分隔符。
fn split_top_ws(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            c if (c.is_whitespace() || c == ',') && depth <= 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 平移矩阵() {
        let t = parse_transform("translate(10px, 20px)").unwrap();
        let m = t.resolve(100.0, 100.0);
        assert!((m.e - 10.0).abs() < 1e-9);
        assert!((m.f - 20.0).abs() < 1e-9);
    }

    #[test]
    fn 百分比平移按盒尺寸求解() {
        let t = parse_transform("translate(50%, 25%)").unwrap();
        let m = t.resolve(200.0, 80.0);
        assert!((m.e - 100.0).abs() < 1e-9);
        assert!((m.f - 20.0).abs() < 1e-9);
    }

    #[test]
    fn 旋转与缩放() {
        let r = parse_transform("rotate(90deg)").unwrap().resolve(10.0, 10.0);
        let (x, y) = r.apply(1.0, 0.0);
        assert!(x.abs() < 1e-9 && (y - 1.0).abs() < 1e-9, "rotate90 (1,0)→(0,1):({x},{y})");
        let s = parse_transform("scale(2, 3)").unwrap().resolve(10.0, 10.0);
        assert!((s.a - 2.0).abs() < 1e-9 && (s.d - 3.0).abs() < 1e-9);
    }

    #[test]
    fn 多函数按声明序合成() {
        // translate(10px,0) rotate(90deg):先平移后旋转 → (10,0) 转 90° → (0,10)
        let m = parse_transform("translate(10px, 0) rotate(90deg)").unwrap().resolve(10.0, 10.0);
        let (x, y) = m.apply(0.0, 0.0);
        assert!(x.abs() < 1e-9 && (y - 10.0).abs() < 1e-9, "合成序:({x},{y})");
    }

    #[test]
    fn transform_origin_默认与关键字() {
        let d = TransformOrigin::default();
        assert_eq!(d.x, Length::Pct(50.0));
        let o = parse_transform_origin("left top", 16.0, &ValueCtx::default());
        assert_eq!(o.x, Length::Pct(0.0));
        assert_eq!(o.y, Length::Pct(0.0));
        let o = parse_transform_origin("10px 20px", 16.0, &ValueCtx::default());
        assert_eq!(o.x, Length::Px(10.0));
        assert_eq!(o.y, Length::Px(20.0));
    }

    #[test]
    fn 原点绕转角旋转() {
        // 100×100 盒,原点 left top,rotate(90deg):右上角 (100,0) 顺时针 → (0,100)
        let t = parse_transform("rotate(90deg)").unwrap();
        let o = parse_transform_origin("left top", 16.0, &ValueCtx::default());
        let m = t.resolve_with_origin(0.0, 0.0, 100.0, 100.0, o);
        let (x, y) = m.apply(100.0, 0.0);
        assert!(x.abs() < 1e-9 && (y - 100.0).abs() < 1e-9, "绕角旋转:({x},{y})");
    }

    #[test]
    fn 含原点平移保留平移量() {
        // translate 绕任意原点仍应是纯平移(平移量不被原点吃掉)。
        let t = parse_transform("translate(10px, 20px)").unwrap();
        let m = t.resolve_with_origin(100.0, 50.0, 80.0, 60.0, TransformOrigin::default());
        assert!(
            (m.e - 10.0).abs() < 1e-9 && (m.f - 20.0).abs() < 1e-9,
            "绕原点平移:({},{})",
            m.e,
            m.f
        );
    }

    #[test]
    fn 非法与_none_返回_none() {
        assert!(parse_transform("none").is_none());
        assert!(parse_transform("").is_none());
        assert!(parse_transform("bogus(1)").is_none());
    }

    #[test]
    fn 逆变换还原() {
        let m = parse_transform("translate(5px, 7px) rotate(30deg) scale(1.5)").unwrap().resolve(10.0, 10.0);
        let inv = m.invert().unwrap();
        let p = (37.0, 91.0);
        let (x, y) = inv.apply(m.apply(p.0, p.1).0, m.apply(p.0, p.1).1);
        assert!((x - p.0).abs() < 1e-6 && (y - p.1).abs() < 1e-6, "逆变换还原:({x},{y})");
    }
}
