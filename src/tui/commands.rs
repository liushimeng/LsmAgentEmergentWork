//! 自定义斜杠命令(第十八轮 D2):项目级/用户级 Markdown Prompt 模板。
//!
//! 命令文件约定(对齐 claudecode `.claude/commands` 与 openclaw/pi `prompts/` 双层发现):
//! - 用户级:`~/.laew/commands/*.md`
//! - 项目级:`{工作目录}/.laew/commands/*.md`
//! - 同名优先级:**用户级 > 项目级**(对齐 claudecode「防项目仓库恶意命令覆盖用户偏好」);
//!   内置命令(`/help` `/provider` 等)永不遮蔽(路由顺序:内置 match → 自定义命令)。
//!
//! 文件格式:Markdown + frontmatter(YAML 子集,仅 `description` / `argument-hint` 两个 key):
//!
//! ```md
//! ---
//! description: 代码评审
//! argument-hint: [file]
//! ---
//! 请评审 $ARGUMENTS,重点关注 $1
//! ```
//!
//! 占位符:`$ARGUMENTS`(全量参数)与 `$1`-`$9`(位置参数,按空白分割);模板无占位符但
//! 用户带参调用时,末尾追加 `ARGUMENTS:` 块兜底(claudecode 语义)。
//! 设计见 `tmpPlan/2026-09-10_01-自定义斜杠命令与会话导出方案.md`。

use std::path::{Path, PathBuf};

/// 单个自定义命令(从 Markdown 文件加载)。
#[derive(Debug, Clone)]
pub struct CustomCommand {
    /// 命令名(文件 stem,合法字符集 [A-Za-z0-9_-])。
    pub name: String,
    /// 命令描述(补全列表显示;缺失时取正文首行按字符截 60)。
    pub description: String,
    /// 参数提示(如 "[name]";可空)。
    pub argument_hint: String,
    /// 来源文件绝对路径。
    pub source: PathBuf,
    /// frontmatter 之后的正文模板(占位符未渲染)。
    pub body: String,
}

/// frontmatter 解析结果(仅识别两个 key,其余忽略)。
#[derive(Debug, Default, Clone)]
struct Frontmatter {
    description: Option<String>,
    argument_hint: Option<String>,
}

/// 内置命令名与别名(自定义命令不得遮蔽)。
pub const BUILTIN_NAMES: &[&str] = &[
    "help", "h", "?", "exit", "quit", "q", "clear", "c", "new", "n", "model", "provider", "p",
    "export", "commands",
];

/// 扫描两级命令目录,返回去重后的自定义命令列表(用户级优先)。
///
/// 读取/解析失败的文件静默跳过 —— 自定义命令是增强能力,不允许阻塞 TUI 启动。
pub fn discover(work_dir: &Path) -> Vec<CustomCommand> {
    discover_with_home(work_dir, &home_commands_dir())
}

/// 扫描两级命令目录中「存在 .md 文件但文件名非法被跳过」的项(诊断用)。
/// 2026-09-10 第 23 轮:`/commands` 空列表时列出被跳过文件与原因,
/// 把静默失败变成可诊断(此前中文命令名被过滤后零线索)。
pub fn scan_invalid_names(work_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for dir in [work_dir.join(".laew").join("commands"), home_commands_dir()] {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !valid_command_name(stem) {
                out.push(format!(
                    "{} — 命令名含非法字符(仅允许字母/数字/中文/下划线/连字符)",
                    path.display()
                ));
            }
        }
    }
    out
}

/// [`discover`] 的可测核心:显式传入用户级目录(测试不污染进程级 HOME 环境变量)。
pub fn discover_with_home(work_dir: &Path, user_dir: &Path) -> Vec<CustomCommand> {
    let mut out: Vec<CustomCommand> = Vec::new();
    // 项目级先入列,用户级后入列并按名去重覆盖(用户级优先)
    let project_dir = work_dir.join(".laew").join("commands");
    load_dir(&project_dir, &mut out);
    load_dir(user_dir, &mut out);
    out
}

/// 用户级命令目录 `~/.laew/commands`(HOME 缺失时退化为空路径,扫描自然为空)。
fn home_commands_dir() -> PathBuf {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h).join(".laew").join("commands"),
        _ => PathBuf::new(),
    }
}

/// 扫描单个目录下的 `*.md`(仅顶层,不递归);同名后者覆盖前者(insert 语义)。
fn load_dir(dir: &Path, out: &mut Vec<CustomCommand>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_command_name(stem) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (fm, body) = parse_frontmatter(&raw);
        let description = fm
            .description
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| fallback_description(&body));
        let cmd = CustomCommand {
            name: stem.to_string(),
            description,
            argument_hint: fm.argument_hint.unwrap_or_default(),
            source: path,
            body,
        };
        // 同名覆盖:后加载的(用户级)替换先加载的(项目级)
        match out.iter().position(|c| c.name == cmd.name) {
            Some(i) => out[i] = cmd,
            None => out.push(cmd),
        }
    }
}

/// 命令名合法字符集(防止路径怪字符进入斜杠命令语义)。
fn valid_command_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            // Unicode 字母数字(含中日韩)允许 —— 2026-09-10 第 23 轮:此前仅 ASCII,
            // 中文命令名(如 tmux速查.md)被静默过滤,/commands 与补全均不可见,零线索。
            // `is_alphanumeric` 天然排除空格/路径分隔符/控制字符,安全性不变。
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// 解析 frontmatter(手写 YAML 子集):首行 `---` 起,到下一个 `---` 止。
/// 返回 (frontmatter, 正文);无 frontmatter / 未闭合时返回 (默认, 原文)。
fn parse_frontmatter(raw: &str) -> (Frontmatter, String) {
    let mut lines = raw.split_inclusive('\n');
    // 首行必须是裸 `---`
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return (Frontmatter::default(), raw.to_string());
    }
    let mut fm = Frontmatter::default();
    // 已消费字节偏移(split_inclusive 保留行尾 \n,累计即 raw 内偏移)
    let mut consumed = first.len();
    for line in lines.by_ref() {
        consumed += line.len();
        let t = line.trim_end();
        if t == "---" {
            return (fm, raw[consumed.min(raw.len())..].to_string());
        }
        if let Some((key, value)) = t.split_once(':') {
            let key = key.trim();
            let value = unquote(value.trim());
            match key {
                "description" => fm.description = Some(value),
                "argument-hint" | "argument_hint" => fm.argument_hint = Some(value),
                _ => {}
            }
        }
    }
    // frontmatter 未闭合:按无 frontmatter 处理(全文当正文,不丢内容)
    (Frontmatter::default(), raw.to_string())
}

/// 剥离成对的引号(`"..."` / `'...'`)。
fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (f, l) = (bytes[0], bytes[bytes.len() - 1]);
        if (f == b'"' && l == b'"') || (f == b'\'' && l == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// description 兜底:正文首个非空行,按字符截 60(UTF-8 安全,对齐 openclaw)。
fn fallback_description(body: &str) -> String {
    let first = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        out.push('…');
    }
    out
}

/// 渲染命令模板:占位符替换。
///
/// - `$1` → `$9`:`split_whitespace` 位置参数(1-based),缺失替换为空串;
///   `$10`/`$0` 等非支持编号**原样保留**(不污染);
/// - `$ARGUMENTS`:全量参数(trim 后);
/// - 模板无任何占位符但 args 非空 → 末尾追加 `ARGUMENTS:` 块。
pub fn render(cmd: &CustomCommand, args: &str) -> String {
    let args = args.trim();
    let parts: Vec<&str> = args.split_whitespace().collect();
    let (after_positional, had_positional) = replace_positional(&cmd.body, &parts);
    let has_arguments = cmd.body.contains("$ARGUMENTS");
    let mut out = after_positional.replace("$ARGUMENTS", args);
    if !had_positional && !has_arguments && !args.is_empty() {
        out.push_str("\n\nARGUMENTS:\n");
        out.push_str(args);
    }
    out.trim().to_string()
}

/// 字节扫描式位置参数替换。返回 (结果, 是否发生替换)。
///
/// `$` 后跟连续数字整体解析编号:1-9 替换为对应参数(缺失→空串),
/// 其余(`$0`/`$10`…)原样保留 —— 避免 `str::replace("$1")` 把 `$10` 污染成 `<参数>0`。
fn replace_positional(body: &str, parts: &[&str]) -> (String, bool) {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0usize;
    let mut replaced = false;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            // 收集 `$` 后的连续数字段
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let num = body[i + 1..j].parse::<usize>().unwrap_or(0);
            if (1..=9).contains(&num) {
                out.push_str(parts.get(num - 1).copied().unwrap_or(""));
                replaced = true;
            } else {
                // 非支持编号:整个数字段原样保留
                out.push_str(&body[i..j]);
            }
            i = j;
            continue;
        }
        // 非 `$数字`:按字符推进(UTF-8 边界安全)
        let ch = body[i..].chars().next().expect("char at boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, replaced)
}

/// 按命令名渲染(路由入口):命中返回渲染后的提示词,未命中返回 None。
pub fn render_by_name(commands: &[CustomCommand], name: &str, args: &str) -> Option<String> {
    commands
        .iter()
        .find(|c| c.name == name)
        .map(|c| render(c, args))
}

/// 判断命令名是否被内置命令占用(自定义命令不得遮蔽内置)。
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(body: &str) -> CustomCommand {
        CustomCommand {
            name: "test".into(),
            description: String::new(),
            argument_hint: String::new(),
            source: PathBuf::from("/tmp/x.md"),
            body: body.into(),
        }
    }

    #[test]
    fn frontmatter_standard() {
        let raw = "---\ndescription: 代码评审\nargument-hint: [file]\n---\n请评审正文\n";
        let (fm, body) = parse_frontmatter(raw);
        assert_eq!(fm.description.as_deref(), Some("代码评审"));
        assert_eq!(fm.argument_hint.as_deref(), Some("[file]"));
        assert_eq!(body.trim(), "请评审正文");
    }

    #[test]
    fn frontmatter_quoted_value() {
        let raw = "---\ndescription: \"带 引 号\"\n---\nbody";
        let (fm, _) = parse_frontmatter(raw);
        assert_eq!(fm.description.as_deref(), Some("带 引 号"));
    }

    #[test]
    fn frontmatter_absent() {
        let raw = "没有 frontmatter 的正文";
        let (fm, body) = parse_frontmatter(raw);
        assert!(fm.description.is_none());
        assert_eq!(body, raw);
    }

    #[test]
    fn frontmatter_unclosed_falls_back_to_body() {
        let raw = "---\ndescription: 未闭合\n正文当命令体";
        let (fm, body) = parse_frontmatter(raw);
        // 未闭合:整文当正文,不丢内容
        assert!(fm.description.is_none());
        assert!(body.contains("description"));
    }

    #[test]
    fn frontmatter_unknown_keys_ignored() {
        let raw = "---\nmodel: opus\nallowed-tools: [Bash]\ndescription: d\n---\n正文";
        let (fm, body) = parse_frontmatter(raw);
        assert_eq!(fm.description.as_deref(), Some("d"));
        assert!(fm.argument_hint.is_none());
        assert_eq!(body.trim(), "正文");
    }

    #[test]
    fn fallback_description_truncates_by_chars() {
        // 100 个全角字符 → 截 60 + 省略号(UTF-8 边界安全)
        let body = "评".repeat(100);
        let d = fallback_description(&body);
        assert_eq!(d.chars().count(), 61);
        assert!(d.ends_with('…'));
    }

    #[test]
    fn fallback_description_skips_blank_lines() {
        let d = fallback_description("\n\n  \n第一行有效\n第二行");
        assert_eq!(d, "第一行有效");
    }

    #[test]
    fn render_arguments_placeholder() {
        let c = cmd("向 $ARGUMENTS 问好");
        assert_eq!(render(&c, "world laew"), "向 world laew 问好");
    }

    #[test]
    fn render_positional_placeholders() {
        let c = cmd("一:$1 二:$2 缺:$5");
        assert_eq!(render(&c, "a b c"), "一:a 二:b 缺:");
    }

    #[test]
    fn render_dollar_ten_not_polluted() {
        // `$10` 中的 `$1` 不得被替换(逆序 + 仅当占位符存在时替换)
        let c = cmd("编号 $10 与 $1");
        assert_eq!(render(&c, "A"), "编号 $10 与 A");
    }

    #[test]
    fn render_no_placeholder_appends_arguments_block() {
        let c = cmd("固定模板");
        let out = render(&c, "extra args");
        assert!(out.starts_with("固定模板"));
        assert!(out.contains("ARGUMENTS:\nextra args"));
    }

    #[test]
    fn render_no_placeholder_no_args_unchanged() {
        let c = cmd("固定模板");
        assert_eq!(render(&c, "   "), "固定模板");
    }

    #[test]
    fn valid_command_name_rules() {
        assert!(valid_command_name("review"));
        assert!(valid_command_name("git-commit"));
        assert!(valid_command_name("cmd_2"));
        // Unicode(中日韩)命令名合法(第 23 轮放宽,原 ASCII-only 静默过滤中文)
        assert!(valid_command_name("tmux速查"));
        assert!(valid_command_name("代码审查"));
        assert!(!valid_command_name(""));
        assert!(!valid_command_name("a b"));
        assert!(!valid_command_name("a/b"));
        assert!(!valid_command_name("a.b"));
    }

    #[test]
    fn builtin_shadow_check() {
        assert!(is_builtin("help"));
        assert!(is_builtin("q"));
        assert!(is_builtin("export"));
        assert!(!is_builtin("review"));
    }

    #[test]
    fn render_by_name_hits_and_misses() {
        let mut c = cmd("模板 $ARGUMENTS");
        c.name = "review".into();
        let list = vec![c];
        assert_eq!(
            render_by_name(&list, "review", "x").as_deref(),
            Some("模板 x")
        );
        assert!(render_by_name(&list, "nope", "x").is_none());
    }

    #[test]
    #[test]
    fn scan_invalid_names_reports_only_illegal_md() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let dir = tmp.path().join(".laew").join("commands");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("正常命令.md"), "ok").unwrap();
        std::fs::write(dir.join("tmux速查.md"), "ok").unwrap(); // 中文合法,不报
        std::fs::write(dir.join("bad.name.md"), "非法").unwrap(); // 点号非法,报
        std::fs::write(dir.join("ignored.txt"), "非 md 不报").unwrap();
        let out = scan_invalid_names(tmp.path());
        assert_eq!(out.len(), 1, "仅 bad.name.md 被报告: {out:?}");
        assert!(out[0].contains("bad.name.md"));
    }

    #[test]
    fn discover_loads_cjk_command_name() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let dir = tmp.path().join(".laew").join("commands");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("tmux速查.md"), "---\ndescription: 速查\n---\n正文").unwrap();
        let cmds = discover_with_home(tmp.path(), &tmp.path().join("nonexistent"));
        assert!(cmds.iter().any(|c| c.name == "tmux速查"), "中文名应被加载: {cmds:?}");
    }

    fn discover_two_level_priority_and_dedup() {
        // 临时目录:项目级 + 用户级同名,用户级应胜出
        let tmp = tempfile::tempdir().expect("tmpdir");
        let work = tmp.path().join("work");
        let home = tmp.path().join("home");
        for (base, desc) in [
            (&work, "项目级版本"),
            (&home, "用户级版本"),
        ] {
            let dir = base.join(".laew").join("commands");
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::fs::write(
                dir.join("dup.md"),
                format!("---\ndescription: {desc}\n---\n内容"),
            )
            .expect("write");
            // 项目级额外一个独立命令
            if base == &work {
                std::fs::write(dir.join("only-project.md"), "仅项目级").unwrap();
                // 非法 stem 不加载
                std::fs::write(dir.join("bad.name.md"), "非法").unwrap();
                // 非 md 不加载
                std::fs::write(dir.join("ignored.txt"), "忽略").unwrap();
            }
        }
        // 用户级目录显式传入(不污染进程级 HOME,避免并行测试互扰);
        // user_dir 语义与 home_commands_dir() 一致:完整命令目录路径
        let cmds = discover_with_home(&work, &home.join(".laew").join("commands"));

        let dup = cmds.iter().find(|c| c.name == "dup").expect("dup");
        assert_eq!(dup.description, "用户级版本");
        assert!(dup.source.starts_with(&home));
        assert!(cmds.iter().any(|c| c.name == "only-project"));
        assert!(!cmds.iter().any(|c| c.name == "bad.name"));
        assert_eq!(cmds.iter().filter(|c| c.name == "dup").count(), 1);
    }
}
