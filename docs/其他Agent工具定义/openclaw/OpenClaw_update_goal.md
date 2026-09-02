# update_goal 工具定义
```json
{
  "name": "update_goal",
  "description": "仅在目标真正达成时将其标记为完成（complete），或在同一阻塞条件连续出现至少三个目标回合后才将其标记为受阻（blocked）。请勿因普通的困难或细节上的瑕疵而使用受阻状态。",
  "parameters": {
    "type": "object",
    "required": [
    "status"
    ],
    "properties": {
    "status": {
    "type": "string",
    "enum": [
    "complete",
    "blocked"
    ],
    "description": "complete（完成） | blocked（受阻）。"
    },
    "note": {
    "type": "string",
    "description": "简短的状态备注。"
    }
    }
  }
}
```