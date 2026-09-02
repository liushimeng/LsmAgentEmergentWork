# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "recall",
    "description": "按主题和/或时间搜索本项目已完成的对话轮次——覆盖所有会话，包括当前会话的较早轮次。轮次只有在其结束后才会被索引，因此正在进行中的轮次（当前正在发生的事）不在此列；那部分请依赖你自己的上下文。用它来回忆过去的决策、bug 或方案，即使来自另一个会话。相对日期（如「昨天」）请自行根据当前日期换算后填入 `after`/`before` 字段。只读工具——结果页脚会标明原始逐轮对话记录的存放位置，需要精确全文时可查阅。",
    "parameters": {
      "type": "object",
      "properties": {
        "query": {
          "type": "string",
          "description": "要回忆的关键词/主题"
        },
        "after": {
          "type": "string",
          "description": "可选的下界（含边界）— ISO 日期时间或 YYYY-MM-DD（本地时间）"
        },
        "before": {
          "type": "string",
          "description": "可选的上界（不含边界）— ISO 日期时间或 YYYY-MM-DD（本地时间）"
        },
        "limit": {
          "type": "integer",
          "description": "最多返回的轮次数（默认 8）"
        }
      },
      "required": [
        "query"
      ]
    }
  }
}
```