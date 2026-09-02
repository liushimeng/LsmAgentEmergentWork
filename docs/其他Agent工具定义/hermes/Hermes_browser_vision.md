# browser_vision 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "browser_vision",
    "description": "对当前页面进行截图，用于直观查看页面画面。当你需要看清页面视觉内容时使用，尤其适用于图形验证码、视觉验证校验、复杂页面布局，或是文本快照缺失关键视觉信息的场景。若当前使用的模型原生支持视觉能力，截图会直接附加到上下文，你可在下一轮查看；否则 Hermes 会调用辅助视觉模型并返回文本分析结果。返回内容包含截图路径，你可在回复中通过 MEDIA:<截图路径> 将截图展示给用户。使用前需先调用 browser_navigate。",
    "parameters": {
      "type": "object",
      "properties": {
        "question": {
          "type": "string",
          "description": "你想要从视觉层面了解页面的相关问题，请清晰描述你要查找的内容。"
        },
        "annotate": {
          "type": "boolean",
          "default": false,
          "description": "设为 true 时，会在可交互元素上标注带编号的 [N] 标记，每个 [N] 对应后续浏览器指令可用的元素引用 @eN。适合页面布局核验与空间位置判断场景。"
        }
      },
      "required": [ "question" ]
    }
  }
}
```