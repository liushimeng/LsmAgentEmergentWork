//! 注入命名空间（第 168 轮）：向页面注入的 guard / highlight / follow / 探测脚本
//! 统一使用进程级随机前缀，替代固定 `__laew*` 命名。
//!
//! 背景：headed 缺省模式每个页面 document-start 注入两套脚本，固定
//! `window.__laew*` 全局名与 `data-laew-agent` 属性构成一行枚举即可命中的
//! 自动化框架签名；天猫/京东系风控 JS（aplus/um/jdfacter）在登录风控复核时
//! 采集该指纹会把设备环境打到高风险分（实测：人工扫码登录成功后 JD 返回
//! `.jd-main-risk-fallback` 风控页，「当前页面异常/请刷新或切换账户」）。
//! 本模块把固定名换成进程级随机名（如 `__lzn8f3a2b1`），注入脚本尾部再由
//! 不可枚举化 bootstrap 收敛 `for…in` / `Object.keys` 可见性 —— 环境表现
//! 与装了用户脚本的正常浏览器同水位（puppeteer-extra-stealth 同款策略）。
//!
//! 模板约定：JS 模板内一律写占位符 `%NS%`（非法 JS 标识符，模板内天然零碰撞），
//! 渲染时 `.replace(NS_TOKEN, ns)` —— 不用 `format!`，避免 JS 大括号转义地狱。
//!
//! **第 179 轮：注入面收敛**。此前每个注入 API 都是 `window.<ns>Foo` 形式的
//! **字符串全局键** —— 真机实测有头档每页在 `window` 上留 30 个同前缀键，
//! `Object.getOwnPropertyNames(window)` 一行即可枚举（这是比 `webdriver`、
//! WebGL 兜底串更独有的一处签名）。现在所有窗口面 API 收敛到**一个**
//! `window[Symbol.for('<ns>')]` 命名空间对象上：字符串键 30 → 0，符号键 0 → 1
//! （`Object.keys` / `for…in` / `getOwnPropertyNames` 全部不可见）。
//! DOM 面（element id / class / `data-*` 属性）换用**独立的第二个随机 token**
//! —— 否则「枚举 DOM id 前缀」与「读符号描述」能把两处痕迹串成同一个签名。

use std::sync::OnceLock;

/// 模板占位符（不是合法 JS 标识符，绝不会与模板内代码碰撞）。
pub const NS_TOKEN: &str = "%NS%";

/// 幂等闸的块位（第 177 轮）：`window.<ns>StealthRev` 是**块位掩码**而非单一版本号，
/// 跨档升级时只补跑缺位对应的块。
pub const BLOCK_CORE: u32 = 1;
pub const BLOCK_FIDELITY: u32 = 2;
pub const BLOCK_HARDENED: u32 = 4;

/// window 面上承载注入 API 的全局后缀白名单（第 179 轮）。
///
/// 渲染期把 `window.<ns><Suffix>` 改写成 `NS.<Suffix>` —— `NS` 是各模板闭包内
/// 绑定的**命名空间对象**（由 `window[Symbol.for('<ns>')]` 承载）。页面侧
/// `getOwnPropertyNames(window)` 因此看不到任何注入层字符串键。
///
/// 新增窗口面 API 时**必须**把后缀加进本表，否则它会退回 `window.<ns>Foo`
/// 字符串键形态（单测 `渲染后不得残留窗口字符串键` 会拦住）。
const NS_WINDOW_SUFFIXES: &[&str] = &[
    // 高亮蓝框
    "AgentApplyHighlight",
    "AgentHighlightInstalled",
    "AgentHighlight",
    // 自动跟随
    "AutoFollowStatus",
    "AutoFollow",
    // 页面管控
    "GuardInputSuspended",
    "GuardInputSuspend",
    "GuardGateReady",
    "GuardScrollWatch",
    "GuardSuspended",
    "GuardInstalled",
    "GuardUiProbe",
    "GuardAllow",
    "GuardBlock",
    "GuardNote",
    "GuardMode",
    "GuardRev",
    "GuardRO",
    "GuardMO",
    "GuardSet",
    "GuardSuspend",
    "GuardOff",
    "OverlaySet",
    // 屏蔽页面状态栏 / 进度栏
    "StatusUpdate",
    "StatusGuard",
    "StatusVisible",
    "StatusSuspend",
    "StatusEnsure",
    "StatusProbe",
    "StatusTimer",
    "StatusMO",
    "StatusRev",
    // 隐身幂等闸 / 可见文本读取
    "StealthRev",
    "BodyVisibleText",
];

/// DOM 面上带命名空间前缀的 id / class / `data-*` 后缀（第 179 轮）。
///
/// 这些**留在 DOM 面**（元素 id 仍然要按 id 查找），但换用与 window 符号无关的
/// 第二个随机 token —— 否则枚举 DOM id 前缀 + 读符号描述就能把两处痕迹串起来。
const NS_DOM_SUFFIXES: &[&str] = &[
    "agent_frame__",
    "guard_shields__",
    "guard_status__",
    "guard_progress__",
    "guard_style__",
    "guard_ui__",
    "status_host__",
    "progress_host__",
    "overlay__",
    "locked__",
    "sb_title__",
    "sb_mode__",
    "sb_notice__",
    "sb_note__",
    "pb_action__",
    "pb_progress__",
    "pb_fill__",
    "pb_task__",
    "pb_events__",
    // 短后缀放最后（必须先替换掉上面的长后缀，否则 `agent` 会把
    // `agent_frame__` 截成 `<dom>agent_frame__` 的两段拼接）
    "agent",
    "sh",
];

/// 按长度降序取窗口面后缀（长后缀优先，避免 `GuardSuspend` 抢 `GuardSuspended`、
/// `AutoFollow` 抢 `AutoFollowStatus`、`agent` 抢 `agent_frame__`）。
fn sorted_suffixes(list: &'static [&'static str]) -> Vec<&'static str> {
    let mut v = list.to_vec();
    v.sort_by_key(|s| std::cmp::Reverse(s.len()));
    v
}

fn window_suffixes() -> &'static [&'static str] {
    static V: OnceLock<Vec<&'static str>> = OnceLock::new();
    V.get_or_init(|| sorted_suffixes(NS_WINDOW_SUFFIXES))
}

fn dom_suffixes() -> &'static [&'static str] {
    static V: OnceLock<Vec<&'static str>> = OnceLock::new();
    V.get_or_init(|| sorted_suffixes(NS_DOM_SUFFIXES))
}

/// 渲染注入模板（第 179 轮三步）：
///
/// 1. `%NS%` → 窗口面命名空间（symbol 描述）；
/// 2. `window.<ns><Suffix>` → `NS.<Suffix>`（窗口面 API 收敛到命名空间对象，
///    模板需在闭包顶部绑定 `const NS = window[Symbol.for('%NS%')] || (… = {})`）；
/// 3. `<ns><DomSuffix>` → `<dom_ns><DomSuffix>`（DOM 面换用第二个随机 token）。
///
/// 不用 `format!`（JS 大括号太多），全部走字符串替换。
pub fn render_ns(template: &str) -> String {
    render_with(template, inject_ns(), dom_ns())
}

fn render_with(template: &str, ns: &str, dom: &str) -> String {
    let mut out = template.replace(NS_TOKEN, ns);
    for suffix in window_suffixes() {
        let needle = format!("window.{ns}{suffix}");
        if out.contains(&needle) {
            out = out.replace(&needle, &format!("NS.{suffix}"));
        }
    }
    for suffix in dom_suffixes() {
        let needle = format!("{ns}{suffix}");
        if out.contains(&needle) {
            out = out.replace(&needle, &format!("{dom}{suffix}"));
        }
    }
    out
}

/// 模板闭包顶部的命名空间绑定片段（各模板内联同一行，零全局键）。
///
/// `const NS` 是**闭包局部**，不落 `window`；命名空间对象本身由 symbol 键承载。
pub const NS_BINDING_JS: &str =
    "const NS = window[Symbol.for('%NS%')] || (window[Symbol.for('%NS%')] = {});";

/// 进程级注入命名空间，惰性生成一次：`__lzn` + 8 位十六进制（如 `__lzn8f3a2b1`）。
static NS: OnceLock<String> = OnceLock::new();

/// 进程级 **DOM 面**命名空间（第 179 轮）：与窗口面 token 独立。
///
/// 形如 `d1a2b3c4`（字母开头，保证可作 CSS class / id 选择器），不含 `__lzn`
/// 前缀 —— DOM 枚举与符号描述之间不再有可关联的字符串。
static DOM_NS: OnceLock<String> = OnceLock::new();

/// 取（或首取生成）本进程的注入命名空间。
pub fn inject_ns() -> &'static str {
    NS.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 ^ (d.as_secs() << 20))
            .unwrap_or(0x5eaf00d);
        let pid = std::process::id() as u64;
        format!("__lzn{:08x}", (nanos ^ pid.wrapping_mul(0x9e3779b1)) & 0xffff_ffff)
    })
}

/// 取（或首取生成）本进程的 **DOM 面**命名空间（与窗口面 token 独立）。
pub fn dom_ns() -> &'static str {
    DOM_NS.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 ^ (d.as_secs() << 17))
            .unwrap_or(0x1d0c0de);
        let pid = std::process::id() as u64;
        // 字母开头：`#id` / `.class` 选择器要求首字符不能是数字。
        format!("d{:08x}", (nanos ^ pid.wrapping_mul(0x85eb_ca6b)) & 0xffff_ffff)
    })
}

/// 命名空间对象表达式（第 179 轮）：`window[Symbol.for('<ns>')]`。
pub fn ns_object_expr() -> String {
    format!("window[Symbol.for('{}')]", inject_ns())
}

/// 命名空间成员**读**表达式（供 Rust 侧拼 eval 字符串，零全局字符串键）。
///
/// `ns_member_expr("GuardSet")` → `(window[Symbol.for('<ns>')]||{}).GuardSet` ——
/// 命名空间不存在时读到 `undefined` 而非抛错（与旧 `window.<ns>GuardSet` 同语义）。
pub fn ns_member_expr(suffix: &str) -> String {
    format!("(window[Symbol.for('{}')]||{{}}).{}", inject_ns(), suffix)
}

/// 命名空间对象初始化语句（供 Rust 侧拼多行脚本时使用）。
pub fn ns_bootstrap_js() -> String {
    let ns = inject_ns();
    format!("window[Symbol.for('{ns}')] || (window[Symbol.for('{ns}')] = {{}});")
}

/// 注入层 DOM 过滤属性名：`data-<dom_ns>agent`（替代固定 `data-laew-agent`）。
pub fn agent_attr() -> String {
    format!("data-{}agent", dom_ns())
}

/// 注入层 DOM 过滤选择器：`[data-__lzn8f3a2b1agent]`。
pub fn agent_attr_selector() -> String {
    format!("[{}]", agent_attr())
}

/// 前置块：被替换函数的身份登记表（第 174 轮）。
///
/// 保真档只多一个空 `WeakMap`（零可观测差异）；侵入式档由 [`HARDENED_JS`] 读取它
/// —— 登记过的函数在 `Function.prototype.toString` 下报 `[native code]`，
/// 使「我们改过」这件事本身不可被反查。
const PRELUDE_JS: &str = r#"
    const __fd = new WeakMap();
    const reg = (fn, name) => {
        try { __fd.set(fn, 'function ' + name + '() { [native code] }'); } catch (e) {}
        return fn;
    };
    // 第 177 轮:「重新认领」—— 本脚本每次重注入都新建一个 `__fd`(闭包常量,
    // 不落 window 故零可枚举键)。跨档升级时(保真→侵入)新表看不见**上一轮**注入
    // 的那些改写函数,它们会退回可读的源码形态。解法不是把表挂到 window(那会新增
    // 一个 `getOwnPropertyNames` 可见的全局键,本身即签名),而是在装原生化之前把
    // 「上一轮可能改过、且页面可触达」的固定位置重新扫一遍并登记进新表 ——
    // 页面能拿到的引用只有这些入口,扫全它们即等价于跨轮继承登记表。
    const __adopt = (obj, key) => {
        try {
            if (!obj) return;
            const d = Object.getOwnPropertyDescriptor(obj, key);
            if (!d) return;
            if (typeof d.get === 'function') reg(d.get, 'get ' + key);
            if (typeof d.value === 'function') reg(d.value, key);
        } catch (e) {}
    };
    const __adoptProto = (proto) => {
        try {
            if (!proto) return;
            for (const k of ['item', 'namedItem', 'refresh']) __adopt(proto, k);
            __adopt(proto, Symbol.iterator);
        } catch (e) {}
    };
    // 第 181 轮:嵌套函数认领(按类收口,不是补点)。`__adopt` 只认领「属性值本身
    // 是函数 / 属性是 getter」,对**对象值里的自有函数**一个都认领不上 ——
    // `FIDELITY_JS` ④ 造的 `window.chrome = {app, csi, loadTimes}` 正是这种形态,
    // 于是 `__adopt(window, 'chrome')` 全数跳过:跨档升级(保真→侵入)后本轮 `__fd`
    // 是全新表,`chrome.csi.toString()` 会吐出我们的源码。与第 177 轮修的迭代器
    // 漏登记(`next`/自反方法每次现造、原型上无引用)同类,当时只补了迭代器这一条。
    // 深度上限 2、只走自有属性名:够到 chrome 三件套与其子对象,又不会爬进
    // 扩展注入的深层对象图。真机原生函数被认领是零可观测差异(其 toString 本就
    // 报 [native code],登记表覆盖后返回同一串)。
    const __adoptDeep = (obj, depth) => {
        try {
            if (!obj || typeof obj !== 'object' || (depth | 0) > 2) return;
            const keys = Object.getOwnPropertyNames(obj);
            for (let i = 0; i < keys.length; i++) {
                try {
                    const d = Object.getOwnPropertyDescriptor(obj, keys[i]);
                    if (!d) continue;
                    if (typeof d.get === 'function') reg(d.get, 'get ' + keys[i]);
                    else if (typeof d.value === 'function') reg(d.value, keys[i]);
                    else if (d.value && typeof d.value === 'object') {
                        __adoptDeep(d.value, (depth | 0) + 1);
                    }
                } catch (e) {}
            }
        } catch (e) {}
    };
    // 迭代器工厂:保真块与「重装迭代器」共用同一份实现,保证两侧产出的 next /
    // 自反方法永远在**当前这张表**里登记(第 177 轮)。
    const __mkIterator = (items) => {
        let i = 0;
        const iter = { next: reg(function next() {
            return i < items.length
                ? { value: items[i++], done: false }
                : { value: undefined, done: true };
        }, 'next') };
        iter[Symbol.iterator] = reg(function () { return iter; }, '[Symbol.iterator]');
        return iter;
    };
    // 跨轮认领的例外:迭代器的 next / 自反方法是**每次调用现造的新对象**,原型上
    // 根本没有引用,认领不到。解法不是给原型挂引用(那会新增可枚举痕迹),而是把
    // 迭代器工厂**重装**一遍 —— items 用原生 `item()` 逐位反查回来(不额外存状态)。
    // 只重装**我们自己造的中间原型**,真原型(PluginArray.prototype 等)一律不碰:
    // 在真实插件存在的环境里替换原生迭代器是把好环境改坏。
    const __rehardenIterator = (arr, RealProto) => {
        try {
            if (!arr || typeof arr.length !== 'number' || !RealProto) return;
            const proto = Object.getPrototypeOf(arr);
            if (!proto || proto === RealProto) return;
            const items = [];
            for (let i = 0; i < arr.length; i++) {
                const one = proto.item ? proto.item.call(arr, i) : null;
                if (!one) return;
                items.push(one);
            }
            Object.defineProperty(proto, Symbol.iterator, {
                value: reg(function () { return __mkIterator(items); }, '[Symbol.iterator]'),
                writable: true, enumerable: false, configurable: true
            });
        } catch (e) {}
    };
    const __adoptAll = () => {
        try {
            __adopt(Navigator.prototype, 'webdriver');
            __adopt(Navigator.prototype, 'plugins');
            __adopt(Navigator.prototype, 'mimeTypes');
            __adopt(Navigator.prototype, 'languages');
            __adopt(Navigator.prototype, 'pdfViewerEnabled');
            // 第 181 轮:chrome 走深度认领。旧的 `__adopt(window,'chrome')` 只看
            // 「chrome 本身是不是函数」—— 它是对象,于是三件套(csi/loadTimes/app)
            // 一个都没进登记表,跨档升级后 toString 会吐出我们的源码。
            __adoptDeep(window.chrome, 0);
            __adopt(window.Permissions && Permissions.prototype, 'query');
            // 第 184 轮:FIDELITY_JS ⑥ 的 isExtended 存取器回填(跨档升级认领面收全;
            // 真原生函数被认领是零可观测差异 —— 其 toString 本就报 [native code])
            __adopt(window.Screen && Screen.prototype, 'isExtended');
            __adopt(window.WebGLRenderingContext && WebGLRenderingContext.prototype, 'getParameter');
            __adopt(window.WebGL2RenderingContext && WebGL2RenderingContext.prototype, 'getParameter');
            // 回填的 plugins 数组体与其元素/子数组的**中间原型**(方法都挂在那儿)
            const arr = navigator.plugins;
            const PA = window.PluginArray && PluginArray.prototype;
            const MA = window.MimeTypeArray && MimeTypeArray.prototype;
            if (arr && arr.length) {
                __adoptProto(Object.getPrototypeOf(arr));
                __rehardenIterator(arr, PA);
                __rehardenIterator(navigator.mimeTypes, MA);
                const p0 = arr[0];
                if (p0) {
                    __adoptProto(Object.getPrototypeOf(p0));
                    const mt = p0.mimeTypes;
                    if (mt) { __adoptProto(Object.getPrototypeOf(mt)); __rehardenIterator(mt, MA); }
                }
                const m0 = arr[0] && arr[0][0];
                if (m0) __adoptProto(Object.getPrototypeOf(m0));
            }
        } catch (e) {}
    };"#;

/// 核心块：document-start 压掉 `navigator.webdriver`（第 169 轮，`off` 之外恒注入）。
///
/// 背景：Chrome 155 实测 —— 即使 `disable_default_args()` 剔除
/// `--enable-automation` 且显式传 `--disable-blink-features=AutomationControlled`
/// （`ps` 实证两参数均正确生效），页面读 `navigator.webdriver` **仍为 `true`**
/// （`browser.rs` 隐身自检两轮捕获，2026-10-13/14 天猫京东实测）。天猫/京东系风控 JS
/// （aplus/um/jdfacter）在登录风控复核时采集该指纹，把设备环境打到高风险分
/// （实测：人工扫码登录成功后京东返回 `.jd-main-risk-fallback`「当前页面异常」，
/// jsonpLogin / pc_home_feed 服务端 403）。对策与 puppeteer-extra-stealth 同款：
/// document-start 覆写原型 getter，返回 `false`（与真实 Chrome 语义一致；
/// `undefined` 反而是异常值）。注入失败 fail-open（只损隐身，不损功能）。
///
/// 描述符 `{get, configurable: true, enumerable: true}` 与真实 Chrome 的
/// `Navigator.prototype.webdriver` 存取器同形（`setter` 缺省 undefined 一致）。
const CORE_JS: &str = r#"
    try {
        Object.defineProperty(Navigator.prototype, 'webdriver', {
            get: reg(function webdriver() { return false; }, 'get webdriver'),
            configurable: true, enumerable: true
        });
    } catch (e) {}"#;

/// 保真块（**非侵入式**，第 174 轮）：只移除自动化泄漏 + 回填页面自报真值。
///
/// 三原则：① 只在页面读到**兜底值 / 空值**时回填，真实值原样放行；
/// ② 不改变任何渲染与业务行为（只影响被读到的字段本身）；
/// ③ 被替换的原生函数/属性描述符形态与原生一致（`enumerable: true`）——
/// 否则 `for…in` 枚举原型链即可发现「这个函数被改过」，等于自己制造签名。
/// 杀开关：`LAEW_WEB_FINGERPRINT=off`。
const FIDELITY_JS: &str = r#"
    // ① WebGL 硬件指纹保真回填(第 172 轮,BOSS 直聘实测)。
    //    实测:即便 webdriver 已被压掉,页面读 WebGL 拿到的仍是
    //    `WebKit | WebKit WebGL` —— Chrome 取不到 `WEBGL_debug_renderer_info`
    //    时的**自动化兜底串**;真实 macOS Chrome 给的是
    //    `Google Inc. (Apple) / ANGLE (Apple, ANGLE Metal Renderer: Apple M…)`。
    //    边界:**只覆写 UNMASKED_*(37445/37446)**,且只在读数是兜底串时;
    //    低熵 VENDOR/RENDERER(7936/7937)**必须保持原生** —— 真实 Chrome 恒报
    //    `WebKit` / `WebKit WebGL`(WebGL 规范固定串),改写它们反而制造不一致。
    try {
        const isFallback = (s) => {
            const t = String(s || '').toLowerCase();
            return !t || t.indexOf('webkit') >= 0 || t.indexOf('swiftshader') >= 0
                || t.indexOf('software') >= 0;
        };
        const plat = navigator.platform || '';
        const ua = navigator.userAgent || '';
        let vendor = 'Google Inc. (Google)';
        let renderer = 'ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device), SwiftShader driver)';
        if (/Mac/i.test(plat) || /Macintosh/i.test(ua)) {
            vendor = 'Google Inc. (Apple)';
            renderer = 'ANGLE (Apple, ANGLE Metal Renderer: Apple, Unspecified)';
        } else if (/Windows/i.test(plat) || /Windows NT/i.test(ua)) {
            vendor = 'Google Inc. (Intel)';
            renderer = 'ANGLE (Intel, Intel(R) Iris(R) Xe Graphics, D3D11)';
        }
        const patch = (proto) => {
            if (!proto || !proto.getParameter) return;
            const orig = proto.getParameter;
            const wrapped = reg(function getParameter(pname) {
                const v = orig.apply(this, arguments);
                try {
                    // 37445=UNMASKED_VENDOR_WEBGL, 37446=UNMASKED_RENDERER_WEBGL
                    if (pname === 37445 && isFallback(v)) return vendor;
                    if (pname === 37446 && isFallback(v)) return renderer;
                } catch (e) {}
                return v;
            }, 'getParameter');
            Object.defineProperty(proto, 'getParameter', {
                value: wrapped, configurable: true, writable: true, enumerable: true
            });
        };
        patch(window.WebGLRenderingContext && WebGLRenderingContext.prototype);
        patch(window.WebGL2RenderingContext && WebGL2RenderingContext.prototype);
    } catch (e) {}

    // ② plugins / mimeTypes 空值回填:真实桌面 Chrome 恒 ≥5 个 PDF 查看器插件,
    //    为 0 是典型无头/受限环境特征(见 fingerprint.rs::no_plugins)。
    //
    //    【第 175 轮重写 · 原型自洽】旧实现用**对象字面量**造 PluginArray,实测十项
    //    判据全被识破:toString 报 `[object Object]`(原生 `[object PluginArray]`),
    //    `instanceof PluginArray` 为 false、constructor 为 Object、
    //    `length/item/namedItem/refresh` 成了实例 own(原生全在原型上)、
    //    无 Symbol.iterator、元素不是 Plugin、元素多出 name/filename 等 own 键。
    //    **形状错误的回填比不回填更危险**:缺插件只是弱信号(受限环境本就没有),
    //    而「形状错误的 PluginArray」是强信号(正常浏览器不会造错形状)。
    //    另修一个逻辑错:mimeTypes 必须**按 type 去重**为 2 条,旧实现按插件各 push
    //    一份会变成 10 条。
    //    新做法 = 三层私有原型链,与原生逐字段同形:
    //        实例(own 只有索引+具名键) → 私有中间原型(方法/getter/iterator/tag) → 真原型
    try {
        if ((!navigator.plugins || navigator.plugins.length === 0)
            && typeof PluginArray === 'function' && typeof Plugin === 'function'
            && typeof MimeTypeArray === 'function' && typeof MimeType === 'function') {
            const MIMES = [
                { type: 'application/pdf', suffixes: 'pdf', description: 'Portable Document Format' },
                { type: 'text/pdf', suffixes: 'pdf', description: 'Portable Document Format' }
            ];
            const NAMES = ['PDF Viewer', 'Chrome PDF Viewer', 'Chromium PDF Viewer',
                           'Microsoft Edge PDF Viewer', 'WebKit built-in PDF'];
            // 每个数组体各有自己的私有中间原型,故 namedItem 的「按具名键回指实例」
            // 用**每次调用独立的闭包变量**(不能共用一个 ctx.self:plugins 与
            // mimeTypes 两个数组体先后构造会互相覆盖,实测 namedItem 恒查不到)。
            const ctx = { plugins: [], mimeByType: new Map(), mimeOrder: [] };
            const mimeOrder = ctx.mimeOrder;
            const descriptorFor = {
                getter: (name, fn) => ({ get: reg(fn, 'get ' + name), enumerable: true, configurable: true }),
                method: (name, fn) => ({ value: reg(fn, name), writable: true, enumerable: true, configurable: true }),
                // 第 178 轮(真机差分实测):中间原型必须自带 own `constructor`。
                // 原生 `PluginArray.prototype` 的自有属性是
                // `["length","item","namedItem","refresh","constructor"]`,而
                // `Object.create(PluginArray.prototype)` 只**继承** constructor,
                // `Object.getOwnPropertyNames(中间原型)` 会少这一项 —— 一行即可看出
                // 「这不是原生原型」。形态判据见 docs/MCP_Web_Use/32 §七.2。
                ctor: (proto) => ({ value: proto.constructor, writable: true, enumerable: false, configurable: true })
            };
            // 数组体:实例 own 只有索引 + 具名键;length 与方法全在中间原型上。
            //
            // 第 177 轮真机实测修正:`refresh` **只属 PluginArray**。原生
            // `MimeTypeArray.prototype` 的自有属性实测恒为
            // `length / item / namedItem / constructor` —— **没有 refresh**
            // (`PluginArray.prototype` 才有,实测同样四个 + refresh)。旧实现对两个
            // 数组体一律挂 refresh,于是
            // `Object.getOwnPropertyNames(Object.getPrototypeOf(navigator.mimeTypes))`
            // 会多出一个原生没有的键 —— 形状错误的回填比不回填更危险。
            const makeArrayLike = (items, RealProto, tag, nameOf, withRefresh) => {
                const proto = Object.create(RealProto);
                let selfRef = null;   // 本数组体实例的引用(namedItem 按具名键回指它)
                Object.defineProperties(proto, {
                    length: descriptorFor.getter('length', function () { return items.length; }),
                    item: descriptorFor.method('item', function item(i) {
                        return (i >= 0 && i < items.length) ? items[i] : null;
                    }),
                    namedItem: descriptorFor.method('namedItem', function namedItem(n) {
                        return (typeof n === 'string' && selfRef && selfRef[n]) ? selfRef[n] : null;
                    })
                });
                if (withRefresh) {
                    Object.defineProperty(proto, 'refresh',
                        descriptorFor.method('refresh', function refresh() {}));
                }
                // 符号键的**定义序**决定 `getOwnPropertySymbols` 的返回序。原生
                // `PluginArray.prototype` 实测为 `[Symbol.toStringTag, Symbol.iterator]`,
                // 故 toStringTag 必须先定义(真机差分会把顺序差异也照出来)。
                Object.defineProperty(proto, Symbol.toStringTag,
                    { value: tag, enumerable: false, configurable: true });
                Object.defineProperty(proto, Symbol.iterator, {
                    // 第 176 轮:原生 Iterator 的迭代器本身也是 iterable 的
                    // (`iterator[Symbol.iterator]()` 返回自身,ECMAScript 规范要求),
                    // 且其 next 必须可用 —— 由共用的 __mkIterator 统一产出。
                    // 第 177 轮:next 与自反方法此前是**未登记的箭头/普通函数**,
                    // 页面 `String(it.next)` / `String(it[Symbol.iterator])` 一行即可
                    // 读到我们的源码 —— 原生化只登记了外层生成器,漏了迭代器本身。
                    value: reg(function () { return __mkIterator(items); }, '[Symbol.iterator]'),
                    writable: true, enumerable: false, configurable: true
                });
                // 第 178 轮:own `constructor` 收尾挂 —— 字符串键序与原生一致
                // (`length/item/namedItem/[refresh/]constructor`)。
                Object.defineProperty(proto, 'constructor', descriptorFor.ctor(RealProto));
                const o = Object.create(proto);
                items.forEach((it, i) => {
                    Object.defineProperty(o, String(i),
                        { value: it, enumerable: true, writable: false, configurable: true });
                    const key = nameOf(it);
                    if (key != null) {
                        Object.defineProperty(o, key,
                            { value: it, enumerable: true, writable: false, configurable: true });
                    }
                });
                selfRef = o;
                return o;
            };
            // MimeType 元素:四个字段全挂中间原型 getter,实例 own 为空(原生即如此)。
            const makeMime = (spec, plugin) => {
                const proto = Object.create(MimeType.prototype);
                Object.defineProperties(proto, {
                    type: descriptorFor.getter('type', function () { return spec.type; }),
                    suffixes: descriptorFor.getter('suffixes', function () { return spec.suffixes; }),
                    description: descriptorFor.getter('description', function () { return spec.description; }),
                    enabledPlugin: descriptorFor.getter('enabledPlugin', function () { return plugin; })
                });
                // 第 178 轮:own `constructor`(真机差分:原生 `MimeType.prototype` 的
                // own 键为 `["type","suffixes","description","enabledPlugin","constructor"]`)
                Object.defineProperty(proto, 'constructor',
                    descriptorFor.ctor(MimeType.prototype));
                return Object.create(proto);
            };
            NAMES.forEach((name) => {
                const proto = Object.create(Plugin.prototype);
                Object.defineProperties(proto, {
                    name: descriptorFor.getter('name', function () { return name; }),
                    filename: descriptorFor.getter('filename', function () { return 'internal-pdf-viewer'; }),
                    description: descriptorFor.getter('description', function () { return 'Portable Document Format'; })
                });
                const plugin = Object.create(proto);
                const own = MIMES.map((spec, i) => {
                    let mime = ctx.mimeByType.get(spec.type);
                    if (!mime) {
                        mime = makeMime(spec, plugin);
                        ctx.mimeByType.set(spec.type, mime);
                        mimeOrder.push(mime);
                    }
                    return mime;
                });
                Object.defineProperties(proto, {
                    length: descriptorFor.getter('length', function () { return own.length; }),
                    item: descriptorFor.method('item', function item(i) {
                        return (i >= 0 && i < own.length) ? own[i] : null;
                    }),
                    namedItem: descriptorFor.method('namedItem', function namedItem(n) {
                        return (typeof n === 'string' && plugin[n]) ? plugin[n] : null;
                    })
                });
                // 实例 own = mime 索引 + mime type 具名键(与原生 Plugin 实例同形)
                own.forEach((mime, i) => {
                    Object.defineProperty(plugin, String(i),
                        { value: mime, enumerable: true, writable: false, configurable: true });
                    Object.defineProperty(plugin, mime.type,
                        { value: mime, enumerable: true, writable: false, configurable: true });
                });
                // 第 178 轮:own `constructor`(真机差分:原生 `Plugin.prototype` 的
                // own 键为 `["name","filename","description","length","item","namedItem","constructor"]`)
                Object.defineProperty(proto, 'constructor',
                    descriptorFor.ctor(Plugin.prototype));
                ctx.plugins.push(plugin);
            });
            const defNav = (key, val) => {
                try {
                    Object.defineProperty(Navigator.prototype, key, {
                        get: reg(function () { return val; }, 'get ' + key),
                        configurable: true, enumerable: true
                    });
                } catch (e) {}
            };
            defNav('plugins', makeArrayLike(ctx.plugins, PluginArray.prototype,
                'PluginArray', (p) => p.name, true));
            defNav('mimeTypes', makeArrayLike(mimeOrder, MimeTypeArray.prototype,
                'MimeTypeArray', (m) => m.type, false));
            // 第 178 轮:`navigator.pdfViewerEnabled` 必须与回填的 PDF 插件自洽。
            // 回填分支造出 5 个「PDF Viewer」却让 pdfViewerEnabled 停在该环境的原生值
            // (无插件环境通常为 false),就是「5 个 PDF 插件 + PDF 查看器不可用」的
            // 自造矛盾 —— 与第 175 轮「形状错误的回填比不回填更危险」同源。
            // 仅在页面读到 false 时对齐(真实值 true 原样放行),经 reg() 登记身份。
            try {
                if (navigator.pdfViewerEnabled === false) {
                    defNav('pdfViewerEnabled', true);
                }
            } catch (e) {}
        }
    } catch (e) {}

    // ③ navigator.languages 空值回填:取页面自报的 navigator.language 及其基语言。
    //    第 178 轮**方向修正**(真机实测驱动):`NavigatorLanguage.languages` 的
    //    WebIDL 类型是 **`FrozenArray<DOMString>`**,原生**就是冻结的** ——
    //    裸环境(`stealth=off`)四档实测 `Object.isFrozen(navigator.languages) === true`。
    //    第 176 轮据「sequence 返回非 frozen 数组」把 freeze 去掉了,方向恰好相反:
    //    那样回填出来的是**异类**(原生冻结、我们可写),严格模式 `push` 不抛反而成了判据。
    //    故此处恢复 `Object.freeze`,与原生同形。
    try {
        if (!navigator.languages || navigator.languages.length === 0) {
            const lang = navigator.language || 'en-US';
            const base = String(lang).split('-')[0];
            const list = base && base !== lang ? [lang, base] : [lang];
            try {
                Object.defineProperty(Navigator.prototype, 'languages', {
                    get: reg(function () { return Object.freeze(list); }, 'get languages'),
                    configurable: true, enumerable: true
                });
            } catch (e) {}
        }
    } catch (e) {}

    // ④ window.chrome 形态还原。**第 175 轮实测修正**:真机裸环境(全新 profile、
    //    无扩展)读到的是 `{app, csi, loadTimes}` —— **没有 `chrome.runtime`**。
    //    它只在加载了扩展的页面暴露。旧实现在缺失时造一个空 `chrome.runtime`,
    //    等于凭空造出「本机没有的身份信息」(违反工程铁律),且更糟的是**可被一行
    //    识破**:真实扩展上下文的 `chrome.runtime` 必有 `id`/`connect`/`sendMessage`,
    //    空 runtime 比没有 runtime 更可疑。故本分支只补真机实测存在的 csi/loadTimes/app。
    //    **第 181 轮**:从「缺失才建」扩到「**在场但三件套不全也补**」—— 真机
    //    `Object.keys(chrome)` 恒为 `[app,csi,loadTimes]`,缺任一即非真 Chrome 形态
    //    (无头/被降级环境),与 WebGL UNMASKED 回填同性质:还原被抹掉的**形态**,
    //    不伪造身份。补的项一律 `reg()` 登记,跨档升级由 `__adoptDeep` 接住。
    //    在场时**只补缺失项**,绝不整体替换 `window.chrome`(会毁掉扩展注入的
    //    `chrome.runtime` 等真实扩展上下文)。
    try {
        const ch = window.chrome;
        const needApp = !ch || !ch.app;
        const needCsi = !ch || typeof ch.csi !== 'function';
        const needLoad = !ch || typeof ch.loadTimes !== 'function';
        if (needApp || needCsi || needLoad) {
            const mkApp = () => ({
                isInstalled: false,
                InstallState: { DISABLED: 'disabled', INSTALLED: 'installed', NOT_INSTALLED: 'not_installed' },
                RunningState: { CANNOT_RUN: 'cannot_run', READY_TO_RUN: 'ready_to_run', RUNNING: 'running' }
            });
            const mkCsi = () => reg(function csi() {
                const t = (typeof performance !== 'undefined' && performance.timing) || {};
                return { onloadT: t.domContentLoadedEventEnd || 0, startE: t.navigationStart || 0, pageT: 0, tran: 15 };
            }, 'csi');
            const mkLoadTimes = () => reg(function loadTimes() {
                const t = (typeof performance !== 'undefined' && performance.timing) || {};
                return {
                    commitLoadTime: t.domContentLoadedEventEnd || 0,
                    finishDocumentLoadTime: t.domContentLoadedEventEnd || 0,
                    finishLoadTime: t.loadEventEnd || 0,
                    navigationStart: t.navigationStart || 0,
                    requestTime: t.navigationStart || 0
                };
            }, 'loadTimes');
            const target = (ch && typeof ch === 'object') ? ch : {};
            const put = (key, val) => {
                try {
                    Object.defineProperty(target, key, {
                        value: val, configurable: true, writable: true, enumerable: true
                    });
                } catch (e) { try { target[key] = val; } catch (e2) {} }
            };
            if (needApp && !target.app) put('app', mkApp());
            if (needCsi) put('csi', mkCsi());
            if (needLoad) put('loadTimes', mkLoadTimes());
            // 真机没有 chrome 时才整体创建(近乎不可达,防御保留)
            if (!ch) {
                Object.defineProperty(window, 'chrome', {
                    value: target, configurable: true, enumerable: true, writable: true
                });
            }
        }
    } catch (e) {}

    // ⑤ Notification.permission 与 permissions.query 一致性:两者矛盾是经典无头检测
    //    组合。第 176 轮:覆盖全部三种 Notification.permission 值,用**精确映射**判定
    //    不一致 —— Notification.permission 取值 default/denied/granted,而 query 返回
    //    的 state 取值 prompt/denied/granted,**default 与 prompt 语义一致**(都表示
    //    「未决定」)。简单用 !== 比较会把 default vs prompt 误判为不一致并改写,反而
    //    制造不一致。映射:granted↔granted / denied↔denied / default↔prompt。
    //    只在**偏离映射**时就地改写返回对象的 state(以 Notification.permission 为准,
    //    保留原生 PermissionStatus 身份,零伪造),其余一切情况零动作。
    try {
        if (window.Permissions && Permissions.prototype.query
            && typeof Notification !== 'undefined'
            && Notification.permission !== 'unsupported') {
            // default 与 prompt 语义一致(都表示「未决定」),granted/denied 原样。
            const want = Notification.permission === 'default' ? 'prompt'
                                                        : Notification.permission;
            const origQuery = Permissions.prototype.query;
            const wrappedQuery = reg(function query(p) {
                const r = origQuery.apply(this, arguments);
                return Promise.resolve(r).then(function (st) {
                    try {
                        if (p && p.name === 'notifications' && st && st.state !== want) {
                            // 第 177 轮:此前是 `get: () => want` 的**未登记箭头函数**,
                            // 页面 `Object.getOwnPropertyDescriptor(st,'state').get.toString()`
                            // 一行即可读到我们的源码(且原生化只登记外层 query)。
                            Object.defineProperty(st, 'state', {
                                get: reg(function state() { return want; }, 'get state'),
                                configurable: true
                            });
                        }
                    } catch (e) {}
                    return st;
                });
            }, 'query');
            Object.defineProperty(Permissions.prototype, 'query', {
                value: wrappedQuery, configurable: true, writable: true, enumerable: true
            });
        }
    } catch (e) {}

    // ⑥ 显示器拓扑真值(第 184 轮):`screen.isExtended`(Window Management API 的
    //    多显示器标记)是显示器派生读数 —— 真机双档基线(38 号文 §二):有头档跟随
    //    OS 拓扑(本机双显示器读 true),无头档的虚拟屏幕恒 false。无 CDP 原生通道
    //    (setEmulatedMedia 不覆盖该 API),走 JS 回填:先探测原生属性挂在哪个槽位
    //    (screen 实例 own / Screen.prototype)与哪种形态(数据属性/存取器),**同位
    //    同形**回填 —— 布尔值无「形状错误的回填」风险。真值来自 OS
    //    (display_topology,进程级缓存,Rust 侧替换 %MULTI_DISPLAY% 字面量);
    //    只在「OS=多显示器 && 页面读到 false」时动作 —— 单显示器机器零动作
    //    (false 即真值,不伪造拓扑)、有头档原生即真值零动作。真值标记挂 ns 对象
    //    非枚举键 MD,供体检层对账(fingerprint::screen_is_extended 的 md_marker)。
    try {
        if (%MULTI_DISPLAY% === true) {
            try {
                if (__nsObj) Object.defineProperty(__nsObj, 'MD', { value: 1, configurable: true, enumerable: false });
            } catch (e) {}
            try {
                if (window.Screen && screen.isExtended === false) {
                    const own = Object.getOwnPropertyDescriptor(screen, 'isExtended');
                    const onProto = !own && Object.getOwnPropertyDescriptor(Screen.prototype, 'isExtended');
                    if (own && typeof own.value === 'boolean') {
                        // 数据属性形态:镜像 {value, writable, enumerable, configurable}
                        Object.defineProperty(screen, 'isExtended', {
                            value: true,
                            writable: !!own.writable,
                            enumerable: !!own.enumerable,
                            configurable: !!own.configurable
                        });
                    } else if (own || onProto) {
                        // 存取器形态(own 在 screen / 在 Screen.prototype):reg() 登记进原生化表
                        Object.defineProperty(own ? screen : Screen.prototype, 'isExtended', {
                            get: reg(function isExtended() { return true; }, 'get isExtended'),
                            set: undefined, enumerable: true, configurable: true
                        });
                    }
                    // 属性原生缺失的浏览器不凭空补 —— 补了才是签名
                }
            } catch (e) {}
        }
    } catch (e) {}"#;

/// 原生化块（**侵入式**，第 174 轮）：`Function.prototype.toString` 对登记表内的函数
/// 报 `[native code]`。
///
/// 边界：这是「让我们的改写不可反查」，不改变任何函数行为。自反闭合 ——
/// `toString` 包装器自身也在登记表内，故
/// `Function.prototype.toString.call(Function.prototype.toString)` 同样报 native，
/// 不会因「toString 是自己的源码」而暴露。描述符与原生同形
/// （`writable: true, enumerable: false, configurable: true` —— 此处**必须**不可枚举）。
///
/// 第 177 轮:装原生化**之前**先 `__adoptAll()` 重新认领上一轮注入遗留的改写函数 ——
/// 跨档升级(保真→侵入)时本轮的 `__fd` 是全新表,看不见上一轮登记过的函数,
/// 不认领它们就会退回可读的源码形态(而页面照样能通过 `Permissions.prototype.query`
/// 这类稳定入口拿到引用)。
const HARDENED_JS: &str = r#"
    try { __adoptAll(); } catch (e) {}
    try {
        const origToString = Function.prototype.toString;
        const wrappedToString = function toString() {
            try { if (__fd.has(this)) return __fd.get(this); } catch (e) {}
            // 第 184 轮:跨 realm 反查路由。`runForEveryFrame` 之后每个 frame 都有
            // 自己的登记表(帧局部闭包 —— 共享表会让攻击者拿到 symbol 后直接
            // `fd.has(fn)` 探测任意函数是否被改过)。兄弟 realm 的 toString 反查本
            // realm 函数时本帧必然 miss,原样回退 origToString 即把源码交给对端。
            // miss 时路由到 `this` 自身原型链上的 toString —— 那是它**本 realm**
            // 的原生化包装器,持有把它登记进去的那张表;到家后 home === 本包装器
            // 跳出,终止性由「每一跳都严格走向 this 的家」保证(最多两跳)。
            // typeof 守卫:非函数 this 的原生行为是 TypeError,路由到
            // Object.prototype.toString 会返回 "[object …]" 字符串,属行为漂移。
            try {
                if (typeof this === 'function') {
                    const home = Object.getPrototypeOf(this).toString;
                    if (typeof home === 'function' && home !== wrappedToString) return home.call(this);
                }
            } catch (e) {}
            return origToString.call(this);
        };
        try { __fd.set(wrappedToString, 'function toString() { [native code] }'); } catch (e) {}
        Object.defineProperty(Function.prototype, 'toString', {
            value: wrappedToString, configurable: true, writable: true, enumerable: false
        });
    } catch (e) {}"#;

/// 按档组合隐身脚本（`None` = 该档不注入任何脚本）。
///
/// - `Off`：不注入（裸 CDP 环境，排障对照组）；
/// - `Fidelity`（非侵入式）：前置 + 核心 + 保真；
/// - `Hardened`（侵入式）：前置 + 核心 + 保真 + 原生化。
///
/// 保真块另受 legacy 杀开关 `LAEW_WEB_FINGERPRINT=off` 约束（只关回填与体检，
/// 核心 `webdriver` 覆写照旧）。
pub fn stealth_js_for(mode: crate::agent::stealth::StealthMode) -> Option<String> {
    use crate::agent::stealth::{fingerprint_kill_switch, StealthMode};
    if mode == StealthMode::Off {
        return None;
    }
    let mut want: u32 = BLOCK_CORE;
    if !fingerprint_kill_switch() {
        want |= BLOCK_FIDELITY;
    }
    if mode == StealthMode::Hardened {
        want |= BLOCK_HARDENED;
    }
    // 幂等闸按**块位掩码**(第 177 轮,原为单一 rev 整数):同一文档重复注入必须零动作。
    // `set_stealth` 会在每次 open 与运行时切档时对存活页面重挂脚本,整脚本重跑会把包装器
    // 叠加(第二份 `__fd` 看不见第一份的函数 -> 原生化被判为非 native,且 getParameter
    // 被套两层)。位掩码让**跨档升级只跑缺的那几块**:保真->侵入时核心/保真两块的位已置位,
    // 只有原生化块执行 —— 既不叠加包装,又能借 `__adoptAll()` 把上一轮的改写函数认领进新表
    // (登记表是闭包常量,跨重注入本就是新表)。标记沿用同一个不可枚举 window 键(不新增
    // 全局键),值仍是整数。
    // 模板里一律用 `%TOKEN%` 占位（不是合法 JS 标识符，天然零碰撞），不用 `format!` ——
    // 模板正文含大量 JS 花括号，`format!` 的 `{{}}` 转义会把 JS 本身搅乱。
    // 只有 `want` 里置位的块才会被拼进脚本文本：既让脚本体积随档位最小化，也让
    // 「保真档脚本文本里根本没有原生化代码」这件事在源码层可断言（运行时闸只是
    // 兜底，不能替代「压根不装」）。
    // 幂等闸标记挂在**符号命名空间对象**上（第 179 轮）：`window` 上不再有
    // `<ns>StealthRev` 字符串键。`Symbol.for` 是全局符号注册表，同一页面内
    // 各注入脚本拿到同一个 symbol；`getOwnPropertySymbols(window)` 只见 1 个。
    let head = r#"(() => {
  try {
    var __nsObj = window[Symbol.for('%NS%')] || (window[Symbol.for('%NS%')] = {});
    var __cur = 0;
    try { __cur = __nsObj.StealthRev || 0; } catch (e) {}
    var __need = %WANT% & ~__cur;
    if (__need === 0) return;
    Object.defineProperty(__nsObj, 'StealthRev', { value: %WANT%, configurable: true, enumerable: false });
  } catch (e) {}
"#;
    let mut out = String::with_capacity(head.len() + 4096);
    out.push_str(&head.replace("%WANT%", &want.to_string()));
    out.push('\n');
    out.push_str(PRELUDE_JS);
    for (bit, block) in [
        (BLOCK_CORE, CORE_JS),
        (BLOCK_FIDELITY, FIDELITY_JS),
        (BLOCK_HARDENED, HARDENED_JS),
    ] {
        if want & bit == 0 {
            continue;
        }
        out.push_str("\n  if (__need & ");
        out.push_str(&bit.to_string());
        out.push_str(") {\n");
        out.push_str(block);
        out.push_str("\n  }\n");
    }
    out.push_str("})()");
    // 第 184 轮:OS 显示器拓扑真值字面量(true/false/null)替换进 FIDELITY_JS ⑥;
    // null = OS 读取失败,fallback 分支整体 no-op(fail-open 不处置)。放在 render_ns
    // 之前 —— %MULTI_DISPLAY% 不在 render_ns 的改写词表内,先后无影响,但与 %WANT%
    // 同一处集中替换便于审计。
    let out = out.replace("%MULTI_DISPLAY%", crate::agent::display_topology::multi_display_literal());
    Some(render_ns(&out))
}

/// 给页面安装隐身覆写（`Page.addScriptToEvaluateOnNewDocument`：导航/派生新文档
/// 自动重挂 + 当前文档立即执行）。connect 模式（接管用户浏览器）**不注入** ——
/// 真实浏览器本无该指纹，且 connect 契约是零注入。
///
/// 第 184 轮：改走 `runForEveryFrame: true`（`cdp_ext` 补发的字段，chromiumoxide
/// 0.9 类型缺失）—— 脚本进入**所有子 frame**。此前只进顶层：站点自建同源 iframe
/// 拿原生 `Function.prototype.toString` 反查顶层被改写函数即读到我们的源码，
/// 侵入式档「改过不可反查」被完整击穿（真机实证见 `docs/MCP_Web_Use/38` §三）。
/// 老 Chrome 拒绝该字段时回退旧参数重发一次（只丢子帧覆盖，不丢顶层注入）。
pub async fn inject_stealth(page: &chromiumoxide::Page, mode: crate::agent::stealth::StealthMode) {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
    let Some(source) = stealth_js_for(mode) else {
        return;
    };
    let every = crate::agent::cdp_ext::AddScriptToEvaluateOnNewDocumentEveryFrame::new(source.clone(), true);
    if page.execute(every).await.is_ok() {
        return;
    }
    let _ = page
        .execute(AddScriptToEvaluateOnNewDocumentParams {
            source,
            world_name: None,
            include_command_line_api: None,
            run_immediately: Some(true),
        })
        .await;
}

/// UA 去自动化令牌的处置结果（第 178 轮）：调用方据此决定是否需要**落地后补全**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UaSweep {
    /// 页面自报值本就干净（有头档常规情形）—— 零动作。
    Skipped,
    /// 只清理了 UA 字符串（拿不到 Client Hints —— 例如探测发生在 `about:blank`，
    /// 它不是安全上下文，`navigator.userAgentData` 不存在）。
    ///
    /// 第 178 轮真机实测：`setUserAgentOverride` **不带** `userAgentMetadata` 时，
    /// 页面侧的 `navigator.userAgentData.brands` 会被清空 —— 「UA 说 Chrome、
    /// Client Hints 说空」是比原状更显眼的新破绽。故调用方必须在真实文档落地
    /// （安全上下文）后走 [`restore_native_ua`] + 二次 [`apply_ua_destealth`] 补全。
    StringOnly,
    /// UA 字符串与 Client Hints 品牌表一起清理完毕。
    Full,
}

/// 无探针的 UA 字符串覆盖(第 180 轮):给**尚无可用执行上下文**的派生页在出生
/// 时刻立即去令牌用 —— `Emulation.setUserAgentOverride` 只传字符串(等价
/// [`UaSweep::StringOnly`] 语义:页面侧品牌表会被清空,须由落地后补全收口)。
///
/// 为什么不探测:派生 target 首个文档提交前 `Runtime.evaluate` 可能阻塞在
/// provisional 导航上(实测 400ms+),探针式清理赶不上 document-start;而覆盖值
/// 来自浏览器级 `getVersion` 的 UA 字符串(真值,只滤令牌零伪造)。
pub async fn apply_ua_string_override(page: &chromiumoxide::Page, user_agent: &str) -> bool {
    let params =
        match chromiumoxide::cdp::browser_protocol::emulation::SetUserAgentOverrideParams::builder()
            .user_agent(user_agent.to_string())
            .build()
    {
        Ok(p) => p,
        Err(_) => return false,
    };
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), page.execute(params)).await,
        Ok(Ok(_))
    )
}

/// 移除 UA 覆盖、恢复浏览器原生身份（第 178 轮）。
///
/// 用途：`about:blank` 上只清得出 UA 字符串（非安全上下文读不到 Client Hints），
/// 而带上 `userAgentMetadata` 的**完整**覆盖需要品牌表真值。落地到真实文档后，
/// 先撤掉那次「半吊子」覆盖，让页面重新报出**原生** UA 与品牌表，再探一次即可
/// 拿到完整真值并下发完整覆盖。CDP 约定：`userAgent` 传空串即移除覆盖。
pub async fn restore_native_ua(page: &chromiumoxide::Page) -> bool {
    let params =
        match chromiumoxide::cdp::browser_protocol::emulation::SetUserAgentOverrideParams::builder()
            .user_agent(String::new())
            .build()
    {
        Ok(p) => p,
        Err(_) => return false,
    };
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), page.execute(params)).await,
        Ok(Ok(_))
    )
}
/// 无头档 UA 清理（第 174 轮）：`HeadlessChrome/155…` 会出现在 **UA 字符串 + 网络
/// 请求头 + `Sec-CH-UA` 品牌列表**三处，必须一起改 —— 只改 JS 侧的
/// `navigator.userAgent` 骗得过页面、骗不过服务器，且"UA 说 Chrome、Client Hints
/// 说 Headless"是更显眼的新破绽。故走 CDP `Emulation.setUserAgentOverride`
/// （唯一能同时改到网络头的通道）。
///
/// 取值全部来自页面自身读数（[`crate::agent::stealth::UA_PROBE_JS`]），只把品牌列表
/// 里的 `Headless*` 项滤掉 —— **零伪造**。
///
/// 第 178 轮：返回值从 `bool` 细化为 [`UaSweep`]，因为探测**发生在哪**决定了能拿到
/// 什么 —— `about:blank`（第 178 轮起的建页起点）不是安全上下文，
/// `navigator.userAgentData` 不存在 ⇒ 只能清 UA 字符串（`StringOnly`），
/// 调用方必须在真实文档落地后补全。
pub async fn apply_ua_destealth(
    page: &chromiumoxide::Page,
    mode: crate::agent::stealth::StealthMode,
) -> UaSweep {
    use crate::agent::stealth;
    if mode == stealth::StealthMode::Off {
        return UaSweep::Skipped;
    }
    let params = match chromiumoxide::cdp::js_protocol::runtime::EvaluateParams::builder()
        .expression(stealth::UA_PROBE_JS)
        .return_by_value(true)
        .await_promise(true)
        .build()
    {
        Ok(p) => p,
        Err(_) => return UaSweep::Skipped,
    };
    let raw = match tokio::time::timeout(std::time::Duration::from_secs(5), page.evaluate(params)).await
    {
        Ok(Ok(v)) => v,
        _ => return UaSweep::Skipped,
    };
    let Some(text) = raw.value().and_then(|v| v.as_str()) else {
        return UaSweep::Skipped;
    };
    let Ok(probe) = serde_json::from_str::<serde_json::Value>(text) else {
        return UaSweep::Skipped;
    };
    // 无令牌(有头档常规情形)时零动作 —— 这是预期路径,不告警。
    let Some(plan) = stealth::ua_plan(&probe) else {
        return UaSweep::Skipped;
    };
    let mut builder =
        chromiumoxide::cdp::browser_protocol::emulation::SetUserAgentOverrideParams::builder()
            .user_agent(plan.user_agent.clone());
    // 品牌列表被滤空时只改 UA 字符串，不下发一个比真值更可疑的空品牌表。
    let mut sweep = UaSweep::StringOnly;
    if plan.has_metadata() {
        use chromiumoxide::cdp::browser_protocol::emulation::{
            UserAgentBrandVersion as Brand, UserAgentMetadata as Meta,
        };
        let brands = |list: &[(String, String)]| {
            list.iter()
                .map(|(b, v)| Brand::new(b.clone(), v.clone()))
                .collect::<Vec<_>>()
        };
        // 直接构造结构体:chromiumoxide 0.9 的 builder 把 `full_version_list` 生成了
        // 「取单个品牌」的签名(生成器缺陷),用它就表达不出「整张版本表」。
        // 一并下发 Full-Version-List 是必要的:省略会让 `Sec-CH-UA-Full-Version-List`
        // 退化成主版本号(真实 Chrome 报 `155.0.8059.40`),等于自己制造不一致。
        let meta = Meta {
            brands: Some(brands(&plan.brands)),
            full_version_list: if plan.full_version_list.is_empty() {
                None
            } else {
                Some(brands(&plan.full_version_list))
            },
            platform: plan.platform.clone(),
            platform_version: plan.platform_version.clone(),
            architecture: plan.architecture.clone(),
            model: plan.model.clone(),
            mobile: plan.mobile,
            bitness: if plan.bitness.is_empty() {
                None
            } else {
                Some(plan.bitness.clone())
            },
            wow64: None,
            form_factors: None,
        };
        builder = builder.user_agent_metadata(meta);
        sweep = UaSweep::Full;
    }
    match builder.build() {
        Ok(p) => match page.execute(p).await {
            Ok(_) => sweep,
            Err(e) => {
                // 真失败(检测到令牌但覆盖下发不出去)才告警:此时该文档仍带自动化
                // 令牌,是"该修没修上",必须留痕。fail-open 不阻断导航。
                tracing::warn!("UA 去自动化令牌覆盖下发失败(本页 UA 仍可能带 Headless 令牌): {e}");
                UaSweep::Skipped
            }
        },
        Err(e) => {
            tracing::warn!("UA 去自动化令牌覆盖参数构造失败: {e}");
            UaSweep::Skipped
        }
    }
}

/// 隐身自检（L2）：launch 后首个页面探测结果解析。返回违例描述（None = 通过）。
/// 探测表达式返回 `{wd, brand, leaked}`：`wd` = `navigator.webdriver===true`
/// （覆写挂上后应恒 false）；`brand` = 页面可见文本残留「LAEW Agent」品牌签名
/// （注入 UI 挂 closed Shadow DOM 后应恒 false）；`leaked` = `Object.keys(window)`
/// 中仍含 `laew` 字样的键（固定名残留）。
pub fn stealth_probe_issue(probe: &serde_json::Value) -> Option<String> {
    if probe.get("wd").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        return Some("navigator.webdriver=true（覆写脚本未挂上,反检测失效）".to_string());
    }
    if probe.get("brand").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        return Some("页面文本残留品牌签名「LAEW Agent」（注入 UI 未隔离进 closed Shadow DOM）".to_string());
    }
    if let Some(leaked) = probe.get("leaked").and_then(serde_json::Value::as_array) {
        if !leaked.is_empty() {
            return Some(format!(
                "window 可枚举键残留 laew 字样: {}",
                leaked
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 命名空间格式稳定且含随机后缀() {
        let ns = inject_ns();
        assert!(ns.starts_with("__lzn"), "ns 前缀固定: {ns}");
        assert_eq!(ns.len(), "__lzn".len() + 8);
        assert_eq!(inject_ns(), ns, "进程内唯一");
    }

    #[test]
    fn 全局名与属性名拼接() {
        // 第 179 轮:窗口面 API 走 symbol 命名空间成员,不再有 `window.<ns>Foo` 字符串键
        assert_eq!(
            ns_member_expr("GuardSuspend"),
            format!("(window[Symbol.for('{}')]||{{}}).GuardSuspend", inject_ns())
        );
        assert_eq!(
            ns_object_expr(),
            format!("window[Symbol.for('{}')]", inject_ns())
        );
        assert!(ns_bootstrap_js().contains("Symbol.for"));
        // DOM 面换用独立 token,且不带 `__lzn` 前缀
        assert_eq!(agent_attr(), format!("data-{}agent", dom_ns()));
        assert_eq!(
            agent_attr_selector(),
            format!("[data-{}agent]", dom_ns())
        );
        assert_ne!(dom_ns(), inject_ns(), "DOM 面与窗口面 token 必须独立");
        assert!(!dom_ns().starts_with("__lzn"), "DOM 面不得复用窗口前缀:{}", dom_ns());
        assert!(!agent_attr().contains("laew"), "属性名不得含固定签名 laew");
        assert!(!ns_member_expr("X").contains("laew"));
    }

    #[test]
    fn 隐身自检判定() {
        assert!(stealth_probe_issue(&serde_json::json!({"wd": true, "leaked": []})).is_some());
        assert!(
            stealth_probe_issue(&serde_json::json!({"wd": false, "leaked": ["__laewGuardRev"]}))
                .is_some()
        );
        assert!(stealth_probe_issue(&serde_json::json!({"wd": false, "leaked": []})).is_none());
        // 缺字段 fail-open（不告警，避免探测表达式故障误报）
        assert!(stealth_probe_issue(&serde_json::json!({})).is_none());
        // 第 169 轮:品牌文本泄漏通道(注入 UI 未 shadow 化时页面 innerText 可见)
        let brand = stealth_probe_issue(&serde_json::json!({"wd": false, "brand": true, "leaked": []}));
        assert!(brand.is_some(), "brand 泄漏应告警");
        assert!(brand.unwrap().contains("LAEW Agent"));
        assert!(
            stealth_probe_issue(&serde_json::json!({"wd": false, "brand": false, "leaked": []}))
                .is_none()
        );
    }

    #[test]
    fn 隐身脚本按档组合() {
        use crate::agent::stealth::StealthMode;
        // off:零注入(裸环境对照组)
        assert!(stealth_js_for(StealthMode::Off).is_none());
        let f = stealth_js_for(StealthMode::Fidelity).expect("保真档必须注入");
        let h = stealth_js_for(StealthMode::Hardened).expect("侵入式档必须注入");
        // 三档共同核心:webdriver 覆写 + 返回值 false(与真实 Chrome 语义一致)
        for s in [&f, &h] {
            assert!(s.contains("Navigator.prototype"));
            assert!(s.contains("'webdriver'"));
            assert!(s.contains("defineProperty"));
            assert!(s.contains("return false;"));
            // 第 172 轮:WebGL 保真回填 —— 只在读数是自动化兜底串时才回填
            assert!(s.contains("isFallback"));
            assert!(s.contains("37445") && s.contains("37446"));
            assert!(s.contains("ANGLE"));
            // 兜底判据必须同时覆盖 WebKit / SwiftShader / software 三种形态
            assert!(s.contains("webkit") && s.contains("swiftshader") && s.contains("software"));
            // 第 174 轮:低熵 VENDOR/RENDERER(7936/7937)必须保持原生 ——
            // 真实 Chrome 这两个常量恒报 `WebKit` / `WebKit WebGL`,改写它们
            // 反而制造不一致(注释里可以提编号,代码分支里不能有)。
            assert!(
                !s.contains("pname === 7936") && !s.contains("pname === 7937"),
                "低熵串改写会制造不一致"
            );
        }
        // 保真档不含原生化:toString 覆写只在侵入式档
        assert!(!f.contains("Function.prototype.toString"), "保真档不得改函数身份");
        assert!(h.contains("Function.prototype.toString"));
        // 描述符形态还原(第 174 轮):被改写属性必须 enumerable,否则 for…in 枚举原型链即暴露
        for s in [&f, &h] {
            assert!(
                s.contains("writable: true, enumerable: true"),
                "WebGL 覆写须还原 WebIDL 原生描述符形态"
            );
        }
    }

    #[test]
    fn 保真块覆盖五项环境缺口() {
        use crate::agent::stealth::StealthMode;
        let f = stealth_js_for(StealthMode::Fidelity).unwrap();
        // ② plugins / mimeTypes 空值回填
        assert!(f.contains("internal-pdf-viewer"));
        assert!(f.contains("Chrome PDF Viewer"));
        // ③ languages 空值回填
        assert!(f.contains("get languages"));
        // ④ window.chrome 缺失兜底(第 175 轮:只补真机实测存在的三件套)
        assert!(f.contains("'chrome'"));
        assert!(f.contains("chrome.app") || f.contains("InstallState"));
        assert!(f.contains("csi") && f.contains("loadTimes"));
        // ⑤ permissions 一致性(只在不一致时就地改写返回对象 state)
        assert!(f.contains("Permissions.prototype.query"));
        assert!(f.contains("notifications"));
        // 身份登记表:三档共用前置
        assert!(f.contains("new WeakMap()"));
        assert!(f.contains("reg("));
    }

    #[test]
    fn 原生化块自反闭合() {
        use crate::agent::stealth::StealthMode;
        let h = stealth_js_for(StealthMode::Hardened).unwrap();
        // 包装器自身登记 → toString.toString() 亦报 native
        assert!(h.contains("__fd.set(wrappedToString"));
        // 原生描述符形态:toString 必须不可枚举
        assert!(h.contains("writable: true, enumerable: false"));
        assert!(h.contains("[native code]"));
    }

    // ===================== 第 175 轮:对象形态自洽(真机实测驱动) =====================

    /// 第 175 轮真机实测(Chrome 155,`accept.js`):旧的对象字面量实现有十项判据
    /// 全被识破(tag/instanceof/ctor/proto 链/own 键构成/iterator/元素形态…),
    /// 而**形状错误的回填比不回填更危险** —— 缺插件只是弱信号(受限环境本就没有),
    /// 形状错误的 PluginArray 是强信号(正常浏览器不会造错形状)。
    /// 本测试锁死「必须走真原型链 + 方法挂中间原型」这两条根本判据。
    #[test]
    fn 保真块插件回填走真原型链而非对象字面量() {
        use crate::agent::stealth::StealthMode;
        let f = stealth_js_for(StealthMode::Fidelity).unwrap();
        // 真实原型为祖先。PluginArray/MimeTypeArray 的真原型经 makeArrayLike 的
        // `RealProto` 形参传入(两个数组体共用一段构造逻辑),Plugin/MimeType
        // 则直接以字面量作为中间原型的祖先。
        for proto in ["Plugin.prototype", "MimeType.prototype"] {
            assert!(
                f.contains(&format!("Object.create({proto})")),
                "回填对象必须以 {proto} 为原型祖先,否则 instanceof / toString 标签全线失配"
            );
        }
        assert!(
            f.contains("Object.create(RealProto)") && f.contains("PluginArray.prototype"),
            "数组体必须以传入的真原型为祖先(PluginArray.prototype / MimeTypeArray.prototype)"
        );
        // toStringTag 决定 `Object.prototype.toString.call()` 的读数
        assert!(f.contains("'PluginArray'") && f.contains("'MimeTypeArray'"));
        // Symbol.iterator:原生 PluginArray 可迭代,旧实现(普通对象)没有
        assert!(f.contains("Symbol.iterator"));
        // 缺型元素同样要形态正确(元素字段挂中间原型 getter,实例 own 为空)
        assert!(f.contains("enabledPlugin") && f.contains("Object.defineProperties(proto"));
    }

    /// mimeTypes 必须**按 type 去重为 2 条**:旧实现按「每个插件各 push 一份」
    /// 会变成 10 条,真机原生恒为 2(实测 `mimeTypes.length === 2`)。
    #[test]
    fn mime类型按type去重而非按插件数放大() {
        use crate::agent::stealth::StealthMode;
        let f = stealth_js_for(StealthMode::Fidelity).unwrap();
        assert!(
            f.contains("mimeByType"),
            "必须用 type→mime 的映射去重,直接按插件 push 会把 2 条放大成 10 条"
        );
        assert!(
            f.contains("ctx.mimeByType.get(spec.type)"),
            "已建过的 type 必须复用而非新建"
        );
    }

    /// 第 175 轮真机实测:全新 profile 的**真实 Chrome 没有 `chrome.runtime`**
    /// (`Object.keys(chrome)` 恒为 `[app, csi, loadTimes]`)。凭空补一个空
    /// `chrome.runtime` 既违反「不伪造本机没有的信息」铁律,又可被一行识破 ——
    /// 真实扩展上下文的 runtime 必有 id/connect/sendMessage,空 runtime 更可疑。
    #[test]
    fn 不伪造本机没有的chrome运行时字段() {
        use crate::agent::stealth::StealthMode;
        for m in [StealthMode::Fidelity, StealthMode::Hardened] {
            let s = stealth_js_for(m).unwrap();
            assert!(
                !s.contains("'runtime'"),
                "保真块不得凭空补 chrome.runtime(真机裸环境无此字段,空 runtime 比没有更可疑)"
            );
            // 真机实测存在的两件套仍应保留
            assert!(s.contains("csi") && s.contains("loadTimes"));
        }
    }

    /// `namedItem` 必须按**实例的具名键**查找:具名键挂在实例上而方法挂在共享的
    /// 中间原型上,故需要一个回指实例的闭包变量。早期实现查 `items[n]`
    /// (数组的具名键不在数组上)恒返回 null —— 实测已复现该失效。
    #[test]
    fn 具名查找回指实例而非闭包数组() {
        use crate::agent::stealth::StealthMode;
        let f = stealth_js_for(StealthMode::Fidelity).unwrap();
        assert!(
            f.contains("selfRef"),
            "namedItem 须经 selfRef 回指数组体实例(具名键在实例上,不在 items 数组上)"
        );
        assert!(
            !f.contains("items[n]"),
            "不得按数组下标做具名查找 —— 恒查不到,实测 namedItem 返回 null"
        );
    }

    // ===================== 第 176 轮:回填形态自洽 =====================

    /// 第 176 轮:Notification 一致性必须覆盖全部三种 permission 值,并用精确映射
    /// (default↔prompt)判定不一致 —— 简单 !== 会把语义一致的 default vs prompt
    /// 误判为不一致并改写,反而制造不一致。
    #[test]
    fn 通知权限一致性全量覆盖且用精确映射() {
        use crate::agent::stealth::StealthMode;
        for m in [StealthMode::Fidelity, StealthMode::Hardened] {
            let s = stealth_js_for(m).unwrap();
            // 触发条件不再只限 denied:覆盖 granted/denied/default 全部
            assert!(
                s.contains("Notification.permission !== 'unsupported'"),
                "必须覆盖全部三种 Notification.permission 值(不只 denied)"
            );
            // 精确映射:default → prompt(语义一致),granted/denied 原样
            assert!(
                s.contains("Notification.permission === 'default' ? 'prompt'"),
                "default 必须映射到 prompt(两者都表示未决定)"
            );
            assert!(s.contains("const want"), "必须用 want 变量承载映射结果");
            // 判据与改写都以 want 为准
            assert!(s.contains("st.state !== want"), "不一致判据必须用 want");
            assert!(s.contains("get: () => want"), "改写必须以 want 为准");
        }
    }

    /// 第 176 轮引入、**第 178 轮方向修正**:`navigator.languages` 必须**冻结**。
    /// 真机四档实测(含 `stealth=off` 裸环境)`Object.isFrozen(navigator.languages)
    /// === true` —— 规范里 `NavigatorLanguage.languages` 的类型是
    /// `FrozenArray<DOMString>`,冻结是原生形态;第 176 轮按「原生非 frozen」的
    /// 推断把 freeze 去掉了,造出的才是异类(原生冻结而我们可写)。
    #[test]
    fn 语言列表回填必须冻结() {
        use crate::agent::stealth::StealthMode;
        for m in [StealthMode::Fidelity, StealthMode::Hardened] {
            let s = stealth_js_for(m).unwrap();
            assert!(
                s.contains("Object.freeze(list)"),
                "回填必须冻结(原生 FrozenArray<DOMString>,真机四档实测 isFrozen=true):{s}"
            );
        }
    }

    /// 第 176/177 轮:plugins/mimeTypes 的 Symbol.iterator 返回的迭代器必须自反
    /// (iterator[Symbol.iterator] 是函数,ECMAScript 规范要求),且迭代器上的
    /// `next` 与自反方法**必须一并登记进身份表** —— 否则页面 `String(it.next)`
    /// 一行即可读到我们的源码(原生化此前只登记了外层生成器,漏了迭代器本身)。
    #[test]
    fn 插件迭代器自反且迭代器本身已登记() {
        use crate::agent::stealth::StealthMode;
        let f = stealth_js_for(StealthMode::Fidelity).unwrap();
        // 迭代器工厂抽到前置块的 __mkIterator:保真块与「重装迭代器」共用同一份,
        // 保证两侧产出的 next / 自反方法永远登记在**当前这张表**里。
        assert!(
            f.contains("const __mkIterator = (items) =>"),
            "迭代器工厂必须抽成共用实现(否则跨档重装的迭代器不在当前登记表里)"
        );
        assert!(f.contains("next: reg(function next()"), "next 必须登记身份(reg)");
        assert!(
            f.contains("iter[Symbol.iterator] = reg(function () { return iter; }"),
            "自反方法必须登记身份(reg)"
        );
        assert!(
            f.contains("value: reg(function () { return __mkIterator(items); }"),
            "保真块的迭代器入口必须走共用工厂并自行登记"
        );
        assert!(
            !f.contains("next: () => i < items.length"),
            "不得留下未登记的箭头函数形态"
        );
    }

    /// 跨轮认领的例外:迭代器的 next/自反方法是**每次调用现造的新对象**,原型上没有
    /// 引用可认领。解法是把迭代器工厂重装一遍(items 由原生 item() 反查回来),
    /// 且**只重装自造的中间原型** —— 在真实插件存在的环境里替换原生迭代器是改坏。
    #[test]
    fn 迭代器跨档靠重装而非认领() {
        use crate::agent::stealth::StealthMode;
        let h = stealth_js_for(StealthMode::Hardened).unwrap();
        assert!(h.contains("const __rehardenIterator = (arr, RealProto) =>"));
        // 真原型不碰
        assert!(
            h.contains("if (!proto || proto === RealProto) return;"),
            "必须跳过真原型(真实插件存在时替换原生迭代器会把好环境改坏)"
        );
        // items 由原生 item() 反查,不额外往原型上挂状态(那会新增可枚举痕迹)
        assert!(
            h.contains("const one = proto.item ? proto.item.call(arr, i) : null;"),
            "items 必须由原生 item() 反查,不得额外存状态"
        );
        // 三个数组体都要重装:plugins / mimeTypes / 每个插件的 mimeTypes
        assert!(h.contains("__rehardenIterator(arr, PA);"));
        assert!(h.contains("__rehardenIterator(navigator.mimeTypes, MA);"));
        assert!(h.contains("__rehardenIterator(mt, MA);"));
    }

    // ===================== 第 177 轮:注入时序 / 跨档自洽 / 子对象形态 =====================

    /// 幂等闸必须是**块位掩码**:同档重复注入零动作,跨档升级只补跑缺位对应的块。
    /// 若退化成单一 rev 整数,保真→侵入时整脚本重跑会把 `getParameter` 套两层
    /// (两层包装下外层看到内层已回填的读数即原样放行,行为对,但包装链越滚越深,
    /// 且第二份登记表看不见第一份的函数 → 原生化被误判为失败)。
    #[test]
    fn 幂等闸按块位掩码而非单一版本号() {
        use crate::agent::stealth::StealthMode;
        assert_eq!((BLOCK_CORE, BLOCK_FIDELITY, BLOCK_HARDENED), (1, 2, 4), "位必须互不相同且单");
        for m in [StealthMode::Fidelity, StealthMode::Hardened] {
            let s = stealth_js_for(m).unwrap();
            // 读旧值 → 算缺位 → 缺位为零直接 return,再写回新值(标记挂 symbol 命名空间)
            assert!(s.contains("Symbol.for("), "标记必须挂符号命名空间:{s}");
            assert!(s.contains("var __need ="), "必须按位算缺块:{s}");
            assert!(s.contains("if (__need === 0) return;"), "零缺块必须直接返回:{s}");
            // 每个装上的块都套一道同一位判的运行期闸
            assert!(s.contains("if (__need & 1) {"), "核心块须带位闸:{s}");
            if m == StealthMode::Hardened {
                assert!(s.contains("if (__need & 2) {"), "保真块须带位闸:{s}");
                assert!(s.contains("if (__need & 4) {"), "原生化块须带位闸:{s}");
            } else {
                assert!(
                    !s.contains("if (__need & 4) {"),
                    "保真档不得装原生化块(位闸只是兜底,文本层就该没有)"
                );
            }
            // 标记只出现读+写两处,且**不再**是 window 字符串键
            assert_eq!(
                s.matches("StealthRev").count(),
                2,
                "标记键只应出现读+写两处"
            );
            assert!(
                !s.contains(&format!("window.{}StealthRev", inject_ns())),
                "StealthRev 不得再是 window 字符串键:{s}"
            );
        }
    }

    /// 第 179 轮核心判据:窗口面注入 API 全部收敛到**一个 symbol 命名空间**,
    /// 渲染后不得残留任何 `window.<ns><Suffix>` 字符串键;DOM 面必须换用独立 token,
    /// 且长后缀优先替换（`agent_frame__` 不得被 `agent` 截断）。
    #[test]
    fn 渲染后窗口面零字符串键且DOM面独立() {
        let ns = inject_ns();
        let dom = dom_ns();
        let template = [
            "(() => { const NS = window[Symbol.for('%NS%')] || (window[Symbol.for('%NS%')] = {});",
            "window.%NS%GuardMode = 'locked';",
            "if (window.%NS%GuardInputSuspended) window.%NS%GuardInputSuspend(true);",
            "const a = document.getElementById('%NS%agent_frame__');",
            "const b = document.querySelector('[data-%NS%agent]');",
            "el.classList.add('%NS%locked__');",
            "h.__%NS%sh = null;",
            "})()",
        ]
        .join("\n");
        let out = render_with(&template, ns, dom);
        // 窗口面:全部改写为 NS.<Suffix>
        assert!(out.contains("NS.GuardMode = 'locked';"), "{out}");
        assert!(out.contains("NS.GuardInputSuspend(true)"), "{out}");
        // 不得残留任何 window.<ns> 字符串键形态
        assert!(
            !out.contains(&format!("window.{ns}")),
            "渲染后不得残留 window.<ns> 字符串键:{out}"
        );
        // DOM 面:独立 token,且 `agent_frame__` 未被 `agent` 截断
        assert!(out.contains(&format!("'{dom}agent_frame__'")), "{out}");
        assert!(out.contains(&format!("[data-{dom}agent]")), "{out}");
        assert!(out.contains(&format!("'{dom}locked__'")), "{out}");
        assert!(out.contains(&format!("__{dom}sh")), "{out}");
        assert!(!out.contains(&format!("{ns}agent")), "DOM 面不得残留窗口 token:{out}");
        // 符号描述本身保留(每个文档一个 symbol)
        assert!(out.contains(&format!("Symbol.for('{ns}')")));
    }

    /// 后缀白名单排序必须长优先:`GuardSuspended` / `AutoFollowStatus` /
    /// `AgentHighlightInstalled` 不得被它们的短前缀抢走。
    #[test]
    fn 后缀按长度降序匹配() {
        let list = sorted_suffixes(NS_WINDOW_SUFFIXES);
        let pos = |s: &str| list.iter().position(|x| *x == s).unwrap();
        assert!(pos("GuardSuspended") < pos("GuardSuspend"));
        assert!(pos("GuardInputSuspended") < pos("GuardInputSuspend"));
        assert!(pos("AutoFollowStatus") < pos("AutoFollow"));
        assert!(pos("AgentHighlightInstalled") < pos("AgentHighlight"));
        let doms = sorted_suffixes(NS_DOM_SUFFIXES);
        let dpos = |s: &str| doms.iter().position(|x| *x == s).unwrap();
        assert!(dpos("agent_frame__") < dpos("agent"));
    }

    /// 跨档升级时新表看不见上一轮登记的函数 → 原生化必须先「重新认领」。
    /// 认领入口就是页面能拿到引用的那一组稳定位置(原型上的属性/方法/迭代器)。
    #[test]
    fn 原生化前先重新认领上一轮改写() {
        use crate::agent::stealth::StealthMode;
        let h = stealth_js_for(StealthMode::Hardened).unwrap();
        // 认领动作必须在包装 toString 之前发生
        let adopt = h.find("__adoptAll();").expect("原生化前必须先认领");
        let wrap = h.find("Function.prototype.toString").expect("原生化块");
        assert!(adopt < wrap, "认领必须早于原生化,否则上一轮函数仍泄露源码");
        // 认领面覆盖「页面可触达」的固定位置,一个都不能少
        for spot in [
            "Navigator.prototype, 'webdriver'",
            "Navigator.prototype, 'plugins'",
            "Navigator.prototype, 'mimeTypes'",
            "Navigator.prototype, 'languages'",
            "window, 'chrome'",
            "Permissions.prototype, 'query'",
            "WebGLRenderingContext.prototype, 'getParameter'",
            "WebGL2RenderingContext.prototype, 'getParameter'",
        ] {
            assert!(
                h.contains(spot),
                "认领面漏了 {spot} —— 该位置的改写函数会退回可读源码形态"
            );
        }
        // 回填替身的中间原型(方法挂在那儿)也要认领,否则 namedItem/item 泄露
        assert!(h.contains("__adoptProto(Object.getPrototypeOf(arr))"));
        assert!(h.contains("__adoptProto(Object.getPrototypeOf(p0))"));
        // getter 与 value 两种形态都要登记(前者是访问器,后者是普通函数属性)
        assert!(h.contains("if (typeof d.get === 'function')"));
        assert!(h.contains("if (typeof d.value === 'function')"));
    }

    /// 第 177 轮真机实测（Chrome 155，本机 headed）：原生
    /// `MimeTypeArray.prototype` 的自有属性恒为 `length/item/namedItem/constructor`
    /// —— **没有 refresh**（`PluginArray.prototype` 才有）。回填替身必须照此分档，
    /// 多挂一个原生没有的键就是「形状错误的回填」，比不回填更危险。
    ///
    /// 同一次实测否掉了一个看似成立的「缺口」：曾据「原生 `Plugin` 必带
    /// `mimeTypes`」给替身补了该字段，真机读数 `'mimeTypes' in navigator.plugins[0]`
    /// 为 **false** —— WebIDL 的 `Plugin` 接口压根没有这个属性。故不得再补。
    #[test]
    fn 数组体refresh按原生分档() {
        use crate::agent::stealth::StealthMode;
        let f = stealth_js_for(StealthMode::Fidelity).unwrap();
        // 构造器带 withRefresh 形参,按档传值
        assert!(
            f.contains("const makeArrayLike = (items, RealProto, tag, nameOf, withRefresh) =>"),
            "数组体构造器必须带 withRefresh 形参"
        );
        assert!(
            f.contains("'PluginArray', (p) => p.name, true)"),
            "PluginArray 有 refresh(真机实测)"
        );
        assert!(
            f.contains("'MimeTypeArray', (m) => m.type, false)"),
            "MimeTypeArray 没有 refresh(真机实测),挂了就是多一个原生没有的键"
        );
        assert!(
            f.contains("if (withRefresh) {"),
            "refresh 必须条件挂载,不得无条件进数组"
        );
    }

    /// 第 178 轮:回填出「5 个 PDF 插件」却让 `navigator.pdfViewerEnabled` 停在
    /// 该环境的原生值(无插件环境通常为 false),就是自造矛盾 —— 与第 175 轮
    /// 「形状错误的回填比不回填更危险」同源。回填分支必须同批对齐,且只在读到
    /// false 时对齐(真实值 true 原样放行),并经 `reg()` 登记身份。
    #[test]
    fn 插件回填同批对齐PDF查看器开关() {
        use crate::agent::stealth::StealthMode;
        for m in [StealthMode::Fidelity, StealthMode::Hardened] {
            let s = stealth_js_for(m).unwrap();
            assert!(
                s.contains("navigator.pdfViewerEnabled === false"),
                "回填分支必须判 pdfViewerEnabled(只在 false 时对齐,真实值放行)"
            );
            assert!(
                s.contains("defNav('pdfViewerEnabled', true)"),
                "回填分支必须把 pdfViewerEnabled 对齐为 true"
            );
            // 跨档升级时的「重新认领」也要覆盖这个入口(否则升档后它退回可读源码)
            if m == StealthMode::Hardened {
                assert!(
                    s.contains("__adopt(Navigator.prototype, 'pdfViewerEnabled')"),
                    "认领面必须包含 pdfViewerEnabled"
                );
            }
        }
    }

    /// 不得凭空给替身加原生接口没有的属性。`Plugin` 的 WebIDL 接口只有
    /// name/filename/description + 索引取值,**没有 `mimeTypes`**;
    /// 真机 `'mimeTypes' in navigator.plugins[0] === false`。
    #[test]
    fn 不给插件替身虚构原生没有的属性() {
        use crate::agent::stealth::StealthMode;
        for m in [StealthMode::Fidelity, StealthMode::Hardened] {
            let s = stealth_js_for(m).unwrap();
            assert!(
                !s.contains("descriptorFor.getter('mimeTypes'"),
                "Plugin 原生没有 mimeTypes 属性(真机实测),补了反而制造破绽"
            );
            assert!(
                !s.contains("p.mimeTypes") || s.contains("__rehardenIterator"),
                "不得凭空读 Plugin.mimeTypes"
            );
        }
    }

    /// 注入时序：隐身脚本必须在 `goto` **之前**注册,否则首个文档的页面脚本已在
    /// document-start 读到未处置的 `navigator.webdriver=true` 并把风控信标发出去。
    /// 这条锁在 browser.rs 的 open 路径上(源码顺序断言),纯静态但能防回归。
    ///
    /// 第 178 轮扩展:建页必须用 `about:blank`。此前把目标 URL 交给 `createTarget`,
    /// 而 chromiumoxide 会等它**加载完成**才返回 —— 首个真实文档发生在隐身/UA/屏幕
    /// 几何处置**之前**(真机计数服务器实测:目标 URL 被请求 2 次,req#1 的 UA 原样带
    /// `HeadlessChrome`),「注册早于 goto」对它无效。
    #[test]
    fn 隐身脚本注册早于首次导航() {
        let src = include_str!("browser.rs");
        let inject = src
            .find("browser_ns::inject_stealth(&page, inner.stealth).await;")
            .expect("open 路径必须注册隐身脚本");
        let goto = src
            .find("page.goto(url.to_string())")
            .expect("open 路径必须导航");
        assert!(inject < goto, "隐身脚本注册必须早于首次导航");
        // 移位后不得残留「goto 之后再注入一次」的旧调用(重复注册无意义,
        // 且会让首个文档之外的白等一次 CDP 往返)
        let after = &src[goto..];
        let next_goto = after[1..].find("page.goto(url.to_string())");
        let window = match next_goto {
            Some(i) => &after[..i],
            None => after,
        };
        assert!(
            !window.contains("browser_ns::inject_stealth(&page, inner.stealth).await;"),
            "首次导航与下一次导航之间不得重复注册隐身脚本"
        );
        // 建页必须是空载起点:两条建页路径都不得再把目标 URL 交给 createTarget
        assert!(
            !src.contains(".url(url.to_string())"),
            "createTarget 不得携带目标 URL(那会让首个真实文档跑在处置之前)"
        );
        assert!(
            src.contains("browser.new_page(\"about:blank\")")
                && src.contains(".url(\"about:blank\".to_string())"),
            "两条建页路径(同窗口新 Tab / 独立窗口)都必须以 about:blank 起页"
        );
    }
    // ===================== 第 181 轮:改过不可反查闭环 =====================

    /// `__adopt` 只认领「属性值是函数 / 属性是 getter」,对**对象值里的自有函数**
    /// (如 `chrome = {app, csi, loadTimes}`)一个都认领不上 —— 跨档升级后
    /// `chrome.csi.toString()` 会吐出我们的源码。必须有按类收口的 `__adoptDeep`。
    #[test]
    fn 嵌套函数认领存在且被采用() {
        use crate::agent::stealth::StealthMode;
        let h = stealth_js_for(StealthMode::Hardened).expect("侵入式档必须注入");
        assert!(
            h.contains("__adoptDeep"),
            "必须有嵌套函数认领(否则 chrome.csi 等对象值函数跨档后吐源码)"
        );
        // chrome 必须走深度认领,而不是旧的 `__adopt(window, 'chrome')`
        assert!(
            h.contains("__adoptDeep(window.chrome, 0)"),
            "chrome 必须走深度认领:{h}"
        );
        // 深度上限存在(防爬进扩展注入的深层对象图)
        assert!(h.contains("(depth | 0) > 2"), "深度上限 2 必须在");
    }

    /// chrome 形态补全:真机 `Object.keys(chrome)` 恒为 `[app,csi,loadTimes]`,
    /// **在场但缺项**也要补(旧实现只在 `!window.chrome` 时创建,缺项裸奔)。
    /// 同时必须**不整体替换**已存在的 `window.chrome`(会毁掉扩展注入的 runtime)。
    #[test]
    fn chrome形态补全覆盖在场缺项() {
        use crate::agent::stealth::StealthMode;
        let f = stealth_js_for(StealthMode::Fidelity).expect("保真档必须注入");
        assert!(f.contains("needApp"), "必须有在场补全分支");
        assert!(f.contains("needCsi") && f.contains("needLoad"));
        // 三件套的工厂必须都带 reg() 登记(否则侵入档原生化不到)
        assert!(f.contains("reg(function csi()"), "csi 必须登记");
        assert!(f.contains("reg(function loadTimes()"), "loadTimes 必须登记");
        // 仍不得凭空补 chrome.runtime(175 轮真机结论:全新 profile 真机没有)。
        // 判据看**代码**而非注释:补全分支只允许 app/csi/loadTimes 三个键。
        assert!(
            !f.contains("put('runtime'"),
            "补全分支不得写入 runtime 键(真机全新 profile 本就没有)"
        );
        // 在场时不得整体替换(保护扩展注入的真实扩展上下文)
        assert!(
            f.contains("if (!ch)"),
            "只有 chrome 整体缺失时才整体创建,在场时只补缺失项"
        );
    }

    /// 自证入口必须点名 chrome 三件套 —— 登记漏网要在 `data.fingerprint` 里看得见。
    #[test]
    fn chrome自证入口在册() {
        assert!(
            crate::agent::fingerprint::PROBE_JS.contains("chrome_csi")
                && crate::agent::fingerprint::PROBE_JS.contains("chrome_loadTimes"),
            "patched_fn_leak 必须点名 chrome 三件套入口"
        );
    }
}


#[cfg(test)]
mod js_dump {
    /// 把渲染后的隐身脚本落到 `target/stealth_<档>.js`,供 `node --check` / 真机注入复核。
    /// 非断言型辅助(默认不跑),仅在需要时手工执行:
    /// `cargo test --lib js_dump -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn 导出渲染脚本供语法复核() {
        use crate::agent::browser_ns::stealth_js_for;
        use crate::agent::stealth::StealthMode;
        for (name, m) in [("fidelity", StealthMode::Fidelity), ("hardened", StealthMode::Hardened)] {
            let js = stealth_js_for(m).unwrap();
            let p = format!("target/stealth_{name}.js");
            std::fs::write(&p, js).unwrap();
            println!("wrote {p}");
        }
        // 第 179 轮:注入面收敛后,其余四套注入脚本一并导出做 `node --check`
        // (窗口面 API 全部改写成 `NS.<Suffix>`,语法错误只会在真机注入时才暴露)。
        let guard = crate::agent::browser_overlay::guard_script(
            &crate::agent::browser_overlay::PageGuardConfig::locked(),
        );
        std::fs::write("target/guard.js", guard).unwrap();
        println!("wrote target/guard.js");
        let status = crate::agent::browser_ns::render_ns(crate::agent::browser_status::AGENT_STATUS_JS_TEMPLATE);
        std::fs::write("target/status.js", status).unwrap();
        println!("wrote target/status.js");
        let highlight = crate::agent::browser_ns::render_ns(crate::agent::browser::AGENT_HIGHLIGHT_JS);
        std::fs::write("target/highlight.js", highlight).unwrap();
        println!("wrote target/highlight.js");
        let follow = crate::agent::browser_ns::render_ns(crate::agent::browser_follow::AUTO_FOLLOW_JS);
        std::fs::write("target/follow.js", follow).unwrap();
        println!("wrote target/follow.js");
    }
}
