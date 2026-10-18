//! 反检测档位（第 174 轮）—— 「非侵入式 / 侵入式」两档 + 环境保真回填的唯一真源。
//!
//! 背景：`MCP_Web_Use` 经 CDP 驱动 Chrome，附着本身在页面侧留下若干可读痕迹
//! （`navigator.webdriver`、WebGL 兜底串、空 plugins/languages、无头档 UA 里的
//! `HeadlessChrome`）。站点风控采这些字段加权评分，命中高风险即转入反爬分支
//! （空 HTML / 风控壳页 / 原地 reload / 整页抹除）。设计见
//! `docs/MCP_Web_Use/26-反检测档位与非侵入式侵入式模式设计.md`。
//!
//! 三档语义（分界线 = 改动离「本机真实浏览器」有多远）：
//! - [`StealthMode::Off`]：零处置（裸环境）；
//! - [`StealthMode::Fidelity`]（**非侵入式**，缺省）：只移除自动化泄漏 + 回填**页面
//!   自身读到的真值**，从不伪造本机没有的身份信息；
//! - [`StealthMode::Hardened`]（**侵入式**）：在保真之上再让「我们改过」本身不可被
//!   反查（被替换函数的 `Function.prototype.toString` 原生化）。
//!
//! 诚实边界：本模块不做验证码绕过、不做代理池、不做协议逆向；回填一律**只在页面
//! 读到兜底值/空值时生效**，真实值原样放行。

use serde_json::{json, Value};
use std::sync::OnceLock;

/// 反检测档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StealthMode {
    /// 不做任何环境处置（回退到裸 CDP 环境，供排障对照）。
    Off,
    /// 非侵入式：只移除自动化泄漏 + 回填本机真值。
    Fidelity,
    /// 侵入式：保真档全部 + 被替换函数的身份原生化。缺省档(2026-10-16)。
    ///
    /// 缺省从 fidelity 升档依据:BOSS 直聘实测(2026-10-16,llaew_20261016_125732)
    /// fidelity 档下被改写函数的 `Function.prototype.toString` 源码可读
    /// (`patched_functions_visible`)——geetest 类风控 SDK 采集该信号加权,
    /// 匿名访问首页即被 302 到 verify.html;hardened 把「我们改过」本身不可
    /// 反查,该签名从根上消失。`LAEW_WEB_STEALTH=fidelity` 可显式降档。
    #[default]
    Hardened,
}

impl StealthMode {
    /// 规范名（用于响应回报与文档对账）。
    pub fn as_str(self) -> &'static str {
        match self {
            StealthMode::Off => "off",
            StealthMode::Fidelity => "fidelity",
            StealthMode::Hardened => "hardened",
        }
    }

    /// 宽松解析（大小写不敏感 + 中英文别名）。无法识别返回 `None`（由调用方决定回退）。
    pub fn parse(s: &str) -> Option<Self> {
        let t = s.trim().to_ascii_lowercase();
        match t.as_str() {
            "" => None,
            "off" | "none" | "0" | "false" | "no" | "关闭" => Some(StealthMode::Off),
            "fidelity" | "compatible" | "passive" | "nonintrusive" | "non-intrusive"
            | "非侵入式" | "保真" => Some(StealthMode::Fidelity),
            "hardened" | "active" | "intrusive" | "侵入式" | "原生化" => Some(StealthMode::Hardened),
            // 通用真值写法一律按「最保守的开启档」处理（保真），与其它布尔开关语义一致。
            "1" | "true" | "yes" | "on" => Some(StealthMode::Fidelity),
            _ => None,
        }
    }

    /// 从 `LAEW_WEB_STEALTH` 读取（未设置/非法返回 `None`）。
    pub fn from_env() -> Option<Self> {
        std::env::var("LAEW_WEB_STEALTH").ok().and_then(|v| Self::parse(&v))
    }

    /// 解析 `open(stealth=…)` 参数（接受字符串或布尔；非法值返回 `None` 由调用方兜底）。
    pub fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::String(s) => Self::parse(s),
            Value::Bool(b) => Some(if *b { StealthMode::Fidelity } else { StealthMode::Off }),
            _ => None,
        }
    }
}

/// legacy 杀开关：`LAEW_WEB_FINGERPRINT=off` 只关掉**保真回填与体检**（第 172 轮语义），
/// `webdriver` 核心覆写照旧 —— 排障「是不是我们改的」时用它做对照组。
pub fn fingerprint_kill_switch() -> bool {
    static OFF: OnceLock<bool> = OnceLock::new();
    *OFF.get_or_init(|| {
        matches!(
            std::env::var("LAEW_WEB_FINGERPRINT")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "off" | "0" | "false" | "no"
        )
    })
}

/// 进程级生效档位（`LAEW_WEB_STEALTH` > 缺省 `hardened`，2026-10-16 升档）。
///
/// 进程级单例：同一进程内所有页面共用（`open(stealth=…)` / `control(set_stealth)` 的
/// 显式覆盖存在 `BrowserInner.stealth`，见 `browser.rs`）。
pub fn mode() -> StealthMode {
    static M: OnceLock<StealthMode> = OnceLock::new();
    *M.get_or_init(|| StealthMode::from_env().unwrap_or_default())
}

/// 保真块（WebGL 回填 / plugins / languages / chrome.runtime / permissions 一致性）是否注入。
pub fn fidelity_enabled() -> bool {
    mode() != StealthMode::Off && !fingerprint_kill_switch()
}

/// 原生化块是否注入（仅侵入式档）。
pub fn hardened_enabled() -> bool {
    mode() == StealthMode::Hardened
}

/// 响应回报段 `data.stealth`：档位 + 各块是否生效（人工/QC 对账用，零 CDP 开销）。
///
/// `effective` 传**实例实际生效档**（`BrowserManager::stealth_mode()`，可能被
/// `open(stealth=…)` 显式覆盖），进程级默认值另列 `env_default` 供对账。
pub fn status_json(effective: StealthMode) -> Value {
    let kill = fingerprint_kill_switch();
    json!({
        "mode": effective.as_str(),
        "env_default": mode().as_str(),
        "fidelity": effective != StealthMode::Off && !kill,
        "hardened": effective == StealthMode::Hardened,
        "kill_switch": kill,
        "note": "缺省=hardened(侵入式);fidelity=非侵入式(只移除自动化泄漏+回填本机真值)/hardened=侵入式(保真+被替换函数身份原生化,含跨档升级时对上一轮改写的重新认领);\
                 档位只影响新注入,运行时切档为升档即时生效;\
                 注入面(第 179 轮):窗口面 API 全部收敛到一个 window[Symbol.for(<ns>)] 命名空间对象,\
                 window 字符串键恒 0(旧实现有头档 30 个),符号键 1;DOM id/class/data-* 换用独立随机 token;\
                 对账数据见 data.fingerprint.injected_ns_keys / injected_ns_symbols / ns_symbol_present;\
                 第 182 轮:无头档另叠加**原生 CDP 环境仿真**(非 JS 注入)——setEmulatedMedia 四元指针/悬停媒体特性\
                 + setTouchEmulationEnabled(false) 归零触摸点(真机双档基线:有头档 hover/fine/any-* 全 true、\
                 maxTouchPoints=0,无头档裸读数全 false/1);有头档恒零动作,connect 零注入不变;\
                 第 183 轮:参数层真值对齐 —— 无头档不再强制 --disable-gpu/--hide-scrollbars,\
                 WebGL 报本机真 GPU(与本机有头 Chrome 逐字节一致,非 WARP 软渲染串),\
                 滚动条宽度跟随宿主 OS 默认;LAEW_WEB_HEADLESS_SOFTRENDER=on 恢复旧软渲染集(排障);\
                 LAEW_WEB_STEALTH=fidelity 可显式降档,=off 可整体关闭,LAEW_WEB_FINGERPRINT=off 只关保真回填与体检",
    })
}

// =============================== 无头档 UA 清理 ===============================

/// 页面侧高熵取样脚本（返回 JSON 字符串；`await_promise` 必需）。
///
/// **取值全部来自页面自身读数** —— 只用于把品牌列表里的 `HeadlessChrome` 滤掉，
/// 其余字段原样透传，不做任何拼凑。
pub const UA_PROBE_JS: &str = r#"(async () => {
  const out = { ua: navigator.userAgent || '' };
  try {
    const uad = navigator.userAgentData;
    if (uad) {
      let hi = {};
      try {
        hi = await uad.getHighEntropyValues(
          ['platformVersion', 'architecture', 'bitness', 'model', 'fullVersionList']
        ) || {};
      } catch (e) { hi = {}; }
      out.brands = uad.brands || [];
      out.mobile = !!uad.mobile;
      out.platform = uad.platform || '';
      out.platform_version = hi.platformVersion || '';
      out.architecture = hi.architecture || '';
      out.bitness = hi.bitness || '';
      out.model = hi.model || '';
      out.full_version_list = hi.fullVersionList || uad.brands || [];
    }
  } catch (e) {}
  // JSON.stringify 必须在 async 体内(表达式求值 = Promise 时由 CDP
  // `await_promise` 解包;**写在外面会把 Promise 本身序列化成 `{}`**)。
  return JSON.stringify(out);
})()"#;

/// 品牌名是否带自动化令牌（`HeadlessChrome` / `Headless Edge` …）。
pub fn is_headless_brand(brand: &str) -> bool {
    brand.to_ascii_lowercase().contains("headless")
}

/// UA 字符串是否含自动化令牌。
pub fn is_headless_ua(ua: &str) -> bool {
    ua.contains("Headless")
}

/// 去掉 UA 字符串里的自动化令牌(第 180 轮):与 [`UaPlan::user_agent`] 完全同一
/// 替换规则,供「无页面上下文也能先清字符串」的路径复用(浏览器级 `getVersion`
/// 拿到的 UA → 派生页出生即下发 string-only 覆盖,品牌表待落地后补全)。
pub fn strip_headless_tokens(ua: &str) -> String {
    ua.replace("HeadlessChrome", "Chrome").replace("Headless Edge", "Edge")
}

/// UA 清理计划（全部字段来自页面自报值，不伪造）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UaPlan {
    /// 清理后的 UA 字符串。
    pub user_agent: String,
    /// 过滤掉自动化品牌后的品牌列表（可能为空 = 不下发 Client Hints 元数据）。
    pub brands: Vec<(String, String)>,
    /// `Sec-CH-UA-Full-Version-List` 对应列表。
    pub full_version_list: Vec<(String, String)>,
    pub platform: String,
    pub platform_version: String,
    pub architecture: String,
    pub bitness: String,
    pub model: String,
    pub mobile: bool,
}

impl UaPlan {
    /// 是否连 Client Hints 元数据一起下发（品牌列表被滤空时只改 UA 字符串，
    /// 避免下发一个比真值更可疑的空品牌列表）。
    pub fn has_metadata(&self) -> bool {
        !self.brands.is_empty()
    }
}

fn brand_list(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|b| {
                    let brand = b.get("brand").and_then(Value::as_str)?;
                    if is_headless_brand(brand) {
                        return None;
                    }
                    let ver = b.get("version").and_then(Value::as_str).unwrap_or("");
                    Some((brand.to_string(), ver.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 由页面取样结果生成 UA 清理计划；**无需处置时返回 `None`**（真实无头令牌不在时零动作）。
pub fn ua_plan(probe: &Value) -> Option<UaPlan> {
    let ua = probe.get("ua").and_then(Value::as_str).unwrap_or("");
    let brands = brand_list(probe.get("brands").unwrap_or(&Value::Null));
    let dv = probe.get("full_version_list").cloned().unwrap_or(Value::Null);
    let full = if dv.is_null() {
        Vec::new()
    } else {
        brand_list(&dv)
    };
    let raw_brands_len = probe
        .get("brands")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let brands_filtered = brands.len() < raw_brands_len;
    if !is_headless_ua(ua) && !brands_filtered {
        return None;
    }
    Some(UaPlan {
        user_agent: ua.replace("HeadlessChrome", "Chrome").replace("Headless Edge", "Edge"),
        brands,
        full_version_list: full,
        platform: probe.get("platform").and_then(Value::as_str).unwrap_or("").to_string(),
        platform_version: probe
            .get("platform_version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        architecture: probe
            .get("architecture")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        bitness: probe.get("bitness").and_then(Value::as_str).unwrap_or("").to_string(),
        model: probe.get("model").and_then(Value::as_str).unwrap_or("").to_string(),
        mobile: probe.get("mobile").and_then(Value::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 档位解析与别名() {
        assert_eq!(StealthMode::parse("off"), Some(StealthMode::Off));
        assert_eq!(StealthMode::parse("  OFF "), Some(StealthMode::Off));
        assert_eq!(StealthMode::parse("fidelity"), Some(StealthMode::Fidelity));
        assert_eq!(StealthMode::parse("非侵入式"), Some(StealthMode::Fidelity));
        assert_eq!(StealthMode::parse("保真"), Some(StealthMode::Fidelity));
        assert_eq!(StealthMode::parse("hardened"), Some(StealthMode::Hardened));
        assert_eq!(StealthMode::parse("侵入式"), Some(StealthMode::Hardened));
        assert_eq!(StealthMode::parse("Intrusive"), Some(StealthMode::Hardened));
        // 布尔真值按最保守的开启档处理
        assert_eq!(StealthMode::parse("1"), Some(StealthMode::Fidelity));
        assert_eq!(StealthMode::parse("on"), Some(StealthMode::Fidelity));
        // 空串与非法值一律 None（由调用方兜底，不静默改档）
        assert_eq!(StealthMode::parse(""), None);
        assert_eq!(StealthMode::parse("是的"), None);
        assert_eq!(StealthMode::from_value(&json!(true)), Some(StealthMode::Fidelity));
        assert_eq!(StealthMode::from_value(&json!(false)), Some(StealthMode::Off));
        assert_eq!(StealthMode::from_value(&json!("hardened")), Some(StealthMode::Hardened));
        assert_eq!(StealthMode::from_value(&json!(7)), None);
    }

    #[test]
    fn 档位名稳定() {
        // 2026-10-16 缺省升档:hardened(toString 原生化)根治 patched_functions_visible
        // 可被 geetest 类风控反查(BOSS 直聘实测匿名访问即被打标 302 verify.html)。
        assert_eq!(StealthMode::default(), StealthMode::Hardened, "缺省必须是侵入式原生化档");
        assert_eq!(StealthMode::Off.as_str(), "off");
        assert_eq!(StealthMode::Fidelity.as_str(), "fidelity");
        assert_eq!(StealthMode::Hardened.as_str(), "hardened");
    }

    #[test]
    fn 无头令牌识别() {
        assert!(is_headless_ua("Mozilla/5.0 … HeadlessChrome/155.0.0.0 Safari/537.36"));
        assert!(!is_headless_ua("Mozilla/5.0 … Chrome/155.0.0.0 Safari/537.36"));
        assert!(is_headless_brand("HeadlessChrome"));
        assert!(is_headless_brand("headless"));
        assert!(!is_headless_brand("Google Chrome"));
        assert!(!is_headless_brand("Chromium"));
    }

    fn probe() -> Value {
        json!({
            "ua": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/155.0.0.0 Safari/537.36",
            "brands": [
                {"brand": "HeadlessChrome", "version": "155"},
                {"brand": "Chromium", "version": "155"},
                {"brand": "Google Chrome", "version": "155"}
            ],
            "full_version_list": [
                {"brand": "HeadlessChrome", "version": "155.0.0.0"},
                {"brand": "Chromium", "version": "155.0.0.0"},
                {"brand": "Google Chrome", "version": "155.0.0.0"}
            ],
            "platform": "macOS",
            "platform_version": "15.3.1",
            "architecture": "arm",
            "bitness": "64",
            "model": "",
            "mobile": false
        })
    }

    #[test]
    fn ua清理只滤品牌且其余字段原样() {
        let p = ua_plan(&probe()).expect("无头 UA 必须产出清理计划");
        assert!(!is_headless_ua(&p.user_agent), "UA 令牌必须清掉:{}", p.user_agent);
        assert!(p.user_agent.contains("Chrome/155.0.0.0"), "版本号保留:{}", p.user_agent);
        let names: Vec<&str> = p.brands.iter().map(|(b, _)| b.as_str()).collect();
        assert_eq!(names, vec!["Chromium", "Google Chrome"], "只滤自动化品牌");
        let full: Vec<&str> = p.full_version_list.iter().map(|(b, _)| b.as_str()).collect();
        assert_eq!(full, vec!["Chromium", "Google Chrome"]);
        // 其余字段原样透传（零伪造）
        assert_eq!(p.platform, "macOS");
        assert_eq!(p.platform_version, "15.3.1");
        assert_eq!(p.architecture, "arm");
        assert_eq!(p.bitness, "64");
        assert!(!p.mobile);
        assert!(p.has_metadata());
    }

    #[test]
    fn 真实环境零动作() {
        let mut p = probe();
        p["ua"] = json!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36");
        p["brands"] = json!([{"brand": "Chromium", "version": "155"}, {"brand": "Google Chrome", "version": "155"}]);
        p["full_version_list"] = json!([{"brand": "Chromium", "version": "155.0.0.0"}, {"brand": "Google Chrome", "version": "155.0.0.0"}]);
        assert!(ua_plan(&p).is_none(), "干净环境必须零动作（不无谓下发覆盖）");
    }

    #[test]
    fn 品牌被滤空时不下发元数据() {
        let mut p = probe();
        p["brands"] = json!([{"brand": "HeadlessChrome", "version": "155"}]);
        p["full_version_list"] = json!([{"brand": "HeadlessChrome", "version": "155.0.0.0"}]);
        let plan = ua_plan(&p).expect("UA 仍有令牌,需处置");
        assert!(plan.brands.is_empty());
        assert!(!plan.has_metadata(), "品牌滤空时只改 UA 字符串,不下发空品牌表");
    }

    #[test]
    fn 探针脚本存在性() {
        assert!(UA_PROBE_JS.contains("getHighEntropyValues"));
        assert!(UA_PROBE_JS.contains("fullVersionList"));
        assert!(UA_PROBE_JS.contains("JSON.stringify"));
    }
}
