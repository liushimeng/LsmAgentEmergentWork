# cronjob 工具定义
```json
{
"type": "function",
"function": {
  "name": "cronjob",
  "description": " 通过单一集成工具管理定时 cron 任务。
  使用 action='create'，根据提示词或一个 / 多个技能创建新定时任务。
  使用 action='list' 查看所有任务信息。
  使用 action='update'、'pause'、'resume'、'remove'、'run' 对已有任务进行管理。
  若要删除用户不再需要的任务：先执行 action='list' 获取任务 ID，再携带该任务 ID 执行 action='remove'。严禁猜测任务 ID，必须先列出任务列表。
  定时任务会在全新会话中运行，不携带当前对话上下文，因此提示词必须具备完整自包含信息。
  创建任务时若指定技能列表，定时任务执行时会按顺序加载这些技能，再将提示词作为任务指令执行。
  更新任务时传入 skills=[] 会清空已绑定的全部技能。
  注意：智能体最终回复会自动发送至目标渠道。面向用户展示的核心内容需放在最终回复中。定时任务为全自动后台运行，无用户交互，无法发起提问或向用户确认信息。
  重要安全规则：定时任务运行会话不允许递归创建新的定时任务。",
  "parameters": {
    "type": "object",
    "properties": {
      "action": {
        "type": "string",
        "description": "可选值：create、list、update、pause、resume、remove、run。当 action=create 时，schedule 和 prompt 字段为必填项。"
      },
      "job_id": {
        "type": "string",
        "description": "执行 update/pause/resume/remove/run 操作时必填"
      },
      "prompt": {
        "type": "string",
        "description": "创建任务专用：完整自包含的任务提示词。若同时配置技能列表，该提示词将作为搭配技能执行的任务指令。"
      },
      "schedule": {
        "type": "string",
        "description": "action=create 时必填。创建 / 更新任务支持格式：'30m'、'every 2h'、'0 9 * * *' 或 ISO 时间戳。示例：'30m'（每 30 分钟）、'every 2h'（每 2 小时）、'0 9 * * *'（每日早上 9 点）、'2026-06-01T09:00:00'（一次性定时任务）。执行创建操作时必须填写此字段。"
      },
      "name": {
        "type": "string",
        "description": "可选，便于人工识别的任务名称"
      },
      "repeat": {
        "type": "integer",
        "description": "可选，任务重复执行次数。不填则使用默认规则：一次性任务仅执行一次，循环任务永久重复执行。"
      },
      "deliver": {
        "type": "string",
        "description": "不填该参数时，消息会自动推送至当前对话会话（推荐）。自动识别机制会保留对话线程与主题上下文。仅当用户要求推送至其他会话渠道时才手动指定。可选值：'origin'（等同于不填参数）、'local'（仅本地保存，不推送消息）、'all'（广播至所有已连接终端渠道），或指定目标格式 platform:chat_id:thread_id。多目标用英文逗号分隔，例如 'origin,all' 代表同时推送原会话与全部渠道。示例：'telegram:-1001234567890:17585'、'discord:#engineering'、'sms:+15551234567'、'all'。警告：仅填写 platform:chat_id 缺少 :thread_id 会丢失主题定位。'all' 在任务触发时动态读取渠道列表，任务创建后新增的连接渠道会自动接收消息。"
      },
      "skills": {
        "type": "array",
        "items": {
          "type": "string"
        },
        "description": "可选，按加载顺序排列的技能名称列表，定时任务执行前会加载这些技能。更新任务时传入空数组可清空已绑定技能。"
      },
      "model": {
        "type": "object",
        "description": "可选，单独为当前任务覆盖全局模型配置。若省略服务商配置，创建任务时会锁定当前主服务商，保证任务运行环境稳定。",
        "properties": {
          "provider": {
            "type": "string",
            "description": "服务商名称（例如 'openrouter'、'anthropic'，自定义服务商格式为 custom:<名称>，必须携带冒号与后缀名称，不可仅填写 custom）。不填则锁定使用当前默认服务商。"
          },
          "model": {
            "type": "string",
            "description": "模型名称（例如 'anthropic/claude-sonnet-4'、'claude-sonnet-4'）"
          }
        },
        "required": [
        "model"
        ]
      },
      "script": {
      "type": "string",
      "description": "可选，定时周期执行脚本路径。默认模式下脚本标准输出会作为上下文注入智能体提示词（适用于数据采集、变更检测场景）。当 no_agent=True 时，脚本即为任务主体，标准输出原文直接推送（传统监控脚本模式）。相对路径默认解析目录～/.hermes/scripts/。后缀为 .sh/.bash 的脚本通过 bash 运行，其余脚本通过 Python 执行。更新任务时传入空字符串可清空脚本配置。"
      },
      "no_agent": {
      "type": "boolean",
      "default": false,
      "description": " 默认值：False（大模型驱动任务，每个周期智能体都会执行提示词）。设为 True 则完全跳过大模型流程：调度器仅按周期运行脚本，并直接推送脚本输出原文。不消耗 Token、不运行智能体循环、不读取自定义模型配置。
      设为 True 时强制要求：必须配置 script 字段（prompt 与 skills 参数会被忽略）。
      True 模式消息推送规则：(a) 非空标准输出会原样作为消息发送；(b) 输出为空则静默处理，不会推送任何消息，脚本可在无上报内容时不输出信息（监控脚本标准用法）；(c) 脚本非零退出码或执行超时会推送错误告警，避免监控故障静默失效。
      适用场景：仅靠脚本输出的周期性轮询任务（内存 / 磁盘 / GPU 监控、阈值告警、心跳上报、CI 通知、固定输出格式 API 轮询）。默认 False 适用场景：需要逻辑推理的任务，如资讯汇总、每日简报撰写、筛选重点内容、文本润色、基于数据执行条件分支逻辑。"
      },
      "context_from": {
      "type": "array",
      "items": {
      "type": "string"
      },
      "description": "可选，任务 ID 数组，每次运行前会将这些任务最近一次成功执行的输出作为上下文注入当前任务提示词。用于串联多个定时任务：任务 A 采集数据，任务 B 处理数据。数组内每项必须是通过 cronjob action='list' 获取的有效任务 ID。注：仅读取上游任务最新完成结果，不会等待同一周期内正在运行的上游任务。更新任务时传入空数组可清空配置。"
      },
      "enabled_toolsets": {
      "type": "array",
      "items": {
      "type": "string"
      },
      "description": "可选，限定任务智能体可用工具集列表（例如 ["web","terminal","file","delegation"]）。配置后仅加载列表内工具，大幅减少输入 Token 消耗。不填则加载全部默认工具。可根据任务提示词推断所需工具：调用网页搜索填"web"，执行脚本填"terminal"，读取文件填"file"，分发子任务填"delegation"。更新任务时传入空数组可清空限制。"
      },
      "workdir": {
        "type": "string",
        "description": "可选，任务运行的绝对工作目录。配置后该目录下 AGENTS.md/ CLAUDE.md/.cursorrules 文件会注入系统提示词，终端、文件、代码执行工具均以此目录为工作目录，适合绑定特定代码仓库执行任务。路径必须为已存在的绝对路径。不填（默认）则沿用原有逻辑：不加载项目配置文件，工具使用调度器根目录。更新任务时传入空字符串可清空目录配置。配置工作目录的任务会串行执行，保证各任务目录环境相互隔离。"
      }
    },
    "required": [ "action" ]
  }
}
}
```