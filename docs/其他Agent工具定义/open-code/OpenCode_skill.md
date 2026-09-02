# skill 定义工具
```json
{
  "name": "skill",
  "description": "当手头的任务与系统提示中列出的某项技能匹配时，加载该项专门技能。\n\n使用此工具将技能的指令和资源注入当前对话。输出可能包含详细的工作流程指南，以及对同一技能目录下的脚本、文件等的引用。\n\n技能名称必须与系统提示中列出的可用技能之一完全匹配。",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "name": {
    "type": "string",
    "description": "来自 available_skills 的技能名称"
    }
    },
    "required": [
    "name"
    ]
  }
}
```