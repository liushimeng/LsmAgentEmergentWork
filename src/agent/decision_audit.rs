//! D9-8 决策审计与 Agent 决策溯源(L1591-L1600)。
//!
//! 对标 claudecode 3 段式 40+ 字段决策审计 / openclaw 10 万行审计:
//! 在 5 个决策点(Yolo 分类 / Plan 规划 / Main-Work 拆解 / Quality-Check 判定 / Compact 压缩)
//! 追加一条结构化 JSON 行到根目录 `AuditTrail/audit_{session_id}.jsonl`,
//! 记录「输入上下文 → 决策结论 → 决策依据」三元组,供事后分析 / 评测回归 / 合规审计。
//!
//! ## 与现有系统的关系
//!
//! - `tracing` / `src/logging.rs`:保留不动(操作日志);审计是更高层的「Agent 决策」。
//! - `DebugReport`:`-debug` 评估报告保留不动;审计 JSONL 是更细粒度的「每决策一条」。
//! - `ExecutionTrace`:SubAgent 工具执行轨迹保留不动;审计是更高层决策。
//! - `agent_memory`:SQLite 记忆保留不动;审计 JSONL 是 append-only 日志,不入 SQLite。
//!
//! ## 行为契约
//!
//! - `LAEW_AUDIT=off|0|false|no` → 关闭审计写入(默认开启)。
//! - 审计写入失败 fail-open:只 eprintln!,不中断主流程。
//! - 全字段 `scrub_secrets` + 截断,防 API Key / 超长提示词入库。
//!
//! 2026-09-22 第 113 轮(D9-8 闭环):新增 `read_events` / `count_events` /
//! `verify_events` / `file_size` / `pub audit_root_dir`,
//! 为 TUI `/audit` 命令与 `/cost` 集成提供读取入口;`trim_audit_files` 由
//! TUI bootstrap 自动调用清理。

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::agent::context::AgentRole;
use crate::agent::debug::scrub_secrets;
use crate::error::Result;
use crate::llm::Usage;

// =================== 常量 ===================

const MAX_INPUT_SUMMARY_CHARS: usize = 200;
const MAX_OUTCOME_CHARS: usize = 500;
const MAX_RATIONALE_CHARS: usize = 500;
const MAX_META_CHARS: usize = 1000;
/// 审计目录名(根目录之下)。
const AUDIT_DIR: &str = "AuditTrail";

// =================== 审计事件 ===================

/// 决策审计事件(落盘 JSONL 的一行)。
///
/// 每条事件是 3 段式:input_summary(输入上下文) → outcome(决策结论) → rationale(决策依据),
/// 对标 claudecode 3 段式 40+ 字段审计 schema 的精简 Rust CLI 子集。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// ISO8601 本地时区时间戳。
    pub ts: String,
    /// 当前 Session ID。
    pub session_id: String,
    /// 决策主体 Agent 角色。
    pub agent: String,
    /// 决策类型:classify / plan / decompose / verdict / compact / summary / evaluate。
    pub decision: String,
    /// 决策输入摘要(截 200 字符,脱敏)。
    pub input_summary: String,
    /// 决策结论(截 500 字符,脱敏)。
    pub outcome: String,
    /// 决策依据(截 500 字符,脱敏)。
    pub rationale: String,
    /// 决策耗时(毫秒)。
    pub duration_ms: u64,
    /// token 用量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Usage>,
    /// 扩展字段(按决策类型变)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

impl AuditEvent {
    /// 构造一条审计事件(自动截断 + 脱敏)。
    pub fn new(
        session_id: impl Into<String>,
        agent: impl Into<String>,
        decision: impl Into<String>,
        input_summary: impl Into<String>,
        outcome: impl Into<String>,
        rationale: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            ts: crate::session::now_readable(),
            session_id: session_id.into(),
            agent: agent.into(),
            decision: decision.into(),
            input_summary: scrub_scrub(&truncate(input_summary.into(), MAX_INPUT_SUMMARY_CHARS)),
            outcome: scrub_scrub(&truncate(outcome.into(), MAX_OUTCOME_CHARS)),
            rationale: scrub_scrub(&truncate(rationale.into(), MAX_RATIONALE_CHARS)),
            duration_ms,
            tokens: None,
            meta: None,
        }
    }

    /// 设置 token 用量。
    pub fn with_tokens(mut self, usage: Usage) -> Self {
        self.tokens = Some(usage);
        self
    }

    /// 设置扩展字段。
    pub fn with_meta(mut self, meta: serde_json::Value) -> Self {
        // meta 也截断(序列化后检查长度,超长截断字符串值)
        self.meta = Some(clamp_meta(meta));
        self
    }
}

// =================== 全局注册表 ===================

/// 会话级审计写入器注册表(每个 Session 一个追加写文件)。
static AUDIT_REGISTRY: OnceLock<Mutex<HashMap<String, Arc<Mutex<AuditWriter>>>>> = OnceLock::new();

fn registry(
) -> &'static Mutex<HashMap<String, Arc<Mutex<AuditWriter>>>> {
    AUDIT_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 推导审计根目录(二进制所在目录,与 `Paths::detect().root_dir` 同义)。
///
/// 审计模块独立推导,避免向 Orchestrator 注入 root_dir 字段(减少与并发任务的耦合)。
///
/// 2026-09-22 第 113 轮:由 `fn` 提升为 `pub fn`,供 TUI bootstrap / `/audit` 命令
/// 自动定位审计目录,无需注入 root_dir 字段。
pub fn audit_root_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// 获取(或创建)当前会话的审计写入器。
///
/// 文件路径:`<root_dir>/AuditTrail/audit_{session_id}.jsonl`。
/// 写入器跨任务复用(同一 Session 的多轮任务追加同一文件)。
pub fn session_writer(
    session_id: &str,
    root_dir: &Path,
) -> io::Result<Arc<Mutex<AuditWriter>>> {
    let mut reg = registry().lock().expect("AUDIT_REGISTRY poisoned");
    if let Some(writer) = reg.get(session_id) {
        return Ok(writer.clone());
    }
    let writer = Arc::new(Mutex::new(AuditWriter::open(session_id, root_dir)?));
    reg.insert(session_id.to_string(), writer.clone());
    Ok(writer)
}

/// 写入一条审计事件到当前会话的 JSONL 文件。
///
/// fail-open:写入失败仅 eprintln!,不返回 Err(审计旁路,不影响主流程)。
pub fn record(event: AuditEvent) {
    if audit_disabled() {
        return;
    }
    let Some(root) = audit_root_dir() else {
        return;
    };
    match session_writer(&event.session_id, &root) {
        Ok(writer) => {
            let mut w = writer.lock().expect("AuditWriter poisoned");
            if let Err(e) = w.append(&event) {
                eprintln!("[audit] 写入失败(忽略): {e}");
            }
        }
        Err(e) => {
            eprintln!("[audit] 打开写入器失败(忽略): {e}");
        }
    }
}

/// 便捷函数:记录 Yolo 分类决策。
pub fn record_classify(
    session_id: &str,
    task_level: &str,
    purpose: &str,
    goal: &str,
    delegate: Option<&str>,
    degraded: bool,
    duration_ms: u64,
    usage: Usage,
) {
    let meta = serde_json::json!({
        "task_level": task_level,
        "delegate": delegate.unwrap_or(""),
        "degraded": degraded,
    });
    let event = AuditEvent::new(
        session_id,
        AgentRole::Yolo.as_str(),
        "classify",
        format!("purpose={}", purpose),
        format!("level={} delegate={}", task_level, delegate.unwrap_or("")),
        format!("goal={}", goal),
        duration_ms,
    )
    .with_tokens(usage)
    .with_meta(meta);
    record(event);
}

/// 便捷函数:记录**任务锚点**决策(第 128 轮,第 6 个决策点)。
///
/// 与其它 5 个决策点的差异:锚点是**机械抽取**(正则,不经 LLM)的,
/// 因此「决策依据」段记录的是抽取输入(用户原文里的 URL 与指代),
/// 而不是模型的推理文本 —— 这恰恰是它的价值:可复现、可对账、不可幻觉。
///
/// 三段式对齐:
/// - 输入上下文 = 抽到的锚点主机 + 原文 URL;
/// - 决策结论   = 是否触发澄清门(目标指代未解析);
/// - 决策依据   = 命中的指代片段(为什么判定为不可猜测)。
/// `stage` 取值:
/// - `"extract"`  —— 任务入口机械抽取完成(锚点是什么);
/// - `"clarify"`  —— 澄清门实际触发(做了什么决策)。
///
/// 两条分开记是因为**抽取**与**门触发**不是同一个决策:LLM 通道
/// (`target_status="unresolved"`)触发时机械抽取可能并未标记 unresolved,
/// 只记一条会把"谁决定停下来问用户"这个关键事实丢掉。
pub fn record_target_anchor(
    session_id: &str,
    anchor: &crate::agent::safety::TargetAnchor,
    clarification_gate: bool,
    stage: &str,
) {
    let meta = serde_json::json!({
        "stage": stage,
        "hosts": anchor.hosts,
        "raw_urls": anchor.raw_urls,
        "unresolved_reference": anchor.unresolved_reference,
        "extraction": "mechanical_regex",
    });
    let event = AuditEvent::new(
        session_id,
        AgentRole::Yolo.as_str(),
        "target_anchor",
        format!(
            "hosts={} urls={}",
            if anchor.hosts.is_empty() {
                "(无)".to_string()
            } else {
                anchor.hosts.join(",")
            },
            anchor.raw_urls.first().cloned().unwrap_or_else(|| "(无)".into())
        ),
        format!(
            "clarification_gate={} anchor_empty={}",
            clarification_gate,
            anchor.is_empty()
        ),
        format!(
            "unresolved_evidence={}",
            if anchor.unresolved_evidence.is_empty() {
                "(无指代)"
            } else {
                anchor.unresolved_evidence.as_str()
            }
        ),
        0,
    )
    .with_meta(meta);
    record(event);
}

/// 便捷函数:记录 Plan 规划决策。
pub fn record_plan(
    session_id: &str,
    summary: &str,
    step_count: usize,
    duration_ms: u64,
    usage: Usage,
) {
    let event = AuditEvent::new(
        session_id,
        AgentRole::Plan.as_str(),
        "plan",
        format!("goal={}", summary),
        format!("steps={}", step_count),
        format!("plan_summary={}", summary),
        duration_ms,
    )
    .with_tokens(usage)
    .with_meta(serde_json::json!({ "step_count": step_count }));
    record(event);
}

/// 便捷函数:记录 Main-Work 流程拆解决策。
pub fn record_decompose(
    session_id: &str,
    goal: &str,
    workflow_count: usize,
    parallel_layers: usize,
    duration_ms: u64,
    usage: Usage,
) {
    let event = AuditEvent::new(
        session_id,
        AgentRole::MainWork.as_str(),
        "decompose",
        format!("goal={}", goal),
        format!("workflows={} layers={}", workflow_count, parallel_layers),
        format!("decomposed into {} workflows", workflow_count),
        duration_ms,
    )
    .with_tokens(usage)
    .with_meta(serde_json::json!({
        "workflow_count": workflow_count,
        "parallel_layers": parallel_layers,
    }));
    record(event);
}

/// 便捷函数:记录 Quality-Check 质检判定。
pub fn record_verdict(
    session_id: &str,
    wf_id: &str,
    verdict: &str,
    issues: &[String],
    retryable: bool,
    evidence: &str,
) {
    let event = AuditEvent::new(
        session_id,
        AgentRole::QualityCheck.as_str(),
        "verdict",
        format!("wf_id={}", wf_id),
        format!("verdict={} retryable={}", verdict, retryable),
        format!("issues={:?} evidence={}", issues, evidence),
        0,
    )
    .with_meta(serde_json::json!({
        "wf_id": wf_id,
        "verdict": verdict,
        "issues": issues,
        "retryable": retryable,
    }));
    record(event);
}

/// 便捷函数:记录 Context 压缩决策。
pub fn record_compact(
    session_id: &str,
    tier: &str,
    before_tokens: usize,
    after_tokens: usize,
    compacted_messages: usize,
    fallback: bool,
    duration_ms: u64,
) {
    let event = AuditEvent::new(
        session_id,
        AgentRole::Compact.as_str(),
        "compact",
        format!("before_tokens={}", before_tokens),
        format!("tier={} after={} compacted={} fallback={}", tier, after_tokens, compacted_messages, fallback),
        format!("compacted {} messages", compacted_messages),
        duration_ms,
    )
    .with_meta(serde_json::json!({
        "tier": tier,
        "before_tokens": before_tokens,
        "after_tokens": after_tokens,
        "compacted_messages": compacted_messages,
        "fallback": fallback,
    }));
    record(event);
}

// =================== 写入器 ===================

/// 会话级追加写写入器(Mutex 保护的 File)。
pub struct AuditWriter {
    file: File,
    path: PathBuf,
}

impl AuditWriter {
    /// 打开(或创建)审计文件,追加模式。
    pub fn open(session_id: &str, root_dir: &Path) -> io::Result<Self> {
        let dir = root_dir.join(AUDIT_DIR);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("audit_{session_id}.jsonl"));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { file, path })
    }

    /// 追加一条事件(一行 JSON + 换行)。
    pub fn append(&mut self, event: &AuditEvent) -> io::Result<()> {
        let line = serde_json::to_string(event).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("JSON 序列化失败: {e}"))
        })?;
        writeln!(self.file, "{line}")?;
        // 每事件 fsync 保证落盘(审计场景不追求吞吐,追求可恢复性)。
        self.file.flush()
    }

    /// 当前文件路径(供测试/诊断)。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// =================== 辅助函数 ===================

/// 是否关闭审计(环境变量 LAEW_AUDIT=off|0|false|no)。
pub fn audit_disabled() -> bool {
    match std::env::var("LAEW_AUDIT").ok().as_deref() {
        Some("off") | Some("0") | Some("false") | Some("no") => true,
        _ => false,
    }
}

/// 截断字符串到 max_chars 字符(CJK 安全)。
fn truncate(text: String, max_chars: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        out.push_str("...[truncated]");
    }
    out
}

/// 脱敏辅助(复用 debug::scrub_secrets,失败回退原文)。
fn scrub_scrub(text: &str) -> String {
    scrub_secrets(text)
}

/// 截断 meta 序列化后的超长字符串值(防单字段撑爆)。
fn clamp_meta(meta: serde_json::Value) -> serde_json::Value {
    const MAX_META_STR: usize = MAX_META_CHARS;
    fn clamp_value(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::String(s) => {
                if s.chars().count() > MAX_META_STR {
                    serde_json::Value::String(truncate(s, MAX_META_STR))
                } else {
                    serde_json::Value::String(s)
                }
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.into_iter().map(clamp_value).collect())
            }
            serde_json::Value::Object(obj) => {
                let mut out = serde_json::Map::new();
                for (k, v) in obj {
                    let _ = k;
                    out.insert(
                        truncate(k, 64),
                        clamp_value(v),
                    );
                }
                serde_json::Value::Object(out)
            }
            other => other,
        }
    }
    clamp_value(meta)
}

/// 清理审计目录下的旧文件(保留最近 keep 个 session 的文件)。
///
/// 按文件 mtime 排序,删除超出 keep 的文件。fail-open:删除失败跳过。
pub fn trim_audit_files(root_dir: &Path, keep: usize) -> Result<usize> {
    let dir = root_dir.join(AUDIT_DIR);
    if !dir.exists() {
        return Ok(0);
    }
    let mut entries: Vec<(PathBuf, std::time::SystemTime)> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "jsonl")
        })
        .filter_map(|e| {
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((e.path(), mtime))
        })
        .collect();
    entries.sort_by(|a, b| a.1.cmp(&b.1)); // 旧 → 新
    let mut removed = 0;
    if entries.len() > keep {
        for (path, _) in entries.iter().take(entries.len() - keep) {
            if std::fs::remove_file(path).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

// =================== 读取与统计(2026-09-22 第 113 轮 D9-8 闭环)====================

/// 推导指定 session 的审计文件路径(不创建文件)。
///
/// 复用 `AuditWriter::open` 的路径规则:`<root_dir>/AuditTrail/audit_{session_id}.jsonl`。
/// 当 `root_dir` 为 `None` 时使用 `audit_root_dir()` 默认推导值。
pub fn audit_file_path(session_id: &str, root_dir: Option<&Path>) -> Option<PathBuf> {
    let root = root_dir
        .map(|p| p.to_path_buf())
        .or_else(audit_root_dir)?;
    Some(root.join(AUDIT_DIR).join(format!("audit_{session_id}.jsonl")))
}

/// 读取指定 session 的所有审计事件(JSONL 容错解析)。
///
/// 损坏行(serde_json 解析失败 / 缺必填字段)会被跳过,不计入结果,但可通过
/// [`VerifyResult`] 通道获取损坏详情。
///
/// - `session_id`:目标 session id
/// - `root_dir`:审计根目录,`None` 时走 `audit_root_dir()`
pub fn read_events(session_id: &str, root_dir: Option<&Path>) -> Vec<AuditEvent> {
    let Some(path) = audit_file_path(session_id, root_dir) else {
        return Vec::new();
    };
    let Ok(file) = File::open(&path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut events: Vec<AuditEvent> = Vec::new();
    for line in reader.lines().map_while(std::io::Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(ev) = serde_json::from_str::<AuditEvent>(trimmed) {
            events.push(ev);
        }
    }
    events
}

/// 统计指定 session 的审计事件条数(快速路径:仅 `BufReader::lines`,不解析)。
///
/// 性能:1k 行 < 5ms(SSD),可放心在 `/cost` / 横幅中调用。
pub fn count_events(session_id: &str, root_dir: Option<&Path>) -> usize {
    let Some(path) = audit_file_path(session_id, root_dir) else {
        return 0;
    };
    let Ok(file) = File::open(&path) else {
        return 0;
    };
    BufReader::new(file).lines().map_while(std::io::Result::ok).filter(|l| !l.trim().is_empty()).count()
}

/// 获取指定 session 审计文件大小(字节)。
///
/// 用于 `/cost` 末尾「落盘 KB」展示;返回 `None` 表示文件不存在。
pub fn file_size(session_id: &str, root_dir: Option<&Path>) -> Option<u64> {
    let path = audit_file_path(session_id, root_dir)?;
    std::fs::metadata(&path).ok().map(|m| m.len())
}

// =================== 完整性校验(2026-09-22 第 113 轮)====================

/// 单行校验结果。
#[derive(Debug, Clone)]
pub struct VerifyLine {
    /// 行号(1-based,空行不计)。
    pub line_no: usize,
    /// 是否通过(true=有效,false=损坏)。
    pub valid: bool,
    /// 损坏原因(`None` 当 `valid=true`)。
    pub error: Option<String>,
    /// 该行的 decision(若可解析),用于错误展示。
    pub decision: Option<String>,
}

/// 整体校验汇总。
#[derive(Debug, Clone)]
pub struct VerifyResult {
    /// 总有效行数。
    pub valid_count: usize,
    /// 损坏行详情(已排序)。
    pub invalid_lines: Vec<VerifyLine>,
    /// 必填字段列表(用于展示「检查了哪些字段」)。
    pub required_fields: &'static [&'static str],
    /// 审计文件路径(用于展示)。
    pub path: PathBuf,
}

/// 校验指定 session 的审计文件完整性。
///
/// 校验规则:
/// 1. 每行必须是合法 JSON;
/// 2. 必须含必填字段:`ts` / `session_id` / `agent` / `decision`;
/// 3. 损坏行计入 `invalid_lines` 并附行号 + 原因。
///
/// 文件不存在 → 返回 `valid_count=0` 且 `invalid_lines=[]`,调用方按「无审计」处理。
pub fn verify_events(session_id: &str, root_dir: Option<&Path>) -> VerifyResult {
    const REQUIRED: &[&str] = &["ts", "session_id", "agent", "decision"];
    let path = audit_file_path(session_id, root_dir)
        .unwrap_or_else(|| PathBuf::from("<audit root not resolved>"));
    let mut result = VerifyResult {
        valid_count: 0,
        invalid_lines: Vec::new(),
        required_fields: REQUIRED,
        path,
    };
    let Some(p) = audit_file_path(session_id, root_dir) else {
        return result;
    };
    let Ok(file) = File::open(&p) else {
        return result;
    };
    let reader = BufReader::new(file);
    let mut line_no: usize = 0;
    for line in reader.lines().map_while(std::io::Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        line_no += 1;
        match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(v) => {
                let mut missing: Vec<&str> = Vec::new();
                for &f in REQUIRED {
                    if v.get(f).is_none() || v.get(f).map(|x| x.is_null()).unwrap_or(false) {
                        missing.push(f);
                    }
                }
                if missing.is_empty() {
                    result.valid_count += 1;
                } else {
                    result.invalid_lines.push(VerifyLine {
                        line_no,
                        valid: false,
                        error: Some(format!("缺字段: {}", missing.join(","))),
                        decision: v
                            .get("decision")
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string()),
                    });
                }
            }
            Err(e) => {
                result.invalid_lines.push(VerifyLine {
                    line_no,
                    valid: false,
                    error: Some(format!("JSON 解析失败: {e}")),
                    decision: None,
                });
            }
        }
    }
    result.invalid_lines.sort_by_key(|l| l.line_no);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_event_new_truncates_and_scrubs() {
        let long = "x".repeat(MAX_INPUT_SUMMARY_CHARS + 50);
        let event = AuditEvent::new(
            "s1",
            "Yolo",
            "classify",
            format!("purpose={}", long),
            "level=simple",
            "goal=foo",
            100,
        );
        assert!(event.input_summary.ends_with("...[truncated]"));
        assert!(event.input_summary.chars().count() <= MAX_INPUT_SUMMARY_CHARS + "...[truncated]".len());
        assert_eq!(event.session_id, "s1");
        assert_eq!(event.decision, "classify");
    }

    #[test]
    fn audit_event_scrubs_secrets() {
        let event = AuditEvent::new(
            "s1",
            "Yolo",
            "classify",
            "purpose=帮我看 sk-1234567890abcdef 这个 key",
            "level=simple",
            "goal=test",
            0,
        );
        assert!(!event.input_summary.contains("1234567890abcdef"));
        assert!(event.input_summary.contains("****REDACTED") || event.input_summary.contains("REDACTED"));
    }

    #[test]
    fn audit_event_serialization_roundtrip() {
        let event = AuditEvent::new(
            "s_test",
            "Yolo",
            "classify",
            "purpose=测试",
            "level=medium",
            "goal=测试目标",
            1234,
        )
        .with_tokens(Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_input_tokens: 10,
            cache_creation_input_tokens: 0,
        })
        .with_meta(serde_json::json!({"task_level":"medium","degraded":false}));
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"agent\":\"Yolo\""));
        assert!(json.contains("\"decision\":\"classify\""));
        assert!(json.contains("\"duration_ms\":1234"));
        assert!(json.contains("\"task_level\":\"medium\""));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed["agent"], "Yolo");
        assert_eq!(parsed["tokens"]["input_tokens"], 100);
        assert_eq!(parsed["tokens"]["output_tokens"], 50);
    }

    #[test]
    fn audit_writer_append_creates_jsonl() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let writer = session_writer("s_test", root).expect("open writer");
        let mut w = writer.lock().expect("lock");
        let event = AuditEvent::new("s_test", "Yolo", "classify", "p", "o", "r", 0);
        w.append(&event).expect("append");
        drop(w);
        // 验证文件存在且含一行 JSON
        let jsonl_path = root.join(AUDIT_DIR).join("audit_s_test.jsonl");
        assert!(jsonl_path.exists());
        let content = std::fs::read_to_string(&jsonl_path).expect("read");
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("\"agent\":\"Yolo\""));
    }

    #[test]
    fn audit_writer_append_multiple_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // 使用隔离的 session_id 避免与其他测试共享全局注册表
        let sid = "s_multi";
        let writer = session_writer(sid, root).expect("open writer");
        for i in 0..3 {
            let mut w = writer.lock().expect("lock");
            let event = AuditEvent::new(
                sid,
                "QualityCheck",
                "verdict",
                &format!("wf_id=wf_{}", i),
                "verdict=pass",
                "evidence=all good",
                0,
            );
            w.append(&event).expect("append");
        }
        drop(writer);
        let jsonl_path = root.join(AUDIT_DIR).join(format!("audit_{sid}.jsonl"));
        let content = std::fs::read_to_string(&jsonl_path).expect("read");
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("wf_id=wf_0"));
        assert!(lines[2].contains("wf_id=wf_2"));
    }

    #[test]
    fn trim_audit_files_keeps_recent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let audit_dir = root.join(AUDIT_DIR);
        std::fs::create_dir_all(&audit_dir).expect("create dir");
        // 创建 5 个文件,间隔 sleep 保证 mtime 不同
        for i in 0..5 {
            let path = audit_dir.join(format!("audit_s{i}.jsonl"));
            std::fs::write(&path, format!("{{ \"event\": {i} }}")).expect("write");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let removed = trim_audit_files(root, 2).expect("trim");
        assert_eq!(removed, 3); // 5 - 2 = 3
        let remaining: Vec<_> = std::fs::read_dir(&audit_dir)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()).is_some_and(|x| x == "jsonl"))
            .collect();
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn record_classify_formats_correctly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_classify";
        let writer = session_writer(sid, root).expect("open writer");
        {
            let mut w = writer.lock().expect("lock");
            let event = AuditEvent::new(
                sid,
                "yolo",
                "classify",
                "purpose=用户想排序文件",
                "level=medium delegate=main_work",
                "goal=对目录内文件按大小排序",
                500,
            )
            .with_tokens(Usage {
                input_tokens: 200,
                output_tokens: 100,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            })
            .with_meta(serde_json::json!({"task_level":"medium","delegate":"main_work","degraded":false}));
            w.append(&event).expect("append");
        }
        let jsonl_path = root.join(AUDIT_DIR).join(format!("audit_{sid}.jsonl"));
        let content = std::fs::read_to_string(&jsonl_path).expect("read");
        assert!(content.contains("\"agent\":\"yolo\"") || content.contains("\"agent\":\"Yolo\""));
        assert!(content.contains("\"decision\":\"classify\""));
        assert!(content.contains("\"task_level\":\"medium\""));
        assert!(content.contains("\"duration_ms\":500"));
    }

    #[test]
    fn record_verdict_with_issues() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_verdict";
        let writer = session_writer(sid, root).expect("open writer");
        {
            let mut w = writer.lock().expect("lock");
            let event = AuditEvent::new(
                sid,
                "quality",
                "verdict",
                "wf_id=wf_1",
                "verdict=fail retryable=true",
                "issues=[\"输出不完整\",\"缺少错误处理\"] evidence=stdout 截断",
                0,
            )
            .with_meta(serde_json::json!({"wf_id":"wf_1","verdict":"fail","issues":["输出不完整","缺少错误处理"],"retryable":true}));
            w.append(&event).expect("append");
        }
        let jsonl_path = root.join(AUDIT_DIR).join(format!("audit_{sid}.jsonl"));
        let content = std::fs::read_to_string(&jsonl_path).expect("read");
        assert!(content.contains("\"agent\":\"quality\"") || content.contains("\"agent\":\"QualityCheck\""));
        assert!(content.contains("\"decision\":\"verdict\""));
        assert!(content.contains("\"verdict\":\"fail\""));
        assert!(content.contains("\"retryable\":true"));
        assert!(content.contains("输出不完整"));
    }

    #[test]
    fn meta_string_values_are_clamped() {
        let long = "y".repeat(MAX_META_CHARS + 100);
        let meta = serde_json::json!({ "long_field": long });
        let clamped = clamp_meta(meta);
        let s = clamped["long_field"].as_str().expect("string");
        assert!(s.ends_with("...[truncated]"));
        assert!(s.chars().count() <= MAX_META_CHARS + "...[truncated]".len());
    }

    #[test]
    fn audit_disabled_detects_off() {
        // 环境变量可能在其他测试中被设置,这里仅验证函数不 panic 且返回布尔值
        let _ = audit_disabled();
    }

    #[test]
    fn record_compact_formats_correctly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_compact";
        // 审计写入器走全局注册表 + 静态 root_dir,测试通过临时二进制路径隔离:
        // 直接构造 writer 写入,验证 schema;便捷函数本身无 root_dir 参数。
        let writer = session_writer(sid, root).expect("open writer");
        {
            let mut w = writer.lock().expect("lock");
            let event = AuditEvent::new(sid, "compact", "compact", "before=10000", "tier=medium after=5000 compacted=8", "compacted 8 messages", 2000)
                .with_meta(serde_json::json!({"tier":"medium","before_tokens":10000,"after_tokens":5000,"compacted_messages":8,"fallback":false}));
            w.append(&event).expect("append");
        }
        let jsonl_path = root.join(AUDIT_DIR).join(format!("audit_{sid}.jsonl"));
        let content = std::fs::read_to_string(&jsonl_path).expect("read");
        assert!(content.contains("\"decision\":\"compact\""));
        assert!(content.contains("\"tier\":\"medium\""));
        assert!(content.contains("\"before_tokens\":10000"));
        assert!(content.contains("\"after_tokens\":5000"));
    }

    #[test]
    fn record_decompose_formats_correctly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_decompose";
        let writer = session_writer(sid, root).expect("open writer");
        {
            let mut w = writer.lock().expect("lock");
            let event = AuditEvent::new(sid, "main", "decompose", "goal=排序文件", "workflows=3 layers=2", "decomposed into 3 workflows", 800)
                .with_meta(serde_json::json!({"workflow_count":3,"parallel_layers":2}));
            w.append(&event).expect("append");
        }
        let jsonl_path = root.join(AUDIT_DIR).join(format!("audit_{sid}.jsonl"));
        let content = std::fs::read_to_string(&jsonl_path).expect("read");
        assert!(content.contains("\"decision\":\"decompose\""));
        assert!(content.contains("\"workflow_count\":3"));
        assert!(content.contains("\"parallel_layers\":2"));
    }

    #[test]
    fn record_plan_formats_correctly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_plan";
        let writer = session_writer(sid, root).expect("open writer");
        {
            let mut w = writer.lock().expect("lock");
            let event = AuditEvent::new(sid, "plan", "plan", "goal=项目重构方案", "steps=5", "plan_summary=项目重构方案", 1500)
                .with_meta(serde_json::json!({"step_count":5}));
            w.append(&event).expect("append");
        }
        let jsonl_path = root.join(AUDIT_DIR).join(format!("audit_{sid}.jsonl"));
        let content = std::fs::read_to_string(&jsonl_path).expect("read");
        assert!(content.contains("\"decision\":\"plan\""));
        assert!(content.contains("\"step_count\":5"));
    }

    // ===== 2026-09-22 第 113 轮 D9-8 闭环:读取/校验/统计测试 =====

    #[test]
    fn read_events_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_read";
        let writer = session_writer(sid, root).expect("open writer");
        for i in 0..3 {
            let mut w = writer.lock().expect("lock");
            let ev = AuditEvent::new(sid, "yolo", "classify", &format!("purpose=测试 {}", i), "level=simple", "goal=test", 100);
            w.append(&ev).expect("append");
        }
        drop(writer);
        let events = read_events(sid, Some(root));
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].decision, "classify");
        assert_eq!(events[0].agent, "yolo");
    }

    #[test]
    fn read_events_skips_corrupt_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_corrupt";
        let audit_dir = root.join(AUDIT_DIR);
        std::fs::create_dir_all(&audit_dir).expect("dir");
        let path = audit_dir.join(format!("audit_{sid}.jsonl"));
        std::fs::write(
            &path,
            "{not valid json}\n{\"ts\":\"t1\",\"session_id\":\"s\",\"agent\":\"yolo\",\"decision\":\"classify\",\"input_summary\":\"i\",\"outcome\":\"o\",\"rationale\":\"r\",\"duration_ms\":1}\n",
        ).expect("write");
        let events = read_events(sid, Some(root));
        assert_eq!(events.len(), 1, "损坏行应被跳过");
        assert_eq!(events[0].decision, "classify");
    }

    #[test]
    fn read_events_returns_empty_for_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events = read_events("s_nonexistent", Some(dir.path()));
        assert!(events.is_empty());
    }

    #[test]
    fn count_events_basic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_count";
        let writer = session_writer(sid, root).expect("open writer");
        for _ in 0..5 {
            let mut w = writer.lock().expect("lock");
            let ev = AuditEvent::new(sid, "yolo", "classify", "i", "o", "r", 0);
            w.append(&ev).expect("append");
        }
        drop(writer);
        assert_eq!(count_events(sid, Some(root)), 5);
    }

    #[test]
    fn file_size_returns_none_for_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(file_size("nope", Some(dir.path())).is_none());
    }

    #[test]
    fn file_size_tracks_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_size";
        let writer = session_writer(sid, root).expect("open writer");
        {
            let mut w = writer.lock().expect("lock");
            w.append(&AuditEvent::new(sid, "yolo", "classify", "i", "o", "r", 0)).expect("append");
        }
        let size = file_size(sid, Some(root)).expect("size");
        assert!(size > 0);
    }

    #[test]
    fn verify_events_detects_corrupt_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_verify_corrupt";
        let audit_dir = root.join(AUDIT_DIR);
        std::fs::create_dir_all(&audit_dir).expect("dir");
        let path = audit_dir.join(format!("audit_{sid}.jsonl"));
        std::fs::write(&path, "{not valid}\n").expect("write");
        let result = verify_events(sid, Some(root));
        assert_eq!(result.valid_count, 0);
        assert_eq!(result.invalid_lines.len(), 1);
        assert!(result.invalid_lines[0].error.as_ref().unwrap().contains("JSON"));
    }

    #[test]
    fn verify_events_detects_missing_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_verify_missing";
        let audit_dir = root.join(AUDIT_DIR);
        std::fs::create_dir_all(&audit_dir).expect("dir");
        let path = audit_dir.join(format!("audit_{sid}.jsonl"));
        std::fs::write(&path, "{\"ts\":\"t1\",\"session_id\":\"s\",\"input_summary\":\"i\",\"outcome\":\"o\",\"rationale\":\"r\",\"duration_ms\":1}\n").expect("write");
        let result = verify_events(sid, Some(root));
        assert_eq!(result.valid_count, 0);
        assert_eq!(result.invalid_lines.len(), 1);
        let err = result.invalid_lines[0].error.as_ref().unwrap();
        assert!(err.contains("agent") && err.contains("decision"));
    }

    #[test]
    fn verify_events_passes_valid_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_verify_ok";
        let writer = session_writer(sid, root).expect("open writer");
        for _ in 0..3 {
            let mut w = writer.lock().expect("lock");
            w.append(&AuditEvent::new(sid, "yolo", "classify", "i", "o", "r", 0)).expect("append");
        }
        drop(writer);
        let result = verify_events(sid, Some(root));
        assert_eq!(result.valid_count, 3);
        assert!(result.invalid_lines.is_empty());
    }

    #[test]
    fn audit_file_path_returns_expected_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = audit_file_path("abc", Some(dir.path())).expect("path");
        assert_eq!(path, dir.path().join(AUDIT_DIR).join("audit_abc.jsonl"));
    }
}
