//! CSS @import 递归加载。
//!
//! 阶段四：解析 CSS 中的 @import 规则，递归加载所有依赖，
//! 返回内联后的 CSS 文本（@import 替换为实际内容）。
//!
//! 边界：最大递归深度 8 层；循环引用检测；@import 必须出现在规则之前。

use regex::Regex;
use std::collections::HashSet;

/// 最大递归深度。
pub const MAX_IMPORT_DEPTH: usize = 8;

/// 解析并递归加载 @import。
///
/// `css` 是当前 CSS 文本；`base_url` 是解析相对 URL 的基准；
/// `loader` 是外部加载回调（由调用方注入网络能力）。
///
/// 返回内联后的 CSS 文本（所有 @import 被替换为实际内容）。
pub fn resolve_imports<F>(
    css: &str,
    base_url: &str,
    depth: usize,
    visited: &mut HashSet<String>,
    loader: &mut F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<String, String>,
{
    if depth >= MAX_IMPORT_DEPTH {
        return Err(format!("@import 递归深度超过上限({MAX_IMPORT_DEPTH})"));
    }

    let import_re = Regex::new(r#"@import\s+(?:url\(\s*['"]?([^'")]+)['"]?\s*\)|['"]([^'"]+)['"])\s*([^;]*);?"#)
        .map_err(|e| format!("正则编译失败:{e}"))?;

    let mut result = String::with_capacity(css.len());
    let mut last_end = 0;

    for cap in import_re.captures_iter(css) {
        let whole = cap.get(0).unwrap();
        // @import 之前的 CSS 直接保留
        result.push_str(&css[last_end..whole.start()]);
        last_end = whole.end();

        let url = cap
            .get(1)
            .or_else(|| cap.get(2))
            .map(|m| m.as_str())
            .ok_or_else(|| "@import 缺少 URL".to_string())?;
        let media = cap.get(3).map(|m| m.as_str().trim()).unwrap_or("");

        // 解析绝对 URL
        let abs_url = crate::agent::browser::urlutil::resolve(base_url, url)
            .ok_or_else(|| "@import URL 解析失败".to_string())?;

        // 循环引用检测
        if visited.contains(&abs_url) {
            // 循环引用：跳过该 @import
            continue;
        }
        visited.insert(abs_url.clone());

        // 加载依赖 CSS
        let dep_css = loader(&abs_url)?;

        // 递归解析依赖中的 @import
        let inlined = resolve_imports(&dep_css, &abs_url, depth + 1, visited, loader)?;

        // 如果有 media query 条件，包裹
        if !media.is_empty() {
            result.push_str("@media ");
            result.push_str(media);
            result.push_str(" {\n");
            result.push_str(&inlined);
            result.push_str("\n}\n");
        } else {
            result.push_str(&inlined);
            result.push('\n');
        }
    }

    // 剩余部分
    result.push_str(&css[last_end..]);
    Ok(result)
}

/// 检查 CSS 是否包含 @import。
pub fn has_import(css: &str) -> bool {
    css.contains("@import")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_import_检测() {
        assert!(has_import("@import url('a.css');"));
        assert!(has_import("@import \"b.css\";"));
        assert!(!has_import("body { color: red; }"));
    }

    #[test]
    fn resolve_imports_无import直接返回() {
        let css = "body { color: red; }";
        let mut visited = HashSet::new();
        let mut loader = |_url: &str| Err("不应调用".to_string());
        let result = resolve_imports(css, "http://example.com/", 0, &mut visited, &mut loader);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), css);
    }

    #[test]
    fn resolve_imports_单层替换() {
        let css = "@import url('a.css');\nbody { color: red; }";
        let mut visited = HashSet::new();
        let mut loader = |url: &str| {
            assert_eq!(url, "http://example.com/a.css");
            Ok("body { font-size: 14px; }".to_string())
        };
        let result = resolve_imports(css, "http://example.com/", 0, &mut visited, &mut loader).unwrap();
        assert!(result.contains("font-size"));
        assert!(result.contains("color"));
    }

    #[test]
    fn resolve_imports_递归深度限制() {
        let css = "@import url('a.css');";
        let mut visited = HashSet::new();
        let mut loader = |_url: &str| Ok("@import url('b.css');".to_string());
        let result = resolve_imports(css, "http://example.com/", MAX_IMPORT_DEPTH + 1, &mut visited, &mut loader);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_imports_循环引用() {
        let css = "@import url('a.css');";
        let mut visited = HashSet::new();
        let mut loader = |_url: &str| Ok("@import url('a.css');".to_string());
        let result = resolve_imports(css, "http://example.com/", 0, &mut visited, &mut loader);
        // 循环引用应被跳过，不报错
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_imports_media条件包裹() {
        let css = "@import url('print.css') print;";
        let mut visited = HashSet::new();
        let mut loader = |_url: &str| Ok("body { color: black; }".to_string());
        let result = resolve_imports(css, "http://example.com/", 0, &mut visited, &mut loader).unwrap();
        assert!(result.contains("@media print"));
        assert!(result.contains("color: black"));
    }
}
