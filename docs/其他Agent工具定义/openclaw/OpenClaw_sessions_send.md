# sessions_send 工具定义
```json
{
  "name": "sessions_send",
  "description": "通过 sessionKey/label 向可见的会话发送消息，或通过 agentId 向已配置的代理发送消息。拒绝线程范围内的聊天，目标为父级频道会话。若缺少已配置代理的主会话则自动创建；在可用时等待回复。",
  "parameters": {
    "type": "object",
    "required": [
    "message"
    ],
    "properties": {
    "sessionKey": {
    "type": "string"
    },
    "label": {
    "type": "string",
    "minLength": 1,
    "maxLength": 512
    },
    "agentId": {
    "type": "string",
    "minLength": 1,
    "maxLength": 64
    },
    "message": {
    "type": "string"
    },
    "timeoutSeconds": {
    "type": "integer",
    "minimum": 0
    }
    }
  }
}
```