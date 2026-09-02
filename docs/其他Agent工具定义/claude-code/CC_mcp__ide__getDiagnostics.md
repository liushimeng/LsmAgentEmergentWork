# mcp__ide__getDiagnostics 工具定义
```json
{
    "name": "mcp__ide__getDiagnostics",
    "description": "从 VS Code 获取代码诊断信息",
    "input_schema": {
        "type": "object",
        "properties": {
            "uri": {
                "type": "string",
                "description": "可选参数，指定要获取诊断信息的文件统一资源标识符。若不填写，则获取所有文件的诊断信息。"
            }
        },
        "additionalProperties": false,
        "$schema": "http://json-schema.org/draft-07/schema#"
    }
}
```