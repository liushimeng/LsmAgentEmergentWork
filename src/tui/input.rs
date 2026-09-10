//! 自定义输入处理器(基于 crossterm)—— 固定底部输入组件。
//!
//! 方案见 `tmpPlan/2026-09-09_03-TUI底部固定输入组件方案.md`:
//! - DECSTBM 滚动区(`ESC[1;{rows-area}r`)把输出限制在屏幕上方,
//!   底部 `theme::INPUT_AREA_HEIGHT` 行常驻输入面板,位置固定;
//! - 输入行整行铺 `theme::INPUT_BG` 底色 + `theme::INPUT_FG` 前景,
//!   与输出区(默认底色)一眼可辨;
//! - 补全浮层向上覆盖绘制在面板上方(底部无空间向下展开);
//! - 提交时在滚动区底行回显已提交内容,保留输入痕迹;
//! - 退出路径(Ctrl-D / `/exit` → teardown_pinned)还原滚动区并清空面板,不留残迹。
use crate::tui::completion::{CompletionEngine, CompletionItem};
use crate::tui::mention::{mention_token_at, FileSuggester};
use crate::tui::theme;
use crossterm::{
    cursor::MoveTo,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    style::{Attribute, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use std::io::{self, Write};
use std::time::Duration;

/// 返回 `cursor`(字节偏移,须在字符边界)前一个字符的起始字节偏移。
///
/// 输入循环的 cursor 恒为字节偏移并保持在字符边界上;退格 / 左移按「字符」
/// 语义移动,必须先换算到前一个字符的起始边界,否则 `String::remove/insert`
/// 会触发 `is_char_boundary` panic(修复:TUI 中文输入第 2 个字符必崩)。
fn prev_char_boundary(s: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut i = cursor - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 单字符近似显示宽度(列数):CJK / 全角 / 谚文按 2 列,其余按 1 列。
pub fn char_width(c: char) -> u16 {
    let cp = c as u32;
    if (0x1100..=0x115F).contains(&cp)       // 谚文 Jamo
        || (0x2E80..=0xA4CF).contains(&cp)    // CJK 部首 ~ 彝文(含 4E00-9FFF 统一汉字)
        || (0xAC00..=0xD7A3).contains(&cp)    // 谚文音节
        || (0xF900..=0xFAFF).contains(&cp)    // CJK 兼容表意
        || (0xFE30..=0xFE4F).contains(&cp)    // CJK 兼容形式
        || (0xFF00..=0xFF60).contains(&cp)    // 全角 ASCII / 假名
        || (0xFFE0..=0xFFE6).contains(&cp)
    {
        2
    } else {
        1
    }
}

/// 近似显示宽度(列数):CJK / 全角 / 谚文按 2 列,其余按 1 列。
///
/// 不引入 `unicode-width` 依赖的轻量近似,仅用于光标列号换算;
/// 覆盖常用 CJK 区段,边缘字符(组合符等)按 1 列处理,可接受。
/// 饱和到 u16::MAX,避免超长输入溢出。
pub fn display_width(s: &str) -> u16 {
    let w: u32 = s.chars().map(|c| char_width(c) as u32).sum();
    w.min(u16::MAX as u32) as u16
}

/// 从头截取不超过 `max` 显示列的子串(不在双宽字符中间截断)。
fn fit_width(s: &str, max: u16) -> String {
    window_str(s, 0, max)
}

/// 从 `start` 字节偏移起截取不超过 `avail` 显示列的子串。
fn window_str(s: &str, start: usize, avail: u16) -> String {
    let mut out = String::new();
    let mut w = 0u16;
    for c in s[start..].chars() {
        let cw = char_width(c);
        if w + cw > avail {
            break;
        }
        out.push(c);
        w += cw;
    }
    out
}

/// 计算输入内容的水平滚动可视窗口,保证 cursor 列在 `avail` 宽度内可见。
/// 返回 (窗口起始字节偏移, 光标在窗口内的列号)。
fn visible_window(s: &str, cursor: usize, avail: u16) -> (usize, u16) {
    let avail = avail.max(1);
    let mut start = 0usize;
    while start < cursor && display_width(&s[start..cursor]) >= avail {
        start += s[start..].chars().next().map_or(1, |c| c.len_utf8());
    }
    (start, display_width(&s[start..cursor]))
}

// ========== D6 大粘贴防护(2026-09-10 第二十二轮) ==========
//
// 方案:`tmpPlan/2026-09-10_02-D6大粘贴防护与快速输入批量合并方案.md`
// 参考:pi editor.ts handlePaste(>10 行或 >1000 字符 → marker,提交注入原文,L1573)
//      + claudecode inputPaste.ts(TRUNCATION_THRESHOLD=10000,首 500+尾 500 截断,L1448)

/// 粘贴转 marker 阈值:超过此行数(对齐 pi handlePaste)。
const PASTE_MARKER_MAX_LINES: usize = 10;
/// 粘贴转 marker 阈值:超过此字符数(对齐 pi handlePaste)。
const PASTE_MARKER_MAX_CHARS: usize = 1000;
/// 提交注入截断阈值:单份粘贴超过此字符数时截断注入(对齐 claudecode TRUNCATION_THRESHOLD)。
const PASTE_TRUNCATE_CHARS: usize = 10000;
/// 截断注入保留首/尾字符数(对齐 claudecode PREVIEW_LENGTH/2)。
const PASTE_KEEP_HEAD_CHARS: usize = 500;
const PASTE_KEEP_TAIL_CHARS: usize = 500;

/// 从头截取不超过 `n` 个字符的子串(char 边界安全,不在多字节字符中间截断)。
fn head_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// 从尾截取不超过 `n` 个字符的子串(char 边界安全)。
fn tail_chars(s: &str, n: usize) -> &str {
    let total = s.chars().count();
    if total <= n {
        return s;
    }
    match s.char_indices().nth(total - n) {
        Some((idx, _)) => &s[idx..],
        None => s,
    }
}

/// 粘贴登记簿:生命周期 = 单次 read_line 调用,提交即展开。
///
/// 大粘贴不进输入行(避免 1000 行淹没编辑器 + 逐帧 O(N) 宽度换算卡顿),
/// 输入行只显示 `[粘贴 #N +M 行]` / `[粘贴 #N M 字符]` marker;
/// 提交时精确匹配 marker 展开还原原文(pi paste ID 校验思想:
/// 被用户编辑损坏的 marker 不匹配、原样保留)。
struct PasteRegistry {
    counter: usize,
    /// (marker, 完整原文)
    entries: Vec<(String, String)>,
}

impl PasteRegistry {
    fn new() -> Self {
        Self {
            counter: 0,
            entries: Vec::new(),
        }
    }

    /// 过滤粘贴文本:先做换行归一化(CRLF/CR → LF),再剔除其余控制字符(保留 \n 与 \t)。
    ///
    /// 换行归一化的必要性(2026-09-10 第 23 轮实测):tmux 的 bracketed paste 把
    /// 粘贴内容按按键语义回放,LF 全部转为 CR(LF=0/CR=N,探针 hex 取证);桌面终端
    /// 则保留 LF/CRLF。若不归一化,CR 会在下方控制字符过滤中被剔除,换行信息全丢 →
    /// 大粘贴行数判定恒为 1 行、marker 永不触发,小粘贴「换行转空格」归一也失效,
    /// 内容被拼成一行(D6 防护在 tmux 下完全失效)。
    fn filter(text: &str) -> String {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        normalized
            .chars()
            .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
            .collect()
    }

    /// 判定是否为大粘贴(>10 行或 >1000 字符,对齐 pi handlePaste)。
    fn is_large(filtered: &str) -> bool {
        filtered.matches('\n').count() + 1 > PASTE_MARKER_MAX_LINES
            || filtered.chars().count() > PASTE_MARKER_MAX_CHARS
    }

    /// 登记大粘贴,返回输入行用 marker。
    fn register(&mut self, content: String) -> String {
        self.counter += 1;
        let lines = content.matches('\n').count() + 1;
        let marker = if lines > PASTE_MARKER_MAX_LINES {
            format!("[粘贴 #{} +{} 行]", self.counter, lines)
        } else {
            format!("[粘贴 #{} {} 字符]", self.counter, content.chars().count())
        };
        self.entries.push((marker.clone(), content));
        marker
    }

    /// 提交时展开:精确匹配 marker → 原文;单份 >10000 字符截断为
    /// 首 500 + 省略标注 + 尾 500(claudecode inputPaste 语义,防超大粘贴打爆上下文)。
    /// marker 被编辑损坏/编号未知则不展开,原样保留。
    fn expand(&self, buffer: &str) -> String {
        let mut out = buffer.to_string();
        for (marker, content) in &self.entries {
            if !out.contains(marker.as_str()) {
                continue;
            }
            let inject = Self::truncate_for_inject(content);
            out = out.replace(marker.as_str(), &inject);
        }
        out
    }

    /// 截断注入:超过阈值保留首尾各 500 字符,中间以省略标注替换。
    fn truncate_for_inject(content: &str) -> String {
        let total = content.chars().count();
        if total <= PASTE_TRUNCATE_CHARS {
            return content.to_string();
        }
        let omitted = total - PASTE_KEEP_HEAD_CHARS - PASTE_KEEP_TAIL_CHARS;
        format!(
            "{}\n[...中间省略 {} 字符...]\n{}",
            head_chars(content, PASTE_KEEP_HEAD_CHARS),
            omitted,
            tail_chars(content, PASTE_KEEP_TAIL_CHARS),
        )
    }
}

/// 粘贴入缓冲区的结果。
enum PasteInsert {
    /// 小粘贴:归一化文本直接插入(\n/\t 已转空格,单行输入语义)。
    Inline(String),
    /// 大粘贴:已登记,插入 marker。
    Marker(String),
}

/// 粘贴统一入口:过滤 → 大小判定 → 归一化插入或登记 marker。
fn handle_paste_text(text: &str, registry: &mut PasteRegistry) -> PasteInsert {
    let filtered = PasteRegistry::filter(text);
    if PasteRegistry::is_large(&filtered) {
        PasteInsert::Marker(registry.register(filtered))
    } else {
        // 单行输入语义:\n/\t 归一为空格,避免换行被渲染成乱码
        PasteInsert::Inline(filtered.replace(['\n', '\t'], " "))
    }
}

/// 补全菜单 + Tab/Enter 行为的决策结果(2026-09-10 第 24 轮 Enter 吞键修复)。
/// - `Submit(text)`  : 调用方应以 text 作为输入行提交(进入 handle_user_input);
/// - `AcceptOnly(s)` : 调用方仅把 s 写入输入框,不提交(原「接受补全」语义,Tab 路径);
/// - `None`          : 无操作(补全菜单未开 + Tab,或边界场景)。
#[derive(Debug, Clone, PartialEq, Eq)]
enum CompletionDecision {
    Submit(String),
    AcceptOnly(String),
    None,
}

/// @ 提及补全状态(D1,2026-09-10 第二十八轮,L1427 简化实现)。
///
/// 与斜杠命令补全共用浮层渲染,但语义不同:
/// - 斜杠:replacement 替换**整个 buffer**;
/// - @ 提及:replacement 只拼接替换**当前 @token**(`token_start..cursor`),
///   Tab 接受(目录尾随 `/` 保持浮层继续钻取,文件尾随空格闭合),
///   Enter 原样提交 buffer(不触发 slash 的真前缀自动提交)。
///
/// suggester 懒加载:首次检测到 @token 才构建(首次走 walkdir 快照),
/// 生命周期 = 单次行编辑;工作目录 = 进程 cwd(与工程「工作目录」定义一致)。
struct MentionCompletion {
    suggester: Option<FileSuggester>,
    token_start: Option<usize>,
}

impl MentionCompletion {
    fn new() -> Self {
        Self {
            suggester: None,
            token_start: None,
        }
    }

    /// 检测光标前是否处于 @token 内;是则(必要时懒建 suggester)返回候选。
    fn suggest(&mut self, buffer: &str, cursor: usize) -> Vec<CompletionItem> {
        match mention_token_at(buffer, cursor) {
            Some((start, frag)) => {
                self.token_start = Some(start);
                let sugg = self.suggester.get_or_insert_with(|| {
                    let cwd = std::env::current_dir().unwrap_or_default();
                    FileSuggester::new(&cwd)
                });
                sugg.suggest(&frag)
            }
            None => {
                self.token_start = None;
                Vec::new()
            }
        }
    }

    fn deactivate(&mut self) {
        self.token_start = None;
    }
}

/// 决定 Tab/Enter 在补全菜单中的行为。
///
/// 规则(2026-09-10 第 24 轮):
/// - 补全未开:Enter → Submit(buffer);Tab → None。
/// - 补全已开 + buffer.trim() == replacement.trim():Enter/Tab → Submit(buffer)(已匹配,避免再加尾随空格)。
/// - 补全已开 + Enter + buffer 是 replacement 的真前缀:Submit(replacement)(一键补全并提交,
///   解决 `/provider` 等高频命令需要按 2 次 Enter 才能进入子屏的 UX 问题)。
/// - 补全已开 + Tab:AcceptOnly(replacement)(只补全不提交,符合 Tab 的传统语义)。
/// - 补全已开 + Enter + buffer 不是任何补全项真前缀:Submit(buffer)(兜底:不让 Enter 被吞)。
fn completion_enter_tab_decision(
    key: KeyCode,
    buffer: &str,
    completion_active: bool,
    completion_items: &[CompletionItem],
    completion_index: usize,
) -> CompletionDecision {
    // 补全菜单未开:Enter 直接提交当前 buffer;Tab 无操作
    if !completion_active || completion_items.is_empty() {
        return match key {
            KeyCode::Enter => CompletionDecision::Submit(buffer.to_string()),
            _ => CompletionDecision::None,
        };
    }
    let item = match completion_items.get(completion_index) {
        Some(i) => i,
        None => return CompletionDecision::None,
    };
    let trimmed_buf = buffer.trim();
    let trimmed_rep = item.replacement.trim();
    // 路径 A:已匹配(忽略尾随空格)→ 直接以 buffer 提交
    if trimmed_buf == trimmed_rep {
        return CompletionDecision::Submit(buffer.to_string());
    }
    // 路径 B:Enter + buffer 是补全项真前缀 → 用补全项内容直接提交
    if key == KeyCode::Enter
        && trimmed_rep.starts_with(trimmed_buf)
        && trimmed_rep.len() > trimmed_buf.len()
    {
        return CompletionDecision::Submit(item.replacement.clone());
    }
    // 路径 C:Tab → 接受补全但不提交;Enter 兜底 → 以 buffer 提交,不让 Enter 被吞成空操作
    match key {
        KeyCode::Tab => CompletionDecision::AcceptOnly(item.replacement.clone()),
        KeyCode::Enter => CompletionDecision::Submit(buffer.to_string()),
        _ => CompletionDecision::None,
    }
}

/// 屏幕布局:滚动区 + 底部固定输入面板的行号计算。
#[derive(Debug, Clone, Copy)]
struct Layout {
    cols: u16,
    rows: u16,
    /// 面板高度(1 = 矮终端降级,只保留输入行)。
    area_h: u16,
    /// 面板顶行(0-indexed;3 行面板时为分隔线行)。
    panel_top: u16,
    /// 输入行(0-indexed)。
    input_y: u16,
    /// 快捷键提示行(0-indexed;降级时为 None)。
    hint_y: Option<u16>,
}

impl Layout {
    fn detect() -> Self {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        Self::compute(cols, rows)
    }

    fn compute(cols: u16, rows: u16) -> Self {
        // 至少需要 滚动区 3 行 + 面板 3 行,否则降级为 1 行面板
        let area_h = if rows >= theme::INPUT_AREA_HEIGHT + 3 {
            theme::INPUT_AREA_HEIGHT
        } else {
            1
        };
        let (panel_top, input_y, hint_y) = if area_h >= 3 {
            (rows - 3, rows - 2, Some(rows - 1))
        } else {
            (rows.saturating_sub(1), rows.saturating_sub(1), None)
        };
        Self {
            cols,
            rows,
            area_h,
            panel_top,
            input_y,
            hint_y,
        }
    }

    /// 滚动区底行(1-indexed,DECSTBM 参数)。
    fn scroll_bottom(&self) -> u16 {
        // 饱和减法:极端环境(如 script 录屏)terminal::size 可能返回 (0,0),
        // 直接相减会 u16 下溢成 65535
        self.rows.saturating_sub(self.area_h).max(1)
    }

    /// 滚动区最后一行(0-indexed,输出光标锚点)。
    fn scroll_last_row(&self) -> u16 {
        self.scroll_bottom().saturating_sub(1)
    }
}

/// 输入处理结果。
pub enum InputResult {
    /// 用户提交了输入行。
    Submitted(String),
    /// 用户请求退出（Ctrl-D 或空输入时 Ctrl-D）。
    Exit,
    /// 中断（Ctrl-C），未提交。
    Interrupted,
}

/// 还原终端:重置 DECSTBM 滚动区并清空底部输入面板,光标移到面板顶行。
fn teardown_pinned_layout(stdout: &mut impl Write, layout: &Layout) -> io::Result<()> {
    // DECRST: 重置滚动区为全屏
    execute!(stdout, ResetColor, Print("\x1b[r"))?;
    for y in layout.panel_top..layout.rows {
        execute!(stdout, MoveTo(0, y), Clear(ClearType::CurrentLine))?;
    }
    execute!(stdout, MoveTo(0, layout.panel_top))?;
    stdout.flush()?;
    Ok(())
}

/// 会话结束(`/exit`)时拆除固定输入区。主循环退出分支调用(mod.rs)。
pub fn teardown_pinned() {
    let layout = Layout::detect();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let _ = teardown_pinned_layout(&mut stdout, &layout);
}

/// 自定义输入处理器(固定底部输入组件)。
pub struct InputHandler;

impl InputHandler {
    pub fn new() -> Self {
        Self
    }

    /// 读取一行输入,集成补全浮层;输入组件固定屏幕底部。
    ///
    /// # 参数
    /// - `prompt`: 提示符字符串（如 ">> "）
    /// - `engine`: 补全引擎
    pub fn read_line(&self, prompt: &str, engine: &CompletionEngine) -> io::Result<InputResult> {
        // 进入原始模式 + 开启 bracketed paste(终端粘贴整体以 Event::Paste 送达,
        // 不再逐字符事件 —— 对齐 pi editor.ts bracketed paste 模式,L1573)
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout().lock();
        let _ = execute!(stdout, EnableBracketedPaste);
        drop(stdout);

        let result = self.read_line_inner(prompt, engine);

        // 退出原始模式 + 关闭 bracketed paste(无论成功失败)
        let mut stdout = io::stdout().lock();
        let _ = execute!(stdout, DisableBracketedPaste);
        drop(stdout);
        let _ = terminal::disable_raw_mode();

        result
    }

    fn read_line_inner(&self, prompt: &str, engine: &CompletionEngine) -> io::Result<InputResult> {
        let mut layout = Layout::detect();
        let mut buffer = String::new();
        let mut cursor: usize = 0;
        let mut completion_active = false;
        let mut completion_index: usize = 0;
        let mut completion_items: Vec<CompletionItem> = Vec::new();
        let mut overlay_lines: u16 = 0;
        // @ 提及补全状态(D1):token 起点 + 懒加载文件建议器
        let mut mention = MentionCompletion::new();
        // 粘贴登记簿:本次行编辑期间的大粘贴原文(D6,提交时 marker 展开)
        let mut pastes = PasteRegistry::new();
        // 批量合并排空时暂存的非字符事件(crossterm 无 pushback,不能丢)
        let mut pending: Option<Event> = None;

        let stdout = io::stdout();
        let mut stdout = stdout.lock();

        // 设置滚动区 + 绘制固定面板 + 空输入行
        self.enter_pinned(&mut stdout, &layout)?;
        self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;

        loop {
            // 读取事件(优先取批量合并时暂存的 pending)
            let ev = match pending.take() {
                Some(e) => e,
                None => match event::read() {
                    Ok(e) => e,
                    Err(_) => break,
                },
            };

            match ev {
                // bracketed paste:终端粘贴整体送达(D6/L1573)
                Event::Paste(text) => {
                    match handle_paste_text(&text, &mut pastes) {
                        PasteInsert::Inline(s) | PasteInsert::Marker(s) => {
                            buffer.insert_str(cursor, &s);
                            cursor += s.len();
                        }
                    }
                    overlay_lines = self.update_completion(
                        &mut stdout,
                        &layout,
                        prompt,
                        &buffer,
                        cursor,
                        &mut completion_active,
                        &mut completion_index,
                        &mut completion_items,
                        overlay_lines,
                        engine,
                        &mut mention,
                    )?;
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-C: 中断当前输入(面板保留,光标锚定滚动区底行)
                            self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                            self.redraw_line(&mut stdout, &layout, prompt, "", 0)?;
                            execute!(stdout, MoveTo(0, layout.scroll_last_row()))?;
                            stdout.flush()?;
                            return Ok(InputResult::Interrupted);
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-D: 若输入为空则退出(还原滚动区 + 清空面板)
                            if buffer.is_empty() {
                                self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                teardown_pinned_layout(&mut stdout, &layout)?;
                                return Ok(InputResult::Exit);
                            }
                        }
                        // —— 以下是 readline 标准行编辑快捷键 ——
                        // 标准 readline 中 Ctrl-U=kill to beginning of line, Ctrl-K=kill to end of line,
                        // Ctrl-A=beginning of line, Ctrl-E=end of line, Ctrl-W=kill previous word。
                        // 在 tmux/普通终端下这些按键由 crossterm 映射为带 CONTROL 修饰的 Char,
                        // 必须显式拦截,否则会落入下方 Char(c) 兜底分支被当作普通字符插入(实测 u/model 残留 u 即此问题)。
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-U: 删除到行首(kill to beginning of line)
                            buffer.truncate(0);
                            cursor = 0;
                            overlay_lines = self.update_completion(
                                &mut stdout,
                                &layout,
                                prompt,
                                &buffer,
                                cursor,
                                &mut completion_active,
                                &mut completion_index,
                                &mut completion_items,
                                overlay_lines,
                                engine,
                                &mut mention,
                            )?;
                        }
                        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-K: 删除到行尾(kill to end of line)
                            buffer.truncate(cursor);
                            overlay_lines = self.update_completion(
                                &mut stdout,
                                &layout,
                                prompt,
                                &buffer,
                                cursor,
                                &mut completion_active,
                                &mut completion_index,
                                &mut completion_items,
                                overlay_lines,
                                engine,
                                &mut mention,
                            )?;
                        }
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-A: 光标移到行首(beginning of line)
                            cursor = 0;
                            self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                        }
                        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-E: 光标移到行尾(end of line)
                            cursor = buffer.len();
                            self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                        }
                        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-W: 删除前一个 word(kill previous word)
                            // word 定义为连续非空白字符序列,前导空白一并清除。
                            let mut new_cursor = cursor;
                            while new_cursor > 0 {
                                let prev = prev_char_boundary(&buffer, new_cursor);
                                if buffer[prev..new_cursor]
                                    .chars()
                                    .next()
                                    .map_or(false, char::is_whitespace)
                                {
                                    new_cursor = prev;
                                } else {
                                    break;
                                }
                            }
                            while new_cursor > 0 {
                                let prev = prev_char_boundary(&buffer, new_cursor);
                                if buffer[prev..new_cursor]
                                    .chars()
                                    .next()
                                    .map_or(false, |c| !char::is_whitespace(c))
                                {
                                    new_cursor = prev;
                                } else {
                                    break;
                                }
                            }
                            if new_cursor != cursor {
                                buffer.replace_range(new_cursor..cursor, "");
                                cursor = new_cursor;
                                overlay_lines = self.update_completion(
                                    &mut stdout,
                                    &layout,
                                    prompt,
                                    &buffer,
                                    cursor,
                                    &mut completion_active,
                                    &mut completion_index,
                                    &mut completion_items,
                                    overlay_lines,
                                    engine,
                                    &mut mention,
                                )?;
                            }
                        }
                        KeyCode::Esc => {
                            // Esc: 关闭补全浮层;非命令输入时清空输入(对齐 bash Esc 语义)
                            if completion_active {
                                // 补全浮层当前可见:关闭浮层,保留输入
                                overlay_lines =
                                    self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                completion_active = false;
                                completion_items.clear();
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            } else if !buffer.is_empty() && !buffer.trim_start().starts_with('/') {
                                // 非命令输入(不以 / 开头):清空输入缓冲区
                                buffer.clear();
                                cursor = 0;
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                            // 命令输入(以 / 开头)且补全未激活时:保留输入(用户可能想按 Enter 提交)
                        }
                        KeyCode::Up => {
                            // 上箭头：在补全列表中向上移动
                            if completion_active && !completion_items.is_empty() {
                                completion_index = if completion_index == 0 {
                                    completion_items.len() - 1
                                } else {
                                    completion_index - 1
                                };
                                overlay_lines = self.draw_overlay(
                                    &mut stdout,
                                    &layout,
                                    completion_index,
                                    &completion_items,
                                )?;
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Down => {
                            // 下箭头：在补全列表中向下移动
                            if completion_active && !completion_items.is_empty() {
                                completion_index = (completion_index + 1) % completion_items.len();
                                overlay_lines = self.draw_overlay(
                                    &mut stdout,
                                    &layout,
                                    completion_index,
                                    &completion_items,
                                )?;
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Tab | KeyCode::Enter => {
                            // @ 提及补全 + Tab(D1):拼接替换当前 token,不动 buffer 其余部分。
                            // 目录(尾随 /)保持浮层继续钻取;文件(尾随空格)闭合浮层。
                            // Enter 不做特殊处理,落到下方 slash 决策的兜底分支 = 原样提交。
                            if key.code == KeyCode::Tab
                                && completion_active
                                && mention.token_start.is_some()
                            {
                                if let (Some(start), Some(item)) = (
                                    mention.token_start,
                                    completion_items.get(completion_index),
                                ) {
                                    let drill = item.replacement.ends_with('/');
                                    let rep = item.replacement.clone();
                                    // start 指向 `@` 本身。第二十八轮修订:replacement 已含 `@` 前缀,
                                    // 故替换区间改为 `start..cursor`(整个 @token 含 @ 一起替换),
                                    // 避免重复 `@`;cursor 落在 replacement 末尾。
                                    buffer.replace_range(start..cursor, &rep);
                                    cursor = start + rep.len();
                                    if drill {
                                        // 继续钻取:按新 fragment 刷新候选
                                        overlay_lines = self.update_completion(
                                            &mut stdout,
                                            &layout,
                                            prompt,
                                            &buffer,
                                            cursor,
                                            &mut completion_active,
                                            &mut completion_index,
                                            &mut completion_items,
                                            overlay_lines,
                                            engine,
                                            &mut mention,
                                        )?;
                                    } else {
                                        completion_active = false;
                                        completion_items.clear();
                                        mention.deactivate();
                                        overlay_lines = self.clear_overlay(
                                            &mut stdout,
                                            &layout,
                                            overlay_lines,
                                        )?;
                                        self.redraw_line(
                                            &mut stdout,
                                            &layout,
                                            prompt,
                                            &buffer,
                                            cursor,
                                        )?;
                                    }
                                }
                                continue;
                            }
                            // Tab/Enter 在补全菜单打开时行为分流(2026-09-10 第 24 轮修复):
                            //  - Tab  永远只「接受补全」,不提交(用户预期);
                            //  - Enter 若 buffer 是当前选中补全项的「真前缀」,则用补全项的完整内容直接
                            //    提交(避免 `/provider` 等高频命令需要按 2 次 Enter 才能进入子屏);
                            //  - Enter 若 buffer 已等于补全项(忽略尾随空格),直接以 buffer 提交
                            //    (容错:与既有路径 A 一致);
                            //  - Enter 若 buffer 不是任何补全项的真前缀,则「接受补全 + 提交原 buffer」
                            //    (兜底:不让 Enter 被吞成空操作)。
                            match completion_enter_tab_decision(
                                key.code,
                                &buffer,
                                completion_active,
                                &completion_items,
                                completion_index,
                            ) {
                                CompletionDecision::Submit(text) => {
                                    return self.submit(
                                        &mut stdout,
                                        &layout,
                                        prompt,
                                        text,
                                        overlay_lines,
                                        &pastes,
                                    );
                                }
                                CompletionDecision::AcceptOnly(replacement) => {
                                    buffer = replacement;
                                    cursor = buffer.len();
                                    completion_active = false;
                                    completion_items.clear();
                                    overlay_lines = self.clear_overlay(
                                        &mut stdout,
                                        &layout,
                                        overlay_lines,
                                    )?;
                                    self.redraw_line(
                                        &mut stdout,
                                        &layout,
                                        prompt,
                                        &buffer,
                                        cursor,
                                    )?;
                                }
                                CompletionDecision::None => {
                                    // 补全未开 + Tab:无操作(保持静默);
                                    // 补全未开 + Enter:辅助函数已返回 Submit(text=buffer),
                                    // 不会到这里,所以下方兜底不会再跑到。
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            // 退格：删除光标前字符(cursor 为字节偏移,先回退到前一个
                            // 字符的起始边界再删,保证多字节 CJK 不触发 is_char_boundary panic)
                            if cursor > 0 {
                                cursor = prev_char_boundary(&buffer, cursor);
                                buffer.remove(cursor);
                                overlay_lines = self.update_completion(
                                    &mut stdout,
                                    &layout,
                                    prompt,
                                    &buffer,
                                    cursor,
                                    &mut completion_active,
                                    &mut completion_index,
                                    &mut completion_items,
                                    overlay_lines,
                                    engine,
                                    &mut mention,
                                )?;
                            }
                        }
                        // 兜底：某些终端环境下 crossterm 未将退格映射为 KeyCode::Backspace，
                        // 而是作为 Char('\x7f') (DEL) 或 Char('\x08') (BS) 传递。
                        // 此处显式拦截，避免退格字符落入 Char(c) 分支被当作普通字符插入。
                        KeyCode::Char('\x7f') | KeyCode::Char('\x08') => {
                            if cursor > 0 {
                                cursor = prev_char_boundary(&buffer, cursor);
                                buffer.remove(cursor);
                                overlay_lines = self.update_completion(
                                    &mut stdout,
                                    &layout,
                                    prompt,
                                    &buffer,
                                    cursor,
                                    &mut completion_active,
                                    &mut completion_index,
                                    &mut completion_items,
                                    overlay_lines,
                                    engine,
                                    &mut mention,
                                )?;
                            }
                        }
                        KeyCode::Delete => {
                            // Delete：删除光标处字符(cursor 在字符边界,remove 按字节偏移删一个字符)
                            if cursor < buffer.len() {
                                buffer.remove(cursor);
                                overlay_lines = self.update_completion(
                                    &mut stdout,
                                    &layout,
                                    prompt,
                                    &buffer,
                                    cursor,
                                    &mut completion_active,
                                    &mut completion_index,
                                    &mut completion_items,
                                    overlay_lines,
                                    engine,
                                    &mut mention,
                                )?;
                            }
                        }
                        KeyCode::Left => {
                            // 左箭头：移动光标(按字符,不按字节)
                            if cursor > 0 {
                                cursor = prev_char_boundary(&buffer, cursor);
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Right => {
                            // 右箭头：移动光标(前进一个 UTF-8 字符)
                            if cursor < buffer.len() {
                                cursor +=
                                    buffer[cursor..].chars().next().map_or(0, |c| c.len_utf8());
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Home => {
                            cursor = 0;
                            self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                        }
                        KeyCode::End => {
                            cursor = buffer.len();
                            self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                        }
                        KeyCode::Char(c) => {
                            // 可打印字符：插入到光标位置(cursor 为字节偏移,
                            // 增量按 len_utf8 推进——修复中文输入第 2 字符 panic)
                            buffer.insert(cursor, c);
                            cursor += c.len_utf8();
                            // D6 快速输入批量合并:IME 一次上屏多字 / 无 bracketed paste
                            // 的旧终端粘贴,会以极快的连续 Char 事件到达;排空已到达的
                            // 同类事件一次性插入,最后统一一次重绘,避免逐字重绘卡顿。
                            // 非字符事件存入 pending,下轮循环优先处理(不丢事件)。
                            loop {
                                match event::poll(Duration::ZERO) {
                                    Ok(true) => match event::read() {
                                        Ok(Event::Key(k2))
                                            if k2.kind == KeyEventKind::Press
                                                && matches!(k2.code, KeyCode::Char(_))
                                                && (k2.modifiers.is_empty()
                                                    || k2.modifiers == KeyModifiers::SHIFT) =>
                                        {
                                            if let KeyCode::Char(c2) = k2.code {
                                                buffer.insert(cursor, c2);
                                                cursor += c2.len_utf8();
                                            }
                                        }
                                        Ok(other) => {
                                            pending = Some(other);
                                            break;
                                        }
                                        Err(_) => break,
                                    },
                                    _ => break,
                                }
                            }
                            overlay_lines = self.update_completion(
                                &mut stdout,
                                &layout,
                                prompt,
                                &buffer,
                                cursor,
                                &mut completion_active,
                                &mut completion_index,
                                &mut completion_items,
                                overlay_lines,
                                engine,
                                &mut mention,
                            )?;
                        }
                        _ => {}
                    }
                }
                Event::Resize(cols, rows) => {
                    // 终端尺寸变化:重算布局 → 重设滚动区 → 全量重绘面板/浮层/输入行
                    layout = Layout::compute(cols, rows);
                    self.enter_pinned(&mut stdout, &layout)?;
                    overlay_lines = if completion_active && !completion_items.is_empty() {
                        self.draw_overlay(
                            &mut stdout,
                            &layout,
                            completion_index,
                            &completion_items,
                        )?
                    } else {
                        0
                    };
                    self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                }
                _ => {}
            }
        }

        Ok(InputResult::Exit)
    }

    /// 设置 DECSTBM 滚动区并绘制固定面板(分隔线 + 快捷键提示行)。
    fn enter_pinned(&self, stdout: &mut impl Write, layout: &Layout) -> io::Result<()> {
        // DECSTBM: 滚动区 = 1..=scroll_bottom(1-indexed),底部 area_h 行排除在外
        execute!(
            stdout,
            ResetColor,
            Print(format!("\x1b[1;{}r", layout.scroll_bottom()))
        )?;
        // 分隔线
        if layout.area_h >= 3 {
            execute!(
                stdout,
                MoveTo(0, layout.panel_top),
                SetForegroundColor(theme::INPUT_BORDER_FG),
                Print("─".repeat(layout.cols as usize)),
                ResetColor,
            )?;
        }
        // 快捷键提示行
        if let Some(hint_y) = layout.hint_y {
            execute!(
                stdout,
                MoveTo(0, hint_y),
                ResetColor,
                Clear(ClearType::CurrentLine),
                SetForegroundColor(theme::INPUT_HINT_FG),
                Print(fit_width(
                    "  ↑↓ 选择补全  Enter 补全并提交  Tab 仅补全  Esc 关闭补全/清行  Ctrl-D 退出  /help 帮助",
                    layout.cols,
                )),
                ResetColor,
            )?;
        }
        stdout.flush()?;
        Ok(())
    }

    /// 重绘输入行:整行铺 INPUT_BG 底色,提示符 + 水平滚动窗口文本,光标定位。
    fn redraw_line(
        &self,
        stdout: &mut impl Write,
        layout: &Layout,
        prompt: &str,
        buffer: &str,
        cursor: usize,
    ) -> io::Result<()> {
        let prompt_w = display_width(prompt);
        let avail = layout.cols.saturating_sub(prompt_w).max(1);
        let (start, cursor_col) = visible_window(buffer, cursor, avail);
        let window = window_str(buffer, start, avail);
        execute!(
            stdout,
            MoveTo(0, layout.input_y),
            SetBackgroundColor(theme::INPUT_BG),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme::INPUT_PROMPT_FG),
            SetAttribute(Attribute::Bold),
            Print(prompt),
            SetAttribute(Attribute::NormalIntensity),
            SetForegroundColor(theme::INPUT_FG),
            Print(window),
            ResetColor,
        )?;
        execute!(stdout, MoveTo(prompt_w + cursor_col, layout.input_y))?;
        stdout.flush()?;
        Ok(())
    }

    /// 提交:清浮层 → 滚动区底行回显已提交内容 → 清空面板输入行 → 光标锚定滚动区底行。
    ///
    /// 回显用 **marker 版** buffer(大粘贴不刷屏),返回的 `Submitted` 为
    /// **展开还原版**(marker → 原文,超大粘贴截断注入,见 PasteRegistry::expand)。
    #[allow(clippy::too_many_arguments)]
    fn submit(
        &self,
        stdout: &mut impl Write,
        layout: &Layout,
        prompt: &str,
        buffer: String,
        overlay_lines: u16,
        pastes: &PasteRegistry,
    ) -> io::Result<InputResult> {
        self.clear_overlay(stdout, layout, overlay_lines)?;
        // 回显到滚动区底行(保留用户输入痕迹,随历史输出一起滚动;
        // 大粘贴只回显 marker,避免 1000 行原文淹没对话区)
        execute!(
            stdout,
            MoveTo(0, layout.scroll_last_row()),
            ResetColor,
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme::DIM),
            Print(format!("{prompt}{buffer}")),
            Print("\r\n"),
            ResetColor,
        )?;
        // 面板输入行清空(组件常显)
        self.redraw_line(stdout, layout, prompt, "", 0)?;
        // 输出光标锚定滚动区底行,后续 println! 任务输出在滚动区内滚动
        execute!(stdout, MoveTo(0, layout.scroll_last_row()))?;
        stdout.flush()?;
        Ok(InputResult::Submitted(pastes.expand(&buffer)))
    }

    /// 绘制补全浮层(面板上方,向上排布),返回占用行数。
    fn draw_overlay(
        &self,
        stdout: &mut impl Write,
        layout: &Layout,
        selected: usize,
        items: &[CompletionItem],
    ) -> io::Result<u16> {
        if items.is_empty() || layout.panel_top == 0 {
            return Ok(0);
        }
        let avail = layout.panel_top as usize;
        // 预留 1 行快捷键提示;空间不足时优先保证候选项
        let show = items.len().min(avail.saturating_sub(1).max(1));
        let with_hint = avail > show;
        let hint_lines = u16::from(with_hint);
        let first_y = layout.panel_top - show as u16 - hint_lines;

        if with_hint {
            execute!(
                stdout,
                MoveTo(0, first_y),
                ResetColor,
                Clear(ClearType::CurrentLine),
                SetForegroundColor(theme::DIM),
                Print(fit_width(
                    "  ↑↓ 选择  Enter/Tab 接受  Esc 关闭",
                    layout.cols
                )),
                ResetColor,
            )?;
        }
        for (i, item) in items.iter().take(show).enumerate() {
            let y = first_y + hint_lines + i as u16;
            // 选中项：► 前缀 + 反白 + 加粗 + 高亮色(选中效果视觉规范);未选中项：灰色
            let prefix = if i == selected {
                format!(" ► {}  ", item.display)
            } else {
                format!("   {}  ", item.display)
            };
            let pw = display_width(&prefix);
            let desc = fit_width(
                &format!("  {}", item.description),
                layout.cols.saturating_sub(pw),
            );
            execute!(
                stdout,
                MoveTo(0, y),
                ResetColor,
                Clear(ClearType::CurrentLine)
            )?;
            if i == selected {
                execute!(
                    stdout,
                    SetForegroundColor(theme::HIGHLIGHT_FG),
                    SetAttribute(Attribute::Bold),
                    SetAttribute(Attribute::Reverse),
                    Print(fit_width(&prefix, layout.cols)),
                    SetAttribute(Attribute::Reset),
                    SetForegroundColor(theme::DIM),
                    Print(desc),
                    ResetColor,
                )?;
            } else {
                execute!(
                    stdout,
                    SetForegroundColor(theme::DIM),
                    Print(fit_width(&prefix, layout.cols)),
                    Print(desc),
                    ResetColor,
                )?;
            }
        }
        stdout.flush()?;
        Ok(show as u16 + hint_lines)
    }

    /// 清除补全浮层(逐行清空面板上方 lines 行),返回 0(新的占用行数)。
    fn clear_overlay(
        &self,
        stdout: &mut impl Write,
        layout: &Layout,
        lines: u16,
    ) -> io::Result<u16> {
        if lines > 0 {
            let start = layout.panel_top.saturating_sub(lines);
            for y in start..layout.panel_top {
                execute!(
                    stdout,
                    MoveTo(0, y),
                    ResetColor,
                    Clear(ClearType::CurrentLine)
                )?;
            }
            stdout.flush()?;
        }
        Ok(0)
    }

    /// 更新补全浮层（输入变化时调用）:先清旧浮层,再按新候选重绘,最后重绘输入行。
    /// 返回新的浮层占用行数。
    #[allow(clippy::too_many_arguments)]
    fn update_completion(
        &self,
        stdout: &mut impl Write,
        layout: &Layout,
        prompt: &str,
        buffer: &str,
        cursor: usize,
        active: &mut bool,
        index: &mut usize,
        items: &mut Vec<CompletionItem>,
        overlay_lines: u16,
        engine: &CompletionEngine,
        mention: &mut MentionCompletion,
    ) -> io::Result<u16> {
        let mut lines = self.clear_overlay(stdout, layout, overlay_lines)?;

        // 斜杠命令补全优先(输入以 '/' 开头且有后续字符);
        // 否则检测 @ 提及 token(D1,文件路径实时补全)
        let trimmed = buffer.trim_start();
        let new_items = if trimmed.starts_with('/') && trimmed.len() >= 2 {
            mention.deactivate();
            engine.complete(trimmed)
        } else {
            mention.suggest(buffer, cursor)
        };

        if new_items.is_empty() {
            *active = false;
            items.clear();
        } else {
            *items = new_items;
            *index = 0;
            *active = true;
            lines = self.draw_overlay(stdout, layout, *index, items)?;
        }

        self.redraw_line(stdout, layout, prompt, buffer, cursor)?;
        Ok(lines)
    }
}

impl Default for InputHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_handler_creation() {
        let _handler = InputHandler::new();
    }

    // ========== UTF-8 光标修复(第 07 轮,修复中文输入 panic) ==========

    #[test]
    fn prev_char_boundary_walks_back_multibyte() {
        let s = "取消";
        // "取" 3 字节,"消" 3 字节;边界 {0, 3, 6}
        assert_eq!(prev_char_boundary(s, 6), 3);
        assert_eq!(prev_char_boundary(s, 3), 0);
        assert_eq!(prev_char_boundary(s, 0), 0);
    }

    #[test]
    fn prev_char_boundary_mixed_ascii_cjk() {
        let s = "a中b"; // 边界 {0, 1, 4, 5}
        assert_eq!(prev_char_boundary(s, 4), 1);
        assert_eq!(prev_char_boundary(s, 5), 4);
        assert_eq!(prev_char_boundary(s, 1), 0);
    }

    #[test]
    fn display_width_cjk_is_two_columns() {
        assert_eq!(display_width("ab"), 2);
        assert_eq!(display_width("取消"), 4);
        assert_eq!(display_width("a取b"), 4);
        assert_eq!(display_width(""), 0);
    }

    /// 回归钉子:模拟输入循环的「字节偏移 cursor + insert/remove」核心操作,
    /// 中文连续插入 + 退格不再触发 is_char_boundary panic。
    #[test]
    fn utf8_cursor_insert_and_backspace_never_panics() {
        let mut buffer = String::new();
        let mut cursor: usize = 0;
        for c in "长任务取消测试".chars() {
            buffer.insert(cursor, c);
            cursor += c.len_utf8();
        }
        assert_eq!(buffer, "长任务取消测试");
        assert_eq!(cursor, buffer.len());
        // 连续退格到空
        while cursor > 0 {
            cursor = prev_char_boundary(&buffer, cursor);
            let removed = buffer.remove(cursor);
            assert!(!removed.is_ascii()); // 全程删的都是 CJK 字符
        }
        assert!(buffer.is_empty());
        assert_eq!(cursor, 0);
    }

    // ========== 固定底部输入组件(2026-09-09) ==========

    #[test]
    fn layout_full_panel_on_normal_terminal() {
        let l = Layout::compute(100, 30);
        assert_eq!(l.area_h, 3);
        assert_eq!(l.panel_top, 27); // 分隔线
        assert_eq!(l.input_y, 28); // 输入行
        assert_eq!(l.hint_y, Some(29)); // 提示行
        assert_eq!(l.scroll_bottom(), 27); // 1-indexed 滚动区底行
        assert_eq!(l.scroll_last_row(), 26); // 0-indexed 输出锚点
    }

    #[test]
    fn layout_degrades_to_single_line_on_tiny_terminal() {
        let l = Layout::compute(80, 5);
        assert_eq!(l.area_h, 1);
        assert_eq!(l.panel_top, 4);
        assert_eq!(l.input_y, 4);
        assert_eq!(l.hint_y, None);
        assert_eq!(l.scroll_bottom(), 4);
    }

    #[test]
    fn layout_zero_size_terminal_never_underflows() {
        // script 录屏等极端环境 terminal::size() 可能返回 (0,0)
        let l = Layout::compute(0, 0);
        assert_eq!(l.area_h, 1);
        assert_eq!(l.scroll_bottom(), 1); // 不允许下溢成 65535
        assert_eq!(l.scroll_last_row(), 0);
    }

    #[test]
    fn visible_window_short_input_fits() {
        assert_eq!(visible_window("hello", 5, 20), (0, 5));
        assert_eq!(visible_window("", 0, 20), (0, 0));
    }

    #[test]
    fn visible_window_scrolls_when_cursor_beyond_avail() {
        let s = "abcdefghijklmnopqrstuvwxyz"; // 26 列
        let (start, col) = visible_window(s, 26, 10);
        assert!(start > 0);
        assert_eq!(col, 9); // 光标钉在窗口最后一列
        assert!(display_width(&window_str(s, start, 10)) <= 10);
    }

    #[test]
    fn visible_window_cjk_counts_two_columns() {
        let s = "取消任务确认"; // 12 列
        let (start, col) = visible_window(s, s.len(), 8);
        assert!(start > 0);
        assert!(col < 8);
        assert!(s.is_char_boundary(start));
    }

    #[test]
    fn visible_window_cursor_in_middle_no_scroll_when_fits() {
        let s = "aaaaaaaaaaaaaaaaaaaa"; // 20 列
        let (start, col) = visible_window(s, 10, 40);
        assert_eq!(start, 0);
        assert_eq!(col, 10);
    }

    #[test]
    fn fit_width_truncates_by_display_columns() {
        assert_eq!(fit_width("hello", 3), "hel");
        assert_eq!(fit_width("取消任务", 5), "取消"); // "取消任" 6 列 > 5
        assert_eq!(fit_width("abc", 10), "abc");
        assert_eq!(fit_width("abc", 0), "");
    }

    // ========== D6 大粘贴防护(2026-09-10 第二十二轮,L1573+L1448) ==========

    #[test]
    fn paste_filter_strips_control_chars_keeps_newline() {
        assert_eq!(PasteRegistry::filter("a\x07b\x01c\nd"), "abc\nd");
        assert_eq!(PasteRegistry::filter("\x1b[31m红\x1b[0m"), "[31m红[0m");
        assert_eq!(PasteRegistry::filter("正常文本"), "正常文本");
    }

    #[test]
    fn paste_filter_normalizes_cr_and_crlf_to_lf() {
        // tmux bracketed paste 把 LF 转为 CR 按键语义回放(第 23 轮探针取证 LF=0/CR=N):
        // CR 必须归一为 LF,否则行数判定恒 1、marker 永不触发,内容拼成一行。
        assert_eq!(PasteRegistry::filter("a\r\nb\rc"), "a\nb\nc");
        // 纯 CR 多行 → 大粘贴行数判定依据
        let tmux_paste: String = (1..=15).map(|i| format!("第{i}行\r")).collect();
        assert_eq!(PasteRegistry::filter(&tmux_paste).matches('\n').count(), 15);
    }

    #[test]
    fn large_paste_with_tmux_cr_newlines_gets_marker() {
        // 复现本轮 Bug:15 行粘贴以 CR 分隔(tmux 实际形态)必须触发 marker
        let mut reg = PasteRegistry::new();
        let tmux_text: String = (1..=15)
            .map(|i| format!("第{i}行:大粘贴防护测试内容-{i}"))
            .collect::<Vec<_>>()
            .join("\r");
        match handle_paste_text(&tmux_text, &mut reg) {
            PasteInsert::Marker(m) => assert_eq!(m, "[粘贴 #1 +15 行]"),
            PasteInsert::Inline(_) => panic!("tmux CR 形态 15 行粘贴应转 marker"),
        }
        // 展开注入的原文应含 LF 归一化后的完整行
        let expanded = reg.expand("[粘贴 #1 +15 行]");
        assert_eq!(expanded.matches('\n').count(), 14);
    }

    #[test]
    fn small_paste_inline_newline_tab_become_space() {
        let mut reg = PasteRegistry::new();
        match handle_paste_text("第一行\n第二列\tend", &mut reg) {
            PasteInsert::Inline(s) => assert_eq!(s, "第一行 第二列 end"),
            PasteInsert::Marker(_) => panic!("小粘贴不应转 marker"),
        }
        assert!(reg.entries.is_empty());
    }

    #[test]
    fn large_paste_by_lines_gets_marker() {
        let mut reg = PasteRegistry::new();
        let text = (1..=11).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        match handle_paste_text(&text, &mut reg) {
            PasteInsert::Marker(m) => assert_eq!(m, "[粘贴 #1 +11 行]"),
            PasteInsert::Inline(_) => panic!("11 行应转 marker"),
        }
        assert_eq!(reg.entries.len(), 1);
    }

    #[test]
    fn large_paste_by_chars_gets_marker() {
        let mut reg = PasteRegistry::new();
        let text = "a".repeat(1001); // 单行超 1000 字符
        match handle_paste_text(&text, &mut reg) {
            PasteInsert::Marker(m) => assert_eq!(m, "[粘贴 #1 1001 字符]"),
            PasteInsert::Inline(_) => panic!("1001 字符应转 marker"),
        }
    }

    #[test]
    fn paste_counter_increments() {
        let mut reg = PasteRegistry::new();
        let text = "a".repeat(2000);
        let m1 = match handle_paste_text(&text, &mut reg) {
            PasteInsert::Marker(m) => m,
            _ => panic!(),
        };
        let m2 = match handle_paste_text(&text, &mut reg) {
            PasteInsert::Marker(m) => m,
            _ => panic!(),
        };
        assert!(m1.contains("#1"));
        assert!(m2.contains("#2"));
    }

    // ========== Tab/Enter + 补全菜单决策(2026-09-10 第 24 轮 Enter 吞键修复) ==========

    fn items(items: Vec<&str>) -> Vec<CompletionItem> {
        items
            .into_iter()
            .map(|r| CompletionItem {
                display: r.to_string(),
                replacement: r.to_string(),
                description: String::new(),
                usage: String::new(),
            })
            .collect()
    }

    #[test]
    fn completion_enter_when_buffer_equals_replacement_submits_buffer() {
        // 路径 A:buffer 已是完整命令 → 直接以 buffer 提交(避免补全后再加尾随空格)
        let items = items(vec!["/provider list"]);
        let d = completion_enter_tab_decision(KeyCode::Enter, "/provider list", true, &items, 0);
        assert_eq!(d, CompletionDecision::Submit("/provider list".to_string()));
    }

    #[test]
    fn completion_enter_on_prefix_submits_replacement() {
        // 路径 B:`/provider` + Enter → 用 `/provider list` 提交(第 24 轮 Bug 修复)
        // 避免高频命令需要按 2 次 Enter 才能进入子屏。
        let items = items(vec!["/provider list", "/provider add"]);
        let d = completion_enter_tab_decision(KeyCode::Enter, "/provider", true, &items, 0);
        assert_eq!(d, CompletionDecision::Submit("/provider list".to_string()));
    }

    #[test]
    fn completion_enter_on_partial_prefix_submits_replacement() {
        // `/provider a` 命中 `/provider add` 的真前缀 → 一次 Enter 补全并提交
        let items = items(vec!["/provider list", "/provider add"]);
        let d = completion_enter_tab_decision(KeyCode::Enter, "/provider a", true, &items, 1);
        assert_eq!(d, CompletionDecision::Submit("/provider add".to_string()));
    }

    #[test]
    fn completion_tab_on_prefix_accepts_without_submit() {
        // Tab + buffer 是补全项真前缀 → AcceptOnly(只补全,不提交,符合 Tab 传统语义)
        let items = items(vec!["/provider list"]);
        let d = completion_enter_tab_decision(KeyCode::Tab, "/provider", true, &items, 0);
        assert_eq!(d, CompletionDecision::AcceptOnly("/provider list".to_string()));
    }

    #[test]
    fn completion_enter_with_no_completion_submits_buffer() {
        // 补全未开:Enter 提交 buffer(原行为)
        let d = completion_enter_tab_decision(KeyCode::Enter, "hello world", false, &[], 0);
        assert_eq!(d, CompletionDecision::Submit("hello world".to_string()));
    }

    #[test]
    fn completion_tab_with_no_completion_is_noop() {
        // 补全未开:Tab 无操作(避免意外副作用)
        let d = completion_enter_tab_decision(KeyCode::Tab, "hello", false, &[], 0);
        assert_eq!(d, CompletionDecision::None);
    }

    #[test]
    fn completion_enter_on_non_prefix_buffer_submits_buffer_as_fallback() {
        // 兜底:补全已开 + Enter + buffer 不是任何补全项真前缀 → 提交 buffer(不让 Enter 被吞)
        let items = items(vec!["/provider list", "/help"]);
        let d = completion_enter_tab_decision(KeyCode::Enter, "/xyz custom", true, &items, 0);
        assert_eq!(d, CompletionDecision::Submit("/xyz custom".to_string()));
    }

    #[test]
    fn completion_enter_with_buffer_having_trailing_space_matches_replacement() {
        // 路径 A 容错:buffer 末尾有空格时,trim 后与 replacement 相等 → 提交 buffer
        let items = items(vec!["/provider list"]);
        let d = completion_enter_tab_decision(KeyCode::Enter, "/provider list  ", true, &items, 0);
        assert_eq!(d, CompletionDecision::Submit("/provider list  ".to_string()));
    }

    #[test]
    fn expand_restores_marker_to_original() {
        let mut reg = PasteRegistry::new();
        let text = "x".repeat(2000);
        let marker = reg.register(text.clone());
        let buf = format!("帮我分析 {marker} 谢谢");
        let expanded = reg.expand(&buf);
        assert_eq!(expanded, format!("帮我分析 {text} 谢谢"));
    }

    #[test]
    fn expand_truncates_oversized_paste() {
        let mut reg = PasteRegistry::new();
        let text = "0123456789".repeat(1200); // 12000 字符 > 10000 阈值
        let marker = reg.register(text.clone());
        let expanded = reg.expand(&format!("数据:{marker}"));
        assert!(expanded.contains("[...中间省略 11000 字符...]"));
        assert!(expanded.starts_with("数据:0123456789"));
        // 尾 500 字符保留
        assert!(expanded.ends_with("0123456789"));
        // 总长 ≈ 前缀 + 500 + 标注 + 500,远小于原文
        assert!(expanded.chars().count() < 1200);
    }

    #[test]
    fn expand_keeps_damaged_or_unknown_marker_as_is() {
        let mut reg = PasteRegistry::new();
        let marker = reg.register("y".repeat(2000));
        // 用户删了 marker 尾括号 → 不匹配,原样保留
        let damaged = marker.trim_end_matches(']').to_string();
        let buf = format!("看 {damaged}");
        assert_eq!(reg.expand(&buf), buf);
        // 未知编号 → 原样保留
        let buf2 = "看 [粘贴 #99 +5 行]".to_string();
        assert_eq!(reg.expand(&buf2), buf2);
    }

    #[test]
    fn expand_marker_removed_by_user_no_residue() {
        let mut reg = PasteRegistry::new();
        let _marker = reg.register("z".repeat(2000));
        // 用户把 marker 整段删了再提交 → 展开为空串,registry 内容不泄漏
        assert_eq!(reg.expand(""), "");
    }

    #[test]
    fn head_tail_chars_char_boundary_safe() {
        let s = "中文测试abcdef";
        assert_eq!(head_chars(s, 2), "中文");
        assert_eq!(tail_chars(s, 3), "def");
        assert_eq!(head_chars(s, 100), s);
        assert_eq!(tail_chars(s, 100), s);
        assert_eq!(head_chars("", 5), "");
    }

    #[test]
    fn truncate_for_inject_below_threshold_passthrough() {
        let s = "短文本";
        assert_eq!(PasteRegistry::truncate_for_inject(s), s);
        let exact = "a".repeat(PASTE_TRUNCATE_CHARS);
        assert_eq!(PasteRegistry::truncate_for_inject(&exact), exact);
    }
}
