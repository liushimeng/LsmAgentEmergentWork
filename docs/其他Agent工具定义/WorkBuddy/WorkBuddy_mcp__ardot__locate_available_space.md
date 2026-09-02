# mcp__ardot__locate_available_space 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__locate_available_space",
        "description": "在 .ardot 文件内，按照指定方向与目标尺寸查找可用空白区域。

- 重要提示：往根页面插入节点前，必须先调用此工具。
传入 nodeId，则在该指定节点周边寻找空位。
不传入 nodeId，则在画布全部内容范围内寻找空位。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [
                "width",
                "height"
            ],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                },
                "nodeId": {
                    "description": "可选参数，作为周边空位检索基准的节点ID",
                    "type": "string"
                },
                "width": {
                    "type": "number",
                    "description": "所需空白区域的宽度"
                },
                "height": {
                    "type": "number",
                    "description": "所需空白区域的高度"
                },
                "padding": {
                    "default": 100,
                    "description": "最小预留间距",
                    "type": "number"
                },
                "direction": {
                    "default": "right",
                    "description": "空白区域的检索方向",
                    "type": "string",
                    "enum": [
                        "top",
                        "right",
                        "bottom",
                        "left"
                    ]
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```