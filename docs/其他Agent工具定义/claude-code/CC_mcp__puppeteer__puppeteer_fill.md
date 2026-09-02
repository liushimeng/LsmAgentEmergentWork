# mcp__puppeteer__puppeteer_fill 工具定义
```json
{
  "name": "mcp__puppeteer__puppeteer_evaluate",
  "description": "在浏览器控制台中执行 JavaScript 代码",
  "input_schema": {
    "type": "object",
    "properties": {
    "script": {
    "type": "string",
    "description": "需要执行的 JavaScript 代码"
    }
    },
    "required": [
    "script"
    ]
  }
}
```