# browser_navigate 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "browser_navigate",
        "description": " 在浏览器中访问指定网址，初始化会话并加载页面。调用其他浏览器相关工具前必须先执行本工具。针对纯文本接口地址（后缀为 .md、.txt、.json、.yaml、.yml、.csv、.xml 的链接、raw.githubusercontent.com 地址以及各类公开 API 接口），建议优先使用终端 curl 工具或 web_extract 工具获取内容；使用浏览器工具处理这类资源性能开销大且速度缓慢。当需要与页面交互（点击、填写表单、处理动态渲染内容）时再选用浏览器工具。执行后会返回精简页面快照，包含可交互元素与元素引用 ID，跳转页面后无需单独调用 browser_snapshot 工具。",
        "parameters": {
            "type": "object",
            "properties": {
            "url": {
                "type": "string",
                "description": " 需要访问的网址（示例：'https://example.com'）"
            }
            },
            "required": [ "url" ]
        }
    }
}
```