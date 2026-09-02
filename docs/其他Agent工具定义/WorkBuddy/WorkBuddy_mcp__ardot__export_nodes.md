# mcp__ardot__export_nodes 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "mcp__ardot__export_nodes",
        "description": "将节点导出为 PNG/JPEG/WEBP/SVG/PDF 格式的图像文件。

- 接收节点ID数组，将每个节点分别导出为独立图像文件。
- SVG格式单次调用无节点数量限制；图像格式（PNG/JPEG/WEBP）建议每批导出不超过5个节点。
- PDF格式：`nodeIds`内所有画布（Frame）节点会合并生成单个PDF文件（无论传入多少节点ID，仅输出一个文件）。若`nodeIds`不含画布节点，则默认导出当前页面全部画布。画布排序规则：先从上至下，再从左至右（不依照nodeIds传入顺序）。
- 支持格式：PNG、JPEG、WEBP、SVG、PDF。
- 文件命名规则：
  - PNG/JPEG/WEBP/SVG：文件名使用对应节点ID。
  - PDF：仅生成一个文件，以当前文档名称命名（示例：`<文档名称>.pdf`）；文档无名称时默认命名为`document.pdf`。
- 返回以节点ID为键、导出文件绝对路径为值的映射对象。PDF模式下，映射内仅有一条记录，键为文档名称（并非节点ID）。
- 默认使用2倍缩放与高质量参数。
- SVG导出固定使用1倍缩放，不受scale参数影响。
- PDF导出不生效`scale`缩放参数。",
        "parameters": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": [
                "nodeIds",
                "outputDir"
            ],
            "properties": {
                "fileId": {
                    "description": "目标文件ID。如果同时打开多个文件且未指定该参数，工具会返回可用文件ID供选择。",
                    "type": "string"
                },
                "nodeIds": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "待导出的节点ID数组。导出PDF时，非画布（Frame）节点会被忽略；未传入画布节点时的兜底规则详见工具说明。"
                },
                "outputDir": {
                    "type": "string",
                    "description": "导出文件的保存目录"
                },
                "format": {
                    "description": "导出格式（默认：png）。填写`pdf`可将多个画布合并为单个PDF文件。",
                    "type": "string",
                    "enum": [
                        "png",
                        "jpeg",
                        "webp",
                        "svg",
                        "pdf"
                    ]
                },
                "scale": {
                    "description": "导出缩放倍率（默认：2）。不适用于 SVG、PDF 格式。",
                    "type": "number"
                },
                "quality": {
                    "description": "JPEG、WEBP格式的导出画质（取值1-100）。JPEG默认95；WEBP默认100（无损）。",
                    "type": "number"
                }
            },
            "additionalProperties": true
        },
        "strict": false
    }
}
```