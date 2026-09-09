//! Write 工具:覆盖写入/新建文件,自动创建父目录。
//!
//! 受 sandbox-hook 拦截:目标路径必须在工作目录或系统临时目录下。

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::sandbox_hook::{check_write_path, SandboxConfig};
use crate::agent::tools::Tool;
use crate::error::{AgentError, Result};

pub struct WriteTool {
    sandbox: SandboxConfig,
}

impl WriteTool {
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
impl Tool for WriteTool {
    fn name(&self) -> &str { "Write" }

    fn description(&self) -> &str {
        "将 content 完整写入 file_path(覆盖式)。\n\
         - file_path 应为绝对路径或相对工作目录的路径。\n\
         - 父目录不存在会自动创建。\n\
         - 写入成功后返回写入字节数与目标路径。\n\
         - [沙箱] 仅可在工作目录或系统临时目录内写入文件。"
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

        // 沙箱拦截:Check-What-You-Write —— 先解析出最终落盘路径,再对该路径做
        // 白名单检查,保证「检查的就是要写的」(2026-09-09 第 08 轮沙箱细化)。
        let path = resolve_path(path_str);
        check_write_path(&self.sandbox, self.name(), &path.to_string_lossy())?;

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

/// 解析 Write 工具入参路径(简单版,与 Edit 一致)。
///
/// 1. 绝对路径 → 原样返回;
/// 2. 相对路径 → **工作目录**(env::current_dir())拼接,父目录不存在由 execute 自动创建。
///
/// 历史:关联报告 2026-09-09_04 D-003 曾引入「根目录回退」两条分支;沙箱细化
/// (2026-09-09 第 08 轮)后移除——回退落点在白名单(工作目录/临时目录)外必被拦截,
/// 只会把本应写进工作目录的相对路径错误改道到根目录再报错。Read 场景的根目录
/// 回退由 read.rs 自带的路径诊断承担,不受影响。
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
        let out = WriteTool::without_sandbox()
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
        WriteTool::without_sandbox()
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
        let err = WriteTool::without_sandbox()
            .execute(json!({"file_path": "/tmp/x"}))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::ToolExecution { .. }));
    }

    #[tokio::test]
    async fn sandbox_blocks_outside_write() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("f.txt");
        fs::write(&target, "old").unwrap();

        // 沙箱配置为完全不同的目录
        let tool = WriteTool::new(SandboxConfig::for_test(
            PathBuf::from("/home/user/proj"),
            PathBuf::from("/var/tmp"),
        ));
        let err = tool
            .execute(json!({
                "file_path": target.to_str().unwrap(),
                "content": "new"
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::SandboxViolation { .. }));
    }

    /// 白名单内(工作目录)写入放行,含父目录自动创建(真实 roots 管线)。
    #[tokio::test]
    async fn sandbox_allows_write_inside_work_dir() {
        let dir = TempDir::new().unwrap();
        let tool = WriteTool::new(SandboxConfig::new(dir.path().to_path_buf()));
        let target = dir.path().join("sub/dir/f.txt");
        tool.execute(json!({
            "file_path": target.to_str().unwrap(),
            "content": "ok"
        }))
        .await
        .unwrap();
        assert!(target.exists());
        assert_eq!(fs::read_to_string(&target).unwrap(), "ok");
    }

    /// 检查发生在解析之后:带 `..` 穿越的绝对路径按规范化落点判断,
    /// 折叠后跳出白名单即拦截(不依赖进程 cwd,避免与并行测试竞态)。
    #[tokio::test]
    async fn sandbox_check_governs_resolved_path() {
        let dir = TempDir::new().unwrap();
        // temp 根指向不存在的目录,避免 tempdir 本身位于 /tmp 造成「穿越后仍在 /tmp」的偶发放行
        let tool = WriteTool::new(SandboxConfig::for_test(
            dir.path().to_path_buf(),
            PathBuf::from("/var/laew-sbx-nonexist"),
        ));
        let target = dir.path().join("sub/../../escape.txt"); // 规范化后 = /tmp/escape.txt
        let err = tool
            .execute(json!({
                "file_path": target.to_str().unwrap(),
                "content": "x"
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::SandboxViolation { .. }));
    }
}
