# Write 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "Write",
        "description": "向本地文件系统写入文件。

使用规则：
- 如果指定路径已存在文件，该工具会直接覆盖原有文件。
- 目标文件若已存在，**必须先使用 Read 工具读取文件内容**。未预先读取文件就调用本工具将会执行失败。
- 对于代码库内已有文件，优先采用编辑方式。除非明确需要，否则不要新建文件。
- 禁止主动创建文档文件（*.md）或 README 文件，仅在用户明确提出要求时才创建此类文档。
- 仅当用户明确要求时，才在内容中使用表情符号；若无要求，不要在文件内写入表情。
",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "待写入的文件路径（支持绝对路径或相对路径）"
                },
                "content": {
                    "type": "string",
                    "description": "需要写入文件的内容"
                }
            },
            "required": [
                "file_path",
                "content"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```