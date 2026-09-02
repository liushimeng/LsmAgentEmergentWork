# Bash 工具定义
```json
{
    "name": "Bash",
    "description": "执行 Bash 命令并返回输出结果。

    - 多次调用之间工作目录会保留，但建议优先使用绝对路径；组合命令中使用 `cd` 可能触发权限确认。Shell 状态（环境变量、函数）不会持久化，每次都会根据用户配置文件重新初始化。
    - 重要说明：除非收到明确指示，或确认没有专用工具可完成任务，否则请勿使用本工具执行 `cat`、`head`、`tail`、`sed`、`awk`、`echo` 这类命令。请改用对应的专用工具，能为用户带来更好的使用体验。
    - `timeout` 单位为毫秒：默认值 120000，最大值 600000。
    - `run_in_background` 开启后，命令会独立后台运行，跨会话持续执行，命令结束后会自动唤起会话，无需额外添加 `&` 符号。

    # Git 相关规则
    - 当前环境不支持交互式参数（如 `-i`，例如 `git rebase -i`、`git add -i`）。
    - 涉及 GitHub 相关操作（拉取请求、议题、接口调用）请使用 `gh` 命令行工具。
    - 仅在用户要求时执行提交或推送操作；若当前处于默认分支，请先新建分支再操作。
    - Git 提交信息末尾需追加：
    Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
    - 拉取请求描述末尾需追加：
    🤖 Generated with [Claude Code](https://claude.com/claude-code)",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "command": {
                "description": "待执行的命令",
                "type": "string"
            },
            "timeout": {
                "description": "可选超时时间，单位毫秒（最大值 600000）",
                "type": "number"
            },
            "description": {
                "description": "使用主动语态，简洁清晰地描述该命令的作用。描述中不要使用“复杂”“风险”等词汇，仅客观说明功能。

            简单命令（Git、npm、常规命令行工具）控制在 5-10 个单词：
            - ls → 列出当前目录文件
            - git status → 查看工作区状态
            - npm install → 安装项目依赖

            管道命令、特殊参数等不易直观理解的命令，需补充必要上下文说明用途：
            - find . -name \"*.tmp\" -exec rm {} \\; → 递归查找并删除所有 .tmp 后缀文件
            - git reset --hard origin/main → 舍弃本地所有修改，同步远端 main 分支
            - curl -s url | jq '.data[]' → 拉取网页 JSON 数据并提取 data 数组内容",
                "type": "string"
            },
            "run_in_background": {
                "description": "设为 true 则在后台运行该命令",
                "type": "boolean"
            },
            "dangerouslyDisableSandbox": {
                "description": "设为 true 将强制关闭沙箱模式，在无沙箱防护的环境下执行命令",
                "type": "boolean"
            }
        },
        "required": [
            "command"
        ],
        "additionalProperties": false
    }
}
```