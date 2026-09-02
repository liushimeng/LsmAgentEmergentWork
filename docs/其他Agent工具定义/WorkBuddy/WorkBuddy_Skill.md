# Skill 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "Skill",
        "description": "在当前对话中执行技能

当用户要求执行任务时，检查下方可用技能列表是否存在匹配项。技能提供专业能力与领域知识。

当用户提及「斜杠命令」或者「/<内容>」（例如：/commit、/review-pr），代表用户想要调用技能。使用此工具进行调用。

调用方式说明：
- 使用本工具，传入技能名称与可选参数
- 示例：
  - `skill: "pdf"` — 调用pdf技能
  - `skill: "commit", args: "-m 'Fix bug'"` — 携带参数调用
  - `skill: "ms-office-suite:pdf"` — 使用完整限定名称调用

重要规则：
- 当存在匹配用户需求的技能时，这是**强制要求**：在生成任何任务相关回复之前，必须优先调用对应的Skill工具
- 禁止只提及技能名称却不调用本工具
- 不要调用正在运行中的技能
- 本工具不可用于内置CLI命令（例如 /help、/clear 等）
- 如果当前对话轮次存在 <command-name> 标签，说明技能**已经加载完成**，直接按照指令执行，无需再次调用本工具

<available_skills>
- weixinpay-feedback: 【微信支付官方】问题反馈。当用户使用微信AI支付/AI专属卡的开通/绑定或支付/管理过程遇到异常或错误（尤其是同一环节连续/反复报错、重试仍失败），主动引导用户上报问题。开通/绑定过程的问题通过反馈收集表链接反馈；支付/管理问题或用户直接想要反馈时通过工具反馈。关键词：微信AI支付反馈、Agent… (location: /Users/dev02/.workbuddy/plugins/cache/workbuddy-builtin/weixinpay/1.5.111/skills/weixinpay-feedback/SKILL.md)
- weixinpay-pay: 【微信支付官方】微信支付AI专属卡支付能力。普通支付在商户下单后由系统自动触发工程化支付卡片，无需智能体介入；本技能主要指导「重新支付」：用户取消/关闭支付后想对同一笔订单再付一次时，智能体须先询问并确认订单，再用上次相同凭据（payUrls 或 WeixinPay-Required 的值）调用 … (location: /Users/dev02/.workbuddy/plugins/cache/workbuddy-builtin/weixinpay/1.5.111/skills/weixinpay-pay/SKILL.md)
- weixinpay-register: 【微信支付官方】用于用户开通/绑定微信AI支付或者AI专属卡功能，或查询开通/绑定状态。用于用户要开通、绑定、激活在 AI 对话中使用微信支付的能力。也用于回答用户"我是否已开通、如何查看或管理微信AI支付"等开通状态咨询。关键词：微信支付AI专属卡、微信AI支付、开通微信支付、绑定微信支付、激活… (location: /Users/dev02/.workbuddy/plugins/cache/workbuddy-builtin/weixinpay/1.5.111/skills/weixinpay-register/SKILL.md)
- tencent-docs: 腾讯文档个人版（docs.qq.com）-在线云文档平台，是创建、编辑、管理文档的首选 skill。涉及"新建/创建/编辑/读取/查看/搜索文档"、"保存文件"、"云文档"、"腾讯文档"、"docs.qq.com"等操作，请优先使用本 skill。支持能力：(1) 创建各类在线文档（文档/Word… (location: /Users/dev02/.workbuddy/plugins/cache/workbuddy-builtin/tencent-docs-plugin/1.0.0/skills/tencent-docs/SKILL.md)
- tencent-saas-docs: 腾讯文档企业版（saas.docs.qq.com）-在线云文档平台，是创建、编辑、管理文档的首选 skill。涉及"新建/创建/编辑/读取/查看/搜索文档"、"企业文档"、"团队文档"、"saas.docs.qq.com"等操作，请优先使用本 skill。支持能力：(1) 创建各类在线文档（文档/… (location: /Users/dev02/.workbuddy/plugins/cache/workbuddy-builtin/tencent-docs-plugin/1.0.0/skills/tencent-saas-docs/SKILL.md)
- tencent-pptx: 创建专业的 PowerPoint 演示文稿。适用于根据主题、大纲、文档、数据或参考材料生成完整 .pptx；在新建演示文稿时参考上传PPTX 的视觉风格；也可基于材料或旧 PPT 内容重新生成一版演示文稿。 (tencent-pptx@workbuddy-builtin) (location: /Users/dev02/.workbuddy/plugins/cache/workbuddy-builtin/tencent-pptx/v20260712/skills/tencent-pptx/SKILL.md)
- 3D模型与视频特效: 3D模型生成和基于模板的视频特效能力。仅支持：文生3D模型、图生3D模型、基于模板的图片视频特效（video-fx）。 适用于用户需要 AI 创作 3D 模型或对图片应用模板特效（如拥抱、变身、万物归尘等动效模板）的场景。 当用户提出以下意图时触发：生成/制作3D模型、对图片应用特效模板或动效模板… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/buddy-multimodal-generation/SKILL.md)
- cloudstudio-deploy: 将静态网站部署至 CloudStudio 沙箱工作空间。当用户想要部署本地构建目录（例如 dist/、build… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/cloudstudio-deploy/SKILL.md)
- expert-manager:   专家包的全生命周期运营：转化(从开源仓库/本地项目创建专家包)、修改已有专家、合规检查、批量更新、质量审查。
  触发词：创建专家、转化专家、转成专家、生成专家包、导入专家、convert expert、修改专家、编辑专家、更新专家、modify expert、检查专家、审查专家包、专家合规、… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/expert-manager/SKILL.md)
- geo-map-compliance-guard: 任何地图生成、可视化、路线规划、位置服务请求**必须**触发地图合规技能，严格遵守中国地图数据合规要求… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/geo-map-compliance-guard/SKILL.md)
- marketplace-skill-installer:   在 WorkBuddy 对话中通过一句话从推荐市场（BuiltinMarket）搜索并安装 Skill。
  触发词：安装 skill、安装技能、install skill、添加技能、装个 X 技能、帮我安装 X、find skill、install marketplace skill、sea… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/marketplace-skill-installer/SKILL.md)
- neodata-financial-search: NeoData 金融搜索 — 自然语言金融数据检索。可查询股票（A股/港股/美股）、基金、指数、行业板块、宏观经济、外汇、大宗商品… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/neodata-financial-search/SKILL.md)
- skill-creator: 用于指导创建实用技能。用户想要新建技能（或者更新已有技能）以扩展 WorkBuddy 能力时使用此技能… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/skill-creator/SKILL.md)
- tencent-docs-routing: 在处理本地 Office/WPS 文件（doc/docx/dot/wps/wpt、xls/xlsx/xlt/csv/tsv、ppt/pptx/pps/pot），或是创建相关文档前，加载该技能… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/tencent-docs-routing/SKILL.md)
- tencent-local-office-edit: 通过本地 editor_sdk 实时读写本机磁盘上的 Office/WPS 类型文件——文件打开后用户在编辑器中实时可见，编辑所见即所得，保存使用 save_file，请勿主动执行 close_file。支持本地 doc/docx/dot/wps/wpt、xls/xlsx/xlt/csv/tsv… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/tencent-local-office-edit/SKILL.md)
- wb-finance-skill: 金融 / 投资 / 股票 / 基金 / ETF / 板块 / 指数 / 宏观 / 外汇 / 大宗商品 / 财报 / 估值 / 持仓 / 交易 / 仓位 / 量化 / 因子 / 回测 / 选股 / 期权 / 衍生品 / 投行建模 / 技术指标 / 行情监控 / 预警——金融场景总入口，优先级**高于… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/wb-finance-skill/SKILL.md)
- westock-data: 金融市场结构化数据查询的权威入口。支持股票（A股/港股/美股/日韩股）、ETF、指数、板块、期货、外汇、可转债的行情、财报、研报、新闻、公告、事件、股东、分红、ETF持仓、宏观经济、热搜榜、新股/投资日历、龙虎榜等数据查询；不同标的与市场支持的维度不同，以 `help` 与 references/… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/westock-data/SKILL.md)
- westock-tool: 选股 / 选基工具——按条件、策略、标签、事件、排行从全市场批量筛选股票或 ETF。当用户问"找一只 / 哪些股票 / 帮我选 / 推荐 / 排行榜 / TOP / 筛选 / MACD金叉 / 央企股 / ST股 / 高股息ETF"时使用。提供 filter（自定义条件）、strategy（预置策… (location: /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/resources/builtin-skills/westock-tool/SKILL.md)
</available_skills>
",
        "parameters": {
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "技能名称。例如：\"commit\"、\"review-pr\" 或者 \"pdf\""
                },
                "command": {
                    "type": "string",
                    "description": "(旧版兼容字段) 技能名称（不带参数）。例如：\"pdf\" 或 \"xlsx\""
                },
                "args": {
                    "type": "string",
                    "description": "技能可选运行参数"
                }
            },
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```