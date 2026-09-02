# Grep 工具定义
```json
{
  "name": "Grep",
  "description": "一个基于 ripgrep 构建的强大搜索工具\n\n  使用方法：\n  - 对于搜索任务 ALWAYS 使用 Grep 工具。NEVER 调用 `grep` 或 `rg` 作为 Bash 命令。Grep 工具已针对正确的权限和访问进行了优化。\n  - 支持完整的正则表达式语法（例如，\"log.*Error\", \"function\\s+\\w+\"）\n  - 通过 glob 参数（例如 \"*.js\", \"**/*.tsx\"）或 type 参数（例如 \"js\", \"py\", \"rust\"）过滤文件\n  - 输出模式：\"content\" 显示匹配的行，\"files_with_matches\" 仅显示文件路径（默认），\"count\" 显示匹配计数\n  - 对于需要多轮搜索的开放式搜索，请使用 Agent 工具\n  - 模式语法：使用 ripgrep（而非 grep）— 字面量花括号需要转义（例如在 Go 代码中查找 `interface{}` 时，请使用 `interface\\{\\}`）\n  - 多行匹配：默认情况下，模式仅在单行内匹配。对于跨行模式（如 `struct \\{[\\s\\S]*?field`），请使用 `multiline: true`",
  "input_schema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "pattern": {
    "description": "在文件内容中搜索的正则表达式模式",
    "type": "string"
    },
    "path": {
    "description": "要搜索的文件或目录 (rg PATH)。默认为当前工作目录。",
    "type": "string"
    },
    "glob": {
    "description": "用于过滤文件的 Glob 模式（例如 \"*.js\", \"*.{ts,tsx}\"）— 映射到 rg --glob",
    "type": "string"
    },
    "output_mode": {
    "description": "输出模式：\"content\" 显示匹配的行（支持 -A/-B/-C 上下文，-n 行号，head_limit），\"files_with_matches\" 仅显示文件路径（支持 head_limit），\"count\" 显示匹配计数（支持 head_limit）。默认为 \"files_with_matches\"。",
    "type": "string",
    "enum": [
    "content",
    "files_with_matches",
    "count"
    ]
    },
    "-B": {
    "description": "显示匹配前的行数 (rg -B)。需要 output_mode: \"content\"，否则将被忽略。",
    "type": "number"
    },
    "-A": {
    "description": "显示匹配后的行数 (rg -A)。需要 output_mode: \"content\"，否则将被忽略。",
    "type": "number"
    },
    "-C": {
    "description": "上下文行数的别名。",
    "type": "number"
    },
    "context": {
    "description": "显示匹配前后的行数 (rg -C)。需要 output_mode: \"content\"，否则将被忽略。",
    "type": "number"
    },
    "-n": {
    "description": "在输出中显示行号 (rg -n)。需要 output_mode: \"content\"，否则将被忽略。默认为 true。",
    "type": "boolean"
    },
    "-i": {
    "description": "不区分大小写的搜索 (rg -i)",
    "type": "boolean"
    },
    "-o": {
    "description": "仅打印匹配（非空）部分的每一行，每个输出行一个匹配 (rg -o / --only-matching)。需要 output_mode: \"content\"，否则将被忽略。默认为 false。",
    "type": "boolean"
    },
    "type": {
    "description": "要搜索的文件类型 (rg --type)。常见类型：js, py, rust, go, java 等。对于标准文件类型，比 include 更高效。",
    "type": "string"
    },
    "head_limit": {
    "description": "将输出限制为前 N 行/条目，等效于 \"| head -N\"。适用于所有输出模式：content（限制输出行数），files_with_matches（限制文件路径数），count（限制计数条目数）。未指定时默认为 250。传递 0 表示无限制（请谨慎使用 — 大型结果集会浪费上下文）。",
    "type": "number"
    },
    "offset": {
    "description": "在应用 head_limit 之前跳过前 N 行/条目，等效于 \"| tail -n +N | head -N\"。适用于所有输出模式。默认为 0。",
    "type": "number"
    },
    "multiline": {
    "description": "启用多行模式，其中 . 匹配换行符且模式可以跨行（rg -U --multiline-dotall）。默认：false。",
    "type": "boolean"
    }
    },
    "required": [
    "pattern"
    ],
    "additionalProperties": false
  }
}
```