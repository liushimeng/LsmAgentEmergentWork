# WaitForMcpServers 工具定义
```json
{
  "name": "WaitForMcpServers",
  "description": "等待仍在连接中且其工具尚未出现在你的工具列表中的 MCP 服务器。传入 `servers` 以等待特定的服务器，或者省略该参数以等待所有待处理的服务器。

  如果用户的请求需要使用仍处于连接状态的服务器中的工具，请调用此工具进行等待。一旦连接成功，其工具将被添加到你的工具列表中，你可以直接使用它们。当服务器准备就绪时返回 ready=true，如果连接失败、需要身份验证或被禁用则返回 ready=false。

  你无需询问用户确认即可使用此工具。",
  "input_schema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
      "servers": {
        "description": "要等待的服务器名称（默认：所有待处理的服务器）",
        "type": "array",
        "items": {
        "type": "string"
        }
      }
    },
    "additionalProperties": false
  }
}
```