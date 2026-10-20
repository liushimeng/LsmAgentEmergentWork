//! Quality-Check Agent:质检层,对 SubAgent-Work / Main-Work / Plan 单元做质量校验。

mod dead_loop;

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

// 死循环根因裁决通道的实现拆入子模块(工程 1800 行拆分规范);
// 对外类型原路径 `crate::agent::quality::LoopRootCause` 零改动。
pub use dead_loop::{LoopResolution, LoopRootCause};
// 裁决通道的纯函数仅单测直接调用;运行时代路径走 `QualityRunner::check_dead_loop`。
#[cfg(test)]
use dead_loop::{build_dead_loop_prompt, parse_loop_resolution};

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
///
/// 死循环硬门段:轨迹带 `dead_loop` 诊断时,QC 必须判 Fail 并归类根因。
/// 单元 QC 提示词(无死循环段)。
///
/// 绝大多数调用方(含既有单测)不需要关心死循环段 —— 保持 6 参签名不变,
/// 避免为一个条件段改动全部调用点。带诊断的运行时路径走
/// [`build_unit_qc_prompt_full`]。
#[allow(clippy::too_many_arguments)]
pub(super) fn build_unit_qc_prompt(
    goal: &str,
    unit_scope: &str,
    expected_output: &str,
    actual_output: &str,
    trace_summary: &str,
    original_prompt: Option<&str>,
) -> String {
    build_unit_qc_prompt_full(
        goal,
        unit_scope,
        expected_output,
        actual_output,
        trace_summary,
        original_prompt,
        "",
    )
}



#[allow(clippy::too_many_arguments)]
fn build_unit_qc_prompt_full(
    goal: &str,
    unit_scope: &str,
    expected_output: &str,
    actual_output: &str,
    trace_summary: &str,
    original_prompt: Option<&str>,
    dead_loop_section: &str,
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
         若本单元是“验证预期失败”的负例,底层 Bash 非零本身可能是通过条件;此时必须在 evidence 中说明预期性,并引用最终验收输出 EXPECTED_NEGATIVE_OK。{degradation_rule}{dead_loop_section}{QC_JSON_SHAPE_HINT}",
        clip_qc_input(actual_output),
    )
}

/// 死循环硬门提示词段:轨迹带 `dead_loop` 诊断时拼进单元 QC 提示词。
///
/// 与 `degradation_rule` 同款「条件段」写法:无诊断时空串,提示词逐字节不变。
///
/// 设计意图:让 QC 不只是看到「早终止」三个字,而是看到**形态 + 周期 + 取样**,
/// 从而有依据做根因归类;并把「换路径」从自由发挥变成有硬性要求的字段。
fn build_dead_loop_gate(diag: Option<&crate::agent::dead_loop::DeadLoopDiagnosis>) -> String {
    let Some(d) = diag else {
        return String::new();
    };
    format!(
        "\n\
         ★ 死循环硬门(最高优先级,高于上面的反降级与目标一致性门):\n\
         执行轨迹带有**进程级不可伪造**的死循环诊断 —— 对每轮 LLM 交互的输入状态指纹\n\
         (「模型已见信息集合」折叠值)与输出指纹分别取 64 位哈希写入内存索引表,对轮次键序列\n\
         做尾部周期扫描 + 长窗口低新颖度检测,命中如下:\n{detail}\n\
         判定规则:\n\
         ① 命中死循环诊断的单元**一律判 Fail**,无论终答文本写得多漂亮 —— 在原地打转的\n\
         \x20 过程中反复声明「已完成」不构成完成证据(与「跑满上限 + 零产物」同性质);\n\
         ② issues 必须归类根因,用下列**固定枚举之一**开头(供编排器程序化消费):\n\
         \x20 - `same_action`:同一动作原地打转,换个工具/参数/路径即可;\n\
         \x20 - `same_target`:目标不可达(页面/接口/权限/数据不存在),重试无用,应换目标层级;\n\
         \x20 - `context_stuck`:上下文自锁,模型在复述/重写,应压缩上下文后重新下达指令或直接收口;\n\
         \x20 - `budget_starved`:方向正确但预算被无效动作烧光,应砍步骤只做最关键的一件事;\n\
         ③ `retryable`:根因是「路径选择错误」(same_action / context_stuck / budget_starved)\n\
         \x20 时可重试,前提是 suggestion 给出**具体的换路径动作**;根因是 `same_target` 且\n\
         \x20 目标客观不可达时判 retryable=false(重试只会再烧一轮预算);\n\
         ④ `suggestion` 禁止只写「请重试」或「换个方式试试」,必须写明下一步**具体做什么**\n\
         \x20 (换哪个工具 / 换哪个路径 / 换哪一层目标 / 直接以现有信息收口)。",
        detail = d.render(),
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
        let prompt = build_unit_qc_prompt_full(
            goal,
            unit_scope,
            expected_output,
            actual_output,
            &trace_summary,
            original_prompt,
            &build_dead_loop_gate(trace.dead_loop.as_ref()),
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
            "【Quality-Check: Plan 单元】\nPlan Markdown:\n{}\n\n\
             请按 JSON 格式输出 verdict/source/issues/suggestion/retryable/evidence。\
             除结构完整性外,请核对「一、目标」段是否含探索依据(实际读过/观察过的文件、\
             页面或窗口结构要点):代码/网页/桌面操控类任务完全没有任何探索证据的凭空规划\
             属缺陷(视情节给 retryable,建议文案要求先探索后重出)。{QC_JSON_SHAPE_HINT}",
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

    // 死循环硬门(进程级事实):轨迹带 `dead_loop` 诊断时,LLM 的 pass 一律被拒。
    // 与 `early_terminate` / `high_error_rate` 同级 —— 这条门存在的理由是
    // 「循环过程中模型常会反复声明『已完成』」,语义判断很容易被文本说服,
    // 而死循环诊断是进程级机械判定,不可被覆盖。
    if trace.dead_loop.is_some() {
        let diag = trace
            .dead_loop
            .as_ref()
            .expect("刚判过 is_some")
            .headline();
        tracing::warn!(
            source = %source.as_str(),
            diagnosis = %diag,
            "Quality-Check 的 pass 结论被死循环硬门拒绝(进程级事实不可被覆盖)"
        );
        let mut gated = QualityReport::fail(
            source,
            vec![format!(
                "执行轨迹命中 LLM 交互级死循环({diag}),在原地打转的过程不构成完成证据"
            )],
            "按诊断给出的形态换路径重跑:换工具/换参数/换目标层级,或基于已获信息直接收口;\
             不要重复同一动作。",
            true,
        );
        gated.evidence = trace.render_prompt();
        return gated;
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
    // 字段形状归一化(实测 2026-10-13 闲鱼任务):模型把 notes/issues 输出成
    // 「内嵌 JSON 数组的字符串」("[\"...\", \"...\"]"),严格 Vec<String> 反序列化
    // 报 invalid type: string, expected a sequence → 整条 JSON 通道作废 →
    // fail-closed 判 Fail,一份自评 pass 的合格方案被杀、62 分钟任务零后段产出。
    // 归一化对 verdict/retryable 语义零影响,只修输入形状。
    for key in ["issues", "notes"] {
        if let Some(val) = obj.get(key) {
            match val {
                serde_json::Value::String(s) => {
                    let normalized = serde_json::from_str::<Vec<String>>(s)
                        .map(|items| {
                            serde_json::Value::Array(
                                items.into_iter().map(serde_json::Value::String).collect(),
                            )
                        })
                        .unwrap_or_else(|_| serde_json::json!([s]));
                    obj.insert(key.to_string(), normalized);
                }
                // null 对 Vec<String> 不是缺省(#[serde(default)] 只管缺失不管 null),
                // 必须移除键走缺省空数组。
                serde_json::Value::Null => {
                    obj.remove(key);
                }
                _ => {}
            }
        }
    }
    // suggestion/evidence 的反向形状问题(数组/数字/null)一并兜底:
    // 数组 join、标量转字符串、null 移除键走缺省空串。
    for key in ["suggestion", "evidence"] {
        if let Some(val) = obj.get(key) {
            match val {
                serde_json::Value::Array(_) => {
                    let joined = val
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    obj.insert(key.to_string(), serde_json::Value::String(joined));
                }
                serde_json::Value::Null => {
                    obj.remove(key);
                }
                serde_json::Value::String(_) => {}
                other => {
                    obj.insert(key.to_string(), serde_json::Value::String(other.to_string()));
                }
            }
        }
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
mod tests;
