//! Chrome user-data-dir 策略:交互式任务保留专用登录态,CI 默认一次性隔离。
//!
//! 持久 profile 只存当前用户 home 下,并按工作目录哈希隔离;绝不读写用户日常
//! Chrome 默认 profile,也不把 Cookie 落到项目工作区。

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use super::ChromeMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProfileSelection {
    pub dir: PathBuf,
    pub persistent: bool,
}

pub(super) fn select(mode: ChromeMode) -> ProfileSelection {
    if let Some(dir) = std::env::var_os("LAEW_CHROME_PROFILE")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return ProfileSelection { dir, persistent: true };
    }
    let persistent = persistent_requested(
        mode,
        std::env::var("LAEW_CHROME_PROFILE_MODE").ok().as_deref(),
    );
    if persistent {
        if let Some(home) = home_dir() {
            let work = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            return ProfileSelection {
                dir: persistent_dir(&work, &home),
                persistent: true,
            };
        }
        tracing::warn!("无法定位用户 home,Chrome 回退一次性 profile(本次登录态不保留)");
    }
    ProfileSelection { dir: ephemeral_dir(), persistent: false }
}

fn persistent_requested(mode: ChromeMode, raw: Option<&str>) -> bool {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("persistent" | "persist" | "on" | "1" | "true" | "yes") => true,
        Some("ephemeral" | "temp" | "temporary" | "off" | "0" | "false" | "no") => false,
        _ => mode.is_headed(),
    }
}

fn persistent_dir(work_dir: &Path, home: &Path) -> PathBuf {
    let canonical = work_dir.canonicalize().unwrap_or_else(|_| work_dir.to_path_buf());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut hasher);
    home.join(".laew")
        .join("chrome_profiles")
        .join(format!("work_{:016x}", hasher.finish()))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

fn ephemeral_dir() -> PathBuf {
    let mut b = [0u8; 4];
    rand::Rng::fill(&mut rand::thread_rng(), &mut b);
    let suffix = b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    std::env::temp_dir().join(format!("laew_chrome_{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 有头默认持久_无头默认一次性_环境策略可覆盖() {
        assert!(persistent_requested(ChromeMode::Headed, None));
        assert!(!persistent_requested(ChromeMode::Hidden, None));
        assert!(!persistent_requested(ChromeMode::Headed, Some("ephemeral")));
        assert!(persistent_requested(ChromeMode::Hidden, Some("persistent")));
    }

    #[test]
    fn 持久目录按工作区隔离且不落入项目目录() {
        let home = Path::new("/tmp/laew-home");
        let a = persistent_dir(Path::new("/tmp/work-a"), home);
        let b = persistent_dir(Path::new("/tmp/work-b"), home);
        assert_ne!(a, b);
        assert_eq!(a, persistent_dir(Path::new("/tmp/work-a"), home));
        assert!(a.starts_with(home.join(".laew/chrome_profiles")));
        assert!(!a.starts_with("/tmp/work-a"));
    }
}
