//! Glob 工具:快速文件模式匹配,返回按修改时间倒序排列的路径列表。
//!
//! - 支持 `**/*.rs`、`src/**/*.ts` 等 glob 模式
//! - 默认跳过隐藏文件 / 目录与常见忽略目录(`target/`、`node_modules/`、`.git/`)
//! - 结果上限默认 200,超出时截断并提示

use std::path::PathBuf;
use std::time::SystemTime;

use async_trait::async_trait;
use globset::GlobBuilder;
use serde_json::{json, Value};
use walkdir::WalkDir;

use crate::agent::tools::Tool;
use crate::error::{AgentError, Result};

const DEFAULT_LIMIT: usize = 200;

/// 常见忽略目录(basename),跳过可显著加速
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", ".cache", "dist", "build"];

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> &str {
        "快速文件模式匹配工具,适用于任何规模的代码库。\n\
         - 支持 glob 模式,如 \"**/*.rs\" 或 \"src/**/*.ts\"。\n\
         - 结果按修改时间倒序排列(最新在前)。\n\
         - 默认跳过隐藏文件/目录与 .git/target/node_modules 等。\n\
         - 结果上限 200 条,超出时会截断。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "glob 匹配模式,如 \"**/*.rs\"" },
                "path": { "type": "string", "description": "搜索根目录,默认当前工作目录" }
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

        let base_dir = match args.get("path").and_then(Value::as_str) {
            Some(p) => PathBuf::from(p),
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };
        if !base_dir.is_dir() {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!("不是有效目录: {}", base_dir.display()),
            });
        }

        // 构建 GlobMatcher
        // 注意:不使用 literal_separator,让 `*` 也能匹配 `/`,这样 `*.toml` 能匹配根目录文件
        let glob = GlobBuilder::new(pattern)
            .build()
            .map_err(|e| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!("无效的 glob 模式: {e}"),
            })?
            .compile_matcher();

        let base_dir_canonical = base_dir.canonicalize().unwrap_or_else(|_| base_dir.clone());
        let mut matched: Vec<(PathBuf, SystemTime)> = Vec::new();

        for entry in WalkDir::new(&base_dir_canonical)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                // 跳过隐藏目录与常见忽略目录(但保留根目录)
                let depth = e.depth();
                if depth == 0 {
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

            // glob 匹配:优先对"相对于 base_dir 的路径"做匹配;失败则对文件名做匹配
            let rel = path.strip_prefix(&base_dir_canonical).ok();
            let fname = path.file_name().unwrap_or_default();
            let matched_path = if let Some(rel) = rel {
                glob.is_match(rel) || glob.is_match(fname)
            } else {
                glob.is_match(fname)
            };
            if !matched_path {
                continue;
            }

            let mtime = entry
                .metadata()
                .map(|m| m.modified().unwrap_or(SystemTime::UNIX_EPOCH))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            matched.push((path.to_path_buf(), mtime));
        }

        // 按修改时间倒序
        matched.sort_by(|a, b| b.1.cmp(&a.1));

        let total = matched.len();
        let limit = DEFAULT_LIMIT;
        let truncated = matched.len() > limit;
        let slice = if truncated {
            &matched[..limit]
        } else {
            &matched[..]
        };

        let mut buf = String::new();
        buf.push_str(&format!(
            "<<< Glob 匹配 {pattern} (base: {}, 显示 {}/{}) >>>\n",
            base_dir.display(),
            slice.len(),
            total
        ));
        for (p, _) in slice {
            buf.push_str(&format!("{}\n", p.display()));
        }
        if truncated {
            buf.push_str(&format!("...[截断,还有 {} 条未显示]\n", total - limit));
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join("tests")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main()").unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn add").unwrap();
        fs::write(dir.path().join("tests/test1.rs"), "test").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        dir
    }

    #[tokio::test]
    async fn glob_matches_simple_pattern() {
        let dir = setup_dir();
        let out = GlobTool
            .execute(json!({"pattern": "*.toml", "path": dir.path().to_str().unwrap()}))
            .await
            .unwrap();
        assert!(out.contains("Cargo.toml"), "output was:\n{out}");
        assert!(!out.contains(".rs"), "output was:\n{out}");
    }

    #[tokio::test]
    async fn glob_matches_recursive_pattern() {
        let dir = setup_dir();
        let out = GlobTool
            .execute(json!({"pattern": "**/*.rs", "path": dir.path().to_str().unwrap()}))
            .await
            .unwrap();
        assert!(out.contains("main.rs"));
        assert!(out.contains("lib.rs"));
        assert!(out.contains("test1.rs"));
        assert!(!out.contains("Cargo.toml"));
    }

    #[tokio::test]
    async fn glob_skips_git_and_target() {
        let dir = setup_dir();
        fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        fs::write(dir.path().join("target/debug/app.rs"), "x").unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/foo.rs"), "x").unwrap();

        let out = GlobTool
            .execute(json!({"pattern": "**/*.rs", "path": dir.path().to_str().unwrap()}))
            .await
            .unwrap();
        assert!(!out.contains("target/"));
        assert!(!out.contains(".git/"));
        assert!(out.contains("main.rs"));
    }

    #[tokio::test]
    async fn glob_invalid_pattern_errors() {
        let dir = setup_dir();
        // 无效的 glob 字符(未闭合的方括号)
        let err = GlobTool
            .execute(json!({"pattern": "[invalid", "path": dir.path().to_str().unwrap()}))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::ToolExecution { .. }));
    }
}
