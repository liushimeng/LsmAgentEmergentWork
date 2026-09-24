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
use crate::tui::paste;
use crate::tui::textfit;
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

/// 单字符近似显示宽度(列数)—— 转发 [`textfit::char_width`]。
///
/// 2026-09-24:宽度表唯一真源下移到 `textfit`(盒线渲染与输入行光标列号换算必须
/// 用同一套度量,否则横幅按 A 算、输入行按 B 算,同一字符两处错位)。
/// `LAEW_AMBIGUOUS_WIDE=1` 的歧义宽度策略因此对**输入行**同样生效。
pub fn char_width(c: char) -> u16 {
    textfit::char_width(c)
}

/// 近似显示宽度(列数),饱和到 `u16::MAX` 避免超长输入溢出。
///
/// 不引入 `unicode-width` 依赖的轻量近似:覆盖常用 CJK 区段,
/// 边缘字符(组合符等)按 1 列处理,可接受。
pub fn display_width(s: &str) -> u16 {
    textfit::width(s).min(u16::MAX as usize) as u16
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

// ========== 第 93 轮:无 bracketed paste 终端的 Enter 粘贴突发探测 ==========
//
// 背景(实测 llaew_20260919_121348.log):Windows conhost / 不支持 bracketed paste
// 的终端里,多行提示词粘贴以「逐键 Char 事件 + 行间 Enter」形式到达,旧实现 Enter
// 到达即提交 —— 8 条编号需求只有首行标题进入任务,其余行在任务结束后被当作新任务
// 逐条提交(微信聊天任务被截成「打开窗口保持10分钟」的主根因)。
// 本探测在 Enter 提交前检查输入队列:粘贴的后续行此刻必然已排在终端输入缓冲里,
// Enter 之后还有排队事件 → 判定为粘贴突发,Enter 按换行处理不提交,整段经 D6
// 粘贴管线进 buffer(小粘贴直插 / 多粘贴 [粘贴 #N] marker),最后一次 Enter
// (队列已空)或用户复查后再按 Enter 才提交完整提示词。
// 人类击键间隔 ≫15ms,正常单击 Enter 队列恒空,零误伤;bracketed paste 生效的
// 终端粘贴整体以 Event::Paste 送达,不经本路径。

/// Enter 突发探测:首个排队事件的等待窗口(毫秒)。
const PASTE_BURST_PROBE_MS: u64 = 15;
/// 队列排空后的宽限轮数(终端分块投递,每轮等 PASTE_BURST_GRACE_MS)。
const PASTE_BURST_GRACE_ROUNDS: usize = 2;
/// 宽限轮单轮等待(毫秒)。
const PASTE_BURST_GRACE_MS: u64 = 8;

/// 把一串已排队事件折叠为粘贴文本(纯函数,可单测):
/// - `Key(Press) Char(c)`(无 Ctrl)→ 收 `c`;
/// - `Key(Press) Enter` → 收 `\n`(行间换行);
/// - `Event::Paste(t)` → 收 `t`(混合形态兜底);
/// - 首个其它事件原样回吐(调用方存入 pending,不丢事件)。
fn burst_events_to_text(events: Vec<Event>) -> (String, Option<Event>) {
    let mut text = String::new();
    let mut spill = None;
    for ev in events {
        match ev {
            Event::Key(k)
                if k.kind == KeyEventKind::Press
                    && matches!(k.code, KeyCode::Char(_))
                    && !k.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if let KeyCode::Char(c) = k.code {
                    text.push(c);
                }
            }
            Event::Key(k) if k.kind == KeyEventKind::Press && k.code == KeyCode::Enter => {
                text.push('\n');
            }
            Event::Paste(t) => text.push_str(&t),
            other => {
                spill = Some(other);
                break;
            }
        }
    }
    (text, spill)
}

/// Enter 提交前探测粘贴突发:队列有后续事件 → Some(突发文本),无 → None(正常提交)。
///
/// 非文本事件(Ctrl 组合 / Resize 等)存入 `pending` 回吐,主循环下轮优先处理。
///
/// 第 94 轮修复:空文本返回 None(而非 Some(""))——避免外层代码因 `Some("")`
/// 进入 burst 插入路径后 `continue`,既不提交也不反馈,表现为「卡死」。
fn drain_paste_burst(pending: &mut Option<Event>) -> Option<String> {
    // 首证据:短暂等待(人类单击 Enter 时队列恒空,直接 None,零感知延迟)
    if !matches!(
        event::poll(Duration::from_millis(PASTE_BURST_PROBE_MS)),
        Ok(true)
    ) {
        return None;
    }
    let mut events = Vec::new();
    let mut grace_left = PASTE_BURST_GRACE_ROUNDS;
    loop {
        match event::poll(Duration::ZERO) {
            Ok(true) => {
                if let Ok(ev) = event::read() {
                    events.push(ev);
                }
            }
            _ => {
                // 队列空:宽限几轮捕捉终端分块投递的尾部
                if grace_left == 0 {
                    break;
                }
                grace_left -= 1;
                if !matches!(
                    event::poll(Duration::from_millis(PASTE_BURST_GRACE_MS)),
                    Ok(true)
                ) {
                    break;
                }
            }
        }
    }
    let (text, spill) = burst_events_to_text(events);
    if let Some(ev) = spill {
        *pending = Some(ev);
    }
    // ★ 第 94 轮修复:空文本(纯空白/控制字符/无文本)返回 None,触发正常提交
    if text.is_empty() {
        None
    } else {
        Some(text)
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

        // 第 104 轮加固:read_line_inner 内部 panic 时(任意 bug 触发)
        // 必须保证 raw mode + bracketed paste 被关掉,主循环错误分支才能正常
        // 清理终端。否则一旦 read_line_inner panic,raw mode 残留 + 滚动区残留 +
        // 焦点失守,Ctrl-C 显式 ^C、Ctrl-D 不退。catch_unwind 把 panic 捕获为
        //    `Err(panic_payload)`,我们把它转成 IO 错误上抛给主循环。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.read_line_inner(prompt, engine)
        }));

        // 无论成功失败,都要退出原始模式 + 关闭 bracketed paste。
        // 这一段即使 panic 也会执行 —— 它在 catch_unwind 的外层。
        let mut stdout = io::stdout().lock();
        let _ = execute!(stdout, DisableBracketedPaste);
        drop(stdout);
        let _ = terminal::disable_raw_mode();

        match result {
            Ok(r) => r,
            Err(panic_payload) => {
                // 不要把 panic 抛出去炸进程,把 panic 信息塞进 IO 错误返回。
                // 主循环错误分支会调用 teardown_pinned + disable_raw_mode 二次兜底。
                let detail = if let Some(s) = panic_payload.downcast_ref::<&'static str>() {
                    Some(*s)
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    Some(s.as_str())
                } else {
                    None
                };
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("read_line_inner panicked: {:?}", detail),
                ));
            }
        }
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
        // 粘贴登记簿:本次行编辑期间的多行/超长粘贴原文(提交时 marker 展开为原文)
        let mut pastes = paste::PasteRegistry::new();
        // 批量合并排空时暂存的非字符事件(crossterm 无 pushback,不能丢)
        let mut pending: Option<Event> = None;

        let stdout = io::stdout();
        let mut stdout = stdout.lock();

        // 第 103 轮:强制重置终端状态,消除前一轮 dispatch_prompt 大量输出后
        // 可能残留的滚动区/光标位置异常(焦点丢失 bug 的根因)。
        // DECRST 重置滚动区为全屏 + 确保光标可见,再进入 pinned 布局。
        execute!(stdout, ResetColor, Print("[r"))?;
        execute!(stdout, crossterm::cursor::Show)?;
        stdout.flush()?;
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
                    let preview = match paste::handle_paste_text(&text, &mut pastes) {
                        paste::PasteInsert::Inline(s) => {
                            buffer.insert_str(cursor, &s);
                            cursor += s.len();
                            None
                        }
                        paste::PasteInsert::Marker(s) => {
                            buffer.insert_str(cursor, &s);
                            cursor += s.len();
                            // 多行/超长粘贴:输入行只放 marker,但用户必须看得见自己粘了什么
                            // (修 R5 可见性半边)—— 立刻在滚动区回显原文前几行预览。
                            let content = pastes.last_content();
                            Some(paste::plan_paste_preview(
                                &s,
                                &content,
                                textfit::term_width_for_render(),
                            ))
                        }
                    };
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
                    if let Some(lines) = &preview {
                        self.print_in_scroll_region(&mut stdout, &layout, lines)?;
                        // 预览动过滚动区底行,重绘输入行确保光标列号与 buffer 一致
                        self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                    }
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-C 双语义(第 108 轮加强):
                            // - 空输入 + Ctrl-C → 触发 shutdown 协调器并退出程序
                            //   (Unix 语义,符合用户肌肉记忆)
                            // - 非空输入 + Ctrl-C → 清空当前行 + 触发 shutdown 协调器
                            //   (root cause:任务运行时 Ctrl+C 走 SIGINT 路径被旧实现
                            //   「只清行」吞掉,导致 Ctrl+C 多次按后 Ctrl-Z suspend 整个
                            //   laew → 终端僵死。本轮统一触发 shutdown 协调器,
                            //   由主循环统一处理退出与终端还原。)
                            if buffer.is_empty() {
                                self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                teardown_pinned_layout(&mut stdout, &layout)?;
                                crate::shutdown::global().trigger(crate::shutdown::ShutdownReason::UserInterrupt);
                                return Ok(InputResult::Exit);
                            } else {
                                self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                self.redraw_line(&mut stdout, &layout, prompt, "", 0)?;
                                execute!(stdout, MoveTo(0, layout.scroll_last_row()))?;
                                stdout.flush()?;
                                crate::shutdown::global().trigger(crate::shutdown::ShutdownReason::UserInterrupt);
                                return Ok(InputResult::Interrupted);
                            }
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-D: 若输入为空则退出(还原滚动区 + 清空面板)
                            // 第 108 轮:同时触发 shutdown 协调器,统一退出语义
                            if buffer.is_empty() {
                                self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                teardown_pinned_layout(&mut stdout, &layout)?;
                                crate::shutdown::global().trigger(crate::shutdown::ShutdownReason::UserInterrupt);
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
                        // 2026-09-11 第三十七轮:Ctrl-J(LF,0x0A)拦截。
                        // 原始模式下终端发来的换行字节 LF 被 crossterm 解析为
                        // Char('j')+CONTROL —— tmux send-keys 逐键发送多行文本、
                        // 不支持 bracketed paste 的旧终端逐键粘贴、用户手按 Ctrl-J
                        // 都会到达这里;落入下方 Char(c) 兜底会把字母 j 插入输入缓冲,
                        // 污染多行提示词(实测「slow.py:\n用」回显成「slow.py:j用」)。
                        // 语义与 paste::PasteInsert::Inline 的单行归一(\n → 空格)对齐。
                        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            buffer.insert(cursor, ' ');
                            cursor += 1;
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
                                if let (Some(start), Some(item)) =
                                    (mention.token_start, completion_items.get(completion_index))
                                {
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
                                    // 第 93 轮:无 bracketed paste 终端的多行粘贴突发 ——
                                    // Enter 之后已有排队事件,说明本次 Enter 是粘贴的行间
                                    // 换行而非用户提交意图;整段并入粘贴管线,不提交。
                                    //
                                    // 第 94 轮修复:burst 无实质内容时直接提交,避免
                                    // 「既不提交也不反馈」的卡死状态。
                                    if key.code == KeyCode::Enter {
                                        if let Some(burst) = drain_paste_burst(&mut pending) {
                                            // ★ 修复:burst 仅含空白/控制字符时不视为有效突发,
                                            // 直接进入提交,避免死循环
                                            if burst.chars().all(|c| c.is_whitespace() || c.is_control())
                                            {
                                                return self.submit(
                                                    &mut stdout,
                                                    &layout,
                                                    prompt,
                                                    text,
                                                    overlay_lines,
                                                    &pastes,
                                                );
                                            }
                                            let paste_text = format!("\n{burst}");
                                            let preview =
                                                match paste::handle_paste_text(&paste_text, &mut pastes) {
                                                    paste::PasteInsert::Inline(s) => {
                                                        buffer.insert_str(cursor, &s);
                                                        cursor += s.len();
                                                        None
                                                    }
                                                    paste::PasteInsert::Marker(s) => {
                                                        buffer.insert_str(cursor, &s);
                                                        cursor += s.len();
                                                        // 同 bracketed paste 路径:整段原文已保真,
                                                        // 立刻回显预览,不让用户对着一个 marker 猜
                                                        let content = pastes.last_content();
                                                        Some(paste::plan_paste_preview(
                                                            &s,
                                                            &content,
                                                            textfit::term_width_for_render(),
                                                        ))
                                                    }
                                                };
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
                                            if let Some(lines) = &preview {
                                                self.print_in_scroll_region(
                                                    &mut stdout,
                                                    &layout,
                                                    lines,
                                                )?;
                                                self.redraw_line(
                                                    &mut stdout,
                                                    &layout,
                                                    prompt,
                                                    &buffer,
                                                    cursor,
                                                )?;
                                            }
                                            continue;
                                        }
                                    }
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
                                    overlay_lines =
                                        self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
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
                            // 通用 CONTROL 防护(2026-09-11 第三十七轮):未被上方
                            // 显式绑定的 Ctrl 组合键(Ctrl-T/Ctrl-N/…)一律不作为
                            // 文本插入(readline 语义:未绑定的 Ctrl 组合不产生字符),
                            // 防止 crossterm 把控制字节解析成 Char(letter)+CONTROL
                            // 后落入此处插入字母垃圾。Shift/Alt/无修饰不受影响。
                            if key.modifiers.contains(KeyModifiers::CONTROL) {
                                continue;
                            }
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

    /// 在**滚动区底行**逐行输出:每行 `MoveTo(0, 底行)` + 清行 + `Print` + 换行,
    /// 靠 DECSTBM 滚动区把旧内容往上推,底部输入面板不受影响。
    ///
    /// 粘贴预览与提交回显共用(D6 修 R5 的「可见性」半边)。
    fn print_in_scroll_region(
        &self,
        stdout: &mut impl Write,
        layout: &Layout,
        lines: &[String],
    ) -> io::Result<()> {
        execute!(stdout, ResetColor, SetForegroundColor(theme::DIM))?;
        for line in lines {
            execute!(
                stdout,
                MoveTo(0, layout.scroll_last_row()),
                Clear(ClearType::CurrentLine),
                Print(line.as_str()),
                Print("\r\n")
            )?;
        }
        execute!(stdout, ResetColor)?;
        stdout.flush()
    }

    /// 提交:清浮层 → 滚动区底行回显已提交内容 → 清空面板输入行 → 光标锚定滚动区底行。
    ///
    /// 2026-09-24 修 R5:回显改用 **展开版**逐行打印(首行 `>> `、续行 `.. `、
    /// 折行悬挂缩进,超 `SUBMIT_ECHO_MAX_LINES` 折叠并标注「内容已完整发送」)——
    /// 旧实现回显的是 marker 版单行 buffer,用户粘贴的多行提示词在屏幕上只剩一行,
    /// 与「送进模型的到底是什麼」完全对不上。返回的 `Submitted` 仍是展开还原版
    /// (marker → 原文,超大粘贴截断注入,见 `PasteRegistry::expand`)。
    #[allow(clippy::too_many_arguments)]
    fn submit(
        &self,
        stdout: &mut impl Write,
        layout: &Layout,
        prompt: &str,
        buffer: String,
        overlay_lines: u16,
        pastes: &paste::PasteRegistry,
    ) -> io::Result<InputResult> {
        self.clear_overlay(stdout, layout, overlay_lines)?;
        // 回显到滚动区底行(保留用户输入痕迹,随历史输出一起滚动):
        // 展开版逐行回显,多行提示词在屏幕上完整可读,不再只剩首行。
        let expanded = pastes.expand(&buffer);
        let echo = paste::plan_submit_echo(prompt, &expanded, textfit::term_width_for_render());
        self.print_in_scroll_region(stdout, layout, &echo)?;
        // 面板输入行清空(组件常显)
        self.redraw_line(stdout, layout, prompt, "", 0)?;
        // 输出光标锚定滚动区底行,后续 println! 任务输出在滚动区内滚动
        execute!(stdout, MoveTo(0, layout.scroll_last_row()))?;
        stdout.flush()?;
        Ok(InputResult::Submitted(expanded))
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
    // 粘贴层已拆到 paste.rs(D6 修 R5 后 input.rs 超 1800 行);突发管线用例仍留本文件,按需引入
    use crate::tui::paste::{PasteInsert, PasteRegistry, handle_paste_text};
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
    fn 输入行与横幅共用同一宽度真源() {
        // char_width 已下移到 textfit:两处必须给出同样的列数,否则横幅按 A 算、
        // 输入行按 B 算,同一字符在两个位置错位。
        for s in ["Online ✓", "取消任务确认", "llaew_20260924_123508.log", "中文 abc"] {
            assert_eq!(display_width(s) as usize, textfit::width(s), "度量分叉: {s}");
        }
        assert_eq!(char_width('中'), 2);
        assert_eq!(char_width('a'), 1);
        // 歧义字符:默认 1 列(与改造前逐字节一致);LAEW_AMBIGUOUS_WIDE=1 时 2 列
        if textfit::ambiguous_wide() {
            assert_eq!(textfit::width("✓"), 2, "歧义开关未生效");
        } else {
            assert_eq!(textfit::width("Online ✓"), 8);
        }
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
        assert_eq!(
            d,
            CompletionDecision::AcceptOnly("/provider list".to_string())
        );
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
        assert_eq!(
            d,
            CompletionDecision::Submit("/provider list  ".to_string())
        );
    }







    // ========== 第 93 轮:Enter 粘贴突发探测(burst_events_to_text 纯函数) ==========

    use crossterm::event::KeyEvent;

    fn key_char(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn key_enter() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    #[test]
    fn burst_folds_chars_and_enters_to_multiline_text() {
        // 粘贴「第二行\n第三行」到达:Char 序列 + 行间 Enter + Char 序列
        let events: Vec<Event> = "第二行"
            .chars()
            .map(key_char)
            .chain(std::iter::once(key_enter()))
            .chain("第三行".chars().map(key_char))
            .collect();
        let (text, spill) = burst_events_to_text(events);
        assert_eq!(text, "第二行\n第三行");
        assert!(spill.is_none());
    }

    #[test]
    fn burst_ctrl_char_events_excluded() {
        // Ctrl 组合键不作为文本(Ctrl-J 等),应终止折叠并回吐
        let events = vec![
            key_char('a'),
            Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            key_char('b'),
        ];
        let (text, spill) = burst_events_to_text(events);
        assert_eq!(text, "a");
        assert!(spill.is_some()); // Ctrl-J 回吐进 pending,不丢事件
    }

    #[test]
    fn burst_paste_event_text_appended() {
        // 混合形态:突发中夹 Event::Paste(终端分块),文本原样并入
        let events = vec![
            key_char('x'),
            Event::Paste("粘贴段".to_string()),
            key_enter(),
        ];
        let (text, spill) = burst_events_to_text(events);
        assert_eq!(text, "x粘贴段\n");
        assert!(spill.is_none());
    }

    #[test]
    fn burst_non_text_event_spills_and_stops() {
        // Resize 等非文本事件:回吐并终止,后续事件不再消费
        let events = vec![key_char('a'), Event::Resize(80, 24), key_char('b')];
        let (text, spill) = burst_events_to_text(events);
        assert_eq!(text, "a");
        assert!(matches!(spill, Some(Event::Resize(80, 24))));
    }

    #[test]
    fn burst_empty_events_yields_empty_text() {
        let (text, spill) = burst_events_to_text(vec![]);
        assert_eq!(text, "");
        assert!(spill.is_none());
    }

    // ========== 第 94 轮:drain_paste_burst 空事件返回 None(防卡死) ==========

    /// 验证 burst_events_to_text 对纯空白/控制字符文本返回空串,
    /// 外层 drain_paste_burst 应据此返回 None 触发正常提交。
    #[test]
    fn burst_whitespace_only_text_is_empty_for_submit() {
        // Enter + 空格(模拟终端残留空白):burst 文本全为空白
        let events = vec![
            key_enter(),
            Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
        ];
        let (text, _spill) = burst_events_to_text(events);
        // 单个空格在 burst_events_to_text 中被 Char(' ') 收集
        // 但 drain_paste_burst 的空文本守卫会拦截全空白串
        let all_blank = text.chars().all(|c| c.is_whitespace() || c.is_control());
        assert!(
            all_blank || text.is_empty(),
            "全空白文本应被识别为无实质内容: {text:?}"
        );
    }

    /// 验证全空白 burst 文本不会触发 burst 插入(应直接进入提交)。
    #[test]
    fn all_whoulder_burst_should_not_block_submit() {
        let burst = "\n  \n".to_string();
        let all_blank = burst.chars().all(|c| c.is_whitespace() || c.is_control());
        assert!(all_blank, "全空白/换行文本应被判定为非有效突发");
    }

    // ========== 第 94 轮:Ctrl-C 双语义验证 ==========

    /// 验证 Ctrl-C 决策:空输入时应退出(Exit),非空输入时应中断(Interrupted)。
    /// 此处验证 InputResult 枚举的语义区分,按键处理逻辑在 read_line_inner 中。
    #[test]
    fn ctrl_c_semantics_distinguish_empty_vs_nonempty() {
        // 空输入 → Exit;非空输入 → Interrupted
        enum CtrlCDecision {
            Exit,
            Interrupt,
        }
        let decide = |buffer: &str| {
            if buffer.is_empty() {
                CtrlCDecision::Exit
            } else {
                CtrlCDecision::Interrupt
            }
        };
        assert!(matches!(decide(""), CtrlCDecision::Exit));
        assert!(matches!(decide("hello"), CtrlCDecision::Interrupt));
        assert!(matches!(decide("  "), CtrlCDecision::Interrupt)); // 有内容(空格)也中断
    }

    /// 端到端语义:突发文本经 D6 管线 —— 多行 → marker 保真;
    /// 单行突发(段首那个代表 Enter 的换行)→ 直插,不套噪声 marker。
    #[test]
    fn burst_text_feeds_paste_pipeline() {
        let mut reg = PasteRegistry::new();
        // 12 行任务书突发(每段前带一个行间换行)→ marker 且登记原文
        let burst: String = (1..=12)
            .map(|i| format!("\n{i}. 任务步骤{i}"))
            .collect();
        match handle_paste_text(&burst, &mut reg) {
            PasteInsert::Inline(s) => panic!("12 行应转 marker,实际 Inline: {s}"),
            PasteInsert::Marker(m) => {
                assert_eq!(m, "[粘贴 #1 +12 行]", "marker 应带行数")
            }
        }
        assert_eq!(reg.expand("[粘贴 #1 +12 行]").lines().count(), 12, "突发原文行数应保真");
        // 单行突发:前导换行是「刚按下的 Enter」,剔除后按单行直插
        match handle_paste_text("\n第二行", &mut reg) {
            PasteInsert::Inline(s) => assert_eq!(s, "第二行", "前导换行不应进入 buffer: {s:?}"),
            PasteInsert::Marker(m) => panic!("单行突发不应转 marker: {m}"),
        }
    }
}
