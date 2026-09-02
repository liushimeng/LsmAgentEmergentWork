# nodes 工具定义
```json
{
  "name": "nodes",
  "description": "发现/控制已配对的节点：状态、描述、配对、通知、相机/照片/屏幕/定位/通知列表/调用。获取文件请使用 file_fetch。",
  "parameters": {
    "type": "object",
    "required": [
    "action"
    ]
    ,
    "properties": {
    "action": {
    "type": "string",
    "enum": [
    "status",
    "describe",
    "pending",
    "approve",
    "reject",
    "notify",
    "camera_snap",
    "camera_list",
    "camera_clip",
    "photos_latest",
    "screen_record",
    "location_get",
    "notifications_list",
    "notifications_action",
    "device_status",
    "device_info",
    "device_permissions",
    "device_health",
    "invoke"
    ]
    }
    ,
    "gatewayUrl": {
    "type": "string"
    }
    ,
    "gatewayToken": {
    "type": "string"
    }
    ,
    "timeoutMs": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "node": {
    "type": "string"
    }
    ,
    "requestId": {
    "type": "string"
    }
    ,
    "title": {
    "type": "string"
    }
    ,
    "body": {
    "type": "string"
    }
    ,
    "sound": {
    "type": "string"
    }
    ,
    "priority": {
    "type": "string",
    "enum": [
    "passive",
    "active",
    "timeSensitive"
    ]
    }
    ,
    "delivery": {
    "type": "string",
    "enum": [
    "system",
    "overlay",
    "auto"
    ]
    }
    ,
    "facing": {
    "type": "string",
    "enum": [
    "front",
    "back",
    "both"
    ]
    ,
    "description": "camera_snap（拍照）: front（前置）/back（后置）/both（双摄）；camera_clip（录像）: 仅支持 front（前置）/back（后置）。"
    }
    ,
    "maxWidth": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "quality": {
    "type": "number",
    "minimum": 0,
    "maximum": 1
    }
    ,
    "delayMs": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "deviceId": {
    "type": "string"
    }
    ,
    "limit": {
    "type": "integer",
    "minimum": 1,
    "maximum": 20
    }
    ,
    "duration": {
    "type": "string"
    }
    ,
    "durationMs": {
    "type": "integer",
    "minimum": 1,
    "maximum": 300000
    }
    ,
    "includeAudio": {
    "type": "boolean"
    }
    ,
    "fps": {
    "type": "number",
    "exclusiveMinimum": 0
    }
    ,
    "screenIndex": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "outPath": {
    "type": "string"
    }
    ,
    "maxAgeMs": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "locationTimeoutMs": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "desiredAccuracy": {
    "type": "string",
    "enum": [
    "coarse",
    "balanced",
    "precise"
    ]
    }
    ,
    "notificationAction": {
    "type": "string",
    "enum": [
    "open",
    "dismiss",
    "reply"
    ]
    }
    ,
    "notificationKey": {
    "type": "string"
    }
    ,
    "notificationReplyText": {
    "type": "string"
    }
    ,
    "invokeCommand": {
    "type": "string"
    }
    ,
    "invokeParamsJson": {
    "type": "string"
    }
    ,
    "invokeTimeoutMs": {
    "type": "integer",
    "minimum": 1
    }
    }
  }
}
```