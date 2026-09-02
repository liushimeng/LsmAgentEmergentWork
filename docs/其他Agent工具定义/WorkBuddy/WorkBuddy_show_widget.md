# show_widget 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "show_widget",
        "description": "展示可视化内容，包括SVG图形、示意图、图表或交互式HTML组件，内容将直接内嵌在文本回复中渲染。首次调用show_widget前必须先调用read_me（指定模块：diagram|mockup|interactive|chart|art）加载所需设计规范。widget_code必须为原生SVG/HTML片段（禁止包含<html>/<head>/<body>/<!DOCTYPE>标签）；若使用SVG，viewBox需以\"0 0 680 \"开头。",
        "parameters": {
            "type": "object",
            "required": [
                "title",
                "widget_code",
                "loading_messages"
            ],
            "properties": {
                "title": {
                    "type": "string",
                    "description": "该可视化组件的简短标识，使用与用户一致的语言（遵循<response_language>）。名称必须明确且具备区分度；若对话内存在多个可视化内容，仅凭标题就应当能够区分目标组件。空格与连字符会自动转换为下划线，同时该标题将用作下载文件名。"
                },
                "widget_code": {
                    "type": "string",
                    "description": "待渲染的SVG或HTML代码。SVG格式：以<svg>标签起始的原生代码。HTML格式：原生HTML片段，严禁包含文档声明、<html>、<head>、<body>标签。"
                },
                "loading_messages": {
                    "type": "string",
                    "description": "经过JSON编码的字符串数组，包含1~4条可视化渲染过程中展示的加载提示，每条提示约5个词语。使用与用户相同的语言编写。示例：'["准备图表数据","渲染可视化图形","应用样式","即将完成"]'"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```
