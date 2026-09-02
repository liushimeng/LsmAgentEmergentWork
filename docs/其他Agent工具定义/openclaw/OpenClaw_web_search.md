# web_search 工具定义
```json
{
  "name": "web_search",
  "description": "搜索网络以获取当前信息；返回标准化的提供商结果。",
  "parameters": {
    "type": "object",
    "required": [
    "query"
    ],
    "properties": {
    "query": {
    "type": "string",
    "description": "搜索查询词。"
    },
    "count": {
    "type": "number",
    "description": "返回结果的数量。",
    "minimum": 1,
    "maximum": 10
    },
    "country": {
    "type": "string",
    "description": "双字母国家/地区代码。"
    },
    "language": {
    "type": "string",
    "description": "ISO 639-1 语言代码。"
    },
    "freshness": {
    "type": "string",
    "description": "时间过滤条件：day（一天内）/ week（一周内）/ month（一个月内）/ year（一年内）。"
    },
    "date_after": {
    "type": "string",
    "description": "仅返回 YYYY-MM-DD 日期之后发布的内容。"
    },
    "date_before": {
    "type": "string",
    "description": "仅返回 YYYY-MM-DD 日期之前发布的内容。"
    },
    "search_lang": {
    "type": "string",
    "description": "Brave 搜索结果的语言。"
    },
    "ui_lang": {
    "type": "string",
    "description": "Brave 界面区域设置。"
    },
    "domain_filter": {
    "type": "array",
    "items": {
    "type": "string"
    },
    "description": "Perplexity 域名过滤器。"
    },
    "max_tokens": {
    "type": "number",
    "description": "Perplexity 总 Token 预算。",
    "minimum": 1,
    "maximum": 1000000
    },
    "max_tokens_per_page": {
    "type": "number",
    "description": "Perplexity 每个页面的 Token 数量。",
    "minimum": 1
    }
    }
  }
}
```