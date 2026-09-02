# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "grep",
    "description": "在目录下按正则表达式搜索文件内容（感知 gitignore；会跳过构建/缓存目录和 .log 文件）。智能大小写：除非模式中包含大写字母，否则不区分大小写。正则元字符需转义，例如 `console\\.log\\(`。相对路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "pattern": {
          "type": "string",
          "description": "要搜索的正则表达式"
        },
        "path": {
          "type": "string",
          "description": "要搜索的目录或文件（默认：工作目录）"
        },
        "max_results": {
          "type": "integer",
          "description": "最多返回的匹配行数（默认 50）"
        },
        "context": {
          "type": "integer",
          "description": "每个匹配项周围的上下文行数（默认 3，最大 10）"
        }
      },
      "required": [
        "pattern"
      ]
    }
  }
}
```