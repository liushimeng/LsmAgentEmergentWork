# image 工具定义
```json
{
  "name": "image",
  "description": "使用视觉模型分析图像。使用 image 参数传入单个路径/URL，使用 images 参数最多可传入 20 个。仅在图像尚未提供时使用此工具；提示词中已包含的图像当前已可见。",
  "parameters": {
    "type": "object",
    "properties": {
    "prompt": {
    "type": "string"
    }
    ,
    "image": {
    "type": "string",
    "description": "单个图像的路径或 URL。"
    }
    ,
    "images": {
    "type": "array",
    "items": {
    "type": "string"
    }
    ,
    "description": "图像的路径或 URL 列表；maxImages 的默认值为 20。"
    }
    ,
    "model": {
    "type": "string"
    }
    ,
    "maxBytesMb": {
    "type": "number",
    "exclusiveMinimum": 0
    }
    ,
    "maxImages": {
    "type": "integer",
    "minimum": 1
    }
    }
  }
}
```