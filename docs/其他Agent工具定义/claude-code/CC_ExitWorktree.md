# ExitWorktree 工具定义
```json
{
    "name": "ExitWorktree",
    "description": "退出由 EnterWorktree 创建的工作区会话，并将当前会话切回原始工作目录。

    ## 作用范围
    本工具**仅**处理当前会话中通过 EnterWorktree 创建的工作区，不会影响以下内容：
    - 手动执行 `git worktree add` 创建的工作区
    - 上一轮会话创建的工作区（即便当时也是通过 EnterWorktree 创建）
    - 从未调用过 EnterWorktree 时，当前所在的目录

    若在非工作区会话下调用本工具，将**不执行任何操作**：仅提示当前无活跃工作区，文件系统状态保持不变。

    ## 使用场景
    - 用户明确要求“退出工作区”“离开工作区”“返回上级目录”等结束工作区会话的指令
    - 禁止主动调用本工具，仅在用户提出相关要求时使用

    ## 参数说明
    - `action`（必填）：可选值为 `"keep"` 或 `"remove"`
      - `"keep"` — 保留磁盘上的工作区目录与对应分支。适用于后续还要继续使用该工作区，或需要留存代码改动的场景。
      - `"remove"` — 删除工作区目录及对应分支。工作完成或放弃当前任务时，使用该选项彻底清理。
    - `discard_changes`（可选，默认 false）：仅在 `action` 设为 `"remove"` 时生效。若工作区内存在未提交文件、或是存在未合并到原分支的提交记录，**必须将该参数设为 true**，否则工具会拒绝删除并列出相关改动。若工具因检测到改动报错，请先向用户确认，再开启该参数重新执行。

    ## 运行效果
    - 将会话工作目录恢复为进入工作区之前的路径
    - 清空依赖当前工作目录的缓存（系统提示片段、内存文件、方案目录等），让会话状态与原始目录保持一致
    - 若工作区关联了 tmux 会话：选择 `remove` 会终止该 tmux 会话；选择 `keep` 则会保留运行，并返回会话名称方便用户重新连接
    - 退出后，可再次调用 EnterWorktree 创建全新工作区",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "action": {
                "description": "\"keep\" 保留磁盘上的工作区与对应分支；\"remove\" 将两者一并删除。",
                "type": "string",
                "enum": [
                    "keep",
                    "remove"
                ]
            },
            "discard_changes": {
                "description": "当 action 为 \"remove\"，且工作区内存在未提交文件或未合并提交时，必须设为 true，否则工具会拒绝操作并列出相关改动。",
                "type": "boolean"
            }
        },
        "required": [
            "action"
        ],
        "additionalProperties": false
    }
}
```