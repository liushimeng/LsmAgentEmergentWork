//! ProviderForm 屏 —— /provider add 的 Tab 表单实现。

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};

use crate::config::{Db, Paths, Protocol, ProviderRecord};
use crate::tui::engine::{Frame, Outcome, Rect, Screen};
use crate::tui::form::{ConfirmAction, FormOutcome, Tab, TabForm};
use crate::tui::theme::{self, attr};

/// ProviderForm 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Add,
    Edit(i64),
}

/// 提交回调(add 完成后由主屏调用以重建 Agent / 切换 use)。
pub type OnDone = Box<dyn FnMut(i64) + Send>;

pub struct ProviderForm {
    pub mode: Mode,
    pub form: TabForm,
    pub db: Arc<Mutex<Db>>,
    pub paths: Paths,
    pub error: Option<String>,
    pub on_done: Option<OnDone>,
}

impl ProviderForm {
    pub fn new_add(db: Arc<Mutex<Db>>, paths: Paths, on_done: OnDone) -> Self {
        Self {
            mode: Mode::Add,
            form: TabForm::new(Self::default_tabs(None, None, None, None, None)),
            db,
            paths,
            error: None,
            on_done: Some(on_done),
        }
    }

    pub fn new_edit(
        db: Arc<Mutex<Db>>,
        paths: Paths,
        record: ProviderRecord,
        on_done: OnDone,
    ) -> Self {
        Self {
            mode: Mode::Edit(record.id),
            form: TabForm::new(Self::default_tabs(
                Some(record.protocol),
                Some(&record.provider_name),
                Some(&record.model_name),
                Some(&record.end_point),
                Some(&record.api_key),
            )),
            db,
            paths,
            error: None,
            on_done: Some(on_done),
        }
    }

    fn default_tabs(
        proto: Option<Protocol>,
        provider_name: Option<&str>,
        model_name: Option<&str>,
        end_point: Option<&str>,
        api_key: Option<&str>,
    ) -> Vec<Tab> {
        let proto_idx = match proto.unwrap_or(Protocol::Anthropic) {
            Protocol::Anthropic => 0,
            Protocol::OpenAi => 1,
        };
        vec![
            Tab::choice(
                "protocol",
                vec!["anthropic".into(), "openai".into()],
                proto_idx,
            ),
            Tab::text("provider_name", "必填", false, provider_name.unwrap_or("")),
            Tab::text("model_name", "必填", false, model_name.unwrap_or("")),
            Tab::text("end_point", "https://...", false, end_point.unwrap_or("")),
            Tab::text("api_key", "sk-...", true, api_key.unwrap_or("")),
            Tab::text(
                "context_max_size",
                "800000 / 800K / 1M(0=不限)",
                false,
                &crate::config::DEFAULT_CONTEXT_MAX_SIZE.to_string(),
            ),
            Tab::confirm("确认", vec![ConfirmAction::Submit, ConfirmAction::Cancel]),
        ]
    }

    fn validate(&self) -> Result<(Protocol, String, String, String, String, u64), String> {
        let p = self.parse_protocol()?;
        let provider_name = self.form.tabs[1].value.trim().to_string();
        if provider_name.is_empty() {
            return Err("provider_name 不能为空".into());
        }
        let model_name = self.form.tabs[2].value.trim().to_string();
        if model_name.is_empty() {
            return Err("model_name 不能为空".into());
        }
        let end_point = self.form.tabs[3].value.trim().to_string();
        if end_point.is_empty()
            || !(end_point.starts_with("http://") || end_point.starts_with("https://"))
        {
            return Err("end_point 必须以 http:// 或 https:// 开头".into());
        }
        let api_key = self.form.tabs[4].value.clone();
        if api_key.is_empty() {
            return Err("api_key 不能为空".into());
        }
        let ctx_raw = self.form.tabs[5].value.trim().to_string();
        let context_max_size = if ctx_raw.is_empty() {
            crate::config::DEFAULT_CONTEXT_MAX_SIZE
        } else {
            crate::config::parse_context_size(&ctx_raw)?
        };
        Ok((
            p,
            provider_name,
            model_name,
            end_point,
            api_key,
            context_max_size,
        ))
    }

    fn parse_protocol(&self) -> Result<Protocol, String> {
        let s = &self.form.tabs[0].value;
        Protocol::parse(s).map_err(|e| e.to_string())
    }

    fn submit(&mut self) -> Outcome {
        match self.validate() {
            Err(e) => {
                self.error = Some(e);
                Outcome::Continue
            }
            Ok((p, pn, mn, ep, ak, ctx)) => {
                let db = self.db.clone();
                let res = match self.mode {
                    Mode::Add => {
                        let guard = db.lock().expect("db");
                        guard.add_with_context(p, &pn, &mn, &ep, &ak, Some(ctx))
                    }
                    Mode::Edit(id) => {
                        // Edit 模式暂未通过 TUI 触发;这里走 set_active/use 路径即可
                        // Edit 完整 CRUD 需要新增 db.update;为最小可用性,只支持 Add
                        let _ = id;
                        return self.toast("Edit 模式尚未实现;本次仅 Add 生效".into());
                    }
                };
                match res {
                    Ok(new_id) => {
                        if let Some(cb) = self.on_done.as_mut() {
                            cb(new_id);
                        }
                        let msg = format!("✓ 已新增接入记录 id={new_id}");
                        self.toast(msg)
                    }
                    Err(e) => {
                        self.error = Some(format!("写入失败: {e}"));
                        Outcome::Continue
                    }
                }
            }
        }
    }

    fn toast(&self, msg: String) -> Outcome {
        Outcome::Toast(msg)
    }
}

impl Screen for ProviderForm {
    fn title(&self) -> &str {
        match self.mode {
            Mode::Add => "/provider add",
            Mode::Edit(_) => "/provider edit",
        }
    }

    fn render(&self, frame: &mut Frame) {
        // 顶部边框 + 标题
        let outer = Rect::new(0, 0, frame.area.width, frame.area.height);
        frame.border_box(outer, Some(self.title()));

        // 顶部提示
        let hint = "← → 切换 Tab   Enter 进入编辑   Esc 返回";
        frame.put_str(
            Rect::new(
                2,
                frame.area.height.saturating_sub(2),
                frame.area.width.saturating_sub(4),
                1,
            ),
            hint,
            theme::DIM,
            attr::NONE,
        );

        // Tab 列表
        let list_top = 2u16;
        let row_height = 2u16;
        for (i, tab) in self.form.tabs.iter().enumerate() {
            let y = list_top + (i as u16) * row_height;
            if y + 1 >= frame.area.height.saturating_sub(2) {
                break;
            }
            let focused = self.form.focus == i;
            let editing = self.form.is_editing(i);

            // label_area 宽度从 16 调为 24,完整容纳 `context_max_size`(16字符)
            // + `▸ ` 前缀(2字符,▸ 宽度 2)+ `N. ` 序号与点(3字符)+ 空格 1,
            // 共 22 字符;24 给一个余量列避免边缘截断。
            // value_area x 起点同步右移到 26(2+24),保证 label 与 value 间有 2 列间隔。
            let label_area = Rect::new(2, y, 24, 1);
            let value_area = Rect::new(26, y, frame.area.width.saturating_sub(28), 1);
            let (label_fg, label_attrs, label_prefix) = if focused {
                (
                    theme::TAB_FOCUSED_FG,
                    theme::TAB_FOCUSED_ATTRS,
                    theme::TAB_FOCUSED_PREFIX,
                )
            } else {
                (theme::DIM, attr::NONE, "  ")
            };
            let label = format!("{}{}. {}", label_prefix, i + 1, tab.label);
            frame.put_str(label_area, &label, label_fg, label_attrs);

            // 值
            match &tab.kind {
                crate::tui::form::TabKind::Choice { choices, cursor } => {
                    let cur = *cursor;
                    let mut line = String::new();
                    for (j, ch) in choices.iter().enumerate() {
                        if j == cur && focused {
                            line.push_str(&format!("[ {} ]", ch));
                        } else {
                            line.push_str(&format!("  {}  ", ch));
                        }
                    }
                    let (fg, attrs) = if focused {
                        (theme::ACCENT, theme::CHOICE_FOCUSED_ATTRS)
                    } else {
                        (theme::FG, attr::NONE)
                    };
                    frame.put_str(value_area, &line, fg, attrs);
                }
                crate::tui::form::TabKind::Confirm { actions, cursor } => {
                    let cur = *cursor;
                    let mut line = String::new();
                    for (j, a) in actions.iter().enumerate() {
                        let s = a.label();
                        if j == cur && focused {
                            line.push_str(&format!(
                                "{}{}{}",
                                theme::SELECTED_BUTTON_L,
                                s,
                                theme::SELECTED_BUTTON_R
                            ));
                            line.push_str("  ");
                        } else {
                            line.push_str(&format!("  {s}  "));
                        }
                    }
                    let (fg, attrs) = if focused {
                        (theme::SELECTED_FG, theme::SELECTED_ATTRS)
                    } else {
                        (theme::FG, attr::NONE)
                    };
                    frame.put_str(value_area, &line, fg, attrs);
                }
                crate::tui::form::TabKind::Text { masked, .. } => {
                    let display = if *masked && !editing {
                        theme::mask_key(&tab.value)
                    } else if tab.value.is_empty() {
                        // placeholder
                        match i {
                            1 => "<必填>".to_string(),
                            2 => "<必填>".to_string(),
                            3 => "<https://...>".to_string(),
                            4 => "<sk-...>".to_string(),
                            5 => "<800000 / 800K / 1M>".to_string(),
                            _ => String::new(),
                        }
                    } else {
                        tab.value.clone()
                    };
                    let (fg, attrs, wrapped) = if focused && editing {
                        (
                            theme::SELECTED_FG,
                            theme::SELECTED_ATTRS,
                            format!(
                                "{}{}{}",
                                theme::SELECTED_BUTTON_L,
                                display,
                                theme::SELECTED_BUTTON_R
                            ),
                        )
                    } else if focused {
                        (theme::TAB_FOCUSED_FG, theme::TAB_FOCUSED_ATTRS, display)
                    } else {
                        (theme::FG, attr::NONE, display)
                    };
                    frame.put_str(value_area, &wrapped, fg, attrs);
                }
            }
        }

        // 错误提示
        if let Some(e) = &self.error {
            let area = Rect::new(
                2,
                frame.area.height.saturating_sub(4),
                frame.area.width.saturating_sub(4),
                1,
            );
            frame.put_str(area, &format!("! {e}"), theme::ERROR, attr::BOLD);
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        // Ctrl-C: 强退
        if matches!(key.code, KeyCode::Char('c'))
            && key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            return Outcome::Pop;
        }

        let outcome = self.form.handle_key(key);
        self.error = None;
        match outcome {
            FormOutcome::Continue => Outcome::Continue,
            FormOutcome::Submit => self.submit(),
            FormOutcome::Cancel => Outcome::Pop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;
    use tempfile::tempdir;

    fn fresh() -> (Arc<Mutex<Db>>, Paths, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (Arc::new(Mutex::new(db)), paths, dir)
    }

    #[test]
    fn add_form_validates() {
        let (db, paths, _d) = fresh();
        let mut form = ProviderForm::new_add(db.clone(), paths, Box::new(|_| {}));
        // 试图在 end_point 为空时提交
        let err = form.validate();
        assert!(err.is_err());
    }
}
