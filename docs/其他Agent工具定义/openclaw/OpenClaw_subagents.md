# subagents 工具定义
```json
{
  "name": "subagents",
  "description": "列出请求者会话的活跃及近期子代理。如果存在 sessions_yield，请用它来完成操作；不要使用轮询等待循环。",
  "parameters": {
    "type": "object",
    "properties": {
    "action": {
    "type": "string",
    "enum": [
    "list"
    ]
    },
    "recentMinutes": {
    "type": "integer",
    "minimum": 1
    }
    }
  }
}
```