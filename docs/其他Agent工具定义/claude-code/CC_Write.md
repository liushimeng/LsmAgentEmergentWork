# Write 工具定义
```json
{
    "name": "Write",
    "description": "向本地文件系统写入文件，若文件已存在则直接覆盖。

    使用场景：新建文件，或是完整替换已读取过内容的文件。若未提前读取原有文件就执行覆盖写入，操作将会失败。如需对文件进行局部修改，请使用编辑工具。",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "file_path": {
                "description": "待写入文件的绝对路径（必须使用绝对路径，不可使用相对路径）",
                "type": "string"
            },
            "content": {
                "description": "需要写入文件的内容",
                "type": "string"
            }
        },
        "required": [
            "file_path",
            "content"
        ],
        "additionalProperties": false
    }
}
```