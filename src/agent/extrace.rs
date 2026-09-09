//! SubAgent / 任何 Agent 单元的执行轨迹(ExecutionTrace)。
//!
//! 单元结束后作为返回值的一部分,供:
//! - Quality-Check 拿到真实执行证据辅助判据
//! - Agent-Memory 持久化可观测的执行元数据
//! - Orchestrator 失败回流 / Yolo 重新评估时引用具体失败模式
//!
//! 设计见 `tmpPlan/2026-09-09_05-SubAgent执行轨迹与多维失败检测方案.md`。

use serde::{Deserialize, Serialize};

/// 执行轨迹:SubAgent 单元(或其他 Agent 单元)一次 `run_session` 的可观测元数据。
///
/// 字段按"低成本 / 高信号"原则选取 —— 全部为同步计数 / 标志位,不增加 LLM 调用。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionTrace {
    /// LLM 调用总轮数(iter 计数:实际进入 LLM 调用的次数)
    pub iterations: usize,
    /// 工具调用总次数(成功 + 失败)
    pub tool_calls: usize,
    /// 工具调用成功次数
    pub tool_calls_ok: usize,
    /// 工具调用失败次数(`is_error=true`)
    pub tool_calls_err: usize,
    /// 连续相同失败键的最大长度(0 表示未触发早终止)
    pub max_consecutive_failures: usize,
    /// 是否触发 `RepeatedToolFailure` / `MaxIterationsExceeded` 早终止
    pub early_terminated: bool,
    /// 早终止原因(自由文本,与 `AgentError::RepeatedToolFailure` 对齐)
    pub early_terminate_reason: String,
    /// 截断续接次数(>0 表示 LLM 输出被 max_tokens 截断并自动续接过)
    pub truncation_resumes: usize,
    /// 上下文溢出自动恢复次数(L1038/L1044:排水/折叠后重试成功;
    /// >0 表示发生过 prompt-too-long 类溢出并本地恢复)
    pub overflow_recoveries: usize,
    /// max_tokens 静默升级次数(2026-09-09 第 09 轮,实现 L1037)。
    ///
    /// 0 表示本会话 LLM 输出从未被 max_tokens 截断;>0 表示发生过 8K→16K→32K→64K 的
    /// 翻倍升级。写入 Agent-Memory 供后续 Session 决策参考。
    pub max_tokens_upscalings: usize,
    /// max_tokens 升级历史 `(old, new)`,供 Debug Report 调试使用。
    /// 当前实现由 Agent 循环在 finalize 阶段从状态机注入。
    pub max_tokens_history: Vec<(u32, u32)>,
    /// 结构化输出通道命中次数(L6/L19,2026-09-09 第 13 轮):
    /// 模型经 forced tool_choice 以 tool_use 形式提交结果的次数。
    /// >0 表示协议级结构化输出链路生效(优于文本 JSON 启发式解析)。
    pub structured_emits: usize,
    /// 最终输出文本字节数
    pub output_bytes: usize,
    /// 失败模式标签(供 Agent-Memory 索引 / Yolo 失败回流引用)
    pub failure_signals: Vec<String>,
}

impl ExecutionTrace {
    /// 计算失败模式标签:基于当前指标 + 最终文本启发式。
    /// 文本失败措辞部分沿用 `subagent::looks_like_failure` 的中英双语关键词。
    pub fn collect_failure_signals(&mut self, text: &str) {
        let mut signals = Vec::new();

        // 1) 早终止强信号
        if self.early_terminated {
            let reason = self
                .early_terminate_reason
                .chars()
                .take(40)
                .collect::<String>();
            signals.push(format!("early_terminate:{reason}"));
        }

        // 2) 截断信号(token 上限触发自动续接 ≥ 1 次)
        if self.truncation_resumes > 0 {
            signals.push(format!("truncated:{}x", self.truncation_resumes));
        }

        // 2.5) 溢出恢复信号(上下文溢出被排水/折叠本地恢复 ≥ 1 次;恢复成功不算失败)
        if self.overflow_recoveries > 0 {
            signals.push(format!("overflow_recovered:{}x", self.overflow_recoveries));
        }

        // 3) 工具失败率信号(失败占比 ≥ 50%)
        if self.tool_calls > 0 {
            let err_rate = self.tool_calls_err * 100 / self.tool_calls;
            if err_rate >= 50 {
                signals.push(format!(
                    "high_error_rate:{}/{}",
                    self.tool_calls_err, self.tool_calls
                ));
            }
        }

        // 4) 文本失败措辞信号(LLM 自由输出包含失败关键词)
        if looks_like_failure_text(text) {
            signals.push("text_failure_phrase".into());
        }

        // 5) 没有命中任何失败信号时记 "ok" 占位,便于下游聚合
        if signals.is_empty() {
            signals.push("ok".into());
        }
        self.failure_signals = signals;
    }

    /// 综合失败判定:任一强信号即视为失败。
    pub fn is_failed(&self) -> bool {
        self.failure_signals.iter().any(|s| {
            s.starts_with("early_terminate:")
                || s.starts_with("high_error_rate:")
                || s == "text_failure_phrase"
        })
    }

    /// 把 trace 渲染成 QC prompt 可用的紧凑 Markdown(≤ 8 行,防止膨胀 prompt)。
    pub fn render_prompt(&self) -> String {
        format!(
            "- iterations={} tool_calls={}(ok={},err={}) max_consec={}\n\
             - early_terminated={} truncation_resumes={} overflow_recoveries={}\n\
             - output_bytes={}\n\
             - failure_signals=[{}]",
            self.iterations,
            self.tool_calls,
            self.tool_calls_ok,
            self.tool_calls_err,
            self.max_consecutive_failures,
            self.early_terminated,
            self.truncation_resumes,
            self.overflow_recoveries,
            self.output_bytes,
            self.failure_signals.join(","),
        )
    }
}

/// 提取 `subagent::looks_like_failure` 的核心文本判断,
/// 供 `ExecutionTrace::collect_failure_signals` 复用(避免循环依赖)。
///
/// 保留 2026-09-08 第 02 轮的中英双语 + 大小写不敏感 + 前缀空白容忍语义。
fn looks_like_failure_text(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let lower = t.to_lowercase();
    const EN_PATTERNS: &[&str] = &[
        "[failure]",
        "[failed]",
        "[error]",
        "failed:",
        "error:",
        "exception:",
        "fatal error",
        "panic:",
        "crash:",
        "aborted:",
        "killed:",
    ];
    if EN_PATTERNS.iter().any(|p| lower.contains(p)) {
        return true;
    }
    const ZH_PATTERNS: &[&str] = &[
        "[失败]",
        "执行失败",
        "未完成",
        "未能",
        "无法完成",
        "无法",
        "异常退出",
        "出错了",
    ];
    if ZH_PATTERNS.iter().any(|p| t.contains(p)) {
        return true;
    }
    false
}

/// 把工具入参(`serde_json::Value`)压缩成短摘要字符串,用于无文本收敛短路时
/// 渲染「最近工具调用历史」叙事化摘要(关联报告 2026-09-09_06 F-002)。
///
/// 设计要点:
/// - 仅取 1~3 个核心字段的值,避免长字符串(如 Read 的 `file_path`)撑爆单行;
/// - 长字符串截短到 60 字符,防止 history 摘要超过上下文预算;
/// - 空对象 → `"{}"`;非法 JSON → 原样转字符串并截短。
pub fn compact_args_digest(args: &serde_json::Value) -> String {
    use serde_json::Value;
    const MAX_FRAGMENT: usize = 60;
    fn trunc(s: &str, n: usize) -> String {
        if s.chars().count() <= n {
            s.to_string()
        } else {
            let head: String = s.chars().take(n).collect();
            format!("{head}…")
        }
    }
    match args {
        Value::Object(map) => {
            // 优先按 schema 的常见字段顺序: file_path → path → command → pattern → content
            const PREFERRED: &[&str] = &[
                "file_path",
                "path",
                "command",
                "pattern",
                "content",
                "old_string",
                "url",
            ];
            let mut fragments: Vec<String> = Vec::new();
            'outer: for k in PREFERRED {
                if let Some(v) = map.get(*k) {
                    fragments.push(format!(
                        "{}={}",
                        k,
                        trunc(&value_to_compact(v), MAX_FRAGMENT)
                    ));
                    if fragments.len() >= 3 {
                        break 'outer;
                    }
                }
            }
            // 不足 3 个时再补其它字段
            if fragments.len() < 3 {
                for (k, v) in map.iter() {
                    if PREFERRED.contains(&k.as_str()) {
                        continue;
                    }
                    fragments.push(format!(
                        "{}={}",
                        k,
                        trunc(&value_to_compact(v), MAX_FRAGMENT)
                    ));
                    if fragments.len() >= 3 {
                        break;
                    }
                }
            }
            if fragments.is_empty() {
                "{}".to_string()
            } else {
                fragments.join(" ")
            }
        }
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => trunc(s, MAX_FRAGMENT),
        Value::Array(arr) => format!("[{} items]", arr.len()),
    }
}

fn value_to_compact(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_trace_has_no_failure_signals() {
        let mut t = ExecutionTrace::default();
        t.collect_failure_signals("正常输出");
        assert_eq!(t.failure_signals, vec!["ok"]);
        assert!(!t.is_failed());
    }

    #[test]
    fn early_terminate_is_strong_signal() {
        let mut t = ExecutionTrace::default();
        t.early_terminated = true;
        t.early_terminate_reason = "tool=Read attempts=3".into();
        t.collect_failure_signals("ok");
        assert!(t
            .failure_signals
            .iter()
            .any(|s| s.starts_with("early_terminate:")));
        assert!(t.is_failed());
    }

    #[test]
    fn high_error_rate_is_strong_signal() {
        let mut t = ExecutionTrace::default();
        t.tool_calls = 4;
        t.tool_calls_ok = 1;
        t.tool_calls_err = 3;
        t.collect_failure_signals("ok");
        assert!(t
            .failure_signals
            .iter()
            .any(|s| s.starts_with("high_error_rate:")));
        assert!(t.is_failed());
    }

    #[test]
    fn text_failure_phrase_is_strong_signal() {
        let mut t = ExecutionTrace::default();
        t.tool_calls = 1;
        t.tool_calls_ok = 1;
        t.collect_failure_signals("[失败] 原因: timeout");
        assert!(t.failure_signals.iter().any(|s| s == "text_failure_phrase"));
        assert!(t.is_failed());
    }

    #[test]
    fn truncation_signal_is_not_strong_failure() {
        // 截断本身不算失败(已被自动续接),只是弱信号,不进 is_failed
        let mut t = ExecutionTrace::default();
        t.truncation_resumes = 2;
        t.collect_failure_signals("ok 部分输出");
        assert!(t
            .failure_signals
            .iter()
            .any(|s| s.starts_with("truncated:")));
        assert!(!t.is_failed());
    }

    #[test]
    fn overflow_recovery_signal_is_not_strong_failure() {
        // 溢出被本地恢复(排水/折叠)不算失败,只是弱信号,不进 is_failed
        let mut t = ExecutionTrace::default();
        t.overflow_recoveries = 1;
        t.collect_failure_signals("ok");
        assert!(t
            .failure_signals
            .iter()
            .any(|s| s.starts_with("overflow_recovered:")));
        assert!(!t.is_failed());
    }

    #[test]
    fn multiple_signals_can_coexist() {
        let mut t = ExecutionTrace::default();
        t.early_terminated = true;
        t.early_terminate_reason = "max_iter:16".into();
        t.tool_calls = 2;
        t.tool_calls_err = 2;
        t.collect_failure_signals("执行失败: ...");
        // 三个信号并存:early_terminate / high_error_rate / text_failure_phrase
        assert!(t
            .failure_signals
            .iter()
            .any(|s| s.starts_with("early_terminate:")));
        assert!(t
            .failure_signals
            .iter()
            .any(|s| s.starts_with("high_error_rate:")));
        assert!(t.failure_signals.iter().any(|s| s == "text_failure_phrase"));
        assert!(t.is_failed());
    }

    #[test]
    fn render_prompt_stays_compact() {
        let mut t = ExecutionTrace::default();
        t.iterations = 3;
        t.tool_calls = 5;
        t.tool_calls_ok = 4;
        t.tool_calls_err = 1;
        t.output_bytes = 1280;
        t.collect_failure_signals("ok");
        let s = t.render_prompt();
        // 4 行,每行 < 80 字符
        assert!(s.lines().count() <= 8);
        assert!(s.contains("iterations=3"));
        assert!(s.contains("tool_calls=5(ok=4,err=1)"));
        assert!(s.contains("output_bytes=1280"));
    }

    // ========== compact_args_digest(F-002,2026-09-09_06) ==========

    #[test]
    fn digest_prefers_known_keys() {
        // PREFERRED 顺序遇到存在字段就取,凑够 3 个就停
        let v = serde_json::json!({
            "file_path": "/tmp/foo.txt",
            "extra_key": "should_be_dropped"
        });
        let s = compact_args_digest(&v);
        assert!(s.contains("file_path="), "应包含 file_path,实际: {s}");
        // 只有 1 个 PREFERRED 命中 → 取它,然后尝试补 extra_key 直到 3 个
        assert!(
            s.contains("extra_key="),
            "不足 3 个时应补 extra_key,实际: {s}"
        );
    }

    #[test]
    fn digest_stops_at_three_when_preferred_satisfy() {
        // PREFERRED 字段已有 3 个 → 凑满即停,不再补额外字段
        let v = serde_json::json!({
            "file_path": "/tmp/foo.txt",
            "path": "/tmp/alt.txt",
            "command": "ls -la",
            "extra_key": "should_be_dropped"
        });
        let s = compact_args_digest(&v);
        assert!(s.contains("file_path="), "应包含 file_path,实际: {s}");
        assert!(s.contains("path="), "应包含 path,实际: {s}");
        assert!(s.contains("command="), "应包含 command,实际: {s}");
        // 凑满 3 个 → 不再补 extra_key
        assert!(!s.contains("extra_key="), "凑满 3 个后应停止,实际: {s}");
    }

    #[test]
    fn digest_truncates_long_strings() {
        let long_path = "a".repeat(200);
        let v = serde_json::json!({"file_path": long_path});
        let s = compact_args_digest(&v);
        // 截短到 60 字符 + 省略号
        assert!(
            s.chars().count() <= 80,
            "应截短,实际长度 {} 内容: {s}",
            s.chars().count()
        );
        assert!(s.contains('…'), "应含省略号,实际: {s}");
    }

    #[test]
    fn digest_handles_empty_object() {
        let v = serde_json::json!({});
        assert_eq!(compact_args_digest(&v), "{}");
    }

    #[test]
    fn digest_handles_array() {
        let v = serde_json::json!([1, 2, 3, 4, 5]);
        let s = compact_args_digest(&v);
        assert!(s.contains("[5 items]"), "数组应转 [N items],实际: {s}");
    }
}
