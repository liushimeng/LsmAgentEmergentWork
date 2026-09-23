//! Skill 工具 —— `use_skill` + `list_skills`。
//!
//! **设计来源**:对齐 atomcode `crates/atomcode-capabilities/src/skills/use_skill.rs`
//! 第 316 行 + pi `packages/tool-skill/src/index.ts` + openclaw `skill-creator` 反馈链。
//!
//! - **`use_skill`**:加载指定 skill 的 body 并执行变量替换(含 `!`cmd`` shell 注入),
//!   返回展开后的纯文本给模型作为后续指令上下文。
//! - **`list_skills`**:列出当前可见 skill(name + description + source),
//!   解决 catalog 8KB 预算触发 omitted 后模型看不到全貌的问题。
//!
//! **状态注入**:两 Tool 都持 `Arc<SkillRegistry>`,由调用方构造时注入(对齐
//! `SubAgentTool` 零状态 + task_local 的反模式 —— Skill 工具状态天然外层),
//! `use_skill` 额外持 `Arc<String>` session_id(供未来 audit/trace 扩展,
//! 本轮仅透传,不写 trace)。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::tools::Tool;
use crate::error::{AgentError, Result};

use super::registry::SkillRegistry;
use super::skill::{self, Skill};

/// `use_skill` 工具 —— 按需加载 skill body + 变量替换 + shell 注入。
pub struct UseSkillTool {
    pub registry: Arc<SkillRegistry>,
    pub session_id: Arc<String>,
}

impl UseSkillTool {
    pub fn new(registry: Arc<SkillRegistry>, session_id: Arc<String>) -> Self {
        Self { registry, session_id }
    }
}

#[derive(Debug, Deserialize)]
struct UseSkillArgs {
    name: String,
    #[serde(default)]
    arguments: Option<String>,
}

#[async_trait]
impl Tool for UseSkillTool {
    fn name(&self) -> &str {
        "use_skill"
    }

    fn description(&self) -> &str {
        "Invoke a named skill (a reusable prompt/workflow template) and return its content \
         with your arguments substituted. The name must exactly match a skill listed under \
         '=== AVAILABLE SKILLS ===' in the system prompt or returned by list_skills. Never \
         invent or guess a skill name. Trigger a skill when the task matches its listed \
         description — not only when the user names it. list_skills shows any lower-priority \
         skills omitted from the prompt catalog. Variable substitution includes $1..$9 \
         positional args, $ARGUMENTS full args, ${CLAUDE_SKILL_DIR}/${LAEW_SKILL_DIR}, \
         and shell injection via !`cmd` (trusted user-authored content)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Exact skill name from AVAILABLE SKILLS or list_skills; never invent a name"
                },
                "arguments": {
                    "type": "string",
                    "description": "Optional arguments passed to the skill (supports $ARGUMENTS / $1..$9)"
                }
            },
            "required": ["name"]
        })
    }

    fn parallel_safe(&self, _args: &Value) -> bool {
        false
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let parsed: UseSkillArgs = serde_json::from_value(args).map_err(|e| {
            AgentError::ToolExecution {
                tool: "use_skill".to_string(),
                reason: format!("invalid args: {e}"),
            }
        })?;

        let skill = match self.registry.get(&parsed.name) {
            Some(s) => s,
            None => {
                let known: Vec<String> = self
                    .registry
                    .list()
                    .into_iter()
                    .map(|(n, _, _)| n)
                    .collect();
                let known_str = if known.is_empty() {
                    "(none)".to_string()
                } else {
                    known.join(", ")
                };
                return Err(AgentError::ToolExecution {
                    tool: "use_skill".to_string(),
                    reason: format!(
                        "skill '{}' not found. Available: {}. \
                         Do not guess another skill name; use an exact available name \
                         or continue without a skill.",
                        parsed.name, known_str
                    ),
                });
            }
        };

        let arguments = parsed.arguments.unwrap_or_default();
        let sid = self.session_id.as_str().to_string();

        // 变量替换可能阻塞(shell 注入 fork 子进程)→ spawn_blocking
        let skill_for_expand = Arc::clone(&skill);
        let arguments_for_expand = arguments.clone();
        let content = tokio::task::spawn_blocking(move || {
            skill::expand(skill_for_expand.as_ref(), &arguments_for_expand, &sid)
        })
        .await
        .map_err(|e| AgentError::ToolExecution {
            tool: "use_skill".to_string(),
            reason: format!("spawn_blocking join error: {e}"),
        })?;

        Ok(format_skill_payload(&skill, &arguments, &content))
    }
}

/// 输出信封:`<<<LAEW:SKILL_LOADED>>>` 标记,便于日志 grep + 防「模型把它当指令执行」。
/// 内容与现有 `<<<LAEW:>>>` 标记族对齐(`Source` / `SessionHistory` / `ProjectContext`)。
fn format_skill_payload(skill: &Skill, arguments: &str, expanded: &str) -> String {
    let mut out = String::with_capacity(expanded.len() + 128);
    out.push_str("<<<LAEW:SKILL_LOADED\n");
    out.push_str(&format!("name: {}\n", skill.name()));
    out.push_str(&format!("source: {}\n", skill.source.as_str()));
    if !arguments.is_empty() {
        out.push_str(&format!("arguments: {arguments}\n"));
    }
    out.push_str("---\n");
    out.push_str(expanded);
    if !expanded.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(">>>\n");
    out
}

/// `list_skills` 工具 —— 列出当前可见 skill。
pub struct ListSkillsTool {
    pub registry: Arc<SkillRegistry>,
}

impl ListSkillsTool {
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for ListSkillsTool {
    fn name(&self) -> &str {
        "list_skills"
    }

    fn description(&self) -> &str {
        "List the available skills (name + description + source). Invoke one with use_skill. \
         Use this when the system prompt catalog notes omitted skills, or when you need \
         to discover the full set before selecting one."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    fn parallel_safe(&self, _args: &Value) -> bool {
        true
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        let skills = self.registry.list();
        if skills.is_empty() {
            return Ok("No skills are currently available.".to_string());
        }
        let mut out = format!("Available skills ({}):\n", skills.len());
        for (name, desc, source) in &skills {
            if desc.is_empty() {
                out.push_str(&format!("- {name}  [{source}]\n"));
            } else {
                out.push_str(&format!("- {name}: {desc}  [{source}]\n"));
            }
        }
        // 加载诊断(loaded / ignored)作为尾部注释,便于模型/用户看到「为什么少了几个」
        let diag = self.registry.diagnostics();
        if !diag.ignored.is_empty() {
            out.push_str(&format!(
                "\n(ignored {} invalid skill(s); use --diagnostics for details)\n",
                diag.ignored.len()
            ));
        }
        Ok(out)
    }
}

// =================== 测试 ===================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::skills::skill::{Skill, SkillFrontmatter, SkillSource};
    use std::path::PathBuf;

    fn make_skill(name: &str, desc: &str, body: &str) -> Skill {
        Skill {
            frontmatter: SkillFrontmatter {
                name: Some(name.to_string()),
                description: desc.to_string(),
                ..Default::default()
            },
            body: body.to_string(),
            skill_dir: PathBuf::from("/tmp"),
            source_path: PathBuf::from(format!("/tmp/{name}.md")),
            source: SkillSource::Bundled,
        }
    }

    fn empty_registry() -> Arc<SkillRegistry> {
        Arc::new(SkillRegistry::new())
    }

    #[tokio::test]
    async fn use_skill_executes_replaces_arguments() {
        let mut reg = SkillRegistry::new();
        reg.insert(make_skill("x", "x", "用 $ARGUMENTS 提交"));
        let reg = Arc::new(reg);
        let tool = UseSkillTool::new(reg, Arc::new("sid".to_string()));
        let r = tool
            .execute(json!({"name": "x", "arguments": "feat: 新增 Skill"}))
            .await
            .unwrap();
        assert!(r.contains("<<<LAEW:SKILL_LOADED"));
        assert!(r.contains("用 feat: 新增 Skill 提交"));
        assert!(r.contains(">>>"));
    }

    #[tokio::test]
    async fn use_skill_unknown_lists_known() {
        let mut reg = SkillRegistry::new();
        reg.insert(make_skill("a", "a skill", "body"));
        reg.insert(make_skill("b", "b skill", "body"));
        let reg = Arc::new(reg);
        let tool = UseSkillTool::new(reg, Arc::new("sid".to_string()));
        let err = tool
            .execute(json!({"name": "nonexistent"}))
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not found"), "got: {msg}");
        assert!(msg.contains("a") && msg.contains("b"));
    }

    #[tokio::test]
    async fn use_skill_invalid_args_returns_clear_error() {
        let tool = UseSkillTool::new(empty_registry(), Arc::new("sid".to_string()));
        let err = tool
            .execute(json!({"wrong_field": "v"}))
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("invalid args") || msg.contains("missing field"));
    }

    #[tokio::test]
    async fn list_skills_empty_returns_clear_string() {
        let tool = ListSkillsTool::new(empty_registry());
        let out = tool.execute(json!({})).await.unwrap();
        assert!(out.contains("No skills"));
    }

    #[tokio::test]
    async fn list_skills_with_entries_sorted_and_friendly() {
        let mut reg = SkillRegistry::new();
        reg.insert(make_skill("zebra", "z desc", ""));
        reg.insert(make_skill("alpha", "a desc", ""));
        let reg = Arc::new(reg);
        let tool = ListSkillsTool::new(reg);
        let out = tool.execute(json!({})).await.unwrap();
        assert!(out.contains("Available skills (2)"));
        assert!(out.contains("- alpha: a desc"));
        assert!(out.contains("- zebra: z desc"));
        assert!(out.contains("[bundled]"));
    }
}