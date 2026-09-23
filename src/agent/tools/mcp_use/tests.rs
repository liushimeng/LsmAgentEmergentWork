//! MCP_Use 工具单测:Schema/description 契约 + 参数校验信封 + 配置降级路径。
//!
//! 纯参数路径不碰真实网络/子进程;真实 stdio 连通由 `testReport/run_e2e.sh` 的
//! `scripts/mock_mcp_server.py` 用例覆盖。

use serde_json::json;

use super::*;
use crate::agent::tools::Tool;
use crate::database::mcp_server::McpServerEntry;
use crate::database::paths::Paths;
use crate::database::Db;

fn envelope_of(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("工具输出应为合法 JSON 信封")
}

/// 每个用例前清掉 DB 覆盖(防跨用例污染)。
fn reset_db() {
    set_test_db(None);
}

fn temp_db() -> (tempfile::TempDir, Arc<Db>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&Paths::for_test(dir.path())).unwrap());
    set_test_db(Some(db.clone()));
    (dir, db)
}

// ===================== Schema / description 契约 =====================

#[test]
fn schema_action_enum_matches_action_names() {
    let _lock = test_db_lock();
    // 防清单漂移:Schema enum 必须与实现常量逐字对齐(对齐 SubAgent 先例)。
    let tool = McpUseTool;
    let schema = tool.parameters();
    let actions: Vec<&str> = schema["properties"]["action"]["enum"]
        .as_array()
        .expect("action.enum")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(actions, action_names().to_vec(), "action 枚举应与实现一致");
    assert_eq!(schema["required"][0], "action");
    assert_eq!(schema["additionalProperties"], false);
    // 动态值禁止进 enum(否则 tool_schema_validator 硬拒自定义 server/tool 名)。
    for key in ["server", "tool", "uri"] {
        assert!(
            schema["properties"][key].get("enum").is_none(),
            "{key} 必须是自由字符串"
        );
    }
    // arguments 必须是自由对象(MCP 工具入参任意 JSON)。
    assert_eq!(schema["properties"]["arguments"]["type"], "object");
    assert_eq!(
        schema["properties"]["arguments"]["additionalProperties"],
        true,
        "arguments 必须允许任意键(MCP inputSchema 是任意 JSON Schema)"
    );
}

#[test]
fn description_covers_usage_error_codes_and_safety() {
    let _lock = test_db_lock();
    let desc = McpUseTool.description();
    for needle in [
        "list_servers",
        "list_tools",
        "call_tool",
        "list_resources",
        "read_resource",
        "close",
        "inputSchema",
        "5001",
        "5005",
        "标准作业顺序",
        "安全红线",
        "laew mcp add",
    ] {
        assert!(desc.contains(needle), "description 应含 `{needle}`");
    }
}

#[test]
fn parallel_safe_is_false() {
    let _lock = test_db_lock();
    // 外部进程/HTTP 会话共享连接句柄,批次内必须保序串行。
    assert!(!McpUseTool.parallel_safe(&json!({"action": "list_tools"})));
}

// ===================== 参数校验信封(纯参数路径) =====================

#[tokio::test]
async fn missing_action_returns_1001() {
    let _lock = test_db_lock();
    reset_db();
    let out = McpUseTool.execute(json!({})).await.unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 1001);
    assert!(v["message"].as_str().unwrap().contains("缺少 action"));
}

#[tokio::test]
async fn unknown_action_returns_1001_with_action_list() {
    let _lock = test_db_lock();
    reset_db();
    let out = McpUseTool.execute(json!({"action": "frobnicate"})).await.unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 1001);
    assert!(v["message"].as_str().unwrap().contains("未知 action"));
    assert!(v["data"]["actions"].as_array().is_some());
}

#[tokio::test]
async fn call_tool_missing_tool_returns_1001() {
    let _lock = test_db_lock();
    reset_db();
    let out = McpUseTool
        .execute(json!({"action": "call_tool", "server": "s"}))
        .await
        .unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 1001);
    assert!(v["message"].as_str().unwrap().contains("缺少 tool"));
}

#[tokio::test]
async fn call_tool_non_object_arguments_returns_1001() {
    let _lock = test_db_lock();
    reset_db();
    let out = McpUseTool
        .execute(json!({"action": "call_tool", "server": "s", "tool": "t", "arguments": "str"}))
        .await
        .unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 1001);
}

#[tokio::test]
async fn read_resource_missing_uri_returns_1001() {
    let _lock = test_db_lock();
    reset_db();
    let out = McpUseTool
        .execute(json!({"action": "read_resource", "server": "s"}))
        .await
        .unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 1001);
    assert!(v["message"].as_str().unwrap().contains("缺少 uri"));
}

#[tokio::test]
async fn missing_server_returns_5001() {
    let _lock = test_db_lock();
    reset_db();
    let out = McpUseTool.execute(json!({"action": "list_tools"})).await.unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 5001);
    assert!(v["message"].as_str().unwrap().contains("缺少 server"));
}

#[tokio::test]
async fn unknown_server_returns_5001() {
    let _lock = test_db_lock();
    let (_dir, _db) = temp_db();
    let out = McpUseTool
        .execute(json!({"action": "list_tools", "server": "nope"}))
        .await
        .unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 5001);
    assert!(v["message"].as_str().unwrap().contains("nope"));
}

#[tokio::test]
async fn disabled_server_returns_5001() {
    let _lock = test_db_lock();
    let (_dir, db) = temp_db();
    db.add_mcp_server(&McpServerEntry {
        name: "off1".into(),
        transport: "stdio".into(),
        command: Some("true".into()),
        ..Default::default()
    })
    .unwrap();
    // 直接改库置 enabled=0(没有 enable/disable API,用 SQL)。
    {
        let conn = db.conn.lock().unwrap();
        conn.execute("UPDATE mcp_servers SET enabled = 0 WHERE name = 'off1'", [])
            .unwrap();
    }
    let out = McpUseTool
        .execute(json!({"action": "list_tools", "server": "off1"}))
        .await
        .unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 5001);
    assert!(v["message"].as_str().unwrap().contains("禁用"));
}

// ===================== list_servers 优雅降级 =====================

#[tokio::test]
async fn list_servers_without_db_returns_empty_ok() {
    let _lock = test_db_lock();
    reset_db(); // 显式「数据库不可用」
    let out = McpUseTool.execute(json!({"action": "list_servers"})).await.unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 0, "list_servers 永不失败(降级空列表)");
    assert_eq!(v["data"]["servers"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn list_servers_shows_configured_entries() {
    let _lock = test_db_lock();
    let (_dir, db) = temp_db();
    db.add_mcp_server(&McpServerEntry {
        name: "mockA".into(),
        transport: "stdio".into(),
        command: Some("npx".into()),
        args: r#"["-y","mock"]"#.into(),
        ..Default::default()
    })
    .unwrap();
    db.add_mcp_server(&McpServerEntry {
        name: "mockB".into(),
        transport: "http".into(),
        url: Some("https://mcp.example.com".into()),
        ..Default::default()
    })
    .unwrap();
    let out = McpUseTool.execute(json!({"action": "list_servers"})).await.unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 0);
    let servers = v["data"]["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0]["name"], "mockA");
    assert_eq!(servers[0]["transport"], "stdio");
    assert_eq!(servers[1]["name"], "mockB");
    assert_eq!(servers[1]["status"], "idle", "未连接时状态为 idle");
}

// ===================== close 幂等 =====================

#[tokio::test]
async fn close_without_connection_is_idempotent_ok() {
    let _lock = test_db_lock();
    reset_db();
    let out = McpUseTool
        .execute(json!({"action": "close", "server": "whatever"}))
        .await
        .unwrap();
    let v = envelope_of(&out);
    assert_eq!(v["code"], 0, "close 幂等:无连接也返回成功");
    assert_eq!(v["data"]["closed"], false);
}

// ===================== record → config 转换 =====================

#[test]
fn record_to_config_parses_json_fields() {
    let _lock = test_db_lock();
    let rec = McpServerRecord {
        name: "x".into(),
        transport: "stdio".into(),
        command: Some("node".into()),
        args: r#"["server.js","--port","8080"]"#.into(),
        env_json: r#"{"KEY":"VAL"}"#.into(),
        timeout_ms: 12_000,
        ..Default::default()
    };
    let cfg = record_to_config(&rec);
    assert_eq!(cfg.command.as_deref(), Some("node"));
    assert_eq!(cfg.args, vec!["server.js", "--port", "8080"]);
    assert_eq!(cfg.env, vec![("KEY".to_string(), "VAL".to_string())]);
    assert_eq!(cfg.timeout_ms, 12_000);

    // 脏 JSON 降级为空,不 panic。
    let dirty = McpServerRecord {
        args: "not json".into(),
        env_json: "also not".into(),
        headers_json: "{{{".into(),
        ..rec.clone()
    };
    let cfg2 = record_to_config(&dirty);
    assert!(cfg2.args.is_empty());
    assert!(cfg2.env.is_empty());
    assert!(cfg2.headers.is_empty());
}

// ===================== 开关 =====================

#[test]
fn mcp_use_enabled_by_default() {
    let _lock = test_db_lock();
    // 未设 LAEW_MCP_ENABLED 时默认开启(测试进程不应被外部环境污染成 off;
    // 若失败请检查环境变量)。
    assert!(mcp_use_enabled() || std::env::var("LAEW_MCP_ENABLED").is_ok());
}
