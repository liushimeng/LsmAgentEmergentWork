//! AMD `define` 依赖数组静态补齐：无 deps 数组的 `define(id, factory)` 在注入
//! `['require','exports','module', ...静态 require('lit')]` 依赖数组后重放。
//!
//! 背景：requirejs/FIS/mod.js 等装载器对无 deps 数组的 define 走
//! `factory.toString()` 源码级依赖解析（cjsRequireRegExp）。Boa 0.20 的
//! `fn.toString()` 只返回 15 字符壳，解析必然落空：依赖不再预载，工厂在依赖
//! 未就绪时被同步调用，后续模块拿到半成品导出，"cannot convert 'null' or
//! 'undefined' to object" 穿透整文件 eval（实测百度地图 tangram 工厂，widget
//! 层整层卡死）。
//!
//! 内核掌握一切脚本源码，因此在**脚本入口**静态补齐依赖数组——与真实浏览器里
//! toString 解析出的依赖集合一致（requirejs 只收集字符串字面量参数），任何
//! 依赖 toString 解析的装载器由此恢复正确时序。本变换只改写 define 调用头，
//! 工厂体逐字节不动；已有 deps 数组的 define 不触碰。

/// 对脚本源码做 AMD 依赖数组补齐。无 `define(` 时零拷贝原样返回。
pub fn amd_fill_deps(src: &str) -> String {
    if !src.contains("define(") {
        return src.to_string();
    }
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 64);
    let mut i = 0usize;
    while i < bytes.len() {
        // 复用 diag 的词法感知跳过：字符串/模板/正则/注释整体拷贝。
        if let Some((end, kind)) = skip_trivia(bytes, i) {
            out.push_str(&src[i..end]);
            i = end;
            let _ = kind;
            continue;
        }
        // define 关键字边界。
        if bytes[i] == b'd'
            && i + 6 <= bytes.len()
            && &bytes[i..i + 6] == b"define"
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
            && !is_ident_byte(*bytes.get(i + 6).unwrap_or(&b' '))
        {
            match try_fill_define(src, i) {
                Some((end, replacement)) => {
                    out.push_str(&replacement);
                    i = end;
                    continue;
                }
                None => {}
            }
        }
        let ch_len = utf8_len(bytes[i]);
        let end = (i + ch_len).min(bytes.len());
        out.push_str(&src[i..end]);
        i = end;
    }
    out
}

/// 尝试在 `define` 关键字位置解析调用并补齐依赖数组。
/// 命中返回 `(define 调用结束下标, 替换文本)`；非目标形态返回 None。
fn try_fill_define(src: &str, define_pos: usize) -> Option<(usize, String)> {
    let bytes = src.as_bytes();
    let mut i = define_pos + 6;
    i = skip_ws(bytes, i);
    if bytes.get(i) != Some(&b'(') {
        return None;
    }
    i += 1;
    // 参数解析:只认字符串字面量 id / 数组字面量 deps / function 表达式工厂。
    let mut id_end: Option<usize> = None;
    // 第一个参数
    i = skip_ws(bytes, i);
    if bytes.get(i) == Some(&b'\'') || bytes.get(i) == Some(&b'"') {
        let end = skip_string_literal(bytes, i)?;
        id_end = Some(end);
        i = skip_ws(bytes, end);
        if bytes.get(i) == Some(&b',') {
            i = skip_ws(bytes, i + 1);
        } else if bytes.get(i) == Some(&b')') {
            // define('id') — 无工厂,不管。
            return None;
        } else {
            return None;
        }
    }
    // 第二个参数:数组(已有 deps,不触碰)或 function(需补齐)。
    if bytes.get(i) == Some(&b'[') {
        return None; // 已有 deps 数组
    }
    let factory_start = i;
    let is_fn = bytes.get(i) == Some(&b'f')
        && i + 8 <= bytes.len()
        && &bytes[i..i + 8] == b"function"
        && !is_ident_byte(*bytes.get(i + 8).unwrap_or(&b' '));
    if !is_fn && id_end.is_some() {
        return None;
    }
    if !is_fn && id_end.is_none() {
        return None;
    }
    // 定位工厂体大括号。
    let mut j = factory_start + if id_end.is_some() { 0 } else { 0 };
    // 跳过 function 关键字/函数名/形参列表。
    if bytes.get(j) == Some(&b'f') {
        j += 8;
    } else {
        return None;
    }
    j = skip_ws(bytes, j);
    if is_ident_byte(*bytes.get(j).unwrap_or(&b' ')) && bytes.get(j) != Some(&b'(') {
        while j < bytes.len() && is_ident_byte(bytes[j]) {
            j += 1;
        }
        j = skip_ws(bytes, j);
    }
    if bytes.get(j) != Some(&b'(') {
        return None;
    }
    j = match_skip_balanced(bytes, j, b'(', b')')? + 1;
    j = skip_ws(bytes, j);
    if bytes.get(j) != Some(&b'{') {
        return None; // 箭头函数或简写形态,保守不动。
    }
    let body_open = j;
    let body_close = match_skip_balanced(bytes, body_open, b'{', b'}')?;
    // 工厂体结束后应紧跟 `)`(define 收尾)或 `,`(还有后续参数)。
    let after = skip_ws(bytes, body_close + 1);
    let closes_here = matches!(bytes.get(after), Some(&b')') | Some(&b','));
    if !closes_here {
        return None;
    }
    // 收集工厂体内的静态 require('lit')。
    let deps = collect_requires(&src[body_open + 1..body_close]);
    if deps.is_empty() {
        // 无静态依赖:仅补三标识符,保持装载器 cwrap 语义。
    }
    // 构造替换文本:define(<id>, ['require','exports','module', ...deps], factory...)
    let mut repl = String::with_capacity(after - define_pos + deps.len() * 24 + 48);
    repl.push_str(&src[define_pos..factory_start]);
    repl.push_str("['require', 'exports', 'module'");
    for d in &deps {
        repl.push_str(", ");
        repl.push_str(&serde_json::to_string(d).unwrap_or_else(|_| "\"\"".into()));
    }
    repl.push_str("], ");
    repl.push_str(&src[factory_start..after]);
    Some((after, repl))
}

/// 收集源码中的静态 `require('字面量')` 调用（按首次出现去重）。
fn collect_requires(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    let mut prev: u8 = 0;
    while i < bytes.len() {
        if let Some((end, _)) = skip_trivia(bytes, i) {
            prev = bytes[end.saturating_sub(1)];
            i = end;
            continue;
        }
        let b = bytes[i];
        if b == b'r'
            && i + 7 <= bytes.len()
            && &bytes[i..i + 7] == b"require"
            && !is_ident_byte(*bytes.get(i + 7).unwrap_or(&b' '))
            && (i == 0 || !is_ident_byte(prev))
        {
            let mut j = skip_ws(bytes, i + 7);
            if bytes.get(j) == Some(&b'(') {
                j = skip_ws(bytes, j + 1);
                if bytes.get(j) == Some(&b'\'') || bytes.get(j) == Some(&b'"') {
                    if let Some(end) = skip_string_literal(bytes, j) {
                        let raw = &body[j + 1..end - 1];
                        if !raw.is_empty() && !raw.contains('\\') {
                            let lit = raw.to_string();
                            // 后续必须是 `)`。
                            let k = skip_ws(bytes, end);
                            if bytes.get(k) == Some(&b')') && !out.contains(&lit) {
                                out.push(lit);
                            }
                        }
                        prev = bytes[end - 1];
                        i = end;
                        continue;
                    }
                }
            }
        }
        prev = b;
        i += 1;
    }
    out
}

/// 跳过空白（含换行）。
fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// 若 `i` 处是字符串/模板/正则/注释起点，返回 `(结束下标, 种类)`。
fn skip_trivia(bytes: &[u8], i: usize) -> Option<(usize, u8)> {
    let b = *bytes.get(i)?;
    match b {
        b'\'' | b'"' => skip_string_literal(bytes, i).map(|e| (e, b)),
        b'`' => Some((skip_template(bytes, i), b'`')),
        b'/' => {
            if bytes.get(i + 1) == Some(&b'/') {
                let mut j = i + 2;
                while j < bytes.len() && bytes[j] != b'\n' {
                    j += 1;
                }
                Some((j, b'/'))
            } else if bytes.get(i + 1) == Some(&b'*') {
                let mut j = i + 2;
                while j + 1 < bytes.len() && !(bytes[j] == b'*' && bytes[j + 1] == b'/') {
                    j += 1;
                }
                Some(((j + 2).min(bytes.len()), b'*'))
            } else if slash_is_regex(bytes, i) {
                Some((skip_regex_literal(bytes, i), b'r'))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `/` 是正则还是除号:看前一个非空白字节。
fn slash_is_regex(bytes: &[u8], i: usize) -> bool {
    let mut j = i;
    while j > 0 {
        j -= 1;
        let c = bytes[j];
        if (c as char).is_ascii_whitespace() {
            continue;
        }
        return !(is_ident_byte(c) || c == b')' || c == b']' || c == b'}' || c.is_ascii_digit());
    }
    true
}

/// 跳过字符串字面量（单/双引号），返回结束下标（含收尾引号）。
fn skip_string_literal(bytes: &[u8], start: usize) -> Option<usize> {
    let q = *bytes.get(start)?;
    if q != b'\'' && q != b'"' {
        return None;
    }
    let mut j = start + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'\n' => return None,
            c if c == q => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// 跳过模板字符串（含 `${}` 嵌套）。
fn skip_template(bytes: &[u8], start: usize) -> usize {
    let mut j = start + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'`' => return j + 1,
            b'$' if bytes.get(j + 1) == Some(&b'{') => {
                let mut depth = 1i32;
                let mut k = j + 2;
                while k < bytes.len() && depth > 0 {
                    match bytes[k] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        b'\'' | b'"' => {
                            if let Some(e) = skip_string_literal(bytes, k) {
                                k = e;
                                continue;
                            }
                        }
                        b'`' => {
                            k = skip_template(bytes, k);
                            continue;
                        }
                        _ => {}
                    }
                    k += 1;
                }
                j = k;
            }
            _ => j += 1,
        }
    }
    bytes.len()
}

/// 跳过正则字面量，返回结束下标。
fn skip_regex_literal(bytes: &[u8], start: usize) -> usize {
    let mut j = start + 1;
    let mut in_class = false;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'[' => {
                in_class = true;
                j += 1;
            }
            b']' => {
                in_class = false;
                j += 1;
            }
            b'/' if !in_class => {
                let mut k = j + 1;
                while k < bytes.len() && is_ident_byte(bytes[k]) {
                    k += 1;
                }
                return k;
            }
            b'\n' => return j,
            _ => j += 1,
        }
    }
    j
}

/// 从 `open`（开括号）跳到配对闭括号之后，返回闭括号下标。
fn match_skip_balanced(bytes: &[u8], open: usize, ob: u8, cb: u8) -> Option<usize> {
    if bytes.get(open) != Some(&ob) {
        return None;
    }
    let mut depth = 1i32;
    let mut j = open + 1;
    while j < bytes.len() {
        if let Some((end, _)) = skip_trivia(bytes, j) {
            j = end;
            continue;
        }
        match bytes[j] {
            c if c == ob => depth += 1,
            c if c == cb => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

#[cfg(test)]
mod tests;
