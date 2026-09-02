# write_file 工具定义
```json
{
"type": "function",
"function": {
"name": "write_file",
"description": "向文件写入内容，会完整覆盖原有文件内容，可替代终端中的 echo、cat 多行文本语法。会自动创建文件上级目录。本工具会完整重写整个文件，精准局部修改请使用 patch 工具。会自动对 .py/.json/.yaml/.toml 等支持语法校验的文件执行语法检查，仅展示本次写入操作新增的错误，原有历史报错会过滤屏蔽。",
"parameters": {
"type": "object",
"properties": {
"path": {
"type": "string",
"description": "待写入文件路径；文件不存在则新建，已存在则覆盖重写"
},
"content": {
"type": "string",
"description": "完整待写入文件文本内容"
},
"cross_profile": {
"type": "boolean",
"description": "关闭跨配置文件安全防护限制，默认 false。仅在用户明确指示修改其他 Hermes 配置的技能、插件、定时任务、记忆文件时才可设为 true；默认情况下这类跨配置写入操作会拦截并弹出警告，因为会影响当前会话以外的其他配置环境。",
"default": false
}
},
"required": [
"path",
"content"
]
}
}
}
```