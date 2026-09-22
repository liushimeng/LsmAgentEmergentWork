//! 自定义子 Agent 类型(定义文件驱动)—— 第 115 轮(2026-09-22)新增。
//!
//! 设计见 `docs/自感知SubAgent自定义类型与运行持久化/01-设计与解决方案.md` §4。
//!
//! **动机**:D114 的 6 类子 Agent 由 [`SubAgentType`] 枚举硬编码,用户想加一个
//! 「前端审查」「数据库迁移」角色只能改 Rust 重编译;且 `SubAgent` 工具 schema 里
//! `agent_type` 是 JSON Schema `enum`,配合 `tool_schema_validator` 会**硬拒**任何
//! 外部类型。本轮把类型定义外化成 Markdown 文件(对齐 claudecode `.claude/agents/*.md`
//! 与插件 `agents/` 目录),并在实现层解析校验。
//!
//! **发现链**(与 D2 自定义斜杠命令 `tui/commands.rs` 同构,用户学一次即会两处):
//! 1. 项目级 `{工作目录}/.laew/agents/*.md`(随仓库提交,团队共享);
//! 2. 用户级 `~/.laew/agents/*.md`(**同名覆盖**项目级)。
//!
//! 静默跳过的情形(不阻塞主流程):文件名非法 / 读取失败 / 与内置类型 id 冲突;
//! 后两者会进 [`Discovery::ignored`],由 TUI `/agents` 面板如实展示(可解释性优先)。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::agent::project_context;
use crate::agent::self_awareness::{self as sa, SpawnPolicy, SubAgentType, READ_ONLY_TOOLS};

/// 定义文件所在目录名(`{工作目录}/.laew/agents`)。
pub const DIR: &str = ".laew";
/// 定义文件子目录名。
pub const SUBDIR: &str = "agents";
/// 名字/标签截断上限(字符)。
pub const MAX_LABEL_CHARS: usize = 24;
/// 描述截断上限(字符)。
pub const MAX_DESC_CHARS: usize = 200;
/// 正文(专属职责)截断上限(字符)。
pub const MAX_BODY_CHARS: usize = 6000;
/// 单次可加载的自定义类型上限(防极端目录撑爆名册与提示词)。
pub const MAX_DEFS: usize = 32;

// ===========================================================================
// 定义模型
// ===========================================================================

/// 定义文件来源级别(用户级优先)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefScope {
    /// `{工作目录}/.laew/agents`
    Project,
    /// `~/.laew/agents`
    User,
}

impl DefScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

/// 一个自定义子 Agent 类型定义(文件 -> 内存)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDef {
    /// 类型 id(= 文件 stem,全局唯一键)。
    pub id: String,
    /// 名册标签(缺省 `自定义`)。
    pub label: String,
    /// 名册描述(缺省取正文首行截 60)。
    pub description: String,
    /// 继承的内置类型(决定提示词骨架与默认工具面)。
    pub extends: SubAgentType,
    /// `tools` 白名单(空 = 取 `extends` 默认工具)。
    pub tools: Vec<String>,
    /// `readonly: true` 的便捷只读标记。
    pub read_only: bool,
    /// frontmatter 之后的正文(专属职责提示词)。
    pub body: String,
    /// 定义文件绝对/相对路径(面板展示 + 提示词溯源)。
    pub source: PathBuf,
    /// 来源级别。
    pub scope: DefScope,
}

impl AgentDef {
    /// 最终生效的「声明工具面」:显式 `tools` 优先,否则继承 `extends` 默认。
    pub fn declared_tools(&self) -> Vec<String> {
        if self.tools.is_empty() {
            self.extends
                .default_tools()
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            self.tools.clone()
        }
    }

    /// 类型 id -> Pascal 形式的 ASCII 化名字(用于子 Agent 名 / UA 可辨识)。
    pub fn pascal(&self) -> String {
        pascal_from_id(&self.id)
    }
}

/// 被忽略的定义(文件存在但不可用)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredDef {
    /// 文件名 stem(或路径)。
    pub id: String,
    /// 忽略原因(面板展示)。
    pub reason: String,
    /// 来源路径。
    pub source: String,
}

/// 一次发现的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    /// 可用定义(已按 id 去重,用户级优先;按 id 升序)。
    pub defs: Vec<AgentDef>,
    /// 被忽略的定义(供面板解释「为什么我写的文件没生效」)。
    pub ignored: Vec<IgnoredDef>,
}

// ===========================================================================
// 发现
// ===========================================================================

/// 扫描两级定义目录(`discover` 的可测核心:显式传入用户级目录,测试不污染 HOME)。
pub fn discover_with_home(work_dir: &Path, home_dir: &Path) -> Discovery {
    let mut raw: Vec<AgentDef> = Vec::new();
    let mut ignored: Vec<IgnoredDef> = Vec::new();
    // 项目级先入列,用户级后入列(同名覆盖 -> 用户级优先,与 D2 一致)
    load_dir(
        &work_dir.join(DIR).join(SUBDIR),
        DefScope::Project,
        &mut raw,
        &mut ignored,
    );
    load_dir(home_dir, DefScope::User, &mut raw, &mut ignored);

    let mut defs: Vec<AgentDef> = Vec::new();
    for d in raw {
        match defs.iter().position(|x| x.id == d.id) {
            Some(i) => defs[i] = d,
            None => defs.push(d),
        }
    }
    defs.sort_by(|a, b| a.id.cmp(&b.id));
    if defs.len() > MAX_DEFS {
        for d in defs.split_off(MAX_DEFS) {
            ignored.push(IgnoredDef {
                id: d.id,
                reason: format!("超出单次加载上限 {MAX_DEFS} 个自定义类型"),
                source: d.source.display().to_string(),
            });
        }
    }
    ignored.sort_by(|a, b| a.id.cmp(&b.id));
    Discovery { defs, ignored }
}

/// 按**当前工作目录**发现(进程内工作目录由 [`project_context::current_work_dir`]
/// 缓存;无缓存 = 无定义,零成本)。
///
/// 刻意**不做结果缓存**:一次发现 = 2 次 `read_dir` + N 个小文件读取,而调用点
/// 只有「构造系统提示词 / `action=list` / `/agents` 面板」三处(都是低频),
/// 换来的是「编辑定义文件后下一次调用即生效」。
pub fn discover_process() -> Discovery {
    let Some(work) = project_context::current_work_dir() else {
        return Discovery::default();
    };
    discover_in(work)
}

/// 按**指定工作目录**发现(用户级目录取 `HOME`;运行时可注入,便于单测)。
pub fn discover_in(work_dir: &Path) -> Discovery {
    discover_with_home(work_dir, &home_agents_dir())
}

/// 用户级定义目录 `~/.laew/agents`(HOME 缺失时退化为空路径,扫描自然为空)。
pub fn home_agents_dir() -> PathBuf {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h).join(DIR).join(SUBDIR),
        _ => PathBuf::new(),
    }
}

/// 扫描单个目录下的 `*.md`(仅顶层,不递归)。
fn load_dir(dir: &Path, scope: DefScope, out: &mut Vec<AgentDef>, ignored: &mut Vec<IgnoredDef>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
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
        let source = path.display().to_string();
        if !valid_type_id(stem) {
            ignored.push(IgnoredDef {
                id: stem.to_string(),
                reason: "文件名非法(仅允许字母/数字/中文/`-`/`_`)".into(),
                source,
            });
            continue;
        }
        // 内置类型(含别名)不可被遮蔽:命中即忽略(否则该定义永远不可达)
        if SubAgentType::parse(stem).is_some() {
            ignored.push(IgnoredDef {
                id: stem.to_string(),
                reason: "与内置类型(或其别名)id 冲突,内置优先".into(),
                source,
            });
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            ignored.push(IgnoredDef {
                id: stem.to_string(),
                reason: "文件读取失败".into(),
                source,
            });
            continue;
        };
        let id = stem.to_string();
        out.push(parse_def(&id, scope, path, &raw));
    }
}

/// 类型 id 合法字符集(与 D2 `valid_command_name` 同一规则)。
///
/// `is_alphanumeric()` 天然排除空格 / 路径分隔符 / 控制字符,并放行中日韩字符。
fn valid_type_id(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// 定义文件文本 -> [`AgentDef`](frontmatter 缺失时全部走缺省)。
fn parse_def(id: &str, scope: DefScope, source: PathBuf, raw: &str) -> AgentDef {
    let (fm, body) = crate::frontmatter::parse(raw);
    let get = |k: &str| fm.get(k).map(String::as_str).map(str::trim);

    let extends_explicit = get("extends")
        .filter(|s| !s.is_empty())
        .and_then(SubAgentType::parse);
    let read_only = crate::frontmatter::parse_bool(get("readonly"), false);
    let extends = match (extends_explicit, read_only) {
        (Some(t), _) => t,
        // `readonly: true` 且未显式声明 extends -> 只读侦察骨架(与工具面语义一致)
        (None, true) => SubAgentType::Explore,
        (None, false) => SubAgentType::GeneralPurpose,
    };

    let mut tools = parse_tool_list(get("tools"));
    if read_only {
        tools = if tools.is_empty() {
            READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect()
        } else {
            // 显式 tools 与只读集**取交集**(只读声明不能被 tools 绕过)
            tools
                .into_iter()
                .filter(|t| READ_ONLY_TOOLS.contains(&t.as_str()))
                .collect()
        };
    }

    let label = get("label")
        .filter(|s| !s.is_empty())
        .map(|s| clip(s, MAX_LABEL_CHARS))
        .unwrap_or_else(|| "自定义".to_string());
    let description = get("description")
        .filter(|s| !s.is_empty())
        .map(|s| clip(s, MAX_DESC_CHARS))
        .unwrap_or_else(|| crate::frontmatter::fallback_description(&body, 60));

    AgentDef {
        id: id.to_string(),
        label,
        description,
        extends,
        tools,
        read_only,
        body: clip(body.trim(), MAX_BODY_CHARS),
        source,
        scope,
    }
}

/// 字符安全截断(转发 [`crate::agent::dynamic_subagent::clip_chars`],避免 agent 域内
/// 出现两份「CJK 安全截断」实现)。
fn clip(s: &str, max: usize) -> String {
    crate::agent::dynamic_subagent::clip_chars(s, max)
}

/// 解析 `tools` 字段:逗号 / 分号 / 空白分隔,去空、去重、保序。
fn parse_tool_list(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for part in raw.split([',', ';', ' ', '\t', '\n', '、']) {
        let t = part.trim();
        if t.is_empty() || out.iter().any(|x| x == t) {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

/// `fe-reviewer` -> `FeReviewer`;无可保留 ASCII 字符时回退 `Custom`。
fn pascal_from_id(id: &str) -> String {
    let mut out = String::new();
    for seg in id.split(['-', '_', ' ', '.']) {
        let mut seg_out = String::new();
        for c in seg.chars() {
            if c.is_ascii_alphanumeric() {
                seg_out.push(c);
            }
        }
        let mut chars = seg_out.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        "Custom".to_string()
    } else {
        out
    }
}

// ===========================================================================
// 统一类型抽象(内置 / 自定义无差别)
// ===========================================================================

/// 解析后的子 Agent 类型。
#[derive(Debug, Clone)]
pub enum ResolvedAgentType {
    /// 内置 6 类之一。
    Builtin(SubAgentType),
    /// 定义文件声明的自定义类型。
    Custom(Arc<AgentDef>),
}

impl ResolvedAgentType {
    pub fn id(&self) -> &str {
        match self {
            Self::Builtin(t) => t.id(),
            Self::Custom(d) => &d.id,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Builtin(t) => t.label().to_string(),
            Self::Custom(d) => d.label.clone(),
        }
    }

    /// 名册描述(一句话场景说明)。
    pub fn role_hint(&self) -> String {
        match self {
            Self::Builtin(t) => t.role_hint().to_string(),
            Self::Custom(d) => d.description.clone(),
        }
    }

    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// 声明工具面(运行期还会与父策略 / 真实注册表取交集)。
    pub fn declared_tools(&self) -> Vec<String> {
        match self {
            Self::Builtin(t) => t.default_tools().iter().map(|s| s.to_string()).collect(),
            Self::Custom(d) => d.declared_tools(),
        }
    }

    /// 子 Agent 名前缀(`LsmAgentEmergentWork-SubAgent-{pascal}-{name}`)。
    pub fn pascal(&self) -> String {
        match self {
            Self::Builtin(t) => t.pascal().to_string(),
            Self::Custom(d) => d.pascal(),
        }
    }

    /// 定义文件来源(内置为 `None`)。
    pub fn source(&self) -> Option<&Path> {
        match self {
            Self::Builtin(_) => None,
            Self::Custom(d) => Some(d.source.as_path()),
        }
    }

    /// 子 Agent 系统提示词(自包含:子 Agent 看不到父对话)。
    pub fn system_prompt(&self, name: &str) -> String {
        match self {
            Self::Builtin(t) => t.system_prompt(name),
            Self::Custom(d) => d.system_prompt(name),
        }
    }
}

impl AgentDef {
    /// 自定义类型的系统提示词:内置骨架(extends)+ 定义文件正文 + 公共规则。
    pub fn system_prompt(&self, name: &str) -> String {
        let mut out = format!(
            "你是 laew 的动态子 Agent「{name}」,类型:{label}({id},自定义定义文件)。\n\n\
             ## 你的定位(继承自 `{base}`)\n{hint}\n",
            name = name,
            label = self.label,
            id = self.id,
            base = self.extends.id(),
            hint = self.extends.role_hint(),
        );
        if !self.body.trim().is_empty() {
            out.push_str(&format!(
                "\n## 你的专属职责(来自定义文件)\n{}\n",
                self.body.trim()
            ));
        }
        out.push_str(&sa::common_rules_section(self.read_only));
        out
    }
}

impl PartialEq for ResolvedAgentType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Builtin(a), Self::Builtin(b)) => a == b,
            (Self::Custom(a), Self::Custom(b)) => a.id == b.id,
            _ => false,
        }
    }
}

impl Eq for ResolvedAgentType {}

impl Serialize for ResolvedAgentType {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for ResolvedAgentType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        resolve(&raw).ok_or_else(|| de::Error::custom(format!("未知 agent_type `{raw}`")))
    }
}

// ===========================================================================
// 解析
// ===========================================================================

/// 类型 id -> 类型(先内置含别名,再自定义;都未命中返回 `None`)。
///
/// 自定义按**当前工作目录**发现(见 [`discover_process`])。
pub fn resolve(raw: &str) -> Option<ResolvedAgentType> {
    resolve_in(raw, &discover_process().defs)
}

/// [`resolve`] 的可测核心(显式传入定义集合)。
pub fn resolve_in(raw: &str, defs: &[AgentDef]) -> Option<ResolvedAgentType> {
    let key = raw.trim();
    if key.is_empty() {
        return None;
    }
    if let Some(t) = SubAgentType::parse(key) {
        return Some(ResolvedAgentType::Builtin(t));
    }
    let lower = key.to_lowercase();
    defs.iter()
        .find(|d| d.id.to_lowercase() == lower)
        .map(|d| ResolvedAgentType::Custom(Arc::new(d.clone())))
}

/// 当前可用类型 id 清单(内置 + 自定义),供 1001 错误消息与提示词使用。
pub fn available_ids(defs: &[AgentDef]) -> Vec<String> {
    let mut out: Vec<String> = SubAgentType::ALL.iter().map(|t| t.id().to_string()).collect();
    out.extend(defs.iter().map(|d| d.id.clone()));
    out
}

/// 类型在本策略下的可见性(在内即为名册可见;可见性与运行期交集**双重**保证不越权)。
pub fn visible_under(declared: &[String], policy: SpawnPolicy) -> bool {
    if policy == SpawnPolicy::Disabled {
        return false;
    }
    let allowed = policy.allowed_tools();
    !declared.is_empty() && declared.iter().all(|t| allowed.contains(&t.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    fn temp_pair() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        (dir, work, home)
    }

    fn project_dir(work: &Path) -> PathBuf {
        work.join(DIR).join(SUBDIR)
    }

    #[test]
    fn empty_dirs_yield_empty_discovery() {
        let (_g, work, home) = temp_pair();
        let d = discover_with_home(&work, &home);
        assert!(d.defs.is_empty());
        assert!(d.ignored.is_empty());
    }

    #[test]
    fn full_frontmatter_parsed() {
        let (_g, work, home) = temp_pair();
        write(
            &project_dir(&work),
            "fe-reviewer.md",
            "---\nlabel: 前端审查\ndescription: 前端专项\nextends: code-reviewer\ntools: Read, Glob, Grep\nreadonly: true\nfuture: x\n---\n正文职责\n第二行\n",
        );
        let d = discover_with_home(&work, &home);
        assert_eq!(d.defs.len(), 1);
        let def = &d.defs[0];
        assert_eq!(def.id, "fe-reviewer");
        assert_eq!(def.label, "前端审查");
        assert_eq!(def.description, "前端专项");
        assert_eq!(def.extends, SubAgentType::CodeReviewer);
        assert_eq!(def.tools, vec!["Read", "Glob", "Grep"]);
        assert!(def.read_only);
        assert_eq!(def.scope, DefScope::Project);
        assert_eq!(def.body, "正文职责\n第二行", "正文应 trim 首尾空白");
        assert_eq!(def.pascal(), "FeReviewer");
    }

    #[test]
    fn missing_frontmatter_uses_defaults_and_body_first_line() {
        let (_g, work, home) = temp_pair();
        write(
            &project_dir(&work),
            "plain.md",
            "\n\n  第一行即是描述\n后续",
        );
        let d = discover_with_home(&work, &home);
        let def = &d.defs[0];
        assert_eq!(def.label, "自定义");
        assert_eq!(def.description, "第一行即是描述");
        assert_eq!(def.extends, SubAgentType::GeneralPurpose, "缺省通用执行");
        assert!(def.tools.is_empty(), "未声明 tools -> 继承 extends");
        assert_eq!(def.declared_tools(), SubAgentType::GeneralPurpose.default_tools());
    }

    #[test]
    fn user_level_overrides_project_level_same_id() {
        let (_g, work, home) = temp_pair();
        write(&project_dir(&work), "x.md", "---\nlabel: 项目级\n---\np");
        write(&home, "x.md", "---\nlabel: 用户级\n---\nu");
        let d = discover_with_home(&work, &home);
        assert_eq!(d.defs.len(), 1);
        assert_eq!(d.defs[0].label, "用户级");
        assert_eq!(d.defs[0].scope, DefScope::User);
    }

    #[test]
    fn builtin_id_shadow_ignored_with_reason() {
        let (_g, work, home) = temp_pair();
        write(&project_dir(&work), "explore.md", "---\nlabel: 我的侦察\n---\nx");
        write(&project_dir(&work), "reviewer.md", "---\nlabel: 别名冲突\n---\nx");
        let d = discover_with_home(&work, &home);
        assert!(d.defs.is_empty(), "内置(含别名)不可被遮蔽");
        assert_eq!(d.ignored.len(), 2);
        assert!(d.ignored.iter().all(|i| i.reason.contains("内置")));
    }

    #[test]
    fn invalid_filename_and_non_md_skipped() {
        let (_g, work, home) = temp_pair();
        let dir = project_dir(&work);
        write(&dir, "有 空格.md", "---\nlabel: x\n---\nb");
        write(&dir, "note.txt", "不是 markdown");
        write(&dir, "ok-name.md", "---\nlabel: ok\n---\nb");
        let d = discover_with_home(&work, &home);
        assert_eq!(d.defs.len(), 1, "只应加载合法 .md");
        assert_eq!(d.defs[0].id, "ok-name");
        assert_eq!(d.ignored.len(), 1);
        assert!(d.ignored[0].reason.contains("文件名非法"));
    }

    #[test]
    fn readonly_without_extends_uses_explore_and_intersects_tools() {
        let (_g, work, home) = temp_pair();
        write(
            &project_dir(&work),
            "ro.md",
            "---\nreadonly: true\ntools: Read, Write, Bash\n---\nb",
        );
        let d = discover_with_home(&work, &home);
        let def = &d.defs[0];
        assert_eq!(def.extends, SubAgentType::Explore, "readonly 未声明 extends -> 只读骨架");
        assert_eq!(def.tools, vec!["Read"], "显式 tools 与只读集取交集");
        assert!(def.read_only);
    }

    #[test]
    fn readonly_true_with_empty_tools_falls_back_to_readonly_set() {
        let (_g, work, home) = temp_pair();
        write(&project_dir(&work), "ro2.md", "---\nreadonly: yes\n---\nb");
        let d = discover_with_home(&work, &home);
        assert_eq!(d.defs[0].tools, vec!["Read", "Glob", "Grep"]);
    }

    #[test]
    fn tool_list_accepts_multiple_separators_and_dedups() {
        let (_g, work, home) = temp_pair();
        // frontmatter 值必须写在**同一行**(极简 YAML:一行一个 key)
        write(
            &project_dir(&work),
            "many.md",
            "---\ntools: Read, Glob;Grep Read、Bash\n---\nb",
        );
        let d = discover_with_home(&work, &home);
        assert_eq!(d.defs[0].tools, vec!["Read", "Glob", "Grep", "Bash"]);
    }

    #[test]
    fn frontmatter_values_are_single_line_only() {
        // 值换行后不再属于该 key(极简 YAML 的显式边界,避免"看起来生效"的误解)
        let (_g, work, home) = temp_pair();
        write(
            &project_dir(&work),
            "multiline.md",
            "---\ntools: Read,\nBash\n---\nb",
        );
        let d = discover_with_home(&work, &home);
        assert_eq!(d.defs[0].tools, vec!["Read"], "续行不是 value 的一部分");
    }

    #[test]
    fn unknown_extends_falls_back_to_general_purpose() {
        let (_g, work, home) = temp_pair();
        write(&project_dir(&work), "weird.md", "---\nextends: ninja\n---\nb");
        let d = discover_with_home(&work, &home);
        assert_eq!(d.defs[0].extends, SubAgentType::GeneralPurpose);
    }

    #[test]
    fn resolve_prefers_builtin_then_custom() {
        let (_g, work, home) = temp_pair();
        write(&project_dir(&work), "fe-reviewer.md", "---\nlabel: 前端\n---\nb");
        let defs = discover_with_home(&work, &home).defs;

        assert!(matches!(
            resolve_in("explore", &defs),
            Some(ResolvedAgentType::Builtin(SubAgentType::Explore))
        ));
        let custom = resolve_in("FE-Reviewer", &defs).expect("自定义 id 大小写不敏感");
        assert!(custom.is_custom());
        assert_eq!(custom.id(), "fe-reviewer");
        assert_eq!(custom.label(), "前端");
        assert_eq!(custom.pascal(), "FeReviewer");
        assert!(custom.source().is_some());
        assert!(resolve_in("ninja", &defs).is_none());
        assert!(resolve_in("  ", &defs).is_none());
    }

    #[test]
    fn custom_prompt_contains_identity_body_and_leaf_boundary() {
        let (_g, work, home) = temp_pair();
        write(
            &project_dir(&work),
            "fe-reviewer.md",
            "---\nlabel: 前端审查\nextends: code-reviewer\nreadonly: true\n---\n只给问题清单,不写文件。",
        );
        let defs = discover_with_home(&work, &home).defs;
        let t = resolve_in("fe-reviewer", &defs).unwrap();
        let p = t.system_prompt("LsmAgentEmergentWork-SubAgent-FeReviewer-前端审查");
        assert!(p.contains("类型:前端审查(fe-reviewer,自定义定义文件)"));
        assert!(p.contains("继承自 `code-reviewer`"));
        assert!(p.contains("只给问题清单,不写文件。"));
        assert!(p.contains("不能再启动子 Agent"), "叶子语义必须保留");
        assert!(p.contains("不要写文件"), "readonly 语义应进提示词边界");
    }

    #[test]
    fn visibility_is_strict_subset_of_policy() {
        let ro = vec!["Read".to_string(), "Grep".to_string()];
        assert!(visible_under(&ro, SpawnPolicy::ReadOnlyChildren));
        assert!(visible_under(&ro, SpawnPolicy::FullChildren));
        let rw = vec!["Read".to_string(), "Write".to_string()];
        assert!(!visible_under(&rw, SpawnPolicy::ReadOnlyChildren), "只读父不得见可写子");
        assert!(visible_under(&rw, SpawnPolicy::FullChildren));
        assert!(!visible_under(&ro, SpawnPolicy::Disabled));
        assert!(!visible_under(&[], SpawnPolicy::FullChildren), "空工具面不适用");
    }

    #[test]
    fn available_ids_lists_builtin_and_custom() {
        let (_g, work, home) = temp_pair();
        write(&project_dir(&work), "fe-reviewer.md", "---\n---\nb");
        let defs = discover_with_home(&work, &home).defs;
        let ids = available_ids(&defs);
        assert_eq!(ids.len(), 7);
        assert!(ids.contains(&"general-purpose".to_string()));
        assert!(ids.contains(&"fe-reviewer".to_string()));
    }

    #[test]
    fn pascal_from_id_handles_cjk_and_odd_forms() {
        assert_eq!(pascal_from_id("fe-reviewer"), "FeReviewer");
        assert_eq!(pascal_from_id("db_migration"), "DbMigration");
        assert_eq!(pascal_from_id("审查"), "Custom");
        assert_eq!(pascal_from_id("a-b-c"), "ABC");
    }

    #[test]
    fn max_defs_cap_reports_ignored() {
        let (_g, work, home) = temp_pair();
        for i in 0..(MAX_DEFS + 3) {
            write(&project_dir(&work), &format!("t{i:03}.md"), "---\n---\nb");
        }
        let d = discover_with_home(&work, &home);
        assert_eq!(d.defs.len(), MAX_DEFS);
        assert_eq!(d.ignored.len(), 3);
        assert!(d.ignored.iter().all(|i| i.reason.contains("上限")));
    }
}
