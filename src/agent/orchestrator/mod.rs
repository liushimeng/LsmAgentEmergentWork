//! MultiAgentOrchestrator:多 Agent 架构总编排器。
//!
//! 串联 Yolo / Plan / Main-Work / SubAgent-Work / Quality-Check / SessionContext 六大角色,
//! 按用户任务的难度档位走对应链路,并在失败时逐层回流到 Yolo 重新评估。
//!
//! 设计见 `docs/多Agent架构重构/01-设计与解决方案.md` §10。

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::cancel::CancelToken;
use crate::agent::compact::CompactRunner;
use crate::agent::context::AgentRole;
use crate::agent::debug::DebugCollector;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::main_work::{self, MainWorkRunner, WorkFlowPlan, WorkFlowSpec};
use crate::agent::plan::PlanRunner;
use crate::agent::project_context;
use crate::agent::quality::{QualityReport, QualityRunner, Verdict};
use crate::agent::session_context::{
    inject_history_with_entries, SessionContextRunner, DEFAULT_HISTORY_LIMIT,
};
use crate::agent::subagent::{SubAgentRunner, SubFlowInput};
use crate::agent::web_use::WebUseRunner;
use crate::agent::window_use::WindowUseRunner;
use crate::agent::yolo::{TaskClassification, TaskLevel, YoloRunner};
use crate::config::{Db, EventType};
use crate::error::{AgentError, Result};
use crate::llm::cancellable::{CancelGate, CancellableLlmClient};
use crate::llm::Usage;
use crate::session::Session;

/// 任务阶段进度通道(D05/D07 测试轮,2026-09-10 第 23 轮):
/// 编排器在阶段边界(Yolo 分类 / Plan / Main-Work / SubAgent / QC / 失败回流)
/// 发送一行人类可读消息,由 TUI(挂起 1.5s 后打印,快速完成不打扰)或
/// CLI(stderr 立即打印)的消费端展示,解决长任务期间界面零反馈的问题。
pub type ProgressTx = tokio::sync::mpsc::UnboundedSender<String>;

/// 发送一条阶段消息(未接通道 / 接收端已退出时零开销静默)。
fn emit_progress(tx: &Option<ProgressTx>, msg: impl Into<String>) {
    if let Some(tx) = tx {
        let _ = tx.send(msg.into());
    }
}

/// 进阶段子用短文本,避免单条 stage 消息撑爆 TUI。
///
/// 2026-09-16 第 62 轮新增可变长度参数:短标题 60 字符(waiting 心跳用)、
/// 详细描述 180 字符(单元详情面板用),不再固定 180。
fn truncate_progress_text(s: &str, max_chars: usize) -> String {
    let clean = s.replace(['\n', '\r'], " ");
    if clean.chars().count() <= max_chars {
        clean
    } else {
        clean
            .chars()
            .take(max_chars.saturating_sub(1))
            .chain(['…'])
            .collect()
    }
}

/// 兼容旧调用:固定 180 字符上限。可选长度参数供 [laew] 详情面板
/// 在工具调用明细等场景下调小(避免 TUI 一行过长)。
fn truncate_progress_text_default(s: &str, max_chars: usize) -> String {
    truncate_progress_text(s, max_chars)
}

/// Orchestrator 行为参数

// ===========================================================================
// 模块拆分(2026-09-17,方案 tmpPlan/2026-09-17_01-单文件1800行超标拆分重构方案.md):
// 本文件原 2355 行超过单文件 1800 行上限,按功能域拆为:
//   types.rs        共享数据类型(配置/结果/阶段耗时/重试/分层/质检失败)
//   pipeline.rs     handle_inner 主流程 + 三档执行链路(simple/medium/hard)
//   workflows.rs    WorkFlow 分层并行执行 + run_wf_unit 执行单元
//   yolo_reflow.rs  Yolo 分类封装 + 失败回流 + 调试记录 + 兜底建议
//   usage.rs        用量累加与工具参数摘要
//   tests.rs        单元测试
// 公有项经下方 `pub use` 再导出,`crate::agent::orchestrator::Xxx` 路径保持不变;
// 原私有项标 `pub(super)`(可见域 = orchestrator 子树,与拆分前单文件作用域等价)。
// ===========================================================================
mod pipeline;
mod types;
mod usage;
mod workflows;
mod yolo_reflow;

pub use types::{
    LayerInfo, OrchestrationOutcome, OrchestratorConfig, RetryRecord, StageDuration, TaskResult,
    WorkflowResult,
};
use types::{default_wf_exec_role, QualityFailure};
use usage::{add_usage, failure_usage, tool_args_digest};
use workflows::{build_subflow_input, run_wf_unit};
use yolo_reflow::{fallback_suggestion, is_placeholder_direct_answer};

#[cfg(test)]
mod tests;

/// 多 Agent 编排器
pub struct MultiAgentOrchestrator {
    yolo: YoloRunner,
    plan: PlanRunner,
    main_work: MainWorkRunner,
    /// Arc 化:同层 WorkFlow 并行时共享给 tokio::spawn 任务
    sub_agent: Arc<SubAgentRunner>,
    /// Arc 化:桌面窗口操控专项执行单元(delegate_to=windowuse 的 WorkFlow 路由至此)
    window_use: Arc<WindowUseRunner>,
    /// Arc 化:浏览器网页操控专项执行单元(delegate_to=webuse 的 WorkFlow 路由至此,第 11 角色)
    web_use: Arc<WebUseRunner>,
    /// Arc 化:同上(质检随执行单元并行)
    quality: Arc<QualityRunner>,
    session_context: SessionContextRunner,
    compact: CompactRunner,
    db: Arc<Db>,
    cfg: OrchestratorConfig,
    /// 每任务取消门:8 个角色的 LLM 客户端被 `CancellableLlmClient` 统一包裹,
    /// `handle_cancellable` 开始时注入 token、结束时(Drop guard)清除。
    cancel_gate: Arc<CancelGate>,
}

impl MultiAgentOrchestrator {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>, plans_dir: PathBuf) -> Self {
        Self::with_config(llm, db, plans_dir, OrchestratorConfig::default())
    }

    pub fn with_config(
        llm: Arc<dyn crate::llm::LlmClient>,
        db: Arc<Db>,
        plans_dir: PathBuf,
        cfg: OrchestratorConfig,
    ) -> Self {
        // 取消传播:在最外层包裹可取消装饰器(顺序 cancellable > debug > resilient > raw),
        // 全部角色的 LLM 调用一处获得中断能力(方案 §3.3)
        let cancel_gate = CancelGate::new();
        let llm: Arc<dyn crate::llm::LlmClient> =
            Arc::new(CancellableLlmClient::new(llm, cancel_gate.clone()));
        let yolo = YoloRunner::new(llm.clone());
        let plan = PlanRunner::new(llm.clone(), db.clone(), plans_dir);
        let main_work = MainWorkRunner::new(llm.clone(), db.clone());
        let sub_agent = Arc::new(
            SubAgentRunner::new(llm.clone(), db.clone())
                .with_max_iterations(cfg.subagent_max_iterations),
        );
        let window_use = Arc::new(
            WindowUseRunner::new(llm.clone(), db.clone())
                .with_max_iterations(cfg.subagent_max_iterations),
        );
        let web_use = Arc::new(
            WebUseRunner::new(llm.clone(), db.clone())
                .with_max_iterations(cfg.subagent_max_iterations),
        );
        let quality = Arc::new(QualityRunner::new(llm.clone(), db.clone()));
        let session_context = SessionContextRunner::new(llm.clone(), db.clone());
        let compact = CompactRunner::new(llm, db.clone());
        Self {
            yolo,
            plan,
            main_work,
            sub_agent,
            window_use,
            web_use,
            quality,
            session_context,
            compact,
            db,
            cfg,
            cancel_gate,
        }
    }

    /// 处理一次用户输入(不可取消;既有调用方 / 测试零改动)。
    ///
    /// 内部用一个永不触发的全新 token 走 [`handle_cancellable`],语义等价:
    /// fresh token 的 `cancelled()` 永不 resolve,LLM 装饰器等价直通。
    pub async fn handle(&self, session: &mut Session) -> Result<OrchestrationOutcome> {
        let never_fires = CancelToken::new();
        self.handle_cancellable(session, &never_fires).await
    }

    /// [`handle`] 的可取消版本(H9 取消传播,第 07 轮):
    /// - LLM 层:token 经 `cancel_gate` 注入装饰器,所有角色调用可即时中断;
    /// - 编排层:各阶段边界检查,命中即返回 `Err(AgentError::Cancelled)`,
    ///   **不进 QualityFailure / Yolo 失败回流**(取消不是失败,不能重试);
    /// - 执行层:token 传入每个 SubAgent 单元(含同层并行 spawn)。
    pub async fn handle_cancellable(
        &self,
        session: &mut Session,
        cancel: &CancelToken,
    ) -> Result<OrchestrationOutcome> {
        self.handle_cancellable_with_progress(session, cancel, None)
            .await
    }

    /// [`handle_cancellable`] 的带进度版本(2026-09-10 第 23 轮):
    /// `progress` 接收端由调用方(TUI 打印协程 / CLI stderr 打印)持有,
    /// 通道 clone 会贯穿到并行执行的 WorkFlow 单元,任务返回时全部 drop,
    /// 接收端 `recv()` 返回 None 即为「任务结束」信号。
    pub async fn handle_cancellable_with_progress(
        &self,
        session: &mut Session,
        cancel: &CancelToken,
        progress: Option<ProgressTx>,
    ) -> Result<OrchestrationOutcome> {
        let _gate_guard = self.cancel_gate.guard(cancel.clone());
        self.handle_inner(session, cancel, &progress).await
    }
    /// 暴露 Yolo runner(测试用)
    pub fn yolo(&self) -> &YoloRunner {
        &self.yolo
    }
    /// 暴露 Quality runner(测试用)
    pub fn quality(&self) -> &QualityRunner {
        &self.quality
    }
    /// 暴露 SessionContext runner(测试用)
    pub fn session_context(&self) -> &SessionContextRunner {
        &self.session_context
    }
    /// 暴露 SubAgent runner(测试用)
    pub fn sub_agent(&self) -> &SubAgentRunner {
        &self.sub_agent
    }
    /// 暴露 Plan runner(测试用)
    pub fn plan(&self) -> &PlanRunner {
        &self.plan
    }
    /// 暴露 Main-Work runner(测试用)
    pub fn main_work(&self) -> &MainWorkRunner {
        &self.main_work
    }
}

// 解决未使用警告:导入但仅在 cfg(test) 用
#[allow(unused_imports)]
use crate::agent::session_context::SessionSummary as _SessionSummary;
#[allow(unused_imports)]
use crate::agent::subagent::SubFlowOutcome as _SubFlowOutcome;
