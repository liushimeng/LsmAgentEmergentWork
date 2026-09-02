# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "write_file",
    "description": "将内容写入文件：若文件不存在则创建它（包括所需的父级目录），若已存在则覆盖。适用于新建文件或整体重写；对已有文件做小改动请优先使用 edit_file。相对路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "file_path": {
          "type": "string",
          "description": "要写入的路径（绝对路径，或相对于工作目录的路径）"
        },
        "content": {
          "type": "string",
          "description": "要写入的完整内容"
        }
      },
      "required": [
        "file_path",
        "content"
      ]
    }
  }
}
```