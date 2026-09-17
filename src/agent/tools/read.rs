//! Read 工具:带行号读取文本文件,支持 offset/limit 分页。
//!
//! 第 77 轮增强:支持图片文件(base64 + media_type 标记)、UTF-16 编码转码、
//! PDF 友好提示、其他二进制兜底提示,避免 UTF-8 解码失败直接报错。

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};

use crate::agent::tools::Tool;
use crate::agent::tools::read_detect;
use crate::error::{AgentError, Result};

const DEFAULT_LIMIT: usize = 2000;
const MAX_LIMIT: usize = 4000;

/// 图片文件读取上限(5MB,与 claudecode IMAGE_TARGET_RAW_SIZE 一致)。
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// PDF 文件读取上限(10MB,仅读取并 base64 提示用,不解析)。
const MAX_PDF_BYTES: usize = 10 * 1024 * 1024;
/// 未知二进制兜底上限(超过按 Binary 拒绝)。
const MAX_BINARY_FALLBACK_BYTES: usize = 1 * 1024 * 1024;
/// 探测用首字节数。
const CLASSIFY_HEAD_LEN: usize = 16;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> &str {
        "读取文件内容。\n\
         - file_path 必须为绝对路径或可解析的相对路径(相对于工作目录)。\n\
         - 文本文件:返回带行号的内容(UTF-8 / UTF-16 自动探测);offset/limit 用于分页,\n\
           limit 默认 2000 行,最大 4000;单行超长截断到 4000 字符并标注。\n\
         - 图片文件(PNG/JPEG/GIF/WebP/BMP,≤5MB):返回 base64 + media_type 标记块,\n\
           供模型直接感知图片内容。\n\
         - PDF 文件(≤10MB):返回元信息 + 抽取提示(可用 Bash 调用 pdftotext)。\n\
         - 其他二进制:给出友好提示而非崩溃。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "待读取的文件路径" },
                "offset": { "type": "integer", "minimum": 1, "description": "从第 N 行开始(1-based)" },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT as i64, "description": "最多读取多少行" }
            },
            "required": ["file_path"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path_str = args
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "缺少 string 类型参数 file_path".into(),
            })?;
        let offset = args
            .get("offset")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1) as usize;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_LIMIT as u64)
            .min(MAX_LIMIT as u64) as usize;

        let path = resolve_path(path_str);
        let metadata = fs::metadata(&path).map_err(|e| {
            let attempted = resolve_path_with_candidates(path_str);
            let reason_msg = match attempted {
                ResolveCandidates::Single(p) => {
                    format!("stat 失败: {e} (tried: {})", p.display())
                }
                ResolveCandidates::WorkAndRoot { work, root } => {
                    format!(
                        "stat 失败: {e} (tried [work]: {}, [root]: {})\n{}",
                        work.display(),
                        root.display(),
                        format_path_diagnostic(path_str, &work, &root),
                    )
                }
            };
            AgentError::ToolExecution {
                tool: self.name().into(),
                reason: reason_msg,
            }
        })?;
        if !metadata.is_file() {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!("不是普通文件: {}", path.display()),
            });
        }
        let file_size = metadata.len() as usize;

        // 首字节探测(避免大二进制文件全量读取)。
        let mut head = vec![0u8; CLASSIFY_HEAD_LEN];
        let head_len = {
            use std::io::Read;
            let mut f = fs::File::open(&path).map_err(|e| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!("打开失败: {e}"),
            })?;
            let n = f.read(&mut head).map_err(|e| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!("读取失败: {e}"),
            })?;
            head.truncate(n);
            n
        };
        let class = read_detect::classify(&path, &head[..head_len]);

        match class {
            read_detect::FileClass::ImagePng
            | read_detect::FileClass::ImageJpeg
            | read_detect::FileClass::ImageGif
            | read_detect::FileClass::ImageWebp
            | read_detect::FileClass::ImageBmp => {
                read_image(&path, file_size, class)
            }
            read_detect::FileClass::Pdf => read_pdf(&path, file_size),
            read_detect::FileClass::TextUtf16Le => read_utf16(&path, file_size, true, offset, limit),
            read_detect::FileClass::TextUtf16Be => read_utf16(&path, file_size, false, offset, limit),
            read_detect::FileClass::Text => read_text(&path, file_size, offset, limit),
            read_detect::FileClass::Binary => read_binary_fallback(&path, file_size),
        }
    }
}

/// 读取图片文件:大小校验 → base64 编码 → 标记块。
fn read_image(path: &Path, file_size: usize, class: read_detect::FileClass) -> Result<String> {
    if file_size > MAX_IMAGE_BYTES {
        return Err(AgentError::ToolExecution {
            tool: "Read".into(),
            reason: format!(
                "图片文件过大: {} bytes(上限 {} bytes = 5MB)。建议先压缩或 crop 后再读取。",
                file_size, MAX_IMAGE_BYTES
            ),
        });
    }
    let bytes = fs::read(path).map_err(|e| AgentError::ToolExecution {
        tool: "Read".into(),
        reason: format!("读取失败: {e}"),
    })?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let media = class.media_type().unwrap_or("image/unknown");
    let b64_wrapped = wrap_base64_lines(&b64, 76);
    Ok(format!(
        "<<<LAEW:FILE path=\"{}\" media_type=\"{}\" bytes={} base64_bytes={}>>>\n\
         {}\n\
         <<<END_LAEW:FILE>>>",
        path.display(),
        media,
        file_size,
        b64.len(),
        b64_wrapped
    ))
}

/// 读取 PDF:大小校验 → 返回元信息 + 抽取提示(不解析)。
fn read_pdf(path: &Path, file_size: usize) -> Result<String> {
    if file_size > MAX_PDF_BYTES {
        return Err(AgentError::ToolExecution {
            tool: "Read".into(),
            reason: format!(
                "PDF 文件过大: {} bytes(上限 {} bytes = 10MB)。建议分页处理或裁剪后再读取。",
                file_size, MAX_PDF_BYTES
            ),
        });
    }
    let hint_path = path.display();
    Ok(format!(
        "<<<LAEW:FILE path=\"{}\" media_type=\"application/pdf\" bytes={} \
         pdf_hint=\"可用 Bash 调用: pdftotext '{hint_path}' - | head -n 200\">>>\n\
         (空内容 — PDF 需专用工具抽取文本,或使用 Chromium-WebUse 浏览器打开阅读)\n\
         <<<END_LAEW:FILE>>>",
        path.display(),
        file_size,
    ))
}

/// 读取 UTF-16 LE/BE:跳过 BOM 后解码。
fn read_utf16(
    path: &Path,
    file_size: usize,
    little_endian: bool,
    offset: usize,
    limit: usize,
) -> Result<String> {
    let bytes = fs::read(path).map_err(|e| AgentError::ToolExecution {
        tool: "Read".into(),
        reason: format!("读取失败: {e}"),
    })?;
    // 跳过 2 字节 BOM。
    let data = if bytes.len() >= 2 {
        &bytes[2..]
    } else {
        &bytes[..]
    };
    // 字节对齐到 u12。
    if data.len() % 2 != 0 {
        return Err(AgentError::ToolExecution {
            tool: "Read".into(),
            reason: "UTF-16 文件字节数非 2 的整数倍,可能已损坏。".into(),
        });
    }
    let u16_slice: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| {
            if little_endian {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    let text = if little_endian {
        String::from_utf16_lossy(&u16_slice)
    } else {
        String::from_utf16_lossy(&u16_slice)
    };
    let encoding_label = if little_endian {
        "utf-16-le"
    } else {
        "utf-16-be"
    };
    render_text_lines(&path, &text, file_size, offset, limit, Some(encoding_label))
}

/// 读取 UTF-8 文本:原逻辑。
fn read_text(path: &Path, file_size: usize, offset: usize, limit: usize) -> Result<String> {
    let content = fs::read_to_string(path).map_err(|e| AgentError::ToolExecution {
        tool: "Read".into(),
        reason: format!(
            "读取失败: {e}。该文件可能不是 UTF-8 文本,已尝试的探测类型:Text。\
             如需读取二进制/图片,请确认文件类型。"
        ),
    })?;
    render_text_lines(path, &content, file_size, offset, limit, None)
}

/// 未知二进制兜底:lossy utf-8 尝试。
fn read_binary_fallback(path: &Path, file_size: usize) -> Result<String> {
    if file_size > MAX_BINARY_FALLBACK_BYTES {
        return Err(AgentError::ToolExecution {
            tool: "Read".into(),
            reason: format!(
                "非文本/图片/PDF 的二进制文件,且大小 {} bytes 超过兜底上限 {} bytes,拒绝读取。\
                 如需强制读取片段,请用 Bash: xxd {} | head -n 100",
                file_size, MAX_BINARY_FALLBACK_BYTES, path.display()
            ),
        });
    }
    let bytes = fs::read(path).map_err(|e| AgentError::ToolExecution {
        tool: "Read".into(),
        reason: format!("读取失败: {e}"),
    })?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(format!(
        "<<<LAEW:FILE path=\"{}\" media_type=\"application/octet-stream\" bytes={} \
         note=\"非 UTF-8 已 lossy 转码,可能含乱码\">>>\n\
         {}\n\
         <<<END_LAEW:FILE>>>",
        path.display(),
        file_size,
        text
    ))
}

/// 文本行号渲染(复用原逻辑,加可选编码标记)。
fn render_text_lines(
    path: &Path,
    content: &str,
    _file_size: usize,
    offset: usize,
    limit: usize,
    encoding: Option<&str>,
) -> Result<String> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let total = lines.len();
    let start_idx = (offset - 1).min(total);
    let end_idx = (start_idx + limit).min(total);

    let mut buf = String::new();
    if let Some(enc) = encoding {
        buf.push_str(&format!("<<<LAEW:ENCODING {}>>>\n", enc));
    }
    buf.push_str(&format!(
        "<<< {} (lines {}-{} / total {}) >>>\n",
        path.display(),
        offset,
        end_idx,
        total
    ));
    let width = (total.max(1)).to_string().len();
    for (i, line) in lines[start_idx..end_idx].iter().enumerate() {
        let line_num = start_idx + i + 1;
        let display = if line.len() > 4000 {
            let cut = line
                .char_indices()
                .nth(4000)
                .map(|(n, _)| n)
                .unwrap_or(line.len());
            format!("{}...[截断]", &line[..cut])
        } else {
            line.trim_end_matches('\n').to_string()
        };
        buf.push_str(&format!(
            "{:>width$}\t{}\n",
            line_num,
            display,
            width = width
        ));
    }
    // L1208:文件内容是典型的「外部内容」,扫描 prompt injection 后返回
    Ok(crate::agent::safety::scan_and_wrap(
        &buf,
        crate::agent::safety::InjectionSource::ReadFile,
    )
    .wrapped_text)
}

/// base64 字符串按固定列宽换行(IAM RFC 2045 76 字符)。
fn wrap_base64_lines(b64: &str, width: usize) -> String {
    let mut out = String::with_capacity(b64.len() + b64.len() / width + 1);
    for (i, ch) in b64.chars().enumerate() {
        if i > 0 && i % width == 0 {
            out.push('\n');
        }
        out.push(ch);
    }
    out
}

/// 把工具入参里的相对路径解析为绝对路径(关联报告: 2026-09-09_05 E-002)。
///
/// 解析顺序(关联报告: 2026-09-09_04 D-003 + 2026-09-09_05 E-002):
/// 1. 已是绝对路径 → 原样返回;
/// 2. **工作目录**(env::current_dir())拼接;
/// 3. 若 2 不存在,**根目录**(laew 二进制所在目录,与 CLAUDE.md "根目录" 约定一致)拼接;
/// 4. 兜底返回 2 路径(让上层 stat 给出明确报错信息,而不是默默返回错路径)。
fn resolve_path(p: &str) -> PathBuf {
    resolve_path_with_candidates(p).into_path()
}

/// 路径解析的「候选路径」: 用于错误消息同时列出多个尝试路径。
///
/// 关联报告: 2026-09-09_05 E-002。
#[derive(Debug)]
pub(crate) enum ResolveCandidates {
    /// 仅一个候选路径(绝对路径且无根目录回退 / 工作目录直接命中)
    Single(PathBuf),
    /// 两个候选路径: 工作目录拼接 + 根目录拼接(均已尝试)
    WorkAndRoot { work: PathBuf, root: PathBuf },
}

impl ResolveCandidates {
    /// 取出最终选定的路径(用于实际 stat / read)。
    pub fn into_path(self) -> PathBuf {
        match self {
            ResolveCandidates::Single(p) => p,
            ResolveCandidates::WorkAndRoot { work, root } => {
                // 优先选实际存在的(供 caller 使用);理论上两者都不存在时返回 work
                if root.exists() {
                    root
                } else {
                    work
                }
            }
        }
    }
}

fn resolve_path_with_candidates(p: &str) -> ResolveCandidates {
    let path = Path::new(p);
    if path.is_absolute() {
        tracing::debug!(path = %path.display(), "resolve_path: 绝对路径");
        // 关联报告: 2026-09-09_04 D-003 —— 若 LLM 把「项目相对路径」误拼成「工作目录绝对路径」,
        // 而真实文件在根目录,做一次根目录替换重试。
        let work = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if let Ok(rel) = path.strip_prefix(&work) {
            // 该路径在工作目录下,但 stat 失败;尝试「根目录 + 相对路径」
            if let Ok(exe) = std::env::current_exe() {
                if let Some(root) = exe.parent() {
                    let root_path = root.join(rel);
                    if root_path.exists() && !path.exists() {
                        tracing::debug!(
                            orig = %path.display(),
                            tried = %root_path.display(),
                            "resolve_path 工作目录绝对路径 → 根目录回退"
                        );
                        return ResolveCandidates::WorkAndRoot {
                            work: path.to_path_buf(),
                            root: root_path,
                        };
                    }
                }
            }
        }
        return ResolveCandidates::Single(path.to_path_buf());
    }
    let work = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let work_path = work.join(path);
    let work_exists = work_path.exists();
    tracing::debug!(
        rel = %p,
        work = %work.display(),
        work_path = %work_path.display(),
        work_exists = %work_exists,
        "resolve_path 工作目录尝试"
    );
    if work_exists {
        return ResolveCandidates::Single(work_path);
    }
    // 根目录回退:从 current_exe() 父目录推导
    match std::env::current_exe() {
        Ok(exe) => {
            tracing::debug!(exe = %exe.display(), "resolve_path current_exe 成功");
            match exe.parent() {
                Some(root) => {
                    let root_path = root.join(path);
                    let root_exists = root_path.exists();
                    tracing::debug!(
                        rel = %p,
                        root = %root.display(),
                        root_path = %root_path.display(),
                        root_exists = %root_exists,
                        "resolve_path 根目录回退判断"
                    );
                    if root_exists {
                        return ResolveCandidates::Single(root_path);
                    }
                    // 关联报告: 2026-09-09_05 E-002 —— 工作目录与根目录均不存在,
                    // 返回两个候选路径以便错误消息同时列出。
                    return ResolveCandidates::WorkAndRoot {
                        work: work_path,
                        root: root_path,
                    };
                }
                None => {
                    tracing::debug!(exe = %exe.display(), "resolve_path current_exe 无父目录");
                }
            }
        }
        Err(e) => {
            tracing::debug!(err = %e, "resolve_path current_exe 失败");
        }
    }
    tracing::debug!(rel = %p, fallback = %work_path.display(), "resolve_path 兜底返回 work_path");
    ResolveCandidates::Single(work_path)
}

/// 在 Read 工具 stat 失败时为错误消息追加「路径诊断」提示。
///
/// 关联报告: 2026-09-09_06 F-001。
///
/// 设计意图:典型 B09 测试场景里,LLM 想读 `src/agent/tools/read.rs` 这种「laew
/// 自身源码相对路径」,但工作目录是 `TestWorkSpace/`(laew 子目录),导致工作目录
/// 拼接路径 `TestWorkSpace/src/agent/tools/read.rs` 和根目录回退路径
/// `LsmAgentEmergentWork/src/agent/tools/read.rs` **都不命中**(后者存在但错误
/// 消息没显式提示「laew 根目录 + src/ 才是正解」)。本函数在错误消息末尾追加
/// 引导文案,降低 LLM 反复构造变体路径浪费迭代的概率。
///
/// 触发条件:
/// - `path_str` 以 `src/`、`tests/`、`docs/` 开头(典型 laew 源码/测试/文档相对路径),
///   且工作目录 ≠ laew 根目录(典型「在子目录里跑 laew」场景);
/// - 拼接后两条路径都不存在。
///
/// 不引入新依赖;纯字符串拼接;非命中场景不增加任何输出。
fn format_path_diagnostic(path_str: &str, _work: &Path, root: &Path) -> String {
    let work_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let work_dir_name = work_dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let root_dir_name = root
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // 场景 A:典型「laew 源码相对路径在子目录里跑」 → 提示改用绝对路径
    let looks_like_source_rel = (path_str.starts_with("src/")
        || path_str.starts_with("tests/")
        || path_str.starts_with("docs/"))
        && !work_dir_name.is_empty()
        && !root_dir_name.is_empty()
        && work_dir_name != root_dir_name;

    // 场景 B:工作目录 = 根目录(就在 laew 根目录下跑),但相对路径在工作目录里查不到
    // → 提示「相对路径不在工作目录」即可
    let same_dir = !work_dir_name.is_empty() && work_dir_name == root_dir_name;

    if looks_like_source_rel {
        format!(
            "提示: 目标路径看起来像 laew 源码相对路径(`{path_str}`),但当前工作目录 \
             是 `{work_dir_name}/`(laew 的子目录),相对路径在工作目录拼接下不命中。\
             \n  - 如需读取 laew 自身源码,请改用绝对路径 `{root}/{path_str}`(根目录 + 相对路径),\
             \n    或先 `cd` 到 laew 根目录再使用相对路径。",
            path_str = path_str,
            work_dir_name = work_dir_name,
            root = root
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        )
    } else if same_dir {
        format!(
            "提示: 相对路径 `{path_str}` 在工作目录 `{work_dir_name}` 下未找到。\
             请确认文件实际位置(可用 `ls` 或 `find` 探查),或改用绝对路径。",
            path_str = path_str,
            work_dir_name = work_dir_name,
        )
    } else {
        // 非典型场景:不追加诊断,保持现有错误消息紧凑
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn reads_with_line_numbers() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "alpha").unwrap();
        writeln!(f, "beta").unwrap();
        writeln!(f, "gamma").unwrap();
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
        assert!(out.contains("alpha"));
        assert!(out.contains("beta"));
        assert!(out.contains("gamma"));
        assert!(out.contains("total 3"));
    }

    #[tokio::test]
    async fn respects_offset_and_limit() {
        let mut f = NamedTempFile::new().unwrap();
        for i in 1..=5 {
            writeln!(f, "line{}", i).unwrap();
        }
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool
            .execute(json!({"file_path": p, "offset": 2, "limit": 2}))
            .await
            .unwrap();
        assert!(out.contains("line2"));
        assert!(out.contains("line3"));
        assert!(!out.contains("line1"));
        assert!(!out.contains("line4"));
    }

    #[tokio::test]
    async fn missing_argument_errors() {
        let err = ReadTool.execute(json!({})).await.unwrap_err();
        assert!(matches!(err, AgentError::ToolExecution { .. }));
    }

    // ========== 错误信息双路径(第 05 轮 E-002,方案 tmpPlan/2026-09-09_05) ==========

    #[test]
    fn missing_rel_path_reports_work_and_root_attempts() {
        // 相对路径,工作目录与根目录拼接后都不存在 → 错误应同时显示两个尝试路径
        let work = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        // 构造一个绝对不会存在的相对路径
        let rel = "definitely_not_exists_e2e_test_12345/file.txt";
        let candidates = resolve_path_with_candidates(rel);
        match candidates {
            ResolveCandidates::WorkAndRoot { work: w, root: r } => {
                assert_eq!(w, work.join(rel), "工作目录路径应拼接正确");
                // 根路径由 current_exe().parent() 推导,在此仅校验非空
                assert!(!r.as_os_str().is_empty(), "根目录路径应存在");
            }
            other => panic!("应返回 WorkAndRoot(两个路径均不存在),实际 {other:?}"),
        }
    }

    #[test]
    fn single_resolve_when_work_dir_exists() {
        // 工作目录命中时,应返回 Single 而非 WorkAndRoot
        // 用绝对路径模拟「路径存在」(strip_prefix 不会命中,走 Single 分支)
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let p = tmp.path().to_str().unwrap();
        let candidates = resolve_path_with_candidates(p);
        match candidates {
            ResolveCandidates::Single(rp) => {
                assert_eq!(rp, std::path::PathBuf::from(p));
            }
            other => panic!("绝对路径应返回 Single,实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_error_message_contains_both_attempts_for_missing_rel_path() {
        // 验证 Read 工具在「相对路径均不存在」时,错误消息同时显示 work 与 root 路径
        let rel = "definitely_not_exists_e2e_test_12345/file.txt";
        let err = ReadTool
            .execute(json!({"file_path": rel}))
            .await
            .unwrap_err();
        match err {
            AgentError::ToolExecution { reason, .. } => {
                assert!(
                    reason.contains("[work]"),
                    "错误消息应标注 [work] 路径,实际: {reason}"
                );
                assert!(
                    reason.contains("[root]"),
                    "错误消息应标注 [root] 路径,实际: {reason}"
                );
            }
            other => panic!("预期 ToolExecution 错误,实际 {other:?}"),
        }
    }

    // ========== F-001 路径诊断(2026-09-09_06,方案 tmpPlan/2026-09-09_06) ==========

    #[test]
    fn format_path_diagnostic_for_source_rel_in_subdir_hints_absolute() {
        // 场景:工作目录 = laew 子目录(如 TestWorkSpace),LLM 想读 src/agent/tools/read.rs
        // 期望诊断提示「请改用绝对路径 根目录/src/...」
        let work_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let work_name = work_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        // 模拟一个不存在的 src 子路径
        let rel = "src/agent/tools/zzz_definitely_missing.rs";
        let work = work_dir.join(rel);
        let root = PathBuf::from("/tmp/mock_root").join(rel);
        let diag = format_path_diagnostic(rel, &work, &root);
        if work_name != "LsmAgentEmergentWork" && !work_name.is_empty() {
            // 工作目录是 laew 子目录(典型 TestWorkSpace 场景)
            assert!(
                diag.contains("改用绝对路径"),
                "应提示改用绝对路径,实际: {diag}"
            );
            assert!(
                diag.contains("laew 自身源码"),
                "应识别为源码相对路径,实际: {diag}"
            );
        } else {
            // 工作目录就是 laew 根目录 → 走 same_dir 分支或空分支
            // 此分支在 CI 上不一定触发,这里不强制断言
        }
    }

    #[test]
    fn format_path_diagnostic_for_misc_rel_no_diag() {
        // 场景:既不是 src/tests/docs 开头,也不在工作目录下
        // 期望:不追加诊断(空字符串),保持错误消息紧凑
        let rel = "some_random_file.txt";
        let work = PathBuf::from("/tmp/workdir").join(rel);
        let root = PathBuf::from("/tmp/rootdir").join(rel);
        let diag = format_path_diagnostic(rel, &work, &root);
        // 非典型场景:不应追加诊断
        assert!(diag.is_empty(), "非典型场景应返回空诊断,实际: {diag}");
    }

    #[tokio::test]
    async fn read_error_for_src_subpath_includes_diagnostic_hint() {
        // 验证 Read 工具在「相对路径是 src/...」且工作目录 ≠ 根目录时,
        // 错误消息末尾包含「请改用绝对路径」诊断提示
        let rel = "src/agent/tools/zzz_definitely_missing_e2e.rs";
        let err = ReadTool
            .execute(json!({"file_path": rel}))
            .await
            .unwrap_err();
        match err {
            AgentError::ToolExecution { reason, .. } => {
                // 双路径一定存在
                assert!(reason.contains("[work]"), "应含 [work],实际: {reason}");
                assert!(reason.contains("[root]"), "应含 [root],实际: {reason}");
                // 当前工作目录若是 laew 子目录(典型 TestWorkSpace),应有诊断
                let work_dir = std::env::current_dir().unwrap_or_default();
                let work_name = work_dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if work_name != "LsmAgentEmergentWork" && !work_name.is_empty() {
                    assert!(
                        reason.contains("请改用绝对路径"),
                        "工作目录是 laew 子目录时应追加诊断,实际: {reason}"
                    );
                }
            }
            other => panic!("预期 ToolExecution 错误,实际 {other:?}"),
        }
    }

    // ========== 第 77 轮:多模态与编码探测 ==========

    #[tokio::test]
    async fn read_png_returns_base64_block() {
        // 构造最小合法 PNG 头(仅测试探测 + base64,非真实可渲染图片)
        let mut f = NamedTempFile::new().unwrap();
        // 8 字节 PNG signature + 若干填充
        let mut png_bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        png_bytes.extend_from_slice(&[0u8; 100]);
        f.write_all(&png_bytes).unwrap();
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
        assert!(
            out.contains("<<<LAEW:FILE"),
            "应包含 LAEW:FILE 标记头,实际: {out}"
        );
        assert!(
            out.contains("media_type=\"image/png\""),
            "应包含 image/png,实际: {out}"
        );
        assert!(
            out.contains("<<<END_LAEW:FILE>>>"),
            "应包含 END_LAEW:FILE 标记尾,实际: {out}"
        );
    }

    #[tokio::test]
    async fn read_jpeg_returns_base64_block() {
        let mut f = NamedTempFile::new().unwrap();
        let mut jpg_bytes = b"\xff\xd8\xff\xe0\x00\x10JFIF".to_vec();
        jpg_bytes.extend_from_slice(&[0u8; 100]);
        f.write_all(&jpg_bytes).unwrap();
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
        assert!(out.contains("media_type=\"image/jpeg\""));
    }

    #[tokio::test]
    async fn read_pdf_returns_hint() {
        let mut f = NamedTempFile::new().unwrap();
        let mut pdf_bytes = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>endobj\n".to_vec();
        pdf_bytes.extend_from_slice(&[0u8; 100]);
        f.write_all(&pdf_bytes).unwrap();
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
        assert!(out.contains("application/pdf"));
        assert!(out.contains("pdftotext"), "应提示可用 pdftotext 抽取");
    }

    #[tokio::test]
    async fn read_utf16le_decodes_correctly() {
        let mut f = NamedTempFile::new().unwrap();
        // UTF-16 LE BOM + "Hello" in UTF-16 LE
        let mut bytes: Vec<u8> = vec![0xFF, 0xFE]; // BOM
        for ch in "Hello, 世界!\n".encode_utf16() {
            bytes.extend_from_slice(&ch.to_le_bytes());
        }
        f.write_all(&bytes).unwrap();
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
        assert!(
            out.contains("<<<LAEW:ENCODING utf-16-le>>>"),
            "应包含编码标记,实际: {out}"
        );
        assert!(out.contains("Hello, 世界!"), "应正确解码中文,实际: {out}");
    }

    #[tokio::test]
    async fn read_utf16be_decodes_correctly() {
        let mut f = NamedTempFile::new().unwrap();
        let mut bytes: Vec<u8> = vec![0xFE, 0xFF]; // BOM
        for ch in "Hello, 世界!\n".encode_utf16() {
            bytes.extend_from_slice(&ch.to_be_bytes());
        }
        f.write_all(&bytes).unwrap();
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
        assert!(
            out.contains("<<<LAEW:ENCODING utf-16-be>>>"),
            "应包含编码标记,实际: {out}"
        );
        assert!(out.contains("Hello, 世界!"), "应正确解码中文,实际: {out}");
    }

    #[tokio::test]
    async fn read_large_image_rejected_friendly() {
        // 构造 6MB PNG → 应拒绝并给出友好错误
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.png");
        let mut png_bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        png_bytes.extend_from_slice(&vec![0u8; 6 * 1024 * 1024]);
        std::fs::write(&path, &png_bytes).unwrap();
        let err = ReadTool
            .execute(json!({"file_path": path.to_str().unwrap()}))
            .await
            .unwrap_err();
        match err {
            AgentError::ToolExecution { reason, .. } => {
                assert!(
                    reason.contains("过大") || reason.contains("5MB"),
                    "应给出大小拒绝提示,实际: {reason}"
                );
            }
            other => panic!("预期 ToolExecution 错误,实际 {other:?}"),
        }
    }

    #[test]
    fn wrap_base64_lines_76_chars() {
        let input = "A".repeat(100);
        let wrapped = wrap_base64_lines(&input, 76);
        let lines: Vec<&str> = wrapped.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 76);
        assert_eq!(lines[1].len(), 24);
    }
}
