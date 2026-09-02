# session_search 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "session_search",
    "description": "搜索存储在本地会话数据库中的历史会话，或在某个会话内部进行滚动查看。基于 SQLite 消息存储的 FTS5 全文检索。不涉及任何大语言模型（LLM）调用——所有调用方式均直接从数据库返回实际消息。

    【来源优先原则】

    此工具仅搜索 Hermes 的对话历史记录。它不能作为外部来源当前内容的证据。如果用户提供了直接来源（如 URL、电话号码/联系人、应用/线程、文件路径、账户、网站或实时系统），在可访问的情况下，请先于或代替 session_search 检查该原始来源。请将 session_search 作为了解“之前说过什么”的辅助上下文，而不是作为“来源当前包含什么”的主要证据。如果原始来源无法访问，在回退到会话历史之前，请先说明无法访问及其原因。当用户提供了直接来源时，切勿仅凭 session_search 就得出“未找到”或“没有先前通信”的结论。

    【四种调用方式】

    1) 发现（DISCOVERY）—— 传入 query：
    session_search(query=auth refactor, limit=3)
    执行 FTS5 检索，按会话谱系对命中项进行去重，返回前 N 个会话。每个结果包含：
    - session_id（会话ID）、title（标题）、when（时间）、source（来源）
    - snippet（摘要）：FTS5 高亮匹配的摘录
    - bookend_start（开头标记）：会话的前 3 条 user+assistant 消息（目标/启动阶段）
    - messages（消息）：FTS5 匹配项前后的 ±5 条消息，并标记锚点消息（上下文中的命中项）
    - bookend_end（结尾标记）：会话的最后 3 条 user+assistant 消息（解决方案/决定阶段）
    - match_message_id（匹配消息ID）、messages_before（前序消息）、messages_after（后续消息）
    开头/结尾标记与窗口结合，使您能够重构“目标 → 匹配 → 解决方案”的过程，而无需加载完整的对话记录。

    2) 滚动（SCROLL）—— 传入 session_id + around_message_id：
    session_search(session_id=..., around_message_id=12345, window=10)
    返回以锚点为中心的 ±window 条消息窗口。不使用 FTS5，无开头/结尾标记——仅返回切片。在发现调用之后，当您需要比默认 ±5 窗口更多的上下文时使用。
    - 向前滚动：将 messages[-1].id 传回作为 around_message_id。
    - 向后滚动：将 messages[0].id 传回作为 around_message_id。
    - 边界消息会同时出现在两个窗口中，作为方向标记。
    - 当 messages_before 或 messages_after 小于 window 时，表示您已到达会话的开头或结尾。

    3) 读取（READ）—— 仅传入 session_id（不传 around_message_id）：
    session_search(session_id=..., profile=work)
    按 ID 转储整个会话（对于大型会话，返回前 20 条 + 最后 10 条消息）。这是解析用户放入聊天中的 @session:<profile>/<id> 链接的方式：将值按 / 拆分为 profile 和 id，然后调用 session_search(session_id=id, profile=profile)。

    4) 浏览（BROWSE）—— 不传参数：
    session_search()
    按时间顺序返回最近的会话：标题、预览和时间戳。当用户询问“我之前在做什么”但没有指明具体主题时使用。

    【FTS5 语法】

    默认使用 AND 逻辑——多词查询要求所有词均匹配。如需更广泛的召回，请显式使用 OR（alpha OR beta OR gamma）、精确匹配的引号短语（\"docker networking\"）、布尔逻辑（python NOT java）或前缀通配符（deploy*）。

    【使用时机】

    当遇到关于 Hermes 对话历史本身的问题时使用此工具，例如“我们关于 X 做了什么”、“我们把 Y 搁置在哪了”或“找到我们讨论 Z 的会话”。如果用户提供了直接来源标识符，在可访问的情况下请先检查该来源；然后 session_search 可以提供历史上下文。会话数据库记录了“何时说了什么”；而外部工具则展示当前来源/世界的状态。",
    "parameters": {
      "type": "object",
      "properties": {
        "query": {
        "type": "string",
        "description": "搜索查询（发现模式）。用于在过去会话中查找的关键词、短语或布尔表达式。省略此参数以浏览最近的会话。当设置了 session_id + around_message_id（滚动模式）时，此参数将被忽略。"
        }
        ,
        "limit": {
        "type": "integer",
        "description": "仅限发现模式。返回的最大会话数（默认 3，最大 10）。当主题可能跨越多个会话，且您希望挑选正确的会话进行滚动查看时，建议调高至 5–10。",
        "default": 3
        }
        ,
        "sort": {
        "type": "string",
        "enum": [
        "newest",
        "oldest"
        ]
        ,
        "description": "仅限发现模式。在 FTS5 排名之上的时间偏好。省略则保持仅按相关性排序（适合探索性回忆——“关于 X 我们知道什么”）。设置为 'newest' 用于近期偏好问题（“我们把 X 搁置在哪了”）。设置为 'oldest' 用于起源偏好问题（“X 是怎么开始的”）。在滚动和浏览模式中将被忽略。"
        }
        ,
        "session_id": {
        "type": "string",
        "description": "滚动模式。要读取的内部会话。使用先前发现调用返回的 session_id。必须与 around_message_id 配合使用。"
        }
        ,
        "around_message_id": {
        "type": "integer",
        "description": "滚动模式。窗口居中的消息 ID。从发现结果中可使用 match_message_id，或先前窗口中看到的任何 ID。向前滚动请传入最后一个窗口消息的 ID；向后滚动请传入第一个窗口消息的 ID。"
        }
        ,
        "window": {
        "type": "integer",
        "description": "仅限滚动模式。锚点两侧返回的消息数（锚点本身始终包含在内）。限制在 [1, 20] 之间。默认值为 5。",
        "default": 5
        }
        ,
        "role_filter": {
        "type": "string",
        "description": "可选。要包含的逗号分隔的角色列表。发现模式默认为 'user,assistant'（工具输出通常是噪声）。传入 'user,assistant,tool' 以包含工具输出（用于调试工具行为），或传入 'tool' 仅搜索工具输出。"
        }
        ,
        "profile": {
        "type": "string",
        "description": "可选。从另一个 Hermes 配置文件的数据库读取会话（只读）。在解析 @session:<profile>/<id> 链接时使用：将配置文件段传入此参数，并将 session_id 作为 ID 段。省略则使用当前配置文件。"
        }
      }
    }
  }
}
```