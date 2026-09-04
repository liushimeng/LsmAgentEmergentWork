# AtomCode 源码深度分析(8 维度)

> 分析对象:`/usr/local/LsmGitOpenSource/atomcode` (v5.0.9,Rust workspace)
> 架构分层:**kernel**(中立 agent 循环) → **capabilities**(可复用能力) → **coding**(业务运行时) → **cli/tui**(前端驱动)
> 目标调用链:`CLI/TUI/daemon → CodingRuntime → kernel Agent`
> 报告聚焦关键文件路径、行号锚点、核心代码片段与设计要点,便于按需跳读。

---

## 1. 多轮对话的实现

### 1.1 REPL 循环位置

atomcode 没有独立的 REPL 屏,多轮对话由 **kernel 的 session loop** 与 **driver 的命令队列** 协同实现:

- **session loop 入口**:`atomcode-kernel/src/agent.rs:1418-1468` `RunningAgent::session_loop`
  - 持有 `cmd_rx: UnboundedReceiver<AgentCommand>`,循环接收 `SendMessage / SendSyntheticMessage / Cancel / Shutdown / Snapshot / Compact` 等命令
  - 每个真实用户消息走 `process_send_message`(`agent.rs:1475`),synthetic 消息(FIFO 队列)在 turn 边界排空
- **driver 侧**:`atomcode-coding/src/runtime.rs` 的 `CodingRuntimeHandle` 暴露 `submit() / steer() / cancel()` 命令,经 channel 灌入 kernel
- **TUI 前端**:`atomcode-cli/src/tui/` 通过 crossterm 原始模式读键,整串命令发给 runtime

### 1.2 Turn 模型

Turn 是 kernel 的原子执行单元,**一个 user message = 一个 turn**,内部可含多轮 LLM 调用(round):

- **TurnCtx**(`atomcode-kernel/src/hook.rs:19-47`):携带 `session_id / turn_id / request_id / round / max_rounds / cache_epoch / context_window / used_tokens`
  - `turn_id` 单调递增(`agent.rs:1076-1078 turn_counter`),`request_id` 全局唯一
  - 注释明示"deterministic — NOT clock/random — so log stitching stays reproducible"
- **round 循环**:`agent.rs:1850-3941 run_turn` 是核心,每轮:
  1. mint `request_id`(`agent.rs:1941`)
  2. 折叠 steer buffer(`agent.rs:2017-2047`)
  3. `pre_request` hook 投影 + 紧急压缩(`agent.rs:2048-2106`)
  4. `chat_stream` 打开流(`agent.rs:2164-2177`)
  5. 消费 `StreamEvent`(`agent.rs:2395-2796`)
  6. 工具三阶段执行(`agent.rs:3232-3801`)
  7. 无工具时走终止/continuation 判定(`agent.rs:3036-3165`)

### 1.3 流式响应处理

- **StreamEvent 枚举**(`atomcode-kernel/src/stream.rs:93-156`):`TextDelta / Reasoning / ReasoningSignature / ToolCall / ToolCallDelta / Usage / ResponseId / ResponseModel / Error / Malformed / Done`
  - `ReasoningSignature` 用于 Anthropic 风格的有符号 thinking block 边界
  - `ToolCallDelta` 纯展示用(驱动端渲染流式工具调用),实际执行靠完整 `ToolCall`
- **hook transform seam**:`on_text_delta` / `on_reasoning_delta` 在**每个 chunk** 上原地变换(`agent.rs:2542-2594`),保证实时流与存储一致
- **Usage 合并**:`TokenUsage::merge_max`(`stream.rs:25-30`)字段级 max,兼容 OpenAI 单次上报与 Anthropic 分字段累积上报
- **空响应重试**:`agent.rs:2822-2910` 检测 `!saw_stream_content` 的 200,按 `1,1,2,2,3s` 预算重试(`EMPTY_RESPONSE_MAX_RETRIES`)
- **truncation 恢复**:`finish_reason=length` 时注入 `TRUNCATION_RESUME_NUDGE`(`agent.rs:3052-3071`),最多 `MAX_TRUNCATION_CONTINUES` 次

### 1.4 用户中断 / Steering

- **Steer 机制**(`agent.rs:725-733`):用户在 turn 中提交的 `SendMessage` 不排队成新 turn,而是写入 `SteerBuf`,在**下一轮 round 边界**折叠进当前 turn(`agent.rs:2017-2047`)
  - 真实 steer 会重置 tool-loop 状态、清除 repeat 检测
- **Cancel**:`CancellationToken` 协作式取消
  - per-turn token 由 `new_turn_token` 铸造(`agent.rs:1096-1101`),外部 cancel token 的 child
  - 在 stream 消费(`agent.rs:2412-2420`)、工具执行(`agent.rs:3619-3628`)、sleep 等待处均 `biased` 轮询 cancel
  - `keep_interrupted_context`(`agent.rs:889-892`)控制 cancel 是 UNDO(回滚到 `rollback_len`)还是保留部分工作
- **Shutdown**:`agent.rs:1559-1571` 区分 `internal_cancel` + `turn_token.cancel()`,等 turn 正常终结,避免跳过 `finish_cancelled / turn_complete`

---

## 2. Context 的管理和实现

### 2.1 消息历史持久化

- **Conversation**(`atomcode-kernel/src/message.rs`):`Vec<Message>` + `cache_epoch: u64`
  - `Message` 携带 `role / text / tool_calls / tool_call_id / reasoning / reasoning_blocks / meta / synthetic` 等字段
  - `MessageMeta`(`message.rs`)记录 `tokens / elapsed_ms / ctx_window / used_tokens / utilization / round / turn_id / request_id / finish_reason`
- **Snapshot 持久化**:`atomcode-capabilities/src/session/` 提供 `SessionManager`,`SnapshotHook`(`parts.rs:936-944`)在 `turn_complete` 时落盘 `.snapshot + .meta + .jsonl`
  - `SNAPSHOT_VERSION`(`message.rs:931`)前向兼容
  - `SessionBinding`(`parts.rs:812-886`)管理 session id、lease、resume

### 2.2 压缩触发条件

- **任务边界自动触发**(`agent.rs:1102-1128 should_compact`):在 `process_send_message` 中,新 user message 入历史**之前**、turn 运行**之前**触发
  - 读取最近 assistant turn 的 `meta.used_tokens`,按**当前模型窗口**重新计算利用率(避免换小窗口模型时误判)
  - 阈值由 `compact_threshold` 配置
- **紧急溢出触发**(`agent.rs:2060-2106`):pre-send 估算 `est(messages) >= limit` 时,在 round 内压缩(有 `MAX_OVERFLOW_ATTEMPTS` 上限)
- **硬溢出恢复**(`agent.rs:2184-2205`):provider 返回 context-overflow 错误时,压缩后重试同一 round(`round -= 1`)
- **手动 `/compact`**:mid-turn 的 compact 命令被排队到 `pending`,在 turn 边界执行(`agent.rs:1612-1620`),避免破坏 within-turn cache

### 2.3 分层摘要(CompactionStrategy)

- **策略 trait**(`message.rs:900-914 CompactionStrategy`):`plan(&CompactionView) -> CompactionPlan` + `will_summarize()`
  - `CompactionView`(`message.rs:830-839`):只读视图,含 `messages / trigger / ctx_window / used_tokens / utilization / sacred_floor`
  - `CompactionPlan`(`message.rs:861-868`):`drain_from / drain_to / summary / rewrites / resume_note`
- **sacred_floor**(`message.rs:550-567 Conversation::sacred_floor`):保护前缀 = 首部 System + 第一个非 synthetic User,永不被 drain
- **net-loss guard**(`message.rs:615-762 prepare_plan`):候选 messages 的 wire bytes 必须**严格小于**原 bytes 才 commit,否则拒绝(不 bump epoch)
- **cache_epoch**:committed compaction 时 `cache_epoch += 1`(`message.rs:737`),refused 时不变,保证 prefix cache 一致性
- **压力释放**:commit 后按 `bytes_after/bytes_before` 缩放最近 assistant 的 `used_tokens / utilization`(`message.rs:725-736`),避免立即再次触发
- **默认策略**:`NoCompaction`(`message.rs:917-924`),embedder 必须显式注入

### 2.4 工具结果截断

- **kernel 级 cap**:`cap_tool_result(&mut result, self.max_tool_result_bytes)`(`agent.rs:3727`),默认 `DEFAULT_MAX_TOOL_RESULT_BYTES`
  - 在 after-chain 之后、push+emit 之前应用,保证 history / model / driver 看到一致
- **self_bounds_output**:`Tool::self_bounds_output()`(tool.rs:227-237)声明工具自约束输出,跳过通用截断(如 `read_file`)
- **stub tool results**:超大工具结果可被替换为 stub(`testkit.rs:1071 StubToolResultsStrategy`)

---

## 3. Yolo 识别 / 任务分类

> ⚠️ **关键差异**:atomcode **没有**独立的 Yolo Agent。它的"任务分类"是 **goal-mode 的 followup 分类器**,不是 laew 的三档(简单/中/难)入口分类。

### 3.1 Goal 模式入口

- **GoalState**(`atomcode-coding/src/runtime.rs`):用户可设定一个 `condition`(目标完成条件),进入 goal-mode
- **goal continuation**:turn 结束时,若 goal 未达成,注入 `goal_continuation_message`(`controllers.rs:577-581`)继续自主工作
- **goal cap**:`max_rounds` 达到后走 `goal_cap_stop_note`(`controllers.rs:311`)或 checkpoint 续行

### 3.2 意图识别 / Followup 分类

- **`classify_followup`**(`controllers.rs:721`):在 goal-mode 下,用户 mid-turn 输入被分类为:
  - `goal_change`:用户改了目标
  - `progress_update`:用户提供进展
  - `other`:其他
- 实现(`controllers.rs:721-860`):调用独立 LLM 请求,system prompt 为分类指令,输入为 `(condition, user_input)`
- 超时控制:`EVALUATOR_TIMEOUT` + `tokio::time::timeout`

### 3.3 Goal 评估(Evaluator)

- **`evaluate_goal`**(`controllers.rs:384-438`):每轮 goal 结束后,用独立 LLM 调用判定目标是否达成
  - system prompt:`EVALUATOR_SYSTEM_PROMPT`
  - user template:`EVALUATOR_USER_TEMPLATE`,注入 `{condition}` 与 `{summary}`
  - `temperature: 0.0`,`tool_choice: None`
- **`summarize_for_goal`**(`controllers.rs:440-572`):为 evaluator 生成摘要,包括:
  - 本轮编辑的文件列表
  - 上一轮裁决
  - 压缩锚点(`<!-- atomcode:anchor`)
  - 最近 5 个 tool results(失败优先)
  - 最近 5 条 assistant reply

### 3.4 设计要点

- goal-mode 是**单 agent + 评估器**模式,不是多 agent 协作
- 分类器与评估器都是**同步 LLM 调用**,不走 agent loop
- 没有"任务三档分类"概念,难度选择体现在 **team 模块**(见第 7 节)

---

## 5. 质检检查

### 5.1 VerifyCadenceHook(编辑后验证节律)

- **位置**:`atomcode-coding/src/discipline/verify.rs:30 VerifyCadenceHook`
- **机制**:实现 `LifecycleHooks::offer_continuation`(`verify.rs:378-419`)
  - 当模型停止(无工具调用)且本轮有编辑、但**未运行检查命令**时,注入一次性 nudge
  - nudge 文本(`verify.rs:23-25`):"Run a fast check (`cargo check`, `tsc --noEmit`, or the equivalent)"
- **状态机**(`verify.rs:47-53 State`):`nudged_for: Option<NudgedEdit>`,保证每个 edit-batch 只 nudge 一次
  - `NudgedEdit { turn_start, edit_id }` 防止复用 provider id 抑制新编辑

### 5.2 编辑检测

- **工具名键控**:`edit_file / write_file / bash` 三种工具触发
- **bash 排除名单**(`verify.rs:334`):`ls / echo / cat / pwd / tree / find / grep / wc` 等只读命令不算验证
- **工作区门禁**(`verify.rs:147-200 path_in_workspace_lexical`):编辑目标在 workspace 外(如 `/tmp`)不触发
  - 纯 lexical 规范化(不 `canonicalize`,避免挂载阻塞)
- **文档排除**(`verify.rs:217`):`.md / .txt / .json / .yaml` 等 doc/data 文件不触发

### 5.3 执行策略

- **attended 模式**(`verify.rs:86-101 attended`):交互 TUI 下**抑制**强制验证(人在场可主动要求);headless/scheduled 保持强制
- **环境变量覆盖**:`ATOMCODE_VERIFY`(`0/off` 常关,`1/on` 常开)
- **HookChain 优先级**(`parts.rs:910`):VerifyCadenceHook 注册在**倒数第二**(仅 TodoHook 在后),利用 "first Some wins" 契约

### 5.4 设计要点

- 这是**编码领域**的质检,不是通用 QC Agent
- 语言无关:不枚举 build 命令,任何语言的检查都算
- 与 TodoHook 协作(`todo.rs:20`):turn 结束时也检查待办关闭

---

## 6. 任务拆解

### 6.1 Team 多角色架构

- **位置**:`atomcode-coding/src/team/` (`mod.rs / manager.rs / runner.rs / tool.rs`)
- **TeamRole**:多种角色(如 `explorer / implementer / reviewer`),各有独立 persona、工具集、权限
  - `TeamRoleId` + `TeamRoleProfile`(`capabilities/src/team.rs`)
- **TeamTaskSpec**(`capabilities/src/team.rs`):`description / prompt / role / permission / difficulty / scope`
- **TeamRunner**(`runner.rs`):编排多角色并行/串行执行
  - `TeamProviderFactory`:`Fn(TeamDifficulty) -> Arc<dyn LlmProvider>`(按难度选模型)

### 6.2 依赖与编排

- **TeamManager**(`manager.rs`):管理 team 生命周期、任务分配
- **team tool**(`team/tool.rs`):暴露 `team_*` 工具给主 agent,可委派子任务
- **scope 隔离**(`runner.rs:482-490`):每个成员 persona 嵌入 scope(如 `src/**`)与 authority(如 "Do not run shell commands")

### 6.3 Goal 模式(单 agent)

- 与 team 模式并列,单 agent + 条件评估
- `GoalState`(`runtime.rs`)持有 `condition / status / max_rounds / round`
- 续行消息(`controllers.rs:577-581`):"Keep working toward this goal autonomously. Do NOT ask the user questions..."

### 6.4 设计要点

- 任务拆解**不是** Plan Agent 输出 Markdown 方案,而是**角色 + 任务规格**
- 没有显式的 SubTask 依赖图,依赖 TeamRunner 的编排逻辑
- 难度选择映射到**模型选择**(见第 7 节),而非流程差异

---

## 7. 任务分类

### 7.1 TeamDifficulty 枚举

- **位置**:`atomcode-capabilities/src/team.rs`
- **档位**:
  - `TeamDifficulty::Simple` → 映射到 `fast-model`
  - `TeamDifficulty::Hard` → 映射到 `capable-model`
- **无 Medium 档**:只有两档(对比 laew 的三档)

### 7.2 评分与路由

- **TeamProviderFactory**(`runner.rs:23`):`Arc<dyn Fn(TeamDifficulty) -> Arc<dyn LlmProvider> + Send + Sync>`
  - 测试示例(`runner.rs:451-478`):`Simple → "fast-model"`,`Hard → "capable-model"`
- **TeamTaskSpec.difficulty**(`runner.rs:465-476`):每个子任务标注难度,由 factory 选模型
- **无独立评分 prompt**:难度由调用方(人或上层逻辑)指定,不是 LLM 评估

### 7.3 与 laew 对比

| 维度 | atomcode | laew |
|------|----------|------|
| 分类主体 | 无 Yolo,由调用方指定 | Yolo Agent 三步意图识别 |
| 档位数 | 2 (Simple/Hard) | 3 (Simple/Medium/Hard) |
| 评分方式 | 无 LLM 评分 | JSON 解析 + 评分 |
| 流程差异 | 仅模型选择不同 | 不同档位走不同 Agent 链路 |

---

## 9. 工具调用

### 9.1 ToolRegistry 设计

- **位置**:`atomcode-kernel/src/tool.rs:277-328 ToolRegistry`
- **共享所有权**:`Arc<RwLock<BTreeMap<String, Arc<dyn Tool>>>>`,clone 共享同一注册表
- **mount vs register**:
  - `register`:向全局注册表添加工具
  - `mount(names)`:选择子集暴露给 LLM,未 mount 的工具不产生 `ToolDef`、不可解析
- **MountedTools**(`tool.rs:334-347`):`Arc<RwLock<Arc<MountedToolsSnapshot>>>`,写时复制快照
  - `MountedToolsSnapshot`(`tool.rs:343-347`):`revision / selected / defs`,保证 turn 内工具集一致
  - `MountedToolsPublisher`(`tool.rs:386-401`):runtime 独占写方,原子发布新快照

### 9.2 Tool trait

- **位置**:`atomcode-kernel/src/tool.rs:206-272 trait Tool`
- **核心方法**:
  - `name() / description() / parameters_schema()`:元数据
  - `execute(args, ctx) -> ToolResult`:执行
- **风险/能力元数据**:
  - `risk(args) -> RiskLevel`(`tool.rs:215-217`):`Safe / Risky`,参数感知(如 `rm -rf` → Risky)
  - `read_only_hint() -> bool`(`tool.rs:224-226`):无副作用工具(来自 MCP `annotations.readOnlyHint`)
  - `self_bounds_output() -> bool`(`tool.rs:235-237`):自约束输出,跳过截断
  - `parallel_safe(args) -> bool`(`tool.rs:245-247`):默认 `read_only_hint()`
  - `always_grant_scope(args) -> String`(`tool.rs:260-262`):"总是"授权范围
  - `take_policy_intervention(result)`(`tool.rs:268-270`):子 agent 策略事件上抬

### 9.3 流式 tool_use

- **StreamEvent::ToolCallDelta**(`stream.rs:122-127`):`index / id / name / arguments`(partial)
  - 纯展示:驱动端渲染流式调用,**不**用于执行
  - 完整 `ToolCall` 在流结束时单独发出
- **ToolCall 结构**(`tool.rs:32-37`):`id / name / arguments`(raw JSON string)

### 9.4 三阶段执行(Phase ①②③)

- **位置**:`agent.rs:3232-3801`
- **Phase ① CLASSIFY**(按序):
  - 重复检测:`result_ids`(mode A,同 id) / `seen_calls`(mode B,同 name+args)
  - 截断保护:`finish_reason=length` 时所有 call 返回 coach 结果
  - middleware `before` 链:`Proceed / Ask / Allow / Deny / DenyTurn / DenyTurnWithIntervention`
  - 产出 `Vec<CallPlan>`:`Execute / Skip / Result`
- **Phase ② EXECUTE**(并发):
  - `FuturesOrdered` + `RwLock` gate + `Semaphore` cap
  - `parallel_safe` 工具拿 read lock(可并发),副作用工具拿 write lock(独占)
  - cap:`ATOMCODE_MAX_PARALLEL_TOOLS`(默认 4,`clamp(1, MAX_PARALLEL_TOOLS_CEILING)`)
  - 每个 future 内 `biased` 轮询 cancel
- **Phase ③ APPLY**(按序):
  - middleware `after` 链
  - `cap_tool_result` 截断
  - 收集 loop fingerprint(用于 loop 检测)
  - vision 图片收割 → 附加到 follow-up user message
  - emit `ToolResult` + push `Message::tool_result`

### 9.5 回调钩子

- **ToolMiddleware**(`atomcode-kernel/src/middleware.rs`):`before(call, tool, rt) -> BeforeOutcome` / `after(result, tool) -> AfterOutcome`
  - `BeforeOutcome::Allow / Deny / DenyTurn / DenyTurnWithIntervention / Ask / Proceed`
  - `AfterOutcome::Block { reason }`
- **HookChain**(`hook.rs:368-376`):多 hook 扇出,注册顺序 = 执行顺序
  - `offer_continuation`:**first Some wins**
  - `user_prompt_submit`:**first Err short-circuit**

### 9.6 设计要点

- **panic contract**(`tool.rs:197-205`):`execute` 不允许 panic(workspace `panic = "abort"`)
- **cancel checkpoint**:分类前 / 执行前 / 工具内均检查 cancel
- **dedup 双模式**:同 id 跳过 / 同 name+args stub,防止 thinking 模型重复发射

---

## 10. MCP 设计与实现

### 10.1 客户端

- **位置**:`atomcode-capabilities/src/mcp/`
- **McpClient trait**(`mcp/client.rs`):`list_tools() / call_tool() / initialize()`
- **传输层**:
  - `transport_stdio.rs`:`StdioClient`,子进程 stdio
  - `transport_http.rs`:`HttpClient`,HTTP/SSE
- **McpToolInfo**(`mcp/client.rs`):`server_name / tool_name / description / input_schema / read_only`

### 10.2 服务端(Registry)

- **McpRegistry**(`mcp/registry.rs:123-155`):
  - `servers: Arc<RwLock<BTreeMap<String, Arc<dyn McpClient>>>>`
  - `failed_servers / status_overrides / configured_servers`
  - `trusted_servers / auto_approved_tools / tool_aliases / server_instructions`
  - `connect_events: Option<mpsc::UnboundedSender<McpConnectEvent>>`
  - `initial_ready: watch::Sender<bool>`(广播初始连接完成)
- **add_server**(`registry.rs:647`):连接 + initialize + list_tools
- **list_tools_for_server**(`registry.rs:792`):发现工具
- **call_tool**(`registry.rs:851`):路由到具体 server 的 client

### 10.3 Tool Schema 转换

- **McpToolAdapter**(`mcp/tool.rs:71-114`):包装单个 MCP 工具为 kernel `Tool`
  - `mcp_tool_full_name`(`mcp/tool.rs:43-67`):`mcp__{server}__{tool}`,超 64 字符或含非法字符时 hash 后缀
  - `sanitize_name_segment`(`mcp/tool.rs:24-35`):非 `[a-zA-Z0-9_-]` → `-`
- **risk 判定**(`mcp/tool.rs:136-147`):
  - `read_only_hint` → Safe
  - `trusted_servers` 或 `auto_approved_tools` → Safe
  - 否则 → Risky(需审批)
- **execute**(`mcp/tool.rs:162-198`):解析 args → `registry.call_tool(server, tool, arguments)` → 映射结果

### 10.4 信任模型

- **project trust**(`mcp/registry.rs:61-82 project_trust_key`):路径规范化 → hash → `{:016x}`
- **trust store**:`mcp_trust.json`,`projects.{key}` 记录信任项目
- **server 级 trust**:配置 `trust: true` → 所有工具 auto-approve
- **per-tool autoApprove**:server 声明的 `autoApprove` 列表
- **instructions 注入**:`McpInstructionsHook`(`coding/src/mcp_instructions.rs`)在 `pre_request` 时追加 server instructions,用 `<mcp-server-instructions>` 标签隔离
  - 上限:`MAX_SERVER_INSTRUCTIONS_CHARS = 4_000`,`MAX_TOTAL_INSTRUCTIONS_CHARS = 16_000`

### 10.5 设计要点

- MCP 连接是**补充就绪**,不阻塞 session candidate path(`parts.rs:793-809`)
- 工具别名持久化,支持 "Always" 授权
- `McpConnectEvent` 用于 TUI 滚动展示连接状态

---

## 11. SKILL 设计

### 11.1 Skill 定义格式

- **位置**:`atomcode-capabilities/src/skills/skill.rs`
- **两种形态**:
  - 扁平 `*.md`:`name = file stem`,内容 = frontmatter + template
  - 目录 `<dir>/SKILL.md`:`name = directory name`,可捆绑 `scripts/ / references/`
- **Frontmatter**(`skill.rs:196-214`):
  - `name`:技能名
  - `description`:描述(默认取 template 首段)
  - `allowed-tools:`:空格/逗号分隔的工具列表
  - `user-invocable:`:`false` 隐藏于菜单,模型仍可自动调用
- **命名空间**:插件技能注册为 `<namespace>:<skill-name>`(`skill.rs:319-325`)

### 11.2 加载

- **SkillRegistry**(`skills/registry.rs`):
  - `SkillRegistry::load(&[PathBuf])`:扫描目录
  - `load_dir(dir, namespace)`:带命名空间加载
  - `standard_skill_dirs` / `runtime_skill_dirs`(`skills/render.rs:56-95`):`~/.atomcode/skills / ~/.claude/skills / ~/.agents/skills / project/.atomcode/skills`
- **优先级**(`skills/render.rs` source_rank):用户 home > 项目 > 插件

### 11.3 调用与 Prompt 注入

- **Skill::expand**(`skill.rs:32-59`):模板展开
  - `$ARGUMENTS[N]` / `$N` 位置参数
  - `$ARGUMENTS` 全量参数
  - `${CLAUDE_SESSION_ID}` / `${CLAUDE_SKILL_DIR}` 变量
  - `` !`cmd` `` shell 预执行(`skill.rs:150-168`)
- **expand_for_injection**(`skill.rs:64-70`):目录技能附带 `<system-reminder>` 安装路径提示
- **SkillCatalogHook**(`skills/catalog_hook.rs`):`session_start` 时注入 `=== AVAILABLE SKILLS ===` 目录到 system prompt
  - `render_catalog_prioritizing`(`skills/render.rs:113`):按优先级渲染

### 11.4 use_skill / list_skills 工具

- **UseSkillTool**(`skills/use_skill.rs:12-89`):
  - `execute`:查找 skill → `expand_for_injection` → 返回内容
  - 错误时列出可用 skill 名
- **ListSkillsTool**(`skills/use_skill.rs:91-127`):列出所有 skill
- **注册**(`skills/mod.rs:33-34 register_skill_tools`):向 ToolRegistry 注册这两个工具

### 11.5 设计要点

- Skill 是**受信用户内容**,shell 注入是设计意图
- 目录技能通过 `<system-reminder>` 告知模型安装路径,避免在 cwd 搜索捆绑文件
- `SkillFirstHook`(`coding/src/skill_first.rs`):DeepSeek 等弱模型在首轮注入"use_skill first"提醒
- 与 MCP 工具命名空间隔离:`mcp__*` vs `<ns>:<skill>`

---

## 附录 A:关键文件索引

| 关注点 | 文件路径 | 核心行号 |
|--------|---------|---------|
| session loop / run_turn | `atomcode-kernel/src/agent.rs` | 1418 / 1850 |
| process_send_message | `atomcode-kernel/src/agent.rs` | 1475 |
| 工具三阶段执行 | `atomcode-kernel/src/agent.rs` | 3232-3801 |
| StreamEvent | `atomcode-kernel/src/stream.rs` | 93-156 |
| CompactionStrategy / Plan | `atomcode-kernel/src/message.rs` | 861-924 |
| sacred_floor / prepare_plan | `atomcode-kernel/src/message.rs` | 550 / 615 |
| Tool trait / Registry | `atomcode-kernel/src/tool.rs` | 206-328 |
| LifecycleHooks / HookChain | `atomcode-kernel/src/hook.rs` | 184-376 |
| TurnCtx | `atomcode-kernel/src/hook.rs` | 19-47 |
| VerifyCadenceHook | `atomcode-coding/src/discipline/verify.rs` | 30-419 |
| evaluate_goal / classify_followup | `atomcode-coding/src/controllers.rs` | 384 / 721 |
| goal_continuation_message | `atomcode-coding/src/controllers.rs` | 577 |
| TeamRunner / TeamProviderFactory | `atomcode-coding/src/team/runner.rs` | 23 / 451 |
| parts 装配(hooks 注册顺序) | `atomcode-coding/src/parts.rs` | 895-1034 |
| McpRegistry | `atomcode-capabilities/src/mcp/registry.rs` | 123-647 |
| McpToolAdapter | `atomcode-capabilities/src/mcp/tool.rs` | 71-199 |
| Skill 定义 / expand | `atomcode-capabilities/src/skills/skill.rs` | 12-92 |
| use_skill / list_skills | `atomcode-capabilities/src/skills/use_skill.rs` | 12-127 |
| SkillCatalogHook | `atomcode-capabilities/src/skills/catalog_hook.rs` | 23- |

## 附录 B:与 laew 的核心差异

1. **无 Yolo 入口 Agent**:atomcode 的"分类"是 goal-mode 的 followup 分类 + 难度选模型,非独立入口 Agent
2. **无 Plan Agent**:任务拆解靠 Team 多角色或 goal continuation,非 Markdown 方案
3. **无独立 QC Agent**:质检 = VerifyCadenceHook(编辑后验证),非通用质检层
4. **Context 压缩**:策略注入(CompactionStrategy trait),默认 NoCompaction,embedder 显式启用
5. **工具并发**:三阶段 + RwLock gate + Semaphore,支持并行只读工具
6. **MCP 一等公民**:完整 trust / alias / instructions 体系
7. **Skill 即模板**:frontmatter + 变量替换 + shell 注入,通过 use_skill 工具调用
