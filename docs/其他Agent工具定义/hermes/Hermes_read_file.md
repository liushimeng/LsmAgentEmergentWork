# read_file 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "read_file",
    "description": "按行号分页读取文本文件，可替代终端中的 cat/head/tail 命令。输出格式：' 行号 | 文本内容 '。若文件不存在，会推荐名称相近的文件。读取大文件请搭配 offset、limit 参数控制范围。单次读取字符总量超过约 10 万会被拒绝，需分段读取。支持自动提取 Jupyter 笔记 (.ipynb)、Word 文档 (.docx)、Excel 表格 (.xlsx) 内的可读文本。注意：无法读取图片及其他二进制文件，图片请使用 vision_analyze 工具解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "path": {
        "type": "string",
        "description": "待读取文件路径，支持绝对路径、相对路径、~/ 开头路径"
        },
        "offset": {
        "type": "integer",
        "description": "读取起始行号，行号从 1 开始计数，默认值：1",
        "default": 1,
        "minimum": 1
        },
        "limit": {
        "type": "integer",
        "description": "最多读取行数，默认 500 行，上限 2000 行",
        "default": 500,
        "maximum": 2000
        }
      },
      "required": [
      "path"
      ]
    }
  }
}
```