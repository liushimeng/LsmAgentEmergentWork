//! Main-Work Agent:流程层,WorkFlow 编排。
//!
//! 接收 Yolo / Plan 转发的任务,拆出 WorkFlow 列表(每个 WorkFlow 委派给 SubAgent-Work)。
//! 工具集:Bash(只读) + Read(用于查看项目状态)。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::context::AgentRole;
use crate::agent::memory;
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::{AgentError, Result};
use crate::llm::ChatMessage;

/// 单个 WorkFlow 规格(Main-Work 输出)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkFlowSpec {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub branches: Vec<BranchSpec>,
    #[serde(default)]
    pub loops: Vec<LoopSpec>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub acceptance: Vec<String>,
    pub delegate_to: AgentRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSpec {
    pub condition: String,
    pub then: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopSpec {
    pub condition: String,
    pub over: String,
    #[serde(default)]
    pub max_iterations: Option<usize>,
}

/// Main-Work 输出(一个任务拆出多个 WorkFlow)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkFlowPlan {
    #[serde(default)]
    pub workflows: Vec<WorkFlowSpec>,
    #[serde(default)]
    pub summary: String,
}

/// Main-Work 执行器。
pub struct MainWorkRunner {
    agent: Agent,
    db: Arc<Db>,
}

impl MainWorkRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        let agent = Agent::new(llm, AgentProfile::main_work_profile());
        Self { agent, db }
    }

    /// 接收任务目标,产出 WorkFlow 列表。
    pub async fn plan_workflows(
        &self,
        goal: &str,
        decomposition: &[String],
        session_id: &str,
    ) -> Result<WorkFlowPlan> {
        let mut prompt = String::new();
        prompt.push_str(&format!("【Main-Work 任务编排】\n目标: {}\n", goal));
        if !decomposition.is_empty() {
            prompt.push_str("\nYolo 给出的分解步骤(可参考,不一定照搬):\n");
            for (i, s) in decomposition.iter().enumerate() {
                prompt.push_str(&format!("  {}. {}\n", i + 1, s));
            }
        }
        prompt.push_str(
            "\n请按 JSON 格式输出 workflows 数组。每个 workflow 必须包含 \
             id/name/steps/depends_on/acceptance/delegate_to 字段。",
        );

        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage) = self.agent.run_session(&mut sub_session).await?;
        let plan = parse_workflow_plan(&text).unwrap_or_else(|e| {
            tracing::warn!("Main-Work 解析失败,使用单 WorkFlow 兜底: {}", e);
            WorkFlowPlan {
                workflows: vec![WorkFlowSpec {
                    id: "wf-1".into(),
                    name: "默认流程".into(),
                    steps: decomposition.to_vec(),
                    branches: vec![],
                    loops: vec![],
                    depends_on: vec![],
                    acceptance: vec!["完成目标".into()],
                    delegate_to: AgentRole::SubAgent,
                }],
                summary: "Main-Work JSON 解析失败,已使用单 WorkFlow 兜底".into(),
            }
        });

        let _ = memory::record_entry(
            &self.db,
            AgentRole::MainWork,
            session_id,
            goal,
            &format!("workflows: {}", plan.workflows.len()),
            None,
            serde_json::json!({ "workflow_ids": plan.workflows.iter().map(|w| &w.id).collect::<Vec<_>>() }),
        );

        let _ = usage; // 暂不累计
        Ok(plan)
    }

    /// 从 Plan 文档中解析 WorkFlow 段(Plan Agent 的 Markdown 输出)。
    pub fn parse_plan(&self, plan_path: &std::path::Path) -> Result<WorkFlowPlan> {
        let content = std::fs::read_to_string(plan_path).map_err(|e| {
            AgentError::PlanGen(format!("无法读取 Plan 文档 {}: {}", plan_path.display(), e))
        })?;
        parse_plan_markdown(&content)
    }
}

/// 拓扑排序(返回执行顺序)。
///
/// 简单 Kahn 算法;检测循环依赖。
pub fn topo_sort(workflows: &[WorkFlowSpec]) -> Result<Vec<WorkFlowSpec>> {
    let mut by_id: std::collections::HashMap<&str, &WorkFlowSpec> =
        std::collections::HashMap::new();
    for w in workflows {
        by_id.insert(w.id.as_str(), w);
    }
    let mut in_degree: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for w in workflows {
        *in_degree.entry(w.id.as_str()).or_insert(0) += 0;
        for dep in &w.depends_on {
            if !by_id.contains_key(dep.as_str()) {
                return Err(AgentError::WorkflowTopology(format!(
                    "wf={} 依赖未知 wf={}",
                    w.id,
                    dep
                )));
            }
            *in_degree.entry(dep.as_str()).or_insert(0) += 0;
        }
    }
    // 真实 indegree:对于每个 wf,它的 indegree 是「依赖它的 wf 数」,还是「它依赖的 wf 数」?
    // 这里我们要求 indegree = 0 的先执行;使用「它依赖的 wf 数」
    let mut in_degree: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for w in workflows {
        in_degree.insert(w.id.as_str(), w.depends_on.len());
    }
    let mut queue: std::collections::VecDeque<&str> = in_degree
        .iter()
        .filter_map(|(k, v)| if *v == 0 { Some(*k) } else { None })
        .collect();
    let mut order = Vec::new();
    while let Some(id) = queue.pop_front() {
        if let Some(w) = by_id.get(id) {
            order.push((*w).clone());
        }
        for w in workflows {
            if w.depends_on.iter().any(|d| d == id) {
                let entry = in_degree.entry(w.id.as_str()).or_insert(0);
                *entry = entry.saturating_sub(1);
                if *entry == 0 && !queue.contains(&w.id.as_str()) {
                    queue.push_back(w.id.as_str());
                }
            }
        }
    }
    if order.len() != workflows.len() {
        return Err(AgentError::WorkflowTopology(
            "检测到循环依赖,无法拓扑排序".into(),
        ));
    }
    Ok(order)
}

/// 解析 Main-Work JSON 输出(支持代码块 / 裸 JSON)。
pub fn parse_workflow_plan(text: &str) -> Result<WorkFlowPlan> {
    if let Some(json_str) = extract_json_block(text) {
        return serde_json::from_str::<WorkFlowPlan>(json_str).map_err(|e| {
            AgentError::WorkflowParse(format!("JSON 代码块解析失败: {}", e))
        });
    }
    if let Some(json_str) = extract_standalone_json(text) {
        return serde_json::from_str::<WorkFlowPlan>(json_str).map_err(|e| {
            AgentError::WorkflowParse(format!("JSON 对象解析失败: {}", e))
        });
    }
    Err(AgentError::WorkflowParse(
        "未找到合法的 WorkFlow JSON".into(),
    ))
}

fn extract_json_block(text: &str) -> Option<&str> {
    let start_marker = "```json";
    let start = text.find(start_marker)?;
    let content_start = start + start_marker.len();
    let content_start = text[content_start..]
        .find(|c: char| !c.is_whitespace())
        .map(|i| content_start + i)
        .unwrap_or(content_start);
    let end_marker = "```";
    let end = text[content_start..].find(end_marker)?;
    let json_text = &text[content_start..content_start + end];
    let json_text = json_text.trim();
    if json_text.is_empty() { None } else { Some(json_text) }
}

fn extract_standalone_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut end = None;
    for (i, c) in text[start..].char_indices() {
        if escape { escape = false; continue; }
        if c == '\\' && in_string { escape = true; continue; }
        if c == '"' { in_string = !in_string; continue; }
        if in_string { continue; }
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 { end = Some(start + i + 1); break; }
            }
            _ => {}
        }
    }
    end.map(|e| &text[start..e])
}

/// 从 Plan Markdown 提取 WorkFlow 列表(基于 `## 二、WorkFlow 拆解` 段)。
pub fn parse_plan_markdown(content: &str) -> Result<WorkFlowPlan> {
    // 简单行级解析:查找 `### WorkFlow N:` 块,提取 acceptance / 依赖
    let mut workflows = Vec::new();
    let mut in_workflows_section = false;
    let mut current: Option<WorkFlowSpec> = None;
    for line in content.lines() {
        let t = line.trim();
        if !in_workflows_section && (t.starts_with("## 二") || t.contains("WorkFlow 拆解")) {
            in_workflows_section = true;
            continue;
        }
        if !in_workflows_section {
            continue;
        }
        if t.starts_with("## ") && !t.contains("WorkFlow 拆解") {
            // 进入下一段
            if let Some(w) = current.take() {
                workflows.push(w);
            }
            break;
        }
        if let Some(rest) = t.strip_prefix("### WorkFlow ") {
            // 保存上一个
            if let Some(w) = current.take() {
                workflows.push(w);
            }
            // 提取 id 与 name(形如 "1: 名称" 或 "1 名称")
            let after_id = rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == ':' || c == ' ').to_string();
            // 提取纯数字 id
            let id_num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let id = if id_num.is_empty() {
                format!("wf-{}", workflows.len() + 1)
            } else {
                format!("wf-{id_num}")
            };
            current = Some(WorkFlowSpec {
                id,
                name: after_id.trim_start_matches(':').trim().to_string(),
                steps: Vec::new(),
                branches: Vec::new(),
                loops: Vec::new(),
                depends_on: Vec::new(),
                acceptance: Vec::new(),
                delegate_to: AgentRole::SubAgent,
            });
        }
        if let Some(w) = current.as_mut() {
            if t.starts_with("- 依赖:") || t.starts_with("依赖:") {
                // 提取冒号之后的内容
                let after_colon = if let Some(idx) = t.find(':') {
                    t[idx + 1..].trim()
                } else {
                    ""
                };
                if after_colon == "无" || after_colon.is_empty() {
                    continue;
                }
                for part in after_colon.split(',') {
                    let s = part.trim();
                    if !s.is_empty() {
                        w.depends_on.push(s.to_string());
                    }
                }
            } else if t.starts_with("- 验收标准:") || t.starts_with("验收标准:") {
                let after_colon = if let Some(idx) = t.find(':') {
                    t[idx + 1..].trim().to_string()
                } else {
                    String::new()
                };
                w.acceptance.push(after_colon);
            } else if t.starts_with("- [ ]") || t.starts_with("  - [ ]") {
                w.steps
                    .push(t.trim_start_matches(|c: char| c == ' ').trim_start_matches("- [ ]").trim().to_string());
            }
        }
    }
    if let Some(w) = current.take() {
        workflows.push(w);
    }

    if workflows.is_empty() {
        return Err(AgentError::PlanGen(
            "Plan 文档未解析出任何 WorkFlow".into(),
        ));
    }

    Ok(WorkFlowPlan {
        workflows,
        summary: "从 Plan 文档解析得到".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(id: &str, deps: &[&str]) -> WorkFlowSpec {
        WorkFlowSpec {
            id: id.to_string(),
            name: format!("workflow {id}"),
            steps: vec![],
            branches: vec![],
            loops: vec![],
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
        }
    }

    #[test]
    fn topo_sort_simple_chain() {
        let workflows = vec![
            wf("wf-1", &[]),
            wf("wf-2", &["wf-1"]),
            wf("wf-3", &["wf-2"]),
        ];
        let order = topo_sort(&workflows).unwrap();
        let ids: Vec<String> = order.iter().map(|w| w.id.clone()).collect();
        assert_eq!(ids, vec!["wf-1", "wf-2", "wf-3"]);
    }

    #[test]
    fn topo_sort_independent() {
        let workflows = vec![wf("wf-1", &[]), wf("wf-2", &[]), wf("wf-3", &[])];
        let order = topo_sort(&workflows).unwrap();
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn topo_sort_detects_cycle() {
        let workflows = vec![wf("wf-1", &["wf-2"]), wf("wf-2", &["wf-1"])];
        assert!(topo_sort(&workflows).is_err());
    }

    #[test]
    fn topo_sort_unknown_dep() {
        let workflows = vec![wf("wf-1", &["wf-x"])];
        assert!(topo_sort(&workflows).is_err());
    }

    #[test]
    fn parse_workflow_plan_from_block() {
        let text = r#"
```json
{
  "workflows": [
    {"id":"wf-1","name":"a","steps":["s1"],"acceptance":["ok"],"delegate_to":"subagent","depends_on":[]}
  ],
  "summary": "test"
}
```"#;
        let plan = parse_workflow_plan(text).unwrap();
        assert_eq!(plan.workflows.len(), 1);
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert_eq!(plan.workflows[0].delegate_to, AgentRole::SubAgent);
    }

    #[test]
    fn parse_plan_markdown_extracts_workflows() {
        let md = r#"
# 任务方案:test

## 一、目标
目标内容

## 二、WorkFlow 拆解
### WorkFlow 1: 读取源文件
- 步骤:
  - [ ] 读取 src/foo.rs
  - [ ] 解析函数
- 委派 Agent: SubAgent-Work
- 依赖: 无
- 验收标准: 解析成功

### WorkFlow 2: 修改源文件
- 步骤:
  - [ ] 替换函数
- 委派 Agent: SubAgent-Work
- 依赖: wf-1
- 验收标准: cargo test 通过

## 三、关键决策
决策
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("parse failed: {e}"));
        assert_eq!(plan.workflows.len(), 2, "应解析出 2 个 WorkFlow");
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert_eq!(plan.workflows[0].steps.len(), 2);
        assert_eq!(plan.workflows[1].depends_on, vec!["wf-1"]);
    }
}