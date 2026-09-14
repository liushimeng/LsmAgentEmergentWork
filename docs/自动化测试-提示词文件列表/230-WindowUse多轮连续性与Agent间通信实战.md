# 230 WindowUse 多轮连续性与 Agent 间通信实战

> 编号段 HQ01–HQ10 · 聚焦「laew 第 9 角色 WindowUse Agent 的跨轮上下文连续性 + 第 10 角色新增的 AgentMessage 消息桥」:第 2 轮自动引用第 1 轮窗口 / 操作历史回溯 / 跨应用端到端搬运 / WindowUse↔SubAgent 消息传递 / 状态注入幂等 / 别名记忆 / 多窗口并发隔离 / 失败恢复状态保留 / pending_data 跨 WorkFlow 传递 / 跨 Session 隔离
>
> 与现有维度互补说明:
> - `139-桌面自动化与用户操作回放`(EZ 维)是**录制脚本重放**(单次确定性的回放引擎);本文件测的是**多轮对话驱动的交互**(用户每轮新意图,Agent 跨轮保持上下文)。
> - `228-平铺窗口管理与键盘流桌面工程`(HO 维)是**桌面布局与快捷键流**(静态配置);本文件是**运行时跨轮对话驱动**(动态上下文)。
> - `62-桌面小工具软件开发实战`(FL 维)是**开发**新窗口工具;本文件是**使用**已有窗口做事。
> - `225-网页存档与数字保全工程`(HL 维)归档静态网页;本文件归档窗口会话状态(WindowState)到 SQLite。
> - 既有 `docs/WindowUse桌面窗口操控Agent/prompts/window_use测试提示词.md` 是**单轮**操作验证;本文件是**多轮连续性**(`2ea6a23 WindowUse多轮对话与Agent间通信增强` 新增)与**跨 Agent 消息**(`agent_message.rs` + `agent_messages` 表)。
>
> **本文件独特主题**:`WindowState` 持久化到 SQLite `window_state` 表 / 每 session_id 覆盖更新 / 已知窗口快照(`WindowSnapshot{known_controls,last_inspect_path}`) / 操作历史栈(`WindowActionRecord`) / 用户自定义别名映射 / 待处理数据 `pending_data` / AgentMessage 五元组 `(id,from_role,to_role,payload,consumed)` / 三种 `MessagePayload` 变体(WindowTextRead/ProcessedResult/...)/ 消费标记幂等 / 跨 WorkFlow 数据传递 / WindowUse 失败状态保留策略。

---

### HQ01 跨轮窗口连续性(WindowState 注入核心)

- **预期档位**: medium
- **考察维度**: 第 2 轮系统提示词含上一轮 WindowState / 路径自动复用
- **工具链**: WindowList → WindowAction(多轮)
- **对话脚本**:
  1. [第 1 轮] 模拟打开记事本(用 mock 窗口驱动:WindowList 返回含一个 `notepad.exe PID=1234 title="无标题 - 记事本"` 的 mock),在编辑区输入「第一轮内容」。
  2. [第 2 轮] 继续在那个记事本追加「第二轮追加」。(验证:Agent 不应再要求用户描述「是哪个记事本」;系统提示词里的 `<<<LAEW:WINDOW_STATE>>>` 段应包含该 window_id,且路径可复用)
  3. Read `LsmAgentEmergentWork.db`(或 mock SQLite 文件)断言:`window_state` 表中 `last_window_id` 字段指向刚才那个记事本;`action_history` JSON 数组长度 ≥ 2 且两条记录 `window_id` 相同。

### HQ02 操作历史回溯与失效路径降级

- **预期档位**: medium
- **考察维度**: 路径失效时引用历史而非盲目重试
- **工具链**: WindowAction(成功)→ WindowAction(path 失效)→ 历史回溯 → 重选相近路径
- **对话脚本**:
  1. [第 1 轮] 在记事本里输入「历史内容」(假设第 1 轮操作 path=`/Document/Edit`,已知控件 `[("/File", "MenuItem"), ("/Document/Edit", "Document")]`)。
  2. [第 2 轮] 让 Agent 重新点击 `/Document/Edit`(mock 驱动返回 path 失效错误码 `STALE_PATH`)。Agent 应:(a) 检查 action_history;(b) 重新 WindowInspect 同 window_id;(c) 从新 known_controls 找最近的 Edit 控件(path 可能改为 `/Body/Edit`);(d) 重试。
  3. 断言:Round2 mock 接收序列应为 `[WindowAction(stale), WindowInspect, WindowAction(retry with new path)]` 三步而非盲目三次点击。

### HQ03 跨应用端到端数据搬运

- **预期档位**: hard
- **考察维度**: pending_data 在 window_state 中传递 / 不需要用户复述
- **工具链**: WindowList ×2 → WindowAction(get_text) → WindowAction(set_text)
- **对话脚本**:
  1. [第 1 轮] 同时打开两个记事本(A 和 B),在 A 中写入「源数据」。
  2. [第 2 轮] 「把刚才 A 里的内容复制到 B」。(验证:第 2 轮应自动 `WindowInspect(A)` + `get_text` 得到「源数据」存入 `pending_data`,再 `WindowInspect(B)` + `set_text(pending_data)`,全程不要求用户复述「源数据是什么」)
  3. 断言 mock 接收序列:`[WindowList, WindowInspect(A), WindowAction(A,set_text,「源数据」), WindowList, WindowInspect(A), WindowAction(A,get_text), WindowInspect(B), WindowAction(B,set_text,「源数据」)]` —— get_text 与 set_text 之间由 pending_data 衔接。

### HQ04 WindowUse→SubAgent 消息传递(AgentMessage)

- **预期档位**: hard
- **考察维度**: WindowUse 读到的文本通过 AgentMessage 转发 SubAgent 处理
- **工具链**: WindowAction(get_text) → AgentMessage(WindowTextRead) → SubAgent(分析) → AgentMessage(ProcessedResult) → WindowAction(set_text 回到原窗口)
- **对话脚本**:
  1. [第 1 轮] 让 Agent 从记事本读取一段 JSON 文本(假设 mock 内容为 `{"name":"laew","version":3}`),并让 SubAgent 计算字段数。
  2. [第 2 轮] 让 Agent 把「刚才算出的字段数」回写到记事本末尾。
  3. 断言:round1 mock 应观察 WindowUse 调 get_text → 之后 AgentMessage 表新增 `(from_role=WindowUse, to_role=SubAgentWork, payload=WindowTextRead{content:"{\"name\":\"laew\",...}"})` 一行;SubAgent 处理完后产生 `(from_role=SubAgentWork, to_role=WindowUse, payload=ProcessedResult{value:"2"})`;round2 的 WindowUse set_text 接收的参数应为 `ProcessedResult` 解包后的字符串「2」。

### HQ05 状态注入幂等

- **预期档位**: medium
- **考察维度**: 多轮多次操作后 `<<<LAEW:WINDOW_STATE>>>` 段只出现一次
- **工具链**: WindowAction(多轮)→ 检查 prompt 模板拼接结果
- **对话脚本**:
  1. [第 1 轮] 让 Agent 列出当前窗口(WindowList)。
  2. [第 2 轮] 再次列出当前窗口。
  3. 抓取 Round2 真实发给 LLM 的 system prompt(可借助 `-debug` 调试模式的 trace),断言 `<<<LAEW:WINDOW_STATE>>>` 段在整份 prompt 中恰好出现一次,`action_history` JSON 数组元素数量比 round1 多但不存在「重复段」。

### HQ06 窗口别名跨轮记忆

- **预期档位**: medium
- **考察维度**: `WindowState.aliases` HashMap 持久化跨轮
- **工具链**: WindowList → WindowInspect → WindowAction(用别名定位)
- **对话脚本**:
  1. [第 1 轮] 「把标题含『记事本』的窗口记为『我的记事本』,告诉我它的控件列表」。(mock 应记录 `aliases={"我的记事本": <window_id>}`)
  2. [第 2 轮] 「在『我的记事本』里输入「别名定位成功」」。
  3. 断言:round2 mock 收到的 WindowAction 参数中 window_id 应等于 round1 aliases 中「我的记事本」对应的 window_id(非「最新窗口」或「title 模糊匹配」)。

### HQ07 多窗口并发操作状态隔离

- **预期档位**: hard
- **考察维度**: `window_state` 中多窗口 state 按 window_id 分桶 / 互不覆盖
- **工具链**: WindowAction(A) + WindowAction(B) 同轮并发 → 验证 state 写回完整
- **对话脚本**:
  1. [第 1 轮] 「在窗口 A(记事本)输入「A 数据」,在窗口 B(计算器)点击「1」」。
  2. [第 2 轮] 「在窗口 A 追加「B 数据」,在窗口 B 点击「+」「2」「=」」。
  3. 断言 round2 写入 SQLite 的 `window_state.known_windows` JSON 应包含两条记录(window_id A 与 window_id B),各自动作历史条目独立累加,不存在「A 的最后操作覆盖 B 的最后操作」。

### HQ08 失败操作后成功状态保留

- **预期档位**: medium
- **考察维度**: 单条 WindowAction 失败不污染前序成功条目
- **工具链**: WindowAction(成功)→ WindowAction(失败)→ WindowAction(继续)
- **对话脚本**:
  1. [第 1 轮] 在记事本输入「第一步成功」。
  2. [第 2 轮] 让 Agent 点击一个不存在的 path(返回 NOT_FOUND),然后再输入「第二步成功」。
  3. 断言:`action_history` 数组应保留 3 条记录,中间那条 `result_summary="NOT_FOUND"` 但 timestamp 居中;前后的两条 `result_summary` 不应被清空。

### HQ09 待处理数据跨 WorkFlow 传递

- **预期档位**: hard
- **考察维度**: `pending_data` 从 Round1 窗口读取后,在 Round2 WorkFlow 执行时仍可消费
- **工具链**: WindowAction(get_text) → 跨轮状态保持 → WindowAction(set_text)
- **对话脚本**:
  1. [第 1 轮] 「从源窗口读取这段文本并告诉我内容」。
  2. [第 2 轮] 「打开目标窗口,把刚才那段文本原样写入」(目标窗口 round2 才出现)。
  3. 断言:`window_state.pending_data` 字段在 round1 get_text 后非空,跨 round2 仍非空,直到 set_text 消费后才清空;且 set_text 的参数内容与 round1 get_text 的返回内容字节一致。

### HQ10 跨 Session 状态隔离

- **预期档位**: medium
- **考察维度**: 不同 Session ID 的 window_state 互不污染
- **工具链**: WindowAction(Session A) → WindowAction(Session B) → WindowAction(Session A 再次)
- **对话脚本**:
  1. [Session A 第 1 轮] 输入「SessionA 数据」。
  2. [Session B 第 1 轮] 输入「SessionB 数据」。
  3. [Session A 第 2 轮] 读取内容。
  4. 断言:Session A 的 `window_state.session_id` 与 Session B 不同;Session A 第 2 轮读取的内容是「SessionA 数据」,看不到 Session B 的「SessionB 数据」;SQL 查询 `SELECT session_id, COUNT(*) FROM window_state GROUP BY session_id` 至少返回两行不同 session_id。