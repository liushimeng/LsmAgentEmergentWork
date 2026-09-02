# browser_press 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "browser_press",
        "description": "按下键盘按键，适用于提交表单（回车键）、切换焦点（Tab 键）或执行各类键盘快捷键。使用前必须先调用 browser_navigate。",
        "parameters": {
        "type": "object",
        "properties": {
        "key": {
            "type": "string",
            "description": "要按下的按键名称（例如：'Enter'、'Tab'、'Escape'、'ArrowDown'）"
        }
        },
        "required": [ "key" ]
        }
    }
}
```