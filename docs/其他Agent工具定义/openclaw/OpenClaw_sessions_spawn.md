# sessions_spawn 工具定义
```json
{
  "name": "sessions_spawn",
  "description": "生成干净的子会话；默认 `runtime=\"subagent\"`。`mode=\"run\"` 用于一次性后台任务。子代理继承父级工作区。原生子代理会在第一条可见的 `[Subagent Task]` 消息中获取任务。仅限原生模式：仅当子会话需要当前对话记录时使用 `context=\"fork\"`，否则请省略或设置为 `isolated`。用于全新的子会话任务。",
  "parameters": {
    "type": "object",
    "required": [
    "task"
    ],
    "properties": {
    "task": {
    "type": "string"
    },
    "taskName": {
    "type": "string",
    "description": "用于后续定向的稳定别名；仅允许小写字母、数字、下划线和连字符，且必须以字母开头。"
    },
    "label": {
    "type": "string"
    },
    "runtime": {
    "type": "string",
    "enum": [
    "subagent"
    ]
    },
    "agentId": {
    "type": "string"
    },
    "model": {
    "type": "string"
    },
    "thinking": {
    "type": "string"
    },
    "cwd": {
    "type": "string"
    },
    "mode": {
    "type": "string",
    "enum": [
    "run"
    ]
    },
    "cleanup": {
    "type": "string",
    "enum": [
    "delete",
    "keep"
    ]
    },
    "sandbox": {
    "type": "string",
    "enum": [
    "inherit",
    "require"
    ]
    },
    "context": {
    "type": "string",
    "enum": [
    "isolated",
    "fork"
    ],
    "description": "原生上下文。省略或设为 \"isolated\" 以创建干净的子会话；仅当子会话需要请求者的对话记录时才使用 \"fork\"。"
    },
    "lightContext": {
    "type": "boolean",
    "description": "轻量级引导上下文；仅在 runtime=\"subagent\" 时可用。"
    },
    "attachments": {
    "type": "array",
    "items": {
    "type": "object",
    "required": [
    "name",
    "content"
    ],
    "properties": {
    "name": {
    "type": "string"
    },
    "content": {
    "type": "string"
    },
    "encoding": {
    "type": "string",
    "enum": [
    "utf8",
    "base64"
    ]
    },
    "mimeType": {
    "type": "string"
    }
    }
    },
    "maxItems": 50
    },
    "attachAs": {
    "type": "object",
    "properties": {
    "mountPath": {
    "type": "string"
    }
    }
    }
    }
  }
}
```