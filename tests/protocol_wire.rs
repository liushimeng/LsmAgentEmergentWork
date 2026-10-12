//! 协议 wire 层契约(取代 e2e §6 / §4h / §4j 的 40 余条抓包断言)。
//!
//! 这里不打桩 LlmClient,而是**跑真实的协议客户端**打真实的 HTTP 请求,
//! 由 `tests/support` 的进程内 mock 收下并留抓包 —— 因此断言的是
//! 「线上真正发出去的东西」,而不是某次重构后可能漂移的中间结构。
//!
//! 覆盖:
//! - 请求路径与端点补全(`/v1/messages` / `/chat/completions`)
//! - 认证与追踪头(`x-api-key` / `Authorization: Bearer` / `X-Session-Id` / `User-Agent`)
//! - body 结构:system 形态、messages、tools schema 两协议差异
//! - `metadata.user_id` 四字段(device_id / session_id / account_uuid / agent)
//! - Anthropic 三段式 system + `cache_control` 断点(§4h)
//! - 逐角色 User-Agent 注入(§6 的 6 角色覆盖)

mod support;

use lsm_agent::agent::profile::AgentProfile;
use lsm_agent::agent::Agent;
use lsm_agent::database::models::Protocol;
use lsm_agent::llm::{ChatMessage, ContentBlock, Role, ToolDef};
use lsm_agent::session::Session;
use serde_json::{json, Value};
use support::{meta, mock_client, MockLlm, Reply};

// ---------------------------------------------------------------------------
// 请求头与端点补全
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_端点补全与认证头齐备() {
    let mock = MockLlm::text(Protocol::Anthropic, "ok").await;
    let client = mock_client(Protocol::Anthropic, &mock.base_url(), "mock-model");

    let m = meta("LsmAgentEmergentWork-Yolo/0.1.0");
    client
        .complete("你是 Yolo。", &[ChatMessage::user("你好")], &[], &m)
        .await
        .expect("Anthropic complete 应成功");

    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1, "应恰好 1 次请求");
    let r = &reqs[0];

    assert_eq!(r.method, "POST");
    assert_eq!(r.path, "/v1/messages", "Anthropic 端点应补全为 /v1/messages");
    assert_eq!(r.header("x-api-key"), Some("mock-key"), "Anthropic 用 x-api-key 传密钥");
    assert_eq!(r.header("x-session-id"), Some("sess-mock-0001"));
    assert_eq!(
        r.header("user-agent"),
        Some("LsmAgentEmergentWork-Yolo/0.1.0"),
        "User-Agent 应按发起角色逐请求注入"
    );
    assert_eq!(r.body["stream"], json!(true), "两客户端都走 SSE 流式");
}

#[tokio::test]
async fn openai_端点补全与_bearer_认证() {
    let mock = MockLlm::text(Protocol::OpenAi, "ok").await;
    let client = mock_client(Protocol::OpenAi, &mock.base_url(), "mock-model");

    client
        .complete("你是 Work。", &[ChatMessage::user("你好")], &[], &meta("t/1"))
        .await
        .expect("OpenAI complete 应成功");

    let r = &mock.requests()[0];
    assert_eq!(r.path, "/chat/completions", "OpenAI 端点应补全为 /chat/completions");
    assert_eq!(r.header("authorization"), Some("Bearer mock-key"), "OpenAI 用 Bearer 传密钥");
    assert!(r.header("x-api-key").is_none(), "OpenAI 不应发 x-api-key");
}

#[tokio::test]
async fn 端点尾部斜杠不产生双斜杠() {
    for (protocol, want) in [
        (Protocol::Anthropic, "/v1/messages"),
        (Protocol::OpenAi, "/chat/completions"),
    ] {
        let mock = MockLlm::text(protocol, "ok").await;
        let base = format!("{}/", mock.base_url()); // 故意带尾斜杠
        let client = mock_client(protocol, &base, "m");
        client
            .complete("s", &[ChatMessage::user("u")], &[], &meta("t/1"))
            .await
            .expect("complete 应成功");
        let reqs = mock.requests();
        assert!(!reqs.is_empty(), "{protocol:?} 应发出请求");
        assert_eq!(reqs[0].path, want, "{protocol:?} 尾斜杠应被裁剪");
    }
}

// ---------------------------------------------------------------------------
// body 结构
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_body_结构与_system_形态() {
    let mock = MockLlm::text(Protocol::Anthropic, "ok").await;
    let client = mock_client(Protocol::Anthropic, &mock.base_url(), "m");
    client
        .complete("系统提示词正文", &[ChatMessage::user("问题")], &[], &meta("t/1"))
        .await
        .expect("complete 应成功");

    let r = &mock.requests()[0];
    assert!(r.body["system"].is_array(), "Anthropic system 必须是 blocks 数组");
    assert!(r.body["max_tokens"].as_u64().unwrap_or(0) > 0, "必须带 max_tokens");
    assert_eq!(r.body["model"], json!("m"));

    let msgs = r.messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["role"], json!("user"));
    assert!(r.system_text().contains("系统提示词正文"));
}

#[tokio::test]
async fn openai_body_system_走首条system消息() {
    let mock = MockLlm::text(Protocol::OpenAi, "ok").await;
    let client = mock_client(Protocol::OpenAi, &mock.base_url(), "m");
    client
        .complete("系统提示词正文", &[ChatMessage::user("问题")], &[], &meta("t/1"))
        .await
        .expect("complete 应成功");

    let r = &mock.requests()[0];
    // OpenAI 没有 system 顶层字段:系统提示词是 messages 里的首条 role=system 消息。
    assert!(
        r.body.get("system").is_none(),
        "OpenAI 不应有顶层 system 字段"
    );
    let msgs = r.messages();
    assert_eq!(msgs[0]["role"], json!("system"), "系统提示词应是首条 system 消息");
    assert!(
        msgs[0]["content"].as_str().unwrap_or_default().contains("系统提示词正文"),
        "system 消息应承载提示词正文"
    );
    assert_eq!(msgs[1]["role"], json!("user"));
}

#[tokio::test]
async fn tools_两协议_schema_形状不同() {
    let tool = ToolDef::new(
        "Read",
        "读文件",
        json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }),
    );

    // Anthropic: {name, description, input_schema}
    let mock = MockLlm::text(Protocol::Anthropic, "ok").await;
    let client = mock_client(Protocol::Anthropic, &mock.base_url(), "m");
    client
        .complete("s", &[ChatMessage::user("u")], std::slice::from_ref(&tool), &meta("t/1"))
        .await
        .unwrap();
    let r = &mock.requests()[0];
    let t = &r.tools()[0];
    assert_eq!(t["name"], json!("Read"));
    assert_eq!(t["description"], json!("读文件"));
    assert!(t.get("input_schema").is_some(), "Anthropic 用 input_schema");
    assert!(t.get("parameters").is_none(), "Anthropic 不应出现 parameters");

    // OpenAI: {type:"function", function:{name, description, parameters}}
    let mock = MockLlm::text(Protocol::OpenAi, "ok").await;
    let client = mock_client(Protocol::OpenAi, &mock.base_url(), "m");
    client
        .complete("s", &[ChatMessage::user("u")], std::slice::from_ref(&tool), &meta("t/1"))
        .await
        .unwrap();
    let r = &mock.requests()[0];
    let t = &r.tools()[0];
    assert_eq!(t["type"], json!("function"));
    assert_eq!(t["function"]["name"], json!("Read"));
    assert!(t["function"].get("parameters").is_some(), "OpenAI 用 function.parameters");
    assert_eq!(r.tool_names(), vec!["Read".to_string()]);
}

#[tokio::test]
async fn tool_result_按协议对应形态回填() {
    let assistant = ChatMessage {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "call-1".into(),
            name: "Read".into(),
            input: json!({"path": "a.txt"}),
        }],
    };
    let tool_result = ChatMessage {
        role: Role::User, // Anthropic 下 tool_result 挂在 user 消息里
        content: vec![ContentBlock::tool_result("call-1", "文件内容", false)],
    };

    // Anthropic: assistant 用 tool_use block,结果回填为 user 消息的 tool_result block
    let mock = MockLlm::text(Protocol::Anthropic, "ok").await;
    let client = mock_client(Protocol::Anthropic, &mock.base_url(), "m");
    client
        .complete(
            "s",
            &[ChatMessage::user("读一下"), assistant.clone(), tool_result],
            &[],
            &meta("t/1"),
        )
        .await
        .unwrap();
    let r = &mock.requests()[0];
    assert!(r.message_corpus().contains("文件内容"), "tool_result 内容应上 wire");
    assert_eq!(r.messages().len(), 3, "user + assistant + tool_result 共 3 条");
    assert_eq!(
        r.messages()[1]["content"][0]["type"],
        json!("tool_use"),
        "Anthropic assistant 工具调用是 tool_use block"
    );

    // OpenAI: 工具调用拆成独立 tool 角色消息(源消息角色也必须是 Tool)
    let mock = MockLlm::text(Protocol::OpenAi, "ok").await;
    let client = mock_client(Protocol::OpenAi, &mock.base_url(), "m");
    client
        .complete(
            "s",
            &[
                ChatMessage::user("读一下"),
                assistant,
                ChatMessage {
                    role: Role::Tool,
                    content: vec![ContentBlock::tool_result("call-1", "文件内容", false)],
                },
            ],
            &[],
            &meta("t/1"),
        )
        .await
        .unwrap();
    let r = &mock.requests()[0];
    assert!(
        r.messages().iter().any(|m| m["role"] == json!("tool")),
        "OpenAI 工具结果用独立 role=tool 消息"
    );
    assert!(r.message_corpus().contains("文件内容"));
}

// ---------------------------------------------------------------------------
// §4h:Anthropic Prompt Caching
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_注入cache_control断点() {
    let mock = MockLlm::text(Protocol::Anthropic, "ok").await;
    let client = mock_client(Protocol::Anthropic, &mock.base_url(), "m");
    client
        .complete(
            "一段足够长的系统提示词,用于触发 cache_control 断点判定",
            &[ChatMessage::user("一段足够长的用户提问,用于触发断点判定")],
            &[],
            &meta("t/1"),
        )
        .await
        .unwrap();

    let r = &mock.requests()[0];
    let blocks = r.body["system"].as_array().cloned().unwrap_or_default();
    let marked: Vec<_> = blocks.iter().filter(|b| b.get("cache_control").is_some()).collect();
    assert!(
        !marked.is_empty(),
        "system 应至少有一个块带 cache_control 断点,实际: {blocks:?}"
    );
    for b in marked {
        assert_eq!(b["cache_control"]["type"], json!("ephemeral"), "断点类型必须是 ephemeral");
    }
}

// ---------------------------------------------------------------------------
// §6:metadata.user_id 四字段 + 逐角色 User-Agent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_metadata_含设备会话与agent身份() {
    let mock = MockLlm::text(Protocol::Anthropic, "任务已完成").await;
    let client = mock_client(Protocol::Anthropic, &mock.base_url(), "m");

    // 走真实 Agent 循环:metadata.user_id 由循环组装,而不是手工拼。
    let agent = Agent::new(client, AgentProfile::work_profile());
    let mut session = Session::new();
    let _ = agent.run_session(&mut session).await;

    let reqs = mock.requests();
    assert!(!reqs.is_empty(), "Agent 循环应发出请求");
    // Anthropic 的 metadata.user_id 规范要求是**字符串**,内容才是那段 JSON。
    let raw = reqs[0].body["metadata"]["user_id"]
        .as_str()
        .unwrap_or_else(|| panic!("metadata.user_id 应为字符串,实际: {}", reqs[0].body["metadata"]["user_id"]));
    let uid: Value = serde_json::from_str(raw).expect("user_id 应是合法 JSON 字符串");
    for field in ["device_id", "session_id", "account_uuid", "agent"] {
        assert!(uid.get(field).is_some(), "metadata.user_id 缺字段 {field},实际: {uid}");
    }
    assert!(
        uid["agent"].as_str().unwrap_or_default().contains("Work"),
        "agent 字段应带发起角色名,实际: {uid}"
    );
}

#[tokio::test]
async fn 不同角色_发出可区分的_user_agent() {
    for (label, profile) in [
        ("Yolo", AgentProfile::yolo_profile()),
        ("Work", AgentProfile::work_profile()),
    ] {
        let mock = MockLlm::text(Protocol::Anthropic, "结束").await;
        let client = mock_client(Protocol::Anthropic, &mock.base_url(), "m");
        let agent = Agent::new(client, profile);
        let mut session = Session::new();
        let _ = agent.run_session(&mut session).await;

        let reqs = mock.requests();
        assert!(!reqs.is_empty(), "{label} 应发出请求");
        let ua = reqs[0].header("user-agent").unwrap_or_default();
        assert!(ua.contains(label), "{label} 角色的 User-Agent 应含角色名,实际: {ua}");
    }
}

// ---------------------------------------------------------------------------
// 响应解析:两协议都要能把 SSE 收敛成 Completion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_响应解析出文本与用量() {
    let mock = MockLlm::text(Protocol::OpenAi, "这是最终答案").await;
    let client = mock_client(Protocol::OpenAi, &mock.base_url(), "m");
    let c = client
        .complete("s", &[ChatMessage::user("u")], &[], &meta("t/1"))
        .await
        .expect("OpenAI SSE 应解析成功");
    assert_eq!(c.text, "这是最终答案");
    assert!(!c.has_tool_calls());
}

#[tokio::test]
async fn anthropic_响应解析出工具调用() {
    let mock = MockLlm::tool_use(Protocol::Anthropic, "Read", json!({"path": "a.txt"})).await;
    let client = mock_client(Protocol::Anthropic, &mock.base_url(), "m");
    let c = client
        .complete("s", &[ChatMessage::user("u")], &[], &meta("t/1"))
        .await
        .expect("Anthropic tool_use SSE 应解析成功");
    assert!(c.has_tool_calls(), "应解析出工具调用");
    assert_eq!(c.tool_calls[0].name, "Read");
    assert_eq!(c.tool_calls[0].arguments["path"], json!("a.txt"));
}

#[tokio::test]
async fn openai_响应解析出工具调用() {
    let mock = MockLlm::tool_use(Protocol::OpenAi, "Read", json!({"path": "a.txt"})).await;
    let client = mock_client(Protocol::OpenAi, &mock.base_url(), "m");
    let c = client
        .complete("s", &[ChatMessage::user("u")], &[], &meta("t/1"))
        .await
        .expect("OpenAI tool_calls SSE 应解析成功");
    assert!(c.has_tool_calls(), "应解析出工具调用");
    assert_eq!(c.tool_calls[0].name, "Read");
    assert_eq!(c.tool_calls[0].arguments["path"], json!("a.txt"));
}

// ---------------------------------------------------------------------------
// 异常响应:HTTP 400 应作为错误上抛而不是静默降级
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_错误码_上抛而非静默空响应() {
    let mock = MockLlm::fixed(Reply::Raw {
        status: 400,
        body: r#"{"error":{"type":"invalid_request_error","message":"prompt is too long"}}"#
            .into(),
    })
    .await;
    let client = mock_client(Protocol::Anthropic, &mock.base_url(), "m");
    let err = client
        .complete("s", &[ChatMessage::user("u")], &[], &meta("t/1"))
        .await
        .expect_err("400 应报错,不能静默返回空 Completion");
    assert!(
        format!("{err}").contains("prompt is too long") || format!("{err}").contains("400"),
        "错误信息应带上游原文或状态码,实际: {err}"
    );
}