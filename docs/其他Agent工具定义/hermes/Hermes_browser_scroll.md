# browser_scroll 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "browser_scroll",
    "description": "在指定方向上滚动页面。当需要显示当前视口上方或下方的更多内容时，请使用此功能。调用前需先执行 browser_navigate。",
    "parameters": {
      "type": "object",
      "properties": {
      "direction": {
      "type": "string",
      "enum": [
      "up",
      "down"
      ]
      ,
      "description": "滚动的方向"
      }
      }
      ,
      "required": [
      "direction"
      ]
    }
  }
}
```