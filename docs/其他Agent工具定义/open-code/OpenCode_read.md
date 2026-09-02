# read 工具定义
```json
{
  "name": "read",
  "description": "从本地文件系统读取文件或目录。如果路径不存在，则返回错误。

  使用说明：
  - filePath 参数应为绝对路径。
  - 默认情况下，此工具返回文件开头的最多 2000 行。
  - offset 参数是开始读取的行号（从1开始计数）。
  - 要读取后面的章节，请使用更大的 offset 再次调用此工具。
  - 使用 grep 工具在大文件或包含长行的文件中查找特定内容。
  - 如果不确定正确的文件路径，请使用 glob 工具通过 glob 模式查找文件名。
  - 返回的内容每行都会加上行号前缀，格式为 `<行号>: <内容>`。例如，如果文件内容为 \"foo
  \"，您将收到 \"1: foo
  \"。对于目录，条目每行返回一个（无行号），子目录末尾带有 \"/\"。
  - 任何超过 2000 个字符的行都将被截断。
  - 当您知道有多个文件需要读取时，可以并行调用此工具。
  - 避免读取过小的重复片段（如30行的块）。如果需要更多上下文，请读取更大的范围。
  - 此工具可以读取图像文件和 PDF 并将其作为文件附件返回。",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "filePath": {
    "type": "string",
    "description": "要读取的文件或目录的绝对路径"
    },
    "offset": {
    "minimum": 0,
    "type": "integer",
    "maximum": 9007199254740991,
    "description": "开始读取的行号（从1开始计数）"
    },
    "limit": {
    "minimum": 0,
    "type": "integer",
    "maximum": 9007199254740991,
    "description": "要读取的最大行数（默认为2000）"
    }
    },
    "required": [
    "filePath"
    ]
  }
}
```