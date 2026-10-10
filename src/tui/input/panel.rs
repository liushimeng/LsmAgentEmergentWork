//! 底部输入面板与 DECSTBM 滚动区的全部绘制/定位逻辑(第 134 轮从 `input.rs` 拆出)。
//!
//! 职责:
//! - [`Layout`] 行号计算(滚动区 + 面板);
//! - `panel_top_for` 纯函数决策(面板顶行**紧跟光标**,查不到光标行时回退吸底);
//! - `query_cursor_row` 光标行查询(自实现 DSR,原因见函数文档);
//! - [`InputHandler`] 的绘制方法:`enter_pinned` / `redraw_line` /
//!   `print_in_scroll_region` / `draw_overlay` / `clear_overlay`。
//!
//! 机械搬移自 `input.rs`,逻辑零改写;外部 `crate::tui::input::Layout` 等路径
//! 由 `mod.rs` 的 `pub use panel::*` 再导出,调用方零改动。

use std::io::{self, Write};

use crossterm::{
    cursor::MoveTo,
    execute,
    terminal,
    style::{Attribute, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};

use super::{display_width, fit_width, visible_window, window_str};
use crate::tui::completion::CompletionItem;
use crate::tui::theme;

/// 屏幕布局:滚动区 + 底部固定输入面板的行号计算。
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    /// 面板高度(1 = 矮终端降级,只保留输入行)。
    pub(crate) area_h: u16,
    /// 面板顶行(0-indexed;3 行面板时为分隔线行)。
    pub(crate) panel_top: u16,
    /// 输入行(0-indexed)。
    pub(crate) input_y: u16,
    /// 快捷键提示行(0-indexed;降级时为 None)。
    pub(crate) hint_y: Option<u16>,
}

impl Layout {
    pub(crate) fn detect() -> Self {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        Self::compute(cols, rows)
    }

    pub(crate) fn compute(cols: u16, rows: u16) -> Self {
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
    ///
    /// 第 134 轮:面板改为**自适应**(顶行紧跟当前光标)后,滚动区底行就是面板顶行
    /// 本身 —— 输出区填满后光标停在面板正上方,输出与面板之间**恒不留空行**。
    /// 吸底态下 `panel_top == rows - area_h`,与旧行为完全一致。
    pub(crate) fn scroll_bottom(&self) -> u16 {
        self.panel_top.max(1)
    }

    /// 滚动区最后一行(0-indexed,输出光标锚点)。
    pub(crate) fn scroll_last_row(&self) -> u16 {
        self.scroll_bottom().saturating_sub(1)
    }
}

/// 向终端发 `ESC[6n` 并解析回包,返回光标所在行(0-indexed);不支持/超时 → `None`。
///
/// 为什么不直接用 `crossterm::cursor::position()`:本机 tmux 下实测**恒返回 `Ok((0,0))`**
/// (连查三次一致),拿它定位会把自适应面板顶到第 2 行,反把刚打印的启动横幅抹掉。
/// 这里自己发 DSR、用 `libc::poll` 限时读 stdin:最多等 ~300ms,失败即回退吸底,
/// 绝不挂起 TUI;解析只认 `ESC [ <row> ; <col> R` 这一段,误读到的杂字节会被忽略。
/// unix 实现:用 `libc::poll` + `libc::read` 直读 fd 0(见函数文档)。
#[cfg(unix)]
pub(super) fn query_cursor_row() -> Option<u16> {
    // ★ 先把 stdin 里**已存在的陈旧字节**排空再问:实测本机 tmux 下首次查询会读到一条
    //   来路不明的 `[1;1R`(不是本次 DSR 的回包),据此定位会把面板顶到第 2 行,
    //   反把刚打印的启动横幅抹掉(加了第二次查询才拿到真值 19)。这里非阻塞读干,
    //   窗口只有几十微秒,误吞一次按键的概率可忽略(与 crossterm 自身行为同级)。
    let mut drain = [0u8; 64];
    for _ in 0..8 {
        let mut fds = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let n = unsafe { libc::poll(&mut fds, 1, 0) };
        if n <= 0 {
            break;
        }
        let r = unsafe {
            libc::read(
                libc::STDIN_FILENO,
                drain.as_mut_ptr() as *mut libc::c_void,
                drain.len(),
            )
        };
        if r <= 0 {
            break;
        }
    }
    {
        let mut out = io::stdout();
        out.write_all(b"\x1b[6n").ok()?;
        out.flush().ok()?;
    }
    let mut acc: Vec<u8> = Vec::with_capacity(16);
    let mut buf = [0u8; 32];
    for _ in 0..3 {
        let mut fds = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let n = unsafe { libc::poll(&mut fds, 1, 100) };
        if n <= 0 {
            continue;
        }
        let r = unsafe {
            libc::read(
                libc::STDIN_FILENO,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if r <= 0 {
            continue;
        }
        acc.extend_from_slice(&buf[..r as usize]);
        let s = String::from_utf8_lossy(&acc).into_owned();
        let Some(ridx) = s.rfind('R') else { continue };
        let head = &s[..ridx];
        let Some(esc) = head.rfind('\u{1b}') else { continue };
        let body = head[esc + 1..].trim_start_matches('[');
        if let Some(row_s) = body.split(';').next() {
            if let Ok(row) = row_s.trim().parse::<u16>() {
                if row >= 1 {
                    return Some(row - 1);
                }
            }
        }
    }
    None
}

/// 非 unix 平台无 `libc::poll`/`libc::read` 这条限时读路径,返回 `None`。
///
/// 调用方(`input/mod.rs` 的 `Layout::panel_top_for`)对 `None` 的处理是
/// **回退吸底**,与「查询失败/超时」同款 —— 这是既有安全默认,不会猜错位置。
#[cfg(not(unix))]
pub(super) fn query_cursor_row() -> Option<u16> {
    None
}

/// 上一次绘制输入面板的顶行(0-indexed)。
///
/// 第 134 轮面板自适应后,面板顶行不再恒等于 `rows - area_h`,而是由 `enter_pinned`
/// 按光标位置动态算出。`teardown_pinned()`(独立入口,拿不到 `read_line_inner` 内的
/// `layout` 值)靠这个静态量还原出真正的面板范围,否则会漏清自适应面板的残留行。
/// `u16::MAX` = 尚未绘制过,回退到吸底位置。
static LAST_PANEL_TOP: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(u16::MAX);

pub(super) fn remember_panel_top(y: u16) {
    LAST_PANEL_TOP.store(y, std::sync::atomic::Ordering::SeqCst);
}

pub(super) fn recalled_layout() -> Layout {
    let mut l = Layout::detect();
    let top = LAST_PANEL_TOP.load(std::sync::atomic::Ordering::SeqCst);
    if top != u16::MAX && l.area_h >= 3 && top < l.rows {
        l.panel_top = top;
        l.input_y = top + 1;
        l.hint_y = Some(top + 2);
    }
    l
}

/// 面板顶行决策(纯函数,可单测)。
///
/// - `area_h < 3`:矮终端降级为单行面板,顶行恒为 `rows - 1`,不参与自适应;
/// - `cursor_row = None`(终端不支持 DSR / 回包超时):保持吸底,绝不猜;
/// - 否则取「光标下一行」,并夹紧到吸底位置。
pub(super) fn panel_top_for(rows: u16, area_h: u16, cursor_row: Option<u16>) -> u16 {
    let sticky_top = rows.saturating_sub(area_h);
    if area_h < 3 {
        return sticky_top;
    }
    match cursor_row {
        Some(cur) => cur.saturating_add(1).min(sticky_top),
        None => sticky_top,
    }
}

/// 还原终端:重置 DECSTBM 滚动区并清空底部输入面板,光标移到面板顶行。
pub(super) fn teardown_pinned_layout(stdout: &mut impl Write, layout: &Layout) -> io::Result<()> {
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
    let layout = recalled_layout();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let _ = teardown_pinned_layout(&mut stdout, &layout);
}
impl super::InputHandler {
    /// 设置 DECSTBM 滚动区并绘制固定面板(分隔线 + 输入行 + 快捷键提示行)。
    ///
    /// 第 134 轮:面板**自适应** —— 顶行默认紧跟当前光标行,而不是恒定吸屏幕底部。
    /// 旧行为在「输出短 + 大终端」时会在输出末行与分隔线之间留大片空白
    /// (实测 `--debug` 启动横幅后固定留 16 行空),观感像一串空回车。
    /// 自适应后滚动区底行 = 面板顶行,输出填满滚动区时光标正好停在面板正上方,
    /// **输出与面板之间恒不留空行**;内容接近屏幕底部时自动吸底,行为与旧版一致。
    pub(super) fn enter_pinned(
        &self,
        stdout: &mut impl Write,
        layout: &mut Layout,
        cursor_row: Option<u16>,
    ) -> io::Result<()> {
        if layout.area_h >= 3 {
            // 查不到光标行(终端不支持 DSR / 回包超时)→ 保持吸底,不猜、不清屏。
            if cursor_row.is_some() {
                let want = panel_top_for(layout.rows, layout.area_h, cursor_row);
                if want != layout.panel_top {
                    // 面板位置变了:面板以下的行一律是残留(旧面板区 / 上一次的空白),
                    // 一并清干净,否则自适应切换会留下残影。
                    for y in want..layout.rows {
                        execute!(stdout, MoveTo(0, y), Clear(ClearType::CurrentLine))?;
                    }
                    layout.panel_top = want;
                    layout.input_y = want + 1;
                    layout.hint_y = Some(want + 2);
                }
                remember_panel_top(layout.panel_top);
            }
        }
        // DECSTBM: 滚动区 = 1..=scroll_bottom(1-indexed),面板行排除在外
        execute!(
            stdout,
            ResetColor,
            Print(format!("\x1b[1;{}r", layout.scroll_bottom()))
        )?;
        // 光标移到滚动区末行(输出从这里继续,紧贴面板上方)
        execute!(stdout, MoveTo(0, layout.scroll_last_row()))?;
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
    pub(super) fn redraw_line(
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
    pub(super) fn print_in_scroll_region(
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

    /// 绘制补全浮层(面板上方,向上排布),返回占用行数。
    pub(super) fn draw_overlay(
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
    pub(super) fn clear_overlay(
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

}
