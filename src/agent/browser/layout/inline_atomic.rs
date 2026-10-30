//! 行内原子盒(第 44 轮):替换元素与表单控件的**行盒参与模型**。
//!
//! 背景:第 41/42 轮把控件内置 UI、伪类状态、顶层浮层都做到了「有像素」,
//! 但**行内排版**这一层仍是旧模型 —— 行内替换元素与表单控件不参与行盒:
//!
//! 1. 控件盒画在**行首**(收集期的 `line_x`),前面的文字压在控件上;
//! 2. 控件把块游标提前推下去(行内台账 `content_bottom`),段首凭空多出一个
//!    「控件高度」的空洞(实测 `<p>Hello <a>link</a> World</p>` 的文字整体下沉 23px);
//! 3. 控件与同行文字**顶对齐**,没有基线(浏览器的可替换元素底边贴基线,
//!    控件底边略低于基线);
//! 4. 控件宽度不计入行宽,后面的文字直接压在控件上。
//!
//! 本模块引入「行内原子盒」:一个**叶子**的、不可再分的行内盒
//! (替换元素 `img`/`svg`/`canvas`/`iframe` 与表单控件
//! `input`/`select`/`textarea`/`button`/`progress`/`meter`),
//! 它在行内**占宽、参与断行、按基线对齐**,并在行提交时按最终位置发射
//! 盒与显示项。
//!
//! 边界:
//! - **行内块(`display:inline-block`/`inline-flex`)** 第 47 轮起走**独立格式化
//!   上下文**(`layout_inline_block_ctx_at`):子代进独立行内缓冲、按盒内容宽换行、
//!   盒高由内容决定,不再漏进父行。`inline-grid`/`inline-table` 仍不可识别(样式层
//!   归一成 `grid`/`table`),走 `emit_inline_box` 的单盒近似。
//! - **普通行内元素(`a`/`span`/`label`/…)** 第 47 轮起走**行内片段盒**:背景/
//!   边框/阴影在行提交期按行盒片段绘制(位置跟随文本、高度含上下内边距、
//!   多行每行一段),不再画在收集期的行首。
//! - `vertical-align` 只支持 `baseline`/`middle`/`top`/`bottom`/`sub`/`super`
//!   与长度/百分比;`text-top`/`text-bottom` 按 `top`/`bottom` 近似。

use kuchikiki::NodeRef;

use super::super::style::{Affine, ComputedStyle};

/// 垂直对齐方式(CSS `vertical-align` 子集)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum VAlign {
    /// 缺省:盒底边贴基线(替换元素语义)。
    Baseline,
    /// 盒垂直中心与「基线 + 半个 x 高度」对齐。
    Middle,
    /// 盒顶边与行盒顶边对齐。
    Top,
    /// 盒底边与行盒底边对齐。
    Bottom,
    /// 相对基线的显式偏移(正值向上)。
    Offset(f64),
}

/// 原子盒类别(决定基线分解策略)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicKind {
    /// 替换元素(CSS 2.1 §10.8.1):**底边贴基线**,盒体整体位于基线之上。
    Replaced,
    /// 表单控件:浏览器里控件底边略低于行内文字基线(控件自身留了内边距与
    /// 边框),内核按「基线以下 20% 盒高」近似,使控件内部文字基线与行内
    /// 文字基线基本对齐。
    Control,
}

/// 行内原子盒。
#[derive(Debug, Clone)]
pub(crate) struct InlineAtomic {
    /// 原子元素(收集期克隆;提交时按最终位置发射盒与显示项)。
    pub node: NodeRef,
    /// 元素计算样式(收集期快照)。
    pub style: ComputedStyle,
    /// 原子类别(基线分解策略)。
    pub kind: AtomicKind,
    /// 边框盒宽(不含 margin/padding —— 内核约定内边距在盒外)。
    pub w: f64,
    /// 边框盒高。
    pub h: f64,
    /// 左外边距。
    pub margin_left: f64,
    /// 右外边距。
    pub margin_right: f64,
    /// 左内边距(内核约定内边距在盒外,故计入占位宽)。
    pub padding_left: f64,
    /// 基线以上高度。
    pub ascent: f64,
    /// 基线以下高度。
    pub descent: f64,
    /// 垂直对齐方式。
    pub valign: VAlign,
    /// 收集顺序(与文本 run 共用一套序号,保证行内先后关系)。
    pub order: u64,
    /// 收集期的行基准 x(该原子盒所在行内上下文的行首位置)。
    pub base_x: f64,
    /// 祖先链合成变换(行内元素的祖先变换在提交期才合成)。
    pub tf: Option<Affine>,
}

impl InlineAtomic {
    /// 构造原子盒(按类别给出基线分解)。
    pub(crate) fn new(
        node: NodeRef,
        style: ComputedStyle,
        kind: AtomicKind,
        w: f64,
        h: f64,
        base_x: f64,
        tf: Option<Affine>,
    ) -> Self {
        let (ascent, descent) = match kind {
            AtomicKind::Replaced => (h, 0.0),
            AtomicKind::Control => (h * 0.8, h * 0.2),
        };
        let valign = parse_valign(&style);
        let margin_left = style.margin_left;
        let margin_right = style.margin_right;
        let padding_left = style.padding_left;
        Self {
            node,
            style,
            kind,
            w,
            h,
            margin_left,
            margin_right,
            padding_left,
            ascent,
            descent,
            valign,
            order: 0,
            base_x,
            tf,
        }
    }

    /// 原子盒在行内的占位宽(含外边距与内边距)。
    pub(crate) fn occupied_width(&self) -> f64 {
        self.margin_left + self.padding_left + self.w + self.margin_right
    }

    /// 该原子盒对行盒高度的贡献(基线以上 / 基线以下)。
    ///
    /// 非基线对齐(`top`/`middle`/`bottom`/显式偏移)的原子盒不抬高行盒的
    /// 基线,只按自身位置向下/向上延伸。
    pub(crate) fn line_contribution(&self) -> (f64, f64) {
        match self.valign {
            VAlign::Baseline => (self.ascent, self.descent),
            // middle 的盒中心对齐「基线 + 半个 x 高度」:行盒必须同时
            // 容纳中心线以上/以下两段,不能只计控件 descent 把父高压成 20%。
            VAlign::Middle => (
                (self.h / 2.0 - 0.25 * self.style.font_size).max(0.0),
                (self.h / 2.0 + 0.25 * self.style.font_size).max(0.0),
            ),
            VAlign::Top => (0.0, self.h),
            VAlign::Offset(d) => ((self.ascent - d).max(0.0), (self.descent + d).max(0.0)),
            _ => (0.0, self.descent),
        }
    }
}

/// 解析 `vertical-align` 计算值 → 对齐方式。
///
/// 长度/百分比相对**元素自身**的字号/行高求解(百分比按行高,与 CSS 一致)。
pub(crate) fn parse_valign(style: &ComputedStyle) -> VAlign {
    let raw = style.vertical_align.trim().to_ascii_lowercase();
    match raw.as_str() {
        "" | "baseline" => VAlign::Baseline,
        "middle" => VAlign::Middle,
        "top" | "text-top" => VAlign::Top,
        "bottom" | "text-bottom" => VAlign::Bottom,
        "sub" => VAlign::Offset(-0.2 * style.font_size),
        "super" => VAlign::Offset(0.2 * style.font_size),
        v => {
            let em = style.font_size;
            let lh = if style.line_height > 0.0 {
                style.line_height
            } else {
                em * 1.45
            };
            if let Some(p) = v.strip_suffix('%') {
                let p: f64 = p.trim().parse().unwrap_or(0.0);
                return VAlign::Offset(p / 100.0 * lh);
            }
            if let Some(p) = v.strip_suffix("rem") {
                let p: f64 = p.trim().parse().unwrap_or(0.0);
                return VAlign::Offset(p * em);
            }
            if let Some(p) = v.strip_suffix("em") {
                let p: f64 = p.trim().parse().unwrap_or(0.0);
                return VAlign::Offset(p * em);
            }
            if let Some(p) = v.strip_suffix("px") {
                let p: f64 = p.trim().parse().unwrap_or(0.0);
                return VAlign::Offset(p);
            }
            VAlign::Baseline
        }
    }
}
