//! 人工取消旗标(第 186 轮)—— HITL「取消任务」选项的任务级贯通。
//!
//! 背景(2026-10-19 猎聘实测 `Dir01/laew_stdout.log`):弹窗把「取消任务」作为
//! **一等选项**呈现(`control/hitl_ctl.rs` 缺省选项第 2 项、auto-HITL 的
//! `HITL_CHALLENGE_OPTIONS`、系统提示词 `system_prompt/mod.rs` 教 LLM 显式传
//! `options=[…,"取消任务"]`),人工点选后应答文本 `"N. 取消任务"` 进
//! `HumanAssistOutcome::Answered` → 工具返 `code=0` + `next_hint`
//! 「请 inspect 验证页面状态后**继续流程**」。人工用产品提供的标准方式喊停,
//! 运行时把它当「继续」,任务继续空转下游单元(实测多烧一个 wf-2 单元 88.7s
//! 并进入 Yolo 回流)。
//!
//! 工具契约本已承诺「`4002=人工取消`」,但旧实现只在**关窗/Esc** 分支产生,
//! 覆盖不到推荐路径上的「取消任务」选项按钮 —— 本模块补的就是这个缺口。
//!
//! 为什么用进程级单例而不是复用编排器的 [`CancelToken`]:置旗方在**工具层**
//! (`request_human_core`),而 `CancelToken` 由编排器持有并逐层派生下传,工具层
//! 拿不到根 token(多窗口并行 + SubAgent 各自派生,反向寻址不可靠)。与
//! [`HumanAssistHub`](crate::agent::human_assist::HumanAssistHub) 同构最简。

use std::sync::OnceLock;
use std::sync::RwLock;

/// 人工取消旗标的进程级存放。`RwLock<Option<String>>`:`None` = 未取消。
fn cell() -> &'static RwLock<Option<String>> {
    static CELL: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(None))
}

/// 置位人工取消旗标(`reason` 供日志/决策审计对账,可空字符串)。
/// 幂等:已置位时不覆盖首个原因(首个才是人工真实的取消理由)。
pub fn set(reason: &str) {
    let mut g = cell().write().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = Some(reason.to_string());
    }
}

/// 是否已被人工取消。
pub fn is_set() -> bool {
    cell().read().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// 取人工取消原因(未取消时 `None`)。
pub fn reason() -> Option<String> {
    cell().read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 复位旗标 —— **每个用户任务入口必须调一次**,否则上一次任务的取消会污染下一次。
pub fn reset() {
    *cell().write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// 任务入口复位 + 返回是否需要立即短路(内部用,给编排器一个单调用点)。
///
/// 与 [`reset`] 分开是刻意的:调用方常在「先看有没有残留取消」之后才决定
/// 是否继续,两个语义混在一个函数里容易被漏调。
pub fn reset_and_take() -> Option<String> {
    cell()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 串行化:本组用例共享进程级单例,必须互斥(否则并行跑互相踩踏)。
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        let lock = LOCK.get_or_init(|| std::sync::Mutex::new(()));
        lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn 置位取原因与复位() {
        let _g = guard();
        reset();
        assert!(!is_set());
        set("login:人工点选「取消任务」");
        assert!(is_set());
        assert_eq!(reason().as_deref(), Some("login:人工点选「取消任务」"));
        reset();
        assert!(!is_set());
        assert_eq!(reason(), None);
    }

    #[test]
    fn 重复置位保留首个原因() {
        let _g = guard();
        reset();
        set("first");
        set("second");
        assert_eq!(reason().as_deref(), Some("first"));
        reset();
    }

    #[test]
    fn 复位并取走() {
        let _g = guard();
        reset();
        set("x");
        assert_eq!(reset_and_take().as_deref(), Some("x"));
        assert!(!is_set());
        assert_eq!(reset_and_take(), None);
    }
}
