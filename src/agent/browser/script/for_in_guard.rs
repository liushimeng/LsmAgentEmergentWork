//! `for...in` null/undefined 守卫:Boa 0.20 规范偏差的宿主侧等价补偿。
//!
//! Boa 0.20 的 `CreateForInIterator` 对迭代目标无条件 `to_object`,遍历
//! `null`/`undefined` 直接抛 "cannot convert 'null' or 'undefined' to object";
//! ES 规范(ForIn/OfHeadEvaluation)应是**零次迭代**。压缩后的真实站点代码
//! 大量存在 `for (var k in window.x)` 型遍历(实测百度地图 initmap 顶层即命中),
//! 一处 undefined 就把整条脚本打断。
//!
//! 补偿方式:执行前把 `for (HEAD in EXPR)` 改写为 `for (HEAD in (EXPR || {}))`。
//! 语义等价性:for-in 只枚举可枚举字符串键 ——
//! * `EXPR` 为 null/undefined:`||{}` 后零次迭代,与规范一致(Boa 抛错才是偏差);
//! * `EXPR` 为其它假值(0/""/false/NaN):包装对象与 `{}` 同样零次迭代;
//! * `EXPR` 为真值:不改写行为。
//! 改写点只出现在 `for` 头部顶层 ` in ` 处,for-of(` of `)/属性名含 "in" 均不误伤。

/// 判断字节是否为 JS 标识符字符(含 `$`/`_`/非 ASCII 按"非 ASCII 即标识符"宽松处理,
/// 只用于界定关键字边界,不追求完整 Unicode 类别)。
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// 守卫开关:`LAEW_JS_FORIN_GUARD=off` 关闭整条改写(诊断用)。
///
/// 改写是**源码级重写**,对压缩混淆代码有误伤风险(实测百度风控脚本
/// abclite 在守卫开启时抛 ToObject、关闭时脚本跑通),保留开关便于对比。
pub fn enabled() -> bool {
    !std::env::var("LAEW_JS_FORIN_GUARD")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "off" | "0" | "false" | "no"))
        .unwrap_or(false)
}

/// 对脚本源码做 `for...in` 守卫改写。无匹配时原样返回(零拷贝路径)。
pub fn guard_for_in(src: &str) -> String {
    if !enabled() {
        return src.to_string();
    }
    if !src.contains("for") || !src.contains(" in") {
        return src.to_string();
    }
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        // 字符串字面量整体跳过:内部的 "for(x in y)" 是程序数据,不是语法,
        // 改写会静默篡改字符串内容。
        if b == b'\'' || b == b'"' || b == b'`' {
            let end = skip_string(bytes, i);
            out.push_str(&src[i..end]);
            i = end;
            continue;
        }
        // 行注释整体跳过(块注释保留原样逐字节拷贝,其中不可能出现合法改写点)。
        if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            let mut end = i + 2;
            while end < bytes.len() && bytes[end] != b'\n' {
                end += 1;
            }
            out.push_str(&src[i..end]);
            i = end;
            continue;
        }
        // 逐字符拷贝;遇到可能的 `for` 关键字再尝试匹配改写。
        if b == b'f'
            && i + 3 <= bytes.len()
            && &bytes[i..i + 3] == b"for"
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
            && (i + 3 == bytes.len() || !is_ident_byte(bytes[i + 3]))
        {
            match try_rewrite_head(bytes, i) {
                Some((_head_start, expr_start, expr_end, _next)) => {
                    out.push_str(&src[i..expr_start]);
                    out.push('(');
                    out.push_str(&src[expr_start..expr_end]);
                    out.push_str("||{})");
                    i = expr_end;
                }
                None => {
                    out.push('f');
                    i += 1;
                }
            }
        } else {
            let ch_len = utf8_len(b);
            let end = (i + ch_len).min(bytes.len());
            out.push_str(&src[i..end]);
            i = end;
        }
    }
    out
}

/// 跳过一个字符串字面量,返回结束下标(含收尾引号;未闭合时保守停住)。
/// 模板字符串内的 `${}` 表达式理论上可含改写点,但保守整体跳过
/// (模板内插值极少放 for 语句)。
fn skip_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut j = start + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'\n' if quote != b'`' => {
                // 未闭合的单/双引号(压缩代码不该出现),保守停在本行。
                break;
            }
            q if q == quote => {
                j += 1;
                break;
            }
            _ => j += 1,
        }
    }
    j.min(bytes.len())
}

/// 从 `for` 关键字起尝试解析 `for (HEAD in EXPR)`,命中返回
/// `(head_start, expr_start, expr_end, after_paren)`,全部为字节下标;
/// `expr_end` 指向头部收尾 `)` 之前(表达式在源码中的原始区间)。
fn try_rewrite_head(bytes: &[u8], for_pos: usize) -> Option<(usize, usize, usize, usize)> {
    let mut i = for_pos + 3;
    // 跳过 for 与 ( 之间的空白(await 不属 for-in,忽略)。
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'(' {
        return None;
    }
    let head_start = i;
    i += 1;
    let mut depth = 1i32;
    let mut in_str: Option<u8> = None; // 引用状态:b'/' 不可出现在普通字符串里,借位标注
    let mut quote: u8 = 0;
    let mut prev_significant: u8 = 0;
    let mut expr_start = None;
    let mut j = i;
    while j < bytes.len() {
        let b = bytes[j];
        if quote != 0 {
            if quote == b'/' {
                // 正则/除号借用不区分:正则字面量里 `]`/`}` 不闭合,简化按转义处理
                if b == b'\\' {
                    j += 2;
                    continue;
                }
                if b == b'/' {
                    quote = 0;
                }
            } else if b == b'\\' {
                j += 2;
                continue;
            } else if b == quote {
                quote = 0;
            }
            prev_significant = b;
            j += 1;
            continue;
        }
        match b {
            b'\'' | b'"' | b'`' => {
                quote = b;
                in_str = Some(b);
            }
            b'/' if prev_significant != b')' && prev_significant != b']' && prev_significant != 0
                && !is_ident_byte(prev_significant) =>
            {
                // 简化:除号出现在标识符/`)`/`]` 之后,其余按正则开头处理。
                quote = b'/';
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    // 头部结束:需要已定位到顶层 ` in `;
                    // 表达式区间两侧去掉空白,改写紧凑不引入语法歧义。
                    let es = expr_start?;
                    let mut start = es;
                    while start < j && (bytes[start] as char).is_ascii_whitespace() {
                        start += 1;
                    }
                    let mut end = j;
                    while end > start && (bytes[end - 1] as char).is_ascii_whitespace() {
                        end -= 1;
                    }
                    return Some((head_start, start, end, j + 1));
                }
            }
            b';' if depth == 1 => return None, // for(;;) 经典头,无 in
            _ => {}
        }
        // 顶层 ` in ` 关键字识别:for-in 头部**第一个**顶层 `in` 即分隔符
        // (其后仍可出现 `in` 运算符,但必在括号/字符串内,深度>1)。
        // 只需保证 `in` 不是更长标识符的一部分(如变量名 internet/inx):
        // 后随字节须非标识符字符;前随字符是循环变量本身(标识符),不设限。
        if depth == 1 && quote == 0 && b == b'i' && bytes.get(j + 1) == Some(&b'n') {
            let after = bytes.get(j + 2).copied().unwrap_or(b')');
            if !is_ident_byte(after) && expr_start.is_none() {
                expr_start = Some(j + 2);
            }
        }
        if !b.is_ascii_whitespace() || quote != 0 {
            prev_significant = b;
        }
        j += 1;
    }
    let _ = in_str;
    None
}

/// UTF-8 序列首字节对应的字符长度。
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
mod tests {
    use super::*;

    #[test]
    fn 零匹配原样返回() {
        let src = "var a = 1; function f() { return 'for (x in y)'; }";
        assert_eq!(guard_for_in(src), src);
    }

    #[test]
    fn 基本改写_var头() {
        let out = guard_for_in("for(var k in obj){f(k)}");
        assert_eq!(out, "for(var k in (obj||{})){f(k)}");
    }

    #[test]
    fn 裸标识符头与预声明头() {
        assert_eq!(guard_for_in("for(k in obj);"), "for(k in (obj||{}));");
        assert_eq!(
            guard_for_in("var k;for(k in win.x){}"),
            "var k;for(k in (win.x||{})){}"
        );
    }

    #[test]
    fn 嵌套括号与字符串不误伤() {
        // 表达式含括号/字符串,`in` 出现在字符串内不应定位错误
        let out = guard_for_in("for (var k in foo(bar(' in '), 1)) { use(k); }");
        assert_eq!(out, "for (var k in (foo(bar(' in '), 1)||{})) { use(k); }");
        // for-of 不改写
        let of_src = "for (const x of list) { g(x); }";
        assert_eq!(guard_for_in(of_src), of_src);
        // 经典 for(;;) 不改写
        let c_src = "for (var i = 0; i < 10; i++) { h(i); }";
        assert_eq!(guard_for_in(c_src), c_src);
    }

    #[test]
    fn 连续与多行() {
        let out = guard_for_in("for(var a in x){}for(var b in y){}");
        assert_eq!(out, "for(var a in (x||{})){}for(var b in (y||{})){}");
        let multi = "for (var k\n  in obj) {}";
        assert_eq!(guard_for_in(multi), "for (var k\n  in (obj||{})) {}");
    }

    #[test]
    fn 中文与非ascii不截断() {
        let out = guard_for_in("var s='中文';for(var k in obj){}var t='中文尾';");
        assert_eq!(out, "var s='中文';for(var k in (obj||{})){}var t='中文尾';");
    }

    #[test]
    fn 大小写与关键字边界() {
        // `for` 出现在标识符内部(perform / forx)不改写
        let src = "perform(x); forx(y);";
        assert_eq!(guard_for_in(src), src);
    }
}

