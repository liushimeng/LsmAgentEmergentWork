//! 整页抹除(site_wipe)的延迟探测与后台看护(2026-10-14 第二/三轮,2026-10-19 扩)。
//!
//! 从 `mcp_web_use/mod.rs` 按职责拆出(该文件已超单文件行数上限),代码逐行
//! 机械搬移零改写;外部路径经 `mod.rs` 的 `pub(super) use` 再导出保持不变。
//!
//! 两道防线:
//! - [`deferred_wipe_check`]:同步复查(3.5s),只对**有抹除史**的域生效,
//!   命中即在本次响应里改写 `page_health` —— 无史站点零延迟零开销;
//! - [`schedule_wipe_watch`]:后台看护(**4s 首检 + 10s 终检**,2026-10-19),
//!   覆盖「open 返回时健康、数秒后被抹」的延迟形态并补记账。
//!
//! 设计见 `tmpPlan/20261019_111003_站点反爬声誉跨会话持久化与抹除早检根治.md`
//! 与 `tmpPlan/20261014_142000_整页抹除反复启动Chrome风暴与持久熔断根治方案.md`。

/// 延迟抹除复查(2026-10-14 第二轮,猎聘实测 `llaew_20261014_140053.log`):
/// open/navigate 返回时页面健康(标题/URL 正常),但指纹级打标站点会在**数秒后**
/// (实测 4~10s)才 `location→about:blank`,open 结算时的 page_health 看不到,
/// LLM 下一轮读到空白页无结构化信号、瞎猜后 close(all)+重开 → Chrome 拉起风暴。
/// 该域有抹除史(`wipe_total>0`)时,等 [`WIPE_RECHECK_DELAY`] 复读落地 URL:
/// 已变 about:blank → 记账并返回证据,调用方据此改写响应。无史站点零延迟零开销。
const WIPE_RECHECK_DELAY: std::time::Duration = std::time::Duration::from_millis(3500);

/// 返回 `Some((landed_url, late_outcome))` = 命中延迟抹除;`None` = 健康或无史。
/// `count` = 是否计入熔断账本(launch / laew relaunch 实例=true,用户日常 Chrome=false)。
pub(super) async fn deferred_wipe_check(
    page: &chromiumoxide::Page,
    requested_url: &str,
    count: bool,
) -> Option<(String, Option<crate::agent::browser_pace::WipeOutcome>)> {
    let host = crate::agent::browser_pace::host_of(requested_url)?;
    if crate::agent::browser_pace::WebPace::global().wipe_total_for(&host) == 0 {
        return None;
    }
    tokio::time::sleep(WIPE_RECHECK_DELAY).await;
    let landed = page.url().await.ok().flatten().unwrap_or_default();
    if !landed.trim().eq_ignore_ascii_case("about:blank") {
        return None;
    }
    let outcome = if count {
        crate::agent::browser_pace::WebPace::global()
            .note_navigation_outcome(requested_url, &landed)
    } else {
        None
    };
    tracing::warn!(
        requested_url,
        "延迟抹除复查命中:open 返回后页面被站点脚本重定向到 about:blank"
    );
    Some((landed, outcome))
}

// =================== 2026-10-14 第三轮:后台抹除看护(wipe watcher) ===================
//
// 缺口(猎聘实测 `llaew_20261014_145428.log`):整页抹除实测发生在导航完成后
// 4~10s,而 deferred_wipe_check 只在有史域做一次 3.5s 同步复查,且「落地非 blank
// 即清总账」让「先落地成功→数秒后被抹」的序列先清零后无补账 —— 总账轨迹实测
// 0→1→0→1→2→0,持久熔断(阈值 3)永不触发,close(all)→重开的换 profile 循环
// 不受限,Chrome 反复拉起(单任务 4 台)。本看护在每次 laew 身份导航落地非 blank
// 后 spawn 10s 异步任务复读落地 URL,变 blank 即 note_navigation_outcome 补账,
// 总账/连击/冷却/持久熔断全链路如期生效。

/// 后台抹除看护代际表:page_id → 最新一次被看护导航的代际号。同页后续导航
/// bump 代际,旧看护任务醒来后发现自己过期即放弃 —— 防止把「新导航的落地页」
/// 误记为旧请求被抹。
static WIPE_WATCH_GEN: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, u64>>,
> = std::sync::OnceLock::new();

/// 看护窗时长:覆盖实测 4~10s 的延迟抹除(终检,判「存活过看护窗」)。
const WIPE_WATCH_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

/// 首检时刻(2026-10-19)。猎聘实测抹除发生在导航返回后 **3~6s**,而旧实现只在
/// 10s 终检 —— 中间 3~7s 的窗口里模型已经拿着 `about:blank` 的**通用且反向**
/// 建议(「重新加载目标地址」)又烧了一轮。首检把补账与 TTL 台账落账提前到
/// 窗口开启之前,模型下一个动作即可拿到 `page_dead{reason:site_wipe}`。
const WIPE_WATCH_EARLY: std::time::Duration = std::time::Duration::from_secs(4);

fn wipe_watch_bump(page_id: &str) -> u64 {
    let map = WIPE_WATCH_GEN
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    let gen = guard.entry(page_id.to_string()).or_insert(0);
    *gen += 1;
    *gen
}

fn wipe_watch_stale(page_id: &str, gen: u64) -> bool {
    WIPE_WATCH_GEN
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|g| g.get(page_id).copied())
        .map(|cur| cur != gen)
        .unwrap_or(true)
}

/// 读一次落地 URL;句柄已死/页已关返回 `None`(**不算存活**,不清账)。
async fn read_landed(page: &chromiumoxide::Page) -> Option<String> {
    page.url().await.ok().flatten().filter(|u| !u.trim().is_empty())
}

/// 记一次延迟抹除:补账(连击/总账/冷却)+ 写 TTL 台账 + 告警。
fn account_late_wipe(page_id: &str, requested_url: &str, at: &'static str) {
    let landed = "about:blank";
    let outcome =
        crate::agent::browser_pace::WebPace::global().note_navigation_outcome(requested_url, landed);
    // 第 186 轮:本条导航的响应早已返回给 LLM,追改不了;把「哪一页被抹、
    // 被抹前请求的是什么」记进 TTL 台账,让模型**下一轮**的只读观测动作
    // (`inspect`/`explore`)能拿到 `page_dead{reason=site_wipe,late:true}`,
    // 而不是只看到 `about:blank` + `elements{count:0}` 干猜。
    crate::agent::browser_pace::WebPace::global().note_late_wipe(page_id, requested_url);
    tracing::warn!(
        requested_url,
        at,
        streak = outcome.as_ref().map(|o| o.streak).unwrap_or(0),
        "后台抹除看护命中:页面被站点脚本重定向到 about:blank(已补账:连击/总账/冷却)"
    );
}

/// 后台延迟抹除看护:`count=true`(launch 实例 / laew relaunch 调试实例)的导航
/// 落地非 blank 后调用。用户日常 Chrome 不看护(落地行为归用户,不进熔断账本)。
///
/// 两档(2026-10-19):**4s 首检 + 10s 终检**。
/// - 首检命中 → 立即补账 + 落 TTL 台账(模型下一动作即拿到成因与「别重试」建议);
/// - 首检存活 → 不提前清账,等终检再按「存活过看护窗」判恢复(保留原语义铁律:
///   抹除在 4~10s 之间,单次早存活不足以证明恢复)。
pub(super) fn schedule_wipe_watch(
    page: chromiumoxide::Page,
    page_id: &str,
    requested_url: &str,
    count: bool,
) {
    // 2026-10-19:登记「该页最近请求了什么」——死页判据在后台看护台账落账前
    // 靠它回落到域历史(抹除实测 3~6s 发生,看护 10s 才落账)。
    // 放在 `count` 判定**之前**:用户日常 Chrome 的页同样可能整页抹除,
    // 只是不进熔断账本(落地行为归用户),成因信号仍要给。
    crate::agent::browser_pace::WebPace::global().note_page_request(page_id, requested_url);
    if !count {
        return;
    }
    if crate::agent::browser_pace::host_of(requested_url).is_none() {
        return;
    }
    let gen = wipe_watch_bump(page_id);
    let pid = page_id.to_string();
    let url = requested_url.to_string();
    tokio::spawn(async move {
        // ---- 首检(4s):只判抹除,不清账 ----
        tokio::time::sleep(WIPE_WATCH_EARLY).await;
        if wipe_watch_stale(&pid, gen) {
            return; // 同页已有更新的导航接管,旧请求的看护作废
        }
        match read_landed(&page).await.as_deref() {
            Some(u) if !u.trim().eq_ignore_ascii_case("about:blank") => {
                // 存活,继续等终检
            }
            Some(_) => {
                account_late_wipe(&pid, &url, "4s首检");
                return;
            }
            // 回读为空(句柄已死/页已关):不等于存活,但也无法归因于站点抹除,
            // 交给终检/其它路径处理,不在此补账。
            None => return,
        }

        // ---- 终检(10s):判「存活过看护窗」= 真实恢复证据 ----
        tokio::time::sleep(WIPE_WATCH_DELAY - WIPE_WATCH_EARLY).await;
        if wipe_watch_stale(&pid, gen) {
            return;
        }
        let Some(landed) = read_landed(&page).await else {
            return;
        };
        if !landed.trim().eq_ignore_ascii_case("about:blank") {
            // 存活过看护窗 = 真实恢复证据:清该域连击与总账(仅「落地成功」不清 ——
            // 抹除发生在其后数秒,假成功会把补记账清掉,熔断永不触发)。
            if landed.starts_with("http") {
                let cleared = crate::agent::browser_pace::WebPace::global()
                    .note_navigation_survived(&url);
                if cleared {
                    tracing::info!(
                        requested_url = %url,
                        "后台抹除看护:页面存活过看护窗,清该域抹除连击与总账(恢复路径)"
                    );
                }
            }
            return;
        }
        account_late_wipe(&pid, &url, "10s终检");
    });
}
