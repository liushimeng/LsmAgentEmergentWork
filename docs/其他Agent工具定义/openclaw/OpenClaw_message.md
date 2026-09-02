# message 工具定义
```json
{
  "name": "message",
  "description": "发送/删除/管理频道消息。支持的操作：broadcast（广播）、send（发送）。",
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
    "send",
    "broadcast"
    ]
    }
    ,
    "channel": {
    "type": "string"
    }
    ,
    "target": {
    "type": "string",
    "description": "接收者/频道：WhatsApp/Signal 使用 E.164 格式，Telegram 使用 chat id/@username，Discord/Slack/Mattermost 使用 <channelId|user:ID|channel:ID>，iMessage 使用 handle/chat_id"
    }
    ,
    "targets": {
    "type": "array",
    "items": {
    "type": "string",
    "description": "接收者/频道目标列表（格式与 --target 相同）；当目录可用时，接受 ID 或名称。"
    }
    }
    ,
    "accountId": {
    "type": "string"
    }
    ,
    "dryRun": {
    "type": "boolean"
    }
    ,
    "message": {
    "type": "string"
    }
    ,
    "effectId": {
    "type": "string",
    "description": "sendWithEffect 使用的特效 ID 或名称。"
    }
    ,
    "effect": {
    "type": "string",
    "description": "effectId 的别名。"
    }
    ,
    "media": {
    "type": "string",
    "description": "媒体 URL 或路径。data: 协议请使用 buffer 参数。"
    }
    ,
    "filename": {
    "type": "string"
    }
    ,
    "buffer": {
    "type": "string",
    "description": "Base64 编码的附件载荷；支持 data URL。"
    }
    ,
    "contentType": {
    "type": "string"
    }
    ,
    "mimeType": {
    "type": "string"
    }
    ,
    "caption": {
    "type": "string"
    }
    ,
    "attachments": {
    "type": "array",
    "items": {
    "type": "object",
    "properties": {
    "type": {
    "type": "string",
    "enum": [
    "image",
    "audio",
    "video",
    "file"
    ]
    }
    ,
    "media": {
    "type": "string"
    }
    ,
    "name": {
    "type": "string"
    }
    ,
    "mimeType": {
    "type": "string"
    }
    }
    }
    ,
    "description": "结构化附件列表；每个条目使用 media 字段。"
    }
    ,
    "replyTo": {
    "type": "string"
    }
    ,
    "threadId": {
    "type": "string"
    }
    ,
    "asVoice": {
    "type": "boolean"
    }
    ,
    "silent": {
    "type": "boolean"
    }
    ,
    "quoteText": {
    "type": "string",
    "description": "Telegram 回复引用的文本。"
    }
    ,
    "gifPlayback": {
    "type": "boolean"
    }
    ,
    "forceDocument": {
    "type": "boolean",
    "description": "将图片/GIF/视频作为文档发送，以避免压缩。"
    }
    ,
    "asDocument": {
    "type": "boolean",
    "description": "forceDocument 的别名。"
    }
    ,
    "messageId": {
    "type": "string",
    "description": "用于 read/react/edit/delete/pin/unpin 的目标消息 ID。对于类似 reaction 的操作，当可用时默认使用当前接收到的消息 ID。"
    }
    ,
    "message_id": {
    "type": "string",
    "description": "messageId 的 snake_case 别名；默认值相同。"
    }
    ,
    "emoji": {
    "type": "string"
    }
    ,
    "remove": {
    "type": "boolean"
    }
    ,
    "trackToolCalls": {
    "type": "boolean",
    "description": "对于当前消息的 reaction，使被添加 reaction 的消息成为工具进度 reaction 的目标。"
    }
    ,
    "track_tool_calls": {
    "type": "boolean",
    "description": "trackToolCalls 的 snake_case 别名。"
    }
    ,
    "targetAuthor": {
    "type": "string"
    }
    ,
    "targetAuthorUuid": {
    "type": "string"
    }
    ,
    "groupId": {
    "type": "string"
    }
    ,
    "limit": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "pageSize": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "pageToken": {
    "type": "string"
    }
    ,
    "before": {
    "type": "string"
    }
    ,
    "after": {
    "type": "string"
    }
    ,
    "around": {
    "type": "string"
    }
    ,
    "fromMe": {
    "type": "boolean"
    }
    ,
    "includeArchived": {
    "type": "boolean"
    }
    ,
    "pollId": {
    "type": "string"
    }
    ,
    "pollOptionId": {
    "type": "string",
    "description": "投票选项 ID。"
    }
    ,
    "pollOptionIds": {
    "type": "array",
    "items": {
    "type": "string",
    "description": "用于多选的投票选项 ID 列表。"
    }
    }
    ,
    "pollOptionIndex": {
    "type": "integer",
    "minimum": 1,
    "description": "投票选项编号（从 1 开始）。"
    }
    ,
    "pollOptionIndexes": {
    "type": "array",
    "items": {
    "type": "integer",
    "minimum": 1,
    "description": "用于多选的投票选项编号列表（从 1 开始）。"
    }
    }
    ,
    "pollQuestion": {
    "type": "string"
    }
    ,
    "pollOption": {
    "type": "array",
    "items": {
    "type": "string"
    }
    }
    ,
    "pollDurationHours": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "pollMulti": {
    "type": "boolean"
    }
    ,
    "channelId": {
    "type": "string",
    "description": "频道 ID 过滤器。"
    }
    ,
    "chatId": {
    "type": "string",
    "description": "用于获取聊天元数据的聊天 ID。"
    }
    ,
    "channelIds": {
    "type": "array",
    "items": {
    "type": "string",
    "description": "频道 ID 过滤器列表。"
    }
    }
    ,
    "memberId": {
    "type": "string"
    }
    ,
    "memberIdType": {
    "type": "string"
    }
    ,
    "guildId": {
    "type": "string"
    }
    ,
    "userId": {
    "type": "string"
    }
    ,
    "openId": {
    "type": "string"
    }
    ,
    "unionId": {
    "type": "string"
    }
    ,
    "authorId": {
    "type": "string"
    }
    ,
    "authorIds": {
    "type": "array",
    "items": {
    "type": "string"
    }
    }
    ,
    "roleId": {
    "type": "string"
    }
    ,
    "roleIds": {
    "type": "array",
    "items": {
    "type": "string"
    }
    }
    ,
    "participant": {
    "type": "string"
    }
    ,
    "includeMembers": {
    "type": "boolean"
    }
    ,
    "members": {
    "type": "boolean"
    }
    ,
    "scope": {
    "type": "string"
    }
    ,
    "kind": {
    "type": "string"
    }
    ,
    "fileId": {
    "type": "string"
    }
    ,
    "emojiName": {
    "type": "string"
    }
    ,
    "stickerId": {
    "type": "array",
    "items": {
    "type": "string"
    }
    }
    ,
    "stickerName": {
    "type": "string"
    }
    ,
    "stickerDesc": {
    "type": "string"
    }
    ,
    "stickerTags": {
    "type": "string"
    }
    ,
    "threadName": {
    "type": "string"
    }
    ,
    "autoArchiveMin": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "appliedTags": {
    "type": "array",
    "items": {
    "type": "string"
    }
    }
    ,
    "query": {
    "type": "string"
    }
    ,
    "eventName": {
    "type": "string"
    }
    ,
    "eventType": {
    "type": "string"
    }
    ,
    "startTime": {
    "type": "string"
    }
    ,
    "endTime": {
    "type": "string"
    }
    ,
    "desc": {
    "type": "string"
    }
    ,
    "location": {
    "type": "string"
    }
    ,
    "image": {
    "type": "string",
    "description": "活动封面图片的 URL 或路径。"
    }
    ,
    "durationMin": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "until": {
    "type": "string"
    }
    ,
    "reason": {
    "type": "string"
    }
    ,
    "deleteDays": {
    "type": "integer",
    "minimum": 0,
    "maximum": 7
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
    "name": {
    "type": "string"
    }
    ,
    "channelType": {
    "type": "integer",
    "minimum": 0,
    "description": "数字形式的频道类型，例如 Discord。用于避免与 JSON Schema 中的 `type` 发生冲突。"
    }
    ,
    "parentId": {
    "type": "string"
    }
    ,
    "topic": {
    "type": "string"
    }
    ,
    "position": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "nsfw": {
    "type": "boolean"
    }
    ,
    "rateLimitPerUser": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "categoryId": {
    "type": "string"
    }
    ,
    "clearParent": {
    "type": "boolean",
    "description": "当支持时，清除父级/分类。"
    }
    ,
    "activityType": {
    "type": "string",
    "description": "活动类型：playing（正在玩）、streaming（正在直播）、listening（正在听）、watching（正在看）、competing（正在比赛）、custom（自定义）。"
    }
    ,
    "activityName": {
    "type": "string",
    "description": "在侧边栏显示的活动名称；对于 custom 类型将被忽略。"
    }
    ,
    "activityUrl": {
    "type": "string",
    "description": "直播 URL；仅适用于 streaming 类型。"
    }
    ,
    "activityState": {
    "type": "string",
    "description": "状态文本；custom 类型将其用作状态文本。"
    }
    ,
    "status": {
    "type": "string",
    "description": "机器人状态：online（在线）、dnd（请勿打扰）、idle（离开）、invisible（隐身）。"
    }
    }
  }
}
```