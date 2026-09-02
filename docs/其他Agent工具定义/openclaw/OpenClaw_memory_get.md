# memory_get 工具定义
```json
{
  "name": "memory_get",
  "description": "从 MEMORY.md 或 memory/*.md 中进行安全的精确摘录读取。当省略行数参数时，默认返回有界摘录；当存在更多内容时，会包含截断或续读信息；当设置 `corpus=wiki` 时，会从已注册的编译型 wiki 补充内容中进行读取。",
  "parameters": {
    "type": "object",
    "properties": {
    "path": {
    "type": "string"
    }
    ,
    "from": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "lines": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "corpus": {
    "type": "string",
    "enum": [
    "memory",
    "wiki",
    "all"
    ]
    }
    }
    ,
    "required": [
    "path"
    ]
    ,
    "additionalProperties": false
  }
}
```