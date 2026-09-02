# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "code_review",
    "description": "对当前改动执行一次严格的只读（READ-ONLY）代码审查，并返回按优先级排序的问题清单（正确性 > 安全性 > 可靠性）。只处理请求范围内的事项，然后直接调用本工具，无需预先审查 diff。范围过大时返回预检（preflight）结果而非直接开始；只有在用户明确接受该范围后，才回传 `confirm_scope`。本工具会启动一个独立的审查代理，且绝不修改任何文件。",
    "parameters": {
      "type": "object",
      "properties": {
        "scope": {
          "oneOf": [
            {
              "type": "object",
              "properties": {
                "kind": {
                  "const": "working_tree"
                }
              },
              "required": [
                "kind"
              ]
            },
            {
              "type": "object",
              "properties": {
                "kind": {
                  "const": "staged"
                }
              },
              "required": [
                "kind"
              ]
            },
            {
              "type": "object",
              "properties": {
                "kind": {
                  "const": "range"
                },
                "base": {
                  "type": "string"
                },
                "head": {
                  "type": "string",
                  "default": "HEAD"
                }
              },
              "required": [
                "kind",
                "base"
              ]
            },
            {
              "type": "object",
              "properties": {
                "kind": {
                  "const": "commit"
                },
                "rev": {
                  "type": "string"
                }
              },
              "required": [
                "kind",
                "rev"
              ]
            }
          ],
          "description": "明确的互斥审查范围。省略时表示审查工作区（working tree）的改动。"
        },
        "paths": {
          "type": "array",
          "items": {
            "type": "string"
          },
          "description": "可选的仓库相对路径过滤器。"
        },
        "confirm_scope": {
          "type": "string",
          "description": "预检返回的不透明令牌。仅在用户明确确认后传入。"
        }
      }
    }
  }
}
```