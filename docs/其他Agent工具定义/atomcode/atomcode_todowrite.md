# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "todowrite",
    "description": "为当前编码会话创建并维护一个结构化任务列表。有两种调用方式：\n• 计划 / 重新规划（PLAN / RE-PLAN）——发送完整列表：`{\"todos\":[{\"content\":\"…\",\"status\":\"pending|in_progress|completed\"}]}`（会替换之前的列表）。适用于工作包含多个请求、阶段、文件、依赖关系、不确定性，或需要先调研再修改的情况。即使具体步骤要在探索中才逐步明确，非平凡的重构也应使用此方式。初始列表应覆盖完整的工作——从调研、架构/模块设计到实现和验证（如适用）——而不仅是下一步动作。只有在真正简单的单次编辑、信息性提问或单命令请求时才可跳过。\n• 更新单项（初始规划之后的首选方式——不要重发整个列表）：`{\"action\":\"update\",\"id\":N,\"status\":\"in_progress|completed|pending\"}` 修改单个任务（`id` 是该任务在列表中的编号，如 `#3`）；`{\"action\":\"add\",\"content\":\"…\"}` 追加一个新的待办任务。开始执行任务的瞬间就将其设为 `in_progress`；实际完成（已验证）的瞬间就将其设为 `completed`。\n每个任务都是一个具体的、可验证的动作，有明确的产出，后续轮次无需重新规划即可执行——应写 `add error handling to load_config`（为 load_config 添加错误处理），而不是 `handle errors`（处理错误）、`task 1`（任务 1）这类笼统描述。同一时刻只保留恰好一个 `in_progress` 任务（系统自动强制）。只有工作实际完成时才标记为 `completed`，绝不能凭意图标记。",
    "parameters": {
      "type": "object",
      "properties": {
        "todos": {
          "type": "array",
          "description": "计划/重新规划：完整的任务列表——会替换之前的列表。",
          "items": {
            "type": "object",
            "properties": {
              "content": {
                "type": "string",
                "description": "简短、可执行的任务描述。"
              },
              "status": {
                "type": "string",
                "enum": [
                  "pending",
                  "in_progress",
                  "completed"
                ],
                "description": "任务状态。"
              }
            },
            "required": [
              "content",
              "status"
            ]
          }
        },
        "action": {
          "type": "string",
          "enum": [
            "add",
            "update"
          ],
          "description": "更新单项（不要重发整个列表）：`add` 追加任务，或 `update` 修改单个任务的状态。"
        },
        "id": {
          "type": "integer",
          "description": "用于 action=update：要修改的任务编号，从 1 开始（如显示的 #3）。"
        },
        "status": {
          "type": "string",
          "enum": [
            "pending",
            "in_progress",
            "completed"
          ],
          "description": "用于 action=update：新的状态。"
        },
        "content": {
          "type": "string",
          "description": "用于 action=add：新任务的描述。"
        }
      }
    }
  }
}
```