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
use std::io::{self, Write};
use crossterm::{
    cursor::MoveTo,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    style::{Attribute, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use crate::tui::completion::{CompletionEngine, CompletionItem};
use crate::tui::theme;

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
fn char_width(c: char) -> u16 {
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
fn display_width(s: &str) -> u16 {
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
        Self { cols, rows, area_h, panel_top, input_y, hint_y }
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
        // 进入原始模式
        terminal::enable_raw_mode()?;

        let result = self.read_line_inner(prompt, engine);

        // 退出原始模式（无论成功失败）
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

        let stdout = io::stdout();
        let mut stdout = stdout.lock();

        // 设置滚动区 + 绘制固定面板 + 空输入行
        self.enter_pinned(&mut stdout, &layout)?;
        self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;

        loop {
            // 读取事件
            let ev = match event::read() {
                Ok(e) => e,
                Err(_) => break,
            };

            match ev {
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
                        KeyCode::Esc => {
                            // Esc: 关闭补全浮层
                            if completion_active {
                                overlay_lines = self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                completion_active = false;
                                completion_items.clear();
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Up => {
                            // 上箭头：在补全列表中向上移动
                            if completion_active && !completion_items.is_empty() {
                                completion_index = if completion_index == 0 {
                                    completion_items.len() - 1
                                } else {
                                    completion_index - 1
                                };
                                overlay_lines = self.draw_overlay(&mut stdout, &layout, completion_index, &completion_items)?;
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Down => {
                            // 下箭头：在补全列表中向下移动
                            if completion_active && !completion_items.is_empty() {
                                completion_index = (completion_index + 1) % completion_items.len();
                                overlay_lines = self.draw_overlay(&mut stdout, &layout, completion_index, &completion_items)?;
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Tab | KeyCode::Enter => {
                            // Tab 或 Enter：接受当前选中项或提交输入
                            if completion_active && !completion_items.is_empty() {
                                let item = &completion_items[completion_index];
                                // 若缓冲区已等于补全目标(忽略尾随空格),直接提交而非接受补全。
                                // 避免用户已完整输入命令名时 Enter 被吞成"接受补全 + 加尾随空格"。
                                if buffer.trim() == item.replacement.trim() {
                                    return self.submit(&mut stdout, &layout, prompt, buffer, overlay_lines);
                                }
                                // 接受补全并关闭浮层
                                buffer = item.replacement.clone();
                                cursor = buffer.len();
                                completion_active = false;
                                completion_items.clear();
                                overlay_lines = self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            } else if key.code == KeyCode::Enter {
                                // 提交输入
                                return self.submit(&mut stdout, &layout, prompt, buffer, overlay_lines);
                            }
                        }
                        KeyCode::Backspace => {
                            // 退格：删除光标前字符(cursor 为字节偏移,先回退到前一个
                            // 字符的起始边界再删,保证多字节 CJK 不触发 is_char_boundary panic)
                            if cursor > 0 {
                                cursor = prev_char_boundary(&buffer, cursor);
                                buffer.remove(cursor);
                                overlay_lines = self.update_completion(
                                    &mut stdout, &layout, prompt, &buffer, cursor,
                                    &mut completion_active, &mut completion_index,
                                    &mut completion_items, overlay_lines, engine,
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
                                    &mut stdout, &layout, prompt, &buffer, cursor,
                                    &mut completion_active, &mut completion_index,
                                    &mut completion_items, overlay_lines, engine,
                                )?;
                            }
                        }
                        KeyCode::Delete => {
                            // Delete：删除光标处字符(cursor 在字符边界,remove 按字节偏移删一个字符)
                            if cursor < buffer.len() {
                                buffer.remove(cursor);
                                overlay_lines = self.update_completion(
                                    &mut stdout, &layout, prompt, &buffer, cursor,
                                    &mut completion_active, &mut completion_index,
                                    &mut completion_items, overlay_lines, engine,
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
                                cursor += buffer[cursor..].chars().next().map_or(0, |c| c.len_utf8());
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
                            overlay_lines = self.update_completion(
                                &mut stdout, &layout, prompt, &buffer, cursor,
                                &mut completion_active, &mut completion_index,
                                &mut completion_items, overlay_lines, engine,
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
                        self.draw_overlay(&mut stdout, &layout, completion_index, &completion_items)?
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
        execute!(stdout, ResetColor, Print(format!("\x1b[1;{}r", layout.scroll_bottom())))?;
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
                    "  ↑↓ 选择补全  Enter 提交  Esc 关闭补全  Ctrl-D 退出  /help 帮助",
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
    fn submit(
        &self,
        stdout: &mut impl Write,
        layout: &Layout,
        prompt: &str,
        buffer: String,
        overlay_lines: u16,
    ) -> io::Result<InputResult> {
        self.clear_overlay(stdout, layout, overlay_lines)?;
        // 回显到滚动区底行(保留用户输入痕迹,随历史输出一起滚动)
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
        Ok(InputResult::Submitted(buffer))
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
                Print(fit_width("  ↑↓ 选择  Enter/Tab 接受  Esc 关闭", layout.cols)),
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
            execute!(stdout, MoveTo(0, y), ResetColor, Clear(ClearType::CurrentLine))?;
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
    fn clear_overlay(&self, stdout: &mut impl Write, layout: &Layout, lines: u16) -> io::Result<u16> {
        if lines > 0 {
            let start = layout.panel_top.saturating_sub(lines);
            for y in start..layout.panel_top {
                execute!(stdout, MoveTo(0, y), ResetColor, Clear(ClearType::CurrentLine))?;
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
    ) -> io::Result<u16> {
        let mut lines = self.clear_overlay(stdout, layout, overlay_lines)?;

        // 仅当输入以 '/' 开头且有后续字符时激活补全
        let trimmed = buffer.trim_start();
        let new_items = if trimmed.starts_with('/') && trimmed.len() >= 2 {
            engine.complete(trimmed)
        } else {
            Vec::new()
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
}
