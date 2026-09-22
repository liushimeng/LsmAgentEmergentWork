//! 动态子 Agent 运行时:治理(深度/预算/并发)+ 作用域 + 组装 + 运行 + 回收。
//!
//! 第 114 轮(2026-09-22)新增,设计见
//! `docs/自感知SubAgent动态启动/01-设计与解决方案.md` §5.3。
//!
//! 关键机制:
//! - **作用域**:`tokio::task_local!` 承载 [`SubAgentRuntime`]。工具对象是共享的
//!   (并行 WorkFlow 单元共享 `Arc<SubAgentRunner>`),因此运行时**不能**挂在工具实例上;
//!   task-local 随任务隔离,天然支持并行单元各自一份运行时。
//! - **继承**:子 Agent 的 `Agent::run_session_inner` 再次进入本模块时命中父作用域,
//!   `derive_child` 共享同一 [`Governor`](会话级并发/预算/台账)并把 depth + 1。
//! - **零开销**:未开启功能 / 非委派角色 → [`runtime_for`] 返回 `None`,链路原样直通。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::agent::cancel::CancelToken;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::custom_agents::{self as ca, ResolvedAgentType};
use crate::agent::self_awareness::{
    self as sa, SelfAwarenessConfig, SpawnPolicy, BATCH_MERGED_CHARS, CHILD_REPORT_CHARS,
};
use crate::agent::tools::{builtin_registry_with_work_dir, subagent::SubAgentTool};
use crate::agent::{Agent, AgentProfile};
use crate::config::{Db, Paths, RunQuery, SubAgentRunEntry, SubAgentRunRow};
use crate::error::AgentError;
use crate::llm::{ChatMessage, LlmClient, Usage};
use crate::session::Session;

/// 事件环形缓冲上限。
const EVENT_RING_CAP: usize = 64;
/// 单个子 Agent 等待并发槽位的最长时间(防嵌套死锁:超时返回 2003 而非永久等待)。
const PERMIT_WAIT_SECS: u64 = 60;

/// 进程级 run_id 全局序号(见 [`Governor::next_run_id`] 的唯一性说明)。
static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

// ===========================================================================
// 错误
// ===========================================================================

/// 动态启动失败(带统一 JSON 信封 code)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    /// 当前上下文不支持动态启动(无运行时 / 功能关闭)。
    Unavailable,
    /// 参数非法。
    InvalidArgs(String),
    /// 会话级预算耗尽。
    BudgetExhausted { used: usize, max: usize },
    /// 深度超限。
    DepthExceeded { depth: usize, max: usize },
    /// 并发槽位等待超时。
    ConcurrencyTimeout { waited_secs: u64 },
    /// run_id 不存在。
    UnknownRunId(String),
    /// 已取消。
    Cancelled,
}

impl SpawnError {
    /// 统一信封 code(对齐 MCP_Window_Use / MCP_Web_Use 风格)。
    pub fn code(&self) -> u16 {
        match self {
            Self::InvalidArgs(_) => 1001,
            Self::BudgetExhausted { .. } => 2001,
            Self::DepthExceeded { .. } => 2002,
            Self::ConcurrencyTimeout { .. } => 2003,
            Self::Unavailable => 4001,
            Self::Cancelled => 4002,
            Self::UnknownRunId(_) => 4003,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Unavailable => "当前上下文不支持动态启动子 Agent(未开启或不在可委派角色内);\
                 请直接用现有工具完成任务,不要重试本工具。"
                .to_string(),
            Self::InvalidArgs(d) => format!("参数非法:{d}"),
            Self::BudgetExhausted { used, max } => format!(
                "本会话子 Agent 预算已耗尽(已启动 {used} / 上限 {max});请停止委派,由你自己完成剩余工作。"
            ),
            Self::DepthExceeded { depth, max } => format!(
                "不允许继续嵌套(当前深度 {depth} / 上限 {max});你已处于叶子层级,请直接完成任务。"
            ),
            Self::ConcurrencyTimeout { waited_secs } => format!(
                "等待并发槽位 {waited_secs} 秒仍未获得(并发上限已排满);请减少并行数或稍后重试。"
            ),
            Self::UnknownRunId(id) => format!("run_id `{id}` 不存在;可用 action=\"list\" 查看有效句柄。"),
            Self::Cancelled => "任务已取消。".to_string(),
        }
    }

    /// 统一 JSON 信封。
    pub fn envelope(&self) -> String {
        json!({
            "code": self.code(),
            "message": self.message(),
            "data": Value::Null,
        })
        .to_string()
    }
}

// ===========================================================================
// 请求 / 报告
// ===========================================================================

/// `resume` 的前序上下文(血缘续跑的种子)。
///
/// 第 115 轮:语义是「带着前序任务与结论**重新起一个**子 Agent」,不是恢复被冻结的
/// future(工具执行中途状态不可序列化);血缘写入 `subagent_run.resumed_from`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PriorRun {
    pub run_id: String,
    pub agent_type: String,
    pub status: String,
    pub created_at: String,
    pub task: String,
    pub report: String,
}

/// 前序任务 / 结论在种子提示词里的截断上限(字符)。
pub const PRIOR_TASK_CHARS: usize = 2000;
/// 前序结论截断上限(字符)。
pub const PRIOR_REPORT_CHARS: usize = 8000;

/// 一次子 Agent 启动请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentRequest {
    /// 任务描述(必须自包含)。
    pub task: String,
    /// 子 Agent 类型(内置 6 类之一,或 `.laew/agents/*.md` 自定义类型)。
    ///
    /// 第 115 轮从 `SubAgentType` 抬升为 [`ResolvedAgentType`];序列化为类型 id 字符串。
    pub agent_type: ResolvedAgentType,
    /// 可选名称(默认由类型 + 序号派生)。
    #[serde(default)]
    pub name: Option<String>,
    /// 追加到子 Agent 系统提示词的额外指令。
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// 限定工具名(只能收窄)。
    #[serde(default)]
    pub tools: Vec<String>,
    /// 期望产出。
    #[serde(default)]
    pub expected_output: String,
    /// per-子 Agent 迭代上限(4..=32)。
    #[serde(default)]
    pub max_iterations: Option<usize>,
    /// 续跑种子(仅 `resume` 设置)。
    #[serde(default)]
    pub prior: Option<PriorRun>,
}

/// 子 Agent 运行报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentReport {
    pub run_id: String,
    pub name: String,
    pub agent_type: String,
    /// `ok` / `failed` / `timeout` / `cancelled`
    pub status: String,
    /// 子 Agent 最终回答(可能被截断)。
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 实际生效的工具面。
    #[serde(default)]
    pub tools: Vec<String>,
    /// 因父策略 / 平台不可用被剔除的请求工具。
    #[serde(default)]
    pub dropped_tools: Vec<String>,
    #[serde(default)]
    pub iterations: usize,
    #[serde(default)]
    pub tool_calls: usize,
    #[serde(default)]
    pub wallclock_ms: u64,
    #[serde(default)]
    pub usage: UsageSnapshot,
    /// 启动来源:`launch` / `background` / `batch` / `resume`(第 115 轮)。
    #[serde(default)]
    pub origin: String,
    /// 血缘:续跑自哪个 `run_id`(第 115 轮)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
}

/// 用量快照(序列化用;`Usage` 本体不带 Serialize 派生)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
}

impl UsageSnapshot {
    pub fn from_usage(u: &Usage) -> Self {
        Self {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_read_input_tokens: u.cache_read_input_tokens,
            cache_creation_input_tokens: u.cache_creation_input_tokens,
        }
    }
}

/// 子 Agent 生命周期事件(环形缓冲 / TUI `/agents` 用)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentEvent {
    pub run_id: String,
    pub name: String,
    pub agent_type: String,
    pub parent: String,
    pub depth: usize,
    pub status: String,
    pub task_digest: String,
    pub wallclock_ms: u64,
    pub tool_calls: usize,
    pub usage: UsageSnapshot,
    /// 启动来源(第 115 轮:`launch` / `background` / `batch` / `resume`)。
    #[serde(default)]
    pub origin: String,
}

// ===========================================================================
// Governor(会话级治理器)
// ===========================================================================

/// 运行中作业的可读视图。
#[derive(Debug, Clone)]
struct JobView {
    run_id: String,
    name: String,
    agent_type: String,
    started: Instant,
}

/// 作业状态槽。
#[derive(Debug)]
enum JobState {
    Running,
    Done(Box<SubAgentReport>),
}

struct JobEntry {
    view: JobView,
    state: Arc<Mutex<JobState>>,
    cancel: Option<CancelToken>,
    #[allow(dead_code)]
    handle: Option<tokio::task::JoinHandle<()>>,
}

/// 会话级治理器:并发 + 预算 + 台账 + 作业表 + 事件环。
pub struct Governor {
    cfg: SelfAwarenessConfig,
    /// 并发槽位(对齐 atomcode `Semaphore(3)`)。
    pub(crate) sem: Arc<tokio::sync::Semaphore>,
    /// 已消耗预算。
    used: Mutex<usize>,
    /// 动态子 Agent 用量台账(任务边界 drain 计入总用量)。
    usage: Mutex<Usage>,
    /// 作业表(run_id → 句柄)。
    jobs: Mutex<HashMap<String, JobEntry>>,
    /// 事件环形缓冲。
    events: Mutex<VecDeque<SubAgentEvent>>,
    /// run_id 序号。
    seq: AtomicU64,
    /// 运行记录落库句柄(第 115 轮;惰性打开,失败 = `None` 且只 warn 一次)。
    db: OnceLock<Option<Arc<Db>>>,
}

static GOVERNORS: OnceLock<Mutex<HashMap<String, Arc<Governor>>>> = OnceLock::new();

fn governor_table() -> &'static Mutex<HashMap<String, Arc<Governor>>> {
    GOVERNORS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 取(或建)某会话的治理器。同一 TUI Session 内跨任务复用 → 预算与并发全局生效。
pub fn governor_for(session_id: &str, cfg: &SelfAwarenessConfig) -> Arc<Governor> {
    let mut table = governor_table().lock().expect("Governor table poisoned");
    if let Some(g) = table.get(session_id) {
        return g.clone();
    }
    let g = Arc::new(Governor {
        cfg: cfg.clone(),
        sem: Arc::new(tokio::sync::Semaphore::new(cfg.max_parallel.max(1))),
        used: Mutex::new(0),
        usage: Mutex::new(Usage::default()),
        jobs: Mutex::new(HashMap::new()),
        events: Mutex::new(VecDeque::new()),
        seq: AtomicU64::new(0),
        db: OnceLock::new(),
    });
    table.insert(session_id.to_string(), g.clone());
    g
}

/// 清空全部治理器(单测隔离用;生产链路不调用)。
pub fn reset_governors_for_test() {
    governor_table()
        .lock()
        .expect("Governor table poisoned")
        .clear();
}

/// 任务边界排空用量台账(谁先取谁得,取完清零;多次调用不会重复计数)。
pub fn drain_usage(session_id: &str) -> Usage {
    let g = {
        let table = governor_table().lock().expect("Governor table poisoned");
        table.get(session_id).cloned()
    };
    match g {
        Some(g) => {
            let mut u = g.usage.lock().expect("usage poisoned");
            std::mem::take(&mut *u)
        }
        None => Usage::default(),
    }
}

/// 最近事件(TUI `/agents` / `list` 用)。
pub fn recent_events(session_id: &str, n: usize) -> Vec<SubAgentEvent> {
    let g = {
        let table = governor_table().lock().expect("Governor table poisoned");
        table.get(session_id).cloned()
    };
    match g {
        Some(g) => {
            let e = g.events.lock().expect("events poisoned");
            e.iter().rev().take(n).cloned().collect::<Vec<_>>().into_iter().rev().collect()
        }
        None => Vec::new(),
    }
}

/// 只读视图:`(已用预算, 预算上限, 运行中, 并发上限)`。
///
/// 与 [`governor_for`] 不同,**不会**创建治理器(未发生过动态启动 → `None`),
/// 供 TUI `/agents` 之类的只读展示使用。
pub fn governor_view(session_id: &str) -> Option<(usize, usize, usize, usize)> {
    let g = {
        let table = governor_table().lock().expect("Governor table poisoned");
        table.get(session_id).cloned()
    }?;
    Some((g.used(), g.cfg.max_total, g.in_flight(), g.cfg.max_parallel))
}

// ===========================================================================
// 持久化(第 115 轮,2026-09-22)
// ===========================================================================

/// 落库句柄(进程级缓存)。
///
/// `Paths::detect()` + `Db::open()` 带完整性检测,代价不低,但每个进程只需一次;
/// 打开失败缓存 `None` 并 warn 一次 —— 持久化是**增强能力**,绝不能让任务失败。
fn persist_db() -> Option<Arc<Db>> {
    // 单测默认**不落盘**(否则会在 target/ 下建库):需要验证持久化的测试用
    // [`set_test_db`] 显式注入临时库;`set_test_db(None)` 则模拟「数据库不可用」。
    #[cfg(test)]
    {
        return test_db_override().flatten();
    }
    #[allow(unreachable_code)]
    if let Some(over) = test_db_override() {
        return over;
    }
    static CELL: OnceLock<Option<Arc<Db>>> = OnceLock::new();
    CELL.get_or_init(|| match Paths::detect() {
        Ok(paths) => match Db::open(&paths) {
            Ok(db) => Some(Arc::new(db)),
            Err(e) => {
                warn!(error = %e, "子 Agent 运行记录持久化不可用(打开数据库失败),后续不再尝试");
                None
            }
        },
        Err(e) => {
            warn!(error = %e, "子 Agent 运行记录持久化不可用(根目录解析失败)");
            None
        }
    })
    .clone()
}

/// 测试用落库句柄覆盖(`Some(None)` = 显式模拟「数据库不可用」)。
#[cfg(test)]
static TEST_DB_OVERRIDE: OnceLock<Mutex<Option<Option<Arc<Db>>>>> = OnceLock::new();

/// 读取测试覆盖;未设置返回 `None`(生产路径恒为 `None`,零开销)。
fn test_db_override() -> Option<Option<Arc<Db>>> {
    #[cfg(test)]
    {
        let cell = TEST_DB_OVERRIDE.get_or_init(|| Mutex::new(None));
        return cell.lock().expect("db override poisoned").clone();
    }
    #[allow(unreachable_code)]
    None
}

/// 注入测试用数据库(单测专用;`None` = 模拟数据库不可用)。
#[cfg(test)]
pub fn set_test_db(db: Option<Arc<Db>>) {
    let cell = TEST_DB_OVERRIDE.get_or_init(|| Mutex::new(None));
    *cell.lock().expect("db override poisoned") = Some(db);
}

/// 清空测试覆盖(恢复「按真实根目录打开」语义)。
#[cfg(test)]
pub fn clear_test_db() {
    let cell = TEST_DB_OVERRIDE.get_or_init(|| Mutex::new(None));
    *cell.lock().expect("db override poisoned") = None;
}

/// 会话启动期的持久化维护:孤儿标记 + 自动 trim(失败 fail-open)。
///
/// 设计见 `docs/自感知SubAgent自定义类型与运行持久化/01-设计与解决方案.md` §5.3。
pub fn maintain_run_store(current_session: &str, cfg: &SelfAwarenessConfig) {
    if !cfg.enabled || !cfg.persist {
        return;
    }
    let Some(db) = persist_db() else {
        return;
    };
    match db.mark_subagent_orphans(current_session) {
        Ok(n) if n > 0 => info!(orphaned = n, "启动期标记残留子 Agent 作业为 orphaned"),
        Ok(_) => {}
        Err(e) => warn!(error = %e, "孤儿作业标记失败(忽略)"),
    }
    match db.trim_subagent_runs(cfg.run_keep) {
        Ok(n) if n > 0 => info!(trimmed = n, "子 Agent 运行记录自动清理"),
        Ok(_) => {}
        Err(e) => warn!(error = %e, "运行记录 trim 失败(忽略)"),
    }
}

/// 查询运行记录(`action="history"` 与 TUI `/agents history` 共用)。
///
/// 返回 `None` 表示持久化不可用(未开启 / 数据库打不开)。
pub fn query_runs(
    session_id: Option<&str>,
    agent_type: Option<&str>,
    limit: usize,
) -> Option<Vec<SubAgentRunRow>> {
    if !sa::config().persist {
        return None;
    }
    let db = persist_db()?;
    let q = RunQuery {
        session_id: session_id.map(str::to_string),
        agent_type: agent_type.map(str::to_string),
        limit,
    };
    match db.list_subagent_runs(&q) {
        Ok(rows) => Some(rows),
        Err(e) => {
            warn!(error = %e, "子 Agent 运行记录查询失败");
            None
        }
    }
}

/// 取单条运行记录(供 `resume` 解析前序)。
pub fn get_run(run_id: &str) -> Option<SubAgentRunRow> {
    if !sa::config().persist {
        return None;
    }
    let db = persist_db()?;
    match db.get_subagent_run(run_id) {
        Ok(row) => row,
        Err(e) => {
            warn!(error = %e, run_id = %run_id, "子 Agent 运行记录读取失败");
            None
        }
    }
}

/// 统计孤儿作业数(面板展示)。
pub fn count_orphans(session_id: Option<&str>) -> usize {
    if !sa::config().persist {
        return 0;
    }
    persist_db()
        .and_then(|db| db.count_subagent_orphans(session_id).ok())
        .unwrap_or(0)
}

/// 当前配置(只读展示用)。
pub fn config() -> &'static SelfAwarenessConfig {
    crate::agent::self_awareness::config()
}

impl Governor {
    pub fn cfg(&self) -> &SelfAwarenessConfig {
        &self.cfg
    }

    pub fn used(&self) -> usize {
        *self.used.lock().expect("budget poisoned")
    }

    pub fn remaining(&self) -> usize {
        self.cfg.max_total.saturating_sub(self.used())
    }

    pub fn in_flight(&self) -> usize {
        self.jobs
            .lock()
            .expect("jobs poisoned")
            .values()
            .filter(|j| matches!(*j.state.lock().expect("job state poisoned"), JobState::Running))
            .count()
    }

    /// 预扣 n 个预算(全有或全无;失败不改动计数)。
    fn try_charge(&self, n: usize) -> bool {
        let mut used = self.used.lock().expect("budget poisoned");
        if used.saturating_add(n) > self.cfg.max_total {
            return false;
        }
        *used += n;
        true
    }

    /// 归还预算(批量启动中途失败时回滚)。
    fn refund(&self, n: usize) {
        let mut used = self.used.lock().expect("budget poisoned");
        *used = used.saturating_sub(n);
    }

    fn next_run_id(&self) -> String {
        // 会话内序号(事件/展示顺序) + **进程级全局序号**(run_id 唯一性)。
        //
        // 2026-09-22 第 115 轮修复:D114 只用「会话内 seq + pid」,而 `/new` `/clear`
        // 会产生新 Session(= 新 Governor,seq 从 1 重来)—— 同一进程内两个会话会
        // 生成**相同的 run_id**。内存作业表各存各的看不出来,但落库后主键冲突会
        // 让后写覆盖先写(实测:并行/跨会话测试互相覆盖)。改为全局自增后,
        // 「同进程唯一 + 跨进程靠 pid 区分」成立。
        let n = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let g = RUN_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
        let pid = std::process::id();
        format!("sa-{pid:x}{g:04x}-{n:x}")
    }

    fn record_usage(&self, u: &Usage) {
        let mut total = self.usage.lock().expect("usage poisoned");
        *total = total.merge(*u);
    }

    fn push_event(&self, ev: SubAgentEvent) {
        let mut ring = self.events.lock().expect("events poisoned");
        if ring.len() >= EVENT_RING_CAP {
            ring.pop_front();
        }
        ring.push_back(ev);
    }

    fn running_views(&self) -> Vec<JobView> {
        let jobs = self.jobs.lock().expect("jobs poisoned");
        let mut v: Vec<JobView> = jobs
            .values()
            .filter(|j| matches!(*j.state.lock().expect("job state poisoned"), JobState::Running))
            .map(|j| j.view.clone())
            .collect();
        v.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        v
    }
}

// ===========================================================================
// 运行时(作用域)
// ===========================================================================

tokio::task_local! {
    /// 当前任务所属的 Agent 运行时(由 `Agent::run_session_inner` 建立)。
    static CURRENT_RUNTIME: Arc<SubAgentRuntime>;
}

/// 取当前作用域运行时(无 → None;不 panic)。
pub fn current() -> Option<Arc<SubAgentRuntime>> {
    CURRENT_RUNTIME.try_with(|r| r.clone()).ok()
}

/// 在作用域内运行 future(供 `Agent::run_session_inner` 与子 Agent 组装使用)。
pub async fn scope<F>(rt: Arc<SubAgentRuntime>, f: F) -> F::Output
where
    F: std::future::Future,
{
    CURRENT_RUNTIME.scope(rt, f).await
}

/// 单个 Agent 实例的动态启动运行时。
pub struct SubAgentRuntime {
    llm: Arc<dyn LlmClient>,
    /// 发起方 Agent 名(父)。
    parent_name: String,
    /// 发起方工具面(自感知渲染 + 收窄参照)。
    parent_tools: Vec<String>,
    /// 父能力策略。
    policy: SpawnPolicy,
    cfg: SelfAwarenessConfig,
    governor: Arc<Governor>,
    /// 0 = 顶层 Agent;每深入一层 +1。
    depth: usize,
    work_dir: PathBuf,
    cancel: Option<CancelToken>,
    session_id: String,
}

/// 为一次 Agent 运行构造运行时(顶层新建 / 子 Agent 继承)。
///
/// 返回 `None` 表示本次运行不参与动态启动(不注册工具的角色、功能关闭、
/// 或注册表里没有 `SubAgent` 工具) —— 调用方原样直通,零开销。
pub fn runtime_for(
    agent_name: &str,
    profile: &AgentProfile,
    llm: Arc<dyn LlmClient>,
    session_id: &str,
    cancel: Option<&CancelToken>,
) -> Option<Arc<SubAgentRuntime>> {
    let tool_registered = profile.tools.get(SUBAGENT_TOOL_NAME).is_ok();
    let child_capable = profile.spawn_policy != SpawnPolicy::Disabled;
    // ① 子 Agent 由 `run_child` 显式建好作用域(名字 = 作用域 parent_name):
    //    直接复用,避免 depth 被二次 +1(否则 max_depth=2 时子 Agent 会失去委派能力)。
    if let Some(owner) = current() {
        if owner.parent_name == agent_name {
            return Some(owner);
        }
    }
    // ② 命中他人作用域(带外派生):总继承,共享治理器,只把 depth + 1。
    if let Some(parent) = current() {
        return Some(Arc::new(SubAgentRuntime {
            llm,
            parent_name: agent_name.to_string(),
            parent_tools: profile.tools.names().iter().map(|s| s.to_string()).collect(),
            policy: profile.spawn_policy,
            cfg: parent.cfg.clone(),
            governor: parent.governor.clone(),
            depth: parent.depth + 1,
            work_dir: parent.work_dir.clone(),
            cancel: parent.cancel.clone(),
            session_id: session_id.to_string(),
        }));
    }
    if !child_capable || !tool_registered {
        return None;
    }
    let cfg = sa::config().clone();
    if !cfg.enabled || cfg.max_depth == 0 {
        return None;
    }
    Some(Arc::new(SubAgentRuntime {
        llm,
        parent_name: agent_name.to_string(),
        parent_tools: profile.tools.names().iter().map(|s| s.to_string()).collect(),
        policy: profile.spawn_policy,
        governor: governor_for(session_id, &cfg),
        cfg,
        depth: 0,
        work_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        cancel: cancel.cloned(),
        session_id: session_id.to_string(),
    }))
}

/// `SubAgent` 工具名(与 `tools::subagent::SUBAGENT_TOOL` 一致;此处避免循环 use)。
const SUBAGENT_TOOL_NAME: &str = "SubAgent";

impl SubAgentRuntime {
    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 本次运行的工作目录(自定义 Agent 定义文件的发现基准;Bash/Read/Write 同基准)。
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// 本运行可见的自定义子 Agent 类型定义(按 `work_dir` 发现,定义即生效)。
    pub fn defs(&self) -> Vec<ca::AgentDef> {
        ca::discover_in(&self.work_dir).defs
    }

    pub fn can_spawn(&self) -> bool {
        self.cfg.can_spawn_at(self.depth)
    }

    pub fn remaining(&self) -> usize {
        self.governor.remaining()
    }

    /// `action="list"` 的运行时快照:自感知的动态部分。
    pub fn snapshot_json(&self) -> Value {
        // 第 115 轮:名册 = 内置 + 用户/项目 `.laew/agents/*.md` 自定义类型;
        // 被策略隐藏的自定义类型单列 skipped(可解释性:用户知道自己的定义为何不可用)
        let disco = ca::discover_in(&self.work_dir);
        let view = sa::roster_view(&disco.defs, self.policy);
        let roster: Vec<Value> = view
            .entries
            .into_iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "label": r.label,
                    "desc": r.desc,
                    "tools": r.tools,
                    "custom": r.custom,
                    "source": r.source,
                })
            })
            .collect();
        let mut skipped: Vec<Value> = view
            .skipped
            .into_iter()
            .map(|s| json!({"id": s.id, "reason": s.reason, "source": s.source}))
            .collect();
        for ig in &disco.ignored {
            skipped.push(json!({
                "id": ig.id,
                "reason": ig.reason,
                "source": ig.source,
            }));
        }
        let running: Vec<Value> = self
            .governor
            .running_views()
            .into_iter()
            .map(|v| {
                json!({
                    "run_id": v.run_id,
                    "name": v.name,
                    "agent_type": v.agent_type,
                    "elapsed_ms": v.started.elapsed().as_millis() as u64,
                })
            })
            .collect();
        let recent: Vec<Value> = recent_events(&self.session_id, 5)
            .into_iter()
            .map(|e| {
                json!({
                    "run_id": e.run_id,
                    "name": e.name,
                    "agent_type": e.agent_type,
                    "status": e.status,
                    "wallclock_ms": e.wallclock_ms,
                    "tool_calls": e.tool_calls,
                    "usage": e.usage,
                })
            })
            .collect();
        json!({
            "agent": {
                "name": self.parent_name,
                "role_label": sa::role_label_for(&self.parent_name),
                "tools": self.parent_tools,
                "spawn_policy": self.policy.as_str(),
                "depth": self.depth,
                "max_depth": self.cfg.max_depth,
                "can_spawn": self.can_spawn(),
            },
            "limits": {
                "max_total": self.cfg.max_total,
                "used": self.governor.used(),
                "remaining": self.governor.remaining(),
                "max_parallel": self.cfg.max_parallel,
                "in_flight": self.governor.in_flight(),
                "max_iterations": self.cfg.max_iterations,
                "timeout_secs": self.cfg.timeout_secs,
                "persist": self.cfg.persist,
            },
            "roster": roster,
            "custom_skipped": skipped,
            "running": running,
            "recent": recent,
            "session_id": self.session_id,
            "work_dir": self.work_dir.display().to_string(),
        })
    }

    /// 前台启动单个子 Agent(治理校验 → 组装 → 运行 → 回收)。
    pub async fn launch(&self, req: SubAgentRequest) -> Result<SubAgentReport, SpawnError> {
        self.launch_with_origin(req, "launch").await
    }

    /// 前台启动(可指定 `origin`:第 115 轮 `resume` 用它记录血缘来源)。
    pub async fn launch_with_origin(
        &self,
        req: SubAgentRequest,
        origin: &str,
    ) -> Result<SubAgentReport, SpawnError> {
        self.check_can_spawn()?;
        if !self.governor.try_charge(1) {
            return Err(SpawnError::BudgetExhausted {
                used: self.governor.used(),
                max: self.cfg.max_total,
            });
        }
        let permit = match self.acquire_permit().await {
            Ok(p) => p,
            Err(e) => {
                self.governor.refund(1);
                return Err(e);
            }
        };
        let run_id = self.governor.next_run_id();
        let report = self.run_child(&req, &run_id, origin).await;
        drop(permit);
        Ok(report)
    }

    /// 后台启动:立即返回 `run_id`,由 `action="result"` 取回报告。
    pub fn launch_background(self: &Arc<Self>, req: SubAgentRequest) -> Result<Value, SpawnError> {
        self.check_can_spawn()?;
        if !self.governor.try_charge(1) {
            return Err(SpawnError::BudgetExhausted {
                used: self.governor.used(),
                max: self.cfg.max_total,
            });
        }
        let run_id = self.governor.next_run_id();
        let name = req.name.clone().unwrap_or_else(|| req.agent_type.id().to_string());
        let state = Arc::new(Mutex::new(JobState::Running));
        let child_token = self.cancel.as_ref().map(|t| t.child_token());
        let rt = self.clone_arc();
        let req2 = req.clone();
        let run_id2 = run_id.clone();
        let state2 = state.clone();
        let handle = tokio::spawn(async move {
            let report = match rt.acquire_permit().await {
                Ok(permit) => {
                    let r = rt.run_child(&req2, &run_id2, "background").await;
                    drop(permit);
                    r
                }
                Err(e) => SubAgentReport {
                    run_id: run_id2.clone(),
                    name: req2
                        .name
                        .clone()
                        .unwrap_or_else(|| req2.agent_type.id().to_string()),
                    agent_type: req2.agent_type.id().to_string(),
                    status: "failed".into(),
                    text: String::new(),
                    error: Some(e.message()),
                    tools: Vec::new(),
                    dropped_tools: Vec::new(),
                    iterations: 0,
                    tool_calls: 0,
                    wallclock_ms: 0,
                    usage: UsageSnapshot::default(),
                    origin: "background".into(),
                    resumed_from: req2.prior.as_ref().map(|p| p.run_id.clone()),
                },
            };
            *state2.lock().expect("job state poisoned") = JobState::Done(Box::new(report));
        });
        self.governor.jobs.lock().expect("jobs poisoned").insert(
            run_id.clone(),
            JobEntry {
                view: JobView {
                    run_id: run_id.clone(),
                    name: name.clone(),
                    agent_type: req.agent_type.id().to_string(),
                    started: Instant::now(),
                },
                state,
                cancel: child_token,
                handle: Some(handle),
            },
        );
        Ok(json!({
            "run_id": run_id,
            "name": name,
            "agent_type": req.agent_type.id(),
            "status": "running",
            "hint": "已后台启动;稍后用 action=\"result\", run_id=... 取回结果。",
        }))
    }

    /// 批量并行启动(预算全有或全无预扣;`buffer_unordered` 控制并发)。
    pub async fn batch(
        self: &Arc<Self>,
        reqs: Vec<SubAgentRequest>,
        max_concurrency: Option<usize>,
    ) -> Result<Vec<SubAgentReport>, SpawnError> {
        if reqs.is_empty() {
            return Err(SpawnError::InvalidArgs("tasks 不能为空".into()));
        }
        if reqs.len() > sa::MAX_BATCH_TASKS {
            return Err(SpawnError::InvalidArgs(format!(
                "单次 batch 最多 {} 个子任务(收到 {})",
                sa::MAX_BATCH_TASKS,
                reqs.len()
            )));
        }
        self.check_can_spawn()?;
        if !self.governor.try_charge(reqs.len()) {
            return Err(SpawnError::BudgetExhausted {
                used: self.governor.used(),
                max: self.cfg.max_total,
            });
        }
        let conc = max_concurrency
            .unwrap_or(self.cfg.max_parallel)
            .clamp(1, self.cfg.max_parallel.max(1));
        use futures::stream::StreamExt;
        let rt = self.clone_arc();
        let reports: Vec<SubAgentReport> = futures::stream::iter(reqs.into_iter().map(move |req| {
            let rt = rt.clone();
            async move {
                let permit = match rt.acquire_permit().await {
                    Ok(p) => p,
                    Err(e) => {
                        return SubAgentReport {
                            run_id: String::new(),
                            name: req.name.clone().unwrap_or_else(|| req.agent_type.id().to_string()),
                            agent_type: req.agent_type.id().to_string(),
                            status: "failed".into(),
                            text: String::new(),
                            error: Some(e.message()),
                            tools: Vec::new(),
                            dropped_tools: Vec::new(),
                            iterations: 0,
                            tool_calls: 0,
                            wallclock_ms: 0,
                            usage: UsageSnapshot::default(),
                            origin: "batch".into(),
                            resumed_from: req.prior.as_ref().map(|p| p.run_id.clone()),
                        };
                    }
                };
                let run_id = rt.governor.next_run_id();
                let r = rt.run_child(&req, &run_id, "batch").await;
                drop(permit);
                r
            }
        }))
        .buffer_unordered(conc)
        .collect()
        .await;
        Ok(reports)
    }

    /// 查询后台作业。
    pub fn job_result(&self, run_id: &str) -> Result<Value, SpawnError> {
        let jobs = self.governor.jobs.lock().expect("jobs poisoned");
        let entry = jobs
            .get(run_id)
            .ok_or_else(|| SpawnError::UnknownRunId(run_id.to_string()))?;
        let state = entry.state.lock().expect("job state poisoned");
        match &*state {
            JobState::Running => Ok(json!({
                "run_id": run_id,
                "name": entry.view.name,
                "agent_type": entry.view.agent_type,
                "status": "running",
                "elapsed_ms": entry.view.started.elapsed().as_millis() as u64,
                "hint": "仍在运行;稍后再查询。",
            })),
            JobState::Done(r) => Ok(json!({
                "run_id": r.run_id,
                "name": r.name,
                "agent_type": r.agent_type,
                "status": r.status,
                "text": r.text,
                "error": r.error,
                "tools": r.tools,
                "dropped_tools": r.dropped_tools,
                "iterations": r.iterations,
                "tool_calls": r.tool_calls,
                "wallclock_ms": r.wallclock_ms,
                "usage": r.usage,
            })),
        }
    }

    /// 取消后台作业。
    pub fn job_cancel(&self, run_id: &str) -> Result<Value, SpawnError> {
        let jobs = self.governor.jobs.lock().expect("jobs poisoned");
        let entry = jobs
            .get(run_id)
            .ok_or_else(|| SpawnError::UnknownRunId(run_id.to_string()))?;
        let running = matches!(*entry.state.lock().expect("job state poisoned"), JobState::Running);
        if running {
            if let Some(t) = &entry.cancel {
                t.cancel();
            }
        }
        Ok(json!({
            "run_id": run_id,
            "status": if running { "cancelling" } else { "already_finished" },
        }))
    }

    // ---------------- 内部 ----------------

    fn clone_arc(self: &Arc<Self>) -> Arc<Self> {
        Arc::clone(self)
    }

    fn check_can_spawn(&self) -> Result<(), SpawnError> {
        if !self.cfg.enabled {
            return Err(SpawnError::Unavailable);
        }
        if !self.can_spawn() {
            return Err(SpawnError::DepthExceeded {
                depth: self.depth,
                max: self.cfg.max_depth,
            });
        }
        Ok(())
    }

    async fn acquire_permit(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, SpawnError> {
        match tokio::time::timeout(
            Duration::from_secs(PERMIT_WAIT_SECS),
            self.governor.sem.clone().acquire_owned(),
        )
        .await
        {
            Ok(Ok(p)) => Ok(p),
            Ok(Err(_)) => Err(SpawnError::Unavailable),
            Err(_) => Err(SpawnError::ConcurrencyTimeout {
                waited_secs: PERMIT_WAIT_SECS,
            }),
        }
    }

    /// 组装子 Agent profile(类型提示词 + 工具交集 + 叶子/可继续委派语义)。
    fn assemble_child(&self, req: &SubAgentRequest, run_id: &str) -> ChildAssembly {
        let base = builtin_registry_with_work_dir(self.work_dir.clone());
        let available: Vec<String> = base.names().iter().map(|s| s.to_string()).collect();
        let allowed: Vec<&str> = self.policy.allowed_tools().to_vec();
        let requested: Vec<String> = if req.tools.is_empty() {
            // 第 115 轮:自定义类型用「声明工具面」(显式 tools 或继承 extends 默认)
            req.agent_type.declared_tools()
        } else {
            req.tools
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let mut kept: Vec<String> = Vec::new();
        let mut dropped: Vec<String> = Vec::new();
        for name in &requested {
            if !available.contains(name) {
                dropped.push(format!("{name}(不可用/平台不支持)"));
            } else if !allowed.contains(&name.as_str()) {
                dropped.push(format!("{name}(超出父 Agent 能力上限)"));
            } else if !kept.contains(name) {
                kept.push(name.clone());
            }
        }
        if kept.is_empty() {
            // 地板:交集为空时退回策略上限 ∩ 平台可用,避免造出零工具 Agent
            kept = allowed
                .iter()
                .filter(|n| available.iter().any(|a| a == *n))
                .map(|s| s.to_string())
                .collect();
        }
        let child_depth = self.depth + 1;
        let child_can_spawn = self.cfg.can_spawn_at(child_depth);
        let mut registry = base.subset(
            &kept.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            &[SUBAGENT_TOOL_NAME],
        );
        if child_can_spawn {
            registry = registry.register(Arc::new(SubAgentTool));
        }
        let final_tools: Vec<String> = registry.names().iter().map(|s| s.to_string()).collect();
        let name = derive_child_name(req, run_id);
        let mut prompt = req.agent_type.system_prompt(&name);
        if let Some(extra) = req
            .system_prompt
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            prompt.push_str("\n\n## 父 Agent 追加要求\n");
            prompt.push_str(extra);
        }
        let policy = if child_can_spawn {
            self.policy
        } else {
            SpawnPolicy::Disabled
        };
        let profile = AgentProfile::dynamic_child(name, prompt, registry, policy);
        ChildAssembly {
            profile,
            tools: final_tools,
            dropped,
            policy,
        }
    }

    /// 运行一个子 Agent(全仓**唯一**运行入口:前台 / 后台 / batch / resume 都经此)。
    ///
    /// `origin` 用于运行记录落库与事件(`launch` / `background` / `batch` / `resume`)。
    async fn run_child(
        &self,
        req: &SubAgentRequest,
        run_id: &str,
        origin: &str,
    ) -> SubAgentReport {
        let started = Instant::now();
        let assembled = self.assemble_child(req, run_id);
        let tools = assembled.tools;
        let dropped = assembled.dropped;
        let child_policy = assembled.policy;
        let profile = assembled.profile;
        let name = profile.name.clone();
        self.persist_start(&name, req, run_id, origin);

        let mut user_prompt = String::new();
        user_prompt.push_str(&format!("【任务】\n{}\n", req.task.trim()));
        // 第 115 轮:resume 的种子上下文(前序任务 + 前序结论),让子 Agent 站在
        // 既有结论上继续,而不是从零开始(子 Agent 本身仍看不到父对话)
        if let Some(p) = req.prior.as_ref() {
            user_prompt.push_str(&format!(
                "\n【前序会话(继承自 {rid})】\n类型:{t}  状态:{s}  完成于:{at}\n\
                 【前序任务】\n{task}\n【前序结论】\n{report}\n",
                rid = p.run_id,
                t = p.agent_type,
                s = p.status,
                at = p.created_at,
                task = clip_chars(p.task.trim(), PRIOR_TASK_CHARS),
                report = clip_chars(p.report.trim(), PRIOR_REPORT_CHARS),
            ));
        }
        if !req.expected_output.trim().is_empty() {
            user_prompt.push_str(&format!(
                "\n【期望产出】\n{}\n",
                req.expected_output.trim()
            ));
        }
        user_prompt.push_str(&format!(
            "\n【运行环境】工作目录:{};你是动态子 Agent({}),看不到父 Agent 的对话上下文;\n\
             完成后用简洁中文回答,回答必须直接包含任务要求的内容本身。",
            self.work_dir.display(),
            req.agent_type.id()
        ));

        let mut child_session = Session::new();
        child_session.id = self.session_id.clone();
        child_session.context_mut().push(ChatMessage::user(&user_prompt));

        let max_iter = req
            .max_iterations
            .unwrap_or(self.cfg.max_iterations)
            .clamp(4, 32);
        let agent = Agent::new(self.llm.clone(), profile).with_max_iterations(max_iter);
        let child_token = self.cancel.as_ref().map(|t| t.child_token());
        let child_rt = Arc::new(SubAgentRuntime {
            llm: self.llm.clone(),
            parent_name: name.clone(),
            parent_tools: tools.clone(),
            policy: child_policy,
            cfg: self.cfg.clone(),
            governor: self.governor.clone(),
            depth: self.depth + 1,
            work_dir: self.work_dir.clone(),
            cancel: child_token.clone(),
            session_id: self.session_id.clone(),
        });

        info!(
            run_id = %run_id,
            parent = %self.parent_name,
            name = %name,
            agent_type = req.agent_type.id(),
            depth = self.depth + 1,
            tools = %tools.join(","),
            "动态子 Agent 启动"
        );

        let timeout = Duration::from_secs(self.cfg.timeout_secs);
        let fut = async {
            scope(child_rt, async move {
                agent
                    .run_session_cancellable(&mut child_session, child_token.as_ref())
                    .await
            })
            .await
        };

        let (status, text, error, usage, trace): (
            &str,
            String,
            Option<String>,
            Usage,
            ExecutionTrace,
        ) = match tokio::time::timeout(timeout, fut).await {
            Err(_) => (
                "timeout",
                String::new(),
                Some(format!("子 Agent 超过 {} 秒未完成", self.cfg.timeout_secs)),
                Usage::default(),
                ExecutionTrace::default(),
            ),
            Ok(Ok((t, u, tr))) => {
                let failed = tr.is_failed();
                (
                    if failed { "failed" } else { "ok" },
                    t,
                    if failed {
                        Some(format!(
                            "执行轨迹判定失败:{}",
                            tr.failure_signals.join(",")
                        ))
                    } else {
                        None
                    },
                    u,
                    tr,
                )
            }
            Ok(Err(AgentError::RepeatedToolFailure {
                tool,
                attempts,
                last_error,
                trace: carried,
            })) => (
                "failed",
                String::new(),
                Some(format!(
                    "工具 {tool} 连续 {attempts} 次失败:{last_error}"
                )),
                Usage::default(),
                *carried,
            ),
            Ok(Err(AgentError::MaxIterationsExceeded {
                iterations: n,
                trace: carried,
            })) => (
                "failed",
                String::new(),
                Some(format!("迭代达到 {n} 次上限仍未给出最终答案")),
                Usage::default(),
                *carried,
            ),
            Ok(Err(AgentError::Cancelled)) => {
                ("cancelled", String::new(), Some("已取消".into()), Usage::default(), ExecutionTrace::default())
            }
            Ok(Err(e)) => (
                "failed",
                String::new(),
                Some(format!("LLM / 运行错误:{e}")),
                Usage::default(),
                ExecutionTrace::default(),
            ),
        };

        let text = clip_chars(&text, CHILD_REPORT_CHARS);
        let wallclock_ms = started.elapsed().as_millis() as u64;
        self.governor.record_usage(&usage);
        self.governor.push_event(SubAgentEvent {
            run_id: run_id.to_string(),
            name: name.clone(),
            agent_type: req.agent_type.id().to_string(),
            parent: self.parent_name.clone(),
            depth: self.depth + 1,
            status: status.to_string(),
            task_digest: clip_chars(req.task.trim(), 120),
            wallclock_ms,
            tool_calls: trace.tool_calls,
            usage: UsageSnapshot::from_usage(&usage),
            origin: origin.to_string(),
        });
        info!(
            run_id = %run_id,
            name = %name,
            status = %status,
            wallclock_ms,
            tool_calls = trace.tool_calls,
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            "动态子 Agent 完成"
        );
        if status == "failed" || status == "timeout" {
            warn!(run_id = %run_id, name = %name, status = %status, error = ?error, "动态子 Agent 未成功");
        }

        let report = SubAgentReport {
            run_id: run_id.to_string(),
            name,
            agent_type: req.agent_type.id().to_string(),
            status: status.to_string(),
            text,
            error,
            tools,
            dropped_tools: dropped,
            iterations: trace.iterations,
            tool_calls: trace.tool_calls,
            wallclock_ms,
            usage: UsageSnapshot::from_usage(&usage),
            origin: origin.to_string(),
            resumed_from: req.prior.as_ref().map(|p| p.run_id.clone()),
        };
        self.persist_finish(&report);
        report
    }

    /// 落库「开始」行(status=running);失败只 warn,绝不影响任务。
    fn persist_start(&self, name: &str, req: &SubAgentRequest, run_id: &str, origin: &str) {
        if !self.cfg.persist {
            return;
        }
        let Some(db) = persist_db() else {
            return;
        };
        let entry = SubAgentRunEntry {
            run_id: run_id.to_string(),
            session_id: self.session_id.clone(),
            parent: self.parent_name.clone(),
            name: name.to_string(),
            agent_type: req.agent_type.id().to_string(),
            depth: self.depth + 1,
            status: crate::config::subagent_run::STATUS_RUNNING.to_string(),
            task: clip_chars(req.task.trim(), PRIOR_TASK_CHARS * 4),
            origin: origin.to_string(),
            resumed_from: req.prior.as_ref().map(|p| p.run_id.clone()),
            ..SubAgentRunEntry::default()
        };
        if let Err(e) = db.insert_subagent_run(&entry) {
            warn!(error = %e, run_id = %run_id, "子 Agent 运行记录写入失败(忽略,不影响任务)");
        }
    }

    /// 落库终态(状态 / 产物 / 用量 / 工具面);失败只 warn。
    fn persist_finish(&self, report: &SubAgentReport) {
        if !self.cfg.persist || report.run_id.is_empty() {
            return;
        }
        let Some(db) = persist_db() else {
            return;
        };
        let entry = SubAgentRunEntry {
            run_id: report.run_id.clone(),
            status: report.status.clone(),
            report: report.text.clone(),
            error: report.error.clone(),
            tools: report.tools.clone(),
            dropped_tools: report.dropped_tools.clone(),
            iterations: report.iterations,
            tool_calls: report.tool_calls,
            wallclock_ms: report.wallclock_ms,
            input_tokens: report.usage.input_tokens,
            output_tokens: report.usage.output_tokens,
            ..SubAgentRunEntry::default()
        };
        if let Err(e) = db.finish_subagent_run(&entry) {
            warn!(error = %e, run_id = %report.run_id, "子 Agent 运行记录收尾失败(忽略)");
        }
    }
}

/// 派生子 Agent 名(用于日志 / UA / 报告);非法字符替换,长度截断。
/// 子 Agent 组装结果(profile + 实际工具面 + 被剔除工具 + 生效策略)。
struct ChildAssembly {
    profile: AgentProfile,
    tools: Vec<String>,
    dropped: Vec<String>,
    policy: SpawnPolicy,
}

/// 派生子 Agent 名(用于日志 / UA / 报告);非法字符替换,长度截断。
fn derive_child_name(req: &SubAgentRequest, fallback_id: &str) -> String {
    let raw = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_id);
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_whitespace() || c == '/' { '-' } else { c })
        .collect();
    let short = clip_chars(&cleaned, 24);
    format!("LsmAgentEmergentWork-SubAgent-{}-{}", req.agent_type.pascal(), short)
}

/// 按字符数截断(超长追加省略标注;CJK 安全)。
pub fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…(共 {} 字符,已截断)", s.chars().count())
}

/// batch 报告合并文本(逐子小标题 + 截断,总长受限)。
pub fn merge_batch_text(reports: &[SubAgentReport]) -> String {
    let mut out = String::new();
    for r in reports {
        let body = if r.text.trim().is_empty() {
            r.error.clone().unwrap_or_else(|| "(无输出)".to_string())
        } else {
            clip_chars(&r.text, sa::BATCH_CHILD_CHARS)
        };
        out.push_str(&format!(
            "\n### [{}] {} ({}) — {}\n{}\n",
            r.agent_type, r.name, r.run_id, r.status, body
        ));
    }
    clip_chars(out.trim_start(), BATCH_MERGED_CHARS)
}

/// 报告 → JSON(工具返回 data 用)。
pub fn report_json(r: &SubAgentReport) -> Value {
    json!({
        "run_id": r.run_id,
        "name": r.name,
        "agent_type": r.agent_type,
        "status": r.status,
        "origin": r.origin,
        "resumed_from": r.resumed_from,
        "text": r.text,
        "error": r.error,
        "tools": r.tools,
        "dropped_tools": r.dropped_tools,
        "iterations": r.iterations,
        "tool_calls": r.tool_calls,
        "wallclock_ms": r.wallclock_ms,
        "usage": r.usage,
    })
}

#[cfg(test)]
/// 测试用:构造一个独立运行时(供 tools/subagent.rs 的用例复用)。
pub(crate) fn test_runtime(
    llm: Arc<dyn LlmClient>,
    session: &str,
    policy: SpawnPolicy,
    depth: usize,
    cfg: SelfAwarenessConfig,
) -> Arc<SubAgentRuntime> {
    test_runtime_in(
        llm,
        session,
        policy,
        depth,
        cfg,
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    )
}

/// 同 [`test_runtime`],但显式指定工作目录(自定义 Agent 定义文件的发现基准)。
pub(crate) fn test_runtime_in(
    llm: Arc<dyn LlmClient>,
    session: &str,
    policy: SpawnPolicy,
    depth: usize,
    cfg: SelfAwarenessConfig,
    work_dir: PathBuf,
) -> Arc<SubAgentRuntime> {
    Arc::new(SubAgentRuntime {
        llm,
        parent_name: "LsmAgentEmergentWork-SubAgent-Work".into(),
        parent_tools: vec!["Bash".into(), "Read".into(), "SubAgent".into()],
        policy,
        governor: governor_for(session, &cfg),
        cfg,
        depth,
        work_dir,
        cancel: None,
        session_id: session.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::self_awareness::SubAgentType;
    use crate::agent::self_awareness::SelfAwarenessConfig;
    use crate::agent::tools::Tool;
    use crate::config::Protocol;
    use crate::llm::{Completion, RequestMeta, ToolDef};
    use std::sync::atomic::AtomicUsize;

    /// 脚本化 mock LLM:记录收到的 system / user 文本,直接返回最终文本。
    struct ScriptLlm {
        calls: AtomicUsize,
        systems: Mutex<Vec<String>>,
        users: Mutex<Vec<String>>,
    }

    impl ScriptLlm {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                systems: Mutex::new(Vec::new()),
                users: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for ScriptLlm {
        async fn complete(
            &self,
            system: &str,
            messages: &[ChatMessage],
            _tools: &[ToolDef],
            _meta: &RequestMeta,
        ) -> crate::error::Result<Completion> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.systems.lock().unwrap().push(system.to_string());
            let user_text: String = messages
                .iter()
                .filter(|m| m.role == crate::llm::Role::User)
                .map(|m| m.content_text())
                .collect::<Vec<_>>()
                .join("\n");
            self.users.lock().unwrap().push(user_text);
            Ok(Completion {
                text: "子 Agent 结果:OK".into(),
                tool_calls: vec![],
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_read_input_tokens: 1,
                    cache_creation_input_tokens: 2,
                },
                stop_reason: None,
            })
        }
        fn protocol(&self) -> Protocol {
            Protocol::Anthropic
        }
    }

    /// 并行测试隔离:每个运行时用**唯一** session id(治理器按 session 注册,
    /// 独享 id 即独享预算 / 并发 / 台账,不与其他测试串味)。
    fn unique_session(tag: &str) -> String {
        static N: AtomicUsize = AtomicUsize::new(0);
        format!("sess-{tag}-{}", N.fetch_add(1, Ordering::SeqCst))
    }

    /// 直接构造运行时(绕过 `agent_loop` 的 `runtime_for`);返回 (运行时, session_id)。
    fn rt(
        llm: Arc<dyn LlmClient>,
        tag: &str,
        policy: SpawnPolicy,
        depth: usize,
        cfg: SelfAwarenessConfig,
    ) -> (Arc<SubAgentRuntime>, String) {
        let session = unique_session(tag);
        let r = Arc::new(SubAgentRuntime {
            llm,
            parent_name: "LsmAgentEmergentWork-SubAgent-Work".into(),
            parent_tools: vec!["Bash".into(), "Read".into(), "SubAgent".into()],
            policy,
            governor: governor_for(&session, &cfg),
            cfg,
            depth,
            work_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            cancel: None,
            session_id: session.clone(),
        });
        (r, session)
    }

    fn req(task: &str, t: SubAgentType) -> SubAgentRequest {
        SubAgentRequest {
            task: task.into(),
            agent_type: ResolvedAgentType::Builtin(t),
            name: None,
            system_prompt: None,
            tools: vec![],
            expected_output: String::new(),
            max_iterations: None,
            prior: None,
        }
    }

    // ========== 第 115 轮(2026-09-22):自定义类型 / 持久化 / 续跑 ==========

    /// 写一个自定义 Agent 定义文件并返回工作目录。
    fn work_dir_with_agent(name: &str, content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join(ca::DIR).join(ca::SUBDIR);
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join(name), content).unwrap();
        dir
    }

    /// 建一个临时库并注入为落库目标;返回的 TempDir 需在测试结束前持有。
    ///
    /// 落库目标通过**进程级覆盖**注入,并发测试间会互相看到 —— 因此所有触库断言
    /// 都必须只针对「自己的 run_id / session_id」,且用 [`db_test_lock`] 串行化
    /// 「注入 → 断言 → 清空」全过程。
    fn inject_temp_db() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::config::Paths::for_test(dir.path());
        set_test_db(Some(Arc::new(Db::open(&paths).unwrap())));
        dir
    }

    /// 触库测试的串行化闸门(避免 A 的临时库被 B 覆盖时读到空结果)。
    async fn db_test_lock() -> tokio::sync::MutexGuard<'static, ()> {
        static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        LOCK.lock().await
    }

    #[tokio::test]
    async fn custom_agent_type_uses_definition_body_and_tools() {
        let dir = work_dir_with_agent(
            "fe-reviewer.md",
            "---\nlabel: 前端审查\ndescription: 前端专项\nextends: code-reviewer\n\
             tools: Read, Glob, Grep, Bash\nreadonly: true\n---\n只给问题清单,不写文件。\n",
        );
        let llm = ScriptLlm::new();
        let r = test_runtime_in(
            llm.clone(),
            &unique_session("custom"),
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
            dir.path().to_path_buf(),
        );
        let defs = r.defs();
        assert_eq!(defs.len(), 1, "应发现自定义定义");
        let t = ca::resolve_in("fe-reviewer", &defs).expect("自定义类型可解析");

        let mut request = req("审查 src/tui 的最近改动,给 P0-P2 清单", SubAgentType::Explore);
        request.agent_type = t;
        let report = scope(r.clone(), r.launch(request)).await.unwrap();
        assert_eq!(report.status, "ok");
        assert_eq!(report.agent_type, "fe-reviewer", "报告应回传自定义类型 id");
        assert_eq!(report.origin, "launch");
        assert!(report.resumed_from.is_none());

        // readonly 在**定义解析期**就收窄工具面(Bash 不进声明),运行期自然仍无 Bash
        for t in ["Read", "Glob", "Grep"] {
            assert!(report.tools.contains(&t.to_string()), "应含 {t}: {:?}", report.tools);
        }
        assert!(!report.tools.contains(&"Bash".to_string()), "readonly 不得带入 Bash");
        // 子 Agent 提示词含定义文件正文与自定义身份
        let sys = llm.systems.lock().unwrap().join("\n---\n");
        assert!(sys.contains("类型:前端审查(fe-reviewer,自定义定义文件)"));
        assert!(sys.contains("只给问题清单,不写文件。"));
        assert!(sys.contains("只读**委派"));
    }

    #[tokio::test]
    async fn custom_type_tools_are_narrowed_by_parent_policy_and_reported() {
        let dir = work_dir_with_agent(
            "patcher.md",
            "---\nlabel: 打补丁\ntools: Read, Write\n---\n直接改文件。\n",
        );
        // 只读父策略:声明的 Write 必须被剔除并如实上报(即便名册本就不展示它)
        let r = test_runtime_in(
            ScriptLlm::new(),
            &unique_session("narrow"),
            SpawnPolicy::ReadOnlyChildren,
            0,
            SelfAwarenessConfig::default(),
            dir.path().to_path_buf(),
        );
        let t = ca::resolve_in("patcher", &r.defs()).expect("自定义类型可解析");
        let mut request = req("改一处小 bug", SubAgentType::Explore);
        request.agent_type = t;
        let report = scope(r.clone(), r.launch(request)).await.unwrap();
        assert_eq!(report.status, "ok");
        assert!(report.tools.contains(&"Read".to_string()));
        assert!(!report.tools.contains(&"Write".to_string()));
        assert!(
            report
                .dropped_tools
                .iter()
                .any(|d| d.contains("Write") && d.contains("超出父 Agent 能力上限")),
            "越权工具须解释性上报: {:?}",
            report.dropped_tools
        );
    }

    #[tokio::test]
    async fn persist_writes_running_row_then_final_row() {
        let _lock = db_test_lock().await;
        let _guard = inject_temp_db();
        let llm = ScriptLlm::new();
        let (r, sid) = rt(llm, "persist", SpawnPolicy::FullChildren, 0, SelfAwarenessConfig::default());
        let report = scope(r.clone(), r.launch(req("写一份结论", SubAgentType::Plan)))
            .await
            .unwrap();
        assert_eq!(report.status, "ok");

        let row = get_run(&report.run_id).expect("应已落库");
        assert_eq!(row.session_id, sid);
        assert_eq!(row.status, "ok");
        assert_eq!(row.agent_type, "plan");
        assert_eq!(row.origin, "launch");
        assert!(row.report.contains("子 Agent"), "报告文本应落库");
        assert!(row.input_tokens > 0, "用量应落库");
        assert!(!row.created_at.is_empty());
        // 终态行不得残留 running
        let rows = query_runs(Some(&sid), None, 10).expect("查询应可用");
        assert_eq!(rows.len(), 1);
        assert!(rows.iter().all(|r| r.status != "running"));
        clear_test_db();
    }

    #[tokio::test]
    async fn persist_off_writes_nothing() {
        let _lock = db_test_lock().await;
        let _guard = inject_temp_db();
        let mut cfg = SelfAwarenessConfig::default();
        cfg.persist = false;
        let (r, sid) = rt(ScriptLlm::new(), "no-persist", SpawnPolicy::FullChildren, 0, cfg);
        let report = scope(r.clone(), r.launch(req("不落库", SubAgentType::Explore)))
            .await
            .unwrap();
        assert_eq!(report.status, "ok");
        assert!(get_run(&report.run_id).is_none(), "关闭持久化后不得落库");
        // 注:`query_runs` / `get_run` 的开关取自**进程级** `sa::config()`(与
        // `LAEW_SUBAGENT_PERSIST` 同级),本测试用运行时级 cfg 关闭,故只断言「没写」。
        let rows = query_runs(Some(&sid), None, 10).expect("全局开关未关,查询应可用");
        assert!(
            rows.iter().all(|r| r.run_id != report.run_id),
            "关闭运行时持久化后该 run 不得出现在库中"
        );
        clear_test_db();
    }

    #[tokio::test]
    async fn resume_seeds_prior_context_and_records_lineage() {
        let _lock = db_test_lock().await;
        let _guard = inject_temp_db();
        let llm = ScriptLlm::new();
        let (r, sid) = rt(llm.clone(), "resume", SpawnPolicy::FullChildren, 0, SelfAwarenessConfig::default());
        let first = scope(r.clone(), r.launch(req("先给出初步结论", SubAgentType::Explore)))
            .await
            .unwrap();
        assert_eq!(first.status, "ok");

        let prior_row = get_run(&first.run_id).expect("前序行应存在");
        let mut again = req("把结论展开成可执行步骤", SubAgentType::Explore);
        again.prior = Some(PriorRun {
            run_id: prior_row.run_id.clone(),
            agent_type: prior_row.agent_type.clone(),
            status: prior_row.status.clone(),
            created_at: prior_row.created_at.clone(),
            task: prior_row.task.clone(),
            report: prior_row.report.clone(),
        });
        let second = scope(r.clone(), r.launch_with_origin(again, "resume"))
            .await
            .unwrap();
        assert_eq!(second.status, "ok");
        assert_eq!(second.origin, "resume");
        assert_eq!(second.resumed_from.as_deref(), Some(first.run_id.as_str()));

        // 种子上下文必须真的进了子 Agent 的 user prompt
        let users = llm.users.lock().unwrap().clone();
        let resumed_prompt = users.last().expect("应有第二次调用").clone();
        assert!(resumed_prompt.contains("【前序会话(继承自"));
        assert!(resumed_prompt.contains("【前序任务】"));
        assert!(resumed_prompt.contains("【前序结论】"));
        assert!(resumed_prompt.contains("把结论展开成可执行步骤"));

        // 落库:两条,第二条带血缘
        let rows = query_runs(Some(&sid), None, 10).unwrap();
        assert_eq!(rows.len(), 2);
        let resumed_row = rows.iter().find(|r| r.run_id == second.run_id).unwrap();
        assert_eq!(resumed_row.origin, "resume");
        assert_eq!(resumed_row.resumed_from.as_deref(), Some(first.run_id.as_str()));
        clear_test_db();
    }

    /// 工具层闭环:`launch` 落库 -> `history` 查到 -> `resume` 读前序并记血缘。
    #[tokio::test]
    async fn tool_history_and_resume_close_the_loop() {
        let _lock = db_test_lock().await;
        let _guard = inject_temp_db();
        let llm = ScriptLlm::new();
        let r = test_runtime_in(
            llm.clone(),
            &unique_session("tool-loop"),
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        );
        let tool = SubAgentTool;
        let sid = r.session_id().to_string();
        let out = scope(r.clone(), async {
            let first = serde_json::from_str::<Value>(
                &tool
                    .execute(json!({
                        "action": "launch",
                        "task": "先给出初步结论",
                        "agent_type": "explore",
                        "name": "侦察-1"
                    }))
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(first["code"], 0);
            let run_id = first["data"]["run_id"].as_str().unwrap().to_string();

            let hist = serde_json::from_str::<Value>(
                &tool.execute(json!({"action": "history"})).await.unwrap(),
            )
            .unwrap();
            assert_eq!(hist["code"], 0);
            assert_eq!(hist["data"]["persist"], true);
            let mine = hist["data"]["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["run_id"] == json!(run_id))
                .expect("刚跑完的作业应出现在 history 里")
                .clone();
            assert_eq!(mine["status"], "ok");
            assert_eq!(mine["origin"], "launch");

            let resumed = serde_json::from_str::<Value>(
                &tool
                    .execute(json!({
                        "action": "resume",
                        "run_id": run_id,
                        "task": "把结论展开成可执行步骤"
                    }))
                    .await
                    .unwrap(),
            )
            .unwrap();
            (resumed, run_id)
        })
        .await;
        let (resumed, prior_id) = out;
        assert_eq!(resumed["code"], 0, "resume 应成功: {resumed}");
        assert_eq!(resumed["data"]["origin"], "resume");
        assert_eq!(resumed["data"]["resumed_from"], json!(prior_id));

        // 子 Agent 的 user prompt 里必须真的带了前序上下文
        let users = llm.users.lock().unwrap().clone();
        let last = users.last().expect("应有续跑调用").clone();
        assert!(last.contains("【前序会话(继承自"));
        assert!(last.contains("【前序结论】"));
        assert!(last.contains("把结论展开成可执行步骤"));

        // 血缘落库
        let rows = query_runs(Some(&sid), None, 10).unwrap();
        let resumed_row = rows
            .iter()
            .find(|r| r.origin == "resume")
            .expect("续跑行应落库");
        assert_eq!(resumed_row.resumed_from.as_deref(), Some(prior_id.as_str()));
        clear_test_db();
    }

    #[tokio::test]
    async fn snapshot_exposes_custom_roster_and_skipped() {
        let dir = work_dir_with_agent(
            "patcher.md",
            "---\nlabel: 打补丁\ndescription: 需要写盘\ntools: Read, Write\n---\n直接改文件。\n",
        );
        // Yolo 策略(只读子 Agent)下,可写的自定义类型应被隐藏并给出原因
        let r = test_runtime_in(
            ScriptLlm::new(),
            &unique_session("snap"),
            SpawnPolicy::ReadOnlyChildren,
            0,
            SelfAwarenessConfig::default(),
            dir.path().to_path_buf(),
        );
        let snap = r.snapshot_json();
        let skipped = snap["custom_skipped"].as_array().unwrap();
        assert!(
            skipped.iter().any(|s| s["id"] == "patcher"),
            "只读父下可写自定义类型应被隐藏: {snap}"
        );
        assert!(snap["limits"]["persist"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn launch_runs_child_reports_usage_and_narrows_tools() {
        let llm = ScriptLlm::new();
        let (r, _sid) = rt(llm.clone(), "launch",
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        let report = scope(
            r.clone(),
            r.launch(req("调研 src/agent 的模块划分,给出清单", SubAgentType::Explore)),
        )
        .await
        .expect("launch 应成功");
        assert_eq!(report.status, "ok");
        assert_eq!(report.text, "子 Agent 结果:OK");
        assert_eq!(report.usage.input_tokens, 10);
        assert_eq!(report.usage.output_tokens, 5);
        // explore 默认工具 = Read/Glob/Grep,且不含 SubAgent(叶子)
        assert_eq!(report.tools, vec!["Read", "Glob", "Grep"]);
        assert!(!report.tools.contains(&"SubAgent".to_string()));
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        // 子 Agent prompt 自包含 + 叶子语义
        let system = llm.systems.lock().unwrap()[0].clone();
        assert!(system.contains("不能再启动子 Agent"));
        assert!(system.contains("Explore"), "system 应含类型身份");
        let user = llm.users.lock().unwrap()[0].clone();
        assert!(user.contains("调研 src/agent 的模块划分"));
        assert!(user.contains("看不到父 Agent 的对话上下文"));
        // 用量台账已记账
        let drained = drain_usage(&_sid);
        assert_eq!(drained.input_tokens, 10);
        assert_eq!(drain_usage(&_sid).input_tokens, 0, "drain 应清零");
    }

    #[tokio::test]
    async fn launch_drops_tools_beyond_parent_policy() {
        let llm = ScriptLlm::new();
        let (r, _sid) = rt(llm, "policy",
            SpawnPolicy::ReadOnlyChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        let mut request = req("给 src 写一份重构方案", SubAgentType::Plan);
        request.tools = vec!["Write".into(), "Bash".into(), "Read".into()];
        let report = scope(r.clone(), r.launch(request)).await.unwrap();
        assert_eq!(report.tools, vec!["Read"], "只读父的策略上限只留 Read");
        assert!(
            report.dropped_tools.iter().any(|d| d.contains("Write")),
            "被剔除工具应如实报告: {:?}",
            report.dropped_tools
        );
        assert!(
            report.dropped_tools.iter().any(|d| d.contains("Bash")),
            "被剔除工具应如实报告: {:?}",
            report.dropped_tools
        );
    }

    #[tokio::test]
    async fn policy_floor_prevents_zero_tool_child() {
        // 请求一个与类型默认工具完全不相干的集合 → 交集为空时退回策略上限,
        // 避免造出零工具 Agent(纯文本空转)。
        let (r, _sid) = rt(ScriptLlm::new(), "floor",
            SpawnPolicy::ReadOnlyChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        let mut request = req("只读侦察", SubAgentType::Explore);
        request.tools = vec!["Write".into()];
        let report = scope(r.clone(), r.launch(request)).await.unwrap();
        assert_eq!(report.status, "ok");
        assert_eq!(report.tools, vec!["Read", "Glob", "Grep"], "应退回策略地板");
    }

    #[tokio::test]
    async fn depth_exceeded_returns_2002() {
        let cfg = SelfAwarenessConfig {
            max_depth: 1,
            ..SelfAwarenessConfig::default()
        };
        let (r, _sid) = rt(ScriptLlm::new(), "depth", SpawnPolicy::FullChildren, 1, cfg);
        assert!(!r.can_spawn());
        let err = scope(r.clone(), r.launch(req("再开一个", SubAgentType::Explore)))
            .await
            .unwrap_err();
        assert_eq!(err.code(), 2002);
        assert!(err.message().contains("不允许继续嵌套"));
    }

    #[tokio::test]
    async fn budget_exhausted_returns_2001_and_does_not_overrun() {
        let cfg = SelfAwarenessConfig {
            max_total: 1,
            ..SelfAwarenessConfig::default()
        };
        let (r, _sid) = rt(ScriptLlm::new(), "budget", SpawnPolicy::FullChildren, 0, cfg);
        let first = scope(r.clone(), r.launch(req("任务一", SubAgentType::Explore))).await;
        assert!(first.is_ok());
        let second = scope(r.clone(), r.launch(req("任务二", SubAgentType::Explore))).await;
        assert_eq!(second.unwrap_err().code(), 2001);
        assert_eq!(r.governor.used(), 1, "预算不应超扣");
    }

    #[tokio::test]
    async fn batch_runs_all_children_and_merges_text() {
        let llm = ScriptLlm::new();
        let (r, _sid) = rt(llm.clone(), "batch",
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        let reqs = vec![
            req("调研 A 模块", SubAgentType::Explore),
            req("调研 B 模块", SubAgentType::Explore),
            req("调研 C 模块", SubAgentType::Explore),
        ];
        let reports = scope(r.clone(), r.batch(reqs, Some(3))).await.unwrap();
        assert_eq!(reports.len(), 3);
        assert!(reports.iter().all(|x| x.status == "ok"));
        assert_eq!(llm.calls.load(Ordering::SeqCst), 3, "每个子任务都应真实调用 LLM");
        assert_eq!(r.governor.used(), 3, "批量预扣应精确计 3");
        let merged = merge_batch_text(&reports);
        assert!(merged.contains("explore"));
        assert!(merged.contains("子 Agent 结果:OK"));
    }

    #[tokio::test]
    async fn batch_over_limit_and_over_budget_are_rejected() {
        let (r, _sid) = rt(ScriptLlm::new(), "batch-limit",
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        let too_many: Vec<SubAgentRequest> = (0..9)
            .map(|i| req(&format!("任务{i}"), SubAgentType::Explore))
            .collect();
        let err = scope(r.clone(), r.batch(too_many, None)).await.unwrap_err();
        assert_eq!(err.code(), 1001);

        let cfg = SelfAwarenessConfig {
            max_total: 2,
            ..SelfAwarenessConfig::default()
        };
        let (r2, _sid2) = rt(ScriptLlm::new(), "batch-budget", SpawnPolicy::FullChildren, 0, cfg);
        let four: Vec<SubAgentRequest> = (0..4)
            .map(|i| req(&format!("任务{i}"), SubAgentType::Explore))
            .collect();
        let err = scope(r2.clone(), r2.batch(four, None)).await.unwrap_err();
        assert_eq!(err.code(), 2001);
        assert_eq!(r2.governor.used(), 0, "全有或全无:失败的批量不扣预算");
    }

    #[tokio::test]
    async fn snapshot_exposes_self_awareness_fields() {
        let (r, _sid) = rt(ScriptLlm::new(), "snap",
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        let snap = r.snapshot_json();
        assert_eq!(snap["agent"]["can_spawn"], true);
        assert_eq!(snap["agent"]["max_depth"], 1);
        assert_eq!(snap["agent"]["spawn_policy"], "full");
        assert_eq!(snap["limits"]["remaining"], 8);
        assert_eq!(snap["limits"]["used"], 0);
        assert_eq!(snap["limits"]["max_parallel"], 3);
        assert_eq!(snap["roster"].as_array().unwrap().len(), 6);
        assert_eq!(snap["session_id"], _sid);
        // 只读策略下名册收敛为 2 项
        let (ro, _sid_ro) = rt(
            ScriptLlm::new(),
            "snap-ro",
            SpawnPolicy::ReadOnlyChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        assert_eq!(ro.snapshot_json()["roster"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn background_job_can_be_polled_and_cancelled() {
        let (r, _sid) = rt(ScriptLlm::new(), "bg",
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        let started = {
            let r2 = r.clone();
            scope(
                r.clone(),
                async move { r2.launch_background(req("后台调研", SubAgentType::Explore)) },
            )
            .await
            .unwrap()
        };
        let run_id = started["run_id"].as_str().unwrap().to_string();
        assert_eq!(started["status"], "running");
        let mut final_state = Value::Null;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let v = r.job_result(&run_id).unwrap();
            if v["status"] != "running" {
                final_state = v;
                break;
            }
        }
        assert_eq!(final_state["status"], "ok", "后台作业应完成: {final_state:?}");
        assert_eq!(final_state["text"], "子 Agent 结果:OK");
        // 未知 run_id → 4003
        assert_eq!(r.job_result("sa-nope").unwrap_err().code(), 4003);
        // 已完成作业的 cancel 幂等
        let c = r.job_cancel(&run_id).unwrap();
        assert_eq!(c["status"], "already_finished");
    }

    #[tokio::test]
    async fn max_depth_two_keeps_subagent_tool_on_child() {
        // LAEW_SUBAGENT_MAX_DEPTH=2:子 Agent 仍持 SubAgent 工具(可再拆一层);
        // 默认 1 层时子 Agent 必须被收成叶子(见上方 launch_* 用例)。
        let cfg = SelfAwarenessConfig {
            max_depth: 2,
            ..SelfAwarenessConfig::default()
        };
        let (r, _sid) = rt(ScriptLlm::new(), "depth2", SpawnPolicy::FullChildren, 0, cfg);
        let report = scope(r.clone(), r.launch(req("调研 A 模块", SubAgentType::Explore)))
            .await
            .unwrap();
        assert_eq!(report.status, "ok");
        assert!(
            report.tools.contains(&"SubAgent".to_string()),
            "max_depth=2 时子 Agent 应保留委派能力: {:?}",
            report.tools
        );
    }

    #[tokio::test]
    async fn parent_cancel_propagates_to_child() {
        let token = CancelToken::new();
        token.cancel();
        let cfg = SelfAwarenessConfig::default();
        let (base, _sid_c) = rt(ScriptLlm::new(), "cancel", SpawnPolicy::FullChildren, 0, cfg);
        let r = Arc::new(SubAgentRuntime {
            cancel: Some(token),
            ..match Arc::try_unwrap(base) {
                Ok(inner) => inner,
                Err(_) => unreachable!("独占持有"),
            }
        });
        let report = scope(r.clone(), r.launch(req("会被取消", SubAgentType::Explore)))
            .await
            .unwrap();
        assert_eq!(report.status, "cancelled", "父取消应即时中止子 Agent");
    }

    #[test]
    fn runtime_for_is_none_for_disabled_profiles_and_without_tool() {
        let llm: Arc<dyn LlmClient> = ScriptLlm::new();
        // 质检角色:策略 Disabled → 不建运行时(零开销直通)
        let qc = AgentProfile::quality_check_profile();
        assert!(runtime_for("qc", &qc, llm.clone(), "sess-none", None).is_none());
        // 无父作用域时 current() 为 None(不在 scope 内)
        assert!(current().is_none());
    }

    #[tokio::test]
    async fn runtime_inherits_governor_and_increments_depth() {
        let cfg = SelfAwarenessConfig::default();
        let (r, sid) = rt(ScriptLlm::new(), "inherit", SpawnPolicy::FullChildren, 0, cfg);
        let sid2 = sid.clone();
        let child_profile = AgentProfile::dynamic_child(
            "child",
            "child prompt".into(),
            crate::agent::tools::sub_agent_work_registry(),
            SpawnPolicy::FullChildren,
        );
        let derived = scope(r.clone(), async {
            // 作用域内构造子 Agent 的运行时 → 命中继承分支
            runtime_for(
                "LsmAgentEmergentWork-SubAgent-Explore-x",
                &child_profile,
                r.llm.clone(),
                &sid2,
                None,
            )
            .expect("子 Agent 应继承运行时")
        })
        .await;
        assert_eq!(derived.depth(), 1);
        assert_eq!(derived.remaining(), r.remaining(), "共享同一治理器");
        assert!(!derived.can_spawn(), "max_depth=1 → 子 Agent 不能再启动");

        // 名字与作用域 owner 相同时必须**原样复用**(防 depth 二次 +1)
        let same = scope(r.clone(), async {
            runtime_for(
                "LsmAgentEmergentWork-SubAgent-Work",
                &AgentProfile::sub_agent_work_profile(),
                r.llm.clone(),
                &sid2,
                None,
            )
            .expect("应复用当前作用域")
        })
        .await;
        assert!(Arc::ptr_eq(&same, &r), "同名字 Agent 应复用作用域运行时");
        assert_eq!(same.depth(), 0, "复用不得改变 depth");
    }
}
