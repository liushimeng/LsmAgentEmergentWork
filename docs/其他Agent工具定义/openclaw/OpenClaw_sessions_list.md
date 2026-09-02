# sessions_list 工具定义
```json
{
  "name": "sessions_list",
  "description": "列出可见的会话；支持按类型、标签、智能体ID、搜索词、活跃度进行过滤。建议在调用 sessions_history 或 sessions_send 进行目标选择之前使用此功能。",
  "parameters": {
    "type": "object",
    "properties": {
      "kinds": {
        "type": "array",
        "items": {
          "type": "string"
        }
      },
      "limit": {
        "type": "integer",
        "minimum": 1
      },
      "activeMinutes": {
        "type": "integer",
        "minimum": 1
      },
      "messageLimit": {
        "type": "integer",
        "minimum": 0
      },
      "label": {
        "type": "string",
        "minLength": 1
      },
      "agentId": {
        "type": "string",
        "minLength": 1,
        "maxLength": 64
      },
      "search": {
        "type": "string",
        "minLength": 1
      },
      "includeDerivedTitles": {
        "type": "boolean"
      },
      "includeLastMessage": {
        "type": "boolean"
      }
    }
  }
}
```