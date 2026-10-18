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
  // 第 184 轮:子帧 realm 反查探针的载体 iframe —— **创建在前**(初始 about:blank
  // 文档随 appendChild 同步可用),测量放在返回对象**最后一个字段** —— 中间隔着
  // WebGL 上下文创建等重活,给 runForEveryFrame 的子帧注入留出时序余量。
  const xFrame = (() => {
    try {
      const f = document.createElement('iframe');
      f.setAttribute('srcdoc', '<html><body>i</body></html>');
      f.style.display = 'none';
      document.body.appendChild(f);
      return (f.contentWindow && f.contentWindow.Function) ? f : null;
    } catch (e) { return null; }
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
    })(),
    // ===================== 第 184 轮:子帧覆盖 / 显示器拓扑 =====================
    // ⑩ 显示器拓扑读数。`value` = screen.isExtended 页面读数;`own`/`proto` = 原生
    //    属性挂在哪个槽位;`md_marker` = 注入层放在 ns 命名空间对象上的 OS 真值
    //    标记(FIDELITY_JS ⑥ 只在 OS=多显示器时写入)—— md_marker 在场而 value
    //    仍为 false 即回填失效(is_extended_mismatch,处置失效自检)。
    screen_is_extended: (() => {
      try {
        const own = Object.getOwnPropertyDescriptor(screen, 'isExtended');
        const proto = own ? null : Object.getOwnPropertyDescriptor(Screen.prototype, 'isExtended');
        const v = screen.isExtended;
        let marker = false;
        try {
          marker = Object.getOwnPropertySymbols(window).some((s) => {
            try { const o = window[s]; return !!(o && o.MD === 1); } catch (e) { return false; }
          });
        } catch (e) {}
        return { value: (typeof v === 'boolean') ? v : null, own: !!own, proto: !!proto, md_marker: marker };
      } catch (e) { return null; }
    })(),
    // ⑪ 跨 realm toString 反查(本轮核心自证):借自建同源 iframe 的 toString 反查
    //    顶层被改写的三个入口(toString 自身 / webdriver getter / permissions.query),
    //    非 [native code] 即泄漏 —— 真实浏览器这些入口恒原生,零假阳性;修复后
    //    (runForEveryFrame + 跨 realm 路由)应恒空数组。测量放在最后(见文件头
    //    xFrame 注释)。保真档命中属该档语义(本就不改函数身份),交由 Rust 侧
    //    MODE_DESIGN_CODES 分流;侵入档命中 = 子帧覆盖或路由失效的真故障。
    crossrealm_tostring_leak: (() => {
      try {
        if (!xFrame) return null;
        const W = xFrame.contentWindow;
        const probe = (fn) => {
          try { return !/\[native code\]/.test(String(W.Function.prototype.toString.call(fn))); }
          catch (e) { return false; }
        };
        const sites = [];
        try { if (probe(Function.prototype.toString)) sites.push('toString'); } catch (e) {}
        try {
          const d = Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver');
          if (d && d.get && probe(d.get)) sites.push('webdriver_getter');
        } catch (e) {}
        try { if (probe(Permissions.prototype.query)) sites.push('permissions_query'); } catch (e) {}
        try { xFrame.remove(); } catch (e) {}
        return sites;
      } catch (e) { return null; }
    })()
  };
})())"#;

/// WebGL「自动化兜底串」判定(小写包含)。
///
/// 第 183 轮新增 `basic render driver` 令牌:WARP(Windows 高级光栅化平台)读数
/// `ANGLE (Microsoft, Microsoft Basic Render Driver …)` 不含 webkit/swiftshader/
/// software 任一旧令牌,旧判定既不回填也不告警地放行 —— 而它是无 GPU/虚拟机/
/// 被禁用显卡环境的典型读数(实测带 `--disable-gpu` 的无头档即报它,与本机真
/// Intel UHD 串形成硬件指纹差)。第 183 轮已从参数层根治(无头档不再强制软渲染),
/// 仍命中即说明真 GPU 初始化失败(VM/RDP/驱动问题)。
fn is_default_webgl(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    s.contains("webkit")
        || s.contains("swiftshader")
        || s.contains("software")
        || s.contains("basic render driver")
}

/// 桌面 UA 判定(第 182 轮抽出共用):指针媒体/触摸点判据只适用于桌面形态,
/// 移动 UA 下 `hover:none`/`pointer:coarse`/多点触摸是**正常读数**,不得误伤。
/// UA 为空(探针未采到)时返回 false —— fail-open 不猜测。
fn desktop_ua(fp: &Value) -> bool {
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
            "advice": "WebGL vendor/renderer 是软渲染/兜底串(WebKit/SwiftShader/software/WARP),\
                       与真实桌面 Chrome 的硬件串不符,是高风险指纹;\
                       处置:第 183 轮起无头档已不强制 --disable-gpu(真 GPU 与本机有头逐字节一致),\
                       仍命中通常说明真 GPU 初始化失败(VM/RDP/驱动被禁)或误开了\
                       LAEW_WEB_HEADLESS_SOFTRENDER=on;JS 侧保真回填仍在(browser_ns),\
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
    //
    // 第 182 轮:`notification_denied` **退出 suspicious**（降级为 build() 顶层观测字段）。
    // 真机双档实测：laew 启动参数集自带 `--deny-permission-prompts`（治理原生权限
    // 气泡），**有头/无头两档** Notification.permission 恒读 `denied` —— 这是我们自己
    // 启动参数造成的诚实状态，真实用户拒绝通知后读数完全相同，零判别力只剩噪声；
    // 且 FIDELITY_JS ⑤（第 176 轮精确映射）已保证 permissions.query 与之一致
    // （denied↔denied），经典「单侧 denied 矛盾组合」并不成立。字段保留在
    // build() 输出里供人工/QC 对账，但不进 suspicious、不抬 risk_tier。
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
    // 指针/悬停媒体查询:桌面 UA 下双 false 是无头典型读数。**第 182 轮真机双档
    // 基线钉死**:有头档四元(hover/any-hover/pointer/any-pointer)恒全 true,无头档
    // 裸读数恒全 false —— 零假阳性确认;处置已内置为无头档原生 CDP 仿真
    // (`browser_env_emulation::apply_pointer_env_emulation`,setEmulatedMedia+setTouchEmulationEnabled,
    // 零注入面),故本条保留为**处置失效自检**:launch 无头档仍命中即说明仿真没下发
    // (stealth=off 或 CDP 超时 fail-open);connect 模式命中则该用户环境本就如实。
    if let Some(mq) = fp.get("pointer_mq").filter(|v| !v.is_null()) {
        let hover = mq.get("hover").and_then(Value::as_bool);
        let fine = mq.get("fine").and_then(Value::as_bool);
        if desktop_ua(fp) && hover == Some(false) && fine == Some(false) {
            out.push(json!({
                "code": "pointer_media_headless",
                "advice": "桌面 UA 下 (hover:hover) 与 (pointer:fine) 双双 false —— 无头环境典型读数\
                           (第 182 轮真机基线:有头档恒全 true);处置已内置(无头档 setEmulatedMedia 仿真),\
                           仍命中说明仿真未生效(stealth=off / CDP 超时);connect 模式命中则该用户环境本就如实"
            }));
        }
    }
    // 第 182 轮:桌面 UA 下 `navigator.maxTouchPoints > 0` 指向无头环境(真机双档:
    // 有头档 0 / 无头档裸读数 1)。与指针媒体同批处置(setTouchEmulationEnabled(false)
    // 归零),本条同为处置失效自检;移动 UA 不误伤。
    if desktop_ua(fp) && fp.get("max_touch_points").and_then(Value::as_u64).unwrap_or(0) > 0 {
        out.push(json!({
            "code": "touch_points_headless",
            "advice": "桌面 UA 却报 maxTouchPoints>0 —— 真机基线有头档恒 0、无头档裸读数 1;\
                       处置已内置(无头档触摸仿真关闭),仍命中说明仿真未生效"
        }));
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
    // ===================== 第 184 轮:子帧覆盖 / 显示器拓扑 =====================
    // 跨 realm toString 反查:站点自建同源 iframe 借其 toString 反查顶层被改写函数,
    // 非 [native code] 即泄漏我们的注入源码。真浏览器这些入口恒原生 → 零假阳性。
    // 侵入档命中 = runForEveryFrame 未生效(老 Chrome 回退/CDP 失败)或跨 realm
    // 路由失效,属处置失效真故障;保真档命中属该档语义(本就不改函数身份),
    // 由 MODE_DESIGN_CODES 分流为 by_design。
    if let Some(leaked) = fp.get("crossrealm_tostring_leak").and_then(Value::as_array) {
        if !leaked.is_empty() {
            out.push(json!({
                "code": "crossrealm_tostring_leak",
                "advice": format!(
                    "这些被改写的入口能被**自建 iframe 的 toString** 反查到源码(子帧 realm 未被处置): {}\
                     —— 一行 JS 即识破函数被包装;处置:确认注入走 cdp_ext 的 runForEveryFrame \
                     路径(老 Chrome 回退时只剩顶层覆盖)且 HARDENED_JS 跨 realm 路由在位;\
                     保真档下这是该档语义(可 control(set_stealth) 升 hardened)",
                    leaked.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
                )
            }));
        }
    }
    // 显示器拓扑回填失效自检:注入层已声明 OS=多显示器(ns 对象 MD 标记在场)而页面
    // 仍读 false —— FIDELITY_JS ⑥ 没生效(槽位形态探测未命中/属性原生缺失/被覆盖)。
    // md_marker 不在场(OS 单显示器或未注入)时零判定 —— fail-open 不猜测。
    if let Some(se) = fp.get("screen_is_extended").filter(|v| !v.is_null()) {
        if se.get("md_marker").and_then(Value::as_bool) == Some(true)
            && se.get("value").and_then(Value::as_bool) == Some(false)
        {
            out.push(json!({
                "code": "is_extended_mismatch",
                "advice": "OS 实读为多显示器(注入层已声明真值)而页面 screen.isExtended 仍读 false\
                           —— 显示器拓扑回填未生效;真机基线:有头档双显示器读 true、无头档裸读数恒 false\
                           (38 号文 §二);处置:确认注入的是最新 browser_ns::FIDELITY_JS ⑥(同位同形回填)"
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
    // 第 184 轮:跨 realm 反查泄漏在保真档下同样是「本就不改函数身份」的档位语义。
    "crossrealm_tostring_leak",
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
        // 第 182 轮:观测字段(不进 suspicious、不抬 risk_tier,只供人工/QC 对账)
        "notification_permission": fp.get("notification_permission").cloned().unwrap_or(Value::Null),
        "max_touch_points": fp.get("max_touch_points").cloned().unwrap_or(Value::Null),
        // 第 184 轮:子帧覆盖 / 显示器拓扑观测字段
        "crossrealm_tostring_leak": fp.get("crossrealm_tostring_leak").cloned().unwrap_or(Value::Null),
        "screen_is_extended": fp.get("screen_is_extended").cloned().unwrap_or(Value::Null),
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
                 第 182 轮:无头档指针媒体四元与触摸点已有原生 CDP 仿真(pointer_media_headless/\
                 touch_points_headless 是处置失效自检);notification_permission 为纯观测字段 —— \
                 laew 启动参数(--deny-permission-prompts)下两档恒 denied,与真实用户拒绝通知的读数一致,\
                 不再计 suspicious。mode_design_excluded=true(保真档)时 patched_* 两类属该档设计取舍,\
                 标 by_design 且不计入 risk_tier"
    })
}

#[cfg(test)]
mod tests;
