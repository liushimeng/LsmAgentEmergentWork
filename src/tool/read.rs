//! Read 工具:带行号读取文件,支持 offset/limit 分页。

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AgentError, Result};
use crate::tool::Tool;

const DEFAULT_LIMIT: usize = 2000;
const MAX_LIMIT: usize = 4000;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str { "Read" }

    fn description(&self) -> &str {
        "读取文本文件并按 cat -n 风格返回带行号的内容。\n\
         - file_path 必须为绝对路径或可解析的相对路径(相对于工作目录)。\n\
         - offset/limit 用于分页,limit 默认 2000 行,最大 4000。\n\
         - 单行超长会被截断到 4000 字符并标注。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "待读取的文件路径" },
                "offset": { "type": "integer", "minimum": 1, "description": "从第 N 行开始(1-based)" },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT as i64, "description": "最多读取多少行" }
            },
            "required": ["file_path"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path_str = args
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "缺少 string 类型参数 file_path".into(),
            })?;
        let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_LIMIT as u64)
            .min(MAX_LIMIT as u64) as usize;

        let path = resolve_path(path_str);
        let metadata = fs::metadata(&path).map_err(|e| AgentError::ToolExecution {
            tool: self.name().into(),
            reason: format!("stat 失败: {e}"),
        })?;
        if !metadata.is_file() {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!("不是普通文件: {}", path.display()),
            });
        }

        let content = fs::read_to_string(&path).map_err(|e| AgentError::ToolExecution {
            tool: self.name().into(),
            reason: format!("读取失败: {e}"),
        })?;

        let lines: Vec<&str> = content.split_inclusive('\n').collect();
        let total = lines.len();
        let start_idx = (offset - 1).min(total);
        let end_idx = (start_idx + limit).min(total);

        let width = (total.max(1)).to_string().len();
        let mut buf = String::new();
        buf.push_str(&format!(
            "<<< {} (lines {}-{} / total {}) >>>\n",
            path.display(),
            offset,
            end_idx,
            total
        ));
        for (i, line) in lines[start_idx..end_idx].iter().enumerate() {
            let line_num = start_idx + i + 1;
            let display = if line.len() > 4000 {
                format!("{}...[截断]", &line[..line.char_indices().nth(4000).map(|(n, _)| n).unwrap_or(line.len())])
            } else {
                line.trim_end_matches('\n').to_string()
            };
            buf.push_str(&format!("{:>width$}\t{}\n", line_num, display, width = width));
        }
        Ok(buf)
    }
}

fn resolve_path(p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[tokio::test]
    async fn reads_with_line_numbers() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "alpha").unwrap();
        writeln!(f, "beta").unwrap();
        writeln!(f, "gamma").unwrap();
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
        assert!(out.contains("alpha"));
        assert!(out.contains("beta"));
        assert!(out.contains("gamma"));
        assert!(out.contains("total 3"));
    }

    #[tokio::test]
    async fn respects_offset_and_limit() {
        let mut f = NamedTempFile::new().unwrap();
        for i in 1..=5 {
            writeln!(f, "line{}", i).unwrap();
        }
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool
            .execute(json!({"file_path": p, "offset": 2, "limit": 2}))
            .await
            .unwrap();
        assert!(out.contains("line2"));
        assert!(out.contains("line3"));
        assert!(!out.contains("line1"));
        assert!(!out.contains("line4"));
    }

    #[tokio::test]
    async fn missing_argument_errors() {
        let err = ReadTool.execute(json!({})).await.unwrap_err();
        assert!(matches!(err, AgentError::ToolExecution { .. }));
    }
}
