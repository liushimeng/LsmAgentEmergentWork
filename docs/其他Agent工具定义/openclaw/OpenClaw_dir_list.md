# dir_list 工具定义
```json
{
  "name": "dir_list",
  "description": "从已配对的节点获取结构化的目录列表。返回文件和子目录的元数据（名称、路径、大小、MIME类型、是否为目录、修改时间），不传输文件内容。用于在使用 file_fetch 获取文件之前探查存在哪些文件。分页基于偏移量；请传入上一次结果中的 nextPageToken。需要操作员显式启用：gateway.nodes.allowCommands 必须包含 'dir.list'，且 plugins.entries.file-transfer.config.nodes.<node>.allowReadPaths 必须匹配该目录路径。若未配置相关策略，所有调用均会被拒绝。",
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
      "description": "节点上目录的绝对路径。在服务端进行规范化处理。"
      }
      ,
      "pageToken": {
      "type": "string",
      "description": "上一次 dir_list 调用返回的分页令牌。省略此参数将从头开始获取。"
      }
      ,
      "maxEntries": {
      "type": "integer",
      "minimum": 1,
      "description": "每页的最大条目数。默认值为 200，硬性上限为 5000。"
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