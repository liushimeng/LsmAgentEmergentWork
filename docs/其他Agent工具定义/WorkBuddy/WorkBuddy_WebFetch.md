# WebFetch 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "WebFetch",
        "description": "
- 从指定网址获取网页内容，并由AI模型进行处理
- 入参包含网址与提示词
- 获取网页内容，将HTML转换为Markdown格式
- 使用轻量高速模型，根据提示词处理网页内容
- 返回模型针对网页内容生成的应答结果
- 需要获取并分析网页内容时使用本工具

使用注意事项：
  - 重要：如果存在MCP提供的网页拉取工具，请优先使用该工具，其限制通常更少。所有MCP工具名称均以\"mcp__\"开头。
  - 网址必须是格式完整有效的链接
  - HTTP协议链接会自动升级为HTTPS
  - 提示词需要写明你想要从页面提取的信息
  - 本工具为只读工具，不会修改任何文件
  - 若网页内容体量极大，返回结果可能会被摘要精简
  - 内置15分钟自动清理缓存，重复访问同一链接可加速响应
  - 当链接重定向至其他域名时，工具会进行提示，并以专用格式返回重定向地址。你需要使用该重定向地址重新发起WebFetch请求拉取内容。
",
        "parameters": {
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "需要拉取内容的网址"
                },
                "prompt": {
                    "type": "string",
                    "description": "针对拉取到的网页内容执行处理的提示词"
                }
            },
            "required": [
                "url",
                "prompt"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```