//! 浏览器**控制权状态机**(双浏览器工具共用):`Agent 控制状态` ⇄ `用户控制状态`。
//!
//! 设计见 `docs/双浏览器工具架构/03-浏览器控制权状态机与视口自适应.md`。
//!
//! ## 为什么要有这一层
//!
//! 此前两套浏览器内核都缺一个**任务级**语义:「现在这块页面归谁管」。
//! - Chrome 侧:headed 窗口人工可以直接点,Agent 也在点 —— 谁在控制无人知晓,
//!   状态栏只能如实写「页面开放」(等于放弃了防交叉操作);
//! - Lsm 侧:`set_guard` / `set_overlay` 只记录不生效,人工无法操作页面,
//!   状态栏写「页面已锁定(仅观看)」但**没有任何闸门**。
//!
//! 本模块给出唯一真源:每个 `(引擎, page_id)` 有且只有一条控制权记录,
//! 两个状态**互斥**,切换是原子替换。
//!
//! ## 两条铁律
//!
//! 1. **蒙层在位 ⇔ 控制权在 Agent**:`agent` 态必有蒙层(人工点不动),
//!    `user` 态必无蒙层(人工可自由操作)。
//! 2. **用户控制状态下 Agent 的页面写操作一律拒绝**(错误码 `4004`):
//!    人机抢同一块页面是最难排查的一类故障,从工具层机械拦住,不给模型
//!    「再点一次说不定就成了」的机会。观测类动作(inspect / screenshot / wait)
//!    始终放行。
//!
//! ## 数据面
//!
//! 进程内单例注册表(`Mutex<HashMap<(Engine, String), ControlRecord>>`),
//! 与 `web_activity.rs` 的活动台账**互不干扰**:活动台账是 task_local 的
//! 「谁在干什么」,本注册表是**全局的**「页面归谁管」——控制权必须跨
//! 任务可见(人工在弹窗里点「解除屏蔽」时,Agent 进程内任意路径都要读到)。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// 控制权不在 Agent 时的错误码(与工具层既有错误码体系并列)。
pub const CODE_CONTROL_DENIED: i32 = 4004;

/// 浏览器引擎标识(控制权按引擎分别记账,两引擎的 page_id 空间独立)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Engine {
    /// `MCP_Web_Chrome_Use`(Chrome/CDP 子进程)。
    Chrome,
    /// `MCP_Web_LsmAgentBrowser_Use`(进程内自研内核)。
    Lsm,
}

impl Engine {
    /// 引擎标识字符串(信封回写用)。
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Chrome => "chrome",
            Engine::Lsm => "lsm",
        }
    }
}

/// 控制权状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlState {
    /// **Agent 控制状态**:蒙层在位,人工不可操作页面,Agent 独占写操作权。
    Agent,
    /// **用户控制状态**:蒙层撤除,控制权交还人工,Agent 写操作被机械拒绝。
    User,
}

impl ControlState {
    /// 状态字符串(信封/状态栏回写用)。
    pub fn as_str(self) -> &'static str {
        match self {
            ControlState::Agent => "agent",
            ControlState::User => "user",
        }
    }

    /// 中文名(状态栏/提示词用)。
    pub fn label(self) -> &'static str {
        match self {
            ControlState::Agent => "Agent 控制状态",
            ControlState::User => "用户控制状态",
        }
    }

    /// 解析外部传入的状态字符串(接受中英文别名)。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "agent" | "ai" | "auto" | "locked" | "代理" | "智能体" => Some(ControlState::Agent),
            "user" | "human" | "open" | "用户" | "人工" => Some(ControlState::User),
            _ => None,
        }
    }
}

/// 单条控制权记录。
#[derive(Debug, Clone)]
pub struct ControlRecord {
    /// 当前状态。
    pub state: ControlState,
    /// 自定义提示(状态栏/告知条第三行,≤60 字符)。
    pub note: Option<String>,
    /// 切换原因(`manual` / `hitl` / `open` / `system`,便于诊断与审计)。
    pub reason: String,
    /// 切换时刻(毫秒时间戳)。
    pub since_ms: u128,
}

/// 控制权被拒的详情(工具层直接映射成信封)。
#[derive(Debug, Clone)]
pub struct ControlDenied {
    /// 错误码(恒为 `4004`)。
    pub code: i32,
    /// 面向模型的中文说明。
    pub message: String,
    /// 附加信息(当前状态 / note / 建议动作)。
    pub data: serde_json::Value,
}

fn registry() -> &'static Mutex<HashMap<(Engine, String), ControlRecord>> {
    static REG: OnceLock<Mutex<HashMap<(Engine, String), ControlRecord>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 控制权状态机是否启用(`LAEW_WEB_CONTROL=off|0|false|no` 全关)。
///
/// 关闭后:不记控制权、不挂蒙层、不拦 Agent 写操作 —— 完全回到本轮改造前
/// 的行为(人工可直接操作 Chrome 窗口)。
pub fn enabled() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_CONTROL")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 蒙层是否启用(`LAEW_WEB_MASK=off|0|false|no` 关蒙层,保留闸门与状态栏文案)。
pub fn mask_enabled() -> bool {
    enabled()
        && !matches!(
            std::env::var("LAEW_WEB_MASK")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "off" | "0" | "false" | "no"
        )
}

/// 裁剪 note(≤60 字符,与既有 `guard_note` 约束一致)。
fn clip_note(note: Option<&str>) -> Option<String> {
    let n = note?.trim().to_string();
    if n.is_empty() {
        return None;
    }
    let clipped: String = n.chars().take(60).collect();
    Some(clipped)
}

/// 设置控制权状态(原子替换,两个状态互斥)。
///
/// 返回写入后的记录。`LAEW_WEB_CONTROL=off` 时返回**默认的 agent 态**且不落账。
pub fn set_state(
    engine: Engine,
    page_id: &str,
    state: ControlState,
    note: Option<&str>,
    reason: &str,
) -> ControlRecord {
    let record = ControlRecord {
        state,
        note: clip_note(note),
        reason: reason.to_string(),
        since_ms: crate::agent::web_activity::now_ms(),
    };
    if !enabled() || page_id.is_empty() {
        return record;
    }
    if let Ok(mut map) = registry().lock() {
        map.insert((engine, page_id.to_string()), record.clone());
    }
    record
}

/// 读取控制权记录(无记录 = 默认 `agent` 态)。
pub fn record_of(engine: Engine, page_id: &str) -> Option<ControlRecord> {
    if !enabled() || page_id.is_empty() {
        return None;
    }
    registry()
        .lock()
        .ok()
        .and_then(|m| m.get(&(engine, page_id.to_string())).cloned())
}

/// 读取当前控制权状态(无记录 = `Agent` 控制状态)。
pub fn state_of(engine: Engine, page_id: &str) -> ControlState {
    record_of(engine, page_id)
        .map(|r| r.state)
        .unwrap_or(ControlState::Agent)
}

/// 是否处于**用户控制状态**(人工正在操作该页)。
pub fn is_user_control(engine: Engine, page_id: &str) -> bool {
    state_of(engine, page_id) == ControlState::User
}

/// 清除该页的控制权记录(页面关闭时调用,防泄漏)。
pub fn clear(engine: Engine, page_id: &str) {
    if let Ok(mut map) = registry().lock() {
        map.remove(&(engine, page_id.to_string()));
    }
}

/// 该页是否应挂蒙层(agent 态 + 蒙层开关打开)。
pub fn mask_active(engine: Engine, page_id: &str) -> bool {
    mask_enabled() && state_of(engine, page_id) == ControlState::Agent
}

/// 观测类动作(任何控制权状态下都放行)。
const READ_ACTIONS: &[&str] = &[
    "screenshot",
    "wait",
    "inspect",
    "heartbeat",
    "ping",
    "list",
    "get",
];

/// 控制权动作(本身就是「拿回控制权」的手段,任何状态下都放行)。
const CONTROL_ACTIONS: &[&str] = &["set_control_state", "set_guard", "set_overlay"];

/// 判断某动作在当前控制权状态下是否应被拒绝。
///
/// - `agent` 态:一律放行;
/// - `user` 态:仅放行观测类动作与控制权动作,其余(会改变页面的动作)拒绝。
pub fn action_is_write(action: &str) -> bool {
    let a = action.trim();
    if READ_ACTIONS.contains(&a) || CONTROL_ACTIONS.contains(&a) {
        return false;
    }
    true
}

/// Agent 写操作闸:在 `user` 态下机械拒绝会改变页面的动作。
///
/// 返回 `Ok(())` 表示放行;`Err(ControlDenied)` 表示拒绝并附带可执行的
/// 提示(告诉模型「先把控制权要回来」,而不是让它反复重试)。
pub fn guard_agent_write(
    engine: Engine,
    page_id: &str,
    action: &str,
) -> Result<(), ControlDenied> {
    if !enabled() || !is_user_control(engine, page_id) {
        return Ok(());
    }
    if !action_is_write(action) {
        return Ok(());
    }
    let rec = record_of(engine, page_id);
    let note = rec.as_ref().and_then(|r| r.note.clone());
    let mut data = serde_json::json!({
        "control_state": ControlState::User.as_str(),
        "control_state_label": ControlState::User.label(),
        "action": action,
        "page_id": page_id,
        "engine": engine.as_str(),
    });
    if let Some(n) = note {
        data["note"] = serde_json::Value::String(n);
    }
    data["hint"] = serde_json::Value::String(
        "当前是用户控制状态(控制权已交还人工),Agent 的页面写操作被拒绝。\
观测请改用 inspect/screenshot/wait(它们不受限);eval_js 也算写操作(它能改页面),同样被拒。\
如需继续自动操作,先用 control(control_action=set_control_state, params={\"state\":\"agent\"}) 取回控制权;\
若人工仍在操作,先等人工完成再取回。"
            .to_string(),
    );
    Err(ControlDenied {
        code: CODE_CONTROL_DENIED,
        message: "控制权当前在用户手上,Agent 不能改变页面(可先取回控制权)".to_string(),
        data,
    })
}

/// HITL 辅助:进入用户控制状态,返回进入前的状态(收口时恢复)。
///
/// 用于 `request_human` —— 提问期间人工要扫码/输码/登录,页面必须交还人工。
pub fn enter_user_control(
    engine: Engine,
    page_id: &str,
    note: Option<&str>,
    reason: &str,
) -> ControlState {
    let prev = state_of(engine, page_id);
    set_state(engine, page_id, ControlState::User, note, reason);
    prev
}

/// HITL 辅助:按进入前的状态恢复控制权。
pub fn restore_control(engine: Engine, page_id: &str, prev: ControlState, reason: &str) {
    set_state(engine, page_id, prev, None, reason);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn reset() {
        if let Ok(mut m) = registry().lock() {
            m.clear();
        }
    }

    #[test]
    fn 默认是agent控制状态() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        assert_eq!(state_of(Engine::Chrome, "c_1"), ControlState::Agent);
        assert!(!is_user_control(Engine::Chrome, "c_1"));
    }

    #[test]
    fn 两态互斥切换() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        set_state(Engine::Chrome, "c_x", ControlState::User, None, "manual");
        assert!(is_user_control(Engine::Chrome, "c_x"));
        assert_eq!(record_of(Engine::Chrome, "c_x").unwrap().state, ControlState::User);
        set_state(Engine::Chrome, "c_x", ControlState::Agent, None, "manual");
        assert!(!is_user_control(Engine::Chrome, "c_x"));
        // 记录里不应残留 user
        assert_eq!(record_of(Engine::Chrome, "c_x").unwrap().state, ControlState::Agent);
    }

    #[test]
    fn 两引擎记账互相独立() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        set_state(Engine::Chrome, "p_same", ControlState::User, None, "manual");
        assert!(is_user_control(Engine::Chrome, "p_same"));
        assert!(!is_user_control(Engine::Lsm, "p_same"));
    }

    #[test]
    fn 用户态拦写放行观测() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        set_state(Engine::Lsm, "p_1", ControlState::User, None, "manual");
        assert!(guard_agent_write(Engine::Lsm, "p_1", "click").is_err());
        assert!(guard_agent_write(Engine::Lsm, "p_1", "input_text").is_err());
        assert!(guard_agent_write(Engine::Lsm, "p_1", "navigate").is_err());
        assert!(guard_agent_write(Engine::Lsm, "p_1", "screenshot").is_ok());
        assert!(guard_agent_write(Engine::Lsm, "p_1", "wait").is_ok());
        // 取回控制权的动作本身不被拦
        assert!(guard_agent_write(Engine::Lsm, "p_1", "set_control_state").is_ok());
    }

    #[test]
    fn agent态不拦任何写操作() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        assert!(guard_agent_write(Engine::Chrome, "c_2", "click").is_ok());
    }

    #[test]
    fn 拒绝信封带可执行提示() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        set_state(
            Engine::Chrome,
            "c_3",
            ControlState::User,
            Some("请人工完成登录".into()),
            "hitl",
        );
        let err = guard_agent_write(Engine::Chrome, "c_3", "click").unwrap_err();
        assert_eq!(err.code, CODE_CONTROL_DENIED);
        assert_eq!(err.data["control_state"], serde_json::json!("user"));
        assert_eq!(err.data["note"], serde_json::json!("请人工完成登录"));
        assert!(err.data["hint"].as_str().unwrap().contains("set_control_state"));
    }

    #[test]
    fn note超长裁剪() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        let long = "长".repeat(120);
        let rec = set_state(Engine::Lsm, "p_9", ControlState::User, Some(long.as_str()), "manual");
        assert_eq!(rec.note.as_ref().unwrap().chars().count(), 60);
    }

    #[test]
    fn hitl进出恢复原态() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        set_state(Engine::Chrome, "c_4", ControlState::User, None, "manual");
        let prev = enter_user_control(Engine::Chrome, "c_4", Some("人工扫码".into()), "hitl");
        assert_eq!(prev, ControlState::User);
        assert!(is_user_control(Engine::Chrome, "c_4"));
        restore_control(Engine::Chrome, "c_4", prev, "hitl_end");
        assert!(is_user_control(Engine::Chrome, "c_4"));
    }

    #[test]
    fn 状态字符串解析() {
        assert_eq!(ControlState::parse("agent"), Some(ControlState::Agent));
        assert_eq!(ControlState::parse(" USER "), Some(ControlState::User));
        assert_eq!(ControlState::parse("用户"), Some(ControlState::User));
        assert_eq!(ControlState::parse("locked"), Some(ControlState::Agent));
        assert_eq!(ControlState::parse("nonsense"), None);
    }

    #[test]
    fn clear清记录() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        set_state(Engine::Lsm, "p_c", ControlState::User, None, "manual");
        assert!(is_user_control(Engine::Lsm, "p_c"));
        clear(Engine::Lsm, "p_c");
        assert!(!is_user_control(Engine::Lsm, "p_c"));
    }
}
