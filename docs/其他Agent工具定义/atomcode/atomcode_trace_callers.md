# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "trace_callers",
    "description": "追踪某个符号的所有调用方（反向调用图），按 BFS 广度优先搜索至指定深度。展示调用链及其深度和定义所在文件。示例：{\"symbol\":\"process\",\"depth\":3}。",
    "parameters": {
      "type": "object",
      "properties": {
        "symbol": {
          "type": "string",
          "description": "要追踪调用方的符号名称"
        },
        "depth": {
          "type": "integer",
          "description": "最大遍历深度（默认 3，最大 5）"
        }
      },
      "required": [
        "symbol"
      ]
    }
  }
}
```