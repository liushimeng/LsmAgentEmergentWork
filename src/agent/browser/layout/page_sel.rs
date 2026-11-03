//! 页面级自由选区的高亮绘制(39 号 P2b)。
//!
//! 与控件内选区(`widgets::emit_edit_overlay`)的区别:那里的索引空间是控件的
//! `value`(单值字符串),这里的索引空间是**元素可见文本**(`document::node_text`,
//!与 `human_input::page_text_caret_at` 同源)。
//!
//! **本轮近似**(方案 §7.2):跨元素整片高亮未建模 —— 只画首/尾元素内的区段
//! (首元素 `[start,len)`、尾元素 `[0,end)`),中间整元素不画。文本
//! (`getSelection().toString()`)仍按两端之间全部文本计算,可能与渲染的高亮
//! 范围不完全重合,如实写进 `page_health` 能力缺口。

use kuchikiki::NodeRef;

use super::super::document;
use super::super::style::ComputedStyle;
use super::{Affine, LayoutCtx};

/// 选区高亮色(与控件内选区一致:系统选区蓝近似)。
const SEL_COLOR: [u8; 3] = [60, 130, 235];
const SEL_ALPHA: u8 = 110;

/// 给带 `data-lsm-page-sel` 的元素画选区高亮。
///
/// 属性值由 [`Document::apply_page_sel`] 写入:
/// - `"start,end"` = 本元素内 `[start, end)`;
/// - `"start,"`(end 位为 `-1`)= 本元素内 `[start, len)`(首元素到文本末尾)。
///
/// 几何按**单行**近似(跨元素拖选不折行;多行块元素的精确行定位留后续阶段)。
pub(crate) fn emit_page_sel_overlay(
    ctx: &mut LayoutCtx<'_>,
    node: &NodeRef,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let Some(raw) = document::attr(node, document::ATTR_PAGE_SEL) else {
        return;
    };
    let parsed = parse_range(&raw);
    let Some((start, end)) = parsed else {
        return;
    };
    let text = document::node_text(node);
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let s = start.min(len);
    // end < 0 = 到文本末尾(首元素)。
    let e = if end < 0 {
        len
    } else {
        (end.max(0) as usize).min(len)
    };
    if s >= e {
        return;
    }
    let size = style.font_size.max(8.0);
    let line_height = size * 1.35;
    let y = by + ((h - line_height) / 2.0).max(1.0);
    let x0 = bx + 2.0;
    // 可见区宽度按元素盒宽(页面文本无控件内边距)。
    let spans = ctx.font().char_spans(&text, size, (w - 4.0).max(1.0));
    let sc = s.min(spans.len());
    let ec = e.min(spans.len());
    if sc >= ec {
        return;
    }
    let x = x0 + spans[sc].0;
    let ww = (spans[ec - 1].1 - spans[sc].0).max(1.0);
    ctx.push_widget_fill(style, SEL_COLOR, SEL_ALPHA, x, y, ww, line_height, tf);
}

/// 解析 `data-lsm-page-sel` 的 `"start,end"`(允许 end 位缺省/负数 = 到末尾)。
fn parse_range(raw: &str) -> Option<(usize, i64)> {
    let mut it = raw.split(',');
    let start = it.next()?.trim().parse::<usize>().ok()?;
    let end = match it.next() {
        Some(s) if !s.trim().is_empty() => s.trim().parse::<i64>().ok()?,
        // 只有 start 没有 end:按「到末尾」处理(与 -1 同义)。
        _ => -1,
    };
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析选区区间_常规与到末尾() {
        assert_eq!(parse_range("2,5"), Some((2, 5)));
        // 首元素形态:start + 负数 end = 到文本末尾。
        assert_eq!(parse_range("3,-1"), Some((3, -1)));
        // 缺 end 位同样按到末尾。
        assert_eq!(parse_range("1,"), Some((1, -1)));
        assert_eq!(parse_range("0,0"), Some((0, 0)));
    }

    #[test]
    fn 解析选区区间_非法值一律落空() {
        assert_eq!(parse_range(""), None);
        assert_eq!(parse_range("abc"), None);
        assert_eq!(parse_range("1,abc"), None);
        assert_eq!(parse_range(",3"), None);
    }
}
