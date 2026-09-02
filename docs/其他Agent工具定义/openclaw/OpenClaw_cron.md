# cron 工具定义
```json
{
  "name": "cron",
  "description": "管理网关定时任务和唤醒事件：提醒、稍后回访、延迟跟进、周期性工作。不要使用 exec sleep/进程轮询来模拟调度。

主定时任务 => 用于心跳的系统事件。隔离定时任务 => `openclaw tasks` 中的后台任务。

操作：
- status: 调度器状态
- list: 列出任务；includeDisabled 为 true 时包含禁用的任务；agentId 过滤器会从会话中自动填充
- get: 获取单个任务；需要 jobId
- add: 创建任务；需要任务对象
- update: 更新任务；需要 jobId 和更新内容
- remove: 删除任务；需要 jobId
- run: 立即触发执行；需要 jobId
- runs: 执行历史；需要 jobId
- wake: 发送唤醒事件；需要 text，可选的 mode

任务结构（用于 add 操作）：
{
  \"name\": \"字符串\",
  \"schedule\": { ... },      // 必需
  \"payload\": { ... },       // 必需
  \"delivery\": { ... },      // 可选，用于 isolated/current/session 的通知，任意目标的 webhook
  \"sessionTarget\": \"main\" | \"isolated\" | \"current\" | \"session:<id>\",
  \"enabled\": true | false   // 默认为 true
}

会话目标选项：
- \"main\": 主会话；要求 payload.kind=\"systemEvent\"
- \"isolated\": 临时独立会话；要求 payload.kind=\"agentTurn\"
- \"current\": 绑定时创建时的当前会话
- \"session:<id>\": 持久化命名会话

默认值：
- payload.kind=\"systemEvent\" → 默认为 \"main\"
- payload.kind=\"agentTurn\" → 默认为 \"isolated\"
当前绑定需要 sessionTarget=\"current\"。

调度类型（schedule.kind）：
- \"at\": 一次性绝对时间
  { \"kind\": \"at\", \"at\": \"<ISO-8601 时间戳>\" }
- \"every\": 周期性间隔
  { \"kind\": \"every\", \"everyMs\": <毫秒数>, \"anchorMs\": <可选的起始毫秒数> }
- \"cron\": 表达式，使用提供的时区，或时区省略时使用网关主机的本地时区
  { \"kind\": \"cron\", \"expr\": \"<cron表达式>\", \"tz\": \"<可选的IANA时区>\" }
  使用本地挂钟时间编写表达式；不要先将请求的本地时间转换为 UTC。
  省略 tz => 使用网关主机的本地时区，而非 UTC。
  例如上海每天下午6点：{ \"kind\": \"cron\", \"expr\": \"0 18 * * *\", \"tz\": \"Asia/Shanghai\" }

对于 \"at\"，不带时区的 ISO 时间戳视为 UTC。

负载类型（payload.kind）：
- \"systemEvent\": 将文本作为系统事件注入
  { \"kind\": \"systemEvent\", \"text\": \"<消息>\" }
- \"agentTurn\": 使用提示词运行代理；仅限 isolated/current/session
  { \"kind\": \"agentTurn\", \"message\": \"<提示词>\", \"model\": \"<可选>\", \"thinking\": \"<可选>\", \"timeoutSeconds\": <可选，0=无超时> }

交付（顶级）：
  { \"mode\": \"none|announce|webhook\", \"channel\": \"<可选>\", \"to\": \"<可选>\", \"threadId\": \"<可选>\", \"bestEffort\": <可选的布尔值> }
  - 省略时 isolated agentTurn 默认值：\"announce\"
  - announce: 发送到聊天频道；仅限 isolated/current/session；可选的 channel/to
  - threadId: 聊天线程/主题 id
  - webhook: 将运行完成事件 POST 到 delivery.to URL
  - 指定聊天/收件人：设置 announce delivery.channel/to；不要在运行内部调用消息工具。

关键约束：
- sessionTarget=\"main\" 要求 payload.kind=\"systemEvent\"
- sessionTarget=\"isolated\" | \"current\" | \"session:xxx\" 要求 payload.kind=\"agentTurn\"
- Webhook: delivery.mode=\"webhook\" 且提供 delivery.to URL。
默认：优先使用 isolated agentTurn 任务，除非用户明确要求绑定当前会话。

受限的定时任务运行：
- 部分 isolated 定时任务运行获得狭窄的自我清理权限：仅限自身 status/list，仅限当前任务的 get/runs，修改操作仅限 remove 当前任务。

唤醒模式（用于 wake 操作）：
- \"next-heartbeat\" 默认：唤醒下一次心跳
- \"now\": 立即唤醒

使用 jobId 作为规范形式；id 也可接受用于兼容。contextMessages (0-10) 将先前的消息添加为任务上下文。",
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
  "list",
  "get",
  "add",
  "update",
  "remove",
  "run",
  "runs",
  "wake"
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
  "includeDisabled": {
  "type": "boolean"
  }
  ,
  "job": {
  "type": "object",
  "properties": {
  "name": {
  "type": "string",
  "description": "任务名称"
  }
  ,
  "schedule": {
  "type": "object",
  "properties": {
  "kind": {
  "type": "string",
  "enum": [
  "at",
  "every",
  "cron"
  ]
  ,
  "description": "调度类型"
  }
  ,
  "at": {
  "type": "string",
  "description": "ISO-8601 时间（kind=at）"
  }
  ,
  "everyMs": {
  "type": "integer",
  "minimum": 1,
  "description": "间隔毫秒数（kind=every）"
  }
  ,
  "anchorMs": {
  "type": "integer",
  "minimum": 0,
  "description": "起始锚点毫秒数（kind=every）"
  }
  ,
  "expr": {
  "type": "string",
  "description": "Cron 表达式，使用指定时区的挂钟时间；不要转换为 UTC。省略 tz 则使用网关主机的本地时区。例如上海每天下午6点：expr \"0 18 * * *\"，tz \"Asia/Shanghai\"。"
  }
  ,
  "tz": {
  "type": "string",
  "description": "用于 cron 挂钟字段的 IANA 时区，例如 \"Asia/Shanghai\"；省略则使用网关主机的本地时区。"
  }
  ,
  "staggerMs": {
  "type": "integer",
  "minimum": 0,
  "description": "抖动毫秒数（kind=cron）"
  }
  }
  ,
  "additionalProperties": true
  }
  ,
  "sessionTarget": {
  "type": "string",
  "description": "main | isolated | current | session:<id>"
  }
  ,
  "wakeMode": {
  "type": "string",
  "enum": [
  "now",
  "next-heartbeat"
  ]
  ,
  "description": "唤醒时机"
  }
  ,
  "payload": {
  "type": "object",
  "properties": {
  "kind": {
  "type": "string",
  "enum": [
  "systemEvent",
  "agentTurn"
  ]
  ,
  "description": "负载类型"
  }
  ,
  "text": {
  "type": "string",
  "description": "systemEvent 文本"
  }
  ,
  "message": {
  "type": "string",
  "description": "agentTurn 提示词"
  }
  ,
  "model": {
  "type": "string",
  "description": "模型覆盖"
  }
  ,
  "thinking": {
  "type": "string",
  "description": "思考模式覆盖"
  }
  ,
  "timeoutSeconds": {
  "type": "number",
  "minimum": 0
  }
  ,
  "lightContext": {
  "type": "boolean"
  }
  ,
  "allowUnsafeExternalContent": {
  "type": "boolean"
  }
  ,
  "fallbacks": {
  "type": "array",
  "items": {
  "type": "string"
  }
  ,
  "description": "备用模型列表"
  }
  ,
  "toolsAllow": {
  "type": "array",
  "items": {
  "type": "string"
  }
  ,
  "description": "允许使用的工具"
  }
  }
  ,
  "additionalProperties": true
  }
  ,
  "delivery": {
  "type": "object",
  "properties": {
  "mode": {
  "type": "string",
  "enum": [
  "none",
  "announce",
  "webhook"
  ]
  ,
  "description": "交付模式"
  }
  ,
  "channel": {
  "type": "string",
  "description": "交付频道"
  }
  ,
  "to": {
  "type": "string",
  "description": "交付目标"
  }
  ,
  "threadId": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "number"
  }
  ]
  ,
  "description": "线程/主题 id"
  }
  ,
  "bestEffort": {
  "type": "boolean"
  }
  ,
  "accountId": {
  "type": "string",
  "description": "交付账户"
  }
  ,
  "failureDestination": {
  "type": "object",
  "properties": {
  "channel": {
  "type": "string",
  "description": "失败交付频道"
  }
  ,
  "to": {
  "type": "string",
  "description": "失败交付目标"
  }
  ,
  "accountId": {
  "type": "string",
  "description": "失败交付账户"
  }
  ,
  "mode": {
  "anyOf": [
  {
  "type": "string",
  "const": "announce"
  }
  ,
  {
  "type": "string",
  "const": "webhook"
  }
  ]
  }
  }
  ,
  "additionalProperties": true
  }
  }
  ,
  "additionalProperties": true
  }
  ,
  "agentId": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "代理 id，或设为 null 以保持未设置状态"
  }
  ,
  "description": {
  "type": "string",
  "description": "人类可读描述"
  }
  ,
  "enabled": {
  "type": "boolean"
  }
  ,
  "deleteAfterRun": {
  "type": "boolean",
  "description": "首次运行后删除"
  }
  ,
  "sessionKey": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "显式会话键，或设为 null 以清除"
  }
  ,
  "failureAlert": {
  "type": "object",
  "properties": {
  "after": {
  "type": "integer",
  "minimum": 1,
  "description": "触发告警前的失败次数"
  }
  ,
  "channel": {
  "type": "string",
  "description": "告警频道"
  }
  ,
  "to": {
  "type": "string",
  "description": "告警目标"
  }
  ,
  "cooldownMs": {
  "type": "integer",
  "minimum": 0,
  "description": "告警冷却毫秒数"
  }
  ,
  "includeSkipped": {
  "type": "boolean",
  "description": "跳过的运行是否计入告警计数"
  }
  ,
  "mode": {
  "type": "string",
  "enum": [
  "announce",
  "webhook"
  ]
  }
  ,
  "accountId": {
  "type": "string"
  }
  }
  ,
  "additionalProperties": true,
  "description": "失败告警对象；false 表示禁用告警"
  }
  }
  ,
  "additionalProperties": true
  }
  ,
  "jobId": {
  "type": "string"
  }
  ,
  "id": {
  "type": "string"
  }
  ,
  "patch": {
  "type": "object",
  "properties": {
  "name": {
  "type": "string",
  "description": "任务名称"
  }
  ,
  "schedule": {
  "type": "object",
  "properties": {
  "kind": {
  "type": "string",
  "enum": [
  "at",
  "every",
  "cron"
  ]
  ,
  "description": "调度类型"
  }
  ,
  "at": {
  "type": "string",
  "description": "ISO-8601 时间（kind=at）"
  }
  ,
  "everyMs": {
  "type": "integer",
  "minimum": 1,
  "description": "间隔毫秒数（kind=every）"
  }
  ,
  "anchorMs": {
  "type": "integer",
  "minimum": 0,
  "description": "起始锚点毫秒数（kind=every）"
  }
  ,
  "expr": {
  "type": "string",
  "description": "Cron 表达式，使用指定时区的挂钟时间；不要转换为 UTC。省略 tz 则使用网关主机的本地时区。例如上海每天下午6点：expr \"0 18 * * *\"，tz \"Asia/Shanghai\"。"
  }
  ,
  "tz": {
  "type": "string",
  "description": "用于 cron 挂钟字段的 IANA 时区，例如 \"Asia/Shanghai\"；省略则使用网关主机的本地时区。"
  }
  ,
  "staggerMs": {
  "type": "integer",
  "minimum": 0,
  "description": "抖动毫秒数（kind=cron）"
  }
  }
  ,
  "additionalProperties": true
  }
  ,
  "sessionTarget": {
  "type": "string",
  "description": "会话目标"
  }
  ,
  "wakeMode": {
  "type": "string",
  "enum": [
  "now",
  "next-heartbeat"
  ]
  }
  ,
  "payload": {
  "type": "object",
  "properties": {
  "kind": {
  "type": "string",
  "enum": [
  "systemEvent",
  "agentTurn"
  ]
  ,
  "description": "负载类型"
  }
  ,
  "text": {
  "type": "string",
  "description": "systemEvent 文本"
  }
  ,
  "message": {
  "type": "string",
  "description": "agentTurn 提示词"
  }
  ,
  "model": {
  "type": "string",
  "description": "模型覆盖"
  }
  ,
  "thinking": {
  "type": "string",
  "description": "思考模式覆盖"
  }
  ,
  "timeoutSeconds": {
  "type": "number",
  "minimum": 0
  }
  ,
  "lightContext": {
  "type": "boolean"
  }
  ,
  "allowUnsafeExternalContent": {
  "type": "boolean"
  }
  ,
  "fallbacks": {
  "type": "array",
  "items": {
  "type": "string"
  }
  ,
  "description": "备用模型列表"
  }
  ,
  "toolsAllow": {
  "anyOf": [
  {
  "type": "array",
  "items": {
  "type": "string"
  }
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "允许使用的工具 id，或设为 null 以清除"
  }
  }
  ,
  "additionalProperties": true
  }
  ,
  "delivery": {
  "type": "object",
  "properties": {
  "mode": {
  "type": "string",
  "enum": [
  "none",
  "announce",
  "webhook"
  ]
  ,
  "description": "交付模式"
  }
  ,
  "channel": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "交付频道，或设为 null 以清除"
  }
  ,
  "to": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "交付目标，或设为 null 以清除"
  }
  ,
  "threadId": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "number"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "线程/主题 id"
  }
  ,
  "bestEffort": {
  "type": "boolean"
  }
  ,
  "accountId": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "交付账户，或设为 null 以清除"
  }
  ,
  "failureDestination": {
  "anyOf": [
  {
  "type": "object",
  "properties": {
  "channel": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "失败交付频道，或设为 null 以清除"
  }
  ,
  "to": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "失败交付目标，或设为 null 以清除"
  }
  ,
  "accountId": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "失败交付账户，或设为 null 以清除"
  }
  ,
  "mode": {
  "anyOf": [
  {
  "type": "string",
  "const": "announce"
  }
  ,
  {
  "type": "string",
  "const": "webhook"
  }
  ,
  {
  "type": "null"
  }
  ]
  }
  }
  ,
  "additionalProperties": true
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "失败交付目标，或设为 null 以清除"
  }
  }
  ,
  "additionalProperties": true
  }
  ,
  "description": {
  "type": "string"
  }
  ,
  "enabled": {
  "type": "boolean"
  }
  ,
  "deleteAfterRun": {
  "type": "boolean"
  }
  ,
  "agentId": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "代理 id，或设为 null 以清除"
  }
  ,
  "sessionKey": {
  "anyOf": [
  {
  "type": "string"
  }
  ,
  {
  "type": "null"
  }
  ]
  ,
  "description": "显式会话键，或设为 null 以清除"
  }
  ,
  "failureAlert": {
  "type": "object",
  "properties": {
  "after": {
  "type": "integer",
  "minimum": 1,
  "description": "触发告警前的失败次数"
  }
  ,
  "channel": {
  "type": "string",
  "description": "告警频道"
  }
  ,
  "to": {
  "type": "string",
  "description": "告警目标"
  }
  ,
  "cooldownMs": {
  "type": "integer",
  "minimum": 0,
  "description": "告警冷却毫秒数"
  }
  ,
  "includeSkipped": {
  "type": "boolean",
  "description": "跳过的运行是否计入告警计数"
  }
  ,
  "mode": {
  "type": "string",
  "enum": [
  "announce",
  "webhook"
  ]
  }
  ,
  "accountId": {
  "type": "string"
  }
  }
  ,
  "additionalProperties": true,
  "description": "失败告警对象；false 表示禁用告警"
  }
  }
  ,
  "additionalProperties": true
  }
  ,
  "text": {
  "type": "string"
  }
  ,
  "mode": {
  "type": "string",
  "enum": [
  "now",
  "next-heartbeat"
  ]
  }
  ,
  "runMode": {
  "type": "string",
  "enum": [
  "due",
  "force"
  ]
  }
  ,
  "contextMessages": {
  "type": "integer",
  "minimum": 0,
  "maximum": 10
  }
  ,
  "agentId": {
  "type": "string",
  "description": "列表过滤器：代理 id"
  }
  }
  ,
  "additionalProperties": true
  }
}
```