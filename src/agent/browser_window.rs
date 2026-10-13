//! 浏览器窗口可见性 —— headed 启动后的窗口置前(第 156 轮 F2)。
//!
//! # 为什么独立成模块
//!
//! `browser.rs` 在第 156 轮前已是 1964 行(存量超 1800 行上限),按工程约定
//! 「临界文件新增代码优先落到职责子模块」,本能力拆出,`browser.rs` 只留调用点。
//!
//! # 解决什么问题
//!
//! headed 模式确实会拉起 Chrome(`code:0` + `final_url` + `title` 都能拿到),
//! 但项目**从不激活窗口**:
//! - `Page.bringToFront` 此前只在 `request_human` 前调用一次(`control.rs`),
//!   **open 路径零调用**;
//! - 不设 `--window-position`、不设 `--start-maximized`;
//! - `fit_headed_window` 显式不传 `left/top`(保持 Chrome 已有位置)。
//!
//! 于是 Chrome 窗口可能落在终端/编辑器之后,用户看到的就是「Agent 根本没启动
//! 浏览器」—— 实测反馈与「工具零调用」现象容易被混为一谈,排查成本很高。
//!
//! 这里在 headed 打开后补一次置前:纯 CDP,跨平台一致,**零新依赖、零平台分支**。

use super::browser::BrowserManager;

/// 把 headed 窗口带到前台,返回是否成功(**fail-open**)。
///
/// 仅 headed 有意义(无头没有可见窗口);模式判定由调用方
/// [`BrowserManager::fit_headed_window`] 完成(它在函数开头就 `return` 非 headed)。
///
/// 失败只 debug 记录,**绝不改变 open 的 `code:0` 语义** —— 置前是体验增强,
/// 不是功能前提,不能因为平台差异让整条 open 链路失败。
///
/// 第 166 轮焦点治理(`LAEW_WEB_FOCUS`,见 `browser_focus` 模块头):
/// - `keep`(缺省):**抬升不抢焦** —— OS 级 AXRaise / SetWindowPos(NOACTIVATE)
///   只把窗口提到 z-order 前排;连 `Page.bringToFront` 都不调(它会连带激活窗口
///   抢走键盘焦点,用户正在其它窗口打字时击键会灌进 Chrome);
/// - `always`:旧行为 —— CDP `Page.bringToFront` 置前激活;
/// - `never`:完全不碰。
pub(super) async fn activate_headed_window(mgr: &BrowserManager, page_id: &str) -> bool {
    use super::browser_focus::{browser_pid_hint, focus_policy_from_env, raise_window_no_activate};
    use super::browser_focus::FocusPolicy;
    match focus_policy_from_env() {
        FocusPolicy::Keep => {
            let pid = browser_pid_hint();
            if pid == 0 {
                // pid 未登记(connect 模式本就整路早退):不猜不置前,fail-open。
                tracing::debug!(page_id = %page_id, "keep 档抬升跳过(浏览器 pid 未登记)");
                return false;
            }
            let ok = raise_window_no_activate(pid);
            if !ok {
                tracing::debug!(
                    page_id = %page_id,
                    pid,
                    "抬升不抢焦失败(fail-open,不回退成抢焦点;需要激活用 LAEW_WEB_FOCUS=always)"
                );
            }
            ok
        }
        FocusPolicy::Always => match mgr.bring_page_to_front(page_id).await {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!(
                    page_id = %page_id,
                    error = %e,
                    "headed 窗口置前失败(fail-open,不影响 open 语义)"
                );
                false
            }
        },
        FocusPolicy::Never => false,
    }
}
