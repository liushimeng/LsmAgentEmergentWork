# view_image 工具定义
```json
{
"type": "function",
"name": "view_image",
"description": "View a local image file from the filesystem when visual inspection is needed. Use this for images already available on disk.",
"strict": false,
"parameters": {
"type": "object",
"properties": {
"path": {
"type": "string",
"description": "Local filesystem path to an image file."
}
}
,
"required": [
"path"
]
,
"additionalProperties": false
}
}
```