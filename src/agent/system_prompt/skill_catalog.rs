//! Skill catalog 子模块 —— 2026-09-23 第 126 轮新增。
//!
//! 把 Skill 系统的 catalog 段封装成可拼装段,与 `mcp_use_hint` 同款
//! (独立子模块防止 `system_prompt/mod.rs` 接近 1800 行上限)。
//!
//! **挂载点**:`SystemPrompt::sub_agent_work()` / `SystemPrompt::main_work()`
//! 的 `append_base()` 末尾(`sub_agent_work()` / `main_work()` 已是
//! 链式 builder 模式,新段追加零侵入)。
//!
//! **设计**:此模块只放「静态 hook」与最简注册函数,真正的 catalog 字节
//! 内容由 `agent::skills::render::render_catalog` 运行时生成(挂在
//! rules 段 cache 复用需按实例字节一致)。
//!
//! **协议差异**:
//! - Anthropic 路径:`render(Anthropic)` 输出整体作 rules 段(catalog 字节级稳定时复用);
//! - OpenAI 路径:走 `render(OpenAI)` 单字符串 system message。

use crate::agent::skills;

/// 在 system prompt 中追加 Skill catalog 段(若 catalog 非空)。
///
/// 调用方应在 profile 装配时一次性调用并把结果固化到 SystemPrompt,
/// Anthropic prefix cache 跨 SubAgent/Main-Work 实例复用最大化。
pub fn append_to(prompt: crate::agent::system_prompt::SystemPrompt, registry: &skills::SkillRegistry) -> crate::agent::system_prompt::SystemPrompt {
    let skills_arc = registry.list_arc();
    if let Some(catalog) = skills::render::render_catalog(&skills_arc) {
        prompt.append_base(&format!("\n{catalog}\n"))
    } else {
        prompt
    }
}