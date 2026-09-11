//! /provider 子屏桥接(自 tui/mod.rs 拆分,2026-09-11,单文件 ≤1800 行规范)。
//!
//! 职责:把 /provider list|add|del 命令接入 engine 的 Screen 栈
//! (alternate screen + raw mode),Toast 在 leave_alt 之后回主屏输出;
//! 非 TTY(管道 / e2e)回退 print 输出,保证 run_e2e.sh 兼容。

use anyhow::Result;

use super::TuiSession;
use super::atty;

impl TuiSession {
    /// 进入 ProviderList 屏(子屏通过 engine 渲染)。
    /// 当 stdin 不是 TTY(如 e2e 管道)时,回退到 print 输出以保持兼容。
    pub(crate) async fn run_provider_list_screen(&mut self) -> Result<()> {
        use crate::tui::engine::{enter_alt, leave_alt};
        use crate::tui::screen::provider_list::ProviderList;

        if !atty() {
            // 非 TTY:回退到 print 输出
            return self.list_providers();
        }

        let screen: Box<dyn crate::tui::engine::Screen> =
            Box::new(ProviderList::new(self.db.clone(), self.paths.clone()));
        enter_alt().map_err(anyhow::Error::from)?;
        let toast = Self::run_screen_loop(screen).await?;
        leave_alt().map_err(anyhow::Error::from)?;
        // Toast 必须在 leave_alt 之后输出,否则会被 alternate screen 切换吞掉
        // (第三十轮 Bug 修复)
        if let Some(msg) = toast {
            println!("  {msg}");
        }
        // 重建 YoloRunner(可能切换了 use)
        self.rebuild_orchestrator()?;
        Ok(())
    }

    pub(crate) async fn run_provider_add_screen(&mut self) -> Result<()> {
        use crate::tui::engine::{enter_alt, leave_alt};
        use crate::tui::screen::provider_form::ProviderForm;

        if !atty() {
            return self.add_provider_interactive().map(|_| ());
        }

        let db = self.db.clone();
        let on_done = Box::new(move |id: i64| {
            let _ = db.lock().expect("db").set_active(id);
        });
        let screen: Box<dyn crate::tui::engine::Screen> = Box::new(ProviderForm::new_add(
            self.db.clone(),
            self.paths.clone(),
            on_done,
        ));
        enter_alt().map_err(anyhow::Error::from)?;
        let toast = Self::run_screen_loop(screen).await?;
        leave_alt().map_err(anyhow::Error::from)?;
        // Toast 必须在 leave_alt 之后输出,否则会被 alternate screen 切换吞掉
        if let Some(msg) = toast {
            println!("  {msg}");
        }
        self.rebuild_orchestrator()?;
        Ok(())
    }

    pub(crate) async fn run_provider_del_screen(&mut self) -> Result<()> {
        use crate::tui::engine::{enter_alt, leave_alt};
        use crate::tui::screen::provider_del::ProviderDelPicker;

        if !atty() {
            // 非 TTY:提示使用 CLI 子命令
            println!("  非交互模式请使用: laew provider del <id>");
            return Ok(());
        }

        let screen: Box<dyn crate::tui::engine::Screen> = Box::new(ProviderDelPicker::new(
            self.db.clone(),
            self.paths.clone(),
            -1,
        ));
        enter_alt().map_err(anyhow::Error::from)?;
        let toast = Self::run_screen_loop(screen).await?;
        leave_alt().map_err(anyhow::Error::from)?;
        // Toast 必须在 leave_alt 之后输出,否则会被 alternate screen 切换吞掉
        if let Some(msg) = toast {
            println!("  {msg}");
        }
        self.rebuild_orchestrator()?;
        Ok(())
    }

    /// 通用子屏循环:渲染 → 读键 → 处理 Outcome。
    /// 屏幕栈:`Vec<Box<dyn Screen>>`,top 是当前屏;`Push` 压栈、`Pop` 出栈。
    /// 当栈清空时退出循环(回到主屏)。
    ///
    /// 返回值:`Result<Option<String>>` —— `Some(msg)` 表示子屏触发了
    /// `Outcome::Toast(msg)`,由调用者在退出 alternate screen 后输出到主屏。
    /// 第三十轮 Bug 修复:之前在子屏内直接 `println!` 会被 alternate screen
    /// 切换吞掉,用户看不到任何反馈。
    async fn run_screen_loop(
        initial: Box<dyn crate::tui::engine::Screen>,
    ) -> Result<Option<String>> {
        use crate::tui::engine::{present, read_key, Frame, Outcome, Rect};

        let mut stack: Vec<Box<dyn crate::tui::engine::Screen>> = Vec::new();
        stack.push(initial);

        // 栈中每层屏都进入一次
        for s in stack.iter_mut() {
            s.on_enter();
        }

        while let Some(top) = stack.last_mut() {
            let area = Rect::full_screen();
            let mut frame = Frame::new(area);
            top.render(&mut frame);
            present(&frame).map_err(anyhow::Error::from)?;

            let key = read_key().map_err(anyhow::Error::from)?;
            // 用 take() 取出 Outcome 后再处理,避免借用冲突
            let outcome = top.handle_key(key);
            match outcome {
                Outcome::Continue => {}
                Outcome::Pop => {
                    let mut popped = stack.pop().unwrap();
                    popped.on_exit();
                }
                Outcome::Push(mut new_screen) => {
                    new_screen.on_enter();
                    stack.push(new_screen);
                }
                Outcome::Toast(msg) => {
                    // Toast:弹出现有屏栈,把消息回给调用者,在主屏上下文打印。
                    // 修复点:不在 alt screen 内 println,避免被 leave_alt 吞掉。
                    while let Some(mut s) = stack.pop() {
                        s.on_exit();
                    }
                    return Ok(Some(msg));
                }
                Outcome::Quit => std::process::exit(0),
            }
        }
        Ok(None)
    }
}
