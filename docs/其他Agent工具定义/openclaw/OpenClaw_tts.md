# tts 工具定义
```json
{
  "name": "tts",
  "description": "仅在明确需要音频输出（语音/朗读/TTS）或当前已启用 TTS 配置时使用。切勿用于普通的文本回复。音频将自动通过工具结果进行交付；成功后请遵循回复指令，无需重复输出文本或音频。",
  "parameters": {
    "type": "object",
    "required": [
    "text"
    ],
    "properties": {
    "text": {
    "type": "string",
    "description": "需要朗读的文本。"
    },
    "channel": {
    "type": "string",
    "description": "频道 ID；作为输出格式的提示。"
    },
    "timeoutMs": {
    "type": "integer",
    "description": "服务提供商的超时时间（毫秒）。",
    "minimum": 1
    }
    }
  }
}
```