# read 工具定义
```json
{
  "name": "read",
  "description": "读取文件内容。支持文本文件和图像（jpg、png、gif、webp）。图像将作为附件发送。对于文本文件，输出内容会被截断至 2000 行或 50KB（以先达到的条件为准）。对于大文件，请使用 offset 和 limit 参数。当需要读取完整文件时，请通过调整 offset 继续读取，直至完成。",
  "parameters": {
    "type": "object",
    "required": [
    "path"
    ]
    ,
    "properties": {
    "path": {
    "type": "string",
    "description": "要读取的文件路径（相对路径或绝对路径）"
    }
    ,
    "offset": {
    "type": "number",
    "description": "开始读取的行号（从 1 开始计数）"
    }
    ,
    "limit": {
    "type": "number",
    "description": "最多读取的行数"
    }
    }
  }
}
```