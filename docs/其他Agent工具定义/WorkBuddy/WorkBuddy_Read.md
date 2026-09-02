# Read 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "Read",
        "description": "读取本地文件系统中的文件。你可以直接使用该工具访问任意文件。
假定此工具能够读取设备上所有文件。如果用户提供文件路径，则默认该路径有效。读取不存在的文件属于合法操作，工具会返回错误信息。

使用规则：
- file_path 参数必须填写绝对路径，不能使用相对路径
- 默认行为：从文件开头读取最多2000行内容
- 你可以选择指定起始行偏移量与读取行数上限（读取大文件非常实用）；若无特殊需求，建议不设置这两个参数，直接读取完整文件
- 超过2000个字符的单行内容会被截断
- 返回结果采用 cat -n 格式展示，行号从1开始计数
- CodeBuddy Code 可通过该工具读取图片文件（例如 PNG、JPG 等）。由于 CodeBuddy Code 属于多模态大模型，读取图片时会以可视化形式呈现内容。
- 支持读取PDF文件（.pdf）。PDF按页面解析，同时提取文本与图像内容用于分析。
- 支持读取Jupyter Notebook（.ipynb文件），返回所有单元格及其运行输出，整合代码、文本与可视化图表。
- 该工具仅支持读取文件，无法读取文件夹。如需浏览目录，请通过 Bash 工具执行 ls 命令。
- 单次回复内可以同时发起多个工具调用。建议并行预读取多个可能用到的文件，提升效率。
- 你会经常需要读取截图。若用户提供截图路径，务必使用本工具打开对应路径文件。本工具兼容所有临时文件路径。
- 如果你读取到一个存在但内容为空的文件，返回内容位置会显示系统提醒警告，而非文件内容。
",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "待读取文件路径（支持绝对路径或相对路径）"
                },
                "offset": {
                    "type": "number",
                    "description": "读取起始行号。仅在文件过大无法一次性读取时填写"
                },
                "limit": {
                    "type": "number",
                    "description": "读取的总行数。仅在文件过大无法一次性读取时填写。"
                }
            },
            "required": [
                "file_path"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```