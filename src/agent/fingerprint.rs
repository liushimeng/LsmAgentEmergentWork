//! 反检测指纹体检(第 172 轮):把「这环境在风控眼里有多可疑」变成可见数据。
//!
//! 背景(BOSS 直聘实测,见
//! `tmpPlan/20261014_164500_BOSS直聘页面自刷新与反爬识别治理方案.md`):
//! 站点对「一次性全新 profile + 自动化浏览器」做自杀式反爬(原地 reload /
//! 抹成空白页)。我们在 `browser_ns.rs` 已压掉 `navigator.webdriver`,但实测
//! WebGL 仍返回 `WebKit|WebKit WebGL` —— 那是 Chrome 拿不到
//! `WEBGL_debug_renderer_info` 时的**自动化兜底串**,真实 macOS Chrome 给的是
//! `Google Inc. (Apple) / ANGLE (Apple, ANGLE Metal Renderer: Apple M…)`。
//! 风控 JS 采 WebGL 是标准动作,这一项单独就足以把环境打到高风险分。
//!
//! 本模块做两件事:
//! 1. `probe_js` / `parse` —— 一次 eval 采齐一组指纹字段(纯函数可单测);
//! 2. `classify` —— 把字段翻译成 `suspicious` 列表 + 可行动建议,
//!    随 `open` 响应 `data.fingerprint` 回传,让「被反爬」这件事第一轮就可见。
//!
//! 诚实边界:本模块**不做**验证码/风控绕过,只做「不留明显破绽 + 被拦时立刻可见
//! + 给出合规改道(接管用户已登录浏览器)」。

use serde_json::{json, Value};

/// 采集指纹的页面脚本(返回值 `JSON.stringify`,避免 CDP 值序列化踩坑)。
pub const PROBE_JS: &str = r#"JSON.stringify((() => {
  const gl = (() => {
    try {
      const c = document.createElement('canvas');
      const g = c.getContext('webgl') || c.getContext('experimental-webgl');
      if (!g) return { ok: false };
      const ext = g.getExtension('WEBGL_debug_renderer_info');
      return {
        ok: true,
        vendor: ext ? g.getParameter(ext.UNMASKED_VENDOR_WEBGL) : g.getParameter(g.VENDOR),
        renderer: ext ? g.getParameter(ext.UNMASKED_RENDERER_WEBGL) : g.getParameter(g.RENDERER),
        unmasked: !!ext
      };
    } catch (e) { return { ok: false }; }
  })();
  return {
    webdriver: navigator.webdriver === true,
    webgl: gl,
    plugins: navigator.plugins ? navigator.plugins.length : 0,
    languages: navigator.languages ? navigator.languages.length : 0,
    chrome_runtime: !!(window.chrome && window.chrome.runtime),
    hardware_concurrency: navigator.hardwareConcurrency || 0,
    max_touch_points: navigator.maxTouchPoints || 0,
    ua_platform: navigator.platform || '',
    ua_brand: navigator.userAgentData ? navigator.userAgentData.brands.map(b => b.brand + ' ' + b.version).join(',') : '',
    screen: [screen.width, screen.height, devicePixelRatio || 1].join('x'),
    timezone: (() => { try { return Intl.DateTimeFormat().resolvedOptions().timeZone || ''; } catch (e) { return ''; } })(),
    notification_permission: (typeof Notification !== 'undefined') ? Notification.permission : 'unsupported',
    // 第 174 轮:反检测档位对账用项(缺测量返回 null = fail-open,不告警)
    ua_headless: (navigator.userAgent || '').indexOf('Headless') >= 0,
    webgl_parameter_enumerable: (() => {
      try {
        const p = window.WebGLRenderingContext && WebGLRenderingContext.prototype;
        if (!p) return null;
        const d = Object.getOwnPropertyDescriptor(p, 'getParameter');
        return d ? !!d.enumerable : null;
      } catch (e) { return null; }
    })(),
    patched_functions_native: (() => {
      try {
        const p = window.WebGLRenderingContext && WebGLRenderingContext.prototype;
        if (!p || !p.getParameter) return null;
        return /\[native code\]/.test(Function.prototype.toString.call(p.getParameter));
      } catch (e) { return null; }
    })(),
    // 第 175 轮:屏幕几何自洽自证(根治「outer > screen 物理不可能」)。
    // `outer_gt_screen` = 窗口比屏幕还大(5% 容差吸收 DPI 缩放/多显示器坐标差异),
    // 真实显示器宽度永远 ≥ 窗口宽度,命中即说明视口覆盖没带 screen 声明。
    geometry: (() => {
      try {
        const sw = screen.width, sh = screen.height;
        const ow = window.outerWidth || 0, oh = window.outerHeight || 0;
        const iw = window.innerWidth || 0, ih = window.innerHeight || 0;
        return {
          screen: sw + 'x' + sh,
          window_outer: ow + 'x' + oh,
          window_inner: iw + 'x' + ih,
          outer_gt_screen: ow > sw * 1.05 || oh > sh * 1.05,
          screen_is_headless_default: sw === 800 && sh === 600,
          // 第 179 轮:无头档窗口几何 signature —— 真实有头窗口有标题栏/工具栏,
          // outer 恒 > inner;outer === inner 是无头环境的典型读数(全屏窗口同样命中,
          // 故只作软提示,不当硬判据)。
          outer_equals_inner: ow > 0 && oh > 0 && iw > 0 && ih > 0
              && ow === iw && oh === ih
        };
      } catch (e) { return null; }
    })(),
    // 第 175 轮:plugins/mimeTypes **对象形态**自证。回填动作只有与原生
    // PluginArray 同构才安全(形状错误的回填比不回填更危险),故体检层必须能
    // 看见形态失配:tag 不是 [object PluginArray] 或 instanceof 为 false
    // 即说明我们造出了形状错误的替身。
    plugin_shape_ok: (() => {
      try {
        if (!navigator.plugins || navigator.plugins.length === 0) return null; // 本就空,不判定
        const tag = Object.prototype.toString.call(navigator.plugins);
        const inst = (typeof PluginArray === 'function') ? (navigator.plugins instanceof PluginArray) : null;
        const named = (() => { try { return !!navigator.plugins.namedItem('PDF Viewer'); } catch (e) { return false; } })();
        return { tag, inst, named_item_ok: named };
      } catch (e) { return null; }
    })(),
    // 第 176 轮:languages 回填形态自证。**第 178 轮方向修正(真机实测驱动)**:
    // `NavigatorLanguage.languages` 的 WebIDL 类型是 `FrozenArray<DOMString>`,
    // 原生**就是冻结的**(裸环境 `stealth=off` 四档实测 `Object.isFrozen === true`)。
    // 故告警条件是「读到**非**冻结」—— 那才说明注入把原生形态改坏了。
    languages_frozen: (() => {
      try { return Object.isFrozen(navigator.languages || []); } catch (e) { return null; }
    })(),
    // 第 178 轮:PDF 查看器开关与插件表的一致性(2xx 系的回填产物自洽)。
    pdf_viewer_enabled: (() => {
      try { return (typeof navigator.pdfViewerEnabled === 'undefined') ? null : navigator.pdfViewerEnabled; }
      catch (e) { return null; }
    })(),
    // 第 178 轮:注入层 window 键面计量。**第 178 轮修了一个死自证** ——
    // 旧正则 `^__lz[0-9a-f]{8}` 撞不上真实形态 `__lzn` + 8 hex(第 5 字符是 n,
    // 不是 hex),该 filter 恒为空数组,这一项自诞生起就一直报 0。真机实测:
    // 有头档 30 个、无头档 1 个、裸环境 0 个。`*_symbols` 为下一步「收敛成
    // 单一 symbol 命名空间」提供验收基线(原生恒 0)。
    injected_ns_keys: (() => {
      try { return Object.getOwnPropertyNames(window).filter((n) => /^__lzn[0-9a-f]{8}/.test(n)).length; }
      catch (e) { return null; }
    })(),
    injected_ns_symbols: (() => {
      try { return Object.getOwnPropertySymbols(window).length; } catch (e) { return null; }
    })(),
    // 第 179 轮:注入面收敛的**正向**判据 —— window 上存在一个 symbol 键,其值
    // 带 `StealthRev` 数字位掩码,即命名空间对象在场。字符串键应恒为 0
    // (`injected_ns_keys`),符号键应为 1。
    ns_symbol_present: (() => {
      try {
        return Object.getOwnPropertySymbols(window).some((s) => {
          try {
            const v = window[s];
            return !!(v && typeof v === 'object' && typeof v.StealthRev === 'number');
          } catch (e) { return false; }
        });
      } catch (e) { return null; }
    })(),
    // 第 176 轮:迭代器自反性自证。原生 Iterator 的迭代器本身 iterable
    // (iterator[Symbol.iterator] 是函数);若为 undefined 说明我们返回的裸对象
    // 没补自反方法 —— 零成本形态判据。
    iterator_self_iterable: (() => {
      try {
        if (!navigator.plugins || navigator.plugins.length === 0) return null;
        const it = navigator.plugins[Symbol.iterator]();
        return typeof (it && it[Symbol.iterator]);
      } catch (e) { return null; }
    })(),
    // ===================== 第 177 轮:漏检与自造破绽自证 =====================
    // ① 已知自动化框架的全局泄漏扫描。第三方驱动/框架(driver、playwright、puppeteer、
    //    phantom、selenium)会在 window 上挂固定名全局,一行 `getOwnPropertyNames` 即可
    //    枚举。我们自己经 CDP 驱动不产生这些键 —— 非空即说明页面里有第三方自动化痕迹。
    automation_globals: (() => {
      try {
        const re = /^(cdc_|domAutomation|_phantom$|callPhantom$|__selenium|__nightmare|__playwright|__pw|__puppeteer|__driver|__fxdriver|webdriver$)/i;
        return Object.getOwnPropertyNames(window).filter((n) => re.test(n));
      } catch (e) { return null; }
    })(),
    // ② 我们自己改写过的函数里**仍能被 toString 读到源码**的那些(原生化登记漏网)。
    //    逐个走页面可触达的稳定入口取引用再问 toString;非 `[native code]` 即漏登记。
    //    保真档下命中属预期(该档不改函数身份),侵入档命中即说明登记表没兜住。
    patched_fn_leak: (() => {
      try {
        const sites = [];
        const add = (label, fn) => {
          try {
            if (typeof fn !== 'function') return;
            if (/\[native code\]/.test(Function.prototype.toString.call(fn))) return;
            sites.push(label);
          } catch (e) {}
        };
        try { add('webgl_getParameter', WebGLRenderingContext.prototype.getParameter); } catch (e) {}
        try { add('permissions_query', Permissions.prototype.query); } catch (e) {}
        try {
          const d = Object.getOwnPropertyDescriptor(Navigator.prototype, 'plugins');
          if (d && d.get) add('nav_plugins_getter', d.get);
          const w = Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver');
          if (w && w.get) add('nav_webdriver_getter', w.get);
          // 第 179 轮:补齐另外三个会被保真块改写的入口(漏检面收全)
          const lg = Object.getOwnPropertyDescriptor(Navigator.prototype, 'languages');
          if (lg && lg.get) add('nav_languages_getter', lg.get);
          const pv = Object.getOwnPropertyDescriptor(Navigator.prototype, 'pdfViewerEnabled');
          if (pv && pv.get) add('nav_pdf_viewer_getter', pv.get);
          add('function_toString', Function.prototype.toString);
        } catch (e) {}
        try {
          const it = navigator.plugins && navigator.plugins[Symbol.iterator] && navigator.plugins[Symbol.iterator]();
          if (it) { add('plugins_iter_next', it.next); add('plugins_iter_self', it[Symbol.iterator]); }
        } catch (e) {}
        // 第 181 轮:补 chrome 三件套入口 —— 它们挂在**对象值**上,旧的
        // `__adopt(window,'chrome')` 认领不到,跨档升级后正是这里吐源码。
        try {
          const c = window.chrome;
          if (c) {
            if (typeof c.csi === 'function') add('chrome_csi', c.csi);
            if (typeof c.loadTimes === 'function') add('chrome_loadTimes', c.loadTimes);
            if (c.app && typeof c.app.install === 'function') add('chrome_app_install', c.app.install);
          }
        } catch (e) {}
        return sites;
      } catch (e) { return null; }
    })(),
    // ③ 焦点/可见性自洽:**第 178 轮降级为只观测**。第 177 轮曾把
    //    「visible 却无焦点」当硬判据(命中即 risk_tier=high),真机四档实测该
    //    推断不成立:有头档恒 `has_focus=false`(缺省 `LAEW_WEB_FOCUS=keep` 会
    //    主动把键盘焦点归还给调用前的应用,是**我们自己的设计**),无头档反而
    //    `has_focus=true` —— 与理论相反;且真实用户窗口被别的窗口遮挡(未最小化)时
    //    同样是 visible + 无焦点,这是**普通浏览的常态**而非「无人值守」证据。
    //    字段保留供诊断,不再参与 suspicious / HARD_CODES。
    focus_visibility: (() => {
      try {
        return { visibility: document.visibilityState || '', has_focus: document.hasFocus() === true };
      } catch (e) { return null; }
    })(),
    // ===================== 第 181 轮:体检面补齐(只观测,不回填) =====================
    // 上面的探针表与 26 §一 表 1–10(我们**已处置**的项)一一对应;这里补的是风控
    // 还会采、而我们既未处置也看不见的向量。本轮一律**只体检不改读数** ——
    // 「被拦时如实上报并交还人工」的前提是看得见自己被拦在哪儿。
    // 一律 fail-open:任何探测抛错返回 null,不告警不猜测。
    // ① `Error().stack` 里的自动化令牌。真实 Chrome 的 stack 只有页面/V8 帧,
    //    出现驱动或自动化框架名即硬证据(我们经 CDP 驱动不产生这些帧)。
    stack_automation_tokens: (() => {
      try {
        const s = (new Error()).stack || '';
        if (!s) return null;
        const re = /\$cdc_|\$chrome_asyncScriptInfo|__puppeteer|__playwright|__nightmare|__selenium|_phantom|callPhantom|pptr|puppeteer|playwright/i;
        return re.test(s);
      } catch (e) { return null; }
    })(),
    // ② `document` 上的 chromedriver 系遗留。旧实现只扫 `window` 自有键,
    //    document 这一侧完全是盲区。
    document_automation_globals: (() => {
      try {
        const re = /^\$cdc_|\$chrome_asyncScriptInfo|__webdriver_|__driver_/i;
        return Object.getOwnPropertyNames(document).filter((n) => re.test(n));
      } catch (e) { return null; }
    })(),
    // ③ `window.chrome` 形态自证。真机 `Object.keys(chrome)` 恒为 `[app,csi,loadTimes]`,
    //    在场却缺任一即非真 Chrome 形态(无头/被降级环境)。注意:**不检查
    //    `chrome.runtime`** —— 175 轮真机实测全新 profile 的真机没有它。
    chrome_shape: (() => {
      try {
        if (!window.chrome) return null;
        return {
          app: !!window.chrome.app,
          csi: typeof window.chrome.csi === 'function',
          load_times: typeof window.chrome.loadTimes === 'function'
        };
      } catch (e) { return null; }
    })(),
    // ④ `navigator.webdriver` 的**位置**自证。原生只挂在 `Navigator.prototype` 上,
    //    实例自有属性恒 undefined;若 `getOwnPropertyDescriptor(navigator,'webdriver')`
    //    非空,说明 defineProperty 打错了目标(改到实例上)—— 一行即可识破。
    webdriver_own_property: (() => {
      try {
        return Object.getOwnPropertyDescriptor(navigator, 'webdriver') != null;
      } catch (e) { return null; }
    })(),
    // ⑤ 指针/悬停媒体查询。桌面 UA 下 `(hover:hover)` 与 `(pointer:fine)` 双双 false
    //    是无头环境的典型读数。**本轮只观测不回填** —— 是否属「无头误报」需真机
    //    四档基线确认(见 35 §六.1)。
    pointer_mq: (() => {
      try {
        const mq = (q) => { try { return !!(window.matchMedia && matchMedia(q).matches); } catch (e) { return null; } };
        return { hover: mq('(hover:hover)'), fine: mq('(pointer:fine)') };
      } catch (e) { return null; }
    })(),
    // ⑥ colorDepth / pixelDepth 一致性(真实浏览器二者恒等)。
    color_depth: (() => {
      try {
        return { color: screen.colorDepth, pixel: screen.pixelDepth };
      } catch (e) { return null; }
    })(),
    // ⑦ `screen.orientation` 存在性(现代 Chrome 恒有)。
    screen_orientation: (() => {
      try {
        return { present: !!(screen.orientation && screen.orientation.type), type: (screen.orientation && screen.orientation.type) || '' };
      } catch (e) { return null; }
    })(),
    // ⑧ 硬件参数可置信度。`hardwareConcurrency`/`deviceMemory` 为 0 是物理不可能
    //    (真机恒 ≥1),已采集多年却从未判定 —— 这里补上判定并回报原值。
    hardware_params: (() => {
      try {
        return {
          hc: navigator.hardwareConcurrency || 0,
          dm: (typeof navigator.deviceMemory === 'number') ? navigator.deviceMemory : null
        };
      } catch (e) { return null; }
    })(),
    // ⑨ `navigator.platform` 与 UA 声明的 OS 交叉一致性。
    platform_ua: (() => {
      try {
        return { platform: navigator.platform || '', ua: navigator.userAgent || '' };
      } catch (e) { return null; }
    })()
  };
})())"#;

/// WebGL「自动化兜底串」判定(小写包含)。
fn is_default_webgl(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    s.contains("webkit") || s.contains("swiftshader") || s.contains("software")
}

/// 解析 [`PROBE_JS`] 返回文本。
pub fn parse(raw: &str) -> Option<Value> {
    serde_json::from_str::<Value>(raw.trim()).ok()
}

/// 把指纹字段翻译成 `suspicious` 列表(`{code, advice}`)。
pub fn classify(fp: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let webgl = fp.get("webgl").cloned().unwrap_or(Value::Null);
    let unmasked = webgl.get("unmasked").and_then(Value::as_bool).unwrap_or(false);
    let vendor = webgl.get("vendor").and_then(Value::as_str).unwrap_or("");
    let renderer = webgl.get("renderer").and_then(Value::as_str).unwrap_or("");
    if !unmasked {
        out.push(json!({
            "code": "webgl_debug_renderer_info_missing",
            "advice": "页面拿不到 WEBGL_debug_renderer_info(风控最常采集的硬件项之一),\
                       需在页面脚本加载前补齐(见 browser_ns::fingerprint_fill_js)"
        }));
    } else if is_default_webgl(vendor) || is_default_webgl(renderer) {
        out.push(json!({
            "code": "webgl_default_strings",
            "advice": "WebGL vendor/renderer 是自动化兜底串(WebKit/SwiftShader/software),\
                       与真实桌面 Chrome 的 Apple/ANGLE/D3D11 串不符,是高风险指纹;\
                       处置:已在 browser_ns 注入保真回填,若仍命中请开 LAEW_WEB_FINGERPRINT=off 复核是否被覆盖;\
                       关键任务建议 open(reuse_existing=true) 用真实浏览器指纹"
        }));
    }
    if fp.get("webdriver").and_then(Value::as_bool).unwrap_or(false) {
        out.push(json!({
            "code": "webdriver_true",
            "advice": "navigator.webdriver 仍为 true(隐身覆写未生效或被站点读到原生值),\
                       几乎必被风控判机器人;复核 launch 是否带 --disable-blink-features=AutomationControlled"
        }));
    }
    if fp.get("plugins").and_then(Value::as_u64).unwrap_or(0) == 0 {
        out.push(json!({
            "code": "no_plugins",
            "advice": "navigator.plugins 为空(真实桌面 Chrome 恒 ≥3),典型无头/受限环境特征"
        }));
    }
    if fp.get("languages").and_then(Value::as_u64).unwrap_or(0) == 0 {
        out.push(json!({
            "code": "no_languages",
            "advice": "navigator.languages 为空,与 UA 声明的系统语言不自洽"
        }));
    }
    // `window.chrome.runtime` 只在加载了扩展时存在;无扩展的真实 Chrome(含
    // 全新 profile 冷启动)读它同样为 undefined —— 把它打成 suspicious 会把
    // 本机每一台合规启动的浏览器都标成可疑(2026-10-14 实测 oneshot 修复后的
    // example.com 探针误报)。故只采字段不告警,该项不进入 suspicious。
    //
    // 第 175 轮真机复核**确认了这条结论**:裸环境 `Object.keys(chrome)` 恒为
    // `[app, csi, loadTimes]`,`chrome.runtime` 不存在。据此 `browser_ns::FIDELITY_JS`
    // ④ 已删除「凭空补 runtime」的旧实现 —— 造一个空 `chrome.runtime` 既违反
    // 「不伪造本机没有的信息」铁律,又可被一行识破(真实扩展上下文的 runtime
    // 必有 id/connect/sendMessage,空 runtime 比没有 runtime 更可疑)。
    // Notification.permission = 'denied' 但 permissions.query 报告 'prompt'
    // 是知名检测组合;这里只做单侧提示(不额外发一次 permissions.query)。
    if fp.get("notification_permission").and_then(Value::as_str) == Some("denied") {
        out.push(json!({
            "code": "notification_denied",
            "advice": "Notification.permission=denied(站点常据此推断自动化/无人值守),人工确认是否被我们的权限预授权影响"
        }));
    }
    // ===================== 第 174 轮:反检测档位自证项 =====================
    // 以下三项只在**明确读到异常值**时告警(缺测量 = fail-open 不告警)。
    if fp.get("ua_headless").and_then(Value::as_bool) == Some(true) {
        out.push(json!({
            "code": "headless_ua",
            "advice": "UA 仍含 HeadlessChrome 令牌 —— 它同时出现在网络请求头与 Sec-CH-UA 品牌表,\
                       服务端一眼识别为无头自动化;处置:确认启动档非 LAEW_WEB_STEALTH=off,\
                       工具会在首航前经 Emulation.setUserAgentOverride 清理(仅 token 清理,不改身份)"
        }));
    }
    if fp.get("webgl_parameter_enumerable").and_then(Value::as_bool) == Some(false) {
        out.push(json!({
            "code": "webgl_descriptor_tampered",
            "advice": "被我们改写的 WebGL 方法描述符 enumerable=false(原生 WebIDL 操作恒可枚举),\
                       for…in 枚举原型链即可发现「改过」;处置:确认非 LAEW_WEB_FINGERPRINT=off 且注入生效"
        }));
    }
    // patched_functions_native=false 在**非侵入式档属预期**(保真档不改函数身份),
    // 故措辞是「可升档」而非「故障」。
    if fp.get("patched_functions_native").and_then(Value::as_bool) == Some(false) {
        out.push(json!({
            "code": "patched_functions_visible",
            "advice": "被改写函数能被 Function.prototype.toString 读到我们的源码(压制手法本身成为签名);\
                       需要更强隐蔽时升档 open(stealth=\"hardened\") —— 侵入式档会把这些函数的身份原生化"
        }));
    }
    // ===================== 第 175 轮:屏幕几何与对象形态自证 =====================
    // 两项都是「我们自己制造的破绽」类信号,命中即说明对应根治逻辑失效,
    // 故措辞是明确的「处置」而非「可升档」。
    if let Some(g) = fp.get("geometry").filter(|v| !v.is_null()) {
        let headless_default = g.get("screen_is_headless_default").and_then(Value::as_bool) == Some(true);
        if g.get("outer_gt_screen").and_then(Value::as_bool) == Some(true) {
            if headless_default {
                // screen=800x600 无头默认值 + outer 更大 → 视口覆盖缺 screen 声明
                out.push(json!({
                    "code": "screen_geometry_impossible",
                    "advice": format!(
                        "窗口比屏幕还大(window.outer={} > screen={}) —— 真实显示器宽度永远 ≥ 窗口宽度,\
                         这是物理不可能,属零成本风控硬判据;根因是视口覆盖没带 screen 声明\
                         (screen 停在 Chrome 无头默认值 800x600);处置:确认页面由本工具拉起\
                         且走了屏幕几何自洽覆盖",
                        g.get("window_outer").and_then(Value::as_str).unwrap_or("?"),
                        g.get("screen").and_then(Value::as_str).unwrap_or("?")
                    )
                }));
            } else {
                // screen 非无头默认值但 outer 仍略大 → DPI 缩放/多显示器坐标差异
                // (实测 Windows 125% 缩放:outer=1922 vs screen=1907,差 0.8%)
                // 5% 容差已吸收大部分,残余命中降级为 info 级提示,不计入 suspicious
            }
        } else if headless_default {
            out.push(json!({
                "code": "screen_headless_default",
                "advice": "screen 恰为 Chrome 无头默认值 800x600(未收到 screen 声明的典型读数);\
                           比 outer_gt_screen 更弱但同样指向无头环境,处置同 screen_geometry_impossible"
            }));
        }
    }
    // 形状错误的回填比不回填更危险 —— 形态失配是强信号(正常浏览器不会造错形状)。
    // `plugin_shape_ok` 为 null 表示「插件本就为空 / 探测失败」,不参与判定。
    if let Some(p) = fp.get("plugin_shape_ok").filter(|v| !v.is_null()) {
        let tag_ok = p.get("tag").and_then(Value::as_str) == Some("[object PluginArray]");
        let inst_bad = p.get("inst").and_then(Value::as_bool) == Some(false);
        let named_bad = p.get("named_item_ok").and_then(Value::as_bool) == Some(false);
        if !tag_ok || inst_bad || named_bad {
            out.push(json!({
                "code": "plugin_shape_mismatch",
                "advice": format!(
                    "plugins 替身与原生 PluginArray 形态不符(tag={} instanceof={} namedItem={})—— \
                     形状错误的回填比不回填更危险(缺插件只是弱信号,错形状是强信号);\
                     处置:确认注入的是最新 browser_ns::FIDELITY_JS(私有中间原型 + 真原型链),\
                     旧的对象字面量实现会全线命中",
                    p.get("tag").and_then(Value::as_str).unwrap_or("?"),
                    p.get("inst").map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                    p.get("named_item_ok").map(|v| v.to_string()).unwrap_or_else(|| "?".into())
                )
            }));
        }
    }
    // ===================== 第 176 轮:languages/迭代器形态自证 =====================
    // 第 178 轮**方向修正**:原生 languages 是 `FrozenArray<DOMString>`(冻结),
    // 故告警条件是读到**非**冻结 —— 只有注入把它改坏了才会非冻结。
    // 第 176 轮按相反的推断(「原生非 frozen」)告警,真机四档实测证伪:那条自证
    // 在**每一台真实浏览器上恒告警**,把干净的 risk_tier=ok 常年抬成 warn。
    if fp.get("languages_frozen").and_then(Value::as_bool) == Some(false) {
        out.push(json!({
            "code": "languages_not_frozen",
            "advice": "navigator.languages **非冻结** —— 原生 `NavigatorLanguage.languages` 的 \
                       WebIDL 类型是 FrozenArray<DOMString>(真机实测裸环境亦为 true),\
                       非冻结说明有注入把它改坏了(严格模式 push 不抛本身即判据);\
                       处置:确认注入的是最新 browser_ns::FIDELITY_JS(回填经 Object.freeze)"
        }));
    }
    // 第 178 轮:PDF 查看器开关与插件表必须自洽 —— 插件表非空却报「PDF 查看器不可用」
    // 是自造矛盾(回填分支现在会一并对齐,故这条命中即说明对齐没生效或环境被改坏)。
    {
        let plugins = fp.get("plugins").and_then(Value::as_u64).unwrap_or(0);
        if plugins > 0 && fp.get("pdf_viewer_enabled").and_then(Value::as_bool) == Some(false) {
            out.push(json!({
                "code": "pdf_viewer_plugin_mismatch",
                "advice": "navigator.plugins 非空(含 PDF 插件)却报 pdfViewerEnabled=false,\
                           两者矛盾(真实浏览器二者一致);处置:确认注入的是最新 \
                           browser_ns::FIDELITY_JS(回填分支已同批对齐 pdfViewerEnabled)"
            }));
        }
    }
    // iterator_self_iterable 缺测(null = 插件本就空 / 探测失败) = fail-open 不告警;
    // 只有明确读到非 'function'(即 undefined,裸对象没补自反)才告警。
    if let Some(v) = fp.get("iterator_self_iterable") {
        if !v.is_null() && v.as_str() != Some("function") {
            out.push(json!({
                "code": "iterator_not_self_iterable",
                "advice": "plugins[Symbol.iterator]() 返回的迭代器不自反(原生 Iterator 迭代器\
                           本身 iterable,iterator[Symbol.iterator] 是函数);\
                           处置:确认注入的是最新 browser_ns::FIDELITY_JS(第 176 轮已补自反)"
            }));
        }
    }
    // ===================== 第 177 轮:漏检与自造破绽 =====================
    // 第三方自动化框架的全局泄漏:我们经 CDP 驱动不产生这些键,非空即说明页面上还有
    // 别的自动化痕迹(或站点自己埋的同名脚本)。这是**硬判据**,优先级与 webdriver 同级。
    if let Some(leaked) = fp.get("automation_globals").and_then(Value::as_array) {
        if !leaked.is_empty() {
            out.push(json!({
                "code": "automation_global_leak",
                "advice": format!(
                    "window 上存在已知自动化框架的全局: {} —— 本工具经 CDP 驱动不会产生这些键,\
                     命中说明页面上还有第三方自动化痕迹(或站点脚本同名),\
                     处置:open(reuse_existing=true) 接管用户已登录的真实浏览器",
                    leaked.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
                )
            }));
        }
    }
    // 自造破绽:被改写函数仍可被 toString 读到源码。侵入档命中即登记表漏网(处置=查注入),
    // 保真档命中属该档语义(它压根不改函数身份)——措辞按「可升档」而非「故障」。
    if let Some(sites) = fp.get("patched_fn_leak").and_then(Value::as_array) {
        if !sites.is_empty() {
            out.push(json!({
                "code": "patched_function_source_visible",
                "advice": format!(
                    "这些被改写的函数能被 Function.prototype.toString 读到源码(压制手法本身成为签名): {}\
                     —— 保真档下这是该档语义;若已在 data.stealth.mode=hardened 仍命中,\
                     说明身份登记表漏了这些入口,处置:确认注入的是最新 browser_ns::PRELUDE_JS/HARDENED_JS",
                    sites.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
                )
            }));
        }
    }
    // 第 178 轮:**焦点/可见性自洽退出告警面**。第 177 轮把它当硬判据,真机四档
    // 实测推翻(详见 PROBE_JS 内该字段注释):有头档因我们的焦点租约恒无焦点、
    // 无头档反有焦点,且真实用户窗口被遮挡时同样是 visible+无焦点。字段只回报不判定。
    //
    // ===================== 第 179 轮:注入面收敛自证 =====================
    // 注入层窗口面 API 已全部收敛到一个 symbol 命名空间对象。`injected_ns_keys`
    // 非零 = 有代码倒退回了 `window.<ns>Foo` 字符串键形态(真机基线:收敛前有头档
    // 30 / 无头档 1 / 裸环境 0;收敛后应**恒为 0**)。这是**我们自己制造的**签名,
    // 命中即应修注入,而不是去调风控策略。
    if let Some(keys) = fp.get("injected_ns_keys").and_then(Value::as_u64) {
        if keys > 0 {
            out.push(json!({
                "code": "injected_ns_string_keys",
                "advice": format!(
                    "window 上残留 {keys} 个注入层字符串键(`__lzn…` 前缀)—— 注入面收敛被破坏,\
                     风控一行 `getOwnPropertyNames(window)` 即可枚举;\
                     处置:确认注入脚本走 browser_ns::render_ns(窗口面 API 必须改写成 `NS.<Suffix>`),\
                     新增窗口 API 时同步登记 NS_WINDOW_SUFFIXES"
                )
            }));
        }
    }
    // 无头档窗口几何 signature。真实有头窗口有标题栏/工具栏,outer 恒 > inner;
    // outer === inner 是无头环境的典型读数。全屏的真实窗口也会命中,故只作软提示。
    if let Some(g) = fp.get("geometry").filter(|v| !v.is_null()) {
        if g.get("outer_equals_inner").and_then(Value::as_bool) == Some(true) {
            out.push(json!({
                "code": "outer_equals_inner",
                "advice": "window.outer 与 inner 完全相等(真实有头窗口因标题栏/工具栏恒 outer>inner),\
                           属无头档窗口几何特征;关键站点优先 headed 可见模式或 open(reuse_existing=true)。\
                           注:真实全屏窗口同样会命中,故只作提示"
            }));
        }
    }
    // ===================== 第 181 轮:体检面补齐(只观测的向量) =====================
    // 定级原则:**只有零假阳性的才进 HARD_CODES**。第 178 轮的教训是把推断当
    // 硬判据会长年误报(focus_visibility / languages_frozen 都栽过),故新增项里
    // 只有 stack 令牌与 document 级自动化遗留是「本机绝对不该出现」的硬证据,
    // 其余一律 warn,等真机基线钉死再决定是否升级。
    if fp.get("stack_automation_tokens").and_then(Value::as_bool) == Some(true) {
        out.push(json!({
            "code": "stack_trace_automation_tokens",
            "advice": "Error().stack 命中自动化框架/驱动令牌(真实 Chrome 的 stack 只有页面/V8 帧,\
                       本工具经 CDP 驱动也不产生这些帧)—— 属零假阳性硬证据;\
                       处置:open(reuse_existing=true) 接管用户已登录的真实浏览器"
        }));
    }
    if let Some(leaked) = fp.get("document_automation_globals").and_then(Value::as_array) {
        if !leaked.is_empty() {
            out.push(json!({
                "code": "automation_doc_leak",
                "advice": format!(
                    "document 上存在 chromedriver 系遗留属性: {} —— 本工具经 CDP 驱动不产生这些键,\
                     属零假阳性硬证据;处置:open(reuse_existing=true) 接管用户已登录的真实浏览器",
                    leaked.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
                )
            }));
        }
    }
    // chrome 在场却缺三件套:真机 Object.keys(chrome) 恒为 [app,csi,loadTimes]。
    // 只在**明确在场且缺项**时告警(chrome_shape 为 null = chrome 本就不存在/探测失败,
    // fail-open 不告警)。
    if let Some(cs) = fp.get("chrome_shape").filter(|v| !v.is_null()) {
        let missing: Vec<&str> = vec![
            (!cs.get("app").and_then(Value::as_bool).unwrap_or(false)).then_some("app"),
            (!cs.get("csi").and_then(Value::as_bool).unwrap_or(false)).then_some("csi"),
            (!cs.get("load_times").and_then(Value::as_bool).unwrap_or(false)).then_some("loadTimes"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !missing.is_empty() {
            out.push(json!({
                "code": "chrome_api_incomplete",
                "advice": format!(
                    "window.chrome 在场却缺 {} —— 真机 Object.keys(chrome) 恒为 [app,csi,loadTimes],\
                     缺项即非真 Chrome 形态(无头/被降级环境);处置:确认注入的是最新 \
                     browser_ns::FIDELITY_JS ④(在场补全 + 缺失创建双分支);\
                     注:chrome.runtime 不在检查之列(真机全新 profile 本就没有)",
                    missing.join("/")
                )
            }));
        }
    }
    // defineProperty 打错目标:原生 webdriver 只挂在 Navigator.prototype,
    // 实例自有属性恒 undefined。非空即说明改到实例上,一行即可识破。
    if fp.get("webdriver_own_property").and_then(Value::as_bool) == Some(true) {
        out.push(json!({
            "code": "webdriver_own_property",
            "advice": "getOwnPropertyDescriptor(navigator,'webdriver') 非空 —— 原生该属性只挂在 \
                       Navigator.prototype 上,实例自有属性恒 undefined;命中说明覆写打错了目标\
                       (defineProperty 落到了实例),处置:确认注入的是最新 browser_ns::CORE_JS\
                       (覆写 Navigator.prototype)"
        }));
    }
    // 指针/悬停媒体查询:桌面 UA 下双 false 是无头典型读数。**只观测**——
    // 是否属「无头误报」需真机四档基线确认后再决定是否回填(35 §六.1)。
    if let Some(mq) = fp.get("pointer_mq").filter(|v| !v.is_null()) {
        let hover = mq.get("hover").and_then(Value::as_bool);
        let fine = mq.get("fine").and_then(Value::as_bool);
        let desktop_ua = {
            let ua = fp
                .get("platform_ua")
                .and_then(|p| p.get("ua"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let lower = ua.to_ascii_lowercase();
            !ua.is_empty()
                && !lower.contains("android")
                && !lower.contains("iphone")
                && !lower.contains("ipad")
                && !lower.contains("mobile")
        };
        if desktop_ua && hover == Some(false) && fine == Some(false) {
            out.push(json!({
                "code": "pointer_media_headless",
                "advice": "桌面 UA 下 (hover:hover) 与 (pointer:fine) 双双 false —— 无头环境典型读数\
                           (真实桌面 Chrome 恒有鼠标指针);本轮只观测不回填,待真机基线确认;\
                           关键站点优先 headed 可见模式或 open(reuse_existing=true)"
            }));
        }
    }
    if let Some(cd) = fp.get("color_depth").filter(|v| !v.is_null()) {
        let c = cd.get("color").and_then(Value::as_u64);
        let p = cd.get("pixel").and_then(Value::as_u64);
        if c.is_some() && p.is_some() && c != p {
            out.push(json!({
                "code": "color_depth_mismatch",
                "advice": format!(
                    "screen.colorDepth({}) 与 screen.pixelDepth({}) 不一致 —— 真实浏览器二者恒等,\
                     命中说明视口/设备度量覆盖把其中一个改坏了;本轮只观测",
                    c.map(|v| v.to_string()).unwrap_or_default(),
                    p.map(|v| v.to_string()).unwrap_or_default()
                )
            }));
        }
    }
    if let Some(so) = fp.get("screen_orientation").filter(|v| !v.is_null()) {
        if so.get("present").and_then(Value::as_bool) == Some(false) {
            out.push(json!({
                "code": "screen_orientation_missing",
                "advice": "screen.orientation 缺失或无 type —— 现代 Chrome 恒有;\
                           本轮只观测,待真机基线确认(部分无头档确实不暴露)"
            }));
        }
    }
    if let Some(hw) = fp.get("hardware_params").filter(|v| !v.is_null()) {
        let hc = hw.get("hc").and_then(Value::as_u64).unwrap_or(0);
        let dm = hw.get("dm").and_then(Value::as_u64);
        if hc == 0 || dm == Some(0) {
            out.push(json!({
                "code": "hw_params_unset",
                "advice": format!(
                    "hardwareConcurrency={} deviceMemory={} —— 真机恒 ≥1,为 0 是物理不可能;\
                     命中说明环境被降级/字段被抹;本轮只观测",
                    hc,
                    dm.map(|v| v.to_string()).unwrap_or_else(|| "null".into())
                )
            }));
        }
    }
    // platform 与 UA 声明的 OS 交叉不一致(如 platform=Linux 而 UA 写 Windows NT)。
    if let Some(pu) = fp.get("platform_ua").filter(|v| !v.is_null()) {
        let plat = pu.get("platform").and_then(Value::as_str).unwrap_or("");
        let ua = pu.get("ua").and_then(Value::as_str).unwrap_or("");
        let mismatch = (!plat.is_empty() && !ua.is_empty()) && {
            let plat_l = plat.to_ascii_lowercase();
            let ua_l = ua.to_ascii_lowercase();
            // navigator.platform 的合法取值只有 Win32/Win64/MacIntel/MacPPC/Linux …
            // 用 starts_with 而非 contains:`darwin` 里也含 "win",contains 会误判。
            let plat_win = plat_l.starts_with("win");
            let plat_mac = plat_l.starts_with("mac");
            let plat_linux = plat_l.starts_with("linux") && !plat_l.contains("android");
            let ua_win = ua_l.contains("windows nt");
            let ua_mac = ua_l.contains("macintosh") || ua_l.contains("mac os x");
            let ua_linux = ua_l.contains("linux") && !ua_l.contains("android");
            (plat_win && !ua_win) || (plat_mac && !ua_mac) || (plat_linux && !ua_linux)
        };
        if mismatch {
            out.push(json!({
                "code": "platform_ua_mismatch",
                "advice": format!(
                    "navigator.platform({plat}) 与 UA 声明的 OS 不一致 —— 真实浏览器二者自洽,\
                     命中说明 UA/Client Hints 覆盖与 platform 只改了一侧;\
                     处置:确认 UA 去令牌走 browser_ns::apply_ua_destealth(只滤品牌,不动 platform)"
                )
            }));
        }
    }
    out
}

/// 硬判据集合：命中任一即 `risk_tier=high`（建议直接改道 `open(reuse_existing=true)`
/// 接管用户已登录的真实浏览器，而不是继续在拉起环境上调参）。
///
/// 第 178 轮：`focus_visibility_mismatch` 已**移出**该集合（真机四档实测证伪，
/// 见 `classify` 末尾注释）。
///
/// 第 181 轮新增两条 —— 只收「零假阳性」的硬证据（stack 令牌 / document 级
/// 自动化遗留）；同批新增的 `chrome_api_incomplete` / `pointer_media_headless` 等
/// 一律先停在 `warn`，等真机基线钉死再决定是否升级（178 轮教训：把推断当硬判据
/// 会长年误报）。
const HARD_CODES: &[&str] = &[
    "webdriver_true",
    "headless_ua",
    "webgl_default_strings",
    "automation_global_leak",
    "screen_geometry_impossible",
    "screen_headless_default",
    "plugin_shape_mismatch",
    "stack_trace_automation_tokens",
    "automation_doc_leak",
];

/// 「我们自己制造的破绽」类告警 —— 这些只反映**注入形态是否正确**，
/// 与环境是否像真浏览器无关。在**保真档**下命中属该档语义（它本来就不改函数身份），
/// 不应计入 `risk_tier`；在**侵入式档**下命中才是登记表漏网的真故障。
const MODE_DESIGN_CODES: &[&str] = &[
    "patched_functions_visible",
    "patched_function_source_visible",
];

/// 环境风险档（`ok` / `warn` / `high`）——把一长串 `suspicious` 收敛成**一个可执行信号**
/// 给 Agent：high 就别在拉起环境上继续调参了，直接换真实浏览器指纹。
pub fn risk_tier(susp: &[Value]) -> &'static str {
    if susp.is_empty() {
        return "ok";
    }
    let hard = susp.iter().any(|s| {
        s.get("code")
            .and_then(Value::as_str)
            .is_some_and(|c| HARD_CODES.contains(&c))
    });
    if hard {
        "high"
    } else {
        "warn"
    }
}

/// 该告警码是否属于「档位设计取舍」而非环境缺陷（见 [`MODE_DESIGN_CODES`]）。
pub fn is_mode_design_code(code: &str) -> bool {
    MODE_DESIGN_CODES.contains(&code)
}

/// 组装完整 `data.fingerprint` 响应段(健康环境也返回,供人工/QC 对账)。
///
/// 第 181 轮起接收**当前生效反检测档位**做口径分流（见 35 §2.3）：
/// - **保真档**：`patched_*` 两类告警是该档语义（本就不改函数身份），标
///   `by_design: true` 且**不计入 `risk_tier`** —— 否则非侵入式档常年被它们抬成
///   `warn`，真正值得看的 `webgl_default_strings` / `automation_global_leak` 反被淹没；
/// - **侵入式档**：同类命中是登记表漏网的真故障，正常计入；
/// - **关闭档**：全部照常计入（排障对照要看全貌）。
pub fn build(fp: &Value, mode: crate::agent::stealth::StealthMode) -> Value {
    use crate::agent::stealth::StealthMode;
    let mut susp = classify(fp);
    let exclude_design = mode == StealthMode::Fidelity;
    if exclude_design {
        for s in susp.iter_mut() {
            if s.get("code")
                .and_then(Value::as_str)
                .is_some_and(is_mode_design_code)
            {
                if let Some(obj) = s.as_object_mut() {
                    obj.insert("by_design".to_string(), json!(true));
                }
            }
        }
    }
    let tier_src: Vec<Value> = if exclude_design {
        susp.iter()
            .filter(|s| {
                !s.get("code")
                    .and_then(Value::as_str)
                    .is_some_and(is_mode_design_code)
            })
            .cloned()
            .collect()
    } else {
        susp.clone()
    };
    let tier = risk_tier(&tier_src);
    json!({
        "risk_tier": tier,
        "mode_design_excluded": exclude_design,
        "webdriver": fp.get("webdriver").cloned().unwrap_or(Value::Null),
        "webgl": fp.get("webgl").cloned().unwrap_or(Value::Null),
        "plugins": fp.get("plugins").cloned().unwrap_or(Value::Null),
        "languages": fp.get("languages").cloned().unwrap_or(Value::Null),
        "chrome_runtime": fp.get("chrome_runtime").cloned().unwrap_or(Value::Null),
        "screen": fp.get("screen").cloned().unwrap_or(Value::Null),
        "timezone": fp.get("timezone").cloned().unwrap_or(Value::Null),
        "ua_brand": fp.get("ua_brand").cloned().unwrap_or(Value::Null),
        "geometry": fp.get("geometry").cloned().unwrap_or(Value::Null),
        "plugin_shape_ok": fp.get("plugin_shape_ok").cloned().unwrap_or(Value::Null),
        "languages_frozen": fp.get("languages_frozen").cloned().unwrap_or(Value::Null),
        "pdf_viewer_enabled": fp.get("pdf_viewer_enabled").cloned().unwrap_or(Value::Null),
        "iterator_self_iterable": fp.get("iterator_self_iterable").cloned().unwrap_or(Value::Null),
        "automation_globals": fp.get("automation_globals").cloned().unwrap_or(Value::Null),
        "patched_fn_leak": fp.get("patched_fn_leak").cloned().unwrap_or(Value::Null),
        "focus_visibility": fp.get("focus_visibility").cloned().unwrap_or(Value::Null),
        "injected_ns_keys": fp.get("injected_ns_keys").cloned().unwrap_or(Value::Null),
        "injected_ns_symbols": fp.get("injected_ns_symbols").cloned().unwrap_or(Value::Null),
        "ns_symbol_present": fp.get("ns_symbol_present").cloned().unwrap_or(Value::Null),
        // 第 181 轮:体检面补齐(只观测项,原值回报供人工/QC 对账)
        "stack_automation_tokens": fp.get("stack_automation_tokens").cloned().unwrap_or(Value::Null),
        "document_automation_globals": fp.get("document_automation_globals").cloned().unwrap_or(Value::Null),
        "chrome_shape": fp.get("chrome_shape").cloned().unwrap_or(Value::Null),
        "webdriver_own_property": fp.get("webdriver_own_property").cloned().unwrap_or(Value::Null),
        "pointer_mq": fp.get("pointer_mq").cloned().unwrap_or(Value::Null),
        "color_depth": fp.get("color_depth").cloned().unwrap_or(Value::Null),
        "screen_orientation": fp.get("screen_orientation").cloned().unwrap_or(Value::Null),
        "hardware_params": fp.get("hardware_params").cloned().unwrap_or(Value::Null),
        "platform_ua": fp.get("platform_ua").cloned().unwrap_or(Value::Null),
        "suspicious": susp,
        "note": "risk_tier=high 表示环境在风控眼里已是硬判据级别,继续在本环境调参收益极低,\
                 应直接 open(reuse_existing=true) 接管用户已登录的真实浏览器;\
                 warn=有可升档/可复核项;suspicious 非空不代表一定被拦,但站点风控采这些字段加权很高。\
                 第 179 轮起注入层窗口面 API 收敛到一个 symbol 命名空间对象:\
                 injected_ns_keys 应为 0(非零=渲染链路倒退,命中会给出 injected_ns_string_keys),\
                 injected_ns_symbols 应为 1、ns_symbol_present 应为 true(裸环境/stealth=off 时为 0/false)。\
                 focus_visibility 自第 178 轮起只回报不判定(真机复核:真实用户窗口被遮挡时同样是 visible+无焦点)。\
                 第 181 轮:体检面扩到 20 项(见 docs/MCP_Web_Use/35);其中 stack_trace_automation_tokens 与 \
                 automation_doc_leak 是零假阳性硬证据,其余新增项一律只观测不回填。\
                 mode_design_excluded=true(保真档)时 patched_* 两类属该档设计取舍,标 by_design 且不计入 risk_tier"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::stealth::StealthMode;

    fn fp(webgl: Value, extra: Value) -> Value {
        let mut v = json!({
            "webdriver": false,
            "webgl": webgl,
            "plugins": 5,
            "languages": 2,
            "chrome_runtime": true,
            "notification_permission": "default",
            // 第 174 轮新增项:健康基线(缺测一律 fail-open 不告警)
            "ua_headless": false,
            "webgl_parameter_enumerable": true,
            "patched_functions_native": true
        });
        if let (Some(dst), Some(src)) = (v.as_object_mut(), extra.as_object()) {
            for (k, val) in src {
                dst.insert(k.clone(), val.clone());
            }
        }
        v
    }

    fn codes(v: &Value) -> Vec<String> {
        classify(v)
            .iter()
            .filter_map(|x| x["code"].as_str().map(str::to_string))
            .collect()
    }

    #[test]
    fn 真实环境零告警() {
        let v = fp(
            json!({"ok": true, "unmasked": true,
                   "vendor": "Google Inc. (Apple)",
                   "renderer": "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified)"}),
            json!({}),
        );
        assert!(codes(&v).is_empty(), "真实指纹不应告警:{:?}", codes(&v));
    }

    #[test]
    fn webgl兜底串命中() {
        let v = fp(
            json!({"ok": true, "unmasked": true,
                   "vendor": "WebKit", "renderer": "WebKit WebGL"}),
            json!({}),
        );
        assert_eq!(codes(&v), vec!["webgl_default_strings".to_string()]);
    }

    #[test]
    fn 扩展缺失命中() {
        let v = fp(json!({"ok": true, "unmasked": false, "vendor": "WebKit", "renderer": "WebKit WebGL"}), json!({}));
        assert_eq!(codes(&v), vec!["webgl_debug_renderer_info_missing".to_string()]);
    }

    #[test]
    fn 多项异常同时命中() {
        let v = fp(
            json!({"ok": false}),
            json!({"webdriver": true, "plugins": 0, "languages": 0,
                   "chrome_runtime": false, "notification_permission": "denied"}),
        );
        let c = codes(&v);
        for expect in [
            "webgl_debug_renderer_info_missing",
            "webdriver_true",
            "no_plugins",
            "no_languages",
            "notification_denied",
        ] {
            assert!(c.contains(&expect.to_string()), "缺 {expect}:{c:?}");
        }
    }

    #[test]
    fn 反检测档位自证项() {
        // 无头 UA 令牌 + 描述符被改 + 函数身份可见
        let v = fp(
            json!({"ok": true, "unmasked": true,
                   "vendor": "Google Inc. (Apple)",
                   "renderer": "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified)"}),
            json!({"ua_headless": true, "webgl_parameter_enumerable": false,
                   "patched_functions_native": false}),
        );
        let c = codes(&v);
        assert!(c.contains(&"headless_ua".to_string()), "{c:?}");
        assert!(c.contains(&"webgl_descriptor_tampered".to_string()), "{c:?}");
        assert!(c.contains(&"patched_functions_visible".to_string()), "{c:?}");
        // 缺测 fail-open(不因探测表达式故障误报)
        let missing = json!({"webdriver": false, "plugins": 5, "languages": 2, "chrome_runtime": true,
                             "notification_permission": "default",
                             "webgl": {"ok": true, "unmasked": true,
                                       "vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple)"}});
        assert!(codes(&missing).is_empty(), "缺测不得告警:{:?}", codes(&missing));
    }

    #[test]
    fn 屏幕几何不自洽命中() {
        // 真机实测的坏形态:window.outer=1920x1080 但 screen 只有 800x600
        // (视口覆盖没带 screen 声明,屏幕停在 Chrome 无头默认值)
        let v = fp(
            json!({"ok": true, "unmasked": true,
                   "vendor": "Google Inc. (Intel)",
                   "renderer": "ANGLE (Intel, Intel(R) UHD Graphics, D3D11)"}),
            json!({"geometry": {"screen": "800x600", "window_outer": "1920x1080",
                                "outer_gt_screen": true, "screen_is_headless_default": true}}),
        );
        let c = codes(&v);
        // 两条信号强弱有别:物理不可能(outer>screen)是硬判据,优先报出并
        // **不再叠加**较弱的「screen 恰为无头默认值」,避免同一根因刷两条告警。
        assert_eq!(c, vec!["screen_geometry_impossible".to_string()], "{c:?}");
        // 仅 screen 落在无头默认值、但未越界(弱信号单独成码)
        let weak = fp(
            json!({"ok": true, "unmasked": true,
                   "vendor": "Google Inc. (Intel)",
                   "renderer": "ANGLE (Intel, Intel(R) UHD Graphics, D3D11)"}),
            json!({"geometry": {"screen": "800x600", "window_outer": "800x600",
                                "outer_gt_screen": false, "screen_is_headless_default": true}}),
        );
        assert!(codes(&weak).contains(&"screen_headless_default".to_string()), "{:?}", codes(&weak));
        // 自洽形态(补过 screen 声明)不得告警
        let ok = fp(
            json!({"ok": true, "unmasked": true,
                   "vendor": "Google Inc. (Intel)",
                   "renderer": "ANGLE (Intel, Intel(R) UHD Graphics, D3D11)"}),
            json!({"geometry": {"screen": "1920x1080", "window_outer": "1920x1080",
                                "outer_gt_screen": false, "screen_is_headless_default": false}}),
        );
        assert!(codes(&ok).is_empty(), "几何自洽不得告警:{:?}", codes(&ok));
        // DPI 缩放残余命中(outer 略大但 screen 非无头默认值)不告警
        let dpi = fp(
            json!({"ok": true, "unmasked": true,
                   "vendor": "Google Inc. (Intel)",
                   "renderer": "ANGLE (Intel, Intel(R) UHD Graphics, D3D11)"}),
            json!({"geometry": {"screen": "1907x1440", "window_outer": "1922x1082",
                                "outer_gt_screen": true, "screen_is_headless_default": false}}),
        );
        assert!(codes(&dpi).is_empty(), "DPI 缩放假象不得告警:{:?}", codes(&dpi));
        // 缺测 fail-open
        let missing = fp(json!({"ok": true, "unmasked": true,
                                "vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple)"}), json!({}));
        assert!(codes(&missing).is_empty(), "缺 geometry 不得告警:{:?}", codes(&missing));
    }

    #[test]
    fn 插件形态失配命中() {
        let base = json!({"ok": true, "unmasked": true,
                          "vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple)"});
        // 三种失配各命中一次
        for bad in [
            json!({"tag": "[object Object]", "inst": false, "named_item_ok": true}),
            json!({"tag": "[object PluginArray]", "inst": false, "named_item_ok": true}),
            json!({"tag": "[object PluginArray]", "inst": true, "named_item_ok": false}),
        ] {
            let v = fp(base.clone(), json!({"plugin_shape_ok": bad}));
            assert!(
                codes(&v).contains(&"plugin_shape_mismatch".to_string()),
                "{bad} 应命中,实际 {:?}",
                codes(&v)
            );
        }
        // 形态正确 → 不告警
        let good = fp(base.clone(), json!({"plugin_shape_ok": {
            "tag": "[object PluginArray]", "inst": true, "named_item_ok": true }}));
        assert!(codes(&good).is_empty(), "形态正确不得告警:{:?}", codes(&good));
        // 插件本就为空(未触发回填)→ 不判定,fail-open
        let empty = fp(base, json!({"plugin_shape_ok": serde_json::Value::Null}));
        assert!(codes(&empty).is_empty(), "plugins 为空不得判定形态:{:?}", codes(&empty));
    }

    #[test]
    fn chrome运行时恒不告警() {
        // 第 175 轮真机实测:无扩展的真实 Chrome 就没有 chrome.runtime,
        // 故无论读到什么都不得判为可疑(否则会把每台合规浏览器都标红)。
        for rt in [json!(false), json!(true)] {
            let v = fp(json!({"ok": true, "unmasked": true,
                              "vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple)"}),
                       json!({"chrome_runtime": rt.clone()}));
            assert!(
                !codes(&v).contains(&"chrome_runtime".to_string()),
                "chrome_runtime={rt} 不得告警:{:?}",
                codes(&v)
            );
        }
    }

    #[test]
    fn parse_与build() {
        let raw = r#"{"webdriver":false,"webgl":{"ok":true,"unmasked":true,"vendor":"Google Inc. (Apple)","renderer":"ANGLE (Apple)"},"plugins":5,"languages":2,"chrome_runtime":true}"#;
        let v = parse(raw).unwrap();
        let b = build(&v, StealthMode::Hardened);
        assert_eq!(b["webdriver"], json!(false));
        assert!(b["suspicious"].as_array().unwrap().is_empty());
        assert!(b["note"].as_str().unwrap().contains("reuse_existing"));
        assert!(parse("garbage").is_none());
    }

    // ===================== 第 176 轮:languages/迭代器形态自证 =====================

    #[test]
    fn 语言列表非冻结才命中告警() {
        let base = json!({"ok": true, "unmasked": true,
                          "vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple)"});
        // 原生形态(冻结)→ 不告警。第 178 轮真机实测:裸环境也是 true,
        // 故「冻结」是正常态,按第 176 轮的旧逻辑会在每台真机上恒告警。
        let ok = fp(base.clone(), json!({"languages_frozen": true}));
        assert!(
            !codes(&ok).contains(&"languages_frozen".to_string()),
            "原生冻结形态不得告警:{:?}",
            codes(&ok)
        );
        assert!(
            !codes(&ok).contains(&"languages_not_frozen".to_string()),
            "{:?}",
            codes(&ok)
        );
        // 非冻结 → 命中(注入把 FrozenArray 改坏了)
        let v = fp(base.clone(), json!({"languages_frozen": false}));
        assert!(
            codes(&v).contains(&"languages_not_frozen".to_string()),
            "非冻结应命中:{:?}",
            codes(&v)
        );
        // 缺测 fail-open
        let missing = fp(base, json!({}));
        assert!(
            !codes(&missing)
                .iter()
                .any(|c| c.starts_with("languages")),
            "缺测不得告警:{:?}",
            codes(&missing)
        );
    }

    /// 第 178 轮:PDF 查看器开关与插件表自洽 —— 插件非空却报「不可用」是自造矛盾。
    #[test]
    fn pdf查看器开关与插件表一致性自证() {
        let base = json!({"ok": true, "unmasked": true,
                          "vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple)"});
        // 插件非空 + pdfViewerEnabled=false → 命中
        let bad = fp(base.clone(), json!({"plugins": 5, "pdf_viewer_enabled": false}));
        assert!(
            codes(&bad).contains(&"pdf_viewer_plugin_mismatch".to_string()),
            "{:?}",
            codes(&bad)
        );
        // 一致(真机常态:plugins=5 + true)→ 不告警
        let ok = fp(base.clone(), json!({"plugins": 5, "pdf_viewer_enabled": true}));
        assert!(
            !codes(&ok).contains(&"pdf_viewer_plugin_mismatch".to_string()),
            "{:?}",
            codes(&ok)
        );
        // 插件为空(回填未触发)时不做判定,fail-open
        let empty = fp(base, json!({"plugins": 0, "pdf_viewer_enabled": false}));
        assert!(
            !codes(&empty).contains(&"pdf_viewer_plugin_mismatch".to_string()),
            "{:?}",
            codes(&empty)
        );
    }

    #[test]
    fn 迭代器不自反命中告警() {
        let base = json!({"ok": true, "unmasked": true,
                          "vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple)"});
        // iterator_self_iterable='undefined'(裸对象没补自反)→ 命中
        let v = fp(base.clone(), json!({"iterator_self_iterable": "undefined"}));
        assert!(
            codes(&v).contains(&"iterator_not_self_iterable".to_string()),
            "迭代器不自反应命中:{:?}",
            codes(&v)
        );
        // 修复后 'function' → 不告警
        let ok = fp(base.clone(), json!({"iterator_self_iterable": "function"}));
        assert!(
            !codes(&ok).contains(&"iterator_not_self_iterable".to_string()),
            "自反后不得告警:{:?}",
            codes(&ok)
        );
        // 缺测(null = 插件本就空)→ fail-open 不告警
        let missing = fp(base, json!({}));
        assert!(
            !codes(&missing).contains(&"iterator_not_self_iterable".to_string()),
            "缺测不得告警:{:?}",
            codes(&missing)
        );
    }

    #[test]
    fn 自证项回报到build() {
        let raw = r#"{"languages_frozen":false,"iterator_self_iterable":"function"}"#;
        let v = parse(raw).unwrap();
        let b = build(&v, StealthMode::Hardened);
        assert_eq!(b["languages_frozen"], json!(false));
        assert_eq!(b["iterator_self_iterable"], json!("function"));
    }

    // ===================== 第 177 轮:漏检与自造破绽自证 =====================

    fn healthy_base() -> Value {
        json!({"ok": true, "unmasked": true,
               "vendor": "Google Inc. (Apple)",
               "renderer": "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified)"})
    }

    /// 第三方自动化框架的全局泄漏是**硬判据**:我们经 CDP 驱动不产生 `cdc_*` /
    /// `__selenium*` / `__playwright*` 这类键,命中即说明页面上还有别的自动化痕迹。
    #[test]
    fn 自动化全局泄漏命中且给出具体键名() {
        let v = fp(
            healthy_base(),
            json!({"automation_globals": ["cdc_adoQpoasnfa76pfcZLmcfl_Array", "domAutomation"]}),
        );
        let c = classify(&v);
        let item = c
            .iter()
            .find(|x| x["code"] == "automation_global_leak")
            .expect("非空全局必须命中");
        let advice = item["advice"].as_str().unwrap();
        assert!(advice.contains("cdc_adoQpoasnfa76pfcZLmcfl_Array"), "须回报具体键名:{advice}");
        assert!(advice.contains("reuse_existing"), "须给出改道建议:{advice}");
        // 空数组(干净环境)不得告警
        let clean = fp(healthy_base(), json!({"automation_globals": []}));
        assert!(!codes(&clean).contains(&"automation_global_leak".to_string()));
        // 缺测 fail-open
        assert!(!codes(&fp(healthy_base(), json!({})))
            .contains(&"automation_global_leak".to_string()));
    }

    /// 我们改写过的函数仍能被 toString 读到源码 = 压制手法本身成为签名。
    /// 保真档下这是该档语义(它压根不改函数身份),措辞必须落在「可升档」而非「故障」。
    #[test]
    fn 改写函数源码可见命中() {
        let v = fp(
            healthy_base(),
            json!({"patched_fn_leak": ["permissions_query", "plugins_iter_next"]}),
        );
        let c = classify(&v);
        let item = c
            .iter()
            .find(|x| x["code"] == "patched_function_source_visible")
            .expect("非空漏登记必须命中");
        let advice = item["advice"].as_str().unwrap();
        assert!(advice.contains("permissions_query"), "须回报具体入口:{advice}");
        assert!(advice.contains("保真档"), "保真档下这是语义而非故障:{advice}");
        // 空数组(侵入档登记完整)不得告警
        let ok = fp(healthy_base(), json!({"patched_fn_leak": []}));
        assert!(!codes(&ok).contains(&"patched_function_source_visible".to_string()));
    }

    /// 第 178 轮:**焦点/可见性自洽退出告警面**。第 177 轮把它当硬判据
    /// (命中即 risk_tier=high),真机四档实测证伪:
    /// - 有头档 `has_focus=false`(缺省 `LAEW_WEB_FOCUS=keep` 主动归还键盘焦点,
    ///   是我们自己的设计),无头档反而 `has_focus=true`;
    /// - 真实用户窗口被别的窗口遮挡(未最小化)时同样是 visible + 无焦点。
    /// 故字段只回报不判定 —— 任何组合都不得产生 suspicious,更不得抬 risk_tier。
    #[test]
    fn 焦点可见性只回报不判定() {
        let base = json!({"ok": true, "unmasked": true,
                          "vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple)"});
        for fv in [
            json!({"visibility": "visible", "has_focus": false}),
            json!({"visibility": "visible", "has_focus": true}),
            json!({"visibility": "hidden", "has_focus": false}),
            json!({"visibility": "hidden", "has_focus": true}),
        ] {
            let v = fp(base.clone(), json!({"focus_visibility": fv.clone()}));
            assert!(
                codes(&v).is_empty(),
                "{fv} 不得产生任何告警:{:?}",
                codes(&v)
            );
        }
        // 硬判据集合里也不得再出现该码
        assert!(
            !HARD_CODES.contains(&"focus_visibility_mismatch"),
            "焦点可见性判据已真机证伪,不得留在 HARD_CODES"
        );
        // 字段本身仍要回报(供人工/QC 诊断)
        let b = build(&fp(base, json!({"focus_visibility": {"visibility": "visible", "has_focus": false}})), StealthMode::Hardened);
        assert_eq!(b["focus_visibility"]["has_focus"], json!(false));
    }

    /// `risk_tier` 把一长串 suspicious 收敛成**一个可执行信号**:high = 别在拉起
    /// 环境上继续调参了,直接换真实浏览器指纹。软项(如保真档的源码可见)只到 warn。
    #[test]
    fn 风险档三档收敛() {
        assert_eq!(risk_tier(&[]), "ok");
        // 软项 → warn
        let soft = classify(&fp(
            healthy_base(),
            json!({"patched_fn_leak": ["permissions_query"]}),
        ));
        assert_eq!(risk_tier(&soft), "warn", "{:?}", codes(&fp(healthy_base(), json!({"patched_fn_leak": ["permissions_query"]}))));
        // 硬项 → high
        let hard = classify(&fp(
            healthy_base(),
            json!({"automation_globals": ["cdc_x"]}),
        ));
        assert_eq!(risk_tier(&hard), "high");
        // healthy_base 下 WebGL 是真串,不会额外触发 webgl_default_strings
        assert!(!codes(&fp(healthy_base(), json!({}))).contains(&"webgl_default_strings".to_string()));
    }

    /// `build()` 必须把新自证项与 `risk_tier` 一并回报,否则探针白采集。
    /// 第 178 轮:`focus_visibility` 退出判定面后,本条不再靠它抬 `risk_tier`。
    #[test]
    fn 新自证项回报到build() {
        let raw = r#"{"automation_globals":[],"patched_fn_leak":["permissions_query"],
            "focus_visibility":{"visibility":"visible","has_focus":false},
            "injected_ns_keys":0,"injected_ns_symbols":1,"ns_symbol_present":true,
            "pdf_viewer_enabled":true}"#;
        let b = build(&parse(raw).unwrap(), StealthMode::Hardened);
        assert_eq!(b["automation_globals"], json!([]));
        assert_eq!(b["patched_fn_leak"], json!(["permissions_query"]));
        assert_eq!(b["injected_ns_keys"], json!(0));
        assert_eq!(b["injected_ns_symbols"], json!(1));
        assert_eq!(b["ns_symbol_present"], json!(true));
        assert_eq!(b["pdf_viewer_enabled"], json!(true));
        assert_eq!(b["focus_visibility"]["has_focus"], json!(false));
        // 只余软项(改写函数源码可见)⇒ warn;不得因 focus_visibility 抬成 high
        assert_eq!(b["risk_tier"], "warn", "{b}");
        // 收敛后的干净基线:0 个字符串键 / 1 个符号键时**不得**新增告警
        assert!(!b["suspicious"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["code"] == "injected_ns_string_keys"));
        assert!(b["note"].as_str().unwrap().contains("injected_ns_symbols"));
        assert!(b["note"].as_str().unwrap().contains("focus_visibility"));
    }

    /// 第 179 轮:注入面收敛的回归判据 —— 字符串键非零即说明渲染链路倒退,
    /// 必须给出可行动建议(而不是"只观测")。
    #[test]
    fn 注入层字符串键泄漏命中() {
        let v = fp(healthy_base(), json!({"injected_ns_keys": 30, "injected_ns_symbols": 0}));
        let c = classify(&v);
        let item = c
            .iter()
            .find(|x| x["code"] == "injected_ns_string_keys")
            .expect("字符串键非零必须告警");
        let advice = item["advice"].as_str().unwrap();
        assert!(advice.contains("getOwnPropertyNames"), "须说明可枚举性:{advice}");
        assert!(advice.contains("render_ns"), "须指向根因与修法:{advice}");
        // 0 个字符串键(收敛后基线)不得告警
        assert!(!codes(&fp(healthy_base(), json!({"injected_ns_keys": 0})))
            .contains(&"injected_ns_string_keys".to_string()));
        // 缺测 fail-open
        assert!(!codes(&fp(healthy_base(), json!({})))
            .contains(&"injected_ns_string_keys".to_string()));
    }

    /// 第 179 轮:无头档窗口几何 signature —— outer === inner 只作软提示,
    /// 不得升级成 high(真实全屏窗口同样命中)。
    #[test]
    fn 窗口内外尺寸相等只作软提示() {
        let v = fp(
            healthy_base(),
            json!({"geometry": {"screen": "1920x1080", "window_outer": "1920x1080",
                                 "window_inner": "1920x1080", "outer_gt_screen": false,
                                 "screen_is_headless_default": false, "outer_equals_inner": true}}),
        );
        let c = classify(&v);
        assert!(c.iter().any(|x| x["code"] == "outer_equals_inner"));
        assert_eq!(risk_tier(&c), "warn", "只作软提示,不得升 high");
        // 有头窗口(outer > inner)不告警
        let v2 = fp(
            healthy_base(),
            json!({"geometry": {"screen": "1920x1080", "window_outer": "1920x1040",
                                 "window_inner": "1920x940", "outer_gt_screen": false,
                                 "screen_is_headless_default": false, "outer_equals_inner": false}}),
        );
        assert!(!codes(&v2).contains(&"outer_equals_inner".to_string()));
    }

    /// 第 178 轮:注入层 window 键面的计量正则必须撞得上**真实命名空间形态**
    /// (`inject_ns()` = `__lzn` + 8 hex)。旧正则 `^__lz[0-9a-f]{8}` 恒不匹配
    /// (第 5 字符是 `n`,不是 hex),导致该自证项自诞生起一直报 0 —— 假阴性比
    /// 没有这项更糟:它让人以为「几乎没有痕迹」。
    #[test]
    fn 命名空间键正则匹配真实形态() {
        let ns = crate::agent::browser_ns::inject_ns();
        assert!(ns.starts_with("__lzn"), "命名空间形态变化,请同步修正探针正则:{ns}");
        // 与 PROBE_JS 内保持一致的正则(用真 regex 引擎,不用手搓匹配)
        let fixed = regex::Regex::new(r"^__lzn[0-9a-f]{8}").unwrap();
        assert!(fixed.is_match(ns), "计量正则必须命中真实命名空间:{ns}");
        // 反例:旧正则(把 `n` 当 hex)必须**不**命中 —— 锁死这次修的是真缺陷
        let old = regex::Regex::new(r"^__lz[0-9a-f]{8}").unwrap();
        assert!(
            !old.is_match(ns),
            "旧正则应不命中(这正是它恒报 0 的原因):{ns}"
        );
        // 探针源码里必须已是修正后的形态(防回归)
        assert!(PROBE_JS.contains(r"/^__lzn[0-9a-f]{8}/"));
        assert!(!PROBE_JS.contains(r"/^__lz[0-9a-f]{8}/"));
    }

    // ===================== 第 181 轮:体检面补齐 =====================

    /// 新增探针必须真的在 `PROBE_JS` 里 —— 否则字段恒 null、告警恒不触发,
    /// 白采集(第 178 轮 `injected_ns_keys` 正则死自证的同类教训)。
    #[test]
    fn 新增探针全部在册() {
        for key in [
            "stack_automation_tokens",
            "document_automation_globals",
            "chrome_shape",
            "webdriver_own_property",
            "pointer_mq",
            "color_depth",
            "screen_orientation",
            "hardware_params",
            "platform_ua",
        ] {
            assert!(
                PROBE_JS.contains(key),
                "探针字段 {key} 必须出现在 PROBE_JS(否则恒 null 白采集)"
            );
        }
    }

    #[test]
    fn stack令牌是硬判据() {
        let v = fp(healthy_webgl(), json!({"stack_automation_tokens": true}));
        assert!(codes(&v).contains(&"stack_trace_automation_tokens".to_string()));
        assert_eq!(risk_tier(&classify(&v)), "high", "stack 令牌属零假阳性硬证据");
        // fail-open:缺测量(null)不得告警
        let v2 = fp(healthy_webgl(), json!({"stack_automation_tokens": null}));
        assert!(!codes(&v2).contains(&"stack_trace_automation_tokens".to_string()));
    }

    #[test]
    fn document级自动化遗留是硬判据() {
        let v = fp(
            healthy_webgl(),
            json!({"document_automation_globals": ["$cdc_ado9po7621af"]}),
        );
        assert!(codes(&v).contains(&"automation_doc_leak".to_string()));
        assert_eq!(risk_tier(&classify(&v)), "high");
        // 空数组 = 干净
        let v2 = fp(healthy_webgl(), json!({"document_automation_globals": []}));
        assert!(!codes(&v2).contains(&"automation_doc_leak".to_string()));
    }

    #[test]
    fn chrome三件套缺失命中但只warn() {
        // 在场但缺 loadTimes
        let v = fp(
            healthy_webgl(),
            json!({"chrome_shape": {"app": true, "csi": true, "load_times": false}}),
        );
        let c = codes(&v);
        assert!(c.contains(&"chrome_api_incomplete".to_string()), "{c:?}");
        assert_eq!(risk_tier(&classify(&v)), "warn", "真机基线钉死前一律不上硬判据");
        // 三件套齐全 = 干净
        let v2 = fp(
            healthy_webgl(),
            json!({"chrome_shape": {"app": true, "csi": true, "load_times": true}}),
        );
        assert!(!codes(&v2).contains(&"chrome_api_incomplete".to_string()));
        // chrome 本就不存在(null) = fail-open 不告警
        let v3 = fp(healthy_webgl(), json!({"chrome_shape": null}));
        assert!(!codes(&v3).contains(&"chrome_api_incomplete".to_string()));
    }

    #[test]
    fn webdriver落在实例自有属性即命中() {
        let v = fp(healthy_webgl(), json!({"webdriver_own_property": true}));
        assert!(codes(&v).contains(&"webdriver_own_property".to_string()));
        let v2 = fp(healthy_webgl(), json!({"webdriver_own_property": false}));
        assert!(!codes(&v2).contains(&"webdriver_own_property".to_string()));
        // 缺测量 fail-open
        let v3 = fp(healthy_webgl(), json!({}));
        assert!(!codes(&v3).contains(&"webdriver_own_property".to_string()));
    }

    #[test]
    fn 桌面UA下指针媒体双false命中() {
        let v = fp(
            healthy_webgl(),
            json!({
                "pointer_mq": {"hover": false, "fine": false},
                "platform_ua": {"platform": "Win32", "ua": "Mozilla/5.0 (Windows NT 10.0) Chrome/155.0.0.0"}
            }),
        );
        assert!(codes(&v).contains(&"pointer_media_headless".to_string()));
        // 移动 UA 不误伤
        let v2 = fp(
            healthy_webgl(),
            json!({
                "pointer_mq": {"hover": false, "fine": false},
                "platform_ua": {"platform": "Linux armv81", "ua": "Mozilla/5.0 (Linux; Android 14) Mobile Safari"}
            }),
        );
        assert!(!codes(&v2).contains(&"pointer_media_headless".to_string()));
        // 有鼠标 = 干净
        let v3 = fp(
            healthy_webgl(),
            json!({
                "pointer_mq": {"hover": true, "fine": true},
                "platform_ua": {"platform": "Win32", "ua": "Mozilla/5.0 (Windows NT 10.0) Chrome/155.0.0.0"}
            }),
        );
        assert!(!codes(&v3).contains(&"pointer_media_headless".to_string()));
    }

    #[test]
    fn 颜色深度不一致与方向缺失与硬件参数() {
        let v = fp(
            healthy_webgl(),
            json!({
                "color_depth": {"color": 24, "pixel": 30},
                "screen_orientation": {"present": false, "type": ""},
                "hardware_params": {"hc": 0, "dm": null}
            }),
        );
        let c = codes(&v);
        assert!(c.contains(&"color_depth_mismatch".to_string()), "{c:?}");
        assert!(c.contains(&"screen_orientation_missing".to_string()), "{c:?}");
        assert!(c.contains(&"hw_params_unset".to_string()), "{c:?}");
        // 一致 / 齐全 = 干净
        let v2 = fp(
            healthy_webgl(),
            json!({
                "color_depth": {"color": 24, "pixel": 24},
                "screen_orientation": {"present": true, "type": "landscape-primary"},
                "hardware_params": {"hc": 16, "dm": 8}
            }),
        );
        let c2 = codes(&v2);
        assert!(!c2.contains(&"color_depth_mismatch".to_string()), "{c2:?}");
        assert!(!c2.contains(&"screen_orientation_missing".to_string()), "{c2:?}");
        assert!(!c2.contains(&"hw_params_unset".to_string()), "{c2:?}");
    }

    #[test]
    fn platform与UA交叉不一致命中() {
        let v = fp(
            healthy_webgl(),
            json!({"platform_ua": {"platform": "Linux x86_64", "ua": "Mozilla/5.0 (Windows NT 10.0; Win64) Chrome/155"}}),
        );
        assert!(codes(&v).contains(&"platform_ua_mismatch".to_string()));
        // 自洽 = 干净
        let v2 = fp(
            healthy_webgl(),
            json!({"platform_ua": {"platform": "Win32", "ua": "Mozilla/5.0 (Windows NT 10.0; Win64) Chrome/155"}}),
        );
        assert!(!codes(&v2).contains(&"platform_ua_mismatch".to_string()));
    }

    /// 保真档下「改过可见」是该档设计取舍:标 `by_design` 且**不计入** risk_tier,
    /// 否则非侵入式档常年被它们抬成 warn,把真正值得看的硬信号淹掉。
    #[test]
    fn 保真档下改过可见属设计取舍不抬档() {
        let v = fp(healthy_webgl(), json!({"patched_fn_leak": ["permissions_query"]}));
        let hard = build(&v, StealthMode::Hardened);
        assert_eq!(hard["risk_tier"], "warn", "侵入式档同类命中是登记表漏网,应计入");
        assert_eq!(hard["mode_design_excluded"], json!(false));
        assert!(
            hard["suspicious"][0].get("by_design").is_none(),
            "侵入式档不该打 by_design 标"
        );

        let fid = build(&v, StealthMode::Fidelity);
        assert_eq!(
            fid["risk_tier"], "ok",
            "保真档下 patched_* 属设计取舍,不得抬 risk_tier:{fid}"
        );
        assert_eq!(fid["mode_design_excluded"], json!(true));
        assert_eq!(
            fid["suspicious"][0]["by_design"], json!(true),
            "仍要回报,但标明属设计取舍:{fid}"
        );

        // off 档:排障对照要看全貌,照常计入
        let off = build(&v, StealthMode::Off);
        assert_eq!(off["risk_tier"], "warn");
        assert_eq!(off["mode_design_excluded"], json!(false));
    }

    /// 保真档的口径分流**只**作用于设计取舍类告警,真正的环境硬缺陷照常升 high。
    #[test]
    fn 保真档仍会因真缺陷升high() {
        let v = fp(healthy_webgl(), json!({
            "patched_fn_leak": ["chrome_csi"],
            "stack_automation_tokens": true
        }));
        let fid = build(&v, StealthMode::Fidelity);
        assert_eq!(fid["risk_tier"], "high", "stack 令牌不受口径分流影响:{fid}");
    }

    /// 硬判据集合:只收零假阳性项。第 181 轮新增的两条之外,同批的
    /// `chrome_api_incomplete` / `pointer_media_headless` 等**不得**进集合
    /// (178 轮教训:把推断当硬判据会长年误报)。
    #[test]
    fn 硬判据集合只收零假阳性() {
        for c in [
            "stack_trace_automation_tokens",
            "automation_doc_leak",
            "webdriver_true",
            "headless_ua",
        ] {
            assert!(HARD_CODES.contains(&c), "{c} 应在硬判据集合");
        }
        for c in [
            "chrome_api_incomplete",
            "pointer_media_headless",
            "color_depth_mismatch",
            "screen_orientation_missing",
            "hw_params_unset",
            "platform_ua_mismatch",
            "webdriver_own_property",
            "patched_function_source_visible",
        ] {
            assert!(
                !HARD_CODES.contains(&c),
                "{c} 真机基线钉死前不得进硬判据集合"
            );
        }
    }

    /// chrome 自证入口必须覆盖三件套 —— 否则登记漏网在 data.fingerprint 里点不出名。
    #[test]
    fn chrome自证入口在册() {
        for key in ["chrome_csi", "chrome_loadTimes", "chrome_app_install"] {
            assert!(
                PROBE_JS.contains(key),
                "patched_fn_leak 自证入口 {key} 必须在册"
            );
        }
    }

    fn healthy_webgl() -> Value {
        json!({"ok": true, "unmasked": true,
               "vendor": "Google Inc. (Apple)",
               "renderer": "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified)"})
    }
}