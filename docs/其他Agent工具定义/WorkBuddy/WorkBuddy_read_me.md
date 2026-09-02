# read_me 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "read_me",
        "description": "返回 show_widget 所需的上下文信息（CSS变量、色彩、排版、布局规则、示例）。在首次调用 show_widget 之前执行调用。后续如果需要加载其他模块，可以再次调用。请勿向用户提及或描述本次调用，这属于内部初始化步骤。",
        "parameters": {
            "type": "object",
            "required": [
                "modules"
            ],
            "properties": {
                "modules": {
                    "anyOf": [
                        {
                            "type": "array",
                            "items": {
                                "type": "string"
                            }
                        },
                        {
                            "type": "string"
                        }
                    ],
                    "description": "需要加载的模块，请选择所有适用模块。可选模块：diagram（图表）、mockup（原型）、interactive（交互组件）、art（艺术绘图）、chart（数据图表）。传入形式支持数组（例如 [\"diagram\",\"chart\"]）、JSON字符串数组，或者英文逗号分隔字符串，如 \"diagram,chart\"。"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```