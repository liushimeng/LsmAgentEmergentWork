//! 绘制显示列表(阶段六第一步):布局期 DFS 产出的**自包含**绘制项序列。
//!
//! 每项自带 `(z, seq, rank)` 三元排序键、透明度与裁剪矩形——祖先组的
//! `opacity` / `overflow:hidden` 在布局发射期折叠进项内,光栅化器无需树结构。
//! 平铺五表(`Layout::boxes/lines/paints/borders/shadows`)保留服务几何寻址,
//! 显示列表只服务「画」。设计见 22 号设计文档第三节。

use std::sync::Arc;

use super::super::layout::{BorderDraw, Layout, PaintBox, ShadowDraw, TextLine};
use super::super::style::{CornerRadius, GradientSpec};

/// 裁剪矩形(页坐标 x/y/w/h)。
pub type ClipRect = (f64, f64, f64, f64);

/// 背景填充:纯色或渐变。
#[derive(Debug, Clone)]
pub enum BgFill {
    Solid([u8; 3]),
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
    /// canvas 区域占位说明块(内核未实现 2D/WebGL 像素渲染)。
    /// `pale`:元素自身无背景时先铺淡底(与旧渲染一致)。
    CanvasNote { pale: bool },
}

/// 绘制项。`x/y/w/h` 为页坐标 border box;`opacity` 已含祖先组透明度乘积;
/// `clip` 已含祖先 `overflow:hidden` 链交集(页坐标)。
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
}

impl DisplayItem {
    /// 同元素通道序(越小越先画)。与旧管线通道顺序一致:
    /// 阴影→背景→边框→canvas 占位→图片→文字。
    pub fn rank(&self) -> u8 {
        match self.kind {
            DisplayKind::Shadow { .. } => 0,
            DisplayKind::Background { .. } => 1,
            DisplayKind::Border { .. } => 2,
            DisplayKind::CanvasNote { .. } => 3,
            DisplayKind::Image { .. } => 4,
            DisplayKind::Text { .. } => 5,
        }
    }

    /// 片段平移:几何与裁剪矩形一并移动。
    pub fn translate(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        if let Some(c) = &mut self.clip {
            c.0 += dx;
            c.1 += dy;
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

/// 显示列表终排:先 z 分层(沿用 z-index 语义),同 z 恢复 DFS 先序(seq),
/// 同元素的同类通道序由 rank 收尾。稳定排序保证同键保持发射序。
pub fn sort_items(items: &mut [DisplayItem]) {
    items.sort_by(|a, b| (a.z, a.seq, a.rank()).cmp(&(b.z, b.seq, b.rank())));
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
                    fill: BgFill::Solid(bg),
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
    }
}
