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
    connect_mode: bool,
    pages: HashMap<String, PageEntry>,
    /// launch 模式的一次性 user-data-dir(关闭浏览器时整目录清理)。
    user_data_dir: Option<PathBuf>,
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
                        connect_mode: false,
                        pages: HashMap::new(),
                        user_data_dir: None,
                    }),
                })
            })
            .clone()
    }

    /// 新建页面;必要时先启动或接管浏览器。
    ///
    /// 返回 `(page_id, title, final_url)`;未检测到浏览器返回含 NO_BROWSER 哨兵的错误。
    /// 2026-09-17 第 74 轮:`mode` 参数控制是否真正启动浏览器进程;`Hidden` 走纯 CDP
    /// 嵌入式模式,默认无可见窗口,解决"误开 macOS 系统默认浏览器"问题。
    pub async fn new_page(
        &self,
        url: &str,
        mode: BrowserMode,
        connect_url: Option<&str>,
        user_agent: Option<&str>,
    ) -> chromiumoxide::error::Result<(String, String, String)> {
        let mut inner = self.inner.lock().await;
        let mut launch_dir: Option<PathBuf> = None;

        // Browser 操作超时(2026-09-16 第 66 轮):launch/connect/goto 统一 30s,
        // 防止页面挂起/Chrome 启动失败导致无限等待(用户反馈浏览器任务卡住 58.8s)。
        let browser_timeout = std::time::Duration::from_secs(30);

        if inner.browser.is_none() {
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
                // Hidden 模式下额外禁用 GPU 与 sandbox,降低嵌入式启动失败率
                if matches!(mode, BrowserMode::Hidden | BrowserMode::NewHeadless) {
                    builder = builder
                        .disable_default_args()
                        .arg("--disable-gpu")
                        .arg("--no-sandbox")
                        .arg("--disable-dev-shm-usage");
                }
                builder = builder.window_size(1440, 900);
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
                launch_dir = Some(dir);
                let (browser, mut handler) = tokio::time::timeout(
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
            inner.connect_mode = connect_mode;
            inner.user_data_dir = launch_dir;
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
                }
                // 清理一次性 user-data-dir(Chrome 退出可能有几百 ms 延迟,重试几次)
                if let Some(dir) = inner.user_data_dir.take() {
                    spawn_tempdir_cleanup(dir);
                }
            }
            if let Some(handler) = inner.handler.take() {
                handler.abort();
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
            }
            // 清理一次性 user-data-dir。
            if let Some(dir) = inner.user_data_dir.take() {
                spawn_tempdir_cleanup(dir);
            }
        }
        if let Some(handler) = inner.handler.take() {
            handler.abort();
        }
        inner.connect_mode = false;
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
