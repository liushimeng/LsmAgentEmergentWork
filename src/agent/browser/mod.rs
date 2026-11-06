//! LsmAgentBrowser:进程内自研浏览器内核。
//!
//! 取代 Chrome + CDP:窗口/标签页、网络栈、DOM、布局、渲染、JS 执行全部在本进程。
//! `MCP_Web_LsmAgentBrowser_Use` 的对外契约由 `tools/mcp_web_lsm_browser_use` 兼容层保持不变。
//!
//! 设计见 `docs/MCP_Web_LsmAgentBrowser_Use/`。

pub mod api;
pub mod cache;
pub mod document;
pub mod find;
pub mod human_input;
pub mod images;
pub mod inspect;
pub mod js;
pub mod layout;
pub mod net;
/// 屏蔽页面状态栏 / 屏蔽页面进度栏(查看器帧合成层,`docs/双浏览器工具架构/02`)。
pub mod page_bars;
pub mod render;
pub mod reqctx;
pub mod resource;
pub mod resources;
pub mod script;
pub mod security;
/// 显示器尺寸探测(headed 查看器视口夹紧,避免窗口超出屏幕)。
pub mod screen;
pub mod storage;
pub mod svg;
pub mod style;
pub mod urlutil;
pub mod view;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod svg_tests;

use std::collections::HashMap;
use std::sync::Arc;

use once_cell::sync::Lazy;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::artifact_root;
use storage::{CookieJar, Cookie};

/// 默认 User-Agent:按宿主 OS 合成的桌面 Chrome UA(反-反爬指纹基线)。
/// 此前是 `Mozilla/5.0 (compatible; LsmAgentBrowser/0.1; +https://localhost/laew)`
/// —— 自曝爬虫身份,且与 `navigator.userAgent` 同源透传,厂商指纹(GeeTest `w=`
/// 等)按「非常见浏览器 UA」直接降权(实测 zhipin 安全验证页 reset 循环形态
/// 之一)。需要机器人礼貌标识的场合,由调用方 `open(user_agent=...)` 显式覆盖。
pub const DEFAULT_UA: &str = if cfg!(target_os = "windows") {
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/128.0.0.0 Safari/537.36"
} else if cfg!(target_os = "macos") {
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/128.0.0.0 Safari/537.36"
} else {
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/128.0.0.0 Safari/537.36"
};
/// 默认视口尺寸(1920×1080):有头窗口初始内容区按此铺开,人工拖动后按真实
/// 内容区尺寸自适应(见 `view::resize`)。
pub const DEFAULT_VIEWPORT_W: u32 = 1920;
pub const DEFAULT_VIEWPORT_H: u32 = 1080;
pub const MAX_TABS_DEFAULT: usize = 8;
pub const MAX_WINDOWS_DEFAULT: usize = 4;

/// 浏览器模式。`Headed` = 进程内无头内核 + 独立查看器子进程原生窗口显示渲染帧
/// (引擎线程零 UI 依赖);查看器不可用时自动回退纯无头,页面能力不受影响。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserMode {
    Headed,
    Hidden,
}

impl BrowserMode {
    pub const DEFAULT: BrowserMode = BrowserMode::Hidden;

    pub fn from_arg(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "headed" => Some(Self::Headed),
            "hidden" | "new_headless" | "headless" => Some(Self::Hidden),
            _ => None,
        }
    }

    pub fn from_env_or_default() -> Self {
        if let Ok(v) = std::env::var("LAEW_BROWSER_MODE") {
            if let Some(m) = Self::from_arg(&v) {
                return m;
            }
        }
        Self::interactive_default()
    }

    /// 显式 `LAEW_BROWSER_MODE` **优先于**工具传入的 `mode` 参数,再退回交互式默认。
    ///
    /// 用户在终端跑 `LAEW_BROWSER_MODE=headed laew ...` 是「我要看着浏览器跑」的强信号,
    /// 应压过 LLM 自行挑选的 `mode`(实测百度地图任务里 LLM 会选 hidden 导致窗口不显示,
    /// 用户反馈「看不到浏览器运行」)。
    ///
    /// **未设环境变量时**:工具参数里的 `hidden` 视为「模型没有意见」,由交互式
    /// 判定决定(终端交互 = headed,管道/CI = hidden)。模型显式给的 `headed` 仍然
    /// 照办(自动化里偶尔需要真的开窗口看渲染)。
    /// 理由:「用户看不到浏览器跑起来」的根因就是 LLM 顺手挑了 `hidden`
    /// (2026-10-27 百度地图任务实测:模型第一句就写「使用内置浏览器(hidden 模式)」),
    /// 而用户本人就在终端前看着 —— 终端交互这个信号比模型的选择更权威。
    /// 真要无头跑,`LAEW_BROWSER_MODE=hidden` 精确覆盖(CI / 批量采集零影响)。
    pub fn from_env_or_arg(mode_arg: Option<&str>) -> Self {
        if let Ok(v) = std::env::var("LAEW_BROWSER_MODE") {
            if let Some(m) = Self::from_arg(&v) {
                return m;
            }
        }
        match mode_arg.and_then(Self::from_arg) {
            Some(Self::Headed) => Self::Headed,
            // `hidden` / 未指定:交给交互性判定(终端交互默认有头)。
            _ => Self::interactive_default(),
        }
    }

    /// 环境变量未设置时的默认档:**交互式终端(stdin/stdout 均为 TTY)默认有头**
    /// —— 用户在终端跑 Agent 时能直接看到浏览器窗口;管道/cargo test/CI 无 TTY
    /// 场景保持无头,既有测试与自动化路径零影响。显式 LAEW_BROWSER_MODE 仍精确覆盖。
    fn interactive_default() -> Self {
        use std::io::IsTerminal as _;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            Self::Headed
        } else {
            Self::Hidden
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Headed => "headed",
            Self::Hidden => "hidden",
        }
    }

    pub fn is_headed(self) -> bool {
        matches!(self, Self::Headed)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub mobile: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: DEFAULT_VIEWPORT_W,
            height: DEFAULT_VIEWPORT_H,
            device_scale_factor: 1.0,
            mobile: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct NetworkRecord {
    pub url: String,
    pub method: String,
    pub status: u16,
    pub content_type: String,
    pub bytes: usize,
    pub elapsed_ms: u64,
    pub redirect: bool,
    /// 是否来自全内存 HTTP 缓存（命中强缓存时 elapsed_ms=0）。
    pub from_cache: bool,
    pub at_ms: u128,
    /// 页面 fetch/XHR 请求体片段(≤256 字符;宿主预取类记录为 None)。
    pub req_body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConsoleRecord {
    pub level: String,
    pub text: String,
    pub at_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct GuardState {
    pub mode: String,
    pub allow_selectors: Vec<String>,
    pub block_selectors: Vec<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TabState {
    pub id: String,
    pub window_id: String,
    pub url: String,
    pub title: String,
    pub status: u16,
    pub content_type: String,
    pub html: String,
    pub redirect_chain: Vec<String>,
    pub source_bytes: usize,
    pub history: Vec<HistoryEntry>,
    pub history_index: usize,
    pub session_storage: HashMap<String, String>,
    pub viewport: Viewport,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub network: Vec<NetworkRecord>,
    pub console: Vec<ConsoleRecord>,
    pub form_values: HashMap<String, String>,
    /// details/dialog 的运行时开合覆盖：key = stable selector，value = 是否 open。
    /// 仅作为 `tab.html` 属性之上的覆盖层，布局/样式计算前落到解析树。
    pub widget_open: HashMap<String, bool>,
    /// 第 46 轮:元素级滚动偏移（key = stable selector，value = 纵向滚动量 px）。
    ///
    /// 落到临时解析树的 `data-lsm-scroll-top` 属性后由布局层把可滚动
    /// 容器的内容上移；只对 `overflow:auto|scroll` 的元素有意义。
    pub scroll_offsets: HashMap<String, f64>,
    /// 第 41 轮:运行时伪类状态（hover/focus/focus-visible/active）。
    /// key = stable selector，value = 状态位。落到临时解析树的 `data-lsm-*`
    /// 属性后由样式引擎的 `:hover`/`:focus`/`:focus-visible`/`:active` 命中。
    pub pseudo_states: HashMap<String, document::PseudoFlags>,
    /// 39 号 P2a:文本控件运行时光标/选区（key = stable selector）。
    /// 与 `form_values` 同族的运行时覆盖层,`document_with_state` 落
    /// `data-lsm-caret`/`data-lsm-sel` 属性,布局层据此画光标条与选区高亮。
    pub form_caret: HashMap<String, document::CaretState>,
    /// 39 号 P2b:页面级自由选区(跨元素拖选,与 `form_caret` 两本账)。
    /// `document_with_state` 落首/尾元素 `data-lsm-page-sel` 属性。
    pub page_sel: Option<document::PageSelection>,
    /// P2c:页面查找(Ctrl/Cmd+F)状态;查找栏与高亮画在查看器帧合成层。
    pub find: find::FindState,
    pub focused: Option<String>,
    pub mouse: Option<(f64, f64)>,
    pub layout: layout::Layout,
    /// HTML 修订号:每次 `tab.html` 写入自增(经 `set_html`)。
    /// 与 `layout_rev` 配对做「快照×布局」一致性守卫:
    /// eval/dispatch/screenshot/inspect 入口发现两者不一致即重算布局,
    /// 根治「HTML 已被脚本改写、rect_map 仍是旧布局」导致的位置键
    /// (`:nth-of-type`)整体漂移 → JS 桥几何全 0 的问题
    /// (实测猎聘:`run_pending_scripts` 改 html 不重排,页面脚本
    /// `getBoundingClientRect/clientWidth` 全 0,二维码/地图测量链全断)。
    pub html_rev: u64,
    /// 布局修订号:最后一次 `tab.layout` 计算对应的 `html_rev`。
    pub layout_rev: u64,
    pub created_at_ms: u128,
    pub last_active_ms: u128,
    pub foreground: bool,
    pub user_agent: String,
    pub mode: BrowserMode,
    pub guard: GuardState,
    /// 屏蔽页面状态栏/进度栏快照(查看器帧合成层渲染,`page_bars.rs`)。
    pub bars: page_bars::PageBars,
    pub highlight: bool,
    pub auto_follow: bool,
    /// 页面 JS 线程句柄(阶段二:持久 Boa Context,导航换页时重建)。
    pub js: Option<script::JsHandle>,
    /// 已执行脚本指纹(首屏管线 + 交互后懒加载补齐共用一本账)。
    pub executed_scripts: std::collections::HashSet<u64>,
    /// 已解码图片缓存(阶段三第三步:URL → RGBA8,全内存)。
    pub images: images::ImageStore,
    /// 安全上下文（CORS/CSP 缓存、页面源）。
    pub security: security::SecurityContext,
    /// 页面申请过 WebGL 上下文的次数(>0 = 站点依赖 WebGL 绘制;内核无 WebGL,
    /// 该区域画不出 → 工具层据此如实告知调用方改用 MCP_Web_Chrome_Use)。
    pub webgl_asked: u32,
    /// 最近一次脚本管线(首屏或交互补齐)是否被预算截断 + 剩余待执行脚本数
    /// (第 174 轮观测性:eval_js/控制动作结果与 page_health 均带出,
    /// Agent 可据此 wait/重试,不再对「组件差一口气装不上」盲探测)。
    pub script_budget_cut: bool,
    pub script_pending: usize,
    /// P2d:datalist 联想面板状态(`Some` = 面板展开;39 号设计文档)。
    pub datalist: Option<DatalistState>,
    /// P2d:select[multiple] 的人工 Shift 范围选择锚点(key = select 稳定
    /// 选择器,value = 上次普通/Ctrl 点击的 option 下标)。
    pub select_anchor: HashMap<String, usize>,
    /// P2d:查看器上报的人工缩放系数(默认 1.0;仅记账展示,不参与渲染
    /// ——缩放由查看器侧实现,Agent 截图取证仍是未缩放基准)。
    pub human_zoom: f64,
}

/// P2d:datalist 联想面板状态(39 号设计文档)。
///
/// 命中项与高亮行由 `document_with_state` 落成内核私有属性
/// (`data-lsm-datalist-open`/`data-lsm-dl-hit`/`data-lsm-dl-idx`),
/// 布局层 `top_layer::emit_datalist_panel` 只认 DOM 标记(与 select
/// 展开面板同哲学)。
#[derive(Debug, Clone)]
pub struct DatalistState {
    /// 联想输入框(稳定选择器)。
    pub input_sel: String,
    /// 命中项:(option 稳定选择器, value, label),按 DOM 序。
    pub hits: Vec<(String, String, String)>,
    /// 高亮行下标(`usize::MAX` = 无高亮)。
    pub highlight: usize,
}

impl TabState {
    /// 写入新 HTML 并自增修订号。**所有 `tab.html` 的运行期变更必须走这里**
    /// (open/navigate 构造期除外,它们随后同步写 `layout_rev`),
    /// 否则 `ensure_layout_locked` 的「快照×布局」一致性守卫会漏判。
    pub fn set_html(&mut self, html: String) {
        self.html = html;
        self.html_rev = self.html_rev.wrapping_add(1);
    }

    /// 解析当前 HTML 并落入运行时控件覆盖，供样式/布局/快照共享同一状态。
    pub fn document_with_state(&self) -> document::Document {
        let mut doc = document::Document::parse(&self.html, &self.url);
        doc.apply_widget_state(
            &self.form_values,
            &self.widget_open,
            &self.pseudo_states,
            self.datalist.as_ref(),
        );
        doc.apply_caret_state(&self.form_caret);
        doc.apply_scroll_state(&self.scroll_offsets);
        if let Some(sel) = &self.page_sel {
            doc.apply_page_sel(sel);
        }
        doc
    }
}

#[derive(Debug, Clone)]
pub struct WindowState {
    pub id: String,
    pub tabs: Vec<String>,
    pub active: Option<String>,
    pub width: u32,
    pub height: u32,
    pub created_at_ms: u128,
}

#[derive(Debug, Default)]
pub struct Inner {
    pub windows: Vec<WindowState>,
    pub tabs: HashMap<String, TabState>,
    pub cookies: CookieJar,
    pub local_storage: HashMap<String, HashMap<String, String>>,
    /// 全内存 HTTP 缓存（阶段四第二步接入主管线）。
    pub http_cache: cache::HttpCache,
    pub seq: u64,
    pub window_seq: u64,
    pub started_ms: u128,
}

#[derive(Debug, Clone)]
pub struct PageSummary {
    pub page_id: String,
    pub url: String,
    pub title: String,
    pub created_at: u128,
    pub foreground: bool,
    pub window_id: String,
}

#[derive(Debug, Clone)]
pub struct OpenOutcome {
    pub page_id: String,
    pub title: String,
    pub final_url: String,
    pub request_url: String,
    pub mode: String,
    pub mode_hint: Option<String>,
    pub reused: bool,
    pub status: u16,
    pub content_type: String,
    pub source_bytes: usize,
    pub redirect_chain: Vec<String>,
    pub window_id: String,
    pub viewport_width: u32,
    pub viewport_height: u32,
    /// 页面申请过 WebGL 上下文(>0 = 站点依赖 WebGL 绘制,内核无 WebGL → 该区域画不出)。
    pub webgl_asked: u32,
    /// 页面内 iframe 数量(第 174 轮:内核不渲染 iframe 内容,如实计数告知,
    /// 免得 Agent 对 iframe 里的登录/验证码区域反复重试)。
    pub iframe_count: usize,
    /// 页面内 `<video>` 数量:内核不渲染视频画面(hero 背景视频区域会留
    /// 纯色/空白)。如实计数告知,与 webgl/iframe 同款边界上报。
    pub video_count: usize,
    /// 脚本管线是否被截断(墙钟预算耗尽或 wait_until 轮次到点)。截断事实
    /// 必须在 open 响应里直接可见(2026-11-01:免得 Agent 再补一轮
    /// inspect(page_health) 才发现首屏没跑完)。
    pub script_budget_cut: bool,
    /// 截断时仍未取回执行的脚本数(正常收敛 = 0)。
    pub script_pending: usize,
    /// 各阶段耗时账单(毫秒,2026-11-01 天猫二轮):`fetch_ms/styles_ms/
    /// pipeline_ms/images_ms/layout_ms/total_ms`(+ 复用路径 `pump_ms`)与
    /// `pipeline_rounds/scripts_executed`。Agent 据此自校准「要不要带更大
    /// timeout_ms 重开 / 是否已被截断」;缺 `..Default` 旧调用点由
    /// `OpenOutcome::default_timings()` 兜底空对象。
    pub timings: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct NavOutcome {
    pub page_id: String,
    pub url: String,
    pub requested_url: String,
    pub title: String,
    pub status: u16,
    pub content_type: String,
    pub source_bytes: usize,
    pub redirect_chain: Vec<String>,
}

/// 导航预算与就绪语义提示(2026-11-01 天猫真机:`open` 契约落地)。
///
/// 真机实测:`open(timeout_ms=45000, wait_until=domcontentloaded)` 实际
/// 282.9s 才返回 —— `wait_until` 只写在 schema 里零消费,`timeout_ms` 只约束
/// 网络抓取,脚本管线独立吃满 240s(+2×60s 延长)预算。本结构把调用方语义
/// 带进管线种子:
/// - `whole_call_budget_ms`:**显式**传入 `timeout_ms` 时给出,作为整次调用的
///   墙钟上限(抓取已耗时从预算里扣除,管线只吃剩余);未显式传入为 `None`,
///   维持「抓取 60s + 管线默认预算」的旧行为,避免重站点回归;
/// - `wait_until`:`domcontentloaded` → 管线 `max_rounds=1`(首轮同步脚本,
///   约当 DOMContentLoaded 语义,快速返回,pending 由 `script_pending` /
///   `script_budget_cut` 如实上报);`networkidle`/缺省 → 整管线。
#[derive(Debug, Clone, Default)]
pub struct NavHints {
    /// `"load"` / `"domcontentloaded"` / `"networkidle"` / 空(缺省)
    pub wait_until: &'static str,
    /// 整次调用墙钟上限(毫秒);仅调用方显式传 `timeout_ms` 时设置
    pub whole_call_budget_ms: Option<u64>,
    /// 强制重载(2026-11-01 天猫二轮):精确同 URL 复用默认改为**廉价复用**
    /// (小额补泵 + 返回现态,不重新导航);调用方要求全新加载时置 true,
    /// 走既有全量导航(重站全管线分钟级,只在明确要新页面时才付这笔钱)。
    pub force_reload: bool,
}

impl NavHints {
    /// 显式整调用预算的**剩余量**(毫秒);未显式传预算返回 None。
    /// 用途(P1-4):管线之外的后置阶段(样式预取/图片/重排)都以它夹紧,
    /// 让 `timeout_ms` 真正是整次调用的墙钟上界,而不是只管脚本管线。
    fn whole_call_remaining(&self, started: std::time::Instant) -> Option<u64> {
        self.whole_call_budget_ms
            .map(|total| total.saturating_sub(started.elapsed().as_millis() as u64))
    }

    /// 按 hints 把管线种子夹紧:轮次与墙钟预算、宽限都受调用方语义约束。
    fn pipeline_seed(&self, started: std::time::Instant) -> script::PipelineSeed {
        let mut seed = script::PipelineSeed::default();
        if self.wait_until == "domcontentloaded" {
            seed.max_rounds = 1;
        }
        if let Some(total) = self.whole_call_budget_ms {
            let elapsed = started.elapsed().as_millis() as u64;
            let remaining = total.saturating_sub(elapsed);
            // 预算必须是正数(0 语义是「用默认」,不能让夹紧退化成默认大预算)
            seed.budget_ms = remaining.clamp(1, u64::MAX);
            // 宽限续等也计入调用方预算:给 2s 小宽限,防「线程在跑被判超时」
            // 又不至于把整次调用拖出预算(默认宽限 60s 会破约)。
            seed.grace_ms = 2_000;
            // 硬预算:到点即截断,跳过「有待取回脚本就顺延 60s」续期。
            seed.hard_deadline = true;
        }
        seed
    }
}

#[derive(Debug, Clone, Default)]
pub struct CloseOutcome {
    pub closed: Vec<String>,
    pub message: String,
}

/// 统一错误:`code` 与 `MCP_Web_LsmAgentBrowser_Use` 信封错误码一致。
#[derive(Debug, Clone)]
pub struct EngineError {
    pub code: i32,
    pub message: String,
    pub data: Value,
}

impl EngineError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: json!({}),
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }

    pub fn page_not_found(page_id: &str) -> Self {
        // 跨工具 page_id 误用诊断(实测 QC 曾建议把 Chrome 的 c_ page_id 传给本工具):
        // 两浏览器工具页面注册表独立,直接点破比干巴巴的「无效」省一轮试错。
        if let Some(rest) = page_id.strip_prefix("c_") {
            if rest.chars().all(|c| c.is_ascii_hexdigit()) {
                return Self::new(
                    2000,
                    "页面不存在:该 page_id 属于 MCP_Web_Chrome_Use 的注册表\
                     (两浏览器工具页面注册表独立,page_id 不跨工具复用)。\
                     请用 action=open 重新打开页面,或 action=list 查看本工具存活页面",
                )
                .with_data(json!({"page_id": page_id, "cross_tool_page_id": true}));
            }
        }
        Self::new(2000, "page_id 无效或页面已关闭").with_data(json!({"page_id": page_id}))
    }

    pub fn action_failed(message: impl Into<String>) -> Self {
        Self::new(2002, message)
    }

    pub fn param(message: impl Into<String>) -> Self {
        Self::new(1001, message)
    }
}

pub type EngineResult<T> = Result<T, EngineError>;

/// 查看器「跟随页」显示语义的唯一真源(2026-11-02 天猫一轮:查看器白板根因)。
///
/// 旧语义:查看器恒显「前台页」(= 最近 open 的页)——Plan 侦察连开骨架屏/错误页
/// 会把人工视口劫走,Agent 回工作页也被 1Hz 心跳用前台帧每秒盖回,人工看到的是
/// 「白板」。新语义:显示跟随**最近一次有意义交互的页**——工具动作的目标页
/// (`note_action_begin/end` 落账)、人工在查看器里的操作页(`handle_view_command`),
/// 无记录时才回退前台页。设计见 `docs/MCP_Web_LsmAgentBrowser_Use/05-窗口交互与页面观察/38-查看器跟随页语义与天猫白板根因.md`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ViewFollow {
    /// 跟随指定标签页(不在场/非 headed 时由心跳回退前台页)。
    Page(String),
    /// 加载首帧已推给查看器:心跳暂停推送,防旧页帧一秒后盖掉加载页;
    /// 超过 TTL(整管线预算上限)视为孤儿导航,回退前台链。
    PendingLoading { since_ms: u128 },
}

pub struct LsmBrowserManager {
    inner: Mutex<Inner>,
    http: net::HttpClient,
    font: render::FontSet,
    /// 有头帧查看器(惰性;headed 模式首个页面打开时启动,失败静默降级)。
    view: std::sync::Mutex<Option<view::ViewSink>>,
    /// 有头查看器的交互命令通道(查看器工具栏/地址栏 → 引擎)。
    view_cmd_tx: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<view::ViewCommand>>>,
    /// 查看器跟随页(显示语义见 `ViewFollow`;仅 1Hz 心跳消费)。
    view_follow: std::sync::Mutex<Option<ViewFollow>>,
    /// 自身弱引用(查看器命令处理任务用它回到引擎;非 Arc 场景为空)。
    self_weak: std::sync::Mutex<std::sync::Weak<LsmBrowserManager>>,
    /// 两条告知栏 1Hz 心跳的单实例闸(`page_bars.rs::ensure_bars_ticker`)。
    bars_ticker_started: std::sync::atomic::AtomicBool,
    /// 人类输入会话态(查看器键鼠;按下/双击/hover 去重,`human_input/mod.rs`)。
    human: std::sync::Mutex<human_input::HumanSession>,
    /// 查看器窗口被人工关闭(`viewer_closed` 信号 / `close_view`)。
    /// 置位期间 `push_frame` 不自动重生窗口(不与用户抢窗口);
    /// 工具动作路径(`ensure_view`/`revive_viewer`)仍可按设计复活。
    view_user_closed: std::sync::atomic::AtomicBool,
    /// 查看器子进程启动次数(首启不计;意外死亡自动重生上限 3 次/会话,
    /// 防崩溃循环;超限回退纯无头并打 WARN,见 `view::bridge::push_frame`)。
    view_respawns: std::sync::atomic::AtomicUsize,
}

/// 兼容旧调用点的类型名。
pub type BrowserManager = LsmBrowserManager;

static GLOBAL: Lazy<Arc<LsmBrowserManager>> =
    Lazy::new(|| LsmBrowserManager::new_arc());

/// 页面序号分段源(每管理器 `0x100000` 页额度,见 [`LsmBrowserManager::new`])。
static MANAGER_SEQ_SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl LsmBrowserManager {
    pub fn new() -> Self {
        // 页面序号按管理器分段(<<20):多管理器并行(单元/集成测试)不得共用
        // page_id —— web_control 等进程级全局注册表按 (引擎, page_id) 键控,
        // 同名页面会互相覆盖/清除控制权态(并行单测曾偶发 agent_control 误拦)。
        // 生产进程只有单管理器(种子 0),页 id 与旧格式完全一致。
        let seq_base = MANAGER_SEQ_SEED
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            << 20;
        Self {
            inner: Mutex::new(Inner {
                started_ms: now_ms(),
                seq: seq_base,
                ..Default::default()
            }),
            http: net::HttpClient::new(DEFAULT_UA),
            font: render::FontSet::load(),
            view: std::sync::Mutex::new(None),
            view_cmd_tx: std::sync::Mutex::new(None),
            view_follow: std::sync::Mutex::new(None),
            self_weak: std::sync::Mutex::new(std::sync::Weak::new()),
            bars_ticker_started: std::sync::atomic::AtomicBool::new(false),
            human: std::sync::Mutex::new(human_input::HumanSession::default()),
            view_user_closed: std::sync::atomic::AtomicBool::new(false),
            view_respawns: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// 构造 `Arc` 包裹的管理器:查看器命令任务需要弱引用回到引擎。
    pub fn new_arc() -> Arc<Self> {
        let me = Arc::new(Self::new());
        *me.self_weak.lock().unwrap() = Arc::downgrade(&me);
        me
    }

    /// 「快照×布局」一致性守卫:`tab.html` 在上次布局之后被改写
    /// (脚本动作/管线补齐)时重算布局。
    ///
    /// 布局盒与快照节点都以 `stable_selector`(路径含 `:nth-of-type`)为键,
    /// HTML 一变位置键整体漂移;旧 `rect_map` × 新快照 = 键全错位,
    /// JS 桥 `getBoundingClientRect/clientWidth` 全 0(实测猎聘首页:
    /// `run_pending_scripts` 改 html 不重排,宿主 rect_for 正常、
    /// 页面脚本测量全 0,二维码 canvas/地图容器测量链全断)。
    /// eval/dispatch/screenshot/inspect 入口都必须先过这道守卫。
    pub fn ensure_layout_locked(&self, tab: &mut TabState) {
        if tab.layout_rev == tab.html_rev {
            return;
        }
        let doc = tab.document_with_state();
        let media = style::MediaCtx::new(tab.viewport.width as f64, tab.viewport.height as f64);
        let sizes = images::ImageSizes::from_store(&tab.images);
        tab.layout = layout::compute_full(
            &doc,
            tab.viewport.width as f64,
            &self.font,
            &media,
            &sizes,
        );
        tab.layout_rev = tab.html_rev;
    }

    /// 有头查看器 ↔ 引擎的桥接方法(查看器生命周期 / 帧推送 / 命令处理)
    /// 已拆到 `view::bridge`(单文件行数上限)。

    pub fn global() -> Arc<Self> {
        GLOBAL.clone()
    }

    /// 有头查看器是否在位(headed 但查看器已退出、hidden 从未启动都返回 false)。
    /// `request_human` 用它判断「人工有没有可见窗口可操作」——隐藏模式下弹窗
    /// 提示人工「请在浏览器窗口完成验证」是不可能指令,必须提前告知模型。
    pub fn viewer_alive(&self) -> bool {
        self.view
            .lock()
            .map(|v| v.as_ref().map(|s| s.is_alive()).unwrap_or(false))
            .unwrap_or(false)
    }

    pub fn user_agent(&self) -> &str {
        self.http.user_agent()
    }

    pub fn font(&self) -> &render::FontSet {
        &self.font
    }

    pub async fn shutdown(&self) {
        let mut inner = self.inner.lock().await;
        let ids: Vec<String> = inner.tabs.keys().cloned().collect();
        shutdown_js_handles(&inner, &ids);
        inner.tabs.clear();
        inner.windows.clear();
        inner.cookies.clear();
    }

    /// 兼容旧退出路径:新内核没有子进程,无需同步清理。
    pub fn cleanup_sync() {}

    pub async fn has_browser(&self) -> bool {
        !self.inner.lock().await.tabs.is_empty()
    }

    pub async fn current_mode(&self) -> Option<BrowserMode> {
        self.inner
            .lock()
            .await
            .tabs
            .values()
            .next()
            .map(|t| t.mode)
    }

    pub async fn page_exists(&self, page_id: &str) -> bool {
        self.inner.lock().await.tabs.contains_key(page_id)
    }

    /// 丢弃失效的 JS 线程句柄(线程超时/退出后由交互层调用)。
    pub async fn clear_js(&self, page_id: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(tab) = inner.tabs.get_mut(page_id) {
            tab.js = None;
        }
    }

    pub async fn list_pages(&self) -> Vec<PageSummary> {
        let inner = self.inner.lock().await;
        let mut out: Vec<PageSummary> = inner
            .tabs
            .values()
            .map(|t| PageSummary {
                page_id: t.id.clone(),
                url: t.url.clone(),
                title: t.title.clone(),
                created_at: t.created_at_ms,
                foreground: t.foreground,
                window_id: t.window_id.clone(),
            })
            .collect();
        out.sort_by_key(|p| p.created_at);
        out
    }

    pub async fn page_summary(&self, page_id: &str) -> Option<PageSummary> {
        let inner = self.inner.lock().await;
        inner.tabs.get(page_id).map(|t| PageSummary {
            page_id: t.id.clone(),
            url: t.url.clone(),
            title: t.title.clone(),
            created_at: t.created_at_ms,
            foreground: t.foreground,
            window_id: t.window_id.clone(),
        })
    }

    /// 打开页面(可复用同 URL 页面)。
    pub async fn new_page(
        &self,
        url: &str,
        mode: BrowserMode,
        user_agent: Option<&str>,
        width: Option<u32>,
        height: Option<u32>,
        new_window: bool,
        reuse: bool,
        timeout_ms: u64,
        hints: &NavHints,
    ) -> EngineResult<OpenOutcome> {
        self.new_page_in(None, url, mode, user_agent, width, height, new_window, reuse, timeout_ms, hints)
            .await
    }

    /// open(page_id=X, url=Y):显式指定承载页。URL 与该页不同时**在该页内
    /// 原地导航**,而不是按站点匹配找不到就悄悄另开新页 —— 模型普遍把
    /// page_id 当「在这个页里打开」用,落空后另开的新页没有任何回执指向,
    /// 后续 inspect/control 仍打在旧页上,表现为「页面跳走/操作不生效」
    /// (2026-11-05 闲鱼实测:open(page_id,登录URL) 后 Agent 一直看首页)。
    pub async fn new_page_in(
        &self,
        prefer_page: Option<&str>,
        url: &str,
        mode: BrowserMode,
        user_agent: Option<&str>,
        width: Option<u32>,
        height: Option<u32>,
        new_window: bool,
        reuse: bool,
        timeout_ms: u64,
        hints: &NavHints,
    ) -> EngineResult<OpenOutcome> {
        let started = std::time::Instant::now();
        let requested = urlutil::resolve("", url)
            .ok_or_else(|| EngineError::param(format!("URL 非法:{url}")))?;
        if !urlutil::is_navigable(&requested) {
            return Err(EngineError::param(format!(
                "不支持的协议:{requested}(允许 http/https/file/data/about)"
            )));
        }
        let mut inner = self.inner.lock().await;
        let max_tabs = env_limit("LAEW_WEB_MAX_TABS", MAX_TABS_DEFAULT, 2);
        let max_windows = env_limit("LAEW_WEB_MAX_WINDOWS", MAX_WINDOWS_DEFAULT, 1);
        if new_window && inner.windows.len() >= max_windows {
            return Err(EngineError::new(1001, "逻辑窗口数量达到上限")
                .with_data(json!({"windows": inner.windows.len(), "max": max_windows,
                                   "hint": "先 close 不需要的 page_id 再 open(new_window=true)"})));
        }
        if reuse && !new_window {
            // 显式 page_id 优先:调用方点名要在这一页打开(同 URL 走廉价复用,
            // 不同 URL 原地导航);未点名才按 URL/站点全局找可复用页。
            let preferred = match prefer_page {
                Some(p) if inner.tabs.contains_key(p) => Some(p.to_string()),
                _ => None,
            };
            if let Some(existing) = preferred.or_else(|| find_reusable(&inner, &requested)) {
                // 2026-11-01 天猫二轮:精确同 URL 复用改**廉价复用** ——
                // 原实现无条件全量重导航:重站(天猫)全管线 240s+顺延 60s+图片 20s
                // ≈ 327.7s 黑盒,且导航会 shutdown 旧 JS 线程,把首开后台自建的
                // 半成品进度整页丢弃。现语义与 Chrome 工具「同 URL 自动复用无主页」
                // 对齐:小额补泵(≤8s)推进半成品页,返回现态;要全新页面用
                // `params.reload=true`(NavHints::force_reload)。同源**不同 URL**
                // 仍走全量导航(本来就是去新页面)。
                let exact_hit = inner
                    .tabs
                    .get(&existing)
                    .map(|t| urlutil::canonical(&t.url) == urlutil::canonical(&requested))
                    .unwrap_or(false);
                if exact_hit && !hints.force_reload {
                    let pid = existing.clone();
                    drop(inner);
                    let pump_started = std::time::Instant::now();
                    let pump_budget = api::interaction_budget_ms().min(8_000);
                    api::run_pending_scripts(self, &pid, pump_budget, 1_000).await;
                    let pump_ms = pump_started.elapsed().as_millis() as u64;
                    let mut inner = self.inner.lock().await;
                    let (window_id, vw, vh, webgl_asked, tab_mode, script_budget_cut, script_pending) = inner
                        .tabs
                        .get(&pid)
                        .map(|t| {
                            (
                                t.window_id.clone(),
                                t.viewport.width,
                                t.viewport.height,
                                t.webgl_asked,
                                t.mode,
                                t.script_budget_cut,
                                t.script_pending,
                            )
                        })
                        .unwrap_or((String::new(), 0, 0, 0, mode, false, 0));
                    let iframe_count = inner
                        .tabs
                        .get(&pid)
                        .and_then(|t| document::Document::parse(&t.html, &t.url).select("iframe").ok().map(|v| v.len()))
                        .unwrap_or(0);
                    let video_count = inner
                        .tabs
                        .get(&pid)
                        .and_then(|t| document::Document::parse(&t.html, &t.url).select("video").ok().map(|v| v.len()))
                        .unwrap_or(0);
                    let (title, final_url, status, content_type, source_bytes, redirect_chain) = inner
                        .tabs
                        .get(&pid)
                        .map(|t| {
                            (
                                t.title.clone(),
                                t.url.clone(),
                                t.status,
                                t.content_type.clone(),
                                t.source_bytes,
                                t.redirect_chain.clone(),
                            )
                        })
                        .unwrap_or_else(|| {
                            (String::new(), requested.clone(), 0, String::new(), 0, Vec::new())
                        });
                    tracing::info!(
                        page_id = %pid,
                        pump_ms,
                        budget_cut = script_budget_cut,
                        pending = script_pending,
                        "open 精确同 URL 廉价复用(未重导航;强制重载用 reload=true)"
                    );
                    return Ok(OpenOutcome {
                        page_id: pid,
                        title,
                        final_url,
                        request_url: requested,
                        mode: tab_mode.as_str().to_string(),
                        mode_hint: mode_hint(mode),
                        reused: true,
                        status,
                        content_type,
                        source_bytes,
                        redirect_chain,
                        window_id,
                        viewport_width: vw,
                        viewport_height: vh,
                        webgl_asked,
                        iframe_count,
                        video_count,
                        script_budget_cut,
                        script_pending,
                        timings: json!({
                            "reused": true,
                            "pump_ms": pump_ms,
                            "total_ms": started.elapsed().as_millis() as u64,
                            "note": "廉价复用:未重新导航;强制重载 open params.reload=true",
                        }),
                    });
                }
                let pid = existing.clone();
                let ua = inner
                    .tabs
                    .get(&pid)
                    .map(|t| t.user_agent.clone())
                    .unwrap_or_else(|| user_agent.unwrap_or(DEFAULT_UA).to_string());
                match self
                    .navigate_locked(&mut inner, &pid, &requested, timeout_ms, true, &ua, hints)
                    .await
                {
                    Ok(nav) => {
                        let (window_id, vw, vh, webgl_asked, tab_mode, script_budget_cut, script_pending) = inner
                            .tabs
                            .get(&pid)
                            .map(|t| {
                                (
                                    t.window_id.clone(),
                                    t.viewport.width,
                                    t.viewport.height,
                                    t.webgl_asked,
                                    t.mode,
                                    t.script_budget_cut,
                                    t.script_pending,
                                )
                            })
                            .unwrap_or((String::new(), 0, 0, 0, mode, false, 0));
                        let iframe_count = inner
                            .tabs
                            .get(&existing)
                            .and_then(|t| document::Document::parse(&t.html, &t.url).select("iframe").ok().map(|v| v.len()))
                            .unwrap_or(0);
                        let video_count = inner
                            .tabs
                            .get(&existing)
                            .and_then(|t| document::Document::parse(&t.html, &t.url).select("video").ok().map(|v| v.len()))
                            .unwrap_or(0);
                        return Ok(OpenOutcome {
                            page_id: pid,
                            title: nav.title,
                            final_url: nav.url,
                            request_url: nav.requested_url,
                            mode: tab_mode.as_str().to_string(),
                            mode_hint: mode_hint(mode),
                            reused: true,
                            status: nav.status,
                            content_type: nav.content_type,
                            source_bytes: nav.source_bytes,
                            redirect_chain: nav.redirect_chain,
                            window_id,
                            viewport_width: vw,
                            viewport_height: vh,
                            webgl_asked: webgl_asked,
                            iframe_count,
                            video_count,
                            script_budget_cut,
                            script_pending,
                            timings: json!({
                                "reused": true,
                                "navigated": true,
                                "total_ms": started.elapsed().as_millis() as u64,
                            }),
                        });
                    }
                    Err(err) => {
                        tracing::warn!(
                            page_id = %pid,
                            url = %requested,
                            error = %err.message,
                            "复用页面导航失败,剔除该页后新建页面"
                        );
                        shutdown_js_handles(&inner, &[pid.clone()]);
                        inner.tabs.remove(&pid);
                        close_from_windows(&mut inner, &[pid.clone()]);
                        clear_session_if_empty(&mut inner);
                    }
                }
            }
        }

        if !new_window && inner.tabs.len() >= max_tabs {
            return Err(EngineError::new(1001, "标签页数量达到上限")
                .with_data(json!({"tabs": inner.tabs.len(), "max": max_tabs,
                                   "hint": "先 close 不需要的 page_id,或复用已有页面"})));
        }
        let ua = user_agent.unwrap_or(DEFAULT_UA).to_string();
        // 视口尺寸:缺省 1920×1080(1080p)。**headed 模式**再按主显示器可用区域
        // 夹紧 —— 查看器窗口按视口开,屏幕比 1080p 小的时候窗口会超出屏幕,
        // 底部区域人工根本看不到(缺区域/渲染不全)。hidden 模式不夹:
        // 没有窗口,逻辑视口与物理屏无关,夹了反而影响站点布局。
        let (mut vp_w, mut vp_h) = (
            width.unwrap_or(DEFAULT_VIEWPORT_W).clamp(320, 7680),
            height.unwrap_or(DEFAULT_VIEWPORT_H).clamp(240, 4320),
        );
        if mode.is_headed() {
            let (fw, fh) = screen::fit_viewport_to_screen(vp_w, vp_h);
            if (fw, fh) != (vp_w, vp_h) {
                tracing::info!(
                    requested_w = vp_w,
                    requested_h = vp_h,
                    fitted_w = fw,
                    fitted_h = fh,
                    "headed 视口按屏幕可用区夹紧(避免查看器窗口超出屏幕)"
                );
            }
            vp_w = fw;
            vp_h = fh;
        }
        let viewport = Viewport {
            width: vp_w,
            height: vp_h,
            ..Default::default()
        };
        let (resp, from_cache) = self.fetch_main(&mut inner, &requested, &ua, timeout_ms).await?;
        let t_fetch = std::time::Instant::now();
        // 第 175 轮:非 HTML 主文档(图片/纯文本/JSON)合成展示页,避免黑屏。
        let raw_html = synthesize_non_html_document(&resp.content_type(), &resp.bytes, &resp.final_url)
            .unwrap_or_else(|| resp.text());
        let content_type = resp.content_type();
        let empty_session = HashMap::new();
        // 阶段四第二步：解析主文档 CSP 头，供脚本管线门禁。
        let csp_policy = parse_csp_header(&resp.headers);
        // 外链样式表先预取内联,脚本执行与布局都消费同一份样式。
        // 显式整调用预算时请求级超时按剩余量夹紧(P1-4:预算覆盖全部后置阶段)。
        let css_timeout = match hints.whole_call_remaining(started) {
            Some(remaining) => timeout_ms.min(remaining.max(1_000)),
            None => timeout_ms,
        };
        let resources_out = resources::load_stylesheets(
            &mut inner,
            &self.http,
            &raw_html,
            &resp.final_url,
            &ua,
            css_timeout,
        )
        .await;
        let t_styles = std::time::Instant::now();
        // 脚本前预布局(空图片尺寸):JS 桥的 clientWidth 等几何属性需要真实矩形,
        // 容器 CSS 定宽场景(BMap 初始化读 #MapHolder 尺寸)不依赖图片固有尺寸。
        let pre_rects = {
            let pre_doc = document::Document::parse(&resources_out.html, &resp.final_url);
            let pre_media = style::MediaCtx::new(viewport.width as f64, viewport.height as f64);
            let pre_sizes = images::ImageSizes::default();
            layout::compute_full(&pre_doc, viewport.width as f64, &self.font, &pre_media, &pre_sizes)
                .rect_map()
        };
        // 重站点脚本管线可能达到分钟级:先显示加载首帧,再执行脚本并替换为真实页面。
        if mode.is_headed() {
            self.ensure_loading_view(&resp.final_url, viewport.width, viewport.height);
        }
        let pipeline = script::run_pipeline(
            &mut inner,
            &self.http,
            &resources_out.html,
            &resp.final_url,
            &ua,
            (viewport.width, viewport.height),
            &empty_session,
            csp_policy.as_ref(),
            &pre_rects,
            hints.pipeline_seed(started),
        )
        .await;
        let t_pipeline = std::time::Instant::now();
        let html = pipeline.html;
        dump_html_if_env(&html);
        // 阶段三第三步:预取图片(替换元素固有尺寸参与布局)。
        // P1-4(2026-11-01 天猫二轮):显式整调用预算必须覆盖图片阶段 ——
        // 真机实测 timeout_ms=60000 返回 83.9s,超预算 40% 全部超在这里
        // (图片吃独立 20s 预算)。剩余预算归零时直接跳过(skipped 如实记账)。
        let images_budget_cap = hints.whole_call_remaining(started);
        let images_out = images::load_images(
            &mut inner,
            &self.http,
            &html,
            &resp.final_url,
            &ua,
            timeout_ms,
            &images::ImageStore::new(),
            viewport.width as f64,
            images_budget_cap,
        )
        .await;
        let t_images = std::time::Instant::now();
        let mut doc = document::Document::parse(&html, &resp.final_url);
        doc.apply_widget_state(&pipeline.form_values, &HashMap::new(), &HashMap::new(), None);
        let title = pipeline.title.clone().unwrap_or_else(|| doc.title());
        let media = style::MediaCtx::new(viewport.width as f64, viewport.height as f64);
        let layout = layout::compute_full(
            &doc,
            viewport.width as f64,
            &self.font,
            &media,
            &images_out.sizes,
        );
        let t_layout = std::time::Instant::now();
        let page_id = format!("p_{:08x}", inner.seq + 1);
        inner.seq += 1;
        let window_id = if new_window || inner.windows.is_empty() {
            inner.window_seq += 1;
            let wid = format!("w_{:04x}", inner.window_seq);
            inner.windows.push(WindowState {
                id: wid.clone(),
                tabs: Vec::new(),
                active: None,
                width: viewport.width,
                height: viewport.height,
                created_at_ms: now_ms(),
            });
            wid
        } else {
            inner
                .windows
                .last()
                .map(|w| w.id.clone())
                .unwrap_or_else(|| "w_0001".to_string())
        };
        // 同窗口新页:前台让给新页。
        for w in inner.windows.iter_mut() {
            if w.id == window_id {
                w.active = Some(page_id.clone());
            }
        }
        for t in inner.tabs.values_mut() {
            if t.window_id == window_id {
                t.foreground = false;
            }
        }
        // 管线观测快照(在 TabState 构造 move `pipeline.executed/js` 前取值)。
        let pipeline_rounds = pipeline.rounds;
        let scripts_executed = pipeline.executed.len();
        let tab = TabState {
            id: page_id.clone(),
            window_id: window_id.clone(),
            url: resp.final_url.clone(),
            title: title.clone(),
            status: resp.status,
            content_type: content_type.clone(),
            html,
            redirect_chain: resp.redirect_chain.clone(),
            source_bytes: resp.bytes.len(),
            history: vec![HistoryEntry {
                url: resp.final_url.clone(),
                title: title.clone(),
            }],
            history_index: 0,
            session_storage: HashMap::new(),
            viewport,
            scroll_x: 0.0,
            scroll_y: 0.0,
            scroll_offsets: HashMap::new(),
            network: {
                let mut recs = vec![NetworkRecord {
                    url: requested.clone(),
                    method: "GET".to_string(),
                    status: resp.status,
                    content_type: content_type.clone(),
                    bytes: resp.bytes.len(),
                    elapsed_ms: resp.elapsed_ms,
                    redirect: resp.redirect_chain.len() > 1,
                    from_cache,
                    at_ms: now_ms(), req_body: None,}];
                recs.extend(resources_out.network.clone());
                recs.extend(pipeline.network.clone());
                recs.extend(images_out.network.clone());
                recs
            },
            console: {
                let mut logs = resources_out.console.clone();
                logs.extend(pipeline.console.clone());
                logs.extend(images_out.console.clone());
                logs
            },
            form_values: pipeline.form_values.clone(),
            widget_open: HashMap::new(),
            pseudo_states: HashMap::new(),
            form_caret: HashMap::new(),
            page_sel: None,
            find: find::FindState::default(),
            focused: None,
            mouse: None,
            layout,
            // open():布局与 html 同源计算,两者修订号对齐(守卫见 `ensure_layout_locked`)。
            html_rev: 1,
            layout_rev: 1,
            created_at_ms: now_ms(),
            last_active_ms: now_ms(),
            foreground: true,
            user_agent: ua,
            mode,
            guard: GuardState {
                mode: "open".to_string(),
                ..Default::default()
            },
            bars: page_bars::PageBars::default(),
            highlight: false,
            auto_follow: false,
            js: pipeline.js,
            executed_scripts: pipeline.executed,
            images: images_out.store,
            security: security::SecurityContext::new(),
            webgl_asked: pipeline.webgl_asked,
            script_budget_cut: pipeline.budget_cut,
            script_pending: pipeline.pending_scripts,
            datalist: None,
            select_anchor: HashMap::new(),
            human_zoom: 1.0,
        };
        inner.tabs.insert(page_id.clone(), tab);
        // 阶段四第二步：CSP 策略与页面源写入安全上下文（供 fetch/XHR connect-src 门禁）。
        if let Some(tab) = inner.tabs.get_mut(&page_id) {
            tab.security.set_page_origin(&resp.final_url);
            if let Some(policy) = &csp_policy {
                tab.security.cache_csp(&resp.final_url, policy.clone());
            }
        }
        if let Some(w) = inner.windows.iter_mut().find(|w| w.id == window_id) {
            w.tabs.push(page_id.clone());
        }
        // 有头:首个页面打开时启动查看器并推首帧(失败静默降级纯无头)。
        if mode.is_headed() {
            if let Some(tab) = inner.tabs.get(&page_id) {
                self.ensure_view(tab);
            }
        }
        let timings = json!({
            "fetch_ms": t_fetch.duration_since(started).as_millis() as u64,
            "styles_ms": t_styles.duration_since(t_fetch).as_millis() as u64,
            "pipeline_ms": t_pipeline.duration_since(t_styles).as_millis() as u64,
            "images_ms": t_images.duration_since(t_pipeline).as_millis() as u64,
            "layout_ms": t_layout.duration_since(t_images).as_millis() as u64,
            "total_ms": t_layout.duration_since(started).as_millis() as u64,
            "pipeline_rounds": pipeline_rounds,
            "scripts_executed": scripts_executed,
        });
        tracing::info!(
            url = %resp.final_url,
            budget_cut = pipeline.budget_cut,
            pending = pipeline.pending_scripts,
            rounds = pipeline_rounds,
            executed = scripts_executed,
            total_ms = t_layout.duration_since(started).as_millis() as u64,
            "open 完成(阶段耗时见响应 timings)"
        );
        Ok(OpenOutcome {
            page_id,
            title,
            final_url: resp.final_url,
            request_url: requested,
            mode: mode.as_str().to_string(),
            mode_hint: mode_hint(mode),
            reused: false,
            status: resp.status,
            content_type,
            source_bytes: resp.bytes.len(),
            redirect_chain: resp.redirect_chain,
            window_id,
            viewport_width: viewport.width,
            viewport_height: viewport.height,
            webgl_asked: pipeline.webgl_asked,
            iframe_count: doc.select("iframe").map(|v| v.len()).unwrap_or(0),
            video_count: doc.select("video").map(|v| v.len()).unwrap_or(0),
            script_budget_cut: pipeline.budget_cut,
            script_pending: pipeline.pending_scripts,
            timings,
        })
    }

    pub async fn navigate(
        &self,
        page_id: &str,
        url: &str,
        timeout_ms: u64,
    ) -> EngineResult<NavOutcome> {
        let requested = urlutil::resolve("", url)
            .ok_or_else(|| EngineError::param(format!("URL 非法:{url}")))?;
        let mut inner = self.inner.lock().await;
        let ua = inner
            .tabs
            .get(page_id)
            .map(|t| t.user_agent.clone())
            .ok_or_else(|| EngineError::page_not_found(page_id))?;
        self.navigate_locked(&mut inner, page_id, &requested, timeout_ms, true, &ua, &NavHints::default())
            .await
    }

    async fn navigate_locked(
        &self,
        inner: &mut Inner,
        page_id: &str,
        url: &str,
        timeout_ms: u64,
        push_history: bool,
        ua: &str,
        hints: &NavHints,
    ) -> EngineResult<NavOutcome> {
        self.load_locked(
            inner,
            page_id,
            url,
            timeout_ms,
            push_history,
            ua,
            "GET",
            None,
            hints,
        )
        .await
    }

    /// 表单提交/带请求体的导航。
    pub async fn submit_form(
        &self,
        page_id: &str,
        url: &str,
        method: &str,
        body: Option<Vec<u8>>,
        timeout_ms: u64,
    ) -> EngineResult<NavOutcome> {
        let mut inner = self.inner.lock().await;
        let ua = inner
            .tabs
            .get(page_id)
            .map(|t| t.user_agent.clone())
            .ok_or_else(|| EngineError::page_not_found(page_id))?;
        self.load_locked(&mut inner, page_id, url, timeout_ms, true, &ua, method, body, &NavHints::default())
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn load_locked(
        &self,
        inner: &mut Inner,
        page_id: &str,
        url: &str,
        timeout_ms: u64,
        push_history: bool,
        ua: &str,
        method: &str,
        body: Option<Vec<u8>>,
        hints: &NavHints,
    ) -> EngineResult<NavOutcome> {
        let started = std::time::Instant::now();
        if !urlutil::is_navigable(url) {
            return Err(EngineError::param(format!("不支持的协议:{url}")));
        }
        if !inner.tabs.contains_key(page_id) {
            return Err(EngineError::page_not_found(page_id));
        }
        let mut request = net::FetchRequest::get(url, timeout_ms.max(1_000));
        request.method = method.to_ascii_uppercase();
        request.body = body;
        request.context = Some(reqctx::RequestContext::subresource("", reqctx::Dest::Document, ua));
        let (resp, from_cache) = if request.method == "GET" {
            // 阶段四第二步：GET 统一走带缓存的取页通道。
            if ua != DEFAULT_UA {
                request
                    .headers
                    .push(("user-agent".to_string(), ua.to_string()));
            }
            self.fetch_main(inner, url, ua, timeout_ms).await?
        } else {
            let resp = self
                .http
                .fetch(request, &mut inner.cookies)
                .await
                .map_err(|e| {
                    EngineError::new(2001, e)
                        .with_data(json!({"reason": "connection_level", "retryable": true}))
                })?;
            (resp, false)
        };
        // 第 175 轮:非 HTML 主文档(图片/纯文本/JSON)合成展示页,避免黑屏。
        let raw_html = synthesize_non_html_document(&resp.content_type(), &resp.bytes, &resp.final_url)
            .unwrap_or_else(|| resp.text());
        let content_type = resp.content_type();
        // 阶段四第二步：解析主文档 CSP 头，供脚本管线门禁与安全上下文缓存。
        let csp_policy = parse_csp_header(&resp.headers);
        // 外链样式表先预取内联(失败降级,不阻断导航)。
        let resources_out = resources::load_stylesheets(
            inner,
            &self.http,
            &raw_html,
            &resp.final_url,
            ua,
            timeout_ms,
        )
        .await;
        // 导航换页:旧 JS 线程全局状态随页面销毁,重建线程。
        if let Some(tab) = inner.tabs.get_mut(page_id) {
            if let Some(old) = tab.js.take() {
                old.shutdown();
            }
        }
        let (viewport_w, viewport_h, session_snapshot) = inner
            .tabs
            .get(page_id)
            .map(|t| (t.viewport.width, t.viewport.height, t.session_storage.clone()))
            .unwrap_or((DEFAULT_VIEWPORT_W, DEFAULT_VIEWPORT_H, HashMap::new()));
        // 脚本前预布局(空图片尺寸):与 open 同理,给 JS 桥喂元素几何。
        let pre_rects = {
            let pre_doc = document::Document::parse(&resources_out.html, &resp.final_url);
            let pre_media = style::MediaCtx::new(viewport_w as f64, viewport_h as f64);
            let pre_sizes = images::ImageSizes::default();
            layout::compute_full(&pre_doc, viewport_w as f64, &self.font, &pre_media, &pre_sizes)
                .rect_map()
        };
        // 有头:长导航先推加载首帧(2026-11-02 天猫一轮 R3:此前 navigate 路径
        // 无加载视图,重导航期间查看器冻结旧帧,人工误判「页面已死」去点刷新,
        // 雪上加霜)。`ensure_loading_view` 同步置 `PendingLoading`,1Hz 心跳暂停
        // 推送;导航收尾 `ensure_view` 把跟随页落回 `Page(目标页)` 后恢复。
        if inner
            .tabs
            .get(page_id)
            .map(|t| t.mode.is_headed())
            .unwrap_or(false)
        {
            self.ensure_loading_view(&resp.final_url, viewport_w, viewport_h);
        }
        let pipeline = script::run_pipeline(
            inner,
            &self.http,
            &resources_out.html,
            &resp.final_url,
            ua,
            (viewport_w, viewport_h),
            &session_snapshot,
            csp_policy.as_ref(),
            &pre_rects,
            hints.pipeline_seed(started),
        )
        .await;
        let html = pipeline.html;
        dump_html_if_env(&html);
        // 阶段三第三步:预取图片(替换元素固有尺寸参与布局)。
        // P1-4:整调用预算同样夹紧图片阶段(与新页路径同语义)。
        let images_budget_cap = hints.whole_call_remaining(started);
        let images_out = images::load_images(
            inner,
            &self.http,
            &html,
            &resp.final_url,
            ua,
            timeout_ms,
            &images::ImageStore::new(),
            viewport_w as f64,
            images_budget_cap,
        )
        .await;
        let mut doc = document::Document::parse(&html, &resp.final_url);
        doc.apply_widget_state(&pipeline.form_values, &HashMap::new(), &HashMap::new(), None);
        let title = pipeline.title.clone().unwrap_or_else(|| doc.title());
        let media = style::MediaCtx::new(viewport_w as f64, viewport_h as f64);
        let layout = layout::compute_full(&doc, viewport_w as f64, &self.font, &media, &images_out.sizes);
        let Some(tab) = inner.tabs.get_mut(page_id) else {
            return Err(EngineError::page_not_found(page_id));
        };
        tab.url = resp.final_url.clone();
        tab.title = title.clone();
        tab.status = resp.status;
        tab.content_type = content_type.clone();
        tab.set_html(html);
        tab.redirect_chain = resp.redirect_chain.clone();
        tab.source_bytes = resp.bytes.len();
        tab.layout = layout;
        tab.layout_rev = tab.html_rev;
        tab.scroll_x = 0.0;
        tab.scroll_y = 0.0;
        tab.form_values = pipeline.form_values.clone();
        tab.widget_open.clear();
        // 第 41 轮:交互态随导航复位(新页面无悬停/焦点)。
        tab.pseudo_states.clear();
        tab.focused = None;
        tab.last_active_ms = now_ms();
        // 阶段四第二步：CSP 策略与页面源写入安全上下文（供 fetch/XHR connect-src 门禁）。
        tab.security.set_page_origin(&resp.final_url);
        if let Some(policy) = &csp_policy {
            tab.security.cache_csp(&resp.final_url, policy.clone());
        }
        tab.console = resources_out.console.clone();
        tab.console.extend(pipeline.console.clone());
        tab.console.extend(images_out.console.clone());
        for rec in resources_out.network {
            record_network(tab, rec);
        }
        tab.js = pipeline.js;
        tab.script_budget_cut = pipeline.budget_cut;
        tab.script_pending = pipeline.pending_scripts;
        tab.executed_scripts = pipeline.executed.clone();
        tab.images = images_out.store;
        for rec in pipeline.network {
            record_network(tab, rec);
        }
        for rec in images_out.network {
            record_network(tab, rec);
        }
        if push_history {
            tab.history.truncate(tab.history_index + 1);
            tab.history.push(HistoryEntry {
                url: resp.final_url.clone(),
                title: title.clone(),
            });
            tab.history_index = tab.history.len().saturating_sub(1);
        }
        // 有头:导航完成后推帧(查看器展示最新页面)。查看器可能中途退出
        // (人工误关窗口/查看器进程崩溃),push_frame 对死句柄是 no-op,页面
        // 从此不可见且永不重生——登录类 HITL 要求人工在窗口扫码,窗口却
        // 再也回不来,任务直接卡死(实测京东登录链)。改走 ensure_view:
        // 幂等,死句柄重建查看器子进程,活句柄等价于推一帧。
        if tab.mode.is_headed() {
            self.ensure_view(tab);
        }
        record_network(
            tab,
            NetworkRecord {
                url: url.to_string(),
                method: method.to_ascii_uppercase(),
                status: resp.status,
                content_type: content_type.clone(),
                bytes: resp.bytes.len(),
                elapsed_ms: resp.elapsed_ms,
                redirect: resp.redirect_chain.len() > 1,
                from_cache,
                at_ms: now_ms(), req_body: None,},
        );
        Ok(NavOutcome {
            page_id: page_id.to_string(),
            url: resp.final_url,
            requested_url: url.to_string(),
            title,
            status: resp.status,
            content_type,
            source_bytes: resp.bytes.len(),
            redirect_chain: resp.redirect_chain,
        })
    }

    /// 取主文档（GET）。阶段四第二步接入全内存 HTTP 缓存：
    /// 命中强缓存直接返回（`from_cache=true`），未命中走网络并按需写缓存。
    async fn fetch_main(
        &self,
        inner: &mut Inner,
        url: &str,
        ua: &str,
        timeout_ms: u64,
    ) -> EngineResult<(net::FetchResponse, bool)> {
        if cache_enabled() {
            if let Some(entry) = inner.http_cache.get(url) {
                return Ok((
                    net::FetchResponse {
                        final_url: url.to_string(),
                        status: entry.status,
                        headers: entry.headers.clone(),
                        bytes: entry.bytes.clone(),
                        redirect_chain: vec![],
                        elapsed_ms: 0,
                    },
                    true,
                ));
            }
        }
        let mut req = net::FetchRequest::get(url, timeout_ms.max(1_000));
        // 主文档导航:浏览器语义 —— 顶层导航不带 Referer,但带完整 Accept/UA/
        // Sec-Fetch-*(navigate) 指纹头;风控站点对「裸 UA」的主文档同样会拦。
        req.context = Some(reqctx::RequestContext::subresource("", reqctx::Dest::Document, ua));
        let resp = self
            .http
            .fetch(req, &mut inner.cookies)
            .await
            .map_err(|e| {
                EngineError::new(2001, e)
                    .with_data(json!({"reason": "connection_level", "retryable": true}))
            })?;
        cache_put(&mut inner.http_cache, url, &resp);
        Ok((resp, false))
    }

    /// 后退/前进/刷新。返回导航结果。
    pub async fn history_nav(&self, page_id: &str, dir: &str, timeout_ms: u64) -> EngineResult<NavOutcome> {
        let (target, target_index) = {
            let inner = self.inner.lock().await;
            let tab = inner
                .tabs
                .get(page_id)
                .ok_or_else(|| EngineError::page_not_found(page_id))?;
            match dir {
                "back" if tab.history_index > 0 => (
                    tab.history[tab.history_index - 1].url.clone(),
                    Some(tab.history_index - 1),
                ),
                "forward" if tab.history_index + 1 < tab.history.len() => (
                    tab.history[tab.history_index + 1].url.clone(),
                    Some(tab.history_index + 1),
                ),
                "reload" => (tab.url.clone(), None),
                _ => {
                    return Ok(NavOutcome {
                        page_id: page_id.to_string(),
                        url: tab.url.clone(),
                        requested_url: tab.url.clone(),
                        title: tab.title.clone(),
                        status: tab.status,
                        content_type: tab.content_type.clone(),
                        source_bytes: tab.source_bytes,
                        redirect_chain: tab.redirect_chain.clone(),
                    })
                }
            }
        };
        let mut inner = self.inner.lock().await;
        let ua = inner
            .tabs
            .get(page_id)
            .map(|t| t.user_agent.clone())
            .unwrap_or_else(|| DEFAULT_UA.to_string());
        let mut out = self
            .navigate_locked(&mut inner, page_id, &target, timeout_ms, false, &ua, &NavHints::default())
            .await?;
        if let Some(tab) = inner.tabs.get_mut(page_id) {
            if let Some(idx) = target_index {
                tab.history_index = idx;
            }
            // 重新导航后 URL/标题以实际落地为准,历史栈条目同步更正。
            if let Some(entry) = tab.history.get_mut(tab.history_index) {
                entry.url = tab.url.clone();
                entry.title = tab.title.clone();
            }
            out.title = tab.title.clone();
            out.url = tab.url.clone();
        }
        Ok(out)
    }

    pub async fn close_page(&self, page_id: &str) -> CloseOutcome {
        let mut inner = self.inner.lock().await;
        match page_id {
            "all" => {
                let ids: Vec<String> = inner.tabs.keys().cloned().collect();
                shutdown_js_handles(&inner, &ids);
                inner.tabs.clear();
                inner.windows.clear();
                inner.cookies.clear();
                inner.local_storage.clear();
                // 有头:所有页面关闭 → 查看器一并关闭。
                self.close_view();
                CloseOutcome {
                    closed: ids,
                    message: "closed".to_string(),
                }
            }
            "background" => {
                let ids: Vec<String> = inner
                    .tabs
                    .values()
                    .filter(|t| !t.foreground)
                    .map(|t| t.id.clone())
                    .collect();
                shutdown_js_handles(&inner, &ids);
                for id in &ids {
                    inner.tabs.remove(id);
                }
                close_from_windows(&mut inner, &ids);
                clear_session_if_empty(&mut inner);
                CloseOutcome {
                    closed: ids,
                    message: "closed".to_string(),
                }
            }
            "others" => CloseOutcome::default(),
            id => {
                let existed = inner.tabs.contains_key(id);
                if existed {
                    let ids = vec![id.to_string()];
                    shutdown_js_handles(&inner, &ids);
                    inner.tabs.remove(id);
                    close_from_windows(&mut inner, &ids);
                    clear_session_if_empty(&mut inner);
                    // 有头:最后一个页面关闭 → 查看器一并关闭。
                    if inner.tabs.is_empty() {
                        self.close_view();
                    }
                    CloseOutcome {
                        closed: ids,
                        message: "closed".to_string(),
                    }
                } else {
                    CloseOutcome {
                        closed: Vec::new(),
                        message: format!("page_id 不存在:{id}"),
                    }
                }
            }
        }
    }

    pub async fn close_others(&self, keep: &str) -> CloseOutcome {
        let mut inner = self.inner.lock().await;
        let ids: Vec<String> = inner
            .tabs
            .keys()
            .filter(|id| id.as_str() != keep)
            .cloned()
            .collect();
        shutdown_js_handles(&inner, &ids);
        for id in &ids {
            inner.tabs.remove(id);
        }
        close_from_windows(&mut inner, &ids);
        clear_session_if_empty(&mut inner);
        CloseOutcome {
            closed: ids,
            message: "closed".to_string(),
        }
    }

    pub async fn bring_to_front(&self, page_id: &str) -> bool {
        let mut inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get(page_id) else {
            return false;
        };
        let window_id = tab.window_id.clone();
        for t in inner.tabs.values_mut() {
            if t.window_id == window_id {
                t.foreground = t.id == page_id;
            }
        }
        if let Some(w) = inner.windows.iter_mut().find(|w| w.id == window_id) {
            w.active = Some(page_id.to_string());
        }
        true
    }

    pub async fn new_tab(&self, from_page: &str, url: &str, timeout_ms: u64) -> EngineResult<OpenOutcome> {
        let source_window = {
            let inner = self.inner.lock().await;
            inner
                .tabs
                .get(from_page)
                .map(|t| t.window_id.clone())
                .ok_or_else(|| EngineError::page_not_found(from_page))?
        };
        let mut out = self
            .new_page(url, BrowserMode::Hidden, None, None, None, false, false, timeout_ms, &NavHints::default())
            .await?;
        if out.window_id != source_window {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(&out.page_id) {
                tab.window_id = source_window.clone();
            }
            for w in inner.windows.iter_mut() {
                w.tabs.retain(|id| id != &out.page_id);
            }
            if let Some(w) = inner.windows.iter_mut().find(|w| w.id == source_window) {
                w.tabs.push(out.page_id.clone());
                w.active = Some(out.page_id.clone());
            }
            inner.windows.retain(|w| !w.tabs.is_empty());
            out.window_id = source_window;
        }
        Ok(out)
    }

    pub async fn set_viewport(&self, page_id: &str, width: u32, height: u32) -> EngineResult<Value> {
        let mut inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get_mut(page_id) else {
            return Err(EngineError::page_not_found(page_id));
        };
        tab.viewport.width = width.clamp(320, 7680);
        tab.viewport.height = height.clamp(240, 4320);
        // 重新布局(与视口自适应共用同一条路径,见 `view::resize::relayout_tab`)。
        self.relayout_tab(tab);
        // 有头:视口重排后推帧。
        if tab.mode.is_headed() {
            self.push_frame(tab);
        }
        Ok(json!({
            "width": tab.viewport.width,
            "height": tab.viewport.height,
            "device_scale_factor": tab.viewport.device_scale_factor,
            "mobile": tab.viewport.mobile,
            "note": "LsmAgentBrowser 阶段一视口重排已完成",
        }))
    }

    pub async fn set_cookie(
        &self,
        page_id: Option<&str>,
        name: &str,
        value: &str,
        domain: Option<&str>,
        path: Option<&str>,
        url: Option<&str>,
    ) -> EngineResult<Value> {
        let mut inner = self.inner.lock().await;
        let base = url
            .map(str::to_string)
            .or_else(|| {
                page_id.and_then(|p| inner.tabs.get(p).map(|t| t.url.clone()))
            })
            .ok_or_else(|| EngineError::param("set_cookie 需要 page_id 或 url"))?;
        inner.cookies.set_explicit(name, value, domain, path, &base);
        Ok(json!({"name": name, "value": value}))
    }

    pub async fn set_cookie_raw(&self, url: &str, raw: &str) {
        let mut inner = self.inner.lock().await;
        inner.cookies.set_from_headers(url, &[("set-cookie".to_string(), raw.to_string())]);
    }

    pub async fn delete_cookie(
        &self,
        page_id: Option<&str>,
        name: &str,
        domain: Option<&str>,
    ) -> EngineResult<Value> {
        let mut inner = self.inner.lock().await;
        let _ = page_id;
        inner.cookies.delete(name, domain);
        Ok(json!({"name": name}))
    }

    pub async fn cookies_json(&self) -> Value {
        let inner = self.inner.lock().await;
        json!(inner
            .cookies
            .all()
            .iter()
            .map(Cookie::to_json)
            .collect::<Vec<_>>())
    }

    pub async fn storage_get(&self, page_id: &str, kind: &str) -> EngineResult<Value> {
        let inner = self.inner.lock().await;
        let tab = inner
            .tabs
            .get(page_id)
            .ok_or_else(|| EngineError::page_not_found(page_id))?;
        if kind == "session" {
            return Ok(json!(tab.session_storage));
        }
        let origin = urlutil::storage_origin(&tab.url);
        let map = inner.local_storage.get(&origin).cloned().unwrap_or_default();
        Ok(json!(map))
    }

    pub async fn storage_set(
        &self,
        page_id: &str,
        kind: &str,
        items: &[(String, String)],
    ) -> EngineResult<Value> {
        let mut inner = self.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(page_id)
            .ok_or_else(|| EngineError::page_not_found(page_id))?;
        if kind == "session" {
            for (k, v) in items {
                tab.session_storage.insert(k.clone(), v.clone());
            }
            return Ok(json!({"kind": "session", "snapshot": tab.session_storage}));
        }
        let origin = urlutil::storage_origin(&tab.url);
        let map = inner.local_storage.entry(origin).or_default();
        for (k, v) in items {
            map.insert(k.clone(), v.clone());
        }
        Ok(json!({"kind": "local", "snapshot": map}))
    }

    pub async fn storage_clear(
        &self,
        page_id: &str,
        kind: &str,
        keys: &[String],
    ) -> EngineResult<Value> {
        let mut inner = self.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(page_id)
            .ok_or_else(|| EngineError::page_not_found(page_id))?;
        if kind == "session" {
            if keys.is_empty() {
                tab.session_storage.clear();
            } else {
                for k in keys {
                    tab.session_storage.remove(k);
                }
            }
            return Ok(json!({"kind": "session", "result": "cleared"}));
        }
        let origin = urlutil::storage_origin(&tab.url);
        let map = inner.local_storage.entry(origin).or_default();
        if keys.is_empty() {
            map.clear();
        } else {
            for k in keys {
                map.remove(k);
            }
        }
        Ok(json!({"kind": "local", "result": "cleared"}))
    }

    /// 截图。`params` 见兼容层文档。
    pub async fn screenshot(&self, page_id: &str, params: &Value) -> EngineResult<Value> {
        // 先让持久 JS 线程把定时器/微任务与 Canvas 像素回写到宿主,再取布局帧。
        api::sync_page(self, page_id).await;
        let mut inner = self.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(page_id)
            .ok_or_else(|| EngineError::page_not_found(page_id))?;
        // 「快照×布局」一致性守卫:sync_page 应用动作后 html 可能已变,先重排再出帧。
        self.ensure_layout_locked(tab);
        self.screenshot_locked(tab, params)
    }

    pub(crate) fn screenshot_locked(&self, tab: &TabState, params: &Value) -> EngineResult<Value> {
        let full_page = params.get("full_page").and_then(Value::as_bool).unwrap_or(false);
        let ocr = params.get("ocr").and_then(Value::as_bool).unwrap_or(false);
        // OCR 语义 = "截图里能看到的文字":优先取布局产出的文本行;
        // 布局无文本(内容被 display:none 剪枝或由 JS 生成)时回退到 DOM 文本。
        //
        // ★ 这里**只算不算返回**:`ocr=true` 是「顺带给我文字」,不是「只给我文字」。
        // 旧实现在此处直接 return,PNG 分支被整段跳过 —— 调用方拿到 code:0 却
        // 找不到文件(实测:Agent 落盘全景截图后靠 `ls` 复核才发现文件不存在,
        // 直接判死 wf-2)。渲染与落盘必须照走,OCR 结果事后并入返回。
        let ocr_block = if ocr {
            let layout_text = tab
                .layout
                .lines
                .iter()
                .filter(|l| l.visible)
                .map(|l| l.text.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let (text, source) = if layout_text.trim().is_empty() {
                let doc = document::Document::parse(&tab.html, &tab.url);
                (document::collapse_ws(&doc.text()), "dom_text")
            } else {
                (layout_text, "lsm_layout_text")
            };
            let blocks: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            Some(json!({
                "ocr_text": text,
                "ocr_block_count": blocks.len(),
                "ocr_source": source,
                "note": "阶段一 OCR 返回内核可见文本;真实像素 OCR 在阶段五接入。\
                         ocr_source=dom_text 表示布局层没有可见文本(内容被 display:none 剪枝或由页面脚本生成)",
            }))
        } else {
            None
        };
        // 阶段四第二步：clip 区域截图（文档坐标；full_page=false 时 y 相对视口）。
        let clip = params.get("clip").and_then(Value::as_object).map(|c| {
            (
                c.get("x").and_then(Value::as_f64).unwrap_or(0.0),
                c.get("y").and_then(Value::as_f64).unwrap_or(0.0),
                c.get("width").and_then(Value::as_f64).unwrap_or(0.0),
                c.get("height").and_then(Value::as_f64).unwrap_or(0.0),
            )
        });
        // 整页截图的画布高 = page_height(整篇文档),此时若仍减 tab.scroll_y,
        // 文档内容会整体上移、顶部被切、底部留白(视口截图才需要减)。
        let scroll_y = if full_page { 0.0 } else { tab.scroll_y };
        let scroll_x = if full_page { 0.0 } else { tab.scroll_x };
        let pixmap = self
            .font
            .render_pixmap(
                &tab.layout,
                tab.viewport.width,
                tab.viewport.height,
                scroll_x,
                scroll_y,
                full_page,
                &tab.images,
            )
            .ok_or_else(|| EngineError::new(3001, "渲染失败(无法创建像素缓冲区)"))?;
        let mut applied_clip = None;
        let png = if let Some((cx, cy, cw, ch)) = clip {
            if cw > 0.0 && ch > 0.0 {
                // 文档坐标 → 渲染像素坐标（视口截图时 y 需加回 scroll_y 偏移）。
                let py = if full_page { cy } else { cy + tab.scroll_y };
                if let Some(cropped) = render::crop_pixmap(&pixmap, cx, py, cw, ch) {
                    applied_clip = Some(json!({
                        "x": cx, "y": cy, "width": cw, "height": ch,
                    }));
                    cropped
                } else {
                    pixmap.encode_png().unwrap_or_default()
                }
            } else {
                pixmap.encode_png().unwrap_or_default()
            }
        } else {
            pixmap.encode_png().unwrap_or_default()
        };
        if png.is_empty() {
            return Err(EngineError::new(3001, "渲染失败(无法编码 PNG)"));
        }
        // 有头:截到的视口帧同步推给查看器(所见即所得;全页截图仍推视口帧)。
        if tab.mode.is_headed() && !full_page && applied_clip.is_none() {
            if let Ok(guard) = self.view.lock() {
                if let Some(sink) = guard.as_ref() {
                    sink.send_frame(&png);
                }
            }
        }
        // 落盘路径:`save_path` 为契约名,`output_path`/`path` 为常见别名
        // (LLM 常按直觉写别名,静默忽略会让 Agent 找不到自己要的截图)。
        let save_path = ["save_path", "output_path", "path"]
            .iter()
            .find_map(|k| params.get(*k).and_then(Value::as_str))
            .filter(|s| !s.trim().is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let dir = artifact_root::artifact_dir("web_screenshots");
                let _ = std::fs::create_dir_all(&dir);
                dir.join(format!("web_{}_{}.png", now_ms(), tab.id))
            });
        if let Some(parent) = save_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&save_path, &png)
            .map_err(|e| EngineError::action_failed(format!("截图落盘失败:{e}")))?;
        let mut data = json!({
            "save_path": save_path.to_string_lossy(),
            "byte_size": png.len(),
            "format": "png",
            "full_page": full_page,
            "viewport": {"width": tab.viewport.width, "height": tab.viewport.height},
        });
        if let Some(clip) = applied_clip {
            data["clip"] = clip;
        }
        if !self.font.has_font() {
            data["render_note"] = json!("未找到系统字体,截图为占位色块");
        }
        if params.get("return_base64").and_then(Value::as_bool).unwrap_or(false) {
            use base64::Engine as _;
            data["image_base64"] = json!(base64::engine::general_purpose::STANDARD.encode(&png));
        }
        // OCR 与落盘并存:调用方一次调用既拿到文件也拿到文字。
        if let Some(extra) = ocr_block {
            if let (Some(dst), Some(src)) = (data.as_object_mut(), extra.as_object()) {
                for (k, v) in src {
                    dst.insert(k.clone(), v.clone());
                }
            }
        }
        Ok(data)
    }

    /// 下载 URL 或页内链接。
    pub async fn download(&self, page_id: &str, params: &Value) -> EngineResult<Value> {
        let (url, save_dir, filename) = {
            let inner = self.inner.lock().await;
            let tab = inner
                .tabs
                .get(page_id)
                .ok_or_else(|| EngineError::page_not_found(page_id))?;
            let url = if let Some(u) = params.get("url").and_then(Value::as_str) {
                urlutil::resolve(&tab.url, u)
                    .ok_or_else(|| EngineError::param("download url 非法"))?
            } else if let Some(sel) = params.get("selector").and_then(Value::as_str) {
                let doc = document::Document::parse(&tab.html, &tab.url);
                let node = doc
                    .first(sel)
                    .map_err(EngineError::action_failed)?
                    .ok_or_else(|| EngineError::action_failed(format!("选择器未命中:{sel}")))?;
                document::attr(&node, "href")
                    .and_then(|h| urlutil::resolve(&tab.url, &h))
                    .ok_or_else(|| EngineError::action_failed("目标元素没有可下载链接"))?
            } else {
                return Err(EngineError::param("download 需要 url 或 selector"));
            };
            (
                url,
                params
                    .get("save_dir")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                params
                    .get("filename")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        };
        let mut req = net::FetchRequest::get(&url, 120_000);
        req.max_bytes = 512 * 1024 * 1024;
        let mut inner = self.inner.lock().await;
        let resp = self
            .http
            .fetch(req, &mut inner.cookies)
            .await
            .map_err(|e| EngineError::new(2001, e))?;
        let dir = save_dir
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let d = artifact_root::artifact_dir("web_downloads");
                let _ = std::fs::create_dir_all(&d);
                d
            });
        let _ = std::fs::create_dir_all(&dir);
        let name = filename.unwrap_or_else(|| {
            let ct = resp.content_type();
            let ext = if ct.contains("html") {
                "html"
            } else if ct.contains("json") {
                "json"
            } else if ct.contains("png") {
                "png"
            } else if ct.contains("jpeg") || ct.contains("jpg") {
                "jpg"
            } else if ct.contains("pdf") {
                "pdf"
            } else {
                ""
            };
            let base = urlutil::filename_from_url(&resp.final_url);
            if ext.is_empty() || base.ends_with(&format!(".{ext}")) {
                base
            } else {
                format!("{base}.{ext}")
            }
        });
        let path = dir.join(urlutil::sanitize_filename(&name));
        std::fs::write(&path, &resp.bytes)
            .map_err(|e| EngineError::action_failed(format!("下载落盘失败:{e}")))?;
        Ok(json!({
            "guid": format!("dl_{}", now_ms()),
            "url": resp.final_url,
            "suggested_filename": name,
            "save_path": path.to_string_lossy(),
            "byte_size": resp.bytes.len(),
            "received_bytes": resp.bytes.len(),
            "total_bytes": resp.bytes.len(),
            "state": "completed",
            "elapsed_ms": resp.elapsed_ms,
        }))
    }
}

fn env_limit(key: &str, default: usize, min: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map(|v| v.max(min))
        .unwrap_or(default)
}

/// LAEW_DUMP_HTML=<路径>:把管线后的整页 HTML 落盘(诊断「JS 建好的 UI 没回写 /
/// 布局空白」类问题的离线分析入口,与 tests 里的布局探针配套),默认关闭。
/// 2026-11-01 天猫二轮:抽成共用助手 —— 此前只在新页 `load_locked` 落盘,
/// navigate 复用路径的第二次全量导航**不落盘**,真机排障时 dump 停在首开
/// 状态,离线分析断粮。
fn dump_html_if_env(html: &str) {
    if let Ok(p) = std::env::var("LAEW_DUMP_HTML") {
        let _ = std::fs::write(&p, html);
        eprintln!("[dump] wrote {} bytes to {}", html.len(), p);
    }
}

fn mode_hint(requested: BrowserMode) -> Option<String> {
    if requested.is_headed() {
        Some(
            "有头模式已启用:独立查看器子进程(`laew __browser-view`)以原生窗口显示软件渲染帧。\
             查看器退出/不可用时自动回退纯无头,页面能力不受影响。"
                .to_string(),
        )
    } else {
        None
    }
}

// 会话生命周期自由函数已按 1800 行约定机械拆分至 `session_lifecycle.rs`
// (调用点零改写,经此重导入保持原名字解析)。
mod session_lifecycle;
use session_lifecycle::{
    clear_session_if_empty, close_from_windows, find_reusable, record_network,
    shutdown_js_handles,
};

/// 全内存 HTTP 缓存开关（阶段四第二步）：`LAEW_WEB_CACHE=off` 关闭。
fn cache_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_CACHE")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 非 HTML 主文档合成包装页(第 175 轮,真实浏览器行为)。
///
/// 导航到 `image/*`、`text/plain`、`application/json` 等非 HTML 资源时,
/// 真实浏览器会合成展示页(图片居中显示 / 文本等宽呈现);此前一律把响应
/// 字节流塞给 HTML 解析器 → 空 DOM → 渲染全黑。百度地图登录二维码接口
/// (`passport.baidu.com/v2/api/qrcode`,Content-Type: image/png)实测黑屏,
/// Agent「把可见浏览器窗口当二维码展示板」的路径被阻断。
///
/// - `image/*`(≤4MB):flex 居中 `<img>`,**data: 内嵌**——二维码等一次性
///   资源二次网络取回会变样/失效,必须用本次响应字节;
/// - `text/plain` / `application/json`(含 `+json`):等宽 `<pre>`(HTML 转义);
/// - 其余明确非 HTML 类型:类型/大小/URL 说明页;
/// - HTML / XML / 空 Content-Type:返回 None,调用方走原解析路径。
fn synthesize_non_html_document(content_type: &str, bytes: &[u8], final_url: &str) -> Option<String> {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if ct.is_empty() || ct.contains("html") || ct.contains("xml") {
        return None;
    }
    let esc = |s: &str| -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    };
    let url_disp = esc(final_url);
    if ct.starts_with("image/") && bytes.len() <= 4 * 1024 * 1024 {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        return Some(format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{url}</title></head>\
             <body style=\"margin:0;height:100vh;background:#f5f5f5;display:flex;align-items:center;justify-content:center\">\
             <img style=\"max-width:100%;object-fit:contain\" src=\"data:{ct};base64,{b64}\" alt=\"image\">\
             </body></html>",
            url = url_disp,
            ct = ct,
            b64 = b64,
        ));
    }
    if (ct.starts_with("text/") && ct != "text/html") || ct == "application/json" || ct.ends_with("+json") {
        let text = urlutil::decode_body(bytes, content_type);
        let shown = if text.chars().count() > 200_000 {
            let mut s: String = text.chars().take(200_000).collect();
            s.push_str("\n…(截断)");
            s
        } else {
            text
        };
        return Some(format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{url}</title></head>\
             <body style=\"margin:0;padding:16px;background:#fff\">\
             <pre style=\"white-space:pre-wrap;font-family:monospace;font-size:13px;line-height:1.6;margin:0\">{text}</pre>\
             </body></html>",
            url = url_disp,
            text = esc(&shown),
        ));
    }
    // 图片超限或其余二进制:说明页。
    Some(format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{url}</title></head>\
         <body style=\"margin:0;padding:32px;background:#fff;color:#333;font-size:14px\">\
         <p><b>非 HTML 文档</b></p><p>类型:{ct}</p><p>大小:{size} 字节</p><p>URL:{url}</p>\
         <p>内核暂不支持在页面内呈现该类型。</p></body></html>",
        ct = esc(&ct),
        size = bytes.len(),
        url = url_disp,
    ))
}

/// 解析主文档响应的 CSP 头（阶段四第二步）。
/// report-only 模式不执行，返回 None。
fn parse_csp_header(headers: &[(String, String)]) -> Option<security::CspPolicy> {
    let header = net::header_value(headers, "content-security-policy")?;
    if security::CspPolicy::is_report_only(&header) {
        return None;
    }
    Some(security::CspPolicy::parse(&header))
}

/// 按响应头判断可缓存性并写入全内存缓存（阶段四第二步）。
/// 只缓存 GET 2xx 且 Cache-Control 允许存储的响应。
fn cache_put(cache: &mut cache::HttpCache, url: &str, resp: &net::FetchResponse) {
    if !(200..300).contains(&resp.status) {
        return;
    }
    let cc = net::header_value(&resp.headers, "cache-control").unwrap_or_default();
    let policy = cache::parse_cache_control(&cc);
    if !policy.is_cacheable() {
        return;
    }
    cache.put(cache::CacheEntry::new(
        url.to_string(),
        resp.status,
        resp.headers.clone(),
        resp.bytes.clone(),
        now_ms(),
    ));
}

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 供兼容层读取标签页只读视图(标题/URL/HTML/布局)。
pub(crate) async fn tab_view(
    mgr: &LsmBrowserManager,
    page_id: &str,
) -> EngineResult<(String, String, String, layout::Layout, Viewport, f64, f64)> {
    let inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get(page_id)
        .ok_or_else(|| EngineError::page_not_found(page_id))?;
    Ok((
        tab.url.clone(),
        tab.title.clone(),
        tab.html.clone(),
        tab.layout.clone(),
        tab.viewport,
        tab.scroll_x,
        tab.scroll_y,
    ))
}
