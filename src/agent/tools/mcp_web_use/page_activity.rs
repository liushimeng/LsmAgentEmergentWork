//! 多标签页感知与后台挂起治理(设计:`docs/MCP_Web_Use/18-多标签页感知与后台挂起治理.md`)。
//!
//! 背景:Chrome 一个窗口只有一个激活标签页,其余标签页全部转后台挂起 ——
//! 渲染暂停(截图 = 最后一次前台渲染的旧帧/空白)、`requestAnimationFrame` 停摆、
//! IntersectionObserver 不触发(懒加载内容不加载)、定时器被节流。Agent 的页面会
//! 因三种途径不断增多/换前台:`open`/`new_tab` 主动开页、点击/JS `window.open`
//! 派生新页(抢走前台,原页转后台)、人工手动切换标签页(异步发生)。
//! 此前 Agent 对「哪些页在后台」零感知:对后台页 `screenshot` 拿旧图当证据,
//! 也没有「这页还要不要」的收口手段。
//!
//! 本模块是策略层唯一事实源:
//! - [`probe_foreground`]:页面前后台即时探测(**不做缓存** —— 人工切页后下一次
//!   探测立即反映新事实;唯一事实源是页面自己报告的 `document.visibilityState`);
//! - [`gate_visual_action`]:视觉证据类动作(`screenshot`/`inspect(ocr|screenshot)`)
//!   在后台页执行前 **6003 拦截**(`params.focus=true` 自动前台化后执行 /
//!   `params.force=true` 强制);其余动作放行但响应附 `page_background` 提示
//!   (DOM 读写与 CDP 输入在后台页仍正确,只是渲染类能力被节流);
//! - [`tabs_hint`]/[`PAGE_ATTENTION_THRESHOLD`]:页数堆积提醒(`list` 附带);
//! - [`close_background_pages`]:`close(page_id="background")` 批量收口后台页;
//! - [`act_bring_to_front`]:`control(bring_to_front)` 标签页前台化(与元素级
//!   `focus` 区分 —— 那是聚焦页内元素,这是激活浏览器标签页)。
//!
//! 与既有机制正交:页面管控三档(guard)是注入策略,前后台是浏览器事实,互不干扰;
//! `bring_to_front` 不触碰管控状态。闸门顺序:6002(人工核验硬闸)→ 4003(HITL
//! 挂起)→ **6003(本闸)** —— 人工核验与页面所有权优先于前后台事实。
//!
//! 杀开关 `LAEW_WEB_BACKGROUND_GATE=off/0/false/no`:6003 拦截、`page_background`
//! 标注、`list` 的 foreground 探测与 tabs_hint 全部关闭(`bring_to_front` 动作与
//! `close(background)` 是纯能力,保留)。全部探测 fail-open:超时/失败不闸不标,
//! 绝不为一条提示卡住动作。

use serde_json::{json, Value};

/// 后台挂起拦截错误码。
///
/// 归 6xxx「范围/人工核验约束」段(6001 任务锚点越界、6002 人工核验输入硬闸同
/// 语义家族):这不是页面出错(2xxx),而是「页面在后台,视觉证据必然失真」的
/// 硬事实 —— 模型需要的不是换 selector 重试,而是前台化或收口。
pub(super) const CODE_PAGE_BACKGROUND: i32 = 6003;

/// 页数关注阈值:存活页面数达到该值时 `list` 响应附 `tabs_hint` 收口提醒。
pub(super) const PAGE_ATTENTION_THRESHOLD: usize = 6;

/// 前后台探测超时(ms):页面主线程对一次 evaluate 的正常应答是毫秒级;被验证
/// 弹窗卡死时 2s 即弃(fail-open 放行 —— 闸门失守的代价只是少一条提示)。
const PROBE_TIMEOUT_MS: u64 = 2_000;

/// 探测 JS:`visibilityState=="visible"` 即该页是其窗口的当前激活标签页。
/// 多窗口时每窗口各有一个前台页,均为 visible,如实呈现;headless 两档页面对
/// CDP 恒 visible,闸门自然零命中,不添噪声。
const PROBE_JS: &str =
    r#"JSON.stringify({v: document.visibilityState, f: document.hasFocus()})"#;

/// 视觉证据类 control_action:结果依赖页面真实渲染,后台标签页上必然失真。
/// DOM 读写/CDP 输入/滚动等动作在后台页仍正确,不在此列(放行 + 附提示)。
pub(super) fn action_needs_foreground(action: &str) -> bool {
    matches!(action, "screenshot")
}

/// inspect info 中同样依赖真实渲染的维度。
pub(super) fn info_needs_foreground(info: &str) -> bool {
    matches!(info, "screenshot" | "ocr")
}

/// 元操作/自带前台语义:永不闸。`bring_to_front` 是解法本身;`set_guard`/
/// `set_overlay`/`auto_follow`/`heartbeat`/`set_highlight` 等是状态设置;
/// `request_human` 自带窗口置前;`close_tab` 本身就是收口。
pub(super) fn action_bypasses_gate(action: &str) -> bool {
    matches!(
        action,
        "bring_to_front"
            | "set_guard"
            | "set_overlay"
            | "set_highlight"
            | "auto_follow"
            | "heartbeat"
            | "close_tab"
            | "request_human"
    )
}

/// 总开关(默认开启,`LAEW_WEB_BACKGROUND_GATE=off/0/false/no` 关闭)。
pub(super) fn gate_enabled_from_env() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_BACKGROUND_GATE")
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 页面前后台快照(探测结果)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PageActivity {
    pub visibility: String,
    pub focused: bool,
}

impl PageActivity {
    /// 该页是其窗口的当前激活标签页(非后台挂起)。
    pub fn foreground(&self) -> bool {
        self.visibility == "visible"
    }
}

/// 解析探测返回(纯函数,可单测):入参是 `{"v":"visible","f":true}` 形态的
/// JSON 字符串;非法输入返回 None(调用方 fail-open)。
pub(super) fn parse_probe(raw: &Value) -> Option<PageActivity> {
    let s = raw.as_str()?;
    let obj: Value = serde_json::from_str(s).ok()?;
    Some(PageActivity {
        visibility: obj.get("v")?.as_str()?.to_string(),
        focused: obj.get("f")?.as_bool()?,
    })
}

/// 探测单页前后台(即时,无缓存)。超时/失败返回 None(fail-open:不闸、不标)。
pub(super) async fn probe_foreground(page: &chromiumoxide::Page) -> Option<PageActivity> {
    use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
    let params = EvaluateParams::builder()
        .expression(PROBE_JS.to_string())
        .return_by_value(true)
        .build()
        .ok()?;
    let v = tokio::time::timeout(
        std::time::Duration::from_millis(PROBE_TIMEOUT_MS),
        page.evaluate(params),
    )
    .await
    .ok()?
    .ok()?;
    parse_probe(&v.value().cloned().unwrap_or(Value::Null))
}

/// 闸门结论:`Allow` 携带可选的 `data.page_background` 标注(后台提示/前台化
/// 事实),`Block` 携带 6003 信封(动作不执行)。
pub(super) enum GateOutcome {
    Allow(Option<Value>),
    Block(crate::error::Result<String>),
}

/// 后台挂起闸(control 动作与 inspect 视觉维度共用,`action` 传 control_action
/// 或 info kind,两命名空间不冲突)。
///
/// 决策序:开关关闭/元操作/页面取不到/探测失败/已前台 ⇒ 放行不标;
/// 后台 + `params.focus=true` ⇒ 先前台化再放行(标注 brought_to_front);
/// 后台 + 视觉类 + 无 focus/force ⇒ 6003 拦截;
/// 后台 + 非视觉类 ⇒ 放行并附后台提示。
pub(super) async fn gate_visual_action(id: &str, action: &str, params: &Value) -> GateOutcome {
    if !gate_enabled_from_env() || action_bypasses_gate(action) {
        return GateOutcome::Allow(None);
    }
    let page = match super::ensure_page(id).await {
        Ok(p) => p,
        // 页面失效/CDP 断连交给动作自身报错,闸门不越位代答。
        Err(_) => return GateOutcome::Allow(None),
    };
    let Some(act) = probe_foreground(&page).await else {
        return GateOutcome::Allow(None);
    };
    if act.foreground() {
        return GateOutcome::Allow(None);
    }
    // 显式前台化逃生口:先把标签页带到前台再执行本动作。
    if params.get("focus").and_then(Value::as_bool).unwrap_or(false) {
        // 第 166 轮:焦点租约 —— 前台化会抢 OS 键盘焦点,keep 档(缺省)借完
        // 即还(随后的截图/OCR 不需要焦点);响应附 focus_restored 对账。
        let lease = crate::agent::browser_focus::FocusLease::capture();
        let brought = crate::agent::browser::BrowserManager::global()
            .bring_page_to_front(id)
            .await
            .is_ok();
        let focus_restored = lease.release();
        let mut note = background_note(&act, brought.then_some(true), None);
        if let Some(obj) = note.as_object_mut() {
            obj.insert("focus_restored".into(), json!(focus_restored));
        }
        return GateOutcome::Allow(Some(note));
    }
    let needs_fg = action_needs_foreground(action) || info_needs_foreground(action);
    if needs_fg && !params.get("force").and_then(Value::as_bool).unwrap_or(false) {
        tracing::warn!(
            page_id = id,
            action = action,
            "MCP_Web_Use 视觉动作命中后台挂起闸(code=6003)"
        );
        return GateOutcome::Block(blocked_envelope(id, action, &act));
    }
    GateOutcome::Allow(Some(background_note(&act, None, None)))
}

/// 6003 拦截信封(纯函数,data 载荷形状单测锁死)。
fn blocked_envelope(id: &str, action: &str, act: &PageActivity) -> crate::error::Result<String> {
    super::envelope(
        CODE_PAGE_BACKGROUND,
        "目标页面在后台挂起:后台标签页渲染暂停,截图/OCR 拿到的是最后一次前台渲染的旧帧或空白,证据必然失真",
        json!({
            "page_id": id,
            "suspended_action": action,
            "page_background": {
                "visibility": act.visibility,
                "focused": act.focused,
                "note": "Chrome 仅激活标签页真实渲染;后台页 rAF 停摆/懒加载不触发/定时器节流",
            },
            "required_action": "三选一:① 重试本动作并在 params 加 focus=true(自动把该标签页带到前台后执行);② 先 control(bring_to_front) 前台化再操作;③ 该页不再需要则 close(page_id) 收口。确认后台截图可用(如已知该页静态)可 params.force=true 强制。",
            "human_note": "切换前台会改变人工当前看到的页面;连续操作同一后台页时先 bring_to_front 一次,不要逐步反复切换",
        }),
    )
}

/// 后台页放行标注(非阻断,挂 `data.page_background`):提示渲染节流风险与对策。
/// `brought_to_front`=经 params.focus 已前台化;`extra` 追加一句场景说明。
pub(super) fn background_note(
    act: &PageActivity,
    brought_to_front: Option<bool>,
    extra: Option<&str>,
) -> Value {
    let mut note = json!({
        "visibility": act.visibility,
        "focused": act.focused,
        "note": "页面在后台:DOM 读写与 CDP 输入仍正确,但渲染/rAF/懒加载可能被 Chrome 节流;若等待超时、内容不更新或截图疑似旧帧,先 control(bring_to_front) 或重试时加 params.focus=true;该页任务完成请 close(page_id) 收口",
    });
    if let Some(b) = brought_to_front {
        note["brought_to_front"] = json!(b);
    }
    if let Some(e) = extra {
        note["note"] = json!(format!("{};{e}", note["note"].as_str().unwrap_or_default()));
    }
    note
}

/// 派生新标签页抢走前台后,给原页动作响应附一条后台标注(点击/JS 开新页的
/// 即时感知;探测失败/原页仍前台/开关关闭均返回 None,零噪声)。
pub(super) async fn note_if_backgrounded(id: &str) -> Option<Value> {
    if !gate_enabled_from_env() {
        return None;
    }
    let page = crate::agent::browser::BrowserManager::global().page(id).await?;
    let act = probe_foreground(&page).await?;
    if act.foreground() {
        return None;
    }
    Some(background_note(
        &act,
        None,
        Some("新派生标签页已接管前台,本页转入后台;后续操作本页先 bring_to_front"),
    ))
}

/// 标签页堆积提示(纯函数):存活页数达到阈值时提醒收口,否则 None。
pub(super) fn tabs_hint(count: usize) -> Option<Value> {
    if count < PAGE_ATTENTION_THRESHOLD {
        return None;
    }
    Some(json!({
        "pages": count,
        "hint": "打开页面较多:后台标签页会被 Chrome 挂起(截图/OCR 返回 6003);已完成的页面请 close(page_id) 及时收口,只需保留前台页时可 close(page_id=\"background\") 批量清理,全部结束用 close(page_id=\"all\")",
    }))
}

/// `close(background)` 安全阀(纯函数):一个前台页都没探测到(窗口全最小化/
/// 探测全失败)时拒绝批量关闭,防止一锅端误杀全部页面。
fn should_refuse_bulk_close(foreground_seen: usize) -> bool {
    foreground_seen == 0
}

/// 批量关闭全部后台页(`close(page_id="background")`)。
///
/// 经 BrowserManager 公共 API 组合(`list_pages`/`page`/`close_page`),
/// `browser.rs` 零改动。探测失败的页**不可判定,跳过不关**(fail-safe);
/// 无任何前台页时整体拒绝(见 [`should_refuse_bulk_close`])。
/// 逐页 `close_page`:最后一页关闭时自动回收 launch 模式浏览器,与逐个
/// close 语义一致(但本函数保证至少保留一个前台页,正常不会触底)。
pub(super) async fn close_background_pages() -> std::result::Result<Vec<String>, String> {
    let mgr = crate::agent::browser::BrowserManager::global();
    let entries = mgr.list_pages().await;
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let mut foreground_seen = 0usize;
    let mut to_close = Vec::new();
    for (id, _url, _title, _ts) in &entries {
        let activity = match mgr.page(id).await {
            Some(p) => probe_foreground(&p).await,
            None => None,
        };
        match activity {
            Some(act) if act.foreground() => foreground_seen += 1,
            Some(_) => to_close.push(id.clone()),
            None => {}
        }
    }
    if should_refuse_bulk_close(foreground_seen) {
        return Err(
            "未探测到任何前台页面(浏览器窗口可能被最小化/页面主线程无响应),\
             拒绝批量关闭以免误杀;请逐页 close(page_id) 指定关闭"
                .into(),
        );
    }
    let mut closed = Vec::new();
    for id in to_close {
        if mgr.close_page(&id).await {
            closed.push(id);
        }
    }
    if !closed.is_empty() {
        tracing::info!(count = closed.len(), "MCP_Web_Use 批量关闭后台标签页");
    }
    Ok(closed)
}

/// `control(bring_to_front)`:把指定标签页带到前台(`Page.bringToFront`)并
/// 复核可见性。与元素级 `focus` 区分:那是聚焦页内输入元素,这是激活标签页。
///
/// 第 166 轮:焦点租约包壳 —— `Page.bringToFront` 会连带激活窗口抢键盘焦点,
/// keep 档(缺省)在本次动作收口归还给调用前的前台应用(响应附
/// `focus_restored` 对账);错误路径同样归还,不泄漏借焦。
pub(super) async fn act_bring_to_front(id: &str) -> std::result::Result<Value, String> {
    let lease = crate::agent::browser_focus::FocusLease::capture();
    let result = bring_to_front_core(id).await;
    let focus_restored = lease.release();
    result.map(|mut v| {
        if let Some(obj) = v.as_object_mut() {
            obj.insert("focus_restored".to_string(), json!(focus_restored));
        }
        v
    })
}

async fn bring_to_front_core(id: &str) -> std::result::Result<Value, String> {
    crate::agent::browser::BrowserManager::global()
        .bring_page_to_front(id)
        .await?;
    // bringToFront 生效有短暂延迟,给 120ms 缓冲后复核(fail-open:复核失败
    // 不影响成功结论,只是 visibility_after 为 null)。
    let page = super::ensure_page(id).await?;
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    let after = probe_foreground(&page).await;
    Ok(json!({
        "brought_to_front": true,
        "visibility_after": after.as_ref().map(|a| a.visibility.clone()),
        "foreground": after.as_ref().map(|a| a.foreground()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量类测试互斥(browser_overlay.rs 同款,防并行 set_var 互踩)。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn 视觉证据类动作分类() {
        for a in ["screenshot"] {
            assert!(action_needs_foreground(a), "{a} 应需前台");
        }
        // DOM/输入/导航/等待类在后台页仍正确,不闸
        for a in [
            "click", "input_text", "key_press", "scroll", "wait", "eval_js",
            "navigate", "reload", "download", "drag", "set_cookie",
        ] {
            assert!(!action_needs_foreground(a), "{a} 不应需前台");
        }
    }

    #[test]
    fn inspect_视觉维度分类() {
        for k in ["screenshot", "ocr"] {
            assert!(info_needs_foreground(k), "{k} 应需前台");
        }
        for k in ["elements", "dom", "console", "network", "page_state", "blockers", "coverage"] {
            assert!(!info_needs_foreground(k), "{k} 不应需前台");
        }
    }

    #[test]
    fn 元操作永不闸() {
        for a in [
            "bring_to_front", "set_guard", "set_overlay", "set_highlight",
            "auto_follow", "heartbeat", "close_tab", "request_human",
        ] {
            assert!(action_bypasses_gate(a), "{a} 应绕过闸门");
        }
        assert!(!action_bypasses_gate("screenshot"));
        assert!(!action_bypasses_gate("click"));
    }

    #[test]
    fn 探测结果解析() {
        let visible = parse_probe(&Value::String(
            r#"{"v":"visible","f":true}"#.into(),
        ))
        .expect("合法输入应解析成功");
        assert_eq!(visible.visibility, "visible");
        assert!(visible.focused);
        assert!(visible.foreground());

        let hidden = parse_probe(&Value::String(
            r#"{"v":"hidden","f":false}"#.into(),
        ))
        .expect("合法输入应解析成功");
        assert!(!hidden.foreground());

        // 非法输入:非字符串 / 非 JSON / 缺字段 ⇒ None(fail-open)
        assert!(parse_probe(&json!({"v": "visible"})).is_none());
        assert!(parse_probe(&Value::String("not json".into())).is_none());
        assert!(parse_probe(&Value::String(r#"{"v":"hidden"}"#.into())).is_none());
        assert!(parse_probe(&Value::Null).is_none());
    }

    #[test]
    fn 杀开关解析() {
        let _guard = ENV_LOCK.lock().unwrap();
        for off in ["off", "0", "false", "no", "OFF", " Off "] {
            std::env::set_var("LAEW_WEB_BACKGROUND_GATE", off);
            assert!(!gate_enabled_from_env(), "{off} 应关闭治理");
        }
        std::env::set_var("LAEW_WEB_BACKGROUND_GATE", "1");
        assert!(gate_enabled_from_env());
        std::env::remove_var("LAEW_WEB_BACKGROUND_GATE");
        assert!(gate_enabled_from_env(), "缺省应开启");
    }

    #[test]
    fn 标签页堆积提示阈值() {
        assert!(tabs_hint(0).is_none());
        assert!(tabs_hint(PAGE_ATTENTION_THRESHOLD - 1).is_none());
        let hint = tabs_hint(PAGE_ATTENTION_THRESHOLD).expect("达到阈值应生成提示");
        assert_eq!(
            hint.get("pages").and_then(Value::as_u64),
            Some(PAGE_ATTENTION_THRESHOLD as u64)
        );
        let text = hint.get("hint").and_then(Value::as_str).unwrap_or_default();
        assert!(text.contains("close"), "提示应包含收口对策");
    }

    #[test]
    fn 无前台页时拒绝批量关闭() {
        assert!(should_refuse_bulk_close(0));
        assert!(!should_refuse_bulk_close(1));
        assert!(!should_refuse_bulk_close(3));
    }

    #[test]
    fn 后台标注与拦截载荷形状() {
        let act = PageActivity { visibility: "hidden".into(), focused: false };
        let note = background_note(&act, None, None);
        assert_eq!(note["visibility"], "hidden");
        assert!(note.get("brought_to_front").is_none(), "未前台化时不应带该字段");
        let note = background_note(&act, Some(true), Some("测试追加"));
        assert_eq!(note["brought_to_front"], true);
        assert!(
            note["note"].as_str().unwrap().ends_with("测试追加"),
            "extra 应追加在 note 尾部"
        );

        // 6003 信封是纯函数产物,直接锁 envelope JSON 形状
        let out = blocked_envelope("p_deadbeef", "screenshot", &act).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["code"], CODE_PAGE_BACKGROUND);
        assert_eq!(v["data"]["suspended_action"], "screenshot");
        assert_eq!(v["data"]["page_background"]["visibility"], "hidden");
        assert!(
            v["data"]["required_action"]
                .as_str()
                .unwrap()
                .contains("focus=true"),
            "拦截载荷必须给出 focus=true 对策"
        );
    }
}
