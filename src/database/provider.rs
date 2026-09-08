//! Provider 表的 CRUD 操作
//!
//! 包含基本的增删改查，以及导入/导出功能。

use rusqlite::{params, OptionalExtension};

use crate::database::models::{
    ExportData, ImportInput, ImportResult, Protocol, ProviderRecord,
};
use crate::database::{ConfigError, Db, Result};

impl Db {
    /// 新增一条记录;若库为空则自动激活
    pub fn add(
        &self,
        protocol: Protocol,
        provider_name: &str,
        model_name: &str,
        end_point: &str,
        api_key: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM providers", [], |r| r.get::<_, i64>(0))?;
        let activate = count == 0;
        conn.execute(
            "INSERT INTO providers(protocol, provider_name, model_name, end_point, api_key, is_active)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                protocol.as_str(),
                provider_name,
                model_name,
                end_point,
                api_key,
                if activate { 1 } else { 0 }
            ],
        )?;
        let id = conn.last_insert_rowid();
        Ok(id)
    }

    /// 插入一条记录（不自动激活），用于导入
    fn add_without_activate(
        &self,
        protocol: Protocol,
        provider_name: &str,
        model_name: &str,
        end_point: &str,
        api_key: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT OR IGNORE INTO providers(protocol, provider_name, model_name, end_point, api_key, is_active)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![
                protocol.as_str(),
                provider_name,
                model_name,
                end_point,
                api_key,
            ],
        )?;
        let id = conn.last_insert_rowid();
        Ok(id)
    }

    pub fn list(&self) -> Result<Vec<ProviderRecord>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, protocol, provider_name, model_name, end_point, api_key, is_active, created_at
             FROM providers ORDER BY id ASC",
        )?;
        let iter = stmt.query_map([], row_to_record)?;
        let mut out = Vec::new();
        for r in iter {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_active(&self) -> Result<Option<ProviderRecord>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let row = conn
            .query_row(
                "SELECT id, protocol, provider_name, model_name, end_point, api_key, is_active, created_at
                 FROM providers WHERE is_active = 1 LIMIT 1",
                [],
                row_to_record,
            )
            .optional()?;
        Ok(row)
    }

    pub fn get(&self, id: i64) -> Result<ProviderRecord> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let row = conn
            .query_row(
                "SELECT id, protocol, provider_name, model_name, end_point, api_key, is_active, created_at
                 FROM providers WHERE id = ?1",
                params![id],
                row_to_record,
            )
            .optional()?;
        row.ok_or(ConfigError::NotFound(id))
    }

    /// 把指定 id 设为唯一激活;其他全部清零
    pub fn set_active(&self, id: i64) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex poisoned");
        let tx = conn.transaction()?;
        let updated = tx.execute("UPDATE providers SET is_active = 0", [])?;
        let target = tx.execute(
            "UPDATE providers SET is_active = 1 WHERE id = ?1",
            params![id],
        )?;
        if target == 0 {
            return Err(ConfigError::NotFound(id));
        }
        let _ = updated;
        tx.commit()?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let n = conn.execute("DELETE FROM providers WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(ConfigError::NotFound(id));
        }
        Ok(())
    }

    /// 检查是否存在重复记录（基于 UNIQUE 约束字段）
    pub fn exists(
        &self,
        protocol: Protocol,
        provider_name: &str,
        model_name: &str,
        end_point: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE protocol = ?1 AND provider_name = ?2 AND model_name = ?3 AND end_point = ?4",
            params![protocol.as_str(), provider_name, model_name, end_point],
            |r| r.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }

    // ===== 导入/导出功能 =====

    /// 从 JSON 字符串导入 Provider 配置
    pub fn import_from_json(&self, json: &str) -> Result<ImportResult> {
        let input: ImportInput = serde_json::from_str(json)
            .map_err(|e| ConfigError::Import(format!("JSON 解析失败: {e}")))?;

        let items = input.into_vec();
        if items.is_empty() {
            return Err(ConfigError::Import("导入数据为空".to_string()));
        }

        let mut result = ImportResult::default();

        for item in items {
            // 验证 protocol
            let protocol = match Protocol::parse(&item.protocol) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("⚠ 跳过无效记录: protocol='{}' 错误: {}", item.protocol, e);
                    result.failed += 1;
                    continue;
                }
            };

            // 验证必尝字段不为空
            if item.provider_name.trim().is_empty()
                || item.model_name.trim().is_empty()
                || item.end_point.trim().is_empty()
                || item.api_key.trim().is_empty()
            {
                eprintln!(
                    "⚠ 跳过无效记录: provider_name='{}' 存在空字段",
                    item.provider_name
                );
                result.failed += 1;
                continue;
            }

            // 检查是否已存在
            if self.exists(protocol, &item.provider_name, &item.model_name, &item.end_point)? {
                println!(
                    "⊘ 跳过重复: {}/{}/{}",
                    item.protocol, item.provider_name, item.model_name
                );
                result.skipped += 1;
                continue;
            }

            // 插入记录
            match self.add_without_activate(
                protocol,
                &item.provider_name,
                &item.model_name,
                &item.end_point,
                &item.api_key,
            ) {
                Ok(id) => {
                    println!(
                        "✓ 导入成功 id={}: {}/{}/{}",
                        id, item.protocol, item.provider_name, item.model_name
                    );
                    result.success += 1;
                }
                Err(e) => {
                    eprintln!(
                        "✗ 导入失败: {}/{}/{} 错误: {}",
                        item.protocol, item.provider_name, item.model_name, e
                    );
                    result.failed += 1;
                }
            }
        }

        Ok(result)
    }

    /// 导出所有 Provider 为 JSON 字符串
    pub fn export_to_json(&self) -> Result<String> {
        let records = self.list()?;
        let export_data = ExportData::from_records(records);
        let json = serde_json::to_string_pretty(&export_data)
            .map_err(|e| ConfigError::Export(format!("JSON 序列化失败: {e}")))?;
        Ok(json)
    }
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderRecord> {
    let proto_str: String = row.get(1)?;
    let protocol = match proto_str.as_str() {
        "anthropic" => Protocol::Anthropic,
        "openai" => Protocol::OpenAi,
        // CHECK 约束保证只会是这两个值
        _ => unreachable!("unknown protocol in db: {proto_str}"),
    };
    let is_active_int: i64 = row.get(6)?;
    Ok(ProviderRecord {
        id: row.get(0)?,
        protocol,
        provider_name: row.get(2)?,
        model_name: row.get(3)?,
        end_point: row.get(4)?,
        api_key: row.get(5)?,
        is_active: is_active_int != 0,
        created_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Paths;

    fn fresh_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (db, dir)
    }

    #[test]
    fn import_single_object() {
        let (db, _d) = fresh_db();
        let json = r#"{
            "protocol": "anthropic",
            "provider_name": "Test",
            "model_name": "claude-3",
            "end_point": "https://api.anthropic.com",
            "api_key": "sk-test-1234"
        }"#;

        let result = db.import_from_json(json).unwrap();
        assert_eq!(result.success, 1);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);

        let records = db.list().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].provider_name, "Test");
    }

    #[test]
    fn import_batch_array() {
        let (db, _d) = fresh_db();
        let json = r#"[
            {
                "protocol": "anthropic",
                "provider_name": "P1",
                "model_name": "m1",
                "end_point": "https://a",
                "api_key": "k1"
            },
            {
                "protocol": "openai",
                "provider_name": "P2",
                "model_name": "m2",
                "end_point": "https://b",
                "api_key": "k2"
            }
        ]"#;

        let result = db.import_from_json(json).unwrap();
        assert_eq!(result.success, 2);
        assert_eq!(db.list().unwrap().len(), 2);
    }

    #[test]
    fn import_dedup() {
        let (db, _d) = fresh_db();
        let json1 = r#"{
            "protocol": "anthropic",
            "provider_name": "P1",
            "model_name": "m1",
            "end_point": "https://a",
            "api_key": "k1"
        }"#;
        let json2 = r#"{
            "protocol": "anthropic",
            "provider_name": "P1",
            "model_name": "m1",
            "end_point": "https://a",
            "api_key": "k1-different-key"
        }"#;

        db.import_from_json(json1).unwrap();
        let result = db.import_from_json(json2).unwrap();

        assert_eq!(result.success, 0);
        assert_eq!(result.skipped, 1);
        assert_eq!(db.list().unwrap().len(), 1);
    }

    #[test]
    fn import_invalid_protocol() {
        let (db, _d) = fresh_db();
        let json = r#"{
            "protocol": "invalid",
            "provider_name": "P1",
            "model_name": "m1",
            "end_point": "https://a",
            "api_key": "k1"
        }"#;

        let result = db.import_from_json(json).unwrap();
        assert_eq!(result.failed, 1);
        assert_eq!(result.success, 0);
    }

    #[test]
    fn import_empty_fields() {
        let (db, _d) = fresh_db();
        let json = r#"{
            "protocol": "anthropic",
            "provider_name": "",
            "model_name": "m1",
            "end_point": "https://a",
            "api_key": "k1"
        }"#;

        let result = db.import_from_json(json).unwrap();
        assert_eq!(result.failed, 1);
        assert_eq!(result.success, 0);
    }

    #[test]
    fn export_roundtrip() {
        let (db, _d) = fresh_db();
        db.add(Protocol::Anthropic, "P1", "m1", "https://a", "k1")
            .unwrap();
        db.add(Protocol::OpenAi, "P2", "m2", "https://b", "k2")
            .unwrap();

        let json = db.export_to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["version"], "1.0");
        assert_eq!(parsed["count"], 2);
        assert_eq!(parsed["providers"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn export_empty() {
        let (db, _d) = fresh_db();
        let json = db.export_to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["count"], 0);
        assert!(parsed["providers"].as_array().unwrap().is_empty());
    }
}
