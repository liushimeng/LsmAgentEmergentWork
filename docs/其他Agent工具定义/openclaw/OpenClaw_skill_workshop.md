# skill_workshop 工具定义
```json
{
  "name": "skill_workshop",
  "description": "当需要捕获、改进或明确批准可复用的流程时，用于创建、更新、修订、列出、检查、应用、拒绝或隔离“技能工坊（Skill Workshop）”的提案。",
  "parameters": {
    "type": "object",
    "required": [
    "action"
    ],
    "properties": {
    "action": {
    "type": "string",
    "enum": [
    "create",
    "update",
    "revise",
    "list",
    "inspect",
    "apply",
    "reject",
    "quarantine"
    ],
    "description": "create 用于新建技能提案，update 用于更新现有技能，revise 用于修订待处理的提案，list 或 inspect 用于发现和查看提案，apply/reject/quarantine 用于对提案执行明确的生命周期操作。"
    },
    "proposal_id": {
    "type": "string",
    "description": "当 action 为 inspect、revise、apply、reject 或 quarantine 时，提供现有提案的 ID。"
    },
    "name": {
    "type": "string",
    "description": "技能/提案名称。当 action=create 时为必填项；当 proposal_id 未知且 action 为 inspect 或 revise 时，可作为可选的解析器。"
    },
    "query": {
    "type": "string",
    "description": "当 action=list 时的可选查询参数。"
    },
    "status": {
    "type": "string",
    "enum": [
    "pending",
    "applied",
    "rejected",
    "quarantined",
    "stale"
    ],
    "description": "当 action=list 时的可选提案状态过滤器。"
    },
    "limit": {
    "type": "integer",
    "minimum": 1,
    "maximum": 50,
    "description": "当 action=list 时返回的最大提案数量。默认值为 20。"
    },
    "description": {
    "type": "string",
    "maxLength": 160,
    "description": "当 action 为 create、update 或 revise 时的技能描述。请保持简明扼要；最大长度为 160 字节。"
    },
    "skill_name": {
    "type": "string",
    "description": "当 action=update 时，提供现有的技能名称或键。"
    },
    "proposal_content": {
    "type": "string",
    "description": "当 action 为 create、update 或 revise 时，提供完整的提议流程 Markdown 内容。该文件将被保存为 PROPOSAL.md。大小需保持在配置的 skills.workshop.maxSkillBytes 以内；默认最大值为 40000 字节。"
    },
    "support_files": {
    "type": "array",
    "items": {
    "type": "object",
    "required": [
    "path",
    "content"
    ],
    "properties": {
    "path": {
    "type": "string",
    "description": "位于 assets/、examples/、references/、scripts/ 或 templates/ 目录下的相对支持文件路径。"
    },
    "content": {
    "type": "string",
    "description": "支持文件的文本内容。"
    }
    },
    "additionalProperties": false
    },
    "description": "可选的附加文件，将随提案一起保存。"
    },
    "goal": {
    "type": "string",
    "description": "提案或改进的目标。"
    },
    "evidence": {
    "type": "string",
    "description": "简短的证据或备注。"
    },
    "reason": {
    "type": "string",
    "description": "当 action 为 apply、reject 或 quarantine 时的可选操作原因。"
    }
    },
    "additionalProperties": false
  }
}
```