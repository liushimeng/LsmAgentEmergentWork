# TaskUpdate 工具定义
```json
{
"name": "TaskUpdate",
"description": "使用此工具更新任务列表中的任务。

## 何时使用此工具

**将任务标记为已解决：**
- 当您完成了任务中描述的工作时
- 当任务不再需要或已被取代时
- **重要：** 完成任务后，务必将其标记为已解决
- 解决后，调用 TaskList 以查找下一个任务

- **仅**在完全完成任务时将其标记为已完成
- 如果遇到错误、阻碍或无法完成，请将任务保持为 in_progress 状态
- 当受阻时，创建一个新任务来描述需要解决的问题
- 如果出现以下情况，切勿将任务标记为已完成：
  - 测试失败
  - 实现不完整
  - 遇到未解决的错误
  - 无法找到必要的文件或依赖项

**删除任务：**
- 当任务不再相关或创建有误时
- 将状态设置为 `deleted` 将永久删除该任务

**更新任务详情：**
- 当需求发生变化或变得更加清晰时
- 当建立任务之间的依赖关系时

## 可更新的字段

- **status**: 任务状态（请参阅下方的状态工作流）
- **subject**: 更改任务标题（使用祈使语气，例如“运行测试”）
- **description**: 更改任务描述
- **activeForm**: 当处于 in_progress 状态时，在旋转器中显示的现在分词形式（例如，“正在运行测试”）
- **owner**: 更改任务负责人（代理名称）
- **metadata**: 将元数据键合并到任务中（将键设置为 null 以删除它）
- **addBlocks**: 标记为此任务完成后才能开始的任务
- **addBlockedBy**: 标记为此任务开始前必须完成的任务

## 状态工作流

状态流转：`pending`（待处理） → `in_progress`（进行中） → `completed`（已完成）

使用 `deleted` 永久删除任务。

## 陈旧性

更新任务前，请务必使用 `TaskGet` 读取任务的最新状态。

## 示例

开始工作时将任务标记为进行中：
````json
{\"taskId\": \"1\", \"status\": \"in_progress\"}
````

完成工作后将任务标记为已完成：
````json
{\"taskId\": \"1\", \"status\": \"completed\"}
````

删除任务：
````json
{\"taskId\": \"1\", \"status\": \"deleted\"}
````

通过设置负责人认领任务：
````json
{\"taskId\": \"1\", \"owner\": \"my-name\"}
````

设置任务依赖关系：
````json
{\"taskId\": \"2\", \"addBlockedBy\": [\"1\"]}
````
",
"input_schema": {
"$schema": "https://json-schema.org/draft/2020-12/schema",
"type": "object",
"properties": {
"taskId": {
"description": "要更新的任务的 ID",
"type": "string"
},
"subject": {
"description": "任务的新主题",
"type": "string"
},
"description": {
"description": "任务的新描述",
"type": "string"
},
"activeForm": {
"description": "当处于 in_progress 状态时，在旋转器中显示的现在分词形式（例如，“正在运行测试”）",
"type": "string"
},
"status": {
"description": "任务的新状态",
"anyOf": [
{
"type": "string",
"enum": [
"pending",
"in_progress",
"completed"
]
},
{
"type": "string",
"const": "deleted"
}
]
},
"addBlocks": {
"description": "此任务所阻塞的任务 ID",
"type": "array",
"items": {
"type": "string"
}
},
"addBlockedBy": {
"description": "阻塞此任务的任务 ID",
"type": "array",
"items": {
"type": "string"
}
},
"owner": {
"description": "任务的新负责人",
"type": "string"
},
"metadata": {
"description": "要合并到任务中的元数据键。将键设置为 null 以删除它。",
"type": "object",
"propertyNames": {
"type": "string"
},
"additionalProperties": {}
}
},
"required": [
"taskId"
],
"additionalProperties": false
}
}
```