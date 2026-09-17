//! Fallback 窗口驱动(Linux / 其他非 Windows/macOS 平台)。
//!
//! 尽力而为策略:
//! - 检测到 `wmctrl` 时,`wmctrl -l -p` 可列举窗口(id/标题/PID);
//! - 检测到 `xdotool` 时,`windowactivate` 可聚焦窗口;
//! - 控件级 `inspect` / `act` 无跨桌面环境统一 API(X11/Wayland 分裂,
//!   AT-SPI 覆盖率差),一律返回结构化「平台不支持」错误,fail-closed,
//!   让 QC 判定失败并回流 Yolo 给出「请在 Windows/macOS 执行」的用户建议。
//!
//! 设计见 `docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md` §2.3 Fallback 后端。

use std::process::Command;

use super::{
    matches_filter, platform_err, ControlAction, ControlNode, Rect, WindowDriver, WindowInfo,
};
use crate::error::Result;

/// 不支持控件级操作时的统一文案(LLM 可读,指导改道)。
pub const UNSUPPORTED_MSG: &str = "当前平台不支持控件级窗口操控(需要 Windows 的 UI Automation \
     或 macOS 的 Accessibility API)。Linux 桌面环境(X11/Wayland)无统一控件树接口;\
     请改用 Bash 工具配合具体命令行方式完成,或告知用户在 Windows/macOS 上执行本任务。";

pub struct FallbackDriver {
    has_wmctrl: bool,
    has_xdotool: bool,
}

impl Default for FallbackDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FallbackDriver {
    pub fn new() -> Self {
        Self {
            has_wmctrl: command_exists("wmctrl"),
            has_xdotool: command_exists("xdotool"),
        }
    }
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

impl WindowDriver for FallbackDriver {
    fn platform_name(&self) -> &'static str {
        "fallback"
    }

    fn list_windows(&self, filter: Option<&str>) -> Result<Vec<WindowInfo>> {
        if !self.has_wmctrl {
            return Err(platform_err(
                self.platform_name(),
                format!(
                    "未检测到 wmctrl(窗口列举依赖),且当前平台无控件级操控能力。{UNSUPPORTED_MSG}"
                ),
            ));
        }
        // wmctrl -l -p 输出: <win_id> <desktop> <pid> <host> <title...>
        let out = Command::new("wmctrl")
            .args(["-l", "-p"])
            .output()
            .map_err(|e| platform_err(self.platform_name(), format!("执行 wmctrl 失败: {e}")))?;
        if !out.status.success() {
            return Err(platform_err(
                self.platform_name(),
                format!(
                    "wmctrl -l -p 退出码 {:?}: {}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut wins = Vec::new();
        for line in stdout.lines() {
            let mut parts = line.splitn(5, char::is_whitespace);
            let id = parts.next().unwrap_or_default().trim().to_string();
            let _desktop = parts.next();
            let pid: u32 = parts
                .next()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            let _host = parts.next();
            let title = parts.next().unwrap_or_default().trim().to_string();
            if id.is_empty() {
                continue;
            }
            if !matches_filter(&title, filter) {
                continue;
            }
            wins.push(WindowInfo {
                id,
                title,
                process_name: String::new(),
                pid,
                bounds: Rect::default(),
                cg_window_id: None,
                hwnd: None,
                wmctrl_id: None,
            });
        }
        Ok(wins)
    }

    fn inspect(
        &self,
        _window_id: &str,
        _max_depth: usize,
        _filter: Option<&str>,
    ) -> Result<ControlNode> {
        Err(platform_err(self.platform_name(), UNSUPPORTED_MSG))
    }

    fn act(&self, window_id: &str, path: &str, action: ControlAction) -> Result<String> {
        // 唯一可尽力而为的操作:xdotool 聚焦窗口(path 必须为根 "/")
        if action == ControlAction::Focus && (path == "/" || path.is_empty()) && self.has_xdotool {
            let status = Command::new("xdotool")
                .args(["windowactivate", window_id])
                .status()
                .map_err(|e| {
                    platform_err(self.platform_name(), format!("执行 xdotool 失败: {e}"))
                })?;
            if status.success() {
                return Ok(format!("已聚焦窗口 {window_id}"));
            }
            return Err(platform_err(
                self.platform_name(),
                format!(
                    "xdotool windowactivate {window_id} 退出码 {:?}",
                    status.code()
                ),
            ));
        }
        // 2026-09-16 第 66 轮:xdotool 滚轮滚动(click 4=向上 / click 5=向下,
        // 先激活窗口再逐行投递;scroll_to_visible 无通用实现,回退小步 scroll)。
        let scroll_lines = match &action {
            ControlAction::Scroll { lines } => Some(*lines),
            ControlAction::ScrollToVisible => Some(-3),
            _ => None,
        };
        if let Some(lines) = scroll_lines {
            if !self.has_xdotool {
                return Err(platform_err(
                    self.platform_name(),
                    "scroll 需要 xdotool(可 sudo apt install xdotool)",
                ));
            }
            let _ = Command::new("xdotool")
                .args(["windowactivate", window_id])
                .status();
            let button = if lines > 0 { "4" } else { "5" };
            for _ in 0..lines.abs() {
                let status = Command::new("xdotool")
                    .args(["click", button])
                    .status()
                    .map_err(|e| {
                        platform_err(self.platform_name(), format!("执行 xdotool 失败: {e}"))
                    })?;
                if !status.success() {
                    return Err(platform_err(
                        self.platform_name(),
                        format!("xdotool click {button} 退出码 {:?}", status.code()),
                    ));
                }
            }
            return Ok(format!(
                "已在窗口 {window_id} 滚动 {} 行({})",
                lines.abs(),
                if lines > 0 { "向上" } else { "向下" }
            ));
        }
        Err(platform_err(self.platform_name(), UNSUPPORTED_MSG))
    }

    fn permission_hint(&self) -> Option<String> {
        if self.has_wmctrl || self.has_xdotool {
            None
        } else {
            Some(format!(
                "未检测到 wmctrl / xdotool(可 sudo apt install wmctrl xdotool 获得基础窗口列举/聚焦能力);\
                 控件级操控 {UNSUPPORTED_MSG}"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_inspect_is_structured_error() {
        let d = FallbackDriver {
            has_wmctrl: false,
            has_xdotool: false,
        };
        let err = d.inspect("0x1", 3, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("不支持控件级窗口操控"), "实际: {msg}");
        assert!(msg.contains("Windows"), "实际: {msg}");
        assert!(msg.contains("macOS"), "实际: {msg}");
    }

    #[test]
    fn list_windows_without_wmctrl_errors() {
        let d = FallbackDriver {
            has_wmctrl: false,
            has_xdotool: false,
        };
        let err = d.list_windows(None).unwrap_err();
        assert!(err.to_string().contains("wmctrl"));
    }

    #[test]
    fn permission_hint_mentions_tools_when_missing() {
        let d = FallbackDriver {
            has_wmctrl: false,
            has_xdotool: false,
        };
        let hint = d.permission_hint().expect("缺依赖时应给出引导");
        assert!(hint.contains("wmctrl") && hint.contains("xdotool"));
    }
}
