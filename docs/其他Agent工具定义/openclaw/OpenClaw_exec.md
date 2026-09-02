# exec 工具定义
```json
{
  "name": "exec",
  "description": "执行 Shell 命令，支持对立即开始的任务进行后台延续。使用 yieldMs/background 参数以便稍后通过 process 工具继续处理。对于立即开始的长时间运行任务，如果启用了自动完成唤醒功能，且命令有输出或失败，请依赖该功能；否则，请使用 process 工具来确认任务完成。当需要查看日志、状态、输入或进行干预时，请随时使用 process 工具。请勿使用 exec sleep 或延迟循环来设置提醒或延迟后续操作；请改用 cron 工具。对于需要 TTY 的命令（如终端 UI、编码代理），请使用 pty=true。",
  "parameters": {
    "type": "object",
    "required": [
    "command"
    ]
    ,
    "properties": {
    "command": {
    "type": "string",
    "description": "要执行的 Shell 命令"
    }
    ,
    "workdir": {
    "type": "string",
    "description": "工作目录（默认为当前工作目录 cwd）"
    }
    ,
    "env": {
    "type": "object",
    "patternProperties": {
      "^.*$": {
      "type": "string"
      }
    }
    }
    ,
    "yieldMs": {
    "type": "number",
    "description": "转入后台前等待的毫秒数（默认为 10000）"
    }
    ,
    "background": {
    "type": "boolean",
    "description": "立即在后台运行"
    }
    ,
    "timeout": {
    "type": "number",
    "description": "超时时间（单位：秒，可选，超时后会终止进程）"
    }
    ,
    "pty": {
    "type": "boolean",
    "description": "在可用时于伪终端（PTY）中运行（适用于需要 TTY 的命令行界面、编码代理）"
    }
    ,
    "elevated": {
    "type": "boolean",
    "description": "在主机上以提升的权限运行（如果允许）"
    }
    ,
    "host": {
    "type": "string",
    "enum": [
    "auto",
    "sandbox",
    "gateway",
    "node"
    ]
    ,
    "description": "执行主机/目标（auto|sandbox|gateway|node）。"
    }
    ,
    "security": {
    "type": "string",
    "description": "常规调用时会被忽略；exec 的安全性由 tools.exec.security 和主机审批设置决定。"
    }
    ,
    "ask": {
    "type": "string",
    "description": "基础的询问机制来自 tools.exec.ask 和主机审批；当有效的主机询问机制关闭时，来自渠道的请求会忽略单次调用的 ask 参数。"
    }
    ,
    "node": {
    "type": "string",
    "description": "当 host=node 时使用的节点 ID 或名称。"
    }
    }
  }
}
```