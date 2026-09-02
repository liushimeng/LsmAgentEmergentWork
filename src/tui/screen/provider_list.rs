//! ProviderList 屏 —— /provider list 的 Tab 化展示。
//!
//! Tab 顺序：
//! 1..=5. 5 个只读字段 (id / protocol / provider_name / model_name / end_point / api_key)
//! 6. 操作按钮组 [Switch] [Delete] [Back]

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Attribute;

use crate::config::{Db, Paths, ProviderRecord};
use crate::tui::engine::{Frame, Outcome, Rect, Screen};
use crate::tui::screen::provider_del::ProviderDelPicker;
use crate::tui::theme;

const FIELD_LABELS: [&str; 6] = ["id", "protocol", "provider_name", "model_name", "end_point", "api_key"];

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
            4 => r.end_point.clone(),
            5 => theme::mask_key(&r.api_key),
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
            Attribute::Reset,
        );

        if self.records.is_empty() {
            let area = Rect::new(4, 4, frame.area.width.saturating_sub(8), 1);
            frame.put_str(area, "(空)尚未配置任何接入记录,使用 /provider add 新增。", theme::FG, Attribute::Reset);
        } else {
            // 字段 Tab 列表
            let top = 3u16;
            for (i, label) in FIELD_LABELS.iter().enumerate() {
                let y = top + i as u16;
                if y + 1 >= frame.area.height.saturating_sub(3) {
                    break;
                }
                let focused = self.action_cursor == 99 && false; // 字段不参与按钮焦点
                let _ = focused;
                let label_area = Rect::new(2, y, 18, 1);
                let value_area = Rect::new(22, y, frame.area.width.saturating_sub(24), 1);
                frame.put_str(label_area, &format!("{}:", label), theme::ACCENT, Attribute::Reset);
                frame.put_str(value_area, &self.field_value(i), theme::FG, Attribute::Reset);
            }

            // 操作按钮
            let action_y = frame.area.height.saturating_sub(4);
            let labels = ["[ 设为当前 s ]", "[ 删除 d ]", "[ 返回 Esc ]"];
            let mut x = 2u16;
            for (i, l) in labels.iter().enumerate() {
                let attr = if self.action_cursor == i {
                    Attribute::Reverse
                } else {
                    Attribute::Reset
                };
                let area = Rect::new(x, action_y, l.chars().count() as u16 + 2, 1);
                frame.put_str(area, l, theme::FG, attr);
                x += l.chars().count() as u16 + 4;
            }
        }

        // 帮助栏
        let help = "操作: s 设为当前   d 删除   n/p 下一/上一条   ← → 切换按钮   Esc 返回";
        frame.put_str(
            Rect::new(2, frame.area.height.saturating_sub(2), frame.area.width.saturating_sub(4), 1),
            help,
            theme::DIM,
            Attribute::Reset,
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