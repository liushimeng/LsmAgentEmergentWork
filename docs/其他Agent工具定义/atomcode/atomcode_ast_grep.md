# Agent 工具定义
```json
{
  "name": "ast_grep",
  "description": "基于 AST 模式的结构化代码搜索（原生 ast-grep）——匹配语法而非文本。当正则表达式不够精确时使用：例如定位特定的调用形式、声明或语法结构。元变量：`$NAME` 捕获一个节点，`$_` 匹配一个节点但不绑定，`$$$` 匹配零个或多个节点（名称须大写；每个必须是一个完整的 AST 节点）。模式必须能被目标语言解析为一个有效的节点。`paths` 默认为工作目录；传入 `lang` 可强制指定语言。",
  "parameters": {
    "type": "object",
    "properties": {
      "pattern": {
        "type": "string",
        "description": "单个 AST 模式，例如 \"$X.unwrap()\" 或 \"fn $NAME($$$) { $$$ }\""
      },
      "paths": {
        "type": "array",
        "items": {
          "type": "string"
        },
        "description": "要搜索的文件/目录/通配符（默认：工作目录）"
      },
      "lang": {
        "type": "string",
        "description": "强制指定语言（例如 rust、typescript）；省略则根据文件扩展名推断"
      }
    },
    "required": [
      "pattern"
    ]
  }
}
```