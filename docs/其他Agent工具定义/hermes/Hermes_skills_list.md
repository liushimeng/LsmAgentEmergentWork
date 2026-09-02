# skills_list 工具定义
```json
{
"type": "function",
"function": {
"name": "skills_list",
"description": "列出可用的技能（名称 + 描述）。使用 skill_view(name) 加载完整内容。",
"parameters": {
"type": "object",
"properties": {
"category": {
"type": "string",
"description": "可选的分类过滤器，用于缩小结果范围"
}
}
}
}
}
```