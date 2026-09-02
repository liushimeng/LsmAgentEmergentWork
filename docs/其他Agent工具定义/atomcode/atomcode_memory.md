# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "memory",
    "description": "持久保存一条关于用户或本项目的、非显而易见的经验，让未来的会话也能记住。当用户表达长期偏好、以应当牢记的方式纠正你，或你发现了非显而易见的项目约定/特性时，使用 `action:\"remember\"`。使用 `action:\"forget\"` 删除匹配关键词的条目，使用 `action:\"list\"` 查看当前记忆。不要记录：显而易见的事实、标准的工具/语言行为、AGENTS.md/.atomcode.md 中已有的内容、冗长的解释，或仅限单次会话的一次性信息。每条记录保持为一行简洁文字。",
    "parameters": {
      "type": "object",
      "properties": {
        "action": {
          "type": "string",
          "enum": [
            "remember",
            "forget",
            "list"
          ],
          "description": "记住一条事实、按关键词删除条目，或列出当前记忆"
        },
        "content": {
          "type": "string",
          "description": "要记住的简洁事实（action=remember 时必填）"
        },
        "keyword": {
          "type": "string",
          "description": "要删除条目的子串关键词（action=forget 时必填）"
        },
        "scope": {
          "type": "string",
          "enum": [
            "project",
            "global"
          ],
          "description": "project（默认）= 仅限本仓库；global = 所有项目"
        }
      },
      "required": [
        "action"
      ]
    }
  }
}
```