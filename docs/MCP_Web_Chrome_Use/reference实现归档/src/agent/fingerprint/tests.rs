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

    /// 第 183 轮:WARP 软渲染串(实测 `--disable-gpu` 无头档读数)必须命中兜底串
    /// 判定 —— 旧 is_default_webgl 的三个令牌一个不含,曾原样放行。
    #[test]
    fn webgl_warp软渲染串命中() {
        let v = fp(
            json!({"ok": true, "unmasked": true,
                   "vendor": "Google Inc. (Microsoft)",
                   "renderer": "ANGLE (Microsoft, Microsoft Basic Render Driver (0x000008C) Direct3D11 vs_5_0 ps_5_0, D3D11)"}),
            json!({}),
        );
        assert_eq!(codes(&v), vec!["webgl_default_strings".to_string()]);
        // 第 183 轮后的健康读数(真 GPU,与本机有头逐字节一致)不得命中
        let healthy = fp(
            json!({"ok": true, "unmasked": true,
                   "vendor": "Google Inc. (Intel)",
                   "renderer": "ANGLE (Intel, Intel(R) UHD Graphics (0x00004688) Direct3D11 vs_5_0 ps_5_0, D3D11)"}),
            json!({}),
        );
        assert!(codes(&healthy).is_empty(), "真硬件串不得告警:{:?}", codes(&healthy));
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
        ] {
            assert!(c.contains(&expect.to_string()), "缺 {expect}:{c:?}");
        }
        // 第 182 轮:单侧 denied 已退出 suspicious(laew 启动参数下两档恒 denied,零判别力)
        assert!(
            !c.contains(&"notification_denied".to_string()),
            "单侧 denied 不得再进 suspicious:{c:?}"
        );
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
            // 第 184 轮
            "crossrealm_tostring_leak",
            "screen_is_extended",
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
            "touch_points_headless",
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

    // ===================== 第 182 轮:指针/触摸环境仿真自检与降噪 =====================

    /// 桌面 UA + 触摸点 >0 命中(真机基线:有头档 0 / 无头档裸读数 1);移动 UA 不误伤;
    /// 仿真生效后读 0 → 不命中(自检语义)。
    #[test]
    fn 桌面UA下触摸点非零命中() {
        let base = healthy_webgl();
        let desktop = json!({"platform_ua": {"platform": "Win32", "ua": "Mozilla/5.0 (Windows NT 10.0) Chrome/155"}});
        let v = fp(
            base.clone(),
            json!({"max_touch_points": 1, "platform_ua": desktop["platform_ua"].clone()}),
        );
        assert!(codes(&v).contains(&"touch_points_headless".to_string()));
        // 仿真生效(归零)→ 不命中
        let ok = fp(
            base.clone(),
            json!({"max_touch_points": 0, "platform_ua": desktop["platform_ua"].clone()}),
        );
        assert!(!codes(&ok).contains(&"touch_points_headless".to_string()));
        // 移动 UA(触摸正常)→ 不误伤
        let mob = fp(
            base.clone(),
            json!({"max_touch_points": 5,
                   "platform_ua": {"platform": "Linux armv81", "ua": "Mozilla/5.0 (Linux; Android 14) Mobile Safari"}}),
        );
        assert!(!codes(&mob).contains(&"touch_points_headless".to_string()));
        // 桌面 UA 但缺触摸点字段(老探针)→ fail-open 不告警
        let missing = fp(base, json!({"platform_ua": desktop["platform_ua"].clone()}));
        assert!(!codes(&missing).contains(&"touch_points_headless".to_string()));
    }

    /// 第 182 轮:`notification_denied` 退出 suspicious 后,字段仍要回报到 build()
    /// 顶层(观测可见性不丢),且任何单侧 denied 值都不得抬 risk_tier。
    #[test]
    fn 单侧denied只观测不告警且回报build() {
        let base = healthy_webgl();
        for perm in ["denied", "default", "granted"] {
            let v = fp(base.clone(), json!({"notification_permission": perm}));
            assert!(
                codes(&v).is_empty(),
                "单侧 {perm} 不得产生任何告警:{:?}",
                codes(&v)
            );
            let b = build(&v, StealthMode::Hardened);
            assert_eq!(b["notification_permission"], json!(perm));
            assert_eq!(b["risk_tier"], "ok", "{perm} 不得抬档:{b}");
        }
        // build() 顶层回报触摸点观测值
        let v = fp(
            base,
            json!({"max_touch_points": 0,
                   "platform_ua": {"platform": "Win32", "ua": "Mozilla/5.0 (Windows NT 10.0) Chrome/155"}}),
        );
        let b = build(&v, StealthMode::Hardened);
        assert_eq!(b["max_touch_points"], json!(0));
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
