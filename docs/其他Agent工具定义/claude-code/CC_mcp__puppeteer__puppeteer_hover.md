# mcp__puppeteer__puppeteer_hover 工具定义
```json
{
  "name": "mcp__puppeteer__puppeteer_hover",
  "description": "悬停在页面上的某个元素上",
  "input_schema": {
    "type": "object",
    "properties": {
    "selector": {
    "type": "string",
    "description": "用于定位要悬停的元素的 CSS 选择器"
    }
    },
    "required": [
    "selector"
    ]
  }
}
```