# create_goal 工具定义
```json
{
  "name": "create_goal",
  "description": "仅在用户或系统指令明确要求时创建目标。若目标已存在则会失败；请使用面向用户的目标控制功能来清除现有目标。",
  "parameters": {
    "type": "object",
    "required": [
    "objective"
    ]
    ,
    "properties": {
    "objective": {
    "type": "string",
    "description": "要追求的具体目标。仅在明确要求时创建。"
    }
    ,
    "token_budget": {
    "type": "number",
    "description": "此目标的可选正数 Token 预算。"
    }
    }
  }
}
```