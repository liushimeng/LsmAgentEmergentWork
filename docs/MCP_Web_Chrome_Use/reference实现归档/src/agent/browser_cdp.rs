//! CDP 调用有界超时兜底（2026-10-16）。
//!
//! 背景：`chromiumoxide` 0.9 的所有 `Page`/`Browser` 调用最终都排队在 browser
//! handler 任务的事件循环里；某 target 的导航状态机一旦卡死（实测：中国大陆
//! 网络下 `github.com/commits` 连接级 stall，load 事件永不到达），该 target
//! 的后续 CDP 消息全部排队，客户端裸 await 永久挂起（实测 22 分钟零日志，
//! 整个 Agent 循环只能人工杀进程，见
//! `tmpPlan/20261016_163802_工具执行总超时与CDP有界化根治浏览器挂死.md`）。
//!
//! 本模块给高扇出轻量 CDP 调用统一加墙钟预算：超时/出错 → WARN + `None`
//! （fail-open，探测类调用按「不可用」处理，由调用方决定降级路径）。
//! 与 `tool_exec` 的工具级总超时互补：本模块让单条调用秒级失败，
//! 总超时是最后保险。

use std::future::Future;
use std::time::Duration;

use tracing::warn;

/// 单条 CDP 轻量调用的默认墙钟预算（秒）。
///
/// 取值依据：`Page::url()`/`get_title()`/各 `Enable`/`SetPermission` 类调用
/// 正常往返毫秒级；10s 足够覆盖慢网关下的真机延迟，又远小于工具级总超时
/// （`LAEW_TOOL_TIMEOUT_SECS` 默认 300s），给同工具内的后续降级留足预算。
pub(crate) const CDP_BOUND_SECS: u64 = 10;

/// 有界执行一条 CDP 调用：成功返回 `Some(Ok 值)`；出错或超时返回 `None` 并 WARN。
///
/// 仅供「探测/快照」类调用使用（返回值拿不到就按缺失处理的路径）。
/// 需要区分错误类型的调用方请自行 `match` 后降级。
pub(crate) async fn cdp_with_timeout<T, E: std::fmt::Display>(
    what: &str,
    fut: impl Future<Output = std::result::Result<T, E>>,
) -> Option<T> {
    cdp_with_timeout_for(what, CDP_BOUND_SECS, fut).await
}

/// [`cdp_with_timeout`] 的可调预算版本（测试用 1s 界锁死语义，避免慢测试）。
pub(crate) async fn cdp_with_timeout_for<T, E: std::fmt::Display>(
    what: &str,
    secs: u64,
    fut: impl Future<Output = std::result::Result<T, E>>,
) -> Option<T> {
    match tokio::time::timeout(Duration::from_secs(secs.max(1)), fut).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            warn!(what, error = %e, "CDP 调用失败(有界兜底),按不可用处理");
            None
        }
        Err(_) => {
            warn!(what, secs, "CDP 调用超时(有界兜底),按不可用处理——浏览器 handler 可能已卡死");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 正常返回的调用原样透出值。
    #[tokio::test]
    async fn cdp_bound_ok_value_passthrough() {
        let v = cdp_with_timeout_for("t", 5, async { Ok::<_, String>(42) }).await;
        assert_eq!(v, Some(42));
    }

    /// 出错返回 None（fail-open），不 panic。
    #[tokio::test]
    async fn cdp_bound_err_maps_to_none() {
        let v = cdp_with_timeout_for("t", 5, async { Err::<i32, _>("boom") }).await;
        assert_eq!(v, None);
    }

    /// 永不完成的 future 在预算界处被放弃，返回 None（挂死根治的核心语义）。
    #[tokio::test(start_paused = true)]
    async fn cdp_bound_hanging_future_aborted_at_budget() {
        let v = cdp_with_timeout_for("t", 1, std::future::pending::<Result<i32, String>>()).await;
        assert_eq!(v, None);
    }
}
