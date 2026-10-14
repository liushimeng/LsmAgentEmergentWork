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

use std::sync::OnceLock;

/// 模板占位符（不是合法 JS 标识符，绝不会与模板内代码碰撞）。
pub const NS_TOKEN: &str = "%NS%";

/// 渲染注入模板：把模板内 `%NS%` 占位符替换为本进程命名空间。
pub fn render_ns(template: &str) -> String {
    template.replace(NS_TOKEN, inject_ns())
}

/// 进程级注入命名空间，惰性生成一次：`__lzn` + 8 位十六进制（如 `__lzn8f3a2b1`）。
static NS: OnceLock<String> = OnceLock::new();

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

/// 命名空间内的 window 全局名：`ns_global("GuardSuspend")` → `__lzn8f3a2b1GuardSuspend`。
pub fn ns_global(suffix: &str) -> String {
    format!("{}{}", inject_ns(), suffix)
}

/// 注入层 DOM 过滤属性名：`data-__lzn8f3a2b1agent`（替代固定 `data-laew-agent`）。
pub fn agent_attr() -> String {
    format!("data-{}agent", inject_ns())
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
    try {
        if (!navigator.plugins || navigator.plugins.length === 0) {
            const MIMES = [
                { type: 'application/pdf', suffixes: 'pdf', description: 'Portable Document Format' },
                { type: 'text/pdf', suffixes: 'pdf', description: 'Portable Document Format' }
            ];
            const NAMES = ['PDF Viewer', 'Chrome PDF Viewer', 'Chromium PDF Viewer',
                           'Microsoft Edge PDF Viewer', 'WebKit built-in PDF'];
            const makeMime = (m, plugin) => ({
                type: m.type, suffixes: m.suffixes, description: m.description, enabledPlugin: plugin
            });
            const plugins = NAMES.map((name) => {
                const plugin = {
                    name: name, filename: 'internal-pdf-viewer',
                    description: 'Portable Document Format', length: MIMES.length
                };
                const mimes = MIMES.map((m) => makeMime(m, plugin));
                plugin.item = (i) => (i >= 0 && i < mimes.length ? mimes[i] : null);
                plugin.namedItem = (t) => mimes.filter((m) => m.type === t)[0] || null;
                return plugin;
            });
            const mimeTypes = MIMES.map((m) => makeMime(m, plugins[0]));
            const arrLike = (items) => {
                const o = {
                    length: items.length,
                    item: (i) => (i >= 0 && i < items.length ? items[i] : null),
                    namedItem: (n) => items.filter((x) => x.name === n || x.type === n)[0] || null,
                    refresh: () => undefined
                };
                items.forEach((it, i) => { try { o[i] = it; } catch (e) {} });
                return o;
            };
            const defNav = (key, val) => {
                try {
                    Object.defineProperty(Navigator.prototype, key, {
                        get: reg(function () { return val; }, 'get ' + key),
                        configurable: true, enumerable: true
                    });
                } catch (e) {}
            };
            defNav('plugins', arrLike(plugins));
            defNav('mimeTypes', arrLike(mimeTypes));
        }
    } catch (e) {}

    // ③ navigator.languages 空值回填:取页面自报的 navigator.language 及其基语言。
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

    // ④ window.chrome 缺失回填:真实桌面 Chrome 恒有(chrome.runtime 是常见检测项)。
    try {
        if (!window.chrome) {
            Object.defineProperty(window, 'chrome', {
                value: { runtime: {}, app: { isInstalled: false } },
                configurable: true, enumerable: true, writable: true
            });
        } else if (!window.chrome.runtime) {
            try {
                Object.defineProperty(window.chrome, 'runtime', {
                    value: {}, configurable: true, enumerable: true
                });
            } catch (e) {}
        }
    } catch (e) {}

    // ⑤ Notification.permission 与 permissions.query 一致性:两者矛盾是经典无头检测
    //    组合。只在**确认不一致**时就地改写返回对象的 state(保留原生 PermissionStatus
    //    身份,不做对象替换),其余一切情况零动作。
    try {
        if (window.Permissions && Permissions.prototype.query
            && typeof Notification !== 'undefined' && Notification.permission === 'denied') {
            const origQuery = Permissions.prototype.query;
            const wrappedQuery = reg(function query(p) {
                const r = origQuery.apply(this, arguments);
                return Promise.resolve(r).then(function (st) {
                    try {
                        if (p && p.name === 'notifications' && st && st.state !== Notification.permission) {
                            Object.defineProperty(st, 'state', {
                                get: () => Notification.permission, configurable: true
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
    } catch (e) {}"#;

/// 原生化块（**侵入式**，第 174 轮）：`Function.prototype.toString` 对登记表内的函数
/// 报 `[native code]`。
///
/// 边界：这是「让我们的改写不可反查」，不改变任何函数行为。自反闭合 ——
/// `toString` 包装器自身也在登记表内，故
/// `Function.prototype.toString.call(Function.prototype.toString)` 同样报 native，
/// 不会因「toString 是自己的源码」而暴露。描述符与原生同形
/// （`writable: true, enumerable: false, configurable: true` —— 此处**必须**不可枚举）。
const HARDENED_JS: &str = r#"
    try {
        const origToString = Function.prototype.toString;
        const wrappedToString = function toString() {
            try { if (__fd.has(this)) return __fd.get(this); } catch (e) {}
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
    let mut blocks: Vec<&str> = vec![PRELUDE_JS, CORE_JS];
    if !fingerprint_kill_switch() {
        blocks.push(FIDELITY_JS);
    }
    if mode == StealthMode::Hardened {
        blocks.push(HARDENED_JS);
    }
    // 幂等闸(第 174 轮):同一文档**同档重复注入必须零动作** ——
    // `set_stealth` 会在每次 open 与运行时切档时对存活页面重挂脚本，而重复注入
    // 会叠加包装(第二份 `__fd` 登记表看不见第一份的函数 → 原生化被判为非 native,
    // 且 getParameter 被套两层)。升档(rev 变大)才允许重跑。标记本身不可枚举,
    // 不引入新的可枚举全局(隐身自检只查 laew 字样,但少一个键少一分痕迹)。
    let rev: u32 = if mode == StealthMode::Hardened { 2 } else { 1 };
    Some(render_ns(&format!(
        "(() => {{\n\
         \x20   try {{\n\
         \x20       if (window.{NS_TOKEN}StealthRev >= {rev}) return;\n\
         \x20       Object.defineProperty(window, '{NS_TOKEN}StealthRev', \
         {{ value: {rev}, configurable: true, enumerable: false }});\n\
         \x20   }} catch (e) {{}}\n\
         {}\n\
         }})()",
        blocks.join("\n")
    )))
}

/// 给页面安装隐身覆写（`Page.addScriptToEvaluateOnNewDocument`：导航/派生新文档
/// 自动重挂 + 当前文档立即执行）。connect 模式（接管用户浏览器）**不注入** ——
/// 真实浏览器本无该指纹，且 connect 契约是零注入。
pub async fn inject_stealth(page: &chromiumoxide::Page, mode: crate::agent::stealth::StealthMode) {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
    let Some(source) = stealth_js_for(mode) else {
        return;
    };
    let _ = page
        .execute(AddScriptToEvaluateOnNewDocumentParams {
            source,
            world_name: None,
            include_command_line_api: None,
            run_immediately: Some(true),
        })
        .await;
}

/// 无头档 UA 清理（第 174 轮）：`HeadlessChrome/155…` 会出现在 **UA 字符串 + 网络
/// 请求头 + `Sec-CH-UA` 品牌列表**三处，必须一起改 —— 只改 JS 侧的
/// `navigator.userAgent` 骗得过页面、骗不过服务器，且"UA 说 Chrome、Client Hints
/// 说 Headless"是更显眼的新破绽。故走 CDP `Emulation.setUserAgentOverride`
/// （唯一能同时改到网络头的通道）。
///
/// 取值全部来自页面自身读数（[`crate::agent::stealth::UA_PROBE_JS`]），只把品牌列表
/// 里的 `Headless*` 项滤掉 —— **零伪造**。返回 `true` 表示已下发覆盖。
pub async fn apply_ua_destealth(
    page: &chromiumoxide::Page,
    mode: crate::agent::stealth::StealthMode,
) -> bool {
    use crate::agent::stealth;
    if mode == stealth::StealthMode::Off {
        return false;
    }
    let params = match chromiumoxide::cdp::js_protocol::runtime::EvaluateParams::builder()
        .expression(stealth::UA_PROBE_JS)
        .return_by_value(true)
        .await_promise(true)
        .build()
    {
        Ok(p) => p,
        Err(_) => return false,
    };
    let raw = match tokio::time::timeout(std::time::Duration::from_secs(5), page.evaluate(params)).await
    {
        Ok(Ok(v)) => v,
        _ => return false,
    };
    let Some(text) = raw.value().and_then(|v| v.as_str()) else {
        return false;
    };
    let Ok(probe) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    // 无令牌(有头档常规情形)时零动作 —— 这是预期路径,不告警。
    let Some(plan) = stealth::ua_plan(&probe) else {
        return false;
    };
    let mut builder =
        chromiumoxide::cdp::browser_protocol::emulation::SetUserAgentOverrideParams::builder()
            .user_agent(plan.user_agent.clone());
    // 品牌列表被滤空时只改 UA 字符串，不下发一个比真值更可疑的空品牌表。
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
    }
    match builder.build() {
        Ok(p) => match page.execute(p).await {
            Ok(_) => true,
            Err(e) => {
                // 真失败(检测到令牌但覆盖下发不出去)才告警:此时该文档仍带自动化
                // 令牌,是"该修没修上",必须留痕。fail-open 不阻断导航。
                tracing::warn!("UA 去自动化令牌覆盖下发失败(本页 UA 仍可能带 Headless 令牌): {e}");
                false
            }
        },
        Err(e) => {
            tracing::warn!("UA 去自动化令牌覆盖参数构造失败: {e}");
            false
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
        assert_eq!(ns_global("GuardSuspend"), format!("{}GuardSuspend", inject_ns()));
        assert_eq!(agent_attr(), format!("data-{}agent", inject_ns()));
        assert_eq!(
            agent_attr_selector(),
            format!("[data-{}agent]", inject_ns())
        );
        assert!(!agent_attr().contains("laew"), "属性名不得含固定签名 laew");
        assert!(!ns_global("X").contains("laew"));
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
        // ④ window.chrome 缺失回填
        assert!(f.contains("'chrome'"));
        assert!(f.contains("chrome.runtime"));
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
}

