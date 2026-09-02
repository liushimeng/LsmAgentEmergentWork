# mcp__ardot__fetch_variables 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__fetch_variables",
        "description": "获取此设计文件中定义的变量。

- 可利用本工具返回的数据创建可在代码项目中使用的全局 CSS 样式规则。
- 根据设计稿生成代码时，调用本工具获取全局样式规范。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```