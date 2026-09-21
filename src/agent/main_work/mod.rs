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
use delegate::{gather_spec_text, text_contains_any_ci, DESKTOP_GUI_STRICT_KEYWORDS};
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
        // 2026-09-19 第91轮 P0-1 改动1: 三条绝对关键约束 + 第一性事实优先 + 输出前自检清单
        // 2026-09-19 第93轮: 第(3)条修正 —— 长等待正确载体是 run_sequence(单步≤30s)/chat_loop,
        // input_batch wait ≤5s 仅限步骤间节奏(实测 SubAgent 按旧指引 input_batch 20×30s wait 第 1 步即败);
        // 新增第(4)条 —— 首响应直接输出 JSON,禁止先探索(实测浪费 36s 一次 LLM 往返)。
        prompt.push_str(&format!("【Main-Work 任务编排 · 第107轮】五条绝对关键约束(1)「目标」是 Yolo 摘要可能丢动词,用户原始输入第一性;(2)acceptance 必须覆盖原始每条编号与关键动词(聊天/发送/保存/截图/打开/查找/启动/键入/枚举/关闭);(3)长时任务用 chat_loop / run_sequence wait(单步≤30s)编排节奏,严禁 Bash sleep 循环;input_batch wait ≤5s 仅限步骤间节奏微调;(4)首次响应直接输出 JSON 编排,禁止先调用 Read/Bash 探索——任务所需信息已全部在本提示中;(5)独立桌面软件目标(含豆包/Doubao)必须锚定 MCP_Window_Use,禁止改写为 MCP_Web_Use/网页任务。\n"));
        if let Some(orig) = original_prompt.filter(|s| !s.trim().is_empty()) {
            prompt.push_str(&format!("第一性事实 · 用户原始输入(必读):\n{orig}\n"));
        }
        prompt.push_str(&format!("Yolo 摘要 · 仅作参考:\n目标: {goal}\n"));
        if !decomposition.is_empty() {
            prompt.push_str("Yolo 分解步骤(可参考不一定照搬):\n");
            for (i, s) in decomposition.iter().enumerate() {
                prompt.push_str(&format!("  {}. {}\n", i + 1, s));
            }
        }
        // 2026-09-16 第 59 轮:透传 Yolo 推断的 suggested_delegate,引导 Main-Work 正确委派
        // (第 89 轮:执行器唯一化为 subagent,提示语相应简化)
        if let Some(d) = suggested_delegate {
            prompt.push_str(&format!(
                "\n【重要】Yolo 基于用户输入关键词推断该任务应委派给: {d}\n编排时请按上述建议设置 delegate_to。\n"
            ));
        }
        if !retry_hint.is_empty() {
            prompt.push_str(&format!(
                "\n【重要】上一轮同目标执行失败,失败原因:\n{retry_hint}\n\
                 请在本轮拆解中针对上述原因调整编排(补充前置检查 / 拆细步骤 / 明确验收命令)。\n"
            ));
        }
        prompt.push_str(&format!("输出前自检清单(必填):原始每条编号是否都映射到某 wf 的 steps+acceptance? 核心动作动词(聊天/发送/保存报告/截图/...)是否完整保留? 时长/数量/频率是否在 acceptance 出现? 长时任务是否走 chat_loop/run_sequence wait 而非 Bash sleep 或 input_batch 长 wait? 交互类任务 acceptance 是否锚定 UI 动作产物而非「保活/进程存活/时间差」?\n"));
        // F8:补全 branches/loops 的 schema 示例并注明可省略 —— 此前提示词只列了
        // 六个必填字段,LLM 自行发明 branches 字符串形态导致类型失配(F1 的源头)。
        prompt.push_str(
            "\n\n【重要:编排范式——请严格按下方结构输出】\n★ 例 1(★最重要★ 自绘 UI 任务:微信/钉钉/飞书/QQ/桌面聊天 等)\n当任务涉及上述自绘 UI 应用时,**禁止**生成「控件结构深度解析」「控件路径映射表」\n\"AX 路径枚举」等 wf 单元(自绘 UI 控件树为空,这些任务不可能完成,浪费迭代预算)。\n**正确模板(2~3 个 wf,不要 6 个)**:\n  wf-1:打开/激活目标应用 + explore 快照(一次拿全:capability + 控件树 + 屏幕坐标);\n  wf-2:执行核心任务(chat_loop 多轮聊天 / run_sequence 批量操作);\n  wf-3(可选):生成报告(读取 chat_log 落盘 Markdown)。\nwf-2 的 acceptance 锚定 chat_log_path 文件中的 [SEND]/[RECV]/[SUMMARY] 行数,\n不锚定控件路径(自绘 UI 无路径可引用)。\n\n例 2(常规 GUI 任务:VSCode / Notion / 浏览器 DevTools):3~4 个 wf,explore 后\nrun_sequence 连续执行 + 验证,详见下方说明。\n\n请严格按以下 JSON 结构输出(可包裹在 \x60\x60\x60json 代码块中):\n\
             {\"workflows\": [{\"id\": \"wf-1\", \"name\": \"流程名\", \"steps\": [\"步骤\"], \
             \"branches\": [\"条件: 动作\"], \"loops\": [\"条件: 遍历对象\"], \"depends_on\": [], \
             \"acceptance\": [\"可验证的验收标准\"], \"delegate_to\": \"subagent\", \"max_iterations\": 24}], \"summary\": \"编排思路\"}\n\
             约束:\n\
             - id/name/steps/acceptance/delegate_to 必填;branches/loops/depends_on/summary 可省略。\n\
             - max_iterations: 可选整数 4~32;长时多轮聊天(>5 分钟或 >10 轮)/ 长时保活 / 含 5+ 步骤的复杂单元建议显式设置(如 24 或 32),避免 SubAgent 跑满 16 次迭代上限半途而废。\n\
             - branches/loops 元素是字符串(形如 \"条件: 动作\")或对象({\"condition\":…,\"then\":…} / {\"condition\":…,\"over\":…})均可。\n\
             - delegate_to 唯一合法值:\"subagent\"(通用执行层 SubAgent-Work)。桌面窗口操控类任务\n\
               (枚举窗口、遍历控件、点击按钮、向窗口输入/读取文本)由 SubAgent-Work 的\n\
               MCP_Window_Use 工具承担(macOS / Windows);网页/浏览器操作类任务(打开网址、\n\
               浏览网页、网页登录、点击/输入/滚动页面、网页截图、抓取页面信息、爬虫采集、\n\
               查看 Console/Network/DOM)由 SubAgent-Work 的 MCP_Web_Use 工具承担\n\
               (CDP 内存无头浏览器,全平台)—— 两类任务照常描述步骤即可,不需要特殊 delegate 值。\n\
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
               支持 action=ocr + click_point 视觉路线,编排时正常按步骤描述即可,不需要拆成 Bash 检查单元。\n\
             - 桌面窗口操控流程引用 MCP_Window_Use 时(2026-09-18 第 87 轮;第 90 轮 +input_batch;\n\
               2026-09-19 第 91 轮 +run_sequence 连续工作模式;**2026-09-21 第 100 轮 +explore\n\
               探索侧 + 双工作模式框架**),steps 中只允许使用以下合法 action:open / list / find /\n\
               inspect / control / ocr / screenshot / capability_probe / osascript_run / chat_send /\n\
               chat_loop / input_batch / run_sequence / **explore**(第 100 轮新增:探索快照,把\n\
               capability_probe + find + inspect + ocr 四步合并为一次调用);禁止臆造\n\
               list_windows / get_window_info / get_ui_tree 等不存在的接口名 —— 执行层按字面调用\n\
               会直接失败空转。\n\
             - **首选 explore + run_sequence 双调用(第 100 轮 · 连续执行模式,人机共用机器\n\
               必备)**:同一桌面应用连续 UI 链(打开 → 检视/OCR → 点击 → 输入 → 发送 → 复查),\n\
               在 subagent WorkFlow 内**两步走完**,禁止拆 capability_probe + find + inspect +\n\
               ocr 四次单步调用 —— 那正是要消除的「Agent 操作太慢与人冲突」源头:\n\
               ① `action=explore(query=..., window_id=..., max_depth=6, snapshot_label=...)` 一次\n\
                  拿全 + 快照落盘 `<工作目录>/laew_ui_snapshot_<ts>.json` + actionable 摘要\n\
                  (含屏幕绝对坐标,可直接喂给后续 click_point);\n\
               ② `action=run_sequence(window_id=..., steps=[...], focus_guard=true,\n\
                  on_error=retry)` 一次连续执行 + 验证/重试/焦点守护 + log 落盘;\n\
               acceptance 锚定 UI 动作产物 + run_sequence 落盘 log_path(默认\n\
               <工作目录>/laew_sequence_<ts>.log)。失败片段 re-explore 后新 run_sequence\n\
               补做,验收看 [STEP]/[RETRY]/[FAIL]/[SUMMARY] 行。**人机共用机器、任务链 ≥\n\
               3 步、用户随时会切窗的场景一律首选此双调用范式**;仅 ≤2 步的纯读取才走单步模式\n\
               (单 inspect / 单 ocr)。\n\
             - **同应用连续 UI 链优先 run_sequence(第 91 轮「连续工作模式」)**:同一桌面应用的多步\n\
               UI 链(打开 → 检视/OCR → 点击会话 → 等列表刷新 → 输入 → 提交 → 断言已发送),\n\
               在 subagent WorkFlow 内优先用 `action=run_sequence(window_id, steps=[...])` 一次\n\
               调用完成 —— 相比 input_batch 多 3 项能力:**assert_text / wait_for_text 验证等待\n\
               (关键节点自检 + 异步 UI 就绪,不再盲 wait)、on_error=retry 逐步自动重试、\n\
               逐步焦点守护 focus_guard(用户切窗时自动重夺 + 连续 3 次失败止损)**;\n\
               并把执行记录落盘 log_path(默认 <工作目录>/laew_sequence_<unix_ts>.log),\n\
               失败后可对账。长批 steps ≤ 100,整批默认 ≤180s,典型范式见\n\
               系统提示词「连续工作模式 run_sequence」段落。\n\
               注意(第 93 轮):assert_text/wait_for_text 在控件树为空(自绘 UI)时自动 OCR 兜底,\n\
               path=\"/\" 即对整窗可见文本断言(微信 4.x 推荐);**纯 wait 保活不是任何操作的完成证据**\n\
               —— 交互类任务 acceptance 必须锚定 UI 动作产物(chat_log [SEND]/OCR 文本/读取值),\n\
               不得仅锚定「进程存活/时间差」。\n\
             - 浏览器操控流程引用 MCP_Web_Use 时(2026-09-18 第 89 轮),steps 中只允许使用\n\
               以下合法 action:open / list / close / control / inspect;control 内层动作用\n\
               control_action(click/input_text/wait/navigate/screenshot/eval_js 等),inspect\n\
               观察维度用 info(console/network/elements/dom/localstorage 等);禁止臆造\n\
               BrowserNew/goto_page/get_dom_tree 等不存在的接口名。\n\
             - 长时多轮桌面会话(如 15 分钟微信自动聊天):steps 必须引导执行层用\n\
               chat_loop(window_id, messages, interval_seconds, chat_log_path) 一次调用完成多轮\n\
               发送 —— SubAgent 迭代上限 16 次,逐条 chat_send 必然超限失败;\n\
               acceptance 引用 chat_log 落盘文件中的 [SEND]/[RECV]/[SUMMARY] 行计数\n\
               (该日志由 MCP_Window_Use 工具内部落盘,执行轨迹可对账)。\n\
             - acceptance 禁止自定义执行层可用 Bash echo / Write 伪造的文本标记\n\
               (如 mcp_action= / probe_msg_sent=OK 这类自造 grep 锚点)—— 实测执行层会\n\
               手写假日志交差;验收锚点只能用工具自产证据(chat_log [SEND] 行)或 UI 状态。\n\
             - **自绘 UI 任务 WorkFlow 简化(第 101 轮)**:当用户任务涉及「微信」「钉钉」\n\
               「飞书」「QQ」「桌面聊天」等自绘 UI 应用时 —— **禁止**生成「控件结构深度解析」\n\
               「控件路径映射表」「AX 路径枚举」等 wf 单元(自绘 UI 控件树为空,这些任务不可能完成,\n\
               浪费迭代预算);正确模板(2~3 个 wf,不要 6 个):\n\
               wf-1:打开/激活目标应用 + explore 快照(一次拿全);\n\
               wf-2:执行核心任务(chat_loop 多轮聊天 / run_sequence 批量操作);\n\
               wf-3(可选):生成报告(读取 chat_log 落盘 Markdown)。\n\
               wf-2 的 acceptance 锚定 chat_log_path 文件中的 [SEND]/[RECV] 行数,\n\
               不锚定控件路径(自绘 UI 无路径可引用)。",
        );

        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;
        // M8 (2026-09-21 第 102 轮):Plan 解析失败时,若原 prompt 含自绘 UI 关键词
        // (微信/钉钉/飞书/QQ),兜底走 2 wf 模板(探索 + chat_loop),避免退化
        // 「单 wf 默认流程」让 LLM 重蹈 inspect 失败循环。
        let self_drawn_keywords = ["微信", "钉钉", "飞书", "QQ", "wechat", "dingtalk",
            "feishu", "lark", "wecom", "桌面聊天", "桌面 IM"];
        let is_self_drawn_task = original_prompt
            .map(|p| self_drawn_keywords.iter()
                .any(|kw| p.to_lowercase().contains(&kw.to_lowercase())))
            .unwrap_or(false)
            || decomposition.iter().any(|d| self_drawn_keywords.iter()
                .any(|kw| d.to_lowercase().contains(&kw.to_lowercase())));

        let mut plan = parse_workflow_plan(&text).unwrap_or_else(|e| {
            tracing::warn!("Main-Work 解析失败,使用兜底 WorkFlow: {}", e);
            // F2:兜底 acceptance 继承 Yolo 分解步骤(可验证清单),不再退化「完成目标」;
            // degraded=true 供 run_medium 跳过 QC-main,消除必败重试循环。
            let inherited = if decomposition.is_empty() {
                vec!["完成目标".to_string()]
            } else {
                decomposition.to_vec()
            };
            // 2026-09-16 第 59 轮:兜底 WorkFlow 也继承 suggested_delegate
            // (第 89 轮:执行器唯一化,兜底与正常路径统一 SubAgent)
            if is_self_drawn_task {
                // M8:自绘 UI 任务兜底 —— 2 wf 模板(探索 + chat_loop),
                // 强制引导 LLM 走正确路径,避免「控件解析」不可能任务
                WorkFlowPlan {
                    workflows: vec![
                        WorkFlowSpec {
                            id: "wf-1".into(),
                            name: "目标应用激活 + 探索快照".into(),
                            steps: vec![
                                "MCP_Window_Use(action=capability_probe) 拿真实能力矩阵".to_string(),
                                "MCP_Window_Use(action=explore, query=微信, snapshot_label=wechat) 一次拿全".to_string(),
                            ],
                            branches: vec![],
                            loops: vec![],
                            depends_on: vec![],
                            acceptance: vec![
                                "explore 返回 snapshot_path 落盘".to_string(),
                                "tree_summary.self_drawn=true 或 actionable_count<5".to_string(),
                            ],
                            delegate_to: AgentRole::SubAgent,
                            max_iterations: Some(8),
                            original_prompt: original_prompt.map(str::to_string),
                        },
                        WorkFlowSpec {
                            id: "wf-2".into(),
                            name: "执行核心任务(自绘 UI 唯一路径)".into(),
                            steps: vec![
                                "MCP_Window_Use(action=chat_loop, window_id=<wf-1.window_id>, messages=[...], interval_seconds=30, chat_log_path=...)".to_string(),
                            ],
                            branches: vec![],
                            loops: vec![],
                            depends_on: vec!["wf-1".to_string()],
                            acceptance: vec![
                                "chat_log_path 落盘含 [SEND] 行 >= 1".to_string(),
                                "[RECV] 行 >= 1(若对方回复)".to_string(),
                                "[SUMMARY] 行存在且 focus_aborted=false".to_string(),
                            ],
                            delegate_to: AgentRole::SubAgent,
                            max_iterations: Some(24),
                            original_prompt: original_prompt.map(str::to_string),
                        },
                    ],
                    summary: "Main-Work 解析失败,自绘 UI 任务 2 wf 兜底模板(M8)".into(),
                    degraded: true,
                }
            } else {
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
                        // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
                        max_iterations: None, original_prompt: None,
                    }],
                    summary: "Main-Work JSON 解析失败,已使用单 WorkFlow 兜底".into(),
                    degraded: true,
                }
            }
        });

        // 2026-09-19 第 91 轮 P0-6/P0-8:per-unit max_iterations 适配 + original_prompt 透传
        // 给每个 WorkFlow,SubAgentRunner 能在 retry 轮真正看到
        // 失败原因 + 整段用户原始 prompt,避免意图丢失
        for wf in plan.workflows.iter_mut() {
            if wf.original_prompt.is_none() {
                wf.original_prompt = original_prompt.map(str::to_string);
            }
            // 长任务推荐 max_iterations=24(较默认 16 提升 50%)
            if wf.max_iterations.is_none() {
                let is_long = wf.steps.iter().any(|s| {
                    s.contains("长") || s.contains("待") || s.contains("每分钟") || s.contains("秒")
                }) || wf.steps.len() >= 5;
                if is_long {
                    wf.max_iterations = Some(24);
                }
            }
        }
        // 2026-09-16 第 67 轮:同应用桌面窗口操控链自动合并(保留真实窗口焦点连续性)
        coalesce_same_app_window_workflows(&mut plan);
        // 2026-09-17 第 75 轮:兜底 plan 也跑一次关键词推断。
        // Main-Work JSON 解析失败时构造的单 WorkFlow 兜底不会经过
        // `parse_workflow_plan → infer_delegate_to_for_plan`(那是 JSON 路径),
        // 这里手动调用一次,保证「打开网址 + 输入框」等浏览器类任务被归一到 SubAgent
        // (第 89 轮:浏览器操控由 SubAgent-Work 的 MCP_Web_Use 工具承担)。
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
