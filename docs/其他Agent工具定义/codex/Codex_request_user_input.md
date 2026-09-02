# request_user_input 工具定义
```json
{
"type": "function",
"name": "request_user_input",
"description": "向用户发起 1 至 3 个简短问题并等待用户回复。仅当该问题有参考价值、不会阻塞流程，且用户未作答时可基于最优判断继续执行的场景，才设置自动超时时间 autoResolutionMs，取值范围 60000 至 240000 毫秒；若必须获取用户明确输入才能继续，则省略该字段。本工具仅可在计划模式下使用。",
"strict": false,
"parameters": {
"type": "object",
"properties": {
"autoResolutionMs": {
"type": "number",
"description": "可选自动超时时长，单位毫秒，范围 60000 至 240000。仅适用于问题有参考意义但不阻塞任务、用户无回复时可自主判断继续执行的场景；若必须等待用户明确输入才能推进则不填该参数。仅作轻度参考信息时设为 60000，答案会大幅推进工作进度时最高可设 240000。"
}
,
"questions": {
"type": "array",
"description": "展示给用户的问题列表，建议仅设置 1 个，最多不超过 3 个",
"items": {
"type": "object",
"properties": {
"header": {
"type": "string",
"description": "界面展示的简短标题标签（字符数不超过 12）。"
}
,
"id": {
"type": "string",
"description": "用于匹配回答的固定标识，采用下划线命名格式。"
}
,
"options": {
"type": "array",
"description": "提供 2 至 3 个互斥选项。推荐选项放置在首位，标签末尾标注「(Recommended)」。列表内不要添加「其他」选项，客户端会自动补充自定义输入的「其他」选项。",
"items": {
"type": "object",
"properties": {
"description": {
"type": "string",
"description": "简短一句话，说明选择该选项带来的影响或利弊取舍。"
}
,
"label": {
"type": "string",
"description": "面向用户展示的选项名称（1-5 个词语）。"
}
}
,
"required": [
"label",
"description"
]
,
"additionalProperties": false
}
}
,
"question": {
"type": "string",
"description": "展示给用户的单句提问内容。"
}
}
,
"required": [
"id",
"header",
"question",
"options"
]
,
"additionalProperties": false
}
}
}
,
"required": [
"questions"
]
,
"additionalProperties": false
}
}
```