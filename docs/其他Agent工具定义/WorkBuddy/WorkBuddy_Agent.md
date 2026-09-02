# Agent 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "Agent",
        "description": "启动新智能体，自主处理复杂、多步骤任务。

Agent工具用于启动专用子智能体（子进程），使其自主处理复杂任务。每种智能体类型拥有专属能力与可用工具集。

可用智能体类型及其可访问工具：
- general-purpose（通用型）：通用智能体，用于调研复杂问题、检索代码、执行多步骤任务。当检索关键词或文件，且无法确定前几次检索能否命中目标内容时，使用该智能体代为检索。（可用工具：全部）
- statusline-setup：使用该智能体配置用户的Codebuddy代码状态栏设置。（可用工具：Read、Write、Edit、ToolSearch、DeferExecuteTool、SendMessage、ExitPlanMode）
- Explore（代码探索智能体）：专注代码库探索的轻量智能体。适用于根据文件匹配规则快速查找文件（例如：\"src/components/**/*.tsx\"）、按关键词检索代码（例如：“API接口”）、解答代码库相关问题（例如：“API接口如何工作？”）。调用此智能体时需要指定调研详尽等级：\"quick\"快速基础检索、\"medium\"中等程度探索、\"very thorough\"全面多位置、多命名规则深度分析。（可用工具：Read、Bash、PowerShell、Glob、Grep、TaskCreate、TaskGet、TaskUpdate、TaskList、WebFetch、WebSearch、Skill、SendMessage、ToolSearch、DeferExecuteTool、ExitPlanMode）
- Plan（规划智能体）：专注代码库探索的轻量智能体。适用于根据文件匹配规则快速查找文件（例如：\"src/components/**/*.tsx\"）、按关键词检索代码（例如：“API接口”）、解答代码库相关问题（例如：“API接口如何工作？”）。调用此智能体时需要指定调研详尽等级：\"quick\"快速基础检索、\"medium\"中等程度探索、\"very thorough\"全面多位置、多命名规则深度分析。（可用工具：Read、Write、Edit、Bash、Glob、Grep、TaskCreate、TaskGet、TaskUpdate、TaskList、WebFetch、WebSearch、Skill、ToolSearch、DeferExecuteTool、TeamCreate、TeamDelete、SendMessage、ExitPlanMode）

使用Agent工具时，可以通过subagent_type参数选择智能体类型。如省略参数，默认使用general-purpose通用型智能体，拥有独立运行上下文。

请勿在以下场景使用Agent工具：
- 如果你需要读取明确路径的单个文件，请直接使用Read工具，检索效率更高
- 如果你检索特定类定义（例如class Foo），直接使用Bash工具（grep -rn / rg命令），检索效率更高
- 如果你需要在单个文件或者2~3个文件内检索代码，直接使用Read工具，检索效率更高
- 其他不在上述智能体适用场景内的任务

使用注意事项：
- 必须附带简短描述（3-5个词）概括智能体需要执行的工作
- 启动多个相互独立并行工作的智能体时，在同一条消息内批量发起多个工具调用，使其并发运行
- 智能体执行完成后会向你返回一条消息。智能体返回的结果对用户不可见。如需向用户展示结果，你需要主动向用户发送文字消息，简明汇总执行结果。
- 信任但务必核验：智能体的总结仅描述它计划执行的操作，不一定等同于实际执行结果。当智能体编写或修改代码后，在宣告任务完成前务必检查实际代码变更。
- 你可以通过`run_in_background`参数选择后台运行智能体。后台运行时，系统会在任务结束后自动通知你——**不要循环等待、轮询或主动查询运行进度**，应当继续处理其他工作或回复用户。
- **前台运行 vs 后台运行**：当你需要等待智能体结果才能继续后续工作时，使用前台模式（默认），例如调研类智能体，其结论决定后续操作。如果你存在可以并行开展的独立工作，则选择后台模式。
- 恢复之前创建的智能体：使用SendMessage工具，将智能体名称填入`recipient`字段，即可携带完整上下文恢复运行。全新调用Agent会启动无历史记忆的独立智能体，因此prompt提示词必须包含全部上下文、具备独立性。
- 清晰告知智能体任务目标：仅调研检索（文件读取、网页拉取等）还是需要编写代码；智能体无法主动揣测用户真实意图。
- 若智能体说明中提到支持主动触发，则尽量无需等待用户指令，主动调用该智能体。
- 如果用户明确要求智能体“并行运行”，你必须在同一条消息内提交多个Agent工具调用。例如需要同时启动构建校验智能体与测试执行智能体，将两次调用合并至一条消息发送。

## 如何编写prompt提示词

向智能体下达指令，应当如同向刚到场的资深同事布置任务——它看不到当前对话记录，不清楚你之前尝试过哪些方案，也不了解这项任务的意义。
- 说明你的目标以及任务目的
- 描述已经掌握的信息、已经排除的方案
- 提供充足的业务上下文，使智能体能够自主判断，而非机械执行狭隘指令
- 如果需要简短回复，明确说明（例如：“总结控制在200字以内”）
- 检索类任务：直接提供完整检索命令；调研类任务：直接提出问题。如果前提条件发生变化，固化步骤指令会失效。

简短命令式提示词通常只会产出浅显、通用的结果。

**禁止全权移交逻辑整合工作。** 不要写这类指令：“根据你的调研结果修复漏洞”、“基于调研完成开发”。这类语句将综合决策工作推给智能体，而应当由你本人完成整合。编写提示词时体现你的理解：包含文件路径、行号、明确需要修改的内容。

## 创建团队成员智能体

当存在活跃团队（由TeamCreate创建）时，你可以通过填写`name`以及可选参数`team_name`创建团队成员智能体：
- `name`：待创建智能体名称。运行期间可通过SendMessage({to: name})定向发送消息。
- `subagent_type`：该任务使用的专用智能体类型。省略则使用通用智能体。
- `team_name`：归属团队名称。省略则沿用当前团队上下文。
- `mode`：成员智能体权限模式（例如"plan"代表变更需要审批）
- `max_turns`：智能体最大交互轮次（API往返次数），到达上限后自动停止

## 为团队成员选择智能体类型

通过Agent创建成员智能体时，依据任务所需工具选择`subagent_type`。不同类型智能体拥有不同可用工具集，需要匹配任务场景：
- **只读智能体**（例如Explore、Plan）无法编辑、写入文件。仅分配调研检索、方案规划类任务，禁止分配开发实现工作。
- **全权限智能体**（例如general-purpose）拥有全部工具，支持文件编辑、写入、Bash命令执行。适合需要修改文件的任务。
- 在`.workbuddy/agents/`内自定义的智能体可能存在工具权限限制，请阅读对应描述确认能力边界。

创建成员智能体前，请仔细阅读各类智能体描述及其可用工具列表，再选定`subagent_type`。

团队成员始终以后台分离模式运行，依靠SendMessage通信，通过共享任务列表协同工作。

使用示例：

<example_agent_descriptions>
"code-reviewer": 在完成大量代码编写后调用该智能体
"greeting-responder": 使用该智能体回复用户问候，附带趣味短句
</example_agent_descriptions>

<example>
用户："请编写一个判断数字是否为质数的函数"
助手：我将使用Write工具编写如下代码：
<code>
function isPrime(n) {
  if (n <= 1) return false
  for (let i = 2; i * i <= n; i++) {
    if (n % i === 0) return false
  }
  return true
}
</code>
<commentary>
大量代码已编写完成，任务主体结束，调用code-reviewer智能体执行测试
</commentary>
助手：调用Agent工具启动code-reviewer智能体
</example>

<example>
用户："你好"
<commentary>
用户发起问候，启动greeting-responder智能体，生成友好回复
</commentary>
助手："我将使用Agent工具启动greeting-responder智能体"
</example>",
        "parameters": {
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "简短任务描述（3-5个词语）"
                },
                "prompt": {
                    "type": "string",
                    "description": "分配给智能体执行的任务指令"
                },
                "subagent_type": {
                    "type": "string",
                    "description": "本次任务使用的专用智能体类型"
                },
                "model": {
                    "type": "string",
                    "enum": [
                        "default",
                        "lite",
                        "reasoning"
                    ],
                    "description": "智能体所使用模型变体。\"default\"：继承父级模型，均衡适配通用任务；\"lite\"：轻量快速、成本更低，适合简单检索、快速文件操作、简易任务；\"reasoning\"：增强推理能力，适合复杂分析、架构决策、多步骤难题求解。"
                },
                "resume": {
                    "type": "string",
                    "description": "选填，用于恢复运行的智能体ID。提供该参数时，智能体将基于上一轮运行记录继续执行。"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "设为true则后台运行智能体。后续使用TaskOutput读取运行输出。"
                },
                "name": {
                    "type": "string",
                    "description": "创建智能体的名称。运行期间可通过SendMessage({to: name})定向通信。名称\"team-lead\"为TeamCreate创建的团队负责人预留名称——禁止分配给普通成员；建议使用岗位命名，例如\"pm\"、\"ui-designer\"、\"fe-dev\"。同一团队内名称必须唯一。"
                },
                "team_name": {
                    "type": "string",
                    "description": "目标归属团队名称。省略则沿用当前团队上下文。"
                },
                "mode": {
                    "type": "string",
                    "enum": [
                        "acceptEdits",
                        "bypassPermissions",
                        "default",
                        "plan",
                        "dontAsk",
                        "auto"
                    ],
                    "description": "成员智能体权限模式（例如\"plan\"代表所有变更需要审批）。"
                },
                "max_turns": {
                    "type": "integer",
                    "exclusiveMinimum": 0,
                    "description": "智能体最大交互轮次（API往返次数），达到上限自动停止。"
                }
            },
            "required": [
                "description",
                "prompt"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```