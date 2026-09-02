# file_fetch 工具定义
```json
{
  "name": "file_fetch",
  "description": "通过绝对路径从已配对的节点获取文件。对于图像 MIME 类型，返回图像内容块；对于小型文本文件（≤8 KB），将其作为文本内容内联返回；对于其他所有文件，将其保存在网关媒体存储中，并提供一个可传递给 file_write 或其他工具的路径。适用于获取截图、照片、收据、日志和源文件。可与 file_write 配合使用，以将文件从一个节点复制到另一个节点（无需使用 exec/cp 调用 Shell）。需要操作员显式启用：gateway.nodes.allowCommands 必须包含 'file.fetch'，且 plugins.entries.file-transfer.config.nodes.<node>.allowReadPaths 必须匹配该文件路径。若未配置相关策略，所有调用均会被拒绝。",
  "parameters": {
    "type": "object",
    "required": [
    "node",
    "path"
    ]
    ,
    "properties": {
    "node": {
    "type": "string",
    "description": "节点 ID、名称或 IP。解析方式与 nodes 工具相同。"
    }
    ,
    "path": {
    "type": "string",
    "description": "节点上文件的绝对路径。在服务端进行规范化处理。"
    }
    ,
    "maxBytes": {
    "type": "integer",
    "minimum": 1,
    "description": "要获取的最大字节数。默认值为 8 MB，硬性上限为 16 MB（单次往返）。"
    }
    ,
    "gatewayUrl": {
    "type": "string"
    }
    ,
    "gatewayToken": {
    "type": "string"
    }
    ,
    "timeoutMs": {
    "type": "integer",
    "minimum": 1
    }
    }
  }
}
```