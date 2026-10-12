//! Quality-Check Agent:质检层,对 SubAgent-Work / Main-Work / Plan 单元做质量校验。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::context::AgentRole;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::memory;
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::Result;
use crate::llm::{ChatMessage, Usage};
use crate::session;

/// 第 99 轮:QC 输出 JSON 形状硬性提示(三个 QC 提示词共用;实测 LLM 偶发漏掉
/// verdict 字段导致反序列化失败白耗局部重试预算,提示词 + 解析端修复双保险)。
const QC_JSON_SHAPE_HINT: &str = "\n\n【输出格式硬性要求】只输出一个合法 JSON 对象,第一个字段必须是 verdict(缺 verdict 视为无效输出,会被 fail-closed 拒收):\n{\"verdict\":\"pass\",\"source\":\"subagent\",\"issues\":[\"仅列**阻断性**缺陷;非阻断的观察放 notes\"],\"notes\":[\"非阻断观察(不影响通过判定)\"],\"suggestion\":\"失败时给出可执行修改建议,通过时留空\",\"retryable\":false,\"evidence\":\"关键证据摘录\"}\n【issues 与 notes 的区别(2026-10-12 起强制)】\n- `issues` = **阻断性缺陷**:不修就无法进入下一阶段 / 会让产物错误 / 让任务目标无法达成。issues 非空即判 Fail。\n- `notes` = **非阻断观察**:建议改进、口径提醒、措辞弹性、但产物仍可用的意见。notes 非空**不**影响 verdict。\n- 判 pass 时把观察写进 `notes`(**不要**塞进 issues);判 fail 时 issues 必填 blocking 缺陷,notes 可留空。\n- 判据:「这条问题是否会让上游拿到一份**错误的**产物?」会 → issues;不会 → notes。";

/// 质检结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    Fail,
}

/// 质检报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityReport {
    pub verdict: Verdict,
    pub source: AgentRole,
    /// **阻断性缺陷**列表。非空即意味 Fail(见 [`QualityReport::reconcile_verdict`]）。
    #[serde(default)]
    pub issues: Vec<String>,
    /// **非阻断观察**(2026-10-12 新增):模型对措辞弹性/口径/改进建议的备注。
    /// 不参与 verdict 判定,仅如实留痕供 Debug/人工回看。
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub suggestion: String,
    pub retryable: bool,
    #[serde(default)]
    pub evidence: String,
}

impl QualityReport {
    pub fn pass(source: AgentRole) -> Self {
        Self {
            verdict: Verdict::Pass,
            source,
            issues: Vec::new(),
            notes: Vec::new(),
            suggestion: String::new(),
            retryable: false,
            evidence: String::new(),
        }
    }

    pub fn fail(source: AgentRole, issues: Vec<String>, suggestion: &str, retryable: bool) -> Self {
        Self {
            verdict: Verdict::Fail,
            source,
            issues,
            notes: Vec::new(),
            suggestion: suggestion.into(),
            retryable,
            evidence: String::new(),
        }
    }

    /// 第 163 轮:机械消解「`verdict=pass` 却列了 issues」的自相矛盾。
    ///
    /// 输出 schema(`QC_JSON_SHAPE_HINT`)明确写着 `issues` 只列**阻断性缺陷**,
    /// 因此「通过 + 非空 issues」不是一种合法状态 —— 模型要么把真缺陷藏进了 pass,
    /// 要么把非阻断观察错塞进了 `issues`。后者曾被本规则误杀(见下)。
    /// 本方法只做**一致性消解**(让结论与它自己列出的问题对齐),不引入任何
    /// 新的价值判断:`retryable` 仍完全采信模型 —— 模型认为不可重试就不可重试。
    ///
    /// 背景(实测 `llaew_20261012_175823.log`,闲鱼遍历任务):Plan 的 QC 报告
    /// verdict=pass / retryable=false,issues 却有 623 字,原文点名:
    /// 「wf-2~wf-5 存在明确的数据流依赖……但 depends_on 全部为空,未声明链式依赖
    /// ……并行调度时可能乱序执行」。这个**已被点名的硬缺陷**原样进入执行层,
    /// Kahn 把 5 个单元判成同层并发,消费方轮询等不到上游文件(实测约 195s 纯空等),
    /// 连带把产出最终交付物的 wf-5 挤到队尾饿死。
    /// **2026-10-12 反向消解(实测 `llaew_20261012_212828.log`,文心一言任务)**:
    /// 本方法原本只认一个方向(pass+issues→Fail),而模型大量存在
    /// 「verdict=pass + issues 里写一堆**自称非阻断**的备注」的稳定习惯 ——
    /// 4 次 QC 有 3 次如此。其中第 4 次 QC 自己确认「依赖无环、字段齐全、33 项检查
    /// 通过」,5 条 issues 每条都以「非阻断」二字开头,却被升成 Fail,又因模型给了
    /// `retryable=false` 而**直接把一份 9 WorkFlow 的合格方案判死**,任务以
    /// `outcome=failed` 收口,340s / 19 万 input token、零执行单元产出。
    /// 代价远大于收益:消解规则从「拦硬缺陷」变成了「按措辞杀方案」。
    ///
    /// 因此改为**按阻断性数量机械判定**,两个方向都纠:
    /// 1. `pass + issues 非空` → Fail(保留原方向:真缺陷不许藏进 pass);
    /// 2. `fail + issues 为空` → Pass(新增反向:判失败却没给出任何阻断缺陷,
    ///    说明它要表达的是「有观察但不通过」,应作为 notes 放行,不能杀方案)。
    ///
    /// 两个方向都不引入新的价值判断 —— 唯一判据是「`issues` 里有没有模型
    /// 自己认定的阻断缺陷」,与它把 verdict 写成什么无关。
    fn reconcile_verdict(mut self) -> Self {
        if self.verdict == Verdict::Pass && !self.issues.is_empty() {
            tracing::warn!(
                source = ?self.source,
                issues = self.issues.len(),
                "Quality 报告自相矛盾(verdict=pass 但列出了阻断性 issues),机械消解为 Fail;\
                 retryable 仍采信模型判定"
            );
            self.verdict = Verdict::Fail;
            // 消解必须留痕:实测事故的教训正是「QC 判 pass 却列了硬缺陷」被无声吞掉,
            // 缺陷原样进了执行层并在 AuditTrail 留下不可回溯的记录。
            let note = format!(
                "[第 163 轮一致性消解] 模型 verdict=pass 但列出 {} 条阻断性 issues,\
                 已按 issues 消解为 Fail。issues: {}",
                self.issues.len(),
                self.issues.join(" | ")
            );
            self.evidence = if self.evidence.trim().is_empty() {
                note
            } else {
                format!("{}\n{note}", self.evidence.trim())
            };
        } else if self.verdict == Verdict::Fail && self.issues.is_empty() {
            tracing::warn!(
                source = ?self.source,
                notes = self.notes.len(),
                "Quality 报告 verdict=fail 但未给出任何阻断性 issues(观察应写进 notes),\
                 机械消解为 Pass;retryable 仍采信模型判定"
            );
            let note = format!(
                "[2026-10-12 反向一致性消解] 模型 verdict=fail 但 issues 为空 \
                 (仅给出 {n} 条 notes),已消解为 Pass —— 非阻断观察不构成失败依据。notes: {s}",
                n = self.notes.len(),
                s = if self.notes.is_empty() {
                    "(无)".to_string()
                } else {
                    self.notes.join(" | ")
                }
            );
            self.verdict = Verdict::Pass;
            self.evidence = if self.evidence.trim().is_empty() {
                note
            } else {
                format!("{}\n{note}", self.evidence.trim())
            };
        }
        self
    }
}

/// QC 输入裁剪(第 140 轮提速优化):保头 [`QC_CLIP_HEAD`] + 尾 [`QC_CLIP_TAIL`] 字符,
/// 中间省略并标注原长。QC 判定只需「职责 + 期望 + 关键证据」,全量正文灌入会把
/// 单次 QC 推到 60s+(实测 wf-6 QC 64.9s 与 actual_output 规模正相关)。
/// 保留尾部是为了不丢「终答结论 / 收口段」——判定最重要的一段通常在结尾。
pub(crate) fn clip_qc_input(s: &str) -> String {
    const QC_CLIP_HEAD: usize = 6000;
    const QC_CLIP_TAIL: usize = 2000;
    let total = s.chars().count();
    if total <= QC_CLIP_HEAD + QC_CLIP_TAIL {
        return s.to_string();
    }
    let head: String = s.chars().take(QC_CLIP_HEAD).collect();
    let tail: String = s.chars().skip(total - QC_CLIP_TAIL).collect();
    format!("{head}\n…(中段省略 {} 字符,原文 {total} 字符)…\n{tail}", total - QC_CLIP_HEAD - QC_CLIP_TAIL)
}

/// 组装 SubAgent 单元 QC 提示词(纯函数,2026-09-19 第 93 轮抽出便于单测)。
///
/// F10(2026-09-10 第 25 轮):判定基准是「本单元职责」,整体目标仅作背景。
/// 此前 prompt 只给整体 goal,QC(真实 LLM)按整体目标判单元产物,SubAgent
/// 只完成了 wf-1 前置检查也被判「核心任务未完成」→ 无效重试风暴
/// (Debug 报告 debug_report_20260910_150805 问题报告 P1)。
///
/// 第 93 轮:`original_prompt`(用户原始输入)作为第一性事实插入 ——
/// 实测「主动聊天」被编排层降级为「保活等待」时,单元职责/期望输出均已失真,
/// QC 只对照失真标准必然蒙混通过;有原始输入时 QC 可识别「验收标准本身偏离任务」。
fn build_unit_qc_prompt(
    goal: &str,
    unit_scope: &str,
    expected_output: &str,
    actual_output: &str,
    trace_summary: &str,
    original_prompt: Option<&str>,
) -> String {
    let unit_label = "SubAgent 单元";
    let original_section = match original_prompt.map(str::trim).filter(|s| !s.is_empty()) {
        Some(orig) => format!(
            "第一性事实 · 用户原始输入(最高优先级,用于校验单元职责是否偏离任务):\n{orig}\n"
        ),
        None => String::new(),
    };
    let degradation_rule = if original_section.is_empty() {
        String::new()
    } else {
        "\n\
         反降级规则:若「本单元职责/期望输出」本身偏离用户原始输入(漏掉核心动作动词 —— 如把\n\
         「主动聊天/发送消息/保存报告」降级为「保活/等待/进程存活」,或把「操作界面」降级为\n\
         「纯 wait 空等」),无论执行是否达标都判 Fail 且 retryable=true,issues 中逐一指出\n\
         被漏掉的动词;纯等待/时间差/进程存活不构成任何 UI 操作的完成证据。\n\
         桌面目标保真(第 109 轮):用户原始输入指向独立桌面软件(豆包/微信/钉钉/飞书/QQ 等)\n\
         且未明确说「网页/Web 版」时 —— 执行轨迹全部落在 MCP_Web_Use 操作该软件的网页版\n\
         不构成等效完成(判 Fail 且 retryable=true,issues 指出工具错配,应改用 MCP_Window_Use);\n\
         suggestion 中禁止推荐「改用网页版/Web 版替代桌面软件」。\n\
         \n\
         ★ 目标一致性(第 128 轮,最高优先级硬门):用户原始输入显式指定了目标标识\n\
         (站点域名 / URL / 文件路径 / 应用名)时,执行轨迹的实际操作对象必须是**同一个**目标。\n\
         若实际操作了另一个站点/路径/应用 —— 包括「原目标超时/不可达/需登录后自行改派到\n\
         别的站点」—— 一律判 Fail 且 **retryable=false**(改派不是重试能修复的问题,重试只会\n\
         在错误目标上把产出做得更完整)。issues 必须写成「用户指定 X,实际操作 Y」的对账句式;\n\
         suggestion 中**严禁**推荐「改用其它站点/搜索引擎/缓存替代原目标」。\n\
         判定依据(任一即 Fail):① 轨迹出现 code=6001(目标站点越界被工具层阻断);\n\
         ② failure_signals 含 target_drift;③ 工具调用参数/最终 URL 的主机与用户指定域名不同;\n\
         ④ 产物内容全部来自另一个站点。**在错误目标上「完成得很漂亮」仍是 Fail** —— \n\
         这类产出会污染 session_memory,比直接失败危害更大。\n\
         例外:用户原始输入**没有**指定任何目标标识(如「搜一下最新 Rust 新闻」)时,\n\
         执行层自主选择站点属正常行为,不适用本门。"
            .to_string()
    };
    format!(
        "【Quality-Check: {unit_label}】\n\
         {original_section}\
         整体目标(仅作背景,不作为本单元判定依据): {goal}\n\
         本单元职责(判定依据): {unit_scope}\n\
         本单元期望输出: {expected_output}\n\
         实际输出: {}\n\
         \n\
         【执行轨迹】\n{trace_summary}\n\
         \n\
         请基于「本单元职责 + 期望输出 + 实际输出 + 执行轨迹」判定本单元是否完成,**不要用整体目标苛求本单元**\
         (整体目标的其余部分由后续 WorkFlow 单元负责)。按 JSON 输出 verdict/source/issues/suggestion/retryable/evidence。\n\
         判定提示:若轨迹包含 early_terminate / high_error_rate / text_failure_phrase 信号,通常应判 Fail 并把对应信号写入 issues。\n\
         遍历/覆盖类期望(「遍历 P1~P4」「覆盖清单: …」)以实际输出尾部的【机械导航足迹】对账 —— \
         它由工具层统计(open/navigate/new_tab 导航记账,零幻觉):distinct_pages 不足或 top_repeats 异常集中\
         (同一页 ≥4 次)即覆盖未达成,issues 引用足迹原文;足迹达标的子项不要凭想象判 Fail。\n\
         截图证据不作视觉判读依据(Read 读图片只回元信息存根),以文本台账/DOM 片段/操作前后状态为准。\n\
         若本单元是“验证预期失败”的负例,底层 Bash 非零本身可能是通过条件;此时必须在 evidence 中说明预期性,并引用最终验收输出 EXPECTED_NEGATIVE_OK。{degradation_rule}{QC_JSON_SHAPE_HINT}",
        clip_qc_input(actual_output),
    )
}

/// Quality 执行器。
pub struct QualityRunner {
    agent: Agent,
    db: Arc<Db>,
}

impl QualityRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        let agent = Agent::new(llm, AgentProfile::quality_check_profile());
        Self { agent, db }
    }

    /// 校验 SubAgent-Work 单元输出。
    ///
    /// 2026-09-09 第 05 轮:接收 `ExecutionTrace` 作为辅助判据,让 QC 模型
    /// 既能看实际输出文本,也能看到真实执行证据(工具调用次数 / 失败 / 早终止),
    /// 避免「黑盒判定」误判 silently pass。
    ///
    /// 2026-09-09 第 14 轮:返回 `(QualityReport, Usage)`,把 Quality 调用的 LLM
    /// 用量带回给 Orchestrator 累加到 `total_usage`(修复 stdout 用量与 Debug 报告
    /// 不一致的 Bug)。
    pub async fn check_subagent(
        &self,
        goal: &str,
        unit_scope: &str,
        expected_output: &str,
        actual_output: &str,
        trace: &ExecutionTrace,
        session_id: &str,
    ) -> Result<(QualityReport, Usage)> {
        self.check_subagent_with_source(
            AgentRole::SubAgent,
            goal,
            unit_scope,
            expected_output,
            actual_output,
            trace,
            session_id,
            None,
        )
        .await
    }

    /// [`check_subagent`] 的来源角色参数化版本:
    /// 让质检报告 source 与提示词标题反映真实执行角色(第 89 轮起执行器统一
    /// SubAgent,浏览器操控由 MCP_Web_Use 工具承担,参数保留供未来执行器扩展)。
    ///
    /// `original_prompt`(2026-09-19 第 93 轮):用户原始输入(第一性事实)。
    /// 实测微信聊天任务中,Main-Work 把「主动聊天/发送消息」降级为「保活等待」,
    /// QC 只看已降级的单元职责与期望输出,判 ✅ 蒙混通过 —— QC 必须能对照
    /// 用户原始输入发现「验收标准本身偏离任务」。
    #[allow(clippy::too_many_arguments)]
    pub async fn check_subagent_with_source(
        &self,
        source: AgentRole,
        goal: &str,
        unit_scope: &str,
        expected_output: &str,
        actual_output: &str,
        trace: &ExecutionTrace,
        session_id: &str,
        original_prompt: Option<&str>,
    ) -> Result<(QualityReport, Usage)> {
        let trace_summary = trace.render_prompt();
        let prompt = build_unit_qc_prompt(
            goal,
            unit_scope,
            expected_output,
            actual_output,
            &trace_summary,
            original_prompt,
        );
        self.run_check(prompt, source, actual_output, session_id, Some(trace))
            .await
    }

    /// 校验 Main-Work WorkFlow 计划(返回带 LLM Usage)。
    pub async fn check_main(
        &self,
        goal: &str,
        workflow_json: &str,
        session_id: &str,
    ) -> Result<(QualityReport, Usage)> {
        let prompt = format!(
            "【Quality-Check: Main-Work 单元】\n目标: {goal}\nWorkFlow JSON: {}\n\n请按 JSON 格式输出 verdict/source/issues/suggestion/retryable/evidence。{QC_JSON_SHAPE_HINT}",
            clip_qc_input(workflow_json),
        );
        self.run_check(prompt, AgentRole::MainWork, workflow_json, session_id, None)
            .await
    }

    /// 校验 Plan Markdown(返回带 LLM Usage)。
    pub async fn check_plan(
        &self,
        plan_markdown: &str,
        session_id: &str,
    ) -> Result<(QualityReport, Usage)> {
        let prompt = format!(
            "【Quality-Check: Plan 单元】\nPlan Markdown:\n{}\n\n请按 JSON 格式输出 verdict/source/issues/suggestion/retryable/evidence。{QC_JSON_SHAPE_HINT}",
            clip_qc_input(plan_markdown),
        );
        self.run_check(prompt, AgentRole::Plan, plan_markdown, session_id, None)
            .await
    }

    async fn run_check(
        &self,
        prompt: String,
        source: AgentRole,
        actual: &str,
        session_id: &str,
        trace: Option<&ExecutionTrace>,
    ) -> Result<(QualityReport, Usage)> {
        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;
        // P0:fail-closed —— 解析失败时返回 Verdict::Fail + retryable=true,
        // 触发 Yolo 回流,而非 fail-open(默认 Pass)绕过质检门。
        // 设计依据:`docs/Agent源码调研/专题-第八轮-Tool权限策略引擎与沙箱设计深度对比.md`
        // —— 5 态权限状态机 + fail-closed 默认;以及第十四轮 §1.2 错误恢复专题
        // —— silent pass 等于绕过熔断。
        let report = match parse_quality_report(&text, source) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    source = %source.as_str(),
                    "Quality 报告解析失败,fail-closed:返回 Verdict::Fail 触发回流"
                );
                // 落 Agent-Memory 记录原始输出长度,便于未来加入 JSON 修复链
                // (参考 atomcode repair.rs 8 段修复)
                let _ = memory::record_entry(
                    &self.db,
                    AgentRole::QualityCheck,
                    session_id,
                    "parse_failed",
                    &text,
                    Some(&format!("解析失败: {e}")),
                    serde_json::json!({ "raw_output_len": text.len() }),
                );
                QualityReport::fail(
                    source,
                    vec![format!("Quality 报告解析失败: {e}")],
                    "请确保 LLM 输出合法 JSON verdict/report 字段;建议重试或补充上下文。",
                    true,
                )
            }
        };
        let report = match trace {
            Some(trace) => gate_report_on_trace(report, source, trace, actual),
            None => report,
        };
        // 运行日志(2026-09-17 第 69 轮):Quality-Check 报告(决策)—— 全部 QC 入口
        // (单元/计划/Plan)汇聚本函数,一处记录 verdict/issues/建议/证据。
        tracing::info!(
            agent = "LsmAgentEmergentWork-Quality-Check",
            session = session_id,
            source = %source.as_str(),
            verdict = if report.verdict == Verdict::Pass { "pass" } else { "fail" },
            retryable = report.retryable,
            issues = %crate::logging::clip(&report.issues.join(" | ")),
            suggestion = %crate::logging::clip(&report.suggestion),
            evidence = %crate::logging::clip(&report.evidence),
            "Quality-Check 报告(决策)"
        );

        let _ = memory::record_entry(
            &self.db,
            AgentRole::QualityCheck,
            session_id,
            &format!("check source={}", source.as_str()),
            &format!("verdict={:?}", report.verdict),
            if report.verdict == Verdict::Fail {
                Some(&report.suggestion)
            } else {
                None
            },
            serde_json::json!({ "issues": &report.issues, "retryable": report.retryable }),
        );

        let _ = actual;
        Ok((report, usage))
    }
}

/// 用不可伪造的执行轨迹约束 Quality-Check 的“通过”结论。
///
/// LLM QC 是语义判断,但不能覆盖进程级事实:
/// - 进程级硬失败信号(early_terminate / high_error_rate)出现时,pass 一律降级为 fail;
/// - 文本级软信号(text_failure_phrase)降为“证据可豁免”(2026-09-11 第三十六轮 LA-3):
///   终答为提升可观测性引用工具输出摘录时,摘录中的 "AssertionError:"/"error:" 等日志
///   措辞会误触发该信号 —— 引用日志 ≠ 模型自己声称失败。QC 提供非空 evidence 说明
///   预期性,或预期负例契约确认(EXPECTED_NEGATIVE_OK)任一即可保持 pass;
/// - Bash 非零退出是弱信号:若 QC 想把它解释为预期负例,必须提供非空 evidence;
///   空 evidence 的 pass 视为无效质检,触发回流/失败收口。
///
/// 这样保留“命令预期失败”的测试空间,同时杜绝 D01Q4 这类 exit=1 被静默判成功。
fn gate_report_on_trace(
    report: QualityReport,
    source: AgentRole,
    trace: &ExecutionTrace,
    actual_output: &str,
) -> QualityReport {
    if report.verdict == Verdict::Fail {
        return report;
    }

    // 负例测试契约:底层命令非零是“通过条件”,最终断言必须返回零并输出
    // EXPECTED_NEGATIVE_OK。这样避免 QC 模型忘记填 evidence 时,把已经成功验证
    // 的预期负例反复重试;同时仍要求最终验收命令成功,防止用文字口头豁免。
    let expected_negative_confirmed = actual_output.contains("EXPECTED_NEGATIVE_OK")
        && trace.bash_exit_nonzero_count > 0
        && trace.last_bash_exit_code == 0;
    // 文本软信号单独判定:仅当不存在进程级硬信号时才允许证据豁免
    let has_text_failure_phrase = trace
        .failure_signals
        .iter()
        .any(|s| s == "text_failure_phrase");
    let has_hard_failure = trace
        .failure_signals
        .iter()
        .any(|s| s.starts_with("early_terminate:") || s.starts_with("high_error_rate:"));
    let text_phrase_only = has_text_failure_phrase && !has_hard_failure;
    let hard_strong_failure = trace.is_failed() && !text_phrase_only;
    let unevidenced_text_phrase =
        text_phrase_only && report.evidence.trim().is_empty() && !expected_negative_confirmed;
    let unevidenced_bash_failure = trace.bash_exit_nonzero_count > 0
        && report.evidence.trim().is_empty()
        && !expected_negative_confirmed;
    if !hard_strong_failure && !unevidenced_text_phrase && !unevidenced_bash_failure {
        return report;
    }

    let issue = if hard_strong_failure {
        "执行轨迹包含强失败信号,Quality-Check 的 pass 结论被 trace 证据门拒绝".to_string()
    } else if unevidenced_text_phrase {
        "终答包含失败措辞(可能引用了工具日志摘录)且 Quality-Check 未提供非空 evidence 说明预期性"
            .to_string()
    } else {
        "Bash 命令非零退出且 Quality-Check 未提供非空 evidence 说明预期性".to_string()
    };
    let suggestion =
        "请修正命令或产物后重跑;若非零退出是预期负例,Quality-Check 必须在 evidence 中说明。".to_string();
    let mut gated = QualityReport::fail(source, vec![issue], &suggestion, true);
    gated.evidence = trace.render_prompt();
    gated
}

/// 解析 Quality JSON 输出。
/// 直接解析失败时自动走 JSON 修复链(`json_repair` Tier-1 语法修复);
/// 修复不了仍返回 Err(上轮 P0 fail-closed 语义不变,截断 JSON 刻意不补全)。
///
/// 2026-09-17 第 82+ 轮 P0-3:新增「文本关键词降级兜底」 —— 当 JSON 解析完全失败
/// 且文本含可识别关键词时,推断 verdict / retryable / source,避免 591s 任务因
/// Provider 网关截断 verdict 字段而整轮 fail。fail-closed 语义不变:仍优先返回
/// Err,关键词降级仅在 Err 之后作为**最后兜底**。
///
/// 2026-09-18 第 87 轮:JSON 块**提取成功但反序列化失败**(如 LLM 输出畸形
/// `{"$text":...}` 缺 verdict)时不再立即 Err —— 记录诊断后继续尝试独立 JSON
/// 与文本关键词降级,全部失败才返回合并诊断。实测场景:QC 正文 suggestion 里
/// 明确含「失败」信号,却因顶层 JSON 畸形被浪费,触发无谓回流。
/// 解析 QC 报告(对外唯一入口)。第 163 轮的**一致性消解**只作用于模型
/// 真正输出 JSON 的两条路径(`verdict=pass` 且 issues 非空时消解为 Fail)。
///
/// 文本关键词降级路径**不参与**消解:那条路径的 `issues` 装的是「JSON 解析失败」
/// 这类**降级说明**(见 `parse_quality_report_text_fallback`),不是模型列出的缺陷,
/// 拿它去否定降级出来的 Pass 会把整条降级链废掉。
pub fn parse_quality_report(text: &str, source: AgentRole) -> Result<QualityReport> {
    parse_quality_report_raw(text, source)
}

fn parse_quality_report_raw(text: &str, source: AgentRole) -> Result<QualityReport> {
    let mut last_diag: Option<String> = None;
    if let Some(json_str) = extract_json_block(text) {
        // 第 99 轮:先做 verdict/source/retryable 缺失的语义修复,再走语法修复链
        let prepared = repair_quality_json(json_str, source).unwrap_or_else(|| json_str.to_string());
        match crate::agent::json_repair::try_parse::<QualityReport>(&prepared) {
            Ok(r) => return Ok(r.reconcile_verdict()),
            Err(diag) => {
                tracing::warn!(diag = %diag, "Quality ```json 块解析失败,继续降级链");
                last_diag = Some(diag);
            }
        }
    }
    if let Some(json_str) = extract_standalone_json(text) {
        let prepared = repair_quality_json(json_str, source).unwrap_or_else(|| json_str.to_string());
        match crate::agent::json_repair::try_parse::<QualityReport>(&prepared) {
            Ok(mut r) => {
                if r.source != source {
                    r.source = source;
                }
                return Ok(r.reconcile_verdict());
            }
            Err(diag) => {
                tracing::warn!(diag = %diag, "Quality 独立 JSON 解析失败,继续降级链");
                last_diag = Some(diag);
            }
        }
    }
    // 2026-09-17 第 82+ 轮 P0-3:JSON 完全无法提取 → 文本关键词降级。
    // 仅当 JSON 解析链 fail-closed 之后触发,且文本同时含「质检报告」上下文
    // 关键词 + verdict 强信号时才走降级。其它场景仍 Err(避免误判)。
    if let Some(report) = parse_quality_report_text_fallback(text, source) {
        tracing::warn!(
            source = %source.as_str(),
            text_len = text.len(),
            "Quality JSON 解析失败,走文本关键词降级(避免整轮 fail)"
        );
        return Ok(report);
    }
    Err(crate::error::AgentError::Other(match last_diag {
        Some(diag) => format!("Quality JSON 解析失败: {diag}"),
        None => "未找到合法的 Quality JSON".into(),
    }))
}

/// 2026-09-17 第 82+ 轮 P0-3:文本关键词降级。
///
/// 适用场景:Provider 网关 / 日志截断把 emit JSON 的 `verdict` 字段切掉,
/// 仅剩自然语言结论。LLM 通常会在 JSON 后追加一行「结论:通过 / 结论:失败」。
/// 启发式匹配:
/// - verdict 关键词:`通过/pass/✅` → Pass;`失败/fail/❌/错误/缺陷` → Fail
///   (Pass 强信号优先,否则判 Fail);
/// - retryable:含「重试/retry/建议再试/next_iter」 → true,否则 false;
/// - source:从枚举关键词匹配(yolo / plan / main / subagent / quality_check /
///   session_context),否则用调用方传入的 source。
///
/// 返回 `Some(QualityReport)` 当且仅当 verdict 强信号命中(否则返回 None)。
fn parse_quality_report_text_fallback(text: &str, source: AgentRole) -> Option<QualityReport> {
    let lower = text.to_lowercase();
    // verdict:Pass 强信号优先(包含「通过」或"pass"或 ✅)
    let pass_signals = ["结论:通过", "verdict: pass", "通过", "✅", "qc pass"];
    let fail_signals = ["结论:失败", "verdict: fail", "失败", "❌", "qc fail", "错误", "存在缺陷"];
    let verdict = if pass_signals.iter().any(|s| lower.contains(&s.to_lowercase())) {
        Verdict::Pass
    } else if fail_signals.iter().any(|s| lower.contains(&s.to_lowercase())) {
        Verdict::Fail
    } else {
        return None; // 没有强信号,不降级,仍 Err
    };
    // retryable
    let retry_signals = ["可重试", "建议重试", "retryable: true", "retry", "再试"];
    let retryable = retry_signals.iter().any(|s| lower.contains(&s.to_lowercase()));
    // source:从文本识别 AgentRole 枚举;不命中则用调用方传入的 source
    let source_inferred = if lower.contains("quality_check") {
        AgentRole::QualityCheck
    } else if lower.contains("subagent") {
        AgentRole::SubAgent
    } else if lower.contains("main") {
        AgentRole::MainWork
    } else if lower.contains("plan") {
        AgentRole::Plan
    } else if lower.contains("yolo") {
        AgentRole::Yolo
    } else {
        source
    };
    // issues:截前 200 字符作为 evidence(供 Debug 报告观测)
    let evidence = text.chars().take(200).collect::<String>();
    Some(QualityReport {
        verdict,
        source: source_inferred,
        issues: vec![format!("JSON 解析失败,文本降级;evidence={evidence}")],
        notes: Vec::new(),
        suggestion: String::new(),
        retryable,
        evidence,
    })
}

/// 第 99 轮:verdict / source / retryable 缺失的语义修复(serde 反序列化前的兜底,
/// 语法修复链管不到「字段缺失」这类语义级残缺)。
///
/// 实测场景(内网站点登录任务,2026-09-20):QC LLM 输出合法 JSON 但漏掉 `verdict`
/// 字段 → `missing field 'verdict'` 反序列化失败 → fail-closed 判 Fail 白白消耗
/// 一次局部重试预算。推断优先级:
/// 1. `pass`(bool)→ pass/fail;
/// 2. `result` / `status` / `conclusion` 字符串含 pass/通过 或 fail/失败;
/// 3. `issues` 非空 → Fail(列出问题即发现问题)。
/// 均无信号 → 返回 None(保持 fail-closed Err,不误判)。
/// `source` 缺失时用调用方 source 补齐(serde_json::to_value,与反序列化严格互逆);
/// `retryable` 缺失时按 verdict 推断(fail → true)。
fn repair_quality_json(json_str: &str, source: AgentRole) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let obj = v.as_object_mut()?;
    if !obj.contains_key("verdict") {
        let result_str = |k: &str| -> Option<String> {
            let s = obj.get(k)?.as_str()?.to_lowercase();
            if s.contains("pass") || s.contains("通过") {
                Some("pass".to_string())
            } else if s.contains("fail") || s.contains("失败") {
                Some("fail".to_string())
            } else {
                None
            }
        };
        let inferred: String = obj
            .get("pass")
            .and_then(serde_json::Value::as_bool)
            .map(|b| if b { "pass" } else { "fail" }.to_string())
            .or_else(|| ["result", "status", "conclusion"].iter().find_map(|k| result_str(k)))
            .or_else(|| {
                let issues = obj.get("issues")?;
                let non_empty = issues.as_array().map(|a| !a.is_empty()).unwrap_or(false);
                non_empty.then(|| "fail".to_string())
            })?;
        obj.insert("verdict".into(), serde_json::Value::String(inferred));
    }
    if !obj.contains_key("source") {
        let sv = serde_json::to_value(source).ok()?;
        obj.insert("source".into(), sv);
    }
    if !obj.contains_key("retryable") {
        let is_fail = obj.get("verdict").and_then(serde_json::Value::as_str) == Some("fail");
        obj.insert("retryable".into(), serde_json::Value::Bool(is_fail));
    }
    Some(v.to_string())
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
    let json_text = &text[content_start..content_start + end].trim();
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

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== 第 140 轮:QC 输入裁剪 ====================

    #[test]
    fn clip_qc_input_short_is_identity() {
        let s = "短输入原样返回";
        assert_eq!(clip_qc_input(s), s);
    }

    #[test]
    fn clip_qc_input_long_keeps_head_and_tail_with_marker() {
        let head: String = std::iter::repeat('H').take(6000).collect();
        let mid: String = std::iter::repeat('M').take(5000).collect();
        let tail: String = std::iter::repeat('T').take(2000).collect();
        let s = format!("{head}{mid}{tail}");
        let out = clip_qc_input(&s);
        assert!(out.starts_with(&head), "头部 6000 字符应保留");
        assert!(out.ends_with(&tail), "尾部 2000 字符应保留");
        assert!(out.contains("中段省略 5000 字符"), "应标注省略量,实际: {}", &out[6000..6100]);
        assert!(out.contains("原文 13000 字符"), "应标注原文总长");
        assert!(out.len() < s.len(), "裁剪后应更短");
    }

    #[test]
    fn quality_report_pass_default() {
        let r = QualityReport::pass(AgentRole::SubAgent);
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::SubAgent);
    }

    #[test]
    fn quality_report_fail_sets_retryable() {
        let r = QualityReport::fail(AgentRole::SubAgent, vec!["x".into()], "再试一次", true);
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
        assert_eq!(r.suggestion, "再试一次");
    }

    // ========== 第 93 轮:QC prompt 反降级(原始输入透传) ==========

    #[test]
    fn qc_prompt_includes_original_prompt_and_anti_degradation_rule() {
        let p = build_unit_qc_prompt(
            "启动微信保持10分钟",
            "打开微信并维持对话功能10分钟",
            "保活≥600s",
            "已保活612s",
            "iter=10 tools=9",
            Some("打开微信并主动与赵玲玲聊天,每分钟2次,保存 Markdown 报告"),
        );
        assert!(p.contains("第一性事实 · 用户原始输入"));
        assert!(p.contains("赵玲玲"));
        assert!(p.contains("反降级规则"));
        assert!(p.contains("纯等待/时间差/进程存活不构成任何 UI 操作的完成证据"));
    }

    #[test]
    fn qc_prompt_without_original_prompt_has_no_degradation_rule() {
        let p = build_unit_qc_prompt("g", "scope", "exp", "act", "trace", None);
        assert!(!p.contains("第一性事实"));
        assert!(!p.contains("反降级规则"));
        // 空白原始输入视同无
        let p2 = build_unit_qc_prompt("g", "scope", "exp", "act", "trace", Some("  \n "));
        assert!(!p2.contains("第一性事实"));
    }

    // ========== 第 109 轮:桌面目标保真(网页版不构成等效完成) ==========

    #[test]
    fn qc_prompt_desktop_fidelity_rule_present() {
        let p = build_unit_qc_prompt(
            "找到软件豆包,输入信息,读取对话返回的结果",
            "在豆包中输入查询并读取结果",
            "chat_log 含 [SEND] 行",
            "已在网页版完成",
            "iter=8 tools=MCP_Web_Use",
            Some("找到软件 豆包 , 输入信息,读取对话返回的结果,显示相关的对话信息"),
        );
        // 反降级规则必须包含桌面目标保真条款
        assert!(p.contains("桌面目标保真"), "{p}");
        assert!(p.contains("不构成等效完成"), "{p}");
        // suggestion 层面禁止推荐网页版
        assert!(p.contains("禁止推荐「改用网页版"), "{p}");
        assert!(p.contains("MCP_Web_Use"), "{p}");
    }

    // ========== 第 128 轮:目标一致性硬门(禁止改派目标站点) ==========

    #[test]
    fn qc_prompt_target_consistency_gate_present() {
        let p = build_unit_qc_prompt(
            "抓取 anthropic.com 最新 3 篇文章",
            "打开目标网站并 explore 首页结构",
            "final_url 主域 = anthropic.com",
            "已在 ithome.com 抓到 3 篇文章",
            "iter=4 tools=5 target_drift:ithome.com",
            Some("打开 https://www.anthropic.com/ ,找出最新的 3 篇文章,显示标题和 URL"),
        );
        assert!(p.contains("目标一致性"), "{p}");
        assert!(p.contains("最高优先级硬门"), "{p}");
        // 改派必须判不可重试(重试只会在错误目标上把产出做得更完整)
        assert!(p.contains("retryable=false"), "{p}");
        // 对账句式 + 禁止推荐替代品
        assert!(p.contains("用户指定 X,实际操作 Y"), "{p}");
        assert!(p.contains("严禁」推荐") || p.contains("严禁**推荐") || p.contains("严禁"), "{p}");
        assert!(p.contains("搜索引擎"), "{p}");
        // 机械判据必须写进提示词,QC 才能引用
        assert!(p.contains("6001"), "{p}");
        assert!(p.contains("target_drift"), "{p}");
        // 漂亮地完成错误目标仍是 Fail
        assert!(p.contains("完成得很漂亮"), "{p}");
        assert!(p.contains("session_memory"), "{p}");
    }

    #[test]
    fn qc_prompt_target_consistency_has_open_task_exception() {
        // 用户没指定站点时不得套用目标一致性门,否则合法开放任务会被误判
        let p = build_unit_qc_prompt(
            "搜索最新 Rust 新闻",
            "打开站点并抓取",
            "3 条标题+链接",
            "已在 ithome.com 抓到",
            "iter=3 tools=4",
            Some("搜一下最新的 Rust 语言新闻,给我 3 条标题和链接"),
        );
        assert!(p.contains("例外"), "{p}");
        assert!(p.contains("自主选择站点属正常行为"), "{p}");
    }

    #[test]
    fn qc_prompt_target_consistency_absent_without_original_prompt() {
        let p = build_unit_qc_prompt("g", "scope", "exp", "act", "trace", None);
        assert!(!p.contains("目标一致性"), "无原始输入时不得注入该门: {p}");
    }

    #[test]
    fn parse_quality_report_from_block() {
        let text = r#"
```json
{
  "verdict": "pass",
  "source": "subagent",
  "issues": [],
  "suggestion": "",
  "retryable": false,
  "evidence": ""
}
```"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
    }

    // ========== P0 fail-closed 测试 ==========
    // 验证 JSON 解析失败时返回 Verdict::Fail + retryable=true
    // (替代历史的 fail-open 默认 Pass)

    #[test]
    fn parse_quality_report_no_json_fails_closed() {
        // 完全无 JSON → 应返回 Err(让上游 fail-closed 转为 Fail)
        let text = "这是一段不含 JSON 的自然语言,无法被解析为质检报告。";
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_truncated_json_fails_closed() {
        // JSON 被截断 / 不合法 → 应返回 Err
        let text = r#"以下是质检报告:
```json
{
  "verdict": "pass",
  "source": "subagent",
  "issues": [],
  // 后面被截断"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_missing_required_field_fails_closed() {
        // 缺 verdict 字段 → serde 反序列化失败 → Err
        let text = r#"```json
{
  "source": "subagent",
  "issues": [],
  "retryable": false
}
```"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("verdict"));
    }

    #[test]
    fn quality_report_fail_constructor_sets_retryable() {
        // 验证 fail-closed 路径所需的构造函数
        let r = QualityReport::fail(
            AgentRole::SubAgent,
            vec!["Quality 报告解析失败: 字段缺失".to_string()],
            "请确保 LLM 输出合法 JSON verdict/report 字段",
            true,
        );
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
        assert!(r.issues.iter().any(|i| i.contains("解析失败")));
        assert!(r.suggestion.contains("JSON"));
    }

    // ========== 2026-09-17 第 82+ 轮 P0-3 测试 ==========
    // 文本关键词降级:JSON 完全无法提取时,从正文关键词推断 verdict / retryable / source,
    // 避免 Provider 网关截断 verdict 字段导致整轮 fail。

    #[test]
    fn parse_quality_report_text_fallback_pass() {
        // 自然语言结论 + 「通过」关键词 → 降级 Pass
        let text = "经过核查,本单元所有子任务均完成,验收材料齐全,结论:通过";
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::SubAgent);
        assert!(r.issues[0].contains("JSON 解析失败"));
    }

    #[test]
    fn parse_quality_report_text_fallback_fail_with_retry() {
        // 含「失败」+「重试」 → 降级 Fail + retryable=true
        let text = "子任务超时,前置未达成,结论:失败,建议重试";
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
    }

    // ========== 第 163 轮:verdict/issues 一致性消解 ==========

    #[test]
    fn qc_pass_with_issues_is_reconciled_to_fail() {
        // 实测事故形态:Plan 的 QC verdict=pass / retryable=false,issues 却点名
        // 「depends_on 全部为空,未声明链式依赖」—— 已被点名的硬缺陷原样进了执行层。
        let text = r#"```json
{"verdict":"pass","source":"plan","issues":["wf-2~wf-5 存在数据流依赖,但 depends_on 全部为空,未声明链式依赖,并行调度时可能乱序执行"],"suggestion":"","retryable":false,"evidence":"5 个 WorkFlow 已落盘"}
```"#;
        let r = parse_quality_report(text, AgentRole::Plan).unwrap();
        assert_eq!(r.verdict, Verdict::Fail, "pass + 非空 issues 必须消解为 Fail");
        // retryable 仍采信模型判定(不引入新的价值判断)
        assert!(!r.retryable);
        assert_eq!(r.issues.len(), 1);
        assert!(r.evidence.contains("一致性消解"), "消解必须留痕: {}", r.evidence);
        // 模型原有证据不被覆盖
        assert!(r.evidence.contains("5 个 WorkFlow 已落盘"));
    }

    #[test]
    fn qc_pass_without_issues_is_untouched() {
        let text = r#"{"verdict":"pass","source":"subagent","issues":[],"suggestion":"","retryable":false,"evidence":"产物齐全"}"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert!(!r.evidence.contains("一致性消解"), "正常 Pass 不该被加证据串");
        assert_eq!(r.evidence, "产物齐全", "模型证据原样保留");
    }

    #[test]
    fn qc_text_fallback_pass_is_not_reconciled() {
        // 降级路径的 issues 装的是「JSON 解析失败」这类降级说明,不是缺陷,
        // 不能拿它否定降级出来的 Pass。
        let text = "经过核查,本单元所有子任务均完成,验收材料齐全,结论:通过";
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert!(r.issues[0].contains("JSON 解析失败"));
    }

    // ===== 2026-10-12:反向一致性消解(实测 llaew_20261012_212828.log 文心一言任务) =====

    /// 实测事故形态:QC 判 `fail` 但一条阻断缺陷都没给,issues 里装的是它自己
    /// 都标了「非阻断」的观察,模型还给了 `retryable=false` —— 旧规则只认
    /// pass+issues→Fail 一个方向,这份被"按措辞判死"的合格方案一路放行到
    /// 任务以 `outcome=failed` 收口(340s / 19 万 input token、零执行单元)。
    /// 新增反向消解:判失败却无阻断缺陷 → 按 notes 放行。
    #[test]
    fn qc_fail_without_blocking_issues_is_reconciled_to_pass() {
        let text = r#"```json
{"verdict":"fail","source":"main","issues":[],"notes":["非阻断-范围越界:建议把素材研读移出关键路径","非阻断-依赖链风险:素材目录不可读会导致对话链停摆"],"suggestion":"","retryable":false,"evidence":"三项阻断条件均不成立"}
```"#;
        let r = parse_quality_report(text, AgentRole::MainWork).unwrap();
        assert_eq!(
            r.verdict,
            Verdict::Pass,
            "fail + 空 issues(仅 notes)应反向消解为 Pass,不能按措辞杀方案"
        );
        assert!(r.evidence.contains("反向一致性消解"), "消解必须留痕: {}", r.evidence);
        assert!(r.evidence.contains("范围越界"), "notes 原文应留痕供回看");
        // 模型原有证据不被覆盖
        assert!(r.evidence.contains("三项阻断条件均不成立"));
        // retryable 仍完全采信模型
        assert!(!r.retryable);
    }

    /// 反向消解的边界:`fail` + 非空 issues =模型确实认定了阻断缺陷,保持 Fail。
    #[test]
    fn qc_fail_with_blocking_issues_stays_fail() {
        let text = r#"{"verdict":"fail","source":"main","issues":["wf-3 素材路径写成裸文件名,Read 必然 FileNotFound"],"notes":["措辞口径可再统一"],"suggestion":"改用绝对路径","retryable":true,"evidence":"已回读方案文件"}"#;
        let r = parse_quality_report(text, AgentRole::MainWork).unwrap();
        assert_eq!(r.verdict, Verdict::Fail, "有阻断缺陷时保持 Fail");
        assert!(
            !r.evidence.contains("一致性消解"),
            "本来就没有矛盾,不该加消解痕迹: {}",
            r.evidence
        );
        assert!(r.retryable);
    }

    /// 新旧两个方向互不干扰:pass+issues(有阻断缺陷)→ Fail;
    /// fail+空 issues → Pass;pass+空 issues → 原样 Pass。
    #[test]
    fn qc_reconcile_both_directions_are_covered() {
        let pass_clean = r#"{"verdict":"pass","source":"plan","issues":[],"notes":["建议补充阈值"],"retryable":false,"evidence":"结构齐全"}"#;
        assert_eq!(
            parse_quality_report(pass_clean, AgentRole::Plan).unwrap().verdict,
            Verdict::Pass
        );

        let pass_with_blocking = r#"{"verdict":"pass","source":"plan","issues":["depends_on 全部为空,存在硬缺陷"],"retryable":false,"evidence":"已落盘"}"#;
        assert_eq!(
            parse_quality_report(pass_with_blocking, AgentRole::Plan).unwrap().verdict,
            Verdict::Fail
        );

        let fail_without_blocking = r#"{"verdict":"fail","source":"plan","issues":[],"notes":["非阻断观察"],"retryable":false,"evidence":"已落盘"}"#;
        assert_eq!(
            parse_quality_report(fail_without_blocking, AgentRole::Plan).unwrap().verdict,
            Verdict::Pass
        );
    }

    /// QC 提示词必须说清 issues(阻断)与 notes(非阻断)的区别,
    /// 否则模型仍会沿用"pass + 观察写进 issues"的旧习惯(第 163 轮规则的误杀源)。
    #[test]
    fn qc_prompt_distinguishes_blocking_issues_from_notes() {
        assert!(
            QC_JSON_SHAPE_HINT.contains("\"notes\""),
            "schema 提示必须给出 notes 字段"
        );
        assert!(
            QC_JSON_SHAPE_HINT.contains("阻断性") && QC_JSON_SHAPE_HINT.contains("非阻断"),
            "schema 提示必须区分阻断/非阻断"
        );
    }

    #[test]
    fn parse_quality_report_text_fallback_no_strong_signal_fails_closed() {
        // 无强信号关键词 → 仍 fail-closed(避免误判)
        let text = "今天天气很好,适合出门散步";
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_text_fallback_source_inference() {
        // 含「quality_check」→ 推断 source=QualityCheck(即便传入其它角色)
        let text = "QC pass:quality_check 单元所有动作完成";
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::QualityCheck);
    }

    // ========== 2026-09-18 第 87 轮 ==========
    // JSON 块提取成功但顶层畸形(缺 verdict)时,不再立即 Err ——
    // 继续走文本关键词降级,正文里的 fail 信号不应被浪费(实测:QC 输出
    // `{"$text":...}` 畸形 JSON 触发无谓回流)。

    #[test]
    fn parse_quality_report_malformed_json_falls_back_to_text_keywords() {
        let text = r#"```json
{
  "$text": "subagent 自我归因错误:声称工具未暴露,但日志显示已真实执行",
  "item": {"$text": "期望输出缺失:发送记录 0 条,任务失败,建议重试"}
}
```"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
        assert!(r.issues[0].contains("JSON 解析失败"));
    }

    #[test]
    fn parse_quality_report_malformed_json_without_signal_still_fails_closed() {
        // 畸形 JSON 且正文无 verdict 强信号 → 仍 fail-closed Err(语义不变)
        let text = r#"```json
{
  "$text": "今天天气很好",
  "item": {"$text": "适合出门散步"}
}
```"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("verdict"));
    }

    #[test]
    fn trace_gate_rejects_empty_evidence_for_bash_nonzero() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 1;
        trace.collect_failure_signals("完成");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "完成",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.retryable);
        assert!(gated.evidence.contains("bash_exit_nonzero=1"));
    }

    #[test]
    fn trace_gate_allows_expected_bash_nonzero_with_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 2;
        trace.collect_failure_signals("负向用例按预期返回非零");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "exit=2 是本负向用例的预期结果".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "完成");

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_strong_failure_even_with_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.early_terminated = true;
        trace.early_terminate_reason = "max_iter:2".into();
        trace.collect_failure_signals("完成");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "模型解释这是预期情况".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "完成");

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("强失败信号"));
    }

    #[test]
    fn trace_gate_allows_explicit_negative_confirmation_without_qc_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 0;
        trace.collect_failure_signals("EXPECTED_NEGATIVE_OK");

        let output = "工具输出摘录\nEXPECTED_NEGATIVE_OK\n<exit_code>0</exit_code>";
        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            output,
        );

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_negative_marker_without_final_success() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 1;
        trace.collect_failure_signals("EXPECTED_NEGATIVE_OK");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "EXPECTED_NEGATIVE_OK",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("Bash 命令非零退出"));
    }

    #[test]
    fn trace_gate_allows_text_phrase_with_expected_negative_contract() {
        // LA-3:终答摘录引用了 AssertionError 日志(误触发 text_failure_phrase),
        // 但预期负例契约确认(先非零复现 + 最终断言命令 exit=0 + EXPECTED_NEGATIVE_OK)
        // 应保持 pass,不应反复回流重试。
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 0;
        trace.collect_failure_signals(
            "MOCK_FINAL_ANSWER: 验证通过。\n\n[工具输出摘录]\nAssertionError: producer error: 有 queue.Full 未处理",
        );
        assert!(trace
            .failure_signals
            .iter()
            .any(|s| s == "text_failure_phrase"));

        let output = "AssertionError 摘录\nEXPECTED_NEGATIVE_OK\n<exit_code>0</exit_code>";
        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            output,
        );

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_text_phrase_without_contract_or_evidence() {
        // LA-3:纯文本失败措辞(无契约确认、无 QC evidence)仍必须拒绝,
        // 防止模型口头上声称成功、引用失败日志蒙混过关。
        let mut trace = ExecutionTrace::default();
        trace.collect_failure_signals("执行失败: 无法完成队列写入");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "任务完成",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("失败措辞"));
    }

    #[test]
    fn trace_gate_allows_text_phrase_with_qc_evidence() {
        // LA-3:QC 用非空 evidence 说明"该失败措辞来自预期负例日志摘录"→ 放行
        let mut trace = ExecutionTrace::default();
        trace.collect_failure_signals("AssertionError: 预期复现摘录");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "终答中的 AssertionError 是负例脚本预期输出,断言已确认捕获".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "任务完成");

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_hard_failure_still_rejects_with_text_phrase_present() {
        // LA-3:early_terminate 与 text_failure_phrase 并存时按进程级硬信号无条件拒绝
        let mut trace = ExecutionTrace::default();
        trace.early_terminated = true;
        trace.early_terminate_reason = "max_iter:2".into();
        trace.collect_failure_signals("[MaxIterationsExceeded] 迭代达到 2 次上限未得到最终答案");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "模型解释这是预期情况".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "任务完成");

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("强失败信号"));
    }

    // ========== 2026-09-20 第 99 轮:verdict 缺失语义修复测试 ==========
    // 实测场景:QC LLM 输出合法 JSON 但漏 verdict → 反序列化失败 → fail-closed
    // 白耗一次局部重试预算。修复链在语法修复前按信号推断补字段。

    #[test]
    fn repair_quality_report_missing_verdict_with_result_string() {
        let text = r#"```json
{
  "source": "subagent",
  "result": "fail",
  "issues": ["未执行切换子账号操作"],
  "suggestion": "先 click 切换子账号 Tab"
}
```"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable, "verdict 推断为 fail 时 retryable 应补 true");
        assert!(r.issues.iter().any(|i| i.contains("子账号")));
    }

    #[test]
    fn repair_quality_report_missing_verdict_with_pass_bool() {
        let text = r#"{"pass": true, "issues": [], "retryable": false}"#;
        let r = parse_quality_report(text, AgentRole::MainWork).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::MainWork, "缺失 source 应被调用方补齐");
    }

    #[test]
    fn repair_quality_report_missing_verdict_with_nonempty_issues() {
        // issues 非空 + 无其它信号 → 推断 Fail
        let text = r#"{"issues": ["登录按钮未点击"], "suggestion": "重试"}"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
    }

    #[test]
    fn repair_quality_report_missing_all_signals_still_fails_closed() {
        // 无任何可推断信号 → 保持 fail-closed Err(不误判)
        let text = r#"{"source": "subagent", "issues": [], "retryable": false}"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("verdict"));
    }

    #[test]
    fn repair_quality_report_conclusion_chinese_keyword() {
        let text = r#"{"conclusion": "验收通过", "evidence": "菜单已抓取"}"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
    }

    #[test]
    fn quality_prompt_contains_json_shape_hint() {
        // 提示词硬化:三个 QC 提示词都必须携带 JSON 形状硬性要求
        let p = build_unit_qc_prompt(
            "目标", "职责", "期望", "实际", "执行轨迹", None,
        );
        assert!(p.contains("第一个字段必须是 verdict"), "单元 QC 提示词应含形状提示");
    }

}
