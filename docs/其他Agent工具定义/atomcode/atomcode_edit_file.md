# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "edit_file",
    "description": "替换文件中一段精确匹配的文本。`old_string` 必须完全匹配（包括空格和缩进），且除非 `replace_all` 为 true，否则必须在文件中唯一——请包含足够的上下文使其唯一。如果未匹配或匹配不明确，文件将保持原样不变。相对路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "file_path": {
          "type": "string",
          "description": "要编辑的文件路径（绝对路径，或相对于工作目录的路径）"
        },
        "old_string": {
          "type": "string",
          "description": "要查找的精确文本。除非 replace_all 为 true，否则必须唯一。"
        },
        "new_string": {
          "type": "string",
          "description": "替换后的文本。"
        },
        "replace_all": {
          "type": "boolean",
          "description": "替换所有匹配项（默认 false = 要求唯一匹配）。"
        }
      },
      "required": [
        "file_path",
        "old_string",
        "new_string"
      ]
    }
  }
}
```