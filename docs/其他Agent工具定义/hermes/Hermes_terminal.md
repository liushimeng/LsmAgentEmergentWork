# terminal 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "terminal",
    "description": " 在 Linux 环境中执行 Shell 命令。文件系统、当前工作目录、已导出环境变量会在多次调用间持久保留。
    禁止使用 cat/head/tail 读取文件，请改用 read_file；
    禁止使用 grep/rg/find 检索内容，请改用 search_files；
    禁止使用 ls 列出目录，请改用 search_files (target='files')；
    禁止使用 sed/awk 修改文件，请改用 patch；
    禁止使用 echo/cat 多行文本语法创建文件，请改用 write_file。
    terminal 工具仅用于：项目构建、软件安装、Git 操作、进程管理、脚本执行、网络请求、包管理器，以及所有必须依托 Shell 运行的操作。
    由于导出的环境变量状态会持续生效，每个会话仅需激活一次虚拟环境或导出环境配置；除非命令执行后检测到 Shell 环境被重置，否则无需每条命令前都重新加载环境配置。
    前台模式（默认）：命令执行完毕会立刻返回结果，即便设置了较大超时时间。长时间构建 / 脚本可设置 timeout=300，执行速度快的命令仍会秒级返回。短命令优先使用前台模式。
    后台模式：设置 background=true 会返回会话 ID，几乎必须搭配 notify_on_complete=true 使用；仅开启后台无通知时进程静默运行，若不主动调用 process (action='poll') 查询，无法知晓进程执行状态。仅两种合法使用场景：
    常驻永不退出进程（服务、监听程序、守护进程）：无执行结束节点，静默运行符合预期。
    有明确结束节点的长耗时任务（测试、构建、部署、CI 轮询、批量任务）：必须开启 notify_on_complete=true，否则容易遗忘轮询、持续空等等待用户反馈结果。
    运行服务 / 监听程序时，前台模式不要使用 Shell 后台包装命令（nohup/disown/setsid/ 行尾 &），统一设置 background=true，方便 Hermes 统一管控进程生命周期与输出日志。
    启动服务后，通过健康检测或日志标识确认就绪，再单独调用 terminal 执行测试逻辑，避免无意义的循环 sleep 等待。
    使用 process (action="poll") 查看执行进度，process (action="wait") 阻塞等待进程结束。
    工作目录：通过 workdir 参数单独指定本条命令的运行目录。
    伪终端模式：设置 pty=true 适配交互式命令行工具（Codex、Claude Code、Python 交互式终端）。
    未开启 pty=true 时禁止运行 vim/nano 等交互式编辑器，会因缺少伪终端卡死；Git 输出若存在分页器，可通过管道转发至 cat 避免分页阻塞。",
    "parameters": {
      "type": "object",
      "properties": {
        "command": {
        "type": "string",
        "description": "虚拟机中待执行的 Shell 命令"
        },
        "background": {
        "type": "boolean",
        "description": "将命令放到后台执行。几乎都需要搭配 notify_on_complete=true；不开启通知时进程静默运行，只能手动调用 process 轮询状态，长任务极易遗漏结果。两种标准用法：(1) 常驻不退出进程（服务、监听、守护程序），无结束节点，无需通知；(2) 有明确终点的长耗时任务（测试、构建、部署、CI 轮询、批量任务），必须开启完成通知。短命令建议使用前台模式并设置充足超时时间。",
        "default": false
        },
        "timeout": {
        "type": "integer",
        "description": "最大等待秒数（默认 180 秒，前台模式上限 600 秒）。命令完成会立即返回，长任务可设置更大值，不会产生无效等待。前台模式超时超过 600 秒会被拒绝，长时间任务请改用后台模式。",
        "minimum": 1
        },
        "workdir": {
        "type": "string",
        "description": "本条命令的工作目录，填写绝对路径。默认沿用会话全局工作目录。"
        },
        "pty": {
        "type": "boolean",
        "description": "伪终端模式，适配 Codex、Claude Code、Python REPL 等交互式命令行工具。仅本地与 SSH 后端支持，默认关闭。",
        "default": false
        },
        "notify_on_complete": {
        "type": "boolean",
        "description": "开启且 background=true 时，进程结束后自动推送一次完成通知。绝大多数长耗时任务都应使用该配置：测试、构建、部署、批量任务、运行时长超过一分钟且有明确终止节点的操作。开启后可继续处理其他工作，系统会在进程退出时主动推送结果。与 watch_patterns 互斥，同时设置时会丢弃 watch_patterns 配置。",
        "default": false
        },
        "watch_patterns": {
        "type": "array",
        "items": {
        "type": "string"
        },
        "description": "后台进程输出日志监控关键词数组。严格限流：单个进程每 15 秒最多推送一条匹配通知，冷却期内匹配内容会直接丢弃。连续 3 个 15 秒窗口都触发丢弃后，该进程会自动关闭关键词监控，切换为仅进程结束通知模式，避免消息刷屏。仅适用于常驻、永不退出进程中极罕见的一次性中间状态提示，例如服务日志 ['Application startup complete']、守护进程 ['migration done']。禁止用于：(1) 执行结束标识（DONE/PASS），请改用 notify_on_complete；(2) 循环批量任务中的报错关键词（ERROR/Traceback），会频繁触发限流；(3) 与完成通知同时搭配使用。拿不准时优先选择 notify_on_complete。与 notify_on_complete 互斥，二者只能选其一配置。"
        }
      },
      "required": [
      "command"
      ]
    }
  }
}
```