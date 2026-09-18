//! WorkFlow 计划的确定性校验与自动修复(2026-09-16 第 66 轮 P0-1/P0-2)。
//!
//! **背景**:此前 Main-Work 产出的 WorkFlowPlan 直接送 Quality-Check(LLM)做计划级
//! 质检。真实网关实测(2026-09-16 微信窗口操控任务):QC 三次以「loops.max_iterations
//! 为 null」「branches 为空」「目标名 Unicode 上标归一化后与 goal 不一致」等**品相问题**
//! 判 Fail,每轮重试 = Main-Work 30~50s + QC 10s+,三轮耗尽重试预算、162s 零执行。
//!
//! 本模块把「程序可判定」的部分从 LLM QC 手里收回来:
//!
//! - [`auto_repair_plan`]:解析后自动修复——`loops[].max_iterations` 从 condition/over
//!   文本回填上界(「最多N次」「max_iterations=N」等),无界遍历/滚动兜底 10 次;
//! - [`validate_plan_blocking`]:确定性阻断校验——workflows 空 / id 重复 / name、steps
//!   为空 / depends_on 未知或成环(复用 `topo_layers`)。返回空 Vec = 通过。
//!
//! Orchestrator `run_medium` 流程:parse → auto_repair → validate;
//! 校验失败直接 QualityFailure(精确原因、秒级重试,不烧 LLM QC);
//! 校验通过跳过 LLM QC-main(对齐 degraded 路径,真实产物由每单元 QC 把守)。

use crate::agent::main_work::{topo_layers, WorkFlowPlan};

/// 默认循环上界:文本提到滚动/遍历但未给次数时兜底。
const DEFAULT_LOOP_MAX_ITERATIONS: usize = 10;

/// 自动修复计划(当前仅回填 loops.max_iterations)。
///
/// 识别形态(condition + over 拼接文本,大小写不敏感):
/// - `最多10次` / `至多 5 次` / `不超过3次` / `最多滚动10次`
/// - `max_iterations=10` / `max_iterations: 5` / `max 10 iterations`
/// - `10次以内` / `5次内`
/// - 含「滚动 / 遍历 / 逐条 / 逐行 / 逐页」但无显式上界 → 兜底 [`DEFAULT_LOOP_MAX_ITERATIONS`]
pub fn auto_repair_plan(plan: &mut WorkFlowPlan) {
    for wf in plan.workflows.iter_mut() {
        for lp in wf.loops.iter_mut() {
            if lp.max_iterations.is_some() {
                continue;
            }
            let text = format!("{} {}", lp.condition, lp.over);
            if let Some(n) = extract_max_iterations(&text) {
                tracing::info!(
                    wf_id = %wf.id,
                    max_iterations = n,
                    "[2026-09-16 第66轮] loops.max_iterations 已从文本回填"
                );
                lp.max_iterations = Some(n);
            } else if mentions_unbounded_iteration(&text) {
                tracing::info!(
                    wf_id = %wf.id,
                    max_iterations = DEFAULT_LOOP_MAX_ITERATIONS,
                    "[2026-09-16 第66轮] loops 无上界遍历/滚动,已兜底 max_iterations"
                );
                lp.max_iterations = Some(DEFAULT_LOOP_MAX_ITERATIONS);
            }
        }
    }
}

/// 从文本提取显式循环上界。
fn extract_max_iterations(text: &str) -> Option<usize> {
    // 形态 1:max_iterations=10 / max_iterations: 5 / max 10 iterations
    let lower = text.to_lowercase();
    for pat in ["max_iterations", "max iteration", "max"] {
        if let Some(idx) = lower.find(pat) {
            let rest = &lower[idx + pat.len()..];
            let rest = rest.trim_start_matches(['=', ':', ' ', '_', 's']);
            if let Some(n) = take_leading_number(rest) {
                if (1..=1000).contains(&n) {
                    return Some(n);
                }
            }
        }
    }
    // 形态 2:最多N次 / 至多N次 / 不超过N次 / 限制N次 / N次以内 / N次内(数字与「次」间允许空格)
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    for i in 0..n {
        // 数字起点的窗口
        if chars[i].is_ascii_digit() {
            let mut j = i;
            let mut num = 0usize;
            while j < n && chars[j].is_ascii_digit() {
                num = num * 10 + chars[j].to_digit(10).unwrap() as usize;
                j += 1;
            }
            // 跳过数字与「次」之间的空白
            let mut k = j;
            while k < n && chars[k].is_whitespace() {
                k += 1;
            }
            let after: String = chars[k..(k + 3).min(n)].iter().collect();
            let before: String = chars[i.saturating_sub(3)..i].iter().collect();
            let followed_by_ci = after.starts_with('次');
            let preceded_by_bound =
                before.contains('多') || before.contains('至') || before.contains('过');
            if followed_by_ci && (preceded_by_bound || after.starts_with("次以") || after.starts_with("次内"))
                && (1..=1000).contains(&num)
            {
                return Some(num);
            }
        }
    }
    None
}

fn take_leading_number(s: &str) -> Option<usize> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// 文本是否提到「无显式上界的遍历/滚动」。
fn mentions_unbounded_iteration(text: &str) -> bool {
    let has_iter_verb = ["滚动", "遍历", "逐条", "逐行", "逐页", "scroll", "iterate"]
        .iter()
        .any(|k| text.to_lowercase().contains(k));
    // 已含「直到/直至 + 终止条件」的循环视为有界语义,但仍建议有数值上界;
    // 这里只把完全无界描述的判为需要兜底。
    has_iter_verb && extract_max_iterations(text).is_none()
}

/// 确定性阻断校验:返回阻断问题列表(空 = 通过)。
///
/// 只收录**程序可判定、LLM QC 误判率高**的硬问题;品相问题(acceptance 措辞、
/// branches 为空、名称风格)一律不判——那些由每单元 QC 对真实产物把守。
pub fn validate_plan_blocking(plan: &WorkFlowPlan) -> Vec<String> {
    let mut issues = Vec::new();
    if plan.workflows.is_empty() {
        issues.push("workflows 为空:Main-Work 未拆出任何流程单元".to_string());
        return issues;
    }
    let mut seen = std::collections::HashSet::new();
    for wf in &plan.workflows {
        if !seen.insert(wf.id.as_str()) {
            issues.push(format!("workflow id 重复: {}", wf.id));
        }
        if wf.name.trim().is_empty() {
            issues.push(format!("workflow {} 缺少 name", wf.id));
        }
        if wf.steps.is_empty() {
            issues.push(format!("workflow {} 缺少 steps(执行步骤为空)", wf.id));
        }
    }
    // 拓扑校验:未知依赖 / 循环依赖(复用 topo_layers 单一事实源)
    if let Err(e) = topo_layers(&plan.workflows) {
        issues.push(format!("依赖拓扑非法: {e}"));
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::AgentRole;
    use crate::agent::main_work::{LoopSpec, WorkFlowSpec};

    fn wf(id: &str, steps: Vec<&str>, loops: Vec<LoopSpec>) -> WorkFlowSpec {
        WorkFlowSpec {
            id: id.into(),
            name: format!("wf-{id}"),
            steps: steps.into_iter().map(String::from).collect(),
            branches: vec![],
            loops,
            depends_on: vec![],
            acceptance: vec!["ok".into()],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
        }
    }

    fn lp(condition: &str, over: &str) -> LoopSpec {
        LoopSpec {
            condition: condition.into(),
            over: over.into(),
            max_iterations: None,
        }
    }

    #[test]
    fn auto_repair_fills_max_iterations_from_text() {
        let cases: Vec<(&str, usize)> = vec![
            ("当未找到目标时: 滚动列表,最多滚动10次(max_iterations=10)", 10),
            ("最多 5 次重试", 5),
            ("至多3次: 重新检视", 3),
            ("不超过 7 次轮询", 7),
            ("max_iterations=20", 20),
            ("max_iterations: 8", 8),
            ("重试 10次以内", 10),
            ("最多15次内完成", 15),
        ];
        for (text, want) in cases {
            let mut plan = WorkFlowPlan {
                workflows: vec![wf("wf-1", vec!["s"], vec![lp(text, "")])],
                summary: String::new(),
                degraded: false,
            };
            auto_repair_plan(&mut plan);
            assert_eq!(
                plan.workflows[0].loops[0].max_iterations,
                Some(want),
                "文本: {text}"
            );
        }
    }

    #[test]
    fn auto_repair_defaults_unbounded_scroll_to_ten() {
        let mut plan = WorkFlowPlan {
            workflows: vec![wf(
                "wf-1",
                vec!["s"],
                vec![lp("当未找到目标用户时", "滚动通信录列表继续查找")],
            )],
            summary: String::new(),
            degraded: false,
        };
        auto_repair_plan(&mut plan);
        assert_eq!(
            plan.workflows[0].loops[0].max_iterations,
            Some(DEFAULT_LOOP_MAX_ITERATIONS)
        );
    }

    #[test]
    fn auto_repair_preserves_explicit_value_and_noop_loops() {
        let mut plan = WorkFlowPlan {
            workflows: vec![wf(
                "wf-1",
                vec!["s"],
                vec![
                    LoopSpec {
                        condition: "已显式给值".into(),
                        over: String::new(),
                        max_iterations: Some(3),
                    },
                    lp("对每个文件", "执行校验"),
                ],
            )],
            summary: String::new(),
            degraded: false,
        };
        auto_repair_plan(&mut plan);
        assert_eq!(plan.workflows[0].loops[0].max_iterations, Some(3));
        // 无遍历语义 → 不兜底
        assert_eq!(plan.workflows[0].loops[1].max_iterations, None);
    }

    #[test]
    fn validate_blocks_empty_and_dup_and_cycle() {
        // 空 workflows
        let empty = WorkFlowPlan {
            workflows: vec![],
            summary: String::new(),
            degraded: false,
        };
        assert!(validate_plan_blocking(&empty)
            .iter()
            .any(|i| i.contains("为空")));

        // id 重复 + steps 空
        let mut bad = WorkFlowPlan {
            workflows: vec![wf("wf-1", vec![], vec![]), wf("wf-1", vec!["s"], vec![])],
            summary: String::new(),
            degraded: false,
        };
        let issues = validate_plan_blocking(&bad);
        assert!(issues.iter().any(|i| i.contains("重复")), "{issues:?}");
        assert!(issues.iter().any(|i| i.contains("steps")), "{issues:?}");

        // 循环依赖
        bad.workflows = vec![wf("wf-1", vec!["s"], vec![]), wf("wf-2", vec!["s"], vec![])];
        bad.workflows[0].depends_on = vec!["wf-2".into()];
        bad.workflows[1].depends_on = vec!["wf-1".into()];
        let issues = validate_plan_blocking(&bad);
        assert!(issues.iter().any(|i| i.contains("拓扑")), "{issues:?}");

        // 合法计划通过
        let ok = WorkFlowPlan {
            workflows: vec![wf("wf-1", vec!["s"], vec![])],
            summary: String::new(),
            degraded: false,
        };
        assert!(validate_plan_blocking(&ok).is_empty());
    }
}
