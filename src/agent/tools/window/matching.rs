//! 窗口枚举与查找工具(2026-09-17 自 tools/window.rs 拆分,方案见 tmpPlan/2026-09-17_01)。
//!
//! `WindowListTool`(枚举可见顶层窗口)与 `WindowFindTool`(标题/进程名子串 +
//! Damerau-Levenshtein 模糊打分,返回最佳匹配窗口 id)。

use super::*;

// ===================== WindowList =====================

/// 枚举可见顶层窗口。
pub struct WindowListTool;

#[async_trait]
impl Tool for WindowListTool {
    fn name(&self) -> &str {
        "WindowList"
    }

    fn description(&self) -> &str {
        "枚举当前桌面全部可见顶层窗口,返回 JSON 对象(windows 数组含 id/title/进程名/PID/位置尺寸)。\n\
         - filter 可选:按窗口标题或进程名子串过滤(大小写不敏感)。\n\
         - 常见应用支持中英文别名(如 WeChat ↔ 微信)。\n\
         - 返回的 id 是不透明标识,仅供 WindowInspect/WindowAction 回传使用。\n\
         - macOS WindowList 走 CoreGraphics,不需要辅助功能权限;仅 WindowInspect/Action 需要授权。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filter": { "type": "string", "description": "可选,窗口标题/进程名子串过滤" }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let filter = get_str(&args, "filter").map(str::to_string);
        // 2026-09-15 第 53 轮 P2 修复:WindowList 不调用 driver_preflight。
        // 原设计是「缺权限/缺依赖时前置报错」,但实际上:
        //   - Windows:WindowList 无需任何授权(UIA 仅 inspect/act 需权限);
        //   - macOS:WindowList 走 CoreGraphics(CGWindowListCopyWindowInfo),
        //     不需要 AX 无障碍权限(即使未授权也可枚举);
        //   - Fallback (Linux):缺 wmctrl 时由 list_windows 自身返回结构化错误。
        // 旧 preflight 曾误把 WindowList 也 fail-closed(源于「macOS 26 移除 AX」的误判),
        // 但 WindowList 完全可用 —— 改由 list_windows 内部自行处理平台差异。
        // WindowInspect / WindowAction 仍保留 driver_preflight(真需要权限)。
        run_blocking(self.name(), move || {
            let driver = current_driver();
            let all = driver.list_windows(None)?;
            let aliases = filter.as_deref().map(expand_window_query).unwrap_or_default();
            let mut wins = if filter.is_some() {
                all.iter()
                    .filter(|w| window_matches_any(w, &aliases))
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                all.clone()
            };
            let total = wins.len();
            let truncated = total > MAX_WINDOWS;
            if total > MAX_WINDOWS {
                wins.truncate(MAX_WINDOWS);
            }
            let permission_hint = driver.permission_hint();
            let body = json!({
                "driver": driver.platform_name(),
                "filter": filter,
                "expanded_filters": aliases,
                "total_matched": total,
                "total_visible": all.len(),
                "truncated": truncated,
                "inspect_ready": permission_hint.is_none(),
                "permission_hint": permission_hint,
                "windows": wins,
                "hint": if wins.is_empty() {
                    "无匹配窗口;请确认目标应用已启动、已登录且窗口未最小化。可先用 WindowOpen(query) 启动/激活。"
                } else {
                    "window_id 仅在本轮窗口列表中稳定;UI 变化后请重新枚举。"
                }
            });
            Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
        })
        .await
    }
}

// ===================== WindowFind =====================
//
// 2026-09-16 第 56 轮:WindowUse 工具集新增 WindowFind。
// 背景:WindowList 返回全量窗口列表(JSON 数组),LLM 需要自己读 JSON 后再选
// 「微信是 id=xxx 那个」,这一步常消耗数百 token + 一次误选 → WindowFind 直接
// 接「按标题/进程名子串找一个窗口」,返回最佳匹配的 id(单一字符串,极小上下文)。
// - match_mode:
//   - "exact"    标题或进程名完全相等(忽略大小写);有多个时取首个
//   - "contains" 子串匹配(默认,大小写不敏感)
//   - "fuzzy"    子串 + 字符级相似度打分排序,返回 top-1
// - 返回:JSON 字符串 {"window_id":"...", "title":"...", "process_name":"...",
//   "score":0.95, "matched_field":"title"};若无匹配返回 error 引导改宽松策略。
//
// 不引入新依赖,字符串匹配走 std::str 与简单 Damerau-Levenshtein 上界剪枝
// (对窗口标题这种几十字符的串足够,fuzzy 仅用于 top-1 而非全排序)。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MatchMode {
    Exact,
    Contains,
    Fuzzy,
}

impl MatchMode {
    pub(super) fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "exact" => Self::Exact,
            "fuzzy" => Self::Fuzzy,
            _ => Self::Contains,
        }
    }
}

pub(super) struct ScoredHit<'a> {
    pub(super) info: &'a WindowInfo,
    pub(super) score: f64,
    pub(super) matched_field: &'static str, // "title" / "process"
}

fn score_exact<'a>(query_lower: &str, info: &'a WindowInfo) -> Option<ScoredHit<'a>> {
    // 2026-09-16 第 66 轮 P1-5:两侧先做 Unicode 上标归一化(ᴬᴵᴬ ↔ AIA 等价),
    // 再小写比较;否则 filter 写 ASCII 形匹配不到真实窗口标题里的 Modifier Letter。
    let title_l = normalize_unicode_for_match(&info.title).to_lowercase();
    let proc_l = normalize_unicode_for_match(&info.process_name).to_lowercase();
    if title_l == query_lower {
        Some(ScoredHit {
            info,
            score: 1.0,
            matched_field: "title",
        })
    } else if proc_l == query_lower {
        Some(ScoredHit {
            info,
            score: 1.0,
            matched_field: "process",
        })
    } else {
        None
    }
}

fn score_contains<'a>(query_lower: &str, info: &'a WindowInfo) -> Option<ScoredHit<'a>> {
    let title_l = normalize_unicode_for_match(&info.title).to_lowercase();
    let proc_l = normalize_unicode_for_match(&info.process_name).to_lowercase();
    if title_l.contains(query_lower) {
        let q_chars = query_lower.chars().count() as f64;
        let denom = title_l.chars().count().max(1) as f64;
        let s = (q_chars / denom).clamp(0.3, 0.95);
        Some(ScoredHit {
            info,
            score: s,
            matched_field: "title",
        })
    } else if proc_l.contains(query_lower) {
        let q_chars = query_lower.chars().count() as f64;
        let denom = proc_l.chars().count().max(1) as f64;
        let s = (q_chars / denom).clamp(0.3, 0.95);
        Some(ScoredHit {
            info,
            score: s,
            matched_field: "process",
        })
    } else {
        None
    }
}

/// 简化 Levenshtein 距离(无 transposition,纯 DP),
/// 仅用于 ≤ 64 字符的窗口标题;上界剪枝:长度差 > 8 直接返回 max_len。
pub(super) fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let max_len = m.max(n);
    let min_len = m.min(n);
    if max_len - min_len > 8 {
        return max_len;
    }
    let mut prev2: Vec<usize> = (0..=n).collect();
    let mut prev1: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        prev1[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            let del = prev1[j - 1] + 1; // insertion
            let ins = prev2[j] + 1; // deletion
            let sub = prev2[j - 1] + cost;
            prev1[j] = del.min(ins).min(sub);
        }
        std::mem::swap(&mut prev1, &mut prev2);
    }
    prev2[n]
}

fn score_fuzzy<'a>(query: &str, info: &'a WindowInfo) -> Option<ScoredHit<'a>> {
    let q = normalize_unicode_for_match(query).to_lowercase();
    let title_l = normalize_unicode_for_match(&info.title).to_lowercase();
    let proc_l = normalize_unicode_for_match(&info.process_name).to_lowercase();
    let dt = damerau_levenshtein(&q, &title_l);
    let dp = damerau_levenshtein(&q, &proc_l);
    let max_len = q.chars().count().max(info.title.chars().count()).max(1);
    let score_t = (1.0 - (dt as f64) / (max_len as f64)).max(0.0);
    let score_p = (1.0 - (dp as f64) / (max_len as f64)).max(0.0);
    if score_t < 0.2 && score_p < 0.2 {
        return None;
    }
    if score_t >= score_p {
        Some(ScoredHit {
            info,
            score: score_t,
            matched_field: "title",
        })
    } else {
        Some(ScoredHit {
            info,
            score: score_p,
            matched_field: "process",
        })
    }
}

/// 按窗口信息列表 + 模式 + 查询词匹配,返回 top-1。
pub(super) fn pick_top_hit<'a>(wins: &'a [WindowInfo], query: &str, mode: MatchMode) -> Option<ScoredHit<'a>> {
    let query_lower = normalize_unicode_for_match(query).to_lowercase();
    wins.iter()
        .filter_map(|w| match mode {
            MatchMode::Exact => score_exact(&query_lower, w),
            MatchMode::Contains => score_contains(&query_lower, w),
            MatchMode::Fuzzy => score_fuzzy(query, w),
        })
        .max_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// 按标题/进程名子串找一个窗口,返回最佳匹配的 id。
pub struct WindowFindTool;

#[async_trait]
impl Tool for WindowFindTool {
    fn name(&self) -> &str {
        "WindowFind"
    }

    fn description(&self) -> &str {
        "按标题/进程名子串查找一个窗口,直接返回最佳匹配的 window_id(单一字符串)。\n\
         - query 必填:窗口标题或进程名的查询词(大小写不敏感)\n\
         - match_mode 可选:exact / contains / fuzzy(默认 contains);\n\
           exact = 完全相等;contains = 子串命中;fuzzy = 字符级相似度排序。\n\
         - 返回 JSON:{\"window_id\":\"...\",\"title\":\"...\",\"process_name\":\"...\",\
           \"score\":0.95,\"matched_field\":\"title\"}\n\
         用于:WindowList 返回后,不想逐条读 JSON 自己选 — 直接传「微信」/「WeChat」即可。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "查询词,匹配标题或进程名(大小写不敏感)" },
                "match_mode": {
                    "type": "string",
                    "enum": ["exact", "contains", "fuzzy"],
                    "description": "匹配模式,默认 contains"
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = require_str(&args, "query", self.name())?.to_string();
        let mode_str_owned = get_str(&args, "match_mode")
            .unwrap_or("contains")
            .to_string();
        let mode = MatchMode::parse(&mode_str_owned);
        // WindowFind 仅基于 WindowList 结果(走 CoreGraphics,无授权要求),
        // 不调用 driver_preflight,避免误把 macOS AX 未授权也 fail-closed。
        run_blocking(self.name(), move || {
            let driver = current_driver();
            let wins = driver.list_windows(None)?;
            if wins.is_empty() {
                return Err(tool_err(
                    "WindowFind",
                    "无可见窗口,请确认桌面有目标应用在前台",
                ));
            }
            let aliases = expand_window_query(&query);
            let mut hit = None;
            let mut matched_query = query.clone();
            for alias in &aliases {
                if let Some(h) = pick_top_hit(&wins, alias, mode) {
                    hit = Some(h);
                    matched_query = alias.clone();
                    break;
                }
            }
            match hit {
                Some(hit) => {
                    // 2026-09-16 第 58 轮 P1-B:title 为空但 process_name 命中时,
                    // 输出 note 字段说明 WeChat/部分 Electron 应用 NSWindow title
                    // 私有化的已知行为,引导 LLM 继续 WindowInspect 而不是放弃。
                    let note = if hit.info.title.is_empty()
                        && hit.matched_field == "process"
                    {
                        Some(
                            "macOS 上 WeChat / 部分 Electron 应用 NSWindow title \
                             可能为空(CoreGraphics 拿不到),这是正常结果。\
                             窗口已通过 process_name 成功定位,可以直接调用 \
                             WindowInspect(window_id) 继续检视控件树。"
                        )
                    } else if hit.info.title.is_empty() {
                        Some(
                            "title 为空,可能 NSWindow 私有化;通过 process_name 匹配,\
                             可继续 WindowInspect"
                        )
                    } else {
                        None
                    };
                    let mut body = json!({
                        "window_id": hit.info.id,
                        "title": hit.info.title,
                        "process_name": hit.info.process_name,
                        "pid": hit.info.pid,
                        "score": hit.score,
                        "matched_field": hit.matched_field,
                        "match_mode": match mode {
                            MatchMode::Exact => "exact",
                            MatchMode::Contains => "contains",
                            MatchMode::Fuzzy => "fuzzy",
                        },
                        "matched_query": matched_query,
                        "query_aliases": aliases,
                    });
                    if let Some(n) = note {
                        body["note"] = json!(n);
                    }
                    Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
                }
                None => {
                    let titles: Vec<String> = wins
                        .iter()
                        .take(10)
                        .map(|w| format!("{} ({})", w.title, w.process_name))
                        .collect();
                    Err(tool_err(
                        "WindowFind",
                        format!(
                            "无匹配 query={query:?} mode={mode_str_owned:?} 的窗口;当前可见前 10 个:{titles:?}。\
                             建议:改用 match_mode=contains 或更短 query,或先用 WindowList 自行确认可见窗口"
                        ),
                    ))
                }
            }
        })
        .await
    }
}
