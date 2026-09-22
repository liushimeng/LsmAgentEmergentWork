//! Markdown frontmatter 解析(极简 YAML 子集)—— 全仓共享。
//!
//! 2026-09-22 第 115 轮抽出:此前只有 `tui/commands.rs`(D2 自定义斜杠命令)有实现,
//! 本轮 `agent/custom_agents.rs`(自定义子 Agent 类型定义文件)需要**完全相同**的语义,
//! 若各自实现一份,两处对「引号剥离 / 未闭合回退 / 未知 key 忽略」的处理迟早漂移。
//!
//! 语义(与 D2 行为逐字对齐,原 `tui/commands.rs` 的单测即为回归网):
//! - 首行必须是裸 `---`;否则视为无 frontmatter,返回空 map + **原文**为正文;
//! - 到下一个 `---` 行为止;未闭合时按无 frontmatter 处理(全文当正文,不丢内容);
//! - 每行 `key: value`,`value` 两侧空白裁剪 + 剥离成对引号(`"..."` / `'...'`);
//! - 无冒号的行、空 key 忽略;重复 key **后者覆盖前者**;
//! - 返回的正文是 `---` 结束行之后的剩余内容(保留原缩进与空行)。

use std::collections::HashMap;

/// frontmatter 结束标记行(`trim_end` 后严格等于它才算结束)。
const FENCE: &str = "---";

/// 解析 frontmatter:返回 (键值对, 正文)。
///
/// 无 frontmatter / 未闭合时返回 (空 map, `raw` 原文)。
pub fn parse(raw: &str) -> (HashMap<String, String>, String) {
    let mut lines = raw.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != FENCE {
        return (HashMap::new(), raw.to_string());
    }
    let mut map: HashMap<String, String> = HashMap::new();
    // 已消费字节偏移(split_inclusive 保留行尾 \n,累计即 raw 内偏移)
    let mut consumed = first.len();
    for line in lines.by_ref() {
        consumed += line.len();
        let t = line.trim_end();
        if t == FENCE {
            return (map, raw[consumed.min(raw.len())..].to_string());
        }
        let Some((key, value)) = t.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        map.insert(key, unquote(value.trim()));
    }
    // frontmatter 未闭合:按无 frontmatter 处理(全文当正文,不丢内容)
    (HashMap::new(), raw.to_string())
}

/// 剥离成对的引号(`"..."` / `'...'`)。
pub fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (f, l) = (bytes[0], bytes[bytes.len() - 1]);
        if (f == b'"' && l == b'"') || (f == b'\'' && l == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// 取首个非空行作为描述兜底,按字符截断(UTF-8 / CJK 安全)。
pub fn fallback_description(body: &str, max_chars: usize) -> String {
    let first = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = first.chars().take(max_chars).collect();
    if first.chars().count() > max_chars {
        out.push('…');
    }
    out
}

/// 解析布尔值:缺省 / 无法识别 -> `default`。
pub fn parse_bool(raw: Option<&str>, default: bool) -> bool {
    match raw.map(|s| s.trim().to_lowercase()).as_deref() {
        None | Some("") => default,
        Some("true") | Some("yes") | Some("on") | Some("1") => true,
        Some("false") | Some("no") | Some("off") | Some("0") => false,
        Some(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_block_parsed_and_body_split() {
        let raw = "---\nlabel: 前端\nreadonly: true\n---\n正文第一行\n正文第二行\n";
        let (fm, body) = parse(raw);
        assert_eq!(fm.get("label").map(String::as_str), Some("前端"));
        assert_eq!(fm.get("readonly").map(String::as_str), Some("true"));
        assert_eq!(body, "正文第一行\n正文第二行\n");
    }

    #[test]
    fn quoted_values_unquoted() {
        let raw = "---\na: \"x y\"\nb: 'z'\n---\nbody";
        let (fm, _) = parse(raw);
        assert_eq!(fm["a"], "x y");
        assert_eq!(fm["b"], "z");
    }

    #[test]
    fn absent_frontmatter_returns_raw_as_body() {
        let raw = "没有 frontmatter 的正文";
        let (fm, body) = parse(raw);
        assert!(fm.is_empty());
        assert_eq!(body, raw);
    }

    #[test]
    fn unclosed_frontmatter_falls_back_to_raw() {
        let raw = "---\nlabel: x\n正文(未闭合)";
        let (fm, body) = parse(raw);
        assert!(fm.is_empty(), "未闭合不得半解析");
        assert_eq!(body, raw, "内容不得丢失");
    }

    #[test]
    fn unknown_keys_and_lines_without_colon_ignored() {
        let raw = "---\n没有冒号\n: 空key\nfuture_key: v\n---\nb";
        let (fm, _) = parse(raw);
        assert_eq!(fm.len(), 1);
        assert_eq!(fm["future_key"], "v");
    }

    #[test]
    fn duplicate_key_last_wins() {
        let (fm, _) = parse("---\nk: a\nk: b\n---\n");
        assert_eq!(fm["k"], "b");
    }

    #[test]
    fn bool_parsing_defaults_on_unknown() {
        assert!(parse_bool(Some("true"), false));
        assert!(parse_bool(Some("YES"), false));
        assert!(!parse_bool(Some("off"), true));
        assert!(parse_bool(None, true));
        assert!(parse_bool(Some("maybe"), true), "无法识别回退 default");
    }

    #[test]
    fn fallback_description_truncates_by_chars() {
        let body = "\n\n  第一个非空行但很长很长很长\n第二行";
        let d = fallback_description(body, 6);
        assert_eq!(d.chars().count(), 7, "6 字符 + 省略号");
        assert!(d.ends_with('…'));
    }
}
