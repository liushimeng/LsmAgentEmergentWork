# todowrite 工具定义
```json
{
  "name": "todowrite",
  "description": "为当前编码会话创建并维护一个结构化的任务列表。跟踪进度、组织多步骤工作，并向用户展示状态。

  ## 何时使用
  在以下情况主动使用：
  - 任务需要 3 个或更多不同的步骤或操作（不仅仅是单个概念步骤的 3 次工具调用）
  - 工作非微不足道且能从规划中受益
  - 用户提供了多个任务（编号或逗号分隔）或明确要求生成待办事项列表
  - 收到新指令时 - 将其作为待办事项捕获
  - 开始任务时 - 在工作前将其标记为 `in_progress`（一次只有一个）
  - 完成任务时 - 将其标记为 `completed` 并添加在工作中发现的任何后续任务

  ## 何时不应使用
  在以下情况跳过：
  - 工作是单一、直接的任务（或少于 3 个微不足道的步骤）
  - 请求纯粹是信息性的或对话式的
  - 跟踪不会增加组织价值

  ## 状态
  - `pending` - 尚未开始
  - `in_progress` - 正在积极处理（剩余工作时一次仅有一个）
  - `completed` - 已成功完成
  - `cancelled` - 不再需要

  ## 规则
  - 实时更新状态；不要批量完成
  - 仅在所需工作实际完成后标记为 `completed`，包括任何必要的验证。绝不能仅基于意图。
  - 当仍有工作时，保持恰好一个 `in_progress`
  - 如果受阻或部分完成，请保持 `in_progress` 并添加描述阻碍的后续待办事项
  - 逐字保留用户提供的命令（标志、参数、顺序）
  - 项目应具体且可操作；将大工作分解为更小的步骤

  ## 示例

  使用它：
  - “添加深色模式切换并运行测试” -> 多步骤功能 + 明确验证
  - “在整个仓库中将 getCwd 重命名为 getCurrentWorkingDirectory” -> grep 显示 8 个文件中有 15 个出现
  - “实现注册、目录、购物车、结账” -> 多个复杂功能

  跳过它：
  - “如何用 Python 打印 Hello World？” -> 信息性
  - “在 calculateTotal 中添加注释” -> 单次编辑
  - “运行 npm install 并告诉我发生了什么” -> 一个命令

  如有疑问，请使用它。",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "todos": {
    "type": "array",
    "items": {
    "type": "object",
    "properties": {
    "content": {
    "type": "string",
    "description": "任务的简短描述"
    },
    "status": {
    "type": "string",
    "description": "任务的当前状态：pending, in_progress, completed, cancelled"
    },
    "priority": {
    "type": "string",
    "description": "任务的优先级：high, medium, low"
    }
    },
    "required": [
    "content",
    "status",
    "priority"
    ]
    },
    "description": "更新后的待办事项列表"
    }
    },
    "required": [
    "todos"
    ]
  }
}
```