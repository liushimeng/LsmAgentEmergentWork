# mcp__puppeteer__puppeteer_screenshot 工具定义
```json
{
  "name": "mcp__puppeteer__puppeteer_screenshot",
  "description": "对当前页面或特定元素进行截图",
  "input_schema": {
    "type": "object",
    "properties": {
    "name": {
    "type": "string",
    "description": "截图的名称"
    }
    ,
    "selector": {
    "type": "string",
    "description": "用于定位要截图的元素的 CSS 选择器"
    }
    ,
    "width": {
    "type": "number",
    "description": "宽度（像素），默认为 800"
    }
    ,
    "height": {
    "type": "number",
    "description": "高度（像素），默认为 600"
    }
    ,
    "encoded": {
    "type": "boolean",
    "description": "如果为 true，则将截图捕获为 base64 编码的数据 URI（文本形式），而不是二进制图像内容。默认为 false。"
    }
    }
    ,
    "required": [
    "name"
    ]
  }
}
```