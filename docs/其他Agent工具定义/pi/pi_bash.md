# bash 工具定义
```json
{
  "name": "bash",
  "description": "在当前工作目录下执行 bash 命令。返回标准输出（stdout）和标准错误输出（stderr）。输出内容会被截断至最后 2000 行或 50KB（以先达到的限制为准）。若输出被截断，完整内容将保存至临时文件。可选择性地提供超时时间（以秒为单位）。",
  "parameters": {
    "type": "object",
    "required": [
    "command"
    ],
    "properties": {
      "command": {
      "type": "string",
      "description": "要执行的 Bash 命令"
      },
      "timeout": {
      "type": "number",
      "description": "超时时间（单位：秒，可选参数，默认无超时限制）"
      }
    }
  },
  "strict": false
}
```