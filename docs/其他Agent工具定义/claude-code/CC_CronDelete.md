# CronDelete 工具定义
```json
{
    "name": "CronDelete",
    "description": "取消通过「定时任务创建」工具添加的定时任务。会从配置文件 .claude/scheduled_tasks.json（持久化任务）或会话内存（临时任务）中移除该任务。",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "id": {
                "description": "由定时任务创建工具返回的任务ID",
                "type": "string"
            }
        },
        "required": [
            "id"
        ],
        "additionalProperties": false
    }
}
```
