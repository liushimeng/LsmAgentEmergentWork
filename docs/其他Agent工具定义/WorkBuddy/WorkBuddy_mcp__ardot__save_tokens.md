# mcp__ardot__save_tokens 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__save_tokens",
        "description": "向WebView保存Ardot平台的身份凭证令牌。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [
                "authToken"
            ],
            "properties": {
                "authToken": {
                    "type": "string",
                    "description": "Ardot平台的身份认证令牌。"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```
