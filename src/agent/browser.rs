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
    /// 进程外 parent-death watchdog（仅 launch 模式）。
    watchdog: Option<std::process::Child>,
    connect_mode: bool,
    pages: HashMap<String, PageEntry>,
    /// launch 模式的一次性 user-data-dir(关闭浏览器时整目录清理)。
    user_data_dir: Option<PathBuf>,
    /// 当前浏览器实例的启动模式(open 复用判定 / adopted 页高亮注入用)。
    mode: BrowserMode,
    /// Agent 高亮蓝框是否启用(headed 可视化场景标识 Agent 控制的窗口)。
    highlight: bool,
}

/// 浏览器会话管理器(进程内单例)。
pub struct BrowserManager {
    inner: Mutex<BrowserInner>,
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

/// Agent 高亮蓝框注入脚本(2026-09-20 第 100 轮):
///
/// headed 可视化模式下,桌面上可能同时存在多个浏览器窗口,人工介入时必须一眼
/// 识别「哪个窗口被 Agent 控制」。本脚本在页面四周渲染一圈 Chrome 品牌蓝
/// (#1a73e8)选中效果 + 右上角「LAEW Agent 控制中」徽标:
/// - `pointer-events:none` 不拦截任何鼠标/键盘事件,渲染与普通浏览器一致;
/// - `data-laew-agent="1"` 标记,MCP_Web_Use 的 elements/dom 提取自动过滤该子树;
/// - 通过 `Page.addScriptToEvaluateOnNewDocument` 挂载,导航/派生新页自动重注入。
pub const AGENT_HIGHLIGHT_JS: &str = r#"(() => {
    if (window.__laewAgentHighlightInstalled) return;
    window.__laewAgentHighlightInstalled = true;
    const ensure = () => {
        let frame = document.getElementById('__laew_agent_frame__');
        if (!frame) {
            frame = document.createElement('div');
            frame.id = '__laew_agent_frame__';
            frame.setAttribute('data-laew-agent', '1');
            frame.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483647;box-shadow:inset 0 0 0 3px #1a73e8;';
            const badge = document.createElement('div');
            badge.id = '__laew_agent_badge__';
            badge.setAttribute('data-laew-agent', '1');
            badge.style.cssText = 'position:absolute;top:8px;right:8px;padding:2px 10px;background:#1a73e8;color:#fff;font:600 12px/1.6 system-ui,sans-serif;border-radius:10px;box-shadow:0 1px 4px rgba(26,115,232,.45);';
            badge.textContent = 'LAEW Agent 控制中';
            frame.appendChild(badge);
            (document.body || document.documentElement).appendChild(frame);
        }
        frame.style.display = window.__laewAgentHighlight === false ? 'none' : '';
    };
    window.__laewAgentApplyHighlight = (enabled) => {
        window.__laewAgentHighlight = !!enabled;
        ensure();
        const f = document.getElementById('__laew_agent_frame__');
        if (f) f.style.display = enabled ? '' : 'none';
        return !!enabled;
    };
    ensure();
})()"#;

/// 给页面注入 Agent 高亮蓝框(新文档自动重挂 + 当前文档立即执行)。
async fn inject_agent_highlight(page: &Page) {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
    let _ = page
        .execute(AddScriptToEvaluateOnNewDocumentParams {
            source: AGENT_HIGHLIGHT_JS.to_string(),
            world_name: None,
            include_command_line_api: None,
            // 立即在已存在的执行上下文运行(等效旧 evaluate 路径)
            run_immediately: Some(true),
        })
        .await;
}

/// 未安装浏览器哨兵:BrowserNew 据此返回错误码 3001。
pub const NO_BROWSER_SENTINEL: &str = "NO_BROWSER";

/// 2026-09-17 第 74 轮:浏览器启动模式三档枚举。
///
/// - `Hidden`:纯 CDP 嵌入式无头(`--headless=new`,系统级无窗口),推荐默认;
/// - `NewHeadless`:`--headless=new` 老式(headless=true 路径),保留兼容;
/// - `Headed`:有窗口浏览器(用户调试/截图场景),默认禁止。
///
/// 工具面 `BrowserNew` 默认 `hidden`,仅当用户显式 `mode=headed` 才出窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserMode {
    Hidden,
    NewHeadless,
    Headed,
}

impl BrowserMode {
    pub fn from_env_or_default() -> Self {
        // 显式环境变量优先(供调试/QA/截图场景)。
        if let Ok(v) = std::env::var("LAEW_BROWSER_MODE") {
            match v.to_ascii_lowercase().as_str() {
                "headed" | "head" | "with_head" | "false" | "0" => return Self::Headed,
                "new_headless" | "old_headless" | "true" | "1" => return Self::NewHeadless,
                "hidden" | "inprocess" | "cdp_only" => return Self::Hidden,
                _ => {}
            }
        }
        // 兼容旧 bool 开关。
        match std::env::var("LAEW_BROWSER_HEADLESS").ok().as_deref() {
            Some("0") | Some("false") | Some("no") | Some("off") => Self::Headed,
            _ => Self::Hidden, // 默认无窗口,与 74 轮修复目标一致
        }
    }
}

// =================== 第 125 轮(2026-09-23):视口基准 1080p 与 2K 自动扩展 ===================

/// 全模式默认启动窗口:1920×1080(1080p)。hidden 原 1440×900 视口过窄,
/// 现代 Web 应用(min-width > 1440 的后台/SaaS)横向被裁 → 元素不可见不可点、
/// 截图显示不全,本轮根治。
pub const DEFAULT_WINDOW_W: u32 = 1920;
/// 全模式默认启动窗口高(1080p)。
pub const DEFAULT_WINDOW_H: u32 = 1080;
/// 视口自动扩展上限宽(2K = 2560×1440):内容超此宽度仍溢出时保持滚动 +
/// full_page 截图既有路径,不再无限撑大。
pub const VIEWPORT_FIT_MAX_W: u32 = 2560;
/// 视口自动扩展上限高(2K)。
pub const VIEWPORT_FIT_MAX_H: u32 = 1440;
/// 溢出判定容差(px):亚像素布局/阴影圆角/1px 边框误差不触发扩展。
const VIEWPORT_FIT_TOL: f64 = 2.0;

/// 默认启动窗口尺寸(第 125 轮:全模式统一 1080p)。
pub fn default_window_size() -> (u32, u32) {
    (DEFAULT_WINDOW_W, DEFAULT_WINDOW_H)
}

/// 单维度适配判定:内容维度 > 视口维度 + 容差 且仍有扩展空间时返回 Some(目标)。
/// 只放大不缩小;当前已 ≥ cap 时维持原状(返回 None,交给滚动/full_page)。
fn fit_dim(cur: f64, content: f64, cap: u32) -> Option<u32> {
    if content <= cur + VIEWPORT_FIT_TOL {
        return None;
    }
    let cur_i = cur.max(0.0).ceil() as u32;
    if cur_i >= cap {
        return None;
    }
    let target = content.ceil() as u32;
    let target = target.clamp(cur_i + 1, cap);
    if target > cur_i {
        Some(target)
    } else {
        None
    }
}

/// 视口自动扩展决策(纯函数,可单测):输入当前视口 (iw,ih) 与内容滚动尺寸
/// (sw,sh),需要扩展返回 Some((target_w,target_h)),否则 None。
pub fn viewport_fit_plan(
    iw: f64,
    ih: f64,
    sw: f64,
    sh: f64,
    max_w: u32,
    max_h: u32,
) -> Option<(u32, u32)> {
    match (fit_dim(iw, sw, max_w), fit_dim(ih, sh, max_h)) {
        (Some(w), Some(h)) => Some((w, h)),
        (Some(w), None) => Some((w, ih.max(0.0).ceil() as u32)),
        (None, Some(h)) => Some((iw.max(0.0).ceil() as u32, h)),
        (None, None) => None,
    }
}

/// 视口/内容尺寸测量 JS(fit 与截图溢出提示共用)。
const VIEWPORT_METRICS_JS: &str = r#"(() => {
    const de = document.documentElement, b = document.body;
    const sw = Math.max(de ? de.scrollWidth : 0, b ? b.scrollWidth : 0);
    const sh = Math.max(de ? de.scrollHeight : 0, b ? b.scrollHeight : 0);
    return {
        innerWidth: window.innerWidth, innerHeight: window.innerHeight,
        scrollWidth: sw, scrollHeight: sh,
        devicePixelRatio: window.devicePixelRatio || 1,
    };
})()"#;

/// 读取页面视口 + 内容滚动尺寸(CDP evaluate,returnByValue)。
pub(crate) async fn page_viewport_metrics(
    page: &chromiumoxide::Page,
) -> std::result::Result<Value, String> {
    page.evaluate(VIEWPORT_METRICS_JS)
        .await
        .map_err(|e| format!("测量视口失败: {e}"))?
        .value()
        .cloned()
        .ok_or_else(|| "测量视口失败: 返回为空".to_string())
}

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

impl BrowserManager {
    pub fn global() -> Arc<Self> {
        static MANAGER: OnceLock<Arc<BrowserManager>> = OnceLock::new();
        MANAGER
            .get_or_init(|| {
                Arc::new(Self {
                    inner: Mutex::new(BrowserInner {
                        browser: None,
                        handler: None,
                        watchdog: None,
                        connect_mode: false,
                        pages: HashMap::new(),
                        user_data_dir: None,
                        mode: BrowserMode::Hidden,
                        highlight: true,
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

    /// 新建页面;必要时先启动或接管浏览器。
    ///
    /// 返回 `(page_id, title, final_url)`;未检测到浏览器返回含 NO_BROWSER 哨兵的错误。
    /// 2026-09-17 第 74 轮:`mode` 参数控制是否真正启动浏览器进程;`Hidden` 走纯 CDP
    /// 嵌入式模式,默认无可见窗口,解决"误开 macOS 系统默认浏览器"问题。
    /// 2026-09-20 第 100 轮:`window_size` 支持自定义启动窗口(headed 默认 1920×1080
    /// 1080p);`highlight` 控制 Agent 高亮蓝框注入(headed 可视化标识)。
    #[allow(clippy::too_many_arguments)]
    pub async fn new_page(
        &self,
        url: &str,
        mode: BrowserMode,
        connect_url: Option<&str>,
        user_agent: Option<&str>,
        window_size: Option<(u32, u32)>,
        highlight: bool,
    ) -> chromiumoxide::error::Result<(String, String, String)> {
        let mut inner = self.inner.lock().await;
        let mut launch_dir: Option<PathBuf> = None;
        let mut launch_watchdog: Option<std::process::Child> = None;

        // Browser 操作超时(2026-09-16 第 66 轮):launch/connect/goto 统一 30s,
        // 防止页面挂起/Chrome 启动失败导致无限等待(用户反馈浏览器任务卡住 58.8s)。
        let browser_timeout = std::time::Duration::from_secs(30);

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
                (browser, task, true)
            } else {
                let exe = detect_browser().ok_or_else(|| {
                    chromiumoxide::error::CdpError::msg(format!(
                        "{NO_BROWSER_SENTINEL}: 未检测到 Chrome/Edge/Chromium 浏览器。\
                         请安装 Google Chrome(https://www.google.cn/chrome/)或 Microsoft Edge, \
                         或设置环境变量 LAEW_BROWSER_PATH 指向浏览器可执行文件"
                    ))
                })?;
                let mut builder = BrowserConfig::builder().chrome_executable(exe);
                // 2026-09-17 第 74 轮:三档 mode 控制浏览器启动形态。
                // 默认 Hidden(纯 CDP headless=new,系统级无窗口),
                // 解决"误开 macOS 系统默认浏览器"问题。
                match mode {
                    BrowserMode::Hidden => {
                        builder = builder.new_headless_mode();
                    }
                    BrowserMode::NewHeadless => {
                        builder = builder.new_headless_mode();
                    }
                    BrowserMode::Headed => {
                        builder = builder.with_head();
                    }
                }
                // Hidden 模式下额外禁用 GPU 与 sandbox,降低嵌入式启动失败率;
                // --hide-scrollbars(第 125 轮):headless 截图滚动条不再遮挡内容,
                // 且 innerWidth 不再被经典滚动条蚕食(推荐启动参数矩阵,
                // 参考 docs/Agent源码调研/专题/专题-第十三轮-GUI自动化与浏览器控制深度对比.md §6.1)
                if matches!(mode, BrowserMode::Hidden | BrowserMode::NewHeadless) {
                    builder = builder
                        .disable_default_args()
                        .arg("--disable-gpu")
                        .arg("--no-sandbox")
                        .arg("--disable-dev-shm-usage")
                        .arg("--hide-scrollbars");
                }
                // 窗口尺寸(第 125 轮):显式参数优先;缺省**全模式 1920×1080(1080p)**
                // ——hidden 原 1440×900 视口过窄,现代 Web 应用(min-width>1440 的管理后台/
                // SaaS)横向被裁导致元素不可见、不可点,截图也显示不全。
                let (win_w, win_h) = window_size.unwrap_or_else(default_window_size);
                builder = builder.window_size(win_w, win_h);
                // 一次性 user-data-dir:避免 chromiumoxide 默认固定目录的 SingletonLock 冲突
                // (并行/上次异常退出后残留锁会导致 Chrome 拒启),同时与用户日常 profile 隔离。
                let dir = std::env::temp_dir().join(format!("laew_browser_{}", {
                    let mut b = [0u8; 4];
                    rand::Rng::fill(&mut rand::thread_rng(), &mut b);
                    b.iter().map(|x| format!("{x:02x}")).collect::<String>()
                }));
                builder = builder.user_data_dir(&dir);
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
            inner.user_data_dir = launch_dir;
            inner.mode = mode;
            inner.highlight = highlight;
            // 第 78 轮:标记浏览器已启动,供 cleanup_sync() 快速判断避免无意义创建 Runtime。
            browser_started_flag::set_started();
        }

        let browser = inner.browser.as_ref().expect("browser initialized");
        let page = browser.new_page(url.to_string()).await?;
        tokio::time::timeout(browser_timeout, page.goto(url.to_string()))
            .await
            .map_err(|_| {
                chromiumoxide::error::CdpError::msg(format!(
                    "page.goto({url}) 超时({}s),请检查网络连接或稍后重试",
                    browser_timeout.as_secs()
                ))
            })??;

        // UA 覆盖(可选)
        if let Some(ua) = user_agent.filter(|s| !s.is_empty()) {
            let params = chromiumoxide::cdp::browser_protocol::emulation::SetUserAgentOverrideParams::builder()
                .user_agent(ua)
                .build()
                .map_err(chromiumoxide::error::CdpError::msg)?;
            let _ = page.execute(params).await;
        }

        // 开启 Runtime / Network 域,挂 Console / Network 事件监听(环形缓冲)
        let events = EventBuffer::default();
        let _ = page
            .execute(chromiumoxide::cdp::js_protocol::runtime::EnableParams::default())
            .await;
        let _ = page
            .execute(chromiumoxide::cdp::browser_protocol::network::EnableParams::default())
            .await;
        spawn_event_listeners(&page, events.clone()).await;
        // 第 100 轮:headed 可视化模式注入 Agent 高亮蓝框(hidden 无窗口,注入无意义
        // 且污染 DOM 提取,跳过)。注意:浏览器已存在时以实例真实 mode 为准
        // (inner.mode),而非本次 open 请求的 mode —— 单实例复用,不新启浏览器。
        let want_highlight =
            inner.highlight && matches!(inner.mode, BrowserMode::Headed);
        if want_highlight {
            inject_agent_highlight(&page).await;
        }

        let title = page.get_title().await.ok().flatten().unwrap_or_default();
        let final_url = page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| url.to_string());
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
        Ok((id, title, final_url))
    }

    /// 列出当前注册页面(带存活性探测,失效 entry 顺手清理)。
    pub async fn list_pages(&self) -> Vec<(String, String, String, String)> {
        let mut inner = self.inner.lock().await;
        let mut dead = Vec::new();
        let mut out = Vec::new();
        for (id, entry) in &inner.pages {
            match entry.page.url().await {
                Ok(Some(url)) => {
                    let title = entry
                        .page
                        .get_title()
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    out.push((id.clone(), url, title, entry.created_at.clone()));
                }
                _ => dead.push(id.clone()),
            }
        }
        for id in dead {
            inner.pages.remove(&id);
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
        Ok(json!({
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
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let viewport = page
            .evaluate(
                "({width: window.innerWidth, height: window.innerHeight, devicePixelRatio: window.devicePixelRatio, scrollWidth: document.documentElement.scrollWidth, scrollHeight: document.documentElement.scrollHeight})",
            )
            .await
            .map_err(|e| format!("读取视口失败:{e}"))?;
        Ok(viewport.value().cloned().unwrap_or(Value::Null))
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
            .mobile(false)
            .build()
            .map_err(|e| e.to_string())?;
        page.execute(params)
            .await
            .map_err(|e| format!("视口扩展失败: {e}"))?;
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
    /// 依据 `window.__laewAgentHighlight` 延续)。
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
        let js = format!(
            "window.__laewAgentApplyHighlight ? window.__laewAgentApplyHighlight({enabled}) : null"
        );
        let r = page
            .evaluate(js)
            .await
            .map_err(|e| format!("切换高亮失败:{e}"))?;
        Ok(json!({
            "enabled": enabled,
            "applied": !r.value().map(|v| v.is_null()).unwrap_or(true),
        }))
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

    /// 动作后 adopt 派生标签页:diff browser.pages() 与注册表,
    /// 未登记的 page target 注册为新 page_id 返回。
    pub async fn adopt_spawned_pages(&self) -> Vec<String> {
        let mut inner = self.inner.lock().await;
        let Some(browser) = inner.browser.as_ref() else {
            return Vec::new();
        };
        let Ok(pages) = browser.pages().await else {
            return Vec::new();
        };
        let known: std::collections::HashSet<String> =
            inner.pages.values().map(|e| e.target_id.clone()).collect();
        let mut adopted = Vec::new();
        for page in pages {
            let tid: String = page.target_id().clone().into();
            if known.contains(&tid) {
                continue;
            }
            let events = EventBuffer::default();
            let _ = page
                .execute(chromiumoxide::cdp::js_protocol::runtime::EnableParams::default())
                .await;
            let _ = page
                .execute(chromiumoxide::cdp::browser_protocol::network::EnableParams::default())
                .await;
            spawn_event_listeners(&page, events.clone()).await;
            // 第 100 轮:headed + highlight 时,派生/adopt 的新标签页同样注入蓝框。
            if inner.highlight && matches!(inner.mode, BrowserMode::Headed) {
                inject_agent_highlight(&page).await;
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
            adopted.push(id);
        }
        adopted
    }

    /// 关闭页面;最后一个页面关闭时回收 launch 模式浏览器进程。
    /// 返回 false 表示 page_id 不存在(幂等语义,工具层映射 2000)。
    pub async fn close_page(&self, page_id: &str) -> bool {
        let mut inner = self.inner.lock().await;
        let Some(entry) = inner.pages.remove(page_id) else {
            return false;
        };
        let _ = entry.page.close().await;
        if inner.pages.is_empty() {
            if !inner.connect_mode {
                if let Some(mut browser) = inner.browser.take() {
                    let _ = browser.close().await;
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        browser.kill(),
                    )
                    .await;
                }
                // 清理一次性 user-data-dir(Chrome 退出可能有几百 ms 延迟,重试几次)
                if let Some(dir) = inner.user_data_dir.take() {
                    spawn_tempdir_cleanup(dir);
                }
            }
            if let Some(handler) = inner.handler.take() {
                handler.abort();
            }
            if let Some(child) = inner.watchdog.take() {
                spawn_watchdog_reaper(child);
            }
            inner.connect_mode = false;
        }
        true
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
                spawn_tempdir_cleanup(dir);
            }
        }
        if let Some(handler) = inner.handler.take() {
            handler.abort();
        }
        if let Some(child) = inner.watchdog.take() {
            spawn_watchdog_reaper(child);
        }
        inner.connect_mode = false;
        inner.mode = BrowserMode::Hidden;
        inner.highlight = true;
    }

    /// 同步清理入口(供 atexit / panic hook / signal handler 调用)。
    ///
    /// 内部创建独立 current-thread Runtime 执行异步 shutdown,避免依赖可能已销毁的
    /// 主 Runtime(panic/atexit 场景主 Runtime 可能正在 teardown)。
    /// 清理失败不 panic,避免 panic-in-panic 递归。
    ///
    /// 设计(第 78 轮,2026-09-17):多层防御的 L2/L3 兜底。
    pub fn cleanup_sync() {
        // 快速路径:通过全局静态 flag 判断是否曾启动浏览器,避免无意义创建 Runtime。
        if !browser_started_flag::is_started() {
            return;
        }
        // 清理过程中忽略任何 panic,防止 panic-in-panic。
        let result = std::panic::catch_unwind(|| {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            rt.block_on(async {
                let manager = Self::global();
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(8),
                    manager.shutdown(),
                )
                .await;
            });
        });
        if let Err(_) = result {
            // cleanup 中 panic,静默忽略;此时进程即将终止,无影响。
            #[cfg(debug_assertions)]
            eprintln!("[laew] browser cleanup_sync panic suppressed");
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
fn spawn_parent_watchdog(browser_pid: u32, user_data_dir: &Path) -> Option<std::process::Child> {
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
    unsafe {
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
        let ev = events;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_id_format() {
        let id = new_page_id();
        assert!(id.starts_with("p_"));
        assert_eq!(id.len(), 2 + 8);
        assert!(id[2..].chars().all(|c| c.is_ascii_hexdigit()));
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
    // ★ 2026-09-17 第 79 轮:环境变量类测试用互斥锁串行 —— 4 个测试并行跑时
    // set_var/remove_var 互相踩(default 测试读到 env_overrides 设置的 headed),
    // 全量 `cargo test` 偶发失败(存量 flaky,与功能无关)。

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn browser_mode_default_is_hidden() {
        let _g = env_lock();
        unsafe { std::env::remove_var("LAEW_BROWSER_MODE") };
        unsafe { std::env::remove_var("LAEW_BROWSER_HEADLESS") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::Hidden);
    }

    #[test]
    fn browser_mode_env_overrides() {
        let _g = env_lock();
        unsafe { std::env::set_var("LAEW_BROWSER_MODE", "headed") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::Headed);
        unsafe { std::env::set_var("LAEW_BROWSER_MODE", "new_headless") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::NewHeadless);
        unsafe { std::env::set_var("LAEW_BROWSER_MODE", "hidden") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::Hidden);
        unsafe { std::env::remove_var("LAEW_BROWSER_MODE") };
    }

    #[test]
    fn browser_mode_legacy_headless_env() {
        let _g = env_lock();
        unsafe { std::env::set_var("LAEW_BROWSER_HEADLESS", "0") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::Headed);
        unsafe { std::env::set_var("LAEW_BROWSER_HEADLESS", "false") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::Headed);
        unsafe { std::env::remove_var("LAEW_BROWSER_HEADLESS") };
        assert_eq!(BrowserMode::from_env_or_default(), BrowserMode::Hidden);
    }

    #[test]
    fn browser_mode_invalid_env_does_not_panic() {
        let _g = env_lock();
        unsafe { std::env::set_var("LAEW_BROWSER_MODE", "garbage") };
        unsafe { std::env::remove_var("LAEW_BROWSER_HEADLESS") };
        // 只确保不 panic;非法值走默认 fallback
        let _ = BrowserMode::from_env_or_default();
        unsafe { std::env::remove_var("LAEW_BROWSER_MODE") };
    }
}
