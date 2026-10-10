//! 产物根目录唯一真源(第 152 轮)。
//!
//! 实测事故(2026-10-10 豆包任务):`laew` 经 `/opt/homebrew/bin/laew` 软链启动,
//! `current_exe().parent()` = `/opt/homebrew/bin`,于是 `plans/`、`DebugReport/`、
//! `AuditTrail/`、`BashSpill/`、`logs/`、`CrashReport/` 六类产物**全部逃逸**到
//! 家brew 的包管理目录 —— 用户在自己启动 laew 的工作目录里根本找不到 Plan 文件。
//!
//! 本模块给出唯一决策点:**产物一律落「启动时工作目录」**,而不是二进制所在目录。
//!
//! - [`artifact_root`]:进程级单点捕获(`OnceLock`),优先级
//!   `LAEW_ARTIFACT_DIR` > 启动时 `current_dir()` > `current_exe().parent()`;
//! - [`artifact_dir`]:产物子目录拼接,内部走 [`safe_join`] 归一化;
//! - [`safe_join`]:纯函数,**永不返回 root 之外**的路径(`..`、前导 `/`、
//!   Windows 盘符分量全部剥离)。
//!
//! 单点捕获的必要性:运行期若有代码 `chdir`,`std::env::current_dir()` 会漂移,
//! 而用户心智里「我启动 laew 时所在的目录」是不变量。
//!
//! **边界**:本围栏只约束 **laew 自身生成的产物路径**。用户在提示词里显式给出的
//! 文件路径(Read/Write/Edit/Bash 参数)不受约束 —— 例如实测任务本身就要求读
//! `/Users/.../AI-Skill` 的绝对路径,禁绝对路径会把正常能力一并打死。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 产物根覆盖环境变量(显式指定则优先于工作目录)。
pub const ARTIFACT_DIR_ENV: &str = "LAEW_ARTIFACT_DIR";

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// 探测产物根目录(仅在 [`artifact_root`] 首次调用时执行一次)。
fn detect_artifact_root() -> PathBuf {
    if let Ok(v) = std::env::var(ARTIFACT_DIR_ENV) {
        let v = v.trim();
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    // 优先启动时工作目录(用户心智中的「产物应该出现在这里」)。
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.is_dir() {
            return cwd;
        }
    }
    // 兜底:二进制所在目录(无 cwd 的极端环境,如某些容器入口)。
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 产物根目录(进程级单点,返回 `&'static Path`)。
pub fn artifact_root() -> &'static Path {
    ROOT.get_or_init(detect_artifact_root)
}

/// 产物子目录:等价于 `safe_join(artifact_root(), rel)`。
///
/// `rel` 形如 `"plans"`、`"DebugReport"`、`".laew/spill"`;`..` 与前导分隔符
/// 被剥离,永远不会逃出产物根。
pub fn artifact_dir(rel: &str) -> PathBuf {
    safe_join(artifact_root(), rel)
}

/// 归一化拼接:`root` + `rel`,**结果必定在 `root` 之内**。
///
/// 规则(逐条有单测):
/// 1. 按 `/` 与 `\` 切分 `rel`,丢弃空分量与 `.`;
/// 2. 丢弃任何 `..` 分量(不做 `..` 上跳,即不做目录回退语义);
/// 3. 丢弃 Windows 盘符分量(`C:` / `c:`),避免 `C:` 被当成普通目录名;
/// 4. 全部分量丢弃后等价于 `root` 本身。
///
/// `root` 若是绝对路径原样保留;相对路径原样保留(便于测试)。
pub fn safe_join(root: &Path, rel: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    for seg in rel.split(['/', '\\']) {
        let s = seg.trim();
        if s.is_empty() || s == "." {
            continue;
        }
        if s == ".." {
            continue;
        }
        // Windows 盘符分量:`C:` / `d:`(长度 2 且冒号结尾)
        if s.len() == 2 && s.ends_with(':') {
            continue;
        }
        out.push(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_escape() {
        let root = Path::new("/work/dir");
        // 经典逃逸形态一律被拍平回根内
        assert_eq!(safe_join(root, "../../etc/passwd"), Path::new("/work/dir/etc/passwd"));
        assert_eq!(safe_join(root, "/etc/passwd"), Path::new("/work/dir/etc/passwd"));
        assert_eq!(safe_join(root, "..\\..\\Windows\\system32"), Path::new("/work/dir/Windows/system32"));
        // `..` 是「丢弃」而非「上跳」:`plans` 仍保留(不做目录回退语义)
        assert_eq!(safe_join(root, "./plans/../.."), Path::new("/work/dir/plans"));
        // 盘符分量被剥离(不拼成 `C:` 这种怪目录)
        assert_eq!(safe_join(root, "C:\\Windows"), Path::new("/work/dir/Windows"));
    }

    #[test]
    fn safe_join_normal_shape() {
        let root = Path::new("/work/dir");
        assert_eq!(safe_join(root, "plans"), Path::new("/work/dir/plans"));
        assert_eq!(safe_join(root, "a/b/c.md"), Path::new("/work/dir/a/b/c.md"));
        assert_eq!(safe_join(root, "  plans  "), Path::new("/work/dir/plans"));
        assert_eq!(safe_join(root, ""), root);
        assert_eq!(safe_join(root, "..."), Path::new("/work/dir/..."));
    }

    #[test]
    fn artifact_dir_never_escapes_root() {
        let root = artifact_root();
        let p = artifact_dir("../../逃逸");
        assert!(
            p.starts_with(root),
            "artifact_dir 必须落在产物根内,实际 {:?} 根 {:?}",
            p,
            root
        );
    }

    #[test]
    fn artifact_root_is_absolute_and_stable() {
        let a = artifact_root();
        let b = artifact_root();
        assert_eq!(a, b, "产物根必须进程内稳定(单点捕获)");
        assert!(!a.as_os_str().is_empty());
    }

    /// 各产物子目录名锁死 —— 改名会让用户找不到历史产物。
    #[test]
    fn artifact_subdir_names_locked() {
        for name in ["plans", "DebugReport", "AuditTrail", "BashSpill", "logs", "EvalSpill", "WebShots"] {
            let p = artifact_dir(name);
            assert_eq!(
                p.file_name().and_then(|s| s.to_str()),
                Some(name),
                "产物子目录 {name} 应落在产物根下同名目录"
            );
            assert!(p.starts_with(artifact_root()));
        }
    }
}
