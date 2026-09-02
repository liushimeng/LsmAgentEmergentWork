# create_goal 工具定义
```json
{
"type": "function",
"name": "create_goal",
"description": " 仅在用户或系统 / 开发指引明确要求时创建目标，不得从普通任务自行推断生成目标。
仅当明确指定令牌配额时才填写 token_budget。若存在未完成的目标，调用将失败；仅可使用 update_goal 修改目标状态。",
"strict": false,
"parameters": {
"type": "object",
"properties": {
"objective": {
"type": "string",
"description": "必填项。需要推进完成的具体目标。当前无目标时将创建一条新的活跃目标；已有已完成目标时会替换现有目标。"
}
,
"token_budget": {
"type": "integer",
"description": "新目标可用的正整数令牌配额，无明确要求则省略该字段。"
}
}
,
"required": [
"objective"
]
,
"additionalProperties": false
}
}
```