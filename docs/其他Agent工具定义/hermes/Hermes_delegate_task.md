# delegate_task 工具定义
```json
{
"type": "function",
"function": {
  "name": "delegate_task",
  "description": "生成一个或多个子代理（subagents），在隔离的上下文中处理任务。每个子代理拥有独立的对话、终端会话和工具集。仅返回最终摘要——中间工具结果永远不会进入您的上下文窗口。

  两种模式（'goal' 或 'tasks' 之一为必填）：
  1. 单任务模式：提供 'goal'（+ 可选的 context 和 toolsets）。
  2. 批量（并行）模式：提供 'tasks' 数组，当前用户最多可同时并发 3 个任务（通过 config.yaml 中的 delegation.max_concurrent_children 配置）。当前用户的嵌套委托已关闭（max_spawn_depth=1）：每个子代理都是叶子节点，无法进一步委托。如需启用嵌套，请在 config.yaml 中提高 delegation.max_spawn_depth。

  两种模式均在后台运行。delegate_task 会立即返回——您和用户可以继续工作，每个子代理的完整结果会在其完成时作为新消息重新进入对话。批量模式只是 N 个独立的后台子代理（N 个句柄，各自独立完成任务）。请勿等待或轮询；分发任务后直接继续其他工作即可。

  何时使用 delegate_task：
  - 需要重度推理的子任务（调试、代码审查、研究综合）
  - 会将中间数据淹没您上下文的任务
  - 并行的独立工作流（同时研究 A 和 B）

  何时不应使用（请改用以下替代方案）：
  - 不需要推理的机械性多步骤工作 -> 使用 execute_code
  - 单次工具调用 -> 直接调用该工具
  - 需要用户交互的任务 -> 子代理无法使用 clarify
  - 必须比当前回合存活更久的持久化长时间运行工作 -> 使用 cronjob (action='create') 或 terminal(background=True, notify_on_complete=True)。后台委托【不】具备持久性：如果父会话被关闭 (/new) 或在子代理完成前进程退出，该子代理的工作将被丢弃；/stop 会取消所有正在运行的后台子代理。

  重要注意事项：
  - 子代理【没有】您对话的记忆。请通过 'context' 字段传递所有相关信息（文件路径、错误消息、约束条件）。
  - 如果用户使用非英语书写，或要求以特定语言/语气/风格输出，请在 'context' 中说明（例如 respond in Chinese、return output in Japanese）。否则子代理默认使用英语，其摘要可能会以错误的语言污染您的最终回复。
  - 子代理的摘要是【自我报告】，而非经过验证的事实。声称“上传成功”或“文件已写入”的子代理可能是错的。对于具有外部副作用的操作（HTTP POST/PUT、远程写入、共享路径下的文件创建、发布），要求子代理返回可验证的句柄（URL、ID、绝对路径、HTTP 状态码），并在告知用户操作成功前自行验证——获取 URL、检查文件状态、读回内容。
  - 叶子子代理（role='leaf'，默认）【不能】调用：delegate_task、clarify、memory、send_message、execute_code。
  - 编排器子代理（role='orchestrator'）保留 delegate_task 权限以便生成自己的工作线程，但仍不能使用 clarify、memory、send_message 或 execute_code。当前用户的编排器受 max_spawn_depth=1 限制，可通过 delegation.orchestrator_enabled=false 全局禁用。
  - 无法在每次调用时选择子代理模型：子代理继承父模型（及其回退链），除非您通过 config.yaml 中的 delegation.provider / delegation.model 将所有子代理固定到某个模型。
  - 每个子代理拥有独立的终端会话（独立的工作目录和状态）。
  - 结果始终以数组形式返回，每个任务对应一个条目。",
  "parameters": {
    "type": "object",
    "properties": {
    "goal": {
    "type": "string",
    "description": "子代理应完成的目标。请具体且自包含——子代理对您的对话历史一无所知。"
    }
    ,
    "context": {
    "type": "string",
    "description": "子代理所需的背景信息：文件路径、错误消息、项目结构、约束条件。您描述得越具体，子代理的表现越好。"
    }
    ,
    "toolsets": {
      "type": "array",
      "items": {
      "type": "string"
      }
      ,
      "description": "为此子代理启用的工具集。默认：继承您已启用的工具集。可用工具集：'browser'、'coding'、'computer_use'、'cronjob'、'discord'、'discord_admin'、'feishu_doc'、'feishu_drive'、'file'、'homeassistant'、'image_gen'、'kanban'、'search'、'session_search'、'skills'、'spotify'、'terminal'、'todo'、'tts'、'video'、'video_gen'、'vision'、'web'、'x_search'、'yuanbao'。常见组合：代码工作使用 ['terminal', 'file']，研究使用 ['web']，网页交互使用 ['browser']，全栈任务使用 ['terminal', 'file', 'web']。"
    }
    ,
    "tasks": {
      "type": "array",
      "items": {
      "type": "object",
      "properties": {
      "goal": {
      "type": "string",
      "description": "任务目标"
      }
      ,
      "context": {
      "type": "string",
      "description": "特定于任务的上下文"
      }
      ,
      "toolsets": {
      "type": "array",
      "items": {
      "type": "string"
      }
      ,
      "description": "此特定任务的工具集。可用选项：'browser'、'coding'、'computer_use'、'cronjob'、'discord'、'discord_admin'、'feishu_doc'、'feishu_drive'、'file'、'homeassistant'、'image_gen'、'kanban'、'search'、'session_search'、'skills'、'spotify'、'terminal'、'todo'、'tts'、'video'、'video_gen'、'vision'、'web'、'x_search'、'yuanbao'。网络访问使用 'web'，Shell 命令使用 'terminal'，网页交互使用 'browser'。"
      }
      ,
      "acp_command": {
      "type": "string",
      "description": "针对每个任务的 ACP 命令覆盖（例如 'copilot'）。仅针对此任务覆盖顶层的 acp_command。除非用户明确告知已安装 ACP CLI，否则请勿设置。"
      }
      ,
      "acp_args": {
      "type": "array",
      "items": {
      "type": "string"
      }
      ,
      "description": "针对每个任务的 ACP 参数覆盖。除非设置了 acp_command，否则请留空。"
      }
      ,
      "role": {
      "type": "string",
      "enum": [
      "leaf",
      "orchestrator"
      ]
      ,
      "description": "针对每个任务的角色覆盖。语义请参阅顶层 'role' 说明。"
      }
      }
      ,
      "required": [
      "goal"
      ]
      }
      ,
      "description": "批量模式：要并行运行的任务（当前用户最多 3 个，通过 delegation.max_concurrent_children 设置）。每个任务拥有独立的子代理、隔离的上下文和终端会话。提供此参数时，顶层的 goal/context/toolsets 将被忽略。"
    },
    "role": {
      "type": "string",
      "enum": [
      "leaf",
      "orchestrator"
      ]
      ,
      "description": "子代理的角色。'leaf'（默认）= 专注的工作线程，无法进一步委托。'orchestrator' = 可以使用 delegate_task 生成自己的工作线程。当前用户的嵌套已关闭（max_spawn_depth=1）；'orchestrator' 会被静默强制为 'leaf'。如需启用，请在 config.yaml 中提高 delegation.max_spawn_depth。"
    },
    "background": {
      "type": "boolean",
      "description": "已弃用 / 被忽略。单任务委托始终自动在后台运行——您无需（也无法）选择开启或关闭。结果会在子代理完成时作为新消息重新进入对话；在此期间请继续工作。设置此参数无效；保留该参数仅为向后兼容。"
    }
    ,
    "acp_command": {
    "type": "string",
    "description": "覆盖子代理的 ACP 命令（例如 'copilot'。设置后，子代理将使用 ACP 子进程传输，而不是继承父级的传输方式。需要兼容 ACP 的 CLI（目前为通过 'copilot --acp --stdio' 调用的 GitHub Copilot CLI）。实现请参阅 agent/copilot_acp_client.py。重要提示：除非用户明确告知已安装并配置了特定的 ACP 兼容 CLI，否则请勿设置此项。留空则使用父级的默认传输方式（Hermes 子代理）。"
    }
    ,
    "acp_args": {
    "type": "array",
    "items": {
    "type": "string"
    }
    ,
    "description": "ACP 命令的参数（默认：['--acp', '--stdio']）。仅在设置了 acp_command 时使用。除非显式提供了 acp_command，否则请留空。"
    }
    }
  }
}
}
```