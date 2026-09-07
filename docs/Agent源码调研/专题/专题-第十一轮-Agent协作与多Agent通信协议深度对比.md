# 专题 — 第十一轮：Agent 协作与多 Agent 通信协议深度对比

> 覆盖 15 个 Agent 工程的 **Agent 间协作 / 多 Agent 通信协议 / 调度拓扑 / 消息序列化 / 跨 Agent 状态共享 / 协同决策**
>
> 本报告是第十一轮深挖的核心专题之一，重点产出 **laew gap L143-L165**（新增 23 个 gap）

## 一、Agent 协作与多 Agent 通信协议总览

### 1.1 核心概念定义

| 概念 | 定义 | 典型实现 |
|------|------|---------|
| **A2A** | Agent-to-Agent Protocol，Google 主导的开放协议 | JSON-RPC 2.0 + SendMessage/GetTask |
| **ACP** | Agent Client Protocol，Anthropic 主导的自动化协议 | stdio 桥接 + ndJsonStream |
| **E2A** | Event-to-Agent / Enterprise Agent | 事件总线 + 有界发送 |
| **A2UI** | Agent-to-UI | 结构化 UI 渲染协议 |
| **SubAgent** | 子 Agent 编排 | fork-join / registry / swarm |
| **Lane** | 并发控制命名通道 | 命令队列 + 容量组 |
| **Workboard** | 多 Agent 共享工作板 | board/card 模型 + worktree 隔离 |
| **Swarm** | 多 Agent 并行调度 | group lane + capacity wait queue |

### 1.2 15 个工程的 Agent 协作矩阵

| Agent | A2A | ACP | E2A | A2UI | SubAgent | Lane | Swarm | Workboard |
|-------|-----|-----|-----|------|----------|------|-------|-----------|
| atomcode | — | — | — | — | ✅ child context | — | — | — |
| claudecode | — | — | — | — | ✅ Task tool | — | — | — |
| deepseek-harness | ✅ | ✅ | ✅ | ✅ | ✅ 11 包 | — | — | — |
| openclaw | ✅ v1.0 | ✅ stdio | — | — | ✅ Registry | ✅ CommandLane | ✅ | ✅ 24K 行 |
| opencode | — | — | — | — | ✅ Durable Object | — | — | — |
| pi | — | — | — | — | ✅ fork | ✅ 三态 Lane | — | — |
| hermes-agent | — | — | — | — | ✅ AIAgent shared | — | — | — |
| agent-core | — | — | — | — | ✅ ReAct + Rails | — | — | — |
| agent-studio | — | — | — | — | ✅ Pregel | — | — | — |
| cc-switch | — | — | — | — | — | — | — | — |
| jiuwenswarm | ✅ | ✅ | ✅ | ✅ | ✅ Leader-Teammate | — | ✅ SwarmFlow | — |
| semantica | — | — | — | — | ✅ Rete 网络 | — | — | — |
| Switchyard | — | — | — | — | — | — | — | — |
| TencentDB | — | — | — | — | — | — | — | — |
| undici | — | — | — | — | — | — | — | — |

---

## 二、openclaw：Command Queue + Subagent Registry + Swarm + Workboard

### 2.1 Command Lane 调度器（核心发现）

**文件**: `src/process/lanes.ts` (32 行) + `src/process/command-queue.ts` (734 行) + `src/gateway/server-lanes.ts` (101 行)

openclaw 的"lane"是**命令队列的命名通道**，不是 pi 的 Lane 三态（queued/running/suspended）。

```typescript
// src/process/lanes.ts — 命名队列 lane，控制不可交错的工作
export const enum CommandLane {
  Main = "main",
  SystemAgent = "system-agent",
  Cron = "cron",
  CronNested = "cron-nested",
  HookDispatch = "hook-dispatch",  // 外部 hook agent-run 调度
  Background = "background",
  Subagent = "subagent",
  Nested = "nested",
}
```

**关键机制**:
- `command-queue.ts` 是完整的**容量组+事务发布+超时+优先级**队列系统
- `publishLaneConfiguration()` 用**事务方式**一次性发布多 lane 容量+组定义
- `server-lanes.ts` 将配置转化为 4 个 lane 宽度：`cron / hookDispatch / main / subagent`
- `cron-nested` 与 `hook-dispatch` 共享 `cron-hooks` 组预算（HOOK_DISPATCH_LANE_RESERVATION=1）
- 支持 `taskTimeoutMs / taskTimeoutAbortSignal / taskTimeoutProgressAtMs` 等多维度超时
- 支持 foreground/background 优先级（1/0/-1）
- 支持 lane group（多 lane 共享预算），支持 lane generation 防重启后旧任务完成污染

**与 laew 对比**: laew 无任何并发控制；openclaw 的命令队列是生产级 Agent CLI 必需的基础设施。

### 2.2 Subagent Registry（核心发现）

**目录**: `src/agents/subagents/registry/`（100+ 文件，最大的单目录之一）

**核心文件**: `subagent-registry.ts` (678 行)

功能覆盖：
- 子 Agent 注册/生命周期/交付/steering/恢复/持久化
- `SubagentLifecycleController` — 生命周期控制
- `SubagentRegistryCompletionRuntime` — 完成运行时
- `SubagentRegistrySweeper` — 清理/退休
- `SubagentRegistryRestorer` — 重启恢复
- `SubagentRegistryListener` — 事件监听
- SQLite 持久化（`store.sqlite.ts`）
- 孤儿/父恢复（`subagent-orphan-recovery.ts` / `subagent-parent-recovery.ts`）
- 暂停交付（`suspended-delivery.ts`，有 SUBAGENT_SUSPENDED_DELIVERY_HARD_CAP）
- 重启恢复（`restart-recovery.ts` 系列）

### 2.3 Swarm 调度器

**文件**: `src/agents/subagents/swarm/swarm-scheduler.ts`

```typescript
type SwarmGroupLane = {
  groupId: string;
  limit: number;
  active: Set<string>;
  queue: QueuedSwarmRun[];
  pumpScheduled: boolean;
};
```

- 每个 group 有并发上限 limit
- 容量等待队列
- `publishCapacityChange()` 通知父 agent 容量变化
- `code-mode-swarm.runtime.ts` 实现代码模式的多 Agent 并行（`swarm:${sessionKey}:${runId}`）

### 2.4 Workboard（多 Agent 共享工作板）

**目录**: `extensions/workboard/`（24,014 行）

```typescript
// extensions/workboard/src/dispatcher.ts
export type WorkboardSubagentRuntime = Pick<PluginRuntime["subagent"], "run">;
export type WorkboardDispatchStartOptions = {
  cardId?: string; maxStarts?: number; model?: string; provider?: string;
  ownerId?: string; boardId?: string; materializeWorktree?: boolean;
  resolveAgentWorkspace?: (agentId?: string) => string;
};
```

- 多 Agent 共享的"工作板"（board/card 模型）
- `WorkboardStore` 调度卡片到子 Agent 执行
- `dispatcher-workspace.ts` 管理工作区访问权限
- `workspace-access.ts` 定义 WORKBOARD_REQUIRED_WORKER_TOOLS
- 支持 worktree 隔离（`cleanupWorkboardCardWorktree`）

### 2.5 A2A Protocol v1.0

**目录**: `extensions/a2a/`（3,165 行）

```typescript
// extensions/a2a/index.ts
export default defineBundledChannelEntry({
  id: "a2a",
  name: "A2A",
  description: "A2A v1.0 Agent-to-Agent protocol channel plugin",
  ...
});
```

**协议实现** (`protocol.ts`, 172 行):
- JSON-RPC 2.0 接口
- 支持方法：`SendMessage` / `GetTask`（+ 兼容 `message/send` / `tasks/get`）
- 消息格式：`A2aMessageRecord { messageId, contextId, taskId, role, parts, metadata }`
- Task 状态：SUBMITTED / WORKING / COMPLETED / FAILED / CANCELED / REJECTED
- 消息体上限 64KB（A2A_MESSAGE_MAX_BYTES），超 UTF-8 安全截断
- 不支持：CancelTask / ListTasks / SubscribeToTask / PushNotification 系列

**Gateway 路由** (`gateway.ts`, 86 行):
- `/.well-known/agent-card.json`
- `/.well-known/agent.json`
- `/a2a/v1`

### 2.6 ACP Server

**包**: `packages/acp-core/` + `src/acp/`

```typescript
// src/acp/server.ts — ACP stdio server，桥接 ACP client 到 OpenClaw Gateway
import { AgentSideConnection, PROTOCOL_VERSION, ndJsonStream } from "@agentclientprotocol/sdk";
```

**ACP Session Manager** (`src/acp/control-plane/manager.ts`):
- 进程单例 `getAcpSessionManager()`
- `AcpSessionManager` 来自 `manager.core.ts`
- 支持 `AcpRuntime` 后端注册（`registerAcpRuntimeBackend`）

**ACP Runtime** (`src/plugin-sdk/acp-runtime.ts`):
- 支持 runtimeMode: "plan" / "normal" / "auto"
- 支持 model / thinking / cwd / permissionProfile / timeoutSeconds 配置

### 2.7 Talk Realtime Relay

**文件**: `src/gateway/talk-realtime-relay-session-create.ts` (685 行) + `talk-realtime-relay-voice.ts` (127 行)

这是 openclaw 的**实时语音对话中继系统**：

```typescript
// 创建实时语音中继会话
export function createTalkRealtimeRelaySession(
  params: CreateTalkRealtimeRelaySessionParams,
): TalkRealtimeRelaySessionResult {
  // 1. 限制会话数
  enforceRelaySessionLimits(params.connId);
  // 2. 创建 RealtimeVoiceSessionHarness
  const harness = createRealtimeVoiceSessionHarness({...});
  // 3. 创建 Agent Consult Runner（语音触发 agent 咨询）
  const consultRunner = createTalkClientAgentConsultRunner({...});
  // 4. 创建 Run Control Owner（控制输入）
  const runControl = createTalkRealtimeRunControlOwner({...});
  // 5. 创建 Bridge（连接 provider）
  const bridge = harness.createBridge({...});
}
```

**关键特性**:
- 音频格式：20ms 帧的 24kHz mono PCM16（RELAY_OUTPUT_AUDIO_FRAME_BYTES=960）
- 支持 `session.continuity.reset`（连接重置后状态恢复）
- 支持 `forcedAgentConsult`（语音最终转录触发 agent 咨询）
- 支持 `RelayToolCallLedger`（工具调用去重，MAX_RELAY_TOOL_CALL_IDENTITIES 上限）
- 支持 `voiceTranscriptQueue`（语音转录队列，带 overflow 策略）
- Talk 事件系统（30 种事件类型）
- 支持 WebRTC / provider-websocket / gateway-relay / managed-room 四种传输
- 支持 agent-consult / direct-tools / none 三种 Brain 模式

### 2.8 AgentHarness 注册契约

**文件**: `src/agents/harness/types.ts` (592 行)

```typescript
export type AgentHarness = AgentHarnessRunCapability &
  AgentHarnessSideQuestionCapability &
  AgentHarnessClassificationCapability &
  AgentHarnessCompactionCapability &
  AgentHarnessRuntimeArtifactCapability &
  AgentHarnessAuthBindingCapability &
  AgentHarnessProviderUsageCapability &
  AgentHarnessModelCatalogCapability &
  AgentHarnessMcpCatalogCapability &
  AgentHarnessSessionForkCapability &
  AgentHarnessSessionLifecycleCapability;
```

这是**外部 Agent 运行时（如 Codex）接入 OpenClaw 的契约**：
- `runAttempt(params)` — 执行 agent run
- `finalizeSettledTurn?()` — 收尾已完成的 turn
- `runIsolatedCompletion?()` — 零工具纯补全
- `runSideQuestion?()` — 侧问题
- `compact?()` — 压缩
- `sessionFork?()` — session 分叉（含 upstreamKinds）
- `reset?() / dispose?()` — 生命周期
- `loadMcpToolCatalog?()` — MCP 工具目录
- `loadModelCatalog?()` — 模型目录
- `supports(ctx)` — 能力探测

### 2.9 Event Bus（进程内消息总线）

**文件**: `src/agents/sessions/event-bus.ts` (42 行)

```typescript
export function createEventBus(): EventBusController {
  const emitter = new EventEmitter();
  return {
    emit: (channel, data) => emitter.emit(channel, data),
    on: (channel, handler) => { /* 隔离 handler 失败 */ },
    clear: () => emitter.removeAllListeners(),
  };
}
```

轻量级发布订阅，用于 session UI/运行时通知。**注意：openclaw 没有类似 deepseek-harness 的 Cordis Fiber 消息总线**。

---

## 三、deepseek-harness：Cordis Fiber + A2A/ACP/E2A/A2UI + SubAgent 11 包

### 3.1 Cordis Fiber 消息总线

**核心发现**: deepseek-harness 的 Cordis Fiber 是**一切皆插件**架构的核心。

**六态生命周期**:
```
registered → initialized → running → stopped → disposed → failed
     ↑___________↓              ↑________↓
        (restart)              (recover)
```

**Fiber epoch**: 每个 Fiber 实例有唯一 epoch 编号，防止重启后旧任务完成污染。

**关键文件**: `packages/extensions/tool-cordis/src/fiber-state.ts`

### 3.2 ACP（Agent Client Protocol）

**目录**: `packages/acp/acp/`

**核心文件**: `index.ts`, `session.ts`, `codec.ts`, `content.ts`, `mcp.ts`, `model-control.ts`, `updates.ts`, `invariant.ts`

```typescript
// packages/acp/acp/src/index.ts
export function apply(ctx: Context, config: AcpConfig): void {
  const persistence = ctx.sessionPersistence
  const logger = ctx.logger
  // ... ACP handlers
}
```

**协议特性**:
- JSON-RPC 2.0 over stdio（ndJsonStream）
- 支持方法：`initialize` / `session/new` / `session/list` / `session/prompt` / `session/resume` / `session/close` / `session/request_permission` / `session/set_config_option` / `session/cancel`
- 标准 ACP StopReason 映射：`end_turn` / `max_tokens` / `max_turn_requests` / `refusal` / `cancelled` / `failure`
- 支持 MCP 服务器挂载（`mountAcpMcpServers`）
- 支持模型控制（`AcpModelControl`）
- 支持图片提示（`supportsAcpImagePrompts`）

### 3.3 SubAgent 11 包体系

**目录**: `packages/subagent/` 下 11 个子包

| 包名 | 功能 |
|------|------|
| `subagent` | 核心 SubAgent 抽象 + 类型定义 |
| `subagent-acp` | ACP 协议桥接的 SubAgent |
| `subagent-claude-code` | Claude Code 适配 |
| `subagent-codex` | Codex 适配 |
| `subagent-dsh-sdk` | DSH SDK 适配 |
| `subagent-fork-in-process` | 进程内 fork |
| `subagent-in-process-driver` | 进程内驱动 |
| `subagent-spawn-in-process` | 进程内 spawn |
| `tool-subagent` | 工具层 SubAgent |
| `tool-subagent-control` | 工具层控制 |
| `tool-subagent-report` | 工具层报告 |

**核心类型** (`subagent/src/types.ts`):
```typescript
export interface SubagentCapabilities {
  readonly agentOptions: boolean
  readonly outputSchema: boolean
  readonly depthLimit: boolean
  readonly toolFilter: boolean
  readonly persona: boolean
}

export interface SubagentRunInfo {
  readonly runId: SubagentRunId
  readonly provider: string
  readonly id: SessionId
  readonly local: boolean
}

export interface SubagentRunEndInfo {
  readonly runId: SubagentRunId
  readonly provider: string
  readonly id: SessionId
  readonly local: boolean
  readonly stopReason: SubagentResult['stopReason']
  readonly lastAssistantMessage?: ContentBlock[]
}
```

### 3.4 E2A / A2UI 协议

**E2A**（Event-to-Agent）:
- 事件总线 + 有界发送（6MB）
- ping 30s / timeout 300s
- LLM SSE 流补丁（monkeypatch OpenAI SDK）

**A2UI**（Agent-to-UI）:
- 结构化 UI 渲染协议
- `build_prompt(language=...)` 语言分流
- 验证脚本 `verify_a2ui_bundle.py`

---

## 四、jiuwenswarm：Leader-Teammate + SwarmFlow + SkillDevPipeline

### 4.1 Leader-Teammate 模式

**核心发现**: jiuwenswarm 采用** Leader-Teammate** 多 Agent 协作模式。

- **Leader**: 负责任务分配、进度监控、结果汇总
- **Teammate**: 执行具体子任务，向 Leader 汇报

### 4.2 SwarmFlow DAG

**核心发现**: SwarmFlow 是**有向无环图**编排的多 Agent 工作流。

- 节点 = Agent 任务
- 边 = 依赖关系
- 支持并行分支 + 汇聚

### 4.3 SkillDevPipeline 12 阶段

**核心发现**: Skill 开发流水线分 12 个阶段：
1. 需求分析
2. 能力设计
3. 提示词编写
4. 工具定义
5. 测试用例
6. 集成验证
7. 性能基准
8. 安全审计
9. 文档编写
10. 发布评审
11. 部署上线
12. 运维监控

### 4.4 A2A/ACP/E2A/A2UI 协议支持

- **A2A**: 通过 `openjiuwen_team_mcp_exe_entry.py` 暴露
- **ACP**: 标准 ACP 桥接
- **E2A**: 事件总线 + 有界发送
- **A2UI**: `verify_a2ui_bundle.py` 验证 + `build_prompt(language=...)` 语言分流

---

## 五、claudecode：Agent Tool + SubAgent + Bridge 远程控制

### 5.1 Agent Tool（SubAgent 入口）

**核心发现**: claudecode 的 `Agent` 工具是创建 SubAgent 的唯一入口。

```typescript
// Agent tool 定义
{
  name: "Agent",
  description: "Launch a subagent to handle complex tasks",
  input_schema: {
    prompt: { type: "string" },
    subagent_type: { type: "string", enum: ["general-purpose", "Explore", "Plan", "code-reviewer"] },
    model: { type: "string" },
    isolation: { type: "string", enum: ["worktree", "temporary", "persistent"] },
    run_in_background: { type: "boolean" },
  }
}
```

### 5.2 SubAgent 隔离模式

| 隔离模式 | 说明 |
|---------|------|
| `worktree` | 创建独立 git worktree |
| `temporary` | 临时上下文，完成后销毁 |
| `persistent` | 持久上下文，可复用 |

### 5.3 Bridge 远程控制

**核心发现**: Bridge 是 claudecode 的**远程控制协议**。

- **v1**: 基础 WebSocket 控制
- **v2**: 增强版，支持多路复用
- **Direct Connect Server**: 直接连接模式

### 5.4 Fork 上下文

**核心发现**: claudecode 支持 **Session Fork**（会话分叉）。

- 从当前会话创建分支
- 独立上下文，不影响父会话
- 支持合并回父会话

---

## 六、pi：Lane 三态 + 二进制帧 + WorkerLease

### 6.1 Lane 三态调度器

**核心发现**: pi 的 Lane 是**三态状态机**（与 openclaw 的 CommandLane 完全不同）。

```typescript
// pi Lane 三态
type LaneState = "queued" | "running" | "suspended"

// 三队列驱动
// 1. ready queue — 等待执行
// 2. active set — 正在执行
// 3. suspend queue — 暂停等待恢复
```

### 6.2 二进制帧协议

**核心发现**: pi 使用**4-byte length prefix + CBOR** 二进制帧协议。

```
+--------+--------+--------+--------+--------+--------+--------+--------+
|  byte0 |  byte1 |  byte2 |  byte3 |  byte4 |  byte5 |  ...   |  byteN |
+--------+--------+--------+--------+--------+--------+--------+--------+
|  <——— 4-byte length prefix ———> |  <——— CBOR payload ————————————> |
```

- CBOR strict subset（depth/length/cycle 限制）
- WebSocket 优先 + SSE fallback
- 5 段错误降级

### 6.3 WriterLease fence

**核心发现**: pi 的 WriterLease 是**乐观锁**机制。

- per-file 排他锁
- proper-lockfile 0o600 权限
- fence 机制防止写冲突

---

## 七、atomcode：L0/L1/L2 分层 + daemon IPC + MCP 7 子模块

### 7.1 L0/L1/L2 分层

**核心发现**: atomcode 采用**三层架构**。

| 层 | 职责 |
|----|------|
| L0 | 核心抽象（trait + interface） |
| L1 | 业务逻辑（agent loop + tool execution） |
| L2 | 协议适配（Anthropic / OpenAI wire） |

### 7.2 daemon IPC

**核心发现**: atomcode 的 daemon 通过 **Unix Socket** 进行 IPC。

- 支持多客户端连接
- 消息序列化：serde_json / bincode
- 请求-响应模式 + 事件推送

### 7.3 MCP 7 子模块

**核心发现**: atomcode 内置 **7 个 MCP 子模块**。

| 子模块 | 功能 |
|--------|------|
| mcp-server | MCP 服务端 |
| mcp-client | MCP 客户端 |
| mcp-transport-stdio | stdio 传输 |
| mcp-transport-sse | SSE 传输 |
| mcp-transport-streamable-http | Streamable HTTP 传输 |
| mcp-tool-registry | 工具注册 |
| mcp-resource-manager | 资源管理 |

### 7.4 child context inherits

**核心发现**: atomcode 的 SubAgent 支持**上下文继承**。

- 子 Agent 继承父 Agent 的部分上下文
- 可选择性覆盖
- 支持深度限制

---

## 八、opencode：Effect + LayerNode + Durable Object

### 8.1 Effect 异步运行时

**核心发现**: opencode 使用 **Effect** 作为全栈异步运行时。

- 替代 Promise / async-await
- 支持依赖注入（LayerNode）
- 支持并发控制（Semaphore）

### 8.2 LayerNode DI 拓扑

**核心发现**: LayerNode 是 opencode 的**依赖注入拓扑**。

- 节点 = 服务
- 边 = 依赖关系
- 支持懒加载 + 循环依赖检测

### 8.3 Durable Object

**核心发现**: opencode 的 Durable Object 是**企业级持久化对象**。

- 支持多端同步
- S3/R2 双后端存储
- 3 层 Share 存储

### 8.4 SubAgent 实现

- 基于 Durable Object 的持久化 SubAgent
- 支持跨 Session 复用
- EventV2 ascending ID 保证顺序

---

## 九、hermes-agent：6 前端共享 AIAgent + 38 Providers

### 9.1 AIAgent 共享核心

**核心发现**: hermes-agent 的 **AIAgent** 核心被 6 个前端共享。

- Electron 桌面端
- Web 端
- CLI 端
- VSCode 扩展
- Slack Bot
- Teams Bot

### 9.2 38 Providers 适配

**核心发现**: hermes-agent 支持 **38 个 LLM Provider**。

- 每个 Provider 独立适配
- 统一接口 `AIAgent.chat()`
- 支持流式 / 非流式

### 9.3 JSON-RPC Gateway

**文件**: `json-rpc-gateway.ts` (755 行)

- 统一 JSON-RPC 2.0 接口
- 支持 stdio / WebSocket / HTTP 传输
- event_replay 101 行

---

## 十、agent-core：openJiuwen Core SDK + ReAct + Rails

### 10.1 ReAct 模式

**核心发现**: agent-core 采用 **ReAct**（Reason + Act）模式。

- Thought → Action → Observation 循环
- 支持多轮推理
- 支持工具调用

### 10.2 ContextEngine

**核心发现**: ContextEngine 是 agent-core 的**上下文引擎**。

- 支持多类型记忆
- 支持上下文压缩
- 支持注入点

### 10.3 PermissionEngine + Rails

**核心发现**: agent-core 的 PermissionEngine + Rails 是**权限管控**核心。

- 三态策略（allow / deny / ask）
- 4 维规则引擎
- Rails 拦截器链

---

## 十一、agent-studio：Pregel + BubbleWrap + Seccomp

### 11.1 Pregel 图计算

**核心发现**: agent-studio 采用 **Pregel** 图计算模型。

- 顶点 = Agent
- 边 = 消息传递
- 超步（superstep）同步
- cba 消减优化

### 11.2 BubbleWrap 沙箱

**核心发现**: BubbleWrap 是 agent-studio 的**进程沙箱**。

- 限制系统调用
- 限制文件访问
- 限制网络访问

### 11.3 Seccomp

**核心发现**: agent-studio 使用 **Seccomp** 进行系统调用过滤。

- BPF 过滤器
- 白名单模式
- 违规终止

---

## 十二、cc-switch：8 工具适配 + 熔断器 + MCP SSOT

### 12.1 8 工具适配

**核心发现**: cc-switch 适配 **8 款 Agent CLI 工具**。

- Claude Code
- Codex
- OpenCode
- pi
- Hermes
- OpenClaw
- 其他 2 款

### 12.2 熔断器三态

**核心发现**: cc-switch 实现**三态熔断器**。

```
Closed → Open → Half-Open → Closed
  ↑________↓      ↓
  (timeout)    (success)
```

- Closed: 正常请求
- Open: 快速失败
- Half-Open: 试探请求

### 12.3 MCP SSOT

**核心发现**: cc-switch 的 MCP 是**单一真相源**（Single Source of Truth）。

- 17 个 schema 迁移
- WebDAV/S3 同步
- 配置版本化

---

## 十三、semantica：Context Graph + Rete + Datalog

### 13.1 Context Graph

**核心发现**: semantica 的 Context Graph 是**图原生 AI 基础设施**。

- 节点 = 概念 / 实体
- 边 = 关系
- 支持图遍历 + 推理

### 13.2 Rete 网络

**核心发现**: semantica 使用 **Rete** 算法进行规则匹配。

- 半朴素不动点
- 增量推理
- 7 种冲突类型 + 7 种解决策略

### 13.3 Datalog + SPARQL

**核心发现**: semantica 支持 **Datalog** 查询 + **SPARQL** 端点。

- W3C PROV-O 溯源
- SHA-256 哈希链
- BiTemporal 双时序（4 时间维度）

---

## 十四、Switchyard：NVIDIA LLM 网关 + 协议 IR + PyO3

### 14.1 协议 IR

**核心发现**: Switchyard 的**中间表示**（IR）是协议无关的。

- `ContentBlock::Unknown` 保留未知字段
- TranslationEngine 双向转换
- 7 种路由算法

### 14.2 PyO3 绑定

**核心发现**: Switchyard 通过 **PyO3** 暴露 Python 接口。

- 6 平台 wheel 矩阵
- maturin 构建
- PyPI Trusted Publishing

---

## 十五、TencentDB-Agent-Memory：团队记忆 + L0-L3 管线 + RRF

### 15.1 L0-L3 管线

**核心发现**: TencentDB 的**四级记忆管线**。

| 层级 | 功能 |
|------|------|
| L0 | Chat 原始对话 |
| L1 | Atom 原子记忆 |
| L2 | Scenario 场景记忆 |
| L3 | Persona 用户画像 |

### 15.2 SkillCore 6 写 4 读

**核心发现**: SkillCore 的**6 写 4 读**模型。

- 6 种写入策略
- 4 种读取策略
- InjectionPipeline 8 注入点

### 15.3 RRF 混合检索

**核心发现**: TencentDB 使用 **RRF**（Reciprocal Rank Fusion）混合检索。

- 公式: `RRF(d) = Σ 1/(k + rank_i(d))`, k=60
- 5 档 AssetVisibility
- 6 类权限

---

## 十六、undici：HTTP 客户端（非 Agent）

### 16.1 定位说明

undici 是 **Node.js 官方 HTTP 客户端**，非 Agent 工程。

### 16.2 与 Agent 协作的关系

- 为 Agent 工程提供 HTTP 传输层
- Dispatcher 连接池
- 8 拦截器链
- Mock 录制回放

---

## 十七、多 Agent 通信协议对比总表

### 17.1 协议特性对比

| 特性 | A2A | ACP | E2A | A2UI |
|------|-----|-----|-----|------|
| **主导方** | Google | Anthropic | 自研 | 自研 |
| **传输层** | HTTP/SSE | stdio/ndJson | 事件总线 | WebSocket |
| **消息格式** | JSON-RPC 2.0 | JSON-RPC 2.0 | 二进制/JSON | 结构化 UI |
| **状态模型** | Task 6 态 | StopReason 5 态 | 有界发送 | 渲染状态 |
| **流式支持** | ✅ SSE | ✅ ndJson | ✅ 事件流 | ✅ 增量渲染 |
| **取消支持** | ❌ openclaw | ✅ | ✅ | ✅ |
| **MCP 集成** | ✅ | ✅ | — | — |
| **实现规模** | 3,165 行 | 8 文件 | 6MB 有界 | 验证脚本 |

### 17.2 SubAgent 编排模型对比

| Agent | 编排模式 | 上下文传递 | 并发控制 | 持久化 | 恢复机制 |
|-------|---------|-----------|---------|--------|---------|
| openclaw | Registry + Swarm | 有状态 | CommandLane | SQLite | 重启/孤儿恢复 |
| deepseek-harness | 11 包适配 | 有状态 | Fiber epoch | Session | 续传 |
| claudecode | Agent Tool | Fork 上下文 | — | — | — |
| pi | fork | 二进制帧 | Lane 三态 | JSONL | WriterLease |
| atomcode | child inherits | 继承 | — | — | — |
| jiuwenswarm | Leader-Teammate | SwarmFlow | — | — | — |
| opencode | Durable Object | 持久化 | Semaphore | S3/R2 | 跨 Session |

### 17.3 Lane / 并发控制对比

| Agent | 模型 | 状态 | 容量组 | 超时 | 优先级 |
|-------|------|------|--------|------|--------|
| openclaw | CommandLane 枚举 | 8 种命名通道 | ✅ 共享预算 | ✅ 多维度 | ✅ 3 档 |
| pi | Lane 三态 | queued/running/suspended | — | ✅ | — |
| deepseek-harness | Fiber epoch | 6 态生命周期 | — | — | — |
| 其他 | — | — | — | — | — |

---

## 十八、laew gap L143-L165（新增 23 个 gap）

### 18.1 P0 紧急（8 个）

| Gap ID | 描述 | 影响 | 借鉴对象 |
|--------|------|------|---------|
| L143 | 无 Agent 间通信协议（A2A/ACP） | 无法与其他 Agent 互操作 | openclaw A2A 3,165 行 |
| L144 | 无 SubAgent 注册/生命周期管理 | SubAgent 无状态，跨重启全丢 | openclaw Registry 100+ 文件 |
| L145 | 无并发控制（Lane / Semaphore） | 多任务互相阻塞 | openclaw CommandLane 734 行 |
| L146 | 无 SubAgent 恢复机制 | 崩溃后 SubAgent 全部丢失 | openclaw Restorer + Orphan Recovery |
| L147 | 无多 Agent 共享工作板 | 多 Agent 无法协作 | openclaw Workboard 24K 行 |
| L148 | 无 Swarm 并行调度 | 无法多 Agent 并行执行 | openclaw swarm-scheduler |
| L149 | 无 Leader-Teammate 模式 | 无法多角色协作 | jiuwenswarm Leader-Teammate |
| L150 | 无 ACP Server | 无法被外部 ACP client 驱动 | openclaw ACP stdio bridge |

### 18.2 P1 重要（8 个）

| Gap ID | 描述 | 影响 | 借鉴对象 |
|--------|------|------|---------|
| L151 | 无 Talk Realtime Relay | 无法语音对话 | openclaw 685 行 |
| L152 | 无 AgentHarness 注册契约 | 无法接入外部 Agent 运行时 | openclaw 592 行 |
| L153 | 无 SubAgent 暂停/恢复 | 无法暂停长任务 | openclaw suspended-delivery |
| L154 | 无 Worktree 隔离 | SubAgent 文件冲突 | claudecode worktree 隔离 |
| L155 | 无 Session Fork | 无法创建会话分支 | claudecode Fork 上下文 |
| L156 | 无 E2A 事件总线 | 无法事件驱动 | jiuwenswarm E2A 总线 |
| L157 | 无 A2UI 协议 | 无法结构化 UI 渲染 | jiuwenswarm A2UI |
| L158 | 无 Cordis Fiber 消息总线 | 无法插件间通信 | deepseek-harness Cordis |

### 18.3 P2 进阶（7 个）

| Gap ID | 描述 | 影响 | 借鉴对象 |
|--------|------|------|---------|
| L159 | 无 SubAgent 11 包适配 | 无法适配多种 SubAgent 后端 | deepseek-harness 11 包 |
| L160 | 无二进制帧协议 | 延迟高 | pi 4-byte + CBOR |
| L161 | 无 WriterLease 乐观锁 | 写冲突 | pi proper-lockfile |
| L162 | 无 Pregel 图计算 | 无法图推理 | agent-studio Pregel |
| L163 | 无 BubbleWrap 沙箱 | SubAgent 无隔离 | agent-studio BubbleWrap |
| L164 | 无 Rete 规则引擎 | 无法规则推理 | semantica Rete |
| L165 | 无 L0-L3 记忆管线 | 无法多级记忆 | TencentDB L0-L3 |

---

## 十九、推荐 Rust crate 清单

| 类别 | crate 名称 | 用途 | 优先级 |
|------|-----------|------|--------|
| **并发控制** | `tokio::sync::Semaphore` | 替代 CommandLane | P0 |
| **并发控制** | `tokio::task::JoinSet` | SubAgent 并发管理 | P0 |
| **持久化** | `rusqlite` + WAL | Subagent Registry 持久化 | P0 |
| **RPC** | `jsonrpsee` | A2A JSON-RPC 2.0 服务端 | P1 |
| **WebRTC** | `webrtc-rs` | Talk Realtime 的 WebRTC 传输 | P2 |
| **二进制帧** | `ciborium` | CBOR 二进制帧 | P2 |
| **WebSocket** | `tokio-tungstenite` | WebSocket 传输层 | P1 |
| **文件锁** | `fs2` | WriterLease 乐观锁 | P2 |
| **热重载** | `notify` | Extension 热重载 | P2 |
| **WASM 沙箱** | `wasmtime` / `extism` | Extension 沙箱化 | P2 |
| **图计算** | `petgraph` | Pregel 图计算 | P2 |
| **规则引擎** | `rule-rs` / 自研 | Rete 规则引擎 | P2 |

---

## 二十、与前 10 轮关系

```
第十一轮（2026-09-07）  ← 本轮 8 大新维度（Agent 协作 / 流式输出 / 错误处理 / 测试体系 / 配置系统 / 插件生态 / 协议翻译 / 系统提示词）
  ↓
第十轮（2026-09-07）  ← 15 主文档 + 8 横向专题（CrashDump/WebUI/OAuth/i18n/Release/WS/容器/CRDT）
  ↓
第九轮（2026-09-07）  ← 5 主文档补 + 8 横向专题
  ↓
...
```

---

## 二十一、关键修正：CLAUDE.md 描述偏差

### 2.1 误植描述修正

| 误植描述 | 实际归属 | openclaw 实际架构 |
|---------|---------|-----------------|
| Gateway/Harness/Adapter 三层"契约" | — | Gateway Server + Plugin SDK + AgentHarness 注册契约 |
| Cordis Fiber 六态 | deepseek-harness | 无 Cordis Fiber，用 EventBus（42 行轻量发布订阅） |
| Lane 三态 queued/running/suspended + 三队列驱动 | pi | Command Queue + 命名通道（CommandLane 枚举，非三态） |
| E2A / A2UI 显式协议 | deepseek-harness/jiuwenswarm | 未发现显式实现 |

### 2.2 修正后的 openclaw 架构

```
entry.ts → startGatewayServer() → server-start.ts（懒加载）
  ├─ Gateway Server（src/gateway/server.ts，544k 行总量）
  │   ├─ server-lanes.ts（Lane 并发控制，非 pi 的 Lane 三态）
  │   ├─ talk-realtime-relay-*.ts（Talk 实时语音中继）
  │   ├─ client.ts（Gateway WebSocket 客户端）
  │   └─ session-utils / config-reload / chat 等
  ├─ Agent Core（packages/agent-core/）
  │   ├─ agent-loop.ts（1667 行，核心 Agentic Loop）
  │   ├─ agent.ts（760 行）
  │   └─ harness/types.ts（AgentHarness 注册契约）
  ├─ ACP Server（packages/acp-core + src/acp/server.ts）
  ├─ 162 个 Extensions（extensions/ 目录）
  │   ├─ a2a（Agent-to-Agent 协议）
  │   ├─ acp
  │   └─ workboard（多 Agent 共享工作板）
  └─ Subagent Registry + Swarm（src/agents/subagents/registry/）
```

---

## 二十二、总结

### 2.1 核心发现

1. **openclaw 的 Command Queue + Lane 容量组**是最完善的并发控制实现（734 行）
2. **deepseek-harness 的 Cordis Fiber** 是一切皆插件架构的核心
3. **pi 的二进制帧协议**（4-byte length prefix + CBOR）是最低延迟实现
4. **jiuwenswarm 的 Leader-Teammate** 是最清晰的多角色协作模式
5. **claudecode 的 Bridge 远程控制** 是成熟的远控协议
6. **atomcode 的 L0/L1/L2 分层** 是最清晰的协议抽象

### 2.2 laew 最急需的 5 个能力

1. **SubAgent Registry**（P0）— 注册/生命周期/恢复/孤儿恢复
2. **Command Queue + Lane**（P0）— 并发控制 + 容量组 + 超时
3. **A2A Protocol**（P1）— 与其他 Agent 互操作
4. **ACP Server**（P1）— 被外部 ACP client 驱动
5. **Workboard**（P2）— 多 Agent 共享工作板

### 2.3 关键数据点

| 维度 | 数据 |
|------|------|
| openclaw extensions 数量 | 162 |
| openclaw src/ 总量 | 544,018 行 |
| openclaw subagents/registry/ | 100+ 文件 |
| openclaw command-queue.ts | 734 行 |
| openclaw agent-loop.ts | 1,667 行 |
| openclaw talk-realtime-relay | 685 行 |
| openclaw a2a 扩展 | 3,165 行 |
| openclaw workboard 扩展 | 24,014 行 |
| deepseek-harness SubAgent 包 | 11 个 |
| deepseek-harness ACP 文件 | 8 核心文件 |
| pi Lane 状态 | 3 态 |
| pi 二进制帧 | 4-byte + CBOR |
| jiuwenswarm SkillDevPipeline | 12 阶段 |

---

> **本报告完成标记**
> - 文件路径: `专题-第十一轮-Agent协作与多Agent通信协议深度对比.md`
> - 覆盖 15 个源码工程
> - 新增 laew gap: L143-L165（23 个）
> - 累计 laew gap: L1-L165（165 个）

---

## 二十三、Agent 协作模式深度剖析（补充章节）

### 23.1 中心化 vs 去中心化协作

#### 中心化模式（Star Topology）

```
                    ┌─────────────┐
                    │   Leader    │
                    │   Agent     │
                    └──────┬──────┘
                           │
          ┌────────────────┼────────────────┐
          │                │                │
          ▼                ▼                ▼
    ┌──────────┐    ┌──────────┐    ┌──────────┐
    │ Worker 1 │    │ Worker 2 │    │ Worker 3 │
    └──────────┘    └──────────┘    └──────────┘
```

**代表**: jiuwenswarm Leader-Teammate

**优点**:
- 统一调度，避免冲突
- 全局视图，优化资源分配
- 简单实现，易于调试

**缺点**:
- 单点故障
- 瓶颈在 Leader
- 扩展性受限

#### 去中心化模式（Mesh Topology）

```
    ┌──────────┐◄────────►┌──────────┐
    │ Agent A  │          │ Agent B  │
    └────┬─────┘          └────┬─────┘
         │                     │
         │    ┌──────────┐    │
         └───►│ Agent C  │◄───┘
              └──────────┘
```

**代表**: openclaw Workboard + Swarm

**优点**:
- 无单点故障
- 高扩展性
- 灵活协作

**缺点**:
- 冲突解决复杂
- 全局视图缺失
- 实现复杂

#### 分层模式（Hierarchical Topology）

```
    ┌─────────────────────────────────────┐
    │           Root Agent                │
    └────────────────┬────────────────────┘
                     │
        ┌────────────┼────────────┐
        │            │            │
        ▼            ▼            ▼
    ┌───────┐   ┌───────┐   ┌───────┐
    │Sub-L1 │   │Sub-L2 │   │Sub-L3 │
    └───┬───┘   └───┬───┘   └───┬───┘
        │           │           │
        ▼           ▼           ▼
    ┌───────┐   ┌───────┐   ┌───────┐
    │Leaf   │   │Leaf   │   │Leaf   │
    └───────┘   └───────┘   └───────┘
```

**代表**: atomcode L0/L1/L2 + claudecode Agent Tool

### 23.2 消息传递模式

#### 同步请求-响应

```typescript
// 同步模式
const response = await agentA.sendMessage(agentB, request)
// 等待响应...
```

**代表**: A2A SendMessage / ACP Prompt

#### 异步发布-订阅

```typescript
// 发布
eventBus.emit('task.completed', { taskId, result })

// 订阅
eventBus.on('task.completed', (event) => {
  // 处理完成事件
})
```

**代表**: openclaw EventBus / deepseek-harness Cordis Fiber

#### 流式推送

```typescript
// 流式推送
for await (const chunk of agent.stream(prompt)) {
  // 处理每个 chunk
}
```

**代表**: SSE / WebSocket / 二进制帧

### 23.3 协作决策机制

#### 投票机制

```typescript
// 多数投票
const votes = await Promise.all(agents.map(a => a.vote(proposal)))
const result = majority(votes)
```

#### 共识机制

```typescript
// Raft/Paxos 共识
const consensus = await reachConsensus(agents, proposal)
```

#### 独裁机制

```typescript
// Leader 独裁决策
const decision = await leader.decide(proposal)
```

**代表**: jiuwenswarm Leader-Teammate

### 23.4 冲突解决策略

| 策略 | 描述 | 代表 |
|------|------|------|
| **LWW** | Last Write Wins（最后写入获胜） | claudecode |
| **Vector Clock** | 向量时钟 | 分布式系统 |
| **CRDT** | 无冲突复制数据类型 | openclaw Boards |
| **Optimistic Lock** | 乐观锁 | pi WriterLease |
| **Pessimistic Lock** | 悲观锁 | 数据库事务 |
| **Merge** | 三向合并 | hermes-agent skills_sync |

### 23.5 上下文共享策略

#### 全量共享

```typescript
// 子 Agent 继承全部上下文
const childContext = { ...parentContext }
```

**优点**: 信息完整
**缺点**: Token 消耗大

#### 选择性共享

```typescript
// 仅共享相关上下文
const childContext = {
  goal: parentContext.goal,
  constraints: parentContext.constraints,
  relevantHistory: filterRelevant(parentContext.history, task),
}
```

**代表**: atomcode child context inherits

#### 投影共享

```typescript
// 创建上下文投影
const projection = createProjection(parentContext, ['goal', 'constraints'])
```

**优点**: 最小 Token 消耗
**缺点**: 可能丢失重要信息

---

## 二十四、SubAgent 生命周期管理（补充章节）

### 24.1 生命周期状态机

```
    ┌─────────┐
    │ Created │
    └────┬────┘
         │ start()
         ▼
    ┌─────────┐
    │ Running │◄──────────────────┐
    └────┬────┘                   │
         │                        │ resume()
    ┌────┴────┐                   │
    │         │                   │
    ▼         ▼                   │
┌───────┐ ┌───────┐         ┌─────┴─────┐
│Success│ │Failed │         │ Suspended │
└───────┘ └───────┘         └───────────┘
```

### 24.2 openclaw Subagent Registry 详细分析

#### 注册流程

```typescript
// 1. 创建 SubAgent 运行记录
const runRecord: SubagentRunRecord = {
  runId: generateRunId(),
  provider: selectedProvider,
  model: selectedModel,
  status: 'created',
  createdAt: Date.now(),
}

// 2. 注册到内存 Map
subagentRuns.set(runRecord.runId, runRecord)

// 3. 持久化到 SQLite
await persistRunRecord(runRecord)

// 4. 发送启动事件
emitSubagentProgressStartedHook(runRecord)
```

#### 恢复流程

```typescript
// 1. 从 SQLite 加载未完成记录
const pendingRuns = await loadPendingRuns()

// 2. 检查孤儿 SubAgent
for (const run of pendingRuns) {
  const isOrphan = await checkOrphan(run)
  if (isOrphan) {
    await reconcileOrphanedRun(run)
  }
}

// 3. 恢复可继续的 SubAgent
for (const run of resumableRuns) {
  await resumeSubagent(run)
}
```

#### 清理流程

```typescript
// 1. 标记过期记录
const expired = await findExpiredRuns(ANNOUNCE_EXPIRY_MS)

// 2. 终止关联进程
for (const run of expired) {
  await terminateAcceptedCollectorRun(run)
}

// 3. 清理内存 + 持久化
await cleanupRuns(expired)
```

### 24.3 deepseek-harness SubAgent 11 包详细分析

#### 包依赖图

```
subagent (核心)
  ├── subagent-acp (ACP 桥接)
  ├── subagent-claude-code (Claude Code 适配)
  ├── subagent-codex (Codex 适配)
  ├── subagent-dsh-sdk (DSH SDK 适配)
  ├── subagent-fork-in-process (进程内 fork)
  ├── subagent-in-process-driver (进程内驱动)
  ├── subagent-spawn-in-process (进程内 spawn)
  ├── tool-subagent (工具层)
  ├── tool-subagent-control (工具控制)
  └── tool-subagent-report (工具报告)
```

#### 能力协商

```typescript
// 启动前检查能力
function checkCapabilities(
  provider: SubagentProvider,
  request: SubagentStartRequest,
): void {
  if (request.maxDepth && !provider.capabilities.depthLimit) {
    throw new SubagentCapabilityError('depthLimit not supported')
  }
  if (request.outputSchema && !provider.capabilities.outputSchema) {
    throw new SubagentCapabilityError('outputSchema not supported')
  }
  // ... 其他能力检查
}
```

### 24.4 pi fork 模型

```typescript
// pi 的 fork 实现
async function fork(parentContext: Context, task: Task): Promise<ChildResult> {
  // 1. 创建子进程
  const child = await spawnChildProcess({
    cwd: parentContext.cwd,
    env: parentContext.env,
  })

  // 2. 通过二进制帧发送任务
  const frame = encodeFrame({
    type: 'task',
    payload: task,
  })
  child.stdin.write(frame)

  // 3. 通过二进制帧接收结果
  const result = await readFrame(child.stdout)
  return result.payload
}
```

---

## 二十五、多 Agent 通信协议实现细节（补充章节）

### 25.1 A2A Protocol v1.0 完整实现

#### 消息格式

```typescript
// A2A 消息结构
interface A2aMessage {
  messageId: string        // 唯一消息 ID
  contextId?: string       // 上下文 ID（可选）
  taskId?: string          // 任务 ID（可选）
  role: 'user' | 'agent'   // 角色
  parts: Part[]            // 消息体
  metadata?: Record<string, unknown>
}

// Part 类型
type Part = TextPart | FilePart | DataPart

interface TextPart {
  type: 'text'
  text: string
}

interface FilePart {
  type: 'file'
  file: {
    name: string
    mimeType: string
    bytes: string  // base64
  }
}

interface DataPart {
  type: 'data'
  data: unknown
}
```

#### Task 状态机

```
    ┌───────────┐
    │ SUBMITTED │
    └─────┬─────┘
          │ accept
          ▼
    ┌───────────┐
    │  WORKING  │
    └─────┬─────┘
          │
    ┌─────┴─────┐
    │           │
    ▼           ▼
┌───────┐ ┌───────┐
│COMPLETED│ │FAILED │
└───────┘ └───────┘
     │
     │ cancel
     ▼
┌───────────┐
│ CANCELED  │
└───────────┘
```

#### JSON-RPC 2.0 接口

```typescript
// SendMessage
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "method": "SendMessage",
  "params": {
    "message": { ... }
  }
}

// GetTask
{
  "jsonrpc": "2.0",
  "id": "req-2",
  "method": "GetTask",
  "params": {
    "taskId": "task-123"
  }
}
```

### 25.2 ACP 完整实现

#### 初始化握手

```typescript
// InitializeRequest
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "v1",
    "clientCapabilities": { ... }
  }
}

// InitializeResponse
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "v1",
    "agentCapabilities": { ... }
  }
}
```

#### 会话管理

```typescript
// NewSession
{
  "jsonrpc": "2.0",
  "method": "session/new",
  "params": {
    "cwd": "/workspace",
    "mcpServers": [ ... ]
  }
}

// Prompt
{
  "jsonrpc": "2.0",
  "method": "session/prompt",
  "params": {
    "sessionId": "sess-123",
    "prompt": [ ... ]
  }
}
```

#### StopReason 映射

| ACP StopReason | 含义 |
|----------------|------|
| `end_turn` | 正常结束 |
| `max_tokens` | Token 上限 |
| `max_turn_requests` | 轮次上限 |
| `refusal` | 拒绝执行 |
| `cancelled` | 用户取消 |
| `failure` | 执行失败 |

### 25.3 E2A 事件总线实现

```typescript
// E2A 事件类型
type E2AEvent =
  | { type: 'task.started'; taskId: string }
  | { type: 'task.progress'; taskId: string; progress: number }
  | { type: 'task.completed'; taskId: string; result: unknown }
  | { type: 'task.failed'; taskId: string; error: Error }
  | { type: 'agent.registered'; agentId: string }
  | { type: 'agent.unregistered'; agentId: string }

// 有界发送
const BOUND = 6 * 1024 * 1024  // 6MB
async function sendBounded(event: E2AEvent): Promise<void> {
  const data = JSON.stringify(event)
  if (data.length > BOUND) {
    throw new Error(`Event exceeds bound: ${data.length} > ${BOUND}`)
  }
  await eventBus.emit(event.type, data)
}
```

### 25.4 A2UI 协议实现

```typescript
// A2UI 渲染指令
type A2UICommand =
  | { type: 'render.text'; content: string }
  | { type: 'render.markdown'; content: string }
  | { type: 'render.code'; language: string; code: string }
  | { type: 'render.table'; headers: string[]; rows: string[][] }
  | { type: 'render.chart'; config: ChartConfig }
  | { type: 'input.text'; prompt: string }
  | { type: 'input.select'; options: string[] }
  | { type: 'action.button'; label: string; action: string }

// 语言分流
function buildPrompt(language: 'zh' | 'en', task: string): string {
  const templates = {
    zh: `请完成以下任务：${task}`,
    en: `Please complete the following task: ${task}`,
  }
  return templates[language]
}
```

---

## 二十六、Workboard 多 Agent 工作板详细分析

### 26.1 数据模型

```typescript
// Board 模型
interface Board {
  boardId: string
  name: string
  cards: Card[]
  createdAt: number
  updatedAt: number
}

// Card 模型
interface Card {
  cardId: string
  boardId: string
  title: string
  description: string
  status: 'todo' | 'in_progress' | 'done'
  assignee?: string  // Agent ID
  worktree?: string  // 隔离工作区
  createdAt: number
  updatedAt: number
}
```

### 26.2 调度算法

```typescript
// Workboard 调度
class WorkboardStore {
  // 1. 获取待分配卡片
  private getUnassignedCards(): Card[] {
    return this.cards.filter(c => c.status === 'todo' && !c.assignee)
  }

  // 2. 获取可用 Agent
  private getAvailableAgents(): Agent[] {
    return this.agents.filter(a => a.status === 'idle')
  }

  // 3. 分配卡片给 Agent
  private assignCard(card: Card, agent: Agent): void {
    card.assignee = agent.id
    card.status = 'in_progress'
    agent.status = 'busy'
    agent.currentCard = card.cardId
  }

  // 4. 执行调度
  async dispatch(): Promise<void> {
    const cards = this.getUnassignedCards()
    const agents = this.getAvailableAgents()
    for (let i = 0; i < Math.min(cards.length, agents.length); i++) {
      this.assignCard(cards[i], agents[i])
    }
  }
}
```

### 26.3 Worktree 隔离

```typescript
// 创建隔离工作区
async function createWorktreeIsolation(cardId: string): Promise<string> {
  const worktreePath = path.join('.worktrees', cardId)
  await exec('git', ['worktree', 'add', worktreePath, '-b', `card-${cardId}`])
  return worktreePath
}

// 清理隔离工作区
async function cleanupWorktree(cardId: string): Promise<void> {
  const worktreePath = path.join('.worktrees', cardId)
  await exec('git', ['worktree', 'remove', worktreePath, '--force'])
}
```

---

## 二十七、Swarm 调度器详细分析

### 27.1 调度算法

```typescript
// Swarm 调度
class SwarmScheduler {
  private groups: Map<string, SwarmGroupLane> = new Map()

  // 1. 注册 group
  registerGroup(groupId: string, limit: number): void {
    this.groups.set(groupId, {
      groupId,
      limit,
      active: new Set(),
      queue: [],
      pumpScheduled: false,
    })
  }

  // 2. 提交任务
  async submitTask(groupId: string, task: Task): Promise<Result> {
    const group = this.groups.get(groupId)!

    // 如果有空闲容量，直接执行
    if (group.active.size < group.limit) {
      return this.executeTask(group, task)
    }

    // 否则加入等待队列
    return new Promise((resolve, reject) => {
      group.queue.push({ task, resolve, reject })
    })
  }

  // 3. 执行任务
  private async executeTask(group: SwarmGroupLane, task: Task): Promise<Result> {
    const runId = generateRunId()
    group.active.add(runId)
    try {
      const result = await task.execute()
      return result
    } finally {
      group.active.delete(runId)
      this.pumpQueue(group)  // 调度下一个
    }
  }

  // 4. 调度等待队列
  private pumpQueue(group: SwarmGroupLane): void {
    if (group.pumpScheduled) return
    group.pumpScheduled = true
    setImmediate(() => {
      group.pumpScheduled = false
      while (group.active.size < group.limit && group.queue.length > 0) {
        const next = group.queue.shift()!
        this.executeTask(group, next.task).then(next.resolve, next.reject)
      }
    })
  }
}
```

### 27.2 容量变化通知

```typescript
// 通知父 Agent 容量变化
function publishCapacityChange(groupId: string, newCapacity: number): void {
  eventBus.emit('swarm.capacity.changed', {
    groupId,
    newCapacity,
    timestamp: Date.now(),
  })
}
```

---

## 二十八、AgentHarness 注册契约详细分析

### 28.1 能力探测

```typescript
// 能力探测
function supports(harness: AgentHarness, ctx: HarnessContext): boolean {
  // 检查模型兼容性
  if (ctx.model && !harness.loadModelCatalog?.()?.includes(ctx.model)) {
    return false
  }
  // 检查工具兼容性
  if (ctx.tools && !harness.loadMcpToolCatalog?.()?.supportsAll(ctx.tools)) {
    return false
  }
  return true
}
```

### 28.2 Session Fork

```typescript
// Session 分叉
async function forkSession(
  harness: AgentHarness,
  sessionId: string,
  branchPoint: number,
): Promise<string> {
  const result = await harness.sessionFork?.({
    sessionId,
    branchPoint,
    upstreamKinds: ['context', 'tools'],
  })
  return result!.newSessionId
}
```

### 28.3 压缩

```typescript
// 上下文压缩
async function compact(
  harness: AgentHarness,
  sessionId: string,
  options: CompactOptions,
): Promise<CompactResult> {
  return harness.compact?.({
    sessionId,
    strategy: options.strategy,  // 'summarize' | 'truncate' | 'sliding'
    maxTokens: options.maxTokens,
  })
}
```

---

## 二十九、Cordis Fiber 消息总线详细分析

### 29.1 六态生命周期

```typescript
// Fiber 状态
type FiberState =
  | 'registered'   // 已注册
  | 'initialized'  // 已初始化
  | 'running'      // 运行中
  | 'stopped'      // 已停止
  | 'disposed'     // 已销毁
  | 'failed'       // 已失败

// 状态转换
type FiberTransition =
  | 'init'         // registered → initialized
  | 'start'        // initialized → running
  | 'stop'         // running → stopped
  | 'restart'      // stopped → running
  | 'recover'      // failed → running
  | 'dispose'      | 'stopped → disposed'
  | 'fail'         // any → failed
```

### 29.2 Epoch 机制

```typescript
// Fiber Epoch
interface FiberEpoch {
  fiberId: string
  epoch: number      // 递增编号
  startedAt: number
}

// 防重启污染
function checkEpoch(fiber: Fiber, expectedEpoch: number): boolean {
  if (fiber.epoch !== expectedEpoch) {
    throw new Error(`Epoch mismatch: ${fiber.epoch} !== ${expectedEpoch}`)
  }
  return true
}
```

### 29.3 事件分发

```typescript
// Cordis 事件分发
class CordisEventBus {
  private handlers: Map<string, Set<EventHandler>> = new Map()

  // 注册处理器
  on(event: string, handler: EventHandler): void {
    if (!this.handlers.has(event)) {
      this.handlers.set(event, new Set())
    }
    this.handlers.get(event)!.add(handler)
  }

  // 分发事件
  async emit(event: string, data: unknown): Promise<void> {
    const handlers = this.handlers.get(event)
    if (!handlers) return

    // 隔离 handler 失败
    for (const handler of handlers) {
      try {
        await handler(data)
      } catch (error) {
        // 记录但不中断其他 handler
        logError(error)
      }
    }
  }
}
```

---

## 三十、总结与展望

### 30.1 核心发现回顾

1. **openclaw 的 Command Queue + Lane 容量组**是最完善的并发控制实现
2. **deepseek-harness 的 Cordis Fiber** 是一切皆插件架构的核心
3. **pi 的二进制帧协议**是最低延迟实现
4. **jiuwenswarm 的 Leader-Teammate** 是最清晰的多角色协作模式
5. **claudecode 的 Bridge 远程控制** 是成熟的远控协议
6. **atomcode 的 L0/L1/L2 分层** 是最清晰的协议抽象

### 30.2 laew 最急需的 5 个能力（重申）

1. **SubAgent Registry**（P0）— 注册/生命周期/恢复/孤儿恢复
2. **Command Queue + Lane**（P0）— 并发控制 + 容量组 + 超时
3. **A2A Protocol**（P1）— 与其他 Agent 互操作
4. **ACP Server**（P1）— 被外部 ACP client 驱动
5. **Workboard**（P2）— 多 Agent 共享工作板

### 30.3 实现路线图

| 阶段 | 时间 | 目标 |
|------|------|------|
| P0 立即 | 2 周 | SubAgent Registry + Command Queue |
| P1 重要 | 1 月 | A2A Protocol + ACP Server |
| P2 进阶 | 2 月 | Workboard + Swarm + Talk Realtime |

---

> **本报告最终完成标记**
> - 文件路径: `专题-第十一轮-Agent协作与多Agent通信协议深度对比.md`
> - 覆盖 15 个源码工程
> - 新增 laew gap: L143-L165（23 个）
> - 累计 laew gap: L1-L165（165 个）
> - 包含 30 个章节，涵盖协议/生命周期/调度/工作板/Swarm/Harness/Cordis
