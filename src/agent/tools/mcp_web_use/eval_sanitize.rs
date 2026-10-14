//! `eval_js` / `download` 返回值净化(第 99 轮起, 第 135 轮扩到复合值)。
//!
//! 职责:**在返回值进入 tool_result 之前把大载荷掐断**,防止一次网页抓取把
//! 上下文炸掉。三个分支:
//!
//! | 返回值形态 | 判据 | 处理 |
//! | --- | --- | --- |
//! | `data:` URL 字符串 | 前缀 `data:` | base64/百分号解码后按 mime 落盘 |
//! | 普通长字符串 | 字符数 > [`EVAL_INLINE_CHAR_LIMIT`] | 原文落盘,只回 head + 路径 |
//! | **数组 / 对象**(第 135 轮) | 序列化字节数 > [`EVAL_STRUCT_BYTE_LIMIT`] | 先就地裁剪,仍超限再落盘 |
//!
//! ## 第 135 轮:为什么加复合值闸门
//!
//! 实测事故(`llaew_20261008_173357.log`,36 氪文章列表抓取):第 99 轮的实现
//! 只对 `Value::String` 生效,`Value::Array` / `Value::Object` 直接 `return val`
//! 原样透传。于是第一轮 24 次迭代里出现 4 次 8.5KB~19.2KB 的 eval_js 大对象,
//! 其中一次 17440 B **落盘后 Agent 从未 Read** —— 白烧一轮迭代加一次 LLM 往返,
//! QC 在 issues 里点名了这一点。
//!
//! MCP_Web_Use 因此成为全工程**唯一没有工具级输出预算的富数据工具**(对比 Bash
//! 工具有 `MAX_OUTPUT_CHARS = 30_000` + `bash_spill.rs` 落盘回灌)。
//!
//! 关键取舍:**超限时不就地丢整块,先裁剪再落盘**。裁剪后的数组仍带着最有价值
//! 的前若干条完整记录,模型看完 head 就能判断「数据够不够、要不要再用精确路径
//! eval_js 取子集」,而不是面对一个空信封重新猜一遍。
//!
//! ## 2026-10-14:输入侧整形(顶层裸 `return` 自动包裹)
//!
//! 本模块只做了「返回值出」的闸门;LLM 的**输入形状**也有一个高频翻车点 —— 写出
//! `const x=..; return JSON.stringify(x)` 这类顶层裸 `return` 表达式,`Runtime.evaluate`
//! 是经典脚本环境,顶层 `return` 必然 `SyntaxError: Illegal return statement`
//! (天猫任务实测 3 次,每次模型自行改写重试烧 1~2 轮迭代 + 一次 60~130s 网关往返)。
//! [`wrap_bare_toplevel_return`] 在 `eval_js_string` 网关做零成本吸收。

use base64::Engine;
use serde_json::{json, Value};

use super::now_millis_safe;

/// eval_js 单个字符串的内联上限(字符数);超过即落盘。
pub(super) const EVAL_INLINE_CHAR_LIMIT: usize = 2000;

/// eval_js 复合值(数组/对象)的内联上限(**字节数**,序列化后判定)。
///
/// 与 Bash 工具的 `MAX_OUTPUT_CHARS = 30_000` 同一量级且按字节口径
/// (中文 2000 字符 ≈ 6KB,字符口径会低估三倍)。
pub(super) const EVAL_STRUCT_BYTE_LIMIT: usize = 20_000;

/// 复合值就地裁剪时,数组保留的最大元素数。
pub(super) const EVAL_STRUCT_KEEP_ITEMS: usize = 50;

/// 复合值就地裁剪时,对象内递归保留的最大深度(再深直接替换为占位串)。
pub(super) const EVAL_STRUCT_MAX_DEPTH: usize = 8;

/// 落盘文件的进程内递增序号(消除同毫秒并发互相覆盖)。
///
/// 实测:一次 `sequence` 里连续多次 eval_js 落盘,毫秒时间戳可能相同导致
/// 后写覆盖先写,模型读到的路径内容被悄悄换掉。
static SPILL_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 生成一个不会互相覆盖的落盘文件名(不含扩展名部分)。
fn spill_stem() -> String {
    let seq = SPILL_SEQ
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("laew_web_result_{}_{}", now_millis_safe(), seq)
}

/// eval_js 输入侧整形:顶层裸 `return` 自动包裹成 IIFE(2026-10-14)。
///
/// 背景(天猫任务实测 3 次,`llaew_20261014_122337.log`):LLM 会写出
/// `const x=..; return JSON.stringify(x)` 这类顶层裸 `return` 表达式;
/// `Runtime.evaluate` 是经典脚本环境,顶层 `return` 必然
/// `SyntaxError: Illegal return statement`,模型每次自行改写成 `(()=>{…})()` 重试,
/// 烧 1~2 轮迭代 + 一次 60~130s 网关往返(其中一次发生在 sequence 步里导致整批
/// `completed:false`)。
///
/// 判定(保守):[`scan_toplevel_keywords`] 逐字符扫描,深度 0(`()[]{}` 配对之外)、
/// 双侧词法边界完整(前后都不是 `[A-Za-z0-9_$.]`)的 `return` 才算顶层裸 return;
/// 字符串字面量(`'`/`"`/`` ` ``,含 `\` 转义)与注释(`//`、`/* */`)整体跳过。
/// 命中且**没有**顶层裸 `await` —— async 包裹会把返回值变成 Promise、改变
/// `return_by_value` 语义,保守不碰(模型下一轮自纠,实测可自愈)—— 才返回
/// `(() => { <原文> })()`;其余一律原样返回:IIFE / 函数体内 return / `obj.return`
/// 属性名 / 字符串与注释里的 return 全部零扰动。
///
/// 已知盲区(可忽略):深度 0 的正则字面量含 `return`(如 `/return/.test(s)`)会被
/// 误判命中 —— 包裹后仍是合法 JS,只是该表达式的值变成 `undefined`,模型下一轮
/// 自纠;触发频率远低于裸 return 本身。
pub(super) fn wrap_bare_toplevel_return(js: &str) -> String {
    let (has_return, has_await) = scan_toplevel_keywords(js);
    if !has_return || has_await {
        return js.to_string();
    }
    format!("(() => {{ {js} }})()")
}

/// 深度 0 关键字扫描:返回 (顶层裸 return 在场, 顶层裸 await 在场)。
///
/// 不追求完整 JS 词法(正则/模板串 `${}` 简化按字面处理,见上方盲区说明),
/// 只要求「合法表达式里的 return/await 一定位于某个 `()[]{}` 配对之内」——
/// 这对所有正常的函数体/IIFE/对象字面量/调用式都成立。
fn scan_toplevel_keywords(src: &str) -> (bool, bool) {
    let b = src.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    let mut depth: i32 = 0;
    let (mut has_return, mut has_await) = (false, false);
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    while i < n {
        let c = b[i];
        match c {
            b'\'' | b'"' | b'`' => {
                // 字符串字面量:跳到配对引号(`\` 转义连跳两字符)
                let quote = c;
                i += 1;
                while i < n {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == quote {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'/' if i + 1 < n && b[i + 1] == b'/' => {
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(n);
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                i += 1;
            }
            _ if depth == 0 && (c == b'r' || c == b'a') => {
                // 前边界:行首,或前一字符不是词符/`.`(后者排掉 obj.return)
                let prev_ok = i == 0 || {
                    let p = b[i - 1];
                    !is_word(p) && p != b'.'
                };
                if prev_ok {
                    // 后边界:关键字结尾的下一字符不是词符(`myreturn` 不算)
                    let boundary_after = |len: usize| -> bool {
                        let j = i + len;
                        j >= n || !is_word(b[j])
                    };
                    if b[i..].starts_with(b"return") && boundary_after(6) {
                        has_return = true;
                    }
                    if b[i..].starts_with(b"await") && boundary_after(5) {
                        has_await = true;
                    }
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    (has_return, has_await)
}

/// eval_js 返回值净化入口。
///
/// 小结果**一字不动**地透传(向后兼容);只有超过阈值或 `data:` 前缀才介入。
pub(super) fn sanitize_eval_result(val: Value) -> Value {
    // 分支一:字符串(data: URL / 超长文本)
    if let Value::String(s) = &val {
        let is_data = s.starts_with("data:");
        if !is_data && s.chars().count() <= EVAL_INLINE_CHAR_LIMIT {
            return val;
        }
        return spill_string_result(s);
    }

    // 分支二:复合值(第 135 轮)
    if val.is_array() || val.is_object() {
        let text = serde_json::to_string(&val).unwrap_or_default();
        if text.len() <= EVAL_STRUCT_BYTE_LIMIT {
            return val;
        }
        let (kept, of, depth_capped) = shrink_struct(&val, 0);
        let shrunk_text = serde_json::to_string(&kept).unwrap_or_default();
        // 裁剪后仍超限 → 落盘;否则直接回裁剪结果(信息量更大,省一次 IO)
        if shrunk_text.len() > EVAL_STRUCT_BYTE_LIMIT {
            return spill_json_result(&kept, &text);
        }
        return annotate_truncated(kept, of, text.len(), depth_capped);
    }

    val
}

/// 就地裁剪复合值:数组截断到 [`EVAL_STRUCT_KEEP_ITEMS`] 项,对象递归瘦身。
///
/// 返回 `(裁剪后的值, 原数组元素数或 0, 是否触到深度上限)`。
fn shrink_struct(v: &Value, depth: usize) -> (Value, usize, bool) {
    let depth_capped = depth >= EVAL_STRUCT_MAX_DEPTH;
    match v {
        Value::Array(arr) => {
            let of = arr.len();
            let mut kept: Vec<Value> = arr
                .iter()
                .take(EVAL_STRUCT_KEEP_ITEMS)
                .map(|item| shrink_struct(item, depth + 1).0)
                .collect();
            // 保留数组头部标量字段的可读性:元素若是对象,只留标量(数组/对象递归再裁)
            for item in kept.iter_mut() {
                if let Value::Object(_) = item {
                    if let Value::Object(obj) = item {
                        obj.retain(|_, val| !val.is_array() && !val.is_object());
                    }
                }
            }
            if of > EVAL_STRUCT_KEEP_ITEMS {
                kept.push(json!({"__skipped__": of - EVAL_STRUCT_KEEP_ITEMS}));
            }
            (Value::Array(kept), of, depth_capped)
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                if val.is_array() || val.is_object() {
                    if depth_capped {
                        out.insert(k.clone(), json!("…(嵌套过深,已省略)"));
                    } else {
                        out.insert(k.clone(), shrink_struct(val, depth + 1).0);
                    }
                } else {
                    out.insert(k.clone(), val.clone());
                }
            }
            (Value::Object(out), 0, depth_capped)
        }
        other => (other.clone(), 0, false),
    }
}

/// 给裁剪后的值追加 `__truncated__` 标注(字段级裁剪后仍内联的情况)。
fn annotate_truncated(v: Value, of: usize, original_bytes: usize, depth_capped: bool) -> Value {
    let mut obj = match v {
        Value::Object(map) => map,
        Value::Array(arr) => {
            let mut m = serde_json::Map::new();
            m.insert("items".into(), Value::Array(arr));
            m
        }
        other => return other,
    };
    obj.insert(
        "__truncated__".into(),
        json!({
            "kept": EVAL_STRUCT_KEEP_ITEMS.min(if of == 0 { EVAL_STRUCT_KEEP_ITEMS } else { of }),
            "of": of,
            "original_bytes": original_bytes,
            "depth_capped": depth_capped,
        }),
    );
    Value::Object(obj)
}

/// eval_js 大结果落盘目录(第 152 轮:产物根下的 `EvalSpill/`,不再落系统临时目录)。
///
/// 落 temp 目录的痛点:用户在任务里拿到 `saved_to` 绝对路径却发现文件"不见了"
/// (系统临时目录会被清理、且与工作目录不在一起);统一落工作目录后,产物与 Plan /
/// 报告同处一地,`ls` 一下就能看到。
fn spill_path(file_name: &str) -> std::path::PathBuf {
    let dir = crate::artifact_root::artifact_dir("EvalSpill");
    let _ = std::fs::create_dir_all(&dir);
    crate::artifact_root::safe_join(&dir, file_name)
}

/// 字符串分支:落盘 + 只回 head + 路径(第 99 轮行为原样保留)。
fn spill_string_result(s: &str) -> Value {
    let (byte_size, saved_to) = match decode_data_url(s) {
        Some(bytes) => {
            let path = spill_path(&format!(
                "{}.{}",
                spill_stem(),
                data_ext_from_mime(data_mime(s))
            ));
            match std::fs::write(&path, &bytes) {
                Ok(_) => (bytes.len(), Some(path.display().to_string())),
                Err(_) => (bytes.len(), None),
            }
        }
        None => {
            let path = spill_path(&format!("{}.txt", spill_stem()));
            match std::fs::write(&path, s) {
                Ok(_) => (s.len(), Some(path.display().to_string())),
                Err(_) => (s.len(), None),
            }
        }
    };
    let head: String = s.chars().take(160).collect();
    let total = s.chars().count();
    let mut out = json!({
        "result_truncated": true,
        "result_head": head,
        "result_len": total,
        "byte_size": byte_size,
        "hint": "返回值过大(或为 data: URL),已自动落盘;需要内容时用 Read 工具读 saved_to 路径,不要把原始数据塞进后续工具参数",
    });
    if let Some(p) = saved_to {
        out["saved_to"] = json!(p);
    }
    out
}

/// 复合值落盘分支(裁剪后仍超限时):与字符串分支**同一套信封形状**,
/// 模型已经学会读 `saved_to`,不需要学第二种格式。
fn spill_json_result(shrunk: &Value, original_text: &str) -> Value {
    let path = spill_path(&format!("{}.json", spill_stem()));
    let saved_to = std::fs::write(&path, original_text)
        .ok()
        .map(|_| path.display().to_string());
    let head: String = original_text.chars().take(160).collect();
    let mut out = json!({
        "result_truncated": true,
        "value_kind": if original_text.starts_with('[') { "array" } else { "object" },
        "result_head": head,
        "result_len": original_text.len(),
        "byte_size": original_text.len(),
        "preview_items": EVAL_STRUCT_KEEP_ITEMS,
        "hint": "返回值过大(数组/对象),已裁剪并把完整内容落盘;需要精确子集时用 eval_js 自行取路径(如 JSON.stringify(全局变量.属性.列表.slice(0,20))),不要把原始数据塞进后续工具参数",
    });
    if let Some(p) = saved_to {
        out["saved_to"] = json!(p);
    }
    // 裁剪后的前若干条直接内联,让模型无需 IO 就能判断数据结构
    out["preview"] = shrunk.clone();
    out
}

/// data: URL 的 mime 段(如 `data:image/png;base64,` → `image/png`)。
pub(super) fn data_mime(s: &str) -> &str {
    s.strip_prefix("data:")
        .and_then(|rest| rest.split(';').next())
        .unwrap_or("")
}

/// mime → 落盘扩展名。
pub(super) fn data_ext_from_mime(mime: &str) -> &str {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "text/plain" => "txt",
        "text/html" => "html",
        "application/json" => "json",
        "application/pdf" => "pdf",
        _ => "bin",
    }
}

/// 解码 data: URL 载荷(base64 或百分号编码);非 data: 前缀返回 None。
pub(super) fn decode_data_url(s: &str) -> Option<Vec<u8>> {
    let rest = s.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    if meta.contains(";base64") {
        base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .ok()
    } else {
        percent_decode(payload).map(String::into_bytes)
    }
}

/// 百分号解码(URL 编码),`+` 视为空格;非法序列返回 None。
pub(super) fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                let hex_str = std::str::from_utf8(hex).ok()?;
                let v = u8::from_str_radix(hex_str, 16).ok()?;
                out.push(v);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 顶层裸return自动包裹() {
        assert_eq!(
            wrap_bare_toplevel_return("return 1+1"),
            "(() => { return 1+1 })()"
        );
        // 多语句 + 换行 + 前导空白:原文原样进 IIFE
        let multi = "const x=1;\n  return JSON.stringify(x)";
        assert_eq!(wrap_bare_toplevel_return(multi), format!("(() => {{ {multi} }})()"));
    }

    #[test]
    fn IIFE与函数体内return不包裹() {
        assert_eq!(wrap_bare_toplevel_return("(() => { return 1 })()"), "(() => { return 1 })()");
        assert_eq!(
            wrap_bare_toplevel_return("function f(){ return 1 }; f()"),
            "function f(){ return 1 }; f()"
        );
        assert_eq!(
            wrap_bare_toplevel_return("async function g(){ return await fetch(u) }"),
            "async function g(){ return await fetch(u) }"
        );
    }

    #[test]
    fn 边界形态不误伤() {
        // 属性名 / 词中段 / 字符串与模板串 / 注释 / 对象字面量键
        assert_eq!(wrap_bare_toplevel_return("x.return"), "x.return");
        assert_eq!(wrap_bare_toplevel_return("myreturn = 1"), "myreturn = 1");
        assert_eq!(
            wrap_bare_toplevel_return("var s = 'return 1'; s"),
            "var s = 'return 1'; s"
        );
        assert_eq!(
            wrap_bare_toplevel_return("var t = `tpl return ${x}`; t"),
            "var t = `tpl return ${x}`; t"
        );
        assert_eq!(
            wrap_bare_toplevel_return("// return 1\n1+1"),
            "// return 1\n1+1"
        );
        assert_eq!(
            wrap_bare_toplevel_return("/* return 1 */ 1+1"),
            "/* return 1 */ 1+1"
        );
        assert_eq!(
            wrap_bare_toplevel_return("JSON.stringify({a:1}).slice(0,2)"),
            "JSON.stringify({a:1}).slice(0,2)"
        );
    }

    #[test]
    fn 顶层await与return并存不包裹() {
        // async 包裹会改返回值语义(Promise),保守原样返回让模型自纠
        let js = "await fetch(u); return 1";
        assert_eq!(wrap_bare_toplevel_return(js), js);
        // await 在函数体内不算顶层
        assert_eq!(
            wrap_bare_toplevel_return("async ()=>{ await x; return 1 }"),
            "async ()=>{ await x; return 1 }"
        );
    }
}
