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
    #[serde(
        default = "default_delegate_to",
        deserialize_with = "lenient_delegate_to"
    )]
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
        "windowuse" | "windowuseagent" | "window" | "窗口" => AgentRole::WindowUse,
        "webuse" | "webuseagent" | "chromium" | "chromiumwebuse" | "browser" | "web" | "浏览器"
        | "网页" => AgentRole::WebUse,
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

// ========== delegate_to 推断(2026-09-16 第 54 轮补丁 B) ==========
//
// 2026-09-16 第 54 轮:实测「打开微信发消息」任务 Main-Work 把含 osascript 步骤的工作流
// delegate_to=windowuse,但 WindowUse 没有 Bash 工具,导致 SubAgent 循环无法执行命令 →
// 任务空转失败。修复:解析 WorkFlowPlan 后,基于步骤文本 + branches 文本 + 验收文本
// 的关键词匹配,自动纠正 delegate_to。
//   - 命中 shell 命令关键词(osascript / screencapture / cliclick / pbcopy / System Events /
//     坐标点击 / keystroke 等) → SubAgent(WindowUse 没有 Bash 工具,即便有也是白名单)
//   - 命中 GUI 控件关键词(WindowList / WindowInspect / WindowAction / 点击按钮 /
//     控件树 / 枚举窗口) → WindowUse
//   - 都命中 / 都没命中 → 保持 Main-Work 显式选择(不强行覆盖)
// 纯字符串匹配,无新依赖;纠正日志写到 tracing::info!,QC 报告可观察到。

const SUBAGENT_KEYWORDS: &[&str] = &[
    "osascript",
    "applescript",
    "screencapture",
    "cliclick",
    "pbcopy",
    "pbpaste",
    "system events",
    "keystroke",
    "坐标点击",
    "剪贴板",
    "sendinput",
    "uiautomation",
    "powershell",
    "xdotool",
    "wmctrl",
    "adb shell",
    "sendevent",
    "input keyevent",
    "input tap",
    "adb ",
    "shell",
];

const WINDOW_USE_KEYWORDS: &[&str] = &[
    "windowlist",
    "windowinspect",
    "windowaction",
    "控件树",
    "枚举窗口",
    "ui automation",
    "axpress",
    "set_text",
    "控件路径",
    "无障碍",
    "ui automation",
    "invoke pattern",
];

/// 2026-09-16 第 61 轮:浏览器/网页操控关键词(Chromium-WebUse,第 11 角色)。
const WEB_USE_KEYWORDS: &[&str] = &[
    "browsernew",
    "browsercontrol",
    "browserinspect",
    "browserlist",
    "browserclose",
    "浏览器",
    "网页",
    "网站",
    "网址",
    "爬虫",
    "抓取页面",
    "采集页面",
    "chrome",
    "chromium",
    "cdp",
    "devtools",
    "webdriver",
    "headless",
    "无头浏览器",
    "dom",
    "localstorage",
    "页面截图",
    "表单提交",
    "http://",
    "https://",
];

fn text_contains_any_ci(text: &str, keywords: &[&str]) -> bool {
    let lower = text.to_lowercase();
    keywords.iter().any(|k| lower.contains(k))
}

fn gather_spec_text(spec: &WorkFlowSpec) -> String {
    let mut s = String::new();
    s.push_str(&spec.name);
    s.push('\n');
    for st in &spec.steps {
        s.push_str(st);
        s.push('\n');
    }
    for b in &spec.branches {
        s.push_str(&b.condition);
        s.push('\n');
        s.push_str(&b.then);
        s.push('\n');
    }
    for l in &spec.loops {
        s.push_str(&l.condition);
        s.push('\n');
        s.push_str(&l.over);
        s.push('\n');
    }
    for a in &spec.acceptance {
        s.push_str(a);
        s.push('\n');
    }
    s
}

/// 基于步骤文本推断 delegate_to(返回 None 表示不强行纠正,保留原值)。
pub fn infer_delegate_to(spec: &WorkFlowSpec) -> Option<AgentRole> {
    let text = gather_spec_text(spec);
    let shell_hit = text_contains_any_ci(&text, SUBAGENT_KEYWORDS);
    let gui_hit = text_contains_any_ci(&text, WINDOW_USE_KEYWORDS);
    let web_hit = text_contains_any_ci(&text, WEB_USE_KEYWORDS);
    // 2026-09-16 第 61 轮:网页/浏览器操控 → WebUse(shell 仍最优先,WebUse 无 Bash 工具;
    // web+gui 同命中按网页处理,网页语境也有"控件"表述)。
    if web_hit && !shell_hit {
        return Some(AgentRole::WebUse);
    }
    match (shell_hit, gui_hit) {
        // shell 命令占主导 → 必须 SubAgent(WindowUse 没有 Bash / 只有白名单)
        (true, false) => Some(AgentRole::SubAgent),
        // GUI 控件占主导 → WindowUse
        (false, true) => Some(AgentRole::WindowUse),
        // 都命中 → shell 优先(更通用的工具集,且 WindowUse 已扩 Bash 白名单兜底)
        (true, true) => Some(AgentRole::SubAgent),
        // 都没命中 → 保持原样
        (false, false) => None,
    }
}

/// 对整份 plan 做 delegate_to 推断 + 纠正 + 日志。
pub fn infer_delegate_to_for_plan(plan: &mut WorkFlowPlan) {
    for wf in plan.workflows.iter_mut() {
        if let Some(inferred) = infer_delegate_to(wf) {
            if inferred != wf.delegate_to {
                tracing::info!(
                    wf_id = %wf.id,
                    from = %wf.delegate_to.as_str(),
                    to = %inferred.as_str(),
                    "[2026-09-16 B] delegate_to 已自动纠正(基于步骤关键词推断)"
                );
                wf.delegate_to = inferred;
            }
        }
    }
}

#[cfg(test)]
mod infer_tests {
    use super::*;

    #[test]
    fn shell_steps_route_to_subagent() {
        let spec = WorkFlowSpec {
            id: "wf-1".into(),
            name: "启动微信".into(),
            steps: vec!["执行 osascript -e 'tell application \"WeChat\" to activate'".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::WindowUse, // 显式选错
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }

    #[test]
    fn gui_steps_route_to_windowuse() {
        let spec = WorkFlowSpec {
            id: "wf-2".into(),
            name: "枚举窗口".into(),
            steps: vec![
                "调用 WindowList 找目标窗口".into(),
                "调用 WindowInspect 遍历控件树".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec!["找到目标窗口 id".into()],
            delegate_to: AgentRole::SubAgent,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WindowUse));
    }

    #[test]
    fn no_keyword_keeps_explicit_choice() {
        let spec = WorkFlowSpec {
            id: "wf-3".into(),
            name: "通用操作".into(),
            steps: vec!["读取项目根目录结构".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
        };
        assert_eq!(infer_delegate_to(&spec), None);
    }

    #[test]
    fn same_wechat_window_chain_is_coalesced() {
        fn wf(id: &str, step: &str, dep: &str) -> WorkFlowSpec {
            WorkFlowSpec {
                id: id.into(),
                name: format!("微信-{step}"),
                steps: vec![step.into()],
                branches: vec![],
                loops: vec![],
                depends_on: if dep.is_empty() {
                    vec![]
                } else {
                    vec![dep.into()]
                },
                acceptance: vec![format!("完成{step}")],
                delegate_to: AgentRole::WindowUse,
            }
        }
        let mut plan = WorkFlowPlan {
            workflows: vec![
                wf("wf-1", "打开微信", ""),
                wf("wf-2", "搜索赵玲玲", "wf-1"),
                wf("wf-3", "发送消息", "wf-2"),
            ],
            summary: String::new(),
            degraded: false,
        };
        coalesce_wechat_window_workflows(&mut plan);
        assert_eq!(plan.workflows.len(), 1);
        assert_eq!(plan.workflows[0].steps.len(), 3);
        assert!(plan.workflows[0].acceptance.len() >= 3);
    }

    #[test]
    fn both_keywords_pick_subagent() {
        // 混合型:osascript + 控件关键词 → 优先 SubAgent(通用工具集更稳妥)
        let spec = WorkFlowSpec {
            id: "wf-mix".into(),
            name: "混合".into(),
            steps: vec![
                "osascript -e 'tell application \"WeChat\" to activate'".into(),
                "WindowInspect 检视".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }
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
                "windowuse" => {
                    "编排时所有涉及桌面软件窗口操作的流程,请将 delegate_to 设为 \"windowuse\"。"
                }
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
             - delegate_to 三选一:默认填 \"subagent\"(通用执行);若该流程是「读取/操作桌面软件窗口\n\
               (枚举窗口、遍历控件、点击按钮、向窗口输入/读取文本)」类任务,必须填 \"windowuse\",\n\
               由 WindowUse Agent(LsmAgentEmergentWork-WindowUse)执行;\n\
               若该流程是「网页/浏览器操作(打开网址、浏览网页、网页登录、点击/输入/滚动页面、\n\
               网页截图、抓取页面信息、爬虫采集、查看 Console/Network/DOM)」类任务,必须填 \"webuse\",\n\
               由 Chromium-WebUse Agent(LsmAgentEmergentWork-Chromium-WebUse)执行。\n\
             - 同一桌面应用的连续 UI 操作链(打开/激活 → 等待窗口 → 搜索 → 选择会话 → 输入 → 校验 →\n\
               发送/提交)必须合并为一个 windowuse WorkFlow,不要按每个按钮拆成多个串行单元;\n\
               跨单元会丢失真实焦点与控件状态。只有不同应用或互不依赖的窗口操作才允许拆分。\n\
             - acceptance 必须是可执行验证的验收标准(命令 / 可比对的预期输出),不要写「完成目标」这类空话。\n\
             - acceptance 中涉及文本长度验证时,使用字符计数(wc -m / ${#var})而非字节计数(length($0) / wc -c),\n\
               避免中文 UTF-8(每字 3 字节)导致计数偏差。",
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
                Some("windowuse") => AgentRole::WindowUse,
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
        // 2026-09-16 第 59 轮:如果 Yolo 明确建议 windowuse 且 plan 未指定,强制覆盖
        if let Some("windowuse") = suggested_delegate {
            for wf in plan.workflows.iter_mut() {
                if wf.delegate_to == AgentRole::SubAgent {
                    // 仅当 WorkFlow 含窗口操作类步骤时才覆盖(避免误伤纯代码流程)
                    let text = gather_spec_text(wf);
                    let has_window_ops = text_contains_any_ci(
                        &text,
                        &[
                            "微信", "wechat", "qq", "窗口", "window", "点击", "click", "打开",
                            "open", "发送", "send", "应用", "app", "软件",
                        ],
                    );
                    if has_window_ops {
                        wf.delegate_to = AgentRole::WindowUse;
                        tracing::info!(wf_id = %wf.id, "suggested_delegate=windowuse,已覆盖 WorkFlow delegate_to");
                    }
                }
            }
        }
        if let Some("windowuse") = suggested_delegate {
            coalesce_wechat_window_workflows(&mut plan);
        }

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

/// 2026-09-16 第 61 轮:把同一微信 UI 链的多个 WindowUse 单元自动合并。
///
/// 实测 Main-Work 常把「打开微信 → 搜索联系人 → 打开会话 → 输入 → 发送」拆成
/// 5 个串行单元。每个单元都有独立 Agent 上下文,真实焦点 / 搜索框状态无法传递。
/// 这里在 Yolo 已判定 windowuse 时兜底合并,保留真实窗口连续性。
fn coalesce_wechat_window_workflows(plan: &mut WorkFlowPlan) {
    let same_wechat = |w: &WorkFlowSpec| {
        w.delegate_to == AgentRole::WindowUse && {
            let text = gather_spec_text(w).to_lowercase();
            text.contains("微信") || text.contains("wechat")
        }
    };
    let Some(first_idx) = plan.workflows.iter().position(same_wechat) else {
        return;
    };
    let group_ids: Vec<String> = plan
        .workflows
        .iter()
        .filter(|w| same_wechat(w))
        .map(|w| w.id.clone())
        .collect();
    if group_ids.len() < 2 {
        return;
    }

    let mut branches = Vec::new();
    let mut loops = Vec::new();
    let mut steps = Vec::new();
    let mut acceptance = Vec::new();
    for w in plan.workflows.iter().filter(|w| same_wechat(w)) {
        branches.extend(w.branches.clone());
        loops.extend(w.loops.clone());
        steps.extend(w.steps.clone());
        acceptance.extend(w.acceptance.clone());
    }
    let target_id = plan.workflows[first_idx].id.clone();
    {
        let first = &mut plan.workflows[first_idx];
        first.name = "微信连续操控(自动合并同应用UI链)".into();
        first.branches = branches;
        first.loops = loops;
        first.steps = steps;
        first.acceptance = acceptance;
        first
            .depends_on
            .retain(|dep| !group_ids.iter().any(|id| id == dep));
    }
    plan.workflows
        .retain(|w| !same_wechat(w) || w.id == target_id);
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
        "同一微信应用 WindowUse 链已自动合并,保留真实窗口焦点连续性"
    );
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
    let mut in_degree: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
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
        // (解决 osascript 步骤被错委派到 WindowUse 的问题)
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

/// I4(2026-09-14 第 51 轮):workflows 数组按 id 去重(保留首次出现)。
/// 真实网关实测 Main-Work 偶发把同一批 wf 输出两遍(wf-1..wf-5 各 2 条共 10 条,
/// ak05),id 是依赖解析主键,重复触发 QC 必拒 + 依赖歧义;去重属无损修复。
pub fn dedup_workflow_ids(plan: &mut WorkFlowPlan) {
    let mut seen = std::collections::HashSet::new();
    let before = plan.workflows.len();
    plan.workflows.retain(|w| seen.insert(w.id.clone()));
    if plan.workflows.len() != before {
        tracing::warn!(
            "WorkFlow 数组存在重复 id,已按 id 去重: {} → {} 条",
            before,
            plan.workflows.len()
        );
    }
}

/// 归一化单条 `depends_on` 条目,提取其中引用的 wf id。
///
/// LLM 生成的依赖声明常附带「变量透传」注释,例如:
///   - `wf-1（需要 FIRST_CHROME_WINDOW_ID）`(Plan 文档路径 / JSON 路径均有)
///   - `wf-2 (needs data)` / `wf-3：控件树数据`
/// 这些注释让 `topo_layers` 的精确匹配失败,报「依赖未知 wf=wf-1（需要...）」,整份计划
/// 直接被丢弃,上游 Yolo 反复重试空转。修复:剥离注释,只保留开头的 wf id 记号。
/// 返回 None 表示条目为空或剥离后无有效 id(调用方应丢弃该条目)。
fn normalize_dep_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 取第一个「中文全角括号 / 半角括号 / 中文冒号 / 空白」之前的 token 作为 id;
    // 同时去掉首尾可能残留的引号 / 方括号。
    let end = trimmed
        .find(|c: char| matches!(c, '（' | '(' | '：' | ':' | ' ' | '\t'))
        .unwrap_or(trimmed.len());
    let token =
        trimmed[..end].trim_matches(|c: char| matches!(c, '"' | '\'' | '[' | ']' | ',' | '、'));
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

/// 归一化整份计划的 `depends_on` 列表:剥离注释、去空、去自环、去重。
/// 在两个解析入口(Main-Work JSON / Plan 文档)都调用,确保 `topo_layers` 拿到干净的 id 集合。
pub fn sanitize_depends_on(plan: &mut WorkFlowPlan) {
    let ids: std::collections::HashSet<String> =
        plan.workflows.iter().map(|w| w.id.clone()).collect();
    let mut changed = false;
    for w in plan.workflows.iter_mut() {
        let original = w.depends_on.clone();
        let mut cleaned: Vec<String> = original
            .iter()
            .filter_map(|d| normalize_dep_id(d))
            .filter(|d| {
                if d == &w.id {
                    // 自环无意义,丢弃
                    return false;
                }
                if !ids.contains(d) {
                    // 注释剥离后仍不是已知 wf id → 保留但 topo_layers 会报错;
                    // 这里仅做归一化,不在解析层吞错,让拓扑层给出精准提示
                    return true;
                }
                true
            })
            .collect();
        cleaned.dedup();
        if cleaned != original {
            changed = true;
            w.depends_on = cleaned;
        }
    }
    if changed {
        tracing::info!("[2026-09-16 F2] depends_on 已归一化(剥离变量透传注释 / 去自环 / 去重)");
    }
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
    // (修复「检视 Chrome 窗口」hard 任务拓扑失败 + WindowUse 错委派为 subagent 的问题)
    sanitize_depends_on(&mut plan);
    infer_delegate_to_for_plan(&mut plan);
    Ok(plan)
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
    fn normalize_dep_id_strips_annotations() {
        // 全角括号注释(Plan 文档路径典型形态)
        assert_eq!(
            normalize_dep_id("wf-1（需要 FIRST_CHROME_WINDOW_ID）"),
            Some("wf-1".to_string())
        );
        // 半角括号 + 中文说明
        assert_eq!(
            normalize_dep_id("wf-2 (needs data)"),
            Some("wf-2".to_string())
        );
        // 中文冒号
        assert_eq!(
            normalize_dep_id("wf-3：控件树数据"),
            Some("wf-3".to_string())
        );
        // 干净 id
        assert_eq!(normalize_dep_id("wf-1"), Some("wf-1".to_string()));
        // 带引号 / 空格 / 逗号
        assert_eq!(normalize_dep_id("  \"wf-4\" "), Some("wf-4".to_string()));
        assert_eq!(normalize_dep_id("wf-5, wf-6"), Some("wf-5".to_string()));
        // 纯注释 / 空串
        assert_eq!(normalize_dep_id("（需要 X）"), None);
        assert_eq!(normalize_dep_id("   "), None);
        assert_eq!(normalize_dep_id(""), None);
    }

    #[test]
    fn sanitize_depends_on_fixes_unknown_dependency() {
        // 复现 DebugReport_20260916_102929 的失败场景:wf-2 依赖 "wf-1(需要变量)"
        // 未归一化时 topo_layers 报「依赖未知 wf=wf-1(需要 FIRST_CHROME_WINDOW_ID)」
        let mut plan = WorkFlowPlan {
            workflows: vec![
                wf("wf-1", &[]),
                wf("wf-2", &["wf-1（需要 FIRST_CHROME_WINDOW_ID）"]),
                wf("wf-3", &["wf-2（需要控件树数据）", "wf-1"]),
            ],
            summary: "test".into(),
            degraded: false,
        };
        sanitize_depends_on(&mut plan);
        assert_eq!(plan.workflows[1].depends_on, vec!["wf-1".to_string()]);
        assert_eq!(
            plan.workflows[2].depends_on,
            vec!["wf-2".to_string(), "wf-1".to_string()]
        );
        // 归一化后 topo_layers 应成功分层(串行链 3 层)
        let order = topo_sort(&plan.workflows).unwrap();
        let ids: Vec<&str> = order.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["wf-1", "wf-2", "wf-3"]);
    }

    #[test]
    fn sanitize_depends_on_removes_self_loop_and_dedup() {
        let mut plan = WorkFlowPlan {
            workflows: vec![wf("wf-1", &["wf-1", "wf-1（注释）", ""])],
            summary: "test".into(),
            degraded: false,
        };
        sanitize_depends_on(&mut plan);
        // 自环剥离 + 去重 + 去空 → 空列表
        assert!(plan.workflows[0].depends_on.is_empty());
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
    fn parse_workflow_plan_sanitizes_modifier_letters_and_nested_quotes() {
        // 2026-09-16 第 65 轮 P0-A:WorkFlow JSON 含 Unicode Modifier Letter 与
        // 中文「」嵌套时,sanitize 后应能正常反序列化,而不是触发 JSON 校验失败。
        // 真实场景:用户输入"赵玲玲AAAA",LLM 在多
        // 个 JSON 字段里复制该字符串,反复出现嵌套「」和乱码。
        // 注:Rust 原生字符 \u{1d2c}/\u{1d35} 在非 raw string 中正常,但因测试用
        // raw string literal 嵌套 JSON 转义易踩坑,这里用普通字符串拼接含 Unicode
        // 字符的 JSON,避免双重 escape 陷阱。
        let modifier_a = '\u{1d2c}';
        let modifier_i = '\u{1d35}';
        let weird = format!("{modifier_a}{modifier_a}{modifier_i}{modifier_a}");
        let json = format!(
            r#"```json
{{
  "workflows": [
    {{
      "id": "wf-1",
      "name": "微信自动化",
      "steps": ["查找名为「赵玲玲{weird}」的用户"],
      "acceptance": ["已发送消息"],
      "delegate_to": "windowuse",
      "depends_on": []
    }}
  ],
  "summary": "找到「赵玲玲{weird}」并发送消息"
}}
```"#
        );
        let plan = parse_workflow_plan(&json).expect("sanitize 后应能解析");
        assert_eq!(plan.workflows.len(), 1);
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert_eq!(plan.workflows[0].delegate_to, AgentRole::WindowUse);
        // Modifier Letter 已归一化为 Latin 等价字符:ᴬᴬᴵᴬ → AAIA
        let first_step = &plan.workflows[0].steps[0];
        assert!(first_step.contains("AAIA"), "ᴬᴬᴵᴬ 应归一为 AAIA: {first_step}");
    }

    #[test]
    fn parse_workflow_plan_preserves_smart_quotes_in_json_strings() {
        // 2026-09-16 第 65 轮修订:智能引号 \u{201C}\u{201D} 在 JSON 字符串值
        // 内层是合法的装饰引号,与 JSON 解析器无冲突,无需 sanitize 替换。
        // 本测试验证含智能引号的 JSON 能正常被 parse_workflow_plan 解析。
        let left_smart = '\u{201C}';
        let right_smart = '\u{201D}';
        let raw = format!(
            r#"```json
{{
  "workflows": [
    {{"id":"wf-1","name":"{left_smart}测试{right_smart}","steps":["打开微信","点击通讯录"],"acceptance":["ok"],"delegate_to":"windowuse"}}
  ],
  "summary": "test"
}}
```"#
        );
        let plan = parse_workflow_plan(&raw).expect("sanitize 后应能解析");
        assert_eq!(plan.workflows.len(), 1);
        assert!(
            plan.workflows[0].name.contains(left_smart),
            "智能引号应保留(装饰引号): {0}",
            plan.workflows[0].name
        );
        assert!(plan.workflows[0].name.contains(right_smart));
    }

    #[test]
    fn parse_workflow_plan_handles_cjk_inner_ascii_quotes() {
        // 真实场景:LLM 在中文字符串内层用 ASCII " 包裹用户名(典型错误)。
        // 如 `"steps":["查找名为"张三"的用户"]`,sanitize 后内层 ASCII " 应被
        // 替换为「」,JSON 仍合法。
        let raw = r#"```json
{
  "workflows": [
    {"id":"wf-1","name":"微信任务","steps":["查找名为"张三"的用户并发送消息"],"acceptance":["ok"],"delegate_to":"windowuse"}
  ],
  "summary": "test"
}
```"#;
        let plan = parse_workflow_plan(raw).expect("sanitize 后应能解析");
        assert_eq!(plan.workflows.len(), 1);
        // 修复后的 step 文本应不含裸 ASCII "包围中文
        let first_step = &plan.workflows[0].steps[0];
        assert!(
            !first_step.contains("\"张") && !first_step.contains("三\""),
            "中文字符串内的 ASCII \" 应被替换为「」: {first_step}"
        );
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

    /// I2(2026-09-14 第 51 轮):Plan 提示词漂移兜底 —— 粗体 bullet 变体
    /// `- **wf-N …**`(无 `### WorkFlow N:` 标题)也必须解析出 workflow,
    /// 否则 hard 任务「Plan 文档未解析出任何 WorkFlow」整链失败(fl10 实测)。
    #[test]
    fn parse_plan_markdown_bold_bullet_variant() {
        let md = r#"
# 任务方案摘要

## 二、WorkFlow 拆解
- **wf-1 / 子任务 1 — 编写 fl10_rps.py**:asyncio server + JSON Lines + --dry-run
- **wf-2 / 子任务 2 — py_compile**:断言退出码 = 0
- **wf-3 / 子任务 3 — JSON 配置**:两个配置文件 utf-8 保存

## 三、关键决策
决策
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("parse failed: {e}"));
        assert_eq!(plan.workflows.len(), 3, "粗体变体应解析出 3 个 WorkFlow");
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert!(
            plan.workflows[0].name.contains("编写"),
            "name: {}",
            plan.workflows[0].name
        );
        assert!(
            !plan.workflows[0].steps.is_empty(),
            "说明 bullet 应兜底成为步骤: {:?}",
            plan.workflows[0].steps
        );
    }

    /// I2c(2026-09-14 第 51 轮):表格行变体 `| **wf-N** | 名称 | 步骤 | 依赖 |`
    /// 必须解析出 workflow 及依赖链(et08/et09 实测形态)。
    #[test]
    fn parse_plan_markdown_table_row_variant() {
        let md = r#"
# 任务方案

## 二、WorkFlow 拆解
| 编号 | WorkFlow 名称 | 核心步骤 | 依赖 |
|------|--------------|---------|------|
| **wf-1** | 目录准备 | 创建 tmpPlan/agent-test/ | 无 |
| **wf-2** | 编写脚本 | 实现核心模型 + demo | wf-1 |
| **wf-3** | 验证 | py_compile 断言 | wf-2、wf-1 |

## 三、关键决策
决策
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("parse failed: {e}"));
        assert_eq!(plan.workflows.len(), 3, "表格行应解析出 3 个 WorkFlow");
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert_eq!(plan.workflows[1].depends_on, vec!["wf-1"]);
        assert_eq!(
            plan.workflows[2].depends_on,
            vec!["wf-2", "wf-1"],
            "顿号分隔依赖: {:?}",
            plan.workflows[2].depends_on
        );
        assert_eq!(plan.workflows[1].steps, vec!["实现核心模型 + demo"]);
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
        for alias in [
            "\"SubAgent-Work\"",
            "\"subagent\"",
            "\"work\"",
            "\"MAIN-WORK\"",
        ] {
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

    /// WindowUse 委派(第 9 角色):windowuse 各别名归一到 AgentRole::WindowUse。
    #[test]
    fn parse_delegate_to_window_use_aliases() {
        let mk = |v: &str| {
            format!(r#"{{"workflows": [{{"id": "wf-1", "name": "n", "delegate_to": {v}}}]}}"#)
        };
        for alias in [
            "\"windowuse\"",
            "\"WindowUse\"",
            "\"window-use\"",
            "\"window_use\"",
            "\"WindowUseAgent\"",
            "\"窗口\"",
        ] {
            let plan: WorkFlowPlan = serde_json::from_str(&mk(alias)).unwrap();
            assert_eq!(
                plan.workflows[0].delegate_to,
                AgentRole::WindowUse,
                "alias={alias}"
            );
        }
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
#[cfg(test)]
mod dedup_tests {
    use super::*;

    /// I4(2026-09-14 第 51 轮):重复 id 去重(ak05 实测 wf-1..wf-5 各出现 2 次)。
    #[test]
    fn dedup_workflow_ids_keeps_first_occurrence() {
        let src = r#"{"workflows":[
            {"id":"wf-1","name":"a","steps":["s1"],"acceptance":["x"]},
            {"id":"wf-2","name":"b","steps":["s2"],"acceptance":["y"]},
            {"id":"wf-1","name":"a2","steps":["dup"],"acceptance":["dup"]}
        ],"summary":"s"}"#;
        let mut plan = parse_workflow_plan(src).unwrap();
        assert_eq!(plan.workflows.len(), 2, "parse_workflow_plan 内部已去重");
        dedup_workflow_ids(&mut plan); // 幂等
        assert_eq!(plan.workflows.len(), 2);
        assert_eq!(plan.workflows[0].name, "a", "应保留首次出现");
        assert_eq!(plan.workflows[1].id, "wf-2");
    }
}
