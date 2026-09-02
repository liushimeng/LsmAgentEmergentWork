# session_status 工具定义
```json
{
  "name": "session_status",
  "description": "显示当前/可见会话的类似 /status 的状态卡片：包含模型、使用情况、时间、费用和任务。使用 `sessionKey=\"current\"` 表示当前会话；UI 界面标签（如 `openclaw-tui`）并非会话键。`model` 参数用于设置会话模型覆盖；`model=default` 用于重置。适用于查询当前活跃的模型或会话配置相关问题。",
  "parameters": {
    "type": "object",
    "properties": {
    "sessionKey": {
    "type": "string"
    }
    ,
    "model": {
    "type": "string"
    }
    }
  }
}
```