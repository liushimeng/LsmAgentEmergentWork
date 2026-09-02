# image_generate 工具定义
```json
{
"type": "function",
"function": {
  "name": "image_generate",
  "description": "通过文本提示词生成高质量图像（文生图），或在当前活动模型支持的情况下，编辑/转换现有图像（图生图）。传入 image_url 以编辑该图像；添加 reference_image_urls 以提供风格/构图参考；若两者均省略，则为文生图模式。底层后端（FAL、OpenAI、xAI 等）和模型由用户配置，代理无法自行选择。返回的 image 字段包含一个 URL 或绝对文件路径；请使用 Markdown 格式 进行展示，网关会自动进行交付。当活动终端后端处于不同的文件系统时，成功的本地文件结果可能还会包含 agent_visible_image 字段，以便后续进行终端/文件操作。\n\n当前活动后端：FAL.ai · 模型：FLUX 2 Klein 9B\n- 支持文生图（省略 image_url）和图生图/编辑（传入 image_url）；通过 reference_image_urls 最多支持 9 张参考图像 —— 系统会自动路由",
  "parameters": {
    "type": "object",
    "properties": {
    "prompt": {
    "type": "string",
    "description": "描述所需图像的文本提示词（文生图），或描述要应用的编辑操作（图生图）。请尽可能详细和具体。"
    }
    ,
    "aspect_ratio": {
    "type": "string",
    "enum": [
    "landscape",
    "square",
    "portrait"
    ]
    ,
    "description": "生成图像的宽高比。'landscape' 为 16:9 横屏，'portrait' 为 16:9 竖屏，'square' 为 1:1 正方形。",
    "default": "landscape"
    }
    ,
    "image_url": {
    "type": "string",
    "description": "可选的源图像，用于编辑/转换（图生图）。提供此参数时，活动后端会路由至其图像编辑端点；省略时，则仅通过文本生成。可传入公共 URL 或对话中的绝对本地文件路径。仅支持编辑的模型才会处理此参数 —— 上方说明会指出当前活动模型是否支持。"
    }
    ,
    "reference_image_urls": {
    "type": "array",
    "items": {
    "type": "string"
    }
    ,
    "description": "可选的额外参考图像 URL/路径列表（用于风格、角色或构图参考），以指导图生图编辑。仅部分模型支持，且数量上限因模型而异；上方说明会指出最大值。"
    }
    }
    ,
    "required": [
    "prompt"
    ]
  }
}
}
```