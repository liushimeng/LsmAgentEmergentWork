//! 自定义输入处理器（基于 crossterm）。
//!
//! 提供原始终态下的行输入能力，并集成下拉式补全列表：
//! - 上下箭头导航补全候选项
//! - 未选中项显示灰色（ANSI 90m）
//! - Enter/Tab 接受选中项
//! - Esc 关闭补全列表

use std::io::{self, Write};

use crossterm::{
    cursor::{MoveToColumn, MoveUp},
    event::{self, Event, KeyCode, KeyModifiers, KeyEventKind},
    execute,
    style::{Attribute, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};

use crate::tui::completion::CompletionEngine;

/// ANSI 颜色常量。
mod colors {
    use crossterm::style::Color;
    pub const GRAY: Color = Color::DarkGrey;          // 未选中项 / 描述
    pub const HIGHLIGHT_FG: Color = Color::White;     // 选中项前景
    pub const DIM: Color = Color::Grey;               // 辅助文字
}

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

/// 近似显示宽度(列数):CJK / 全角 / 谚文按 2 列,其余按 1 列。
///
/// 不引入 `unicode-width` 依赖的轻量近似,仅用于光标列号换算;
/// 覆盖常用 CJK 区段,边缘字符(组合符等)按 1 列处理,可接受。
fn display_width(s: &str) -> u16 {
    s.chars()
        .map(|c| {
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
        })
        .sum()
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

/// 自定义输入处理器。
pub struct InputHandler;

impl InputHandler {
    pub fn new() -> Self {
        Self
    }

    /// 读取一行输入，集成下拉式补全。
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
        let mut buffer = String::new();
        let mut cursor: usize = 0;
        let mut completion_active = false;
        let mut completion_index: usize = 0;
        let mut completion_items = Vec::new();
        let prompt_width = prompt.len() as u16;

        // 输出初始提示符
        let stdout = io::stdout();
        let mut stdout = stdout.lock();
        execute!(stdout, Print(prompt), MoveToColumn(prompt_width))?;
        stdout.flush()?;

        loop {
            // 读取事件
            let event = match event::read() {
                Ok(e) => e,
                Err(_) => break,
            };

            match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-C: 中断当前输入
                            self.clear_current_line(&mut stdout, prompt_width, &buffer, completion_active, &completion_items)?;
                            return Ok(InputResult::Interrupted);
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-D: 若输入为空则退出
                            if buffer.is_empty() {
                                self.clear_current_line(&mut stdout, prompt_width, &buffer, completion_active, &completion_items)?;
                                return Ok(InputResult::Exit);
                            }
                        }
                        KeyCode::Esc => {
                            // Esc: 关闭补全列表
                            if completion_active {
                                self.clear_completion_list(&mut stdout, prompt_width, &buffer, completion_active, &completion_items)?;
                                completion_active = false;
                                completion_items.clear();
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
                                self.redraw_completion(&mut stdout, prompt_width, &buffer, completion_active, completion_index, &completion_items)?;
                            }
                        }
                        KeyCode::Down => {
                            // 下箭头：在补全列表中向下移动
                            if completion_active && !completion_items.is_empty() {
                                completion_index = (completion_index + 1) % completion_items.len();
                                self.redraw_completion(&mut stdout, prompt_width, &buffer, completion_active, completion_index, &completion_items)?;
                            }
                        }
                        KeyCode::Tab | KeyCode::Enter => {
                            // Tab 或 Enter：接受当前选中项或提交输入
                            if completion_active && !completion_items.is_empty() {
                                let item = &completion_items[completion_index];
                                // 若缓冲区已等于补全目标(忽略尾随空格),直接提交而非接受补全。
                                // 避免用户已完整输入命令名时 Enter 被吞成"接受补全 + 加尾随空格"。
                                if buffer.trim() == item.replacement.trim() {
                                    self.clear_completion_list(&mut stdout, prompt_width, &buffer, completion_active, &completion_items)?;
                                    execute!(stdout, Print("\r\n"))?;
                                    stdout.flush()?;
                                    return Ok(InputResult::Submitted(buffer));
                                }
                                // 接受补全
                                self.accept_completion(&mut stdout, &mut buffer, &mut cursor, prompt_width, &item.replacement)?;
                                // 接受后关闭补全列表
                                self.clear_completion_list(&mut stdout, prompt_width, &buffer, completion_active, &completion_items)?;
                                completion_active = false;
                                completion_items.clear();
                            } else if key.code == KeyCode::Enter {
                                // 提交输入
                                self.clear_completion_list(&mut stdout, prompt_width, &buffer, completion_active, &completion_items)?;
                                execute!(stdout, Print("\r\n"))?;
                                stdout.flush()?;
                                return Ok(InputResult::Submitted(buffer));
                            }
                        }
                        KeyCode::Backspace => {
                            // 退格：删除光标前字符(cursor 为字节偏移,先回退到前一个
                            // 字符的起始边界再删,保证多字节 CJK 不触发 is_char_boundary panic)
                            if cursor > 0 {
                                cursor = prev_char_boundary(&buffer, cursor);
                                buffer.remove(cursor);
                                self.redraw_line(&mut stdout, prompt, &buffer, cursor, prompt_width)?;
                                // 更新补全列表
                                self.update_completion(&mut stdout, &buffer, prompt_width, &mut completion_active, &mut completion_index, &mut completion_items, engine)?;
                            }
                        }
                        // 兜底：某些终端环境下 crossterm 未将退格映射为 KeyCode::Backspace，
                        // 而是作为 Char('\x7f') (DEL) 或 Char('\x08') (BS) 传递。
                        // 此处显式拦截，避免退格字符落入 Char(c) 分支被当作普通字符插入。
                        KeyCode::Char('\x7f') | KeyCode::Char('\x08') => {
                            if cursor > 0 {
                                cursor = prev_char_boundary(&buffer, cursor);
                                buffer.remove(cursor);
                                self.redraw_line(&mut stdout, prompt, &buffer, cursor, prompt_width)?;
                                self.update_completion(&mut stdout, &buffer, prompt_width, &mut completion_active, &mut completion_index, &mut completion_items, engine)?;
                            }
                        }
                        KeyCode::Delete => {
                            // Delete：删除光标处字符(cursor 在字符边界,remove 按字节偏移删一个字符)
                            if cursor < buffer.len() {
                                buffer.remove(cursor);
                                self.redraw_line(&mut stdout, prompt, &buffer, cursor, prompt_width)?;
                                self.update_completion(&mut stdout, &buffer, prompt_width, &mut completion_active, &mut completion_index, &mut completion_items, engine)?;
                            }
                        }
                        KeyCode::Left => {
                            // 左箭头：移动光标(按字符,不按字节)
                            if cursor > 0 {
                                cursor = prev_char_boundary(&buffer, cursor);
                                execute!(stdout, MoveToColumn(prompt_width + display_width(&buffer[..cursor])))?;
                                stdout.flush()?;
                            }
                        }
                        KeyCode::Right => {
                            // 右箭头：移动光标(前进一个 UTF-8 字符)
                            if cursor < buffer.len() {
                                cursor += buffer[cursor..].chars().next().map_or(0, |c| c.len_utf8());
                                execute!(stdout, MoveToColumn(prompt_width + display_width(&buffer[..cursor])))?;
                                stdout.flush()?;
                            }
                        }
                        KeyCode::Home => {
                            cursor = 0;
                            execute!(stdout, MoveToColumn(prompt_width))?;
                            stdout.flush()?;
                        }
                        KeyCode::End => {
                            cursor = buffer.len();
                            execute!(stdout, MoveToColumn(prompt_width + display_width(&buffer)))?;
                            stdout.flush()?;
                        }
                        KeyCode::Char(c) => {
                            // 可打印字符：插入到光标位置(cursor 为字节偏移,
                            // 增量按 len_utf8 推进——修复中文输入第 2 字符 panic)
                            buffer.insert(cursor, c);
                            cursor += c.len_utf8();
                            self.redraw_line(&mut stdout, prompt, &buffer, cursor, prompt_width)?;
                            // 更新补全列表
                            self.update_completion(&mut stdout, &buffer, prompt_width, &mut completion_active, &mut completion_index, &mut completion_items, engine)?;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        Ok(InputResult::Exit)
    }

    /// 重绘当前输入行（清除旧内容，输出新内容）。
    fn redraw_line(&self, stdout: &mut impl Write, prompt: &str, buffer: &str, cursor: usize, prompt_width: u16) -> io::Result<()> {
        // 移到行首，清除整行，输出提示符 + 输入内容
        execute!(
            stdout,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            Print(prompt),
            Print(buffer),
        )?;
        // 移动光标到正确位置(cursor 为字节偏移,按显示宽度换算列号)
        execute!(stdout, MoveToColumn(prompt_width + display_width(&buffer[..cursor])))?;
        stdout.flush()?;
        Ok(())
    }

    /// 接受补全：用替换文本替换当前输入。
    /// `replacement` 已包含 `/` 前缀与尾随空格,直接写入缓冲区即可。
    fn accept_completion(&self, stdout: &mut impl Write, buffer: &mut String, cursor: &mut usize, prompt_width: u16, replacement: &str) -> io::Result<()> {
        *buffer = replacement.to_string();
        *cursor = buffer.len();
        self.redraw_line(stdout, ">> ", buffer, *cursor, prompt_width)
    }

    /// 清除补全列表区域。
    fn clear_completion_list(&self, stdout: &mut impl Write, prompt_width: u16, buffer: &str, completion_active: bool, items: &[crate::tui::completion::CompletionItem]) -> io::Result<()> {
        if !completion_active || items.is_empty() {
            return Ok(());
        }
        // 先移回输入行
        execute!(stdout, MoveToColumn(prompt_width + display_width(buffer)))?;
        // 清除输入行以下的内容
        execute!(stdout, Clear(ClearType::FromCursorDown))?;
        stdout.flush()?;
        Ok(())
    }

    /// 更新补全列表（输入变化时调用）。
    fn update_completion(
        &self,
        stdout: &mut impl Write,
        buffer: &str,
        prompt_width: u16,
        active: &mut bool,
        index: &mut usize,
        items: &mut Vec<crate::tui::completion::CompletionItem>,
        engine: &CompletionEngine,
    ) -> io::Result<()> {
        // 仅当输入以 '/' 开头时激活补全
        let trimmed = buffer.trim_start();
        if !trimmed.starts_with('/') || trimmed.len() < 2 {
            // 关闭补全
            if *active {
                self.clear_completion_list(stdout, prompt_width, buffer, *active, items)?;
                *active = false;
                items.clear();
            }
            return Ok(());
        }

        // 获取补全候选项
        let new_items = engine.complete(trimmed);

        if new_items.is_empty() {
            // 无匹配，关闭补全
            if *active {
                self.clear_completion_list(stdout, prompt_width, buffer, *active, items)?;
                *active = false;
                items.clear();
            }
            return Ok(());
        }

        // 更新补全列表
        *items = new_items;
        *index = 0;
        *active = true;

        // 绘制补全列表
        self.draw_completion(stdout, prompt_width, buffer, *index, items)
    }

    /// 绘制补全列表。
    fn draw_completion(
        &self,
        stdout: &mut impl Write,
        prompt_width: u16,
        buffer: &str,
        selected: usize,
        items: &[crate::tui::completion::CompletionItem],
    ) -> io::Result<()> {
        // 移到输入行末尾
        execute!(stdout, MoveToColumn(prompt_width + display_width(buffer)))?;
        // 清除之前的补全列表
        execute!(stdout, Clear(ClearType::FromCursorDown))?;
        // 换行开始绘制补全列表
        execute!(stdout, Print("\r\n"))?;

        for (i, item) in items.iter().enumerate() {
            if i == selected {
                // 选中项：► 前缀 + 反白 + 加粗 + 高亮色
                execute!(
                    stdout,
                    SetForegroundColor(colors::HIGHLIGHT_FG),
                    SetAttribute(Attribute::Bold),
                    SetAttribute(Attribute::Reverse),
                    Print(format!(" ► {}  ", item.display)),
                    SetAttribute(Attribute::Reset),
                    SetForegroundColor(colors::GRAY),
                    Print(format!("  {}\r\n", item.description)),
                )?;
            } else {
                // 未选中项：灰色显示
                execute!(
                    stdout,
                    SetForegroundColor(colors::GRAY),
                    Print(format!("   {}  ", item.display)),
                    Print(format!("  {}\r\n", item.description)),
                )?;
            }
        }

        // 绘制提示
        execute!(
            stdout,
            SetForegroundColor(colors::DIM),
            Print("  ↑↓ 选择  Enter/Tab 接受  Esc 关闭\r\n"),
        )?;
        execute!(stdout, ResetColor)?;

        // 光标移回输入行
        let lines_below = items.len() as u16 + 1; // +1 为提示行
        execute!(stdout, MoveUp(lines_below))?;
        execute!(stdout, MoveToColumn(prompt_width + display_width(buffer)))?;

        stdout.flush()?;
        Ok(())
    }

    /// 重绘补全列表（仅更新选中状态）。
    fn redraw_completion(
        &self,
        stdout: &mut impl Write,
        prompt_width: u16,
        buffer: &str,
        active: bool,
        selected: usize,
        items: &[crate::tui::completion::CompletionItem],
    ) -> io::Result<()> {
        if active && !items.is_empty() {
            self.draw_completion(stdout, prompt_width, buffer, selected, items)
        } else {
            Ok(())
        }
    }

    /// 清除当前输入行（中断或退出时调用）。
    fn clear_current_line(
        &self,
        stdout: &mut impl Write,
        prompt_width: u16,
        buffer: &str,
        completion_active: bool,
        items: &[crate::tui::completion::CompletionItem],
    ) -> io::Result<()> {
        // 清除补全列表
        if completion_active && !items.is_empty() {
            execute!(stdout, MoveToColumn(prompt_width + display_width(buffer)))?;
            execute!(stdout, Clear(ClearType::FromCursorDown))?;
        }
        // 清除输入行
        execute!(stdout, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
        stdout.flush()?;
        Ok(())
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
}
