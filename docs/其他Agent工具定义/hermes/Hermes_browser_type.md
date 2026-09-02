# browser_type 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "browser_type",
    "description": "向引用 ID 对应的输入框输入文本。会先清空输入框原有内容，再录入新文本。需先调用 browser_navigate 和 browser_snapshot。",
    "parameters": {
      "type": "object",
      "properties": {
        "ref": {
        "type": "string",
        "description": "快照中获取的元素引用标识（例如：'@e3'）"
        },
        "text": {
        "type": "string",
        "description": "需要输入到输入框的文本内容"
        }
      },
      "required": [ "ref", "text" ]
    }
  }
}
```