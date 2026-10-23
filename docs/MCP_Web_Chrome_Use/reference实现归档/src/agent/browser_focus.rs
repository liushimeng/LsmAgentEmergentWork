//! 焦点租约 —— Chrome 置前/抢焦的「借-还」治理(需求:Chrome 获取焦点后
//! 不得影响用户/人工使用电脑的其它操作)。
//!
//! # 问题本质
//!
//! CDP 操作(点击/输入/滚动/截图/读 DOM)**全部不需要 Chrome 有 OS 键盘焦点** ——
//! `Page.bringToFront` 只为「窗口可见 / 标签页激活」,连带激活浏览器窗口抢走键盘
//! 焦点纯属副作用。实测痛点:Agent 打开浏览器/置前后**永不归还焦点**,用户正在
//! 其它聊天窗口打字时击键会灌进 Chrome;全仓此前不存在任何「记录前台应用→恢复」
//! 代码。设计详案 `tmpPlan/20261013_120500_MCP_WebUse屏蔽完整性与焦点租约方案.md`。
//!
//! # 三档策略(`LAEW_WEB_FOCUS`)
//!
//! - **keep(缺省)· 借焦速还**:
//!   1. 窗口置前改走「**抬升不抢焦**」([`raise_window_no_activate`]):
//!      macOS `System Events … AXRaise`(只调窗口层级,不激活应用,与
//!      `macos_legacy::bring_to_front` 第一步同款);Windows `SetWindowPos(HWND_TOP,
//!      SWP_NOACTIVATE)`;
//!   2. 凡自动抢焦点的收口点(open / bring_to_front / 6003 focus / request_human)
//!      走 [`FocusLease`]:动作前快照前台应用 → 动作 → 收口归还。
//!   自平衡:用户本来就在盯 Chrome 时快照=Chrome,归还=无操作;用户在别的应用
//!   打字时快照=那个应用,Chrome 办完事立刻把键盘焦点还回去。
//! - **always**:旧行为 —— 置前即 `Page.bringToFront` 激活、不归还(适合全程盯屏)。
//! - **never**:完全不碰焦点(不抬升不归还;`request_human` 的 bring_to_front
//!   仍执行 —— 人工要能看到页面)。
//!
//! # 实现约束
//!
//! 快照/归还/抬升全走**系统工具子进程**(macOS osascript System Events /
//! Windows PowerShell Add-Type 一行式),零新 crate、不做 cfg 门控 FFI
//! (ring 阻断 msvc 交叉 check,cfg 门控 FFI 在本机无法编译验证,见
//! 「cargo fix 跨平台陷阱」教训);Linux 无主桌面焦点 API → no-op(fail-open)。
//! 全部失败只降级不阻断 —— 焦点治理是体验增强,不是功能前提。

use std::sync::atomic::{AtomicU64, Ordering};

/// 焦点租约策略(`LAEW_WEB_FOCUS`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPolicy {
    /// 借焦速还(缺省):置前不抢焦 + 自动抢焦点收口归还。
    Keep,
    /// 旧行为:置前即激活、不归还。
    Always,
    /// 完全不碰焦点。
    Never,
}

impl FocusPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Always => "always",
            Self::Never => "never",
        }
    }

    /// 解析 `LAEW_WEB_FOCUS` 值;`None` = 非法值(调用方回退 keep 并 warn)。
    /// 与 `LAEW_WEB_GUARD` 等同款宽容:大小写不敏感、容忍空白。
    pub fn from_env_value(v: &str) -> Option<Self> {
        match v.trim().to_ascii_lowercase().as_str() {
            "keep" | "soft" | "borrow" => Some(Self::Keep),
            "always" | "on" | "1" | "true" | "yes" | "steal" => Some(Self::Always),
            "never" | "off" | "0" | "false" | "no" => Some(Self::Never),
            _ => None,
        }
    }
}

/// 进程级策略(env 读取,单测经 [`focus_policy_from_str`] 纯函数路径)。
pub fn focus_policy_from_env() -> FocusPolicy {
    match std::env::var("LAEW_WEB_FOCUS") {
        Ok(v) => match FocusPolicy::from_env_value(&v) {
            Some(p) => p,
            None => {
                tracing::warn!("LAEW_WEB_FOCUS={v} 非法,回退 keep(借焦速还)");
                FocusPolicy::Keep
            }
        },
        Err(_) => FocusPolicy::Keep,
    }
}

/// 前台应用快照(pid + 名称;`handle` 仅 Windows HWND 用,macOS/Linux 恒 0)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontApp {
    pub name: String,
    pub pid: u32,
    pub handle: i64,
}

/// 解析 macOS 快照行(`osascript` 的 AppleScript list 字符串化:`名称, pid`)。
///
/// 名称可含逗号(如 "Code - Insiders" 无逗号,但防御起见)—— pid 在末段,
/// 从最后一个 `", "` 处切。纯函数,单测锁死。
pub fn parse_front_app_macos(line: &str) -> Option<FrontApp> {
    let line = line.trim();
    let (name, pid_s) = line.rsplit_once(", ")?;
    let pid: u32 = pid_s.trim().parse().ok()?;
    if pid == 0 {
        return None;
    }
    let name = name.trim().trim_matches('"').to_string();
    if name.is_empty() {
        return None;
    }
    Some(FrontApp { name, pid, handle: 0 })
}

/// 解析 Windows 快照行(`hwnd,pid,name`)。纯函数,单测锁死。
pub fn parse_front_app_windows(line: &str) -> Option<FrontApp> {
    let line = line.trim();
    let mut it = line.splitn(3, ',');
    let handle: i64 = it.next()?.trim().parse().ok()?;
    let pid: u32 = it.next()?.trim().parse().ok()?;
    let name = it.next().unwrap_or("").trim().to_string();
    if pid == 0 {
        return None;
    }
    Some(FrontApp { name, pid, handle })
}

/// 快照当前前台应用(`keep` 档借焦前调用)。探测失败 → None(租约空转,fail-open)。
pub fn snapshot_front_app() -> Option<FrontApp> {
    #[cfg(target_os = "macos")]
    {
        // 与 macos_legacy::bring_to_front 的激活原语同款 System Events 通道。
        let script = "tell application \"System Events\" to get {name, unix id} of first application process whose frontmost is true";
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        parse_front_app_macos(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(target_os = "windows")]
    {
        // PowerShell 一行式:取前台 HWND → 线程 pid + 进程名。低频调用(~百 ms)。
        let script = "Add-Type 'using System;using System.Runtime.InteropServices;public class LaewF{[DllImport(\"user32.dll\")]public static extern IntPtr GetForegroundWindow();[DllImport(\"user32.dll\")]public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);}';$h=[LaewF]::GetForegroundWindow();$p=[uint32]0;[void][LaewF]::GetWindowThreadProcessId($h,[ref]$p);$n='';try{$n=(Get-Process -Id $p -ErrorAction Stop).ProcessName}catch{};Write-Output ($h.ToInt64().ToString()+','+$p.ToString()+','+$n)";
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        parse_front_app_windows(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// 把键盘焦点归还给快照应用(`keep` 档借焦收口调用)。
///
/// 自身进程/空快照直接跳过;失败仅 debug(应用已退出等),不阻断工具语义。
pub fn restore_front_app(prev: &FrontApp) -> bool {
    if prev.pid == 0 || prev.pid == std::process::id() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"System Events\" to set frontmost of (first application process whose unix id is {}) to true",
            prev.pid
        );
        let ok = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            tracing::debug!(pid = prev.pid, name = %prev.name, "焦点归还失败(fail-open)");
        }
        ok
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "Add-Type 'using System;using System.Runtime.InteropServices;public class LaewR{{[DllImport(\"user32.dll\")]public static extern bool SetForegroundWindow(IntPtr h);}}';[void][LaewR]::SetForegroundWindow([IntPtr]{})",
            prev.handle
        );
        let ok = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            tracing::debug!(pid = prev.pid, name = %prev.name, "焦点归还失败(fail-open)");
        }
        ok
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = prev;
        false
    }
}

/// 窗口「抬升不抢焦」:把 pid 的主窗口提到 z-order 前排但**不激活**应用。
///
/// macOS 走 `System Events … perform action "AXRaise"`(与
/// `macos_legacy::bring_to_front` 第一步同款语义 —— 只调窗口层级);
/// Windows 走 `SetWindowPos(HWND_TOP, SWP_NOACTIVATE)`。
/// 失败(fail-open)返回 false,调用方不得回退成抢焦点(`always` 档才走激活)。
pub fn raise_window_no_activate(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"System Events\" to tell (first application process whose unix id is {}) to perform action \"AXRaise\" of window 1",
            pid
        );
        std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "windows")]
    {
        // HWND_TOP=0;SWP_NOSIZE|SWP_NOMOVE|SWP_NOACTIVATE|SWP_SHOWWINDOW = 0x53
        let script = format!(
            "Add-Type 'using System;using System.Runtime.InteropServices;public class LaewW{{[DllImport(\"user32.dll\")]public static extern bool SetWindowPos(IntPtr h,IntPtr a,int x,int y,int cx,int cy,uint f);}}';$h=(Get-Process -Id {} -ErrorAction SilentlyContinue).MainWindowHandle;if($h -ne [IntPtr]::Zero){{[void][LaewW]::SetWindowPos($h,[IntPtr]0,0,0,0,0,0x53)}}",
            pid
        );
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = pid;
        false
    }
}

/// launch 模式浏览器主进程 PID(供「抬升不抢焦」定位 Chrome 窗口;
/// connect 模式恒 0 —— 接管的是用户自己的浏览器,零干预不抬升)。
pub fn browser_pid_hint() -> u32 {
    crate::agent::browser::peek_browser_pid()
}

/// 焦点租约:借焦前 [`FocusLease::capture`] 快照,收口 [`FocusLease::release`] 归还。
///
/// 仅 `keep` 档有实际动作;`always`/`never` 与快照失败时空转(零副作用)。
pub struct FocusLease {
    prev: Option<FrontApp>,
    policy: FocusPolicy,
}

impl FocusLease {
    /// 借焦前捕获当前前台应用(仅 keep 档快照;其余档空租约)。
    pub fn capture() -> Self {
        let policy = focus_policy_from_env();
        let prev = if policy == FocusPolicy::Keep {
            snapshot_front_app()
        } else {
            None
        };
        Self { prev, policy }
    }

    /// 收口归还焦点。返回是否实际归还;消费 self(租约一次性)。
    pub fn release(self) -> bool {
        match (self.policy, self.prev) {
            (FocusPolicy::Keep, Some(prev)) => {
                let ok = restore_front_app(&prev);
                if ok {
                    note_focus_restored();
                }
                ok
            }
            _ => false,
        }
    }

    /// 快照的应用(供响应字段对账)。
    pub fn snapshot_app(&self) -> Option<&FrontApp> {
        self.prev.as_ref()
    }
}

/// 供响应透出的策略计数(keep 档归还次数;调试对账用,进程级弱一致)。
static FOCUS_RESTORE_COUNT: AtomicU64 = AtomicU64::new(0);

/// 记录一次成功归还(内部用)。
pub fn note_focus_restored() {
    FOCUS_RESTORE_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// 已归还次数(调试/测试对账)。
pub fn focus_restored_count() -> u64 {
    FOCUS_RESTORE_COUNT.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量类测试互斥(browser_overlay.rs 同款,防并行 set_var 互踩)。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn 焦点策略解析三档与别名() {
        assert_eq!(FocusPolicy::from_env_value("keep"), Some(FocusPolicy::Keep));
        assert_eq!(FocusPolicy::from_env_value(" soft "), Some(FocusPolicy::Keep));
        assert_eq!(FocusPolicy::from_env_value("ALWAYS"), Some(FocusPolicy::Always));
        assert_eq!(FocusPolicy::from_env_value("steal"), Some(FocusPolicy::Always));
        assert_eq!(FocusPolicy::from_env_value("never"), Some(FocusPolicy::Never));
        assert_eq!(FocusPolicy::from_env_value("off"), Some(FocusPolicy::Never));
        assert_eq!(FocusPolicy::from_env_value("garbage"), None);
    }

    #[test]
    fn 焦点策略环境变量缺省与非法回退() {
        let _g = env_lock();
        unsafe {
            std::env::remove_var("LAEW_WEB_FOCUS");
        }
        assert_eq!(focus_policy_from_env(), FocusPolicy::Keep);
        unsafe {
            std::env::set_var("LAEW_WEB_FOCUS", "always");
        }
        assert_eq!(focus_policy_from_env(), FocusPolicy::Always);
        unsafe {
            std::env::set_var("LAEW_WEB_FOCUS", "garbage");
        }
        assert_eq!(focus_policy_from_env(), FocusPolicy::Keep, "非法值回退 keep");
        unsafe {
            std::env::remove_var("LAEW_WEB_FOCUS");
        }
    }

    #[test]
    fn macos快照行解析容忍名称逗号() {
        let a = parse_front_app_macos("Google Chrome, 40897").unwrap();
        assert_eq!(a.name, "Google Chrome");
        assert_eq!(a.pid, 40897);
        assert_eq!(a.handle, 0);
        // 名称含逗号:pid 在末段,从最后 ", " 切
        let b = parse_front_app_macos("Foo, Bar App, 12").unwrap();
        assert_eq!(b.name, "Foo, Bar App");
        assert_eq!(b.pid, 12);
        // 引号包裹的名称容忍剥除
        let c = parse_front_app_macos("\"Terminal\", 3").unwrap();
        assert_eq!(c.name, "Terminal");
        assert!(parse_front_app_macos("not-a-pair").is_none());
        assert!(parse_front_app_macos("App, 0").is_none(), "pid=0 无效");
        assert!(parse_front_app_macos("").is_none());
    }

    #[test]
    fn windows快照行解析() {
        let a = parse_front_app_windows("197123,40897,chrome").unwrap();
        assert_eq!(a.handle, 197123);
        assert_eq!(a.pid, 40897);
        assert_eq!(a.name, "chrome");
        // 进程名缺失时容忍(名称空,pid/handle 仍可用)
        let b = parse_front_app_windows("5,6").unwrap();
        assert_eq!(b.pid, 6);
        assert!(parse_front_app_windows("x,2,y").is_none(), "handle 非数字无效");
        assert!(parse_front_app_windows("1,0,n").is_none(), "pid=0 无效");
    }

    #[test]
    fn 租约空转不归还() {
        // 快照 None 的租约 release 必须为 false 且零副作用
        let lease = FocusLease { prev: None, policy: FocusPolicy::Keep };
        assert!(!lease.release());
        // always 档即使有快照也不归还(旧行为)
        let lease = FocusLease {
            prev: Some(FrontApp { name: "X".into(), pid: 1, handle: 0 }),
            policy: FocusPolicy::Always,
        };
        assert!(!lease.release());
    }

    #[test]
    fn 归还自身进程跳过() {
        let me = FrontApp {
            name: "laew".into(),
            pid: std::process::id(),
            handle: 0,
        };
        assert!(!restore_front_app(&me), "自身进程不做归还");
    }
}
