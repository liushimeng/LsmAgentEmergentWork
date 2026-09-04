# OpenClaw 源码调研报告

> 调研对象:`/usr/local/LsmGitOpenSource/openclaw`(openclaw/openclaw,v2026.8.1)
> 调研日期:2026-09-04
> 调研目的:为 LsmAgentEmergentWork 提供设计参照

---

## 1. 项目元信息

| 维度 | 内容 |
| --- | --- |
| 定位 | **多渠道 AI 网关 / 个人与团队助理**(不是纯编码 Agent)。自述:"Multi-channel AI gateway with extensible messaging integrations" |
| 语言 | TypeScript(Node 22.22.3+ / 24.15+ / 25.9+),伴生原生 App 用 Swift(iOS/macOS)、Kotlin(Android) |
| 包管理 | **pnpm workspace**(`pnpm-workspace.yaml`),明确禁止 `npm install` |
| 构建 | `tsdown`(rolldown 系)+ 自研编排脚本 `scripts/build-all.mts`;`tsconfig.json` 15KB 巨型 project-references |
| 入口 | `openclaw.mjs`(bin shim,25KB)→ `src/entry.ts`(339 行)→ `src/cli/*` commander 分发 |
| 测试 | **vitest**;测试代码 ~301 万行 vs 生产代码 ~201 万行(测试比生产还多) |
| Lint/Format | **oxlint**(15KB 配置)+ **oxfmt**;pre-commit + semgrep + `.crabbox.yaml` |
| 核心依赖 | `@anthropic-ai/sdk`、`openai`、`@google/genai`、`@mistralai/mistralai`、`@modelcontextprotocol/sdk`、`@agentclientprotocol/sdk`(ACP)、`@earendil-works/pi-tui`(TUI)、`kysely`+SQLite、`grammy`(Telegram)、`express`、`typebox`(工具 schema) |
| 规模 | src+packages 非测试 TS ≈ **201 万行 / 1.6 万文件**;`ui/` 前端 ≈ 82 万行;`extensions/` **161 个插件** |
| 文档 | `README.md` 112KB、`AGENTS.md`(=CLAUDE.md)66KB、`CHANGELOG.md` **4.1MB**、`SECURITY.md` 36KB、`VISION.md` |
| License | MIT |

**规模警示**:这是本轮调研中体量最大的项目(比 opencode 大一个数量级),不适合整体照搬,但架构分层与若干子系统设计极具参考价值。

---

## 2. 目录树

```
openclaw/
├── openclaw.mjs              # bin 入口 shim(版本快速路径/respawn)
├── package.json              # 132KB(!),含 dist 白名单与数百 scripts
├── tsdown.config.ts          # 31KB 构建配置
├── src/                      # 主运行时(79 个顶层目录)
├── packages/                 # 23 个内部包(可被插件复用的稳定契约层)
├── extensions/               # 161 个一等公民插件(provider / channel / 能力)
├── ui/                       # Control UI(Web 前端,独立 vite)
├── apps/                     # android / ios / macos / linux / shared 伴生 App
├── skills/                   # 52 个内置 Skill(SKILL.md + frontmatter)
├── custodian-skills/ qa/ security/ config/ deploy/ scripts/ test/ docs/
└── taxonomy.yaml             # 707KB 能力/模型分类表
```

### 2.1 `src/` 核心目录(按文件数排序)

| 目录 | 文件数 | 职责 | 分类 |
| --- | --- | --- | --- |
| `agents/` | 3261 | **Agent 全部实现**:工具装配、运行器、子代理、沙箱、harness、鉴权 | 核心 |
| `gateway/` | 2336 | 本地控制面:JSON-RPC server、session 路由、审批、看板 | 核心 |
| `infra/` | 1377 | 环境、策略、诊断、状态迁移 | 辅助 |
| `commands/` | 1075 | CLI 子命令实现 | 辅助 |
| `plugins/` | 1007 | 插件注册表 / 激活 / 契约 / hook 分发 | 核心 |
| `auto-reply/` | 786 | **消息→回复主编排**(get-reply 管线、队列、指令解析) | 核心 |
| `cli/` 764 / `config/` 748 | | CLI 骨架 / 配置 schema | 辅助 |
| `plugin-sdk/` | 648 | 暴露给插件的运行时 API 门面 | 核心 |
| `channels/` | 483 | 渠道抽象(DM 门禁、草稿流式、allowlist) | 核心 |
| `skills/` | 248 | Skill 发现 / 加载 / 运行 / **workshop(自演化)** | 核心 |
| `tui/` | 120 | 终端 UI(pi-tui) | 辅助 |
| `sessions/` 76 / `agents/sessions/` 149 | | 会话生命周期、转写、持久化 | 核心 |
| `llm/` | 47 | Provider stream wrapper、OAuth、模型注册表 | 核心 |
| `mcp/` | 22 | **MCP 服务端**(把 OpenClaw 工具暴露出去) | 核心 |
| `context-engine/` | 18 | **可插拔上下文引擎注册表** | 核心 |
| `tasks/` 97 / `cron/` 394 / `hooks/` 77 | | 任务注册表、定时、事件钩子 | 核心 |
| `trajectory/` 14 / `audit/` 40 / `security/` 90 | | 轨迹导出、审计、危险工具清单 | 辅助 |

### 2.2 `packages/`(契约层,与 `src/` 解耦)

```
agent-core/        # ★ Agent 主循环 + 压缩 + 会话上下文(36 文件)
llm-core/          # ★ 消息模型 / Model / Tool / EventStream 纯类型契约
ai/                # ★ Provider 传输实现(anthropic/openai/google/bedrock/...)
plugin-sdk/        # 插件 SDK 类型
gateway-protocol/  # Gateway JSON-RPC schema + 校验器
gateway-client/    # 客户端(TUI/UI 共用)
terminal-core/ markdown-core/ media-core/ model-catalog-core/
normalization-core/ net-policy/ retry/ tool-call-repair/
session-url-contract/ workboard-contract/ acp-core/ sdk/
```

### 2.3 `src/agents/` 内部(最核心)

```
agents/
├── embedded-agent-runner/   566 文件 ★ 内置 Agent 运行器(run/ attempt 分阶段)
├── tools/                   281 文件   OpenClaw 专属工具(message/gateway/skill/media/...)
├── subagents/               211 文件 ★ 子代理:spawn / announce / swarm / registry
├── sessions/                149 文件   会话管理器与持久化(JSONL)
├── auth-profiles/           105 文件   多套模型凭据档案
├── sandbox/                  97 文件   容器/FS 挂载沙箱
├── harness/                  94 文件 ★ 可插拔 Agent Harness 契约
├── cli-runner/               77 文件 ★ 外部 CLI Agent(claude/codex/gemini)驱动
├── failover/                 35 文件 ★ 错误分类与模型故障转移
├── runtime-plan/ worktrees/ command/ modes/ agent-hooks/
└── agent-tools*.ts (≈30 个)          工具装配与 before/after 策略流水线
```

---

## 3. 架构骨架

### 3.1 分层

```
渠道(WhatsApp/Telegram/Slack/Discord/Signal/iMessage/SMS/Matrix)
        │  extensions/<channel>
        ▼
   auto-reply 管线  src/auto-reply/reply/get-reply.ts (1364 行)
        │  路由 → 会话解析 → 指令/斜杠命令 → 模型选择 → 运行准入
        ▼
   Gateway(本地控制面) src/gateway/agent-turn/agent-turn-service.ts
        │  JSON-RPC over WS,统一 session/tool/event/approval
        ▼
   Agent Harness 选择  src/agents/harness/*
        ├── builtin-openclaw  → embedded-agent-runner → packages/agent-core
        ├── cli-runner        → claude / codex / gemini CLI 子进程
        └── acp               → Agent Client Protocol 外部 Agent
        ▼
   agentLoop  packages/agent-core/src/agent-loop.ts (1776 行)
        ▼
   StreamFn   packages/ai/src/transports/*  →  Provider HTTP
```

### 3.2 Agent 主循环

- 位置:`packages/agent-core/src/agent-loop.ts`
- 公开 API:`agentLoop()` / `agentLoopContinue()`(返回 `EventStream<AgentEvent, AgentMessage[]>`),以及 `runAgentLoop()` / `runAgentLoopContinue()`(Promise 版)。核心私有函数 `runLoop()`。
- 事件模型:`agent_start / turn_start / message_start / message_end / tool_execution_start / tool_execution_end / turn_end / agent_end`,通过 `AgentEventSink` 推送。
- 循环内建能力(全部在 core 层,而非上层拼装):
  - **steering(转向消息)**:用户在模型思考中途插话,循环在 checkpoint 处 drain 队列;`QueueMode: "all" | "one-at-a-time"`。
  - **工具并行/串行**:`ToolExecutionMode: "sequential" | "parallel"`,并行时结果按 assistant 源序输出但 `tool_execution_end` 按完成序发出。
  - **工具循环检测与恢复**:`ToolLoopIntervention{kind:"critical-tool-loop"}` + `ToolLoopWarning`,循环内一次性恢复,再犯即终止。
  - **批准入(batch admission)**:`InternalBeforeToolBatchResult` 对整批工具调用做一次准入,而非逐个。
  - **中断语义**:`turn-interruption.ts` 生成 `InterruptedTurnMessage`,保证 transcript 可续接(`TranscriptNotContinuableError`)。
- 上层 `src/agents/embedded-agent-runner/` 把一次 run 拆成显式阶段文件:`attempt-setup → attempt-session-prepare → attempt-prompt-build → attempt-stream-prepare → attempt-llm-boundary → attempt-stream-settle → attempt-finalize → attempt-settle → terminal-resolution`,每阶段 500-800 行独立文件。

### 3.3 消息模型

`packages/llm-core/src/types.ts`(744 行)是唯一真源:

```ts
type Message = UserMessage | AssistantMessage | ToolResultMessage;
interface Context { systemPrompt?; messages: Message[]; tools?: Tool[]; ... }
interface AssistantMessage { content: (Text|Thinking|ToolCall)[]; stopReason: StopReason; usage: Usage; errorMessage? }
type KnownApi = "openai-completions" | "mistral-conversations" | "openai-responses"
              | "azure-openai-responses" | "openai-chatgpt-responses" | "anthropic-messages"
              | "bedrock-converse-stream" | "google-generative-ai" | "google-vertex";
```

关键约定:**StreamFn 永不 throw**——请求/模型/运行时失败必须编码进返回的 stream(最终 `AssistantMessage.stopReason = "error" | "aborted"` + `errorMessage`)。这让上层重试/failover 逻辑无需 try/catch 分叉。

工具 schema 使用 **typebox**(`Tool<TParameters extends TSchema>`),而非 zod。

### 3.4 Context 管理

三层:

1. **core 压缩**:`packages/agent-core/src/harness/compaction/compaction.ts`(1040 行)——`shouldCompact` / `findCutPoint` / `generateSummary` / `prepareCompaction` / `estimateContextTokens`,含图片 token 估算(`IMAGE_BLOCK_TOKENS`)、尾部 toolResult 配对保护(`tool-result-pairing.ts`)、分支摘要(`branch-summarization.ts`)。
2. **可插拔 Context Engine**:`src/context-engine/{types,registry,delegate}.ts`——插件可注册自定义上下文引擎,实现 `assemble/ingest/bootstrap/maintenance`;返回 `AssembleResult{messages, estimatedTokens, promptAuthority, systemPromptAddition, contextProjection}`;支持 `thread_bootstrap`(持久线程只注入一次 + epoch 轮换)与 `per_turn` 两种投影模式;引擎故障有 **quarantine 隔离 + degraded/fallback 降级**(`quarantine-health.ts`、`compaction-watchdog.ts`)。
3. **运行器侧**:抢占式压缩 `run/preemptive-compaction.ts`、溢出压缩 `run.overflow-compaction.harness.ts`、排队压缩 `compact.queued.ts`、工具结果截断 `tool-result-truncation.ts`(1370 行)与 `tool-result-context-guard.ts`。

### 3.5 Provider(LlmClient)

- 注册表:`packages/ai/src/api-registry.ts` —— 按 `Api` id 注册 `{stream, streamSimple}`,插件可注册自定义 API family。
- 传输实现:`packages/ai/src/transports/*` + `internal/{anthropic,openai}.ts`;`src/llm/providers/stream-wrappers/` 放厂商特化补丁(anthropic cache-control、google thinking payload、moonshot thinking、minimax、zai、reasoning-effort 归一)。
- 认证:`src/llm/utils/oauth/{anthropic,openai-chatgpt}.ts` 支持 OAuth 订阅登录,`src/agents/auth-profiles/` 支持多凭据档案切换。
- Provider 生态放在 `extensions/`:anthropic、amazon-bedrock、anthropic-vertex、azure-*、cerebras、cohere、deepseek、deepinfra、baseten、chutes、byteplus、alibaba、copilot、clawrouter、cloudflare-ai-gateway、arcee、beam…(≈40+)。
- **故障转移**:`src/agents/failover/classify.ts`(437 行)把错误分类为 auth / billing / rate-limit / overload / context-overflow / structured-misc,附大量 `*.cases.ts` 表驱动测试;`request-error-facets.ts`、`retry-evidence.ts` 产出可解释的用户文案。

### 3.6 TUI / REPL / UI

- **TUI**:`src/tui/tui.ts`(2020 行)基于 `@earendil-works/pi-tui`;通过 `gateway-client` 连 Gateway,不直接跑 Agent;支持 session picker、overlays、审批、OSC8 超链接、PTY 测试夹具。
- **Control UI**:`ui/`(独立 vite 工程,82 万行 TS/TSX)。
- **CLI**:commander,`src/cli/command-catalog.ts` 集中注册,含 precomputed help 快速路径、profile、容器目标、respawn 策略。
- **ACP**:`src/acp/` + `packages/acp-core/`,实现 Agent Client Protocol,可作为 IDE 侧 Agent 接入。

---

## 4. 核心特征对照

| 特征 | 支持 | 实现位置 / 说明 |
| --- | --- | --- |
| 多 Agent | ✅ 强 | `src/agents/subagents/`:`spawn/` 派生、`announce/` 结果回传与唤醒(21 个文件专做"子代理完成如何通知父代理")、`registry/` SQLite 注册表 |
| 并行编队(Swarm) | ✅ | `subagents/swarm/`:`swarm-scheduler.ts` 分组泳道限流(maxConcurrent 8 / maxChildrenPerGroup 50 / maxTotalPerGroup 200)、`swarm-collector.ts` 汇聚、`swarm-output-schema.ts` 结构化输出、`swarm-code-mode.ts` |
| 任务分类 | ⚠️ 局部 | 无"用户意图分类器";但有 `src/sessions/classify-session-kind.ts`(会话种类)、`failover/classification-rules.ts`(错误分类)、`harness/result-classification.ts`(结果分类) |
| 任务拆解 | ⚠️ 间接 | 无内置 planner 模块;拆解通过 **子代理 spawn + swarm** 与 `src/tasks/task-executor.ts` 任务注册表实现,由模型自行决定 |
| 质检 / Review | ✅ 特色 | `src/agents/exec-auto-reviewer.ts` —— **独立小模型作为"exec 安全审查员"**,在 shell 命令执行前返回 `{decision:allow|ask, risk, rationale}`;同样机制用于 dashboard widget 能力授权 |
| MCP | ✅ 双向 | 客户端:`src/agents/agent-bundle-mcp-*.ts`(≈16 文件,含安装/生命周期/物化/命名);服务端:`src/mcp/{openclaw-tools-serve,plugin-tools-serve,tools-stdio-server,channel-server}.ts` |
| Skill | ✅ 强 | `skills/` 52 个内置(`SKILL.md` + YAML frontmatter,含 `requires.bins` 与 `install` 声明);`src/skills/{discovery,loading,runtime,lifecycle,security,workshop}` |
| Skill 自演化 | ✅ 独有 | `src/skills/workshop/`(≈40 文件):curator 扫描历史 → 生成 proposal → review/apply/rollback,配 SQLite store + 提案哈希 + 目标锁 |
| Session | ✅ | JSONL 文件持久化(`session-manager-persistence.ts`)+ 多个 SQLite store;`src/sessions/` 负责生命周期/准入/转写/参与者记录;Gateway 侧统一 session 路由 |
| 多 Provider | ✅ 强 | 9 个内置 API family + 40+ provider 插件 + OAuth + 多凭据档案 + 自动故障转移 |
| 流式响应 | ✅ | `EventStream`(`packages/llm-core/src/utils/event-stream.ts`)贯穿 provider → agent-loop → gateway → TUI/UI;渠道侧还有"草稿流式"`channels/draft-stream-loop.ts` 边生成边编辑消息 |
| 沙箱 | ✅ | `src/agents/sandbox/`:容器化、FS 挂载白名单、workspace 只读挂载 |
| 审批 | ✅ | Gateway 级 exec-approval 全链路(`bash-tools.exec-approval-*`,8 文件),支持渠道 reaction 审批、Web Push |
| 定时 / 事件 | ✅ | `src/cron/`(394 文件)+ `src/hooks/`(gmail-watcher 等) |
| 轨迹导出 | ✅ | `src/trajectory/`:SQLite runtime store + `command-export.ts` |

---

## 5. 关键文件清单(20 个)

### 5.1 Agent 内核(packages/agent-core)

| 文件 | 行数 | 职责 |
| --- | --- | --- |
| `packages/agent-core/src/agent-loop.ts` | 1776 | **主循环**:turn 编排、steering、工具批准入、并行执行、循环检测 |
| `packages/agent-core/src/agent.ts` | 700 | `Agent` 类,封装循环 + 配置 + 生命周期 |
| `packages/agent-core/src/types.ts` | 651 | Agent 层契约:`AgentTool` / `AgentEvent` / `AgentLoopConfig` / 各 hook 结果类型 |
| `packages/agent-core/src/harness/compaction/compaction.ts` | 1040 | 上下文压缩全套算法 |
| `packages/agent-core/src/turn-interruption.ts` | 88 | 中断/失败消息构造与 transcript 可续接保证 |

### 5.2 LLM 层

| 文件 | 行数 | 职责 |
| --- | --- | --- |
| `packages/llm-core/src/types.ts` | 744 | **消息/模型/工具/流事件唯一真源** |
| `packages/ai/src/api-registry.ts` | 119 | Provider API 注册表(核心 + 插件) |
| `packages/ai/src/transports/provider-transport-stream.ts` | — | 统一流式传输骨架 |
| `src/agents/failover/classify.ts` | 437 | 错误分类 → 故障转移决策 |
| `src/llm/model-registry.ts` | — | 模型目录与运行时绑定 |

### 5.3 运行器与工具装配

| 文件 | 行数 | 职责 |
| --- | --- | --- |
| `src/agents/agent-tools.ts` | 1100 | **有效工具面装配**:core/shell/channel/plugin/tool-search 合并 + 沙箱/档案/发送者/群组/子代理策略过滤 |
| `src/agents/core-coding-tools.ts` | 261 | read/write/edit/apply_patch/exec/process 六件套(含沙箱变体) |
| `src/agents/embedded-agent-runner/runs.ts` | 1463 | run 注册表与生命周期 |
| `src/agents/embedded-agent-runner/run-loop.ts` | 720 | 尝试(attempt)重试外层循环 |
| `src/agents/embedded-agent-runner/tool-result-truncation.ts` | 1370 | 工具结果预算与截断策略 |
| `src/agents/system-prompt.ts` | 1610 | 系统提示装配(+ `system-prompt-contribution.ts` 插件贡献点) |
| `src/agents/tool-search.ts` | 421 | **渐进式工具披露**(tool_search / tool_describe / tool_call) |

### 5.4 编排与外围

| 文件 | 行数 | 职责 |
| --- | --- | --- |
| `src/auto-reply/reply/get-reply.ts` | 1364 | 入站消息 → 回复的主编排管线 |
| `src/context-engine/registry.ts` | 796 | 可插拔上下文引擎注册/解析/隔离 |
| `src/agents/harness/types.ts` | 569 | `AgentHarness` 契约(内置 / CLI / ACP 三态) |
| `src/tui/tui.ts` | 2020 | TUI 主程序(经 Gateway 客户端) |

---

## 6. 独特设计(与 LsmAgentEmergentWork 的差异)

LsmAgentEmergentWork 现状:Rust 单体,`src/agent/{orchestrator,plan,quality,subagent,memory,profile,context,main_work,yolo}.rs` ≈ 3.7k 行,`src/llm/{anthropic,openai,sse}.rs` 手写 provider,工具仅 `bash/read/write`。以下是 OpenClaw 值得借鉴的差异点。

### 6.1 Agent Harness 抽象:自己跑 or 外包给别的 Agent
`src/agents/harness/` 定义统一 `AgentHarness` 契约,同一套会话/工具/审批基础设施下可切换三种执行体:内置 `builtin-openclaw`(自研循环)、`cli-runner`(拉起 claude / codex / gemini CLI 子进程并解析其事件流)、`acp`(Agent Client Protocol 外部 Agent)。`harness/auto-selection.ts` 按可用性自动选。
→ 对本项目的意义:`orchestrator.rs` 可抽出 trait,让"调用外部 CLI Agent"成为一种可插拔后端,而不必自研全部能力。

### 6.2 渐进式工具披露(Tool Search)
当工具数量爆炸(插件 + MCP + 渠道工具可达数百个)时,不把全部 schema 塞进上下文,而是只暴露三个元工具:`tool_search`(检索)、`tool_describe`(取 schema)、`tool_call`(执行)。配 `tool-search-ranking.ts` 排序与 `tool-search-code-mode.ts` 代码模式。
→ 本项目工具少时不必要,但一旦接 MCP 就会遇到同一问题,值得预留。

### 6.3 独立"审查员模型"做执行前风控
`exec-auto-reviewer.ts` + `exec-auto-reviewer.prompt.ts`:每条待执行 shell 命令交给一个廉价模型,以**纯数据**(明确要求忽略命令内嵌的任何指令,防提示注入)审查,输出 `{decision, risk, rationale}` 决定直接放行还是弹审批。同机制复用于 widget 能力授权。
→ 与本项目 `quality.rs`(结果质检)正交:OpenClaw 做的是**执行前**质检。两者结合可形成"前置风控 + 后置质检"闭环。

### 6.4 可插拔 Context Engine + 隔离降级
上下文管理不是硬编码,而是注册表 + 契约(`assemble/ingest/bootstrap/maintenance`)。引擎可声明是否自己拥有压缩权(`promptAuthority`),可要求"持久线程只 bootstrap 一次"(`thread_bootstrap` + epoch)。引擎抛错会被 **quarantine**,运行时降到 `fallback`/`degraded` 模式并持久化隔离记录,而不是让整个 Agent 崩掉。
→ 本项目 `context.rs` + `memory.rs` 可参考"引擎可替换 + 失败降级不致命"这两点。

### 6.5 Skill Workshop:技能自演化闭环
`src/skills/workshop/`(约 40 文件)让 Agent **自己改进自己的 Skill**:扫描历史会话找候选(`history-scan-*.ts`)→ 起草提案(`proposal-draft.ts` + `proposal-hash.ts` 去重)→ 评审(`collection-review.ts` / `experience-review-*.ts`)→ 应用或回滚(`apply-transition.ts` / `collection-rollback.ts` / `store-sqlite-rollback.ts`),全程带目标锁与 SQLite 事务。
→ 这是把"经验沉淀"工程化的完整样板,对本项目 `memory.rs` 的长期演进方向有直接参考价值。

### 6.6 (附加)错误分类表驱动 + StreamFn 不抛异常契约
`failover/` 用 `*.cases.ts` 把每类 provider 错误(计费、限流、超载、上下文溢出、鉴权格式)固化为测试用例表;同时约定 provider stream **绝不 throw**,失败编码进流的终态消息。两者结合让重试/降级/换模型逻辑完全数据驱动、无异常控制流。
→ 本项目 `src/llm/sse.rs` + `anthropic.rs` 目前是手写错误处理,可借鉴"错误分类枚举 + 用例表"的做法。

---

## 7. 借鉴优先级建议

| 优先级 | 借鉴点 | 落地成本 |
| --- | --- | --- |
| 高 | 执行前审查员模型(6.3)——补齐 `yolo.rs` 之外的中间档风控 | 低,一个 prompt + 一次小模型调用 |
| 高 | 错误分类表驱动 + stream 不抛异常(6.6) | 中,需重构 `src/llm/` 错误路径 |
| 中 | Agent Harness trait 抽象(6.1) | 中,`orchestrator.rs` 抽 trait |
| 中 | Context Engine 失败降级/隔离(6.4) | 中 |
| 低 | Tool Search 渐进披露(6.2) | 待工具数 > 30 再考虑 |
| 低 | Skill Workshop 自演化(6.5) | 高,作为长期方向 |

## 8. 不建议照搬

- **文件粒度**:OpenClaw 把一个 run 拆成 40+ 个 500-800 行文件(`attempt-*.ts`),对 3.7k 行的本项目属于严重过度工程。
- **测试量级**:301 万行测试代码是团队规模产物,不可类比。
- **Gateway 多渠道控制面**:本项目定位是本地编码 Agent,不需要 WhatsApp/Telegram 网关那一层。