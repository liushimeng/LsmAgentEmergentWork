//! 通用 Tab 表单状态机 —— 被 ProviderForm 屏使用;也可被未来 `/settings` 等复用。
//!
//! 设计见 `docs/TUI界面与CLI渲染引擎/02-技术设计.md` §5 与
//! `03-Tab表单与Provider操作设计.md` §1-§2。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::theme::{self, BUTTON_FOCUSED};

/// 单个 Tab 的种类。
#[derive(Debug, Clone)]
pub enum TabKind {
    /// 单行文本输入。
    Text {
        placeholder: String,
        /// 编辑态是否明文显示;非编辑态永远脱敏。
        masked: bool,
    },
    /// 二选一 / 多选一(Enter 切换)。
    Choice {
        choices: Vec<String>,
        cursor: usize,
    },
    /// 确认按钮组(由左右键切换子按钮)。
    Confirm {
        actions: Vec<ConfirmAction>,
        cursor: usize,
    },
}

/// 确认按钮种类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    Submit,   // [ 确认 ]
    Cancel,   // [ 取消 ]
    Delete,   // [ 确认删除 ]
    Switch,   // [ 设为当前 ]
}

impl ConfirmAction {
    pub fn label(&self) -> &'static str {
        match self {
            ConfirmAction::Submit => "[ 确认 ]",
            ConfirmAction::Cancel => "[ 取消 ]",
            ConfirmAction::Delete => "[ 确认删除 ]",
            ConfirmAction::Switch => "[ 设为当前 ]",
        }
    }
}

/// 单个 Tab。
#[derive(Debug, Clone)]
pub struct Tab {
    pub label: String,
    pub kind: TabKind,
    /// 当前值(对 Choice 而言是 choices[cursor] 的一份字符串快照)。
    pub value: String,
}

impl Tab {
    pub fn text(label: &str, placeholder: &str, masked: bool, initial: &str) -> Self {
        Self {
            label: label.to_string(),
            kind: TabKind::Text { placeholder: placeholder.to_string(), masked },
            value: initial.to_string(),
        }
    }

    pub fn choice(label: &str, choices: Vec<String>, initial: usize) -> Self {
        let value = choices.get(initial).cloned().unwrap_or_default();
        Self {
            label: label.to_string(),
            kind: TabKind::Choice { choices, cursor: initial },
            value,
        }
    }

    pub fn confirm(label: &str, actions: Vec<ConfirmAction>) -> Self {
        Self {
            label: label.to_string(),
            kind: TabKind::Confirm { actions, cursor: 0 },
            value: String::new(),
        }
    }
}

/// 表单处理结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormOutcome {
    Continue,
    Submit,    // [ 确认 ] / [ 确认删除 ] / [ 设为当前 ]
    Cancel,    // [ 取消 ]
}

/// Tab 表单状态机。
#[derive(Debug, Clone)]
pub struct TabForm {
    pub tabs: Vec<Tab>,
    pub focus: usize,
    /// 处于编辑态的 Tab 下标(None = 浏览态)。
    pub editing: Option<usize>,
}

impl TabForm {
    pub fn new(tabs: Vec<Tab>) -> Self {
        Self { tabs, focus: 0, editing: None }
    }

    pub fn current(&self) -> &Tab {
        &self.tabs[self.focus]
    }

    pub fn current_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.focus]
    }

    /// 处理 KeyEvent;返回是否提交 / 取消。
    pub fn handle_key(&mut self, key: KeyEvent) -> FormOutcome {
        match self.editing {
            Some(idx) if idx == self.focus => self.handle_editing_key(key),
            _ => self.handle_browse_key(key),
        }
    }

    fn handle_browse_key(&mut self, key: KeyEvent) -> FormOutcome {
        let n = self.tabs.len();
        match key.code {
            KeyCode::Left | KeyCode::BackTab => {
                self.focus = (self.focus + n - 1) % n;
                FormOutcome::Continue
            }
            KeyCode::Right | KeyCode::Tab => {
                self.focus = (self.focus + 1) % n;
                FormOutcome::Continue
            }
            KeyCode::Enter => {
                // 先 clone 出 kind,避免与后续 &mut self.tabs 冲突
                let kind = self.tabs[self.focus].kind.clone();
                match &kind {
                    TabKind::Text { .. } => {
                        self.editing = Some(self.focus);
                        FormOutcome::Continue
                    }
                    TabKind::Choice { choices, cursor } => {
                        if !choices.is_empty() {
                            let next = (*cursor + 1) % choices.len();
                            let new_value = choices[next].clone();
                            self.tabs[self.focus].kind = TabKind::Choice {
                                choices: choices.clone(),
                                cursor: next,
                            };
                            self.tabs[self.focus].value = new_value;
                        }
                        FormOutcome::Continue
                    }
                    TabKind::Confirm { actions, cursor } => {
                        let action = actions[*cursor].clone();
                        match action {
                            ConfirmAction::Submit | ConfirmAction::Delete | ConfirmAction::Switch => {
                                FormOutcome::Submit
                            }
                            ConfirmAction::Cancel => FormOutcome::Cancel,
                        }
                    }
                }
            }
            KeyCode::Esc => FormOutcome::Cancel,
            _ => FormOutcome::Continue,
        }
    }

    fn handle_editing_key(&mut self, key: KeyEvent) -> FormOutcome {
        let kind = std::mem::replace(&mut self.tabs[self.focus].kind, TabKind::Text {
            placeholder: String::new(),
            masked: false,
        });
        let outcome = match &kind {
            TabKind::Text { .. } => self.edit_text(key),
            TabKind::Choice { .. } => self.edit_choice(key),
            TabKind::Confirm { actions, cursor } => self.edit_confirm(key, actions.clone(), *cursor),
        };
        // 写回 kind(可能被 edit_choice / edit_confirm 修改)
        self.tabs[self.focus].kind = kind;
        outcome
    }

    fn edit_text(&mut self, key: KeyEvent) -> FormOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.editing = None;
                FormOutcome::Continue
            }
            KeyCode::Backspace => {
                if !self.tabs[self.focus].value.is_empty() {
                    self.tabs[self.focus].value.pop();
                }
                FormOutcome::Continue
            }
            KeyCode::Delete => {
                // 简化:同 Backspace(末尾删除)
                if !self.tabs[self.focus].value.is_empty() {
                    self.tabs[self.focus].value.pop();
                }
                FormOutcome::Continue
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    return FormOutcome::Continue;
                }
                self.tabs[self.focus].value.push(c);
                FormOutcome::Continue
            }
            _ => FormOutcome::Continue,
        }
    }

    fn edit_choice(&mut self, key: KeyEvent) -> FormOutcome {
        let n = match &self.tabs[self.focus].kind {
            TabKind::Choice { choices, .. } => choices.len(),
            _ => 0,
        };
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.editing = None;
                FormOutcome::Continue
            }
            KeyCode::Left | KeyCode::Up => {
                if let TabKind::Choice { choices, cursor } = &mut self.tabs[self.focus].kind {
                    let next = if *cursor == 0 { n - 1 } else { *cursor - 1 };
                    *cursor = next;
                    self.tabs[self.focus].value = choices[next].clone();
                }
                FormOutcome::Continue
            }
            KeyCode::Right | KeyCode::Down => {
                if let TabKind::Choice { choices, cursor } = &mut self.tabs[self.focus].kind {
                    let next = (*cursor + 1) % n;
                    *cursor = next;
                    self.tabs[self.focus].value = choices[next].clone();
                }
                FormOutcome::Continue
            }
            _ => FormOutcome::Continue,
        }
    }

    fn edit_confirm(&mut self, key: KeyEvent, actions: Vec<ConfirmAction>, cursor: usize) -> FormOutcome {
        match key.code {
            KeyCode::Esc => {
                self.editing = None;
                FormOutcome::Continue
            }
            KeyCode::Left => {
                if let TabKind::Confirm { actions, cursor } = &mut self.tabs[self.focus].kind {
                    let n = actions.len();
                    *cursor = if *cursor == 0 { n - 1 } else { *cursor - 1 };
                }
                FormOutcome::Continue
            }
            KeyCode::Right => {
                if let TabKind::Confirm { actions, cursor } = &mut self.tabs[self.focus].kind {
                    let n = actions.len();
                    *cursor = (*cursor + 1) % n;
                }
                FormOutcome::Continue
            }
            KeyCode::Enter => {
                let act = actions[cursor].clone();
                self.editing = None;
                match act {
                    ConfirmAction::Cancel => FormOutcome::Cancel,
                    _ => FormOutcome::Submit,
                }
            }
            _ => FormOutcome::Continue,
        }
    }

    /// 当前焦点 Tab 是否处于编辑态。
    pub fn is_editing(&self, idx: usize) -> bool {
        self.editing == Some(idx)
    }

    /// 渲染：把每个 Tab 写成一行(由 ProviderForm 屏调用;这里仅做数据 → 文本)。
    pub fn display_value(&self, idx: usize) -> String {
        let tab = &self.tabs[idx];
        match &tab.kind {
            TabKind::Text { masked: true, .. } => {
                if self.is_editing(idx) {
                    tab.value.clone()
                } else {
                    theme::mask_key(&tab.value)
                }
            }
            _ => tab.value.clone(),
        }
    }

    /// 当前按钮是否被聚焦(用于决定是否渲染反白)。
    pub fn button_attr(&self) -> crossterm::style::Attribute {
        if self.editing == Some(self.focus) {
            BUTTON_FOCUSED
        } else {
            crossterm::style::Attribute::Reset
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn k(c: char) -> KeyEvent {
        KeyEvent::new(crossterm::event::KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn kcode(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn navigate_tabs() {
        let form = TabForm::new(vec![
            Tab::text("a", "", false, ""),
            Tab::text("b", "", false, ""),
            Tab::text("c", "", false, ""),
        ]);
        let mut f = form;
        f.handle_key(kcode(KeyCode::Right));
        assert_eq!(f.focus, 1);
        f.handle_key(kcode(KeyCode::Right));
        assert_eq!(f.focus, 2);
        f.handle_key(kcode(KeyCode::Right));
        assert_eq!(f.focus, 0); // 环回
        f.handle_key(kcode(KeyCode::Left));
        assert_eq!(f.focus, 2); // 反向环回
    }

    #[test]
    fn enter_text_then_type() {
        let mut f = TabForm::new(vec![Tab::text("name", "", false, "")]);
        assert_eq!(f.handle_key(kcode(KeyCode::Enter)), FormOutcome::Continue);
        assert_eq!(f.editing, Some(0));
        f.handle_key(k('a'));
        f.handle_key(k('b'));
        assert_eq!(f.tabs[0].value, "ab");
        f.handle_key(kcode(KeyCode::Enter));
        assert_eq!(f.editing, None);
        assert_eq!(f.tabs[0].value, "ab");
    }

    #[test]
    fn choice_toggles() {
        let mut f = TabForm::new(vec![Tab::choice(
            "p",
            vec!["anthropic".into(), "openai".into()],
            0,
        )]);
        assert_eq!(f.tabs[0].value, "anthropic");
        f.handle_key(kcode(KeyCode::Enter));
        assert_eq!(f.tabs[0].value, "openai");
    }

    #[test]
    fn confirm_cancel() {
        let mut f = TabForm::new(vec![Tab::confirm(
            "ok",
            vec![ConfirmAction::Submit, ConfirmAction::Cancel],
        )]);
        assert_eq!(f.handle_key(kcode(KeyCode::Enter)), FormOutcome::Submit);
    }
}