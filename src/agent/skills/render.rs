//! Skill catalog 渲染 —— 对齐 atomcode `crates/atomcode-capabilities/src/skills/render.rs`
//! 第 401 行的 8KB 字节预算 + 1024 字符 description 截断 + 后赢排序。
//!
//! **核心目标**:catalog 文本**字节级稳定**(同一 registry 同一顺序),
//! 让 Anthropic prefix cache 跨 SubAgent/Main-Work 实例复用最大化。
//! 渲染时严格按 name 字典序(BTreeMap 已保证),description 截到 PER_SKILL_DESC_CAP,
//! 整体字节超 CATALOG_BYTE_BUDGET 时把尾部条目归为 "...and X more omitted"
//! 引导模型调 `list_skills` 拿完整列表。

use std::sync::Arc;

use super::skill::Skill;

/// catalog 字节预算(对齐 atomcode CATALOG_BYTE_BUDGET=8000)。
///
/// 8000B 大约够装 50-80 条 `- name: description` 短条目,
/// 真实场景下用户级 ~/.laew/skills 不超过 20 条,跨 agent `.agents/skills` 兜底,
/// 触发 omitted 注释的概率极低;但对大型团队共享库仍必要。
pub const CATALOG_BYTE_BUDGET: usize = 8000;

/// 单条 description 字符上限(对齐 pi MAX_DESCRIPTION_LENGTH=1024)。
pub const PER_SKILL_DESC_CAP: usize = 1024;

/// catalog 起始头标记(对齐 atomcode `render.rs:1-25` 的 CATALOG_HEADER)。
///
/// 用 `=== AVAILABLE SKILLS ===` 作识别符,便于 `resume` 时按前缀字节
/// 原地 reconcile(对齐 atomcode SkillCatalogHook 第 36-42 行)。
pub const CATALOG_HEADER: &str = "=== AVAILABLE SKILLS ===";

/// GUIDANCE 契约段(对齐 atomcode `render.rs:35`)。
///
/// 强约束「never invent」「must load before work」「call list_skills when omitted」
/// —— 这是模型正确使用 Skill 的认知基础,改一个字就可能破坏契约。
pub const CATALOG_GUIDANCE: &str = "Skills are reusable instruction templates for specific tasks. \
The names listed below are the only skill names you may pass directly to `use_skill`; \
never invent or guess a skill name from memory. Match a task only against descriptions \
actually shown below. If a task clearly matches a shown skill's description — not only \
when the user names the skill — you MUST load that exact skill with `use_skill` and \
follow it BEFORE doing the work. If this catalog says skills were omitted, call \
`list_skills` before using an omitted name. If `use_skill` reports a missing skill, \
do not guess another name; briefly note it and continue with the best fallback. \
Announce in one line which skill you're using.";

/// 渲染 catalog 文本。
///
/// `skills` 不必排序(调用方应传 BTreeMap 的有序视图,本函数不再重排以保持
/// 字节稳定)。返回 `None` 当且仅当 skills 为空。
pub fn render_catalog(skills: &[Arc<Skill>]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let mut sorted: Vec<&Skill> = skills.iter().map(|s| s.as_ref()).collect();
    sorted.sort_by(|a, b| a.name().cmp(b.name()));

    let mut out = String::with_capacity(CATALOG_BYTE_BUDGET + 1024);
    out.push_str(CATALOG_HEADER);
    out.push('\n');
    out.push_str(CATALOG_GUIDANCE);
    out.push('\n');

    let mut body_bytes = out.len();
    let mut omitted: usize = 0;
    for s in &sorted {
        let desc = truncate_chars(s.description(), PER_SKILL_DESC_CAP);
        let line = format!("- {}: {}\n", s.name(), desc);
        if body_bytes + line.len() <= CATALOG_BYTE_BUDGET {
            body_bytes += line.len();
            out.push_str(&line);
        } else {
            omitted += 1;
        }
    }
    if omitted > 0 {
        out.push_str(&format!(
            "... and {omitted} more skill(s) omitted; call `list_skills` for full list.\n"
        ));
    }
    Some(out)
}

/// 按字符数截断(CJK 安全),与 `skill::truncate_chars` 同语义,本地独立避免循环依赖。
fn truncate_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    if total <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::skills::skill::{Skill, SkillFrontmatter, SkillSource};
    use std::path::PathBuf;

    fn make_arc(name: &str, desc: &str) -> Arc<Skill> {
        Arc::new(Skill {
            frontmatter: SkillFrontmatter {
                name: Some(name.to_string()),
                description: desc.to_string(),
                ..Default::default()
            },
            body: String::new(),
            skill_dir: PathBuf::from("/tmp"),
            source_path: PathBuf::from(format!("/tmp/{name}.md")),
            source: SkillSource::Bundled,
        })
    }

    #[test]
    fn empty_skills_returns_none() {
        assert!(render_catalog(&[]).is_none());
    }

    #[test]
    fn header_and_guidance_present() {
        let s = vec![make_arc("a", "x")];
        let out = render_catalog(&s).unwrap();
        assert!(out.starts_with(CATALOG_HEADER));
        assert!(out.contains(CATALOG_GUIDANCE));
        assert!(out.contains("- a: x"));
    }

    #[test]
    fn sorted_by_name_alphabetic() {
        let s = vec![
            make_arc("zebra", "z"),
            make_arc("alpha", "a"),
            make_arc("mike", "m"),
        ];
        let out = render_catalog(&s).unwrap();
        let pos_alpha = out.find("- alpha:").unwrap();
        let pos_mike = out.find("- mike:").unwrap();
        let pos_zebra = out.find("- zebra:").unwrap();
        assert!(pos_alpha < pos_mike && pos_mike < pos_zebra);
    }

    #[test]
    fn byte_budget_triggers_omitted() {
        // 构造 50 条 200 字符 description,超出 8KB 必触发 omitted
        let s: Vec<Arc<Skill>> = (0..50)
            .map(|i| make_arc(&format!("skill-{i:02}"), &"x".repeat(200)))
            .collect();
        let out = render_catalog(&s).unwrap();
        assert!(out.contains("more skill(s) omitted"));
        // 头尾标志必命中
        assert!(out.starts_with(CATALOG_HEADER));
        assert!(out.len() <= CATALOG_BYTE_BUDGET + 200, "整体超出预算 + 省略注释 + 引导");
    }

    #[test]
    fn description_truncated_to_cap() {
        let long = "y".repeat(PER_SKILL_DESC_CAP + 100);
        let s = vec![make_arc("a", &long)];
        let out = render_catalog(&s).unwrap();
        // 应截到 PER_SKILL_DESC_CAP 字符 + 省略号
        let line_a = out.lines().find(|l| l.starts_with("- a:")).unwrap();
        let after_colon = &line_a[line_a.find(':').unwrap() + 2..];
        // 去掉尾部省略号再看字符数
        assert!(after_colon.ends_with('…'));
        let body = &after_colon[..after_colon.len() - '…'.len_utf8()];
        assert_eq!(body.chars().count(), PER_SKILL_DESC_CAP);
    }

    #[test]
    fn byte_stability_same_input_same_output() {
        let s = vec![make_arc("a", "x"), make_arc("b", "y")];
        let o1 = render_catalog(&s).unwrap();
        let o2 = render_catalog(&s).unwrap();
        assert_eq!(o1, o2, "同输入必字节级一致(prefix cache 复用基础)");
    }
}