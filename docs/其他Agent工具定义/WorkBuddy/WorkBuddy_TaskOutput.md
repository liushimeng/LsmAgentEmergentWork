# TaskOutput 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "TaskOutput",
        "description": "- 获取正在运行或已完成任务的输出内容（后台Shell、智能体任务或远程会话）
- 传入 task_id 参数用于指定目标任务
- 返回任务输出内容以及状态信息
- block=true（默认值）：阻塞等待任务执行完成
- block=false：非阻塞方式，仅查询当前运行状态
- 任务ID可通过 /tasks 命令获取
- 支持全部任务类型：后台Shell、异步智能体、远程会话
",
        "parameters": {
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "待查询的后台任务ID"
                },
                "block": {
                    "type": "boolean",
                    "description": "是否阻塞等待任务完成（默认值：true）"
                },
                "timeout": {
                    "type": "number",
                    "default": 60000,
                    "description": "超时时间，单位毫秒（取值范围0-600000，默认：60000）"
                },
                "filter": {
                    "type": "string",
                    "description": "可选正则表达式，用于过滤输出日志行（仅对后台Shell任务生效）"
                }
            },
            "required": [
                "task_id"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```