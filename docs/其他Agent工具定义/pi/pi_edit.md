# edit 工具定义
```json
{
  "name": "edit",
  "description": "使用精确文本替换来编辑单个文件。每个 edits[].oldText 必须匹配原始文件中唯一且不重叠的区域。如果两处更改影响同一个代码块或相邻的行，请将它们合并为一个 edit，而不是发出重叠的编辑操作。不要仅仅为了连接相距较远的更改而包含大段未修改的区域。",
  "parameters": {
    "type": "object",
    "required": [
    "path",
    "edits"
    ],
    "properties": {
    "path": {
    "type": "string",
    "description": "要编辑的文件路径（相对路径或绝对路径）"
    },
    "edits": {
    "type": "array",
    "items": {
      "type": "object",
      "required": [
      "oldText",
      "newText"
      ],
      "properties": {
      "oldText": {
      "type": "string",
      "description": "用于单次目标替换的精确文本。它在原始文件中必须是唯一的，且不得与同一次调用中的任何其他 edits[].oldText 重叠。"
      },
      "newText": {
      "type": "string",
      "description": "本次目标编辑的替换文本。"
      }
      },
      "additionalProperties": false
    },
    "description": "一个或多个目标替换操作。每次编辑都是与原始文件进行匹配，而非增量匹配。请勿包含重叠或嵌套的编辑操作。如果两处更改涉及相同的代码块或相邻的行，请将它们合并为一个 edit。"
    }
    },
    "additionalProperties": false
  },
  "strict": false
}
```