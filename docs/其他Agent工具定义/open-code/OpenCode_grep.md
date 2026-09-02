# grep 工具定义
```json
{
  "name": "grep",
  "description": "- 快速内容搜索工具，适用于任何规模的代码库
  - 使用正则表达式搜索文件内容
  - 支持完整的正则语法（例如 \"log.*Error\", \"function\\s+\\w+\" 等）
  - 使用 include 参数通过模式过滤文件（例如 \"*.js\", \"*.{ts,tsx}\"）
  - 返回包含匹配行的文件路径和行号
  - 当您需要查找包含特定模式的文件时，请使用此工具
  - 如果您需要识别/计算文件中匹配项的数量，请直接使用 Bash 工具配合 `rg` (ripgrep)。不要使用 `grep`。
  - 当您正在进行一项开放式的搜索，可能需要多轮的 globbing 和 grepping 时，请改用 Task 工具",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "pattern": {
    "type": "string",
    "description": "要在文件内容中搜索的正则表达式模式"
    },
    "path": {
    "type": "string",
    "description": "要搜索的目录。默认为当前工作目录。"
    },
    "include": {
    "type": "string",
    "description": "要在搜索中包含的文件模式（例如 \"*.js\", \"*.{ts,tsx}\"）"
    }
    },
    "required": [
    "pattern"
    ]
  }
}
```