# apply_patch 工具定义
```json
{
  "name": "apply_patch",
  "description": "使用 apply_patch 格式对一个或多个文件应用补丁。输入内容必须包含 *** Begin Patch 和 *** End Patch 标记。",
  "parameters": {
    "type": "object",
    "required": [
    "input"
    ]
    ,
    "properties": {
    "input": {
    "type": "string",
    "description": "使用 *** Begin Patch/End Patch 格式的补丁内容。"
    }
    }
  }
}
```