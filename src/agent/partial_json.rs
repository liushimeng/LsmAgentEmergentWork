//! 流式 partial JSON 解析(Tier-1.5 结构截断恢复)。
//!
//! 当 LLM 因 `max_tokens` / 网络中断 / idle 超时而在 tool_call 参数 JSON 中途断流时,
//! `json_buf` 是残缺 JSON。本模块尽可能恢复已完成的字段,避免整个 tool_call 退化为
//! `{"_raw": "..."}`(那会丢失所有已完整输出的字段,工具调用完全不可用)。
//!
//! 与 `json_repair.rs`(Tier-1 语法修复)的职责分离:
//! - `json_repair`:语法错误(智能引号 / 单引号 / 尾逗号 / 全角标点 / 控制字符 / Python 常量);
//! - `partial_json`(**本模块**):**结构性截断**(EOF 在值中间)—— 本不该与 Tier-1 混用,
//!   因为「截断的 Quality 报告自动补全」语义有害(第七轮 fail-closed 测试钉死),
//!   但「tool_call 参数截断后保留已输出字段」是安全且有益的。
//!
//! 设计对齐开源 `partial-json`(微软 `partial-json.ts` / npm `partial-json`):
//! - 单遍 O(N) char 扫描,`in_string` / `escape` / 结构栈状态感知;
//! - 「trailing string」语义:EOF 在字符串中间时保留已累积内容作为该字符串值;
//! - 不完整的原子值(数字/布尔/null 被截断)丢弃该 key-value;
//! - 完全无法解析 → 返回 `None`(由调用方回退 `_raw`)。
//!
//! 纯 Rust 手写,**不引入新 crate**(CLAUDE.md 约定)。

use serde_json::{json, Map, Value};

/// 超过该大小不做 partial 解析(防性能退化),对齐 `json_repair::MAX_REPAIR_BYTES`。
const MAX_PARTIAL_BYTES: usize = 512 * 1024;

/// 截断标记 key,插入到恢复出的对象中,让下游知道参数可能不完整。
pub const TRUNCATED_KEY: &str = "__truncated__";

/// 尝试把残缺 JSON 对象解析为尽可能完整的 `Value::Object`。
///
/// - 完整合法 JSON → 等价 `serde_json::from_str`(不标截断);
/// - 截断 JSON → 恢复已完成的字段 + 可选保留 trailing string + 插入
///   `TRUNCATED_KEY: true`;
/// - 完全无法解析(不以 `{` 开头 / 第一个 key 都解析不出 / 超 512KB) → `None`。
///
/// 仅处理对象顶层(工具调用参数都是 object),数组顶层不在范围内。
pub fn parse_partial_json_object(input: &str) -> Option<Value> {
    if input.len() > MAX_PARTIAL_BYTES {
        return None;
    }
    // 快速路径:完整合法 JSON 直接解,不标截断。
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(input) {
        return Some(Value::Object(map));
    }
    // 空对象 / 仅 `{` 后截断
    let trimmed = input.trim();
    if trimmed == "{}" || trimmed == "{" || trimmed.starts_with("{ ") && trimmed.len() <= 2 {
        return Some(json!({}));
    }

    let mut s = Scanner::new(input);
    match s.parse_object() {
        Ok(v) => Some(v),
        Err(ScanError::Invalid) => None,
    }
}

/// 扫描器状态。
struct Scanner<'a> {
    bytes: &'a [u8],
    len: usize,
    i: usize,
}

/// 扫描结果:对象头合法 → Ok(可能含截断);完全非法 → Invalid。
#[derive(Debug)]
enum ScanError {
    Invalid,
}

impl<'a> Scanner<'a> {
    fn new(input: &'a str) -> Self {
        let bytes = input.as_bytes();
        Self {
            bytes,
            len: bytes.len(),
            i: 0,
        }
    }

    #[inline]
    fn at_eof(&self) -> bool {
        self.i >= self.len
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        if self.i < self.len {
            Some(self.bytes[self.i])
        } else {
            None
        }
    }

    /// 解析一个 JSON 对象,允许在任意位置截断。
    fn parse_object(&mut self) -> Result<Value, ScanError> {
        self.skip_ws();
        if !self.expect_char(b'{') {
            return Err(ScanError::Invalid);
        }
        self.skip_ws();

        let mut map = Map::new();
        // 空对象 `{}`
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Object(map));
        }

        let mut any_pair = false;
        loop {
            self.skip_ws();
            if self.at_eof() {
                // EOF 在 `{` 后 / pair 间 → 截断,返回已解析的 pairs
                break;
            }
            if self.peek() == Some(b'}') {
                self.i += 1;
                return Ok(Value::Object(map));
            }

            // 解析 key
            let key = match self.parse_string() {
                Some(k) => k,
                None => break, // key 截断 → 停止
            };

            self.skip_ws();
            if !self.expect_char(b':') {
                break; // 冒号缺失 → 停止,key 丢弃
            }

            self.skip_ws();
            // 解析 value(允许 trailing string)
            let value = match self.parse_value() {
                Some(v) => v,
                None => break, // value 截断 → key 丢弃,停止
            };

            map.insert(key, value);
            any_pair = true;

            self.skip_ws();
            if self.at_eof() {
                break;
            }
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                    continue;
                }
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Object(map));
                }
                _ => break, // 噪声 → 保守停止
            }
        }

        // 截断:标标记
        if any_pair {
            map.insert(TRUNCATED_KEY.into(), Value::Bool(true));
            Ok(Value::Object(map))
        } else {
            // 没解析出任何完整 pair,但对象头 `{` 是合法的 → 空 + 标记
            map.insert(TRUNCATED_KEY.into(), Value::Bool(true));
            Ok(Value::Object(map))
        }
    }

    /// 解析一个 JSON value(完整或 trailing-string 截断)。
    fn parse_value(&mut self) -> Option<Value> {
        self.skip_ws();
        match self.peek()? {
            b'"' => self.parse_string().map(Value::String),
            b'{' => self.parse_object().ok(),
            b'[' => self.parse_array().map(Value::Array),
            b't' | b'f' => self.parse_bool().map(Value::Bool),
            b'n' => self.parse_null().map(|()| Value::Null),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => None,
        }
    }

    /// 解析数组,允许在中间截断(保留已解析元素)。
    fn parse_array(&mut self) -> Option<Vec<Value>> {
        if self.peek()? != b'[' {
            return None;
        }
        self.i += 1;
        self.skip_ws();

        let mut arr = Vec::new();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Some(arr);
        }

        loop {
            match self.parse_value() {
                Some(v) => arr.push(v),
                None => break,
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                    continue;
                }
                Some(b']') => {
                    self.i += 1;
                    return Some(arr);
                }
                _ => break,
            }
        }
        // 截断:仍返回已解析元素
        Some(arr)
    }

    /// 解析字符串;EOF 在串中间 → 返回已累积内容(trailing string)。
    fn parse_string(&mut self) -> Option<String> {
        if self.peek()? != b'"' {
            return None;
        }
        self.i += 1; // 跳过开引号
        let mut s = String::new();
        let mut escape = false;

        while self.i < self.len {
            let b = self.bytes[self.i];
            if escape {
                s.push(b as char);
                escape = false;
                self.i += 1;
                continue;
            }
            match b {
                b'\\' => {
                    s.push('\\');
                    escape = true;
                    self.i += 1;
                }
                b'"' => {
                    self.i += 1;
                    return Some(s);
                }
                _ => {
                    s.push(b as char);
                    self.i += 1;
                }
            }
        }
        // EOF 在串中间 → trailing string 语义
        Some(s)
    }

    /// 解析布尔;不完整 → None。
    fn parse_bool(&mut self) -> Option<bool> {
        let rest = std::str::from_utf8(&self.bytes[self.i..]).ok()?;
        if rest.starts_with("true")
            && (rest.len() == 4 || !rest.as_bytes()[4].is_ascii_alphanumeric())
        {
            self.i += 4;
            return Some(true);
        }
        if rest.starts_with("false")
            && (rest.len() == 5 || !rest.as_bytes()[5].is_ascii_alphanumeric())
        {
            self.i += 5;
            return Some(false);
        }
        None
    }

    /// 解析 null;不完整 → None。
    fn parse_null(&mut self) -> Option<()> {
        let rest = std::str::from_utf8(&self.bytes[self.i..]).ok()?;
        if rest.starts_with("null")
            && (rest.len() == 4 || !rest.as_bytes()[4].is_ascii_alphanumeric())
        {
            self.i += 4;
            return Some(());
        }
        None
    }

    /// 解析数字(整数/浮点/指数)。数字末尾即值边界,所以截断在数字末尾仍是合法值。
    fn parse_number(&mut self) -> Option<Value> {
        let start = self.i;
        let len = self.len;

        if self.peek()? == b'-' {
            self.i += 1;
        }

        let mut has_digits = false;
        while self.i < len && self.bytes[self.i].is_ascii_digit() {
            self.i += 1;
            has_digits = true;
        }

        if self.i < len && self.bytes[self.i] == b'.' {
            let dot = self.i;
            self.i += 1;
            let mut frac = 0;
            while self.i < len && self.bytes[self.i].is_ascii_digit() {
                self.i += 1;
                frac += 1;
            }
            if frac == 0 {
                self.i = dot; // `.` 后无数字 → 退回
            }
        }

        if self.i < len && (self.bytes[self.i] == b'e' || self.bytes[self.i] == b'E') {
            let e = self.i;
            self.i += 1;
            if self.i < len && (self.bytes[self.i] == b'+' || self.bytes[self.i] == b'-') {
                self.i += 1;
            }
            let mut exp = 0;
            while self.i < len && self.bytes[self.i].is_ascii_digit() {
                self.i += 1;
                exp += 1;
            }
            if exp == 0 {
                self.i = e; // 指数不完整 → 退回
            }
        }

        if !has_digits {
            self.i = start;
            return None;
        }

        let s = std::str::from_utf8(&self.bytes[start..self.i]).ok()?;
        serde_json::from_str::<serde_json::Number>(s)
            .ok()
            .map(Value::Number)
    }

    fn skip_ws(&mut self) {
        while self.i < self.len && self.bytes[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn expect_char(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_object_untouched() {
        let v = parse_partial_json_object(r#"{"command":"ls"}"#).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["command"], "ls");
        assert!(m.get(TRUNCATED_KEY).is_none(), "完整 JSON 不应标截断");
    }

    #[test]
    fn trailing_string_preserved() {
        let v = parse_partial_json_object(r#"{"command":"git log --on"#).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["command"], "git log --on");
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn multiple_fields_with_trailing() {
        let v = parse_partial_json_object(r#"{"command":"ls","timeout":30,"wd":"/ho"#).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["command"], "ls");
        assert_eq!(m["timeout"], 30);
        assert_eq!(m["wd"], "/ho");
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn trailing_comma_then_incomplete_key() {
        let v = parse_partial_json_object(r#"{"a":1,"b"#).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["a"], 1);
        assert!(m.get("b").is_none());
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn complete_number() {
        let v = parse_partial_json_object(r#"{"n":123456789}"#).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["n"], 123456789u64);
        assert!(m.get(TRUNCATED_KEY).is_none());
    }

    #[test]
    fn number_intact_object_unclosed() {
        let v = parse_partial_json_object(r#"{"n":123456789"#).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["n"], 123456789u64);
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn truncated_bool_discards_pair() {
        let v = parse_partial_json_object(r#"{"a":1,"f":tr"#).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["a"], 1);
        assert!(m.get("f").is_none());
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn empty_object() {
        let v = parse_partial_json_object("{}").unwrap();
        let m = v.as_object().unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn open_brace_only() {
        let v = parse_partial_json_object("{").unwrap();
        let m = v.as_object().unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn non_json_returns_none() {
        assert!(parse_partial_json_object("not json").is_none());
    }

    #[test]
    fn escaped_quote_in_string() {
        let v = parse_partial_json_object(r#"{"command":"say \"hi"#).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["command"], r#"say \"hi"#);
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn nested_object_truncated() {
        let v = parse_partial_json_object(r#"{"env":{"PATH":"/usr/b"#).unwrap();
        let m = v.as_object().unwrap();
        let inner = m["env"].as_object().unwrap();
        assert_eq!(inner["PATH"], "/usr/b");
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn array_value_truncated() {
        let v = parse_partial_json_object(r#"{"args":["a","b"#).unwrap();
        let m = v.as_object().unwrap();
        let arr = m["args"].as_array().unwrap();
        assert_eq!(arr, &vec![json!("a"), json!("b")]);
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn array_value_complete() {
        let v = parse_partial_json_object(r#"{"args":["a","b"]}"#).unwrap();
        let m = v.as_object().unwrap();
        let arr = m["args"].as_array().unwrap();
        assert_eq!(arr, &vec![json!("a"), json!("b")]);
        assert!(m.get(TRUNCATED_KEY).is_none());
    }

    #[test]
    fn float_unclosed() {
        let v = parse_partial_json_object(r#"{"pi":3.14"#).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["pi"], 3.14);
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn oversize_returns_none() {
        let big = format!(r#"{{"k":"{}"}}"#, "x".repeat(MAX_PARTIAL_BYTES + 10));
        assert!(parse_partial_json_object(&big).is_none());
    }

    #[test]
    fn trailing_newline_in_string() {
        let v = parse_partial_json_object("{\"content\":\"hello\nwor").unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["content"], "hello\nwor");
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn all_outputs_are_valid_json() {
        let cases = [
            r#"{"command":"ls"#,
            r#"{"a":1,"b":"two"#,
            r#"{"x":[1,2,3"#,
            r#"{"a":1}"#,
        ];
        for case in cases {
            let v = parse_partial_json_object(case).unwrap();
            let s = serde_json::to_string(&v).unwrap();
            let _: Value = serde_json::from_str(&s).unwrap();
        }
    }

    #[test]
    fn trailing_string_with_comma_after_complete_pair() {
        // 完整 pair 后跟逗号,再跟残缺 key → 应保留完整 pair,丢弃残缺 key
        let v = parse_partial_json_object(r#"{"a":"1","#).unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["a"], "1");
        assert_eq!(m[TRUNCATED_KEY], true);
    }

    #[test]
    fn complete_object_with_whitespace() {
        let v = parse_partial_json_object("  {  \"a\"  :  1  }  ").unwrap();
        let m = v.as_object().unwrap();
        assert_eq!(m["a"], 1);
        assert!(m.get(TRUNCATED_KEY).is_none());
    }
}
