# process 工具定义
```json
{
  "name": "process",
  "description": "管理已启动的 exec 会话：支持 list（列表）、poll（轮询）、log（日志）、write（写入）、send-keys（发送按键）、submit（提交）、paste（粘贴）、kill（终止）。当需要获取状态、查看日志、确认静默成功，或在无法使用自动完成唤醒功能时确认任务完成，请使用 poll 或 log。当需要提示等待输入时，也可使用 poll 或 log。当需要输入数据或进行干预时，请使用 write、send-keys、submit、paste 或 kill。请勿使用 process 轮询来模拟定时器或提醒；对于定时后续操作，请使用 cron。",
  "parameters": {
    "type": "object",
    "required": [
    "action"
    ]
    ,
    "properties": {
    "action": {
    "type": "string",
    "description": "进程操作（list|poll|log|write|send-keys|submit|paste|kill|clear|remove）"
    }
    ,
    "sessionId": {
    "type": "string",
    "description": "除 list 外的其他操作所需的会话 ID"
    }
    ,
    "data": {
    "type": "string",
    "description": "write 操作要写入的数据"
    }
    ,
    "keys": {
    "type": "array",
    "items": {
    "type": "string"
    }
    ,
    "description": "send-keys 操作要发送的按键标记"
    }
    ,
    "hex": {
    "type": "array",
    "items": {
    "type": "string"
    }
    ,
    "description": "send-keys 操作要发送的十六进制字节"
    }
    ,
    "literal": {
    "type": "string",
    "description": "send-keys 操作的原始字符串"
    }
    ,
    "text": {
    "type": "string",
    "description": "paste 操作要粘贴的文本"
    }
    ,
    "bracketed": {
    "type": "boolean",
    "description": "在粘贴时使用括号模式（bracketed mode）包裹"
    }
    ,
    "eof": {
    "type": "boolean",
    "description": "写入后关闭标准输入（stdin）"
    }
    ,
    "offset": {
    "type": "number",
    "description": "日志偏移量"
    }
    ,
    "limit": {
    "type": "number",
    "description": "日志长度"
    }
    ,
    "timeout": {
    "type": "number",
    "description": "用于 poll 操作：在返回前最多等待的毫秒数；最大值为 30000 毫秒，高于此值的设定将被截断为 30000",
    "minimum": 0
    }
    }
  }
}
```