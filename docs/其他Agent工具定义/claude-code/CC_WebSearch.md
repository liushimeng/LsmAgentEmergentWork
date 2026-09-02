# WebSearch 工具定义
```json
{
    "name": "网页搜索",
    "description": "进行网络搜索，返回包含标题与网址的结果条目。**仅支持美国地区检索**。

- 当前时间为2026年6月，检索近期信息时请以此时间为准。
- 可通过`allowed_domains`（允许域名）和`blocked_domains`（屏蔽域名）筛选搜索结果。
- 依据搜索结果作答后，需以Markdown链接形式在文末列出引用的网址，并标注「来源(Sources)」。",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "对象",
        "properties": {
            "query": {
                "description": "搜索关键词",
                "type": "字符串",
                "minLength": 2
            },
            "allowed_domains": {
                "description": "仅展示来自这些域名的搜索结果",
                "type": "array",
                "items": {
                    "type": "字符串"
                }
            },
            "blocked_domains": {
                "description": "屏蔽来自这些域名的所有搜索结果",
                "type": "数组",
                "items": {
                    "type": "字符串"
                }
            }
        },
        "required": [
            "query"
        ],
        "additionalProperties": false
    }
}
```