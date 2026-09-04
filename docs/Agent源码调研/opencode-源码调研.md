# opencode 源码调研报告

> 调研对象:`/usr/local/LsmGitOpenSource/opencode`(Anomalyco/opencode,dev 分支)
> 调研日期:2026-09-04
> 调研目的:为 LsmAgentEmergentWork 提供设计参照

---

## 1. 项目元信息

| 维度 | 内容 |
| --- | --- |
| 定位 | 开源 AI 编码 Agent(CLI + TUI + Desktop + Server) |
| 语言 | TypeScript(全栈 `tsconfig.json` 统一配置,Bun 运行时) |
| 包管理 | Bun workspaces(`packages/*`、`packages/console/*`、`packages/stats/*`、`packages/sdk/js`、`packages/slack`),`turbo.json` 做编排,`sst.config.ts` 做部署 |
| 构建 | `bun run --cwd packages/opencode src/index.ts`(开发);通过 `script/build.ts` + esbuild 发布 |
| 入口 | `packages/opencode/src/index.ts`(`yargs` 子命令分发) |
| 测试 | `bun:test`,模块内置 `test/` 目录(如 `packages/tui/test/*`、`packages/opencode/test/*`) |
| Lint/Format | oxlint + Prettier(`semi:false`、`printWidth:120`) |
| 核心依赖 | `effect 4.0-beta`(`@effect/opentelemetry`、`@effect/platform-node`)、`drizzle-orm`、`zod 4`、`ai 6`(Vercel AI SDK)、`@modelcontextprotocol/sdk`、`@opentui/core|keymap|solid`、`solid-js`、`hono`、`@pierre/diffs`、`fuzzysort`、`ulid` |
| 模型对接 | 自研 `packages/llm` + 复用 `@ai-sdk/*` 适配层(Anthropic / OpenAI / Gemini / Bedrock / Vertex / OpenAI-Responses / OpenAI-Chat / OpenAI-Compatible / GitHub Copilot / OpenRouter / Azure / Cloudflare / xAI / GitLab Workflow 等) |
| README | `README.md`(EN)+ 19 种语言版本;另有 `CONTEXT.md` 32 KB、`AGENTS.md` 9 KB 描述 Agent 协议 |
| License | MIT |

## 2. 目录树(顶层 + 关键包)

```
opencode/
├── AGENTS.md                 # 多 Agent 协作规约
├── CONTEXT.md                # 项目上下文总览
├── install                   # Bash 安装入口
├── package.json              # 根工作区 + 依赖 catalog
├── turbo.json                # Turbo 任务编排
├── sst.config.ts             # SST 部署
├── github/  infra/  nix/  patches/  perf/  script/  specs/
└── packages/                 # 34 个子包,核心如下
    ├── opencode/             # CLI + Server + Runtime 主体(~ 18k 行 src/)
    ├── tui/                  # 终端 UI(Solid + OpenTUI)
    ├── llm/                  # 协议无关 LLM 客户端 + Provider 路由
    ├── core/                 # 跨包共享 schema / 数据库 / Effect 层 / v1 兼容层
    ├── sdk/  sdk-next/       # JS SDK,Server HTTP API 强类型
    ├── plugin/               # 插件 SDK 类型
    ├── protocol/             # 协议常量
    ├── schema/               # zod schema 集合
    ├── function/  containers/# 云函数/容器化执行
    ├── enterprise/  identity/# 企业版 & 鉴权
    ├── console/  stats/      # 控制台 Web + 统计服务(SolidStart)
    ├── app/  web/            # 用户端 Web(SolidStart)
    ├── desktop/              # Electron 桌面端
    └── slack/                # Slack 桥接
```

`packages/opencode/src/`(单体大仓内最大的运行时包)结构:

```
src/
├── index.ts                  # CLI 入口(yargs)
├── node.ts                   # Node 平台层
├── account/   auth/   ide/   installation/   project/   share/
├── acp/                     # Agent Client Protocol(IDE 接入)
├── agent/                   # Agent 注册表 + 默认 Agent + generate.txt
│   └── prompt/              # compaction/explore/summary/title
├── background/              # 后台 Job
├── bus/                     # 进程内 EventBus(Effect Bus)
├── cli/
│   ├── bootstrap.ts  cmd/  effect-cmd.ts  effect/  error.ts  heap.ts  logo.ts
│   ├── network.ts  ui.ts  upgrade.ts
│   ├── tui/                # CLI 侧 TUI 适配(RPC bridge + worker)
│   └── cmd/                # yargs 子命令(tui / run / serve / mcp / agent / ...)
├── command/  config/        # 用户斜杠命令 + 配置加载
├── control-plane/           # 多 Provider 路由
├── effect/                  # Effect 应用运行时桥接(InstanceState 等)
├── event-manifest.ts  event-v2-bridge.ts
├── env/  format/  id/  image/  lsp/  markdown.d.ts  audio.d.ts
├── mcp/                     # MCP 客户端(Stdio/SSE/StreamableHTTP)+ OAuth + Catalog
├── patch/  permission/      # patch.ts + 权限规则引擎
├── plugin/                  # 插件加载 + GitHub Copilot / OpenAI / Modal / TUI 插件
├── provider/                # Provider registry + 模型转换 + Auth
├── question/                # 用户问答工具
├── server/                  # Hono + Effect HttpApi + MDNS
│   ├── routes/   shared/   server.ts  mdns.ts  projectors.ts  ...
├── session/                 # Session/Message/Stream/Agent 主循环
│   ├── session.ts  prompt.ts  processor.ts  llm.ts  message-v2.ts
│   ├── compaction.ts  instruction.ts  overflow.ts  reminders.ts  retry.ts
│   ├── revert.ts  run-state.ts  status.ts  summary.ts  system.ts  todo.ts  tools.ts
│   ├── llm/                # ai-sdk.ts / native-request.ts / native-runtime.ts / request.ts
│   └── prompt/             # default.txt / anthropic.txt / gemini.txt / plan*.txt ...
├── skill/                   # Skill 发现 + 注册
├── snapshot/                # 文件级快照(回滚)
├── storage/                 # SQLite/Drizzle 持久化
├── sync/   tool/   util/   worktree/
└── sql.d.ts  temporary.ts
```

### 核心 / 辅助目录标注

| 类型 | 目录 | 职责 |
| --- | --- | --- |
| **核心** | `agent/` `session/` `tool/` `provider/` `llm/`(包) `mcp/` `skill/` `permission/` | Agent 主循环 / 会话状态机 / 工具注册 / 模型接入 / MCP / Skill / 权限 |
| **辅助** | `cli/cmd/` `cli/tui/` `cli/cmd/run/` `server/` `control-plane/` `command/` `config/` `plugin/` `effect/` `util/` `lsp/` `storage/` `snapshot/` `acp/` | 命令行界面 / 终端 UI / HTTP 服务 / 控制平面 / 命令解析 / 配置 / 插件 / Effect 运行时 / 工具函数 / LSP 集成 / 持久化 / 文件快照 / IDE 接入 |

## 3. 架构骨架

### 3.1 分层
- **`packages/core`**:`Effect Service` 接口、`drizzle` Schema、`zod` v1 schema、`@effect/platform-node` 平台层。其它包均依赖它但不被它依赖。
- **`packages/llm`**:协议无关的 LLM 抽象(`LLMRequest` / `LLMResponse` / `LLMEvent`)、`Message` / `ToolDefinition` / `GenerationOptions`、`provider-error.ts`、`cache-policy.ts`、`route/`(client/executor/protocol/framing/transport)+ `protocols/`(Anthropic-Messages / Bedrock-Converse / Gemini / OpenAI-Responses / OpenAI-Chat / OpenAI-Compatible / shared)。
- **`packages/opencode`**:运行时主体 —— `Session → Processor → LLM` 三段式,通过 Effect `Service`/`LayerNode` 注入。
- **`packages/tui`**:Solid + OpenTUI 渲染,经 RPC (`cli/tui/worker.ts`) 与 Server 通信;`app.tsx` 装配 Provider 树(`ProjectProvider` / `ThemeProvider` / `RouteProvider` / `SDKProvider` / `SyncProvider` / `PermissionProvider` / `DialogProvider` 等)。
- **`packages/sdk`**:`createOpencodeClient`,消费 Server 暴露的 Effect HttpApi。

### 3.2 主循环位置
- **Agent**:`packages/opencode/src/agent/agent.ts`(453 行)`Service` 提供 `get/list/defaultInfo/generate`,并以 Schema(`Info`)声明 Agent 元数据(name/mode/native/permission/prompt/options/model/variant/steps)。默认注册 `build / plan / general / explore / compaction / title / summary`,用户可在 `cfg.agent` 扩展。
- **会话主循环**:`packages/opencode/src/session/prompt.ts`(1631 行)+ `processor.ts`(732 行)+ `llm.ts`(404 行)+ `compaction.ts`(608 行)+ `instruction.ts`(237 行)。
  - `SessionPrompt.prompt()` → `SessionProcessor.create()` → `LLM.stream()`(本质是 Vercel AI SDK `streamText` / `wrapLanguageModel`)→ 解析 `LLMEvent` → `updateToolCall`/`completeToolCall` → 触发下一轮。
  - `instruction.ts` 是核心"上下文装配器",按 Agent / 模型 / 工具 / Skill / Plan / 系统模板拼装最终 prompt。
  - `compaction.ts`:上下文超限时基于 `PRUNE_MINIMUM(20k)` / `PRUNE_PROTECT(40k)` 自动摘要;`overflow.ts` 判断溢出。
  - `doom_loop` 检测在 `processor.ts` 中(`DOOM_LOOP_THRESHOLD=3`)。

### 3.3 消息模型
- `MessageV2`(737 行)+ `SessionV1.WithParts`(来自 core 的 v1 兼容层),由 `User / Assistant / ToolPart / TextPart / ReasoningPart / FilePart / CompactionPart / SubtaskPart / ErrorInfo` 构成。
- SQLite + Drizzle 存储(`SessionTable / MessageTable / PartTable`);`cursor` 提供 base64url 编码的游标分页。
- `revert.ts`(136 行):基于 `Snapshot` 文件级快照回滚会话;`summary.ts`(160 行):增量 diff 摘要。

### 3.4 LLM 接入(Provider/LlmClient)
- `packages/opencode/src/provider/provider.ts`(2068 行):**Provider Registry**,声明 Anthropic / OpenAI / Google / Bedrock / Vertex / GitHub Copilot / OpenRouter / Azure / Cloudflare / xAI / Cerebras / DigitalOcean / Snowflake / 自定义 OpenAI-Compatible 等;`Auth` 适配每个 Provider 的 OAuth/API Key,`providerOptions` 由 `transform.ts`(1890 行)注入各家专属参数。
- `packages/llm` 是独立的协议路由库,可被任何上层消费,不绑定 opencode。
- **流式响应**:`llm.ts` 返回 `Effect.Stream<LLMEvent>`,内部通过 `streamText` 拉流,经 `EventV2Bridge` 翻译为对外的 `MessageUpdated / PartDelta / PartUpdated / PartRemoved` 事件,通过 `bus/` + Server SSE 推送给前端与 TUI。

### 3.5 TUI / REPL
- 入口 `cli/cmd/tui.ts`(309 行)启动 `worker.ts`(80 行,内嵌 RPC),Worker 调用 `Server.listen()` 并桥接 fetch + 全局事件;`tui/index.tsx` 与 `app.tsx` 渲染。
- 直跑模式 `cli/cmd/run.ts`(1016 行):流式输出,支持 `--continue` / `--session` / `--fork` / `--command` / `--format json`,可直接连在进程内 Server。
- 路由:`tui/routes/home.tsx`、`routes/session/index.tsx`(主会话页 + dialog 子页 `dialog-message / dialog-timeline / dialog-fork-from-timeline / dialog-subagent / permission / question / sidebar / subagent-footer`)。
- 上下文 Provider:`ProjectProvider / ThemeProvider / SDKProvider / SyncProvider / DataProvider / LocationProvider / LocalProvider / KVProvider / PermissionProvider / ArgsProvider / RouteProvider / EditorContextProvider / ToastProvider / DialogProvider / PromptRefProvider / TuiConfigProvider`。
- 输入/快捷键:`keymap.tsx`(注册 OpenCode keymap)、`commands/OPENCODE_BASE_MODE`、命令面板 `CommandPaletteDialog`,以及 `prompt/frecency` 频次感知补全。

## 4. 核心特性

| 能力 | 实现 / 证据 |
| --- | --- |
| 多 Agent | ✅ 默认 `build/plan/general/explore + 隐藏 compaction/title/summary`,用户可在 `cfg.agent` 扩展;通过 `PermissionV1.Ruleset` + `subagent-permissions.ts` 派生子 Agent 权限 |
| 任务分类 | ✅ `Agent.generate()` 用 `streamObject` 让 LLM 基于描述生成新 Agent(系统 prompt `generate.txt`) |
| 任务拆解 / Subagent | ✅ `tool/task.ts`(371 行)+ `tool/subagent`(`background` 模式可异步)、`subagent_depth` 配置控制嵌套深度、`task_id` 可恢复先前会话 |
| 质检 / Doom Loop | ✅ `processor.ts` 的 `DOOM_LOOP_THRESHOLD=3` + `permission.doom_loop: ask` 询问用户;`Compaction` 自动收敛长上下文 |
| MCP | ✅ `mcp/`(`Stdio` / `SSE` / `StreamableHTTP` 三种 transport + `oauth-provider.ts` + `oauth-callback.ts` + `catalog.ts` 公共 MCP 注册表 + `browser.ts` 浏览器授权) |
| Skill | ✅ `skill/index.ts` + `discovery.ts`:扫描 `**/SKILL.md` 支持 `.claude/skills`、`opencode/skills`、`.opencode/skills`、`AGENTS.md` 等外部目录;内嵌 `customize-opencode` 自描述 Skill |
| Session | ✅ 多 Session / Fork / Continue / Revert / Snapshot,`session.ts` 1016 行;游标分页 / 子会话(`parentID`) |
| 多 Provider | ✅ 14+ Provider,每个 Provider 含独立 `Auth` + `transform.ts` 中的专属参数映射,支持 `OPENCODE` 自有模型聚合 |
| 流式响应 | ✅ `LLM.stream` 返回 Effect Stream,经 `EventV2Bridge` 广播 `PartDelta` 等事件,Server 通过 SSE + WebSocket(`WebSocketTracker`)推送 |
| 其他亮点 | Plan 模式(`plan.ts` + `plan-enter/exit.txt` 自动切换)、LSP 集成(`lsp/lsp.ts`)、MDNS 服务发现、ACP(IDE 桥接)、桌面应用(Electron)、Cloudflare/Azure/Slack 桥接、OpenTelemetry、Heap Snapshot、Heap start、Plugin 系统、`@pierre/diffs` 渲染 diff、`apply_patch` 工具、`code-mode` 沙箱 |

## 5. 关键文件清单(按职责,行数基于 `wc -l`)

### Agent / Prompt / 主循环
- `packages/opencode/src/agent/agent.ts` —— 453 行,Agent 注册表 + `generate`
- `packages/opencode/src/agent/subagent-permissions.ts` —— 27 行,子 Agent 权限派生
- `packages/opencode/src/agent/prompt/{compaction,summary,title,explore}.txt` —— 系统 prompt
- `packages/opencode/src/agent/generate.txt` —— Agent 生成器系统 prompt
- `packages/opencode/src/session/prompt.ts` —— 1631 行,`SessionPrompt.Service`,主入口
- `packages/opencode/src/session/processor.ts` —— 732 行,流处理 + Doom Loop
- `packages/opencode/src/session/llm.ts` —— 404 行,LLM stream 封装
- `packages/opencode/src/session/llm/{ai-sdk,native-request,native-runtime,request}.ts` —— 291/196/195/226 行,LLM 适配
- `packages/opencode/src/session/session.ts` —— 1016 行,Session CRUD + Revert
- `packages/opencode/src/session/message-v2.ts` —— 737 行,消息/Part 模型
- `packages/opencode/src/session/compaction.ts` —— 608 行,上下文压缩
- `packages/opencode/src/session/instruction.ts` —— 237 行,上下文装配器
- `packages/opencode/src/session/{system,summary,reminders,todo,tools,revert,run-state,status,overflow,retry}.ts` —— 系统/摘要/提醒/TODO/回滚

### Provider / LLM 抽象
- `packages/opencode/src/provider/provider.ts` —— 2068 行,Provider Registry
- `packages/opencode/src/provider/transform.ts` —— 1890 行,Provider 适配参数
- `packages/opencode/src/provider/auth.ts` —— 229 行,Provider 认证
- `packages/llm/src/llm.ts` —— 186 行,协议无关 LLM 入口
- `packages/llm/src/route/client.ts` —— 436 行,LLM 路由客户端
- `packages/llm/src/route/executor.ts` —— 385 行,请求执行
- `packages/llm/src/{tool,tool-runtime,cache-policy,provider-error,provider}.ts` —— 工具/缓存/错误

### 工具 / MCP / Skill / 权限
- `packages/opencode/src/tool/registry.ts` —— 455 行,工具注册中心
- `packages/opencode/src/tool/{edit,read,write,grep,glob,shell,apply_patch,code-mode,lsp,plan,task,skill,todo,question,webfetch,websearch,mcp-websearch,truncate}.ts` —— 工具实现
- `packages/opencode/src/mcp/index.ts` —— MCP 客户端主体
- `packages/opencode/src/mcp/{auth,oauth-provider,oauth-callback,catalog,browser}.ts` —— OAuth/Catalog/浏览器
- `packages/opencode/src/skill/{index,discovery}.ts` —— Skill 注册 + 扫描
- `packages/opencode/src/permission/index.ts` —— 权限引擎

### CLI / Server / TUI
- `packages/opencode/src/index.ts` —— 142 行,CLI 入口
- `packages/opencode/src/cli/cmd/{tui,run,serve,mcp,agent,github,debug,generate,upgrade,providers,export,import,attach,web,pr,session,db,plug,acp,uninstall,stats,account,prompt-display,models}.ts`
- `packages/opencode/src/cli/cmd/run.ts` —— 1016 行,直跑模式
- `packages/opencode/src/cli/cmd/tui.ts` —— 309 行,TUI 入口
- `packages/opencode/src/cli/tui/worker.ts` —— 80 行,RPC worker
- `packages/opencode/src/server/server.ts` —— Hono + Effect HttpApi Server

## 6. 独特设计(相对 LsmAgentEmergentWork 的差异化)

1. **Effect + Layer DI 全栈化**。整个运行时基于 `effect` v4-beta,所有服务以 `Context.Service<Service, Interface>()("@opencode/Xxx")` 声明、`LayerNode` 注入(`packages/core/effect/layer-node`)。`InstanceState` 解决"同进程多工作区(多 git 仓库)状态隔离",`LocationServiceMap` 把"Location → Service"显式映射。对比 LsmAgentEmergentWork 的轻量 DI,这种重型 Effect DI 提供了更强的可观测性(`@effect/opentelemetry`)和组合性,但学习曲线显著更高。
2. **协议无关 LLM 抽象层(`packages/llm`)**。`LLMRequest / LLMResponse / LLMEvent / ToolDefinition / GenerationOptions / Message` 是 Effect Schema 化的中转层,`protocols/` 下为每个 Provider 写"协议适配器"(Anthropic-Messages / OpenAI-Responses / OpenAI-Chat / Gemini / Bedrock-Converse / OpenAI-Compatible),`route/client.ts` 通过策略(`cache-policy.ts`、`provider-error.ts`)路由。LsmAgentEmergentWork 当前直接绑定 Vercel AI SDK,而 opencode 在其上多加一层"路由",使得多 Provider/缓存/重试/协议分支统一管理。
3. **基于文件快照 + 游标的 Session 模型**。`snapshot.ts` 在每次流前捕获工作区快照;`revert.ts` 可精确回滚到任意 Part;`message-v2.ts` 用 base64url 游标(`encode/decode`)分页;`compaction.ts` 引入 `PRUNE_MINIMUM/PRUNE_PROTECT/MIN_PRESERVE_RECENT_TOKENS/MAX_PRESERVE_RECENT_TOKENS` 等多档 token 阈值,自动在 user/assistant 边界做"对话折断+摘要+保护近期"。比单纯的"上下文截断"更细致。
4. **Skill 双轨制 + Project Hint**。`skill/index.ts` 同时扫描 `.claude/skills/**/SKILL.md`、`opencode/skills/**/SKILL.md`、`.agents/skills/**/SKILL.md`、项目内 `**/SKILL.md`,还内置一个 `customize-opencode` Skill 用于教 LLM 写 opencode 自己的配置。`tool/skill.ts` 让 LLM 按需加载;`discovery.ts` 用 `Glob` 缓存。这种"约定多目录 + 自动发现 + 模型端按需取用"是 LsmAgentEmergentWork 可借鉴的体验设计。另外 `Agent.generate()` 用 `streamObject` 让 LLM 反向产出新 Agent,实现了"运行时动态增加 Agent"。
5. **TUI/Server 拆进程 RPC 桥**。`cli/cmd/tui.ts` 启动 `worker.ts`(子进程);Worker 通过 `Rpc` 把 fetch 与 `global.event` 转发给 TUI;TUI 因此是"无状态视图",Server 可以独立升级/重启,而 TUI 自动重连。`cli/cmd/run.ts` 又提供"直跑模式",既能本进程内嵌 Server(`--mini`),也能 `--attach` 远端 Server;`acp/` 还能把 Agent 暴露给 IDE(Agent Client Protocol)。这是 LsmAgentEmergentWork 当前未覆盖的"前端↔Agent 进程边界"维度。
6. **Plan Mode + 子 Agent 嵌套权限**。`plan.ts` + `plan-enter/exit.txt` 实现"Plan ↔ Build"无痕切换;`agent/subagent-permissions.ts` 在子 Agent 启动时根据父 Session 权限 + 子 Agent 静态权限动态派生 `Ruleset`,并强制 `todowrite/task` 对子 Agent `deny`,再叠加 `cfg.experimental?.primary_tools` 显式 deny。`subagent_depth` 控制最大嵌套层数,`task_id` 支持"恢复先前任务"——是少见的"Agent 即会话"可恢复式实现。
wc -l /usr/local/LsmGitOpenSource/LsmAgentEmergentWork/docs/Agent源码调研/opencode-源码调研.md
