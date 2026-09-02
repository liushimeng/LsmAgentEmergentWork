# present_files 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "present_files",
        "description": "向用户展示文件与预览内容，作为任务的最终结果。这是展示结果的统一入口。生成用户需要查看、检查或下载的文件后调用本工具。将所有相关内容一并传入；传入顺序代表展示优先级——将用户优先查看的内容放在最前方。第一项内容会自动打开并获得焦点。每一项内容可以是本地绝对文件路径或者 http/https 链接：
- 本地文件（图片、报告、PPTX、视频、代码等）以成果卡片形式展示。
- 本地HTML文件（.html/.htm）会同时在实时预览面板打开，并且列入成果卡片列表。
- http/https 链接（包含你启动的本地开发服务地址）将在内置浏览器预览面板打开。访问本地地址前需要先启动服务（使用Bash工具）；工具会检测地址是否可访问，若服务未运行会返回相关指引。",
        "parameters": {
            "type": "object",
            "required": [
                "files"
            ],
            "properties": {
                "files": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "待展示内容数组，按照展示优先级从高到低排序。每一项为本地文件绝对路径（示例：/Users/foo/report.pdf、/Users/foo/index.html）或 http/https 链接（示例：你启动的本地服务地址 http://localhost:8080，或是远程网页地址）。"
                },
                "cwd": {
                    "type": "string",
                    "description": "当前工作目录（绝对路径）。建议填写，桌面客户端可通过该路径将预览归属至对应会话；访问本地服务链接时该项为必填项，当服务无法访问时，工具可根据该路径查找本地文件作为备选方案。"
                },
                "explanation": {
                    "type": "string",
                    "description": "简短说明生成了哪些内容、调用本工具的原因。"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```