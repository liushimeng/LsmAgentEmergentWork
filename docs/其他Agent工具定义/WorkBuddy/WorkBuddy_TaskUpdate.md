# TaskUpdate 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "TaskUpdate",
    "description": "使用此工具更新任务列表中的任务。

## 何时使用此工具

**将任务标记为已解决：**
- 当你已完成任务中描述的工作时
- 当任务不再需要或已被取代时
- 重要：完成任务后，始终将你的任务标记为已解决
- 解决后，调用 TaskList 查找你的下一个任务

- 仅当你已完全完成任务时，才将任务标记为已完成
- 如果你遇到错误、阻碍或无法完成，请将任务保持为 in_progress
- 当受阻时，创建一个新任务来描述需要解决的问题
- 如果出现以下情况，切勿将任务标记为已完成：
  - 测试失败
  - 实现不完整
  - 你遇到了未解决的错误
  - 你找不到必要的文件或依赖项

**删除任务：**
- 当任务不再相关或创建错误时
- 将状态设置为 `deleted` 将永久删除任务

**更新任务详情：**
- 当需求发生变化或变得更清晰时
- 当在任务之间建立依赖关系时

## 可更新的字段

- **status**：任务状态（参见下方的状态工作流）
- **subject**：更改任务标题（命令式，例如，“运行测试”）
- **description**：更改任务描述
- **activeForm**：in_progress 时在微调器中显示的现在进行时（例如，“正在运行测试”）
- **owner**：更改任务所有者（代理名称）
- **metadata**：将元数据键合并到任务中（将键设置为 null 以删除它）
- **addBlocks**：标记在此任务完成之前无法开始的任务
- **addBlockedBy**：标记在此任务开始之前必须完成的任务

## 状态工作流

状态进展：`pending` → `in_progress` → `completed`

使用 `deleted` 永久删除任务。

## 陈旧性

在更新任务之前，请务必使用 `TaskGet` 读取任务的最新状态。

## 示例

开始工作时将任务标记为进行中：
````json
{\"taskId\": \"1\", \"status\": \"in_progress\"}
````

完成后将任务标记为已完成：
````json
{\"taskId\": \"1\", \"status\": \"completed\"}
````

删除不再需要的任务：
````json
{\"taskId\": \"3\", \"status\": \"deleted\"}
````

在任务之间添加依赖关系：
````json
{\"taskId\": \"2\", \"addBlockedBy\": [\"1\"]}
````
",
    "parameters": {
      "type": "object",
      "properties": {
        "taskId": {
          "type": "string",
          "description": "要更新的任务的 ID"
        },
        "subject": {
          "type": "string",
          "description": "任务的新主题"
        },
        "description": {
          "type": "string",
          "description": "任务的新描述"
        },
        "activeForm": {
          "type": "string",
          "description": "in_progress 时在微调器中显示的现在进行时（例如，“正在运行测试”）"
        },
        "status": {
          "type": "string",
          "enum": [
            "pending",
            "in_progress",
            "completed",
            "deleted"
          ],
          "description": "任务的新状态"
        },
        "addBlocks": {
          "type": "array",
          "items": {
            "type": "string"
          },
          "description": "此任务阻止的任务 ID"
        },
        "addBlockedBy": {
          "type": "array",
          "items": {
            "type": "string"
          },
          "description": "阻止此任务的任务 ID"
        },
        "owner": {
          "type": "string",
          "description": "任务的新所有者"
        },
        "metadata": {
          "type": "object",
          "additionalProperties": {},
          "description": "要合并到任务中的元数据键。将键设置为 null 以删除它。"
        }
      },
      "required": [
        "taskId"
      ],
      "additionalProperties": false,
      "$schema": "http://json-schema.org/draft-07/schema#"
    },
    "strict": false
  }
}
```