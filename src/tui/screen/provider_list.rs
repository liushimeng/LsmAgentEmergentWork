//! ProviderList 屏 —— /provider list 的 Tab 化展示。
//!
//! Tab 顺序：
//! 1..=5. 5 个只读字段 (id / protocol / provider_name / model_name / end_point / api_key)
//! 6. 操作按钮组 [Switch] [Delete] [Back]

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Db, Paths, ProviderRecord};
use crate::tui::engine::{Frame, Outcome, Rect, Screen};
use crate::tui::input::display_width;
use crate::tui::screen::provider_del::ProviderDelPicker;
use crate::tui::theme::{self, attr};

const FIELD_LABELS: [&str; 7] = ["id", "protocol", "provider_name", "model_name", "end_point", "api_key", "context_max_size"];

pub struct ProviderList {
    pub records: Vec<ProviderRecord>,
    pub cursor: usize,
    pub action_cursor: usize,
    pub db: Arc<Mutex<Db>>,
    pub paths: Paths,
}

impl ProviderList {
    pub fn new(db: Arc<Mutex<Db>>, paths: Paths) -> Self {
        let records = db.lock().expect("db").list().unwrap_or_default();
        Self {
            records,
            cursor: 0,
            action_cursor: 0,
            db,
            paths,
        }
    }

    fn current(&self) -> Option<&ProviderRecord> {
        self.records.get(self.cursor)
    }

    fn field_value(&self, idx: usize) -> String {
        let r = match self.current() {
            Some(r) => r,
            None => return String::new(),
        };
        match idx {
            0 => r.id.to_string(),
            1 => r.protocol.as_str().to_string(),
            2 => r.provider_name.clone(),
            3 => r.model_name.clone(),
            // end_point 在子屏值列宽有限(默认 96 列 → 值列 ≈ 72 字符),
            // 超长按智能省略:保留 scheme://host 末段与 path 末两段。
            // 关联报告: 2026-09-09_07 F-007-3
            4 => truncate_endpoint(&r.end_point, 72),
            5 => theme::mask_key(&r.api_key),
            6 => format!(
                "{} ({})",
                r.context_max_size,
                crate::config::format_context_size(r.context_max_size)
            ),
            _ => String::new(),
        }
    }
    fn switch_active(&mut self) -> Outcome {
        let id = match self.current() {
            Some(r) => r.id,
            None => return Outcome::Continue,
        };
        let res = self.db.lock().expect("db").set_active(id);
        match res {
            Ok(_) => Outcome::Toast(format!("✓ 已切换当前模型为 id={id}")),
            Err(e) => Outcome::Toast(format!("! 切换失败: {e}")),
        }
    }
}

/// 智能截断 end_point URL,保留 scheme://host + path 末两段(对长 Anthropic/OpenAI 端点友好)。
/// 字符宽度按 char count 计(中文 / emoji 不会越界,但 CJK 比例过高时仍可能折行)。
/// 关联报告: 2026-09-09_07 F-007-3
fn truncate_endpoint(url: &str, max_chars: usize) -> String {
    let chars: Vec<char> = url.chars().collect();
    if chars.len() <= max_chars {
        return url.to_string();
    }
    // 按 `/` 切分,保留首段(scheme://host)与末两段(path 子段)
    let parts: Vec<&str> = url.split('/').collect();
    if parts.len() <= 4 {
        // 无 path 或仅有 1 段:简单按 char 截断,加省略号
        let head: String = chars.iter().take(max_chars.saturating_sub(1)).collect();
        return format!("{head}…");
    }
    let head = parts[0]; // e.g. "https:" 或 "http:"
    let host = parts.get(2).copied().unwrap_or("");
    let tail = &parts[parts.len() - 2..];
    let prefix = format!("{head}//{host}/…/{}", tail.join("/"));
    if prefix.chars().count() <= max_chars {
        return prefix;
    }
    // host 也太长:再按 char 截断 head
    let head_budget = max_chars.saturating_sub(tail.join("/").chars().count() + 6);
    if head_budget == 0 {
        return format!("…/{}", tail.join("/"));
    }
    let head_trunc: String = format!("{head}//{host}").chars().take(head_budget).collect();
    format!("{head_trunc}…/{}", tail.join("/"))
}

impl Screen for ProviderList {
    fn title(&self) -> &str {
        "/provider list"
    }

    fn render(&self, frame: &mut Frame) {
        frame.border_box(
            Rect::new(0, 0, frame.area.width, frame.area.height),
            Some(self.title()),
        );

        // 标题行
        let header = format!(
            "记录: {}/{}   当前: {}   Tab ←→ 切换  ↑↓ 记录  Enter 按钮  Esc 返回",
            if self.records.is_empty() { 0 } else { self.cursor + 1 },
            self.records.len(),
            self.current()
                .map(|r| format!("id={}", r.id))
                .unwrap_or_else(|| "<无>".to_string()),
        );
        frame.put_str(
            Rect::new(2, 1, frame.area.width.saturating_sub(4), 1),
            &header,
            theme::DIM,
            attr::NONE,
        );

        if self.records.is_empty() {
            let area = Rect::new(4, 4, frame.area.width.saturating_sub(8), 1);
            frame.put_str(area, "(空)尚未配置任何接入记录,使用 /provider add 新增。", theme::FG, attr::NONE);
        } else {
            // 字段 Tab 列表
            let top = 3u16;
            for (i, label) in FIELD_LABELS.iter().enumerate() {
                let y = top + i as u16;
                if y + 1 >= frame.area.height.saturating_sub(3) {
                    break;
                }
                let label_area = Rect::new(2, y, 18, 1);
                let value_area = Rect::new(22, y, frame.area.width.saturating_sub(24), 1);
                frame.put_str(label_area, &format!("{}:", label), theme::ACCENT, attr::NONE);
                frame.put_str(value_area, &self.field_value(i), theme::FG, attr::NONE);
            }

            // 操作按钮 —— 选中态: `▶ [ X ] ◀` + ACCENT + Bold + Reverse
            let action_y = frame.area.height.saturating_sub(4);
            let labels = ["[ 设为当前 s ]", "[ 删除 d ]", "[ 返回 Esc ]"];
            let mut x = 2u16;
            for (i, l) in labels.iter().enumerate() {
                let focused = self.action_cursor == i;
                let (fg, attrs, text) = if focused {
                    (
                        theme::SELECTED_FG,
                        theme::SELECTED_ATTRS,
                        format!("{}{}{}", theme::SELECTED_BUTTON_L, l, theme::SELECTED_BUTTON_R),
                    )
                } else {
                    (theme::FG, attr::NONE, format!("  {}  ", l))
                };
                let w = display_width(&text);
                // 注意:必须用 display_width() 而非 chars().count() —— CJK 字符占 2 列,
                // 用 chars().count() 会低估宽度,导致末尾的 `]` / `◀` 被 put_str 的边界检查截断
                // (provider_del.rs 同源 bug 已修,此处同步修复)。
                let area = Rect::new(x, action_y, w + 1, 1);
                frame.put_str(area, &text, fg, attrs);
                x += w + 3;
            }
        }

        // 帮助栏
        let help = "操作: s 设为当前   d 删除   n/p 下一/上一条   ← → 切换按钮   Esc 返回";
        frame.put_str(
            Rect::new(2, frame.area.height.saturating_sub(2), frame.area.width.saturating_sub(4), 1),
            help,
            theme::DIM,
            attr::NONE,
        );
    }

    fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return Outcome::Continue;
        }
        match key.code {
            KeyCode::Esc => Outcome::Pop,
            KeyCode::Left => {
                if self.action_cursor == 0 {
                    self.action_cursor = 2;
                } else {
                    self.action_cursor -= 1;
                }
                Outcome::Continue
            }
            KeyCode::Right => {
                self.action_cursor = (self.action_cursor + 1) % 3;
                Outcome::Continue
            }
            KeyCode::Up | KeyCode::Char('p') => {
                if self.records.is_empty() {
                    return Outcome::Continue;
                }
                if self.cursor == 0 {
                    self.cursor = self.records.len() - 1;
                } else {
                    self.cursor -= 1;
                }
                Outcome::Continue
            }
            KeyCode::Down | KeyCode::Char('n') => {
                if self.records.is_empty() {
                    return Outcome::Continue;
                }
                self.cursor = (self.cursor + 1) % self.records.len();
                Outcome::Continue
            }
            KeyCode::Char('s') => self.switch_active(),
            KeyCode::Char('d') => {
                if let Some(r) = self.current().cloned() {
                    Outcome::Push(Box::new(ProviderDelPicker::new(self.db.clone(), self.paths.clone(), r.id)))
                } else {
                    Outcome::Continue
                }
            }
            KeyCode::Enter => match self.action_cursor {
                0 => self.switch_active(),
                1 => {
                    if let Some(r) = self.current().cloned() {
                        Outcome::Push(Box::new(ProviderDelPicker::new(
                            self.db.clone(),
                            self.paths.clone(),
                            r.id,
                        )))
                    } else {
                        Outcome::Continue
                    }
                }
                _ => Outcome::Pop,
            },
            _ => Outcome::Continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Protocol;
    use tempfile::tempdir;

    /// F-007-3 智能截断 end_point —— 不依赖任何状态,直接验证纯函数分支。
    #[test]
    fn truncate_endpoint_short_url_unchanged() {
        assert_eq!(
            truncate_endpoint("http://127.0.0.1:18905", 72),
            "http://127.0.0.1:18905"
        );
    }

    #[test]
    fn truncate_endpoint_long_url_with_path() {
        let long = "https://api.anthropic.com/v1/messages/very/long/path/segment";
        let out = truncate_endpoint(long, 30);
        assert!(out.chars().count() <= 30, "应 ≤ max_chars: {out}");
        assert!(out.contains("…"), "应包含省略号: {out}");
        assert!(
            out.ends_with("path/segment") || out.ends_with("path/segmen…"),
            "应保留 path 末两段: {out}"
        );
    }

    #[test]
    fn truncate_endpoint_no_path() {
        let long = "https://very-very-long-host-name.example.com:8080";
        let out = truncate_endpoint(long, 24);
        assert!(out.chars().count() <= 24, "应 ≤ max_chars: {out}");
        assert!(out.contains("…"), "应包含省略号: {out}");
    }

    #[test]
    fn list_renders_empty() {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        let screen = ProviderList::new(Arc::new(Mutex::new(db)), paths);
        assert!(screen.records.is_empty());
    }

    #[test]
    fn list_with_records() {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        db.add(Protocol::Anthropic, "p1", "m1", "https://x", "k1234").unwrap();
        let screen = ProviderList::new(Arc::new(Mutex::new(db)), paths);
        assert_eq!(screen.records.len(), 1);
        assert_eq!(screen.field_value(0), "1");
        assert_eq!(screen.field_value(5), "****1234");
    }
}
