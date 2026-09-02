# question 工具定义
```json
{
  "name": "question",
  "description": "在执行过程中需要向用户提问时使用此工具。这使您能够：
  1. 收集用户偏好或需求
  2. 澄清模糊的指示
  3. 在工作时就实施选择获取决策
  4. 向用户提供有关后续方向的选择。

  使用说明：
  - 当启用 `custom` 时（默认），会自动添加一个“输入自己的答案”选项；因此不要包含“其他”或通配符选项
  - 答案以标签数组形式返回；设置 `multiple: true` 以允许多选
  - 如果您推荐特定选项，请将其作为列表中的第一个选项，并在标签末尾添加“(Recommended)”
  ",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "questions": {
    "type": "array",
    "items": {
    "type": "object",
    "properties": {
    "question": {
    "type": "string",
    "description": "完整的问题"
    },
    "header": {
    "type": "string",
    "description": "非常简短的标签（最多30个字符）"
    },
    "options": {
    "type": "array",
    "items": {
    "type": "object",
    "properties": {
    "label": {
    "type": "string",
    "description": "显示文本（1-5个词，简洁）"
    },
    "description": {
    "type": "string",
    "description": "对选项的解释"
    }
    },
    "required": [
    "label",
    "description"
    ]
    },
    "description": "可用的选择"
    },
    "multiple": {
    "type": "boolean",
    "description": "允许选择多个选项"
    }
    },
    "required": [
    "question",
    "header",
    "options"
    ]
    },
    "description": "要提问的问题"
    }
    },
    "required": [
    "questions"
    ]
  }
}
```