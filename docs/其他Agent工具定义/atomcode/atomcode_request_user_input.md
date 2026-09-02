# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "request_user_input",
    "description": "向用户提出结构化问题，并等待其回答后再继续。仅用于真正需要用户决定的事项——偏好、确认、方案之间的选择——而非任何你自己可以决定、查找或验证的内容。当用户明确要求你推荐、比较或提供选项供其挑选时（例如「推荐几个 X 让我选」「让我挑一个」），请在此处以具体选项呈现（当用户可能要选择多个时，设置 `mode`=\"multiple\"），而不是把列表写成文字段落。单个问题时，设置 `header`、`question`、`mode`（\"single\"=单选，\"multiple\"=多选，\"text\"=自由输入）和 `options`（single/multiple 模式下不能为空）。若要在一次交互中提出最多 4 个相关问题，请改为传入包含相同结构的 `questions` 数组。single/multiple 模式下会自动附加一个自由输入的「自行输入答案」选项，因此不要自己添加「其他」/兜底选项。每个 `header` 保持简短（几个词）。",
    "parameters": {
      "type": "object",
      "properties": {
        "header": {
          "type": "string",
          "description": "非常简短的标签（几个词）。"
        },
        "question": {
          "type": "string",
          "description": "一个清晰的句子，最好以「？」结尾。"
        },
        "mode": {
          "type": "string",
          "enum": [
            "single",
            "multiple",
            "text"
          ]
        },
        "options": {
          "type": "array",
          "description": "single/multiple 模式的选项；text 模式省略。",
          "items": {
            "type": "object",
            "required": [
              "label"
            ],
            "properties": {
              "label": {
                "type": "string"
              },
              "description": {
                "type": "string"
              }
            }
          }
        },
        "questions": {
          "type": "array",
          "description": "一次交互中回答的最多 4 个问题。要么提供顶层的 header/question/mode/options 用于单个问题，要么提供此数组。",
          "maxItems": 4,
          "items": {
            "type": "object",
            "required": [
              "header",
              "question",
              "mode"
            ],
            "properties": {
              "header": {
                "type": "string",
                "description": "非常简短的标签（几个词）。"
              },
              "question": {
                "type": "string",
                "description": "一个清晰的句子，最好以「？」结尾。"
              },
              "mode": {
                "type": "string",
                "enum": [
                  "single",
                  "multiple",
                  "text"
                ]
              },
              "options": {
                "type": "array",
                "description": "single/multiple 模式的选项；text 模式省略。",
                "items": {
                  "type": "object",
                  "required": [
                    "label"
                  ],
                  "properties": {
                    "label": {
                      "type": "string"
                    },
                    "description": {
                      "type": "string"
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
```