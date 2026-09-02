# edit 工具定义
```json
{
  "name": "edit",
  "description": "在文件中执行精确的字符串替换。

  使用说明：
  - 在对话中，您必须至少使用一次 `Read` 工具后才能进行编辑。如果您尝试在未读取文件的情况下进行编辑，此工具将报错。
  - 在编辑来自 Read 工具输出的文本时，请确保保留与行号前缀后完全相同的缩进（制表符/空格）。行号前缀格式为：行号 + 冒号 + 空格（例如 `1: `）。该空格之后的所有内容都是要匹配的实际文件内容。切勿将行号前缀的任何部分包含在 oldString 或 newString 中。
  - 始终优先编辑代码库中现有的文件。除非明确要求，否则绝不要编写新文件。
  - 仅在用户明确要求时才使用表情符号。除非被要求，否则避免向文件中添加表情符号。
  - 如果文件中未找到 `oldString`，编辑将失败，并提示错误“oldString not found in content”。
  - 如果文件中找到多个 `oldString`，编辑将失败，并提示错误“Found multiple matches for oldString. Provide more surrounding lines in oldString to identify the correct match.”。请提供包含更多周围行的更大字符串以使其唯一，或使用 `replaceAll` 参数来更改 `oldString` 的每个实例。
  - 使用 `replaceAll` 在整个文件中替换和重命名字符串。如果要重命名变量等，此参数非常有用。",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "filePath": {
    "type": "string",
    "description": "要修改的文件的绝对路径"
    },
    "oldString": {
    "type": "string",
    "description": "要替换的文本"
    },
    "newString": {
    "type": "string",
    "description": "要替换为的新文本（必须与 oldString 不同）"
    },
    "replaceAll": {
    "type": "boolean",
    "description": "替换 oldString 的所有出现项（默认为 false）"
    }
    },
    "required": [
    "filePath",
    "oldString",
    "newString"
    ]
  }
}
```