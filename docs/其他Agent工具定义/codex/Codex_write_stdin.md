# write_stdin 工具定义
```json
{
"type": "function",
"name": "write_stdin",
"description": "向已存在的统一执行会话写入字符，并返回近期输出内容。",
"strict": false,
"parameters": {
"type": "object",
"properties": {
"chars": {
"type": "string",
"description": "需要写入标准输入的字节内容。默认为空，仅轮询数据而不执行写入操作。"
}
,
"max_output_tokens": {
"type": "number",
"description": "输出令牌配额。默认 10000 个令牌；超大请求可能会受策略限制截断。"
}
,
"session_id": {
"type": "number",
"description": "正在运行的统一执行会话唯一标识。"
}
,
"yield_time_ms": {
"type": "number",
"description": "等待后再返回输出数据。非空写入操作默认 250 毫秒，上限 30000 毫秒；空轮询操作默认等待 5000 至 300000 毫秒。"
}
}
,
"required": [
"session_id"
]
,
"additionalProperties": false
}
}
```