//! 页面管控(可视化模式蒙层 / 人工操作拦截 / 部分屏蔽,第 141 轮起,第 143 轮扩展)。
//!
//! 需求:headed 可视化浏览器中,人工**可实时观看**页面变化;人工与页面的交互
//! 权限由**页面管控三档**决定(详案 `docs/MCP_Web_Chrome_Use/07-页面管控三档模式与人工交互设计.md`):
//!
//! - **locked 屏蔽模式(缺省,第 141 轮)**:人工不可点击/操作(防交叉操作),
//!   人工与页面的交互一律走「人工介入」弹窗(`request_human`,提问期间默认临时解锁);
//! - **open 非屏蔽模式(第 143 轮)**:人工可直接操作页面(屏蔽页面状态栏明示「页面开放」),
//!   用于人工亲自操作 / 人工登录先行流;
//! - **partial 部分屏蔽模式(第 143 轮)**:盾区内人工不可操作、盾区外放行
//!   (`allow_selectors` 白名单挖洞 / `block_selectors` 黑名单罩元素)。
//!
//! 三档实现(第 141 轮双层 + 第 143 轮盾区):
//! - **L1 输入拦截(locked,CDP 硬闸门)**:`Input.setIgnoreInputEvents(ignore=true)`,
//!   浏览器级丢弃该页面全部真实用户输入(鼠标/滚轮/键盘/触摸),target 级全 frame
//!   一致、跨导航持久。**实测同时拦截 CDP 合成输入**(`Input.dispatch*`),因此
//!   Agent 自己的输入类动作必须「先解后锁」([`action_dispatches_input`] 白名单,
//!   `mcp_web_use::control::run` 统一包裹);open/partial 不设此闸门。
//! - **L2 视觉蒙层(locked,JS 纯视觉)**:全屏半透明遮罩,`pointer-events:none`
//!   不参与 hit-test,`data-%NS%agent="1"`(blockers/elements/dom 提取过滤),
//!   `Page.addScriptToEvaluateOnNewDocument` 挂载(导航自动重注入,全 frame 生效)。
//! - **L3 盾区(partial,DOM 级)**:受保护区域覆盖 `pointer-events:auto` 的半透明
//!   盾罩吞掉人工输入;黑名单 = 每命中元素一个红调盾罩,白名单 = 允许区并集在视口
//!   内做「垂直条带分解」求补集(深色调盾罩拼出整屏罩+洞的效果,纯矩形零兼容性
//!   依赖 —— 实测 `clip-path: path(evenodd,…)` 会被部分 Chrome 版本拒绝,不采纳)。
//!   第 144 轮:白名单在**子 frame 零命中时不加盾**(fail-open)—— 白名单语义定义
//!   在主文档坐标空间,跨域验证码 iframe 内部必须放行,整 frame 加盾会把主文档
//!   挖好的洞从内部锁死;洞外区域由主文档条带盾照拦,安全性不降。
//!   盾罩同样会吞 Agent 的 CDP 点击(hit-test 层不可区分),partial 下 Agent 输入
//!   动作「先隐盾后复盾」(`%NS%GuardSuspend`,见 `control::guard_lift_for_input`)。
//!
//! **第 171 轮:告知性 UI 外迁**。原左下角静态状态条(档位文案 + 注意事项 +
//! `guard_note`)与右上角「LAEW Agent 控制中」徽标已合并为两条语义清晰的栏,放在
//! `browser_status.rs`:**屏蔽页面状态栏**(右上角,含具体 Agent 名)与
//! **屏蔽页面进度栏**(左下角,动态进度)。本模块自此**不再持有任何文字元素**,
//! 只保留「蒙层 + 盾区 + 输入闸 + 锁定 CSS」;档位切换经 [`apply_page_guard`]
//! 收口时顺带把状态推给两条栏。详见 `docs/MCP_Web_Chrome_Use/24-屏蔽页面状态栏与进度栏.md`。
//!
//! 为什么不用 DOM 蒙层(`pointer-events:auto`)拦**整页**输入:CDP 合成输入与真实
//! 输入在 hit-test 层不可区分(`isTrusted` 均为 true),会把 Agent 自己的点击一并
//! 吃掉;且「挂起标志」跨域 iframe 读不到,Agent 点跨域 iframe 区域会被误拦且无法
//! 解除。整页拦截用 CDP 层,只有 partial 的**分区**拦截才用 DOM 盾区(区域粒度
//! CDP 做不到,盾区脚本运行在各 frame 自己的文档里,跨域 iframe 同样罩得住)。
//!
//! **第 155 轮:locked 档「可滚动观看」**。CDP `setIgnoreInputEvents` 是页面级
//! 一刀切 —— 连滚轮一起拦,人工在 locked 档连「自己往上翻历史」都做不到(豆包
//! 多轮对话实测:页面靠 auto_follow 停在最新,人工一翻就断)。该论证只否定
//! pointer-events 蒙层;**事件监听器 + `isTrusted` 判据**没有那个缺陷:真实人工
//! 输入与 CDP 合成输入 `isTrusted=true`(拦),页面自合成事件(`el.click()` /
//! `dispatchEvent`,eval_js 驱动)`isTrusted=false`(放行 —— CDP 拦截下本就不进
//! 输入管线,能力面等价)。因此 locked 档缺省改用 **DOM 选择性输入闸**:拦
//! 点击/键盘/拖拽/触摸,放行滚轮/滚动/鼠标移动;杀开关 `LAEW_WEB_LOCKED_SCROLL=off`
//! 或闸注入失败时 fail-closed 回退 CDP 全量拦截。设计详案
//! `docs/MCP_Web_Chrome_Use/13-locked档可滚动观看与人工应答语义.md`。

use chromiumoxide::cdp::browser_protocol::input::SetIgnoreInputEventsParams;
use chromiumoxide::Page;
use serde_json::{json, Value};

/// 蒙层遮罩元素 id(locked 档视觉层;测试断言锚点;`inspect(elements/dom)` 经
/// `data-<ns>agent` 过滤)。第 168 轮:id 前缀随进程级随机命名空间(隐身,见
/// `browser_ns`),由模板 `%NS%` 渲染 —— 故为函数而非常量。
pub fn overlay_element_id() -> String {
    format!("{}overlay__", crate::agent::browser_ns::dom_ns())
}

/// 盾区容器 id(partial 档,黑名单盾罩/白名单整屏挖洞盾的父节点)。
pub fn shields_element_id() -> String {
    format!("{}guard_shields__", crate::agent::browser_ns::dom_ns())
}

/// 注入 UI 的 closed Shadow DOM host id(第 169 轮)。蒙层/盾区整体住在
/// 该 host 的影子树里(页面 JS 不可读,防风控采 innerText 拿到品牌签名),
/// light DOM 只留这个无文本 host —— 故「视觉元素是否在场」的主文档探针查它。
pub fn guard_ui_host_id() -> String {
    format!("{}guard_ui__", crate::agent::browser_ns::dom_ns())
}

/// 单侧选择器(allow/block)数量上限。
pub const MAX_GUARD_SELECTORS: usize = 8;

/// 单条选择器字符上限。
pub const MAX_GUARD_SELECTOR_LEN: usize = 160;

/// `guard_note` / `note` 字符上限(屏蔽页面状态栏第四行,超长静默截断)。
pub const MAX_GUARD_NOTE_LEN: usize = 60;

// ===================== 第 143 轮:页面管控三档模型 =====================

/// 页面管控三档(第 143 轮)。三档语义与决策表见 doc 07 §3.1。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageGuardMode {
    /// 屏蔽模式(缺省,= 第 141 轮蒙层):CDP 输入拦截 + 半透明蒙层,人工可看不可点。
    Locked,
    /// 非屏蔽模式:无拦截无蒙层,屏蔽页面状态栏明示「页面开放·人工可直接操作」。
    Open,
    /// 部分屏蔽模式:盾区内拦截(CDOM 盾罩),盾区外人工可操作。
    Partial,
}

impl PageGuardMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::Open => "open",
            Self::Partial => "partial",
        }
    }

    /// 解析工具参数 `guard` / set_guard `mode` 字符串;`None` = 非法值。
    /// 含中文别名(LLM 偶发输出中文档位名时宽容解析)。
    pub fn from_arg(v: &str) -> Option<Self> {
        match v.trim().to_ascii_lowercase().as_str() {
            "locked" | "lock" | "屏蔽" | "屏蔽模式" => Some(Self::Locked),
            "open" | "unlock" | "free" | "开放" | "非屏蔽" | "非屏蔽模式" => Some(Self::Open),
            "partial" | "semi" | "部分" | "部分屏蔽" | "部分屏蔽模式" => Some(Self::Partial),
            _ => None,
        }
    }

    /// 解析 `LAEW_WEB_GUARD` 的值(第 143 轮):
    /// `locked`/`lock`/on 系 → 屏蔽;`open`/`off` 系 → 开放;
    /// **partial 不支持**(环境变量无选择器载体,doc 07 §5.3)→ None 落下一级。
    pub fn from_env_value(v: &str) -> Option<Self> {
        match v.trim().to_ascii_lowercase().as_str() {
            "locked" | "lock" | "on" | "1" | "true" | "yes" => Some(Self::Locked),
            "open" | "off" | "0" | "false" | "no" => Some(Self::Open),
            _ => None,
        }
    }
}

/// 页面管控配置(实例期望态,`BrowserInner.guard` 持有)。
#[derive(Debug, Clone, PartialEq)]
pub struct PageGuardConfig {
    pub mode: PageGuardMode,
    /// 白名单(仅 partial):**只有**命中区域人工可操作,其余整屏屏蔽(挖洞)。
    pub allow_selectors: Vec<String>,
    /// 黑名单(仅 partial):命中区域人工**不可**操作,其余放行(元素盾罩)。
    pub block_selectors: Vec<String>,
    /// 屏蔽页面状态栏第四行自定义提示(三档通用,≤60 字符;人工登录先行流的 UI 载体)。
    pub note: Option<String>,
}

impl Default for PageGuardConfig {
    fn default() -> Self {
        Self::from_env_default()
    }
}

impl PageGuardConfig {
    /// 屏蔽模式(缺省档)。
    pub fn locked() -> Self {
        Self { mode: PageGuardMode::Locked, allow_selectors: Vec::new(), block_selectors: Vec::new(), note: None }
    }

    /// 开放模式。
    pub fn open() -> Self {
        Self { mode: PageGuardMode::Open, allow_selectors: Vec::new(), block_selectors: Vec::new(), note: None }
    }

    /// 部分屏蔽模式(选择器联合校验走 [`Self::validate`])。
    pub fn partial(allow: Vec<String>, block: Vec<String>) -> Self {
        Self { mode: PageGuardMode::Partial, allow_selectors: allow, block_selectors: block, note: None }
    }

    /// 第 141 轮 legacy 布尔语义映射:true=屏蔽,false=开放。
    pub fn legacy_overlay(on: bool) -> Self {
        if on { Self::locked() } else { Self::open() }
    }

    /// `LAEW_WEB_GUARD` > `LAEW_WEB_OVERLAY`(legacy)> 缺省 locked 的级联缺省。
    pub fn from_env_default() -> Self {
        if let Ok(v) = std::env::var("LAEW_WEB_GUARD") {
            if let Some(m) = PageGuardMode::from_env_value(&v) {
                return Self { mode: m, ..Self::locked() };
            }
            tracing::warn!(
                "LAEW_WEB_GUARD={v} 非法或不支持(partial 需选择器,须在 open 参数 / set_guard 传),忽略"
            );
        }
        if overlay_default_from_env() {
            Self::locked()
        } else {
            Self::open()
        }
    }

    /// 归一化:选择器 trim、note 截断到 [`MAX_GUARD_NOTE_LEN`] 字符(按字符边界)。
    pub fn normalized(&self) -> Self {
        Self {
            mode: self.mode,
            allow_selectors: self
                .allow_selectors
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            block_selectors: self
                .block_selectors
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            note: self
                .note
                .as_ref()
                .map(|n| {
                    let t = n.trim();
                    if t.chars().count() <= MAX_GUARD_NOTE_LEN {
                        t.to_string()
                    } else {
                        t.chars().take(MAX_GUARD_NOTE_LEN).collect()
                    }
                })
                .filter(|s| !s.is_empty()),
        }
    }

    /// 参数校验(1001 语义):allow/block 互斥、partial 必须给其一、
    /// locked/open 不收选择器、数量与长度上限。
    pub fn validate(&self) -> std::result::Result<(), String> {
        if !self.allow_selectors.is_empty() && !self.block_selectors.is_empty() {
            return Err("allow_selectors 与 block_selectors 互斥,只能提供其一".into());
        }
        for (name, list) in [
            ("allow_selectors", &self.allow_selectors),
            ("block_selectors", &self.block_selectors),
        ] {
            if list.len() > MAX_GUARD_SELECTORS {
                return Err(format!("{name} 数量超过上限({MAX_GUARD_SELECTORS})"));
            }
            for s in list {
                if s.chars().count() > MAX_GUARD_SELECTOR_LEN {
                    return Err(format!("{name} 单条选择器超过 {MAX_GUARD_SELECTOR_LEN} 字符"));
                }
            }
        }
        match self.mode {
            PageGuardMode::Partial => {
                if self.allow_selectors.is_empty() && self.block_selectors.is_empty() {
                    return Err(
                        "guard=partial 需要 allow_selectors 或 block_selectors 之一(非空)".into(),
                    );
                }
            }
            PageGuardMode::Locked | PageGuardMode::Open => {
                if !self.allow_selectors.is_empty() || !self.block_selectors.is_empty() {
                    return Err("guard=locked/open 不接受选择器;分区管控请用 guard=partial".into());
                }
            }
        }
        Ok(())
    }
}

// ===================== 管控 JS(蒙层 + 盾区两件套) =====================
// 第 171 轮:第三件「左下角静态状态条」已迁 `browser_status.rs`(屏蔽页面状态栏 +
// 屏蔽页面进度栏)。

/// 管控 JS 模板:`{MODE}`/`{ALLOW}`/`{BLOCK}`/`{NOTE}`/`{SCROLLWATCH}` 占位符由
/// [`guard_script`] 烘焙为初始配置,使运行期 `set_guard` 后发生的导航按「期望态」
/// 重建(配合导航类动作收尾的 re-assert,与第 141 轮同构)。
///
/// 设计要点:
/// - 蒙层 `pointer-events:none`(纯视觉,零 hit-test 参与,不挡点击);
/// - 盾罩 `pointer-events:auto`(真拦截,只出现在 partial);
/// - z-index 序:高亮蓝框 2147483647 > 蒙层 2147483646 > 盾区 2147483645
///   (第 171 轮:两条告知栏 z-index 2147483646 / host 2147483644,与蒙层同档不遮页面);
/// - `data-%NS%agent="1"`:与高亮蓝框同款标记,`blockers` 正文扫描
///   (克隆 body 后剥 `[data-%NS%agent]`)与 elements/dom 提取天然排除;
/// - 白名单 = 允许区并集的「垂直条带分解」补集矩形盾(x 去重分条带 → 条带内
///   y 区间合并求补),纯矩形 + pointer-events,零兼容性依赖(不用 clip-path);
/// - 盾罩矩形随滚动/缩放/DOM 变化 rAF 节流重算;MutationObserver 忽略
///   `[data-%NS%agent]` 自身节点的变更(防自激循环);
/// - `document.body` 未就绪(极早期文档)时挂 documentElement,DOMContentLoaded 补挂。
pub const AGENT_GUARD_JS_TEMPLATE: &str = r#"(() => {
    // 第 179 轮:窗口面 API 收敛到一个 symbol 命名空间对象(零 window 字符串键)。
    const NS = window[Symbol.for('%NS%')] || (window[Symbol.for('%NS%')] = {});
    const REV = {REV};
    // 第 157 轮:版本化重装 —— 同一页面多次注册脚本(重复 open / rearm 重注入)后,
    // 「先注册的脚本抢先安装、后注册的脚本被 early-return」会让旧档在导航后复活
    // (实测观感:页面屏蔽失效、可编辑且无提示)。现在低版本直接退出;高版本先
    // 卸载旧实例(输入闸监听器 + 滚动/resize 调度 + 观察器 + 视觉元素)再重装,
    // 保证「最新期望档必赢」。
    if (window.%NS%GuardRev >= REV) return;
    window.%NS%GuardRev = REV;
    try { if (window.%NS%GuardOff) window.%NS%GuardOff(); } catch (e) {}
    window.%NS%GuardInstalled = true;
    window.%NS%GuardMode = {MODE};
    window.%NS%GuardAllow = {ALLOW};
    window.%NS%GuardBlock = {BLOCK};
    window.%NS%GuardNote = {NOTE};
    window.%NS%GuardSuspended = false;
    // 第 155 轮:locked 档 DOM 选择性输入闸(可滚动观看)。
    // 只拦 isTrusted 事件(真实人工输入 + CDP 合成输入);页面自合成事件
    // (eval_js 驱动的 el.click()/dispatchEvent)isTrusted=false 不拦 —— 与
    // CDP 拦截下的 Agent 能力面等价。滚轮/滚动/鼠标移动不拦,人工可自由翻阅观看。
    window.%NS%GuardScrollWatch = {SCROLLWATCH};
    window.%NS%GuardInputSuspended = false;
    window.%NS%GuardGateReady = false;
    // 第 157 轮:GATE_EVTS/gate 提升到闸块外定义 —— 版本化重装时
    // %NS%GuardOff 需要引用它们卸载旧监听器。
    const GATE_EVTS = ['mousedown', 'mouseup', 'click', 'dblclick', 'auxclick',
        'contextmenu', 'pointerdown', 'pointerup', 'dragstart', 'drop', 'selectstart',
        'keydown', 'keyup', 'keypress', 'touchstart', 'touchend'];
    const gate = (e) => {
        try { if (!e.isTrusted) return; } catch (ex) { return; }
        if (window.%NS%GuardMode !== 'locked') return;
        if (window.%NS%GuardInputSuspended) return;
        try { e.stopImmediatePropagation(); e.preventDefault(); } catch (ex) {}
    };
    if (window.%NS%GuardScrollWatch) {
        for (const t of GATE_EVTS) window.addEventListener(t, gate, true);
        window.%NS%GuardInputSuspend = (s) => {
            window.%NS%GuardInputSuspended = !!s;
            try { applyLockCss(); } catch (ex) {}
            return true;
        };
        window.%NS%GuardGateReady = true;
    }
    const MASK_ID = '%NS%overlay__';
    const SHIELDS_ID = '%NS%guard_shields__';
    const STYLE_ID = '%NS%guard_style__';
    const HOST_ID = '%NS%guard_ui__';
    // 第 171 轮:左下角静态状态条(档位文案 + 注意事项 + guard_note)已整体迁到
    // `browser_status.rs` 的**屏蔽页面状态栏(右上角)**;本脚本不再自建任何文字
    // 元素 —— 告知性 UI 单一真源,避免同屏两处重复声明与两套 shadow host。
    // 第 169 轮:注入 UI 挂 closed Shadow DOM —— 盾区标签(「🔒 已锁定(仅 Agent)」)
    // 盾区标签(「🔒 已锁定(仅 Agent)」)等文本若留在 light DOM,页面 JS 一行
    // `document.body.innerText` 即可枚举到自动化框架品牌签名(实测京东风控 JS
    // 采集该文本把设备判为自动化,`.jd-main-risk-fallback`「当前页面异常」)。
    // 挂 closed shadow 后人工截图/肉眼可见不变,hit-testing 穿 shadow 边界
    // (盾区 pointer-events:auto 照常拦截),页面 innerText/textContent/DOM 枚举/
    // innerHTML 序列化均读不到影子树内容。attachShadow 不可用时 fail-open 回退
    // light DOM(只损隐身不损管控功能)。
    let SH = null;
    const el = (id) => (SH && SH.getElementById(id)) || document.getElementById(id);
    const mount = (node) => (SH || root()).appendChild(node);
    // 第 163 轮:document-start 时 `document.documentElement` 与 `document.body`
    // **都可能是 null**(`readyState === 'loading'`)。旧写法 `document.body ||
    // document.documentElement` 在此二者皆 null → `root().appendChild()` 抛
    // TypeError,视觉层(蒙层/盾区)一个都没建。实测后果:此后任何一次
    // navigate / 复用 open 的 goto 都继承空 DOM,蒙层**永久消失**,
    // 而 `%NS%GuardSet` 已定义且返回非 null,`apply_page_guard` 照样回报
    // `visual_mask: true` —— 输入锁真锁了,屏幕上一丝视觉都没有,且系统自称成功。
    // `documentElement` 在 document-start 即存在,放前面即可。
    const root = () => document.documentElement || document.body;
    // 就绪门:两者都没有时直接返回,由下面的 DOMContentLoaded / load 重试补建。
    const ready = () => !!(document.documentElement || document.body);
    const ensure = () => {
        if (!ready()) return;
        // shadow host 装配(每文档一次):SPA 整树替换把 host 冲掉时按 id 重找重建,
        // attachShadow 抛错(重复挂载/老引擎)fail-open 回退 light DOM。
        if (!document.getElementById(HOST_ID) || !SH) {
            let h = document.getElementById(HOST_ID);
            if (!h) {
                h = document.createElement('div');
                h.id = HOST_ID;
                h.setAttribute('data-%NS%agent', '1');
                h.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483647;';
                root().appendChild(h);
            }
            try { SH = h.attachShadow ? h.attachShadow({ mode: 'closed' }) : null; } catch (ex) { SH = null; }
        }
        if (!el(MASK_ID)) {
            const m = document.createElement('div');
            m.id = MASK_ID;
            m.setAttribute('data-%NS%agent', '1');
            m.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483646;'
                + 'background:rgba(15,23,42,0.22);';
            mount(m);
        }
        if (!el(SHIELDS_ID)) {
            const sc = document.createElement('div');
            sc.id = SHIELDS_ID;
            sc.setAttribute('data-%NS%agent', '1');
            sc.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483645;';
            mount(sc);
        }
    };
    // 锁定态 CSS 层:悬停任何组件光标 ⃠(「不可点击」视觉态)+ 全页禁选
    // (user-select 硬底;selectstart 闸是事件层双保险)。class 与 style 随
    // 「locked 且未挂起」共用一个判据 —— Agent 输入动作「先解后锁」期间
    // (%NS%GuardInputSuspend / %NS%GuardSuspend)CSS 锁同步放开,收口复锁。
    // open/partial 不上全局锁(partial 盾罩自带 cursor:not-allowed)。
    // 注:本样式必须留在**主文档**(作用对象是页面元素,影子树样式打不到
    // 页面节点);其选择器类名已 ns 随机化,不构成固定签名。
    const applyLockCss = () => {
        const on = window.%NS%GuardMode === 'locked'
            && !window.%NS%GuardSuspended && !window.%NS%GuardInputSuspended;
        const r = root();
        if (!r) return;
        r.classList.toggle('%NS%locked__', on);
        let st = el(STYLE_ID);
        if (on) {
            if (!st) {
                st = document.createElement('style');
                st.id = STYLE_ID;
                st.setAttribute('data-%NS%agent', '1');
                r.appendChild(st);
            }
            st.textContent = 'html.%NS%locked__ *{cursor:not-allowed !important;'
                + 'user-select:none !important;-webkit-user-select:none !important;}';
        } else if (st) {
            st.remove();
        }
    };
    // 第 171 轮:原 `setChip`(左下角静态状态条刷新)已随状态条整体迁到
    // `browser_status.rs`;档位/注意事项/guard_note 由 Rust 侧 `sync` 推给状态栏。
    const rebuildShields = () => {
        const sc = el(SHIELDS_ID);
        if (!sc) return;
        while (sc.firstChild) sc.removeChild(sc.firstChild);
        const allow = Array.isArray(window.%NS%GuardAllow) ? window.%NS%GuardAllow : [];
        const block = Array.isArray(window.%NS%GuardBlock) ? window.%NS%GuardBlock : [];
        const vw = window.innerWidth, vh = window.innerHeight;
        // 盾罩标签预算(选择器命中很多元素时避免满屏小标签)
        let tagBudget = 6;
        const addShield = (x, y, w, h, dark) => {
            const sh = document.createElement('div');
            sh.setAttribute('data-%NS%agent', '1');
            sh.style.cssText = 'position:fixed;left:' + x + 'px;top:' + y + 'px;width:' + w
                + 'px;height:' + h + 'px;pointer-events:auto;z-index:2147483645;cursor:not-allowed;'
                + (dark
                    ? 'background:rgba(15,23,42,0.30);'
                    : 'background:rgba(185,28,28,0.18);border:1.5px dashed rgba(185,28,28,0.8);')
                + 'box-sizing:border-box;';
            if (tagBudget > 0) {
                tagBudget--;
                const tag = document.createElement('div');
                tag.setAttribute('data-%NS%agent', '1');
                tag.style.cssText = 'position:absolute;right:0;bottom:100%;padding:2px 8px;'
                    + 'background:rgba(127,29,29,0.95);color:#fecaca;'
                    + 'font:600 11px/1.5 system-ui,-apple-system,"PingFang SC","Microsoft YaHei",sans-serif;'
                    + 'border-radius:5px 5px 0 0;white-space:nowrap;';
                tag.textContent = '🔒 已锁定(仅 Agent)';
                sh.appendChild(tag);
            }
            sc.appendChild(sh);
        };
        if (allow.length) {
            // 白名单语义定义在主文档坐标空间(条带分解挖洞的 rect 均为主文档
            // viewport 坐标):子 frame 一律不加盾(fail-open)——否则漂移选择器
            // 可能在子 frame 自身坐标系命中元素,按错误位置罩盾,把主文档挖好的
            // 洞从内部锁死。
            if (window.top !== window) return;
            // 白名单:整屏深色罩挖洞 —— 对允许矩形并集在视口内做「垂直条带分解」
            // 求补集(若干互不重叠的矩形盾)。纯矩形 + pointer-events,零兼容性依赖
            //(实测 clip-path: path(evenodd,…) 会被部分 Chrome 版本拒绝,不采纳)。
            const rects = [];
            for (const sel of allow) {
                // @rect:x,y,w,h —— 绝对视口挖洞(CDP DOM pierce 跨 frame 穿透
                // 生成的凭证输入框矩形),不经选择器解析,直接入洞;随
                // scroll/resize/布局变化由既有 schedule 重算裁剪
                if (sel.indexOf('@rect:') === 0) {
                    const p = sel.slice(6).split(',').map(Number);
                    if (p.length === 4 && p.every((n) => isFinite(n))) {
                        const x1 = Math.max(0, p[0]), y1 = Math.max(0, p[1]);
                        const x2 = Math.min(vw, p[0] + p[2]), y2 = Math.min(vh, p[1] + p[3]);
                        if (x2 - x1 > 0 && y2 - y1 > 0) rects.push([x1, y1, x2, y2]);
                    }
                    continue;
                }
                try {
                    for (const node of document.querySelectorAll(sel)) {
                        const r = node.getBoundingClientRect();
                        const x1 = Math.max(0, r.left), y1 = Math.max(0, r.top);
                        const x2 = Math.min(vw, r.right), y2 = Math.min(vh, r.bottom);
                        if (x2 - x1 > 0 && y2 - y1 > 0) rects.push([x1, y1, x2, y2]);
                    }
                } catch (e) {}
            }
            if (!rects.length) {
                // 白名单全部未命中(选择器漂移/元素消失)→ 本帧不加盾(fail-open),
                // 人工操作优先于管控保留。
                return;
            }
            const xs = [0, vw];
            for (const [x1, , x2] of rects) { xs.push(x1, x2); }
            xs.sort((a, b) => a - b);
            for (let i = 0; i + 1 < xs.length; i++) {
                const sx1 = xs[i], sx2 = xs[i + 1];
                if (sx2 - sx1 < 1) continue;
                // 该垂直条带内被允许区覆盖的 y 区间
                const ivs = [];
                for (const [x1, y1, x2, y2] of rects) {
                    if (x1 <= sx1 && sx2 <= x2) ivs.push([y1, y2]);
                }
                if (!ivs.length) {
                    addShield(sx1, 0, sx2 - sx1, vh, true);
                    continue;
                }
                ivs.sort((a, b) => a[0] - b[0]);
                let cur = 0;
                for (const [ya, yb] of ivs) {
                    if (ya > cur) addShield(sx1, cur, sx2 - sx1, ya - cur, true);
                    cur = Math.max(cur, yb);
                }
                if (cur < vh) addShield(sx1, cur, sx2 - sx1, vh - cur, true);
            }
        } else if (block.length) {
            for (const sel of block) {
                try {
                    for (const node of document.querySelectorAll(sel)) {
                        const r = node.getBoundingClientRect();
                        if (r.width <= 0 || r.height <= 0) continue;
                        addShield(r.left, r.top, r.width, r.height, false);
                    }
                } catch (e) {}
            }
        }
    };
    const refresh = () => {
        const mode = window.%NS%GuardMode;
        const susp = !!window.%NS%GuardSuspended;
        ensure();
        applyLockCss();
        const m = el(MASK_ID);
        if (m) m.style.display = (mode === 'locked' && !susp) ? '' : 'none';
        const sc = el(SHIELDS_ID);
        if (sc) {
            const want = (mode === 'partial' && !susp);
            if (!want) {
                sc.style.display = 'none';
            } else {
                sc.style.display = '';
                rebuildShields();
            }
        }
    };
    window.%NS%GuardSet = (mode, allow, block, note) => {
        window.%NS%GuardMode = String(mode || 'locked');
        window.%NS%GuardAllow = Array.isArray(allow) ? allow : [];
        window.%NS%GuardBlock = Array.isArray(block) ? block : [];
        window.%NS%GuardNote = (typeof note === 'string' && note) ? note : null;
        refresh();
        return window.%NS%GuardMode;
    };
    window.%NS%GuardSuspend = (s) => {
        window.%NS%GuardSuspended = !!s;
        refresh();
        return true;
    };
    // 第 141 轮 legacy 兼容:蒙层布尔开关(set_overlay 旧路径/单测锚点);
    // true=切屏蔽模式,false=切开放模式(模式级开关,非可见性开关)。
    window.%NS%OverlaySet = (on) =>
        window.%NS%GuardSet(on ? 'locked' : 'open', [], [], null);
    // 第 169 轮:影子树自探(非枚举)—— closed shadow 对页面/Rust evaluate 都不可读,
    // 视觉层校验(visual_mask)与真浏览器回归测试经此读 UI 在场性/显示态。
    window.%NS%GuardUiProbe = () => ({
        host: !!document.getElementById(HOST_ID),
        mask: !!el(MASK_ID),
        mask_display: el(MASK_ID) ? (el(MASK_ID).style.display || '') : 'missing',
        shields: !!el(SHIELDS_ID),
        shields_count: el(SHIELDS_ID) ? el(SHIELDS_ID).childElementCount : -1,
        shields_display: el(SHIELDS_ID) ? (el(SHIELDS_ID).style.display || '') : 'missing',
    });
    let raf = 0;
    const schedule = () => {
        if (window.%NS%GuardMode !== 'partial' || window.%NS%GuardSuspended) return;
        if (!raf) raf = window.requestAnimationFrame(() => { raf = 0; refresh(); });
    };
    window.addEventListener('scroll', schedule, { passive: true, capture: true });
    window.addEventListener('resize', schedule, { passive: true });
    // 布局位移兜底:图片/字体加载、同域 iframe 内容撑高等纯布局变化不触发
    // MutationObserver 的有效观察属性,但会移动白名单元素的矩形 —— load 事件
    // 与文档级 ResizeObserver 补上这一重建触发源。
    window.addEventListener('load', schedule, { passive: true });
    try {
        if (window.ResizeObserver && !window.%NS%GuardRO) {
            window.%NS%GuardRO = new ResizeObserver(() => schedule());
            const roTarget = document.documentElement || document.body;
            if (roTarget) window.%NS%GuardRO.observe(roTarget);
        }
    } catch (e) {}
    // 第 163 轮:MutationObserver 装配独立成函数,由 bootRetry 与首次调用各调一次。
    // observe 的 target 必须非 null,否则抛错被外层 catch 吞掉 → 观察器永不安装,
    // DOM 变化后无人重建视觉元素。
    const armObserver = () => {
        const target = document.documentElement || document.body;
        if (!target) return;
        if (window.%NS%GuardMO) { try { window.%NS%GuardMO.disconnect(); } catch (e) {} }
        try {
            window.%NS%GuardMO = new MutationObserver((muts) => {
                for (const mu of muts) {
                    const t = mu.target;
                    if (t && t.closest && t.closest('[data-%NS%agent]')) continue;
                    return schedule();
                }
            });
            window.%NS%GuardMO.observe(target, {
                childList: true, subtree: true,
                attributes: true, attributeFilter: ['class', 'style'],
            });
        } catch (e) {}
    };
    // 第 163 轮:**重试注册必须在首次 refresh() 之前**。
    // 旧顺序是 `refresh(); if (loading) addEventListener(DOMContentLoaded, refresh)`;
    // 但 document-start 时 refresh() 会因 root() 为 null 抛 TypeError,
    // 异常直接吞掉后面整段 —— DOMContentLoaded 补挂**永远不会被注册**,
    // 视觉层就此永久缺失(实测:首开正常,首次 navigate 后蒙层全部消失)。
    // `%NS%GuardOff` 同样定义在 refresh() 之后,抛异常后未定义,
    // 第 157 轮版本化重装一并哑火。
    const bootRetry = () => { try { armObserver(); refresh(); } catch (e) {} };
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', bootRetry, { once: true });
        // 双保险:部分文档 DOMContentLoaded 后仍无 body(load 前),load 再兜一次。
        window.addEventListener('load', bootRetry, { once: true });
    }
    armObserver();
    refresh();
    // 第 157 轮:卸载钩子 —— 高版本重装前移除旧实例的输入闸监听器、调度监听、
    // 观察器与蒙层/盾区视觉元素,防监听器与视觉泄漏。
    window.%NS%GuardOff = () => {
        try { for (const t of GATE_EVTS) window.removeEventListener(t, gate, true); } catch (e) {}
        try { window.removeEventListener('scroll', schedule, true); } catch (e) {}
        try { window.removeEventListener('resize', schedule); } catch (e) {}
        try { if (window.%NS%GuardMO) window.%NS%GuardMO.disconnect(); } catch (e) {}
        // 第 169 轮:注入 UI 整体住在 shadow host 里,拆 host 即清空影子树
        // (锁定样式 STYLE_ID 在主文档,单独移除)。
        for (const id of [HOST_ID, STYLE_ID]) {
            const n = document.getElementById(id);
            if (n) n.remove();
        }
        SH = null;
        try { if (root()) root().classList.remove('%NS%locked__'); } catch (e) {}
    };
    // 第 168 轮:隐身收敛 —— API 入口全部改成不可枚举(for…in / Object.keys 不可见)。
    // 实测京东人工扫码登录后返回 .jd-main-risk-fallback 风控页(「当前页面异常」),
    // 根因之一是风控 JS 枚举 window 拿到 __laew* 固定签名;不可枚举后与装了
    // 用户脚本的正常浏览器同水位(puppeteer-extra-stealth 同款策略)。fail-open:
    // defineProperty 失败仅损失隐身,不影响管控功能。
    // 第 179 轮:命名空间对象内部的键同样不可枚举(防止枚举 symbol 值后一眼看全)。
    try {
        ['GuardRev', 'GuardInstalled', 'GuardMode', 'GuardAllow',
         'GuardBlock', 'GuardNote', 'GuardSuspended', 'GuardScrollWatch',
         'GuardInputSuspended', 'GuardGateReady', 'GuardInputSuspend',
         'GuardOff', 'GuardSet', 'GuardSuspend', 'GuardMO', 'GuardRO',
         'GuardUiProbe', 'OverlaySet']
            .forEach((k) => {
                const d = Object.getOwnPropertyDescriptor(NS, k);
                if (!d) return;
                Object.defineProperty(NS, k,
                    { enumerable: false, configurable: true, writable: true, value: d.value });
            });
    } catch (e) {}
})()"#;

/// 按实例期望态生成注入脚本(烘焙初始配置;选择器/note 以 JSON 字面量嵌入,
/// 滚动观看开关按 [`locked_scroll_watch_enabled`] 烘焙 —— 环境变量进程级不变,
/// 烘焙即终态)。
///
/// 第 157 轮:每次生成单调递增版本号(REV)烘焙进脚本 —— 版本化重装的依据,
/// 保证同一页面多次注册时「最新期望档必赢」(防旧档脚本导航后抢先安装)。
pub fn guard_script(cfg: &PageGuardConfig) -> String {
    static GUARD_SCRIPT_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let rev = GUARD_SCRIPT_REV.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let cfg = cfg.normalized();
    let mode = serde_json::to_string(cfg.mode.as_str()).unwrap_or_else(|_| "\"locked\"".into());
    let allow = serde_json::to_string(&cfg.allow_selectors).unwrap_or_else(|_| "[]".into());
    let block = serde_json::to_string(&cfg.block_selectors).unwrap_or_else(|_| "[]".into());
    let note = serde_json::to_string(&cfg.note).unwrap_or_else(|_| "null".into());
    let scroll_watch = if locked_scroll_watch_enabled() { "true" } else { "false" };
    let baked = AGENT_GUARD_JS_TEMPLATE
        .replace("{REV}", &rev.to_string())
        .replace("{MODE}", &mode)
        .replace("{ALLOW}", &allow)
        .replace("{BLOCK}", &block)
        .replace("{NOTE}", &note)
        .replace("{SCROLLWATCH}", scroll_watch);
    // 第 168 轮：%NS% 占位符 → 进程级随机命名空间（隐身，见 browser_ns.rs）。
    // 第 179 轮：统一走 render_ns —— 窗口面 API 收敛到 symbol 命名空间、
    // DOM 面换成独立 token（只做 `%NS%` 直替会留下 19 个 window 字符串键）。
    crate::agent::browser_ns::render_ns(&baked)
}

/// `LAEW_WEB_LOCKED_SCROLL`(第 155 轮):`off/0/false/no` → 关闭 locked 档
/// 「可滚动观看」DOM 输入闸,回退第 141 轮 CDP 全量输入拦截(连滚轮一起拦)。
/// 默认开启。与 `LAEW_WEB_OVERLAY` / `LAEW_WEB_GUARD` 同款只认 off 系。
pub fn locked_scroll_watch_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_LOCKED_SCROLL")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// `LAEW_WEB_OVERLAY` 缺省开关(第 141 轮 legacy,第 143 轮起作为 guard 级联的
/// 末端输入):`off/0/false/no` → 开放;未设置/其它 → 屏蔽。
pub fn overlay_default_from_env() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_OVERLAY")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 给页面注入管控脚本(新文档自动重挂 + 当前文档立即执行,全 frame 生效)。
///
/// 与 `browser.rs::inject_agent_highlight` 同款通道;参数为实例期望态,
/// 导航后新文档按它重建,配合导航类动作收尾的 re-assert 使各层收敛。
/// 第 143 轮起 headed(非 connect)全档注入(open 档也有屏蔽页面状态栏告知人工可操作)。
/// 第 171 轮:同步注入屏蔽页面状态栏/进度栏(告知性 UI 独立生命周期,不随 guard
/// 版本化重装被拆掉)。
pub async fn inject_agent_guard(page: &Page, cfg: &PageGuardConfig) {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
    let _ = page
        .execute(AddScriptToEvaluateOnNewDocumentParams {
            source: guard_script(cfg),
            world_name: None,
            include_command_line_api: None,
            run_immediately: Some(true),
        })
        .await;
    crate::agent::browser_status::ensure(page).await;
}

/// 应用页面管控三档(L1 输入拦截按档 + JS 全量设置)。
///
/// - locked(第 155 轮起缺省):**DOM 选择性输入闸**(滚动观看)—— JS 先设置,
///   探测闸在场(`%NS%GuardGateReady`)→ CDP 显式放行(`setIgnoreInputEvents(false)`
///   清残留),人工滚轮可滚、点击/键盘被闸拦;
/// - locked(闸不在场:`LAEW_WEB_LOCKED_SCROLL=off` / hidden / 注入失败):
///   fail-closed 回退第 141 轮 CDP 全量拦截 `setIgnoreInputEvents(true)`;
/// - open:`setIgnoreInputEvents(false)` + `%NS%GuardSet('open',…)`(放行 + 状态栏显「页面开放」);
/// - partial:`setIgnoreInputEvents(false)` + `%NS%GuardSet('partial',…)`(盾区 + 状态栏显「部分锁定」)。
///
/// 返回应用结果(fail-open:单层失败不阻断另一层,如实回报);
/// 未注入脚本的页面(hidden/connect/注入失败)JS 侧返回 null → applied=false。
pub async fn apply_page_guard(
    page: &Page,
    cfg: &PageGuardConfig,
) -> std::result::Result<Value, String> {
    let cfg = cfg.normalized();
    let locked = cfg.mode == PageGuardMode::Locked;
    // L2/L3(+locked 的 DOM 输入闸):JS 先行 —— CDP 是否回退拦截取决于闸是否在场。
    let allow = serde_json::to_string(&cfg.allow_selectors).unwrap_or_else(|_| "[]".into());
    let block = serde_json::to_string(&cfg.block_selectors).unwrap_or_else(|_| "[]".into());
    let note = serde_json::to_string(&cfg.note).unwrap_or_else(|_| "null".into());
    let mode = serde_json::to_string(cfg.mode.as_str()).unwrap_or_else(|_| "\"locked\"".into());
    let ns_set = crate::agent::browser_ns::ns_member_expr("GuardSet");
    let js = format!("{ns_set} ? {ns_set}({mode}, {allow}, {block}, {note}) : null");
    let r = page
        .evaluate(js.as_str())
        .await
        .map_err(|e| format!("应用页面管控失败:{e}"))?;
    let js_applied = !r.value().map(|v| v.is_null()).unwrap_or(true);
    // 第 163 轮:`%NS%GuardSet` 返回非 null **不代表视觉层真的建起来了** ——
    // document-start 注入时它已定义,但建元素的代码因 root() 为 null 抛异常,
    // 于是「函数可用」与「屏幕上看得见」被混为一谈,`visual_mask` 恒真。
    // 这里独立校验视觉元素是否真实存在于 DOM,让回报字段不再「嘴上说成功」。
    // 第 169 轮:视觉元素住进 closed Shadow DOM,主文档只剩无文本 host,
    // 探针改为「host 在场(影子树同建)或 fallback light DOM 元素任一在场」。
    // 第 171 轮:两条栏(屏蔽页面状态栏/进度栏)有独立 host,一并计入在场性。
    // 2026-10-14:两条栏各自独立 host,两个都要探。
    let visual_present = page
        .evaluate(format!(
            "!!(document.getElementById('{}') \
             || document.getElementById('{}') \
             || document.getElementById('{}') \
             || document.getElementById('{}') \
             || document.getElementById('{}'))",
            guard_ui_host_id(),
            overlay_element_id(),
            shields_element_id(),
            crate::agent::browser_status::status_host_id(),
            crate::agent::browser_status::progress_host_id()
        ))
        .await
        .ok()
        .and_then(|v| v.value().and_then(|val| val.as_bool()))
        .unwrap_or(false);
    if locked && js_applied && !visual_present {
        tracing::warn!(
            "页面管控脚本已注入但视觉元素缺失(输入闸可能已生效而蒙层未渲染);\
             多为 document-start 就绪时序问题,应重跑本轮根元素门与 DOMContentLoaded 补挂"
        );
    }
    // 第 171 轮:屏蔽页面状态栏/进度栏全量刷新 —— 本函数是管控期望态的**唯一收口**
    // (open 首航 / set_guard / HITL 解锁与恢复 / 导航后 re-assert 四条路径都经此),
    // 顺带把「谁在控制 + 档位 + 注意事项 + 进度」推给页面侧(导航产生新文档后
    // 两条栏虽由 addScript 自动重挂,内容需重推)。fail-open。
    crate::agent::browser_status::push(page, Some(&cfg), None, None, None).await;
    // 闸在场性探测(第 155 轮):只有「locked + 滚动观看开启 + 脚本已注入且闸已装」
    // 才走 DOM 闸;任何一环缺失都回退 CDP 全量拦截(fail-closed)。
    let gate_ready = if locked && locked_scroll_watch_enabled() && js_applied {
        page
            .evaluate(format!(
                "!!{}",
                crate::agent::browser_ns::ns_member_expr("GuardGateReady")
            ))
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    } else {
        false
    };
    let use_dom_gate = locked && gate_ready;
    // L1:CDP 输入拦截 —— 仅在「locked 且 DOM 闸不在场」时启用;其余档(含闸接管的
    // locked)显式 false,顺带清掉此前回退态可能残留的 CDP 锁(标志跨导航持久)。
    let want_cdp_block = locked && !use_dom_gate;
    let input_lock_applied = page
        .execute(SetIgnoreInputEventsParams::new(want_cdp_block))
        .await
        .is_ok();
    // 第 157 轮:档位复核 rearm —— 应用后探测页面实际档位,与期望不符(陈旧
    // 脚本抢先安装 / 执行上下文漂移,实测观感「屏蔽失效、页面可编辑且无提示」)
    // 时重注入最新版本脚本并再应用一次;重注入经版本化重装安全(新档必赢)。
    // eval 级开销,fail-open(探测失败不阻断)。
    let mut rearmed = false;
    if locked {
        let actual = page
            .evaluate(format!(
                "{} || null",
                crate::agent::browser_ns::ns_member_expr("GuardMode")
            ))
            .await
            .ok()
            .and_then(|r| r.value().cloned());
        if actual.as_ref().and_then(Value::as_str) != Some("locked") {
            inject_agent_guard(page, &cfg).await;
            let _ = page.evaluate(js.as_str()).await;
            rearmed = true;
        }
    }
    Ok(json!({
        "mode": cfg.mode.as_str(),
        "input_locked": want_cdp_block && input_lock_applied,
        "input_lock_applied": input_lock_applied,
        "input_gate": if use_dom_gate { "dom_scrollable" } else if locked { "cdp" } else { "none" },
        "scroll_watch": use_dom_gate,
        "visual_mask": locked && js_applied,
        "shields_active": cfg.mode == PageGuardMode::Partial && js_applied,
        "rearmed": rearmed,
    }))
}

/// 挂起/恢复 locked 档 DOM 输入闸(第 155 轮,Agent 输入动作「先解后锁」的闸版)。
///
/// 闸不在场(未注入 / 杀开关关闭 / `%NS%GuardGateReady` 未置位)返回 false,
/// 调用方(`guard_ctl::guard_lift_for_input`)据此回退 CDP 解锁路径。
pub async fn gate_suspend_input(page: &Page, suspend: bool) -> bool {
    let ready = crate::agent::browser_ns::ns_member_expr("GuardGateReady");
    let suspend_fn = crate::agent::browser_ns::ns_member_expr("GuardInputSuspend");
    let js = format!("{ready} ? {suspend_fn}({suspend}) : null");
    match page.evaluate(js.as_str()).await {
        Ok(r) => !r.value().map(|v| v.is_null()).unwrap_or(true),
        Err(_) => false,
    }
}

/// 挂起/恢复全部管控视觉(蒙层 + 盾区 + 屏蔽页面状态栏/进度栏;不动 CDP 输入拦截)。
///
/// 三个使用点:截图/OCR/HITL 附图「拍前隐藏 → 拍完恢复」(证据链干净);
/// partial 下 Agent 输入动作「先隐盾后复盾」;不关心 CDP 的纯视觉场景。
/// 返回是否实际应用(未注入脚本 = false)。
pub async fn guard_suspend_visuals(page: &Page, hide: bool) -> bool {
    let ns_suspend = crate::agent::browser_ns::ns_member_expr("GuardSuspend");
    let js = format!("{ns_suspend} ? {ns_suspend}({hide}) : null");
    let applied = match page.evaluate(js.as_str()).await {
        Ok(r) => !r.value().map(|v| v.is_null()).unwrap_or(true),
        Err(_) => false,
    };
    // 第 171 轮:两条栏是告知性 UI,截图/OCR 取证同样要避让(否则证据里永远带着
    // 「正在 open …」的动态角标,人工核对截图时分不清是页面内容还是注入层)。
    crate::agent::browser_status::set_suspended(page, hide).await;
    applied
}

// ===================== 第 141 轮 legacy API(委托保留,路径零改动) =====================

/// legacy:按布尔蒙层语义注入(第 141 轮路径;新代码用 [`inject_agent_guard`])。
pub async fn inject_agent_overlay(page: &Page, initial_on: bool) {
    inject_agent_guard(page, &PageGuardConfig::legacy_overlay(initial_on)).await;
}

/// legacy:应用/解除页面蒙层(第 141 轮路径;新代码用 [`apply_page_guard`])。
/// `active=true` → 屏蔽模式,`active=false` → 开放模式(模式级切换)。
pub async fn apply_page_overlay(page: &Page, active: bool) -> std::result::Result<Value, String> {
    apply_page_guard(page, &PageGuardConfig::legacy_overlay(active)).await
}

/// legacy:仅切换视觉可见性(第 141 轮截图避让通道;委托挂起通道,
/// 现在连蒙层与两条告知栏一起隐藏 —— 证据更干净)。
pub async fn mask_set_visible(page: &Page, visible: bool) -> bool {
    guard_suspend_visuals(page, !visible).await
}

// ===================== 动作白名单(第 141 轮,第 143 轮沿用) =====================

/// 该 control_action 是否会经 CDP `Input.dispatch*`(或 chromiumoxide
/// `Element::click()/type_str()`,内部同为 Input 域)注入输入事件。
///
/// 命中者由 `mcp_web_use::control::run` 统一让路:
/// locked 档「先解后锁」(动作前 `setIgnoreInputEvents(false)` → 动作 → 复锁)、
/// partial 档「先隐盾后复盾」(动作前隐藏盾罩 → 动作 → 恢复)。
/// 保守圈定:部分动作默认走 JS 路径(如 `input_text(use_js=true)`),多让一次
/// 无害;漏掉真注入输入的动作则会被蒙层/盾区吃掉,宁可多不可少。
/// 不注入输入事件的动作(`wait/eval_js/screenshot/set_*/navigate 类`)不让路 ——
/// 长动作(如 wait 30s)期间保持管控,避免页面裸奔。
pub fn action_dispatches_input(action: &str) -> bool {
    matches!(
        action,
        "click"
            | "human_click"
            | "right_click"
            | "double_click"
            | "hover"
            | "scroll"
            | "scroll_to"
            | "key_press"
            | "press_sequence"
            | "input_text"
            | "human_input"
            | "clear_input"
            | "upload_file"
            | "select_option"
            | "select_dropdown"
            | "download"
            | "drag"
            | "focus"
            | "blur"
            | "mouse_move"
            | "dispatch_event"
    )
}

/// 该 control_action 完成后是否需要 re-assert 管控(导航类)。
///
/// CDP 输入拦截标志实测跨导航持久;管控 JS 新文档按注入时烘焙的初始配置重建,
/// 若运行期 `set_guard` 改档后发生导航,脚本会以旧档回归 —— re-assert
/// 用实例期望态重新应用,保证各层收敛。
pub fn action_needs_overlay_reassert(action: &str) -> bool {
    matches!(action, "navigate" | "back" | "forward" | "reload" | "new_tab")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::browser_ns::dom_ns;

    /// 环境变量类测试互斥(browser_mode.rs 同款,防并行 set_var 互踩)。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    // =================== 第 141 轮 legacy 环境变量 ===================

    #[test]
    fn env_default_on_and_off_values() {
        let _g = env_lock();
        unsafe {
            std::env::remove_var("LAEW_WEB_OVERLAY");
        }
        assert!(overlay_default_from_env());
        for v in ["off", "0", "false", "no", "OFF", " No "] {
            unsafe { std::env::set_var("LAEW_WEB_OVERLAY", v) };
            assert!(!overlay_default_from_env(), "{v} 应视为关闭");
        }
        // 非关闭语义的值不误杀(与 LAEW_TARGET_ANCHOR 等开关同款只认 off 系)
        for v in ["on", "1", "true", "garbage"] {
            unsafe { std::env::set_var("LAEW_WEB_OVERLAY", v) };
            assert!(overlay_default_from_env(), "{v} 应视为开启");
        }
        unsafe {
            std::env::remove_var("LAEW_WEB_OVERLAY");
        }
    }

    // =================== 第 143 轮:guard 环境变量级联 ===================

    #[test]
    fn guard_env_cascade() {
        let _g = env_lock();
        unsafe {
            std::env::remove_var("LAEW_WEB_GUARD");
            std::env::remove_var("LAEW_WEB_OVERLAY");
        }
        // 缺省:locked
        assert_eq!(PageGuardConfig::from_env_default().mode, PageGuardMode::Locked);
        // legacy LAEW_WEB_OVERLAY=off → open
        unsafe { std::env::set_var("LAEW_WEB_OVERLAY", "off") };
        assert_eq!(PageGuardConfig::from_env_default().mode, PageGuardMode::Open);
        // LAEW_WEB_GUARD 优先于 legacy overlay
        unsafe { std::env::set_var("LAEW_WEB_GUARD", "locked") };
        assert_eq!(PageGuardConfig::from_env_default().mode, PageGuardMode::Locked);
        unsafe { std::env::set_var("LAEW_WEB_GUARD", "open") };
        assert_eq!(PageGuardConfig::from_env_default().mode, PageGuardMode::Open);
        // partial 无选择器载体,env 不支持 → 落下一级(legacy off → open)
        unsafe { std::env::set_var("LAEW_WEB_GUARD", "partial") };
        assert_eq!(
            PageGuardConfig::from_env_default().mode,
            PageGuardMode::Open,
            "env partial 应被忽略并走下一级"
        );
        // 非法值同理落下一级
        unsafe { std::env::set_var("LAEW_WEB_GUARD", "garbage") };
        assert_eq!(PageGuardConfig::from_env_default().mode, PageGuardMode::Open);
        unsafe {
            std::env::remove_var("LAEW_WEB_GUARD");
            std::env::remove_var("LAEW_WEB_OVERLAY");
        }
    }

    #[test]
    fn guard_mode_parse() {
        assert_eq!(PageGuardMode::from_arg("locked"), Some(PageGuardMode::Locked));
        assert_eq!(PageGuardMode::from_arg(" Open "), Some(PageGuardMode::Open));
        assert_eq!(PageGuardMode::from_arg("partial"), Some(PageGuardMode::Partial));
        // 中文别名宽容解析
        assert_eq!(PageGuardMode::from_arg("屏蔽"), Some(PageGuardMode::Locked));
        assert_eq!(PageGuardMode::from_arg("非屏蔽"), Some(PageGuardMode::Open));
        assert_eq!(PageGuardMode::from_arg("部分屏蔽"), Some(PageGuardMode::Partial));
        assert_eq!(PageGuardMode::from_arg("nonsense"), None);
        // 环境变量:partial 不支持
        assert_eq!(PageGuardMode::from_env_value("partial"), None);
        assert_eq!(PageGuardMode::from_env_value("locked"), Some(PageGuardMode::Locked));
        assert_eq!(PageGuardMode::from_env_value("0"), Some(PageGuardMode::Open));
    }

    #[test]
    fn guard_config_validate_branches() {
        // partial 缺选择器
        assert!(PageGuardConfig::partial(vec![], vec![]).validate().is_err());
        // allow + block 互斥
        assert!(
            PageGuardConfig::partial(vec!["a".into()], vec!["b".into()])
                .validate()
                .is_err()
        );
        // partial 单侧合法
        assert!(PageGuardConfig::partial(vec![".login".into()], vec![]).validate().is_ok());
        assert!(PageGuardConfig::partial(vec![], vec!["#pay".into()]).validate().is_ok());
        // locked/open 不收选择器
        let mut c = PageGuardConfig::locked();
        c.allow_selectors = vec!["a".into()];
        assert!(c.validate().is_err());
        // 数量上限
        let many: Vec<String> = (0..=MAX_GUARD_SELECTORS).map(|i| format!("s{i}")).collect();
        assert!(PageGuardConfig::partial(many, vec![]).validate().is_err());
        // 单条长度上限
        let long = "a".repeat(MAX_GUARD_SELECTOR_LEN + 1);
        assert!(PageGuardConfig::partial(vec![long], vec![]).validate().is_err());
    }

    #[test]
    fn guard_config_normalize_trims_and_clips_note() {
        let mut c = PageGuardConfig::partial(vec!["  .login  ".into(), " ".into()], vec![]);
        c.note = Some("很长的提示".repeat(30));
        let n = c.normalized();
        assert_eq!(n.allow_selectors, vec![".login".to_string()], "空白选择器应被剔除");
        assert!(
            n.note.as_ref().unwrap().chars().count() <= MAX_GUARD_NOTE_LEN,
            "note 应截断到 {MAX_GUARD_NOTE_LEN} 字符"
        );
    }

    // =================== 管控 JS 模板标记 ===================

    #[test]
    fn guard_script_bakes_initial_config() {
        let _g = env_lock();
        unsafe {
            std::env::remove_var("LAEW_WEB_LOCKED_SCROLL");
        }
        let mut cfg = PageGuardConfig::partial(vec![".login-box".into()], vec![]);
        cfg.note = Some("请登录后应答弹窗".into());
        let js = guard_script(&cfg);
        assert!(js.contains(&format!("NS.GuardMode = \"partial\";")), "{js}");
        assert!(js.contains(&format!("NS.GuardAllow = [\".login-box\"];")), "{js}");
        assert!(js.contains(&format!("NS.GuardBlock = [];")), "{js}");
        assert!(
            js.contains(&format!("NS.GuardNote = \"请登录后应答弹窗\";")),
            "{js}"
        );
        // 第 155 轮:滚动观看开关烘焙(缺省开启)
        assert!(js.contains(&format!("NS.GuardScrollWatch = true;")), "{js}");
        // 占位符全部被烘焙
        for ph in ["{MODE}", "{ALLOW}", "{BLOCK}", "{NOTE}", "{SCROLLWATCH}", "{REV}"] {
            assert!(!js.contains(ph), "占位符 {ph} 未烘焙");
        }
        // locked 缺省烘焙
        let js2 = guard_script(&PageGuardConfig::locked());
        assert!(js2.contains(&format!("NS.GuardMode = \"locked\";")));
        assert!(js2.contains(&format!("NS.GuardNote = null;")));
    }

    // =================== 第 155 轮:locked 档 DOM 输入闸(可滚动观看) ===================

    #[test]
    fn locked_scroll_watch_env_default_and_off() {
        let _g = env_lock();
        unsafe {
            std::env::remove_var("LAEW_WEB_LOCKED_SCROLL");
        }
        assert!(locked_scroll_watch_enabled(), "缺省应开启可滚动观看");
        for v in ["off", "0", "false", "no", "OFF", " No "] {
            unsafe { std::env::set_var("LAEW_WEB_LOCKED_SCROLL", v) };
            assert!(!locked_scroll_watch_enabled(), "{v} 应视为关闭");
        }
        // 非关闭语义的值不误杀(与 LAEW_WEB_OVERLAY 同款只认 off 系)
        for v in ["on", "1", "true", "garbage"] {
            unsafe { std::env::set_var("LAEW_WEB_LOCKED_SCROLL", v) };
            assert!(locked_scroll_watch_enabled(), "{v} 应视为开启");
        }
        unsafe {
            std::env::remove_var("LAEW_WEB_LOCKED_SCROLL");
        }
    }

    /// 杀开关关闭时:闸不安装;开启时:闸标记齐全且拦截清单
    /// **不含** wheel/scroll/mousemove(人工滚动通路的回归锁)。
    ///
    /// 第 171 轮:档位文案已迁到 `browser_status::guard_text`,本测试只锁闸安装
    /// 与烘焙开关;文案断言改在 `browser_status` 的单测里。
    #[test]
    fn guard_script_gate_install_and_kill_switch() {
        let _g = env_lock();
        unsafe {
            std::env::remove_var("LAEW_WEB_LOCKED_SCROLL");
        }
        let on = guard_script(&PageGuardConfig::locked());
        assert!(on.contains(&format!("NS.GuardScrollWatch = true;")), "{on}");
        assert!(on.contains(&format!("NS.GuardGateReady = true;")), "{on}");
        assert!(on.contains(&format!("NS.GuardInputSuspend")), "{on}");
        // isTrusted 判据:页面自合成事件(isTrusted=false)必须放行
        assert!(on.contains("if (!e.isTrusted) return;"), "{on}");
        // 拦截清单含交互事件,不含滚动/移动通路(断言收窄到 GATE_EVTS 数组段,
        // 不与既有 partial 盾区的 'scroll' 重算监听混淆)
        let gate_list = on
            .split("const GATE_EVTS = [")
            .nth(1)
            .unwrap_or_default()
            .split("];")
            .next()
            .unwrap_or_default();
        assert!(!gate_list.is_empty(), "GATE_EVTS 数组段缺失:{on}");
        for evt in ["'mousedown'", "'click'", "'keydown'", "'touchstart'", "'dragstart'", "'selectstart'"] {
            assert!(gate_list.contains(evt), "拦截清单应含 {evt}:{gate_list}");
        }
        for evt in ["'wheel'", "'scroll'", "'mousemove'", "'pointermove'", "'touchmove'"] {
            assert!(!gate_list.contains(evt), "拦截清单不得含 {evt}(人工滚动通路):{gate_list}");
        }
        // 第 171 轮:档位/注意事项文案不再烘焙进本脚本(迁至状态栏),此处确认脚本内无残留
        assert!(!on.contains("页面已锁定"), "档位文案应由状态栏承载,guard 脚本不再自建:{on}");
        assert!(!on.contains("人工可滚动浏览"), "注意事项应迁至状态栏:{on}");

        unsafe { std::env::set_var("LAEW_WEB_LOCKED_SCROLL", "off") };
        let off = guard_script(&PageGuardConfig::locked());
        // 杀开关关闭:烘焙 false → 闸安装 if 块为死代码(运行时不注册监听,
        // %NS%GuardGateReady 保持 false → apply_page_guard 探测失败回退 CDP)。
        // 模板是静态文本,if 块源码仍在,只能断言烘焙结果本身。
        assert!(off.contains(&format!("NS.GuardScrollWatch = false;")), "{off}");
        assert!(
            off.contains(&format!("if (NS.GuardScrollWatch) {{")),
            "闸安装必须以烘焙开关为门(不得无条件安装):{off}"
        );
        unsafe {
            std::env::remove_var("LAEW_WEB_LOCKED_SCROLL");
        }
    }

    /// 管控脚本关键标记锁死:两件套元素 id(蒙层 + 盾区)/ 提取过滤(data-%NS%agent)/
    /// 挖洞 clip-path / 挂载函数 / pointer-events 语义。
    ///
    /// 第 171 轮:第三件「左下角静态状态条」已迁至 `browser_status`(屏蔽页面状态栏 +
    /// 屏蔽页面进度栏),本脚本不再含任何文字元素。
    #[test]
    fn guard_script_markers_locked() {
        let _g = env_lock();
        unsafe {
            std::env::remove_var("LAEW_WEB_LOCKED_SCROLL");
        }
        let js = guard_script(&PageGuardConfig::locked());
        assert!(js.contains(&overlay_element_id()), "蒙层元素 id 应出现在脚本中");
        assert!(js.contains(&shields_element_id()), "盾区容器 id 应出现在脚本中");
        assert!(js.contains(&format!("data-{}agent", dom_ns())));
        assert!(js.contains("z-index:2147483646"), "蒙层 z-index");
        assert!(js.contains("z-index:2147483645"), "盾区 z-index 恰低一档");
        assert!(js.contains(&format!("NS.GuardSet")));
        assert!(js.contains(&format!("NS.GuardSuspend")));
        assert!(js.contains(&format!("NS.OverlaySet")), "第 141 轮 legacy 别名保留");
        assert!(
            !js.contains("overlay_hint__"),
            "左下角静态状态条已迁至 browser_status(第 171 轮),脚本内不得残留"
        );
        assert!(
            js.contains(&crate::agent::browser_status::status_host_id()) == false,
            "两条栏有独立 host,不在 guard 脚本内"
        );
        // 三档档位文案(第 171 轮迁至 browser_status 的屏蔽页面状态栏,此处只回归「不得回流」)
        assert!(!js.contains("页面已锁定"), "档位文案不得回流 guard 脚本:{js}");
        // 盾区技术要点:白名单垂直条带分解求补集(纯矩形,零 clip-path 依赖)+
        // 黑名单红调盾罩 + 标签预算
        assert!(js.contains("垂直条带分解"));
        // 第 144 轮:子 frame 白名单零命中 fail-open(跨域验证码 iframe 内部放行)
        assert!(
            js.contains("window.top !== window"),
            "子 frame 白名单零命中应跳过加盾"
        );
        assert!(js.contains("fail-open"));
        assert!(!js.contains("clipPath"), "不依赖 clip-path(部分 Chrome 拒绝 evenodd path)");
        assert!(js.contains("rgba(185,28,28,0.18)"), "黑名单红调盾罩");
        assert!(js.contains("rgba(15,23,42,0.30)"), "白名单深色盾罩");
        assert!(js.contains("已锁定(仅 Agent)"));
        assert!(js.contains("tagBudget"), "盾罩标签应有数量预算");
        // 第 166 轮:屏蔽完整性 —— 锁定态 CSS 层(悬停 ⃠ 光标 + 全页禁选)+
        // selectstart 事件闸双保险 + 盾罩 not-allowed 光标。
        assert!(js.contains("cursor:not-allowed !important"), "锁定态应有 not-allowed 光标");
        assert!(js.contains("user-select:none !important"), "锁定态应禁选");
        assert!(js.contains("-webkit-user-select:none !important"), "WebKit 前缀禁选");
        assert!(js.contains(&format!("{}locked__", dom_ns())), "锁定 class 标记");
        assert!(js.contains(&format!("STYLE_ID = '{}guard_style__'", dom_ns())), "锁定样式元素 id");
        assert!(js.contains("applyLockCss"), "CSS 锁应有统一应用函数");
        // CSS 锁与输入闸挂起联动(Agent 输入动作期间同步放开,需求 2)
        let suspend_seg = js
            .split(&format!("NS.GuardInputSuspend = (s) =>"))
            .nth(1)
            .unwrap_or_default()
            .split("return true;")
            .next()
            .unwrap_or_default();
        assert!(
            suspend_seg.contains("applyLockCss"),
            "输入闸挂起必须联动 CSS 锁(先解后锁的 CSS 侧):{suspend_seg}"
        );
        // selectstart 闸(事件层)+ CSS(user-select)双保险
        let gate_list = js
            .split("const GATE_EVTS = [")
            .nth(1)
            .unwrap_or_default()
            .split("];")
            .next()
            .unwrap_or_default();
        assert!(gate_list.contains("'selectstart'"), "拦截清单应含 selectstart:{gate_list}");
        // 盾罩(partial)自身光标 not-allowed(z-index 2147483645 段唯一定位盾罩样式行)
        assert!(
            js.contains("z-index:2147483645;cursor:not-allowed;"),
            "盾罩应有 not-allowed 光标"
        );
        // 卸载钩子清理 CSS 锁(防旧 class/style 泄漏到新实例)
        let off_seg = js
            .split(&format!("NS.GuardOff = () =>"))
            .nth(1)
            .unwrap_or_default();
        assert!(off_seg.contains("STYLE_ID"), "卸载应移除锁定样式:{off_seg}");
        assert!(off_seg.contains(&format!("{}locked__", dom_ns())), "卸载应移除锁定 class:{off_seg}");
        // 蒙层纯视觉:蒙层本身 pointer-events:none;盾罩 pointer-events:auto
        let mask_seg = js
            .split("if (!el(MASK_ID))")
            .nth(1)
            .unwrap_or_default()
            .split("if (window.top === window")
            .next()
            .unwrap_or_default();
        assert!(
            mask_seg.contains("pointer-events:none"),
            "蒙层应为纯视觉(pointer-events:none)"
        );
    }

    /// HITL 盾区精化回归锁死:`@rect:` 绝对挖洞 / 子 frame 一律不加盾 /
    /// 布局位移(load + ResizeObserver)重建触发源。
    #[test]
    fn guard_js_supports_rect_holes_and_layout_triggers() {
        let js = super::AGENT_GUARD_JS_TEMPLATE;
        assert!(js.contains("@rect:"), "白名单应支持 @rect 绝对视口挖洞");
        assert!(
            js.contains("p.every((n) => isFinite(n))"),
            "@rect 参数应做有限性校验"
        );
        assert!(
            js.contains("if (window.top !== window) return;"),
            "子 frame 应在 allow 分支顶部直接不加盾(fail-open)"
        );
        assert!(
            js.contains("addEventListener('load', schedule"),
            "load 事件应触发盾区重建(图片/字体加载位移)"
        );
        assert!(
            js.contains("ResizeObserver"),
            "文档级 ResizeObserver 应触发盾区重建(布局撑高)"
        );
        // @rect 分支必须先于选择器解析 continue,避免绝对洞被 querySelectorAll 吞掉
        let rect_branch = js.find("indexOf('@rect:')").unwrap_or_default();
        let sel_loop = js.find("querySelectorAll(sel)").unwrap_or_default();
        assert!(
            rect_branch < sel_loop && rect_branch > 0,
            "@rect 解析应位于选择器循环内且先于选择器解析"
        );
    }

    /// 第 163 轮回归锁死:document-start 注入时的就绪门与**重试注册顺序**。
    ///
    /// 实测事故(`llaew_20261012_175823.log` + 真机 Chrome 复现):首开正常,
    /// 首次 navigate 后蒙层 / 盾区**永久消失**,而 `apply_page_guard`
    /// 仍回报 `visual_mask: true`。根因是 `root()` 写成 `document.body ||
    /// document.documentElement`(document-start 时两者皆 null → appendChild 抛
    /// TypeError),且 `refresh()` 排在 `DOMContentLoaded` 补挂注册**之前** ——
    /// 异常把整段注册吃掉,视觉层再无重建机会。
    #[test]
    fn guard_js_uses_ready_root_and_registers_retry_before_first_refresh() {        let js = super::AGENT_GUARD_JS_TEMPLATE;
        // ① 就绪门:documentElement 优先(document-start 即存在)
        assert!(
            js.contains("document.documentElement || document.body"),
            "root 必须以 documentElement 为先,否则 document-start 时为 null"
        );
        assert!(
            !js.contains("document.body || document.documentElement"),
            "旧写法 document-start 时两者皆 null,已移除"
        );
        // ② 重试注册必须早于首次 refresh()
        let retry = js
            .find("addEventListener('DOMContentLoaded'")
            .unwrap_or_else(|| panic!("应注册 DOMContentLoaded 补挂"));
        let first_refresh = js
            .find("    armObserver();\n    refresh();")
            .or_else(|| js.find("\n    refresh();"))
            .unwrap_or_else(|| panic!("应存在首次 refresh() 调用"));
        assert!(
            retry < first_refresh,
            "重试注册({retry})必须排在首次 refresh()({first_refresh})之前,\
             否则 refresh() 抛异常会吃掉整段注册"
        );
        // ③ 双保险:load 事件也要补挂一次
        assert!(
            js.contains("addEventListener('load', bootRetry"),
            "应有 load 兜底补挂(部分文档 DOMContentLoaded 后仍无 body)"
        );
        // ④ 观察器 observe 目标必须先判空
        assert!(
            js.contains("if (!target) return;"),
            "MutationObserver.observe 目标为 null 会抛错被吞,必须先判空"
        );
    }

    #[test]
    fn highlight_js_has_ready_retry_and_self_healing_early_return() {
        let js = crate::agent::browser::AGENT_HIGHLIGHT_JS;
        assert!(
            js.contains("document.documentElement || document.body"),
            "蓝框 root 同病:document-start 时 body/documentElement 皆可能为 null"
        );
        // 半成品实例可自愈:标志已置但元素不在时不得早退
        assert!(
            js.contains("%NS%AgentHighlightInstalled\n        && document.getElementById"),
            "早退门必须校验 DOM 元素真实存在,否则首装抛异常后永久无法补装"
        );
        assert!(
            js.contains("addEventListener('DOMContentLoaded', boot"),
            "蓝框也需 DOMContentLoaded 补挂"
        );
    }

    #[test]
    fn input_action_whitelist() {
        // 注入输入的动作必须让路(含 chromiumoxide Element click/type 路径)
        for a in [
            "click", "human_click", "right_click", "double_click", "hover", "scroll",
            "scroll_to", "key_press", "press_sequence", "input_text", "human_input",
            "clear_input", "upload_file", "select_option", "select_dropdown", "download", "drag", "focus",
            "blur", "mouse_move", "dispatch_event",
        ] {
            assert!(action_dispatches_input(a), "{a} 应让路");
        }
        // 长动作/纯读/状态设置不让路(wait 30s 期间不能裸奔)
        for a in [
            "wait", "eval_js", "screenshot", "heartbeat", "navigate", "back", "forward",
            "reload", "new_tab", "close_tab", "set_cookie", "set_storage", "set_viewport",
            "set_window", "sync_viewport", "set_highlight", "set_overlay", "set_guard",
            "request_human",
        ] {
            assert!(!action_dispatches_input(a), "{a} 不应让路");
        }
    }

    #[test]
    fn nav_actions_need_reassert() {
        for a in ["navigate", "back", "forward", "reload", "new_tab"] {
            assert!(action_needs_overlay_reassert(a), "{a} 应回收 re-assert");
        }
        for a in ["click", "wait", "eval_js", "set_overlay", "set_guard", "request_human"] {
            assert!(!action_needs_overlay_reassert(a), "{a} 无需 re-assert");
        }
    }

    // =================== 真浏览器集成验证(#[ignore],本地 --ignored 跑) ===================
    //
    // 覆盖第 141 轮三条关键链路(详案 docs/MCP_Web_Chrome_Use/05 §10)+ 第 143 轮 partial:
    // 1. 管控元素注入存在性(headed + locked;蒙层 + 盾区);
    // 2. 输入锁语义:锁定时 CDP 合成点击被拦(实测 setIgnoreInputEvents 连合成
    //    输入一起拦)→ 解锁后恢复 —— 即「先解后锁」包装的必要性回归锁;
    // 3. 蒙层隐藏/恢复(截图避让通道);
    // 4.(新)set_guard 三档切换 + partial 黑名单盾吞人工(合成)点击 + 先隐盾后复盾;
    // 5.(新)白名单垂直条带分解挖洞:允许区内合成点击可穿透、区外被盾吞。
    // 需要本机安装 Chrome/Edge;跑 `cargo test --lib browser_overlay -- --ignored --nocapture`。
    #[tokio::test]
    #[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
    async fn real_browser_overlay_lock_and_mask() {
        use crate::agent::browser::{BrowserManager, BrowserMode};
        let mgr = BrowserManager::global();
        let url = "data:text/html,<button id='b' onclick='this.dataset.c=(+this.dataset.c||0)+1' \
                   style='position:fixed;left:0;top:0;width:200px;height:100px'>x</button>";
        let (pid, _, _) = mgr
            .new_page(url, BrowserMode::Headed, None, None, Some((800, 600)), true, PageGuardConfig::locked())
            .await
            .expect("启动有头浏览器失败(本机是否装有 Chrome?)");
        let page = mgr.page(&pid).await.expect("page 句柄");

        let count = |page: &Page| {
            let p = page.clone();
            async move {
                p.evaluate(
                    "document.getElementById('b') ? (+document.getElementById('b').dataset.c || 0) : -1",
                )
                .await
                .ok()
                .and_then(|r| r.value().cloned())
                .and_then(|v| v.as_i64())
                .unwrap_or(-1)
            }
        };
        let cdp_click = |page: &Page| {
            let p = page.clone();
            async move {
                use chromiumoxide::cdp::browser_protocol::input::{
                    DispatchMouseEventParams, DispatchMouseEventType,
                };
                for t in [DispatchMouseEventType::MousePressed, DispatchMouseEventType::MouseReleased] {
                    let _ = p
                        .execute(
                            DispatchMouseEventParams::builder()
                                .r#type(t)
                                .x(50.0)
                                .y(30.0)
                                .button(chromiumoxide::cdp::browser_protocol::input::MouseButton::Left)
                                .click_count(1)
                                .build()
                                .unwrap(),
                        )
                        .await;
                }
            }
        };

        // ① 管控元素存在(第 169 轮:蒙层/盾区住 closed Shadow DOM,
        //    主文档探针读不到,经影子树自探 API 校验在场性)
        let mask_probe = page
            .evaluate(
                crate::agent::browser_ns::render_ns(
                    "(function(){const p = (window[Symbol.for('%NS%')]||{}).GuardUiProbe && (window[Symbol.for('%NS%')]||{}).GuardUiProbe(); \
                 return !!(p && p.mask && p.hint && p.shields);})()",
                ),
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            // 第 163 轮回归锁死:真实浏览器下也必须验「**导航之后**视觉层仍在」。
            // 旧用例只在 new_page 之后断言(load 后的安全窗口),从不 navigate ——
            // 于是 document-start 注入时就绪门缺失这个 P0 缺陷一直绿灯:
            // 首开正常,首次 navigate 后蒙层/蓝色边框永久消失,
            // 而 apply_page_guard 仍回报 visual_mask:true(谎报成功)。
            // 用 data: URL 再导航一次,document-start 时机与真实站点一致。
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(mask_probe, "蒙层/盾区容器未注入");

        // ② 输入锁:new_page 已 apply(locked) → CDP 点击应被拦
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 0, "锁定时 CDP 点击不应生效");
        // 解锁 → 点击生效
        apply_page_overlay(&page, false).await.unwrap();
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 1, "解锁后 CDP 点击应生效");
        // 复锁 → 再拦(「先解后锁」回归锁)
        apply_page_overlay(&page, true).await.unwrap();
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 1, "复锁后 CDP 点击应再次被拦");

        // ②b 第 155 轮:locked 档 DOM 选择性输入闸(可滚动观看)。闸在场时
        //     CDP 合成点击(isTrusted=true)被闸拦;闸挂起(先解)后通过;
        //     页面自合成点击(isTrusted=false,eval_js 驱动)始终放行(Agent
        //     能力面与 CDP 拦截等价);wheel 不在拦截清单(人工滚动通路)。
        let gate_probe = page
            .evaluate(crate::agent::browser_ns::render_ns("!!(window[Symbol.for('%NS%')]||{}).GuardGateReady && !!(window[Symbol.for('%NS%')]||{}).GuardInputSuspend"))
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(gate_probe, "可滚动观看闸未安装(测试环境应缺省开启)");
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 1, "闸激活时 CDP 合成点击应被拦");
        assert!(gate_suspend_input(&page, true).await, "闸挂起应实际应用");
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 2, "闸挂起后 CDP 点击应通过(先解后锁)");
        assert!(gate_suspend_input(&page, false).await);
        let synth = page
            .evaluate(
                "(function(){document.getElementById('b').click(); \
                 return (+document.getElementById('b').dataset.c||0);})()",
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        assert_eq!(synth, 3, "页面自合成点击(isTrusted=false)应放行:{synth}");
        let wheel = page
            .evaluate(
                "(function(){var got=false; var f=function(){got=true;}; \
                 window.addEventListener('wheel', f, {passive:true}); \
                 window.dispatchEvent(new WheelEvent('wheel')); \
                 window.removeEventListener('wheel', f); return got;})()",
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(wheel, "wheel 事件应放行(人工滚动通路)");

        // ③ 蒙层隐藏/恢复(截图避让通道)
        assert!(mask_set_visible(&page, false).await, "蒙层隐藏应实际应用");
        let hidden = page
            .evaluate(crate::agent::browser_ns::render_ns("((window[Symbol.for('%NS%')]||{}).GuardUiProbe()||{}).mask_display"))
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        assert_eq!(hidden, "none", "隐藏后 display 应为 none");
        assert!(mask_set_visible(&page, true).await);

        // ④ 第 143 轮:set_guard 切 partial(黑名单罩住按钮)→ 盾吞点击;
        //    先隐盾(suspend)→ 点击穿透;复盾 → 再吞。
        let mut partial = PageGuardConfig::partial(vec![], vec!["#b".into()]);
        partial.note = Some("集成验证".into());
        apply_page_guard(&page, &partial).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let shields = page
            .evaluate(
                crate::agent::browser_ns::render_ns(
                    "(function(){const p = (window[Symbol.for('%NS%')]||{}).GuardUiProbe && (window[Symbol.for('%NS%')]||{}).GuardUiProbe(); \
                 return p ? p.shields_display + '/' + p.shields_count : 'missing';})()",
                ),
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        assert_eq!(shields, "/1", "partial 黑名单应生成 1 个盾罩:{shields}");
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 3, "盾区应吞掉(合成)人工点击");
        assert!(guard_suspend_visuals(&page, true).await, "隐盾应实际应用");
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 4, "隐盾后 CDP 点击应穿透生效");
        assert!(guard_suspend_visuals(&page, false).await);
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 4, "复盾后点击应再次被吞");

        // ⑤ 第 143 轮:白名单挖洞(只允许 200x100 按钮区,垂直条带分解求补集)
        //    → 区内可点、按钮外(400,300)的命中目标应是盾罩本身。
        let whitelist = PageGuardConfig::partial(vec!["#b".into()], vec![]);
        apply_page_guard(&page, &whitelist).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let hit = page
            .evaluate(
                "(function(){const el=document.elementFromPoint(50,30); \
                 return el && el.id ? el.id : (el ? 'shield' : 'none');})()",
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        assert_eq!(hit, "b", "允许区内 elementFromPoint 应命中按钮本体:{hit}");
        let hit2 = page
            .evaluate(
                crate::agent::browser_ns::render_ns(
                    "(function(){const el=document.elementFromPoint(400,300); \
                 if (!el) return 'none'; \
                 if (el.id === '%NS%guard_ui__') return 'shield'; \
                 return el.id ? el.id : (el ? 'shield' : 'none');})()",
                ),
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        assert_eq!(hit2, "shield", "允许区外应命中盾罩(人工点击被吞):{hit2}");

        // ④ 第 163 轮回归锁死:**导航之后**视觉层必须仍在。
        //     这是本次 P0 的判别点 —— document-start 注入时 root() 为 null 抛异常,
        //     若 `DOMContentLoaded` 重试注册排在首次 `refresh()` 之后就会被整段吃掉,
        //     表现为「首开正常,首次 navigate 后蒙层/蓝色边框永久消失」,
        //     而 `apply_page_guard` 仍回报 `visual_mask: true`(谎报成功)。
        //     放在收尾前:导航会重置页面内计数器,不能插在计数类断言中间。
        page.goto(url).await.expect("第 163 轮回归:再导航一次应成功");
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        let after_nav = page
            .evaluate(
                crate::agent::browser_ns::render_ns(
                    "JSON.stringify((function(){const p = (window[Symbol.for('%NS%')]||{}).GuardUiProbe && (window[Symbol.for('%NS%')]||{}).GuardUiProbe(); \
                 return {mask: !!(p && p.mask), hint: !!(p && p.hint), \
                 frame: !!document.getElementById('%NS%agent_frame__'), \
                 shields: !!(p && p.shields)};})())",
                ),
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(
            after_nav.contains("\"mask\":true")
                && after_nav.contains("\"hint\":true")
                && after_nav.contains("\"frame\":true")
                && after_nav.contains("\"shields\":true"),
            "第 163 轮回归:导航后蒙层/蓝色边框/盾区丢失(实际 {after_nav})"
        );

        // 收尾:回收浏览器(全局单例,必须清干净防污染其它测试)
        mgr.shutdown().await;
    }
}
