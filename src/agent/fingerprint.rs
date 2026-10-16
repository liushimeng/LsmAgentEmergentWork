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
        return {
          screen: sw + 'x' + sh,
          window_outer: ow + 'x' + oh,
          outer_gt_screen: ow > sw * 1.05 || oh > sh * 1.05,
          screen_is_headless_default: sw === 800 && sh === 600
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
    // 第 176 轮:languages 回填形态自证。原生 WebIDL sequence<DOMString>
    // 返回**非 frozen**数组(Object.isFrozen===false);若为 true 说明我们 freeze 了
    // (严格模式 push 抛 TypeError,原生不抛)—— 零成本形态判据。
    languages_frozen: (() => {
      try { return Object.isFrozen(navigator.languages || []); } catch (e) { return null; }
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
    // 两项都是「我们自己制造的形态破绽」类信号,命中即说明对应根治逻辑失效。
    if fp.get("languages_frozen").and_then(Value::as_bool) == Some(true) {
        out.push(json!({
            "code": "languages_frozen",
            "advice": "navigator.languages 被 freeze(原生 WebIDL sequence 返回非 frozen 数组);\
                       严格模式 push 会抛 TypeError(原生不抛),是零成本形态判据;\
                       处置:确认注入的是最新 browser_ns::FIDELITY_JS(第 176 轮已去 freeze)"
        }));
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
    out
}

/// 组装完整 `data.fingerprint` 响应段(健康环境也返回,供人工/QC 对账)。
pub fn build(fp: &Value) -> Value {
    let susp = classify(fp);
    json!({
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
        "iterator_self_iterable": fp.get("iterator_self_iterable").cloned().unwrap_or(Value::Null),
        "suspicious": susp,
        "note": "suspicious 非空不代表一定被拦,但站点风控采这些字段加权很高;\
                 被拦时的首选改道是 open(reuse_existing=true) 接管用户已登录的真实浏览器"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let b = build(&v);
        assert_eq!(b["webdriver"], json!(false));
        assert!(b["suspicious"].as_array().unwrap().is_empty());
        assert!(b["note"].as_str().unwrap().contains("reuse_existing"));
        assert!(parse("garbage").is_none());
    }

    // ===================== 第 176 轮:languages/迭代器形态自证 =====================

    #[test]
    fn 语言列表冻结命中告警() {
        let base = json!({"ok": true, "unmasked": true,
                          "vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple)"});
        // languages_frozen=true → 命中(我们 freeze 了,原生不 freeze)
        let v = fp(base.clone(), json!({"languages_frozen": true}));
        assert!(
            codes(&v).contains(&"languages_frozen".to_string()),
            "languages_frozen=true 应命中:{:?}",
            codes(&v)
        );
        // 原生态 false → 不告警
        let ok = fp(base.clone(), json!({"languages_frozen": false}));
        assert!(
            !codes(&ok).contains(&"languages_frozen".to_string()),
            "原生非 frozen 不得告警:{:?}",
            codes(&ok)
        );
        // 缺测 fail-open
        let missing = fp(base, json!({}));
        assert!(
            !codes(&missing).contains(&"languages_frozen".to_string()),
            "缺测不得告警:{:?}",
            codes(&missing)
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
        let b = build(&v);
        assert_eq!(b["languages_frozen"], json!(false));
        assert_eq!(b["iterator_self_iterable"], json!("function"));
    }
}