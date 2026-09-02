# Glob 工具定义
```json
{
  "name": "Glob",
  "description": "- 快速的文件模式匹配工具，适用于任意规模的代码库\n- 支持 glob 模式，如 \"**/*.js\" 或 \"src/**/*.ts\"\n- 返回按修改时间排序的匹配文件路径\n- 当你需要通过名称模式查找文件时，请使用此工具\n- 当你正在进行一个开放式的搜索，可能需要多轮的 globbing 和 grepping 时，请改用 Agent 工具",
  "input_schema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "pattern": {
    "description": "用于匹配文件的 glob 模式",
    "type": "string"
    },
    "path": {
    "description": "要搜索的目录。如果未指定，将使用当前工作目录。重要提示：省略此字段以使用默认目录。不要输入 \"undefined\" 或 \"null\"，只需省略该字段即可实现默认行为。如果提供，必须是一个有效的目录路径。",
    "type": "string"
    }
    },
    "required": [
    "pattern"
    ],
    "additionalProperties": false
  }
}
```