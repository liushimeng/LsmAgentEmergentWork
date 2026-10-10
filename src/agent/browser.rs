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
    /// 页面管控期望态(第 143 轮:locked 屏蔽/open 开放/partial 部分屏蔽;
    /// 第 141 轮的 `overlay: bool` 升级而来,locked 档语义与蒙层完全一致)。
    guard: super::browser_overlay::PageGuardConfig,
    /// 自动跟随最新内容期望态(第 152 轮,默认开):导航后新文档按它重挂跟随器,
    /// 与 guard 的期望态 re-assert 同构。connect 模式恒 false(不干预用户浏览器)。
    auto_follow: bool,
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

// 第 139 轮:模式决策搬到 `super::browser_mode`,窗口/视口尺寸决策搬到
// `super::browser_viewport`。此处 `pub use` 再导出,保证所有
// `crate::agent::browser::XXX` 外部引用路径零改动。
pub use super::browser_mode::BrowserMode;
pub use super::browser_viewport::{
    default_window_size, headed_window_plan, viewport_fit_plan, DEFAULT_WINDOW_H,
    DEFAULT_WINDOW_W, HEADED_SCREEN_MARGIN, MIN_VIEWPORT_H, MIN_VIEWPORT_W, VIEWPORT_FIT_MAX_H,
    VIEWPORT_FIT_MAX_W,
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
                        guard: super::browser_overlay::PageGuardConfig::from_env_default(),
                        // 第 152 轮:默认开启跟随 —— 人工在 locked 档无法滚动,
                        // 页面必须自己停在最新,否则多轮对话只有第一屏可见。
                        auto_follow: true,
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
        let mut inner = self.inner.lock().await;
        let mut launch_dir: Option<PathBuf> = None;
        let mut launch_watchdog: Option<std::process::Child> = None;
        // launch 模式浏览器主进程 PID(第 127 轮):供 cleanup_registry 登记给
        // cleanup_sync 同步清理路径使用;connect 模式保持 0(不拥有外部进程)。
        let mut launch_pid: u32 = 0;

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
        // 第 143 轮:页面管控(headed && 非 connect 才注入;三档全注入 —— open 档
        // 也要有「页面开放·人工可直接操作」状态条,人工明确知道自己可以上手)。
        // 注意与高亮相互独立:highlight=false 也能有管控,guard=open 也能带蓝框。
        if !inner.connect_mode && matches!(inner.mode, BrowserMode::Headed) {
            let cfg = inner.guard.clone();
            super::browser_overlay::inject_agent_guard(&page, &cfg).await;
            let _ = super::browser_overlay::apply_page_guard(&page, &cfg).await;
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
        let inner_mode = self.inner.lock().await.mode;
        let base = json!({
            "mode": inner_mode.as_str(),
            "min_viewport": [super::browser_viewport::MIN_VIEWPORT_W, super::browser_viewport::MIN_VIEWPORT_H],
        });
        if !inner_mode.is_headed() {
            return Ok(json!({ "adjusted": false, "reason": "非有头模式,无需收边", "base": base }));
        }
        let page = self
            .page(page_id)
            .await
            .ok_or_else(|| "page_id 不存在".to_string())?;
        let m = headed_window_metrics(&page).await?;
        let f = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let (ow, oh) = (f("outerWidth"), f("outerHeight"));
        let (iw, ih) = (f("innerWidth"), f("innerHeight"));
        let chrome = oh - ih;
        let (aw, ah) = (f("availWidth"), f("availHeight"));
        let measured = json!({
            "screen": {"avail_width": aw, "avail_height": ah},
            "outer": {"width": ow, "height": oh},
            "inner": {"width": iw, "height": ih},
            "chrome_height": chrome,
        });
        let Some((tw, th)) = headed_window_plan(ow, oh, aw, ah, chrome) else {
            return Ok(json!({
                "adjusted": false,
                "reason": "窗口已完整落在屏幕内且视口不低于下限",
                "measured": measured,
                "base": base,
            }));
        };
        // 只传 width/height,不动 left/top —— 保持用户/Chrome 已有的窗口位置。
        let applied = self
            .set_window_bounds(page_id, Some(tw as i64), Some(th as i64), None, None, None)
            .await?;
        Ok(json!({
            "adjusted": true,
            "from": {"width": ow, "height": oh},
            "target": {"width": tw, "height": th},
            "measured": measured,
            "applied": applied.get("window").cloned().unwrap_or(Value::Null),
            "note": "有头模式窗口已按屏幕工作区收边,保证完整可见;页面视口下限 720p、默认 1080p",
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

    /// 运行时切换页面管控三档(第 143 轮,镜像 `set_highlight` 语义):
    /// 校验 + 归一化 → 更新实例期望态 → 立即对指定 page 应用
    /// (CDP 输入拦截按档 + JS 蒙层/状态条/盾区)。
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
            "visual_mask": applied["visual_mask"],
            "shields_active": applied["shields_active"],
            "hint": "页面管控三档(第 143 轮):locked=屏蔽(蒙层+输入拦截,缺省)/ open=非屏蔽(人工可直接操作)/ partial=部分屏蔽(盾区外人工可操作);运行时随时切换",
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
        let mut inner = self.inner.lock().await;
        let Some(browser) = inner.browser.as_ref() else {
            return Vec::new();
        };
        if inner.connect_mode {
            // 接管模式:浏览器归用户所有,laew 不认领也不关闭任何未知页面。
            return Vec::new();
        }
        let Ok(pages) = browser.pages().await else {
            return Vec::new();
        };
        let known: std::collections::HashSet<String> =
            inner.pages.values().map(|e| e.target_id.clone()).collect();
        let mut adopted = Vec::new();
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
            } else {
                candidates.push(page);
            }
        }
        // 给刚打开的窗口一次导航时间再复查,避免误杀 `target=_blank` 真页面
        if !noise.is_empty() && !candidates.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let mut still_noise = Vec::new();
            for page in noise {
                let url = page.url().await.ok().flatten().unwrap_or_default();
                let title = Self::page_title(&page).await;
                if super::browser_noise::is_noise_page(&url, &title) {
                    still_noise.push(page);
                } else {
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
            // 第 100 轮:headed + highlight 时,派生/adopt 的新标签页同样注入蓝框。
            if inner.highlight && matches!(inner.mode, BrowserMode::Headed) {
                inject_agent_highlight(&page).await;
            }
            // 第 143 轮:管控同理,派生新标签页按实例期望态注入 + 应用(三档)。
            if !inner.connect_mode && matches!(inner.mode, BrowserMode::Headed) {
                let cfg = inner.guard.clone();
                super::browser_overlay::inject_agent_guard(&page, &cfg).await;
                let _ = super::browser_overlay::apply_page_guard(&page, &cfg).await;
            }
            // 第 152 轮:派生页同样按实例期望态挂上跟随器(否则新标签页不跟随)
            if !inner.connect_mode && inner.auto_follow {
                super::browser_follow::install(&page, true).await;
                let _ = super::browser_follow::set(&page, true).await;
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
                    // 第 127 轮:graceful 路径已回收,清除同步清理登记,
                    // 防 atexit/panic 路径重复动作与 PID 复用误杀。
                    cleanup_registry::clear();
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
                // 第 127 轮:graceful 路径已回收,清除同步清理登记,
                // 防 atexit/panic 路径重复动作与 PID 复用误杀。
                cleanup_registry::clear();
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
        inner.guard = super::browser_overlay::PageGuardConfig::from_env_default();
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

    /// 取出 profile 目录(锁被占用 / 未登记 → None,跳过目录清理)。
    pub fn take_user_data_dir() -> Option<PathBuf> {
        USER_DATA_DIR.lock().ok().and_then(|mut slot| slot.take())
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
}
