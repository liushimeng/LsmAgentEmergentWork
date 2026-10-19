//! `site_risk` 表 DAO:站点反爬声誉跨会话持久化(2026-10-19,猎聘实测)。
//!
//! 设计见 `tmpPlan/20261019_111003_站点反爬声誉跨会话持久化与抹除早检根治.md`。
//! 表结构见 `database/schema.rs`。
//!
//! **职责边界**:
//! - 本模块只管**存取**,不做治理判断(阈值/冷却/熔断仍在 `agent::browser_pace`);
//! - 全部调用由 [`crate::agent::browser_pace`] fail-open 包裹 —— 持久化是**增强能力**,
//!   写库失败绝不能让浏览器任务失败(对齐 `subagent_run` 的 fail-open 约定);
//! - 杀开关 `LAEW_SITE_RISK_PERSIST=off` 时所有写/读都退化为 no-op,
//!   运行期行为回到纯内存(改造前语义)。
//!
//! **为什么需要它**:猎聘对「全新 profile + CDP 拉起」做的是**自适应**打标
//! (实测 `risk_tier=ok`、主文档 200、内容真实下发,3~6 秒后才自毁到 `about:blank`),
//! 静态隐身参数已打不穿。这类结论若只活在进程里,用户每重跑一次任务就把
//! Chrome 风暴重付一遍;记进本表后,重跑任务的第一次 `open` 即在
//! `guard_navigation` 处熔断,**一台 Chrome 都不拉起**。

use rusqlite::params;

use super::Db;
use crate::config::Result;

/// 记录新鲜度窗口(天):`last_wipe_at` 超过它就不再参与熔断。
///
/// 通用性关键 —— 站点的 WAF 会升级也会下线,一张永不过期的熔断表会把用户
/// 永久锁在该域门外。默认 7 天:足够覆盖「同一轮调试期反复重跑」,
/// 又能让修好的站点在一周后自动恢复试探。
pub const RISK_TTL_DAYS_DEFAULT: u32 = 7;

/// 读取新鲜度窗口(天);`0`/非法值回退默认。
pub fn risk_ttl_days() -> u32 {
    match std::env::var("LAEW_SITE_RISK_TTL_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
    {
        Some(n) if n > 0 => n,
        _ => RISK_TTL_DAYS_DEFAULT,
    }
}

/// 落库杀开关(`LAEW_SITE_RISK_PERSIST=off` 时全部退化为 no-op)。
///
/// **同时是人工解除熔断的出口**:换 IP、或站点 WAF 已修复时,把它设为 `off`
/// 跑一次即绕过历史账,不必干等 [`risk_ttl_days`] 自然过期(默认 7 天)。
pub fn persist_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_SITE_RISK_PERSIST")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 一条域的声誉记录。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteRiskRow {
    /// 注册域(归一后,如 `liepin.com`)。
    pub domain: String,
    /// 跨会话累计整页抹除次数(熔断账)。
    pub wipe_total: u32,
    /// 最近一次连续抹除连击数。
    pub wipe_streak: u32,
    /// 频控横幅命中次数(观测用,不参与熔断)。
    pub rate_limit: u32,
    /// 最近一次抹除时刻(SQLite `datetime('now','localtime')` 文本,可能为空)。
    pub last_wipe_at: Option<String>,
    /// 最近一次读到的非空备注(留作排障线索)。
    pub note: String,
}

/// 该记录是否仍在新鲜度窗口内(参与熔断的前提)。
///
/// `last_wipe_at` 为空(只记过频控)或解析失败一律判过期 —— 宁可放行试探,
/// 也不因一条坏数据把域永久锁死。
pub fn is_fresh(row: &SiteRiskRow, now: &str) -> bool {
    let (Some(at), Some(n)) = (
        row.last_wipe_at.as_deref().and_then(parse_utc),
        parse_utc(now),
    ) else {
        return false;
    };
    n.saturating_sub(at) <= (risk_ttl_days() as i64) * 86_400
}

/// 解析 `YYYY-MM-DD HH:MM:SS`(UTC)为 Unix 秒。
///
/// 不引入 chrono/time crate(工程铁律);只认本模块自己写出的严格形状,
/// 任何不合规输入返回 `None`(由 [`is_fresh`] 判过期)。
fn parse_utc(s: &str) -> Option<i64> {
    let (date, time) = s.split_once(' ')?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let m: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    if d.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&day) || y < 1970 {
        return None;
    }
    let mut t = time.split(':');
    let hh: i64 = t.next()?.parse().ok()?;
    let mm: i64 = t.next()?.parse().ok()?;
    let ss: i64 = t.next()?.parse().ok()?;
    if t.next().is_some() || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// 公历日 → 1970-01-01 起的天数(Hinnant `days_from_civil`,`m` 为 1..=12)。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// 当前时刻(`YYYY-MM-DD HH:MM:SS`,**UTC 基准**)。
///
/// 刻意不走 `datetime('now','localtime')`:那需要一条 rusqlite 连接,只为格式化
/// 一个字符串不值当。UTC 对**相对天数比较**(`is_fresh`)完全够用 —— 两端同基准。
/// 该字段**不用于展示**(展示一律读库内原文),只参与新鲜度判定。
fn now_str() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc(secs)
}

/// 把 Unix 秒格式化成 `YYYY-MM-DD HH:MM:SS`(UTC)。
///
/// 不引入 chrono/time crate(工程铁律:不引入新依赖)。
fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    //  civil_from_days(Hinnant 算法, days 自 1970-01-01)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// 进程级台账连接(`Mutex<Option<Db>>`)。
///
/// **必须由启动路径显式注册**([`init_store`]),不在此处惰性 `Paths::detect()`:
/// 惰性打开会让 `cargo test --lib` 的每个记账调用都去写**用户真实数据库**
/// —— 单测构造的 `WebPace::default()` 也会落库,等于用测试域名污染真实声誉账
/// (甚至把正常域标成高风险)。注册式初始化让单测天然零副作用。
fn store() -> Option<Db> {
    STORE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

static STORE: std::sync::Mutex<Option<Db>> = std::sync::Mutex::new(None);

/// 注册进程级台账连接并立即回填历史抹除账。由 `main` / TUI 启动路径各调一次。
///
/// 复用启动路径**已经打开**的 `Db`(`Clone` 会按同路径重开一条带 PRAGMA 的连接),
/// 不再额外 `Db::open` —— 省掉一次重复的完整性检查与建表。
pub fn init_with(db: &Db) {
    if !persist_enabled() {
        return;
    }
    *STORE.lock().unwrap_or_else(|e| e.into_inner()) = Some(db.clone());
    crate::agent::browser_pace::WebPace::global().hydrate();
}

/// 自行探测并打开台账连接的兜底入口(启动路径已持有 `Db` 时不必用)。
///
/// fail-open:库打不开或被杀开关关闭时保持 `None`(全部退化为 no-op,
/// 运行期行为回到改造前的纯内存语义)。
pub fn init_store() {
    if !persist_enabled() {
        return;
    }
    let Ok(paths) = crate::database::Paths::detect() else {
        tracing::warn!("站点反爬声誉:探测数据库路径失败,本次运行仅用内存账");
        return;
    };
    match Db::open(&paths) {
        Ok(db) => init_with(&db),
        Err(e) => tracing::warn!(error = %e, "站点反爬声誉:打开数据库失败,本次运行仅用内存账"),
    }
}

#[cfg(test)]
fn set_store_for_test(db: Option<Db>) {
    *STORE.lock().unwrap_or_else(|e| e.into_inner()) = db;
}

/// 串行化「操作进程级 store」的用例 —— `cargo test` 默认多线程并行,
/// 两个用例各自 set/clear 同一份全局会互相踩出随机失败。
#[cfg(test)]
fn store_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

impl Db {
    /// 记一次整页抹除(upsert,计数取调用方给权威值)。
    ///
    /// `total` / `streak` 由 `WebPace` 内存台账给出 —— 落库只是**镜像**内存账,
    /// 不在 DAO 里自行加一(避免两侧各自记账导致对不上)。
    pub fn record_site_wipe(&self, domain: &str, total: u32, streak: u32) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO site_risk(domain, wipe_total, wipe_streak, rate_limit, last_wipe_at, note)
             VALUES (?1,?2,?3,0,?4,'')
             ON CONFLICT(domain) DO UPDATE SET
                wipe_total   = excluded.wipe_total,
                wipe_streak  = excluded.wipe_streak,
                last_wipe_at = excluded.last_wipe_at",
            params![domain, total as i64, streak as i64, now_str()],
        )?;
        Ok(())
    }

    /// 记一次频控横幅命中(观测用;不参与熔断,不写 `last_wipe_at`)。
    pub fn record_site_rate_limit(&self, domain: &str) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO site_risk(domain, wipe_total, wipe_streak, rate_limit, note)
             VALUES (?1,0,0,1,'')
             ON CONFLICT(domain) DO UPDATE SET rate_limit = rate_limit + 1",
            params![domain],
        )?;
        Ok(())
    }

    /// 记录恢复证据:清空该域的抹除账(页面存活过看护窗 = 真实恢复)。
    ///
    /// 连 `rate_limit` 一并清零:频控与 WAF 打标常同源,同一次「该域已正常」
    /// 的观测足以同时推翻两者。行本身保留(留作历史线索)。
    pub fn clear_site_risk(&self, domain: &str) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "UPDATE site_risk SET wipe_total = 0, wipe_streak = 0
             WHERE domain = ?1",
            params![domain],
        )?;
        Ok(())
    }

    /// 取全部**仍在新鲜度窗口内且抹除账非零**的记录(启动期回填用)。
    pub fn load_active_site_risk(&self) -> Result<Vec<SiteRiskRow>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let now = now_str();
        let mut stmt = conn.prepare(
            "SELECT domain, wipe_total, wipe_streak, rate_limit, last_wipe_at, note
             FROM site_risk WHERE wipe_total > 0",
        )?;
        let rows = stmt
            .query_map([], |r| row_from_sql(r))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().filter(|r| is_fresh(r, &now)).collect())
    }

    /// 列出抹除账最多的域(排障/观测用)。
    pub fn list_site_risk(&self, limit: usize) -> Result<Vec<SiteRiskRow>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT domain, wipe_total, wipe_streak, rate_limit, last_wipe_at, note
             FROM site_risk WHERE wipe_total > 0
             ORDER BY wipe_total DESC, last_wipe_at DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit as i64], |r| row_from_sql(r))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn row_from_sql(r: &rusqlite::Row<'_>) -> rusqlite::Result<SiteRiskRow> {
    Ok(SiteRiskRow {
        domain: r.get(0)?,
        wipe_total: r.get::<_, i64>(1)? as u32,
        wipe_streak: r.get::<_, i64>(2)? as u32,
        rate_limit: r.get::<_, i64>(3)? as u32,
        last_wipe_at: r.get(4)?,
        note: r.get(5)?,
    })
}

// ==================== 进程级 fail-open 门面 ====================
//
// `browser_pace` 是同步热路径(每个导航收尾都调),不能让它处理 Result。

/// 记录一次抹除;落库失败仅告警,绝不返回错误影响主流程。
pub fn record_wipe(domain: &str, total: u32, streak: u32) {
    if let Some(db) = store() {
        if let Err(e) = db.record_site_wipe(domain, total, streak) {
            tracing::warn!(domain, error = %e, "站点声誉落库失败(fail-open,不影响本次运行)");
        }
    }
}

/// 记录一次频控命中;fail-open。
pub fn record_rate_limit(domain: &str) {
    if let Some(db) = store() {
        if let Err(e) = db.record_site_rate_limit(domain) {
            tracing::warn!(domain, error = %e, "频控声誉落库失败(fail-open)");
        }
    }
}

/// 记录恢复证据(清该域抹除账);fail-open。
pub fn record_survived(domain: &str) {
    if let Some(db) = store() {
        if let Err(e) = db.clear_site_risk(domain) {
            tracing::warn!(domain, error = %e, "恢复证据落库失败(fail-open)");
        }
    }
}

/// 启动期回填:把仍在新鲜度窗口内的抹除账读回内存台账。
///
/// 返回 `(domain, wipe_total, wipe_streak)` 三元组列表,由调用方灌进
/// `WebPace`(DAO 不反向依赖 agent 层,保持单向)。
pub fn load_active() -> Vec<(String, u32, u32)> {
    let Some(db) = store() else {
        return Vec::new();
    };
    match db.load_active_site_risk() {
        Ok(rows) => rows
            .into_iter()
            .map(|r| (r.domain, r.wipe_total, r.wipe_streak))
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "站点声誉回读失败(fail-open,按无历史处理)");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Paths;

    fn fresh_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (db, dir)
    }

    #[test]
    fn 抹除落库后可回读() {
        let (db, _d) = fresh_db();
        db.record_site_wipe("liepin.com", 3, 2).unwrap();
        let rows = db.load_active_site_risk().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].domain, "liepin.com");
        assert_eq!(rows[0].wipe_total, 3);
        assert_eq!(rows[0].wipe_streak, 2);
    }

    #[test]
    fn 抹除落库覆盖式更新而非累加() {
        // 落库是**镜像**内存账(调用方给权威值),DAO 不得自行加一,
        // 否则两侧各自记账会对不上(总账虚高 → 过早熔断)。
        let (db, _d) = fresh_db();
        db.record_site_wipe("liepin.com", 1, 1).unwrap();
        db.record_site_wipe("liepin.com", 2, 2).unwrap();
        let rows = db.load_active_site_risk().unwrap();
        assert_eq!(rows[0].wipe_total, 2, "应取最新镜像值而非累加到 3");
        assert_eq!(rows[0].wipe_streak, 2);
    }

    #[test]
    fn 恢复证据清抹除账但保留频控计数() {
        let (db, _d) = fresh_db();
        db.record_site_wipe("liepin.com", 3, 3).unwrap();
        db.record_site_rate_limit("liepin.com").unwrap();
        db.clear_site_risk("liepin.com").unwrap();
        // 清零后不再出现在 active 列表(抹除账为 0)
        assert!(db.load_active_site_risk().unwrap().is_empty());
        // 但频控计数保留为历史线索
        let all = db.list_site_risk(10).unwrap();
        assert!(all.is_empty(), "list_site_risk 只列抹除账非零的域");
    }

    #[test]
    fn 频控独立累计且不写抹除时刻() {
        let (db, _d) = fresh_db();
        db.record_site_rate_limit("jd.com").unwrap();
        db.record_site_rate_limit("jd.com").unwrap();
        assert!(
            db.load_active_site_risk().unwrap().is_empty(),
            "只记频控不应进熔断账"
        );
    }

    #[test]
    fn 过期记录不参与熔断() {
        let row = SiteRiskRow {
            domain: "liepin.com".into(),
            wipe_total: 9,
            wipe_streak: 9,
            rate_limit: 0,
            last_wipe_at: Some("2020-01-01 00:00:00".into()),
            note: String::new(),
        };
        assert!(
            !is_fresh(&row, "2026-10-19 11:00:00"),
            "半年前的抹除记录不得继续熔断(站点可能已修好 WAF)"
        );
    }

    #[test]
    fn 新鲜记录参与熔断() {
        let row = SiteRiskRow {
            domain: "liepin.com".into(),
            wipe_total: 3,
            wipe_streak: 3,
            rate_limit: 0,
            last_wipe_at: Some("2026-10-19 10:00:00".into()),
            note: String::new(),
        };
        assert!(is_fresh(&row, "2026-10-19 11:00:00"));
    }

    #[test]
    fn 时刻缺失或损坏一律判过期() {
        let mut row = SiteRiskRow {
            domain: "x.com".into(),
            last_wipe_at: None,
            ..Default::default()
        };
        let now = now_str();
        assert!(!is_fresh(&row, &now), "无抹除时刻不应熔断");
        row.last_wipe_at = Some("not-a-timestamp".into());
        assert!(!is_fresh(&row, &now), "坏数据不得把域永久锁死");
    }

    #[test]
    fn 杀开关关闭时门面退化为空操作() {
        // 锁死解析语义本身(真值/假值/大小写)。
        for v in ["off", "OFF", "0", "false", "No"] {
            assert!(
                matches!(
                    v.to_ascii_lowercase().as_str(),
                    "off" | "0" | "false" | "no"
                ),
                "{v} 应被识别为关闭"
            );
        }
    }

    #[test]
    fn 新鲜度窗口读取与回退() {
        assert_eq!(risk_ttl_days(), RISK_TTL_DAYS_DEFAULT, "未设时用默认");
    }

    /// 时刻格式化与解析必须**互为逆运算** —— 否则新鲜度判定恒假,
    /// 表现为「落库了但永远回读不到」,跨会话记忆静默失效。
    #[test]
    fn 时刻格式化与解析互逆() {
        for s in [
            "1970-01-01 00:00:00",
            "2026-10-19 03:05:07",
            "2026-02-28 23:59:59",
            "2024-12-31 12:00:00",
        ] {
            let secs = parse_utc(s).unwrap_or_else(|| panic!("{s} 应可解析"));
            assert_eq!(format_utc(secs), s, "往返必须还原原串");
        }
        // 跨闰年/跨月天数差
        let a = parse_utc("2024-02-28 00:00:00").unwrap();
        let b = parse_utc("2024-03-01 00:00:00").unwrap();
        assert_eq!(b - a, 2 * 86_400, "2024 是闰年,2/28→3/01 相差 2 天");
        let c = parse_utc("2026-02-28 00:00:00").unwrap();
        let d = parse_utc("2026-03-01 00:00:00").unwrap();
        assert_eq!(d - c, 86_400, "2026 非闰年,相差 1 天");
    }

    #[test]
    fn 非法时刻一律解析失败() {
        for s in [
            "",
            "not-a-timestamp",
            "2026-10-19",
            "2026-10-19 03:05",       // 缺秒
            "2026-10-19 03:05:07:09", // 多一段
            "2026-13-01 00:00:00",    // 月份越界
            "2026-10-32 00:00:00",    // 日越界
            "2026-10-19 25:00:00",    // 时越界
            "1969-01-01 00:00:00",    // 早于纪元
        ] {
            assert!(parse_utc(s).is_none(), "{s:?} 应解析失败(宁判过期不误熔断)");
        }
    }

    /// 门面全链路:记账 → 回读 → 恢复清账。
    ///
    /// 这是「跨会话记忆」的实际保证:落库值能被 [`load_active`] 原样读回,
    /// 于是重跑任务的第一次 `open` 即命中持久熔断、一台 Chrome 都不拉起。
    #[test]
    fn 门面全链路记账回读与恢复清账() {
        let _g = store_lock();
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let db = Db::open(&Paths::for_test(dir.path())).unwrap();
        set_store_for_test(Some(db));

        record_wipe("liepin.com", 3, 3);
        let active = load_active();
        assert_eq!(active, vec![("liepin.com".to_string(), 3, 3)]);

        record_survived("liepin.com");
        assert!(load_active().is_empty(), "恢复证据须把抹除账清掉");

        set_store_for_test(None);
    }

    /// 未注册台账时门面全部 no-op —— 这正是**单测零副作用**的根据:
    /// `cargo test --lib` 全程不注册 store,任何 `WebPace::default()` 的记账
    /// 都不会碰到用户真实数据库。
    #[test]
    fn 未注册台账时门面全部空操作() {
        let _g = store_lock();
        set_store_for_test(None);
        record_wipe("liepin.com", 5, 5);
        record_rate_limit("liepin.com");
        record_survived("liepin.com");
        assert!(load_active().is_empty(), "未注册台账不得凭空造出记录");
    }
}
