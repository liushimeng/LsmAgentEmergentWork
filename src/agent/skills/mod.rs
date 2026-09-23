//! Skill 系统(渐进式披露的载体)—— 2026-09-23 第 126 轮新增。
//!
//! 渐进式披露的语义:启动时仅在 system prompt 注入「name + 短描述」的 catalog
//! (字节预算 8KB),LLM 看到任务匹配某 skill 的描述时,主动调用 `use_skill`
//! 工具按需加载完整 SKILL body(典型 5-50KB);完整变量替换与 shell 注入仅在
//! 加载后发生,不污染 catalog 上下文。
//!
//! **设计来源**(知识库 80+ 份调研文档,已固化为本仓基线):
//! - `docs/Agent源码调研/专题/专题-第六轮-Skill系统深度对比.md`(2200 行,六工程对比)
//! - `docs/Agent源码调研/专题/专题-第八轮-Skill一等公民与Workshop自演化深度对比.md`(2400 行)
//!
//! **核心参考实现**:
//! - atomcode `crates/atomcode-capabilities/src/skills/` (BTreeMap + 8KB 预算 + SkillCatalogHook)
//! - pi `packages/coding-agent/src/core/skills.ts` (Agent Skills 标准 + 6 源发现)
//! - openclaw `skills/*/SKILL.md` (`metadata.openclaw.*` 嵌套 metadata)
//!
//! **laew 集成面**:
//! - 数据模型 + 加载器:`skill.rs` / `registry.rs`
//! - catalog 渲染:`render.rs`(挂载到 `system_prompt/skill_catalog.rs`)
//! - 工具:`tools.rs` 的 `UseSkillTool` + `ListSkillsTool`
//! - 注册器:`tools/mod.rs::register_skill_tools`(对齐 `register_mcp_use` 开关模式)
//! - 注入:`SubAgentRunner::with_skills` / `MainWorkRunner::with_skills` builder
//! - TUI:`/skill` / `/skills` builtin slash 命令
//!
//! **不做**(避免 scope creep):
//! - skill-creator / Workshop 自演化 / 远程分发 / 热重载 / 描述优化循环 — 后续轮次

pub mod bundled;
pub mod registry;
pub mod render;
pub mod skill;
pub mod tools;

pub use registry::{standard_skill_dirs, SkillRegistry};
pub use skill::{SkillFrontmatter, SkillMetadata, SkillSource};
pub use render::render_catalog;