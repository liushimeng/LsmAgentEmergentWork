//! 窗口视觉工具(2026-09-16 第 67 轮):WindowOCR / WindowScreenshot。
//!
//! 背景:微信 4.x 等自绘 UI(MMUIRenderSubWindow)的 UIA 控件树为空,控件树路线
//! 结构性失效 —— 本模块补齐「视觉路线」:
//! - **WindowOCR**:GDI 截窗口 → Windows.Media.Ocr(系统内置离线引擎)→ 词级
//!   `{text, x, y, width, height}` + 换算好的屏幕绝对坐标(`screen_x/screen_y`),
//!   LLM 直接拿中心坐标调 `WindowAction click_point / type_text`;
//! - **WindowScreenshot**:跨平台截图落盘(Windows 第 67 轮起改为纯 Rust GDI,
//!   去 PowerShell 依赖;macOS/Linux 维持系统命令)。
//!
//! 设计见 `tmpPlan/2026-09-16_02-WindowUse第67轮全链路强化与微信4x视觉路线方案.md`。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Tool;
use super::window::{get_str, require_str, tool_err};
use crate::agent::window::{current_driver, Rect};
use crate::error::Result;

// ===================== WindowOCR =====================

/// OCR 词数返回上限。
const MAX_OCR_BLOCKS: usize = 200;

/// 对窗口区域做 OCR,返回带坐标的词块(视觉路线的「读界面」入口)。
pub struct WindowOCRTool;

#[async_trait]
impl Tool for WindowOCRTool {
    fn name(&self) -> &str {
        "WindowOCR"
    }

    fn description(&self) -> &str {
        "对指定窗口做 OCR 文字识别,返回带坐标的词块(视觉路线入口,适合微信 4.x 等控件树为空的自绘 UI)。\n\
         - window_id 必填(WindowOpen/WindowFind 拿到);\n\
         - region 可选:{\"x\":N,\"y\":N,\"width\":N,\"height\":N} 窗口相对坐标,只识别局部(坐标原点=窗口左上);\n\
         - lang 可选:BCP-47 如 zh-Hans-CN / en-US,默认取系统配置语言。\n\
         返回 blocks:每块含 text + 窗口相对坐标(x,y,width,height)+ **屏幕绝对坐标**\n\
         (screen_x/screen_y,中心点 screen_cx/screen_cy)—— 后者直接作为\n\
         WindowAction click_point / scroll_point 的 x/y 参数。\n\
         用法:找按钮/菜单文字 → click 到其中心;找列表项 → click 或滚动后重 OCR;\n\
         验证结果 → 操作后重 OCR 确认文本出现。截图识别是近似值,点击尽量用词块中心。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "window_id": { "type": "string", "description": "窗口 id" },
                "region": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "integer" },
                        "y": { "type": "integer" },
                        "width": { "type": "integer" },
                        "height": { "type": "integer" }
                    },
                    "required": ["x", "y", "width", "height"],
                    "description": "可选:窗口相对识别区域(物理像素)"
                },
                "lang": { "type": "string", "description": "可选:BCP-47 语言标签,默认系统语言" }
            },
            "required": ["window_id"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let window_id = require_str(&args, "window_id", self.name())?.to_string();
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
        super::window::run_blocking(self.name(), move || {
            let driver = current_driver();
            let blocks = driver.ocr(&window_id, region, lang.as_deref())?;
            // 找窗口原点换算屏幕绝对坐标(list_windows 按 id 精确回查)
            let all = driver.list_windows(None)?;
            let win = all.iter().find(|w| w.id == window_id);
            let origin = win.map(|w| (w.bounds.x, w.bounds.y, w.bounds.width, w.bounds.height));
            let blocks_json: Vec<Value> = blocks
                .iter()
                .take(MAX_OCR_BLOCKS)
                .map(|b| {
                    let (cx, cy) = (b.x + b.width / 2, b.y + b.height / 2);
                    let mut obj = json!({
                        "text": b.text,
                        "x": b.x,
                        "y": b.y,
                        "width": b.width,
                        "height": b.height,
                        "screen_cx": origin.map(|(ox, oy, _, _)| ox + cx),
                        "screen_cy": origin.map(|(ox, oy, _, _)| oy + cy),
                    });
                    if let Some((ox, oy, _, _)) = origin {
                        obj["screen_x"] = json!(ox + b.x);
                        obj["screen_y"] = json!(oy + b.y);
                    }
                    obj
                })
                .collect();
            let body = json!({
                "window_id": window_id,
                "window_bounds": origin.map(|(x, y, w, h)| json!({"x": x, "y": y, "width": w, "height": h})),
                "lang": lang,
                "block_count": blocks.len(),
                "truncated": blocks.len() > MAX_OCR_BLOCKS,
                "blocks": blocks_json,
                "next_action": "找到目标文字块后,用其 screen_cx/screen_cy 调 WindowAction(action=click_point, x, y);\
                                输入文本先 click_point 输入框,再 action=type_text;完成后重新 WindowOCR 验证。"
            });
            Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
        })
        .await
    }
}

// ===================== WindowScreenshot =====================
//
// 2026-09-16 第 56 轮:WindowUse 工具集新增 WindowScreenshot。
// 2026-09-16 第 67 轮:Windows 路径改为纯 Rust GDI 截图(去 PowerShell 依赖),
// macOS/Linux 维持系统命令(白名单内)。
//
// 参数:
// - output_path:可选,默认临时目录 `laew_screenshot_<时间戳>.png`;目录不存在自动 mkdir。
// - region:可选 JSON {x,y,width,height},**窗口相对坐标**(物理像素)。
// 返回 JSON {"path":"...","size_bytes":N,"created_at":"..."}。

/// 平台默认截图命令(macOS / Linux;Windows 走纯 Rust GDI,不经过本函数)。
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

/// 落盘截图。
pub struct WindowScreenshotTool;

#[async_trait]
impl Tool for WindowScreenshotTool {
    fn name(&self) -> &str {
        "WindowScreenshot"
    }

    fn description(&self) -> &str {
        "截图落盘,返回 PNG 文件路径(为后续 OCR / 视觉验证铺路;一般场景优先 WindowOCR 直接拿文本+坐标)。\n\
         - output_path 可选:默认临时目录 laew_screenshot_<ts>.png;目录不存在自动 mkdir。\n\
         - region 可选:{\"x\":N,\"y\":N,\"width\":N,\"height\":N} 窗口相对区域(物理像素)。\n\
         平台差异(自动选用):Windows 纯 Rust GDI 截图 / macOS screencapture / Linux import(尽力而为)。\n\
         \n\
         【⚠️ 重要提示】\n\
         1) 本工具只产出 PNG 文件,不做 OCR / 视觉识别;\n\
         2) **禁止**使用 Read 工具读取 PNG(Read 仅支持 UTF-8 文本,二进制会失败);\n\
         3) 需要识别界面文字时直接用 WindowOCR(返回文本+坐标),不要先截图再读图。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "window_id": { "type": "string", "description": "目标窗口 id(Windows 必填:按窗口区域截图)" },
                "output_path": { "type": "string", "description": "输出 PNG 路径,默认临时目录" },
                "region": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "integer" },
                        "y": { "type": "integer" },
                        "width": { "type": "integer" },
                        "height": { "type": "integer" }
                    },
                    "required": ["x", "y", "width", "height"],
                    "description": "可选截图区域(窗口相对坐标)"
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
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

        // Windows:纯 Rust GDI 截图(第 67 轮,去 PowerShell 依赖)
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
            super::window::run_blocking(self.name(), move || {
                // 需要窗口句柄:window_id 必填(Windows 截图按窗口区域)
                let wid = window_id.clone().ok_or_else(|| {
                    tool_err(
                        "WindowScreenshot",
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
                    "next_action": "PNG 已落盘;需要识别文字直接用 WindowOCR,禁止 Read PNG。"
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
                        tool_err(self.name(), format!("创建父目录 {parent:?} 失败: {e}"))
                    })?;
                }
            }

            // 2026-09-17 第 74 轮 T2:优先走原生 CGWindow 截图(macOS)
            // 只需辅助功能权限,无需屏幕录制权限(screencapture 需要)
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
                    match driver.screenshot_to(wid, region_rect, path) {
                        Ok(_) => {
                            let meta = std::fs::metadata(&output_path).map_err(|e| {
                                tool_err(
                                    self.name(),
                                    format!("截图未生成: {e}"),
                                )
                            })?;
                            let body = json!({
                                "path": output_path,
                                "size_bytes": meta.len(),
                                "created_at_unix": ts,
                                "platform": std::env::consts::OS,
                                "method": "cgwindow_native",
                                "next_action": "PNG 已落盘(CGWindow 原生截图,无需屏幕录制权限);需要识别文字直接用 WindowOCR,禁止 Read PNG。"
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
                    self.name(),
                    format!("截图未生成: {};命令输出={}", e, output),
                )
            })?;
            let body = json!({
                "path": output_path,
                "size_bytes": meta.len(),
                "created_at_unix": ts,
                "platform": std::env::consts::OS,
                "command": command,
                "next_action": "PNG 已落盘;需要识别文字直接用 WindowOCR,禁止 Read PNG。"
            });
            Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn default_screenshot_command_macos() {
        if cfg!(target_os = "macos") {
            let cmd = super::default_screenshot_command("/tmp/x.png", None);
            assert!(cmd.contains("screencapture"));
            assert!(cmd.contains("/tmp/x.png"));
            let cmd2 = super::default_screenshot_command(
                "/tmp/x.png",
                Some(&serde_json::json!({"x":10,"y":20,"width":100,"height":200})),
            );
            assert!(cmd2.contains("10,20,100,200"));
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn default_screenshot_command_linux() {
        if cfg!(not(any(target_os = "macos", windows))) {
            let cmd = super::default_screenshot_command("/tmp/x.png", None);
            assert!(!cmd.is_empty());
        }
    }
}
