# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "list_directory",
    "description": "列出目录树（缩进显示；目录以 '/' 结尾）。`depth` 控制递归深度（默认 2，最大 5）。构建/版本控制/缓存目录（node_modules、.git、target 等）会被跳过。相对路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "path": {
          "type": "string",
          "description": "要列出的目录（默认：工作目录）"
        },
        "depth": {
          "type": "integer",
          "description": "最大递归深度（默认 2，最大 5）"
        }
      }
    }
  }
}
```