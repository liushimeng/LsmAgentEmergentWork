//! 页面管控(可视化模式蒙层 / 人工操作拦截 / 部分屏蔽,第 141 轮起,第 143 轮扩展)。
//!
//! 需求:headed 可视化浏览器中,人工**可实时观看**页面变化;人工与页面的交互
//! 权限由**页面管控三档**决定(详案 `docs/MCP_Web_Use/07-页面管控三档模式与人工交互设计.md`):
//!
//! - **locked 屏蔽模式(缺省,第 141 轮)**:人工不可点击/操作(防交叉操作),
//!   人工与页面的交互一律走「人工介入」弹窗(`request_human`,提问期间默认临时解锁);
//! - **open 非屏蔽模式(第 143 轮)**:人工可直接操作页面(状态条明示「页面开放」),
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
//!   不参与 hit-test,`data-laew-agent="1"`(blockers/elements/dom 提取过滤),
//!   `Page.addScriptToEvaluateOnNewDocument` 挂载(导航自动重注入,全 frame 生效)。
//! - **L3 盾区(partial,DOM 级)**:受保护区域覆盖 `pointer-events:auto` 的半透明
//!   盾罩吞掉人工输入;黑名单 = 每命中元素一个红调盾罩,白名单 = 允许区并集在视口
//!   内做「垂直条带分解」求补集(深色调盾罩拼出整屏罩+洞的效果,纯矩形零兼容性
//!   依赖 —— 实测 `clip-path: path(evenodd,…)` 会被部分 Chrome 版本拒绝,不采纳)。
//!   盾罩同样会吞 Agent 的 CDP 点击(hit-test 层不可区分),partial 下 Agent 输入
//!   动作「先隐盾后复盾」(`__laewGuardSuspend`,见 `control::guard_lift_for_input`)。
//!
//! 为什么不用 DOM 蒙层(`pointer-events:auto`)拦**整页**输入:CDP 合成输入与真实
//! 输入在 hit-test 层不可区分(`isTrusted` 均为 true),会把 Agent 自己的点击一并
//! 吃掉;且「挂起标志」跨域 iframe 读不到,Agent 点跨域 iframe 区域会被误拦且无法
//! 解除。整页拦截用 CDP 层,只有 partial 的**分区**拦截才用 DOM 盾区(区域粒度
//! CDP 做不到,盾区脚本运行在各 frame 自己的文档里,跨域 iframe 同样罩得住)。

use chromiumoxide::cdp::browser_protocol::input::SetIgnoreInputEventsParams;
use chromiumoxide::Page;
use serde_json::{json, Value};

/// 蒙层遮罩元素 id(locked 档视觉层;测试断言锚点;`inspect(elements/dom)` 经
/// data-laew-agent 过滤)。
pub const OVERLAY_ELEMENT_ID: &str = "__laew_overlay__";

/// 状态条 id(三档共用,仅主文档注入,第 143 轮从蒙层子元素改为独立元素:
/// 蒙层只在 locked 显示,而「人工可操作」的提示在 open/partial 也要在)。
pub const OVERLAY_HINT_ID: &str = "__laew_overlay_hint__";

/// 盾区容器 id(partial 档,黑名单盾罩/白名单整屏挖洞盾的父节点)。
pub const SHIELDS_ELEMENT_ID: &str = "__laew_guard_shields__";

/// 单侧选择器(allow/block)数量上限。
pub const MAX_GUARD_SELECTORS: usize = 8;

/// 单条选择器字符上限。
pub const MAX_GUARD_SELECTOR_LEN: usize = 160;

/// `guard_note` / `note` 字符上限(状态条第三行,超长静默截断)。
pub const MAX_GUARD_NOTE_LEN: usize = 60;

// ===================== 第 143 轮:页面管控三档模型 =====================

/// 页面管控三档(第 143 轮)。三档语义与决策表见 doc 07 §3.1。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageGuardMode {
    /// 屏蔽模式(缺省,= 第 141 轮蒙层):CDP 输入拦截 + 半透明蒙层,人工可看不可点。
    Locked,
    /// 非屏蔽模式:无拦截无蒙层,状态条明示「页面开放·人工可直接操作」。
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
    /// 状态条第三行自定义提示(三档通用,≤60 字符;人工登录先行流的 UI 载体)。
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

// ===================== 管控 JS(蒙层 + 状态条 + 盾区三件套) =====================

/// 管控 JS 模板:`{MODE}`/`{ALLOW}`/`{BLOCK}`/`{NOTE}` 占位符由 [`guard_script`]
/// 烘焙为初始配置,使运行期 `set_guard` 后发生的导航按「期望态」重建
/// (配合导航类动作收尾的 re-assert,与第 141 轮同构)。
///
/// 设计要点:
/// - 蒙层 `pointer-events:none`(纯视觉,零 hit-test 参与)与状态条
///   `pointer-events:none`(三档都只是「告知」,不挡点击);
/// - 盾罩 `pointer-events:auto`(真拦截,只出现在 partial);
/// - z-index 序:高亮蓝框 2147483647 > 蒙层/状态条 2147483646 > 盾区 2147483645;
/// - `data-laew-agent="1"`:与高亮蓝框同款标记,`blockers` 正文扫描
///   (克隆 body 后剥 `[data-laew-agent]`)与 elements/dom 提取天然排除;
/// - 白名单 = 允许区并集的「垂直条带分解」补集矩形盾(x 去重分条带 → 条带内
///   y 区间合并求补),纯矩形 + pointer-events,零兼容性依赖(不用 clip-path);
/// - 盾罩矩形随滚动/缩放/DOM 变化 rAF 节流重算;MutationObserver 忽略
///   `[data-laew-agent]` 自身节点的变更(防自激循环);
/// - `document.body` 未就绪(极早期文档)时挂 documentElement,DOMContentLoaded 补挂。
pub const AGENT_GUARD_JS_TEMPLATE: &str = r#"(() => {
    if (window.__laewGuardInstalled) return;
    window.__laewGuardInstalled = true;
    window.__laewGuardMode = {MODE};
    window.__laewGuardAllow = {ALLOW};
    window.__laewGuardBlock = {BLOCK};
    window.__laewGuardNote = {NOTE};
    window.__laewGuardSuspended = false;
    const MASK_ID = '__laew_overlay__';
    const HINT_ID = '__laew_overlay_hint__';
    const SHIELDS_ID = '__laew_guard_shields__';
    const CHIP_TEXT = {
        locked: ['🔒 LAEW Agent 控制中 · 页面已锁定(仅观看)',
                 '人工操作已拦截;如需介入请应答「人工介入」弹窗'],
        open:   ['🔓 页面开放 · 人工可直接操作',
                 'Agent 协同中;人工完成操作后应答「人工介入」弹窗告知'],
        partial:['🛡 部分锁定 · 盾区不可操作',
                 '其余区域人工可直接操作;需要调整范围请告知 Agent'],
    };
    const el = (id) => document.getElementById(id);
    const root = () => document.body || document.documentElement;
    const ensure = () => {
        if (!el(MASK_ID)) {
            const m = document.createElement('div');
            m.id = MASK_ID;
            m.setAttribute('data-laew-agent', '1');
            m.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483646;'
                + 'background:rgba(15,23,42,0.22);';
            root().appendChild(m);
        }
        if (window.top === window && !el(HINT_ID)) {
            const c = document.createElement('div');
            c.id = HINT_ID;
            c.setAttribute('data-laew-agent', '1');
            c.style.cssText = 'position:fixed;left:18px;bottom:18px;margin:0;padding:8px 14px;'
                + 'background:rgba(15,23,42,0.85);color:#e2e8f0;'
                + 'font:600 13px/1.7 system-ui,-apple-system,"PingFang SC","Microsoft YaHei",sans-serif;'
                + 'border-radius:10px;box-shadow:0 4px 16px rgba(0,0,0,0.35);max-width:72%;'
                + 'pointer-events:none;z-index:2147483646;';
            const t = document.createElement('div');
            t.setAttribute('data-laew-agent', '1');
            t.className = '__laew_guard_hint_title__';
            const s = document.createElement('div');
            s.setAttribute('data-laew-agent', '1');
            s.className = '__laew_guard_hint_sub__';
            s.style.cssText = 'font-weight:400;color:#94a3b8;font-size:12px;';
            const n = document.createElement('div');
            n.setAttribute('data-laew-agent', '1');
            n.className = '__laew_guard_hint_note__';
            n.style.cssText = 'font-weight:400;color:#cbd5e1;font-size:12px;margin-top:2px;';
            c.appendChild(t); c.appendChild(s); c.appendChild(n);
            root().appendChild(c);
        }
        if (!el(SHIELDS_ID)) {
            const sc = document.createElement('div');
            sc.id = SHIELDS_ID;
            sc.setAttribute('data-laew-agent', '1');
            sc.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483645;';
            root().appendChild(sc);
        }
    };
    const setChip = (mode, note) => {
        const c = el(HINT_ID);
        if (!c) return;
        const pair = CHIP_TEXT[mode] || CHIP_TEXT.locked;
        const t = c.querySelector('.__laew_guard_hint_title__');
        const s = c.querySelector('.__laew_guard_hint_sub__');
        const n = c.querySelector('.__laew_guard_hint_note__');
        if (t) t.textContent = pair[0];
        if (s) s.textContent = pair[1];
        if (n) { n.textContent = note || ''; n.style.display = note ? '' : 'none'; }
    };
    const rebuildShields = () => {
        const sc = el(SHIELDS_ID);
        if (!sc) return;
        while (sc.firstChild) sc.removeChild(sc.firstChild);
        const allow = Array.isArray(window.__laewGuardAllow) ? window.__laewGuardAllow : [];
        const block = Array.isArray(window.__laewGuardBlock) ? window.__laewGuardBlock : [];
        const vw = window.innerWidth, vh = window.innerHeight;
        // 盾罩标签预算(选择器命中很多元素时避免满屏小标签)
        let tagBudget = 6;
        const addShield = (x, y, w, h, dark) => {
            const sh = document.createElement('div');
            sh.setAttribute('data-laew-agent', '1');
            sh.style.cssText = 'position:fixed;left:' + x + 'px;top:' + y + 'px;width:' + w
                + 'px;height:' + h + 'px;pointer-events:auto;z-index:2147483645;'
                + (dark
                    ? 'background:rgba(15,23,42,0.30);'
                    : 'background:rgba(185,28,28,0.18);border:1.5px dashed rgba(185,28,28,0.8);')
                + 'box-sizing:border-box;';
            if (tagBudget > 0) {
                tagBudget--;
                const tag = document.createElement('div');
                tag.setAttribute('data-laew-agent', '1');
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
            // 白名单:整屏深色罩挖洞 —— 对允许矩形并集在视口内做「垂直条带分解」
            // 求补集(若干互不重叠的矩形盾)。纯矩形 + pointer-events,零兼容性依赖
            //(实测 clip-path: path(evenodd,…) 会被部分 Chrome 版本拒绝,不采纳)。
            const rects = [];
            for (const sel of allow) {
                try {
                    for (const node of document.querySelectorAll(sel)) {
                        const r = node.getBoundingClientRect();
                        const x1 = Math.max(0, r.left), y1 = Math.max(0, r.top);
                        const x2 = Math.min(vw, r.right), y2 = Math.min(vh, r.bottom);
                        if (x2 - x1 > 0 && y2 - y1 > 0) rects.push([x1, y1, x2, y2]);
                    }
                } catch (e) {}
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
        const mode = window.__laewGuardMode;
        const susp = !!window.__laewGuardSuspended;
        ensure();
        const m = el(MASK_ID);
        if (m) m.style.display = (mode === 'locked' && !susp) ? '' : 'none';
        const c = el(HINT_ID);
        if (c) {
            c.style.display = susp ? 'none' : '';
            if (!susp) setChip(mode, window.__laewGuardNote);
        }
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
    window.__laewGuardSet = (mode, allow, block, note) => {
        window.__laewGuardMode = String(mode || 'locked');
        window.__laewGuardAllow = Array.isArray(allow) ? allow : [];
        window.__laewGuardBlock = Array.isArray(block) ? block : [];
        window.__laewGuardNote = (typeof note === 'string' && note) ? note : null;
        refresh();
        return window.__laewGuardMode;
    };
    window.__laewGuardSuspend = (s) => {
        window.__laewGuardSuspended = !!s;
        refresh();
        return true;
    };
    // 第 141 轮 legacy 兼容:蒙层布尔开关(set_overlay 旧路径/单测锚点);
    // true=切屏蔽模式,false=切开放模式(模式级开关,非可见性开关)。
    window.__laewOverlaySet = (on) =>
        window.__laewGuardSet(on ? 'locked' : 'open', [], [], null);
    let raf = 0;
    const schedule = () => {
        if (window.__laewGuardMode !== 'partial' || window.__laewGuardSuspended) return;
        if (!raf) raf = window.requestAnimationFrame(() => { raf = 0; refresh(); });
    };
    window.addEventListener('scroll', schedule, { passive: true, capture: true });
    window.addEventListener('resize', schedule, { passive: true });
    try {
        new MutationObserver((muts) => {
            for (const mu of muts) {
                const t = mu.target;
                if (t && t.closest && t.closest('[data-laew-agent]')) continue;
                return schedule();
            }
        }).observe(document.documentElement, {
            childList: true, subtree: true,
            attributes: true, attributeFilter: ['class', 'style'],
        });
    } catch (e) {}
    refresh();
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', () => refresh(), { once: true });
    }
})()"#;

/// 按实例期望态生成注入脚本(烘焙初始配置;选择器/note 以 JSON 字面量嵌入)。
pub fn guard_script(cfg: &PageGuardConfig) -> String {
    let cfg = cfg.normalized();
    let mode = serde_json::to_string(cfg.mode.as_str()).unwrap_or_else(|_| "\"locked\"".into());
    let allow = serde_json::to_string(&cfg.allow_selectors).unwrap_or_else(|_| "[]".into());
    let block = serde_json::to_string(&cfg.block_selectors).unwrap_or_else(|_| "[]".into());
    let note = serde_json::to_string(&cfg.note).unwrap_or_else(|_| "null".into());
    AGENT_GUARD_JS_TEMPLATE
        .replace("{MODE}", &mode)
        .replace("{ALLOW}", &allow)
        .replace("{BLOCK}", &block)
        .replace("{NOTE}", &note)
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
/// 第 143 轮起 headed(非 connect)全档注入(open 档也有状态条告知人工可操作)。
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
}

/// 应用页面管控三档(L1 CDP 输入拦截按档 + JS 全量设置)。
///
/// - locked:`setIgnoreInputEvents(true)` + `__laewGuardSet('locked',…)`(蒙层+锁定状态条);
/// - open:`setIgnoreInputEvents(false)` + `__laewGuardSet('open',…)`(放行+开放状态条);
/// - partial:`setIgnoreInputEvents(false)` + `__laewGuardSet('partial',…)`(盾区+状态条)。
///
/// 返回应用结果(fail-open:单层失败不阻断另一层,如实回报);
/// 未注入脚本的页面(hidden/connect/注入失败)JS 侧返回 null → applied=false。
pub async fn apply_page_guard(
    page: &Page,
    cfg: &PageGuardConfig,
) -> std::result::Result<Value, String> {
    let cfg = cfg.normalized();
    let input_locked = cfg.mode == PageGuardMode::Locked;
    // L1:CDP 输入拦截(实测同时拦 CDP 合成输入 → locked 下输入动作须先解后锁)
    let input_lock_applied = page
        .execute(SetIgnoreInputEventsParams::new(input_locked))
        .await
        .is_ok();
    // L2/L3:JS 蒙层 + 状态条 + 盾区(未注入脚本的页面返回 null → applied=false)
    let allow = serde_json::to_string(&cfg.allow_selectors).unwrap_or_else(|_| "[]".into());
    let block = serde_json::to_string(&cfg.block_selectors).unwrap_or_else(|_| "[]".into());
    let note = serde_json::to_string(&cfg.note).unwrap_or_else(|_| "null".into());
    let mode = serde_json::to_string(cfg.mode.as_str()).unwrap_or_else(|_| "\"locked\"".into());
    let js = format!(
        "window.__laewGuardSet ? window.__laewGuardSet({mode}, {allow}, {block}, {note}) : null"
    );
    let r = page
        .evaluate(js.as_str())
        .await
        .map_err(|e| format!("应用页面管控失败:{e}"))?;
    let js_applied = !r.value().map(|v| v.is_null()).unwrap_or(true);
    Ok(json!({
        "mode": cfg.mode.as_str(),
        "input_locked": input_locked && input_lock_applied,
        "input_lock_applied": input_lock_applied,
        "visual_mask": cfg.mode == PageGuardMode::Locked && js_applied,
        "shields_active": cfg.mode == PageGuardMode::Partial && js_applied,
    }))
}

/// 挂起/恢复全部管控视觉(蒙层 + 状态条 + 盾区;不动 CDP 输入拦截)。
///
/// 三个使用点:截图/OCR/HITL 附图「拍前隐藏 → 拍完恢复」(证据链干净);
/// partial 下 Agent 输入动作「先隐盾后复盾」;不关心 CDP 的纯视觉场景。
/// 返回是否实际应用(未注入脚本 = false)。
pub async fn guard_suspend_visuals(page: &Page, hide: bool) -> bool {
    let js = format!("window.__laewGuardSuspend ? window.__laewGuardSuspend({hide}) : null");
    match page.evaluate(js.as_str()).await {
        Ok(r) => !r.value().map(|v| v.is_null()).unwrap_or(true),
        Err(_) => false,
    }
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
/// 现在连状态条一起隐藏 —— 证据更干净)。
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
        let mut cfg = PageGuardConfig::partial(vec![".login-box".into()], vec![]);
        cfg.note = Some("请登录后应答弹窗".into());
        let js = guard_script(&cfg);
        assert!(js.contains("window.__laewGuardMode = \"partial\";"), "{js}");
        assert!(js.contains("window.__laewGuardAllow = [\".login-box\"];"), "{js}");
        assert!(js.contains("window.__laewGuardBlock = [];"), "{js}");
        assert!(
            js.contains("window.__laewGuardNote = \"请登录后应答弹窗\";"),
            "{js}"
        );
        // 占位符全部被烘焙
        for ph in ["{MODE}", "{ALLOW}", "{BLOCK}", "{NOTE}"] {
            assert!(!js.contains(ph), "占位符 {ph} 未烘焙");
        }
        // locked 缺省烘焙
        let js2 = guard_script(&PageGuardConfig::locked());
        assert!(js2.contains("window.__laewGuardMode = \"locked\";"));
        assert!(js2.contains("window.__laewGuardNote = null;"));
    }

    /// 管控脚本关键标记锁死:三件套元素 id / 提取过滤(data-laew-agent)/
    /// 三档状态条文案 / 挖洞 clip-path / 挂载函数 / pointer-events 语义。
    #[test]
    fn guard_script_markers_locked() {
        let js = guard_script(&PageGuardConfig::locked());
        assert!(js.contains(OVERLAY_ELEMENT_ID), "蒙层元素 id 应出现在脚本中");
        assert!(js.contains(OVERLAY_HINT_ID), "状态条 id 应出现在脚本中");
        assert!(js.contains(SHIELDS_ELEMENT_ID), "盾区容器 id 应出现在脚本中");
        assert!(js.contains("data-laew-agent"));
        assert!(js.contains("z-index:2147483646"), "蒙层/状态条 z-index");
        assert!(js.contains("z-index:2147483645"), "盾区 z-index 恰低一档");
        assert!(js.contains("window.__laewGuardSet"));
        assert!(js.contains("window.__laewGuardSuspend"));
        assert!(js.contains("window.__laewOverlaySet"), "第 141 轮 legacy 别名保留");
        assert!(js.contains("window.top === window"), "状态条仅主文档");
        // 三档状态条文案
        assert!(js.contains("页面已锁定(仅观看)"));
        assert!(js.contains("页面开放 · 人工可直接操作"));
        assert!(js.contains("盾区不可操作"));
        // 盾区技术要点:白名单垂直条带分解求补集(纯矩形,零 clip-path 依赖)+
        // 黑名单红调盾罩 + 标签预算
        assert!(js.contains("垂直条带分解"));
        assert!(!js.contains("clipPath"), "不依赖 clip-path(部分 Chrome 拒绝 evenodd path)");
        assert!(js.contains("rgba(185,28,28,0.18)"), "黑名单红调盾罩");
        assert!(js.contains("rgba(15,23,42,0.30)"), "白名单深色盾罩");
        assert!(js.contains("已锁定(仅 Agent)"));
        assert!(js.contains("tagBudget"), "盾罩标签应有数量预算");
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

    #[test]
    fn input_action_whitelist() {
        // 注入输入的动作必须让路(含 chromiumoxide Element click/type 路径)
        for a in [
            "click", "human_click", "right_click", "double_click", "hover", "scroll",
            "scroll_to", "key_press", "press_sequence", "input_text", "human_input",
            "clear_input", "upload_file", "select_option", "download", "drag", "focus",
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
    // 覆盖第 141 轮三条关键链路(详案 docs/MCP_Web_Use/05 §10)+ 第 143 轮 partial:
    // 1. 管控元素注入存在性(headed + locked;含三档共用状态条);
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

        // ① 管控元素存在(蒙层/状态条在主文档,盾区容器在)
        let mask_probe = page
            .evaluate(
                "!!document.getElementById('__laew_overlay__') && !!document.getElementById('__laew_overlay_hint__') \
                 && !!document.getElementById('__laew_guard_shields__')",
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(mask_probe, "蒙层/状态条/盾区容器未注入");

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

        // ③ 蒙层隐藏/恢复(截图避让通道)
        assert!(mask_set_visible(&page, false).await, "蒙层隐藏应实际应用");
        let hidden = page
            .evaluate("getComputedStyle(document.getElementById('__laew_overlay__')).display")
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
                "(function(){const sc=document.getElementById('__laew_guard_shields__'); \
                 return sc ? sc.style.display + '/' + sc.childElementCount : 'missing';})()",
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        assert_eq!(shields, "/1", "partial 黑名单应生成 1 个盾罩:{shields}");
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 1, "盾区应吞掉(合成)人工点击");
        assert!(guard_suspend_visuals(&page, true).await, "隐盾应实际应用");
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 2, "隐盾后 CDP 点击应穿透生效");
        assert!(guard_suspend_visuals(&page, false).await);
        cdp_click(&page).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(count(&page).await, 2, "复盾后点击应再次被吞");

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
                "(function(){const el=document.elementFromPoint(400,300); \
                 return el && el.id ? el.id : (el ? 'shield' : 'none');})()",
            )
            .await
            .ok()
            .and_then(|r| r.value().cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        assert_eq!(hit2, "shield", "允许区外应命中盾罩(人工点击被吞):{hit2}");

        // 收尾:回收浏览器(全局单例,必须清干净防污染其它测试)
        mgr.shutdown().await;
    }
}
