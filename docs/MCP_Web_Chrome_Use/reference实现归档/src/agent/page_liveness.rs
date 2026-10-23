//! 页面存活性台账(第 172 轮):识别「站点自发反复重载 / 自杀式刷新」页面。
//!
//! 背景(2026-10-14 BOSS 直聘实测,见
//! `tmpPlan/20261014_164500_BOSS直聘页面自刷新与反爬识别治理方案.md`):
//! 站点对「一次性全新 profile + 自动化浏览器」执行自杀式反爬 —— 文档被原地
//! reload 或整页抹除。实测证据:页面静置 30s(期间零工具动作)后
//! `performance.timeOrigin` 变化、`window.__laewProbe` 全局标记丢失、URL 与标题
//! 不变。对模型的危害有两层:
//! ① 人工在 HITL 弹窗里输入的字符随文档销毁,体感「输入不了」;
//! ② Agent 把这种页面当正常页面反复截图/OCR/点击,空转数分钟。
//!
//! 与 [`crate::agent::browser_pace`] 的 `site_wipe` 互补:wipe 记的是「落地
//! about:blank」,本模块记的是「URL 不变但文档反复重建」——两者互不覆盖。
//!
//! 设计约束:
//! - 采样必须**便宜**:单条 `eval_js` 读 `performance.timeOrigin` + `location.href`,
//!   每页 5s 节流(节流窗口在采样点判定,不在调用点)。
//! - 记账必须**区分发起方**:Agent 主动导航(`open`/`navigate`/`back`/`reload`/
//!   `new_tab`)不算自刷新,否则每次导航都误判。
//! - 判定必须是**纯函数**,便于单测锁死阈值(滚动窗口 + 次数)。

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

/// 自刷新判定窗口(秒)。
const WINDOW_SECS: u64 = 90;
/// 窗口内自发重载次数阈值(达到即判不稳定)。
const RELOAD_THRESHOLD: usize = 3;
/// 30s 内 ≥2 次即判强不稳定(短周期形态,人工几乎无法输入)。
const BURST_WINDOW_SECS: u64 = 30;
const BURST_THRESHOLD: usize = 2;

/// 单页存活性记账。
#[derive(Debug, Default, Clone)]
struct Entry {
    /// 最近一次采样到的 `performance.timeOrigin`(文档身份)。
    time_origin: Option<f64>,
    /// 自发重载发生时刻(epoch 秒),滚动窗口内计数。
    self_reloads: Vec<u64>,
    /// 最近一次落地 URL(报告里回传给模型,便于人工核对「刷的是哪一页」)。
    last_url: String,
    /// 最近采样时刻,用于 5s 节流。
    last_sampled: u64,
    /// Agent 主动导航次数(可观测用,不参与判定)。
    agent_navs: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn registry() -> &'static Mutex<HashMap<String, Entry>> {
    static REG: std::sync::OnceLock<Mutex<HashMap<String, Entry>>> = std::sync::OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 采样 JS:一次读回文档身份 + 落地 URL。返回 `JSON.stringify` 便于
/// `serde_json` 直解(与仓库既有 eval 探针同款风格)。
pub const PROBE_JS: &str = r#"JSON.stringify({o:performance.timeOrigin,u:location.href})"#;

/// 解析 [`PROBE_JS`] 的返回文本 → `(time_origin, url)`。
pub fn parse_probe(raw: &str) -> Option<(f64, String)> {
    let v: Value = serde_json::from_str(raw.trim()).ok()?;
    let o = v.get("o")?.as_f64()?;
    let u = v.get("u").and_then(Value::as_str).unwrap_or_default().to_string();
    Some((o, u))
}

/// 纯判定:给定自发重载时刻序列,返回 `(unstable, reason, 窗口内次数)`。
fn judge(stamps: &[u64], now: u64) -> (bool, &'static str, usize) {
    let in_window: Vec<u64> = stamps
        .iter()
        .copied()
        .filter(|t| now.saturating_sub(*t) <= WINDOW_SECS)
        .collect();
    let burst: Vec<u64> = in_window
        .iter()
        .copied()
        .filter(|t| now.saturating_sub(*t) <= BURST_WINDOW_SECS)
        .collect();
    if burst.len() >= BURST_THRESHOLD {
        return (true, "self_reload_burst", burst.len());
    }
    if in_window.len() >= RELOAD_THRESHOLD {
        return (true, "self_reload_loop", in_window.len());
    }
    (false, "", in_window.len())
}

/// 记账一次采样结果。`agent_initiated=true` 表示这次文档替换是 Agent 自己
/// 触发的导航(不计入自刷新),但会刷新 `time_origin` 基线,避免误判下一次。
pub fn observe(page_id: &str, time_origin: f64, url: &str, agent_initiated: bool) {
    let mut map = registry().lock().unwrap();
    let e = map.entry(page_id.to_string()).or_default();
    let now = now_secs();
    e.last_sampled = now;
    if !url.is_empty() {
        e.last_url = url.to_string();
    }
    match e.time_origin {
        None => e.time_origin = Some(time_origin),
        Some(prev) if (prev - time_origin).abs() < f64::EPSILON => {}
        Some(_) => {
            // 文档身份变化 = 发生了一次重建。
            e.time_origin = Some(time_origin);
            if agent_initiated {
                e.agent_navs += 1;
            } else {
                e.self_reloads.push(now);
            }
        }
    }
}

/// 标记一次 Agent 主动导航(不采样,仅记账;采样由调用方随后 `observe` 完成)。
pub fn note_agent_navigation(page_id: &str) {
    let mut map = registry().lock().unwrap();
    let e = map.entry(page_id.to_string()).or_default();
    e.agent_navs += 1;
    e.last_sampled = now_secs();
}

/// 该页是否处于「5s 节流窗口内」(调用方据此跳过采样)。首次采样恒执行。
pub fn should_sample(page_id: &str) -> bool {
    let map = registry().lock().unwrap();
    match map.get(page_id) {
        None => true,
        Some(e) => now_secs().saturating_sub(e.last_sampled) >= 5,
    }
}

/// 取该页的存活性报告(仅在不稳定时返回 `Some`,健康页零噪声)。
pub fn report(page_id: &str) -> Option<Value> {
    let map = registry().lock().unwrap();
    let e = map.get(page_id)?;
    let now = now_secs();
    let (unstable, reason, count) = judge(&e.self_reloads, now);
    if !unstable {
        return None;
    }
    let oldest = e.self_reloads.iter().copied().min().unwrap_or(now);
    Some(json!({
        "unstable": true,
        "reason": reason,
        "self_reloads": count,
        "window_secs": WINDOW_SECS,
        "span_secs": now.saturating_sub(oldest),
        "url": e.last_url,
        "advice": "该页面正被站点反复重载/自愈(每次重载都会销毁页面内已输入的内容),\
                   在此页上人工操作与 Agent 交互都会反复归零;\
                   处置:① 先在稳定页面完成登录(短信/扫码必须人工);\
                   ② 优先 open(reuse_existing=true) 接管已登录的真实浏览器(connect 模式零注入、真实指纹);\
                   ③ 不要反复 reload —— 重载只会再吃一次风控并延长封锁",
    }))
}

/// 附加报告到已有响应对象(健康/未采样时不写入任何字段)。
pub fn attach(data: &mut Value, page_id: &str) {
    if let Some(v) = report(page_id) {
        data["page_liveness"] = v;
    }
}

/// 页关闭/回收时清账。
pub fn forget(page_id: &str) {
    registry().lock().unwrap().remove(page_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 探针解析() {
        let (o, u) = parse_probe(r#"{"o":1791965342949.1,"u":"https://x.test/a"}"#).unwrap();
        assert!((o - 1791965342949.1).abs() < 1.0);
        assert_eq!(u, "https://x.test/a");
        assert!(parse_probe("not json").is_none());
        assert!(parse_probe(r#"{"u":"x"}"#).is_none());
    }

    #[test]
    fn 阈值判据_窗口内不足不报() {
        let now = 1000u64;
        assert_eq!(judge(&[], now).0, false, "零事件不报");
        // 单次重载 = 一次正常 SPA 重定向/站点小抖动,不报(否则正常页也会被
        // 打成不稳定,人工介入被无谓告警)。
        assert_eq!(judge(&[now - 1], now).0, false);
        assert_eq!(judge(&[now - 200], now).0, false, "窗口外事件不计入");
    }

    #[test]
    fn 阈值判据_短周期与长周期() {
        let now = 1000u64;
        // 30s 内 2 次 → burst
        let (u, reason, _) = judge(&[now - 5, now - 10], now);
        assert!(u && reason == "self_reload_burst");
        // 90s 内 3 次(每次间隔 >30s) → loop
        let (u, reason, n) = judge(&[now - 5, now - 40, now - 70], now);
        assert!(u && reason == "self_reload_loop" && n == 3);
    }

    #[test]
    fn 观察记账_自发与主动分开() {
        let id = "liveness_test_page";
        observe(id, 1.0, "https://a.test/", false);
        observe(id, 2.0, "https://a.test/", false);
        // 主动导航不算自刷新
        observe(id, 3.0, "https://a.test/b", true);
        // 再一次自发 → 2 次自发(命中 burst),1 次主动
        observe(id, 4.0, "https://a.test/", false);
        let rep = report(id).expect("2 次紧邻自发应命中 burst");
        let count = rep["self_reloads"].as_u64().unwrap();
        assert_eq!(count, 2, "主动导航不应计入自刷新");
        forget(id);
        assert!(report(id).is_none(), "forget 后不应再出报告");
    }

    #[test]
    fn 采样节流_首次必执行() {
        let id = "liveness_throttle_page";
        assert!(should_sample(id));
        observe(id, 1.0, "u", false);
        assert!(!should_sample(id), "刚采样过应跳过");
        forget(id);
    }
}