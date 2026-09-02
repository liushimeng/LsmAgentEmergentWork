# browser_console 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "browser_console",
        "description": "获取当前页面的浏览器控制台输出与 JavaScript 错误信息。返回 console.log/ 警告 / 错误 / 信息日志以及未捕获的 JS 异常。可用于排查静默 JavaScript 报错、接口调用失败与应用警告。需先调用 browser_navigate 工具。若传入 expression 参数，将在页面环境中执行 JS 代码并返回执行结果，可用来查看 DOM、读取页面状态或通过代码提取数据。",
        "parameters": {
            "type": "object",
            "properties": {
            "clear": {
            "type": "boolean",
            "default": false,
            "description": "设为 true 时，读取日志后清空消息缓冲区"
            },
            "expression": {
            "type": "string",
            "description": "在页面环境中执行的 JavaScript 表达式，运行逻辑等同于浏览器开发者工具控制台，可完整访问 DOM、window、document 对象。返回值会序列化为 JSON 格式。示例：'document.title' 或 'document.querySelectorAll ("a").length'"
            }
            }
        }
    }
}
```