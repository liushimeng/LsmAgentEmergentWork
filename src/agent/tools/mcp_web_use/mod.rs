//! MCP_Web_Use 工具(2026-09-18 第 89 轮):浏览器网页操控的统一 MCP 风格入口。
//!
//! 由原 Chromium-WebUse Agent(第 11 角色)的 5 个独立工具(BrowserNew / BrowserList /
//! BrowserClose / BrowserControl / BrowserInspect)收敛而来:单工具 + `action` 枚举
//! 分发,结构化 JSON 入参/出参,由持有该工具的 Agent(SubAgent-Work)在自身
//! 多轮对话循环中反复调用。
//!
//! - CDP 协议与浏览器进程管理封闭在 `agent::browser` 驱动层(本工具的"MCP 服务"实现,
//!   chromiumoxide,Chrome / Edge / Chromium / Brave,Windows / macOS / Linux),
//!   本模块只做参数校验、action 分发与输出组织;
//! - `page_id` 全生命周期:open 拿 id → control/inspect 多轮复用 → 点击链接/新开标签页
//!   经 `spawned_page_id` 回传新 id → close 释放(最后一个页面关闭时回收浏览器进程);
//! - 所有 action 返回统一 JSON 信封 `{code,message,data}`(0 成功,1001 参数错误,
//!   2000 page_id 失效,2001 断连,2002 动作失败,2003 页面崩溃,3001 未检测到浏览器)
//!   —— Agent 据错误码做机械决策(2000 → action=list 重新同步;2002 → 换 selector/路径重试);
//! - **无平台门控**:CDP 三平台行为一致,未安装浏览器时返回结构化 3001 信封 + 安装引导
//!   (不崩溃),因此全平台注册进 `builtin_registry()` 并同步注入系统提示词使用说明。
//!
//! 设计见 `docs/MCP_Web_Use/01-设计与解决方案.md`(唯一最新版)。
//! 技术参考:`docs/浏览器CDP工具/Rust操作Chrome浏览器CDP完整技术方案.md`。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Tool;
use crate::agent::browser::{BrowserManager, BrowserMode, NO_BROWSER_SENTINEL};
use crate::error::Result;

mod blocker_probe;
mod captcha_crop;
mod control;
mod eval_sanitize;
mod extract;
mod guard_ctl;
mod hitl_guard;
mod hitl_hint;
mod inspect;
mod page_state;
mod unlock_zone;
/// 遍历访问台账(第 146 轮):`pub(crate)` 供编排层(QC 机械足迹)取 seq/footprint。
pub(crate) mod visit_ledger;

#[cfg(test)]
mod tests;

/// 工具名(LLM 可见的唯一浏览器操控入口)。
pub const MCP_WEB_USE_TOOL_NAME: &str = "MCP_Web_Use";

/// 目标站点越界错误码(第 128 轮,任务锚点)。
///
/// 新开 **6xxx「范围约束」段**:1xxx 参数 / 2xxx 浏览器与页面 / 3xxx 环境缺失 /
/// 4xxx 人工介入(「需要人帮忙完成」) / **6xxx 不允许做(「这件事本身越界」)**。
/// 不用 5xxx —— 那一段已被 `Use_MCP`(server 配置与连接)占用,避免跨工具语义混淆。
pub(super) const CODE_TARGET_ANCHOR_VIOLATION: i32 = 6001;

/// 人工核验弹窗输入硬闸错误码(第 151 轮)。
///
/// 归入 6xxx「范围/人工核验约束」段:与 6001(任务锚点越界)同语义家族 ——
/// 「这件事本身不允许做」,不是可换 selector 重试的可恢复错误。
pub(super) const CODE_BLOCKER_INPUT_GATE: i32 = 6002;

// ===================== 共享辅助(自 tools/browser.rs 平移) =====================

pub(super) fn envelope(code: i32, message: &str, data: Value) -> Result<String> {
    Ok(json!({"code": code, "message": message, "data": data}).to_string())
}

/// 任务锚点守卫(第 128 轮):目标 URL 越出用户原文指定的站点时返回 6001 信封。
///
/// 接入点是**显式带 URL 参数的动作**(`open` / `navigate` / `new_tab`)——
/// 这三个是「Agent 主动选择目标站点」的动作,也正是实测漂移的发生点
/// (`news.ycombinator.com` 超时后自行改开 `ithome.com`)。
/// 点击导航的落地页无法预知,不在这里拦;由 `target_drift` 信号 + QC 目标一致性门事后对账。
///
/// `None` = 放行(开关关闭 / 无锚点 / 锚点为空 / host 在锚点内 / host 解析不出来)。
/// 解析不出来时 fail-open:防漂移机制自身绝不能成为新的失败面。
pub(super) fn target_anchor_guard(url: &str) -> Option<Result<String>> {
    let violation = crate::agent::safety::check_open_against_target_anchor(url)?;
    tracing::warn!(
        requested_host = %violation.host,
        allowed_hosts = ?violation.allowed_hosts,
        anchor_evidence = %violation.evidence,
        "MCP_Web_Use 目标站点越界已阻断(任务锚点,code=6001)"
    );
    Some(envelope(
        CODE_TARGET_ANCHOR_VIOLATION,
        "目标站点越界(任务锚点约束)",
        json!({
            "requested_url": url,
            "requested_host": violation.host,
            "allowed_hosts": violation.allowed_hosts,
            "anchor_evidence": violation.evidence,
            "required_action": "如实报告「目标站点不可达或未指定」并结束本单元;\
                                禁止改用其它站点、搜索引擎或缓存替代 —— 可以失败,不可以乱跑。",
        }),
    ))
}

pub(super) fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// 把字符串安全嵌入 JS 字面量(转义 `\`, `'`, 控制字符)。
pub(super) fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

pub(super) fn now_millis_safe() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

// ===================== 第 150 轮:eval 执行超时兜底 =====================

/// eval 默认超时(ms):实测豆包拖拽验证落地后页面主线程被拖住,
/// `Runtime.evaluate` 响应永不返回,裸 await 把整个 sequence 卡死 56s 直到用户
/// Ctrl-C(`tmpPlan/2026-10-10_01-验证弹窗处置闭环与执行超时兜底.md`)。
/// 页面主线程对一次 evaluate 的正常响应是毫秒级,30s 是极宽松上限。
pub(super) const EVAL_DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// 环境变量取值解析(纯函数,可单测):正整数生效,缺省/0/非法回退默认。
pub(super) fn eval_timeout_from(raw: Option<String>) -> u64 {
    raw.and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(EVAL_DEFAULT_TIMEOUT_MS)
}

/// 环境变量覆盖(0/非法回退默认)。
pub(super) fn eval_timeout_from_env() -> u64 {
    eval_timeout_from(std::env::var("LAEW_WEB_EVAL_TIMEOUT_MS").ok())
}

/// 超时错误的统一处置指引(附在错误文案尾部,把模型推回感知-人工闭环)。
pub(super) const EVAL_TIMEOUT_HINT: &str = "页面主线程可能卡死或弹出人工验证;\
建议 inspect(blockers) 或 control(screenshot, params={\"ocr\":true}) 确认页面状态;\
若确认是验证弹窗,立即 request_human(reason=\"captcha\") 请人工完成验证后再继续";

/// 调 JS 并以 returnByValue 提取结果(带超时兜底,第 150 轮)。
pub(super) async fn eval_js_string(
    page: &chromiumoxide::Page,
    js: &str,
) -> std::result::Result<Value, String> {
    use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
    let params = EvaluateParams::builder()
        .expression(js.to_string())
        .return_by_value(true)
        .build()
        .map_err(|e| e.to_string())?;
    // 超时竞争:命中即返回错误(带处置指引),不再无限等 CDP 响应。
    let v = tokio::time::timeout(
        std::time::Duration::from_millis(eval_timeout_from_env()),
        page.evaluate(params),
    )
    .await
    .map_err(|_| format!("eval_js 执行超时({}ms): {EVAL_TIMEOUT_HINT}", eval_timeout_from_env()))?
    .map_err(|e| e.to_string())?;
    Ok(v.value().cloned().unwrap_or(Value::Null))
}

/// 找元素中心点 (x,y,w,h,visible);不存在返回 Err("not_found")。
pub(super) async fn eval_find_center(
    page: &chromiumoxide::Page,
    selector: &str,
    nth: usize,
) -> std::result::Result<Value, String> {
    let sel = js_str(selector);
    let js = format!(
        r#"(() => {{
            const els = document.querySelectorAll({sel});
            const el = els[{nth}];
            if (!el) return null;
            try {{ el.scrollIntoView({{block:'center'}}); }} catch(e) {{}}
            const r = el.getBoundingClientRect();
            return {{x: r.x + r.width/2, y: r.y + r.height/2, w: r.width, h: r.height,
                    visible: r.width>0 && r.height>0}};
        }})()"#,
    );
    eval_js_string(page, &js).await
        .and_then(|v| if v.is_null() { Err("not_found".into()) } else { Ok(v) })
}

pub(super) async fn ensure_page(id: &str) -> std::result::Result<chromiumoxide::Page, String> {
    BrowserManager::global().page(id).await.ok_or_else(|| "page_id 不存在".into())
}

/// 从工具输出文本中提取 page_id(2026-09-16 第 64 轮起持续使用)。
///
/// 仅匹配 action=open 成功时返回的 `data.page_id:"p_xxxxxxxx"`(8 位 hex),
/// 防止误匹配其它字段。失败(code=3001 等)时无 page_id,返回 None。
pub(crate) fn extract_page_id_from_text(text: &str) -> Option<String> {
    const PREFIX: &str = "\"page_id\":\"p_";
    let start = text.find(PREFIX)?;
    let after = &text[start + PREFIX.len()..];
    let end = after
        .find('"')
        .map(|i| start + PREFIX.len() + i + 1)
        .unwrap_or(text.len());
    let pid_start = start + PREFIX.len() - 2; // 含 "p_"
    Some(text[pid_start..end].trim_matches('"').to_string())
}

// ===================== 第 99 轮:open 复用 / close 清场 辅助 =====================

/// URL 归一化(复用匹配用):去尾部 `/` 与 `#fragment` 后精确比较。
pub(super) fn normalize_web_url(u: &str) -> String {
    let mut s = u.trim().to_string();
    if let Some(i) = s.find('#') {
        s.truncate(i);
    }
    while s.ends_with('/') {
        s.pop();
    }
    s
}

/// 提取 `scheme://host[:port]` origin(第 146 轮:origin 级页面复用兜底)。
/// 仅 http/https 返回 Some;chrome://、about:、devtools://、file://、裸串一律 None
/// (这些页面永不参与 origin 兜底,避免把工具页/空白页导航走)。
pub(super) fn url_origin(u: &str) -> Option<String> {
    let parsed = url::Url::parse(u.trim()).ok()?;
    match parsed.scheme() {
        "http" | "https" => Some(parsed.origin().ascii_serialization()),
        _ => None,
    }
}

/// 页面复用匹配级别(响应 `reuse_match` 字段,审计可对账)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReuseMatch {
    /// 第 99 轮行为:URL 精确匹配(归一化后)。
    Exact,
    /// 第 146 轮兜底:同 origin(scheme+host+port)匹配 —— 长遍历任务里存活页
    /// 停在别的路由时,精确匹配落空会新开标签页(实测 14:08 `open(OrganManagement)`
    /// 又开了 p_fdfb9239),兜底改为在最早打开的同站页面上原地导航,保住登录态与
    /// 页面上下文,不再分叉页面句柄。
    Origin,
}

impl ReuseMatch {
    fn as_str(self) -> &'static str {
        match self {
            ReuseMatch::Exact => "exact",
            ReuseMatch::Origin => "origin",
        }
    }
}

/// 在已存活页面中找可复用页面(第 99 轮:同 URL 精确匹配,根治重试/重复 open
/// 导致的页面泄漏;第 146 轮:精确未命中时按同 origin 兜底)。
/// 基于 `list_pages()` 公共 API(自带失效 entry 清理)。
///
/// origin 兜底规则:仅 http(s) 页面参与;多候选取 `created_at` 最早(主工作页,
/// 通常是登录页,登录态最全);`origin_fallback=false`(connect 模式接管用户浏览器)
/// 时禁用 —— 绝不导航用户自己打开的标签页。
async fn find_reusable_page(
    url: &str,
    origin_fallback: bool,
) -> Option<(String, ReuseMatch, String)> {
    let pages = BrowserManager::global().list_pages().await;
    let want = normalize_web_url(url);
    // ① 精确匹配(第 99 轮原行为,优先)
    if let Some((id, u, _, _)) = pages.iter().find(|(_, u, _, _)| normalize_web_url(u) == want) {
        return Some((id.clone(), ReuseMatch::Exact, u.clone()));
    }
    // ② 同 origin 兜底(第 146 轮)
    if !origin_fallback {
        return None;
    }
    let Some(want_origin) = url_origin(url) else {
        return None;
    };
    pages
        .iter()
        .filter(|(_, u, _, _)| url_origin(u) == Some(want_origin.clone()))
        .min_by_key(|(_, _, _, created_at)| created_at.clone())
        .map(|(id, u, _, _)| (id.clone(), ReuseMatch::Origin, u.clone()))
}

/// open 成功响应的四步引导(新建与复用两条路径共用)。
fn open_next_steps() -> Value {
    json!([
        {"step": 1, "action": "control", "control_action": "input_text",
         "selector_hint": "textarea, [contenteditable=true], input[type=text]",
         "tip": "在对话框/输入框输入你的查询文本"},
        {"step": 2, "action": "control", "control_action": "click",
         "selector_hint": "button[type=submit], .submit-btn, [class*=send], [class*=submit], img[class*=button]",
         "tip": "点击提交按钮(图片按钮可用 selector 命中 img 元素)"},
        {"step": 3, "action": "control", "control_action": "wait",
         "selector_hint": "[class*=response], [class*=answer], [class*=result], [class*=message]",
         // AI 类网站长尾响应常见 30-60s(第 82 轮)
         "timeout_ms": 60000,
         "tip": "等待 AI 回复出现,最长等 60 秒"},
        {"step": 4, "action": "inspect", "info": "elements",
         "selector_hint": "[class*=response], [class*=answer], [class*=result]",
         "include_text": true,
         "tip": "提取 AI 回复文本;若 include_text 太短,可改 info=dom 获取 outer_html"},
        {"step": "alt:登录场景", "note": "如果页面是登录表单,推荐流程:inspect(elements)→screenshot(ocr=true读验证码)→sequence(填表+点击+等待)→inspect(验证登录成功)"}
    ])
}

/// 「这一步走不通」的确定性下一步提示(第 131 轮,纯函数,可单测)。
///
/// 背景:人工介入(HITL)长期只写在**系统提示词**里(`MCP_WEB_USE_PROMPT_SECTION`
/// 第 13/16 条),工具响应层从不携带。实测中模型看到 `ocr_error` 后自由发挥 —— 反复
/// 调参 OCR、换 region、猜验证码,而不是去 `request_human`,把迭代预算烧光。
///
/// 本函数把「下一步该做什么」从提示词层下沉到**工具返回层**:凡是判定为
/// 「自动路径已走到尽头」的分支(OCR 不可用 / blockers 命中)都在响应里附
/// `next_action` + `human_assist` 载荷,LLM 不查文档即可照着构造
/// `control_action=request_human` 的参数。
pub(crate) fn human_assist_hint(reason: &str, message: &str, options: &[&str]) -> Value {
    json!({
        "next_action": "request_human",
        "human_assist": {
            "ready": true,
            "reason": reason,
            "message": message,
            "options": options,
            "call": format!(
                "MCP_Web_Use(action=\"control\", control_action=\"request_human\", \
                 params={{\"reason\":\"{reason}\",\"message\":\"…\",\"options\":[…]}})"
            ),
            "rule": "严禁伪造/猜测验证码或跳过人工验证;人工应答后用 data.human_response 继续",
        }
    })
}

/// OCR 结果里是否已带可读文字(纯函数)。
fn ocr_has_text(out: &Value) -> bool {
    out.get("ocr_text")
        .and_then(Value::as_str)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// 「OCR 没能拿到验证码」判定(纯函数,可单测):报错了,或者返回了空文本。
///
/// 空文本与 `ocr_error` 同等对待 —— 验证码区域是纯噪声图时 OCR 常常返回空串,
/// 模型此时最容易误以为「再调一次参数也许能出来」而反复重试。
pub(crate) fn ocr_unavailable_hint(out: &Value) -> Option<Value> {
    let failed = out.get("ocr_error").is_some() || !ocr_has_text(out);
    if !failed {
        return None;
    }
    Some(human_assist_hint(
        "captcha",
        "页面验证码 OCR 不可用(或返回空文本),请人工在弹窗中识别并输入验证码后继续",
        &["我已完成验证,继续", "取消任务"],
    ))
}

/// 把提示字段并入响应对象(纯函数;已存在则不覆盖)。
pub(crate) fn merge_hint(out: &mut Value, hint: Value) {
    if let (Some(dst), Some(src)) = (out.as_object_mut(), hint.as_object()) {
        for (k, v) in src {
            dst.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
}

/// 解析 open 的 `window_width`/`window_height`(第 100 轮):
/// 任一缺失返回 None(由驱动层按 mode 给默认);均存在时 clamp 到安全区间
/// (320~7680 / 240~4320,覆盖 1080p/2K/4K 与最小可读尺寸)。
fn parse_window_size(args: &Value) -> Option<(u32, u32)> {
    let w = args.get("window_width").and_then(Value::as_u64)?;
    let h = args.get("window_height").and_then(Value::as_u64)?;
    let w = w.clamp(320, 7680) as u32;
    let h = h.clamp(240, 4320) as u32;
    Some((w, h))
}

/// 第 125 轮:open 是否自动扩展视口(默认开;显式 false 才关)。
/// 页面内容超出视口(横向裁切/可视高度不足)时,导航完成后自动把视口撑到
/// 2K 上限(≤2560×1440),结果回 `data.viewport` 供 LLM 对账。
fn auto_expand_enabled(args: &Value) -> bool {
    args.get("auto_expand_viewport")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// 对页面执行视口自适应(fail-open:测量/扩展失败只记录,不影响 open 成功语义)。
async fn fit_viewport(page_id: &str) -> Option<Value> {
    BrowserManager::global()
        .fit_viewport_to_content(
            page_id,
            crate::agent::browser::VIEWPORT_FIT_MAX_W,
            crate::agent::browser::VIEWPORT_FIT_MAX_H,
        )
        .await
        .ok()
}

/// 第 139 轮:有头模式窗口收边(fail-open;见 `BrowserManager::fit_headed_window`)。
///
/// **必须先于 [`fit_viewport`] 调用** —— 收边走 `Browser.setWindowBounds`,会清除
/// `Emulation.setDeviceMetricsOverride`;晚一步会把刚扩好的视口覆盖抹掉。
async fn fit_headed_window(page_id: &str) -> Option<Value> {
    BrowserManager::global().fit_headed_window(page_id).await.ok()
}

/// 缺省浏览器模式(第 139 轮:可见模式)。
///
/// 调用方未显式传 `mode` 时走这里;此前是硬编码 `Hidden`,导致
/// `LAEW_BROWSER_MODE` 开关形同虚设、「是否弹窗」全看 LLM 这轮传不传
/// `mode=headed`(实测同一程序在 Terminal 不弹窗、VS Code 弹窗的根因)。
fn resolve_requested_mode(args: &Value) -> BrowserMode {
    str_arg(args, "mode")
        .and_then(|v| BrowserMode::from_arg(&v))
        .unwrap_or_else(BrowserMode::from_env_or_default)
}

// ===================== 第 143 轮:页面管控三档(guard) =====================

/// 从 open 参数提取字符串数组(`allow_selectors` / `block_selectors`)。
fn str_array_arg(args: &Value, key: &str) -> Option<Vec<String>> {
    let arr = args.get(key)?;
    let a = arr.as_array()?;
    Some(
        a.iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
    )
}

/// 解析 open 的页面管控配置(第 143 轮,级联:显式 `guard` > legacy `overlay`
/// 布尔 > 环境变量缺省 `LAEW_WEB_GUARD`/`LAEW_WEB_OVERLAY` > locked)。
///
/// Err = 1001 参数错误(非法档位 / partial 缺选择器 / allow+block 互斥 /
/// locked|open 带选择器 / 超限),由 run_open_inner 直接收口。
fn parse_open_guard(args: &Value) -> std::result::Result<crate::agent::browser_overlay::PageGuardConfig, String> {
    use crate::agent::browser_overlay::{PageGuardConfig, PageGuardMode};
    if let Some(g) = str_arg(args, "guard") {
        let mode = PageGuardMode::from_arg(g)
            .ok_or_else(|| format!("非法 guard 值「{g}」(允许 locked/open/partial)"))?;
        let mut cfg = PageGuardConfig {
            mode,
            allow_selectors: str_array_arg(args, "allow_selectors").unwrap_or_default(),
            block_selectors: str_array_arg(args, "block_selectors").unwrap_or_default(),
            note: str_arg(args, "guard_note").map(str::to_string),
        };
        cfg = cfg.normalized();
        cfg.validate()?;
        return Ok(cfg);
    }
    // 第 141 轮 legacy 布尔语义(true=屏蔽/false=开放)完整保留。
    if let Some(ov) = args.get("overlay").and_then(Value::as_bool) {
        return Ok(PageGuardConfig::legacy_overlay(ov));
    }
    Ok(PageGuardConfig::from_env_default())
}

/// open 成功响应的 guard/overlay 双字段(第 143 轮,纯读取,便于复用路径共用)。
///
/// 读**实例真实期望态**而非本次请求值 —— 单实例复用时 `new_page` 不会用本次参数
/// 覆盖既有实例的管控期望态,回报请求值会与实际不符(与第 141 轮 overlay 字段同理由)。
/// `data.overlay` 为 legacy 派生字段(`enabled ≡ mode==locked`),既有提示词/测试零破坏。
async fn guard_response_fields(effective_mode: BrowserMode, connect_mode: bool) -> (Value, Value) {
    use crate::agent::browser_overlay::PageGuardConfig;
    let mgr = BrowserManager::global();
    let cfg: PageGuardConfig = mgr.current_guard().await;
    let guard_field = json!({
        "mode": cfg.mode.as_str(),
        "allow_selectors": cfg.allow_selectors,
        "block_selectors": cfg.block_selectors,
        "note": cfg.note,
        "hint": "页面管控三档(第 143 轮):locked=屏蔽(缺省,蒙层+输入拦截,人工可看不可点)/ open=非屏蔽(人工可直接操作)/ partial=部分屏蔽(盾区外人工可操作);运行时切换 control(set_guard, mode/allow_selectors/block_selectors/note);缺省档环境变量 LAEW_WEB_GUARD",
    });
    let overlay_field = if connect_mode {
        json!({
            "enabled": false,
            "hint": "接管外部浏览器(connect 模式)默认不锁人工输入(不干扰用户当前使用);\
                     需要时 control(set_guard, mode=\"locked\") 手动开",
        })
    } else if effective_mode.is_headed() {
        let locked = mgr.overlay_active().await;
        json!({
            "enabled": locked,
            "input_locked": locked,
            "visual_mask": locked,
            "hint": "legacy 字段(第 141 轮):enabled ≡ guard.mode==locked;完整三档状态读 data.guard;\
                     运行时切换 control(set_guard) 或 legacy control(set_overlay)",
        })
    } else {
        json!({"enabled": false, "hint": "非可视化模式(hidden/new_headless)无页面管控视觉"})
    };
    (guard_field, overlay_field)
}

// ===================== 第 142 轮:复用已登录浏览器(connect 模式增强) =====================

/// 复用浏览器不可用错误码(第 142 轮):调试端口未开启,无法接管用户已登录浏览器。
///
/// 3xxx 段 = 环境缺失:3001 未安装浏览器 / **3002 不可复用(调试端口未开)**。
/// 确定性失败:Agent 按 data.relaunch_command 引导用户,不要反复重试 open。
pub(super) const CODE_REUSE_UNAVAILABLE: i32 = 3002;

/// open 成功响应注入复用元数据(纯函数,便于单测)。
///
/// 探测接管成功后,在 run_open 响应 JSON 的 **data 对象**内附加
/// reuse_existing_chrome / debug_port / browser_version / auto_relaunch
/// 字段(与 page_id 同级,LLM 从 data 读取);解析失败原样返回。
pub(super) fn inject_reuse_metadata(response: &str, meta: &Value) -> String {
    let Ok(mut v) = serde_json::from_str::<Value>(response) else {
        return response.to_string();
    };
    if let (Some(dst), Some(src)) = (v.get_mut("data").and_then(Value::as_object_mut), meta.as_object())
    {
        for (k, val) in src {
            dst.entry(k.clone()).or_insert_with(|| val.clone());
        }
    }
    v.to_string()
}

/// 复用已登录浏览器入口:自动探测调试端口 → 接管;失败 → 3002 引导 /
/// auto_relaunch 自动重启接管。
///
/// 三档能力与设计见 `docs/MCP_Web_Use/06-复用已登录浏览器会话.md`。
async fn run_open_reuse(args: &Value, url: &str) -> Result<String> {
    let port = crate::agent::browser_reuse::debug_port();
    let auto_relaunch = args
        .get("auto_relaunch")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // 档位一:自动探测接管。
    if let Some(info) = crate::agent::browser_reuse::probe_debug_endpoint(port).await {
        return open_via_connect(
            args,
            url,
            &info.connect_url,
            json!({
                "reuse_existing_chrome": true,
                "debug_port": info.port,
                "browser_version": info.browser_version,
            }),
        )
        .await;
    }
    // 档位三:自动重启接管(显式 opt-in)。第 144 轮:不退出用户 Chrome ——
    // 复制登录态到独立调试 profile 并另启独立实例(与用户浏览器并存)。
    if auto_relaunch {
        return match crate::agent::browser_reuse::relaunch_and_wait(port).await {
            Some(out) => {
                open_via_connect(
                    args,
                    url,
                    &out.info.connect_url,
                    json!({
                        "reuse_existing_chrome": true,
                        "auto_relaunch": true,
                        "debug_port": out.info.port,
                        "browser_version": out.info.browser_version,
                        // 第 144 轮:登录态复制计数对账(0 = 用户 Chrome 未运行过/
                        // 复制全失败,该网站可能表现为未登录,走 request_human 兜底)。
                        "login_state_copied": out.login_state_copied,
                        "login_state_skipped": out.login_state_skipped,
                    }),
                )
                .await
            }
            None => envelope(
                CODE_REUSE_UNAVAILABLE,
                "自动另启调试浏览器后端口仍未就绪",
                json!({
                    "debug_port": port,
                    "chrome_running": crate::agent::browser_reuse::chromium_running(),
                    "relaunch_command": crate::agent::browser_reuse::relaunch_command_string(
                        port,
                        &crate::agent::browser_reuse::debug_profile_dir(),
                    ),
                    "hint": "自动另启失败(浏览器未安装/启动超时/profile 被上次实例锁定);可把 relaunch_command 转述给用户执行后重试;不会退出用户已打开的浏览器",
                }),
            ),
        };
    }
    // 档位二:结构化引导。
    envelope(
        CODE_REUSE_UNAVAILABLE,
        "未检测到可复用的浏览器(调试端口未开启)",
        json!({
            "debug_port": port,
            "chrome_running": crate::agent::browser_reuse::chromium_running(),
            "relaunch_command": crate::agent::browser_reuse::relaunch_command_string(
                port,
                &crate::agent::browser_reuse::debug_profile_dir(),
            ),
            "auto_relaunch": true,
            "hint": "把 relaunch_command 转述给用户执行(该命令以独立调试 profile 另启一个 Chrome 实例,不影响已打开的浏览器);或以 auto_relaunch=true 重试,由工具自动复制登录态并另启调试实例(不退出用户浏览器);Chrome 136+ 默认 profile 上调试端口会被忽略,必须用独立 --user-data-dir",
        }),
    )
}

/// 以 connect_url 走 run_open_inner 既有流程(注入 connect_url),响应附加复用元数据。
async fn open_via_connect(args: &Value, url: &str, connect_url: &str, meta: Value) -> Result<String> {
    let mut args2 = args.clone();
    args2["connect_url"] = json!(connect_url);
    let res = run_open_inner(&args2, url).await?;
    Ok(inject_reuse_metadata(&res, &meta))
}

// ===================== action=open(list/close 同级,轻量内联) =====================

/// 启动/接管浏览器并打开一个页面(原 BrowserNew)。
///
/// 返回 `{page_id,title,final_url,mode,next_steps}`;`next_steps` 是 4 步
/// input_text/click/wait/elements 引导(带 selector_hint),Agent 循环在
/// open 成功后会把 next_steps 注入 LLM 上下文(见 `agent_loop.rs` 引导钩子)。
///
/// 第 99 轮:同 URL 复用 —— 已存在同 URL 存活页面时默认导航刷新并返回同一
/// page_id(`reused:true`),根治「重试/重复 open 泄漏页面」;`reuse:false`
/// 强制新开。
///
/// 第 100 轮:窗口可视化 —— `window_width/window_height`(headed 默认 1920×1080
/// 1080p)、`highlight`(默认 true,蓝色选中边框标识 Agent 控制窗口);浏览器实例
/// 已存在时永远复用同一进程(响应 `browser_reused:true` + 真实 mode),不重复开浏览器。
async fn run_open(args: Value) -> Result<String> {
    let Some(url) = str_arg(&args, "url") else {
        return envelope(1001, "缺少 url", json!({}));
    };
    // 第 128 轮:任务锚点越界阻断 —— 必须先于「同 URL 复用」探测,
    // 否则越界 URL 若恰好命中某个存活页面会被静默复用,阻断形同虚设。
    // 实测漂移正是发生在 open:news.ycombinator.com 超时后自行改开 ithome.com。
    if let Some(blocked) = target_anchor_guard(url) {
        return blocked;
    }
    // 第 142 轮:复用已登录浏览器 —— 显式 connect_url 优先(跳过探测)。
    let reuse_existing = args
        .get("reuse_existing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if reuse_existing && str_arg(&args, "connect_url").is_none() {
        return run_open_reuse(&args, url).await;
    }
    run_open_inner(&args, url).await
}

/// run_open 主体(url 已提取、锚点守卫已过、复用分支已判定)。
///
/// 第 142 轮拆出:复用路径(open_via_connect)直接调本函数,避免
/// run_open ↔ run_open_reuse ↔ open_via_connect 递归 async fn 循环。
async fn run_open_inner(args: &Value, url: &str) -> Result<String> {
    let reuse = args.get("reuse").and_then(Value::as_bool).unwrap_or(true);
    // 第 103 轮:读取 timeout_ms 参数,控制页面加载超时(默认 60s)
    let timeout_ms = args.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(60000);
    // 第 146 轮:connect 模式(接管用户浏览器)禁用 origin 级兜底 ——
    // 不导航用户自己打开的标签页;精确匹配(第 99 轮语义)不受影响。
    let connect_mode = BrowserManager::global().is_connect_mode().await;
    if reuse {
        if let Some((pid, match_kind, previous_url)) =
            find_reusable_page(url, !connect_mode).await
        {
            // 复用路径:在同一页面上导航刷新(新验证码/新会话态),page_id 不变;
            // 导航失败(页面挂死/断连)时落回新建路径。
            if let Some(page) = BrowserManager::global().page(&pid).await {
                let goto = tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    page.goto(url.to_string()),
                )
                .await;
                if let Ok(Ok(_)) = goto {
                    // 第 100 轮:复用路径同样带单实例元数据(浏览器进程级复用 + 真实 mode)
                    let (browser_reused, mode_label) = {
                        let mgr = BrowserManager::global();
                        let m = mgr.current_mode().await;
                        (mgr.has_browser().await, m.unwrap_or(BrowserMode::DEFAULT).as_str())
                    };
                    // 第 143 轮:goto 产生新文档,管控按实例期望态 re-assert 收敛
                    //(CDP 输入拦截实测跨导航持久,管控 JS 按注入时烘焙态重建,此处统一对齐)。
                    if BrowserManager::global().guard_visuals_active().await {
                        let cfg = BrowserManager::global().current_guard().await;
                        let _ = crate::agent::browser_overlay::apply_page_guard(&page, &cfg).await;
                    }
                    // 第 152 轮:goto 产生新文档,自动跟随按实例期望态 re-assert
                    // (人工在 locked 档无法滚动,页面必须自己停在最新)
                    BrowserManager::global().reassert_auto_follow(&page).await;
                    let title = page.get_title().await.ok().flatten().unwrap_or_default();
                    let final_url = page
                        .url()
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| url.to_string());
                    // 第 146 轮:访问台账记账(导航成功后;重复访问警示随响应附带)
                    visit_ledger::record_visit(
                        &pid,
                        &final_url,
                        &title,
                        visit_ledger::VisitSource::Open,
                    );
                    // 第 125 轮:复用路径同样做视口自适应(默认开,auto_expand_viewport=false 关)
                    let mut payload = json!({
                        "page_id": pid,
                        "title": title,
                        "final_url": final_url,
                        "reused": true,
                        // 第 146 轮:复用匹配级别与复用前 URL(exact=URL 精确命中,
                        // origin=同站最早页面兜底命中),供 QC/审计对账页面句柄不分裂。
                        "reuse_match": match_kind.as_str(),
                        "previous_url": previous_url,
                        "browser_reused": browser_reused,
                        "mode": mode_label,
                        "next_steps": open_next_steps(),
                    });
                    if let Some(note) = visit_ledger::visit_note(&final_url) {
                        payload["visit_note"] = note;
                    }
                    // 第 139 轮:人工拖过窗口后,复用路径同样按屏幕工作区收边(有头才动)。
                    if mode_label == BrowserMode::Headed.as_str() {
                        if let Some(v) = fit_headed_window(&pid).await {
                            payload["headed_window"] = v;
                        }
                    }
                    // 第 143 轮:复用路径回报管控状态(guard/overlay 双字段,读实例真实期望态)
                    let (guard_field, overlay_field) = guard_response_fields(
                        BrowserMode::from_arg(&mode_label).unwrap_or(BrowserMode::DEFAULT),
                        BrowserManager::global().is_connect_mode().await,
                    )
                    .await;
                    if mode_label == BrowserMode::Headed.as_str() {
                        payload["guard"] = guard_field;
                        payload["overlay"] = overlay_field;
                    }
                    if auto_expand_enabled(args) {
                        if let Some(v) = fit_viewport(&pid).await {
                            payload["viewport"] = v;
                        }
                    }
                    return envelope(0, "ok", payload);
                }
            }
        }
    }
    // 三档 mode(第 74 轮):缺省可见;第 139 轮起缺省走 `BrowserMode::from_env_or_default`
    // (默认 headed,CI/无 GUI 会话自动回退 hidden),`LAEW_BROWSER_MODE` 真正生效。
    let requested_mode = resolve_requested_mode(&args);
    // 单浏览器实例语义(第 100 轮):实例已存在时永远复用,绝不因 mode/参数差异
    // 新启第二个浏览器进程;请求 mode 与实例不一致时回真实 mode + mode_hint。
    let (browser_reused, effective_mode, mode_hint) =
        match BrowserManager::global().current_mode().await {
            Some(existing) => {
                let hint = if existing != requested_mode {
                    Some(format!(
                        "已复用现有浏览器实例(实际 mode={});如需 {} 可视化,先 close(page_id=\"all\") 回收后以该 mode 重开",
                        existing.as_str(),
                        requested_mode.as_str(),
                    ))
                } else {
                    None
                };
                (true, existing, hint)
            }
            None => (false, requested_mode, None),
        };
    let connect = str_arg(args, "connect_url");
    let ua = str_arg(args, "user_agent");
    // 窗口尺寸(第 125 轮):显式参数 clamp 到安全区间;缺省由驱动层决定
    // (全模式统一 1920×1080 = 1080p)。内容仍超视口时 open 后自动扩展
    // 视口到 2K(auto_expand_viewport,默认开)。
    let window_size = parse_window_size(args);
    let highlight = args.get("highlight").and_then(Value::as_bool).unwrap_or(true);
    // 第 143 轮:页面管控三档(级联:显式 guard > legacy overlay > 环境变量缺省;
    // 仅最终 mode=headed 且非 connect 生效)。参数错误(非法档位/partial 缺选择器/
    // allow+block 互斥等)直接 1001 收口,不静默降级。
    let guard = match parse_open_guard(&args) {
        Ok(g) => g,
        Err(e) => {
            return envelope(
                1001,
                &e,
                json!({
                    "guard": "locked|open|partial",
                    "hint": "partial 需要 allow_selectors(白名单:只有命中区人工可操作)或 block_selectors(黑名单:命中区人工不可操作)之一,两者互斥且各 ≤8 条",
                }),
            )
        }
    };
    match BrowserManager::global()
        .new_page(url, effective_mode, connect, ua, window_size, highlight, guard)
        .await
    {
        Ok((page_id, title, final_url)) => {
            if str_arg(args, "wait_until") == Some("networkidle") {
                // 第 140 轮:兜底 1500ms→700ms —— Chrome 的 networkidle 判定已含
                // 500ms 静默窗,额外 700ms 足够让迟到的渲染提交完成。
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            }
            // 第 142 轮:connect 模式(接管外部浏览器)判定,供 guard/overlay 字段分路提示。
            // (第 146 轮:connect_mode 已在函数开头取过 —— origin 复用兜底也要用)
            // 第 143 轮:管控状态双字段(读实例真实期望态,理由见 guard_response_fields)。
            let (guard_field, overlay_field) =
                guard_response_fields(effective_mode, connect_mode).await;
            let mut data = json!({
                "page_id": page_id,
                "title": title,
                "final_url": final_url,
                "reused": false,
                "mode": effective_mode.as_str(),
                "browser_reused": browser_reused,
                "highlight": highlight && effective_mode.is_headed(),
                // 第 142 轮:connect 模式 = 接管用户外部浏览器,close 只断连不关浏览器。
                "connect_mode": connect_mode,
                // 第 143 轮:页面管控三档状态(locked/open/partial)。
                "guard": guard_field,
                // 第 141 轮 legacy 字段(enabled ≡ guard.mode==locked),派生保留。
                "overlay": overlay_field,
                "window": {
                    "requested": window_size.map(|(w, h)| [w, h]),
                    "default": [1920, 1080],
                    "min_viewport": [1280, 720],
                    "hint": "默认 1080p;小屏自动按屏幕工作区收边保证完整可见,页面视口不低于 720p(见 data.headed_window);内容超视口时已自动扩展视口(见 data.viewport);运行时调整:control(set_window);人工拖动窗口后:control(sync_viewport)",
                },
                // next_steps —— 分步引导,降低 LLM 编排成本(第 74 轮):
                // input_text → click → wait → elements 四步最常见动作链。
                "next_steps": open_next_steps(),
            });
            if let Some(arr) = args.get("block_resources").and_then(Value::as_array) {
                data["block_resources_hint"] = json!(arr);
            }
            if let Some(hint) = mode_hint {
                data["mode_hint"] = json!(hint);
            }
            // 第 139 轮:有头模式窗口收边(必须先于视口自适应 —— setWindowBounds
            // 会清除 device metrics 覆盖,晚一步会把刚扩好的视口抹掉)。
            if effective_mode.is_headed() {
                if let Some(v) = fit_headed_window(&page_id).await {
                    data["headed_window"] = v;
                }
            }
            // 第 125 轮:视口自适应(默认开)——内容宽/高超出视口时自动扩展到
            // 2K 上限,根治「视口太窄页面显示不全、元素不可见不可点」。
            if auto_expand_enabled(args) {
                match fit_viewport(&page_id).await {
                    Some(v) => data["viewport"] = v,
                    None => {
                        data["viewport"] =
                            json!({"expanded": false, "note": "视口测量失败(不影响页面操作);需要时手动 control(set_viewport)"})
                    }
                }
            }
            // 第 146 轮:访问台账记账(新建页面导航成功后;重复访问警示随响应附带)
            visit_ledger::record_visit(
                &page_id,
                &final_url,
                &title,
                visit_ledger::VisitSource::Open,
            );
            if let Some(note) = visit_ledger::visit_note(&final_url) {
                data["visit_note"] = note;
            }
            envelope(0, "ok", data)
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains(NO_BROWSER_SENTINEL) {
                envelope(
                    3001,
                    "未检测到 Chrome/Edge/Chromium",
                    json!({"install":"请安装 Google Chrome 或 Microsoft Edge,或设置 LAEW_BROWSER_PATH"}),
                )
            } else {
                envelope(2001, &msg, json!({}))
            }
        }
    }
}

/// 列出当前存活页面(原 BrowserList);返回前自动清理失效 entry。
async fn run_list(_: Value) -> Result<String> {
    let pages: Vec<Value> = BrowserManager::global()
        .list_pages()
        .await
        .into_iter()
        .map(|(id, url, title, ts)| json!({"page_id":id,"url":url,"title":title,"created_at":ts}))
        .collect();
    envelope(0, "ok", json!({"pages": pages}))
}

/// 关闭指定 page_id(原 BrowserClose);最后一个页面关闭时回收内部浏览器进程
/// (launch 模式)。幂等。
/// 第 99 轮:`page_id="all"` 一键清场(关闭全部页面 + 回收浏览器,任务收尾用)。
async fn run_close(args: Value) -> Result<String> {
    let Some(id) = str_arg(&args, "page_id") else {
        return envelope(1001, "缺少 page_id", json!({}));
    };
    if id == "all" {
        let n = BrowserManager::global().list_pages().await.len();
        BrowserManager::global().shutdown().await;
        return envelope(0, "closed", json!({"page_id": "all", "closed": n}));
    }
    if BrowserManager::global().close_page(id).await {
        envelope(0, "closed", json!({"page_id":id}))
    } else {
        envelope(2000, "page_id 不存在", json!({"page_id":id}))
    }
}

// ===================== action=sequence(连续执行模式) =====================

/// 连续模式单批步骤上限:兼顾长任务编排与单次 tool_result 输出预算。
const MAX_SEQUENCE_STEPS: usize = 24;

/// 单步时长预算(第 150 轮,最后防线):任一步(浏览器启动/下载/eval 等)卡死
/// 不得超过该时长;`request_human` 步豁免(人工介入合法长等待,本身有 30 分钟上限)。
/// 实测事故:eval 无超时 + 验证弹窗卡死主线程 → 整批 56s 无返回直到用户 Ctrl-C。
const STEP_CAP_SECS: u64 = 150;

/// 步骤数据是否携带 blocker_alert(第 150 轮,纯判定可单测):
/// 命中即中断整批 —— 继续执行只会把后续输入灌进验证弹窗后面的黑洞。
pub(super) fn step_has_blocker_alert(data: &Value) -> bool {
    data.get("blocker_alert").is_some()
}

/// 递归替换步骤里的 page_id 占位符。
fn resolve_page_placeholders(value: &mut Value, current: Option<&str>, spawned: Option<&str>) {
    match value {
        Value::String(s) => {
            if let Some(id) = current {
                *s = s.replace("${page_id}", id).replace("$page_id", id);
            }
            if let Some(id) = spawned {
                *s = s
                    .replace("${spawned_page_id}", id)
                    .replace("$spawned_page_id", id);
            }
        }
        Value::Array(items) => {
            for item in items {
                resolve_page_placeholders(item, current, spawned);
            }
        }
        Value::Object(map) => {
            for (_, item) in map.iter_mut() {
                resolve_page_placeholders(item, current, spawned);
            }
        }
        _ => {}
    }
}

/// sequence 步骤缺 `action` 字段时按字段推断(第 147 轮,纯函数可单测)。
///
/// 实测豆包任务 LLM 常写 `{"control_action":"wait","params":{...}}`(漏 `action:"control"`),
/// 整步没执行就报 1001「缺少 action」白烧迭代。按第 99/135 轮「参数别名容错」哲学:
/// 有 `control_action` → control,有 `info` → inspect;两者皆无返回 None(维持 1001)。
fn infer_step_action(step: &Value) -> Option<&'static str> {
    if step.get("control_action").and_then(Value::as_str).is_some() {
        Some("control")
    } else if step.get("info").and_then(Value::as_str).is_some() {
        Some("inspect")
    } else {
        None
    }
}

/// 连续执行一组已明确的浏览器动作(连续执行模式)。
async fn run_sequence(args: Value) -> Result<String> {
    let Some(steps) = args.get("steps").and_then(Value::as_array) else {
        return envelope(1001, "缺少 steps 数组", json!({}));
    };
    if steps.is_empty() {
        return envelope(1001, "steps 不能为空", json!({}));
    }
    if steps.len() > MAX_SEQUENCE_STEPS {
        return envelope(
            1001,
            &format!("steps 数量超过上限({MAX_SEQUENCE_STEPS})"),
            json!({"count": steps.len(), "max": MAX_SEQUENCE_STEPS}),
        );
    }

    let stop_on_error = args
        .get("stop_on_error")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut current_page = str_arg(&args, "page_id").map(str::to_string);
    let mut last_spawned: Option<String> = None;
    let mut results: Vec<Value> = Vec::with_capacity(steps.len());
    let mut failed_at: Option<usize> = None;
    let mut first_error: Option<(i32, String)> = None;
    // 第 150 轮:blocker_alert 命中即中断整批(优先级高于 stop_on_error);
    // shutdown 触发即取消整批(SIGINT 止血,不等外层 drop)。
    let mut interrupted_by: Option<&'static str> = None;

    for (index, raw_step) in steps.iter().enumerate() {
        // 步间取消探测:同步零成本;命中即中断,已完成步骤照常带回。
        if crate::shutdown::global().is_triggered() {
            interrupted_by = Some("shutdown");
            break;
        }
        if !raw_step.is_object() {
            failed_at = Some(index);
            first_error = Some((1001, "step 必须是对象".into()));
            results.push(json!({
                "index": index + 1,
                "code": 1001,
                "message": "step 必须是对象",
                "data": {},
            }));
            break;
        }
        let mut step = raw_step.clone();
        let mut action = step
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // 第 147 轮:缺 action 时按字段推断并**回写进步骤**(execute 需要 action 分发)
        if action.is_empty() {
            if let Some(inferred) = infer_step_action(&step) {
                action = inferred.to_string();
                if let Value::Object(ref mut map) = step {
                    map.insert("action".into(), Value::String(inferred.to_string()));
                }
            }
        }
        if action == "sequence" {
            failed_at = Some(index);
            first_error = Some((1001, "sequence 步骤不允许嵌套 sequence".into()));
            results.push(json!({
                "index": index + 1,
                "action": action,
                "code": 1001,
                "message": "sequence 步骤不允许嵌套 sequence",
                "data": {},
            }));
            break;
        }

        let follow_spawned = step
            .get("follow_spawned")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if let Value::Object(ref mut map) = step {
            map.remove("follow_spawned");
        }
        resolve_page_placeholders(&mut step, current_page.as_deref(), last_spawned.as_deref());
        let control_action = step
            .get("control_action")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let step_page_id = str_arg(&step, "page_id").map(str::to_string);

        // 第 150 轮:单步时长预算(最后防线);request_human 步豁免(人工介入
        // 合法长等待,HITL 自身有 30 分钟总上限)。
        let parsed: Value = if control_action == "request_human" {
            let output = McpWebUseTool.execute(step).await?;
            serde_json::from_str(&output).unwrap_or_else(
                |_| json!({"code": 2002, "message": "步骤返回非法 JSON", "data": {"raw": output}}),
            )
        } else {
            match tokio::time::timeout(
                std::time::Duration::from_secs(STEP_CAP_SECS),
                McpWebUseTool.execute(step),
            )
            .await
            {
                Ok(Ok(output)) => serde_json::from_str(&output).unwrap_or_else(
                    |_| json!({"code": 2002, "message": "步骤返回非法 JSON", "data": {"raw": output}}),
                ),
                Ok(Err(e)) => return Err(e),
                Err(_) => json!({
                    "code": 2002,
                    "message": format!(
                        "步骤执行超时(>{STEP_CAP_SECS}s):页面可能无响应或弹出人工验证;\
                         建议 inspect(blockers) 确认,必要时 request_human(reason=\"captcha\")"
                    ),
                    "data": {},
                }),
            }
        };
        let code = parsed.get("code").and_then(Value::as_i64).unwrap_or(2002) as i32;
        let message = parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let data = parsed.get("data").cloned().unwrap_or_else(|| json!({}));

        if code == 0 {
            if action == "open" {
                if let Some(id) = data.get("page_id").and_then(Value::as_str) {
                    current_page = Some(id.to_string());
                }
            } else if action == "control" {
                if control_action == "new_tab" {
                    if let Some(id) = data.get("spawned_page_id").and_then(Value::as_str) {
                        current_page = Some(id.to_string());
                    }
                }
            } else if action == "close" && current_page.as_deref() == step_page_id.as_deref() {
                current_page = None;
            }

            if let Some(spawned) = data.get("spawned_page_id").and_then(Value::as_str) {
                last_spawned = Some(spawned.to_string());
                if follow_spawned {
                    current_page = Some(spawned.to_string());
                }
            }
        }

        results.push(json!({
            "index": index + 1,
            "action": action,
            "code": code,
            "message": message,
            "data": data,
        }));
        // 第 150 轮:blocker_alert 命中立即中断整批(优先级高于 stop_on_error):
        // 验证弹窗在场时继续执行只会把后续输入灌进黑洞;告警随顶层 data 透出,
        // LLM 当轮即见 human_assist 载荷并发起 request_human。
        if step_has_blocker_alert(&data) {
            interrupted_by = Some("blocker_alert");
            break;
        }
        if code != 0 {
            failed_at = Some(index);
            first_error = Some((code, message));
            if stop_on_error {
                break;
            }
        }
    }

    let completed = failed_at.is_none() && interrupted_by.is_none();
    let mut data = json!({
        "mode": "continuous",
        "steps": results,
        "step_count": steps.len(),
        "completed": completed,
        "stop_on_error": stop_on_error,
    });
    if let Some(reason) = interrupted_by {
        data["interrupted_by"] = json!(reason);
        // blocker 告警提升到顶层,LLM 无需翻 steps 数组即见处置载荷。
        if reason == "blocker_alert" {
            if let Some(alert) = results
                .iter()
                .rev()
                .find_map(|r| r.get("data").and_then(|d| d.get("blocker_alert")).cloned())
            {
                data["blocker_alert"] = alert;
            }
            if let Some(ha) = results
                .iter()
                .rev()
                .find_map(|r| r.get("data").and_then(|d| d.get("human_assist")).cloned())
            {
                data["human_assist"] = ha;
            }
            if let Some(na) = results
                .iter()
                .rev()
                .find_map(|r| r.get("data").and_then(|d| d.get("next_action")).cloned())
            {
                data["next_action"] = na;
            }
        }
    }
    if let Some(index) = failed_at {
        data["failed_at"] = json!(index + 1);
    }
    if let Some(id) = &current_page {
        data["page_id"] = json!(id);
    }
    if let Some(id) = &last_spawned {
        data["spawned_page_id"] = json!(id);
    }

    // 第 150 轮:shutdown 中断走独立取消码(外层 exec 的取消竞争会兜底 drop,
    // 这里是「不依赖 drop 也立即止血」的同步路径)。
    if interrupted_by == Some("shutdown") {
        return envelope(4999, "sequence 已取消(进程关闭信号)", data);
    }
    match first_error {
        Some((code, message)) if !completed => {
            envelope(code, &format!("sequence 未完成:{message}"), data)
        }
        Some(_) | None => envelope(0, "sequence completed", data),
    }
}

// ===================== MCP_Web_Use 工具门面 =====================

/// 浏览器网页操控统一入口(MCP 风格单工具 + action 分发)。
pub struct McpWebUseTool;

/// 工具描述:同时承担「使用说明」职责(原 Chromium-WebUse Agent 系统提示词的
/// 作业规范部分精炼,全文见 SubAgent-Work 系统提示词的 MCP_Web_Use 段)。
const MCP_WEB_USE_DESCRIPTION: &str = r#"通过 CDP 驱动 Chromium 系浏览器操作网页(macOS / Windows / Linux,内存无头浏览器默认,也可接管已开浏览器;MCP 风格单工具多 action)。
用 action 参数选择操作:
- open(url*, mode?, reuse?, connect_url?, user_agent?, wait_until?, window_width?, window_height?, auto_expand_viewport?, highlight?, guard?, allow_selectors?, block_selectors?, guard_note?, overlay?, timeout_ms?, reuse_existing?, auto_relaunch?): 启动/接管 Chromium 并打开页面。**默认可见窗口(mode=headed,第 139 轮)**——不要为了「省资源」主动传 hidden,除非任务明确要求静默后台跑;无 GUI 会话(CI/容器/SSH)会自动回退 hidden,可用 LAEW_BROWSER_MODE 强制。使用一次性临时 profile,不干扰用户日常浏览器;全模式启动窗口默认 1920×1080(1080p),可用 window_width/window_height 自定义;highlight=true(默认)时 headed 窗口页面四周显示一圈蓝色选中边框+右上角「LAEW Agent 控制中」徽标,人工可一眼识别 Agent 控制的窗口;**页面管控三档(第 143 轮,guard 参数,缺省 locked)**:locked=屏蔽模式(半透明蒙层+锁定人工输入,人工可实时观看不可点击,防交叉操作;Agent 输入动作自动「先解后锁」不受影响,人工交互一律走 request_human 默认自动解锁)/ open=非屏蔽模式(人工可直接操作页面,状态条明示「🔓 页面开放」,用于人工亲自操作、人工登录先行流)/ partial=部分屏蔽模式(须同时给 allow_selectors 白名单「只有命中区人工可操作」或 block_selectors 黑名单「命中区人工不可操作」之一,互斥,各 ≤8 条;盾区内人工不可操作、盾区外放行,Agent 动作自动「先隐盾后复盾」);guard_note(≤60 字符)写进页面状态条第三行引导人工(如「请登录后应答弹窗,Agent 将接管后续操作」);缺省随 LAEW_WEB_GUARD;仅 headed 生效;运行时切换 control(set_guard);legacy overlay 布尔参数保留(true≡locked/false≡open,显式传 guard 时被忽略);timeout_ms 控制页面加载超时(毫秒,默认 60000,内网慢速网站可加大)。connect_url 接管已用 --remote-debugging-port 启动的浏览器。**复用已登录浏览器(第 142 轮)**:reuse_existing=true 自动探测本机调试端口(默认 9222,LAEW_CHROME_DEBUG_PORT 可覆盖;依次尝试 localhost/[::1]/127.0.0.1 —— Chrome 154+ 的 DevTools HTTP 端点只服务 IPv6 loopback 连接,IPv4 返回 404)并接管用户已登录的 Chrome(保留 Cookie/登录态),响应 data.reuse_existing_chrome=true + data.debug_port + data.browser_version + data.connect_mode=true;探测失败返回 code=3002 + data.relaunch_command(平台相关重启命令,转述用户执行,或 auto_relaunch=true 重试,由工具自动复制登录态并另启独立调试 Chrome 实例,不退出、不影响用户已打开的浏览器);connect 模式 close 只断连不关用户浏览器,默认不锁人工输入(管控恒为 open 且零注入,需要时 set_guard 手动开)。同 URL 已有存活页面时默认复用(导航刷新,响应 reused:true 且 page_id 不变;reuse=false 强制新开);**同 origin 兜底复用(第 146 轮)**:精确未命中但存在同 scheme+host+port 的存活页面(取最早打开的主工作页)时,在该页面上原地导航复用(响应 reuse_match=origin + previous_url),长遍历任务不再因路由不同新开冗余标签页、登录态与页面上下文不丢失;connect 模式(接管用户浏览器)不做 origin 兜底,绝不导航用户自己的标签页;浏览器实例已存在时永远复用同一进程(响应 browser_reused:true + 真实 mode),不重复打开多个浏览器。**窗口收边(第 139 轮,默认开)**:headed 模式下导航完成后按屏幕工作区自动收窄窗口(小屏笔记本不再把窗口挤出屏外「显示不全」),并保证页面视口不低于 720p,结果回 data.headed_window。**视口自适应(第 125 轮,默认开)**:导航完成后若页面内容超出视口(横向被裁/可视高度不足),自动把视口扩展到 ≤2560×1440(2K)并回 data.viewport(expanded/from/to/content/clamped/hint)——根治「视口太窄页面显示不全、元素不可见不可点」;auto_expand_viewport=false 可关;内容仍超 2K 上限时按 hint 走 full_page 截图或 set_viewport 显式超限。返回 {page_id,title,final_url,reused,mode,browser_reused,connect_mode,window,guard,overlay(legacy),headed_window?,viewport?,next_steps}。未检测到浏览器返回 code=3001(确定性失败,如实告知用户安装引导,不要重试)。
- list(): 列出当前存活页面 [{page_id,url,title,created_at}];返回前自动清理失效 entry。冷启动后多轮任务优先用它同步页面索引。
- close(page_id*): 关闭指定页面;最后一个页面关闭时回收浏览器进程。page_id="all" 一键关闭全部页面并回收浏览器(任务收尾清场)。幂等;对话型页面(用户可能继续追问)可保留复用。
- control(page_id*, control_action*, params?): 全部写操作统一入口。control_action 枚举:click/human_click/right_click/double_click/hover/scroll/scroll_to/key_press/press_sequence/input_text/human_input/clear_input/upload_file/select_option/download/new_tab/close_tab/navigate/back/forward/reload/wait/eval_js/set_cookie/delete_cookie/set_storage/clear_storage/set_viewport/screenshot/heartbeat/drag/focus/blur/mouse_move/dispatch_event/set_window/sync_viewport/set_highlight/set_overlay/set_guard/auto_follow/request_human。点击链接/new_tab 派生的新标签页经响应 spawned_page_id 回传,后续操作新页面必须用新 page_id。screenshot 一律落盘返回 save_path(看图片文字用 params.ocr=true,文本模型无法消费 base64);eval_js 直接写表达式,支持 return 与多语句(失败自动 IIFE 重试),超长返回值自动落盘并以 saved_to 引用;单次执行有超时兜底(默认 30s,await_promise=true 时 60s,可用 params.timeout_ms 指定、上限 120s——页内 Promise 轮询等合法长等待必须显式传 timeout_ms),超时返回 2002 并附「页面可能弹出人工验证」的处置指引;download 支持 http(s) url 或 selector、save_dir、filename、timeout_ms,data: URL 直接解码落盘,完成后返回绝对 save_path 与 byte_size。set_window 运行时调整真实浏览器窗口(width/height/left/top/window_state=maximized|fullscreen|minimized|normal,CDP setWindowBounds,调整后自动清除视口覆盖保证渲染自适应不缺区域);sync_viewport 在人工拖动窗口大小后调用,清除 device metrics 覆盖使视口=窗口内容区(会撤销 open 时的自动 2K 扩展);set_viewport(width,height,device_scale_factor?=1,mobile?=false) 手动设置布局视口(宽 320~7680/高 240~4320 自动 clamp;open 已默认自动扩展视口,仅当自动结果不理想时才手动指定);set_highlight(enabled) 运行时开关蓝色选中边框;auto_follow(enabled?=true | scroll_to_bottom?=true) 第 152 轮新增的「自动跟随最新内容」开关——多轮对话站(聊天/论坛/工单)消息栈不断变长时,人工在 locked 档**无法自行滚动**(CDP Input.setIgnoreInputEvents 连滚轮一并拦),必须由页面自己停在最新;跟随器仅在「用户本来就在底部」时吸底,人工往上翻历史不会被拽回;scroll_to_bottom=true 表示不改开关、立刻吸底一次;导航后按期望态自动重挂;set_guard(mode?="locked"|"open"|"partial", allow_selectors?, block_selectors?, note?) 运行时切换页面管控三档(第 143 轮,推荐;字段缺省=保持现值,空数组/空串=清除;partial 校验同 open);set_overlay(enabled) 蒙层开关 legacy 别名(true=set_guard(locked),false=set_guard(open),仅 headed 生效);request_human(reason=captcha|sms|qr_login|login|real_name|two_factor|oauth|manual_verify|custom, message?, options?, timeout_ms? 缺省按 reason 分档 captcha/sms/two_factor=120s 其余 300s, bring_to_front?=true, image_path? 指定已有截图文件, unlock_page?=true 提问期间自动放行页面供人工直接操作(第 144 轮:优先 partial 白名单挖洞,只放行账号/密码/验证码等人工必填输入区域——自动探测,或 allow_selectors? 显式指定 ≤8 条;探测不到才整页切 open;应答/超时/取消后自动恢复原档;open 下无需动作;纯问答场景传 false;提问期间人工提交表单触发导航会自动重放放行态,多步登录不断链)) 人工介入:滑块/短信验证码/扫码登录/实名认证/人脸核身/2FA 邮箱验证码/第三方 OAuth 等无法自动跳过的流程。reason=captcha 时会自动截取当前视口(或用 image_path 指定已保存的截图),弹窗内直接展示验证码图片,人工读码后填入输入框即可,不必切换窗口。macOS/Windows 桌面自动弹出人工介入弹窗(置顶+倒计时+时间轴,文案可鼠标选中复制,人工在弹窗点选项/输入文本/取消,-p 模式同样可弹;第 145 轮:主按钮为「提交 / 继续」双语义——有输入=提交验证码/动态码文本,空输入=选项 1(人工在浏览器完成操作后的交棒动作);另有「⏱ +2分钟」按钮延长等待,总上限 30 分钟;TUI 兜底行读),code=0 时 human_response 为人工回答(assist_channel 标注 gui/tui;next_hint 按「输码 vs 已完成」分流——自由文本验证码立即 input_text 填入提交,选项应答才 inspect 验证),人工取消返回 code=4002,超时或弹窗与 TUI 均不可用返回 code=4001(如实告知用户改用交互模式重试,严禁伪造结果),**4003=人工介入进行中**(提问未结束时页面归人工所有,输入类动作与可变更 eval_js 会被挂起,人工应答后重试即可,确认无并发人工操作可加 params.force=true)。
- inspect(page_id*, info*, params?): 全部只读观察统一入口。info 枚举:console(控制台输出)/network(请求响应流)/elements(元素文本与矩形;params.selector 可选,缺失时默认返回 input/button/select/textarea/a/[role=button] 等全页交互元素)/dom(outerHTML 或节点树)/localstorage/sessionstorage/cookies/screenshot/page_meta/viewport(视口+内容尺寸与 overflow 溢出判定:横向溢出=页面显示不全需 set_viewport/重开 open 自动扩展,纵向溢出截图用 full_page=true)/url/title/ping/image_urls/ocr(截图+OCR 识别图片文字,验证码/图表标签用;region 过滤词块)/blockers(启发式检测验证码/短信/扫码/登录墙等人工阻断,**含弹层语义挑战检测(第 147 轮)**:图片选择「选出在公园能看到的事物」/滑块/语义题等题面无验证码关键词的形态靠弹层结构+文案识别,返回 blockers[]+suggested_action=request_human)/extract_links(批量提取页面所有链接,返回 links[{href,text,context,is_external}]+total+truncated+scanned+hostname;params.selector 默认 "a" 可选,params.max_links 默认 200 上限 500,params.include_context 默认 true 含文章前后文供时间推断)/extract(【第 135 轮,列表/表格抓取首选】一次调用把列表页压成结构化条目并可在页面内完成过滤,只回精简字段)/page_state(【第 135 轮】读取页面 SSR 注水数据与 JSON-LD,列表数据藏在全局变量时先读它)/coverage(【第 146 轮】遍历覆盖台账:工具层自动记录 open/navigate/new_tab 的全部导航,返回 summary{distinct_pages,total_visits}+top_repeats+pages 清单;**遍历类任务开工/收口必查,未访问页面优先,防同页打转**;进程级全局事实,不依赖存活页面)。
【extract 详解(抓文章列表/新闻流/商品列表优先用它,不要手写 eval_js 猜字段)】params:item_selector*(列表项根节点 CSS 选择器,不知填什么先 probe=true)、fields(字段投影,形如 {"title":{"selector":"h3 a","required":true},"time":{"selector":"time","attr":"datetime"},"summary":{"selector":".descript","max_chars":300}},省略则只回通用 text+__url)、url_from(取链接的选择器,默认 "a",结果落在 __url)、limit(默认 200 上限 1000)、scan_cap(内部扫描上限,默认 1000)、filter{keywords[],match_all,fields[],time_field,since,until,sort("time:desc"),limit_after_filter}。返回 {items,total,returned,scanned,truncated,dropped_required,matched_before_filter,unparsed_time_fields,time_range,field_names,hostname,hint}。**时间字段支持中文相对时间**("3小时前"/"昨天")、ISO、"YYYY-MM-DD HH:mm" 与 Unix 秒;since/until 是闭区间。probe=true 时不抽数据,只返回 {repeated_classes,likely_item_classes,common_selectors} 供你选 item_selector,免去盲试选择器。过滤在页面内完成,返回值天然精简,不会撑爆上下文。
【page_state 详解】params:keys?(候选全局变量名,默认 __NEXT_DATA__/__NUXT__/__INITIAL_STATE__/__APOLLO_STATE__/__PRELOADED_STATE__/__remixContext/initialState/__INITIAL_DATA__)、probe_window?(默认 true,扫出 window 上所有 __ 前缀键名+一层结构,让你不必先猜名字)、max_bytes?(默认 20000)、max_depth?(默认 8)、max_array_items?(默认 200)。返回 {found,globals,window_globals,jsonld,dropped_paths,truncated,hostname,hint}。**被裁掉的内容会列进 dropped_paths** —— 别误以为数据就这么多;按 globals 的结构选定路径后,用 eval_js 取精确子集(如 JSON.stringify(window.__NEXT_DATA__.props.pageProps.list.slice(0,20)))。
- sequence(steps*, stop_on_error?): 连续执行模式。steps 最多 24 个,每项结构与单步调用相同(open/list/close/control/inspect),**action 可省略**(第 147 轮:含 control_action 默认 control、含 info 默认 inspect),禁止嵌套 sequence;批内 page_id 用 "$page_id"/"${page_id}" 占位,点击派生新页可用 "$spawned_page_id"/"${spawned_page_id}",默认自动跟随 spawned_page_id,单步可 follow_spawned=false 保持原页。响应逐步返回 code/message/data,并给出最终 page_id。**第 150 轮步级防护**:任一步数据带 blocker_alert(验证弹窗命中)立即中断整批并顶层透出 blocker_alert/human_assist/next_action —— 收到后必须停止继续输入,立即 request_human 让人工完成验证、交棒后再续跑剩余步骤;单步超 150s 按步骤错误处理(request_human 步豁免);等待 AI 站点回复请用 control(wait),不要把多轮提问无超时地堆进同一批。
- explore(page_id*, queries*, summary_hint?)(第 118 轮):批量观察。一次调用合并多个 inspect 维度(elements/dom/screenshot/blockers 等),queries 数组最多 8 项,每项为单步 inspect 入参(如 {"info":"elements"} / {"info":"dom","params":{"selector":"form"}} / {"info":"screenshot"} / {"info":"blockers"});返回 {results:[{info,code,message,data},...], summary_hint, ok_count, err_count}。**进入新页面先用 1 次 explore 收集完整状态**(对比单步 inspect 节省 3-5 次 LLM round-trip);blockers 命中会附带 next_action hint 指引走 request_human。
- batch(page_id*, steps*, stop_on_error?)(第 118 轮):批量混合执行,与 sequence 同语义但推荐用于「inspect + control 混合的稳定流程」(登录/表单类:inspect 验证 → input × N → click → wait → inspect 验证)。一次调用最多 24 步,允许任意 control + inspect 顺序。**对比 sequence 推荐用于批量执行场景**。

【两种工作模式】1) 单步执行模式:直接调用 open/control/inspect/list/close,一次一个动作,适合探索、调试和高风险操作;2) 连续执行模式:先用单步 inspect(elements/dom/console/network)探索结构,再 action=sequence 一次执行已明确动作链,适合流程稳定任务(登录/表单类:inspect(form) → ocr 验证码 → sequence(input×N + click + wait + verify) 一次打包)。两种模式可混合、可多次调用。
【标准作业顺序】open 拿 page_id → inspect 探索真实 DOM → control 执行动作 → inspect 验证结果 → 任务完成后 close 释放(确定不再需要的页面;全部结束用 page_id="all" 清场)。
【错误码对策】1001 修正参数(含 guard 档位类:partial 缺 allow_selectors/block_selectors 之一、两者互斥、locked/open 不收选择器);2000 page_id 失效→action=list 重新同步;2001 断连→重新 open;2002 换 selector 或 input_text 的 use_js 路径重试;3001 未安装浏览器→如实告知用户,不要编造结果;**3002 复用浏览器不可用(调试端口未开启)→ 把 data.relaunch_command 转述用户执行,或以 auto_relaunch=true 重试(自动复制登录态并另启独立调试实例,不退出用户已打开的浏览器);不要反复重试 open**;**6001 目标站点越界(任务锚点)→ 立即停止该路径,如实报告「目标站点不可达或未指定」并结束本单元;严禁改用其它站点、搜索引擎、缓存或名称相似的替代品 —— 可以失败,不可以乱跑**;**6002 人工核验弹窗在场,输入动作已拦截 → 不要重试、不要换 eval_js 绕过(可变更 eval 同样被拦);立即按 data.human_assist 载荷 request_human(reason=captcha) 让人工完成验证,应答后页面干净再继续;确认是结构误判才可 params.force=true 强制执行**。
【页面管控三档(第 143 轮,仅 headed 可视化模式)】guard 决定人工对页面的操作权限,按用户意图选档:**用户没说要自己动手/缺省 → locked**(防交叉操作是安全基线,含支付/删除等高风险动作的任务必须 locked);**用户说「我自己操作/我来点/我先登录/你看着」→ open(guard_note 写清引导)**;**用户要分区协作(「我操作登录框,你管其余/锁住支付按钮」)→ partial + allow_selectors(白名单)或 block_selectors(黑名单)**;信号不明确时 locked 起步、需要人工参与时经 request_human 询问。open/partial 下人工与 Agent 并发操作页面,**关键写操作前先 inspect 验证现场**再动手。人工登录先行流:open(guard="open", guard_note="请登录后应答弹窗…") → request_human(reason="login", message=…) → 人工在页面完成登录并应答 → set_guard(mode="locked") 回到防交叉基线 → 继续任务。运行时随时 set_guard 切换;蒙层/盾区/状态条是设计行为不是页面故障,不要试图用 eval_js 移除。
【人工介入(HITL)】遇到滑块/图形验证码(OCR 不可读)/短信验证码/扫码登录/人脸核身等无法自动完成的流程:可视化场景确认 data.mode=headed(缺省即是,无需显式传 mode=headed),让人工看到窗口(蓝色边框标识,页面默认有半透明蒙层锁定人工输入——这是设计行为不是页面故障),再 control(request_human, reason=..., message=说明要人工做什么, options=[...]);**提问期间自动放行人工输入区域(unlock_page 默认 true;第 144 轮:账号/密码/验证码等人工必填区域自动成为 partial 部分屏蔽的非屏蔽区域——状态条显示「🛡 部分锁定·人工输入区已开放」,人工只能在放行区域内拖滑块/扫码/填表,其余页面保持 Agent 管控;探测不到凭证区才整页开放(第 145 轮:两档状态条均注明「完成后回弹窗点『提交 / 继续』」);应答/超时/取消后自动复锁;inspect(info=blockers) 命中时 data.credential_zones 给出可传 allow_selectors 的区域选择器)**;macOS/Windows 桌面自动弹出人工介入弹窗(置顶+倒计时,人工在弹窗点选项/输入验证码;第 145 轮:主按钮「提交 / 继续」双语义,人工在浏览器完成操作后空输入点击即交棒 Agent 接管;「⏱ +2分钟」可延长等待,总上限 30 分钟,实测人工整链登录超 120s 分档超时的根治;弹窗文本可鼠标选中复制,输入框支持 ⌘C/⌘V/⌘A,也有「📋 复制」一键复制全部信息),Linux 或弹窗不可用时 TUI 选择块行读;code=0 用 data.human_response 继续——**先看 next_hint 分流**(第 145 轮:human_response 是自由文本验证码/动态码 → 立即 input_text 填入并提交表单;是「N. 选项」形态(如「我已完成」)→ inspect 验证页面状态后继续),data.page_unlock 回报本次放行方式对账;4001=超时/弹窗与 TUI 均不可用,如实报告;4002=人工取消(弹窗取消/Esc/关窗),终止该路径。当前实例是无头而任务需要可视化时,先 close("all") 回收再以 mode=headed 重开。
【自动阻断感知(第 147 轮;第 151 轮升级)】输入类动作(click/input_text/key_press/drag 等 20 个)、wait(含等待超时失败)、导航类动作与 **eval_js**(含 await_promise 长轮询——「发出消息→风控弹窗」正是发生在这个窗口)完成后,工具**自动探测**人工验证挑战 —— 图片选择题(「选出在公园能看到的事物」/「选出卧室家具」类)/滑块/拖拽/语义题等**题面没有「验证码」关键词**的形态,靠弹层结构(高 z-index 模态层 + 九宫格小图或 CSS 背景图网格 / 验证码尺寸 iframe / canvas / 验证文案)识别,不依赖你主动调 inspect(blockers)。命中时响应 data 携带 `blocker_alert{kind,matched,snippet,evidence,hint}` + `next_action="request_human"` + `human_assist` 载荷(题面 snippet 直接进 message,人工在弹窗即知要验证什么):**立即停止继续输入/发消息/重试**(实测事故:验证弹窗在场,Agent 无感知连发 16 轮消息直到用户手动终止),按载荷 request_human(reason=captcha) 让人工完成;不确定弹窗内容可先 control(screenshot, params.ocr=true) 确认。页面干净时响应零附加;wait 超时 + blocker_alert 同时出现 = 「回复没来 + 弹窗在场」的最强信号,优先按 blocker_alert 处置而非加大等待重试。**第 151 轮输入硬闸**:输入类动作与可变更 eval_js(execCommand/dispatchEvent/合成事件等)**执行前**还会做一次结构判定探测,命中真实验证弹窗时直接返回 6002 拦截本动作(见错误码对策)—— 人工完成验证、应答弹窗后页面干净即自动恢复,不需要任何解锁操作。
【作业要点】中文输入优先 params.use_js=true(React/Vue 受控组件兼容);复杂页面先 inspect(info=elements) 探测真实 DOM 再操作,不要硬猜 selector;AI 对话类网站回复等待用 control(wait, selector=[class*=response]..., timeout_ms=60000);看图片里的文字(验证码/图表标签)一律 screenshot(params.ocr=true) 或 inspect(info=ocr),禁止 Read 图片文件、禁止用 Bash/python/tesseract 解码图片(文本模型无视觉,纯浪费迭代);验证码读码后不要刷新页面或点击验证码图(刷新即换码),提交报验证码错误才点图刷新重读;DOM 提取注意 truncated 标记分段。
【安全红线】支付/删除/确认提交/登出等不可逆或高风险动作禁止放进 sequence,必须单步执行并检查页面状态;登录凭证只填用户明确提供的账号密码,不要编造;OCR 不可用的平台上验证码类任务如实报告等待人工,禁止猜测验证码;只读优先——能 inspect 回答的问题不做任何写操作。"#;

#[async_trait]
impl Tool for McpWebUseTool {
    fn name(&self) -> &str {
        MCP_WEB_USE_TOOL_NAME
    }

    fn description(&self) -> &str {
        MCP_WEB_USE_DESCRIPTION
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "list", "close", "control", "inspect", "sequence", "explore", "batch"],
                    "description": "要执行的浏览器操作:open(启动/接管浏览器并打开页面) / list(列出存活页面) / close(关闭页面) / control(写操作统一入口) / inspect(只读观察统一入口) / sequence(连续执行一批操作) / explore(批量观察:一次调用多个 inspect info 维度,合并返回) / batch(批量混合执行:同 sequence 但推荐用于 control + inspect 混合场景,允许任意步骤顺序)"
                },
                "url": { "type": "string", "description": "open 必填:目标网址" },
                "reuse": { "type": "boolean", "default": true, "description": "open 可选:同 URL 已有存活页面时复用(导航刷新,page_id 不变,响应 reused:true);精确未命中时同 origin 存活页面兜底复用(reuse_match=origin,第 146 轮);false 强制新开页面" },
                "mode": { "type": "string", "enum": ["headed", "hidden", "new_headless"], "default": "headed", "description": "open 可选:浏览器模式;headed=可见窗口(默认,第 139 轮),hidden=纯 CDP 无窗口,new_headless=旧 headless=true。缺省由 LAEW_BROWSER_MODE 与 GUI 会话探测决定:无 GUI 会话(CI/容器/SSH)自动回退 hidden" },
                "window_width": { "type": "integer", "minimum": 320, "maximum": 7680, "description": "open 可选:浏览器窗口宽(px);缺省全模式统一 1920(1080p),小屏按屏幕工作区自动收边;与 window_height 成对使用;浏览器已存在时仅记录请求值(单实例复用)" },
                "window_height": { "type": "integer", "minimum": 240, "maximum": 4320, "description": "open 可选:浏览器窗口高(px);缺省全模式统一 1080(1080p);运行时调整用 control(set_window),人工拖动后用 control(sync_viewport) 自适应" },
                "auto_expand_viewport": { "type": "boolean", "default": true, "description": "open 可选(默认 true):导航完成后若页面内容超出视口(横向被裁/可视高度不足),自动用 device metrics 把视口扩展到 ≤2560×1440(2K),扩展结果回 data.viewport(expanded/from/to/content/clamped/hint)——根治「视口太窄页面显示不全、元素不可见不可点」;false 关闭;内容仍超 2K 上限时截图用 params.full_page=true 或 set_viewport 显式超限" },
                "highlight": { "type": "boolean", "default": true, "description": "open 可选:headed 模式在页面四周注入一圈蓝色选中边框+右上角「LAEW Agent 控制中」徽标(标识 Agent 控制的窗口,人工介入用);pointer-events:none 不影响页面交互;可用 control(set_highlight, enabled=false) 运行时关闭" },
                "guard": { "type": "string", "enum": ["locked", "open", "partial"], "default": "locked", "description": "open 可选(第 143 轮):页面管控三档。locked=屏蔽模式(缺省,半透明蒙层+锁定人工输入,人工可实时观看不可点击,防交叉操作;Agent 输入动作自动先解后锁)/ open=非屏蔽模式(人工可直接操作页面,状态条明示「页面开放」,用于人工亲自操作或人工登录先行流)/ partial=部分屏蔽模式(盾区内人工不可操作、盾区外放行,须同时给 allow_selectors 或 block_selectors 之一)。缺省随 LAEW_WEB_GUARD;仅 headed 生效;运行时切换 control(set_guard)" },
                "allow_selectors": { "type": "array", "items": { "type": "string" }, "maxItems": 8, "description": "open 可选(第 143 轮,仅 guard=partial):白名单 —— 只有命中选择器的区域人工可操作,其余整屏屏蔽(深色罩挖洞);与 block_selectors 互斥;选择器按 frame 各自生效" },
                "block_selectors": { "type": "array", "items": { "type": "string" }, "maxItems": 8, "description": "open 可选(第 143 轮,仅 guard=partial):黑名单 —— 命中选择器的区域人工不可操作(红色盾罩,如支付/删除按钮),其余放行;与 allow_selectors 互斥" },
                "guard_note": { "type": "string", "maxLength": 60, "description": "open 可选(第 143 轮):页面状态条第三行自定义提示(三档通用,≤60 字符),如人工登录先行流传「请登录后应答弹窗,Agent 将接管后续操作」" },
                "overlay": { "type": "boolean", "default": true, "description": "open 可选(第 141 轮 legacy,建议改用 guard):true ≡ guard=locked,false ≡ guard=open;显式传 guard 时本参数被忽略;可用 control(set_guard) 运行时切换" },
                "timeout_ms": { "type": "integer", "description": "open 可选:页面加载超时(毫秒),默认 60000" },
                "connect_url": { "type": "string", "description": "open 可选:接管已开浏览器,如 http://localhost:9222(需 --remote-debugging-port 启动;Chrome 154+ 只接受 localhost/IPv6 连接,不建议用 127.0.0.1);reuse_existing=true 会自动探测默认端口,无需显式传本参数" },
                "reuse_existing": { "type": "boolean", "default": false, "description": "open 可选(第 142 轮):true=复用用户已登录浏览器——自动探测本机 CDP 调试端口(默认 9222,LAEW_CHROME_DEBUG_PORT 可覆盖)并接管,保留登录态;探测失败返回 code=3002 + data.relaunch_command(平台相关重启命令);与 connect_url 并存时 connect_url 优先" },
                "auto_relaunch": { "type": "boolean", "default": false, "description": "open 可选(第 142 轮;第 144 轮起不退出用户浏览器):reuse_existing=true 且探测失败时,自动执行「复制登录态 → 以独立调试 profile 另启 Chrome 实例 → 接管」;与用户已打开的浏览器并存互不影响,绝不关闭用户浏览器;显式 opt-in,不默认开启;成功响应附 login_state_copied/login_state_skipped 计数" },
                "user_agent": { "type": "string", "description": "open 可选:覆盖 User-Agent" },
                "wait_until": { "type": "string", "enum": ["load", "domcontentloaded", "networkidle"], "description": "open 可选:打开后额外等待(networkidle 额外等 1.5s)" },
                "block_resources": { "type": "array", "items": { "type": "string" }, "description": "open 可选:拦截资源类型(image/stylesheet/font/media/script),当前仅记录提示" },
                "page_id": { "type": "string", "description": "close/control/inspect 必填:open/list 返回或 spawned_page_id 回传的页面句柄;close 时可为 \"all\" 一键关闭全部页面并回收浏览器" },
                "control_action": {
                    "type": "string",
                    "enum": [
                        "click", "human_click", "right_click", "double_click", "hover",
                        "scroll", "scroll_to", "key_press", "press_sequence",
                        "input_text", "human_input", "clear_input",
                        "upload_file", "select_option", "download",
                        "new_tab", "close_tab",
                        "navigate", "back", "forward", "reload",
                        "wait", "eval_js",
                        "set_cookie", "delete_cookie",
                        "set_storage", "clear_storage", "set_viewport",
                        "screenshot", "heartbeat",
                        "drag", "focus", "blur", "mouse_move", "dispatch_event",
                        "set_window", "sync_viewport", "set_highlight", "set_overlay", "set_guard", "auto_follow", "request_human"
                    ],
                    "description": "control 必填:具体写操作(鼠标/键盘/输入/上传/下载/标签页/导航/等待/JS/Cookie/Storage/视口/截图等 42 个;set_window/sync_viewport=窗口自适应,set_overlay=蒙层开关(legacy),set_guard=页面管控三档切换(第 143 轮),auto_follow=自动跟随最新内容(第 152 轮,多轮对话站让人工看得到最新消息),request_human=人工介入)"
                },
                "info": {
                    "type": "string",
                    "enum": [
                        "console", "network", "elements", "dom",
                        "localstorage", "sessionstorage", "cookies",
                        "screenshot", "page_meta", "viewport",
                        "url", "title", "ping", "image_urls", "ocr", "blockers", "visibility",
                        "extract_links", "extract", "page_state", "coverage"
                    ],
                    "description": "inspect 必填:观察维度(Console 输出 / Network 流 / Elements 元素 / DOM / localStorage 等 21 个);ocr=截图+OCR 识别图片文字(验证码/图表标签,region 过滤词块);blockers=检测验证码/短信/扫码/登录墙等人工阻断(第 152 轮:同时给出 challenge_type 拖拽拼图/滑块/图片点选/文字码 与 interaction 人工操作说明,按它选 request_human 的 reason);visibility=【第 152 轮】可视性诊断(滚动容器/是否在底部/视口下方还剩多少 px),回答「Agent 看得到、人工看不到」的落差,给出 scroll_to/auto_follow 处置;extract_links=批量提取页面所有链接(href+文本+上下文);extract=【列表/表格抓取首选】一次把列表页压成结构化条目并可按关键词+时间窗+排序过滤(抓文章列表、新闻流、商品列表优先用它,别手写 eval_js);page_state=读取页面 SSR 注水数据(window.__NEXT_DATA__/initialState 等)+ JSON-LD,列表数据藏在全局变量时先读它;coverage=【第 146 轮】遍历覆盖台账(distinct 页面/每页访问次数/top 重复),遍历任务开工与收口必查、未访问页面优先"
                },
                "params": { "type": "object", "description": "control/inspect 可选:动作参数对象(selector/text/key/keys/timeout_ms/url/x/y/file_paths/save_dir/filename 等按 control_action/info 各异)。wait 纯等待支持 duration_ms 或别名 ms(单次 ≤30000,默认 500;等元素用 selector+timeout_ms ≤120000)。screenshot/ocr 支持 save_path(落盘路径)、ocr=true(返回 OCR 文字)、region{x,y,width,height}(OCR 词块过滤)、return_base64=true(显式内联 base64,默认不返回)、full_page/format/quality" },
                "execution_mode": {
                    "type": "string",
                    "enum": ["single_step", "continuous"],
                    "default": "single_step",
                    "description": "使用模式说明。single_step=默认,一次调用一个 action,适合探索/调试/高风险操作;continuous=action=sequence,先探索后在 steps 中一次执行动作链,适合稳定流程。两种模式可混合多次调用"
                },
                "steps": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 24,
                    "description": "sequence 必填:连续执行步骤。每项结构与单步入参相同(action/page_id/control_action/info/params),action 可省略(第 147 轮:含 control_action 默认 control,含 info 默认 inspect),可用 $page_id、$spawned_page_id 占位符;单步级 follow_spawned=false 可禁止自动跟随新标签页",
                    "items": { "type": "object", "additionalProperties": true }
                },
                "stop_on_error": { "type": "boolean", "default": true, "description": "sequence 可选:默认 true,任一步失败立即停止;false 用于采集全量执行报告" },
                "queries": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "description": "explore 必填:批量观察的 inspect 维度列表(每项为单步 inspect 入参,如 {\"info\":\"elements\"}/{\"info\":\"dom\",\"params\":{\"selector\":\"form\"}}/{\"info\":\"screenshot\"}/{\"info\":\"blockers\"}),一次返回合并结果(节省 LLM round-trip)",
                    "items": { "type": "object", "additionalProperties": true }
                },
                "summary_hint": { "type": "string", "description": "explore 可选:附加提示,会出现在响应 data.summary_hint 字段,便于 LLM 在终答中引用" }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let Some(action) = str_arg(&args, "action") else {
            return envelope(1001, "缺少 action", json!({}));
        };
        match action {
            "open" => run_open(args).await,
            "list" => run_list(args).await,
            "close" => run_close(args).await,
            "control" => control::run(args).await,
            "inspect" => inspect::run(args).await,
            "sequence" => run_sequence(args).await,
            "explore" => control::run_explore(args).await,
            "batch" => control::run_batch(args).await,
            other => envelope(1001, "未知 action", json!({"action": other})),
        }
    }
}
