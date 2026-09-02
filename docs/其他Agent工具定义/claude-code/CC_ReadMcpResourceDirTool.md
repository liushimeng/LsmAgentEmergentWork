# ReadMcpResourceDirTool 工具定义
```json
{
  "name": "ReadMcpResourceDirTool",
  "description": "
    列出 MCP 服务器上某个目录资源的直接子项（接口标识：`resources/directory/read`）。

    参数说明：
    - server（必填）：待读取数据的 MCP 服务器名称
    - uri（必填）：目标目录资源的统一资源标识符

    该工具仅做单层目录列举，不会递归遍历。返回的每一条目录项都携带独立的`uri`；子目录对应的媒体类型为「inode/directory」—— 若需进入子目录查看内容，需传入该子目录的`uri`再次调用本工具。

    仅可用于已声明支持目录列举功能的 MCP 服务器；向不支持该能力的服务器调用本工具会返回错误。
  ",
  "input\_schema": {
    "$schema": "[https://json-schema.org/draft/2020-12/schema](https://link.wtturl.cn/?target=https%3A%2F%2Fjson-schema.org%2Fdraft%2F2020-12%2Fschema&scene=im&aid=582478&lang=zh)",
    "type": "object",
    "properties": {
    "server": {
    "description": "MCP 服务器名称",
    "type": "string"
    }
    ,
    "uri": {
    "description": "需要列举内容的目录资源统一资源标识符",
    "type": "string"
    }
    }
    ,
    "required": \[
    "server",
    "uri"
    \]
    ,
    "additionalProperties": false
  }
}
```