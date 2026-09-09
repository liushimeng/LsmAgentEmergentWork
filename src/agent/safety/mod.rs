//! Agent 安全相关工具集合。
//!
//! 本轮(2026-09-09)新引入 `prompt_injection` 模块(L1208):
//! 在工具结果拼回 prompt 前扫描可疑的 prompt injection 模式,
//! 对齐 openclaw §11.0 14 种正则与 claudecode §11.1「flag it directly to the user」温和告警哲学。
//!
//! 后续可扩展:
//! - SSRF 检测(L1215)
//! - 密钥扫描(API Key / Token)
//! - 22 层 Bash 检测对齐 claudecode

pub mod prompt_injection;

pub use prompt_injection::{
    scan_and_wrap, InjectionSource, InjectionVerdict, MatchHit, Severity, INJECTION_BOUNDARY,
};
