# Claude Code 源码调研

> 调研对象:`/usr/local/LsmGitOpenSource/claudecode`(Anthropic 官方 Claude Code 源码)。
> 调研日期:2026-09-04。代码规模 ~218k 行 TS/TSX,源码版本基于现有快照。
> 注意:本仓库是 Claude Code 的 *源码库* 视图(已无 `package.json`、无 git 历史,仅保留 `src/`、`vendor/`、`node_modules/` 及一份 `ARCHITECTURE_README.md` + 多份 Mermaid 图)。所有文件路径均为绝对路径。

---

## 1. 项目元信息

| 项目 | 描述 |
| --- | --- |
| 名称 | Claude Code(CLI 形态的 AI 编程 Agent) |
| 主语言 | TypeScript / TSX,目标运行时 Bun(`bun:bundle` 内置特性) |
| 前端框架 | React + [Ink](https://github.com/vadimdemedes/ink)(终端 TUI) |
| LLM SDK | `@anthropic-ai/sdk` + `@modelcontextprotocol/sdk` |
| 总代码量 | `src/` 下 ~218,405 行(`.ts` + `.tsx`) |
| 主要第三方依赖 | `lodash-es`、`strip-ansi`、`zod`、`crypto`(UUID);UI 端 React/Ink;本地原生模块见 `vendor/` |
| 入口点 | `src/main.tsx`(实际 CLI 启动)、`src/entrypoints/cli.tsx`(fast-path 分发);SDK 入口 `src/entrypoints/agentSdkTypes.ts`;MCP 服务入口 `src/entrypoints/mcp.ts` |
| 构建系统 | Bun bundle(`feature('XXX')` 编译期 DCE 条件分支);输出二进制 `claude`,通过 `bun:bundle` 暴露的 `feature()` 与 `MACRO.*` 宏 |
| 测试目录 | `src/tools/testing/`(每个工具内置 `testing/` 子目录,如 `tools/BashTool/testing/`);`vendor/` 内含原生模块 C++ 源码(`audio-capture-src`、`image-processor-src`、`modifiers-napi-src`、`url-handler-src`) |
| 输出二进制 | 单文件 CLI(Bun 打包),支持子命令 `claude daemon`、`claude remote-control`、`claude ps/logs/attach/kill`、`claude mcp` 等 |
| 文档 | `ARCHITECTURE_README.md` + 9 份 `*.mmd` 架构图(overall/conversation/tool/memory/multi-agent/permission/MCP/query-engine) |

入口分发思路(`src/entrypoints/cli.tsx`):
- `--version`/`-v`:**零依赖** fast-path,只输出 `MACRO.VERSION`;
- `--dump-system-prompt`、`--claude-in-chrome-mcp`、`--chrome-native-host`、`--computer-use-mcp`、`--daemon-worker`、`remote-control`/`rc`/`remote`/`sync`/`bridge`、`daemon`、`ps|logs|attach|kill`/`--bg`/`--background`、`new|list|reply`(模板作业)、`environment-runner`、`self-hosted-runner`、`--worktree --tmux` 等均各自走快速通道;
- 默认路径 `import('../main.js')` → `cliMain()`。

---

## 2. 目录树(顶层 + 二级)

```
/usr/local/LsmGitOpenSource/claudecode/
├── src/                     # 主代码(218k 行)
│   ├── main.tsx             # CLI 主入口(4683 行)
│   ├── query.ts             # 主对话循环 query()/queryLoop()(1729 行)
│   ├── QueryEngine.ts       # 高级查询编排类(1295 行)
│   ├── Tool.ts              # 工具基类 & 注册中心(792 行)
│   ├── tools.ts             # 工具聚合 & 默认参数(389 行)
│   ├── Task.ts / tasks.ts   # 任务类型 & 调度(125+39 行)
│   ├── context.ts           # Context 注入辅助(189 行)
│   ├── setup.ts             # 安装/初始化(477 行)
│   ├── history.ts / cost-tracker.ts / costHook.ts / ink.ts / interactiveHelpers.tsx
│   │   replLauncher.tsx     # REPL 启动壳(22 行,仅 import)
│   ├── components/          # React Ink UI 组件(144 个 .tsx)
│   ├── screens/             # 大型屏幕组件:REPL.tsx(5005 行)、Doctor.tsx、ResumeConversation.tsx
│   ├── hooks/               # React hooks(useCanUseTool、useInputBuffer、useGlobalKeybindings 等)
│   ├── context/             # React Context(mailbox、notifications、stats、voice 等)
│   ├── state/               # 全局 AppState/AppStateStore/selectors
│   ├── tools/               # 工具实现目录(43+ 个工具)
│   ├── tasks/               # 任务运行器(本地/远程/进程内 teammate/Shell/Dream)
│   ├── skills/              # 技能注册与加载(bundled/*.ts 17 个内置技能)
│   ├── services/            # 业务服务(api/mcp/compact/SessionMemory/MagicDocs/...)/agentTool
│   ├── commands/            # 100+ 个 slash 命令
│   ├── utils/               # 通用工具(消息处理、权限、文件历史、Shell、tokens、附件 等)
│   ├── constants/           # prompts、betas、oauth、toolLimits、systemPromptSections...
│   ├── entrypoints/         # cli / mcp / sdk / agentSdkTypes / init / sandboxTypes
│   ├── types/          # 共享类型
│   ├── bridge/              # 远程 Bridge / Remote Control(30+ 文件)
│   ├── buddy/               # 桌面宠物(CompanionSprite)
│   ├── components/agents/   # Agent 状态展示
│   ├── components/permissions/ # 权限对话框(Bash/FileEdit/Sandbox 等)
│   ├── components/memory/   # 记忆相关 UI
│   ├── coordinator/         # Coordinator 模式入口(coordinatorMode.ts 369 行)
│   ├── voice/               # 语音模式开关
│   ├── outputStyles/        # 输出风格加载器
│   ├── plugins/             # 插件加载
│   ├── remote/              # 远端会话管理(WebSocket/Permission Bridge/SDK 适配)
│   ├── server/              # 直连会话服务
│   ├── memdir/              # 记忆目录加载
│   ├── migrations/          # 配置迁移脚本
│   ├── ink/                 # 自定义 Ink 扩展
│   ├── vim/                 # vim 键位
│   ├── keybindings/         # 键位映射
│   ├── assistant/           # 后台 assistant 守护
│   ├── bootstrap/           # 启动状态
│   ├── native-ts/           # 与 NAPI 原生模块绑定
│   ├── queries/             # (本仓库未发现独立 queries 目录;查询主逻辑在 query.ts/QueryEngine.ts)
│   ├── hooks/               # React hook 集合
│   └── .../                 # 其余辅助模块
├── vendor/                  # 原生模块源码
│   ├── audio-capture-src
│   ├── image-processor-src
│   ├── modifiers-napi-src
│   └── url-handler-src
├── node_modules/            # 199 个第三方依赖
├── architecture_overview.{mmd,png,svg,jpg}
├── complete_architecture.{mmd,png,svg,jpg}
├── conversation_flow.{mmd,...}
├── tool_execution_flow.{mmd,...}
├── memory_management.{mmd,...}
├── multi_agent_collaboration.{mmd,...}
├── permission_system.{mmd,...}
├── mcp_integration.{mmd,...}
├── query_engine_detail.{mmd,...}
├── prompt_cn_list.md
└── ARCHITECTURE_README.md
```

### 核心目录(Agent / Runtime / Tools / Context / Llm / MCP / Skill / Prompt)

| 目录 | 主要职责 |
| --- | --- |
| `src/` 顶层(`query.ts`、`QueryEngine.ts`、`Tool.ts`、`tools.ts`、`main.tsx`) | Agent 运行时主循环 |
| `src/tools/` | 43+ 个工具实现(Bash、FileRead、FileEdit、Glob、Grep、WebFetch、WebSearch、Agent、AskUser、TodoWrite、MCP、Skill、TaskCreate/Get/List/Output/Stop/Update、ScheduleCron、Sleep、NotebookEdit、PowerShell、REPLTool、SyntheticOutput、TeamCreate/Delete、ToolSearch、Config、SendMessage、Enter/ExitPlanMode、EnterWorktree/ExitWorktree、Brief、ReadMcpResource、ListMcpResources、McpAuth、RemoteTrigger 等) |
| `src/tools/AgentTool/` | 子 Agent 运行(runAgent 973 行、loadAgentsDir 755 行、AgentTool.tsx、prompt、resumeAgent、forkSubagent、agentMemory 等) |
| `src/tasks/` | 任务抽象(LocalMainSessionTask、LocalAgentTask、InProcessTeammateTask、RemoteAgentTask、LocalShellTask、DreamTask) |
| `src/coordinator/` | Coordinator 模式(协调多个 Worker) |
| `src/commands/agents/` | `/agents` 命令及代理管理 UI |
| `src/services/api/` | Anthropic API 客户端与适配(claude.ts 3419 行、client.ts、withRetry、dumpPrompts、promptCacheBreakDetection 等) |
| `src/services/mcp/` | MCP 连接管理(client.ts 3348 行、auth.ts 2465 行、config.ts 1578 行、MCPConnectionManager、useManageMCPConnections 1141 行、SDK Transport、官方注册表、OAuth/XAA 登录) |
| `src/skills/` | 技能加载(loadSkillsDir 1086 行、bundled/*.ts 内置 17 个技能:batch/loop/remember/updateConfig/skillify/simplify/stuck/verify/scheduleRemoteAgents 等) |
| `src/services/SessionMemory/` | 会话记忆(sessions memory):sessionMemory.ts 495 行 + prompts |
| `src/services/extractMemories/` | 记忆提取 |
| `src/services/MagicDocs/` | CLAUDE.md 自动更新 |
| `src/services/compact/` | 上下文压缩(compact 1705 行、microCompact、autoCompact、reactiveCompact、sessionMemoryCompact) |
| `src/services/tools/` | 工具执行编排(StreamingToolExecutor 530 行、toolOrchestration、toolHooks) |
| `src/constants/prompts.ts` | 系统提示词生成(914 行) |
| `src/services/SessionMemory/prompts.ts` | 长期记忆提示词(324 行) |
| `src/bridge/` | Bridge 远程会话(30+ 文件,remote control、SSH-backed session、pollConfig、sessionRunner、replBridge、messaging) |

### 辅助目录(CLI / TUI / UI / Utils / Tests)

| 目录 | 职责 |
| --- | --- |
| `src/entrypoints/` | CLI/MCP/SDK 三入口 + sandboxTypes、agentSdkTypes、init |
| `src/cli/` | 进程内子命令(`bg.js`、`exit.ts`、`print.ts`、`ndjsonSafeStringify.ts`、`remoteIO.ts`、`structuredIO.ts`、`transports/`、`update.ts`、handlers/) |
| `src/replLauncher.tsx` | REPL 启动壳(仅动态 import `App.tsx` + `screens/REPL.tsx`) |
| `src/components/` | 144 个 React/Ink 组件(App、Messages、Message、MessageRow、PromptInput/*、MemoryUsageIndicator、Permission*、MCPServer* 等) |
| `src/screens/` | 顶级屏幕(REPL.tsx 5005 行、Doctor、ResumeConversation) |
| `src/hooks/` | React hook 工具(键位、命令队列、命令键位、IDE 集成、权限、剪贴板、通知 等) |
| `src/context/` | React Context Provider(mailbox、notifications、stats、voice、modal、overlay、promptOverlay、QueuedMessage、fpsMetrics) |
| `src/state/` | AppState/AppStateStore/selectors |
| `src/utils/` | 通用工具(消息处理、tokens、Shell、文件历史、附件、提示上下文、消息队列、权限、沙箱、hook 引擎、键位、图像校验、远程控制 等) |
| `src/utils/messages/` | 消息归一化、SDK 消息映射、系统初始化消息 |
| `src/utils/permissions/` | 文件系统/Shell 权限 |
| `src/commands/` | 100+ slash 命令(add-dir、advisor、agents、brief、bughunter、commit、commit-push-pr、compact、config、cost、ctx_viz、doctor、effort、env、export、fast、feedback、help、hooks、ide、install、memory、mobile、model、init、review、security-review、session、skills、task、terminal-setup、theme、update、voice...) |
| `src/memdir/` | 长期记忆目录加载 |
| `src/outputStyles/` | 输出风格 |
| `src/plugins/` | 插件加载与缓存 |
| `src/voice/` | 语音模式开关 |
| `src/upstreamproxy/` | 上游代理(relay) |
| `src/remote/` | 远端会话管理(WebSocket、SDK 适配、Permission Bridge) |
| `src/server/` | 直连会话服务 |
| `src/buddy/` | 桌面宠物 |
| `src/vim/` | vim 模式 |
| `src/keybindings/` | 键位映射配置 |
| `src/migrations/` | 配置/版本迁移脚本 |
| `src/native-ts/` | NAPI 原生模块绑定 |
| `src/tools/testing/` | 每个工具的测试 fixture(在工具目录内,如 `BashTool/testing/`) |
| `src/assistant/` | 后台 assistant 守护 |

---

## 3. 架构骨架

### 3.1 Agent 主循环位置

- **核心入口** `src/main.tsx`(4683 行):CLI 启动;`run()` 用 `commander` 注册所有子命令;末尾构建 `AppState` → 调 `launchRepl()` → 渲染 `<App><REPL/></App>`。
- **REPL 主屏** `src/screens/REPL.tsx`(5005 行):终端交互主界面;承载 PromptInput、Messages、权限请求、Agent 状态、Teammate 视图等。
- **查询循环** `src/query.ts`(1729 行):
  - `export async function* query(params)`(`query.ts:219`)是顶层 AsyncGenerator,内部委托给 `queryLoop(params, consumedCommandUuids)`(`query.ts:241`)。
  - `queryLoop` 是真正的 Agent 主循环:多轮对话、Tool 调用、Stop Hook、Reactive Compact、Microcompact、Tool Result Budget、Stream Recovery、命令生命周期通知;通过 `state = { ... }` 模式管理可变跨轮状态。
- **高层编排** `src/QueryEngine.ts`(1295 行):
  - `export class QueryEngine`(`QueryEngine.ts:184`)封装 QueryConfig、Session Info、系统提示词装配、压缩、附件、记忆附件去重、Reactive Compact、技能预取、消息归一化等。
  - `export async function* ask({ ... })`(`QueryEngine.ts:1186`)是外部 SDK 风格的查询入口。

### 3.2 消息模型

- **核心消息类型** 集中在 `src/entrypoints/agentSdkTypes.ts` → `sdk/coreTypes.ts`、`sdk/runtimeTypes.ts`、`sdk/toolTypes.ts`:
  - `SDKMessage`(联合类型)、`SDKUserMessage`、`SDKAssistantMessage`、`SDKResultMessage`、`SDKSystemMessage`、`SDKCompactBoundaryMessage`、`SDKUserMessageReplay`、`SDKPermissionDenial`、`SDKSessionInfo` 等。
- **内部消息** 见 `src/utils/messages.ts`(>3000 行):`createUserMessage`、`createAssistantAPIErrorMessage`、`createMicrocompactBoundaryMessage`、`createToolUseSummaryMessage`、`createSystemMessage`、`stripSignatureBlocks`、`normalizeMessagesForAPI`、`createAttachmentMessage`、`filterDuplicateMemoryAttachments`、`getAttachmentMessages`、`startRelevantMemoryPrefetch`。
- **消息映射** `src/utils/messages/mappers.ts`(本地 ↔ SDK)、`src/utils/messages/systemInit.ts`(系统初始化消息)。
- **流式事件** 在 query 循环中以 `StreamEvent | RequestStartEvent | Message | TombstoneMessage | ToolUseSummaryMessage` 形式 yield;`StreamingToolUse`、`StreamingToolUse`(`utils/messages.ts:2915/2921`)与 `services/tools/StreamingToolExecutor.ts`(530 行)配套。

### 3.3 Context 管理

- **装配层** `src/services/api/claude.ts`:
  - `buildSystemPromptBlocks()`、`getSystemPrompt`(`constants/prompts.ts:914`)、`fetchSystemPromptParts`(`utils/queryContext.ts`)、`prependUserContext`、`appendSystemContext`(`utils/api.ts`)。
  - `prependUserContext` 注入用户级上下文(OS、shell、git、env、CLAUDE.md 等);`appendSystemContext` 注入尾部系统片段。
- **压缩管线** `src/services/compact/`:
  - `compact.ts`(1705 行)、`microCompact.ts`、`apiMicrocompact.ts`、`autoCompact.ts`、`reactiveCompact.ts`(feature 门禁)、`sessionMemoryCompact.ts`、`compactWarningHook.ts`、`compactWarningState.ts`、`grouping.ts`、`timeBasedMCConfig.ts`、`postCompactCleanup.ts`、`prompt.ts`。
- **记忆系统**:
  - **会话内** `src/services/SessionMemory/sessionMemory.ts`(495 行)+ `prompts.ts`(324 行) + `sessionMemoryCompact.ts`;
  - **跨会话** `src/memdir/`、`src/services/extractMemories/extractMemories.ts` + `prompts.ts`;
  - **CLAUDE.md 自动维护** `src/services/MagicDocs/magicDocs.ts` + `prompts.ts`。
- **Context 计算** `src/utils/tokens.ts`、`src/utils/analyzeContext.ts`、`src/services/api/tokenEstimation.ts`、`src/query/tokenBudget.ts`、`src/components/ContextVisualization.tsx`。
- **状态** `src/state/AppState.tsx` + `AppStateStore.ts` + `selectors.ts` + `onChangeAppState.ts`(持久化变更回调)。

### 3.4 Provider(LlmClient)

- **客户端工厂** `src/services/api/client.ts`:`export async function getAnthropicClient({...})`(`client.ts:88`),支持自定义 baseURL、auth header、CA 证书、代理、自定义 fetch。
- **API 调用** `src/services/api/claude.ts`(3419 行):
  - `queryModelWithStreaming`(异步生成器,`claude.ts:752`)是流式入口;
  - `queryModelWithoutStreaming`(`claude.ts:709`)、`executeNonStreamingRequest`(`claude.ts:818`)、`verifyApiKey`(`claude.ts:530`)、`queryHaiku`(`claude.ts:3241`)、`queryWithModel`(`claude.ts:3300`)。
  - `userMessageToMessageParam`、`assistantMessageToMessageParam`、`buildSystemPromptBlocks`、`getExtraBodyParams`、`getPromptCachingEnabled`、`getCacheControl`、`configureTaskBudgetParams`、`addCacheBreakpoints`、`stripExcessMediaItems`、`cleanupStream`、`updateUsage`、`accumulateUsage`、`getMaxOutputTokensForModel`、`adjustParamsForNonStreaming` 等。
- **重试 & 错误** `src/services/api/withRetry.ts`、`src/services/api/errors.ts`(分类 `categorizeRetryableAPIError`)、`src/services/api/errorUtils.ts`、`src/services/api/promptCacheBreakDetection.ts`。
- **首次 token 时间** `src/services/api/firstTokenDate.ts`;**用量** `src/services/api/usage.ts`、`src/services/api/logging.ts`、`src/services/api/emptyUsage.ts`、`src/services/api/metricsOptOut.ts`、`src/cost-tracker.ts` + `src/costHook.ts`。

> 设计上只内置 Anthropic Claude 一个 Provider(未见 OpenAI/Gemini/Llama 适配层),但 `getAnthropicClient` 接受 `baseURL`/`customFetch`,支持第三方代理。

### 3.5 TUI / REPL

- **渲染壳**:`src/replLauncher.tsx`(22 行) → `src/components/App.tsx`(55 行,React Compiler 已启用) → `src/screens/REPL.tsx`(5005 行)。
- **Ink 抽象** `src/ink.ts`、`src/ink/`(自定义 Ink 扩展)。
- **核心组件**:`App`、`REPL`、`Message`、`MessageModel`、`MessageRow`、`MessageResponse`、`Messages`、`messages/`、`PromptInput/*`(PromptInput、HistorySearchInput、ShimmeredInput、Notifications、ModeIndicator、Footer、HelpMenu、QueuedCommands、StashNotice)、`BaseTextInput`、`Markdown`、`HighlightedCode`、`CompactSummary`、`MemoryUsageIndicator`、`CostThresholdDialog`、`FastIcon`、`Feedback`、`DesignSystem/`、`LogoV2`、`Passes/`、`CustomSelect/`、`ClaudeCodeHint/`、`HelpV2/`、`LspRecommendation/`。
- **键位** `src/hooks/useGlobalKeybindings.tsx`、`useCommandKeybindings.tsx`、`useCommandQueue.ts`、`useExitOnCtrlCD.ts`、`useExitOnCtrlCDWithKeybindings.ts`、`useDoublePress.ts`、`useInputBuffer.ts`、`src/keybindings/`、`src/vim/`。
- **Context Providers** `src/context/`(`AppStateProvider`、`StatsProvider`、`FpsMetricsProvider`、`NotificationsProvider`、`MailboxProvider`、`VoiceProvider`、`ModalProvider`、`OverlayProvider`、`PromptOverlayProvider`、`QueuedMessageProvider`)。
- **远程渲染** `src/bridge/replBridge.tsx`、`bridge/sessionRunner.ts` 让本地 REPL 渲染 + 远端 SSH-backed session 执行(`bridge/types.ts`、`bridge/createSession.ts`)。
- **Bridge 远程控制** 独立的 `bridgeMain()`(见 `cli.tsx:127` + `bridge/bridgeMain.ts`),接受 `remote-control/rc/remote/sync/bridge` 子命令;通过 OAuth token + GrowthBook 灰度开关控制。

---

## 4. 核心特征

| 维度 | 是否 | 关键证据 |
| --- | --- | --- |
| 多 Agent(子代理) | 是 | `tools/AgentTool/`(`AgentTool.tsx`、`runAgent.ts`、`loadAgentsDir.ts`、`builtInAgents.ts`、`agentMemory*.ts`、`agentColorManager.ts`、`forkSubagent.ts`、`resumeAgent.ts`、`prompt.ts`);**Swarm/Coordinator** 模式 `coordinator/coordinatorMode.ts` + `tasks/InProcessTeammateTask/` + `tasks/LocalAgentTask/` + `tasks/RemoteAgentTask/` + `tasks/LocalShellTask/` + `tasks/DreamTask/` + `services/AgentSummary/agentSummary.ts` |
| 任务分类 | 是 | `commands/jobs/classifier.js`(query.ts 中 `feature('TEMPLATES')` 条件加载);`/cost`、`/review`、`/security-review`、`/effort`、`/model`、`/advisor` 等 |
| 任务拆解 | 是 | `tools/TodoWriteTool/`、`tools/TaskCreateTool/`、`TaskGet/List/Output/Stop/Update`(`tools/Task*`)、`/task` 命令;**AgentTool** 可拆解子任务交给子 Agent |
| 质检/自检 | 是 | `skills/verify.ts`、`verifyContent.ts`、`debug.ts`、`simplify.ts`、`stuck.ts`、`review`、`commands/security-review`、`code-review`、`commands/ant-trace`、`commands/autofix-pr`、`commands/good-claude`;CLI skill `verifyContent`/`simplify`/`remember`/`skillify`/`stuck`/`debug`/`updateConfig` |
| MCP | 是 | `services/mcp/` 全面支持:MCPConnectionManager、client.ts、auth.ts、config.ts、useManageMCPConnections、officialRegistry、OAuth、elicitHandler、SdkControlTransport、InProcessTransport、vscodeSdkMcp、xaa/xaaIdpLogin、headersHelper、channelAllowlist/Notification/Permissions、mcpStringUtils、envExpansion、normalization、utils、types;MCP 工具 `tools/MCPTool/`、`tools/ListMcpResourcesTool/`、`tools/ReadMcpResourceTool/`、`tools/McpAuthTool/`、`components/mcp/`、`components/MCPServer*Dialog.tsx` |
| Skill | 是 | `skills/`(`loadSkillsDir.ts` 1086 行、`bundledSkills.ts`、`mcpSkillBuilders.ts`、`bundled/` 17 个内置技能);`tools/SkillTool/`、`tools/ToolSearchTool/` |
| Session | 是 | `entrypoints/agentSdkTypes.ts`(SDK session、`ListSessionsOptions`/`GetSessionInfoOptions`/`ForkSessionOptions`/`ForkSessionResult`/`SDKSessionInfo`);`utils/sessionStorage.ts`(recordTranscript、flushSessionStorage);`assistant/sessionHistory.ts`;`cli/bg.js`(`ps|logs|attach|kill` + `--bg`/`--background`) |
| 多 Provider | 否(默认仅 Anthropic) | 仅内置 `services/api/claude.ts`;但 `client.ts:getAnthropicClient` 接受 `baseURL`/`customFetch`,可对接代理;另支持 `filesApi.ts`、`grove.ts`、`bootstrap.ts`(自建 OAuth) |
| 流式响应 | 是 | `queryModelWithStreaming`(async generator)+ `StreamingToolExecutor.ts`(530 行)+ `StreamingToolUse`/`StreamingThinking` 类型(`utils/messages.ts:2915/2921`)+ SSE/stream-json transport(`cli/transports/`、`cli/ndjsonSafeStringify.ts`、`cli/structuredIO.ts`) |

其它显著特征:
- **权限模型**:多模式 `default/auto/ask/yolo/plan`(见 `utils/permissions/`、`components/permissions/`,含 BashPermissionRequest、SandboxPermissionRequest、FileEditPermissionRequest、AskUserQuestionPermissionRequest、PermissionDialog、PermissionPrompt、FallbackPermissionRequest、SkillPermissionRequest 等)。
- **沙箱**:`utils/bash/`、`tools/BashTool/shouldUseSandbox.ts` + `bashPermissions.ts` + `bashSecurity.ts` + `pathValidation.ts` + `readOnlyValidation.ts` + `destructiveCommandWarning.ts` + `modeValidation.ts` + `sedValidation.ts` + `sedEditParser.ts` + `commandSemantics.ts` + `commentLabel.ts` + `utils.ts`;**SandboxPermissionRequest** UI。
- **Hooks 系统**:`hooks/`、`utils/hooks/`(postSamplingHooks、stopHooks、pre/post tool hooks);用户可在 `settings.json` 注册。
- **插件**:`services/plugins/`、`utils/plugins/pluginLoader.ts`、`types/plugin.ts`。
- **远程控制 / 远端会话**:`bridge/`(30+ 文件)、`remote/`、`server/`、`upstreamproxy/`;CCR(`CLAUDE_CODE_REMOTE`)模式独立环境配置。
- **Daemon & 后台任务**:`cli/bg.js`(`--bg`/`--background`)、`tasks/LocalShellTask/`(后台 shell)、`utils/background/`、`utils/backgroundHousekeeping.ts`。
- **语音**:`voice/`、`services/voice.ts`、`voiceKeyterms.ts`、`voiceStreamSTT.ts`、`components/voice/`(`voice.tsx`)、`buddy/`(桌面宠物伴侣)。
- **MagicDocs** 自动维护 CLAUDE.md、`SessionMemory` 长期记忆、`autoDream` 后台整理。
- **Worktree + tmux**:`utils/worktree.ts`、`utils/worktreeModeEnabled.ts`、`tools/EnterWorktreeTool/`、`tools/ExitWorktreeTool/`。
- **Vim 模式** `vim/` + `vimModeEnabled` 类设置。
- **Reactive Compact & Context Collapse**:`services/compact/reactiveCompact.ts`(`feature('REACTIVE_COMPACT')`)、`services/contextCollapse/index.ts`(`feature('CONTEXT_COLLAPSE')`)。

---

## 5. 关键文件清单(20 个核心源文件)

| # | 路径(绝对) | 职责 | 行数(估) |
| --- | --- | --- | --- |
| 1 | `/usr/local/LsmGitOpenSource/claudecode/src/main.tsx` | CLI 主入口,Commander 子命令 + REPL 启动 | ~4,683 |
| 2 | `/usr/local/LsmGitOpenSource/claudecode/src/query.ts` | `query()` / `queryLoop()` 主对话循环(AsyncGenerator) | ~1,729 |
| 3 | `/usr/local/LsmGitOpenSource/claudecode/src/QueryEngine.ts` | 高级查询编排类 `QueryEngine` + `ask()` SDK 入口 | ~1,295 |
| 4 | `/usr/local/LsmGitOpenSource/claudecode/src/Tool.ts` | 工具基类 `Tool` + 工具注册/查找 | ~792 |
| 5 | `/usr/local/LsmGitOpenSource/claudecode/src/tools.ts` | 工具聚合 & 默认开关 | ~389 |
| 6 | `/usr/local/LsmGitOpenSource/claudecode/src/services/api/claude.ts` | Anthropic SDK 适配,流式/非流式 query、prompt caching | ~3,419 |
| 7 | `/usr/local/LsmGitOpenSource/claudecode/src/services/api/client.ts` | `getAnthropicClient()` 工厂(baseURL/CA/proxy/fetch) | ~389 |
| 8 | `/usr/local/LsmGitOpenSource/claudecode/src/services/mcp/client.ts` | MCP 客户端核心,工具/资源/通知/elicitation | ~3,348 |
| 9 | `/usr/local/LsmGitOpenSource/claudecode/src/services/mcp/MCPConnectionManager.tsx` | MCP 连接生命周期管理(React 组件) | ~72 |
| 10 | `/usr/local/LsmGitOpenSource/claudecode/src/services/tools/StreamingToolExecutor.ts` | 流式工具执行器 | ~530 |
| 11 | `/usr/local/LsmGitOpenSource/claudecode/src/services/compact/compact.ts` | 上下文压缩(主入口) | ~1,705 |
| 12 | `/usr/local/LsmGitOpenSource/claudecode/src/services/SessionMemory/sessionMemory.ts` | 会话长期记忆 | ~495 |
| 13 | `/usr/local/LsmGitOpenSource/claudecode/src/skills/loadSkillsDir.ts` | 技能目录加载 + 解析 | ~1,086 |
| 14 | `/usr/local/LsmGitOpenSource/claudecode/src/tools/AgentTool/runAgent.ts` | 子 Agent 运行(runAgent 主体) | ~973 |
| 15 | `/usr/local/LsmGitOpenSource/claudecode/src/tools/AgentTool/loadAgentsDir.ts` | 子 Agent 描述加载 | ~755 |
| 16 | `/usr/local/LsmGitOpenSource/claudecode/src/tasks/LocalMainSessionTask.ts` | 主会话任务 | ~479 |
| 17 | `/usr/local/LsmGitOpenSource/claudecode/src/coordinator/coordinatorMode.ts` | Coordinator 多 Agent 协调模式 | ~369 |
| 18 | `/usr/local/LsmGitOpenSource/claudecode/src/screens/REPL.tsx` | 终端主屏(5005 行,TUI 核心) | ~5,005 |
| 19 | `/usr/local/LsmGitOpenSource/claudecode/src/entrypoints/cli.tsx` | CLI fast-path 分发(动态 import) | ~302 |
| 20 | `/usr/local/LsmGitOpenSource/claudecode/src/constants/prompts.ts` | 系统提示词生成(`getSystemPrompt`) | ~914 |

> 备注:`src/services/mcp/auth.ts`(2,465)、`src/services/mcp/config.ts`(1,578)、`src/services/mcp/useManageMCPConnections.ts`(1,141)、`src/components/Message.tsx`(626)、`src/components/PromptInput/PromptInput.tsx`(>900,见文件计数)、`src/hooks/useCanUseTool.tsx`(203)、`src/services/api/dumpPrompts.ts`、`src/utils/messages.ts`(~3000 行)、`src/bridge/replBridge.tsx`、`src/components/PermissionDialog.tsx`、`src/components/permissions/SandboxPermissionRequest.tsx`、`src/components/permissions/FilePermissionDialog/`、`src/utils/sessionStorage.ts`、`src/services/tools/toolOrchestration.ts`(188) 等亦为关键支撑,未列入 Top-20。

---

## 6. 与 LsmAgentEmergentWork 不同的设计亮点

1. **Bun bundle + `feature()` 编译期 DCE**:用 `feature('REACTIVE_COMPACT')`、`feature('COORDINATOR_MODE')`、`feature('ABLATION_BASELINE')` 等守卫整段 require/import,做死代码消除;顶层 CLI 用 fast-path(`src/entrypoints/cli.tsx`)对 `--version`、`--dump-system-prompt`、`--daemon-worker` 等命令动态 import 主程序,大幅缩短启动耗时。`MACRO.VERSION` 等值也在 build 期 inline。这与 LsmAgentEmergentWork 直接 ESM/TS 加载全部模块的策略不同。
2. **多 Agent Swarm + Coordinator 模式**:除传统 `AgentTool`(子代理工具),还有 `coordinator/coordinatorMode.ts` 协调器模式 + `tasks/InProcessTeammateTask/` + `LocalAgentTask` + `RemoteAgentTask` + `LocalShellTask` + `DreamTask`,支持*队友 Agent*(Teammate)、远程后台 Agent、后台 Shell、Agent 后台整理(Dream),配合 `services/AgentSummary/agentSummary.ts` 汇总;`AgentSwarm` 模式下可 fork、resume 子 agent(`AgentTool/forkSubagent.ts`、`resumeAgent.ts`)。这是 LsmAgentEmergentWork 当前未覆盖的“Team/Swarm”级能力。
3. **跨层级的 Context 三段压缩管线**:`microCompact` → `autoCompact` → `reactiveCompact` → `sessionMemoryCompact`,叠加 `contextCollapse`(feature 门禁);同时通过 `services/SessionMemory/` 长期记忆 + `services/extractMemories/` 提取 + `services/MagicDocs/` 自动维护 CLAUDE.md + `util/analyzeContext.ts` 与 `components/ContextVisualization.tsx` 可视化,形成“短期/中期/长期/项目文件”四级 Context 体系,远超单一压缩策略。
4. **远端 Bridge + Remote Control + SSH-backed Session**:本地 REPL 渲染 + 远端 SSH-backed Session 执行(`bridge/sessionRunner.ts`、`bridge/createSession.ts`、`bridge/replBridge.tsx`);通过 WebSocket/SDK 适配(`remote/SessionsWebSocket.ts`、`remote/RemoteSessionManager.ts`、`remote/sdkMessageAdapter.ts`)与权限桥接(`remote/remotePermissionBridge.ts`),并提供 `claude remote-control/rc/remote/sync/bridge` 子命令;`upstreamproxy/relay.ts` 进一步中继请求,支持*云端执行 + 本地 TUI*分离。LsmAgentEmergentWork 当前主要是本地单进程。
5. **Bun 编译期 feature flag + ABLATION_BASELINE + 极简 fast-path**:在入口直接用 `process.env[k] ??= '1'` 强制注入消融开关(`src/entrypoints/cli.tsx:21-25`),便于 A/B 实验与 harness-science 评估;同时所有子命令都做了动态 import fast-path,显著降低冷启动。配合 React Compiler 已默认开启(见 `components/App.tsx` 中 `react/compiler-runtime` 与 `_c(N)` 缓存块),将运行时性能优化前置到编译期。

---

## 7. 小结

Claude Code 是一个以 Bun 打包、单二进制 CLI + React/Ink TUI 为外壳,围绕 *Anthropic Claude 单一模型 + MCP 工具扩展 + Skill 注册 + 多 Agent Swarm/Coordinator + 多层级 Context 压缩 + Bridge 远程控制* 构建的成熟 Agent 运行时。其工程特征可概括为:

- **入口极简**:fast-path + 动态 import + 编译期 `feature()` DCE;
- **类型先行**:消息/会话/权限/工具契约集中在 `entrypoints/agentSdkTypes.ts` 与 `types/` 下,SDK 与 CLI 共享同一份契约;
- **多 Agent 完备**:子 Agent、Swarm/Coordinator、Teammate、Remote Agent、Shell Agent、Dream 全部覆盖;
- **Context 工程化**:microCompact / autoCompact / reactiveCompact / sessionMemoryCompact / extractMemories / MagicDocs 形成闭环;
- **MCP 一等公民**:`services/mcp/` 12,000+ 行,MCPConnectionManager + MCP 工具 + 官方注册表 + OAuth + XAA + SdkControlTransport + InProcessTransport + VSCode SDK 集成;
- **Skill 系统**:`skills/bundled/` 17 个内置技能 + `loadSkillsDir.ts` 通用加载器 + `mcpSkillBuilders.ts`;
- **多模式权限**:`default / auto / ask / yolo / plan` 五大模式,叠加 Bash/Edit/Write/Sed/Sandbox/AskUser/Fallback/Skill 等独立请求对话框;
- **远程化**:Bridge、Remote Control、SSH-backed Session、CCR 容器环境、后台进程;
- **生态**:100+ slash 命令、43+ 个工具、插件系统、Hook 系统、键位/快捷键系统、voice/buddy、autoUpdater、worktree+tmux、vim 模式等。
