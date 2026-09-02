# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "read_symbol",
    "description": "按名称读取单个符号（函数/类/结构体/方法）的完整源码，并附带行号。比 read_file 更精准——恰好只返回该符号本身。建议先用 list_symbols 查找可用符号名。相对路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "file_path": {
          "type": "string",
          "description": "源文件路径（绝对路径，或相对于工作目录的路径）"
        },
        "symbol": {
          "type": "string",
          "description": "要读取的符号名称"
        }
      },
      "required": [
        "file_path",
        "symbol"
      ]
    }
  }
}
```