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
        Ok(self
            .find_id(protocol, provider_name, model_name, end_point)?
            .is_some())
    }

    /// 按 UNIQUE 四元组查找记录 id
    fn find_id(
        &self,
        protocol: Protocol,
        provider_name: &str,
        model_name: &str,
        end_point: &str,
    ) -> Result<Option<i64>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let id = conn
            .query_row(
                "SELECT id FROM providers WHERE protocol = ?1 AND provider_name = ?2 AND model_name = ?3 AND end_point = ?4",
                params![protocol.as_str(), provider_name, model_name, end_point],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        Ok(id)
    }

    // ===== 导入/导出功能 =====

    /// 从 JSON 字符串导入 Provider 配置
    ///
    /// 支持三种输入: 单条对象 / 对象数组 / `--outprovider` 导出信封(`{"providers":[...]}`)。
    /// 激活策略: 文件中带 `is_active: true` 的记录(导出信封)导入后恢复为激活;
    /// 否则若库中当前无任何激活记录, 自动激活本次第一条成功导入的记录, 保证导入即可用。
    pub fn import_from_json(&self, json: &str) -> Result<ImportResult> {
        let input: ImportInput = serde_json::from_str(json)
            .map_err(|e| ConfigError::Import(format!(
                "JSON 解析失败: {e}(要求: 单条对象 / 对象数组 / 导出格式 {{\"providers\":[...]}}, 每条须含 protocol/provider_name/model_name/end_point/api_key)"
            )))?;

        let items = input.into_vec();
        if items.is_empty() {
            return Err(ConfigError::Import("导入数据为空".to_string()));
        }

        let mut result = ImportResult::default();
        // 本次首条成功导入的记录 id(用于空库兜底激活)
        let mut first_success_id: Option<i64> = None;
        // 文件中标记 is_active=true 的记录坐标(用于恢复激活态)
        let mut desired_active: Option<(Protocol, String, String, String)> = None;

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

            // 验证必填字段不为空
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

            if item.is_active == Some(true) {
                desired_active = Some((
                    protocol,
                    item.provider_name.clone(),
                    item.model_name.clone(),
                    item.end_point.clone(),
                ));
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
                    if first_success_id.is_none() {
                        first_success_id = Some(id);
                    }
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

        // 激活策略: 优先恢复文件声明的 is_active; 否则空库兜底激活首条导入记录
        if let Some((protocol, provider_name, model_name, end_point)) = desired_active {
            match self.find_id(protocol, &provider_name, &model_name, &end_point)? {
                Some(id) => {
                    self.set_active(id)?;
                    println!("★ 已按文件声明激活 id={id}: {provider_name}/{model_name}");
                }
                None => {
                    eprintln!("⚠ 文件声明的激活记录 {provider_name}/{model_name} 未能导入, 跳过激活");
                }
            }
        } else if self.get_active()?.is_none() {
            if let Some(id) = first_success_id {
                self.set_active(id)?;
                println!("★ 库中无激活记录, 已自动激活首条导入记录 id={id}");
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

    #[test]
    fn import_export_envelope_roundtrip() {
        // 导出 → 再导入到空库: 记录全量恢复, 且激活态被还原
        let (src, _d1) = fresh_db();
        let id1 = src.add(Protocol::Anthropic, "P1", "m1", "https://a", "k1").unwrap();
        src.add(Protocol::OpenAi, "P2", "m2", "https://b", "k2").unwrap();
        src.set_active(id1).unwrap();
        let json = src.export_to_json().unwrap();

        let (dst, _d2) = fresh_db();
        let result = dst.import_from_json(&json).unwrap();
        assert_eq!(result.success, 2);
        assert_eq!(result.failed, 0);
        assert_eq!(dst.list().unwrap().len(), 2);
        let active = dst.get_active().unwrap().expect("导入后应有激活记录");
        assert_eq!(active.provider_name, "P1");
    }

    #[test]
    fn import_activates_first_when_no_active() {
        // 空库导入无 is_active 标记的配置: 自动激活首条成功记录
        let (db, _d) = fresh_db();
        let json = r#"{
            "protocol": "anthropic",
            "provider_name": "P1",
            "model_name": "m1",
            "end_point": "https://a",
            "api_key": "k1"
        }"#;
        db.import_from_json(json).unwrap();
        let active = db.get_active().unwrap().expect("空库导入应自动激活");
        assert_eq!(active.provider_name, "P1");
    }

    #[test]
    fn import_keeps_existing_active_when_unmarked() {
        // 已有激活记录时, 导入无 is_active 标记的配置不得改变激活态
        let (db, _d) = fresh_db();
        let id = db.add(Protocol::Anthropic, "Old", "m0", "https://old", "k0").unwrap();
        db.set_active(id).unwrap();
        let json = r#"{
            "protocol": "openai",
            "provider_name": "New",
            "model_name": "m1",
            "end_point": "https://n",
            "api_key": "k1"
        }"#;
        db.import_from_json(json).unwrap();
        let active = db.get_active().unwrap().unwrap();
        assert_eq!(active.provider_name, "Old");
    }

    #[test]
    fn import_envelope_restores_active_on_duplicate() {
        // 信封中标记激活的记录即使因重复被跳过, 也应恢复为激活
        let (src, _d1) = fresh_db();
        src.add(Protocol::Anthropic, "P1", "m1", "https://a", "k1").unwrap();
        let id2 = src.add(Protocol::OpenAi, "P2", "m2", "https://b", "k2").unwrap();
        src.set_active(id2).unwrap();
        let json = src.export_to_json().unwrap();

        let (dst, _d2) = fresh_db();
        dst.add(Protocol::Anthropic, "P1", "m1", "https://a", "k-other").unwrap();
        dst.add(Protocol::OpenAi, "P2", "m2", "https://b", "k-other").unwrap();
        let result = dst.import_from_json(&json).unwrap();
        assert_eq!(result.skipped, 2);
        let active = dst.get_active().unwrap().expect("应恢复激活态");
        assert_eq!(active.provider_name, "P2");
    }
}
