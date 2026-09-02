# webfetch 工具定义
```json
{
  "name": "webfetch",
  "description": "- 从指定的 URL 获取内容
  - 接受 URL 和可选的格式作为输入
  - 获取 URL 内容，并转换为请求的格式（默认为 markdown）
  - 以指定的格式返回内容
  - 当您需要检索和分析网页内容时，请使用此工具

  使用说明：
    - 重要提示：如果存在其他提供更好网页获取功能、更针对特定任务或限制更少的工具，请优先使用该工具而不是此工具。
    - URL 必须是格式完整的有效 URL
    - HTTP URL 将自动升级为 HTTPS
    - 格式选项：\"markdown\"（默认）、\"text\" 或 \"html\"
    - 这个工具是只读的，不会修改任何文件
    - 如果内容非常大，结果可能会被摘要",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "url": {
    "type": "string",
    "description": "要获取内容的 URL"
    },
    "format": {
    "type": "string",
    "enum": [
    "text",
    "markdown",
    "html"
    ],
    "description": "返回内容的格式（text、markdown 或 html）。默认为 markdown。",
    "default": "markdown"
    },
    "timeout": {
    "type": "number",
    "description": "可选的超时时间，单位为秒（最大 120）"
    }
    },
    "required": [
    "url"
    ]
  }
}
```