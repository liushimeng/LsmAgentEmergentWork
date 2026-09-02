# StructuredOutput 工具定义
```json
{
"name": "StructuredOutput",
  "description": "使用此工具以请求的结构化格式返回最终响应。你必须在响应结束时恰好调用此工具一次，以提供结构化输出。",
  "input_schema": {
    "type": "object",
    "properties": {
      "sections": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "name": {
          "type": "string"
          },
          "startLine": {
          "type": "number"
          },
          "endLine": {
          "type": "number"
          },
          "suggestedFileName": {
          "type": "string"
          },
          "description": {
          "type": "string"
          }
          }
        }
      }
    }
  }
}
```