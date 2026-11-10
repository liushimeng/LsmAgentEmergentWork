//! DevTools 一级菜单栏 + Console 面板的**平台无关共享层**(58 号设计文档)。
//!
//! 状态与纯逻辑的唯一真源:macOS(`view/macos/devtools.rs`)与 Windows
//! (`view/windows.rs`)查看器共用本层,各自只做 UI 装配/绘制/消息循环:
//! * **面板模型**:`PANEL_NAMES` 十个一级按钮(仅 `console` 有真实功能),
//!   `PANEL` 当前面板态,`toggle_panel_state` 纯状态翻转;
//! * **console 台账快照**:`push_payload`(读线程收 `'d'` 帧)→
//!   `DEVTOOL_SLOT`(latest-wins)→ `take_slot_and_merge`(定时器消费,
//!   `merge_snapshot` 合并进 `CONSOLE_BUF`);
//! * **过滤/搜索**:`FilterState` + `entry_visible`(级别开关 + 关键词);
//! * **显示语义**:`entry_class`(过滤分类)/ `entry_style`(前缀 + RGB 配色)。

use std::sync::Mutex;

use once_cell::sync::Lazy;

/// 菜单栏十个面板(显示名,面板 key)。仅 `console` 有真实功能,其余占位。
pub const PANEL_NAMES: [(&str, &str); 10] = [
    ("Elements", "elements"),
    ("Console", "console"),
    ("Sources", "sources"),
    ("Network", "network"),
    ("Performance", "performance"),
    ("Memory", "memory"),
    ("Application", "application"),
    ("Security", "security"),
    ("Lighthouse", "lighthouse"),
    ("Recorder", "recorder"),
];

/// 台账级别 → 过滤分类(`info`/`warn`/`error`)。
/// `assert` 在 PRELUDE 已折成 `error`;`eval_error` 是人工 eval 异常,归 error 显示。
pub fn entry_class(level: &str) -> &'static str {
    match level {
        "warn" => "warn",
        "error" | "assert" | "eval_error" => "error",
        _ => "info",
    }
}

/// 台账级别 → 显示前缀与 RGB 配色(Chrome 风格:错误红/警告橙/eval 蓝)。
pub fn entry_style(level: &str) -> (&'static str, f64, f64, f64) {
    match level {
        "warn" => ("⚠ ", 0.71, 0.38, 0.0),
        "error" | "assert" | "eval_error" => ("❌ ", 0.80, 0.0, 0.0),
        "input" => ("❯ ", 0.10, 0.45, 0.85),
        "result" => ("← ", 0.05, 0.40, 0.25),
        "group" => ("▸ ", 0.13, 0.13, 0.13),
        "time" => ("⏱ ", 0.30, 0.30, 0.30),
        "count" => ("# ", 0.30, 0.30, 0.30),
        "alert" | "confirm" | "prompt" => ("🔔 ", 0.55, 0.20, 0.65),
        _ => ("", 0.13, 0.13, 0.13),
    }
}

/// 过滤状态(查看器本地,不回流宿主)。
#[derive(Debug, Clone)]
pub struct FilterState {
    pub info: bool,
    pub warn: bool,
    pub error: bool,
    pub preserve: bool,
    pub search: String,
}

impl Default for FilterState {
    /// 默认三级全开(preserve 关、无关键词)——缺省看到全部日志。
    fn default() -> Self {
        Self {
            info: true,
            warn: true,
            error: true,
            preserve: false,
            search: String::new(),
        }
    }
}

/// 过滤判定:级别开关 + 关键词(大小写不敏感)。
pub fn entry_visible(f: &FilterState, level: &str, text: &str) -> bool {
    let pass = match entry_class(level) {
        "warn" => f.warn,
        "error" => f.error,
        _ => f.info,
    };
    if !pass {
        return false;
    }
    if f.search.is_empty() {
        return true;
    }
    text.to_lowercase().contains(&f.search.to_lowercase())
}

/// 快照合并游标:宿主侧已消费的条目数 + 所属页。
#[derive(Debug, Default)]
pub struct SnapState {
    pub page_id: String,
    pub len: usize,
}

/// 快照 → 本地缓冲的合并规则(幂等,免疫乱序/丢帧):
/// * **同页增长**(`entries.len() >= snap.len`):只追加尾巴(避免重绘闪烁);
/// * **同页缩短**(console.clear/台账重置)或**换页**:整体替换;
/// * **Preserve log 开启**:替换改为追加(跨导航保留历史)。
pub fn merge_snapshot(
    buf: &mut Vec<(String, String)>,
    snap: &mut SnapState,
    page_id: &str,
    entries: Vec<(String, String)>,
    preserve: bool,
) {
    if snap.page_id == page_id && entries.len() >= snap.len {
        for e in entries.iter().skip(snap.len) {
            buf.push(e.clone());
        }
        snap.len = entries.len();
        return;
    }
    if preserve {
        buf.extend(entries.iter().cloned());
    } else {
        *buf = entries.clone();
    }
    snap.page_id = page_id.to_string();
    snap.len = entries.len();
}

/// 字符串 → JSON 字符串字面量(含引号;FFI 层不额外引依赖)。
pub fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------- 运行态(双平台共用;DEVTOOL_SLOT 读线程写、定时器消费) ----------

/// 当前面板(`"none"` / `"console"` / 占位面板 key)。
pub static PANEL: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new("none".to_string()));
/// 读线程收到的最新 console 快照(整份替换,latest-wins)。
pub static DEVTOOL_SLOT: Lazy<Mutex<Option<(String, Vec<(String, String)>)>>> =
    Lazy::new(|| Mutex::new(None));
/// 本地日志缓冲 + 快照合并游标。
pub static CONSOLE_BUF: Lazy<Mutex<Vec<(String, String)>>> = Lazy::new(|| Mutex::new(Vec::new()));
pub static SNAP: Lazy<Mutex<SnapState>> = Lazy::new(|| Mutex::new(SnapState::default()));
/// 过滤态(缺省三级全开,见 `FilterState::default`)。
pub static FILTER: Lazy<Mutex<FilterState>> = Lazy::new(|| Mutex::new(FilterState::default()));

/// 面板是否展开(含占位面板)。
pub fn panel_open() -> bool {
    PANEL.lock().map(|p| p.as_str() != "none").unwrap_or(false)
}

/// 当前是否 Console 真面板。
pub fn console_open() -> bool {
    PANEL.lock().map(|p| p.as_str() == "console").unwrap_or(false)
}

/// 纯状态翻面板:同键再点 = 收起;返回翻转后的面板名(平台层据此做 UI 联动)。
pub fn toggle_panel_state(name: &str) -> String {
    let mut cur = PANEL.lock().unwrap();
    let next = if cur.as_str() == name {
        "none".to_string()
    } else {
        name.to_string()
    };
    *cur = next.clone();
    next
}

/// 读线程收到 `'d'` 帧:解析 console_sync 快照入槽(latest-wins)。
pub fn push_payload(payload: &[u8]) {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(payload) else {
        return;
    };
    if v.get("kind").and_then(|k| k.as_str()) != Some("console_sync") {
        return;
    }
    let page_id = v
        .get("page_id")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();
    let entries: Vec<(String, String)> = v
        .get("entries")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|it| {
                    Some((
                        it.get("level")?.as_str()?.to_string(),
                        it.get("text")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    if let Ok(mut slot) = DEVTOOL_SLOT.lock() {
        *slot = Some((page_id, entries));
    }
}

/// 定时器消费快照槽 → 合并进本地缓冲;返回是否**有变化**(需要重绘)。
pub fn take_slot_and_merge() -> bool {
    let taken = DEVTOOL_SLOT.lock().ok().and_then(|mut s| s.take());
    let Some((page_id, entries)) = taken else {
        return false;
    };
    let preserve = FILTER.lock().map(|f| f.preserve).unwrap_or(false);
    if let Ok(mut buf) = CONSOLE_BUF.lock() {
        if let Ok(mut snap) = SNAP.lock() {
            merge_snapshot(&mut buf, &mut snap, &page_id, entries, preserve);
        }
    }
    true
}

/// 清本地显示缓冲(快照游标不动——宿主台账只追加新尾巴,不复活旧日志)。
pub fn clear_buf() {
    if let Ok(mut buf) = CONSOLE_BUF.lock() {
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 台账级别_过滤分类与配色映射() {
        assert_eq!(entry_class("log"), "info");
        assert_eq!(entry_class("info"), "info");
        assert_eq!(entry_class("debug"), "info");
        assert_eq!(entry_class("group"), "info");
        assert_eq!(entry_class("warn"), "warn");
        assert_eq!(entry_class("error"), "error");
        assert_eq!(entry_class("assert"), "error");
        assert_eq!(entry_class("eval_error"), "error");
        // 配色:错误红、警告橙、eval 蓝,普通日志深灰。
        let (_, r, _, _) = entry_style("error");
        assert!(r > 0.7, "error 应为红色系:{r}");
        let (_, r, g, _) = entry_style("warn");
        assert!(r > 0.6 && g > 0.2, "warn 应为橙色系");
        let (prefix, _, _, _) = entry_style("input");
        assert_eq!(prefix, "❯ ");
    }

    #[test]
    fn 过滤判定_级别开关与关键词() {
        let f = FilterState::default();
        assert!(entry_visible(&f, "log", "任意"));
        let only_err = FilterState {
            info: false,
            warn: false,
            error: true,
            ..Default::default()
        };
        assert!(!entry_visible(&only_err, "log", "普通"));
        assert!(!entry_visible(&only_err, "warn", "警告"));
        assert!(entry_visible(&only_err, "error", "错误"));
        // 关键词大小写不敏感。
        let f2 = FilterState {
            search: "百度".into(),
            ..Default::default()
        };
        assert!(entry_visible(&f2, "log", "百度地图初始化"));
        assert!(!entry_visible(&f2, "log", "高德地图"));
    }

    #[test]
    fn 快照合并_同页追加尾巴_缩短替换_换页替换() {
        let mut buf: Vec<(String, String)> = Vec::new();
        let mut snap = SnapState::default();
        let e = |l: &str, t: &str| (l.to_string(), t.to_string());
        merge_snapshot(
            &mut buf,
            &mut snap,
            "p_1",
            vec![e("log", "a"), e("log", "b")],
            false,
        );
        assert_eq!(buf.len(), 2);
        merge_snapshot(
            &mut buf,
            &mut snap,
            "p_1",
            vec![e("log", "a"), e("log", "b"), e("warn", "c")],
            false,
        );
        assert_eq!(buf.len(), 3, "{buf:?}");
        assert_eq!(buf[2].1, "c");
        merge_snapshot(&mut buf, &mut snap, "p_1", vec![e("log", "d")], false);
        assert_eq!(buf.len(), 1, "{buf:?}");
        assert_eq!(buf[0].1, "d");
        merge_snapshot(&mut buf, &mut snap, "p_2", vec![e("error", "e")], false);
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].0, "error");
    }

    #[test]
    fn 快照合并_preserve_log_跨页保留() {
        let mut buf: Vec<(String, String)> = Vec::new();
        let mut snap = SnapState::default();
        let e = |l: &str, t: &str| (l.to_string(), t.to_string());
        merge_snapshot(&mut buf, &mut snap, "p_1", vec![e("log", "旧页")], true);
        merge_snapshot(&mut buf, &mut snap, "p_2", vec![e("log", "新页")], true);
        assert_eq!(buf.len(), 2, "preserve 下换页不清历史:{buf:?}");
        merge_snapshot(
            &mut buf,
            &mut snap,
            "p_2",
            vec![e("log", "新页"), e("log", "追加")],
            true,
        );
        assert_eq!(buf.len(), 3);
        assert_eq!(buf[2].1, "追加");
    }

    #[test]
    fn eval命令JSON转义_引号换行控制字符() {
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("行1\n行2"), "\"行1\\n行2\"");
        assert_eq!(json_string("\\"), "\"\\\\\"");
        assert_eq!(json_string("制表\t"), "\"制表\\t\"");
    }

    #[test]
    fn 面板翻转_同键收起_异键切换() {
        let mut guard = PANEL.lock().unwrap();
        *guard = "none".to_string();
        drop(guard);
        assert_eq!(toggle_panel_state("console"), "console");
        assert!(console_open());
        assert!(panel_open());
        assert_eq!(toggle_panel_state("console"), "none", "同键再点收起");
        assert!(!panel_open());
        assert_eq!(toggle_panel_state("elements"), "elements");
        assert!(panel_open());
        assert!(!console_open(), "占位面板不算 console");
        toggle_panel_state("elements");
    }
}
