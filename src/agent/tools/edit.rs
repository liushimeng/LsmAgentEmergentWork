//! Edit 工具:在文件中执行精准字符串替换。
//!
//! - `old_string` 必须与文件内容完全一致(包含缩进),且默认唯一匹配
//! - `replace_all=true` 可替换所有匹配项
//! - 受 sandbox-hook 拦截:目标路径必须在工作目录或系统临时目录下

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::sandbox_hook::{check_write_path, SandboxConfig};
use crate::agent::tools::Tool;
use crate::error::{AgentError, Result};

pub struct EditTool {
    sandbox: SandboxConfig,
}

impl EditTool {
    pub fn new(sandbox: SandboxConfig) -> Self {
        Self { sandbox }
    }

    /// 向后兼容:无沙箱限制(仅测试用)。
    #[cfg(test)]
    pub fn without_sandbox() -> Self {
        Self {
            sandbox: SandboxConfig::for_test(PathBuf::from("/"), PathBuf::from("/")),
        }
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> &str {
        "在文件中执行精准字符串替换。\n\
         - file_path 必须为绝对路径或相对于工作目录的相对路径。\n\
         - old_string 必须与文件内容完全一致(包含空格/缩进)。\n\
         - 默认 old_string 必须唯一匹配;若存在多个匹配请将 old_string 扩展为更大的上下文,\n\
           或设置 replace_all=true 一次性替换所有匹配。\n\
         - new_string 必须与 old_string 不同。\n\
         - 编辑前建议先 Read 该文件以确认内容。\n\
         - [沙箱] 仅可在工作目录或系统临时目录内修改文件。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "待修改文件的路径" },
                "old_string": { "type": "string", "description": "需要被替换的原文(必须与文件内容完全一致)" },
                "new_string": { "type": "string", "description": "用于替换的新文本(必须与 old_string 不同)" },
                "replace_all": { "type": "boolean", "default": false, "description": "是否替换所有匹配项" }
            },
            "required": ["file_path", "old_string", "new_string"],
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
        let old = args
            .get("old_string")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "缺少 string 类型参数 old_string".into(),
            })?;
        let new = args
            .get("new_string")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "缺少 string 类型参数 new_string".into(),
            })?;
        let replace_all = args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if old == new {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "new_string 必须与 old_string 不同".into(),
            });
        }

        // 沙箱拦截
        check_write_path(&self.sandbox, self.name(), path_str)?;

        let path = resolve_path(path_str);

        // 必须是已存在的普通文件
        if !path.exists() {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!("文件不存在: {}", path.display()),
            });
        }
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

        if content.is_empty() {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "文件为空,无法执行替换".into(),
            });
        }

        let count = content.matches(old).count();
        if count == 0 {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "未找到匹配的 old_string;可能文件内容已变更,请先 Read 该文件确认内容。".into(),
            });
        }
        if count > 1 && !replace_all {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!(
                    "old_string 在文件中匹配了 {count} 次;请扩大上下文使其唯一,或设置 replace_all=true。"
                ),
            });
        }

        let replaced = if replace_all {
            content.replace(old, new)
        } else {
            // count == 1,安全替换一次
            content.replacen(old, new, 1)
        };

        fs::write(&path, &replaced).map_err(|e| AgentError::ToolExecution {
            tool: self.name().into(),
            reason: format!("写入失败: {e}"),
        })?;

        Ok(format!(
            "[Edit] 已替换 {count} 处匹配;文件 {}",
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
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn edit_replaces_unique_match() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "fn main() {{").unwrap();
        writeln!(f, "    println!(\"hello\");").unwrap();
        writeln!(f, "}}").unwrap();
        let p = f.path().to_str().unwrap().to_string();

        let out = EditTool::without_sandbox()
            .execute(json!({
                "file_path": p,
                "old_string": "hello",
                "new_string": "world"
            }))
            .await
            .unwrap();
        assert!(out.contains("已替换 1 处"));

        let content = fs::read_to_string(&p).unwrap();
        assert!(content.contains("world"));
        assert!(!content.contains("hello"));
    }

    #[tokio::test]
    async fn edit_rejects_duplicate_without_replace_all() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "abc").unwrap();
        writeln!(f, "abc").unwrap();
        let p = f.path().to_str().unwrap().to_string();

        let err = EditTool::without_sandbox()
            .execute(json!({
                "file_path": p,
                "old_string": "abc",
                "new_string": "xyz"
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::ToolExecution { .. }));
        let msg = format!("{err}");
        assert!(msg.contains("2 次"));
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "aaa").unwrap();
        writeln!(f, "bbb").unwrap();
        writeln!(f, "aaa").unwrap();
        let p = f.path().to_str().unwrap().to_string();

        let out = EditTool::without_sandbox()
            .execute(json!({
                "file_path": p,
                "old_string": "aaa",
                "new_string": "ccc",
                "replace_all": true
            }))
            .await
            .unwrap();
        assert!(out.contains("已替换 2 处"));

        let content = fs::read_to_string(&p).unwrap();
        assert!(!content.contains("aaa"));
        assert_eq!(content.matches("ccc").count(), 2);
    }

    #[tokio::test]
    async fn edit_rejects_missing_target() {
        let err = EditTool::without_sandbox()
            .execute(json!({
                "file_path": "/tmp/nonexistent-file-xyz-123.txt",
                "old_string": "a",
                "new_string": "b"
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::ToolExecution { .. }));
        assert!(format!("{err}").contains("不存在"));
    }

    #[tokio::test]
    async fn edit_rejects_same_content() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "hello").unwrap();
        let p = f.path().to_str().unwrap().to_string();

        let err = EditTool::without_sandbox()
            .execute(json!({
                "file_path": p,
                "old_string": "hello",
                "new_string": "hello"
            }))
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("必须与 old_string 不同"));
    }

    #[tokio::test]
    async fn sandbox_blocks_outside_write() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "hello").unwrap();
        let p = f.path().to_str().unwrap().to_string();

        // 沙箱配置为完全不同的目录
        let tool = EditTool::new(SandboxConfig::for_test(
            PathBuf::from("/home/user/proj"),
            PathBuf::from("/var/tmp"),
        ));
        let err = tool
            .execute(json!({
                "file_path": p,
                "old_string": "hello",
                "new_string": "world"
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::SandboxViolation { .. }));
    }
}
