# DeferExecuteTool 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "DeferExecuteTool",
        "description": "通过名称执行延迟工具。借助本工具可以调用经由 ToolSearch 查询到的工具，无需将其加载至当前可用工具列表。

使用流程：
- 首先调用 ToolSearch 查询目标工具并获取参数规范定义
- 随后调用本工具，传入准确的工具名称与参数
- 参数会依据目标工具的规范在校验通过后执行
- 目标工具自身的权限校验与钩子逻辑正常生效

示例：
DeferExecuteTool({ toolName: "ImageGen", params: { prompt: "山间日落" } })

注意事项：
- 参数校验失败时，会返回详细错误信息以及预期的参数规范
- 如果你在上一轮对话中已经知晓工具名称与参数结构，可以省略 ToolSearch 步骤
- 本工具遵循标准权限校验规则（根据权限配置，执行可能需要用户授权）
",
        "parameters": {
            "type": "object",
            "properties": {
                "toolName": {
                    "type": "string",
                    "description": "待执行延迟工具的完整名称（与 ToolSearch 返回结果保持一致）。"
                },
                "params": {
                    "type": "object",
                    "additionalProperties": {},
                    "description": "传递给目标工具的参数。必须与上一轮 ToolSearch 返回的工具参数规范匹配。"
                }
            },
            "required": [
                "toolName",
                "params"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```