# text_to_speech 工具定义
```json
{
"type": "function",
"function": {
"name": "text_to_speech",
"description": "将文本转换为语音音频。返回格式为 MEDIA: 的文件路径，平台会原生分发该音频文件。兼容的服务商在 Telegram 上会以语音气泡展示；其他平台音频将作为普通附件发送。命令行模式下，音频文件保存至～/voice-memos/。语音音色与服务商由用户自行配置（内置服务商如 edge、openai，或在 tts.providers.<名称> 下配置自定义命令服务商），不可由模型自行选择。",
"parameters": {
"type": "object",
"properties": {
"text": {
"type": "string",
"description": "待转换为语音的文本。各服务商存在独立字符上限并自动强制截断（OpenAI 4096 字符、xAI 15000 字符、MiniMax 10000 字符、ElevenLabs 根据模型为 5000~40000 字符），超长输入会被截断处理。"
},
"output_path": {
"type": "string",
"description": "可选参数，自定义音频保存路径。默认路径：~/.hermes/audio_cache/< 时间戳 >.mp3"
}
},
"required": [
"text"
]
}
}
}
```