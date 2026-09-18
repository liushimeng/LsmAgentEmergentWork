//! MCP_Window_Use 查询类 action(2026-09-18 第 84 轮自 tools/window/matching.rs 迁入):
//! `list`(枚举可见顶层窗口)与 `find`(标题/进程名子串 + Damerau-Levenshtein
//! 模糊打分,返回最佳匹配窗口 id)。

use super::*;

// ===================== action=list =====================

/// 枚举可见顶层窗口。
///
/// 不调用 driver_preflight:Windows 无需任何授权;macOS 走 CoreGraphics
/// (CGWindowListCopyWindowInfo)不需要 AX 无障碍权限;缺依赖由 list_windows
/// 自身返回结构化错误。inspect / control 才保留 driver_preflight(真需要权限)。
pub(super) async fn run_list(args: Value) -> Result<String> {
    let filter = get_str(&args, "filter").map(str::to_string);
    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
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
                "无匹配窗口;请确认目标应用已启动、已登录且窗口未最小化。可先用 MCP_Window_Use(action=open, query=...) 启动/激活。"
            } else {
                "window_id 仅在本轮窗口列表中稳定;UI 变化后请重新枚举。"
            }
        });
        Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
    })
    .await
}

// ===================== action=find =====================
//
// 直接接「按标题/进程名子串找一个窗口」,返回最佳匹配的 id(单一字符串,极小上下文),
// 省一次「读 WindowList 全量 JSON 再自己选」。
// - match_mode:
//   - "exact"    标题或进程名完全相等(忽略大小写);有多个时取首个
//   - "contains" 子串匹配(默认,大小写不敏感)
//   - "fuzzy"    子串 + 字符级相似度打分排序,返回 top-1
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
    // 两侧先做 Unicode 上标归一化(ᴬᴵᴬ ↔ AIA 等价),再小写比较;
    // 否则 filter 写 ASCII 形匹配不到真实窗口标题里的 Modifier Letter。
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
///
/// 仅基于 list_windows 结果(macOS 走 CoreGraphics,无授权要求),
/// 不调用 driver_preflight,避免误把 macOS AX 未授权也 fail-closed。
pub(super) async fn run_find(args: Value) -> Result<String> {
    let query = require_str(&args, "query", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let mode_str_owned = get_str(&args, "match_mode")
        .unwrap_or("contains")
        .to_string();
    let mode = MatchMode::parse(&mode_str_owned);
    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
        let driver = current_driver();
        let wins = driver.list_windows(None)?;
        if wins.is_empty() {
            return Err(tool_err(
                MCP_WINDOW_USE_TOOL_NAME,
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
                // title 为空但 process_name 命中时,输出 note 字段说明 WeChat/部分
                // Electron 应用 NSWindow title 私有化的已知行为,引导 LLM 继续 inspect。
                let note = if hit.info.title.is_empty()
                    && hit.matched_field == "process"
                {
                    Some(
                        "macOS 上 WeChat / 部分 Electron 应用 NSWindow title \
                         可能为空(CoreGraphics 拿不到),这是正常结果。\
                         窗口已通过 process_name 成功定位,可以直接调用 \
                         MCP_Window_Use(action=inspect, window_id=...) 继续检视控件树。"
                    )
                } else if hit.info.title.is_empty() {
                    Some(
                        "title 为空,可能 NSWindow 私有化;通过 process_name 匹配,\
                         可继续 MCP_Window_Use(action=inspect)"
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
                    MCP_WINDOW_USE_TOOL_NAME,
                    format!(
                        "无匹配 query={query:?} mode={mode_str_owned:?} 的窗口;当前可见前 10 个:{titles:?}。\
                         建议:改用 match_mode=contains 或更短 query,或先用 action=list 自行确认可见窗口"
                    ),
                ))
            }
        }
    })
    .await
}
