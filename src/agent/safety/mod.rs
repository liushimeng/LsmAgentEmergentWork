//! Agent 安全相关工具集合。
//!
//! 本轮(2026-09-09)新引入 `prompt_injection` 模块(L1208):
//! 在工具结果拼回 prompt 前扫描可疑的 prompt injection 模式,
//! 对齐 openclaw §11.0 14 种正则与 claudecode §11.1「flag it directly to the user」温和告警哲学。
//!
//! 后续可扩展:
//! - 密钥扫描(API Key / Token)
//! - 22 层 Bash 检测对齐 claudecode

pub mod credentials;
pub mod prompt_injection;
pub mod target_anchor;
pub mod url_safety;

pub use credentials::{Vault, CREDENTIAL_PREFIX};
pub use prompt_injection::{
    scan_and_wrap, InjectionSource, InjectionVerdict, MatchHit, Severity, INJECTION_BOUNDARY,
};
pub use target_anchor::{
    build_clarification_message, check_open_against_anchor, check_open_against_target_anchor,
    current_target_anchor, detect_target_drift, detect_unresolved_target_reference,
    extract_hosts_from_text,
    host_matches_anchor_domain, install_target_anchor, parse_url_host, registrable_domain_of_host,
    registrable_domain_of_url, set_current_target_anchor, target_anchor_block_enabled,
    target_anchor_enabled, AnchorViolation, TargetAnchor, TargetAnchorScopeGuard,
};
pub use url_safety::{
    check_endpoint_safety, is_safe_endpoint, is_safe_endpoint_for_record, probe_is_private,
    BLOCKED_HOSTNAMES,
};
