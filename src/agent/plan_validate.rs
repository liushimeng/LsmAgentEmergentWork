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
//!   为空 / depends_on 未知或成环(复用 `topo_layers`)/ **「澄清/询问用户」伪单元**
//!   (第 128 轮:DAG 单元无法暂停等待用户,见 [`CLARIFY_UNIT_MARKERS`])。返回空 Vec = 通过。
//!
//! Orchestrator `run_medium` 流程:parse → auto_repair → validate;
//! 校验失败直接 QualityFailure(精确原因、秒级重试,不烧 LLM QC);
//! 校验通过跳过 LLM QC-main(对齐 degraded 路径,真实产物由每单元 QC 把守)。

use crate::agent::main_work::{topo_layers, WorkFlowPlan, WorkFlowSpec};

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

/// 「澄清/询问用户」类伪单元的文本标记(第 128 轮)。
///
/// 这类单元在 DAG 里**结构上不可能完成**:WorkFlow 单元运行在 Kahn 分层调度中,
/// 没有暂停等待用户输入的语义;唯一能触达用户的 `HumanAssistHub` 只被
/// `MCP_Web_Use control_action=request_human` 使用,reason 枚举是验证码/短信/扫码一类
/// **页面阻断**,不含「目标不明确」。
///
/// 于是执行层只能用 `Bash echo "澄清问询已发出"` + `Write` 落盘假装提问,QC 按
/// 「本单元职责 = 发出澄清」判 ✅ 通过,`wf-2 depends_on wf-1` 的依赖门形同虚设,
/// 下游单元在没有目标的情况下继续乱跑 —— 实测事故(`llaew_20260924_151442.log`)
/// 的 wf-1 正是这个形态。
const CLARIFY_UNIT_MARKERS: &[&str] = &[
    "澄清",
    "询问用户",
    "向用户确认",
    "等待用户",
    "需用户确认",
    "请用户提供",
    "让用户选择",
    "ask the user",
    "ask user",
    "clarify with",
];

/// 单元是否是「澄清/询问用户」伪单元(纯函数,可单测)。
///
/// 扫 name + steps + acceptance 三处 —— 实测 LLM 会把澄清意图写在任意一处。
fn is_clarification_unit(wf: &WorkFlowSpec) -> bool {
    let mut haystack = wf.name.to_lowercase();
    for s in &wf.steps {
        haystack.push(' ');
        haystack.push_str(&s.to_lowercase());
    }
    for a in &wf.acceptance {
        haystack.push(' ');
        haystack.push_str(&a.to_lowercase());
    }
    CLARIFY_UNIT_MARKERS
        .iter()
        .any(|m| haystack.contains(&m.to_lowercase()))
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
        // 第 128 轮:澄清伪单元阻断(见 CLARIFY_UNIT_MARKERS 文档注释)。
        // 判为阻断问题 → 走既有「秒级回流重拆」路径,retry_hint 会带上明确禁止语;
        // 目标确实不可解析时,编排器的 L1 澄清门会先于拆解回到用户,不会走到这里。
        if is_clarification_unit(wf) {
            issues.push(format!(
                "workflow {}({})是「澄清/询问用户」伪单元:WorkFlow 单元在 DAG 里无法暂停\
                 等待用户输入,只会让执行层用 Bash echo 假装提问、QC 判通过、下游单元在没有\
                 目标的情况下继续乱跑。信息不足时应输出 0 个 workflows 并在 summary 写明\
                 「需用户澄清:<缺什么>」,由编排器回到用户",
                wf.id,
                wf.name
            ));
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
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None, pre_explore: false,
        }
    }

    fn lp(condition: &str, over: &str) -> LoopSpec {
        LoopSpec {
            condition: condition.into(),
            over: over.into(),
            max_iterations: None,
        }
    }

    // ========== 第 128 轮:澄清伪单元阻断 ==========

    fn wf_named(id: &str, name: &str, steps: Vec<&str>, acceptance: Vec<&str>) -> WorkFlowSpec {
        let mut spec = wf(id, steps, vec![]);
        spec.name = name.to_string();
        spec.acceptance = acceptance.into_iter().map(String::from).collect();
        spec
    }

    fn plan_of(specs: Vec<WorkFlowSpec>) -> WorkFlowPlan {
        WorkFlowPlan {
            workflows: specs,
            summary: String::new(),
            degraded: false,
        }
    }

    #[test]
    fn clarify_unit_in_name_is_blocked() {
        // 实测事故的 wf-1 原样:name 里带「澄清」
        let plan = plan_of(vec![wf_named(
            "wf-1",
            "澄清目标网站(用户必须先指明 URL 或站点名)",
            vec!["向用户发出澄清问询"],
            vec!["澄清问题含关键词"],
        )]);
        let issues = validate_plan_blocking(&plan);
        assert!(
            issues.iter().any(|i| i.contains("伪单元")),
            "应阻断澄清单元: {issues:?}"
        );
    }

    #[test]
    fn clarify_unit_in_steps_is_blocked() {
        // 意图藏在 steps 里也要抓到
        let plan = plan_of(vec![wf_named(
            "wf-1",
            "前置准备",
            vec!["等待用户回复目标 URL 后再继续"],
            vec!["ok"],
        )]);
        assert!(validate_plan_blocking(&plan).iter().any(|i| i.contains("伪单元")));
    }

    #[test]
    fn clarify_unit_in_acceptance_is_blocked() {
        let plan = plan_of(vec![wf_named(
            "wf-2",
            "打开网站",
            vec!["MCP_Web_Use(action=open)"],
            vec!["需用户确认站点后才算完成"],
        )]);
        assert!(validate_plan_blocking(&plan).iter().any(|i| i.contains("伪单元")));
    }

    #[test]
    fn clarify_marker_set_covers_common_phrasings() {
        for name in [
            "询问用户目标",
            "向用户确认网站",
            "请用户提供 URL",
            "让用户选择站点",
            "ask the user for the target",
        ] {
            let plan = plan_of(vec![wf_named("wf-1", name, vec!["s"], vec!["ok"])]);
            assert!(
                validate_plan_blocking(&plan).iter().any(|i| i.contains("伪单元")),
                "应命中: {name}"
            );
        }
    }

    #[test]
    fn normal_web_units_are_not_false_flagged() {
        // 正常单元(含事故里的 wf-2/3/4 形态)不得被误判
        let plan = plan_of(vec![
            wf_named(
                "wf-2",
                "打开目标网站 + explore 批量观察首页结构",
                vec!["MCP_Web_Use(action=open, url=https://www.anthropic.com/)"],
                vec!["final_url 主域 = anthropic.com"],
            ),
            wf_named(
                "wf-3",
                "提取按时间排序的最新 3 篇文章",
                vec!["MCP_Web_Use(action=control, control_action=eval_js)"],
                vec!["返回数组 length >= 3"],
            ),
            wf_named(
                "wf-4",
                "汇总并展示 3 篇文章信息",
                vec!["整理标题+时间+摘要+链接"],
                vec!["段数=3"],
            ),
        ]);
        let issues = validate_plan_blocking(&plan);
        assert!(
            !issues.iter().any(|i| i.contains("伪单元")),
            "正常单元不得误判: {issues:?}"
        );
        assert!(issues.is_empty(), "该计划应完全通过: {issues:?}");
    }

    #[test]
    fn clarify_block_message_tells_llm_what_to_do_instead() {
        let plan = plan_of(vec![wf_named("wf-1", "澄清目标", vec!["问用户"], vec!["ok"])]);
        let msg = validate_plan_blocking(&plan)
            .into_iter()
            .find(|i| i.contains("伪单元"))
            .expect("应有阻断信息");
        // 必须给出替代做法,否则重拆仍会重蹈覆辙
        assert!(msg.contains("0 个 workflows"), "{msg}");
        assert!(msg.contains("需用户澄清"), "{msg}");
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
