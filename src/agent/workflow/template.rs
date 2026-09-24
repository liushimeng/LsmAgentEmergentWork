//! WorkFlow 模板库:可复用工作流模板管理。
//!
//! 提供内置模板 + 自定义模板存储,支持按类别/标签检索。

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::workflow::goal::Goal;
use crate::agent::workflow::WorkflowConfig;
use crate::config::Db;

/// 模板类别。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateCategory {
    Refactoring,
    Testing,
    Analysis,
    Migration,
    Documentation,
    Custom(String),
}

/// WorkFlow 模板。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: TemplateCategory,
    pub phase_names: Vec<String>,
    pub default_config: WorkflowConfig,
    pub tags: Vec<String>,
    pub usage_count: usize,
}

/// 模板库。
pub struct TemplateLibrary {
    /// 预留:模板持久化(load/save)接入后启用读取。
    #[allow(dead_code)]
    db: Option<Arc<Db>>,
    templates: HashMap<String, WorkflowTemplate>,
}

impl TemplateLibrary {
    pub fn new(db: Option<Arc<Db>>) -> Self {
        let mut lib = Self {
            db,
            templates: HashMap::new(),
        };
        lib.load_builtin_templates();
        lib
    }

    fn load_builtin_templates(&mut self) {
        let builtins = vec![
            WorkflowTemplate {
                id: "refactor-module".into(),
                name: "refactor-module".into(),
                description: "重构单个模块:分析→设计→实现→测试→验证".into(),
                category: TemplateCategory::Refactoring,
                phase_names: vec![
                    "现状分析".into(),
                    "重构设计".into(),
                    "代码实现".into(),
                    "测试验证".into(),
                    "集成验证".into(),
                ],
                default_config: WorkflowConfig::default(),
                tags: vec!["重构".into(), "模块".into()],
                usage_count: 0,
            },
            WorkflowTemplate {
                id: "batch-test-gen".into(),
                name: "batch-test-gen".into(),
                description: "批量生成测试:扫描→分组→并行生成→汇总→修复".into(),
                category: TemplateCategory::Testing,
                phase_names: vec![
                    "代码扫描".into(),
                    "分组策略".into(),
                    "并行生成".into(),
                    "汇总检查".into(),
                    "失败修复".into(),
                ],
                default_config: WorkflowConfig {
                    max_parallel_squads: 5,
                    batch_chunk_size: 20,
                    ..Default::default()
                },
                tags: vec!["测试".into(), "批量".into()],
                usage_count: 0,
            },
            WorkflowTemplate {
                id: "code-review".into(),
                name: "code-review".into(),
                description: "代码审查:扫描→并行审查→汇总报告→建议".into(),
                category: TemplateCategory::Analysis,
                phase_names: vec![
                    "代码扫描".into(),
                    "并行审查".into(),
                    "问题汇总".into(),
                    "改进建议".into(),
                ],
                default_config: WorkflowConfig::default(),
                tags: vec!["审查".into(), "质量".into()],
                usage_count: 0,
            },
            WorkflowTemplate {
                id: "dependency-upgrade".into(),
                name: "dependency-upgrade".into(),
                description: "依赖升级:分析→升级→修复编译→测试→验证".into(),
                category: TemplateCategory::Migration,
                phase_names: vec![
                    "依赖分析".into(),
                    "执行升级".into(),
                    "编译修复".into(),
                    "回归测试".into(),
                    "集成验证".into(),
                ],
                default_config: WorkflowConfig::default(),
                tags: vec!["依赖".into(), "升级".into()],
                usage_count: 0,
            },
            WorkflowTemplate {
                id: "doc-generation".into(),
                name: "doc-generation".into(),
                description: "文档生成:扫描→并行生成→交叉引用→汇总".into(),
                category: TemplateCategory::Documentation,
                phase_names: vec![
                    "代码扫描".into(),
                    "并行生成".into(),
                    "交叉引用".into(),
                    "汇总输出".into(),
                ],
                default_config: WorkflowConfig {
                    max_parallel_squads: 5,
                    ..Default::default()
                },
                tags: vec!["文档".into(), "生成".into()],
                usage_count: 0,
            },
        ];

        for t in builtins {
            self.templates.insert(t.id.clone(), t);
        }
    }

    pub fn get(&self, id: &str) -> Option<&WorkflowTemplate> {
        self.templates.get(id)
    }

    pub fn list_all(&self) -> Vec<&WorkflowTemplate> {
        self.templates.values().collect()
    }

    pub fn list_by_category(&self, category: &TemplateCategory) -> Vec<&WorkflowTemplate> {
        self.templates
            .values()
            .filter(|t| t.category == *category)
            .collect()
    }

    pub fn find_by_tags(&self, tags: &[String]) -> Vec<&WorkflowTemplate> {
        self.templates
            .values()
            .filter(|t| tags.iter().any(|tag| t.tags.contains(tag)))
            .collect()
    }

    pub fn register(&mut self, template: WorkflowTemplate) {
        self.templates.insert(template.id.clone(), template);
    }

    pub fn use_template(&mut self, id: &str) -> Option<WorkflowTemplate> {
        self.templates.get_mut(id).map(|t| {
            t.usage_count += 1;
            t.clone()
        })
    }

    pub fn build_goal_from_template(&self, template_id: &str, session_id: &str) -> Option<Goal> {
        self.get(template_id).map(|t| {
            let mut goal = Goal::new(
                format!("goal-{}-{}", template_id, session_id),
                format!("WorkFlow: {}", t.name),
                t.description.clone(),
            );
            for (i, phase_name) in t.phase_names.iter().enumerate() {
                let subgoal = Goal::new(
                    format!("{}-phase-{}", goal.id, i),
                    phase_name.clone(),
                    format!("阶段 {}: {}", i + 1, phase_name),
                );
                goal.add_subgoal(subgoal);
            }
            goal
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_library_loads_builtins() {
        let lib = TemplateLibrary::new(None);
        assert_eq!(lib.list_all().len(), 5);
    }

    #[test]
    fn template_library_get_by_id() {
        let lib = TemplateLibrary::new(None);
        let t = lib.get("refactor-module");
        assert!(t.is_some());
        assert_eq!(t.unwrap().name, "refactor-module");
    }

    #[test]
    fn template_library_list_by_category() {
        let lib = TemplateLibrary::new(None);
        let testing = lib.list_by_category(&TemplateCategory::Testing);
        assert_eq!(testing.len(), 1);
        assert_eq!(testing[0].id, "batch-test-gen");
    }

    #[test]
    fn template_library_find_by_tags() {
        let lib = TemplateLibrary::new(None);
        let results = lib.find_by_tags(&vec!["批量".to_string()]);
        assert!(!results.is_empty());
    }

    #[test]
    fn template_library_register_custom() {
        let mut lib = TemplateLibrary::new(None);
        let custom = WorkflowTemplate {
            id: "custom-flow".into(),
            name: "custom-flow".into(),
            description: "自定义流程".into(),
            category: TemplateCategory::Custom("mycat".into()),
            phase_names: vec!["步骤1".into(), "步骤2".into()],
            default_config: WorkflowConfig::default(),
            tags: vec!["自定义".into()],
            usage_count: 0,
        };
        lib.register(custom);
        assert!(lib.get("custom-flow").is_some());
    }

    #[test]
    fn template_library_use_template_increments_count() {
        let mut lib = TemplateLibrary::new(None);
        let t = lib.use_template("refactor-module").unwrap();
        assert_eq!(t.usage_count, 1);
    }

    #[test]
    fn template_library_build_goal_from_template() {
        let lib = TemplateLibrary::new(None);
        let goal = lib.build_goal_from_template("refactor-module", "s-1");
        assert!(goal.is_some());
        let goal = goal.unwrap();
        assert_eq!(goal.subgoals.len(), 5);
        assert_eq!(goal.subgoals[0].title, "现状分析");
    }

    #[test]
    fn template_category_serializes() {
        let cat = TemplateCategory::Refactoring;
        let json = serde_json::to_string(&cat).unwrap();
        assert_eq!(json, "\"refactoring\"");
    }
}
