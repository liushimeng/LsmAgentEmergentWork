# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "blast_radius",
    "description": "估算修改某个文件的影响范围（blast radius）：直接依赖项（深度 1）、间接依赖项（深度 2-3），以及受影响的文件总数。相对路径相对于工作目录解析。",
    "parameters": {
      "type": "object",
      "properties": {
        "file": {
          "type": "string",
          "description": "文件路径（绝对路径，或相对于工作目录的路径）"
        }
      },
      "required": [
        "file"
      ]
    }
  }
}

```
