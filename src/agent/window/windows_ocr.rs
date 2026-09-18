//! Windows OCR 管线(2026-09-16 第 67 轮,视觉路线核心)。
//!
//! 链路:GDI `BitBlt` 截窗口区域 → GDI+ 编码临时 PNG → WinRT
//! `StorageFile → BitmapDecoder → SoftwareBitmap → OcrEngine.RecognizeAsync`
//! → 词级 `{text, x, y, w, h}`(窗口相对物理像素)。
//!
//! 动机:微信 4.x 等自绘 UI 的 UIA 控件树为空(MMUIRenderSubWindow 自绘),
//! 控件树路线结构性失效;`Windows.Media.Ocr` 是系统内置离线引擎
//! (zh-Hans-CN 实测可用),零新依赖、零外部命令,符合「优先 OS API」原则。
//!
//! 坐标约定:进程已做 Per-Monitor-V2 DPI 感知(见 `windows_input::ensure_dpi_aware`),
//! GetWindowRect / BitBlt / OCR 词框 / SetCursorPos 四者同源(物理像素),
//! OCR 返回的窗口相对坐标 + 窗口原点 = 可直接 `click_point` 的屏幕绝对坐标。

#![cfg(windows)]

use std::sync::OnceLock;

use windows::core::{Interface, HSTRING};
use windows::Foundation::Rect as WinRtRect;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::StorageFile;
use windows::Storage::Streams::IRandomAccessStream;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, SRCCOPY,
};
use windows::Win32::Graphics::GdiPlus::{
    GdipCreateBitmapFromHBITMAP, GdipDisposeImage, GdipSaveImageToFile, GdiplusStartup,
    GdiplusStartupInput,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use super::platform_err;
use crate::error::Result;
use super::windows_input::ensure_dpi_aware;
use super::{OcrBlock, Rect};

/// OCR 词数上限(防 tool_result 暴涨)。
const MAX_OCR_WORDS: usize = 300;

/// GDI+ 一次初始化(进程生命周期内常驻,不做 Shutdown)。
fn ensure_gdiplus() -> Result<()> {
    static TOKEN: OnceLock<std::result::Result<usize, String>> = OnceLock::new();
    // get_or_try_init 未稳定(once_cell_try),这里 get_or_init 内部急切求值 Result。
    let slot = TOKEN.get_or_init(|| {
        let mut token: usize = 0;
        // GdiplusVersion 必须显式置 1(Default 的 0 会初始化失败)
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        // SAFETY:标准 GDI+ 启动;输出参数为原生指针。
        let status = unsafe { GdiplusStartup(&mut token, &input, std::ptr::null_mut()) };
        if status.0 == 0 {
            Ok(token)
        } else {
            Err(format!("GDI+ 初始化失败: {status:?}(PNG 编码不可用,OCR 无法执行)"))
        }
    });
    match slot {
        Ok(_) => Ok(()),
        Err(msg) => Err(platform_err("windows", msg.clone())),
    }
}

/// 截取并保存到**指定路径**(MCP_Window_Use(action=screenshot) 的 Windows 纯 Rust 路径)。
pub fn capture_window_png_to(
    hwnd: HWND,
    region: Option<Rect>,
    out_path: &std::path::Path,
) -> Result<Rect> {
    let (_png, _win, cap) = capture_window_png_impl(hwnd, region, Some(out_path))?;
    Ok(cap)
}

/// 截取窗口(可选区域,窗口相对物理像素)→ 临时 PNG 路径。
pub fn capture_window_png(
    hwnd: HWND,
    region: Option<Rect>,
) -> Result<(std::path::PathBuf, Rect, Rect)> {
    capture_window_png_impl(hwnd, region, None)
}

/// 截图实现:`out_path = None` 时写临时文件(OCR 路径,调用方负责清理),否则写指定路径。
fn capture_window_png_impl(
    hwnd: HWND,
    region: Option<Rect>,
    out_path: Option<&std::path::Path>,
) -> Result<(std::path::PathBuf, Rect, Rect)> {
    ensure_dpi_aware();
    ensure_gdiplus()?;

    // 窗口矩形(物理像素;含标题栏 —— 与 MCP_Window_Use(action=list) bounds 同基准)
    let mut wr = RECT::default();
    // SAFETY:按 HWND 取矩形,失败转错误。
    unsafe {
        if GetWindowRect(hwnd, &mut wr).is_err() {
            return Err(platform_err("windows", "GetWindowRect 失败(窗口可能已关闭)"));
        }
    }
    let win = Rect {
        x: wr.left as i64,
        y: wr.top as i64,
        width: (wr.right - wr.left) as i64,
        height: (wr.bottom - wr.top) as i64,
    };
    if win.width <= 0 || win.height <= 0 {
        return Err(platform_err(
            "windows",
            format!("窗口矩形非法({win:?});最小化窗口请先恢复(MCP_Window_Use(action=open) 会自动恢复)"),
        ));
    }
    let cap = match region {
        Some(r) => {
            // 裁剪到窗口范围内
            let x0 = r.x.clamp(0, win.width.saturating_sub(1));
            let y0 = r.y.clamp(0, win.height.saturating_sub(1));
            let x1 = (r.x + r.width).clamp(x0 + 1, win.width);
            let y1 = (r.y + r.height).clamp(y0 + 1, win.height);
            Rect {
                x: win.x + x0,
                y: win.y + y0,
                width: x1 - x0,
                height: y1 - y0,
            }
        }
        None => win,
    };

    // 目标文件:显式路径 or 临时文件
    let target: std::path::PathBuf = match out_path {
        Some(p) => p.to_path_buf(),
        None => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            std::env::temp_dir().join(format!("laew_ocr_{ts}.png"))
        }
    };

    // GDI 截屏(句柄在本函数内闭环释放)
    // SAFETY:标准 CreateCompatibleDC/BitBlt/DeleteDC 序列;失败路径同样释放。
    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(platform_err("windows", "GetDC(屏幕) 失败"));
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        let bmp = CreateCompatibleBitmap(screen_dc, cap.width as i32, cap.height as i32);
        let old = SelectObject(mem_dc, windows::Win32::Graphics::Gdi::HGDIOBJ(bmp.0));
        let blit_ok = BitBlt(
            mem_dc,
            0,
            0,
            cap.width as i32,
            cap.height as i32,
            screen_dc,
            cap.x as i32,
            cap.y as i32,
            SRCCOPY,
        )
        .is_ok();
        let result: Result<(std::path::PathBuf, Rect, Rect)> = if blit_ok {
            save_hbitmap_png(bmp, &target)
                .map(|_| (target.clone(), win, cap))
        } else {
            Err(platform_err(
                "windows",
                "BitBlt 截屏失败(窗口可能在锁屏桌面 / 另一用户会话)",
            ))
        };
        let _ = SelectObject(mem_dc, old);
        let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen_dc);
        result
    }
}

/// HBITMAP → PNG 文件(GDI+)。
///
/// # Safety
/// `bmp` 必须是有效的 GDI 位图句柄(调用方 CreateCompatibleBitmap 产物)。
unsafe fn save_hbitmap_png(
    bmp: windows::Win32::Graphics::Gdi::HBITMAP,
    target: &std::path::Path,
) -> Result<()> {
    let mut gpbitmap: *mut windows::Win32::Graphics::GdiPlus::GpBitmap = std::ptr::null_mut();
    let st = GdipCreateBitmapFromHBITMAP(
        bmp,
        windows::Win32::Graphics::Gdi::HPALETTE(std::ptr::null_mut()),
        &mut gpbitmap,
    );
    if st.0 != 0 {
        return Err(platform_err(
            "windows",
            format!("GdipCreateBitmapFromHBITMAP 失败: {st:?}"),
        ));
    }
    // PNG 编码器 CLSID {557CF406-1A04-11D3-9A73-0000F81EF32E}
    let png_clsid = windows::core::GUID::from_u128(0x557cf406_1a04_11d3_9a73_0000f81ef32e);
    let path_w: HSTRING = target.as_os_str().into();
    let save_st = GdipSaveImageToFile(
        gpbitmap as *mut windows::Win32::Graphics::GdiPlus::GpImage,
        &path_w,
        &png_clsid,
        std::ptr::null(),
    );
    let _ = GdipDisposeImage(gpbitmap as *mut windows::Win32::Graphics::GdiPlus::GpImage);
    if save_st.0 != 0 {
        return Err(platform_err(
            "windows",
            format!("GdipSaveImageToFile(PNG) 失败: {save_st:?}(目标 {target:?})"),
        ));
    }
    Ok(())
}

/// 对窗口(可选区域)执行 OCR,返回词级文本块(窗口相对物理像素)。
pub fn ocr_window(hwnd: HWND, region: Option<Rect>, lang: Option<&str>) -> Result<Vec<OcrBlock>> {
    let (png, win, cap) = capture_window_png(hwnd, region)?;
    // 兜底清理临时 PNG(正常/异常路径都不留残留)
    struct Cleanup<'a>(&'a std::path::Path);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
    let _guard = Cleanup(&png);

    let engine = build_ocr_engine(lang)?;
    let soft = decode_png_to_software_bitmap(&png)?;

    // OCR 引擎要求 Bgra8 / Gray8;PNG 常见解出 Rgba8 → 必要时转换
    let target = if soft.BitmapPixelFormat().map(|f| f == BitmapPixelFormat::Bgra8).unwrap_or(false)
    {
        soft
    } else {
        SoftwareBitmap::Convert(&soft, BitmapPixelFormat::Bgra8)
            .map_err(|e| platform_err("windows", format!("SoftwareBitmap::Convert(Bgra8) 失败: {e}")))?
    };

    let ocr_result = engine
        .RecognizeAsync(&target)
        .map_err(|e| platform_err("windows", format!("RecognizeAsync 失败: {e}")))?
        .get()
        .map_err(|e| platform_err("windows", format!("OCR 识别失败: {e}")))?;

    // 2026-09-16 第 67 轮实测:Windows OCR 对中文把每个汉字切成独立 Word
    // ('微'(392,49) / '信'(406,49) / '支'(420,49) ...),词级输出无法直接检索
    // 「通讯录」这类短语 —— 这里按 **Line 聚合**:每条 OcrLine 的词合并为
    // 一个块(文本拼接 + 词框并集),LLM 拿到的即是「一行 UI 文本 + 一个矩形」。
    let mut blocks = Vec::new();
    let lines = ocr_result
        .Lines()
        .map_err(|e| platform_err("windows", format!("读取 OCR Lines 失败: {e}")))?;
    for line in lines {
        if blocks.len() >= MAX_OCR_WORDS {
            return Ok(blocks);
        }
        let words = line
            .Words()
            .map_err(|e| platform_err("windows", format!("读取 OCR Words 失败: {e}")))?;
        let mut text = String::new();
        let mut x0 = i64::MAX;
        let mut y0 = i64::MAX;
        let mut x1 = i64::MIN;
        let mut y1 = i64::MIN;
        let mut any = false;
        for word in words {
            let t = word.Text().unwrap_or_default().to_string();
            if t.trim().is_empty() {
                continue;
            }
            let r: WinRtRect = word.BoundingRect().unwrap_or_default();
            any = true;
            text.push_str(&t);
            let wx = (cap.x - win.x) + r.X as i64;
            let wy = (cap.y - win.y) + r.Y as i64;
            x0 = x0.min(wx);
            y0 = y0.min(wy);
            x1 = x1.max(wx + r.Width.max(0.0) as i64);
            y1 = y1.max(wy + r.Height.max(0.0) as i64);
        }
        if any && !text.trim().is_empty() {
            blocks.push(OcrBlock {
                text,
                x: x0,
                y: y0,
                width: (x1 - x0).max(1),
                height: (y1 - y0).max(1),
            });
        }
    }
    Ok(blocks)
}

/// 临时 PNG → SoftwareBitmap(WinRT 解码,阻塞 get())。
fn decode_png_to_software_bitmap(png: &std::path::Path) -> Result<SoftwareBitmap> {
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(png.as_os_str()))
        .map_err(|e| platform_err("windows", format!("StorageFile 打开临时 PNG 失败: {e}")))?
        .get()
        .map_err(|e| platform_err("windows", format!("获取临时 PNG 失败: {e}")))?;
    let stream_with_type = file
        .OpenReadAsync()
        .map_err(|e| platform_err("windows", format!("OpenReadAsync 失败: {e}")))?
        .get()
        .map_err(|e| platform_err("windows", format!("打开 PNG 流失败: {e}")))?;
    let stream: IRandomAccessStream = stream_with_type
        .cast::<IRandomAccessStream>()
        .map_err(|e| platform_err("windows", format!("流类型转换失败: {e}")))?;
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|e| platform_err("windows", format!("BitmapDecoder 创建失败: {e}")))?
        .get()
        .map_err(|e| platform_err("windows", format!("BitmapDecoder 解码失败: {e}")))?;
    decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| platform_err("windows", format!("GetSoftwareBitmapAsync 失败: {e}")))?
        .get()
        .map_err(|e| platform_err("windows", format!("SoftwareBitmap 转换失败: {e}")))
}

/// 构造 OCR 引擎:显式语言 → 用户配置语言。
fn build_ocr_engine(lang: Option<&str>) -> Result<OcrEngine> {
    if let Some(tag) = lang.filter(|s| !s.trim().is_empty()) {
        let language = Language::CreateLanguage(&HSTRING::from(tag))
            .map_err(|e| platform_err("windows", format!("语言 {tag:?} 非法: {e}")))?;
        if !OcrEngine::IsLanguageSupported(&language).unwrap_or(false) {
            return Err(platform_err(
                "windows",
                format!("OCR 不支持语言 {tag};请省略 lang 参数用默认语言,或在系统安装对应 OCR 语言包"),
            ));
        }
        let engine = OcrEngine::TryCreateFromLanguage(&language)
            .map_err(|e| platform_err("windows", format!("OCR 引擎创建失败({tag}): {e}")))?;
        return Ok(engine);
    }
    OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
        platform_err(
            "windows",
            format!(
                "OCR 引擎不可用(TryCreateFromUserProfileLanguages 失败: {e});\
                 请检查 系统 OCR 语言包 是否安装中文/英文"
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_clsid_constant() {
        // 编码器 CLSID 固定值冒烟(防手抄错;GUID 无 Display,用 from_values 对照)
        let g = windows::core::GUID::from_u128(0x557cf406_1a04_11d3_9a73_0000f81ef32e);
        assert_eq!(
            g,
            windows::core::GUID::from_values(
                0x557cf406,
                0x1a04,
                0x11d3,
                [0x9a, 0x73, 0x00, 0x00, 0xf8, 0x1e, 0xf3, 0x2e]
            )
        );
    }

    /// 只读冒烟:对整个屏幕做一次小区域 OCR(验证 GDI+/WinRT/OCR 链路连通)。
    /// 依赖桌面会话,CI 无桌面时忽略。
    #[test]
    #[ignore = "需要交互桌面会话;人工运行 cargo test -p lsm-agent --lib -- --ignored"]
    fn ocr_screen_smoke() {
        let wins = super::super::current_driver().list_windows(None).unwrap();
        if wins.is_empty() {
            println!("无可见窗口,跳过");
            return;
        }
        let w = &wins[0];
        let hwnd = super::super::windows::WindowsDriver::parse_hwnd(&w.id).unwrap();
        let blocks = super::ocr_window(hwnd, None, None).unwrap();
        println!("OCR blocks = {}", blocks.len());
        for b in blocks.iter().take(5) {
            println!("  {} @ ({},{})", b.text, b.x, b.y);
        }
    }
}
