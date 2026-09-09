//! 沙箱 Hook:拦截并限制写操作(Write / Edit)的目录范围。
//!
//! 仅允许在以下目录执行写入:
//! 1. **工作目录**(`work_dir`)- 启动 `laew` 时所在的目录及其递归子目录
//! 2. **系统临时目录**(`temp_dir`)- `std::env::temp_dir()` 返回的路径
//!
//! Read / Glob / Grep 不受限;Bash 不经过本沙箱(保留全部权限)。
//!
//! 细化(2026-09-09 第 08 轮,方案 `tmpPlan/2026-09-09_08-沙箱权限管控细化方案.md`):
//! - **Check-What-You-Write**:工具层必须「先解析出最终落盘路径、再交给本模块检查」,
//!   防止「检查 A 路径、实写 B 路径」的绕过(如曾经的根目录回退)。
//! - **符号链接防逃逸**:规范化时对目标的最深已存在祖先做 canonicalize 再拼回尾部,
//!   工作目录内指向外部的 symlink 会被解析后拦截。
//! - **白名单双形态**:每个白名单根同时以「原始折叠形态 + canonical 形态」参与前缀匹配。
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
    /// 白名单前缀集合(每个根的「原始折叠 + canonical」双形态,构造时预计算去重)。
    /// 空路径前缀会被忽略,避免「空前缀匹配一切」的 fail-open。
    roots: Vec<PathBuf>,
}

impl SandboxConfig {
    /// 用工作目录与系统临时目录构造沙箱配置。
    pub fn new(work_dir: PathBuf) -> Self {
        let temp_dir = std::env::temp_dir();
        Self::build(work_dir, temp_dir)
    }

    /// 测试用:完全自定义两个目录。
    pub fn for_test(work_dir: PathBuf, temp_dir: PathBuf) -> Self {
        Self::build(work_dir, temp_dir)
    }

    fn build(work_dir: PathBuf, temp_dir: PathBuf) -> Self {
        let mut roots: Vec<PathBuf> = Vec::new();
        for raw in [&work_dir, &temp_dir] {
            // 与目标路径走同一条规范化管线(fold + 最深已存在祖先 canonicalize),
            // 双方对称处理,符号链接形态差异在两侧同时被解析。
            let folded = normalize_path(raw);
            // 空前缀(如 work_dir 折叠后为空)忽略 —— 空前缀会匹配一切
            if folded.as_os_str().is_empty() {
                continue;
            }
            if !roots.contains(&folded) {
                roots.push(folded.clone());
            }
            if let Ok(canonical) = folded.canonicalize() {
                if !roots.contains(&canonical) {
                    roots.push(canonical);
                }
            }
        }
        Self { work_dir, temp_dir, roots }
    }

    /// 白名单前缀集合(测试与诊断用)。
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
}

/// 检查写操作的目标路径是否在白名单内。
///
/// - `tool_name`:工具名称(用于错误消息)
/// - `target_path`:工具调用中的 file_path。**调用方应传入「最终落盘路径」**
///   (Write / Edit 先 `resolve_path` 再调用本函数),保证检查的就是要写的。
///   绝对路径原样规范化;相对路径基于当前工作目录解析。
///
/// 返回 `Ok(())` 表允许;返回 `Err(AgentError::SandboxViolation)` 表拦截。
///
/// 已知边界:检查与落盘之间存在理论 TOCTOU 窗口(检查后 symlink 被替换)。
/// 本沙箱的威胁模型是「拦截 LLM 误操作」,不防本地恶意进程竞态。
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

    // 规范化路径(fold + 符号链接防逃逸)
    let canonical = normalize_path(&abs);

    // 白名单检查:任一双形态前缀命中即放行
    if cfg.roots.iter().any(|root| starts_with(&canonical, root)) {
        return Ok(());
    }

    // 拦截(path 报「规范化后的真实落点」,便于 LLM 自纠)
    Err(AgentError::SandboxViolation {
        tool: tool_name.into(),
        path: canonical.display().to_string(),
        work_dir: cfg.work_dir.display().to_string(),
        temp_dir: cfg.temp_dir.display().to_string(),
    })
}

/// 路径规范化:先词典序折叠 `.`/`..`,再对「最深已存在祖先」做 canonicalize 拼回尾部。
///
/// 这样新建文件(整体不存在,canonicalize 失败)也能解析掉路径中已存在部分里的
/// 符号链接,堵住「工作目录内 symlink 指向外部」的逃逸。
fn normalize_path(p: &Path) -> PathBuf {
    let folded = fold(p);
    match canonical_prefix(&folded) {
        Some((base, tail)) => base.join(tail),
        None => folded,
    }
}

/// 词典序折叠 `.` 与 `..`(不要求任何部分存在)。
fn fold(p: &Path) -> PathBuf {
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

/// 自 `p` 向上找「最深已存在祖先」,返回 `(canonical(祖先), 剩余尾部)`。
/// 整条链都不存在(理论上仅相对空路径)时返回 `None`。
fn canonical_prefix(p: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut cur = p.to_path_buf();
    let mut tail = PathBuf::new();
    loop {
        if let Ok(c) = cur.canonicalize() {
            return Some((c, tail));
        }
        // 路径以 `..` 结尾时 file_name() 为 None,交回 fold 结果处理
        let name = cur.file_name()?.to_os_string();
        tail = Path::new(&name).join(&tail);
        cur = cur.parent()?.to_path_buf();
    }
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

    // ===== 2026-09-09 第 08 轮沙箱细化新增 =====

    /// 符号链接逃逸:工作目录内 symlink 指向外部,新建/覆盖文件都必须被拦截。
    #[cfg(unix)]
    #[test]
    fn symlink_escape_blocked() {
        use std::os::unix::fs::symlink;
        let work = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig::for_test(
            work.path().to_path_buf(),
            PathBuf::from("/var/run-sbx-nonexist"),
        );
        // 场景 1:经「指向外部目录的 symlink」新建文件
        symlink(outside.path(), work.path().join("lnk")).unwrap();
        let target = work.path().join("lnk/new.txt");
        let err = check_write_path(&cfg, "Write", target.to_str().unwrap()).unwrap_err();
        assert!(matches!(err, AgentError::SandboxViolation { .. }));
        // 场景 2:目标本身是「指向外部文件的 symlink」(覆盖写会跟随链接)
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "x").unwrap();
        symlink(&outside_file, work.path().join("lnkfile")).unwrap();
        let target2 = work.path().join("lnkfile");
        let err2 = check_write_path(&cfg, "Edit", target2.to_str().unwrap()).unwrap_err();
        assert!(matches!(err2, AgentError::SandboxViolation { .. }));
    }

    /// 白名单根以符号链接别名书写时,经别名与经真实路径写入都放行(双形态匹配)。
    #[cfg(unix)]
    #[test]
    fn symlinked_whitelist_root_still_allowed() {
        use std::os::unix::fs::symlink;
        let real = tempfile::tempdir().unwrap();
        let alias_parent = tempfile::tempdir().unwrap();
        let alias = alias_parent.path().join("alias");
        symlink(real.path(), &alias).unwrap();
        let cfg = SandboxConfig::for_test(alias.clone(), PathBuf::from("/var/run-sbx-nonexist"));
        assert!(check_write_path(&cfg, "Write", alias.join("new.txt").to_str().unwrap()).is_ok());
        assert!(
            check_write_path(&cfg, "Write", real.path().join("new2.txt").to_str().unwrap()).is_ok()
        );
    }

    /// canonical_prefix:自目标向上找最深已存在祖先,canonicalize 后拼回尾部。
    #[test]
    fn canonical_prefix_resolves_deepest_existing_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        let deep_canonical = deep.canonicalize().unwrap();
        // 尾部一级不存在
        let (base, tail) = canonical_prefix(&deep.join("c.txt")).unwrap();
        assert_eq!(base, deep_canonical);
        assert_eq!(tail, PathBuf::from("c.txt"));
        // 尾部多级不存在
        let (base2, tail2) = canonical_prefix(&deep.join("x/y/z")).unwrap();
        assert_eq!(base2, deep_canonical);
        assert_eq!(tail2, PathBuf::from("x/y/z"));
        // 目标本身存在 → canonicalize 整体
        let (base3, tail3) = canonical_prefix(&deep).unwrap();
        assert_eq!(base3, deep_canonical);
        assert!(tail3.as_os_str().is_empty());
    }

    /// 拦截错误中 path 字段报告「规范化后的真实落点」(而非原始入参)。
    #[test]
    fn error_reports_normalized_path() {
        let cfg = SandboxConfig::for_test(PathBuf::from("/home/user/proj"), PathBuf::from("/tmp"));
        let err = check_write_path(&cfg, "Write", "/home/user/proj/../../../../etc/evil.txt")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("/etc/evil.txt"), "错误应包含规范化路径: {msg}");
    }
}
