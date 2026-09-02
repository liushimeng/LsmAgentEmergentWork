# file_write 工具定义
```json
{
  "name": "file_write",
  "description": "通过绝对路径将文件字节写入已配对的节点。采用原子写入方式（临时文件 + 重命名）。默认拒绝覆盖现有文件——请传递 overwrite=true 以进行替换。除非策略明确允许跟踪符号链接，否则拒绝通过符号链接目标进行写入。可与 file_fetch 配合使用，将其返回的 mediaId 作为 sourceMediaId 传递，以实现二进制文件的复制。需要操作员显式启用：gateway.nodes.allowCommands 必须包含 'file.write'，且 plugins.entries.file-transfer.config.nodes.<node>.allowWritePaths 必须匹配目标路径。若未配置相关策略，所有调用均会被拒绝。",
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
    "description": "要写入文件的节点 ID 或显示名称。"
    }
    ,
    "path": {
    "type": "string",
    "description": "节点上要写入的绝对路径。在服务端进行规范化处理。"
    }
    ,
    "contentBase64": {
    "type": "string",
    "description": "要写入的 Base64 编码字节。解码后的最大容量为 16 MB。"
    }
    ,
    "sourceMediaId": {
    "type": "string",
    "description": "由 file_fetch 返回的媒体 ID。在进行二进制复制时优先使用此参数，因为字节数据将保留在网关媒体存储中。"
    }
    ,
    "mimeType": {
    "type": "string",
    "description": "内容类型提示。不会根据实际内容进行验证。"
    }
    ,
    "overwrite": {
    "type": "boolean",
    "description": "允许覆盖现有文件。默认为 false。",
    "default": false
    }
    ,
    "createParents": {
    "type": "boolean",
    "description": "创建缺失的父级目录（等同于 mkdir -p）。默认为 false。",
    "default": false
    }
    }
  }
}
```