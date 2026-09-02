# canvas 工具定义
```json
{
  "name": "canvas",
  "description": "控制节点画布（present/hide/navigate/eval/snapshot/A2UI）。使用 snapshot 来捕获渲染后的用户界面。",
  "parameters": {
    "type": "object",
    "required": [
    "action"
    ]
    ,
    "properties": {
    "action": {
    "type": "string",
    "enum": [
    "present",
    "hide",
    "navigate",
    "eval",
    "snapshot",
    "a2ui_push",
    "a2ui_reset"
    ]
    }
    ,
    "gatewayUrl": {
    "type": "string"
    }
    ,
    "gatewayToken": {
    "type": "string"
    }
    ,
    "timeoutMs": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "node": {
    "type": "string"
    }
    ,
    "target": {
    "type": "string"
    }
    ,
    "x": {
    "type": "number"
    }
    ,
    "y": {
    "type": "number"
    }
    ,
    "width": {
    "type": "number"
    }
    ,
    "height": {
    "type": "number"
    }
    ,
    "url": {
    "type": "string"
    }
    ,
    "javaScript": {
    "type": "string"
    }
    ,
    "outputFormat": {
    "type": "string",
    "enum": [
    "png",
    "jpg",
    "jpeg"
    ]
    }
    ,
    "maxWidth": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "quality": {
    "type": "number",
    "minimum": 0,
    "maximum": 1
    }
    ,
    "delayMs": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "jsonl": {
    "type": "string"
    }
    ,
    "jsonlPath": {
    "type": "string"
    }
    }
  }
}
```