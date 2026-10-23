//! chromiumoxide 0.9 类型化 CDP 命令的补丁层（第 184 轮）。
//!
//! 背景（源码层 + 真机实证，见 `docs/MCP_Web_Chrome_Use/38` §三）：该 crate 生成的
//! `AddScriptToEvaluateOnNewDocumentParams` **没有 `runForEveryFrame` 字段**
//! （.cdp 快照落后于协议），而 CDP 该参数缺省 `false` = **脚本只注入顶层 frame**。
//! 隐身脚本不进子帧时，站点自建同源 iframe 即可拿到一个未经处置的**原生**
//! `Function.prototype.toString` 反查顶层被改写函数（toString 自身 / webdriver
//! getter / permissions.query），直接读到我们的注入源码 —— 侵入式档「改过不可
//! 反查」被完整击穿。
//!
//! 解法：用 `#[serde(flatten)]` 在类型化参数之上补发一个字段，实现
//! `chromiumoxide::Method`/`Command` 走 `Page::execute` 正常通道 —— 不 fork
//! CDP 客户端、不引新 crate。老 Chrome 若拒绝未知字段（协议错误），调用方回退
//! 旧参数重发一次（只丢子帧覆盖，不丢顶层注入）。

use chromiumoxide::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, AddScriptToEvaluateOnNewDocumentReturns,
};

/// `Page.addScriptToEvaluateOnNewDocument` + `runForEveryFrame: true`。
///
/// 与原参数的唯一差异：脚本会在**每个新 frame**（含 srcdoc/about:blank/跨域 iframe）
/// 的 document 创建时刻执行 —— 与 Playwright `addInitScript` 同源语义。
#[derive(Debug, serde::Serialize)]
pub(crate) struct AddScriptToEvaluateOnNewDocumentEveryFrame {
    #[serde(flatten)]
    base: AddScriptToEvaluateOnNewDocumentParams,
    /// CDP 协议字段（chromiumoxide 0.9.1 类型缺失，缺省 false = 只进顶层）。
    #[serde(rename = "runForEveryFrame")]
    run_for_every_frame: bool,
}

impl AddScriptToEvaluateOnNewDocumentEveryFrame {
    /// 构造等价于 `AddScriptToEvaluateOnNewDocumentParams { source, run_immediately }`
    /// 且 `runForEveryFrame: true` 的命令。
    pub(crate) fn new(source: String, run_immediately: bool) -> Self {
        Self {
            base: AddScriptToEvaluateOnNewDocumentParams {
                source,
                world_name: None,
                include_command_line_api: None,
                run_immediately: Some(run_immediately),
            },
            run_for_every_frame: true,
        }
    }
}

impl chromiumoxide::Method for AddScriptToEvaluateOnNewDocumentEveryFrame {
    fn identifier(&self) -> chromiumoxide::types::MethodId {
        "Page.addScriptToEvaluateOnNewDocument".into()
    }
}

impl chromiumoxide::Command for AddScriptToEvaluateOnNewDocumentEveryFrame {
    type Response = AddScriptToEvaluateOnNewDocumentReturns;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chromiumoxide::Method as _;

    /// 序列化形状必须真的带出 `runForEveryFrame` —— 否则整个补丁层是死代码
    /// （第 178 轮 `injected_ns_keys` 死自证的同类教训：机制存在不等于机制生效）。
    #[test]
    fn 序列化补发run_for_every_frame() {
        let cmd = AddScriptToEvaluateOnNewDocumentEveryFrame::new("console.log(1)".into(), true);
        let v = serde_json::to_value(&cmd).expect("serialize");
        assert_eq!(v["runForEveryFrame"], serde_json::json!(true), "字段必须在场:{v}");
        assert_eq!(v["source"], serde_json::json!("console.log(1)"));
        assert_eq!(v["runImmediately"], serde_json::json!(true));
        assert_eq!(v["worldName"], serde_json::Value::Null, "None 不下发");
        // 方法标识必须与原命令一致(走同一 CDP method)
        assert_eq!(cmd.identifier().as_ref(), "Page.addScriptToEvaluateOnNewDocument");
    }
}
