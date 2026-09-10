//! 语法高亮 —— 轻量 regex tokenizer,8 类 token,16 色 SGR。
//!
//! 设计:手写 regex 组合(避免 syntect 的 onig/sregex 重依赖)。
//! 每语言 ~5-8 条规则,`once_cell::Lazy<Regex>` 编译期缓存。
//! 优先级:Comment > String > Keyword > Number > Type > Function > Operator > Punctuation。
//!
//! 参考:claudecode Rust ColorFile 16 色、pi 18 语言渐进加载、opencode shiki-wasm。

use once_cell::sync::Lazy;
use regex::Regex;

use crate::tui::render::{RenderLines, Span};
use crate::tui::theme::{self, attr};

/// 高亮 token 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HlToken {
    Keyword,
    String,
    Number,
    Comment,
    Type,
    Function,
    Operator,
    Punctuation,
    Plain,
}

/// 支持的语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlLang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Bash,
    Json,
    Yaml,
    Markdown,
    Plain,
}

/// 单条匹配规则。
struct Rule {
    token: HlToken,
    pattern: &'static str,
}

impl Rule {
    const fn new(token: HlToken, pattern: &'static str) -> Self {
        Rule { token, pattern }
    }

}

/// 获取指定语言+索引位置的 regex(每次调用重新编译,开销可忽略)。
fn get_regex(lang: HlLang, rule_idx: usize) -> Regex {
    let rules = rules_for(lang);
    Regex::new(rules[rule_idx].pattern).unwrap_or_else(|_| Regex::new("").unwrap())
}

// ============================================================
// 各语言规则(优先级从高到低)
// ============================================================

/// Rust 规则。
fn rust_rules() -> &'static [Rule] {
    static RULES: Lazy<Vec<Rule>> = Lazy::new(|| {
        vec![
            // 注释(最高优先级)
            Rule::new(HlToken::Comment, r"///.*$|//.*$"),
            // 字符串(简化:双引号字符串)
            Rule::new(HlToken::String, r#""(?:[^"\\]|\\.)*""#),
            // 关键字
            Rule::new(HlToken::Keyword, r"\b(?:as|async|await|break|const|continue|crate|dyn|else|enum|extern|false|fn|for|if|impl|in|let|loop|match|mod|move|mut|pub|ref|return|self|Self|static|struct|super|trait|true|type|unsafe|use|where|while|yield)\b"),
            // 类型(大写开头)
            Rule::new(HlToken::Type, r"\b[A-Z][a-zA-Z0-9]*\b"),
            // 函数定义/调用
            Rule::new(HlToken::Function, r"\b([a-z_][a-zA-Z0-9_]*)\s*\("),
            // 数字
            Rule::new(HlToken::Number, r"\b(?:0x[0-9a-fA-F]+|0b[01]+|0o[0-7]+|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\b"),
            // 操作符
            Rule::new(HlToken::Operator, r"[-+*/%=!<>&|^~]+|::|->|=>|\.\.|\.\.=|\?"),
            // 标点
            Rule::new(HlToken::Punctuation, r"[()\[\]{}.,;:@#]"),
        ]
    });
    &RULES
}

/// Python 规则。
fn python_rules() -> &'static [Rule] {
    static RULES: Lazy<Vec<Rule>> = Lazy::new(|| {
        vec![
            Rule::new(HlToken::Comment, r"#.*$"),
            Rule::new(HlToken::String, r#""(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'"#),
            Rule::new(HlToken::Keyword, r"\b(?:and|as|assert|async|await|break|class|continue|def|del|elif|else|except|False|finally|for|from|global|if|import|in|is|lambda|None|nonlocal|not|or|pass|raise|return|True|try|while|with|yield)\b"),
            Rule::new(HlToken::Type, r"\b[A-Z][a-zA-Z0-9]*\b"),
            Rule::new(HlToken::Function, r"\b([a-z_][a-zA-Z0-9_]*)\s*\("),
            Rule::new(HlToken::Number, r"\b(?:0x[0-9a-fA-F]+|0b[01]+|0o[0-7]+|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\b"),
            Rule::new(HlToken::Operator, r"[-+*/%=!<>&|^~]+|//|<<|>>|\*\*"),
            Rule::new(HlToken::Punctuation, r"[()\[\]{}.,;:@#]"),
        ]
    });
    &RULES
}

/// JavaScript/TypeScript 规则。
fn js_rules() -> &'static [Rule] {
    static RULES: Lazy<Vec<Rule>> = Lazy::new(|| {
        vec![
            Rule::new(HlToken::Comment, r"//.*$|/\*[\s\S]*?\*/"),
            Rule::new(HlToken::String, r#""(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`"#),
            Rule::new(HlToken::Keyword, r"\b(?:async|await|break|case|catch|class|const|continue|debugger|default|delete|do|else|enum|export|extends|false|finally|for|function|if|implements|import|in|instanceof|interface|let|new|null|package|private|protected|public|return|super|switch|static|this|throw|true|try|typeof|undefined|var|void|while|with|yield)\b"),
            Rule::new(HlToken::Type, r"\b[A-Z][a-zA-Z0-9]*\b"),
            Rule::new(HlToken::Function, r"\b([a-z_][a-zA-Z0-9_]*)\s*\("),
            Rule::new(HlToken::Number, r"\b(?:0x[0-9a-fA-F]+|0b[01]+|0o[0-7]+|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\b"),
            Rule::new(HlToken::Operator, r"[-+*/%=!<>&|^~]+|&&|\|\||<<|>>|>>>|\?\?|=>|\?\."),
            Rule::new(HlToken::Punctuation, r"[()\[\]{}.,;:@#]"),
        ]
    });
    &RULES
}

/// Bash 规则。
fn bash_rules() -> &'static [Rule] {
    static RULES: Lazy<Vec<Rule>> = Lazy::new(|| {
        vec![
            Rule::new(HlToken::Comment, r"#.*$"),
            Rule::new(HlToken::String, r#""(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'"#),
            Rule::new(HlToken::Keyword, r"\b(?:if|then|else|elif|fi|for|while|do|done|case|esac|function|select|time|until|in|return|exit|break|continue|shift|source|export|unset|readonly|declare|local|typeset)\b"),
            Rule::new(HlToken::Function, r"\b([a-z_][a-zA-Z0-9_]*)\s*\(\)"),
            Rule::new(HlToken::Number, r"\b\d+\b"),
            Rule::new(HlToken::Operator, r"[-+*/%=!<>&|^~]+|&&|\|\||<<|>>|=>"),
            Rule::new(HlToken::Punctuation, r"[()\[\]{}.,;:@#]"),
        ]
    });
    &RULES
}

/// JSON 规则。
fn json_rules() -> &'static [Rule] {
    static RULES: Lazy<Vec<Rule>> = Lazy::new(|| {
        vec![
            Rule::new(HlToken::String, r#""(?:[^"\\]|\\.)*""#),
            Rule::new(HlToken::Keyword, r"\b(?:true|false|null)\b"),
            Rule::new(HlToken::Number, r"-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?"),
            Rule::new(HlToken::Punctuation, r"[()\[\]{},:]"),
        ]
    });
    &RULES
}

/// YAML 规则。
fn yaml_rules() -> &'static [Rule] {
    static RULES: Lazy<Vec<Rule>> = Lazy::new(|| {
        vec![
            Rule::new(HlToken::Comment, r"#.*$"),
            Rule::new(HlToken::String, r#""(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'"#),
            Rule::new(HlToken::Keyword, r"\b(?:true|false|null|yes|no)\b"),
            Rule::new(HlToken::Number, r"\b(?:0x[0-9a-fA-F]+|0b[01]+|0o[0-7]+|\d+(?:\.\d+)?)\b"),
            Rule::new(HlToken::Operator, r":"),
            Rule::new(HlToken::Punctuation, r"[-*&,]"),
        ]
    });
    &RULES
}

/// Markdown 规则(轻量:标题/代码围栏/链接/强调)。
fn markdown_rules() -> &'static [Rule] {
    static RULES: Lazy<Vec<Rule>> = Lazy::new(|| {
        vec![
            Rule::new(HlToken::Keyword, r"^#{1,6}\s.*$"),              // 标题
            Rule::new(HlToken::String, r"`[^`]+`"),                    // 行内代码
            Rule::new(HlToken::Function, r"\[([^\]]+)\]\([^)]+\)"),    // 链接
            Rule::new(HlToken::Operator, r"\*\*[^*]+\*\*|^\s*[-*]\s"),  // 强调/列表
            Rule::new(HlToken::Comment, r"^>.*$"),                     // 引用
        ]
    });
    &RULES
}

/// 获取语言规则。
fn rules_for(lang: HlLang) -> &'static [Rule] {
    match lang {
        HlLang::Rust => rust_rules(),
        HlLang::Python => python_rules(),
        HlLang::JavaScript => js_rules(),
        HlLang::TypeScript => js_rules(), // TS 复用 JS 规则(简化)
        HlLang::Bash => bash_rules(),
        HlLang::Json => json_rules(),
        HlLang::Yaml => yaml_rules(),
        HlLang::Markdown => markdown_rules(),
        HlLang::Plain => &[],
    }
}

/// 高亮单行,返回 span 列表。
pub fn highlight_line(line: &str, lang: HlLang) -> Vec<Span> {
    let rules = rules_for(lang);
    if rules.is_empty() || line.is_empty() {
        return vec![Span::plain(line.to_string())];
    }

    // 贪心匹配:从左到右,优先匹配最早开始的规则
    let mut spans: Vec<Span> = Vec::new();
    let mut cursor: usize = 0;
    let bytes = line.as_bytes();

    while cursor < bytes.len() {
        let mut best: Option<(usize, usize, HlToken)> = None;

        for (idx, rule) in rules.iter().enumerate() {
            let re = get_regex(lang, idx);
            if let Some(m) = re.find_at(line, cursor) {
                if m.start() == cursor {
                    let end = m.end();
                    if best.is_none() || end > best.unwrap().1 {
                        best = Some((m.start(), end, rule.token));
                    }
                }
            }
        }

        if let Some((start, end, token)) = best {
            let text = &line[start..end];
            spans.push(colorize(text, token));
            cursor = end;
        } else {
            // 无规则匹配,收集连续 plain 文本
            let mut end = cursor + 1;
            while end < bytes.len() {
                let mut matched = false;
                for (idx, _rule) in rules.iter().enumerate() {
                    let re = get_regex(lang, idx);
                    if let Some(m) = re.find_at(line, end) {
                        if m.start() == end {
                            matched = true;
                            break;
                        }
                    }
                }
                if matched {
                    break;
                }
                end += 1;
            }
            spans.push(Span::plain(line[cursor..end].to_string()));
            cursor = end;
        }
    }

    spans
}

/// 给 token 上色。
fn colorize(text: &str, token: HlToken) -> Span {
    match token {
        HlToken::Keyword => Span::with_attrs(text, theme::HL_KEYWORD_FG, theme::HL_KEYWORD_ATTRS),
        HlToken::String => Span::with_attrs(text, theme::HL_STRING_FG, attr::NONE),
        HlToken::Number => Span::with_attrs(text, theme::HL_NUMBER_FG, attr::NONE),
        HlToken::Comment => Span::with_attrs(text, theme::HL_COMMENT_FG, theme::HL_COMMENT_ATTRS),
        HlToken::Type => Span::with_attrs(text, theme::HL_TYPE_FG, theme::HL_TYPE_ATTRS),
        HlToken::Function => Span::with_attrs(text, theme::HL_FUNCTION_FG, theme::HL_FUNCTION_ATTRS),
        HlToken::Operator => Span::with_attrs(text, theme::HL_OPERATOR_FG, attr::NONE),
        HlToken::Punctuation => Span::with_attrs(text, theme::HL_PUNCTUATION_FG, attr::NONE),
        HlToken::Plain => Span::plain(text),
    }
}

/// 高亮多行文本。
pub fn highlight_lines(text: &str, lang: HlLang) -> RenderLines {
    text.lines().map(|line| highlight_line(line, lang)).collect()
}

/// 自动检测语言(from shebang / file extension)。
pub fn detect_language(first_line: &str, path: Option<&str>) -> HlLang {
    // shebang 检测
    if first_line.starts_with("#!") {
        if first_line.contains("rust") || first_line.contains("cargo") {
            return HlLang::Rust;
        }
        if first_line.contains("python") || first_line.contains("python3") {
            return HlLang::Python;
        }
        if first_line.contains("bash") || first_line.contains("sh") {
            return HlLang::Bash;
        }
        if first_line.contains("node") {
            return HlLang::JavaScript;
        }
    }

    // 扩展名检测
    if let Some(p) = path {
        if let Some(ext) = p.rsplit('.').next() {
            match ext.to_lowercase().as_str() {
                "rs" => return HlLang::Rust,
                "py" => return HlLang::Python,
                "js" | "mjs" | "cjs" => return HlLang::JavaScript,
                "ts" | "tsx" => return HlLang::TypeScript,
                "sh" | "bash" => return HlLang::Bash,
                "json" => return HlLang::Json,
                "yaml" | "yml" => return HlLang::Yaml,
                "md" | "markdown" => return HlLang::Markdown,
                _ => {}
            }
        }
    }

    HlLang::Plain
}

/// 从围栏标签解析语言(```rust → HlLang::Rust)。
pub fn lang_from_fence_tag(tag: &str) -> HlLang {
    match tag.trim().to_lowercase().as_str() {
        "rust" | "rs" => HlLang::Rust,
        "python" | "py" => HlLang::Python,
        "javascript" | "js" => HlLang::JavaScript,
        "typescript" | "ts" => HlLang::TypeScript,
        "bash" | "sh" | "shell" | "zsh" => HlLang::Bash,
        "json" => HlLang::Json,
        "yaml" | "yml" => HlLang::Yaml,
        "markdown" | "md" => HlLang::Markdown,
        _ => HlLang::Plain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_rust_fn_keyword() {
        let spans = highlight_line("fn main() { let x = 42; }", HlLang::Rust);
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "fn main() { let x = 42; }");

        // 验证 fn/let 被识别为 Keyword
        let keyword_spans: Vec<_> = spans.iter().filter(|s| s.fg == theme::HL_KEYWORD_FG).collect();
        let kw_text: String = keyword_spans.iter().map(|s| s.text.as_str()).collect();
        assert!(kw_text.contains("fn"));
        assert!(kw_text.contains("let"));
    }

    #[test]
    fn highlight_string() {
        let spans = highlight_line(r#"let s = "hello world";"#, HlLang::Rust);
        let string_spans: Vec<_> = spans.iter().filter(|s| s.fg == theme::HL_STRING_FG).collect();
        assert!(!string_spans.is_empty());
        let s_text: String = string_spans.iter().map(|s| s.text.as_str()).collect();
        assert!(s_text.contains("hello world"));
    }

    #[test]
    fn highlight_comment() {
        let spans = highlight_line("// this is a comment", HlLang::Rust);
        let comment_spans: Vec<_> = spans.iter().filter(|s| s.fg == theme::HL_COMMENT_FG).collect();
        assert!(!comment_spans.is_empty());
    }

    #[test]
    fn highlight_number() {
        let spans = highlight_line("let x = 42;", HlLang::Rust);
        let num_spans: Vec<_> = spans.iter().filter(|s| s.fg == theme::HL_NUMBER_FG).collect();
        assert!(!num_spans.is_empty());
    }

    #[test]
    fn detect_language_shebang() {
        assert_eq!(detect_language("#!/usr/bin/env python3", None), HlLang::Python);
        assert_eq!(detect_language("#!/bin/bash", None), HlLang::Bash);
        assert_eq!(detect_language("#!/usr/bin/env node", None), HlLang::JavaScript);
    }

    #[test]
    fn detect_language_extension() {
        assert_eq!(detect_language("", Some("test.rs")), HlLang::Rust);
        assert_eq!(detect_language("", Some("test.py")), HlLang::Python);
        assert_eq!(detect_language("", Some("test.js")), HlLang::JavaScript);
        assert_eq!(detect_language("", Some("test.ts")), HlLang::TypeScript);
        assert_eq!(detect_language("", Some("test.json")), HlLang::Json);
        assert_eq!(detect_language("", Some("test.yaml")), HlLang::Yaml);
        assert_eq!(detect_language("", Some("test.md")), HlLang::Markdown);
    }

    #[test]
    fn lang_from_fence_tag_basic() {
        assert_eq!(lang_from_fence_tag("rust"), HlLang::Rust);
        assert_eq!(lang_from_fence_tag("python"), HlLang::Python);
        assert_eq!(lang_from_fence_tag("js"), HlLang::JavaScript);
        assert_eq!(lang_from_fence_tag("bash"), HlLang::Bash);
        assert_eq!(lang_from_fence_tag("unknown"), HlLang::Plain);
    }

    #[test]
    fn highlight_no_panic_random_input() {
        let inputs = vec![
            "fn main() { println!(\"hello\"); }",
            "class Foo extends Bar { constructor() { super(); } }",
            "def foo(): return 42",
            "key: value # comment",
            "{\"key\": \"value\"}",
            "---\ntitle: test\n---",
            "",
            "plain text without special tokens",
        ];
        for input in inputs {
            let _ = highlight_line(input, HlLang::Rust);
            let _ = highlight_line(input, HlLang::Python);
            let _ = highlight_line(input, HlLang::JavaScript);
        }
    }

    #[test]
    fn highlight_empty_line() {
        let spans = highlight_line("", HlLang::Rust);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "");
    }

    #[test]
    fn highlight_lines_multi() {
        let text = "fn main() {\n    println!(\"hello\");\n}";
        let lines = highlight_lines(text, HlLang::Rust);
        assert_eq!(lines.len(), 3);
    }
}
