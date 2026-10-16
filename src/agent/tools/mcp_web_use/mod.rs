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
use crate::agent::page_liveness;
use crate::error::Result;
use std::collections::HashMap;

mod blocker_probe;
mod control;
mod eval_sanitize;
mod extract;
mod follow_ctl;
mod guard_ctl;
mod hitl_guard;
mod hitl_hint;
mod input_ctl;
mod inspect;
mod login_wall;
/// 多标签页感知与后台挂起治理(6003 闸 / foreground 探测 / bring_to_front / close background)。
mod page_activity;
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

// 2026-10-13:auto-HITL(自动弹出人工介入)的 sequence/batch 上下文守卫:
// 批内单步 150s 预算(STEP_CAP_SECS)会截断 300s 分档的人工等待,批内保持
// 「提示载荷 → LLM 以独立 request_human 步介入」的既有语义(该步已豁免预算)。
// run_sequence 进入时置 true,`in_sequence_ctx` 供 `hitl_ctl::auto_hitl` 守卫判定。
tokio::task_local! {
    static IN_SEQUENCE: bool;
}

/// 当前是否处于 sequence/batch 步执行上下文(false = 单步动作 / 探测失败)。
pub(super) fn in_sequence_ctx() -> bool {
    IN_SEQUENCE.try_with(|&v| v).unwrap_or(false)
}

/// 测试钩子:在 IN_SEQUENCE scope 内执行 future(auto-HITL 守卫单测用)。
#[cfg(test)]
async fn run_in_sequence_scope_for_test<F: std::future::Future<Output = bool>>(
    v: bool,
    f: F,
) -> bool {
    IN_SEQUENCE.scope(v, f).await
}

/// 任务锚点守卫(第 128 轮):目标 URL 越出用户原文指定的站点时返回 6001 信封。
///
/// 接入点是**显式带 URL 参数的动作**(`open` / `navigate` / `new_tab` / `new_window`)——
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
            // 2026-10-14(天猫实测):锚点站点的组成部分常落在集团同域(如 tmall.com
            // 的搜索/登录落地 s.taobao.com / login.taobao.com),显式 navigate 会被
            // 本守卫拦(防漂移语义不变);合法正路是**由页面自身发起导航**。不指路
            // 的话模型要花 1~2 轮自己悟出来(实测 Plan 探索期间白烧两轮)。
            // 2026-10-16(LinkedIn 实测):同 SLD 跨 TLD 区域名(linkedin.com→linkedin.cn)
            // 已自动放行,可直接 open/navigate 继续,不会再收到 6001。
            "workaround": "若该 URL 实为锚点站点的组成部分(集团同域搜索/登录/详情跳转,\
                           如 tmall.com 任务下的 s.taobao.com(搜索)/ login.taobao.com(登录)\
                           / taobao.com(集团首页)):改走页面内表单提交(输入框填词+回车)\
                           或点击页面内链接,由页面自身发起导航,\
                           不经 open/navigate/new_tab。\
                           同 SLD 跨 TLD 区域名(如 linkedin.com→linkedin.cn)已自动放行,\
                           可直接 open/navigate;除此之外不得绕行。",
        }),
    ))
}

pub(super) fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// 从 JSON Value 读取布尔参数,兼容 LLM 高频输出的字符串 `"true"`/`"false"`。
///
/// 2026-10-16 闲鱼实测:模型对 `extract(probe=…)` 等布尔参数反复传字符串 `"true"`,
/// `Value::as_bool()` 对字符串返回 `None`,`unwrap_or(false)` 静默回退默认值,
/// probe 模式未触发 → 落入必填检查 → 2002 白烧一轮。
///
/// 兼容规则:JSON `true`/`false` 直取;字符串 `"true"`/`"1"`/`"yes"`/`"on"` → `true`,
/// `"false"`/`"0"`/`"no"`/`"off"`/`""` → `false`;数字 `0` → `false` 非零 → `true`;
/// 其他/缺失 → `default`。
pub(super) fn bool_param(v: Option<&Value>, default: bool) -> bool {
    match v {
        None => default,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => true,
            "false" | "0" | "no" | "off" | "" => false,
            _ => default,
        },
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(default),
        _ => default,
    }
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

// ===================== 多窗口并行:窗口数上限 =====================

/// 单浏览器进程允许同时存在的 Agent 窗口数上限(默认)。
/// `open(new_window=true)` / `control(new_window)` 每开一个新窗口前按
/// **去重 CDP windowId** 计数,超限 1001 提示先关闭闲置页/窗口。
/// 上限按窗口计而非按页签计 —— 同窗口多 Tab 不占新窗口额度。
pub(super) const MAX_WINDOWS_DEFAULT: usize = 4;

/// 环境变量取值解析(纯函数,可单测):正整数生效,缺省/0/非法回退默认。
pub(super) fn max_windows_from(raw: Option<String>) -> usize {
    raw.and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(MAX_WINDOWS_DEFAULT)
}

/// 环境变量覆盖(0/非法回退默认)。
pub(super) fn max_windows_from_env() -> usize {
    max_windows_from(std::env::var("LAEW_WEB_MAX_WINDOWS").ok())
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
    // 第 168 轮:模板内 `%NS%` 占位符 → 进程级随机命名空间(隐身,见 browser_ns.rs)。
    // 集中在本网关替换,probe/inspect/page_health 等全部内部模板自动生效;
    // LLM 用户表达式含字面 %NS% 的概率可忽略,误替换也无害。
    // 2026-10-14:输入侧整形 —— 顶层裸 `return` 自动包裹成 IIFE(经典脚本环境必然
    // SyntaxError,实测模型反复翻车;内部模板本就是 IIFE,零扰动)。
    // 2026-10-15:二次整形 —— 顶层 `const`/`let`/`class` 声明块级包裹:全局词法
    // 绑定跨 evaluate 常驻,同名再声明必炸(实测 `const sleep` 二次声明 SyntaxError
    // 白烧一轮迭代);裸 return 已走 IIFE(自带作用域),此处对剩余形态生效。
    let shaped = eval_sanitize::wrap_bare_toplevel_return(js);
    let js = crate::agent::browser_ns::render_ns(&eval_sanitize::shield_toplevel_lexical(&shaped));
    let params = EvaluateParams::builder()
        .expression(js.clone())
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
    .map_err(|e| format_eval_err(&e, &js))?;
    Ok(v.value().cloned().unwrap_or(Value::Null))
}

/// eval_js 失败归一(2026-10-16,USA.gov 遍历实测):`CdpError::JavascriptException`
/// 的 Display 是 `#[error("{0:?}")]` 结构体 Debug 裸串 —— 真正的错误文本
/// (`exception.description`,如 `SyntaxError: Unexpected token ')'`)排在
/// `script_id`/`hash` 之后,被首行 120 字符截断后模型只能靠列号盲猜(实测单次
/// 语法错误白烧 1~2 轮迭代,24 步 sequence 内连环 2002 整批作废)。此处下钻出
/// 「错误文本首行 + 行列 + 出错位置附近脚本摘录」;非 JS 异常回退首行截断语义。
pub(super) fn format_eval_err(
    e: &chromiumoxide::error::CdpError,
    script: &str,
) -> String {
    use chromiumoxide::error::CdpError;
    if let CdpError::JavascriptException(details) = e {
        let desc = details
            .exception
            .as_ref()
            .and_then(|ex| {
                ex.description
                    .clone()
                    .or_else(|| ex.value.as_ref().and_then(|v| v.as_str().map(str::to_string)))
            })
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| details.text.clone());
        let head: String = desc.lines().next().unwrap_or("").trim().chars().take(160).collect();
        let line = details.line_number.max(0) as usize;
        let col = details.column_number.max(0) as usize;
        return format!(
            "JS异常[{head}] (line {line} col {col}) 出错位置附近: {}",
            script_excerpt(script, line, col)
        );
    }
    let s = e.to_string();
    let first = s.lines().next().unwrap_or(&s);
    let mut out: String = first.chars().take(120).collect();
    if first.chars().count() > 120 {
        out.push('…');
    }
    out
}

/// 出错位置附近脚本摘录(诊断用,F1):按 CDP 的 0-based line/col 定位,取
/// [前 48, 后 24] 字符窗口;char 计数近似 UTF-16 列(BMP 内等价,代理对仅差
/// 偏移少许,诊断场景可接受);越界/定位失败回退脚本尾部。
fn script_excerpt(script: &str, line: usize, col: usize) -> String {
    let chars: Vec<char> = script.chars().collect();
    let mut offset = 0usize;
    for _ in 0..line {
        match chars.get(offset..).and_then(|rest| rest.iter().position(|&c| c == '\n')) {
            Some(i) => offset += i + 1,
            None => {
                offset = chars.len();
                break;
            }
        }
    }
    let pos = offset.saturating_add(col).min(chars.len());
    let start = pos.saturating_sub(48);
    let end = (pos + 24).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s.insert_str(0, "…");
    }
    if end < chars.len() {
        s.push('…');
    }
    s.replace('\n', "\\n")
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
    // 第 159 轮:走 ensure_page_alive —— 人工地址栏导航等导致 CDP 会话掉线时,
    // 按 target_id 自愈重建句柄,而不是让后续所有动作永久失败。
    // 2026-12:区分「标签页真被关」与「浏览器 CDP 连接已死」—— 后者要把
    // 「重新 open 恢复会话」的处置指引如实上报,不能谎称 page_id 不存在。
    match BrowserManager::global().ensure_page_alive(id).await {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err("page_id 不存在(标签页已关闭)".to_string()),
        Err(reason) => Err(reason),
    }
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

/// 整页抹除结构证据(2026-10-14,猎聘实测 `llaew_20261014_125139.log`)。
///
/// 猎聘对 CDP 会话做会话/指纹级打标:主文档 200 + 子资源齐全、页面完整渲染,
/// 数秒后被站点侧脚本 `location→about:blank` 整页抹除(人工登录同样被抹,实测
/// 「页面是黑的」)。此前 `page_health` 按 `final_url`(=about:blank)匹配网络
/// 事件恒 `main_doc_status=null`,只能给泛泛的 `empty_dom` 建议「先 wait 复查」,
/// Agent 空转 25+ 次导航。这里按**请求 URL** 扫缓冲,把「页面曾真实加载后被抹」
/// 的结构性证据(主文档有响应 / 同域子资源计数)透出来,判定零关键词。
#[derive(Debug, PartialEq, Eq)]
pub(super) struct WipeEvidence {
    /// 请求 URL 主文档的 response 状态(倒序最新一条;None=缓冲里无该条目)
    pub main_doc_status: Option<i64>,
    /// 同注册域 response 子资源条数(含主文档)
    pub subresources: usize,
}

/// 从网络事件(倒序,最新在前)判定整页抹除证据;不命中返回 `None`(维持既有
/// `empty_dom` 路径 —— 服务端从未响应不是抹除)。纯函数,可单测。
pub(super) fn wipe_evidence_from_events(
    events: &[&Value],
    requested_url: &str,
) -> Option<WipeEvidence> {
    let host = crate::agent::browser_pace::host_of(requested_url)?;
    let mut main_doc_status = None;
    let mut subresources = 0usize;
    for d in events {
        let Some(url) = d.get("url").and_then(Value::as_str) else {
            continue;
        };
        if crate::agent::browser_pace::host_of(url).as_deref() != Some(host.as_str()) {
            continue;
        }
        if d.get("phase").and_then(Value::as_str) != Some("response") {
            continue;
        }
        subresources += 1;
        if url == requested_url && main_doc_status.is_none() {
            main_doc_status = d.get("status").and_then(Value::as_i64);
        }
    }
    (main_doc_status.is_some() || subresources >= 5).then_some(WipeEvidence {
        main_doc_status,
        subresources,
    })
}

/// 整页抹除连击记账 → 响应字段(2026-10-14)。治理状态本体在 `browser_pace`
/// (连击计数/冷却/总账),这里只把本次结果与处置建议透给 LLM。
pub(super) fn site_wipe_field(outcome: Option<crate::agent::browser_pace::WipeOutcome>) -> Option<Value> {
    let o = outcome?;
    Some(json!({
        "streak": o.streak,
        "cooldown_triggered": o.cooldown_triggered,
        "note": "本次导航最终落地 about:blank(整页抹除连击记账):站点侧脚本在页面加载后清空了文档,原会话内重试无效",
        "advice": "处置:可试 1 次 close(page_id=\"all\") 回收浏览器换全新 profile 后重新 open(每试一次多弹一台 Chrome);\
若再被抹除即是指纹级打标(换新实例无效),改用 open(reuse_existing=true) 接管本机日常浏览器(真实指纹+登录态)或 request_human;\
同域累计抹除达到阈值后工具层对该域 laew 身份持久熔断(6004),不再放行重试",
    }))
}

/// open 导航的浏览器身份判定(2026-10-14 第二轮,纯逻辑部分便于单测):
/// `true` = laew 自起身份(launch 全新实例 / auto_relaunch 调试实例),
/// `false` = 接管用户日常 Chrome(真实指纹)。持久熔断只拦 laew 身份。
async fn open_laew_identity(connect_url: Option<&str>) -> bool {
    if let Some(c) = connect_url {
        // 本次显式 connect:看目标端口是否是 laew 登记的 relaunch 实例。
        // 解析失败按用户身份处理(保守:不误拦)。
        return c
            .rsplit_once(':')
            .and_then(|(_, p)| p.trim_end_matches('/').parse::<u16>().ok())
            .map(crate::agent::browser_reuse::is_laew_relaunch_port)
            .unwrap_or(false);
    }
    if BrowserManager::global().is_connect_mode().await {
        // 复用既有 connect 实例:只有 laew relaunch 实例才算 laew 身份。
        return BrowserManager::global().connect_is_laew_relaunch().await;
    }
    // launch 模式(全新实例)= laew 身份。
    true
}

/// 合并两次抹除记账:延迟复查(late)优先于 open 结算时的当场结果。
fn merge_wipe_outcome(
    first: Option<crate::agent::browser_pace::WipeOutcome>,
    late: Option<crate::agent::browser_pace::WipeOutcome>,
) -> Option<crate::agent::browser_pace::WipeOutcome> {
    late.or(first)
}

/// 延迟抹除复查(2026-10-14 第二轮,猎聘实测 `llaew_20261014_140053.log`):
/// open/navigate 返回时页面健康(标题/URL 正常),但指纹级打标站点会在**数秒后**
/// (实测 4~10s)才 `location→about:blank`,open 结算时的 page_health 看不到,
/// LLM 下一轮读到空白页无结构化信号、瞎猜后 close(all)+重开 → Chrome 拉起风暴。
/// 该域有抹除史(`wipe_total>0`)时,等 [`WIPE_RECHECK_DELAY`] 复读落地 URL:
/// 已变 about:blank → 记账并返回证据,调用方据此改写响应。无史站点零延迟零开销。
const WIPE_RECHECK_DELAY: std::time::Duration = std::time::Duration::from_millis(3500);

/// 返回 `Some((landed_url, late_outcome))` = 命中延迟抹除;`None` = 健康或无史。
/// `count` = 是否计入熔断账本(launch / laew relaunch 实例=true,用户日常 Chrome=false)。
pub(super) async fn deferred_wipe_check(
    page: &chromiumoxide::Page,
    requested_url: &str,
    count: bool,
) -> Option<(String, Option<crate::agent::browser_pace::WipeOutcome>)> {
    let host = crate::agent::browser_pace::host_of(requested_url)?;
    if crate::agent::browser_pace::WebPace::global().wipe_total_for(&host) == 0 {
        return None;
    }
    tokio::time::sleep(WIPE_RECHECK_DELAY).await;
    let landed = page.url().await.ok().flatten().unwrap_or_default();
    if !landed.trim().eq_ignore_ascii_case("about:blank") {
        return None;
    }
    let outcome = if count {
        crate::agent::browser_pace::WebPace::global()
            .note_navigation_outcome(requested_url, &landed)
    } else {
        None
    };
    tracing::warn!(
        requested_url,
        "延迟抹除复查命中:open 返回后页面被站点脚本重定向到 about:blank"
    );
    Some((landed, outcome))
}

// =================== 2026-10-14 第三轮:后台抹除看护(wipe watcher) ===================
//
// 缺口(猎聘实测 `llaew_20261014_145428.log`):整页抹除实测发生在导航完成后
// 4~10s,而 deferred_wipe_check 只在有史域做一次 3.5s 同步复查,且「落地非 blank
// 即清总账」让「先落地成功→数秒后被抹」的序列先清零后无补账 —— 总账轨迹实测
// 0→1→0→1→2→0,持久熔断(阈值 3)永不触发,close(all)→重开的换 profile 循环
// 不受限,Chrome 反复拉起(单任务 4 台)。本看护在每次 laew 身份导航落地非 blank
// 后 spawn 10s 异步任务复读落地 URL,变 blank 即 note_navigation_outcome 补账,
// 总账/连击/冷却/持久熔断全链路如期生效。

/// 后台抹除看护代际表:page_id → 最新一次被看护导航的代际号。同页后续导航
/// bump 代际,旧看护任务醒来后发现自己过期即放弃 —— 防止把「新导航的落地页」
/// 误记为旧请求被抹。
static WIPE_WATCH_GEN: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, u64>>,
> = std::sync::OnceLock::new();

/// 看护窗时长:覆盖实测 4~10s 的延迟抹除。
const WIPE_WATCH_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

fn wipe_watch_bump(page_id: &str) -> u64 {
    let map = WIPE_WATCH_GEN
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    let gen = guard.entry(page_id.to_string()).or_insert(0);
    *gen += 1;
    *gen
}

fn wipe_watch_stale(page_id: &str, gen: u64) -> bool {
    WIPE_WATCH_GEN
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|g| g.get(page_id).copied())
        .map(|cur| cur != gen)
        .unwrap_or(true)
}

/// 后台延迟抹除看护:`count=true`(launch 实例 / laew relaunch 调试实例)的导航
/// 落地非 blank 后调用。用户日常 Chrome 不看护(落地行为归用户,不进熔断账本)。
pub(super) fn schedule_wipe_watch(
    page: chromiumoxide::Page,
    page_id: &str,
    requested_url: &str,
    count: bool,
) {
    if !count {
        return;
    }
    if crate::agent::browser_pace::host_of(requested_url).is_none() {
        return;
    }
    let gen = wipe_watch_bump(page_id);
    let pid = page_id.to_string();
    let url = requested_url.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(WIPE_WATCH_DELAY).await;
        if wipe_watch_stale(&pid, gen) {
            return; // 同页已有更新的导航接管,旧请求的看护作废
        }
        let landed = page.url().await.ok().flatten().unwrap_or_default();
        if !landed.trim().eq_ignore_ascii_case("about:blank") {
            // 存活过看护窗 = 真实恢复证据:清该域连击与总账(仅「落地成功」不清 ——
            // 抹除发生在其后 4~10s,假成功会把补记账清掉,熔断永不触发)。
            // 回读为空(句柄已死/页已关)不算存活,不清账。
            if landed.starts_with("http") {
                let cleared = crate::agent::browser_pace::WebPace::global()
                    .note_navigation_survived(&url);
                if cleared {
                    tracing::info!(
                        requested_url = %url,
                        "后台抹除看护:页面存活过看护窗,清该域抹除连击与总账(恢复路径)"
                    );
                }
            }
            return;
        }
        let outcome = crate::agent::browser_pace::WebPace::global()
            .note_navigation_outcome(&url, &landed);
        tracing::warn!(
            requested_url = %url,
            streak = outcome.as_ref().map(|o| o.streak).unwrap_or(0),
            "后台抹除看护命中:导航返回后约 10s 页面被站点脚本重定向到 about:blank(已补账:连击/总账/冷却)"
        );
    });
}

// ============================ 第 172 轮:页面存活性 / 登录墙 / 指纹 ============================
//
// 背景(BOSS 直聘实测,`tmpPlan/20261014_164500_BOSS直聘页面自刷新与反爬识别治理方案.md`):
// ① 站点对「一次性全新 profile + 自动化浏览器」做自杀式反爬 —— 页面静置期间被原地
//    reload(实测 30s 内 timeOrigin 变化、页面全局标记丢失、URL/标题不变),工具层
//    完全无感知,人工在 HITL 弹窗里输入的内容随文档销毁(体感「输入不了」);
// ② `control(navigate)` 只回报**请求** URL,服务端把页面 302 到登录页 / 302 回首页
//    这类事实全部不可见,Agent 连续 3 轮试探才自行发现「要登录」,甚至把首页推荐位
//    卡片当成职位列表抓取。
//
// 三个信号统一由本节装配:
/// - [`crate::agent::page_liveness`] 自发重载台账 → `data.page_liveness`;
/// - [`login_wall`] 登录墙形态判定 → `data.login_wall`;
/// - [`crate::agent::fingerprint`] 环境指纹体检 → `data.fingerprint`。

/// 采样一次页面文档身份(`performance.timeOrigin` + 落地 URL)并记账。
///
/// `agent_initiated=true` 表示本次文档替换由 Agent 自己的导航触发(不计入
/// 自刷新统计);`false` 用于常规动作后的复查采样 —— 文档身份变了 = 站点自己
/// 重建了文档,这是「页面自刷新」的唯一机械证据。
///
/// 5s 节流(见 [`page_liveness::should_sample`]):高频动作序列下最多每 5s 一次
/// 额外 CDP 往返,fail-open(超时/页面不存在/解析失败一律静默)。
pub(super) async fn sample_liveness(page_id: &str, agent_initiated: bool) {
    if !page_liveness::should_sample(page_id) {
        return;
    }
    let page = match BrowserManager::global().ensure_page_alive(page_id).await {
        Ok(Some(p)) => p,
        _ => return,
    };
    let raw = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        eval_js_string(&page, page_liveness::PROBE_JS),
    )
    .await
    {
        Ok(Ok(v)) => v,
        _ => return,
    };
    if let Some((origin, url)) = page_liveness::parse_probe(raw.as_str().unwrap_or_default()) {
        page_liveness::observe(page_id, origin, &url, agent_initiated);
    }
}

/// 取该页当前的登录墙/会话 DOM 形态(轻量判断式,只回固定几项标记,不取整页 HTML)。
async fn login_dom_probe(page: &chromiumoxide::Page) -> Option<String> {
    let js = r#"(() => {
        const has = (s) => { try { return !!document.querySelector(s); } catch (e) { return false; } };
        const marks = [];
        if (has("input[type='password']") || has('input[type="password"]')) marks.push('password_input');
        const btns = Array.from(document.querySelectorAll('button,a,[role=button]'))
            .slice(0, 80).map(e => (e.innerText || '').trim()).filter(Boolean);
        if (btns.some(t => /登录|登入|sign in|log ?in/i.test(t))) marks.push('login_button_text');
        if (has("a[href*='logout']") || has("[class*='logout']")) marks.push('session_logout');
        if (has("[class*='avatar']") || has("[class*='user-info']") || has('[data-uid]')) marks.push('session_avatar');
        return marks.join(',');
    })()"#;
    let v = eval_js_string(page, js).await.ok()?;
    v.as_str().map(str::to_string)
}

/// 采集环境指纹并装配 `data.fingerprint`(fail-open;采不到就不带字段)。
pub(super) async fn attach_fingerprint(data: &mut Value, page_id: &str) {
    if fingerprint_disabled() {
        return;
    }
    // 节流:同一页 30s 内只采一次(fingerprint 字段全静态,重复采无意义)。
    static LAST: std::sync::OnceLock<std::sync::Mutex<HashMap<String, u64>>> = std::sync::OnceLock::new();
    let reg = LAST.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    {
        let mut m = reg.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if m.get(page_id).is_some_and(|t| now.saturating_sub(*t) < 30) {
            return;
        }
        m.insert(page_id.to_string(), now);
    }
    let page = match BrowserManager::global().ensure_page_alive(page_id).await {
        Ok(Some(p)) => p,
        _ => return,
    };
    let raw = match tokio::time::timeout(
        std::time::Duration::from_secs(8),
        eval_js_string(&page, crate::agent::fingerprint::PROBE_JS),
    )
    .await
    {
        Ok(Ok(v)) => v,
        _ => return,
    };
    if let Some(fp) = crate::agent::fingerprint::parse(raw.as_str().unwrap_or_default()) {
        data["fingerprint"] = crate::agent::fingerprint::build(&fp);
    }
}

/// 指纹保真注入杀开关(`LAEW_WEB_FINGERPRINT=off`):连体检带注入一起关,
/// 供排障「是不是我们改的」时对照。唯一真源已上移到
/// [`crate::agent::stealth::fingerprint_kill_switch`](第 174 轮:同一开关还要决定
/// `browser_ns` 保真块是否注入,两处读法必须同源)。
pub(super) fn fingerprint_disabled() -> bool {
    crate::agent::stealth::fingerprint_kill_switch()
}

/// 装配 `data.stealth`(第 174 轮):反检测档位 + 各块是否生效 + 进程级默认值。
///
/// 零 CDP 开销(只读进程态),每次导航/只读动作收尾都带 —— 人工与 QC 据此对账
/// 「这一轮到底处在哪一档」,不必翻环境变量。
pub(super) async fn attach_stealth_mode(data: &mut Value) {
    let effective = BrowserManager::global().stealth_mode().await;
    data["stealth"] = crate::agent::stealth::status_json(effective);
}

/// 导航类动作收尾的统一信号装配:自刷新台账(Agent 主动) + 登录墙 + 指纹。
///
/// `final_url` 必须是**实际落地** URL(而非请求 URL)—— 登录墙判定完全依赖它。
pub(super) async fn enrich_after_navigation(
    page_id: &str,
    data: &mut Value,
    final_url: &str,
    include_fingerprint: bool,
) {
    sample_liveness(page_id, true).await;
    page_liveness::attach(data, page_id);
    if let Ok(Some(page)) = BrowserManager::global().ensure_page_alive(page_id).await {
        let dom = login_dom_probe(&page).await;
        login_wall::attach(data, final_url, dom.as_deref());
    }
    if include_fingerprint {
        attach_fingerprint(data, page_id).await;
    }
    attach_stealth_mode(data).await;
}

/// 只读动作收尾的信号装配:自刷新台账(非主动)+ 登录墙复查(不采指纹)。
pub(super) async fn enrich_after_read(page_id: &str, data: &mut Value) {
    sample_liveness(page_id, false).await;
    page_liveness::attach(data, page_id);
    if let Ok(Some(page)) = BrowserManager::global().ensure_page_alive(page_id).await {
        let dom = login_dom_probe(&page).await;
        if let Ok(Some(url)) = page.url().await {
            login_wall::attach(data, &url, dom.as_deref());
        }
    }
    attach_stealth_mode(data).await;
}

/// open/navigate 响应附抹除账本快照(2026-10-14 第三轮):该域总账 >0 时返回
/// `site_wipe_ledger` 字段 —— 落地成功但域有抹除史时,LLM 一眼看到局势,
/// 数据尽快落盘、别按正常站点推进。纯转发 [`WebPace::wipe_ledger_snapshot`]。
/// 连接级失败识别(第 172 轮):判断一条 CDP/浏览器错误是否属于「句柄已死、
/// 该清掉重来」而非「动作本身失败」。
///
/// 背景(2026-10-14 对照任务实测):`Browser::connect` 接到一个「HTTP 探得到、
/// WebSocket 已死」的实例后,后续命令统一返回 futures 的 `oneshot canceled`
/// —— 该错误对模型零信息量,实测模型原样重试 3 次全灭(每次约 1~3s,白烧)。
/// 这里把它归一为可行动文案 + 就地清尸,使「重试一次」真正能恢复。
///
/// 判据是**错误文本形态**而非错误类型(CDP 错误类型在跨版本间不稳定),纯函数
/// 便于单测锁死。
pub(super) fn is_connection_level_failure(msg: &str) -> bool {
    const MARKERS: &[&str] = &[
        "oneshot canceled",
        "Channel is closed",
        "channel closed",
        "Session with given id not found",
        "Target closed",
        "connection closed",
        "WebSocket",
        "handshake",
    ];
    let m = msg.to_ascii_lowercase();
    MARKERS.iter().any(|k| m.contains(&k.to_ascii_lowercase()))
}

/// 导航失败分类与诊断(纯函数,可单测)。
///
/// 背景(2026-10-16 LinkedIn 实测):`net::ERR_HTTP_RESPONSE_CODE_FAILURE` 返回
/// `{"code":2001,"data":{},"message":"net::ERR_HTTP_RESPONSE_CODE_FAILURE"}`,
/// 零诊断 —— LLM 只能盲猜「可能是网络问题」,同错误连吃 3 次共烧 10 轮迭代。
/// 该错误名的语义是「服务器返回了 HTTP 状态码但 Chrome 判为失败」(反爬/WAF
/// 常见 403/429/999),与 `ERR_CONNECTION_RESET`(网络层,重试可恢复)处置完全不同。
///
/// 本函数按错误文本形态分类,产出 `nav_failure{class,status_hint,retryable,remedy}`
/// 诊断块,挂进失败信封的 `data`,让 LLM 第一轮就能做正确决策。
pub(super) fn nav_failure_diagnose(msg: &str) -> Value {
    let m = msg.to_ascii_lowercase();
    let class = if m.contains("http_response_code_failure") || m.contains("http response code") {
        "http_response_failure"
    } else if m.contains("err_connection")
        || m.contains("err_name_not_resolved")
        || m.contains("err_timed_out")
        || m.contains("err_internet_disconnected")
        || m.contains("err_address_unreachable")
        || m.contains("err_tunnel_connection_failed")
    {
        "connection"
    } else if m.contains("err_cert") || m.contains("err_ssl") || m.contains("err_tls") {
        "tls"
    } else if m.contains("timeout") || m.contains("超时") {
        "timeout"
    } else {
        "other"
    };
    let (retryable, remedy) = match class {
        "http_response_failure" => (
            false,
            "服务器已拒绝本次请求(反爬/WAF 返回非成功状态码,常见 403/429/999)。\
             reload/同参数重试无效。首选 open(reuse_existing=true) 接管已登录的日常浏览器\
             (真实指纹+登录态,connect 模式零注入);仍不可达时 request_human 请人工打开,\
             或如实报告不可达并停止 —— 禁止改用 Bash curl/ping 绕行,禁止换站点替代",
        ),
        "connection" => (
            true,
            "网络层失败(连接被重置/DNS 解析失败/网络不可达)。直接重试同一条 open 一次;\
             连续两次失败则如实报告网络不可达,不要反复重试同一参数",
        ),
        "tls" => (
            true,
            "TLS/证书校验失败。内网自签名证书场景设 LAEW_TLS_INSECURE=1;\
             公网站点则可能是中间人拦截或证书过期,如实报告并停止",
        ),
        "timeout" => (
            true,
            "导航超时。检查网络连通性后重试一次;连续超时则如实报告站点响应超时",
        ),
        _ => (
            true,
            "导航失败(原因未归类)。重试一次;若同错误复现,改用 open(reuse_existing=true)\
             接管已登录浏览器,或 request_human 请人工打开",
        ),
    };
    json!({
        "class": class,
        "error_text": msg,
        "retryable": retryable,
        "remedy": remedy,
    })
}

pub(super) fn site_wipe_ledger_field(requested_url: &str) -> Option<Value> {
    let host = crate::agent::browser_pace::host_of(requested_url)?;
    crate::agent::browser_pace::WebPace::global().wipe_ledger_snapshot(&host)
}

/// auto_relaunch 成功信封补抹除预警(2026-10-14 第三轮):复制登录态的调试实例
/// 仍属 CDP 拉起环境,指纹级打标站点照样被抹(猎聘实测)。该域有抹除史时在
/// data 上附 `site_wipe_warning`,引导模型被抹后直接转用户日常 Chrome 接管,
/// 不要再 close(all) 换 profile 硬闯一轮。只补 code=0 信封(失败信封自有指引)。
fn annotate_wipe_warning(res: &str, url: &str) -> Option<String> {
    let host = crate::agent::browser_pace::host_of(url)?;
    let ledger = crate::agent::browser_pace::WebPace::global().wipe_ledger_snapshot(&host)?;
    let mut v: Value = serde_json::from_str(res).ok()?;
    if v.get("code").and_then(Value::as_i64) != Some(0) {
        return None;
    }
    v["data"]["site_wipe_warning"] = json!({
        "ledger": ledger,
        "note": "该域曾整页抹除本环境:auto_relaunch 实例仍属 CDP 拉起环境,可能同样被抹;\
若本页数秒后变 about:blank,不要再 close(all) 换 profile 硬闯 —— 把 3002/6004 信封里的 relaunch_command 转述给用户以其日常 Chrome 执行后 open(reuse_existing=true) 接管,或 request_human",
    });
    serde_json::to_string(&v).ok()
}

/// 第 161 轮:open 响应主文档健康诊断(fail-open;不命中不带字段,零噪声)。
///
/// 背景(内网站点实测 llaew_20261012_160035 日志):主文档被 WAF 返回 HTTP 400 空
/// HTML,页面白屏;第 159 轮虽已把 Network 证据(含 loadingFailed)收进环形缓冲,
/// 但 Agent 要自己想到 `inspect(network)` 才看得到,实测盲试 reload/wait/fetch
/// 5 轮约 3 分钟才确认不可达。open 返回时主文档状态码已在缓冲里,当场透出
/// 「页面空白 + 结构性证据 + 处置建议」,模型第一轮就能改道 reuse_existing /
/// request_human,不再空烧。判定只认结构证据(状态码 ≥400 / 排除注入层后 DOM 为空),
/// 不做关键词猜测:
/// - `main_doc_error`:主文档响应状态 ≥400(服务器/WAF 拒绝,reload 通常无效);
/// - `empty_dom`:排除 `[data-%NS%agent]` 注入层后仅剩 html/head/body 且无文本
///   (SPA 首帧未跑完也会短暂如此,文案相应更软,引导先 wait 复查);
/// - `site_wipe`(2026-10-14):落地 about:blank 但请求 URL 主文档有响应/同域
///   子资源齐全 —— 页面曾完整加载后被站点侧脚本整体抹除(见 [`WipeEvidence`])。
///
/// `requested_url` = 本次显式导航请求的 URL(与 `final_url` 在被抹除时不同)。
async fn page_health_field(page_id: &str, final_url: &str, requested_url: &str) -> Option<Value> {
    // ⓪ 整页抹除证据(最优先:final_url 落 about:blank 时按请求 URL 扫缓冲;
    //    比 empty_dom 更具体 —— 「曾加载后被抹」与「从未加载」处置完全不同)。
    if final_url.trim().eq_ignore_ascii_case("about:blank")
        && !requested_url.trim().eq_ignore_ascii_case("about:blank")
    {
        let wipe = BrowserManager::global()
            .events(page_id)
            .await
            .and_then(|buf| {
                buf.network.lock().ok().and_then(|q| {
                    let events: Vec<&Value> = q.iter().rev().map(|ev| &ev.data).collect();
                    wipe_evidence_from_events(&events, requested_url)
                })
            });
        if let Some(ev) = wipe {
            return Some(json!({
                "blank": true,
                "reason": "site_wipe",
                "main_doc_status": ev.main_doc_status,
                "subresources": ev.subresources,
                "requested_url": requested_url,
                "note": "页面已完整加载后被站点侧脚本整体重定向到 about:blank(会话/指纹级反爬整页抹除,猎聘实测):主文档/子资源均有响应,非网络故障;原会话内重试无效",
                "next_action": "不要在原会话继续导航重试。处置:可试 1 次 close(page_id=\"all\") 回收浏览器换全新 profile 后重新 open(每试一次多弹一台 Chrome);\
若再被抹除即指纹级打标(换新实例无效),改用 open(reuse_existing=true) 接管本机日常浏览器(真实指纹+登录态)或 request_human;\
同域累计抹除达阈值后工具层对该域 laew 身份持久熔断(6004);人工在场可 request_human 协助登录",
            }));
        }
    }
    // ① 主文档状态:扫 Network 环形缓冲里 url 命中 final_url 的 response 条目
    //    (倒序取最后一条,天然覆盖「同 URL 多次重定向/reload」)。
    let main_status = BrowserManager::global()
        .events(page_id)
        .await
        .and_then(|buf| {
            buf.network.lock().ok().and_then(|q| {
                q.iter().rev().find_map(|ev| {
                    let d = &ev.data;
                    (d.get("phase").and_then(Value::as_str) == Some("response")
                        && d.get("url").and_then(Value::as_str) == Some(final_url))
                        .then(|| d.get("status").and_then(Value::as_i64))
                        .flatten()
                })
            })
        });
    // ② 空文档判定(只读 evaluate,不改 DOM;connect 模式同样只读不注入)。
    // 判定只看结构证据:排除 `[data-<ns>agent]` 注入层(高亮框/蒙层/状态条/盾区
    // 的 light DOM host)后仅剩 html/head/body(real ≤3)即无真实内容。**不**叠加
    // body.innerText 文本门槛 —— 注入层文案(第 169 轮起住 closed Shadow DOM,
    // 不再进 innerText;此前恒计入约 50 字)在历史版本会把真空页误判为非空
    // (内网站点 400 空文档实测如此),文本门槛对结构判定无增益。
    // 第 168 轮:同探空文本前置匹配**风控页签名**(实测京东 `.jd-main-risk-fallback`
    // 「当前页面异常/请刷新或切换账户试试」,登录后风控复核触发,自动 reload 无效)。
    let dom: Option<Value> = async {
        let page = BrowserManager::global().page(page_id).await?;
        let v = eval_js_string(
            &page,
            r#"JSON.stringify((()=>{
                try {
                    const real = document.querySelectorAll('*:not([data-%NS%agent])').length;
                    const t = (document.body && (document.body.innerText || '').slice(0, 600)) || '';
                    const risk = t.indexOf('当前页面异常') >= 0
                        || t.indexOf('请刷新或切换账户') >= 0
                        || t.indexOf('内容太火爆了') >= 0;
                    return {real, risk};
                } catch (e) { return {real: -1, risk: false}; }
            })())"#,
        )
        .await
        .ok()?;
        let s = v.as_str()?;
        serde_json::from_str::<Value>(s).ok()
    }
    .await;
    let dom_blank = dom
        .as_ref()
        .map(|d| d.get("real").and_then(Value::as_i64).unwrap_or(-1) <= 3);
    // 风控拦截页优先于 empty_dom 报告(页面 DOM 并不空 —— 有风控 fallback 容器,
    // 但 reload 空转是负收益,必须第一时间给出正确处置路径)。
    if dom
        .as_ref()
        .map(|d| d.get("risk").and_then(Value::as_bool).unwrap_or(false))
        .unwrap_or(false)
    {
        return Some(json!({
            "blank": false,
            "reason": "risk_page",
            "main_doc_status": main_status,
            "note": "站点风控拦截页(登录后常见,实测 .jd-main-risk-fallback):自动 reload/点页面「刷新」均无效",
            "next_action": "control(request_human, reason=manual_verify) 请人工在窗口处理(刷新/滑块/切换账号);根治建议 open(reuse_existing=true) 复用日常浏览器(真实指纹+登录态,connect 模式零注入)",
        }));
    }
    if let Some(status) = main_status.filter(|s| *s >= 400) {
        return Some(json!({
            "blank": dom_blank.unwrap_or(true),
            "reason": "main_doc_error",
            "main_doc_status": status,
            "note": "主文档返回非成功状态(服务器/WAF 拒绝;瑞数类反爬对自动化浏览器恒 4xx 空 HTML),reload 通常无效",
            "next_action": "inspect(network) 查看全部拦截证据;改用 open(reuse_existing=true) 复用已登录浏览器(真实指纹+登录态);仍不可达时 request_human 请人工打开,或如实报告不可达并停止",
        }));
    }
    if dom_blank == Some(true) {
        return Some(json!({
            "blank": true,
            "reason": "empty_dom",
            "main_doc_status": main_status,
            "note": "页面 DOM 无真实内容(可能 JS 未执行完成,或服务端/WAF 返回空文档)",
            "next_action": "先 wait 1~2s 再 inspect(elements) 复查;持续空白按反爬处理:inspect(network) 查证据、open(reuse_existing=true) 复用已登录浏览器,仍不可达 request_human 或如实报告",
        }));
    }
    None
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
    if args.get("overlay").is_some() {
        let ov = bool_param(args.get("overlay"), false);
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
        let res = open_via_connect(
            args,
            url,
            &info.connect_url,
            json!({
                "reuse_existing_chrome": true,
                "debug_port": info.port,
                "browser_version": info.browser_version,
            }),
        )
        .await?;
        // 第 170 轮:端口命中的实例不可接管 —— 通常是上次 auto_relaunch 残留的
        // 调试实例或不可用的用户 Chrome;错误信封补提示,引导模型走普通 open
        // 兜底,不要对着同一端点反复重试(实测 receiver-gone 死循环)。
        // 注意:run_open_inner 把浏览器失败封成 **Ok(2001 信封字符串)**,这里
        // 必须解包信封看 code,Err 分支是真异常(不含 2001 信封)。
        if let Some(rewritten) = annotate_reuse_failure(
            &res,
            "probe_matched_but_dead",
            format!(
                "端口 {} 上的 Chrome 不可接管(CDP 无响应);可能是上次残留的调试实例,直接用普通 open(url) 冷启动即可",
                info.port
            ),
        ) {
            return Ok(rewritten);
        }
        return Ok(res);
    }
    // 档位三:自动重启接管(显式 opt-in)。第 144 轮:不退出用户 Chrome ——
    // 复制登录态到独立调试 profile 并另启独立实例(与用户浏览器并存)。
    if auto_relaunch {
        // 第 174 轮:熔断预检前移 —— 即将拉起的调试实例注定 laew 身份,若目标域
        // 已持久熔断(指纹级打标),先复制登录态+spawn Chrome 再被 guard 拒绝是
        // 「先斩后奏」:实例整场闲置还占住调试端口(猎聘实测 91614 全程未用)。
        // 命中即原样返回 6004,不复制、不拉起、不登记。
        if let Some(gated) = crate::agent::browser_pace::guard_navigation(
            url,
            bool_param(args.get("force"), false),
            true,
        )
        .await
        {
            return gated;
        }
        return match crate::agent::browser_reuse::relaunch_and_wait(port).await {
            Some(out) => {
                let child_pid = out.child_pid;
                let res = open_via_connect(
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
                .await?;
                // 第 170 轮:接管失败(2001/2002)必须回收我们刚拉起的调试实例 ——
                // 否则它成为孤儿 about:blank 窗口留在用户桌面,且后续 probe 9222
                // 命中这个不可接管的实例,connect 再次失败形成死循环(猎聘任务
                // 实测连烧 3 次 2001 + close(all) 白烧约 1 分钟)。回收后错误
                // 信封改写为「已回收 + 普通 open 兜底」,让模型跳出复用路径。
                // (run_open_inner 把浏览器失败封成 Ok(2001 信封),故解包看 code。)
                if envelope_is_disconnect(&res) {
                    crate::agent::browser_reuse::kill_relaunch_browser(child_pid);
                    if let Some(rewritten) = annotate_reuse_failure(
                        &res,
                        "relaunch_killed",
                        format!(
                            "auto_relaunch 拉起的调试实例接管失败已回收(端口 {} 已释放);直接用普通 open(url) 冷启动即可,不要重试 reuse_existing",
                            out.info.port
                        ),
                    ) {
                        return Ok(rewritten);
                    }
                }
                // 2026-10-14 第三轮:该域有抹除史时补 site_wipe_warning —— 复制登录态
                // 的调试实例仍属 CDP 拉起环境,指纹级打标站点照样被抹(猎聘实测),
                // 提前预警「被抹后转用户日常 Chrome 接管」,别再 close(all) 硬闯一轮。
                Ok(annotate_wipe_warning(&res, url).unwrap_or(res))
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

/// 第 170 轮:复用路径(档位一/档位三)connect 失败时的错误信封判定+改写。
/// `envelope_is_disconnect` 判定信封 code 是否为 2001/2002(CDP 断连/动作失败);
/// `annotate_reuse_failure` 命中则注入 `key:true` + 兜底提示并返回改写后的信封 JSON,
/// 未命中/畸形返回 None(调用方透传原信封)。
fn envelope_is_disconnect(env_str: &str) -> bool {
    serde_json::from_str::<Value>(env_str)
        .map(|v| v["code"] == 2001 || v["code"] == 2002)
        .unwrap_or(false)
}

fn annotate_reuse_failure(env_str: &str, key: &str, hint: String) -> Option<String> {
    if !envelope_is_disconnect(env_str) {
        return None;
    }
    let mut v = serde_json::from_str::<Value>(env_str).ok()?;
    v["data"][key] = json!(true);
    v["data"]["hint"] = json!(hint);
    Some(serde_json::to_string(&v).unwrap_or_else(|_| env_str.to_string()))
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
/// 步内参数形态兼容(2026-10-14,纯函数可单测):顶层缺 `url` 且 `params` 是对象时
/// 把 params 逐键并入顶层(不覆盖既有顶层键)。
///
/// sequence/batch 步内其余 action 的参数统一住 `params`(navigate/click/…),唯
/// open 要求顶层 `url`;实测 LLM 在步内自然写 `{"action":"open","params":{"url":…,
/// "guard":…}}`,整步 1001「缺少 url」5ms 全灭(猎聘任务 iter18)。顶层 open 语义不变。
fn merge_open_params(args: &mut Value) {
    if str_arg(args, "url").is_some() {
        return;
    }
    let Some(Value::Object(params)) = args.get("params").cloned() else {
        return;
    };
    if let Value::Object(top) = args {
        for (k, v) in params {
            top.entry(k).or_insert(v);
        }
    }
}

async fn run_open(mut args: Value) -> Result<String> {
    merge_open_params(&mut args);
    let Some(url) = str_arg(&args, "url") else {
        return envelope(1001, "缺少 url(顶层或 params.url)", json!({}));
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
    // 多窗口并行:new_window(自有浏览器新窗口)与「接管用户已登录浏览器」互斥。
    let new_window = args
        .get("new_window")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if new_window && reuse_existing {
        return envelope(
            1001,
            "new_window 与 reuse_existing 互斥(新窗口仅支持 launch 模式的自有浏览器;connect 模式不干预用户浏览器)",
            json!({"new_window": true, "reuse_existing": reuse_existing}),
        );
    }
    // 第 174 轮:反检测档位显式覆盖(非侵入式 fidelity / 侵入式 hardened / off)。
    // 页面创建、复用、自愈、派生四条路径共用同一个注入点(读实例期望态),故这里
    // 只需在 open 入口设一次;运行中切换用 control(set_stealth)。
    if let Some(v) = args.get("stealth") {
        match crate::agent::stealth::StealthMode::from_value(v) {
            Some(m) => {
                BrowserManager::global().set_stealth(m).await;
            }
            None => {
                return envelope(
                    1001,
                    "stealth 取值非法(fidelity=非侵入式 / hardened=侵入式 / off=不处置)",
                    json!({"stealth": v, "accepted": ["fidelity", "hardened", "off"]}),
                );
            }
        }
    }
    // 第 166 轮:焦点租约 —— Chrome 启动/窗口置前会抢走 OS 键盘焦点,
    // keep 档(缺省)在本次 open 全链收口后把焦点归还给调用前的前台应用,
    // 用户正在其它窗口打字不被打断(见 browser_focus 模块头)。
    let focus_lease = crate::agent::browser_focus::FocusLease::capture();
    let res = if reuse_existing && str_arg(&args, "connect_url").is_none() {
        run_open_reuse(&args, url).await
    } else {
        run_open_inner(&args, url).await
    };
    focus_lease.release();
    res
}

/// run_open 主体(url 已提取、锚点守卫已过、复用分支已判定)。
///
/// 第 142 轮拆出:复用路径(open_via_connect)直接调本函数,避免
/// run_open ↔ run_open_reuse ↔ open_via_connect 递归 async fn 循环。
async fn run_open_inner(args: &Value, url: &str) -> Result<String> {
    // 第 171 轮:站点导航节奏闸 —— 频控冷却中同域导航返 6004(动作不执行,附剩余
    // 秒数与处置建议),间隔不足透明补齐;open/new_window/connect 复用全路径共用
    // 本入口(京东频控实测:命中频控后继续硬闯只会延长封锁,见 browser_pace 模块头)。
    // 2026-10-14 第二轮:第三参 laew_identity —— 持久熔断(总账 ≥ 阈值)只拦
    // laew 自起身份(launch 全新实例 / auto_relaunch 调试实例),不拦用户日常
    // Chrome(真实指纹,指纹级打标站点通常不抹)。
    if let Some(gated) = crate::agent::browser_pace::guard_navigation(
        url,
        bool_param(args.get("force"), false),
        open_laew_identity(str_arg(args, "connect_url")).await,
    )
    .await
    {
        return gated;
    }
    // 2026-10-14:清扫整页抹除遗留的 about:blank 死页(猎聘实测单任务残留 6 个空白
    // 窗口,占 LAEW_WEB_MAX_WINDOWS 额度又污染屏幕)。若空白页恰为全部页面,会经
    // close_page 的「最后一页回收」语义自然换全新 profile —— 正是整页抹除的恢复
    // 路径,故放在 mode 探测/new_window 分发之前,响应里的 browser_reused 才真实。
    let closed_blanks = BrowserManager::global().close_blank_pages().await;
    let reuse = bool_param(args.get("reuse"), true);
    // 多窗口并行(双站同时工作):独立新窗口路径,校验/收尾自成一支
    // (connect 禁用 / 仅 headed / 窗口上限 / 平铺代替单窗收边)。
    if bool_param(args.get("new_window"), false) {
        return run_open_new_window(args, url).await;
    }
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
                if let Ok(Ok(_)) = goto {                    // 第 100 轮:复用路径同样带单实例元数据(浏览器进程级复用 + 真实 mode)
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
                    // 2026-10-16:get_title/url 有界化(卡死 handler 上的裸 await
                    // 会无声挂死整个工具调用,见 browser_cdp 模块头)。
                    let mut title =
                        crate::agent::browser_cdp::cdp_with_timeout("open.reuse.get_title", page.get_title())
                            .await
                            .flatten()
                            .unwrap_or_default();
                    let mut final_url =
                        crate::agent::browser_cdp::cdp_with_timeout("open.reuse.url", page.url())
                            .await
                            .flatten()
                            .unwrap_or_else(|| url.to_string());
                    // 第 170 轮:复用路径同享首航空白自愈(猎聘 WAF 首击实测:
                    // 首次导航落 about:blank,同页二次导航即成功)。
                    // 2026-10-14 第三轮:已证实的抹除域(streak≥1/冷却中)跳过 ——
                    // 重导航只会再吃一次风控请求、多闪一次窗口(自愈是给瞬态首击的)。
                    let wipe_flagged = crate::agent::browser_pace::host_of(url)
                        .map(|h| {
                            crate::agent::browser_pace::WebPace::global().wipe_flagged(&h)
                        })
                        .unwrap_or(false);
                    if final_url == "about:blank" && !wipe_flagged {
                        tracing::warn!(
                            url,
                            "open 复用路径首航落 about:blank,原地重导航一次"
                        );
                        let retry = tokio::time::timeout(
                            std::time::Duration::from_millis(timeout_ms),
                            page.goto(url.to_string()),
                        )
                        .await;
                        if let Ok(Ok(_)) = retry {
                            title = crate::agent::browser_cdp::cdp_with_timeout(
                                "open.reuse.retry.get_title",
                                page.get_title(),
                            )
                            .await
                            .flatten()
                            .unwrap_or_default();
                            final_url = crate::agent::browser_cdp::cdp_with_timeout(
                                "open.reuse.retry.url",
                                page.url(),
                            )
                            .await
                            .flatten()
                            .unwrap_or(final_url);
                        }
                    }
                    // 2026-10-14:整页抹除记账(launch 与 laew relaunch 调试实例记账;
                    // 用户日常 Chrome 不记 —— 落地行为归用户,防误伤)。连续 ≥2 次
                    // 自动进 site_wipe 冷却(6004),详见 browser_pace。
                    let count_wipes = !connect_mode
                        || BrowserManager::global().connect_is_laew_relaunch().await;
                    let mut wipe_outcome = if count_wipes {
                        crate::agent::browser_pace::WebPace::global()
                            .note_navigation_outcome(url, &final_url)
                    } else {
                        None
                    };
                    // 2026-10-14 第二轮:延迟抹除复查 —— 结算时页面健康、数秒后
                    // 才被抹除的形态(猎聘实测)。有抹除史的域等 3.5s 复读落地
                    // URL;当场已落 blank 的不重复复查(避免同一次导航双记账)。
                    if !final_url.trim().eq_ignore_ascii_case("about:blank") {
                        if let Some((landed, late)) =
                            deferred_wipe_check(&page, url, count_wipes).await
                        {
                            final_url = landed;
                            title = page.get_title().await.ok().flatten().unwrap_or_default();
                            wipe_outcome = merge_wipe_outcome(wipe_outcome, late);
                        }
                        // 2026-10-14 第三轮:落地非 blank → 后台 10s 抹除看护补账
                        // (延迟抹除实测 4~10s,同步复查覆盖不到;见 schedule_wipe_watch)。
                        if !final_url.trim().eq_ignore_ascii_case("about:blank") {
                            schedule_wipe_watch(page.clone(), &pid, url, count_wipes);
                        }
                    }
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
                    if let Some(v) = site_wipe_field(wipe_outcome) {
                        payload["site_wipe"] = v;
                    }
                    // 2026-10-14 第三轮:域有抹除史时附账本快照(落地成功也要看到局势)。
                    if let Some(v) = site_wipe_ledger_field(url) {
                        payload["site_wipe_ledger"] = v;
                    }
                    if closed_blanks > 0 {
                        payload["closed_blank_pages"] = json!(closed_blanks);
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
                    // 第 161 轮:复用路径同样做主文档健康诊断(空白页证据当场透出)。
                    if let Some(v) = page_health_field(&pid, &final_url, url).await {
                        payload["page_health"] = v;
                    }
                    // 第 172 轮:复用路径同样补自刷新台账 + 登录墙 + 指纹。
                    enrich_after_navigation(&pid, &mut payload, &final_url, true).await;
                    // 第 162 轮:地理位置原生权限弹窗自动处置(复用路径同样执行;
                    // connect 模式接管用户浏览器,不干预其权限设置)。
                    if !connect_mode {
                        if let Some(v) = BrowserManager::global().auto_geo_permission(&pid).await
                        {
                            payload["permissions"] = v;
                        }
                    }
                    return envelope(0, "ok", payload);
                }
                // 2026-10-16:复用导航失败(超时/断连)——该 target 的导航状态机可能已
                // 卡死(实测 github.com/commits 网络 stall:goto 超时后 handler 侧残留
                // 挂起态,后续一切 CDP 调用排队挂死,整个任务 22 分钟无声停摆)。
                // 落新建路径前先剔除这颗死页:close_page 有界(best-effort),恰为最后
                // 一页时触发浏览器整实例回收,新建路径自然换新实例,自愈合。
                tracing::warn!(
                    page_id = %pid,
                    url,
                    "open 复用路径导航失败,剔除该页后走新建路径"
                );
                let _ = BrowserManager::global().close_page(&pid).await;
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
    let highlight = bool_param(args.get("highlight"), true);
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
            // 第 162 轮:首次接管用户浏览器时,函数开头读到的 connect_mode 还是
            // false(尚未连接),必须以 new_page 完成后的真实状态回报,否则
            // connect 模式首开的响应 connect_mode 误报 false(实测 9222 有调试
            // Chrome 时单测 open_reuse_existing_returns_3002_or_succeeds 命中)。
            let connected_now = BrowserManager::global().is_connect_mode().await;
            // 2026-10-14:整页抹除记账(launch 与 laew relaunch 调试实例记账;用户
            // 日常 Chrome 不记防误伤;new_page 内部已含「首航 blank 原地重导航
            // 一次」,此处记的是最终落地)。连续 ≥2 次自动进冷却(6004)。
            let count_wipes = !connected_now
                || BrowserManager::global().connect_is_laew_relaunch().await;
            let mut wipe_outcome = if count_wipes {
                crate::agent::browser_pace::WebPace::global()
                    .note_navigation_outcome(url, &final_url)
            } else {
                None
            };
            // 2026-10-14 第二轮:延迟抹除复查(open 返回后数秒才被抹除的形态,
            // 猎聘实测)。有抹除史的域等 3.5s 复读落地 URL;命中则改写 final_url
            // 让下方 page_health 给出 site_wipe 结构化证据,LLM 不必再瞎猜。
            let mut final_url = final_url;
            let mut title = title;
            if !final_url.trim().eq_ignore_ascii_case("about:blank") {
                if let Some(page) = BrowserManager::global().page(&page_id).await {
                    if let Some((landed, late)) =
                        deferred_wipe_check(&page, url, count_wipes).await
                    {
                        final_url = landed;
                        title = String::new();
                        wipe_outcome = merge_wipe_outcome(wipe_outcome, late);
                    }
                    // 2026-10-14 第三轮:落地非 blank → 后台 10s 抹除看护补账
                    // (延迟抹除实测 4~10s,同步复查覆盖不到;见 schedule_wipe_watch)。
                    if !final_url.trim().eq_ignore_ascii_case("about:blank") {
                        schedule_wipe_watch(page.clone(), &page_id, url, count_wipes);
                    }
                }
            }
            // 第 142 轮:connect 模式(接管外部浏览器)判定,供 guard/overlay 字段分路提示。
            // 第 143 轮:管控状态双字段(读实例真实期望态,理由见 guard_response_fields)。
            let (guard_field, overlay_field) =
                guard_response_fields(effective_mode, connected_now).await;
            let mut data = json!({
                "page_id": page_id,
                "title": title,
                "final_url": final_url,
                "reused": false,
                "mode": effective_mode.as_str(),
                "browser_reused": browser_reused,
                "highlight": highlight && effective_mode.is_headed(),
                // 第 142 轮:connect 模式 = 接管用户外部浏览器,close 只断连不关浏览器。
                "connect_mode": connected_now,
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
            // 第 161 轮:主文档健康诊断 —— 空白页(主文档 4xx/5xx 或空 DOM)当场
            // 透出证据与处置,Agent 不必再盲试 reload 才发现不可达。
            if let Some(v) = page_health_field(&page_id, &final_url, url).await {
                data["page_health"] = v;
            }
            // 第 172 轮:自刷新台账 + 登录墙 + 环境指纹(首航即可见被反爬的迹象)。
            enrich_after_navigation(&page_id, &mut data, &final_url, true).await;
            if let Some(v) = site_wipe_field(wipe_outcome) {
                data["site_wipe"] = v;
            }
            // 2026-10-14 第三轮:域有抹除史时附账本快照(落地成功也要看到局势)。
            if let Some(v) = site_wipe_ledger_field(url) {
                data["site_wipe_ledger"] = v;
            }
            if closed_blanks > 0 {
                data["closed_blank_pages"] = json!(closed_blanks);
            }
            // 第 162 轮:地理位置原生权限弹窗自动处置 —— 导航完成后对页面 origin
            // CDP 预授权,弹窗永不出现(connected_now 覆盖首次接管场景,不干预
            // 用户浏览器的权限设置)。
            if !connected_now {
                if let Some(v) = BrowserManager::global().auto_geo_permission(&page_id).await {
                    data["permissions"] = v;
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
            } else if is_connection_level_failure(&msg) {
                // 第 172 轮:连接级失败归一 —— 底层 futures 的 `oneshot canceled`
                // /「Session with given id not found」对模型毫无信息量,实测
                // (2026-10-14 对照任务)模型只能原样重试 3 次全灭。这里:
                // ① 换成可行动的中文文案 + 明确「重试一次通常即可」;
                // ② 就地清掉可能已死的句柄,保证模型的下一次 `open` 是真冷启动。
                BrowserManager::global().drop_dead_browser().await;
                envelope(
                    2001,
                    "浏览器实例握手/连接失败(已自动清理,重试一次通常即可恢复)",
                    json!({
                        "reason": "connection_level",
                        "detail": msg,
                        "retryable": true,
                        "remedy": "直接重试同一条 open(不换 URL、不改参数);\
若连续两次同样失败:① 确认没有残留的调试 Chrome 占着复用端口;\
② 改 open(reuse_existing=true, auto_relaunch=true) 让工具复制登录态另起实例;\
③ 仍失败则如实告知用户「浏览器启动异常」,不要反复重试同一参数",
                    }),
                )
            } else {
                envelope(2001, &msg, nav_failure_diagnose(&msg))
            }
        }
    }
}

/// run_open_inner 的多窗口分支(`open(new_window=true)` / `control(new_window)` 共用)。
///
/// 校验链:connect 模式不支持(不干预用户浏览器) → 仅 headed → 窗口数上限
/// (`LAEW_WEB_MAX_WINDOWS`,按去重 windowId 计)→ launch/复用实例 →
/// `new_page_in_new_window`(CDP `Target.createTarget newWindow:true`)。
/// 成功收尾与同窗口 open 对齐(管控/geo/台账/健康诊断),但**不做**单窗
/// `fit_headed_window`(多个 1920×1080 互相遮挡会被 occlusion 节流,与后台
/// Tab 挂起同病),改为 `tile_agent_windows` 横向平铺全部 Agent 窗口。
async fn run_open_new_window(args: &Value, url: &str) -> Result<String> {
    let mgr = BrowserManager::global();
    // connect 模式(接管用户浏览器)整路不干预:新开窗口违反「零注入」原则。
    if str_arg(args, "connect_url").is_some() || mgr.is_connect_mode().await {
        return envelope(
            1001,
            "new_window 不支持 connect 模式(接管用户浏览器,不新开窗口)",
            json!({"new_window": true}),
        );
    }
    let requested_mode = resolve_requested_mode(args);
    // 单实例语义:实例已存在时以实例现值为准;新窗口仅 headed 有意义
    // (无窗口模式没有「同时显示」概念,headless shell 也不支持 newWindow)。
    let effective_mode = match mgr.current_mode().await {
        Some(existing) => existing,
        None => requested_mode,
    };
    if !effective_mode.is_headed() {
        return envelope(
            1001,
            "new_window 仅 headed(可见窗口)模式可用",
            json!({
                "new_window": true,
                "mode": effective_mode.as_str(),
                "hint": "多站点同时工作需要可见窗口:以 headed 打开第一个站点,再对其余站点 new_window=true",
            }),
        );
    }
    // 窗口数上限(浏览器未启动时 0,不挡首开)。
    let existed = mgr.has_browser().await;
    let windows_now = mgr.count_windows().await;
    let cap = max_windows_from_env();
    if windows_now >= cap {
        return envelope(
            1001,
            "Agent 窗口数已达上限",
            json!({
                "new_window": true,
                "windows": windows_now,
                "max_windows": cap,
                "env": "LAEW_WEB_MAX_WINDOWS",
                "hint": "先 close(page_id=…) 关闭闲置页面(窗口内最后一页关闭时 Chrome 自动收窗),或提高上限后重试;窗口都被其它 Agent 占用时可改用 open(url)(不带 new_window,在既有窗口开新 Tab;后台 Tab 渲染挂起,操作前 list 查 foreground / control(bring_to_front) 前台化)继续工作,不要原地反复重试",
            }),
        );
    }
    let ua = str_arg(args, "user_agent");
    // 窗口尺寸(显式参数 clamp 到安全区间;缺省由驱动层决定)。
    let window_size = parse_window_size(args);
    let highlight = bool_param(args.get("highlight"), true);
    let guard = match parse_open_guard(args) {
        Ok(g) => g,
        Err(e) => {
            return envelope(
                1001,
                &e,
                json!({
                    "guard": "locked|open|partial",
                    "hint": "partial 需要 allow_selectors(白名单)或 block_selectors(黑名单)之一,两者互斥且各 ≤8 条",
                }),
            )
        }
    };
    match mgr
        .new_page_in_new_window(url, effective_mode, ua, window_size, highlight, guard)
        .await
    {
        Ok((page_id, title, final_url)) => {
            if str_arg(args, "wait_until") == Some("networkidle") {
                // 与 run_open_inner 同构:Chrome networkidle 判定已含 500ms 静默窗,
                // 额外 700ms 足够让迟到的渲染提交完成。
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            }
            let (guard_field, overlay_field) = guard_response_fields(effective_mode, false).await;
            let mut data = json!({
                "page_id": page_id,
                "title": title,
                "final_url": final_url,
                "reused": false,
                "new_window": true,
                "mode": effective_mode.as_str(),
                "browser_reused": existed,
                "highlight": highlight && effective_mode.is_headed(),
                "connect_mode": false,
                "guard": guard_field,
                "overlay": overlay_field,
                "window": {
                    "requested": window_size.map(|(w, h)| [w, h]),
                    "default": [1920, 1080],
                    "min_viewport": [1280, 720],
                    "hint": "多窗口并行:本页在独立新窗口打开;各 Agent 窗口互不遮挡时页面同时活跃、互不挂起。运行时调整 control(set_window);人工拖动窗口后 control(sync_viewport)",
                },
                "next_steps": open_next_steps(),
            });
            if let Some(arr) = args.get("block_resources").and_then(Value::as_array) {
                data["block_resources_hint"] = json!(arr);
            }
            // 多窗口收尾:横向平铺全部 Agent 窗口(防互相遮挡被 occlusion 节流,
            // 取代单窗 open 的 fit_headed_window;平铺报告供对账)。
            if let Some(v) = mgr.tile_agent_windows().await {
                data["windows"] = v;
            }
            // 视口自适应(内容超视口时扩展到 2K 上限)。
            if auto_expand_enabled(args) {
                match fit_viewport(&page_id).await {
                    Some(v) => data["viewport"] = v,
                    None => {
                        data["viewport"] =
                            json!({"expanded": false, "note": "视口测量失败(不影响页面操作);需要时手动 control(set_viewport)"})
                    }
                }
            }
            // 主文档健康诊断(空白页证据当场透出)。
            if let Some(v) = page_health_field(&page_id, &final_url, url).await {
                data["page_health"] = v;
            }
            // 第 172 轮:new_window 路径同样补自刷新台账 + 登录墙 + 指纹。
            enrich_after_navigation(&page_id, &mut data, &final_url, true).await;
            // 2026-10-14:整页抹除记账(本路径已拒绝 connect 模式,恒记)。
            let wipe_outcome = crate::agent::browser_pace::WebPace::global()
                .note_navigation_outcome(url, &final_url);
            if let Some(v) = site_wipe_field(wipe_outcome) {
                data["site_wipe"] = v;
            }
            // 地理位置原生权限弹窗自动处置(connect 已早退,恒非接管)。
            if let Some(v) = mgr.auto_geo_permission(&page_id).await {
                data["permissions"] = v;
            }
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
            } else if is_connection_level_failure(&msg) {
                // 第 172 轮:连接级失败归一 —— 底层 futures 的 `oneshot canceled`
                // /「Session with given id not found」对模型毫无信息量,实测
                // (2026-10-14 对照任务)模型只能原样重试 3 次全灭。这里:
                // ① 换成可行动的中文文案 + 明确「重试一次通常即可」;
                // ② 就地清掉可能已死的句柄,保证模型的下一次 `open` 是真冷启动。
                BrowserManager::global().drop_dead_browser().await;
                envelope(
                    2001,
                    "浏览器实例握手/连接失败(已自动清理,重试一次通常即可恢复)",
                    json!({
                        "reason": "connection_level",
                        "detail": msg,
                        "retryable": true,
                        "remedy": "直接重试同一条 open(不换 URL、不改参数);\
若连续两次同样失败:① 确认没有残留的调试 Chrome 占着复用端口;\
② 改 open(reuse_existing=true, auto_relaunch=true) 让工具复制登录态另起实例;\
③ 仍失败则如实告知用户「浏览器启动异常」,不要反复重试同一参数",
                    }),
                )
            } else {
                envelope(2001, &msg, nav_failure_diagnose(&msg))
            }
        }
    }
}

/// 列出当前存活页面(原 BrowserList);返回前自动清理失效 entry。
/// 多标签页感知:逐页即时探测前后台(`foreground`,人工切页会实时变化,不缓存),
/// 页数达到阈值附 `tabs_hint` 收口提醒;`LAEW_WEB_BACKGROUND_GATE=off` 时跳过探测
/// (foreground 恒 null,保持旧观感)。
async fn run_list(_: Value) -> Result<String> {
    let mgr = BrowserManager::global();
    let raw = mgr.list_pages().await;
    let probe_enabled = page_activity::gate_enabled_from_env();
    let mut pages = Vec::with_capacity(raw.len());
    // 多窗口并行:逐页探测 CDP windowId,Agent 一眼看出「哪几页在同一窗口」
    // —— 不同 window_id 的页面分处独立可见窗口,可同时活跃工作。
    let mut window_ids: Vec<i64> = Vec::new();
    for (id, url, title, ts) in raw {
        let foreground = if probe_enabled {
            match mgr.page(&id).await {
                Some(p) => page_activity::probe_foreground(&p)
                    .await
                    .map(|a| a.foreground()),
                None => None,
            }
        } else {
            None
        };
        let window_id = mgr.page_window_id(&id).await;
        if let Some(w) = window_id {
            if !window_ids.contains(&w) {
                window_ids.push(w);
            }
        }
        pages.push(json!({
            "page_id": id, "url": url, "title": title,
            "created_at": ts, "foreground": foreground,
            "window_id": window_id,
        }));
    }
    let n = pages.len();
    let mut data = json!({"pages": pages, "count": n, "windows": window_ids.len()});
    if let Some(hint) = page_activity::tabs_hint(n) {
        data["tabs_hint"] = hint;
    }
    // docs/MCP_Web_Use/25:单站点堆积感知 —— 同 host 存活页 >3 时提醒收口
    // (与 tabs_hint 的全局总量阈值互补:同站多 Tab 是最典型的失控形态)。
    let host_entries: Vec<(String, String)> = {
        let raw = mgr.list_pages().await;
        raw.iter()
            .map(|(id, url, _, _)| {
                (
                    id.clone(),
                    crate::agent::browser_pace::host_of(url).unwrap_or_default(),
                )
            })
            .collect()
    };
    if let Some(alert) = page_activity::origin_tabs_alert(&host_entries) {
        data["origin_tabs_alert"] = alert;
    }
    envelope(0, "ok", data)
}

/// 关闭指定 page_id(原 BrowserClose);最后一个页面关闭时回收内部浏览器进程
/// (launch 模式)。幂等。
/// 第 99 轮:`page_id="all"` 一键清场(关闭全部页面 + 回收浏览器,任务收尾用)。
/// 多标签页治理:`page_id="background"` 批量关闭全部后台页(至少保留一个前台页;
/// 探测不到前台页时拒绝,防止窗口最小化时一锅端误杀)。
async fn run_close(args: Value) -> Result<String> {
    let Some(id) = str_arg(&args, "page_id") else {
        return envelope(1001, "缺少 page_id", json!({}));
    };
    if id == "all" {
        // 2026-10-14 第二轮:当前 connect 的是 laew 自起的 auto_relaunch 调试
        // 实例时,close(all) 连同进程一起回收 —— 它是 laew 身份(laew 对自己
        // 拉起的实例有生杀权,doc 08 红线),只断连会残留空白 Chrome 窗口在用户
        // 桌面并继续占用调试端口(实测同时残留 2 台)。用户日常 Chrome 仍只断连。
        let relaunch_pid = if BrowserManager::global().connect_is_laew_relaunch().await {
            crate::agent::browser_reuse::relaunch_instance_pid()
        } else {
            None
        };
        let n = BrowserManager::global().list_pages().await.len();
        BrowserManager::global().shutdown().await;
        if let Some(pid) = relaunch_pid {
            crate::agent::browser_reuse::kill_relaunch_browser(pid);
        }
        // 2026-10-14:回收浏览器 = 全新 profile(新 cookie/新指纹)= 全新会话身份,
        // 原会话的频控/抹除**连击与冷却**清零,紧随其后的 open 不被自己的熔断
        // 卡住;**抹除总账 wipe_total 保留**(clear_all 不清)—— 指纹级打标站点
        // 换新实例照样被抹,总账若随之清零会形成「拉起 Chrome → 被抹 → close →
        // 再拉起」的风暴(实测 9 分钟拉起 8 台,见 browser_pace 模块头 4.)。
        crate::agent::browser_pace::WebPace::global().clear_all();
        let mut data = json!({"page_id": "all", "closed": n});
        if relaunch_pid.is_some() {
            data["relaunch_instance_killed"] = json!(true);
        }
        return envelope(0, "closed", data);
    }
    if id == "background" {
        return match page_activity::close_background_pages().await {
            Ok(closed) => envelope(
                0,
                "closed",
                json!({"page_id": "background", "closed": closed, "count": closed.len()}),
            ),
            Err(reason) => envelope(2002, &reason, json!({"page_id": "background"})),
        };
    }
    // docs/MCP_Web_Use/25:others —— 关闭除 keep 外全部页面(收口「留一个工作页」
    // 的精确表达;keep 必须显式给出且存活,防一锅端)。
    if id == "others" {
        let keep = str_arg(&args, "keep").unwrap_or("");
        if keep.is_empty() {
            return envelope(
                1001,
                "close(page_id=\"others\") 必须同时给 keep=\"<保留的page_id>\"",
                json!({"page_id": "others"}),
            );
        }
        return match page_activity::close_pages_except(keep).await {
            Ok(closed) => envelope(
                0,
                "closed",
                json!({"page_id": "others", "keep": keep, "closed": closed, "count": closed.len()}),
            ),
            Err(reason) => envelope(2002, &reason, json!({"page_id": "others", "keep": keep})),
        };
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
/// 第 171 轮:`kind=login` 的关键词命中**不中断** —— 登录墙是「状态疑似」而非
/// 「输入黑洞」,与验证码弹窗不同类;实测京东频控降级壳页头渲染登出样式
/// (「你好,请登录」)而登录态实际在,6 次误中断整批 sequence 剩余步骤。
pub(super) fn step_has_blocker_alert(data: &Value) -> bool {
    match data.get("blocker_alert").and_then(|a| a.get("kind")).and_then(Value::as_str) {
        Some("login") => false,
        _ => data.get("blocker_alert").is_some(),
    }
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

/// 批量步骤 message 截断(第 157 轮,纯函数可单测):成功步 300 字符、失败步
/// 2000;2002 的 ExceptionDetails 巨串实测单个 ~1KB,逐步原样回填会反复灌爆
/// 上下文,失败步保留足够诊断长度、成功步只留结论。
fn clip_step_message(code: i32, message: &str) -> String {
    let budget = if code == 0 { 300 } else { 2000 };
    let mut chars = message.chars();
    if message.chars().count() <= budget {
        return message.to_string();
    }
    let kept: String = chars.by_ref().take(budget).collect();
    format!("{}…(截断,省略{}字符)", kept, message.chars().count() - budget)
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
///
/// 2026-10-13:整段包进 `IN_SEQUENCE` scope —— 批内步骤不走 auto-HITL 自动弹出
/// (150s 单步预算会截断人工等待),见 [`in_sequence_ctx`] 文档。
async fn run_sequence(args: Value) -> Result<String> {
    IN_SEQUENCE.scope(true, run_sequence_inner(args)).await
}

async fn run_sequence_inner(args: Value) -> Result<String> {
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
    // 2026-10-14:未解析的 `$` 前缀占位符不作 page_id —— 占位符只认 $page_id/
    // $spawned_page_id(下方 resolve_page_placeholders),LLM 编造的 `$NEW` 等
    // (猎聘任务 iter18 实测)若被当真实 page_id 继承,后续步骤全部 1001 整批污染。
    let mut current_page = str_arg(&args, "page_id")
        .map(str::to_string)
        .filter(|s| !s.starts_with('$'));
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
        // 第 157 轮:step 缺 page_id 时自动继承(顶层 page_id → 上一步 page_id →
        // 派生页)。实测 LLM 在顶层传了 page_id、steps 内逐步漏传 → 每步
        // 1001 缺少 page_id 整批全灭,白烧一轮 LLM 迭代;显式 step page_id 仍优先。
        if step.get("page_id").is_none() {
            if let Some(cur) = current_page.clone() {
                step["page_id"] = Value::String(cur);
            }
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

        // 第 157 轮:批量结果瘦身 —— 成功步 message 截 300 字符,失败步截 2000;
        // 2002 的 ExceptionDetails 巨串实测单个 ~1KB,逐步原样回填会反复灌爆上下文。
        let step_message = clip_step_message(code, &message);
        results.push(json!({
            "index": index + 1,
            "action": action,
            "code": code,
            "message": step_message,
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
            first_error = Some((code, clip_step_message(code, &message)));
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

/// 任务收口标签页治理 facade(`docs/MCP_Web_Use/25`):编排器任务边界调用,
/// 关闭本任务新开的后台标签页(前台页保留)。编排器不直接摸浏览器,经本
/// facade 进 `page_activity::finalize_task_tabs`;无浏览器实例/开关关闭时
/// 零开销,fail-open 绝不阻断任务收口。
pub async fn finalize_task_tabs() -> serde_json::Value {
    page_activity::finalize_task_tabs().await
}

/// 任务开始:快照标签页基线(供收口 diff「本任务新开的页」)。
pub async fn begin_task_tab_ledger() {
    crate::agent::tab_ledger::begin_task().await;
}

/// 浏览器网页操控统一入口(MCP 风格单工具 + action 分发)。
pub struct McpWebUseTool;

/// 工具描述:同时承担「使用说明」职责(原 Chromium-WebUse Agent 系统提示词的
/// 作业规范部分精炼,全文见 SubAgent-Work 系统提示词的 MCP_Web_Use 段)。
const MCP_WEB_USE_DESCRIPTION: &str = r#"通过 CDP 驱动 Chromium 系浏览器操作网页(macOS / Windows / Linux,内存无头浏览器默认,也可接管已开浏览器;MCP 风格单工具多 action)。
用 action 参数选择操作:
- open(url*, mode?, reuse?, connect_url?, user_agent?, stealth?, wait_until?, window_width?, window_height?, auto_expand_viewport?, highlight?, guard?, allow_selectors?, block_selectors?, guard_note?, overlay?, timeout_ms?, reuse_existing?, auto_relaunch?): 启动/接管 Chromium 并打开页面。**默认可见窗口(mode=headed,第 139 轮)**——不要为了「省资源」主动传 hidden,除非任务明确要求静默后台跑;无 GUI 会话(CI/容器/SSH)会自动回退 hidden,可用 LAEW_BROWSER_MODE 强制。使用一次性临时 profile,不干扰用户日常浏览器;**headed 缺省把窗口铺满屏幕工作区并置前**(响应 data.headed_window.fill_ratio 回报实际铺满率,勿再自行传 window_width/window_height —— 显式指定会关闭铺满、改为只做「不溢出」收边);highlight=true(默认)时 headed 窗口页面四周显示一圈蓝色选中边框,右上角显示「屏蔽页面状态栏」(🔵 LAEW Agent 控制中 · 具体 Agent 名 / 当前管控档位 / 注意事项 / guard_note)、左下角显示「屏蔽页面进度栏」(当前动作 + 迭代进度 + 任务摘要 + 最近事件),人工可一眼看清谁在控制、正在干什么;set_highlight(false) 一并隐藏两条栏;**页面管控三档(第 143 轮,guard 参数,缺省 locked)**:locked=屏蔽模式(半透明蒙层+锁定人工输入,人工可实时观看不可点击,防交叉操作;Agent 输入动作自动「先解后锁」不受影响,人工交互一律走 request_human 默认自动解锁)/ open=非屏蔽模式(人工可直接操作页面,屏蔽页面状态栏明示「🔓 页面开放」,用于人工亲自操作、人工登录先行流)/ partial=部分屏蔽模式(须同时给 allow_selectors 白名单「只有命中区人工可操作」或 block_selectors 黑名单「命中区人工不可操作」之一,互斥,各 ≤8 条;盾区内人工不可操作、盾区外放行,Agent 动作自动「先隐盾后复盾」);guard_note(≤60 字符)写进屏蔽页面状态栏第四行引导人工(如「请登录后应答弹窗,Agent 将接管后续操作」);缺省随 LAEW_WEB_GUARD;仅 headed 生效;运行时切换 control(set_guard);legacy overlay 布尔参数保留(true≡locked/false≡open,显式传 guard 时被忽略);timeout_ms 控制页面加载超时(毫秒,默认 60000,内网慢速网站可加大)。connect_url 接管已用 --remote-debugging-port 启动的浏览器。**复用已登录浏览器(第 142 轮)**:reuse_existing=true 自动探测本机调试端口(默认 9222,LAEW_CHROME_DEBUG_PORT 可覆盖;依次尝试 localhost/[::1]/127.0.0.1 —— Chrome 154+ 的 DevTools HTTP 端点只服务 IPv6 loopback 连接,IPv4 返回 404)并接管用户已登录的 Chrome(保留 Cookie/登录态),响应 data.reuse_existing_chrome=true + data.debug_port + data.browser_version + data.connect_mode=true;探测失败返回 code=3002 + data.relaunch_command(平台相关重启命令,转述用户执行,或 auto_relaunch=true 重试,由工具自动复制登录态并另启独立调试 Chrome 实例,不退出、不影响用户已打开的浏览器);connect 模式 close 只断连不关用户浏览器,默认不锁人工输入(管控恒为 open 且零注入,需要时 set_guard 手动开)。同 URL 已有存活页面时默认复用(导航刷新,响应 reused:true 且 page_id 不变;reuse=false 强制新开);**同 origin 兜底复用(第 146 轮)**:精确未命中但存在同 scheme+host+port 的存活页面(取最早打开的主工作页)时,在该页面上原地导航复用(响应 reuse_match=origin + previous_url),长遍历任务不再因路由不同新开冗余标签页、登录态与页面上下文不丢失;connect 模式(接管用户浏览器)不做 origin 兜底,绝不导航用户自己的标签页;浏览器实例已存在时永远复用同一进程(响应 browser_reused:true + 真实 mode),不重复打开多个浏览器。**窗口收边(第 139 轮,默认开)**:headed 模式下导航完成后按屏幕工作区自动收窄窗口(小屏笔记本不再把窗口挤出屏外「显示不全」),并保证页面视口不低于 720p,结果回 data.headed_window。**视口自适应(默认开)**:窗口铺满后页面视口即随窗口自适应(下限 720p);若页面内容仍超出视口(横向被裁/可视高度不足),再把视口扩展到 ≤2560×1440(2K)并回 data.viewport(expanded/from/to/content/clamped/hint)——根治「视口太窄页面显示不全、元素不可见不可点」;auto_expand_viewport=false 可关;内容仍超 2K 上限时按 hint 走 full_page 截图或 set_viewport 显式超限。**主文档健康诊断 data.page_health(第 161 轮)**:页面空白(主文档状态 ≥400 或空 DOM)时响应附带 {blank, reason=main_doc_error|empty_dom, main_doc_status, note, next_action}——main_doc_error 表示服务器/WAF 拒绝(reload 无效),按 next_action 改道 open(reuse_existing=true) 复用已登录浏览器或 request_human,不要反复 reload;页面正常时不带该字段。**地理位置授权自动处置 data.permissions(第 162 轮)**:open/navigate 导航完成后对页面 origin 自动预授权,Chrome 原生地理位置弹窗不再出现(不再拖住任务等人工点选);响应附带 {geolocation: granted|denied|skipped, mode, origin},缺省 grant,可用 LAEW_WEB_GEO=deny/prompt 改档,connect 模式(接管用户浏览器)不干预。返回 {page_id,title,final_url,reused,mode,browser_reused,connect_mode,window,guard,overlay(legacy),headed_window?,viewport?,page_health?,next_steps}。未检测到浏览器返回 code=3001(确定性失败,如实告知用户安装引导,不要重试)。**反爬三信号(第 172 轮,open/navigate/inspect/control 响应可能附带)**:① `data.login_wall{login_url,evidence,next_action}` —— 服务端把页面 302 到登录页时透出(navigate 另有 requested_url/final_url/redirected 三件套:落地≠请求即 redirected=true);看到这个就直接 `control(request_human, reason="login")`,**不要反复 navigate 试探**;② `data.page_liveness{unstable,reason=self_reload_loop|self_reload_burst,self_reloads,advice}` —— 页面被站点反复重载/自愈(每次都销毁页面内已输入内容,人工也输入不了);处置:换稳定页面登录,或 `open(reuse_existing=true)` 接管用户已登录的真实浏览器,**不要反复 reload**;③ `data.fingerprint{suspicious:[{code,advice}],...}` —— 环境指纹体检(WebGL 兜底串/webdriver/插件数等);suspicious 非空表示该环境在风控眼里可疑,关键任务优先用真实浏览器指纹。④ 连接级失败(底层 `oneshot canceled` 等)已归一为可行动文案并自动清句柄,**原样重试同一条 open 一次通常即可恢复**,不要换参数乱试。杀开关 LAEW_WEB_FINGERPRINT=off(连指纹体检带 WebGL 保真注入一起关)。**反检测档位 data.stealth(第 174 轮)**:响应附带 {mode,fidelity,hardened,kill_switch} 回报本轮生效档位 —— fidelity=非侵入式(缺省,只移除自动化泄漏+回填本机真值)/hardened=侵入式(保真+被替换函数身份原生化)/off=不处置;`open(stealth=…)` 或 `control(set_stealth, mode=…)` 可切档,缺省不用传;多数站点无需调整,仅在风控明确认得自动化特征时升 hardened,排障时切 off 做对照。**行为仿真/IP 代理/打码/协议逆向一律不做**(见 docs/MCP_Web_Use/26 §三:前两者实测无效且越界,后两者摧毁「浏览器轨迹即证据」的 QC/审计根基)。**站点导航节奏闸(第 171 轮)**:open/navigate/new_window 显式导航共用 —— 同一注册域两次导航间隔不足缺省 2.5s(LAEW_WEB_PACE_MIN_INTERVAL_MS)时透明补齐等待;任何探针命中频控/限流标记(「由于访问频繁导致无法搜索」「请求频繁」类横幅)后该域自动进入冷却(缺省 240s,LAEW_WEB_RATE_COOLDOWN_SECS),冷却期内同域导航返回 **6004**(附 data.host/remaining_secs/advice/force_hint,动作未执行)—— 此时不要换 URL/关键词硬闯(高频只会延长封锁),先做本地整理、落盘或其它页面工作,等冷却结束低频重试;剩余 ≤45s 时工具自动等待放行;确需立即导航加 params.force=true;LAEW_WEB_PACE=off 全关。
- list(): 列出当前存活页面 [{page_id,url,title,created_at,foreground}];返回前自动清理失效 entry。foreground=该页是否为其窗口的当前激活标签页(即时探测不缓存,人工切页后以最新 list 为准);页数较多时附 tabs_hint 收口提醒;**同站点标签页堆积(同 host >3)时附 origin_tabs_alert**(host/tabs/page_ids/hint——同站多 Tab 是最典型的失控形态,看到后应立即收口)。冷启动后多轮任务优先用它同步页面索引。
- close(page_id*): 关闭指定页面;最后一个页面关闭时回收浏览器进程。page_id="all" 一键关闭全部页面并回收浏览器(任务收尾清场);page_id="background" 批量关闭全部后台标签页(至少保留一个前台页;探测不到前台页时拒绝执行);**page_id="others" 关闭除 keep 外全部页面(收口「留一个工作页」的精确表达,keep 必须显式给出且存活,防一锅端)**。幂等;对话型页面(用户可能继续追问)可保留复用。
- control(page_id*, control_action*, params?): 全部写操作统一入口。control_action 枚举:click/human_click/right_click/double_click/hover/scroll/scroll_to/scroll_to_text/key_press/press_sequence/input_text/human_input/clear_input/select_option/select_dropdown/upload_file/download/new_tab/new_window/close_tab/bring_to_front/navigate/back/forward/reload/wait/eval_js/set_cookie/delete_cookie/set_storage/clear_storage/set_viewport/screenshot/heartbeat/drag/focus/blur/mouse_move/dispatch_event/set_window/sync_viewport/set_highlight/set_overlay/set_guard/auto_follow/request_human。点击链接/new_tab 派生的新标签页经响应 spawned_page_id 回传,后续操作新页面必须用新 page_id;派生页通常抢走前台,原页面转后台时响应附 data.page_background 标注。new_window(params.url 必填)在**独立新窗口**打开页面并直接返回新 page_id(完整 open 语义:锚点守卫/管控/台账/健康诊断),是多站点同时工作的标准动作(见【多窗口并行】)。bring_to_front 把指定标签页带到前台(标签页级激活,与元素级 focus 区分),是后台页做 screenshot/ocr 返回 6003 时的标准对策。screenshot 一律落盘返回 save_path(看图片文字用 params.ocr=true,文本模型无法消费 base64);eval_js 直接写表达式,支持 return 与多语句(失败自动 IIFE 重试),超长返回值自动落盘并以 saved_to 引用;单次执行有超时兜底(默认 30s,await_promise=true 时 60s,可用 params.timeout_ms 指定、上限 120s——页内 Promise 轮询等合法长等待必须显式传 timeout_ms),超时返回 2002 并附「页面可能弹出人工验证」的处置指引;download 支持 http(s) url 或 selector、save_dir、filename、timeout_ms,data: URL 直接解码落盘,完成后返回绝对 save_path 与 byte_size。set_window 运行时调整真实浏览器窗口(width/height/left/top/window_state=maximized|fullscreen|minimized|normal,CDP setWindowBounds,调整后自动清除视口覆盖保证渲染自适应不缺区域);sync_viewport 在人工拖动窗口大小后调用,清除 device metrics 覆盖使视口=窗口内容区(会撤销 open 时的自动 2K 扩展);set_stealth(mode?="fidelity"|"hardened"|"off") 运行时切换反检测档位(第 174 轮:非侵入式保真 / 侵入式原生化 / 不处置;全局作用于全部存活页面;升档即时生效,降档只对新文档生效——已执行的覆写无法回退);set_viewport(width,height,device_scale_factor?=1,mobile?=false) 手动设置布局视口(宽 320~7680/高 240~4320 自动 clamp;open 已默认自动扩展视口,仅当自动结果不理想时才手动指定);set_highlight(enabled) 运行时开关蓝色选中边框;auto_follow(enabled?=true | scroll_to_bottom?=true) 第 152 轮新增的「自动跟随最新内容」开关——多轮对话站(聊天/论坛/工单)消息栈不断变长时,人工在 locked 档**无法自行滚动**(CDP Input.setIgnoreInputEvents 连滚轮一并拦),必须由页面自己停在最新;跟随器仅在「用户本来就在底部」时吸底,人工往上翻历史不会被拽回;scroll_to_bottom=true 表示不改开关、立刻吸底一次;导航后按期望态自动重挂;set_guard(mode?="locked"|"open"|"partial", allow_selectors?, block_selectors?, note?) 运行时切换页面管控三档(第 143 轮,推荐;字段缺省=保持现值,空数组/空串=清除;partial 校验同 open);set_overlay(enabled) 蒙层开关 legacy 别名(true=set_guard(locked),false=set_guard(open),仅 headed 生效);request_human(reason=captcha|sms|qr_login|login|real_name|two_factor|oauth|manual_verify|custom, message?, options?, multi_select?=false 选项多选(弹窗渲染为复选框组,点「继续」收集全部勾选,应答形如 `1. 选项a；3. 选项c`;缺省单选=点选项即应答), timeout_ms? 缺省按 reason 分档 captcha/sms/two_factor=120s 其余 300s, bring_to_front?=true, unlock_page?=true 提问期间自动放行页面供人工直接操作(第 167 轮:缺省**整页 open 安全全面解除屏蔽**——人工自由操作,完成后回弹窗点『继续』交还控制权;显式传 allow_selectors? ≤8 条才走 partial 白名单挖洞,自动并入挑战层选择器+覆盖验证,洞没罩住挑战即回退整页 open;应答/超时/取消后自动恢复该页解锁前管控档;页面本就 open 或未装管控则无需动作;纯问答场景传 false;提问期间人工提交表单触发导航会自动重放放行态,多步登录不断链)) 人工介入:登录、滑块/短信验证码/扫码、实名认证、人脸核身、2FA、第三方 OAuth、中途人工核验、让人工做选择题(单选/多选)或给补充指示等一切无法自动跳过的流程。macOS/Windows 桌面自动弹出人工介入弹窗(置顶+倒计时+时间轴,文案可鼠标选中复制;人工在弹窗点选项(单选)/勾选项(多选)/在输入框打字给 Agent 补充说明/取消,-p 模式同样可弹;主按钮「继续」双语义——有输入=把补充说明交给 Agent,多选=收集勾选,空输入=选项 1(人工在浏览器完成操作后的交棒动作);「⏱ +2分钟」延长等待总上限 30 分钟;「🔓 解除屏蔽/🔒 恢复屏蔽」互切页面管控;TUI 兜底行读),code=0 时 human_response 为人工回答(assist_channel 标注 gui/tui;next_hint 按「输码 vs 已完成」分流——自由文本验证码立即 input_text 填入提交,选项应答才 inspect 验证),人工取消返回 code=4002,超时或弹窗与 TUI 均不可用返回 code=4001(如实告知用户改用交互模式重试,严禁伪造结果),**4003=人工介入进行中**(提问未结束时页面归人工所有,输入类动作与可变更 eval_js 会被挂起,人工应答后重试即可,确认无并发人工操作可加 params.force=true)。
- inspect(page_id*, info*, params?): 全部只读观察统一入口。info 枚举:console(控制台输出)/network(请求响应流)/elements(元素文本与矩形;params.selector 可选,缺失时默认返回 input/button/select/textarea/a/[role=button] 等全页交互元素)/dom(outerHTML 或节点树)/localstorage/sessionstorage/cookies/screenshot/page_meta/viewport(视口+内容尺寸与 overflow 溢出判定:横向溢出=页面显示不全需 set_viewport/重开 open 自动扩展,纵向溢出截图用 full_page=true)/url/title/ping/image_urls/ocr(截图+OCR 识别图片文字,验证码/图表标签用;region 过滤词块)/blockers(启发式检测验证码/短信/扫码/登录墙等人工阻断,**含弹层语义挑战检测(第 147 轮)**:图片选择「选出在公园能看到的事物」/滑块/语义题等题面无验证码关键词的形态靠弹层结构+文案识别,返回 blockers[]+suggested_action=request_human)/extract_links(批量提取页面所有链接,返回 links[{href,text,context,is_external}]+total+truncated+scanned+hostname;params.selector 默认 "a" 可选,params.max_links 默认 200 上限 500,params.include_context 默认 true 含文章前后文供时间推断)/extract(【第 135 轮,列表/表格抓取首选】一次调用把列表页压成结构化条目并可在页面内完成过滤,只回精简字段)/page_state(【第 135 轮】读取页面 SSR 注水数据与 JSON-LD,列表数据藏在全局变量时先读它)/coverage(【第 146 轮,第 163 轮修正 key 口径】遍历覆盖台账:工具层自动记录 open/navigate/new_tab 的全部导航,返回 summary{distinct_pages,total_visits}+top_repeats+pages 清单;**遍历类任务开工/收口必查,未访问页面优先,防同页打转**;进程级全局事实,不依赖存活页面。key 归一只折叠**翻页/游标参数**(page/pn/offset/cursor…),保留决定页面身份的 query(如 `?q=` 搜索词、`?cat=` 类目、`?city=` 城市)——旧口径丢弃全部 query,会把搜索/分类驱动站点的几十个真实页面塌缩成 1 个,曾导致 QC 对真实遍历过的单元做出错误指控)。
【extract 详解(抓文章列表/新闻流/商品列表优先用它,不要手写 eval_js 猜字段)】params:item_selector*(列表项根节点 CSS 选择器,不知填什么先 probe=true)、fields(字段投影,形如 {"title":{"selector":"h3 a","required":true},"time":{"selector":"time","attr":"datetime"},"summary":{"selector":".descript","max_chars":300}},省略则只回通用 text+__url)、url_from(取链接的选择器,默认 "a",结果落在 __url)、limit(默认 200 上限 1000)、scan_cap(内部扫描上限,默认 1000)、filter{keywords[],match_all,fields[],time_field,since,until,sort("time:desc"),limit_after_filter}。**数值过滤/排序(通用,不限价格)**:filter.numeric{field?,min?,max?} —— 从字段文本抽取第一个数值(如 "¥10,999" / "32G" / "1.2万")按闭区间过滤;解析不出数字的条目**保留**(fail-open),响应 unparsed_numeric_fields 告知条数,防「字段格式异常→数据静默清空」;sort 对能解析成数值的字段**按数值比较**(根治字典序下 "999">"1299" 的反序,价格/销量/评分排序可直接用),字段未指定 field 时取第一条命名字段。返回 {items,total,returned,scanned,truncated,dropped_required,matched_before_filter,unparsed_time_fields,unparsed_numeric_fields,time_range,field_names,hostname,hint}。**时间字段支持中文相对时间**("3小时前"/"昨天")、ISO、"YYYY-MM-DD HH:mm" 与 Unix 秒;since/until 是闭区间。probe=true 时不抽数据,只返回 {repeated_classes,likely_item_classes,common_selectors} 供你选 item_selector,免去盲试选择器。过滤在页面内完成,返回值天然精简,不会撑爆上下文。
【page_state 详解】params:keys?(候选全局变量名,默认 __NEXT_DATA__/__NUXT__/__INITIAL_STATE__/__APOLLO_STATE__/__PRELOADED_STATE__/__remixContext/initialState/__INITIAL_DATA__)、probe_window?(默认 true,扫出 window 上所有 __ 前缀键名+一层结构,让你不必先猜名字)、max_bytes?(默认 20000)、max_depth?(默认 8)、max_array_items?(默认 200)。返回 {found,globals,window_globals,jsonld,dropped_paths,truncated,hostname,hint}。**被裁掉的内容会列进 dropped_paths** —— 别误以为数据就这么多;按 globals 的结构选定路径后,用 eval_js 取精确子集(如 JSON.stringify(window.__NEXT_DATA__.props.pageProps.list.slice(0,20)))。
- sequence(steps*, stop_on_error?): 连续执行模式。steps 最多 24 个,每项结构与单步调用相同(open/list/close/control/inspect),**action 可省略**(第 147 轮:含 control_action 默认 control、含 info 默认 inspect),禁止嵌套 sequence;**step 可省略 page_id**(第 157 轮:自动继承顶层 page_id,顶层也没有时继承上一步/派生页),批内显式 page_id 仍优先;字符串参数里仍可用 "$page_id"/"${page_id}" 占位,点击派生新页可用 "$spawned_page_id"/"${spawned_page_id}",默认自动跟随 spawned_page_id,单步可 follow_spawned=false 保持原页。响应逐步返回 code/message/data(成功步 message 截 300 字符、失败步截 2000),并给出最终 page_id。**第 150 轮步级防护**:任一步数据带 blocker_alert(验证弹窗命中)立即中断整批并顶层透出 blocker_alert/human_assist/next_action —— 收到后必须停止继续输入,立即 request_human 让人工完成验证、交棒后再续跑剩余步骤;单步超 150s 按步骤错误处理(request_human 步豁免);等待 AI 站点回复请用 control(wait),不要把多轮提问无超时地堆进同一批。
- explore(page_id?, queries*, summary_hint?)(第 118 轮):批量观察。一次调用合并多个 inspect 维度(elements/dom/screenshot/blockers 等),queries 数组最多 8 项,每项为单步 inspect 入参(如 {"info":"elements"} / {"info":"dom","params":{"selector":"form"}} / {"info":"screenshot"} / {"info":"blockers"});返回 {results:[{info,code,message,data},...], summary_hint, ok_count, err_count}。**进入新页面先用 1 次 explore 收集完整状态**(对比单步 inspect 节省 3-5 次 LLM round-trip);新页面也可直接传 url(省略 page_id,explore 自动打开后再观察,响应附 auto_opened);blockers 命中会附带 next_action hint 指引走 request_human。
- batch(page_id*, steps*, stop_on_error?)(第 118 轮):批量混合执行,与 sequence 同语义但推荐用于「inspect + control 混合的稳定流程」(登录/表单类:inspect 验证 → input × N → click → wait → inspect 验证)。一次调用最多 24 步,允许任意 control + inspect 顺序;step 可省略 page_id(第 157 轮自动继承)。**对比 sequence 推荐用于批量执行场景**。

【select_dropdown 操作范式(第 158 轮)】自定义下拉/级联选择器(Ant Design / Element UI / 自研组件库)不能用 select_option(后者只支持原生 `<select>`),改用 select_dropdown。参数:option_text(必填,选项文本如"北京")、trigger_text(可选,触发器文本如"区域",点击展开面板)、cascader_text(可选,级联第二级如"全北京")、wait_ms(可选,等待面板出现,默认 800ms)、selector(可选,面板容器选择器限定查找范围)。典型用法:闲鱼城市选择 → control(select_dropdown, params={"trigger_text":"区域","option_text":"北京","cascader_text":"全北京"})。响应回读 value_after 确认选中值。自愈与诊断(第 164 轮):**选项优先**——先探测选项,面板已被前序动作点开时直接选,不再点触发器(避免把已开面板 toggle 关回去);选项未命中才点触发器展开,随后选项搜索自动重试 3 次(含补等待);选中后自动探测面板内「确定/完成」类确认按钮并点击(响应 confirm_clicked),并回读触发器文本(trigger_after——found=false 表示触发器已回显选中值);**切勿对同一选项重复盲点**(toggle 控件会取消选中),筛选未生效时先看 trigger_after/value_after 事实再换策略(如切换排序或级联第二级);仍失败时错误携带诊断(文本命中数/可见数)——「命中但隐藏」多为面板收合的 visibility:hidden 挂载副本,先点击触发器展开再重试;可见性判定走计算样式,「文本+图标」非叶子触发器(如 <span>区域<img></span>)也能命中。select_option 在 selector 指向非原生 select 时也会自动回退到文本匹配路径(需传 text 参数),但推荐用 select_dropdown 语义更清晰。
【两种工作模式】1) 单步执行模式:直接调用 open/control/inspect/list/close,一次一个动作,适合探索、调试和高风险操作;2) 连续执行模式:先用单步 inspect(elements/dom/console/network)探索结构,再 action=sequence 一次执行已明确动作链,适合流程稳定任务(登录/表单类:inspect(form) → ocr 验证码 → sequence(input×N + click + wait + verify) 一次打包)。两种模式可混合、可多次调用。
【标准作业顺序】open 拿 page_id → inspect 探索真实 DOM → control 执行动作 → inspect 验证结果 → 任务完成后 close 释放(确定不再需要的页面;全部结束用 page_id="all" 清场,只留一个工作页用 page_id="others"+keep 精确收口)。
【多标签页与后台挂起】Chrome 一个窗口只有一个激活标签页,其余全部转后台挂起(渲染暂停/rAF 停摆/懒加载不触发/定时器节流),页面会因 open/new_tab、点击或 JS window.open 派生新页、人工手动切页而不断增多或换前台:① 前后台状态看 list 的 foreground 标记(人工可能随时手动切页,以最新探测为准);② 对后台页 screenshot / inspect(ocr) 返回 6003(后台截图=最后一次前台渲染的旧帧,证据必然失真)——重试加 params.focus=true 自动前台化后执行,或先 control(bring_to_front);其余动作(DOM 读写/CDP 输入/滚动)在后台页照常正确,响应附 page_background 提示;③ 标签页及时收口:单页任务完成即 close(page_id),后台页批量清理 close(page_id="background"),留一个工作页用 close(page_id="others", keep="保留的page_id"),全部结束 close(page_id="all"),页数多时 list 附 tabs_hint、同站堆积(list 附 origin_tabs_alert)先收口再继续;④ 切换前台会改变人工当前看到的页面,人工正在操作(open/partial 档)时避免来回切换;单页能完成的任务不要新开标签页。**任务边界兜底**:每个用户任务结束时编排器会自动关闭**本任务新开的后台标签页**(前台页保留;LAEW_WEB_TAB_AUTOCLOSE=off 关闭)——但执行中同站 Tab >3 就应立即主动收口,不要堆到任务结束;打开新页前先 list 看存量,同站已有存活页优先复用而不是再开新页。
【多窗口并行(多站点同时工作)】同一窗口的后台 Tab 会被挂起(见上条),「天猫+京东双站同时比价/监控」类任务需要**两个站点同时活跃**:用 `open(url, new_window=true)` 或 `control(new_window, params={url})` 给每个站点一个**独立可见窗口** —— 单浏览器进程多窗口,profile/登录态/HITL/下载全共享;各窗口互不遮挡时页面同时活跃、互不挂起(窗口打开后自动横向平铺防互相遮挡,平铺报告见 data.windows;前后台状态看 list 的 foreground/window_id)。单 Agent 可跨窗口交替操作两个 page_id;任务可分解时也推荐 SubAgent 分工 —— 每个子 Agent 负责一个窗口的页面,天然并行。限制:仅 headed 模式;connect(接管用户浏览器)不支持;窗口数上限 LAEW_WEB_MAX_WINDOWS(默认 4);窗口内最后一页 close 后 Chrome 自动收窗。
【焦点与操作节奏】CDP 操作(点击/输入/滚动/截图/读 DOM)全部不需要 Chrome 有键盘焦点,日常操作不要调用 bring_to_front;键盘焦点占用只发生在 open 启动浏览器、request_human 人工介入、6003 后台页取证这三处,且系统在动作收口后会把焦点自动归还给用户原先的应用(缺省行为,响应附 focus_restored 对账;用户要全程盯屏观看时可设 LAEW_WEB_FOCUS=always)。置前与人工介入是用户注意力被占用的时刻:动手前先用 explore/inspect 把现场与步骤规划好,再用 sequence/batch 一次打包做完,避免逐步置前、逐步试错来回打断人工;人工正在其它窗口打字时,更要避免触发置前类动作。
【错误码对策】1001 修正参数(含 guard 档位类:partial 缺 allow_selectors/block_selectors 之一、两者互斥、locked/open 不收选择器);2000 page_id 失效→action=list 重新同步;2001 断连→重新 open;2002 换 selector 重试;input 类失败先看响应里的 value_after/fallbacks 判断卡在哪一级(input_text 已自动尝试 JS 赋值/CDP 逐键/慢速逐字三级阶梯);press_sequence 带 selector 会先聚焦再按键;key_press 键名大小写不敏感(enter/esc/space/del/方向键等),多字符按键名不是合法按键;3001 未安装浏览器→如实告知用户,不要编造结果;**3002 复用浏览器不可用(调试端口未开启)→ 把 data.relaunch_command 转述用户执行,或以 auto_relaunch=true 重试(自动复制登录态并另启独立调试实例,不退出用户已打开的浏览器);不要反复重试 open**;**6001 目标站点越界(任务锚点)→ 立即停止该路径,如实报告「目标站点不可达或未指定」并结束本单元;严禁改用其它站点、搜索引擎、缓存或名称相似的替代品 —— 可以失败,不可以乱跑**;**6002 人工核验弹窗在场,输入动作已拦截 → 不要重试、不要换 eval_js 绕过(可变更 eval 同样被拦);桌面环境下工具会**自动弹出人工介入弹窗**等人工处理(2026-10-13,应答结果在 data.auto_hitl,人工点「继续」后直接重试本动作即可;LAEW_HITL_AUTO=off 关闭回退提示模式,此时按 data.human_assist 载荷 request_human(reason=captcha) 让人工完成);确认是结构误判才可 params.force=true 强制执行**;**6003 页面后台挂起 → 截图/OCR 在后台标签页必然失真,严禁拿旧截图当证据;按 data.required_action 三选一(params.focus=true 重试自动前台化 / 先 control(bring_to_front) / 该页不再需要则 close 收口),确认后台截图可用才 params.force=true**。
【页面管控三档(第 143 轮,仅 headed 可视化模式)】guard 决定人工对页面的操作权限,按用户意图选档:**用户没说要自己动手/缺省 → locked**(防交叉操作是安全基线,含支付/删除等高风险动作的任务必须 locked);**用户说「我自己操作/我来点/我先登录/你看着」→ open(guard_note 写清引导)**;**用户要分区协作(「我操作登录框,你管其余/锁住支付按钮」)→ partial + allow_selectors(白名单)或 block_selectors(黑名单)**;信号不明确时 locked 起步、需要人工参与时经 request_human 询问。open/partial 下人工与 Agent 并发操作页面,**关键写操作前先 inspect 验证现场**再动手。人工登录先行流:open(guard="open", guard_note="请登录后应答弹窗…") → control(request_human, params={reason:"login", message:…}) → 人工在页面完成登录并应答 → set_guard(mode="locked") 回到防交叉基线 → 继续任务。运行时随时 set_guard 切换;蒙层/盾区/屏蔽页面状态栏与进度栏是设计行为不是页面故障,不要试图用 eval_js 移除。
【人工介入(HITL)】遇到滑块/图形验证码(OCR 不可读)/短信验证码/扫码登录/人脸核身等无法自动完成的流程:可视化场景确认 data.mode=headed(缺省即是,无需显式传 mode=headed),让人工看到窗口(蓝色边框标识,页面默认有半透明蒙层锁定人工输入——这是设计行为不是页面故障),再 control(request_human, params={reason:..., message:说明要人工做什么, options:[...]})(reason/message/options 三键也接受顶层便捷写法,等价放入 params);**提问期间安全全面解除屏蔽(unlock_page 默认 true;第 167 轮:缺省整页 open——屏蔽页面状态栏显示「🔓 页面开放」+ guard_note「人工介入中·页面已全面解锁,可自由操作;完成后回弹窗点『提交 / 继续』」,人工自由拖滑块/扫码/登录/填表,点『提交 / 继续』交还控制权;显式传 allow_selectors 才走 partial 挖洞,自动并入挑战层选择器+覆盖验证,洞没罩住挑战回退整页 open;应答/超时/取消后自动恢复该页解锁前管控档;inspect(info=blockers) 命中时 data.credential_zones 给出可传 allow_selectors 的区域选择器)**;macOS/Windows 桌面自动弹出人工介入弹窗(置顶+倒计时,人工在弹窗点选项/输入验证码;第 145 轮:主按钮「提交 / 继续」双语义,人工在浏览器完成操作后空输入点击即交棒 Agent 接管;「⏱ +2分钟」可延长等待,总上限 30 分钟,实测人工整链登录超 120s 分档超时的根治;弹窗文本可鼠标选中复制,输入框支持 ⌘C/⌘V/⌘A,也有「📋 复制」一键复制全部信息),Linux 或弹窗不可用时 TUI 选择块行读;code=0 用 data.human_response 继续——**先看 next_hint 分流**(第 145 轮:human_response 是自由文本验证码/动态码 → 立即 input_text 填入并提交表单;是「N. 选项」形态(如「我已完成」)→ inspect 验证页面状态后继续),data.page_unlock 回报本次放行方式对账;4001=超时/弹窗与 TUI 均不可用,如实报告;4002=人工取消(弹窗取消/Esc/关窗),终止该路径。当前实例是无头而任务需要可视化时,先 close("all") 回收再以 mode=headed 重开。
【自动阻断感知(第 147 轮;第 151 轮升级)】输入类动作(click/input_text/key_press/drag 等 20 个)、wait(含等待超时失败)、导航类动作与 **eval_js**(含 await_promise 长轮询——「发出消息→风控弹窗」正是发生在这个窗口)完成后,工具**自动探测**人工验证挑战 —— 图片选择题(「选出在公园能看到的事物」/「选出卧室家具」类)/滑块/拖拽/语义题等**题面没有「验证码」关键词**的形态,靠弹层结构(高 z-index 模态层 + 九宫格小图或 CSS 背景图网格 / 验证码尺寸 iframe / canvas / 验证文案)识别,不依赖你主动调 inspect(blockers)。命中时(2026-10-13 起,桌面环境且非批量步骤中)**自动弹出人工介入弹窗**等人工处理(data.auto_hitl 回报应答结果,human_response 为人工答复),人工完成后按其应答继续;不能自动弹出的场景(批量步骤中/已有人在应答/开关关闭)响应 data 携带 `blocker_alert{kind,matched,snippet,evidence,hint}` + `next_action="request_human"` + `human_assist` 载荷(题面 snippet 直接进 message,人工在弹窗即知要验证什么):**立即停止继续输入/发消息/重试**(实测事故:验证弹窗在场,Agent 无感知连发 16 轮消息直到用户手动终止),按载荷 request_human(reason=captcha) 让人工完成;不确定弹窗内容可先 control(screenshot, params.ocr=true) 确认。页面干净时响应零附加;wait 超时 + blocker_alert 同时出现 = 「回复没来 + 弹窗在场」的最强信号,优先按 blocker_alert 处置而非加大等待重试。**第 151 轮输入硬闸**:输入类动作与可变更 eval_js(execCommand/dispatchEvent/合成事件等)**执行前**还会做一次结构判定探测,命中真实验证弹窗时直接返回 6002 拦截本动作(见错误码对策)—— 人工完成验证、应答弹窗后页面干净即自动恢复,不需要任何解锁操作。
【作业要点】文本输入一律用 input_text(selector,text) —— 默认 auto 自动阶梯:JS 原生赋值(input/textarea/select/contenteditable 通用,受控组件兼容)→ 页内回读校验 → 未生效自动降级 CDP 逐键输入 → 再未生效降级慢速逐字,响应带 mode/applied/value_after(回读摘要)如实汇报生效路径,**不再需要传 use_js**;批量对话投放按 sequence 打包:input_text → key_press(enter) → wait(selector=回复容器,timeout_ms=60000) → eval_js 提取回复。key_press(key) 键名大小写不敏感且支持别名(enter/esc/tab/space/del/ins/pgup/方向键)与组合写法(ctrl+a、cmd+v);press_sequence(keys,selector?) 带 selector 会先点击聚焦再逐键,防止按键打进失焦页面;复杂页面先 inspect(info=elements) 探测真实 DOM 再操作,不要硬猜 selector;**要滚到页面上某个语义区块(评论区/商品参数/规格表/规格选择)用 control(scroll_to_text, params={text:"累计评价", block?:"center"})** —— 按可见文本自动定位并居中,免去先 inspect 拿选择器再 scroll_to 的往返,返回 {found,tag,text_len,head};找不到时说明该区块尚未渲染(先 wait/scroll 触发懒加载)、在**跨域** iframe 内(拿不到执行权,响应会报「已探测主 frame + N 个嵌套 frame」),或文案不同;响应 frame 字段回报命中位置(main / frame:N);**翻页/懒加载后要重复滚动**,只在顶部 DOM 里取到的元素往往是占位骨架;AI 对话类网站回复等待用 control(wait, selector=[class*=response]..., timeout_ms=60000);看图片里的文字(验证码/图表标签)一律 screenshot(params.ocr=true) 或 inspect(info=ocr),禁止 Read 图片文件、禁止用 Bash/python/tesseract 解码图片(文本模型无视觉,纯浪费迭代);验证码读码后不要刷新页面或点击验证码图(刷新即换码),提交报验证码错误才点图刷新重读;DOM 提取注意 truncated 标记分段。
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
                "force": { "type": "boolean", "default": false, "description": "open/navigate 可选(第 171 轮):true 跳过站点导航节奏闸(频控冷却 6004 / 最小导航间隔),仅在确知冷却误标或必须立即导航时使用" },
                "new_window": { "type": "boolean", "default": false, "description": "open 可选(多窗口并行,双站同时工作):true 时在**独立新窗口**(而非同窗口新 Tab)打开页面 —— 各 Agent 窗口互不遮挡时页面同时活跃、互不挂起,单 Agent 可跨窗口交替操作,也适合 SubAgent 分工各管一窗(如天猫一窗/京东一窗并行比价)。打开后自动横向平铺全部 Agent 窗口(防互相遮挡被 occlusion 节流,报告见 data.windows)。限制:仅 headed 模式;与 reuse_existing 互斥;窗口数上限 LAEW_WEB_MAX_WINDOWS(默认 4,按去重窗口计);当前后台/同窗口新页请继续用 new_tab=false 默认路径" },
                "reuse": { "type": "boolean", "default": true, "description": "open 可选:同 URL 已有存活页面时复用(导航刷新,page_id 不变,响应 reused:true);精确未命中时同 origin 存活页面兜底复用(reuse_match=origin,第 146 轮);false 强制新开页面(new_window=true 时不参与复用)" },
                "mode": { "type": "string", "enum": ["headed", "hidden", "new_headless"], "default": "headed", "description": "open 可选:浏览器模式;headed=可见窗口(默认,第 139 轮),hidden=纯 CDP 无窗口,new_headless=旧 headless=true。缺省由 LAEW_BROWSER_MODE 与 GUI 会话探测决定:无 GUI 会话(CI/容器/SSH)自动回退 hidden" },
                "window_width": { "type": "integer", "minimum": 320, "maximum": 7680, "description": "open 可选:浏览器窗口宽(px);**缺省不传** —— headed 自动铺满屏幕工作区;仅当任务明确要求特定窗口尺寸时才传,与 window_height 必须成对(显式指定会关闭铺满、只做不溢出收边)" },
                "window_height": { "type": "integer", "minimum": 240, "maximum": 4320, "description": "open 可选:浏览器窗口高(px);**缺省不传** —— headed 自动铺满屏幕工作区;仅当任务明确要求特定窗口尺寸时才传,与 window_width 必须成对(显式指定会关闭铺满、只做不溢出收边);运行时调整用 control(set_window),人工拖动后用 control(sync_viewport) 自适应" },
                "auto_expand_viewport": { "type": "boolean", "default": true, "description": "open 可选(默认 true):导航完成后若页面内容超出视口(横向被裁/可视高度不足),自动用 device metrics 把视口扩展到 ≤2560×1440(2K),扩展结果回 data.viewport(expanded/from/to/content/clamped/hint)——根治「视口太窄页面显示不全、元素不可见不可点」;false 关闭;内容仍超 2K 上限时截图用 params.full_page=true 或 set_viewport 显式超限" },
                "highlight": { "type": "boolean", "default": true, "description": "open 可选:headed 模式在页面四周注入一圈蓝色选中边框,右上角显示「屏蔽页面状态栏」(LAEW Agent 控制中 · 具体 Agent 名 / 三档档位 / 注意事项 / guard_note)、左下角显示「屏蔽页面进度栏」(当前动作 / 迭代进度与耗时 / 任务摘要 / 最近事件时间线)——给人工看的实时现场;pointer-events:none 不影响页面交互;可用 control(set_highlight, enabled=false) 一并运行时关闭两条栏" },
                "guard": { "type": "string", "enum": ["locked", "open", "partial"], "default": "locked", "description": "open 可选(第 143 轮):页面管控三档。locked=屏蔽模式(缺省,半透明蒙层+锁定人工输入,人工可实时观看不可点击,防交叉操作;Agent 输入动作自动先解后锁)/ open=非屏蔽模式(人工可直接操作页面,状态条明示「页面开放」,用于人工亲自操作或人工登录先行流)/ partial=部分屏蔽模式(盾区内人工不可操作、盾区外放行,须同时给 allow_selectors 或 block_selectors 之一)。缺省随 LAEW_WEB_GUARD;仅 headed 生效;运行时切换 control(set_guard)" },
                "stealth": { "type": "string", "enum": ["fidelity", "hardened", "off"], "default": "fidelity", "description": "open 可选(第 174 轮):反检测档位。fidelity=非侵入式(缺省,只移除自动化泄漏+回填本机真值:webdriver/WebGL 兜底串/空 plugins·languages·chrome.runtime/权限一致性/无头 UA 令牌)/ hardened=侵入式(保真之上再让「我们改过」不可反查:被替换函数 Function.prototype.toString 原生化)/ off=不处置(排障对照)。**缺省不用传**;仅在站点风控明确认得自动化特征、或需要排障对照时使用。缺省随 LAEW_WEB_STEALTH;运行时切换 control(set_stealth);接管用户浏览器(connect)零注入不受影响" },
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
                "page_id": { "type": "string", "description": "close/control/inspect 必填:open/list 返回或 spawned_page_id 回传的页面句柄;close 时可为 \"all\" 一键关闭全部页面并回收浏览器,或 \"background\" 批量关闭全部后台标签页(至少保留一个前台页,探测不到前台页时拒绝执行),或 \"others\" 关闭除 keep 外全部页面(keep 必填且必须是存活 page_id,防一锅端)" },
                "keep": { "type": "string", "description": "close 可选(仅 page_id=\"others\" 时必填):要保留的存活 page_id,其余全部关闭" },
                "control_action": {
                    "type": "string",
                    "enum": [
                        "click", "human_click", "right_click", "double_click", "hover",
                        "scroll", "scroll_to", "scroll_to_text", "key_press", "press_sequence",
                        "input_text", "human_input", "clear_input",
                        "upload_file", "select_option", "select_dropdown", "download",
                        "new_tab", "new_window", "close_tab", "bring_to_front",
                        "navigate", "back", "forward", "reload",
                        "wait", "eval_js",
                        "set_cookie", "delete_cookie",
                        "set_storage", "clear_storage", "set_viewport",
                        "screenshot", "heartbeat",
                        "drag", "focus", "blur", "mouse_move", "dispatch_event",
                        "set_window", "sync_viewport", "set_highlight", "set_overlay", "set_guard", "auto_follow", "request_human"
                    ],
                    "description": "control 必填:具体写操作(鼠标/键盘/输入/上传/下载/标签页/导航/等待/JS/Cookie/Storage/视口/截图等 45 个;set_window/sync_viewport=窗口自适应,set_overlay=蒙层开关(legacy),set_guard=页面管控三档切换(第 143 轮),auto_follow=自动跟随最新内容(第 152 轮,多轮对话站让人工看得到最新消息),request_human=人工介入,select_dropdown=自定义下拉/级联选择器(第 158 轮),bring_to_front=把指定标签页带到前台(标签页级激活,与元素级 focus 区分;后台页做 screenshot/ocr 返回 6003 时的标准对策),new_window=独立新窗口打开页面(多站点同时工作,见工具描述【多窗口并行】))"
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
                "params": { "type": "object", "description": "control/inspect 可选:动作参数对象(selector/text/key/keys/timeout_ms/url/x/y/file_paths/save_dir/filename 等按 control_action/info 各异)。wait 纯等待支持 duration_ms 或别名 ms(单次 ≤30000,默认 500;等元素用 selector+timeout_ms ≤120000)。screenshot/ocr 支持 save_path(落盘路径)、ocr=true(返回 OCR 文字)、region{x,y,width,height}(OCR 词块过滤)、return_base64=true(显式内联 base64,默认不返回)、full_page/format/quality。后台标签页做 screenshot/ocr 返回 6003:重试时加 focus=true 自动把该标签页带到前台后执行(或先 control(bring_to_front);确认后台截图可用可 force=true 强制)" },
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
                "summary_hint": { "type": "string", "description": "explore 可选:附加提示,会出现在响应 data.summary_hint 字段,便于 LLM 在终答中引用" },
                // 2026-10-16:summary 是 summary_hint 的别名(explore 可选;模型自然写法
                // 漂移,additionalProperties=false 曾硬拒白烧一轮迭代,与下方 request_human
                // 顶层别名同策略)。
                "summary": { "type": "string", "description": "explore 可选:等价 summary_hint(别名兼容)" },
                // 2026-10-14 第三轮:control(request_human) 顶层便捷别名 —— 系统
                // 提示词的自然写法诱导模型把这三个键放顶层,additionalProperties=false
                // 曾三连拒(实测烧 3 轮迭代);放行后由 control 入口归一化提升进 params。
                "message": { "type": "string", "maxLength": 600, "description": "control(request_human) 便捷别名:等价 params.message(要人工做什么的说明);其余动作的参数请放 params 对象" },
                "reason": { "type": "string", "description": "control(request_human) 便捷别名:等价 params.reason(captcha|sms|qr_login|login|real_name|two_factor|oauth|manual_verify|custom)" },
                "options": { "type": "array", "items": { "type": "string" }, "maxItems": 8, "description": "control(request_human) 便捷别名:等价 params.options(供人工选择的选项列表)" }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let Some(action) = str_arg(&args, "action").map(str::to_string) else {
            return envelope(1001, "缺少 action", json!({}));
        };
        // 第 171 轮:屏蔽页面进度栏动态刷新 —— 动作前推「正在执行 …」,动作后推
        // 事件条目(时间线滚动)。只在有存活浏览器页面时产生 CDP 往返(毫秒级),
        // 无浏览器实例时 `broadcast` 立即返回,零开销。fail-open:推送失败不影响动作。
        let label = crate::agent::browser_status::describe_web_action(&args);
        let started = std::time::Instant::now();
        crate::agent::browser_status::broadcast_start(&label).await;
        let out = dispatch_web_action(&action, args).await;
        crate::agent::browser_status::broadcast_done(
            &label,
            out.as_ref().map(|s| s.contains("\"code\":0")).unwrap_or(false),
            started.elapsed().as_millis() as u64,
        )
        .await;
        out
    }
}

/// 八种 action 的分发(第 171 轮自 `execute` 抽出,便于在分发前后统一挂
/// 屏蔽页面进度栏推送)。
async fn dispatch_web_action(action: &str, args: Value) -> Result<String> {
    match action {
        "open" => run_open(args).await,
        "list" => run_list(args).await,
        "close" => run_close(args).await,
        "control" => control::run(args).await,
        "inspect" => {
            // 2026-10-16:参数形态兼容 —— 模型常把 explore 的 queries[] 批量观察形态
            // 误传给 inspect(实测返回 1001「缺少 info」白烧一轮 LLM 迭代)。info 缺失
            // 且 queries 为非空数组时机械转交 explore,语义完全等价(explore 即批量
            // inspect)。分流放这里而非 inspect::run 入口,避免 explore→inspect
            // async 递归(E0733 requires boxing)。
            if str_arg(&args, "info").is_none()
                && args
                    .get("queries")
                    .and_then(Value::as_array)
                    .map(|a| !a.is_empty())
                    .unwrap_or(false)
            {
                control::run_explore(args).await
            } else {
                inspect::run(args).await
            }
        }
        "sequence" => run_sequence(args).await,
        "explore" => control::run_explore(args).await,
        "batch" => control::run_batch(args).await,
        other => envelope(1001, "未知 action", json!({"action": other})),
    }
}
