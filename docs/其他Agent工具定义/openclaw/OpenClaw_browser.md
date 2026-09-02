# browser 工具定义
```json
{
  "name": "browser",
  "description": "通过 OpenClaw 的浏览器控制服务器控制浏览器（status/start/stop/profiles/tabs/open/snapshot/screenshot/actions）。浏览器选择：默认省略 profile 参数以使用隔离的由 OpenClaw 管理的浏览器（`openclaw`）。若要使用已登录用户的浏览器，请使用 profile=\"user\"。所选主机或浏览器节点上必须运行受支持的基于 Chromium 的浏览器（v144+）。仅在现有登录状态/Cookie 至关重要且用户在场时使用。对于 profile=\"user\" 或其他现有会话配置，在执行 act:type、evaluate、hover、scrollIntoView、drag、select 和 fill 时请省略 timeoutMs；该驱动程序会拒绝针对这些操作的单次调用超时覆盖。当节点托管的浏览器代理可用时，工具可能会自动路由至该代理。使用 node=<id|name> 或 target=\"node\" 来固定节点。使用来自快照的引用（例如 e12）时，请保持同一标签页：建议将快照响应中的 targetId 传递给后续操作（act/click/type 等）。对于标签页操作，targetId 也接受 tabId 句柄（t1）以及 action=tabs 返回的标签。对于多步骤浏览器操作、登录检查、过期引用、重复标签页或 Google Meet 流程，请在可用时使用内置的 browser-automation 技能。若需在多次调用间获得稳定且可自动解析的引用，请使用带有 refs=\"aria\" 的快照（Playwright aria-ref ids）。默认的 refs=\"role\" 是基于 role+name 的。使用 snapshot+act 进行 UI 自动化。默认情况下避免使用 act:wait；仅在不存在可靠 UI 状态的特殊情况下使用。target 用于选择浏览器位置（sandbox|host|node）。默认值：host。允许使用 Host target。",
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
    "doctor",
    "status",
    "start",
    "stop",
    "profiles",
    "tabs",
    "open",
    "focus",
    "close",
    "snapshot",
    "screenshot",
    "navigate",
    "console",
    "pdf",
    "upload",
    "dialog",
    "act"
    ]
    }
    ,
    "target": {
    "type": "string",
    "enum": [
    "sandbox",
    "host",
    "node"
    ]
    }
    ,
    "node": {
    "type": "string"
    }
    ,
    "profile": {
    "type": "string"
    }
    ,
    "targetUrl": {
    "type": "string"
    }
    ,
    "url": {
    "type": "string"
    }
    ,
    "targetId": {
    "type": "string",
    "description": "标签页引用。优先使用 tabs 输出中的 suggestedTargetId、tabId 或 label；为保持兼容性，仍支持原始 CDP targetId 和唯一的原始前缀。"
    }
    ,
    "label": {
    "type": "string"
    }
    ,
    "limit": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "maxChars": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "mode": {
    "type": "string",
    "enum": [
    "efficient"
    ]
    }
    ,
    "snapshotFormat": {
    "type": "string",
    "enum": [
    "aria",
    "ai"
    ]
    }
    ,
    "refs": {
    "type": "string",
    "enum": [
    "role",
    "aria"
    ]
    }
    ,
    "interactive": {
    "type": "boolean"
    }
    ,
    "compact": {
    "type": "boolean"
    }
    ,
    "depth": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "selector": {
    "type": "string"
    }
    ,
    "frame": {
    "type": "string"
    }
    ,
    "labels": {
    "type": "boolean"
    }
    ,
    "urls": {
    "type": "boolean"
    }
    ,
    "fullPage": {
    "type": "boolean"
    }
    ,
    "ref": {
    "type": "string"
    }
    ,
    "element": {
    "type": "string"
    }
    ,
    "type": {
    "type": "string",
    "enum": [
    "png",
    "jpeg"
    ]
    }
    ,
    "level": {
    "type": "string"
    }
    ,
    "paths": {
    "type": "array",
    "items": {
    "type": "string"
    }
    }
    ,
    "inputRef": {
    "type": "string"
    }
    ,
    "timeoutMs": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "dialogId": {
    "type": "string"
    }
    ,
    "accept": {
    "type": "boolean"
    }
    ,
    "promptText": {
    "type": "string"
    }
    ,
    "kind": {
    "type": "string",
    "enum": [
    "click",
    "clickCoords",
    "type",
    "press",
    "hover",
    "drag",
    "select",
    "fill",
    "resize",
    "wait",
    "evaluate",
    "close"
    ]
    }
    ,
    "doubleClick": {
    "type": "boolean"
    }
    ,
    "button": {
    "type": "string"
    }
    ,
    "modifiers": {
    "type": "array",
    "items": {
    "type": "string"
    }
    }
    ,
    "x": {
    "type": "number"
    }
    ,
    "y": {
    "type": "number"
    }
    ,
    "text": {
    "type": "string"
    }
    ,
    "submit": {
    "type": "boolean"
    }
    ,
    "slowly": {
    "type": "boolean"
    }
    ,
    "key": {
    "type": "string"
    }
    ,
    "delayMs": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "startRef": {
    "type": "string"
    }
    ,
    "endRef": {
    "type": "string"
    }
    ,
    "values": {
    "type": "array",
    "items": {
    "type": "string"
    }
    }
    ,
    "fields": {
    "type": "array",
    "items": {
    "type": "object",
    "properties": {},
    "additionalProperties": true
    }
    }
    ,
    "width": {
    "type": "integer",
    "minimum": 1,
    "maximum": 8192
    }
    ,
    "height": {
    "type": "integer",
    "minimum": 1,
    "maximum": 8192
    }
    ,
    "timeMs": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "textGone": {
    "type": "string"
    }
    ,
    "loadState": {
    "type": "string"
    }
    ,
    "fn": {
    "type": "string"
    }
    ,
    "request": {
    "type": "object",
    "required": [
    "kind"
    ]
    ,
    "properties": {
    "kind": {
    "type": "string",
    "enum": [
    "click",
    "clickCoords",
    "type",
    "press",
    "hover",
    "drag",
    "select",
    "fill",
    "resize",
    "wait",
    "evaluate",
    "close"
    ]
    }
    ,
    "targetId": {
    "type": "string",
    "description": "标签页引用。优先使用 tabs 输出中的 suggestedTargetId、tabId 或 label；为保持兼容性，仍支持原始 CDP targetId 和唯一的原始前缀。"
    }
    ,
    "ref": {
    "type": "string"
    }
    ,
    "doubleClick": {
    "type": "boolean"
    }
    ,
    "button": {
    "type": "string"
    }
    ,
    "modifiers": {
    "type": "array",
    "items": {
    "type": "string"
    }
    }
    ,
    "x": {
    "type": "number"
    }
    ,
    "y": {
    "type": "number"
    }
    ,
    "text": {
    "type": "string"
    }
    ,
    "submit": {
    "type": "boolean"
    }
    ,
    "slowly": {
    "type": "boolean"
    }
    ,
    "key": {
    "type": "string"
    }
    ,
    "delayMs": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "startRef": {
    "type": "string"
    }
    ,
    "endRef": {
    "type": "string"
    }
    ,
    "values": {
    "type": "array",
    "items": {
    "type": "string"
    }
    }
    ,
    "fields": {
    "type": "array",
    "items": {
    "type": "object",
    "properties": {},
    "additionalProperties": true
    }
    }
    ,
    "width": {
    "type": "integer",
    "minimum": 1,
    "maximum": 8192
    }
    ,
    "height": {
    "type": "integer",
    "minimum": 1,
    "maximum": 8192
    }
    ,
    "timeMs": {
    "type": "integer",
    "minimum": 0
    }
    ,
    "selector": {
    "type": "string"
    }
    ,
    "url": {
    "type": "string"
    }
    ,
    "loadState": {
    "type": "string"
    }
    ,
    "textGone": {
    "type": "string"
    }
    ,
    "timeoutMs": {
    "type": "integer",
    "minimum": 1
    }
    ,
    "fn": {
    "type": "string"
    }
    }
    }
    }
  }
}
```