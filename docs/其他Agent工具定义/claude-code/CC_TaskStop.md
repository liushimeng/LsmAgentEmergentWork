# TaskStop 工具定义
```json
{
    "name": "TaskStop",
    "description": "
    - 根据任务ID终止正在运行的后台任务
    - 传入 task_id 参数指定要停止的任务
    - 返回执行成功或失败状态
    - 如需终止长时间运行的任务，可使用本工具
    ",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "对象",
        "properties": {
            "task_id": {
                "description": "需要停止的后台任务ID",
                "type": "字符串"
            },
            "shell_id": {
                "description": "已废弃：请改用 task_id",
                "type": "字符串"
            }
        },
        "additionalProperties": false
    }
}
```