//! 显示宽度与盒线渲染原语 —— TUI「信息完整 + 行宽精确」的唯一真源。
//!
//! 背景(方案 `tmpPlan/2026-09-24_02-TUI显示信息完整化与宽度自适应方案.md`):
//! 横幅与 `/help` 的盒线原先各自硬编码「边框 58 内宽 + 每行 45/46 填充预算」两套
//! 互不自洽的常量,且截断函数把 `…` **追加**在预算之外(宽度 = max+1)。结果是
//! 同一个盒子的行宽在 59~61 之间跳动(右边框参差),被截断的行顶穿边框;盒宽又与
//! 终端宽度无关,100 列终端上照样按 45 列砍掉 endpoint 与长路径 —— 用户主观感受即
//! 「信息显示不全」。本模块把这件事收敛成两条不变量:
//!
//! - **G4 精确预算**:[`clip`] / [`clip_mid`] 结果宽度**严格 ≤ max**(省略号占预算,不追加);
//! - **G1 盒线等宽**:[`InfoBox::render`] 返回的每一行显示宽度严格相等,且 `≤ 终端宽 - 1`;
//! - **G2/G3 完整优先**:盒宽 = min(内容自然宽, 终端可用宽);超出的内容**折行续排**而不是砍尾,
//!   所以宽终端零损失、窄终端不溢出。路径/endpoint 这类「拆开就不可复制」的值走
//!   [`RowKind::KvKeepTail`] —— 单行 + 中间省略(保尾部文件名)。
//!
//! 另有 [`width`] 的**歧义宽度策略**(G8):`✓ · … → ║` 等 East Asian Ambiguous 字符在
//! 部分 Windows conhost / xterm `-ctwidth` 环境按 2 列渲染,`LAEW_AMBIGUOUS_WIDE=1`
//! 让它们按 2 列参与计算(只改计算不改渲染),给老终端一个 escape hatch。默认关闭 = 现状。

use std::sync::OnceLock;

/// 盒内内容最小宽度(低于此值盒子已不可读,直接按终端能给的就给)。
const MIN_INNER: usize = 20;
/// 单个值最多折行数(超出则末行尾部省略,防止异常超长值把横幅撑成半屏)。
const MAX_WRAP_LINES: usize = 3;
/// 边框左右内衬(各 1 空格)+ 2 根竖线 = 4 列;再留 1 列防「末列自动折行」终端。
const BOX_DECOR_COLUMNS: usize = 5;

// ==================== 宽度度量 ====================

/// 单字符近似显示宽度(列数):CJK / 全角 / 谚文按 2 列;
/// 歧义宽度字符按 [`ambiguous_wide`] 策略取 1 或 2 列;其余 1 列。
///
/// 不引入 `unicode-width` 依赖的轻量近似,覆盖常用 CJK 区段;边缘字符(组合符等)
/// 按 1 列处理,可接受。本函数是全工程宽度计算的唯一真源,
/// `crate::tui::input::char_width` 亦转发至此。
pub fn char_width(c: char) -> u16 {
    char_width_with(c, ambiguous_wide())
}

/// [`char_width`] 的策略注入版 —— 循环内先取一次歧义策略,避免逐字符读 `OnceLock`。
#[inline]
fn char_width_with(c: char, amb: bool) -> u16 {
    let cp = c as u32;
    if (0x1100..=0x115F).contains(&cp) // 谚文 Jamo
        || (0x2E80..=0xA4CF).contains(&cp) // CJK 部首 ~ 彝文(含 4E00-9FFF 统一汉字)
        || (0xAC00..=0xD7A3).contains(&cp) // 谚文音节
        || (0xF900..=0xFAFF).contains(&cp) // CJK 兼容表意
        || (0xFE30..=0xFE4F).contains(&cp) // CJK 兼容形式
        || (0xFF00..=0xFF60).contains(&cp) // 全角 ASCII / 假名
        || (0xFFE0..=0xFFE6).contains(&cp)
    {
        return 2;
    }
    if amb && is_ambiguous_wide(cp) {
        return 2;
    }
    1
}

/// East Asian **Ambiguous** 区段:现代终端渲染 1 列,部分中文 locale 的老终端渲染 2 列。
fn is_ambiguous_wide(cp: u32) -> bool {
    (0x00A1..=0x00FF).contains(&cp) // ¡-ÿ(含 ·)
        || (0x2010..=0x2027).contains(&cp) // 连字符 ~ 水平线(含 … U+2026)
        || (0x2030..=0x205E).contains(&cp)
        || (0x2190..=0x21FF).contains(&cp) // 箭头(→ ← ↑ ↓)
        || (0x2500..=0x257F).contains(&cp) // 盒线制表符(─ │ ║ ╔ ═)
        || (0x2580..=0x259F).contains(&cp) // 块元素
        || (0x25A0..=0x25FF).contains(&cp) // 几何图形(► ◆ ○)
        || (0x2600..=0x26FF).contains(&cp) // 杂项符号(✓ ☐ ⚠)
        || (0x2700..=0x27BF).contains(&cp) // 装饰符号
}

/// `LAEW_AMBIGUOUS_WIDE=1` 时歧义宽度字符按 2 列参与计算(默认 false)。
///
/// 只影响**补空格与折行的计算**,不改渲染字符本身:在把 `║` 渲染成 2 列的老终端上,
/// 开启后盒线重新对齐;新终端保持默认即与现状逐字节一致。
pub fn ambiguous_wide() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| {
        std::env::var("LAEW_AMBIGUOUS_WIDE")
            .map(|v| {
                let v = v.trim().to_ascii_lowercase();
                v == "1" || v == "true" || v == "yes" || v == "on"
            })
            .unwrap_or(false)
    })
}

/// 字符串显示宽度(列数):跳过 ANSI 转义序列,只数可见字符。
pub fn width(s: &str) -> usize {
    let mut w = 0usize;
    for c in strip_ansi(s).chars() {
        w += char_width(c) as usize;
    }
    w
}

/// 渲染用终端列数:探测失败(非 TTY / 管道 / e2e)回退 80。
///
/// 与 `pathfmt::terminal_width()` 的差异是**不设 40 列下限** —— [`InfoBox`] 在任意窄度下
/// 都能保持等宽,40 下限反而会让 <44 列的终端(窄 tmux 面板 / 手机 SSH)撑出折行。
pub fn term_width_for_render() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .clamp(20, 400)
}

/// 剔除 ANSI CSI / OSC 转义序列(度量与折行只看可见文本)。
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // ESC [ … 终止于字母;ESC ] … 终止于 BEL/ST;其余 ESC x 单序列吃掉下一字符
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() || c == '~' {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {}
            None => break,
        }
    }
    out
}

/// 从头截取不超过 `max` 显示列的前缀(不在双宽字符中间截断)。
pub fn head_until(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = char_width(c) as usize;
        if w + cw > max {
            break;
        }
        w += cw;
        out.push(c);
    }
    out
}

/// 从尾截取不超过 `max` 显示列的后缀(不在双宽字符中间截断)。
pub fn tail_from(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars().rev() {
        let cw = char_width(c) as usize;
        if w + cw > max {
            break;
        }
        w += cw;
        out.insert(0, c);
    }
    out
}

// ==================== 截断(G4:结果宽度严格 ≤ max) ====================

/// 尾部省略到宽度**不超过** `max`(省略号计入预算 —— 修复旧 `truncate` 的 `max+1` 越界)。
///
/// `max == 0` → 空串;`max == 1` → 只能放 `…`;`max < 2` 之外都保证不越界。
pub fn clip(s: &str, max: usize) -> String {
    let s = strip_ansi(s);
    if width(&s) <= max {
        return s;
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    format!("{}…", head_until(&s, max - 1))
}

/// 中间省略到宽度**不超过** `max`,优先保尾(路径保文件名、URL 保 host)。
/// 头 30% / 尾 70%(与 `pathfmt::elide_middle` 同策略,此处为不依赖 ANSI 的等价实现)。
pub fn clip_mid(s: &str, max: usize) -> String {
    let s = strip_ansi(s);
    if width(&s) <= max {
        return s;
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    let budget = max - 1;
    // 极端窄预算:头 0 尾 n,等价于 clip 的保尾版
    let tail_w = if budget <= 2 { budget } else { budget * 7 / 10 };
    let head_w = budget - tail_w;
    if head_w == 0 {
        format!("…{}", tail_from(&s, tail_w))
    } else {
        format!("{}…{}", head_until(&s, head_w), tail_from(&s, tail_w))
    }
}

/// 按显示宽度右侧补空格到恰 `w` 列(已超宽则原样返回,交由调用方的预算保证)。
pub fn pad(s: &str, w: usize) -> String {
    let have = width(s);
    if have >= w {
        return s.to_string();
    }
    format!("{}{}", s, " ".repeat(w - have))
}

// ==================== 折行(G3:放不下就续排,不砍尾) ====================

/// 可断行的「软断点」字符:在其**之后**断开最不影响可读性与可复制性。
fn is_break_opportunity(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '/' | '\\' | ':' | ',' | ';' | '|' | '·' | '-' | ')' | ']' | '}' | '】' | '）'
    )
}

/// 按显示宽度折行:先按原 `\n` 硬分,再对每段贪心折到宽度 `w`。
///
/// 断点优先落在 [`is_break_opportunity`] 之后(且必须落在行后半段,避免早断出碎行),
/// 找不到则硬断在能放下的最后一个字符;续行吞掉行首空格。保证:
/// - 每行宽度 `≤ w`(除非 `w == 0` 或单个字符本身宽于 `w`);
/// - 不丢任何非空白字符(只以断点处的空格为代价)。
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    if w == 0 {
        return vec![s.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    for para in s.split('\n') {
        // rest 拥有剩余文本,避免「切片生命周期 vs 循环内重新赋值」的借用绕路
        let mut rest: String = para.trim_end_matches(['\r', ' ']).to_string();
        if rest.is_empty() {
            out.push(String::new());
            continue;
        }
        while width(&rest) > w {
            let chars: Vec<char> = rest.chars().collect();
            // fit = 能放下的最后一个字符下标(不含);soft = 行后半段最晚软断点(新段起点)
            let mut acc = 0usize;
            let mut fit = chars.len();
            let mut soft: Option<usize> = None;
            for (i, &c) in chars.iter().enumerate() {
                let cw = char_width(c) as usize;
                if acc + cw > w {
                    fit = i;
                    break;
                }
                acc += cw;
                if is_break_opportunity(c) && i + 1 > w / 2 {
                    soft = Some(i + 1);
                }
            }
            let cut = soft.unwrap_or(fit).max(1).min(chars.len());
            let line: String = chars[..cut].iter().collect();
            out.push(line.trim_end().to_string());
            rest = chars[cut..].iter().collect::<String>();
            while rest.starts_with(' ') {
                rest.remove(0);
            }
            if rest.is_empty() {
                break;
            }
        }
        if !rest.is_empty() {
            out.push(rest);
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

// ==================== 信息盒(G1/G2/G3) ====================

/// 盒线风格:`Double` 用于启动横幅,`Light` 用于 `/help` 等信息块。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoxStyle {
    /// 双线框(横幅)。
    #[default]
    Double,
    /// 单线框(帮助 / 信息块)。
    Light,
}

impl BoxStyle {
    fn tl(self) -> &'static str {
        match self {
            BoxStyle::Double => "╔",
            BoxStyle::Light => "┌",
        }
    }
    fn tr(self) -> &'static str {
        match self {
            BoxStyle::Double => "╗",
            BoxStyle::Light => "┐",
        }
    }
    fn bl(self) -> &'static str {
        match self {
            BoxStyle::Double => "╚",
            BoxStyle::Light => "└",
        }
    }
    fn br(self) -> &'static str {
        match self {
            BoxStyle::Double => "╝",
            BoxStyle::Light => "┘",
        }
    }
    fn v(self) -> &'static str {
        match self {
            BoxStyle::Double => "║",
            BoxStyle::Light => "│",
        }
    }
    fn h(self) -> &'static str {
        match self {
            BoxStyle::Double => "═",
            BoxStyle::Light => "─",
        }
    }
    fn ml(self) -> &'static str {
        match self {
            BoxStyle::Double => "╠",
            BoxStyle::Light => "├",
        }
    }
    fn mr(self) -> &'static str {
        match self {
            BoxStyle::Double => "╣",
            BoxStyle::Light => "┤",
        }
    }
}

/// 信息盒行种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// 整幅文本(标题行),超长折行。
    Span,
    /// 整幅居中文本。
    SpanCenter,
    /// 标签 + 值;值超长时**折行续排**(不砍尾)。
    Kv,
    /// 标签 + 值;值必须留在单行,超长时**中间省略**保尾(路径 / URL 拆开就不可复制)。
    KvKeepTail,
    /// 分隔线。
    Sep,
}

/// 信息盒的一行。
#[derive(Debug, Clone)]
pub struct Row {
    kind: RowKind,
    label: String,
    value: String,
    /// 标签后是否补 `:`(`cols()` 表头/表格列为 false,避免「命令:」这种带冒号的表头)
    colon: bool,
}

impl Row {
    fn kv(label: String, value: String, kind: RowKind) -> Self {
        Self { kind, label, value, colon: true }
    }
}

/// 声明式信息盒:行数与标签长度与盒宽解耦,渲染结果保证各行等宽。
#[derive(Debug, Clone, Default)]
pub struct InfoBox {
    rows: Vec<Row>,
    style: BoxStyle,
}

impl InfoBox {
    pub fn new(style: BoxStyle) -> Self {
        Self { rows: Vec::new(), style }
    }

    /// 追加标题行(整幅)。
    pub fn span(mut self, text: impl Into<String>) -> Self {
        self.rows.push(Row {
            kind: RowKind::Span,
            label: String::new(),
            value: text.into(),
            colon: true,
        });
        self
    }

    /// 追加居中行。
    pub fn span_center(mut self, text: impl Into<String>) -> Self {
        self.rows.push(Row {
            kind: RowKind::SpanCenter,
            label: String::new(),
            value: text.into(),
            colon: true,
        });
        self
    }

    /// 追加 `标签: 值` 行(值超长折行续排)。
    pub fn kv(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.rows.push(Row::kv(label.into(), value.into(), RowKind::Kv));
        self
    }

    /// 追加「单行 + 中间省略保尾」行(路径 / URL)。
    pub fn kv_keep_tail(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.rows.push(Row::kv(label.into(), value.into(), RowKind::KvKeepTail));
        self
    }

    /// 追加两列表格行(命令 / 说明对照):与 [`InfoBox::kv`] 同列宽,但标签**不带冒号**。
    pub fn cols(mut self, left: impl Into<String>, right: impl Into<String>) -> Self {
        let mut r = Row::kv(left.into(), right.into(), RowKind::Kv);
        r.colon = false;
        self.rows.push(r);
        self
    }

    /// 追加分隔线。
    pub fn sep(mut self) -> Self {
        self.rows.push(Row {
            kind: RowKind::Sep,
            label: String::new(),
            value: String::new(),
            colon: true,
        });
        self
    }

    /// 渲染为若干行(不含末尾 `\n`),保证:
    /// - 每行显示宽度**严格相等**(= 盒总宽);
    /// - 盒总宽 `≤ term_w - 1`(折行安全);
    /// - `term_w` 足够大时内容零省略(不含 `…`)。
    pub fn render(&self, indent: usize, term_w: usize) -> Vec<String> {
        let st = self.style;
        // ① 标签列宽(含冒号)
        let label_col = self
            .rows
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Kv | RowKind::KvKeepTail))
            .map(|r| width(&r.label) + 1) // +1 给冒号
            .max()
            .unwrap_or(0);
        // ② 内容自然宽(不折行需要的内宽)
        let natural = self
            .rows
            .iter()
            .map(|r| match r.kind {
                RowKind::Span | RowKind::SpanCenter => width(&r.value),
                RowKind::Kv | RowKind::KvKeepTail => {
                    label_col + 1 + r.value.split('\n').map(width).max().unwrap_or(0)
                }
                RowKind::Sep => 0,
            })
            .max()
            .unwrap_or(0);
        // ③ 可用内宽 = 终端宽 - 缩进 - 边框装饰(2 竖线 + 2 内衬 + 末列折行保险)
        let avail = term_w.saturating_sub(indent).saturating_sub(BOX_DECOR_COLUMNS).max(4);
        // 终端够宽时给个「至少 MIN_INNER」的地板,免得两三十字的短信息缩成火柴盒;
        // 终端本身很窄时地板自动退到 avail,绝不反向撑破终端。
        let floor = MIN_INNER.min(avail).min(natural.max(8));
        let inner = natural.min(avail).max(floor);
        let line_of = |content: &str| -> String {
            format!(
                "{}{} {} {}",
                " ".repeat(indent),
                st.v(),
                pad(content, inner),
                st.v()
            )
        };
        let mut out: Vec<String> = Vec::new();
        out.push(format!(
            "{}{}{}",
            " ".repeat(indent),
            st.tl(),
            st.h().repeat(inner + 2) + st.tr()
        ));
        for r in &self.rows {
            match r.kind {
                RowKind::Sep => out.push(format!(
                    "{}{}{}{}",
                    " ".repeat(indent),
                    st.ml(),
                    st.h().repeat(inner + 2),
                    st.mr()
                )),
                RowKind::Span | RowKind::SpanCenter => {
                    for line in wrap_capped(&r.value, inner) {
                        if r.kind == RowKind::SpanCenter {
                            let w = width(&line);
                            let extra = inner.saturating_sub(w);
                            let left = extra / 2;
                            out.push(line_of(&format!(
                                "{}{}{}",
                                " ".repeat(left),
                                line,
                                " ".repeat(extra - left)
                            )));
                        } else {
                            out.push(line_of(&line));
                        }
                    }
                }
                kind @ (RowKind::Kv | RowKind::KvKeepTail) => {
                    // 表格型行(cols)标签不带冒号,信息盒型行(kv)带 —— 列宽已按含冒号算好
                    let labelled = if r.colon { format!("{}:", r.label) } else { r.label.clone() };
                    let label_cell = pad(&labelled, label_col);
                    let budget = inner.saturating_sub(label_col + 1);
                    let prefix = format!("{label_cell} ");
                    let cont_prefix = " ".repeat(label_col + 1);
                    if budget < 4 {
                        // 标签列几乎吃满内宽:退化成一整幅行,至少保证信息可见
                        for line in wrap_capped(&r.value, inner) {
                            out.push(line_of(&line));
                        }
                        continue;
                    }
                    let lines: Vec<String> = if kind == RowKind::KvKeepTail {
                        vec![clip_mid(&r.value, budget)]
                    } else {
                        wrap_capped(&r.value, budget)
                    };
                    for (i, line) in lines.iter().enumerate() {
                        let p = if i == 0 { &prefix } else { &cont_prefix };
                        out.push(line_of(&format!("{p}{line}")));
                    }
                }
            }
        }
        out.push(format!(
            "{}{}{}",
            " ".repeat(indent),
            st.bl(),
            st.h().repeat(inner + 2) + st.br()
        ));
        out
    }
}

/// 折行 + 行数封顶:超过 [`MAX_WRAP_LINES`] 时末行尾部省略,并标注省略行数。
fn wrap_capped(s: &str, w: usize) -> Vec<String> {
    let all = wrap_safe(s, w);
    if all.len() <= MAX_WRAP_LINES {
        return all;
    }
    let hidden = all.len() - MAX_WRAP_LINES + 1;
    let mut out: Vec<String> = all[..MAX_WRAP_LINES - 1].to_vec();
    let tail_note = format!("…(余 {} 行)", hidden);
    let note_w = width(&tail_note);
    let keep = w.saturating_sub(note_w).max(1);
    out.push(format!("{}{}", clip(&all[MAX_WRAP_LINES - 1], keep), tail_note));
    out
}

/// [`wrap`] 的**无借用陷阱**实现:逐段贪心折行。
fn wrap_safe(s: &str, w: usize) -> Vec<String> {
    if w == 0 {
        return vec![s.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    for para in s.split('\n') {
        let mut rest: String = para.trim_end_matches(['\r', ' ']).to_string();
        if rest.is_empty() {
            out.push(String::new());
            continue;
        }
        while width(&rest) > w {
            let chars: Vec<char> = rest.chars().collect();
            let mut acc = 0usize;
            let mut fit = chars.len();
            let mut soft: Option<usize> = None;
            for (i, &c) in chars.iter().enumerate() {
                let cw = char_width(c) as usize;
                if acc + cw > w {
                    fit = i;
                    break;
                }
                acc += cw;
                if is_break_opportunity(c) && i + 1 > w / 2 {
                    soft = Some(i + 1);
                }
            }
            let cut = soft.unwrap_or_else(|| if fit == 0 { 1 } else { fit });
            let line: String = chars[..cut.min(chars.len())].iter().collect();
            out.push(line.trim_end().to_string());
            rest = chars[cut.min(chars.len())..].iter().collect::<String>();
            while rest.starts_with(' ') {
                rest.remove(0);
            }
            if rest.is_empty() {
                break;
            }
        }
        if !rest.is_empty() {
            out.push(rest);
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_结果宽度严格不超过_max() {
        // 旧 truncate 的回归点:截断后追加 … 导致宽度 = max+1
        for max in [1usize, 2, 5, 10, 20, 45, 60] {
            let out = clip("当前模型 anthropic liusm191-laew-model @ https://gw.example.com", max);
            assert!(width(&out) <= max, "clip(_, {max}) 越界: 宽 {} > {max} → {out:?}", width(&out));
        }
    }

    #[test]
    fn clip_短文本原样_cjk_不拆半() {
        assert_eq!(clip("短", 10), "短");
        // 预算 5:「显示宽」占 4,放不下第 3 个汉字 → 尾部 …,总宽 5
        let out = clip("显示宽度", 5);
        assert_eq!(width(&out), 5);
        assert!(out.ends_with('…'));
        assert!(!out.contains("度度"));
    }

    #[test]
    fn clip_mid_保尾部文件名() {
        let p = "/very/long/dir/path/DebugReport/debug_report_20260910_123729_d76fb4.md";
        let out = clip_mid(p, 60);
        assert!(width(&out) <= 60);
        assert!(out.ends_with("debug_report_20260910_123729_d76fb4.md"), "文件名被砍: {out}");
        // 更窄时至少保住扩展名(中间省略优先保尾)
        let tight = clip_mid(p, 24);
        assert!(width(&tight) <= 24);
        assert!(tight.ends_with("d76fb4.md"), "尾部未保住: {tight}");
    }

    #[test]
    fn clip_mid_极窄预算不越界() {
        for max in 0usize..6 {
            let out = clip_mid("abcdefghijklmn", max);
            assert!(width(&out) <= max.max(width(&out)) && width(&out) <= max.max(1));
        }
    }

    #[test]
    fn width_跳过_ansi_序列() {
        assert_eq!(width("\x1b[31m红色\x1b[0m"), 4);
        assert_eq!(width("abc"), 3);
    }

    #[test]
    fn pad_补到恰_w_列() {
        assert_eq!(pad("中文", 10), "中文      ");
        assert_eq!(width(&pad("中文", 10)), 10);
        // 恰好等于目标宽度 → 不补
        assert_eq!(pad("中文", 4), "中文");
        // 已超宽 → 原样返回(截断是 clip 的职责,pad 不砍内容)
        assert_eq!(pad("已经很长了啦", 2), "已经很长了啦");
    }

    #[test]
    fn wrap_每行不超宽_且不丢字符() {
        let s = "第一行内容;第二行是英文 with spaces and a very long tail end here";
        let lines = wrap_safe(s, 20);
        assert!(lines.iter().all(|l| width(l) <= 20), "折行超宽: {lines:?}");
        // 折行只可能以断点处的空格为代价,其余字符(含 ;)必须一个不丢
        let joined: String = lines.concat();
        assert_eq!(joined.replace(' ', ""), s.replace(' ', ""));
    }

    #[test]
    fn wrap_优先在分隔符断行() {
        let s = "https://gw.example.com/very/deep/path/v1/chat/completions";
        let lines = wrap_safe(s, 24);
        assert!(lines.iter().all(|l| width(l) <= 24));
        assert!(lines[0].ends_with('/') || lines[0].ends_with("com"), "未按分隔符断: {lines:?}");
    }

    #[test]
    fn infobox_各行严格等宽_多种终端宽() {
        for term_w in [40usize, 50, 60, 70, 80, 90, 100, 120, 160, 200] {
            let box_ = sample_box();
            let lines = box_.render(0, term_w);
            let w0 = width(&lines[0]);
            for l in &lines {
                assert_eq!(width(l), w0, "终端 {term_w} 列下行宽不等:\n{}", lines.join("\n"));
                assert!(w0 <= term_w - 1, "盒宽 {w0} 溢出终端 {term_w}");
            }
        }
    }

    #[test]
    fn infobox_宽终端零省略号() {
        let lines = sample_box().render(0, 200);
        let joined = lines.join("\n");
        assert!(!joined.contains('…'), "宽终端不应截断:\n{joined}");
        assert!(joined.contains("https://gw.example.com/v1/messages"), "endpoint 应完整");
    }

    #[test]
    fn infobox_窄终端折行而非砍尾() {
        let lines = sample_box().render(0, 40);
        let body = lines.iter().filter(|l| l.contains("Session") || l.contains("gw.example")).count();
        assert!(body >= 1);
        // 40 列下模型行必然折成 ≥2 行(值宽 > 预算)
        assert!(lines.len() > 8, "窄终端应有折行,实得 {} 行:\n{}", lines.len(), lines.join("\n"));
    }

    #[test]
    fn infobox_标签列自动对齐() {
        let lines = sample_box().render(0, 120);
        // 找到两个 Kv 行的冒号位置,必须一致
        let colon = |l: &str| l.find(':').unwrap_or(usize::MAX);
        let a = lines.iter().find(|l| l.contains("项目说明:")).expect("行存在");
        let b = lines.iter().find(|l| l.contains("工作目录:")).expect("行存在");
        assert_eq!(colon(a), colon(b), "冒号未对齐:\n{a}\n{b}");
    }

    #[test]
    fn infobox_缩进生效() {
        let lines = sample_box().render(2, 120);
        assert!(lines[0].starts_with("  ╔"), "缩进未生效: {}", lines[0]);
        let w0 = width(&lines[0]);
        assert!(lines.iter().all(|l| width(l) == w0));
    }

    #[test]
    fn cols_行标签不带冒号_kv_行带() {
        let lines = InfoBox::new(BoxStyle::Light)
            .cols("命令", "说明")
            .cols("/help", "显示本帮助")
            .kv("主题", "default")
            .render(0, 60);
        assert!(lines.iter().any(|l| l.contains("命令") && !l.contains("命令:")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("主题:")), "kv 行应带冒号");
        // 两列表格的值列仍与 kv 行的值列同起点(共用 label_col)
        let value_col = |key: &str| -> usize {
            let l = lines.iter().find(|l| l.contains(key)).expect("行存在");
            let at = l.find(key).unwrap() + key.len(); // 字节下标:标签之后
            let rest = &l[at..];
            // 跳过冒号(kv 行有)与填充空格(cols 行无冒号)—— 都是 ASCII,1 字节 = 1 列
            let lead = rest.len() - rest.trim_start_matches([' ', ':']).len();
            width(&l[..at]) + lead
        };
        assert_eq!(value_col("命令"), value_col("主题"), "两列表格与 kv 值列不同起点");
    }

    #[test]
    fn infobox_分隔线与盒宽一致() {
        let lines = InfoBox::new(BoxStyle::Light)
            .span("标题")
            .sep()
            .kv("命令", "/help —— 显示帮助")
            .render(0, 60);
        let w0 = width(&lines[0]);
        let sep = lines.iter().find(|l| l.contains('├')).expect("有分隔线");
        assert_eq!(width(sep), w0, "分隔线不等宽");
    }

    fn sample_box() -> InfoBox {
        InfoBox::new(BoxStyle::Double)
            .span("LsmAgentEmergentWork · laew TUI · v0.1.0")
            .kv("编译时间", "2026-09-24 12:35:08")
            .sep()
            .kv_keep_tail("根目录", "D:/MyLocalGit/LsmAgentEmergentWork")
            .kv_keep_tail("工作目录", "D:/MyLocalGit/LsmAgentEmergentWork/TestWorkSpace")
            .kv("项目说明", "未找到")
            .kv("工作区", "[通用] 非 git")
            .kv(
                "当前模型",
                "[anthropic] liusm191-laew-model/liusm191-laew-model @ https://gw.example.com/v1/messages",
            )
            .kv("Session", "20260924-123508-35d8407d-1790224508748-6f5ddf")
            .kv("主题", "default (切换 /theme [kind])")
            .kv("连接", "Online ✓")
            .kv("日志文件", "llaew_20260924_123508.log (级别: DEBUG)")
    }
}
