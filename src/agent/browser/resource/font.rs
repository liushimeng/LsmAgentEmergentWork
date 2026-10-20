//! 字体加载与解码：@font-face / 外链字体资源。
//!
//! 阶段四：支持 CSS @font-face 声明的字体文件加载与解码。
//! 依赖 `fontdue` 解析 TTF/OTF/WOFF；WOFF2 需 brotli 解码（当前 image crate 已引入）。
//! 字体文件上限 32MB；加载失败不阻塞页面渲染，回退系统字体。

use std::sync::Arc;

/// 已解码字体资源。
#[derive(Debug, Clone)]
pub struct FontResource {
    /// 字体源 URL。
    pub url: String,
    /// 字体族名（@font-face font-family）。
    pub family: String,
    /// 解码后的字体对象。
    pub font: Arc<fontdue::Font>,
    /// 字体文件字节数。
    pub bytes_len: usize,
}

/// 从字节解码字体（同步）。
pub fn decode_font(bytes: &[u8]) -> Result<fontdue::Font, String> {
    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
        .map_err(|e| format!("字体解码失败:{e:?}"))
}

/// 判断 Content-Type 是否为字体资源。
pub fn is_font_content_type(ct: &str) -> bool {
    let ct = ct.to_ascii_lowercase();
    ct.contains("font")
        || ct.contains("ttf")
        || ct.contains("otf")
        || ct.contains("woff")
        || ct.contains("woff2")
        || ct.contains("truetype")
        || ct.contains("opentype")
        || ct.contains("eot")
        || ct.contains("svg")
}

/// 字体文件字节上限：32MB。
pub const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_font_content_type_识别() {
        assert!(is_font_content_type("font/ttf"));
        assert!(is_font_content_type("font/woff2"));
        assert!(is_font_content_type("application/x-font-ttf"));
        assert!(is_font_content_type("font/opentype"));
        assert!(!is_font_content_type("text/html"));
        assert!(!is_font_content_type("image/png"));
    }

    #[test]
    fn decode_font_空字节失败() {
        assert!(decode_font(&[]).is_err());
    }
}
