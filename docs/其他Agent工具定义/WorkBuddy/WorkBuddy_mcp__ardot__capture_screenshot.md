# mcp__ardot__capture_screenshot 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__capture_screenshot",
        "description": "该工具可获取设计文件中一个或多个节点的截图。

使用规则：
- 重要提示：必须传入「screenShotDir」参数，指定截图保存目录。
- 重要提示：将所有需要核验的节点ID放入「nodeIds」数组，单次调用传入（数量少于10个）。批量截图相比重复调用本工具速度快很多。
- 重要提示：务必分析返回的截图，确认渲染效果正常、符合设计要求，不存在视觉错误、显示异常、元素错位等问题。
- 重要提示：获取截图后仔细排查生成设计中潜在的问题与样式不一致情况。
- 重要提示：不要对宽或高超过2000的超大节点截图。应当分区域分别截图核验。
- 根据节点ID获取指定节点、画布框架或其他元素截图，可用于对比校验生成布局与原始设计。
- 也可使用该工具查看元素与设计系统组件，核验布局与视觉效果。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [
                "nodeIds",
                "screenShotDir"
            ],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                },
                "nodeIds": {
                    "minItems": 1,
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "待截图的节点ID数组。将所有需要核验的节点一次性传入。示例：nodeIds: ['nodeId1', 'nodeId2', ...]"
                },
                "screenShotDir": {
                    "type": "string",
                    "description": "截图保存目录，必填参数。建议使用当前工作目录下的缓存文件夹。"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```