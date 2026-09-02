# mcp__ardot__scan_exportable_resources 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__scan_exportable_resources",
        "description": "遍历节点树，返回所有可导出资源节点，并按类型分类。

- 传入根节点ID，递归扫描其所有后代节点。
- 返回包含两组列表的映射：「image」（带有图像填充的节点，应当导出为PNG/JPEG/WEBP格式）与「svg」（仅包含矢量/图形元素的节点，可导出为SVG格式）。
- 在调用 export_nodes 之前使用此工具，确定需要导出的节点与对应的导出格式。
- 若省略 nodeId，则扫描当前页面。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                },
                "nodeId": {
                    "description": "作为扫描起点的根节点ID。若省略，则扫描当前页面。",
                    "type": "string"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```