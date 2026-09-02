# Agent 工具定义
```json
{
  "name": "atomgit_repo",
  "description": "操作 AtomGit 的仓库。action：\"list\"（获取你的仓库列表），\"view\"（需要 owner+repo），\"create\"（需要 name；可选 owner=组织、description、private），\"delete\"（需要 owner+repo），\"fork\"（需要 owner+repo；可选 name、private），\"clone\"（需要 owner+repo；可选 branch、dir — 执行本地 `git clone`），\"create_tag\"（需要 owner+repo+tag_name；可选 refs=起始点（默认 main）、tag_message），\"labels\"（需要 owner+repo — 读取项目的标签列表），\"ensure_label\"（需要 owner+repo；可选 label，默认 atomcode）。",
  "parameters": {
    "type": "object",
    "properties": {
      "action": {
        "type": "string",
        "enum": [
          "list",
          "view",
          "create",
          "delete",
          "fork",
          "clone",
          "create_tag",
          "labels",
          "ensure_label"
        ]
      },
      "owner": {
        "type": "string",
        "description": "仓库所有者（create 时填组织名）。创建个人仓库时可省略。"
      },
      "repo": {
        "type": "string",
        "description": "仓库名称，用于 view/delete/fork/clone。"
      },
      "name": {
        "type": "string",
        "description": "新仓库名称（create）或 fork 后的目标仓库名。"
      },
      "description": {
        "type": "string"
      },
      "private": {
        "type": "boolean"
      },
      "branch": {
        "type": "string",
        "description": "要克隆的分支。"
      },
      "dir": {
        "type": "string",
        "description": "克隆的目标目录（相对于工作目录）。"
      },
      "tag_name": {
        "type": "string",
        "description": "新标签名称（create_tag）。"
      },
      "refs": {
        "type": "string",
        "description": "create_tag 的起始点 — 分支/提交/标签（默认 main）。"
      },
      "tag_message": {
        "type": "string",
        "description": "标签描述（create_tag，可选）。"
      },
      "label": {
        "type": "string",
        "description": "用于 ensure_label 的标签名（默认 \"atomcode\"）。"
      },
      "limit": {
        "type": "integer",
        "description": "列表最大返回仓库数（默认 30）。"
      }
    },
    "required": [
      "action"
    ]
  }
}
```