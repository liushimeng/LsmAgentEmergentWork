# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "search_replace",
    "description": "一次性在多个文件中查找并替换文本——会替换每个匹配文件中的所有出现位置。适用于项目级的重命名（如 CSS 类名、import 语句、配置键、字符串字面量）。单个文件请优先使用 `edit_file`。`regex:true` 启用正则模式（可在 `replace` 中使用 `$1`/`$2` 捕获组）；默认为字面量匹配。`glob` 限定范围（如 \"*.rs\"、\"src/**/*.ts\"）；`path` 设置搜索根目录（默认：工作目录）。",
    "parameters": {
      "type": "object",
      "properties": {
        "search": {
          "type": "string",
          "description": "要查找的文本或正则模式"
        },
        "replace": {
          "type": "string",
          "description": "替换后的文本（正则模式可用 $1、$2 引用捕获组）"
        },
        "glob": {
          "type": "string",
          "description": "限定范围的文件模式，如 \"*.rs\"、\"src/**/*.ts\"（默认：所有文件）"
        },
        "path": {
          "type": "string",
          "description": "要搜索的目录（默认：工作目录）"
        },
        "regex": {
          "type": "boolean",
          "description": "使用正则匹配（默认：false = 字面量匹配）"
        }
      },
      "required": [
        "search",
        "replace"
      ]
    }
  }
}
```