# AskUserQuestion 工具定义
```json
{
    "name": "AskUserQuestion",
    "description": "仅在遇到**必须由用户决策**、且无法通过需求文档、代码内容或通用默认方案自行判断时，使用本工具。

    使用说明：
    - 用户始终可以选择「其他」选项来自定义输入内容
    - 将 multiSelect 设为 true，可支持一题多选
    - 若需要推荐某个选项，请将其放在选项列表首位，并在选项名称末尾标注「（推荐）」

    规划模式说明：如需进入规划模式，请使用进入规划模式工具，而非本工具。进入规划模式后，可在最终确定方案前，使用本工具确认需求或挑选实现方案。**禁止**用本工具询问「方案是否就绪」「是否继续执行」等和方案本身相关的问题——用户在你调用退出规划模式并发起确认前，无法查看方案内容。

    本工具仅用于**用户的选择会直接影响后续执行流程**的场景。若存在通用默认选项，或相关信息可在代码库中自行核查，无需向用户提问：直接选用合理方案、在回复中说明并继续执行即可。

    预览功能：
    当需要展示实体内容供用户直观对比时，可在选项中使用可选字段 preview，适用场景包括：
    - UI 布局/组件的 ASCII 原型图
    - 不同实现方式的代码片段
    - 各类示意图版本
    - 配置示例

    预览内容会以等宽字体的 Markdown 代码块形式展示，支持带换行的多行文本。只要任意选项配置了预览内容，界面就会变为左右布局：左侧竖向展示选项列表，右侧展示对应预览内容。仅单纯偏好选择、依靠名称和描述即可区分的场景，请勿使用预览功能。注意：预览功能**仅支持单选问题，不支持多选**。
    ",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "questions": {
                "description": "向用户提出的问题（数量 1-4 个）",
                "minItems": 1,
                "maxItems": 4,
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "question": {
                            "description": "完整的问题内容，表述清晰明确，末尾需加问号。示例：“我们该使用哪个库来处理日期格式化？”。若开启多选，请对应调整句式，例如：“你需要启用哪些功能？”",
                            "type": "string"
                        },
                        "header": {
                            "description": "简短标签，以角标/标签形式展示（最多12个字符）。示例：“认证方式”、“类库”、“实现方案”。",
                            "type": "string"
                        },
                        "options": {
                            "description": "该问题的可选答案，数量 2-4 个。默认情况下各选项互斥（开启多选除外）。无需手动添加「其他」选项，该选项会自动生成。",
                            "minItems": 2,
                            "maxItems": 4,
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": {
                                        "description": "选项展示名称，供用户查看和选择，简洁明了（1-5个词语），清晰表达选项含义。",
                                        "type": "string"
                                    },
                                    "description": {
                                        "description": "选项说明，解释该选项的含义、选择后产生的结果，用于补充说明利弊与影响。",
                                        "type": "string"
                                    },
                                    "preview": {
                                        "description": "可选预览内容，选中该选项时展示。适用于原型图、代码片段、对比视图等辅助用户选择的内容，格式要求详见工具说明。",
                                        "type": "string"
                                    }
                                },
                                "required": [
                                    "label",
                                    "description"
                                ],
                                "additionalProperties": false
                            }
                        },
                        "multiSelect": {
                            "description": "设为 true 允许多选，适用于选项互不排斥的场景。默认值：false",
                            "default": false,
                            "type": "boolean"
                        }
                    },
                    "required": [
                        "question",
                        "header",
                        "options",
                        "multiSelect"
                    ],
                    "additionalProperties": false
                }
            },
            "answers": {
                "description": "交互组件收集到的用户答案",
                "type": "object",
                "propertyNames": {
                    "type": "string"
                },
                "additionalProperties": {
                    "type": "string"
                }
            },
            "annotations": {
                "description": "用户针对单个问题补充的备注信息（例如针对预览选项的说明），以问题文本作为键名。",
                "type": "object",
                "propertyNames": {
                    "type": "string"
                },
                "additionalProperties": {
                    "type": "object",
                    "properties": {
                        "preview": {
                            "description": "若问题使用了预览功能，此字段为所选选项的预览内容。",
                            "type": "string"
                        },
                        "notes": {
                            "description": "用户针对所选内容填写的自定义备注文本。",
                            "type": "string"
                        }
                    },
                    "additionalProperties": false
                }
            },
            "metadata": {
                "description": "可选元数据，用于数据追踪与统计分析，不会向用户展示。",
                "type": "object",
                "properties": {
                    "source": {
                        "description": "可选标识，用于标记问题来源（例如 /remember 指令标记为 remember），用于统计追踪。",
                        "type": "string"
                    }
                },
                "additionalProperties": false
            }
        },
        "required": [
            "questions"
        ],
        "additionalProperties": false
    }
}
```