//! diag.rs 测试：tokenizer 拆分正确性 + 定位器端到端（真实 boa Context）。

use super::*;
use boa_engine::Source;
use boa_engine::Context;
// Boa 0.21: Token 的 span 由 `boa_ast::Spanned` trait 提供。
use boa_engine::ast::Spanned;

#[test]
fn 顶层拆分_基本分号() {
    let src = "var a = 1; var b = 2;\nfoo();";
    let spans = split_top_level(src);
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].text(src), "var a = 1;");
    assert_eq!(spans[1].text(src), "var b = 2;");
    assert_eq!(spans[2].text(src), "foo();");
}

#[test]
fn 顶层拆分_字符串与注释中的分号不切() {
    let src = "var s = 'a;b'; // c;d\nvar t = \"e;f\";\n/* g;h */ var u = 1;";
    let spans = split_top_level(src);
    // 注释归属下一段开头，不影响切分点正确性
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].text(src), "var s = 'a;b';");
    assert!(spans[1].text(src).contains("var t = \"e;f\";"));
    assert_eq!(spans[2].text(src), "var u = 1;");
}

#[test]
fn 顶层拆分_模板字符串与嵌套括号() {
    let src = "var a = `x;${foo(';')};y`; if (a) { f(1); g(2); }\nvar b = 3;";
    let spans = split_top_level(src);
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].text(src), "var a = `x;${foo(';')};y`;");
    assert_eq!(spans[1].text(src), "if (a) { f(1); g(2); }");
    assert_eq!(spans[2].text(src), "var b = 3;");
}

#[test]
fn 顶层拆分_正则与除法歧义() {
    // `/[;}]/` 是正则（含括号类字符），不能影响后续切分；`a / b;` 是除法。
    let src = "var r = /[;}]+/g; var q = a / b; var z = 1;";
    let spans = split_top_level(src);
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].text(src), "var r = /[;}]+/g;");
    assert_eq!(spans[1].text(src), "var q = a / b;");
}

#[test]
fn 顶层拆分_行注释与空白不产生空语句() {
    let src = "  // hello\n  \n var a = 1;\n\n";
    let spans = split_top_level(src);
    assert_eq!(spans.len(), 1);
}

#[test]
fn 函数体扫描_嵌套与字符串不误伤() {
    let src = r#"define('x', function(require, exports, module){
        var s = "function not() { real(); }";
        function inner() { return 1; }
        (function(){ deep(); });
    });"#;
    let bodies = find_function_bodies(src);
    // 只返回最外层函数体（下钻由 drill 递归负责）；字符串内的 function 不算
    assert_eq!(bodies.len(), 1, "实际 {}", bodies.len());
    // 外层体截空重拼后语法仍完整
    let b = bodies[0];
    let rebuilt = format!("{}{}", &src[..b.body_open + 1], &src[b.body_close..]);
    assert!(rebuilt.trim_end().ends_with("});"), "{}", rebuilt);
}

#[test]
fn 定位器_顶层直抛语句() {
    let mut ctx = Context::default();
    // 前四条正常，第四条对 undefined 做属性访问（Boa to_object 抛错）
    let src = "var a = 1;\nvar b = a + 1;\nvar u;\nu.foo;\nvar c = 3;";
    let report = localize(&mut ctx, src, "(test)");
    let text = report.join("\n");
    assert!(text.contains("顶层前缀在第 4/5"), "{}", text);
    assert!(text.contains("u.foo"), "{}", text);
}

#[test]
fn 定位器_define工厂体下钻() {
    let mut ctx = Context::default();
    // define 桩：路径一（顶层前缀重放）需要全局 define 存在才不误报 ReferenceError。
    ctx.eval(Source::from_bytes(
        b"function define(){}; define.amd={}; function require(){return {}};",
    ))
    .expect("桩注入");
    // define 注册不抛错，工厂体内部抛错 → 路径二应直达工厂内 culprit 语句
    let src = "define('m1', function(require, exports, module){\n\
                 var ok = 1;\n\
                 var dep = require('whatever');\n\
                 var u;\n\
                 u.prop.foo;\n\
                 var tail = 2;\n\
               });\n\
               define('m2', function(require, exports, module){ var fine = 1; });";
    let report = localize(&mut ctx, src, "(test)");
    let text = report.join("\n");
    assert!(text.contains("工厂体前缀在第 4/5"), "{}", text);
    assert!(text.contains("u.prop.foo"), "{}", text);
}

#[test]
fn 定位器_无抛错时如实报告() {
    let mut ctx = Context::default();
    let src = "var a = 1; var b = 2;";
    let report = localize(&mut ctx, src, "(test)");
    let text = report.join("\n");
    assert!(text.contains("未能复现"), "{}", text);
}

/// boa 词法器探针：对真实大文件做顶层语句切分真值，验证 regex/division 消歧。
/// 用法：`LAEW_DIAG_LIB=/path/to/libs.js cargo test --lib boa词法器 -- --nocapture --ignored`
#[test]
#[ignore]
fn boa词法器顶层切分真值() {
    use boa_engine::ast::Punctuator;
    use boa_parser::lexer::{Lexer, TokenKind};
    use boa_parser::source::ReadChar;

    /// 按字符的 ReadChar 实现（UTF8Input::new 是 pub(crate)，测试侧自建）。
    struct CharsInput<'a> {
        chars: std::str::Chars<'a>,
    }
    impl ReadChar for CharsInput<'_> {
        fn next_char(&mut self) -> std::io::Result<Option<u32>> {
            Ok(self.chars.next().map(u32::from))
        }
    }
    let Ok(path) = std::env::var("LAEW_DIAG_LIB") else {
        eprintln!("skip: 未设置 LAEW_DIAG_LIB");
        return;
    };
    let src = std::fs::read_to_string(&path).expect("读取文件");
    // 行首字节偏移索引（Position 为 1-based 行列）。
    let mut line_start = vec![0usize];
    for (idx, b) in src.bytes().enumerate() {
        if b == b'\n' {
            line_start.push(idx + 1);
        }
    }
    let pos_to_offset = |line: usize, col: usize| -> usize {
        let ls = line_start.get(line.saturating_sub(1)).copied().unwrap_or(0);
        // 列按字符计（非字节），在行内逐字符推进。
        let line_end = src[ls..].find('\n').map(|p| ls + p).unwrap_or(src.len());
        src[ls..line_end]
            .char_indices()
            .nth(col.saturating_sub(1))
            .map(|(i, _)| ls + i)
            .unwrap_or(line_end)
    };
    let mut lex = Lexer::new(CharsInput { chars: src.chars() });
    let mut interner = boa_interner::Interner::new();
    let mut depth = 0i32;
    let mut boundaries = Vec::new();
    let mut final_depth_events = Vec::new();
    loop {
        let tok = lex.next(&mut interner).expect("词法错误");
        let Some(tok) = tok else { break };
        let span = tok.span();
        let end_off = pos_to_offset(span.end().line_number() as usize, span.end().column_number() as usize);
        match tok.kind() {
            TokenKind::Punctuator(Punctuator::OpenParen | Punctuator::OpenBracket | Punctuator::OpenBlock) => depth += 1,
            TokenKind::Punctuator(Punctuator::CloseParen | Punctuator::CloseBracket | Punctuator::CloseBlock) => {
                depth -= 1;
                if depth < 0 {
                    final_depth_events.push(format!("NEG @{end_off} {:?}", &src[end_off.saturating_sub(50)..(end_off + 15).min(src.len())]));
                    depth = 0;
                }
            }
            TokenKind::Punctuator(Punctuator::Semicolon) if depth == 0 => boundaries.push(end_off),
            _ => {}
        }
    }
    eprintln!("顶层分号切分点: {} 个, 最终 depth {}", boundaries.len(), depth);
    let mut prev = 0usize;
    for (i, b) in boundaries.iter().enumerate() {
        eprintln!("stmt#{i} [{}..{}] len={} tail={:?}", prev, b, b - prev, clip(&src[b.saturating_sub(30)..*b], 30));
        prev = *b;
    }
    eprintln!("尾部残余 [{}..{}] len={}", prev, src.len(), src.len() - prev);
    for e in &final_depth_events {
        eprintln!("{e}");
    }
}

/// 增量解析探针：对行累积前缀跑 boa parse_script，首个解析失败的行即含卡死切分的构造。
/// 用法：`LAEW_DIAG_LIB=/path/to/libs.js cargo test --lib 增量解析 -- --nocapture --ignored`
#[test]
#[ignore]
fn 增量解析定位歧义构造() {
    let Ok(path) = std::env::var("LAEW_DIAG_LIB") else {
        eprintln!("skip: 未设置 LAEW_DIAG_LIB");
        return;
    };
    let src = std::fs::read_to_string(&path).expect("读取文件");
    let line_starts: Vec<usize> = {
        let mut v = vec![0usize];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                v.push(i + 1);
            }
        }
        v
    };
    let parses = |prefix: &str| -> bool {
        let mut interner = boa_interner::Interner::new();
        let scope = boa_engine::ast::scope::Scope::default();
        let res = boa_parser::Parser::new(boa_parser::Source::from_bytes(prefix))
            .parse_script(&scope, &mut interner);
        res.is_ok()
    };
    let n = line_starts.len();
    // 找首个解析失败的行（二进制搜索：解析失败是单调的——歧义构造之后全部失败）。
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let end = line_starts.get(mid + 1).copied().unwrap_or(src.len());
        if parses(&src[..end]) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    eprintln!("首个解析失败行: {} (0-based), 前一行解析 OK = {}", lo, lo > 0);
    let ls = line_starts[lo];
    let le = line_starts.get(lo + 1).copied().unwrap_or(src.len());
    eprintln!("该行内容: {:?}", &src[ls..le]);
    let ctxs = ls.saturating_sub(200);
    eprintln!("上下文: {:?}", &src[ctxs..(le + 100).min(src.len())]);
}
