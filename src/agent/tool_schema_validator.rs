//! 工具参数 Schema 预校验器。
//!
//! 在工具执行前对 LLM 输出的 `tool_call.arguments` 做一次轻量级 JSON Schema 校验,
//! 提前拦截类型错误 / 缺少必填字段 / 越界值等问题,给出结构化错误信息供 LLM 自我修复。
//!
//! 对应知识库 gap L16(第七轮结构化输出专题)。
//!
//! 支持的 Schema 子集:`type` / `required` / `properties` / `additionalProperties` /
//! `enum` / `minimum` / `maximum` / `minLength` / `maxLength` / `items`。
//! 未识别的关键字静默忽略,Schema 格式异常时降级为不校验(保持向后兼容)。

use serde_json::Value;

use crate::error::{AgentError, Result};

/// 校验工具参数是否符合其声明的 JSON Schema。
///
/// # 参数
/// - `tool_name`:工具名(用于错误信息)
/// - `schema`:工具 `parameters()` 返回的 JSON Schema
/// - `args`:LLM 输出的 tool_call.arguments
///
/// # 错误
/// 校验失败返回 [`AgentError::ToolSchemaValidation`]。
pub fn validate_tool_args(tool_name: &str, schema: &Value, args: &Value) -> Result<()> {
    // Schema 必须是对象,否则跳过校验(降级)
    if !schema.is_object() {
        return Ok(());
    }
    validate_value(tool_name, schema, args, "$")
}

/// 递归校验值是否符合 Schema。
fn validate_value(tool_name: &str, schema: &Value, value: &Value, path: &str) -> Result<()> {
    let schema = match schema.as_object() {
        Some(s) => s,
        None => return Ok(()), // Schema 不是对象 → 跳过
    };

    // 1. type 校验(若声明)
    if let Some(type_val) = schema.get("type") {
        if let Err(expected) = check_type(type_val, value) {
            return Err(AgentError::ToolSchemaValidation {
                tool: tool_name.into(),
                path: path.into(),
                reason: "类型不匹配".into(),
                expected,
                actual: value_to_brief(value),
            });
        }
    }

    // 2. enum 校验(若声明)
    if let Some(enum_val) = schema.get("enum") {
        if let Some(arr) = enum_val.as_array() {
            if !arr.iter().any(|v| v == value) {
                return Err(AgentError::ToolSchemaValidation {
                    tool: tool_name.into(),
                    path: path.into(),
                    reason: "取值不在允许范围内".into(),
                    expected: format!("取值于 {}", serde_json::to_string(arr).unwrap_or_default()),
                    actual: value_to_brief(value),
                });
            }
        }
    }

    // 3. 按类型分发深度校验
    match value {
        Value::Null => {} // null 仅在 type 含 "null" 时通过(type 校验已处理)
        Value::Bool(_) => {}
        Value::Number(n) => {
            check_number_bounds(tool_name, schema, n, path)?;
        }
        Value::String(s) => {
            check_string_bounds(tool_name, schema, s, path)?;
        }
        Value::Array(arr) => {
            check_array_items(tool_name, schema, arr, path)?;
        }
        Value::Object(map) => {
            check_object_properties(tool_name, schema, map, path)?;
        }
    }

    Ok(())
}

/// 检查值类型是否匹配 Schema 声明的 type。
/// 支持单类型字符串 `"string"` 和多类型数组 `["string","null"]`。
/// 返回 Ok(()) 表示类型匹配,Err(期望类型描述) 表示不匹配。
fn check_type(type_val: &Value, value: &Value) -> std::result::Result<(), String> {
    let actual_type = json_type_name(value);

    if let Some(t_str) = type_val.as_str() {
        // 单类型
        if t_str == actual_type || (t_str == "number" && actual_type == "integer") {
            Ok(())
        } else {
            Err(format!("类型为 {t_str}"))
        }
    } else if let Some(arr) = type_val.as_array() {
        // 多类型:["string","null"]
        for t in arr {
            if let Some(t_str) = t.as_str() {
                if t_str == actual_type || (t_str == "number" && actual_type == "integer") {
                    return Ok(());
                }
            }
        }
        Err(format!(
            "类型为 {}",
            serde_json::to_string(type_val).unwrap_or_default()
        ))
    } else {
        Ok(()) // type 字段格式异常 → 跳过
    }
}

/// 检查数值边界(minimum/maximum)。
fn check_number_bounds(
    tool_name: &str,
    schema: &serde_json::Map<String, Value>,
    n: &serde_json::Number,
    path: &str,
) -> Result<()> {
    let num_f64 = n.as_f64().unwrap_or(f64::NAN);

    if let Some(min) = schema.get("minimum").and_then(|v| v.as_f64()) {
        if num_f64 < min {
            return Err(AgentError::ToolSchemaValidation {
                tool: tool_name.into(),
                path: path.into(),
                reason: format!("不能小于 {min}"),
                expected: format!("≥ {min}"),
                actual: n.to_string(),
            });
        }
    }

    if let Some(max) = schema.get("maximum").and_then(|v| v.as_f64()) {
        if num_f64 > max {
            return Err(AgentError::ToolSchemaValidation {
                tool: tool_name.into(),
                path: path.into(),
                reason: format!("不能大于 {max}"),
                expected: format!("≤ {max}"),
                actual: n.to_string(),
            });
        }
    }

    Ok(())
}

/// 检查字符串边界(minLength/maxLength)。
fn check_string_bounds(
    tool_name: &str,
    schema: &serde_json::Map<String, Value>,
    s: &str,
    path: &str,
) -> Result<()> {
    let len = s.chars().count(); // Unicode 字符数,非字节数

    if let Some(min) = schema.get("minLength").and_then(|v| v.as_u64()) {
        if (len as u64) < min {
            return Err(AgentError::ToolSchemaValidation {
                tool: tool_name.into(),
                path: path.into(),
                reason: format!("长度不能小于 {min}"),
                expected: format!("字符数 ≥ {min}"),
                actual: format!("{len}"),
            });
        }
    }

    if let Some(max) = schema.get("maxLength").and_then(|v| v.as_u64()) {
        if (len as u64) > max {
            return Err(AgentError::ToolSchemaValidation {
                tool: tool_name.into(),
                path: path.into(),
                reason: format!("长度不能大于 {max}"),
                expected: format!("字符数 ≤ {max}"),
                actual: format!("{len}"),
            });
        }
    }

    Ok(())
}

/// 递归校验数组元素。
fn check_array_items(
    tool_name: &str,
    schema: &serde_json::Map<String, Value>,
    arr: &[Value],
    path: &str,
) -> Result<()> {
    if let Some(items_schema) = schema.get("items") {
        for (i, item) in arr.iter().enumerate() {
            let item_path = format!("{path}[{i}]");
            validate_value(tool_name, items_schema, item, &item_path)?;
        }
    }
    Ok(())
}

/// 校验对象属性(required / properties / additionalProperties)。
fn check_object_properties(
    tool_name: &str,
    schema: &serde_json::Map<String, Value>,
    map: &serde_json::Map<String, Value>,
    path: &str,
) -> Result<()> {
    // required 校验
    if let Some(req) = schema.get("required").and_then(|v| v.as_array()) {
        for r in req {
            if let Some(field) = r.as_str() {
                if !map.contains_key(field) {
                    return Err(AgentError::ToolSchemaValidation {
                        tool: tool_name.into(),
                        path: if path == "$" {
                            field.into()
                        } else {
                            format!("{path}.{field}")
                        },
                        reason: "必填字段缺失".into(),
                        expected: format!("存在字段 '{field}'"),
                        actual: "缺失".into(),
                    });
                }
            }
        }
    }

    // properties 递归校验
    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (key, prop_schema) in props {
            if let Some(val) = map.get(key) {
                let field_path = if path == "$" {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                validate_value(tool_name, prop_schema, val, &field_path)?;
            }
        }
    }

    // additionalProperties:false 时检查多余字段
    if schema
        .get("additionalProperties")
        .and_then(|v| v.as_bool())
        == Some(false)
    {
        if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
            for key in map.keys() {
                if !props.contains_key(key) {
                    return Err(AgentError::ToolSchemaValidation {
                        tool: tool_name.into(),
                        path: if path == "$" {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        },
                        reason: "未声明的字段(Schema additionalProperties=false)".into(),
                        expected: "不允许的字段".into(),
                        actual: format!("存在字段 '{key}'"),
                    });
                }
            }
        }
    }

    Ok(())
}

/// 获取 JSON 值的类型名称。
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 值的简短表示(截断长字符串,用于错误信息)。
fn value_to_brief(value: &Value) -> String {
    let s = match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    // 截断长字符串
    const MAX_LEN: usize = 80;
    if s.chars().count() > MAX_LEN {
        format!("{}...(截断)", s.chars().take(MAX_LEN).collect::<String>())
    } else {
        s
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TOOL: &str = "TestTool";

    // ========== type 校验 ==========

    #[test]
    fn type_string_ok() {
        let schema = json!({"type": "string"});
        assert!(validate_tool_args(TOOL, &schema, &json!("hello")).is_ok());
    }

    #[test]
    fn type_string_mismatch() {
        let schema = json!({"type": "string"});
        let err = validate_tool_args(TOOL, &schema, &json!(42)).unwrap_err();
        match err {
            AgentError::ToolSchemaValidation { path, expected, .. } => {
                assert_eq!(path, "$");
                assert!(expected.contains("string"));
            }
            other => panic!("期望 ToolSchemaValidation,得到 {other:?}"),
        }
    }

    #[test]
    fn type_integer_ok() {
        let schema = json!({"type": "integer"});
        assert!(validate_tool_args(TOOL, &schema, &json!(42)).is_ok());
    }

    #[test]
    fn type_integer_reject_float() {
        let schema = json!({"type": "integer"});
        // 42.0 在 JSON 中 parse 为 integer,42.5 为 float
        assert!(validate_tool_args(TOOL, &schema, &json!(42.5)).is_err());
    }

    #[test]
    fn type_number_accepts_integer() {
        let schema = json!({"type": "number"});
        assert!(validate_tool_args(TOOL, &schema, &json!(42)).is_ok());
    }

    #[test]
    fn type_boolean_ok() {
        let schema = json!({"type": "boolean"});
        assert!(validate_tool_args(TOOL, &schema, &json!(true)).is_ok());
    }

    #[test]
    fn type_array_ok() {
        let schema = json!({"type": "array"});
        assert!(validate_tool_args(TOOL, &schema, &json!([1, 2, 3])).is_ok());
    }

    #[test]
    fn type_object_ok() {
        let schema = json!({"type": "object"});
        assert!(validate_tool_args(TOOL, &schema, &json!({"a": 1})).is_ok());
    }

    #[test]
    fn type_multi_with_null() {
        let schema = json!({"type": ["string", "null"]});
        assert!(validate_tool_args(TOOL, &schema, &json!("hi")).is_ok());
        assert!(validate_tool_args(TOOL, &schema, &json!(null)).is_ok());
        assert!(validate_tool_args(TOOL, &schema, &json!(42)).is_err());
    }

    // ========== required 校验 ==========

    #[test]
    fn required_field_present() {
        let schema = json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"]
        });
        assert!(validate_tool_args(TOOL, &schema, &json!({"command": "ls"})).is_ok());
    }

    #[test]
    fn required_field_missing() {
        let schema = json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"]
        });
        let err = validate_tool_args(TOOL, &schema, &json!({})).unwrap_err();
        match err {
            AgentError::ToolSchemaValidation { path, reason, .. } => {
                assert_eq!(path, "command");
                assert!(reason.contains("必填"));
            }
            other => panic!("期望 ToolSchemaValidation,得到 {other:?}"),
        }
    }

    // ========== properties 递归校验 ==========

    #[test]
    fn nested_property_ok() {
        let schema = json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout_ms": {"type": "integer", "minimum": 1, "maximum": 600000}
            },
            "required": ["command"]
        });
        assert!(validate_tool_args(
            TOOL,
            &schema,
            &json!({"command": "sleep 1", "timeout_ms": 5000})
        )
        .is_ok());
    }

    #[test]
    fn nested_property_type_error() {
        let schema = json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout_ms": {"type": "integer"}
            },
            "required": ["command"]
        });
        let err =
            validate_tool_args(TOOL, &schema, &json!({"command": "ls", "timeout_ms": "fast"}))
                .unwrap_err();
        match err {
            AgentError::ToolSchemaValidation { path, expected, .. } => {
                assert_eq!(path, "timeout_ms");
                assert!(expected.contains("integer"));
            }
            other => panic!("期望 ToolSchemaValidation,得到 {other:?}"),
        }
    }

    // ========== enum 校验 ==========

    #[test]
    fn enum_ok() {
        let schema = json!({"enum": ["read", "write", "execute"]});
        assert!(validate_tool_args(TOOL, &schema, &json!("read")).is_ok());
    }

    #[test]
    fn enum_violation() {
        let schema = json!({"enum": ["read", "write"]});
        let err = validate_tool_args(TOOL, &schema, &json!("delete")).unwrap_err();
        // 错误信息应包含实际值 "delete"
        let display = format!("{err}");
        assert!(display.contains("delete"), "错误信息应包含实际值: {display}");
        // 错误信息应包含"取值"提示
        assert!(
            display.contains("取值") || display.contains("允许"),
            "错误信息应提示允许范围: {display}"
        );
    }

    // ========== minimum / maximum 校验 ==========

    #[test]
    fn minimum_ok() {
        let schema = json!({"type": "integer", "minimum": 1});
        assert!(validate_tool_args(TOOL, &schema, &json!(10)).is_ok());
    }

    #[test]
    fn minimum_violation() {
        let schema = json!({"type": "integer", "minimum": 1});
        let err = validate_tool_args(TOOL, &schema, &json!(0)).unwrap_err();
        match err {
            AgentError::ToolSchemaValidation { expected, .. } => {
                assert!(expected.contains("≥ 1"));
            }
            other => panic!("期望 ToolSchemaValidation,得到 {other:?}"),
        }
    }

    #[test]
    fn maximum_violation() {
        let schema = json!({"type": "integer", "maximum": 100});
        let err = validate_tool_args(TOOL, &schema, &json!(200)).unwrap_err();
        match err {
            AgentError::ToolSchemaValidation { expected, .. } => {
                assert!(expected.contains("≤ 100"));
            }
            other => panic!("期望 ToolSchemaValidation,得到 {other:?}"),
        }
    }

    // ========== minLength / maxLength 校验 ==========

    #[test]
    fn min_length_ok() {
        let schema = json!({"type": "string", "minLength": 1});
        assert!(validate_tool_args(TOOL, &schema, &json!("a")).is_ok());
    }

    #[test]
    fn min_length_violation() {
        let schema = json!({"type": "string", "minLength": 1});
        let err = validate_tool_args(TOOL, &schema, &json!("")).unwrap_err();
        match err {
            AgentError::ToolSchemaValidation { expected, .. } => {
                assert!(expected.contains("≥ 1"));
            }
            other => panic!("期望 ToolSchemaValidation,得到 {other:?}"),
        }
    }

    #[test]
    fn max_length_violation() {
        let schema = json!({"type": "string", "maxLength": 5});
        let err = validate_tool_args(TOOL, &schema, &json!("hello world")).unwrap_err();
        match err {
            AgentError::ToolSchemaValidation { path, expected, actual, .. } => {
                assert_eq!(path, "$");
                assert!(expected.contains("≤ 5"));
                assert!(actual.contains("11"));
            }
            other => panic!("期望 ToolSchemaValidation,得到 {other:?}"),
        }
    }

    // ========== items 数组元素校验 ==========

    #[test]
    fn items_ok() {
        let schema = json!({"type": "array", "items": {"type": "string"}});
        assert!(validate_tool_args(TOOL, &schema, &json!(["a", "b", "c"])).is_ok());
    }

    #[test]
    fn items_violation() {
        let schema = json!({"type": "array", "items": {"type": "string"}});
        let err = validate_tool_args(TOOL, &schema, &json!(["a", 42, "c"])).unwrap_err();
        match err {
            AgentError::ToolSchemaValidation { path, expected, .. } => {
                assert_eq!(path, "$[1]");
                assert!(expected.contains("string"));
            }
            other => panic!("期望 ToolSchemaValidation,得到 {other:?}"),
        }
    }

    // ========== additionalProperties:false 校验 ==========

    #[test]
    fn additional_properties_false_ok() {
        let schema = json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "additionalProperties": false
        });
        assert!(validate_tool_args(TOOL, &schema, &json!({"name": "test"})).is_ok());
    }

    #[test]
    fn additional_properties_false_reject_extra() {
        let schema = json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "additionalProperties": false
        });
        let err = validate_tool_args(TOOL, &schema, &json!({"name": "test", "extra": 1}))
            .unwrap_err();
        match err {
            AgentError::ToolSchemaValidation { path, reason, .. } => {
                assert_eq!(path, "extra");
                assert!(reason.contains("未声明"));
            }
            other => panic!("期望 ToolSchemaValidation,得到 {other:?}"),
        }
    }

    // ========== 边界场景 ==========

    #[test]
    fn no_type_skips_type_check() {
        // Schema 没有 type 关键字 → 不做类型校验(可搭配 enum 等)
        let schema = json!({"minLength": 1});
        assert!(validate_tool_args(TOOL, &schema, &json!("ok")).is_ok());
        assert!(validate_tool_args(TOOL, &schema, &json!(42)).is_ok()); // 不检查类型
    }

    #[test]
    fn schema_not_object_skips() {
        // Schema 本身不是对象 → 降级为不校验
        let schema = json!("invalid");
        assert!(validate_tool_args(TOOL, &schema, &json!("anything")).is_ok());
    }

    #[test]
    fn empty_object_args_with_no_required() {
        let schema = json!({
            "type": "object",
            "properties": {"opt": {"type": "string"}}
        });
        assert!(validate_tool_args(TOOL, &schema, &json!({})).is_ok());
    }

    #[test]
    fn null_value_with_non_null_type() {
        let schema = json!({"type": "string"});
        assert!(validate_tool_args(TOOL, &schema, &json!(null)).is_err());
    }

    #[test]
    fn real_world_bash_tool_schema() {
        // 模拟 Bash 工具的真实 Schema
        let schema = json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "命令"},
                "timeout_ms": {"type": "integer", "minimum": 1, "maximum": 600000},
                "description": {"type": "string"}
            },
            "required": ["command"],
            "additionalProperties": false
        });

        // 正确调用
        assert!(validate_tool_args(
            "Bash",
            &schema,
            &json!({"command": "ls -la", "timeout_ms": 5000})
        )
        .is_ok());

        // 缺少 command
        assert!(validate_tool_args("Bash", &schema, &json!({"timeout_ms": 5000})).is_err());

        // timeout_ms 越界
        assert!(validate_tool_args("Bash", &schema, &json!({"command": "x", "timeout_ms": 0}))
            .is_err());

        // 多余字段
        assert!(validate_tool_args(
            "Bash",
            &schema,
            &json!({"command": "x", "unknown": 1})
        )
        .is_err());
    }

    #[test]
    fn real_world_edit_tool_schema() {
        // 模拟 Edit 工具的真实 Schema
        let schema = json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean"}
            },
            "required": ["file_path", "old_string", "new_string"],
            "additionalProperties": false
        });

        assert!(validate_tool_args(
            "Edit",
            &schema,
            &json!({"file_path": "a.rs", "old_string": "foo", "new_string": "bar"})
        )
        .is_ok());

        // 缺少 new_string
        assert!(validate_tool_args(
            "Edit",
            &schema,
            &json!({"file_path": "a.rs", "old_string": "foo"})
        )
        .is_err());
    }

    #[test]
    fn error_message_contains_tool_name() {
        let schema = json!({"type": "string"});
        let err = validate_tool_args("MyTool", &schema, &json!(42)).unwrap_err();
        let display = format!("{err}");
        assert!(display.contains("MyTool"));
    }

    #[test]
    fn long_string_truncated_in_error() {
        let schema = json!({"type": "integer"});
        let long_str = "x".repeat(200);
        let err = validate_tool_args(TOOL, &schema, &json!(long_str)).unwrap_err();
        let display = format!("{err}");
        // 长字符串应被截断
        assert!(display.contains("截断") || display.len() < 300);
    }
}
