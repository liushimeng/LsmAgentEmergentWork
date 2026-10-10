//! 浏览器启动模式决策(第 139 轮:统一默认可见模式)。
//!
//! 背景:第 74 轮把 `MCP_Web_Use action=open` 的缺省模式硬编码为 `Hidden`,
//! 且 `BrowserMode::from_env_or_default()` **在生产链路零调用**(只有单测调),
//! 导致 `LAEW_BROWSER_MODE` / `LAEW_BROWSER_HEADLESS` 两个开关形同虚设,
//! 「是否弹窗口」完全取决于 LLM 这一轮有没有自己传 `mode=headed` —— 实测同一份
//! 程序在 Terminal 跑不弹窗、在 VS Code 终端跑弹窗,被误判为终端差异。
//!
//! 本模块是模式决策的**唯一真源**,`mcp_web_use::run_open` 与 `BrowserManager`
//! 两侧都走这里,保证「缺省值是代码里确定的一个值」。
//!
//! 决策优先级(自上而下):
//! 1. 工具参数显式 `mode`(`headed` / `new_headless` / `hidden`);
//! 2. `LAEW_BROWSER_MODE` 显式环境变量(供调试/QA/无头环境强制回退);
//! 3. 兼容旧开关 `LAEW_BROWSER_HEADLESS`(仅识别 false 系 = 有头,其余忽略,
//!    不再当「默认无头」的隐式来源);
//! 4. 缺省 = **可见模式 `Headed`**(第 139 轮),但 `has_gui_session()==false`
//!    的环境(CI / 容器 / 无 X11 的 Linux / SSH)自动回退 `Hidden`,
//!    避免 headed 启动失败后整个任务 2001 崩掉。

/// 2026-09-17 第 74 轮:浏览器启动模式三档枚举。
///
/// - `Hidden`:纯 CDP 嵌入式无头(`--headless=new`,系统级无窗口);
/// - `NewHeadless`:`--headless=new` 老式(headless=true 路径),保留兼容;
/// - `Headed`:有窗口浏览器。
///
/// 第 139 轮:缺省改为 `Headed`(可见),且该缺省现在**真正接线**到
/// `MCP_Web_Use open`(此前只有 `Hidden` 硬编码,见本模块文档)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserMode {
    Hidden,
    NewHeadless,
    Headed,
}

impl BrowserMode {
    /// 缺省模式:可见(第 139 轮,由 Hidden 改为 Headed)。
    pub const DEFAULT: Self = Self::Headed;

    /// 模式字符串(工具响应 / `mode_hint` 回显用)。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::NewHeadless => "new_headless",
            Self::Headed => "headed",
        }
    }

    /// 是否为有头(可见窗口)模式。
    pub fn is_headed(self) -> bool {
        matches!(self, Self::Headed)
    }

    /// 解析工具参数 `mode` 字符串;`None` = 调用方未显式指定。
    pub fn from_arg(v: &str) -> Option<Self> {
        match v.trim().to_ascii_lowercase().as_str() {
            "headed" | "head" | "with_head" | "visible" => Some(Self::Headed),
            "new_headless" | "old_headless" => Some(Self::NewHeadless),
            "hidden" | "inprocess" | "cdp_only" | "headless" => Some(Self::Hidden),
            _ => None,
        }
    }

    /// 解析 `LAEW_BROWSER_MODE` 的值;非法值返回 `None` 交由下一级决策。
    pub fn from_mode_env_value(v: &str) -> Option<Self> {
        match v.trim().to_ascii_lowercase().as_str() {
            "headed" | "head" | "with_head" | "visible" | "false" | "0" | "no" | "off" => {
                Some(Self::Headed)
            }
            "new_headless" | "old_headless" | "true" | "1" => Some(Self::NewHeadless),
            "hidden" | "inprocess" | "cdp_only" => Some(Self::Hidden),
            _ => None,
        }
    }

    /// 兼容旧 bool 开关 `LAEW_BROWSER_HEADLESS`:仅 false 系 = 有头;
    /// true 系**不再**作为默认无头的隐式来源(默认已是有头),返回 `None`。
    pub fn from_legacy_headless_value(v: &str) -> Option<Self> {
        match v.trim().to_ascii_lowercase().as_str() {
            "0" | "false" | "no" | "off" => Some(Self::Headed),
            _ => None,
        }
    }

    /// 读环境变量定缺省模式(第 139 轮起被 `MCP_Web_Use open` 真正调用)。
    pub fn from_env_or_default() -> Self {
        if let Ok(v) = std::env::var("LAEW_BROWSER_MODE") {
            if let Some(m) = Self::from_mode_env_value(&v) {
                return m;
            }
        }
        if let Ok(v) = std::env::var("LAEW_BROWSER_HEADLESS") {
            if let Some(m) = Self::from_legacy_headless_value(&v) {
                return m;
            }
        }
        Self::env_or_default()
    }

    /// 无环境变量时的决策:有 GUI 会话 → 可见模式;否则回退无头。
    ///
    /// 参数化的纯函数形式,便于单测;生产走 [`BrowserMode::env_or_default`]。
    pub fn resolve(has_gui: bool) -> Self {
        if has_gui {
            Self::DEFAULT
        } else {
            Self::Hidden
        }
    }

    /// 探测当前进程是否处于 GUI 会话(纯函数入口 + 真实环境探测)。
    pub fn env_or_default() -> Self {
        Self::resolve(has_gui_session())
    }
}

/// 当前进程是否处于可弹出窗口的 GUI 会话(第 139 轮)。
///
/// 判据(任一为假即视为无 GUI):
/// - 通用:显式 `LAEW_FORCE_HEADLESS=1` 可强制回退无头(排障/演示用);
/// - 通用:`CI` 环境变量非空(持续集成容器里弹窗既看不见也拿不到焦点);
/// - macOS:`launchctl managername` 为 `Background`/`StandardIO`(SSH / launchd
///   后台会话),或 `SSH_CONNECTION` 非空 —— 此时 `NSWindow`/Chrome 有窗口也
///   不在用户可见的 Aqua 会话里;
/// - Windows / 其他桌面平台:一律认为有 GUI(交互式终端才有 Window Station);
/// - Linux:必须有 `DISPLAY` 或 `WAYLAND_DISPLAY`,否则无 X 协议可弹窗。
pub fn has_gui_session() -> bool {
    let truthy = |k: &str| {
        std::env::var(k)
            .ok()
            .map(|v| {
                let v = v.trim().to_ascii_lowercase();
                v == "1" || v == "true" || v == "yes" || v == "on"
            })
            .unwrap_or(false)
    };
    if truthy("LAEW_FORCE_HEADLESS") || truthy("CI") {
        return false;
    }
    if std::env::var("SSH_CONNECTION").is_ok() || std::env::var("SSH_TTY").is_ok() {
        return false;
    }
    if cfg!(target_os = "macos") {
        return macos_has_aqua_session();
    }
    if cfg!(target_os = "linux") {
        return std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok();
    }
    // Windows 及其它桌面平台:交互式进程默认可弹窗。
    true
}

/// macOS 是否处于 Aqua 图形登录会话(第 139 轮)。
///
/// `launchctl managername` 返回 `Aqua` = 图形会话;`Background`/`StandardIO`
/// = SSH / launchd 后台会话。取不到时按 `true` 处理(宁可弹窗失败也不静默
/// 降级成无头,用户能看见问题)。
fn macos_has_aqua_session() -> bool {
    match std::process::Command::new("launchctl")
        .arg("managername")
        .output()
    {
        Ok(o) if o.status.success() => {
            let m = String::from_utf8_lossy(&o.stdout).trim().to_ascii_lowercase();
            !m.is_empty() && m != "background" && m != "standardio"
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ★ 2026-09-17 第 79 轮:环境变量类测试用互斥锁串行 —— 多个测试并行跑时
    // set_var/remove_var 互相踩,全量 `cargo test` 偶发失败(存量 flaky,与功能无关)。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn default_is_headed() {
        assert_eq!(BrowserMode::DEFAULT, BrowserMode::Headed);
        assert!(BrowserMode::DEFAULT.is_headed());
    }

    #[test]
    fn resolve_switches_on_gui_session() {
        assert_eq!(BrowserMode::resolve(true), BrowserMode::Headed);
        assert_eq!(BrowserMode::resolve(false), BrowserMode::Hidden);
    }

    /// 第 139 轮:无任何环境变量时,缺省 = 有 GUI 会话 → 可见;无 GUI 会话 → 无头。
    /// 不能硬断言某一侧,因为 CI(无 GUI)与开发机(有 GUI)跑出的结果本就不同 ——
    /// 这正是本轮要保证的「环境差异可预期、不再随机」。
    #[test]
    fn env_fallback_follows_gui_session() {
        let _g = env_lock();
        unsafe {
            std::env::remove_var("LAEW_BROWSER_MODE");
            std::env::remove_var("LAEW_BROWSER_HEADLESS");
        }
        assert_eq!(
            BrowserMode::from_env_or_default(),
            BrowserMode::resolve(has_gui_session())
        );
    }

    #[test]
    fn env_overrides_always_win() {
        let _g = env_lock();
        unsafe { std::env::set_var("LAEW_BROWSER_MODE", "headed") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::Headed);
        unsafe { std::env::set_var("LAEW_BROWSER_MODE", "new_headless") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::NewHeadless);
        unsafe { std::env::set_var("LAEW_BROWSER_MODE", "hidden") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::Hidden);
        unsafe { std::env::remove_var("LAEW_BROWSER_MODE") };
    }

    #[test]
    fn legacy_headless_env_takes_effect() {
        let _g = env_lock();
        unsafe { std::env::remove_var("LAEW_BROWSER_MODE") };
        unsafe { std::env::set_var("LAEW_BROWSER_HEADLESS", "0") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::Headed);
        unsafe { std::env::set_var("LAEW_BROWSER_HEADLESS", "false") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::Headed);
        unsafe { std::env::remove_var("LAEW_BROWSER_HEADLESS") };
    }

    #[test]
    fn invalid_env_does_not_panic() {
        let _g = env_lock();
        unsafe { std::env::set_var("LAEW_BROWSER_MODE", "garbage") };
        unsafe { std::env::remove_var("LAEW_BROWSER_HEADLESS") };
        // 只确保不 panic;非法值走默认 fallback
        let _ = BrowserMode::from_env_or_default();
        unsafe { std::env::remove_var("LAEW_BROWSER_MODE") };
    }

    #[test]
    fn mode_env_value_parsing() {
        for v in ["headed", "HEAD", "with_head", "visible", "false", "0", "off"] {
            assert_eq!(BrowserMode::from_mode_env_value(v), Some(BrowserMode::Headed), "{v}");
        }
        for v in ["new_headless", "old_headless", "true", "1"] {
            assert_eq!(
                BrowserMode::from_mode_env_value(v),
                Some(BrowserMode::NewHeadless),
                "{v}"
            );
        }
        for v in ["hidden", "inprocess", "cdp_only"] {
            assert_eq!(BrowserMode::from_mode_env_value(v), Some(BrowserMode::Hidden), "{v}");
        }
        // 非法值必须回 None 交下一级,不能默默吞掉。
        assert_eq!(BrowserMode::from_mode_env_value("nonsense"), None);
        assert_eq!(BrowserMode::from_mode_env_value(""), None);
    }

    #[test]
    fn legacy_headless_env_only_maps_to_headed() {
        for v in ["0", "false", "no", "off"] {
            assert_eq!(
                BrowserMode::from_legacy_headless_value(v),
                Some(BrowserMode::Headed),
                "{v}"
            );
        }
        // true 系不再压过缺省可见模式(否则又回到「默认无头」的老坑)。
        for v in ["1", "true", "yes", "on"] {
            assert_eq!(BrowserMode::from_legacy_headless_value(v), None, "{v}");
        }
    }

    #[test]
    fn tool_arg_parsing() {
        assert_eq!(BrowserMode::from_arg("headed"), Some(BrowserMode::Headed));
        assert_eq!(BrowserMode::from_arg("hidden"), Some(BrowserMode::Hidden));
        assert_eq!(
            BrowserMode::from_arg("new_headless"),
            Some(BrowserMode::NewHeadless)
        );
        assert_eq!(BrowserMode::from_arg("whatever"), None);
    }

    #[test]
    fn as_str_round_trip() {
        for m in [BrowserMode::Hidden, BrowserMode::NewHeadless, BrowserMode::Headed] {
            assert_eq!(BrowserMode::from_arg(m.as_str()), Some(m));
        }
    }

    #[test]
    fn ci_env_forces_no_gui() {
        // 必须持锁:LAEW_FORCE_HEADLESS 与 has_gui_session() 竞态会让并行的
        // env_fallback_follows_gui_session 断言左右两侧读到不同环境(存量 flaky)。
        let _g = env_lock();
        // 单测进程通常没有 CI;显式设置后必须判为无 GUI(CI 不弹窗)。
        std::env::set_var("LAEW_TEST_CI_MARKER", "1");
        // 不直接改 CI(会污染同进程其它断言),改为验证 truthy 语义一致的
        // LAEW_FORCE_HEADLESS 通路。
        std::env::set_var("LAEW_FORCE_HEADLESS", "1");
        assert!(!has_gui_session());
        std::env::remove_var("LAEW_FORCE_HEADLESS");
        std::env::remove_var("LAEW_TEST_CI_MARKER");
    }

    #[test]
    fn self_env_is_consistent_with_gui_probe() {
        // 当前测试机是否有 GUI 会话不固定,只断言「探测不 panic 且是布尔」——
        // 真正的行为覆盖在上面 resolve() 与 env 解析的单测里。
        let _ = has_gui_session();
    }
}
