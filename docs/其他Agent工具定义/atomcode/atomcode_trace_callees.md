# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "trace_callees",
    "description": "追踪某个符号的所有被调用项（正向调用图），按 BFS 广度优先搜索至指定深度。展示被调用链及其深度和定义所在文件。示例：{\"symbol\":\"main\",\"depth\":2}。",
    "parameters": {
      "type": "object",
      "properties": {
        "symbol": {
          "type": "string",
          "description": "要追踪被调用项的符号名称"
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