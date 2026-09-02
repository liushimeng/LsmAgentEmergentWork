# mcp__ardot__upload_images 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__upload_images",
        "description": "上传本地图片文件，并将图片填充应用到指定节点，支持批量操作。

- 当智能体生成图片并提供文件路径时，可使用该工具快速为多个节点设置图片。
- 每一条配置项将一个本地图片文件路径关联至目标节点ID。
- 图片将被设置为节点的图像填充，覆盖原有所有填充内容。
- 支持图片格式：PNG、JPEG、WEBP。
- 目标节点必须支持填充功能（例如：画布容器、矩形、椭圆等图形）。
- 单次调用最多传入20条配置项，保障运行性能。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [
                "items"
            ],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果当前打开了多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "nodeId": {
                                "type": "string",
                                "description": "需要应用图片填充的目标节点ID"
                            },
                            "filePath": {
                                "type": "string",
                                "description": "本地图片文件的绝对路径"
                            }
                        },
                        "required": [
                            "nodeId",
                            "filePath"
                        ]
                    },
                    "description": "节点与图片的配对数组。数组内每一项对应一张本地图片与一个目标节点的绑定关系。"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```