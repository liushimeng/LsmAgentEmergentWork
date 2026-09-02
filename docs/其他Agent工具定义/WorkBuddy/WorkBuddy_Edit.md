# Edit 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "Edit",
        "description": "在文件内执行精确字符串替换操作。

使用规则：
- 在执行编辑操作前，本次对话中至少调用一次 `Read` 工具读取目标文件。未读取文件就尝试编辑将触发工具报错。
- 使用 Read 工具返回的文本进行编辑时，务必保留行号后缀之后原始的缩进格式（制表符/空格）。行号前缀格式为：空格 + 行号 + 制表符。制表符后方内容才是用于匹配的真实文件内容。请勿将任何行号前缀内容纳入 old_string 或 new_string。
- 优先编辑代码库中已存在的文件。除非明确要求，否则不要新建文件。
- 仅在用户明确要求时使用表情符号，若无要求不要在文件中添加表情。
- 如果 `old_string` 在文件内不唯一，本次编辑会失败。你可以增加更多上下文扩大匹配字符串保证唯一性，或者启用 `replace_all` 参数替换全部匹配项。
- 需要在文件内批量替换、重命名字符串时使用 `replace_all`。例如变量重命名场景非常适合该参数。

核心强制要求：
- 若 `old_string` 与 `new_string` 内容完全相同，工具执行失败
- `old_string` 必须和文件内容精确匹配（包含空格、空白字符），否则工具执行失败
- 该工具仅用于修改内容——新旧文本一致等同于没有变更

警告：
- 禁止使用本工具校验内容，禁止执行无实质改动的空操作
- `old_string` 和 `new_string` 绝不允许传入相同内容
- 如果无需修改任何内容，不要调用本工具
- 确保文本精确匹配，包含所有空白字符
",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "待修改文件路径（支持绝对路径或相对路径）"
                },
                "old_string": {
                    "type": "string",
                    "description": "待被替换的原始文本"
                },
                "new_string": {
                    "type": "string",
                    "description": "用于替换旧文本的新文本，内容必须和 old_string 不一致。两者相同时工具将会执行失败。"
                },
                "replace_all": {
                    "type": "boolean",
                    "default": false,
                    "description": "是否替换 old_string 的所有匹配项（默认 false）"
                }
            },
            "required": [
                "file_path",
                "old_string",
                "new_string"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```