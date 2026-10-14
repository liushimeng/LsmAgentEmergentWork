//! 站点导航节奏治理(第 171 轮,京东 1W 组装机任务实测事故治理)。
//!
//! 实测事故(2026-10-14,`llaew_20261014_105403.log`):京东对高频搜索会话回频控横幅
//! 「抱歉由于访问频繁导致无法搜索,请稍后再试!」,但工具层两处失守——
//! 1. 横幅文本不在 [`super`](crate::agent::tools::mcp_web_use::inspect) 的阻断模式表
//!    (表里只有「访问过于频繁」,「访问频繁」是不同子串),频控状态从未成为结构化信号;
//! 2. 无导航节奏治理:全 run 对 jd.com 显式导航 ~117 次 search + ~40 次 list
//!    (QC 重试每次从头重扫相同页面),命中频控后仍继续硬闯,只会延长封锁。
//!
//! 本模块提供两条互补防线,进程级单例(与 `TargetAnchor` 同构):
//! 1. **最小导航间隔**:同一注册域内两次显式导航(open/navigate/new_window)间隔不足
//!    [`MIN_INTERVAL_MS`] 时**透明等待补齐** —— 不打断 LLM 动作语义,只把突发流量
//!    摊平(单步 sequence/batch 里连发导航是实测事故形态之一);
//! 2. **频控冷却**:任何探针(推送式 `blocker_probe` / 拉取式 `inspect(blockers)`)
//!    命中 `rate_limited` 时 [`WebPace::mark_rate_limited`] 该域,冷却期内同域导航
//!    返回 **6004**(动作不执行,附剩余秒数与处置建议);剩余 ≤
//!    [`AUTO_WAIT_SECS`] 时透明等待后放行;`params.force=true` 逃生口。
//! 3. **整页抹除(site_wipe)冷却**(2026-10-14,猎聘实测 `llaew_20261014_125139.log`):
//!    部分站点对 CDP 会话做**会话/指纹级打标**,被标会话每次页面完整加载(主文档
//!    200 + 子资源齐全)后数秒内被站点脚本 `location→about:blank` 整页抹除,
//!    人工登录同样被抹(「页面是黑的」)。[`WebPace::note_navigation_outcome`]
//!    记录「显式导航落 about:blank」域级连击,连续 ≥[`WIPE_STREAK_THRESHOLD`]
//!    次触发 `cause=site_wipe` 冷却(短于频控——目的不是等时间而是强制换策略:
//!    close(all) 回收浏览器换全新 profile / reuse_existing 接管日常浏览器)。
//!
//! 杀开关 `LAEW_WEB_PACE=off/0/false/no` 整体关闭(严格向后兼容)。
//!
//! 注意:只统计**显式导航**(open/navigate/new_window/connect 复用),点击链接派生
//! 的导航不统计 —— 锚点站点出发的合法外链跳转是正常任务形态,由 target_anchor
//! 与 QC 目标一致性门事后对账,不在节奏层管辖。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::json;

/// 频控冷却信封码(6xxx 范围约束段,避开 5xxx Use_MCP 与 4xxx HITL)。
pub const CODE_SITE_PACE_COOLDOWN: i32 = 6004;

/// 整页抹除连击阈值:同注册域**连续** N 次显式导航落 about:blank 触发 site_wipe 冷却。
///
/// 取 2:第 170 轮实测的「首航 blank 同页二次导航即成功」瞬态首击在 open 内部
/// 重导航后落地非 blank 即清零,不会误触发;只有「含内部重导航仍 blank」的完整
/// 流程连击两次才熔断(第一次靠 page_health{site_wipe} 证据引导 LLM 自行改道,
/// 第二次仍硬闯则强制冷却止损)。
pub const WIPE_STREAK_THRESHOLD: u32 = 2;

/// 总开关(默认开;`LAEW_WEB_PACE=off/0/false/no` 关闭)。
pub fn enabled() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_PACE").ok().as_deref(),
        Some("off") | Some("0") | Some("false") | Some("no")
    )
}

/// 同域最小导航间隔(毫秒;默认 2500,`0`/非法值回退默认)。
pub fn min_interval_ms() -> u64 {
    env_ms("LAEW_WEB_PACE_MIN_INTERVAL_MS", 2500)
}

/// 频控冷却时长(秒;默认 240,`0`/非法值回退默认)。
pub fn rate_cooldown_secs() -> u64 {
    env_secs("LAEW_WEB_RATE_COOLDOWN_SECS", 240)
}

/// 整页抹除冷却时长(秒;默认 90,`0`/非法值回退默认)。短于频控:抹除是会话级
/// 打标,等时间无意义,冷却的目的是**强制换策略**(close(all) 换全新 profile 会
/// 经 [`WebPace::clear_all`] 立即解除冷却,reuse_existing 接管日常浏览器同理不受影响)。
pub fn wipe_cooldown_secs() -> u64 {
    env_secs("LAEW_WEB_WIPE_COOLDOWN_SECS", 90)
}

/// 冷却成因(信封 `data.cause` 与文案分流;频控靠等,抹除靠换身份)。
pub const CAUSE_RATE_LIMITED: &str = "rate_limited";
pub const CAUSE_SITE_WIPE: &str = "site_wipe";

/// 自动等待上限(秒;冷却剩余 ≤ 此值时透明等待后放行;默认 45,`0` 关闭自动等待
/// 一律走 6004 拦截)。
pub fn auto_wait_secs() -> u64 {
    env_secs("LAEW_WEB_PACE_AUTO_WAIT_SECS", 45)
}

fn env_ms(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn env_secs(name: &str, default: u64) -> u64 {
    env_ms(name, default)
}

/// URL → 注册域(search.jd.com / www.jd.com / list.jd.com → `jd.com`)。
///
/// - IPv4/IPv6 字面量原样返回(不做域折叠,冷却按具体地址生效);
/// - 双段后缀(com.cn/co.uk 等)保留三段(`a.b.com.cn` → `b.com.cn`);
/// - 无 scheme / 无主机 返回 `None`(非可解析 URL,交由调用方 fail-open)。
pub fn host_of(url: &str) -> Option<String> {
    let after = url.split_once("://").map(|(_, rest)| rest)?;
    let host = after
        .split(['/', '?', '#'])
        .next()
        .filter(|s| !s.is_empty())?;
    // 剥端口:`8.130.85.252:29003` / `[::1]:9222`(带端口的串过不了 IpAddr 解析,
    // 不剥会被当域名做标签折叠)。IPv6 保留裸地址(去方括号,`::1` 可直接解析)。
    let host = if let Some(stripped) = host.strip_prefix('[') {
        stripped.split(']').next().unwrap_or(stripped)
    } else {
        host.split(':').next().unwrap_or(host)
    };
    let host = host.to_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Some(host);
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 {
        return Some(host);
    }
    let last_two = labels[labels.len() - 2..].join(".");
    // 双段顶级域(中国/英联邦常见):折叠多一级,防 `a.jd.co.jp` 与 `b.jd.co.jp`
    // 各算各的域
    const TWO_PART_TLDS: &[&str] = &[
        "com.cn", "net.cn", "org.cn", "gov.cn", "edu.cn", "ac.cn", "mil.cn",
        "co.uk", "org.uk", "gov.uk", "ac.uk", "me.uk", "com.tw", "com.hk",
        "com.sg", "com.au", "net.au", "co.jp", "ne.jp", "or.jp", "com.br",
    ];
    if TWO_PART_TLDS.contains(&last_two.as_str()) && labels.len() >= 3 {
        Some(labels[labels.len() - 3..].join("."))
    } else {
        Some(last_two)
    }
}

/// 一次导航闸决策(纯函数,可单测)。
#[derive(Debug, PartialEq, Eq)]
pub enum PaceDecision {
    /// 允许导航(可立即放行)
    Allow,
    /// 透明等待(冷却剩余/间隔补差,等待后照常执行)
    Wait(Duration),
    /// 拦截(返 6004,动作不执行)
    Block { remaining: Duration },
}

impl PaceDecision {
    /// 纯决策(不落账本;放行/等待路径由 [`WebPace::note_nav_for`] 在决策后登记)。
    fn decide(state: &PaceState, host: &str, min_interval: Duration, auto_wait: Duration) -> PaceDecision {
        let now = Instant::now();
        if let Some(until) = state.cooldown_until.get(host) {
            let remaining = until.saturating_duration_since(now);
            if remaining > Duration::ZERO {
                return if auto_wait > Duration::ZERO && remaining <= auto_wait {
                    PaceDecision::Wait(remaining)
                } else {
                    PaceDecision::Block { remaining }
                };
            }
        }
        if min_interval > Duration::ZERO {
            if let Some(last) = state.last_nav.get(host) {
                let gap = now.saturating_duration_since(*last);
                if gap < min_interval {
                    return PaceDecision::Wait(min_interval - gap);
                }
            }
        }
        PaceDecision::Allow
    }
}

/// 节奏状态(进程内单例)。
pub struct PaceState {
    /// 每注册域最近一次显式导航时刻
    last_nav: HashMap<String, Instant>,
    /// 每注册域冷却截止时刻(频控与整页抹除共用一张冷却表)
    cooldown_until: HashMap<String, Instant>,
    /// 每注册域冷却成因(`rate_limited` / `site_wipe`,信封文案分流)
    cooldown_cause: HashMap<String, &'static str>,
    /// 每注册域「显式导航落 about:blank」连击计数(落地非 blank 即清零)
    wipe_streak: HashMap<String, u32>,
}

impl Default for PaceState {
    fn default() -> Self {
        Self {
            last_nav: HashMap::new(),
            cooldown_until: HashMap::new(),
            cooldown_cause: HashMap::new(),
            wipe_streak: HashMap::new(),
        }
    }
}

/// 一次导航落地的抹除记账结果(供调用方附加到响应,如 open 的 `data.site_wipe`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WipeOutcome {
    /// 本次落地后该域的连击次数
    pub streak: u32,
    /// 本次是否触发了 site_wipe 冷却
    pub cooldown_triggered: bool,
}

/// 全局单例句柄。
#[derive(Default)]
pub struct WebPace {
    state: Mutex<PaceState>,
}

static GLOBAL: OnceLock<WebPace> = OnceLock::new();

impl WebPace {
    pub fn global() -> &'static WebPace {
        GLOBAL.get_or_init(WebPace::default)
    }

    /// 标记某域进入频控冷却(幂等取更晚截止时刻;成因跟随实际生效的标记)。
    pub fn mark_rate_limited_for(&self, host: &str, secs: Duration) {
        let until = Instant::now() + secs;
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.cooldown_until.entry(host.to_string()).or_insert(until);
        if until > *entry {
            *entry = until;
            guard
                .cooldown_cause
                .insert(host.to_string(), CAUSE_RATE_LIMITED);
        }
    }

    /// 按环境默认冷却时长标记。
    pub fn mark_rate_limited(&self, host: &str) {
        self.mark_rate_limited_for(host, Duration::from_secs(rate_cooldown_secs()));
    }

    /// 显式导航落地记账(整页抹除连击;2026-10-14 猎聘实测形态,见模块头 3.)。
    ///
    /// - 落地 `about:blank` → 该域连击 +1;连击 ≥ [`WIPE_STREAK_THRESHOLD`] 触发
    ///   `cause=site_wipe` 冷却(幂等取更晚;未过阈值不拦截,靠 page_health 证据引导);
    /// - 落地非 blank → 连击清零(瞬态首击/恢复均走这里);
    /// - 请求 URL 不可解析 host / 落地为空 → 不记账。
    ///
    /// 返回 `Some` 仅当本次落地被计为抹除连击(调用方据此在响应里附 `site_wipe` 段)。
    pub fn note_navigation_outcome(
        &self,
        requested_url: &str,
        landed_url: &str,
    ) -> Option<WipeOutcome> {
        if !enabled() {
            return None;
        }
        let host = host_of(requested_url)?;
        if !landed_url.trim().eq_ignore_ascii_case("about:blank") {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .wipe_streak
                .remove(&host);
            return None;
        }
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let streak = guard.wipe_streak.entry(host.clone()).or_insert(0);
        *streak += 1;
        let streak = *streak;
        let mut triggered = false;
        if streak >= WIPE_STREAK_THRESHOLD {
            let until = Instant::now() + Duration::from_secs(wipe_cooldown_secs());
            let entry = guard.cooldown_until.entry(host.clone()).or_insert(until);
            if until > *entry {
                *entry = until;
            }
            // 抹除与频控共用冷却表:后到的标记覆盖成因,信封按最近信号给建议。
            guard.cooldown_cause.insert(host, CAUSE_SITE_WIPE);
            triggered = true;
        }
        Some(WipeOutcome {
            streak,
            cooldown_triggered: triggered,
        })
    }

    /// 冷却成因(`rate_limited` / `site_wipe`);无有效冷却返回 `None`。
    pub fn cooldown_cause_for(&self, host: &str) -> Option<&'static str> {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .cooldown_until
            .get(host)
            .map(|until| until.saturating_duration_since(Instant::now()))
            .filter(|d| *d > Duration::ZERO)?;
        guard.cooldown_cause.get(host).copied()
    }

    /// 清空全部冷却与抹除连击(close(page_id="all") 回收浏览器时调用)。
    ///
    /// 全新 profile = 全新会话身份(新 cookie/新指纹),原会话的频控/抹除标记
    /// 不再适用;不清 `last_nav`(最小间隔节奏与身份无关)。
    pub fn clear_all(&self) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.cooldown_until.clear();
        guard.cooldown_cause.clear();
        guard.wipe_streak.clear();
    }

    /// 频控剩余冷却;`None` = 无冷却。
    pub fn cooldown_remaining_for(&self, host: &str) -> Option<Duration> {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .cooldown_until
            .get(host)
            .map(|until| until.saturating_duration_since(Instant::now()))
            .filter(|d| *d > Duration::ZERO)
    }

    /// 登记一次导航(最小间隔账本)。
    pub fn note_nav_for(&self, host: &str) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.last_nav.insert(host.to_string(), Instant::now());
    }

    /// 闸决策(测试/复用入口)。
    pub fn decide(&self, host: &str) -> PaceDecision {
        let min = Duration::from_millis(min_interval_ms());
        let auto = Duration::from_secs(auto_wait_secs());
        PaceDecision::decide(&self.state.lock().unwrap_or_else(|e| e.into_inner()), host, min, auto)
    }
}

/// 统一导航闸:`open` / `navigate` / `new_window`(含 connect 复用)共用。
///
/// 返回 `Some` = 拦截信封(6004,动作不执行);`None` = 放行(透明等待已在内部完成,
/// 导航账本已登记)。`force=true` 逃生口跳过全部节奏约束。
/// 拦截文案按冷却成因分流:频控靠等(冷却后低频重试),抹除靠换身份(全新 profile
/// / 接管日常浏览器)——建议错位会把 LLM 引向无效等待(猎聘实测硬闯 25+ 次的教训)。
pub async fn guard_navigation(url: &str, force: bool) -> Option<crate::error::Result<String>> {
    if !enabled() || force {
        return None;
    }
    let host = host_of(url)?;
    match WebPace::global().decide(&host) {
        PaceDecision::Block { remaining } => {
            let secs = remaining.as_secs().max(1);
            let cause = WebPace::global()
                .cooldown_cause_for(&host)
                .unwrap_or(CAUSE_RATE_LIMITED);
            let (cooldown, advice, msg) = if cause == CAUSE_SITE_WIPE {
                (
                    wipe_cooldown_secs(),
                    "站点整页抹除(反爬,会话/指纹级打标):本会话内继续导航/重试均无效。\
处置:① close(page_id=\"all\") 回收浏览器换全新 profile(冷却自动解除)后重新 open;\
② open(reuse_existing=true) 接管本机日常浏览器(真实指纹+登录态);\
③ 人工在场可 request_human 协助;已采集数据先落盘",
                    format!(
                        "站点整页抹除冷却中({host}):剩余 {secs}s;同会话重试无效,\
建议 close(page_id=\"all\") 回收浏览器(自动解除冷却)或 reuse_existing 接管日常浏览器;\
确需原会话立即重试可 params.force=true",
                    ),
                )
            } else {
                (
                    rate_cooldown_secs(),
                    "站点频控/限流:不要对该站点继续连续导航/搜索(高频只会延长封锁);\
先做本地整理或其它页面/站点工作,冷却后低频重试;已采集数据先落盘。\
任何探针命中频控标记后工具层自动进入冷却",
                    format!(
                        "站点频控冷却中({host}):剩余 {secs}s 后放行;冷却期内不要对同站点连续导航/搜索;\
确需立即导航可 params.force=true",
                    ),
                )
            };
            let data = json!({
                "host": host,
                "cause": cause,
                "remaining_secs": secs,
                "cooldown_secs": cooldown,
                "retry_after_secs": secs,
                "advice": advice,
                "force_hint": "确认必须立即导航时,可在 params 加 force=true 跳过节奏闸强制执行",
            });
            // 与 mcp_web_use::envelope 同形(该 helper 是 pub(super),跨模块不可见)
            serde_json::to_string(&json!({
                "code": CODE_SITE_PACE_COOLDOWN,
                "message": msg,
                "data": data,
            }))
            .ok()
            .map(Ok)
        }
        PaceDecision::Wait(d) => {
            tokio::time::sleep(d).await;
            WebPace::global().note_nav_for(&host);
            None
        }
        PaceDecision::Allow => {
            WebPace::global().note_nav_for(&host);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pace() -> WebPace {
        WebPace::default()
    }

    #[test]
    fn 域名归一_多级子域折叠到注册域() {
        assert_eq!(
            host_of("https://search.jd.com/Search?keyword=CPU"),
            Some("jd.com".into())
        );
        assert_eq!(
            host_of("http://www.jd.com/"),
            Some("jd.com".into())
        );
        assert_eq!(
            host_of("https://list.jd.com/list.html?cat=670,677"),
            Some("jd.com".into())
        );
        assert_eq!(
            host_of("https://item.jd.com/100042811600.html"),
            Some("jd.com".into())
        );
    }

    #[test]
    fn 域名归一_双段后缀保留三段() {
        assert_eq!(
            host_of("https://www.example.com.cn/a"),
            Some("example.com.cn".into())
        );
        assert_eq!(
            host_of("https://a.b.example.co.uk"),
            Some("example.co.uk".into())
        );
        assert_eq!(
            host_of("https://shop.example.com"),
            Some("example.com".into())
        );
    }

    #[test]
    fn 域名归一_IP地址原样返回() {
        assert_eq!(
            host_of("https://8.130.85.252:29003/anthropic"),
            Some("8.130.85.252".into())
        );
        assert_eq!(host_of("https://[::1]:9222/json"), Some("::1".into()));
        // IPv6 字面量不按点折叠
        assert!(host_of("http://[::1]/x").is_some());
    }

    #[test]
    fn 域名归一_非法URL返回None() {
        assert_eq!(host_of("about:blank"), None);
        assert_eq!(host_of(""), None);
        assert_eq!(host_of("not a url"), None);
    }

    #[test]
    fn 频控冷却_超过自动等待上限时拦截并报剩余秒数() {
        let p = pace();
        let host = "jd.com";
        // 冷却 60s > 自动等待 1s → Block,剩余 ∈ (1s, 60s]
        p.mark_rate_limited_for(host, Duration::from_secs(60));
        let state = p.state.lock().unwrap();
        let d = PaceDecision::decide(
            &state,
            host,
            Duration::from_millis(100),
            Duration::from_secs(1),
        );
        match d {
            PaceDecision::Block { remaining } => {
                assert!(remaining > Duration::from_secs(1) && remaining <= Duration::from_secs(60));
            }
            other => panic!("期望 Block,实际 {other:?}"),
        }
    }

    #[test]
    fn 频控冷却_自动等待内决策为Wait且不超过剩余() {
        let p = pace();
        let host = "jd.com";
        // 冷却 500ms ≤ 自动等待 1s → 透明等待,等待 ≤ 剩余
        p.mark_rate_limited_for(host, Duration::from_millis(500));
        let state = p.state.lock().unwrap();
        let d = PaceDecision::decide(
            &state,
            host,
            Duration::from_millis(100),
            Duration::from_secs(1),
        );
        match d {
            PaceDecision::Wait(w) => {
                assert!(w > Duration::ZERO && w <= Duration::from_millis(500));
            }
            other => panic!("期望 Wait,实际 {other:?}"),
        }
    }

    #[test]
    fn 频控冷却_自动等待关闭时一律拦截() {
        let p = pace();
        let host = "jd.com";
        p.mark_rate_limited_for(host, Duration::from_millis(100));
        let state = p.state.lock().unwrap();
        let d = PaceDecision::decide(&state, host, Duration::ZERO, Duration::ZERO);
        assert!(matches!(d, PaceDecision::Block { .. }));
    }

    #[test]
    fn 频控冷却_冷却过期后放行() {
        let p = pace();
        let host = "jd.com";
        p.mark_rate_limited_for(host, Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(p.decide(host), PaceDecision::Allow);
    }

    #[test]
    fn 最小间隔_同域连续导航需补差等待() {
        let p = pace();
        let host = "jd.com";
        p.note_nav_for(host);
        // 最近一次导航在"现在",min_interval=2500ms → 需补 ~2500ms
        let state = p.state.lock().unwrap();
        match PaceDecision::decide(&state, host, Duration::from_millis(2500), Duration::from_secs(45)) {
            PaceDecision::Wait(w) => assert!(w > Duration::from_millis(2400) && w <= Duration::from_millis(2500)),
            other => panic!("期望 Wait,实际 {other:?}"),
        }
        // 未登记域不受影响
        assert_eq!(
            PaceDecision::decide(&state, "other.com", Duration::from_millis(2500), Duration::from_secs(45)),
            PaceDecision::Allow
        );
    }

    #[test]
    fn 最小间隔_间隔足够后放行() {
        let p = pace();
        let host = "jd.com";
        p.note_nav_for(host);
        // 把 last_nav 拨到 3 秒前(超过 2500ms 最小间隔)
        {
            let mut guard = p.state.lock().unwrap();
            let _ = guard
                .last_nav
                .insert(host.to_string(), Instant::now() - Duration::from_secs(3));
        }
        let state = p.state.lock().unwrap();
        assert_eq!(
            PaceDecision::decide(&state, host, Duration::from_millis(2500), Duration::from_secs(45)),
            PaceDecision::Allow
        );
    }

    #[test]
    fn 信封码落在6xxx范围约束段() {
        assert_eq!(CODE_SITE_PACE_COOLDOWN, 6004);
    }

    #[test]
    fn 开关默认开启时env解析合法值回退() {
        // 不设环境变量时 min_interval_ms() 回退默认(>0)
        assert!(min_interval_ms() > 0);
        assert!(rate_cooldown_secs() > 0);
        assert!(auto_wait_secs() > 0);
    }

    // ========== 整页抹除(site_wipe)治理(2026-10-14,猎聘实测) ==========

    #[test]
    fn 整页抹除_落地about_blank累计连击() {
        let p = pace();
        // 第一次落 about:blank:连击 1,未过阈值不触发冷却
        let o1 = p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        assert_eq!(
            o1,
            Some(WipeOutcome {
                streak: 1,
                cooldown_triggered: false
            })
        );
        assert_eq!(p.cooldown_remaining_for("liepin.com"), None);
        // 第二次:连击 2 = 阈值,触发 site_wipe 冷却
        let o2 = p.note_navigation_outcome("https://www.liepin.com/zhaopin/", "about:blank");
        assert_eq!(
            o2,
            Some(WipeOutcome {
                streak: 2,
                cooldown_triggered: true
            })
        );
        assert!(p.cooldown_remaining_for("liepin.com").is_some());
        assert_eq!(p.cooldown_cause_for("liepin.com"), Some(CAUSE_SITE_WIPE));
    }

    #[test]
    fn 整页抹除_落地正常页面即清零连击() {
        let p = pace();
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        // 瞬态首击形态:open 内部重导航成功 → 落地目标站 → 连击清零
        assert_eq!(
            p.note_navigation_outcome("https://www.liepin.com/", "https://www.liepin.com/"),
            None
        );
        // 再次单次 blank 仍是连击 1,不触发
        let o = p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        assert_eq!(o.map(|x| (x.streak, x.cooldown_triggered)), Some((1, false)));
        assert_eq!(p.cooldown_remaining_for("liepin.com"), None);
    }

    #[test]
    fn 整页抹除_子域共享注册域连击() {
        let p = pace();
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        let o = p.note_navigation_outcome("https://api-c.liepin.com/x", "about:blank");
        assert!(o.is_some_and(|x| x.cooldown_triggered));
        assert_eq!(p.cooldown_cause_for("liepin.com"), Some(CAUSE_SITE_WIPE));
    }

    #[test]
    fn 整页抹除_不可解析host不记账() {
        let p = pace();
        assert_eq!(p.note_navigation_outcome("about:blank", "about:blank"), None);
        assert_eq!(p.note_navigation_outcome("not a url", "about:blank"), None);
    }

    #[test]
    fn 整页抹除_冷却期导航决策为拦截() {
        let p = pace();
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        // 冷却 90s > 自动等待 45s → Block
        match p.decide("liepin.com") {
            PaceDecision::Block { .. } => {}
            other => panic!("期望 Block,实际 {other:?}"),
        }
        // 其它域不受影响
        assert_eq!(p.decide("jd.com"), PaceDecision::Allow);
    }

    #[test]
    fn 整页抹除_clear_all清空冷却与连击_保留间隔账本() {
        let p = pace();
        p.note_nav_for("liepin.com");
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        assert!(p.cooldown_remaining_for("liepin.com").is_some());
        // close(all) 回收浏览器:全新 profile = 全新身份
        p.clear_all();
        assert_eq!(p.cooldown_remaining_for("liepin.com"), None);
        assert_eq!(p.cooldown_cause_for("liepin.com"), None);
        // 连击同样清零:新身份单次 blank 不再直接熔断
        let o = p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        assert_eq!(o.map(|x| (x.streak, x.cooldown_triggered)), Some((1, false)));
    }

    #[test]
    fn 冷却成因_频控标记覆盖抹除成因() {
        let p = pace();
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        assert_eq!(p.cooldown_cause_for("liepin.com"), Some(CAUSE_SITE_WIPE));
        // 频控横幅标记时长更长(240s > 90s)→ 成因随实际生效的标记切换
        p.mark_rate_limited_for("liepin.com", Duration::from_secs(240));
        assert_eq!(p.cooldown_cause_for("liepin.com"), Some(CAUSE_RATE_LIMITED));
    }
}
