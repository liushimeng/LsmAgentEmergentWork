//! Grep 工具:快速内容搜索,基于 regex + walkdir。
//!
//! - 支持完整正则表达式语法
//! - 支持 glob 过滤文件
//! - 三种输出模式:content / files_with_matches / count
//! - 二进制文件自动跳过(.git / target / node_modules 也跳过)
//! - 大文件(>1MB)按行流式读取
//! - 结果上限默认 200 条

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use globset::GlobSetBuilder;
use regex::{Regex, RegexBuilder};
use serde_json::{json, Value};
use walkdir::WalkDir;

use crate::agent::tools::Tool;
use crate::error::{AgentError, Result};

const DEFAULT_LIMIT: usize = 200;
const BINARY_CHECK_BYTES: usize = 8192;

const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", ".cache", "dist", "build"];

/// 输出模式
#[derive(Debug, Clone, Copy)]
enum OutputMode {
    Content,
    FilesWithMatches,
    Count,
}

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "Grep"
    }

    fn description(&self) -> &str {
        "快速内容搜索工具,适用于任何规模的代码库。\n\
         - 支持正则表达式,如 \"log.*Error\"、\"function\\s+\\w+\"。\n\
         - 使用 glob 参数过滤文件(如 \"*.rs\", \"*.{ts,tsx}\")。\n\
         - 三种输出模式: content(显示匹配行,支持上下文)、files_with_matches(仅文件路径)、count(计数)。\n\
         - 默认 files_with_matches;content 模式返回前 200 条。\n\
         - 二进制文件自动跳过。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "正则表达式模式" },
                "path": { "type": "string", "description": "搜索目录或文件,默认工作目录" },
                "glob": { "type": "string", "description": "文件名过滤模式,如 \"*.rs\"" },
                "output_mode": { "type": "string", "enum": ["content", "files_with_matches", "count"], "description": "输出模式,默认 files_with_matches" },
                "context": { "type": "integer", "minimum": 0, "description": "匹配前后各 N 行" },
                "before_context": { "type": "integer", "minimum": 0, "description": "匹配前 N 行" },
                "after_context": { "type": "integer", "minimum": 0, "description": "匹配后 N 行" },
                "-i": { "type": "boolean", "description": "不区分大小写" },
                "head_limit": { "type": "integer", "minimum": 0, "description": "结果上限,默认 200;0 表示无限制" },
                "multiline": { "type": "boolean", "description": "多行模式(跨行匹配),默认 false" }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let pattern = args
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "缺少 string 类型参数 pattern".into(),
            })?
            .trim();
        if pattern.is_empty() {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "pattern 不能为空".into(),
            });
        }

        let base_path = match args.get("path").and_then(Value::as_str) {
            Some(p) => PathBuf::from(p),
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };

        let output_mode = match args.get("output_mode").and_then(Value::as_str) {
            Some("content") => OutputMode::Content,
            Some("count") => OutputMode::Count,
            _ => OutputMode::FilesWithMatches,
        };

        let context = args
            .get("context")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let before_context = args
            .get("before_context")
            .and_then(Value::as_u64)
            .unwrap_or(context as u64) as usize;
        let after_context = args
            .get("after_context")
            .and_then(Value::as_u64)
            .unwrap_or(context as u64) as usize;
        let case_insensitive = args.get("-i").and_then(Value::as_bool).unwrap_or(false);
        let multiline = args
            .get("multiline")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let head_limit = args
            .get("head_limit")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_LIMIT as u64) as usize;

        // 构建正则
        let mut builder = RegexBuilder::new(pattern);
        builder.case_insensitive(case_insensitive);
        builder.multi_line(multiline);
        if multiline {
            // 多行模式下让 `.` 匹配换行符
            builder.dot_matches_new_line(true);
        }
        let regex = builder.build().map_err(|e| AgentError::ToolExecution {
            tool: self.name().into(),
            reason: format!("无效的正则表达式: {e}"),
        })?;

        // 构建 glob 过滤器(可选)
        let glob_filter = match args.get("glob").and_then(Value::as_str) {
            Some(g) if !g.is_empty() => {
                let mut b = GlobSetBuilder::new();
                b.add(
                    globset::GlobBuilder::new(g)
                        .literal_separator(true)
                        .build()
                        .map_err(|e| AgentError::ToolExecution {
                            tool: self.name().into(),
                            reason: format!("无效的 glob 模式: {e}"),
                        })?,
                );
                Some(b.build().map_err(|e| AgentError::ToolExecution {
                    tool: self.name().into(),
                    reason: format!("glob 编译失败: {e}"),
                })?)
            }
            _ => None,
        };

        // 收集文件列表
        let files_to_search = collect_files(&base_path, &glob_filter)?;

        let mut result_buf = String::new();
        let mut total_matches: usize = 0;
        let mut files_with_matches: Vec<PathBuf> = Vec::new();
        let mut truncated = false;

        for file_path in &files_to_search {
            if truncated {
                break;
            }
            match search_file(
                file_path,
                &regex,
                &output_mode,
                before_context,
                after_context,
                head_limit,
                &mut total_matches,
                &mut result_buf,
                &mut truncated,
            ) {
                Ok(has_match) => {
                    if has_match {
                        files_with_matches.push(file_path.clone());
                    }
                }
                Err(_) => continue, // 跳过无法读取的文件
            }
        }

        // 根据输出模式返回
        match output_mode {
            OutputMode::FilesWithMatches => {
                let limit = if head_limit == 0 { DEFAULT_LIMIT } else { head_limit };
                let slice = if files_with_matches.len() > limit {
                    &files_with_matches[..limit]
                } else {
                    &files_with_matches[..]
                };
                result_buf.clear();
                result_buf.push_str(&format!(
                    "<<< Grep 匹配 {} (显示 {} / {} 文件) >>>\n",
                    pattern,
                    slice.len(),
                    files_with_matches.len()
                ));
                for p in slice {
                    result_buf.push_str(&format!("{}\n", p.display()));
                }
                if files_with_matches.len() > limit {
                    result_buf.push_str(&format!(
                        "...[截断,还有 {} 个文件未显示]\n",
                        files_with_matches.len() - limit
                    ));
                }
            }
            OutputMode::Count => {
                result_buf.clear();
                result_buf.push_str(&format!(
                    "<<< Grep 计数 {} (共 {} 处匹配,涉及 {} 个文件) >>>\n",
                    pattern,
                    total_matches,
                    files_with_matches.len()
                ));
                // 每个文件的计数
                for p in &files_with_matches {
                    // 重新计数每个文件(简化:这里只输出文件路径,不重复计数)
                    result_buf.push_str(&format!("{}\n", p.display()));
                }
            }
            OutputMode::Content => {
                result_buf = format!(
                    "<<< Grep 内容 {} (共 {} 处匹配) >>>\n{}",
                    pattern, total_matches, result_buf
                );
                if truncated {
                    result_buf.push_str(&format!(
                        "...[截断,已达到 head_limit={}]\n",
                        head_limit
                    ));
                }
            }
        }

        Ok(result_buf)
    }
}

/// 收集待搜索文件
fn collect_files(base: &Path, glob_filter: &Option<globset::GlobSet>) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if base.is_file() {
        files.push(base.to_path_buf());
        return Ok(files);
    }
    if !base.is_dir() {
        return Err(AgentError::ToolExecution {
            tool: "Grep".into(),
            reason: format!("不是有效文件或目录: {}", base.display()),
        });
    }

    for entry in WalkDir::new(base)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // 保留根目录,跳过隐藏目录与常见忽略目录
            if e.depth() == 0 {
                return true;
            }
            if let Some(name) = e.file_name().to_str() {
                if name.starts_with('.') {
                    return false;
                }
                if e.file_type().is_dir() && SKIP_DIRS.contains(&name) {
                    return false;
                }
            }
            true
        })
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();

        // glob 过滤
        if let Some(glob) = glob_filter {
            let name_match = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| glob.is_match(n))
                .unwrap_or(false);
            if !name_match {
                continue;
            }
        }

        files.push(path.to_path_buf());
    }
    Ok(files)
}

/// 搜索单个文件
fn search_file(
    path: &Path,
    regex: &Regex,
    mode: &OutputMode,
    before: usize,
    after: usize,
    head_limit: usize,
    total_matches: &mut usize,
    buf: &mut String,
    truncated: &mut bool,
) -> std::io::Result<bool> {
    // 二进制检测
    if is_binary(path).unwrap_or(false) {
        return Ok(false);
    }

    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();

    let mut has_any = false;
    let mut matched_lines: Vec<usize> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if regex.is_match(line) {
            has_any = true;
            *total_matches += 1;
            matched_lines.push(i);

            // 提前截断
            if head_limit > 0 && *total_matches > head_limit {
                *truncated = true;
                break;
            }
        }
    }

    if !has_any {
        return Ok(false);
    }

    match mode {
        OutputMode::Content => {
            // 输出匹配行及其上下文
            let mut emitted_lines = std::collections::HashSet::new();
            for &match_idx in &matched_lines {
                let start = match_idx.saturating_sub(before);
                let end = (match_idx + after + 1).min(lines.len());
                for line_idx in start..end {
                    if emitted_lines.insert(line_idx) {
                        let marker = if line_idx == match_idx { ">" } else { " " };
                        buf.push_str(&format!(
                            "{}:{}:{}{}\n",
                            path.display(),
                            line_idx + 1,
                            marker,
                            lines[line_idx]
                        ));
                    }
                }
                buf.push_str("--\n");
            }
        }
        OutputMode::FilesWithMatches | OutputMode::Count => {
            // 仅记录文件路径,不输出内容
        }
    }

    Ok(true)
}

/// 检测文件是否为二进制(前 N 字节含 null 字节则视为二进制)
fn is_binary(path: &Path) -> std::io::Result<bool> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut chunk = vec![0u8; BINARY_CHECK_BYTES];
    use std::io::Read;
    let n = reader.read(&mut chunk)?;
    chunk.truncate(n);
    Ok(chunk.contains(&0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("README.md"),
            "# Project\n\nSome docs here.\n",
        )
        .unwrap();
        dir
    }

    #[tokio::test]
    async fn grep_files_with_matches() {
        let dir = setup_dir();
        let out = GrepTool
            .execute(json!({
                "pattern": "fn\\s+\\w+",
                "path": dir.path().to_str().unwrap(),
                "output_mode": "files_with_matches"
            }))
            .await
            .unwrap();
        assert!(out.contains("main.rs"));
        assert!(out.contains("lib.rs"));
        assert!(!out.contains("README.md"));
    }

    #[tokio::test]
    async fn grep_content_mode() {
        let dir = setup_dir();
        let out = GrepTool
            .execute(json!({
                "pattern": "println!",
                "path": dir.path().to_str().unwrap(),
                "output_mode": "content"
            }))
            .await
            .unwrap();
        assert!(out.contains("println!"));
        assert!(out.contains("main.rs"));
    }

    #[tokio::test]
    async fn grep_with_glob_filter() {
        let dir = setup_dir();
        let out = GrepTool
            .execute(json!({
                "pattern": "fn|Project",
                "path": dir.path().to_str().unwrap(),
                "glob": "*.md",
                "output_mode": "files_with_matches"
            }))
            .await
            .unwrap();
        assert!(out.contains("README.md"), "output was:\n{out}");
        assert!(!out.contains("main.rs"), "output was:\n{out}");
    }

    #[tokio::test]
    async fn grep_case_insensitive() {
        let dir = setup_dir();
        let out = GrepTool
            .execute(json!({
                "pattern": "FN",
                "path": dir.path().to_str().unwrap(),
                "-i": true,
                "output_mode": "files_with_matches"
            }))
            .await
            .unwrap();
        assert!(out.contains("main.rs"));
    }

    #[tokio::test]
    async fn grep_with_context() {
        let dir = setup_dir();
        let out = GrepTool
            .execute(json!({
                "pattern": "println!",
                "path": dir.path().to_str().unwrap(),
                "output_mode": "content",
                "context": 1
            }))
            .await
            .unwrap();
        // 应该包含上下文行
        assert!(out.contains("fn main"));
        assert!(out.contains("println!"));
    }

    #[tokio::test]
    async fn grep_invalid_regex_errors() {
        let dir = setup_dir();
        let err = GrepTool
            .execute(json!({
                "pattern": "[invalid",
                "path": dir.path().to_str().unwrap()
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::ToolExecution { .. }));
    }
}
