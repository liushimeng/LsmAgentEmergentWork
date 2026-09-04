# OpenClaw 源码深度分析(8 维度)

> 分析对象:`/usr/local/LsmGitOpenSource/openclaw`(TypeScript 全栈 Agent 框架)
> 分析日期:2026-09-04
> 说明:OpenClaw 是**单 Agent + 子 Agent 协作**架构(非 laew 的 6 角色多 Agent),以 `agent-loop` 为核心循环,`context-engine` 为可插拔上下文层,`subagents/` 为任务拆解/执行层,`skills/` 为技能系统(含 workshop 自演化),`mcp-*` 为双向 MCP 集成。

---

## 1. 多轮对话的实现

**核心文件**:`packages/agent-core/src/agent-loop.ts`(1776 行)

### 1.1 双层循环架构

OpenClaw 的多轮对话不是简单的"用户→模型→用户"循环,而是 **双层 while 循环**:

```
runLoop (agent-loop.ts:295-531)
├── Outer while(true)                      // 持续直到无 follow-up 消息
│   └── Inner while(hasMoreToolCalls || pendingMessages.length>0)
│       ├── 注入 pendingMessages(steering) // 用户排队消息
│       ├── streamAssistantResponse()      // 流式调用 LLM
│       ├── executeToolCalls()             // 执行工具批次
│       ├── prepareNextTurn()              // 可切换模型/thinking
│       ├── shouldStopAfterTurn()          // 停止钩子
│       └── getSteeringAtCheckpoint()      // 检查 steering 队列
└── getFollowUpMessages()                  // 子 Agent 完成等外部事件
```

**关键设计要点**:

- **Outer loop**(agent-loop.ts:348):只要 `getFollowUpMessages()` 或 `getSteeringAtCheckpoint()` 返回新消息,就继续——这是子 Agent 完成、cron 触发、跨会话消息能续接对话的根本原因。
- **Inner loop**(agent-loop.ts:352):`hasMoreToolCalls` 由工具批次执行结果决定;`pendingMessages` 是 steering 队列(用户输入排队)。
- **Steering 队列**(agent-loop.ts:84-92):`getSteeringMessages` 回调,在每次 turn 开始/工具执行前/工具执行后多处 checkpoint 轮询,实现"用户随时插入消息"。

### 1.2 Turn 编排与流式响应

`streamAssistantResponse()`(agent-loop.ts:537-635):

- 先 `transformContext()`(可配置上下文变换)→ `normalizeCoreContextMessages()` → `convertToLlm()` 转协议消息。
- 流式消费 LLM 响应,事件分两类:
  - **增量事件**(`text_delta`/`thinking_delta`/`toolcall_delta`):`resolveAssistantMessageUpdate()` 合并到 `partialMessage`,发 `message_update`。
  - **终态事件**(`done`/`error`):`finalizeAssistantMessage()` → `removeNonExecutableToolCalls()` + `ensureToolTurnIdentity()`(uuidv7 生成 turnId) + `withAssistantTurnTaint()`(污染标记)。
- **turnTainted 机制**(agent-loop.ts:308,322-330):abort 后标记 turn 被污染,后续 turn 继承该标记,防止 abort 产物被错误续接。

### 1.3 Tool Loop 检测与恢复

`tool-loop-detection.ts`(776 行)是独立模块:

- **6 种检测器**(`LoopDetectorKind`,tool-loop-detection.ts:28-34):`generic_repeat`/`argument_churn`/`unknown_tool_repeat`/`known_poll_no_progress`/`global_circuit_breaker`/`ping_pong`。
- **阈值**(tool-loop-detection.ts:49-52):`UNKNOWN_TOOL_THRESHOLD=10`、`CRITICAL_THRESHOLD=20`、`GLOBAL_CIRCUIT_BREAKER_THRESHOLD=30`。
- **哈希去重**(tool-loop-detection.ts:70-72):`hashToolCall = name:sha256(stableStringify(params))`,对 outcome 也做 `digestToolOutcome`。
- **ping-pong 检测**:识别 A→B→A→B 的工具对互调模式。

在 agent-loop 中通过 `toolLoopRecoveryState.criticalToolLoopSeen`(agent-loop.ts:309-311,434) 跨 turn 传播,第二次 critical loop 直接终止整个 run(`TOOL_LOOP_RECOVERY_TERMINATED_MESSAGE`,agent-loop.ts:75-76)。

### 1.4 Batch Admission(工具批次准入)

`executeToolCalls()`(agent-loop.ts:640-708) 的批次处理:

1. **validate 阶段**:对每个 toolCall 调 `validateToolCallForBatchAdmission()` → `resolveToolCallTool()`(支持 `resolveDeferredTool` 延迟解析)+ `validateToolArguments()`。
2. **beforeToolBatch 钩子**(agent-loop.ts:658-689):整批准入拦截,返回 `intervention` 时触发 `completeToolLoopInterventionBatch()`。
3. **并行/串行分组**(agent-loop.ts:690-707):检测 `executionMode === "sequential"` 的工具,若有则整批串行。
4. **launchParallelToolCalls()**(agent-loop.ts:1025-1093):并行启动,但 steering 消息到达时跳过未启动的调用(`releaseSkippedCalls`)。

**关键类型**(agent-loop.ts:710-981):`ExecutedToolCallBatch`/`ToolBatchContext`/`PreparedToolCall`/`ReadyToolCallExecution`/`FinalizedToolCallOutcome`/`ParallelToolCallLaunches`——完整的批次生命周期类型。

### 1.5 中断与 Abort

`stopIfAborted()`(agent-loop.ts:317-345):每个 checkpoint 检查 signal,abort 时写 `createFailureMessage` + `appendInterruptedTurnMessage`,确保 session 后处理不会从 toolUse 消息错误续接。

---

## 2. Context 的管理和实现

**核心文件**:
- `packages/agent-core/src/harness/compaction/compaction.ts`(1040 行)——内置压缩算法
- `src/context-engine/types.ts`(539 行)——ContextEngine 接口
- `src/context-engine/registry.ts`(796 行)——注册中心 + 隔离降级
- `src/context-engine/delegate.ts`(186 行)——委托桥接
- `src/context-engine/quarantine-health.ts`(90 行)——隔离持久化

### 2.1 双层 Context 架构

OpenClaw 把上下文管理分成 **两层**:

1. **Harness 内置压缩**(compaction.ts):默认实现,不依赖插件。
2. **ContextEngine 可插拔层**(context-engine/):第三方插件可完全替换上下文管理(如 RAG、向量召回、外部存储)。

通过 `delegateCompactionToRuntime()`(delegate.ts:69-138) 桥接:第三方 engine 可以不实现自己的 compact,直接委托给内置运行时。

### 2.2 Compaction(压缩)核心算法

**触发条件**(`shouldCompact`,compaction.ts:299-308):
```
contextTokens > contextWindow - reserveTokens
```
默认 `reserveTokens=16384`,`keepRecentTokens=20000`(`DEFAULT_COMPACTION_SETTINGS`,compaction.ts:173-177)。

**Token 估算**(`estimateContextTokens`,compaction.ts:268-296):
- 优先用 provider 上报的 `usage.contextUsage.totalTokens`(state==="available")。
- 无可靠 usage 时回退到字符估算:`estimateTokens()` 按角色分别估算(text/thinking/toolCall/bashExecution/branchSummary/compactionSummary),`CHARS_PER_TOKEN_ESTIMATE` 换算。
- **unavailable context barrier**(compaction.ts:201-216):CLI 合成标记会阻断 usage 回溯,强制全量估算。

**Cut Point 选择**(`findCutPoint`,compaction.ts:454-524):
- 只在 turn 边界切分(`isCutPointMessage`/`isTurnStartMessage`,compaction.ts:369-399):user/assistant/bashExecution/custom/branchSummary/compactionSummary 是切分点,toolResult 不是。
- 支持 **split turn**(compaction.ts:443-451,519-523):当切分落在 turn 中间时,把 turn 前缀单独作为 `turnPrefixMessages` 单独摘要。

**摘要生成**(`generateSummary`,compaction.ts:693-726):
- 系统提示词 `SUMMARIZATION_SYSTEM_PROMPT`(compaction.ts:526-528) + 结构化格式 `SUMMARIZATION_PROMPT`(compaction.ts:530-561):Goal/Constraints/Progress/Key Decisions/Next Steps/Critical Context。
- **增量更新**:有 `previousSummary` 时用 `UPDATE_SUMMARIZATION_PROMPT`(compaction.ts:563-600),保留旧信息并合并新进展。
- **硬上限** `MAX_COMPACTION_SUMMARY_CHARS=16_000`(compaction.ts:121),超出截断并加 `SUMMARY_TRUNCATED_MARKER`。
- **latestUnresolvedUserRequest**(compaction.ts:123-143):保留最近一条用户请求(≤800 字符),确保 compaction 后仍知道用户在等什么。

**File Operations 追踪**(`CompactionDetails`,compaction.ts:47-54):记录 `readFiles`/`modifiedFiles`,跨 compaction 合并(`mergeSummaryFileOperations`),让后续 prompt 知道哪些文件已被操作。

### 2.3 ContextEngine 接口(types.ts:352-539)

**核心方法**:

| 方法 | 作用 |
|------|------|
| `bootstrap?()` | 会话初始化,可导入历史 |
| `ingest()/ingestBatch?()` | 写入单条/批量消息 |
| `afterTurn?()` | turn 后触发后台压缩决策 |
| `commitTurn?()` | 原子幂等提交一个 turn(`advancementKey` 去重) |
| `assemble()` | **核心**:在 token budget 内组装模型上下文 |
| `compact()` | 压缩上下文 |
| `maintain?()` | transcript 维护(rewriteTranscriptEntries) |
| `prepareSubagentSpawn?()/onSubagentEnded?()` | 子 Agent 生命周期 |

**AssembleResult**(types.ts:7-39):
- `messages`:有序消息
- `estimatedTokens`:估算 token
- `promptAuthority`:`"assembled"` 或 `"preassembly_may_overflow"`——后者让 precheck 取 max(组装后,组装前未窗口化历史),防止 engine 隐藏溢出。
- `systemPromptAddition`:engine 可注入额外系统提示
- `contextProjection`:`"per_turn"` 或 `"thread_bootstrap"`——后者用于有持久后端线程的 engine(如 Claude 的 prompt caching 后端)。

**RuntimeSettings**(types.ts:80-110):完整的运行时元数据(schemaVersion=1),含 host/mode/harnessId/model/selection/executionHost/limits/diagnostics。

### 2.4 Registry 与隔离降级(registry.ts)

**注册**(registry.ts:307-357):
- `registerContextEngineForOwner(id, factory, owner)`:每个 engine 有 **trusted owner**,默认 slot id 归 core 所有。
- `adoptRuntimeContextEngineRegistrations()`(registry.ts:389-427):跨 registry 世代采用 runtime engine,要求 source 相同(防 workspace 阴影攻击)。

**Quarantine 隔离降级**(registry.ts:250-282):
- 引擎在 guarded 方法(`bootstrap/maintain/ingest/ingestBatch/afterTurn/commitTurn/assemble/compact/prepareSubagentSpawn`)中抛错时,`recordContextEngineQuarantine()` 记录隔离。
- **First failure wins**:同一 engine 只记录首次失败原因。
- **降级行为**:`wrapResolvedContextEngine()`(registry.ts:98-209) 用 Proxy 包装引擎,隔离后所有 guarded 调用转给 fallback engine(默认 legacy)。
- **compact/prepareSubagentSpawn 不降级**(registry.ts:192-194):直接抛出,因为压缩失败不能静默替换。
- **Abort 不触发隔离**(registry.ts:180-184):`isContextEngineAbortRejection` 识别后 caller intent,不记录。

**持久化隔离**(quarantine-health.ts):
- `recordPersistedContextEngineQuarantine()` 写入 `RuntimeHealthStore`,跨进程可见。
- `listContextEngineQuarantines()` 合并内存 + 持久化记录。
- `clearPersistedContextEngineQuarantineForProcess()` 清理。

### 2.5 Delegate 桥接(delegate.ts)

- `delegateCompactionToRuntime()`(delegate.ts:69-138):第三方 engine 的 `compact()` 可委托给内置 `compactEmbeddedAgentSessionOnDemand()`。
- `buildMemorySystemPromptAddition()`(delegate.ts:163-178):非 legacy engine 可选择性接入 memory/wiki 提示。
- `assertCompactionSessionIdentity()`(delegate.ts:28-54):校验 successor session target 与 caller 身份一致,防止压缩后切换到错误 session。

---

## 3. Yolo 识别 / 任务分类

**关键发现**:OpenClaw **没有** laew 那样的"Yolo 入口层 + 三档任务分类"设计。

### 3.1 "Yolo" 在 OpenClaw 中的含义

搜索 `yolo` 仅出现在两处,且都是 **exec(命令执行)模式**,不是任务分类:

- `agent-bundle-mcp-harness.ts:178`:`Exact established Codex yolo predicate; no other profile bypasses approval metadata.`
- `bash-tools.exec-host-gateway.ts:923` / `bash-tools.exec-host-node.ts:202`:`Warning: security audit suppression changes require explicit approval unless exec is running in yolo mode.`

即 **Yolo = exec 免审批模式**(类似 laew 的"接入点"概念,但仅针对命令执行,不是任务入口层)。

### 3.2 任务分类的替代方案

OpenClaw 不做"简单/中/硬"三档分类,而是:

1. **单 Agent 直接处理**:用户输入直接进入 `agent-loop`,由模型自己决定如何拆解。
2. **SubAgent 委派**:模型通过 `sessions_spawn` 工具主动生成子 Agent(见第 6 节)。
3. **Swarm 并行**:通过 `sessions_spawn` + `collect=true` 进入 swarm 模式,多子 Agent 并行(见第 6 节)。

**设计差异**:laew 用 Yolo Agent 做"入口层"先分类;OpenClaw 把这个决策权交给 LLM 自身(通过 system prompt 引导)。

---

## 4. 质检检查

**核心文件**:
- `src/agents/exec-auto-reviewer.ts`(472 行)——模型-backed 命令审查员
- `src/infra/exec-auto-review.ts`(113 行)——审查决策类型与兜底

### 4.1 Exec Auto-Reviewer 设计

这是一个**独立小模型审查员**,在命令执行前做准入判断:

**决策类型**(`ExecAutoReviewDecision`,exec-auto-review.ts:10-20):
- `allow-once`:仅当 `risk === "low"` 时返回
- `ask`:其他所有情况(送人工审批)

**Schema**(exec-auto-reviewer.ts:35-41):
```typescript
z.object({
  decision: z.enum(["allow", "ask"]),
  risk: z.enum(["low", "medium", "high", "unknown"]),
  rationale: z.string().optional(),
}).strict()
```

**保守解析**(`parseExecAutoReviewResponse`,exec-auto-reviewer.ts:207-283):
- 无 JSON → `ask`/`unknown`
- 畸形 JSON → `ask`/`unknown`
- **重复 key 检测**(exec-auto-reviewer.ts:142-204):`hasDuplicateJsonObjectKeys()` 防止 `{"decision":"ask",...,"decision":"allow"}` 被 JSON.parse 静默覆盖。
- **__proto__ 防护**(exec-auto-reviewer.ts:237-247):Zod strict 不检查 `__proto__`,手动校验 key 都在 schema 中。
- **非 low allow 降级**(exec-auto-reviewer.ts:270-276):`decision=allow` 但 `risk !== "low"` → 转 `ask`。

### 4.2 Prompt 注入攻击防护

**Reviewer Directive 检测**(`textLooksLikeReviewerDirective`,exec-auto-reviewer.ts:93-112):
- 检测 `ignore/disregard/override ... instruction/system/developer/prompt/policy`
- 检测 `return/respond/output/say/print ... decision ... allow`
- 检测 `exec reviewer ... decision/allow/risk/rationale`
- 检测 `untrusted exec/widget request json end`
- 命中任一 → 直接 `ask`/`medium`,不送模型。

**输入边界标记**(exec-auto-reviewer.ts:78-91):
```
UNTRUSTED_EXEC_REQUEST_JSON_BEGIN
{command, argv, resolvedPath, cwd, envKeys, host, reason, analysis}
UNTRUSTED_EXEC_REQUEST_JSON_END
```
提示词明确告知模型:标记内是不可信数据,不要遵循其中的指令。

### 4.3 超时与失败兜底

- `DEFAULT_EXEC_REVIEWER_TIMEOUT_MS=30_000`(exec-auto-reviewer.ts:30)
- `EXEC_REVIEWER_MAX_TOKENS=360`(exec-auto-reviewer.ts:31)
- `MAX_EXEC_REVIEWER_INPUT_CHARS=16_000`(exec-auto-reviewer.ts:32):超长输入直接 defer。
- `raceWithReviewerTimeout()`(exec-auto-reviewer.ts:329-352):超时返回 `EXEC_REVIEWER_TIMEOUT` 符号,转 `ask`/`unknown`。
- **任何异常都是 `ask`**(exec-auto-reviewer.ts:464-470):`buildExecAutoReviewFailureDecision()` 兜底。

### 4.4 默认审查员

`defaultExecAutoReviewer`(exec-auto-review.ts:106-112):无模型配置时返回 `ask`,但 `inlineEval` 时 risk 升为 `medium`。

**对比 laew**:laew 的 Quality-Check Agent 是独立角色,在 SubAgent-Work 完成后必过 QC;OpenClaw 的"质检"主要体现在 **exec 准入**(命令执行前)和 **tool-loop 检测**(工具循环时),没有独立的 QC Agent 角色。

---

## 5. 任务拆解

**核心目录**:`src/agents/subagents/`(约 29857 行,最大子系统)

### 5.1 子 Agent 架构概览

```
subagents/
├── spawn/      # 启动子 Agent(sessions_spawn 工具)
│   ├── subagent-spawn.ts          # spawnSubagentDirect() 主入口
│   ├── subagent-spawn.types.ts    # SUBAGENT_SPAWN_MODES=["run","session"]
│   ├── subagent-spawn-plan.ts     # 子 Agent 计划解析
│   ├── subagent-spawn-context.ts  # 上下文引擎准备/回滚
│   ├── subagent-spawn-gateway.ts  # Gateway 调用
│   ├── subagent-spawn-cleanup.ts  # 失败清理
│   └── acp-spawn.ts               # ACP 运行时子 Agent
├── registry/   # 任务注册表 + 生命周期
│   ├── subagent-registry.ts                    # 主协调器
│   ├── subagent-registry.types.ts              # SubagentRunRecord
│   ├── subagent-registry-lifecycle.ts          # 生命周期控制器
│   ├── subagent-registry-run-manager.ts        # 运行管理
│   ├── subagent-registry-run-recovery.ts       # 中断恢复
│   ├── subagent-registry-sweeper.ts            # 清理扫描
│   ├── subagent-registry.store.sqlite.ts       # SQLite 持久化
│   └── subagent-session-reconciliation.ts      # 会话对账
├── announce/   # 完成通知
│   ├── subagent-announce.ts                    # 通知协调
│   ├── subagent-announce-output.ts             # 输出捕获
│   ├── subagent-announce-delivery.ts           # 投递
│   └── subagent-announce-handoff.ts            # 交接
├── completion/ # 完成处理
│   ├── subagent-completion-result.ts           # 结果选择
│   └── subagent-completion-delivery.ts         # 完成投递
└── swarm/      # 并行 swarm
    ├── swarm-scheduler.ts          # FIFO 限流调度
    ├── swarm-config.ts             # 配置解析
    ├── swarm-collector.ts          # 结果收集
    └── swarm-code-mode.ts          # Code Mode 幂等键
```

### 5.2 Spawn 流程

`sessions-spawn-tool.ts` 是 `sessions_spawn` 工具的实现,`spawnSubagentDirect()`(subagent-spawn.ts:92) 是主入口:

1. **请求解析**(`resolveSubagentSpawnRequest`):提取 task/label/spawnMode/sandbox/contextMode/swarmConfig。
2. **计划解析**(`resolveSubagentChildPlan`):确定 model/thinkingOverride/childSessionKey/spawnedWorkspaceDir。
3. **初始 Session 创建**(`createInitialSubagentSession`):写 SQLite,绑定 tool allowlist/denylist 继承。
4. **ContextEngine 准备**(`prepareContextEngineSubagentSpawn`):调用 engine 的 `prepareSubagentSpawn()`,返回 rollback handle。
5. **Gateway 调用**(`callNativeSubagentGateway`):通过 Gateway 启动子 Agent run。
6. **注册**(`subagent-registry.ts`):`registerSubagentRun()` 写入 `subagentRuns` Map + SQLite。

**Spawn 模式**(subagent-spawn.types.ts:3-13):
- `SUBAGENT_SPAWN_MODES = ["run","session"]`
- `SUBAGENT_SPAWN_CONTEXT_MODES = ["isolated","fork"]`——isolated 独立上下文,fork 继承父上下文
- `SUBAGENT_SPAWN_SANDBOX_MODES = ["inherit","require"]`

### 5.3 任务注册表(subagent-registry)

`SubagentRunRecord`(subagent-registry.types.ts:222-269) 是核心类型:

```typescript
type SubagentRunRecord = {
  runId: string;
  taskRunId?: string;           // 解耦 task 与 run(steer/restart 换 runId 但继续同一 task)
  requesterTurnRunId?: string;  // 精确 requester 尝试(用于取消)
  childSessionKey: string;
  requesterSessionKey: string;
  task: string;
  cleanup: "delete" | "keep";
  execution: SubagentExecutionState;     // queued/running/interrupted/terminal
  completion: SubagentCompletionState;   // 完成捕获
  delivery: SubagentCompletionDeliveryState; // 投递状态机
  killReconciliation?: SubagentKillReconciliationState;
  // ...
};
```

**持久化**(subagent-registry.store.sqlite.ts):`saveSubagentRegistryToSqlite()` / `loadSubagentRegistryFromSqlite()`,进程间共享。

**Sweeper**(subagent-registry-sweeper.ts):定期扫描清理过期/孤儿 run。

**Restart Recovery**(subagent-registry-restart-recovery.ts):Gateway 重启后恢复中断的 run,用 `SubagentRestartRecoveryReceipt`(subagent-registry.types.ts:61-68) 跟踪 phase:`reserved/attempted/consumed/accepted/abandoned`。

### 5.4 Announce(完成通知)

`subagent-announce.ts` 协调子 Agent 完成后的通知:

- **输出捕获**(`subagent-announce-output.ts`):`readSubagentOutput()` 读子会话最终 assistant 文本,`MAX_CHILD_COMPLETION_RESULT_CHARS=512` 截断。
- **Silent Token**(subagent-announce.ts:115-121):子 Agent 回复 `SILENT_REPLY_TOKEN` 表示"无需通知"。
- **Steer 注入**:子 Agent 完成时,通过 `formatAgentInternalEventsForPrompt()` 包装成 steer 消息注入父 Agent 的 steering 队列。
- **Delivery 状态机**(`SubagentCompletionDeliveryState`,subagent-registry.types.ts:128-178):`not_required/pending/in_progress/delivered/failed/suspended/discarded`,含 `requesterVisibleFinal` 去重。

### 5.5 Completion 结果选择

`resolveSubagentCompletionResultText()`(subagent-completion-result.ts:5-28):
- `terminalReply`(生产者终端证据)优先级最高
- 否则 `resultText` → `fallbackResultText`
- `outcome.status === "ok"` 时用 `selectDeliverableSessionsReply()` 选可投递回复

---

## 6. 任务分类(Swarm 与调度)

**核心文件**:
- `src/agents/subagents/swarm/swarm-scheduler.ts`(限流调度)
- `src/agents/subagents/swarm/swarm-config.ts`(配置)
- `src/agents/subagents/swarm/swarm-collector.ts`(结果收集)
- `src/agents/subagents/swarm/swarm-output-schema.ts`(结构化输出校验)

### 6.1 Swarm 模式

Swarm 是 OpenClaw 的**并行子 Agent 协作**模式:

- 通过 `sessions_spawn` + `collect=true` 启用
- 多个子 Agent 并行执行,结果由 **collector** 汇总
- 支持 **structured output**(`outputSchema`),用 `validateStructuredOutputSchema()`(swarm-output-schema.ts) 预检

**配置**(`resolveSwarmConfig`,swarm-config.ts:39-72):
```typescript
DEFAULT_SWARM_CONFIG = {
  enabled: false,
  maxConcurrent: 8,        // 最大并行
  maxChildrenPerGroup: 50,
  maxTotalPerGroup: 200,
  waitTimeoutSecondsMax: 600,
}
```

### 6.2 Scheduler 限流

`swarm-scheduler.ts` 实现 **per-group FIFO 限流**:

- **Lane 机制**:每个 `groupId` 一个 `SwarmGroupLane`,含 `active Set` + `queue`。
- **reserveSwarmRun()**(swarm-scheduler.ts:149-164):预留 FIFO 位置(在异步 spawn 准备前)。
- **activateSwarmRun()**(swarm-scheduler.ts:199+):绑定 launch 工作。
- **pumpLane()**(swarm-scheduler.ts:96-112):microtask 调度,当 `active.size < limit` 时启动队列中的下一个。
- **startQueuedRun()**(swarm-scheduler.ts:53-94):启动后若失败,`onStartFailure()` 判断是否持久化失败,否则回队列头部重试(1s 后 `retryReady=true`)。
- **Capacity 通知**(`publishCapacityChange`,swarm-scheduler.ts:34-43):`isSwarmRunWaitingForCapacity()` 让 UI 显示等待状态。

### 6.3 Collector 结果收集

`updateSwarmCollectorCompletion()`(swarm-collector.ts:31-78):
- 冻结 collector record
- `consumeSwarmStructuredOutput()` 读取结构化输出
- `resolveStatus()`(swarm-collector.ts:12-28):killed→killed / timeout→timeout / ok→done / 有 structured 但 error="completed"→done
- schema 校验失败转 `failed`

### 6.4 会话种类与错误分类

**会话种类**(通过 session key 段标识):
- `cron`:定时任务
- `hook`:钩子触发
- `subagent`:子 Agent
- `skill-workshop-review`:技能 workshop 评审
- `active-memory`:主动记忆
- `heartbeat`:心跳

**错误分类**(subagent 相关):
- `SubagentRunOutcome.status`:`"ok" | "error" | "timeout" | "unknown"`
- `SubagentLifecycleEndedReason`:生命周期结束原因
- `SubagentDeliveryDisposition`(subagent-registry.types.ts:70-76):`"delivered" | "session_queued" | "intentional_non_delivery" | "retryable" | "ambiguous" | "permanent_failure"`
- `lastDropReason`(subagent-registry.types.ts:171-178):`"queue_cap" | "parent_run_ended" | "sink_unavailable" | "steer_dropped" | "dedupe" | "waiting_for_requester_turn"`

---

## 7. 工具调用

**核心文件**:
- `src/agents/agent-tools.ts`(1100 行)——工具表面装配
- `src/agents/core-coding-tools.ts`(261 行)——核心编码工具
- `src/agents/tool-search.ts`(421 行)——渐进披露工具搜索
- `src/agents/tool-search-code-mode.ts`(Code Mode 子进程)
- `src/agents/tool-search-runtime.ts`(749 行)——搜索运行时
- `src/agents/tool-loop-detection.ts`(776 行)——循环检测

### 7.1 工具表面装配(agent-tools.ts)

`buildEffectiveAgentToolSurface()` 是总装配函数,按顺序叠加:

1. **createCoreCodingTools()**(core-coding-tools.ts):read/write/edit/apply_patch/exec/process
2. **createOpenClawTools()**:sessions_spawn/sessions_send/sessions_yield、channel、cron 等
3. **createMemoryTools()**:memory_read/memory_write
4. **createToolSearchTools()**:tool_search/tool_describe/tool_call/tool_search_code
5. **Plugin 工具**:`resolveOpenClawPluginToolsForOptions()`
6. **MCP 工具**:`mergeMcpToolCatalogs()`
7. **Channel 工具**:`listChannelAgentTools()`

**策略管道**(`applyToolPolicyPipeline`,agent-tools.ts:103):
- `filterToolsByClientCaps()`:按客户端能力过滤
- `filterToolsByMessageProvider()`:按消息 provider 过滤
- `filterLocalModelLeanTools()`:本地模型精简工具
- `shouldSuppressManagedWebSearchTool()`:托管 web search 去重
- `applyExecPolicyLayer()`:exec 策略层
- `expandToolGroups()`:工具组展开
- `replaceWithEffectiveToolAllowlist()`:最终 allowlist 替换

**Memory Flush 写保护**(agent-tools.ts:129):`MEMORY_FLUSH_ALLOWED_TOOL_NAMES = new Set(["read","write"])`——只有 read/write 允许 memory flush append-only 写。

### 7.2 Core Coding Tools(core-coding-tools.ts)

`createCoreCodingTools(options)` 工厂:

- **read**:沙箱内 `createSandboxedReadTool`(bridge fs)或宿主机 `createReadTool` + `createOpenClawReadTool`(含 image sanitization)
- **write/edit**:`createHostWorkspaceEditTool`/`createHostWorkspaceWriteTool` + `wrapToolWorkspaceRootGuardWithOptions()` 限制在 containmentRoot 内
- **apply_patch**:`createApplyPatchTool()`,受 `isApplyPatchAllowedForModel()` 策略控制
- **exec/process**:`createLazyExecTool()`/`createLazyProcessTool()`,沙箱可选

**Skill 内容注入**(core-coding-tools.ts:156-163):`wrapReadToolWithSkillContent()` 让 read 工具在读取 skill 文件时注入 skill 内容。

### 7.3 Tool Search(渐进披露)

OpenClaw 的工具表面可能很大(52 个内置 skill + MCP + 插件),所以引入 **渐进披露**:

**4 个搜索工具**(tool-search.ts):
- `tool_search`:文本搜索(lexical + 参数元数据索引,`buildLexicalIndex`/`scoreLexical`,tool-search-runtime.ts:60-77)
- `tool_describe`:查看工具详情(含 parameters/outputSchema)
- `tool_call`:通过 id/name 调用工具
- `tool_search_code`:Code Mode,在 `--permission` Node 子进程中执行用户代码,bridge 方法 `search`/`describe`/`call`(tool-search-code-mode.ts:56-101)

**Catalog 压缩**(tool-search.ts:89-149):
- `compactBatchCandidateDescription()`:描述截断到 180 字符
- `compactBatchCandidate()`:元数据截断到 2000 字符
- `boundToolSearchBatchResponse()`:整体响应截断到 `MAX_TOOL_SEARCH_BATCH_RESPONSE_CHARS`

**Unknown Tool 恢复**(tool-search-runtime.ts:111-140):
- 模型调用不存在工具时,`scoreUnknownToolSuggestion()` 找最多 3 个建议
- 错误消息引导:`"Use tool_search to find a tool, tool_describe to inspect it, then tool_call with the exact id or name."`

**Code Mode 子进程**(tool-search-code-mode.ts:103-150):
- `spawn(process.execPath, ["--permission","--input-type=module","--eval", TOOL_SEARCH_CODE_MODE_CHILD_SOURCE])`
- IPC 通信,`bridgeAbortController` 联动父 signal
- stderr 捕获 + 超时 kill

### 7.4 Tool Loop Detection(tool-loop-detection.ts)

已在第 1.3 节详述,补充:

- **参数 churn 检测**(`tool-loop-argument-churn.ts`):参数微小变化但无进展
- **no-progress 检测**(`tool-loop-no-progress.ts`):工具结果无实质变化
- **write-outcome 检测**(`tool-loop-write-outcome.ts`):写操作结果重复
- **配置**(`tool-loop-detection-config.ts`):`ToolLoopDetectionConfig` 可配阈值

---

## 8. MCP 设计与实现

**核心文件**(约 60+ 文件):
- `src/agents/agent-bundle-mcp-runtime.ts`(1139 行)——Session-scoped MCP 运行时
- `src/agents/agent-bundle-mcp-types.ts`(150+ 行)——类型定义
- `src/agents/agent-bundle-mcp-manager.ts`——运行时管理
- `src/agents/mcp-transport.ts`(130+ 行)——传输工厂
- `src/agents/mcp-stdio-transport.ts`(120+ 行)——stdio 传输
- `src/agents/mcp-http-transport.ts`(120+ 行)——SSE/HTTP 传输
- `src/agents/mcp-client-lifecycle.ts`——连接生命周期
- `src/agents/mcp-oauth.ts`/`mcp-oauth-fetch.ts`——OAuth
- `src/agents/mcp-tool-filter.ts`——工具过滤
- `src/agents/mcp-json-schema-validator.ts`——Schema 校验
- `src/agents/mcp-pagination.ts`——分页列举
- `src/agents/mcp-tool-metadata.ts`——工具元数据

### 8.1 双向 MCP

OpenClaw 同时是 **MCP Client**(连接外部 MCP Server)和 **MCP Server**(暴露自身工具给外部):

- **Client 侧**(上述文件):连接 stdio/SSE/Streamable-HTTP 服务器,拉取工具列表,代理调用。
- **Server 侧**:通过 `tool-stdio-server` 等暴露 OpenClaw 工具(搜索 `agent-bundle-mcp-server`/`embedded-agent-mcp.ts`)。

### 8.2 Session-scoped MCP Runtime

`createSessionMcpRuntime(params)`(agent-bundle-mcp-runtime.ts:246) 是核心工厂:

```typescript
type BundleMcpSession = {
  serverName: string;
  client: Client;                    // @modelcontextprotocol/sdk Client
  transport: Transport;
  transportType: "stdio" | "sse" | "streamable-http";
  requestTimeoutMs: number;
  supportsParallelToolCalls: boolean;
  connected: boolean;
  retiring: boolean;
  // ...
};
```

**Catalog 列举**(`listAllTools`,agent-bundle-mcp-runtime.ts:113-149):
- `collectMcpPaginatedItems()` 分页拉取,限制 `MAX_LIST_PAGES=128` / `MAX_LIST_ITEMS=16_384` / `MAX_LIST_BYTES=10MB`
- 超时 `BUNDLE_MCP_CATALOG_LIST_TIMEOUT_MS=1500`

**失败退避**(`recordServerToolFailure`,agent-bundle-mcp-runtime.ts:333-349):
- `BUNDLE_MCP_FAILURE_THRESHOLD=3` 次失败后进入退避
- `BUNDLE_MCP_FAILURE_COOLDOWN_MS=60_000` 冷却

**Catalog 失效**(`scheduleCatalogServerRetry`,agent-bundle-mcp-runtime.ts:295-328):单 server 失败只标记 diagnostic,不失效整个 catalog,`catalogRetryIsDue()` 决定重试时机。

### 8.3 传输层

**传输工厂**(`resolveMcpTransport`,mcp-transport.ts:28):
- 根据 config 选择 stdio / SSE / streamable-HTTP
- 附加 auth profile bearer / OAuth bearer / same-origin headers

**Stdio 传输**(`OpenClawStdioClientTransport`,mcp-stdio-transport.ts:35):
- `spawn(command, args, {detached: true})`,非 win32 用 child.pid 作为 PGID
- stderr 流通过 `PassThrough` + `StringDecoder` 行缓冲,8KB 截断
- close 时 `signalProcessTree(child.pid, "SIGKILL")` 清理子进程树
- OOM score 调整:`prepareOomScoreAdjustedSpawn()`

**HTTP 传输**(`mcp-http-transport.ts`):
- 包装 `SSEClientTransport` / `StreamableHTTPClientTransport`
- **响应限流**(`limitMcpResponseStream`,mcp-http-transport.ts:36-97):匹配 SDK stdio 上限 `STDIO_DEFAULT_MAX_BUFFER_SIZE`,SSE 事件边界重置计数
- **OAuth 获取**(`mcp-oauth-fetch.ts`):`withMcpOAuthBearer()` 注入 bearer token

### 8.4 工具投影与过滤

**Server 名称消毒**(`agent-bundle-mcp-names.ts`):`sanitizeServerName()` + `assignSafeServerNames()` 生成稳定 tool name。

**工具过滤**(`mcp-tool-filter.ts`):
- `isMcpToolAllowed()`:deny/allow 策略
- `normalizeMcpToolFilter()`:标准化过滤配置

**Schema 校验**(`mcp-json-schema-validator.ts`):`createMcpJsonSchemaValidator()` 校验入参。

**Codex 审批模式**(`mcp-codex-tool-approval.ts`):`normalizeMcpCodexToolAnnotations()` + `resolveProjectedMcpCodexToolApprovalMode()` 适配 Codex 的工具审批元数据。

**MCP Apps**(agent-bundle-mcp-runtime.ts:79-80):`MCP_APPS_CLIENT_EXTENSION="io.modelcontextprotocol/ui"`,支持 `text/html;profile=mcp-app` MIME 类型的 UI 资源。

---

## 9. SKILL 设计

**核心目录**:`skills/`(52 个内置技能) + `src/skills/`(加载/发现/序列化) + `src/skills/workshop/`(自演化)

### 9.1 技能格式

每个技能是目录下的 `SKILL.md`,frontmatter 示例(`skills/coding-agent/SKILL.md`):

```yaml
---
name: coding-agent
description: "Delegate coding work to Codex, Claude Code, or OpenCode as background workers..."
metadata:
  openclaw:
    emoji: "🧩"
    requires:
      anyBins: ["claude", "codex", "opencode"]
      config: ["skills.entries.coding-agent.enabled"]
    install:
      - id: node-claude
        kind: node
        package: "@anthropic-ai/claude-code"
        bins: ["claude"]
---
```

**Frontmatter 解析**(`parseSkillFrontmatter`,frontmatter.ts:25-32):
- 基于 `markdown-core` 的 frontmatter 解析
- 安装 spec 支持 `brew`/`node`/`go`/`uv`/`download` 五种 kind
- 安全校验:brew formula 正则、npm spec 校验、Go module 正则、URL 仅 http/https

### 9.2 Skill 类型(skill-contract.ts)

```typescript
interface Skill {
  name: string;
  displayName?: string;       // 从 H1 标题解析
  description: string;
  locationNote?: string;
  readContent?: string;       // 非文件系统技能(node:// 等)
  filePath: string;
  baseDir: string;
  sourceInfo: SourceInfo;
  disableModelInvocation: boolean;
  source: string;
}
```

**Prompt 渲染**(`formatSkillsForPromptCore`,skill-contract.ts:69-92):
```xml
<available_skills>
  <skill>
    <name>...</name>
    <description>...</description>
    <location>...</location>
  </skill>
</available_skills>
```

**紧凑格式**(`formatSkillsCompactForPrompt`,skill-contract.ts:95-120):描述截断到 `COMPACT_DESCRIPTION_MAX_CHARS=220`。

### 9.3 Prompt 预算(skill-prompt-limits.ts)

`prepareSkillsForPrompt()`(skill-prompt-limits.ts:78-187) 是核心:

- `DEFAULT_MAX_SKILLS_IN_PROMPT=150`
- `DEFAULT_MAX_SKILLS_PROMPT_CHARS=18_000`
- **渐进降级**:
  1. 先尝试 full 格式
  2. 超预算 → compact 格式(描述截断)
  3. 仍超 → 二分查找最大技能数
  4. 最后尝试去掉 limit note
- **截断警告**(`buildSkillsLimitNote`,skill-prompt-limits.ts:15-33):`⚠️ Skills truncated: included X of Y`

### 9.4 Skill Snapshot 与注入

`buildSkillSnapshot()`(workspace-skill-prompt.ts:57-82):
- 解析 eligible skills → `filterPromptVisibleSkillEntries()` → `prepareSkillsForPrompt()`
- 输出 `SkillSnapshot`:`prompt` + `skills` + `skillFilter` + `skillOverrides` + `nodeSkillsEligibility` + `resolvedSkills` + `promptFormatVersion=4`

**版本兼容**(workspace-skill-prompt.ts:112-150):
- `snapshotHasUnavailableSkill` + 旧格式 → `rebuildAfterUnsafeSnapshot()`
- `snapshotHasLegacySkillIdentity` → 重建

### 9.5 Workshop 自演化

`src/skills/workshop/` 是 OpenClaw 最独特的部分——**技能可以自我演化**:

**核心流程**:

1. **History Scan**(`history-scan.ts`):扫描会话历史,发现技能改进机会
   - `runSkillHistoryScanCore()`:分 batch 读取会话(`HISTORY_SCAN_MAX_SESSION_CHARS`)
   - 游标分页(`oldestCursor`/`newestCursor`),方向 `older`/`newer`
   - 持久化状态 `StoredHistoryScanState`

2. **Experience Review**(`experience-review.ts`):后台 agent 评审技能使用体验
   - `prepareSkillExperienceReviewCandidate()`:过滤 cron/heartbeat/memory/overflow/sandbox 会话
   - `EXPERIENCE_REVIEW_MIN_MODEL_ITERATIONS=10`
   - `EXPERIENCE_REVIEW_TIMEOUT_MS=120_000`
   - 调度器 `ExperienceReviewScheduler`

3. **Proposal Generation**(`proposal-generation.ts`):生成技能修改提案
   - `stageSkillProposalGeneration()`:原子写入 generation draft(先 staging dir → move)
   - `SkillProposalStatus`:`"pending" | "applied" | "rejected" | "quarantined" | "stale"`
   - `MAX_SKILL_PROPOSAL_ORIGIN_RUN_IDS=4096`

4. **Autonomous Apply**(`autonomous-apply.ts`):自动应用提案
   - `applyAutonomousSkillProposal()`:workshop-owned 技能可直接应用
   - user-authored 技能 → `pending` 等待人工审核(`USER_AUTHORED_PENDING_REASON`)
   - `withSkillProposalCommitLock()` 保证原子性

5. **Collection Plan**(`collection-plan.ts`):技能集合规划
   - `validateSkillCollectionPlan()`:校验 write/drop/reason 完整性
   - 每个 agent 必须保留至少一个可见技能

**Workshop 类型**(workshop/types.ts):
- `SKILL_WORKSHOP_SCHEMA="openclaw.skill-workshop.proposal.v1"`
- `SkillWorkshopProposalMutationBudget`:run 级预算(remaining/completed/successfulMutations/failedMutations)
- `SkillProposalScan`:`pending/clean/failed/quarantined` + critical/warn/info findings

**安全机制**:
- `isWorkshopOwnedSkillDir()`:只有 workshop 创建的技能可自动修改
- `assertProposalId()`:提案 ID 格式校验
- `revisionHash` + `expectedRevisionHash`:乐观并发控制
- `SkillProposalSupportFile`:附件 hash 校验

---

## 总结:OpenClaw vs laew 的设计对照

| 维度 | OpenClaw | laew |
|------|----------|------|
| 多轮对话 | 双层 while 循环 + steering 队列 + follow-up | TUI REPL + Screen 栈 |
| Context | 内置 compaction + 可插拔 ContextEngine + quarantine 隔离 | 无 compaction,依赖模型上下文窗口 |
| Yolo/分类 | 无入口层分类(Yolo=exec 免审批) | Yolo Agent 三档分类 |
| 质检 | exec auto-reviewer + tool-loop detection | Quality-Check Agent 必过 |
| 任务拆解 | sessions_spawn + registry + announce | Main-Work → SubAgent-Work |
| 任务分类 | Swarm 并行 + session key 段标识 | simple/medium/hard 三档 |
| 工具调用 | agent-tools 装配 + tool-search 渐进披露 | Bash/Read/Write 三工具 |
| MCP | 双向(client+server)、3 传输、OAuth、分页 | 无 |
| SKILL | 52 内置 + workshop 自演化 + frontmatter 注入 | 无 |

**核心差异**:OpenClaw 是 **LLM 自主决策** 架构(模型决定拆解/工具/子 Agent),laew 是 **编排器驱动** 架构(MultiAgentOrchestrator 强制路由)。OpenClaw 的优势在灵活性(MCP/skill/workshop)和生态;laew 的优势在可控性(角色隔离/质量门控/确定性路由)。
