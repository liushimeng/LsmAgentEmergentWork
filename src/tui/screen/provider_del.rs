//! ProviderDel 屏 —— Picker(选记录) → Confirm(二次确认) → 删除。

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Attribute;

use crate::config::{Db, Paths, ProviderRecord};
use crate::tui::engine::{Frame, Outcome, Rect, Screen};
use crate::tui::theme;

/// 选择屏：列出所有记录,高亮 cursor 指向的那条;Enter 进 Confirm。
pub struct ProviderDelPicker {
    pub records: Vec<ProviderRecord>,
    pub cursor: usize,
    pub db: Arc<Mutex<Db>>,
    pub paths: Paths,
}

impl ProviderDelPicker {
    pub fn new(db: Arc<Mutex<Db>>, paths: Paths, _seed_id: i64) -> Self {
        let records = db.lock().expect("db").list().unwrap_or_default();
        let cursor = records.iter().position(|r| r.id == _seed_id).unwrap_or(0);
        Self { records, cursor, db, paths }
    }
}

impl Screen for ProviderDelPicker {
    fn title(&self) -> &str {
        "/provider del"
    }

    fn render(&self, frame: &mut Frame) {
        frame.border_box(
            Rect::new(0, 0, frame.area.width, frame.area.height),
            Some(self.title()),
        );
        let header = "请选择要删除的接入记录(↑↓ 选择, Enter 进入确认页, Esc 取消):";
        frame.put_str(
            Rect::new(2, 1, frame.area.width.saturating_sub(4), 1),
            header,
            theme::DIM,
            Attribute::Reset,
        );

        if self.records.is_empty() {
            let area = Rect::new(4, 4, frame.area.width.saturating_sub(8), 1);
            frame.put_str(area, "(空)没有可删除的接入记录。", theme::FG, Attribute::Reset);
            return;
        }

        let top = 3u16;
        for (i, r) in self.records.iter().enumerate() {
            let y = top + i as u16;
            if y + 1 >= frame.area.height.saturating_sub(3) {
                break;
            }
            let marker = if i == self.cursor { "►" } else { " " };
            let line = format!(
                "{} id={}  [{}]  {}/{}  @ {}  key={}",
                marker,
                r.id,
                r.protocol.as_str(),
                r.provider_name,
                r.model_name,
                r.end_point,
                theme::mask_key(&r.api_key),
            );
            let attr = if i == self.cursor {
                Attribute::Reverse
            } else {
                Attribute::Reset
            };
            frame.put_str(
                Rect::new(4, y, frame.area.width.saturating_sub(8), 1),
                &line,
                theme::FG,
                attr,
            );
        }

        let help = "↑↓ 选择   Enter 进入确认页   Esc 返回";
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
            KeyCode::Up => {
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
            KeyCode::Down => {
                if self.records.is_empty() {
                    return Outcome::Continue;
                }
                self.cursor = (self.cursor + 1) % self.records.len();
                Outcome::Continue
            }
            KeyCode::Enter => {
                if let Some(r) = self.records.get(self.cursor).cloned() {
                    Outcome::Push(Box::new(ProviderDelConfirm::new(self.db.clone(), self.paths.clone(), r)))
                } else {
                    Outcome::Continue
                }
            }
            _ => Outcome::Continue,
        }
    }
}

/// 二次确认屏。
pub struct ProviderDelConfirm {
    pub target: ProviderRecord,
    pub cursor: usize, // 0 = 确认删除, 1 = 取消
    pub db: Arc<Mutex<Db>>,
    pub paths: Paths,
}

impl ProviderDelConfirm {
    pub fn new(db: Arc<Mutex<Db>>, paths: Paths, target: ProviderRecord) -> Self {
        Self { target, cursor: 1, db, paths }
    }
}

impl Screen for ProviderDelConfirm {
    fn title(&self) -> &str {
        "/provider del > 确认"
    }

    fn render(&self, frame: &mut Frame) {
        frame.border_box(
            Rect::new(0, 0, frame.area.width, frame.area.height),
            Some(self.title()),
        );

        let header = "确认删除以下接入记录?此操作不可撤销:";
        frame.put_str(
            Rect::new(2, 1, frame.area.width.saturating_sub(4), 1),
            header,
            theme::DIM,
            Attribute::Reset,
        );

        let r = &self.target;
        let line1 = format!("  id       : {}", r.id);
        let line2 = format!("  protocol : {}", r.protocol.as_str());
        let line3 = format!("  provider : {}", r.provider_name);
        let line4 = format!("  model    : {}", r.model_name);
        let line5 = format!("  endpoint : {}", r.end_point);
        let line6 = format!("  api_key  : {}", theme::mask_key(&r.api_key));

        let top = 3u16;
        for (i, line) in [line1, line2, line3, line4, line5, line6].iter().enumerate() {
            let y = top + i as u16;
            if y + 1 >= frame.area.height.saturating_sub(3) {
                break;
            }
            frame.put_str(
                Rect::new(4, y, frame.area.width.saturating_sub(8), 1),
                line,
                theme::FG,
                Attribute::Reset,
            );
        }

        // 按钮
        let button_y = frame.area.height.saturating_sub(4);
        let confirm_label = "[ 确认删除 ]";
        let cancel_label = "[ 取消 ]";
        let confirm_attr = if self.cursor == 0 {
            Attribute::Reverse
        } else {
            Attribute::Reset
        };
        let cancel_attr = if self.cursor == 1 {
            Attribute::Reverse
        } else {
            Attribute::Reset
        };
        frame.put_str(
            Rect::new(4, button_y, confirm_label.chars().count() as u16, 1),
            confirm_label,
            theme::FG,
            confirm_attr,
        );
        frame.put_str(
            Rect::new(4 + confirm_label.chars().count() as u16 + 4, button_y, cancel_label.chars().count() as u16, 1),
            cancel_label,
            theme::FG,
            cancel_attr,
        );

        let help = "← → 切换按钮   Enter 触发   Esc 取消";
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
                self.cursor = if self.cursor == 0 { 1 } else { 0 };
                Outcome::Continue
            }
            KeyCode::Right => {
                self.cursor = if self.cursor == 0 { 1 } else { 0 };
                Outcome::Continue
            }
            KeyCode::Enter => {
                if self.cursor == 0 {
                    let id = self.target.id;
                    let res = self.db.lock().expect("db").delete(id);
                    match res {
                        Ok(_) => Outcome::Toast(format!("✓ 已删除 id={id}")),
                        Err(e) => Outcome::Toast(format!("! 删除失败: {e}")),
                    }
                } else {
                    Outcome::Pop
                }
            }
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
    fn picker_with_records() {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        let id = db.add(Protocol::Anthropic, "p", "m", "https://x", "k1234").unwrap();
        let picker = ProviderDelPicker::new(Arc::new(Mutex::new(db)), paths, id);
        assert_eq!(picker.records.len(), 1);
        assert_eq!(picker.cursor, 0);
    }

    #[test]
    fn confirm_default_cancel() {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        let id = db.add(Protocol::Anthropic, "p", "m", "https://x", "k1234").unwrap();
        let r = db.get(id).unwrap();
        let confirm = ProviderDelConfirm::new(Arc::new(Mutex::new(db)), paths, r);
        // 默认 cursor=1 (取消);防止误删
        assert_eq!(confirm.cursor, 1);
    }
}