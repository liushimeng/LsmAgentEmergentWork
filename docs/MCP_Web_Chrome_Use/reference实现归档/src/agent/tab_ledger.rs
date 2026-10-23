//! 任务级标签页台账（`docs/MCP_Web_Chrome_Use/25`）：记录「本任务开始时已存在哪些
//! 页签（基线）」，任务收口时与现存活页 diff，得出**本任务新开的页签** —
//! 这是任务边界自动清理（`page_activity::finalize_task_tabs`）的判定输入。
//!
//! 刻意不做「打开埋点」（browser.rs `new_page`/`adopt_spawned_pages` 零改动）：
//! 页签来源有三条（open / control(new_tab) / 点击与 JS 派生收编），埋点要覆盖
//! 全部注册路径且与在途并发改动抢同一个文件；基线 diff 在收口时经
//! `BrowserManager::list_pages` 现取，天然覆盖全部来源，且中途关闭的页自然
//! 不在现取集合里，无需维护「开-关」配对状态。
//!
//! 线程模型：TUI 单轮任务串行、`-p` 单任务，全局单份 Mutex 足够；fail-open
//! （锁中毒/浏览器未启动一律不阻断主流程）。

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// 任务开始时的基线 page_id 集合（`begin_task` 快照；收口后清空）。
static BASELINE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn baseline() -> &'static Mutex<HashSet<String>> {
    BASELINE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 本机制开关：`LAEW_WEB_TAB_AUTOCLOSE=off` 关闭任务收口自动清理（默认开启）。
pub fn autoclose_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_TAB_AUTOCLOSE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 任务开始：快照当时存活的全部 page_id 作为基线（fail-open：浏览器未启动/
/// 锁中毒时基线为空 —— diff 结果 = 全部现存活页，语义退化为「本任务开的页
/// 无法区分」，此时收口保留一切，不会误杀）。
pub async fn begin_task() {
    let ids: HashSet<String> = crate::agent::browser::BrowserManager::global()
        .list_pages()
        .await
        .into_iter()
        .map(|(id, _, _, _)| id)
        .collect();
    if let Ok(mut b) = baseline().lock() {
        *b = ids;
    }
}

/// 基线集合快照（供 diff 纯函数与测试）。
pub fn baseline_snapshot() -> HashSet<String> {
    baseline().lock().map(|b| b.clone()).unwrap_or_default()
}

/// 本任务新开的页 = 现存活页 - 基线（纯函数，收口判定核心）。
pub fn newly_opened(alive: &[String], base: &HashSet<String>) -> Vec<String> {
    alive.iter().filter(|id| !base.contains(*id)).cloned().collect()
}

/// 任务收口：清空基线（下一任务重新快照，跨任务页签不被后续任务误认）。
pub fn end_task() {
    if let Ok(mut b) = baseline().lock() {
        b.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 新开户diff_基线外的才算() {
        let base = set(&["p_a", "p_b"]);
        let alive = vec!["p_a".to_string(), "p_c".to_string(), "p_d".to_string()];
        let new_ids = newly_opened(&alive, &base);
        assert_eq!(new_ids, vec!["p_c".to_string(), "p_d".to_string()]);
    }

    #[test]
    fn 新开户diff_全部在基线内时为空() {
        let base = set(&["p_a", "p_b"]);
        let alive = vec!["p_a".to_string(), "p_b".to_string()];
        assert!(newly_opened(&alive, &base).is_empty());
    }

    #[test]
    fn 基线空时退化为全部保留不误杀语义由调用方决定() {
        // 基线拿不到(fail-open)时 diff = 全部现存活页；收口方据此「无 diff
        // 证据的新页」仍按后台/前台判定处置 —— 本函数只保证纯 diff 正确。
        let base = set(&[]);
        let alive = vec!["p_x".to_string()];
        assert_eq!(newly_opened(&alive, &base), vec!["p_x".to_string()]);
    }

    #[test]
    fn end_task清空基线() {
        {
            let mut b = baseline().lock().unwrap();
            *b = set(&["p_a"]);
        }
        end_task();
        assert!(baseline_snapshot().is_empty());
    }
}
