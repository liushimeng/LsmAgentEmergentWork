# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "use_skill",
    "description": "调用指定名称的技能（可复用的提示词/工作流模板），并返回将你的参数代入后的技能内容。名称必须与系统提示中「=== AVAILABLE SKILLS ===」下列出的技能或 list_skills 返回的技能完全一致。绝不编造或猜测技能名称。当任务与某个技能的描述相符时就应触发该技能——而不限于用户明确点名时。list_skills 还会显示未收录进提示目录的低优先级技能。",
    "parameters": {
      "type": "object",
      "properties": {
        "name": {
          "type": "string",
          "description": "来自 AVAILABLE SKILLS 或 list_skills 的确切技能名称；绝不可编造名称"
        },
        "arguments": {
          "type": "string",
          "description": "传递给技能的参数（可选）"
        }
      },
      "required": [
        "name"
      ]
    }
  }
}
```