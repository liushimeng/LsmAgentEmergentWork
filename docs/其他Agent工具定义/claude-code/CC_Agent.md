# Agent 工具定义
```json
{
    "name": "Agent",
    "description": "启动新代理来处理复杂的多步骤任务。每种代理类型都具备专属能力与可用工具。

    可用代理类型及其权限工具：
    - claude：通用代理，适用于无法匹配其他专用代理的所有任务。未指定代理名称时，FleetView 默认使用此类型。（可用工具：全部）
    - claude-code-guide：当用户咨询以下相关问题时使用该代理（提问句式如“Claude 能否……”、“Claude 有……”、“如何使用 Claude……”）：(1) 命令行工具 Claude Code，包括功能、钩子、斜杠指令、MCP 服务、配置项、IDE 集成、快捷键；(2) Claude 代理开发套件，用于构建自定义代理；(3) Claude 应用程序接口（原 Anthropic API），包含接口调用、工具使用、官方开发套件用法。**重要提示**：新建代理前，请检查是否存在正在运行或刚结束的 claude-code-guide 代理，可通过发送消息继续复用，无需重复创建。（可用工具：命令行、读取、网页拉取、网页搜索）
    - Explore：只读检索代理，适用于大范围并行检索。当需要遍历大量文件、目录或命名规则，且仅需最终结论、无需完整文件内容时使用。该代理仅读取文件片段以定位代码，不做代码评审与审计。可指定检索深度：medium 代表常规检索，very thorough 代表全面检索，覆盖多路径与命名规则。（可用工具：除代理、退出规划模式、编辑、写入、笔记本编辑外的全部工具）
    - general-purpose：通用代理，用于调研复杂问题、检索代码以及执行多步骤任务。若检索关键词或文件时，无法保证前几次查找就能匹配到目标，可交由该代理执行检索。（可用工具：全部）
    - Plan：架构设计代理，用于制定实施方案。需要规划任务执行方案时使用，可输出分步计划、标记核心文件，并权衡架构设计的优劣取舍。（可用工具：除代理、退出规划模式、编辑、写入、笔记本编辑外的全部工具）
    - statusline-setup：用于配置用户的 Claude Code 状态栏设置。（可用工具：读取、编辑）

    使用本工具时，通过 subagent_type 参数指定代理类型，省略该参数则默认使用通用代理。

    ## 使用场景
    当任务可匹配对应专用代理、需要并行执行独立工作，或是需要读取多个文件才能作答时，可使用本工具委派任务，仅保留最终结论，无需留存完整文件内容。若已知目标文件、符号或数据，仅查询单一信息，直接检索即可，无需委派代理。委派检索任务后，不要重复自行执行，等待代理返回结果。

    - 工具返回结果为代理的最终输出内容，该内容不会直接展示给用户，只需转述关键信息。
    - 可通过代理ID或名称调用发送消息接口，接续已有代理并保留上下文；重新调用代理工具则会创建全新会话。
    - 配置 `isolation: "worktree"` 可为代理分配独立 Git 工作区，工作区无文件变更时会自动清理。
    - 配置 `run_in_background: true` 可让代理在后台异步运行，执行完成后会收到通知。
    - 如需同时启动多个独立代理，可在同一条消息中批量调用工具，实现并发执行。",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "description": {
                "description": "任务简述（3-5个词语）",
                "type": "string"
            },
            "prompt": {
                "description": "分配给代理的任务内容",
                "type": "string"
            },
            "subagent_type": {
                "description": "本次任务使用的专用代理类型",
                "type": "string"
            },
            "model": {
                "description": "可选，为当前代理单独指定模型，优先级高于代理预设模型。未填写则使用代理默认模型，或继承上级模型配置。",
                "type": "string",
                "enum": [
                    "sonnet",
                    "opus",
                    "haiku"
                ]
            },
            "run_in_background": {
                "description": "设为 true 则在后台运行代理，任务完成后会收到通知",
                "type": "boolean"
            },
            "isolation": {
                "description": "隔离模式。填写 \"worktree\" 会创建临时 Git 工作区，让代理在仓库独立副本中操作。",
                "type": "string",
                "enum": [
                    "worktree"
                ]
            }
        },
        "required": [
            "description",
            "prompt"
        ],
        "additionalProperties": false
    }
}
```