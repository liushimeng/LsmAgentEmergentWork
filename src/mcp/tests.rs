//! MCP 协议层单测:JSON-RPC 编解码 / ContentBlock 投影 / 截断 / 退避 / 分页容错。

use serde_json::json;

use super::jsonrpc::{candidate_frames, encode_notification, encode_request, extract_result, parse_incoming, Incoming};
use super::manager::{BackoffPolicy, BackoffState};
use super::*;

// ===================== JSON-RPC 编解码 =====================

#[test]
fn encode_request_is_single_line_json() {
    let s = encode_request(7, "tools/list", json!({"cursor": "c1"}));
    assert!(!s.contains('\n'), "NDJSON 行帧约束:消息内不得含裸换行");
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 7);
    assert_eq!(v["method"], "tools/list");
    assert_eq!(v["params"]["cursor"], "c1");
}

#[test]
fn encode_notification_has_no_id() {
    let s = encode_notification("notifications/initialized", json!({}));
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.get("id").is_none(), "通知帧必须无 id");
    assert_eq!(v["method"], "notifications/initialized");
}

#[test]
fn parse_incoming_response_ok_and_error() {
    let ok = parse_incoming(r#"{"jsonrpc":"2.0","id":3,"result":{"tools":[]}}"#).unwrap();
    match ok {
        Incoming::Response { id, result, error } => {
            assert_eq!(id, 3);
            assert!(error.is_none());
            assert_eq!(result.unwrap()["tools"], json!([]));
        }
        other => panic!("应为 Response: {other:?}"),
    }
    let err = parse_incoming(r#"{"jsonrpc":"2.0","id":4,"error":{"code":-32601,"message":"no"}}"#).unwrap();
    match err {
        Incoming::Response { id, error, .. } => {
            assert_eq!(id, 4);
            let e = error.unwrap();
            assert_eq!(e.code, -32601);
            assert_eq!(e.message, "no");
        }
        other => panic!("应为 Response: {other:?}"),
    }
}

#[test]
fn parse_incoming_notification_and_garbage() {
    let n = parse_incoming(r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#).unwrap();
    assert!(matches!(n, Incoming::Notification { .. }));
    assert!(parse_incoming("not json at all").is_none());
    assert!(parse_incoming("").is_none());
}

#[test]
fn extract_result_from_plain_json_and_sse() {
    let body = r#"{"jsonrpc":"2.0","id":9,"result":{"ok":true}}"#;
    let v = extract_result(body, 9).unwrap();
    assert_eq!(v["ok"], true);

    // SSE 多帧,取 id 匹配的那帧;无关帧(含 id 不匹配的 error)跳过。
    let sse = concat!(
        "event: message\n",
        "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":1}\n\n",
        "data: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{\"via\":\"sse\"}}\n\n",
    );
    let v = extract_result(sse, 9).unwrap();
    assert_eq!(v["via"], "sse");

    let err_frame = r#"{"jsonrpc":"2.0","id":9,"error":{"code":-32000,"message":"boom"}}"#;
    match extract_result(err_frame, 9) {
        Err(McpError::Call { code, message }) => {
            assert_eq!(code.as_deref(), Some("-32000"));
            assert!(message.contains("boom"));
        }
        other => panic!("应为 Call 错误: {other:?}"),
    }

    // 找不到匹配 id → Protocol 错误。
    assert!(matches!(
        extract_result(r#"{"jsonrpc":"2.0","id":1,"result":1}"#, 99),
        Err(McpError::Protocol(_))
    ));
}

#[test]
fn candidate_frames_shapes() {
    assert_eq!(candidate_frames(r#"{"a":1}"#).len(), 1);
    let sse = "data: {\"x\":1}\n\ndata: {\"y\":2}\n\n";
    assert_eq!(candidate_frames(sse).len(), 2);
    assert!(candidate_frames("data: [DONE]\n").is_empty(), "[DONE] 哨兵不产出帧");
}

// ===================== ContentBlock 投影 =====================

#[test]
fn project_text_passthrough_and_empty_fallback() {
    let v = json!({"content": [{"type": "text", "text": "hello"}]});
    let out = project_content_blocks(&v);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].kind, "text");
    assert_eq!(out[0].text, "hello");

    let empty = project_content_blocks(&json!({"content": []}));
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0].kind, "empty", "空结果也给出可见占位,不静默");
}

#[test]
fn project_image_resource_audio_degrade_not_drop() {
    let v = json!({"content": [
        {"type": "image", "mimeType": "image/png", "data": "AAAA"},
        {"type": "resource", "resource": {"uri": "file:///a", "mimeType": "text/plain", "text": "body"}},
        {"type": "resource_link", "uri": "https://x", "name": "link"},
        {"type": "audio", "mimeType": "audio/wav"},
        {"type": "weird_future_type"}
    ]});
    let out = project_content_blocks(&v);
    assert_eq!(out.len(), 5, "永不静默丢弃:每个块都有投影");
    assert_eq!(out[0].kind, "image");
    assert!(out[0].text.contains("image/png"));
    assert!(out[1].text.contains("file:///a") && out[1].text.contains("body"));
    assert!(out[2].text.contains("https://x"));
    assert_eq!(out[3].kind, "audio");
    assert!(out[4].text.contains("unsupported"), "未知类型降级为诊断文本");
}

#[test]
fn project_merges_when_over_limit() {
    let big = "x".repeat(MAX_RESULT_CHARS + 1000);
    let v = json!({"content": [{"type": "text", "text": big}]});
    let out = project_content_blocks(&v);
    assert_eq!(out.len(), 1);
    assert!(out[0].text.contains("省略"), "超限应中间截断标注");
    assert!(out[0].text.chars().count() < MAX_RESULT_CHARS + 100);
}

#[test]
fn truncate_middle_keeps_head_and_tail() {
    let s = "a".repeat(100) + &"b".repeat(100);
    let t = truncate_middle(&s, 60);
    assert!(t.starts_with('a') && t.ends_with('b'));
    assert!(t.contains("省略"));
    assert_eq!(truncate_middle("short", 60), "short");
}

// ===================== 退避状态机 =====================

#[test]
fn backoff_delay_doubles_and_caps() {
    let p = BackoffPolicy::default();
    assert_eq!(p.delay_ms(0), 0);
    assert_eq!(p.delay_ms(1), 500);
    assert_eq!(p.delay_ms(2), 1000);
    assert_eq!(p.delay_ms(3), 2000);
    assert_eq!(p.delay_ms(10), 30_000, "封顶 max_delay_ms");
}

#[test]
fn backoff_stability_window_resets_counter() {
    let p = BackoffPolicy::default();
    let mut st = BackoffState::default();
    let t0 = std::time::Instant::now();

    st.on_success(t0);
    // 稳定性窗口内连续失败:计数累积。
    st.on_failure(&p, t0 + std::time::Duration::from_millis(1000));
    assert_eq!(st.failed_attempts, 1);
    st.on_failure(&p, t0 + std::time::Duration::from_millis(2000));
    assert_eq!(st.failed_attempts, 2);

    // 长时间稳定后再失败:计数归零重来(DeepSeek 稳定性窗口)。
    let after_window = t0 + std::time::Duration::from_millis(2000 + p.stability_window_ms + 1);
    st.on_failure(&p, after_window);
    assert_eq!(st.failed_attempts, 1, "稳定性窗口应重置累积计数");
    assert!(!st.exhausted);
}

#[test]
fn backoff_exhausts_then_cools_down() {
    let p = BackoffPolicy {
        max_attempts: 2,
        ..Default::default()
    };
    let mut st = BackoffState::default();
    let t0 = std::time::Instant::now();
    assert!(st.on_failure(&p, t0).is_some());
    assert!(st.on_failure(&p, t0).is_some());
    assert!(st.on_failure(&p, t0).is_none(), "超 max_attempts 后返回 None");
    assert!(st.exhausted);
    assert_eq!(st.cooldown_remaining_ms(t0), Some(u64::MAX), "exhausted = 永久冷却到显式 connect");

    st.reset();
    assert!(!st.exhausted);
    assert_eq!(st.cooldown_remaining_ms(t0), None);
}

#[test]
fn backoff_cooldown_remaining_counts_down() {
    let p = BackoffPolicy::default();
    let mut st = BackoffState::default();
    let t0 = std::time::Instant::now();
    st.on_failure(&p, t0); // delay 500ms
    let rem = st.cooldown_remaining_ms(t0 + std::time::Duration::from_millis(200));
    assert!(rem.is_some() && rem.unwrap() <= 500);
    assert_eq!(
        st.cooldown_remaining_ms(t0 + std::time::Duration::from_millis(600)),
        Some(0),
        "过期后剩余 0(可立即重试)"
    );
}

// ===================== 错误码映射 / 传输类型 =====================

#[test]
fn error_envelope_code_mapping() {
    assert_eq!(error_envelope_code(&McpError::UnknownServer("x".into())), 5001);
    assert_eq!(error_envelope_code(&McpError::Connect("x".into())), 5002);
    assert_eq!(
        error_envelope_code(&McpError::Cooldown { retry_after_ms: 1 }),
        5002
    );
    assert_eq!(error_envelope_code(&McpError::Handshake("x".into())), 5003);
    assert_eq!(error_envelope_code(&McpError::Protocol("x".into())), 5003);
    assert_eq!(error_envelope_code(&McpError::Timeout(1)), 5004);
    assert_eq!(
        error_envelope_code(&McpError::Call {
            message: "m".into(),
            code: None
        }),
        5005
    );
    assert_eq!(error_envelope_code(&McpError::Resource("x".into())), 5006);
    assert_eq!(error_envelope_code(&McpError::Environment("x".into())), 3001);
}

#[test]
fn transport_kind_parse_aliases() {
    assert_eq!(TransportKind::parse("stdio").unwrap(), TransportKind::Stdio);
    assert_eq!(TransportKind::parse("HTTP").unwrap(), TransportKind::Http);
    assert_eq!(TransportKind::parse("streamable-http").unwrap(), TransportKind::Http);
    assert!(TransportKind::parse("websocket").is_err());
}
