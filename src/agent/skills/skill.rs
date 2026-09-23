//! Skill 数据模型 —— 对齐 atomcode + pi + openclaw 三家最佳实践。
//!
//! **frontmatter 全字段**(本轮范围):
//! - 顶层:`name / version / license / compatibility / user-invocable / allowed-tools / source`
//! - 嵌套 metadata(对齐 openclaw `metadata.openclaw.*`):
//!   - `metadata.<key>` 扁平字段(emoji / homepage / category / os)
//!   - `metadata.<key>.<sub>` 二级嵌套(requires.bins / install.kind 等)
//!   - `tags` / `categories` 等单值列表(`tags: [a, b, c]` 风格)
//!
//! **变量替换**(对齐 atomcode `skill.rs:32-92` 的 `match_substitution`):
//! - `$ARGUMENTS`(全量参数)
//! - `$1`..`$9`(位置参数,`$10` 原样保留避免歧义)
//! - `${CLAUDE_SKILL_DIR}` / `${LAEW_SKILL_DIR}`(skill 文件目录)
//! - `!`cmd``(shell 注入,信任用户)
//!
//! **atomcode 行为**:无占位符的 Skill(body 不含 `$ARGUMENTS`)调用时若带参数,
//! 自动在末尾追加 `\n\nARGUMENTS: <参数>`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::frontmatter;

pub const MAX_NAME_LEN: usize = 64;
pub const MAX_DESC_LEN: usize = 1024;
pub const MAX_BODY_BYTES: usize = 50_000;

/// Skill 来源(决定优先级与目录)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillSource {
    /// `~/.laew/skills/` 用户级
    User,
    /// `{cwd}/.laew/skills/` 项目级(随仓库提交)
    Project,
    /// `{cwd}/.agents/skills/` 跨 agent 共享(对齐 pi/opencode/openclaw)
    CrossAgent,
    /// 内嵌二进制(bundled/git-commit 等)
    Bundled,
}

impl SkillSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::CrossAgent => "cross-agent",
            Self::Bundled => "bundled",
        }
    }
}

impl std::fmt::Display for SkillSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 嵌套 metadata(对齐 openclaw `metadata.openclaw.*`)。
#[derive(Debug, Clone, Default)]
pub struct SkillMetadata {
    /// `metadata.<key>: value` —— emoji / homepage / category / os
    pub flat: HashMap<String, String>,
    /// `metadata.<key>.<sub>: value` —— requires.bins / install.kind
    pub nested: HashMap<String, HashMap<String, String>>,
    /// `metadata.tags: [a, b, c]` 风格单值列表
    pub lists: HashMap<String, Vec<String>>,
}

/// 全字段 frontmatter(对齐 jiuwenswarm 6 字段 + openclaw metadata + pi/atomcode 顶层)。
#[derive(Debug, Clone, Default)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: String, // 必填(可由 fallback_description 兜底)
    pub version: Option<String>,
    pub license: Option<String>,
    pub compatibility: Option<String>, // 例如 "claudecode>=1.0"
    pub allowed_tools: Vec<String>,
    pub user_invocable: bool, // 默认 true
    pub source: Option<String>, // 跨引擎标识("claudecode" / "opencode" 等)
    pub metadata: SkillMetadata,
}

impl SkillFrontmatter {
    /// ALLOWED_KEYS 白名单(对齐 jiuwenswarm `validate_stage.py:32-45`)。
    /// 未知 key 不阻断,只忽略(对齐 frontmatter.rs 现有策略)。
    pub fn allowed_top_level_keys() -> &'static [&'static str] {
        &[
            "name",
            "description",
            "version",
            "license",
            "allowed-tools",
            "metadata",
            "compatibility",
            "user-invocable",
            "source",
            "tags",
            "categories",
        ]
    }
}

/// Skill 完整结构。
#[derive(Debug, Clone)]
pub struct Skill {
    pub frontmatter: SkillFrontmatter,
    pub body: String,
    pub skill_dir: PathBuf,
    pub source_path: PathBuf,
    pub source: SkillSource,
}

impl Skill {
    /// 取 name(优先 frontmatter,缺省回退文件名 stem)。
    pub fn name(&self) -> &str {
        self.frontmatter
            .name
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                self.source_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
            })
    }

    pub fn description(&self) -> &str {
        &self.frontmatter.description
    }

    pub fn allowed_tools(&self) -> &[String] {
        &self.frontmatter.allowed_tools
    }
}

/// 加载诊断(对齐 pi 的 diagnostics 收集策略)。
#[derive(Debug, Clone, Default)]
pub struct SkillLoadDiagnostics {
    pub loaded: usize,
    pub ignored: Vec<(PathBuf, String)>,
}

/// 从单文件加载 Skill(顶层 `---` frontmatter + 任意长度 body)。
pub fn load_from_file(path: &Path, source: SkillSource) -> Result<Skill, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    if raw.len() > MAX_BODY_BYTES {
        return Err(format!(
            "body {} bytes > MAX_BODY_BYTES({MAX_BODY_BYTES})",
            raw.len()
        ));
    }
    load_from_str(&raw, path, source)
}

/// 从 raw 字符串加载(供 bundled `include_str!` 复用)。
pub fn load_from_str(raw: &str, path: &Path, source: SkillSource) -> Result<Skill, String> {
    let fm = parse_full_frontmatter(raw);
    let body = extract_body(raw);

    // name: frontmatter 优先,缺省回退文件名 stem
    let name = fm
        .name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string()
        });
    validate_name(&name)?;

    // description: 缺省时回退首段
    let mut fm = fm;
    if fm.description.trim().is_empty() {
        fm.description = frontmatter::fallback_description(&body, 200);
    }
    if fm.description.chars().count() > MAX_DESC_LEN {
        return Err(format!(
            "description '{}' > MAX_DESC_LEN({MAX_DESC_LEN})",
            truncate_chars(&fm.description, 32)
        ));
    }

    let skill_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    Ok(Skill {
        frontmatter: fm,
        body,
        skill_dir,
        source_path: path.to_path_buf(),
        source,
    })
}

/// 解析全字段 frontmatter。
///
/// 策略:
/// 1. 先用 `frontmatter::parse` 取顶层 `key: value` 平面 map;
/// 2. 顶层白名单直接落入 `SkillFrontmatter`;
/// 3. 嵌套 `metadata.*` 行走简化二级解析(`metadata.<key>` 扁平 / `metadata.<key>.<sub>` 嵌套);
/// 4. `tags` / `categories` 走单值列表解析(逗号/空白分隔);
/// 5. 其它未知顶层 key 静默忽略(对齐 frontmatter.rs 策略)。
fn parse_full_frontmatter(raw: &str) -> SkillFrontmatter {
    let (map, _body) = frontmatter::parse(raw);
    let mut fm = SkillFrontmatter::default();

    // 顶层字段
    fm.name = map.get("name").cloned().filter(|s| !s.is_empty());
    fm.description = map.get("description").cloned().unwrap_or_default();
    fm.version = map.get("version").cloned();
    fm.license = map.get("license").cloned();
    fm.compatibility = map.get("compatibility").cloned();
    fm.source = map.get("source").cloned();
    fm.user_invocable = frontmatter::parse_bool(map.get("user-invocable").map(String::as_str), true);
    fm.allowed_tools = parse_list_field(map.get("allowed-tools"));

    // 嵌套 metadata.<key>.<sub>
    for (k, v) in &map {
        if let Some(rest) = k.strip_prefix("metadata.") {
            if let Some((sub, deep)) = rest.split_once('.') {
                fm.metadata
                    .nested
                    .entry(sub.to_string())
                    .or_default()
                    .insert(deep.to_string(), v.clone());
            } else {
                fm.metadata.flat.insert(rest.to_string(), v.clone());
            }
        }
    }

    // tags / categories 单值列表(允许顶层直接出现,也对齐 metadata.tags 嵌套形式)
    if let Some(tags) = map.get("tags") {
        fm.metadata.lists.insert("tags".to_string(), parse_list(tags));
    }
    if let Some(cats) = map.get("categories") {
        fm.metadata
            .lists
            .insert("categories".to_string(), parse_list(cats));
    }
    if let Some(nested_tags) = fm.metadata.nested.remove("tags") {
        for (k, v) in nested_tags {
            let mut list = fm.metadata.lists.entry("tags".to_string()).or_default();
            if !list.contains(&v) {
                list.push(v.clone());
            }
            let _ = k;
        }
    }

    fm
}

fn parse_list_field(raw: Option<&String>) -> Vec<String> {
    raw.map(|s| parse_list(s)).unwrap_or_default()
}

fn parse_list(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    // 支持 `[a, b, c]` 风格(对齐 openclaw / jiuwenswarm)
    let inner = if trimmed.starts_with('[') && trimmed.ends_with(']') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    inner
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

fn extract_body(raw: &str) -> String {
    let (_map, body) = frontmatter::parse(raw);
    body
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(format!("name '{name}' must be 1-{MAX_NAME_LEN} chars"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("name '{name}' has invalid characters"));
    }
    if name.starts_with('-') || name.starts_with('_') || name.ends_with('-') {
        return Err(format!("name '{name}' has bad prefix/suffix"));
    }
    Ok(())
}

fn truncate_chars(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

// =================== 变量替换 ===================

/// Skill body 展开(对齐 atomcode skill.rs:32-92 的 match_substitution)。
///
/// 支持:
/// - `$ARGUMENTS` —— 全量参数
/// - `$1`..`$9` —— 位置参数;`$10` 原样保留(避免 `$1` + `0` 与 `$10` 歧义)
/// - `${CLAUDE_SKILL_DIR}` / `${LAEW_SKILL_DIR}` —— skill 文件目录
/// - `` !`cmd`` —— shell 注入(信任用户)
/// - **atomcode 行为**:body 不含 `$ARGUMENTS` 时若带参数,末尾追加 `ARGUMENTS: <参数>` 块
pub fn expand(skill: &Skill, arguments: &str, _session_id: &str) -> String {
    let positional: Vec<&str> = arguments.split_whitespace().collect();
    let skill_dir = skill.skill_dir.to_string_lossy().into_owned();
    let body = skill.body.clone();

    // 占位符检测:必须在替换前判断,否则替换后看不到原 body 里的 $ARGUMENTS
    let has_arg_placeholder = body.contains("$ARGUMENTS");

    let mut result = apply_substitutions(&body, arguments, &positional, &skill_dir);

    if !has_arg_placeholder && !arguments.trim().is_empty() {
        result.push_str("\n\nARGUMENTS: ");
        result.push_str(arguments);
    }

    result
}

/// 单遍左到右扫描,替换值不再二次展开(避免 `$1` 被再次展开成 `$2`)。
fn apply_substitutions(
    body: &str,
    arguments: &str,
    positional: &[&str],
    skill_dir: &str,
) -> String {
    let mut out = String::with_capacity(body.len() + 64);
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];

        // ` $ARGUMENTS`
        if body[i..].starts_with("$ARGUMENTS") {
            out.push_str(arguments);
            i += "$ARGUMENTS".len();
            continue;
        }

        // `${CLAUDE_SKILL_DIR}` / `${LAEW_SKILL_DIR}`
        if body[i..].starts_with("${CLAUDE_SKILL_DIR}") {
            out.push_str(skill_dir);
            i += "${CLAUDE_SKILL_DIR}".len();
            continue;
        }
        if body[i..].starts_with("${LAEW_SKILL_DIR}") {
            out.push_str(skill_dir);
            i += "${LAEW_SKILL_DIR}".len();
            continue;
        }

        // ` $N`(N ∈ 1..9)
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            // 取最长 digit run
            let mut end = i + 2;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            let digit_run = &body[i + 1..end];
            let n = digit_run.parse::<usize>().unwrap_or(0);
            if digit_run.len() == 1 && (1..=9).contains(&n) {
                if let Some(arg) = positional.get(n - 1) {
                    out.push_str(arg);
                } else {
                    // 越位:保留字面 $N 让 LLM 看到
                    out.push('$');
                    out.push_str(digit_run);
                }
            } else {
                // $10 / $11 等多位数 → 原样保留
                out.push('$');
                out.push_str(digit_run);
            }
            i = end;
            continue;
        }

        // `` !`cmd` `` —— shell 注入(单遍扫描,捕获 `` ` `` 配对)
        if b == b'!' && i + 1 < bytes.len() && bytes[i + 1] == b'`' {
            if let Some(end) = find_unescaped_backtick(&body[i + 2..]) {
                let cmd = &body[i + 2..i + 2 + end];
                let cmd_trim = cmd.trim();
                if !cmd_trim.is_empty() {
                    match run_shell_inline(cmd_trim) {
                        Ok(stdout) => out.push_str(&stdout),
                        Err(e) => out.push_str(&format!("[shell error: {e}]")),
                    }
                }
                i += 2 + end + 1;
                continue;
            }
        }

        // 普通字符
        let cur = body[i..].chars().next().unwrap_or(' ');
        out.push(cur);
        i += cur.len_utf8();
    }
    out
}

fn find_unescaped_backtick(s: &str) -> Option<usize> {
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 同步 shell 注入(本轮单遍保证完整度**就地**完成,信任 Skill 作者)。
/// 不走 BashTool(避免过度耦合);失败时返回 stderr 摘要,**不阻断**后续替换。
fn run_shell_inline(cmd: &str) -> Result<String, String> {
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .output()
        .map_err(|e| format!("spawn bash: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "exit {:?} stderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_path() -> PathBuf {
        PathBuf::from("/tmp/test_skill.md")
    }

    fn make_skill(body: &str, name: &str) -> Skill {
        Skill {
            frontmatter: SkillFrontmatter {
                name: Some(name.to_string()),
                description: "test skill".into(),
                ..Default::default()
            },
            body: body.to_string(),
            skill_dir: PathBuf::from("/tmp"),
            source_path: PathBuf::from(format!("/tmp/{name}.md")),
            source: SkillSource::User,
        }
    }

    #[test]
    fn validate_name_accepts_alnum_dash_under() {
        assert!(validate_name("git-commit").is_ok());
        assert!(validate_name("pdf_tools").is_ok());
        assert!(validate_name("a1").is_ok());
    }

    #[test]
    fn validate_name_rejects_bad() {
        assert!(validate_name("").is_err());
        assert!(validate_name("-leading").is_err());
        assert!(validate_name("_underscore_lead").is_err());
        assert!(validate_name("trailing-").is_err());
        assert!(validate_name("with space").is_err());
        assert!(validate_name("中文名").is_err());
        assert!(validate_name(&"x".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn load_from_str_basic_frontmatter() {
        let raw = "---\nname: git-commit\ndescription: 按规范提交代码\n---\n# Body\n使用 $ARGUMENTS。\n";
        let skill = load_from_str(raw, &test_path(), SkillSource::User).unwrap();
        assert_eq!(skill.name(), "git-commit");
        assert_eq!(skill.description(), "按规范提交代码");
        assert!(skill.body.contains("使用 $ARGUMENTS"));
    }

    #[test]
    fn load_from_str_name_fallback_to_filename() {
        let raw = "---\ndescription: 无 name 兜底\n---\nbody";
        let p = PathBuf::from("/tmp/auto-name.md");
        let skill = load_from_str(raw, &p, SkillSource::Project).unwrap();
        assert_eq!(skill.name(), "auto-name");
    }

    #[test]
    fn load_from_str_description_fallback_to_first_line() {
        let raw = "---\nname: x\n---\n首段描述兜底测试\n\n第二段被忽略";
        let skill = load_from_str(raw, &test_path(), SkillSource::User).unwrap();
        assert!(skill.description().starts_with("首段"));
    }

    #[test]
    fn load_from_str_nested_metadata_flat_and_nested() {
        let raw = "\
---
description: 嵌套 metadata 测试
metadata.emoji: \"🧩\"
metadata.os: darwin
metadata.requires.bins: claude
metadata.install.kind: brew
tags: [dev, productivity]
---
body";
        let skill = load_from_str(raw, &test_path(), SkillSource::User).unwrap();
        assert_eq!(skill.frontmatter.metadata.flat.get("emoji").unwrap(), "🧩");
        assert_eq!(skill.frontmatter.metadata.flat.get("os").unwrap(), "darwin");
        let req = skill.frontmatter.metadata.nested.get("requires").unwrap();
        assert_eq!(req.get("bins").unwrap(), "claude");
        let inst = skill.frontmatter.metadata.nested.get("install").unwrap();
        assert_eq!(inst.get("kind").unwrap(), "brew");
        assert_eq!(
            skill.frontmatter.metadata.lists.get("tags").unwrap(),
            &vec!["dev".to_string(), "productivity".to_string()]
        );
    }

    #[test]
    fn load_from_str_allowed_tools_list() {
        let raw = "---\ndescription: x\nallowed-tools: Read, Write , Bash\n---\nb";
        let skill = load_from_str(raw, &test_path(), SkillSource::User).unwrap();
        assert_eq!(skill.allowed_tools(), &["Read", "Write", "Bash"]);
    }

    #[test]
    fn load_from_str_rejects_description_too_long() {
        let long = "x".repeat(MAX_DESC_LEN + 10);
        let raw = format!("---\ndescription: {long}\n---\nbody");
        let r = load_from_str(&raw, &test_path(), SkillSource::User);
        assert!(r.is_err());
    }

    #[test]
    fn load_from_str_absent_frontmatter_uses_raw_as_body() {
        let raw = "无 frontmatter 的纯 body";
        let skill = load_from_str(raw, &test_path(), SkillSource::User).unwrap();
        assert_eq!(skill.name(), "test_skill");
        assert!(skill.body.contains("纯 body"));
        assert!(skill.description().contains("无 frontmatter") || skill.description().is_empty());
    }

    #[test]
    fn expand_replaces_arguments_var() {
        let s = make_skill("使用 $ARGUMENTS 完成提交。", "git-commit");
        let out = expand(&s, "feat: 新增 skill", "sid");
        assert_eq!(out, "使用 feat: 新增 skill 完成提交。");
    }

    #[test]
    fn expand_replaces_positional_vars() {
        let s = make_skill("$1 先 $2 然后 $3", "x");
        let out = expand(&s, "a b c", "sid");
        // atomcode 行为:无 $ARGUMENTS 占位符时末尾追加 ARGUMENTS 块
        assert_eq!(out, "a 先 b 然后 c\n\nARGUMENTS: a b c");
    }

    #[test]
    fn expand_positional_oob_keeps_literal() {
        let s = make_skill("$5 不存在", "x");
        let out = expand(&s, "a b", "sid");
        // $5 越位保留字面;body 无 $ARGUMENTS → 追加 ARGUMENTS
        assert_eq!(out, "$5 不存在\n\nARGUMENTS: a b");
    }

    #[test]
    fn expand_ten_dollar_keeps_literal() {
        let s = make_skill("$10 原样保留", "x");
        let out = expand(&s, "a b c d e f g h i j", "sid");
        // $10 多位数原样保留;body 无 $ARGUMENTS → 追加 ARGUMENTS
        assert_eq!(out, "$10 原样保留\n\nARGUMENTS: a b c d e f g h i j");
    }

    #[test]
    fn expand_replaces_skill_dir_var() {
        let s = make_skill("位于 ${CLAUDE_SKILL_DIR} 或 ${LAEW_SKILL_DIR}", "x");
        let out = expand(&s, "", "sid");
        assert!(out.contains("/tmp"));
    }

    #[test]
    fn expand_atomcode_append_arguments_when_no_placeholder() {
        // body 无 $ARGUMENTS 占位符,但带参数 → 末尾追加 ARGUMENTS 块
        let s = make_skill("纯 body 不带占位符。", "x");
        let out = expand(&s, "feat: 新功能", "sid");
        assert!(out.contains("纯 body 不带占位符。"));
        assert!(out.contains("\n\nARGUMENTS: feat: 新功能"));
    }

    #[test]
    fn expand_atomcode_no_append_when_empty_arguments() {
        let s = make_skill("纯 body", "x");
        let out = expand(&s, "", "sid");
        assert_eq!(out, "纯 body");
        assert!(!out.contains("ARGUMENTS:"));
    }

    #[test]
    fn expand_shell_injection_runs_bash() {
        let s = make_skill("hostname = !`echo unit-test-host`", "x");
        let out = expand(&s, "", "sid");
        assert!(out.contains("unit-test-host"), "got: {out}");
    }

    #[test]
    fn expand_shell_injection_handles_failure() {
        // 命令本身退出非零(此处 `false` 是末命令,整段 exit=1)
        let s = make_skill("x = !`false`", "x");
        let out = expand(&s, "", "sid");
        assert!(out.contains("[shell error:"), "got: {out}");
        assert!(out.contains("exit"), "got: {out}");
    }

    #[test]
    fn expand_shell_injection_last_command_zero_exit_returns_stdout() {
        // `false; echo oops 1>&2` —— 最后 `echo` 退出 0,整段 exit=0,走 Ok 路径
        let s = make_skill("x = !`false; echo oops 1>&2`", "x");
        let out = expand(&s, "", "sid");
        assert!(!out.contains("[shell error:"), "整段应成功,got: {out}");
    }

    #[test]
    fn expand_does_not_recursively_expand() {
        // $1 替换成 "y" 后,y 不再被当 $ 开头再次展开
        let s = make_skill("$1 和 $ARGUMENTS", "x");
        let out = expand(&s, "$ARGUMENTS", "sid");
        assert_eq!(out, "$ARGUMENTS 和 $ARGUMENTS");
    }
}