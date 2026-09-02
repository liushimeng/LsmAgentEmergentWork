# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "read_file",
    "description": "从文件系统读取文件。返回的内容带有从 1 开始的行号前缀（`<n>\t<内容>`）。默认最多返回 300 行；受输出预算限制可能返回更少。当结果显示了续读偏移量时，请从该偏移量继续读取，而不是重新从第 1 行读起。需要更大的相关窗口时，使用 `offset`（从 1 开始的起始行）和 `limit`（最大行数）；避免多次微小且重叠的读取。若要一次读取多个不连续的窗口（例如骨架中列出的多个符号），请传入 `ranges` 而非分页读取。如果路径是目录，则改为列出其条目。相对路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "file_path": {
          "type": "string",
          "description": "要读取的路径（绝对路径，或相对于工作目录的路径）"
        },
        "offset": {
          "type": "integer",
          "minimum": 1,
          "description": "起始行号，从 1 开始计数。省略则从第 1 行开始；结果不完整时，使用显示的下一个偏移量继续。"
        },
        "limit": {
          "type": "integer",
          "minimum": 1,
          "default": 300,
          "description": "最多读取的行数。默认为 300；受输出字节预算限制可能返回更少。"
        },
        "ranges": {
          "type": "array",
          "description": "在一次调用中读取多个不连续的行窗口，而非分页读取。每个元素为 {offset, limit}。优先级高于顶层的 offset/limit，也高于大文件骨架。当你已知晓所需范围时（例如来自骨架中各符号的 offset/limit），优先使用此参数。所有窗口共享同一输出预算。",
          "items": {
            "type": "object",
            "properties": {
              "offset": {
                "type": "integer",
                "minimum": 1,
                "description": "起始行号，从 1 开始计数"
              },
              "limit": {
                "type": "integer",
                "minimum": 1,
                "description": "此窗口的最大行数"
              }
            },
            "required": [
              "offset"
            ]
          }
        }
      },
      "required": [
        "file_path"
      ]
    }
  }
}
```