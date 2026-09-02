# write 工具定义
```json
{
  "name": "write",
  "description": "将内容写入文件。如果文件不存在则创建，如果已存在则覆盖。会自动创建父级目录。",
  "parameters": {
    "type": "object",
    "required": [
    "path",
    "content"
    ],
    "properties": {
      "path": {
      "type": "string",
      "description": "要写入的文件路径（相对路径或绝对路径）"
      },
      "content": {
      "type": "string",
      "description": "要写入文件的内容"
      }
    }
  },
  "strict": false
}
```