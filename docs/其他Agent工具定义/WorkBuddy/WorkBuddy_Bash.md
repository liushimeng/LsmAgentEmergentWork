# Bash 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "Bash",
        "description": "执行指定的 bash 命令并返回命令输出结果。

工作目录在多次命令调用间保持不变，但 Shell 环境状态不会持久。Shell 环境变量会从用户配置文件初始化。

重要说明：用户默认 Shell 为 zsh。编写命令时需要使用兼容该 Shell 的语法。


重要规则：尽量不要使用该工具执行 `cat`、`head`、`tail`、`sed`、`awk`、`echo` 命令，除非用户明确要求，或是确认专用工具无法完成任务。应当优先使用对应的专用工具，以提供更好使用体验：

- 读取文件：使用 Read（禁止使用 cat/head/tail）
- 修改文件：使用 Edit（禁止使用 sed/awk）
- 写入文件：使用 Write（禁止使用 echo > / cat <<EOF）
- 文本输出：直接返回文字内容（禁止使用 echo/printf）

重要规则：尽量不要使用该工具执行 `find`、`grep`、`rg` 命令，除非用户明确要求，或是确认专用工具无法完成任务。应当优先使用对应的专用工具，以提供更好使用体验：

- 文件检索：使用 Glob（禁止使用 find、ls）
- 内容检索：使用 Grep（禁止使用 grep、rg）

虽然 Bash 工具能够实现上述功能，但优先使用内置专用工具，方便校验工具调用记录、权限审批流程更友好。

# 使用指引
- 如果命令需要新建目录或文件，先执行 `ls` 校验上级目录是否存在、路径位置是否正确。
- 文件路径包含空格时，在命令中使用双引号包裹路径（例如 cd \"path with spaces/file.txt\"）。
- 尽量使用绝对路径、避免频繁使用 `cd`，维持会话工作目录稳定。仅当用户明确要求时使用 `cd`。特别注意：不要在 git 命令前拼接 `cd <当前目录>`；git 会直接在当前工作区执行，拼接写法会触发权限确认弹窗。
- 可以可选设置超时时间，单位毫秒（上限 600000ms）。若命令可能运行较长时间（安装程序、构建流程、长时间运行程序、数据处理），**建议不填写 timeout 参数**。系统默认超时（120000ms）由环境变量 `BASH_DEFAULT_TIMEOUT_MS` 控制，用户可自行调整，无需修改指令。仅当命令执行时长有明确较短上限（快速探测、健康检查）时，才显式指定超时时间。
- 非交互式执行模式（`--print`/`-p`、`--output-format stream-json`）在主智能体本轮任务结束后会终止进程，即便之前命令启动了后台长期子进程。设计多步骤流程（技能脚本、启动构建/分析流水线）时，要么在前台等待子进程执行完毕（不要直接 `nohup ... &` 然后立刻返回），或是合理规划执行轮次，确保最后一步在智能体退出前收集产物与退出码。
- 可通过 `run_in_background` 参数将命令放到后台执行。长时间任务建议开启该参数：依赖安装、项目构建、镜像拉取、长时间测试、启动服务等。示例：`yarn install/package/build`、`npm install/build`、`pnpm install`、`docker build/pull`、`cargo build`、`make`、`mvn package`、`go build`、`gradle build`。后台启动后会返回 `task_id`；**无需主动轮询**，命令执行完毕后会通过 `<task-notification>` 消息自动通知。仅收到通知且需要查看输出时，调用 TaskOutput 工具。启用该参数后无需在命令末尾添加 `&`。
- 如果长时间执行的命令忘记开启后台参数并触发前台超时，命令**会自动转入后台运行，不会被强制终止**（不会发送 SIGTERM，不会丢失运行状态）。工具返回结果中会提供新 `task_id`；执行结束后会推送 `<task-notification>`。仅有 `sleep` 命令不会自动后台运行，因为 sleep 的用途本身就是阻塞等待。
- 多条命令执行规范：
  - 命令相互独立、可以并行执行：在单条消息内发起多次 Bash 工具调用。示例：同时执行 git status 和 git diff，并行发起两次调用。
  - 命令存在依赖、必须顺序执行：在单次 Bash 调用中使用 `&&` 串联命令。
  - 使用 `;` 仅适用于顺序执行且不关心前面命令是否执行成功的场景。
  - 禁止使用换行符分隔多条命令（被引号包裹的字符串内部允许换行）。
- Git 命令相关规范：
  - 提交代码优先使用 `/commit` 指令，自动处理 git 安全机制、多行文本格式、提交钩子异常恢复。
  - 需要提交、推送、创建合并请求时，使用 `/commit-push-pr`。
  - 优先新建提交，尽量避免修改已有提交。
  - 执行高危操作前（`git reset --hard`、`git push --force`、`git checkout --`）先评估有无更安全方案；仅在确实必要时执行破坏性操作。
  - 除非用户明确要求，否则不要跳过钩子（`--no-verify`）、绕过签名校验（`--no-gpg-sign`、`-c commit.gpgsign=false`）。钩子执行失败时排查并修复根源问题。
- 避免无意义的 `sleep` 等待：
  - 可以立刻执行的多条命令之间不要增加 sleep，直接依次运行。
  - 长时间任务希望收到完成通知 → 使用 `run_in_background`，无需 sleep。
  - 不要通过 sleep 循环反复重试失败命令，应当定位问题根本原因。
  - 等待由 `run_in_background` 启动的后台任务：任务结束会自动通知，**禁止 sleep、轮询、主动查询进度**。
  - 如果必须轮询外部进程，使用状态检测命令（例如 gh run view），不要先 sleep。
  - 必须使用 sleep 时，缩短等待时长（1~5秒），避免阻塞用户。
- GitHub 相关操作（工单、PR检查、版本发布、评论），通过 Bash 工具调用 `gh` 命令。拿到 GitHub 链接时，使用 gh 获取信息。示例：通过 `gh api repos/foo/bar/pulls/123/comments` 查看PR评论。
",
        "parameters": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "待执行的命令"
                },
                "timeout": {
                    "type": "number",
                    "description": "可选超时时间，单位毫秒。不填则使用系统默认值；实际上限受环境变量 `BASH_MAX_TIMEOUT_MS` / `settings.env.BASH_MAX_TIMEOUT_MS` 限制。查看工具说明获取当前会话生效的默认值与最大值。"
                },
                "description": {
                    "type": "string",
                    "description": "使用主动语态清晰简洁描述这条命令作用。
描述中禁止使用「复杂」「风险」这类词汇。

简单命令（git、npm、标准命令行工具）保持简短（5-10个词语）：
  - ls → \"列出当前目录文件\"
  - git status → \"查看工作区状态\"
  - npm install → \"安装项目依赖包\"

难以一眼看懂的命令（管道命令、特殊参数）详细说明：
  - find . -name \"*.tmp\" -exec rm {} \\; → \"递归查找并删除所有 .tmp 临时文件\"
  - git reset --hard origin/main → \"丢弃本地所有修改，同步远端main分支\"
  - curl -s url | jq '.data[]' → \"请求接口JSON数据，提取data数组内容\"

描述语言必须遵循系统提示词 <response_language>，或是用户输入使用的语言（大概率为中文）。"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "设为 true 时后台运行该命令。后续使用 BashOutput 读取输出内容。"
                },
                "dangerouslyDisableSandbox": {
                    "type": "boolean",
                    "description": "仅当确认命令需要在沙箱外部运行，且预期用户会批准时，设置为 true。

开启后，工具会请求用户明确授权；授权通过后命令脱离沙箱隔离运行。用户依然有权拒绝执行。

不要预先开启该参数。仅在沙箱模式执行失败（返回信息包含 SANDBOX PERMISSION DENIED），或是任务本身无法在沙箱内执行（例如安装系统软件包、修改工作目录以外文件）时使用。"
                }
            },
            "required": [
                "command"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```