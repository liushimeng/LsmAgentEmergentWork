//! macOS Vision OCR 引擎(2026-09-17 第 74 轮 T1,第 86 轮修正)
//!
//! 基于 Vision.framework 的 VNRecognizeTextRequest:
//! 1. CGWindowListCreateImage 截取窗口图像 —— **macOS 26.5 实测需要屏幕录制权限**;
//!    第 86 轮修正了之前「无需屏幕录制」的错误注释。屏录未授权时 CGWindow 返回
//!    null,工具层会自动降级到 screencapture 命令(screencapture -x 同样需屏录,
//!    最终全失败,LLM 应改走 osascript_fallback 路线)。
//! 2. 转为 CGImage → Vision 处理
//! 3. VNRecognizeTextRequest 识别文字 + 坐标
//! 4. 返回 OcrBlock {text, x, y, width, height}(窗口相对坐标)
//!
//! 与 Windows 后端(Windows.Media.Ocr)对齐:同样返回窗口相对坐标 + 置信度,
//! 工具层(screen_cx/screen_y 换算)复用,无需改动。

use std::ffi::c_void;
use std::path::Path;

use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation::number::{CFNumberGetValue, CFNumberRef};
use core_foundation::string::CFString;
use core_graphics::display::CGRectNull;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use foreign_types::ForeignType;

use crate::error::{AgentError, Result};

// ===================== 公共类型 =====================

/// OCR 词块(窗口相对坐标,物理像素)。
#[derive(Debug, Clone)]
pub struct VisionOcrBlock {
    pub text: String,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub confidence: f32,
}

/// Vision OCR 配置。
#[derive(Debug, Clone)]
pub struct VisionOcrConfig {
    /// 识别精度(0=fast, 1=accurate)。
    pub recognition_level: i32,
    /// 语言列表(BCP-47,如 ["zh-Hans","en"])。
    pub languages: Vec<String>,
    /// 是否启用语言校正。
    pub language_correction: bool,
    /// 最低置信度阈值(0-1)。
    pub min_confidence: f32,
}

impl Default for VisionOcrConfig {
    fn default() -> Self {
        Self {
            recognition_level: 1, // accurate
            languages: vec!["zh-Hans".into(), "zh-Hant".into(), "en".into()],
            language_correction: true,
            min_confidence: 0.3,
        }
    }
}

// ===================== 公共接口 =====================

/// 对指定窗口做 OCR 识别。
///
/// # 参数
/// - `window_id`: CGWindowID(u32)
/// - `region`: 窗口相对区域(None = 整个窗口)
/// - `config`: OCR 配置(None = 默认)
///
/// # 返回
/// 识别到的词块列表(窗口相对坐标)。
pub fn ocr_window(
    window_id: u32,
    region: Option<(i64, i64, i64, i64)>,
    config: Option<VisionOcrConfig>,
) -> Result<Vec<VisionOcrBlock>> {
    let cfg = config.unwrap_or_default();

    // 1. 截取窗口图像
    let cg_image = capture_window_image(window_id, region)?;

    // 2. 调用 Vision OCR
    let blocks = vision_ocr_on_cgimage(&cg_image, &cfg)?;

    Ok(blocks)
}

/// 对 PNG 文件直接 OCR(第 99 轮:供 MCP_Web_Use 截图链路复用)。
///
/// 与 [`ocr_window`] 的差异:输入已是磁盘上的 PNG(浏览器 CDP 截图字节落盘),
/// 不触碰 CGWindowListCreateImage —— **无需屏幕录制权限**;输出同为词块列表
/// (图像左上角原点坐标),与窗口 OCR 工具层语义一致。
/// 输出临时 JSON 加时间戳防同进程并发覆盖。
pub fn ocr_png_file(
    input_path: &Path,
    config: Option<VisionOcrConfig>,
) -> Result<Vec<VisionOcrBlock>> {
    let cfg = config.unwrap_or_default();
    let output_path = std::env::temp_dir().join(format!(
        "laew_ocr_output_{}_{}.json",
        std::process::id(),
        now_millis_safe()
    ));
    let result = call_vision_ocr_helper(input_path, &output_path, &cfg);
    let _ = std::fs::remove_file(&output_path);
    result
}

fn now_millis_safe() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

/// 对指定窗口截图并保存到文件(2026-09-17 第 74 轮 T2)。
///
/// 使用 CGWindowListCreateImage;**第 86/87 轮实测需要屏幕录制授权**
/// (未授权时按窗口截取返回 null,此前「无需屏幕录制」注释为误写)。
pub fn screenshot_window(
    window_id: u32,
    region: Option<(i64, i64, i64, i64)>,
    output_path: &Path,
) -> Result<()> {
    // 1. 截取窗口图像
    let cg_image = capture_window_image(window_id, region)?;

    // 2. 保存为 PNG
    save_cgimage_as_png(&cg_image, output_path)
}

/// 全屏截图(无需窗口 ID)。
pub fn screenshot_fullscreen(region: Option<(i64, i64, i64, i64)>, output_path: &Path) -> Result<()> {
    let cg_image = capture_fullscreen_image(region)?;
    save_cgimage_as_png(&cg_image, output_path)
}

// ===================== 内部实现 =====================

/// 截取窗口图像(CGWindowListCreateImage)。
fn capture_window_image(
    window_id: u32,
    region: Option<(i64, i64, i64, i64)>,
) -> Result<CGImage> {
    let rect = match region {
        Some((x, y, w, h)) => CGRect::new(
            &CGPoint::new(x as f64, y as f64),
            &CGSize::new(w as f64, h as f64),
        ),
        None => unsafe { CGRectNull },
    };

    let cg_image = unsafe {
        core_graphics::window::CGWindowListCreateImage(
            rect,
            core_graphics::window::kCGWindowListOptionIncludingWindow,
            window_id,
            core_graphics::window::kCGWindowImageBoundsIgnoreFraming
                | core_graphics::window::kCGWindowImageShouldBeOpaque,
        )
    };

    if cg_image.is_null() {
        // 2026-09-18 第 87 轮:错误归因修正 —— 按窗口截取返回 null 的首要原因是
        // **屏幕录制未授权**(macOS 26.5 实测;辅助功能授权与此无关),而非辅助功能。
        return Err(AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=screenshot)".into(),
            reason: format!(
                "CGWindowListCreateImage 返回 null (window_id={window_id}):\
                 屏幕录制未授权(macOS 按窗口截取走 TCC 屏录门控),或窗口已失效。\
                 处置:1) 禁止继续重试 ocr/screenshot;2) 改走 action=inspect 控件树路线,\
                 或 action=chat_send(osascript_fallback 路线,不依赖截图);\
                 3) 如需视觉路线:系统设置 → 隐私与安全性 → 屏幕录制 → 勾选宿主终端后重开终端"
            ),
        });
    }

    // 从 *mut CGImage 转为 CGImage (ForeignType trait)
    let image = unsafe { CGImage::from_ptr(cg_image) };
    Ok(image)
}

/// 全屏截图。
fn capture_fullscreen_image(region: Option<(i64, i64, i64, i64)>) -> Result<CGImage> {
    let rect = match region {
        Some((x, y, w, h)) => CGRect::new(
            &CGPoint::new(x as f64, y as f64),
            &CGSize::new(w as f64, h as f64),
        ),
        None => unsafe { CGRectNull },
    };

    let cg_image = unsafe {
        core_graphics::window::CGWindowListCreateImage(
            rect,
            core_graphics::window::kCGWindowListOptionAll,
            0,
            core_graphics::window::kCGWindowImageBoundsIgnoreFraming,
        )
    };

    if cg_image.is_null() {
        return Err(AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=screenshot)".into(),
            reason: "全屏截图失败:CGWindowListCreateImage 返回 null".into(),
        });
    }

    let image = unsafe { CGImage::from_ptr(cg_image) };
    Ok(image)
}

/// 保存 CGImage 为 PNG 文件。
fn save_cgimage_as_png(cg_image: &CGImage, output_path: &Path) -> Result<()> {
    // 确保父目录存在
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| AgentError::ToolExecution {
                tool: "MCP_Window_Use(action=screenshot)".into(),
                reason: format!("创建父目录 {parent:?} 失败: {e}"),
            })?;
        }
    }

    let width = cg_image.width();
    let height = cg_image.height();
    let data = cg_image.data();
    let bytes = data.bytes();

    // 转换为 RGBA
    let mut rgba_bytes: Vec<u8> = Vec::with_capacity(width * height * 4);
    let bytes_per_row = cg_image.bytes_per_row();
    let bits_per_pixel = cg_image.bits_per_pixel();
    let bytes_per_pixel = bits_per_pixel / 8;

    for y in 0..height {
        let row_start = y * bytes_per_row;
        for x in 0..width {
            let pixel_start = row_start + x * bytes_per_pixel;
            if pixel_start + 3 < bytes.len() {
                rgba_bytes.push(bytes[pixel_start]); // R
                rgba_bytes.push(bytes[pixel_start + 1]); // G
                rgba_bytes.push(bytes[pixel_start + 2]); // B
                rgba_bytes.push(bytes[pixel_start + 3]); // A
            }
        }
    }

    let image_buffer =
        image::RgbaImage::from_raw(width as u32, height as u32, rgba_bytes).ok_or_else(
            || AgentError::ToolExecution {
                tool: "MCP_Window_Use(action=screenshot)".into(),
                reason: "CGImage 数据转换失败".into(),
            },
        )?;

    image_buffer
        .save_with_format(output_path, image::ImageFormat::Png)
        .map_err(|e| AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=screenshot)".into(),
            reason: format!("PNG 编码失败: {e}"),
        })?;

    Ok(())
}

/// 在 CGImage 上执行 Vision OCR。
///
/// 使用 Vision.framework 的 VNRecognizeTextRequest。
fn vision_ocr_on_cgimage(cg_image: &CGImage, cfg: &VisionOcrConfig) -> Result<Vec<VisionOcrBlock>> {
    // 由于 Vision.framework 的 FFI 较复杂,这里使用一种更可靠的方案:
    // 1. 先把 CGImage 保存为临时 PNG
    // 2. 调用内置的 Swift 辅助工具进行识别
    // 3. 解析返回的 JSON 结果

    // 临时文件路径
    let tmp_dir = std::env::temp_dir();
    let screenshot_path = tmp_dir.join(format!("laew_ocr_input_{}.png", std::process::id()));
    let output_path = tmp_dir.join(format!("laew_ocr_output_{}.json", std::process::id()));

    // 1. 保存截图
    save_cgimage_as_png(cg_image, &screenshot_path)?;

    // 2. 调用 Vision OCR 辅助工具
    let result = call_vision_ocr_helper(&screenshot_path, &output_path, cfg);

    // 3. 清理临时文件
    let _ = std::fs::remove_file(&screenshot_path);
    let _ = std::fs::remove_file(&output_path);

    result
}

/// 调用 Vision OCR 辅助工具。
///
/// 辅助工具是一段 Swift 代码,使用 Vision.framework 进行 OCR。
/// 运行时写入临时文件并编译执行。
fn call_vision_ocr_helper(
    input_path: &Path,
    output_path: &Path,
    cfg: &VisionOcrConfig,
) -> Result<Vec<VisionOcrBlock>> {
    // 生成 Swift 脚本
    let swift_script = generate_ocr_swift_script(input_path, output_path, cfg);

    let script_path = std::env::temp_dir().join(format!("laew_ocr_script_{}.swift", std::process::id()));
    std::fs::write(&script_path, &swift_script).map_err(|e| AgentError::ToolExecution {
        tool: "MCP_Window_Use(action=ocr)".into(),
        reason: format!("写入 Swift 脚本失败: {e}"),
    })?;

    // 编译并执行
    let output = std::process::Command::new("swift")
        .arg(&script_path)
        .output()
        .map_err(|e| {
            // swift 未安装,使用降级方案
            AgentError::ToolExecution {
                tool: "MCP_Window_Use(action=ocr)".into(),
                reason: format!(
                    "调用 swift 失败: {}。\
                     macOS Vision OCR 需要安装 Xcode Command Line Tools,\
                     请运行 'xcode-select --install' 后重试。\
                     或使用 tesseract 替代: brew install tesseract tesseract-lang",
                    e
                ),
            }
        })?;

    // 清理脚本文件
    let _ = std::fs::remove_file(&script_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=ocr)".into(),
            reason: format!("Vision OCR 执行失败: {stderr}"),
        });
    }

    // 读取输出 JSON
    let json_str = std::fs::read_to_string(output_path).map_err(|e| AgentError::ToolExecution {
        tool: "MCP_Window_Use(action=ocr)".into(),
        reason: format!("读取 OCR 结果失败: {e}"),
    })?;

    // 解析 JSON
    parse_ocr_json_output(&json_str)
}

/// 生成 Vision OCR 的 Swift 脚本。
fn generate_ocr_swift_script(input_path: &Path, output_path: &Path, cfg: &VisionOcrConfig) -> String {
    let input_path_str = input_path.to_string_lossy();
    let output_path_str = output_path.to_string_lossy();
    let min_confidence = cfg.min_confidence;

    // 构建语言数组
    let langs_array = cfg
        .languages
        .iter()
        .map(|l| format!("\"{l}\""))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        r#"import Foundation
import Vision

// 读取输入图片
let inputPath = "{input_path_str}"
let outputPath = "{output_path_str}"

guard let imageData = NSData(contentsOfFile: inputPath),
      let image = NSImage(data: imageData as Data),
      let cgImage = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {{
    print("Error: Cannot load image from {{inputPath}}")
    exit(1)
}}

// 创建 OCR 请求
let request = VNRecognizeTextRequest {{ request, error in
    guard let observations = request.results as? [VNRecognizedTextObservation] else {{
        print("Error: No results")
        exit(1)
    }}

    var blocks: [[String: Any]] = []

    for observation in observations {{
        guard let topCandidate = observation.topCandidates(1).first else {{ continue }}
        let confidence = topCandidate.confidence

        if confidence < {min_confidence} {{ continue }}

        let text = topCandidate.string
        let bbox = observation.boundingBox

        // Vision 坐标系:原点在左下,y 轴向上
        // 转换为图像坐标系:原点在左上,y 轴向下
        let imgWidth = CGFloat(cgImage.width)
        let imgHeight = CGFloat(cgImage.height)

        let x = bbox.origin.x * imgWidth
        let y = (1.0 - bbox.origin.y - bbox.size.height) * imgHeight
        let width = bbox.size.width * imgWidth
        let height = bbox.size.height * imgHeight

        blocks.append([
            "text": text,
            "x": Int(x),
            "y": Int(y),
            "width": Int(width),
            "height": Int(height),
            "confidence": confidence
        ])
    }}

    // 写入输出文件
    let output: [String: Any] = [
        "success": true,
        "block_count": blocks.count,
        "blocks": blocks
    ]

    do {{
        let jsonData = try JSONSerialization.data(withJSONObject: output, options: .prettyPrinted)
        try jsonData.write(to: URL(fileURLWithPath: outputPath))
    }} catch {{
        print("Error writing output: {{error}}")
        exit(1)
    }}
}}

// 设置识别参数
request.recognitionLevel = {recognition_level}
request.recognitionLanguages = [{langs_array}]
request.usesLanguageCorrection = {language_correction}

// 执行 OCR
let handler = VNSequenceRequestHandler()
do {{
    try handler.perform([request], on: cgImage)
}} catch {{
    print("Error performing OCR: {{error}}")
    exit(1)
}}
"#,
        recognition_level = if cfg.recognition_level == 1 { "accurate" } else { "fast" },
        language_correction = if cfg.language_correction { "true" } else { "false" },
    )
}

/// 解析 OCR JSON 输出。
fn parse_ocr_json_output(json_str: &str) -> Result<Vec<VisionOcrBlock>> {
    let json: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=ocr)".into(),
            reason: format!("OCR 结果 JSON 解析失败: {e}"),
        })?;

    let blocks = json
        .get("blocks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=ocr)".into(),
            reason: "OCR 结果缺少 blocks 字段".into(),
        })?;

    let mut result = Vec::new();
    for block in blocks {
        let text = block
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let x = block.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
        let y = block.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
        let width = block.get("width").and_then(|v| v.as_i64()).unwrap_or(0);
        let height = block.get("height").and_then(|v| v.as_i64()).unwrap_or(0);
        let confidence = block
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;

        result.push(VisionOcrBlock {
            text,
            x,
            y,
            width,
            height,
            confidence,
        });
    }

    Ok(result)
}

// ===================== 窗口 ID 解析 =====================

/// 从 "pid:ax_idx" 格式提取 CGWindowID。
///
/// 注意:window_id 是进程级标识,需要转换为 CGWindowID 才能用于截图。
/// 这里通过 CGWindowListCopyWindowInfo 查找匹配的窗口。
pub fn resolve_cgwindow_id(pid: i32, title: Option<&str>) -> Result<u32> {
    unsafe {
        let list = core_graphics::window::CGWindowListCopyWindowInfo(
            core_graphics::window::kCGWindowListOptionOnScreenOnly
                | core_graphics::window::kCGWindowListExcludeDesktopElements,
            0,
        );
        // 避免 unused warning
        let _ = title;

        if list.is_null() {
            return Err(AgentError::ToolExecution {
                tool: "MCP_Window_Use(action=ocr)".into(),
                reason: "CGWindowListCopyWindowInfo 返回 null".into(),
            });
        }

        let count = CFArrayGetCount(list);
        let mut found_id: Option<u32> = None;

        for i in 0..count {
            let dict = CFArrayGetValueAtIndex(list, i) as CFDictionaryRef;
            if dict.is_null() {
                continue;
            }

            // 获取 kCGWindowOwnerPID
            let pid_key = CFString::new("kCGWindowOwnerPID");
            let pid_ref = CFDictionaryGetValue(dict, pid_key.as_concrete_TypeRef() as *const c_void);
            if !pid_ref.is_null() {
                let mut pid_val: i64 = 0;
                CFNumberGetValue(pid_ref as CFNumberRef, 4, &mut pid_val as *mut i64 as *mut c_void);

                if pid_val == pid as i64 {
                    // 如果指定了标题,进一步匹配
                    if let Some(expected_title) = title {
                        let title_key = CFString::new("kCGWindowName");
                        let title_ref =
                            CFDictionaryGetValue(dict, title_key.as_concrete_TypeRef() as *const c_void);
                        if !title_ref.is_null() {
                            let cf_string =
                                CFString::wrap_under_get_rule(title_ref as core_foundation::string::CFStringRef);
                            let window_title = cf_string.to_string();
                            if !window_title.contains(expected_title) {
                                continue;
                            }
                        }
                    }

                    // 获取 kCGWindowNumber
                    let id_key = CFString::new("kCGWindowNumber");
                    let id_ref = CFDictionaryGetValue(dict, id_key.as_concrete_TypeRef() as *const c_void);
                    if !id_ref.is_null() {
                        let mut id_val: i64 = 0;
                        CFNumberGetValue(id_ref as CFNumberRef, 4, &mut id_val as *mut i64 as *mut c_void);
                        found_id = Some(id_val as u32);
                        break;
                    }
                }
            }
        }

        CFRelease(list as CFTypeRef);

        found_id.ok_or_else(|| AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=ocr)".into(),
            reason: format!("未找到 pid={pid} 的 CGWindowID"),
        })
    }
}

// ===================== 测试 =====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_ocr_config_default() {
        let cfg = VisionOcrConfig::default();
        assert_eq!(cfg.recognition_level, 1);
        assert_eq!(cfg.languages.len(), 3);
        assert!(cfg.language_correction);
        assert!((cfg.min_confidence - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_ocr_json_output_valid() {
        let json_str = r#"{
            "success": true,
            "block_count": 2,
            "blocks": [
                {"text": "你好", "x": 100, "y": 200, "width": 50, "height": 20, "confidence": 0.95},
                {"text": "World", "x": 100, "y": 230, "width": 60, "height": 20, "confidence": 0.88}
            ]
        }"#;

        let blocks = parse_ocr_json_output(json_str).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, "你好");
        assert_eq!(blocks[0].x, 100);
        assert!((blocks[0].confidence - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_ocr_json_output_empty() {
        let json_str = r#"{"success": true, "block_count": 0, "blocks": []}"#;
        let blocks = parse_ocr_json_output(json_str).unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn parse_ocr_json_output_missing_field() {
        let json_str = r#"{"success": true}"#;
        let result = parse_ocr_json_output(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_cgwindow_id_invalid_pid() {
        // 测试无效 PID
        let result = resolve_cgwindow_id(999999, None);
        assert!(result.is_err());
    }
}
