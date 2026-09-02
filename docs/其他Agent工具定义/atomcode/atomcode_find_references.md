# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "find_references",
    "description": "在整个项目中查找某个符号（函数/类/变量）的所有引用，按整词文本匹配，并感知 gitignore。返回格式为 文件:行号: 内容。相对路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "symbol": {
          "type": "string",
          "description": "要查找引用的符号名称"
        },
        "path": {
          "type": "string",
          "description": "要搜索的目录（默认：工作目录）"
        }
      },
      "required": [
        "symbol"
      ]
    }
  }
}
```