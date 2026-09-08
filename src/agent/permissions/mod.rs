//! 权限拦截层:危险命令 + 敏感路径检测的统一入口。
//!
//! 设计参考:
//! - atomcode `bash.rs:1653` 的 `check_destructive_command`
//! - atomcode `sensitive_path.rs` 的 `references_sensitive_path`
//! - claudecode `dangerousPatterns.ts` 的正则白/黑名单
//!
//! 本轮实现 `check_bash_command` 作为 BashTool 的入口闸门。
//! 默认 fail-closed:命中任一规则即返回 [`AgentError::PermissionDenied`]。

pub mod dangerous;
pub mod sensitive;

pub use dangerous::check_destructive_command;
pub use sensitive::references_sensitive_path;

use crate::error::{AgentError, Result};

/// 综合检查:bash 命令是否应该被拦截。
///
/// - `command`: 待执行的命令字符串(可包含 `&&` / `||` / `;` / `|` 等连接符)
/// - 返回 `Ok(())` 表示放行,`Err(PermissionDenied)` 表示拦截
///
/// 拦截时错误原因直接反馈给 Agent,Agent 可根据错误改用更安全的写法重试。
pub fn check_bash_command(command: &str) -> Result<()> {
    if let Some(reason) = check_destructive_command(command) {
        return Err(AgentError::PermissionDenied {
            tool: "Bash".into(),
            reason: format!("危险命令被拦截: {reason}"),
        });
    }
    if references_sensitive_path(command) {
        return Err(AgentError::PermissionDenied {
            tool: "Bash".into(),
            reason: "命令引用了敏感路径(SSH/AWS/GnuPG/.env/shell history 等),请改用专用工具或去掉对敏感路径的引用".into(),
        });
    }
    Ok(())
}

/// 检查字符串是否看起来像 shell 命令(启发式,用于只对命令型输入做检查)。
///
/// 当前实现:含连接符(`;` `|` `&`)或换行符视作命令。
#[allow(dead_code)]
pub fn looks_like_shell_command(s: &str) -> bool {
    s.contains(';') || s.contains('|') || s.contains('&') || s.contains('\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_safe_command() {
        assert!(check_bash_command("echo hello").is_ok());
        assert!(check_bash_command("ls -la /tmp").is_ok());
        assert!(check_bash_command("cargo build --release").is_ok());
    }

    #[test]
    fn blocks_destructive_command() {
        let err = check_bash_command("rm -rf /").unwrap_err();
        match err {
            AgentError::PermissionDenied { tool, reason } => {
                assert_eq!(tool, "Bash");
                assert!(reason.contains("危险"));
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[test]
    fn blocks_sensitive_path() {
        let err = check_bash_command("cat ~/.ssh/id_rsa").unwrap_err();
        match err {
            AgentError::PermissionDenied { reason, .. } => {
                assert!(reason.contains("敏感路径"));
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[test]
    fn destructive_takes_precedence_over_sensitive() {
        // rm -rf / 也含危险,先报危险
        let err = check_bash_command("sudo cat ~/.ssh/id_rsa").unwrap_err();
        match err {
            AgentError::PermissionDenied { reason, .. } => {
                // sudo 被 dangerous 命中
                assert!(reason.contains("危险命令"));
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[test]
    fn looks_like_shell_command_heuristic() {
        assert!(looks_like_shell_command("a; b"));
        assert!(looks_like_shell_command("a | b"));
        assert!(looks_like_shell_command("a && b"));
        assert!(!looks_like_shell_command("echo hello"));
        assert!(!looks_like_shell_command("just_a_path"));
    }
}
