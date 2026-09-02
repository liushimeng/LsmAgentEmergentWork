# Agent 工具定义
```json
{
  "name": "atomgit_pr",
  "description": "操作 AtomGit 的 Pull Request。action：\"list\"（需要 owner+repo；可选 state=open|closed|all、limit），\"view\"（需要 owner+repo+number），\"create\"（需要 owner+repo+title+head+base；可选 body），\"close\"（需要 owner+repo+number），\"comment_create\"/\"comment_view\"（需要 owner+repo+number；create 需传 body），\"comment_edit\"/\"comment_delete\"（需要 owner+repo+comment_id；edit 需传 body），\"comment_reply\"（需要 owner+repo+number+parent_id+body），\"link_issues\"/\"unlink_issues\"（需要 owner+repo+number+issues=[编号数组]）。",
  "parameters": {
    "type": "object",
    "properties": {
      "action": {
        "type": "string",
        "enum": [
          "list",
          "view",
          "create",
          "close",
          "comment_create",
          "comment_view",
          "comment_edit",
          "comment_delete",
          "comment_reply",
          "link_issues",
          "unlink_issues"
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
        "description": "PR 编号。"
      },
      "title": {
        "type": "string"
      },
      "body": {
        "type": "string"
      },
      "base": {
        "type": "string",
        "description": "基础分支（create 时使用）。"
      },
      "head": {
        "type": "string",
        "description": "源分支（create 时使用），例如 \"owner/repo:branch\"。"
      },
      "comment_id": {
        "type": "integer",
        "description": "用于 comment_edit/comment_delete 的评论 ID。"
      },
      "parent_id": {
        "type": "integer",
        "description": "用于 comment_reply 的父评论 ID。"
      },
      "issues": {
        "type": "array",
        "items": {
          "type": "integer"
        },
        "description": "用于关联/取消关联的 Issue 编号数组。"
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