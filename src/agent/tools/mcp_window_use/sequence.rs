//! `action=run_sequence`(2026-09-19 第 91 轮):**连续工作模式** ——
//! 探索排查窗口结构后,把统一 plan 好的一整套动作(含验证 / 等待 UI 就绪)
//! 一次工具调用连续执行完毕,逐步落盘执行记录,异常按策略处理。
//!
//! 动机(设计见 `docs/MCP_Window_Use/03-连续工作模式设计与解决方案.md`):
//! - 用户与 Agent 共用一台机器,会随时用键盘鼠标、切换窗口 —— 长批次执行
//!   中途焦点随时被抢。input_batch 只在开头做一次前台守卫;本 action 每个
//!   **物理输入步骤**前都做焦点守护,丢失则重新激活 + 轮询等待,
//!   连续 3 次重夺失败止损中止(与第 88 轮 chat_loop 止损同源);
//! - input_batch 的 10 个 op 全是「动作」,没有验证能力;本 action 新增
//!   `assert_text`(关键节点自检)/ `wait_for_text`(轮询等异步 UI 就绪,
//!   替代盲 wait)/ `wait_front`(显式恢复前台);
//! - 错误策略三档:`abort`(默认,失败即停)/ `continue` / `retry`
//!   (逐步重试 ≤3 次,间隔可调;步骤级可覆盖),另有步骤级 `optional`
//!   容忍非关键步骤失败;
//! - 每步落盘 [STEP]/[RETRY]/[FOCUS_LOST]/[FOCUS_REGAINED]/[FAIL] +
//!   末尾 [SUMMARY] 到 `log_path`(默认 <工作目录>/laew_sequence_<ts>.log),
//!   长批失败后可追溯现场。
//!
//! 护栏:steps ≤ 100;max_total_ms ≤ 600s(默认 180s);wait ≤ 30s;
//! wait_for_text timeout ≤ 30s;retry_times ≤ 3;focus_wait_ms ≤ 30s。
//! 物理输入最终落到既有 ControlAction,权限/优先级链与 control 一致,
//! 不新增攻击面。
use serde_json::{json, Value};

use super::chat::{append_chat_log, format_chat_log_line};
use super::input_batch::{execute_step, step_i64, step_str};
use super::*;
use crate::agent::window::{probe_capability, ControlAction};

/// steps 数组长度上限(连续工作模式面向长批,比 input_batch 40 放宽)。
const MAX_SEQ_STEPS: usize = 100;
/// 整批默认时长硬顶(毫秒)。
const DEFAULT_MAX_TOTAL_MS: u64 = 180_000;
/// 整批时长硬顶上限(毫秒)。
const MAX_TOTAL_MS_CAP: u64 = 600_000;
/// 连续模式 wait 单步上限(毫秒;比 input_batch 5000 放宽,内部分片睡眠)。
const MAX_SEQ_WAIT_MS: u64 = 30_000;
/// wait 分片粒度(毫秒;每片检查整批时长硬顶)。
const WAIT_CHUNK_MS: u64 = 500;
/// wait_for_text 默认超时(毫秒)。
const DEFAULT_WAIT_TEXT_TIMEOUT_MS: u64 = 10_000;
/// wait_for_text 超时上限(毫秒)。
const MAX_WAIT_TEXT_TIMEOUT_MS: u64 = 30_000;
/// wait_for_text 默认轮询间隔(毫秒)。
const DEFAULT_WAIT_TEXT_POLL_MS: u64 = 500;
/// 焦点丢失后重夺默认预算(毫秒)。
const DEFAULT_FOCUS_WAIT_MS: u64 = 5000;
/// 焦点重夺预算上限(毫秒)。
const MAX_FOCUS_WAIT_MS: u64 = 30_000;
/// 批次开头前台守卫轮询预算(与 chat.rs ensure_frontmost 同值)。
const FRONTMOST_TIMEOUT_MS: u64 = 1500;
/// 前台守卫轮询间隔(毫秒)。
const FRONTMOST_POLL_MS: u64 = 150;
/// 连续焦点重夺失败止损阈值(与 chat_loop MAX_CONSECUTIVE_FOCUS_FAIL 同值)。
const MAX_CONSECUTIVE_FOCUS_FAIL: usize = 3;
/// 默认每步重试次数(on_error=retry 时)。
const DEFAULT_RETRY_TIMES: u64 = 1;
/// 每步重试次数上限。
const MAX_RETRY_TIMES: u64 = 3;
/// 默认重试间隔(毫秒)。
const DEFAULT_RETRY_DELAY_MS: u64 = 500;
/// 重试间隔上限(毫秒)。
const MAX_RETRY_DELAY_MS: u64 = 5000;
/// 每步执行前 delay_ms 上限(与 input_batch 一致)。
const MAX_STEP_DELAY_MS: u64 = 2000;

/// 错误处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnError {
    /// 失败即停(默认),剩余步骤标记 skipped。
    Abort,
    /// 失败后继续后续步骤。
    Continue,
    /// 先逐步重试,仍败则停。
    Retry,
}

fn parse_on_error(args: &Value) -> Result<OnError> {
    match get_str(args, "on_error").unwrap_or("abort") {
        "abort" => Ok(OnError::Abort),
        "continue" => Ok(OnError::Continue),
        "retry" => Ok(OnError::Retry),
        other => Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            format!("on_error 非法值 {other:?};合法:abort / continue / retry"),
        )),
    }
}

/// 物理输入 op 集合(focus_guard 守护对象:执行前必须确认窗口前台)。
fn is_physical_op(op: &str) -> bool {
    matches!(
        op,
        "mouse_click" | "mouse_drag" | "mouse_scroll" | "key_press" | "type_text" | "click"
    )
}

/// 解析 `log_path` 参数(可选),缺省 = `<工作目录>/laew_sequence_<unix_ts>.log`。
fn resolve_log_path(args: &Value) -> String {
    if let Some(p) = get_str(args, "log_path") {
        return p.to_string();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    cwd.join(format!("laew_sequence_{}.log", now_unix()))
        .to_string_lossy()
        .to_string()
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 截断文本用于落盘(避免超长 detail 打爆日志)。
fn truncate_log(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max).collect();
        s.push_str("...(truncated)");
        s
    }
}

// ===================== 第 93 轮:自绘 UI OCR 兜底断言 =====================

/// 全窗口 OCR 文本(assert_text / wait_for_text 在控件树为空时的视觉兜底)。
///
/// 实测背景(llaew_20260919_121348.log):微信 4.x 自绘 UI 控件树只有空 Pane,
/// `assert_text{path:"/", contains:"发送"}` 断言到的是**窗口标题**(如
/// 「lsm(刘诗萌)」),3 次重试必败;LLM 真正想断言的是「窗口里能否看到某文本」,
/// 那正是 OCR 的语义。GetText 未命中时经本函数对整窗 OCR 再断言。
fn ocr_window_text(
    driver: &dyn crate::agent::window::WindowDriver,
    window_id: &str,
) -> Result<String> {
    let wins = driver.list_windows(None)?;
    let info = wins.into_iter().find(|w| w.id == window_id).ok_or_else(|| {
        tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            "window_id 已失效(可能已关闭/最小化),请重新 action=list",
        )
    })?;
    let blocks = driver.ocr_with_info(&info, None, None)?;
    Ok(blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" "))
}

/// 当前平台 OCR 是否可用(不可用时不进 OCR 兜底,直接按控件文本结果判定)。
fn ocr_available() -> bool {
    probe_capability().ocr_screenshot_cgwindow
}

// ===================== 第 93 轮:纯 wait 批次自检 =====================

/// 非动作 op(等待 / 验证类,不产生 UI 交互证据)。
const NON_ACTION_OPS: &[&str] = &["wait", "wait_for_text", "wait_front", "assert_text"];

/// 判定整批是否「纯等待/断言、零 UI 动作」。
///
/// 实测背景:SubAgent 把「10 分钟微信聊天」降级成 16×30s wait 充时长,
/// QC 凭时间差验收蒙混通过。`wait_only=true` 时返回体 next_action 给出
/// 明确警示:纯等待不构成任何操作的完成证据。
fn compute_wait_only(results: &[Value]) -> bool {
    !results.is_empty()
        && results.iter().all(|s| {
            let op = s["op"].as_str().unwrap_or("");
            NON_ACTION_OPS.contains(&op)
        })
}

/// `action=run_sequence` 入口:参数校验 + 同步执行循环(run_blocking 包裹)。
pub(super) async fn run_input_sequence(args: Value) -> Result<String> {
    let window_id = require_str(&args, "window_id", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let on_error = parse_on_error(&args)?;
    let steps = args
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            tool_err(
                MCP_WINDOW_USE_TOOL_NAME,
                "缺少 array 类型参数 steps(连续工作模式步骤数组,元素为 {\"op\": ...} 对象)",
            )
        })?;
    if steps.is_empty() {
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            "steps 不能为空(至少 1 个步骤对象)",
        ));
    }
    if steps.len() > MAX_SEQ_STEPS {
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            format!(
                "steps 长度 {} 超上限 {MAX_SEQ_STEPS}(拆成多次 run_sequence 调用)",
                steps.len()
            ),
        ));
    }
    let retry_times = args
        .get("retry_times")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_RETRY_TIMES)
        .min(MAX_RETRY_TIMES);
    let retry_delay_ms = args
        .get("retry_delay_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_RETRY_DELAY_MS)
        .min(MAX_RETRY_DELAY_MS);
    let focus_guard = args
        .get("focus_guard")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let focus_wait_ms = args
        .get("focus_wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_FOCUS_WAIT_MS)
        .min(MAX_FOCUS_WAIT_MS);
    let max_total_ms = args
        .get("max_total_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_TOTAL_MS)
        .min(MAX_TOTAL_MS_CAP);
    let log_path = resolve_log_path(&args);

    let batch = steps.clone();
    let wid = window_id.clone();
    let lp = log_path.clone();
    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
        let cfg = SeqConfig {
            on_error,
            retry_times,
            retry_delay_ms,
            focus_guard,
            focus_wait_ms,
            max_total_ms,
            log_path: lp.clone(),
        };
        let outcome = execute_sequence(&wid, &batch, &cfg)?;
        let steps_ok = outcome
            .results
            .iter()
            .filter(|s| s["ok"].as_bool().unwrap_or(false))
            .count();
        // 第 93 轮:纯 wait 批次自检(全批无任何 UI 动作 → 完成证据为零)
        let wait_only = compute_wait_only(&outcome.results);
        let mut body = json!({
            "ok": outcome.failed.is_empty() && !outcome.focus_aborted,
            "action": "run_sequence",
            "window_id": wid,
            "frontmost_acquired": true,
            "on_error": format!("{on_error:?}").to_lowercase(),
            "steps_total": outcome.results.len(),
            "steps_ok": steps_ok,
            "wait_only": wait_only,
            "steps": outcome.results,
            "results": outcome.read_values,
            "retried_steps": outcome.retried_steps,
            "focus_lost_count": outcome.focus_lost_count,
            "focus_aborted": outcome.focus_aborted,
            "elapsed_ms": outcome.elapsed_ms,
            "log_path": lp,
        });
        if !outcome.failed.is_empty() {
            body["failed_steps"] = json!(outcome.failed);
        }
        body["next_action"] = json!(if outcome.focus_aborted {
            format!(
                "连续 {MAX_CONSECUTIVE_FOCUS_FAIL} 次焦点重夺失败已止损中止(用户可能正在操作其他窗口,继续执行会误输入到别的软件);\
                 请提醒用户保持目标窗口前台后,把未执行片段拆成新的 run_sequence 重新发起"
            )
        } else if !outcome.failed.is_empty() {
            "部分步骤失败(见 failed_steps 与 log_path 落盘记录);UI 可能已变化,\
             先 action=inspect 或 action=ocr 重新定位,再把失败片段拆成新的 run_sequence 补做"
                .to_string()
        } else if wait_only {
            // 第 93 轮:整批全 wait/断言、零 UI 动作 —— 明确警示这不是完成证据
            "⚠️ 本批全部步骤均为 wait/断言(纯等待),未执行任何 UI 交互动作:\
             纯等待不构成「操作软件」的完成证据。若任务要求点击/输入/发送/读取界面内容,\
             请改用 mouse_click / type_text / chat_send / chat_loop 等动作步骤重新编排;\
             仅当用户明确要求「保持窗口打开不操作」时,纯 wait 保活才是合理交付"
                .to_string()
        } else {
            "全部步骤执行完成;执行记录已落盘 log_path,可 action=inspect / action=ocr 复查 UI 状态"
                .to_string()
        });
        Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
    })
    .await
}

/// 连续执行配置(同步闭包内共享)。
struct SeqConfig {
    on_error: OnError,
    retry_times: u64,
    retry_delay_ms: u64,
    focus_guard: bool,
    focus_wait_ms: u64,
    max_total_ms: u64,
    log_path: String,
}

/// 连续执行结果(同步闭包内聚合)。
struct SeqOutcome {
    results: Vec<Value>,
    /// get_text 步骤读取的值(key → 文本)。
    read_values: serde_json::Map<String, Value>,
    failed: Vec<usize>,
    /// 发生过重试的步骤数。
    retried_steps: usize,
    /// 焦点丢失次数(每次成功重夺计 1)。
    focus_lost_count: usize,
    /// 连续焦点重夺失败达到阈值,止损中止。
    focus_aborted: bool,
    elapsed_ms: u128,
}

/// 同步前台守卫(run_blocking 内用 thread::sleep;语义同 chat.rs::ensure_frontmost:
/// 平台不支持真实前台探测时放行,避免 Linux 误阻断)。
fn ensure_frontmost_sync(window_id: &str, budget_ms: u64) -> (bool, u32, u64) {
    let driver = current_driver();
    let started = std::time::Instant::now();
    let mut attempts = 0u32;
    let _ = driver.bring_to_front(window_id);
    loop {
        attempts += 1;
        if driver.is_frontmost(window_id) {
            return (true, attempts, started.elapsed().as_millis() as u64);
        }
        if started.elapsed().as_millis() as u64 >= budget_ms {
            let supported = cfg!(any(target_os = "macos", windows));
            return (!supported, attempts, started.elapsed().as_millis() as u64);
        }
        std::thread::sleep(std::time::Duration::from_millis(FRONTMOST_POLL_MS));
    }
}

/// 把剩余步骤标记为 skipped(可观测,不静默丢弃)。
fn mark_skipped(outcome: &mut SeqOutcome, steps: &[Value], from: usize, reason: &str) {
    for (j, s) in steps.iter().enumerate().skip(from) {
        let op = s.get("op").and_then(Value::as_str).unwrap_or("?");
        outcome.results.push(json!({
            "i": j, "op": op, "ok": false, "skipped": true, "detail": reason
        }));
    }
}

/// 连续执行主循环(同步;被 run_blocking 包裹)。
fn execute_sequence(window_id: &str, steps: &[Value], cfg: &SeqConfig) -> Result<SeqOutcome> {
    // ① 批次开头前台守卫(与 input_batch 一致:拿不到前台一步都不执行,不盲打)
    let (frontmost, attempts, elapsed) = ensure_frontmost_sync(window_id, FRONTMOST_TIMEOUT_MS);
    if !frontmost {
        append_chat_log(
            &cfg.log_path,
            &format_chat_log_line(
                now_unix(),
                "FAIL",
                &format!("window_id={window_id} 批次开头前台守卫未通过({elapsed}ms {attempts} 次)"),
            ),
        );
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            format!(
                "前台守卫未通过({elapsed}ms 内未拿到前台,轮询 {attempts} 次);连续工作模式已中止,\
                 防止误操作其他窗口(可 action=open 重新激活后重试)"
            ),
        ));
    }

    let driver = current_driver();
    let started = std::time::Instant::now();
    let mut outcome = SeqOutcome {
        results: Vec::with_capacity(steps.len()),
        read_values: serde_json::Map::new(),
        failed: Vec::new(),
        retried_steps: 0,
        focus_lost_count: 0,
        focus_aborted: false,
        elapsed_ms: 0,
    };
    let mut consecutive_focus_fail = 0usize;

    let mut i = 0usize;
    while i < steps.len() {
        let step = &steps[i];
        let op = step.get("op").and_then(Value::as_str).unwrap_or("");

        // ② 整批时长硬顶
        if started.elapsed().as_millis() as u64 > cfg.max_total_ms {
            mark_skipped(
                &mut outcome,
                steps,
                i,
                &format!("整批执行超过 {}ms 硬顶,本步骤跳过", cfg.max_total_ms),
            );
            break;
        }

        // ③ 步前 delay
        let delay = step
            .get("delay_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(MAX_STEP_DELAY_MS);
        if delay > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }

        // ④ 焦点守护:物理输入步骤执行前确认窗口仍在前台
        if cfg.focus_guard && is_physical_op(op) && !driver.is_frontmost(window_id) {
            // 平台不支持真实前台探测时 is_frontmost 恒 false —— 与
            // ensure_frontmost 同策略放行,不在非桌面平台误阻断。
            let supported = cfg!(any(target_os = "macos", windows));
            if supported {
                outcome.focus_lost_count += 1;
                append_chat_log(
                    &cfg.log_path,
                    &format_chat_log_line(
                        now_unix(),
                        "FOCUS_LOST",
                        &format!(
                            "window_id={window_id} 步骤 {i}({op})执行前焦点不在目标窗口,重新激活"
                        ),
                    ),
                );
                let (ok, att, el) = ensure_frontmost_sync(window_id, cfg.focus_wait_ms);
                if ok {
                    consecutive_focus_fail = 0;
                    append_chat_log(
                        &cfg.log_path,
                        &format_chat_log_line(
                            now_unix(),
                            "FOCUS_REGAINED",
                            &format!("elapsed={el}ms attempts={att}"),
                        ),
                    );
                } else {
                    consecutive_focus_fail += 1;
                    if consecutive_focus_fail >= MAX_CONSECUTIVE_FOCUS_FAIL {
                        outcome.focus_aborted = true;
                        outcome.results.push(json!({
                            "i": i, "op": op, "ok": false,
                            "detail": format!(
                                "连续 {MAX_CONSECUTIVE_FOCUS_FAIL} 次焦点重夺失败,止损中止\
                                 (用户可能正在操作其他窗口)"
                            )
                        }));
                        outcome.failed.push(i);
                        append_chat_log(
                            &cfg.log_path,
                            &format_chat_log_line(
                                now_unix(),
                                "FAIL",
                                &format!(
                                    "window_id={window_id} 连续 {MAX_CONSECUTIVE_FOCUS_FAIL} 次焦点重夺失败,止损中止"
                                ),
                            ),
                        );
                        mark_skipped(&mut outcome, steps, i + 1, "焦点止损中止,本步骤未执行");
                        break;
                    }
                    // 未达阈值:本步按失败处理(走 on_error 策略)
                    let mut rec = json!({
                        "i": i, "op": op, "ok": false,
                        "detail": format!("焦点重夺失败({}ms 预算内未拿回前台)", cfg.focus_wait_ms)
                    });
                    let optional = step
                        .get("optional")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if optional {
                        rec["optional"] = json!(true);
                        outcome.results.push(rec);
                        i += 1;
                        continue;
                    }
                    outcome.failed.push(i);
                    outcome.results.push(rec);
                    if cfg.on_error == OnError::Continue {
                        i += 1;
                        continue;
                    }
                    mark_skipped(
                        &mut outcome,
                        steps,
                        i + 1,
                        "前置步骤焦点重夺失败(fail-fast),本步骤未执行",
                    );
                    break;
                }
            }
        } else if cfg.focus_guard && is_physical_op(op) {
            consecutive_focus_fail = 0;
        }

        // ⑤ 执行(含 retry 策略)
        let optional = step
            .get("optional")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let step_retry_times = step
            .get("retry_times")
            .and_then(Value::as_u64)
            .unwrap_or(cfg.retry_times)
            .min(MAX_RETRY_TIMES);
        let step_retry_delay = step
            .get("retry_delay_ms")
            .and_then(Value::as_u64)
            .unwrap_or(cfg.retry_delay_ms)
            .min(MAX_RETRY_DELAY_MS);
        let max_attempts = if cfg.on_error == OnError::Retry {
            1 + step_retry_times
        } else {
            1
        };

        let mut record;
        let mut attempt = 0u64;
        loop {
            attempt += 1;
            record = execute_seq_step(
                window_id,
                driver.as_ref(),
                step,
                i,
                &mut outcome.read_values,
                cfg,
                started,
            );
            let ok = record["ok"].as_bool().unwrap_or(false);
            if ok || attempt >= max_attempts {
                break;
            }
            if attempt == 1 {
                outcome.retried_steps += 1;
            }
            append_chat_log(
                &cfg.log_path,
                &format_chat_log_line(
                    now_unix(),
                    "RETRY",
                    &format!(
                        "i={i} op={op} attempt={}/{max_attempts} detail=\"{}\"",
                        attempt + 1,
                        truncate_log(record["detail"].as_str().unwrap_or(""), 120)
                    ),
                ),
            );
            std::thread::sleep(std::time::Duration::from_millis(step_retry_delay));
        }
        if attempt > 1 {
            record["attempts"] = json!(attempt);
        }
        let ok = record["ok"].as_bool().unwrap_or(false);

        // ⑥ 落盘 [STEP] / [FAIL]
        let label = if ok { "STEP" } else { "FAIL" };
        append_chat_log(
            &cfg.log_path,
            &format_chat_log_line(
                now_unix(),
                label,
                &format!(
                    "i={i} op={op} ok={ok} attempts={attempt} detail=\"{}\"",
                    truncate_log(record["detail"].as_str().unwrap_or(""), 160)
                ),
            ),
        );

        if !ok && !optional {
            outcome.failed.push(i);
            outcome.results.push(record);
            match cfg.on_error {
                OnError::Continue => {
                    i += 1;
                    continue;
                }
                OnError::Abort | OnError::Retry => {
                    mark_skipped(
                        &mut outcome,
                        steps,
                        i + 1,
                        "前置步骤失败(fail-fast),本步骤未执行;\
                         可 on_error=continue 继续或 on_error=retry 自动重试",
                    );
                    break;
                }
            }
        }
        let mut record = record;
        if !ok && optional {
            record["optional"] = json!(true);
        }
        outcome.results.push(record);
        i += 1;
    }

    outcome.elapsed_ms = started.elapsed().as_millis();

    // ⑦ [SUMMARY] 落盘
    let ok_count = outcome
        .results
        .iter()
        .filter(|s| s["ok"].as_bool().unwrap_or(false))
        .count();
    append_chat_log(
        &cfg.log_path,
        &format_chat_log_line(
            now_unix(),
            "SUMMARY",
            &format!(
                "total={} ok={} failed={} retried={} focus_lost={} focus_aborted={} elapsed={:.1}s",
                outcome.results.len(),
                ok_count,
                outcome.failed.len(),
                outcome.retried_steps,
                outcome.focus_lost_count,
                outcome.focus_aborted,
                outcome.elapsed_ms as f64 / 1000.0
            ),
        ),
    );
    Ok(outcome)
}

/// 执行单步(连续模式):新 op 本模块处理,其余委托 input_batch::execute_step。
fn execute_seq_step(
    window_id: &str,
    driver: &dyn crate::agent::window::WindowDriver,
    step: &Value,
    idx: usize,
    read_values: &mut serde_json::Map<String, Value>,
    cfg: &SeqConfig,
    batch_started: std::time::Instant,
) -> Value {
    let op = step.get("op").and_then(Value::as_str).unwrap_or("");
    let fail = |detail: String| -> Value {
        json!({"i": idx, "op": op, "ok": false, "detail": detail})
    };
    match op {
        // ===== 连续模式新增:验证与等待 =====
        "assert_text" => {
            let Some(path) = step_str(step, "path") else {
                return fail("op=assert_text 缺少 string 参数 path(控件路径,取 action=inspect)".into());
            };
            let Some(contains) = step_str(step, "contains") else {
                return fail("op=assert_text 缺少 string 参数 contains(期望包含的文本)".into());
            };
            let uia_text = driver.act(window_id, path, ControlAction::GetText).ok();
            if let Some(text) = &uia_text {
                if text.contains(contains) {
                    return json!({"i": idx, "op": op, "ok": true,
                                  "detail": format!("断言通过:控件文本包含 {contains:?} (route=uia)")});
                }
            }
            // 第 93 轮:自绘 UI(控件树空 / 窗口根 GetText=标题)OCR 兜底 ——
            // LLM 断言的通常是「窗口里能否看到某文本」,正是 OCR 语义。
            if ocr_available() {
                match ocr_window_text(driver, window_id) {
                    Ok(ocr_text) if ocr_text.contains(contains) => {
                        return json!({"i": idx, "op": op, "ok": true,
                                      "detail": format!("断言通过:窗口 OCR 文本包含 {contains:?} (route=ocr)")});
                    }
                    Ok(ocr_text) => {
                        return fail(format!(
                            "断言失败:控件文本与窗口 OCR 均不含 {contains:?};控件=\"{}\",OCR=\"{}\" (route=uia+ocr)",
                            truncate_log(uia_text.as_deref().unwrap_or("<读取失败>"), 60),
                            truncate_log(&ocr_text, 120)
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "assert_text OCR 兜底失败,按控件文本结果判定");
                    }
                }
            }
            match uia_text {
                Some(text) => fail(format!(
                    "断言失败:控件文本不含 {contains:?},实际=\"{}\" (route=uia;\
                     若为自绘 UI 且 OCR 可用,工具已自动 OCR 兜底仍未命中)",
                    truncate_log(&text, 120)
                )),
                None => fail("读取控件文本失败:控件树不可用;自绘 UI 请确认 OCR 能力(action=capability_probe)".into()),
            }
        }
        "wait_for_text" => {
            let Some(path) = step_str(step, "path") else {
                return fail("op=wait_for_text 缺少 string 参数 path(控件路径,取 action=inspect)".into());
            };
            let Some(contains) = step_str(step, "contains") else {
                return fail("op=wait_for_text 缺少 string 参数 contains(等待出现的文本)".into());
            };
            let timeout_ms = step
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_WAIT_TEXT_TIMEOUT_MS);
            if timeout_ms > MAX_WAIT_TEXT_TIMEOUT_MS {
                return fail(format!(
                    "wait_for_text timeout_ms={timeout_ms} 超上限 {MAX_WAIT_TEXT_TIMEOUT_MS}"
                ));
            }
            let poll_ms = step
                .get("poll_ms")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_WAIT_TEXT_POLL_MS)
                .clamp(100, 5000);
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
            let ocr_ok = ocr_available();
            loop {
                if let Ok(text) = driver.act(window_id, path, ControlAction::GetText) {
                    if text.contains(contains) {
                        return json!({"i": idx, "op": op, "ok": true,
                                      "detail": format!("等到目标文本 {contains:?} (route=uia)")});
                    }
                }
                // 第 93 轮:自绘 UI OCR 兜底(每轮一次;OCR ~300ms 在 500ms 轮询预算内)
                if ocr_ok {
                    if let Ok(ocr_text) = ocr_window_text(driver, window_id) {
                        if ocr_text.contains(contains) {
                            return json!({"i": idx, "op": op, "ok": true,
                                          "detail": format!("等到目标文本 {contains:?} (route=ocr)")});
                        }
                    }
                }
                if std::time::Instant::now() >= deadline {
                    return fail(format!(
                        "等待超时({timeout_ms}ms):控件文本与窗口 OCR 始终未出现 {contains:?}"
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(poll_ms));
            }
        }
        "wait_front" => {
            let timeout_ms = step
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(cfg.focus_wait_ms);
            if timeout_ms > MAX_FOCUS_WAIT_MS {
                return fail(format!(
                    "wait_front timeout_ms={timeout_ms} 超上限 {MAX_FOCUS_WAIT_MS}"
                ));
            }
            let (ok, attempts, elapsed) = ensure_frontmost_sync(window_id, timeout_ms);
            if ok {
                json!({"i": idx, "op": op, "ok": true,
                       "detail": format!("窗口已前台({elapsed}ms, 轮询 {attempts} 次)")})
            } else {
                fail(format!(
                    "wait_front 超时({timeout_ms}ms 内未拿到前台,轮询 {attempts} 次)"
                ))
            }
        }
        // ===== wait:连续模式上限放宽到 30s,分片睡眠并响应整批时长硬顶 =====
        "wait" => {
            let ms = step.get("ms").and_then(Value::as_u64).unwrap_or(200);
            if ms > MAX_SEQ_WAIT_MS {
                return fail(format!("wait ms={ms} 超上限 {MAX_SEQ_WAIT_MS}"));
            }
            let mut remaining = ms;
            while remaining > 0 {
                if batch_started.elapsed().as_millis() as u64 > cfg.max_total_ms {
                    return fail("wait 分片期间整批超过时长硬顶,提前结束".into());
                }
                let chunk = remaining.min(WAIT_CHUNK_MS);
                std::thread::sleep(std::time::Duration::from_millis(chunk));
                remaining -= chunk;
            }
            json!({"i": idx, "op": op, "ok": true, "detail": format!("已等待 {ms}ms")})
        }
        // ===== 其余 op 全部委托 input_batch 同一执行函数(语义完全一致) =====
        _ => execute_step(window_id, driver, step, idx, read_values),
    }
}

/// 供测试与内部使用:读取步骤 i64 参数(保持本模块自洽)。
#[allow(dead_code)]
fn seq_step_i64(step: &Value, key: &str) -> Option<i64> {
    step_i64(step, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_on_error_accepts_three_modes() {
        assert_eq!(parse_on_error(&json!({})).unwrap(), OnError::Abort);
        assert_eq!(
            parse_on_error(&json!({"on_error": "continue"})).unwrap(),
            OnError::Continue
        );
        assert_eq!(
            parse_on_error(&json!({"on_error": "retry"})).unwrap(),
            OnError::Retry
        );
        assert!(parse_on_error(&json!({"on_error": "explode"})).is_err());
    }

    #[test]
    fn is_physical_op_covers_mouse_and_keyboard() {
        for op in [
            "mouse_click",
            "mouse_drag",
            "mouse_scroll",
            "key_press",
            "type_text",
            "click",
        ] {
            assert!(is_physical_op(op), "{op} 应受焦点守护");
        }
        for op in [
            "mouse_move",
            "get_text",
            "wait",
            "assert_text",
            "wait_for_text",
        ] {
            assert!(!is_physical_op(op), "{op} 不应受焦点守护");
        }
    }

    #[test]
    fn compute_wait_only_flags_pure_wait_batches() {
        // 第 93 轮实测形态:16×wait + 2×assert_text(断言可走 optional 蒙混)→ wait_only
        let results: Vec<Value> = (0..16)
            .map(|i| json!({"i": i, "op": "wait", "ok": true}))
            .chain([
                json!({"i": 16, "op": "assert_text", "ok": false, "optional": true}),
                json!({"i": 17, "op": "assert_text", "ok": false, "optional": true}),
            ])
            .collect();
        assert!(compute_wait_only(&results));
        // 任意一个真实 UI 动作 → 非 wait_only
        let mut with_action = results.clone();
        with_action.push(json!({"i": 18, "op": "type_text", "ok": true}));
        assert!(!compute_wait_only(&with_action));
        // get_text 产出读取证据 → 非 wait_only
        let mut with_read = results.clone();
        with_read.push(json!({"i": 18, "op": "get_text", "ok": true}));
        assert!(!compute_wait_only(&with_read));
        // 空批次 → false(不产生误报)
        assert!(!compute_wait_only(&[]));
    }

    #[test]
    fn seq_step_validates_new_ops_params() {
        let driver = crate::agent::window::current_driver();
        let cfg = SeqConfig {
            on_error: OnError::Abort,
            retry_times: 1,
            retry_delay_ms: 100,
            focus_guard: false,
            focus_wait_ms: 1000,
            max_total_ms: 60_000,
            log_path: String::new(),
        };
        let mut reads = serde_json::Map::new();
        let now = std::time::Instant::now();
        // assert_text 缺 contains
        let v = execute_seq_step(
            "w",
            driver.as_ref(),
            &json!({"op": "assert_text", "path": "/0"}),
            0,
            &mut reads,
            &cfg,
            now,
        );
        assert_eq!(v["ok"], json!(false));
        assert!(v["detail"].as_str().unwrap().contains("contains"));
        // wait_for_text timeout 超上限
        let v = execute_seq_step(
            "w",
            driver.as_ref(),
            &json!({"op": "wait_for_text", "path": "/0", "contains": "x", "timeout_ms": 99999}),
            0,
            &mut reads,
            &cfg,
            now,
        );
        assert!(v["detail"].as_str().unwrap().contains("超上限"));
        // wait_front timeout 超上限
        let v = execute_seq_step(
            "w",
            driver.as_ref(),
            &json!({"op": "wait_front", "timeout_ms": 99999}),
            0,
            &mut reads,
            &cfg,
            now,
        );
        assert!(v["detail"].as_str().unwrap().contains("超上限"));
        // wait 连续模式放宽:31000 超限(上限 30000)
        let v = execute_seq_step(
            "w",
            driver.as_ref(),
            &json!({"op": "wait", "ms": 31000}),
            0,
            &mut reads,
            &cfg,
            now,
        );
        assert!(v["detail"].as_str().unwrap().contains("超上限"));
    }

    #[tokio::test]
    async fn run_input_sequence_validates_top_params() {
        // 缺 steps
        let r = run_input_sequence(json!({"window_id": "w"})).await;
        assert!(r.is_err());
        // steps 空
        let r = run_input_sequence(json!({"window_id": "w", "steps": []})).await;
        assert!(r.is_err());
        // steps 超限
        let steps: Vec<Value> = (0..101).map(|_| json!({"op": "wait", "ms": 0})).collect();
        let r = run_input_sequence(json!({"window_id": "w", "steps": steps})).await;
        assert!(r.unwrap_err().to_string().contains("超上限"));
        // on_error 非法
        let r = run_input_sequence(
            json!({"window_id": "w", "steps": [{"op": "wait", "ms": 1}], "on_error": "boom"}),
        )
        .await;
        assert!(r.unwrap_err().to_string().contains("on_error 非法"));
    }
}
