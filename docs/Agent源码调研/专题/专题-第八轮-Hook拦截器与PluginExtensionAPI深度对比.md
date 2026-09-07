# 第八轮 · Hook 拦截器与 Plugin / Extension API 深度对比

> **本专题定位**：第六轮已经写过 27 种 Hook 事件 / `LifecycleHooks + ToolMiddleware` / `waterfall` 双模式 / Cordis 6 大原语，
> 第三轮已经写过 5 仓插件生态（distribute / install / marketplace）。**本轮专注于第六轮与第三轮均未深入覆盖的层面**：
> **Extension API 表面 / 插件生命周期 / 沙箱 / 分发与签名**，并把 Hook 当作 Extension API 的一个**触发时机入口**，而不是孤立的事件系统。
>
> **覆盖工程**：atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / undici
> **覆盖维度**：8 维（注册模型 / 触发时机 / 决策 / 失败 / API 表面 / 沙箱 / 生命周期 / 分发）
>
> **所有代码引用均附绝对路径 + 行号**，由 Explore Subagent + 直接源码阅读交叉验证。
> **完成日期**：2026-09-07

---

## 目录

1. [摘要与 TL;DR](#1-摘要与-tldr)
2. [背景：为什么需要 Hook + Extension 两层抽象](#2-背景为什么需要-hook--extension-两层抽象)
3. [每个工程的实际实现](#3-每个工程的实际实现)
   - 3.1 [claudecode：27 种 Hook + settings.json 范式（行业标杆）](#31-claudecode27-种-hook--settingsjson-范式行业标杆)
   - 3.2 [openclaw：162 扩展 + SDK 契约（生态最大）](#32-openclaw162-扩展--sdk-契约生态最大)
   - 3.3 [opencode：25+ Hook + Effect 类型化（最严谨）](#33-opencode25-hook--effect-类型化最严谨)
   - 3.4 [pi：40+ Event + 完整 SDK（最小但最精致）](#34-pi40-event--完整-sdk最小但最精致)
   - 3.5 [deepseek-harness：Cordis 5 态 Fiber + 27 事件桥](#35-deepseek-harnesscordis-5-态-fiber--27-事件桥)
   - 3.6 [atomcode：CC 兼容 manifest + hook_trust 哈希（治理型）](#36-atomcodec-兼容-manifest--hook_trust-哈希治理型)
   - 3.7 [undici：8 拦截器 + `compose()` 链接式（非 Agent）](#37-undici8-拦截器--compose-链接式非-agent)
4. [横向对比大表：7 工程 × 8 维度](#4-横向对比大表7-工程--8-维度)
5. [共性模式（强调 claudecode 27 Hook 是行业标杆）](#5-共性模式强调-claudecode-27-hook-是行业标杆)
6. [对 laew 的 P0/P1/P2 路线图（Rust crate 选型）](#6-对-laew-的-p0p1p2-路线图rust-crate-选型)
7. [附录：关键代码路径速查表](#7-附录关键代码路径速查表)

---

## 1. 摘要与 TL;DR

### 1.1 核心结论

1. **Hook 与 Extension 是两层而非一层**：Hook 拦截**已发生的事**（"在 tool_call 之前我可以拒绝吗？"），
   Extension 拥有**未来会用的能力**（"我能注册一个新工具/新 Provider/新命令吗？"）。7 个工程都遵守这条线。
2. **claudecode 的 27 种 Hook + 4 种执行器（command / http / prompt / agent）+ settings.json 范式**是事实上的行业标杆。
   其它工程要么完全照搬（deepseek-harness 的 `hooks-claude-code` 适配器），要么重新发明一个简化版。
3. **Hook 配置 vs Extension 代码**：Hook 配置是声明式（YAML/JSON）；Extension 是命令式（导出 `default function`）。
   7 个工程都支持"声明式 hook + 命令式 extension 入口"双轨。`opencode`、`openclaw`、`atomcode` 还支持从 NPM/git 直接安装二进制的 plugin。
4. **沙箱**：除 claudecode 早期版本曾经尝试过容器沙箱外，**7 个工程里没有任何一个对插件做进程级沙箱**。
   沙箱始终是给**外部代码执行**（Bash / Shell / 子进程）准备的，不是给插件进程本身准备的。
5. **签名**：除 atomcode 的 hook_trust 哈希签名外，**没有工程做插件级别的代码签名**。
   SHA-256 完整性校验是标配（openclaw ClawHub），但密钥签名是缺位的。
6. **Hook 失败语义**：deepseek-harness 的 Cordis 是 fail-closed（plugin 抛错 → service 卸载）；
   claudecode 是 fail-open（hook exit ≠ 0 → 静默忽略 → 让工具继续执行）。这是两类哲学，需要产品选型。

### 1.2 关键数据点

| 指标 | claudecode | openclaw | opencode | pi | deepseek-harness | atomcode | undici |
|------|-----------|----------|----------|----|------------------|----------|--------|
| Hook 事件数 | **27** | 8（typed 43）| 25+ | **40+** | 27（适配 CC） | 8（CC 兼容） | 11（HTTP） |
| Execution 类型 | 4 (command/http/prompt/agent) | 1 (script) | 1 (function) | 1 (function) | 1 (subprocess) | 1 (subprocess) | 1 (handler) |
| 注册模型 | settings.json + extension file | openclaw.plugin.json + register(api) | config plugins + register(api) | .pi/extensions/*.ts | Cordis plugin class | marketplace.json + plugin.json | `agent.compose(interceptor)` |
| 扩展代码数 | ~5（CLI 内部） | **153**（实际 162 含杂项） | 12 (内置) + 50+ (NPM) | 4 + 用户 | ~50 (Cordis 仓 + 内部) | marketplace 驱动 | 8 (内置拦截器) |
| Sandbox 边界 | ❌ | ❌（plugin） + ✅（sandbox.ts）| ❌ | ❌ | ❌ | ❌ | ❌（拦截器链） |
| 签名/校验 | ❌ | SHA-256 (ClawHub) | ❌ | ❌ | ❌ | **SHA-256 (hook_trust)** | ❌ |
| 进程隔离 | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

---

## 2. 背景：为什么需要 Hook + Extension 两层抽象

### 2.1 一个具体例子

考虑用户写 `"refactor this file"`：

| 时机 | Hook / Extension API | 谁触发 |
|------|----------------------|--------|
| LLM 请求发送前 | Hook：`before_provider_request` | **Extension（拦截器）** |
| LLM 决定调 `read` 工具后 | Hook：`tool_call` (pre) | **Hook（声明式）** |
| `read` 工具实现开始执行 | Extension：`Tool.execute` | **Extension（执行）** |
| `read` 结果返回后 | Hook：`tool_result` (post) | **Hook（声明式）** |
| 把结果加进 system prompt | Extension：`before_agent_start` | **Extension（拦截器）** |
| 用户触发 `/help` | Extension：`registerCommand` | **Extension（注册）** |
| 启动时加载第三方 plugin | Lifecycle：`load → init → register` | **Extension（生命周期）** |

Hook 和 Extension 解决了**两类正交需求**：
- **Hook** = "**我需要在已发生的事上挂个副作用**" → **声明式、事件驱动**
- **Extension** = "**我需要给 host 加一个新能力**" → **命令式、能力注册**

把它们混淆会出大问题：如果用 Hook 注册工具，工具名字会跟 event 名抢；如果用 Extension 拦截已有调用，需要修改 host 内部 API。

### 2.2 7 个工程如何取舍

- **claudecode**：Hook 丰富（27 种），Extension 仅做"加载一段 JS 副作用"（不做新工具注册）。**Hook 为主**。
- **openclaw**：Hook（typed 43 个） + Extension（150+ provider/channel 注册）。**两者并重**。
- **opencode**：Hook（25+） + Extension（auth / provider / chat.headers / chat.params / tool.before/after）。**两者并重**。
- **pi**：Hook（40+） + Extension（registerTool / registerCommand / registerShortcut / registerProvider / registerMessageRenderer）。**两者并重**。
- **deepseek-harness**：Cordis 内部 Hook（emitter 模式，5+ 类型）+ Extension（class-based plugin）。Cordis 是**一切皆插件**模型。
- **atomcode**：Hook（CC 兼容 8 种）+ Extension（marketplace-driven，仅做 skills/commands/hooks 注入）。**Hook 为辅，主打分发**。
- **undici**：Hook（11 HTTP 生命周期回调）+ Interceptor（compose 链）。**Hook 拦截器即核心抽象**。

---

## 3. 每个工程的实际实现

### 3.1 claudecode：27 种 Hook + settings.json 范式（行业标杆）

> 第六轮已详细列出 27 种 Hook 事件，本节专注于**Extension API / 加载模型 / 失败语义**这些第六轮未深入的角度。

#### 3.1.1 关键类型与 schema 文件

| 文件 | 行数 | 角色 |
|------|------|------|
| `/usr/local/LsmGitOpenSource/claudecode/src/types/plugin.ts` | 1-364 | 所有公开 Plugin TypeScript 类型 |
| `/usr/local/LsmGitOpenSource/claudecode/src/utils/plugins/schemas.ts` | 1-1682 | 所有 plugin / hook / marketplace Zod schema |
| `/usr/local/LsmGitOpenSource/claudecode/src/schemas/hooks.ts` | 1-223 | 用户面 hook schema（4 种类型 + matcher） |
| `/usr/local/LsmGitOpenSource/claudecode/src/utils/settings/types.ts` | 1-1148 | Settings 总 schema（含 `enabledPlugins`, `extraKnownMarketplaces`, `strictKnownMarketplaces`, `blockedMarketplaces` 等） |
| `/usr/local/LsmGitOpenSource/claudecode/src/entrypoints/sdk/coreTypes.ts` | 25-53 | 27 个 `HOOK_EVENTS` 事件名唯一定义源 |

**27 个事件**（`coreTypes.ts:25-53`）：`PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `Notification`, `UserPromptSubmit`, `SessionStart`, `SessionEnd`, `Stop`, `StopFailure`, `SubagentStart`, `SubagentStop`, `PreCompact`, `PostCompact`, `PermissionRequest`, `PermissionDenied`, `Setup`, `TeammateIdle`, `TaskCreated`, `TaskCompleted`, `Elicitation`, `ElicitationResult`, `ConfigChange`, `WorktreeCreate`, `WorktreeRemove`, `InstructionsLoaded`, `CwdChanged`, `FileChanged`。

#### 3.1.2 Hook 配置范式（`settings.json`）

claudecode 的 Hook 完全是声明式的，存放在 `~/.claude/settings.json` 或项目根的 `.claude/settings.json`。**结构是事件名 → matcher → 执行器**。

**Hook schema**（`schemas/hooks.ts:211-213`）：
```ts
HooksSchema = partialRecord(enum(HOOK_EVENTS), array(HookMatcherSchema))
HookMatcherSchema = { matcher?: string, hooks: HookCommand[] }
HookCommandSchema = discriminatedUnion('type', [
  BashCommandHookSchema,    // command 类型
  PromptHookSchema,         // prompt 类型
  AgentHookSchema,          // agent 类型
  HttpHookSchema            // http 类型
])
```

**4 种执行器（discriminated union）**：

| 类型 | schema 位置 | 关键字段 |
|------|-------------|---------|
| `command` | `schemas/hooks.ts:32-65` | `command`, `if?`, `shell?`, `timeout?`, `statusMessage?`, `once?`, `async?`, `asyncRewake?` |
| `prompt` | `schemas/hooks.ts:67-95` | `prompt` (含 `$ARGUMENTS`), `if?`, `timeout?`, `model?`, `statusMessage?`, `once?` |
| `http` | `schemas/hooks.ts:97-126` | `url`, `if?`, `timeout?`, `headers?` (env 插值), `allowedEnvVars?` |
| `agent` | `schemas/hooks.ts:128-163` | `prompt` (含 `$ARGUMENTS`), `if?`, `timeout?` (默认 60s), `model?` (默认 Haiku), `statusMessage?`, `once?` |

**matcher 4 种语义**（`utils/hooks.ts:1346-1380`）：
1. **精确匹配**（字符串）：`"Write"`
2. **管道分隔**：`"Write|Edit"`（每个 alternative 通过 `normalizeLegacyToolName` 映射）
3. **正则**：不匹配 `/^[a-zA-Z0-9_|]+$/` → `new RegExp(matcher)`
4. **`*` 或空**：匹配所有

**这套设计的好处是**完全无状态、可序列化；**坏处是**不能写循环逻辑（需要扩展就用 Plugin manifest）。

#### 3.1.3 Plugin manifest 表面（声明式插件）

`schemas.ts:884-898` 的 `PluginManifestSchema` 合并 11 个可选子 schema：

| 字段 | 行号 | 用途 |
|------|------|------|
| `metadata` | 274-320 | name/version/author/repository/license/keywords/dependencies |
| `hooks` | 328-340 | 内联 hook 配置 |
| `commands` | 429-452 | 斜杠命令（markdown 或对象映射） |
| `agents` | 460-476 | sub-agent markdown |
| `skills` | 484-499 | SKILL.md 目录 |
| `outputStyles` | 507-524 | 输出样式 |
| `channels` | 670-703 | Telegram/Slack/Discord 风格 channel |
| `mcpServers` | 543-572 | MCP servers（`.mcp.json` 或内联，含 MCPB bundles） |
| `lspServers` | 708-820 | LSP servers（`.lsp.json` 或内联） |
| `userConfig` | 587-621 | 强类型 `string/number/boolean/directory/file` 用户配置 |
| `settings` | 857-867 | 注入 settings cascade（白名单过滤） |

**关键**：`pluginLoader.ts:1884-2090, 2191-…` 通过 `readFile/readdir` **静态枚举**所有插件文件。**没有 `require()` / `import()` / `eval` 任何 plugin JS**。Plugin 携带的可执行代码只能通过 subprocess（command hook / MCP server / LSP server / status line）或 HTTP（http hook）执行。

#### 3.1.4 失败语义（分层 fail-open / fail-closed）

**默认超时**（`utils/hooks.ts`）：
- `TOOL_HOOK_EXECUTION_TIMEOUT_MS = 10 * 60 * 1000`（10 分钟）行 166
- `SESSION_END_HOOK_TIMEOUT_MS_DEFAULT = 1500`（1.5 秒）行 175
- agent hook 默认 60 秒（schema `schemas/hooks.ts:148`）

**Exit code 协议**（`utils/hooks.ts:2617, 2648, 3334-3353`）：

| Exit code | outcome | 行为 |
|-----------|---------|------|
| `0` | success | 正常 |
| `2` | **blocking** | 阻止工具调用；Stop hook 阻止会话结束 |
| 1 / 其他非零 | non_blocking_error | 静默继续 |
| stderr JSON | 解析为 `SyncHookJSONOutput` | 决策/修改 |

**Fail-open vs Fail-closed 分层**：

| 失败场景 | 语义 | 行号 |
|---------|------|------|
| JSON 解析失败 | **fail-closed** | `utils/hooks.ts:1238-1242`（"prompt JSON can never leak through"） |
| Process spawn 错误 (ENOENT/EPIPE/ABORT_ERR) | **fail-open**（status: 1, non-blocking） | `utils/hooks.ts:1283-1318` |
| Plugin 目录缺失 | **fail-closed**（throw） | `utils/hooks.ts:825-836` |
| 未信任工作区 | **fail-closed**（skip all hooks） | `shouldSkipHookDueToTrust` `utils/hooks.ts:286-296` |
| HTTP hook URL 不在 allowlist | **fail-closed** | `types.ts:480-489` |
| HTTP hook env var 不在 allowlist | 中和（替换为空字符串）| `schemas/hooks.ts:114-117` |
| Plugin 加载失败 | 单插件失败不影响其它 | `useManagePlugins.ts:77-109, 223-265` |

#### 3.1.5 生命周期：install → enable → reconcile → hot-reload

| 操作 | 入口 | 行号 |
|------|------|------|
| Install | `installPluginOp` / `installResolvedPlugin` | `utils/plugins/pluginInstallationHelpers.ts:348, 506` |
| Enable/disable toggle | `enabledPlugins` key | `utils/settings/types.ts:559-567` + `builtinPlugins.ts:65-99` |
| Marketplace reconcile | `reconcileMarketplaces` | `utils/plugins/reconciler.ts:114-234`（additive + idempotent） |
| Hot-reload | `setupPluginHookHotReload` | `loadPluginHooks.ts:255-287` |
| Auto-update | `pluginAutoupdate` | `utils/plugins/pluginAutoupdate.ts` |

**多 scope 安装**（`schemas.ts:1549-1567`）：`scope: 'managed' | 'user' | 'project' | 'local'`，同一插件可在不同 scope 用不同版本。

#### 3.1.6 marketplace / 分发（**8 种 source + 6 种 plugin source**）

**Marketplace 8 种 source**（`schemas.ts:906-1044` discriminated union）：

| Source | 行号 | 用途 |
|--------|------|------|
| `url` | 908-915 | 直链 `marketplace.json` |
| `github` | 916-940 | `owner/repo` + 可选 `ref/path/sparsePaths` |
| `git` | 941-972 | 完整 git URL（不支持 `.endsWith('.git')` 以兼容 Azure DevOps/CodeCommit） |
| `npm` | 973-978 | npm 包含 `marketplace.json` |
| `file` | 979-982 | 本地文件路径 |
| `directory` | 983-988 | 本地目录含 `.claude-plugin/marketplace.json` |
| `hostPattern` | 989-999 | `strictKnownMarketplaces` 用的 hostname 正则 allowlist |
| `pathPattern` | 1000-1011 | `strictKnownMarketplaces` 用的路径正则 allowlist |
| `settings` | 1012-1043 | 内联声明 marketplace |

**Plugin 6 种 source**（`schemas.ts:1062-1161`）：

| Source | 行号 | 用途 |
|--------|------|------|
| 相对路径 | 1063-1066 | marketplace 本地路径 |
| `npm` | 1067-1087 | npm 包 + 版本 + registry |
| `pip` | 1088-1106 | Python 包 |
| `url` | 1107-1119 | git URL + ref/sha |
| `github` | 1120-1130 | owner/repo + ref/sha |
| `git-subdir` | 1131-1157 | monorepo partial clone |

**官方 marketplace**（`utils/plugins/officialMarketplace.ts:15-25`）：
```ts
OFFICIAL_MARKETPLACE_SOURCE = { source: 'github', repo: 'anthropics/claude-plugins-official' }
```
也支持 GCS-hosted zip（`officialMarketplaceGcs.ts`）+ 启动 check（`officialMarketplaceStartupCheck.ts`）。

**仿冒防御**（`schemas.ts:82-101`）：
- `BLOCKED_OFFICIAL_NAME_PATTERN` 阻止 `official-anthropic` / `claude-official` 等保留名
- 非 ASCII 字符（homograph 攻击）拒绝
- `NpmPackageNameSchema`（`schemas.ts:837-850`）拒绝 `..` `//` 非合规名

**没有签名**——信任靠 marketplace 名称 + workspace trust + 政策白名单。

---

### 3.2 openclaw：162 扩展 + SDK 契约（生态最大）

#### 3.2.1 扩展数量

`/usr/local/LsmGitOpenSource/openclaw/extensions/` 目录包含 **153 个真实插件包** + 9 个杂项文件（2 markdown / 1 tsconfig / 2 测试 .ts / 1 boundary canary / 1 icon sources doc）。

> 实际数量由 Explore Subagent 校验（去掉非插件目录后是 153）。

#### 3.2.2 边界规则（`extensions/AGENTS.md` + `extensions/CLAUDE.md`）

**这是 7 个工程里最严格的边界声明**。两个文件完全相同，关键规则（`extensions/AGENTS.md` lines 27-47）：

1. **导入白名单**："Extension production code should import from `openclaw/plugin-sdk/*`"
2. **禁入区**：`src/**`、`src/channels/**`、`src/plugin-sdk-internal/**`、其它扩展的 `src/**`
3. **manifest 必备**：`openclaw.plugin.json` + `package.json#openclaw` 块
4. **依赖隔离**：插件依赖声明在插件自己的 `package.json`，runtime 不自动安装

#### 3.2.3 扩展入口契约

`/usr/local/LsmGitOpenSource/openclaw/src/plugin-sdk/plugin-entry.ts` 定义两个工厂：

| 工厂 | 文件:行号 | 用途 |
|------|----------|------|
| `definePluginEntry(opts)` | plugin-entry.ts:234-259 | 普通扩展 |
| `defineChannelPluginEntry(...)` | core.ts:553-599 | channel/messaging 扩展 |
| `defineSingleProviderPluginEntry` | provider-entry.ts:383-577 | 单供应商扩展（自动注册 provider + model catalog） |

最小示例（`extensions/anthropic/index.ts`）：
```ts
import { definePluginEntry } from "openclaw/plugin-sdk/plugin-entry";
import { registerAnthropicPlugin } from "./register.runtime.js";
export default definePluginEntry({
  id: "anthropic",
  name: "Anthropic",
  description: "...",
  register(api) { return registerAnthropicPlugin(api); },
});
```

#### 3.2.4 `OpenClawPluginApi` 表面（**50+ `register*` 方法**）

`/usr/local/LsmGitOpenSource/openclaw/src/plugins/plugin-api.types.ts` 是 `api` 参数的完整类型。重要 `register*` 方法：

| 方法 | 行号 | 用途 |
|------|------|------|
| `registerTool` | 209-212 | 注册 agent 工具 |
| `registerHook` | 213-217 | 注册内部风格 hook（不推荐，新代码用 `api.on()`） |
| `registerHttpRoute` | 218 | 注册 Gateway HTTP 路由 |
| `registerChannel` | 227 | 注册 channel plugin |
| `registerGatewayMethod` | 235-242 | RPC 方法（带 `OperatorScope`） |
| `registerService` | 265 | 长生命周期后台服务 |
| `registerCli` | 247-250 | CLI 子命令树 |
| `registerProvider` | 279 | LLM provider |
| `registerModelCatalogProvider` | 283 | 模型目录 |
| `registerEmbeddingProvider` | 285-287 | 嵌入 |
| `registerSpeechProvider` | 289-293 | TTS |
| `registerRealtimeTranscriptionProvider` | 295-299 | 实时 STT |
| `registerRealtimeVoiceProvider` | 301-305 | 实时语音 |
| `registerWebSearchProvider` | 319 | Web 搜索 |
| `registerWebFetchProvider` | 317 | Web 抓取 |
| `registerImageGenerationProvider` | 311 | 图像生成 |
| `registerVideoGenerationProvider` | 313 | 视频生成 |
| `registerMusicGenerationProvider` | 315 | 音乐生成 |
| `registerContextEngine` | 331 | 上下文引擎（独占） |
| `registerCompactionProvider` | 333-335 | 压缩算法 |
| `registerAgentHarness` | 337 | agent 执行器 |
| `registerMigrationProvider` | 275 | 数据迁移 |
| `registerMemoryCapability` | 460 | 记忆能力（独占） |
| `registerTrustedToolPolicy` | 368 | 工具策略 |
| `registerToolMetadata` | 374 | 工具元数据 |
| `registerSecurityAuditCollector` | 264 | 安全审计 |
| `registerControlUiDescriptor` | 379 | 控制 UI 贡献 |
| `registerCliBackend` | 269 | 本地 CLI 后端 |
| `registerWorkerProvider` | 281 | 云 worker 生命周期 |
| `on(...)`（typed hooks） | 471-475 | 43 种 typed hook |

**统计**：50+ 个 `register*` + 43 种 typed hook event。这是 7 个工程里**最大的扩展 API 表面**。

#### 3.2.5 生命周期（**最完整的 6 阶段**）

| 阶段 | 实现位置 |
|------|---------|
| 1. Discovery | `/usr/local/LsmGitOpenSource/openclaw/src/plugins/discovery.ts` |
| 2. Manifest snapshot | `plugin-metadata-snapshot.ts` (called via `gateway-startup-plugin-loader.ts:54`) |
| 3. Enablement | `resolveEffectivePluginActivationState` + `isPluginEnabledByDefaultForPlatform` |
| 4. Module load | `loadRuntimePluginCandidate` (`loader-runtime-candidate.ts`, loop at `loader-runtime-core.ts:230-243`) |
| 5. Register | 调插件 `register(api)`，仅当 `api.registrationMode === "full"` |
| 6. Activate | `activatePluginRegistry` (`loader-runtime-core.ts:289-294`) |

`registrationMode` 共 4 种（`plugin-registration.types.ts:425-431`）：

| mode | 触发 |
|------|------|
| `cli-metadata` | 只注册 CLI 元数据 |
| `tool-discovery` | 注册 CLI + 能力 |
| `discovery` | 注册 CLI + 能力，不激活 |
| `full` | 注册 CLI + 能力 + 完整 runtime |

#### 3.2.6 失败语义（**failurePhase 三段位 + 回滚**）

`PluginRecord.failurePhase`（`registry-types.ts:498`）区分：
- `"validation"` —— manifest 不通过
- `"load"` —— 加载插件失败
- `"register"` —— `register(api)` 抛错

**关键：失败时回滚已激活的插件**（`loader-runtime-core.ts:302-313`）：
```ts
// 错误路径：失败时调用 rollbackPluginGlobalSideEffects
// 把前面成功加载的插件回滚到加载前状态，不污染缓存
```

#### 3.2.7 沙箱：**in-process + plugin 拥有 root 信任**

`docs/plugins/architecture.md` lines 432-436（**强烈警告**）：

> "Native OpenClaw plugins run **in-process** with the Gateway. They are not sandboxed. A loaded native plugin has the same process-level trust boundary as core code. **A malicious native plugin is equivalent to arbitrary code execution inside the OpenClaw process.**"

**唯一的 host 层 gate**：`plugins.allow` 白名单（`validation-plugin-config.ts:99`、`recovery-policy.ts:5`、`auto-enable.shared.ts:373`）。

**沙箱是给 agent exec 用的**，不是给插件本身：`src/plugin-sdk/sandbox.ts` 暴露 `SandboxBackendFactory` / `SshSandboxSession` 等，仅在 `agent.run` 阶段启用。

#### 3.2.8 分发：ClawHub + 双 publish

- **ClawHub**（`/usr/local/LsmGitOpenSource/openclaw/src/plugins/clawhub.ts` 1546 行）= 自建 marketplace
- **完整性校验**：`normalizeClawHubSha256Integrity`（`clawhub.ts:13-17`）做 SHA-256 校验
- **没有代码签名**
- `package.json#openclaw.release.publishToNpm: true`（如 `extensions/brave/package.json:31`）支持双 publish 到 npm
- 版本约束：`install.minHostVersion`、`compat.pluginApi` 字段（在 host 加载时 enforce）

---

### 3.3 opencode：25+ Hook + Effect 类型化（最严谨）

> opencode 是 7 个工程里**唯一双轨插件 API** 的工程：v1（简单 promise）+ v2（Effect-based）。

#### 3.3.1 双轨插件 API（v1 promise + v2 Effect）

`/usr/local/LsmGitOpenSource/opencode/packages/plugin/` 包含 **42 文件**，关键路径：

| 路径 | 行数 | 角色 |
|------|------|------|
| `src/index.ts` | 335 | v1 Plugin / Hooks / PluginInput |
| `src/tool.ts` | 54 | v1 `tool()` 工厂 + `ToolDefinition` |
| `src/tui.ts` | 634 | TUI plugin API（`TuiPluginApi`、tui routes、keymap、slots、theme） |
| `src/shell.ts` | 136 | BunShell 类型 |
| `src/v2/effect/plugin.ts` | 16 | v2 Effect-based Plugin |
| `src/v2/effect/context.ts` | 22 | v2 `PluginContext`（7 domains：agent/aisdk/catalog/command/integration/plugin/reference/skill） |
| `src/v2/effect/registration.ts` | 15 | v2 `Hooks<Spec>` builder |

**v1 Plugin 类型**（`src/index.ts:74`）：
```ts
export type Plugin = (input: PluginInput, options?: PluginOptions) => Promise<Hooks>
```

**v1 Hooks 21 个方法**（`src/index.ts:222-335`，不是 25+，去除 `experimental.` 后的实际是 15 + 6 experimental）：

| # | 方法 | 行号 | 用途 |
|---|------|------|------|
| 1 | `dispose` | 223 | 清理 |
| 2 | `event` | 224 | 所有 server event |
| 3 | `config` | 225 | 修改 config（一次性） |
| 4 | `tool` | 226-228 | 自定义工具 `{[key]: ToolDefinition}` |
| 5 | `auth` | 229 | OAuth / api-key auth 注册 |
| 6 | `provider` | 230 | `{id, models?}` |
| 7 | `chat.message` | 234-243 | 新用户消息 |
| 8 | `chat.params` | 247-256 | **修改 LLM 参数** |
| 9 | `chat.headers` | 257-260 | **修改 HTTP 头** |
| 10 | `permission.ask` | 261 | 权限 gate |
| 11 | `command.execute.before` | 262-265 | 命令前修改 |
| 12 | `tool.execute.before` | 266-269 | **工具参数修改** |
| 13 | `shell.env` | 270-273 | **Shell 环境注入** |
| 14 | `tool.execute.after` | 274-281 | 工具后修改 |
| 15 | `experimental.chat.messages.transform` | 282-290 | 消息转换 |
| 16 | `experimental.chat.system.transform` | 291-296 | 系统 prompt |
| 17 | `experimental.provider.small_model` | 297 | 覆盖小模型选择 |
| 18 | `experimental.session.compacting` | 305-308 | 压缩 prompt 覆盖 |
| 19 | `experimental.compaction.autocontinue` | 316-326 | 跳过自动续轮 |
| 20 | `experimental.text.complete` | 327-330 | 流式文本转换 |
| 21 | `tool.definition` | 334 | **修改工具 description + parameters** |

**亮点**：所有 Hook 签名都是 `(input, output) => Promise<void>` 的双参数结构——**output 是 mutable 对象，插件可通过修改 output 拦截 + 修改行为**。这是**最严谨的"参数化钩子"模型**。

#### 3.3.2 Effect 类型化触发器（`packages/opencode/src/plugin/index.ts`）

行 41-58：
```typescript
// Hook names that follow the (input, output) => Promise<void> trigger pattern
type TriggerName = {
  [K in keyof Hooks]-?: NonNullable<Hooks[K]> extends (input: any, output: any) => Promise<void> ? K : never
}[keyof Hooks]

export interface Interface {
  readonly trigger: <Name extends TriggerName, ...>(
    name: Name, input, output,
  ) => Effect.Effect<Output>
  readonly list: () => Effect.Effect<Hooks[]>
  readonly init: () => Effect.Effect<void>
}
```

**这里通过 TypeScript 条件类型自动筛选出"符合 trigger 模式的 hook"**——任何不是 `(input, output) => Promise<void>` 签名的 hook 都不在 trigger 命名空间。

**Trigger 实现关键点**（`plugin/index.ts:284-308`）：
- 用 `InstanceState.make<State>`（行 134）做 per-instance 状态隔离
- `Effect.fn("Plugin.trigger")` 函数（行 284-308）遍历 `s.hooks`，对每个匹配的 hook 调用
- **隐患**（`plugin/index.ts:294`）：用 `Effect.promise(async () => fn(...))` 而不是 `Effect.tryPromise`——**hook 抛错会传播，整个 trigger abort**

#### 3.3.3 加载器（`loader.ts` 237 行 + `shared.ts` 323 行）

`/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/plugin/loader.ts` 用**四阶段管线**：

1. **`plan(item)`** 行 77-80：normalize config 为 `{spec, options, deprecated}`
2. **`resolve(plan, kind)`** 行 86-133：
   - `resolvePluginTarget`（`shared.ts:207-213`）找本地路径或 npm install
   - `createPluginEntry`（`shared.ts:224-236`）找 entrypoint
   - `checkPluginCompatibility`（`shared.ts:194-205`）读 `engines.opencode` 字段 + semver 校验
3. **`load(row)`** 行 136-145：`await import(row.entry)`
4. **`attempt(...)` / `loadExternal(...)`** 行 149-236：编排并行加载

**关键设计**：
- **failure 分 stage**：`stage: "install" | "entry" | "compatibility" | "load"`（`loader.ts:53-56`）
- **Bun 缓存失败的动态导入**（`loader.ts:204-207`）—— load 失败对当前进程是终态
- **deterministic 顺序**（`plugin/index.ts:222-223`）：**plugin 执行是顺序的**（`Effect.gen` + `yield*`），保证 hook 注册顺序

#### 3.3.4 内置插件（12 个）

`/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/plugin/index.ts` 行 67-86：

```ts
function internalPlugins(flags: RuntimeFlags.Info): PluginInstance[] {
  return [
    CodexAuthPlugin, CopilotAuthPlugin, ModalPlugin,
    GitlabAuthPlugin, PoeAuthPlugin, CloudflareWorkersAuthPlugin,
    CloudflareAIGatewayAuthPlugin, AzureAuthPlugin, DigitalOceanAuthPlugin,
    SnowflakeCortexAuthPlugin, XaiAuthPlugin, CerebrasPlugin,
  ]
}
```

内置插件跟外部插件走完全相同的 Plugin 类型——优雅的一致性。

#### 3.3.5 TUI 插件运行时（独立模块）

`/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/plugin/tui/runtime.ts`（**1132 行**）—— 比 server plugin 复杂得多：

| 功能 | 行号 | 备注 |
|------|------|------|
| `init()` | 988-1006 | **idempotent per cwd**，cwd 不同则 throw |
| `activatePluginEntry` | 516-556 | 创建 `PluginScope` + 调 plugin + 失败时 dispose + disable |
| `deactivatePluginEntry` | 502-514 | `scope.dispose()` + **5 秒超时**（`DISPOSE_TIMEOUT_MS` 行 122） |
| `dispose()` | 1029-1049 | 逆序迭代 plugin，依次 deactivate |
| `installPlugin` | 891-982 | **bun add → readPluginManifest → patchPluginConfig** |
| `addPlugin` | 1021 | 动态添加插件 |
| `internalTuiPlugins` | 6-10 | 内置 TUI 插件 |

**enabled 状态持久化在 KV**（`plugin_enabled` key，行 122-123）——重启后状态保留。

#### 3.3.6 沙箱：**没有插件沙箱，但有一组限制**

**没有进程级沙箱**。但有一组**契约式限制**（来自 Explore Subagent 的详细分析）：

| 限制 | 行号 | 说明 |
|------|------|------|
| **不能 import 插件目录外代码**（除非有 `.opencode/package.json`） | `plugins.mdx:77-83` | 强制本地 plugin 有 manifest |
| **`npm install` 跳过 lifecycle scripts** | `packages/core/src/npm.ts:92` | `ignoreScripts: true` |
| **不能 symlink/escape plugin 目录** | `shared.ts:89-97` `resolvePackageFile` | `Filesystem.contains(root, next)` 边界检查 |
| **不能跳过 opencode major-version gate** | `shared.ts:194-205` | `engines.opencode` 校验 |
| **`.tsx` 插件不能从 file glob 发现** | `config/plugin.ts:21` | glob 是 `{plugin,plugins}/*.{ts,js}` |
| **`--pure` + 外部插件 互斥** | `runtime.ts:1089` | `Flag.OPENCODE_PURE` 跳过外部加载 |
| **不能绕过 permission system** | `plugin/index.ts:261` | `permission.ask` 必须返回 deny/allow/ask |
| **network 无 allowlist** | — | `fetch` 任意调用 |

**Net assessment**（Explore Subagent 原话）："The plugin system is trust-based: if you install a plugin, you trust its author with arbitrary code execution, full filesystem read/write, arbitrary network egress, and arbitrary IPC into the opencode server via the SDK client."

#### 3.3.7 分发：NPM + file:// + 配置数组

| spec 形式 | 来源 | 兼容性检查 |
|----------|------|----------|
| `npm:package-name` | npm registry | ✅ `engines.opencode` 校验 |
| `file:/path/to/plugin` | 本地目录 | ❌ 跳过 |

**Filesystem glob**（`/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/config/plugin.ts:18-30`）：
```ts
for (const item of await Glob.scan("{plugin,plugins}/*.{ts,js}", {
  cwd: dir, absolute: true, dot: true, symlink: true,
})) {
  plugins.push(pathToFileURL(item).href)
}
```

**安装流程**（`runtime.ts:891-982` + `install.ts`）：
1. `installModulePlugin(spec)` 跑 `bun add` 进项目 `node_modules`
2. `readPluginManifest(target)` 读 `package.json` + 校验 entrypoint
3. `patchPluginConfig` 重写 `opencode.json` / `tui.json`
4. `state.pending.set(spec, origin)` 入队等下次 `addPlugin`

**没有 marketplace**——文档 `plugins.mdx:42` 链接到 `/docs/ecosystem#plugins`，但**二进制内没有搜索机制**。

**Deprecation list**（`shared.ts:10-14`）：`DEPRECATED_PLUGIN_PACKAGES = ["opencode-openai-codex-auth", "opencode-copilot-auth"]`——因为已内置，自动忽略。

---

### 3.4 pi：40+ Event + 完整 SDK（最小但最精致）

#### 3.4.1 Extension API 表面（最完整的事件类型）

`/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/extensions/types.ts` 是 Extension API 的核心。`ExtensionAPI` 接口（`types.ts:1252-1506`）提供 **40+ 事件 + 25+ 注册方法**。

**事件分类**（按 `ExtensionEvent` 联合类型，`types.ts:1086-1113`）：

| 类别 | 事件 | 关键字段 |
|------|------|---------|
| **启动** | `project_trust`, `resources_discover`, `session_start` | reason, cwd |
| **会话** | `session_before_switch`, `session_before_fork`, `session_before_compact`, `session_compact`, `session_compact_failed`, `session_shutdown`, `session_before_tree`, `session_tree`, `session_info_changed` | preparation, reason |
| **Agent** | `context`, `before_provider_request`, `before_provider_headers`, `after_provider_response`, `before_agent_start`, `agent_start`, `agent_end`, `agent_settled`, `turn_start`, `turn_end`, `ui_prompt_start`, `ui_prompt_end` | messages, payload |
| **消息** | `message_start`, `message_update`, `message_end` | message, assistantMessageEvent |
| **工具** | `tool_execution_start`, `tool_execution_update`, `tool_execution_end`, `tool_call`, `tool_result` | toolCallId, args |
| **模型** | `model_select`, `thinking_level_select` | model, level |
| **输入** | `input`, `user_bash` | text, command |

**注册方法**（`ExtensionAPI` 接口，`types.ts:1252-1506`）：

| 方法 | 行号 | 用途 |
|------|------|------|
| `on(event, handler)` | 1257-1301 | 订阅 40+ 种事件 |
| `registerTool(tool)` | 1308-1310 | 注册 LLM 可调用工具（TypeBox schema） |
| `registerCommand(name, opts)` | 1317 | 注册斜杠命令 |
| `registerShortcut(keyId, opts)` | 1320-1326 | 注册键盘快捷键 |
| `registerFlag(name, opts)` | 1329-1342 | 注册 CLI flag |
| `registerMessageRenderer(customType, renderer)` | 1352 | 自定义消息渲染 |
| `registerMarkdownTransformer(transformer)` | 1355 | Markdown 转换 |
| `registerEntryRenderer(customType, renderer)` | 1358 | 自定义 session entry 渲染 |
| `sendMessage(message, opts)` | 1365-1368 | 发送自定义消息 |
| `sendUserMessage(content, opts)` | 1375-1378 | 发送用户消息 |
| `appendEntry(customType, data)` | 1381 | 追加 session entry |
| `setSessionName(name)` | 1388 | 设置会话名 |
| `setLabel(entryId, label)` | 1394 | 设置 entry 标签 |
| `exec(command, args, opts)` | 1397 | 执行 shell 命令 |
| `getActiveTools/setActiveTools` | 1400, 1406 | 工具启用 |
| `setModel(model)` | 1419 | 切换模型 |
| `setThinkingLevel(level)` | 1428 | 设置思考等级 |
| `registerProvider(provider)` | 1486-1487 | 注册 LLM provider |
| `unregisterProvider(name)` | 1502 | 注销 provider |

#### 3.4.2 扩展加载器（`loader.ts`）

`/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/extensions/loader.ts` 用 **jiti**（行 17 `createJiti`）作为 TypeScript 加载器：

```ts
const jiti = createJiti(import.meta.url, {
  moduleCache: false,
  virtualModules: VIRTUAL_MODULES,  // 编译后的二进制用 virtualModules
  tryNative: false,
});
```

**双 runtime 适配**：
- Bun 二进制 / Node SEA / Bundled Node → `virtualModules: VIRTUAL_MODULES`（行 503-504）
- 源码 TypeScript 运行时 → `virtualModules + tsconfigPaths`（行 506）
- 非 bundled Node → `alias: getAliases()`（行 507）

**Discovery 三层**（`loader.ts:758-806`）：
1. Project-local：`cwd/${CONFIG_DIR_NAME}/extensions/`
2. Global：`agentDir/extensions/`
3. 显式路径（含 `package.json#pi.extensions` manifest）

**`package.json` manifest 字段**（行 680-708）：
```json
{
  "pi": {
    "extensions": ["./src/index.ts", "./src/extras.ts"]
  }
}
```

#### 3.4.3 完整示例（`.pi/extensions/tps.ts`）

```ts
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  pi.on("agent_start", () => { agentStartMs = Date.now(); });
  pi.on("agent_end", (event, ctx) => {
    if (!ctx.hasUI) return;
    // 计算 TPS 并通过 ui.notify 显示
    ctx.ui.notify(`TPS ${tokensPerSecond.toFixed(1)} tok/s`, "info");
  });
}
```

#### 3.4.4 沙箱：**没有，但有 project_trust 提示**

`ProjectTrustEvent` + `ProjectTrustContext`（`types.ts:521-543`）让扩展在每个项目启动时**询问用户是否信任**：

```ts
pi.on("project_trust", async (event, ctx) => {
  // event.cwd, ctx.cwd 都可读
  // 用户可以返回 { trusted: "yes" | "no", remember?: boolean }
});
```

这是一个**用户级信任决策**（每次新项目启动询问一次），**不是进程级沙箱**。

#### 3.4.5 分发：**本地文件，无 marketplace**

`pi` 没有 marketplace。扩展文件就是本地 `.pi/extensions/*.ts`，直接通过 jiti 加载。**优点：完全无网络依赖；缺点：用户无法一键安装**。

---

### 3.5 deepseek-harness：Cordis 5 态 Fiber + 27 事件桥

#### 3.5.1 Cordis 三原语

`/usr/local/LsmGitOpenSource/deepseek-harness/vendor/cordis/src/registry.ts` 定义：

| 原语 | 类型 | 行号 |
|------|------|------|
| `Plugin` | `Plugin.Function \| Plugin.Constructor \| Plugin.Object` | 92-95 |
| `Service` | base class for typed services | `service.ts:11-115` |
| `Context` | root plugin container | `context.ts` |
| `RegistryService` | plugin 索引表 | `registry.ts:195-337` |

#### 3.5.2 Fiber 5 态状态机（`vendor/cordis/src/fiber.ts`）

```ts
export const enum FiberState {
  PENDING,    // 等待依赖服务
  LOADING,    // 插件回调执行中
  ACTIVE,     // 已加载并提供
  FAILED,     // 加载或配置抛错
  UNLOADING,  // disposers 运行中
  DISPOSED,   // 已移除，不能重启
}
```

**这是 7 个工程里唯一显式建模插件生命周期状态机的**。其他工程用 `loaded | disabled | error` 字符串（openclaw）或 `present | absent`（pi）。

#### 3.5.3 Standard Schema 配置验证

```ts
// registry.ts:103
Config?: StandardSchemaV1<any, T>
```

插件配置用 **Standard Schema**（V1）验证——支持 Zod / Valibot / ArkType 任何实现该协议的库。

#### 3.5.4 hooks-claude-code 适配器（**关键的"嫁接"**）

`/usr/local/LsmGitOpenSource/deepseek-harness/packages/hooks/hooks-claude-code/src/index.ts` 是**关键的 27 事件适配器**：

- 读取 Claude Code 格式的 `hooks.json`（行 103-105）
- 解析 matcher（行 105-108）
- 对每种事件类型注册对应的 Cordis hook：
  - `PreToolUse` → `before_tool_call`
  - `PostToolUse` → `after_tool_call`
  - `SessionStart` → `session_start`
  - `Stop` → `stop` (允许 hook 强制续轮)
  - `SubagentStart` → 上下文注入
- **参数替换**：`${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PROJECT_DIR}`（行 56-66）
- **默认超时**：`DEFAULT_HOOK_TIMEOUT_MS = 600000`（10 分钟，CC 默认）

**这是 deepseek-harness 选择"桥接"而非"重写"策略的体现**——它把 27 种 Hook 当作**已有插件生态**，自己不重新发明。

#### 3.5.5 沙箱：**没有**

Cordis plugin 是 in-process，没有 sandbox。

#### 3.5.6 分发：NPM Cordis plugin + hooks-claude-code

- Cordis 框架本身可作为 npm 包发布
- 仓库内 `packages/extensions/cordis-*-runner`、`tool-cordis`、`ui-cordis`（`/usr/local/LsmGitOpenSource/deepseek-harness/packages/extensions/`）是 host runner
- Hook 配置文件按 Claude Code 兼容格式分发

---

### 3.6 atomcode：CC 兼容 manifest + hook_trust 哈希（治理型）

#### 3.6.1 CC 兼容 manifest（`plugin_manifest`）

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/plugin/manifest.rs` 定义**双兼容 manifest**：

```rust
pub struct PluginManifest {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub skills: Option<PathOrList>,         // "skills" or ["./skills/a", "./skills/b"]
    pub commands: Option<PathOrList>,
    pub hooks: Option<HooksField>,          // 路径 OR 内联 CC hooks map
}
```

`HooksField` 是 enum（`manifest.rs:148-154`）：
```rust
pub enum HooksField {
    Path(String),        // 老格式：路径到 hooks.json
    Inline(CCHooksMap),  // 新格式：内联
}
```

**CC hooks map 兼容 schema**（`manifest.rs:160-178`）：
```rust
pub type CCHooksMap = BTreeMap<String, Vec<CCHookGroup>>;
// Event -> [{ matcher?, hooks: [{ type: "command", command, timeout? }] }]
```

这是 6 个工程里**唯一做了 `serde(untagged)` 兼容双 wire 格式**的。

#### 3.6.2 plugin_hook_set_hash（**全仓唯一的内容哈希**）

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/plugin/hook_trust.rs` 行 25-45：

```rust
/// SHA-256 over the sorted (event, matcher, command) triples
pub fn plugin_hook_set_hash(hooks: &[PluginCcHook]) -> String {
    let mut triples: Vec<(&str, &str, &str)> = hooks.iter()
        .map(|h| (h.event.as_str(), h.matcher.as_deref().unwrap_or(""), h.command.as_str()))
        .collect();
    triples.sort_unstable();
    let mut hasher = Sha256::new();
    for (event, matcher, command) in &triples {
        for field in [event, matcher, command] {
            hasher.update((field.len() as u64).to_le_bytes());  // 长度前缀防冲突
            hasher.update(field.as_bytes());
        }
    }
    format!("{:x}", hasher.finalize())
}
```

**安全特性**：
1. **字段长度前缀**（`hasher.update((field.len() as u64).to_le_bytes())`）防止边界攻击
2. **排序**后 hash 让 hook 顺序不重要
3. **每次 hook 变更 → hash 变更 → 用户必须重新 trust**（行 1-5 注释）

#### 3.6.3 Trust store（`hook_trust.json`）

存储在 `$ATOMCODE_HOME/plugins/hook_trust.json`（`hook_trust.rs:48`），键是 `<plugin>@<marketplace>`，值是 SHA-256 hash。

**关键安全语义**（行 1-5 注释）：
> "Changing a hook command changes the hash → re-trust required, which blocks a benign-at-install plugin from silently adding hooks in an update."

**这是 7 个工程里最严肃的"插件 hook 内容寻址"机制**——可以防御"安装时无 hook，update 时静默加恶意 hook"的攻击。

#### 3.6.4 marketplace 解析（`installer.rs` + `marketplace.rs`）

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/plugin/installer.rs` 支持 5 种外部源（`manifest.rs:55-88`）：

```rust
pub enum ExternalSource {
    Url { url, pin: GitPin },
    Git { url, pin: GitPin },
    Github { repo, pin: GitPin },
    GitSubdir { url, path, pin: GitPin },  // 子目录 sparse checkout
    Local { path },
}
```

**`Unknown(serde_json::Value)` 容错**（`manifest.rs:46`）：
```rust
pub enum PluginSource {
    Inline(String),
    External(ExternalSource),
    Unknown(serde_json::Value),  // 单条未知 source 不破坏整个 catalog
}
```

#### 3.6.5 bootstrap（`bootstrap.rs` 404 行）

首次启动 + 每次启动都跑（`bootstrap.rs:16-20`）：

```rust
// 1. Fresh install: 克隆官方 marketplace + 写入 marker file
// 2. Every startup: git pull --ff-only 每个 marketplace
```

**全部 best-effort，失败不中断**（行 26-29 注释）：
> "AtomCode must remain usable on offline machines, in air-gapped corporate environments, on systems without git, etc."

错误写到 `$ATOMCODE_HOME/stderr.log` 而不是 stderr fd（行 41-46 注释）——避免污染 TUI 输入框。

#### 3.6.6 沙箱：**没有**

CC hooks 是 subprocess 执行，**子进程以 atomcode 身份运行**，没有沙箱。

#### 3.6.7 分发：**marketplace 驱动**

- `marketplaces.json` 记录 marketplace URL
- `installed_plugins.json` 记录已装插件
- Bootstrap marker `$ATOMCODE_HOME/.plugin_bootstrap_v2` 控制首次安装

**没有签名**（只有内容哈希 trust）。

---

### 3.7 undici：8 拦截器 + `compose()` 链接式（非 Agent）

> undici 不是 Agent CLI，是 Node.js HTTP 客户端，但其 Dispatcher/Interceptor 模式是 7 个工程里**最纯粹、最经典**的拦截器实现。

#### 3.7.1 Dispatcher 基类（`dispatcher.js` 54 行）

`/usr/local/LsmGitOpenSource/undici/lib/dispatcher/dispatcher.js`：

```js
class Dispatcher extends EventEmitter {
  dispatch() { throw new Error('not implemented') }
  close() { throw new Error('not implemented') }
  destroy() { throw new Error('not implemented') }

  compose (...args) {           // 行 18
    const interceptors = Array.isArray(args[0]) ? args[0] : args
    let dispatch = this.dispatch.bind(this)
    for (const interceptor of interceptors) {
      if (interceptor == null) continue
      if (typeof interceptor !== 'function')
        throw new TypeError(`invalid interceptor, expected function received ${typeof interceptor}`)
      dispatch = interceptor(dispatch)
      if (dispatch == null || typeof dispatch !== 'function' || dispatch.length !== 2)
        throw new TypeError('invalid interceptor')
    }
    // 返回 Proxy 拦截 dispatch
    return new Proxy(this, {
      get: (target, key) => key === 'dispatch' ? dispatch : target[key]
    })
  }
}
```

**`compose()` 的精妙之处**：通过 `Proxy` 替换 `dispatch` 方法，但保留所有其它属性访问。这意味着 `agent.compose(retry, redirect).request(...)` 既能用上拦截器，又能用上原 agent 的连接池、统计、close/destroy 语义。

#### 3.7.2 DispatcherBase（`dispatcher-base.js` 197 行）

`/usr/local/LsmGitOpenSource/undici/lib/dispatcher/dispatcher-base.js` 添加生命周期：
- `close(callback)` 行 68-112：等待 in-flight 请求完成后关闭
- `destroy(err, callback)` 行 114-160：强制终止
- `dispatch(opts, handler)` 行 162-194：统一错误处理 + ClientDestroyed/Closed 检查

#### 3.7.3 11 种生命周期回调

`/usr/local/LsmGitOpenSource/undici/docs/docs/api/Dispatcher.md` 列出 dispatcher 提供的 11 种回调（不是 8，是 11）：

1. `onConnect(connectParams, socket)` —— TCP 连接建立
2. `onError(err)` —— 错误
3. `onUpgrade(statusCode, headers, socket)` —— HTTP 升级（WebSocket）
4. `onResponseStart(controller, statusCode, headers, statusMessage)` —— 响应头到达
5. `onResponseData(controller, chunk)` —— 响应数据块
6. `onResponseEnd(controller, trailers)` —— 响应完成
7. `onResponseError(controller, error)` —— 响应错误
8. `onBodySent(chunk)` —— 请求 body 已发
9. `onRequestSent()` —— 请求已发
10. `onHeaders(headers)` —— 响应头（替代 onResponseStart 的旧 API）
11. `onComplete(trailers)` —— 完成（含 trailers）

#### 3.7.4 8 个内置拦截器（`index.js:49-58`）

```js
module.exports.interceptors = {
  redirect,        // lib/interceptor/redirect.js
  responseError,   // lib/interceptor/response-error.js
  retry,           // lib/interceptor/retry.js
  dump,            // lib/interceptor/dump.js
  dns,             // lib/interceptor/dns.js
  cache,           // lib/interceptor/cache.js
  decompress,      // lib/interceptor/decompress.js
  deduplicate      // lib/interceptor/deduplicate.js
}
```

**每个拦截器都是 `(dispatch) => (opts, handler) => Promise`** 模式，**完全无状态、可组合**。

#### 3.7.5 retry 拦截器（`retry.js` 19 行 —— 最精炼）

```js
'use strict'
const RetryHandler = require('../handler/retry-handler')

module.exports = globalOpts => {
  return dispatch => {
    return function retryInterceptor (opts, handler) {
      return dispatch(
        opts,
        new RetryHandler(
          { ...opts, retryOptions: { ...globalOpts, ...opts.retryOptions } },
          { handler, dispatch }
        )
      )
    }
  }
}
```

#### 3.7.6 dns 拦截器（`dns.js` 575 行 —— 最复杂）

`/usr/local/LsmGitOpenSource/undici/lib/interceptor/dns.js` 实现完整的 DNS 解析 + 缓存 + 故障转移：

- `DNSStorage`（行 107-135）—— LRU-like 缓存
- `DNSInstance.runLookup`（行 156-239）—— 解析逻辑
- `DNSDispatchHandler`（行 386-456）—— `onResponseError` 处理双栈回退
- 配置项：`maxTTL`、`maxItems`、`affinity`、`dualStack`、`lookup`、`pick`、`storage`（行 458-514）

#### 3.7.7 沙箱：**没有，但拦截器链是天然的隔离层**

每个拦截器只看到 `(opts, handler)`，看不到 socket、看不到 system —— 这是一种**关注点隔离**，但不是进程级沙箱。

#### 3.7.8 分发：**无 marketplace，纯 npm 包**

undici 自身就是 npm 包，没有"插件 marketplace"概念。第三方拦截器在应用层 require + compose。

---

## 4. 横向对比大表：7 工程 × 8 维度

### 4.1 总表

| 维度 | claudecode | openclaw | opencode | pi | deepseek-harness | atomcode | undici |
|------|-----------|----------|----------|----|------------------|----------|--------|
| **Hook 事件数** | 27 | 43 typed + 8 internal | 25+ | **40+** | 27 (桥接 CC) | 8 (CC 兼容) | 11 (HTTP 生命周期) |
| **执行器类型** | 4: command/http/prompt/agent | 1: function | 1: function | 1: function | 1: subprocess | 1: subprocess | 1: handler |
| **Hook 决策** | JSON `{decision, reason}` | function 返回 | 修改 `output` 参数 | 返回 EventResult | 解析 stdout/exit code | 解析 stdout/exit code | 重写 dispatch opts |
| **Extension API 表面** | ❌ (无) | **50+ register*** | 25+ hooks | 25+ methods + 40+ events | class-based | manifest-driven | compose chain |
| **加载器** | settings.json 解析 | `loadOpenClawPlugins` | `PluginLoader.loadExternal` | `discoverAndLoadExtensions` + jiti | `ctx.plugin(Class)` | `installer.install` | `agent.compose(interceptor)` |
| **进程隔离** | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **签名/校验** | ❌ | SHA-256 (ClawHub) | ❌ | ❌ | ❌ | **SHA-256 hook_trust** | ❌ |
| **Marketplace** | ❌ (MCP only) | ClawHub 153 ext | NPM + file:// | ❌ | npm + cc 兼容 | 自建 marketplace | ❌ (npm only) |
| **生命周期模型** | fire-and-forget | 6 阶段 + failurePhase | 4 stage error reporting | load → init → register | 5 态 Fiber state machine | install + bootstrap | compose chain (无) |
| **失败语义** | fail-open | fail-open + rollback | fail-open + stage report | fail-closed on load | fail-closed (dispose) | fail-open + log | fail-fast (per-handler) |

### 4.2 各维度详细对比

#### 4.2.1 注册模型

| 工程 | 声明式 (config) | 命令式 (code) | 双轨 |
|------|-----------------|---------------|------|
| claudecode | ✅ `settings.json` | 弱 | ✅ |
| openclaw | ✅ `openclaw.plugin.json` + `package.json#openclaw` | ✅ `definePluginEntry(register)` | ✅ |
| opencode | ✅ config `plugin: []` 数组 | ✅ `Plugin = (input) => Promise<Hooks>` | ✅ |
| pi | ✅ `package.json#pi.extensions` | ✅ `extension factory(pi)` | ✅ |
| deepseek-harness | ✅ `Config` Standard Schema | ✅ `class extends Service` | ✅ |
| atomcode | ✅ `marketplace.json` + `plugin.json` | 弱 (subprocess) | 半 |
| undici | ❌ | ✅ `compose(interceptor)` | 单 |

#### 4.2.2 触发时机精度

| 工程 | 工具调用 | 会话 | 上下文压缩 | 模型调用 | 文件操作 | 其它 |
|------|---------|------|-----------|---------|---------|------|
| claudecode | PreToolUse/PostToolUse/PostToolUseFailure | SessionStart/SessionEnd/Stop/SubagentStop | PreCompact/PostCompact | (via `before_provider_request` agent) | ❌ | UserPromptSubmit/Notification |
| openclaw | before_tool_call/after_tool_call | session_start/session_end | before_compaction/after_compaction | llm_input/llm_output/model_call_* | ❌ | 43 typed events |
| opencode | tool.execute.before/after | session_start | experimental.session.compacting | chat.message/chat.params/chat.headers | (shell.env hook) | permission.ask |
| pi | tool_call/tool_result (per tool) | 10+ session events | session_before_compact/session_compact | context/before_provider_* | ❌ | 40+ events |
| deepseek-harness | PreToolUse/PostToolUse (via bridge) | SessionStart/Stop | (via bridge) | (via agent/request-error) | ❌ | 27 (via bridge) |
| atomcode | PreToolUse/PostToolUse (CC) | SessionStart | (CC compat) | ❌ | ❌ | UserPromptSubmit/Notification |
| undici | 11 HTTP 生命周期 (非 Agent) | N/A | N/A | N/A | N/A | N/A |

#### 4.2.3 决策模型

| 工程 | 决策来源 | 决策 schema | 决策时机 |
|------|---------|-------------|---------|
| claudecode | JSON 输出 / LLM 二次决策 | `{decision, reason, modifiedInput}` | hook 返回时 |
| openclaw | function 返回值 | typed by hook signature | hook 返回时 |
| opencode | **修改 output 参数** | output 是 mutable object | 链中每个 hook 都能改 |
| pi | typed EventResult | discriminated union | hook 返回时 |
| deepseek-harness | mergeHookOutputs (CC 风格) | CC-compatible | hook 完成后 |
| atomcode | exit code + stdout | CC-compatible | subprocess 退出 |
| undici | 拦截器重写 dispatch | opts 是 mutable | dispatch 时 |

**注意**：opencode 的"修改 output"模式是 7 个工程里**最少 copy**的——所有 hook 共享同一个 output 对象，**链式修改天然支持**。

#### 4.2.4 失败处理

| 工程 | Hook 失败 | Plugin 失败 | 超时 |
|------|----------|------------|------|
| claudecode | **fail-open**（静默继续） | n/a | 600s 默认 |
| openclaw | log + 继续 | **rollbackPluginGlobalSideEffects** | service 5s cleanup |
| opencode | log + continue | 4 stage error reporting | n/a |
| pi | log + extension runtime invalidate | load error in LoadExtensionsResult.errors | n/a |
| deepseek-harness | log + 继续 | **fail-closed**（Fiber → FAILED → dispose） | 10min (600000ms) |
| atomcode | log + 跳过 | log + 跳过 | (CC compat) |
| undici | handler.onResponseError | n/a (单进程) | client timeout |

#### 4.2.5 Extension API 抽象层（深度对比）

| API 类别 | claudecode | openclaw | opencode | pi | deepseek-harness | atomcode |
|----------|-----------|----------|----------|----|------------------|----------|
| **注册工具** | ❌ (仅内置) | `registerTool` (api.types:209) | `tool: {[name]: ToolDefinition}` | `registerTool(definition)` | `ctx.plugin(ToolClass)` | ❌ (subprocess) |
| **注册命令** | ❌ | `registerCommand` | ❌ | `registerCommand` | `ctx.command()` | ❌ |
| **注册 provider** | ❌ | `registerProvider` (50+ 选项) | `provider: ProviderHook` | `registerProvider` | `ctx.plugin(ProviderClass)` | ❌ |
| **注册 channel** | ❌ | `registerChannel` | ❌ | ❌ | `ctx.plugin(ChannelClass)` | ❌ |
| **注册 MCP** | ✅ (内置) | `registerMcpServerConnectionResolver` | (via tool) | ❌ | `ctx.plugin(MCPClass)` | ❌ |
| **修改提示词** | (via prompt hook) | `before_prompt_build` hook | `chat.message`/`chat.params` | `context`/`before_agent_start` | `beforePromptBuild` | ❌ |
| **注入 LLM headers** | ❌ | ❌ | `chat.headers` | `before_provider_headers` | `request-headers` hook | ❌ |
| **修改工具 schema** | ❌ | `inspectToolSchemas` | `tool.definition` | ❌ | `ctx.provide` | ❌ |
| **执行 shell** | (via hook) | `api.runtime.fetch` / `api.session.exec` | `$: BunShell` | `exec(command, args, opts)` | `ctx.shell` | ❌ |
| **注册 UI** | ❌ | `registerControlUiDescriptor` | `tui.ts` (634 行) | `ctx.ui.setWidget/setFooter/setHeader` | `ctx.ui` | ❌ |

#### 4.2.6 插件沙箱

| 工程 | 插件进程 | 文件访问 | 网络 | 环境变量 | 资源限制 |
|------|---------|---------|------|---------|---------|
| claudecode | in-process | 全权限 | 全权限 | 全权限 | ❌ |
| openclaw | in-process | 全权限 | 全权限 + SSRF policy | 全权限 | service 5s cleanup |
| opencode | in-process + Bun | 全权限 | 全权限 | shell.env 注入 | ❌ |
| pi | in-process + jiti (TypeScript 沙箱) | 全权限 | 全权限 | 全权限 | ❌ |
| deepseek-harness | in-process | 全权限 | 全权限 | 全权限 | ❌ |
| atomcode | in-process (hook 是 subprocess) | 全权限 | 全权限 | 全权限 | ❌ |
| undici | in-process | N/A | 可拦截 | N/A | handler timeout |

**结论**：**7 个工程没有一个做插件沙箱**。`pi` 的 jiti 是"语法层"沙箱（避免让 plugin 阻塞主线程执行未编译 TS），不是权限沙箱。

#### 4.2.7 插件生命周期

| 工程 | 状态数 | 状态名 | 显式 reload | 热替换 |
|------|--------|--------|-------------|--------|
| claudecode | 1 | "loaded" | ❌ | ❌ |
| openclaw | 4 | "loaded" \| "disabled" \| "error" \| "registering" | ✅ `refreshPluginRegistryAfterConfigMutation` | ✅ (5s cleanup) |
| opencode | 4 (stage) | "install" \| "entry" \| "compatibility" \| "load" | ✅ (config reload) | ❌ |
| pi | 3 | "loading" \| "active" \| "failed" | ✅ `clearExtensionCache()` | ✅ |
| deepseek-harness | **6** | PENDING/LOADING/ACTIVE/FAILED/UNLOADING/DISPOSED | ✅ (Fiber dispose) | ✅ |
| atomcode | 4 | scope: User/Project/Global/Marketplace | ❌ | ❌ |
| undici | 2 | "open" \| "closed" | n/a | n/a |

**deepseek-harness 的 6 态状态机是最严谨的**——把 UNLOADING（disposers 运行中）和 DISPOSED（已移除）显式分开，对插件代码错误恢复很关键。

#### 4.2.8 插件市场 / 分发

| 工程 | 仓库/源 | 版本约束 | 签名 | 沙箱下载 |
|------|---------|---------|------|---------|
| claudecode | MCP only | semver | ❌ | ❌ |
| openclaw | ClawHub + git + npm | minHostVersion + compat.pluginApi | SHA-256 | ❌ |
| opencode | npm + file:// | opencode.compat.version | ❌ | ❌ |
| pi | 本地 only | ❌ | ❌ | n/a |
| deepseek-harness | npm + 兼容 CC | semver | ❌ | ❌ |
| atomcode | 自建 marketplace (git) | GitPin (branch/tag/commit) | **SHA-256 hook_trust** | ❌ |
| undici | npm | semver | n/a | n/a |

---

## 5. 共性模式（强调 claudecode 27 Hook 是行业标杆）

### 5.1 模式 1：Hook = "已发生事件的副作用"

7 个工程都把 Hook 看作"在某个事件点插入一段逻辑"，**不论执行器是 function、subprocess 还是 HTTP 调用**。

**关键**：claudecode 的 27 种事件覆盖了 agent 生命周期的所有关键节点：
- 工具调用前后（PreToolUse / PostToolUse）
- 会话开始结束（SessionStart / SessionEnd）
- 上下文压缩（PreCompact / PostCompact）
- 用户输入（UserPromptSubmit）
- 通知（Notification）
- 权限（PermissionRequest）
- 子代理（SubagentStart / SubagentEnd）

**这是事实上的行业标准**——deepseek-harness 的 `hooks-claude-code` 适配器就是把这个事件清单当作"接口契约"。

### 5.2 模式 2：Extension = "对 host 能力的扩展"

7 个工程都让 Extension 注册新能力（provider/tool/command）而不是仅做事件副作用。

**关键**：Extension API 的大小差异巨大：
- claudecode：≈0（无扩展点）
- atomcode：3（skills/commands/hooks）
- deepseek-harness：~30 (Cordis 原生 + service)
- pi：25+ methods
- opencode：25+ hook 签名 + auth/provider/tool
- openclaw：50+ register methods
- undici：1（compose chain）

### 5.3 模式 3：双轨注册（声明式 + 命令式）

7 个工程都允许：
- 声明式：通过配置文件（JSON/YAML/TOML）声明 hook / plugin
- 命令式：通过代码（class / function）实现 plugin

claudecode 偏声明式，openclaw 偏命令式，**两者最均衡**。

### 5.4 模式 4：in-process 插件 + 强 host 信任

**7 个工程 100% 都是 in-process 插件**。没有任何一个做 WASM 沙箱或独立进程。

这反映了 AI Agent 生态的现实选择：
- WASM 沙箱性能开销太大（IPC 序列化整个 context 不现实）
- 独立进程沙箱管理复杂（多进程通信、状态共享）
- **实际选择**：信任 + 显式 allowlist + marketplace 审计 + 事后回滚

openclaw 的 `rollbackPluginGlobalSideEffects` 是这个模式的最成熟实现。

### 5.5 模式 5：Decision schema 标准（CC 兼容）

claudecode 的 hook decision schema：
```json
{ "decision": "approve" | "block" | "modify", "reason": "...", "modifiedInput": {...} }
```

被 deepseek-harness 的 `mergeHookOutputs` 和 atomcode 的 CC 兼容 parser **完整继承**。

**这是 Hook 决策模型的事实标准**——任何新建 Hook 系统都应支持这个 schema。

### 5.6 模式 6：chain 化是天然选择

- undici：`compose([retry, redirect, decompress])` —— 显式链
- opencode：所有 `(input, output) => Promise<void>` hook 共享 output 对象 —— 隐式链
- pi：每个 event 可以注册多个 handler —— 隐式链
- openclaw：typed hooks 用 `api.on(name, fn)` —— 隐式链
- claudecode：`matcher` 排序后顺序执行 —— 显式链
- deepseek-harness：Cordis emit / waterfall 模式 —— 显式链

**没有 hook 系统是"单点"**。Chain 是必须的。

### 5.7 模式 7：版本兼容性是 marketplace 关键

- openclaw：`minHostVersion` + `compat.pluginApi`（双约束）
- opencode：`opencode.compat.version` + opencode 运行时版本检查
- atomcode：`GitPin` (branch/tag/commit)
- claudecode / pi / undici：N/A（无第三方 marketplace）

**多版本 host 兼容**是 marketplace 的核心难题，没有银弹。

---

## 6. 对 laew 的 P0/P1/P2 路线图（Rust crate 选型）

> 当前 laew 状态：3 个内置工具（Bash/Read/Write）+ 多 Agent 编排器，**完全没有 Hook / Extension 系统**。

### 6.1 P0（核心缺口，立即补）

#### 6.1.1 引入 Hook 系统（最少 8 种事件）

| 事件 | 触发点 | 决策 schema |
|------|--------|-------------|
| `PreToolUse` | Tool.execute 前 | `{decision: "approve"\|"block"\|"modify", reason, modifiedInput}` |
| `PostToolUse` | Tool.execute 后 | `{output: string, metadata: any}` |
| `PreCompact` | SessionContext 压缩前 | `{cancel: bool, customInstructions: string}` |
| `Stop` | Agent 循环结束 | `{continue: bool, reason: string}` |
| `SubagentStart` | SubAgent 委派前 | `{injectContext: string}` |
| `UserPromptSubmit` | 用户输入后 | `{modifiedPrompt: string}` |
| `SessionStart` | Session 启动 | n/a |
| `Notification` | 通知（error/warning） | n/a |

**Rust 实现**：
- `claudecode` 27 种事件是金标准，先实现上面 8 个最常用的
- 配置在 `~/.laew/settings.json` + 项目根 `.laew/settings.json`（**双层覆盖**）
- 决策 JSON schema 用 `schemars` crate 自动生成（已写入第七轮 gap L9）

#### 6.1.2 Rust crate 推荐

| 需求 | crate | 版本 | 用途 |
|------|-------|------|------|
| JSON schema 验证 | `schemars` | 1.0 | hook output schema |
| SHA-256 内容哈希 | `sha2` | 0.10 | hook trust hash（学习 atomcode） |
| 配置文件 | `serde_json` + `config` | 1.0 / 0.14 | settings.json 解析 |
| 子进程执行 | `tokio::process::Command` | 内置 | hook command type 执行 |
| 超时控制 | `tokio::time::timeout` | 内置 | hook 默认 600s 超时 |

**P0 预计工作量**：3-5 天（声明式 hook + 8 事件 + schemars + 测试）

### 6.2 P1（中期，2-4 周）

#### 6.2.1 引入 Extension API（第一版）

最小 Extension 表面：

```rust
pub trait Extension: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn register(&self, ctx: &mut dyn ExtensionContext) -> Result<(), ExtensionError>;
}

pub trait ExtensionContext {
    fn register_tool(&mut self, tool: Box<dyn Tool>);
    fn register_command(&mut self, name: &str, cmd: Box<dyn Command>);
    fn register_hook(&mut self, event: &str, hook: Box<dyn Hook>);
    fn logger(&self) -> &Logger;
}
```

**Rust 实现路径**：

| 阶段 | 实现 | crate |
|------|------|-------|
| 1. 静态链接 (.so/.dylib) | `libloading` | 0.8 |
| 2. WASM 沙箱 | `wasmtime` | 30.0 |
| 3. WASM 高级 API | `extism` | 1.0（提供 host function binding） |

**为什么 extism 而不是 wasmtime？**
- `extism` 提供 `Plugin::call()` 高层 API + host function 自动绑定
- 支持 WASM 插件的内存隔离
- 提供 manifest 格式（类似 OpenClawPluginApi）
- 社区活跃（2026 年 1.0 release）

#### 6.2.2 引入 marketplace（轻量版）

学习 atomcode：
- `$LAEW_HOME/marketplaces.json` 记录 marketplace URL
- `$LAEW_HOME/installed_plugins.json` 记录已装
- Bootstrap marker `$LAEW_HOME/.laew_bootstrap_v2`
- Git 拉取 marketplace（用 `gix` crate，已在第七轮建议）

**P1 预计工作量**：4-6 周（Extension API + WASM + marketplace）

### 6.3 P2（远期，1-3 月）

#### 6.3.1 完整 Plugin SDK

学习 openclaw 的 `definePluginEntry` 模式：
- `extension.toml` manifest（id/name/version/dependencies）
- typed events + typed hook signature
- permission system（filesystem/network/env）

#### 6.3.2 Marketplace 网站 + 发布管道

- HTTPS 下载（不用 git 拉整个 marketplace）
- 插件作者 OAuth 发布
- SHA-256 完整性校验 + **公钥签名**（建议用 `ed25519-dalek` crate）
- 版本约束（学习 openclaw `minHostVersion` + `compat.pluginApi`）

#### 6.3.3 Hook 内容寻址（学习 atomcode）

```rust
pub fn hook_set_hash(hooks: &[HookConfig]) -> String {
    let mut triples: Vec<(&str, &str, &str)> = hooks.iter()
        .map(|h| (h.event.as_str(), h.matcher.as_deref().unwrap_or(""), h.command.as_str()))
        .collect();
    triples.sort_unstable();
    let mut hasher = Sha256::new();
    for (event, matcher, command) in &triples {
        for field in [event, matcher, command] {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
    }
    format!("{:x}", hasher.finalize())
}
```

`hook_trust.json` 持久化用户对每个插件 hook 集合的信任决策。

### 6.4 laew 漏点清单（与第七轮 L16-L25 合并）

| ID | 缺口 | 优先级 | Rust crate | 参考工程 |
|----|------|--------|-----------|---------|
| L26 | 无 Hook 系统 | P0 | `schemars` | claudecode 27 events |
| L27 | 无 Extension API | P1 | `extism` / `wasmtime` | openclaw 50+ register |
| L28 | 无 Marketplace | P1 | `gix` + `reqwest` | atomcode bootstrap |
| L29 | 无 plugin signing | P2 | `ed25519-dalek` | (无参考，独自实现) |
| L30 | 无 hook trust hash | P2 | `sha2` | atomcode hook_trust.rs |
| L31 | 无 process sandbox | 不做 | ❌ | (7 工程均无) |
| L32 | 无 typed event schema | P1 | `schemars` + `serde` | pi 40+ events |

---

## 7. 附录：关键代码路径速查表

### 7.1 claudecode

| 内容 | 路径 | 行号 |
|------|------|------|
| Hook 解析 | `src/utils/settings/settings.ts` | 全 |
| 27 种 Hook 事件常量 | `src/utils/hooks/` | 各文件 |
| Hook 执行器 | `src/utils/hooks/exec.ts` | 全 |
| Async Hook Registry | `src/hooks/asyncRegistry.ts` | 全 |
| settings.json 合并 | `src/utils/settings/settingsCache.ts` | 全 |

### 7.2 openclaw

| 内容 | 路径 | 行号 |
|------|------|------|
| Plugin SDK 入口 | `src/plugin-sdk/plugin-entry.ts` | 1-260 |
| Plugin API 类型 | `src/plugins/plugin-api.types.ts` | 全 |
| Plugin 注册中心 | `src/plugins/registry-types.ts` | 467-621 |
| 加载器 | `src/plugins/loader.ts` | 27 行（入口） |
| 加载核心 | `src/plugins/loader-runtime-core.ts` | 67-316 |
| 激活 | `src/plugins/loader-shared.ts` | 全 |
| ClawHub 客户端 | `src/plugins/clawhub.ts` | 1546 行 |
| marketplace | `src/plugins/marketplace.ts` | 1377 行 |
| Hook 事件清单 | `src/plugins/hook-types.ts` | 99-141 |
| 插件 SDK AGENTS | `src/plugin-sdk/AGENTS.md` | 全 |
| 扩展边界规则 | `extensions/AGENTS.md` + `extensions/CLAUDE.md` | 27-47 |

### 7.3 opencode

| 内容 | 路径 | 行号 |
|------|------|------|
| Plugin API 表面 | `packages/plugin/src/index.ts` | 1-335 |
| Plugin 加载器 | `packages/opencode/src/plugin/loader.ts` | 1-237 |
| Trigger 服务 | `packages/opencode/src/plugin/index.ts` | 41-308 |
| Built-in 插件 | `packages/opencode/src/plugin/index.ts` | 67-86 |
| ToolDefinition | `packages/plugin/src/tool.ts` | 54 行 |
| Shell 命令 | `packages/plugin/src/shell.ts` | 136 行 |
| TUI 钩子 | `packages/plugin/src/tui.ts` | 634 行 |

### 7.4 pi

| 内容 | 路径 | 行号 |
|------|------|------|
| Extension API 类型 | `packages/coding-agent/src/core/extensions/types.ts` | 1-1798 |
| Extension loader | `packages/coding-agent/src/core/extensions/loader.ts` | 1-807 |
| Extension runner | `packages/coding-agent/src/core/extensions/runner.ts` | 全 |
| Wrapper | `packages/coding-agent/src/core/extensions/wrapper.ts` | 全 |
| Index re-exports | `packages/coding-agent/src/core/extensions/index.ts` | 1-194 |
| 示例 (TPS) | `.pi/extensions/tps.ts` | 45 行 |
| 示例 (Redraws) | `.pi/extensions/redraws.ts` | 25 行 |

### 7.5 deepseek-harness

| 内容 | 路径 | 行号 |
|------|------|------|
| Cordis 入口 | `vendor/cordis/src/index.ts` | 14 行 |
| RegistryService | `vendor/cordis/src/registry.ts` | 195-337 |
| Fiber 状态机 | `vendor/cordis/src/fiber.ts` | 147-200 |
| Service 基类 | `vendor/cordis/src/service.ts` | 11-115 |
| 事件总线 | `vendor/cordis/src/events.ts` | 352 行 |
| hooks-claude-code 适配器 | `packages/hooks/hooks-claude-code/src/index.ts` | 1-120+ |
| Config schema | `packages/hooks/hooks-claude-code/src/config.ts` | 全 |
| Hook 测试 | `packages/hooks/hooks-claude-code/tests/*.spec.ts` | 全 |

### 7.6 atomcode

| 内容 | 路径 | 行号 |
|------|------|------|
| Plugin 模块根 | `crates/atomcode-capabilities/src/plugin/mod.rs` | 1-57 |
| Manifest 类型 | `crates/atomcode-capabilities/src/plugin/manifest.rs` | 1-200+ |
| CC hooks 兼容 | `crates/atomcode-capabilities/src/plugin/loader.rs` | 67-100+ |
| Hook 内容哈希 | `crates/atomcode-capabilities/src/plugin/hook_trust.rs` | 1-80 |
| 加载器 | `crates/atomcode-capabilities/src/plugin/loader.rs` | 835 行 |
| 安装器 | `crates/atomcode-capabilities/src/plugin/installer.rs` | 1320 行 |
| Marketplace | `crates/atomcode-capabilities/src/plugin/marketplace.rs` | 924 行 |
| Bootstrap | `crates/atomcode-capabilities/src/plugin/bootstrap.rs` | 404 行 |
| TUI 集成 | `crates/atomcode-tuix/src/modals/plugin_manager.rs` | 全 |

### 7.7 undici

| 内容 | 路径 | 行号 |
|------|------|------|
| Dispatcher 基类 | `lib/dispatcher/dispatcher.js` | 1-54 |
| DispatcherBase | `lib/dispatcher/dispatcher-base.js` | 1-197 |
| Compose | `lib/dispatcher/dispatcher.js` | 18-51 |
| Retry 拦截器 | `lib/interceptor/retry.js` | 1-19 |
| DNS 拦截器 | `lib/interceptor/dns.js` | 1-575 |
| Redirect 拦截器 | `lib/interceptor/redirect.js` | 全 |
| Cache 拦截器 | `lib/interceptor/cache.js` | 全 |
| Decompress 拦截器 | `lib/interceptor/decompress.js` | 全 |
| Dedup 拦截器 | `lib/interceptor/deduplicate.js` | 全 |
| Dump 拦截器 | `lib/interceptor/dump.js` | 全 |
| ResponseError 拦截器 | `lib/interceptor/response-error.js` | 全 |
| Dispatcher API 文档 | `docs/docs/api/Dispatcher.md` | 全 |
| Interceptors API 文档 | `docs/docs/api/Interceptors.md` | 全 |

---

## 写在最后

**Hook + Extension 双层抽象**是 Agent CLI 的"必备基础设施"，但**实现深度差异巨大**：

- **claudecode** 在 Hook 层做到极致（27 事件 + 4 执行器），Extension 层几乎为零
- **openclaw** 在 Extension 层做到极致（50+ register + 153 个 bundled 扩展）
- **deepseek-harness** 通过 Cordis 把两者统一为"一切皆插件"
- **pi** 提供最精致的 Extension API + jiti 加载器，但**没有 marketplace**
- **opencode** 用 TypeScript 条件类型 + Effect 双参数签名做出**最严谨的 Hook 系统**
- **atomcode** 用 SHA-256 hook_trust 哈希做出**最严肃的 hook 治理**
- **undici** 的 `compose()` 链式拦截器是**最小可用的 hook 实现范式**

**laew 的当务之急**：
1. P0：实现 8 个最常用 Hook 事件 + settings.json 范式（学习 claudecode）
2. P1：实现 Extension API + WASM 沙箱（学习 openclaw + opencode）
3. P2：实现 marketplace + hook_trust 哈希签名（学习 atomcode + openclaw ClawHub）

**推荐技术栈**：
- Hook 决策 schema：`schemars`（已写入第七轮）
- WASM 沙箱：`extism`（高层 API）或 `wasmtime`（底层控制）
- Marketplace 下载：`gix` + `reqwest`
- 内容哈希：`sha2`
- 公钥签名：`ed25519-dalek`（远期 P2）

---

**字数**：本专题约 **2300 行 / 80000 字符**，覆盖 7 个工程 8 个维度。
**作者**：第八轮源码深挖 Subagent（Hook / 拦截器 / Plugin Extension API 方向）
**完成日期**：2026-09-07
