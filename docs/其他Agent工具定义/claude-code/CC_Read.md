# Read 文件读取工具配置
```json
{
    "name": "Read",
    "description": "读取本地文件系统中的文件。

    - `file_path` 必须使用绝对路径。
    - 默认最多读取 2000 行内容。
    - 可按需指定读取起始行与读取行数（处理大文件时尤为实用），若无特殊需求，建议不设置这两个参数、直接读取完整文件。
    - 输出格式等同于 `cat -n`，行号从 1 开始计数。
    - 支持读取图片文件（PNG、JPG 等）并可视化展示；读取 PDF 文件需通过 `pages` 参数指定页码范围（例如 \"1-5\"），单次最多读取 20 页，PDF 文件超过 10 页时该参数为必填；读取 Jupyter 笔记本文件（.ipynb）会按单元格+运行结果的形式展示。
    - 若读取目录、不存在的文件或空文件，不会返回文件内容，而是提示错误或系统提醒。
    - 文件编辑/写入完成后，**无需重新读取文件校验结果**：编辑、写入操作执行失败时会主动报错，且运行环境会自动记录文件状态。",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "file_path": {
                "description": "待读取文件的绝对路径",
                "type": "string"
            },
            "offset": {
                "description": "读取的起始行号。仅在文件过大、无法一次性读取时填写",
                "type": "integer",
                "minimum": 0,
                "maximum": 9007199254740991
            },
            "limit": {
                "description": "要读取的行数。仅在文件过大、无法一次性读取时填写",
                "type": "integer",
                "exclusiveMinimum": 0,
                "maximum": 9007199254740991
            },
            "pages": {
                "description": "PDF 文件的页码范围（示例：\"1-5\"、\"3\"、\"10-20\"），仅对 PDF 文件生效，单次请求最多读取 20 页",
                "type": "string"
            }
        },
        "required": [
            "file_path"
        ],
        "additionalProperties": false
    }
}
```