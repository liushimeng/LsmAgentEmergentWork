# exec_command 工具定义
```json
{
  "type": "function",
  "name": "exec_command",
  "description": "在伪终端中执行命令，返回输出内容或用于持续交互的会话 ID。",
  "strict": false,
  "parameters": {
    "type": "object",
    "properties": {
    "cmd": {
    "type": "string",
    "description": "待执行的 Shell 命令。"
    }
    ,
    "justification": {
    "type": "string",
    "description": " 当开启require_escalated权限时，展示给用户的授权询问文案；无需该权限时省略此字段。"
    }
    ,
    "login": {
    "type": "boolean",
    "description": "为 true 时以登录交互模式（-l/-i）启动 Shell；为 false 则关闭该模式。默认值为 true。"
    }
    ,
    "max_output_tokens": {
    "type": "number",
    "description": "输出 Token 上限。默认 10000 个 Token；超大输出请求可能会受策略限制截断。"
    }
    ,
    "prefix_rule": {
    "type": "array",
    "description": " 命令专用复用授权前缀，仅在sandbox_permissions设为require_escalated时生效；示例：["git", "pull"]。",
    "items": {
    "type": "string"
    }
    }
    ,
    "sandbox_permissions": {
    "type": "string",
    "description": " 单条命令沙箱权限覆盖配置。默认值为use_default；如需脱离沙箱执行请填写require_escalated。",
    "enum": [
    "use_default",
    "require_escalated"
    ]
    }
    ,
    "shell": {
    "type": "string",
    "description": "要启动的 Shell 程序路径。默认使用用户系统默认 Shell。"
    }
    ,
    "tty": {
    "type": "boolean",
    "description": "为 true 时为命令分配伪终端；为 false 或省略时使用普通管道传输输出。"
    }
    ,
    "workdir": {
    "type": "string",
    "description": "命令执行的工作目录。默认使用当前交互回合的工作目录。"
    }
    ,
    "yield_time_ms": {
    "type": "number",
    "description": "输出推送前的等待时长。默认 10000 毫秒；有效取值区间 250~30000 毫秒。"
    }
    }
    ,
    "required": ["cmd"],
    "additionalProperties": false
  }
}
```