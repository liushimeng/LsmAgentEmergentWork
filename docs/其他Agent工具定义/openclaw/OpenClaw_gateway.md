# gateway 工具定义
```json
{
  "name": "gateway",
  "description": "网关的重启、配置或更新操作。在编辑配置之前，请使用 config.schema.lookup 并指定具体的点号路径。对于部分合并，优先使用 config.patch；仅在需要全量替换时使用 config.apply。对于有意删除数组条目的 config.patch 操作，请传递 replacePaths 并指定受影响的准确数组路径。系统会根据需要执行热重载或重启。始终传递面向用户的 `note` 参数，以便在重启后发送提示。如果重启后必须在内部继续执行后续工作，请传递一次性执行的 `continuationMessage`；若该轮次之后需要可见的后续跟进，必须使用 message 工具。请勿直接写入重启标记文件。",
  "parameters": {
    "type": "object",
    "required": [
    "action"
    ]
    ,
    "properties": {
    "action": {
    "type": "string",
    "enum": [
    "restart",
    "config.get",
    "config.schema.lookup",
    "config.apply",
    "config.patch",
    "update.run"
    ]
    }
    ,
    "delayMs": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "reason": {
    "type": "string"
    }
    ,
    "continuationMessage": {
    "type": "string"
    }
    ,
    "gatewayUrl": {
    "type": "string"
    }
    ,
    "gatewayToken": {
    "type": "string"
    }
    ,
    "timeoutMs": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "path": {
    "type": "string"
    }
    ,
    "raw": {
    "type": "string"
    }
    ,
    "baseHash": {
    "type": "string"
    }
    ,
    "replacePaths": {
    "type": "array",
    "items": {
    "type": "string"
    }
    ,
    "maxItems": 256
    }
    ,
    "sessionKey": {
    "type": "string"
    }
    ,
    "note": {
    "type": "string"
    }
    ,
    "restartDelayMs": {
    "type": "integer",
    "minimum": 0
    }
    }
  }
}
```