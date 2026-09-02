# todo 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "todo",
    "description": "管理当前会话的任务列表。适用于包含 3 个以上步骤的复杂任务，或当用户提供多个任务时使用。不传参数调用即可读取当前列表。

    写入操作：
    - 提供 'todos' 数组以创建/更新任务项
    - merge=false（默认）：使用全新的计划替换整个列表
    - merge=true：根据 id 更新现有任务项，并添加任何新的任务项

    每个任务项结构：{id: 字符串, content: 字符串, status: pending|in_progress|completed|cancelled}
    列表的顺序即代表优先级。同一时间只允许有一个任务处于 in_progress（进行中）状态。
    任务完成后应立即标记为 completed（已完成）。如果某项任务失败，请将其标记为 cancelled（已取消），并添加一个修订后的新任务项。

    每次调用始终返回完整的当前列表。",
    "parameters": {
      "type": "object",
      "properties": {
        "todos": {
        "type": "array",
        "description": "要写入的任务项。省略此参数以读取当前列表。",
        "items": {
        "type": "object",
        "properties": {
        "id": {
        "type": "string",
        "description": "任务项的唯一标识符"
        }
        ,
        "content": {
        "type": "string",
        "description": "任务描述"
        }
        ,
        "status": {
        "type": "string",
        "enum": [
        "pending",
        "in_progress",
        "completed",
        "cancelled"
        ]
        ,
        "description": "当前状态"
        }
        }
        ,
        "required": [
        "id",
        "content",
        "status"
        ]
        }
        }
        ,
        "merge": {
        "type": "boolean",
        "description": "true：根据 id 更新现有任务项并添加新任务项。false（默认）：替换整个列表。",
        "default": false
        }
      }
    }
  }
}
```