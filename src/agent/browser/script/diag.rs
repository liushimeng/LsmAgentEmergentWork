//! 语句级 JS 执行错误定位器（env 门控 `LAEW_JS_DIAG=1`，默认零行为变化）。
//!
//! 背景：Boa 0.20 的运行时错误不带任何源码位置（`JsError` 无 stack 字段、
//! JS 侧 `e.stack` 为空、AST 节点不携带 span），压缩脚本失败时只剩
//! "TypeError: cannot convert 'null' or 'undefined' to object" 一句，无从定位
//! （实测百度地图 `libs_b18b356.js` tangram 工厂 400KB 因此整层卡死）。
//!
//! 定位原理：**词法级语句拆分 + 前缀重放二分**。
//! * 自研 tokenizer 在 depth-0 的 `;` 切分语句（字符串/模板 `${}` 嵌套/注释/
//!   正则 vs 除法全部状态机处理，不做无验证的正则文本切分）；
//! * 候选片段包进 `function(require,exports,module){...}` 驱动器直接执行，
//!   绕过站点装载器的模块缓存（重复 define 不会"已定义跳过"导致不复现）；
//! * 累积前缀首个抛错的分界即 culprit 语句；再对语句内函数体做
//!   「截断函数体」重放二分，逐层下钻直到语句小于阈值；
//! * AMD 感知：顶层 `define(id, factory)` 直接对其工厂体下钻，
//!   跳过站点 requirejs 的注册/引导层。
//!
//! 切分偏差由 SyntaxError 兜底：前缀重放抛 SyntaxError 视为切分界无效，跳过分界继续。
//! 报告为多层 culprit 语句文本 + 位置链，供人读源码落地针对性 shim。

use boa_engine::{Context, JsValue, Source};

/// 语句报告阈值： culprit 语句短于此值即停止下钻。
const REPORT_THRESHOLD: usize = 1500;
/// 前缀扫描粗粒度步长（语句数）：先按块扫描定位粗块，再块内线性精查。
const CHUNK: usize = 32;

/// 诊断开关：仅 `LAEW_JS_DIAG` 为真时启用。
pub fn diag_enabled() -> bool {
    std::env::var("LAEW_JS_DIAG")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// 源码区间（字节下标，[start, end)）。
#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

impl Span {
    fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
}

/// 词法扫描器：产出"可语法感知跳过"的 token 流视角。
/// 只关心括号深度与语句终结点，不做完整 AST。
struct Lexer<'a> {
    bytes: &'a [u8],
    /// 字符串/正则引用字符（0 = 不在引用内）。
    quote: u8,
    /// 上一个有效 token 首字节（正则 vs 除法判定用；0 = 无）。
    prev: u8,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            bytes: src.as_bytes(),
            quote: 0,
            prev: 0,
        }
    }

    /// 跳过空白与注释（行/块），返回 `(下标, 跳过段内是否含换行)`。
    fn skip_ws_comments(&self, mut i: usize) -> (usize, bool) {
        let mut saw_newline = false;
        loop {
            while i < self.bytes.len()
                && (self.bytes[i] as char).is_ascii_whitespace()
            {
                if self.bytes[i] == b'\n' {
                    saw_newline = true;
                }
                i += 1;
            }
            if i + 1 < self.bytes.len() && self.bytes[i] == b'/' && self.bytes[i + 1] == b'/' {
                while i < self.bytes.len() && self.bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if i + 1 < self.bytes.len() && self.bytes[i] == b'/' && self.bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < self.bytes.len() && !(self.bytes[i] == b'*' && self.bytes[i + 1] == b'/') {
                    if self.bytes[i] == b'\n' {
                        saw_newline = true;
                    }
                    i += 1;
                }
                i = (i + 2).min(self.bytes.len());
                continue;
            }
            return (i, saw_newline);
        }
    }

    /// 判定 `i` 处的 `/` 是正则开头还是除号（依据上一个有效 token）。
    fn slash_is_regex(&self, i: usize) -> bool {
        let b = self.bytes[i];
        debug_assert_eq!(b, b'/');
        if self.prev == 0 {
            return true;
        }
        let p = self.prev;
        // 标识符/数字/`)`/`]`/`}` 之后是除号；其余（运算符、关键字、`(`、`[`、`,` 等）之后是正则。
        !(is_ident_byte(p) || p == b')' || p == b']' || p == b'}' || p.is_ascii_digit())
    }

    /// 跳过字符串/模板/正则字面量，返回结束下标（首个不被消费的字节）。
    fn skip_literal(&self, start: usize) -> usize {
        let bytes = self.bytes;
        let q = bytes[start];
        if q == b'`' {
            return skip_template(bytes, start);
        }
        let mut j = start + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => j += 2,
                b'\n' => return j, // 未闭合单/双引号保守停住
                c if c == q => return j + 1,
                _ => j += 1,
            }
        }
        j
    }

    /// 跳过正则字面量（`/` 开头；已判定是正则），返回结束下标。
    fn skip_regex(&self, start: usize) -> usize {
        let bytes = self.bytes;
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
                    // 消费标志位 [gimsuy]*
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
}

/// 跳过模板字符串（含 `${}` 嵌套表达式），返回结束下标（含收尾 backtick）。
fn skip_template(bytes: &[u8], start: usize) -> usize {
    let mut j = start + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'`' => return j + 1,
            b'$' if bytes.get(j + 1) == Some(&b'{') => {
                // 进入表达式模式：按括号配对找 `}`，内部可再嵌套模板/字符串。
                let mut depth = 1i32;
                let mut k = j + 2;
                while k < bytes.len() && depth > 0 {
                    match bytes[k] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        b'\'' | b'"' => k = skip_quoted(bytes, k),
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
    j
}

/// 跳过单/双引号字符串（与 Lexer::skip_literal 同规则，供自由函数复用）。
fn skip_quoted(bytes: &[u8], start: usize) -> usize {
    let q = bytes[start];
    let mut j = start + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'\n' => return j,
            c if c == q => return j + 1,
            _ => j += 1,
        }
    }
    j
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// 顶层语句拆分：在 depth-0 的 `;` 处切分；末尾残余（无 `;` 收尾）整体为最后一条。
/// 空白/纯注释段不产生空语句。注释已由 skip_ws_comments 消费，此处不再出现 `/` 开头注释。
/// 另含 ASI 启发：depth-0 换行处，前一 token 可终结语句且后一 token 可开启新语句时切分
/// （覆盖 `if (...){...}` 无分号收尾的语句；误切由前缀重放的 SyntaxError 兜底跳过）。
/// 词法感知 {} 深度：跳过字符串/模板/正则/注释后统计的最终深度（均衡代码为 0）。
fn balance_depth(src: &str) -> i32 {
    let mut lex = Lexer::new(src);
    let bytes = lex.bytes;
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        i = lex.skip_ws_comments(i).0;
        if i >= bytes.len() {
            break;
        }
        let b = bytes[i];
        match b {
            b'\'' | b'"' | b'`' => {
                i = lex.skip_literal(i);
            }
            b'/' if lex.slash_is_regex(i) => {
                i = lex.skip_regex(i);
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
            }
            _ => {
                lex.prev = b;
                i += 1;
            }
        }
    }
    depth
}

fn split_top_level(src: &str) -> Vec<Span> {
    let mut lex = Lexer::new(src);
    let bytes = lex.bytes;
    let mut spans = Vec::new();
    let mut stmt_start = 0usize;
    let mut i = 0usize;
    let mut depth = 0i32;
    while i < bytes.len() {
        let (ni, saw_newline) = lex.skip_ws_comments(i);
        // ASI 启发：跳过的空白段含换行、depth-0、前一 token 可终结语句、
        // 下一 token 可开启新语句 → 切分（else/catch/finally 续行除外）。
        if saw_newline
            && depth == 0
            && can_end_statement(lex.prev)
            && ni < bytes.len()
            && ni > stmt_start
        {
            let next = bytes[ni];
            if can_start_statement(next) && !continues_block(&src[ni..]) {
                push_stmt(&mut spans, src, stmt_start, ni);
                stmt_start = ni;
            }
        }
        i = ni;
        if i >= bytes.len() {
            break;
        }
        let b = bytes[i];
        match b {
            b'\'' | b'"' | b'`' => {
                let end = lex.skip_literal(i);
                lex.prev = bytes[end.saturating_sub(1)];
                i = end;
            }
            b'/' if lex.slash_is_regex(i) => {
                let end = lex.skip_regex(i);
                lex.prev = b'/';
                i = end;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                lex.prev = b;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                lex.prev = b;
                i += 1;
            }
            b';' if depth == 0 => {
                push_stmt(&mut spans, src, stmt_start, i + 1);
                stmt_start = i + 1;
                lex.prev = b';';
                i += 1;
            }
            _ => {
                lex.prev = b;
                i += 1;
            }
        }
    }
    push_stmt(&mut spans, src, stmt_start, src.len());
    spans
}

/// 前一 token 是否可作为语句结尾（标识符/数字/字符串尾/右括号类）。
fn can_end_statement(prev: u8) -> bool {
    prev != 0
        && (is_ident_byte(prev)
            || prev.is_ascii_digit()
            || prev == b')'
            || prev == b']'
            || prev == b'}'
            || prev == b'`'
            || prev == b'\''
            || prev == b'"')
}

/// 后一字符是否可开启新语句（标识符/数字/左括号类/字符串起点；
/// 运算符 `+`/`-`/`.`/`,` 等视为续行，不切）。
fn can_start_statement(c: u8) -> bool {
    c != 0
        && (is_ident_byte(c)
            || c.is_ascii_digit()
            || c == b'('
            || c == b'['
            || c == b'{'
            || c == b'`'
            || c == b'\''
            || c == b'"')
}

/// 识别续行关键字：else / catch / finally（`}` 换行后接这些词时属于同一条语句）。
fn continues_block(rest: &str) -> bool {
    let word: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    matches!(word.as_str(), "else" | "catch" | "finally")
}

fn push_stmt(spans: &mut Vec<Span>, src: &str, start: usize, end: usize) {
    let seg = &src[start..end];
    if seg.trim().is_empty() {
        return;
    }
    // 去掉首尾空白，让前缀拼接紧凑（trim 后长度差即空白字符数）。
    let mut s = start + (seg.len() - seg.trim_start().len());
    let e = end - (seg.len() - seg.trim_end().len());
    // 再去掉段首注释（块/行），让语句文本从真实语法起点开始。
    loop {
        let rest = &src[s..e];
        let t = rest.trim_start();
        let off = rest.len() - t.len();
        s += off;
        if t.starts_with("/*") {
            if let Some(p) = t.find("*/") {
                s += p + 2;
                continue;
            }
        } else if t.starts_with("//") {
            if let Some(p) = t.find('\n') {
                s += p + 1;
                continue;
            }
        }
        break;
    }
    if s < e {
        spans.push(Span { start: s, end: e });
    }
}

/// 函数体区间：`body_open` 指向 `{`，`body_close` 指向配对 `}`（均为字节下标）。
#[derive(Debug, Clone, Copy)]
struct BodyRange {
    body_open: usize,
    body_close: usize,
}

/// 在语句源码内扫描函数表达式/声明体（词法感知），返回全部函数体区间（按源码顺序）。
fn find_function_bodies(stmt_src: &str) -> Vec<BodyRange> {
    let mut out = Vec::new();
    let mut lex = Lexer::new(stmt_src);
    let bytes = lex.bytes;
    let mut i = 0usize;
    while i < bytes.len() {
        i = lex.skip_ws_comments(i).0;
        if i >= bytes.len() {
            break;
        }
        let b = bytes[i];
        if b == b'\'' || b == b'"' || b == b'`' {
            i = lex.skip_literal(i);
            continue;
        }
        if b == b'/' && lex.slash_is_regex(i) {
            i = lex.skip_regex(i);
            continue;
        }
        if b == b'/' {
            i += 1;
            continue;
        }
        // function 关键字边界判定
        if b == b'f'
            && i + 8 <= bytes.len()
            && &bytes[i..i + 8] == b"function"
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
            && !is_ident_byte(*bytes.get(i + 8).unwrap_or(&b' '))
        {
            if let Some(body) = scan_function_body(bytes, i + 8) {
                out.push(body);
                i = body.body_close + 1;
                continue;
            }
        }
        lex.prev = b;
        i += 1;
    }
    out
}

/// 从 `function` 关键字之后扫到函数体 `{...}`，返回区间。
fn scan_function_body(bytes: &[u8], mut i: usize) -> Option<BodyRange> {
    // 跳过函数名（可选）
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    if i < bytes.len() && is_ident_byte(bytes[i]) && bytes[i] != b'(' {
        while i < bytes.len() && is_ident_byte(bytes[i]) {
            i += 1;
        }
    }
    // 形参列表
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'(' {
        return None;
    }
    let mut depth = 1i32;
    i += 1;
    let mut quote = 0u8;
    while i < bytes.len() && depth > 0 {
        let b = bytes[i];
        if quote != 0 {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == quote {
                quote = 0;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' | b'`' => quote = b,
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    // 箭头函数 `=>` 无大括号体或函数体
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    if i + 1 < bytes.len() && bytes[i] == b'=' && bytes[i + 1] == b'>' {
        return None; // 交给外层（箭头函数体按普通表达式处理）
    }
    if i >= bytes.len() || bytes[i] != b'{' {
        return None;
    }
    let body_open = i;
    let mut d = 1i32;
    i += 1;
    let mut q = 0u8;
    while i < bytes.len() && d > 0 {
        let b = bytes[i];
        if q != 0 {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                q = 0;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' => q = b,
            b'`' => {
                // 模板内 `${}` 的 `}` 不应减深度：整段跳过。
                let end = skip_template(bytes, i);
                i = end;
                continue;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
            b'{' => d += 1,
            b'}' => d -= 1,
            _ => {}
        }
        i += 1;
    }
    if d != 0 {
        return None;
    }
    Some(BodyRange {
        body_open,
        body_close: i - 1,
    })
}

/// 定位入口：在已有 boa Context 上对失败脚本做语句级定位。
/// `src` 应为生产路径同条件处理过的源码（已套 for-in 守卫），
/// 返回多层报告行（人已可读），无进展时返回说明行。
pub fn localize(ctx: &mut Context, src: &str, origin: &str) -> Vec<String> {
    let mut report = vec![format!(
        "[JS诊断] 开始定位 {origin}:{} 字节",
        src.len()
    )];
    // 收集 define 陷阱已捕获的原始执行记录(ThreadRuntime 启动时安装,见 runtime.rs)。
    for line in collect_probe_log(ctx) {
        report.push(format!("[JS诊断] 探针: {line}"));
    }
    let stmts = split_top_level(src);
    report.push(format!("[JS诊断] 顶层语句数: {}", stmts.len()));
    if stmts.is_empty() {
        return report;
    }

    // 路径一：普通前缀重放（覆盖顶层即抛错的脚本）
    match scan_prefixes(ctx, &|k| {
        concat_spans(src, &stmts[..k])
    }, stmts.len()) {
        Some(k) => {
            report.push(format!(
                "[JS诊断] 顶层前缀在第 {}/{} 条语句抛错",
                k + 1,
                stmts.len()
            ));
            let prefix = concat_spans(src, &stmts[..k]);
            drill(ctx, &prefix, stmts[k].text(src), 0, &mut report);
            return report;
        }
        None => report.push("[JS诊断] 顶层前缀重放未复现（throw 可能在 define 工厂内）".to_string()),
    }

    // 路径二：AMD 感知——对每个顶层 define 的工厂体直接下钻。
    // 重放内容为**纯工厂体语句**（驱动器自带 (require,exports,module) 形参注入），
    // 不经过任何装载器：站点 requirejs 可能同步执行工厂、内核 AMD 装载器只注册不执行，
    // 两种语义下都能稳定复现工厂体内的 throw。
    for (idx, sp) in stmts.iter().enumerate() {
        let text = sp.text(src);
        let Some(body) = extract_define_factory(text) else {
            continue;
        };
        let inner_src = &text[body.body_open + 1..body.body_close];
        let inner = split_top_level(inner_src);
        if inner.is_empty() {
            continue;
        }
        report.push(format!(
            "[JS诊断] define 语句 #{idx} 工厂体语句数: {}",
            inner.len()
        ));
        let build = |k: usize| concat_spans(inner_src, &inner[..k]);
        if let Some(k) = scan_prefixes(ctx, &build, inner.len()) {
            report.push(format!(
                "[JS诊断] define #{} 工厂体前缀在第 {}/{} 条语句抛错",
                idx,
                k + 1,
                inner.len()
            ));
            drill(ctx, &concat_spans(inner_src, &inner[..k]), inner[k].text(inner_src), 1, &mut report);
            return report;
        }
        report.push(format!("[JS诊断] define 语句 #{idx} 工厂体前缀未抛错，跳过"));
    }
    report.push("[JS诊断] 未能复现 throw：错误可能依赖页面时序/事件回调".to_string());
    report
}

/// 从 define 语句文本提取工厂函数体（define(id, deps?, factory)）。
/// 返回的区间是相对**语句文本**的字节下标。
fn extract_define_factory(stmt: &str) -> Option<BodyRange> {
    let t = stmt.trim_start();
    // rest 空间（去掉前导空白与 define 关键字）→ 语句空间的偏移量。
    let base = stmt.len() - t.len() + "define".len();
    let rest = t.strip_prefix("define")?;
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    // define 后应紧跟 (
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'(' {
        return None;
    }
    // 在 define 参数内找最外层 function 体（工厂通常是末位参数）。
    find_function_bodies(&rest[i..])
        .first()
        .map(|b| BodyRange {
            body_open: base + i + b.body_open,
            body_close: base + i + b.body_close,
        })
}

/// 累积前缀扫描：粗块步进定位粗块，再块内线性精查；跳过 SyntaxError（切分界无效）。
/// `build(k)` 产出前 k 条语句的重放源码。返回首个抛错的分界 k（0-based）。
fn scan_prefixes(ctx: &mut Context, build: &dyn Fn(usize) -> String, n: usize) -> Option<usize> {
    let throws = |ctx: &mut Context, code: &str| -> bool {
        match run_driver(ctx, code) {
            DriverResult::Throw(_) => true,
            _ => false,
        }
    };
    let mut prev_ok = 0usize; // 已知不抛的最大前缀
    let mut k = CHUNK.min(n);
    loop {
        if throws(ctx, &build(k)) {
            // 块内线性精查 (prev_ok, k]
            for j in (prev_ok + 1)..=k {
                if throws(ctx, &build(j)) {
                    return Some(j - 1);
                }
            }
            return Some(k - 1);
        }
        if k >= n {
            return None;
        }
        prev_ok = k;
        k = (k + CHUNK).min(n);
    }
}

enum DriverResult {
    Ok,
    Throw(String),
    DriverError(String),
}

/// 驱动器执行：候选代码以**直接 eval** 包进 `(require,exports,module)` 工厂执行
/// （eval 在局部作用域求值，工厂形参对代码可见，与 AMD 工厂语义一致；
/// 间接 eval 会丢形参），返回 ok / throw:<msg> / driver_error:<msg>（SyntaxError 归 driver_error）。
fn run_driver(ctx: &mut Context, body: &str) -> DriverResult {
    let payload = serde_json::to_string(body).unwrap_or_else(|_| "\"\"".into());
    let src = format!(
        "(function(){{try{{var R=(function(require,exports,module){{eval({payload});return \"ok\";}})((typeof require==='function')?require:function(){{return {{}};}},{{}},{{exports:{{}}}});return R;}}catch(e){{return \"throw:\"+String((e&&e.stack)||e);}}}})()"
    );
    let v = ctx.eval(Source::from_bytes(src.as_bytes()));
    match v {
        Ok(JsValue::String(s)) => {
            let text = s.to_std_string_escaped();
            if let Some(msg) = text.strip_prefix("throw:") {
                // 载荷内的 SyntaxError = 切分边界切坏（截断处不成语法），归 driver_error 跳过；
                // 若是驱动器自身语法错,ctx.eval 早已走 Err 分支,不会到这里。
                if msg.starts_with("SyntaxError") {
                    DriverResult::DriverError(msg.to_string())
                } else {
                    DriverResult::Throw(msg.to_string())
                }
            } else if text == "ok" {
                DriverResult::Ok
            } else {
                DriverResult::DriverError(text)
            }
        }
        Ok(_) => DriverResult::DriverError("驱动器返回非字符串".to_string()),
        Err(e) => {
            let msg = format!("{e}");
            DriverResult::DriverError(msg)
        }
    }
}

/// 逐层下钻：在 prefix 之后对 stmt 做函数体截断重放，直到语句小于阈值。
fn drill(ctx: &mut Context, prefix: &str, stmt: &str, level: usize, report: &mut Vec<String>) {
    let trimmed = stmt.trim();
    report.push(format!(
        "[JS诊断] L{level} culprit 语句 ({} 字符): {}",
        trimmed.chars().count(),
        clip(trimmed, 600)
    ));
    if trimmed.len() <= REPORT_THRESHOLD {
        return;
    }
    let bodies = find_function_bodies(stmt);
    if bodies.is_empty() {
        report.push("[JS诊断] 语句内无可下钻函数体，停止".to_string());
        return;
    }
    let dbg = std::env::var("LAEW_DIAG_DEBUG").is_ok();
    // 从大到小尝试各函数体
    let mut bodies_sorted = bodies.clone();
    bodies_sorted.sort_by_key(|b| b.body_open);
    for body in bodies_sorted {
        // 体深 = 该函数体内容的**词法感知**最终 {} 深度（均衡体为 0），用于配平截空。
        let body_depth = balance_depth(&stmt[body.body_open + 1..body.body_close]).max(0) as usize;
        // 配平后截空:追加与体深相等的 `}`,保持外层结构完整,
        // 避免"截掉大括弧体让整段失衡→语法错→误跳过"。
        let mut repair_src = String::with_capacity(stmt.len() + 16);
        repair_src.push_str(&stmt[..body.body_open + 1]);
        repair_src.push_str(&"}".repeat(body_depth));
        repair_src.push_str(&stmt[body.body_close..]);
        let full_prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix};\n")
        };
        match run_driver(ctx, &format!("{full_prefix}{repair_src}")) {
            DriverResult::Throw(m) => {
                if dbg {
                    report.push(format!("[JS诊断:dbg] 配平截空 @[{},{}] 仍抛: {}", body.body_open, body.body_close, clip(&m, 120)));
                }
                continue; // 截掉此体仍抛 → throw 不在此体内
            }
            DriverResult::DriverError(e) => {
                if dbg {
                    report.push(format!("[JS诊断:dbg] 配平截空 @[{},{}] 语法错: {}", body.body_open, body.body_close, clip(&e, 80)));
                }
                continue;
            }
            DriverResult::Ok => {
                if dbg {
                    report.push(format!("[JS诊断:dbg] 配平截空 @[{},{}] 不抛 → 二分其语句", body.body_open, body.body_close));
                }
            }
        }
        // 截空后不抛、完整应抛 → throw 在此体内：二分其语句
        let inner_src = &stmt[body.body_open + 1..body.body_close];
        let inner = split_top_level(inner_src);
        if inner.is_empty() {
            continue;
        }
        let build = |k: usize| {
            let mut s = String::with_capacity(prefix.len() + stmt.len());
            s.push_str(&full_prefix);
            s.push_str(&stmt[..body.body_open + 1]);
            s.push_str(&concat_spans(inner_src, &inner[..k]));
            s.push_str(&stmt[body.body_close..]);
            s
        };
        if let Some(j) = scan_prefixes(ctx, &build, inner.len()) {
            let mut drill_prefix = String::new();
            drill_prefix.push_str(&full_prefix);
            drill_prefix.push_str(&stmt[..body.body_open + 1]);
            drill_prefix.push_str(&concat_spans(inner_src, &inner[..j]));
            drill_prefix.push_str(&stmt[body.body_close..]);
            report.push(format!(
                "[JS诊断] L{level} → 函数体语句 #{}/{} 抛错，继续下钻",
                j + 1,
                inner.len()
            ));
            let stmt_text = inner[j].text(inner_src).to_string();
            drill(ctx, &drill_prefix, &stmt_text, level + 1, report);
            return;
        }
    }
    // 回退：throw 在所有函数体之外的 span 代码里（或体扫描漏检）。
    // 单语句 span（切分器被某构造卡死时多语句会合并成一段）做不了语句级前缀，
    // 改用**强制配平截断二分**：在行粒度切点截断，追加 k 个 `}` 修复平衡后重放；
    // 首个修复后仍抛的切点即 culprit 区域（容忍 ±数行）。
    let units = split_top_level(stmt);
    if units.len() > 1 {
        let build = |k: usize| {
            let mut s = String::with_capacity(prefix.len() + stmt.len());
            s.push_str(prefix);
            if !prefix.is_empty() {
                s.push_str(";\n");
            }
            s.push_str(&concat_spans(stmt, &units[..k]));
            s
        };
        if let Some(j) = scan_prefixes(ctx, &build, units.len()) {
            report.push(format!(
                "[JS诊断] L{level} 语句级前缀在第 {}/{} 条抛错，取该段",
                j + 1,
                units.len()
            ));
            let stmt_text = units[j].text(stmt).to_string();
            drill(ctx, &build(j), &stmt_text, level + 1, report);
            return;
        }
    }
    match locate_by_repair(ctx, prefix, stmt, level, report) {
        true => return,
        false => report.push("[JS诊断] 无函数体满足且强制配平截断未复现，停止".to_string()),
    }
}

/// 强制配平截断二分：行粒度切点 + 追加 k 个 `}` 修复平衡，首个修复后抛错的切点即 culprit 区域。
/// 返回 true 表示已定位并报告。
fn locate_by_repair(ctx: &mut Context, prefix: &str, stmt: &str, level: usize, report: &mut Vec<String>) -> bool {
    // 候选切点：每个换行处（行粒度，足够人读源码定位）。
    let mut cuts: Vec<usize> = stmt
        .char_indices()
        .filter(|(_, c)| *c == '\n')
        .map(|(i, _)| i + 1)
        .collect();
    cuts.push(stmt.len());
    let base = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix};\n")
    };
    // test(P) -> Some(true)=修复后抛错, Some(false)=修复后不抛, None=无法修复
    let test = |ctx: &mut Context, p: usize| -> Option<bool> {
        for k in 0..=12u8 {
            let repair = "}".repeat(k as usize);
            let code = format!("{}{}{}", base, &stmt[..p], repair);
            match run_driver(ctx, &code) {
                DriverResult::Ok => return Some(false),
                DriverResult::Throw(_) => return Some(true),
                DriverResult::DriverError(_) => continue,
            }
        }
        None
    };
    // 粗扫（块步进）+ 精扫（线性），容忍 None（切点落在字符串/正则内无法修复）。
    let n = cuts.len();
    let chunk = 64usize;
    let mut prev_ok = 0usize;
    let mut k = chunk.min(n);
    let mut found = None;
    loop {
        match test(ctx, cuts[k - 1]) {
            Some(true) => {
                for j in (prev_ok + 1)..=k {
                    if matches!(test(ctx, cuts[j - 1]), Some(true)) {
                        found = Some(j - 1);
                        break;
                    }
                }
                if found.is_none() {
                    found = Some(k - 1);
                }
                break;
            }
            _ => {}
        }
        if k >= n {
            break;
        }
        prev_ok = k;
        k = (k + chunk).min(n);
    }
    let Some(idx) = found else { return false };
    let cut = cuts[idx];
    // 报告切点所在行 ±3 行上下文。
    let line_start = stmt[..cut].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let prev_nl = stmt[..line_start.saturating_sub(1)].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let ctx_end = (cut..stmt.len())
        .find(|&i| stmt[i..].starts_with('\n'))
        .map(|i| i + 1)
        .unwrap_or(stmt.len());
    let tail_end = (0..6)
        .fold(ctx_end, |acc, _| {
            stmt[acc..].find('\n').map(|p| acc + p + 1).unwrap_or(stmt.len())
        });
    report.push(format!(
        "[JS诊断] L{level} 强制配平截断定位: 切点行起始 {} / 语句长 {}，上下文:",
        line_start,
        stmt.len()
    ));
    report.push(format!("[JS诊断] L{level} context:\n{}", clip(&stmt[prev_nl..tail_end.min(stmt.len())], 1200)));
    true
}

fn concat_spans(src: &str, spans: &[Span]) -> String {
    let mut out = String::with_capacity(spans.iter().map(|s| s.end - s.start + 1).sum());
    for sp in spans {
        out.push_str(sp.text(src));
        out.push('\n');
    }
    out
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests;

/// 读取探针捕获日志并清理。
fn collect_probe_log(ctx: &mut Context) -> Vec<String> {
    let src = r#"(function(){
  var G = globalThis;
  var log = G.__lsmProbeLog || [];
  G.__lsmProbeLog = [];
  return JSON.stringify(log);
})()"#;
    let v = ctx.eval(Source::from_bytes(src.as_bytes()));
    match v {
        Ok(v) => {
            let t = v
                .to_string(ctx)
                .map(|x| x.to_std_string_escaped())
                .unwrap_or_default();
            serde_json::from_str::<Vec<String>>(&t).unwrap_or_default()
        }
        Err(_) => Vec::new(),
    }
}

/// define 陷阱(仅 LAEW_JS_DIAG=1 时装入 JS 线程):用访问器拦截全局 define 赋值,
/// 任何装载器(站点 _jsload/FIS/mod.js 或内核 AMD)安装 define 时都被包裹,
/// 工厂被调用时记录实参形态与错误到 `__lsmProbeLog`(环形 50 条)。
pub const DEFINE_TRAP_JS: &str = r##"
(function () {
  var G = globalThis;
  if (G.__lsmDefineTrap) return;
  G.__lsmDefineTrap = true;
  G.__lsmProbeLog = [];
  function rec(line) {
    if (G.__lsmProbeLog.length < 50) G.__lsmProbeLog.push(line);
  }
  function wrapDefine(fn) {
    if (typeof fn !== "function" || fn.__lsmWrapped) return fn;
    function probe(id, deps, factory) {
      if (typeof id === "string" && typeof factory === "function") {
        var flen = -1;
        try { flen = factory.length; } catch (e) {}
        try { rec("define 调用 id=" + id + " deps=" + (deps == null ? "null" : deps.length) + " flen=" + flen); } catch (e) {}
        var wrapped = function () {
          var types = [];
          for (var i = 0; i < arguments.length; i++) {
            var t = typeof arguments[i];
            if (t === "object" && arguments[i] === null) t = "null";
            types.push(t);
          }
          try {
            rec("调用 " + id + " argc=" + arguments.length + " types=" + types.join(",") + " flen=" + flen);
            return factory.apply(this, arguments);
          } catch (e) {
            rec("工厂抛错 " + id + ": " + String((e && e.stack) || e) +
              " | argc=" + arguments.length + " types=" + types.join(",") + " flen=" + flen);
            throw e;
          }
        };
        return fn.call(this, id, deps, wrapped);
      }
      return fn.apply(this, arguments);
    }
    probe.amd = fn.amd;
    probe.__lsmWrapped = true;
    return probe;
  }
  try {
    // 先收留当前 define(PRELUDE AMD 装载器),避免访问器把已有实现藏成 undefined。
    var cur = G.define;
    G.__lsmDefineCurrent = wrapDefine(cur);
    Object.defineProperty(G, "define", {
      configurable: true,
      get: function () { return G.__lsmDefineCurrent; },
      set: function (v) {
        rec("define 被赋值: type=" + typeof v + " amd=" + (v && v.amd ? "y" : "n"));
        G.__lsmDefineCurrent = wrapDefine(v);
      }
    });
    rec("define 陷阱已安装 define=" + typeof G.define + " require=" + typeof G.require + " _jsload=" + typeof G._jsload);
  } catch (e) {
    rec("define 陷阱安装失败: " + e);
  }
})();
"##;
