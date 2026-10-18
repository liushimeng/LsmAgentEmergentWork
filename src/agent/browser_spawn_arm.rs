//! 派生页首文档早期武装（第 180 轮，`docs/MCP_Web_Use/34`）。
//!
//! 背景：由页面自己派生的新标签页（`<a target=_blank>` 点击、`window.open`、
//! `control(new_tab)`）此前要等到 `adopt_spawned_pages_ex`（触发动作收尾时巡检
//! `browser.pages()`）才能被反检测处置 —— 首个文档的 document-start 跑在裸 CDP
//! 环境里：无头档 `screen=800x600` 而 `outer=1920`（`screen_geometry_impossible`
//! 硬判据，且 adopt 序列从不补 screen_sync）、UA 带 `HeadlessChrome` 令牌、
//! 保真回填未挂。sequence/batch 批内前几步派生的页更要等到批结束。
//!
//! 本模块的 watcher 任务消费浏览器级 `Target.targetCreated` 事件流，对「页面派生
//! 的新 page target」（`type=="page"` 且 `opener_id` 存在 —— laew 自己 `open`/
//! `new_window` 经 `Target.createTarget` 创建的页 opener 恒空，天然排除）在出生
//! 时刻立即武装：隐身脚本（抢 document-start）→ 屏幕几何自洽 → UA 去令牌（含
//! 落地后补全）→ headed 呈现层对齐 adopt。
//!
//! 治理铁律：
//! - watcher 自身**不持有 `BrowserManager.inner` 锁**；页面句柄与配置快照经
//!   [`BrowserManager::spawn_arm_resolve`] **有界**获取（`new_page_impl` 持锁可跨
//!   launch+goto 达 30s+，锁等待 3s 封顶拿不到就放弃，交 adopt 收编兜底）；
//! - 全部 CDP 调用有界（handler 卡死时放弃，fail-open）；
//! - connect 模式不启动（接管用户浏览器零注入契约）；
//! - 杀开关 `LAEW_WEB_SPAWN_ARM=off` 回退第 179 轮行为（adopt 时武装）。

use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::target::EventTargetCreated;
use chromiumoxide::listeners::EventStream;
use futures::StreamExt;

use super::browser_mode::BrowserMode;
use super::browser_ns::UaSweep;
use super::stealth::StealthMode;

/// 派生页早期武装的杀开关：`LAEW_WEB_SPAWN_ARM=off/0/false/no/关闭` 关闭。
pub(crate) fn kill_switch_from_value(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "false" | "no" | "关闭"
    )
}

/// 当前进程是否启用派生页早期武装（缺省开启）。
pub(crate) fn enabled() -> bool {
    !kill_switch_from_value(&std::env::var("LAEW_WEB_SPAWN_ARM").unwrap_or_default())
}

/// 资格过滤（纯函数，单测锁死）：`type=="page"` 且带 opener（= 页面派生）。
///
/// 不能放宽为「所有新 page target」：laew 自己 `open` 的页会在 `new_page_impl`
/// 同步武装，watcher 再插一手会与 UA 二段式（restore→覆盖）交错竞争；也不能收紧为
/// 「opener 是注册表内页面」：链式派生（孙页的 opener 是尚未收编的子页）会漏。
/// `window.open(url,'_blank','noopener')` 同样命中 —— TargetInfo 的 opener 是
/// **创建者**（创建该 target 的页面），不是 `window.opener` JS 关系。
fn spawn_arm_eligible(target_type: &str, opener_present: bool) -> bool {
    target_type == "page" && opener_present
}

/// 武装一个派生 target 所需的实例期望态快照（browser.rs 侧构造）。
#[derive(Debug, Clone)]
pub(crate) struct SpawnArmConfig {
    /// 反检测档位期望态（与 `BrowserInner.stealth` 同源）。
    pub(crate) stealth: StealthMode,
    /// 无头档屏幕几何自洽尺寸（`BrowserInner.screen_sync`，headed/connect 恒 None）。
    pub(crate) screen_sync: Option<(u32, u32)>,
    /// **浏览器级 UA 字符串的去令牌结果**（第 180 轮）：实例是无头档且
    /// `Browser.getVersion` 的 UA 带令牌时为 `Some(清理后字符串)`。派生页出生
    /// 时刻无需页面执行上下文即可先行下发 string-only 覆盖(探针式清理会被
    /// provisional 导航阻塞,赶不上 document-start);品牌表由落地后补全收口。
    /// 有头档/connect 恒 `None`(本就无令牌,零动作)。
    pub(crate) ua_string_sweep: Option<String>,
    /// 页面管控期望态（headed 呈现层对齐 adopt 用）。
    pub(crate) guard: super::browser_overlay::PageGuardConfig,
    /// Agent 高亮蓝框是否启用。
    pub(crate) highlight: bool,
    /// 实例启动模式。
    pub(crate) mode: BrowserMode,
    /// 自动跟随期望态。
    pub(crate) auto_follow: bool,
    /// connect 模式（watcher 本不启动，保留字段做防御性判断）。
    pub(crate) connect_mode: bool,
}

/// watcher 主循环：消费 `Target.targetCreated` 事件流直到连接关闭。
///
/// 事件流由调用方（`new_page_impl`，launch 模式 + 开关开启）在实例建立时注册
/// `browser.event_listener::<EventTargetCreated>()` 取得 —— watcher 全程不持有
/// `Browser` 句柄（chromiumoxide 的 `Browser` 非 Clone），页面句柄与配置快照经
/// [`super::browser::BrowserManager::spawn_arm_resolve`] 按事件有界解析。
pub(crate) async fn watch(mut stream: EventStream<EventTargetCreated>) {
    tracing::info!("派生页早期武装 watcher 已启动(页面派生的新标签页提交即处置)");
    while let Some(ev) = stream.next().await {
        let info = &ev.target_info;
        let url = info.url.clone();
        let tid = info.target_id.clone();
        if !spawn_arm_eligible(&info.r#type, info.opener_id.is_some()) {
            continue;
        }
        tracing::debug!(url = %url, "派生页早期武装:收到 targetCreated 事件");
        // 每个 target 的武装独立成任务:单个慢页面(句柄解析/落地等待)不得
        // 阻塞事件流上后续 target 的早期武装。
        tokio::spawn(async move {
            arm_spawned_target(tid, url).await;
        });
    }
}

/// 武装单个派生 target(全链 fail-open:任一步失败只留痕,不阻断页面本身)。
async fn arm_spawned_target(
    tid: chromiumoxide::cdp::browser_protocol::target::TargetId,
    url: String,
) {
    let resolve_started = Instant::now();
    let (page, cfg) =
        match super::browser::BrowserManager::global().spawn_arm_resolve(&tid).await {
            Some(v) => v,
            None => return,
        };
    tracing::debug!(
        url = %url,
        elapsed_ms = resolve_started.elapsed().as_millis() as u64,
        "派生页早期武装:句柄解析完成"
    );
    // ① 隐身脚本最先:addScriptToEvaluateOnNewDocument 抢的是新文档 document-start
    //    执行序。**提交门约束**(实测钉死,见 docs/MCP_Web_Use/34 §三):chromiumoxide
    //    把页面句柄的全部命令压在 target init 链之后,而 init 链要等 provisional
    //    导航**提交**才完成 —— 派生页首个文档的 document-start(内联脚本级)物理
    //    赶不上,本序列落地在提交后 ~5ms;此后的一切(子资源请求/XHR/信标/后续
    //    文档)全部在处置之下。
    super::browser_ns::inject_stealth(&page, cfg.stealth).await;
    // ② 屏幕几何自洽(无头档):chromiumoxide init 链的 setDeviceMetricsOverride
    //    不带 screen 字段,不补则派生页整个首生命周期带着 outer>screen 硬判据。
    //    本命令排在 init 链之后(提交门),不会被 init 的无 screen 覆盖回抹。
    super::browser::apply_screen_sync_override(&page, cfg.screen_sync).await;
    // ②b 指针/悬停媒体特性 + 触摸点仿真(第 182 轮):与 screen_sync 同批下发,
    // 派生页首个文档起 `matchMedia` 四元与 maxTouchPoints 即与有头档对齐。
    // 纯 CDP 仿真零注入面;有头档零动作;watcher 只服务于 launch 实例。
    super::browser_env_emulation::apply_pointer_env_emulation(&page, cfg.stealth, cfg.mode).await;
    // ③ UA 去令牌。无头档走**无探针通道**:覆盖值来自浏览器级 `getVersion` 的
    //    UA 字符串(实例建立时已探好存快照),派生页出生即下发 —— 探针式清理
    //    (`apply_ua_destealth` 的 `Runtime.evaluate`)在首个文档提交前可能阻塞在
    //    provisional 导航上(实测 400ms+),赶不上 document-start。string-only 覆盖
    //    的品牌表清空副作用由 ⑤ 落地后补全收口(与 `new_page_impl` 首航同语义)。
    //    有头档(快照无值)退回探针式:有头本就无令牌,探一次即 Skipped,零负担。
    let sweep = if let Some(ua) = cfg.ua_string_sweep.as_ref() {
        let ok = super::browser_ns::apply_ua_string_override(&page, ua).await;
        if ok {
            UaSweep::StringOnly
        } else {
            tracing::warn!(url = %url, "派生页出生 UA 字符串覆盖下发失败,退回探针式清理");
            super::browser_ns::apply_ua_destealth(&page, cfg.stealth).await
        }
    } else {
        super::browser_ns::apply_ua_destealth(&page, cfg.stealth).await
    };
    // ④ headed 呈现层对齐 adopt:派生页从出生即受 locked 管控(人工可看不可点,
    //    不再有「派生后到 adopt 前可被人工误点」的空窗)。全部幂等,adopt 后到
    //    重装一次零风险。
    if !cfg.connect_mode && cfg.highlight && matches!(cfg.mode, BrowserMode::Headed) {
        super::browser::inject_agent_highlight(&page).await;
    }
    if !cfg.connect_mode && matches!(cfg.mode, BrowserMode::Headed) {
        super::browser_overlay::inject_agent_guard(&page, &cfg.guard).await;
        let _ = super::browser_overlay::apply_page_guard(&page, &cfg.guard).await;
    }
    if !cfg.connect_mode && cfg.auto_follow {
        super::browser_follow::install(&page, true).await;
        let _ = super::browser_follow::set(&page, true).await;
    }
    tracing::info!(url = %url, sweep = ?sweep, "派生页早期武装完成(隐身/屏幕几何/UA 已就位)");
    // ⑤ UA 落地后补全:有头档(sweep 恒 Skipped 且无 screen_sync)不起后续任务,
    //    零开销;无头档或 StringOnly 需要等真实文档落地后补全品牌表 + 重发 screen。
    if sweep != UaSweep::Full && (cfg.screen_sync.is_some() || sweep == UaSweep::StringOnly) {
        let deadline = Instant::now() + Duration::from_secs(20);
        tokio::spawn(async move {
            complete_after_landing(page, cfg, sweep, deadline).await;
        });
    }
}

/// UA 落地后补全(复用 178 轮二段式语义):等 URL 离开 about:blank 后
/// `restore_native_ua` 撤掉「只清了字符串」的半吊子覆盖 → 重探 → 下发带品牌表的
/// 完整覆盖;并**重发一次 screen_sync** —— 兜底与 chromiumoxide init 链的竞态
/// (我方覆盖先到、init 不带 screen 字段的覆盖后到会把 screen 抹回 800x600)。
async fn complete_after_landing(
    page: chromiumoxide::Page,
    cfg: SpawnArmConfig,
    initial: UaSweep,
    deadline: Instant,
) {
    let started = Instant::now();
    let mut restored = false;
    // screen 重发两跳:①出生 300ms 后一次(兜底与 chromiumoxide init 链
    // `setDeviceMetricsOverride`(不带 screen 字段)的竞态 —— 我方覆盖先到、init
    // 的后到会把 screen 抹回 800x600;init 链正常 <100ms 完成,300ms 裕量足够);
    // ②真实文档落地后一次(init 竞态窗口彻底关闭)。
    let mut screen_early_sent = false;
    let mut screen_landing_sent = false;
    let mut probe_retries = 0u8;
    loop {
        if Instant::now() >= deadline {
            tracing::warn!("派生页 UA 落地后补全超时放弃(20s):首文档可能带 Headless 令牌");
            return;
        }
        let url = match page.url().await {
            Ok(Some(u)) => u,
            Ok(None) => String::new(),
            // 会话已死(target 被关):静默退出。
            Err(_) => return,
        };
        let landed = !url.is_empty() && url != "about:blank";
        if !screen_early_sent && started.elapsed() >= Duration::from_millis(300) {
            super::browser::apply_screen_sync_override(&page, cfg.screen_sync).await;
            screen_early_sent = true;
        }
        if landed && !screen_landing_sent {
            super::browser::apply_screen_sync_override(&page, cfg.screen_sync).await;
            screen_landing_sent = true;
        }
        if !landed {
            tokio::time::sleep(Duration::from_millis(150)).await;
            continue;
        }
        if initial == UaSweep::StringOnly && !restored {
            super::browser_ns::restore_native_ua(&page).await;
            restored = true;
        }
        match super::browser_ns::apply_ua_destealth(&page, cfg.stealth).await {
            UaSweep::Full => {
                tracing::info!(url = %url, "派生页 UA 落地后补全完成(Client Hints 品牌表已齐)");
                return;
            }
            UaSweep::Skipped => {
                // 落地后仍 Skipped:要么环境本就干净(有头档路径,到此为止),
                // 要么探测偶发失败 —— 有界重试两轮后放弃。
                probe_retries += 1;
                if probe_retries >= 3 {
                    tracing::debug!(url = %url, "派生页 UA 落地后补全:探测无产出,按干净环境收口");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            UaSweep::StringOnly => {
                // 落地后品牌表仍滤空(浏览器自报就没有非自动化品牌):重探无意义,
                // 留痕收口,不空转烧预算。
                tracing::warn!(url = %url, "派生页 UA 落地后仍只有字符串清理(品牌表滤空)");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 资格过滤:只有「页面派生的 page target」才武装。
    #[test]
    fn 资格过滤_页面派生才武装() {
        assert!(spawn_arm_eligible("page", true), "页面派生的新标签页必须武装");
        assert!(
            !spawn_arm_eligible("page", false),
            "laew 自己 open/初始页签/人工 Ctrl+T(opener 恒空)不得插手 —— 它们由 new_page_impl 同步武装"
        );
        assert!(
            !spawn_arm_eligible("iframe", true),
            "OOPIF target(type=iframe)不单独武装 —— 隐身脚本经 addScriptToEvaluateOnNewDocument 的全 frame 语义覆盖"
        );
        assert!(!spawn_arm_eligible("service_worker", true));
        assert!(!spawn_arm_eligible("shared_worker", false));
        assert!(!spawn_arm_eligible("browser", false));
    }

    /// 杀开关解析:off 系关、其余(含未设置空串)开。
    #[test]
    fn 杀开关解析() {
        for v in ["off", "0", "false", "no", "关闭", " OFF ", "No"] {
            assert!(kill_switch_from_value(v), "{v:?} 应识别为关闭");
        }
        for v in ["", "1", "on", "true", "是的"] {
            assert!(!kill_switch_from_value(v), "{v:?} 应保持开启(缺省)");
        }
    }
}
