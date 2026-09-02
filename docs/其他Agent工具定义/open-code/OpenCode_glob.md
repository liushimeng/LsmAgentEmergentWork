# glob 工具定义
```json
{
  "name": "glob",
  "description": "- 快速文件模式匹配工具，适用于任何规模的代码库
  - 支持 glob 模式，如 \"**/*.js\" 或 \"src/**/*.ts\"
  - 返回匹配的文件路径
  - 当您需要通过名称模式查找文件时，请使用此工具
  - 当您正在进行一项开放式的搜索，可能需要多轮的 globbing 和 grepping 时，请改用 Task 工具
  - 您有能力在单个响应中调用多个工具。将多个可能有用的搜索作为批处理推测性地执行，通常会更好。",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "pattern": {
    "type": "string",
    "description": "用于匹配文件的 glob 模式"
    },
    "path": {
    "type": "string",
    "description": "要搜索的目录。如果未指定，将使用当前工作目录。重要提示：为了使用默认目录，请省略此字段。不要输入 \"undefined\" 或 \"null\"，如果希望使用默认行为，请直接省略它。如果提供，必须是一个有效的目录路径。"
    }
    },
    "required": [
    "pattern"
    ]
  }
}
```