# mcp__ardot__batch_read 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__batch_read",
        "description": "通过匹配检索条件以及节点ID批量读取节点信息。

- 重要提示：将多条检索条件与节点ID查询合并至单次调用。
- 重要提示：若仅需要读取部分指定属性，请在properties数组中声明。
- 当patterns与nodeIds均未填写时，接口返回文档顶层子节点。
- 使用searchDepth设置向下检索的层级深度。
- 当读取包含超过500个子节点的节点时，子节点信息会进行压缩，仅保留ID、名称、类型以及压缩后的子节点信息。
- 操作设计系统组件时，禁止逐个读取组件，请在一次工具调用中传入全部目标组件。

大型文档遍历方案：
1. 优先读取顶层节点或已知ID的节点，梳理整体文档结构
2. 判断当前获取的节点信息是否满足任务需求
3. 若需要更多数据，且子节点显示为省略「...」，使用对应子节点ID发起新一轮读取调用",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                },
                "nodeIds": {
                    "description": "待读取的节点ID数组。示例：nodeIds: ['nodeId1', 'nodeId2', ...]",
                    "type": "array",
                    "items": {
                        "type": "string"
                    }
                },
                "patterns": {
                    "description": "检索匹配节点的规则对象数组",
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "description": "用于匹配节点名称的正则表达式",
                                "type": "string"
                            },
                            "reusable": {
                                "description": "根据可复用属性进行筛选",
                                "type": "boolean"
                            },
                            "type": {
                                "description": "根据节点类型筛选",
                                "type": "string"
                            }
                        }
                    }
                },
                "parentId": {
                    "description": "限定检索范围的父节点ID",
                    "type": "string"
                },
                "readDepth": {
                    "description": "返回结果中携带的子节点层级数量（默认值：2）",
                    "type": "number"
                },
                "searchDepth": {
                    "description": "按照检索规则向下搜索的最大层级深度",
                    "type": "number"
                },
                "properties": {
                    "description": "需要在返回结果中携带的指定属性数组。配置后仅返回列表内的属性。示例：[\"fills\", \"strokes\", \"effects\", \"constraints\"]",
                    "type": "array",
                    "items": {
                        "type": "string"
                    }
                },
                "resolveInstances": {
                    "description": "解析实例对应的主组件节点数据，获取实例完整源结构。如需读取实例内部子节点，请将此项设为true。",
                    "type": "boolean"
                },
                "resolveVariables": {
                    "description": "解析变量引用，获取变量计算后的实际值",
                    "type": "boolean"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```