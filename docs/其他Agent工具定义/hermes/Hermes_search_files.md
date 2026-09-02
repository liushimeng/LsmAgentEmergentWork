# search_files 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "search_files",
    "description": "搜索文件内容或按文件名查找文件。在终端中请使用此功能代替 grep/rg/find/ls。基于 Ripgrep 实现，速度比 Shell 等效命令更快。

    内容搜索（target='content'）：在文件内部进行正则表达式搜索。输出模式包括：带行号的完整匹配项、仅文件路径，或匹配数量。

    文件搜索（target='files'）：通过通配符模式（glob pattern）查找文件（例如 '.py'、'config'）。同样可以代替 ls 使用 —— 结果会按修改时间排序。",
    "parameters": {
    "type": "object",
    "properties": {
      "pattern": {
      "type": "string",
      "description": "内容搜索的正则表达式模式，或文件搜索的通配符模式（例如 '.py'）"
      }
      ,
      "target": {
      "type": "string",
      "enum": [
      "content",
      "files"
      ]
      ,
      "description": "'content' 搜索文件内容，'files' 按文件名搜索文件",
      "default": "content"
      }
      ,
      "path": {
      "type": "string",
      "description": "要搜索的目录或文件（默认值：当前工作目录）",
      "default": "."
      }
      ,
      "file_glob": {
      "type": "string",
      "description": "在 grep 模式下按模式过滤文件（例如 '*.py' 表示仅搜索 Python 文件）"
      }
      ,
      "limit": {
      "type": "integer",
      "description": "返回的最大结果数（默认值：50）",
      "default": 50
      }
      ,
      "offset": {
      "type": "integer",
      "description": "跳过前 N 个结果以实现分页（默认值：0）",
      "default": 0
      }
      ,
      "output_mode": {
      "type": "string",
      "enum": [
      "content",
      "files_only",
      "count"
      ]
      ,
      "description": "grep 模式的输出格式：'content' 显示带行号的匹配行，'files_only' 仅列出文件路径，'count' 显示每个文件的匹配数量",
      "default": "content"
      }
      ,
      "context": {
      "type": "integer",
      "description": "每个匹配项前后显示的上下文行数（仅限 grep 模式）",
      "default": 0
      }
    },
    "required": [ "pattern" ]
    }
  }
}
```