# mcp__ide__executeCode 工具定义
```json
{
    "name": "mcp__ide__executeCode",
    "description": "在当前笔记本文件的 Jupyter 内核中运行 Python 代码。

    所有代码均会在当前 Jupyter 内核中执行。

    除非用户明确要求，否则请勿新增变量或修改内核运行状态。

    执行过的代码逻辑与数据会在多次调用本工具之间持续保留，重启内核后才会清空。",
    "input_schema": {
        "type": "object",
        "properties": {
            "code": {
                "type": "string",
                "description": "将要在内核中执行的代码"
            }
        },
        "required": [
            "code"
        ],
        "additionalProperties": false,
        "$schema": "http://json-schema.org/draft-07/schema#"
    }
}
```