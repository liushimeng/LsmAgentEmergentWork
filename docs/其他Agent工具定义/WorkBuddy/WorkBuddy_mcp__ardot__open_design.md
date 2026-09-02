# mcp__ardot__open_design 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__open_design",
        "description": "使用该工具（open_design）在交互式编辑器界面中打开已存在的Ardot设计文件。当用户要求打开、查看、载入、编辑或处理Ardot设计、文档、草稿或文件时调用此工具。请提供 fileUrl（完整Ardot链接，例如 https://ardot.tencent.com/file/<id>?node_id=<nid>）或 fileId（数字ID）。若两个参数同时传入，fileUrl优先级更高，并且会完整保留其查询参数（包含node_id）。

该工具为异步执行——它仅触发加载流程，文档需要数秒时间完成载入。工具返回结果后，请轮询调用 fetch_file_info 检查文档是否就绪。如果返回 NO_ADAPTER，等待3秒后重试，最多重试3次。连续重试3次仍然失败则停止操作并告知用户，不要尝试通过其他替代方案继续执行设计相关任务。仅当 fetch_file_info 调用成功后，才能执行后续操作。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [],
            "properties": {
                "fileUrl": {
                    "description": "Ardot完整文件链接（例如 https://ardot.tencent.com/file/667788990055443?node_id=1:2）。优先使用该参数；传入后，全部查询参数（包括node_id）会转发至编辑器。",
                    "type": "string"
                },
                "fileId": {
                    "description": "Ardot数字文件ID（例如 667788990055443）。未提供fileUrl时使用此参数。",
                    "type": "string"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```