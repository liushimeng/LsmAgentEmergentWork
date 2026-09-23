//! MCP server 接入记录 DAO(2026-09-23 第 123 轮)。
//!
//! SQLite `mcp_servers` 表 CRUD:`laew mcp add|list|del|test` 与 `MCP_Use` 工具共用。
//! 敏感请求头(headers)经 `Vault`(AES-256-GCM)加密落 `headers_enc`(对齐 providers.api_key
//! 的 D9-4 先例)。设计见 `docs/MCP_Use/01-设计与解决方案.md` §5。
//!
//! 表 DDL 见 [`crate::database::schema`] 的 `mcp_servers` 建表段。

use rusqlite::params;

use super::{ConfigError, Db, Result};

/// 一条 MCP server 接入记录(读出时 headers 已解密)。
#[derive(Debug, Clone, Default)]
pub struct McpServerRecord {
    pub id: i64,
    pub name: String,
    /// `stdio` / `http`。
    pub transport: String,
    /// stdio:可执行文件。
    pub command: Option<String>,
    /// stdio:argv(JSON 数组字符串,原样保存)。
    pub args: String,
    /// stdio:子进程环境(JSON 对象字符串)。
    pub env_json: String,
    /// http:Streamable HTTP 端点。
    pub url: Option<String>,
    /// http:自定义请求头(JSON 对象字符串,已解密;无则空对象)。
    pub headers_json: String,
    pub enabled: bool,
    pub timeout_ms: i64,
    pub created_at: String,
}

/// 写入条目(`add_mcp_server` 入参)。
#[derive(Debug, Clone, Default)]
pub struct McpServerEntry {
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: String,
    pub env_json: String,
    pub url: Option<String>,
    pub headers_json: String,
    pub timeout_ms: Option<i64>,
}

impl McpServerEntry {
    pub const DEFAULT_TIMEOUT_MS: i64 = 30_000;

    /// 写入前校验(transport 专属必填 + JSON 字段形态)。
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(ConfigError::Validation("name 不能为空".into()));
        }
        match self.transport.as_str() {
            "stdio" => {
                if self.command.as_deref().map(str::trim).unwrap_or("").is_empty() {
                    return Err(ConfigError::Validation(
                        "transport=stdio 必须提供 --command".into(),
                    ));
                }
            }
            "http" => {
                if self.url.as_deref().map(str::trim).unwrap_or("").is_empty() {
                    return Err(ConfigError::Validation(
                        "transport=http 必须提供 --url".into(),
                    ));
                }
            }
            other => {
                return Err(ConfigError::Validation(format!(
                    "transport 仅支持 stdio / http,got: {other}"
                )));
            }
        }
        if !self.args.trim().is_empty() {
            serde_json::from_str::<serde_json::Value>(&self.args).map_err(|e| {
                ConfigError::Validation(format!("args 不是合法 JSON 数组: {e}"))
            })?;
        }
        if !self.env_json.trim().is_empty() {
            serde_json::from_str::<serde_json::Value>(&self.env_json).map_err(|e| {
                ConfigError::Validation(format!("env 不是合法 JSON 对象: {e}"))
            })?;
        }
        Ok(())
    }
}

impl Db {
    /// 新增一条 MCP server 记录(返回自增 id)。
    pub fn add_mcp_server(&self, entry: &McpServerEntry) -> Result<i64> {
        entry.validate()?;
        let conn = self.conn.lock().expect("db mutex poisoned");
        // 敏感请求头 Vault 加密;空头不落密文。
        let headers_enc = if entry.headers_json.trim().is_empty()
            || entry.headers_json.trim() == "{}"
        {
            None
        } else {
            Some(crate::agent::safety::Vault::global()?.encrypt(&entry.headers_json)?)
        };
        conn.execute(
            "INSERT INTO mcp_servers(name, transport, command, args, env_json, url, headers_enc, timeout_ms, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now','localtime'))",
            params![
                entry.name.trim(),
                entry.transport.trim(),
                entry.command,
                if entry.args.trim().is_empty() { "[]" } else { entry.args.trim() },
                if entry.env_json.trim().is_empty() { "{}" } else { entry.env_json.trim() },
                entry.url,
                headers_enc,
                entry.timeout_ms.unwrap_or(McpServerEntry::DEFAULT_TIMEOUT_MS),
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 全量列表(按 id 升序)。
    pub fn list_mcp_servers(&self) -> Result<Vec<McpServerRecord>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, transport, command, args, env_json, url, headers_enc, enabled, timeout_ms, created_at
             FROM mcp_servers ORDER BY id ASC",
        )?;
        let iter = stmt.query_map([], row_to_mcp_server)?;
        let mut out = Vec::new();
        for r in iter {
            out.push(r?);
        }
        Ok(out)
    }

    /// 按 id 或 name 查找(id 字符串可解析为数字时按 id,否则按 name)。
    pub fn get_mcp_server(&self, id_or_name: &str) -> Result<McpServerRecord> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let by_id = id_or_name.trim().parse::<i64>().ok();
        let row = if let Some(id) = by_id {
            conn.query_row(
                "SELECT id, name, transport, command, args, env_json, url, headers_enc, enabled, timeout_ms, created_at
                 FROM mcp_servers WHERE id = ?1",
                params![id],
                row_to_mcp_server,
            )
        } else {
            conn.query_row(
                "SELECT id, name, transport, command, args, env_json, url, headers_enc, enabled, timeout_ms, created_at
                 FROM mcp_servers WHERE name = ?1",
                params![id_or_name.trim()],
                row_to_mcp_server,
            )
        };
        row.map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                ConfigError::Validation(format!("MCP server 不存在: {id_or_name}"))
            }
            other => ConfigError::from(other),
        })
    }

    /// 按 name 查找(工具门面 `MCP_Use` 用;不存在返回 `None`)。
    pub fn get_mcp_server_by_name(&self, name: &str) -> Result<Option<McpServerRecord>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let row = conn
            .query_row(
                "SELECT id, name, transport, command, args, env_json, url, headers_enc, enabled, timeout_ms, created_at
                 FROM mcp_servers WHERE name = ?1",
                params![name.trim()],
                row_to_mcp_server,
            )
            .optional()?;
        Ok(row)
    }

    /// 删除(id 或 name)。
    pub fn delete_mcp_server(&self, id_or_name: &str) -> Result<()> {
        let rec = self.get_mcp_server(id_or_name)?;
        let conn = self.conn.lock().expect("db mutex poisoned");
        let n = conn.execute("DELETE FROM mcp_servers WHERE id = ?1", params![rec.id])?;
        if n == 0 {
            return Err(ConfigError::Validation(format!(
                "MCP server 不存在: {id_or_name}"
            )));
        }
        Ok(())
    }
}

use rusqlite::OptionalExtension;

fn row_to_mcp_server(row: &rusqlite::Row<'_>) -> rusqlite::Result<McpServerRecord> {
    let id: i64 = row.get(0)?;
    let headers_enc: Option<String> = row.get(7)?;
    // headers 解密失败降级空对象 + 告警,不崩整个列表(对齐 providers.api_key 处置)。
    let headers_json = match headers_enc {
        None => "{}".to_string(),
        Some(enc) => {
            if crate::agent::safety::Vault::is_encrypted(&enc) {
                match crate::agent::safety::Vault::global().and_then(|v| v.decrypt(&enc)) {
                    Ok(plain) => plain,
                    Err(e) => {
                        tracing::warn!(mcp_id = id, error = %e, "MCP headers 解密失败,已降级为空对象");
                        "{}".to_string()
                    }
                }
            } else {
                enc
            }
        }
    };
    let enabled: i64 = row.get(8)?;
    let timeout_ms: i64 = row.get(9)?;
    Ok(McpServerRecord {
        id,
        name: row.get(1)?,
        transport: row.get(2)?,
        command: row.get(3)?,
        args: row.get(4)?,
        env_json: row.get(5)?,
        url: row.get(6)?,
        headers_json,
        enabled: enabled != 0,
        timeout_ms,
        created_at: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::paths::Paths;
    use tempfile::tempdir;

    fn test_db() -> (tempfile::TempDir, Db) {
        let dir = tempdir().unwrap();
        let db = Db::open(&Paths::for_test(dir.path())).unwrap();
        (dir, db)
    }

    fn stdio_entry(name: &str) -> McpServerEntry {
        McpServerEntry {
            name: name.into(),
            transport: "stdio".into(),
            command: Some("npx".into()),
            args: r#"["-y","@mock/mcp"]"#.into(),
            env_json: r#"{"FOO":"bar"}"#.into(),
            url: None,
            headers_json: "{}".into(),
            timeout_ms: Some(15_000),
        }
    }

    #[test]
    fn add_list_get_delete_roundtrip() {
        let (_dir, db) = test_db();
        let id = db.add_mcp_server(&stdio_entry("mockA")).unwrap();
        assert!(id > 0);

        let list = db.list_mcp_servers().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "mockA");
        assert_eq!(list[0].transport, "stdio");
        assert_eq!(list[0].command.as_deref(), Some("npx"));
        assert!(list[0].enabled);
        assert_eq!(list[0].timeout_ms, 15_000);

        let by_name = db.get_mcp_server("mockA").unwrap();
        assert_eq!(by_name.id, id);
        let by_id = db.get_mcp_server(&id.to_string()).unwrap();
        assert_eq!(by_id.name, "mockA");

        db.delete_mcp_server("mockA").unwrap();
        assert!(db.list_mcp_servers().unwrap().is_empty());
    }

    #[test]
    fn validate_rejects_missing_command_or_url() {
        let (_dir, db) = test_db();
        let mut e = stdio_entry("bad");
        e.command = None;
        assert!(db.add_mcp_server(&e).is_err(), "stdio 缺 command 必须拒绝");

        let e2 = McpServerEntry {
            name: "bad2".into(),
            transport: "http".into(),
            url: None,
            ..Default::default()
        };
        assert!(db.add_mcp_server(&e2).is_err(), "http 缺 url 必须拒绝");

        let e3 = McpServerEntry {
            name: "bad3".into(),
            transport: "ws".into(),
            ..Default::default()
        };
        assert!(db.add_mcp_server(&e3).is_err(), "未知 transport 必须拒绝");
    }

    #[test]
    fn headers_encrypted_roundtrip() {
        let (_dir, db) = test_db();
        let e = McpServerEntry {
            name: "remote".into(),
            transport: "http".into(),
            command: None,
            args: String::new(),
            env_json: String::new(),
            url: Some("https://mcp.example.com/sse".into()),
            headers_json: r#"{"Authorization":"Bearer sk-secret-1234"}"#.into(),
            timeout_ms: None,
        };
        db.add_mcp_server(&e).unwrap();

        // 落库形态必须是密文(不能在库里看到明文 token)。
        let raw: String = {
            use rusqlite::OptionalExtension;
            let conn = db.conn.lock().unwrap();
            conn.query_row(
                "SELECT headers_enc FROM mcp_servers WHERE name='remote'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .unwrap()
            .unwrap()
        };
        assert!(!raw.contains("sk-secret-1234"), "headers 必须加密落库");
        assert!(crate::agent::safety::Vault::is_encrypted(&raw));

        // 读出解密回原文。
        let rec = db.get_mcp_server("remote").unwrap();
        assert!(rec.headers_json.contains("sk-secret-1234"));
    }

    #[test]
    fn duplicate_name_rejected() {
        let (_dir, db) = test_db();
        db.add_mcp_server(&stdio_entry("dup")).unwrap();
        assert!(db.add_mcp_server(&stdio_entry("dup")).is_err());
    }

    #[test]
    fn old_db_gains_mcp_servers_table_via_init_schema() {
        // init_schema 全量 IF NOT EXISTS:旧库打开后新表自动存在(零迁移成本)。
        let (_dir, db) = test_db();
        assert!(db.list_mcp_servers().unwrap().is_empty());
    }
}
