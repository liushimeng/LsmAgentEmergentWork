//! 文本光标 / 选区 / 编辑语义纯逻辑(39 号设计文档 P2a)。
//!
//! 人工侧文本输入此前是「尾部追加/删除」近似;本模块补齐浏览器级文本编辑
//! 的**纯函数核心**(零 I/O、零 DOM,单测主场):
//!
//! * 光标映射:点击坐标 → char 索引(单行 / textarea 行列,与控件文本绘制
//!   共用 [`FontSet::char_spans`] / [`FontSet::wrap_text_rows`] 几何);
//! * 编辑运算:插入(替换选区)/ Backspace / Delete / 删词——返回新值与新光标;
//! * 光标移动:左右(含 Ctrl 词跳、Shift 扩选、选区折叠)、上下(按行)、
//!   行首行尾 / 文首文尾;
//! * 词界:双击选词 / Ctrl 词跳共用(字母数字下划线连跑;CJK 按字类连跑近似);
//! * [`EditHistory`]:单控件撤销/重做双栈(⌘/Ctrl+Z / Y)。
//!
//! 所有索引均为 **char 索引**,调用方以 `Vec<char>` 语义使用,中文/emoji 不裂。

use super::super::document::CaretState;
use super::super::render::FontSet;

/// 单控件撤销/重做历史(人工交互语义,Agent 侧编辑不入栈)。
#[derive(Debug, Default)]
pub struct EditHistory {
    undo: Vec<(String, CaretState)>,
    redo: Vec<(String, CaretState)>,
}

/// 历史栈上限(单控件;超出丢最旧)。
const HISTORY_CAP: usize = 100;

impl EditHistory {
    /// 编辑前快照入栈并清空重做栈(浏览器语义:新编辑断开 redo 链)。
    pub fn push(&mut self, value: String, caret: CaretState) {
        self.undo.push((value, caret));
        if self.undo.len() > HISTORY_CAP {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// 撤销:当前态入重做栈,返回恢复的 `(值, 光标)`。
    pub fn undo(&mut self, current: String, caret: CaretState) -> Option<(String, CaretState)> {
        let prev = self.undo.pop()?;
        self.redo.push((current, caret));
        Some(prev)
    }

    /// 重做:当前态入撤销栈,返回恢复的 `(值, 光标)`。
    pub fn redo(&mut self, current: String, caret: CaretState) -> Option<(String, CaretState)> {
        let next = self.redo.pop()?;
        self.undo.push((current, caret));
        Some(next)
    }

    /// 仅测试用:栈深度。
    #[cfg(test)]
    pub(crate) fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

/// 值的 char 向量视图。
fn chars_of(value: &str) -> Vec<char> {
    value.chars().collect()
}

/// char 向量拼回字符串。
fn join(chars: &[char]) -> String {
    chars.iter().collect()
}

/// 光标/选区按值长夹紧(外部值被 Agent 侧改写后防越界)。
fn clamp_caret(caret: CaretState, len: usize) -> CaretState {
    CaretState {
        pos: caret.pos.min(len),
        anchor: caret.anchor.min(len),
    }
}

/// 是否词字符(词界近似:字母数字下划线 + 全部字母类,含 CJK)。
fn is_word_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// 双击选词边界(pos 为点击光标位置):词字符按「ASCII 词」与「非 ASCII 词」
/// 两类连跑扩展;非词字符单选当前字符(pos 落尾部时取前一字符)。
pub fn word_bounds(chars: &[char], pos: usize) -> (usize, usize) {
    if chars.is_empty() {
        return (0, 0);
    }
    let mut i = pos.min(chars.len());
    if i >= chars.len() {
        i = chars.len() - 1;
    }
    if !is_word_char(chars[i]) {
        // 双击非词字符(空白/标点):浏览器选相邻词(向右优先,其次向左)。
        if i + 1 < chars.len() && is_word_char(chars[i + 1]) {
            i += 1;
        } else if i > 0 && is_word_char(chars[i - 1]) {
            i -= 1;
        } else {
            return (i, i + 1);
        }
    }
    let class = |c: char| if c.is_ascii() { 1u8 } else { 2u8 };
    let k = class(chars[i]);
    let mut start = i;
    while start > 0 && is_word_char(chars[start - 1]) && class(chars[start - 1]) == k {
        start -= 1;
    }
    let mut end = i + 1;
    while end < chars.len() && is_word_char(chars[end]) && class(chars[end]) == k {
        end += 1;
    }
    (start, end)
}

/// Ctrl+←/→ 词跳目标:向左 = 跳过非词字符再跳过词字符到词首;
/// 向右 = 跳过词字符再跳过非词字符到下一词首。
pub fn word_jump(chars: &[char], pos: usize, dir: i32) -> usize {
    let mut i = pos.min(chars.len());
    if dir < 0 {
        while i > 0 && !is_word_char(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word_char(chars[i - 1]) {
            i -= 1;
        }
    } else {
        while i < chars.len() && is_word_char(chars[i]) {
            i += 1;
        }
        while i < chars.len() && !is_word_char(chars[i]) {
            i += 1;
        }
    }
    i
}

// ===================== 编辑运算 =====================

/// 插入文本(有选区先删选区):返回 `(新值, 新光标)`(光标落插入串尾)。
pub fn edit_insert(value: &str, caret: CaretState, piece: &str) -> (String, CaretState) {
    let mut chars = chars_of(value);
    let caret = clamp_caret(caret, chars.len());
    let (a, b) = caret.range();
    let _ = chars.splice(a..b, piece.chars());
    let pos = a + piece.chars().count();
    (join(&chars), CaretState::collapsed(pos))
}

/// Backspace:有选区删选区;否则删光标前一字符(0 处 no-op)。
pub fn edit_backspace(value: &str, caret: CaretState) -> (String, CaretState) {
    let mut chars = chars_of(value);
    let caret = clamp_caret(caret, chars.len());
    let (a, b) = caret.range();
    if a != b {
        let _ = chars.splice(a..b, std::iter::empty());
        return (join(&chars), CaretState::collapsed(a));
    }
    if caret.pos == 0 {
        return (value.to_string(), caret);
    }
    chars.remove(caret.pos - 1);
    (join(&chars), CaretState::collapsed(caret.pos - 1))
}

/// Delete:有选区删选区;否则删光标处字符(尾部 no-op)。
pub fn edit_delete(value: &str, caret: CaretState) -> (String, CaretState) {
    let mut chars = chars_of(value);
    let caret = clamp_caret(caret, chars.len());
    let (a, b) = caret.range();
    if a != b {
        let _ = chars.splice(a..b, std::iter::empty());
        return (join(&chars), CaretState::collapsed(a));
    }
    if caret.pos >= chars.len() {
        return (value.to_string(), caret);
    }
    chars.remove(caret.pos);
    (join(&chars), CaretState::collapsed(caret.pos))
}

/// Ctrl+Backspace 删词:有选区删选区;否则删光标前到词首。
pub fn edit_delete_word_back(value: &str, caret: CaretState) -> (String, CaretState) {
    let chars = chars_of(value);
    let caret = clamp_caret(caret, chars.len());
    if !caret.is_empty() {
        return edit_backspace(value, caret);
    }
    let target = word_jump(&chars, caret.pos, -1);
    if target == caret.pos {
        return (value.to_string(), caret);
    }
    let mut chars = chars;
    let _ = chars.splice(target..caret.pos, std::iter::empty());
    (join(&chars), CaretState::collapsed(target))
}

// ===================== 光标移动 =====================

/// 左右移动(`word` = Ctrl 词跳;`extend` = Shift 扩选)。
///
/// 有选区且非扩选:折叠到近端(← 折到选区头,→ 折到选区尾,浏览器语义)。
pub fn caret_move(value: &str, caret: CaretState, dir: i32, word: bool, extend: bool) -> CaretState {
    let chars = chars_of(value);
    let caret = clamp_caret(caret, chars.len());
    if !extend && !caret.is_empty() {
        let (a, b) = caret.range();
        return CaretState::collapsed(if dir < 0 { a } else { b });
    }
    let target = if word {
        word_jump(&chars, caret.pos, dir)
    } else {
        (caret.pos as i64 + dir as i64).clamp(0, chars.len() as i64) as usize
    };
    if extend {
        CaretState { pos: target, anchor: caret.anchor }
    } else {
        CaretState::collapsed(target)
    }
}

/// 上下移动(按换行行,`rows` 为 `FontSet::wrap_text_rows` 的输出):
/// 目标行同列(列超出行尾则落行尾);`extend` 扩选。
pub fn caret_move_vert(
    value: &str,
    caret: CaretState,
    rows: &[(String, usize)],
    dir: i32,
    extend: bool,
) -> CaretState {
    let chars = chars_of(value);
    let caret = clamp_caret(caret, chars.len());
    if rows.is_empty() {
        return caret;
    }
    // 当前行 =最后一个行首 ≤ pos 的行。
    let mut row_idx = 0usize;
    for (i, (_, start)) in rows.iter().enumerate() {
        if *start <= caret.pos {
            row_idx = i;
        } else {
            break;
        }
    }
    let (row_text, row_start) = &rows[row_idx];
    let col = caret.pos.saturating_sub(*row_start).min(row_text.chars().count());
    let next_i = row_idx as i64 + dir as i64;
    if next_i < 0 || next_i as usize >= rows.len() {
        return caret;
    }
    let (next_text, next_start) = &rows[next_i as usize];
    let target = (*next_start + col.min(next_text.chars().count())).min(chars.len());
    if extend {
        CaretState { pos: target, anchor: caret.anchor }
    } else {
        CaretState::collapsed(target)
    }
}

/// 行首/行尾(Home/End;`doc` = 文首/文尾,`extend` 扩选)。
pub fn caret_to_edge(
    value: &str,
    caret: CaretState,
    rows: &[(String, usize)],
    end: bool,
    doc: bool,
    extend: bool,
) -> CaretState {
    let chars = chars_of(value);
    let caret = clamp_caret(caret, chars.len());
    let target = if doc {
        if end {
            chars.len()
        } else {
            0
        }
    } else if end {
        // 行尾 = 当前行文本末尾(\n 之前)。
        let mut row_idx = 0usize;
        for (i, (_, start)) in rows.iter().enumerate() {
            if *start <= caret.pos {
                row_idx = i;
            } else {
                break;
            }
        }
        rows.get(row_idx)
            .map(|(t, s)| s + t.chars().count())
            .unwrap_or(chars.len())
            .min(chars.len())
    } else {
        rows.iter()
            .rev()
            .find(|(_, start)| *start <= caret.pos)
            .map(|(_, start)| *start)
            .unwrap_or(0)
    };
    if extend {
        CaretState { pos: target, anchor: caret.anchor }
    } else {
        CaretState::collapsed(target)
    }
}

/// Ctrl+A 全选。
pub fn select_all(value: &str) -> CaretState {
    CaretState { pos: chars_of(value).len(), anchor: 0 }
}

/// 双击选词 / 三击全选的选区构造。
pub fn select_word(value: &str, pos: usize) -> CaretState {
    let chars = chars_of(value);
    let (a, b) = word_bounds(&chars, pos);
    CaretState { pos: b, anchor: a }
}

/// 取选区文本(无选区返回 None)。
pub fn selected_text(value: &str, caret: CaretState) -> Option<String> {
    let chars = chars_of(value);
    let caret = clamp_caret(caret, chars.len());
    let (a, b) = caret.range();
    if a == b {
        return None;
    }
    Some(join(&chars[a..b]))
}

// ===================== 光标映射(点击 → 索引) =====================

/// 单行控件:行内相对 x → char 索引(最近字符边界;`spans` 为
/// `FontSet::char_spans` 的可见字符跨度)。
pub fn index_from_spans(spans: &[(f64, f64)], x_rel: f64) -> usize {
    for (i, (s, e)) in spans.iter().enumerate() {
        if x_rel < (s + e) / 2.0 {
            return i;
        }
    }
    spans.len()
}

/// textarea:行内相对 `(x_rel, y_rel)` → char 索引(`rows` 为
/// `FontSet::wrap_text_rows` 输出,`line_height` 同绘制)。
pub fn index_from_rows(
    font: &FontSet,
    rows: &[(String, usize)],
    size: f64,
    x_rel: f64,
    y_rel: f64,
    line_height: f64,
) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let row_idx = if line_height <= 0.0 {
        0
    } else {
        let i = (y_rel / line_height).floor();
        if i < 0.0 {
            0
        } else {
            (i as usize).min(rows.len() - 1)
        }
    };
    let (row_text, row_start) = &rows[row_idx];
    let spans = font.char_spans(row_text, size, f64::MAX);
    row_start + index_from_spans(&spans, x_rel)
}

/// 特殊键需要的方向量:`(水平, 垂直, 词跳, 扩选)` 由调用方按 mods 组装;
/// 本结构只承载「移动到目标索引」的统一收口。
pub fn move_to(value: &str, caret: CaretState, target: usize, extend: bool) -> CaretState {
    let len = chars_of(value).len();
    let target = target.min(len);
    if extend {
        CaretState { pos: target, anchor: caret.anchor }
    } else {
        CaretState::collapsed(target)
    }
}
