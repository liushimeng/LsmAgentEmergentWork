//! CSS 渐变函数解析(阶段六第一步:`linear-gradient` / `radial-gradient`)。
//!
//! 产出与渲染无关的纯数据 `GradientSpec`;光栅化层(`render/raster.rs`)按盒
//! 尺寸求值。支持的语法子集(真实站点主流写法):
//! - 前缀:`linear-gradient(` / `-webkit-linear-gradient(` / `radial-gradient(`
//!   / `-webkit-radial-gradient(` / `repeating-*`(按普通渐变 Pad 近似);
//! - 线性方向:`to right` 系列(可双轴)与 `Ndeg`(CSS 标准:0deg 向上、顺时针),
//!   缺省 180deg(to bottom);`-webkit-` 裸 deg 角度与标准同式处理(近似);
//! - 径向:`circle` / `ellipse` 形状(统一按椭圆画)、`farthest-corner`(缺省)
//!   与 `farthest-side`、`at center`(其余位置关键字近似为 center);
//! - 色标:`<color> [ <percentage> | <0-1> ]?`,双位置语法(`red 0% 50%`),
//!   无位置的色标均匀分布;颜色复用 `values::parse_color`。
//!
//! 已知近似(见 22 号设计文档「边界」):`turn`/`rad` 单位、`closest-*` 尺寸、
//! 非 center 位置、`in oklab` 插值均按缺省语义近似;多背景层只取首层。

use super::values::parse_color;

/// 单个色标。`pos` 为 0..=1 的归一位置(解析期把百分比/单位数归一)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub pos: f64,
    pub color: ([u8; 3], u8),
}

/// 渐变形状。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradientKind {
    /// 线性渐变。`angle_deg` 为 CSS 标准角:0deg = 向上,顺时针增。
    Linear { angle_deg: f64 },
    /// 径向渐变。`farthest_side=false` 为 farthest-corner(缺省),
    /// `true` 为 farthest-side。
    Radial { farthest_side: bool },
}

/// 渐变规格(计算样式持有;布局期克隆进显示项)。
#[derive(Debug, Clone, PartialEq)]
pub struct GradientSpec {
    /// 原始函数文本(`getComputedStyle`  fidelity 用)。
    pub raw: String,
    pub kind: GradientKind,
    pub stops: Vec<GradientStop>,
}

/// 解析渐变函数文本;非渐变(或解析失败)返回 `None`。
pub fn parse_gradient(text: &str) -> Option<GradientSpec> {
    let raw = text.trim();
    let low = raw.to_ascii_lowercase();
    let (kind_prefix, body) = if let Some(b) = strip_fn(&low, "repeating-linear-gradient(") {
        (FnKind::Linear, b)
    } else if let Some(b) = strip_fn(&low, "linear-gradient(") {
        (FnKind::Linear, b)
    } else if let Some(b) = strip_fn(&low, "-webkit-linear-gradient(") {
        (FnKind::Linear, b)
    } else if let Some(b) = strip_fn(&low, "repeating-radial-gradient(") {
        (FnKind::Radial, b)
    } else if let Some(b) = strip_fn(&low, "radial-gradient(") {
        (FnKind::Radial, b)
    } else if let Some(b) = strip_fn(&low, "-webkit-radial-gradient(") {
        (FnKind::Radial, b)
    } else {
        return None;
    };
    // 末层 `)` 由 strip_fn 留在 body 里,这里去掉后再做顶层逗号切分。
    let body = body.trim().strip_suffix(')').unwrap_or(body.trim());
    let args = split_top_level(body);
    if args.is_empty() {
        return None;
    }
    let (kind, stop_start) = match kind_prefix {
        FnKind::Linear => parse_linear_head(&args)?,
        FnKind::Radial => parse_radial_head(&args)?,
    };
    let mut stops = parse_stops(&args[stop_start.min(args.len())..])?;
    normalize_stop_positions(&mut stops);
    if stops.len() < 2 {
        // 单色渐变按两份同色色标补齐(渲染层退化为纯色填充)。
        let only = stops.first()?.color;
        stops = vec![
            GradientStop { pos: 0.0, color: only },
            GradientStop { pos: 1.0, color: only },
        ];
    }
    Some(GradientSpec {
        raw: raw.to_string(),
        kind,
        stops,
    })
}

#[derive(Clone, Copy)]
enum FnKind {
    Linear,
    Radial,
}

/// 前缀命中则返回函数体(保留末层 `)`)。
fn strip_fn<'a>(low: &'a str, prefix: &str) -> Option<&'a str> {
    low.starts_with(prefix).then(|| &low[prefix.len()..])
}

/// 顶层逗号切分(括号内的逗号不切;`rgba(...)` 等函数参数安全)。
fn split_top_level(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for ch in text.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur = String::new();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// 顶层空白切分(括号内的空白不切;`rgb(1, 2, 3)` 保持单 token)。
fn split_ws_top_level(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for ch in text.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur = String::new();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// 线性渐变头部:返回(形状, 色标起始下标)。首个参数是角度/方向时消费之。
fn parse_linear_head(args: &[String]) -> Option<(GradientKind, usize)> {
    let first = args.first()?.to_ascii_lowercase();
    if let Some(rest) = first.strip_prefix("to ") {
        // to <side> [+ <side>]:右=90deg、下=180deg、左=270deg、上=0deg;
        // 对角(如 to bottom right)取两轴合成角。
        let (mut dx, mut dy) = (0.0f64, 0.0f64);
        for side in rest.split_whitespace() {
            match side {
                "left" => dx -= 1.0,
                "right" => dx += 1.0,
                "top" => dy -= 1.0,
                "bottom" => dy += 1.0,
                _ => return Some((GradientKind::Linear { angle_deg: 180.0 }, 1)),
            }
        }
        if dx == 0.0 && dy == 0.0 {
            return Some((GradientKind::Linear { angle_deg: 180.0 }, 1));
        }
        // CSS 角:0deg 向上、顺时针;atan2(x, -y) 把屏幕向量映射到该角制。
        let angle = dx.atan2(-dy).to_degrees();
        return Some((
            GradientKind::Linear {
                angle_deg: (angle + 360.0) % 360.0,
            },
            1,
        ));
    }
    if let Some(deg) = first.strip_suffix("deg") {
        if let Ok(v) = deg.trim().parse::<f64>() {
            return Some((
                GradientKind::Linear {
                    angle_deg: v.rem_euclid(360.0),
                },
                1,
            ));
        }
    }
    if first.ends_with("turn") || first.ends_with("rad") {
        // turn/rad 单位按缺省 to bottom 近似。
        return Some((GradientKind::Linear { angle_deg: 180.0 }, 1));
    }
    // 无方向参数:180deg(to bottom),全部参数是色标。
    Some((GradientKind::Linear { angle_deg: 180.0 }, 0))
}

/// 径向渐变头部:形状/尺寸/位置参数在色标之前,逐个识别。
/// 参数按空白分词后**全部**命中头部关键字才算头部(如 `ellipse farthest-side`
/// / `circle at center` / `at 50% 50%`);出现任何非关键字即视为色标起点。
fn parse_radial_head(args: &[String]) -> Option<(GradientKind, usize)> {
    let mut farthest_side = false;
    let mut idx = 0usize;
    while idx < args.len() {
        let a = args[idx].to_ascii_lowercase();
        let toks: Vec<&str> = a.split_whitespace().collect();
        let is_head = !toks.is_empty()
            && toks.iter().all(|t| {
                matches!(
                    *t,
                    "circle"
                        | "ellipse"
                        | "closest-side"
                        | "farthest-side"
                        | "closest-corner"
                        | "farthest-corner"
                        | "center"
                        | "at"
                ) || t.ends_with("px")
                    || t.ends_with("rem")
                    || t.ends_with('%')
            });
        if !is_head {
            break;
        }
        if toks
            .iter()
            .any(|t| *t == "farthest-side" || t.contains("closest"))
        {
            // farthest-side 直取;closest-* 按较保守的 farthest-side 近似。
            farthest_side = true;
        }
        idx += 1;
    }
    if idx >= args.len() {
        return None;
    }
    Some((GradientKind::Radial { farthest_side }, idx))
}

/// 色标解析:`<color> [pos] [pos]?`(双位置语法展开为两个同色色标)。
fn parse_stops(args: &[String]) -> Option<Vec<GradientStop>> {
    let mut raw_stops: Vec<(Option<f64>, ([u8; 3], u8))> = Vec::new();
    for arg in args {
        // 双位置语法:`red 0% 50%`;颜色可能是 `rgb(1, 2, 3)` 这类含空格函数,
        // 空白切分必须同样尊重括号嵌套。
        let toks = split_ws_top_level(arg);
        let mut color: Option<([u8; 3], u8)> = None;
        let mut positions: Vec<f64> = Vec::new();
        let mut color_token_len = 0usize;
        // 颜色可能含空格(rgb(1, 2, 3) 已被顶层切分压成单 token,此处无需再拼)。
        for (i, t) in toks.iter().enumerate() {
            if let Some(Some(c)) = parse_color(t) {
                color = Some(c);
                color_token_len = i + 1;
                break;
            }
        }
        let color = color?;
        for t in &toks[color_token_len..] {
            if let Some(p) = parse_position(t) {
                positions.push(p);
            }
        }
        match positions.len() {
            0 => raw_stops.push((None, color)),
            1 => raw_stops.push((Some(positions[0]), color)),
            _ => {
                raw_stops.push((Some(positions[0]), color));
                raw_stops.push((Some(positions[1]), color));
            }
        }
    }
    if raw_stops.is_empty() {
        return None;
    }
    // 缺省位置均匀分布:位于已定位置之间按序插值。
    let mut out = Vec::with_capacity(raw_stops.len());
    let n = raw_stops.len();
    for (i, (pos, color)) in raw_stops.into_iter().enumerate() {
        let p = match pos {
            Some(p) => p,
            None => default_position(i, n),
        };
        out.push(GradientStop {
            pos: p.clamp(0.0, 1.0),
            color,
        });
    }
    Some(out)
}

/// 无位置色标的缺省位置:两端固定 0/1,中间 i/(n-1)。
fn default_position(i: usize, n: usize) -> f64 {
    if n <= 1 {
        return 0.0;
    }
    if i == 0 {
        0.0
    } else if i == n - 1 {
        1.0
    } else {
        i as f64 / (n - 1) as f64
    }
}

/// 色标位置归一:升序排序(允许原始乱序的站点写法)、首尾补 0/1。
fn normalize_stop_positions(stops: &mut [GradientStop]) {
    stops.sort_by(|a, b| a.pos.partial_cmp(&b.pos).unwrap_or(std::cmp::Ordering::Equal));
    if let Some(first) = stops.first_mut() {
        first.pos = 0.0;
    }
    if let Some(last) = stops.last_mut() {
        last.pos = 1.0;
    }
}

/// 位置取值:`50%` → 0.5;`0.5`(单位数)→ 0.5;px 长度无法预知盒尺寸,返回 None
/// (该色标按缺省分布处理)。
fn parse_position(t: &str) -> Option<f64> {
    let t = t.trim();
    if let Some(pct) = t.strip_suffix('%') {
        let v = pct.trim().parse::<f64>().ok()?;
        return Some((v / 100.0).clamp(0.0, 1.0));
    }
    if let Ok(v) = t.parse::<f64>() {
        if (0.0..=1.0).contains(&v) {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 线性渐变_to_right_角度() {
        let g = parse_gradient("linear-gradient(to right, #ff0000, #0000ff)").unwrap();
        assert_eq!(g.kind, GradientKind::Linear { angle_deg: 90.0 });
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].pos, 0.0);
        assert_eq!(g.stops[0].color, ([255, 0, 0], 255));
        assert_eq!(g.stops[1].pos, 1.0);
    }

    #[test]
    fn 线性渐变_度数与缺省() {
        let g = parse_gradient("linear-gradient(135deg, red 0%, blue 100%)").unwrap();
        assert_eq!(g.kind, GradientKind::Linear { angle_deg: 135.0 });
        let g = parse_gradient("linear-gradient(red, blue)").unwrap();
        assert_eq!(g.kind, GradientKind::Linear { angle_deg: 180.0 });
        let g = parse_gradient("-webkit-linear-gradient(red, blue)").unwrap();
        assert_eq!(g.kind, GradientKind::Linear { angle_deg: 180.0 });
    }

    #[test]
    fn 线性渐变_对角方向() {
        let g = parse_gradient("linear-gradient(to bottom right, red, blue)").unwrap();
        assert_eq!(g.kind, GradientKind::Linear { angle_deg: 135.0 });
        let g = parse_gradient("linear-gradient(to top, red, blue)").unwrap();
        assert_eq!(g.kind, GradientKind::Linear { angle_deg: 0.0 });
    }

    #[test]
    fn 线性渐变_色标缺省分布与双位置() {
        let g = parse_gradient("linear-gradient(to right, red, green, blue)").unwrap();
        assert_eq!(g.stops[1].pos, 0.5);
        let g = parse_gradient("linear-gradient(to right, red 0% 50%, blue 100%)").unwrap();
        assert_eq!(g.stops.len(), 3);
        assert_eq!(g.stops[1].pos, 0.5);
        assert_eq!(g.stops[1].color, ([255, 0, 0], 255));
    }

    #[test]
    fn 线性渐变_rgba_色标() {
        let g = parse_gradient("linear-gradient(to right, rgba(255,0,0,0.5), rgba(0,0,255,1))")
            .unwrap();
        assert_eq!(g.stops[0].color, ([255, 0, 0], 128));
        assert_eq!(g.stops[1].color.1, 255);
    }

    #[test]
    fn 径向渐变_头部参数() {
        let g = parse_gradient("radial-gradient(circle at center, red, blue)").unwrap();
        assert_eq!(
            g.kind,
            GradientKind::Radial { farthest_side: false }
        );
        let g = parse_gradient("radial-gradient(ellipse farthest-side, red, blue)").unwrap();
        assert_eq!(g.kind, GradientKind::Radial { farthest_side: true });
        let g = parse_gradient("-webkit-radial-gradient(center, red, blue)").unwrap();
        assert_eq!(g.stops.len(), 2);
    }

    #[test]
    fn repeating_前缀按普通渐变() {
        let g = parse_gradient("repeating-linear-gradient(to right, red, blue)").unwrap();
        assert!(matches!(g.kind, GradientKind::Linear { .. }));
    }

    #[test]
    fn 非渐变文本返回_none() {
        assert!(parse_gradient("url(a.png)").is_none());
        assert!(parse_gradient("none").is_none());
        assert!(parse_gradient("solid").is_none());
        assert!(parse_gradient("linear-gradient(").is_none());
    }

    #[test]
    fn 单色渐变补齐为两份同色() {
        let g = parse_gradient("linear-gradient(to right, #123456)").unwrap();
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].color, g.stops[1].color);
    }
}
