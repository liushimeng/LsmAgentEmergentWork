# mcp__puppeteer__puppeteer_click 工具定义
```json
{
  "name": "mcp__puppeteer__puppeteer_click",
  "description": "点击页面上的某个元素",
  "input_schema": {
    "type": "object",
    "properties": {
    "selector": {
    "type": "string",
    "description": "用于定位要点击的元素的 CSS 选择器"
    }
    },
    "required": [
    "selector"
    ]
  }
}
```