//! 遍历访问台账(第 146 轮):MCP_Web_Use 进程级「去过哪些页面/去过几次」唯一事实源。
//!
//! 背景:实测 77 分钟网页遍历任务(`llaew_20261010_125445.log`),菜单 9 页只深访 6 页、
//! 同一批页面被反复遍历(R1 wf-3 与 R2 wf-2/3/4 大面积重测),而「哪些页面去过/没去过」
//! 系统层面无人知晓 —— SubAgent 只能自己发明 `ce_evidence/ledger/*.json` 约定,单元间
//! 不共享、QC 无法机械对账。本模块把访问记录下沉到工具层:
//!
//! - `record_visit`:open(新建/复用两路)/ navigate / new_tab 导航成功后记账
//!   (sequence/batch 经 `McpWebUseTool::execute` 分发,自动覆盖);
//! - `inspect(info=coverage)`:全局查询(distinct 页面数/总导航次数/每页访问计数),
//!   Plan/Main-Work/SubAgent/QC 零 LLM 成本获得覆盖清单;
//! - `visit_note`:同一 URL 访问 ≥3 次时在 open/navigate/new_tab 响应附打转警示;
//! - `footprint_since` / `seq`:单元执行前后取快照,delta 汇成 QC 的【机械导航足迹】。
//!
//! 生命周期:进程级全局(与 `BrowserManager::global()` 同构),跨 SubAgent 单元、跨
//! 档位重试轮存续;不落盘(会话级事实,进程重启即新任务)。

use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

/// 台账条目上限(防长任务无限膨胀;超出时丢弃最旧的**已结束**计数聚合不可行,
/// 改为:超限后新 URL 照记、最旧条目合并进 `overflow_dropped` 计数)。
const MAX_ENTRIES: usize = 512;

/// 重复访问警示阈值(同一 URL 累计 ≥N 次时附 visit_note)。
pub(crate) const REPEAT_NOTE_THRESHOLD: u32 = 3;

/// 单条页面记录(map 键 = 归一化 key,不再冗余存于条目内)。
#[derive(Debug, Clone)]
struct VisitEntry {
    /// 最近一次完整 URL(展示用)。
    last_url: String,
    /// 最近一次页面标题。
    title: String,
    visits: u32,
    /// 各来源计数:open / navigate / new_tab。
    from_open: u32,
    from_navigate: u32,
    from_new_tab: u32,
    /// 单调递增事件序号(footprint delta 用)。
    last_seq: u64,
    first_at_ms: u64,
    last_at_ms: u64,
    last_page_id: String,
}

/// 台账主体。
#[derive(Debug, Default)]
struct VisitLedger {
    entries: std::collections::HashMap<String, VisitEntry>,
    /// 滚动事件环(单调 seq → key):`footprint_since` 用它求**真增量**,
    /// 不受聚合表历史计数污染(单元前已访问 3 次的页面,单元内只应计本次)。
    recent: std::collections::VecDeque<(u64, String)>,
    /// 单调递增事件序号(先取值再自增,seq 即"已发生的事件数")。
    next_seq: u64,
    session_start_ms: u64,
    total_visits: u64,
    overflow_dropped: u32,
}

/// 滚动事件环上限(与条目上限同量级;超出丢最旧,只影响极长任务的远古足迹)。
const MAX_RECENT_EVENTS: usize = 512;

fn ledger_slot() -> &'static Mutex<VisitLedger> {
    static SLOT: OnceLock<Mutex<VisitLedger>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(VisitLedger::default()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 翻页 / 游标类 query 参数名(小写比较):这类参数**只改变同一页面的取数窗口**,
/// 不构成新页面,覆盖度里应折叠为同一个 key(第 146 轮的原意)。
///
/// 其余 query 参数**决定页面身份**,必须原样保留。
const PAGINATION_PARAMS: &[&str] = &[
    "page", "pageno", "pagenumber", "pagenum", "pageindex", "page_index", "p", "pn", "pi",
    "offset", "from", "start", "cursor", "sp", "scroll", "scrolltop", "pageno_", "s",
];

/// 该 query 参数是否为翻页/游标参数(可折叠)。
fn is_pagination_param(key: &str) -> bool {
    let k = key.trim().to_ascii_lowercase();
    PAGINATION_PARAMS.contains(&k.as_str())
}

/// 记录 key 归一化:去 `#fragment`、去尾 `/`,并**按翻页参数**折叠 query。
///
/// 第 163 轮修正(实测 `llaew_20261012_175823.log`):旧实现直接在 `?` 处截断,
/// 把**全部** query 折叠掉 —— 而搜索/分类驱动站点(闲鱼 `?q=`、电商 `?cat=`、
/// 论坛 `?t=`…)的页面身份**恰恰在 query 里**。闲鱼全场 37 个真实品类搜索页被
/// 塌缩成 3 个 key,`inspect(info=coverage)` 报 `distinct_pages=3`、
/// `total_navs=40`,而 QC 提示词明确要求「distinct_pages 不足即覆盖未达成」
/// (`agent/quality.rs`)—— 于是 QC 据此对真实遍历过的单元做出**错误指控**
/// (「全程仅访问 3 个不同页面 / 涉嫌编造」),并写进 AuditTrail 决策审计。
/// 反向副作用同样致命:同一批搜索页共享一个 key,`visit_note` 在 Agent 连续遍历
/// 时反复弹出「确认不是重复打转」(实测 38 次、计数涨到 49),台账自相矛盾。
///
/// 现改为**只折叠翻页类参数**,保留决定页面身份的全部参数;剩余参数按字典序排序
/// 使 key 与参数书写顺序无关。`?page=2` 与 `?q=手机` 仍是两个不同 key —— 前者只是
/// 同一页的第 2 页,后者是另一个品类的搜索结果页。
pub(super) fn visit_key(url: &str) -> String {
    let trimmed = url.trim();
    // 先去 fragment(fragment 自身可能含 `?`,不先切会误判为 query)。
    let s = match trimmed.find('#') {
        Some(i) => &trimmed[..i],
        None => trimmed,
    };
    let (base, query) = match s.find('?') {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let mut key = base.trim_end_matches('/').to_string();
    let Some(query) = query else { return key };
    // 保留决定页面身份的参数(翻页/游标参数折叠),排序后拼接保证 key 稳定。
    let mut kept: Vec<(String, String)> = Vec::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = match pair.find('=') {
            Some(i) => (&pair[..i], &pair[i + 1..]),
            None => (pair, ""),
        };
        if k.trim().is_empty() || is_pagination_param(k) {
            continue;
        }
        kept.push((k.to_string(), v.to_string()));
    }
    if kept.is_empty() {
        return key;
    }
    kept.sort();
    key.push('?');
    key.push_str(
        &kept
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&"),
    );
    key
}

/// 导航来源标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VisitSource {
    Open,
    Navigate,
    NewTab,
}

impl VisitSource {
    #[allow(dead_code)] // 保留给后续按来源过滤的覆盖查询
    fn as_str(self) -> &'static str {
        match self {
            VisitSource::Open => "open",
            VisitSource::Navigate => "navigate",
            VisitSource::NewTab => "new_tab",
        }
    }
}

/// 记录一次成功导航。fail-open:锁中毒/超限时静默丢弃,绝不影响工具主流程。
pub(super) fn record_visit(page_id: &str, url: &str, title: &str, source: VisitSource) {
    let key = visit_key(url);
    if key.is_empty() {
        return;
    }
    let mut led = match ledger_slot().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let now = now_ms();
    if led.session_start_ms == 0 {
        led.session_start_ms = now;
    }
    let seq = led.next_seq;
    led.next_seq += 1;
    led.total_visits += 1;
    led.recent.push_back((seq, key.clone()));
    while led.recent.len() > MAX_RECENT_EVENTS {
        led.recent.pop_front();
    }
    let entry = led.entries.entry(key).or_insert_with(|| VisitEntry {
        last_url: String::new(),
        title: String::new(),
        visits: 0,
        from_open: 0,
        from_navigate: 0,
        from_new_tab: 0,
        last_seq: 0,
        first_at_ms: now,
        last_at_ms: now,
        last_page_id: String::new(),
    });
    entry.last_url = url.trim().to_string();
    if !title.trim().is_empty() {
        entry.title = title.trim().to_string();
    }
    entry.visits += 1;
    match source {
        VisitSource::Open => entry.from_open += 1,
        VisitSource::Navigate => entry.from_navigate += 1,
        VisitSource::NewTab => entry.from_new_tab += 1,
    }
    entry.last_seq = seq;
    entry.last_at_ms = now;
    entry.last_page_id = page_id.to_string();
    // 超限兜底:直接丢最旧条目(计数并入 overflow_dropped),覆盖度查询仍如实。
    if led.entries.len() > MAX_ENTRIES {
        let oldest = led
            .entries
            .iter()
            .min_by_key(|(_, e)| e.last_at_ms)
            .map(|(k, _)| k.clone());
        if let Some(oldest) = oldest {
            led.entries.remove(&oldest);
            led.overflow_dropped += 1;
        }
    }
}

/// 当前事件序号(快照用:执行单元前取一次,单元后 `footprint_since` 求增量)。
pub(crate) fn seq() -> u64 {
    ledger_slot()
        .lock()
        .map(|l| l.next_seq)
        .unwrap_or(0)
}

/// 重复访问警示:该 URL 累计已访问次数 ≥ 阈值时返回提示文案,否则 None。
/// 调用点在导航**成功后**记完账再取,次数含本次。
pub(super) fn visit_note(url: &str) -> Option<Value> {
    let key = visit_key(url);
    if key.is_empty() {
        return None;
    }
    let led = ledger_slot().lock().ok()?;
    let e = led.entries.get(&key)?;
    if e.visits < REPEAT_NOTE_THRESHOLD {
        return None;
    }
    Some(json!({
        "visits": e.visits,
        "hint": format!(
            "该 URL 本次会话已访问 {} 次,确认不是重复打转;未访问页面优先,覆盖清单见 inspect(info=coverage)",
            e.visits
        ),
    }))
}

/// 覆盖清单查询(inspect(info=coverage) 的 data 载荷)。
/// 按 last_at 倒序、上限 60 条;summary 提供聚合口径。
pub(crate) fn coverage_payload() -> Value {
    let guard = ledger_slot().lock();
    // (key, entry) 成对取出(排序 tie-break 用 key;entry 不再冗余存 key)。
    let (mut entries, total_visits, session_start_ms, overflow_dropped) = match guard {
        Ok(l) => (
            l.entries.iter().map(|(k, e)| (k.clone(), e.clone())).collect::<Vec<_>>(),
            l.total_visits,
            l.session_start_ms,
            l.overflow_dropped,
        ),
        Err(_) => (Vec::new(), 0, 0, 0),
    };
    let distinct = entries.len();
    entries.sort_by(|(ka, a), (kb, b)| {
        b.last_at_ms
            .cmp(&a.last_at_ms)
            .then_with(|| ka.cmp(kb))
    });
    let top_repeats: Vec<Value> = {
        let mut by_visits = entries.clone();
        by_visits.sort_by(|(ka, a), (kb, b)| b.visits.cmp(&a.visits).then_with(|| ka.cmp(kb)));
        by_visits
            .iter()
            .take(5)
            .map(|(_, e)| json!({"url": e.last_url, "visits": e.visits}))
            .collect()
    };
    let pages: Vec<Value> = entries
        .iter()
        .take(60)
        .map(|(_, e)| {
            json!({
                "url": e.last_url,
                "title": e.title,
                "visits": e.visits,
                "via": format!("open={},navigate={},new_tab={}", e.from_open, e.from_navigate, e.from_new_tab),
                "first_at_ms": e.first_at_ms,
                "last_at_ms": e.last_at_ms,
                "last_page_id": e.last_page_id,
            })
        })
        .collect();
    json!({
        "summary": {
            "distinct_pages": distinct,
            "total_visits": total_visits,
            "session_start_ms": session_start_ms,
            "repeat_threshold": REPEAT_NOTE_THRESHOLD,
            "overflow_dropped": overflow_dropped,
            "shown": pages.len(),
            "hint": "工具层自动记账(open/navigate/new_tab 导航成功即记);遍历任务开工先对账,未访问页面优先",
        },
        "top_repeats": top_repeats,
        "pages": pages,
    })
}

/// 自某快照序号以来的机械足迹(单元 QC 用):distinct/总导航/最重复 Top5。
/// 纯本地统计、零 LLM 成本,QC 拿它对「覆盖类期望」做机械对账,漏页与打转一眼可见。
/// 基于**滚动事件环**求真增量 —— 聚合表的累计计数不含历史污染。
pub(crate) fn footprint_since(since_seq: u64) -> String {
    let guard = ledger_slot().lock();
    // 经引用读(match 值语义会把 guard move 掉,后续 display 闭包还要用它)。
    let events: Vec<(u64, String)> = match guard.as_ref() {
        Ok(l) => l
            .recent
            .iter()
            .rev()
            .map_while(|(seq, key)| (*seq >= since_seq).then(|| (*seq, key.clone())))
            .collect(),
        Err(_) => Vec::new(),
    };
    if events.is_empty() {
        return "无导航记录(本单元未发生 open/navigate/new_tab)".to_string();
    }
    // URL 展示取聚合表里的最近完整 URL;查不到(key 被超限淘汰)退回 key 本身。
    let display = |key: &str| -> String {
        guard
            .as_ref()
            .ok()
            .and_then(|l| l.entries.get(key).map(|e| e.last_url.clone()))
            .unwrap_or_else(|| key.to_string())
    };
    let mut counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for (_, key) in &events {
        *counts.entry(key.as_str()).or_insert(0) += 1;
    }
    let total: u32 = counts.values().sum();
    let mut ranked: Vec<(&str, u32)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let top: Vec<String> = ranked
        .iter()
        .take(5)
        .map(|(k, n)| format!("{}×{}", clip_url(&display(k)), n))
        .collect();
    format!(
        "distinct_pages={},total_navs={},top_repeats=[{}]",
        ranked.len(),
        total,
        top.join(", ")
    )
}

/// URL 展示裁剪(足迹一行内保持紧凑)。
fn clip_url(u: &str) -> String {
    const MAX: usize = 72;
    let chars: Vec<char> = u.chars().collect();
    if chars.len() <= MAX {
        u.to_string()
    } else {
        let head: String = chars[..48].iter().collect();
        let tail: String = chars[chars.len() - 16..].iter().collect();
        format!("{head}…{tail}")
    }
}

/// 清空台账(测试用;运行期无调用方)。
#[cfg(test)]
pub(super) fn reset_for_test() {
    if let Ok(mut l) = ledger_slot().lock() {
        *l = VisitLedger::default();
    }
}

/// 台账测试互斥锁:台账是进程级全局,并行单测会互相踩(实测偶发失败),
/// 所有触碰台账的测试先 `let _g = test_lock().lock().unwrap();` 再 reset+断言。
#[cfg(test)]
pub(super) fn test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visit_key_normalizes_fragment_query_slash() {
        assert_eq!(visit_key("http://a.com/x/#top"), "http://a.com/x");
        assert_eq!(visit_key("http://a.com/x?page=2"), "http://a.com/x");
        assert_eq!(visit_key(" http://a.com/ "), "http://a.com");
    }

    /// 第 163 轮:页面身份在 query 里的站点(搜索/分类驱动)不得塌缩成 1 个 key。
    /// 这是 P0 回归的锁死用例 —— 旧实现在 `?` 处截断,闲鱼 37 个品类搜索页被
    /// 压成 3 个,QC 据此做出错误指控。
    #[test]
    fn visit_key_keeps_identity_query_collapses_pagination() {
        // 不同搜索词 = 不同页面(必须区分)
        assert_ne!(
            visit_key("https://www.goofish.com/search?q=手机"),
            visit_key("https://www.goofish.com/search?q=家电")
        );
        // 不同类目 / 城市同样区分
        assert_ne!(
            visit_key("https://x.com/list?cat=book"),
            visit_key("https://x.com/list?cat=phone")
        );
        assert_ne!(
            visit_key("https://x.com/list?q=a&city=北京"),
            visit_key("https://x.com/list?q=a&city=上海")
        );
        // 翻页参数折叠(同一页的第 N 页不算新页面)
        assert_eq!(
            visit_key("https://x.com/list?q=a&page=3"),
            visit_key("https://x.com/list?q=a")
        );
        assert_eq!(
            visit_key("https://x.com/list?q=a&offset=40"),
            visit_key("https://x.com/list?q=a")
        );
        // 语义参数 + 翻页参数混合:语义部分必须完整保留
        assert_eq!(
            visit_key("https://x.com/list?q=a&cat=b&page=2"),
            "https://x.com/list?cat=b&q=a"
        );
        // key 与参数书写顺序无关
        assert_eq!(
            visit_key("https://x.com/list?cat=b&q=a"),
            visit_key("https://x.com/list?q=a&cat=b")
        );
        // 纯翻页 URL 归一到裸 path,不残留 '?'
        assert_eq!(visit_key("https://x.com/list?page=2&offset=20"), "https://x.com/list");
        // fragment 先于 query 切:fragment 里的 '?' 不得被当成 query
        assert_eq!(visit_key("https://x.com/list?cat=b#frag?x=1"), "https://x.com/list?cat=b");
    }

    #[test]
    fn record_and_coverage_and_note() {
        let _g = test_lock().lock().unwrap();
        reset_for_test();
        record_visit("p1", "http://a.com/login", "登录", VisitSource::Open);
        record_visit("p1", "http://a.com/home", "首页", VisitSource::Navigate);
        // 翻页参数折叠 → 与 /home 同一个 key
        record_visit("p1", "http://a.com/home?page=2", "首页", VisitSource::Navigate);
        record_visit("p2", "http://a.com/home", "首页", VisitSource::NewTab);
        let cov = coverage_payload();
        let s = &cov["summary"];
        assert_eq!(s["distinct_pages"], 2);
        assert_eq!(s["total_visits"], 4);
        // /home 三次(query 折叠 + new_tab 各记一次)→ 触发重复警示
        assert_eq!(visit_note("http://a.com/home").unwrap()["visits"], 3);
        assert!(visit_note("http://a.com/login").is_none());
        // 台账里 home 的 visits 聚合为 3
        let home = cov["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["url"].as_str().unwrap().contains("/home"))
            .unwrap()
            .clone();
        assert_eq!(home["visits"], 3);
        assert!(home["via"].as_str().unwrap().contains("navigate=2"));
    }

    /// 第 163 轮回归锁死:遍历多个品类搜索页时,distinct_pages 必须随品类增长,
    /// 且不对每个搜索页误报「重复打转」。
    #[test]
    fn search_遍历不塌缩且不误报打转() {
        let _g = test_lock().lock().unwrap();
        reset_for_test();
        for (i, cat) in ["手机", "家电", "图书", "潮玩", "母婴"].iter().enumerate() {
            record_visit(
                "p1",
                &format!("https://www.goofish.com/search?q={cat}"),
                "搜索结果",
                VisitSource::Navigate,
            );
            // 每类再翻两页,这些不该增加 distinct,但总导航数应累加
            for p in 1..=2 {
                record_visit(
                    "p1",
                    &format!("https://www.goofish.com/search?q={cat}&page={p}"),
                    "搜索结果",
                    VisitSource::Navigate,
                );
            }
            let _ = i;
        }
        let s = coverage_payload();
        let summary = &s["summary"];
        assert_eq!(
            summary["distinct_pages"], 5,
            "5 个品类搜索页必须计 5 个页面: {}",
            s
        );
        assert_eq!(summary["total_visits"], 15);
        // 关键回归点:每个搜索页只累计**自己的** 3 次访问(1 次基址 + 2 次翻页)。
        // 旧实现下 5 个品类共享一个 key,这里会报 15 —— 那正是实测日志里
        // `visit_note` 计数一路涨到 49、反复劝 Agent「确认不是重复打转」的根因。
        let note = visit_note("https://www.goofish.com/search?q=手机")
            .expect("该搜索页自身已达 3 次阈值,应给出重复提示");
        assert_eq!(
            note["visits"], 3,
            "重复次数必须只计本搜索页,不得把其它品类累加进来"
        );
        // 另一品类同样独立
        assert_eq!(
            visit_note("https://www.goofish.com/search?q=家电").unwrap()["visits"],
            3,
            "各品类搜索页计数互相独立"
        );
    }

    #[test]
    fn footprint_delta_and_seq() {
        let _g = test_lock().lock().unwrap();
        reset_for_test();
        let before = seq();
        record_visit("p1", "http://a.com/x", "", VisitSource::Navigate);
        record_visit("p1", "http://a.com/y", "", VisitSource::Navigate);
        let fp = footprint_since(before);
        assert!(fp.contains("distinct_pages=2"), "实际: {fp}");
        assert!(fp.contains("total_navs=2"), "实际: {fp}");
        // 无新增导航时如实说明
        let fp2 = footprint_since(seq());
        assert!(fp2.contains("无导航记录"), "实际: {fp2}");
    }

    #[test]
    fn overflow_guard_keeps_budget() {
        let _g = test_lock().lock().unwrap();
        reset_for_test();
        for i in 0..(MAX_ENTRIES + 8) {
            record_visit("p1", &format!("http://a.com/p/{i}"), "", VisitSource::Navigate);
        }
        let cov = coverage_payload();
        let s = &cov["summary"];
        assert_eq!(
            s["distinct_pages"].as_u64().unwrap() as usize,
            MAX_ENTRIES,
            "超限后条目数应被压回上限"
        );
        assert!(s["overflow_dropped"].as_u64().unwrap() >= 8);
    }
}
