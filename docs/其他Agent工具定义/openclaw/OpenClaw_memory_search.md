# memory_search 工具定义
```json
{
  "name": "memory_search",
  "description": "强制回忆步骤：在回答有关先前工作、决策、日期、人物、偏好或待办事项的问题之前，必须对 MEMORY.md 和 memory/*.md（以及可选的会话记录）进行语义搜索。可选参数 `corpus=wiki` 或 `corpus=all` 还会搜索已注册的编译型 wiki 补充内容。`corpus=memory` 会将命中结果限制在已索引的记忆文件中（排除会话记录片段参与排序）。`corpus=sessions` 会将命中结果限制在已索引的会话记录中（适用与会话历史工具相同的可见性规则）。如果响应中包含 disabled=true，则表示记忆检索功能不可用，应向用户说明此情况。",
  "parameters": {
    "type": "object",
    "properties": {
    "query": {
    "type": "string"
    }
    ,
    "maxResults": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "minScore": {
    "type": "number"
    }
    ,
    "corpus": {
    "type": "string",
    "enum": [
    "memory",
    "wiki",
    "all",
    "sessions"
    ]
    }
    }
    ,
    "required": [
    "query"
    ]
    ,
    "additionalProperties": false
  }
}
```