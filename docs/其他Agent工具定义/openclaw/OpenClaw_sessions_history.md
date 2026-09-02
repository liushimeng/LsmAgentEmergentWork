# sessions_history 工具定义
```json
{
  "name": "sessions_history",
  "description": "获取可见会话的脱敏历史记录。建议在回复、调试或恢复会话前使用；支持限制条数和包含工具消息。",
  "parameters": {
    "type": "object",
    "required": [
    "sessionKey"
    ]
    ,
    "properties": {
    "sessionKey": {
    "type": "string"
    }
    ,
    "limit": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "includeTools": {
    "type": "boolean"
    }
    }
  }
}
```