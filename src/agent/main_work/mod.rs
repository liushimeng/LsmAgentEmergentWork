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


// ===========================================================================
// 模块拆分(2026-09-17,方案 tmpPlan/2026-09-17_01-单文件1800行超标拆分重构方案.md):
// 本文件原 1878 行超过单文件 1800 行上限,按功能域拆为:
//   spec.rs      WorkFlow 规格数据模型 + 宽松反序列化
//   delegate.rs  delegate_to 委派推断(GUI 优先语义)
//   topo.rs      Kahn 分层拓扑 / id 去重 / depends_on 治理
//   parse.rs     JSON / Markdown 双通道方案解析
//   tests.rs     单元测试
// 公有项经下方 `pub use` 再导出,`crate::agent::main_work::Xxx` 路径保持不变。
// ===========================================================================
mod delegate;
mod parse;
mod spec;
mod tests;
mod topo;

pub use delegate::{infer_delegate_to, infer_delegate_to_for_plan};
pub use parse::{parse_plan_markdown, parse_workflow_plan};
pub use spec::{BranchSpec, LoopSpec, WorkFlowPlan, WorkFlowSpec};
pub use topo::{dedup_workflow_ids, sanitize_depends_on, topo_layers, topo_sort};
// 原私有项:供本模块 impl 与测试经 `use super::*` 取用(可见域与拆分前等价)
use delegate::{
    gather_spec_text, text_contains_any_ci, DESKTOP_GUI_STRICT_KEYWORDS, WEB_USE_KEYWORDS,
};
use spec::split_condition_then;
use topo::normalize_dep_id;

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
        Self::plan_workflows_inner(
            self,
            goal,
            decomposition,
            session_id,
            retry_hint,
            original_prompt,
            None,
        )
        .await
    }

    /// 2026-09-16 第 59 轮:带 suggested_delegate 的重载,供 Orchestrator 传入 Yolo 推断结果。
    pub async fn plan_workflows_with_delegate(
        &self,
        goal: &str,
        decomposition: &[String],
        session_id: &str,
        retry_hint: &str,
        original_prompt: Option<&str>,
        suggested_delegate: Option<&str>,
    ) -> Result<(WorkFlowPlan, Usage)> {
        Self::plan_workflows_inner(
            self,
            goal,
            decomposition,
            session_id,
            retry_hint,
            original_prompt,
            suggested_delegate,
        )
        .await
    }

    async fn plan_workflows_inner(
        &self,
        goal: &str,
        decomposition: &[String],
        session_id: &str,
        retry_hint: &str,
        original_prompt: Option<&str>,
        suggested_delegate: Option<&str>,
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
        // 2026-09-16 第 59 轮:透传 Yolo 推断的 suggested_delegate,引导 Main-Work 正确委派
        if let Some(d) = suggested_delegate {
            let hint = match d {
                "webuse" => {
                    "编排时所有涉及网页/浏览器操作的流程,请将 delegate_to 设为 \"webuse\"。"
                }
                _ => "编排时请按上述建议设置 delegate_to。",
            };
            prompt.push_str(&format!(
                "\n【重要】Yolo 基于用户输入关键词推断该任务应委派给: {d}\n{hint}\n"
            ));
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
             - delegate_to 二选一:默认填 \"subagent\"(通用执行,含「读取/操作桌面软件窗口:\n\
               枚举窗口、遍历控件、点击按钮、向窗口输入/读取文本」类任务 —— 执行层 SubAgent-Work\n\
               在 macOS / Windows 上持有 MCP_Window_Use 工具,可完成桌面窗口操控);\n\
               若该流程是「网页/浏览器操作(打开网址、浏览网页、网页登录、点击/输入/滚动页面、\n\
               网页截图、抓取页面信息、爬虫采集、查看 Console/Network/DOM)」类任务,必须填 \"webuse\",\n\
               由 Chromium-WebUse Agent(LsmAgentEmergentWork-Chromium-WebUse)执行。\n\
             - 同一桌面应用的连续 UI 操作链(打开/激活 → 等待窗口 → 搜索 → 选择会话 → 输入 → 校验 →\n\
               发送/提交)必须合并为一个 subagent WorkFlow,不要按每个按钮拆成多个串行单元;\n\
               跨单元会丢失真实焦点与控件状态。只有不同应用或互不依赖的窗口操作才允许拆分。\n\
             - acceptance 必须是可执行验证的验收标准(命令 / 可比对的预期输出),不要写「完成目标」这类空话。\n\
             - acceptance 中涉及文本长度验证时,使用字符计数(wc -m / ${#var})而非字节计数(length($0) / wc -c),\n\
               避免中文 UTF-8(每字 3 字节)导致计数偏差。\n\
             - 桌面窗口操控类流程,acceptance 用 **UI 结果验证**(窗口内出现的目标文本/\n\
               控件状态/OCR 可见性),不要写「进程存在/窗口标题/bash 检查」类验收 ——\n\
               桌面应用的进程名常与产品名不一致(如微信 4.x 是 Weixin.exe 而非 WeChat.exe),\n\
               bash 进程检查极易误判「应用未安装」。\n\
             - 同一应用的连续 UI 操作链(打开/激活 → 检视/OCR → 点击 → 输入 → 发送 → 复查)必须\n\
               合并为一个 subagent WorkFlow;微信 4.x 等自绘 UI 的控件树为空,MCP_Window_Use\n\
               支持 action=ocr + click_point 视觉路线,编排时正常按步骤描述即可,不需要拆成 Bash 检查单元。",
        );

        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;
        let mut plan = parse_workflow_plan(&text).unwrap_or_else(|e| {
            tracing::warn!("Main-Work 解析失败,使用单 WorkFlow 兜底: {}", e);
            // F2:兜底 acceptance 继承 Yolo 分解步骤(可验证清单),不再退化「完成目标」;
            // degraded=true 供 run_medium 跳过 QC-main,消除必败重试循环。
            let inherited = if decomposition.is_empty() {
                vec!["完成目标".to_string()]
            } else {
                decomposition.to_vec()
            };
            // 2026-09-16 第 59 轮:兜底 WorkFlow 也继承 suggested_delegate
            let fallback_delegate = match suggested_delegate {
                Some("webuse") => AgentRole::WebUse,
                _ => AgentRole::SubAgent,
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
                    delegate_to: fallback_delegate,
                }],
                summary: "Main-Work JSON 解析失败,已使用单 WorkFlow 兜底".into(),
                degraded: true,
            }
        });

        // 2026-09-16 第 61 轮:Yolo 建议 webuse 且 plan 未指定时,强制覆盖
        if let Some("webuse") = suggested_delegate {
            for wf in plan.workflows.iter_mut() {
                if wf.delegate_to == AgentRole::SubAgent {
                    let text = gather_spec_text(wf);
                    if text_contains_any_ci(&text, WEB_USE_KEYWORDS) {
                        wf.delegate_to = AgentRole::WebUse;
                        tracing::info!(wf_id = %wf.id, "suggested_delegate=webuse,已覆盖 WorkFlow delegate_to");
                    }
                }
            }
        }
        // 2026-09-16 第 67 轮:同应用桌面窗口操控链自动合并(保留真实窗口焦点连续性)
        coalesce_same_app_window_workflows(&mut plan);
        // 2026-09-17 第 75 轮:兜底 plan 也跑一次关键词推断。
        // Main-Work JSON 解析失败时构造的单 WorkFlow 兜底不会经过
        // `parse_workflow_plan → infer_delegate_to_for_plan`(那是 JSON 路径),
        // 这里手动调用一次,保证「打开网址 + 输入框」等 web 任务被纠正到 WebUse。
        // 成功路径 `infer_delegate_to_for_plan` 已在 parse.rs 中调用,本调用幂等。
        infer_delegate_to_for_plan(&mut plan);

        let _ = memory::record_entry(
            &self.db,
            AgentRole::MainWork,
            session_id,
            goal,
            &format!("workflows: {}", plan.workflows.len()),
            None,
            serde_json::json!({ "workflow_ids": plan.workflows.iter().map(|w| &w.id).collect::<Vec<_>>() }),
        );

        // 运行日志(2026-09-17 第 69 轮):Main-Work 拆解完成(决策)—— 逐单元
        // id/name/delegate/depends_on,排查「任务被拆成了什么、委派给了谁」。
        let units_desc = plan
            .workflows
            .iter()
            .map(|w| {
                format!(
                    "{}[{}]→{}(deps:{})",
                    w.id,
                    crate::logging::clip_for_log(&w.name, 60),
                    w.delegate_to.as_str(),
                    if w.depends_on.is_empty() {
                        "-".to_string()
                    } else {
                        w.depends_on.join("+")
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(" ; ");
        tracing::info!(
            agent = "LsmAgentEmergentWork-Main-Work",
            session = session_id,
            workflows = plan.workflows.len(),
            degraded = plan.degraded,
            units = %units_desc,
            "Main-Work 拆解完成(决策)"
        );

        Ok((plan, usage))
    }

    /// 从 Plan 文档中解析 WorkFlow 段(Plan Agent 的 Markdown 输出)。
    pub fn parse_plan(&self, plan_path: &std::path::Path) -> Result<WorkFlowPlan> {
        let content = std::fs::read_to_string(plan_path).map_err(|e| {
            AgentError::PlanGen(format!("无法读取 Plan 文档 {}: {}", plan_path.display(), e))
        })?;
        let plan = parse_plan_markdown(&content)?;
        // 运行日志(2026-09-17 第 69 轮):hard 档 Main-Work 从 Plan 文档解析出的
        // WorkFlow(非 LLM 拆解路径,与 plan_workflows_inner 的拆解事件对偶)。
        tracing::info!(
            agent = "LsmAgentEmergentWork-Main-Work",
            plan_path = %plan_path.display(),
            workflows = plan.workflows.len(),
            "Main-Work 解析 Plan 完成(决策)"
        );
        Ok(plan)
    }
}

/// 2026-09-16 第 61 轮:把同一微信 UI 链的多个桌面窗口操控单元自动合并。
/// 2026-09-16 第 67 轮:泛化为 `coalesce_same_app_window_workflows` —— 按 app 家族
/// (微信/QQ/钉钉/飞书/Telegram)分组,同族 ≥2 个窗口操控单元自动合并。
/// 2026-09-18 第 84 轮:WindowUse Agent 删除,判定条件改为
/// `delegate_to == SubAgent` + 桌面 GUI 强信号(MCP_Window_Use 承担窗口操控)。
///
/// 实测 Main-Work 常把「打开微信 → 搜索联系人 → 打开会话 → 输入 → 发送」拆成
/// 5 个串行单元。每个单元都有独立 Agent 上下文,真实焦点 / 搜索框状态无法传递。
/// 合并保留真实窗口连续性。
fn coalesce_same_app_window_workflows(plan: &mut WorkFlowPlan) {
    /// app 家族关键词(小写匹配)。
    const APP_FAMILIES: &[(&str, &[&str])] = &[
        ("微信", &["微信", "wechat", "weixin"]),
        ("QQ", &["qq"]),
        ("钉钉", &["钉钉", "dingtalk"]),
        ("飞书", &["飞书", "feishu", "lark"]),
        ("Telegram", &["telegram", "tg"]),
    ];
    let family_of = |w: &WorkFlowSpec| -> Option<&'static str> {
        if w.delegate_to != AgentRole::SubAgent {
            return None;
        }
        let text = gather_spec_text(w).to_lowercase();
        // 2026-09-18 第 84 轮:必须同时含桌面 GUI 强信号,避免误并普通 SubAgent 流程
        if !text_contains_any_ci(&text, DESKTOP_GUI_STRICT_KEYWORDS) {
            return None;
        }
        APP_FAMILIES
            .iter()
            .find(|(_, kws)| kws.iter().any(|k| text.contains(k)))
            .map(|(name, _)| *name)
    };
    for (family_name, _) in APP_FAMILIES {
        let same_family = |w: &WorkFlowSpec| family_of(w) == Some(*family_name);
        let Some(first_idx) = plan.workflows.iter().position(same_family) else {
            continue;
        };
        let group_ids: Vec<String> = plan
            .workflows
            .iter()
            .filter(|w| same_family(w))
            .map(|w| w.id.clone())
            .collect();
        if group_ids.len() < 2 {
            continue;
        }

        let mut branches = Vec::new();
        let mut loops = Vec::new();
        let mut steps = Vec::new();
        let mut acceptance = Vec::new();
        for w in plan.workflows.iter().filter(|w| same_family(w)) {
            branches.extend(w.branches.clone());
            loops.extend(w.loops.clone());
            steps.extend(w.steps.clone());
            acceptance.extend(w.acceptance.clone());
        }
        let target_id = plan.workflows[first_idx].id.clone();
        {
            let first = &mut plan.workflows[first_idx];
            first.name = format!("{family_name}连续操控(自动合并同应用UI链)");
            first.branches = branches;
            first.loops = loops;
            first.steps = steps;
            first.acceptance = acceptance;
            first
                .depends_on
                .retain(|dep| !group_ids.iter().any(|id| id == dep));
        }
        plan.workflows
            .retain(|w| !same_family(w) || w.id == target_id);
        for w in &mut plan.workflows {
            for dep in &mut w.depends_on {
                if group_ids.iter().any(|id| *id == *dep) {
                    *dep = target_id.clone();
                }
            }
            w.depends_on.dedup();
        }
        tracing::info!(
            target = %target_id,
            merged = group_ids.len(),
            "同一 {} 应用桌面窗口操控链已自动合并,保留真实窗口焦点连续性", family_name
        );
    }
}
