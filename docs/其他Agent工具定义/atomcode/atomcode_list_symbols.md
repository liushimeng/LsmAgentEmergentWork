# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "list_symbols",
    "description": "列出源文件中定义的函数、类、结构体、方法及其他符号，并附带各自的代码行范围。在编辑前了解文件结构时，比 read_file 更快、更精准。支持 Rust、Python、JS/TS/TSX、Go、Java、C/C++、C#、HTML、PHP。相对路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "file_path": {
          "type": "string",
          "description": "源文件路径（绝对路径，或相对于工作目录的路径）"
        }
      },
      "required": [
        "file_path"
      ]
    }
  }
}
```