# process 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "process",
    "description": "管理通过 terminal (background=true) 启动的后台进程。支持操作：'list'（列出全部进程）、'poll'（查询运行状态与新增输出）、'log'（分页查看完整输出日志）、'wait'（阻塞等待进程结束或超时）、'kill'（终止进程）、'write'（向标准输入发送原始文本，不带换行符）、'submit'（发送文本并自动追加回车，用于应答交互提示）、'close'（关闭标准输入并发送文件结束符 EOF）。",
    "parameters": {
      "type": "object",
      "properties": {
      "action": {
      "type": "string",
      "enum": [
      "list",
      "poll",
      "log",
      "wait",
      "kill",
      "write",
      "submit",
      "close"
      ],
      "description": "针对后台进程执行的操作类型"
      },
      "session_id": {
      "type": "string",
      "description": "进程会话 ID（来自后台 terminal 工具返回结果）。除 'list' 操作外，其余所有操作均必填。"
      },
      "data": {
      "type": "string",
      "description": "发送至进程标准输入的文本（仅用于 'write' 和'submit' 操作）"
      },
      "timeout": {
      "type": "integer",
      "description": "'wait' 操作的最大阻塞等待秒数。超时后会返回当前已产生的部分输出。",
      "minimum": 1
      },
      "offset": {
      "type": "integer",
      "description": "'log' 操作的日志起始行偏移量（默认读取最后 200 行）"
      },
      "limit": {
      "type": "integer",
      "description": "'log' 操作最多返回的日志行数",
      "minimum": 1
      }
      },
      "required": [
      "action"
      ]
    }
  }
}
```