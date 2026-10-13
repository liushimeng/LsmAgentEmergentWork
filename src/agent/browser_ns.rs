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

/// 隐身自检（L2）：launch 后首个页面探测结果解析。返回违例描述（None = 通过）。
/// 探测表达式返回 `{wd, leaked}`：`wd` = `navigator.webdriver===true`；
/// `leaked` = `Object.keys(window)` 中仍含 `laew` 字样的键（固定名残留）。
pub fn stealth_probe_issue(probe: &serde_json::Value) -> Option<String> {
    if probe.get("wd").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        return Some("navigator.webdriver=true（反检测启动参数未生效）".to_string());
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
    }
}
