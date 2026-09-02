# web_fetch 工具定义
```json
{
  "name": "web_fetch",
  "description": "获取 URL 并提取可读的 Markdown 或纯文本。轻量级网页访问；不包含浏览器自动化功能。",
  "parameters": {
    "type": "object",
    "required": [
    "url"
    ],
    "properties": {
    "url": {
    "type": "string",
    "description": "HTTP(S) 链接。"
    },
    "extractMode": {
    "type": "string",
    "enum": [
    "markdown",
    "text"
    ],
    "description": "以 Markdown 或纯文本格式提取内容。",
    "default": "markdown"
    },
    "maxChars": {
    "type": "integer",
    "description": "返回的最大字符数；超出部分将被截断。",
    "minimum": 100
    }
    }
  }
}
```