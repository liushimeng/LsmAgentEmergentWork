# WebFetch 工具定义
```json
{
"name": "WebFetch",
"description": "获取一个 URL，将页面转换为 markdown 格式，并使用一个小型快速模型针对其回答 `prompt`。\n\n- 在经过身份验证/私有的 URL 上会失败 —— 请改用经过身份验证的 MCP 工具或 `gh`。\n- HTTP 会被升级为 HTTPS。跨主机的重定向会返回给您而不是自动跟随；请使用重定向 URL 再次调用。\n- 响应会按 URL 缓存 15 分钟。",
"input_schema": {
"$schema": "https://json-schema.org/draft/2020-12/schema",
"type": "object",
"properties": {
"url": {
"description": "要获取内容的 URL",
"type": "string",
"format": "uri"
},
"prompt": {
"description": "要在获取的内容上运行的提示",
"type": "string"
}
},
"required": [
"url",
"prompt"
],
"additionalProperties": false
}
}
```