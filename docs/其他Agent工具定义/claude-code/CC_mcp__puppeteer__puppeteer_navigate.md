# mcp__puppeteer__puppeteer_navigate 工具定义
```json
{
  "name": "mcp__puppeteer__puppeteer_navigate",
  "description": "导航到指定 URL",
  "input_schema": {
    "type": "object",
    "properties": {
    "url": {
    "type": "string",
    "description": "要导航到的 URL 地址"
    }
    ,
    "launchOptions": {
    "type": "object",
    "description": "PuppeteerJS 启动选项。默认为 null。如果修改且不为 null，浏览器将重启。示例: { headless: true, args: ['--no-sandbox'] }"
    }
    ,
    "allowDangerous": {
    "type": "boolean",
    "description": "是否允许降低安全性的危险启动选项。当设置为 false 时，像 --no-sandbox 这样的危险参数会抛出错误。默认为 false。"
    }
    }
    ,
    "required": [
    "url"
    ]
  }
}
```