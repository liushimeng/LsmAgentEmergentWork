# vision_analyze 工具定义
```json
{
"type": "function",
"function": {
"name": "vision_analyze",
"description": "将图像加载到对话中以便您能够查看。接受 URL、本地文件路径或 data URL。当您的活动模型具备原生视觉能力时，图像将直接附加到您的上下文中，您可以在下一轮对话中自行读取像素——当用户提及图像时（例如消息中的文件路径、工具输出中的 URL、浏览器截图等），请随时调用此功能。对于非视觉模型，将回退使用辅助视觉模型，该模型会返回一段文本描述。",
"parameters": {
"type": "object",
"properties": {
"image_url": {
"type": "string",
"description": "要加载的图像 URL（http/https）、本地文件路径或 data: URL。"
}
,
"question": {
"type": "string",
"description": "您关于该图像的具体问题或请求。作为可选的上下文，供模型在下一轮看到图像后使用。"
}
}
,
"required": [
"image_url",
"question"
]
}
}
}
```
