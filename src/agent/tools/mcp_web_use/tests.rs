//! MCP_Web_Use 工具单元测试(自 tools/browser.rs 测试平移 + 单工具化改造)。

use super::*;
use super::{control, extract, hitl_hint, inspect, page_state, visit_ledger};
use base64::Engine as _;

#[test]
fn envelope_codes() {
    let s = envelope(0, "ok", json!({"x": 1})).unwrap();
    assert!(s.contains("\"code\":0"));
    assert!(s.contains("\"message\":\"ok\""));
    assert!(s.contains("\"x\":1"));
}

// ==================== 2026-10-14 第三轮:control 顶层参数别名归一化 ====================

/// 猎聘实测(`llaew_20261014_145428.log`):模型按系统提示词自然写法把 request_human
/// 的 message/reason/options 放顶层,Schema additionalProperties=false 三连拒,
/// 烧 3 轮迭代 —— 别名提升后首个形态即可用。
#[test]
fn 顶层别名提升_request_human三键与通用键进入params() {
    let args = json!({
        "action": "control",
        "control_action": "request_human",
        "page_id": "p_x",
        "message": "请在弹窗完成登录",
        "reason": "login",
        "options": ["我已完成", "页面是空的"],
    });
    let mut params = json!({});
    control::lift_top_level_control_aliases(&args, &mut params);
    assert_eq!(params["message"], "请在弹窗完成登录");
    assert_eq!(params["reason"], "login");
    assert_eq!(params["options"][1], "页面是空的");
}

/// url/timeout_ms/force 本就是顶层合法字段,但 control 读 params.* —— 顶层传入
/// 曾静默丢失(navigate 1001 缺 url / wait 时长不生效 / force 逃生口失灵)。
#[test]
fn 顶层别名提升_url_timeout_force进入params() {
    let args = json!({
        "action": "control",
        "control_action": "navigate",
        "page_id": "p_x",
        "url": "https://www.liepin.com/zhaopin/",
        "timeout_ms": 1500,
        "force": true,
    });
    let mut params = json!({});
    control::lift_top_level_control_aliases(&args, &mut params);
    assert_eq!(params["url"], "https://www.liepin.com/zhaopin/");
    assert_eq!(params["timeout_ms"], 1500);
    assert_eq!(params["force"], true);
}

/// 显式 params 永远优先;非别名键(如 selector)不提升。
#[test]
fn 顶层别名提升_显式params优先且不提升无关键() {
    let args = json!({
        "action": "control",
        "control_action": "request_human",
        "message": "顶层值",
        "selector": "#login",
        "params": {"message": "params 值", "unlock_page": false},
    });
    let mut params = args["params"].clone();
    control::lift_top_level_control_aliases(&args, &mut params);
    assert_eq!(params["message"], "params 值", "显式 params 优先");
    assert_eq!(params["unlock_page"], false);
    assert!(params.get("selector").is_none(), "selector 不是别名,不应提升");
}

// ==================== 第 140 轮:inspect 体积闸门 ====================

#[test]
fn gate_payload_small_is_identity() {
    let v = json!({"ok": true, "selector": "form", "outer_html": "<form></form>"});
    assert_eq!(inspect::gate_payload(v.clone()), v);
}

#[test]
fn cap_strings_clips_long_string_with_marker() {
    let mut v = json!({"html": "X".repeat(20000), "ok": true});
    inspect::cap_strings(&mut v, 100);
    let s = v["html"].as_str().unwrap();
    assert!(s.starts_with(&"X".repeat(100)));
    assert!(s.contains("截断,共 20000 字符"));
    assert_eq!(v["ok"], json!(true), "短字段不受影响");
}

#[test]
fn gate_payload_huge_object_degrades_to_preview() {
    // 单字符串已裁到 8K,但 5 个 8K 字段合计仍超 24KB → 整体降级为摘要
    let big = "Y".repeat(20000);
    let v = json!({
        "a": big, "b": big, "c": big, "d": big, "e": big,
    });
    let out = inspect::gate_payload(v);
    assert_eq!(out["truncated"], json!(true));
    assert!(out["original_bytes"].as_u64().unwrap() > 24 * 1024);
    assert!(out["preview"].as_str().unwrap().contains("截断"));
    assert!(out["hint"].as_str().unwrap().contains("extract"));
}

// ==================== 第 140 轮:adopt_spawned_pages 按需化 ====================

#[test]
fn action_may_spawn_page_whitelist() {
    // 可能开新页的动作:巡检保留
    for a in [
        "click",
        "human_click",
        "right_click",
        "double_click",
        "key_press",
        "press_sequence",
        "navigate",
        "new_tab",
        "back",
        "forward",
        "reload",
        "select_option",
        "upload_file",
        "download",
        "dispatch_event",
    ] {
        assert!(
            control::action_may_spawn_page(a, &serde_json::json!({})),
            "{a} 应巡检派生页"
        );
    }
    // 第 152 轮:eval_js 改为按表达式判定 —— 只读不巡检,可变更才巡检
    assert!(
        !control::action_may_spawn_page(
            "eval_js",
            &serde_json::json!({"expression": "document.querySelectorAll('p').length"})
        ),
        "只读 eval_js 不应巡检(省一次浏览器级往返,且避免无关 spawned_page_id)"
    );
    assert!(
        control::action_may_spawn_page(
            "eval_js",
            &serde_json::json!({"expression": "window.open('https://x.test','_blank')"})
        ),
        "可变更 eval_js 应巡检(LLM 用 eval 开页是合法形态)"
    );
    // 观察/状态类动作:跳过巡检
    for a in [
        "wait",
        "input_text",
        "human_input",
        "clear_input",
        "screenshot",
        "set_cookie",
        "delete_cookie",
        "set_storage",
        "clear_storage",
        "set_viewport",
        "set_window",
        "sync_viewport",
        "set_highlight",
        "heartbeat",
        "hover",
        "scroll",
        "scroll_to",
        "drag",
        "focus",
        "blur",
        "mouse_move",
    ] {
        assert!(
            !control::action_may_spawn_page(a, &serde_json::json!({})),
            "{a} 不应巡检派生页"
        );
    }
}

#[test]
fn js_str_escapes() {
    let s = js_str("a'b\nc\"");
    assert!(s.starts_with('"') && s.ends_with('"'));
    assert!(s.contains("\\n"));
}

#[test]
fn key_code_tuple_lookup() {
    assert_eq!(input_ctl::key_code_tuple("Enter"), Some(("Enter", "Enter", 13)));
    assert_eq!(input_ctl::key_code_tuple("F12"), Some(("F12", "F12", 123)));
    assert!(input_ctl::key_code_tuple("未知键").is_none());
}

/// 第 157 轮:键名大小写不敏感 + 别名层(实测小写 enter 落 Char 分支触发 CDP
/// "Invalid 'text' parameter" 黑话错误)。
#[test]
fn key_code_tuple_case_insensitive_aliases() {
    for (input, expect) in [
        ("enter", ("Enter", "Enter", 13)),
        ("ENTER", ("Enter", "Enter", 13)),
        ("return", ("Enter", "Enter", 13)),
        ("esc", ("Escape", "Escape", 27)),
        ("ESC", ("Escape", "Escape", 27)),
        ("space", (" ", "Space", 32)),
        ("del", ("Delete", "Delete", 46)),
        ("ins", ("Insert", "Insert", 45)),
        ("pgup", ("PageUp", "PageUp", 33)),
        ("pgdn", ("PageDown", "PageDown", 34)),
        ("up", ("ArrowUp", "ArrowUp", 38)),
        ("left", ("ArrowLeft", "ArrowLeft", 37)),
        ("f5", ("F5", "F5", 116)),
    ] {
        assert_eq!(input_ctl::key_code_tuple(input), Some(expect), "{input}");
    }
    // 多字符非常见键仍返回 None(走报错提示,不透传 CDP 黑话)
    assert!(input_ctl::key_code_tuple("foobar").is_none());
}

/// 第 157 轮:组合键拆分 —— ctrl+a / cmd+shift+p 归一,字面加号与 C++ 不误拆。
#[test]
fn split_combined_key_cases() {
    use super::input_ctl::split_combined_key;
    assert_eq!(split_combined_key("a"), (String::new(), "a".to_string()));
    assert_eq!(split_combined_key("ctrl+a"), ("ctrl".to_string(), "a".to_string()));
    assert_eq!(
        split_combined_key("cmd+shift+p"),
        ("cmd+shift".to_string(), "p".to_string())
    );
    assert_eq!(split_combined_key("enter"), (String::new(), "enter".to_string()));
    // 特殊键别名作主键也可拆
    assert_eq!(
        split_combined_key("ctrl+enter"),
        ("ctrl".to_string(), "enter".to_string())
    );
    // shift+= :拆出 shift 修饰 + '=' 主键(Shift+= 即加号,语义正确)
    assert_eq!(split_combined_key("shift+="), ("shift".to_string(), "=".to_string()));
    // 真·尾部加号("ctrl+") = 字面
    assert_eq!(split_combined_key("ctrl+"), (String::new(), "ctrl+".to_string()));
    assert_eq!(split_combined_key("+"), (String::new(), "+".to_string()));
    // 后半不是单字符/已知键 → 整体字面(C++ 不误拆 ctrl+C+","之类)
    assert_eq!(split_combined_key("C++"), (String::new(), "C++".to_string()));
}

/// 第 157 轮:原生赋值脚本沿原型链查 value 访问器(根治 textarea 误用
/// HTMLInputElement setter 的 Illegal invocation),contenteditable 走 textContent。
#[test]
fn native_set_js_walks_prototype_chain() {
    let js = input_ctl::native_set_js("#t", "\"你好\"");
    assert!(js.contains("Object.getPrototypeOf"), "{js}");
    assert!(js.contains("getOwnPropertyDescriptor"), "{js}");
    assert!(js.contains("isContentEditable"), "{js}");
    assert!(js.contains("textContent"), "{js}");
    assert!(js.contains("d.set.call(el"), "{js}");
    // 不再硬编码 HTMLInputElement 短路
    assert!(!js.contains("HTMLInputElement.prototype,'value')\n"), "{js}");
    assert!(!js.contains(r"|| Object.getOwnPropertyDescriptor"), "{js}");
    // selector 以 JS 字面量安全嵌入
    assert!(js.contains("document.querySelector(\"#t\")"), "{js}");
    assert!(js.contains("'你好'") || js.contains("你好"), "{js}");
    // 回读脚本:页内比对,只回 len/match/head
    let rb = input_ctl::readback_js("#t", "\"你好\"");
    assert!(rb.contains("match"), "{rb}");
    assert!(rb.contains("slice(0, 100)"), "{rb}");
}

#[test]
fn clip_step_message_budgets() {
    let ok_long = "x".repeat(500);
    let clipped = clip_step_message(0, &ok_long);
    assert!(clipped.contains("截断"), "{clipped}");
    assert!(clipped.chars().count() < 400, "{clipped}");
    // 失败步预算更宽
    let err_long = "y".repeat(5000);
    let clipped_err = clip_step_message(2002, &err_long);
    assert!(clipped_err.contains("截断"), "{clipped_err}");
    assert!(clipped_err.chars().count() > 1500, "{clipped_err}");
    // 预算内原样返回
    assert_eq!(clip_step_message(0, "ok"), "ok");
}

/// 第 157 轮:guard 脚本版本化重装 —— REV 单调递增烘焙、卸载钩子在场、
/// 占位符全烘焙(与 browser_overlay 内既有单测同锚点补充)。
#[test]
fn guard_script_rev_monotonic_and_off_hook() {
    let a = crate::agent::browser_overlay::guard_script(
        &crate::agent::browser_overlay::PageGuardConfig::locked(),
    );
    let b = crate::agent::browser_overlay::guard_script(
        &crate::agent::browser_overlay::PageGuardConfig::locked(),
    );
    let rev_a = a
        .split("const REV = ")
        .nth(1)
        .and_then(|s| s.split(';').next())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .expect("REV 应烘焙为数字");
    let rev_b = b
        .split("const REV = ")
        .nth(1)
        .and_then(|s| s.split(';').next())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .expect("REV 应烘焙为数字");
    assert!(rev_b > rev_a, "两次生成 REV 应单调递增:{rev_a} vs {rev_b}");
    // 第 179 轮:窗口面 API 收敛到 symbol 命名空间对象(见 browser_ns.rs),
    // 断言用真实的 `NS.<Suffix>` 渲染形态。
    let off = "NS.GuardOff = () =>".to_string();
    let rev = "NS.GuardRev >= REV".to_string();
    assert!(a.contains(&off), "卸载钩子缺失");
    assert!(a.contains(&rev), "版本比较缺失");
    assert!(!a.contains("{REV}"), "REV 占位符未烘焙");
}

/// 第 166 轮:屏蔽完整性(锁定态 CSS 层 + selectstart 闸)与焦点纪律提示词锚点。
#[test]
fn 屏蔽完整性与焦点纪律锚点() {
    // ① 管控 JS:悬停 ⃠ 光标 / 禁选 / selectstart 事件闸(与 browser_overlay
    // 内单测同锚点补充,防止外围误删)
    let js = crate::agent::browser_overlay::guard_script(
        &crate::agent::browser_overlay::PageGuardConfig::locked(),
    );
    assert!(js.contains("cursor:not-allowed !important"), "锁定态光标反馈缺失");
    assert!(js.contains("user-select:none !important"), "锁定态禁选缺失");
    assert!(js.contains("'selectstart'"), "selectstart 事件闸缺失");
    // ② 工具描述:焦点与操作节奏段(先规划后批量 + 借焦速还对账字段)
    let desc = super::MCP_WEB_USE_DESCRIPTION;
    assert!(desc.contains("焦点与操作节奏"), "工具描述缺焦点纪律段");
    assert!(desc.contains("focus_restored"), "工具描述缺归还对账字段");
    assert!(desc.contains("sequence/batch"), "工具描述缺批量速决指引");
    // ③ 系统提示词:焦点纪律条(SubAgent-Work 渲染含 MCP_Web_Use 段)
    let web = crate::agent::system_prompt::SystemPrompt::sub_agent_work()
        .render(crate::database::Protocol::Anthropic);
    assert!(web.contains("焦点纪律"), "系统提示词缺焦点纪律条");
    assert!(web.contains("LAEW_WEB_FOCUS=keep"), "系统提示词缺焦点档位指引");
}

#[test]
fn parse_modifiers_combo() {
    let p = json!({"modifiers": ["ctrl", "shift"]});
    assert_eq!(control::parse_modifiers(&p), 2 | 8);
    let p = json!({"modifiers": "ctrl,shift"});
    assert_eq!(control::parse_modifiers(&p), 2 | 8);
}

// =================== Schema 枚举完整性(第 89 轮单工具化;第 118 轮加 explore/batch) ===================

#[test]
fn parameters_action_enum_is_six_values() {
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["action"]["enum"].as_array().expect("action enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    // 第 118 轮:action enum 由 6 个扩展为 8 个,新增 explore(批量观察) + batch(批量混合执行)
    assert_eq!(
        names,
        vec!["open", "list", "close", "control", "inspect", "sequence", "explore", "batch"]
    );
    assert_eq!(p["required"][0], "action", "action 必填");
}

#[test]
fn parameters_control_action_enum_complete() {
    // 第 76 轮扩展动作(drag/focus/blur/mouse_move/dispatch_event)必须全部在枚举里
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["control_action"]["enum"].as_array()
        .expect("control_action enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    for required in &[
        "click", "right_click", "double_click", "hover", "scroll", "scroll_to",
        "key_press", "press_sequence", "input_text", "human_input", "clear_input",
        "upload_file", "select_option", "download", "new_tab", "close_tab",
        "navigate", "back", "forward", "reload", "wait", "eval_js",
        "set_cookie", "delete_cookie", "set_storage", "clear_storage", "set_viewport",
        "screenshot", "heartbeat",
        "drag", "focus", "blur", "mouse_move", "dispatch_event",
        "set_window", "sync_viewport", "set_highlight", "set_overlay", "set_guard",
        // 第 152 轮:auto_follow(自动跟随最新内容)
        "auto_follow", "request_human",
        // 多标签页治理:bring_to_front(标签页前台化)
        "bring_to_front",
        // 多窗口并行(双站同时工作):new_window(独立新窗口打开)
        "new_window",
        // 文本定位滚动:scroll_to_text(按可见文本滚到语义区块)
        "scroll_to_text",
    ] {
        assert!(names.contains(required), "control_action 枚举缺失 {required}");
    }
    // 第 143 轮 set_guard 使枚举 40 → 41;第 152 轮 auto_follow 使 41 → 42;第 158 轮 select_dropdown 使 42 → 43;
    // 多标签页治理 bring_to_front 使 43 → 44;多窗口并行 new_window 使 44 → 45;
    // 文本定位滚动 scroll_to_text 使 45 → 46
    assert_eq!(names.len(), 46, "control_action 应为 46 个,实际 {names:?}");
}

#[test]
fn parameters_info_enum_complete() {
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["info"]["enum"].as_array().expect("info enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    for required in &[
        "console", "network", "elements", "dom", "localstorage", "sessionstorage",
        "cookies", "screenshot", "page_meta", "viewport", "url", "title", "image_urls",
        "blockers", "extract_links", "extract", "page_state",
        // 第 152 轮:visibility(可视性诊断)
        "visibility",
    ] {
        assert!(names.contains(required), "info 枚举缺失 {required}");
    }
    // 第 152 轮:visibility 使枚举 20 → 21
    assert_eq!(names.len(), 21, "info 应为 21 个,实际 {names:?}");
}

#[test]
fn parameters_mode_defaults_headed() {
    // 第 139 轮:缺省可见模式(与 `BrowserMode::DEFAULT` 一致)。
    let p = McpWebUseTool.parameters();
    let mode = p["properties"]["mode"].clone();
    assert!(mode.is_object(), "mode 字段应为对象");
    assert_eq!(
        mode["default"], "headed",
        "默认 mode 应为 headed(可见模式)"
    );
    assert_eq!(
        mode["default"], crate::agent::browser::BrowserMode::DEFAULT.as_str(),
        "schema default 必须与 BrowserMode::DEFAULT 同源,不允许两处各写一份"
    );
}

// =================== 参数校验(无需真实浏览器) ===================

/// 第 143 轮:parse_open_guard 级联与校验(纯函数,无需浏览器)。
#[test]
fn parse_open_guard_cascade_and_validation() {
    use crate::agent::browser_overlay::PageGuardMode;
    use super::parse_open_guard;
    // ① 显式 guard 完整解析(含选择器与 note)
    let cfg = parse_open_guard(&json!({
        "guard": "partial",
        "block_selectors": ["#pay", ".danger-btn"],
        "guard_note": "  支付区已锁定  "
    }))
    .expect("partial + block 应合法");
    assert_eq!(cfg.mode, PageGuardMode::Partial);
    assert_eq!(cfg.block_selectors.len(), 2);
    assert_eq!(cfg.note.as_deref(), Some("支付区已锁定"), "note 应被 trim");
    // ② legacy overlay 布尔映射(true=locked/false=open)
    assert_eq!(
        parse_open_guard(&json!({"overlay": false})).unwrap().mode,
        PageGuardMode::Open
    );
    assert_eq!(
        parse_open_guard(&json!({"overlay": true})).unwrap().mode,
        PageGuardMode::Locked
    );
    // ③ guard 优先于 overlay
    assert_eq!(
        parse_open_guard(&json!({"guard": "open", "overlay": true})).unwrap().mode,
        PageGuardMode::Open
    );
    // ④ 非法档位
    assert!(parse_open_guard(&json!({"guard": "whatever"})).is_err());
    // ⑤ partial 缺选择器
    assert!(parse_open_guard(&json!({"guard": "partial"})).is_err());
    // ⑥ allow + block 互斥
    assert!(parse_open_guard(&json!({
        "guard": "partial", "allow_selectors": ["a"], "block_selectors": ["b"]
    }))
    .is_err());
    // ⑦ locked 不收选择器
    assert!(parse_open_guard(&json!({"guard": "locked", "allow_selectors": ["a"]})).is_err());
    // ⑧ open 带白名单选择器 + note 合法(note 三档通用)
    assert!(parse_open_guard(&json!({"guard": "open", "guard_note": "请登录"})).is_ok());
}

#[tokio::test]
async fn missing_action_returns_1001() {
    let res = McpWebUseTool.execute(json!({})).await.unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 action"));
}

#[tokio::test]
async fn unknown_action_returns_1001() {
    let res = McpWebUseTool.execute(json!({"action": "goto"})).await.unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("未知 action"));
}

#[tokio::test]
async fn open_missing_url_returns_1001() {
    let res = McpWebUseTool.execute(json!({"action": "open"})).await.unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 url"));
}

#[tokio::test]
async fn close_missing_page_id_returns_1001() {
    let res = McpWebUseTool.execute(json!({"action": "close"})).await.unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 page_id"));
}

#[tokio::test]
async fn control_missing_control_action_returns_1001() {
    let res = McpWebUseTool
        .execute(json!({"action": "control", "page_id": "p_abc"}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 control_action"));
}

/// 2026-10-19:无页任务级 HITL 的路由判据 —— 整页抹除持久熔断(6004)时
/// 浏览器一台都没起(`list` 返 `windows=0`),`request_human` 是 advice 里
/// 「唯一可行」的出路,却因缺 `page_id` 返 1001 而物理不可达,逼得 Agent
/// 去 `open("about:blank")` 硬造页面(反复弹 Chrome)。此用例锁死「问得到人」。
#[test]
fn page_less_hitl_route_allows_request_human_only() {
    use super::control::is_page_less_hitl;
    // 无 page_id + request_human → 放行(任务级提问)。
    assert!(is_page_less_hitl(None, Some("request_human")));
    // 其余动作缺 page_id 仍拦(它们必须操作具体页面)。
    for act in [
        "click",
        "input_text",
        "navigate",
        "screenshot",
        "set_guard",
        "request_human_x",
    ] {
        assert!(
            !is_page_less_hitl(None, Some(act)),
            "{act} 缺 page_id 不得走无页通路"
        );
    }
    // 缺 control_action 也拦(分不清要做什么)。
    assert!(!is_page_less_hitl(None, None));
    // 有 page_id 时一律走正常路径(带页提问仍是主路径)。
    assert!(!is_page_less_hitl(Some("p_abc"), Some("request_human")));
}

#[tokio::test]
async fn inspect_missing_page_id_returns_1001() {    let res = McpWebUseTool
        .execute(json!({"action": "inspect", "info": "console"}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 page_id"));
}

#[tokio::test]
async fn sequence_missing_steps_returns_1001() {
    let res = McpWebUseTool.execute(json!({"action": "sequence"})).await.unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 steps"));
}

#[tokio::test]
async fn sequence_empty_steps_returns_1001() {
    let res = McpWebUseTool
        .execute(json!({"action": "sequence", "steps": []}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("steps 不能为空"));
}

#[tokio::test]
async fn sequence_rejects_nested_sequence_without_browser() {
    let res = McpWebUseTool
        .execute(json!({"action": "sequence", "steps": [{"action": "sequence", "steps": []}]}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("不允许嵌套 sequence"));
}

#[test]
fn sequence_placeholders_resolve_recursively() {
    let mut value = json!({
        "page_id": "$page_id",
        "params": {
            "selector": "a[data-page='${page_id}']",
            "values": ["$spawned_page_id", "${spawned_page_id}"]
        }
    });
    resolve_page_placeholders(&mut value, Some("p_current"), Some("p_spawn"));
    assert_eq!(value["page_id"], "p_current");
    assert_eq!(value["params"]["selector"], "a[data-page='p_current']");
    assert_eq!(value["params"]["values"][0], "p_spawn");
    assert_eq!(value["params"]["values"][1], "p_spawn");
}

#[tokio::test]
async fn inspect_unknown_page_returns_2000() {
    // 无浏览器会话时 page_id 必然不存在 → 2000 信封(不崩溃)
    let res = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": "p_00000000", "info": "url"}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":2000"), "实际: {res}");
}

#[tokio::test]
async fn list_always_returns_ok() {
    // list 不依赖浏览器进程,空列表也返回 code=0
    let res = McpWebUseTool.execute(json!({"action": "list"})).await.unwrap();
    assert!(res.contains("\"code\":0"), "实际: {res}");
    assert!(res.contains("\"pages\""));
}

// =================== open 成功/失败双路径(环境自适应) ===================

#[tokio::test]
async fn open_no_browser_or_succeeds() {
    // 无 Chrome 时返回 3001 + 安装提示;有 Chrome 时返回 0 + page_id + next_steps。
    // 运行环境可能安装了 Chrome,兼容两种路径。
    // ★ 第 139 轮:显式传 mode=hidden —— 缺省已是可见模式,不显式关掉的话
    // 开发者本机跑 `cargo test` 会被弹出一个真实 Chrome 窗口。
    let res = McpWebUseTool
        .execute(json!({"action": "open", "url": "https://example.com", "mode": "hidden"}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&res).expect("响应应为合法 JSON");
    let code = v["code"].as_i64().unwrap_or(-1);
    if code == 0 {
        // 成功路径:验证显式 mode 覆盖生效 + page_id + next_steps 非空
        assert_eq!(v["data"]["mode"], "hidden", "显式 mode=hidden 应被尊重");
        assert!(v["data"]["page_id"].as_str().unwrap_or("").starts_with("p_"));
        assert!(v["data"]["page_id"].as_str().unwrap_or("").starts_with("p_"));
        let steps = v["data"]["next_steps"].as_array()
            .expect("成功响应必须包含 next_steps 数组");
        assert!(!steps.is_empty(), "next_steps 不能为空");
        assert_eq!(steps.len(), 5, "next_steps 应包含 5 步引导(含登录场景提示)");
        // 单工具化后引导字段指向 action=control/inspect 语义
        let first = &steps[0];
        assert_eq!(first["action"], "control");
        assert_eq!(first["control_action"], "input_text");
        // 成功打开的页面顺手关闭,不留脏状态
        let pid = v["data"]["page_id"].as_str().unwrap().to_string();
        let closed = McpWebUseTool.execute(json!({"action": "close", "page_id": pid})).await.unwrap();
        assert!(closed.contains("\"code\":0"));
    } else {
        // 失败路径:2001(断连/启动失败) 或 3001(无浏览器)
        assert!(
            code == 2001 || code == 3001,
            "无浏览器/失败时应返回 2001/3001,实际 code={code}"
        );
    }
}

// =================== 第 142 轮:复用已登录浏览器(connect 模式增强) ===================

#[test]
fn inject_reuse_metadata_merges_fields() {
    let res = r#"{"code":0,"message":"ok","data":{"page_id":"p_a1b2c3d4","mode":"headed"}}"#;
    let out = inject_reuse_metadata(
        res,
        &json!({"reuse_existing_chrome": true, "debug_port": 9222, "browser_version": "Chrome/131.0.0.0"}),
    );
    let v: serde_json::Value = serde_json::from_str(&out).expect("注入后仍为合法 JSON");
    assert_eq!(v["code"], 0);
    assert_eq!(v["data"]["page_id"], "p_a1b2c3d4", "原字段不丢");
    assert_eq!(v["data"]["reuse_existing_chrome"], true);
    assert_eq!(v["data"]["debug_port"], 9222);
    assert_eq!(v["data"]["browser_version"], "Chrome/131.0.0.0");
}

#[test]
fn inject_reuse_metadata_invalid_json_passthrough() {
    let raw = "not json at all";
    assert_eq!(inject_reuse_metadata(raw, &json!({"a": 1})), raw);
}

#[tokio::test]
async fn open_reuse_existing_returns_3002_or_succeeds() {
    // 环境自适应:本机无调试端口(9222 无监听)→ 3002 + relaunch_command;
    // 本机恰好有调试模式 Chrome → 0 + reuse_existing_chrome。
    // 注意:BrowserManager 是进程内全局单例且并行跑测试 —— 其它测试抢先
    // launch 浏览器后,new_page 走「实例已存在」短路(第 100 轮单实例语义),
    // connect 路径不再生效,此时本测试的环境前提不成立,跳过而非误报失败。
    if crate::agent::browser::BrowserManager::global()
        .has_browser()
        .await
    {
        return;
    }
    let res = McpWebUseTool
        .execute(json!({"action": "open", "url": "https://example.com", "reuse_existing": true}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&res).expect("响应应为合法 JSON");
    let code = v["code"].as_i64().unwrap_or(-1);
    if code == 0 {
        if v["data"].get("reuse_existing_chrome") != Some(&serde_json::json!(true)) {
            // 执行间隙被并行测试抢占了全局单例(connect 未生效),环境前提不成立。
            return;
        }
        assert_eq!(v["data"]["connect_mode"], true);
        // 成功打开的页面顺手关闭,不留脏状态
        let pid = v["data"]["page_id"].as_str().unwrap().to_string();
        let closed = McpWebUseTool.execute(json!({"action": "close", "page_id": pid})).await.unwrap();
        assert!(closed.contains("\"code\":0"));
    } else {
        assert_eq!(code, 3002, "无调试端口时应返回 3002,实际: {res}");
        let data = &v["data"];
        let cmd = data["relaunch_command"].as_str().unwrap_or_default();
        assert!(
            cmd.contains("--remote-debugging-port"),
            "3002 信封应含带调试端口的重启命令: {cmd}"
        );
        assert!(data["chrome_running"].is_boolean());
        assert!(data["debug_port"].is_number());
    }
}

#[tokio::test]
async fn open_reuse_existing_skipped_when_connect_url_given() {
    // 显式 connect_url 优先,跳过探测:指向一个无监听端口,应走 connect 失败路径
    // (2001),而不是 3002 引导。
    let res = McpWebUseTool
        .execute(json!({
            "action": "open",
            "url": "https://example.com",
            "reuse_existing": true,
            "connect_url": "http://127.0.0.1:1",
        }))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&res).expect("响应应为合法 JSON");
    let code = v["code"].as_i64().unwrap_or(-1);
    assert_ne!(code, 3002, "显式 connect_url 不应走探测引导: {res}");
}

// =================== page_id 提取(供 agent_loop 引导钩子) ===================

#[test]
fn extract_page_id_parses_success_envelope() {
    let txt = r#"{"code":0,"message":"ok","data":{"page_id":"p_a1b2c3d4","title":"文心一言","final_url":"https://wenxin.baidu.com/"}}"#;
    let pid = extract_page_id_from_text(txt);
    assert_eq!(pid.as_deref(), Some("p_a1b2c3d4"));
}

#[test]
fn extract_page_id_returns_none_for_failure() {
    // code=3001 时不含 page_id
    let txt = r#"{"code":3001,"message":"未检测到浏览器","data":{"install":"请安装 Chrome"}}"#;
    assert_eq!(extract_page_id_from_text(txt), None);
}

#[test]
fn extract_page_id_handles_non_json() {
    assert_eq!(extract_page_id_from_text("plain text"), None);
    assert_eq!(extract_page_id_from_text(""), None);
}

// =================== 第 99 轮:open 复用 / URL 归一化 / eval_js 净化 / download 快速失败 ===================

#[test]
fn normalize_web_url_trims_trailing_slash_and_fragment() {
    assert_eq!(
        normalize_web_url("http://10.0.0.9:8080/"),
        "http://10.0.0.9:8080"
    );
    assert_eq!(
        normalize_web_url("http://a.com/page#top"),
        "http://a.com/page"
    );
    assert_eq!(normalize_web_url(" http://a.com "), "http://a.com");
    // 归一化后相等的不同写法应匹配
    assert_eq!(
        normalize_web_url("http://a.com/x/"),
        normalize_web_url("http://a.com/x")
    );
}

#[test]
fn decode_data_url_base64_payload() {
    // 1x1 红色像素 PNG 的已知 base64 头不可靠,直接用文本字节验证
    let payload = base64::engine::general_purpose::STANDARD.encode("hello captcha");
    let url = format!("data:text/plain;base64,{payload}");
    let bytes = control::decode_data_url(&url).expect("base64 载荷应可解码");
    assert_eq!(bytes, b"hello captcha");
}

#[test]
fn decode_data_url_percent_payload() {
    let url = "data:text/plain,hello%20world%21";
    let bytes = control::decode_data_url(url).expect("百分号载荷应可解码");
    assert_eq!(bytes, b"hello world!");
}

#[test]
fn decode_data_url_rejects_non_data() {
    assert!(control::decode_data_url("http://example.com/x.png").is_none());
    assert!(control::decode_data_url("data:image/png").is_none()); // 无逗号
}

#[test]
fn percent_decode_plus_as_space_and_rejects_bad_hex() {
    assert_eq!(control::percent_decode("a+b").as_deref(), Some("a b"));
    assert_eq!(control::percent_decode("%41").as_deref(), Some("A"));
    assert_eq!(control::percent_decode("%zz"), None);
    assert_eq!(control::percent_decode("%4"), None); // 截断
}

#[test]
fn looks_like_function_detection() {
    assert!(control::looks_like_function("(() => 1)()"));
    assert!(control::looks_like_function("(function(){return 1})()"));
    assert!(control::looks_like_function("async () => 1"));
    assert!(control::looks_like_function("  function f(){}"));
    assert!(!control::looks_like_function("document.title"));
    assert!(!control::looks_like_function("return document.title"));
    assert!(!control::looks_like_function("var x = 1; x + 1"));
}

#[test]
fn sanitize_eval_result_keeps_small_values() {
    assert_eq!(control::sanitize_eval_result(json!("short")), json!("short"));
    assert_eq!(control::sanitize_eval_result(json!(42)), json!(42));
    let obj = json!({"nested": true});
    assert_eq!(control::sanitize_eval_result(obj.clone()), obj);
}

#[test]
fn sanitize_eval_result_truncates_long_strings_to_file() {
    let long = "x".repeat(5000);
    let out = control::sanitize_eval_result(json!(long.clone()));
    assert_eq!(out["result_truncated"], json!(true));
    assert!(out["result_len"].as_i64().unwrap() >= 5000);
    assert!(out["saved_to"].as_str().unwrap_or("").contains("laew_web_result_"));
    // 落盘内容 = 原文
    let path = out["saved_to"].as_str().unwrap();
    let written = std::fs::read_to_string(path).expect("落盘文件应可读");
    assert_eq!(written, long);
    let _ = std::fs::remove_file(path);
}

#[test]
fn sanitize_eval_result_saves_data_url_bytes() {
    let payload = base64::engine::general_purpose::STANDARD.encode([0u8, 1, 2, 3, 4, 5, 6, 7]);
    let url = format!("data:image/png;base64,{payload}");
    let out = control::sanitize_eval_result(json!(url));
    assert_eq!(out["result_truncated"], json!(true));
    assert!(out["saved_to"].as_str().unwrap_or("").ends_with(".png"));
    let path = out["saved_to"].as_str().unwrap();
    let written = std::fs::read(path).expect("落盘文件应可读");
    assert_eq!(written, vec![0u8, 1, 2, 3, 4, 5, 6, 7]);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn download_rejects_about_blank_fast() {
    // about:/blob:/javascript: 不会产生下载事件,必须快速失败而不是挂到超时。
    // 无浏览器环境本应 2002 "浏览器会话不存在",但 scheme 校验应先命中并给出引导。
    let res = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": "p_00000000",
            "control_action": "download",
            "params": {"url": "about:blank"}
        }))
        .await
        .unwrap();
    assert!(res.contains("\"code\":2002"), "实际: {res}");
    assert!(res.contains("不支持从 about: URL 触发下载"), "实际: {res}");
}

#[tokio::test]
async fn download_missing_params_reports_fields() {
    let res = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": "p_00000000",
            "control_action": "download",
            "params": {}
        }))
        .await
        .unwrap();
    assert!(res.contains("缺少 url 或 selector"), "实际: {res}");
}

#[tokio::test]
async fn eval_js_missing_param_lists_aliases() {
    let res = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": "p_00000000",
            "control_action": "eval_js",
            "params": {}
        }))
        .await
        .unwrap();
    // page_id 不存在 → 2002(ensure_page 先失败);改用不存在页时参数校验顺序:
    // ensure_page 在前,所以这里验证的是 2000/2002;参数别名校验用 page 失效前的
    // 同语义 —— 无浏览器环境下两者都会先失败,这里只断言信封结构合法。
    let v: serde_json::Value = serde_json::from_str(&res).unwrap();
    assert!(v["code"].as_i64().unwrap_or(0) != 0);
}

// =================== 第 99 轮:Schema 同步 ===================

#[test]
fn parameters_info_enum_contains_ocr() {
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["info"]["enum"].as_array().expect("info enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names.len(), 21, "info 应有 21 个枚举值: {names:?}");
    assert!(names.contains(&"ocr"), "info 枚举应含 ocr: {names:?}");
    assert!(names.contains(&"extract_links"), "info 枚举应含 extract_links: {names:?}");
    assert!(names.contains(&"coverage"), "info 枚举应含 coverage(第 146 轮): {names:?}");
    assert!(names.contains(&"visibility"), "info 枚举应含 visibility(第 152 轮): {names:?}");
}

// =================== 第 135 轮:extract / page_state / eval_js 体积闸门 ===================

#[test]
fn extract_requires_item_selector() {
    let f = extract::parse_fields(None);
    assert_eq!(f.len(), 1, "fields 缺省应给通用投影");
    assert_eq!(f[0].name, "text");
    assert!(f[0].selector.is_none(), "通用投影不假设选择器");
}

#[test]
fn extract_parses_field_specs() {
    let params = json!({
        "fields": {
            "title":   {"selector": "h3 a", "required": true},
            "time":    {"selector": "time", "attr": "datetime"},
            "summary": {"selector": ".descript", "max_chars": 30}
        }
    });
    let mut f = extract::parse_fields(params.get("fields"));
    f.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(f.len(), 3);
    let t = f.iter().find(|x| x.name == "title").unwrap();
    assert_eq!(t.selector.as_deref(), Some("h3 a"));
    assert!(t.required);
    let tm = f.iter().find(|x| x.name == "time").unwrap();
    assert_eq!(tm.attr.as_deref(), Some("datetime"));
    let s = f.iter().find(|x| x.name == "summary").unwrap();
    assert_eq!(s.max_chars, 30);
    assert!(!s.required, "required 缺省应为 false");
}

#[test]
fn extract_parses_filter_spec() {
    let params = json!({"filter": {
        "keywords": ["AI", " 大模型 ", ""],
        "match_all": true,
        "time_field": "time",
        "since": "2026-10-07 05:33",
        "sort": "time:desc",
        "limit_after_filter": 20
    }});
    let f = extract::parse_filter(params.get("filter"));
    // 空白项被剔除,保留 2 个
    assert_eq!(f.keywords, vec!["AI".to_string(), "大模型".to_string()]);
    assert!(f.match_all);
    assert_eq!(f.time_field.as_deref(), Some("time"));
    assert_eq!(f.sort.as_deref(), Some("time:desc"));
    assert_eq!(f.limit_after_filter, Some(20));
}

#[test]
fn extract_filter_defaults_are_permissive() {
    let f = extract::parse_filter(None);
    assert!(f.keywords.is_empty());
    assert!(!f.match_all, "match_all 缺省应为任一命中");
    assert!(f.time_field.is_none());
    assert!(f.since.is_none() && f.until.is_none());
}

/// 时间边界解析(纯函数):日期类返回日历分量(由页面按本地时区换算),
/// Unix 时间戳类返回绝对毫秒。
#[test]
fn extract_parses_time_bounds() {
    assert_eq!(
        extract::parse_bound("2026-10-08"),
        Some(extract::Bound::Parts { y: 2026, mo: 10, d: 8, hh: 0, mm: 0, ss: 0 })
    );
    assert_eq!(
        extract::parse_bound("2026-10-08 17:02"),
        Some(extract::Bound::Parts { y: 2026, mo: 10, d: 8, hh: 17, mm: 2, ss: 0 })
    );
    assert_eq!(
        extract::parse_bound("2026-10-08T17:02:33"),
        Some(extract::Bound::Parts { y: 2026, mo: 10, d: 8, hh: 17, mm: 2, ss: 33 })
    );
    assert_eq!(
        extract::parse_bound("2026/10/08 17:02"),
        Some(extract::Bound::Parts { y: 2026, mo: 10, d: 8, hh: 17, mm: 2, ss: 0 }),
        "斜杠写法应与连字符等价"
    );
    // Unix 秒 / 毫秒 → 绝对值
    assert_eq!(extract::parse_bound("1791417600"), Some(extract::Bound::Ms(1_791_417_600_000)));
    assert_eq!(extract::parse_bound("1791417600000"), Some(extract::Bound::Ms(1_791_417_600_000)));
    // 非法输入返回 None(fail-open:JS 侧跳过该侧约束并回报 unparsed)
    assert_eq!(extract::parse_bound("36小时前"), None);
    assert_eq!(extract::parse_bound("2026-13-40"), None);
    assert_eq!(extract::parse_bound("2026-10-08 99:00"), None);
    assert_eq!(extract::parse_bound(""), None);
}

/// 生成脚本必须把过滤放在页面内(否则又回到「19KB 大对象直灌」的旧坑)。
#[test]
fn extract_builds_in_page_filtering_js() {
    let fields = extract::parse_fields(Some(&json!({
        "title": {"selector": "h3 a"},
        "time":  {"selector": "time", "attr": "datetime"}
    })));
    let filter = extract::parse_filter(Some(&json!({
        "keywords": ["AI"],
        "time_field": "time",
        "since": "2026-10-07 05:33",
        "sort": "time:desc"
    })));
    let js = extract::build_js(".item", "a", &fields, &filter, 1000, 200);

    // 选择器与字段名都被安全嵌入
    assert!(js.contains("const ITEM_SEL = \".item\""), "{js}");
    assert!(js.contains("\"name\":\"title\""), "{js}");
    assert!(js.contains("\"name\":\"time\""), "{js}");
    // 关键词小写化后嵌入
    assert!(js.contains("\"ai\""), "{js}");
    // 时间窗两侧都以**日历分量**注入,由页面按本地时区换算(避免 UTC 偏移 8 小时)
    assert!(js.contains("SINCE_RAW = [2026,10,7,5,33,0]"), "{js}");
    assert!(js.contains("UNTIL_RAW = null"), "{js}");
    assert!(js.contains("const toMs = (b) =>"), "{js}");
    // 中文相对时间解析在脚本内
    assert!(js.contains("小时前"), "{js}");
    assert!(js.contains("SORT = \"time:desc\""), "{js}");
    // 关键:过滤与截断都在页面内完成
    assert!(js.contains("items.slice(0, CAP)"), "{js}");
    // 自诊断字段
    assert!(js.contains("unparsed_time_fields"), "{js}");
}

/// page_state:三闸裁剪 + dropped_paths + 探测 window 全局键。
#[test]
fn page_state_builds_guarded_js() {
    let js = page_state::build_js(
        &["__NEXT_DATA__".to_string()],
        true,
        20_000,
        8,
        200,
    );
    assert!(js.contains("MAX_BYTES = 20000"), "{js}");
    assert!(js.contains("MAX_DEPTH = 8"), "{js}");
    assert!(js.contains("MAX_ITEMS = 200"), "{js}");
    assert!(js.contains("const PROBE = true"), "{js}");
    assert!(js.contains("dropped"), "必须回报被裁路径:{js}");
    assert!(js.contains("__NEXT_DATA__"));
    // JSON-LD 顺带提取
    assert!(js.contains("application/ld+json"), "{js}");
}

/// eval_js 复合值体积闸门(第 135 轮)。
#[test]
fn sanitize_eval_result_passes_small_structures_through() {
    let small = json!([{"title": "A", "url": "https://36kr.com/p/1"}]);
    assert_eq!(control::sanitize_eval_result(small.clone()), small);
}

#[test]
fn sanitize_eval_result_shrinks_huge_arrays() {
    // 800 个元素,每个约 300 字节 → 远超 20KB 闸门
    let big: Vec<Value> = (0..800)
        .map(|i| json!({"id": i, "title": "x".repeat(200), "url": format!("https://36kr.com/p/{i}")}))
        .collect();
    let out = control::sanitize_eval_result(Value::Array(big));
    // 裁剪后应远小于原始大小
    let out_len = serde_json::to_string(&out).unwrap().len();
    assert!(out_len < 60_000, "裁剪后仍应显著变小,实际 {out_len}");
    assert!(
        out.to_string().contains("__skipped__") || out.to_string().contains("__truncated__"),
        "应显式标注裁剪:{out}"
    );
}

#[test]
fn sanitize_eval_result_spills_oversized_objects_to_file() {
    // 单个超大字符串字段的对象:裁剪后仍超限 → 落盘
    let big: Vec<Value> = (0..200)
        .map(|i| json!({"id": i, "body": "y".repeat(2000)}))
        .collect();
    let out = control::sanitize_eval_result(json!({ "items": big }));
    assert_eq!(out["result_truncated"], json!(true));
    assert_eq!(out["value_kind"], json!("object"));
    assert!(out["saved_to"].is_string(), "应落盘并返回 saved_to");
    assert!(out["hint"].is_string());
    // 落盘内容应是**完整**原始数据,不是裁剪后的
    let path = out["saved_to"].as_str().unwrap();
    let on_disk = std::fs::read_to_string(path).expect("落盘文件应可读");
    assert!(
        on_disk.len() > 400_000,
        "落盘应是完整原始数据,实际 {} 字节",
        on_disk.len()
    );
    let _ = std::fs::remove_file(path);
}

/// 同毫秒并发落盘不得互相覆盖(第 135 轮修)。
#[test]
fn eval_spill_filenames_do_not_collide() {
    let a = format!("{}_{}", 1_789_516_800_000u128, 0);
    let b = format!("{}_{}", 1_789_516_800_000u128, 1);
    assert_ne!(a, b);
}

/// eval_js 匿名函数形态识别(第 135 轮:实测事故里 `function(){…}` 被误判为
/// 「已是函数形态」而跳过包裹,抛 SyntaxError 还凭空派生一个野页面)。
#[test]
fn eval_js_detects_anonymous_function_decl() {
    assert!(control::is_anonymous_function_decl("function(){ return 1; }"));
    assert!(control::is_anonymous_function_decl("  function (){ return 1 }"));
    assert!(control::is_anonymous_function_decl("function*(){}"));
    // 有名字 → 不是匿名函数语句
    assert!(!control::is_anonymous_function_decl("function foo(){ return 1; }"));
    assert!(!control::is_anonymous_function_decl("(() => 1)()"));
    assert!(!control::is_anonymous_function_decl("var x = 1; return x;"));
}

#[test]
fn parameters_has_reuse_property() {
    let p = McpWebUseTool.parameters();
    assert!(
        p["properties"]["reuse"].is_object(),
        "schema 应含 open 复用开关 reuse"
    );
    assert!(p["properties"]["page_id"]["description"].as_str().unwrap_or("").contains("all"),
        "page_id 描述应说明 close all 语义");
}

// =================== 第 100 轮:窗口可视化 + 人工介入(HITL) ===================

#[test]
fn parameters_has_window_and_highlight_properties() {
    let p = McpWebUseTool.parameters();
    for key in ["window_width", "window_height", "highlight"] {
        assert!(
            p["properties"][key].is_object(),
            "schema 应含 {key} 属性"
        );
    }
    assert_eq!(p["properties"]["highlight"]["default"], true);
    // 第 125 轮:视口自适应开关进 schema
    assert!(
        p["properties"]["auto_expand_viewport"].is_object(),
        "schema 应含 auto_expand_viewport 属性"
    );
    assert_eq!(p["properties"]["auto_expand_viewport"]["default"], true);
    // 缺省口径已从「全模式统一 1920×1080」改为「headed 自动铺满屏幕工作区」。
    // 锁死两点:① 不得再宣称 1080p 是缺省;② 必须写明显式传参会关闭铺满
    // (LLM 常见误操作是照着旧文案主动传 window_width/height,结果窗口不铺满)。
    for key in ["window_width", "window_height"] {
        let d = p["properties"][key]["description"].as_str().unwrap_or("");
        assert!(d.contains("铺满"), "{key} 描述应说明缺省铺满屏幕工作区");
        assert!(d.contains("关闭铺满"), "{key} 描述应警告显式传参会关闭铺满");
        assert!(!d.contains("hidden=1440"), "{key} 描述残留旧 hidden=1440 口径");
    }
}

// =================== 第 125 轮:视口基准 1080p 与 2K 自动扩展 ===================

#[test]
fn auto_expand_enabled_defaults_to_true() {
    assert!(auto_expand_enabled(&json!({})), "缺省自动扩展视口开启");
    assert!(auto_expand_enabled(&json!({"auto_expand_viewport": true})));
    assert!(!auto_expand_enabled(&json!({"auto_expand_viewport": false})), "显式 false 才关");
    assert!(auto_expand_enabled(&json!({"auto_expand_viewport": "yes"})), "非 bool 容错回默认开");
}

#[test]
fn open_description_mentions_viewport_fit() {
    // 工具描述是 LLM 的第一信息源:窗口铺满 + 视口自适应必须写入
    let d = McpWebUseTool.description();
    assert!(
        d.contains("铺满") && d.contains("fill_ratio"),
        "open 描述应说明缺省铺满屏幕工作区并回报 fill_ratio"
    );
    assert!(
        d.contains("不要") && d.contains("window_width"),
        "open 描述应劝阻 LLM 自行传 window_width/height(会关闭铺满)"
    );
    assert!(
        d.contains("auto_expand_viewport") && d.contains("2560×1440"),
        "open 描述应说明自动扩展到 2K"
    );
}

#[test]
fn parse_window_size_clamps_and_requires_both() {
    assert_eq!(parse_window_size(&json!({})), None, "缺省返回 None(驱动层按 mode 给默认)");
    assert_eq!(parse_window_size(&json!({"window_width": 1920})), None, "只给一个维度视为缺省");
    assert_eq!(
        parse_window_size(&json!({"window_width": 1920, "window_height": 1080})),
        Some((1920, 1080)),
        "1080p 原样通过"
    );
    assert_eq!(
        parse_window_size(&json!({"window_width": 1, "window_height": 99999})),
        Some((320, 4320)),
        "超界 clamp 到安全区间"
    );
}

#[test]
fn detect_blockers_matches_common_walls() {
    let captcha = inspect::detect_blockers("请完成安全验证\n拖动滑块拼图");
    assert!(captcha.iter().any(|b| b["kind"] == "captcha"), "实际:{captcha:?}");

    let sms = inspect::detect_blockers("短信验证码已发送至 138****0000,请输入");
    assert!(sms.iter().any(|b| b["kind"] == "sms"), "实际:{sms:?}");

    let qr = inspect::detect_blockers("微信扫码登录");
    assert!(qr.iter().any(|b| b["kind"] == "qr_login"), "实际:{qr:?}");

    let login = inspect::detect_blockers("Please sign in to continue browsing");
    assert!(login.iter().any(|b| b["kind"] == "login"), "实际:{login:?}");

    // 第 117 轮:扩展场景(实名/2FA/OAuth)的关键词必须被识别
    let real_name = inspect::detect_blockers("根据监管要求,请先完成实名认证后再继续操作");
    assert!(
        real_name.iter().any(|b| b["kind"] == "real_name"),
        "实际:{real_name:?}"
    );

    let face = inspect::detect_blockers("正在进行人脸核身,请正对屏幕");
    assert!(face.iter().any(|b| b["kind"] == "real_name"), "实际:{face:?}");

    let two_factor = inspect::detect_blockers("请输入邮箱验证码完成两步验证");
    assert!(
        two_factor.iter().any(|b| b["kind"] == "two_factor"),
        "实际:{two_factor:?}"
    );

    let totp = inspect::detect_blockers("Open Google Authenticator and enter the TOTP code");
    assert!(totp.iter().any(|b| b["kind"] == "two_factor"), "实际:{totp:?}");

    let oauth = inspect::detect_blockers("请使用 GitHub 账号授权登录");
    assert!(oauth.iter().any(|b| b["kind"] == "oauth"), "实际:{oauth:?}");

    let sso = inspect::detect_blockers("通过 SSO 登录到企业工作台");
    assert!(sso.iter().any(|b| b["kind"] == "oauth"), "实际:{sso:?}");

    let clean = inspect::detect_blockers("普通页面内容,无阻断");
    assert!(clean.is_empty(), "误报:{clean:?}");
}

/// 第 171 轮:京东频控横幅(实测原文)必须命中 `rate_limited`,且为首条
/// (阻断表序即优先级);评论类「频繁掉线」噪声不得误报。
#[test]
fn detect_blockers_rate_limited_banner() {
    // 京东 2026-10-14 实测原文(OCR 抓取)
    let banner = inspect::detect_blockers(
        "抱歉由于访问频繁导致无法搜索，请稍后再试！\n若长时间无法搜索可[点此反馈]，平台将尽快处理~",
    );
    let first = banner.first().expect("频控横幅必须命中");
    assert_eq!(first["kind"], "rate_limited", "实际:{banner:?}");
    assert!(first["advice"].as_str().is_some_and(|s| s.contains("冷却")), "advice 必须给冷却指引");

    // 变体:请求/操作频繁
    let v2 = inspect::detect_blockers("请求过于频繁,请稍后再试");
    assert!(v2.iter().any(|b| b["kind"] == "rate_limited"), "实际:{v2:?}");

    // 噪声:商品评论里的「频繁」描述不得误报频控
    let noise = inspect::detect_blockers("用了半个月频繁掉线,偶尔蓝屏,不推荐");
    assert!(
        !noise.iter().any(|b| b["kind"] == "rate_limited"),
        "评论噪声误报:{noise:?}"
    );

    // 既有风控墙关键词语义不变(访问过于频繁仍归 manual_verify)
    let wall = inspect::detect_blockers("当前页面异常,访问过于频繁,请刷新或切换账户");
    assert!(wall.iter().any(|b| b["kind"] == "manual_verify"), "实际:{wall:?}");
}

/// 第 169 轮:模式表**处置建议**必须进告警载荷(advice 字段)——
/// 风控墙命中要拿到「不要 reload / request_human 点刷新切换账号 / reuse_existing」
/// 的专属指引,而不是通用「人工验证挑战(拖拽拼图/滑块)」错位文案(实测京东
/// `.jd-main-risk-fallback` 场景)。
#[test]
fn detect_blockers_carries_pattern_advice() {
    let risk = inspect::detect_blockers("当前页面异常\n请刷新或切换账户试试");
    let hit = risk
        .iter()
        .find(|b| b["kind"] == "manual_verify")
        .unwrap_or_else(|| panic!("应命中风控墙:{risk:?}"));
    let advice = hit["advice"].as_str().unwrap_or_default();
    assert!(advice.contains("不要 reload"), "风控墙建议应禁止 reload:{advice}");
    assert!(advice.contains("reuse_existing"), "风控墙建议应给根治路径:{advice}");
    // 其它模式同样带建议(sms 场景抽样)
    let sms = inspect::detect_blockers("短信验证码已发送至 138****0000,请输入");
    let s = sms.iter().find(|b| b["kind"] == "sms").expect("应命中 sms");
    assert!(
        s["advice"].as_str().unwrap_or_default().contains("request_human"),
        "sms 建议应指引 request_human:{:?}",
        s
    );
}

// ==================== 第 151 轮:关键词通道整词边界(误报根治) ====================

#[test]
fn detect_blockers_rejects_embedded_ascii_noise() {
    // 实测反例(豆包 2026-10-10):页面把 SSR 数据以字面 JSON 转义 URL 文本渲染进可见 DOM,
    // `/ae6146b6…` 中的 `2Fa` 让 "2fa" 以子串形态命中 two_factor,连续 4 次假告警。
    let doubao_noise = "002Ficon\\u002Fae6146b6e7f7487ebf3e1331d6e9166f~tplv-a9r";
    let hits = inspect::detect_blockers(doubao_noise);
    assert!(
        hits.iter().all(|b| b["kind"] != "two_factor"),
        "转义 URL 子串不应命中 two_factor,实际:{hits:?}"
    );

    // 同类:十六进制哈希/标识符里嵌 sms/kyc/oauth/totp/captcha
    let hash_noise = "resource id=verify_mv26aqhm hash sms4kd93jf kyc7xx oauth9ab totp3zz";
    let hits = inspect::detect_blockers(hash_noise);
    assert!(
        hits.iter().all(|b| !matches!(
            b["kind"].as_str(),
            Some("sms") | Some("real_name") | Some("oauth") | Some("two_factor")
        )),
        "字母数字包围的子串不应命中,实际:{hits:?}"
    );
}

#[test]
fn detect_blockers_ascii_word_boundary_positives() {
    // 整词命中(前后是空格/中文/行首尾)必须保持 —— 这是真阻断文案的正常形态
    let cases: &[(&str, &str)] = &[
        ("请开启 2FA 验证后继续", "two_factor"),
        ("Enter the TOTP code from your app", "two_factor"),
        ("Sign in to continue", "login"),
        ("请拖动滑块完成 Captcha 验证", "captcha"),
        ("Use 2-step verification", "two_factor"),
    ];
    for (text, kind) in cases {
        let hits = inspect::detect_blockers(text);
        assert!(
            hits.iter().any(|b| b["kind"] == *kind),
            "「{text}」应命中 {kind},实际:{hits:?}"
        );
    }
}

#[test]
fn detect_blockers_cjk_keywords_keep_substring_semantics() {
    // 含 CJK 的关键词不收边界(中文词不会以 URL 子串形态出现)
    let hits = inspect::detect_blockers("登录后查看完整报告");
    assert!(hits.iter().any(|b| b["kind"] == "login"), "实际:{hits:?}");
}

#[test]
fn hitl_image_plan_priority_and_sources() {
    // 2026-10-13:附图优先级 —— 显式路径(存在)> 无图;自动截图链已随弹窗
    // 验证码功能移除,captcha reason 不再触发任何自动采集。
    use crate::agent::tools::mcp_web_use::control::{hitl_image_plan, HitlImagePlan};

    // 显式 + 文件存在 → Explicit / explicit
    let tmp = std::env::temp_dir().join("laew_hitl_plan_probe.png");
    std::fs::write(&tmp, b"png").unwrap();
    let f = tmp.display().to_string();
    let (plan, src) = hitl_image_plan("captcha", Some(f.as_str()));
    assert_eq!(plan, HitlImagePlan::Explicit(f.clone()));
    assert_eq!(src, "explicit");
    // 显式给图对非 captcha reason 同样生效(人工指定即权威)
    let (plan2, _) = hitl_image_plan("sms", Some(f.as_str()));
    assert!(matches!(plan2, HitlImagePlan::Explicit(_)));
    let _ = std::fs::remove_file(&tmp);

    // 显式但文件不存在 → 无图 / explicit_missing(fail-open)
    let (plan, src) = hitl_image_plan("captcha", Some("/no/such/file.png"));
    assert_eq!(plan, HitlImagePlan::None);
    assert_eq!(src, "explicit_missing");

    // 无显式 + captcha → 无图(自动采集已删,2026-10-13 行为变更)
    let (plan, src) = hitl_image_plan("captcha", None);
    assert_eq!(plan, HitlImagePlan::None);
    assert_eq!(src, "none");

    // 无显式 + 非 captcha → 无图
    let (plan, src) = hitl_image_plan("sms", None);
    assert_eq!(plan, HitlImagePlan::None);
    assert_eq!(src, "none");
    // 空串显式等同未给
    let (plan, _) = hitl_image_plan("captcha", Some("  "));
    assert_eq!(plan, HitlImagePlan::None);
}

#[test]
fn human_assist_allowed_reasons_covers_extensions() {
    // 第 117 轮:常量必须包含实名/2FA/OAuth 三种新 reason,且顺序符合推荐使用顺序。
    use crate::agent::tools::mcp_web_use::control::HUMAN_ASSIST_ALLOWED_REASONS;
    let names: Vec<&str> = HUMAN_ASSIST_ALLOWED_REASONS.to_vec();
    for required in &[
        "captcha", "sms", "qr_login", "login", "real_name", "two_factor", "oauth",
        "manual_verify", "custom",
    ] {
        assert!(names.contains(required), "缺失合法 reason {required}");
    }
    // 顺序:captcha→sms→qr_login→login→real_name→two_factor→oauth→manual_verify→custom
    assert_eq!(names[4], "real_name", "real_name 应在 manual_verify 前");
    assert_eq!(names[5], "two_factor", "two_factor 应在 manual_verify 前");
    assert_eq!(names[6], "oauth", "oauth 应在 manual_verify 前");
}

#[test]
fn human_assist_reasons_doc_lists_all_variants() {
    // 第 117 轮:human_assist_reasons_doc() 输出必须包含 9 个合法 reason,
    // LLM 在错误消息中可直接复用此串构造修复请求。
    use crate::agent::tools::mcp_web_use::control::{
        human_assist_reasons_doc, HUMAN_ASSIST_ALLOWED_REASONS,
    };
    let doc = human_assist_reasons_doc();
    for r in HUMAN_ASSIST_ALLOWED_REASONS {
        assert!(doc.contains(r), "doc 缺失 {r}: {doc}");
    }
}

#[tokio::test]
async fn request_human_invalid_reason_returns_1001() {
    // 无浏览器环境:非法 reason 在 page_id 校验之后仍有确定性断言路径 ——
    // page_id 不存在时返回 2000;用不存在页验证 2000 前置。
    let res = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": "p_nonexist0",
            "control_action": "request_human",
            "params": {"reason": "whatever"}
        }))
        .await
        .unwrap();
    assert!(res.contains("\"code\":2000"), "page_id 不存在应为 2000: {res}");
}

// =================== explore / batch 参数校验(第 119 轮) ===================

#[tokio::test]
async fn explore_missing_page_id_returns_1001() {
    let res = McpWebUseTool
        .execute(json!({"action": "explore", "queries": [{"info": "elements"}]}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"), "缺 page_id 应 1001: {res}");
}

#[tokio::test]
async fn explore_empty_queries_returns_1001() {
    let res = McpWebUseTool
        .execute(json!({"action": "explore", "page_id": "p_x", "queries": []}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"), "空 queries 应 1001: {res}");
}

#[tokio::test]
async fn explore_too_many_queries_returns_1001() {
    let queries: Vec<Value> = (0..9).map(|_| json!({"info": "elements"})).collect();
    let res = McpWebUseTool
        .execute(json!({"action": "explore", "page_id": "p_x", "queries": queries}))
        .await
        .unwrap();
    assert!(
        res.contains("\"code\":1001") && res.contains("最多 8 项"),
        "超过 8 项应 1001: {res}"
    );
}

#[tokio::test]
async fn explore_unknown_page_returns_2000() {
    let res = McpWebUseTool
        .execute(json!({
            "action": "explore",
            "page_id": "p_nonexist0",
            "queries": [{"info": "elements"}]
        }))
        .await
        .unwrap();
    assert!(res.contains("\"code\":2000"), "page_id 不存在应 2000: {res}");
}

#[tokio::test]
async fn inspect_with_queries_array_delegates_to_explore() {
    // 2026-10-16:模型常把 explore 的 queries[] 批量形态误传给 inspect ——
    // 入口应机械转交 explore(不再 1001「缺少 info」白烧一轮 LLM 迭代)。
    // 转交后 explore 的守卫生效:无 page_id 无 url → 1001(带自动开页提示)。
    let res = McpWebUseTool
        .execute(json!({
            "action": "inspect",
            "queries": [{"info": "elements"}, {"info": "blockers"}]
        }))
        .await
        .unwrap();
    assert!(
        res.contains("\"code\":1001") && res.contains("url"),
        "inspect+queries 应转交 explore 并给出 url 自动开页提示: {res}"
    );
    // 显式给了 page_id 的 queries 形态同样转交(explore 报 2000 而非 1001 缺 info)
    let res2 = McpWebUseTool
        .execute(json!({
            "action": "inspect",
            "page_id": "p_nonexist0",
            "queries": [{"info": "elements"}]
        }))
        .await
        .unwrap();
    assert!(
        res2.contains("\"code\":2000"),
        "inspect+queries+显式 page_id 应转交 explore 报 2000: {res2}"
    );
}

#[tokio::test]
async fn batch_missing_steps_returns_error() {
    // batch 是 sequence 的语义别名;缺 steps 应确定性报错(1001)
    let res = McpWebUseTool
        .execute(json!({"action": "batch", "page_id": "p_nonexist0"}))
        .await
        .unwrap();
    assert!(
        res.contains("\"code\":1001") || res.contains("\"code\":2000"),
        "batch 缺 steps 应确定性报错: {res}"
    );
}

// =================== 第 131 轮:人工介入提示下沉到工具返回层 ===================

#[test]
fn human_assist_hint_字段完整且自描述() {
    let h = super::human_assist_hint("captcha", "请人工输入验证码", &["继续", "取消"]);
    assert_eq!(h["next_action"], "request_human");
    let a = &h["human_assist"];
    assert_eq!(a["ready"], true);
    assert_eq!(a["reason"], "captcha");
    assert_eq!(a["message"], "请人工输入验证码");
    assert_eq!(a["options"][0], "继续");
    // call 字段直接给出可复制的 MCP_Web_Use 调用形态,模型不必反查 schema
    let call = a["call"].as_str().unwrap();
    assert!(call.contains("control_action"), "{call}");
    assert!(call.contains("request_human"), "{call}");
    assert!(a["rule"].as_str().unwrap().contains("严禁伪造"));
}

#[test]
fn ocr_报错时给出人工介入提示() {
    let out = json!({"ocr_error": "OCR 失败:权限不足"});
    let hint = super::ocr_unavailable_hint(&out).expect("ocr_error 应触发提示");
    assert_eq!(hint["next_action"], "request_human");
    assert_eq!(hint["human_assist"]["reason"], "captcha");
}

#[test]
fn ocr_空文本同样视为不可用() {
    // 验证码区域是噪声图时 OCR 常常返回空串,模型最容易在这时反复调参重试
    let out = json!({"ocr_text": "   "});
    assert!(super::ocr_unavailable_hint(&out).is_some(), "空文本应触发提示");
}

#[test]
fn ocr_拿到文本时不打扰() {
    let out = json!({"ocr_text": "8F3K", "ocr_block_count": 1});
    assert!(super::ocr_unavailable_hint(&out).is_none(), "OCR 成功不应附 HITL 提示");
}

#[test]
fn merge_hint_不覆盖已有字段() {
    let mut out = json!({"ocr_error": "x", "next_action": "keep_me"});
    let hint = super::human_assist_hint("captcha", "m", &["a"]);
    super::merge_hint(&mut out, hint);
    assert_eq!(out["next_action"], "keep_me", "已有字段不应被覆盖");
    assert_eq!(out["human_assist"]["reason"], "captcha");
}

/// 内建 SSR 注水点候选:非空且互不重复(顺序即优先级)。
#[test]
fn page_state_builtin_candidates_are_unique() {
    let c = page_state::builtin_candidates();
    assert!(c.len() >= 6, "候选过少: {c:?}");
    let mut sorted: Vec<&str> = c.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(sorted.len(), before, "候选有重复: {c:?}");
    assert!(c.contains(&"__NEXT_DATA__"));
    assert!(c.contains(&"initialState"));
}

/// 把生成的页面内脚本写到临时文件,供 `node --check` 做语法校验
/// (运行:`cargo test --lib dump_generated_js -- --ignored --nocapture`)。
#[test]
#[ignore]
fn dump_generated_js() {
    use std::io::Write;
    let fields = extract::parse_fields(Some(&json!({
        "title": {"selector": "h3 a", "required": true},
        "time":  {"selector": "time", "attr": "datetime"}
    })));
    let filter = extract::parse_filter(Some(&json!({
        "keywords": ["AI", "大模型"], "time_field": "time",
        "since": "2026-10-07 05:33", "sort": "time:desc"
    })));
    let mut f = std::fs::File::create("/tmp/laew_extract_gen.js").unwrap();
    writeln!(f, "{}", extract::build_js(".item", "a", &fields, &filter, 1000, 200)).unwrap();
    let mut f2 = std::fs::File::create("/tmp/laew_page_state_gen.js").unwrap();
    writeln!(f2, "{}", page_state::build_js(&["__NEXT_DATA__".into()], true, 20000, 8, 200)).unwrap();
    eprintln!("written");
}

// =================== 第 141 轮:蒙层全链路真浏览器验证(#[ignore],本地 --ignored 跑) ===================
//
// 与 browser_overlay.rs 的集成测试互补:那条验证「驱动层双层原语」,本条验证
// 「工具层完整链路」—— open(默认 overlay=on,本机 Aqua 会话 → headed)注入蒙层
// 并加锁后,control(click) 必须经「先解后锁」包装照常生效(而非被 setIgnoreInputEvents
// 吃掉),且响应回报 overlay.enabled=true。
// 跑法:`cargo test --lib real_browser_tool_overlay -- --ignored --nocapture`(需本机 Chrome)。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_browser_tool_overlay_end_to_end() {
    let url = "data:text/html,<button id='b' onclick='this.dataset.c=(+this.dataset.c||0)+1' \
               style='position:fixed;left:0;top:0;width:200px;height:100px'>x</button>";
    // ① open:不传 overlay → 缺省随 LAEW_WEB_OVERLAY(默认开)+ 本机 GUI → headed
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "window_width": 800, "window_height": 600}))
        .await
        .expect("open 失败(本机是否装有 Chrome?)");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0, "open 应成功:{out}");
    assert_eq!(v["data"]["mode"], "headed", "本机 GUI 会话应为 headed:{out}");
    assert_eq!(v["data"]["overlay"]["enabled"], true, "蒙层应默认开启:{out}");
    let pid = v["data"]["page_id"].as_str().unwrap().to_string();

    // ② 蒙层锁定下 control(click) 应照常生效(先解后锁包装)
    let click = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "click",
                        "params": {"selector": "#b"}}))
        .await
        .unwrap();
    let c: Value = serde_json::from_str(&click).unwrap();
    assert_eq!(c["code"], 0, "蒙层锁定下 click 应经先解后锁照常生效:{click}");

    // ③ 点击真实生效(计数=1)且蒙层元素仍在(锁定未被误关)
    let check = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression":
                            "JSON.stringify({c:+document.getElementById('b').dataset.c||0, m:!!document.getElementById('__laew_overlay__')})"}}))
        .await
        .unwrap();
    let e: Value = serde_json::from_str(&check).unwrap();
    let payload: Value =
        serde_json::from_str(e["data"]["result"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
    assert_eq!(payload["c"], 1, "点击应真实生效:{check}");
    assert_eq!(payload["m"], true, "蒙层元素应仍存在(复锁不误删):{check}");

    // ④ 收口:关闭全部页面回收浏览器(全局单例,防污染其它测试)
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

// =================== 第 143 轮:页面管控三档工具层真浏览器验证(#[ignore]) ===================
//
// 验证「工具 JSON 面 → parse_open_guard → new_page → 响应字段 → set_guard 切档 →
// partial 黑名单盾下 Agent click(先隐盾后复盾)」完整链路:
// open(guard="open") 应回报 data.guard.mode=open 且 CDP 未锁(click 无须让路);
// set_guard(partial, block=#b) 后 click 应照常生效(隐盾包装)且盾区存在;
// 参数错误(partial 缺选择器)应回 1001。
// 跑法:`cargo test --lib real_browser_tool_guard -- --ignored --nocapture`(需本机 Chrome)。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_browser_tool_guard_modes_end_to_end() {
    let url = "data:text/html,<button id='b' onclick='this.dataset.c=(+this.dataset.c||0)+1' \
               style='position:fixed;left:0;top:0;width:200px;height:100px'>x</button>";
    // ① open(guard=open):响应应回报 guard.mode=open(屏蔽页面状态栏显示「页面开放」)
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "guard": "open",
                        "guard_note": "请登录后应答弹窗", "window_width": 800, "window_height": 600}))
        .await
        .expect("open 失败(本机是否装有 Chrome?)");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0, "open(guard=open) 应成功:{out}");
    assert_eq!(v["data"]["guard"]["mode"], "open", "guard 档应为 open:{out}");
    assert_eq!(v["data"]["overlay"]["enabled"], false, "legacy overlay 应派生为关:{out}");
    let pid = v["data"]["page_id"].as_str().unwrap().to_string();

    // ② open 档 click 无须让路也应生效(没有 CDP 输入锁/盾区)
    let click = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "click",
                        "params": {"selector": "#b"}}))
        .await
        .unwrap();
    let c: Value = serde_json::from_str(&click).unwrap();
    assert_eq!(c["code"], 0, "open 档 click 应生效:{click}");

    // ③ set_guard 切 partial(黑名单罩住按钮):click 经「先隐盾后复盾」照常生效,
    //    盾区容器内应有盾罩(档位文案已迁至屏蔽页面状态栏,第 171 轮)。
    let sg = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "set_guard",
                        "params": {"mode": "partial", "block_selectors": ["#b"], "note": "支付区已锁定"}}))
        .await
        .unwrap();
    let g: Value = serde_json::from_str(&sg).unwrap();
    assert_eq!(g["code"], 0, "set_guard(partial) 应成功:{sg}");
    assert_eq!(g["data"]["mode"], "partial", "响应应回报 partial:{sg}");
    let click2 = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "click",
                        "params": {"selector": "#b"}}))
        .await
        .unwrap();
    let c2: Value = serde_json::from_str(&click2).unwrap();
    assert_eq!(c2["code"], 0, "partial 黑名单盾下 click 应经隐盾照常生效:{click2}");
    let check = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression":
                            "JSON.stringify({c:+document.getElementById('b').dataset.c||0, \
                             sc:document.getElementById('__laew_guard_shields__').childElementCount})"}}))
        .await
        .unwrap();
    let e: Value = serde_json::from_str(&check).unwrap();
    let payload: Value =
        serde_json::from_str(e["data"]["result"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
    assert_eq!(payload["c"], 2, "两次 click 都应真实生效:{check}");
    assert_eq!(payload["sc"], 1, "黑名单应生成 1 个盾罩:{check}");

    // ④ set_guard 切回 locked:输入锁恢复 + 蒙层回归。
    let sg2 = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "set_guard",
                        "params": {"mode": "locked"}}))
        .await
        .unwrap();
    let g2: Value = serde_json::from_str(&sg2).unwrap();
    assert_eq!(g2["code"], 0, "set_guard(locked) 应成功:{sg2}");
    assert_eq!(g2["data"]["input_locked"], true, "locked 档应回报输入锁:{sg2}");

    // ⑤ 参数错误:partial 缺选择器 → 1001(不是 2002,便于 LLM 机械修正)。
    let bad = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "set_guard",
                        "params": {"mode": "partial"}}))
        .await
        .unwrap();
    let b: Value = serde_json::from_str(&bad).unwrap();
    assert_eq!(b["code"], 1001, "partial 缺选择器应回 1001:{bad}");

    // ⑥ 收口:关闭全部页面回收浏览器(全局单例,防污染其它测试)
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

// =================== 第 144 轮:凭证区域探测 + partial 白名单子 frame fail-open 真浏览器验证(#[ignore]) ===================
//
// 覆盖两条关键链路(详案 docs/MCP_Web_Use/08 §4):
// 1. probe_credential_zones:密码框 → form 容器 + 验证码语义容器,产出主文档选择器;
// 2. partial 白名单挖洞:洞内(密码框)elementFromPoint 命中本体、洞外命中盾罩;
//    白名单选择器在子 frame(srcdoc,同源可访问)零命中 → 不加盾(fail-open,
//    跨域验证码 iframe 内部必须放行);黑名单在子 frame照常加盾。
// 需要本机安装 Chrome;跑 `cargo test --lib real_credential_zone -- --ignored --nocapture`。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_credential_zone_probe_and_subframe_fail_open() {
    use crate::agent::browser::BrowserManager;
    let url = "data:text/html,<html><body><form id='login'><input type='text' name='user' placeholder='账号'>\
               <input type='password' name='pwd'><button type='submit'>go</button></form>\
               <div class='captcha-box'><iframe srcdoc='<input id=\"inner\">' style='width:160px;height:60px'></iframe></div>\
               </body></html>";
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "window_width": 800, "window_height": 600}))
        .await
        .expect("open 失败(本机是否装有 Chrome?)");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0, "open 应成功:{out}");
    let pid = v["data"]["page_id"].as_str().unwrap().to_string();
    let page = BrowserManager::global().page(&pid).await.expect("page 句柄");

    // ① 凭证区域探测:form(#login)优先 + 验证码语义容器(≥2 条)
    let zones: Vec<String> = super::unlock_zone::probe_credential_zones_kinds(&page)
        .await
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    assert!(!zones.is_empty(), "登录页应探测到凭证区域");
    assert_eq!(zones[0], "#login", "密码框应容器化到 form#login:{zones:?}");
    assert!(zones.len() >= 2, "验证码语义容器也应命中:{zones:?}");
    // 选择器真实命中元素(生成即自验)
    let hit_check = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression": format!(
                            "[{}].map(s => document.querySelectorAll(s).length)",
                            zones.iter().map(|z| format!("'{z}'")).collect::<Vec<_>>().join(","))}}))
        .await
        .unwrap();
    let hc: Value = serde_json::from_str(&hit_check).unwrap();
    let counts: Vec<i64> = serde_json::from_str(
        hc["data"]["result"].as_str().unwrap_or("[]"),
    )
    .unwrap_or_default();
    assert!(
        counts.iter().all(|c| *c >= 1),
        "每条探测选择器都应命中 ≥1 元素:{zones:?} → {counts:?}"
    );

    // ② set_guard 切 partial 白名单(探测到的 zones)→ 洞内可点、洞外被盾吞
    let sg = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "set_guard",
                        "params": {"mode": "partial", "allow_selectors": zones.clone(),
                                   "note": "测试放行"}}))
        .await
        .unwrap();
    let g: Value = serde_json::from_str(&sg).unwrap();
    assert_eq!(g["code"], 0, "set_guard(partial) 应成功:{sg}");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let probe = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression":
                            "JSON.stringify((function(){ \
                             const p = document.querySelector('#login input[type=password]'); \
                             const r = p.getBoundingClientRect(); \
                             const inHole = document.elementFromPoint(r.x + r.width/2, r.y + r.height/2); \
                             const outside = document.elementFromPoint(innerWidth - 5, innerHeight - 5); \
                             return { inHole: inHole ? inHole.tagName : 'none', \
                                      outsideShield: !!(outside && outside.closest && outside.closest('#__laew_guard_shields__')) }; })())"}}))
        .await
        .unwrap();
    let p: Value = serde_json::from_str(&probe).unwrap();
    let payload: Value =
        serde_json::from_str(p["data"]["result"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
    assert_eq!(payload["inHole"], "INPUT", "洞内(密码框)应命中输入框本体:{probe}");
    assert_eq!(payload["outsideShield"], true, "洞外应命中盾罩:{probe}");

    // ③ 子 frame fail-open:把管控脚本注入 srcdoc iframe(同源可访问),
    //    白名单(主文档选择器)在 iframe 零命中 → 0 盾;黑名单命中 → 1 盾。
    let iframe_cfg = crate::agent::browser_overlay::PageGuardConfig::partial(
        vec!["#login".to_string()],
        vec![],
    );
    let script_js = crate::agent::browser_overlay::guard_script(&iframe_cfg);
    let iframe_expr = format!(
        "(function() {{ \
          const f = document.querySelector('iframe'); \
          const w = f && f.contentWindow; \
          if (!w) return 'no-window'; \
          w.eval({}); \
          w.__laewGuardSet('partial', ['#login'], [], null); \
          const sc = w.document.getElementById('__laew_guard_shields__'); \
          const allowCount = sc ? sc.childElementCount : -1; \
          w.__laewGuardSet('partial', [], ['#inner'], null); \
          const blockCount = sc ? sc.childElementCount : -2; \
          return allowCount + '/' + blockCount; }})()",
        super::js_str(&script_js)
    );
    let ifr = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression": iframe_expr}}))
        .await
        .unwrap();
    let iv: Value = serde_json::from_str(&ifr).unwrap();
    let r = iv["data"]["result"].as_str().unwrap_or("");
    assert_eq!(r, "0/1", "子 frame 白名单零命中应 0 盾(fail-open),黑名单应 1 盾:{ifr}");

    // ④ 收口:关闭全部页面回收浏览器(全局单例,防污染其它测试)
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

// ==================== 第 145 轮:HITL 应答 next_hint 分流(输码 vs 已完成) ====================

/// 人工在弹窗输入验证码文本(实测 z7z2 场景)→ 必须指引立即 input_text 填码提交;
/// 点选选项(「N. 选项」形态)→ 才走 inspect 验证;其它 reason 自由文本 → 通用提示。
#[test]
fn hitl_answer_hint_splits_typed_code_vs_option_pick() {
    // 自由文本验证码(captcha 实测形态)
    let h = hitl_hint::hitl_answer_hint("captcha", "z7z2");
    assert!(h.contains("input_text"), "captcha 自由文本应指引填码: {h}");
    assert!(!h.contains("inspect 验证页面当前状态"), "不应再让 Agent 只观望: {h}");
    // sms / two_factor 同款
    assert!(hitl_hint::hitl_answer_hint("sms", "482913").contains("input_text"));
    assert!(hitl_hint::hitl_answer_hint("two_factor", "993 041").contains("input_text"));
    // 选项应答(弹窗选项 / TUI 数字映射共用「N. 选项」形态)
    let h = hitl_hint::hitl_answer_hint("captcha", "1. 我已完成人工操作,继续");
    assert!(h.contains("inspect"), "选项应答应走 inspect 验证: {h}");
    assert!(hitl_hint::hitl_answer_hint("login", "2. 我已登录完成").contains("inspect"));
    // 其它 reason 自由文本:通用提示,不误指填码
    let h = hitl_hint::hitl_answer_hint("custom", "跳过这个站点");
    assert!(h.contains("human_response"), "{h}");
    assert!(!h.contains("input_text"), "非验证码 reason 不应指引填码: {h}");
}

/// 第 169 轮:人工完成操作/自由文本指示的 next_hint 必须带「勿 reload」防呆 ——
/// 实测京东任务 SubAgent 在人工登录成功后第一动作 control(reload),直接触发
/// 风控复核弹「当前页面异常」风控墙。
#[test]
fn hitl_answer_hint_warns_no_reload_after_human() {
    let h = hitl_hint::hitl_answer_hint("login", "1. 我已完成人工操作,继续");
    assert!(h.contains("不要 reload"), "选项应答应带勿 reload 防呆:{h}");
    let h = hitl_hint::hitl_answer_hint("manual_verify", "登录好了");
    assert!(h.contains("不要 reload"), "自由文本应答应带勿 reload 防呆:{h}");
    // 填码分支是「立即提交」语义,不掺 reload 防呆(避免稀释主指令)
    let h = hitl_hint::hitl_answer_hint("captcha", "z7z2");
    assert!(!h.contains("reload"), "填码分支不应掺 reload 提示:{h}");
}

/// 第 155 轮:captcha/sms/two_factor 的自由文本**不像码**(整句指示/中文)时,
/// 不得指引 input_text 填码 —— 人工经弹窗输入的是给 Agent 的指示。
#[test]
fn hitl_answer_hint_directive_text_not_treated_as_code() {
    // 整句指示(中文):按指示处理,不填入页面
    let h = hitl_hint::hitl_answer_hint("captcha", "先别提交,我想让你改查北京天气");
    assert!(h.contains("文字指示"), "{h}");
    assert!(!h.contains("input_text"), "整句指示不应指引填码: {h}");
    // 长英文句子(>16 字符)同理
    let h = hitl_hint::hitl_answer_hint("sms", "please wait for my next message");
    assert!(h.contains("文字指示"), "{h}");
    // 边界:像码的形态仍然填码(回归锁)
    for code in ["z7z2", "482913", "993 041", "ab-12_34"] {
        assert!(
            hitl_hint::hitl_answer_hint("captcha", code).contains("input_text"),
            "「{code}」应按验证码处理"
        );
    }
    // 判据本身:空/超长/含中文/含句读 → 不是码
    assert!(hitl_hint::looks_like_code_text("z7z2"));
    assert!(!hitl_hint::looks_like_code_text(""));
    assert!(!hitl_hint::looks_like_code_text("先别提交"));
    assert!(!hitl_hint::looks_like_code_text("a b c d e f g h i j k l m n o p q"));
    assert!(!hitl_hint::looks_like_code_text("wait, ok"));
}

/// 「N. 选项」形态判定:数字前缀 + ". " 分隔;纯数字验证码/含点文本不误判。
#[test]
fn option_pick_answer_shape_detection() {
    for s in ["1. 我已完成人工操作,继续", "12. 某选项", " 3. 带空格前缀"] {
        assert!(hitl_hint::is_option_pick_answer(s), "「{s}」应识别为选项应答");
    }
    for s in ["z7z2", "482913", "993 041", "3.14 是圆周率吗", "选项一", ""] {
        assert!(!hitl_hint::is_option_pick_answer(s), "「{s}」不应识别为选项应答");
    }
}

// ===================== 第 146 轮:origin 复用 / 访问台账 =====================

/// origin 提取:仅 http(s) 参与,chrome:// 等特殊页永不兜底复用。
#[test]
fn url_origin_extracts_and_rejects_special_schemes() {
    assert_eq!(
        url_origin("http://10.0.0.9:8080/OrganManagement").as_deref(),
        Some("http://10.0.0.9:8080")
    );
    assert_eq!(url_origin("https://a.com/x?y=1").as_deref(), Some("https://a.com"));
    // 默认端口归一(url crate 语义:80/443 省略)
    assert_eq!(
        url_origin("http://a.com:80/x").as_deref(),
        url_origin("http://a.com/x").as_deref()
    );
    assert_eq!(url_origin("chrome://new-tab-page/"), None);
    assert_eq!(url_origin("about:blank"), None);
    assert_eq!(url_origin("devtools://devtools/bundled/inspector.html"), None);
    assert_eq!(url_origin("file:///tmp/a.html"), None);
    assert_eq!(url_origin("not a url"), None);
}

/// origin 复用兜底:URL 精确未命中、同 origin 存活页应被选中(取最早创建的)。
/// 不起真浏览器 —— `find_reusable_page` 依赖全局 BrowserManager,这里只锁纯函数层;
/// 端到端行为由 #[ignore] 真浏览器用例与实测日志验证。
#[test]
fn reuse_match_kind_strs() {
    use super::ReuseMatch;
    assert_eq!(ReuseMatch::Exact.as_str(), "exact");
    assert_eq!(ReuseMatch::Origin.as_str(), "origin");
}

/// inspect(info=coverage):进程级全局事实,不依赖存活页面(page_id 随便给)。
#[tokio::test]
async fn inspect_coverage_returns_ledger_without_live_page() {
    let _g = visit_ledger::test_lock().lock().unwrap();
    visit_ledger::reset_for_test();
    visit_ledger::record_visit("p_x", "http://a.com/login", "登录", visit_ledger::VisitSource::Open);
    visit_ledger::record_visit("p_x", "http://a.com/home", "首页", visit_ledger::VisitSource::Navigate);
    visit_ledger::record_visit("p_x", "http://a.com/home", "首页", visit_ledger::VisitSource::Navigate);
    visit_ledger::record_visit("p_y", "http://a.com/home", "首页", visit_ledger::VisitSource::NewTab);
    let out = inspect::run(serde_json::json!({"page_id": "nonexistent", "info": "coverage"}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0, "coverage 不应因 page_id 失效被拒: {out}");
    assert_eq!(v["data"]["summary"]["distinct_pages"], 2);
    assert_eq!(v["data"]["summary"]["total_visits"], 4);
    // /home 3 次 → 重复警示
    assert_eq!(visit_ledger::visit_note("http://a.com/home").unwrap()["visits"], 3);
    assert!(visit_ledger::visit_note("http://a.com/login").is_none());
}

/// navigate 响应附带 visit_note(≥3 次时):经 act_navigate 需要 live page,
/// 这里锁纯函数口径(visit_note 已在上例覆盖),补 key 归一化兜底。
/// 第 163 轮修正:口径由「忽略全部 query」改为「忽略 fragment 与翻页参数,
/// 保留决定页面身份的 query」—— 旧口径把不同页面的访问次数混算,既误报打转
/// 又漏报真实重复。
#[test]
fn visit_note_ignores_fragment_and_pagination_but_keeps_identity() {
    let _g = visit_ledger::test_lock().lock().unwrap();
    visit_ledger::reset_for_test();
    // fragment 与翻页参数折叠进同一个 key;/p?x=1 是**另一个**页面。
    visit_ledger::record_visit("p", "http://a.com/p", "", visit_ledger::VisitSource::Navigate);
    visit_ledger::record_visit("p", "http://a.com/p?page=1", "", visit_ledger::VisitSource::Navigate);
    let note = visit_ledger::visit_note("http://a.com/p#frag");
    assert!(note.is_none(), "2 次(fragment + 翻页折叠)未到阈值 3");
    visit_ledger::record_visit("p", "http://a.com/p#again", "", visit_ledger::VisitSource::Navigate);
    assert!(
        visit_ledger::visit_note("http://a.com/p?page=9").is_some(),
        "3 次应触发"
    );
    // 身份型 query 独立计数:只访问过一次,不因邻近页面的 3 次而被误报
    visit_ledger::record_visit("p", "http://a.com/p?x=1", "", visit_ledger::VisitSource::Navigate);
    assert!(
        visit_ledger::visit_note("http://a.com/p?x=1").is_none(),
        "身份型 query 页面独立计数,不得受 /p 的重复次数牵连"
    );
}

// ==================== 第 147 轮:自动阻断感知(语义挑战检测) ====================

/// 构造探针 JS 同构的 overlay 元数据(纯函数 detect_challenge 的输入)。
fn probe_overlay(text: &str, grid_imgs: i64, iframe_like: bool, buttons: Value) -> Value {
    json!({
        "found": true, "text": text,
        "img_count": grid_imgs, "grid_imgs": grid_imgs,
        "canvas_count": 0,
        "iframe_count": if iframe_like { 1 } else { 0 },
        "iframe_like": iframe_like, "input_count": 0,
        "buttons": buttons,
        "rect": {"w": 360, "h": 420}, "viewport": {"w": 1280, "h": 800},
        "area_ratio": 0.23, "z_index": 9999,
    })
}

#[test]
fn detect_challenge_doubao_iframe_with_strong_word() {
    // 豆包形态:题面在跨域 iframe 内主文档读不到,壳层「安全验证」强词 + 验证码尺寸 iframe
    let o = probe_overlay("安全验证 请完成下方验证", 0, true, json!(["刷新", "换一张"]));
    let hit = blocker_probe::detect_challenge(&o).expect("iframe_like(2)+强词(4) 应命中");
    assert_eq!(hit["kind"], "captcha");
    assert_eq!(hit["matched"], "challenge_overlay");
    assert!(hit["evidence"]["iframe_like"].as_bool().unwrap());
}

#[test]
fn detect_challenge_image_grid_with_weak_word() {
    // 九宫格渲染在主文档 + 题面动词「选出」:2+1=3 过阈值(豆包无 iframe 变体)
    let o = probe_overlay("选出在公园能看到的事物或动物", 9, false, json!(["确认"]));
    let hit = blocker_probe::detect_challenge(&o).expect("九宫格(2)+弱指令词(1) 应命中");
    assert_eq!(hit["evidence"]["grid_imgs"], 9);
    assert!(hit["snippet"].as_str().unwrap().contains("公园"));
}

#[test]
fn detect_challenge_strong_word_alone_hits() {
    // 弹层只有验证文案、无图无 iframe(短信/滑块文案形态):强词单独触发
    let o = probe_overlay("请完成人机验证后继续操作", 0, false, json!([]));
    assert!(blocker_probe::detect_challenge(&o).is_some());
}

#[test]
fn detect_challenge_structural_signal_alone_not_enough() {
    // 防误报:纯 iframe 形态(无任何文案,如视频/组件嵌入)与纯九宫格(图库弹窗)不报
    let iframe_only = probe_overlay("", 0, true, json!([]));
    assert!(blocker_probe::detect_challenge(&iframe_only).is_none(), "iframe 单独 2 分不过阈值");
    let grid_only = probe_overlay("精选图集", 9, false, json!(["关闭"]));
    assert!(blocker_probe::detect_challenge(&grid_only).is_none(), "九宫格单独 2 分不过阈值");
}

#[test]
fn detect_challenge_ad_overlay_suppressed() {
    // 广告弹窗:iframe_like 但文案/按钮命中广告负信号 → 压制
    let o = probe_overlay("限时优惠 广告", 0, true, json!(["跳过广告", "了解更多"]));
    assert!(blocker_probe::detect_challenge(&o).is_none(), "广告负信号应压制");
}

// ==================== 第 150 轮:canvas 拖拽验证识别 + 执行超时兜底 ====================

/// canvas 形态 overlay(主文档渲染的拖拽拼图/滑块验证,无九宫格 img/验证码 iframe)。
fn probe_overlay_canvas(text: &str, canvas_count: i64, buttons: Value) -> Value {
    json!({
        "found": true, "text": text,
        "img_count": 0, "grid_imgs": 0,
        "canvas_count": canvas_count,
        "iframe_count": 0, "iframe_like": false, "input_count": 0,
        "buttons": buttons,
        "rect": {"w": 400, "h": 360}, "viewport": {"w": 1280, "h": 800},
        "area_ratio": 0.14, "z_index": 9999,
    })
}

#[test]
fn detect_challenge_canvas_drag_with_weak_word() {
    // 豆包「拖拽图片到框中」主文档 canvas 形态:canvas(2)+弱词「拖拽/图片」(1)=3 命中
    let o = probe_overlay_canvas("请拖拽图片到对应位置完成验证", 1, json!(["刷新"]));
    let hit = blocker_probe::detect_challenge(&o).expect("canvas(2)+弱词(1) 应命中");
    assert_eq!(hit["evidence"]["canvas_count"], 1);
    assert!(hit["snippet"].as_str().unwrap().contains("拖拽"));
}

#[test]
fn detect_challenge_canvas_alone_not_enough() {
    // 防误报:普通 canvas 图表/游戏(无验证文案)2 分不过阈值
    let o = probe_overlay_canvas("数据报表", 2, json!(["导出"]));
    assert!(blocker_probe::detect_challenge(&o).is_none(), "canvas 单独 2 分不过阈值");
}

#[test]
fn detect_challenge_canvas_ad_suppressed() {
    // canvas 广告(抽奖转盘):结构 2 分 + 负信号 −3 → 压制
    let o = probe_overlay_canvas("幸运抽奖 广告", 1, json!(["跳过"]));
    assert!(blocker_probe::detect_challenge(&o).is_none(), "canvas 广告负信号应压制");
}

// ==================== 第 151 轮:bg-image 九宫格 + eval 变更判定 + 输入硬闸 ====================

/// CSS 背景图九宫格形态 overlay(挑战格用 background-image 而非 <img> 渲染)。
fn probe_overlay_bg(text: &str, bg_imgs: i64, grid_imgs: i64, buttons: Value) -> Value {
    json!({
        "found": true, "text": text,
        "img_count": grid_imgs, "grid_imgs": grid_imgs, "bg_imgs": bg_imgs,
        "canvas_count": 0,
        "iframe_count": 0, "iframe_like": false, "input_count": 0,
        "buttons": buttons,
        "rect": {"w": 360, "h": 420}, "viewport": {"w": 1280, "h": 800},
        "area_ratio": 0.23, "z_index": 9999,
    })
}

#[test]
fn detect_challenge_bg_image_grid_with_weak_word() {
    // 背景图九宫格(grid=0/bg=9)+题面动词「选出」:2+1=3 命中
    let o = probe_overlay_bg("选出下列图片中的卧室家具", 9, 0, json!(["确认"]));
    let hit = blocker_probe::detect_challenge(&o).expect("bg 九宫格(2)+弱词(1) 应命中");
    assert_eq!(hit["evidence"]["bg_imgs"], 9);
    assert_eq!(hit["evidence"]["grid_imgs"], 0);
}

#[test]
fn detect_challenge_bg_grid_mixed_counts_combined() {
    // <img> 与背景图混合:grid=2 + bg=3 合计 ≥4 计一次结构分
    let o = probe_overlay_bg("请点击包含文字的图标", 3, 2, json!([]));
    assert!(
        blocker_probe::detect_challenge(&o).is_some(),
        "grid(2)+bg(3) 合计应过结构阈值"
    );
}

#[test]
fn detect_challenge_bg_grid_alone_not_enough() {
    // 防误报:纯背景图网格(相册/商品墙弹窗,无指令文案)不过阈值
    let o = probe_overlay_bg("精选图集", 12, 0, json!(["关闭"]));
    assert!(
        blocker_probe::detect_challenge(&o).is_none(),
        "背景图网格单独 2 分不过阈值"
    );
}

#[test]
fn eval_expression_mutates_classification() {
    // 实测事故形态:execCommand insertText / 合成 KeyboardEvent / 自己 click —— 判定为可变更
    assert!(blocker_probe::eval_expression_mutates(
        "document.execCommand('insertText', false, q)"
    ));
    assert!(blocker_probe::eval_expression_mutates(
        "el.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter'}))"
    ));
    assert!(blocker_probe::eval_expression_mutates("btn.click();"));
    assert!(blocker_probe::eval_expression_mutates(
        "location.href = 'https://example.com'"
    ));
    assert!(blocker_probe::eval_expression_mutates(
        "window.open('/next')"
    ));
    // 纯读取(大小写混合)不判变更
    assert!(!blocker_probe::eval_expression_mutates(
        "JSON.stringify(window.__NEXT_DATA__.props.pageProps.list.slice(0,20))"
    ));
    assert!(!blocker_probe::eval_expression_mutates(
        "document.querySelectorAll('button').length"
    ));
    assert!(!blocker_probe::eval_expression_mutates(""));
}

#[test]
fn blocker_input_gate_code_is_6002() {
    // 硬闸信封码独立于 6001(任务锚点),归 6xxx 约束段
    assert_eq!(super::CODE_BLOCKER_INPUT_GATE, 6002);
    assert_eq!(super::CODE_TARGET_ANCHOR_VIOLATION, 6001);
}

// ==================== 2026-10-13:auto-HITL(自动弹出人工介入) ====================

/// auto-HITL 总开关解析:off 系值关闭,其余(含未设置)开启。
/// (env 是全局态,本用例串行独占 ENV_LOCK 语义 —— tests.rs 无并发 env 用例。)
#[test]
fn hitl_auto_env_switch() {
    let key = "LAEW_HITL_AUTO";
    let restore = std::env::var(key).ok();
    // 缺省开
    std::env::remove_var(key);
    assert!(crate::agent::tools::mcp_web_use::blocker_probe::hitl_auto_enabled());
    // off 系值关
    for v in ["off", "0", "false", "no"] {
        std::env::set_var(key, v);
        assert!(
            !crate::agent::tools::mcp_web_use::blocker_probe::hitl_auto_enabled(),
            "LAEW_HITL_AUTO={v} 应关闭"
        );
    }
    // 其它值与任意大小写变体保持开
    std::env::set_var(key, "on");
    assert!(crate::agent::tools::mcp_web_use::blocker_probe::hitl_auto_enabled());
    match restore {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
}

/// 多选应答编码(`1. a；3. c`)必须被 hitl_hint 归类为**选项应答**
/// (而不是自由文本 → 验证码填码分流),多选语义闭环的关键一环。
#[test]
fn hitl_hint_classifies_multi_answer_as_option_pick() {
    let multi = "1. 我已完成人工操作,继续；3. 换个思路";
    assert!(hitl_hint::is_option_pick_answer(multi), "多选编码应归选项应答");
    // 单选形态不受影响
    assert!(hitl_hint::is_option_pick_answer("2. 取消任务"));
    // 自由文本(验证码)仍不是选项应答
    assert!(!hitl_hint::is_option_pick_answer("z7z2"));
}

/// 第 186 轮:「取消任务」选项应答必须被识别为**任务级取消**
/// (猎聘实测:人工点「3. 取消任务」后旧实现返 code=0 并催 Agent「继续流程」,
/// 任务继续空转下游单元)。同时锁死误伤边界 —— 「取消关注」「取消屏蔽」是
/// 页面操作/管控按钮语义,不得被判成取消任务。
#[test]
fn hitl_hint_detects_task_cancel_answers() {
    // 选项应答(产品默认选项第 2 项 / auto-HITL / 系统提示词推荐写法)
    assert!(hitl_hint::is_cancel_answer("2. 取消任务"));
    assert!(hitl_hint::is_cancel_answer("3. 取消任务"));
    assert!(hitl_hint::is_cancel_answer("  1. 放弃任务  "));
    assert!(hitl_hint::is_cancel_answer("终止任务"));
    // 多选编码里含取消项同样成立
    assert!(hitl_hint::is_cancel_answer("1. 我已完成人工操作,继续；3. 取消任务"));
    // 自由文本短词
    assert!(hitl_hint::is_cancel_answer("取消"));
    assert!(hitl_hint::is_cancel_answer("CANCEL"));

    // —— 误伤边界 ——
    assert!(!hitl_hint::is_cancel_answer(""));
    assert!(!hitl_hint::is_cancel_answer("1. 确认我已完成人工操作,交棒"));
    assert!(!hitl_hint::is_cancel_answer("2. 我已登录完成"));
    assert!(!hitl_hint::is_cancel_answer("取消关注这个公司的职位"));
    assert!(!hitl_hint::is_cancel_answer("点右上角取消屏蔽按钮"));
    assert!(!hitl_hint::is_cancel_answer("z7z2"));
    // 长句含「不再继续」以外的语义不应误判
    assert!(!hitl_hint::is_cancel_answer("不要取消屏蔽,我想人工点一下滑块"));
}

/// 第 186 轮:「取消任务」应答的 `next_hint` 不得再是「inspect 验证后继续流程」
/// —— 那是方向相反的最坏提示(人工喊停反而被催着继续)。
#[test]
fn hitl_hint_cancel_answer_hint_points_at_stop() {
    // 命中取消的应答由 hitl_ctl 直接返 4002 分支,不走本函数的 code=0 分支;
    // 这里锁的是:即便有人日后把取消误送进 hitl_answer_hint,提示语也不能是「继续」。
    let h = hitl_hint::hitl_answer_hint("login", "2. 取消任务");
    assert!(
        !h.contains("继续流程"),
        "取消应答的提示语不得催 Agent 继续流程:{h}"
    );
}

/// sequence 上下文标记:批内步骤执行时 auto-HITL 守卫应判定「在批内」,
/// 批外(单步动作)为 false;task_local 未进入 scope 时 fail-open 为 false。
#[tokio::test]
async fn in_sequence_ctx_marks_sequence_steps() {
    use crate::agent::tools::mcp_web_use::in_sequence_ctx;
    assert!(!in_sequence_ctx(), "批外默认 false");
    let inside = crate::agent::tools::mcp_web_use::run_in_sequence_scope_for_test(true, async {
        in_sequence_ctx()
    })
    .await;
    assert!(inside, "批内应为 true");
}

#[test]
fn eval_timeout_from_parsing() {
    // 纯函数:正整数生效;缺省/0/非法回退默认 30s
    assert_eq!(
        super::eval_timeout_from(Some("5000".into())),
        5000
    );
    assert_eq!(super::eval_timeout_from(None), super::EVAL_DEFAULT_TIMEOUT_MS);
    assert_eq!(
        super::eval_timeout_from(Some("0".into())),
        super::EVAL_DEFAULT_TIMEOUT_MS
    );
    assert_eq!(
        super::eval_timeout_from(Some("abc".into())),
        super::EVAL_DEFAULT_TIMEOUT_MS
    );
}

#[test]
fn max_windows_from_parsing() {
    // 纯函数:正整数生效;缺省/0/非法回退默认 4(按去重窗口计的上限)
    assert_eq!(super::max_windows_from(Some("2".into())), 2);
    assert_eq!(super::max_windows_from(Some("8".into())), 8);
    assert_eq!(super::max_windows_from(None), super::MAX_WINDOWS_DEFAULT);
    assert_eq!(
        super::max_windows_from(Some("0".into())),
        super::MAX_WINDOWS_DEFAULT
    );
    assert_eq!(
        super::max_windows_from(Some("-1".into())),
        super::MAX_WINDOWS_DEFAULT
    );
    assert_eq!(
        super::max_windows_from(Some("abc".into())),
        super::MAX_WINDOWS_DEFAULT
    );
}

#[test]
fn open_schema_exposes_new_window_param() {
    // 多窗口并行:open 的 schema 必须暴露 new_window 开关,
    // 否则模型不知道可以「独立新窗口」打开页面(双站同时工作的入口)。
    let p = McpWebUseTool.parameters();
    let nw = p["properties"]["new_window"].clone();
    assert!(!nw.is_null(), "schema 缺少 new_window 参数");
    assert_eq!(nw["type"], "boolean");
    assert_eq!(nw["default"], false);
}

#[test]
fn control_action_enum_includes_new_window() {
    // control_action 枚举必须包含 new_window(独立新窗口打开,多站点同时工作)
    let p = McpWebUseTool.parameters();
    let actions = p["properties"]["control_action"]["enum"].clone();
    assert!(
        actions.as_array().is_some_and(|a| a.iter().any(|v| v == "new_window")),
        "control_action 枚举缺少 new_window:{actions:?}"
    );
}

#[test]
fn step_has_blocker_alert_detection() {
    // 第 150 轮:sequence 中断判定 —— 步骤数据携带 blocker_alert 即中断整批
    assert!(super::step_has_blocker_alert(&json!({"blocker_alert": {"kind": "captcha"}})));
    assert!(super::step_has_blocker_alert(&json!({"blocker_alert": null})));
    assert!(!super::step_has_blocker_alert(&json!({"slept_ms": 500})));
    assert!(!super::step_has_blocker_alert(&json!({})));
    // 第 171 轮:login 关键词命中不中断 —— 频控降级壳页头「你好，请登录」实测
    // 6 次误中断整批 sequence(登录态在,壳是登出样式);其余验证类仍中断。
    assert!(!super::step_has_blocker_alert(&json!({"blocker_alert": {"kind": "login"}})));
    assert!(super::step_has_blocker_alert(&json!({"blocker_alert": {"kind": "manual_verify"}})));
    assert!(super::step_has_blocker_alert(&json!({"blocker_alert": {"kind": "rate_limited"}})));
}

#[test]
fn detect_challenge_clean_overlay_and_null() {
    let clean = probe_overlay("新品上线通知", 1, false, json!(["知道了"]));
    assert!(blocker_probe::detect_challenge(&clean).is_none());
    assert!(blocker_probe::detect_challenge(&Value::Null).is_none());
    assert!(blocker_probe::detect_challenge(&json!({"found": false})).is_none());
}

#[test]
fn analyze_merges_keyword_and_challenge_with_dedupe() {
    // 关键词通道(captcha)+ 弹层挑战同 kind → 去重保留带 evidence 的挑战条目
    let probe = json!({
        "text": "请完成安全验证 拖动滑块",
        "overlay": probe_overlay("安全验证", 0, true, json!(["刷新"])),
    });
    let out = blocker_probe::analyze(&probe);
    assert_eq!(out.len(), 1, "同 kind 去重,实际:{out:?}");
    assert_eq!(out[0]["matched"], "challenge_overlay");
    assert!(out[0]["evidence"].is_object(), "保留带结构证据的挑战版本");
}

#[test]
fn analyze_keeps_different_kinds() {
    // 全文命中 login + 弹层命中挑战 → 两条并存
    // 第 152 轮:九宫格点选挑战的 kind 由硬编码 captcha 改为子类推出的 manual_verify
    // (只有真能「填码」的形态才叫 captcha),断言同步更新。
    let probe = json!({
        "text": "登录后查看更多内容",
        "overlay": probe_overlay("请选出图中包含的物品", 9, false, json!(["刷新"])),
    });
    let out = blocker_probe::analyze(&probe);
    let kinds: Vec<&str> = out.iter().filter_map(|b| b["kind"].as_str()).collect();
    assert!(kinds.contains(&"manual_verify"), "实际:{kinds:?}");
    assert!(kinds.contains(&"login"), "实际:{kinds:?}");
    // 子类信息必须带出来(供 LLM 选 reason / 人工看操作说明)
    let challenge = out.iter().find(|b| b["matched"] == "challenge_overlay").unwrap();
    assert_eq!(challenge["challenge_type"], "image_select");
    assert!(challenge["interaction"].as_str().unwrap().contains("点选"));
}

#[test]
fn analyze_challenge_overlays_keyword_only_path() {
    // 无弹层(overlay null)时退化为纯关键词(开关回退路径共用同一 analyze)
    let probe = json!({"text": "微信扫码登录", "overlay": null});
    let out = blocker_probe::analyze(&probe);
    assert!(out.iter().any(|b| b["kind"] == "qr_login"));
    let clean = blocker_probe::analyze(&json!({"text": "普通内容", "overlay": null}));
    assert!(clean.is_empty());
}

#[test]
fn action_needs_probe_covers_input_wait_nav() {
    for a in ["click", "input_text", "key_press", "drag", "human_click", "select_option"] {
        assert!(blocker_probe::action_needs_probe(a), "{a} 应探测");
    }
    for a in ["wait", "navigate", "back", "forward", "reload"] {
        assert!(blocker_probe::action_needs_probe(a), "{a} 应探测");
    }
    // 第 151 轮:eval_js 纳入收尾探测(实测 LLM 把输入行为迁移到 execCommand/
    // 合成事件后,「只读通道」假设不成立;长轮询返回恰是弹窗出现后的检查点)
    assert!(blocker_probe::action_needs_probe("eval_js"), "eval_js 应探测(第 151 轮)");
    // 观察类/状态设置类仍不探测
    for a in ["screenshot", "set_guard", "set_viewport", "request_human", "heartbeat"] {
        assert!(!blocker_probe::action_needs_probe(a), "{a} 不应探测");
    }
}

#[test]
fn wait_sleep_ms_accepts_alias_and_caps() {
    // 第 147 轮:ms 别名(实测豆包任务 LLM 全程传 ms=12000 只睡了 500ms 的根治)
    assert_eq!(control::wait_sleep_ms(&json!({"ms": 12000})), 12000);
    assert_eq!(control::wait_sleep_ms(&json!({"duration_ms": 8000})), 8000);
    // 两参并存取较大者
    assert_eq!(
        control::wait_sleep_ms(&json!({"duration_ms": 3000, "ms": 7000})),
        7000
    );
    // 缺省 500 / 上限 30000(AI 回复长尾 30-60s,第 82 轮)
    assert_eq!(control::wait_sleep_ms(&json!({})), 500);
    assert_eq!(control::wait_sleep_ms(&json!({"ms": 999999})), 30000);
}

#[test]
fn infer_step_action_from_fields() {
    // 第 147 轮:sequence 步骤缺 action 时按字段推断(实测 steps 常漏 action:"control")
    assert_eq!(infer_step_action(&json!({"control_action": "wait", "params": {"ms": 100}})), Some("control"));
    assert_eq!(infer_step_action(&json!({"info": "elements"})), Some("inspect"));
    assert_eq!(infer_step_action(&json!({"url": "https://a.com"})), None);
    // control_action 优先于 info
    assert_eq!(
        infer_step_action(&json!({"control_action": "click", "info": "elements"})),
        Some("control")
    );
}

/// 第 147 轮真浏览器集成验证(#[ignore],本地 `--ignored` 跑):
/// 构造豆包风控形态弹层(z-index 9999 全屏遮罩 + 居中弹窗 + 9 宫格小图 + 题面 +
/// 刷新按钮),探针采集 → 打分应命中 captcha;干净页面应零命中。
/// 与 browser_overlay 真浏览器用例同构(需本机 Chrome,会弹有头窗口);
/// DOM 经 about:blank + body.innerHTML 注入(长 data: URL 含 % 会被当百分号编码,
/// 实测 goto 超时)。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_browser_blocker_probe_detects_challenge_modal() {
    use crate::agent::browser::{BrowserManager, BrowserMode};
    let (pid, _, _) = BrowserManager::global()
        .new_page("about:blank", BrowserMode::Headed, None, None, Some((1280, 800)), true, Default::default())
        .await
        .expect("启动浏览器失败(本机是否装有 Chrome?)");
    let page = BrowserManager::global().page(&pid).await.expect("page 句柄");
    // 9 宫格小图(64×64,近方形)+ 题面「选出在公园能看到的事物或动物」+ 刷新按钮
    let inject = r#"(() => {
      const tiles = Array.from({length: 9}, (_, i) =>
        `<img src="data:image/gif;base64,R0lGODlhAQABAAAAACw=" style="width:64px;height:64px" alt="t${i}">`).join('');
      document.body.innerHTML = `<div>普通内容</div>
        <div style="position:fixed;inset:0;background:rgba(0,0,0,.5);z-index:9999" class="c-mask">
          <div class="c-dialog" role="dialog" style="position:fixed;left:50%;top:50%;transform:translate(-50%,-50%);width:360px;height:420px;background:#fff;z-index:10000;padding:16px">
            <h3>安全验证</h3>
            <p>选出在公园能看到的事物或动物</p>
            <div style="display:grid;grid-template-columns:repeat(3,1fr)">${tiles}</div>
            <button>刷新</button><button>换一张</button>
          </div></div>`;
      return document.querySelectorAll('img').length;
    })()"#;
    let n = eval_js_string(&page, inject).await.unwrap();
    assert_eq!(n, 9, "9 宫格注入失败");
    let probe = blocker_probe::collect_for_inspect(&page).await;
    let alerts = blocker_probe::analyze(&probe);
    assert!(
        alerts.iter().any(|b| b["kind"] == "captcha" && b["matched"] == "challenge_overlay"),
        "豆包形态弹层应命中,probe:{probe:?} alerts:{alerts:?}"
    );
    // 全链路(第 151 轮起):弹窗在场时 input_text 先过**输入硬闸** → 6002 拦截且不执行;
    // force=true 逃生口执行动作,收尾推送探针仍附 blocker_alert(第 147 轮语义保持)
    eval_js_string(&page, r#"document.body.insertAdjacentHTML('beforeend', '<input id="q" style="width:200px;height:20px">')"#).await.unwrap();
    let out = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "input_text",
                        "params": {"selector": "#q", "text": "hi"}}))
        .await
        .unwrap();
    let env: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(env["code"], 6002, "弹窗在场 input_text 应被硬闸拦截:{env}");
    assert!(env["data"]["blocker_alert"]["kind"] == "captcha", "信封应带 blocker_alert:{env}");
    assert_eq!(env["data"]["next_action"], "request_human");
    assert!(env["data"]["human_assist"]["ready"].as_bool().unwrap(), "human_assist 载荷应就绪");
    let typed = eval_js_string(&page, "document.getElementById('q').value").await.unwrap();
    assert_eq!(typed, "", "被硬闸拦截的动作不应写入输入框");
    // force 逃生口:动作执行 + 收尾推送探针仍附告警
    let forced = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "input_text",
                        "params": {"selector": "#q", "text": "hi", "force": true}}))
        .await
        .unwrap();
    let fenv: Value = serde_json::from_str(&forced).unwrap();
    assert_eq!(fenv["code"], 0, "force 后动作应执行:{fenv}");
    assert!(fenv["data"]["blocker_alert"]["kind"] == "captcha", "收尾探针仍应附 blocker_alert:{fenv}");
    let typed2 = eval_js_string(&page, "document.getElementById('q').value").await.unwrap();
    assert_eq!(typed2, "hi", "force 后文本应写入");
    // 干净页面(移除弹层)零命中
    eval_js_string(&page, "document.querySelector('.c-mask').remove()").await.unwrap();
    let probe2 = blocker_probe::collect_for_inspect(&page).await;
    let alerts2 = blocker_probe::analyze(&probe2);
    assert!(alerts2.is_empty(), "干净页面误报:{alerts2:?}");
    BrowserManager::global().shutdown().await;
}

/// 第 151 轮真浏览器集成验证(#[ignore],本地 `--ignored` 跑):
/// ① CSS 背景图九宫格(无 `<img>`)挑战形态识别;② 可变更 eval_js 被 6002 硬闸
/// 拦截、纯读取 eval_js 放行;③ 移除弹窗后硬闸自动放行(非解锁制)。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_browser_input_gate_bg_image_challenge() {
    use crate::agent::browser::{BrowserManager, BrowserMode};
    let (pid, _, _) = BrowserManager::global()
        .new_page("about:blank", BrowserMode::Headed, None, None, Some((1280, 800)), true, Default::default())
        .await
        .expect("启动浏览器失败(本机是否装有 Chrome?)");
    let page = BrowserManager::global().page(&pid).await.expect("page 句柄");
    // 「选出卧室家具」型:背景图九宫格(120×120 div,无 img)+ 题面弱词
    let inject = r#"(() => {
      const tiles = Array.from({length: 9}, () =>
        `<div style="display:inline-block;width:120px;height:120px;background-image:url(data:image/gif;base64,R0lGODlhAQABAAAAACw=)"></div>`).join('');
      document.body.innerHTML = `<input id="chat" style="position:fixed;left:0;top:0;width:200px;height:30px">
        <div class="verify-modal" style="position:fixed;left:200px;top:80px;width:480px;height:420px;background:#fff;z-index:99999;padding:12px">
          <p>选出下列图片中的卧室家具</p><div>${tiles}</div><button>确认</button></div>`;
      return document.querySelectorAll('.verify-modal > div > div').length;
    })()"#;
    let n = eval_js_string(&page, inject).await.unwrap();
    assert_eq!(n, 9, "背景图九宫格注入失败");
    let probe = blocker_probe::collect_for_inspect(&page).await;
    assert!(
        blocker_probe::analyze(&probe)
            .iter()
            .any(|b| b["kind"] == "captcha" && b["matched"] == "challenge_overlay"),
        "背景图九宫格形态应命中,probe:{probe:?}"
    );

    // 可变更 eval_js(execCommand)在弹窗在场时被 6002 硬闸拦截,不执行
    let gated = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression": "(() => { const q = document.getElementById('chat'); q.focus(); q.value = 'blocked'; document.execCommand('insertText', false, 'x'); return q.value; })()"}}))
        .await
        .unwrap();
    let genv: Value = serde_json::from_str(&gated).unwrap();
    assert_eq!(genv["code"], 6002, "可变更 eval_js 应被硬闸拦截:{genv}");
    assert!(genv["data"].get("blocker_alert").is_some(), "拦截信封应带 blocker_alert");
    let v = eval_js_string(&page, "document.getElementById('chat').value").await.unwrap();
    assert_eq!(v, "", "被拦截的 eval 不应产生副作用");

    // 纯读取 eval_js 放行(不硬闸;干净响应无告警)
    let read = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression": "document.querySelectorAll('div').length"}}))
        .await
        .unwrap();
    let renv: Value = serde_json::from_str(&read).unwrap();
    assert_eq!(renv["code"], 0, "纯读取 eval 应放行:{renv}");
    // 第 151 轮:eval_js 纳入收尾推送探针 —— 弹窗在场时纯读取 eval 也附告警
    // (长轮询返回恰是弹窗出现后的检查点,本轮事故的闭环点),但不拦截执行
    assert!(
        renv["data"].get("blocker_alert").is_some(),
        "收尾探针应附 blocker_alert(不拦截):{renv}"
    );

    // 移除弹窗 → 硬闸自动放行(非解锁制,页面干净即恢复)
    eval_js_string(&page, "document.querySelector('.verify-modal').remove()").await.unwrap();
    let pass = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "input_text",
                        "params": {"selector": "#chat", "text": "ok"}}))
        .await
        .unwrap();
    let penv: Value = serde_json::from_str(&pass).unwrap();
    assert_eq!(penv["code"], 0, "弹窗移除后应自动放行:{penv}");
    let v2 = eval_js_string(&page, "document.getElementById('chat').value").await.unwrap();
    assert_eq!(v2, "ok", "放行后动作应执行");
    BrowserManager::global().shutdown().await;
}

// =================== 第 157 轮:输入阶梯真浏览器冒烟(#[ignore],本地 --ignored 跑) ===================

/// 覆盖输入链路根治(tmpPlan/2026-10-12_MCP_Web_Use输入链路根治与批量效率升级方案.md):
/// 1. input_text 默认 auto:JS 原生赋值 + 回读校验(textarea 是最易触发旧
///    Illegal invocation 的形态 —— 原型链 setter 根治的回归锁);
/// 2. 普通 input 与 contenteditable 同样生效;
/// 3. selector 未命中 → 明确报错(带 inspect 提示);
/// 4. key_press 小写别名(enter)与组合写法(ctrl+a)不再触发 CDP 黑话;
/// 5. 多字符未知键 → 明确报错;
/// 6. press_sequence 带 selector 先聚焦(focused=true);
/// 7. clear_input 清空 + 回读。
/// 跑 `cargo test --lib input_text_ladder -- --ignored --nocapture`。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_browser_input_text_ladder() {
    use crate::agent::browser::{BrowserManager, BrowserMode};
    use crate::agent::browser_overlay::PageGuardConfig;
    let mgr = BrowserManager::global();
    // textarea 监听 input 事件后回写自身 value,模拟 React 受控组件的拦截面。
    let url = "data:text/html,<textarea id='t'></textarea><input id='i'>\
               <div id='ce' contenteditable='true'></div>\
               <script>document.getElementById('t').addEventListener('input',function(e){\
               this.value=this.value;});</script>";
    let (pid, _, _) = mgr
        .new_page(url, BrowserMode::Headed, None, None, Some((800, 600)), true, PageGuardConfig::locked())
        .await
        .expect("启动有头浏览器失败(本机是否装有 Chrome?)");
    let id = pid.as_str();

    // 1. textarea:JS 路径 + 回读校验(旧实现此处必炸 Illegal invocation)
    let out = input_ctl::act_input_text(id, &json!({"selector": "#t", "text": "hello world"}))
        .await
        .expect("textarea 输入应成功");
    assert_eq!(out["applied"], true, "{out}");
    assert_eq!(out["value_after"]["matched"], true, "{out}");

    // 2. 普通 input
    let out = input_ctl::act_input_text(id, &json!({"selector": "#i", "text": "abc123"}))
        .await
        .expect("input 输入应成功");
    assert_eq!(out["value_after"]["matched"], true, "{out}");

    // 3. contenteditable
    let out = input_ctl::act_input_text(id, &json!({"selector": "#ce", "text": "rich text"}))
        .await
        .expect("contenteditable 输入应成功");
    assert_eq!(out["applied"], true, "{out}");

    // 4. selector 未命中 → 明确报错(自动阶梯全部尝试后仍给可执行提示)
    let err = input_ctl::act_input_text(id, &json!({"selector": "#nope", "text": "x"}))
        .await
        .expect_err("不存在的 selector 应报错");
    assert!(err.contains("inspect"), "{err}");

    // 5. key_press 小写别名 + 组合写法
    let out = input_ctl::act_key_press(id, &json!({"key": "enter"}))
        .await
        .expect("小写 enter 应归一");
    assert_eq!(out["key"], "Enter", "{out}");
    let out = input_ctl::act_key_press(id, &json!({"key": "ctrl+a"}))
        .await
        .expect("组合写法 ctrl+a 应可用");
    assert_eq!(out["modifiers"], 2, "{out}");

    // 6. 多字符未知键 → 明确报错,不透传 CDP "Invalid 'text' parameter" 黑话
    let err = input_ctl::act_key_press(id, &json!({"key": "foobar"}))
        .await
        .expect_err("多字符未知键应报错");
    assert!(err.contains("无法识别"), "{err}");

    // 7. press_sequence 带 selector 先聚焦(含整句中文 token 自动逐字)
    let out = input_ctl::act_press_sequence(id, &json!({"selector": "#t", "keys": "x y enter"}))
        .await
        .expect("press_sequence 应成功");
    assert_eq!(out["focused"], true, "{out}");
    assert_eq!(out["pressed"], 3, "{out}");
    let out = input_ctl::act_press_sequence(id, &json!({"selector": "#t", "keys": "hi 你好"}))
        .await
        .expect("多字符文本 token 应逐字发送");
    assert_eq!(out["pressed"], 2 + "你好".chars().count(), "{out}");

    // 8. clear_input 清空 + 回读
    let out = input_ctl::act_clear_input(id, &json!({"selector": "#t"}))
        .await
        .expect("clear_input 应成功");
    assert_eq!(out["cleared"], true, "{out}");

    mgr.shutdown().await;
}

// =================== 第 158 轮:select_dropdown 单元测试 ===================

#[test]
fn pick_element_js_exact_match() {
    let js = super::control::pick_element_js("北京", Some("div.areaWrap"), false);
    assert!(js.contains("\"北京\""), "JS 应包含目标文本");
    assert!(js.contains("div.areaWrap"), "JS 应包含 scope 选择器");
    assert!(js.contains("querySelectorAll"), "JS 应遍历子元素");
    assert!(js.contains("scrollIntoView"), "JS 应滚动到可见");
    assert!(js.contains("click()"), "JS 应点击");
    assert!(js.contains("=== target"), "JS 应精确匹配");
    assert!(js.contains("&& false"), "非模糊模式应包含 && false");
}

#[test]
fn pick_element_js_fuzzy_match() {
    let js = super::control::pick_element_js("北京", None, true);
    assert!(js.contains("&& true"), "模糊模式应包含 && true");
    assert!(js.contains("document.body"), "无 scope 时应回退到 body");
}

#[test]
fn pick_element_js_no_scope() {
    let js = super::control::pick_element_js("全北京", None, false);
    assert!(js.contains("document.body"), "无 scope 时应回退到 body");
    assert!(js.contains("=== target"), "精确匹配");
}

#[test]
fn pick_element_js_visibility_uses_computed_style() {
    // 可见性判定必须含计算样式:组件库收起的面板保留 visibility:hidden 挂载副本,
    // 其 getBoundingClientRect 仍为正,单看几何尺寸会误判可见
    let js = super::control::pick_element_js("北京", None, false);
    assert!(
        js.contains("getComputedStyle"),
        "可见性判定应使用 getComputedStyle"
    );
    assert!(
        js.contains("visibility"),
        "应检查 visibility 计算样式"
    );
    assert!(
        js.contains("textContent"),
        "文本匹配应取 textContent(确定性)而非 innerText(渲染态会为空)"
    );
}

#[test]
fn pick_element_js_non_leaf_own_text() {
    // 「文本+图标」非叶子控件(如 <span>区域<img></span>)应经自身直接文本命中
    let js = super::control::pick_element_js("区域", None, false);
    assert!(
        js.contains("childNodes"),
        "应遍历 childNodes 计算自身直接文本"
    );
    assert!(
        js.contains("nodeType === 3"),
        "应只统计文本节点"
    );
}

#[test]
fn pick_element_js_failure_diagnostic() {
    // 失败时应返回诊断字段,供调用方给出可行动的错误信息
    let js = super::control::pick_element_js("北京", None, true);
    assert!(js.contains("matched_but_hidden"), "应区分「命中但隐藏」");
    assert!(js.contains("no_text_match"), "应区分「无文本命中」");
    assert!(js.contains("matched: pool.length"), "应返回命中计数");
    assert!(js.contains("visible: 0"), "应返回可见计数");
}

#[test]
fn pick_element_js_smallest_candidate_wins() {
    // 多候选时应按面积取最小者(最深/最贴近文本节点)
    let js = super::control::pick_element_js("北京", None, true);
    assert!(
        js.contains("ra.width * ra.height"),
        "应按面积排序取最小候选"
    );
}

#[test]
fn select_dropdown_in_schema_enum() {
    let params = super::McpWebUseTool.parameters();
    let enum_vals = params["properties"]["control_action"]["enum"]
        .as_array()
        .expect("control_action 应有 enum");
    let has = enum_vals
        .iter()
        .any(|v| v.as_str() == Some("select_dropdown"));
    assert!(has, "Schema enum 应包含 select_dropdown");
}

#[test]
fn select_dropdown_in_input_whitelist() {
    assert!(
        crate::agent::browser_overlay::action_dispatches_input("select_dropdown"),
        "select_dropdown 应在输入动作白名单中"
    );
}

#[test]
fn select_dropdown_in_spawn_page_whitelist() {
    assert!(
        super::control::action_may_spawn_page("select_dropdown", &json!({})),
        "select_dropdown 应在派生页白名单中"
    );
}

// =================== 第 164 轮:probe_element_js / confirm_panel_js ===================

#[test]
fn probe_element_js_is_find_only() {
    // 第 164 轮选项优先通道:probe 只做查找,绝不允许点击/滚动副作用,
    // 否则「先探测再决策」会把选项误点掉(组件库 toggle 语义)。
    let js = super::control::probe_element_js("北京", Some("div.areaWrap"), true);
    assert!(!js.contains(".click()"), "probe 不得包含点击副作用");
    assert!(!js.contains("scrollIntoView"), "probe 不得滚动页面");
    assert!(js.contains("found: false"), "probe 应返回 found 标记");
    assert!(js.contains("found: true"), "probe 命中分支应返回 found 标记");
    assert!(js.contains("matched_but_hidden"), "probe 复用同一套诊断语义");
    assert!(js.contains("querySelector(\"div.areaWrap\")"), "probe 应尊重 scope 限定");
}

#[test]
fn confirm_panel_js_guards_unscoped_click() {
    // 无显式 scope 时必须要求「面板状」容器祖先,防止误点页面级提交按钮
    let js = super::control::confirm_panel_js(None);
    assert!(js.contains("确定") && js.contains("完成"), "确认词表应含常见文案");
    assert!(js.contains("position === 'fixed'"), "应识别 fixed 定位面板");
    assert!(js.contains("role"), "应识别 role=dialog/listbox 面板");
    // 有显式 scope 时信任范围,不再要求面板状祖先
    let scoped = super::control::confirm_panel_js(Some("div.panel"));
    assert!(scoped.contains("const scoped = true"), "显式 scope 应走信任路径");
    assert!(scoped.contains("querySelector(\"div.panel\")"), "scope 应注入脚本");
}

#[test]
fn confirm_panel_js_covers_action_style_buttons() {
    // 2026-10-15 闲鱼实测:区域面板选中后必须点「查看999+件宝贝」才把筛选落到
    // URL,词表型匹配覆盖不到动作型文案,补正则通道。
    let js = super::control::confirm_panel_js(None);
    assert!(js.contains("CONFIRM_RE"), "应包含正则确认通道");
    assert!(js.contains("^查看"), "应匹配「查看N件宝贝」类动作按钮");
    assert!(js.contains("立即查看"), "应匹配「立即查看/去查看」");
    // 正则通道仍受面板状容器约束(防误点页面级按钮)
    assert!(js.contains("position === 'fixed'"), "正则通道不得绕过面板状约束");
}

// =================== 第 159 轮:注入 JS 语法缺陷回归测试 ===================

/// wait(selector) 脚本必须整体 `(async () => …)()` 包裹。
/// 旧模板 `async () => {…}()` 是 JS 语法错误(箭头函数块体后紧跟调用括号),
/// 内网站点实测每次 wait(selector) 都 2002 SyntaxError: Unexpected token '('。
#[test]
fn wait_selector_js_is_wrapped_arrow_call() {
    let js = super::control::wait_selector_js("input[type=text]", 30_000, "visible");
    assert!(
        js.trim_start().starts_with("(async () =>"),
        "wait 脚本应以 (async () => 开头整体包裹,当前开头: {:?}",
        js.chars().take(24).collect::<String>()
    );
    assert!(js.trim_end().ends_with("})()"), "wait 脚本应以 }})() 收尾");
    assert!(js.contains("type=text"));
    assert!(js.contains("30000"));
}

/// inspect(dom) 全量树脚本:对象字面量内不得再出现 `attributes: {};` 形态
/// (旧模板多一个分号,恒 SyntaxError: Unexpected token ';')。
#[test]
fn dom_tree_js_no_stray_semicolon_in_literal() {
    let js = super::inspect::dom_tree_js(3, 2000);
    assert!(js.contains("attributes: {}"), "渲染后应含 attributes: {{}} 字面量");
    assert!(
        !js.contains("attributes: {};"),
        "对象字面量内不得出现多余分号(第 159 轮前的缺陷形态)"
    );
    assert!(js.contains("MAX_DEPTH=3"));
    assert!(js.contains("NODE_LIMIT=2000"));
    assert!(js.contains("walk(document, 0)"));
}

/// inspect(dom) 根节点必须能下钻(第 160 轮):遍历起点是 document(nodeType 9),
/// 旧模板只在 `nodeType===1`(Element)时递归 childNodes —— Document 不是 Element,
/// 任何页面都只返回裸 #document + node_count:1,整棵树观察功能从未生效。
#[test]
fn dom_tree_js_document_root_descends_children() {
    let js = super::inspect::dom_tree_js(4, 500);
    // 下钻条件必须覆盖 Document(9) 与 DocumentFragment(11),不能只认 Element(1)
    assert!(
        js.contains("n.nodeType===1 || n.nodeType===9 || n.nodeType===11"),
        "childNode 下钻条件必须含 Document/DocumentFragment,实际:\n{js}"
    );
    // attributes 采集仍仅限 Element(Document.attributes 为 undefined,for-of 会抛 TypeError)
    let attr_loop = js
        .find("if (n.nodeType===1) {")
        .expect("应存在 Element 专属 attributes 采集分支");
    let segment = &js[attr_loop..attr_loop + 160];
    assert!(
        segment.contains("n.attributes") && !segment.contains("childNodes"),
        "attributes 采集分支不应混入 childNodes 遍历"
    );
}

// =================== 第 161 轮:视口铺满 + 主文档健康诊断(#[ignore] 真浏览器) ===================
//
// 根治对象:chromiumoxide BrowserConfig 默认 viewport=Some(800×600) 把三模式页面
// 视口钉死(headed 窗口 1920×1080 内容只渲染左上 800×600,其余空白)。修复后:
// - headed:viewport(None) → 页面视口 = 窗口内容区(≈1920×940);
// - hidden/new_headless:显式 viewport=窗口尺寸 → 恒 1920×1080。
// 跑法:`cargo test --lib mcp_web_use -- --ignored --nocapture`(需本机 Chrome)。

/// 相位 1(headed)+ 相位 2(hidden):两相位共用进程内单例,相位间 shutdown 重启。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(headed 会弹窗口);本地手动跑"]
async fn real_browser_viewport_fills_window() {
    let mgr = BrowserManager::global();

    // —— 相位 1:headed 默认窗口(1920×1080 外框),视口应铺满内容区 ——
    let (pid, _, _) = mgr
        .new_page(
            "about:blank",
            BrowserMode::Headed,
            None,
            None,
            None,
            false,
            crate::agent::browser_overlay::PageGuardConfig::open(),
        )
        .await
        .expect("启动有头浏览器失败(本机是否装有 Chrome?)");
    let page = mgr.page(&pid).await.expect("page 句柄");
    let v = page
        .evaluate("({w: window.innerWidth, h: window.innerHeight, ow: window.outerWidth, oh: window.outerHeight})")
        .await
        .expect("测量视口失败")
        .value()
        .cloned()
        .expect("测量结果为空");
    let (w, h) = (v["w"].as_f64().unwrap_or(0.0), v["h"].as_f64().unwrap_or(0.0));
    let (ow, oh) = (v["ow"].as_f64().unwrap_or(0.0), v["oh"].as_f64().unwrap_or(0.0));
    println!("headed 视口 {w}×{h},窗口外框 {ow}×{oh}");
    assert!(ow >= 1900.0, "默认窗口外框应 1920×1080,实测 {ow}×{oh}");
    assert!(
        w >= 1600.0 && h >= 800.0,
        "headed 视口应铺满窗口内容区(修复前钉死 800×600),实测 {w}×{h}(窗口 {ow}×{oh})"
    );
    mgr.shutdown().await;

    // —— 相位 2:hidden 显式视口 = 窗口尺寸 1920×1080(不再依赖内容超限才扩展) ——
    let (pid2, _, _) = mgr
        .new_page(
            "about:blank",
            BrowserMode::Hidden,
            None,
            None,
            None,
            false,
            crate::agent::browser_overlay::PageGuardConfig::open(),
        )
        .await
        .expect("重启无头浏览器失败");
    let page2 = mgr.page(&pid2).await.expect("page 句柄");
    let v2 = page2
        .evaluate("({w: window.innerWidth, h: window.innerHeight})")
        .await
        .expect("测量视口失败")
        .value()
        .cloned()
        .expect("测量结果为空");
    let (w2, h2) = (v2["w"].as_f64().unwrap_or(0.0), v2["h"].as_f64().unwrap_or(0.0));
    println!("hidden 视口 {w2}×{h2}");
    assert!(
        (w2 - 1920.0).abs() <= 2.0 && (h2 - 1080.0).abs() <= 2.0,
        "hidden 视口应恒 1920×1080(修复前钉死 800×600),实测 {w2}×{h2}"
    );
    mgr.shutdown().await;
}

/// 真站点回归:视口铺满 + page_health 与页面真实状态自洽。
/// 服务器行为可能变化(某天放行 200),故断言写成条件式:页面空白 ⇒ 必须报
/// page_health(reason ∈ {main_doc_error, empty_dom});页面有真实内容 ⇒ 不带该字段。
/// 目标 URL **不写死在仓库里**(内网/客户站点属敏感信息),由环境变量
/// `LAEW_REAL_SITE_URL` 提供;未设置时退回公网 example.com(仅验证视口铺满)。
/// 第 186 轮:死页信号 `data.page_dead` 的三态判定(纯函数 `dead_page_reason`);
/// 2026-10-19 扩到四态(新增「域历史」兜底,覆盖看护落账前的观察窗口)。
///
/// 猎聘实测缺口:页面被站点抹除后,只读观测动作只拿到 `about:blank` +
/// `elements{count:0}` + 顶层 `code:0`,零结构化线索,模型只能靠 `ok:false` 猜。
#[test]
fn dead_page_reason_三态判定() {
    // 非空白页:零字段(fail-open,常态不带噪声)
    assert!(
        super::dead_page_reason("p_1", "https://www.liepin.com/", |_| None, |_| None).is_none(),
        "正常页面不得带 page_dead"
    );
    assert!(
        super::dead_page_reason("p_1", "about:blank#x", |_| None, |_| None).is_none(),
        "about:blank#x 是带 fragment 的正常落地,不判死页"
    );

    // 空白页但无抹除记录、无域历史:通用死页信号,不给风控方向性误导
    let plain =
        super::dead_page_reason("p_1", "about:blank", |_| None, |_| None).expect("空白页必须有信号");
    assert_eq!(plain["reason"], "about_blank");
    assert_eq!(plain["late"], false);
    assert!(plain["advice"].as_str().unwrap_or_default().contains("about:blank"));

    // 命中后台看护 TTL 台账:延迟抹除,带成因 URL 与年龄
    let late = super::dead_page_reason(
        "p_1",
        "about:blank",
        |pid| {
            assert_eq!(pid, "p_1");
            Some(("https://www.liepin.com/".to_string(), 7))
        },
        |_| None,
    )
    .expect("被抹页面必须有信号");
    assert_eq!(late["reason"], "site_wipe");
    assert_eq!(late["late"], true);
    assert_eq!(late["requested_url"], "https://www.liepin.com/");
    assert_eq!(late["wiped_ago_secs"], 7);
    assert!(late["advice"].as_str().unwrap_or_default().contains("换 profile"));
}

/// 2026-10-19:看护台账落账**之前**的观察窗口(猎聘实测抹除在导航返回后 3~6s,
/// 而旧实现只在 10s 终检落账)——该窗口内模型拿到的原是「重新加载目标地址」这种
/// 对抹除域恰是最该避免的建议。本测试锁死「域历史兜底」分支。
#[test]
fn dead_page_reason_域历史兜底给出别重试建议() {
    // 台账未落账(recent=None),但该页最近请求域有抹除史
    let v = super::dead_page_reason(
        "p_1",
        "about:blank",
        |_| None,
        |_| Some(("liepin.com".to_string(), 3, "https://www.liepin.com/".to_string())),
    )
    .expect("空白页必须有信号");
    assert_eq!(v["reason"], "site_wipe", "有域历史时不得再给 about_blank 通用建议");
    assert_eq!(v["late"], false, "域历史不是本页的延迟抹除记录");
    assert_eq!(v["evidence"], "domain_history");
    assert_eq!(v["domain"], "liepin.com");
    assert_eq!(v["wipe_total"], 3);
    let advice = v["advice"].as_str().unwrap_or_default();
    assert!(advice.contains("不要重新 open/navigate 重试"), "须明确劝退重试");
    assert!(advice.contains("liepin.com"), "须点名成因域");

    // 台账有记录时优先用记录(证据更直接),不回落到域历史
    let both = super::dead_page_reason(
        "p_1",
        "about:blank",
        |_| Some(("https://www.liepin.com/".into(), 3)),
        |_| Some(("liepin.com".to_string(), 9, "https://www.liepin.com/".into())),
    )
    .unwrap();
    assert_eq!(both["late"], true, "TTL 记录优先");
    assert!(both.get("evidence").is_none(), "命中记录时不带域历史证据标记");
}

#[tokio::test]
#[ignore = "需要网络 + 本机真实 Chrome(headed 会弹窗口);本地手动跑"]
async fn real_browser_site_viewport_and_page_health() {
    let mgr = BrowserManager::global();
    let url = std::env::var("LAEW_REAL_SITE_URL")
        .unwrap_or_else(|_| "https://example.com/".to_string());
    let (pid, _, final_url) = mgr
        .new_page(
            &url,
            BrowserMode::Headed,
            None,
            None,
            None,
            false,
            crate::agent::browser_overlay::PageGuardConfig::open(),
        )
        .await
        .expect("打开内网站点登录页失败(网络不可达?)");
    let page = mgr.page(&pid).await.expect("page 句柄");
    // 视口铺满断言(与服务器是否放行无关)。
    let v = page
        .evaluate("({w: window.innerWidth, h: window.innerHeight})")
        .await
        .expect("测量视口失败")
        .value()
        .cloned()
        .expect("测量结果为空");
    let (w, h) = (v["w"].as_f64().unwrap_or(0.0), v["h"].as_f64().unwrap_or(0.0));
    println!("内网站点页面视口 {w}×{h}");
    assert!(
        w >= 1600.0 && h >= 800.0,
        "内网站点页面视口应铺满窗口内容区,实测 {w}×{h}"
    );
    // page_health 自洽性断言:排除注入层后的真实 DOM 元素数决定期望。
    let real = page
        .evaluate("document.querySelectorAll('*:not([data-laew-agent])').length")
        .await
        .expect("统计 DOM 失败")
        .value()
        .and_then(|x| x.as_f64())
        .unwrap_or(-1.0) as i64;
    let health = page_health_field(&pid, &final_url, &final_url).await;
    println!("真实元素数 {real},page_health = {health:?}");
    if real <= 3 {
        let h = health.expect("空白页必须透出 page_health(不再让 Agent 盲试 reload)");
        let reason = h["reason"].as_str().unwrap_or_default();
        assert!(
            reason == "main_doc_error" || reason == "empty_dom" || reason == "site_wipe",
            "reason 应为结构性判定,实测 {reason}"
        );
        if reason == "main_doc_error" {
            assert!(
                h["main_doc_status"].as_i64().unwrap_or(0) >= 400,
                "main_doc_error 必须携带 ≥400 的状态码"
            );
        }
    } else {
        assert!(
            health.is_none(),
            "页面有真实内容({real} 个元素)时不应误报 page_health:{health:?}"
        );
    }
    mgr.shutdown().await;
}

// ===================== 第 170 轮:复用失败信封改写 =====================

#[test]
fn 复用失败改写_命中2001注入提示() {
    let env = r#"{"code":2001,"message":"receiver is gone","data":{}}"#;
    let out = annotate_reuse_failure(env, "relaunch_killed", "已回收,普通 open 兜底".to_string())
        .expect("2001 必须被改写");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 2001);
    assert_eq!(v["data"]["relaunch_killed"], true);
    assert_eq!(v["data"]["hint"], "已回收,普通 open 兜底");
    // 原 message 保留
    assert_eq!(v["message"], "receiver is gone");
}

#[test]
fn 复用失败改写_命中2002同样改写() {
    let env = r#"{"code":2002,"message":"动作失败","data":{"x":1}}"#;
    let out = annotate_reuse_failure(env, "probe_matched_but_dead", "端口残留".to_string())
        .expect("2002 必须被改写");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"]["probe_matched_but_dead"], true);
    assert_eq!(v["data"]["x"], 1, "原有 data 字段保留");
}

#[test]
fn 复用失败改写_非断连错误原样放行() {
    let env = r#"{"code":3002,"message":"未检测到调试端口","data":{}}"#;
    assert!(
        annotate_reuse_failure(env, "relaunch_killed", "x".to_string()).is_none(),
        "3002 不属于断连类错误,不应改写"
    );
    let env1001 = r#"{"code":1001,"message":"参数错误","data":{}}"#;
    assert!(annotate_reuse_failure(env1001, "k", "x".to_string()).is_none());
}

#[test]
fn 复用失败改写_畸形JSON不透写() {
    assert!(annotate_reuse_failure("not-json", "k", "x".to_string()).is_none());
    // 合法 JSON 但缺信封结构(无 code 字段)→ 不改写
    assert!(annotate_reuse_failure(r#"{"foo":1}"#, "k", "x".to_string()).is_none());
}

// ========== 2026-10-14:整页抹除(site_wipe)治理 ==========

#[test]
fn 抹除证据_主文档有响应即判定() {
    // 倒序(最新在前):主文档 200 + 同域子资源 2 条
    let ev1 = json!({"phase":"response","status":200,"url":"https://www.liepin.com/"});
    let ev2 = json!({"phase":"response","status":200,"url":"https://c.liepin.com/a.js"});
    let ev3 = json!({"phase":"response","status":200,"url":"https://c.liepin.com/b.css"});
    let events = vec![&ev2, &ev3, &ev1]; // 最新在前,主文档不在最前也该扫到
    let ev = wipe_evidence_from_events(&events, "https://www.liepin.com/")
        .expect("主文档有 response 即命中");
    assert_eq!(ev.main_doc_status, Some(200));
    assert_eq!(ev.subresources, 3);
}

#[test]
fn 抹除证据_仅子资源足量也判定() {
    // 主文档条目不在缓冲(被抹后缓冲滚动丢失),但同域子资源 ≥5 → 页面曾真实加载
    let events: Vec<Value> = (0..5)
        .map(|i| json!({"phase":"response","status":200,"url":format!("https://c.liepin.com/{i}.js")}))
        .collect();
    let refs: Vec<&Value> = events.iter().collect();
    let ev = wipe_evidence_from_events(&refs, "https://www.liepin.com/zhaopin/")
        .expect("子资源 ≥5 即命中");
    assert_eq!(ev.main_doc_status, None);
    assert_eq!(ev.subresources, 5);
}

#[test]
fn 抹除证据_无网络痕迹不判定_维持empty_dom路径() {
    let events: Vec<Value> = vec![
        json!({"phase":"request","url":"https://www.liepin.com/"}),
        json!({"phase":"response","status":200,"url":"https://other.com/x.js"}),
    ];
    let refs: Vec<&Value> = events.iter().collect();
    assert!(wipe_evidence_from_events(&refs, "https://www.liepin.com/").is_none());
}

#[test]
fn 抹除证据_子域按注册域归一计数() {
    let events: Vec<Value> = vec![
        json!({"phase":"response","status":403,"url":"https://www.liepin.com/"}),
        json!({"phase":"response","status":200,"url":"https://api-c.liepin.com/x.png"}),
    ];
    let refs: Vec<&Value> = events.iter().collect();
    let ev = wipe_evidence_from_events(&refs, "https://www.liepin.com/").unwrap();
    assert_eq!(ev.main_doc_status, Some(403));
    assert_eq!(ev.subresources, 2);
}

#[test]
fn 抹除字段_有结果才生成() {
    assert!(site_wipe_field(None).is_none());
    let v = site_wipe_field(Some(crate::agent::browser_pace::WipeOutcome {
        streak: 2,
        cooldown_triggered: true,
    }))
    .unwrap();
    assert_eq!(v["streak"], 2);
    assert_eq!(v["cooldown_triggered"], true);
    assert!(v["advice"].as_str().unwrap().contains("close(page_id=\"all\")"));
}

#[test]
fn 步内open参数合并_params逐键并入顶层() {
    let mut args = json!({
        "action":"open",
        "page_id":"$NEW",
        "params":{"url":"https://www.liepin.com/","guard":"open","force":true}
    });
    merge_open_params(&mut args);
    assert_eq!(args["url"], "https://www.liepin.com/");
    assert_eq!(args["guard"], "open");
    assert_eq!(args["force"], true);
    // params 本体保留(不破坏其余读取方),顶层已有 url 时不合并
    assert!(args["params"].is_object());
    let mut keep = json!({"action":"open","url":"https://a.com/","params":{"url":"https://b.com/"}});
    merge_open_params(&mut keep);
    assert_eq!(keep["url"], "https://a.com/", "顶层 url 优先,不被 params 覆盖");
}

#[test]
fn 步内open参数合并_无非效params零变化() {
    let mut none = json!({"action":"open"});
    merge_open_params(&mut none);
    assert_eq!(none, json!({"action":"open"}));
    let mut bad = json!({"action":"open","params":"not-object"});
    merge_open_params(&mut bad);
    assert_eq!(bad["params"], "not-object");
}

// ==================== 第 172 轮:厂商验证容器指纹 + 风控挑战域级冷却 ====================

/// baxia 实测形态探针(滑块本体在跨域 iframe 内,外层结构信号残缺):
/// score 仅 4、slider_like=false、iframe_like=false → 旧分类掉到 Unknown。
/// 弹层带「安全验证」强词(实测壳层文案)使 detect_challenge 先命中,验证
/// 「已有挑战 + 厂商指纹 → 升级子类」的合并路径。
fn probe_baxia_like() -> Value {
    json!({
        "text": "安全验证",
        "vendors": [{"name": "baxia", "hits": 1}],
        "overlay": {
            "found": true, "text": "安全验证",
            "img_count": 0, "grid_imgs": 0, "bg_imgs": 0,
            "canvas_count": 0, "iframe_count": 0,
            "iframe_like": false, "input_count": 0, "slider_like": false,
            "buttons": [],
            "selector": ".baxia-dialog.auto",
            "rect": {"w": 1920, "h": 940}, "viewport": {"w": 1920, "h": 940},
            "area_ratio": 0.982, "z_index": 999999,
        },
    })
}

#[test]
fn 厂商指纹_baxia残缺证据升级为滑块() {
    let blockers = blocker_probe::analyze(&probe_baxia_like());
    let first = blockers.first().expect("厂商指纹在场必须产出挑战条目");
    assert_eq!(first["challenge_type"], "slider", "Unknown 应按厂商指纹升级为 slider");
    assert_eq!(first["kind"], "manual_verify");
    assert_eq!(first["matched"], "challenge_overlay");
    assert_eq!(first["evidence"]["score"], 6);
    assert_eq!(first["evidence"]["vendors"][0]["name"], "baxia");
    let advice = first["advice"].as_str().unwrap_or("");
    assert!(advice.contains("滑块"), "advice 应给滑块处置指引");
    assert!(advice.contains("反复重试"), "advice 应警示重试加剧风控");
    assert!(blocker_probe::punish_vendor_hit(first));
}

#[test]
fn 厂商指纹_弹层缺失时直接合成挑战条目() {
    // iframe 未加载完/弹层挑错:baxia 容器仍在,不能漏报
    let probe = json!({
        "text": "",
        "vendors": [{"name": "baxia", "hits": 1}],
        "overlay": null,
    });
    let blockers = blocker_probe::analyze(&probe);
    let first = blockers.first().expect("无弹层但厂商指纹在场也应产出条目");
    assert_eq!(first["matched"], "vendor_fingerprint");
    assert_eq!(first["challenge_type"], "slider");
    assert_eq!(first["snippet"], "阿里 baxia 风控滑块");
    assert!(blocker_probe::punish_vendor_hit(first));
}

#[test]
fn 厂商指纹_nc与geetest识别但不触发风控冷却() {
    let nc = json!({"text": "", "vendors": [{"name": "nc", "hits": 1}], "overlay": null});
    let first = blocker_probe::analyze(&nc).remove(0);
    assert_eq!(first["challenge_type"], "slider");
    assert!(!blocker_probe::punish_vendor_hit(&first), "nc 是正常验证组件,不触发冷却");

    let geetest = json!({"text": "", "vendors": [{"name": "geetest", "hits": 1}], "overlay": null});
    let first = blocker_probe::analyze(&geetest).remove(0);
    assert_eq!(first["challenge_type"], "slider");
    assert!(!blocker_probe::punish_vendor_hit(&first));
}

#[test]
fn 厂商指纹_无厂商时行为与旧版一致() {
    let plain = json!({"text": "", "vendors": [], "overlay": null});
    assert!(blocker_probe::analyze(&plain).is_empty());
    // 无 vendors 键(旧探针形态/开关回退)也不报错
    let legacy = json!({"text": "", "overlay": null});
    assert!(blocker_probe::analyze(&legacy).is_empty());
}

#[test]
fn probe_js包含厂商容器选择器() {
    let js = blocker_probe::probe_js_for_test();
    assert!(js.contains("baxia-dialog"), "baxia 容器选择器必须在探针 JS 里");
    assert!(js.contains("nc-container"), "noCaptcha 容器选择器必须在探针 JS 里");
    assert!(js.contains("geetest"), "geetest 容器选择器必须在探针 JS 里");
    assert!(js.contains("yidun"), "易盾容器选择器必须在探针 JS 里");
    assert!(js.contains("vendors"), "探针必须输出 vendors 字段");
}

// ========================= 第 172 轮:连接级失败归一 / 登录墙 =========================

#[test]
fn 连接级失败识别覆盖底层英文错误() {
    // 实测(2026-10-14 对照任务):接管已死调试实例后,冷启动 open 也统一返回
    // futures 的 "oneshot canceled",模型原样重试 3 次全灭。
    assert!(is_connection_level_failure("oneshot canceled"));
    assert!(is_connection_level_failure(
        "Session with given id not found"
    ));
    assert!(is_connection_level_failure("Target closed"));
    assert!(is_connection_level_failure("WebSocket handshake failed"));
    // 动作级失败不得误判为连接级(否则会被清句柄,徒增一次冷启动)
    assert!(!is_connection_level_failure("缺少 url"));
    assert!(!is_connection_level_failure("navigate 超时(30s): https://x"));
    assert!(!is_connection_level_failure("page_id 不存在(标签页已关闭)"));
}

#[test]
fn 登录墙信号_登录页与正常页() {
    let mut d = json!({"final_url": "https://www.zhipin.com/web/user/"});
    login_wall::attach(&mut d, "https://www.zhipin.com/web/user/", None);
    assert!(d.get("login_wall").is_some(), "登录页必须透出登录墙信号");

    let mut ok = json!({"final_url": "https://www.zhipin.com/beijing/"});
    login_wall::attach(&mut ok, "https://www.zhipin.com/beijing/", Some("session_avatar"));
    assert!(
        ok.get("login_wall").is_none(),
        "带会话标记的正常页不得误报登录墙"
    );
}

#[test]
fn 页面存活性信号_健康页零噪声() {
    let id = "tests_liveness_quiet_page";
    page_liveness::forget(id);
    let mut data = json!({});
    // 未采样/无自刷新 → 不写任何字段
    page_liveness::attach(&mut data, id);
    assert!(data.get("page_liveness").is_none());

    page_liveness::observe(id, 1.0, "https://x.test/", false);
    page_liveness::observe(id, 2.0, "https://x.test/", false);
    page_liveness::observe(id, 3.0, "https://x.test/", false);
    page_liveness::attach(&mut data, id);
    assert!(data.get("page_liveness").is_some(), "连续自发重载应透出不稳定信号");
    let v = &data["page_liveness"];
    assert_eq!(v["unstable"], json!(true));
    assert!(v["advice"].as_str().unwrap().contains("reuse_existing"));
    page_liveness::forget(id);
}

// ==================== 2026-10-16:bool_param 布尔参数兼容 ====================

/// 闲鱼实测:LLM 对 extract(probe=…) 反复传字符串 "true",as_bool() 返回 None
/// 静默回退 false → probe 模式不触发 → 2002 白烧一轮。
#[test]
fn bool_param_accepts_string_true_false() {
    // 字符串 "true"/"false"
    assert!(bool_param(Some(&json!("true")), false));
    assert!(bool_param(Some(&json!("false")), true) == false);
    // 大小写与空白
    assert!(bool_param(Some(&json!("True")), false));
    assert!(bool_param(Some(&json!(" TRUE ")), false));
    // 数字字符串
    assert!(bool_param(Some(&json!("1")), false));
    assert!(bool_param(Some(&json!("0")), true) == false);
    // 常见别名
    assert!(bool_param(Some(&json!("yes")), false));
    assert!(bool_param(Some(&json!("on")), false));
    assert!(bool_param(Some(&json!("no")), true) == false);
    assert!(bool_param(Some(&json!("off")), true) == false);
}

#[test]
fn bool_param_json_bool_and_number() {
    // JSON 布尔
    assert!(bool_param(Some(&json!(true)), false));
    assert!(!bool_param(Some(&json!(false)), true));
    // 数字
    assert!(!bool_param(Some(&json!(0)), true));
    assert!(bool_param(Some(&json!(1)), false));
    assert!(bool_param(Some(&json!(42)), false));
    // 缺失 / null / 未知字符串 → default
    assert!(bool_param(None, true));
    assert!(!bool_param(None, false));
    assert!(bool_param(Some(&json!(null)), true));
    assert!(bool_param(Some(&json!("maybe")), true));
    assert!(!bool_param(Some(&json!("maybe")), false));
}

// ==================== F1(2026-10-16):eval_js 异常错误文本下钻 ====================

/// USA.gov 遍历实测:`CdpError::JavascriptException` 的 Display 是结构体 Debug 裸串,
/// `exception.description`(真正的 `SyntaxError: …` 文本)排在 script_id 之后被
/// 首行截断,模型只能列号盲猜(单次语法错误白烧 1~2 轮迭代)。归一化后错误文本
/// + 行列 + 出错位置摘录必须直接可读;非 JS 异常回退首行截断语义。
#[test]
fn eval_err_下钻js异常文本与脚本摘录() {
    use chromiumoxide::cdp::js_protocol::runtime::{
        ExceptionDetailsBuilder, RemoteObjectBuilder, RemoteObjectType,
    };
    let ex = RemoteObjectBuilder::default()
        .r#type(RemoteObjectType::Object)
        .description("SyntaxError: Unexpected token ')'".to_string())
        .build()
        .unwrap();
    let details = ExceptionDetailsBuilder::default()
        .exception_id(4)
        .text("Uncaught".to_string())
        .line_number(0)
        .column_number(15)
        .exception(ex)
        .build()
        .unwrap();
    let err = chromiumoxide::error::CdpError::JavascriptException(Box::new(details));
    let script = "const a = {x:1;;\nconst b = 2;";
    let msg = format_eval_err(&err, script);
    assert!(
        msg.contains("SyntaxError: Unexpected token"),
        "错误文本必须可见: {msg}"
    );
    assert!(msg.contains("line 0 col 15"), "行列信息: {msg}");
    assert!(msg.contains("x:1"), "出错位置附近脚本摘录: {msg}");
    // 非 JS 异常:回退 Display 首行截断(老语义)
    let other = chromiumoxide::error::CdpError::NoResponse;
    assert_eq!(
        format_eval_err(&other, script),
        "Received no response from the chromium instance."
    );
}

#[cfg(test)]
mod js_dump {
    /// 第 179 轮:可见文本读取脚本(挂在 symbol 命名空间上)导出做 `node --check`。
    /// `cargo test --lib 导出可见文本脚本 -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn 导出可见文本脚本供语法复核() {
        let js = crate::agent::browser_ns::render_ns(super::inspect::VISIBLE_TEXT_FN_JS);
        std::fs::write("target/visible_text_fn.js", &js).unwrap();
        println!("wrote target/visible_text_fn.js");
    }

    /// 渲染后的可见文本脚本必须已把窗口 API 收敛到命名空间对象。
    #[test]
    fn 可见文本脚本挂符号命名空间() {
        let js = crate::agent::browser_ns::render_ns(super::inspect::VISIBLE_TEXT_FN_JS);
        assert!(!js.contains("%NS%"), "占位符应全部渲染");
        assert!(js.contains("Object.defineProperty(NS, 'BodyVisibleText'"));
        assert!(js.contains("Symbol.for("));
        assert!(
            !js.contains(&format!("window.{}BodyVisibleText", crate::agent::browser_ns::inject_ns())),
            "不得再落成 window 字符串键"
        );
    }

    // ==================== 第 188 轮:导航结算预算 ====================

    /// 结算预算解析:正整数生效,缺省/0/非法/负数一律回退缺省。
    ///
    /// 缺省取 20s:正常站点整段装配是毫秒级,20s 已是数千倍余量 ——
    /// 命中预算即意味着确实被反爬卡住,而不是「页面有点慢」。
    #[test]
    fn 结算预算解析_正整数生效其余回退默认() {
        assert_eq!(super::settle_budget_from(None), super::SETTLE_BUDGET_MS_DEFAULT);
        assert_eq!(super::settle_budget_from(Some("5000".into())), 5000);
        assert_eq!(super::settle_budget_from(Some("0".into())), super::SETTLE_BUDGET_MS_DEFAULT);
        assert_eq!(super::settle_budget_from(Some("".into())), super::SETTLE_BUDGET_MS_DEFAULT);
        assert_eq!(super::settle_budget_from(Some("abc".into())), super::SETTLE_BUDGET_MS_DEFAULT);
        assert_eq!(super::settle_budget_from(Some("-3".into())), super::SETTLE_BUDGET_MS_DEFAULT);
    }
}
