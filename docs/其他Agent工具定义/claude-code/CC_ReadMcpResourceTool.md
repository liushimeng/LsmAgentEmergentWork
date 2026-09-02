# ReadMcpResourceTool 工具定义
```json
{
  "name": "ReadMcpResourceTool",
  "description": "
  从 MCP 服务器读取特定资源，通过服务器名称和资源 URI 进行标识。

  参数：
  - server (必填): 要从中读取资源的 MCP 服务器名称
  - uri (必填): 要读取的资源 URI
  ",
  "input_schema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "server": {
    "description": "MCP 服务器名称",
    "type": "string"
    },
    "uri": {
    "description": "要读取的资源 URI",
    "type": "string"
    }
    },
    "required": [
    "server",
    "uri"
    ],
    "additionalProperties": false
  }
}
```