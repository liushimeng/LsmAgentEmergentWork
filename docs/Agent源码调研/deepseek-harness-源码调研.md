# deepseek-harness 源码调研

> 调研对象: `/usr/local/LsmGitOpenSource/deepseek-harness`
> 调研日期: 2026-09-04
> 调研范围: 仓库元信息、目录结构、架构骨架、核心特征、关键文件、独特设计

---

## 1. 项目元信息

| 项目 | 取值 |
|---|---|
| 包名 | `@deepseek-ai/dsh-root`(根)、子包 `@deepseek-ai/dsh-*` |
| 当前版本 | `0.1.2-alpha.1`(developer preview) |
| 仓库命名 | DeepSeek Harness,CLI 命令 `dsh` |
| License | MIT |
| 语言 | TypeScript(`type: module`,ESM 全量)+ 少量 Python SDK + Rust native(Landlock) |
| Node 要求 | `^22.19.0 \|\| >=24.0.0` |
| 包管理器 | pnpm `11.7.0` + workspaces |
| 核心框架 | **Cordis**(vendored,`vendor/`)+ `Schemastery` + `Typert`(自研 RPC 类型图) |
| LLM 适配来源 | 自研 + `@earendil-works/pi-ai`(vendored,作为多 Provider 中间层) |
| 测试栈 | vitest(`test`、`test:e2e`、`test:snapshot`、`test:web`、`test:expected`)、`knip`、`jscpd`、Lefthook、mermaid |
| 文档站 | VitePress(双语 `docs/architecture.md` 等) |
| 入口文件 | `apps/cli/src/bin.ts`(CLI 调度器),`pnpm dsh` / `npx @deepseek-ai/dsh web` |
| 应用 profile | `web`、`headless`、`sdk`、`sdk-minimal`、`acp`(均由 `dsh --profile X` 启动) |
| 提交流水 | `pnpm dsh` → `parseDshArgs` → `profile-boot.ts`/`plugin.ts`/`dump-config.ts` |
| 质量门槛 | 100% per-file 覆盖率(per AGENTS.md)、knip、publint、双语文档 gates |

**架构标语(README + architecture.md)**:"everything-is-a-plugin",基于 Cordis 的可时空组合(参见 [_A Programming Paradigm for Spatiotemporal Composability_](https://arxiv.org/abs/2608.25512))。没有特权内核,所有扩展(模型适配、工具、会话日志、Agent 循环本身)都是插件。

---

## 2. 目录树

```
deepseek-harness/
├── apps/                        # 应用入口(仅 CLI)
│   ├── cli/                     # dsh CLI 调度器
│   │   └── src/ {bin.ts,args.ts,plugin.ts,profile-boot.ts,process-shutdown.ts,dump-config.ts,sdk-source.cordis.patch.yml}
│   └── web/                     # 前端 vite 工程(只挂载 web bundle)
├── packages/                    # 247 个 npm 包,51 个分组
│   ├── core/                    # 产品 API 脊柱(8 包)
│   │   ├── agent/               # ctx.agents 服务、Agent 接口、inbox、model-selection
│   │   ├── agent-loop/          # ReactLoopAgent 主驱动
│   │   ├── agent-default-model/ # 默认模型策略
│   │   ├── agent-tool-presentation/ # 工具 UI 展示
│   │   ├── scope/               # scoped-events、Scope 链路
│   │   ├── session/             # SessionEvent 日志 + surface
│   │   ├── system-prompt/       # PromptAssembly 渲染
│   │   └── tools/               # ToolDefinition + 执行管线 + JSON Schema
│   ├── llm/                     # LLM 能力族
│   │   ├── llm/                 # 抽象服务、BlockAssembler、消息体、Retry
│   │   ├── llm-deepseek/        # DeepSeek 适配(SSE/files API/pricing)
│   │   ├── llm-pi-ai/           # 通过 pi-ai 接入 Anthropic/OpenAI/Gemini/...
│   │   ├── llm-retry/           # 装饰器式重试插件
│   │   ├── deepseek-llm-api-extensions/
│   │   ├── plugin-package-inventory-deepseek/
│   │   └── token-meter/         # token 计量与计价
│   ├── mcp/                     # MCP 客户端
│   │   └── mcp-client/          # Stdio/Streamable HTTP 桥
│   ├── skill/                   # Skill 能力族
│   │   ├── skill/               # Service Definition:目录合并与优先级
│   │   ├── skill-filesystem/    # Provider:从磁盘读 SKILL.md
│   │   ├── skill-badge/         # 标记徽章
│   │   └── tool-skill/          # Consumer:model-facing loader 工具
│   ├── subagent/                # 子 Agent 能力族(11 包)
│   │   ├── subagent/            # Service Definition + continuation manager
│   │   ├── subagent-spawn-in-process/
│   │   ├── subagent-fork-in-process/
│   │   ├── subagent-acp/        # 跨进程 ACP 子代理
│   │   ├── subagent-claude-code/# 调用 Claude Code 作为子代理
│   │   ├── subagent-codex/      # 调用 Codex 作为子代理
│   │   ├── subagent-dsh-sdk/    # 通过 SDK 调用
│   │   ├── subagent-in-process-driver/
│   │   ├── tool-subagent/       # 模型侧委派工具
│   │   ├── tool-subagent-control/ # 控制子代理
│   │   └── tool-subagent-report/  # 拉取子代理报告
│   ├── session/                 # 会话数据平面
│   │   ├── session-persistence/      # 写入协调、revision
│   │   ├── session-persistence-jsonl/ # JSONL 后端
│   │   ├── session-persistence-sqlite/# SQLite 后端(SCHEMA_VERSION)
│   │   ├── session-projection/       # 派生视图
│   │   ├── session-projection-cache/ # 投影缓存
│   │   ├── session-stats/
│   │   ├── session-telemetry/,session-telemetry-otel/
│   │   ├── session-title/,session-title-llm/,session-title-first-prompt-llm/,session-title-all-prompts-llm/
│   │   ├── session-log-deepseek/
│   │   └── session-checkpoint-policy/
│   ├── context/                 # 请求上下文(6 包)
│   │   ├── agent-instructions/  # 工作区 AGENTS.md
│   │   ├── file-reference/      # 通用文件引用
│   │   ├── file-reference-local/# 本地解析
│   │   ├── session-reference/   # 跨 session 引用
│   │   ├── time-context/        # 时区/时间
│   │   └── tmux-context/
│   ├── shell/                   # bash 能力族(local + sandbox + pwsh + tool)
│   ├── terminal/                # 持久化 PTY 能力族
│   ├── fs/                      # 文件系统能力族(本地 + 沙箱 + 工具)
│   ├── sandbox/                 # 进程隔离(bwrap / Landlock / Seatbelt / Windows ACL)
│   ├── web/                     # 网络能力(8 包: web + tool-web + fetch + 3 个 search providers)
│   ├── compaction/              # 上下文压缩
│   ├── goal/                    # 同 session 目标管理(goal + goal-round-driver + command-goal + tool-goal)
│   ├── plan/                    # plan-mode
│   ├── todo/                    # todo_write
│   ├── jobs/                    # 后台任务
│   ├── workflow/                # 工作流(ralph)
│   ├── schedule/                # 同 session 计划任务
│   ├── feedback/                # 人类反馈
│   ├── preset/                  # agent preset(per-session composition)
│   ├── guard/                   # 循环卫道士(repeat-tool-reminder、timeout-policy)
│   ├── hooks/                   # Claude Code / Codex 钩子桥
│   ├── settings/                # 用户设置
│   ├── credentials/             # 凭据/授权
│   ├── attachment/              # 附件(content-addressed)
│   ├── spill/                   # 大工具结果溢出
│   ├── storage/                 # 非 session 存储
│   ├── interaction/             # 审批 / 命令 / ask-user / 权限预设
│   ├── api/                     # Remote BFF + Typert gateway
│   ├── typert/                  # 类型图生成/加载/注册 + protocol
│   ├── sdk/                     # JSON-RPC TS SDK(client/server/protocol)
│   ├── acp/                     # Agent Client Protocol 服务
│   ├── extensions/              # 自修改: agent 运行时增删插件
│   ├── bundle/                  # 安装型 profile 补丁层(base/web-app/headless/sdk-app/sdk-minimal/acp-app)
│   ├── client/                  # Web 浏览器半(40+ 包: ui-* 主题、slot、对话框、WebSocket、HMR)
│   ├── host/                    # Web 服务端半(API gateway + webserver)
│   ├── boot/                    # profile / 启动胶水
│   ├── test-support/            # 测试夹具 + LLM mock server
│   ├── runtime-diagnostics/     # 不变式检查报告
│   ├── util/                    # 零依赖工具(brand、timeout、atomic-write、home-paths、crypto)
│   ├── examples/                # 复用的组合 bundle(agent-spine)
│   └── experimental/            # 私有原型:inspector、agent-team 等
├── vendor/                      # vendored 源码(cordis、pi-ai、若干子模块)
│   └── ...
├── native/                      # node-addon-landlock-run(Rust 源)
├── python/                      # Python SDK + bundled runtime(sdk + sdk-runtime)
├── docs/                        # 架构 / 子系统 / 教程 / postmortem / 国际化
├── scripts/                     # gates、生成器、verify-*(>80 个)
├── snapshots/                   # 录制会话回放(session/web/sdk/acp)
├── website/                     # VitePress 文档投影
├── .agents/                     # 代理工作流与笔记
├── AGENTS.md                    # AI 代理协作守则(根级,被 CLAUDE.md 链接)
└── pnpm-workspace.yaml
```

**核心 vs 辅助目录标注**

- **核心目录**:`core/`、`llm/`、`mcp/`、`skill/`、`subagent/`、`session/`、`context/`
- **能力/工具族**:`shell/`、`terminal/`、`fs/`、`sandbox/`、`web/`、`compaction/`、`goal/`、`plan/`、`jobs/`、`workflow/`
- **辅助/cli**:`apps/cli/`(唯一 CLI 入口)
- **Web/UI**:`client/`(浏览器 40 包)、`host/`(服务端 4 包)、`extensions/ui-cordis`
- **公共底座**:`util/`、`boot/`、`typert/`、`api/`、`sdk/`、`acp/`、`bundle/`
- **持久化/存储**:`storage/`、`session/session-persistence-*`、`attachment/`、`spill/`
- **测试/工具**:`test-support/`、`runtime-diagnostics/`、`scripts/`、`snapshots/`

---

## 3. 架构骨架

### 3.1 三大骨架位

- **Plugin 框架**:`vendor/cordis`(vendored)。每个能力都通过 `cordis.yml` 行挂载,注册项是 `ctx.effect()`/`ctx.on()`,卸载时反向 unwind。
- **类型图/远程**:`packages/typert/` 提供类型图生成器 + 运行时注册表;`packages/api/gateway` 是 Remote BFF 装配 + Typert RPC gateway。`@deepseek-ai/dsh-typert-protocol` 暴露 `Remote`/`TypertRemoteService` 等原语。
- **配置/Settings**:`packages/settings/`(用户设置,`Settings` schema),`packages/credentials/`(凭据引用),`packages/extensions/cordis-{host,client}-runner` 提供 Cordis 在宿主/客户端的加载器。

### 3.2 Agent 主循环

**位置**:`packages/core/agent-loop/src/agent.ts` 的 `ReactLoopAgent`(~543 行)+ 同包 `index.ts` 工厂(~714 行)+ `tool-calls.ts`(~289 行)。

- **驱动相位**:`{idle | maintenance | running}` 三态机(`Phase` 联合类型),`wakeDriver`/`kick`/`whenIdle` 控制。
- **Turn/Step 模型**:`turn/start → step/start → preStep → buildRequest → stream(llm) → assistant/chunk* → assistant/message → tool/call* → tools/* → step/end → turn/end`。
- **Inbox 模型**(`core/agent/src/inbox.ts`):`next-turn` / `next-step` 两个槽,`send/followup/steer/inject` 写入;`steer/inject` 进入 next-step 槽,`followup` 进入 next-turn 槽。
- **事件瀑布**:`agent/pre-step`、`agent/request`、`agent/request-error`、`agent/turn-stopping`、`agent/status`、`agent/error`、`agent/inbox/{inserted,claimed,discarded}`。`llm/stream` 是全局 waterfall(同包 `llm/src/index.ts`)。
- **scope/隔离**:`packages/core/scope/` 提供 `scopeOf()`、`scoped-events`,`agent/ctx` 携带 `Scoped` 元数据,实现 "Scope 链路"(scope chain),让注册项只对一个 agent 可见。
- **可重建性不变式**:`Model-visible ⟺ logged`(AGENTS.md 规则):任何送进 LLM 的内容必须可从 session log 重建。`SessionEventMap` 是合并可扩展的 declaration-merge map,新事件要求严格构建期类型。

### 3.3 消息模型(`packages/llm/llm/src/message.ts` + `content.ts`)

- 角色:`UserMessage`/`AssistantMessage`/`ToolResultMessage`,带 `MessageSource`/`ModelMessageSource`/`ToolMessageSource`。
- 内容块:`ContentBlockMap = { text, reasoning, image, tool-call, tool-result }`,**merge-extensible**(可经 `declare module` 增项)。
- 流式 chunk:`StreamChunk` 联合(`block-start/text-delta/reasoning-delta/tool-call-delta/block-end/usage/finish`),`BlockAssembler`(`assembler.ts`,207 行)负责拼装。
- 完成原因:`FinishReasonMap = { stop, tool-calls, max-tokens, aborted, error }`,`aborted` 携带 `LlmFailure`。
- token 用量:`TokenUsage { inputTokens, outputTokens, totalTokens?, cacheReadTokens?, cacheWriteTokens?, reasoningTokens? }`(disjoint 计数)。

### 3.4 Context 管理

- `packages/core/context/`(6 包):`agent-instructions`(工作区 AGENTS.md)、`file-reference`、`file-reference-local`、`session-reference`、`time-context`、`tmux-context`。每个都是一个 Cordis 插件,在 `systemPrompt.assemble()` 阶段被收为 prompt 段落。
- `packages/core/system-prompt/src/index.ts`(~596 行)实现 `PromptAssembly`、`renderPrompt`、`joinContextSections`、`renderContextSections`,并支持 LLM 工具 schema 注入。
- `packages/core/agent-loop/src/runtime-context.ts`(~76 行) `RuntimeContextProjection`:把已经渲染的段落作为 "context" 注入下一步请求。

### 3.5 Provider(LlmClient / 适配器)

- `LlmRuntime` 在 `ctx.llm` 上,以 waterfall 形式拦截每一次流式调用(`llm/stream` 事件)。
- 抽象接口 `LlmAdapter` 在 `llm/llm/src/index.ts`,具体适配器:
  - `llm-deepseek/src/adapter.ts`(~707 行):DeepSeek 原生协议 + SSE + 文件 API + image tokens + request pricing。
  - `llm-pi-ai/src/adapter.ts`(~420 行)+ `catalog.ts`(~908 行)+ `discovery.ts`(~284 行)+ `provider.ts`(~192 行):通过 vendored `pi-ai` 适配 Anthropic / OpenAI / Gemini / Bedrock 等;`MODALITY_GATE`、`THINKING_LEVEL_GATE` 是编译期漂移闸口。
- `prepareCall(config, signal)` 在 loop 阶段用 `agent/request` 之前,合并 adapter-defaults(reasoningEffort/maxTokens)。
- `llm/llm-deepseek/src/files-api.ts`、`upload-index.ts`、`file-store.ts` 处理文件/图片附件。
- `llm/llm-retry/` 作为可叠加的 `LlmRuntime` 装饰器。

### 3.6 TUI/REPL 与 Web/CLI

- **CLI**:`apps/cli/src/bin.ts`(50 行)调度 `parseDshArgs` → `runProfile` / `runPlugin` / `runDumpConfig`,真正启动由 `profile-boot.ts` 完成。`pnpm dsh web` 启动 Web UI(默认 `http://127.0.0.1:3080`)。
- **Web 端**:`apps/web/` 是 Vite 入口;`packages/host/webserver/` + `packages/host/frontend-static/` 提供 HTTP;`packages/client/`(40 包)拆分为 `ui-*` 主题卡,`client/store`、`client/connection`、`client/hmr`、`client/locale` 等。`ui-conversation` 是 Chat 节点装配;`client/web` 是浏览器壳。
- **Headless/SDK/ACP**:`packages/bundle/{headless,sdk-app,sdk-minimal,acp-app}` 各自挂载,共享 `bundle/base/` 的能力基线。
- 没有传统 TUI(纯终端 UI):项目优先 Web UI + SDK over JSON-RPC + ACP 协议。

### 3.7 数据持久化

- `packages/session/`(16 包):`session-persistence/{jsonl,sqlite}`、`session-projection`/`-cache`、`session-stats`、`session-telemetry`/`-otel`、`session-title`/`-llm`/`-first-prompt-llm`/`-all-prompts-llm`、`session-log-deepseek`、`session-checkpoint-policy`。
- `core/session/src/`(~3.2k 行)实现 `SessionEventMap`/`surface`/`chunk-rows`/`repair`/`preparation` 等核心。
- 写入路径:`Session.append()` 触发 `session-persistence/src/write-behind.ts` 异步落地,SQLite 用单调 `SCHEMA_VERSION`;JSONL 兼容。

---

## 4. 核心特征支持矩阵

| 能力 | 支持 | 入口/关键包 |
|---|---|---|
| 多 Agent | 是 | `core/agent/`(注册表 + factory)、`subagent/`(capability seam)、`experimental/agent-team`(私有协调) |
| 任务分类 / Planner | 部分 | `core/agent/src/model-selection.ts`(75 行) + `packages/goal/`(目标管理 + 轮次驱动 `goal-round-driver`) + `plan/plan-mode` |
| 任务拆解 | 是 | `subagent/`(in-process / fork / spawn / out-of-process / acp / claude-code / codex / dsh-sdk 8 个 provider)+ `tool-subagent*` |
| 质检 / 卫道士 | 是 | `guard/repeat-tool-reminder`、`guard/timeout-policy`、`runtime-diagnostics/` |
| MCP | 是 | `mcp/mcp-client/`(Stdio + Streamable HTTP + 重连策略 + namespace 隔离) |
| Skill | 是 | `skill/`(Service Definition + 4 个包,模型侧有 `tool-skill`) |
| Session | 是 | `core/session/` + `session/session-*`(持久化、投影、标题、遥测、检查点) |
| 多 Provider | 是 | `llm-deepseek` + `llm-pi-ai`(Anthropic/OpenAI/Gemini/Bedrock)+ `llm-retry` + `web/web-search-{deepseek,exa,perplexity}` + `extensions/cordis-*` |
| 流式响应 | 是 | `LlmRuntime` 暴露 `AsyncIterable<StreamChunk>`,`agent-loop/step()` 消费流;前端 `client/` 渲染 chunk |

**其他亮点**

- **Continuable 子代理**:`subagent/continuation.ts`(~1569 行)用 `ContinuationManager` 维护耐久型子 agent,parent 持 `AgentHandle` 通过 inbox 推消息;`start` vs `startContinuable` 区分一次性 vs 持续型。
- **Pre-step 决策**:`agent/pre-step` 是 waterfall,可拒绝 / 重写 / 标记 `startsRequestSeries`;支持请求系列(series)概念。
- **Scoping**:`packages/core/scope/` 的 `Scope` 是组合可时空(spatiotemporal)的核心原语,论文已发。
- **Cordis 概念完整实现**:`Remote`(BFF RPC)、`Typert`(类型图)、`Service`(依赖注入)齐全。
- **Bundle 模型**:`packages/bundle/` 的 `--profile X` 形式让用户可叠加补丁,默认 patch 是用户级 `cordis.patch.yml` + home-level + 命令行 `--patch`。
- **快照测试**:`snapshots/{session,web,sdk,acp}` + `vitest.snapshot.config.ts` 录制+回放,keyless;`test:web-stress` 与 `test:web:perf`。
- **快照驱动 UI**:Web UI 渲染 "Chat 节点",`ui-conversation` + `ui-trajectory` + `ui-tool` + `ui-skill` 等都是 typed 节点。

---

## 5. 关键文件清单(按职责)

### 5.1 核心循环 / 代理(~3.5k 行)

| 文件 | 行数 | 职责 |
|---|---|---|
| `packages/core/agent-loop/src/agent.ts` | 543 | `ReactLoopAgent`:turn/step 主循环、inbox、wake/latch、buildRequest、stream 消费 |
| `packages/core/agent-loop/src/index.ts` | 714 | agent-loop 工厂、注册、`PreparedAgent` 生命周期 |
| `packages/core/agent-loop/src/tool-calls.ts` | 289 | `executeToolCalls` 并行执行工具,输出 context 回到 inbox |
| `packages/core/agent-loop/src/runtime-context.ts` | 76 | 运行时上下文投影(把段落作为下一步消息) |
| `packages/core/agent-loop/src/invariant.ts` | 63 | 不变式断言 |
| `packages/core/agent/src/index.ts` | 697 | `ctx.agents` 服务、Agent 注册表、setup-commit 模型 |
| `packages/core/agent/src/inbox.ts` | 220 | Inbox 队列与 `claim` 语义 |
| `packages/core/agent/src/dispatch.ts` | 176 | `agentEvents`/`emitAgentEvent`、event 路由 |
| `packages/core/agent/src/runtime-types.ts` | 299 | AgentOptions / AgentEvent 等 |

### 5.2 LLM / Provider(~5.6k 行)

| 文件 | 行数 | 职责 |
|---|---|---|
| `packages/llm/llm/src/index.ts` | 1091 | `LlmRuntime`、waterfall 拦截、LlmError 分类 |
| `packages/llm/llm/src/types.ts` | 429 | 消息体、ContentBlockMap、FinishReasonMap、GenerateOptions |
| `packages/llm/llm/src/assembler.ts` | 207 | `BlockAssembler` 把 chunk 拼成 block |
| `packages/llm/llm/src/content.ts` | 289 | 内容块、image 投影 |
| `packages/llm/llm/src/message.ts` | 242 | `Message` 角色 + 源 |
| `packages/llm/llm/src/retry-policy.ts` | 195 | 重试策略 |
| `packages/llm/llm/src/error.ts` | 163 | `HarnessError`、`LlmError` 错误码 |
| `packages/llm/llm-deepseek/src/adapter.ts` | 707 | DeepSeek SSE 适配 |
| `packages/llm/llm-deepseek/src/serialize.ts` | 430 | 请求/响应序列化 |
| `packages/llm/llm-deepseek/src/index.ts` | 495 | DeepSeek 适配入口 |
| `packages/llm/llm-pi-ai/src/catalog.ts` | 908 | pi-ai 目录封装、`MODALITY_GATE` |
| `packages/llm/llm-pi-ai/src/adapter.ts` | 420 | pi-ai 适配 |
| `packages/llm/llm-pi-ai/src/config.ts` | 472 | pi-ai 配置 |
| `packages/llm/llm-pi-ai/src/discovery.ts` | 284 | 模型发现(端点) |
| `packages/llm/llm-pi-ai/src/stream.ts` | 231 | pi-ai 流式 |

### 5.3 Session / 上下文(~3.2k 行)

| 文件 | 行数 | 职责 |
|---|---|---|
| `packages/core/session/src/index.ts` | 1156 | `Session` 主类、append、`deriveMessages` |
| `packages/core/session/src/surface.ts` | 460 | 表面生成、replacement |
| `packages/core/session/src/types.ts` | 417 | `SessionEventMap`、`known-event-types` |
| `packages/core/session/src/chunk-rows.ts` | 369 | chunk 行投影 |
| `packages/core/system-prompt/src/index.ts` | 596 | `PromptAssembly` 装配与渲染 |
| `packages/core/scope/src/index.ts` | 204 | `Scope` 链路、scoped-events |
| `packages/core/scope/src/store.ts` | 267 | 作用域存储 |
| `packages/core/tools/src/index.ts` | 1945 | 工具注册表、pre/post-execute、JSON Schema、Python 类型桥 |

### 5.4 Skill / MCP / Subagent(~3.0k 行)

| 文件 | 行数 | 职责 |
|---|---|---|
| `packages/skill/skill/src/index.ts` | 868 | skill Service Definition(目录合并 + 优先级 + invocation) |
| `packages/mcp/mcp-client/src/tools.ts` | 559 | MCP 工具注册(命名空间 `mcp__<server>__<tool>`) |
| `packages/mcp/mcp-client/src/connection.ts` | 351 | MCP 连接管理 + 重连 |
| `packages/subagent/subagent/src/continuation.ts` | 1569 | 持续型子代理管理器(超大模块) |
| `packages/subagent/subagent/src/index.ts` | 638 | 子代理 Service Definition、`SubagentProvider` 接口 |
| `packages/subagent/subagent/src/lifecycle.ts` | 269 | 激活 + 生命周期事件 |
| `packages/subagent/subagent/src/list-children.ts` | 407 | 列出子代理 / 后裔 |

### 5.5 应用 / 入口(轻量)

| 文件 | 行数 | 职责 |
|---|---|---|
| `apps/cli/src/bin.ts` | 50 | 调度 profile / plugin / dump-config |
| `apps/cli/src/profile-boot.ts` | 307 | 启动一个 profile(挂载 bundles + patch) |
| `apps/cli/src/args.ts` | 191 | `parseDshArgs` |
| `apps/cli/src/plugin.ts` | 163 | `dsh plugin` 子命令(插件管理) |
| `apps/cli/src/process-shutdown.ts` | 77 | 优雅退出 |

---

## 6. 与 LsmAgentEmergentWork 不同的独特设计

> 列出 5 个最具差异化的设计点。

### 6.1 "Everything-is-a-Plugin" + Cordis 时间空间组合

DeepSeek Harness 没有 "core binary" 概念。**包括 Agent 循环本身在内,每一个能力都是 Cordis 插件**;用户通过 `dsh --profile web` 启动一组 bundle,叠加 `cordis.patch.yml` 和 `--patch` 覆盖。`packages/core/scope/` 的 `Scope` 实现了 "spatiotemporal composability"(论文 `arXiv:2608.25512`),让一个注册项可被限定到具体 agent / 时段 / 上下文,卸载时反向 unwind。LsmAgent 目前没有 "插件即默认" 的全栈能力,且没有 Scoped 链路原语。

### 6.2 Session Log 作为单一可信源("Model-visible ⟺ Logged")

会话是 **append-only `SessionEvent` 流**,模型可见内容必须能从 log 重建。`core/session/src/index.ts` 1156 行,核心是 `SessionEventMap`(merge-extensible 的 declaration-merge map)和 `surface`/`chunk-rows`/`repair`/`preparation` 投影。新增一个 "模型可见的输入" 强制要求新增一个 session 事件,编译期 fail-closed。LsmAgent 没有这种 "log = ground truth" 的强约束,也不要求扩展事件必须带构建期类型。

### 6.3 Capability Seam 三角色(Definition/Provider/Consumer)

每个能力(shell、terminal、fs、sandbox、skill、subagent、web、storage、webhook 等)都明确拆成 **Service Definition / Service Provider / Service Consumer** 三类包。例:`subagent/` 11 个包拆分得很细(`subagent` 定义、`subagent-{spawn,fork,acp,claude-code,codex,dsh-sdk,in-process-driver}` 提供,`tool-subagent*` 消费)。LsmAgent 通常把能力集中在单一模块,没有这种三角色强约束,也缺少 `tool-subagent-control` / `tool-subagent-report` 这种细粒度模型侧工具拆分。

### 6.4 Typert 类型图 + Remote BFF 端到端 RPC

自研 **`@deepseek-ai/dsh-typert-protocol`** + `packages/typert/{generator,loader,registry,protocol}` + `packages/api/gateway` 构成完整 RPC 类型系统:服务端与客户端共享 "类型图",`Remote` / `TypertRemoteService` 让客户端像调用本地 Service 一样调用远端,编译期约束,而不是写 OpenAPI 文档。`packages/sdk/{client,server,protocol}` + Python SDK + `subagent-dsh-sdk` 都基于同一套。这套体系 LsmAgent 完全没有,客户端/服务端类型同步靠手工或 JSON Schema 兜底。

### 6.5 Continuable Subagent + ACP/Claude Code/Codex 异构子代理

`subagent/continuation.ts`(1569 行)用 `ContinuationManager` 管理 **可继续** 的子 agent(一次创建,后续通过 inbox 推动多轮),并提供 **跨产品/跨协议** 子代理:`subagent-claude-code`、`subagent-codex`、`subagent-acp`、`subagent-dsh-sdk`、`subagent-fork-in-process`、`subagent-spawn-in-process`。模型侧有三个独立工具:`tool-subagent`(委派)、`tool-subagent-control`(控制)、`tool-subagent-report`(拉取报告),分别管理不同生命周期。LsmAgent 子代理模型相对单一(通常只支持 in-process 子 agent),没有把"跨产品子代理"作为一等公民,也没有 `start` vs `startContinuable` 区分。

### 6.6(补充)声明合并 + 编译期漂移闸口

`ContentBlockMap`/`FinishReasonMap`/`SessionEventMap` 全部用 `declare module` 合并扩展,新类型若漏改下游会 **编译失败**。`llm-pi-ai/src/catalog.ts` 中的 `MODALITY_GATE` / `THINKING_LEVEL_GATE` 是 **Record 键漂移闸口** — pi-ai 升级时改字段会立刻在 dsh 编译期报警,而不是运行期悄悄变窄。LsmAgent 缺少这种 "升级安全性" 抽象。

---

## 7. 调研结论

DeepSeek Harness 是一个以 **Cordis 插件化 + 类型合并扩展 + 单一可信日志 + 三角色能力缝合 + 类型图 RPC** 为骨架的工业级 Agent harness。其工程化深度(247 包 / 51 组 / 双语 gates / 100% 文件覆盖率 / snapshot 录制回放)显著高于一般 Agent 框架,且把 "spatiotemporal composability" 提到了论文层。架构对 LsmAgent 最有借鉴价值的是:
1. Session Log 作为单一可信源 + 编译期 fail-closed 事件扩展;
2. Capability Seam 三角色拆分(Definition/Provider/Consumer);
3. Scope 链路作为多 agent 隔离和卸载原语;
4. 类型图驱动的 RPC(Typert)减少 schema 漂移;
5. 持续型 vs 一次性子代理的明确区分(continuable subagent)。
