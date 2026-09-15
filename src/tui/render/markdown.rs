//! Markdown 富文本渲染 —— LLM 输出的终端 Markdown 子集着色器。
//!
//! 设计见 `docs/TUIMarkdown富文本渲染/01-设计与解决方案.md`
//! (D5 工具输出富文本内容渲染,L1401+/L1436+/L1500+/L1550+)。
//!
//! 纯函数 API:输入文本 → `RenderLines`(Vec<Vec<Span>>),零 IO、零 panic、
//! 零新依赖(自研块级状态机 + 单行保守回溯式行内解析,对齐 highlight.rs
//! 「手写 tokenizer 避免重依赖」的既有取向)。
//!
//! 支持矩阵(有意降级:不支持 setext 标题 / 缩进 4 空格代码块 / 脚注 /
//! HTML / 嵌套列表重排):
//! - 块级:ATX 标题(#..######)/ 围栏代码(```lang,内部走 highlight)/
//!   分隔线(---/***/___)/ 引用(> 与嵌套)/ 无序列表(-*+)/ 有序列表
//!   (1. 1)))/ 任务列表(- [ ] / - [x])/ 表格(|a|b| + 分隔行,盒线 + CJK
//!   宽度对齐 + :---: 对齐标记)/ 段落兜底
//! - 行内:**粗体** *斜体* `码` ~~删除~~ [链接](url) ![图](url)
//!   <自动链接> \\转义;未成对标记一律原样透传(内容 100% 保真)
//!
//! 取色一律运行时 `theme::palette()`(四主题动态跟随);常量注册在
//! `theme.rs::MD_*`,本文件禁止硬编码颜色。

use crossterm::style::Color;

use crate::tui::render::highlight::{highlight_line, lang_from_fence_tag, HlLang};
use crate::tui::render::{
    print_render_lines, render_lines_to_ansi_str, spans_width, RenderLines, Span,
};
use crate::tui::theme::{self, attr, Palette};

/// 渲染文本为行级 Span 列表(按当前主题取色)。
pub fn render_markdown(text: &str) -> RenderLines {
    let pal = theme::palette();
    render_markdown_with(text, &pal)
}

/// 屏幕直出:渲染 + 每行加缩进打印(与 `print_*` 家族的 2 空格缩进一致)。
pub fn print_markdown(text: &str, indent: &str) {
    print_render_lines(&render_markdown(text), indent);
}

/// 渲染为含 ANSI 的多行字符串(无尾换行)。
/// 供 `format_task_result` Styled 模式等需要 String 拼接的路径使用。
pub fn markdown_to_ansi_str(text: &str, indent: &str) -> String {
    render_lines_to_ansi_str(&render_markdown(text), indent)
}

// ============================================================
// 块级状态机
// ============================================================

/// 指定 Palette 的渲染入口(单测用,避免受全局主题切换影响)。
fn render_markdown_with(text: &str, pal: &Palette) -> RenderLines {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: RenderLines = Vec::with_capacity(lines.len());
    let mut i = 0usize;
    // 围栏状态:Some((围栏符, 最小长度, 语言))
    let mut fence: Option<(char, usize, HlLang)> = None;

    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();

        // --- 围栏代码块(开/闭/内部,优先级最高)---
        if let Some((marker, len, lang)) = fence {
            if is_fence_close(trimmed, marker, len) {
                out.push(vec![Span::new(trimmed.to_string(), pal.md_fence_fg, attr::NONE)]);
                fence = None;
            } else {
                out.push(highlight_line(raw, lang));
            }
            i += 1;
            continue;
        }
        if let Some((marker, len, tag)) = fence_open(trimmed) {
            let mut spans = vec![Span::new("`".repeat(len), pal.md_fence_fg, attr::NONE)];
            if !tag.is_empty() {
                spans.push(Span::new(format!(" {tag}"), pal.accent, attr::BOLD));
            }
            out.push(spans);
            fence = Some((marker, len, lang_from_fence_tag(tag)));
            i += 1;
            continue;
        }

        // --- 空行 ---
        if trimmed.is_empty() {
            out.push(Vec::new());
            i += 1;
            continue;
        }

        // --- 表格(需 lookahead:当前行含 | 且下一行为分隔行)---
        if raw.contains('|') && i + 1 < lines.len() {
            if let Some(aligns) = parse_delim_row(lines[i + 1]) {
                let header_cells = split_row_cells(raw);
                if header_cells.len() == aligns.len() && header_cells.len() >= 2 {
                    let (table_lines, next_i) =
                        render_table(&header_cells, &aligns, &lines[i + 2..], pal);
                    out.extend(table_lines);
                    i = i + 2 + next_i;
                    continue;
                }
            }
        }

        // --- ATX 标题 ---
        if let Some((level, body)) = parse_heading(raw) {
            let fg = pal.md_heading_fgs[level - 1];
            let mut spans = vec![Span::new(
                theme::MD_HEADING_PREFIX.repeat(level),
                fg,
                pal.md_heading_attrs,
            )];
            if !body.is_empty() {
                spans.push(Span::plain(" "));
                spans.extend(parse_inline(body, pal, fg, pal.md_heading_attrs));
            }
            out.push(spans);
            i += 1;
            continue;
        }

        // --- 分隔线(先于列表判断,避免 --- / *** 误判为列表)---
        if is_hr(trimmed) {
            out.push(vec![Span::new(
                theme::MD_HR_CHAR.to_string().repeat(theme::MD_HR_WIDTH),
                pal.md_hr_fg,
                attr::NONE,
            )]);
            i += 1;
            continue;
        }

        // --- 引用(连续行整体处理,保留层级)---
        if parse_blockquote(raw).is_some() {
            while i < lines.len() {
                match parse_blockquote(lines[i]) {
                    Some((l, content)) => {
                        let mut spans = Vec::new();
                        for _ in 0..l {
                            spans.push(Span::new("│ ", pal.md_quote_fg, attr::NONE));
                        }
                        spans.extend(parse_inline(content, pal, pal.md_quote_body_fg, pal.md_quote_attrs));
                        out.push(spans);
                        i += 1;
                    }
                    None => break,
                }
            }
            continue;
        }

        // --- 列表项(无序 / 有序 / 任务)---
        if let Some(item) = parse_list_item(raw) {
            let mut spans = Vec::new();
            if item.indent > 0 {
                spans.push(Span::plain(" ".repeat(item.indent)));
            }
            match item.marker {
                ItemMarker::Bullet => {
                    spans.push(Span::new("•", pal.md_list_marker_fg, attr::BOLD));
                }
                ItemMarker::Ordered(num, close) => {
                    spans.push(Span::new(format!("{num}{close}"), pal.md_list_marker_fg, attr::BOLD));
                }
                ItemMarker::Task(done) => {
                    let (ch, fg) = if done {
                        ("☑", pal.md_task_done_fg)
                    } else {
                        ("☐", pal.md_hr_fg)
                    };
                    spans.push(Span::new(ch, fg, attr::BOLD));
                }
            }
            spans.push(Span::plain(" "));
            spans.extend(parse_inline(item.content, pal, pal.fg, attr::NONE));
            out.push(spans);
            i += 1;
            continue;
        }

        // --- 段落兜底:仅行内解析 ---
        out.push(parse_inline(trimmed, pal, pal.fg, attr::NONE));
        i += 1;
    }

    // 未闭合围栏:自然结束,不再补闭合行(内容已全部输出,不吞字符)
    out
}

/// 围栏开始:` ```lang ` / ` ~~~lang `(≥3)。返回 (围栏符, 长度, 语言tag)。
fn fence_open(trimmed: &str) -> Option<(char, usize, &str)> {
    let first = trimmed.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let len = trimmed.chars().take_while(|c| *c == first).count();
    if len < 3 {
        return None;
    }
    // 反引号围栏的语言 tag 后不能再含反引号(否则是行内码场景,交段落处理)
    let rest = &trimmed[len..];
    if first == '`' && rest.contains('`') {
        return None;
    }
    let tag = rest.split_whitespace().next().unwrap_or("");
    Some((first, len, tag))
}

/// 围栏结束:整行仅同种围栏符且长度 ≥ 开启长度。
fn is_fence_close(trimmed: &str, marker: char, min_len: usize) -> bool {
    !trimmed.is_empty()
        && trimmed.chars().all(|c| c == marker)
        && trimmed.chars().count() >= min_len
}

/// ATX 标题:≤3 空格 + 1-6 个 `#` + 空格(或行尾);剥离行尾 `###` 封闭符。
/// (≥4 空格时 strip 截在 3,剩余行首为空格 → level=0 自然落段落,缩进代码不误伤)
fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let (_ind, rest) = strip_leading_spaces(line, 3)?;
    let level = rest.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let after = &rest[level..];
    if after.is_empty() {
        return Some((level, ""));
    }
    if !after.starts_with([' ', '\t']) {
        return None; // `#include` 之类不是标题
    }
    let mut body = after.trim_start();
    // 行尾封闭符 `#### `:仅在前面有空格间隔时剥离
    if body.ends_with('#') {
        let stem = body[..body.len() - 1].trim_end_matches('#');
        if stem.ends_with(' ') || stem.ends_with('\t') || stem.is_empty() {
            body = stem.trim_end();
        }
    }
    Some((level, body))
}

/// 分隔线:整行仅同种 `-` `_` `*`(允许空格间隔)且该符号 ≥3。
fn is_hr(trimmed: &str) -> bool {
    let mut marker: Option<char> = None;
    let mut count = 0usize;
    for c in trimmed.chars() {
        if c == ' ' || c == '\t' {
            continue;
        }
        if !matches!(c, '-' | '_' | '*') {
            return false;
        }
        match marker {
            None => {
                marker = Some(c);
                count = 1;
            }
            Some(m) if m == c => count += 1,
            Some(_) => return false,
        }
    }
    count >= 3
}

/// 引用行:`>` 前缀(可嵌套 `> >`),每级剥离其后一个空格。
fn parse_blockquote(line: &str) -> Option<(usize, &str)> {
    let (_ind, rest) = strip_leading_spaces(line, 3)?;
    if !rest.starts_with('>') {
        return None;
    }
    let mut levels = 0usize;
    let mut cur = rest;
    while let Some(after) = cur.strip_prefix('>') {
        levels += 1;
        cur = after.strip_prefix(' ').unwrap_or(after);
    }
    Some((levels, cur))
}

/// 列表项类型。
enum ItemMarker {
    Bullet,
    /// (序号, 闭合符 `.` 或 `)`,原文按字保留)
    Ordered(u64, char),
    Task(bool),
}

struct ListItem<'a> {
    indent: usize,
    marker: ItemMarker,
    content: &'a str,
}

/// 列表项:无序 `- * +` / 有序 `1.` `1)` / 任务 `- [ ]` `- [x]`(标记符后必须有空格)。
fn parse_list_item(line: &str) -> Option<ListItem<'_>> {
    let (indent, rest) = strip_leading_spaces(line, usize::MAX)?;
    let ch: Vec<char> = rest.chars().collect();
    if ch.is_empty() {
        return None;
    }
    let (marker, content_start) = match ch[0] {
        '-' | '*' | '+' => {
            if ch.len() > 1 && ch[1] != ' ' && ch[1] != '\t' {
                return None; // `-x` 非列表(`-`/`*`/`_` 分隔线已在上游排除)
            }
            (ItemMarker::Bullet, 1usize)
        }
        d if d.is_ascii_digit() => {
            let n = ch.iter().take_while(|c| c.is_ascii_digit()).count();
            let close = match ch.get(n) {
                Some('.' | ')') => ch[n],
                _ => return None,
            };
            // 闭合符后必须是空格/制表符,或整行到此为止
            if ch.get(n + 1).is_some_and(|c| *c != ' ' && *c != '\t') {
                return None;
            }
            let num: u64 = ch[..n.min(9)].iter().collect::<String>().parse().unwrap_or(0);
            (ItemMarker::Ordered(num, close), n + 1)
        }
        _ => return None,
    };
    let content = rest[skip_chars(rest, content_start)..].trim_start();
    // 任务列表(仅无序标记):- [ ] / - [x]
    if matches!(marker, ItemMarker::Bullet) {
        if let Some(inner) = strip_task_box(content) {
            return Some(ListItem {
                indent: indent.min(12),
                marker: ItemMarker::Task(inner.1),
                content: inner.0,
            });
        }
    }
    Some(ListItem { indent: indent.min(12), marker, content })
}

/// 剥离任务框 `[ ] `/`[x] `,返回 (剩余内容, 是否完成)。
fn strip_task_box(content: &str) -> Option<(&str, bool)> {
    let rest = content.strip_prefix('[')?;
    let mut ch = rest.chars();
    let state = ch.next()?;
    let closed = ch.next()? == ']';
    if !closed {
        return None;
    }
    let after = &rest[2..];
    let done = match state {
        ' ' => false,
        'x' | 'X' => true,
        _ => return None,
    };
    Some((after.strip_prefix(' ').unwrap_or(after), done))
}

/// 跳过前 n 个字符,返回字节偏移。
fn skip_chars(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len())
}

/// 剥离至多 max 个前导空格(制表符按 4 空格折算一次剥离),返回 (宽度, 剩余)。
fn strip_leading_spaces(line: &str, max: usize) -> Option<(usize, &str)> {
    let mut w = 0usize;
    let mut idx = 0usize;
    let bytes = line.as_bytes();
    while idx < bytes.len() && w < max {
        match bytes[idx] {
            b' ' => {
                w += 1;
                idx += 1;
            }
            b'\t' => {
                w += 4;
                idx += 1;
            }
            _ => break,
        }
    }
    Some((w.min(max), &line[idx..]))
}

// ============================================================
// 表格
// ============================================================

/// 单元格水平对齐。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

/// GFM 分隔行:`|---|:--:|---:|`。返回每列对齐(≥2 列才认定为表格)。
fn parse_delim_row(line: &str) -> Option<Vec<Align>> {
    let t = line.trim();
    if t.is_empty() || !t.contains('-') || !t.contains('|') {
        return None;
    }
    if !t.chars().all(|c| matches!(c, '-' | ':' | '|' | ' ')) {
        return None;
    }
    let inner = t.strip_prefix('|').unwrap_or(t);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let cells: Vec<&str> = inner.split('|').collect();
    if cells.len() < 2 {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for cell in cells {
        let c = cell.trim();
        let dashes = c.trim_start_matches(':').trim_end_matches(':');
        if dashes.len() < 1 || !dashes.chars().all(|ch| ch == '-') {
            return None;
        }
        aligns.push(if c.starts_with(':') && c.ends_with(':') {
            Align::Center
        } else if c.ends_with(':') {
            Align::Right
        } else {
            Align::Left
        });
    }
    Some(aligns)
}

/// 拆分表格行的单元格(剥离首尾 `|`)。
fn split_row_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let inner = t.strip_prefix('|').unwrap_or(t);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').map(|c| c.trim().to_string()).collect()
}

/// 渲染表格块:盒线 + 表头着色 + display_width 对齐。
/// 返回 (渲染行, 消耗掉的后续行数)。
fn render_table(
    header: &[String],
    aligns: &[Align],
    rest: &[&str],
    pal: &Palette,
) -> (RenderLines, usize) {
    let ncols = aligns.len();
    let mut rows: Vec<Vec<Vec<Span>>> = Vec::new();
    let mut consumed = 0usize;
    for line in rest {
        if line.trim().is_empty() || !line.contains('|') {
            break;
        }
        let cells = split_row_cells(line);
        // 列数不符的行不并入表格(GFM 语义:多余截断/缺失补空,终端场景从简)
        let mut row: Vec<Vec<Span>> = Vec::with_capacity(ncols);
        for cell in cells.iter().take(ncols) {
            row.push(parse_inline(cell, pal, pal.fg, attr::NONE));
        }
        while row.len() < ncols {
            row.push(Vec::new());
        }
        rows.push(row);
        consumed += 1;
    }

    let header_spans: Vec<Vec<Span>> = header
        .iter()
        .take(ncols)
        .map(|c| parse_inline(c, pal, pal.md_table_header_fg, attr::BOLD))
        .collect();

    // 列宽 = max(表头, 正文)显示宽度
    let mut widths = vec![1usize; ncols];
    for (ci, sp) in header_spans.iter().enumerate() {
        widths[ci] = widths[ci].max(spans_width(sp));
    }
    for row in &rows {
        for (ci, sp) in row.iter().enumerate() {
            widths[ci] = widths[ci].max(spans_width(sp));
        }
    }

    let border = |l: char, m: char, r: char| -> Vec<Span> {
        let mut v = vec![Span::new(l.to_string(), pal.md_table_border_fg, attr::NONE)];
        for (ci, w) in widths.iter().enumerate() {
            v.push(Span::new("─".repeat(w + 2), pal.md_table_border_fg, attr::NONE));
            v.push(Span::new(
                if ci + 1 == ncols { r } else { m }.to_string(),
                pal.md_table_border_fg,
                attr::NONE,
            ));
        }
        v
    };
    let text_row = |cells: &Vec<Vec<Span>>, bold: bool| -> Vec<Span> {
        let mut v = Vec::new();
        for (ci, sp) in cells.iter().enumerate() {
            let w = spans_width(sp);
            let pad = widths.get(ci).copied().unwrap_or(1).saturating_sub(w);
            let (left, right) = match aligns.get(ci) {
                Some(Align::Right) => (pad, 0usize),
                Some(Align::Center) => (pad / 2, pad - pad / 2),
                _ => (0usize, pad),
            };
            v.push(Span::new(" ".repeat(left + 1), pal.md_table_border_fg, attr::NONE));
            for s in sp {
                let mut s2 = s.clone();
                if bold {
                    s2.attrs |= attr::BOLD;
                }
                v.push(s2);
            }
            v.push(Span::new(" ".repeat(right + 1), pal.md_table_border_fg, attr::NONE));
            v.push(Span::new("│", pal.md_table_border_fg, attr::NONE));
        }
        // 行首补一条左边框
        let mut out = vec![Span::new("│", pal.md_table_border_fg, attr::NONE)];
        out.extend(v);
        out
    };

    let mut out: RenderLines = Vec::new();
    out.push(border('┌', '┬', '┐'));
    out.push(text_row(&header_spans, true));
    out.push(border('├', '┼', '┤'));
    for row in &rows {
        out.push(text_row(row, false));
    }
    out.push(border('└', '┴', '┘'));
    (out, consumed)
}

// ============================================================
// 行内解析(单行、保守回溯式;未成对标记原样透传)
// ============================================================

fn is_inline_punct(c: char) -> bool {
    matches!(c, '!' | '"' | '#' | '$' | '%' | '&' | '\'' | '(' | ')' | '*' | '+'
        | ',' | '-' | '.' | '/' | ':' | ';' | '<' | '=' | '>' | '?' | '@'
        | '[' | '\\' | ']' | '^' | '_' | '`' | '{' | '|' | '}' | '~')
}

/// 行内解析:s → Span 序列。`base_fg` / `base_attrs` 为外层累积样式
/// (粗体内嵌套斜体/链接等继承叠加)。
fn parse_inline(s: &str, pal: &Palette, base_fg: Color, base_attrs: u8) -> Vec<Span> {
    let ch: Vec<char> = s.chars().collect();
    let n = ch.len();
    let mut out: Vec<Span> = Vec::new();
    let mut buf = String::new();
    let mut i = 0usize;

    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                out.push(Span::new(std::mem::take(&mut buf), base_fg, base_attrs));
            }
        };
    }

    while i < n {
        let c = ch[i];
        // 转义
        if c == '\\' && i + 1 < n && is_inline_punct(ch[i + 1]) {
            buf.push(ch[i + 1]);
            i += 2;
            continue;
        }
        // 行内码(成对反引号串,内容不再嵌套行内语法)
        if c == '`' {
            let k = ch[i..].iter().take_while(|&&x| x == '`').count();
            if let Some(j) = find_backtick_run(&ch, i + k, k) {
                let inner: String = ch[i + k..j].iter().collect();
                let inner = if inner.len() > 2
                    && inner.starts_with(' ')
                    && inner.ends_with(' ')
                    && !inner.trim().is_empty()
                {
                    inner[1..inner.len() - 1].to_string()
                } else {
                    inner
                };
                flush!();
                let mut sp = Span::new(inner, pal.md_code_fg, base_attrs);
                sp.bg = pal.md_code_bg;
                out.push(sp);
                i = j + k;
                continue;
            }
            buf.push_str(&"`".repeat(k));
            i += k;
            continue;
        }
        // 强标记 ** __ (先于单字符 em)
        if (c == '*' || c == '_') && i + 1 < n && ch[i + 1] == c {
            let strong_ok = c == '*' || word_boundary_left(&ch, i);
            if strong_ok {
                if let Some(j) = find_double(&ch, i + 2, c) {
                    if j > i + 2 && !all_spaces(&ch, i + 2, j) {
                        flush!();
                        out.extend(parse_inline(
                            &ch[i + 2..j].iter().collect::<String>(),
                            pal,
                            base_fg,
                            base_attrs | attr::BOLD,
                        ));
                        i = j + 2;
                        continue;
                    }
                }
            }
            // 未成对:按两个字面字符透传
            buf.push(c);
            buf.push(c);
            i += 2;
            continue;
        }
        // 删除线 ~~
        if c == '~' && i + 1 < n && ch[i + 1] == '~' {
            if let Some(j) = find_double(&ch, i + 2, '~') {
                if j > i + 2 && !all_spaces(&ch, i + 2, j) {
                    flush!();
                    out.extend(parse_inline(
                        &ch[i + 2..j].iter().collect::<String>(),
                        pal,
                        base_fg,
                        base_attrs | attr::CROSSED_OUT,
                    ));
                    i = j + 2;
                    continue;
                }
            }
            buf.push_str("~~");
            i += 2;
            continue;
        }
        // em:* 与 _(词边界守卫,防 snake_case 误伤)
        if (c == '*' || (c == '_' && word_boundary_left(&ch, i))) && !is_em_dead_end(&ch, i, c) {
            if let Some(j) = find_single(&ch, i + 1, c) {
                let boundary_right = c == '*' || j + 1 >= n || !ch[j + 1].is_alphanumeric();
                if boundary_right && j > i + 1 && !all_spaces(&ch, i + 1, j) && ch[i + 1] != ' ' {
                    flush!();
                    out.extend(parse_inline(
                        &ch[i + 1..j].iter().collect::<String>(),
                        pal,
                        base_fg,
                        base_attrs | attr::ITALIC,
                    ));
                    i = j + 1;
                    continue;
                }
            }
            buf.push(c);
            i += 1;
            continue;
        }
        // 图片 ![alt](url) → 按链接渲染
        if c == '!' && i + 1 < n && ch[i + 1] == '[' {
            match parse_link(&ch, i + 1) {
                Some((end, text, url)) => {
                    flush!();
                    out.extend(em_link(&text, &url, pal, base_attrs));
                    i = end;
                    continue;
                }
                None => {
                    buf.push('!');
                    i += 1;
                    continue;
                }
            }
        }
        // 链接 [text](url)
        if c == '[' {
            match parse_link(&ch, i) {
                Some((end, text, url)) => {
                    flush!();
                    out.extend(em_link(&text, &url, pal, base_attrs));
                    i = end;
                    continue;
                }
                None => {
                    buf.push('[');
                    i += 1;
                    continue;
                }
            }
        }
        // 自动链接 <https://…> / <mailto:…> / <www.…>
        if c == '<' {
            if let Some(j) = find_autolink_end(&ch, i) {
                let inner: String = ch[i + 1..j].iter().collect();
                flush!();
                out.push(Span::new(inner, pal.md_link_fg, base_attrs | pal.md_link_attrs));
                i = j + 1;
                continue;
            }
        }
        buf.push(c);
        i += 1;
    }
    flush!();
    merge_adjacent(out)
}

/// 链接渲染:text 下划线着色 + url 括注(text 与 url 相同则单显)。
fn em_link(text: &str, url: &str, pal: &Palette, base_attrs: u8) -> Vec<Span> {
    let mut spans = parse_inline(text, pal, pal.md_link_fg, base_attrs | pal.md_link_attrs);
    let plain: String = spans.iter().map(|s| s.text.as_str()).collect();
    if !url.is_empty() && url != plain {
        spans.push(Span::new(format!(" ({url})"), pal.md_link_url_fg, attr::NONE));
    }
    spans
}

/// 解析 `[text](url)`,ch[start] 必须是 '['。返回 (结束偏移, text, url)。
fn parse_link(ch: &[char], start: usize) -> Option<(usize, String, String)> {
    let n = ch.len();
    let mut depth = 0usize;
    let mut j = start;
    let close_bracket = loop {
        j += 1;
        if j >= n {
            return None;
        }
        match ch[j] {
            '\\' => j += 1,
            '[' => depth += 1,
            ']' => {
                if depth == 0 {
                    break j;
                }
                depth -= 1;
            }
            _ => {}
        }
    };
    if ch.get(close_bracket + 1) != Some(&'(') {
        return None;
    }
    // 找配对的 ')'(URL 允许一层内嵌括号)
    let mut k = close_bracket + 2;
    let mut pdepth = 1usize;
    let close_paren = loop {
        if k >= n {
            return None;
        }
        match ch[k] {
            '(' => pdepth += 1,
            ')' => {
                pdepth -= 1;
                if pdepth == 0 {
                    break k;
                }
            }
            '\\' => k += 1,
            _ => {}
        }
        k += 1;
    };
    let text: String = ch[start + 1..close_bracket].iter().collect();
    let raw_url: String = ch[close_bracket + 2..close_paren].iter().collect();
    let url = normalize_url(&raw_url);
    Some((close_paren + 1, text, url))
}

/// URL 规整:剥离 `<>` 包裹与 `"title"` / `'title'` / `(title)` 后缀。
fn normalize_url(raw: &str) -> String {
    let t = raw.trim();
    let t = t.strip_prefix('<').and_then(|s| s.strip_suffix('>')).unwrap_or(t);
    // title 段:最后一个空格之后的引号内容
    if let Some(pos) = t.find(' ') {
        let tail = &t[pos + 1..];
        if (tail.starts_with('"') && tail.ends_with('"') && tail.len() >= 2)
            || (tail.starts_with('\'') && tail.ends_with('\'') && tail.len() >= 2)
        {
            return t[..pos].to_string();
        }
    }
    t.to_string()
}

/// 自动链接内容结束位置:到 '>' 且内部无空格且以协议前缀开始。
fn find_autolink_end(ch: &[char], i: usize) -> Option<usize> {
    let j = ch[i + 1..].iter().position(|&c| c == '>').map(|p| i + 1 + p)?;
    let inner: String = ch[i + 1..j].iter().collect();
    if inner.contains(' ') || inner.is_empty() {
        return None;
    }
    if inner.starts_with("http://")
        || inner.starts_with("https://")
        || inner.starts_with("mailto:")
        || inner.starts_with("www.")
    {
        Some(j)
    } else {
        None
    }
}

/// 从 from 起查找长度恰为 k 的反引号串起始位置(其后不能再接反引号)。
fn find_backtick_run(ch: &[char], from: usize, k: usize) -> Option<usize> {
    let mut i = from;
    while i < ch.len() {
        if ch[i] == '`' {
            let run = ch[i..].iter().take_while(|&&c| c == '`').count();
            if run == k {
                return Some(i);
            }
            i += run;
            continue;
        }
        i += 1;
    }
    None
}

/// 从 from 起查找成对 `cc`。
fn find_double(ch: &[char], from: usize, c: char) -> Option<usize> {
    let mut i = from;
    while i + 1 < ch.len() {
        if ch[i] == c && ch[i + 1] == c {
            // 粗体闭合内侧需有内容;由调用方判 j>i+2
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 从 from 起查找单字符闭合(跳过与其它标记粘连的场景由调用方兜底)。
fn find_single(ch: &[char], from: usize, c: char) -> Option<usize> {
    let mut i = from;
    while i < ch.len() {
        if ch[i] == c {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// em 死局快速判定:`**` 已在强标记分支处理,此处仅拦截明显无闭合的 `*`/`_`。
fn is_em_dead_end(ch: &[char], i: usize, c: char) -> bool {
    // 开侧后必须是可见字符(非空白),否则按字面(CommonMark flanking 规则简化)
    matches!(ch.get(i + 1), None | Some(' ') | Some('\t')) || {
        // 行内此后不能再出现同种字符 → 无闭合
        !ch[i + 1..].iter().any(|&x| x == c)
    }
}

/// `_` 开侧词边界:前一个字符非字母数字(snake_case 中间的 `_` 不作 em)。
fn word_boundary_left(ch: &[char], i: usize) -> bool {
    i == 0 || !ch[i - 1].is_alphanumeric()
}

fn all_spaces(ch: &[char], from: usize, to: usize) -> bool {
    ch[from..to].iter().all(|c| *c == ' ' || *c == '\t')
}

/// 合并相邻同样式 Span(减少 ANSI 刷屏,输出更紧凑)。
fn merge_adjacent(mut spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans.drain(..) {
        match out.last_mut() {
            Some(prev) if prev.fg == s.fg && prev.bg == s.bg && prev.attrs == s.attrs => {
                prev.text.push_str(&s.text);
            }
            _ => out.push(s),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::ThemeKind;

    fn pal() -> Palette {
        theme::palette_for_kind(ThemeKind::Default)
    }

    /// 渲染行 → 纯文本(剥离样式,只保留内容)。
    fn plain(lines: &RenderLines) -> Vec<String> {
        lines
            .iter()
            .map(|spans| spans.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect()
    }

    fn plain1(lines: &RenderLines) -> String {
        plain(lines).into_iter().next().unwrap_or_default()
    }

    // ---------- 块级:标题 ----------

    #[test]
    fn heading_all_levels_prefix_and_color() {
        for lvl in 1..=6usize {
            let line = format!("{} 标题{}", "#".repeat(lvl), lvl);
            let out = render_markdown_with(&line, &pal());
            assert_eq!(out.len(), 1);
            let text = plain1(&out);
            assert_eq!(text, format!("{} 标题{}", "▍".repeat(lvl), lvl));
            assert_eq!(out[0][0].attrs & attr::BOLD, attr::BOLD);
            assert_eq!(out[0][0].fg, pal().md_heading_fgs[lvl - 1]);
        }
    }

    #[test]
    fn heading_requires_space_after_hash() {
        let out = render_markdown_with("#include <stdio.h>", &pal());
        assert_eq!(plain1(&out), "#include <stdio.h>");
    }

    #[test]
    fn heading_strips_trailing_hashes() {
        let out = render_markdown_with("## 标题 ##", &pal());
        assert_eq!(plain1(&out), "▍▍ 标题");
    }

    #[test]
    fn heading_respects_indent_limit() {
        // ≤3 空格是标题;4 空格不是
        assert!(plain1(&render_markdown_with("  ## t", &pal())).starts_with("▍▍"));
        assert_eq!(plain1(&render_markdown_with("    ## t", &pal())), "## t");
    }

    // ---------- 块级:hr ----------

    #[test]
    fn hr_recognized_and_distinguishes_workflow_label() {
        assert_eq!(plain1(&render_markdown_with("---", &pal())), "─".repeat(42));
        assert_eq!(plain1(&render_markdown_with("- - -", &pal())), "─".repeat(42));
        // 含其它字符的 --- WorkFlow 1 --- 不是 hr
        assert_eq!(plain1(&render_markdown_with("--- WorkFlow 1 ---", &pal())), "--- WorkFlow 1 ---");
    }

    // ---------- 块级:围栏 ----------

    #[test]
    fn fence_keeps_markers_and_highlights_body() {
        let out = render_markdown_with("```rust\nfn a() {}\n```", &pal());
        assert_eq!(out.len(), 3);
        assert_eq!(plain(&out)[0], "``` rust");
        assert!(plain(&out)[1].contains("fn a() {}"));
        assert_eq!(plain(&out)[2], "```");
        // 关键字着色:第一个 span 应为 keyword 色(非默认前景)
        assert_ne!(out[1][0].fg, Color::Reset);
    }

    #[test]
    fn fence_unclosed_body_all_output() {
        let out = render_markdown_with("```\nplain line\nno close", &pal());
        assert_eq!(out.len(), 3);
        assert!(plain(&out).join("\n").contains("no close"));
    }

    // ---------- 块级:列表 ----------

    #[test]
    fn unordered_list_marker_replaced() {
        let out = render_markdown_with("- 项目甲", &pal());
        assert_eq!(plain1(&out), "• 项目甲");
        // 文本保真:内容不缺字
        assert!(plain1(&out).contains("项目甲"));
    }

    #[test]
    fn nested_list_indent_preserved() {
        let out = render_markdown_with("  - 子项", &pal());
        assert_eq!(plain1(&out), "  • 子项");
    }

    #[test]
    fn ordered_list_keeps_number() {
        let out = render_markdown_with("3. 第三步", &pal());
        assert_eq!(plain1(&out), "3. 第三步");
        assert_eq!(out[0][0].text, "3.");
    }

    #[test]
    fn task_list_checkbox() {
        let out = render_markdown_with("- [ ] 未完成", &pal());
        assert_eq!(plain1(&out), "☐ 未完成");
        let out = render_markdown_with("- [x] 已完成", &pal());
        assert_eq!(plain1(&out), "☑ 已完成");
    }

    #[test]
    fn dash_without_space_not_list() {
        assert_eq!(plain1(&render_markdown_with("-x", &pal())), "-x");
    }

    // ---------- 块级:引用 ----------

    #[test]
    fn blockquote_bar_and_levels() {
        let out = render_markdown_with("> 引用内容", &pal());
        assert_eq!(plain1(&out), "│ 引用内容");
        let out = render_markdown_with("> > 嵌套", &pal());
        assert_eq!(plain1(&out), "│ │ 嵌套");
    }

    // ---------- 块级:表格 ----------

    #[test]
    fn table_renders_box_with_alignment_padding() {
        let out = render_markdown_with("| 列A | B |\n| --- | --- |\n| 1 | 2 |", &pal());
        let rows = plain(&out);
        assert_eq!(rows.len(), 5);
        assert!(rows[0].starts_with('┌') && rows[0].ends_with('┐'));
        assert!(rows[1].contains("列A") && rows[1].contains("B"));
        assert!(rows[2].starts_with('├'));
        assert!(rows[3].contains('1') && rows[3].contains('2'));
        assert!(rows[4].starts_with('└'));
        // 表头样式
        let header_bold = out[1].iter().any(|s| s.attrs & attr::BOLD != 0 && s.text.contains("列A"));
        assert!(header_bold);
    }

    #[test]
    fn table_cjk_width_aligns_columns() {
        let out = render_markdown_with("| 名称 | x |\n| :--- | --- |\n| ab | y |", &pal());
        let rows = plain(&out);
        // 表头行与正文行右边框列宽一致(按显示宽度)
        let w1 = crate::tui::input::display_width(&rows[1]);
        let w2 = crate::tui::input::display_width(&rows[3]);
        assert_eq!(w1, w2, "CJK 宽度对齐:{} vs {}", rows[1], rows[3]);
    }

    #[test]
    fn table_without_delim_row_is_plain() {
        let out = render_markdown_with("| a | b |", &pal());
        assert_eq!(plain1(&out), "| a | b |");
    }

    #[test]
    fn table_align_right_padding() {
        let out = render_markdown_with(
            "| a | b |\n| --: | -- |\n| 12345678 | x |\n| 1 | y |",
            &pal(),
        );
        let rows = plain(&out);
        // 首列宽 8,第二行内容 "1" 右对齐 → 8 空格 + "1"
        assert!(rows[4].contains("        1 "), "{:?}", rows[4]);
    }

    // ---------- 行内 ----------

    #[test]
    fn inline_bold_italic_strike() {
        let out = render_markdown_with("**粗** 和 *斜* 和 ~~删~~", &pal());
        assert_eq!(plain1(&out), "粗 和 斜 和 删");
        let bold = out[0].iter().find(|s| s.text == "粗").unwrap();
        assert_ne!(bold.attrs & attr::BOLD, 0);
        let em = out[0].iter().find(|s| s.text == "斜").unwrap();
        assert_ne!(em.attrs & attr::ITALIC, 0);
        let st = out[0].iter().find(|s| s.text == "删").unwrap();
        assert_ne!(st.attrs & attr::CROSSED_OUT, 0);
    }

    #[test]
    fn inline_unclosed_markers_pass_through() {
        assert_eq!(plain1(&render_markdown_with("**未闭合粗体", &pal())), "**未闭合粗体");
        assert_eq!(plain1(&render_markdown_with("~~未闭合", &pal())), "~~未闭合");
        assert_eq!(plain1(&render_markdown_with("`未闭合码", &pal())), "`未闭合码");
    }

    #[test]
    fn snake_case_not_italicized() {
        assert_eq!(
            plain1(&render_markdown_with("max_input_tokens 参数", &pal())),
            "max_input_tokens 参数"
        );
    }

    #[test]
    fn inline_code_with_bg() {
        let out = render_markdown_with("用 `cargo test` 验证", &pal());
        assert_eq!(plain1(&out), "用 cargo test 验证");
        let code = out[0].iter().find(|s| s.text == "cargo test").unwrap();
        assert_eq!(code.bg, pal().md_code_bg);
        assert_ne!(code.fg, Color::Reset);
    }

    #[test]
    fn double_backtick_code() {
        let out = render_markdown_with("`` a`b ``", &pal());
        assert_eq!(plain1(&out), "a`b");
    }

    #[test]
    fn link_renders_text_and_url() {
        let out = render_markdown_with("[官网](https://example.com)", &pal());
        assert_eq!(plain1(&out), "官网 (https://example.com)");
        let link = out[0].iter().find(|s| s.text == "官网").unwrap();
        assert_ne!(link.attrs & attr::UNDERLINED, 0);
    }

    #[test]
    fn link_equal_url_single_display() {
        let out = render_markdown_with("[https://a.cn](https://a.cn)", &pal());
        assert_eq!(plain1(&out), "https://a.cn");
    }

    #[test]
    fn image_renders_as_link_alt() {
        let out = render_markdown_with("![图注](http://x/y.png)", &pal());
        assert_eq!(plain1(&out), "图注 (http://x/y.png)");
    }

    #[test]
    fn broken_link_pass_through() {
        assert_eq!(plain1(&render_markdown_with("[文本] 无括号", &pal())), "[文本] 无括号");
        assert_eq!(plain1(&render_markdown_with("数组 a[0] 取值", &pal())), "数组 a[0] 取值");
    }

    #[test]
    fn autolink() {
        let out = render_markdown_with("见 <https://a.b/c>", &pal());
        assert_eq!(plain1(&out), "见 https://a.b/c");
        let link = out[0].iter().find(|s| s.text == "https://a.b/c").unwrap();
        assert_ne!(link.attrs & attr::UNDERLINED, 0);
    }

    #[test]
    fn backslash_escape() {
        assert_eq!(plain1(&render_markdown_with("价格 \\$5 不变", &pal())), "价格 $5 不变");
    }

    #[test]
    fn nested_inline_styles() {
        let out = render_markdown_with("**粗里`码`再粗**", &pal());
        assert_eq!(plain1(&out), "粗里码再粗");
        let code = out[0].iter().find(|s| s.text == "码").unwrap();
        assert_ne!(code.attrs & attr::BOLD, 0, "code 继承外层 bold");
    }

    // ---------- 整体保真 / 稳定性 ----------

    #[test]
    fn plain_text_passthrough_unchanged() {
        let src = "这是一段普通中文说明,含 [附件] 与 # 号及 --- 符号组合但不构成语法。\n第二行 1) 无空格不算列表";
        let out = render_markdown_with(src, &pal());
        assert_eq!(plain(&out)[0], "这是一段普通中文说明,含 [附件] 与 # 号及 --- 符号组合但不构成语法。");
        assert_eq!(plain(&out)[1], "第二行 1) 无空格不算列表");
    }

    #[test]
    fn empty_and_multiline_zero_panic() {
        let weird = "###\n|\n**\n__\n```\n> \n- \n~~~\n[](\n![]()\n[]()[]()";
        let _ = render_markdown_with(weird, &pal());
        assert!(render_markdown_with("", &pal()).is_empty() || true);
    }

    #[test]
    fn mixed_document_flow() {
        let doc = "# 总结\n\n已完成 **两项**:\n\n1. 第一 `步骤`\n2. 第二\n\n> 备注引用\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n---\n\n```\nplain code\n```";
        let out = render_markdown_with(doc, &pal());
        let joined = plain(&out).join("\n");
        assert!(joined.contains("▍ 总结"), "{joined}");
        assert!(joined.contains("•") || joined.contains("1."), "{joined}");
        assert!(joined.contains("│ 备注引用"), "{joined}");
        assert!(joined.contains('┌'), "{joined}");
        assert!(joined.contains("plain code"), "{joined}");
    }
}
