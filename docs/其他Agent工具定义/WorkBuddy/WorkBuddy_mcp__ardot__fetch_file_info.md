# mcp__ardot__fetch_file_info 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__fetch_file_info",
        "description": "获取当前激活设计文件的基础信息。

- 返回文件名、文件链接、文件ID以及用户对此文件拥有的权限。
- 当需要确认当前正在编辑的文件，或是执行编辑操作前校验用户是否具备写入权限时，使用本工具。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```