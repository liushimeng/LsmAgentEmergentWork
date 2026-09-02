# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "task",
    "description": "将一个或多个子任务分派给隔离子代理。每个任务：{description, prompt, subagent_type: 'explore'|'worker', difficulty: 'simple'|'hard', model?: 已配置的模型 ID, role?: 角色配置}。'explore' = 只读调研并返回发现结果；'worker' = 修改文件后即停止（之后由你审查 diff）。可选角色包括 architect、reviewer、tester、rust 和 tui_ux 等；角色权限必须与 subagent_type 匹配。'simple' 使用快速模型运行，'hard' 使用高能力模型运行。分派多个任务时，给每个 worker 定义紧凑明确的任务并分配互不重叠的文件范围。子代理并行运行，且自身不能再分派任务。整批任务会作为一个 JSON 载荷一次性发出，因此每个 `prompt` 要保持简洁，并小批量分派（每次几个）：一次调用中过多的长 prompt 可能超出模型输出上限而被判定为无效 JSON——宁可用几次较小的调用，也不要一次巨大的调用。每个 `worker` 必须声明 `scope`（相对于工作目录的 glob 列表），列出其可写入的文件；并行 worker 的 scope 必须互不重叠。",
    "parameters": {
      "type": "object",
      "properties": {
        "tasks": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "description": {
                "type": "string",
                "description": "3-5 个词的简短标签"
              },
              "prompt": {
                "type": "string",
                "description": "分派给子代理的完整子任务内容"
              },
              "subagent_type": {
                "type": "string",
                "enum": [
                  "explore",
                  "worker"
                ]
              },
              "difficulty": {
                "type": "string",
                "enum": [
                  "simple",
                  "hard"
                ]
              },
              "model": {
                "type": "string",
                "description": "可选的、已配置的模型选择 ID；会覆盖难度级别的模型路由"
              },
              "role": {
                "type": "string",
                "enum": [
                  "planner",
                  "architect",
                  "explorer",
                  "implementer",
                  "rust",
                  "tui_ux",
                  "reviewer",
                  "tester",
                  "debugger",
                  "security",
                  "performance",
                  "docs_writer",
                  "release_manager",
                  "migration_compat"
                ]
              },
              "scope": {
                "type": "array",
                "items": {
                  "type": "string"
                },
                "description": "仅适用于 worker，且 worker 必填：worker 可写入的、相对于工作目录的 glob 范围（例如 [\"src/auth/\", \"Cargo.toml\"]）。worker 只能写入该范围内的文件；读取不受限制。explore 类型会忽略此参数。"
              }
            },
            "required": [
              "description",
              "prompt",
              "subagent_type"
            ]
          }
        }
      },
      "required": [
        "tasks"
      ]
    }
  }
}
```