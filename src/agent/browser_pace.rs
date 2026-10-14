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
//! 4. **整页抹除总账与 laew 身份持久熔断**(2026-10-14 第二轮,猎聘实测
//!    `llaew_20261014_140053.log`):第 3 条里的连击/冷却可被
//!    `close(page_id="all") → clear_all()` 一键清零逃逸 —— 猎聘类站点对
//!    **CDP 拉起的 Chrome 环境本身**做指纹级打标,换全新 profile 照样被抹,
//!    LLM 按 6004 建议「close(all) 换全新 profile」反而每轮多弹一台 Chrome
//!    再被抹除(实测 9 分钟拉起 8 次浏览器的风暴)。[`PaceState::wipe_total`]
//!    是**跨浏览器实例的总账**(`clear_all` 不清),同域累计抹除 ≥
//!    [`wipe_total_threshold`] 次后对 **laew 身份**导航(launch 模式 /
//!    auto_relaunch 调试实例)持久返 6004 —— 与用户真实 Chrome 接管
//!    (真实指纹,通常不被指纹级打标)区分开,只剩 reuse_existing /
//!    request_human 两条出路,`force=true` 保留逃生口。
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

/// 整页抹除总账阈值(默认 3,`0`/非法值回退默认;`LAEW_WEB_WIPE_TOTAL_THRESHOLD`)。
///
/// 同注册域**跨浏览器实例**累计抹除 ≥ N 次后,对 laew 身份(launch 模式 /
/// auto_relaunch 调试实例)的该域导航**持久熔断**(6004,`close(all)` 逃逸无效)。
/// 取 3:给「会话级打标换全新 profile 即可恢复」的站点留 1~2 次试错空间,
/// 第 3 次仍被抹除即可判定为指纹级打标(换新实例无意义,只会反复弹 Chrome)。
pub const WIPE_TOTAL_THRESHOLD_DEFAULT: u32 = 3;

/// 整页抹除总账阈值(env 覆盖;`0`/非法值回退默认)。
pub fn wipe_total_threshold() -> u32 {
    std::env::var("LAEW_WEB_WIPE_TOTAL_THRESHOLD")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(WIPE_TOTAL_THRESHOLD_DEFAULT)
}

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

/// 风控挑战冷却时长(秒;默认 300,`0`/非法值回退默认;`LAEW_WEB_RISK_COOLDOWN_SECS`)。
/// 第 172 轮:人工验证挑战(baxia 滑块 punish 实测)是会话/指纹级打标,与抹除同属
/// 「换策略」族 —— 等待收益低,目的是**停止重试加剧风控**;人工完成验证即解除
/// (`WebPace::clear_risk_challenge`),不拦后续正常工作。
pub fn risk_cooldown_secs() -> u64 {
    env_secs("LAEW_WEB_RISK_COOLDOWN_SECS", 300)
}

/// 冷却成因(信封 `data.cause` 与文案分流;频控靠等,抹除靠换身份)。
pub const CAUSE_RATE_LIMITED: &str = "rate_limited";
pub const CAUSE_SITE_WIPE: &str = "site_wipe";
/// 第 172 轮:人工验证挑战/风控打标(baxia 滑块 punish 实测形态)。与会话级打标
/// 的 site_wipe 同属「换策略」族:等待收益低,目的是停止重试加剧风控。
pub const CAUSE_RISK_CHALLENGE: &str = "risk_challenge";

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
    /// 每注册域**跨浏览器实例**的累计抹除次数(总账;`clear_all` 不清,
    /// 落地非 blank 才清零)。同域累计 ≥ 阈值后对 laew 身份导航持久熔断 —
    /// 根治「close(all) 清连击 → 换全新 profile 再被抹」的 Chrome 拉起风暴
    /// (猎聘对 CDP 拉起环境做指纹级打标,换新实例无效,2026-10-14 实测)。
    wipe_total: HashMap<String, u32>,
}

impl Default for PaceState {
    fn default() -> Self {
        Self {
            last_nav: HashMap::new(),
            cooldown_until: HashMap::new(),
            cooldown_cause: HashMap::new(),
            wipe_streak: HashMap::new(),
            wipe_total: HashMap::new(),
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
        // `>=` 而非 `>`:全新域首记时 or_insert 返回与 until 相等的既有值,
        // `>` 会跳过成因写入 —— 成因缺失的裸冷却只能落回频控默认文案(第 172 轮
        // 修:mark_risk_challenge_for 同款隐患由单测锁死)。
        if until >= *entry {
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

    /// 标记某域进入**风控挑战冷却**(baxia 滑块 punish 实测形态;第 172 轮)。
    /// 幂等取更晚截止时刻;成因跟随实际生效的标记。语义与频控冷却共用一张表,
    /// 6004 按成因分流文案 —— 频控靠等,挑战/抹除靠换策略。
    pub fn mark_risk_challenge_for(&self, host: &str, secs: Duration) {
        let until = Instant::now() + secs;
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.cooldown_until.entry(host.to_string()).or_insert(until);
        // `>=`:全新域首记时必须写入成因,否则裸冷却落回频控默认文案(见
        // mark_rate_limited_for 同款注释)。
        if until >= *entry {
            *entry = until;
            guard
                .cooldown_cause
                .insert(host.to_string(), CAUSE_RISK_CHALLENGE);
        }
    }

    /// 按环境默认冷却时长标记风控挑战冷却。
    pub fn mark_risk_challenge(&self, host: &str) {
        self.mark_risk_challenge_for(host, Duration::from_secs(risk_cooldown_secs()));
    }

    /// 解除某域的风控挑战冷却(`request_human` 收口人工已完成验证时调用)。
    /// 只清 `risk_challenge` 成因的条目,不动 rate_limited / site_wipe。
    pub fn clear_risk_challenge(&self, host: &str) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if guard.cooldown_cause.get(host) == Some(&CAUSE_RISK_CHALLENGE) {
            guard.cooldown_cause.remove(host);
            guard.cooldown_until.remove(host);
        }
    }

    /// 显式导航落地记账(整页抹除连击;2026-10-14 猎聘实测形态,见模块头 3.)。
    ///
    /// - 落地 `about:blank` → 该域连击 +1、**总账 +1**(`wipe_total` 跨实例累计,
    ///   达 [`wipe_total_threshold`] 后对 laew 身份持久熔断);连击 ≥
    ///   [`WIPE_STREAK_THRESHOLD`] 触发 `cause=site_wipe` 冷却(幂等取更晚;
    ///   未过阈值不拦截,靠 page_health 证据引导);
    /// - 落地非 blank → **不清账**(2026-10-14 第三轮:抹除实测发生在导航完成后
    ///   4~10s,落地成功只是假恢复;清账只认 [`Self::note_navigation_survived`]
    ///   的「存活过看护窗」证据);
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
            // 2026-10-14 第三轮:**落地非 blank 不清账**。抹除实测发生在导航完成后
            // 4~10s(猎聘 `llaew_20261014_145428.log`),落地成功只说明「结算那一
            // 刻没被抹」。若此处清零,后台看护补记的抹除会被下一次导航的假成功
            // 清掉,总账永远归零、持久熔断(阈值 3)永不触发(实测轨迹
            // 0→1→0→1→2→0)。清账改由 [`Self::note_navigation_survived`] 执行 ——
            // 页面**存活过看护窗**才是真恢复证据。
            return None;
        }
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *guard.wipe_total.entry(host.clone()).or_insert(0) += 1;
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

    /// 该域跨实例累计抹除次数(总账;无记录返回 0)。
    pub fn wipe_total_for(&self, host: &str) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wipe_total
            .get(host)
            .copied()
            .unwrap_or(0)
    }

    /// 恢复记账(2026-10-14 第三轮):页面**存活过看护窗**(后台看护复读落地 URL
    /// 仍非 blank 且是 http(s))时,清该域连击与总账,并解除 `cause=site_wipe`
    /// 冷却 —— 这是真实恢复证据(新会话没被打标),恢复路径不被自己的熔断卡住。
    /// 仅「落地成功」不清账(抹除在其后 4~10s 发生,落地成功只是假恢复)。
    /// 返回是否确有账目被清(供看护任务按需打日志/测试)。
    pub fn note_navigation_survived(&self, requested_url: &str) -> bool {
        if !enabled() {
            return false;
        }
        let Some(host) = host_of(requested_url) else {
            return false;
        };
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let had_streak = guard.wipe_streak.remove(&host).is_some();
        let had_total = guard.wipe_total.remove(&host).is_some();
        // 只解抹除成因的冷却,频控冷却不动(频控靠时间,与本环境恢复无关)。
        if guard.cooldown_cause.get(host.as_str()) == Some(&CAUSE_SITE_WIPE) {
            guard.cooldown_until.remove(host.as_str());
            guard.cooldown_cause.remove(host.as_str());
        }
        had_streak || had_total
    }

    /// 该域是否已达 laew 身份持久熔断(总账 ≥ 阈值)。
    pub fn wipe_exhausted(&self, host: &str) -> bool {
        self.wipe_total_for(host) >= wipe_total_threshold()
    }

    /// 该域当前抹除连击数(2026-10-14 第三轮,供「连击域跳过徒劳重导航」判定)。
    pub fn wipe_streak_for(&self, host: &str) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wipe_streak
            .get(host)
            .copied()
            .unwrap_or(0)
    }

    /// 该域是否已被证实整页抹除(连击 ≥1 或冷却中)——**已证实的抹除域**再导航
    /// 大概率照样被抹:① 跳过「首航落 blank 原地重导航」自愈(那是给瞬态 WAF
    /// 首击设计的,对抹除域只是再吃一次风控请求、多闪一次窗口);② 响应附
    /// 局势提示。瞬态首击(streak=0 且无冷却)不受影响,自愈照常。
    pub fn wipe_flagged(&self, host: &str) -> bool {
        self.wipe_streak_for(host) >= 1 || self.cooldown_remaining_for(host).is_some()
    }

    /// 该域抹除账本快照(2026-10-14 第三轮):总账 >0 才返回 `Some`,供 open/navigate
    /// 响应附 `site_wipe_ledger` 字段 —— 落地成功但域有抹除史时,LLM 一眼看到
    /// 「该域已被抹 N 次」的局势,不再按正常站点推进(实测:总账只在 6004 拦截
    /// 或当场抹除时可见,成功落地时模型对风险零感知)。
    pub fn wipe_ledger_snapshot(&self, host: &str) -> Option<serde_json::Value> {
        let total = self.wipe_total_for(host);
        if total == 0 {
            return None;
        }
        let cooldown_secs = self
            .cooldown_remaining_for(host)
            .map(|d| d.as_secs().max(1))
            .unwrap_or(0);
        Some(serde_json::json!({
            "total": total,
            "streak": self.wipe_streak_for(host),
            "threshold": wipe_total_threshold(),
            "cooldown_remaining_secs": cooldown_secs,
            "note": "该域曾对本环境整页抹除(站点反爬):本页当前正常不代表安全,数秒后仍可能被抹;\
多次抹除达阈值后 laew 身份导航将被持久熔断;采集到的数据应尽快落盘",
        }))
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

    /// 清空冷却与抹除连击(close(page_id="all") 回收浏览器时调用)。
    ///
    /// 全新 profile = 全新会话身份(新 cookie/新指纹),原会话的频控/抹除**连击**
    /// 不再适用;不清 `last_nav`(最小间隔节奏与身份无关)。
    ///
    /// **不清 `wipe_total` 总账**(2026-10-14 第二轮):指纹级打标站点(猎聘实测)
    /// 对 CDP 拉起环境本身打标,close(all)+全新 profile 照样被抹 —— 总账若随
    /// clear_all 清零,LLM 每 close 一次就重置熔断,形成「拉起 Chrome → 被抹 →
    /// close → 再拉起」的风暴(实测 9 分钟 8 台)。总账只在「页面存活过看护窗」
    /// ([`Self::note_navigation_survived`],后台看护复读落地 URL 仍非 blank)
    /// 时清零。
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
///
/// `laew_identity` = 本次导航的目标浏览器是否是 **laew 自起身份**(launch 模式
/// 全新实例 / auto_relaunch 调试实例)。指纹级打标站点(猎聘实测)对 CDP 拉起
/// 环境本身打标,`false`(接管用户日常 Chrome,真实指纹)通常不受此熔断影响。
///
/// 拦截文案按冷却成因分流:频控靠等(冷却后低频重试),抹除靠换身份(全新 profile
/// / 接管日常浏览器)——建议错位会把 LLM 引向无效等待(猎聘实测硬闯 25+ 次的教训)。
/// **持久熔断档**(总账 ≥ 阈值且 laew 身份):不再建议「close(all) 换全新
/// profile」—— 那正是 Chrome 拉起风暴的源头;只剩接管用户日常浏览器或人工。
pub async fn guard_navigation(
    url: &str,
    force: bool,
    laew_identity: bool,
) -> Option<crate::error::Result<String>> {
    if !enabled() || force {
        return None;
    }
    let host = host_of(url)?;
    // 持久熔断(2026-10-14 第二轮):总账达阈值且目标是 laew 自起身份 ——
    // 换全新 profile/新实例对该域已被实测无效,不再放行(close(all) 逃逸无效)。
    if laew_identity && WebPace::global().wipe_exhausted(&host) {
        return persistent_wipe_envelope(
            &host,
            WebPace::global().wipe_total_for(&host),
            wipe_total_threshold(),
        )
        .map(Ok);
    }
    match WebPace::global().decide(&host) {
        PaceDecision::Block { remaining } => {
            let secs = remaining.as_secs().max(1);
            let cause = WebPace::global()
                .cooldown_cause_for(&host)
                .unwrap_or(CAUSE_RATE_LIMITED);
            let (cooldown, advice, msg) = match cause {
                CAUSE_SITE_WIPE => (
                    wipe_cooldown_secs(),
                    "站点整页抹除(反爬,会话/指纹级打标)。**先别急着 close(all) 重开** —— \
每试一次多弹一台 Chrome 窗口,若该域是指纹级打标(对 CDP 拉起环境本身打标)换新 profile 照样被抹。\
可先试 1 次 close(page_id=\"all\") 回收浏览器换全新 profile(冷却自动解除)后重新 open;\
若再被抹除即判定指纹级打标,改用 open(reuse_existing=true) 接管本机日常浏览器(真实指纹+登录态),\
或 request_human 人工协助;已采集数据先落盘",
                    format!(
                        "站点整页抹除冷却中({host}):剩余 {secs}s;建议 close(page_id=\"all\") 回收浏览器换全新 profile 试 1 次(冷却自动解除);\
若再被抹除则换 open(reuse_existing=true) 接管日常浏览器或 request_human(指纹级打标时换新实例无效);\
确需原会话立即重试可 params.force=true",
                    ),
                ),
                // 第 172 轮:人工验证挑战/风控打标(baxia 滑块 punish 实测)——
                // 冷却期内对该域继续搜索/导航只会加剧风控(实测升级为会话级,
                // 连首页都被打标);人工完成验证后冷却已自动解除,走不到这里。
                CAUSE_RISK_CHALLENGE => (
                    risk_cooldown_secs(),
                    "站点人工验证挑战/风控打标(滑块 punish):冷却期内对该域继续搜索/导航\
只会加剧风控(实测升级后会话级打标,连首页都被拦截)。处置:① 等待冷却后低频重试;\
② request_human 让人工在浏览器完成验证(HITL 弹窗可切「🎮 侵入式」用触控板拖滑块),\
完成后冷却自动解除;③ 已采集数据先落盘;④ 换身份:close(page_id=\"all\") 回收浏览器\
(冷却自动解除)或 open(reuse_existing=true) 接管日常浏览器",
                    format!(
                        "站点风控挑战冷却中({host}):剩余 {secs}s;冷却期内不要对同域继续搜索/导航(会加剧风控);\
可 request_human 让人工完成验证(收口自动解除冷却)或等冷却后低频重试;\
确需立即导航可 params.force=true",
                    ),
                ),
                _ => (
                    rate_cooldown_secs(),
                    "站点频控/限流:不要对该站点继续连续导航/搜索(高频只会延长封锁);\
先做本地整理或其它页面/站点工作,冷却后低频重试;已采集数据先落盘。\
任何探针命中频控标记后工具层自动进入冷却",
                    format!(
                        "站点频控冷却中({host}):剩余 {secs}s 后放行;冷却期内不要对同站点连续导航/搜索;\
确需立即导航可 params.force=true",
                    ),
                ),
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

/// 持久熔断信封(纯函数,便于单测):`total >= threshold` 才构造,否则 `None`。
///
/// 2026-10-14 第三轮:附 `relaunch_command`(用户日常 Chrome 以调试端口重启的命令,
/// 纯字符串拼装)—— 之前模型要走一轮 3002 才拿到可转述命令,而日常 Chrome 无调试
/// 端口时它只会 auto_relaunch(又是 laew 身份实例,照样被抹),指引不闭环。
fn persistent_wipe_envelope(host: &str, total: u32, threshold: u32) -> Option<String> {
    if total < threshold {
        return None;
    }
    let relaunch_command = crate::agent::browser_reuse::relaunch_command_string(
        crate::agent::browser_reuse::debug_port(),
        &crate::agent::browser_reuse::debug_profile_dir(),
    );
    let msg = format!(
        "站点整页抹除持久熔断({host}):已对 laew 拉起环境累计抹除 {total} 次(≥{threshold}),\
判定为指纹级打标 —— close(all) 换全新 profile/调试实例重试均被实测无效且每试一次多弹一台 Chrome。\
只剩两条出路:① open(reuse_existing=true) 接管**用户日常 Chrome**(真实指纹+登录态,不受此熔断;\
未开调试端口时把 data.relaunch_command 转述给用户执行后再接管,不要用 auto_relaunch —— 那仍是 laew 身份);\
② request_human 如实交还现场由人工处理;已采集数据先落盘。确需硬闯可 params.force=true"
    );
    let data = json!({
        "host": host,
        "cause": CAUSE_SITE_WIPE,
        "persistent": true,
        "wipe_total": total,
        "wipe_total_threshold": threshold,
        "relaunch_command": relaunch_command,
        "relaunch_hint": "让用户以其日常 Chrome 执行 relaunch_command(独立调试 profile,不影响已打开窗口);\
完成后 open(reuse_existing=true) 即接管真实指纹环境",
        "advice": "该域已对 laew 拉起的浏览器环境(含全新 profile 与 auto_relaunch 调试实例)做指纹级打标,\
继续重试只会反复弹 Chrome 窗口并再次被抹除。处置:① 把 data.relaunch_command 转述给用户以其日常 Chrome 执行,\
再 open(reuse_existing=true) 接管(真实指纹+登录态,不受此熔断;auto_relaunch 无效,别再试);\
② request_human 人工协助;已采集数据先落盘。确需立即导航可 params.force=true",
        "force_hint": "确认必须立即导航时,可在 params 加 force=true 跳过节奏闸强制执行",
    });
    serde_json::to_string(&json!({
        "code": CODE_SITE_PACE_COOLDOWN,
        "message": msg,
        "data": data,
    }))
    .ok()
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
    fn 整页抹除_落地非blank不清账_存活过看护窗才清零() {
        let p = pace();
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        // 2026-10-14 第三轮:落地非 blank 不清账(抹除发生在其后 4~10s,落地成功
        // 只是假恢复 —— 清掉会让后台补记账归零、熔断永不触发)。
        assert_eq!(
            p.note_navigation_outcome("https://www.liepin.com/", "https://www.liepin.com/"),
            None
        );
        assert_eq!(p.wipe_streak_for("liepin.com"), 1, "落地成功不清连击");
        // 后台看护复读仍存活 = 真实恢复证据 → 清零
        assert!(p.note_navigation_survived("https://www.liepin.com/"));
        assert_eq!(p.wipe_streak_for("liepin.com"), 0);
        assert_eq!(p.wipe_total_for("liepin.com"), 0);
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

    // ==================== 2026-10-14 第三轮:后台补记账 + 局势可见性 ====================

    #[test]
    fn wipe_flagged_连击或冷却成立为真_存活恢复后为假() {
        let p = pace();
        assert!(!p.wipe_flagged("liepin.com"), "无任何抹除记录不应判定为抹除域");
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        assert!(p.wipe_flagged("liepin.com"), "连击 1 即已证实抹除(跳过重导航自愈)");
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        assert!(p.wipe_flagged("liepin.com"), "冷却中同样为真");
        // 存活过看护窗 = 真实恢复:连击清零后(冷却未到)不再判定为抹除域,
        // 瞬态首击自愈照常。
        p.note_navigation_survived("https://www.liepin.com/");
        assert!(!p.wipe_flagged("liepin.com"));
    }

    #[test]
    fn 账本快照_总账零时无字段_有史时含总数阈值与冷却() {
        let _g = env_lock(); // env 解析测试并行会改阈值变量,读者同样持锁防竞态
        let p = pace();
        assert!(p.wipe_ledger_snapshot("liepin.com").is_none());
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        let snap = p.wipe_ledger_snapshot("liepin.com").expect("有抹除史应有快照");
        assert_eq!(snap["total"], 1);
        assert_eq!(snap["streak"], 1);
        assert_eq!(snap["threshold"], wipe_total_threshold());
        // 存活过看护窗(恢复)清总账 → 快照恢复为 None
        assert!(p.note_navigation_survived("https://www.liepin.com/"));
        assert!(p.wipe_ledger_snapshot("liepin.com").is_none());
    }

    /// 猎聘实测总账轨迹 0→1→0→1→2→0 的复演:补记账(后台看护)后「落地成功→
    /// 数秒后被抹」序列必须单调累积,持久熔断如期在阈值处触发。
    #[test]
    fn 补记账语义复演_落地假恢复不清账_攒到阈值熔断() {
        let _g = env_lock(); // 同 账本快照 注:阈值变量读者持锁防并行竞态
        let p = pace();
        for _ in 0..3 {
            // 每次导航:open 结算落地非 blank(不清账)→ 10s 后看护判定被抹(+1)
            p.note_navigation_outcome("https://www.liepin.com/", "https://www.liepin.com/");
            p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        }
        assert_eq!(p.wipe_total_for("liepin.com"), 3, "补记账后总账应单调累积");
        assert!(p.wipe_exhausted("liepin.com"), "累计达阈值即熔断");
        let env = persistent_wipe_envelope("liepin.com", 3, wipe_total_threshold())
            .expect("达阈值构造熔断信封");
        let v: serde_json::Value = serde_json::from_str(&env).unwrap();
        assert_eq!(v["data"]["persistent"], true);
        // 2026-10-14 第三轮:熔断信封必须附可转述给用户的重启命令
        let cmd = v["data"]["relaunch_command"].as_str().unwrap_or("");
        assert!(!cmd.trim().is_empty(), "relaunch_command 不得为空");
        assert!(
            v["data"]["advice"].as_str().unwrap_or("").contains("relaunch_command"),
            "advice 须指引转述 relaunch_command"
        );
        // 真实恢复(存活过看护窗)后总账清零,熔断解除 —— 恢复路径不被卡死
        p.note_navigation_survived("https://www.liepin.com/");
        assert!(!p.wipe_exhausted("liepin.com"));
    }

    #[test]
    fn 熔断信封_未达阈值为空() {
        assert!(persistent_wipe_envelope("liepin.com", 2, wipe_total_threshold()).is_none());
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
    fn 整页抹除_clear_all清冷却与连击_但总账保留() {
        let p = pace();
        p.note_nav_for("liepin.com");
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        assert!(p.cooldown_remaining_for("liepin.com").is_some());
        assert_eq!(p.wipe_total_for("liepin.com"), 2);
        // close(all) 回收浏览器:冷却/连击清零(新身份的当次 blank 不再直接熔断),
        // 但跨实例总账保留 —— 根治「close(all) 逃逸熔断再拉一台 Chrome」的风暴。
        p.clear_all();
        assert_eq!(p.cooldown_remaining_for("liepin.com"), None);
        assert_eq!(p.cooldown_cause_for("liepin.com"), None);
        assert_eq!(p.wipe_total_for("liepin.com"), 2, "总账不被 clear_all 清除");
        let o = p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        assert_eq!(o.map(|x| (x.streak, x.cooldown_triggered)), Some((1, false)));
        assert_eq!(p.wipe_total_for("liepin.com"), 3);
    }

    #[test]
    fn 整页抹除_总账达阈值后laew身份持久熔断_用户身份不受影响() {
        let _g = env_lock(); // 阈值变量读者持锁防与 env 解析测试并行竞态(基线既有偶发)
        let p = pace();
        for _ in 0..wipe_total_threshold() {
            p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        }
        assert!(p.wipe_exhausted("liepin.com"));
        // 持久熔断信封:laew 身份被拦(6004,cause=site_wipe,persistent=true)
        let env = persistent_wipe_envelope(
            "liepin.com",
            p.wipe_total_for("liepin.com"),
            wipe_total_threshold(),
        )
        .expect("达阈值应构造持久熔断信封");
        let v: serde_json::Value = serde_json::from_str(&env).unwrap();
        assert_eq!(v["code"], CODE_SITE_PACE_COOLDOWN);
        assert_eq!(v["data"]["cause"], CAUSE_SITE_WIPE);
        assert_eq!(v["data"]["persistent"], true);
        // 未达阈值不构造信封
        assert!(persistent_wipe_envelope("liepin.com", 1, wipe_total_threshold()).is_none());
    }

    #[test]
    fn 整页抹除_存活过看护窗后总账清零恢复() {
        let _g = env_lock(); // 同 账本快照 注:阈值变量读者持锁防并行竞态
        let p = pace();
        for _ in 0..wipe_total_threshold() {
            p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        }
        assert!(p.wipe_exhausted("liepin.com"));
        // 仅「落地成功」不算恢复(抹除发生在其后 4~10s):
        assert_eq!(
            p.note_navigation_outcome("https://www.liepin.com/", "https://www.liepin.com/"),
            None
        );
        assert!(p.wipe_exhausted("liepin.com"), "落地成功不解除熔断");
        // 恢复证据(后台看护复读仍存活,如接管用户日常 Chrome 后稳定落地):
        // 总账与抹除冷却一并清零
        assert!(p.note_navigation_survived("https://www.liepin.com/"));
        assert!(!p.wipe_exhausted("liepin.com"));
        assert_eq!(p.wipe_total_for("liepin.com"), 0);
        assert_eq!(p.cooldown_remaining_for("liepin.com"), None);
    }

    #[test]
    fn 整页抹除_总账跨不同浏览器实例累计() {
        let p = pace();
        // 实例 1 被抹 2 次 → close(all) → 实例 2 再被抹:总账连续累计
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        p.note_navigation_outcome("https://www.liepin.com/", "about:blank");
        p.clear_all();
        p.note_navigation_outcome("https://www.liepin.com/zhaopin/", "about:blank");
        assert_eq!(p.wipe_total_for("liepin.com"), 3);
        assert!(p.wipe_exhausted("liepin.com"));
        // 其它域的总账互不影响
        assert_eq!(p.wipe_total_for("jd.com"), 0);
    }

    #[test]
    fn 整页抹除_总账阈值env解析与回退() {
        let _g = env_lock();
        unsafe { std::env::remove_var("LAEW_WEB_WIPE_TOTAL_THRESHOLD") };
        assert_eq!(wipe_total_threshold(), WIPE_TOTAL_THRESHOLD_DEFAULT);
        unsafe { std::env::set_var("LAEW_WEB_WIPE_TOTAL_THRESHOLD", "5") };
        assert_eq!(wipe_total_threshold(), 5);
        unsafe { std::env::set_var("LAEW_WEB_WIPE_TOTAL_THRESHOLD", "0") };
        assert_eq!(wipe_total_threshold(), WIPE_TOTAL_THRESHOLD_DEFAULT, "0 回退默认");
        unsafe { std::env::set_var("LAEW_WEB_WIPE_TOTAL_THRESHOLD", "bad") };
        assert_eq!(wipe_total_threshold(), WIPE_TOTAL_THRESHOLD_DEFAULT, "非法回退默认");
        unsafe { std::env::remove_var("LAEW_WEB_WIPE_TOTAL_THRESHOLD") };
    }

    /// 环境变量类测试用互斥锁串行(对齐 browser_reuse.rs 既有模式):
    /// 多个测试并行跑时 set_var/remove_var 互相踩,全量 cargo test 偶发失败。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
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

    // 第 172 轮:风控挑战(baxia 滑块 punish)域级冷却 —— 天猫实测:挑战后立刻
    // 重试搜索会把风控升级为会话级(连首页都被打标),需要工具层止动。
    #[test]
    fn 风控挑战_标记后同域导航被拦_成因正确() {
        let p = pace();
        p.mark_risk_challenge_for("taobao.com", Duration::from_secs(300));
        assert_eq!(p.cooldown_cause_for("taobao.com"), Some(CAUSE_RISK_CHALLENGE));
        assert!(
            matches!(p.decide("taobao.com"), PaceDecision::Block { .. }),
            "风险冷却期内同域导航应被拦"
        );
        assert!(
            matches!(p.decide("jd.com"), PaceDecision::Allow),
            "冷却按注册域隔离,不连坐其它站点"
        );
    }

    #[test]
    fn 风控挑战_解除只清risk成因_不动频控与抹除() {
        let p = pace();
        // 频控成因:解除 risk 不得动它
        p.mark_rate_limited_for("jd.com", Duration::from_secs(240));
        p.clear_risk_challenge("jd.com");
        assert_eq!(p.cooldown_cause_for("jd.com"), Some(CAUSE_RATE_LIMITED));
        // risk 成因:解除后立即放行
        p.mark_risk_challenge_for("taobao.com", Duration::from_secs(300));
        p.clear_risk_challenge("taobao.com");
        assert_eq!(p.cooldown_cause_for("taobao.com"), None);
        assert!(
            matches!(p.decide("taobao.com"), PaceDecision::Allow),
            "解除后同域导航立即恢复"
        );
    }

    #[test]
    fn 风控挑战_晚到的频控标记覆盖risk成因() {
        let p = pace();
        p.mark_risk_challenge_for("taobao.com", Duration::from_secs(300));
        // 更晚截止的频控标记(240s < 300s 不会覆盖;用更长的显式时长模拟)
        p.mark_rate_limited_for("taobao.com", Duration::from_secs(600));
        assert_eq!(p.cooldown_cause_for("taobao.com"), Some(CAUSE_RATE_LIMITED));
        // 反向:risk 标记比既有频控短也不覆盖
        let q = pace();
        q.mark_rate_limited_for("taobao.com", Duration::from_secs(600));
        q.mark_risk_challenge_for("taobao.com", Duration::from_secs(300));
        assert_eq!(q.cooldown_cause_for("taobao.com"), Some(CAUSE_RATE_LIMITED));
    }

    #[test]
    fn 风控挑战_clear_all同样解除() {
        // close(all) 换全新身份 = 风险标记不再适用(与频控/抹除连击同语义)
        let p = pace();
        p.mark_risk_challenge_for("taobao.com", Duration::from_secs(300));
        p.clear_all();
        assert_eq!(p.cooldown_cause_for("taobao.com"), None);
    }

    #[test]
    fn 风控挑战_冷却时长环境变量可配() {
        let _g = env_lock();
        unsafe { std::env::remove_var("LAEW_WEB_RISK_COOLDOWN_SECS") };
        assert_eq!(risk_cooldown_secs(), 300, "缺省 300s");
        unsafe { std::env::set_var("LAEW_WEB_RISK_COOLDOWN_SECS", "60") };
        assert_eq!(risk_cooldown_secs(), 60);
        unsafe { std::env::set_var("LAEW_WEB_RISK_COOLDOWN_SECS", "0") };
        assert_eq!(risk_cooldown_secs(), 300, "0 回退默认");
        unsafe { std::env::set_var("LAEW_WEB_RISK_COOLDOWN_SECS", "bad") };
        assert_eq!(risk_cooldown_secs(), 300, "非法回退默认");
        unsafe { std::env::remove_var("LAEW_WEB_RISK_COOLDOWN_SECS") };
    }
}
