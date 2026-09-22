//! WorkFlow 方案解析(2026-09-17 自 main_work.rs 拆分)。
//!
//! Main-Work JSON 输出解析(sanitize + JSON 修复链)与 Plan Markdown 行级解析双通道。

use super::*;

/// 解析 Main-Work JSON 输出(支持代码块 / 裸 JSON)。
/// 直接解析失败时自动走 JSON 修复链(`json_repair`),修复不了仍 WorkflowParse。
///
/// 2026-09-16 第 65 轮 P0-A:在 `try_parse_lenient` 之前先走
/// [`crate::agent::workflow_json_validate::sanitize_workflow_text`],把 LLM 常见的
/// 智能引号 / 嵌套中文角括号 / 中文字符串内层 ASCII `"` / Unicode Modifier Letter
/// 块(`ᴬᴵᴬ` 等)/ 控制字符 5 类错误模式归一化,根因修复上一轮「Main-Work 反复重拆」
/// 死循环。`extract_json_block` 已经切出 JSON 段,sanitize 不会破坏外层 LLM 解释文字。
pub fn parse_workflow_plan(text: &str) -> Result<WorkFlowPlan> {
    if let Some(json_str) = extract_json_block(text) {
        // P0-A:先 sanitize 再进 json_repair 修复链(关联:workflow_json_validate.rs)
        let sanitized = crate::agent::workflow_json_validate::sanitize_workflow_text(json_str);
        // Main-Work 路径:启用 Tier-2 截断补全(关联报告: 2026-09-09_04 D-001)。
        let mut plan = crate::agent::json_repair::try_parse_lenient(&sanitized)
            .map_err(AgentError::WorkflowParse)?;
        dedup_workflow_ids(&mut plan);
        // 2026-09-16 F2:归一化 depends_on(剥离 LLM 附加的变量透传注释)
        sanitize_depends_on(&mut plan);
        // 2026-09-16 第 54 轮补丁 B:基于步骤关键词自动纠正 delegate_to
        // (解决 osascript 步骤被错委派的问题)
        infer_delegate_to_for_plan(&mut plan);
        return Ok(plan);
    }
    if let Some(json_str) = extract_standalone_json(text) {
        // P0-A:同上,sanitize 后再解析
        let sanitized = crate::agent::workflow_json_validate::sanitize_workflow_text(json_str);
        let mut plan = crate::agent::json_repair::try_parse_lenient(&sanitized)
            .map_err(AgentError::WorkflowParse)?;
        dedup_workflow_ids(&mut plan);
        sanitize_depends_on(&mut plan);
        infer_delegate_to_for_plan(&mut plan);
        return Ok(plan);
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
    if json_text.is_empty() {
        None
    } else {
        Some(json_text)
    }
}

fn extract_standalone_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut end = None;
    for (i, c) in text[start..].char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' && in_string {
            escape = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i + 1);
                    break;
                }
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
        // P0-A:Plan 文档路径同样先 sanitize,防止 Plan Agent 输出含 Modifier Letter /
        // 嵌套角括号的 JSON 时直接放弃整份计划。
        let sanitized = crate::agent::workflow_json_validate::sanitize_workflow_text(json_str);
        if let Ok(plan) = serde_json::from_str::<WorkFlowPlan>(&sanitized) {
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
            let after_id = rest
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == ':' || c == ' ')
                .to_string();
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
                // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
                // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
                max_iterations: None, original_prompt: None, pre_explore: false,
            });
        }
        // I2c(2026-09-14 第 51 轮):兜底兼容表格行变体 `| **wf-1** | 名称 | 步骤 | 依赖 |`
        // —— Plan 提示词漂移的另一常见形态(et08/et09 实测,表格行含全部四要素,
        // 旧解析器对表格行零识别 → acceptance/depends 全丢或 0 workflow)。
        if t.starts_with('|') && t.contains("wf-") && !t.contains("---") {
            let cells: Vec<String> = t
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().trim_matches('*').trim().to_string())
                .collect();
            let id_cell = cells.first().cloned().unwrap_or_default();
            let id_num: String = id_cell
                .trim_start_matches("wf-")
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !id_num.is_empty() && cells.len() >= 3 {
                if let Some(w) = current.take() {
                    workflows.push(w);
                }
                let name = cells.get(1).cloned().unwrap_or_default();
                // 步骤列:整段作为一条步骤(保持语义完整,不粗暴切分)
                let steps: Vec<String> = cells
                    .get(2)
                    .map(|s| {
                        let s = s.trim();
                        if s.is_empty() {
                            Vec::new()
                        } else {
                            vec![s.to_string()]
                        }
                    })
                    .unwrap_or_default();
                // 依赖列:逗号/顿号分隔,忽略「无」与空
                let depends_on: Vec<String> = cells
                    .get(3)
                    .map(|d| {
                        d.split(|c| c == ',' || c == '、' || c == ';')
                            .map(|p| p.trim())
                            .filter(|p| !p.is_empty() && *p != "无" && *p != "-" && *p != "—")
                            .map(|p| p.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                current = Some(WorkFlowSpec {
                    id: format!("wf-{id_num}"),
                    name,
                    steps,
                    branches: Vec::new(),
                    loops: Vec::new(),
                    depends_on,
                    acceptance: Vec::new(),
                    delegate_to: AgentRole::SubAgent,
                    max_iterations: None, original_prompt: None, pre_explore: false,
                    // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
                });
                continue;
            }
        }
        // I2(2026-09-14 第 51 轮):兜底兼容粗体 bullet 变体 `- **wf-1 …** …` /
        if t.starts_with("- **wf-") || t.starts_with("**wf-") {
            let inner = t
                .trim_start_matches("- ")
                .trim_start_matches("**")
                .to_string();
            let id_num: String = inner
                .trim_start_matches("wf-")
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !id_num.is_empty() {
                // 拆出「wf-N 名称」与粗体闭合后的描述(描述兜底为首步骤)
                let head = &inner[..(3 + id_num.len()).min(inner.len())];
                let rest = &inner[head.len()..];
                let (name_raw, desc) = match rest.find("**") {
                    Some(i) => (
                        &rest[..i],
                        rest[i + 2..].trim_start_matches(':').trim().to_string(),
                    ),
                    None => (rest, String::new()),
                };
                let name = name_raw
                    .trim_start_matches(|c: char| {
                        matches!(c, '/' | ':' | '—' | '–' | '-' | ' ' | '\t')
                    })
                    .trim()
                    .to_string();
                if let Some(w) = current.take() {
                    workflows.push(w);
                }
                let mut steps = Vec::new();
                if !desc.is_empty() {
                    steps.push(desc);
                }
                current = Some(WorkFlowSpec {
                    id: format!("wf-{id_num}"),
                    name,
                    steps,
                    branches: Vec::new(),
                    loops: Vec::new(),
                    depends_on: Vec::new(),
                    acceptance: Vec::new(),
                    delegate_to: AgentRole::SubAgent,
                    max_iterations: None, original_prompt: None, pre_explore: false,
                    // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
                });
                continue;
            }
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
                w.steps.push(
                    t.trim_start_matches(|c: char| c == ' ')
                        .trim_start_matches("- [ ]")
                        .trim()
                        .to_string(),
                );
            } else if t.starts_with("- ")
                && !t.starts_with("- 委派")
                && !t.starts_with("- 步骤")
                && !t.starts_with("- 依赖")
                && !t.starts_with("- 验收标准")
            {
                // I2(2026-09-14 第 51 轮):兜底把普通 bullet 视为步骤 ——
                // 粗体变体方案(`- **wf-N …**` 开头)没有 `- [ ]` 步骤标记,
                // 其后的说明 bullet 即步骤本体;模板格式方案不受影响
                // (其普通 bullet 仅 `- 委派 Agent:` 一类,已在上面排除)。
                let step = t.trim_start_matches("- ").trim();
                if !step.is_empty() {
                    w.steps.push(step.to_string());
                }
            }
        }
    }
    if let Some(w) = current.take() {
        workflows.push(w);
    }

    if workflows.is_empty() {
        return Err(AgentError::PlanGen("Plan 文档未解析出任何 WorkFlow".into()));
    }

    let mut plan = WorkFlowPlan {
        workflows,
        summary: "从 Plan 文档解析得到".into(),
        degraded: false,
    };
    dedup_workflow_ids(&mut plan);
    // 2026-09-16 F2/F3:Plan 文档路径同样做 depends_on 归一化 + delegate_to 自动纠正
    // (修复「检视 Chrome 窗口」hard 任务拓扑失败 + 委派纠正的问题)
    sanitize_depends_on(&mut plan);
    infer_delegate_to_for_plan(&mut plan);
    Ok(plan)
}
