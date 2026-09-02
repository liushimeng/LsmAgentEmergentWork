# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "schedule_wakeup",
    "description": "在自主节奏的 /loop 中安排下一次恢复工作的时间。仅在 /loop 内调用。\n\n本轮工作结束后，如果任务还需要再执行一轮，调用本工具设置下一次唤醒；如果任务已完成或不再需要运行，则不要调用——循环就此结束。\n\n运行时会把 delay_seconds 限制在 [60, 3600] 区间内。",
    "parameters": {
      "type": "object",
      "properties": {
        "delay_seconds": {
          "type": "integer"
        },
        "reason": {
          "type": "string"
        },
        "prompt": {
          "type": "string"
        }
      },
      "required": [
        "delay_seconds",
        "reason",
        "prompt"
      ]
    }
  }
}
```