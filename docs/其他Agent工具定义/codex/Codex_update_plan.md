# update_plan 工具定义
```json
{
"type": "function",
"name": "update_plan",
"description": " 更新任务计划。
可提供一段可选说明以及计划项列表，每个计划项包含步骤内容与状态。
同一时间最多只能有一个步骤处于进行中状态。
",
"strict": false,
"parameters": {
"type": "object",
"properties": {
"explanation": {
"type": "string",
"description": "本次计划更新的可选说明文字。"
}
,
"plan": {
"type": "array",
"description": "步骤列表",
"items": {
"type": "object",
"properties": {
"status": {
"type": "string",
"description": "步骤状态。",
"enum": [
"pending",
"in_progress",
"completed"
]
}
,
"step": {
"type": "string",
"description": "任务步骤描述文本。"
}
}
,
"required": [
"step",
"status"
]
,
"additionalProperties": false
}
}
}
,
"required": [
"plan"
]
,
"additionalProperties": false
}
}
```