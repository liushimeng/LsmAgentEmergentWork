//! Bash 工具:在工作目录下执行 shell 命令,带超时与输出截断。

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::error::{AgentError, Result};
use crate::tool::Tool;

/// 默认超时与上限
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_OUTPUT_CHARS: usize = 30_000;

/// 当前进程工作目录(通过 env::current_dir 惰性获取)
fn current_work_dir() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str { "Bash" }

    fn description(&self) -> &str {
        "在工作目录下执行 bash 命令,返回 stdout + stderr + 退出码。\n\
         - 超时单位毫秒,默认 120000,最大 600000。\n\
         - 一次调用执行一条命令;多条命令请用 && / ; 连接。\n\
         - 避免使用 cat/head/tail/sed/awk/echo 这类专用命令 — 请改用 Read / Write 等专用工具。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "待执行的 bash 命令字符串" },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_TIMEOUT_MS as i64,
                    "description": "可选超时(毫秒)"
                },
                "description": {
                    "type": "string",
                    "description": "一句话描述命令用途(便于审计)"
                }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "缺少 string 类型参数 command".into(),
            })?
            .to_string();

        let timeout_ms = args
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        let mut cmd = Command::new("bash");
        cmd.arg("-lc").arg(&command);
        cmd.current_dir(current_work_dir());

        let output = tokio::time::timeout(Duration::from_millis(timeout_ms), cmd.output()).await;
        let output = match output {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Err(AgentError::ToolExecution {
                    tool: self.name().into(),
                    reason: format!("启动命令失败: {e}"),
                });
            }
            Err(_) => {
                return Ok(format!(
                    "[Bash] 超时(>{timeout_ms}ms)被强制终止;命令可能仍在后台运行。"
                ));
            }
        };

        let code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let stdout_trunc = truncate(&stdout, MAX_OUTPUT_CHARS);
        let stderr_trunc = truncate(&stderr, MAX_OUTPUT_CHARS);

        let mut buf = String::new();
        if !stdout_trunc.truncated.is_empty() {
            buf.push_str("<stdout>\n");
            buf.push_str(&stdout_trunc.text);
            if stdout_trunc.omitted > 0 {
                buf.push_str(&format!("\n...[stdout 截断,省略 {} 字符]", stdout_trunc.omitted));
            }
            buf.push('\n');
        }
        if !stderr_trunc.truncated.is_empty() {
            buf.push_str("<stderr>\n");
            buf.push_str(&stderr_trunc.text);
            if stderr_trunc.omitted > 0 {
                buf.push_str(&format!("\n...[stderr 截断,省略 {} 字符]", stderr_trunc.omitted));
            }
            buf.push('\n');
        }
        if buf.is_empty() {
            buf.push_str("<stdout>\n(无输出)\n");
        }
        buf.push_str(&format!("\n<exit_code>{code}</exit_code>"));
        Ok(buf)
    }
}

struct Truncated {
    text: String,
    truncated: String,
    omitted: usize,
}

fn truncate(s: &str, max: usize) -> Truncated {
    if s.len() <= max {
        Truncated {
            text: s.to_string(),
            truncated: s.to_string(),
            omitted: 0,
        }
    } else {
        // 防止按 char 边界切割出错,按 char 索引切
        let cut = s
            .char_indices()
            .nth(max)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        Truncated {
            text: s[..cut].to_string(),
            truncated: s[..cut].to_string(),
            omitted: s.len() - cut,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_via_bash() {
        let out = BashTool
            .execute(json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(out.contains("hello"));
        assert!(out.contains("<exit_code>0</exit_code>"));
    }

    #[tokio::test]
    async fn exit_code_propagated() {
        let out = BashTool
            .execute(json!({"command": "exit 7"}))
            .await
            .unwrap();
        assert!(out.contains("<exit_code>7</exit_code>"));
    }

    #[tokio::test]
    async fn missing_command_argument_errors() {
        let err = BashTool.execute(json!({})).await.unwrap_err();
        match err {
            AgentError::ToolExecution { reason, .. } => assert!(reason.contains("command")),
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
