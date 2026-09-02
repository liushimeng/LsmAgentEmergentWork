# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "file_dependencies",
    "description": "展示某个文件的依赖关系：它使用了哪些文件（其符号所调用的被调用方 callees），以及哪些文件使用了它（调用方 callers）。相对路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "file": {
          "type": "string",
          "description": "文件路径（绝对路径，或相对于工作目录的路径）"
        }
      },
      "required": [
        "file"
      ]
    }
  }
}
```