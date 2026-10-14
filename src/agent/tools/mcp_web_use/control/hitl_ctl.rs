//! MCP_Web_Use control_action=request_human:人工介入(HITL)闭环。
//!
//! 2026-10-13 自 `control/mod.rs` 迁入(control.rs 超 1800 行拆分规范),并完成
//! 「通用化 + 自动弹出」改造:
//! - [`HitlSpec`]/[`request_human_core`]:LLM 显式调用与工具层**自动升级**
//!   (auto-HITL)共用同一执行核 —— 解锁判定/页面实际态快照、挑战层滚入视口、
//!   「解除屏蔽/恢复屏蔽」互切 handler 安装、焦点租约、收口恢复全套;
//! - `multi_select`:选项多选(弹窗渲染为复选框组,「继续」收集勾选);
//! - [`auto_hitl`]:输入硬闸(6002)与收尾探针命中时**自动弹出**人工介入弹窗,
//!   守卫条件见其文档(`LAEW_HITL_AUTO` / 非 sequence 上下文 / hub 无 pending)。
//!
//! 设计见 `docs/MCP_Web_Use/03-人工介入弹窗原生UI方案.md` §4.7、§5。

use serde_json::{json, Value};

use super::super::{envelope, str_arg};

/// request_human 的 reason → 默认文案与展示标签。
/// 第 130 轮收口到 `human_assist::{kind_label, default_message}` 单一事实源
/// (弹窗 / TUI / 工具侧三处共用);完整合法 reason 列表见
/// [`HUMAN_ASSIST_ALLOWED_REASONS`],新增 reason 同步更新那边 + 校验顺序 + 文档。
fn human_assist_defaults(reason: &str) -> (&'static str, &'static str) {
    (
        crate::agent::human_assist::kind_label(reason),
        crate::agent::human_assist::default_message(reason),
    )
}

/// request_human 合法 reason 集合(用于参数校验与对外暴露给 LLM)。
/// 顺序即为推荐使用顺序;新增 reason 必须同步更新此处 + TUI 标签 + 文档。
pub const HUMAN_ASSIST_ALLOWED_REASONS: &[&str] = &[
    "captcha",
    "sms",
    "qr_login",
    "login",
    "real_name",
    "two_factor",
    "oauth",
    "manual_verify",
    "custom",
];

/// 2026-10-13:人工介入附图决策(原 `captcha_crop` 子模块的 `HitlImagePlan`/
/// `hitl_image_plan`,随弹窗验证码功能移除一并收口到此处)。
///
/// 弹窗已不渲染图片,自动截图采集链整链删除;仅保留**显式** `params.image_path`
/// 透传(TUI 兜底打印路径供 `open` 查看)。返回 `(计划, 来源标签)` 供信封对账。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitlImagePlan {
    /// 使用显式路径(已确认文件存在)。
    Explicit(String),
    /// 不附图。
    None,
}

pub fn hitl_image_plan(reason: &str, explicit: Option<&str>) -> (HitlImagePlan, &'static str) {
    match explicit {
        Some(path) if !path.trim().is_empty() => {
            if std::path::Path::new(path.trim()).is_file() {
                (HitlImagePlan::Explicit(path.trim().to_string()), "explicit")
            } else {
                // 显式指定但文件不存在:忽略并标注,fail-open 不阻断提问
                (HitlImagePlan::None, "explicit_missing")
            }
        }
        _ => {
            let _ = reason; // 历史形参保留(调用点契约零改动);reason 不再影响附图
            (HitlImagePlan::None, "none")
        }
    }
}

/// 把 reason 列表渲染成斜杠分隔字符串(用于错误消息/工具 Schema 描述)。
pub fn human_assist_reasons_doc() -> String {
    HUMAN_ASSIST_ALLOWED_REASONS.join("/")
}

/// 应用 HITL 提问期间的临时放行态(第 144 轮,纯应用不落期望态)。
///
/// - `allow` 非空:**partial + 白名单挖洞**(账号/密码/验证码等人工必填区域成为
///   非屏蔽区域,其余保持 Agent 管控,最小权限;状态条第三行引导人工);
/// - `allow` 为空:整页 open(第 143 轮行为,探测不到凭证区的回退档)。
///
/// 第 145/167 轮:两档的引导 note 都指明收口动作「回到弹窗点『继续』」——
/// 管控三档(locked/open/partial)只在 Agent 全面操作时生效;HITL 期间是临时放行态,
/// 人工完成操作后的交棒动作必须显性可见(实测反馈:整页 open 回退档只有通用
/// 「页面开放」状态条,人工不知道做完要去弹窗点继续)。第 167 轮起缺省档即
/// 整页 open(安全全面解锁),note 明示「可自由操作」。
///
/// 成功返回导航重放协程句柄(提问期间人工提交登录触发导航时重放临时态);
/// 应用失败返回 None(等同未解锁,提问照常进行,fail-open)。
async fn apply_hitl_unlock(
    page: &chromiumoxide::Page,
    allow: &[String],
) -> Option<tokio::task::JoinHandle<()>> {
    use crate::agent::browser_overlay::{apply_page_guard, PageGuardConfig};
    let cfg = if allow.is_empty() {
        let mut c = PageGuardConfig::open();
        c.note =
            Some("人工介入中·页面已全面解锁,可自由操作;完成后回弹窗点『提交 / 继续』".to_string());
        c
    } else {
        let mut c = PageGuardConfig::partial(allow.to_vec(), vec![]);
        c.note = Some("人工输入区已开放(部分锁定),完成后回弹窗点『提交 / 继续』".to_string());
        c
    };
    apply_page_guard(page, &cfg).await.ok()?;
    // 2026-10-14(天猫 baxia 实测):人工核验期间挂起两条栏 —— 状态栏/进度栏以
    // z-index 2147483646 叠在挑战蒙层上,把滑块/验证码控件与提示罩死,人工无法
    // 操作(弹窗「页面已全面解锁」状态行已承担告知职责)。收口恢复管控时一并
    // 恢复(fail-open:挂起失败不阻断提问)。
    crate::agent::browser_status::set_suspended(page, true).await;
    Some(crate::agent::tools::mcp_web_use::unlock_zone::spawn_unlock_reassert(
        page.clone(), cfg,
    ))
}

/// HITL 解锁判定 + 页面实际态快照(2026-10-13 抽出,供解锁决策与弹窗「解除屏蔽/
/// 恢复屏蔽」切换按钮初始标题共用,避免对同一页重复 eval)。
///
/// 返回 (是否需要解锁, 页面实际态快照):locked/partial → 需要;open/未装 JS →
/// 不需要。JS 态缺失(guard 脚本注入完全失败)时 CDP 输入锁 fail-closed 兜底
/// 可能仍在场 —— 退回全局期望态判断(此情形下页面没有独立态可读)。
async fn hitl_unlock_need_and_snapshot(
    id: &str,
) -> (
    bool,
    Option<crate::agent::browser_overlay::PageGuardConfig>,
) {
    use crate::agent::browser_overlay::PageGuardMode;
    match crate::agent::browser::BrowserManager::global().page(id).await {
        Some(page) => {
            let snap =
                crate::agent::tools::mcp_web_use::unlock_zone::page_guard_snapshot(&page).await;
            let needs = match snap.as_ref() {
                Some(c) => {
                    matches!(c.mode, PageGuardMode::Locked | PageGuardMode::Partial)
                }
                None => {
                    crate::agent::browser::BrowserManager::global()
                        .current_guard()
                        .await
                        .mode
                        != PageGuardMode::Open
                }
            };
            (needs, snap)
        }
        None => (false, None),
    }
}

// =================== 2026-10-13:HitlSpec / 执行核 / 自动升级 ===================

/// 一次人工介入请求的完整规格 —— LLM 显式调用(参数解析后)与工具层自动升级
/// (auto-HITL)共用,保证两条路径拿到同一套解锁/快照/收口待遇。
#[derive(Debug, Clone)]
pub(super) struct HitlSpec {
    /// 阻断类型(合法集合见 [`HUMAN_ASSIST_ALLOWED_REASONS`])。
    pub reason: String,
    /// 给人看的具体说明。
    pub message: String,
    /// 编号选项(空 → 核内回退缺省对;>6 截断)。
    pub options: Vec<String>,
    /// 选项是否多选(弹窗复选框组)。
    pub multi_select: bool,
    /// 等待上限毫秒(None → 按 reason 分档:captcha/sms/two_factor=120s 其余 300s)。
    pub timeout_ms: Option<u64>,
    /// 是否先把页面带到前台。
    pub bring_to_front: bool,
    /// 提问期间是否安全全面解锁页面(缺省 true;纯问答场景 false)。
    pub unlock_page: bool,
    /// 显式 partial 白名单(空 → 缺省整页 open)。
    pub allow_selectors: Vec<String>,
    /// 显式附图路径(空 → 无图;弹窗不渲染,仅供 TUI 兜底打印)。
    pub image_path: Option<String>,
    /// 2026-10-13(二):侵入式中继映射区(视口坐标 {x,y,width,height};None =
    /// 会话开始时自动探测挑战层/回退全视口)。弹窗「侵入式」触控板落点映射依据。
    pub relay_rect: Option<Value>,
    /// 发起来源(LLM 显式 / 工具层自动)。
    pub origin: HitlOrigin,
}

/// HITL 发起来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HitlOrigin {
    /// LLM 显式 `control(request_human)`。
    Llm,
    /// 工具层自动升级(输入硬闸 6002 `input_gate` / 收尾探针 `blocker_probe`)。
    Auto { trigger: &'static str },
}

/// 执行核产出:信封三件套 + 结局枚举(auto 路径据此标注 `data.auto_hitl.status`)。
#[derive(Debug, Clone)]
pub(in crate::agent::tools::mcp_web_use) struct HitlReport {
    pub code: i32,
    pub message: String,
    pub data: Value,
    pub status: HitlStatus,
}

/// 人工介入四态结局(对齐 `HumanAssistOutcome`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::agent::tools::mcp_web_use) enum HitlStatus {
    Answered,
    Cancelled,
    Timeout,
    Unavailable,
}

impl HitlStatus {
    fn as_str(self) -> &'static str {
        match self {
            HitlStatus::Answered => "answered",
            HitlStatus::Cancelled => "cancelled",
            HitlStatus::Timeout => "timeout",
            HitlStatus::Unavailable => "unavailable",
        }
    }
}

/// 解析 `relay_rect` 参数(视口坐标 `{x,y,width,height}`;宽高非正 → None)。
/// 纯函数,可单测。
pub(super) fn parse_relay_rect(v: &Option<Value>) -> Option<(f64, f64, f64, f64)> {
    let obj = v.as_ref()?;
    let f = |k: &str| obj.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let (x, y, w, h) = (f("x"), f("y"), f("width"), f("height"));
    if w > 0.0 && h > 0.0 {
        Some((x, y, w, h))
    } else {
        None
    }
}

/// request_human 执行核(迁移自 act_request_human 主体,2026-10-13):
/// 解锁判定/快照 → 挑战层滚入视口 → 焦点租约与置前 → 安装「解除屏蔽/恢复屏蔽」
/// 互切 handler → hub 发起(弹窗优先/TUI 兜底)→ 收口恢复管控与焦点 → 信封。
async fn request_human_core(id: &str, spec: &HitlSpec) -> HitlReport {
    let reason = spec.reason.as_str();
    let (label, default_message) = human_assist_defaults(reason);
    let message = if spec.message.trim().is_empty() {
        default_message.to_string()
    } else {
        spec.message.clone()
    };
    let mut options = spec.options.clone();
    if options.is_empty() {
        options = vec![
            "我已完成人工操作,继续".to_string(),
            "取消任务".to_string(),
        ];
    }
    options.truncate(6);
    let timeout_ms = spec.timeout_ms.unwrap_or_else(|| {
        // 第 118 轮:按 reason 分档默认超时(LLM 显式传 timeout_ms 仍走传入值)
        // - captcha / sms / two_factor:120_000(2 分钟)
        //   短文本回 TUI,人工可直接输入,无需更长等待;对齐 claudecode 实践
        // - qr_login / real_name / oauth / login / manual_verify / custom:300_000(5 分钟)
        //   扫码/刷脸/账密登录等需要人工在浏览器窗口操作的场景,留足余量
        match reason {
            "captcha" | "sms" | "two_factor" => 120_000,
            _ => 300_000,
        }
    });

    // 第 167 轮:HITL 解锁语义重构 —— 缺省**安全全面解锁**(整页 open)。
    // 背景(实测 2026-10-13 双站比价,tmpPlan/20261013_134500):
    // - 第 144 轮自动探测 partial 的白名单是 `:nth-of-type` 位置型选择器,
    //   React SPA 上漂移后洞消失,天猫滑块被盾罩盖死(人工被迫在弹窗打字反馈);
    // - 解锁判定读全局 `current_guard()`,多窗口并行下被另一页的 set_guard(open)
    //   改写,京东页 locked 态从未解除,人工登录页点不了。
    // 新语义:
    // - 判定改读**页面实际态**(unlock_zone::page_guard_snapshot),每页独立;
    // - 未显式传 allow_selectors → 整页 open:人工介入窗口 = 全面解除屏蔽,
    //   人工自由操作,完成后回弹窗点「提交 / 继续」交还控制权;
    // - 显式 allow_selectors → partial 保留 + 两道安全网:挑战层选择器并洞
    //   (blocker_probe 的类名/id 选择器,稳定)+ 应用前覆盖验证,洞没罩住
    //   挑战即回退整页 open(fail-open);
    // - 收口恢复按**本页解锁前实际态快照**重放(不再拿全局配置灌页面)。
    // 提问期间人工提交表单触发导航(登录→2FA)由导航重放协程维持放行态。
    // 纯问答场景(确认继续?)可传 unlock_page=false。
    // 第 167 轮:挑战层探测提前到解锁决策之前 —— 返回的类名/id 选择器供显式
    // partial 并洞(安全网),顺带把弹层滚进视口(人工可读,原顺序在解锁之后)。
    let challenge_selector = match crate::agent::browser::BrowserManager::global().page(id).await {
        Some(pg) => crate::agent::tools::mcp_web_use::blocker_probe::scroll_challenge_into_view(
            &pg,
        )
        .await,
        None => None,
    };
    let explicit_allow = crate::agent::tools::mcp_web_use::unlock_zone::sanitize_selector_list(
        &spec.allow_selectors,
    );
    let mut unlock_source: &str = "none";
    let mut unlock_allow: Vec<String> = Vec::new();
    let mut unlock_verify: &str = "none";
    let mut unlock_reassert: Option<tokio::task::JoinHandle<()>> = None;
    let mut unlock_snapshot: Option<crate::agent::browser_overlay::PageGuardConfig> = None;
    // 2026-10-13:解锁判定 + 页面实际态快照一次评估,供解锁决策与弹窗「解除屏蔽/
    // 恢复屏蔽」切换按钮初始标题共用(避免对同一页重复 eval)。
    let unlock_need = if spec.unlock_page {
        hitl_unlock_need_and_snapshot(id).await
    } else {
        (false, None)
    };
    let unlock_applied = spec.unlock_page
        && crate::agent::browser::BrowserManager::global().guard_visuals_active().await
        && match crate::agent::browser::BrowserManager::global().page(id).await {
            Some(page) => {
                let (needs_unlock, snap) = unlock_need.clone();
                // 页面实际态优先:locked/partial 才放行,已是 open/未装 JS → 无需。
                // JS 态缺失(guard 脚本注入完全失败)时 CDP 输入锁 fail-closed 兜底
                // 可能仍在场 —— 退回全局期望态判断(此情形下页面没有独立态可读)。
                if !needs_unlock {
                    false
                } else {
                    unlock_snapshot = snap;
                    // 放行配置选择:显式 partial(安全网)或缺省安全全面解锁。
                    let mut allow: Vec<String> = Vec::new();
                    if !explicit_allow.is_empty() {
                        let merged =
                            crate::agent::tools::mcp_web_use::unlock_zone::merge_challenge_selector(
                                &explicit_allow,
                                challenge_selector.as_deref(),
                            );
                        let coverage = match challenge_selector.as_deref() {
                            Some(ch) => {
                                crate::agent::tools::mcp_web_use::unlock_zone::challenge_covered_by_allow(&page, &merged, ch).await
                            }
                            // 无挑战层在场:无从验证,尊重显式选择维持 partial。
                            None => crate::agent::tools::mcp_web_use::unlock_zone::ChallengeCoverage::NoChallenge,
                        };
                        unlock_verify = match coverage {
                            crate::agent::tools::mcp_web_use::unlock_zone::ChallengeCoverage::Covered => "covered",
                            crate::agent::tools::mcp_web_use::unlock_zone::ChallengeCoverage::NoChallenge => "no-challenge",
                            crate::agent::tools::mcp_web_use::unlock_zone::ChallengeCoverage::Uncovered => "uncovered",
                        };
                        if matches!(
                            coverage,
                            crate::agent::tools::mcp_web_use::unlock_zone::ChallengeCoverage::Uncovered
                        ) {
                            // 安全网:洞没罩住挑战层 → 回退整页 open(fail-open)。
                            unlock_source = "explicit_open_uncovered";
                        } else {
                            allow = merged;
                            unlock_source = "explicit";
                        }
                    } else {
                        unlock_source = "safe_open";
                    }
                    unlock_allow = allow.clone();
                    match apply_hitl_unlock(&page, &allow).await {
                        Some(handle) => {
                            unlock_reassert = Some(handle);
                            true
                        }
                        None => {
                            unlock_source = "none";
                            unlock_allow.clear();
                            false
                        }
                    }
                }
            }
            None => false,
        };

    // 先把窗口带到前台(headed 才有视觉效果;hidden 模式静默失败不阻断提问)
    // 第 166 轮:焦点租约 —— 置前/弹窗都会抢 OS 键盘焦点;提问期间焦点归人工,
    // 收口(应答/超时/取消)后 keep 档(缺省)把键盘焦点归还给提问前的前台应用,
    // 用户正在其它窗口打字不被打断(见 browser_focus 模块头)。
    let focus_lease = crate::agent::browser_focus::FocusLease::capture();
    if spec.bring_to_front {
        let _ = crate::agent::browser::BrowserManager::global()
            .bring_page_to_front(id)
            .await;
    }
    let url = match crate::agent::browser::BrowserManager::global().page(id).await {
        Some(pg) => pg.url().await.ok().flatten().unwrap_or_default(),
        None => String::new(),
    };

    // 第 152 轮:提问前先把验证弹层滚进视口中央(第 167 轮提前到解锁决策之前,
    // challenge_selector 同时供显式 partial 并洞)。验证弹窗落在下半屏/视口外时,
    // 下面那张自动附图截到的是空白区域,人工看不出要做什么;而 locked 档
    // `Input.setIgnoreInputEvents` 连滚轮一起拦,人工也滚不过去。fail-open。
    let challenge_scrolled = challenge_selector.clone();
    // 2026-10-13:弹窗验证码相关功能移除 —— 不再自动采集验证码截图(自动采集链已删);
    // 显式 `params.image_path` 仍透传(TUI 兜底打印路径供 `open`)。
    let (image_path, image_source, image_mode, image_clip) = {
        let (plan, source) = hitl_image_plan(reason, spec.image_path.as_deref());
        match plan {
            HitlImagePlan::Explicit(path) => (path, source, "explicit", Value::Null),
            HitlImagePlan::None => (String::new(), source, "none", Value::Null),
        }
    };

    // 2026-10-13:弹窗「解除屏蔽/恢复屏蔽」互切按钮的页面管控执行器。人工在弹窗
    // 点切换按钮 → 子进程 stdout guard 中间行 → hub.guard_toggle_from_ui → 本
    // handler(同步闭包,内部 tokio::spawn 落页):
    // - unlock=true → 整页 open(apply_hitl_unlock 内含导航重放协程,句柄存入
    //   manual_reassert,收口时必须 abort,否则会把恢复 locked 的页重新顶开);
    // - unlock=false → abort 手动重放 + 按本页解锁前快照恢复(缺失回退全局配置)。
    let manual_reassert: std::sync::Arc<
        std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(None));
    {
        let page_id_g = id.to_string();
        let snapshot_g = unlock_snapshot.clone();
        let manual_reassert_g = manual_reassert.clone();
        let hub = crate::agent::human_assist::HumanAssistHub::global();
        hub.set_guard_toggle_handler(Some(std::sync::Arc::new(move |unlock| {
            let page_id = page_id_g.clone();
            let snap = snapshot_g.clone();
            let manual_reassert = manual_reassert_g.clone();
            tokio::spawn(async move {
                let mgr = crate::agent::browser::BrowserManager::global();
                let Some(page) = mgr.page(&page_id).await else { return };
                if unlock {
                    if let Some(handle) = apply_hitl_unlock(&page, &[]).await {
                        let mut slot =
                            manual_reassert.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(old) = slot.replace(handle) {
                            old.abort();
                        }
                    }
                } else {
                    if let Some(old) =
                        manual_reassert.lock().unwrap_or_else(|p| p.into_inner()).take()
                    {
                        old.abort();
                    }
                    let cfg = match snap {
                        Some(c) => c,
                        None => mgr.current_guard().await,
                    };
                    let _ =
                        crate::agent::browser_overlay::apply_page_guard(&page, &cfg).await;
                    // 人工手动「恢复屏蔽」:管控回档,两条栏同步复显(与收口同构)。
                    crate::agent::browser_status::set_suspended(&page, false).await;
                }
            });
        })));
    }
    // 2026-10-13(二):弹窗「侵入式」触控板 → CDP 可信物理输入回放处理器。
    // - session_start:放行本页(复用与 guard 切换同一套解锁路径)+ 解析映射区
    //   (显式 relay_rect > 挑战层自动探测 > 全视口)+ 窗口置前(人工在浏览器里看着
    //   自己的手势被回放,视觉闭环);
    // - down/move/up/tap/key:落页回放(坐标归一化 → 映射区/视口像素);
    // - session_end:清中继状态;管控恢复统一走收口路径(下方)与 guard 切换(不重复)。
    {
        let page_id_r = id.to_string();
        let relay_rect_r = spec.relay_rect.clone();
        let snapshot_r = unlock_snapshot.clone();
        let manual_reassert_r = manual_reassert.clone();
        let rect_slot: std::sync::Arc<
            std::sync::Mutex<Option<(f64, f64, f64, f64)>>,
        > = std::sync::Arc::new(std::sync::Mutex::new(None));
        let hub = crate::agent::human_assist::HumanAssistHub::global();
        hub.set_relay_handler(Some(std::sync::Arc::new(move |ev| {
            let page_id = page_id_r.clone();
            let explicit = relay_rect_r.clone();
            let snap = snapshot_r.clone();
            let manual_reassert = manual_reassert_r.clone();
            let rect_slot = rect_slot.clone();
            let kind = ev.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
            let nx = ev.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let ny = ev.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let key = ev.get("key").and_then(Value::as_str).unwrap_or("").to_string();
            tokio::spawn(async move {
                let mgr = crate::agent::browser::BrowserManager::global();
                match kind.as_str() {
                    "session_start" => {
                        let Some(page) = mgr.page(&page_id).await else { return };
                        // ① 放行页面:locked 档自有输入闸只拦 isTrusted=true,
                        //    不放行会把我们自己的物理回放一起拦掉。
                        if let Some(handle) = apply_hitl_unlock(&page, &[]).await {
                            let mut slot =
                                manual_reassert.lock().unwrap_or_else(|p| p.into_inner());
                            if let Some(old) = slot.replace(handle) {
                                old.abort();
                            }
                        }
                        // ② 映射区解析(显式 > 自动探测挑战层 > None=全视口)。
                        let rect = match parse_relay_rect(&explicit) {
                            Some(r) => Some(r),
                            None => crate::agent::browser::relay_rect_probe(&page).await,
                        };
                        *rect_slot.lock().unwrap_or_else(|p| p.into_inner()) = rect;
                        // ③ 窗口置前(人工看着回放发生;失败不阻断)。
                        let _ = mgr.bring_page_to_front(&page_id).await;
                    }
                    "session_end" => {
                        mgr.relay_reset(&page_id);
                        *rect_slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
                        // 恢复解锁前管控快照(与 guard 切换的 relock 同构;
                        // 收口路径还会再恢复一次,幂等)。
                        if let Some(old) =
                            manual_reassert.lock().unwrap_or_else(|p| p.into_inner()).take()
                        {
                            old.abort();
                        }
                        let Some(page) = mgr.page(&page_id).await else { return };
                        let cfg = match snap {
                            Some(c) => c,
                            None => mgr.current_guard().await,
                        };
                        let _ =
                            crate::agent::browser_overlay::apply_page_guard(&page, &cfg).await;
                        // 侵入式中继收口:管控恢复,两条栏同步复显。
                        crate::agent::browser_status::set_suspended(&page, false).await;
                    }
                    "down" | "move" | "up" | "tap" => {
                        let rect =
                            *rect_slot.lock().unwrap_or_else(|p| p.into_inner());
                        let _ = mgr.relay_mouse(&page_id, &kind, nx, ny, rect).await;
                    }
                    "key" => {
                        let _ = mgr.relay_key(&page_id, &key).await;
                    }
                    _ => {}
                }
            });
        })));
    }

    // 2026-10-13:弹窗切换按钮初始标题依据 —— 页面本就已放行(open/无管控)则显示
    // 「🔒 恢复屏蔽」,locked/partial 显示「🔓 解除屏蔽」。
    let guard_unlocked = !unlock_need.0;

    let outcome = crate::agent::human_assist::HumanAssistHub::global()
        .request(
            reason,
            &message,
            options.clone(),
            &url,
            id,
            timeout_ms,
            &image_path,
            guard_unlocked,
            spec.multi_select,
            spec.relay_rect.clone(),
        )
        .await;
    // 2026-10-13:收口即清除弹窗切换 handler(防陈旧 handler 泄漏到下一请求;
    // hub 清槽路径另有防御性清除,双保险)。
    crate::agent::human_assist::HumanAssistHub::global().set_guard_toggle_handler(None);
    // 2026-10-13(二):侵入式中继 handler 同步清除(双保险,防陈旧 handler 泄漏)。
    crate::agent::human_assist::HumanAssistHub::global().set_relay_handler(None);
    // 2026-10-13:人工手动解锁触发的导航重放协程一并收口(否则会把下面的恢复动作
    // 重新顶开)。
    if let Some(handle) = manual_reassert
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
    {
        handle.abort();
    }
    // 第 143/144/167 轮:人工介入收口(应答/超时/取消/不可用)后恢复管控
    // —— 无论何种结局,「人工可操作窗口」都必须关闭:先回收导航重放协程(防恢复后
    // 又被重放覆盖),再恢复原档。第 167 轮:恢复态取**本页解锁前实际态快照**
    // (多窗口并行下各页 guard 独立,全局 current_guard() 会被其它页的 set_guard
    // 改写,拿它灌页面会把 A 页的档错装到 B 页);快照缺失时回退全局配置。
    // fail-open(恢复失败不阻断信封返回,下一个输入动作的「先解/先隐」与导航
    // re-assert 会自愈)。
    if unlock_applied {
        if let Some(handle) = unlock_reassert.take() {
            handle.abort();
        }
        if let Some(page) = crate::agent::browser::BrowserManager::global().page(id).await {
            let cfg = match unlock_snapshot.take() {
                Some(c) => c,
                // 解锁成功应用过则快照必然在场;此分支仅防御(如页面在解锁后
                // 被关闭重开,guard 状态已随新文档重置)。
                None => crate::agent::browser::BrowserManager::global().current_guard().await,
            };
            let _ = crate::agent::browser_overlay::apply_page_guard(&page, &cfg).await;
            // 人工操作窗口关闭:两条栏复显(挂起点在 apply_hitl_unlock,对称收口)。
            crate::agent::browser_status::set_suspended(&page, false).await;
        }
    }
    // 焦点租约收口:人工介入结束,键盘焦点归还提问前的前台应用(fail-open)。
    focus_lease.release();
    use crate::agent::human_assist::{AssistVia, HumanAssistOutcome};
    match outcome {
        HumanAssistOutcome::Answered { text: answer, via } => HitlReport {
            code: 0,
            message: "人工已响应".to_string(),
            status: HitlStatus::Answered,
            data: json!({
                "reason": reason,
                "kind_label": label,
                "human_response": answer,
                "assist_channel": via.as_str(),
                // 2026-10-13:发起来源对账(llm 显式 / auto:input_gate|blocker_probe)。
                "origin": match &spec.origin {
                    HitlOrigin::Llm => "llm".to_string(),
                    HitlOrigin::Auto { trigger } => format!("auto:{trigger}"),
                },
                // 第 144/167 轮:提问期间页面放行方式对账(open=安全全面解锁(缺省)/
                // partial=显式 allow_selectors 挖洞 / none=未放行(页面本就 open 或
                // 未装管控);source: safe_open|explicit|explicit_open_uncovered|none;
                // verify: 显式 partial 的覆盖验证结论,challenge: 并洞的挑战层选择器)。
                "page_unlock": json!({
                    "applied": unlock_applied,
                    "mode": if !unlock_applied { "none" }
                        else if unlock_allow.is_empty() { "open" }
                        else { "partial" },
                    "allow_selectors": unlock_allow,
                    "source": unlock_source,
                    "verify": unlock_verify,
                    "challenge": challenge_selector,
                }),
                // 2026-10-13:附图对账(仅显式 image_path 透传;弹窗不再渲染图片,
                // 自动采集链已删,TUI 兜底打印路径供 open)
                "image_path": image_path,
                "image_source": image_source,
                "image_mode": image_mode,
                "image_clip": image_clip,
                // 第 152 轮:提问前是否把验证弹层滚进了视口(人工能否看到)
                "challenge_scrolled": challenge_scrolled,
                // 第 145 轮:按「输码 vs 已完成」分流 —— 自由文本(验证码)+ captcha/
                // sms/two_factor → 立即 input_text 填码提交;选项应答 → inspect 验证。
                "next_hint": crate::agent::tools::mcp_web_use::hitl_hint::hitl_answer_hint(reason, &answer),
            }),
        },
        HumanAssistOutcome::Timeout { via } => HitlReport {
            code: 4001,
            message: "人工介入等待超时".to_string(),
            status: HitlStatus::Timeout,
            data: json!({
                "reason": reason,
                "kind_label": label,
                "assist_channel": via.as_str(),
                "hint": "等待人工响应超时。可再次 request_human 重试,或如实向用户报告需要人工配合后结束",
            }),
        },
        HumanAssistOutcome::Cancelled { via } => HitlReport {
            code: 4002,
            message: "人工已取消介入".to_string(),
            status: HitlStatus::Cancelled,
            data: json!({
                "reason": reason,
                "kind_label": label,
                "assist_channel": via.as_str(),
                "hint": "人工选择取消。终止当前浏览器流程,汇总已完成部分向用户报告,不要继续重试",
            }),
        },
        HumanAssistOutcome::Unavailable => HitlReport {
            code: 4001,
            message: "人工介入不可用(弹窗与 TUI 均不可用)".to_string(),
            status: HitlStatus::Unavailable,
            data: json!({
                "reason": reason,
                "kind_label": label,
                "assist_channel": AssistVia::Tui.as_str(),
                // 第 131 轮:原信封只说「不可用」,用户无从判断是没弹、弹失败、
                // 还是压根没走到这一步。附本机能力诊断以便排障。
                "human_ui_diagnostics": crate::agent::human_ui::diagnostics(),
                "hint": "当前环境既无桌面弹窗(macOS/Windows 会话)也无 TUI 交互。如实告知用户:请在 laew TUI 交互模式或桌面会话下重新执行该任务",
            }),
        },
    }
}

/// control_action=request_human:人工介入请求(HITL 闭环,LLM 显式调用入口)。
///
/// 流程:可选 `bring_to_front`(默认 true)把浏览器窗口带到前台 → 附图仅显式
/// `params.image_path` 透传(2026-10-13 起弹窗不再渲染图片,自动验证码截图链已删)
/// → 经 HumanAssistHub 发起结构化提问
/// (macOS/Windows 桌面**弹窗优先**(第 130 轮,置顶+倒计时+时间轴+可选中复制+
/// 「解除屏蔽/恢复屏蔽」互切按钮+单选/多选选项),TUI 兜底行读)→ oneshot 回填。
/// 返回专用信封:
/// - code=0:`data.human_response` 为人工输入(选项文本、多选编码或自由文本),
///   `data.assist_channel` 标注应答通道(gui/tui);
/// - code=4001:超时 / 弹窗与 TUI 均不可用;
/// - code=4002:人工明确取消(弹窗取消/Esc/关窗,或 TUI 输入 q/取消)。
pub(super) async fn act_request_human(
    id: &str,
    p: &Value,
) -> crate::error::Result<String> {
    // page_id 有效性前置校验(2000 语义与其它动作一致)
    if crate::agent::browser::BrowserManager::global().page(id).await.is_none() {
        return envelope(2000, "page_id 不存在", json!({"page_id": id}));
    }
    let reason = str_arg(p, "reason").unwrap_or("custom");
    if !HUMAN_ASSIST_ALLOWED_REASONS.contains(&reason) {
        return envelope(
            1001,
            &format!("非法 reason(允许 {})", human_assist_reasons_doc()),
            json!({"reason": reason, "allowed": HUMAN_ASSIST_ALLOWED_REASONS}),
        );
    }
    let options: Vec<String> = p
        .get("options")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let allow_selectors = p
        .get("allow_selectors")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let spec = HitlSpec {
        reason: reason.to_string(),
        message: str_arg(p, "message").unwrap_or_default().to_string(),
        options,
        // 2026-10-13:选项多选(弹窗复选框组;应答编码 `1. a；3. c`)。
        multi_select: p.get("multi_select").and_then(Value::as_bool).unwrap_or(false),
        timeout_ms: p.get("timeout_ms").and_then(Value::as_u64),
        bring_to_front: p.get("bring_to_front").and_then(Value::as_bool).unwrap_or(true),
        unlock_page: p.get("unlock_page").and_then(Value::as_bool).unwrap_or(true),
        allow_selectors,
        image_path: str_arg(p, "image_path").map(str::to_string),
        // 2026-10-13(二):侵入式映射区(可选;数值字段缺失/非法 → None 走自动探测)。
        relay_rect: p
            .get("relay_rect")
            .filter(|v| v.is_object())
            .cloned(),
        origin: HitlOrigin::Llm,
    };
    // 第 152 轮:人工介入是「页面状态已被人改变」的新事实 —— 清掉本页的告警指纹,
    // 否则「弹窗还在 → 但上次已告警过 → 不再提醒」会造成漏报。(仅 LLM 显式路径;
    // auto 路径保留指纹表:同一挑战重复命中只回 repeat 事实,不重复弹窗。)
    crate::agent::tools::mcp_web_use::blocker_probe::forget_fingerprints(id);
    let report = request_human_core(id, &spec).await;
    envelope(report.code, &report.message, report.data)
}

/// 工具层**自动弹出**人工介入(auto-HITL,2026-10-13)。
///
/// 触发方:blocker_probe 的输入前置硬闸(6002,`input_gate`)与收尾探针
/// (`blocker_probe`,仅首次命中)。守卫条件(任一不满足返回 None,调用方回退
/// 「提示载荷 → LLM 显式 request_human」的既有语义):
/// 1. `LAEW_HITL_AUTO` 未关闭(off/0/false/no,缺省开);
/// 2. **不在 sequence/batch 上下文**(批内单步 150s 预算会截断 300s 分档的
///    人工等待;批内由 LLM 以独立 request_human 步介入,该步已豁免预算);
/// 3. hub 无 pending(槽位占用说明已有人在等应答,不排队叠加、不重复打扰)。
///
/// 返回 `Some(HitlReport)` 时,`data` 已并入 `auto_hitl{trigger,status,...}`
/// 对账字段(status=answered 时含 human_response/next_hint,可直接驱动下一步)。
pub(in crate::agent::tools::mcp_web_use) async fn auto_hitl(
    page_id: &str,
    reason: &str,
    message: &str,
    trigger: &'static str,
) -> Option<HitlReport> {
    use crate::agent::tools::mcp_web_use::{blocker_probe, in_sequence_ctx};
    if !blocker_probe::hitl_auto_enabled() {
        return None;
    }
    if in_sequence_ctx() {
        return None;
    }
    if crate::agent::human_assist::HumanAssistHub::global().is_pending() {
        return None;
    }
    let reason = if HUMAN_ASSIST_ALLOWED_REASONS.contains(&reason) {
        reason
    } else {
        "manual_verify"
    };
    let spec = HitlSpec {
        reason: reason.to_string(),
        message: message.to_string(),
        options: blocker_probe::HITL_CHALLENGE_OPTIONS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        multi_select: false,
        timeout_ms: None,
        bring_to_front: true,
        unlock_page: true,
        allow_selectors: Vec::new(),
        image_path: None,
        // auto 路径不预置映射区:session_start 时自动探测挑战层(输入硬闸/收尾探针
        // 命中的挑战弹层正是侵入式要操作的目标)。
        relay_rect: None,
        origin: HitlOrigin::Auto { trigger },
    };
    let mut report = request_human_core(page_id, &spec).await;
    // 标注自动介入事实(调用方按自身语义包装信封码,如 6002 拦截信封)。
    let auto = json!({
        "trigger": trigger,
        "status": report.status.as_str(),
    });
    if let Some(dst) = report.data.as_object_mut() {
        dst.insert("auto_hitl".to_string(), auto);
    }
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-13(二):侵入式映射区解析(显式 relay_rect 的容错通道)。
    #[test]
    fn parse_relay_rect_is_fail_open() {
        assert_eq!(parse_relay_rect(&None), None);
        assert_eq!(
            parse_relay_rect(&Some(serde_json::json!({
                "x": 1.0, "y": 2.0, "width": 0.0, "height": 10.0
            }))),
            None,
            "宽高非正 → None"
        );
        assert_eq!(
            parse_relay_rect(&Some(serde_json::json!({
                "x": 10.0, "y": 20.0, "width": 300.0, "height": 120.0
            }))),
            Some((10.0, 20.0, 300.0, 120.0))
        );
        // 非对象 → None
        assert_eq!(parse_relay_rect(&Some(serde_json::json!("x"))), None);
    }
}
