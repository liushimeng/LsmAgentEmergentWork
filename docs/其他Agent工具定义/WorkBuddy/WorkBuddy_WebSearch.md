# WebSearch 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "WebSearch",
        "description": "
- 在互联网执行搜索，并利用搜索结果生成回答内容
- 针对时事、最新数据提供时效性信息
- 返回结构化搜索结果块
- 当需要获取知识库截止日期以外的信息时使用本工具
- 在单次API请求内自动完成多项检索

使用注意事项：
  - 支持域名筛选，可指定包含或屏蔽特定网站
  - 需要留意环境内的「当前日期」。例如环境标注「当前日期：2025-07-01」，用户需要最新文档时，搜索关键词不要使用2024年，应当使用2025年。
  - 当需要进行对比、多主题调研，或是需要多措辞/多语言/多角度变体查询以提升覆盖度时，使用`query_keyword_groups`；`query`保留为主检索意图，最多5组关键词。
    示例："NVIDIA stock and AI chip market latest news" → query_keyword_groups: ["NVIDIA stock latest news", "AI chip market latest news", "英伟达 股价 AI芯片 最新动态"]
  - 使用`topic`参数指定搜索领域，提升结果相关性。可选值：general（默认）、news、programming、documentation、academic、finance、technology、legal、medical。
",
        "parameters": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 2,
                    "description": "搜索主关键词"
                },
                "allowed_domains": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "仅返回来自这些域名的搜索结果"
                },
                "blocked_domains": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "屏蔽这些域名，不返回其搜索结果"
                },
                "query_keyword_groups": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "maxItems": 5,
                    "description": "当对比多项技术/产品/概念，或是检索包含多个独立子主题时**必须使用**。每个角度填写一条简短关键词。示例：对比 Redis、Memcached、DragonflyDB → [\"Redis high concurrency benchmark\", \"Memcached performance benchmark\", \"DragonflyDB performance benchmark\", \"Redis vs Memcached vs DragonflyDB comparison\"]。禁止拆分多次调用WebSearch，请在单次请求使用此字段。最多5组。"
                },
                "topic": {
                    "type": "string",
                    "enum": [
                        "general",
                        "news",
                        "programming",
                        "documentation",
                        "academic",
                        "finance",
                        "technology",
                        "legal",
                        "medical"
                    ],
                    "description": "可选主题类型，优化搜索相关性。可选值：general, news, programming, documentation, academic, finance, technology, legal, medical"
                }
            },
            "required": [
                "query"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```