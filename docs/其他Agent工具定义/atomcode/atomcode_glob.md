# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "glob",
    "description": "在基础目录下按 glob 模式查找文件（例如 `**/*.rs`、`src/**/*.ts`），并感知 gitignore。`**` 可跨目录匹配，`*` 不行。构建/版本控制/缓存目录会被跳过。相对的基础目录路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "pattern": {
          "type": "string",
          "description": "Glob 模式，例如 **/*.rs"
        },
        "path": {
          "type": "string",
          "description": "要搜索的基础目录（默认：工作目录）"
        }
      },
      "required": [
        "pattern"
      ]
    }
  }
}
```