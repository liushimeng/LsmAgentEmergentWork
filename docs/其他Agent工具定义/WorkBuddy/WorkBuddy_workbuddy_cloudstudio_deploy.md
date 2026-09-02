# workbuddy_cloudstudio_deploy 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "workbuddy_cloudstudio_deploy",
        "description": "将本地静态站点目录部署至 CloudStudio 沙箱工作空间。创建新工作空间、上传文件、启动静态文件服务器，并返回访问链接。当用户要求在云端部署、发布或预览 Web 应用、静态网站时使用本工具。目录内需要包含编译构建后的产物（例如 dist/、build/、out/）且存在 index.html。部署成功后，在最终总结回复（使用与对话一致的语言）中**必须告知用户可以在设置菜单中管理已发布应用（例如删除应用）**，并且使用加粗 Markdown 格式（前后包裹**）展示对应语言的菜单路径：中文：「设置 - 数据管理 - 我发布的应用」，英文："Settings - Data Management - Published Apps"。",
        "parameters": {
            "type": "object",
            "required": [
                "directory"
            ],
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "待部署本地目录的绝对路径。该目录应当是包含 index.html 的构建产物目录（示例：/path/to/project/dist）。"
                },
                "port": {
                    "type": "number",
                    "description": "沙箱内部静态文件服务器占用端口，默认值3000。"
                },
                "entry": {
                    "type": "string",
                    "description": "入口HTML文件名（例如 "index.html"、"review.html"）。留空时自动识别。"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```