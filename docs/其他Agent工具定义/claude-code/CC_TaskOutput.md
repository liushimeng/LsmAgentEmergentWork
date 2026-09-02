# TaskOutput 工具定义
```json
{
    "name": "TaskOutput",
    "description": "**已废弃**：后台任务会在工具返回结果中输出文件路径，任务完成后你也会收到附带同一路径的 <task-notification> 通知。
    - 脚本任务：建议通过该输出文件路径使用读取工具，文件内包含标准输出与标准错误信息。
    - 本地代理任务：直接使用代理工具的返回结果，**请勿读取**.output 文件。该文件为指向子代理完整会话日志（JSONL格式）的符号链接，会造成上下文窗口溢出。
    - 远程代理任务：建议通过输出文件路径使用读取工具，文件内容为远程会话的流式输出，规则同脚本任务。

    本工具用于获取运行中或已结束任务的输出内容，支持后台命令行、代理任务及远程会话。
    - 传入 task_id 参数指定目标任务
    - 返回任务输出内容及状态信息
    - block 设为 true（默认值）：阻塞等待任务执行完成
    - block 设为 false：非阻塞查询当前运行状态
    - 可通过 /tasks 指令查询任务ID
    - 支持所有任务类型：后台命令行、异步代理、远程会话",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "对象",
        "properties": {
            "task_id": {
                "description": "待获取输出内容的任务ID",
                "type": "字符串"
            },
            "block": {
                "description": "是否阻塞等待任务执行完成",
                "default": true,
                "type": "布尔值"
            },
            "timeout": {
                "description": "最大等待时长，单位：毫秒",
                "default": 30000,
                "type": "数值",
                "minimum": 0,
                "maximum": 600000
            }
        },
        "required": [
            "task_id",
            "block",
            "timeout"
        ],
        "additionalProperties": false
    }
}
```