# write 工具定义
```json
{
  "name": "write",
  "description": "将文件写入本地文件系统。

  使用方法：
  - 如果提供的路径下已存在文件，此工具将覆盖该文件。
  - 如果这是现有文件，你必须先使用 Read 工具读取文件内容。如果你未先读取文件，此工具将执行失败。
  - 始终优先编辑代码库中已存在的文件。除非有明确要求，否则绝不要编写新文件。
  - 绝不要主动创建文档文件（*.md）或 README 文件。仅在用户明确要求时才创建文档文件。
  - 仅在用户明确要求时才使用表情符号。除非被要求，否则避免向文件写入表情符号。",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "content": {
    "type": "string",
    "description": "要写入文件的内容"
    },
    "filePath": {
    "type": "string",
    "description": "要写入文件的绝对路径（必须是绝对路径，而非相对路径）"
    }
    },
    "required": [
    "content",
    "filePath"
    ]
  }
}
```