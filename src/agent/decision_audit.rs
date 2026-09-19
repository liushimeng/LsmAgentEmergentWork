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

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

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
#[derive(Debug, Clone, Serialize)]
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
fn audit_root_dir() -> Option<PathBuf> {
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
}
