# mcp__ardot__apply_variables 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__apply_variables",
        "description": "创建或更新变量集与变量。

- 默认采用合并模式；设置 `replace: true` 将会删除未列出的变量集/变量。
- 名称禁止包含 `$` 或 `:` 字符。
- 数据格式：`{ "<变量集名称>": { modes?: [...], variables: { "<变量名>": { type, valuesByMode|value, scopes? } } } }`
- 支持类型：BOOLEAN（布尔）、FLOAT（浮点数）、STRING（字符串）、COLOR（颜色，格式`{r,g,b,a}`，取值范围0~1，不支持十六进制色值）。

重要提示：加载 '{SKILL_ROOT}/tool-usage/apply-variables.md' 查阅详细使用文档。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [
                "variables"
            ],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                },
                "variables": {
                    "type": "object",
                    "propertyNames": {
                        "type": "string"
                    },
                    "additionalProperties": {},
                    "description": "待创建或更新的变量集，以变量集名称作为键名。"
                },
                "replace": {
                    "description": "设为 true 时，会完整覆盖已有变量定义，而非合并更新",
                    "type": "boolean"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```