# AskUserQuestion 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "AskUserQuestion",
    "description": "在执行过程中需要向用户提问时使用此工具。这允许你：
1. 收集用户偏好或需求
2. 澄清模糊的指令
3. 在工作过程中就实现选择获取决策
4. 为用户提供关于采取何种方向的选择。

使用说明：
- 用户将始终能够选择“其他”来提供自定义文本输入
- 使用 multiSelect: true 允许为问题选择多个答案
- 如果你推荐特定选项，请将其作为列表中的第一个选项，并在标签末尾添加“（推荐）”

示例：
````json
{\"questions\": [{\"question\": \"我们应该使用哪种方法？\", \"header\": \"方法\", \"options\": [{\"label\": \"选项 A（推荐）\", \"description\": \"简单直接\"}, {\"label\": \"选项 B\", \"description\": \"更灵活但复杂\"}]}]}
````
",
    "parameters": {
      "type": "object",
      "properties": {
        "questions": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "question": {
                "type": "string",
                "description": "要向用户提出的完整问题。应清晰、具体，并以问号结尾。示例：“我们应该使用哪个库来进行日期格式化？”如果 multiSelect 为 true，请相应地措辞，例如“你想启用哪些功能？”"
              },
              "header": {
                "type": "string",
                "maxLength": 12,
                "description": "作为芯片/标签显示的极短标签（最多 12 个字符）。示例：“认证方法”、“库”、“方法”。"
              },
              "options": {
                "type": "array",
                "items": {
                  "type": "object",
                  "properties": {
                    "label": {
                      "type": "string",
                      "description": "此选项的显示文本，用户将看到并选择。不得包含前导或尾随空格。应简洁（1-5 个词）并清楚描述选择。"
                    },
                    "description": {
                      "type": "string",
                      "description": "解释此选项的含义或选择后会发生什么。对于提供关于权衡或影响的上下文很有用。不得包含前导或尾随空格。"
                    }
                  },
                  "required": [
                    "label",
                    "description"
                  ],
                  "additionalProperties": false
                },
                "minItems": 2,
                "maxItems": 4,
                "description": "此问题的可用选择。必须有 2-4 个选项。每个选项必须是具有“label”和“description”字段的对象（不是纯字符串）。“label”字段用作查找用户答案的键，因此不得有前导或尾随空格（例如，{\"label\":\"认证方法\"} 是正确的，{\" label\":\"认证方法\"} 或 {\"label \":\"认证方法\"} 周围有空格是不正确的）。每个选项应该是不同的、互斥的选择（除非启用了 multiSelect）。不要包含“其他”或“其他”选项——UI 会在预定义选项下方自动提供一个自由文本输入字段，以便用户在没有合适的选择时输入自定义答案。包含此类选项将导致重复的 UI 元素。重要提示：始终将选项格式化为对象，例如 [{\"label\": \"选项 1\", \"description\": \"描述 1\"}, {\"label\": \"选项 2\", \"description\": \"描述 2\"}]，切勿作为纯字符串。"
              },
              "multiSelect": {
                "type": "boolean",
                "default": false,
                "description": "设置为 true 以允许用户选择多个选项，而不仅仅是一个。当选择不是互斥时使用。"
              }
            },
            "required": [
              "question",
              "header",
              "options"
            ],
            "additionalProperties": false
          },
          "minItems": 1,
          "maxItems": 4,
          "description": "要向用户提出的问题（1-4 个问题）"
        },
        "answers": {
          "type": "object",
          "additionalProperties": {
            "type": "string"
          },
          "description": "由权限组件收集的用户答案"
        },
        "metadata": {
          "type": "object",
          "properties": {
            "source": {
              "type": "string",
              "description": "此问题来源的可选标识符（例如，用于 /remember 命令的“remember”）。用于分析跟踪。"
            }
          },
          "additionalProperties": false,
          "description": "用于跟踪和分析目的的可选元数据。不显示给用户。"
        }
      },
      "required": [
        "questions"
      ],
      "additionalProperties": false,
      "$schema": "http://json-schema.org/draft-07/schema#"
    },
    "strict": false
  }
}
```