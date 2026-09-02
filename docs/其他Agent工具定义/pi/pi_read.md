# read 工具定义
```json
{
  "name": "read",
  "description": "读取文件内容。支持文本文件和图像文件（jpg、png、gif、webp）。图像以附件形式发送。对于文本文件，输出会被截断为最多 2000 行或 50KB（以先达到的为准）。大文件请使用 offset/limit 参数。若需完整文件内容，请通过 offset 分段读取直至完成。",
  "parameters": {
    "type": "object",
    "required": [
    "path"
    ],
    "properties": {
      "path": {
      "type": "string",
      "description": "要读取的文件路径（相对路径或绝对路径）"
      },
      "offset": {
      "type": "number",
      "description": "开始读取的行号（从 1 开始计数）"
      },
      "limit": {
      "type": "number",
      "description": "最多读取的行数"
      }
    }
  },
  "strict": false
}
```