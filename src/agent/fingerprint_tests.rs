//! 第 184 轮新增体检判据用例:跨 realm toString 反查泄漏 / 显示器拓扑回填失效自检。
//!
//! 放同级兄弟文件而非并入 `fingerprint.rs` 内联 tests —— 该文件已近 1800 行上限
//! (react_tests 同款模式,见 `agent/mod.rs` 声明处注释)。夹具刻意自带最小副本,
//! 不跨模块借用内联测试的私有 helper。

use serde_json::{json, Value};

/// 健康指纹基线(镜像 fingerprint::tests::fp 的最小集,缺测字段一律 fail-open)。
fn healthy_fp(extra: Value) -> Value {
    let mut v = json!({
        "webdriver": false,
        "webgl": {
            "ok": true, "unmasked": true,
            "vendor": "Google Inc. (Intel)",
            "renderer": "ANGLE (Intel, Intel(R) UHD Graphics, D3D11)"
        },
        "plugins": 5,
        "languages": 2,
        "chrome_runtime": false,
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
    crate::agent::fingerprint::classify(v)
        .iter()
        .filter_map(|x| x["code"].as_str().map(str::to_string))
        .collect()
}

/// 跨 realm 反查泄漏:命中(非空数组)即告警;空数组/缺测(null)零告警;
/// 不进 HARD_CODES(处置失效类与 pointer_media_headless 同级,不是环境硬判据)。
#[test]
fn 跨realm反查泄漏判定三分支() {
    use crate::agent::fingerprint::risk_tier;
    // ① 命中:三个入口点名回报
    let v = healthy_fp(json!({"crossrealm_tostring_leak": ["toString", "webdriver_getter"]}));
    assert!(codes(&v).contains(&"crossrealm_tostring_leak".to_string()));
    // ② 干净:空数组 = 修复后的常态,零告警
    let v2 = healthy_fp(json!({"crossrealm_tostring_leak": []}));
    assert!(!codes(&v2).contains(&"crossrealm_tostring_leak".to_string()));
    // ③ 缺测 fail-open:null(iframe 不可用/探测抛错)不告警
    let v3 = healthy_fp(json!({"crossrealm_tostring_leak": null}));
    assert!(!codes(&v3).contains(&"crossrealm_tostring_leak".to_string()));
    // 定级:不进 HARD_CODES —— 命中时 risk_tier 最高 warn,不是 high
    assert_eq!(risk_tier(&crate::agent::fingerprint::classify(&v)), "warn");
}

/// 保真档分流:crossrealm_tostring_leak 在 fidelity 下标 by_design 且不计入
/// risk_tier(该档本就不改函数身份);侵入档命中计入 —— 与 patched_* 两类同款。
#[test]
fn 跨realm泄漏保真档by_design() {
    use crate::agent::fingerprint::{build, risk_tier};
    use crate::agent::stealth::StealthMode;
    let fp = healthy_fp(json!({"crossrealm_tostring_leak": ["permissions_query"]}));

    let fid = build(&fp, StealthMode::Fidelity);
    let entry = fid["suspicious"]
        .as_array()
        .expect("suspicious 数组")
        .iter()
        .find(|s| s["code"] == json!("crossrealm_tostring_leak"))
        .expect("保真档仍应回报该条(可见性不丢)");
    assert_eq!(entry["by_design"], json!(true), "保真档标 by_design");
    // 不计入 risk_tier:排除后无其它告警 → ok
    let susp = fid["suspicious"].as_array().unwrap().clone();
    let kept: Vec<Value> = susp
        .iter()
        .filter(|s| !crate::agent::fingerprint::is_mode_design_code(
            s["code"].as_str().unwrap_or("")
        ))
        .cloned()
        .collect();
    assert_eq!(risk_tier(&kept), "ok", "保真档 by_design 不抬 risk_tier");

    let hard = build(&fp, StealthMode::Hardened);
    let entry2 = hard["suspicious"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["code"] == json!("crossrealm_tostring_leak"))
        .expect("侵入档照常回报");
    assert!(entry2.get("by_design").is_none(), "侵入档命中是真故障,不标 by_design");
    assert_eq!(hard["mode_design_excluded"], json!(false));
}

/// is_extended_mismatch:md_marker 在场(OS=多显示器真值已声明)而页面读 false
/// 才命中;单显示器(marker 不在场)或回填成功(value=true)零告警;缺测 fail-open。
#[test]
fn 显示器拓扑回填失效自检四分支() {
    // ① 失效:真值标记在场 + 页面读 false
    let v = healthy_fp(json!({"screen_is_extended": {"value": false, "own": false, "proto": true, "md_marker": true}}));
    assert!(codes(&v).contains(&"is_extended_mismatch".to_string()));
    // ② 回填成功:marker 在场 + 页面读 true(FIDELITY_JS ⑥ 已生效)
    let v2 = healthy_fp(json!({"screen_is_extended": {"value": true, "own": false, "proto": true, "md_marker": true}}));
    assert!(!codes(&v2).contains(&"is_extended_mismatch".to_string()));
    // ③ 单显示器机器:marker 不在场 → false 是真值,零判定(fail-open 不猜测)
    let v3 = healthy_fp(json!({"screen_is_extended": {"value": false, "own": false, "proto": true, "md_marker": false}}));
    assert!(!codes(&v3).contains(&"is_extended_mismatch".to_string()));
    // ④ 缺测:整个字段 null(非 Chrome/探测失败)
    let v4 = healthy_fp(json!({"screen_is_extended": null}));
    assert!(!codes(&v4).contains(&"is_extended_mismatch".to_string()));
    // 不进 HARD_CODES(178 轮教训:处置失效类保持 warn)
    use crate::agent::fingerprint::risk_tier;
    assert_eq!(risk_tier(&crate::agent::fingerprint::classify(&v)), "warn");
}

/// 探针时序契约:xFrame 必须创建在返回对象**之前**(给子帧注入留时序余量),
/// crossrealm 测量必须是返回对象**最后一个字段** —— 顺序倒了会把「注入未落地」
/// 误报成泄漏。源码层断言,防后续改动把顺序调换。
#[test]
fn 探针时序契约_创建在前测量在末() {
    let js = crate::agent::fingerprint::PROBE_JS;
    let at = |needle: &str| js.find(needle).expect(needle);
    assert!(at("const xFrame") < at("crossrealm_tostring_leak:"), "载体 iframe 必须先于测量创建");
    // 测量字段是最后一个探针字段(其后只剩闭合括号与 stringify 收尾)
    let last_field = ["platform_ua:", "screen_is_extended:", "crossrealm_tostring_leak:"]
        .iter()
        .map(|n| at(n))
        .max();
    assert_eq!(last_field, Some(at("crossrealm_tostring_leak:")), "跨 realm 测量必须在对象末位");
}

/// %MULTI_DISPLAY% 渲染契约:stealth_js_for 的产物必须已替换占位符(不残留
/// `%MULTI_DISPLAY%` 字面量 —— 残留即 JS 语法错,整个保真块失效),且三态字面量
/// 进的是 FIDELITY 块(off 档无脚本、kill switch 下无该块)。
#[test]
fn 显示器真值占位符已渲染() {
    let js = crate::agent::browser_ns::stealth_js_for(crate::agent::stealth::StealthMode::Hardened)
        .expect("hardened 脚本");
    assert!(!js.contains("%MULTI_DISPLAY%"), "占位符必须被替换,残留即 JS 语法错");
    // 三态字面量之一在 FIDELITY 块内(JavaScript 里 `null === true` 恒 false,fail-open 分支)
    assert!(
        js.contains("=== true") || js.contains("null"),
        "真值字面量应在场"
    );
    // off 档零脚本
    assert!(crate::agent::browser_ns::stealth_js_for(crate::agent::stealth::StealthMode::Off).is_none());
}

/// 跨 realm 路由代码契约:HARDENED_JS 必须含 typeof 守卫与 home 路由
/// (行为漂移防线:非函数 this 必须回落 origToString 抛 TypeError)。
#[test]
fn 原生化跨realm路由在位() {
    // 从 stealth_js_for 产物断言(hardened 必含原生化块)
    let js = crate::agent::browser_ns::stealth_js_for(crate::agent::stealth::StealthMode::Hardened).unwrap();
    assert!(js.contains("typeof this === 'function'"), "非函数 this 守卫必须在位");
    assert!(js.contains("Object.getPrototypeOf(this).toString"), "回家路由必须在位");
    // 保真档产物不含原生化块(档位隔离源码层可断言 —— 26 轮以来的既有口径)
    let fid = crate::agent::browser_ns::stealth_js_for(crate::agent::stealth::StealthMode::Fidelity).unwrap();
    assert!(!fid.contains("Object.getPrototypeOf(this).toString"), "保真档不得带原生化路由");
}
