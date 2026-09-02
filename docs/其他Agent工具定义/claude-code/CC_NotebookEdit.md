# NotebookEdit 工具定义
```json
{
    "name": "NotebookEdit",
    "description": "对 Jupyter 笔记本文件（.ipynb）中的单个单元格执行替换、插入或删除操作。

    使用规则：
    - 编辑前必须在当前会话中先用读取工具打开该笔记本文件，否则本工具会执行失败。
    - `notebook_path` 必须填写绝对路径。
    - `cell_id` 为读取工具输出内容里 `<cell id="...">` 标签中的编号，执行替换和删除操作时该项为必填。
    - `edit_mode` 默认值为 `replace`。选择 `insert` 可在指定编号的单元格后方新增单元格（未填写 `cell_id` 则在文档开头插入），插入时必须指定单元格类型。选择 `delete` 可删除对应单元格。",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "notebook_path": {
                "description": "待编辑的 Jupyter 笔记本文件绝对路径（必须使用绝对路径，不可使用相对路径）",
                "type": "string"
            },
            "cell_id": {
                "description": "待编辑单元格的编号。插入新单元格时，新单元格会置于该编号单元格之后；若不填写，则在文档开头插入。",
                "type": "string"
            },
            "new_source": {
                "description": "单元格的新内容",
                "type": "string"
            },
            "cell_type": {
                "description": "单元格类型（代码单元格或富文本单元格）。不填写则沿用原有单元格类型；编辑模式为插入时，该项为必填项。",
                "type": "string",
                "enum": [
                    "code",
                    "markdown"
                ]
            },
            "edit_mode": {
                "description": "编辑类型：替换、插入、删除，默认为替换。",
                "type": "string",
                "enum": [
                    "replace",
                    "insert",
                    "delete"
                ]
            }
        },
        "required": [
            "notebook_path",
            "new_source"
        ],
        "additionalProperties": false
    }
}
```