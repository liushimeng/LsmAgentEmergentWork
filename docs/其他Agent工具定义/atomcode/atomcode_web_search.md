# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "web_search",
    "description": "在网络上搜索信息——返回标题、URL 和摘要片段。用于查找文档、查询 API、调研类库，或获取本地没有的信息；随后可对某个结果 URL 调用 `web_fetch` 来阅读全文。`max_results` 可限制结果数量（默认 8）。",
    "parameters": {
      "type": "object",
      "properties": {
        "query": {
          "type": "string",
          "description": "搜索查询词"
        },
        "max_results": {
          "type": "integer",
          "description": "最大结果数（默认 8）"
        }
      },
      "required": [
        "query"
      ]
    }
  }
}
```