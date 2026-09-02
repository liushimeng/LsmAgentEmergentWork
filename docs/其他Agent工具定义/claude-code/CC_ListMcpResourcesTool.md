# ListMcpResourcesTool 工具定义
```json
{
  "name": "ListMcpResourcesTool",
  "description": "
  从配置的 MCP 服务器列出可用资源。
  每个返回的资源将包含所有标准 MCP 资源字段，以及一个指示资源所属服务器的 'server' 字段。

  参数：
  - server (可选): 要从中获取资源的特定 MCP 服务器的名称。如果未提供，
    则返回所有服务器的资源。
  ",
  "input_schema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "server": {
    "description": "用于筛选资源的可选服务器名称",
    "type": "string"
    }
    },
    "additionalProperties": false
  }
}
```