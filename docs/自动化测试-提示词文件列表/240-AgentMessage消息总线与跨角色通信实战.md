# 240 AgentMessage 消息总线与跨角色通信实战

> 编号段 IA01–IA10 · 聚焦「laew Agent 间消息通信机制(agent_message.rs)」:AgentMessage 五元组(id, session_id, from_role, to_role, payload, created_at, consumed) / MessagePayload 四变体(WindowTextRead/TextToWindow/WindowFocus/Data) / AgentRole 八角色(Yolo/Plan/MainWork/SubWork/QualityCheck/SessionContext/Debug/Compact) / 消费标记幂等 / SQLite `agent_messages` 表持久化 / 按 (session_id, to_role, consumed) 索引查询 / WindowUse↔SubAgentWork 消息桥 / MainWork→WindowUse 窗口聚焦 / 跨 WorkFlow 数据传递 / 消息过期清理
>
> 与现有维度互补说明:
> - `230-WindowUse多轮连续性与Agent间通信实战`(HQ 维)是**多轮连续性 + 消息桥**;本文件是**消息总线全能力**——测 AgentMessage 所有变体、角色组合、幂等消费、持久化查询。
> - `22-分布式系统与微服务架构`(DB 维)是**外部分布式系统**;本文件是**laew 内部消息总线**。
> - `110-即时通讯IM系统编程`(EC 维)是**IM 系统**;本文件是**Agent IM**。
> - `205-进程间通信IPC深度实战`(HF 维)是**IPC 机制**;本文件是**Agent IPC**。
> - `142-边缘函数与事件流集成实战`(ET 维)是**事件流**;本文件是**Agent 事件流**。
>
> **本文件独特主题**:`AgentMessage{id, session_id, from_role, to_role, payload, created_at, consumed}` / `MessagePayload` 四变体:WindowTextRead{window_id, window_title, path, content}/TextToWindow{target_window_id, content}/WindowFocus{window_id, reason}/Data{key, value} / `AgentMessageManager` CRUD:send/receive/consume/query / SQLite `agent_messages` 表:`id, session_id, from_role, to_role, payload(JSON), created_at, consumed` / 索引:`(session_id, to_role, consumed)` / 消费幂等:consumed=true 不重复处理 / 跨角色路由:from_role→to_role / 消息过期清理(24h)。

---

### IA01 AgentMessage 基础结构

- **预期档位**: simple
- **考察维度**: AgentMessage 五元组 / MessagePayload 四变体
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ia01_message_struct.py`:消息结构模拟器(参考 `src/agent/agent_message.rs`):(a) `AgentMessage{id, session_id, from_role, to_role, payload, created_at, consumed}`;(b) `MessagePayload` 四变体:WindowTextRead{window_id, window_title, path, content} / TextToWindow{target_window_id, content} / WindowFocus{window_id, reason} / Data{key, value};(c) `new(session_id, from_role, to_role, payload) -> AgentMessage`(consumed 默认 false);(d) JSON 序列化/反序列化。
  2. Bash:构造 4 种 payload 消息各 1 条,断言:序列化→反序列化往返一致,consumed 默认 false,id 格式正确。
  3. Read 源码 + Bash 断言:含五元组、含四变体、含构造函数。

### IA02 消息发送与接收

- **预期档位**: simple
- **考察维度**: send / receive / 按 to_role 路由
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ia02_send_recv.py`:消息收发模拟器:(a) `AgentMessageManager{session_id, messages: Vec<AgentMessage>}`;(b) `send(from_role, to_role, payload) -> AgentMessage`;(c) `receive(to_role) -> Vec<AgentMessage>`:按 to_role 过滤未消费消息;(d) `consume(message_id) -> Result<()>`:标记 consumed=true;(e) 发送后立即接收 → 返回该消息。
  2. Bash:Manager 发 3 条消息(SubAgentWork 2 条 + WindowUse 1 条) → receive(SubAgentWork) 返回 2 条 → receive(WindowUse) 返回 1 条 → receive(MainWork) 返回空。
  3. Read 源码 + Bash 断言:含 send、含 receive 按角色过滤、含消息路由。

### IA03 消费标记幂等

- **预期档位**: medium
- **考察维度**: consumed 标记 / 幂等消费 / 重复消费检测
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ia03_idempotent.py`:幂等消费模拟器:(a) `consume(message_id) -> Result<(), ConsumeError>`:已 consumed 返回 Err(AlreadyConsumed);(b) `receive(to_role) -> Vec<AgentMessage>`:只返回 consumed=false 的消息;(c) 消费后再次 receive → 不包含已消费消息;(d) 多次 consume 同一条消息 → 第 2 次起返回 AlreadyConsumed。
  2. Bash:发 1 条消息 → receive → consume → receive(已空) → 再次 consume → Err(AlreadyConsumed),断言:消费幂等,重复消费返回错误。
  3. Read 源码 + Bash 断言:含 consume 标记、含 receive 过滤、含幂等错误。

### IA04 WindowUse→SubAgentWork 消息桥

- **预期档位**: medium
- **考察维度**: WindowTextRead payload / 窗口文本传递给 SubAgent 处理
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ia04_window_bridge.py`:窗口消息桥模拟器:(a) WindowUse 读到窗口文本后 send(from_role=WindowUse, to_role=SubAgentWork, payload=WindowTextRead{window_id, window_title, path, content});(b) SubAgentWork receive → 获得 WindowTextRead → 处理;(c) 处理完后 send(from_role=SubAgentWork, to_role=WindowUse, payload=TextToWindow{target_window_id, content:processed});(d) WindowUse receive → 获得 TextToWindow → 写回窗口。
  2. Bash:模拟上述流程,断言:消息序列为[WindowUse→SubAgentWork(WindowTextRead), SubAgentWork→WindowUse(TextToWindow)],payload 内容正确,两条消息都被正确消费。
  3. Read 源码 + Bash 断言:含 WindowTextRead、含 TextToWindow、含双向桥。

### IA05 MainWork→WindowUse 窗口聚焦

- **预期档位**: medium
- **考察维度**: WindowFocus payload / Main-Work 指示 WindowUse 聚焦特定窗口
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ia05_focus.py`:窗口聚焦模拟器:(a) Main-Work 拆解 WorkFlow 时 send(from_role=MainWork, to_role=WindowUse, payload=WindowFocus{window_id, reason});(b) WindowUse receive → 获得 WindowFocus → 聚焦该窗口;(c) reason 字段:Main-Work 说明为何聚焦该窗口(如「该窗口包含待处理数据」)。
  2. Bash:Main-Work 发 2 条 WindowFocus(2 个不同 window_id) → WindowUse receive → 断言:收到 2 条,按聚焦顺序处理,reason 字段正确。
  3. Read 源码 + Bash 断言:含 WindowFocus、含 reason、含顺序处理。

### IA06 Data 通用数据传递

- **预期档位**: simple
- **考察维度**: Data payload / 任意 key-value 数据传递
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ia06_data_payload.py`:通用数据传递模拟器:(a) `Data{key, value}` payload;(b) 任意角色间传递任意结构化数据;(c) key 命名约定:`wf.phase.result`/`wf.goal.status`/`custom.*`;(d) value 为 JSON 字符串。
  2. Bash:SubAgentWork → MainWork 传递 `Data{key:"wf.phase.result", value:"{\"phase_id\":\"P1\",\"status\":\"Completed\"}"}` → MainWork receive → 解析 value → 断言 key/value 正确。
  3. Read 源码 + Bash 断言:含 Data 变体、含 key-value、含 JSON value。

### IA07 SQLite 持久化与查询

- **预期档位**: medium
- **考察维度**: agent_messages 表 / 按 (session_id, to_role, consumed) 索引查询
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ia07_persistence.py`:持久化模拟器(参考 `src/agent/agent_message.rs` + `src/config/agent_message.rs`):(a) `agent_messages` 表:`id, session_id, from_role, to_role, payload(JSON), created_at, consumed`;(b) 索引:`(session_id, to_role, consumed)`;(c) `save(message) -> Result<()>`;(d) `query(session_id, to_role, consumed) -> Vec<AgentMessage>`;(e) `mark_consumed(message_id) -> Result<()>`。
  2. Bash:写入 5 条消息(session-1 的 3 条 + session-2 的 2 条) → query(session-1, SubAgentWork, false) 返回 2 条 → mark_consumed 1 条 → query(session-1, SubAgentWork, false) 返回 1 条。
  3. Read 源码 + Bash 断言:含写入、含索引查询、含 consumed 标记。

### IA08 跨 WorkFlow 数据传递

- **预期档位**: hard
- **考察维度**: pending_data 在 WorkFlow 多 Phase 间传递 / AgentMessage 承载
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ia08_cross_wf.py`:跨 WorkFlow 数据传递模拟器:(a) WorkFlow Phase 1:WindowUse 读窗口 → send(WindowTextRead) → SubAgent 处理 → send(Data{key:"wf.pending", value:result});(b) WorkFlow Phase 2:MainWork receive Data → 提取 pending → send(WindowFocus) → WindowUse 写回;(c) 数据不经过用户复述,全程通过 AgentMessage 传递;(d) 模拟 3 Phase WorkFlow,中间数据通过 AgentMessage 桥传递。
  2. Bash:跑上述场景,断言:Phase 1 产生的数据被 Phase 2 正确消费,消息序列正确,无数据丢失。
  3. Read 源码 + Bash 断言:含跨 Phase 传递、含 Data 承载、含无用户复述。

### IA09 消息过期清理

- **预期档位**: medium
- **考察维度**: 24h 过期清理 / 已消费消息清理
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ia09_expiry.py`:消息过期清理模拟器:(a) `cleanup_expired(max_age_hours=24) -> usize`:删除超过 24h 的消息;(b) `cleanup_consumed() -> usize`:删除所有 consumed=true 的消息;(c) `cleanup() -> (expired, consumed)`:综合清理;(d) 清理前统计 → 清理后统计 → 返回清理数量。
  2. Bash:写入 10 条消息(5 条 24h 前 + 5 条近期,其中 3 条已消费) → cleanup → 断言:删除 5 条过期 + 3 条已消费,剩余 2 条。
  3. Read 源码 + Bash 断言:含过期清理、含已消费清理、含综合清理。

### IA10 消息总线完整工作流

- **预期档位**: hard
- **考察维度**: 完整工作流:WindowUse 读 → SubAgent 处理 → WindowUse 写 + MainWork 监控
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ia10_full_workflow.py`:完整工作流模拟器:(a) Main-Work 发 WindowFocus 指示 WindowUse 聚焦窗口 A;(b) WindowUse 读窗口 A 内容 → send(WindowTextRead) → SubAgentWork;(c) SubAgentWork 处理(如 JSON 字段计数) → send(TextToWindow) → WindowUse;(d) WindowUse 写回窗口 A;(e) WindowUse → MainWork 发 Data{key:"wf.result", value:summary} 汇报完成;(f) MainWork 记录 WorkFlow 进度。
  2. Bash:跑完整工作流,断言:消息序列[MainWork→WindowUse(WindowFocus), WindowUse→SubAgentWork(WindowTextRead), SubAgentWork→WindowUse(TextToWindow), WindowUse→MainWork(Data)] 全部正确,消费标记正确,无孤儿消息。
  3. Read 源码 + Bash 断言:含完整工作流、含所有消息类型、含消费完整性。
