# mcp__ardot__capture_layout 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__capture_layout",
        "description": "检查设计文件当前的布局结构。

- 省略 maxDepth 参数时需谨慎，过大的深度值会产生大量返回数据。
- 如果仅需要排查布局问题（例如文本行高、元素重叠、内容裁切等），请启用 problemsOnly 参数。
- maxDepth=1 代表仅返回 parentId 容器内顶层节点的布局信息。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [
                "parentId"
            ],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                },
                "parentId": {
                    "type": "string",
                    "description": "用于限定查询范围的父节点ID"
                },
                "maxDepth": {
                    "description": "返回布局包围盒信息的最大层级深度",
                    "type": "number"
                },
                "problemsOnly": {
                    "default": true,
                    "description": "仅返回存在布局异常的节点",
                    "type": "boolean"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```