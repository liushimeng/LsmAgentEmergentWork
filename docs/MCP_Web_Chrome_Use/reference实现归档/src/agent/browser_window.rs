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

// ===================== 多窗口并行(双站同时工作) =====================
//
// `open(new_window=true)` / `control(new_window)` 经 CDP `Target.createTarget
// (newWindow:true)` 在**单浏览器进程**里起多个独立可见窗口 —— 各窗口互不遮挡时
// 页面同时活跃、互不挂起(同窗口后台 Tab 会被 Chrome 挂起,见 mcp_web_use 的
// 后台挂起治理)。本模块提供窗口维度的查询与几何治理:
// - [`BrowserManager::page_window_id`]:页面 → CDP windowId(list 分组展示);
// - [`BrowserManager::count_windows`]:去重窗口数(`LAEW_WEB_MAX_WINDOWS` 上限闸);
// - [`BrowserManager::tile_agent_windows`]:窗口横向平铺(防互相遮挡被 occlusion
//   节流 —— Chrome 会把被完全遮挡的窗口标记 occluded 并停渲染,与后台 Tab 同病)。
//
// 按工程约定「browser.rs 超线,新增代码落职责子模块」,本组能力放这里,
// 只用 BrowserManager 公共 API,不触碰其私有字段。

use chromiumoxide::cdp::browser_protocol::browser::GetWindowForTargetParams;
use serde_json::{json, Value};

impl BrowserManager {
    /// 页面所属 CDP 窗口 id(多窗口并行:list 按窗口分组展示用;fail-open None)。
    pub async fn page_window_id(&self, page_id: &str) -> Option<i64> {
        let page = self.page(page_id).await?;
        page.execute(GetWindowForTargetParams::default())
            .await
            .ok()
            .map(|w| *w.window_id.inner())
    }

    /// 当前 Agent 窗口数(CDP windowId 去重;浏览器未启动 = 0)。
    ///
    /// 多窗口并行的上限闸(`LAEW_WEB_MAX_WINDOWS`)计数依据 —— 上限按**窗口**
    /// 计而非按页签计,同窗口多 Tab 不占新窗口额度。
    pub async fn count_windows(&self) -> usize {
        let ids: Vec<i64> = {
            let pages: Vec<chromiumoxide::Page> = {
                let mut ids_probe = Vec::new();
                // list_pages 顺手清理失效 entry;这里只需要 id 列表。
                for (id, _, _, _) in self.list_pages().await {
                    if let Some(p) = self.page(&id).await {
                        ids_probe.push(p);
                    }
                }
                ids_probe
            };
            let mut ids = Vec::new();
            for p in pages {
                if let Ok(w) = p.execute(GetWindowForTargetParams::default()).await {
                    ids.push(*w.window_id.inner());
                }
            }
            ids
        };
        let mut ids = ids;
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }

    /// 多窗口并行:把所有 Agent 窗口横向平铺进屏幕工作区(互不遮挡)。
    ///
    /// 仅在 headed && 非 connect && 去重窗口数 ≥2 时执行;fail-open(测量/调整
    /// 失败只跳过),不影响 open 语义。返回平铺报告供 open 响应 `data.windows`
    /// 引用。各窗口宽度按工作区等分 —— 窗口数多时单窗视口会低于 720p 下限,
    /// 「同时工作」优先于「单窗大屏」,如实退让。
    pub async fn tile_agent_windows(&self) -> Option<Value> {
        // headed && 非 connect 才有可见窗口几何可言(与 fit_headed_window 同判据)。
        let mode = self.current_mode().await;
        if !mode.is_some_and(|m| m.is_headed()) || self.is_connect_mode().await {
            return None;
        }
        // 每窗口取一个代表页(windowId 去重)。
        let mut per_window: Vec<(i64, String)> = Vec::new();
        let mut first_page: Option<chromiumoxide::Page> = None;
        for (id, _, _, _) in self.list_pages().await {
            let Some(page) = self.page(&id).await else { continue };
            if let Ok(w) = page.execute(GetWindowForTargetParams::default()).await {
                let wid = *w.window_id.inner();
                if first_page.is_none() {
                    first_page = Some(page.clone());
                }
                if !per_window.iter().any(|(w2, _)| *w2 == wid) {
                    per_window.push((wid, id));
                }
            }
        }
        if per_window.len() < 2 {
            return None;
        }
        // 屏幕工作区:用任一代表页实测(availLeft/availTop 覆盖副屏/多屏原点)。
        let page = first_page?;
        let m = super::browser::headed_window_metrics(&page).await.ok()?;
        let f = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let plan = super::browser_viewport::tile_window_plan(
            f("availWidth"),
            f("availHeight"),
            f("availLeft"),
            f("availTop"),
            per_window.len(),
        )?;
        let mut detail = Vec::new();
        for ((wid, pid), (l, t, w, h)) in per_window.iter().zip(plan.iter()) {
            match self
                .set_window_bounds(pid, Some(*w), Some(*h), Some(*l), Some(*t), None)
                .await
            {
                Ok(v) => detail.push(json!({
                    "window_id": wid,
                    "page_id": pid,
                    "bounds": v.get("window").cloned().unwrap_or(Value::Null),
                })),
                Err(e) => detail.push(json!({
                    "window_id": wid,
                    "page_id": pid,
                    "error": e,
                })),
            }
        }
        Some(json!({
            "tiled": true,
            "window_count": per_window.len(),
            "detail": detail,
            "note": "多个 Agent 窗口已横向平铺进屏幕工作区(互不遮挡,各窗口页面同时活跃渲染);窗口可人工拖动调整,后续新窗口打开时重新平铺",
        }))
    }
}
