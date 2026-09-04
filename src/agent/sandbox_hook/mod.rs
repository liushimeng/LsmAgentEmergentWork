//! 沙箱 Hook:拦截并限制写操作(Write / Edit)的目录范围。
//!
//! 仅允许在以下目录执行写入:
//! 1. **工作目录**(`work_dir`)- 启动 `laew` 时所在的目录及其递归子目录
//! 2. **系统临时目录**(`temp_dir`)- `std::env::temp_dir()` 返回的路径
//!
//! 设计见 `docs/新工具Edit_Glob_Grep与沙箱Hook设计/01-设计与解决方案.md`。

use std::path::{Path, PathBuf};

use crate::error::{AgentError, Result};

/// 沙箱配置:白名单根目录
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// 工作目录(启动命令时所在目录)
    pub work_dir: PathBuf,
    /// 系统临时目录
    pub temp_dir: PathBuf,
}

impl SandboxConfig {
    /// 用工作目录与系统临时目录构造沙箱配置。
    pub fn new(work_dir: PathBuf) -> Self {
        let temp_dir = std::env::temp_dir();
        Self { work_dir, temp_dir }
    }

    /// 测试用:完全自定义两个目录。
    pub fn for_test(work_dir: PathBuf, temp_dir: PathBuf) -> Self {
        Self { work_dir, temp_dir }
    }
}

/// 检查写操作的目标路径是否在白名单内。
///
/// - `tool_name`:工具名称(用于错误消息)
/// - `target_path`:工具调用中的 file_path(可能是相对路径或绝对路径)
///
/// 返回 `Ok(())` 表允许;返回 `Err(AgentError::SandboxViolation)` 表拦截。
pub fn check_write_path(cfg: &SandboxConfig, tool_name: &str, target_path: &str) -> Result<()> {
    let target = Path::new(target_path);

    // 解析为绝对路径:相对路径基于当前工作目录解析
    let abs = if target.is_absolute() {
        target.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| cfg.work_dir.clone())
            .join(target)
    };

    // 规范化路径(去掉 `.` `..` 等)
    let canonical = normalize_path(&abs);

    // 白名单检查:work_dir 或 temp_dir
    if starts_with(&canonical, &cfg.work_dir) || starts_with(&canonical, &cfg.temp_dir) {
        return Ok(());
    }

    // 拦截
    Err(AgentError::SandboxViolation {
        tool: tool_name.into(),
        path: target_path.into(),
        work_dir: cfg.work_dir.display().to_string(),
        temp_dir: cfg.temp_dir.display().to_string(),
    })
}

/// 路径规范化(不要求文件存在,只做 lexicographic 规范化)。
fn normalize_path(p: &Path) -> PathBuf {
    // 优先尝试 canonicalize(解析符号链接);失败则做 components 折叠
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    // fallback:手动折叠 `.` 与 `..`
    let mut components = Vec::new();
    for comp in p.components() {
        match comp {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                components.push(comp);
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(components.last(), Some(std::path::Component::Normal(_))) {
                    components.pop();
                } else {
                    components.push(comp);
                }
            }
            std::path::Component::Normal(_) => {
                components.push(comp);
            }
        }
    }
    components.iter().collect()
}

/// 检查 `path` 是否以 `prefix` 开头(按路径组件比对,避免字符串前缀误判)。
fn starts_with(path: &Path, prefix: &Path) -> bool {
    let path_comps: Vec<_> = path.components().collect();
    let prefix_comps: Vec<_> = prefix.components().collect();
    if prefix_comps.len() > path_comps.len() {
        return false;
    }
    path_comps[..prefix_comps.len()]
        .iter()
        .zip(prefix_comps.iter())
        .all(|(a, b)| a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_write_under_work_dir() {
        let cfg = SandboxConfig::for_test(PathBuf::from("/home/user/proj"), PathBuf::from("/tmp"));
        assert!(check_write_path(&cfg, "Write", "/home/user/proj/src/main.rs").is_ok());
        assert!(check_write_path(&cfg, "Write", "/home/user/proj").is_ok());
        assert!(check_write_path(&cfg, "Edit", "/home/user/proj/a/b/c.txt").is_ok());
    }

    #[test]
    fn allows_write_under_temp_dir() {
        let cfg = SandboxConfig::for_test(PathBuf::from("/home/user/proj"), PathBuf::from("/tmp"));
        assert!(check_write_path(&cfg, "Write", "/tmp/laew-output.txt").is_ok());
        assert!(check_write_path(&cfg, "Write", "/tmp/a/b/c").is_ok());
    }

    #[test]
    fn rejects_write_outside_whitelist() {
        let cfg = SandboxConfig::for_test(PathBuf::from("/home/user/proj"), PathBuf::from("/tmp"));
        let err = check_write_path(&cfg, "Write", "/etc/passwd").unwrap_err();
        assert!(matches!(err, AgentError::SandboxViolation { .. }));
    }

    #[test]
    fn rejects_parent_escape_attempt() {
        let cfg = SandboxConfig::for_test(PathBuf::from("/home/user/proj"), PathBuf::from("/tmp"));
        // ../../../etc/passwd 基于当前工作目录解析后跳出工作目录
        // 注意:这个测试的语义取决于 cwd;如果 cwd 在 /home/user/proj 下,则跳出会被拦截
        let result = check_write_path(&cfg, "Write", "/home/user/proj/../../../etc/passwd");
        // 规范化后变成 /etc/passwd,应当被拦截
        assert!(result.is_err());
    }

    #[test]
    fn rejects_home_dir() {
        let cfg = SandboxConfig::for_test(PathBuf::from("/home/user/proj"), PathBuf::from("/tmp"));
        let err = check_write_path(&cfg, "Edit", "/home/user/.bashrc").unwrap_err();
        assert!(matches!(err, AgentError::SandboxViolation { .. }));
    }

    #[test]
    fn string_prefix_false_positive() {
        // /home/user/proj2 不应被视为 /home/user/proj 的子目录
        let cfg = SandboxConfig::for_test(PathBuf::from("/home/user/proj"), PathBuf::from("/tmp"));
        let err = check_write_path(&cfg, "Write", "/home/user/proj2/evil.txt").unwrap_err();
        assert!(matches!(err, AgentError::SandboxViolation { .. }));
    }

    #[test]
    fn relative_path_resolved_against_cwd() {
        // 相对路径应基于 current_dir 解析
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().to_path_buf();
        // 使用 /var/run 作为临时目录(不太可能与 tempdir 重合)
        let cfg = SandboxConfig::for_test(work.clone(), PathBuf::from("/var/run"));
        // 把当前工作目录切到 work
        std::env::set_current_dir(&work).unwrap();
        assert!(check_write_path(&cfg, "Write", "src/main.rs").is_ok());
        // ../other 解析后是 work 的父目录下的 other,不在白名单内
        assert!(check_write_path(&cfg, "Write", "../other").is_err());
    }
}
