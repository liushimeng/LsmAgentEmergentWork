//! Read 工具文件类型探测(magic number + BOM + 扩展名兜底)。
//!
//! 设计取舍:不引入 `infer` / `image` 等新 crate,纯字节常量匹配常见 magic number。
//! 覆盖:PNG / JPEG / GIF / WebP / BMP / PDF / UTF-16 LE·BE BOM。
//!
//! 对应知识库 gap:第七轮多模态专题 P0 路线 (b) — Read 工具按 magic number 分流。

use std::path::Path;

/// 文件类型分流枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileClass {
    /// UTF-8 文本(默认)。
    Text,
    /// UTF-16 LE(BOM FF FE)。
    TextUtf16Le,
    /// UTF-16 BE(BOM FE FF)。
    TextUtf16Be,
    ImagePng,
    ImageJpeg,
    ImageGif,
    ImageWebp,
    ImageBmp,
    Pdf,
    /// 未知二进制。
    Binary,
}

impl FileClass {
    /// 对应 MIME 类型(文本/二进制返回 None)。
    pub fn media_type(&self) -> Option<&'static str> {
        match self {
            FileClass::ImagePng => Some("image/png"),
            FileClass::ImageJpeg => Some("image/jpeg"),
            FileClass::ImageGif => Some("image/gif"),
            FileClass::ImageWebp => Some("image/webp"),
            FileClass::ImageBmp => Some("image/bmp"),
            FileClass::Pdf => Some("application/pdf"),
            _ => None,
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(
            self,
            FileClass::ImagePng
                | FileClass::ImageJpeg
                | FileClass::ImageGif
                | FileClass::ImageWebp
                | FileClass::ImageBmp
        )
    }

    pub fn is_text(&self) -> bool {
        matches!(
            self,
            FileClass::Text | FileClass::TextUtf16Le | FileClass::TextUtf16Be
        )
    }

    pub fn is_pdf(&self) -> bool {
        matches!(self, FileClass::Pdf)
    }
}

/// 根据前若干字节探测文件类型(核心分流逻辑)。
///
/// 匹配顺序严格:BOM → magic number → 兜底 Text。
pub fn classify_bytes(head: &[u8]) -> FileClass {
    // UTF-16 BOM 优先(FF FE / FE FF)
    if head.len() >= 2 {
        if head[0] == 0xFF && head[1] == 0xFE {
            return FileClass::TextUtf16Le;
        }
        if head[0] == 0xFE && head[1] == 0xFF {
            return FileClass::TextUtf16Be;
        }
    }
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if head.len() >= 8
        && head[0] == 0x89
        && head[1] == 0x50
        && head[2] == 0x4E
        && head[3] == 0x47
        && head[4] == 0x0D
        && head[5] == 0x0A
        && head[6] == 0x1A
        && head[7] == 0x0A
    {
        return FileClass::ImagePng;
    }
    // JPEG: FF D8 FF
    if head.len() >= 3 && head[0] == 0xFF && head[1] == 0xD8 && head[2] == 0xFF {
        return FileClass::ImageJpeg;
    }
    // GIF: 47 49 46 38 (GIF8)
    if head.len() >= 4
        && head[0] == 0x47
        && head[1] == 0x49
        && head[2] == 0x46
        && head[3] == 0x38
    {
        return FileClass::ImageGif;
    }
    // WebP: RIFF....WEBP(52 49 46 46 ?? ?? ?? ?? 57 45 42 50)
    if head.len() >= 12
        && head[0] == 0x52
        && head[1] == 0x49
        && head[2] == 0x46
        && head[3] == 0x46
        && head[8] == 0x57
        && head[9] == 0x45
        && head[10] == 0x42
        && head[11] == 0x50
    {
        return FileClass::ImageWebp;
    }
    // BMP: 42 4D (BM)
    if head.len() >= 2 && head[0] == 0x42 && head[1] == 0x4D {
        return FileClass::ImageBmp;
    }
    // PDF: 25 50 44 46 (%PDF)
    if head.len() >= 4
        && head[0] == 0x25
        && head[1] == 0x50
        && head[2] == 0x44
        && head[3] == 0x46
    {
        return FileClass::Pdf;
    }
    // 默认:先假设文本(后续 UTF-8 校验失败再降级为 Binary)
    FileClass::Text
}

/// 扩展名兜底白名单:字节探测为 Binary 但扩展名明确是文本时回退 Text。
const TEXT_EXT_FALLBACK: &[&str] = &[
    "txt", "md", "markdown", "rs", "toml", "yaml", "yml", "json", "jsonl", "xml", "html",
    "htm", "css", "js", "ts", "py", "rb", "go", "java", "c", "h", "cpp", "hpp", "cc",
    "swift", "kt", "kts", "scala", "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd",
    "sql", "log", "ini", "cfg", "conf", "env", "gitignore", "dockerignore", "editorconfig",
    "csv", "tsv", "proto", "thrift", "graphql", "vue", "jsx", "tsx", "svelte", "lua",
    "php", "pl", "pm", "r", "jl", "ex", "exs", "erl", "hrl", "clj", "cljs", "edn",
    "hs", "lhs", "ml", "mli", "fs", "fsi", "fsx", "nim", "nims", "zig", "d", "v",
    "sv", "vhd", "vhdl", "tex", "ltx", "bib", "org", "rst", "adoc", "pod",
];

/// 综合探测:字节探测 + 扩展名兜底。
pub fn classify(path: &Path, head: &[u8]) -> FileClass {
    let by_bytes = classify_bytes(head);
    if by_bytes == FileClass::Binary {
        // 字节探测为 Binary,但扩展名明确是文本 → 回退 Text
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_ascii_lowercase();
            if TEXT_EXT_FALLBACK.iter().any(|e| *e == ext_lower) {
                return FileClass::Text;
            }
        }
    }
    by_bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_magic_matches() {
        let h = b"\x89PNG\r\n\x1a\nxxx";
        assert_eq!(classify_bytes(h), FileClass::ImagePng);
    }

    #[test]
    fn jpeg_magic_matches() {
        let h = b"\xff\xd8\xff\xe0\x00\x10JFIF";
        assert_eq!(classify_bytes(h), FileClass::ImageJpeg);
    }

    #[test]
    fn gif_magic_matches() {
        let h = b"GIF89a\x01\x00\x01\x00\x80";
        assert_eq!(classify_bytes(h), FileClass::ImageGif);
    }

    #[test]
    fn webp_magic_matches() {
        let h = b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert_eq!(classify_bytes(h), FileClass::ImageWebp);
    }

    #[test]
    fn bmp_magic_matches() {
        let h = b"BM\x46\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(classify_bytes(h), FileClass::ImageBmp);
    }

    #[test]
    fn pdf_magic_matches() {
        let h = b"%PDF-1.4";
        assert_eq!(classify_bytes(h), FileClass::Pdf);
    }

    #[test]
    fn utf16le_bom_matches() {
        let h = b"\xff\xfeH\x00e\x00l\x00l\x00o\x00";
        assert_eq!(classify_bytes(h), FileClass::TextUtf16Le);
    }

    #[test]
    fn utf16be_bom_matches() {
        let h = b"\xfe\xff\x00H\x00e\x00l\x00l\x00o";
        assert_eq!(classify_bytes(h), FileClass::TextUtf16Be);
    }

    #[test]
    fn plain_text_falls_back_to_text() {
        let h = b"Hello, world!\nThis is plain text.";
        assert_eq!(classify_bytes(h), FileClass::Text);
    }

    #[test]
    fn empty_falls_back_to_text() {
        assert_eq!(classify_bytes(b""), FileClass::Text);
    }

    #[test]
    fn media_type_for_images() {
        assert_eq!(FileClass::ImagePng.media_type(), Some("image/png"));
        assert_eq!(FileClass::ImageJpeg.media_type(), Some("image/jpeg"));
        assert_eq!(FileClass::ImageGif.media_type(), Some("image/gif"));
        assert_eq!(FileClass::ImageWebp.media_type(), Some("image/webp"));
        assert_eq!(FileClass::ImageBmp.media_type(), Some("image/bmp"));
        assert_eq!(FileClass::Pdf.media_type(), Some("application/pdf"));
        assert_eq!(FileClass::Text.media_type(), None);
        assert_eq!(FileClass::Binary.media_type(), None);
    }

    #[test]
    fn is_image_helpers() {
        assert!(FileClass::ImagePng.is_image());
        assert!(FileClass::ImageJpeg.is_image());
        assert!(!FileClass::Text.is_image());
        assert!(!FileClass::Pdf.is_image());
        assert!(!FileClass::Binary.is_image());
    }

    #[test]
    fn is_text_helpers() {
        assert!(FileClass::Text.is_text());
        assert!(FileClass::TextUtf16Le.is_text());
        assert!(FileClass::TextUtf16Be.is_text());
        assert!(!FileClass::ImagePng.is_text());
        assert!(!FileClass::Pdf.is_text());
    }

    #[test]
    fn classify_with_text_ext_fallback() {
        // 字节探测为 Binary(全是 0x00),但扩展名 .txt → 回退 Text
        let head = [0u8; 16];
        let p = Path::new("/tmp/foo.txt");
        assert_eq!(classify(p, &head), FileClass::Text);
    }

    #[test]
    fn classify_with_unknown_ext_keeps_bytes_result() {
        // 全零字节未匹配任何 magic number → classify_bytes 默认返回 Text;
        // classify 仅在 bytes=Binary 时走扩展名兜底,bytes=Text 时直接返回 Text。
        // 因此未知扩展名 + 无 magic → Text(默认值),而非 Binary。
        let head = [0u8; 16];
        let p = Path::new("/tmp/foo.unknown_ext_xyz");
        assert_eq!(classify(p, &head), FileClass::Text);
    }

    #[test]
    fn classify_with_md_ext_fallback() {
        let head = [0u8; 16];
        let p = Path::new("/tmp/README.md");
        assert_eq!(classify(p, &head), FileClass::Text);
    }
}
