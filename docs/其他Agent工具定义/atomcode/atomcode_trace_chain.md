# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "trace_chain",
    "description": "查找两个符号之间最短的调用链（BFS 广度优先搜索，最多 10 跳）。示例：{\"from\":\"main\",\"to\":\"db_query\"}。",
    "parameters": {
      "type": "object",
      "properties": {
        "from": {
          "type": "string",
          "description": "起始符号名称"
        },
        "to": {
          "type": "string",
          "description": "目标符号名称"
        }
      },
      "required": [
        "from",
        "to"
      ]
    }
  }
}
```