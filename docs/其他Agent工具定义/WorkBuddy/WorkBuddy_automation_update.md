# automation_update 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "automation_update",
        "description": "用于列出、查看、创建、更新或删除定时自动化任务。使用模式“list”列出全部自动化任务（仅摘要信息）；“view”根据ID获取单个自动化任务完整可编辑配置；“create”新建自动化任务（需要填写完整字段）；“update”修改已有任务（仅id为必填项；未指定字段将保留原有值）；“delete”根据ID删除自动化任务。删除操作立即生效，属于软删除；该条目将在列表/查看界面隐藏，但仍保存在存储中，可通过后台工具恢复。",
        "parameters": {
            "type": "object",
            "required": [
                "mode"
            ],
            "properties": {
                "mode": {
                    "type": "string",
                    "description": "操作模式：可选值 \"list\" | \"view\" | \"create\" | \"update\" | \"delete\""
                },
                "id": {
                    "type": "string",
                    "description": "自动化任务ID（view、update、delete模式必填；list、create模式无需填写）"
                },
                "name": {
                    "type": "string",
                    "description": "便于人类识别的自动化任务名称"
                },
                "prompt": {
                    "type": "string",
                    "description": "自动化任务运行时执行的提示词/指令"
                },
                "scheduleType": {
                    "type": "string",
                    "description": "\"once\" 代表一次性执行，\"recurring\" 代表重复循环执行"
                },
                "rrule": {
                    "type": "string",
                    "description": "遵循RFC 5545标准的循环调度RRULE字符串（scheduleType为\"recurring\"时必填）。支持按天、小时、周、月、年循环（DAILY, HOURLY, WEEKLY, MONTHLY, YEARLY）。"
                },
                "scheduledAt": {
                    "type": "string",
                    "description": "一次性任务执行时间，ISO 8601 格式日期时间（scheduleType为\"once\"时必填）"
                },
                "cwds": {
                    "type": "string",
                    "description": "自动化任务使用的工作目录，多个目录使用英文逗号分隔。不允许使用Claw（本地助手）工作空间，填写后会被拒绝。"
                },
                "status": {
                    "type": "string",
                    "description": "可选值 \"ACTIVE\"（启用）或 \"PAUSED\"（暂停）"
                },
                "validFrom": {
                    "type": "string",
                    "description": "调度生效起始时间，ISO 8601 日期/时间格式（选填）"
                },
                "validUntil": {
                    "type": "string",
                    "description": "调度失效截止时间，ISO 8601 日期/时间格式（选填）"
                },
                "modelId": {
                    "type": "string",
                    "description": "自动化任务运行时所使用的模型ID"
                },
                "modelIsThinking": {
                    "type": "boolean",
                    "description": "选定模型是否开启思考/推理模式运行"
                },
                "expertId": {
                    "type": "string",
                    "description": "自动化执行期间启用的专家ID或名称。系统会尽可能解析为对应的内部唯一ID。"
                },
                "expertMarketplace": {
                    "type": "string",
                    "description": "用于定位专家来源的专家市场标识"
                },
                "connectorIds": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "自动化任务运行期间激活的MCP连接器配置ID列表"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```