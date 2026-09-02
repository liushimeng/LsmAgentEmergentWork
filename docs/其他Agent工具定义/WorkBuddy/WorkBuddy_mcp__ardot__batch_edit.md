# mcp__ardot__batch_edit 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__batch_edit",
        "description": "在Ardot设计画布中创建、更新、移动、删除节点。
单次调用可批量执行多条新增/复制/更新/移动/删除/图片相关操作。

重要提示：请在技能内加载 '{SKILL_ROOT}/tool-usage/batch-edit.md' 查看详细使用文档。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [
                "operations"
            ],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                },
                "operations": {
                    "type": "string",
                    "description": "待执行的新增/复制/更新/替换/移动/删除/图片操作列表，格式遵循工具说明文档。单次调用操作数量尽量不超过25条。"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```