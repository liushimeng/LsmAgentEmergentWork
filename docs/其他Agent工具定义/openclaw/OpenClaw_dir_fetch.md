# dir_fetch 工具定义
```json
{
  "name": "dir_fetch",
  "description": "从已配对的节点获取目录树（以 gzip 压缩的 tarball 形式），在网关上解包，并返回已保存路径的清单。用于在单次往返中拉取源码树、资产文件夹或日志目录。解包后的文件存放在网关上（而非您的本地机器）；请将 localPath 传递给其他工具，或对单个条目使用 file_fetch 以将它们传输到其他地方。拒绝处理压缩后大于 16 MB 的目录树。需要操作员显式启用：gateway.nodes.allowCommands 必须包含 'dir.fetch'，且 plugins.entries.file-transfer.config.nodes.<node>.allowReadPaths 必须匹配该目录路径。",
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
    "description": "节点上要获取的目录的绝对路径。在服务端进行规范化处理。"
    }
    ,
    "maxBytes": {
    "type": "integer",
    "minimum": 1,
    "description": "要获取的 gzip 压缩 tarball 的最大字节数。默认值为 8 MB，硬性上限为 16 MB（单次往返）。"
    }
    ,
    "includeDotfiles": {
    "type": "boolean",
    "description": "为 v2 版本保留；当前始终包含以点开头的文件（BSD tar 在 v1 版本中的特性）。"
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