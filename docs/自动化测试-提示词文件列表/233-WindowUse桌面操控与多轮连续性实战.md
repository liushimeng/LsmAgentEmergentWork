# 233 WindowUse 桌面操控与多轮连续性实战

> 编号段 HT01–HT10 · 聚焦「laew 第 9 角色 WindowUse Agent 的桌面操控全能力」:WindowList 枚举窗口 / WindowInspect 控件树遍历 / WindowAction 操作执行 / WindowState 跨轮持久化 / 操作历史回溯 / 别名记忆 / pending_data 跨应用传递 / AgentMessage 消息桥 / 多窗口并发隔离 / 失败恢复状态保留
>
> 与现有维度互补说明:
> - `230-WindowUse多轮连续性与Agent间通信实战`(HQ 维)是**多轮连续性 + 消息桥**;本文件是**桌面操控全能力**——测 WindowUse Agent 的窗口枚举、控件遍历、操作执行、状态管理等基础能力。
> - `139-桌面自动化与用户操作回放`(EZ 维)是**录制脚本重放**;本文件是**实时交互操控**。
> - `228-平铺窗口管理与键盘流桌面工程`(HO 维)是**桌面布局配置**;本文件是**运行时窗口操控**。
> - `62-桌面小工具软件开发实战`(FL 维)是**开发新窗口工具**;本文件是**使用已有窗口做事**。
> - `25-桌面应用与跨平台开发`(DH 维)是**桌面应用开发**;本文件是**桌面应用操控**。
>
> **本文件独特主题**:`WindowSnapshot{window_id, title, process_name, pid, last_inspect_path, known_controls}` / `WindowActionRecord{timestamp, window_id, window_title, path, action, result_summary}` / `PendingData{source_window_id, source_path, content, target_window_id, created_at}` / `WindowSessionState{session_id, last_window, action_history, known_windows, window_aliases, pending_data}` / `WindowStateManager` SQLite 持久化 / `AgentMessage` 五元组 `(id, session_id, from_role, to_role, payload, created_at, consumed)` / `MessagePayload` 四变体(WindowTextRead/TextToWindow/WindowFocus/Data) / 平台驱动层(Windows UIA / macOS AX / fallback) / 失败状态保留策略。

---

### HT01 WindowList 枚举与窗口快照构建

- **预期档位**: simple
- **考察维度**: WindowList 工具返回窗口列表 / WindowSnapshot 构建
- **工具链**: WindowList → WindowInspect
- **对话脚本**:
  1. [第 1 轮] 模拟 WindowList 返回 3 个窗口(notepad.exe PID=1234 title="无标题 - 记事本"/calc.exe PID=5678 title="计算器"/explorer.exe PID=9012 title="文件资源管理器")。
  2. 让 Agent 对记事本做 WindowInspect,返回控件树 `{root: [("/Document/Edit","Document"),("/File","MenuItem"),("/Format","MenuItem")]}`。
  3. 断言:Agent 构建的 `WindowSnapshot` 含 `window_id=1234, title="无标题 - 记事本", process_name="notepad.exe", pid=1234, known_controls=[("/Document/Edit","Document"),...]`。

### HT02 WindowInspect 控件树遍历与路径定位

- **预期档位**: medium
- **考察维度**: 控件树结构 / 路径定位 / known_controls 缓存
- **工具链**: WindowInspect → WindowAction
- **对话脚本**:
  1. [第 1 轮] 模拟 WindowInspect 返回多层控件树:`{root: [("/Document/Edit","Document"),("/File",[("/File/New","MenuItem"),("/File/Save","MenuItem")])]}`。
  2. 让 Agent 定位到 `/Document/Edit` 并执行 `type_text("hello")`。
  3. 断言:Agent 正确解析控件树、WindowAction 参数含 `path="/Document/Edit"`、操作后 known_controls 缓存更新。

### HT03 WindowAction 操作执行与结果记录

- **预期档位**: medium
- **考察维度**: WindowAction 工具执行 / WindowActionRecord 落盘
- **工具链**: WindowAction → Read(window_state)
- **对话脚本**:
  1. [第 1 轮] 在记事本执行一系列操作:`type_text("第一行")` → `key("Enter")` → `type_text("第二行")`。
  2. Read `window_state` 表断言:`action_history` JSON 数组长度 = 3,每条含 `window_id/path/action/result_summary` 字段。
  3. 断言:`last_window` 指向该记事本,`updated_at` 时间戳更新。

### HT04 WindowState 跨轮注入与路径复用

- **预期档位**: medium
- **考察维度**: 第 2 轮系统提示词含上一轮 WindowState / 路径自动复用
- **工具链**: WindowAction(第 1 轮) → WindowAction(第 2 轮)
- **对话脚本**:
  1. [第 1 轮] 在记事本输入「第一轮内容」(操作 path=`/Document/Edit`)。
  2. [第 2 轮] 继续在那个记事本追加「第二轮追加」。(验证:Agent 不应再要求用户描述「是哪个记事本」;系统提示词里的 `<<<LAEW:WINDOW_STATE>>>` 段应包含该 window_id,且路径可复用)
  3. Read `LsmAgentEmergentWork.db`(或 mock SQLite 文件)断言:`window_state` 表中 `last_window_id` 字段指向刚才那个记事本;`action_history` JSON 数组长度 ≥ 2 且两条记录 `window_id` 相同。

### HT05 操作历史回溯与失效路径降级

- **预期档位**: medium
- **考察维度**: 路径失效时引用历史而非盲目重试
- **工具链**: WindowAction(成功) → WindowAction(path 失效) → 历史回溯 → 重选相近路径
- **对话脚本**:
  1. [第 1 轮] 在记事本里输入「历史内容」(假设第 1 轮操作 path=`/Document/Edit`,已知控件 `[("/File", "MenuItem"), ("/Document/Edit", "Document")]`)。
  2. [第 2 轮] 让 Agent 重新点击 `/Document/Edit`(mock 驱动返回 path 失效错误码 `STALE_PATH`)。Agent 应:(a) 检查 action_history;(b) 重新 WindowInspect 同 window_id;(c) 从新 known_controls 找最近的 Edit 控件(path 可能改为 `/Body/Edit`);(d) 重试。
  3. 断言:Round2 mock 接收序列应为 `[WindowAction(stale), WindowInspect, WindowAction(retry with new path)]` 三步而非盲目三次点击。

### HT06 跨应用端到端数据搬运

- **预期档位**: hard
- **考察维度**: pending_data 在 window_state 中传递 / 不需要用户复述
- **工具链**: WindowList ×2 → WindowAction(get_text) → WindowAction(set_text)
- **对话脚本**:
  1. [第 1 轮] 同时打开两个记事本(A 和 B),在 A 中写入「源数据」。
  2. [第 2 轮] 「把刚才 A 里的内容复制到 B」。(验证:第 2 轮应自动 `WindowInspect(A)` + `get_text` 得到「源数据」存入 `pending_data`,再 `WindowInspect(B)` + `set_text(pending_data)`,全程不要求用户复述「源数据是什么」)
  3. 断言 mock 接收序列:`[WindowList, WindowInspect(A), WindowAction(A,set_text,「源数据」), WindowList, WindowInspect(A), WindowAction(A,get_text), WindowInspect(B), WindowAction(B,set_text,「源数据」)]` —— get_text 与 set_text 之间由 pending_data 衔接。

### HT07 窗口别名跨轮记忆

- **预期档位**: medium
- **考察维度**: `WindowSessionState.window_aliases` HashMap 持久化跨轮
- **工具链**: WindowList → WindowInspect → WindowAction(用别名定位)
- **对话脚本**:
  1. [第 1 轮] 「把标题含『记事本』的窗口记为『我的记事本』,告诉我它的控件列表」。(mock 应记录 `aliases={"我的记事本": <window_id>}`)
  2. [第 2 轮] 「在『我的记事本』里输入「别名定位成功」」。
  3. 断言:round2 mock 收到的 WindowAction 参数中 window_id 应等于 round1 aliases 中「我的记事本」对应的 window_id(非「最新窗口」或「title 模糊匹配」)。

### HT08 多窗口并发操作状态隔离

- **预期档位**: hard
- **考察维度**: `window_state` 中多窗口 state 按 window_id 分桶 / 互不覆盖
- **工具链**: WindowAction(A) + WindowAction(B) 同轮并发 → 验证 state 写回完整
- **对话脚本**:
  1. [第 1 轮] 同时操作两个窗口:在 A 输入「A 内容」,在 B 输入「B 内容」。
  2. [第 2 轮] 再次操作 A 追加「A 追加」。
  3. 断言:round2 的 `window_state` 中 A 的 action_history 长度 = 2(第 1 轮 + 第 2 轮),B 的 action_history 长度 = 1(仅第 1 轮),`last_window` 指向 A(最后操作),两窗口 state 互不覆盖。

### HT09 AgentMessage 消息桥与消费幂等

- **预期档位**: hard
- **考察维度**: AgentMessage 消费标记 / 幂等处理 / 跨角色数据传递
- **工具链**: WindowAction(get_text) → AgentMessage(WindowTextRead) → SubAgent(分析) → AgentMessage(TextToWindow) → WindowAction(set_text)
- **对话脚本**:
  1. [第 1 轮] 让 Agent 从记事本读取一段 JSON 文本(假设 mock 内容为 `{"name":"laew","version":3}`),并让 SubAgent 计算字段数。
  2. [第 2 轮] 让 Agent 把「刚才算出的字段数」回写到记事本末尾。
  3. 断言:round1 mock 应观察 WindowUse 调 get_text → 之后 AgentMessage 表新增 `(from_role=WindowUse, to_role=SubAgentWork, payload=WindowTextRead{content:"{\"name\":\"laew\",...}"})` 一行;SubAgent 处理完后产生 `(from_role=SubAgentWork, to_role=WindowUse, payload=TextToWindow{content:"2"})`;round2 的 WindowUse set_text 接收的参数应为 `TextToWindow` 解包后的字符串「2」;第二次消费同一消息时 consumed=true 不重复处理。

### HT10 失败状态保留与重试策略

- **预期档位**: medium
- **考察维度**: WindowUse 失败后状态不丢失 / 重试时复用已知控件
- **工具链**: WindowAction(失败) → 状态保留 → WindowAction(重试)
- **对话脚本**:
  1. [第 1 轮] 让 Agent 点击 `/Document/Edit`(mock 返回失败 `ELEMENT_NOT_FOUND`)。
  2. [第 2 轮] 「重试刚才的操作」。(验证:Agent 应读取 window_state 中的 action_history 发现上一轮失败,重新 WindowInspect 刷新 known_controls,再尝试相近路径)
  3. 断言:round2 mock 接收序列为 `[WindowInspect(刷新), WindowAction(重试)]`,不盲目重复原 path;window_state 中 action_history 含两条记录(一条失败 + 一条成功/失败)。
