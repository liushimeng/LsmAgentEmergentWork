//! Write 工具:覆盖写入/新建文件,自动创建父目录。

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AgentError, Result};
use crate::tool::Tool;

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str { "Write" }

    fn description(&self) -> &str {
        "将 content 完整写入 file_path(覆盖式)。\n\
         - file_path 应为绝对路径或相对工作目录的路径。\n\
         - 父目录不存在会自动创建。\n\
         - 写入成功后返回写入字节数与目标路径。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "目标文件路径" },
                "content": { "type": "string", "description": "完整文件内容" }
            },
            "required": ["file_path", "content"],
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
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "缺少 string 类型参数 content".into(),
            })?;

        let path = resolve_path(path_str);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| AgentError::ToolExecution {
                    tool: self.name().into(),
                    reason: format!("创建父目录失败: {e}"),
                })?;
            }
        }

        let bytes = content.len();
        fs::write(&path, content).map_err(|e| AgentError::ToolExecution {
            tool: self.name().into(),
            reason: format!("写入失败: {e}"),
        })?;

        Ok(format!(
            "[Write] 写入 {bytes} 字节到 {}",
            path.display()
        ))
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
    use tempfile::TempDir;

    #[tokio::test]
    async fn writes_and_creates_parent() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("a/b/c.txt");
        let path_str = target.to_str().unwrap().to_string();
        let out = WriteTool
            .execute(json!({"file_path": path_str, "content": "hi\n"}))
            .await
            .unwrap();
        assert!(out.contains("写入"));
        assert!(target.exists());
        assert_eq!(fs::read_to_string(&target).unwrap(), "hi\n");
    }

    #[tokio::test]
    async fn overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("f.txt");
        fs::write(&target, "old").unwrap();
        WriteTool
            .execute(json!({
                "file_path": target.to_str().unwrap(),
                "content": "new"
            }))
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    }

    #[tokio::test]
    async fn missing_content_argument_errors() {
        let err = WriteTool
            .execute(json!({"file_path": "/tmp/x"}))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::ToolExecution { .. }));
    }
}
