//! D1 @文件提及系统(2026-09-10 第二十八轮,L1426)。
//!
//! 用户在提示词中写 `@路径` / `@"带空格路径"` / `@路径#L10` / `@路径#L10-20`,
//! 提交时(TUI dispatch_prompt / CLI -p/-f)由 `expand_mentions` 提取、读取并
//! 以 `<<<LAEW:ATTACHMENTS>>>` 附件块追加到 user 消息末尾,送入主 Session 上下文。
//!
//! 设计来源:claudecode `src/utils/attachments.ts`(三正则形态 / 行号片段 /
//! 目录内联 / 大文件降级),见 docs/Agent源码调研/专题/专题-第十八轮-claudecode-深度分析.md §D1。
//!
//! 关键语义:
//! - **存在才注入**:路径不存在/二进制/协议形态(`@server:uri`)/agent 伪提及
//!   (`@agent-*`)一律跳过并记入 missed,原文不动 —— 避免邮箱、粘贴文本中的
//!   `@` 误命中污染上下文。
//! - **只读快照**:内容注入即定格,后续文件变更不影响本轮(与 Read 工具语义一致)。
//! - **路径基准 = 工作目录**(与 Bash/Read/Write 工具一致),绝对路径亦允许。
//! - **去重**:规范化路径 + 行区间相同只读一次(claudecode existingFileState 简化版)。

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

/// 单文件注入上限(claudecode maxSizeBytes=256KB 同款)。
const MAX_FILE_BYTES: u64 = 256 * 1024;
/// 目录内联条目上限(claudecode 1000,工程语境收敛到 200 —— 目录树仅作导航提示)。
const MAX_DIR_ENTRIES: usize = 200;
/// 二进制嗅探窗口:前 8KB 含 NUL 即判定二进制。
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// 单次提示词最多注入的附件数(防止一句话 @ 几十个文件打爆上下文)。
const MAX_ATTACHMENTS: usize = 8;

/// 附件块标记(对外可见,e2e wire 级断言用)。
pub const ATTACHMENTS_BEGIN: &str = "<<<LAEW:ATTACHMENTS>>>";
pub const ATTACHMENTS_END: &str = "<<<LAEW:ATTACHMENTS:END>>>";

/// 一条 @ 提及(提取阶段的纯文本形态,未做文件系统解析)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    /// 原始 token(含 `@` 与可选引号/行号后缀),用于 missed 报告。
    pub raw: String,
    /// 路径文本(已去引号、去行号后缀)。
    pub path_text: String,
    /// 起始行(1-based,含);None = 全文件。
    pub line_start: Option<u32>,
    /// 结束行(1-based,含);None = 到文件尾(或跟随 line_start 单行)。
    pub line_end: Option<u32>,
}

/// 展开结果。
#[derive(Debug, Clone)]
pub struct ExpandOutcome {
    /// 原文 + (有命中时)附件块;零命中时与原文完全相等。
    pub message: String,
    /// 成功附加的文件/目录数。
    pub attached: usize,
    /// 未命中 token 及原因(不存在/二进制/超上限等),供调用方打印提示。
    pub missed: Vec<String>,
}

fn mention_regexes() -> &'static (Regex, Regex) {
    static RE: OnceLock<(Regex, Regex)> = OnceLock::new();
    RE.get_or_init(|| {
        // 带引号:@\"path with spaces\"(#L 后缀在引号外)
        let quoted = Regex::new(r#"(?:^|\s)@"([^"]+)""#).expect("quoted mention regex");
        // 裸形态前置必须是行首或空白 → 邮箱 user@host 天然排除;
        // token 内不允许出现第二个 @(防链式吞噬)与空白。
        let bare = Regex::new(r"(?:^|\s)@([^\s@]+)").expect("bare mention regex");
        (quoted, bare)
    })
}

/// 解析 `@path#L10` / `@path#L10-20` 后缀;非 L 锚点(#heading)忽略。
/// 返回 (路径文本, 起始行, 结束行),逆区间归一(L5-3 → L3-5)。
fn parse_line_suffix(text: &str) -> (String, Option<u32>, Option<u32>) {
    let Some(hash) = text.find('#') else {
        return (text.to_string(), None, None);
    };
    let filename = &text[..hash];
    let frag = &text[hash + 1..];
    let Some(digits) = frag.strip_prefix('L') else {
        return (filename.to_string(), None, None); // #heading 类锚点忽略
    };
    let (a, b) = match digits.split_once('-') {
        Some((a, b)) => (a.parse::<u32>().ok(), b.parse::<u32>().ok()),
        None => (digits.parse::<u32>().ok(), None),
    };
    match (a, b) {
        (Some(s), Some(e)) => {
            let (lo, hi) = if s <= e { (s, e) } else { (e, s) };
            (filename.to_string(), Some(lo), Some(hi))
        }
        (Some(s), None) => (filename.to_string(), Some(s), Some(s)),
        _ => (filename.to_string(), None, None),
    }
}

/// 协议/agent 伪提及排除:`@agent-*`(claudecode agent 提及)、`@server:uri`(MCP)、
/// `@scheme://...`。laew 无对应体系,命中即非文件,直接排除。
fn is_non_file_token(path_text: &str) -> bool {
    if path_text.starts_with("agent-") {
        return true;
    }
    if path_text.contains("://") {
        return true;
    }
    // @server:uri 形态(首个路径分隔符前出现冒号)
    if let Some(colon) = path_text.find(':') {
        let slash = path_text.find(['/', '\\']).unwrap_or(usize::MAX);
        if colon < slash {
            // Windows 盘符 C:\ 例外(colon==1 且紧跟路径分隔符)
            let is_drive = colon == 1 && path_text.as_bytes()[0].is_ascii_alphabetic();
            if !is_drive {
                return true;
            }
        }
    }
    false
}

/// 从用户输入提取全部 @ 提及(未做文件系统解析,纯文本阶段)。
pub fn extract_mentions(text: &str) -> Vec<Mention> {
    let (quoted_re, bare_re) = mention_regexes();
    // 记录带引号命中的 (start, end),裸形态跳过与其重叠的命中(引号形态优先,
    // 防止 @"a b" 同时被裸正则抓出 @"a 半截)。
    let mut quoted_spans: Vec<(usize, usize)> = Vec::new();
    let mut out: Vec<Mention> = Vec::new();
    for caps in quoted_re.captures_iter(text) {
        let m = caps.get(0).expect("whole match");
        let inner = caps.get(1).expect("group 1").as_str();
        let (path_text, ls, le) = parse_line_suffix(inner);
        if path_text.is_empty() || is_non_file_token(&path_text) {
            continue;
        }
        quoted_spans.push((m.start(), m.end()));
        out.push(Mention {
            raw: m.as_str().trim_start().to_string(),
            path_text,
            line_start: ls,
            line_end: le,
        });
    }
    for caps in bare_re.captures_iter(text) {
        let m = caps.get(0).expect("whole match");
        if quoted_spans
            .iter()
            .any(|&(s, e)| m.start() < e && m.end() > s)
        {
            continue;
        }
        let token = caps.get(1).expect("group 1").as_str();
        // 引号起始的半截(如 @"abc 缺右引号)不是合法裸提及,跳过
        if token.starts_with('"') {
            continue;
        }
        let (path_text, ls, le) = parse_line_suffix(token);
        if path_text.is_empty() || is_non_file_token(&path_text) {
            continue;
        }
        out.push(Mention {
            raw: m.as_str().trim_start().to_string(),
            path_text,
            line_start: ls,
            line_end: le,
        });
    }
    out
}

/// 提及 → 绝对路径(相对路径以工作目录为基准)。
fn resolve(work_dir: &Path, path_text: &str) -> PathBuf {
    let p = Path::new(path_text);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        work_dir.join(p)
    }
}

/// 前 8KB 含 NUL → 二进制。
fn looks_binary(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; BINARY_SNIFF_BYTES];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    buf[..n].contains(&0)
}

/// 读取文件内容(尺寸上限 + 行区间切片 + 二进制跳过)。
/// 返回 Ok((内容, 标注行));Err(原因) 记入 missed。
fn load_file(path: &Path, m: &Mention) -> Result<(String, String), String> {
    let meta = fs::metadata(path).map_err(|e| format!("读取失败: {e}"))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "文件过大({} KB > 256 KB 上限),已跳过 — 请用 @路径#L起-止 引用关键片段",
            meta.len() / 1024
        ));
    }
    if looks_binary(path) {
        return Err("二进制文件,已跳过".to_string());
    }
    let raw = fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    let text = String::from_utf8_lossy(&raw);
    match (m.line_start, m.line_end) {
        (Some(s), Some(e)) => {
            let lines: Vec<&str> = text.lines().collect();
            let total = lines.len() as u32;
            if s > total {
                return Err(format!("起始行 L{s} 超出文件总行数({total})"));
            }
            let e = e.min(total);
            let slice = &lines[(s - 1) as usize..e as usize];
            Ok((
                slice.join("\n"),
                format!("L{s}-{e} / 共 {total} 行, {} 行", slice.len()),
            ))
        }
        _ => {
            let total = text.lines().count();
            Ok((text.into_owned(), format!("共 {total} 行")))
        }
    }
}

/// 目录内联为条目列表(排序 + 子目录带 `/` + 200 条上限)。
fn load_dir(path: &Path) -> Result<(String, String), String> {
    let rd = fs::read_dir(path).map_err(|e| format!("读取目录失败: {e}"))?;
    let mut names: Vec<String> = Vec::new();
    for entry in rd.flatten() {
        let mut name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            name.push('/');
        }
        names.push(name);
    }
    names.sort();
    let total = names.len();
    let truncated = total > MAX_DIR_ENTRIES;
    names.truncate(MAX_DIR_ENTRIES);
    if truncated {
        names.push(format!("… 还有 {} 条", total - MAX_DIR_ENTRIES));
    }
    Ok((
        names.join("\n"),
        format!("目录, {total} 条目{}", if truncated { "(截断)" } else { "" }),
    ))
}

/// 提取并展开 @ 提及:命中内容以附件块追加到原文末尾。
///
/// 零命中时返回的 message 与 text 完全相等(调用方可零开销透传)。
pub fn expand_mentions(text: &str, work_dir: &Path) -> ExpandOutcome {
    let mentions = extract_mentions(text);
    let mut blocks: Vec<String> = Vec::new();
    let mut missed: Vec<String> = Vec::new();
    let mut seen: Vec<(PathBuf, Option<u32>, Option<u32>)> = Vec::new();

    for m in mentions {
        if blocks.len() >= MAX_ATTACHMENTS {
            missed.push(format!("{}(超过单轮 {} 个附件上限)", m.raw, MAX_ATTACHMENTS));
            continue;
        }
        let abs = resolve(work_dir, &m.path_text);
        // 去重键:路径 + 行区间(canonicalize 失败退回 join 结果,保持确定性)
        let key = (
            fs::canonicalize(&abs).unwrap_or_else(|_| abs.clone()),
            m.line_start,
            m.line_end,
        );
        if seen.contains(&key) {
            continue; // 重复提及静默跳过(claudecode already_read 简化版)
        }
        if !abs.exists() {
            missed.push(format!("{}(路径不存在,已忽略)", m.raw));
            continue;
        }
        let loaded = if abs.is_dir() {
            load_dir(&abs)
        } else {
            load_file(&abs, &m)
        };
        match loaded {
            Ok((content, note)) => {
                seen.push(key);
                blocks.push(format!(
                    "[附件 {}] @{} ({})\n<<<FILE: {}>>>\n{}\n<<<END FILE>>>",
                    blocks.len() + 1,
                    m.path_text,
                    note,
                    m.path_text,
                    content
                ));
            }
            Err(reason) => missed.push(format!("{}({})", m.raw, reason)),
        }
    }

    let attached = blocks.len();
    let message = if blocks.is_empty() {
        text.to_string()
    } else {
        format!(
            "{}\n\n{}\n以下 @ 提及的内容由系统自动附加(只读快照,供参考)。\n\n{}\n{}",
            text,
            ATTACHMENTS_BEGIN,
            blocks.join("\n\n"),
            ATTACHMENTS_END
        )
    };
    ExpandOutcome {
        message,
        attached,
        missed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mention_of(text: &str) -> Vec<String> {
        extract_mentions(text)
            .into_iter()
            .map(|m| m.path_text)
            .collect()
    }

    // ---------- 提取 ----------

    #[test]
    fn extracts_bare_mention() {
        assert_eq!(mention_of("看看 @src/main.rs 这个文件"), ["src/main.rs"]);
    }

    #[test]
    fn extracts_quoted_mention_with_spaces() {
        assert_eq!(
            mention_of(r#"check @"my docs/note.md" please"#),
            ["my docs/note.md"]
        );
    }

    #[test]
    fn ignores_email_like_at() {
        // 前置非空白 → 不识别
        assert!(extract_mentions("联系 user@example.com 或 admin@host").is_empty());
    }

    #[test]
    fn parses_single_line_suffix() {
        let ms = extract_mentions("看 @a.rs#L10 这段");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].path_text, "a.rs");
        assert_eq!(ms[0].line_start, Some(10));
        assert_eq!(ms[0].line_end, Some(10));
    }

    #[test]
    fn parses_line_range_suffix() {
        let ms = extract_mentions("@a.rs#L10-20");
        assert_eq!(ms[0].line_start, Some(10));
        assert_eq!(ms[0].line_end, Some(20));
    }

    #[test]
    fn normalizes_reversed_range() {
        let ms = extract_mentions("@a.rs#L20-10");
        assert_eq!(ms[0].line_start, Some(10));
        assert_eq!(ms[0].line_end, Some(20));
    }

    #[test]
    fn ignores_heading_anchor() {
        let ms = extract_mentions("@README.md#安装");
        assert_eq!(ms[0].path_text, "README.md");
        assert_eq!(ms[0].line_start, None);
    }

    #[test]
    fn excludes_agent_and_mcp_pseudo_mentions() {
        assert!(extract_mentions("@agent-reviewer 看一下").is_empty());
        assert!(extract_mentions("@server:res://x 资源").is_empty());
        assert!(extract_mentions("@https://example.com 链接").is_empty());
    }

    #[test]
    fn quoted_and_bare_coexist_without_overlap() {
        let ms = extract_mentions(r#"对比 @"a b.txt" 和 @c.txt"#);
        let paths: Vec<&str> = ms.iter().map(|m| m.path_text.as_str()).collect();
        assert_eq!(paths, ["a b.txt", "c.txt"]);
    }

    #[test]
    fn unclosed_quote_is_not_bare_mention() {
        // @"abc 缺右引号:裸正则会抓出 @"abc,应被跳过
        assert!(extract_mentions(r#"看 @"abc 的内容"#).is_empty());
    }

    // ---------- 展开(文件系统) ----------

    #[test]
    fn expands_existing_file_with_attachment_block() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("canary.txt"), "CANARY-CONTENT-001\n").unwrap();
        let out = expand_mentions("总结一下 @canary.txt", tmp.path());
        assert_eq!(out.attached, 1);
        assert!(out.missed.is_empty());
        assert!(out.message.contains(ATTACHMENTS_BEGIN));
        assert!(out.message.contains("CANARY-CONTENT-001"));
        assert!(out.message.contains("<<<FILE: canary.txt>>>"));
        assert!(out.message.starts_with("总结一下 @canary.txt")); // 原文保留
    }

    #[test]
    fn nonexistent_path_is_missed_and_text_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let out = expand_mentions("看 @不存在.txt", tmp.path());
        assert_eq!(out.attached, 0);
        assert_eq!(out.missed.len(), 1);
        assert_eq!(out.message, "看 @不存在.txt"); // 零命中零改动
    }

    #[test]
    fn line_range_slices_content() {
        let tmp = tempfile::tempdir().unwrap();
        let body: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        fs::write(tmp.path().join("r.txt"), body).unwrap();
        let out = expand_mentions("@r.txt#L3-5", tmp.path());
        assert!(out.message.contains("line3\nline4\nline5"));
        assert!(!out.message.contains("line6"));
        assert!(out.message.contains("L3-5 / 共 10 行"));
    }

    #[test]
    fn out_of_range_start_is_missed() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("s.txt"), "only one line\n").unwrap();
        let out = expand_mentions("@s.txt#L99", tmp.path());
        assert_eq!(out.attached, 0);
        assert!(out.missed[0].contains("超出文件总行数"));
    }

    #[test]
    fn directory_mention_inlines_listing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub/a.txt"), "a").unwrap();
        fs::write(tmp.path().join("top.txt"), "t").unwrap();
        let out = expand_mentions("@sub", tmp.path());
        assert_eq!(out.attached, 1);
        assert!(out.message.contains("a.txt"));
        assert!(out.message.contains("目录, 1 条目"));
    }

    #[test]
    fn duplicate_mention_reads_once() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("d.txt"), "DUP-CANARY\n").unwrap();
        let out = expand_mentions("@d.txt 和 @d.txt 再看一遍", tmp.path());
        assert_eq!(out.attached, 1);
        assert_eq!(out.message.matches("DUP-CANARY").count(), 1);
    }

    #[test]
    fn oversized_file_is_missed() {
        let tmp = tempfile::tempdir().unwrap();
        let big = "x".repeat((MAX_FILE_BYTES + 1) as usize);
        fs::write(tmp.path().join("big.txt"), big).unwrap();
        let out = expand_mentions("@big.txt", tmp.path());
        assert_eq!(out.attached, 0);
        assert!(out.missed[0].contains("文件过大"));
    }

    #[test]
    fn binary_file_is_missed() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("bin.dat"), b"\x00\x01\x02binary").unwrap();
        let out = expand_mentions("@bin.dat", tmp.path());
        assert_eq!(out.attached, 0);
        assert!(out.missed[0].contains("二进制"));
    }

    #[test]
    fn quoted_path_with_spaces_expands() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("my docs")).unwrap();
        fs::write(tmp.path().join("my docs/note.md"), "SPACE-CANARY\n").unwrap();
        let out = expand_mentions(r#"读 @"my docs/note.md""#, tmp.path());
        assert_eq!(out.attached, 1);
        assert!(out.message.contains("SPACE-CANARY"));
    }

    #[test]
    fn attachment_cap_limits_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let mut prompt = String::new();
        for i in 0..12 {
            fs::write(tmp.path().join(format!("f{i}.txt")), format!("c{i}")).unwrap();
            prompt.push_str(&format!("@f{i}.txt "));
        }
        let out = expand_mentions(&prompt, tmp.path());
        assert_eq!(out.attached, MAX_ATTACHMENTS);
        assert_eq!(out.missed.len(), 12 - MAX_ATTACHMENTS);
    }
}
