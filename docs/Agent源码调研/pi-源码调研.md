# Pi 源码调研报告

> 调研对象:`/usr/local/LsmGitOpenSource/pi`(mono-repo,工作区名 `pi-monorepo`)
> 调研日期:2026-09-04
> 调研目的:对比 LsmAgentEmergentWork(YoloAgent,Rust)与 Pi Agent 的架构差异

---

## 1. 项目元信息

| 项 | 值 |
|---|---|
| 仓库名 | `pi`(earendil-works 自研) |
| 根 `package.json` | `pi-monorepo`,`private: true`,`type: module`,`version 0.0.3` |
| 语言 | TypeScript(Node ≥ 22.19) |
| 包管理 | npm + workspaces(`save-exact=true`,`min-release-age=2`) |
| 单测 | Vitest + Node `--test`(见 `test.sh`) |
| Lint/Format | Biome 2.3.5(`biome check --write --error-on-warnings`) |
| 类型检查 | `tsgo --noEmit`(TypeScript 7.0 native preview) |
| Release 工具 | Bun 编译 standalone,`scripts/build-binaries.sh` |
| 依赖锁定 | `package-lock.json` 是 ground truth,`coding-agent/npm-shrinkwrap.json` 给 npm 用户用 |
| 顶层入口 | `packages/coding-agent/src/main.ts`(CLI),`packages/coding-agent/src/index.ts`(SDK) |
| 仓库类型 | npm workspaces(monorepo) |
| Node 要求 | `engines.node >= 22.19.0` |

构建脚本(`package.json`)按依赖顺序编译:`tui → telemetry → ai → agent → session-backends/sqlite-node → protocol → client → server → coding-agent`,共 12 个子包。

---

## 2. 目录树(顶层包)

```
pi/
├── packages/
│   ├── ai/                       @earendil-works/pi-ai          多 Provider LLM 统一 API
│   ├── agent/                    @earendil-works/pi-agent-core  Agent 运行时
│   ├── coding-agent/             @earendil-works/pi-coding-agent 交互式 CLI
│   ├── tui/                      @earendil-works/pi-tui         TUI 渲染库(差异渲染)
│   ├── telemetry/                @earendil-works/pi-telemetry   Telemetry contracts
│   ├── protocol/                 JSONL / RPC 协议
│   ├── client/                   RPC client
│   ├── server/                   RPC server(对应 client)
│   ├── session-backends/
│   │   └── sqlite-node/          SQLite session 后端
│   └── evals/                    @earendil-works/pi-evals       评测
├── scripts/                      构建/发布/check 脚本
├── .pi/                          仓库自身的 pi 配置
└── pi-test.sh / pi-test.bat      从源码运行 pi 的 launcher
```

### 核心包结构

**`packages/ai/src/`**(多 Provider LLM 抽象,944+866 行核心)

```
api/                   各 Provider 协议适配
  anthropic-messages.ts, openai-responses.ts, openai-completions.ts,
  google-generative-ai.ts, google-vertex.ts, bedrock-converse-stream.ts,
  azure-openai-responses.ts, openai-codex-responses.ts, pi-messages.ts,
  lazy.ts, ...
auth/                  认证/凭据存储
  types.ts, context.ts, credential-store.ts, resolve.ts,
  oauth/             OAuth 实现(anthropic 等)
compat.ts             兼容层(streamSimple 等)
providers/            30+ Provider 工厂
  anthropic.ts, openai.ts, google.ts, bedrock.ts,
  openai-codex.ts, github-copilot.ts, openrouter.ts, xai.ts,
  groq.ts, deepseek.ts, cerebras.ts, together.ts, fireworks.ts,
  baseten.ts, nvidia.ts, huggingface.ts, vercel-ai-gateway.ts,
  cloudflare-workers-ai.ts, opencode.ts, opencode-go.ts,
  kimi-coding.ts, moonshotai.ts, moonshotai-cn.ts, minimax.ts,
  minimax-cn.ts, ant-ling.ts, mistral.ts, xiaomi.ts, zai.ts, ...
images-api-registry.ts / images.ts / images-models.ts
models.ts / models-store.ts / models.generated.ts / model-catalog.ts
oauth.ts / bun-oauth.ts
session-resources.ts
types.ts              核心类型(Api, ProviderId, Model, Context, Message, ...)
utils/
  event-stream.ts     AssistantMessageEventStream
  retry.ts            retryAssistantCall / RetryPolicy
  overflow.ts         isContextOverflow / isRecoverableLength
  abort.ts            raceWithAbortSignal
  diagnostics.ts      AssistantMessageDiagnostic
  text.ts             contentText
  validation.ts       validateToolArguments(TypeBox)
  uuid.ts             uuidv7
  json-parse.ts
  typebox-helpers.ts
index.ts              仅类型导出,不注册 Provider 工厂
cli.ts                `pi-ai` 自带 CLI(查询模型)
```

**`packages/agent/src/`**(Agent 运行时 + 高阶 Harness)

```
agent.ts              顶层 Agent 类 + AgentState
agent-loop.ts         runAgentLoop / runAgentLoopContinue(主循环)
node.ts               Node 适配入口
index.ts              公开 API
proxy.ts              代理/中间件
stream-fn.ts          默认 streamFn
types.ts              AgentContext / AgentEvent / AgentTool / AgentMessage / AgentState
harness/
  agent-harness.ts    高阶 Harness(并发 lane, run/compact/navigate)
  events.ts           HarnessEvent 类型
  messages.ts         convertToLlm / 自定义消息转换
  prompt-templates.ts 模板加载
  reducer.ts          状态机 reducer(667 行)
  result.ts           Result 类型 + TaggedError
  skills.ts           Skill 加载与格式化(386 行)
  system-prompt.ts    系统提示组装
  telemetry.ts        TelemetryContext
  types.ts            AgentHarnessResources / Skill / PromptTemplate
  compaction/         上下文压缩(branch-summarization / compaction / utils)
  env/                执行环境抽象(nodejs.ts)
  session/            Session 模型(JSONL / state / context / memory / types / testing)
  tools/              内置工具(read / write / edit / edit-diff / bash / image / ...)
search/
  scanning.ts         文件扫描
  index.ts            搜索入口
```

**`packages/coding-agent/src/`**(CLI 入口 + 模式)

```
main.ts               CLI 入口(978 行)
cli.ts                转发到 main
rpc-entry.ts          RPC 模式入口
index.ts              SDK 入口(export createAgentSession)
migrations.ts         配置迁移
config.ts             常量(APP_NAME / ENV_SESSION_DIR / ...)
package-manager-cli.ts  包管理命令
cli/                  CLI 子命令(auth / list-models / project-trust / startup-ui / ...)
core/                 AgentSession + 全部服务
  agent-session.ts              主类(3515 行!)
  agent-session-runtime.ts      runtime 工厂
  agent-session-services.ts     服务组装
  bash-executor.ts              bash 工具
  compaction/                   复用 agent 的压缩 + 自己的封装
  event-bus.ts                  事件总线
  extensions/                   扩展加载/运行/类型
  export-html/                  会话导出 HTML
  model-runtime.ts / resolver.ts / registry.ts
  settings-manager.ts / trust-manager.ts
  session-manager.ts / cwd.ts / export.ts
  tools/                        bash / read / write / edit / find / grep / ls ...
  skills.ts / system-prompt.ts / slash-commands.ts / prompt-templates.ts
  sdk.ts                        createAgentSession 公共 API
modes/                三种运行模式
  interactive/
    interactive-mode.ts(6575 行,TUI 入口)
    components/    40+ UI 组件
      footer.ts, diff.ts, markdown.ts, mermaid.ts,
      session-selector.ts, model-selector.ts, theme-selector.ts,
      skill-invocation-message.ts, tool-execution.ts, ...
  rpc/
    rpc-mode.ts / rpc-client.ts / jsonl.ts / rpc-types.ts
  print-mode.ts      一次性 / pipe 模式
  json-event.ts      JSON 事件序列化
server/
  create-harness.ts  远程 harness
client/
  index.ts / remote-session.ts / transcript.ts
extensions/           内置扩展
  index.ts           注册全部
  llama/             llama.cpp 桥接
bun/                  Bun 启动包装
```

**`packages/tui/src/`**(终端 UI 库)

```
tui.ts                TUI 容器 + 渲染循环(1263 行)
editor-component.ts   多行编辑器
fuzzy.ts              模糊搜索
keybindings.ts / keys.ts / native-modifiers.ts
layout.ts / layout-node.ts
components/           基础组件
  box, stack, h-stack, v-stack, text, scroll-view,
  input, select-list, editor, settings-list, markdown,
  image, truncated-text, loader, cancellable-loader, spacer
autocomplete.ts / kill-ring.ts / undo-stack.ts
terminal.ts / terminal-colors.ts / terminal-image.ts
stdin-buffer.ts / tui-main-screen.ts / tui-alt-screen.ts
alt-screen-search.ts / latex.ts / word-navigation.ts
utils.ts / native-module-path.ts
```

---

## 3. 架构骨架

### 3.1 分层

```
┌──────────────────────────────────────────────────────────┐
│ coding-agent   (CLI / Modes: interactive / rpc / print) │
│   ↳ AgentSession (3515 lines)  生命周期/会话/事件        │
├──────────────────────────────────────────────────────────┤
│ agent-core     (Agent / AgentLoop / Harness)             │
│   ↳ agent.ts / agent-loop.ts  纯循环                     │
│   ↳ harness/   并发 lane、compaction、skill、session     │
├──────────────────────────────────────────────────────────┤
│ pi-ai          (Models.streamSimple / 30+ Providers)     │
│   ↳ EventStream(AssistantMessageEventStream)             │
├──────────────────────────────────────────────────────────┤
│ pi-tui         (差异渲染 TUI 库)                          │
└──────────────────────────────────────────────────────────┘
```

### 3.2 Agent 主循环

- 位置:`packages/agent/src/agent-loop.ts`(803 行)
- 入口:`runAgentLoop(prompts, context, config, emit, signal, streamFn)` 和 `runAgentLoopContinue(...)`(用于 retry,context 已有 user/toolResult)
- 数据契约:上下文全程使用 `AgentMessage`(自家类型),只在 LLM 调用边界 `convertToLlm` 转成 `Message[]`
- 输出:`EventStream<AgentEvent, AgentMessage[]>`(消费者:`stream.push(event)` + `stream.end(messages)`)
- 关键回调(在 `AgentLoopConfig`):
  - `beforeToolCall(ctx) → BeforeToolCallResult | undefined`(可 `block`/`terminate`)
  - `afterToolCall(ctx) → AfterToolCallResult | undefined`(部分覆盖 content/details/isError/usage/terminate)
  - `prepareNextTurn(ctx)`(队列消费前)
  - `shouldStopAfterTurn(ctx)`
  - `convertToLlm(messages) → Message[]`
- 工具调度:
  - `ToolExecutionMode = "sequential" | "parallel"`
  - parallel 模式:prepare 串行 → 执行并发 → `tool_execution_end` 按完成顺序 → tool-result 消息按 assistant 源顺序发射
- `QueueMode = "all" | "one-at-a-time"`,控制队列消费策略

### 3.3 Agent 高阶 Harness

- 位置:`packages/agent/src/harness/agent-harness.ts`(508 行)+ `reducer.ts`(667 行)
- 概念:**Lane**(并发轨道),每条 lane 有自己的"叶节点"
- 操作类型:`run` / `compaction` / `navigation`(同一 lane 内串行,跨 lane 并行)
- 错误体系:基于 `TaggedError("Name")<{...}>` 的细分错误类(`LaneBusy`/`MissingIdentities`/`NoActiveRun`/`NothingToResume`/`UnknownSkill`/`UnknownTemplate`/`UnknownTarget`/`LaneExists`/`InvalidLane`/`Closed` ...)
- 状态机:`reducer.ts` 处理 lane 状态转换
- 结果:`RunOutcome = completed | aborted | failed | suspended`;`suspended` 用于 `DeferredHandle`(长任务挂起)

### 3.4 消息模型

- `pi-ai`:`Message = UserMessage | AssistantMessage | ToolResultMessage`,`AssistantMessage.content = (TextContent | ThinkingContent | ToolCall | ImageContent)[]`
- `pi-agent-core`:`AgentMessage = Message & { stableId, ... }`,用于跨 provider 保持消息身份
- Session 存储条目(`harness/session/types.ts`):`MessageEntry`/`ModelChangeEntry`/`ThinkingLevelEntry`/`ActiveToolsEntry`/`CompactionEntry`/`BranchSummaryEntry`/`LabelEntry`/`NoteEntry`/`CustomEntry`/`CustomMessageEntry`/`SessionInfoEntry`/`ProvisionedEntry`
- 序列号 `seq` 跨 lane 共享,`parentId` 指向 lane 叶节点

### 3.5 Context 管理

- 实时:`AgentContext.messages: AgentMessage[]`
- 压缩:`compaction/compaction.ts`,用 LLM 生成 summary + 保留 tail 消息 + 文件操作清单
- 预估 token:`estimateContextTokens` / `calculateContextTokens`
- 触发:`shouldCompact()`(阈值策略)
- 分支摘要:`branch-summarization.ts`(从某节点 fork 出分支时调用)
- Session 重建上下文:`session/context.ts → buildSessionContext()`,从 JSONL 恢复

### 3.6 Provider / LLM 客户端

- `Models.streamSimple(model, context, options)` — 统一入口,返回 `AssistantMessageEventStream`
- Provider 工厂在 `pi-ai/providers/*`,运行时按 `model.provider` 派发
- 30+ 内置 Provider(详见 2 节)
- OAuth 内建(`auth/oauth/anthropic.ts` 等)
- 凭据解析:`resolveProviderAuth()`,支持 `OAuthAuthInfo` / `ApiKey` / `None` / 多种 `OAuthPrompt`
- 重试:`retryAssistantCall(RetryPolicy, RetryCallbacks)`
- 诊断:`AssistantMessageDiagnostic`(流式)
- 模型目录:`models.generated.ts` 在 build 时由 `npm run generate:models` 生成

### 3.7 TUI / REPL

- 库:自定义 `pi-tui`(1263 行 `tui.ts` + 组件),**差异渲染**(`CURSOR_MARKER = "\x1b_pi:c\x07"` 自定义 ESC 序列定位光标)
- 组件:盒子(Box/Stack/Text/ScrollView/Input/Editor/SelectList/...)
- 编辑器:多行编辑器(`editor-component.ts` + `kill-ring.ts` + `undo-stack.ts` + `word-navigation.ts`)
- 搜索:Alt-Screen 搜索(`alt-screen-search.ts` + `fuzzy.ts`)
- 模式入口:`modes/interactive/interactive-mode.ts`(6575 行,聚合 40+ 子组件)
- 另外两种模式:RPC(`rpc-mode.ts`,JSONL over stdio)、Print(`print-mode.ts`,一次性执行)

---

## 4. 核心特征

| 能力 | 支持 | 位置 / 说明 |
|---|---|---|
| 多 Agent | 部分 | `examples/extensions/subagent/` 演示通过外部 `pi` 子进程隔离上下文(每个子 agent 独立进程),**不内置** |
| 任务分类 | 无 | LLM 自由决定,不显式分类 |
| 任务拆解 | 无内置 | Todo/计划由扩展 `examples/extensions/todo.ts` 等提供 |
| 质检 | 无内置 | 只有 `examples/extensions/overlay-qa-tests.ts` 这样的扩展 |
| MCP | **明确不支持** | `coding-agent/README.md:499 "**No MCP.** Build CLI tools with READMEs (see Skills)"`(作者 Mario Zechner 反对 MCP) |
| Skill | 支持,一等公民 | `harness/skills.ts`(386 行)+ `core/skills.ts`(507 行)。支持 YAML frontmatter、`disable-model-invocation`、ignore 文件、`<skill name="..." location="...">` 标签注入 |
| Session | 强大 | 树状条目(`MessageEntry` + `CompactionEntry` + `BranchSummaryEntry` + ...)、JSONL 持久化、可恢复、可分支、SQLite backend 可选 |
| 多 Provider | 广泛 | 30+ 内置,统一 `Models.streamSimple` |
| 流式响应 | 是 | `AssistantMessageEventStream`(async iterable + `push`/`end` 协议),重试/取消/挂起(`DeferredHandle`) |
| 并发执行工具 | 是 | `ToolExecutionMode = "sequential" \| "parallel"` |
| Lane 并发 | 是 | `AgentHarness` 内 lane 模型,run/compact/navigation 三类操作 |
| OAuth | 是 | 多 Provider OAuth + Device Code flow(`auth/oauth/anthropic.ts` + `bun-oauth.ts`) |
| 扩展 | 是 | `core/extensions/` loader/runner/types/wrapper,内置 50+ 示例扩展 |
| 项目信任 | 是 | `trust-manager.ts` / `project-trust.ts` / `cli/project-trust.ts`(沙箱分级) |
| 沙箱 | 推荐外部 | 文档 `containerization.md` 推荐 Gondolin / Docker / OpenShell |

---

## 5. 关键文件清单(20 个核心源文件)

| # | 路径 | 行数 | 职责 |
|---|---|---|---|
| 1 | `packages/coding-agent/src/core/agent-session.ts` | 3515 | AgentSession 主类,跨模式共享(交互/RPC/print 都基于它) |
| 2 | `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | 6575 | TUI 模式入口,聚合 40+ UI 组件 |
| 3 | `packages/coding-agent/src/main.ts` | 978 | CLI 入口,解析参数/启动会话 |
| 4 | `packages/agent/src/agent-loop.ts` | 803 | Agent 主循环(runAgentLoopContinue) |
| 5 | `packages/agent/src/harness/reducer.ts` | 667 | Harness lane 状态机 reducer |
| 6 | `packages/agent/src/agent.ts` | 592 | 顶层 Agent 类 + AgentState |
| 7 | `packages/agent/src/harness/agent-harness.ts` | 508 | 高阶 Harness(并发 lane) |
| 8 | `packages/agent/src/types.ts` | 444 | AgentContext / AgentEvent / AgentTool 等类型 |
| 9 | `packages/coding-agent/src/core/skills.ts` | 507 | Skill 加载与 slash command 集成 |
| 10 | `packages/agent/src/harness/skills.ts` | 386 | Skill 基础加载(agent 层) |
| 11 | `packages/agent/src/harness/compaction/compaction.ts` | ~400 | 上下文压缩主逻辑 |
| 12 | `packages/ai/src/models.ts` | 944 | `Models` 单例 + streamSimple / refresh / store |
| 13 | `packages/ai/src/types.ts` | 866 | Api / ProviderId / Model / Context / Message / Usage / ThinkingLevel |
| 14 | `packages/ai/src/api/anthropic-messages.ts` + `openai-responses.ts` 等 | 各 ~300 | 各 Provider 协议适配 |
| 15 | `packages/tui/src/tui.ts` | 1263 | TUI 容器 + 差异渲染循环 |
| 16 | `packages/coding-agent/src/modes/rpc/rpc-mode.ts` | 821 | JSONL over stdio RPC 模式 |
| 17 | `packages/coding-agent/src/modes/print-mode.ts` | 169 | Print 模式(单次执行) |
| 18 | `packages/agent/src/harness/session/types.ts` | ~300 | Session entry 类型(树状) |
| 19 | `packages/coding-agent/src/core/model-runtime.ts` + `model-resolver.ts` | ~500 | 模型运行时 + CLI 解析 |
| 20 | `packages/coding-agent/src/core/extensions/runner.ts` | ~400 | 扩展加载与生命周期 |

---

## 6. 独特设计(对比 LsmAgentEmergentWork/YoloAgent)

### 6.1 Lane 模型 + reducer 状态机(Harness 并发)

`pi` 的 `AgentHarness` 把"运行 / 压缩 / 导航"三类操作抽象成**并发 lane**,每条 lane 持有一个叶节点指针,操作类型在 lane 内串行、跨 lane 并行。所有 lane 共享一个 `seq` 序号,`reducer.ts` 是 667 行的纯函数状态机。这种"操作即 lane"模型让多任务并行、压缩与运行交错、UI 可订阅任意 lane,远比"一个全局 AgentState"灵活。YoloAgent 当前是单线状态,移植这个 lane 抽象可直接支持并行分支会话。

### 6.2 AgentMessage ↔ Message 双层 + 边界 convertToLlm

`pi-agent-core` 全程用 `AgentMessage`(带 stableId),只在调用 LLM 时一次性 `convertToLlm` → `Message[]`。这让上层(扩展、压缩、持久化、UI)能稳定操作消息身份,不受 Provider schema 漂移影响。YoloAgent 直接用 SSE 解析后的 `ContentBlock`,没有"内部模型"和"对外协议"的分层,改 Provider 时容易牵动上层。

### 6.3 EventStream 协议 + 可挂起(`DeferredHandle`)

`AssistantMessageEventStream` 是 push 协议的 async iterable,允许 `DeferredHandle`(长任务挂起、稍后 resume)。`AgentEvent` 流式发出 `tool_execution_start / end / message_start / end / turn_start / end`,UI 可增量渲染。重试、压缩、中止都不会破坏流。`runAgentLoopContinue` 让 retry 时不必重新投递 user 消息,只续 context。YoloAgent 的 SSE 流更像"一次性解析",没有这个抽象。

### 6.4 树状 Session + JSONL 条目

Session 不是"消息列表",而是**带类型的条目树**:`MessageEntry`、`CompactionEntry`、`BranchSummaryEntry`、`ModelChangeEntry`、`ThinkingLevelEntry`、`LabelEntry`、`NoteEntry` … 每条带 `seq` + `parentId`,可分支、可摘要、可恢复。后端可选 JSONL(`session/jsonl/repo.ts`)或 SQLite(`session-backends/sqlite-node`)。YoloAgent 当前是单条线性对话,要支持"会话树 + 时间旅行"可以直接参考这套类型设计。

### 6.5 拒绝 MCP,把 Skill 做成一等公民

`coding-agent/README.md:499` 明确写 "**No MCP.** Build CLI tools with READMEs (see Skills)"——作者 Mario Zechner 认为 MCP 把"工具描述"和"工具实现"用协议隔开,反而割裂体验。Pi 的替代是 `Skill = SKILL.md(YAML frontmatter + Markdown 正文)`,正文即提示词,执行方由 skill 自己调用本地 CLI / 扩展。加载时支持 `.gitignore`、诊断、`<skill name="..." location="...">` 标签注入。YoloAgent 目前没有 Skill 概念,可考虑这种"轻提示重 CLI"的中间路线。

### 6.6 Tool 回调 + 队列 + parallel/sequential 双模式

`beforeToolCall` 可 `block`/`terminate`、`afterToolCall` 可字段级覆盖结果、`shouldStopAfterTurn` 决定何时停止。`ToolExecutionMode = "sequential" | "parallel"`,`QueueMode = "all" | "one-at-a-time"`。这套"工具钩子"机制让权限、审查、改写、early-exit 都可插拔。YoloAgent 的工具调用更"裸",没有 before/after 钩子和队列策略。

### 6.7 三模式共享 AgentSession(interactive / rpc / print)

同一个 `AgentSession` 被 `InteractiveMode`(TUI)、`RpcMode`(JSONL over stdio)、`PrintMode`(单次)复用,差异只在"事件输出到哪"。`server/create-harness.ts` + `client/remote-session.ts` 让"本地嵌入式"和"远端 RPC"使用相同的会话 API。YoloAgent 当前只支持单进程 TUI,要实现"同会话双端"或"CLI/TUI 切换"可参考这种设计。

---

## 7. 小结

Pi 是一个**高度分层、面向扩展**的 Agent Harness:

- 运行时(agent-core)和协议适配(pi-ai)严格分离,30+ Provider 通过统一 `Models.streamSimple` 接入
- Session 是**带类型的条目树**,支持分支、压缩、JSONL/SQLite 双后端
- 并发由 **Harness lane 模型**驱动,而不是靠多线程或多 Agent
- 扩展体系(`extensions/`)极其庞大,内置 50+ 示例
- 主动**拒绝 MCP**,把 Skill 当一等公民
- TUI 自研差异渲染,光标定位用自定义 ESC 序列

对比 YoloAgent(单进程 Rust + TUI + 单线会话),可借鉴的核心模式有:**lane 抽象**、**AgentMessage 内部模型 + 边界 convert**、**树状 Session**、**Skill 替代 MCP**、**EventStream + DeferredHandle**。