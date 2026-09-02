# Agent 工具定义
```json
{
  "name": "atomgit_issue",
  "description": "操作 AtomGit 的 Issue。action：\"list\"（需要 owner+repo；可选 state=open|closed|all、limit），\"view\"（需要 owner+repo+number），\"create\"（需要 owner+repo+title；可选 body），\"comment_create\"/\"comment_view\"（需要 owner+repo+number；create 需传 body），\"comment_edit\"/\"comment_delete\"（需要 owner+repo+comment_id；edit 需传 body）。",
  "parameters": {
    "type": "object",
    "properties": {
      "action": {
        "type": "string",
        "enum": [
          "list",
          "view",
          "create",
          "comment_create",
          "comment_view",
          "comment_edit",
          "comment_delete"
        ]
      },
      "owner": {
        "type": "string"
      },
      "repo": {
        "type": "string"
      },
      "number": {
        "type": "integer",
        "description": "Issue 编号。"
      },
      "title": {
        "type": "string"
      },
      "body": {
        "type": "string"
      },
      "comment_id": {
        "type": "integer",
        "description": "用于 comment_edit/comment_delete 的评论 ID。"
      },
      "state": {
        "type": "string",
        "description": "列表过滤条件（默认 open）。"
      },
      "limit": {
        "type": "integer",
        "description": "列表最大返回数（默认 30）。"
      }
    },
    "required": [
      "action"
    ]
  }
}
```