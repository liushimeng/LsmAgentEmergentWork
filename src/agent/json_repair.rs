//! LLM 输出 JSON 自动修复链(Tier-1 语法修复)。
//!
//! LLM 偶发输出「形似 JSON 但语法非法」的文本:智能引号 / 全角标点 / 单引号 /
//! 字符串内裸换行 / 尾逗号 / Python 常量。此前这些情况直接解析失败
//! (Yolo 静默降级 simple、Quality fail-closed 触发无谓回流)。
//! 本模块在失败后自动修复重解,**修复不了仍按原错误上抛(fail-closed 不变)**。
//!
//! 设计对齐知识库(专题-第七轮-结构化输出与Schema校验 §2.3 atomcode repair.rs):
//! - 单遍 O(N) 扫描,所有结构性修改带 in_string / escape 状态感知
//!   (字符串内容不被误改);
//! - `MAX_REPAIR_BYTES` 守卫防性能退化;
//! - 全部规则跑完仍非法 → 原样透传(修复失败兜底行为)。
//!
//! **刻意不做截断补全**(补括号/补引号):被截断的 Quality 报告可能 verdict 已出
//! 而 issues 未列全,自动补全语义上有害 —— 上轮 P0 fail-closed 测试
//! (`parse_quality_report_truncated_json_fails_closed`)钉死此语义。

use serde::de::DeserializeOwned;

/// 超过该大小的输入不做修复(防性能退化),对齐 atomcode MAX_REPAIR_BYTES。
const MAX_REPAIR_BYTES: usize = 512 * 1024;

/// fast-path 直解 → 自动修复 → 再解;均失败返回合并诊断(原始错误 + 修复后错误)。
///
/// **不启用截断补全**(fail-closed):Quality-Check 报告被截断可能 verdict 已出
/// 而 issues 列表未列全,自动补全在语义上有害。
/// 上轮 P0 fail-closed 测试(`parse_quality_report_truncated_json_fails_closed`)钉死此语义。
pub fn try_parse<T: DeserializeOwned>(json: &str) -> Result<T, String> {
    match serde_json::from_str::<T>(json) {
        Ok(v) => Ok(v),
        Err(primary) => {
            let repaired = repair_json(json);
            match serde_json::from_str::<T>(&repaired) {
                Ok(v) => {
                    tracing::info!(primary = %primary, "JSON 自动修复成功");
                    Ok(v)
                }
                Err(secondary) => Err(format!(
                    "JSON 解析失败(原始: {primary}; 自动修复后仍失败: {secondary})"
                )),
            }
        }
    }
}

/// 启用 **Tier-2 截断补全** 的解析入口 —— Yolo / Main-Work JSON 输出
/// (token 触顶被服务端截断的场景)可被自动补全为合法 JSON;
/// Quality-Check 报告**必须**使用 `try_parse`(fail-closed 不变,见
/// `parse_quality_report_truncated_json_fails_closed` 测试)。
///
/// 关联报告: 2026-09-09_04-多轮对话测试问题分析 D-001。
pub fn try_parse_lenient<T: DeserializeOwned>(json: &str) -> Result<T, String> {
    match serde_json::from_str::<T>(json) {
        Ok(v) => Ok(v),
        Err(primary) => {
            // 1) 先走 Tier-1 修复链(智能引号 / 单引号 / 尾逗号等)
            let repaired = repair_json(json);
            match serde_json::from_str::<T>(&repaired) {
                Ok(v) => {
                    tracing::info!(primary = %primary, "JSON Tier-1 修复成功");
                    Ok(v)
                }
                Err(secondary) => {
                    // 2) Tier-1 仍失败,若原错误是截断语义,再走 Tier-2 补全
                    if !is_truncation_error(&primary) && !is_truncation_error(&secondary) {
                        return Err(format!(
                            "JSON 解析失败(原始: {primary}; 自动修复后仍失败: {secondary})"
                        ));
                    }
                    let completed = complete_truncated_json(&repaired);
                    match serde_json::from_str::<T>(&completed) {
                        Ok(v) => {
                            tracing::warn!(
                                primary = %primary,
                                "JSON 截断补全成功(Tier-2): LLM 输出被截断,自动补全"
                            );
                            Ok(v)
                        }
                        Err(tertiary) => Err(format!(
                            "JSON 解析失败(原始: {primary}; 截断补全后仍失败: {tertiary})"
                        )),
                    }
                }
            }
        }
    }
}

/// 判断 serde_json::Error 是否呈现「截断」特征 —— `EOF while parsing ...`
/// 或 `unexpected end of input`(serde_json 标准截断错误文案)。
fn is_truncation_error(err: &serde_json::Error) -> bool {
    let s = err.to_string();
    s.contains("EOF while parsing")
        || s.contains("unexpected end of input")
        || (s.contains("EOF") && s.contains("parsing"))
}

/// 依次叠加全部 Tier-1 修复规则。
///
/// 规则顺序:先统一引号类(智能引号 → 全角标点 → 单引号),再字符串内控制字符,
/// 最后结构类(尾逗号 → Python 常量)。后面的规则假定前面的规则已把定界符
/// 统一为 ASCII 引号。
pub fn repair_json(input: &str) -> String {
    if input.len() > MAX_REPAIR_BYTES {
        return input.to_string();
    }
    let s = pass_smart_quotes(input);
    let s = pass_fullwidth_punct(&s);
    let s = pass_single_quotes(&s);
    let s = pass_control_chars(&s);
    let s = pass_trailing_commas(&s);
    pass_python_consts(&s)
}

/// 扫描状态:不在字符串 / 在双引号串 / 在单引号串(待修复的串)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrState {
    Outside,
    InDouble,
    InSingle,
}

/// 单字符处理动作(闭包返回 `None` 表示「交给通用状态机默认处理」)。
enum CharAction {
    /// 闭包已自行输出转换结果,但不改变串状态
    Emit,
    /// 闭包已输出开串定界符,进入双引号串
    EnterDouble,
    /// 闭包已输出开串定界符,进入单引号串
    EnterSingle,
    /// 闭包已输出替换后的闭合定界符,退出字符串
    ExitString,
}

/// 通用字符级扫描器:维护 in_string / escape 状态,把「串外」「串内」字符分别
/// 交给 `outside` / `inside_str` 变换;闭包返回 `None` 时走默认处理
/// (原样输出 + 状态机记账:进串 / 退串 / escape 记账)。
///
/// 转义中的字符(`\x` 的 `x`)会带 `escaped=true` 交给闭包,默认原样透传
/// (已转义的 `\"` 不会误判为闭合、`\n` 不会被二次转义)。
fn scan_transform(
    input: &str,
    outside: &mut dyn FnMut(char, &mut String) -> Option<CharAction>,
    inside_str: &mut dyn FnMut(char, StrState, bool, &mut String) -> Option<CharAction>,
) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    let mut state = StrState::Outside;
    let mut escape = false;
    for c in input.chars() {
        let action = match state {
            StrState::Outside => {
                // 串外无 escape 概念
                outside(c, &mut out).unwrap_or_else(|| {
                    out.push(c);
                    if c == '"' {
                        CharAction::EnterDouble
                    } else if c == '\'' {
                        CharAction::EnterSingle
                    } else {
                        CharAction::Emit
                    }
                })
            }
            s => {
                let escaped = escape;
                inside_str(c, s, escaped, &mut out).unwrap_or_else(|| {
                    out.push(c);
                    if escaped {
                        CharAction::Emit
                    } else if c == '\\' {
                        CharAction::Emit
                    } else if (s == StrState::InDouble && c == '"')
                        || (s == StrState::InSingle && c == '\'')
                    {
                        CharAction::ExitString
                    } else {
                        CharAction::Emit
                    }
                })
            }
        };
        // 统一执行状态迁移
        match action {
            CharAction::Emit => {
                if state != StrState::Outside {
                    if escape {
                        escape = false;
                    } else if c == '\\' {
                        escape = true;
                    }
                }
            }
            CharAction::EnterDouble => {
                state = StrState::InDouble;
                escape = false;
            }
            CharAction::EnterSingle => {
                state = StrState::InSingle;
                escape = false;
            }
            CharAction::ExitString => {
                state = StrState::Outside;
                escape = false;
            }
        }
    }
    out
}

/// 规则 1:智能引号定界符 → ASCII 引号。
///
/// - 串外的 `“`/`”` 视为双引号定界符、`‘`/`’` 视为单引号定界符;
/// - 由 ASCII 引号打开的字符串内,智能引号是**数据**(中文引号常见于内容),
///   不修改;只有由智能引号打开的串才以智能引号闭合(带 opened_by_smart 记忆)。
fn pass_smart_quotes(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    let mut state = StrState::Outside;
    let mut escape = false;
    // 当前串是否由智能引号打开(决定以 ASCII 还是智能引号闭合)
    let mut opened_by_smart = false;
    for c in input.chars() {
        if state != StrState::Outside && escape {
            out.push(c);
            escape = false;
            continue;
        }
        match state {
            StrState::Outside => match c {
                '“' | '”' => {
                    out.push('"');
                    state = StrState::InDouble;
                    opened_by_smart = true;
                }
                '‘' | '’' => {
                    out.push('\'');
                    state = StrState::InSingle;
                    opened_by_smart = true;
                }
                '"' => {
                    out.push(c);
                    state = StrState::InDouble;
                    opened_by_smart = false;
                }
                '\'' => {
                    out.push(c);
                    state = StrState::InSingle;
                    opened_by_smart = false;
                }
                _ => out.push(c),
            },
            StrState::InDouble => {
                if opened_by_smart && c == '”' {
                    out.push('"');
                    state = StrState::Outside;
                } else {
                    out.push(c);
                    if c == '\\' {
                        escape = true;
                    } else if c == '"' {
                        state = StrState::Outside;
                    }
                }
            }
            StrState::InSingle => {
                if opened_by_smart && c == '’' {
                    out.push('\'');
                    state = StrState::Outside;
                } else {
                    out.push(c);
                    if c == '\\' {
                        escape = true;
                    } else if c == '\'' {
                        state = StrState::Outside;
                    }
                }
            }
        }
    }
    out
}

/// 规则 2:全角标点(串外)`：` → `:`,`，` → `,`。中文内容串不修改。
fn pass_fullwidth_punct(input: &str) -> String {
    scan_transform(
        input,
        &mut |c, out| match c {
            '：' => {
                out.push(':');
                Some(CharAction::Emit)
            }
            '，' => {
                out.push(',');
                Some(CharAction::Emit)
            }
            _ => None, // 其余(含引号)走默认状态机
        },
        &mut |_c, _state, _escaped, _out| None, // 串内一律默认(原样)
    )
}

/// 规则 3:单引号字符串 → 双引号(串内裸 `"` 转义为 `\"`,已转义的 `\'`
/// 归一为 `'`);双引号串内的 `'` 是内容。
///
/// 专用扫描器:反斜杠「延迟输出」—— 见到 `\` 先记账不输出,等看到被转义字符
/// 再决定去留(单引号串中 `\'` 的反斜杠要丢,`\\` / `\n` 等要保留)。
fn pass_single_quotes(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    let mut state = StrState::Outside;
    let mut escape = false;
    let mut pending_backslash = false;
    for c in input.chars() {
        if state != StrState::Outside && escape {
            // 被转义字符:先决定延迟的反斜杠去留,再输出本字符
            if pending_backslash {
                if !(state == StrState::InSingle && c == '\'') {
                    out.push('\\');
                }
                pending_backslash = false;
            }
            out.push(c);
            escape = false;
            continue;
        }
        match state {
            StrState::Outside => match c {
                '"' => {
                    out.push('"');
                    state = StrState::InDouble;
                }
                '\'' => {
                    // 单引号开串 → 换成双引号
                    out.push('"');
                    state = StrState::InSingle;
                }
                _ => out.push(c),
            },
            StrState::InDouble => {
                if c == '\\' {
                    escape = true;
                    pending_backslash = true;
                    continue;
                }
                out.push(c);
                if c == '"' {
                    state = StrState::Outside;
                }
            }
            StrState::InSingle => {
                if c == '\\' {
                    escape = true;
                    pending_backslash = true;
                    continue;
                }
                if c == '\'' {
                    // 单引号串闭合 → 输出双引号闭合
                    out.push('"');
                    state = StrState::Outside;
                } else if c == '"' {
                    // 单引号串内的裸双引号 → 转义
                    out.push_str("\\\"");
                } else {
                    out.push(c);
                }
            }
        }
    }
    // 尾部悬挂的反斜杠(畸形输入):补上,交给解析器 fail-closed
    if pending_backslash {
        out.push('\\');
    }
    out
}

/// 规则 4:字符串内裸控制字符(`\n` `\r` `\t` 及其它 < 0x20)→ 转义序列。
/// 已转义序列(`\n` 两个字符)经 escaped 标记识别,不会被二次转义。
fn pass_control_chars(input: &str) -> String {
    scan_transform(
        input,
        &mut |_c, _out| None, // 串外一律默认
        &mut |c, _state, escaped, out| {
            if escaped {
                return None; // 已转义的字符原样透传
            }
            match c {
                '\n' => {
                    out.push_str("\\n");
                    Some(CharAction::Emit)
                }
                '\r' => {
                    out.push_str("\\r");
                    Some(CharAction::Emit)
                }
                '\t' => {
                    out.push_str("\\t");
                    Some(CharAction::Emit)
                }
                c if (c as u32) < 0x20 => {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                    Some(CharAction::Emit)
                }
                _ => None,
            }
        },
    )
}

/// 规则 5:尾逗号删除 —— 串外的 `]` / `}` 前,若最后有效字符是 `,` 则移除
/// (保留中间的空白)。
fn pass_trailing_commas(input: &str) -> String {
    scan_transform(
        input,
        &mut |c, out| match c {
            ']' | '}' => {
                let trimmed = out.trim_end();
                if trimmed.ends_with(',') {
                    let ws: String = out[trimmed.len()..].to_string();
                    out.truncate(trimmed.len() - 1);
                    out.push_str(&ws);
                }
                out.push(c);
                Some(CharAction::Emit)
            }
            _ => None,
        },
        &mut |_c, _state, _escaped, _out| None,
    )
}

/// 规则 6:Python 常量(串外,整词匹配)→ JSON 常量:
/// `True/False/None` → `true/false/null`。字符串内容中的 "True" 是数据,不修改。
fn pass_python_consts(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut state = StrState::Outside;
    let mut escape = false;
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < input.len() {
        let c = input[i..].chars().next().unwrap();
        let clen = c.len_utf8();
        if state != StrState::Outside {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if (state == StrState::InDouble && c == '"')
                || (state == StrState::InSingle && c == '\'')
            {
                state = StrState::Outside;
            }
            i += clen;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let end = bytes[i..]
                .iter()
                .position(|b| !is_word(*b))
                .map(|p| i + p)
                .unwrap_or(input.len());
            match &input[i..end] {
                "True" => {
                    out.push_str("true");
                    i = end;
                    continue;
                }
                "False" => {
                    out.push_str("false");
                    i = end;
                    continue;
                }
                "None" => {
                    out.push_str("null");
                    i = end;
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
        if c == '"' {
            state = StrState::InDouble;
        } else if c == '\'' {
            state = StrState::InSingle;
        }
        i += clen;
    }
    out
}

/// Tier-2 截断补全:扫描 JSON 字符串 / 对象 / 数组的开闭配对,在 EOF 时
/// 自动追加必要的闭合字符(`"`、`}`、`]`),仅在「确实是 EOF 截断」时启用。
///
/// 设计约束:
/// - 仅在字符串/对象/数组开闭配对失衡时追加闭合,**不会插入新内容**;
/// - 若原串以「合法 JSON 字符(`,` / `}` 等)」结尾,说明不是 EOF,直接原样返回;
/// - 若发现完全无法配对(例如键值对一半被截掉),补全可能产生语义错误,
///   调用方应仍按修复失败处理(fail-closed 兜底不变)。
fn complete_truncated_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    // 元素: b'\"' = 在双引号串内; b'{' / b'[' = 在对象/数组内
    let mut stack: Vec<u8> = Vec::with_capacity(8);
    let mut escape = false; // 反斜杠转义态,仅在串内有效
    for c in input.chars() {
        // 反斜杠转义:把 \\ 自身入栈,下一字符原样透传并清 escape
        if escape {
            out.push(c);
            escape = false;
            continue;
        }
        // 是否在串内(栈顶是 ")
        let in_string = stack.last().copied() == Some(b'"');
        match c {
            '\\' if in_string => {
                // 串内的反斜杠:下一个字符原样透传
                out.push(c);
                escape = true;
            }
            '"' => {
                if in_string {
                    stack.pop(); // 闭合字符串
                } else {
                    stack.push(b'"'); // 进入字符串
                }
                out.push(c);
            }
            '{' => {
                if !in_string {
                    stack.push(b'{');
                }
                out.push(c);
            }
            '}' => {
                if !in_string && stack.last().copied() == Some(b'{') {
                    stack.pop();
                }
                out.push(c);
            }
            '[' => {
                if !in_string {
                    stack.push(b'[');
                }
                out.push(c);
            }
            ']' => {
                if !in_string && stack.last().copied() == Some(b'[') {
                    stack.pop();
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }

    if stack.is_empty() {
        return out;
    }

    // EOF 时按栈逆序补全
    let mut suffix = String::new();
    while let Some(top) = stack.pop() {
        match top {
            b'"' => suffix.push('"'),
            b'{' => suffix.push('}'),
            b'[' => suffix.push(']'),
            _ => {}
        }
    }
    if !suffix.is_empty() {
        tracing::debug!(suffix = %suffix, "Tier-2 截断补全:追加闭合字符");
    }
    out.push_str(&suffix);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::collections::HashMap;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Sample {
        name: String,
        tags: Vec<String>,
        flag: bool,
        count: Option<u32>,
    }

    // ========== 各规则单测 ==========

    #[test]
    fn smart_quotes_as_delimiters() {
        let repaired = repair_json("{“name”: “rust”}");
        assert_eq!(repaired, "{\"name\": \"rust\"}");
        let v: HashMap<String, String> = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["name"], "rust");
    }

    #[test]
    fn smart_quotes_inside_ascii_strings_untouched() {
        // ASCII 引号打开的串内,中文智能引号是数据,不应被改动
        let src = r#"{"a": "看“这个”符号"}"#;
        assert_eq!(repair_json(src), src);
    }

    #[test]
    fn fullwidth_punctuation() {
        let repaired = repair_json("{“a”：1，\"b\":2}");
        let v: HashMap<String, i32> = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn single_quoted_strings() {
        let repaired = repair_json("{'name': 'it\\'s', \"note\": \"he said 'ok'\"}");
        let v: HashMap<String, String> = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["name"], "it's");
        assert_eq!(v["note"], "he said 'ok'");
    }

    #[test]
    fn single_quotes_inner_double_quote_escaped() {
        let repaired = repair_json("{'a': '他说\"hi\"'}");
        let v: HashMap<String, String> = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["a"], "他说\"hi\"");
    }

    #[test]
    fn raw_control_chars_in_string() {
        let repaired = repair_json("{\"a\": \"line1\nline2\tend\"}");
        let v: HashMap<String, String> = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["a"], "line1\nline2\tend");
    }

    #[test]
    fn escaped_newline_not_double_escaped() {
        // 已经转义的 \n(两个字符)不应被二次转义成 \\n
        let src = r#"{"a": "line1\nline2"}"#;
        assert_eq!(repair_json(src), src);
    }

    #[test]
    fn trailing_commas_removed() {
        for src in [
            "{\"a\": 1, \"b\": [1, 2, 3,],}",
            "{\"a\": 1, \"b\": [1, 2, 3,] ,}",
            "{\n  \"a\": 1,\n}",
        ] {
            let repaired = repair_json(src);
            let v: serde_json::Value = serde_json::from_str(&repaired)
                .unwrap_or_else(|e| panic!("{src:?} → {repaired:?}: {e}"));
            assert_eq!(v["a"], 1);
        }
    }

    #[test]
    fn python_constants() {
        let repaired = repair_json("{\"flag\": True, \"count\": None, \"tags\": [True, False]}");
        let v: serde_json::Value = serde_json::from_str(&repaired)
            .unwrap_or_else(|e| panic!("{repaired}: {e}"));
        assert_eq!(v["flag"], true);
        assert!(v["count"].is_null());
        assert_eq!(v["tags"][1], false);
    }

    #[test]
    fn python_constants_inside_strings_untouched() {
        let src = r#"{"a": "True False None", "flag": True}"#;
        let repaired = repair_json(src);
        let v: serde_json::Value =
            serde_json::from_str(&repaired).unwrap_or_else(|e| panic!("{repaired}: {e}"));
        assert_eq!(v["a"], "True False None");
        assert_eq!(v["flag"], true);
    }

    #[test]
    fn identifier_containing_true_untouched() {
        // 词边界:Truthful / MyTrue 不应被替换
        let src = r#"{"Truthful": 1, "MyTrue": 2}"#;
        assert_eq!(repair_json(src), src);
    }

    // ========== 组合与边界 ==========

    #[test]
    fn combined_repairs_parse() {
        // 智能引号 + 单引号 + 尾逗号 + True + 裸换行 一起出现
        let broken = "{“name”: 'laew',\n \"tags\": ['a', 'b',], \"flag\": True}";
        let parsed: Sample = serde_json::from_str(&repair_json(broken))
            .unwrap_or_else(|e| panic!("组合修复失败: {e}"));
        assert_eq!(parsed.name, "laew");
        assert_eq!(parsed.tags, vec!["a", "b"]);
        assert!(parsed.flag);
    }

    #[test]
    fn valid_json_unchanged() {
        let src = r#"{"name":"x","tags":["t"],"flag":false,"count":null}"#;
        assert_eq!(repair_json(src), src);
    }

    #[test]
    fn oversized_input_skips_repair() {
        let big = format!("{{\"a\": \"{}\"}}", "x".repeat(MAX_REPAIR_BYTES + 1));
        assert_eq!(repair_json(&big), big, "超限输入应原样返回");
    }

    #[test]
    fn try_parse_repairs_and_parses() {
        let parsed: Sample =
            try_parse("{‘name’: 'laew', \"tags\": [\"t\",], \"flag\": True, \"count\": None}")
                .unwrap();
        assert_eq!(parsed.name, "laew");
        assert_eq!(parsed.count, None);
    }

    #[test]
    fn try_parse_unrepairable_returns_combined_diagnostic() {
        // 截断的 JSON(缺右括号)刻意不修复 → 返回合并诊断
        // (fail-closed:Quality 报告不应被自动补全)
        let err = try_parse::<Sample>("{\"name\": \"laew\"").unwrap_err();
        assert!(err.contains("原始"));
        assert!(err.contains("自动修复后仍失败"));
    }

    // ========== Tier-2 截断补全(D-001)==========
    // 关联报告: 2026-09-09_04-多轮对话测试问题分析 D-001

    #[test]
    fn truncated_string_at_eof_gets_closed() {
        // LLM 输出被 token 触顶截断,停在字符串中部
        let src = r#"{"task_level": "simple", "goal_summary": "区块链"#;
        let r = complete_truncated_json(src);
        assert!(r.ends_with(r#""}"#), "应自动闭合字符串和对象: got {r:?}");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["goal_summary"], "区块链");
    }

    #[test]
    fn truncated_array_gets_closed() {
        let src = r#"{"tags": ["a", "b"#;
        let result = complete_truncated_json(src);
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        let arr = v["tags"].as_array().unwrap();
        assert_eq!(arr, &vec![serde_json::json!("a"), serde_json::json!("b")]);
    }

    #[test]
    fn truncated_nested_object_gets_closed() {
        let src = r#"{"outer": {"inner": 1"#;
        let result = complete_truncated_json(src);
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["outer"]["inner"], 1);
    }

    #[test]
    fn truncated_with_escaped_quote() {
        // 字符串内含转义引号,补全器不应被转义引号干扰
        let src = r#"{"a": "say \"hi\" and then"#;
        let result = complete_truncated_json(src);
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["a"], r#"say "hi" and then"#);
    }

    #[test]
    fn balanced_json_unchanged_by_completion() {
        let src = r#"{"name": "x", "tags": ["a"]}"#;
        assert_eq!(complete_truncated_json(src), src);
    }

    #[test]
    fn try_parse_lenient_repairs_truncated() {
        // Yolo / MainWork 路径:截断 JSON 自动补全
        let src = r#"{"task_level": "simple", "goal_summary": "什么是区块链"#;
        #[derive(Debug, Deserialize, PartialEq)]
        struct Partial {
            task_level: String,
            goal_summary: String,
        }
        let v: Partial = try_parse_lenient(src).expect("截断补全应成功");
        assert_eq!(v.task_level, "simple");
        assert_eq!(v.goal_summary, "什么是区块链");
    }

    #[test]
    fn try_parse_does_not_repair_truncated_quality_report() {
        // Quality-Check 报告:刻意 fail-closed,不允许截断补全
        let src = r#"{"verdict": "pass", "issues"#;
        let err = try_parse::<serde_json::Value>(src).unwrap_err();
        assert!(err.contains("原始"), "应保持原 error 信息: got {err}");
        assert!(!err.contains("截断补全后仍失败"), "Quality 路径不应走 Tier-2: got {err}");
    }

    #[test]
    fn try_parse_fast_path_no_repair() {
        let src = r#"{"name":"x","tags":[],"flag":true,"count":7}"#;
        let parsed: Sample = try_parse(src).unwrap();
        assert_eq!(parsed.count, Some(7));
    }
}
