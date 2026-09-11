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
use crate::llm::{ChatMessage, Usage};

/// 单个 WorkFlow 规格(Main-Work 输出)
///
/// 2026-09-10 第 25 轮(F1):steps/branches/loops/depends_on/acceptance/delegate_to
/// 全部走宽松反序列化 —— 真实 LLM 高频把 branches 写成字符串数组、acceptance 写成
/// 单字符串、delegate_to 写成 "SubAgent-Work" 等别名,严格 serde 校验会丢弃整份计划
/// (D06 实测 3/3 失败,2605 token 高质量编排被整体浪费)。详见
/// `tmpPlan/2026-09-10_08-D06测试与MainWork解析宽松化及重试反馈修复方案.md`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkFlowSpec {
    pub id: String,
    pub name: String,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub steps: Vec<String>,
    #[serde(default, deserialize_with = "lenient_branches")]
    pub branches: Vec<BranchSpec>,
    #[serde(default, deserialize_with = "lenient_loops")]
    pub loops: Vec<LoopSpec>,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub depends_on: Vec<String>,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub acceptance: Vec<String>,
    #[serde(default = "default_delegate_to", deserialize_with = "lenient_delegate_to")]
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
    /// 解析失败走了兜底单 WorkFlow(F2):`run_medium` 据此跳过 QC-main
    /// (兜底计划带自证失败的 summary,送 QC 必然 fail+retryable,形成必败重试循环),
    /// 直接进入执行层,由每 WorkFlow 的 QC 把守真实产物。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub degraded: bool,
}

/// 宽松 `Vec<String>` 反序列化:对象数组(标准)/ 单字符串 → 数组。
fn lenient_strings<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Many(Vec<String>),
        One(String),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Many(v) => Ok(v),
        Raw::One(s) => Ok(vec![s]),
    }
}

/// 把 "条件: 动作" 形态的自然语言字符串切成 (condition, then)。
/// 分隔符取 `：` `:` `→` `->` 中**最早出现**的一个;切不开则整体归 condition。
fn split_condition_then(text: &str) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    let candidates = ["：", ":", "→", "->"];
    let mut best: Option<(usize, usize, usize)> = None; // (char_idx, sep_chars, sep_bytes)
    for sep in candidates {
        if let Some(byte_idx) = text.find(sep) {
            let char_idx = text[..byte_idx].chars().count();
            if best.map_or(true, |(b, _, _)| char_idx < b) {
                best = Some((char_idx, sep.chars().count(), sep.len()));
            }
        }
    }
    match best {
        Some((idx, sep_chars, _)) => (
            chars[..idx].iter().collect::<String>().trim().to_string(),
            chars[idx + sep_chars..]
                .iter()
                .collect::<String>()
                .trim()
                .to_string(),
        ),
        None => (text.trim().to_string(), String::new()),
    }
}

/// 宽松 branches 反序列化(F1):对象数组 / 字符串数组 / 单字符串。
/// 字符串形态 `"若 X 失败: 改用 Y"` → `BranchSpec { condition: "若 X 失败", then: "改用 Y" }`。
fn lenient_branches<'de, D>(deserializer: D) -> std::result::Result<Vec<BranchSpec>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum RawBranch {
        Object(BranchSpec),
        Text(String),
    }
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum RawList {
        Many(Vec<RawBranch>),
        One(RawBranch),
    }
    fn convert(r: RawBranch) -> BranchSpec {
        match r {
            RawBranch::Object(b) => b,
            RawBranch::Text(s) => {
                let (condition, then) = split_condition_then(&s);
                BranchSpec { condition, then }
            }
        }
    }
    Ok(match RawList::deserialize(deserializer)? {
        RawList::Many(v) => v.into_iter().map(convert).collect(),
        RawList::One(r) => vec![convert(r)],
    })
}

/// 宽松 loops 反序列化(F1):对象数组 / 字符串数组 / 单字符串。
/// 字符串形态 `"对每个文件: 执行 X"` → `LoopSpec { condition: "对每个文件", over: "执行 X" }`。
fn lenient_loops<'de, D>(deserializer: D) -> std::result::Result<Vec<LoopSpec>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum RawLoop {
        Object(LoopSpec),
        Text(String),
    }
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum RawList {
        Many(Vec<RawLoop>),
        One(RawLoop),
    }
    fn convert(r: RawLoop) -> LoopSpec {
        match r {
            RawLoop::Object(l) => l,
            RawLoop::Text(s) => {
                let (condition, over) = split_condition_then(&s);
                LoopSpec {
                    condition,
                    over,
                    max_iterations: None,
                }
            }
        }
    }
    Ok(match RawList::deserialize(deserializer)? {
        RawList::Many(v) => v.into_iter().map(convert).collect(),
        RawList::One(r) => vec![convert(r)],
    })
}

/// 宽松 delegate_to 反序列化(F1):接受 "SubAgent-Work" / "main-work" 等别名;
/// 未知变体回退执行层 SubAgent,绝不因该字段丢整份计划。
fn default_delegate_to() -> AgentRole {
    AgentRole::SubAgent
}

fn lenient_delegate_to<'de, D>(deserializer: D) -> std::result::Result<AgentRole, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let norm = raw.trim().to_lowercase().replace(['-', '_', ' '], "");
    let role = match norm.as_str() {
        "subagent" | "subagentwork" | "work" | "执行层" => AgentRole::SubAgent,
        "main" | "mainwork" | "mainworkagent" => AgentRole::MainWork,
        "yolo" => AgentRole::Yolo,
        "plan" => AgentRole::Plan,
        "quality" | "qualitycheck" | "qc" => AgentRole::QualityCheck,
        "session" | "sessioncontext" => AgentRole::SessionContext,
        "compact" => AgentRole::Compact,
        _ => AgentRole::SubAgent,
    };
    Ok(role)
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

    /// 接收任务目标,产出 WorkFlow 列表(2026-09-09 第 14 轮:带回 LLM Usage 用于 Orchestrator 累加)。
    ///
    /// `retry_hint`(2026-09-10 第 25 轮 F3):上一轮执行/QC 的失败原因,重试轮回灌给
    /// Main-Work 参考规避,消除「盲重试」;首轮传空串。
    ///
    /// `original_prompt`(2026-09-11 第三十四轮 LA-1):用户原始 prompt 透传,与
    /// SubAgent 的 #P-A 修复同源 —— 此前 Main-Work 只看到 Yolo 抽象摘要
    /// (goal_summary/分解步骤),拆解脱离用户原始意图的风险与 SubAgent 完全一致;
    /// 现在编排 prompt 头部附「用户原始输入」段。
    pub async fn plan_workflows(
        &self,
        goal: &str,
        decomposition: &[String],
        session_id: &str,
        retry_hint: &str,
        original_prompt: Option<&str>,
    ) -> Result<(WorkFlowPlan, Usage)> {
        let mut prompt = String::new();
        prompt.push_str(&format!("【Main-Work 任务编排】\n目标: {}\n", goal));
        if let Some(orig) = original_prompt.filter(|s| !s.trim().is_empty()) {
            prompt.push_str(&format!("\n用户原始输入:\n{orig}\n"));
        }
        if !decomposition.is_empty() {
            prompt.push_str("\nYolo 给出的分解步骤(可参考,不一定照搬):\n");
            for (i, s) in decomposition.iter().enumerate() {
                prompt.push_str(&format!("  {}. {}\n", i + 1, s));
            }
        }
        if !retry_hint.is_empty() {
            prompt.push_str(&format!(
                "\n【重要】上一轮同目标执行失败,失败原因:\n{retry_hint}\n\
                 请在本轮拆解中针对上述原因调整编排(补充前置检查 / 拆细步骤 / 明确验收命令)。\n"
            ));
        }
        // F8:补全 branches/loops 的 schema 示例并注明可省略 —— 此前提示词只列了
        // 六个必填字段,LLM 自行发明 branches 字符串形态导致类型失配(F1 的源头)。
        prompt.push_str(
            "\n请严格按以下 JSON 结构输出(可包裹在 ```json 代码块中):\n\
             {\"workflows\": [{\"id\": \"wf-1\", \"name\": \"流程名\", \"steps\": [\"步骤\"], \
             \"branches\": [\"条件: 动作\"], \"loops\": [\"条件: 遍历对象\"], \"depends_on\": [], \
             \"acceptance\": [\"可验证的验收标准\"], \"delegate_to\": \"subagent\"}], \"summary\": \"编排思路\"}\n\
             约束:\n\
             - id/name/steps/acceptance/delegate_to 必填;branches/loops/depends_on/summary 可省略。\n\
             - branches/loops 元素是字符串(形如 \"条件: 动作\")或对象({\"condition\":…,\"then\":…} / {\"condition\":…,\"over\":…})均可。\n\
             - delegate_to 固定填 \"subagent\"。\n\
             - acceptance 必须是可执行验证的验收标准(命令 / 可比对的预期输出),不要写「完成目标」这类空话。",
        );

        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;
        let plan = parse_workflow_plan(&text).unwrap_or_else(|e| {
            tracing::warn!("Main-Work 解析失败,使用单 WorkFlow 兜底: {}", e);
            // F2:兜底 acceptance 继承 Yolo 分解步骤(可验证清单),不再退化「完成目标」;
            // degraded=true 供 run_medium 跳过 QC-main,消除必败重试循环。
            let inherited = if decomposition.is_empty() {
                vec!["完成目标".to_string()]
            } else {
                decomposition.to_vec()
            };
            WorkFlowPlan {
                workflows: vec![WorkFlowSpec {
                    id: "wf-1".into(),
                    name: "默认流程".into(),
                    steps: decomposition.to_vec(),
                    branches: vec![],
                    loops: vec![],
                    depends_on: vec![],
                    acceptance: inherited,
                    delegate_to: AgentRole::SubAgent,
                }],
                summary: "Main-Work JSON 解析失败,已使用单 WorkFlow 兜底".into(),
                degraded: true,
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

        Ok((plan, usage))
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
/// 基于 `topo_layers` 分层结果扁平化(单一事实源);检测循环依赖与未知依赖。
pub fn topo_sort(workflows: &[WorkFlowSpec]) -> Result<Vec<WorkFlowSpec>> {
    Ok(topo_layers(workflows)?.into_iter().flatten().collect())
}

/// 拓扑分层(Kahn 分层):同层 WorkFlow 互相无依赖,可并行执行;跨层严格串行。
///
/// - 第 0 层 = 入度 0 节点;每消费一层,其后继入度 -1,归零者入下一层。
/// - **确定性**:层内顺序 = 原 `workflows` 数组顺序(不依赖 HashMap 遍历序)。
/// - 未知依赖 / 环:与 `topo_sort` 一致,报 `AgentError::WorkflowTopology`。
pub fn topo_layers(workflows: &[WorkFlowSpec]) -> Result<Vec<Vec<WorkFlowSpec>>> {
    let mut by_id: std::collections::HashMap<&str, &WorkFlowSpec> =
        std::collections::HashMap::new();
    for w in workflows {
        by_id.insert(w.id.as_str(), w);
    }
    // 入度 = 它依赖的 wf 数;先校验所有依赖已知
    let mut in_degree: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for w in workflows {
        for dep in &w.depends_on {
            if !by_id.contains_key(dep.as_str()) {
                return Err(AgentError::WorkflowTopology(format!(
                    "wf={} 依赖未知 wf={}",
                    w.id, dep
                )));
            }
        }
        in_degree.insert(w.id.as_str(), w.depends_on.len());
    }

    let mut layers: Vec<Vec<WorkFlowSpec>> = Vec::new();
    let mut placed: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut total = 0usize;
    loop {
        // 本轮可放置:入度 0 且尚未放置;按原数组顺序扫描保证确定性
        let layer: Vec<&WorkFlowSpec> = workflows
            .iter()
            .filter(|w| {
                !placed.contains(w.id.as_str())
                    && in_degree.get(w.id.as_str()).copied().unwrap_or(0) == 0
            })
            .collect();
        if layer.is_empty() {
            break;
        }
        for w in &layer {
            placed.insert(w.id.as_str());
        }
        total += layer.len();
        // 消费本层:递减所有依赖本层节点的后继入度
        let layer_ids: std::collections::HashSet<&str> =
            layer.iter().map(|w| w.id.as_str()).collect();
        for w in workflows {
            if placed.contains(w.id.as_str()) {
                continue;
            }
            let dec = w
                .depends_on
                .iter()
                .filter(|d| layer_ids.contains(d.as_str()))
                .count();
            if dec > 0 {
                let entry = in_degree.entry(w.id.as_str()).or_insert(0);
                *entry = entry.saturating_sub(dec);
            }
        }
        layers.push(layer.into_iter().cloned().collect());
    }
    if total != workflows.len() {
        return Err(AgentError::WorkflowTopology(
            "检测到循环依赖,无法拓扑排序".into(),
        ));
    }
    Ok(layers)
}

/// 解析 Main-Work JSON 输出(支持代码块 / 裸 JSON)。
/// 直接解析失败时自动走 JSON 修复链(`json_repair`),修复不了仍 WorkflowParse。
pub fn parse_workflow_plan(text: &str) -> Result<WorkFlowPlan> {
    if let Some(json_str) = extract_json_block(text) {
        // Main-Work 路径:启用 Tier-2 截断补全(关联报告: 2026-09-09_04 D-001)。
        return crate::agent::json_repair::try_parse_lenient(json_str)
            .map_err(AgentError::WorkflowParse);
    }
    if let Some(json_str) = extract_standalone_json(text) {
        return crate::agent::json_repair::try_parse_lenient(json_str)
            .map_err(AgentError::WorkflowParse);
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

/// 从 Plan Markdown 提取 WorkFlow 列表。
///
/// 双通道解析:
/// 1) JSON 代码块兜底(兼容 Plan Agent 直接输出 JSON 的场景)
/// 2) Markdown 行级解析(基于 `## 二、WorkFlow 拆解` 段的 `### WorkFlow N:` 块)
pub fn parse_plan_markdown(content: &str) -> Result<WorkFlowPlan> {
    // 0) 兜底:尝试从 JSON 代码块解析(兼容 Plan Agent 输出 JSON 代码块格式的场景)
    // 关联修复: 2026-09-10 hard 任务 Plan 解析 Bug — Plan Agent 输出 JSON 代码块,
    // 但 parse_plan_markdown 只支持 markdown 行级格式,导致解析失败触发无意义重试循环。
    if let Some(json_str) = extract_json_block(content) {
        if let Ok(plan) = serde_json::from_str::<WorkFlowPlan>(json_str) {
            if !plan.workflows.is_empty() {
                return Ok(plan);
            }
        }
    }
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
        degraded: false,
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
    fn topo_layers_chain_one_per_layer() {
        let workflows = vec![
            wf("wf-1", &[]),
            wf("wf-2", &["wf-1"]),
            wf("wf-3", &["wf-2"]),
        ];
        let layers = topo_layers(&workflows).unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0][0].id, "wf-1");
        assert_eq!(layers[1][0].id, "wf-2");
        assert_eq!(layers[2][0].id, "wf-3");
    }

    #[test]
    fn topo_layers_independent_all_in_one_layer() {
        let workflows = vec![wf("wf-1", &[]), wf("wf-2", &[]), wf("wf-3", &[])];
        let layers = topo_layers(&workflows).unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].len(), 3);
        // 层内顺序 = 原数组顺序(确定性)
        let ids: Vec<&str> = layers[0].iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["wf-1", "wf-2", "wf-3"]);
    }

    #[test]
    fn topo_layers_diamond() {
        // wf-1 → {wf-2, wf-3} → wf-4
        let workflows = vec![
            wf("wf-1", &[]),
            wf("wf-2", &["wf-1"]),
            wf("wf-3", &["wf-1"]),
            wf("wf-4", &["wf-2", "wf-3"]),
        ];
        let layers = topo_layers(&workflows).unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0].len(), 1);
        assert_eq!(layers[1].len(), 2, "wf-2/wf-3 同层可并行");
        assert_eq!(layers[2][0].id, "wf-4");
    }

    #[test]
    fn topo_layers_cycle_error() {
        let workflows = vec![wf("wf-1", &["wf-2"]), wf("wf-2", &["wf-1"])];
        assert!(topo_layers(&workflows).is_err());
    }

    #[test]
    fn topo_layers_unknown_dep_error() {
        let workflows = vec![wf("wf-1", &["wf-x"])];
        assert!(topo_layers(&workflows).is_err());
    }

    #[test]
    fn topo_layers_flatten_matches_topo_sort() {
        // 混合图:flatten(分层) 必须等于 topo_sort 输出(同一事实源)
        let workflows = vec![
            wf("wf-1", &[]),
            wf("wf-2", &[]),
            wf("wf-3", &["wf-1"]),
            wf("wf-4", &["wf-2", "wf-3"]),
        ];
        let flat: Vec<String> = topo_layers(&workflows)
            .unwrap()
            .into_iter()
            .flatten()
            .map(|w| w.id)
            .collect();
        let sorted: Vec<String> = topo_sort(&workflows)
            .unwrap()
            .into_iter()
            .map(|w| w.id)
            .collect();
        assert_eq!(flat, sorted);
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

    /// 验证 parse_plan_markdown 支持 JSON 代码块格式(兜底解析)。
    /// 关联修复: 2026-09-10 hard 任务 Plan 解析 Bug。
    #[test]
    fn parse_plan_markdown_supports_json_code_block() {
        let md = r#"# 方案

```json
{"workflows": [{"id": "wf-1", "name": "执行验证", "steps": ["运行 echo LAEW_MOCK_OK"], "acceptance": ["输出包含 LAEW_MOCK_OK"], "delegate_to": "subagent"}]}
```
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("JSON 代码块解析失败: {e}"));
        assert_eq!(plan.workflows.len(), 1, "应解析出 1 个 WorkFlow");
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert_eq!(plan.workflows[0].name, "执行验证");
        assert_eq!(plan.workflows[0].steps.len(), 1);
    }

    /// F1(2026-09-10 第 25 轮):真实 LLM(D06 实测 MiniMax-M3 3/3)把 branches
    /// 写成字符串数组,宽松反序列化必须解析成功而非丢弃整份计划。
    #[test]
    fn parse_lenient_branches_string_array() {
        let json = r#"{"workflows": [{"id": "wf-1", "name": "创建文件", "steps": ["写入 JSON"],
            "branches": ["若 jq . 解析失败(JSON 语法错误): 修正文件内容后重新自检,最多重试 1 次",
                          "IF jq 不可用或 jq 命令非零退出 → 改用 python3 等效提取"],
            "loops": [], "depends_on": [],
            "acceptance": ["test -f openai_chat_response.json 退出码为 0"],
            "delegate_to": "subagent"}]}"#;
        let plan: WorkFlowPlan =
            serde_json::from_str(json).expect("字符串形态 branches 必须可解析");
        let branches = &plan.workflows[0].branches;
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].condition, "若 jq . 解析失败(JSON 语法错误)");
        assert_eq!(branches[0].then, "修正文件内容后重新自检,最多重试 1 次");
        assert_eq!(branches[1].condition, "IF jq 不可用或 jq 命令非零退出");
        assert_eq!(branches[1].then, "改用 python3 等效提取");
    }

    /// F1:branches 为单字符串 / loops 为字符串数组 / acceptance 为单字符串。
    #[test]
    fn parse_lenient_scalar_shapes() {
        let json = r#"{"workflows": [{"id": "wf-1", "name": "n",
            "branches": "若失败: 重试一次",
            "loops": ["对每个文件: 执行校验"],
            "acceptance": "文件存在"}]}"#;
        let plan: WorkFlowPlan = serde_json::from_str(json).expect("标量形态必须可解析");
        let wf = &plan.workflows[0];
        assert_eq!(wf.branches.len(), 1);
        assert_eq!(wf.branches[0].condition, "若失败");
        assert_eq!(wf.branches[0].then, "重试一次");
        assert_eq!(wf.loops.len(), 1);
        assert_eq!(wf.loops[0].condition, "对每个文件");
        assert_eq!(wf.loops[0].over, "执行校验");
        assert_eq!(wf.acceptance, vec!["文件存在"]);
    }

    /// F1:对象数组(标准形态)与对象/字符串混合数组都兼容。
    #[test]
    fn parse_lenient_branches_object_and_mixed() {
        let json = r#"{"workflows": [{"id": "wf-1", "name": "n",
            "branches": [{"condition": "a", "then": "b"}, "若 X: 则 Y"]}]}"#;
        let plan: WorkFlowPlan = serde_json::from_str(json).expect("混合形态必须可解析");
        let branches = &plan.workflows[0].branches;
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].condition, "a");
        assert_eq!(branches[0].then, "b");
        assert_eq!(branches[1].condition, "若 X");
        assert_eq!(branches[1].then, "则 Y");
    }

    /// F1:delegate_to 接受别名;未知值与字段缺失回退 SubAgent。
    #[test]
    fn parse_lenient_delegate_to() {
        let mk = |v: &str| {
            format!(r#"{{"workflows": [{{"id": "wf-1", "name": "n", "delegate_to": {v}}}]}}"#)
        };
        for alias in ["\"SubAgent-Work\"", "\"subagent\"", "\"work\"", "\"MAIN-WORK\""] {
            let plan: WorkFlowPlan = serde_json::from_str(&mk(alias)).unwrap();
            let expected = if alias.contains("MAIN") {
                AgentRole::MainWork
            } else {
                AgentRole::SubAgent
            };
            assert_eq!(plan.workflows[0].delegate_to, expected, "alias={alias}");
        }
        // 未知值 → SubAgent 兜底
        let plan: WorkFlowPlan = serde_json::from_str(&mk("\"someone-else\"")).unwrap();
        assert_eq!(plan.workflows[0].delegate_to, AgentRole::SubAgent);
        // 字段缺失 → 默认 SubAgent
        let plan: WorkFlowPlan =
            serde_json::from_str(r#"{"workflows": [{"id": "wf-1", "name": "n"}]}"#).unwrap();
        assert_eq!(plan.workflows[0].delegate_to, AgentRole::SubAgent);
    }

    /// F2:split_condition_then 的分隔符取最早出现者,切不开归 condition。
    #[test]
    fn split_condition_then_earliest_separator() {
        let (c, t) = super::split_condition_then("若超时: 重试,若仍失败: 上报");
        assert_eq!(c, "若超时");
        assert_eq!(t, "重试,若仍失败: 上报");
        let (c, t) = super::split_condition_then("无分隔符的整句");
        assert_eq!(c, "无分隔符的整句");
        assert_eq!(t, "");
    }
}