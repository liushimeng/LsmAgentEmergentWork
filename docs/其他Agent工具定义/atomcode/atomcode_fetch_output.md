# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "fetch_output",
    "description": "继续读取被截断的大段工具输出。传入截断标记中的 artifact_id，以及字节偏移量 offset 和长度 limit。返回请求的字节片段；如果该输出片段不可用，则改为重新运行原始命令。",
    "parameters": {
      "type": "object",
      "properties": {
        "artifact_id": {
          "type": "string",
          "description": "截断标记中的 ID"
        },
        "offset": {
          "type": "integer",
          "description": "起始字节偏移量（默认 0）"
        },
        "limit": {
          "type": "integer",
          "description": "最多返回的字节数（默认/最大 65536）"
        }
      },
      "required": [
        "artifact_id"
      ]
    }
  }
}
```