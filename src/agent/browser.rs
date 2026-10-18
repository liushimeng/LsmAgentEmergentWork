//! MCP_Web_Use 工具的 CDP 浏览器驱动层(2026-09-18 第 89 轮起,
//! 原 Chromium-WebUse Agent 的「MCP 服务」实现,Agent 删除后由
//! SubAgent-Work 的 MCP_Web_Use 工具调用,能力零改写保留)。
//!
//! 进程内单浏览器 + 多 Page 注册表语义:`page_id` 对 LLM 不透明,
//! action=open 首次调用时启动(内存无头 `--headless=new`)/ 接管(connect_url)浏览器,
//! action=close 在最后一个页面关闭后回收浏览器进程(launch 模式)。
//! 平台浏览器路径差异封闭在 [`detect_browser`]。
//!
//! 派生标签页(window.open / target=_blank / 中键点击)通过「动作前后 diff
//! `browser.pages()`」adopt 进注册表,新 page_id 经响应 `spawned_page_id` 回传。
//!
//! 设计见 `docs/MCP_Web_Use/01-设计与解决方案.md`;
//! 技术参考 `docs/浏览器CDP工具/Rust操作Chrome浏览器CDP完整技术方案.md`。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::browser::{
    Bounds, EventDownloadProgress, EventDownloadWillBegin, GetWindowForTargetParams,
    SetDownloadBehaviorBehavior, SetDownloadBehaviorParams, SetWindowBoundsParams, WindowState,
};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::browser_perm::{self, GeoMode};

/// Console / Network 事件环形缓冲容量(每页)。
const EVENT_RING_CAP: usize = 500;

/// 浏览器事件(Console / Network)统一条目,直接序列化进工具响应。
#[derive(Debug, Clone)]
pub struct PageEvent {
    pub ts_ms: u128,
    pub data: Value,
}

/// 每页事件缓冲:环形队列 + 最后事件时间(健康度判定)。
#[derive(Debug, Default, Clone)]
pub struct EventBuffer {
    pub console: Arc<std::sync::Mutex<VecDeque<PageEvent>>>,
    pub network: Arc<std::sync::Mutex<VecDeque<PageEvent>>>,
    pub last_event_at: Arc<std::sync::Mutex<Option<u128>>>,
}

impl EventBuffer {
    fn push(
        ring: &Arc<std::sync::Mutex<VecDeque<PageEvent>>>,
        ev: PageEvent,
        last: &Arc<std::sync::Mutex<Option<u128>>>,
    ) {
        if let Ok(mut q) = ring.lock() {
            if q.len() >= EVENT_RING_CAP {
                q.pop_front();
            }
            q.push_back(ev);
        }
        if let Ok(mut t) = last.lock() {
            *t = Some(now_millis());
        }
    }

    pub fn push_console(&self, data: Value) {
        Self::push(
            &self.console,
            PageEvent {
                ts_ms: now_millis(),
                data,
            },
            &self.last_event_at,
        );
    }

    pub fn push_network(&self, data: Value) {
        Self::push(
            &self.network,
            PageEvent {
                ts_ms: now_millis(),
                data,
            },
            &self.last_event_at,
        );
    }

    /// 采集健康度:60s 无新事件视为停滞(对齐 go-web-debug-tool collection_healthy)。
    pub fn collection_healthy(&self) -> (bool, Option<u128>) {
        let last = self.last_event_at.lock().ok().and_then(|t| *t);
        let healthy = match last {
            Some(t) => now_millis().saturating_sub(t) < 60_000,
            None => true, // 尚无事件不算不健康(页面可能本来就安静)
        };
        (healthy, last)
    }
}

pub struct PageEntry {
    pub page: Page,
    pub target_id: String,
    pub created_at: String,
    pub events: EventBuffer,
}

struct BrowserInner {
    browser: Option<Browser>,
    handler: Option<tokio::task::JoinHandle<()>>,
    /// 派生页早期武装 watcher（第 180 轮，仅 launch 模式启动；与 handler 同点回收）。
    /// 订阅浏览器级 `Target.targetCreated`,对「页面派生的新标签页」在导航提交时刻
    /// 下发隐身/屏幕几何/UA 处置(见 `browser_spawn_arm.rs`;提交门约束见
    /// `docs/MCP_Web_Use/34` §二 —— 命令经 init 链在提交时刻生效)。
    spawn_arm: Option<tokio::task::JoinHandle<()>>,
    /// 进程外 parent-death watchdog（仅 launch 模式）。
    watchdog: Option<std::process::Child>,
    connect_mode: bool,
    /// 第 175 轮:屏幕几何自洽尺寸(两档 headless 才有值)——
    /// chromiumoxide 的视口覆盖**不发** `screen_width`/`screen_height`,页面于是读到
    /// 无头默认值 `screen=800x600` 而 `window.outerWidth=1920`(`outer > screen`
    /// 物理不可能,是零成本的风控硬判据)。此处记账的是**已知窗口尺寸**,
    /// 页面创建后据此补发一次同尺寸的 screen 声明,不是编造屏幕分辨率。
    /// headed / connect 恒 `None`(不覆盖视口,页面视口=窗口内容区,本就自洽)。
    screen_sync: Option<(u32, u32)>,
    /// connect 模式下对端的调试端口(2026-10-14 第二轮:判定本次接管的是否
    /// laew 自起的 auto_relaunch 调试实例 —— 它是 laew 身份(CDP 拉起+复制
    /// profile),指纹级打标站点对它的抹除要计入总账;用户日常 Chrome 不记)。
    /// launch 模式恒 None。
    connected_port: Option<u16>,
    /// 浏览器级 UA 字符串(第 180 轮,实例建立时 `Browser.getVersion` 有界探得):
    /// 派生页早期武装的无探针 UA 去令牌通道取值源(无头档带 `HeadlessChrome`
    /// 令牌时,清理后的字符串随 `SpawnArmConfig` 下发,提交后的网络请求与页面
    /// 读数立即干净;探针式清理会被 provisional 导航阻塞,赶不上提交时刻)。
    browser_ua: Option<String>,
    pages: HashMap<String, PageEntry>,
    /// launch 模式的一次性 user-data-dir(关闭浏览器时整目录清理)。
    user_data_dir: Option<PathBuf>,
    /// 当前浏览器实例的启动模式(open 复用判定 / adopted 页高亮注入用)。
    mode: BrowserMode,
    /// Agent 高亮蓝框是否启用(headed 可视化场景标识 Agent 控制的窗口)。
    highlight: bool,
    /// 反检测档位期望态(第 174 轮):`open(stealth=…)` / `control(set_stealth)` 显式
    /// 覆盖,缺省取进程级 `LAEW_WEB_STEALTH`(`stealth::mode()`)。页面创建/复用/
    /// 自愈/派生四条路径共用同一次注入,与 `highlight` 同构。
    stealth: super::stealth::StealthMode,
    /// 页面管控期望态(第 143 轮:locked 屏蔽/open 开放/partial 部分屏蔽;
    /// 第 141 轮的 `overlay: bool` 升级而来,locked 档语义与蒙层完全一致)。
    guard: super::browser_overlay::PageGuardConfig,
    /// 自动跟随最新内容期望态(第 152 轮,默认开):导航后新文档按它重挂跟随器,
    /// 与 guard 的期望态 re-assert 同构。connect 模式恒 false(不干预用户浏览器)。
    auto_follow: bool,
    /// 调用方是否**显式**指定了窗口尺寸(第 156 轮)。
    ///
    /// `false`(缺省)= 由驱动层给 1920×1080,此时 headed 收边按屏幕工作区**铺满**;
    /// `true` = 用户/Agent 明确给了 `window` 参数,只做「不溢出屏幕」收边,不覆盖意图。
    /// connect 模式恒 false 且不参与决策(整路跳过)。
    explicit_window: bool,
    /// F2(2026-10-16):每页「真实可见区」(视口 2K 扩展**覆盖前**的 inner 尺寸)。
    /// `setDeviceMetricsOverride` 撑大布局视口后,`position:fixed;bottom` 锚定的
    /// 注入层(进度栏)会落到布局视口底边 —— 在可见窗口之外(实测布局视口 1440、
    /// 可见窗口 948,进度栏 y=1338 整条不可见)。告知栏据此把 bottom/right 重锚到
    /// 可见区;覆盖被清除(sync_viewport / set_window_bounds)时移除,恢复自然锚定。
    page_visible_box: HashMap<String, (f64, f64)>,
    /// F3(2026-10-16):本浏览器实例是否已完成「默认窗口 1080p 一次性校正」。
    /// `--window-size=1920,1080` 在 Windows 150% 缩放 + 多显示器下不被精确执行
    /// (实测首开 outer 1922×1043,高短 37px,违背「默认 1080p」文档承诺);首开时
    /// 主动 setWindowBounds 校正一次,之后保持「用户调整以其为准」(第 158 轮)。
    default_size_applied: bool,
}

impl BrowserInner {
    /// 派生页早期武装的实例期望态快照(第 180 轮,`browser_spawn_arm::SpawnArmConfig`)。
    fn spawn_arm_config(&self) -> super::browser_spawn_arm::SpawnArmConfig {
        // 无探针 UA 去令牌通道:无头档(screen_sync 有值)+ 档位开启 + 浏览器级
        // UA 确带令牌时,预计算清理后的字符串 —— 派生页出生即下发,不等探针。
        let ua_string_sweep = match (self.screen_sync.is_some(), self.browser_ua.as_deref()) {
            (true, Some(ua))
                if self.stealth != super::stealth::StealthMode::Off
                    && super::stealth::is_headless_ua(ua) =>
            {
                Some(super::stealth::strip_headless_tokens(ua))
            }
            _ => None,
        };
        super::browser_spawn_arm::SpawnArmConfig {
            stealth: self.stealth,
            screen_sync: self.screen_sync,
            ua_string_sweep,
            guard: self.guard.clone(),
            highlight: self.highlight,
            mode: self.mode,
            auto_follow: self.auto_follow,
            connect_mode: self.connect_mode,
        }
    }
}

/// 浏览器会话管理器(进程内单例)。
pub struct BrowserManager {
    inner: Mutex<BrowserInner>,
}

/// 侵入式中继拖拽状态(每页:是否按下 + 上次物理坐标,供轨迹插值)。
#[derive(Default)]
struct RelayDragState {
    down: bool,
    last: Option<(f64, f64)>,
}

/// 进程级中继状态表(侵入式会话专用;收口由 [`BrowserManager::relay_reset`] 清理)。
fn relay_state() -> &'static std::sync::Mutex<HashMap<String, RelayDragState>> {
    static ST: std::sync::OnceLock<std::sync::Mutex<HashMap<String, RelayDragState>>> =
        std::sync::OnceLock::new();
    ST.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 侵入式映射区自动探测(无显式 `relay_rect` 时的兜底):页面上「fixed/absolute
/// + 面积 ≥140×90 + 含挑战证据(slider/canvas/iframe/验证类文案)」的最高 z-index
/// 最深层容器 → 视口矩形。找不到返回 None(调用方回退全视口映射)。
/// 与 blocker_probe 的挑战弹层打分同构的极简版:只求「滑块/验证弹层在哪」。
pub async fn relay_rect_probe(page: &Page) -> Option<(f64, f64, f64, f64)> {
    use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
    let expr = r#"JSON.stringify((() => {
        try {
            const KW = ['滑块', '拖动', '验证', 'captcha', 'verify', '请按住', 'slide'];
            const vw = window.innerWidth, vh = window.innerHeight;
            let best = null;
            for (const el of document.querySelectorAll('div,section,form,iframe')) {
                const cs = getComputedStyle(el);
                if (cs.display === 'none' || cs.visibility === 'hidden' || parseFloat(cs.opacity) < 0.05) continue;
                if (cs.position !== 'fixed' && cs.position !== 'absolute') continue;
                const r = el.getBoundingClientRect();
                if (r.width < 140 || r.height < 90) continue;
                if (r.width * r.height > vw * vh * 0.98) continue;
                const txt = (el.innerText || '').slice(0, 300);
                const cls = String(el.className || '') + ' ' + (el.id || '');
                const hit = KW.some((k) => (txt + ' ' + cls).indexOf(k) >= 0);
                const hasMedia = !!el.querySelector('canvas, iframe, img');
                if (!(hit || hasMedia)) continue;
                const z = parseInt(cs.zIndex, 10) || 0;
                if (!best || z > best.z || (z === best.z && r.width * r.height > best.a)) {
                    best = {z, a: r.width * r.height, r};
                }
            }
            if (!best) return null;
            return {x: best.r.x, y: best.r.y, width: best.r.width, height: best.r.height};
        } catch (e) { return null; }
    })())"#;
    let params = EvaluateParams::builder()
        .expression(expr)
        .return_by_value(true)
        .build()
        .ok()?;
    let v = page.evaluate(params).await.ok()?;
    let s = v.value().and_then(|x| x.as_str().map(str::to_string))?;
    let p: Value = serde_json::from_str(&s).ok()?;
    let f = |k: &str| p.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let (x, y, w, h) = (f("x"), f("y"), f("width"), f("height"));
    if w > 0.0 && h > 0.0 {
        Some((x, y, w, h))
    } else {
        None
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

fn now_compact() -> String {
    now_millis().to_string()
}

/// page_id:`p_` + 8 位随机 hex(go-web-debug-tool 同款形态,Agent 视为不透明字符串)。
fn new_page_id() -> String {
    let mut b = [0u8; 4];
    rand::Rng::fill(&mut rand::thread_rng(), &mut b);
    format!(
        "p_{}",
        b.iter().map(|x| format!("{x:02x}")).collect::<String>()
    )
}

/// 标签页总量上限(`LAEW_WEB_MAX_TABS`,默认 8)。
///
/// 与 `LAEW_WEB_MAX_WINDOWS`(按去重 windowId 计额,同窗口多 Tab 不占额度)互补:
/// 2026-10-14 闲鱼实测同一商品页被重复打开,单任务残留 12 个 tab 一个上限都没触发。
/// `0`/非法回退默认;上限至少 2(保留工作页 + 一个新页)。
fn max_tabs_from_env() -> usize {
    std::env::var("LAEW_WEB_MAX_TABS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n >= 2)
        .unwrap_or(8)
}

/// 一次标签页收编的结果(2026-10-14 扩展:同 URL 去重 + Tab 上限的对账口径)。
#[derive(Debug, Default)]
pub struct AdoptOutcome {
    /// 收编成功的 page_id(可直接操作)
    pub adopted: Vec<String>,
    /// 与注册表内页**同 URL**而被直接关闭的重复页(重复打开的商品页)
    pub deduped: Vec<String>,
    /// 超出 `LAEW_WEB_MAX_TABS` 被丢弃关闭的候选页 URL
    pub dropped: Vec<String>,
}

impl AdoptOutcome {
    /// 有无任何处置(收编/去重/丢弃),供调用方决定是否回传明细。
    pub fn any(&self) -> bool {
        !self.adopted.is_empty() || !self.deduped.is_empty() || !self.dropped.is_empty()
    }
}

/// Agent 高亮蓝框注入脚本(2026-09-20 第 100 轮):
///
/// headed 可视化模式下,桌面上可能同时存在多个浏览器窗口,人工介入时必须一眼
/// 识别「哪个窗口被 Agent 控制」。本脚本在页面四周渲染一圈 Chrome 品牌蓝
/// (#1a73e8)选中效果:
/// - `pointer-events:none` 不拦截任何鼠标/键盘事件,渲染与普通浏览器一致;
/// - `data-%NS%agent="1"` 标记,MCP_Web_Use 的 elements/dom 提取自动过滤该子树;
/// - 通过 `Page.addScriptToEvaluateOnNewDocument` 挂载,导航/派生新页自动重注入。
///
/// 第 171 轮:右上角原「LAEW Agent 控制中」徽标**已移除** —— 品牌文本改由
/// `browser_status.rs` 的**屏蔽页面状态栏**(右上角)唯一承载,并进一步显示具体
/// Agent 名(`LsmAgentEmergentWork-SubAgent-Work` 等)+ 管控档位 + 注意事项。
/// 同屏出现两处同款品牌角标只会让人工分不清哪个是「身份」、哪个是「说明」,故此处
/// 只留蓝框(窗口归属的纯视觉标识),文字交给状态栏。
pub const AGENT_HIGHLIGHT_JS: &str = r#"(() => {
    // 第 179 轮:窗口面 API 收敛到一个 symbol 命名空间对象,`window` 上零字符串键。
    const NS = window[Symbol.for('%NS%')] || (window[Symbol.for('%NS%')] = {});
    // 第 163 轮:早退门加 DOM 校验 —— 旧写法只在**抛异常之前**置
    // `%NS%AgentHighlightInstalled = true`,而 document-start 时 body/documentElement
    // 可能都 null,ensure() 抛异常后标志却是「已装」,后续任何补挂重试都被这道门挡住,
    // 蓝框与徽标永久缺失。改成「标志已置但元素不在」时允许重新装配,让半成品实例自愈。
    if (window.%NS%AgentHighlightInstalled
        && document.getElementById('%NS%agent_frame__')) return;
    window.%NS%AgentHighlightInstalled = true;
    const ensure = () => {
        // 与 guard 同病:document-start 时两者皆 null,`documentElement` 先存在。
        const root = document.documentElement || document.body;
        if (!root) return;
        let frame = document.getElementById('%NS%agent_frame__');
        if (!frame) {
            frame = document.createElement('div');
            frame.id = '%NS%agent_frame__';
            frame.setAttribute('data-%NS%agent', '1');
            frame.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483647;box-shadow:inset 0 0 0 3px #1a73e8;';
            root.appendChild(frame);
        }
        frame.style.display = window.%NS%AgentHighlight === false ? 'none' : '';
    };
    window.%NS%AgentApplyHighlight = (enabled) => {
        window.%NS%AgentHighlight = !!enabled;
        ensure();
        const f = document.getElementById('%NS%agent_frame__');
        if (f) f.style.display = enabled ? '' : 'none';
        return !!enabled;
    };
    // 第 163 轮:就绪重试 —— 注册在首次 ensure() **之前**,旧代码根本没有补挂,
    // 导航后蓝框永久消失。DOMContentLoaded + load 双保险。
    const boot = () => { try { ensure(); } catch (e) {} };
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', boot, { once: true });
        window.addEventListener('load', boot, { once: true });
    }
    boot();
    // 第 179 轮:命名空间对象内部的键同样不可枚举(防止枚举 symbol 值后一眼看全)。
    try {
        ['AgentHighlightInstalled', 'AgentHighlight', 'AgentApplyHighlight']
            .forEach((k) => {
                const d = Object.getOwnPropertyDescriptor(NS, k);
                if (!d) return;
                Object.defineProperty(NS, k,
                    { enumerable: false, configurable: true, writable: true, value: d.value });
            });
    } catch (e) {}
})()"#;

/// 给页面注入 Agent 高亮蓝框(新文档自动重挂 + 当前文档立即执行)。
/// 第 171 轮:顺带注入屏蔽页面状态栏/进度栏(connect 模式零注入契约不变,由调用方
/// 的 `guard_visuals_active()` 判据把守 —— 本函数只在 headed 非 connect 调用)。
/// 第 180 轮起 `pub(crate)`:派生页早期武装 watcher 的 headed 呈现层对齐复用。
pub(crate) async fn inject_agent_highlight(page: &Page) {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
    let _ = page
        .execute(AddScriptToEvaluateOnNewDocumentParams {
            // 第 168 轮:%NS% 占位符 → 进程级随机命名空间（隐身，见 browser_ns.rs）。
            source: super::browser_ns::render_ns(AGENT_HIGHLIGHT_JS),
            world_name: None,
            include_command_line_api: None,
            // 立即在已存在的执行上下文运行(等效旧 evaluate 路径)
            run_immediately: Some(true),
        })
        .await;
    // 第 171 轮:两条告知栏与蓝框同生命周期(highlight 关闭时见 `set_highlight`)。
    super::browser_status::ensure(page).await;
    super::browser_status::push(page, None, None, None, None).await;
}

/// 未安装浏览器哨兵:BrowserNew 据此返回错误码 3001。
pub const NO_BROWSER_SENTINEL: &str = "NO_BROWSER";

// 第 139 轮:模式决策搬到 `super::browser_mode`,窗口/视口尺寸决策搬到
// `super::browser_viewport`。此处 `pub use` 再导出,保证所有
// `crate::agent::browser::XXX` 外部引用路径零改动。
pub use super::browser_mode::BrowserMode;
pub use super::browser_viewport::{
    default_window_size, fill_ratio, headed_window_plan, viewport_fit_plan, DEFAULT_WINDOW_H,
    DEFAULT_WINDOW_W, HEADED_SCREEN_MARGIN, MIN_VIEWPORT_H, MIN_VIEWPORT_W, VIEWPORT_FIT_MAX_H,
    VIEWPORT_FIT_MAX_W, FILL_RATIO_MIN,
};
pub(crate) use super::browser_viewport::{
    headed_window_metrics, page_viewport_metrics, VIEWPORT_FIT_TOL,
};

/// 按平台优先级探测 Chrome / Edge / Chromium / Brave 可执行文件。
///
/// 顺序:环境变量 `LAEW_BROWSER_PATH` → 平台候选路径 → chromiumoxide 自带检测
/// (CHROME env / PATH / Windows 注册表)。Firefox / Safari 不支持 CDP,不纳入。
pub fn detect_browser() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LAEW_BROWSER_PATH") {
        if Path::new(&p).is_file() {
            return Some(PathBuf::from(p));
        }
    }

    #[cfg(target_os = "macos")]
    let candidates: Vec<PathBuf> = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();

    #[cfg(target_os = "windows")]
    let candidates: Vec<PathBuf> = {
        let mut v = Vec::new();
        for base in [
            std::env::var("PROGRAMFILES").ok(),
            std::env::var("PROGRAMFILES(X86)").ok(),
            std::env::var("LOCALAPPDATA").ok(),
        ]
        .into_iter()
        .flatten()
        {
            let base = PathBuf::from(base);
            v.push(base.join(r"Google\Chrome\Application\chrome.exe"));
            v.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
            v.push(base.join(r"Chromium\Application\chrome.exe"));
            v.push(base.join(r"BraveSoftware\Brave-Browser\Application\brave.exe"));
        }
        v
    };

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let candidates: Vec<PathBuf> = [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/microsoft-edge",
        "/usr/bin/brave-browser",
        "/snap/bin/chromium",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();

    for p in &candidates {
        if p.is_file() {
            return Some(p.clone());
        }
    }

    // fallback:chromiumoxide 自带检测(CHROME env / PATH / 注册表)
    chromiumoxide::detection::default_executable(
        chromiumoxide::detection::DetectionOptions::default(),
    )
    .ok()
    .map(PathBuf::from)
}

/// laew 自定义 Chrome 启动开关集合(**裸名**,不含 `--` 前缀)。
///
/// 关键坑(第 171 轮):chromiumoxide `BrowserConfigBuilder::arg` 走
/// `Arg::from(&str)`,把整串当 key 并在渲染时统一补 `--` 前缀(见其
/// `browser/argument.rs::into_iter`)。因此这里只能传裸名 `no-first-run`;
/// 若传 `--no-first-run` 会被渲染成 `----no-first-run`,Chrome 当未知开关
/// **静默忽略** —— 实测正是如此,导致 `--no-first-run` 失效、每次冷启动都
/// 弹「欢迎使用 Google Chrome」首启对话框,并因缺 `--disable-background-mode`
/// 而在 macOS Dock 常驻图标。见 `tmpPlan/20261014_*`。
pub(crate) fn custom_launch_flags(headless: bool, geo_prompt: bool) -> Vec<String> {
    let mut flags = vec![
        // 首启 / 默认浏览器 / 后台常驻治理(根治欢迎框 + Dock 图标驻留)
        "no-first-run",
        "no-default-browser-check",
        "disable-background-mode",
        "no-service-autorun",
        "disable-component-update",
        "hide-crash-restore-bubble",
        "disable-session-crashed-bubble",
        // 反检测 + 证书 + 后台节流
        "disable-background-networking",
        "disable-background-timer-throttling",
        "disable-backgrounding-occluded-windows",
        "disable-component-extensions-with-background-pages",
        "disable-default-apps",
        "disable-hang-monitor",
        "disable-ipc-flooding-protection",
        "disable-prompt-on-repost",
        "disable-renderer-backgrounding",
        "disable-sync",
        "disable-popup-blocking",
        "metrics-recording-only",
        "ignore-certificate-errors",
        "disable-blink-features=AutomationControlled",
        "disable-features=LocalNetworkAccess,BlockInsecurePrivateNetworkRequests,PrivateNetworkAccessRestrictions,BlockLocalNetworkByDefault,PrivateNetworkAccessRespectPreflightResults,HttpsUpgrades,HttpsFirstBalancedModeAutoEnable,TranslateUI",
    ];
    if !geo_prompt {
        flags.push("deny-permission-prompts");
    }
    if headless {
        // 第 183 轮(真机基线驱动,docs/MCP_Web_Use/37):无头档**不再**强制软渲染 ——
        // new headless 与有头同一渲染路径,`--disable-gpu` 只会亲手制造 WebGL WARP 指纹
        // (实测:带它时 UNMASKED_RENDERER = "ANGLE (Microsoft, Microsoft Basic Render
        // Driver …)",去掉后与本机有头 Chrome **逐字节一致** = 真 Intel UHD 串);
        // `--hide-scrollbars` 同为老式无头遗产,去掉后滚动条宽度跟随宿主 OS 默认
        // (Win11 overlay 恒 0 与有头一致,经典滚动条机器上 ~17px 与该机有头一致)。
        // GPU 初始化失败的机器(VM/RDP/驱动被禁)由 Chrome 原生回退 SwiftShader/WARP,
        // 与旧参数时代读数相同,不比改前差。杀开关 `LAEW_WEB_HEADLESS_SOFTRENDER=on`
        // 恢复旧软渲染参数集(排障对照)。
        flags.push("no-sandbox");
        flags.push("disable-dev-shm-usage");
        if headless_softrender_from_env() {
            flags.extend(["disable-gpu", "hide-scrollbars"]);
        }
    }
    flags.into_iter().map(str::to_string).collect()
}

/// 第 183 轮杀开关:`LAEW_WEB_HEADLESS_SOFTRENDER` 置 on/1/true/yes/软渲染 时,
/// 无头档恢复旧软渲染参数集(`--disable-gpu` + `--hide-scrollbars`)。
/// 供「真 GPU 初始化导致渲染异常」的机器排障对照;缺省关闭(走真值参数集)。
/// `browser_reuse::relaunch_args`(auto_relaunch 调试实例)同策略复用。
pub(crate) fn headless_softrender_from_env() -> bool {
    matches!(
        std::env::var("LAEW_WEB_HEADLESS_SOFTRENDER")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "on" | "1" | "true" | "yes" | "软渲染"
    )
}

/// 第 175 轮:下发一次**同尺寸的 screen 声明**,让页面读到的 `screen.*` 与
/// `window.outer*` 自洽(见 [`BrowserInner::screen_sync`] 的完整背景)。
///
/// 复用已有的 device metrics 覆盖通道:同一条命令补 `screen_width`/`screen_height`,
/// 不新增 CDP 往返。fail-open(覆盖下发失败不阻断导航 —— 只损几何自洽,不损功能)。
///
/// 第 180 轮起 `pub(crate)`:派生页早期武装 watcher(`browser_spawn_arm.rs`)与
/// adopt/heal 收编路径共用(无头档派生页的 `outer > screen` 硬判据修复不再单点)。
pub(crate) async fn apply_screen_sync_override(page: &chromiumoxide::Page, sync: Option<(u32, u32)>) {
    let Some((w, h)) = sync else { return };
    if !super::browser_viewport::screen_sync_needed(w, h) {
        return;
    }
    let params =
        match chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams::builder()
            .width(w as i64)
            .height(h as i64)
            .device_scale_factor(1.0)
            .screen_width(w as i64)
            .screen_height(h as i64)
            .position_x(0)
            .position_y(0)
            .mobile(false)
            .build()
        {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("屏幕几何自洽覆盖参数构造失败(页面将读到 screen=800x600 无头默认值): {e}");
                return;
            }
        };
    // 有界超时:本覆盖只影响几何自洽(纯反检测收益),绝不能拖住首次导航。
    // CDP 无响应时 fail-open,降级为「读到 800x600 无头默认值」的老行为。
    match tokio::time::timeout(std::time::Duration::from_secs(5), page.execute(params)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            tracing::warn!("屏幕几何自洽覆盖下发失败(outer>screen 的物理不可能仍会暴露): {e}");
        }
        Err(_) => {
            tracing::warn!("屏幕几何自洽覆盖超时(5s,fail-open 不阻断导航;outer>screen 仍会暴露)");
        }
    }
}

/// 预置 Chrome profile,跳过首启体验(欢迎对话框 / 后台常驻 / 崩溃恢复气泡)。
///
/// 双保险:即便 `--no-first-run` 因上游参数渲染问题失效,profile 侧的
/// `First Run` 哨兵 + `Local State` 也能让 Chrome 判定「非首启」。
pub(crate) fn seed_chrome_profile(dir: &Path) {
    use std::fs;
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    // 1. First Run 哨兵:存在即非首启。
    let _ = fs::write(dir.join("First Run"), b"");
    // 2. Local State:关闭后台常驻 / 崩溃恢复 / 统计上报。
    let local_state = r#"{
  "background_mode": {"enabled": false},
  "user_experience_metrics": {"stability": {"exited_cleanly": true}, "reporting_enabled": false},
  "profile": {"info_cache": {"Default": {"name": "Profile1"}}}
}"#;
    let _ = fs::write(dir.join("Local State"), local_state);
    // 3. Default/Preferences:跳过欢迎页 / 不设默认浏览器 / 不恢复会话。
    let _ = fs::create_dir_all(dir.join("Default"));
    let prefs = r#"{
  "browser": {"has_seen_welcome_page": true, "should_reset_check_default_browser": false},
  "distribution": {"skip_welcome_page": true, "import_search_engine": false, "make_chrome_default": false},
  "first_run_tabs": [],
  "session": {"restore_on_startup": 4},
  "alternate_error_pages": {"enabled": false},
  "safebrowsing": {"enabled": false}
}"#;
    let _ = fs::write(dir.join("Default/Preferences"), prefs);
}

impl BrowserManager {
    pub fn global() -> Arc<Self> {
        static MANAGER: OnceLock<Arc<BrowserManager>> = OnceLock::new();
        MANAGER
            .get_or_init(|| {
                Arc::new(Self {
                    inner: Mutex::new(BrowserInner {
                        browser: None,
                        handler: None,
                        spawn_arm: None,
                        watchdog: None,
                        connect_mode: false,
                        connected_port: None,
                        browser_ua: None,
                        screen_sync: None,
                        pages: HashMap::new(),
                        user_data_dir: None,
                        mode: BrowserMode::Hidden,
                        highlight: true,
                        stealth: super::stealth::mode(),
                        guard: super::browser_overlay::PageGuardConfig::from_env_default(),
                        // 第 152 轮:默认开启跟随 —— 人工在 locked 档无法滚动,
                        // 页面必须自己停在最新,否则多轮对话只有第一屏可见。
                        auto_follow: true,
                        explicit_window: false,
                        page_visible_box: HashMap::new(),
                        default_size_applied: false,
                    }),
                })
            })
            .clone()
    }

    /// 浏览器实例是否已存在(open 复用判定:「不要重复打开多个浏览器」)。
    pub async fn has_browser(&self) -> bool {
        self.inner.lock().await.browser.is_some()
    }

    /// 当前浏览器实例的启动模式(实例复用时报告真实 mode)。
    pub async fn current_mode(&self) -> Option<BrowserMode> {
        let inner = self.inner.lock().await;
        inner.browser.as_ref().map(|_| inner.mode)
    }

    /// F2(2026-10-16):读取页面「真实可见区」(视口 2K 扩展覆盖前的 inner 尺寸)。
    /// None = 未扩展(告知栏自然锚定);Some = 布局视口被 setDeviceMetricsOverride
    /// 撑大,bottom/right 锚定的注入层须按该可见区重锚。broadcast 逐页取用。
    pub async fn page_visible_box(&self, page_id: &str) -> Option<(f64, f64)> {
        self.inner
            .lock()
            .await
            .page_visible_box
            .get(page_id)
            .copied()
    }

    /// 当前实例是否为 connect 模式(接管外部浏览器,第 142 轮)。
    ///
    /// connect 模式不拥有浏览器进程:close 只断连、不关用户浏览器;工具层据此在
    /// open 响应里回 `connect_mode` 字段供 LLM 对账。
    pub async fn is_connect_mode(&self) -> bool {
        self.inner.lock().await.connect_mode
    }

    /// 新建页面;必要时先启动或接管浏览器。
    ///
    /// 返回 `(page_id, title, final_url)`;未检测到浏览器返回含 NO_BROWSER 哨兵的错误。
    /// 2026-09-17 第 74 轮:`mode` 参数控制是否真正启动浏览器进程;`Hidden` 走纯 CDP
    /// 嵌入式模式,默认无可见窗口,解决"误开 macOS 系统默认浏览器"问题。
    /// 2026-09-20 第 100 轮:`window_size` 支持自定义启动窗口(headed 默认 1920×1080
    /// 1080p);`highlight` 控制 Agent 高亮蓝框注入(headed 可视化标识)。
    /// 2026-10-09 第 141 轮:`overlay` 控制可视化蒙层注入(headed 下人工可看不可点)。
    /// 2026-10-10 第 143 轮:第 7 参升级为 `guard`(页面管控三档:locked 屏蔽缺省 /
    /// open 开放 / partial 部分屏蔽),locked 档与第 141 轮蒙层行为完全一致。
    #[allow(clippy::too_many_arguments)]
    pub async fn new_page(
        &self,
        url: &str,
        mode: BrowserMode,
        connect_url: Option<&str>,
        user_agent: Option<&str>,
        window_size: Option<(u32, u32)>,
        highlight: bool,
        guard: super::browser_overlay::PageGuardConfig,
    ) -> chromiumoxide::error::Result<(String, String, String)> {
        self.new_page_impl(
            url,
            mode,
            connect_url,
            user_agent,
            window_size,
            highlight,
            guard,
            false,
        )
        .await
    }

    /// 多窗口并行(双站同时工作):在**独立新窗口**(而非同窗口新 Tab)打开页面。
    ///
    /// CDP `Target.createTarget(newWindow:true)`,单浏览器进程多窗口 —— 各窗口
    /// 独立前台可见、互不遮挡时页面均不被节流,天猫/京东双站可同时活跃工作。
    /// 仅 headed 有意义(headless shell 不支持 newWindow);connect 模式不支持
    /// (不干预用户浏览器)。其余 launch/注入/清扫逻辑与 [`new_page`] 逐行共用。
    pub async fn new_page_in_new_window(
        &self,
        url: &str,
        mode: BrowserMode,
        user_agent: Option<&str>,
        window_size: Option<(u32, u32)>,
        highlight: bool,
        guard: super::browser_overlay::PageGuardConfig,
    ) -> chromiumoxide::error::Result<(String, String, String)> {
        self.new_page_impl(
            url,
            mode,
            None,
            user_agent,
            window_size,
            highlight,
            guard,
            true,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn new_page_impl(
        &self,
        url: &str,
        mode: BrowserMode,
        connect_url: Option<&str>,
        user_agent: Option<&str>,
        window_size: Option<(u32, u32)>,
        highlight: bool,
        guard: super::browser_overlay::PageGuardConfig,
        new_window: bool,
    ) -> chromiumoxide::error::Result<(String, String, String)> {
        let mut inner = self.inner.lock().await;
        let mut launch_dir: Option<PathBuf> = None;
        let mut launch_watchdog: Option<std::process::Child> = None;
        // 第 175 轮:两档 headless 的屏幕几何自洽尺寸(见 builder 处的注释),
        // 页面创建后据此补发一次带 screen 字段的 device metrics 覆盖。
        let mut headless_screen_sync: Option<(u32, u32)> = None;
        // launch 模式浏览器主进程 PID(第 127 轮):供 cleanup_registry 登记给
        // cleanup_sync 同步清理路径使用;connect 模式保持 0(不拥有外部进程)。
        let mut launch_pid: u32 = 0;
        // 第 170 轮:僵尸句柄活探清尸。浏览器进程死亡( headed 窗口被人工关闭 /
        // 外部因素退出)后 inner.browser 句柄不会被任何路径主动清理,此前
        // new_page_impl 的 `if inner.browser.is_none()` 唯一入口判定恒为假 ——
        // ① connect_url 被静默吞掉(auto_relaunch 拉起的调试 Chrome 成孤儿窗口,
        //    用户桌面多出 about:blank Chrome,实测猎聘任务);
        // ② 后续 open 的 goto 全打到僵尸句柄上,chromiumoxide handler 已终止,
        //    必报 "receiver is gone"(2001)且重试无用(实测连烧 3 次)。
        // 这里在入口做廉价活探(3s 超时,存活时通常 <100ms),死了就按
        // close_page 末页回收同语义清尸,让本次调用走全新 launch/connect。
        if inner.browser.is_some() {
            let dead = {
                let browser = inner.browser.as_ref().expect("checked is_some");
                !Self::browser_handle_alive(browser).await
            };
            if dead {
                tracing::warn!("检测到已死亡的浏览器句柄(CDP 无响应),清理后重新拉起");
                if let Some(mut browser) = inner.browser.take() {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        browser.close(),
                    )
                    .await;
                }
                if let Some(handler) = inner.handler.take() {
                    handler.abort();
                }
                if let Some(watcher) = inner.spawn_arm.take() {
                    watcher.abort();
                }
                if let Some(child) = inner.watchdog.take() {
                    spawn_watchdog_reaper(child);
                }
                if !inner.connect_mode {
                    cleanup_registry::clear();
                    if let Some(dir) = inner.user_data_dir.take() {
                        spawn_tempdir_cleanup(dir);
                    }
                }
                inner.pages.clear();
                inner.connect_mode = false;
                inner.connected_port = None;
            }
        }
        // 第 159 轮:本次调用是否首次拉起浏览器 —— 决定注册完本页后是否清扫
        // Chrome 启动自带的初始空标签页(launch 不带初始 URL,Chrome 必开一个
        // about:blank / new-tab-page,实测「先弹空页签再弹目标页签」即它)。
        let just_launched = inner.browser.is_none();

        // Browser 操作超时(2026-09-16 第 66 轮):launch/connect/goto 统一 30s,
        // 防止页面挂起/Chrome 启动失败导致无限等待(用户反馈浏览器任务卡住 58.8s)。
        let browser_timeout = std::time::Duration::from_secs(30);

        // 第 172 轮:死句柄预防(实测 2026-10-14 BOSS 直聘对照任务)。
        //
        // 事故:用户日常 Chrome 占用 9222,HTTP `/json/version` 探得到版本号,但 CDP
        // WebSocket 已无响应;`Browser::connect` 把这个「探得到但已死」的实例写进
        // 全局单例后,handler 任务立即退出 —— 之后**连纯冷启动的 `open` 也一律返回
        // futures 的 `oneshot canceled`**,模型拿到一个不可理解的英文错误,只能反复
        // 重试(实测连续 3 次全灭)。
        //
        // 对策:每次要新开页之前,先对已存在的句柄做活探;死了就地清尸,让本次调用
        // 走真正的 launch/connect 路径。判据与 `browser_handle_alive` 一致
        // (`browser.pages()` 往返失败 = CDP 连接已死),避免两套活探语义分叉。
        if inner.browser.is_some() {
            let alive = {
                let browser = inner.browser.as_ref().expect("is_some");
                Self::browser_handle_alive(browser).await
            };
            if !alive {
                tracing::warn!(
                    "MCP_Web_Use 检测到已注册的浏览器句柄 CDP 无响应,就地清尸后重新拉起(第 172 轮)"
                );
                Self::purge_browser(&mut inner).await;
            }
        }

        if inner.browser.is_none() {
            // 上一版进程内清理无法覆盖 kill -9；先做一次性迁移清扫，再交给新 watchdog。
            if connect_url.is_none() {
                #[cfg(unix)]
                cleanup_legacy_orphans();
                cleanup_stale_profiles();
            }
            let (browser, handler, connect_mode) = if let Some(connect_url) = connect_url {
                let (browser, mut handler) = tokio::time::timeout(
                    browser_timeout,
                    Browser::connect(connect_url.to_string()),
                )
                .await
                .map_err(|_| {
                    chromiumoxide::error::CdpError::msg(format!(
                        "Browser connect 超时({}s),请检查 connect_url 是否正确",
                        browser_timeout.as_secs()
                    ))
                })??;
                let task = tokio::spawn(async move {
                    // 必须持续驱动 handler,否则 CDP 连接挂起
                    while let Some(msg) = handler.next().await {
                        if msg.is_err() {
                            break;
                        }
                    }
                });
                // 记录对端调试端口(laew 身份判定,见 BrowserInner 字段注释)。
                // 取最后一个 `:` 后的段(兼容 `[::1]:9222` 形式;rsplit 不会被
                // scheme 的 `http:` 干扰)。此处已持有 inner 锁,直接赋值。
                inner.connected_port = connect_url
                    .rsplit_once(':')
                    .and_then(|(_, p)| p.trim_end_matches('/').parse::<u16>().ok());
                (browser, task, true)
            } else {
                let exe = detect_browser().ok_or_else(|| {
                    chromiumoxide::error::CdpError::msg(format!(
                        "{NO_BROWSER_SENTINEL}: 未检测到 Chrome/Edge/Chromium 浏览器。\
                         请安装 Google Chrome(https://www.google.cn/chrome/)或 Microsoft Edge, \
                         或设置环境变量 LAEW_BROWSER_PATH 指向浏览器可执行文件"
                    ))
                })?;
                let builder = BrowserConfig::builder().chrome_executable(exe);
                // 第 159 轮:全模式统一 disable_default_args + 自选参数集。
                // chromiumoxide 的 DEFAULT_ARGS(puppeteer 同源)携带 `--enable-automation`,
                // navigator.webdriver=true 会被 WAF 反爬(内网站点 vcm.example.com:8443 实测
                // 瑞数形态 cookie 已种上但文档恒空:head:""/scripts:0/readyState:complete)
                // 直接判为机器人返回空 HTML —— 剔除该旗标并叠加反检测参数。
                // 同时固化三类实测刚需:
                // ① `--ignore-certificate-errors`:自签名证书/自定义域名内网网关直达,
                //    消除证书 interstitial 类整页不可用(与 LAEW_TLS_INSECURE 的 HTTP 客户端
                //    自动策略对齐,浏览器侧同等宽松);
                // ② `--disable-features` 一并禁 Local Network Access / Private Network
                //    Access 系列(Chrome 154 LNA 对「公网域名→私网 IP」子资源按
                //    PermissionBlock 静默拦截,WAF 挑战脚本跑不完 → 永远空白页)与
                //    HttpsUpgrades(自定义端口 https 站点不被静默降级/升级试探);
                // ③ `--disable-blink-features=AutomationControlled`:压掉 webdriver 指纹。
                let mut builder = builder.disable_default_args();
                // 第 171 轮(chromiumoxide 参数双横杠坑):`.arg()` 走 `Arg::from`,
                // 整串当 key 并统一补 `--`,故这里传**裸名**;详见 `custom_launch_flags`。
                let headless = matches!(mode, BrowserMode::Hidden | BrowserMode::NewHeadless);
                let geo_prompt = browser_perm::geo_mode_from_env() == GeoMode::Prompt;
                for flag in custom_launch_flags(headless, geo_prompt) {
                    builder = builder.arg(flag);
                }
                // 2026-09-17 第 74 轮:三档 mode 控制浏览器启动形态。
                // 默认 Hidden(纯 CDP headless=new,系统级无窗口),
                // 解决"误开 macOS 系统默认浏览器"问题。
                match mode {
                    BrowserMode::Hidden | BrowserMode::NewHeadless => {
                        builder = builder.new_headless_mode();
                    }
                    BrowserMode::Headed => {
                        builder = builder.with_head();
                    }
                }
                // 窗口尺寸(第 125 轮):显式参数优先;缺省**全模式 1920×1080(1080p)**
                // ——hidden 原 1440×900 视口过窄,现代 Web 应用(min-width>1440 的管理后台/
                // SaaS)横向被裁导致元素不可见、不可点,截图也显示不全。
                let (win_w, win_h) = window_size.unwrap_or_else(default_window_size);
                builder = builder.window_size(win_w, win_h);
                // 第 161 轮:视口策略根治 —— chromiumoxide BrowserConfig 默认
                // `viewport: Some(Viewport::default()=800×600)`,每个 page target
                // 初始化都会发 `Emulation.setDeviceMetricsOverride(800,600)` 把页面
                // 视口钉死;`hide()`(老式 headless)会清掉默认视口,但我们用的
                // `with_head()`/`new_headless_mode()` 不清 —— 实测内网站点任务 headed
                // 窗口外框 1920×1080、页面只渲染左上 800×600,右侧/下方大片空白
                // (用户看到「白屏且显示区域没有铺满浏览器」);第 156 轮「macOS 上
                // inner 恒 800×600 是布局时序」实为该覆盖钉死(Windows 同样中招),
                // `chrome_height_raw=480` 失真同源;hidden 同被钉死,仅因内容超视口
                // 时 fit_viewport_to_content 自动扩展而被掩盖。策略:headed 禁用视口
                // 仿真(Playwright `viewport:null` 同款,页面视口=窗口内容区,天然
                // 铺满,人工拖动窗口后自适应);两档 headless 显式 viewport=窗口尺寸
                // (headless 窗口即视口,确定性 1920×1080)。connect 模式不受影响
                // (`Browser::connect` 走 HandlerConfig::default(),viewport 本就 None)。
                builder = match mode {
                    BrowserMode::Headed => builder
                        .viewport(None::<chromiumoxide::handler::viewport::Viewport>),
                    BrowserMode::Hidden | BrowserMode::NewHeadless => {
                        builder.viewport(chromiumoxide::handler::viewport::Viewport {
                            width: win_w,
                            height: win_h,
                            device_scale_factor: None,
                            emulating_mobile: false,
                            is_landscape: false,
                            has_touch: false,
                        })
                    }
                };
                // 第 175 轮:无头档**屏幕几何自洽**。chromiumoxide 0.9 的
                // `emulation.rs::init_commands` 只发 width/height,**从不发
                // screen_width/screen_height**(源码已核)→ 页面读到
                // `screen.width = 800x600`(Chrome 无头默认值)而
                // `window.outerWidth = 1920` —— 窗口比屏幕还大,**物理不可能**,
                // 是零成本零误伤的风控硬判据(`verify.js` 实测:`outer_gt_screen`
                // 由 true 变 false)。headed 档无此问题(不覆盖视口,页面视口=窗口
                // 内容区),故只对两档 headless 在此补记屏幕尺寸,待页面创建后
                // 由 `apply_screen_sync_override` 下发。
                headless_screen_sync = match mode {
                    BrowserMode::Hidden | BrowserMode::NewHeadless
                        if super::browser_viewport::screen_sync_needed(win_w, win_h) =>
                    {
                        Some((win_w, win_h))
                    }
                    _ => None,
                };
                // 一次性 user-data-dir:避免 chromiumoxide 默认固定目录的 SingletonLock 冲突
                // (并行/上次异常退出后残留锁会导致 Chrome 拒启),同时与用户日常 profile 隔离。
                let dir = std::env::temp_dir().join(format!("laew_browser_{}", {
                    let mut b = [0u8; 4];
                    rand::Rng::fill(&mut rand::thread_rng(), &mut b);
                    b.iter().map(|x| format!("{x:02x}")).collect::<String>()
                }));
                builder = builder.user_data_dir(&dir);
                // 第 171 轮:预置 profile 跳过首启体验(欢迎框 / 后台常驻 / 崩溃恢复),
                // 与 `--no-first-run` 双保险。
                seed_chrome_profile(&dir);
                let config = builder
                    .build()
                    .map_err(chromiumoxide::error::CdpError::msg)?;
                launch_dir = Some(dir.clone());
                let (mut browser, mut handler) = tokio::time::timeout(
                    browser_timeout,
                    Browser::launch(config),
                )
                .await
                .map_err(|_| {
                    chromiumoxide::error::CdpError::msg(format!(
                        "Browser launch 超时({}s),请检查 Chrome 是否可正常启动",
                        browser_timeout.as_secs()
                    ))
                })??;
                write_profile_owner(&dir);
                let browser_pid = browser
                    .get_mut_child()
                    .and_then(|child| child.as_mut_inner().id());
                launch_pid = browser_pid.unwrap_or(0);
                launch_watchdog = browser_pid.and_then(|pid| spawn_parent_watchdog(pid, &dir));
                let task = tokio::spawn(async move {
                    while let Some(msg) = handler.next().await {
                        if msg.is_err() {
                            break;
                        }
                    }
                });
                (browser, task, false)
            };
            inner.browser = Some(browser);
            inner.handler = Some(handler);
            inner.watchdog = launch_watchdog;
            inner.connect_mode = connect_mode;
            // 第 175 轮:屏幕几何自洽尺寸随实例记账(两档 headless 才有值)。
            inner.screen_sync = headless_screen_sync;
            inner.user_data_dir = launch_dir.clone();
            // 第 142 轮(复用已登录浏览器):connect 接管用户已启动的浏览器,请求
            // mode 只影响注入判断 —— 按有头处理;管控强制 open(不锁用户自己
            // 正在使用的浏览器输入、不向用户页面注入任何视觉,需要时
            // control(set_guard, mode="locked") 手动开)。
            inner.mode = if connect_mode {
                BrowserMode::Headed
            } else {
                mode
            };
            inner.highlight = highlight;
            // 第 156 轮:记录窗口尺寸是否由调用方显式指定 —— 决定 headed 收边
            // 是「按屏幕铺满」还是「保持请求尺寸只做不溢出收边」。
            // connect 模式恒 false(且 fit_headed_window 对 connect 整路跳过)。
            inner.explicit_window = !connect_mode && window_size.is_some();
            // F2/F3(2026-10-16):新实例重置 —— 可见区记账清空(旧页面已不复存在),
            // 1080p 一次性校正对新实例重新生效。
            inner.page_visible_box.clear();
            inner.default_size_applied = false;
            // 第 143 轮:页面管控期望态(connect 模式强制 open;hidden 下无意义,
            // 注入由下方 headed && 非 connect 门控跳过)。
            inner.guard = if connect_mode {
                super::browser_overlay::PageGuardConfig::open()
            } else {
                guard
            };
            // 第 78 轮:标记浏览器已启动,供 cleanup_sync() 快速判断避免无意义创建 Runtime。
            browser_started_flag::set_started();
            // 第 127 轮:同步清理登记(仅 launch 模式 —— 拥有浏览器进程所有权才登记;
            // connect 模式接管的外部浏览器与一次性 profile 都不属于本进程)。
            if !connect_mode {
                if let Some(dir) = launch_dir {
                    cleanup_registry::register(launch_pid, dir);
                }
            }
            // 第 180 轮:浏览器级 UA 探测(取值源:派生页早期武装的无探针 UA 去令牌
            // 通道)。第 185 轮(doc 39)实测发现本机 Chrome 155 new headless 的
            // getVersion 产品名存在**启动期切换**:launch 后立即读恒为
            // `HeadlessChrome/155.0.0.0`(裸 repro 12/12),安静环境 ~1s 后切换为
            // 常规 `Chrome/…`(干净),而 Chrome 启停背靠背(测试连跑/慢机)时切换
            // 可被拖得很久;切换前发出的主文档请求一律带裸令牌(36 §五遗留
            // 「毫秒级竞态」的真身)。故启动后**轮询 getVersion 直至令牌消失再放行
            // 后续导航**:切换后 UA 本就干净,去令牌各通道零动作;预算耗尽仍带令牌
            // 才交给首航剥离覆盖路径兜底(该路径在启停竞争下仍偶发被网络层缺席,
            // 是已记录残差)。总预算 ~11s(90 次 × 1s 有界 + 100ms 间隔),常态 <1s。
            inner.browser_ua = None;
            for attempt in 1..=90 {
                let got = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    inner.browser
                        .as_ref()
                        .expect("browser initialized")
                        .user_agent(),
                )
                .await;
                let mut masked = false;
                match got {
                    Ok(Ok(ua)) => {
                        masked = !super::stealth::is_headless_ua(&ua);
                        inner.browser_ua = Some(ua);
                    }
                    otherwise => {
                        tracing::debug!(?attempt, ?otherwise, "浏览器级 UA 探测失败(重试)");
                    }
                }
                if masked || attempt == 90 {
                    if !masked {
                        tracing::warn!(
                            "UA 启动掩码结算超时(预算内 getVersion 仍带无头令牌),                             交给首航剥离覆盖路径兜底"
                        );
                    }
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            // 第 180 轮:派生页早期武装 watcher(仅 launch 模式 + 开关开启)。
            // 订阅浏览器级 `Target.targetCreated`,页面派生的新标签页出生即下发
            // 隐身/屏幕几何/UA 处置 —— 此前要等 adopt(触发动作收尾)才武装,无头档
            // 派生页整个首生命周期带着 `outer > screen` 硬判据(screen_sync 从未进
            // adopt 序列)。watcher 只持有事件流(不持有 Browser 句柄,其非 Clone),
            // 页面句柄经 `spawn_arm_resolve` 有界解析;与 handler 同点回收。
            // 杀开关 `LAEW_WEB_SPAWN_ARM=off` 回退旧行为。事件流注册有界 2s,
            // 失败仅告警(fail-open,回退 adopt 时武装)。
            if !connect_mode && super::browser_spawn_arm::enabled() {
                let stream_res = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    inner.browser
                        .as_ref()
                        .expect("browser initialized")
                        .event_listener::<chromiumoxide::cdp::browser_protocol::target::EventTargetCreated>(),
                )
                .await;
                match stream_res {
                    Ok(Ok(stream)) => {
                        inner.spawn_arm =
                            Some(tokio::spawn(super::browser_spawn_arm::watch(stream)));
                    }
                    otherwise => {
                        tracing::warn!(
                            ?otherwise,
                            "派生页早期武装:注册 targetCreated 事件流失败(回退 adopt 时武装)"
                        );
                    }
                }
            }
        }

        let browser = inner.browser.as_ref().expect("browser initialized");
        // 多窗口并行:new_window=true 走 CDP `Target.createTarget(newWindow:true)`
        // 起独立可见窗口(单浏览器进程多窗口,profile/登录态/HITL 全复用);
        // false 保持同窗口新 Tab 形态。
        // 2026-10-16:new_page 有界化 —— 它排队在 browser handler 事件循环上,
        // 前一 target 导航卡死时(实测 github.com/commits 网络 stall)会无声挂死,
        // 与下方 goto 的 30s 超时同属一级兜底。
        //
        // 第 178 轮:建页一律用 **about:blank**,真实 URL 只由下方 goto 加载一次。
        // 此前把 url 交给 createTarget,而 chromiumoxide 的 create_page 会等到该 URL
        // **加载完成**才返回 —— 于是首个真实文档发生在隐身/UA/屏幕几何处置**之前**
        // (真机计数服务器实测:目标 URL 被请求 2 次,req#1 的 UA 原样带 `HeadlessChrome`;
        // 站点风控 SDK 恰在文档最早期采指纹并上报,第 177 轮「注册早于 goto」对它无效),
        // 且同一新会话对同一 URL 连续加载两次本身就是异常导航序列。about:blank 无站点
        // 脚本、无网络请求,是纯粹的空载起点。`about:blank` 非安全上下文 ⇒ 读不到
        // Client Hints —— 无妨:真机实测无头档 UA 令牌只出现在 **UA 字符串**里
        // (`Sec-CH-UA` 品牌表恒为正常品牌),而 `navigator.userAgent` 在 about:blank 上可读。
        let page = if new_window {
            use chromiumoxide::cdp::browser_protocol::target::CreateTargetParams;
            let params = CreateTargetParams::builder()
                .url("about:blank".to_string())
                .new_window(true)
                .build()
                .map_err(chromiumoxide::error::CdpError::msg)?;
            tokio::time::timeout(browser_timeout, browser.new_page(params))
                .await
                .map_err(|_| {
                    chromiumoxide::error::CdpError::msg(format!(
                        "browser.new_page 超时({}s):浏览器 handler 可能已卡死(某页面导航挂死会连坐新建),\
                         建议先 MCP_Web_Use close(page_id=\"all\") 回收浏览器后重试",
                        browser_timeout.as_secs()
                    ))
                })??
        } else {
            tokio::time::timeout(browser_timeout, browser.new_page("about:blank"))
                .await
                .map_err(|_| {
                    chromiumoxide::error::CdpError::msg(format!(
                        "browser.new_page 超时({}s):浏览器 handler 可能已卡死(某页面导航挂死会连坐新建),\
                         建议先 MCP_Web_Use close(page_id=\"all\") 回收浏览器后重试",
                        browser_timeout.as_secs()
                    ))
                })??
        };
        // 第 159 轮:先开域挂监听、再 goto —— 主文档的请求/响应/失败才会进
        // Network 环形缓冲。旧顺序 goto 完成才 enable,首个文档的加载证据
        // (状态码/blocked_reason)全部丢失,空白页只能靠猜(内网站点实测 network
        // 事件里只有 favicon,主文档完全不可见)。
        let events = EventBuffer::default();
        // 2026-10-16:两个 Enable 同样有界化(排队在 handler 上,卡死时无声挂起)。
        let _ = super::browser_cdp::cdp_with_timeout(
            "open.runtime_enable",
            page.execute(chromiumoxide::cdp::js_protocol::runtime::EnableParams::default()),
        )
        .await;
        let _ = super::browser_cdp::cdp_with_timeout(
            "open.network_enable",
            page.execute(chromiumoxide::cdp::browser_protocol::network::EnableParams::default()),
        )
        .await;
        spawn_event_listeners(&page, events.clone()).await;
        // 第 174 轮:无头档 UA 清理必须**先于首次导航** —— 否则第一个文档的网络
        // 请求头仍带 `HeadlessChrome` 令牌(JS 侧改 navigator.userAgent 只骗得过
        // 页面、骗不过服务器)。connect 模式零注入契约不变(用户日常浏览器本无该令牌)。
        //
        // 第 178 轮:此刻页面是 about:blank(非安全上下文),`navigator.userAgentData`
        // 不存在 ⇒ 只能清 UA 字符串(`UaSweep::StringOnly`)。真机实测:
        // `setUserAgentOverride` 不带 `userAgentMetadata` 时页面侧的
        // `userAgentData.brands` 会被**清空** —— 「UA 说 Chrome、Client Hints 说空」
        // 是比原状更显眼的新破绽。故落地后（安全上下文）必须补全,见下方 goto 之后。
        // 第 185 轮:首航前的 UA 清理改走**无探针优先通道**(`apply_ua_destealth_first_nav`,
        // docs/MCP_Web_Use/39) —— 用实例建立时 `Browser.getVersion` 缓存的浏览器级 UA
        // 预剥离令牌并在 goto 前同步 await 完成,确定性先于 req#1 的网络头。旧探针式
        // 清理的 `Runtime.evaluate` 排在 handler 事件循环上,真 GPU 冷启动变慢后实测
        // 可拖到超时 → Skipped → goto 带着 HeadlessChrome 令牌出去(36 §五遗留的偶发
        // 竞态);无缓存(getVersion 也失败)才退回探针式兜底。connect 模式零注入契约不变。
        let ua_sweep = if inner.connect_mode {
            super::browser_ns::UaSweep::Skipped
        } else {
            // 第 185 轮:launch 时的 getVersion 探测只有 2s 单次预算,并行拉起多台
            // Chrome 的负载下实测可超时留空(现场日志:ua_sweep=Skipped 且 headless=true
            // → req#1 裸奔)。此处现场补探:不依赖页面上下文,3 次 × 3s 预算,取到即
            // 回写缓存 —— 首航去令牌的主通道(无探针剥离)必须有浏览器级 UA 可用。
            if inner.browser_ua.is_none() {
                let browser = inner.browser.as_ref().expect("browser initialized");
                for attempt in 1..=3 {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        browser.user_agent(),
                    )
                    .await
                    {
                        Ok(Ok(ua)) => {
                            tracing::info!(attempt, "浏览器级 UA 现场补探成功(首航去令牌通道恢复)");
                            inner.browser_ua = Some(ua);
                            break;
                        }
                        otherwise => {
                            tracing::warn!(?attempt, ?otherwise, "浏览器级 UA 现场补探失败");
                        }
                    }
                }
            }
            super::browser_ns::apply_ua_destealth_first_nav(
                &page,
                inner.stealth,
                inner.browser_ua.as_deref(),
            )
            .await
        };
        // 第 185 轮:**网络侧结算导航**(docs/MCP_Web_Use/39 §3)。真机并行压测实证:
        // `setUserAgentOverride` 的 ack 只保证浏览器进程已**受理**,网络服务管道内
        // 落位是异步的 —— ack 后立刻 goto,req#1 偶发仍带 `HeadlessChrome` 令牌,
        // 而同刻页面侧 `navigator.userAgent` 读数已干净(渲染/网络两条管道独立,
        // 「页面可见结算闸」观测不到网络侧滞后,故两者并用)。处置:带令牌无头实例
        // 首航前,先做一次**必定失败的内部结算跳转**(`127.0.0.1:1` 无监听,连接即拒,
        // <100ms,对目标站点完全不可见)—— 请求构造走同一 NetworkContext 管道,
        // 按 FIFO 强制其消化覆盖状态;跳转完成 = 覆盖已落位,真实 goto 的 req#1
        // 确定性干净。有头档无令牌、connect 零注入契约,均零动作。
        let headless_instance =
            matches!(inner.mode, BrowserMode::Hidden | BrowserMode::NewHeadless);
        if ua_sweep != super::browser_ns::UaSweep::Skipped
            && headless_instance
            && !inner.connect_mode
        {
            let settle = chromiumoxide::cdp::browser_protocol::page::NavigateParams::builder()
                .url("http://127.0.0.1:1/".to_string())
                .build();
            if let Ok(p) = settle {
                // 拒连/超时就地吞掉 —— 结算跳转的成败无关紧要,只取它的管道冲刷语义。
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    page.execute(p),
                )
                .await;
            }
        }
        // 第 175 轮:屏幕几何自洽 —— 与 UA 清理同批(都在首次导航前下发),
        // 让首个文档起就满足 `outer <= screen`(真机实测:不补则
        // screen=800x600 < outer=1920,物理不可能,风控一行即可判定)。
        apply_screen_sync_override(&page, inner.screen_sync).await;
        // 第 177 轮:隐身脚本必须在**导航之前**注册。`addScriptToEvaluateOnNewDocument`
        // 只有在 goto 之前下发才能拿到 document-start 执行序;原先它排在 goto 之后,
        // 靠 `run_immediately` 补救 —— 那只对**当前文档**生效,首个文档的页面脚本早已在
        // document-start 跑完,读到的是未处置的 `navigator.webdriver=true` 与空 plugins,
        // 并已把风控信标发出去(风险 SDK 一律在文档最早期采指纹)。移到这里后首个文档
        // 也从一开始就是处置过的环境。connect 模式零注入契约不变。
        if !inner.connect_mode {
            super::browser_ns::inject_stealth(&page, inner.stealth).await;
            // 第 182 轮:无头档指针/悬停媒体特性 + 触摸点**原生 CDP 仿真**与隐身
            // 脚本同批下发(首个文档起即处置过的环境;有头档零动作,connect 零注入
            // 契约不变)。真机实测四元媒体查询与 maxTouchPoints 与有头档逐一对齐。
            super::browser_env_emulation::apply_pointer_env_emulation(&page, inner.stealth, inner.mode).await;
        }
        tokio::time::timeout(browser_timeout, page.goto(url.to_string()))
            .await
            .map_err(|_| {
                chromiumoxide::error::CdpError::msg(format!(
                    "page.goto({url}) 超时({}s),请检查网络连接或稍后重试",
                    browser_timeout.as_secs()
                ))
            })??;

        // 第 178 轮:UA 去令牌的**落地后补全**。首次导航前那次探测发生在
        // `about:blank`(非安全上下文,读不到 Client Hints),只清出了 UA 字符串;
        // 而 `setUserAgentOverride` 不带 `userAgentMetadata` 会把页面侧的
        // `navigator.userAgentData.brands` 清空(真机实测) —— 那等于用「UA 说 Chrome、
        // Client Hints 说空」换掉了原来的令牌,得不偿失。现在文档已落在真实安全上下文,
        // 先撤掉半吊子覆盖让页面重新报出**原生**身份,再探一次即拿到品牌表真值并下发
        // 完整覆盖。窗口极短(页面脚本早已在 document-start 读到干净的 UA 字符串)。
        // 有头档此路径不可达(pre-goto 那次本就 `Skipped`,无令牌)。
        //
        // 第 183 轮:**Skipped 兜底**(docs/MCP_Web_Use/37 §3.4)。首航前探针若在
        // about:blank 上超时/求值失败,`apply_ua_destealth` 返回 `Skipped` 且此前无人
        // 再补 —— goto 就带着 `HeadlessChrome` 令牌出去了。第 185 轮(doc 39)首航前
        // 主通道已改无探针化(getVersion 缓存 UA 预剥离),本兜底现只覆盖「无缓存 +
        // 探针也失败」的窄路径;无头档是唯一有令牌的档,落地后重探一次并下发完整
        // 覆盖,页面侧与后续请求全部干净。有头档本就无令牌、connect 模式零注入契约,
        // 均零动作。
        if ua_sweep == super::browser_ns::UaSweep::StringOnly
            || (ua_sweep == super::browser_ns::UaSweep::Skipped
                && headless_instance
                && !inner.connect_mode)
        {
            super::browser_ns::restore_native_ua(&page).await;
            let done = super::browser_ns::apply_ua_destealth(&page, inner.stealth).await;
            if done != super::browser_ns::UaSweep::Full {
                tracing::warn!(
                    "UA 去令牌落地后补全未生效(Client Hints 品牌表可能为空,与 UA 字符串不自洽): {done:?}"
                );
            }
            // 第 185 轮:落地后这次覆盖同样过结算闸 —— 渲染进程消费仿真状态滞后于
            // ack,页面侧读数干净才说明后续页面自发请求(信标/XHR)必带干净 UA。
            if done != super::browser_ns::UaSweep::Skipped {
                super::browser_ns::await_ua_override_visible(&page).await;
            }
        }

        // UA 覆盖(可选)
        if let Some(ua) = user_agent.filter(|s| !s.is_empty()) {
            let params = chromiumoxide::cdp::browser_protocol::emulation::SetUserAgentOverrideParams::builder()
                .user_agent(ua)
                .build()
                .map_err(chromiumoxide::error::CdpError::msg)?;
            let _ = page.execute(params).await;
        }

        // 第 169/174/177 轮:隐身覆写已在 goto **之前**注册(见上),此处不再重复注入 ——
        // 重复注册会把同一份脚本再挂一遍(靠脚本内按块位掩码的幂等闸兜底,但没有意义)。
        // 第 100 轮:headed 可视化模式注入 Agent 高亮蓝框(hidden 无窗口,注入无意义
        // 且污染 DOM 提取,跳过)。注意:浏览器已存在时以实例真实 mode 为准
        // (inner.mode),而非本次 open 请求的 mode —— 单实例复用,不新启浏览器。
        let want_highlight =
            inner.highlight && matches!(inner.mode, BrowserMode::Headed);
        if want_highlight {
            inject_agent_highlight(&page).await;
        }
        // 第 143 轮:页面管控(headed && 非 connect 才注入;三档全注入 —— open 档
        // 也要有「页面开放·人工可直接操作」状态条,人工明确知道自己可以上手)。
        // 注意与高亮相互独立:highlight=false 也能有管控,guard=open 也能带蓝框。
        if !inner.connect_mode && matches!(inner.mode, BrowserMode::Headed) {
            let cfg = inner.guard.clone();
            super::browser_overlay::inject_agent_guard(&page, &cfg).await;
            let _ = super::browser_overlay::apply_page_guard(&page, &cfg).await;
        }

        let mut title = super::browser_cdp::cdp_with_timeout("open.get_title", page.get_title())
            .await
            .flatten()
            .unwrap_or_default();
        let mut final_url = super::browser_cdp::cdp_with_timeout("open.url", page.url())
            .await
            .flatten()
            .unwrap_or_else(|| url.to_string());
        // 第 170 轮:首航空白自愈 —— 猎聘 WAF 首击实测 3/3:新标签页**首次**导航
        // 不提交(落 about:blank 空文档),同页**第二次**导航即成功(此前只能靠
        // LLM 自觉补 control(navigate),Plan/执行层每次都踩中,每轮白烧 2~4 次
        // 迭代)。这里原地重导航一次(复用同页句柄,不新开页/不重复注入),
        // 仍 blank 才按原路径返回 page_health 诊断。launch/connect/new_window
        // 各路径统一受益;connect 模式同样做 —— open 语义本就是「此页导航到
        // url」,同 URL 原地刷新一次无害。
        // 2026-10-14 第三轮:已证实的抹除域(streak≥1/冷却中)跳过自愈 —— 重导航
        // 只会再吃一次风控请求、多闪一次窗口;自愈是给瞬态 WAF 首击的。
        let wipe_flagged = crate::agent::browser_pace::host_of(url)
            .map(|h| crate::agent::browser_pace::WebPace::global().wipe_flagged(&h))
            .unwrap_or(false);
        if final_url == "about:blank" && !wipe_flagged {
            tracing::warn!(
                url,
                "open 首航落 about:blank(WAF 首击/导航竞态),原地重导航一次"
            );
            let retry = tokio::time::timeout(browser_timeout, page.goto(url.to_string())).await;
            if let Ok(Ok(_)) = retry {
                title =
                    super::browser_cdp::cdp_with_timeout("open.retry.get_title", page.get_title())
                        .await
                        .flatten()
                        .unwrap_or_default();
                final_url =
                    super::browser_cdp::cdp_with_timeout("open.retry.url", page.url())
                        .await
                        .flatten()
                        .unwrap_or(final_url);
            }
        }
        let id = new_page_id();
        let target_id = page.target_id().clone().into();
        inner.pages.insert(
            id.clone(),
            PageEntry {
                page,
                target_id,
                created_at: now_compact(),
                events,
            },
        );
        // 第 159 轮:首次 launch 后清扫浏览器自带的初始空标签页 —— launch 不带
        // 初始 URL,Chrome 必开一个 about:blank / chrome://new-tab-page;不关掉的话
        // ① 用户看到「先空页签后目标页签」两个页签;② 残留页后续会被
        // adopt_spawned_pages 误认领(reload 触发巡检时被收编成伪 spawned_page_id,
        // 实测 142751 日志 line 123/131)。connect 模式绝不碰用户自己的标签页。
        //
        // 判定用 is_noise_url(**不看标题**):刚 launch 的浏览器归 laew 全权所有,
        // 注册表之外只可能是 Chrome 启动残留;初始标签页加载早期 URL 可能还是
        // about:blank、标题已非空,走 is_noise_page 的标题豁免会漏杀(实测首启
        // 双页签正是它)。真实站点 URL 永远不会命中 is_noise_url,无误伤面。
        if just_launched && !inner.connect_mode {
            if let Some(browser) = inner.browser.as_ref() {
                if let Ok(pages) = browser.pages().await {
                    let pages_len = pages.len();
                    let known: std::collections::HashSet<String> =
                        inner.pages.values().map(|e| e.target_id.clone()).collect();
                    let mut closed = 0usize;
                    for p in pages {
                        let tid: String = p.target_id().clone().into();
                        if known.contains(&tid) {
                            continue;
                        }
                        let purl = p.url().await.ok().flatten().unwrap_or_default();
                        if super::browser_noise::is_noise_url(&purl) {
                            if p.close().await.is_ok() {
                                closed += 1;
                            }
                        }
                    }
                    if closed > 0 {
                        tracing::info!(
                            closed,
                            "MCP_Web_Use 首启清扫浏览器初始空标签页(只留 Agent 打开的目标页)"
                        );
                    } else {
                        // 可观测性:清扫未命中也留痕,便于对账「首启是否残留多余页签」。
                        tracing::info!(
                            target_count = pages_len,
                            "MCP_Web_Use 首启清扫完成(无残留初始标签页)"
                        );
                    }
                }
            }
            // 第 168 轮:隐身自检(仅 launch 首启一次,fail-open 只进日志)——
            // 反检测被未来 Chrome 版本抹掉时第一时间可见;命名空间泄漏
            // (固定 laew 签名残留)与品牌文本泄漏(注入 UI 未 shadow 化)同理。
            // 探测页面即刚注册的 Agent 页。
            if let Some(entry) = inner.pages.get(&id) {
                let probe_js = r#"JSON.stringify({wd: navigator.webdriver === true, brand: (document.body && (document.body.innerText || '').indexOf('LAEW Agent') >= 0), leaked: Object.keys(window).filter((k) => k.toLowerCase().indexOf('laew') >= 0)})"#;
                if let Ok(v) = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    entry.page.evaluate(probe_js),
                )
                .await
                {
                    if let Ok(r) = v {
                        let issue = r
                            .value()
                            .cloned()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(s.as_str().unwrap_or("")).ok())
                            .and_then(|p| super::browser_ns::stealth_probe_issue(&p));
                        match issue {
                            Some(msg) => tracing::warn!(
                                "MCP_Web_Use 隐身自检未通过: {msg} (天猫/京东系风控可能因此拦截,见 browser_ns.rs)"
                            ),
                            None => tracing::info!("MCP_Web_Use 隐身自检通过(webdriver=false,无品牌文本,无 laew 残留键)"),
                        }
                    }
                }
            }
        }
        Ok((id, title, final_url))
    }

    /// 第 170 轮:浏览器句柄活探(3s 超时)。`browser.pages()` 失败 = CDP 连接已死
    /// (与 heal_page 同一判据);超时按死处理(fail-closed —— 宁可多清一次尸,
    /// 不留僵尸句柄吞掉 connect_url / 制造 receiver-gone 风暴)。
    async fn browser_handle_alive(browser: &chromiumoxide::Browser) -> bool {
        tokio::time::timeout(std::time::Duration::from_secs(3), browser.pages())
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false)
    }

    /// 第 170 轮:外部可触发的僵尸句柄清尸(工具层 2001 恢复 / close("all") 复用)。
    /// 活探通过则零副作用返回 false;探到死则按 new_page_impl 入口同语义清尸,
    /// 使下一次 open 走全新 launch/connect,返回 true。
    pub async fn drop_dead_browser(&self) -> bool {
        let mut inner = self.inner.lock().await;
        if inner.browser.is_none() {
            return false;
        }
        let dead = {
            let browser = inner.browser.as_ref().expect("checked is_some");
            !Self::browser_handle_alive(browser).await
        };
        if !dead {
            return false;
        }
        tracing::warn!("drop_dead_browser: 清理已死亡的浏览器句柄(CDP 无响应)");
        Self::purge_browser(&mut inner).await;
        true
    }

    /// 清空浏览器实例态(句柄 / handler / watchdog / profile 目录 / 页面表)。
    ///
    /// 第 172 轮抽出:`drop_dead_browser`(外部触发)与 `new_page_impl` 入口的
    /// 「死句柄预防」共用同一份清理语义,避免两处漂移(漏清 handler 会残留
    /// 僵尸任务、漏清 registry 会让 watchdog 杀不掉已退出的 Chrome)。
    /// 调用方必须已持有 `inner` 锁。
    async fn purge_browser(inner: &mut BrowserInner) {
        if let Some(mut browser) = inner.browser.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), browser.close())
                .await;
        }
        if let Some(handler) = inner.handler.take() {
            handler.abort();
        }
        if let Some(watcher) = inner.spawn_arm.take() {
            watcher.abort();
        }
        if let Some(child) = inner.watchdog.take() {
            spawn_watchdog_reaper(child);
        }
        if !inner.connect_mode {
            cleanup_registry::clear();
            if let Some(dir) = inner.user_data_dir.take() {
                spawn_tempdir_cleanup(dir);
            }
        }
        inner.pages.clear();
        inner.connect_mode = false;
    }

    /// 列出当前注册页面(带存活性探测,失效 entry 顺手清理)。
    ///
    /// 2026-10-16:快照-探测-清理三段式 —— CDP 探测(可能撞上卡死的 handler,
    /// 见 browser_cdp 模块头)全程在全局锁外执行,杜绝「持锁跨无界 await」
    /// 把单页卡死放大成整个 BrowserManager 死锁。
    pub async fn list_pages(&self) -> Vec<(String, String, String, String)> {
        let snapshot: Vec<(String, chromiumoxide::Page, String)> = {
            let inner = self.inner.lock().await;
            inner
                .pages
                .iter()
                .map(|(id, entry)| (id.clone(), entry.page.clone(), entry.created_at.clone()))
                .collect()
        };
        let mut dead = Vec::new();
        let mut out = Vec::new();
        for (id, page, created_at) in snapshot {
            match super::browser_cdp::cdp_with_timeout("list_pages.url", page.url()).await {
                Some(Some(url)) => {
                    let title = super::browser_cdp::cdp_with_timeout(
                        "list_pages.get_title",
                        page.get_title(),
                    )
                    .await
                    .flatten()
                    .unwrap_or_default();
                    out.push((id, url, title, created_at));
                }
                // 出错/CDP 超时(可能 handler 卡死):保守按死页清理,
                // 避免每次 list 都重新撞一次卡死 target。
                _ => dead.push(id),
            }
        }
        if !dead.is_empty() {
            let mut inner = self.inner.lock().await;
            for id in dead {
                inner.pages.remove(&id);
                // F2:可见区记账随页销毁,防泄漏。
                inner.page_visible_box.remove(&id);
            }
        }
        out
    }

    /// 获取页面。返回 None 表示 page_id 失效(工具层映射 2000)。
    pub async fn page(&self, page_id: &str) -> Option<Page> {
        self.inner
            .lock()
            .await
            .pages
            .get(page_id)
            .map(|e| e.page.clone())
    }

    /// 获取页面(带失效自愈,第 159 轮)。
    ///
    /// 实测事故(2026-10-12 内网站点):人工在地址栏回车导航后,Agent 对该页面的
    /// 后续操作全面失效 —— 注册表里的 `PageEntry.page` 是一次性握手时拿到的句柄,
    /// CDP 会话掉线(`Session with given id not found` 类)后旧实现无自愈路径,
    /// 所有 control/inspect 动作永久 2002。这里先探活(一次 target 信息往返,
    /// 不经过页面主线程),失效则按 target_id 重建句柄(见 [`Self::heal_page`])。
    ///
    /// 2026-10-12 返回值改为 `Result`:见 [`Self::heal_page`] 关于「页签被关」
    /// 与「CDP 连接已死」两种失败的区分 —— 后者必须把可行动的处置指引
    /// (重新 `open`)传上去,而不是谎称 page_id 不存在。
    pub async fn ensure_page_alive(&self, page_id: &str) -> Result<Option<Page>, String> {
        let cached = {
            let inner = self.inner.lock().await;
            match inner.pages.get(page_id) {
                Some(e) => e.page.clone(),
                None => return Ok(None),
            }
        };
        if cached.url().await.is_ok() {
            return Ok(Some(cached));
        }
        self.heal_page(page_id).await
    }

    /// 按 target_id 重建页面句柄:重开 Runtime/Network 域 + 事件监听,
    /// 高亮/管控/跟随器按实例期望态重注入(人工导航产生的新文档没有旧脚本)。
    ///
    /// 2026-10-12 返回值语义细化 —— 区分两种完全不同的失败:
    /// - `Ok(None)`  = 标签页真的被关掉了(page_id 确实不再存在);
    /// - `Err(reason)` = **浏览器 CDP 连接已死**(handler 已终止,命令发不出去),
    ///   页签可能还在。这两种情况原先都被 `?` 吞成 `None`,上层一律回
    ///   「page_id 不存在」—— 那是**假事实**,会把模型引向「页面没了」的错误结论。
    pub async fn heal_page(&self, page_id: &str) -> Result<Option<Page>, String> {
        let mut inner = self.inner.lock().await;
        let browser = inner.browser.as_ref().ok_or_else(|| {
            "浏览器未启动(无 CDP 连接),请重新 open 目标 URL".to_string()
        })?;
        let entry = inner.pages.get(page_id).ok_or("page_id 不存在")?;
        let target_id = entry.target_id.clone();
        // `browser.pages()` 失败 = CDP 连接已死(chromiumoxide 的 handler 终止后,
        // 命令走 oneshot 会报 "send failed because receiver is gone")。
        let pages = browser.pages().await.map_err(|e| {
            format!(
                "浏览器 CDP 连接已断开({e});页签可能仍在,请对该 URL 重新 `open`(可带 reuse=true)恢复会话"
            )
        })?;
        // 页签确实不在了 —— 那是「page_id 不存在」,不是连接问题。
        let found = pages
            .into_iter()
            .find(|p| {
                let t: String = p.target_id().clone().into();
                t == target_id
            })
            .ok_or("page_id 不存在(标签页已关闭)")?;
        let events = EventBuffer::default();
        let _ = found
            .execute(chromiumoxide::cdp::js_protocol::runtime::EnableParams::default())
            .await;
        let _ = found
            .execute(chromiumoxide::cdp::browser_protocol::network::EnableParams::default())
            .await;
        spawn_event_listeners(&found, events.clone()).await;
        // 第 169 轮:自愈重建句柄的页面重挂隐身覆写(CDP 会话掉线重连后新文档
        // 上下文需要重新挂载;connect 零注入不变)。第 174 轮:UA 清理同挂
        // (重连后的下一次导航同样要无令牌,否则掉线一次就前功尽弃)。第 180 轮:
        // screen_sync 同挂(无头档重连页的几何自洽防线对齐 adopt/new_page 路径)。
        if !inner.connect_mode {
            super::browser_ns::apply_ua_destealth(&found, inner.stealth).await;
            super::browser_ns::inject_stealth(&found, inner.stealth).await;
            apply_screen_sync_override(&found, inner.screen_sync).await;
            // 第 182 轮:自愈页同挂指针/触摸环境仿真(与隐身脚本同批,幂等)。
            super::browser_env_emulation::apply_pointer_env_emulation(&found, inner.stealth, inner.mode).await;
        }
        if inner.highlight && matches!(inner.mode, BrowserMode::Headed) {
            inject_agent_highlight(&found).await;
        }
        if !inner.connect_mode && matches!(inner.mode, BrowserMode::Headed) {
            let cfg = inner.guard.clone();
            super::browser_overlay::inject_agent_guard(&found, &cfg).await;
            let _ = super::browser_overlay::apply_page_guard(&found, &cfg).await;
        }
        if !inner.connect_mode && inner.auto_follow {
            super::browser_follow::install(&found, true).await;
            let _ = super::browser_follow::set(&found, true).await;
        }
        if let Some(e) = inner.pages.get_mut(page_id) {
            e.page = found.clone();
            e.events = events;
        }
        tracing::info!(page_id, target_id, "MCP_Web_Use 页面句柄自愈成功(重建会话与管控)");
        Ok(Some(found))
    }

    /// 获取页面事件缓冲(console/network 观察用)。
    pub async fn events(&self, page_id: &str) -> Option<EventBuffer> {
        self.inner
            .lock()
            .await
            .pages
            .get(page_id)
            .map(|e| e.events.clone())
    }

    // =================== 第 100 轮:窗口可视化 / 人工介入支撑 ===================

    /// 派生页早期武装的句柄解析(第 180 轮):按 target_id 取 `Page` + 配置快照。
    ///
    /// **全程有界**(watcher 铁律:不得持锁跨无界 await):
    /// - 锁等待 ≤1s/次、总预算 3s(`new_page_impl` 持锁可跨 launch+goto 达 30s+,
    ///   等不到就放弃本页,交 adopt 收编兜底);
    /// - 锁内 `get_page` ≤2s(attach 完成前返回 NotFound 是**瞬时**失败,静默重试;
    ///   handler 卡死时 2s 放弃 —— 此时全部 CDP 通道本就同死,不多损什么)。
    /// 失败一律 `None`(fail-open,页面自身不受影响)。
    pub(crate) async fn spawn_arm_resolve(
        &self,
        tid: &chromiumoxide::cdp::browser_protocol::target::TargetId,
    ) -> Option<(chromiumoxide::Page, super::browser_spawn_arm::SpawnArmConfig)> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let guard = match tokio::time::timeout(
                std::time::Duration::from_secs(1),
                self.inner.lock(),
            )
            .await
            {
                Ok(guard) => guard,
                Err(_) => {
                    if std::time::Instant::now() >= deadline {
                        tracing::debug!("派生页早期武装:3s 未取得管理锁(open 在飞),交由 adopt 收编兜底");
                        return None;
                    }
                    continue;
                }
            };
            let Some(browser) = guard.browser.as_ref() else {
                return None;
            };
            let cfg = guard.spawn_arm_config();
            match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                browser.get_page(tid.clone()),
            )
            .await
            {
                Ok(Ok(page)) => return Some((page, cfg)),
                Ok(Err(_)) => {
                    // 未 attach 完(NotFound)或 target 已销毁 —— 释放锁后重试到界。
                    drop(guard);
                    if std::time::Instant::now() >= deadline {
                        tracing::debug!("派生页早期武装:3s 内未取得页面句柄(target 可能已关闭)");
                        return None;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(_) => {
                    tracing::warn!("派生页早期武装:get_page 超时(浏览器 handler 可能已卡死),放弃本页");
                    return None;
                }
            }
        }
    }

    /// 当前生效的反检测档位（第 174 轮）。connect 模式（接管用户浏览器）恒
    /// 与实例期望态无关地零注入，但档位回报照旧，供对账。
    pub async fn stealth_mode(&self) -> super::stealth::StealthMode {
        self.inner.lock().await.stealth
    }

    /// 切换反检测档位（第 174 轮）：`open(stealth=…)` / `control(set_stealth)` 入口。
    ///
    /// **诚实语义**：升档即时生效（对全部存活页面重挂脚本 + 重下发 UA 覆盖）；
    /// 降档只能阻止**新文档**注入 —— 已在旧文档执行过的覆写无法回退（原生对象一旦
    /// 被改写就没有"还原"这个动作），响应据此如实回报，不假装已还原。
    ///
    /// 返回受影响页面数；connect 模式零动作（真实浏览器本无这些痕迹）。
    pub async fn set_stealth(&self, mode: super::stealth::StealthMode) -> usize {
        let (pages, connect_mode, browser_mode) = {
            let mut inner = self.inner.lock().await;
            inner.stealth = mode;
            (
                inner.pages.values().map(|e| e.page.clone()).collect::<Vec<_>>(),
                inner.connect_mode,
                inner.mode,
            )
        };
        if connect_mode {
            return 0;
        }
        for page in &pages {
            super::browser_ns::apply_ua_destealth(page, mode).await;
            super::browser_ns::inject_stealth(page, mode).await;
            // 第 182 轮:档位切换同步仿真态 —— 升档即时仿真,降到 off 复位仿真
            // (有头档零动作;connect 上面已早退)。
            super::browser_env_emulation::apply_pointer_env_emulation(page, mode, browser_mode).await;
        }
        pages.len()
    }

    /// 把指定页面的标签页带到前台(`Page.bringToFront`),人工介入前调用,
    /// 保证用户能看到被 Agent 控制的页面。仅 headed 模式有视觉效果。
    pub async fn bring_page_to_front(&self, page_id: &str) -> std::result::Result<(), String> {
        let page = self
            .page(page_id)
            .await
            .ok_or_else(|| "page_id 不存在".to_string())?;
        page.bring_to_front()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// 第 162 轮:地理位置原生权限弹窗自动处置(详见 `browser_perm` 模块头)。
    /// 取页面当前 URL 的 origin,按 `LAEW_WEB_GEO` 档位 CDP 预授权;
    /// 全程 fail-open(仅 warn 日志),绝不因权限处置失败阻断导航主流程。
    /// 返回处置结果 JSON,供 open/control 响应 `data.permissions` 透出。
    pub async fn auto_geo_permission(&self, page_id: &str) -> Option<Value> {
        let mode = browser_perm::geo_mode_from_env();
        let page = self.page(page_id).await?;
        let url = page.url().await.ok().flatten().unwrap_or_default();
        let origin = match browser_perm::origin_of(&url) {
            Some(o) => o,
            None => return Some(browser_perm::geo_outcome_json(mode, None, None)),
        };
        let inner = self.inner.lock().await;
        let browser = match inner.browser.as_ref() {
            Some(b) => b,
            None => return Some(browser_perm::geo_outcome_json(mode, Some(&origin), None)),
        };
        let applied = match browser_perm::apply_geo_permission(browser, &origin, mode).await {
            Ok(setting) => Some(setting.as_ref().to_string()),
            Err(e) => {
                tracing::warn!("地理位置权限预授权失败(fail-open):{e} origin={origin}");
                None
            }
        };
        drop(inner);
        Some(browser_perm::geo_outcome_json(
            mode,
            Some(&origin),
            applied.as_deref(),
        ))
    }

    /// 运行时调整浏览器窗口位置/尺寸/状态(CDP `Browser.setWindowBounds`)。
    ///
    /// - `window_state` 为 maximized/fullscreen/minimized 时只发状态(协议禁止
    ///   与几何字段混用),normal 可与 left/top/width/height 任意组合;
    /// - 调整成功后自动清除视口覆盖(`Emulation.clearDeviceMetricsOverride`),
    ///   让视口=窗口内容区 —— 人工拖动/Agent 缩放窗口后渲染自适应、不缺区域;
    /// - 返回调整后的真实窗口 bounds + 页面视口(供 Agent 校验)。
    #[allow(clippy::too_many_arguments)]
    pub async fn set_window_bounds(
        &self,
        page_id: &str,
        width: Option<i64>,
        height: Option<i64>,
        left: Option<i64>,
        top: Option<i64>,
        window_state: Option<&str>,
    ) -> std::result::Result<Value, String> {
        let page = self
            .page(page_id)
            .await
            .ok_or_else(|| "page_id 不存在".to_string())?;
        // 持锁完成 browser 命令(Browser 不可 Clone;窗口调整是低频操作,
        // 短暂持锁不影响并发页面操作)。
        let inner = self.inner.lock().await;
        let browser = inner
            .browser
            .as_ref()
            .ok_or_else(|| "浏览器会话不存在".to_string())?;

        let before = page
            .execute(GetWindowForTargetParams::default())
            .await
            .map_err(|e| format!("获取窗口信息失败:{e}"))?;
        let window_id = before.window_id;

        let state = match window_state {
            Some("maximized") => Some(WindowState::Maximized),
            Some("minimized") => Some(WindowState::Minimized),
            Some("fullscreen") => Some(WindowState::Fullscreen),
            Some("normal") | None => None,
            Some(other) => return Err(format!("非法 window_state:{other}")),
        };
        let mut bounds = Bounds::builder();
        if let Some(s) = state {
            bounds = bounds.window_state(s);
        } else {
            if let Some(w) = width {
                bounds = bounds.width(w);
            }
            if let Some(h) = height {
                bounds = bounds.height(h);
            }
            if let Some(l) = left {
                bounds = bounds.left(l);
            }
            if let Some(t) = top {
                bounds = bounds.top(t);
            }
        }
        let params = SetWindowBoundsParams::builder()
            .window_id(window_id)
            .bounds(bounds.build())
            .build()
            .map_err(|e| e.to_string())?;
        browser
            .execute(params)
            .await
            .map_err(|e| format!("调整窗口失败:{e}"))?;

        // 视口跟随窗口:清除 device metrics 覆盖,渲染=窗口内容区(自适应、不缺区域)。
        let _ = page.execute(
            chromiumoxide::cdp::browser_protocol::emulation::ClearDeviceMetricsOverrideParams::default(),
        )
        .await;
        // F2:覆盖已清除 → 告知栏恢复自然锚定。
        self.inner.lock().await.page_visible_box.remove(page_id);
        // 窗口 resize 是异步的,短暂等待后回读真实值。
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let after = page
            .execute(GetWindowForTargetParams::default())
            .await
            .map_err(|e| format!("回读窗口信息失败:{e}"))?;
        let viewport = page
            .evaluate("({width: window.innerWidth, height: window.innerHeight})")
            .await
            .ok()
            .and_then(|v| v.value().cloned())
            .unwrap_or(Value::Null);
        // 第 165 轮(2026-10-12 Windows 实测):CDP `setWindowBounds` 在 Windows 上
        // **不保证**几何精确回环 —— `--device-scale-factor=1.25` 下 DIP↔物理像素
        // 取整叠加窗口边框,实测请求 1100×700 稳定回读 1102×702(999×777 → 1001×778)。
        // 故响应同时回 `requested` 与 `window`(实际 bounds),供 Agent/QC 对账;
        // 判断「窗口尺寸是否到位」应以 `requested` vs `window` 的容差比较为准,
        // 不能假设二者逐像素相等(macOS 上通常相等,Windows 上通常差 1~2px)。
        Ok(json!({
            "requested": {"width": width, "height": height, "left": left, "top": top,
                          "window_state": window_state},
            "window": {
                "left": after.bounds.left,
                "top": after.bounds.top,
                "width": after.bounds.width,
                "height": after.bounds.height,
                "window_state": after.bounds.window_state.as_ref().map(|s| s.as_ref().to_string()),
            },
            "viewport": viewport,
        }))
    }

    /// 视口同步:清除 `Emulation.setDeviceMetricsOverride` 覆盖,让页面视口
    /// 自适应真实窗口内容区(人工拖动窗口大小后调用,消除黑边/缺区域/渲染不全)。
    pub async fn sync_viewport(&self, page_id: &str) -> std::result::Result<Value, String> {
        let page = self
            .page(page_id)
            .await
            .ok_or_else(|| "page_id 不存在".to_string())?;
        page.execute(
            chromiumoxide::cdp::browser_protocol::emulation::ClearDeviceMetricsOverrideParams::default(),
        )
        .await
        .map_err(|e| format!("清除视口覆盖失败:{e}"))?;
        // F2:覆盖已清除 → 告知栏恢复自然锚定(bottom/right 直接锚窗口)。
        self.inner.lock().await.page_visible_box.remove(page_id);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let viewport = page
            .evaluate(
                "({width: window.innerWidth, height: window.innerHeight, devicePixelRatio: window.devicePixelRatio, scrollWidth: document.documentElement.scrollWidth, scrollHeight: document.documentElement.scrollHeight})",
            )
            .await
            .map_err(|e| format!("读取视口失败:{e}"))?;
        Ok(viewport.value().cloned().unwrap_or(Value::Null))
    }

    /// 第 139 轮:有头模式窗口收边 —— 保证「窗口完整落在屏幕内 + 视口 ≥ 720p」。
    ///
    /// 背景(实测 2026-10-09):窗口固定 `--window-size=1920,1080`,在
    /// 1440×900 / 1512×982 之类的小屏笔记本上右侧与底部被挤出屏幕外,用户看到的
    /// 就是「显示不全」。本方法在导航完成后实测 `window.screen.avail*`(工作区,
    /// 已扣 Dock)与 `outer*/inner*`(外框与 chrome 高度),交
    /// [`headed_window_plan`] 决策,必要时走 `Browser.setWindowBounds` 收边。
    ///
    /// 契约:
    /// - 仅对有头模式有意义(无头没有可见窗口,收边只会白折腾),故内部自判;
    /// - **必须早于** [`BrowserManager::fit_viewport_to_content`] 调用 ——
    ///   `set_window_bounds` 会清除 `Emulation.setDeviceMetricsOverride`,晚一步
    ///   会把刚设的视口覆盖抹掉;
    /// - fail-open:测量或调整失败只回 `adjusted:false` + 原因,不影响 open 语义。
    pub async fn fit_headed_window(
        &self,
        page_id: &str,
    ) -> std::result::Result<Value, String> {
        let (inner_mode, connect_mode, explicit_window) = {
            let g = self.inner.lock().await;
            (g.mode, g.connect_mode, g.explicit_window)
        };
        let base = json!({
            "mode": inner_mode.as_str(),
            "min_viewport": [super::browser_viewport::MIN_VIEWPORT_W, super::browser_viewport::MIN_VIEWPORT_H],
        });
        if !inner_mode.is_headed() {
            return Ok(json!({ "adjusted": false, "reason": "非有头模式,无需收边", "base": base }));
        }
        // 第 156 轮:connect 模式(第 142/144 轮「复用已登录浏览器」)接管的是**用户
        // 自己的** Chrome —— 调整它的窗口尺寸/把它抢到前台都是对用户窗口的 intrusive
        // 操作,违反「connect 模式零注入、不干预用户浏览器」原则(该原则此前只覆盖
        // guard 蒙层与输入锁定,窗口几何这一路漏了)。此处整体跳过。
        if connect_mode {
            return Ok(json!({
                "adjusted": false,
                "activated": false,
                "reason": "connect 模式不干预用户浏览器窗口(不调尺寸、不抢前台)",
                "base": base,
            }));
        }
        let page = self
            .page(page_id)
            .await
            .ok_or_else(|| "page_id 不存在".to_string())?;
        // F3(2026-10-16):默认窗口 1080p 首开一次性校正。
        // `--window-size=1920,1080` 在 Windows 150% 缩放 + 多显示器组合下不被
        // Chrome 精确执行(实测首开 outer 1922×1043,高短 37px);第 158 轮的
        // 「保持当前尺寸不铺满」把该错误结果也一并「保持」了。实例级一次性:
        // 偏差超容差(4px,吸收 Windows DIP↔物理取整 ±2px)则 setWindowBounds
        // 校正到「默认尺寸 ∩ 屏幕工作区」;此后不再触碰(用户手动调整以其为准)。
        if !explicit_window {
            let first_fix = {
                let mut g = self.inner.lock().await;
                if g.default_size_applied {
                    None
                } else {
                    g.default_size_applied = true;
                    Some(())
                }
            };
            if first_fix.is_some() {
                if let Ok(fm) = headed_window_metrics(&page).await {
                    let gf = |k: &str| fm.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                    let (ow, oh, aw, ah) =
                        (gf("outerWidth"), gf("outerHeight"), gf("availWidth"), gf("availHeight"));
                    if aw > 0.0 && ah > 0.0 && ow > 0.0 && oh > 0.0 {
                        let margin = HEADED_SCREEN_MARGIN as f64;
                        let tw = (DEFAULT_WINDOW_W as f64).min(aw - margin * 2.0).max(320.0);
                        let th = (DEFAULT_WINDOW_H as f64).min(ah - margin * 2.0).max(240.0);
                        if (ow - tw).abs() > 4.0 || (oh - th).abs() > 4.0 {
                            // 位置不动(尊重 Chrome/窗口管理器摆放);若校正后越出
                            // 工作区,下方常规流程会按收边逻辑补 left/top。
                            let _ = self
                                .set_window_bounds(
                                    page_id,
                                    Some(tw as i64),
                                    Some(th as i64),
                                    None,
                                    None,
                                    None,
                                )
                                .await;
                        }
                    }
                }
            }
        }
        let m = headed_window_metrics(&page).await?;
        let f = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let (ow, oh) = (f("outerWidth"), f("outerHeight"));
        let (iw, ih) = (f("innerWidth"), f("innerHeight"));
        // 第 156 轮:裸减法在 macOS 上确定性失真(inner 报布局实现前的默认值 800×600,
        // 而 outer 已是 --window-size 的 1920×1080),chrome 被算成 480(真值约 143),
        // 窗口每次 headed open 都被反向撑大。改走可信性判定 + 兜底常量。
        let chrome =
            super::browser_viewport::plausible_chrome_height(ow, oh, iw, ih);
        let chrome_measured = oh - ih;
        let (aw, ah) = (f("availWidth"), f("availHeight"));
        // 工作区原点:多屏 / 副屏左置时 availLeft/availTop 非 0,铺满时必须以它为基准。
        let (ax, ay) = (f("availLeft"), f("availTop"));
        let fill = !explicit_window;
        let measured = json!({
            "screen": {"avail_width": aw, "avail_height": ah,
                       "avail_left": ax, "avail_top": ay},
            "outer": {"width": ow, "height": oh},
            "inner": {"width": iw, "height": ih},
            "chrome_height": chrome,
            // 第 156 轮:实测原值与失真回退对账。两者不等即说明本次走了兜底常量,
            // 供 Agent / QC 判断窗口尺寸决策建立在真值还是估计值之上。
            "chrome_height_raw": chrome_measured,
            "chrome_height_fallback": (chrome - chrome_measured).abs() > 0.5,
            "fill_ratio_before": fill_ratio(ow, oh, aw, ah),
        });
        // 第 156 轮 F2:headed 启动后把窗口带到前台。
        // 实测痛点:headed 确实拉起了 Chrome(code:0 + title 都拿到了),但项目从不
        // 激活窗口 —— `bring_page_to_front` 此前只在 `request_human` 前调用一次,
        // open 路径零调用;也不设 `--window-position` / `--start-maximized`。
        // 结果 Chrome 弹在终端/编辑器后面,用户看到的就是「Agent 没启动浏览器」。
        // 纯 CDP `Page.bringToFront`,跨平台一致、零新依赖;fail-open 不影响语义。
        let activated = super::browser_window::activate_headed_window(self, page_id).await;
        let Some((tw, th)) = headed_window_plan(ow, oh, aw, ah, chrome, fill) else {
            return Ok(json!({
                "adjusted": false,
                "activated": activated,
                "fill": fill,
                "reason": if explicit_window {
                    "已显式指定窗口尺寸且完整落在屏幕内,保持不变"
                } else {
                    "窗口尺寸保持当前值(不铺满)且视口不低于下限"
                },
                "measured": measured,
                "base": base,
            }));
        };
        // 第 158 轮:不再铺满,positioned 仅在**收边**(窗口超出屏幕)时设置 left/top
        // 保证完整可见;抬升(窗口低于视口下限)保持原位置不动,尊重用户手动调整的窗口位置。
        let margin = super::browser_viewport::HEADED_SCREEN_MARGIN as i64;
        let is_shrink = (tw as f64) <= ow + 1.0 && (th as f64) <= oh + 1.0 && ((tw as f64) < ow || (th as f64) < oh);
        let (tl, tt) = if is_shrink {
            (Some(ax.round() as i64 + margin), Some(ay.round() as i64 + margin))
        } else {
            (None, None)
        };
        let applied = self
            .set_window_bounds(page_id, Some(tw as i64), Some(th as i64), tl, tt, None)
            .await?;
        // 调整尺寸后 Chrome 可能重新压回 z 序底层,复激活一次保证「调完仍然可见」。
        let activated =
            super::browser_window::activate_headed_window(self, page_id).await || activated;
        Ok(json!({
            "adjusted": true,
            "activated": activated,
            "fill": fill,
            "from": {"width": ow, "height": oh},
            "target": {"width": tw, "height": th},
            "positioned": tl.is_some(),
            "measured": measured,
            "applied": applied.get("window").cloned().unwrap_or(Value::Null),
            "note": if is_shrink {
                "有头模式窗口已收边到屏幕工作区内,保证完整可见;页面视口下限 720p"
            } else {
                "有头模式窗口已抬升到视口下限 720p,保持原位置;不铺满"
            },
            "base": base,
        }))
    }

    /// 第 125 轮:页面内容超出视口时自动扩展视口(上限 2K)。
    ///
    /// open(新建 + 复用两条路径)导航完成后调用,`auto_expand_viewport=false`
    /// 可关:
    /// - 测量 innerWidth/Height 与 scrollWidth/Height,[`viewport_fit_plan`] 决策
    ///   目标尺寸(横向裁切 = 元素不可见不可点的根因;纵向不足 = 截图缺区域);
    /// - 需要扩展时走 `Emulation.setDeviceMetricsOverride`(保持当前 DPR,
    ///   mobile=false)撑大布局视口 —— headless 视口控制的标准机制
    ///   (Playwright/Puppeteer 同款),截图与输入坐标都按新视口映射;
    /// - 覆盖对页面内后续导航持续生效,直到 `sync_viewport` / `set_window` 清除;
    /// - 返回结构化结果供 open 响应 `data.viewport` 引用(供 LLM 对账)。
    pub async fn fit_viewport_to_content(
        &self,
        page_id: &str,
        max_w: u32,
        max_h: u32,
    ) -> std::result::Result<Value, String> {
        let page = self
            .page(page_id)
            .await
            .ok_or_else(|| "page_id 不存在".to_string())?;
        let m = page_viewport_metrics(&page).await?;
        let f = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let (iw, ih, sw, sh) = (f("innerWidth"), f("innerHeight"), f("scrollWidth"), f("scrollHeight"));
        let dpr = m.get("devicePixelRatio").and_then(Value::as_f64).unwrap_or(1.0);
        let content = json!({"width": sw, "height": sh});
        let Some((tw, th)) = viewport_fit_plan(iw, ih, sw, sh, max_w, max_h) else {
            return Ok(json!({
                "expanded": false,
                "viewport": {"width": iw, "height": ih},
                "content": content,
                "window_default": [DEFAULT_WINDOW_W, DEFAULT_WINDOW_H],
            }));
        };
        let clamped =
            sw > max_w as f64 + VIEWPORT_FIT_TOL || sh > max_h as f64 + VIEWPORT_FIT_TOL;
        let params = chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams::builder()
            .width(tw as i64)
            .height(th as i64)
            .device_scale_factor(dpr.clamp(1.0, 4.0))
            // 第 175 轮:同尺寸声明 screen —— 2K 扩展会把 innerWidth 撑到 2560,
            // 而 screen 仍是无头默认值 800x600,`outer > screen` 物理不可能同样成立
            // (与启动档同一根因,这里同样补齐)。
            .screen_width(tw as i64)
            .screen_height(th as i64)
            .position_x(0)
            .position_y(0)
            .mobile(false)
            .build()
            .map_err(|e| e.to_string())?;
        page.execute(params)
            .await
            .map_err(|e| format!("视口扩展失败: {e}"))?;
        // F2(2026-10-16):登记「覆盖前可见区」(上面的 iw/ih 即真实窗口内容区)。
        // 告知栏的 bottom/right 锚定据此重锚可见区,否则锚到撑大后的布局视口
        // 底边落在可见窗口外;下一次 broadcast push 会把它带给页面侧注入层。
        {
            let mut g = self.inner.lock().await;
            g.page_visible_box.insert(page_id.to_string(), (iw, ih));
        }
        // 布局重排是异步的,短暂等待后回读真实视口。
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let m2 = page_viewport_metrics(&page).await.unwrap_or(Value::Null);
        let g = |k: &str, fb: u32| {
            m2.get(k).and_then(Value::as_f64).unwrap_or(fb as f64)
        };
        Ok(json!({
            "expanded": true,
            "from": {"width": iw, "height": ih},
            "to": {"width": g("innerWidth", tw), "height": g("innerHeight", th)},
            "target": {"width": tw, "height": th},
            "content": content,
            "clamped": clamped,
            "hint": clamped.then(|| "页面内容仍超 2K 上限:截图用 params.full_page=true 捕获整页;或 open 传更大 window_width/window_height / control(set_viewport) 显式超限"),
            "note": "device metrics 覆盖对页面内导航持续生效;需还原用 control(sync_viewport)",
        }))
    }

    /// 运行时开关 Agent 高亮蓝框(对当前页面文档立即生效;新文档由挂载脚本
    /// 依据高亮期望态全局名延续(第 168 轮起随进程级随机命名空间,见 browser_ns.rs))。
    pub async fn set_highlight(
        &self,
        page_id: &str,
        enabled: bool,
    ) -> std::result::Result<Value, String> {
        let page = self
            .page(page_id)
            .await
            .ok_or_else(|| "page_id 不存在".to_string())?;
        {
            let mut inner = self.inner.lock().await;
            inner.highlight = enabled;
        }
        let ns = super::browser_ns::ns_member_expr("AgentApplyHighlight");
        let js = format!("{ns} ? {ns}({enabled}) : null");
        let r = page
            .evaluate(js)
            .await
            .map_err(|e| format!("切换高亮失败:{e}"))?;
        // 第 171 轮:两条告知栏随高亮开关联动 —— `set_highlight(false)` 的语义是
        // 「关掉页面上的 Agent 可视化标识」,人工明确不要看到注入层时,状态栏与
        // 进度栏一并隐藏(guard 管控本身不受影响,输入锁照旧)。
        super::browser_status::set_visible(&page, enabled).await;
        Ok(json!({
            "enabled": enabled,
            "applied": !r.value().map(|v| v.is_null()).unwrap_or(true),
        }))
    }

    // ===================== 2026-10-13(二):侵入式物理输入中继 =====================
    //
    // 弹窗「侵入式」触控板上的人工手势 → 本层映射到页面视口坐标 → CDP
    // `Input.dispatchMouseEvent` / `dispatchKeyEvent` 回放。价值:
    // ① 事件是 `isTrusted=true` 的**真物理输入**(页面 JS 无法与真手区分);
    // ② 坐标级派发天然穿透跨域 iframe(天猫/淘宝滑块「请按住滑块」在跨域 iframe 内,
    //    DOM 级挖洞与页面自身闸一律够不着,坐标级 CDP 不受影响);
    // ③ 不依赖浏览器窗口焦点(实测人工在弹窗操作时 Chrome 窗口可能失焦,
    //    真实鼠标按下被窗口激活语义吞掉,滑块拖不动)。
    // 前提:调用方(session_start)已放行本页管控 —— locked 档的自有输入闸只拦
    // `isTrusted=true`,不放行会把我们自己的回放也拦掉。

    /// 中继鼠标事件(kind = down/move/up/tap;nx/ny = 0..1 归一化坐标)。
    /// `rect` = 映射区(视口坐标;None = 映射整个视口)。拖拽中的 move 按
    /// ~10px 步长插值补点(滑块类挑战的轨迹校验需要连续密集采样)。
    pub async fn relay_mouse(
        &self,
        page_id: &str,
        kind: &str,
        nx: f64,
        ny: f64,
        rect: Option<(f64, f64, f64, f64)>,
    ) -> std::result::Result<Value, String> {
        use chromiumoxide::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
        };
        let page = self.page(page_id).await.ok_or_else(|| "page_id 不存在".to_string())?;
        let (vw, vh) = self.viewport_size(&page).await;
        let (px, py) = match rect {
            Some((rx, ry, rw, rh)) => (rx + nx.clamp(0.0, 1.0) * rw, ry + ny.clamp(0.0, 1.0) * rh),
            None => (nx.clamp(0.0, 1.0) * vw, ny.clamp(0.0, 1.0) * vh),
        };
        let (down_before, last) = {
            let st = relay_state();
            let mut guard = st.lock().unwrap_or_else(|p| p.into_inner());
            let entry = guard.entry(page_id.to_string()).or_default();
            (entry.down, entry.last)
        };
        let dispatch = |t: DispatchMouseEventType, x: f64, y: f64, buttons: Option<i64>| {
            let p = page.clone();
            async move {
                let mut b = DispatchMouseEventParams::builder()
                    .r#type(t)
                    .x(x)
                    .y(y)
                    .button(MouseButton::Left)
                    .click_count(1);
                if let Some(n) = buttons {
                    b = b.buttons(n);
                }
                let params = b.build().map_err(|e| e.to_string())?;
                p.execute(params).await.map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            }
        };
        match kind {
            "down" => {
                dispatch(DispatchMouseEventType::MousePressed, px, py, Some(1)).await?;
            }
            "move" => {
                // 拖拽中:先按步长插值补点(最多 20 步),再落到目标点。
                if down_before {
                    if let Some((lx, ly)) = last {
                        let (dx, dy) = (px - lx, py - ly);
                        let dist = (dx * dx + dy * dy).sqrt();
                        let steps = ((dist / 10.0).ceil() as i64).clamp(1, 20);
                        for i in 1..steps {
                            let f = i as f64 / steps as f64;
                            dispatch(
                                DispatchMouseEventType::MouseMoved,
                                lx + dx * f,
                                ly + dy * f,
                                Some(1),
                            )
                            .await?;
                        }
                    }
                }
                dispatch(
                    DispatchMouseEventType::MouseMoved,
                    px,
                    py,
                    if down_before { Some(1) } else { None },
                )
                .await?;
            }
            "up" => {
                dispatch(DispatchMouseEventType::MouseReleased, px, py, None).await?;
            }
            "tap" => {
                dispatch(DispatchMouseEventType::MousePressed, px, py, Some(1)).await?;
                dispatch(DispatchMouseEventType::MouseReleased, px, py, None).await?;
            }
            other => return Err(format!("未知中继鼠标事件: {other}")),
        }
        {
            let st = relay_state();
            let mut guard = st.lock().unwrap_or_else(|p| p.into_inner());
            let entry = guard.entry(page_id.to_string()).or_default();
            match kind {
                "down" => entry.down = true,
                "up" | "tap" => entry.down = false, // tap 已即时抬起
                "move" => entry.down = down_before,
                _ => {}
            }
            entry.last = Some((px, py));
        }
        Ok(json!({"x": px, "y": py, "kind": kind}))
    }

    /// 中继键盘事件(key = enter/esc/tab/space/backspace/delete/left/right/up/down)。
    pub async fn relay_key(
        &self,
        page_id: &str,
        key: &str,
    ) -> std::result::Result<Value, String> {
        use chromiumoxide::cdp::browser_protocol::input::{
            DispatchKeyEventParams, DispatchKeyEventType,
        };
        let (name, code, vk, text): (&str, &str, i64, &str) = match key.to_ascii_lowercase().as_str()
        {
            "enter" | "return" => ("Enter", "Enter", 13, "\r"),
            "esc" | "escape" => ("Escape", "Escape", 27, ""),
            "tab" => ("Tab", "Tab", 9, ""),
            "space" => (" ", "Space", 32, " "),
            "backspace" => ("Backspace", "Backspace", 8, ""),
            "delete" => ("Delete", "Delete", 46, ""),
            "left" => ("ArrowLeft", "ArrowLeft", 37, ""),
            "up" => ("ArrowUp", "ArrowUp", 38, ""),
            "right" => ("ArrowRight", "ArrowRight", 39, ""),
            "down" => ("ArrowDown", "ArrowDown", 40, ""),
            other => return Err(format!("未知中继按键: {other}")),
        };
        let page = self.page(page_id).await.ok_or_else(|| "page_id 不存在".to_string())?;
        let mut down = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyDown)
            .key(name)
            .code(code)
            .windows_virtual_key_code(vk)
            .native_virtual_key_code(vk);
        if !text.is_empty() {
            down = down.text(text);
        }
        page.execute(down.build().map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())?;
        let up = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyUp)
            .key(name)
            .code(code)
            .windows_virtual_key_code(vk)
            .native_virtual_key_code(vk)
            .build()
            .map_err(|e| e.to_string())?;
        page.execute(up).await.map_err(|e| e.to_string())?;
        Ok(json!({"key": name}))
    }

    /// 侵入式会话收口:清本页拖拽中继状态(防陈旧 down 态跨会话泄漏)。
    pub fn relay_reset(&self, page_id: &str) {
        let st = relay_state();
        st.lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(page_id);
    }

    /// 页面视口尺寸(CDP 中继坐标基准);失败回退 1920×1080。
    async fn viewport_size(&self, page: &Page) -> (f64, f64) {
        use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
        let expr = "JSON.stringify({w: window.innerWidth, h: window.innerHeight})";
        if let Ok(params) = EvaluateParams::builder()
            .expression(expr)
            .return_by_value(true)
            .build()
        {
            if let Ok(v) = page.evaluate(params).await {
                let s = v.value().and_then(|x| x.as_str().map(str::to_string));
                if let Some(s) = s {
                    if let Ok(p) = serde_json::from_str::<Value>(&s) {
                        let w = p.get("w").and_then(Value::as_f64).unwrap_or(0.0);
                        let h = p.get("h").and_then(Value::as_f64).unwrap_or(0.0);
                        if w > 0.0 && h > 0.0 {
                            return (w, h);
                        }
                    }
                }
            }
        }
        (1920.0, 1080.0)
    }

    /// 第 152 轮:设置自动跟随期望态并立即应用到当前文档;导航后新文档自动重挂。
    pub async fn set_auto_follow(&self, page_id: &str, enabled: bool) -> bool {
        let Some(page) = self.page(page_id).await else {
            return false;
        };
        {
            let mut inner = self.inner.lock().await;
            inner.auto_follow = enabled;
        }
        super::browser_follow::set(&page, enabled).await
    }

    /// 导航/派生页后的跟随态 re-assert(与 guard 的 re-assert 同构;fail-open)。
    pub(crate) async fn reassert_auto_follow(&self, page: &chromiumoxide::Page) {
        let enabled = {
            let inner = self.inner.lock().await;
            if inner.connect_mode {
                false
            } else {
                inner.auto_follow
            }
        };
        if enabled {
            super::browser_follow::set(page, true).await;
        }
    }

    /// 蒙层是否处于激活态(第 141 轮,第 143 轮语义不变):locked 档 **且** headed。
    ///
    /// 供 control 层决定「输入动作先解后锁 / request_human 解锁」是否需要动 CDP;
    /// 未激活时这些路径零开销(不多发任何 CDP 往返)。connect 模式 guard 恒为
    /// open,天然返回 false。
    pub async fn overlay_active(&self) -> bool {
        let inner = self.inner.lock().await;
        inner.guard.mode == super::browser_overlay::PageGuardMode::Locked
            && matches!(inner.mode, BrowserMode::Headed)
    }

    /// 当前实例的页面管控期望态(第 143 轮)。
    pub async fn current_guard(&self) -> super::browser_overlay::PageGuardConfig {
        self.inner.lock().await.guard.clone()
    }

    /// 管控视觉层是否激活(第 143 轮):headed 且非 connect(= 注入了管控脚本,
    /// 截图避让 / 盾区挂起 / re-assert 需要动 JS 视觉)。hidden 与 connect 均 false。
    pub async fn guard_visuals_active(&self) -> bool {
        let inner = self.inner.lock().await;
        !inner.connect_mode && matches!(inner.mode, BrowserMode::Headed)
    }

    /// 运行时切换页面管控三档(第 143 轮,镜像 `set_highlight` 语义;第 155 轮起
    /// locked 档缺省 DOM 输入闸「可滚动观看」):
    /// 校验 + 归一化 → 更新实例期望态 → 立即对指定 page 应用
    /// (输入拦截按档:locked 优先 DOM 闸、不在场回退 CDP;JS 蒙层/状态条/盾区)。
    pub async fn set_guard(
        &self,
        page_id: &str,
        cfg: super::browser_overlay::PageGuardConfig,
    ) -> std::result::Result<Value, String> {
        let cfg = cfg.normalized();
        cfg.validate()?;
        let page = self
            .page(page_id)
            .await
            .ok_or_else(|| "page_id 不存在".to_string())?;
        {
            let mut inner = self.inner.lock().await;
            inner.guard = cfg.clone();
        }
        let visuals = self.guard_visuals_active().await;
        let applied = if visuals {
            super::browser_overlay::apply_page_guard(&page, &cfg).await?
        } else {
            json!({
                "mode": cfg.mode.as_str(),
                "input_locked": false,
                "visual_mask": false,
                "shields_active": false,
            })
        };
        Ok(json!({
            "mode": cfg.mode.as_str(),
            "allow_selectors": cfg.allow_selectors,
            "block_selectors": cfg.block_selectors,
            "note": cfg.note,
            "visuals_applied": visuals,
            "input_locked": applied["input_locked"],
            "scroll_watch": applied["scroll_watch"],
            "visual_mask": applied["visual_mask"],
            "shields_active": applied["shields_active"],
            "hint": "页面管控三档(第 143 轮):locked=屏蔽(蒙层+输入拦截,缺省;第 155 轮起人工可滚动观看,点击/键盘仍拦)/ open=非屏蔽(人工可直接操作)/ partial=部分屏蔽(盾区外人工可操作);运行时随时切换",
        }))
    }

    /// 运行时开关可视化蒙层(第 141 轮 legacy,第 143 轮起委托 `set_guard`):
    /// `enabled=true` → locked 档,`false` → open 档;partial 请用 `set_guard`。
    pub async fn set_overlay(
        &self,
        page_id: &str,
        enabled: bool,
    ) -> std::result::Result<Value, String> {
        let mut r = self
            .set_guard(page_id, super::browser_overlay::PageGuardConfig::legacy_overlay(enabled))
            .await?;
        if let Some(obj) = r.as_object_mut() {
            obj.insert("enabled".into(), json!(enabled));
            obj.insert(
                "deprecation".into(),
                json!("set_overlay 是第 141 轮 legacy 别名:true=set_guard(mode=locked),false=set_guard(mode=open);部分屏蔽请用 control(set_guard, mode=partial, …)"),
            );
        }
        Ok(r)
    }

    /// 通过页面触发一次下载,并等待 Browser 域事件给出最终落盘路径。
    ///
    /// 设计要点:
    /// - `Browser.setDownloadBehavior(allowAndName)` 统一指定下载目录并开启事件;
    /// - 先注册 `downloadWillBegin` / `downloadProgress` 监听,再用页面内 anchor 或
    ///   selector click 触发,避免事件竞态;
    /// - `guid` 贯穿 begin/progress,过滤并发浏览器下载中的无关事件;
    /// - `filename` 只作为最终重命名目标,先做 file_name 归一化,防止路径穿越。
    #[allow(clippy::too_many_arguments)]
    pub async fn download(
        &self,
        page_id: &str,
        url: Option<&str>,
        selector: Option<&str>,
        save_dir: Option<&str>,
        filename: Option<&str>,
        timeout_ms: u64,
    ) -> std::result::Result<Value, String> {
        const DEFAULT_DOWNLOAD_TIMEOUT_MS: u64 = 120_000;
        let timeout_ms = if timeout_ms == 0 {
            DEFAULT_DOWNLOAD_TIMEOUT_MS
        } else {
            timeout_ms.clamp(1_000, 300_000)
        };

        let base = if let Some(dir) = save_dir.filter(|s| !s.is_empty()) {
            PathBuf::from(dir)
        } else {
            std::env::current_dir().map_err(|e| format!("获取工作目录失败:{e}"))?
        };
        tokio::fs::create_dir_all(&base)
            .await
            .map_err(|e| format!("创建下载目录失败({}):{e}", base.display()))?;
        let base = tokio::fs::canonicalize(&base)
            .await
            .map_err(|e| format!("解析下载目录失败({}):{e}", base.display()))?;
        // 第 165 轮(2026-10-12 Windows 实测):canonicalize 在 Windows 返回 verbatim
        // 形态(`\\?\C:\...`),直接交给 Chrome `setDownloadBehavior(downloadPath=)`
        // 会让下载**必然** `state=Canceled, received=0` —— Chrome 建不出目标文件。
        // 交给外部进程的路径先剥离该前缀;进程内 Path 运算保持 verbatim 不受影响。
        let base = crate::agent::tools::strip_windows_verbatim(&base);

        // 在同一个临界区内完成:查找页面、配置下载行为、注册全局 Browser 事件流。
        // EventStream 与 Page 都是克隆句柄,离开临界区后仍可使用;触发动作前锁已释放。
        let (page, mut begin_stream, mut progress_stream) = {
            let inner = self.inner.lock().await;
            let Some(browser) = inner.browser.as_ref() else {
                return Err("浏览器会话不存在".into());
            };
            let Some(entry) = inner.pages.get(page_id) else {
                return Err("page_id 不存在".into());
            };
            let page = entry.page.clone();
            let params = SetDownloadBehaviorParams::builder()
                .behavior(SetDownloadBehaviorBehavior::AllowAndName)
                .download_path(base.to_string_lossy().to_string())
                .events_enabled(true)
                .build()
                .map_err(|e| e.to_string())?;
            browser
                .execute(params)
                .await
                .map_err(|e| format!("配置下载行为失败:{e}"))?;
            let begin_stream = browser
                .event_listener::<EventDownloadWillBegin>()
                .await
                .map_err(|e| format!("注册下载开始事件失败:{e}"))?;
            let progress_stream = browser
                .event_listener::<EventDownloadProgress>()
                .await
                .map_err(|e| format!("注册下载进度事件失败:{e}"))?;
            (page, begin_stream, progress_stream)
        };

        // 触发下载。优先点击页面既有链接;没有 selector 时注入临时 anchor,
        // 避免用 page.goto 下载 URL 导致 Chrome 以 net::ERR_ABORTED 结束导航。
        if let Some(selector) = selector.filter(|s| !s.is_empty()) {
            let element = page
                .find_element(selector.to_string())
                .await
                .map_err(|e| format!("查找下载元素失败:{e}"))?;
            element
                .click()
                .await
                .map_err(|e| format!("触发下载失败:{e}"))?;
        } else {
            let Some(url) = url.filter(|s| !s.is_empty()) else {
                return Err("缺少 url 或 selector".into());
            };
            let js = format!(
                r#"(() => {{
                    const a = document.createElement('a');
                    a.href = {url};
                    a.rel = 'noopener';
                    a.download = '';
                    a.style.display = 'none';
                    document.body.appendChild(a);
                    a.click();
                    a.remove();
                    return true;
                }})()"#,
                url = serde_json::to_string(url).unwrap_or_else(|_| "\"\"".into()),
            );
            page.evaluate(js.as_str())
                .await
                .map_err(|e| format!("触发下载失败:{e}"))?;
        }

        let started_at = std::time::Instant::now();
        let begin = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
            loop {
                match begin_stream.next().await {
                    Some(ev) => return ev,
                    None => {
                        return Arc::new(EventDownloadWillBegin {
                            frame_id: Default::default(),
                            guid: String::new(),
                            url: String::new(),
                            suggested_filename: String::new(),
                        })
                    }
                }
            }
        })
        .await
        .map_err(|_| format!("等待下载开始超时({timeout_ms}ms)"))?;
        if begin.guid.is_empty() {
            return Err("下载事件流已关闭".into());
        }

        let mut final_progress: Option<EventDownloadProgress> = None;
        let wait_deadline = std::time::Duration::from_millis(timeout_ms);
        let progress = tokio::time::timeout(wait_deadline, async {
            loop {
                match progress_stream.next().await {
                    Some(ev) if ev.guid == begin.guid => {
                        match ev.state {
                            chromiumoxide::cdp::browser_protocol::browser::DownloadProgressState::Completed
                            | chromiumoxide::cdp::browser_protocol::browser::DownloadProgressState::Canceled => {
                                return (*ev).clone()
                            }
                            _ => {
                                final_progress = Some((*ev).clone());
                            }
                        }
                    }
                    Some(_) => continue,
                    None => break,
                }
            }
            final_progress.take().unwrap_or(EventDownloadProgress {
                guid: begin.guid.clone(),
                total_bytes: 0.0,
                received_bytes: 0.0,
                state: chromiumoxide::cdp::browser_protocol::browser::DownloadProgressState::Canceled,
                file_path: None,
            })
        })
        .await
        .map_err(|_| format!("等待下载完成超时({timeout_ms}ms)"))?;

        if !matches!(
            progress.state,
            chromiumoxide::cdp::browser_protocol::browser::DownloadProgressState::Completed
        ) {
            return Err(format!(
                "下载未完成(state={:?}, received={}, total={})",
                progress.state, progress.received_bytes, progress.total_bytes
            ));
        }

        let source = progress
            .file_path
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| base.join(&begin.suggested_filename));
        if !source.exists() {
            return Err(format!(
                "Chrome 报告下载完成但文件不存在:{}",
                source.display()
            ));
        }

        let target = if let Some(filename) = filename.filter(|s| !s.is_empty()) {
            let safe_name = Path::new(filename)
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "." && *s != "..")
                .ok_or_else(|| format!("非法下载文件名:{filename}"))?;
            base.join(safe_name)
        } else {
            base.join(&begin.suggested_filename)
        };
        if source != target {
            tokio::fs::rename(&source, &target).await.map_err(|e| {
                format!(
                    "重命名下载文件失败({} → {}):{e}",
                    source.display(),
                    target.display()
                )
            })?;
        }
        let byte_size = tokio::fs::metadata(&target)
            .await
            .map_err(|e| format!("读取下载文件大小失败({}):{e}", target.display()))?
            .len();

        Ok(json!({
            "guid": begin.guid,
            "url": begin.url,
            "suggested_filename": begin.suggested_filename,
            "save_path": target,
            "byte_size": byte_size,
            "received_bytes": progress.received_bytes,
            "total_bytes": progress.total_bytes,
            "state": "completed",
            "elapsed_ms": started_at.elapsed().as_millis() as u64,
        }))
    }

    /// 页面标题(fail-open:取不到按空串处理,等价于「可能噪声」的最保守侧)。
    async fn page_title(page: &chromiumoxide::Page) -> String {
        page.evaluate("document.title")
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// 动作后 adopt 派生标签页:diff browser.pages() 与注册表,
    /// 未登记的 page target 注册为新 page_id 返回。
    ///
    /// 第 152 轮两处加固(实测:headed 模式下 HITL 收口后前台冒出一批空白/乱码标签页):
    ///
    /// 1. **噪声页直接关掉,不进注册表**(判定见 [`super::browser_noise`])。
    ///    原实现对未知 target 一律收编,收编 ≠ 关闭 —— 页面自己开的
    ///    `about:blank`、风控脚本 `window.open('')` 的空窗、下载导航残留
    ///    全部滞留在前台,越积越多。关闭前先 `sleep(250ms)` 复查一次 URL:
    ///    `target=_blank` 刚打开的窗口 URL 短暂就是 `about:blank`,不给这点
    ///    导航时间会把真页面误杀。
    /// 2. **connect 模式(`open(reuse_existing=true)`)完全不收编**。此时
    ///    `browser.pages()` 返回的是**用户浏览器里的全部标签页**,注册表只装了
    ///    laew 自己开的那几个;照旧逻辑会把用户的私人标签页一次性收编并注入
    ///    管控脚本,`spawned_page_id` 还会指向用户的私人页面。
    pub async fn adopt_spawned_pages(&self) -> Vec<String> {
        self.adopt_spawned_pages_ex().await.adopted
    }

    /// 收编浏览器里「有、注册表里没有」的标签页(2026-10-14 扩展版)。
    ///
    /// 扩展点(闲鱼实测,同一商品页被重复打开 7~8 次 / 单任务残留 12 个 tab):
    /// 1. **同 URL 去重** —— 新页与注册表内页 URL 身份键相同则**不收编、直接关掉**
    ///    (见 [`super::browser_noise::page_url_key`]);`?page=1` 与 `?page=2` 这类
    ///    合法分页键不同,不去重;
    /// 2. **Tab 总量上限** —— `LAEW_WEB_MAX_TABS`(默认 8),超限只丢弃**本次新候选**
    ///    (不关既有工作页);`LAEW_WEB_MAX_WINDOWS` 按 windowId 计额,同窗口多 Tab
    ///    不占额度,12 个 tab 一个上限都没触发,这里补上 tab 维度;
    /// 3. 回报 `deduped`/`dropped`,由工具层全量透出(旧实现只回 `spawned.first()` 一个,
    ///    Agent 不知道有堆积)。
    pub async fn adopt_spawned_pages_ex(&self) -> AdoptOutcome {
        // 第 180 轮:三段式改排(快照-武装-登记)—— 原「全程持锁」把噪声复查的
        // 500ms 等待与逐页武装(含可能阻塞在 provisional 导航上的 UA 探测,实测
        // ~1.4s)全部压在管理锁上,既饿死派生页早期武装 watcher(`spawn_arm_resolve`
        // 排队等锁),也把并行的其它页面操作串在后面(与 `list_pages` 的
        // 「快照-探测-清理三段式」同一纪律)。语义保持:分类在锁内快照上完成,
        // 登记回到锁内并加 target 去重守卫,并行 adopt 不会重复登记同一 target。
        let mut outcome = AdoptOutcome::default();
        // ---------- 阶段 1(锁内):扫描 + 分类基准 + 期望态快照 ----------
        let (pages, known, mut known_keys, cfg, pages_len) = {
            let inner = self.inner.lock().await;
            let Some(browser) = inner.browser.as_ref() else {
                return outcome;
            };
            if inner.connect_mode {
                // 接管模式:浏览器归用户所有,laew 不认领也不关闭任何未知页面。
                return outcome;
            }
            let Ok(pages) = browser.pages().await else {
                return outcome;
            };
            let known: std::collections::HashSet<String> =
                inner.pages.values().map(|e| e.target_id.clone()).collect();
            // 注册表内已知页的 URL 身份键(同页去重基准)。fail-open:取不到 URL 跳过。
            let mut known_keys: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for entry in inner.pages.values() {
                if let Ok(Some(u)) = entry.page.url().await {
                    if let Some(k) = super::browser_noise::page_url_key(&u) {
                        known_keys.insert(k);
                    }
                }
            }
            let cfg = inner.spawn_arm_config();
            (pages, known, known_keys, cfg, inner.pages.len())
        };
        let mut candidates: Vec<chromiumoxide::Page> = Vec::new();
        let mut noise: Vec<chromiumoxide::Page> = Vec::new();
        for page in pages {
            let tid: String = page.target_id().clone().into();
            if known.contains(&tid) {
                continue;
            }
            // 噪声判定:URL + 标题;标题非空即豁免(见 is_noise_page)
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = Self::page_title(&page).await;
            if super::browser_noise::is_noise_page(&url, &title) {
                noise.push(page);
                continue;
            }
            // 同 URL 去重:与注册表内某页同身份 → 不收编,直接关(重复打开的商品页)。
            if let Some(k) = super::browser_noise::page_url_key(&url) {
                if known_keys.contains(&k) {
                    let _ = page.close().await;
                    outcome.deduped.push(url);
                    continue;
                }
                known_keys.insert(k);
            }
            candidates.push(page);
        }
        // ---------- 阶段 2(锁外):噪声复查 + 逐页武装(全部 CDP) ----------
        // 给刚打开的窗口一次导航时间再复查,避免误杀 `target=_blank` 真页面。
        // 2026-10-12 闲鱼实测修正:原条件 `!candidates.is_empty()` 导致「只有新标签页
        // 一个候选」时不等待 —— 而点击商品链接打开的新 tab 初始正是新标签页,
        // 等它导航到真实 URL 后才是该 adopt 的页面。改为**只要有噪声页就等**,
        // 500ms 兜底慢导航;复查后仍是噪声才关闭。
        if !noise.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let mut still_noise = Vec::new();
            for page in noise {
                let url = page.url().await.ok().flatten().unwrap_or_default();
                let title = Self::page_title(&page).await;
                if super::browser_noise::is_noise_page(&url, &title) {
                    still_noise.push(page);
                } else {
                    // 复查后是真页面:同 URL 去重同样适用
                    if let Some(k) = super::browser_noise::page_url_key(&url) {
                        if known_keys.contains(&k) {
                            let _ = page.close().await;
                            outcome.deduped.push(url);
                            continue;
                        }
                        known_keys.insert(k);
                    }
                    candidates.push(page);
                }
            }
            noise = still_noise;
        }
        if !noise.is_empty() {
            let n = noise.len();
            for page in noise {
                let _ = page.close().await;
            }
            tracing::info!(closed = n, "MCP_Web_Use 关闭空白/内部噪声标签页(不收编)");
        }
        // 2026-10-12:多候选时 http/https 优先 —— `spawned.first()` 取第一个,
        // 若候选里混着 about:blank 等内部页,可能回传错误 page_id(实测「点击按钮
        // 打开新页面后 Agent 失去控制」的次生形态)。按 URL _scheme 排序,真实页面在前。
        if candidates.len() > 1 {
            let mut scored: Vec<(bool, chromiumoxide::Page)> = Vec::with_capacity(candidates.len());
            for page in candidates {
                let is_http = page
                    .url()
                    .await
                    .ok()
                    .flatten()
                    .map(|u| u.starts_with("http://") || u.starts_with("https://"))
                    .unwrap_or(false);
                scored.push((is_http, page));
            }
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            candidates = scored.into_iter().map(|(_, p)| p).collect();
        }
        // Tab 总量上限(2026-10-14):只丢弃**本次新候选**的超额部分,不碰既有工作页。
        // `LAEW_WEB_MAX_WINDOWS` 按 windowId 计额(同窗口多 Tab 不占额度),实测 12 个
        // tab 一个上限都没触发,这里补 tab 维度。基准取阶段 1 快照(并行 adopt 的
        // 登记由阶段 3 的去重守卫兜底,上限为建议性)。
        let max_tabs = max_tabs_from_env();
        while pages_len + outcome.adopted.len() + candidates.len() > max_tabs {
            if let Some(extra) = candidates.pop() {
                let url = extra.url().await.ok().flatten().unwrap_or_default();
                let _ = extra.close().await;
                outcome.dropped.push(url);
            } else {
                break;
            }
        }
        let mut armed: Vec<(chromiumoxide::Page, String, EventBuffer)> = Vec::new();
        for page in candidates {
            let tid: String = page.target_id().clone().into();
            let events = EventBuffer::default();
            let _ = page
                .execute(chromiumoxide::cdp::js_protocol::runtime::EnableParams::default())
                .await;
            let _ = page
                .execute(chromiumoxide::cdp::browser_protocol::network::EnableParams::default())
                .await;
            spawn_event_listeners(&page, events.clone()).await;
            // 第 169 轮:派生/adopt 新标签页同样装隐身覆写(connect 零注入不变)。
            // 第 174 轮:UA 清理同挂(无头档派生页同样不能带 HeadlessChrome 令牌)。
            // 第 180 轮:补 screen_sync —— 无头档派生页的 init 链覆盖不带 screen
            // 字段,不补则 `outer > screen` 物理不可能硬判据常驻(watcher 早期武装
            // 之外的第二道防线;两处幂等,先到后到零冲突)。
            if !cfg.connect_mode {
                super::browser_ns::apply_ua_destealth(&page, cfg.stealth).await;
                super::browser_ns::inject_stealth(&page, cfg.stealth).await;
                apply_screen_sync_override(&page, cfg.screen_sync).await;
                // 第 182 轮:派生/adopt 新标签页同挂指针/触摸环境仿真(与早期武装
                // watcher 同语义,两处幂等)。
                super::browser_env_emulation::apply_pointer_env_emulation(&page, cfg.stealth, cfg.mode).await;
            }
            // 第 100 轮:headed + highlight 时,派生/adopt 的新标签页同样注入蓝框。
            if cfg.highlight && matches!(cfg.mode, BrowserMode::Headed) {
                inject_agent_highlight(&page).await;
            }
            // 第 143 轮:管控同理,派生新标签页按实例期望态注入 + 应用(三档)。
            if !cfg.connect_mode && matches!(cfg.mode, BrowserMode::Headed) {
                super::browser_overlay::inject_agent_guard(&page, &cfg.guard).await;
                let _ = super::browser_overlay::apply_page_guard(&page, &cfg.guard).await;
            }
            // 第 152 轮:派生页同样按实例期望态挂上跟随器(否则新标签页不跟随)
            if !cfg.connect_mode && cfg.auto_follow {
                super::browser_follow::install(&page, true).await;
                let _ = super::browser_follow::set(&page, true).await;
            }
            armed.push((page, tid, events));
        }
        // ---------- 阶段 3(锁内):登记注册表(target 去重守卫) ----------
        {
            let mut inner = self.inner.lock().await;
            for (page, tid, events) in armed {
                // 并行 adopt/自愈交错守卫:该 target 已被登记则跳过,不产生重复 page_id。
                if inner.pages.values().any(|e| e.target_id == tid) {
                    continue;
                }
                let id = new_page_id();
                inner.pages.insert(
                    id.clone(),
                    PageEntry {
                        page,
                        target_id: tid,
                        created_at: now_compact(),
                        events,
                    },
                );
                outcome.adopted.push(id);
            }
        }
        outcome
    }

    /// 关闭页面;最后一个页面关闭时回收 launch 模式浏览器进程。
    /// 返回 false 表示 page_id 不存在(幂等语义,工具层映射 2000)。
    pub async fn close_page(&self, page_id: &str) -> bool {
        let mut inner = self.inner.lock().await;
        let Some(entry) = inner.pages.remove(page_id) else {
            return false;
        };
        // F2:可见区记账随页销毁。
        inner.page_visible_box.remove(page_id);
        // 2026-10-16:有界关闭 —— 卡死 target 的 page.close() 会排队在 wedged
        // handler 上永久挂起;注册表已摘除,句柄关不掉由 close(all)/watchdog
        // 的进程级回收兜底。
        let _ = super::browser_cdp::cdp_with_timeout("close_page", entry.page.close()).await;
        if inner.pages.is_empty() {
            if !inner.connect_mode {
                if let Some(mut browser) = inner.browser.take() {
                    // 2026-10-16:browser.close() 同样有界(handler 卡死时排队挂起),
                    // 失败由下方进程级 kill 兜底。
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        browser.close(),
                    )
                    .await;
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        browser.kill(),
                    )
                    .await;
                }
                // 清理一次性 user-data-dir(Chrome 退出可能有几百 ms 延迟,重试几次)
                if let Some(dir) = inner.user_data_dir.take() {
                    // 第 127 轮:graceful 路径已回收,清除同步清理登记,
                    // 防 atexit/panic 路径重复动作与 PID 复用误杀。
                    cleanup_registry::clear();
                    spawn_tempdir_cleanup(dir);
                }
            }
            if let Some(handler) = inner.handler.take() {
                handler.abort();
            }
            if let Some(watcher) = inner.spawn_arm.take() {
                watcher.abort();
            }
            if let Some(child) = inner.watchdog.take() {
                spawn_watchdog_reaper(child);
            }
            inner.connect_mode = false;
        }
        true
    }

    /// 关闭注册表内 URL 恰为 `about:blank` 的页面(2026-10-14,猎聘整页抹除实测)。
    ///
    /// 被站点脚本整页抹除的页面是死页面——网络证据已在此前 open/navigate 响应的
    /// `page_health` 里透出,无需保留句柄;不清会随每次 open 新开页堆积(实测单任务
    /// 残留 6 个 about:blank 窗口,占 `LAEW_WEB_MAX_WINDOWS` 额度又污染屏幕)。
    /// connect 模式零动作(页面归用户所有);URL 查询失败的页面保守保留(fail-open)。
    /// 走 [`Self::close_page`] 复用「最后一页关闭 → 回收浏览器」语义:blank 页恰为
    /// 全部页面时自然触发全新 profile 重启,正是整页抹除的恢复路径。
    pub async fn close_blank_pages(&self) -> usize {
        // 2026-10-16:锁内只取快照(connect 判定 + Page 克隆),CDP 探测放到锁外
        // 有界执行 —— 撞上卡死 handler 时按「非空白页」保守保留(fail-open),
        // 绝不持全局锁跨无界 await(单页卡死 → 全局死锁的级联面,见 browser_cdp)。
        let snapshot: Vec<(String, chromiumoxide::Page)> = {
            let inner = self.inner.lock().await;
            if inner.connect_mode {
                return 0;
            }
            inner
                .pages
                .iter()
                .map(|(id, entry)| (id.clone(), entry.page.clone()))
                .collect()
        };
        let mut candidates: Vec<String> = Vec::new();
        for (id, page) in &snapshot {
            if let Some(Some(url)) =
                super::browser_cdp::cdp_with_timeout("close_blank_pages.url", page.url()).await
            {
                if url.trim().eq_ignore_ascii_case("about:blank") {
                    candidates.push(id.clone());
                }
            }
        }
        if candidates.is_empty() {
            return 0;
        }
        let closed = candidates.len();
        tracing::info!(
            closed,
            "MCP_Web_Use 清扫整页抹除遗留的 about:blank 空白页面(死页面,证据已透出)"
        );
        for id in candidates {
            self.close_page(&id).await;
        }
        closed
    }

    /// 主动关闭所有页面 + 浏览器进程 + handler + 清理 tempdir。
    ///
    /// 幂等:重复调用安全(首次调用后 inner.browser=None,后续为 no-op)。
    /// 供 main 退出 / 任务完成后 / atexit 兜底调用,确保 Chrome 子进程不泄漏。
    ///
    /// 设计(第 78 轮,2026-09-17):解决孤儿 Chrome 进程泄漏问题。
    /// 任务完成或进程退出时,laew 进程终止但 Chrome 子进程变孤儿(reparented to init),
    /// 本次新增 shutdown 统一回收路径,配合 cleanup_sync() 多层防御。
    pub async fn shutdown(&self) {
        let mut inner = self.inner.lock().await;
        // 关闭所有注册页面(存活性已不重要,全部 drain)。
        if !inner.pages.is_empty() {
            let pages: Vec<PageEntry> = inner.pages.drain().map(|(_, e)| e).collect();
            for entry in pages {
                let _ = entry.page.close().await;
            }
        }
        // launch 模式才拥有浏览器进程所有权;connect 模式接管外部浏览器,不关闭。
        if !inner.connect_mode {
            if let Some(mut browser) = inner.browser.take() {
                // browser.close() 发送 CDP Browser.close,等待浏览器退出。
                // 超时 5s 兜底,避免清理挂起。
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    browser.close(),
                )
                .await;
                // Browser.close 只发 CDP 指令；极端卡死的浏览器还需要进程级兜底。
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    browser.kill(),
                )
                .await;
            }
            // 清理一次性 user-data-dir。
            if let Some(dir) = inner.user_data_dir.take() {
                // 第 127 轮:graceful 路径已回收,清除同步清理登记,
                // 防 atexit/panic 路径重复动作与 PID 复用误杀。
                cleanup_registry::clear();
                spawn_tempdir_cleanup(dir);
            }
        }
        if let Some(handler) = inner.handler.take() {
            handler.abort();
        }
        if let Some(watcher) = inner.spawn_arm.take() {
            watcher.abort();
        }
        if let Some(child) = inner.watchdog.take() {
            spawn_watchdog_reaper(child);
        }
        inner.connect_mode = false;
        inner.connected_port = None;
        inner.mode = BrowserMode::Hidden;
        inner.highlight = true;
        inner.guard = super::browser_overlay::PageGuardConfig::from_env_default();
    }

    /// 当前 connect 实例是否 laew 自起的 auto_relaunch 调试实例(2026-10-14
    /// 第二轮)。它是 laew 身份(CDP 拉起 + 复制 profile):指纹级打标站点对它
    /// 的抹除计入 `browser_pace` 总账,close(page_id="all") 时对它有生杀权;
    /// 用户日常 Chrome(真实指纹)两个判定均不成立。
    pub async fn connect_is_laew_relaunch(&self) -> bool {
        let inner = self.inner.lock().await;
        inner.connect_mode
            && inner
                .connected_port
                .is_some_and(crate::agent::browser_reuse::is_laew_relaunch_port)
    }

    /// 同步清理入口(供 atexit / panic hook / signal handler 调用)。
    ///
    /// **第 127 轮(2026-09-23)根治:彻底去 tokio 化,零 runtime、零 block_on。**
    ///
    /// 旧实现「新建 current-thread Runtime + block_on(shutdown())」在 macOS 上
    /// 必然崩溃:`exit()` 流程中主线程 TLS 析构(含 tokio CONTEXT thread-local)
    /// 先于 atexit handler 执行,`Runtime::block_on → Handle::enter` 因 TLS 已销毁
    /// panic(`THREAD_LOCAL_DESTROYED_ERROR`,tokio handle.rs:90:25);panic hook
    /// 路径下该 panic 属于 panic-during-panic,std 无条件 abort(catch_unwind 形同
    /// 虚设,abort 发生在展开之前),并连带 CrashReport 永远写不出来。本机最小复现
    /// 见 `tmpPlan/2026-09-23_07-TUI任务卡住后退出panic双重崩溃根治方案.md` §2.1。
    ///
    /// 现实现:纯 std/libc 同步原语 —— 杀浏览器主进程 PID + 删一次性 profile。
    /// 浏览器兜底回收仍由进程外 watchdog(pipe EOF 感知父死)覆盖,本函数只是
    /// 进程内提前清理的冗余防线,无需优雅 CDP 关停。
    pub fn cleanup_sync() {
        // 快速路径:未启动过浏览器直接返回,零开销。
        if !browser_started_flag::is_started() {
            return;
        }
        // 防御兜底:清理过程绝不外抛 panic(atexit / panic hook 场景任何 panic
        // 都可能升级为 panic-during-panic abort)。
        let _ = std::panic::catch_unwind(Self::cleanup_sync_inner);
    }

    /// [`Self::cleanup_sync`] 的实际清理体(纯同步,禁止引入 tokio 依赖)。
    fn cleanup_sync_inner() {
        // 取后即清,防止进程内残留:一来幂等(重复调用变 no-op),
        // 二来浏览器死后 PID 可能被 OS 复用,清零可避免误杀无关进程。
        let pid = cleanup_registry::take_browser_pid();
        if pid != 0 {
            super::browser_watchdog::terminate_process(pid);
        }
        if let Some(dir) = cleanup_registry::take_user_data_dir() {
            super::browser_watchdog::remove_profile(&dir);
        }
    }
}

/// 浏览器启动标记 flag。
///
/// BrowserManager 首次启动浏览器时置 true,供 cleanup_sync() 快速判断
/// "是否曾启动过浏览器",避免无意义创建 Runtime。
/// 使用 AtomicBool + SeqCst 保证跨线程可见。
mod browser_started_flag {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STARTED: AtomicBool = AtomicBool::new(false);
    pub fn set_started() {
        STARTED.store(true, Ordering::SeqCst);
    }
    pub fn is_started() -> bool {
        STARTED.load(Ordering::SeqCst)
    }
}

/// 同步清理登记簿(第 127 轮,2026-09-23)。
///
/// 记录 launch 模式拥有的浏览器主进程 PID 与一次性 profile 目录,供
/// `cleanup_sync()`(atexit / panic hook 路径)**在不创建任何 tokio runtime 的
/// 前提下**完成同步回收。为什么不用 BrowserManager.inner(tokio Mutex)?
/// - tokio Mutex 的 `lock()` 是 async,同步上下文拿不到;`blocking_lock()` 在
///   锁被活跃任务持有时会永久阻塞退出路径 —— atexit 场景不可接受;
/// - 无锁原子 + std Mutex(`try_lock` 失败即跳过)在 TLS 已销毁的线程上依然安全。
///
/// 登记时机:`launch()` 成功持有 browser 后;清除时机:graceful `shutdown()` /
/// `close_page` 末页回收(异步路径已优雅关停,同步清理无需重复动作)/
/// `cleanup_sync_inner` 取后即清。
mod cleanup_registry {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    /// 浏览器主进程 PID(0 = 未登记)。launch 模式才拥有进程所有权,connect
    /// 模式接管的外部浏览器永不登记。
    static BROWSER_PID: AtomicU32 = AtomicU32::new(0);

    /// 一次性 user-data-dir。std Mutex 允许在任意线程同步访问;poison 状态
    /// (持有线程 panic)直接放弃目录清理 —— 退出路径宁留目录不可挂死/panic,
    /// 且 watchdog 子进程仍会兜底删除。
    static USER_DATA_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

    pub fn register(pid: u32, dir: PathBuf) {
        BROWSER_PID.store(pid, Ordering::SeqCst);
        if let Ok(mut slot) = USER_DATA_DIR.lock() {
            *slot = Some(dir);
        }
    }

    /// 清除登记(graceful 关闭路径调用,防止同步清理重复动作/误杀 PID 复用)。
    pub fn clear() {
        BROWSER_PID.store(0, Ordering::SeqCst);
        if let Ok(mut slot) = USER_DATA_DIR.lock() {
            *slot = None;
        }
    }

    /// 取出并清零浏览器 PID(0 = 无)。
    pub fn take_browser_pid() -> u32 {
        BROWSER_PID.swap(0, Ordering::SeqCst)
    }

    /// 窥视浏览器 PID(0 = 未登记)。不消费登记 —— 焦点治理「抬升不抢焦」
    /// 只读定位 Chrome 主窗口用。connect 模式恒未登记(接管外部浏览器)。
    pub fn peek_browser_pid() -> u32 {
        BROWSER_PID.load(Ordering::SeqCst)
    }

    /// 取出 profile 目录(锁被占用 / 未登记 → None,跳过目录清理)。
    pub fn take_user_data_dir() -> Option<PathBuf> {
        USER_DATA_DIR.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// 窥视 launch 模式浏览器主进程 PID(0 = 未登记/connect 模式)。
/// 焦点治理「抬升不抢焦」(`browser_focus::raise_window_no_activate`)定位
/// Chrome 主窗口用;不消费登记,不影响 shutdown 清理路径。
pub fn peek_browser_pid() -> u32 {
    cleanup_registry::peek_browser_pid()
}

/// 生成 user-data-dir 清理辅助:spawn 异步任务重试删除。
/// Chrome 退出有几百 ms 延迟(写 SingletonLock 等),重试 10 次、每次 300ms。
fn spawn_tempdir_cleanup(dir: PathBuf) {
    tokio::spawn(async move {
        for _ in 0..10 {
            if std::fs::remove_dir_all(&dir).is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    });
}

/// 把 Browser 主进程 PID 交给进程外 watchdog。stdin pipe 保持打开；
/// laew 消失后 pipe EOF 是 kill -9 也无法绕过的退出事实。
pub(crate) fn spawn_parent_watchdog(
    browser_pid: u32,
    user_data_dir: &Path,
) -> Option<std::process::Child> {
    // 正式进程是 laew 本体；集成测试运行在 test harness 里，通过 env 指向 laew
    // binary，保证 BrowserManager 的真实生命周期路径也能被自动化验证。
    let exe = std::env::var_os("LAEW_BROWSER_WATCHDOG_EXE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())?;
    let mut command = std::process::Command::new(exe);
    command
        .arg(super::browser_watchdog::WATCHDOG_COMMAND)
        .arg(browser_pid.to_string())
        .arg(user_data_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // 独立进程组避免终端 Ctrl-C 同步终止 watchdog；父进程真正退出仍会关闭 pipe。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    command.spawn().ok()
}

/// 正常关闭路径收割 watchdog；浏览器退出后它通常已自行返回。
fn spawn_watchdog_reaper(mut child: std::process::Child) {
    tokio::spawn(async move {
        for _ in 0..10 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let _ = child.kill();
        let _ = child.wait();
    });
}

/// 新版本每个 launch profile 都带 owner marker。发现 owner 已不存在时只清理
/// 自己命名空间下的目录，不扫描/不杀无关进程；活跃 owner（并行 laew）跳过。
fn cleanup_stale_profiles() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let is_laew_profile = dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("laew_browser_"));
        if !dir.is_dir() || !is_laew_profile {
            continue;
        }
        let owner = std::fs::read_to_string(dir.join(".laew-owner"))
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok());
        if owner.map_or(true, |pid| !super::browser_watchdog::process_alive(pid)) {
            for _ in 0..3 {
                if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

/// 清理旧版本（无 marker/watchdog）遗留的 laew 专用 Chrome 进程。
///
/// 只匹配 `--user-data-dir=<tmp>/laew_browser_*` 参数；不扫描普通用户浏览器。
#[cfg(unix)]
fn cleanup_legacy_orphans() {
    let Ok(output) = std::process::Command::new("ps")
        .arg("-axo")
        .arg("pid=,command=")
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for pid in legacy_orphan_pids(&text) {
        if let Ok(raw) = i32::try_from(pid) {
            unsafe {
                libc::kill(raw, libc::SIGTERM);
            }
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    for pid in legacy_orphan_pids(&text) {
        if let Ok(raw) = i32::try_from(pid) {
            unsafe {
                libc::kill(raw, libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
fn legacy_orphan_pids(ps_output: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    for line in ps_output.lines() {
        let Some((pid_text, command)) = line.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid_text.trim().parse::<u32>() else {
            continue;
        };
        let owns_laew_profile = command.split_whitespace().any(|arg| {
            arg.strip_prefix("--user-data-dir=")
                .map(Path::new)
                .and_then(Path::file_name)
                .is_some_and(|name| name.to_string_lossy().starts_with("laew_browser_"))
        });
        if !owns_laew_profile {
            continue;
        }
        // 新版本 profile 有 owner；活跃并行 laew 不允许被本次启动误杀。
        let profile_path = command
            .split_whitespace()
            .find_map(|arg| arg.strip_prefix("--user-data-dir=").map(Path::new));
        let owner_alive = profile_path
            .and_then(|dir| std::fs::read_to_string(dir.join(".laew-owner")).ok())
            .and_then(|text| text.trim().parse::<u32>().ok())
            .is_some_and(super::browser_watchdog::process_alive);
        if !owner_alive {
            pids.push(pid);
        }
    }
    pids
}

/// profile owner marker。watchdog 负责杀进程，marker 负责下次启动清理空目录。
fn write_profile_owner(dir: &Path) {
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(dir.join(".laew-owner"), std::process::id().to_string());
    }
}

/// 挂 Console / Network 事件监听任务(chromiumoxide EventStream → 环形缓冲)。
async fn spawn_event_listeners(page: &Page, events: EventBuffer) {
    use chromiumoxide::cdp::browser_protocol::network;
    use chromiumoxide::cdp::js_protocol::runtime;

    if let Ok(mut stream) = page
        .event_listener::<runtime::EventConsoleApiCalled>()
        .await
    {
        let ev = events.clone();
        tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                let text = event
                    .args
                    .iter()
                    .filter_map(|a| a.value.as_ref().map(|v| v.to_string()))
                    .collect::<Vec<_>>()
                    .join(" ");
                ev.push_console(json!({
                    "level": format!("{:?}", event.r#type),
                    "text": text,
                }));
            }
        });
    }

    if let Ok(mut stream) = page
        .event_listener::<network::EventRequestWillBeSent>()
        .await
    {
        let ev = events.clone();
        tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                ev.push_network(json!({
                    "phase": "request",
                    "url": event.request.url,
                    "method": event.request.method,
                }));
            }
        });
    }

    if let Ok(mut stream) = page
        .event_listener::<network::EventResponseReceived>()
        .await
    {
        let ev = events.clone();
        tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                ev.push_network(json!({
                    "phase": "response",
                    "url": event.response.url,
                    "status": event.response.status,
                    "mime_type": event.response.mime_type,
                }));
            }
        });
    }

    // 第 159 轮:loadingFailed 采集 —— 空白页诊断的关键证据。
    // Chrome 154 Local Network Access 拦「公网域名→私网 IP」子资源时只发
    // loadingFailed(blocked_reason=local-network-access),不发 response;
    // 旧实现只采 request/response,Agent 面对 WAF 空白页只能猜「服务端返回空」。
    if let Ok(mut stream) = page
        .event_listener::<network::EventLoadingFailed>()
        .await
    {
        let ev = events;
        tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                ev.push_network(json!({
                    "phase": "loading_failed",
                    "request_id": event.request_id,
                    "type": format!("{:?}", event.r#type),
                    "error_text": event.error_text,
                    "blocked_reason": event
                        .blocked_reason
                        .clone()
                        .map(|b| format!("{b:?}"))
                        .unwrap_or_default(),
                    "canceled": event.canceled.unwrap_or(false),
                }));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== 第 170 轮:僵尸句柄清尸(drop_dead_browser) =====

    #[tokio::test]
    async fn drop_dead_browser_no_browser_is_noop() {
        // 无浏览器实例时:零副作用返回 false(不 panic、不误清)。
        // 全局单例可能被同进程其它用例触碰,故只在确认为无实例时断言。
        let mgr = BrowserManager::global();
        if mgr.has_browser().await {
            return; // 并行用例持有实例,本用例让路(不断言以免互相干扰)
        }
        assert!(!mgr.drop_dead_browser().await);
    }

    #[test]
    fn page_id_format() {
        let id = new_page_id();
        assert!(id.starts_with("p_"));
        assert_eq!(id.len(), 2 + 8);
        assert!(id[2..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ===== 第 127 轮:cleanup_sync 去 tokio 化与同步清理登记簿 =====

    /// 登记簿是进程级全局,并发用例会互相改写登记值 —— 串行化涉及用例。
    static REGISTRY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn cleanup_registry_roundtrip() {
        let _guard = REGISTRY_TEST_LOCK.lock().expect("lock");
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = dir.path().join("profile");
        std::fs::create_dir_all(&profile).expect("create profile");

        cleanup_registry::register(12345, profile.clone());
        assert_eq!(cleanup_registry::take_browser_pid(), 12345);
        // 取后即清:第二次读取为空(幂等,防 PID 复用误杀)。
        assert_eq!(cleanup_registry::take_browser_pid(), 0);
        assert_eq!(
            cleanup_registry::take_user_data_dir(),
            Some(profile.clone())
        );
        assert_eq!(cleanup_registry::take_user_data_dir(), None);
        // 未登记时 clear() 安全无害。
        cleanup_registry::clear();
        assert_eq!(cleanup_registry::take_browser_pid(), 0);
    }

    #[test]
    fn cleanup_sync_is_panic_free_and_removes_registered_profile() {
        let _guard = REGISTRY_TEST_LOCK.lock().expect("lock");
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = dir.path().join("laew_profile");
        std::fs::create_dir_all(profile.join("Default")).expect("create profile");
        std::fs::write(profile.join("marker"), "x").expect("write marker");

        // 登记一个不可能存在的 PID(远超各平台 pid_max):terminate_process
        // 对不存在进程是 ESRCH no-op,绝不误杀真实进程。
        cleanup_registry::register(4_000_000_000, profile.clone());
        browser_started_flag::set_started();

        // 无 tokio runtime 的裸线程上执行(atexit / panic hook 同款环境),
        // 断言零 panic 且登记的 profile 目录被同步删除。
        let joined = std::thread::spawn(move || {
            BrowserManager::cleanup_sync();
            BrowserManager::cleanup_sync(); // 幂等:第二次为 no-op
        })
        .join();
        assert!(joined.is_ok(), "cleanup_sync 在裸线程上不得 panic");
        assert!(
            !profile.exists(),
            "登记的一次性 profile 应被同步清理删除"
        );
    }

    #[test]
    fn terminate_process_noop_on_impossible_pid() {
        // pid=4_000_000_000 超出 Linux(默认 4194304)/macOS(99998)pid_max,
        // 不可能存活:SIGTERM ESRCH → process_alive=false → 立即返回(无重试循环)。
        let start = std::time::Instant::now();
        super::super::browser_watchdog::terminate_process(4_000_000_000);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(3),
            "对不存在 PID 不应进入 SIGTERM 重试轮询"
        );
    }

    // ===== 第 125 轮:视口基准 1080p 与 2K 自动扩展决策 =====

    #[test]
    fn default_window_is_1080p() {
        // 全模式(含 hidden)统一 1080p 起步,替代旧 hidden=1440×900
        assert_eq!(default_window_size(), (1920, 1080));
        assert_eq!((DEFAULT_WINDOW_W, DEFAULT_WINDOW_H), (1920, 1080));
    }

    #[test]
    fn viewport_fit_plan_no_overflow() {
        // 内容 == 视口 / 容差内(≤2px)不扩展
        assert_eq!(viewport_fit_plan(1920.0, 1080.0, 1920.0, 1080.0, 2560, 1440), None);
        assert_eq!(viewport_fit_plan(1920.0, 1080.0, 1922.0, 1082.0, 2560, 1440), None);
    }

    #[test]
    fn viewport_fit_plan_horizontal_clipped() {
        // 横向被裁(经典后台 min-width 2200):宽撑到内容宽,高保持
        assert_eq!(
            viewport_fit_plan(1920.0, 1080.0, 2200.0, 1080.0, 2560, 1440),
            Some((2200, 1080))
        );
    }

    #[test]
    fn viewport_fit_plan_vertical_short_content() {
        // 纵向内容略高于视口且在 2K 高内:一并撑高(截图不缺区域)
        assert_eq!(
            viewport_fit_plan(1920.0, 1080.0, 1920.0, 1200.0, 2560, 1440),
            Some((1920, 1200))
        );
    }

    #[test]
    fn viewport_fit_plan_clamps_to_2k_cap() {
        // 超 2K 内容截到上限并交由 clamped 提示(full_page / 手动 set_viewport)
        assert_eq!(
            viewport_fit_plan(1920.0, 1080.0, 4000.0, 5000.0, 2560, 1440),
            Some((2560, 1440))
        );
        assert_eq!(
            viewport_fit_plan(1920.0, 1080.0, 4000.0, 1080.0, 2560, 1440),
            Some((2560, 1080))
        );
    }

    #[test]
    fn viewport_fit_plan_never_shrinks_or_exceeds_cap_when_larger() {
        // 用户自定义超大窗口(3840 宽 ≥ cap):维持现状不缩也不报错
        assert_eq!(viewport_fit_plan(3840.0, 1080.0, 4200.0, 1080.0, 2560, 1440), None);
        // 亚像素内容尺寸向上取整,绝不小于当前
        assert_eq!(
            viewport_fit_plan(1920.0, 1080.0, 1925.6, 1080.0, 2560, 1440),
            Some((1926, 1080))
        );
    }

    #[test]
    fn viewport_fit_plan_degenerate_zero() {
        // 异常测量(0 值)不应恐慌性撑满
        assert_eq!(viewport_fit_plan(0.0, 0.0, 0.0, 0.0, 2560, 1440), None);
    }

    #[cfg(unix)]
    #[test]
    fn legacy_orphan_parser_scopes_to_laew_profiles_and_live_owner() {
        let live = tempfile::tempdir().unwrap();
        std::fs::write(
            live.path().join(".laew-owner"),
            std::process::id().to_string(),
        )
        .unwrap();
        let ps = format!(
            "  123 /chrome --user-data-dir=/tmp/laew_browser_dead --x\n \
             456 /other-browser --user-data-dir=/tmp/not_laew_profile\n \
             789 /chrome --user-data-dir={} --x\n",
            live.path().display()
        );
        let pids = legacy_orphan_pids(&ps);
        assert_eq!(pids, vec![123]);
    }

    #[test]
    fn event_buffer_ring_cap() {
        let buf = EventBuffer::default();
        for i in 0..600 {
            buf.push_console(json!({"i": i}));
        }
        let q = buf.console.lock().unwrap();
        assert_eq!(q.len(), EVENT_RING_CAP);
        // 最早 100 条被挤出,第一条应是 i=100
        assert_eq!(q.front().unwrap().data["i"], json!(100));
    }

    #[test]
    fn event_buffer_health() {
        let buf = EventBuffer::default();
        // 无事件时视为健康(页面可能本来就安静)
        assert!(buf.collection_healthy().0);
        buf.push_network(json!({"url": "https://example.com"}));
        let (healthy, last) = buf.collection_healthy();
        assert!(healthy);
        assert!(last.is_some());
    }

    // 2026-09-17 第 74 轮:BrowserMode 三档枚举测试。
    // ★ 2026-10-09 第 139 轮:模式决策与 env 解析整体搬到 `super::browser_mode`,
    // 对应单测同步搬到 `browser_mode::tests`(默认语义也改为「可见模式」)。

    // ===== 第 171 轮:chromiumoxide 参数双横杠坑 + profile 预置 =====

    #[test]
    fn custom_launch_flags_are_bare_names() {
        // chromiumoxide 会统一补 `--`,故这里必须是裸名,绝不能带前导 `-`。
        for headless in [false, true] {
            for geo_prompt in [false, true] {
                for f in custom_launch_flags(headless, geo_prompt) {
                    assert!(
                        !f.starts_with('-'),
                        "启动开关必须是裸名(否则被渲染成 ----xxx 被 Chrome 忽略): {f}"
                    );
                }
            }
        }
    }

    #[test]
    fn custom_launch_flags_suppress_first_run_and_background() {
        let flags = custom_launch_flags(false, false);
        for key in [
            "no-first-run",
            "no-default-browser-check",
            "disable-background-mode",
            "no-service-autorun",
        ] {
            assert!(flags.iter().any(|f| f == key), "缺少治理旗标 {key}");
        }
        // geo_prompt=true 时不加 deny-permission-prompts(保持原生行为档)
        assert!(!custom_launch_flags(false, true).contains(&"deny-permission-prompts".to_string()));
        assert!(custom_launch_flags(false, false).contains(&"deny-permission-prompts".to_string()));
        // headless 追加 sandbox 旗标(disable-gpu 自第 183 轮起默认不再下发,见下个测试)
        assert!(custom_launch_flags(true, false).contains(&"no-sandbox".to_string()));
        assert!(!custom_launch_flags(false, false).contains(&"no-sandbox".to_string()));
    }

    /// env 解析测试并行会改 LAEW_WEB_HEADLESS_SOFTRENDER,读者同样持锁防竞态
    /// (与 browser_reuse 的同类测试共用 test_support 全局锁,跨模块也串行)。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_support::GLOBAL_ENV_CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// 第 183 轮:无头档默认**真值参数集**(不带 disable-gpu/hide-scrollbars,
    /// WebGL 报本机真 GPU 而非 WARP 软渲染串);杀开关恢复旧软渲染集。
    #[test]
    fn custom_launch_flags_无头档默认真GPU且杀开关恢复旧集() {
        let _g = env_lock();
        std::env::remove_var("LAEW_WEB_HEADLESS_SOFTRENDER");
        let flags = custom_launch_flags(true, false);
        assert!(
            !flags.contains(&"disable-gpu".to_string()),
            "第 183 轮起无头档默认不强制软渲染(它会制造 WebGL WARP 指纹): {flags:?}"
        );
        assert!(
            !flags.contains(&"hide-scrollbars".to_string()),
            "第 183 轮起无头档默认不隐藏滚动条(宽度应跟随宿主 OS 默认): {flags:?}"
        );
        // 有头档从来就没有这两面旗标
        assert!(!custom_launch_flags(false, false).contains(&"disable-gpu".to_string()));
        assert!(!custom_launch_flags(false, false).contains(&"hide-scrollbars".to_string()));
        // 杀开关:恢复旧软渲染参数集(排障对照)
        for v in ["on", "1", "true", "yes", "软渲染", " ON "] {
            std::env::set_var("LAEW_WEB_HEADLESS_SOFTRENDER", v);
            let flags = custom_launch_flags(true, false);
            assert!(
                flags.contains(&"disable-gpu".to_string())
                    && flags.contains(&"hide-scrollbars".to_string()),
                "杀开关 {v:?} 必须恢复旧软渲染集: {flags:?}"
            );
        }
        // 关闭族值不恢复
        for v in ["off", "0", "false", "no", ""] {
            std::env::set_var("LAEW_WEB_HEADLESS_SOFTRENDER", v);
            assert!(
                !custom_launch_flags(true, false).contains(&"disable-gpu".to_string()),
                "关闭族值 {v:?} 不得恢复软渲染集"
            );
        }
        std::env::remove_var("LAEW_WEB_HEADLESS_SOFTRENDER");
    }

    #[test]
    fn seed_chrome_profile_writes_first_run_and_prefs() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("prof");
        seed_chrome_profile(&profile);
        assert!(profile.join("First Run").is_file(), "First Run 哨兵应存在");
        assert!(profile.join("Local State").is_file());
        assert!(profile.join("Default/Preferences").is_file());
        let prefs = std::fs::read_to_string(profile.join("Default/Preferences")).unwrap();
        assert!(prefs.contains("has_seen_welcome_page"));
        let local = std::fs::read_to_string(profile.join("Local State")).unwrap();
        assert!(local.contains("background_mode"));
    }
}
