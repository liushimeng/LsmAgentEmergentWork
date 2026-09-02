# mcp__ardot__fetch_editor_state 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__fetch_editor_state",
        "description": "获取当前激活的画布编辑器、用户当前选中内容以及其他核心设计信息，用于启动设计任务。

  - 重要提示：如果已经加载「ardot-design-assistant」技能，请始终将「includeSchema」与「includeGeneralEditInstructions」设置为 false，避免重复加载。
  ",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                },
                "includeSchema": {
                    "default": false,
                    "description": "返回结果是否携带Ardot规范定义。仅当本次对话已加载规范时设置为false。",
                    "type": "boolean"
                },
                "includeGeneralEditInstructions": {
                    "default": false,
                    "description": "返回结果是否携带通用编辑指引。若对话内已加载相关指引（例如已加载ardot-design-assistant技能），请设置为false。",
                    "type": "boolean"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```