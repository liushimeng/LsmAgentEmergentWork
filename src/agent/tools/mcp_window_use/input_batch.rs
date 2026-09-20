//! `action=input_batch`(2026-09-19 第 90 轮):鼠标 + 键盘 + 控件树操作的
//! **复合步骤动作**,一次工具调用编排任意顺序的多步操作。
//!
//! 动机(设计见 `docs/MCP_Window_Use/02-鼠标键盘操控与优先级链方案.md` §2.3):
//! - `type_text_submit`(第 80 轮)只覆盖「点+输+回车」一个特例;任意组合
//!   (点输入框 → ctrl+a → 输新文本 → 点确定)此前需多次工具调用,每两次调用
//!   之间都有焦点竞态窗口,且烧 SubAgent 16 次迭代上限;
//! - 本 action 一次前台守卫 + 批量执行,步骤间零 LLM 返场 ——「同时操作鼠标、
//!   键盘」的统一入口。
//!
//! 步骤类型:
//! - 鼠标:mouse_move / mouse_click(左/右/中,单击/双击,modifiers)/
//!   mouse_drag(modifiers)/ mouse_scroll;
//! - 键盘:key_press(单键/组合键,如 ctrl+a)/ type_text(Unicode 物理键入);
//! - 控件树(自动享受驱动层 T1 无障碍 → T2 消息 → T3 物理优先级链):
//!   click / set_text / get_text(读取值进 results);
//! - wait(毫秒级等待)。
//!
//! 护栏:steps ≤ 40;wait ≤ 5000ms;每步 delay_ms ≤ 2000ms;总时长硬顶 60s;
//! 默认 fail-fast,`continue_on_error=true` 时失败继续。

use serde_json::{json, Value};

use super::chat::ensure_frontmost;
use super::*;
use crate::agent::window::ControlAction;

/// steps 数组长度上限。
const MAX_STEPS: usize = 40;
/// 单步 wait 上限(毫秒)。
const MAX_WAIT_MS: u64 = 5000;
/// 每步执行前 delay_ms 上限(毫秒)。
const MAX_STEP_DELAY_MS: u64 = 2000;
/// 整批执行时长硬顶(毫秒)。
const MAX_TOTAL_MS: u128 = 60_000;

/// `action=input_batch` 入口:参数校验 + 前台守卫 + 逐步执行 + 汇总返回。
pub(super) async fn run_input_batch(args: Value) -> Result<String> {
    let window_id = require_str(&args, "window_id", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let continue_on_error = args
        .get("continue_on_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let steps = args
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            tool_err(
                MCP_WINDOW_USE_TOOL_NAME,
                "缺少 array 类型参数 steps(操作步骤数组,元素为 {\"op\": ...} 对象)",
            )
        })?;
    if steps.is_empty() {
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            "steps 不能为空(至少 1 个步骤对象)",
        ));
    }
    if steps.len() > MAX_STEPS {
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            format!("steps 长度 {} 超上限 {MAX_STEPS}(拆成多次 input_batch 调用)", steps.len()),
        ));
    }

    // 一次前台守卫(第 88 轮机制复用):批量物理输入前确认焦点,不盲打。
    let front = ensure_frontmost(&window_id).await?;
    if !front.frontmost {
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            format!(
                "前台守卫未通过({}ms 内未拿到前台,轮询 {} 次);批量输入已中止,防止误操作其他窗口\
                 (可 action=open 重新激活后重试)",
                front.elapsed_ms, front.attempts
            ),
        ));
    }

    let batch = steps.clone();
    let wid = window_id.clone();
    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
        let detail = execute_steps(&wid, &batch, continue_on_error)?;
        let mut body = json!({
            "ok": detail.failed.is_empty(),
            "action": "input_batch",
            "window_id": wid,
            "frontmost_acquired": true,
            "steps_total": detail.results.len(),
            "steps_ok": detail.results.iter().filter(|s| s["ok"].as_bool().unwrap_or(false)).count(),
            "steps": detail.results,
            "results": detail.read_values,
            "elapsed_ms": detail.elapsed_ms,
        });
        if !detail.failed.is_empty() {
            body["failed_steps"] = json!(detail.failed);
            body["next_action"] = json!(
                "部分步骤失败(见 failed_steps);UI 可能已变化,先 action=inspect 或 action=ocr 重新定位再补发失败步骤"
            );
        } else {
            body["next_action"] = json!("全部步骤执行完成;可 action=inspect / action=ocr 复查 UI 状态,或继续下一个 input_batch");
        }
        Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
    })
    .await
}

/// 批量执行结果(同步闭包内聚合)。
struct BatchOutcome {
    results: Vec<Value>,
    /// get_text 步骤读取的值(key → 文本;无 key 时用 path)。
    read_values: serde_json::Map<String, Value>,
    failed: Vec<usize>,
    elapsed_ms: u128,
}

/// 逐步执行(同步;被 run_blocking 包裹)。
fn execute_steps(
    window_id: &str,
    steps: &[Value],
    continue_on_error: bool,
) -> Result<BatchOutcome> {
    let driver = current_driver();
    let started = std::time::Instant::now();
    let mut outcome = BatchOutcome {
        results: Vec::with_capacity(steps.len()),
        read_values: serde_json::Map::new(),
        failed: Vec::new(),
        elapsed_ms: 0,
    };

    for (i, step) in steps.iter().enumerate() {
        // 时长硬顶:超过 60s 立即停止(剩余步骤标记 skipped)
        if started.elapsed().as_millis() > MAX_TOTAL_MS {
            for (j, s) in steps.iter().enumerate().skip(i) {
                let op = s.get("op").and_then(Value::as_str).unwrap_or("?");
                outcome.results.push(json!({
                    "i": j, "op": op, "ok": false, "skipped": true,
                    "detail": "整批执行超过 60s 硬顶,本步骤跳过"
                }));
                outcome.failed.push(j);
            }
            break;
        }
        // 每步执行前可选等待(步骤节奏控制)
        let delay = step
            .get("delay_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(MAX_STEP_DELAY_MS);
        if delay > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        let record = execute_step(window_id, driver.as_ref(), step, i, &mut outcome.read_values);
        let ok = record["ok"].as_bool().unwrap_or(false);
        if !ok {
            outcome.failed.push(i);
            outcome.results.push(record);
            if !continue_on_error {
                // fail-fast:剩余步骤标记 skipped(可观测,不静默丢弃)
                for (j, s) in steps.iter().enumerate().skip(i + 1) {
                    let op = s.get("op").and_then(Value::as_str).unwrap_or("?");
                    outcome.results.push(json!({
                        "i": j, "op": op, "ok": false, "skipped": true,
                        "detail": "前置步骤失败(fail-fast),本步骤未执行;可 continue_on_error=true 改为继续"
                    }));
                }
                break;
            }
        } else {
            outcome.results.push(record);
        }
    }
    outcome.elapsed_ms = started.elapsed().as_millis();
    Ok(outcome)
}

/// 取步骤整型参数(第 91 轮起 pub(super),供 run_sequence 复用)。
pub(super) fn step_i64(step: &Value, key: &str) -> Option<i64> {
    step.get(key).and_then(Value::as_i64)
}

/// 取步骤字符串参数(trim + 非空;第 91 轮起 pub(super),供 run_sequence 复用)。
pub(super) fn step_str<'a>(step: &'a Value, key: &str) -> Option<&'a str> {
    step.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 执行单步,返回 {i, op, ok, route?, detail} 记录。
/// (第 91 轮起 pub(super):run_sequence 连续工作模式复用同一执行函数。)
pub(super) fn execute_step(
    window_id: &str,
    driver: &dyn crate::agent::window::WindowDriver,
    step: &Value,
    idx: usize,
    read_values: &mut serde_json::Map<String, Value>,
) -> Value {
    let op = step.get("op").and_then(Value::as_str).unwrap_or("");
    let fail = |detail: String| -> Value {
        json!({"i": idx, "op": op, "ok": false, "detail": detail})
    };
    let done = |detail: String| -> Value {
        // 从驱动返回文案提取 route= 标注(有则透传)
        let route = detail
            .split("route=")
            .nth(1)
            .map(|r| r.split([')', ' ']).next().unwrap_or(r).to_string());
        let mut v = json!({"i": idx, "op": op, "ok": true, "detail": detail});
        if let Some(r) = route {
            v["route"] = json!(r);
        }
        v
    };

    let need_point = |step: &Value| -> std::result::Result<(i64, i64), Value> {
        match (step_i64(step, "x"), step_i64(step, "y")) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => Err(fail(format!("op={op} 缺少整数参数 x / y(屏幕绝对坐标)"))),
        }
    };

    match op {
        // ===== 鼠标 =====
        "mouse_move" => match need_point(step) {
            Ok((x, y)) => match driver.act(window_id, "/", ControlAction::MovePoint { x, y }) {
                Ok(d) => done(d),
                Err(e) => fail(e.to_string()),
            },
            Err(v) => v,
        },
        "mouse_click" => {
            let (x, y) = match need_point(step) {
                Ok(p) => p,
                Err(v) => return v,
            };
            let button = step_str(step, "button").unwrap_or("left");
            let clicks = step.get("clicks").and_then(Value::as_u64).unwrap_or(1).clamp(1, 2) as u8;
            let modifiers = step_str(step, "modifiers").map(str::to_string);
            let action = match button {
                "left" => ControlAction::ClickPoint {
                    x,
                    y,
                    modifiers: modifiers.clone(),
                },
                "right" => ControlAction::RightClickPoint {
                    x,
                    y,
                    modifiers: modifiers.clone(),
                },
                // 第 96 轮:中键也支持 modifiers(ctrl+中键新标签打开 / shift+中键平移视图)
                "middle" => ControlAction::MiddleClickPoint {
                    x,
                    y,
                    modifiers: modifiers.clone(),
                },
                other => return fail(format!("button 非法: {other}(仅 left/right/middle)")),
            };
            // 左键点击 2 次 = DoubleClickPoint(中键双击极少用,保持原 MiddleClickPoint 不变)
            let action = if matches!(action, ControlAction::ClickPoint { .. }) && clicks == 2 {
                ControlAction::DoubleClickPoint { x, y, modifiers }
            } else {
                action
            };
            match driver.act(window_id, "/", action) {
                Ok(d) => done(d),
                Err(e) => fail(e.to_string()),
            }
        }
        "mouse_drag" => {
            let (x, y) = match need_point(step) {
                Ok(p) => p,
                Err(v) => return v,
            };
            let (x2, y2) = match (step_i64(step, "x2"), step_i64(step, "y2")) {
                (Some(a), Some(b)) => (a, b),
                _ => return fail("op=mouse_drag 缺少整数参数 x2 / y2(拖拽终点)".into()),
            };
            let modifiers = step_str(step, "modifiers").map(str::to_string);
            match driver.act(
                window_id,
                "/",
                ControlAction::DragPoint {
                    x,
                    y,
                    x2,
                    y2,
                    modifiers,
                },
            ) {
                Ok(d) => done(d),
                Err(e) => fail(e.to_string()),
            }
        }
        "mouse_scroll" => {
            let (x, y) = match need_point(step) {
                Ok(p) => p,
                Err(v) => return v,
            };
            let direction = step_str(step, "direction").unwrap_or("down");
            let lines = step.get("lines").and_then(Value::as_i64).unwrap_or(3).clamp(1, 100);
            let signed = match direction {
                "up" | "上" => lines,
                "down" | "下" => -lines,
                other => return fail(format!("direction 非法: {other}(仅 up/down)")),
            };
            match driver.act(
                window_id,
                "/",
                ControlAction::ScrollPoint {
                    x,
                    y,
                    lines: signed as i32,
                },
            ) {
                Ok(d) => done(d),
                Err(e) => fail(e.to_string()),
            }
        }
        // ===== 键盘 =====
        "key_press" => {
            let Some(keys) = step_str(step, "keys") else {
                return fail("op=key_press 缺少 string 参数 keys(如 \"enter\" / \"ctrl+a\")".into());
            };
            match driver.act(window_id, "/", ControlAction::SendKeys(keys.to_string())) {
                Ok(d) => done(d),
                Err(e) => fail(e.to_string()),
            }
        }
        "type_text" => {
            let Some(text) = step.get("text").and_then(Value::as_str) else {
                return fail("op=type_text 缺少 string 参数 text".into());
            };
            match driver.act(window_id, "/", ControlAction::TypeText(text.to_string())) {
                Ok(d) => done(d),
                Err(e) => fail(e.to_string()),
            }
        }
        // ===== 控件树路线(自动 T1→T2→T3) =====
        "click" => {
            let Some(path) = step_str(step, "path") else {
                return fail("op=click 缺少 string 参数 path(控件路径,取 action=inspect)".into());
            };
            match driver.act(window_id, path, ControlAction::Click) {
                Ok(d) => done(d),
                Err(e) => fail(e.to_string()),
            }
        }
        "set_text" => {
            let Some(path) = step_str(step, "path") else {
                return fail("op=set_text 缺少 string 参数 path".into());
            };
            let Some(text) = step.get("text").and_then(Value::as_str) else {
                return fail("op=set_text 缺少 string 参数 text".into());
            };
            match driver.act(window_id, path, ControlAction::SetText(text.to_string())) {
                Ok(d) => done(d),
                Err(e) => fail(e.to_string()),
            }
        }
        "get_text" => {
            let Some(path) = step_str(step, "path") else {
                return fail("op=get_text 缺少 string 参数 path".into());
            };
            match driver.act(window_id, path, ControlAction::GetText) {
                Ok(text) => {
                    let key = step_str(step, "key").unwrap_or(path).to_string();
                    read_values.insert(key.clone(), json!(text));
                    json!({"i": idx, "op": op, "ok": true, "key": key, "detail": text})
                }
                Err(e) => fail(e.to_string()),
            }
        }
        // ===== 等待 =====
        "wait" => {
            let ms = step.get("ms").and_then(Value::as_u64).unwrap_or(200);
            if ms > MAX_WAIT_MS {
                return fail(format!("wait ms={ms} 超上限 {MAX_WAIT_MS}"));
            }
            std::thread::sleep(std::time::Duration::from_millis(ms));
            json!({"i": idx, "op": op, "ok": true, "detail": format!("已等待 {ms}ms")})
        }
        other => fail(format!(
            "未知 op: {other};合法: mouse_move / mouse_click / mouse_drag / mouse_scroll / \
             key_press / type_text / click / set_text / get_text / wait"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_step_rejects_unknown_op() {
        let driver = crate::agent::window::current_driver();
        let mut reads = serde_json::Map::new();
        let v = execute_step("whatever", driver.as_ref(), &json!({"op": "explode"}), 0, &mut reads);
        assert_eq!(v["ok"], json!(false));
        assert!(v["detail"].as_str().unwrap().contains("未知 op"));
    }

    #[test]
    fn execute_step_wait_guard() {
        let driver = crate::agent::window::current_driver();
        let mut reads = serde_json::Map::new();
        // 超上限 wait → 结构化失败(不执行 sleep)
        let v = execute_step(
            "w",
            driver.as_ref(),
            &json!({"op": "wait", "ms": 99999}),
            0,
            &mut reads,
        );
        assert_eq!(v["ok"], json!(false));
        assert!(v["detail"].as_str().unwrap().contains("超上限"));
        // 合法 wait → ok
        let v = execute_step(
            "w",
            driver.as_ref(),
            &json!({"op": "wait", "ms": 10}),
            0,
            &mut reads,
        );
        assert_eq!(v["ok"], json!(true));
    }

    #[test]
    fn execute_step_mouse_click_validates_params() {
        let driver = crate::agent::window::current_driver();
        let mut reads = serde_json::Map::new();
        // 缺坐标 → 参数错误(不到驱动层)
        let v = execute_step(
            "w",
            driver.as_ref(),
            &json!({"op": "mouse_click"}),
            0,
            &mut reads,
        );
        assert_eq!(v["ok"], json!(false));
        assert!(v["detail"].as_str().unwrap().contains("x / y"));
        // 非法 button
        let v = execute_step(
            "w",
            driver.as_ref(),
            &json!({"op": "mouse_click", "x": 1, "y": 2, "button": "side"}),
            0,
            &mut reads,
        );
        assert!(v["detail"].as_str().unwrap().contains("button 非法"));
    }

    #[test]
    fn execute_step_mouse_click_middle_with_modifiers() {
        // 第 96 轮:mouse_click button=middle + modifiers 应构造 MiddleClickPoint{modifiers}
        let driver = crate::agent::window::current_driver();
        let mut reads = serde_json::Map::new();
        // 仅校验参数构造(不实际执行驱动动作):用缺 x/y 先触发参数错误路径,
        // 再用合法 x/y + button=middle + modifiers 验证构造成功(驱动层在 Linux 走 fallback 报错,但参数构造正确)
        let v = execute_step(
            "w",
            driver.as_ref(),
            &json!({"op": "mouse_click", "x": 10, "y": 20, "button": "middle", "modifiers": "ctrl"}),
            0,
            &mut reads,
        );
        // 参数构造应成功(驱动层可能因平台报错,但不应是「参数错误」)
        let detail = v["detail"].as_str().unwrap();
        assert!(
            !detail.contains("x / y") && !detail.contains("button 非法"),
            "中键 modifiers 参数构造应成功,实际: {detail}"
        );
    }
}
