//! 自签名证书 TLS 适配的集成验证(2026-09-10)。
//!
//! 设计见 `docs/自签名证书TLS适配/01-设计与解决方案.md`。
//! 依赖真实内网端点,默认忽略,手动运行:
//!
//! ```bash
//! cargo test --test tls_self_signed -- --ignored --nocapture
//! ```

use lsm_agent::llm::{build_http_client, tls_insecure_for};

/// 用户问题现场:HTTPS + IP + 自签名证书网关。
const SELF_SIGNED_IP_ENDPOINT: &str = "https://8.130.85.252:29003/Anthropic";

/// 合并为一个测试顺序执行,避免环境变量在并行测试间竞争。
#[tokio::test]
#[ignore = "需要可达的真实自签名端点,手动运行"]
async fn ip_self_signed_endpoint_tls_adaptation() {
    // 1) 自动模式:IP 主机放宽,TLS 握手通过。
    assert!(
        tls_insecure_for(SELF_SIGNED_IP_ENDPOINT),
        "自动模式应对 IP 主机放宽证书校验"
    );
    let http = build_http_client(SELF_SIGNED_IP_ENDPOINT);
    // TLS 握手通过即可:未带鉴权,期望拿到 HTTP 响应(401/400/404 均可),
    // 而不是 reqwest 连接期错误(修复前的 `error sending request`)。
    let resp = http
        .post(format!("{SELF_SIGNED_IP_ENDPOINT}/v1/messages"))
        .body("{}")
        .send()
        .await
        .expect("自签名 IP 端点 TLS 握手应成功(修复前此处报错)");
    let status = resp.status();
    println!("自动模式 HTTP 状态(未鉴权,预期 4xx): {status}");
    assert!(
        status.is_client_error() || status.is_success(),
        "应收到正常 HTTP 响应而非 TLS 失败,实际: {status}"
    );

    // 2) 全局严格模式:同一自签名端点必须握手失败(安全基线不被破坏)。
    std::env::set_var("LAEW_TLS_INSECURE", "0");
    let http_strict = build_http_client(SELF_SIGNED_IP_ENDPOINT);
    let result = http_strict
        .post(format!("{SELF_SIGNED_IP_ENDPOINT}/v1/messages"))
        .body("{}")
        .send()
        .await;
    std::env::remove_var("LAEW_TLS_INSECURE");
    assert!(
        result.is_err(),
        "严格模式下自签名证书应校验失败,但请求成功了"
    );
    println!("严格模式按预期拒绝自签名证书: {}", result.unwrap_err());
}
