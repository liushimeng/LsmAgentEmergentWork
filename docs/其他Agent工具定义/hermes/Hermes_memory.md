# memory 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "memory",
    "description": "将持久性事实保存到跨会话保留的持久化记忆中。记忆会被注入到未来的每一轮对话中，因此请保持条目紧凑且具有高信息密度。\n\n【操作方法】：通过 'operations' 数组在【单次】调用中完成【所有】更改（每个元素格式为：{action, content?, old_text?}）。批量操作是原子性执行的，且字符数限制仅针对【最终】结果进行检查——因此，单次调用即可在添加新条目的同时，移除/替换过时的条目以腾出空间，即使单独添加会导致溢出。响应会报告当前字符数/限制字符数并确认完成；一次批量调用即可完成更新，请勿重复调用。仅在需要进行单次独立更改时，才使用单独的 action/content/old_text 字段。\n\n【使用时机】：当用户表达偏好、更正或个人信息，或者您了解到关于其环境、惯例或工作流的稳定事实时，请主动保存。优先级：用户偏好与更正 > 环境事实 > 操作流程。最好的记忆是能让用户免于重复自己。\n\n【如果已满】：添加操作将被拒绝，并会显示当前条目。请重新发起【单次】批量调用，移除或缩短足够多的过时条目，并同时添加新条目。\n\n【目标对象】：'user' = 用户是谁（姓名、角色、偏好、风格）。'memory' = 您的笔记（环境、惯例、工具特性、经验教训）。\n\n【跳过以下情况】：琐碎/显而易见的信息、容易重新发现的事实、原始数据转储、任务进度、已完成工作的日志、临时的 TODO 状态（这些请使用 session_search）。可复用的操作流程应保存在技能（skill）中，而不是记忆中。",
    "parameters": {
      "type": "object",
      "properties": {
        "action": {
          "type": "string",
          "enum": [
          "add",
          "replace",
          "remove"
          ]
          ,
          "description": "要执行的操作（单次操作格式）。当使用 'operations' 数组时请省略此字段。"
        },
        "target": {
          "type": "string",
          "enum": [
          "memory",
          "user"
          ]
          ,
          "description": "记忆存储区：'memory' 用于个人笔记，'user' 用于用户画像。"
        },
        "content": {
        "type": "string",
        "description": "条目内容。在执行 'add' 和 'replace'（单次操作格式）时为必填项。"
        },
        "old_text": {
        "type": "string",
        "description": "在执行 'replace' 和 'remove'（单次操作格式）时为必填项：用于标识要修改的现有条目的简短唯一子字符串。仅在 'add' 操作时省略。"
        },
        "operations": {
          "type": "array",
          "description": "批量操作格式：在单次调用中针对最终字符预算原子性应用的操作列表。当进行多项更改或需要整合以腾出空间时优先使用。每个元素的格式为 {action, content?, old_text?}。",
          "items": {
            "type": "object",
            "properties": {
              "action": {
              "type": "string",
              "enum": [
              "add",
              "replace",
              "remove"
              ]
              },
              "content": {
              "type": "string",
              "description": "用于 add/replace 操作的条目内容。"
              },
              "old_text": {
              "type": "string",
              "description": "用于标识 replace/remove 操作条目的子字符串。"
              }
            },
            "required": [
            "action"
            ]
          }
        }
      }
      ,
      "required": [
      "target"
      ]
    }
  }
}
```