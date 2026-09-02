# mcp__puppeteer__puppeteer_select 定义工具
```json
{
  "name": "mcp__puppeteer__puppeteer_select",
  "description": "使用 Select 标签选择页面上的某个元素",
  "input_schema": {
    "type": "object",
    "properties": {
    "selector": {
    "type": "string",
    "description": "用于定位要选择的元素的 CSS 选择器"
    }
    ,
    "value": {
    "type": "string",
    "description": "需要选择的值"
    }
    }
    ,
    "required": [
    "selector",
    "value"
    ]
  }
}
```