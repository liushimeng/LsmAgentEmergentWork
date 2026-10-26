//! MCP_Web_Chrome_Use 工具单元测试(不拉起真实 Chrome)。

use super::*;
use crate::agent::tools::Tool;

/// 环境变量互斥:`LAEW_CHROME_USE` 是进程全局量,改开关的用例必须与
/// 读开关的用例串行(否则并行下 4030 污染 1001 断言,实测已翻车)。
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn 工具名与常量一致() {
    let tool = McpChromeUseTool;
    assert_eq!(tool.name(), "MCP_Web_Chrome_Use");
    assert_eq!(MCP_WEB_CHROME_USE_TOOL_NAME, "MCP_Web_Chrome_Use");
}

#[test]
fn schema_含五个基础_action() {
    let tool = McpChromeUseTool;
    let schema = tool.parameters();
    let enums = schema["properties"]["action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(enums, vec!["open", "list", "close", "control", "inspect"]);
    assert!(schema["required"].as_array().unwrap().contains(&json!("action")));
    // control_action 含 input_text 别名(对齐 Lsm 版命名)。
    let c_enums = schema["properties"]["control_action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(c_enums.contains(&"input_text"));
    assert!(c_enums.contains(&"type"));
    // 纯文字按钮 / 真实键盘事件(第 178 轮实测:百度地图「选为终点」无稳定选择器)。
    assert!(c_enums.contains(&"click_text"));
    assert!(c_enums.contains(&"key_press"));
    // params 含 save_path / timeout_ms / text / key / exact / max_chars。
    let props = &schema["properties"]["params"]["properties"];
    assert!(props.get("save_path").is_some());
    assert!(props.get("timeout_ms").is_some());
    assert!(props.get("text").is_some());
    assert!(props.get("key").is_some());
    assert!(props.get("exact").is_some());
    assert!(props.get("max_chars").is_some());
    assert!(props.get("max").is_some());
    // 元素发现原语:模型不再盲猜 CSS 选择器。
    let i_enums = schema["properties"]["info"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(i_enums.contains(&"elements"));
}

#[test]
fn schema_顶层允许动作参数直传() {
    // 运行时 params_of 兼容顶层直传;Schema 不得再以 additionalProperties=false
    // 拒绝 max_chars/max/text(实测模型自然写法会放顶层)。
    let schema = McpChromeUseTool.parameters();
    assert!(schema.get("additionalProperties").is_none());
}

#[test]
fn max_elements_arg_夹取范围与默认值() {
    assert_eq!(max_elements_arg(&json!({})), 80);
    assert_eq!(max_elements_arg(&json!({"max": 0})), 1);
    assert_eq!(max_elements_arg(&json!({"max": 999})), 200);
    assert_eq!(max_elements_arg(&json!({"max": "25"})), 25);
}

#[test]
fn 死页错误识别_receiver_gone() {
    assert!(is_dead_page_error(
        "工具执行失败[chrome_browser]: eval_js 失败: send failed because receiver is gone"
    ));
    assert!(is_dead_page_error("Target closed"));
    assert!(!is_dead_page_error("选择器未命中"));
    assert!(!is_dead_page_error("页面不存在"));
}

#[test]
fn cap_chars_按字符边界截断并保留统计() {
    let (s, chars, truncated) = cap_chars("中文abc".repeat(3000), 1_000);
    assert!(truncated);
    assert_eq!(chars, 15_000);
    assert!(s.chars().count() > 1_000, "截断提示本身也应保留:{s}");
    assert!(s.contains("原始 15000 字符"), "{s}");
    let (s2, chars2, t2) = cap_chars("短文本".to_string(), 20_000);
    assert!(!t2 && chars2 == 3 && s2 == "短文本");
}

#[test]
fn max_chars_arg_夹取范围与默认值() {
    assert_eq!(max_chars_arg(&json!({})), 20_000);
    assert_eq!(max_chars_arg(&json!({"max_chars": 10})), 1_000);
    assert_eq!(max_chars_arg(&json!({"max_chars": 999_999})), 200_000);
    assert_eq!(max_chars_arg(&json!({"max_chars": "40000"})), 40_000);
}

#[test]
fn page_missing_message_识别跨工具_page_id() {
    let m = page_missing_message("p_00000001");
    assert!(m.contains("MCP_Web_Chrome_Use") || m.contains("LsmAgentBrowser"), "m={m}");
    assert!(m.contains("不跨工具"), "m={m}");
    // 本工具自己的未知 id 走通用文案。
    let m2 = page_missing_message("c_00ff00ff");
    assert!(m2.contains("页面不存在"), "m2={m2}");
}

#[test]
fn png_size_解析_IHDR() {
    let mut png = vec![0u8; 24];
    png[0..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    png[12..16].copy_from_slice(b"IHDR");
    png[16..20].copy_from_slice(&1920u32.to_be_bytes());
    png[20..24].copy_from_slice(&1080u32.to_be_bytes());
    assert_eq!(crate::agent::chrome_browser::png_size(&png), Some((1920, 1080)));
    assert_eq!(crate::agent::chrome_browser::png_size(&[0u8; 10]), None);
}

#[tokio::test]
async fn 缺少_action_返_1001() {
    let _g = ENV_LOCK.lock().unwrap();
    let tool = McpChromeUseTool;
    let out = tool.execute(json!({})).await.unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 1001);
}

#[tokio::test]
async fn 关闭开关返回_4030() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::set_var("LAEW_CHROME_USE", "off");
    let tool = McpChromeUseTool;
    let out = tool.execute(json!({"action": "list"})).await.unwrap();
    std::env::remove_var("LAEW_CHROME_USE");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 4030);
}

#[tokio::test]
async fn 未知_action_返_1001() {
    let _g = ENV_LOCK.lock().unwrap();
    let tool = McpChromeUseTool;
    let out = tool.execute(json!({"action": "fly"})).await.unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 1001);
    assert_eq!(v["data"]["action"], "fly");
}

#[tokio::test]
async fn close_未知_page_返_2004() {
    let _g = ENV_LOCK.lock().unwrap();
    let tool = McpChromeUseTool;
    let out = tool
        .execute(json!({"action": "close", "page_id": "c_deadbeef"}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 2004);
}

#[test]
fn params_of_兼容顶层直传() {
    let args = json!({"selector": "#a"});
    assert_eq!(params_of(&args)["selector"], "#a");
    let args2 = json!({"params": {"selector": "#b"}, "selector": "#ignored"});
    assert_eq!(params_of(&args2)["selector"], "#b");
}

#[test]
fn i64_arg_兼容字符串数字() {
    assert_eq!(i64_arg(&json!({"y": 42}), "y"), Some(42));
    assert_eq!(i64_arg(&json!({"y": "-42"}), "y"), Some(-42));
    assert_eq!(i64_arg(&json!({"y": "x"}), "y"), None);
    assert_eq!(i64_arg(&json!({}), "y"), None);
}

#[test]
fn eval_script_arg_别名与_lsm_对齐() {
    // 2026-10-24 实测:模型按 Lsm 习惯传 expression,Chrome 旧实现直接 1001。
    assert_eq!(eval_script_arg(&json!({"expression": "1+1"})), Some("1+1"));
    assert_eq!(eval_script_arg(&json!({"script": "2+2"})), Some("2+2"));
    assert_eq!(eval_script_arg(&json!({"js": "3+3"})), Some("3+3"));
    assert_eq!(eval_script_arg(&json!({"code": "4+4"})), Some("4+4"));
    assert_eq!(eval_script_arg(&json!({"function": "5+5"})), Some("5+5"));
    // script 优先于后置别名。
    assert_eq!(
        eval_script_arg(&json!({"expression": "e", "script": "s"})),
        Some("s")
    );
    // 空串视同未给。
    assert_eq!(eval_script_arg(&json!({"script": "", "expression": "e"})), Some("e"));
    assert_eq!(eval_script_arg(&json!({})), None);
}

#[test]
fn schema_含_request_human_与_eval_js_别名说明() {
    let schema = McpChromeUseTool.parameters();
    let c_enums = schema["properties"]["control_action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    // 2026-10-24 实测:登录墙时模型调 request_human 被枚举拒绝,HITL 断路。
    assert!(c_enums.contains(&"request_human"));
    let script_desc = schema["properties"]["params"]["properties"]["script"]["description"]
        .as_str()
        .unwrap();
    assert!(script_desc.contains("expression"), "script 描述应声明别名: {script_desc}");
}

#[test]
fn human_reasons_白名单包含登录与验证码() {
    use crate::agent::human_assist::HUMAN_REASONS;
    assert!(HUMAN_REASONS.contains(&"login"));
    assert!(HUMAN_REASONS.contains(&"captcha"));
    assert!(HUMAN_REASONS.contains(&"qr_login"));
    assert!(HUMAN_REASONS.contains(&"custom"));
}

/// 2026-10-26 实测补齐:真实鼠标通道动作入枚举(与 Lsm 版对齐)。
#[test]
fn schema_含真实鼠标动作() {
    let tool = McpChromeUseTool;
    let schema = tool.parameters();
    let c_enums = schema["properties"]["control_action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(c_enums.contains(&"click_real"), "{c_enums:?}");
    assert!(c_enums.contains(&"hover"), "{c_enums:?}");
    assert!(c_enums.contains(&"double_click"), "{c_enums:?}");
    assert!(c_enums.contains(&"right_click"), "{c_enums:?}");
}

/// 2026-10-26 实测补齐:整页截图 / 就绪判定 / 真实点击通道 / page_id 双位置。
#[test]
fn schema_含_full_page_与就绪判定与_mode_参数() {
    let schema = McpChromeUseTool.parameters();
    let props = &schema["properties"]["params"]["properties"];
    assert!(props.get("full_page").is_some(), "缺 full_page(整页截图)");
    assert!(props.get("state").is_some(), "缺 state(就绪判定)");
    assert!(props.get("js").is_some(), "缺 js(就绪判定)");
    assert!(props.get("mode").is_some(), "缺 mode(真实点击通道)");
    assert!(props.get("page_id").is_some(), "缺 params.page_id");
    // page_id 不再是硬性必填(唯一存活页时自动选中)。
    let desc = schema["properties"]["page_id"]["description"].as_str().unwrap();
    assert!(desc.contains("省略"), "page_id 描述应说明可省略:{desc}");
}

/// 新能力必须可发现:工具 description 需点明真实鼠标通道与整页截图。
#[test]
fn description_点明真实鼠标与整页截图() {
    let d = McpChromeUseTool.description();
    assert!(d.contains("click_real"), "description 缺真实鼠标通道说明");
    assert!(d.contains("full_page"), "description 缺整页截图说明");
    assert!(d.contains("真实输入通道"), "description 缺真实输入通道说明");
    assert!(d.contains("state"), "description 缺就绪判定 state 说明");
}
