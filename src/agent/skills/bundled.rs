//! 内置 Skill 嵌入 —— `include_str!` 编译期嵌入二进制。
//!
//! 三个内置 Skill(对齐 8 角色 system brief 风格):
//! - `git-commit` —— Conventional Commits 规范起草
//! - `code-review` —— 静态代码评审清单
//! - `test-runner` —— 项目测试自动运行与诊断
//!
//! **加载语义**:每次 `SkillRegistry::load()` 都嵌入(bundled 始终最高优先级,后赢覆盖),
//! 目录发现不到时仍可用。

use std::path::PathBuf;

use super::skill::{load_from_str, Skill, SkillSource};

/// 三个内置 SKILL.md 的编译期嵌入内容。
pub const GIT_COMMIT_MD: &str = include_str!("bundled/git-commit.md");
pub const CODE_REVIEW_MD: &str = include_str!("bundled/code-review.md");
pub const TEST_RUNNER_MD: &str = include_str!("bundled/test-runner.md");

/// 加载全部内置 Skill(bundled 永远兜底可用)。
pub fn load_skills() -> Vec<Skill> {
    vec![
        load_bundled("git-commit", GIT_COMMIT_MD),
        load_bundled("code-review", CODE_REVIEW_MD),
        load_bundled("test-runner", TEST_RUNNER_MD),
    ]
}

fn load_bundled(name: &str, raw: &str) -> Skill {
    let source_path = PathBuf::from(format!("<bundled:{name}>"));
    load_from_str(raw, &source_path, SkillSource::Bundled)
        .unwrap_or_else(|e| panic!("bundled skill '{name}' parse failed: {e}"))
}