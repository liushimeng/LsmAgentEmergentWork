//! 会话生命周期自由函数(自 `browser/mod.rs` 机械拆分,满足单文件 ≤ 1800 行约定;
//! 逐行搬移零改写,调用点经 mod.rs 私有 `use` 重导入保持原路径不变)。
//!
//! 职责:标签页复用探测、会话清空、窗口-标签联动关闭、JS 线程收尾通知、
//! 网络台账环形记录。

use super::*;

/// 同 URL/同源页面复用探测(导航复用语义,`open(reuse=true)` 用)。
pub(super) fn find_reusable(inner: &Inner, url: &str) -> Option<String> {
    let canonical = urlutil::canonical(url);
    let exact = inner
        .tabs
        .values()
        .find(|t| urlutil::canonical(&t.url) == canonical)
        .map(|t| t.id.clone());
    if exact.is_some() {
        return exact;
    }
    inner
        .tabs
        .values()
        .filter(|t| urlutil::same_origin(&t.url, url))
        .min_by_key(|t| t.created_at_ms)
        .map(|t| t.id.clone())
}

pub(super) fn clear_session_if_empty(inner: &mut Inner) {
    if inner.tabs.is_empty() {
        inner.cookies.clear();
        inner.local_storage.clear();
        inner.windows.clear();
    }
}

pub(super) fn close_from_windows(inner: &mut Inner, ids: &[String]) {
    for w in inner.windows.iter_mut() {
        w.tabs.retain(|id| !ids.contains(id));
        if let Some(active) = &w.active {
            if ids.contains(active) {
                w.active = w.tabs.first().cloned();
            }
        }
    }
    inner.windows.retain(|w| !w.tabs.is_empty());
}

/// 通知被关闭页面的 JS 线程退出(线程自行收尾,无需等待)。
pub(super) fn shutdown_js_handles(inner: &Inner, ids: &[String]) {
    for id in ids {
        if let Some(tab) = inner.tabs.get(id) {
            if let Some(h) = &tab.js {
                h.shutdown();
            }
        }
    }
}

pub(super) fn record_network(tab: &mut TabState, rec: NetworkRecord) {
    tab.network.push(rec);
    if tab.network.len() > 200 {
        tab.network.remove(0);
    }
}
