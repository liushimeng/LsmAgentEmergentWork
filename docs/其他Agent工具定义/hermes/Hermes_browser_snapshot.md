# browser_snapshot 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "browser_snapshot",
    "description": "获取当前页面无障碍树的文本快照。返回带有引用 ID（如 @e1、@e2）的可交互元素，供 browser_click 和 browser_type 调用。full=false（默认）：仅展示含可交互元素的精简视图。full=true：返回页面全部内容。字符长度超过 8000 的快照会被截断或由大模型摘要。需先调用 browser_navigate。注意：browser_navigate 执行后已返回精简快照 —— 仅在页面发生交互变更后使用本工具刷新快照，或设置 full=true 获取完整页面内容。",
    "parameters": {
      "type": "object",
      "properties": {
        "full": {
        "type": "boolean",
        "description": "设为 true 时返回完整页面内容；设为 false（默认）时仅返回包含可交互元素的精简视图。",
        "default": false
        }
      }
    }
  }
}
```

