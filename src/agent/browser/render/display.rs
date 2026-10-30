//! 绘制显示列表(阶段六第一步):布局期 DFS 产出的**自包含**绘制项序列。
//!
//! 每项自带 `(z, seq, rank)` 三元排序键、透明度与裁剪矩形——祖先组的
//! `opacity` / `overflow:hidden` 在布局发射期折叠进项内,光栅化器无需树结构。
//! 平铺五表(`Layout::boxes/lines/paints/borders/shadows`)保留服务几何寻址,
//! 显示列表只服务「画」。设计见 22 号设计文档第三节。

use std::sync::Arc;

use super::super::layout::{BorderDraw, Layout, PaintBox, ShadowDraw, TextLine};
use super::super::style::{Affine, CornerRadius, GradientSpec};

/// 裁剪矩形(页坐标 x/y/w/h)。
pub type ClipRect = (f64, f64, f64, f64);

/// 背景填充:纯色或渐变。
#[derive(Debug, Clone)]
pub enum BgFill {
    /// 纯色填充(rgb + alpha)。alpha<255 时在绘制期与已绘制的底层背景合成,
    /// 不在计算样式阶段压平(见 `style::values::ComputedStyle::background_alpha`)。
    Solid([u8; 3], u8),
    Gradient(Arc<GradientSpec>),
}

/// 绘制项类别。同元素按 Shadow→Background→Border→Image→CanvasNote→Text 的
/// 通道序叠画(由 `rank()` 给出)。
#[derive(Debug, Clone)]
pub enum DisplayKind {
    Background { fill: BgFill },
    Border {
        widths: [f64; 4],
        colors: [([u8; 3], u8); 4],
    },
    Shadow {
        dx: f64,
        dy: f64,
        blur: f64,
        spread: f64,
        color: ([u8; 3], u8),
    },
    Image {
        src: String,
        fit: String,
        off_x: f64,
        off_y: f64,
    },
    Text {
        text: String,
        size: f64,
        color: [u8; 3],
        bold: bool,
    },
    /// `text-shadow` 单层(阶段六第二步):几何同文本行,颜色为阴影色,
    /// `rank` 排在 `Text` 之前绘制(文本本体盖在其上)。
    TextShadow {
        text: String,
        size: f64,
        color: [u8; 3],
        blur: f64,
    },
    /// canvas 区域占位说明块(尚无像素,或使用的 2D 路径/WebGL 能力未实现)。
    /// `pale`:元素自身无背景时先铺淡底(与旧渲染一致)。
    CanvasNote { pale: bool },
    /// 第 48 轮:媒体/嵌入空盒占位(iframe/svg/video/object/embed)。
    ///
    /// 内核不加载子文档/不解码音视频/无 SVG 光栅化;无像素时必须画
    /// 「能力边界占位」而不是白板,否则截图取证与坐标点击无法区分
    /// 「未实现」与「页面本身空白」。
    MediaFrameNote { pale: bool, tag: String },
    /// 第 48 轮:文本装饰线(删除线/下划线/虚线下划线)。
    TextDecor {
        kind: TextDecorKind,
        color: [u8; 3],
    },
    /// 原生控件几何图元:勾/圆点/箭头/列表圆点。用几何光栅而非字体字形,
    /// 避免跨平台字体缺字导致豆腐块。
    ///
    /// `color` 为图元颜色(第 44 轮):勾/圆点取 `accent-color`(或 UA 默认强调色),
    /// 禁用态控件灰化;列表 marker 取元素文字色。
    WidgetGlyph {
        glyph: WidgetGlyphKind,
        open: bool,
        color: [u8; 3],
    },
}

/// 文本装饰线类型(第 48 轮)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDecorKind {
    StrikeThrough,
    Underline,
    DottedUnderline,
}

/// 控件图元类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetGlyphKind {
    CheckMark,
    RadioDot,
    Caret,
    Bullet,
    /// 第 41 轮新增:未选中 radio 的圆环轮廓。
    BulletRing,
    /// 第 41 轮新增:`square` 列表 marker(实心方块)。
    BulletSquare,
    /// 第 41 轮新增:checkbox 的 indeterminate 横杠。
    Indeterminate,
    /// 第 41 轮新增:滑块(range)拇指,竖条。
    SliderThumb,
    /// 第 41 轮新增:日历图标(date/datetime-local/month/week)。
    CalendarIcon,
    /// 第 41 轮新增:时钟图标(time)。
    ClockIcon,
    /// 第 41 轮新增:向上三角(number 微调按钮上箭头)。
    CaretUp,
}

/// 绘制项。`x/y/w/h` 为**局部**(页坐标)border box;`opacity` 已含祖先组透明度乘积;
/// `clip` 已含祖先 `overflow:hidden` 链交集(页坐标);`transform` 为局部→页坐标仿射
/// (CSS 2D transform,已含祖先链合成),`None` = 恒等。
#[derive(Debug, Clone)]
pub struct DisplayItem {
    pub kind: DisplayKind,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub radius: [CornerRadius; 4],
    pub z: i64,
    pub seq: u64,
    pub opacity: f64,
    pub clip: Option<ClipRect>,
    pub transform: Option<Affine>,
    /// `position: fixed` 子树的标记:其盒坐标以**视口**为原点(布局层
    /// `position.rs` 对 fixed 取包含块 (0,0,vw,vh)),而普通项以页面为原点。
    /// 光栅化时滚动量 `scroll_y` 只对后者生效 —— 否则固定控件会随页面滚动平移。
    pub fixed: bool,
    /// 绘制层:0 = 常规流内容,1 = 定位元素(position != static)及其子树,
    /// 2 = **顶层**(top layer:`dialog[open]`、`select` 展开面板)。
    ///
    /// CSS 2.1 附录 E 第 8 步:定位元素(z-index:auto/0)连同其子树在常规流
    /// 内容(块背景、浮动、行内内容、图片)**之上**绘制。此前只按 (z, seq)
    /// 排序,定位元素若在 DOM 顺序上靠前,会被后面的常规流图片/背景整块盖住
    /// (实测猎聘登录框被 hero 背景图覆盖 → 登录表单完全看不见)。
    ///
    /// 第 41 轮新增第 2 档:顶层 UI 脱离常规层叠上下文,永远画在定位层之上,
    /// 且**不参与祖先 `overflow:hidden` 的裁剪交集**(见 `layout/top_layer.rs`)。
    pub layer: u8,
}

impl DisplayItem {
    /// 绘制大层:顶层最高;负 z-index 定位内容沉回常规流层。
    ///
    /// 排序不能直接用 `layer` 作第一键 —— 那会把所有定位元素(含负 z)
    /// 整档抬到常规流之上。先归一成三档 paint tier,再在档内按 z 排序。
    pub fn paint_tier(&self) -> u8 {
        if self.layer >= 2 {
            2
        } else if self.layer == 1 && self.z < 0 {
            0
        } else {
            self.layer
        }
    }

    /// 同元素通道序(越小越先画)。与旧管线通道顺序一致:
    /// 阴影→背景→边框→canvas 占位→图片→文字。
    pub fn rank(&self) -> u8 {
        match self.kind {
            DisplayKind::Shadow { .. } => 0,
            DisplayKind::Background { .. } => 1,
            DisplayKind::Border { .. } => 2,
            DisplayKind::CanvasNote { .. } | DisplayKind::MediaFrameNote { .. } => 3,
            DisplayKind::WidgetGlyph { .. } => 4,
            DisplayKind::Image { .. } => 5,
            DisplayKind::TextDecor { .. } => 8,
            DisplayKind::TextShadow { .. } => 6,
            DisplayKind::Text { .. } => 7,
        }
    }

    /// 前乘合成祖先变换:`self.transform = outer ∘ self.transform`。
    ///
    /// 用于 `apply_group` 对子树整体施加元素自身 transform(子项可能已带自身变换)。
    pub fn compose_transform(&mut self, outer: Affine) {
        self.transform = Some(match self.transform {
            Some(inner) => outer.compose(&inner),
            None => outer,
        });
    }

    /// 片段平移:几何与裁剪矩形一并移动。
    ///
    /// 带变换时须**共轭**变换(`T' = translate(d) ∘ T ∘ translate(-d)`),
    /// 使变换原点随几何一起平移 —— 否则 `transform-origin` 会相对盒偏移
    /// (片段平移用于 flex/grid 条目内容、absolute 内容、relative 偏移)。
    pub fn translate(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        if let Some(c) = &mut self.clip {
            c.0 += dx;
            c.1 += dy;
        }
        if let Some(t) = self.transform {
            let conj = Affine::translate(dx, dy)
                .compose(&t.compose(&Affine::translate(-dx, -dy)));
            self.transform = Some(conj);
        }
    }

    /// 组透明度折叠(祖先 opacity 乘进)。
    pub fn multiply_opacity(&mut self, factor: f64) {
        self.opacity *= factor;
    }

    /// 组裁剪折叠:祖先 overflow 盒与现有裁剪取交。空交集保持退化矩形,
    /// 光栅化器对空裁剪零绘制。
    pub fn intersect_clip(&mut self, rect: ClipRect) {
        self.clip = Some(match self.clip {
            None => rect,
            Some(c) => {
                let x0 = c.0.max(rect.0);
                let y0 = c.1.max(rect.1);
                let x1 = (c.0 + c.2).min(rect.0 + rect.2);
                let y1 = (c.1 + c.3).min(rect.1 + rect.3);
                (x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
            }
        });
    }
}

/// 显示列表终排:先按绘制大层(常规流 / 定位 / 顶层),档内再按 z 分层,
/// 同 z 恢复 DFS 先序(seq),同元素的同类通道序由 rank 收尾。
/// 顶层必须先于一切普通 z —— 高 `z-index` 广告层不能盖住 dialog/select 面板。
pub fn sort_items(items: &mut [DisplayItem]) {
    items.sort_by(|a, b| {
        (
            a.paint_tier(),
            a.z,
            a.layer,
            a.seq,
            a.rank(),
        )
            .cmp(&(
                b.paint_tier(),
                b.z,
                b.layer,
                b.seq,
                b.rank(),
            ))
    });
}

// ============ 旧管线回退合成器 ============

/// 从平铺五表按**旧通道顺序**合成显示列表(手构 `Layout` 与旧路径兼容)。
///
/// 旧渲染器按 阴影→背景→边框→canvas 占位→图片→文字 整通道叠画,通道顺序
/// 支配一切(通道内按表序,表已按 z 稳定排序)。本合成器用单调递增 seq 复刻
/// 该顺序(不参与 z 全局排序),保证回退输出与重构前逐像素一致。
pub fn build_legacy(layout: &Layout) -> Vec<DisplayItem> {
    let mut out = Vec::new();
    let mut next = NextSeq::default();
    for s in &layout.shadows {
        if !s.visible {
            continue;
        }
        out.push(shadow_item(s, next.get()));
    }
    for b in &layout.boxes {
        if !b.visible {
            continue;
        }
        if let Some(bg) = b.background {
            out.push(DisplayItem {
                kind: DisplayKind::Background {
                    fill: BgFill::Solid(bg, b.background_alpha),
                },
                x: b.x,
                y: b.y,
                w: b.w,
                h: b.h,
                radius: [CornerRadius::Px(0.0); 4],
                z: 0,
                seq: next.get(),
                opacity: 1.0,
                clip: None,
                transform: None,
                fixed: false,
                layer: 0,
            });
        }
    }
    for bd in &layout.borders {
        if !bd.visible {
            continue;
        }
        out.push(border_item(bd, next.get()));
    }
    for b in &layout.boxes {
        if !b.visible || !b.tag.eq_ignore_ascii_case("canvas") || b.w < 48.0 || b.h < 28.0 {
            continue;
        }
        out.push(DisplayItem {
            kind: DisplayKind::CanvasNote {
                pale: b.background.is_none(),
            },
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
            radius: [CornerRadius::Px(0.0); 4],
            z: 0,
            seq: next.get(),
            opacity: 1.0,
            clip: None,
            transform: None,
            fixed: false,
            layer: 0,
        });
    }
    for p in &layout.paints {
        if !p.visible {
            continue;
        }
        out.push(image_item(p, next.get()));
    }
    for l in &layout.lines {
        if !l.visible {
            continue;
        }
        out.push(text_item(l, next.get()));
    }
    out
}

/// 单调递增 seq(legacy 合成器通道顺序的载体)。
#[derive(Default)]
struct NextSeq(u64);

impl NextSeq {
    fn get(&mut self) -> u64 {
        self.0 += 1;
        self.0 - 1
    }
}

fn shadow_item(s: &ShadowDraw, seq: u64) -> DisplayItem {
    DisplayItem {
        kind: DisplayKind::Shadow {
            dx: s.dx,
            dy: s.dy,
            // 旧管线 blur 不实现:回退合成按 0(实心近似)。
            blur: 0.0,
            spread: s.spread,
            color: s.color,
        },
        x: s.x,
        y: s.y,
        w: s.w,
        h: s.h,
        radius: [CornerRadius::Px(0.0); 4],
        z: 0,
        seq,
        opacity: 1.0,
        clip: None,
        transform: None,
        fixed: false,
        layer: 0,
    }
}

fn border_item(b: &BorderDraw, seq: u64) -> DisplayItem {
    DisplayItem {
        kind: DisplayKind::Border {
            widths: b.widths,
            colors: b.colors,
        },
        x: b.x,
        y: b.y,
        w: b.w,
        h: b.h,
        radius: [CornerRadius::Px(0.0); 4],
        z: 0,
        seq,
        opacity: 1.0,
        clip: None,
        transform: None,
        fixed: false,
        layer: 0,
    }
}

fn image_item(p: &PaintBox, seq: u64) -> DisplayItem {
    DisplayItem {
        kind: DisplayKind::Image {
            src: p.src.clone(),
            fit: p.fit.clone(),
            off_x: p.off_x,
            off_y: p.off_y,
        },
        x: p.x,
        y: p.y,
        w: p.w,
        h: p.h,
        radius: [CornerRadius::Px(0.0); 4],
        z: 0,
        seq,
        opacity: 1.0,
        clip: None,
        transform: None,
        fixed: false,
        layer: 0,
    }
}

fn text_item(l: &TextLine, seq: u64) -> DisplayItem {
    DisplayItem {
        kind: DisplayKind::Text {
            text: l.text.clone(),
            size: l.size,
            color: l.color,
            bold: l.bold,
        },
        x: l.x,
        y: l.y,
        w: 0.0,
        h: 0.0,
        radius: [CornerRadius::Px(0.0); 4],
        z: 0,
        seq,
        opacity: 1.0,
        clip: None,
        transform: None,
        fixed: false,
        layer: 0,
    }
}
