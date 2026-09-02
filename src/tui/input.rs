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
                                // 接受补全
                                let item = &completion_items[completion_index];
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
                            // 退格：删除光标前字符
                            if cursor > 0 {
                                cursor -= 1;
                                buffer.remove(cursor);
                                self.redraw_line(&mut stdout, prompt, &buffer, cursor, prompt_width)?;
                                // 更新补全列表
                                self.update_completion(&mut stdout, &buffer, prompt_width, &mut completion_active, &mut completion_index, &mut completion_items, engine)?;
                            }
                        }
                        KeyCode::Delete => {
                            // Delete：删除光标处字符
                            if cursor < buffer.len() {
                                buffer.remove(cursor);
                                self.redraw_line(&mut stdout, prompt, &buffer, cursor, prompt_width)?;
                                self.update_completion(&mut stdout, &buffer, prompt_width, &mut completion_active, &mut completion_index, &mut completion_items, engine)?;
                            }
                        }
                        KeyCode::Left => {
                            // 左箭头：移动光标
                            if cursor > 0 {
                                cursor -= 1;
                                execute!(stdout, MoveToColumn(prompt_width + cursor as u16))?;
                                stdout.flush()?;
                            }
                        }
                        KeyCode::Right => {
                            // 右箭头：移动光标
                            if cursor < buffer.len() {
                                cursor += 1;
                                execute!(stdout, MoveToColumn(prompt_width + cursor as u16))?;
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
                            execute!(stdout, MoveToColumn(prompt_width + cursor as u16))?;
                            stdout.flush()?;
                        }
                        KeyCode::Char(c) => {
                            // 可打印字符：插入到光标位置
                            buffer.insert(cursor, c);
                            cursor += 1;
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
        // 移动光标到正确位置
        execute!(stdout, MoveToColumn(prompt_width + cursor as u16))?;
        stdout.flush()?;
        Ok(())
    }

    /// 接受补全：用替换文本替换当前输入。
    fn accept_completion(&self, stdout: &mut impl Write, buffer: &mut String, cursor: &mut usize, prompt_width: u16, replacement: &str) -> io::Result<()> {
        *buffer = replacement.to_string();
        // 追加一个空格，方便用户继续输入参数
        buffer.push(' ');
        *cursor = buffer.len();
        self.redraw_line(stdout, ">> ", buffer, *cursor, prompt_width)
    }

    /// 清除补全列表区域。
    fn clear_completion_list(&self, stdout: &mut impl Write, prompt_width: u16, buffer: &str, completion_active: bool, items: &[crate::tui::completion::CompletionItem]) -> io::Result<()> {
        if !completion_active || items.is_empty() {
            return Ok(());
        }
        // 先移回输入行
        execute!(stdout, MoveToColumn(prompt_width + buffer.len() as u16))?;
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
        execute!(stdout, MoveToColumn(prompt_width + buffer.len() as u16))?;
        // 清除之前的补全列表
        execute!(stdout, Clear(ClearType::FromCursorDown))?;
        // 换行开始绘制补全列表
        execute!(stdout, Print("\r\n"))?;

        for (i, item) in items.iter().enumerate() {
            if i == selected {
                // 选中项：反白显示
                execute!(
                    stdout,
                    SetForegroundColor(colors::HIGHLIGHT_FG),
                    SetAttribute(Attribute::Reverse),
                    Print(format!(" > {}  ", item.display)),
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
        execute!(stdout, MoveToColumn(prompt_width + buffer.len() as u16))?;

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
            execute!(stdout, MoveToColumn(prompt_width + buffer.len() as u16))?;
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
}
