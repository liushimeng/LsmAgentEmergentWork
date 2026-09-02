# Grep 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "Grep",
        "description": "基于ripgrep实现的强大内容检索工具

  使用规则：
  - 检索任务务必使用Grep工具。禁止通过Bash调用`grep`或`rg`命令。Grep工具针对权限与访问逻辑做了优化。
  - 支持完整正则语法（例如：\"log.*Error\"、\"function\\s+\\w+\"）
  - 可通过glob参数过滤文件（例如：\"*.js\"、\"**/*.tsx\"），或type参数指定文件类型（例如：\"js\"、\"py\"、\"rust\"）
  - 输出模式：\"content\"展示匹配代码行，\"files_with_matches\"仅输出文件路径（默认），\"count\"展示匹配数量
  - 支持分页：`head_limit`限制输出条目（默认无限制），`offset`跳过前N条结果（默认0）
  - 如果检索范围宽泛，需要多轮组合查询，请使用Agent工具
  - 表达式语法：采用ripgrep规则（非传统grep），字面量大括号需要转义（例如查找Go代码`interface{}`需写为`interface\\{\\}`）
  - 多行匹配：默认仅单行内匹配。如果需要跨行匹配（例如`struct \\{[\\s\\S]*?field`），开启`multiline: true`",
        "parameters": {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "用于在文件内容中检索的正则表达式"
                },
                "path": {
                    "type": "string",
                    "description": "待检索文件或目录（对应 rg PATH 参数）。默认使用当前工作目录。"
                },
                "glob": {
                    "type": "string",
                    "description": "用于筛选文件的glob通配规则（例如 \"*.js\"、\"*.{ts,tsx}\"），等价于 rg --glob"
                },
                "output_mode": {
                    "type": "string",
                    "enum": [
                        "content",
                        "files_with_matches",
                        "count"
                    ],
                    "description": "输出模式：\"content\"展示匹配行（支持前后上下文-A/-B/-C、行号-n、head_limit）；\"files_with_matches\"只输出文件路径（支持head_limit）；\"count\"展示匹配次数（支持head_limit）。默认值为 \"files_with_matches\"。"
                },
                "-B": {
                    "type": "number",
                    "description": "每条匹配项前方展示的上下文行数（rg -B）。仅在 output_mode: \"content\" 生效，其余模式忽略。"
                },
                "-A": {
                    "type": "number",
                    "description": "每条匹配项后方展示的上下文行数（rg -A）。仅在 output_mode: \"content\" 生效，其余模式忽略。"
                },
                "-C": {
                    "type": "number",
                    "description": "上下文参数别名。"
                },
                "context": {
                    "type": "number",
                    "description": "每条匹配项前后同时展示的上下文行数（rg -C）。仅在 output_mode: \"content\" 生效，其余模式忽略。"
                },
                "-n": {
                    "type": "boolean",
                    "description": "输出中显示行号（rg -n）。仅在 output_mode: \"content\" 生效，其余模式忽略。"
                },
                "-i": {
                    "type": "boolean",
                    "description": "大小写不敏感检索（rg -i）"
                },
                "type": {
                    "type": "string",
                    "description": "限定检索的文件类型（rg --type）。常用类型：js、py、rust、go、java 等。标准文件类型场景下，该方式比glob包含规则效率更高。"
                },
                "head_limit": {
                    "type": "number",
                    "description": "限制仅输出前N行/条目，等价于 \"| head -N\"。适用于全部输出模式：content限制输出行数，files_with_matches限制文件数量，count限制统计条目。不填时返回ripgrep全部结果。"
                },
                "offset": {
                    "type": "number",
                    "description": "在head_limit生效前跳过前N行/条目，等价于 \"| tail -n +N | head -N\"。适用于全部输出模式，默认值0。"
                },
                "multiline": {
                    "type": "boolean",
                    "description": "开启多行模式，此时.可以匹配换行符，正则能够跨行匹配（rg -U --multiline-dotall）。默认：false。"
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