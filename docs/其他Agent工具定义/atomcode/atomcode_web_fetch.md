# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "web_fetch",
    "description": "通过 http(s) 抓取网页并返回其内容（HTML 会转换为干净的纯文本；使用 `format:\"markdown\"` 可转换为 Markdown 以保留标题/链接/代码）。当用户提供具体的 http(s) URL 要求读取、提取或总结时使用本工具；优先于通过 shell 运行 `curl` 或 `wget`，因为它能处理页面字符集和渲染问题。在 `web_search` 之后读取某个具体结果时也应使用本工具。如果有更具体的专用技能（见系统提示中的 AVAILABLE SKILLS）与要抓取页面的 URL 或域名匹配（例如特定平台的问题跟踪器或文档站点），则不要调用本工具，而必须改用 use_skill 工具。仅允许 http/https URL；对 localhost / 内网私有地址 / 云元数据地址的请求会被阻止。默认返回完整页面；可传入 `max_chars` 限制长度。",
    "parameters": {
      "type": "object",
      "properties": {
        "url": {
          "type": "string",
          "description": "要抓取的 http(s) URL"
        },
        "max_chars": {
          "type": "integer",
          "description": "可选的返回字符数上限。省略则返回完整页面（代码/文档推荐）。"
        },
        "format": {
          "type": "string",
          "enum": [
            "text",
            "markdown"
          ],
          "description": "HTML 渲染方式：'text'（默认，纯文本）或 'markdown'（保留标题/链接/列表/代码）。非 HTML 内容会忽略此参数。"
        }
      },
      "required": [
        "url"
      ]
    }
  }
}
```