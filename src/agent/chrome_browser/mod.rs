//! Chrome/CDP 浏览器驱动基础框架(`MCP_Web_Chrome_Use` 工具底层)。
//!
//! 双浏览器工具架构中的「默认优先」方案:日常 80-90% 的网页打开/读取/交互走本驱动
//! (真 Chromium 渲染,headed 窗口支持人工介入);对 Chrome 风控敏感的站点由
//! `MCP_Web_LsmAgentBrowser_Use`(进程内 LsmAgentBrowser 内核)接管,
//! 选用决策见 `docs/双浏览器工具架构/01-设计与解决方案.md`。
//!
//! 本框架为重写的基础版:子进程拉起/回收、页面注册表、导航/eval/截图/点击/输入。
//! 反检测注入、指纹一致化、HITL 闭环、站点熔断等高级能力按设计文档分阶段
//! 从 `docs/MCP_Web_Chrome_Use/reference实现归档/` 对照移植。
//!
//! 历史选型与启动参数依据(逐条根治记录)见 `docs/MCP_Web_Chrome_Use/` 系列文档。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chromiumoxide::browser::BrowserConfig;
use chromiumoxide::Page;

use crate::error::{AgentError, Result};

mod page;
#[cfg(test)]
mod tests;
pub mod mouse;
pub mod status;
pub mod watchdog;

pub use mouse::*;
pub use page::*;

/// Chrome 启动形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeMode {
    /// 有头窗口(真浏览器形态,支持人工介入;交互式终端默认)。
    Headed,
    /// 纯无头(new headless,系统级无窗口;管道/CI 默认)。
    Hidden,
}

impl ChromeMode {
    /// `LAEW_CHROME_MODE` = `headed` / `hidden`;未设置时按交互性自动判定
    /// (stdin/stdout 均 TTY → headed,否则 hidden),与 `LAEW_BROWSER_MODE` 语义对齐。
    pub fn from_env_or_default() -> Self {
        if let Ok(raw) = std::env::var("LAEW_CHROME_MODE") {
            match raw.trim().to_ascii_lowercase().as_str() {
                "headed" | "head" | "visible" => return Self::Headed,
                "hidden" | "headless" | "new_headless" => return Self::Hidden,
                _ => {}
            }
        }
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            Self::Headed
        } else {
            Self::Hidden
        }
    }

    pub fn is_headed(self) -> bool {
        matches!(self, Self::Headed)
    }
}

/// 检测本机 Chrome / Edge / Chromium / Brave 可执行文件。
///
/// 优先级:`LAEW_BROWSER_PATH` 环境变量 → 平台默认安装路径。
/// (与历史实现一致,见归档 `src/agent/browser.rs::detect_browser`。)
pub fn detect_browser() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LAEW_BROWSER_PATH") {
        let p = p.trim().trim_matches('"').to_string();
        if !p.is_empty() && std::path::Path::new(&p).is_file() {
            return Some(PathBuf::from(p));
        }
    }

    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    ];

    #[cfg(target_os = "windows")]
    let candidates: Vec<String> = {
        let mut v: Vec<String> = Vec::new();
        for base in [
            std::env::var("PROGRAMFILES").ok(),
            std::env::var("PROGRAMFILES(X86)").ok(),
            std::env::var("LOCALAPPDATA").ok(),
        ]
        .into_iter()
        .flatten()
        {
            let base = PathBuf::from(base);
            v.push(
                base.join(r"Google\Chrome\Application\chrome.exe")
                    .to_string_lossy()
                    .into_owned(),
            );
            v.push(
                base.join(r"Microsoft\Edge\Application\msedge.exe")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        // Windows 下借用 candidates 切片统一处理,转为泄漏静态不可行,
        // 直接在下方分支返回。
        for c in &v {
            if std::path::Path::new(c).is_file() {
                return Some(PathBuf::from(c));
            }
        }
        v
    };

    #[cfg(target_os = "linux")]
    let candidates: &[&str] = &[
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/microsoft-edge",
    ];

    #[cfg(not(target_os = "windows"))]
    for c in candidates {
        if std::path::Path::new(c).is_file() {
            return Some(PathBuf::from(c));
        }
    }
    #[cfg(target_os = "windows")]
    let _ = candidates;

    None
}

/// Chrome 启动参数(裸名,chromiumoxide 统一补 `--`,绝不能带前导 `-`)。
///
/// 沿用历史实现的核心集(逐条依据见归档 `src/agent/browser.rs::custom_launch_flags`):
/// 首启/常驻治理、后台节流关闭、`--ignore-certificate-errors`(自签名站点直达,
/// 与 `LAEW_TLS_INSECURE` HTTP 客户端策略对齐)、
/// `--disable-blink-features=AutomationControlled`(压 webdriver 指纹)。
pub(crate) fn custom_launch_flags(headless: bool) -> Vec<String> {
    let mut flags = vec![
        "no-first-run".to_string(),
        "no-default-browser-check".to_string(),
        "disable-background-mode".to_string(),
        "disable-background-networking".to_string(),
        "disable-background-timer-throttling".to_string(),
        "disable-backgrounding-occluded-windows".to_string(),
        "disable-renderer-backgrounding".to_string(),
        "disable-component-update".to_string(),
        "deny-permission-prompts".to_string(),
        "metrics-recording-only".to_string(),
        "ignore-certificate-errors".to_string(),
        "disable-blink-features=AutomationControlled".to_string(),
        "disable-features=LocalNetworkAccess,BlockInsecurePrivateNetworkRequests,PrivateNetworkAccessRestrictions,HttpsUpgrades,TranslateUI".to_string(),
    ];
    if headless {
        // VM/RDP/容器环境兜底(headless 专属;有头档不加,避免制造软渲染指纹)。
        flags.push("no-sandbox".to_string());
        flags.push("disable-dev-shm-usage".to_string());
    }
    flags
}

/// 进程内持有的 Chrome 子进程句柄。
struct BrowserHolder {
    browser: Arc<chromiumoxide::Browser>,
    /// CDP handler 任务(浏览器关闭即退出)。
    _task: tokio::task::JoinHandle<()>,
    /// 一次性 user-data-dir(进程退出时清理)。
    user_data_dir: PathBuf,
    /// Chrome 主进程 pid(等待真正退出后再删 profile;helper 仍存活时会重建目录)。
    browser_pid: Option<u32>,
    /// 进程外 watchdog(主进程被强杀时负责回收 Chrome;正常收口时自行退出)。
    watchdog: Option<std::process::Child>,
}

impl BrowserHolder {
    fn shutdown(&mut self) {
        // `Child::kill` 是异步的;回收路径在同步上下文,
        // 用内层 tokio Child 的 `start_kill()` 发起即返回的 kill(不等待回收完成)。
        // shutdown 时 holder 被唯一持有,Arc 可安全取 &mut。
        if let Some(browser) = Arc::get_mut(&mut self.browser) {
            if let Some(child) = browser.get_mut_child() {
                if let Err(e) = child.as_mut_inner().start_kill() {
                    tracing::warn!(error = %e, "Chrome 子进程回收失败(进程退出时 OS 会兜底回收)");
                }
            }
        }
        // Chrome 退出是异步的:start_kill 只发起、不等待。helper/renderer 在
        // 主进程死后仍可能短暂写 profile,先删会被重建(实测残留 8K Default 目录)。
        // 先等主进程消失,再走带重试 + 静默窗确认的 remove_profile。
        if let Some(pid) = self.browser_pid {
            for _ in 0..50 {
                if !watchdog::process_alive(pid) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        watchdog::remove_profile(&self.user_data_dir);
        // watchdog 观察 Chrome 退出即自行返回;这里只做一次非阻塞回收。
        if let Some(mut wd) = self.watchdog.take() {
            let _ = wd.try_wait();
        }
    }
}

/// 页面所有权标签(第 181 轮,2026-10-27 京东实测)。
///
/// 并行 WorkFlow 单元共享同一 Chrome 实例,实测发生过 A 单元以「风控页」为由
/// close 掉 B 单元正在使用的页面(B 下一次调用报 2004,被迫自愈浪费 2+ 轮)。
/// `open` 时打上打开者(活动台账弱引用)标签;`close` 时拒绝关闭
/// 「拥有者仍活跃且非当前调用者」的页面。单元结束(台账 Drop)后页面成为
/// 孤儿页,允许后续单元 close 或被同 URL `open` 收养复用。
#[derive(Clone)]
pub struct PageOwner {
    /// 打开者 Agent 名(诊断展示用)。
    pub agent: String,
    /// 打开者台账的会话开始时刻(毫秒,展示用)。
    pub started_ms: u128,
    /// 打开者活动台账弱引用:`upgrade` 成功 = 拥有者单元仍在运行。
    weak: std::sync::Weak<std::sync::Mutex<crate::agent::web_activity::ActivityState>>,
}

impl std::fmt::Debug for PageOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageOwner")
            .field("agent", &self.agent)
            .field("alive", &self.alive())
            .finish()
    }
}

impl PageOwner {
    /// 拥有者单元是否仍在运行。
    pub fn alive(&self) -> bool {
        self.weak.upgrade().is_some()
    }

    /// 是否就是当前调用者(按台账指针判等;`current` 为 None 时恒 false)。
    pub fn is_current(
        &self,
        current: Option<&std::sync::Weak<std::sync::Mutex<crate::agent::web_activity::ActivityState>>>,
    ) -> bool {
        current.is_some_and(|w| std::sync::Weak::ptr_eq(&self.weak, w))
    }
}

/// 注册表条目:页面句柄 + 所有权标签。
struct PageEntry {
    page: Arc<Page>,
    owner: Option<PageOwner>,
}

/// 存活页信息(`list_pages_detailed` 返回,含所有权视图)。
#[derive(Debug, Clone)]
pub struct PageInfo {
    pub page_id: String,
    pub url: String,
    /// 打开者(无 = 上下文外打开的无主页)。
    pub owner: Option<PageOwner>,
}

/// Chrome 驱动单例:整个进程一个 Chrome 子进程,page_id → Page 注册表。
pub struct ChromeManager {
    holder: Mutex<Option<BrowserHolder>>,
    pages: Mutex<HashMap<String, PageEntry>>,
    seq: AtomicU64,
    mode: ChromeMode,
}

impl ChromeManager {
    fn new() -> Self {
        Self {
            holder: Mutex::new(None),
            pages: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(0),
            mode: ChromeMode::from_env_or_default(),
        }
    }

    /// 全局单例。
    pub fn global() -> Arc<Self> {
        static GLOBAL: OnceLock<Arc<ChromeManager>> = OnceLock::new();
        GLOBAL.get_or_init(|| Arc::new(Self::new())).clone()
    }

    pub fn mode(&self) -> ChromeMode {
        self.mode
    }

    /// 取当前调用者的所有权标签(不在 Agent 任务树内 = None,无主页)。
    fn owner_of_now() -> Option<PageOwner> {
        let act = crate::agent::web_activity::current()?;
        let snap = act.snapshot();
        Some(PageOwner {
            agent: snap.agent,
            started_ms: snap.started_ms,
            weak: act.inner_weak(),
        })
    }

    /// 确保 Chrome 子进程已拉起(惰性,首次 open 时触发)。
    async fn ensure_browser(&self) -> Result<Arc<chromiumoxide::Browser>> {
        {
            let holder = self.holder.lock().map_err(lock_err)?;
            if let Some(h) = holder.as_ref() {
                return Ok(h.browser.clone());
            }
        }

        let exe = detect_browser().ok_or_else(|| {
            AgentError::ToolExecution { tool: "chrome_browser".into(), reason: format!(
                "未检测到 Chrome/Edge/Chromium 浏览器。请安装 Google Chrome 或 Microsoft Edge, \
                 或设置环境变量 LAEW_BROWSER_PATH 指向浏览器可执行文件"
            )}
        })?;

        let mut builder = BrowserConfig::builder()
            .chrome_executable(exe)
            .disable_default_args();
        // chromiumoxide DEFAULT_ARGS(puppeteer 同源)携带 `--enable-automation`,
        // 会被 WAF 反爬;剔除后叠加自选参数集(历史第 159 轮根治)。
        let headless = !self.mode.is_headed();
        for flag in custom_launch_flags(headless) {
            builder = builder.arg(flag);
        }
        if headless {
            builder = builder.new_headless_mode();
        } else {
            builder = builder.with_head();
        }
        // 窗口尺寸缺省 1920×1080(历史第 125 轮:1440×900 过窄会裁现代 Web 应用)。
        let (win_w, win_h) = (1920u32, 1080u32);
        builder = builder.window_size(win_w, win_h);
        builder = if self.mode.is_headed() {
            // 有头禁用视口仿真:页面视口=窗口内容区,天然铺满(历史第 161 轮)。
            builder.viewport(None::<chromiumoxide::handler::viewport::Viewport>)
        } else {
            builder.viewport(chromiumoxide::handler::viewport::Viewport {
                width: win_w,
                height: win_h,
                device_scale_factor: None,
                emulating_mobile: false,
                is_landscape: false,
                has_touch: false,
            })
        };
        // 一次性 user-data-dir:避免默认固定目录的 SingletonLock 冲突(并行/残留锁),
        // 同时与用户日常 profile 隔离(历史实现同策略)。
        let dir = std::env::temp_dir().join(format!("laew_chrome_{}", {
            let mut b = [0u8; 4];
            rand::Rng::fill(&mut rand::thread_rng(), &mut b);
            b.iter().map(|x| format!("{x:02x}")).collect::<String>()
        }));
        builder = builder.user_data_dir(&dir);

        let config = builder
            .build()
            .map_err(|e| AgentError::ToolExecution { tool: "chrome_browser".into(), reason: format!("Chrome 启动配置失败: {e}")})?;

        let launch_timeout = Duration::from_secs(30);
        let (mut browser, mut handler) = tokio::time::timeout(
            launch_timeout,
            chromiumoxide::Browser::launch(config),
        )
        .await
        .map_err(|_| {
            AgentError::ToolExecution { tool: "chrome_browser".into(), reason: "Chrome 启动超时(30s),请检查 Chrome 是否可正常启动".into()}
        })?
        .map_err(|e| AgentError::ToolExecution { tool: "chrome_browser".into(), reason: format!("Chrome 启动失败: {e}")})?;

        let task = tokio::spawn(async move {
            use futures::StreamExt;
            while let Some(msg) = handler.next().await {
                if msg.is_err() {
                    break;
                }
            }
        });

        let mut holder = self.holder.lock().map_err(lock_err)?;
        // 进程外 watchdog:主进程被 SIGKILL/强杀时,watchdog 依据 stdin pipe EOF
        // 回收 Chrome 主进程与一次性 profile(实测孤儿 Chrome 占用 100MB+/个)。
        let browser_pid = browser.get_mut_child().and_then(|c| c.as_mut_inner().id());
        let watchdog = browser_pid.and_then(|pid| watchdog::spawn_watchdog(pid, &dir));
        if watchdog.is_none() {
            tracing::warn!(
                browser_pid,
                "Chrome watchdog 启动失败(主进程被强杀时可能残留 Chrome 进程)"
            );
        }
        *holder = Some(BrowserHolder {
            browser: Arc::new(browser),
            _task: task,
            user_data_dir: dir,
            browser_pid,
            watchdog,
        });
        Ok(holder.as_ref().unwrap().browser.clone())
    }

    fn next_page_id(&self) -> String {
        let n = self.seq.fetch_add(1, Ordering::SeqCst);
        format!("c_{n:08x}")
    }

    fn register(&self, page: Page) -> (String, Arc<Page>) {
        let id = self.next_page_id();
        let arc = Arc::new(page);
        let entry = PageEntry {
            page: arc.clone(),
            owner: Self::owner_of_now(),
        };
        if let Ok(mut pages) = self.pages.lock() {
            pages.insert(id.clone(), entry);
        }
        (id, arc)
    }

    pub fn get_page(&self, page_id: &str) -> Option<Arc<Page>> {
        self.pages
            .lock()
            .ok()
            .and_then(|pages| pages.get(page_id).map(|e| e.page.clone()))
    }

    /// 取页面所有权标签(不存在/无主页返回 None)。
    pub fn page_owner(&self, page_id: &str) -> Option<PageOwner> {
        self.pages
            .lock()
            .ok()
            .and_then(|pages| pages.get(page_id).and_then(|e| e.owner.clone()))
    }

    /// 转移页面所有权(孤儿页收养时把标签改写为当前调用者)。
    fn set_owner(&self, page_id: &str, owner: Option<PageOwner>) {
        if let Ok(mut pages) = self.pages.lock() {
            if let Some(e) = pages.get_mut(page_id) {
                e.owner = owner;
            }
        }
    }

    /// 找「拥有者仍活跃且非当前调用者」的**存活**页(返回 page_id + 打开者 Agent 名)。
    ///
    /// `close(all)` 的守门:并行层若有兄弟单元还开着页面,全关会连带回收
    /// Chrome 进程杀死全部兄弟单元,必须拒绝。`current` = 当前调用者台账弱引用。
    pub async fn foreign_owner_page(
        &self,
        current: Option<&std::sync::Weak<std::sync::Mutex<crate::agent::web_activity::ActivityState>>>,
    ) -> Option<(String, String)> {
        for info in self.list_pages_detailed().await {
            if let Some(owner) = &info.owner {
                if owner.alive() && !owner.is_current(current) {
                    return Some((info.page_id, owner.agent.clone()));
                }
            }
        }
        None
    }

    /// 打开 URL,返 `(page_id, 最终 URL, 标题)`。
    ///
    /// Chrome 155 首启期存在「标签页创建成功后 target 立即失效」的瞬态竞态
    /// (实测 page_id 返回 12s 后 eval 报 `receiver is gone`)。这里做一次
    /// 存活探测 + 同浏览器内重建,避免一次瞬态失败污染整个任务。
    pub async fn open(&self, url: &str) -> Result<(String, String, String)> {
        // 第 181 轮:同 URL 孤儿页收养(见 adopt_orphan 文档)。
        if let Some(adopted) = self.adopt_orphan(url).await {
            tracing::info!(url, page_id = %adopted.0, "Chrome open 收养孤儿页(前序单元残留)");
            return Ok(adopted);
        }
        let browser = self.ensure_browser().await?;
        let mut last_err: Option<AgentError> = None;
        for attempt in 0..2 {
            match self.open_once(&browser, url).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "Chrome 新建标签页失败/立即失效,自动重试一次");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| AgentError::ToolExecution {
            tool: "chrome_browser".into(),
            reason: "新建 Chrome 标签页失败(无具体错误)".into(),
        }))
    }

    async fn open_once(
        &self,
        browser: &Arc<chromiumoxide::Browser>,
        url: &str,
    ) -> Result<(String, String, String)> {
        let page = tokio::time::timeout(Duration::from_secs(20), browser.new_page(url))
            .await
            .map_err(|_| AgentError::ToolExecution { tool: "chrome_browser".into(), reason: "新建 Chrome 标签页超时(20s)".into()})?
            .map_err(|e| AgentError::ToolExecution { tool: "chrome_browser".into(), reason: format!("新建 Chrome 标签页失败: {e}")})?;
        // `new_page(url)` 本身会等待目标初始化与首文档装载完成,不再重复 goto
        // (冗余双导航会放大启动竞态窗口)。仅当 URL 读不到时兜底导航。
        let (mut final_url, mut title) = page::url_and_title(&page).await;
        if final_url.is_empty() {
            page::navigate(&page, url).await?;
            (final_url, title) = page::url_and_title(&page).await;
        }
        if !page::page_alive(&page).await {
            return Err(AgentError::ToolExecution {
                tool: "chrome_browser".into(),
                reason: "新建标签页初始化后立即失效(target 已销毁或崩溃)".into(),
            });
        }
        let (id, _) = self.register(page);
        Ok((id, final_url, title))
    }

    /// 列出存活页面 `(page_id, 当前 URL)`。
    ///
    /// 同时剔除已失效页面:target 销毁/崩溃后 `Page` 句柄仍在注册表里,
    /// 旧实现会把 url="" 的幽灵页返回给模型(实测 c_00000000)。
    pub async fn list_pages(&self) -> Vec<(String, String)> {
        self.list_pages_detailed()
            .await
            .into_iter()
            .map(|info| (info.page_id, info.url))
            .collect()
    }

    /// 列出存活页面(含所有权视图,第 181 轮)。
    pub async fn list_pages_detailed(&self) -> Vec<PageInfo> {
        let pages: Vec<(String, Arc<Page>, Option<PageOwner>)> = self
            .pages
            .lock()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.page.clone(), v.owner.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut out = Vec::with_capacity(pages.len());
        let mut dead = Vec::new();
        for (id, page, owner) in pages {
            if page::page_alive(&page).await {
                let (url, _) = page::url_and_title(&page).await;
                out.push(PageInfo {
                    page_id: id,
                    url,
                    owner,
                });
            } else {
                dead.push(id);
            }
        }
        if !dead.is_empty() {
            tracing::warn!(pages = ?dead, "剔除已失效的 Chrome 页面");
            if let Ok(mut pages) = self.pages.lock() {
                for id in dead {
                    pages.remove(&id);
                }
            }
        }
        out
    }

    /// 同 URL 孤儿页收养(第 181 轮)。
    ///
    /// 前序单元结束后页面残留(open 恒新建页,实测页面 id 一路涨到
    /// c_00000014+,后续单元还要花轮次分辨「这些页面是不是上个任务留的」)。
    /// 存活页当前 URL 与请求一致且无主/拥有者已死 → 直接复用该页,所有权
    /// 转移给当前调用者;拥有者仍活跃(并行兄弟在用)→ 不复用(避免互踩导航)。
    async fn adopt_orphan(&self, url: &str) -> Option<(String, String, String)> {
        for info in self.list_pages_detailed().await {
            if info.url != url {
                continue;
            }
            let reuse = match &info.owner {
                None => true,
                Some(owner) => !owner.alive(),
            };
            if !reuse {
                return None;
            }
            let page = self.get_page(&info.page_id)?;
            let (cur_url, title) = page::url_and_title(&page).await;
            self.set_owner(&info.page_id, Self::owner_of_now());
            return Some((info.page_id, cur_url, title));
        }
        None
    }

    pub fn close_page(&self, page_id: &str) -> bool {
        if let Ok(mut pages) = self.pages.lock() {
            return pages.remove(page_id).is_some();
        }
        false
    }

    /// 关闭全部页面并回收 Chrome 子进程(`close(page_id="all")` 与进程退出路径调用)。
    pub fn close_all(&self) {
        if let Ok(mut pages) = self.pages.lock() {
            pages.clear();
        }
        if let Ok(mut holder) = self.holder.lock() {
            if let Some(mut h) = holder.take() {
                h.shutdown();
            }
        }
    }
}

fn lock_err(e: impl std::fmt::Display) -> AgentError {
    AgentError::ToolExecution { tool: "chrome_browser".into(), reason: format!("Chrome 驱动内部锁 poisoned: {e}")}
}
