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
    fn parse_与build() {
        let raw = r#"{"webdriver":false,"webgl":{"ok":true,"unmasked":true,"vendor":"Google Inc. (Apple)","renderer":"ANGLE (Apple)"},"plugins":5,"languages":2,"chrome_runtime":true}"#;
        let v = parse(raw).unwrap();
        let b = build(&v);
        assert_eq!(b["webdriver"], json!(false));
        assert!(b["suspicious"].as_array().unwrap().is_empty());
        assert!(b["note"].as_str().unwrap().contains("reuse_existing"));
        assert!(parse("garbage").is_none());
    }
}