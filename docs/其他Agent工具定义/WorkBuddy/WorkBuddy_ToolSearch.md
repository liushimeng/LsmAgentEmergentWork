# ToolSearch 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "ToolSearch",
        "description": "调用延迟工具前，加载对应的工具规范定义（Schema）。

延迟工具**不可直接调用** — 需要先用 ToolSearch 加载工具规范，再通过 DeferExecuteTool 执行调用。

## 两种查询模式
1. 精确匹配（优先使用）：`tool_names: ["ImageGen"]` — 当你明确知道工具名称时使用。
2. 关键词检索：`queries: ["image generation"]` — 不确定适用哪个工具时使用。

## 使用规则
- 已知工具全称 → 使用 `tool_names`；未加载工具规范前，严禁随意猜测参数。
- 查询到目标工具后，通过 DeferExecuteTool 传入校验合法的参数完成调用。


以下为可通过 ToolSearch 查询的延迟工具列表。它们的规范定义不会自动加载，直接调用会触发输入校验异常。调用前必须使用 ToolSearch 配合 `tool_names` 加载规范：



<available_deferred_tools>
connect_cloud_service: 连接云服务，从当前桌面会话获取访问凭证。**仅允许在激活技能/场景为「多模态内容生成」、「neodata-financial-search」或「library（资料库）」时使用**。其他技能或普通对话场景禁止调用，调用将失效。重要提示：调用本工具后，回复用户时不要提及、总结任何凭证、鉴权信息、令牌或登录状态，直接静默执行后续操作。
ImageGen: 注意：图像生成会调用额外模型，产生独立点数消耗。单张图片大约消耗5-10点数。执行前**必须告知用户点数消耗情况**。

根据文本描述生成图像，或依据文本指令编辑、变换已有图片。传入图片参数时执行图生图；无图片参数则文生图。
ListMcpResources: 列出已连接MCP服务对外开放的资源。
ReadMcpResource: 读取MCP服务内指定资源。
TaskStop: 根据任务ID终止正在运行的后台任务。
VideoGen: 注意：视频生成会调用额外模型，产生独立点数消耗。一段5秒视频大约消耗50-100点数。执行前**必须告知用户点数消耗情况**。

借助AI模型，根据文本描述或输入图片生成短视频。传入图片则基于图片生成视频；仅提供文本则纯文生视频。视频生成耗时1-3分钟，成品以MP4格式保存在本地。
workbuddy_marketplace_skill: 在WorkBuddy推荐市场（BuiltinMarket）搜索或安装技能。action填「search」可通过关键词检索技能；action填「install」根据skillId完成安装。鉴权由宿主程序处理，禁止传递令牌或链接地址。

mcp__agent-mail
mcp__agent-mail__DeleteMessage
mcp__agent-mail__download_attachment
mcp__agent-mail__ForwardMessage
mcp__agent-mail__GetMe
mcp__agent-mail__GetMessage
mcp__agent-mail__ListAttachments
mcp__agent-mail__ListMessages
mcp__agent-mail__ReplyMessage
mcp__agent-mail__SearchMessages
mcp__agent-mail__SendMessage
mcp__agent-mail__upload_attachment

mcp__weixinpay
mcp__weixinpay__weixinpay_feedback
mcp__weixinpay__weixinpay_register
mcp__weixinpay__weixinpay_retry_pay
</available_deferred_tools>

",
        "parameters": {
            "type": "object",
            "properties": {
                "queries": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "基于MiniSearch全文检索引擎的关键词检索。支持前缀匹配、模糊匹配以及相关性排序，必须为字符串数组。为获得更好检索效果，建议同时传入中英文关键词（示例：[\"获取时间\", \"get time\"]、[\"文件搜索\", \"file search\", \"glob\"]）。"
                },
                "tool_names": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "精确查找完整限定工具名称（示例：[\"mcp__time__current_time\", \"LSP\", \"mcp__cnb__cnb_create_pull\"]）。禁止使用名称片段、功能描述、关键词进行查询。"
                },
                "top_k": {
                    "type": "integer",
                    "exclusiveMinimum": 0,
                    "maximum": 20,
                    "default": 3,
                    "description": "返回完整详情的工具最大数量（默认3条，上限20条）。仅在使用「queries」检索时生效；使用「tool_names」时该参数无效。超出数量上限的匹配项仅作为候选展示。"
                }
            },
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```