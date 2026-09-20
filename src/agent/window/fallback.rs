//! Fallback 窗口驱动(Linux / 其他非 Windows/macOS 平台)。
//!
//! 尽力而为策略:
//! - 检测到 `wmctrl` 时,`wmctrl -l -p` 可列举窗口(id/标题/PID);
//! - 检测到 `xdotool` 时,`windowactivate` 可聚焦窗口;
//! - 控件级 `inspect` / `act` 无跨桌面环境统一 API(X11/Wayland 分裂,
//!   AT-SPI 覆盖率差),一律返回结构化「平台不支持」错误,fail-closed,
//!   让 QC 判定失败并回流 Yolo 给出「请在 Windows/macOS 执行」的用户建议。
//!
//! 设计见 `docs/MCP_Window_Use/01-设计与解决方案.md` §2.3 Fallback 后端。

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

/// xdotool 单命令执行(失败转结构化错误)。
fn xdotool(args: &[&str]) -> Result<()> {
    let status = Command::new("xdotool")
        .args(args)
        .status()
        .map_err(|e| platform_err("fallback", format!("执行 xdotool 失败: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(platform_err(
            "fallback",
            format!("xdotool {} 退出码 {:?}", args.join(" "), status.code()),
        ))
    }
}

/// 修饰键规格 → xdotool 键名序列(第 90 轮;空规格 → 空序列)。
fn xdotool_mod_names(spec: Option<&str>) -> Result<Vec<&str>> {
    let Some(s) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for seg in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        let name = match seg.to_lowercase().as_str() {
            "ctrl" | "control" => "ctrl",
            "shift" => "shift",
            "alt" | "option" | "opt" => "alt",
            "win" | "meta" | "cmd" | "command" => "super",
            other => {
                return Err(platform_err(
                    "fallback",
                    format!("modifiers 段无法识别: {other:?}(仅允许 ctrl/shift/alt/win 组合)"),
                ))
            }
        };
        out.push(name);
    }
    Ok(out)
}

impl FallbackDriver {
    /// 第 90 轮:坐标/鼠标原语(xdotool)。返回 Ok(Some(文案)) 表示已处理;
    /// Ok(None) 表示该动作不是坐标原语,交回上层原有链路(scroll/type_text/...)。
    ///
    /// 支持:click_point(含 modifiers)/ double_click_point / right_click_point /
    /// middle_click_point / move_point / drag_point / scroll_point。
    fn act_point_action(&self, action: &ControlAction) -> Result<Option<String>> {
        let need_xdotool = || -> Result<()> {
            if self.has_xdotool {
                Ok(())
            } else {
                Err(platform_err(
                    self.platform_name(),
                    "坐标鼠标动作需要 xdotool(可 sudo apt install xdotool)",
                ))
            }
        };
        // 修饰键包裹:keydown 序列 → 动作 → keyup 逆序
        let with_mods = |mods: &[&str], body: &[&str]| -> Result<()> {
            for m in mods {
                xdotool(&["keydown", m])?;
            }
            let r = xdotool(body);
            for m in mods.iter().rev() {
                let _ = xdotool(&["keyup", m]);
            }
            r
        };
        match action {
            ControlAction::ClickPoint {
                x,
                y,
                modifiers,
            } => {
                need_xdotool()?;
                let mods = xdotool_mod_names(modifiers.as_deref())?;
                let xs = x.to_string();
                let ys = y.to_string();
                with_mods(&mods, &["mousemove", &xs, &ys])?;
                xdotool(&["click", "1"])?;
                Ok(Some(format!(
                    "已在 ({x},{y}) 执行左键单击(xdotool,route=physical{})",
                    modifiers
                        .as_deref()
                        .map(|m| format!(" + 按住 {m}"))
                        .unwrap_or_default()
                )))
            }
            ControlAction::DoubleClickPoint {
                x,
                y,
                modifiers,
            } => {
                need_xdotool()?;
                let mods = xdotool_mod_names(modifiers.as_deref())?;
                let xs = x.to_string();
                let ys = y.to_string();
                with_mods(&mods, &["mousemove", &xs, &ys])?;
                xdotool(&["click", "--repeat", "2", "--delay", "80", "1"])?;
                Ok(Some(format!(
                    "已在 ({x},{y}) 执行双击(xdotool,route=physical{})",
                    modifiers
                        .as_deref()
                        .map(|m| format!(" + 按住 {m}"))
                        .unwrap_or_default()
                )))
            }
            ControlAction::RightClickPoint {
                x,
                y,
                modifiers,
            } => {
                need_xdotool()?;
                let mods = xdotool_mod_names(modifiers.as_deref())?;
                let xs = x.to_string();
                let ys = y.to_string();
                with_mods(&mods, &["mousemove", &xs, &ys])?;
                xdotool(&["click", "3"])?;
                Ok(Some(format!(
                    "已在 ({x},{y}) 执行右键单击(xdotool,route=physical{})",
                    modifiers
                        .as_deref()
                        .map(|m| format!(" + 按住 {m}"))
                        .unwrap_or_default()
                )))
            }
            ControlAction::MiddleClickPoint { x, y, modifiers } => {
                need_xdotool()?;
                let mods = xdotool_mod_names(modifiers.as_deref())?;
                let xs = x.to_string();
                let ys = y.to_string();
                // 修饰键按住 → 移到目标 → 中键点击(中键在修饰键仍按住时投递)
                for m in &mods {
                    xdotool(&["keydown", m])?;
                }
                let r = (|| -> Result<()> {
                    xdotool(&["mousemove", &xs, &ys])?;
                    xdotool(&["click", "2"])
                })();
                for m in mods.iter().rev() {
                    let _ = xdotool(&["keyup", m]);
                }
                r?;
                Ok(Some(format!(
                    "已在 ({x},{y}) 执行中键单击(xdotool,route=physical{})",
                    modifiers
                        .as_deref()
                        .map(|m| format!(" + 按住 {m}"))
                        .unwrap_or_default()
                )))
            }
            ControlAction::MovePoint { x, y } => {
                need_xdotool()?;
                let xs = x.to_string();
                let ys = y.to_string();
                xdotool(&["mousemove", &xs, &ys])?;
                Ok(Some(format!("已把光标移动到 ({x},{y})(悬停,xdotool)")))
            }
            ControlAction::DragPoint {
                x,
                y,
                x2,
                y2,
                modifiers,
            } => {
                need_xdotool()?;
                let mods = xdotool_mod_names(modifiers.as_deref())?;
                let fx = x.to_string();
                let fy = y.to_string();
                let tx = x2.to_string();
                let ty = y2.to_string();
                for m in &mods {
                    xdotool(&["keydown", m])?;
                }
                let r = (|| -> Result<()> {
                    xdotool(&["mousemove", &fx, &fy])?;
                    xdotool(&["mousedown", "1"])?;
                    xdotool(&["mousemove", "--sync", &tx, &ty])?;
                    xdotool(&["mouseup", "1"])
                })();
                for m in mods.iter().rev() {
                    let _ = xdotool(&["keyup", m]);
                }
                r?;
                Ok(Some(format!(
                    "已从 ({x},{y}) 拖拽到 ({x2},{y2})(xdotool,route=physical{})",
                    modifiers
                        .as_deref()
                        .map(|m| format!(" + 按住 {m}"))
                        .unwrap_or_default()
                )))
            }
            ControlAction::ScrollPoint { x, y, lines } => {
                need_xdotool()?;
                let xs = x.to_string();
                let ys = y.to_string();
                xdotool(&["mousemove", &xs, &ys])?;
                // click 4=向上 / 5=向下(x11 鼠标键约定)
                let button = if *lines > 0 { "4" } else { "5" };
                for _ in 0..lines.abs().min(100) {
                    xdotool(&["click", button])?;
                }
                Ok(Some(format!(
                    "已在 ({x},{y}) 滚动 {} 行({},route=physical)",
                    lines.abs(),
                    if *lines > 0 { "向上" } else { "向下" }
                )))
            }
            _ => Ok(None),
        }
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
        // ===== 2026-09-19 第 90 轮:鼠标键盘原子能力(xdotool 尽力而为) =====
        // 坐标动作不依赖控件树(自绘 UI 视觉路线),Linux 走 xdotool 物理注入等价物。
        if let Some(done) = self.act_point_action(&action)? {
            return Ok(done);
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
        // 2026-09-17 第 80 轮:Linux 键入/原子提交走 xdotool,与其他平台的
        // SendInput/CGEvent 语义对齐。仍只是 X11/XWayland 下尽力而为。
        let typed = match &action {
            ControlAction::TypeText(text) => Some((text, None, false)),
            ControlAction::TypeTextSubmit { text, x, y } => {
                let point = match (x, y) {
                    (Some(x), Some(y)) => Some((*x, *y)),
                    _ => None,
                };
                Some((text, point, true))
            }
            _ => None,
        };
        if let Some((text, point, submit)) = typed {
            if !self.has_xdotool {
                return Err(platform_err(
                    self.platform_name(),
                    "type_text/type_text_submit 需要 xdotool(可 sudo apt install xdotool)",
                ));
            }
            let _ = Command::new("xdotool")
                .args(["windowactivate", window_id])
                .status();
            if let Some((x, y)) = point {
                for args in [
                    vec!["mousemove".to_string(), x.to_string(), y.to_string()],
                    vec!["click".to_string(), "1".to_string()],
                ] {
                    let status = Command::new("xdotool").args(args).status().map_err(|e| {
                        platform_err(self.platform_name(), format!("执行 xdotool 失败: {e}"))
                    })?;
                    if !status.success() {
                        return Err(platform_err(
                            self.platform_name(),
                            format!("xdotool 坐标定位退出码 {:?}", status.code()),
                        ));
                    }
                }
            }
            let status = Command::new("xdotool")
                .args(["type", "--delay", "25", "--", text])
                .status()
                .map_err(|e| {
                    platform_err(self.platform_name(), format!("执行 xdotool type 失败: {e}"))
                })?;
            if !status.success() {
                return Err(platform_err(
                    self.platform_name(),
                    format!("xdotool type 退出码 {:?}", status.code()),
                ));
            }
            if submit {
                std::thread::sleep(std::time::Duration::from_millis(120));
                let status = Command::new("xdotool")
                    .args(["key", "--clearmodifiers", "Return"])
                    .status()
                    .map_err(|e| {
                        platform_err(self.platform_name(), format!("执行 xdotool key 失败: {e}"))
                    })?;
                if !status.success() {
                    return Err(platform_err(
                        self.platform_name(),
                        format!("xdotool Return 退出码 {:?}", status.code()),
                    ));
                }
            }
            return Ok(format!(
                "已键入 {} 字符{}",
                text.chars().count(),
                if submit { "并提交" } else { "" }
            ));
        }
        Err(platform_err(self.platform_name(), UNSUPPORTED_MSG))
    }

    // 第 81 轮:已前台 → 跳过激活。xdotool getactivewindow 返回十进制窗口 id,
    // wmctrl id 是 0x 十六进制,统一成 u64 对比;失败一律 false(保持旧行为)。
    fn is_frontmost(&self, window_id: &str) -> bool {
        if !self.has_xdotool {
            return false;
        }
        let Ok(target) = u64::from_str_radix(window_id.trim().trim_start_matches("0x"), 16) else {
            return false;
        };
        let Ok(out) = Command::new("xdotool").arg("getactivewindow").output() else {
            return false;
        };
        if !out.status.success() {
            return false;
        }
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .ok()
            .map(|active| active == target)
            .unwrap_or(false)
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
