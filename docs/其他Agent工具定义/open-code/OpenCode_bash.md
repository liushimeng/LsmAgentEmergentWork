# bash 工具定义
```json
{
  "name": "bash",
  "description": "在具有可选超时设置的持久化 Shell 会话中执行给定的 Bash 命令，确保妥善处理并采取安全措施。

  请注意：操作系统：Linux，Shell：Bash

  所有命令默认在当前工作目录下运行。如果需要在不同目录中运行命令，请使用 `workdir` 参数。避免使用 `cd <目录> && <命令>` 模式，请改用 `workdir`。

  对于工作区之外的临时工作，请使用 `/tmp/opencode`。该目录已经创建，存在且已预先批准用于外部目录访问。

  重要提示：此工具用于 git、npm、docker 等终端操作。请勿将其用于文件操作（读取、写入、编辑、搜索、查找文件）——请改用专门的工具。

  在执行命令之前，请遵循以下步骤：

  1. 目录验证：
    - 如果命令将创建新目录或文件，请先使用 `ls` 验证父目录存在且是正确的位置。
    - 例如，在运行 \"mkdir foo/bar\" 之前，请先使用 `ls foo` 检查 \"foo\" 是否存在且是预期的父目录。

  2. 命令执行：
    - 始终使用双引号将包含空格的文件路径括起来（例如，rm \"路径 有 空格/文件.txt\"）。
    - 正确引用的示例：
      - mkdir \"/Users/name/My Documents\" (正确)
      - mkdir /Users/name/My Documents (错误 - 将失败)
      - python \"/路径/有 空格/script.py\" (正确)
      - python /路径/有 空格/script.py (错误 - 将失败)
    - 在确保正确引用后，执行命令。
    - 捕获命令的输出。

  使用说明：
    - command 参数是必需的。
    - 您可以指定以毫秒为单位的可选超时时间。如果未指定，命令将在 120000ms 后超时。
    - 在 5-10 个字内清晰、简洁地描述该命令的作用非常有帮助。
    - 如果输出超过 2000 行或 51200 字节，它将被截断，完整输出将被写入文件。您可以使用 Read 工具配合 offset/limit 参数读取特定部分，或使用 Grep 搜索完整内容。请勿使用 `head`、`tail` 或其他截断命令来限制输出；完整输出将已保存到文件中以供更精确的搜索。

    - 避免使用 Bash 执行 `find`、`grep`、`cat`、`head`、`tail`、`sed`、`awk` 或 `echo` 命令，除非明确指示或任务确实需要。相反，请始终优先使用专用工具：
      - 文件搜索：使用 Glob（而非 find 或 ls）
      - 内容搜索：使用 Grep（而非 grep 或 rg）
      - 读取文件：使用 Read（而非 cat/head/tail）
      - 编辑文件：使用 Edit（而非 sed/awk）
      - 写入文件：使用 Write（而非 echo >/cat <<EOF）
      - 通信：直接输出文本（而非 echo/printf）
    - 当发出多个命令时：
      - 如果命令相互独立且可以并行运行，请在单条消息中进行多次 bash 工具调用。例如，如果您需要运行 \"git status\" 和 \"git diff\"，请发送一条包含两个并行 bash 工具调用的消息。
      - 如果命令相互依赖且必须按顺序运行，请使用单个 Bash 调用并用 '&&' 连接它们（例如，`git add . && git commit -m \"message\" && git push`）。例如，如果一个操作必须在另一个操作开始前完成（如 mkdir 在 cp 之前，Write 在 Bash 进行 git 操作之前，或 git add 在 git commit 之前），请按顺序运行这些操作。
      - 仅当您需要按顺序运行命令但不关心前面的命令是否失败时，才使用 ';'。
      - 请勿使用换行符分隔命令（在带引号的字符串中可以使用换行符）。
    - 避免使用 `cd <目录> && <命令>`。请改用 `workdir` 参数来更改目录。
      <good-example>
      在 command: pytest tests 中使用 workdir=\"/foo/bar\"
      </good-example>
      <bad-example>
      cd /foo/bar && pytest tests
      </bad-example>

  # Git 和 GitHub
  - 仅在明确要求时才提交、修正、推送或创建 PR。
  - 在提交之前，请检查 `git status`、`git diff` 和 `git log --oneline -10`；仅暂存预期的文件，并且切勿提交密钥。
  - 编写符合仓库风格的简洁提交信息。
  - 除非明确要求，否则不要更新 git 配置、跳过钩子、使用交互式 `-i`、强制推送、创建空提交。
  - 如果提交失败或钩子拒绝提交，请修复问题并创建新提交；不要修正失败的提交。
  - 在创建 PR 之前，请检查状态、差异、远程跟踪、最近的提交以及与基础分支的差异。
  - 审查 PR 中包含的所有提交，而不仅仅是最新的一次提交。
  - 使用 `gh` 处理 GitHub 任务，包括 PR、问题、检查和发布；完成后返回 PR URL。",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "command": {
    "type": "string",
    "description": "要执行的命令"
    },
    "timeout": {
    "minimum": -9007199254740991,
    "exclusiveMinimum": 0,
    "type": "integer",
    "maximum": 9007199254740991,
    "description": "可选的超时时间，单位为毫秒"
    },
    "workdir": {
    "type": "string",
    "description": "运行命令的工作目录。默认为当前目录。请使用此参数而非 'cd' 命令。"
    },
    "description": {
    "type": "string",
    "description": "在 5-10 个字内清晰、简洁地描述该命令的作用。示例：
    输入：ls
    输出：列出当前目录文件

    输入：git status
    输出：显示工作区状态

    输入：npm install
    输出：安装包依赖

    输入：mkdir foo
    输出：创建目录 'foo'"
    }
    },
    "required": [
    "command",
    "description"
    ]
  }
}
```