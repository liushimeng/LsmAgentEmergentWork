# Glob 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "Glob",
        "description": "- 高速文件通配匹配工具，支持任意规模代码库
- 支持通配表达式，例如 \"**/*.js\"、\"src/**/*.ts\"
- 返回匹配的文件路径，结果按文件修改时间排序
- 支持分页：`limit`（最大返回条数，默认100）与 `offset`（跳过前N条结果，默认0）
- 需要按照文件名规则查找文件时使用本工具
- 如果属于范围宽泛的检索，可能需要多次通配检索+内容检索组合操作，请改用Agent工具
- 单次回复内可发起多个工具调用。若多个检索任务都具备潜在价值，建议并行执行多项检索。
",
        "parameters": {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "用于匹配文件的glob通配表达式"
                },
                "path": {
                    "type": "string",
                    "description": "检索目标目录。不填则使用当前工作目录。重要提示：想要使用默认目录直接省略该字段。请勿填写\"undefined\"或\"null\"，直接省略即可启用默认逻辑。如果传入内容，则必须是合法目录路径。"
                },
                "limit": {
                    "type": "integer",
                    "exclusiveMinimum": 0,
                    "default": 100,
                    "description": "最大返回结果数量，默认值100。"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 0,
                    "description": "从开头跳过的结果条数，默认值0。"
                }
            },
            "required": [
                "pattern"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```