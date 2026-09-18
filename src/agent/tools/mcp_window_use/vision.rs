//! MCP_Window_Use 视觉类 action(2026-09-18 第 84 轮自 tools/window_vision.rs 迁入):
//! `ocr` 与 `screenshot` —— 视觉路线。
//!
//! 背景:微信 4.x 等自绘 UI(MMUIRenderSubWindow)的 UIA 控件树为空,控件树路线
//! 结构性失效 —— 本模块补齐「视觉路线」:
//! - **ocr**:截窗口 → 系统 OCR(Windows.Media.Ocr / macOS Vision)→ 词级
//!   `{text, x, y, width, height}` + 换算好的屏幕绝对坐标(`screen_x/screen_y`),
//!   LLM 直接拿中心坐标调 `control(click_point / type_text)`;
//! - **screenshot**:跨平台截图落盘(Windows 纯 Rust GDI;macOS CGWindow 原生优先、
//!   screencapture 降级)。

use serde_json::{json, Value};

use super::{get_str, require_str, run_blocking, tool_err, Tool, MCP_WINDOW_USE_TOOL_NAME};
use crate::agent::window::{current_driver, Rect};
use crate::error::Result;

// ===================== action=ocr =====================

/// OCR 词数返回上限。
const MAX_OCR_BLOCKS: usize = 200;

/// 对窗口区域做 OCR,返回带坐标的词块(视觉路线的「读界面」入口)。
pub(super) async fn run_ocr(args: Value) -> Result<String> {
    let window_id = require_str(&args, "window_id", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let lang = get_str(&args, "lang").map(str::to_string);
    let region = args.get("region").and_then(|r| {
        let x = r.get("x").and_then(Value::as_i64)?;
        let y = r.get("y").and_then(Value::as_i64)?;
        let width = r.get("width").and_then(Value::as_i64)?;
        let height = r.get("height").and_then(Value::as_i64)?;
        if width <= 0 || height <= 0 {
            return None;
        }
        Some(Rect { x, y, width, height })
    });
    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
        let driver = current_driver();
        // 先 list_windows 拿到完整 WindowInfo(含 cg_window_id),再调
        // ocr_with_info —— 避免按 PID 匹配错窗口。
        let all = driver.list_windows(None)?;
        let info = all
            .iter()
            .find(|w| w.id == window_id)
            .cloned()
            .ok_or_else(|| {
                tool_err(
                    MCP_WINDOW_USE_TOOL_NAME,
                    format!(
                        "window_id={window_id} 不在当前可见窗口列表;可能已关闭/最小化,或 UI 已变化;请先 action=list 重新枚举"
                    ),
                )
            })?;
        let blocks = driver.ocr_with_info(&info, region, lang.as_deref())?;
        // 屏幕绝对坐标基于窗口 bounds 计算
        let origin = (info.bounds.x, info.bounds.y, info.bounds.width, info.bounds.height);
        let blocks_json: Vec<Value> = blocks
            .iter()
            .take(MAX_OCR_BLOCKS)
            .map(|b| {
                let (cx, cy) = (b.x + b.width / 2, b.y + b.height / 2);
                let (ox, oy, _, _) = origin;
                let mut obj = json!({
                    "text": b.text,
                    "x": b.x,
                    "y": b.y,
                    "width": b.width,
                    "height": b.height,
                    "screen_cx": ox + cx,
                    "screen_cy": oy + cy,
                });
                obj["screen_x"] = json!(ox + b.x);
                obj["screen_y"] = json!(oy + b.y);
                obj
            })
            .collect();
        let body = json!({
            "window_id": window_id,
            "cg_window_id": info.cg_window_id,
            "window_bounds": json!({"x": origin.0, "y": origin.1, "width": origin.2, "height": origin.3}),
            "lang": lang,
            "block_count": blocks.len(),
            "truncated": blocks.len() > MAX_OCR_BLOCKS,
            "blocks": blocks_json,
            "next_action": "找到目标文字块后,用其 screen_cx/screen_cy 调 action=control(control_action=click_point, x, y);\
                            输入文本先 click_point 输入框,再 control_action=type_text;完成后重新 action=ocr 验证。"
        });
        Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
    })
    .await
}

// ===================== action=screenshot =====================
//
// 参数:
// - output_path:可选,默认临时目录 `laew_screenshot_<时间戳>.png`;目录不存在自动 mkdir。
// - region:可选 JSON {x,y,width,height},**窗口相对坐标**(物理像素)。
// 返回 JSON {"path":"...","size_bytes":N,"created_at":"..."}。
//
// ⚠️ 重要提示:本 action 只产出 PNG 文件,不做 OCR / 视觉识别;禁止使用 Read 工具
// 读取 PNG(Read 仅支持 UTF-8 文本,二进制会失败);需要识别界面文字时直接用
// action=ocr(返回文本+坐标),不要先截图再读图。

/// 平台默认截图命令(macOS;Linux 尽力而为;Windows 走纯 Rust GDI,不经过本函数)。
#[cfg(target_os = "macos")]
fn default_screenshot_command(output_path: &str, region: Option<&Value>) -> String {
    if let Some(reg) = region {
        let x = reg.get("x").and_then(Value::as_i64).unwrap_or(0);
        let y = reg.get("y").and_then(Value::as_i64).unwrap_or(0);
        let w = reg.get("width").and_then(Value::as_i64).unwrap_or(0);
        let h = reg.get("height").and_then(Value::as_i64).unwrap_or(0);
        format!("screencapture -x -R{x},{y},{w},{h} -t png {output_path}")
    } else {
        format!("screencapture -x -t png {output_path}")
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn default_screenshot_command(_output_path: &str, _region: Option<&Value>) -> String {
    // Linux:ImageMagick import(常用);失败可改 scrot。
    "import -window root /tmp/laew_screen.png".to_string()
}

/// 截图落盘,返回 PNG 文件路径(一般场景优先 action=ocr 直接拿文本+坐标)。
pub(super) async fn run_screenshot(args: Value) -> Result<String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let default_path = std::env::temp_dir()
        .join(format!("laew_screenshot_{ts}.png"))
        .to_string_lossy()
        .to_string();
    let output_path = get_str(&args, "output_path")
        .map(str::to_string)
        .unwrap_or(default_path);
    let region = args.get("region").cloned();

    // Windows:纯 Rust GDI 截图(去 PowerShell 依赖)
    #[cfg(windows)]
    {
        let window_id = get_str(&args, "window_id").map(str::to_string);
        let region_rect = region.as_ref().and_then(|r| {
            let x = r.get("x").and_then(Value::as_i64)?;
            let y = r.get("y").and_then(Value::as_i64)?;
            let width = r.get("width").and_then(Value::as_i64)?;
            let height = r.get("height").and_then(Value::as_i64)?;
            if width <= 0 || height <= 0 {
                return None;
            }
            Some(Rect { x, y, width, height })
        });
        let output_path_clone = output_path.clone();
        run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
            // 需要窗口句柄:window_id 必填(Windows 截图按窗口区域)
            let wid = window_id.clone().ok_or_else(|| {
                tool_err(
                    MCP_WINDOW_USE_TOOL_NAME,
                    "Windows 平台截图需提供 window_id(窗口相对 region 基于它);\
                     全屏截图请把窗口 id 传桌面窗口或改用 region 传屏幕绝对区域",
                )
            })?;
            let hwnd = crate::agent::window::windows_driver_hwnd(&wid)?;
            let cap = crate::agent::window::windows_ocr_capture_to(
                hwnd,
                region_rect,
                std::path::Path::new(&output_path_clone),
            )?;
            let size = std::fs::metadata(&output_path_clone)
                .map(|m| m.len())
                .unwrap_or(0);
            let _ = cap;
            let body = json!({
                "path": output_path_clone,
                "size_bytes": size,
                "created_at_unix": ts,
                "platform": std::env::consts::OS,
                "next_action": "PNG 已落盘;需要识别文字直接用 action=ocr,禁止 Read PNG。"
            });
            Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
        })
        .await
    }
    #[cfg(not(windows))]
    {
        // 落盘前确保父目录存在。
        if let Some(parent) = std::path::Path::new(&output_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    tool_err(MCP_WINDOW_USE_TOOL_NAME, format!("创建父目录 {parent:?} 失败: {e}"))
                })?;
            }
        }

        // 优先走原生 CGWindow 截图(macOS):只需辅助功能权限,
        // 无需屏幕录制权限(screencapture 需要)。
        #[cfg(target_os = "macos")]
        {
            let window_id_str = get_str(&args, "window_id").map(str::to_string);
            let region_rect = region.as_ref().and_then(|r| {
                let x = r.get("x").and_then(Value::as_i64)?;
                let y = r.get("y").and_then(Value::as_i64)?;
                let width = r.get("width").and_then(Value::as_i64)?;
                let height = r.get("height").and_then(Value::as_i64)?;
                if width <= 0 || height <= 0 {
                    return None;
                }
                Some(crate::agent::window::Rect { x, y, width, height })
            });

            // 尝试原生截图(需要 window_id)
            if let Some(ref wid) = window_id_str {
                let driver = crate::agent::window::current_driver();
                let path = std::path::Path::new(&output_path);
                // 先 list_windows 拿完整 WindowInfo(含 cg_window_id),
                // 避免多窗口进程按 PID 匹配错位。
                let info_opt = driver
                    .list_windows(None)
                    .ok()
                    .and_then(|all| all.into_iter().find(|w| w.id == *wid));
                let screen_result = match info_opt {
                    Some(info) => {
                        driver.screenshot_to_with_info(&info, region_rect, path)
                    }
                    None => driver.screenshot_to(wid, region_rect, path),
                };
                match screen_result {
                    Ok(_) => {
                        let meta = std::fs::metadata(&output_path).map_err(|e| {
                            tool_err(
                                MCP_WINDOW_USE_TOOL_NAME,
                                format!("截图未生成: {e}"),
                            )
                        })?;
                        let body = json!({
                            "path": output_path,
                            "size_bytes": meta.len(),
                            "created_at_unix": ts,
                            "platform": std::env::consts::OS,
                            "method": "cgwindow_native",
                            "next_action": "PNG 已落盘(CGWindow 原生截图);需要识别文字直接用 action=ocr,禁止 Read PNG。\
                                            注意:CGWindow 原生截图实际仍需 macOS 屏幕录制授权(screen_recording=false 时 CGWindow 返回 null 走 screencapture 降级)。"
                        });
                        return Ok(serde_json::to_string_pretty(&body)
                            .unwrap_or_else(|_| "{}".into()));
                    }
                    Err(e) => {
                        // 原生截图失败,记录日志并降级到 screencapture
                        tracing::debug!(
                            error = %e,
                            "CGWindow 原生截图失败,降级到 screencapture"
                        );
                    }
                }
            }
        }

        // 降级方案:走 BashTool 执行 screencapture / import
        let command = default_screenshot_command(&output_path, region.as_ref());
        let bash = crate::agent::tools::bash::BashTool;
        let bash_args = json!({
            "command": command,
            "timeout_ms": 30000
        });
        let output = bash.execute(bash_args).await?;
        let meta = std::fs::metadata(&output_path).map_err(|e| {
            tool_err(
                MCP_WINDOW_USE_TOOL_NAME,
                format!("截图未生成: {};命令输出={}", e, output),
            )
        })?;
        let body = json!({
            "path": output_path,
            "size_bytes": meta.len(),
            "created_at_unix": ts,
            "platform": std::env::consts::OS,
            "command": command,
            "next_action": "PNG 已落盘;需要识别文字直接用 action=ocr,禁止 Read PNG。"
        });
        Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
    }
}
