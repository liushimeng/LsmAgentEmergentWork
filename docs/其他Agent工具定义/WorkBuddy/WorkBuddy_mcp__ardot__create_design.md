# mcp__ardot__create_design 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__create_design",
        "description": "使用该工具（create_design）创建全新的Ardot设计文件，并在交互式编辑器界面中将其打开。当用户要求创建、新建、发起、制作或初始化Ardot设计、文档、草稿、画布或文件时调用此工具。可选择性传入文件名称；若未填写，则使用默认名称。

该工具为异步执行——它仅触发创建流程，文档需要数秒时间完成载入。工具返回结果后，请轮询调用 fetch_file_info 检查文档是否就绪。如果返回 NO_ADAPTER，等待3秒后重试，最多重试3次。连续重试3次仍然失败则停止操作并告知用户，不要尝试通过其他替代方案继续执行设计相关任务。仅当 fetch_file_info 调用成功后，才能执行后续操作。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [],
            "properties": {
                "fileName": {
                    "description": "选填，新设计文件的名称。如果不填写，将使用默认名称。",
                    "type": "string"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```