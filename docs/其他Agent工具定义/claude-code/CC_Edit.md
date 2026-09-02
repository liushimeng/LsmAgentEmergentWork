# Edit 工具定义
```json
{
    "name": "Edit",
    "description": "在文件中执行精准字符串替换。

    - 编辑前必须在当前会话中先读取该文件，否则操作会失败。
    - `old_string` 必须与文件内容完全一致（包含缩进），且内容唯一，否则编辑失败。匹配前需去除读取结果中的行号与制表符前缀。
    - 将 `replace_all` 设为 true 可替换所有匹配内容。",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "file_path": {
                "description": "待修改文件的绝对路径",
                "type": "string"
            },
            "old_string": {
                "description": "需要被替换的原文本",
                "type": "string"
            },
            "new_string": {
                "description": "用于替换的新文本（必须与原文本不同）",
                "type": "string"
            },
            "replace_all": {
                "description": "是否替换所有匹配项（默认值：false）",
                "default": false,
                "type": "boolean"
            }
        },
        "required": [
            "file_path",
            "old_string",
            "new_string"
        ],
        "additionalProperties": false
    }
}
```