//! 独立 CLI 渲染引擎 —— Screen trait + Frame + 全量重绘的 present 实现。
//!
//! 设计要点见 `docs/TUI界面与CLI渲染引擎/02-技术设计.md` §2/§3。
//! - Screen 不直接写 stdout;只往 `Frame` 填充 Cell。
//! - `present` 全量清屏 + 输出,适合 < 30 行的子屏。
//! - 主屏仍然走 `input.rs` 的单行渲染;引擎只接管子屏。

use std::io::{self, Write};
use crate::tui::input::{char_width, display_width};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{KeyEvent, KeyEventKind},
    execute,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::tui::theme::{self, attr};

// Re-export for use by other TUI modules (mod.rs::truncate etc.)

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
    /// 属性位掩码,见 `theme::attr`。
    pub attrs: u8,
    /// true = 该 cell 是某个 wide char 的"第二半占位",present() 必须跳过
    /// (详见 tmpPlan/2026-09-09_15-TUI宽字符二次占位与tmux捕获渲染方案.md)。
    /// 这样 tmux capture-pane 拿到的就是原始 CJK 字符而不是"记 录"带空格,
    /// `testReport/run_e2e.sh::texpect` 的 `grep -F` 字符串断言可正常命中。
    pub skip: bool,
}

impl Cell {
    fn blank() -> Self {
        Self {
            ch: ' ',
            fg: theme::FG,
            bg: Color::Reset,
            attrs: attr::NONE,
            skip: false,
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
    ///
    /// 宽字符(CJK / 全角,char_width == 2)处理:
    /// - cell[x] 写入字符本身
    /// - cell[x+1] 写入相同字符并标 `skip = true`,present() 跳过该 cell,
    ///   这样 tmux capture-pane 拿到的是原始 CJK 字符串(无空格),
    ///   e2e 的 `grep -F "记录:"` 等字符串断言可正常命中
    /// - 越界保护:x+1 >= area.width 时只写左半,避免越界 panic
    pub fn put_char(&mut self, x: u16, y: u16, ch: char, fg: Color, attrs: u8) {
        let w = char_width(ch);
        if let Some(i) = self.idx(x, y) {
            self.cells[i] = Cell {
                ch,
                fg,
                bg: Color::Reset,
                attrs,
                skip: false,
            };
            // 宽字符:标记右半 cell 为 skip(同时写相同 ch 以便回退显示)
            if w == 2 {
                if let Some(j) = self.idx(x + 1, y) {
                    self.cells[j] = Cell {
                        ch,
                        fg,
                        bg: Color::Reset,
                        attrs,
                        skip: true,
                    };
                }
            }
        }
    }

    /// 在区域内写入字符串(遇换行换到下一行;不够放就截断)。
    pub fn put_str(&mut self, area: Rect, s: &str, fg: Color, attrs: u8) {
        let mut x = area.x;
        let mut y = area.y;
        for ch in s.chars() {
            if ch == '\n' {
                y += 1;
                x = area.x;
                continue;
            }
            if x + char_width(ch) > area.x + area.width {
                continue;
            }
            if y >= area.y + area.height {
                break;
            }
            self.put_char(x, y, ch, fg, attrs);
            x += char_width(ch);
        }
    }

    /// 在区域内居中写一行(用于标题 / 单行消息)。
    pub fn put_str_centered(&mut self, y: u16, s: &str, fg: Color, attrs: u8) {
        let w = display_width(s);
        let x = self.area.width.saturating_sub(w) / 2;
        let area = Rect::new(x, y, w.min(self.area.width), 1);
        self.put_str(area, s, fg, attrs);
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
            self.put_char(area.x + x as u16, area.y, '─', theme::ACCENT, attr::NONE);
            self.put_char(
                area.x + x as u16,
                area.y + area.height - 1,
                '─',
                theme::ACCENT,
                attr::NONE,
            );
        }
        // 左 / 右
        for y in 1..(h - 1) {
            self.put_char(area.x, area.y + y as u16, '│', theme::ACCENT, attr::NONE);
            self.put_char(
                area.x + area.width - 1,
                area.y + y as u16,
                '│',
                theme::ACCENT,
                attr::NONE,
            );
        }
        // 四角
        self.put_char(area.x, area.y, '╭', theme::ACCENT, attr::NONE);
        self.put_char(area.x + area.width - 1, area.y, '╮', theme::ACCENT, attr::NONE);
        self.put_char(
            area.x,
            area.y + area.height - 1,
            '╰',
            theme::ACCENT,
            attr::NONE,
        );
        self.put_char(
            area.x + area.width - 1,
            area.y + area.height - 1,
            '╯',
            theme::ACCENT,
            attr::NONE,
        );

        if let Some(t) = title {
            let label = format!(" {} ", t);
            let label_w = display_width(&label);
            self.put_str(
                Rect::new(area.x + 2, area.y, label_w, 1),
                &label,
                theme::ACCENT,
                attr::BOLD,
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
    // 先重置 DECSTBM 滚动区:主屏固定底部输入组件会设置滚动区,
    // 若泄漏进 alternate screen,present() 的行尾换行会在滚动区底缘
    // 触发滚动而非换行,破坏子屏渲染。
    execute!(io::stdout(), ResetColor, Print("\x1b[r"), EnterAlternateScreen, Hide)?;
    Ok(())
}

/// 离开 alternate screen + 显示光标 + 退出原始模式。
pub fn leave_alt() -> io::Result<()> {
    execute!(io::stdout(), Show, LeaveAlternateScreen)?;
    let _ = terminal::disable_raw_mode();
    Ok(())
}

/// 把 attrs 位掩码展开为 crossterm Attribute 序列。
fn attrs_to_list(attrs: u8) -> Vec<Attribute> {
    let mut v = Vec::new();
    if attrs & attr::BOLD != 0 {
        v.push(Attribute::Bold);
    }
    if attrs & attr::REVERSE != 0 {
        v.push(Attribute::Reverse);
    }
    if attrs & attr::DIM != 0 {
        v.push(Attribute::Dim);
    }
    if attrs & attr::UNDERLINED != 0 {
        v.push(Attribute::Underlined);
    }
    v
}

/// 把 Frame 全量绘制到 stdout(子屏用)。
/// 逐 cell 检查 fg/bg/attrs,与"前一个 cell"比较;样式变化时输出 ANSI 序列,相同样式的连续 cell 合并成一个 Print 批次。
pub fn present(frame: &Frame) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    execute!(
        stdout,
        MoveTo(0, 0),
        Clear(ClearType::All),
        ResetColor,
        SetAttribute(Attribute::Reset),
    )?;

    let w = frame.area.width as usize;
    let mut cur_fg = Color::Reset;
    let mut cur_bg = Color::Reset;
    let mut cur_attrs: u8 = attr::NONE;

    for y in 0..frame.area.height {
        execute!(stdout, MoveTo(0, y))?;
        let mut batch = String::with_capacity(w);
        for x in 0..frame.area.width {
            let cell = &frame.cells[(y as usize) * w + (x as usize)];
            if cell.fg != cur_fg || cell.bg != cur_bg || cell.attrs != cur_attrs {
                // flush 旧批次
                if !batch.is_empty() {
                    execute!(stdout, Print(&batch))?;
                    batch.clear();
                }
                // 更新属性(先复位再按需设置)
                if cell.attrs != cur_attrs {
                    execute!(stdout, SetAttribute(Attribute::Reset))?;
                    for a in attrs_to_list(cell.attrs) {
                        execute!(stdout, SetAttribute(a))?;
                    }
                    cur_attrs = cell.attrs;
                }
                if cell.bg != cur_bg {
                    execute!(stdout, SetBackgroundColor(cell.bg))?;
                    cur_bg = cell.bg;
                }
                if cell.fg != cur_fg {
                    execute!(stdout, SetForegroundColor(cell.fg))?;
                    cur_fg = cell.fg;
                }
            }
            // 跳过宽字符续位 cell(write 时已写入 ch 到左半,这里不重复)
            // 否则会把"记 录"重复成"记  录 ",视觉与 tmux 捕获都错位
            if !cell.skip {
                batch.push(cell.ch);
            }
        }
        if !batch.is_empty() {
            execute!(stdout, Print(&batch))?;
        }
        // 每行结束后复位,避免行间样式渗透
        execute!(
            stdout,
            ResetColor,
            SetAttribute(Attribute::Reset),
        )?;
        cur_fg = Color::Reset;
        cur_bg = Color::Reset;
        cur_attrs = attr::NONE;
    }
    execute!(stdout, ResetColor, SetAttribute(Attribute::Reset))?;
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
        f.put_str(Rect::new(0, 0, 10, 1), "hello", theme::FG, attr::NONE);
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

    #[test]
    fn cell_attrs_bitmask() {
        let mut f = Frame::new(Rect::new(0, 0, 8, 1));
        // 同时设置 BOLD + REVERSE
        f.put_str(
            Rect::new(0, 0, 8, 1),
            "OK",
            theme::SELECTED_FG,
            theme::SELECTED_ATTRS,
        );
        let idx = |x, y| (y as usize) * 8 + (x as usize);
        assert_eq!(f.cells[idx(0, 0)].ch, 'O');
        assert_eq!(f.cells[idx(0, 0)].fg, theme::SELECTED_FG);
        assert_eq!(f.cells[idx(0, 0)].attrs & attr::BOLD, attr::BOLD);
        assert_eq!(f.cells[idx(0, 0)].attrs & attr::REVERSE, attr::REVERSE);
        // 空白 cell 应该是 attr::NONE
        assert_eq!(f.cells[idx(7, 0)].attrs, attr::NONE);
    }

    #[test]
    fn attrs_to_list_expands_bits() {
        let list = attrs_to_list(attr::BOLD | attr::REVERSE);
        assert!(list.contains(&Attribute::Bold));
        assert!(list.contains(&Attribute::Reverse));
        let empty = attrs_to_list(attr::NONE);
        assert!(empty.is_empty());
    }
}
