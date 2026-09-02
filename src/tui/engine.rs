//! 独立 CLI 渲染引擎 —— Screen trait + Frame + 全量重绘的 present 实现。
//!
//! 设计要点见 `docs/TUI界面与CLI渲染引擎/02-技术设计.md` §2/§3。
//! - Screen 不直接写 stdout;只往 `Frame` 填充 Cell。
//! - `present` 全量清屏 + 输出,适合 < 30 行的子屏。
//! - 主屏仍然走 `input.rs` 的单行渲染;引擎只接管子屏。

use std::io::{self, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{KeyEvent, KeyEventKind},
    execute,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::tui::theme;

/// 屏幕区域(简化版 ratatui Rect)。
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self { x, y, width, height }
    }

    pub fn full_screen() -> Self {
        let (w, h) = terminal::size().unwrap_or((80, 24));
        Self { x: 0, y: 0, width: w, height: h }
    }
}

/// 单个渲染单元格。
#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attr: Attribute,
}

impl Cell {
    fn blank() -> Self {
        Self {
            ch: ' ',
            fg: theme::FG,
            bg: Color::Reset,
            attr: Attribute::Reset,
        }
    }
}

/// 一帧画面:按行优先存储 Cell。
pub struct Frame {
    pub area: Rect,
    cells: Vec<Cell>,
}

impl Frame {
    pub fn new(area: Rect) -> Self {
        let len = (area.width as usize) * (area.height as usize);
        Self { area, cells: (0..len).map(|_| Cell::blank()).collect() }
    }

    fn idx(&self, x: u16, y: u16) -> Option<usize> {
        if x >= self.area.width || y >= self.area.height {
            return None;
        }
        Some((y as usize) * (self.area.width as usize) + (x as usize))
    }

    /// 在指定位置写一个字符(超出区域静默忽略)。
    pub fn put_char(&mut self, x: u16, y: u16, ch: char, fg: Color, attr: Attribute) {
        if let Some(i) = self.idx(x, y) {
            self.cells[i] = Cell { ch, fg, bg: Color::Reset, attr };
        }
    }

    /// 在区域内写入字符串(遇换行换到下一行;不够放就截断)。
    pub fn put_str(&mut self, area: Rect, s: &str, fg: Color, attr: Attribute) {
        let mut x = area.x;
        let mut y = area.y;
        for ch in s.chars() {
            if ch == '\n' {
                y += 1;
                x = area.x;
                continue;
            }
            if x >= area.x + area.width {
                continue;
            }
            if y >= area.y + area.height {
                break;
            }
            self.put_char(x, y, ch, fg, attr);
            x += 1;
        }
    }

    /// 在区域内居中写一行(用于标题 / 单行消息)。
    pub fn put_str_centered(&mut self, y: u16, s: &str, fg: Color, attr: Attribute) {
        let w = s.chars().count() as u16;
        let x = self.area.width.saturating_sub(w) / 2;
        let area = Rect::new(x, y, w.min(self.area.width), 1);
        self.put_str(area, s, fg, attr);
    }

    /// 用 ASCII 边框绘制一个矩形 + 标题。
    pub fn border_box(&mut self, area: Rect, title: Option<&str>) {
        let w = area.width as usize;
        let h = area.height as usize;
        if w < 2 || h < 2 {
            return;
        }
        // 顶 / 底
        for x in 1..(w - 1) {
            self.put_char(area.x + x as u16, area.y, '─', theme::ACCENT, Attribute::Reset);
            self.put_char(
                area.x + x as u16,
                area.y + area.height - 1,
                '─',
                theme::ACCENT,
                Attribute::Reset,
            );
        }
        // 左 / 右
        for y in 1..(h - 1) {
            self.put_char(area.x, area.y + y as u16, '│', theme::ACCENT, Attribute::Reset);
            self.put_char(
                area.x + area.width - 1,
                area.y + y as u16,
                '│',
                theme::ACCENT,
                Attribute::Reset,
            );
        }
        // 四角
        self.put_char(area.x, area.y, '╭', theme::ACCENT, Attribute::Reset);
        self.put_char(area.x + area.width - 1, area.y, '╮', theme::ACCENT, Attribute::Reset);
        self.put_char(
            area.x,
            area.y + area.height - 1,
            '╰',
            theme::ACCENT,
            Attribute::Reset,
        );
        self.put_char(
            area.x + area.width - 1,
            area.y + area.height - 1,
            '╯',
            theme::ACCENT,
            Attribute::Reset,
        );

        if let Some(t) = title {
            let label = format!(" {} ", t);
            self.put_str(
                Rect::new(area.x + 2, area.y, label.chars().count() as u16, 1),
                &label,
                theme::ACCENT,
                Attribute::Bold,
            );
        }
    }
}

/// Screen 的下一步动作。
/// 不实现 Debug,因为 `Box<dyn Screen>` 不满足 Debug。
pub enum Outcome {
    /// 留在当前屏;引擎会再 render + 等待下一次按键。
    Continue,
    /// 弹出当前屏(主屏收到 Pop 后会退出)。
    Pop,
    /// 推送新屏(模态栈 push)。
    Push(Box<dyn Screen>),
    /// 把消息写到主屏后退出当前屏。
    Toast(String),
    /// 永久退出 TUI。
    Quit,
}

/// 屏幕 trait。
pub trait Screen: Send {
    fn title(&self) -> &str;
    fn render(&self, frame: &mut Frame);
    fn handle_key(&mut self, key: KeyEvent) -> Outcome;
    fn on_enter(&mut self) {}
    fn on_exit(&mut self) {}
}

/// 进入 alternate screen + 隐藏光标。子屏生命周期内调用。
pub fn enter_alt() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    Ok(())
}

/// 离开 alternate screen + 显示光标 + 退出原始模式。
pub fn leave_alt() -> io::Result<()> {
    execute!(io::stdout(), Show, LeaveAlternateScreen)?;
    let _ = terminal::disable_raw_mode();
    Ok(())
}

/// 把 Frame 全量绘制到 stdout(子屏用)。
pub fn present(frame: &Frame) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    execute!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;

    let w = frame.area.width as usize;
    let mut prev_fg = Color::Reset;
    let mut prev_bg = Color::Reset;
    let mut prev_attr = Attribute::Reset;

    for y in 0..frame.area.height {
        // 移到行首
        execute!(stdout, MoveTo(0, y))?;
        let mut line = String::with_capacity(w);
        for x in 0..frame.area.width {
            let cell = &frame.cells[(y as usize) * w + (x as usize)];
            line.push(cell.ch);
        }
        // 重置颜色后输出整行(简化:全行同色,由后续业务控制;子屏用纯 ASCII 边框 + 内容,够用)
        execute!(
            stdout,
            ResetColor,
            SetForegroundColor(prev_fg),
            SetBackgroundColor(prev_bg),
            SetAttribute(prev_attr),
            Print(&line),
        )?;
    }
    execute!(stdout, ResetColor)?;
    stdout.flush()?;
    Ok(())
}

/// 读取一个 Key 事件(只读 Press 事件,过滤 Release/Repeat)。
pub fn read_key() -> io::Result<KeyEvent> {
    loop {
        if let crossterm::event::Event::Key(k) = crossterm::event::read()? {
            if k.kind == KeyEventKind::Press {
                return Ok(k);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_put_str_wraps() {
        let mut f = Frame::new(Rect::new(0, 0, 10, 3));
        f.put_str(Rect::new(0, 0, 10, 1), "hello", theme::FG, Attribute::Reset);
        let idx = |x, y| (y as usize) * 10 + (x as usize);
        assert_eq!(f.cells[idx(0, 0)].ch, 'h');
        assert_eq!(f.cells[idx(4, 0)].ch, 'o');
        assert_eq!(f.cells[idx(5, 0)].ch, ' ');
    }

    #[test]
    fn frame_border_box() {
        let mut f = Frame::new(Rect::new(0, 0, 8, 4));
        f.border_box(Rect::new(0, 0, 8, 4), Some("T"));
        let idx = |x, y| (y as usize) * 8 + (x as usize);
        assert_eq!(f.cells[idx(0, 0)].ch, '╭');
        assert_eq!(f.cells[idx(7, 3)].ch, '╯');
    }
}