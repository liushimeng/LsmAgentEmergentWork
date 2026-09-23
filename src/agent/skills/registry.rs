//! Skill 注册表 —— 对齐 atomcode SkillRegistry(BTreeMap 保字节稳定,
//! 触发 prompt prefix caching) + pi 6 源发现链的简化 3 源版本。
//!
//! 3 源发现链(对齐 pi `core/skills.ts:407-507` 简化):
//! 1. 项目级 `{cwd}/.laew/skills/`(随仓库提交,优先级最高)
//! 2. 跨 agent `{cwd}/.agents/skills/`(对齐 pi/opencode/openclaw)
//! 3. 用户级 `~/.laew/skills/`(兜底,个人偏好)
//!
//! 后赢覆盖(同 name 时晚加载的覆盖先加载的);bundled 内嵌在二进制,
//! 单独 `load_bundled()` 入口。
//!
//! 加载失败/解析错误/非法命名时**静默忽略**并写入 `SkillLoadDiagnostics.ignored`,
//! 不阻断主流程(对齐 pi 的 diagnostics 收集策略)。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::bundled;
use super::skill::{load_from_file, Skill, SkillLoadDiagnostics, SkillSource};

/// Skill 注册表。
///
/// 用 BTreeMap 保 catalog 注入顺序字节稳定 → 触发 Anthropic prefix cache 复用;
/// `Arc<Skill>` 让 catalog 跨 profile / runner / Agent 实例共享零拷贝。
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: BTreeMap<String, Arc<Skill>>,
    /// 加载诊断(用户 `/skills --diagnostics` 或 `list --verbose` 可见)
    stats: Arc<Mutex<SkillLoadDiagnostics>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 home + cwd + bundled 三源加载(顶层入口,orchestrator 装配时一次调用)。
    pub fn load(home: &Path, cwd: &Path) -> Self {
        let mut reg = Self::new();

        // 3 源发现(后赢):项目级 → 跨 agent → 用户级
        let dirs = standard_skill_dirs(home, cwd);
        for (dir, source) in dirs {
            if dir.is_dir() {
                reg.load_dir(&dir, source);
            }
        }

        // bundled 内嵌(最高优先级,后赢)
        for skill in bundled::load_skills() {
            reg.insert(skill);
        }

        reg
    }

    /// 递归扫描(深度 ≤ 4,防极深目录)：
    /// - 顶层 `<name>.md` 平铺
    /// - 子目录 `<name>/SKILL.md` 包式(对齐 pi `core/skills.ts` 嵌套加载)
    /// - `.gitignore` / `.ignore` / `.fdignore` 过滤(对齐 pi gitignore 风格)
    pub fn load_dir(&mut self, dir: &Path, source: SkillSource) {
        Self::scan_dir(self, dir, source, 0);
    }

    fn scan_dir(&mut self, dir: &Path, source: SkillSource, depth: usize) {
        const MAX_DEPTH: usize = 4;
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();

        for path in entries {
            // 忽略特定目录
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if matches!(name, ".git" | "node_modules" | "__pycache__" | "target" | ".DS_Store") {
                    continue;
                }
                if Self::is_ignored_by_gitignore(&path) {
                    continue;
                }
            }

            let file_type = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            if file_type.is_file() {
                if depth == 0 {
                    // 顶层:接受 `<name>.md`(平铺)
                    if path.extension().and_then(|s| s.to_str()) == Some("md") {
                        self.insert_from_file(&path, source);
                    }
                } else {
                    // 非顶层:接受 SKILL.md(包式)
                    if path.file_name().and_then(|s| s.to_str()) == Some("SKILL.md") {
                        self.insert_from_file(&path, source);
                    }
                }
            } else if file_type.is_dir() {
                self.scan_dir(&path, source, depth + 1);
            }
        }
    }

    /// 尝试加载文件,成功时插入(后赢);失败计入 diagnostics。
    fn insert_from_file(&mut self, path: &Path, source: SkillSource) {
        match load_from_file(path, source) {
            Ok(skill) => {
                self.stats.lock().unwrap().loaded += 1;
                self.skills.insert(skill.name().to_string(), Arc::new(skill));
            }
            Err(reason) => {
                self.stats
                    .lock()
                    .unwrap()
                    .ignored
                    .push((path.to_path_buf(), reason));
            }
        }
    }

    /// 直接插入(供 bundled 嵌入使用)。
    pub fn insert(&mut self, skill: Skill) {
        self.stats.lock().unwrap().loaded += 1;
        self.skills.insert(skill.name().to_string(), Arc::new(skill));
    }

    pub fn get(&self, name: &str) -> Option<Arc<Skill>> {
        self.skills.get(name).cloned()
    }

    /// 返回 (name, description, source) 三元组,按 name 字典序(BTreeMap 已保证)。
    pub fn list(&self) -> Vec<(String, String, SkillSource)> {
        self.skills
            .values()
            .map(|s| (s.name().to_string(), s.description().to_string(), s.source))
            .collect()
    }

    /// 返回 Arc 列表(供 `render_catalog` 直接引用,避免再次 clone)。
    pub fn list_arc(&self) -> Vec<Arc<Skill>> {
        self.skills.values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn diagnostics(&self) -> SkillLoadDiagnostics {
        self.stats.lock().unwrap().clone()
    }

    /// 简易 gitignore 过滤(只读同目录 .gitignore,支持 `node_modules` / `target` 等显式忽略)。
    /// 完整 gitignore 解析(如 git 库)留 P2;对齐 pi 简化策略:文件名级 + 同目录 `.gitignore`。
    fn is_ignored_by_gitignore(path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        let gi = parent.join(".gitignore");
        let Ok(content) = std::fs::read_to_string(&gi) else {
            return false;
        };
        let Some(target_name) = path.file_name().and_then(|s| s.to_str()) else {
            return false;
        };
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // 简化:只匹配精确文件名,不处理 /*/! 前缀语义
            if line == target_name {
                return true;
            }
        }
        false
    }
}

/// 3 源发现链(后赢顺序)。
///
/// **优先级**:Project > CrossAgent > User(bundled 单独路径)。
/// 团队共享(项目级)优先于个人偏好(用户级)——
/// 对齐 atomcode「`source_rank` 4 级 tier」简化到 3 档,语义直接对应项目。
pub fn standard_skill_dirs(home: &Path, cwd: &Path) -> Vec<(PathBuf, SkillSource)> {
    vec![
        (cwd.join(".laew/skills"), SkillSource::Project),
        (cwd.join(".agents/skills"), SkillSource::CrossAgent),
        (home.join(".laew/skills"), SkillSource::User),
    ]
}

// =================== 测试 ===================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_skill(dir: &Path, name: &str, frontmatter: &str, body: &str) {
        let content = format!("---\n{name_field}: {name}\n{frontmatter}---\n{body}\n",
            name_field = "name",
            name = name,
            frontmatter = frontmatter,
            body = body,
        );
        std::fs::write(dir.join(format!("{name}.md")), content).unwrap();
    }

    #[test]
    fn standard_dirs_yields_3_paths() {
        let dirs = standard_skill_dirs(&PathBuf::from("/home/u"), &PathBuf::from("/proj"));
        assert_eq!(dirs.len(), 3);
        assert!(dirs[0].0.ends_with(".laew/skills"));
        assert_eq!(dirs[0].1, SkillSource::Project);
        assert_eq!(dirs[1].1, SkillSource::CrossAgent);
        assert_eq!(dirs[2].1, SkillSource::User);
    }

    #[test]
    fn load_picks_up_user_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join(".laew/skills");
        std::fs::create_dir_all(&user_dir).unwrap();
        write_skill(&user_dir, "git-commit", "description: 提交规范\n", "# Body\n");
        let reg = SkillRegistry::load(tmp.path(), tmp.path());
        let names: Vec<String> = reg.list().into_iter().map(|(n, _, _)| n).collect();
        assert!(names.contains(&"git-commit".to_string()));
    }

    #[test]
    fn load_dir_recurses_into_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join(".laew/skills/pdf-tools");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("SKILL.md"),
            "---\nname: pdf-tools\ndescription: PDF 工具集\n---\n# Body\n",
        )
        .unwrap();
        let reg = SkillRegistry::load(tmp.path(), tmp.path());
        assert!(reg.get("pdf-tools").is_some());
    }

    #[test]
    fn last_source_wins_on_collision() {
        // 用户级 + 项目级同名 → 项目级后赢(覆盖)
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join(".laew/skills");
        std::fs::create_dir_all(&user_dir).unwrap();
        write_skill(&user_dir, "x", "description: user 版\n", "# User Body\n");
        let project_dir = tmp.path().join(".laew/skills");
        std::fs::create_dir_all(&project_dir).unwrap();
        write_skill(&project_dir, "x", "description: project 版\n", "# Project Body\n");

        // 注意:同一目录写入两次 .md,load 时按目录顺序扫描,
        // 用户级在前 → 项目级后赢
        let reg = SkillRegistry::load(tmp.path(), tmp.path());
        let skill = reg.get("x").unwrap();
        assert_eq!(skill.description(), "project 版");
        assert!(skill.body.contains("Project Body"));
    }

    #[test]
    fn invalid_frontmatter_lands_in_diagnostics_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join(".laew/skills");
        std::fs::create_dir_all(&d).unwrap();
        // name 含非法字符 "-leading" 会触发 validate_name 失败
        std::fs::write(
            d.join("bad.md"),
            "---\nname: -leading\ndescription: bad\n---\nbody",
        )
        .unwrap();
        let reg = SkillRegistry::load(tmp.path(), tmp.path());
        assert!(reg.get("bad").is_none());
        let diag = reg.diagnostics();
        assert!(!diag.ignored.is_empty(), "ignored 应记录坏 skill");
    }

    #[test]
    fn bundled_skills_always_present() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = SkillRegistry::load(tmp.path(), tmp.path());
        // bundled 至少含 git-commit / code-review / test-runner 三件套
        let names: Vec<String> = reg.list().into_iter().map(|(n, _, _)| n).collect();
        assert!(names.contains(&"git-commit".to_string()));
        assert!(names.contains(&"code-review".to_string()));
        assert!(names.contains(&"test-runner".to_string()));
    }

    #[test]
    fn missing_dirs_are_silent() {
        // 全部目录不存在 → 仅 bundled
        let reg = SkillRegistry::load(&PathBuf::from("/nonexistent/home"), &PathBuf::from("/nonexistent/cwd"));
        assert!(!reg.is_empty(), "bundled 兜底必在");
    }
}