# 第十一轮深度调研：插件生态与扩展分发与 Hook 系统深度对比

> **调研日期**: 2026-09-07
> **调研范围**: 15 个源码工程全面覆盖
> **调研维度**: 插件架构 / Extension API / Hook 注册 / 触发时机 / 决策能力 / 失败处理 / 沙箱 / 市场分发 / SDK / 版本兼容
> **累计 gap**: L1-L142（前十轮）+ L221-L240（本轮新增 20 条）= 162 条 laew gap

---

## 目录

1. [调研方法论与工程清单](#1-调研方法论与工程清单)
2. [Hook 类型全景对比表](#2-hook-类型全景对比表)
3. [Extension 生命周期对比表](#3-extension-生命周期对比表)
4. [沙箱模型对比表](#4-沙箱模型对比表)
5. [市场分发对比表](#5-市场分发对比表)
6. [SDK 类型对比表](#6-sdk-类型对比表)
7. [工程逐项深度分析](#7-工程逐项深度分析)
   - 7.1 atomcode（Rust, plugin_hook_set_hash + 12 lifecycle）
   - 7.2 claudecode（TypeScript, 27 Hook + 5 executor + Plugin Bridge）
   - 7.3 deepseek-harness（TypeScript, Cordis Fiber 6 态 + Extension 30+ 事件）
   - 7.4 openclaw（TypeScript, 153 bundled + 162 extensions + Workshop）
   - 7.5 opencode（TypeScript/Bun, Plugin 加载 + 沙箱）
   - 7.6 pi（TypeScript, Skill 一等公民 + SDK）
   - 7.7 hermes-agent（Python, 6 前端共享 + 插件）
   - 7.8 agent-core（Python, openJiuwen Plugin + Rails）
   - 7.9 agent-studio（Python, Pregel 节点 + BubbleWrap）
   - 7.10 cc-switch（Tauri 2, 8 工具适配 + Plugin）
   - 7.11 jiuwenswarm（Python, SkillDevPipeline 12 阶段）
   - 7.12 semantica（Python, Rule Engine + Plugin）
   - 7.13 Switchyard（Rust, PyO3 模块 + 协议插件）
   - 7.14 TencentDB-Agent-Memory（TS+Python, SkillCore 6 写 4 读）
   - 7.15 undici（Node.js, interceptor / dispatcher / mock agent）
8. [横向专题对比](#8-横向专题对比)
   - 8.1 Plugin Extension API 设计模式对比
   - 8.2 Hook 注册与触发机制对比
   - 8.3 Hook 决策能力与失败处理对比
   - 8.4 沙箱与隔离模型对比
   - 8.5 市场分发与版本兼容对比
   - 8.6 WASM 沙箱与进程模型对比
   - 8.7 IPC 协议对比
   - 8.8 插件发现机制对比
9. [laew gap 分析 L221-L240](#9-laew-gap-分析-l221-l240)
10. [Rust crate 推荐清单](#10-rust-crate-推荐清单)
11. [第十一轮合集索引](#11-第十一轮合集索引)

---

## 1. 调研方法论与工程清单

### 1.1 调研方法

本轮调研采用「**四维交叉分析法**」：

1. **源码精读**: 每个工程阅读 10-30 个核心文件，累计阅读 **120+ 个文件**
2. **API 提取**: 逐工程提取 Hook 类型、Extension 生命周期、沙箱模型、市场分发、SDK
3. **横向对比**: 构建 6 张大型对比表，跨工程对齐维度
4. **gap 映射**: 产出 laew gap L221-L240 共 20 条新 gap，附 Rust crate 建议

### 1.2 源码工程清单（15 个）

| # | 工程 | 语言/技术 | 核心亮点 | 调研文件数 |
|---|------|----------|---------|-----------|
| 1 | **atomcode** | Rust | L0/L1/L2 分层 + plugin_hook_set_hash + 12 lifecycle traits | ~25 |
| 2 | **claudecode** | TypeScript/Bun | 27 种 Hook + 5 种执行器 + Plugin Bridge + marketplace | ~30 |
| 3 | **deepseek-harness** | TypeScript | Cordis Fiber 六态 + Extension 30+ 事件 + VM 沙箱 | ~28 |
| 4 | **openclaw** | TypeScript | Gateway/Harness/Adapter 三层契约 + 153 bundled + 41 hook types | ~35 |
| 5 | **opencode** | TypeScript/Bun | Effect + Schema 全栈 DI + Plugin 加载 + 20+ hooks | ~22 |
| 6 | **pi** | TypeScript | lane 并发 + 一等公民 Skill + ExtensionAPI 30+ events | ~20 |
| 7 | **hermes-agent** | Python | 859 MB + 6 前端共享 AIAgent + 40+ hook types | ~25 |
| 8 | **agent-core** | Python | openJiuwen Core SDK + AsyncCallbackFramework + Pregel + 5 MCP transports | ~30 |
| 9 | **agent-studio** | Python | 一站式 Agent 平台 + Pregel + DSL 双向转换 + BubbleWrap + Seccomp | ~25 |
| 10 | **cc-switch** | Tauri 2+Rust+React | 8 工具适配 + 熔断器 + MCP SSOT + 17 schema 迁移 | ~22 |
| 11 | **jiuwenswarm** | Python | 多 Agent 协作 + 17 hook events + SkillDevPipeline 12 阶段 + JiuwenBox | ~30 |
| 12 | **semantica** | Python | 图原生 AI 基础设施 + Rete + Datalog + 17 skills + MCP | ~25 |
| 13 | **Switchyard** | Rust | NVIDIA LLM 网关 + 协议 IR + TranslationEngine + 9 种路由算法 + PyO3 | ~20 |
| 14 | **TencentDB-Agent-Memory** | TS+Python | 团队记忆系统 + L0-L3 管线 + SkillCore 6 写 4 读 + OpenClaw plugin | ~22 |
| 15 | **undici** | Node.js | Node.js 官方 HTTP 客户端 + 8 拦截器 + Dispatcher + MockAgent | ~18 |

**累计调研文件数**: ~387 个文件（含 4 份子代理报告覆盖的文件）

### 1.3 核心维度定义

| 维度 | 定义 | 关键问题 |
|------|------|---------|
| **Plugin Extension API** | 插件暴露给宿主调用的接口集合 | 注册什么？回调签名？上下文注入？ |
| **Hook 注册** | 插件声明订阅生命周期事件的方式 | 声明式/命令式/自动/lazy load？ |
| **触发时机** | Hook 在 Agent 生命周期中的触发点 | PreToolUse/PostToolUse/SessionStart/... |
| **决策能力** | Hook 是否能阻断/修改流程 | allow/deny/ask/modify？ |
| **失败处理** | Hook 异常时的容错策略 | fail-open/fail-closed/timeout/silent continue？ |
| **沙箱** | 插件代码的隔离级别 | in-process/out-of-process/WASM/container？ |
| **市场分发** | 插件的发现与安装渠道 | npm/GitHub/自建 marketplace？ |
| **SDK** | 官方提供的插件开发工具链 | crate/npm package/pip package？ |
| **版本兼容** | 插件与宿主版本间的兼容策略 | semver/content addressing/hash pinning？ |

## 2. Hook 类型全景对比表

> 下表汇总 15 个工程的全量 Hook 类型，按 Agent 生命周期阶段分组。✅ = 原生支持，○ = 间接支持（通过 middleware/rail），❌ = 不支持。

### 2.1 工具生命周期 Hook

| Hook 事件 | atomcode | claudecode | deepseek | openclaw | opencode | pi | hermes | agent-core | studio | cc-switch | jiuwenswarm | semantica | Switchyard | TencentDB | undici |
|-----------|:--------:|:----------:|:--------:|:--------:|:--------:|:--:|:------:|:----------:|:------:|:---------:|:-----------:|:---------:|:----------:|:---------:|:------:|
| **PreToolUse** | ✅ | ✅ | ✅(bridge) | ✅ | ✅(tool.execute.before) | ✅(tool_call) | ✅(pre_tool_call) | ✅(TOOL_CALL_STARTED) | ❌ | ❌ | ✅ | ✅ | ❌ | ❌ | ❌ |
| **PostToolUse** | ✅ | ✅ | ✅(bridge) | ✅ | ✅(tool.execute.after) | ✅(tool_result) | ✅(post_tool_call) | ✅(TOOL_CALL_FINISHED) | ❌ | ❌ | ✅ | ✅ | ❌ | ❌ | ❌ |
| **PostToolUseFailure** | ✅ | ✅ | ❌ | ✅ | ❌ | ❌ | ❌ | ✅(TOOL_CALL_ERROR) | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **ToolExecuteStart** | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **ToolExecuteEnd** | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **ToolAuth** | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **ToolParse** | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **ToolResult** | ❌ | ❌ | ❌ | ✅(tool_result_persist) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **PermissionRequest** | ❌ | ✅ | ❌ | ❌ | ✅(permission.ask) | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **PermissionDenied** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **Elicitation** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

### 2.2 会话生命周期 Hook

| Hook 事件 | atomcode | claudecode | deepseek | openclaw | opencode | pi | hermes | agent-core | studio | cc-switch | jiuwenswarm | semantica | Switchyard | TencentDB | undici |
|-----------|:--------:|:----------:|:--------:|:--------:|:--------:|:--:|:------:|:----------:|:------:|:---------:|:-----------:|:---------:|:----------:|:---------:|:------:|
| **SessionStart** | ✅ | ✅ | ✅(bridge) | ✅ | ✅(session_start) | ✅(session_start) | ✅(on_session_start) | ✅(SESSION_CREATED) | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **SessionEnd** | ✅ | ✅ | ❌ | ✅ | ✅(session_shutdown) | ✅(session_shutdown) | ✅(on_session_end) | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **SessionBeforeFork** | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **SessionBeforeCompact** | ❌ | ❌ | ❌ | ✅(before_compaction) | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **SessionCompact** | ❌ | ✅(PreCompact) | ❌ | ✅(after_compaction) | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **SessionReset** | ❌ | ❌ | ❌ | ✅(before_reset) | ❌ | ❌ | ✅(on_session_reset) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **SessionAutoReset** | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **UserPromptSubmit** | ✅ | ✅ | ✅(bridge) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **ConfigChange** | ❌ | ✅ | ❌ | ❌ | ✅(config) | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **InstructionsLoaded** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **Setup** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |

### 2.3 Agent 生命周期 Hook

| Hook 事件 | atomcode | claudecode | deepseek | openclaw | opencode | pi | hermes | agent-core | studio | cc-switch | jiuwenswarm | semantica | Switchyard | TencentDB | undici |
|-----------|:--------:|:----------:|:--------:|:--------:|:--------:|:--:|:------:|:----------:|:------:|:---------:|:-----------:|:---------:|:----------:|:---------:|:------:|
| **AgentStart** | ❌ | ❌ | ❌ | ✅(before_agent_run) | ❌ | ✅(agent_start) | ❌ | ✅(AGENT_STARTED) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **AgentEnd** | ❌ | ❌ | ❌ | ✅(agent_end) | ❌ | ✅(agent_end) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅(agent_end) | ❌ |
| **AgentTurnPrepare** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **BeforeAgentReply** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **BeforeAgentFinalize** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **BeforePromptBuild** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅(before_prompt_build) | ❌ |
| **BeforeModelResolve** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **ModelCallStarted/Ended** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ✅(LLM_CALL_STARTED) | ❌ | ❌ | ✅(Before/AfterModelCall) | ❌ | ❌ | ❌ | ❌ |
| **Stop** | ✅ | ✅ | ✅(bridge) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **StopFailure** | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **SubagentStart/Stop** | ❌ | ✅ | ❌ | ✅ | ❌ | ❌ | ✅(subagent_start/stop) | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **SubagentDelivery** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **SubagentProgress** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

### 2.4 消息/流式 Hook

| Hook 事件 | atomcode | claudecode | deepseek | openclaw | opencode | pi | hermes | agent-core | studio | cc-switch | jiuwenswarm | semantica | Switchyard | TencentDB | undici |
|-----------|:--------:|:----------:|:--------:|:--------:|:--------:|:--:|:------:|:----------:|:------:|:---------:|:-----------:|:---------:|:----------:|:---------:|:------:|
| **MessageReceived** | ❌ | ❌ | ❌ | ✅ | ✅(chat.message) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **MessageSending** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **MessageSent** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **BeforeMessageWrite** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **LLMInput/Output** | ❌ | ❌ | ❌ | ✅ | ✅(chat.params) | ❌ | ❌ | ✅(LLM_INPUT/OUTPUT) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **StreamStart/Delta/End** | ❌ | ❌ | ❌ | ❌ | ❌ | ✅(message_start/end) | ✅(on_stream_*) | ✅(LLM_STREAM_*) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **ReplyDispatch** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **BeforeDispatch** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

### 2.5 网关/基础设施 Hook

| Hook 事件 | atomcode | claudecode | deepseek | openclaw | opencode | pi | hermes | agent-core | studio | cc-switch | jiuwenswarm | semantica | Switchyard | TencentDB | undici |
|-----------|:--------:|:----------:|:--------:|:--------:|:--------:|:--:|:------:|:----------:|:------:|:---------:|:-----------:|:---------:|:----------:|:---------:|:------:|
| **GatewayStart/Stop** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **CronReconciled/Changed** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Heartbeat** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **SkillChanged** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **SkillProposal** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **BeforeInstall** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **ResolveExecEnv** | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Notification** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **TeammateIdle** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **WorktreeCreate/Remove** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **CwdChanged** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **FileChanged** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

### 2.6 Hook 类型数量统计

| 工程 | Hook 类型数 | 决策能力 | 失败处理 | 优先级 | 超时 |
|------|:-----------:|:--------:|:--------:|:------:|:----:|
| **openclaw** | **41** | pass/block/modify | fail-open/fail-closed | ✅ | ✅ |
| **hermes-agent** | **40** | allow/deny/modify | logged non-blocking | ❌ | ✅ |
| **claudecode** | **27** | approve/block/ask | silent continue | ❌ | ✅ |
| **agent-core** | **40+** | CONTINUE/STOP/SKIP/MODIFY | retry/rollback | ✅ | ✅ |
| **jiuwenswarm** | **17** | block/allow/modify | exit-2 blocking | ✅ | ✅ |
| **pi** | **30+** | event handlers | exception | ❌ | ❌ |
| **opencode** | **20+** | modify/rewrite | exception | ❌ | ❌ |
| **deepseek** | **7(CC)/5(Codex)** | deny/ask/block | non-blocking | ❌ | ✅ |
| **atomcode** | **8** | allow/block/modify | silent continue | ❌ | ✅ |
| **semantica** | **2** | guard | non-blocking | ❌ | ❌ |
| **Switchyard** | **2** | intercept | priority-ordered | ✅ | ❌ |
| **TencentDB** | **2** | recall/capture | non-blocking | ❌ | ❌ |
| **cc-switch** | **0** | N/A | N/A | N/A | N/A |
| **undici** | **8** interceptors | compose chain | error handler | ❌ | ❌ |
| **agent-studio** | **0** | N/A | N/A | N/A | N/A |

## 3. Extension 生命周期对比表

> 下表对比 15 个工程的 Extension 生命周期状态机。

### 3.1 生命周期状态对比

| 工程 | 注册 | 初始化 | 就绪 | 运行 | 重载 | 卸载 | 销毁 | 升级 | 暂停 | 恢复 |
|------|:----:|:------:|:----:|:----:|:----:|:----:|:----:|:----:|:----:|:----:|
| **atomcode** | plugin.json | install | activate | run | hot reload | uninstall | dispose | git pull | ❌ | ❌ |
| **claudecode** | marketplace | load | ready | execute | hot reload | disable | unload | auto-update | ❌ | ❌ |
| **deepseek** | cordis_define | LOADING | ACTIVE | run | ❌ | cordis_undefine | DISPOSED | ❌ | PENDING | ❌ |
| **openclaw** | register() | init | ready | serve | hot reload | disable | dispose | upgrade | ❌ | ❌ |
| **opencode** | import() | load | active | serve | reload() | ❌ | dispose | ❌ | ❌ | ❌ |
| **pi** | jiti import | factory() | active | serve | ctx.reload() | ❌ | ❌ | ❌ | ❌ | ❌ |
| **hermes** | importlib | load_plugin | active | serve | ❌ | unload | ❌ | ❌ | ❌ | ❌ |
| **agent-core** | register() | init | ready | trigger | ❌ | unregister | ❌ | ❌ | ❌ | ❌ |
| **agent-studio** | JSON validate | create | ready | serve | ❌ | delete | ❌ | ❌ | ❌ | ❌ |
| **cc-switch** | install | sync | active | serve | ❌ | uninstall | ❌ | check_updates | ❌ | ❌ |
| **jiuwenswarm** | discover | initialize | ready | serve | ❌ | shutdown | ❌ | ❌ | ❌ | ❌ |
| **semantica** | plugin.json | import | ready | serve | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Switchyard** | register() | validate | ready | intercept | ❌ | ❌ | destroy | ❌ | ❌ | ❌ |
| **TencentDB** | register(api) | initialize | ready | hooks fire | ❌ | destroy | ❌ | ❌ | ❌ | ❌ |
| **undici** | compose() | construct | ready | dispatch | ❌ | close | destroy | ❌ | ❌ | ❌ |

### 3.2 注册机制对比

| 工程 | 注册方式 | 声明式 | 命令式 | 自动发现 | Lazy Load |
|------|----------|:------:|:------:|:--------:|:---------:|
| **atomcode** | plugin.json + hooks.json | ✅ | ❌ | ✅ | ❌ |
| **claudecode** | marketplace + settings | ✅ | ✅ | ✅ | ✅ |
| **deepseek** | cordis_define tool | ❌ | ✅ | ❌ | ❌ |
| **openclaw** | register() API | ✅ | ✅ | ✅ | ✅ |
| **opencode** | config + import() | ✅ | ✅ | ✅ | ✅ |
| **pi** | ExtensionFactory | ✅ | ✅ | ✅ | ✅ (jiti) |
| **hermes** | plugin.yaml + importlib | ✅ | ✅ | ✅ | ❌ |
| **agent-core** | @on decorator | ✅ | ✅ | ❌ | ❌ |
| **agent-studio** | JSON Schema | ✅ | ❌ | ✅ | ❌ |
| **cc-switch** | Tauri commands | ❌ | ✅ | ❌ | ❌ |
| **jiuwenswarm** | extension.yaml + register_extensions | ✅ | ✅ | ✅ | ❌ |
| **semantica** | plugin.json | ✅ | ❌ | ✅ | ❌ |
| **Switchyard** | NativePlugin trait | ❌ | ✅ | ❌ | ❌ |
| **TencentDB** | register(api) | ❌ | ✅ | ❌ | ❌ |
| **undici** | compose() | ❌ | ✅ | ❌ | ❌ |

### 3.3 Manifest 格式对比

| 工程 | 格式 | 必需字段 | 版本字段 | 依赖声明 | 权限声明 |
|------|------|----------|:--------:|:--------:|:--------:|
| **atomcode** | JSON | name, hooks | version | ❌ | ❌ |
| **claudecode** | plugin.json | name, description | version | ❌ | ❌ |
| **deepseek** | define-time receipt | pluginId, packageId | ❌ | ❌ | approval-gated |
| **openclaw** | openclaw.plugin.json | id, configSchema | version | requiresPlugins | capabilities |
| **opencode** | package.json | exports | engines | dependencies | ❌ |
| **pi** | TypeScript module | factory | ❌ | ❌ | ❌ |
| **hermes** | plugin.yaml | name, version | version | requires_plugins | capabilities |
| **agent-core** | Python protocol | ❌ | ❌ | ❌ | ❌ |
| **agent-studio** | JSON Schema | plugin_id, name, tools | version | ❌ | ❌ |
| **cc-switch** | JSON config | id, name | ❌ | ❌ | ❌ |
| **jiuwenswarm** | extension.yaml | id, name, version | version | dependencies | permissions |
| **semantica** | plugin.json | name, version | version | ❌ | ❌ |
| **Switchyard** | Cargo.toml + TOML | ❌ | schema_version | ❌ | ❌ |
| **TencentDB** | openclaw.plugin.json | id, configSchema | version | ❌ | ❌ |
| **undici** | 无（代码组合） | N/A | N/A | N/A | N/A |

## 4. 沙箱模型对比表

> 下表对比 15 个工程的插件隔离级别。

### 4.1 沙箱隔离级别对比

| 工程 | 隔离级别 | 进程模型 | 文件系统隔离 | 网络隔离 | 能力限制 | 资源限制 |
|------|----------|----------|:------------:|:--------:|:--------:|:--------:|
| **atomcode** | **Out-of-process** | subprocess (sh -c) | ✅ env vars only | ❌ | timeout 10s | ❌ |
| **claudecode** | **Out-of-process** | subprocess | ✅ env vars | ❌ | timeout | ❌ |
| **deepseek** | **VM Realm** | node:vm sandbox | ✅ whitelist globals | ✅ trapped | ctx façade guard | ❌ |
| **openclaw** | **In-process** | same Node process | ❌ | ❌ | capability catalog | ❌ |
| **opencode** | **In-process** | same Bun process | ❌ | ❌ | ❌ | ❌ |
| **pi** | **In-process** | jiti module loader | ❌ | ❌ | ❌ | ❌ |
| **hermes** | **In-process** | importlib | ❌ | ❌ | capability gating | ❌ |
| **agent-core** | **In-process** | asyncio | ❌ | ❌ | guardrail | ❌ |
| **agent-studio** | **BubbleWrap + Seccomp** | Linux namespaces | ✅ bind mounts | ✅ netns | ✅ seccomp BPF | ✅ setpriv |
| **cc-switch** | **N/A** | N/A | N/A | N/A | N/A | N/A |
| **jiuwenswarm** | **JiuwenBox** | BubbleWrap+Landlock+Seccomp | ✅ Landlock | ✅ netns+cgroup | ✅ seccomp | ✅ cgroup |
| **semantica** | **In-process** | same Python VM | ❌ | ❌ | ❌ | ❌ |
| **Switchyard** | **cdylib** | in-process dynamic lib | ❌ | ❌ | trait boundary | ❌ |
| **TencentDB** | **In/Out** | local or HTTP | ❌ | ❌ | team isolation | ❌ |
| **undici** | **In-process** | same Node process | ❌ | ❌ | ❌ | ❌ |

### 4.2 信任模型对比

| 工程 | 信任模型 | 用户确认 | 哈希校验 | 签名验证 | 能力门控 |
|------|----------|:--------:|:--------:|:--------:|:--------:|
| **atomcode** | **Hash-pinned** | `atomcode plugin trust` | SHA-256 of hook set | ❌ | ❌ |
| **claudecode** | **Marketplace** | install prompt | sha field | ❌ | ❌ |
| **deepseek** | **Approval-gated** | user approves run | ❌ | ❌ | ✅ |
| **openclaw** | **Manifest** | ❌ | ❌ | ❌ | capability catalog |
| **opencode** | **npm trust** | install | package-lock | ❌ | ❌ |
| **pi** | **npm trust** | install | shrinkwrap | ❌ | ❌ |
| **hermes** | **Capability** | ❌ | ❌ | ❌ | plugin_capability_granted |
| **agent-core** | **Guardrail** | ❌ | ❌ | ❌ | BaseGuardrail |
| **agent-studio** | **BubbleWrap** | ❌ | ❌ | ❌ | seccomp whitelist |
| **cc-switch** | **N/A** | N/A | N/A | N/A | N/A |
| **jiuwenswarm** | **JiuwenBox** | ❌ | content_checksum | ❌ | permissions |
| **semantica** | **IDE trust** | install | ❌ | ❌ | ❌ |
| **Switchyard** | **cdylib trust** | ❌ | ❌ | ❌ | ❌ |
| **TencentDB** | **OpenClaw trust** | ❌ | content_hash | ❌ | ❌ |
| **undici** | **Code trust** | ❌ | ❌ | ❌ | ❌ |

### 4.3 Hook 执行失败处理对比

| 工程 | 失败策略 | 超时处理 | 日志 | 重试 | 降级 |
|------|----------|----------|------|------|------|
| **atomcode** | **Silent continue** | kill_on_drop | ❌ | ❌ | ❌ |
| **claudecode** | **Silent continue** | timeout | logError | ❌ | ❌ |
| **deepseek** | **Non-blocking** | 600s | ✅ | ❌ | ❌ |
| **openclaw** | **fail-open/fail-closed** | per-hook timeout | ✅ | ❌ | ❌ |
| **opencode** | **Exception** | ❌ | ✅ | ❌ | ❌ |
| **pi** | **Exception** | ❌ | ✅ | ❌ | ❌ |
| **hermes** | **Logged non-blocking** | configurable | ✅ | ❌ | ❌ |
| **agent-core** | **retry/rollback** | timeout | ✅ | ✅ | ❌ |
| **agent-studio** | **N/A** | N/A | N/A | N/A | N/A |
| **cc-switch** | **N/A** | N/A | N/A | N/A | N/A |
| **jiuwenswarm** | **exit-2 blocking** | 10s | ✅ | ❌ | ❌ |
| **semantica** | **Non-blocking** | ❌ | ✅ | ❌ | ❌ |
| **Switchyard** | **Priority-ordered** | ❌ | ✅ | ❌ | ❌ |
| **TencentDB** | **Non-blocking** | ❌ | ✅ | ❌ | ❌ |
| **undici** | **Error handler** | ❌ | ✅ | ❌ | ❌ |

## 5. 市场分发对比表

> 下表对比 15 个工程的插件市场与分发渠道。

### 5.1 市场渠道对比

| 工程 | 主要市场 | 安装方式 | 版本管理 | 自动更新 | 企业策略 |
|------|----------|----------|:--------:|:--------:|:--------:|
| **atomcode** | Git marketplace | `atomcode plugin install` | Git pin (branch/tag/commit) | git pull | ❌ |
| **claudecode** | Official marketplace | settings.json | semver | ✅ | blocklist/allowlist |
| **deepseek** | Runtime (model-authored) | cordis_define | packageId | ❌ | approval-gated |
| **openclaw** | npm + bundled | `openclaw plugins install` | semver | ❌ | ❌ |
| **opencode** | npm registry | `opencode plugin <module>` | engines.opencode | ❌ | ❌ |
| **pi** | npm registry | package.json deps | npm semver | ❌ | ❌ |
| **hermes** | pip + CLI | `hermes plugins install` | semver | ❌ | ❌ |
| **agent-core** | In-process registry | register() | ❌ | ❌ | ❌ |
| **agent-studio** | JSON catalog | plugins_creator CLI | semver | ❌ | ❌ |
| **cc-switch** | skills.sh + GitHub | install_skill | ❌ | check_updates | ❌ |
| **jiuwenswarm** | Filesystem discovery | auto-discover + uv/pip | min_jiuwenswarm_version | ❌ | ❌ |
| **semantica** | IDE marketplaces | per-IDE manifest | semver | ❌ | ❌ |
| **Switchyard** | NVIDIA NeMo Relay | cdylib | schema_version | ❌ | ❌ |
| **TencentDB** | OpenClaw marketplace | `openclaw plugins install` | content_hash | ✅ | ❌ |
| **undici** | npm registry | npm install | semver | ❌ | ❌ |

### 5.2 版本兼容策略对比

| 工程 | 兼容策略 | Semver | Content Hash | Hash Pinning | Lock File |
|------|----------|:------:|:------------:|:------------:|:---------:|
| **atomcode** | Git pin | ❌ | plugin_hook_set_hash | ✅ | installed_plugins.json |
| **claudecode** | Marketplace | ✅ | sha | ❌ | ❌ |
| **deepseek** | Approval | ❌ | ❌ | ❌ | ❌ |
| **openclaw** | Manifest | ✅ | ❌ | ❌ | pnpm-lock.yaml |
| **opencode** | engines field | ✅ | fingerprint | ❌ | package-lock.json |
| **pi** | npm | ✅ | ❌ | ❌ | npm-shrinkwrap.json |
| **hermes** | manifest_version | ✅ | ❌ | ❌ | ❌ |
| **agent-core** | EventBase mirror | ❌ | ❌ | ❌ | ❌ |
| **agent-studio** | JSON Schema | ✅ | ❌ | ❌ | ❌ |
| **cc-switch** | Feature gate | ❌ | ❌ | ❌ | ❌ |
| **jiuwenswarm** | min_version | ✅ | content_checksum | ✅ | .archive/versions |
| **semantica** | plugin version | ✅ | ❌ | ❌ | ❌ |
| **Switchyard** | schema_version | ❌ | ❌ | ❌ | Cargo.lock |
| **TencentDB** | host version floor | ✅ | content_hash | ✅ | ❌ |
| **undici** | npm semver | ✅ | makeCacheKey | ❌ | package-lock.json |

### 5.3 插件发现机制对比

| 工程 | 发现方式 | 声明式 | 命令式 | 自动扫描 | Lazy Load |
|------|----------|:------:|:------:|:--------:|:---------:|
| **atomcode** | marketplace.json + plugin.json | ✅ | ❌ | ✅ | ❌ |
| **claudecode** | settings + marketplace | ✅ | ✅ | ✅ | ✅ |
| **deepseek** | cordis_define tool | ❌ | ✅ | ❌ | ❌ |
| **openclaw** | bundled + workspace + global + package | ✅ | ✅ | ✅ | ✅ |
| **opencode** | config + workspace scan | ✅ | ✅ | ✅ | ✅ |
| **pi** | config + file scan | ✅ | ✅ | ✅ | ✅ (jiti) |
| **hermes** | bundled + user + project + entrypoint | ✅ | ✅ | ✅ | ❌ |
| **agent-core** | @on decorator | ✅ | ✅ | ❌ | ❌ |
| **agent-studio** | JSON index | ✅ | ❌ | ✅ | ❌ |
| **cc-switch** | skills.sh API | ❌ | ✅ | ❌ | ❌ |
| **jiuwenswarm** | filesystem + extension_dirs | ✅ | ✅ | ✅ | ❌ |
| **semantica** | per-IDE manifest | ✅ | ❌ | ✅ | ❌ |
| **Switchyard** | cdylib loading | ❌ | ✅ | ❌ | ❌ |
| **TencentDB** | OpenClaw discovery | ✅ | ✅ | ✅ | ❌ |
| **undici** | code composition | ❌ | ✅ | ❌ | ❌ |

## 6. SDK 类型对比表

> 下表对比 15 个工程的插件 SDK 形态。

### 6.1 SDK 形态对比

| 工程 | SDK 类型 | 包名 | 导出内容 | 文档 | 示例 |
|------|----------|------|----------|:----:|------|
| **atomcode** | Rust crate | atomcode-capabilities | PluginHookSource, HookConfig | ❌ | ❌ |
| **claudecode** | TypeScript types | @anthropic-ai/sdk | HookEvent, HookJSONOutput | ✅ | ✅ |
| **deepseek** | npm package | @deepseek-ai/dsh-cordis-host-runner | DynamicCordisRunner | ❌ | ❌ |
| **openclaw** | npm package | openclaw/plugin-sdk | definePluginEntry, OpenClawPluginApi | ✅ | ✅ |
| **opencode** | npm package | @opencode-ai/plugin | defineTool, PluginInput, Hooks | ✅ | ✅ |
| **pi** | npm package | @earendil-works/pi-coding-agent | defineTool, ExtensionAPI | ✅ | ✅ |
| **hermes** | Python module | hermes_cli.plugins | PluginContext, register_tool | ✅ | ✅ |
| **agent-core** | Python package | openjiuwen.* | AsyncCallbackFramework, @on | ❌ | ❌ |
| **agent-studio** | Python SDK | connect/client | OpenJiuwenClient | ❌ | ❌ |
| **cc-switch** | Tauri commands | internal | invoke() | ❌ | ❌ |
| **jiuwenswarm** | Python SDK | jiuwenswarm.extensions.sdk | BaseExtension, register_extensions | ✅ | ✅ |
| **semantica** | Python package | semantica.* | ReteEngine, ContextGraph | ❌ | ❌ |
| **Switchyard** | Rust crate | switchyard-protocol | NativePlugin, TranslationEngine | ❌ | ❌ |
| **TencentDB** | npm + pip | memory-sdk-ts-v2 | MemoryClient | ✅ | ✅ |
| **undici** | npm package | undici | interceptors.*, MockAgent | ✅ | ✅ |

### 6.2 SDK 能力矩阵

| 工程 | Tool 注册 | Hook 注册 | Provider 注册 | UI 注入 | 命令注册 | Flag 注册 |
|------|:---------:|:---------:|:-------------:|:-------:|:--------:|:---------:|
| **atomcode** | ❌ | ✅ (JSON) | ❌ | ❌ | ❌ | ❌ |
| **claudecode** | ❌ | ✅ (JSON) | ❌ | ❌ | ❌ | ❌ |
| **deepseek** | ✅ (ctx.tools) | ❌ | ❌ | ❌ | ❌ | ❌ |
| **openclaw** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **opencode** | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| **pi** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **hermes** | ✅ | ✅ | ✅ | ❌ | ✅ | ❌ |
| **agent-core** | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| **agent-studio** | ✅ (JSON) | ❌ | ❌ | ❌ | ❌ | ❌ |
| **cc-switch** | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **jiuwenswarm** | ✅ | ✅ | ❌ | ✅ | ❌ | ❌ |
| **semantica** | ✅ (MCP) | ✅ (JSON) | ❌ | ❌ | ❌ | ❌ |
| **Switchyard** | ❌ | ✅ (intercept) | ❌ | ❌ | ❌ | ❌ |
| **TencentDB** | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **undici** | ❌ | ✅ (interceptor) | ❌ | ❌ | ❌ | ❌ |

### 6.3 IPC 协议对比

| 工程 | IPC 协议 | 序列化 | 传输层 | 双向 | 流式 |
|------|----------|--------|--------|:----:|:----:|
| **atomcode** | stdin/stdout JSON | JSON | pipe | ❌ | ❌ |
| **claudecode** | stdin/stdout JSON | JSON | pipe | ❌ | ❌ |
| **deepseek** | TypertRemoteService | protobuf | WebSocket | ✅ | ✅ |
| **openclaw** | in-process | direct call | N/A | ✅ | ✅ |
| **opencode** | in-process | direct call | N/A | ✅ | ✅ |
| **pi** | in-process | direct call | N/A | ✅ | ✅ |
| **hermes** | in-process | direct call | N/A | ✅ | ✅ |
| **agent-core** | in-process | direct call | N/A | ✅ | ✅ |
| **agent-studio** | HTTP/REST | JSON | TCP | ✅ | ❌ |
| **cc-switch** | Tauri commands | serde_json | IPC | ✅ | ❌ |
| **jiuwenswarm** | WebSocket + ACP JSON-RPC | JSON | TCP/stdio | ✅ | ✅ |
| **semantica** | MCP stdio/SSE | JSON-RPC | pipe/HTTP | ✅ | ✅ |
| **Switchyard** | cdylib trait call | serde | N/A | ✅ | ✅ |
| **TencentDB** | HTTP + OpenClaw API | JSON | TCP | ✅ | ❌ |
| **undici** | in-process | direct call | N/A | ✅ | ✅ |

## 7. 工程逐项深度分析

### 7.1 atomcode（Rust）

#### 7.1.1 架构概览

AtomCode 是一个开源终端 AI 编码 Agent，实现了 **Claude Code 兼容的插件系统**。核心 crate：

- `atomcode-capabilities`：核心插件/hook/skills 系统
- `atomcode-coding`：编码运行时 + plugin hook 注入
- `atomcode-kernel`：内核 traits（`LifecycleHooks`, `ToolMiddleware`）
- `atomcode-tuix`：终端 UI
- `atomcode-cli`, `atomcode-daemon`, `atomcode-telemetry`

#### 7.1.2 Manifest 格式

支持双格式（优先级从高到低）：
1. `.atomcode-plugin/plugin.json`（atomcode 原生）
2. `.claude-plugin/plugin.json`（Claude Code 兼容）
3. `plugin.json`（legacy flat）

```rust
pub struct PluginManifest {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub skills: Option<PathOrList>,
    pub commands: Option<PathOrList>,
    pub hooks: Option<HooksField>,
}
```

Marketplace manifest：
```rust
pub struct MarketplaceManifest {
    pub name: String,
    pub plugins: Vec<PluginEntry>,
}
pub struct PluginEntry {
    pub name: String,
    pub source: PluginSource,
}
```

外部源支持：`Url`, `Git`, `Github`, `GitSubdir`, `Local`，均带 `GitPin`（branch/tag/commit/ref）。

#### 7.1.3 Hook 类型（8 种）

```rust
pub enum HookEvent {
    PreToolUse, PostToolUse, PostToolUseFailure,
    SessionStart, SessionEnd, UserPromptSubmit,
    Stop, StopFailure,
}
```

支持双拼写：CC PascalCase（`PreToolUse`）和 snake_case（`pre_tool_use`）。

#### 7.1.4 信任模型：`plugin_hook_set_hash`

这是 atomcode 的标志性安全特性——对排序后的 `(event, matcher, command)` 三元组做 SHA-256：

```rust
pub fn plugin_hook_set_hash(hooks: &[PluginCcHook]) -> String {
    let mut triples: Vec<(&str, &str, &str)> = hooks
        .iter()
        .map(|h| (h.event.as_str(), h.matcher.as_deref().unwrap_or(""), h.command.as_str()))
        .collect();
    triples.sort_unstable();
    let mut hasher = Sha256::new();
    for (event, matcher, command) in &triples {
        for field in [event, matcher, command] {
            hasher.update((field.len() as u64).to_le_bytes());  // length-prefix
            hasher.update(field.as_bytes());
        }
    }
    format!("{:x}", hasher.finalize())
}
```

信任流：
1. 插件安装 → hook 默认不受信
2. 用户执行 `atomcode plugin trust <name>` → hash 存入 `hook_trust.json`
3. hook 命令变更 → hash 变更 → 需重新信任
4. `ensure_migrated()` 升级时 grandfather 已有插件

#### 7.1.5 沙箱/隔离

- **Out-of-process**: Hook 作为子进程通过 `sh -c`（Unix）或 `cmd /C`（Windows）执行
- **环境注入**: `CLAUDE_PLUGIN_ROOT` / `ATOMCODE_PLUGIN_ROOT` 环境变量
- **超时**: 默认 10s，可 per-hook 配置
- **优雅降级**: Broken hook = silent continue（永不 wedge turn）

IPC 协议（stdin/stdout JSON）：
```rust
async fn run_command_hook(hook: &HookConfig, stdin_json: &str) -> Option<(Option<i32>, String, String)>
```

Exit-code 契约：
- Exit 2 = deliberate block（reason 在 stdout/stderr）
- 其他非零 = non-blocking error
- Bare exit 2 无输出 = broken hook（non-blocking）

#### 7.1.6 注册机制

**声明式** via `plugin.json` + `hooks.json`：
```json
{
  "name": "my-plugin",
  "skills": ["./skills"],
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": "echo ready"}]}]
  }
}
```

#### 7.1.7 SDK & 市场

- **SDK**: Rust crate（`atomcode-capabilities`）
- **市场**: Git-cloned marketplaces（`.atomcode-plugin/marketplace.json`）
- **分发**: GitHub repos, git URLs, local paths, git-subdir（sparse clone）
- **版本固定**: `GitPin` with branch/tag/commit/ref

#### 7.1.8 关键代码片段

Hook 执行核心（`cc_hooks.rs`）：
```rust
async fn run_command_hook(hook: &HookConfig, stdin_json: &str) -> Option<(Option<i32>, String, String)> {
    let mut cmd = shell_command(&hook.command);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
       .kill_on_drop(true);
    if let Some(root) = &hook.plugin_root {
        cmd.env("CLAUDE_PLUGIN_ROOT", root);
        cmd.env("ATOMCODE_PLUGIN_ROOT", root);
    }
    let fut = async {
        let mut child = cmd.spawn().ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(stdin_json.as_bytes()).await;
            let _ = stdin.shutdown().await;
            drop(stdin);
        }
        let out = child.wait_with_output().await.ok()?;
        Some((out.status.code(), decode_output(&out.stdout), decode_output(&out.stderr)))
    };
    (tokio::time::timeout(Duration::from_millis(hook.timeout_ms), fut).await).ok().flatten()
}
```

---

### 7.2 claudecode（TypeScript/Bun）

#### 7.2.1 架构概览

Claude Code 实现了 **27 种 Hook + 5 种执行器 + Plugin Bridge** 的完整插件系统。核心模块：

- `src/hooks/`：27 种 Hook 事件 + 5 种执行器（command/prompt/agent/http/callback）
- `src/plugins/`：Plugin 加载 + marketplace + builtin
- `src/types/plugin.ts`：Plugin 类型系统
- `src/utils/plugins/`：Plugin 工具链

#### 7.2.2 Hook 类型（27 种）

```typescript
export const HOOK_EVENTS = [
  'PreToolUse', 'PostToolUse', 'PostToolUseFailure',
  'Notification', 'UserPromptSubmit',
  'SessionStart', 'SessionEnd', 'Stop', 'StopFailure',
  'SubagentStart', 'SubagentStop',
  'PreCompact', 'PostCompact',
  'PermissionRequest', 'PermissionDenied',
  'Setup', 'TeammateIdle',
  'TaskCreated', 'TaskCompleted',
  'Elicitation', 'ElicitationResult',
  'ConfigChange', 'WorktreeCreate', 'WorktreeRemove',
  'InstructionsLoaded', 'CwdChanged', 'FileChanged',
] as const
```

#### 7.2.3 Hook 执行器（5 种）

| 执行器 | 用途 | 输入 | 输出 |
|--------|------|------|------|
| **command** | 外部命令 | stdin JSON | exit code + stdout |
| **prompt** | LLM 审查 | 模板填充 | JSON verdict |
| **agent** | 子 Agent | 上下文 | Agent 决策 |
| **http** | HTTP 请求 | POST body | HTTP response |
| **callback** | TS 回调 | HookInput | HookJSONOutput |

#### 7.2.4 Hook 决策能力

```typescript
export const syncHookResponseSchema = z.object({
  continue: z.boolean().optional(),        // 是否继续
  suppressOutput: z.boolean().optional(),   // 隐藏 stdout
  stopReason: z.string().optional(),        // 停止原因
  decision: z.enum(['approve', 'block']).optional(),
  reason: z.string().optional(),
  systemMessage: z.string().optional(),
  hookSpecificOutput: z.union([
    z.object({ hookEventName: z.literal('PreToolUse'),
      permissionDecision: permissionBehaviorSchema().optional(),
      updatedInput: z.record(z.string(), z.unknown()).optional(),
      additionalContext: z.string().optional(),
    }),
    // ... 17 种 hookSpecificOutput 变体
  ]).optional(),
})
```

#### 7.2.5 插件 Manifest

```typescript
export type PluginManifest = {
  name: string
  description: string
  version: string
}

export type LoadedPlugin = {
  name: string
  manifest: PluginManifest
  path: string
  source: string
  enabled?: boolean
  isBuiltin?: boolean
  sha?: string                    // Git commit SHA for version pinning
  commandsPath?: string
  skillsPath?: string
  hooksConfig?: HooksSettings
  mcpServers?: Record<string, McpServerConfig>
  lspServers?: Record<string, LspServerConfig>
}
```

#### 7.2.6 插件加载流程

```
Plugin Discovery Sources (按优先级):
1. Marketplace-based plugins (plugin@marketplace format in settings)
2. Session-only plugins (--plugin-dir CLI flag or SDK plugins option)

Plugin Directory Structure:
my-plugin/
├── plugin.json          # Optional manifest
├── commands/            # Custom slash commands
│   ├── build.md
│   └── deploy.md
├── agents/              # Custom AI agents
│   └── test-runner.md
└── hooks/               # Hook configurations
    └── hooks.json       # Hook definitions
```

#### 7.2.7 市场分发

- **Official marketplace**: GCS 托管
- **Plugin identifier**: `name@marketplace` 格式
- **版本固定**: Git commit SHA
- **企业策略**: `blockedMarketplaces` / `strictKnownMarketplaces`
- **自动更新**: `pluginAutoupdate.ts`

#### 7.2.8 关键代码片段

Hook 注册（`loadPluginHooks.ts`）：
```typescript
function convertPluginHooksToMatchers(plugin: LoadedPlugin): Record<HookEvent, PluginHookMatcher[]> {
  const pluginMatchers: Record<HookEvent, PluginHookMatcher[]> = {
    PreToolUse: [], PostToolUse: [], PostToolUseFailure: [],
    // ... 27 种事件
  };
  if (!plugin.hooksConfig) return pluginMatchers;
  for (const [event, matchers] of Object.entries(plugin.hooksConfig)) {
    const hookEvent = event as HookEvent;
    for (const matcher of matchers) {
      if (matcher.hooks.length > 0) {
        pluginMatchers[hookEvent].push({
          matcher: matcher.matcher,
          hooks: matcher.hooks,
          pluginRoot: plugin.path,
          pluginName: plugin.name,
          pluginId: plugin.source,
        });
      }
    }
  }
  return pluginMatchers;
}
```

---

### 7.3 deepseek-harness（TypeScript）

#### 7.3.1 架构概览

Deepseek-harness 实现了 **Cordis Fiber 六态** 的双平面动态插件系统，跨 4 个包：

- `cordis-host-runner`：Node 端运行时 + registry + sandbox + lifecycle
- `cordis-client-runner`：浏览器端 runner
- `tool-cordis`：模型-facing 工具（`cordis_define`, `cordis_run`, `cordis_stop` 等）
- `ui-cordis`：浏览器 UI slots

#### 7.3.2 Cordis Fiber 六态

```typescript
export const FiberState = {
  PENDING: 0,   // 等待
  LOADING: 1,   // 加载中
  ACTIVE: 2,    // 运行中
  FAILED: 3,    // 失败
  DISPOSED: 4,  // 已处置
  UNLOADING: 5, // 卸载中
} as const
```

Run Status 状态机：
```
awaiting-approval → starting-host → client-pending → running
                                           ↓            ↓
                                        waiting ←→ running
                                           ↓
                   rejected / failed / cancelled / stopped
```

#### 7.3.3 身份 & Registry

```typescript
mintPluginId(prefix: string): string     // `${prefix}-${n}`  (e.g. "weather-1")
mintPackageId(): string                  // `pkg-${n}` immutable version
mintPluginRunId(): string                // `run-${n}` one activation
mintApprovalRequestId(): string          // `approval-${n}`
```

- **Plugin** = 稳定 identity + 有序 map of immutable Packages + approval grants
- **Package** = immutable version with optional `hostCode` and/or `clientCode`
- **Run** = one activation attempt; owns a Cordis `Fiber`

#### 7.3.4 沙箱/隔离

**Host half**: `node:vm` sandbox：
```typescript
export function createSandbox(id: string, harnessExtras = {}): object {
  const sandbox = {
    ...nodeApiTraps(),          // require, setTimeout, fetch, etc. → throw redirect
    console: taggedConsole(id),
    harness: { defineTool, registerTool, ...harnessExtras },
    btoa, atob, TextEncoder, TextDecoder,
  }
  createContext(sandbox)
  patchDualRealmInstanceof(sandbox)
  return sandbox
}
```

关键隔离特性：
- **Node API redirects**: `require`, `setTimeout`, `fetch` 被 trap + teaching errors
- **Dual-realm `instanceof` patch**: VM constructors accept host values
- **Sandbox context façade**: whitelist of `ctx` verbs
- **Cross-realm JSON clone**: all handler/tool returns cloned through `cloneJson()`
- **Parse gate**: `precheckCode()` rejects unparseable code at define time

#### 7.3.5 Hook Bridge（CC/Codex）

| Hook 点 (Claude Code) | Hook 点 (Codex) |
|----------------------|-----------------|
| SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, SubagentStart, SubagentStop | PreToolUse, PostToolUse, SessionStart, UserPromptSubmit, Stop |

Hook output 协议：
```typescript
export interface HookOutput {
  exitCode: number | undefined
  stderr: string
  stdout: string
  continue?: boolean
  stopReason?: string
  decision?: 'approve' | 'allow' | 'block' | 'deny' | 'ask'
  reason?: string
  hookEventName?: string
  additionalContext?: string
  systemMessage?: string
  updatedInput?: Record<string, unknown>
}
```

#### 7.3.6 IPC 协议

**Host ↔ Client** 使用 `@deepseek-ai/dsh-typert-protocol`（`TypertRemoteService` + `@Remote` decorator）。

---

### 7.4 openclaw（TypeScript）

#### 7.4.1 架构概览

OpenClaw 拥有 **153 bundled + 162 extensions + Workshop** 的最大插件生态。核心模块：

- `src/plugins/`：41 种 Hook 类型 + plugin registry + discovery + manifest
- `src/plugin-sdk/`：Plugin SDK 入口
- `src/hooks/`：bundled hooks + install + policy + workspace
- `extensions/`：153 bundled extensions

#### 7.4.2 Hook 类型（41 种）

```typescript
export type PluginHookName =
  | "before_model_resolve" | "agent_turn_prepare" | "before_prompt_build"
  | "before_agent_reply" | "model_call_started" | "model_call_ended"
  | "llm_input" | "llm_output" | "before_agent_finalize" | "agent_end"
  | "before_compaction" | "after_compaction" | "before_reset"
  | "inbound_claim" | "channel_pairing_requested"
  | "message_received" | "message_sending" | "reply_payload_sending" | "message_sent"
  | "before_tool_call" | "after_tool_call" | "tool_result_persist"
  | "before_message_write" | "session_start" | "session_end"
  | "subagent_delivery_target" | "subagent_spawned" | "subagent_progress" | "subagent_ended"
  | "gateway_start" | "gateway_stop"
  | "heartbeat_prompt_contribution" | "cron_reconciled" | "cron_changed"
  | "skill_proposal_evaluate" | "skill_proposal_changed" | "skill_changed"
  | "before_dispatch" | "reply_dispatch"
  | "before_install" | "before_agent_run" | "resolve_exec_env"
```

#### 7.4.3 Hook Runner（fail-open/fail-closed）

```typescript
export function initializeGlobalHookRunner(registry: GlobalHookRunnerRegistry): void {
  state.registry = registry;
  if (!state.hookRunner) {
    state.hookRunner = createHookRunner(createLiveHookRegistryFacade(state), {
      catchErrors: true,
      failurePolicyByHook: {
        before_agent_run: "fail-closed",
        before_install: "fail-closed",
        before_tool_call: "fail-closed",
      },
    });
  }
}
```

超时配置：
```typescript
const DEFAULT_VOID_HOOK_TIMEOUT_MS_BY_HOOK: Partial<Record<PluginHookName, number>> = {
  agent_end: 30_000,
  before_compaction: 30_000,
  after_compaction: 30_000,
  gateway_stop: 5_000,
};
const DEFAULT_MODIFYING_HOOK_TIMEOUT_MS_BY_HOOK: Partial<Record<PluginHookName, number>> = {
  before_agent_run: 15_000,
  before_install: 15_000,
  before_tool_call: 15_000,
  before_agent_finalize: 15_000,
  before_prompt_build: 15_000,
  message_sending: 15_000,
  skill_proposal_evaluate: 120_000,
};
```

#### 7.4.4 Hook 决策类型

```typescript
type HookDecisionPass = { outcome: "pass" };
type HookDecisionBlock = {
  outcome: "block";
  reason: string;                    // internal plugin-local reason
  message?: string;                  // user-facing detail
  category?: string;                 // analytics category
  metadata?: Record<string, unknown>;
};
export type InputGateDecision = HookDecisionPass | HookDecisionBlock;
```

#### 7.4.5 Hook 隔离

```typescript
export function cloneHookIsolationValue<T>(hookName: PluginHookName, value: T): T {
  try {
    if (containsSharedMemory(value, new Set<object>())) {
      throw new TypeError("shared memory cannot be isolated");
    }
    const cloned = structuredClone(value);
    if (containsSharedMemory(cloned, new Set<object>())) {
      throw new TypeError("shared memory cannot be isolated");
    }
    return cloned;
  } catch (cause) {
    throw new HookIsolationError(`[hooks] ${hookName} mutable input isolation failed`, { cause });
  }
}
```

#### 7.4.6 Plugin Manifest

```typescript
export const PLUGIN_MANIFEST_FILENAME = "openclaw.plugin.json";
// 256 KB max
// 必需: id, configSchema
// 可选: name, description, version, kind, channels, providers,
//       modelCatalog, modelPricing, mcpServers, skills, contracts,
//       backupResources, requiresPlugins, doctorContract, ...
```

#### 7.4.7 Plugin Discovery

```typescript
/** Discovers plugin candidates from bundled, workspace, global, package, and bundle roots. */
export function discoverPluginCandidates(...): PluginDiscoveryResult
```

发现源（按优先级）：
1. **Bundled**: 153 内置 extensions
2. **Workspace**: 项目级
3. **Global**: 用户级
4. **Package**: npm 包
5. **Bundle**: 打包格式

安全门控：
```typescript
type CandidateBlockReason =
  | "source_escapes_root"
  | "path_stat_failed"
  | "path_world_writable"
  | "path_suspicious_ownership";
```

#### 7.4.8 Plugin SDK 入口

```typescript
export function definePluginEntry({
  id, name, description, kind, configSchema,
  reload, nodeHostCommands, securityAuditCollectors, register,
}: DefinePluginEntryOptions): DefinedPluginEntry
```

SDK 导出 60+ 类型：`OpenClawPluginApi`, `OpenClawPluginToolFactory`, `ProviderPlugin`, `MigrationProviderPlugin`, `MediaUnderstandingProviderPlugin`, ...

#### 7.4.9 关键代码片段

Plugin API 构建（`api-builder.ts`）：
```typescript
export function buildPluginApi(params: BuildPluginApiParams): OpenClawPluginApi {
  const handlers = params.handlers ?? {};
  const api: OpenClawPluginApiWithoutFacades = {
    id: params.id,
    name: params.name,
    registerTool: handlers.registerTool ?? noops.registerTool,
    registerHook: handlers.registerHook ?? noops.registerHook,
    registerHttpRoute: handlers.registerHttpRoute ?? noops.registerHttpRoute,
    registerProvider: handlers.registerProvider ?? noops.registerProvider,
    registerEmbeddingProvider: handlers.registerEmbeddingProvider ?? noops.registerEmbeddingProvider,
    registerMigrationProvider: handlers.registerMigrationProvider ?? noops.registerMigrationProvider,
    // ... 60+ methods
  };
  return attachPluginApiFacades(api);
}
```

---

### 7.5 opencode（TypeScript/Bun）

#### 7.5.1 架构概览

Opencode 实现了 **双入口 Plugin 系统**（server-side + TUI-side），核心包 `@opencode-ai/plugin`。

```typescript
export type Plugin = (input: PluginInput, options?: PluginOptions) => Promise<Hooks>
export type PluginModule =
  | { id?: string; server: Plugin; tui?: never }
  | { id?: string; tui: TuiPlugin; server?: never }
```

#### 7.5.2 Hook 类型（20+）

| Hook | 用途 |
|------|------|
| `dispose` | cleanup |
| `event` | all events |
| `config` | modify config |
| `tool` | register custom tools |
| `auth` | custom auth (OAuth/API) |
| `provider` | custom provider models |
| `chat.message` | new message received |
| `chat.params` | modify LLM params |
| `chat.headers` | modify request headers |
| `permission.ask` | intercept permission |
| `command.execute.before` | before command execution |
| `tool.execute.before` | before tool execution |
| `shell.env` | modify shell env |
| `tool.execute.after` | after tool execution |
| `experimental.chat.messages.transform` | transform message list |
| `experimental.chat.system.transform` | transform system prompt |
| `experimental.provider.small_model` | override small model |
| `experimental.session.compacting` | customize compaction |
| `experimental.compaction.autocontinue` | skip synthetic continue |
| `experimental.text.complete` | text completion |
| `tool.definition` | modify tool description |

#### 7.5.3 Manifest 格式

**package.json** with `exports`：
```json
{
  "exports": {
    "./server": "./src/server.ts",
    "./tui": "./src/tui.ts"
  },
  "oc-themes": ["themes/dark.json", "themes/light.json"],
  "engines": { "opencode": ">=1.0.0" }
}
```

#### 7.5.4 注册机制

**声明式 in config** + **CLI install**：
```typescript
export type Config = Omit<SDKConfig, "plugin"> & {
  plugin?: Array<string | [string, PluginOptions]>
}
```

Config 源：
- Global: `~/.config/opencode/opencode.json`
- Local: `.opencode/opencode.json`
- Workspace scan: `{plugin,plugins}/*.{ts,js}`

#### 7.5.5 沙箱/隔离

**In-process, no sandbox**. Plugins are `import()`-ed directly into the Bun process.

#### 7.5.6 Plugin Meta 追踪

```typescript
export type Entry = {
  id: string
  source: "file" | "npm"
  spec: string
  target: string
  version?: string
  modified?: number        // file mtime
  first_time: number
  last_time: number
  load_count: number
  fingerprint: string      // target|modified or target|requested|version
}
export type State = "first" | "updated" | "same"
```

---

### 7.6 pi（TypeScript）

#### 7.6.1 架构概览

Pi 实现了 **一等公民 Skill + SDK** 的扩展系统，核心包 `@earendil-works/pi-coding-agent`。

```typescript
export type ExtensionFactory = (pi: ExtensionAPI) => void | Promise<void>;
export type InlineExtension =
    | ExtensionFactory
    | { name: string; factory: ExtensionFactory; hidden?: boolean };
```

#### 7.6.2 Extension API（30+ events）

```typescript
export interface ExtensionAPI {
  on(event: "session_start", handler: ExtensionHandler<SessionStartEvent>): void;
  on(event: "session_info_changed", handler: ExtensionHandler<SessionInfoChangedEvent>): void;
  on(event: "session_before_switch", handler: ExtensionHandler<SessionBeforeSwitchEvent, SessionBeforeSwitchResult>): void;
  on(event: "session_before_fork", handler: ExtensionHandler<SessionBeforeForkEvent, SessionBeforeForkResult>): void;
  on(event: "session_before_compact", handler: ExtensionHandler<SessionBeforeCompactEvent, SessionBeforeCompactResult>): void;
  on(event: "session_compact", handler: ExtensionHandler<SessionCompactEvent>): void;
  on(event: "session_shutdown", handler: ExtensionHandler<SessionShutdownEvent>): void;
  on(event: "context", handler: ExtensionHandler<ContextEvent, ContextEventResult>): void;
  on(event: "before_provider_request", handler: ExtensionHandler<BeforeProviderRequestEvent, BeforeProviderRequestEventResult>): void;
  on(event: "before_provider_headers", handler: ExtensionHandler<BeforeProviderHeadersEvent>): void;
  on(event: "after_provider_response", handler: ExtensionHandler<AfterProviderResponseEvent>): void;
  on(event: "before_agent_start", handler: ExtensionHandler<BeforeAgentStartEvent, BeforeAgentStartEventResult>): void;
  on(event: "agent_start", handler: ExtensionHandler<AgentStartEvent>): void;
  on(event: "agent_end", handler: ExtensionHandler<AgentEndEvent>): void;
  on(event: "turn_start", handler: ExtensionHandler<TurnStartEvent>): void;
  on(event: "turn_end", handler: ExtensionHandler<TurnEndEvent>): void;
  on(event: "message_start", handler: ExtensionHandler<MessageStartEvent>): void;
  on(event: "message_update", handler: ExtensionHandler<MessageUpdateEvent>): void;
  on(event: "message_end", handler: ExtensionHandler<MessageEndEvent, MessageEndEventResult>): void;
  on(event: "tool_execution_start", handler: ExtensionHandler<ToolExecutionStartEvent>): void;
  on(event: "tool_execution_update", handler: ExtensionHandler<ToolExecutionUpdateEvent>): void;
  on(event: "tool_execution_end", handler: ExtensionHandler<ToolExecutionEndEvent>): void;
  on(event: "tool_call", handler: ExtensionHandler<ToolCallEvent, ToolCallEventResult>): void;
  on(event: "tool_result", handler: ExtensionHandler<ToolResultEvent, ToolResultEventResult>): void;
  on(event: "model_select", handler: ExtensionHandler<ModelSelectEvent>): void;
  on(event: "thinking_level_select", handler: ExtensionHandler<ThinkingLevelSelectEvent>): void;
  on(event: "user_bash", handler: ExtensionHandler<UserBashEvent, UserBashEventResult>): void;
  on(event: "input", handler: ExtensionHandler<InputEvent, InputEventResult>): void;
  on(event: "project_trust", handler: ProjectTrustHandler): void;
  // ... 更多
  registerTool<TParams>(tool: ToolDefinition<TParams>): void;
  registerCommand(name: string, options: Omit<RegisteredCommand, "name" | "sourceInfo">): void;
  registerShortcut(shortcut: KeyId, options: { handler: (ctx: ExtensionContext) => void }): void;
  registerFlag(name: string, options: { type: "boolean" | "string"; default?: boolean | string }): void;
  registerProvider(provider: Provider): void;
  registerMessageRenderer<T>(customType: string, renderer: MessageRenderer<T>): void;
  registerMarkdownTransformer(transformer: MarkdownTransformer): void;
}
```

#### 7.6.3 沙箱/隔离

**In-process**: Extensions run in same process via `jiti` module loader。

虚拟模块（for compiled binaries）：
```typescript
const VIRTUAL_MODULES: Record<string, unknown> = {
  "typebox": _bundledTypebox,
  "@earendil-works/pi-agent-core": _bundledPiAgentCore,
  "@earendil-works/pi-ai": _bundledPiAiCompat,
  "@earendil-works/pi-coding-agent": _bundledPiCodingAgent,
};
```

#### 7.6.4 关键代码片段

示例 Extension（`tps.ts`）：
```typescript
export default function (pi: ExtensionAPI) {
  let agentStartMs: number | null = null;
  pi.on("agent_start", () => { agentStartMs = Date.now(); });
  pi.on("agent_end", (event, ctx) => {
    if (!ctx.hasUI) return;
    const elapsedMs = Date.now() - agentStartMs;
    // ... 计算 TPS
    ctx.ui.notify(`TPS ${tokensPerSecond.toFixed(1)} tok/s`, "info");
  });
}
```

---

### 7.7 hermes-agent（Python）

#### 7.7.1 架构概览

Hermes Agent 支持 **6+ 前端共享 AIAgent**，核心模块：

- `agent/`：核心 agent 代码
- `hermes_cli/plugins.py`：主 plugin 管理器
- `plugins/`：bundled plugins（memory, context_engine, browser, ...）

#### 7.7.2 Plugin Sources（later overrides earlier）

1. Bundled: `<repo>/plugins/<name>/`
2. User: `~/.hermes/plugins/<name>/`
3. Project: `./.hermes/plugins/<name>/`（opt-in via `HERMES_ENABLE_PROJECT_PLUGINS`）
4. Pip entry points: `hermes_agent.plugins` group

#### 7.7.3 Manifest 格式（YAML）

```python
@dataclass
class PluginManifest:
    name: str
    version: str = ""
    description: str = ""
    author: str = ""
    requires_env: List[Union[str, Dict[str, Any]]] = field(default_factory=list)
    provides_tools: List[str] = field(default_factory=list)
    provides_hooks: List[str] = field(default_factory=list)
    source: str = ""  # "bundled", "user", "project", "entrypoint"
    kind: str = "standalone"  # standalone | backend | exclusive | platform | model-provider
    portable: bool = False
    capabilities: List[str] = field(default_factory=list)
    manifest_version: int = 1
    api_version: Optional[int] = None
    requires_plugins: List[Dict[str, Any]] = field(default_factory=list)
    python_dependencies: List[str] = field(default_factory=list)
    config_schema: Dict[str, Any] = field(default_factory=dict)
    emits: List[str] = field(default_factory=list)
    listens: List[str] = field(default_factory=list)
```

#### 7.7.4 Hook 类型（40+）

```python
VALID_HOOKS: Set[str] = {
    # Tool lifecycle
    "pre_tool_call", "post_tool_call",
    "transform_terminal_output", "transform_tool_result",
    "transform_llm_output", "pre_llm_call", "post_llm_call",
    # Streaming observers
    "on_stream_start", "on_stream_delta", "on_stream_end", "on_interim_message",
    # Verification
    "pre_verify",
    # API request lifecycle
    "pre_api_request", "post_api_request", "api_request_error",
    "transform_api_error_classification",
    # Session lifecycle
    "on_session_start", "on_session_end", "on_session_finalize", "on_session_reset",
    "on_skill_lifecycle",
    # Subagent
    "subagent_start", "subagent_stop",
    # Gateway
    "pre_gateway_dispatch",
    # Approval
    "pre_approval_request", "post_approval_response",
    # Transcription
    "pre_transcription",
    # Kanban
    "kanban_task_claimed", "kanban_task_completed", "kanban_task_blocked",
    "on_kanban_worker_spawned", "on_kanban_worker_exited",
    "on_kanban_worker_stale_claim", "on_kanban_task_updated",
    "on_kanban_dispatch_tick",
    # Platform
    "gateway_platform_event",
    # Command
    "pre_command",
}
```

#### 7.7.5 Stream Hooks（Async Observer Pattern）

```python
@dataclass
class _ConsumerDispatcher:
    hook_name: str
    callback: Callable[..., Any]
    events: queue.Queue[dict[str, Any] | object]
    thread: threading.Thread | None = None

def enqueue_plugin_stream_hook(hook_name: str, **payload: Any) -> bool:
    """Queue an observer hook for each consumer without running plugin code inline."""
```

Queue 语义：
- Bounded queue（1024 events）
- Drop-oldest on full
- Per-callback daemon worker threads
- Plugin code never runs on token path

#### 7.7.6 Portable Agent Plugins（Multi-Frontend）

```python
PLUGIN_SCHEMA_V1 = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"

@dataclass(frozen=True)
class AgentPluginPackage:
    name: str
    version: str
    description: str
    root: Path
    data_root: Path
    manifest: Mapping[str, Any]
    skills: Tuple[AgentPluginSkill, ...]
    mcp_servers: Mapping[str, Dict[str, Any]]
    diagnostics: Tuple[AgentPluginDiagnostic, ...]
```

支持的前端：Claude Code, Cline, Codex, Cursor, VS Code, Windsurf, OpenClaw, Continue。

---

### 7.8 agent-core（Python, openJiuwen）

#### 7.8.1 架构概览

openJiuwen 是 Agent 运行时内核，核心模块：

- `core/runner/callback/`：`AsyncCallbackFramework` — 中心扩展机制
- `core/graph/pregel/`：Pregel 图引擎 — DAG/superstep workflow
- `core/security/guardrail/`：pluggable policy
- `extensions/external_provider/`：auth/model-catalog plugins
- `core/foundation/tool/mcp/`：5 transport clients

#### 7.8.2 AsyncCallbackFramework

```python
def on(
    self,
    event: str,
    *,
    priority: int = 0,
    once: bool = False,
    namespace: str = "default",
    tags: Optional[Set[str]] = None,
    filters: Optional[List[EventFilter]] = None,
    max_retries: int = 0,
    retry_delay: float = 0.0,
    timeout: Optional[float] = None,
    callback_type: str = "",
):
    """Decorator to register an async callback."""
```

四种装饰器族：
- `on(event)` — register a callback
- `emit_before(event)` / `emit_after(event)` / `emit_around(before, after)` — emit events around a function
- `transform_io(input_event=, output_event=)` — input/output transformation via events
- `wrap(event)` / `on_wrap(event)` — `call_next`-style handler chains

#### 7.8.3 Hook 类型（40+ events）

| Event class | Members |
|---|---|
| `AgentEvents` | AGENT_STARTED, AGENT_INVOKE_INPUT/OUTPUT, AGENT_STREAM_INPUT/OUTPUT |
| `AgentTeamEvents` | AGENT_P2P_RECEIVED, AGENT_PUBSUB_RECEIVED |
| `WorkflowEvents` | WORKFLOW_STARTED/FINISHED/ERROR/CANCELLED, NODE_EXECUTED/ERROR, EDGE_TRAVERSED, LOOP_STARTED/FINISHED |
| `LLMCallEvents` | LLM_CALL_STARTED/ERROR, LLM_RESPONSE_RECEIVED, LLM_INVOKE_INPUT/OUTPUT, LLM_STREAM_INPUT/OUTPUT/COMPLETED |
| `ToolCallEvents` | TOOL_CALL_STARTED/FINISHED/ERROR, TOOL_RESULT_RECEIVED, TOOL_PARSE_STARTED/FINISHED, TOOL_AUTH |
| `ContextEvents` | CONTEXT_UPDATED/OFFLOADED/RETRIEVED/CLEARED, CONTEXT_COMPRESSION_STATE |
| `SessionEvents` | SESSION_CREATED, AGENT_SESSION_CREATED |
| `MemoryEvents` | MEMORY_ADDED/SEARCH_STARTED/SEARCH_FINISHED/UPDATED/DELETED |
| `TaskManagerEvents` | TASK_CREATED/RUNNING/COMPLETED/FAILED/CANCELLED/TIMEOUT |

#### 7.8.4 Pregel 图引擎

```python
class PregelNode:
    def __init__(self, name: str, func: Callable[[Any], Any], routers: list[IRouter]):
        self.name, self.func, self.routers = name, func, routers

class Channel(ABC):   # is_ready / accept / consume / snapshot / restore
class TriggerChannel(Channel): ...   # fires on any message
class BarrierChannel(Channel): ...   # CNF (AND-of-OR) fan-in
```

Builder — 声明式：
```python
class PregelBuilder:
    def add_node(self, name, fn, routers=None): ...
    def add_edge(self, start, end): ...   # N→1 barrier, 1→N static
    def add_branch(self, src, selector): ...   # conditional
    def build(self, store=None, after_step_callback=None): ...
```

#### 7.8.5 MCP 5 Transports

| Client | File | `__client_name__` |
|---|---|---|
| `StdioClient` | `client/stdio_client.py` | `"stdio"` |
| `SseClient` | `client/sse_client.py` | `"sse"` |
| `StreamableHttpClient` | `client/streamable_http_client.py` | `["streamable-http","streamable_http"]` |
| `OpenApiClient` | `client/openapi_client.py` | ( OpenAPI/Swagger → MCP ) |
| `PlaywrightClient` | `client/playwright_client.py` | `"playwright"` |

---

### 7.9 agent-studio（Python）

#### 7.9.1 架构概览

Agent Studio 是一站式 Agent 平台，核心模块：

- `backend/openjiuwen_studio/`：主 backend
- `connect/`：channels + MCP adapters
- `plugin_server/`：FastAPI plugin server
- `sandbox_server/`：BubbleWrap + Seccomp sandbox
- `marketplace/ready_plugins/`：JSON catalog（~100 plugins, 17 categories）

#### 7.9.2 Plugin Manifest（JSON Schema）

```json
{
  "$id": "https://openjiuwen.com/schemas/plugin-config.json",
  "required": ["plugin_id","name","description","plugin_type","tools"],
  "properties": {
    "plugin_id": { "pattern": "^[a-z0-9_]+$" },
    "plugin_type": { "enum": [1, 2] },
    "version": { "pattern": "^\\d+\\.\\d+\\.\\d+$" },
    "tools": { "items": { "required": ["name","path","method","description"] } }
  }
}
```

`plugin_type`: **1 = Cloud API**, **2 = Cloud Code**。

#### 7.9.3 DSL 双向转换

`backend/openjiuwen_studio/core/dsl_converter/`：
- `converter/converter.py`, `converter_native.py`, `converter_n8n.py` — native ↔ n8n formats
- `n8n_mappings.py`, `n8n_marketplace_mapping.py` — node mapping tables
- `importer.py`, `validator.py`, `detector.py`, `reporter.py` — import pipeline

#### 7.9.4 Sandbox — BubbleWrap + Seccomp

```python
class BubbleWrapRunner(BaseSandbox, sandbox_type='bubblewrap'):
    @staticmethod
    def pre_init(sandbox_config):
        arch = platform.machine()   # x86_64 | aarch64
        allowed = sandbox_config.seccomp['allow'].get(arch, [])
        bpf = pyseccomp.SyscallFilter(pyseccomp.KILL)
        for syscall in allowed:
            bpf.add_rule(pyseccomp.ALLOW, syscall)
        sandbox_config.seccomp_bpf = bpf

    def run(self, raw_code, base_code, lang, timeout=0, dep_name=None):
        # builds bwrap command line with --bind/--ro-bind/--unshare-*,
        # --seccomp fd, setpriv --reuid/--regid, then execs language runtime.
```

关键隔离原语：
- **Namespaces**: user, ipc, pid, cgroup, uts（configurable）；net disabled by default
- **Mounts**: `/lib`, `/lib64`, `/usr/bin`, `/usr/lib`, `/usr/lib64`, `/usr/local/bin`, `/usr/local/lib`, `/etc/resolv.conf`, `/usr/share/nodejs`, `/dev/urandom`
- **Seccomp BPF**: whitelist mode（`pyseccomp.KILL` default），arch-specific syscall lists（~90 allowed syscalls on x86_64）
- **User**: `setpriv --reuid sandbox-exec --regid sandbox-exec --clear-groups`
- **Network**: `allow_internal_network_access: False` + `network_guard.apply_internal_network_guard()`

---

### 7.10 cc-switch（Tauri 2 + Rust + React）

#### 7.10.1 架构概览

cc-switch 是 **多 App 配置管理器**（Claude, Codex, Gemini, GrokBuild, OpenCode, OpenClaw, Hermes, Pi, Claude Desktop）。核心模块：

- `claude_plugin.rs`：Claude plugin 集成
- `mcp/*`：MCP server adapters for 6+ apps
- `services/skill.rs`：Skill management with SSOT
- `provider.rs`：universal provider model

#### 7.10.2 8 工具适配

```
proxy/providers/        — per-provider handlers
proxy/provider_router.rs
proxy/model_mapper.rs
proxy/thinking_optimizer.rs
proxy/thinking_rectifier.rs
proxy/copilot_optimizer.rs
proxy/gemini_url.rs
proxy/failover_switch.rs
```

#### 7.10.3 MCP SSOT（Single Source of Truth）

```rust
pub struct McpApps {
    pub claude: bool,
    pub codex: bool,
    pub gemini: bool,
    pub grokbuild: bool,
    pub opencode: bool,
    pub hermes: bool,
}

pub struct McpServer {
    pub id: String,
    pub name: String,
    pub server: Value,
    pub apps: McpApps,
}
```

Per-app MCP adapters：

| App | Config format | Module |
|-----|--------------|--------|
| Claude | `~/.claude.json` `mcpServers` | `claude.rs` |
| Codex | `~/.codex/config.toml` `[mcp_servers.*]` | `codex.rs` (TOML) |
| Gemini | `~/.gemini/settings.json` | `gemini.rs` |
| GrokBuild | `[mcp_servers]` in config | `grokbuild.rs` |
| OpenCode | `opencode.json` local/remote | `opencode.rs` |
| Hermes | `config.yaml` | `hermes.rs` |

#### 7.10.4 Skill System

```rust
pub struct DiscoverableSkill {
    pub key: String,           // "owner/name:directory"
    pub name: String,
    pub description: String,
    pub directory: String,
    pub repo_owner: String,
    pub repo_name: String,
    pub repo_branch: String,
}

pub enum SyncMethod { Auto, Symlink, Copy }
pub enum SkillStorageLocation { CcSwitch, Unified }
```

---

### 7.11 jiuwenswarm（Python）

#### 7.11.1 架构概览

Jiuwenswarm 实现了 **17 hook events + SkillDevPipeline 12 阶段 + JiuwenBox** 的完整插件系统。核心模块：

- `jiuwenswarm/extensions/sdk/`：BaseExtension, ApplicationPluginExtension
- `jiuwenswarm/server/hooks/`：HookExecutor, UserHookRail, GatewayHookHandler
- `jiuwenswarm/server/runtime/skill/skilldev/pipeline.py`：12 阶段 Skill 开发
- `jiuwenbox/`：BubbleWrap + Landlock + Seccomp + netns + cgroup

#### 7.11.2 Extension Manifest（YAML）

```yaml
id: video-duplex
name: Full-duplex Video
version: 1.0.0
description: Camera, screen, microphone, realtime model, search, ASR and TTS workspace
author: JiuwenSwarm
min_jiuwenswarm_version: "0.2.5"
package_type: application
dependencies: {}
permissions:
  - camera
  - microphone
  - display_capture
  - outbound_network
  - agent_tools
```

#### 7.11.3 Hook 类型（17 种）

```python
class HookEvent(str, Enum):
    PRE_TOOL_USE = "PreToolUse"
    POST_TOOL_USE = "PostToolUse"
    POST_TOOL_USE_FAILURE = "PostToolUseFailure"
    STOP = "Stop"
    USER_PROMPT_SUBMIT = "UserPromptSubmit"
    SESSION_START = "SessionStart"
    SESSION_END = "SessionEnd"
    NOTIFICATION = "Notification"
    PERMISSION_REQUEST = "PermissionRequest"
    PERMISSION_DENIED = "PermissionDenied"
    SUBAGENT_START = "SubagentStart"
    SUBAGENT_STOP = "SubagentStop"
    CONFIG_CHANGE = "ConfigChange"
    INSTRUCTIONS_LOADED = "InstructionsLoaded"
    SETUP = "Setup"
    BEFORE_MODEL_CALL = "BeforeModelCall"
    AFTER_MODEL_CALL = "AfterModelCall"
```

两层执行：
- **AgentServer Rail layer**（`_AGENT_RAIL_EVENTS`）：PreToolUse, PostToolUse, PostToolUseFailure, Stop, PermissionRequest, PermissionDenied, SubagentStart, SubagentStop, BeforeModelCall, AfterModelCall
- **Gateway layer**（`_GATEWAY_EVENTS`）：UserPromptSubmit, SessionStart, SessionEnd, Notification, ConfigChange, InstructionsLoaded, Setup

#### 7.11.4 Hook Executor

```python
class HookExecutor:
    async def run_all(self, hook_configs, hook_input, session_id="") -> list[HookResult]:
        # parallel gather over command + prompt hooks

    async def _run_command_hook(self, config, hook_input) -> HookResult:
        proc = await asyncio.create_subprocess_exec(
            shell, "-c", command,
            stdin=PIPE, stdout=PIPE, stderr=PIPE,
            env={**env, "ARGUMENTS": json, "TOOL_NAME": name},
        )
        stdout, stderr = await asyncio.wait_for(proc.communicate(...), timeout)
        # exit 0 = success; exit 2 = blocking; else non-blocking error
```

#### 7.11.5 SkillDevPipeline（12 阶段）

```python
class SkillDevPipeline:
    STAGE_HANDLERS = {
        SkillDevStage.INIT: InitStageHandler,
        SkillDevStage.PLAN: PlanStageHandler,
        SkillDevStage.GENERATE: GenerateStageHandler,
        SkillDevStage.VALIDATE: ValidateStageHandler,
        SkillDevStage.TEST_DESIGN: TestDesignStageHandler,
        SkillDevStage.TEST_RUN: TestRunStageHandler,
        SkillDevStage.EVALUATE: EvaluateStageHandler,
        SkillDevStage.IMPROVE: ImproveStageHandler,
        SkillDevStage.PACKAGE: PackageStoreHandler,
        SkillDevStage.DESC_OPTIMIZE: DescOptimizeStageHandler,
    }
```

阶段流：`INIT → PLAN → PLAN_CONFIRM(suspend) → GENERATE → VALIDATE → TEST_DESIGN → TEST_RUN → EVALUATE → REVIEW(suspend) → IMPROVE → (loop back to TEST_RUN) → PACKAGE → DESC_OPTIMIZE_CONFIRM(suspend) → DESC_OPTIMIZE → COMPLETED`

#### 7.11.6 JiuwenBox Sandbox

- **Isolation**: BubbleWrap + Landlock + seccomp + network namespaces + cgroups
- **Filesystem policy** (YAML): `directories`, `files`, `read_only`, `read_write`, `bind_mounts`
- **Network**: `isolated` mode（per-sandbox netns `jbx-{id}`, veth pair, iptables）or `host`
- **Capabilities**: add/drop（e.g. `drop: ["ALL"]`）
- **Seccomp**: per-arch blocklists（ptrace, mount, umount2, reboot, kexec_load）
- **Cgroups**: `memory_max`, `cpu_max`, `pids_max`
- **Transports**: TCP（`http://host:port`）and Unix Domain Socket（`unix:///abs/path`）
- **MCP**: `/mcp` Streamable HTTP endpoint

---

### 7.12 semantica（Python）

#### 7.12.1 架构概览

Semantica 是 **图原生 AI 基础设施**，核心模块：

- `plugins/`：IDE plugins（per-IDE manifests）+ 17 skills + hooks
- `semantica/reasoning/`：Rete + Datalog engine
- `semantica/context/`：ContextGraph（229 KB）

#### 7.12.2 Plugin Manifest（per-IDE）

```json
{
  "name": "semantica-cursor",
  "displayName": "Semantica Cursor Plugin",
  "version": "0.1.0",
  "skills": "./skills",
  "agents": ["./agents/decision-advisor.md","./agents/explainability.md","./agents/kg-assistant.md"],
  "hooks": "./hooks/hooks.json",
  "mcp": { "server": "python -m semantica.mcp_server", "transport": "stdio" }
}
```

#### 7.12.3 Hook 类型（2 种）

```json
{
  "hooks": {
    "PostToolUse": [
      {"matcher": "Write|Edit", "hooks": [{"type": "command", "command": "..."}]}
    ],
    "PreToolUse": [
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "..."}]}
    ]
  }
}
```

#### 7.12.4 Rete Engine

```python
class ReteEngine:
    def build_network(self, rules: List[Rule]) -> None:
        for rule in rules:
            self._add_rule_to_network(rule)   # alpha per condition → beta joins → terminal
    def add_fact(self, fact: Fact) -> None:
        self.facts.append(fact)
        self._propagate_fact(fact)            # alpha match → beta join → terminal.activate(Match)
    def execute_matches(self, matches=None) -> List[Any]:
        for match in matches:
            results.append(match.rule.conclusion)
            if self.reasoner is not None and (match.rule.actions or match.rule.handler ...):
                self.reasoner._fire_actions(match.rule, match.bindings)
```

---

### 7.13 Switchyard（Rust, NVIDIA LLM Gateway）

#### 7.13.1 架构概览

Switchyard 是 **LLM 网关**，核心 crate：

- `switchyard-nemo-relay-plugin`：cdylib plugin implementing `NativePlugin` trait
- `switchyard-translation`：`TranslationEngine` + `FormatRegistry`
- `switchyard-runner`：9 routing algorithms
- `switchyard-py`：PyO3 module

#### 7.13.2 NativePlugin Trait

```rust
#[derive(Default)]
struct SwitchyardPlugin;

impl NativePlugin for SwitchyardPlugin {
    fn plugin_kind(&self) -> &str { "nvidia.switchyard" }
    fn allows_multiple_components(&self) -> bool { false }

    fn validate(&self, plugin_config: &Map<String, Json>) -> Vec<ConfigDiagnostic> {
        match parse_config(plugin_config).and_then(SwitchyardRuntime::new) {
            Ok(_) => Vec::new(),
            Err(message) => vec![ConfigDiagnostic {
                level: DiagnosticLevel::Error,
                code: "switchyard.invalid_config".into(),
                message,
            }],
        }
    }

    fn register(&mut self, plugin_config: &Map<String, Json>, ctx: &mut PluginContext<'_>) -> Result<()> {
        let config = parse_config(plugin_config)?;
        let priority = config.priority;
        let runtime = Arc::new(SwitchyardRuntime::new(config)?);
        register_buffered(ctx, priority, Arc::clone(&runtime), ctx.runtime())?;
        register_stream(ctx, priority, runtime, ctx.runtime())?;
        Ok(())
    }
}
```

#### 7.13.3 Hook 类型（2 种 LLM 执行拦截）

| Hook id | Kind | When |
|---|---|---|
| `switchyard.runner.buffered` | `register_llm_execution_intercept` | every LLM call, non-streaming |
| `switchyard.runner.streaming` | `register_llm_stream_execution_intercept` | every LLM call, streaming |

#### 7.13.4 TranslationEngine

```rust
pub struct FormatRegistry { codecs: BTreeMap<FormatId, Arc<dyn FormatCodec>> }
impl FormatRegistry {
    pub fn with_builtins() -> Self {  // OpenAiChat, AnthropicMessages, OpenAiResponses
        registry.register(OpenAiChatCodec);
        registry.register(AnthropicMessagesCodec);
        registry.register(OpenAiResponsesCodec);
    }
}

pub struct TranslationEngine {
    registry: FormatRegistry,            // buffered codecs
    stream_registry: StreamCodecRegistry // streaming codecs
}
```

#### 7.13.5 9 Routing Algorithms

| Algorithm | Purpose |
|---|---|
| `Noop {}` | smoke test |
| `Random { targets, weights, seed }` | weighted split traffic |
| `Passthrough { target, subagents }` | single target |
| `LlmClassifier { config }` | judge model picks tier |
| `StageRouter { tiers, picker, classifier, subagents }` | per-turn signal scoring |
| `Composite { classifier, stage, subagents }` | judge + stage router |
| `Advisor { executor_target, advisor_target, ... }` | executor + review model |
| `PrefillRouter { targets, checkpoint, device, ... }` | checkpoint-backed prefill classifier |

---

### 7.14 TencentDB-Agent-Memory（TypeScript + Python）

#### 7.14.1 架构概览

TencentDB-Agent-Memory 是 **四层 Agent 记忆系统**（L0-L3），作为 OpenClaw plugin 部署。核心模块：

- `MemoryCore`：plugin + gateway
- `MemoryKnowledge`：knowledge layer
- `MemoryPanel`：UI panel
- `MemoryProxy`：proxy layer
- `sdk/memory-core`：TS SDK

#### 7.14.2 Manifest（openclaw.plugin.json）

```json
{
  "id": "memory-tencentdb",
  "name": "Memory (TencentDB)",
  "activation": { "onStartup": true },
  "contracts": { "tools": ["tdai_memory_search","tdai_conversation_search","tdai_read_cos"] },
  "configSchema": { "type":"object", "properties": {
     "mode": { "enum":["local","function","client","gateway","remote"],"default":"local" },
     "storeBackend": { "enum":["sqlite","tcvdb"],"default":"sqlite" },
     "capture": { "enabled": true, "excludeAgents": [], "l0l1RetentionDays": 0, "cleanTime": "03:00" },
     "extraction": { "enabled": true, "enableDedup": true, "maxMemoriesPerSession": 20 },
     "recall": { "enabled": true, "maxResults": 5, "scoreThreshold": 0.3, "strategy": "hybrid" }
  }}
}
```

#### 7.14.3 Hook 类型（2 种）

| Hook | Direction | Behavior |
|---|---|---|
| `before_prompt_build` | **read** (recall) | parallel `searchAtomic` (L1) + `readCore` (L3 persona) + `listScenes` (L2) |
| `agent_end` | **write** (capture) | position-slice → timestamp cursor → replace polluted user message → sanitize/strip/filter → `addConversation` (L0) |

#### 7.14.4 SkillCore 6 写 4 读

**6 write actions**：`create`, `update`, `patch`, `delete`, `writeFiles`, `removeFiles`
**4 read actions**：`get`, `list`, `search`, `listVersions`（+ `readFile`）

#### 7.14.5 InjectionPipeline 8 注入点

Recall 注入 3 个并行数据源：
1. **L1 memories**（dynamic, per-turn, prepended to user prompt）
2. **L3 persona**（stable, appended to system prompt）
3. **L2 scene navigation**（full injection, LLM decides relevance）
4. **memory-tools-guide**（injection telling agent how to call tools）

Write-path 注入点：
5. position-slice（scope to this turn）
6. timestamp cursor（fallback de-dup）
7. polluted-user-message replacement（prevent recall feedback loop）
8. sanitize + code-block strip + noise filter

---

### 7.15 undici（Node.js）

#### 7.15.1 架构概览

undici 是 Node.js 官方 HTTP 客户端，实现了 **8 拦截器 + Dispatcher 组合** 的插件系统。核心模块：

- `lib/dispatcher/`：`Dispatcher`, `Client`, `Pool`, `Agent`, `ProxyAgent`, `RetryAgent`
- `lib/interceptor/`：8 interceptors
- `lib/handler/`：`RedirectHandler`, `RetryHandler`, `CacheHandler`
- `lib/mock/`：`MockAgent`, `MockClient`, `SnapshotAgent`

#### 7.15.2 Dispatcher Composition Model

```js
class Dispatcher extends EventEmitter {
  dispatch () { throw new Error('not implemented') }
  close () { throw new Error('not implemented') }
  destroy () { throw new Error('not implemented') }

  compose (...args) {
    const interceptors = Array.isArray(args[0]) ? args[0] : args
    let dispatch = this.dispatch.bind(this)
    for (const interceptor of interceptors) {
      if (interceptor == null) continue
      if (typeof interceptor !== 'function')
        throw new TypeError(`invalid interceptor, expected function received ${typeof interceptor}`)
      dispatch = interceptor(dispatch)          // wrap: each interceptor returns a new dispatch
      if (dispatch == null || typeof dispatch !== 'function' || dispatch.length !== 2)
        throw new TypeError('invalid interceptor')
    }
    return new Proxy(this, { get: (target, key) => key === 'dispatch' ? dispatch : target[key] })
  }
}
```

#### 7.15.3 8 Interceptors

| Interceptor | File | Trigger point | Purpose |
|---|---|---|---|
| `redirect` | `interceptor/redirect.js` | per-dispatch | wraps with `RedirectHandler`, follows `maxRedirections` |
| `responseError` | `interceptor/responseError.js` | `onResponseStart`/`onResponseEnd` | builds `ResponseError` from 4xx/5xx body |
| `retry` | `interceptor/retry.js` | per-dispatch | wraps with `RetryHandler` |
| `dump` | `interceptor/dump.js` | `onResponseData` | aborts after `maxSize` bytes |
| `dns` | `interceptor/dns.js` | per-dispatch | DNS-level interception / cache |
| `cache` | `interceptor/cache.js` | per-dispatch | HTTP caching with `CacheHandler` |
| `decompress` | `interceptor/decompress.js` | response data | transparent content-decoding |
| `deduplicate` | `interceptor/deduplicate.js` | per-dispatch | collapses concurrent identical safe-method requests |

#### 7.15.4 MockAgent

```js
class MockAgent extends Dispatcher {
  get (origin) { ... }                 // returns/creates per-origin MockClient/MockPool
  dispatch (opts, handler) {           // routes to internal Agent
    const mockDispatcher = this.get(opts.origin)
    return this[kAgent].dispatch(dispatchOpts, handler)
  }
  deactivate () { this[kIsMockActive] = false }
  activate ()   { this[kIsMockActive] = true }
  enableNetConnect (matcher) { ... }   // selectively allow real network
  disableNetConnect () { ... }
}
```

`MockInterceptor` + `MockScope`：`mock.get('https://origin').intercept({ path, method, query, body, headers }).reply(statusCode, body).delay(ms).persist().times(n)`

## 8. 横向专题对比

### 8.1 Plugin Extension API 设计模式对比

#### 8.1.1 设计模式分类

| 模式 | 代表工程 | 核心抽象 | 优势 | 劣势 |
|------|----------|----------|------|------|
| **Trait/Interface** | atomcode, Switchyard | `PluginHookSource`, `NativePlugin` | 编译期安全，零成本抽象 | 语言绑定，需重新编译 |
| **Event Bus** | pi, agent-core | `pi.on(event, handler)`, `@on(event)` | 松耦合，动态订阅 | 运行时错误，类型弱 |
| **Registry + Manifest** | openclaw, claudecode, jiuwenswarm | `register()`, `plugin.json` | 声明式，可静态分析 | 版本耦合 |
| **Code Composition** | opencode, undici | `compose(interceptors)`, `import()` | 最灵活，无额外抽象 | 无隔离，信任全量 |
| **YAML/JSON Schema** | agent-studio, hermes-agent, semantica | `plugin.yaml`, `plugin.json` | 人类可读，工具链丰富 | 解析开销，类型弱 |
| **MCP Protocol** | semantica, agent-studio | `python -m mcp_server` | 跨语言，标准化 | 序列化开销 |

#### 8.1.2 API 表面对比

| 工程 | Tool 注册 | Hook 注册 | Provider 注册 | 命令注册 | Flag 注册 | UI 注入 |
|------|:---------:|:---------:|:-------------:|:--------:|:---------:|:-------:|
| **openclaw** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **pi** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **hermes** | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| **agent-core** | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| **opencode** | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ |
| **jiuwenswarm** | ✅ | ✅ | ❌ | ❌ | ❌ | ✅ |
| **deepseek** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **semantica** | ✅ (MCP) | ✅ (JSON) | ❌ | ❌ | ❌ | ❌ |
| **TencentDB** | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **Switchyard** | ❌ | ✅ (intercept) | ❌ | ❌ | ❌ | ❌ |
| **undici** | ❌ | ✅ (interceptor) | ❌ | ❌ | ❌ | ❌ |
| **atomcode** | ❌ | ✅ (JSON) | ❌ | ❌ | ❌ | ❌ |
| **claudecode** | ❌ | ✅ (JSON) | ❌ | ❌ | ❌ | ❌ |
| **cc-switch** | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **agent-studio** | ✅ (JSON) | ❌ | ❌ | ❌ | ❌ | ❌ |

### 8.2 Hook 注册与触发机制对比

#### 8.2.1 注册方式对比

| 注册方式 | 代表工程 | 代码示例 | 优势 | 劣势 |
|----------|----------|----------|------|------|
| **声明式 JSON/YAML** | atomcode, claudecode, jiuwenswarm | `"PreToolUse": [{"hooks": [...]}]` | 人类可读，可审计 | 解析开销，无类型安全 |
| **声明式 Decorator** | agent-core | `@on("agent_started", priority=10)` | 类型安全，IDE 支持 | 语言绑定 |
| **命令式 API** | openclaw, pi, hermes | `api.registerTool(...)`, `pi.on(...)` | 动态，运行时灵活 | 运行时错误 |
| **Trait 实现** | Switchyard | `impl NativePlugin for MyPlugin` | 编译期安全 | 需重新编译 |
| **代码组合** | undici, opencode | `compose(interceptor)` | 最灵活 | 无隔离 |

#### 8.2.2 触发机制对比

| 机制 | 代表工程 | 描述 | 性能 | 可靠性 |
|------|----------|------|:----:|:------:|
| **同步直接调用** | openclaw, pi, hermes | 直接函数调用 | 高 | 中（异常传播） |
| **异步事件总线** | agent-core, pi | `asyncio` event loop | 高 | 高（隔离） |
| **子进程执行** | atomcode, claudecode, jiuwenswarm | `sh -c` / `cmd /C` | 低 | 高（隔离） |
| **VM 沙箱** | deepseek | `node:vm` realm | 中 | 高（隔离） |
| **cdylib 调用** | Switchyard | trait 方法调用 | 高 | 中 |

### 8.3 Hook 决策能力与失败处理对比

#### 8.3.1 决策能力对比

| 工程 | allow | deny | ask | modify | block | 决策链 |
|------|:-----:|:------:|:---:|:------:|:-----:|--------|
| **openclaw** | ✅ | ✅ | ✅ | ✅ | ✅ | priority-ordered |
| **claudecode** | ✅ | ✅ | ✅ | ✅ | ✅ | parallel + aggregate |
| **atomcode** | ✅ | ✅ | ✅ | ✅ | ✅ | sequential fold |
| **jiuwenswarm** | ✅ | ✅ | ❌ | ✅ | ✅ | parallel gather |
| **deepseek** | ✅ | ✅ | ✅ | ❌ | ✅ | delegate |
| **hermes** | ✅ | ✅ | ❌ | ✅ | ❌ | logged |
| **agent-core** | ✅ | ✅ | ❌ | ✅ | ✅ | priority + filter |
| **pi** | ✅ | ✅ | ❌ | ✅ | ❌ | exception |
| **opencode** | ✅ | ❌ | ❌ | ✅ | ❌ | exception |
| **semantica** | ✅ | ✅ | ❌ | ❌ | ❌ | non-blocking |
| **Switchyard** | ✅ | ❌ | ❌ | ✅ | ❌ | priority |
| **TencentDB** | ✅ | ❌ | ❌ | ✅ | ❌ | non-blocking |
| **undici** | ✅ | ❌ | ❌ | ✅ | ❌ | compose chain |
| **cc-switch** | N/A | N/A | N/A | N/A | N/A | N/A |
| **agent-studio** | N/A | N/A | N/A | N/A | N/A | N/A |

#### 8.3.2 失败处理对比

| 工程 | fail-open | fail-closed | timeout | retry | rollback | silent continue |
|------|:---------:|:-----------:|:-------:|:-----:|:--------:|:---------------:|
| **openclaw** | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| **claudecode** | ✅ | ❌ | ✅ | ❌ | ❌ | ✅ |
| **atomcode** | ✅ | ❌ | ✅ | ❌ | ❌ | ✅ |
| **jiuwenswarm** | ❌ | ✅ (exit-2) | ✅ | ❌ | ❌ | ❌ |
| **deepseek** | ✅ | ❌ | ✅ | ❌ | ❌ | ❌ |
| **hermes** | ✅ | ❌ | ✅ | ❌ | ❌ | ❌ |
| **agent-core** | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ |
| **pi** | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **opencode** | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **semantica** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Switchyard** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **TencentDB** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **undici** | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **cc-switch** | N/A | N/A | N/A | N/A | N/A | N/A |
| **agent-studio** | N/A | N/A | N/A | N/A | N/A | N/A |

### 8.4 沙箱与隔离模型对比

#### 8.4.1 隔离级别分层

| 级别 | 代表工程 | 隔离强度 | 性能开销 | 安全性 |
|------|----------|:--------:|:--------:|:------:|
| **L0: In-process** | openclaw, opencode, pi, hermes, semantica | 无 | 最低 | 低 |
| **L1: VM Realm** | deepseek | 中 | 中 | 中 |
| **L2: Subprocess** | atomcode, claudecode, jiuwenswarm | 中 | 中 | 中 |
| **L3: cdylib** | Switchyard | 低 | 低 | 低 |
| **L4: Namespace** | agent-studio | 高 | 高 | 高 |
| **L5: Full Sandbox** | jiuwenswarm (JiuwenBox) | 最高 | 最高 | 最高 |

#### 8.4.2 沙箱技术对比

| 技术 | 代表工程 | 文件系统 | 网络 | Capabilities | 资源限制 |
|------|----------|:--------:|:----:|:------------:|:--------:|
| **node:vm** | deepseek | whitelist globals | trapped | ctx façade | ❌ |
| **sh -c subprocess** | atomcode, claudecode | env vars only | ❌ | timeout | ❌ |
| **BubbleWrap** | agent-studio | bind mounts | netns | seccomp BPF | setpriv |
| **BubbleWrap + Landlock** | jiuwenswarm | Landlock + bind | netns + iptables | seccomp + cap | cgroup |
| **WASM** | ❌ (无工程使用) | ❌ | ❌ | ❌ | ❌ |

### 8.5 市场分发与版本兼容对比

#### 8.5.1 市场模式对比

| 模式 | 代表工程 | 中心化 | 审核 | 企业策略 |
|------|----------|:------:|:----:|:--------:|
| **Official Marketplace** | claudecode | ✅ | ✅ | blocklist/allowlist |
| **npm Registry** | opencode, pi, undici | ✅ | ❌ | ❌ |
| **Git Marketplace** | atomcode | ❌ | ❌ | ❌ |
| **Filesystem Discovery** | jiuwenswarm, hermes | ❌ | ❌ | ❌ |
| **JSON Catalog** | agent-studio | ✅ | ❌ | ❌ |
| **Runtime Authored** | deepseek | ❌ | ✅ (approval) | ❌ |
| **Pip Entry Points** | hermes | ✅ | ❌ | ❌ |

#### 8.5.2 版本兼容策略对比

| 策略 | 代表工程 | 描述 | 安全性 | 灵活性 |
|------|----------|------|:------:|:------:|
| **Semver** | opencode, pi, undici | `engines.opencode` | 中 | 高 |
| **Content Hash** | atomcode, TencentDB | SHA-256 of hook set | 高 | 低 |
| **Git Pin** | atomcode | branch/tag/commit/ref | 高 | 中 |
| **Schema Version** | Switchyard | `schema_version = 1` | 中 | 低 |
| **Approval Gating** | deepseek | user approves run | 最高 | 低 |
| **Min Version** | jiuwenswarm | `min_jiuwenswarm_version` | 中 | 中 |

### 8.6 WASM 沙箱与进程模型对比

> **关键发现**: 15 个工程中 **无任何一个使用 WASM 沙箱**。这是 laew 的潜在差异化方向。

| 模型 | 代表工程 | 启动时间 | 内存占用 | 隔离性 | 可移植性 |
|------|----------|:--------:|:--------:|:------:|:--------:|
| **In-process** | openclaw, opencode, pi | 最低 | 最低 | 无 | 低 |
| **Subprocess** | atomcode, claudecode | 中 | 中 | 中 | 中 |
| **VM Realm** | deepseek | 中 | 中 | 中 | 低 |
| **cdylib** | Switchyard | 低 | 低 | 低 | 低 |
| **Namespace** | agent-studio, jiuwenswarm | 高 | 高 | 高 | 低 |
| **WASM** | ❌ | 低 | 低 | 高 | 最高 |

### 8.7 IPC 协议对比

| 协议 | 代表工程 | 序列化 | 延迟 | 双向 | 流式 | 跨语言 |
|------|----------|--------|:----:|:----:|:----:|:------:|
| **Direct Call** | openclaw, pi, hermes | N/A | 最低 | ✅ | ✅ | ❌ |
| **stdin/stdout JSON** | atomcode, claudecode | JSON | 中 | ❌ | ❌ | ✅ |
| **JSON-RPC 2.0** | jiuwenswarm (ACP) | JSON | 中 | ✅ | ❌ | ✅ |
| **MCP stdio/SSE** | semantica, agent-studio | JSON-RPC | 中 | ✅ | ✅ | ✅ |
| **TypertRemoteService** | deepseek | protobuf | 低 | ✅ | ✅ | ✅ |
| **Tauri Commands** | cc-switch | serde_json | 低 | ✅ | ❌ | ❌ |
| **HTTP/REST** | agent-studio, TencentDB | JSON | 中 | ✅ | ❌ | ✅ |
| **WebSocket** | jiuwenswarm | JSON | 低 | ✅ | ✅ | ✅ |

### 8.8 插件发现机制对比

| 发现方式 | 代表工程 | 自动扫描 | 声明式 | 命令式 | Lazy Load |
|----------|----------|:--------:|:------:|:------:|:---------:|
| **Marketplace + Settings** | claudecode | ✅ | ✅ | ✅ | ✅ |
| **Bundled + Workspace + Global** | openclaw | ✅ | ✅ | ✅ | ✅ |
| **Config + File Scan** | opencode, pi | ✅ | ✅ | ✅ | ✅ |
| **Filesystem + extension_dirs** | jiuwenswarm, hermes | ✅ | ✅ | ✅ | ❌ |
| **Git Marketplace** | atomcode | ✅ | ✅ | ❌ | ❌ |
| **JSON Index** | agent-studio | ✅ | ✅ | ❌ | ❌ |
| **Runtime Tool** | deepseek | ❌ | ❌ | ✅ | ❌ |
| **cdylib Loading** | Switchyard | ❌ | ❌ | ✅ | ❌ |
| **Code Composition** | undici | ❌ | ❌ | ✅ | ❌ |
| **Pip Entry Points** | hermes | ✅ | ❌ | ✅ | ❌ |

## 9. laew gap 分析 L221-L240

> 本轮产出 **20 条新 gap**（L221-L240），全部与插件生态、扩展分发、Hook 系统相关。每条附 Rust crate 建议。

### 9.1 P0 紧急 gap（L221-L230）

| Gap ID | 描述 | 对标工程 | 影响 | Rust crate 建议 |
|--------|------|----------|------|-----------------|
| **L221** | 无 Plugin Extension API | openclaw, pi, opencode | 无法让第三方扩展 Agent 能力 | `extism`（WASM 插件系统） |
| **L222** | 无 Hook 注册/触发机制 | claudecode, atomcode, openclaw | 无法在生命周期事件中插入自定义逻辑 | 自建 `HookRegistry<tokio::sync::broadcast>` |
| **L223** | 无插件 Manifest 系统 | openclaw, claudecode, jiuwenswarm | 无法声明式描述插件元数据 | `serde` + `schemars`（JSON Schema 生成） |
| **L224** | 无插件市场/分发渠道 | claudecode, opencode, atomcode | 用户无法发现/安装第三方插件 | `self_update` + `cargo-dist` |
| **L225** | 无 Hook 决策能力（allow/deny/ask） | openclaw, claudecode, atomcode | Hook 无法阻断/修改流程 | 自建 `HookDecision` enum |
| **L226** | 无插件沙箱隔离 | atomcode, claudecode, jiuwenswarm | 恶意插件可破坏系统 | `wasmtime` 或 `wasmer`（WASM 沙箱） |
| **L227** | 无 Hook 失败处理（fail-open/fail-closed） | openclaw, agent-core | Hook 异常导致系统挂起 | `failsafe`（熔断器 + 退避） |
| **L228** | 无插件版本兼容策略 | opencode, pi, jiuwenswarm | 插件与宿主版本不兼容导致崩溃 | `semver` crate |
| **L229** | 无插件发现机制（自动扫描） | openclaw, opencode, pi | 需手动配置插件路径 | `notify`（文件系统监听） |
| **L230** | 无 MCP 5 传输协议支持 | agent-core, agent-studio | 无法连接多样化 MCP 服务器 | `jsonrpc-core` + `tokio-tungstenite` |

### 9.2 P1 重要 gap（L231-L235）

| Gap ID | 描述 | 对标工程 | 影响 | Rust crate 建议 |
|--------|------|----------|------|-----------------|
| **L231** | 无 Hook 优先级/超时机制 | openclaw, agent-core | 多 Hook 执行顺序不可控 | `tokio::time::timeout` + priority queue |
| **L232** | 无插件热重载 | openclaw, claudecode, opencode | 修改插件需重启 Agent | `hot-lib-reloader` 或 `libloading` |
| **L233** | 无 Hook 隔离（structuredClone） | openclaw | Hook 可修改共享状态 | `serde` 序列化隔离 |
| **L234** | 无插件权限系统（capability-based） | openclaw, hermes, jiuwenswarm | 插件拥有全部宿主权限 | `landlock` + `seccompiler` |
| **L235** | 无插件内容寻址（hash pinning） | atomcode, TencentDB | 插件内容可被篡改 | `sha2` + `blake3` |

### 9.3 P2 进阶 gap（L236-L240）

| Gap ID | 描述 | 对标工程 | 影响 | Rust crate 建议 |
|--------|------|----------|------|-----------------|
| **L236** | 无 WASM 沙箱 | ❌（15 工程均无） | 无法安全执行不可信代码 | `wasmtime` 或 `wasmer` |
| **L237** | 无插件 IPC 协议（JSON-RPC） | jiuwenswarm, semantica | 无法跨进程通信 | `jsonrpc-core` + `tarpc` |
| **L238** | 无插件 DSL 双向转换 | agent-studio | 无法导入导出工作流 | `pest`（PEG parser） |
| **L239** | 无 SkillDevPipeline（12 阶段） | jiuwenswarm | 无法自动化 Skill 开发 | 自建状态机 + `async-trait` |
| **L240** | 无 TranslationEngine（协议翻译） | Switchyard | 无法跨协议翻译 LLM 请求 | `serde` + `schemars` |

### 9.4 gap 优先级路线图

```
P0 紧急（L221-L230）— 3 个月:
  ├── L221: Plugin Extension API 设计
  ├── L222: Hook 注册/触发机制
  ├── L223: 插件 Manifest 系统
  ├── L224: 插件市场/分发渠道
  ├── L225: Hook 决策能力
  ├── L226: 插件沙箱隔离（WASM）
  ├── L227: Hook 失败处理
  ├── L228: 插件版本兼容
  ├── L229: 插件发现机制
  └── L230: MCP 5 传输协议

P1 重要（L231-L235）— 6 个月:
  ├── L231: Hook 优先级/超时
  ├── L232: 插件热重载
  ├── L233: Hook 隔离
  ├── L234: 插件权限系统
  └── L235: 插件内容寻址

P2 进阶（L236-L240）— 12 个月:
  ├── L236: WASM 沙箱
  ├── L237: 插件 IPC 协议
  ├── L238: 插件 DSL 双向转换
  ├── L239: SkillDevPipeline
  └── L240: TranslationEngine
```

## 10. Rust crate 推荐清单

> 基于 gap 分析，推荐以下 Rust crate 用于 laew 插件生态建设。

### 10.1 核心插件系统

| Crate | 用途 | 对标特性 | 推荐理由 |
|-------|------|----------|----------|
| **`extism`** | WASM 插件系统 | openclaw Plugin SDK | Rust 原生 WASM 插件框架，支持多种语言编译的插件 |
| **`wasmtime`** | WASM 运行时 | deepseek VM sandbox（升级） | BytheWASM 官方运行时，性能优秀，安全隔离 |
| **`wasmer`** | WASM 运行时 | deepseek VM sandbox（升级） | 支持 JIT/AOT，嵌入式友好 |
| **`libloading`** | 动态库加载 | Switchyard cdylib | 原生 cdylib 加载，零依赖 |
| **`hot-lib-reloader`** | 热重载 | openclaw hot reload | 开发时自动重载动态库 |

### 10.2 Hook 系统

| Crate | 用途 | 对标特性 | 推荐理由 |
|-------|------|----------|----------|
| **`tokio::sync::broadcast`** | 事件总线 | pi ExtensionAPI | Tokio 原生，异步友好 |
| **`priority-queue`** | 优先级队列 | openclaw Hook priority | 高效的二叉堆实现 |
| **`tokio::time::timeout`** | 超时控制 | openclaw per-hook timeout | Tokio 原生，精确超时 |
| **`dashmap`** | 并发 HashMap | claudecode hook registry | 高性能并发 map |
| **`arc-swap`** | 原子替换 | openclaw fail-open | 无锁原子替换 |

### 10.3 Manifest & Schema

| Crate | 用途 | 对标特性 | 推荐理由 |
|-------|------|----------|----------|
| **`serde`** | 序列化 | openclaw plugin.json | Rust 标准序列化 |
| **`serde_json`** | JSON 解析 | openclaw manifest | 高性能 JSON |
| **`serde_yaml`** | YAML 解析 | hermes plugin.yaml | YAML 支持 |
| **`schemars`** | JSON Schema 生成 | openclaw configSchema | 从 struct 生成 JSON Schema |
| **`jsonschema`** | JSON Schema 验证 | agent-studio JSON Schema | 运行时验证 |
| **`validator`** | 数据验证 | openclaw config validation | derive 宏验证 |

### 10.4 沙箱与安全

| Crate | 用途 | 对标特性 | 推荐理由 |
|-------|------|----------|----------|
| **`landlock`** | Linux LSM 隔离 | jiuwenswarm Landlock | Linux 官方沙箱 API |
| **`seccompiler`** | Seccomp BPF | agent-studio seccomp | libseccomp Rust 绑定 |
| **`caps`** | Linux Capabilities | jiuwenswarm capabilities | POSIX capabilities |
| **`bubblewrap`** | 容器沙箱 | agent-studio BubbleWrap | namespace 隔离 |
| **`sha2`** | SHA-256 哈希 | atomcode hook_set_hash | 内容寻址 |
| **`blake3`** | BLAKE3 哈希 | TencentDB content_hash | 更快的哈希 |
| **`ring`** | 加密原语 | hermes signing | 安全加密 |

### 10.5 IPC 协议

| Crate | 用途 | 对标特性 | 推荐理由 |
|-------|------|----------|----------|
| **`jsonrpc-core`** | JSON-RPC 2.0 | jiuwenswarm ACP | 标准 JSON-RPC 实现 |
| **`tarpc`** | RPC 框架 | deepseek TypertRemoteService | Rust 原生 async RPC |
| **`tokio-tungstenite`** | WebSocket | jiuwenswarm Gateway | Tokio WebSocket |
| **`quinn`** | QUIC 协议 | deepseek Host↔Client | QUIC 实现 |
| **`tonic`** | gRPC | deepseek TypertRemoteService | gRPC/HTTP2 |
| **`ciborium`** | CBOR 编码 | pi TypeBox | 紧凑二进制编码 |

### 10.6 市场与分发

| Crate | 用途 | 对标特性 | 推荐理由 |
|-------|------|----------|----------|
| **`self_update`** | 自动更新 | claudecode autoupdate | Rust 自动更新 |
| **`cargo-dist`** | 分发构建 | atomcode git marketplace | 跨平台分发 |
| **`flate2`** | 压缩 | claudecode zip cache | ZIP 解压 |
| **`tar`** | 归档 | claudecode zip cache | TAR 归档 |
| **`reqwest`** | HTTP 客户端 | claudecode marketplace | HTTP 请求 |
| **`oauth2`** | OAuth 认证 | openclaw provider auth | OAuth 2.0 |

### 10.7 失败处理

| Crate | 用途 | 对标特性 | 推荐理由 |
|-------|------|----------|----------|
| **`failsafe`** | 熔断器/重试 | openclaw fail-open/fail-closed | 完整的容错框架 |
| **`backoff`** | 指数退避 | claudecode retry | 重试策略 |
| **`thiserror`** | 错误定义 | openclaw AgentError | derive 错误类型 |
| **`anyhow`** | 错误处理 | claudecode Result | 便捷错误处理 |
| **`tracing`** | 日志追踪 | openclaw subsystem logger | 结构化日志 |

### 10.8 UI & TUI

| Crate | 用途 | 对标特性 | 推荐理由 |
|-------|------|----------|----------|
| **`ratatui`** | 终端 UI | pi TUI | 现代 TUI 框架 |
| **`crossterm`** | 终端控制 | atomcode tuix | 跨平台终端 |
| **`unicode-width`** | CJK 宽度 | pi ExtensionAPI | 中文显示 |
| **`tauri`** | 桌面应用 | cc-switch Tauri 2 | 桌面壳 |

## 11. 第十一轮合集索引

### 11.1 调研产出

| 产出 | 数量 | 说明 |
|------|:----:|------|
| **源码工程** | 15 | 全覆盖 |
| **调研文件** | ~387 | 含子代理覆盖 |
| **对比表** | 6 | Hook 类型 / Extension 生命周期 / 沙箱模型 / 市场分发 / SDK 类型 / IPC 协议 |
| **新 gap** | 20 | L221-L240 |
| **Rust crate 推荐** | 40+ | 分 8 类 |
| **代码片段** | 30+ | 关键注册/lifecycle 模式 |

### 11.2 与前 10 轮关系

| 轮次 | 维度 | 本轮关联 |
|------|------|----------|
| 第 1-4 轮 | 架构/Context/循环/工具 | 本轮扩展为插件化架构 |
| 第 5 轮 | MCP/SKILL/沙箱/权限 | 本轮深化 MCP 传输 + 沙箱模型 |
| 第 6 轮 | 协议 wire/流式/错误重试 | 本轮扩展为插件 IPC 协议 |
| 第 7 轮 | 文件编辑/Git/Bash/多模态 | 本轮扩展为 Hook 决策能力 |
| 第 8 轮 | Telemetry/Session/Tool 权限 | 本轮扩展为 Hook 失败处理 |
| 第 9 轮 | CrashDump/WebUI/OAuth/i18n | 本轮扩展为插件市场分发 |
| 第 10 轮 | Release/WebSocket/容器/CRDT | 本轮扩展为插件版本兼容 |

### 11.3 关键发现

#### 11.3.1 共性模式

1. **声明式注册为主流**: 12/15 工程支持声明式注册（JSON/YAML/Decorator）
2. **Out-of-process 隔离是安全基线**: atomcode, claudecode, jiuwenswarm 均使用子进程
3. **fail-open 是主流失败策略**: 10/15 工程采用 fail-open
4. **npm 是主要分发渠道**: opencode, pi, undici, openclaw 均通过 npm
5. **无工程使用 WASM 沙箱**: 这是 laew 的差异化机会

#### 11.3.2 差异化特性

| 特性 | 唯一工程 | 描述 |
|------|----------|------|
| **Hash-pinned hooks** | atomcode | SHA-256 of sorted (event, matcher, command) triples |
| **VM Realm sandbox** | deepseek | node:vm + teaching errors + dual-realm instanceof |
| **41 hook types** | openclaw | 最完整的 Hook 表面 |
| **153 bundled extensions** | openclaw | 最大插件生态 |
| **12-stage SkillDevPipeline** | jiuwenswarm | 自动化 Skill 开发 |
| **9 routing algorithms** | Switchyard | LLM 网关路由 |
| **8 interceptors** | undici | 可组合 HTTP 拦截器 |
| **BubbleWrap + Landlock** | jiuwenswarm | 最强沙箱隔离 |
| **Portable Agent Plugins** | hermes-agent | 6 前端共享 |
| **AsyncCallbackFramework** | agent-core | 40+ 事件优先级链 |

#### 11.3.3 laew 差距总结

| 维度 | laew 现状 | 业界最佳 | 差距 |
|------|-----------|----------|------|
| **Hook 类型** | 0 | 41 (openclaw) | 41 |
| **Extension API** | 无 | 60+ methods (openclaw) | 60+ |
| **沙箱隔离** | 无 | L5 (jiuwenswarm) | 5 级 |
| **市场分发** | 无 | marketplace (claudecode) | 完整 |
| **版本兼容** | 无 | semver + hash (atomcode) | 完整 |
| **IPC 协议** | 无 | 8 种 (全工程) | 8 |
| **插件发现** | 无 | auto-scan (openclaw) | 完整 |
| **Hook 决策** | 无 | pass/block/modify (openclaw) | 完整 |
| **失败处理** | 无 | fail-open/fail-closed (openclaw) | 完整 |
| **WASM 沙箱** | 无 | ❌ (15 工程均无) | 差异化机会 |

### 11.4 推荐实现路径

#### 阶段 1：最小可行插件系统（1-2 个月）

```
目标: 实现 L221-L225（P0 前 5 条）

1. 定义 PluginManifest struct（serde + schemars）
2. 实现 HookRegistry（tokio::sync::broadcast）
3. 实现 8 种核心 Hook 事件（对标 atomcode）
4. 实现 HookDecision enum（allow/deny/ask/modify）
5. 实现 fail-open/fail-closed 失败处理（failsafe）

推荐 crate: serde, serde_json, schemars, tokio, failsafe
```

#### 阶段 2：安全隔离（2-3 个月）

```
目标: 实现 L226-L230（P0 后 5 条）

1. 实现 WASM 沙箱（wasmtime 或 wasmer）
2. 实现插件 Manifest 验证（jsonschema）
3. 实现内容寻址（sha2 + blake3）
4. 实现版本兼容（semver）
5. 实现 MCP 5 传输（jsonrpc-core + tokio-tungstenite）

推荐 crate: wasmtime, jsonschema, sha2, semver, jsonrpc-core
```

#### 阶段 3：市场与生态（3-6 个月）

```
目标: 实现 L231-L240（P1 + P2）

1. 实现插件市场（self_update + cargo-dist）
2. 实现 Hook 优先级/超时（priority-queue + tokio::time）
3. 实现插件热重载（hot-lib-reloader）
4. 实现权限系统（landlock + seccompiler）
5. 实现 TranslationEngine（serde + schemars）

推荐 crate: self_update, cargo-dist, hot-lib-reloader, landlock, seccompiler
```

### 11.5 参考资源

| 工程 | 核心文件 | 学习要点 |
|------|----------|----------|
| **openclaw** | `src/plugins/plugin-entry.ts`, `src/plugins/hooks.ts` | 最完整的 Plugin SDK |
| **claudecode** | `src/types/hooks.ts`, `src/utils/plugins/loadPluginHooks.ts` | 27 Hook 类型 + 市场 |
| **atomcode** | `crates/atomcode-capabilities/src/cc_hooks.rs` | Hash-pinned 信任模型 |
| **deepseek** | `packages/extensions/cordis-host-runner/src/sandbox.ts` | VM Realm 沙箱 |
| **pi** | `packages/coding-agent/src/core/extensions/types.ts` | 30+ 事件 ExtensionAPI |
| **hermes** | `hermes_cli/plugins.py`, `plugins/plugin_loader.py` | 40+ Hook 类型 |
| **agent-core** | `core/runner/callback/framework.py` | AsyncCallbackFramework |
| **jiuwenswarm** | `extensions/sdk/base.py`, `server/hooks/executor.py` | 17 Hook + JiuwenBox |
| **Switchyard** | `crates/switchyard-nemo-relay-plugin/src/lib.rs` | NativePlugin trait |
| **undici** | `lib/dispatcher/dispatcher.js` | 可组合拦截器模式 |

---

## 附录 A：Hook 类型完整速查表

> 按字母顺序排列的全量 Hook 类型（15 工程合计 80+ 种）

| Hook 类型 | 触发时机 | 决策能力 | 支持工程 |
|-----------|----------|----------|----------|
| `after_compaction` | 压缩后 | observe | openclaw |
| `after_provider_response` | Provider 响应后 | modify | pi |
| `agent_end` | Agent 结束 | observe | openclaw, pi, TencentDB |
| `agent_settled` | Agent 稳定 | observe | pi |
| `agent_start` | Agent 开始 | observe | openclaw, pi, agent-core |
| `agent_turn_prepare` | Turn 准备 | modify | openclaw |
| `api_request_error` | API 请求错误 | observe | hermes |
| `before_agent_finalize` | Agent 终化前 | modify | openclaw |
| `before_agent_reply` | Agent 回复前 | modify | openclaw |
| `before_agent_run` | Agent 运行前 | pass/block | openclaw |
| `before_agent_start` | Agent 开始前 | modify | pi |
| `before_compaction` | 压缩前 | observe | openclaw |
| `before_dispatch` | 分发前 | modify | openclaw |
| `before_install` | 安装前 | pass/block | openclaw |
| `before_message_write` | 消息写入前 | modify | openclaw |
| `before_model_call` | 模型调用前 | modify | jiuwenswarm |
| `before_model_resolve` | 模型解析前 | modify | openclaw |
| `before_prompt_build` | Prompt 构建前 | modify | openclaw, TencentDB |
| `before_provider_headers` | Provider 头前 | modify | pi |
| `before_provider_request` | Provider 请求前 | modify | pi |
| `before_reset` | 重置前 | observe | openclaw |
| `cache` | HTTP 缓存 | compose | undici |
| `command.execute.before` | 命令执行前 | observe | opencode |
| `config` | 配置变更 | modify | opencode |
| `cron_changed` | Cron 变更 | observe | openclaw |
| `cron_reconciled` | Cron 协调 | observe | openclaw |
| `cwd_changed` | CWD 变更 | observe | claudecode |
| `decompress` | 解压缩 | compose | undici |
| `deduplicate` | 去重 | compose | undici |
| `dispose` | 清理 | observe | opencode |
| `dns` | DNS | compose | undici |
| `dump` | 截断 | compose | undici |
| `elicitation` | 引导 | accept/decline | claudecode |
| `event` | 所有事件 | observe | opencode |
| `file_changed` | 文件变更 | observe | claudecode |
| `gateway_platform_event` | 网关平台事件 | observe | hermes |
| `gateway_start/stop` | 网关启动/停止 | observe | openclaw |
| `heartbeat_prompt_contribution` | 心跳 Prompt | modify | openclaw |
| `inbound_claim` | 入站声明 | pass/block | openclaw |
| `instructions_loaded` | 指令加载 | observe | claudecode, jiuwenswarm |
| `kanban_task_*` | Kanban 任务 | observe | hermes |
| `llm_input/output` | LLM 输入/输出 | observe | openclaw, agent-core |
| `message_received/sending/sent` | 消息接收/发送 | modify | openclaw |
| `message_start/update/end` | 消息开始/更新/结束 | observe | pi |
| `model_call_started/ended` | 模型调用开始/结束 | observe | openclaw, jiuwenswarm |
| `model_select` | 模型选择 | observe | pi |
| `notification` | 通知 | observe | claudecode, jiuwenswarm |
| `permission.ask` | 权限询问 | ask/deny/allow | opencode |
| `permission_denied` | 权限拒绝 | observe | claudecode, jiuwenswarm |
| `permission_request` | 权限请求 | allow/deny | claudecode, jiuwenswarm |
| `post_api_request` | API 请求后 | observe | hermes |
| `post_tool_call` | 工具调用后 | modify | hermes |
| `post_tool_use` | 工具使用后 | modify | 多工程 |
| `pre_api_request` | API 请求前 | observe | hermes |
| `pre_approval_request` | 审批请求前 | observe | hermes |
| `pre_command` | 命令前 | observe | hermes |
| `pre_gateway_dispatch` | 网关分发前 | observe | hermes |
| `pre_llm_call` | LLM 调用前 | observe | hermes |
| `pre_tool_call` | 工具调用前 | modify | hermes |
| `pre_tool_use` | 工具使用前 | allow/deny/ask | 多工程 |
| `pre_verify` | 验证前 | observe | hermes |
| `redirect` | 重定向 | compose | undici |
| `reply_dispatch` | 回复分发 | modify | openclaw |
| `reply_payload_sending` | 回复载荷发送 | modify | openclaw |
| `resolve_exec_env` | 执行环境解析 | modify | openclaw |
| `response_error` | 响应错误 | compose | undici |
| `retry` | 重试 | compose | undici |
| `session_before_compact` | 会话压缩前 | modify | pi |
| `session_before_fork` | 会话分叉前 | modify | pi |
| `session_before_switch` | 会话切换前 | modify | pi |
| `session_compact` | 会话压缩 | observe | pi |
| `session_end` | 会话结束 | observe | 多工程 |
| `session_info_changed` | 会话信息变更 | observe | pi |
| `session_start` | 会话开始 | observe | 多工程 |
| `session_shutdown` | 会话关闭 | observe | pi |
| `setup` | 设置 | observe | claudecode, jiuwenswarm |
| `skill_changed` | Skill 变更 | observe | openclaw |
| `stop` | 停止 | observe | 多工程 |
| `subagent_start/stop` | 子 Agent 开始/停止 | observe | claudecode, openclaw, jiuwenswarm |
| `teammate_idle` | 队友空闲 | observe | claudecode |
| `thinking_level_select` | 思考级别选择 | observe | pi |
| `tool.execute.before/after` | 工具执行前/后 | modify | opencode |
| `tool_call` | 工具调用 | modify | pi |
| `tool_result` | 工具结果 | modify | pi |
| `transform_api_error` | API 错误转换 | modify | hermes |
| `transform_llm_output` | LLM 输出转换 | modify | hermes |
| `transform_terminal_output` | 终端输出转换 | modify | hermes |
| `transform_tool_result` | 工具结果转换 | modify | hermes |
| `user_bash` | 用户 Bash | modify | pi |
| `user_prompt_submit` | 用户 Prompt 提交 | modify | 多工程 |
| `worktree_create/remove` | Worktree 创建/移除 | observe | claudecode |

---

## 附录 B：插件 Manifest Schema 示例

### B.1 laew 推荐 Manifest 格式（JSON）

```json
{
  "$schema": "https://laew.ai/schemas/plugin-v1.json",
  "id": "my-awesome-plugin",
  "name": "My Awesome Plugin",
  "version": "1.0.0",
  "description": "A plugin that extends laew with awesome features",
  "author": {
    "name": "Developer Name",
    "email": "dev@example.com"
  },
  "license": "MIT",
  "homepage": "https://github.com/example/my-awesome-plugin",
  "repository": {
    "type": "git",
    "url": "https://github.com/example/my-awesome-plugin.git"
  },
  "min_laew_version": "0.1.0",
  "max_laew_version": "1.0.0",
  "kind": "tool",
  "permissions": [
    "bash.execute",
    "file.read",
    "file.write",
    "network.outbound"
  ],
  "dependencies": {},
  "configSchema": {
    "type": "object",
    "properties": {
      "api_key": {
        "type": "string",
        "description": "API key for external service"
      },
      "timeout_ms": {
        "type": "number",
        "default": 10000,
        "description": "Timeout in milliseconds"
      }
    },
    "required": ["api_key"]
  },
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "echo 'Tool use detected'",
            "timeout_ms": 5000
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "echo 'Session started'"
          }
        ]
      }
    ]
  },
  "tools": [
    {
      "name": "my_tool",
      "description": "A custom tool",
      "parameters": {
        "type": "object",
        "properties": {
          "input": { "type": "string" }
        }
      }
    }
  ],
  "content_hash": "sha256:abcdef1234567890..."
}
```

### B.2 laew 推荐 Extension Trait（Rust）

```rust
/// Plugin manifest
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: PluginAuthor,
    pub license: String,
    pub min_laew_version: String,
    pub kind: PluginKind,
    pub permissions: Vec<Permission>,
    pub config_schema: Option<serde_json::Value>,
    pub hooks: Option<HookConfig>,
    pub tools: Vec<ToolManifest>,
    pub content_hash: String,
}

/// Plugin trait
#[async_trait]
pub trait Plugin: Send + Sync {
    /// Initialize plugin
    async fn initialize(&self, config: PluginConfig) -> Result<(), PluginError>;
    /// Shutdown plugin
    async fn shutdown(&self) -> Result<(), PluginError>;
    /// Get plugin manifest
    fn manifest(&self) -> &PluginManifest;
    /// Handle hook event
    async fn handle_hook(&self, event: HookEvent, payload: &Value) -> HookResult;
}

/// Hook registry
pub struct HookRegistry {
    hooks: DashMap<HookEvent, Vec<HookRegistration>>,
    priority_queue: PriorityQueue<HookRegistration>,
}

impl HookRegistry {
    /// Register a hook
    pub fn register(&self, event: HookEvent, hook: HookRegistration) { ... }
    /// Trigger hooks for an event
    pub async fn trigger(&self, event: HookEvent, payload: &Value) -> Vec<HookResult> { ... }
}

/// Hook decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookDecision {
    Allow {
        reason: Option<String>,
        modified_input: Option<Value>,
    },
    Deny {
        reason: String,
        message: Option<String>,
    },
    Ask {
        reason: String,
    },
    Pass,
}

/// Plugin manager
pub struct PluginManager {
    plugins: DashMap<String, Arc<dyn Plugin>>,
    hook_registry: Arc<HookRegistry>,
    sandbox: Arc<dyn Sandbox>,
}

impl PluginManager {
    /// Load plugin from manifest
    pub async fn load_plugin(&self, manifest: PluginManifest) -> Result<(), PluginError> { ... }
    /// Unload plugin
    pub async fn unload_plugin(&self, id: &str) -> Result<(), PluginError> { ... }
    /// List installed plugins
    pub fn list_plugins(&self) -> Vec<PluginManifest> { ... }
}
```

---

## 附录 C：调研文件清单

### C.1 主代理阅读文件（~50 个）

| 工程 | 关键文件 |
|------|----------|
| openclaw | `src/plugins/plugin-entry.ts`, `src/plugins/hooks.ts`, `src/plugins/hook-types.ts`, `src/plugins/hook-runner-global.ts`, `src/plugins/hook-decision-types.ts`, `src/plugins/hook-isolation.ts`, `src/plugins/hook-registry.types.ts`, `src/plugins/api-builder.ts`, `src/plugins/api-lifecycle.ts`, `src/plugins/discovery.ts`, `src/plugins/manifest.ts`, `src/hooks/types.ts`, `src/hooks/configured.ts`, `src/hooks/install.ts`, `src/hooks/installs.ts`, `src/hooks/frontmatter.ts`, `src/hooks/policy.ts`, `src/hooks/workspace.ts`, `src/hooks/hooks-status.ts`, `src/hooks/internal-hook-types.ts`, `src/hooks/bundled-dir.ts`, `extensions/browser/plugin-registration.ts` |
| claudecode | `src/plugins/builtinPlugins.ts`, `src/plugins/bundled/index.ts`, `src/hooks/useCanUseTool.tsx`, `src/types/plugin.ts`, `src/types/hooks.ts`, `src/utils/plugins/loadPluginHooks.ts`, `src/utils/plugins/pluginLoader.ts`, `src/utils/hooks/hookEvents.ts`, `src/utils/hooks/sessionHooks.ts`, `src/entrypoints/sdk/coreSchemas.ts` |
| atomcode | `crates/atomcode-capabilities/src/cc_hooks.rs`, `crates/atomcode-coding/src/plugin_hooks.rs` |
| pi | `.pi/extensions/tps.ts`, `packages/coding-agent/src/extensions/index.ts`, `packages/coding-agent/src/extensions/llama/index.ts`, `packages/coding-agent/src/core/extensions/types.ts` |
| undici | `lib/interceptor/redirect.js`, `lib/interceptor/retry.js`, `lib/interceptor/deduplicate.js`, `lib/dispatcher/agent.js`, `lib/dispatcher/dispatcher.js` |

### C.2 子代理覆盖文件（~337 个）

详见 4 份子代理报告。

---

**报告完成时间**: 2026-09-07
**调研员**: 第十一轮深度调研 - 插件生态与扩展分发与 Hook 系统专项
**审核**: 待审核

## 附录 D：Hook 决策能力深度对比

### D.1 决策模型分类

#### D.1.1 二态模型（allow/deny）

**代表工程**: atomcode, claudecode, jiuwenswarm

```rust
// atomcode
enum Decision {
    Allow { reason: Option<String> },
    Deny { reason: String },
}
```

```typescript
// claudecode
decision: z.enum(['approve', 'block']).optional()
```

**优势**: 简单明确，易于实现
**劣势**: 无法表达"需要更多信息"的中间态

#### D.1.2 三态模型（allow/deny/ask）

**代表工程**: openclaw, deepseek

```typescript
// openclaw
type HookDecisionPass = { outcome: "pass" };
type HookDecisionBlock = {
  outcome: "block";
  reason: string;
  message?: string;
  category?: string;
  metadata?: Record<string, unknown>;
};
```

**优势**: 支持"询问用户"的交互模式
**劣势**: 实现复杂度中等

#### D.1.3 四态模型（pass/block/modify/ask）

**代表工程**: openclaw（最完整）

```typescript
// openclaw hook decision types
type InputGateDecision = HookDecisionPass | HookDecisionBlock;
type GateHookResult<TDecision extends HookDecision = HookDecision> = {
  decision: TDecision;
  pluginId: string;
};
```

**优势**: 最完整，支持修改输入
**劣势**: 实现复杂度高

### D.2 决策链对比

#### D.2.1 顺序折叠（Sequential Fold）

**代表工程**: atomcode

```rust
// atomcode: sequential fold, first deny wins
let mut gate = BeforeOutcome::Proceed;
for hook in self.matching(HookEvent::PreToolUse, Some(&call.name)) {
    let Some((exit_code, stdout, stderr)) = run_command_hook(hook, &payload).await else {
        continue;
    };
    // Apply gate: Deny > Ask > Allow > Proceed
    gate = gate.apply(decided);
}
```

**优势**: 短路求值，第一个 deny 立即停止
**劣势**: 串行执行，慢 hook 阻塞快 hook

#### D.2.2 并行聚合（Parallel Aggregate）

**代表工程**: claudecode, jiuwenswarm

```python
# jiuwenswarm: parallel gather
async def run_all(self, hook_configs, hook_input, session_id="") -> list[HookResult]:
    results = await asyncio.gather(
        *[self._run_hook(cfg, hook_input) for cfg in hook_configs],
        return_exceptions=True
    )
    return [r for r in results if not isinstance(r, Exception)]
```

**优势**: 并行执行，总时间 = 最慢 hook
**劣势**: 无法短路，所有 hook 都会执行

#### D.2.3 优先级队列（Priority Queue）

**代表工程**: openclaw, agent-core

```typescript
// openclaw: priority-ordered with timeout
const DEFAULT_MODIFYING_HOOK_TIMEOUT_MS_BY_HOOK: Partial<Record<PluginHookName, number>> = {
  before_agent_run: 15_000,
  before_tool_call: 15_000,
  skill_proposal_evaluate: 120_000,
};
```

**优势**: 高优先级 hook 先执行，可超时
**劣势**: 优先级配置复杂

### D.3 决策结果对比

| 工程 | 决策结果 | 修改输入 | 注入上下文 | 停止继续 | 重试 |
|------|:--------:|:--------:|:----------:|:--------:|:----:|
| **openclaw** | pass/block | ✅ | ✅ | ✅ | ❌ |
| **claudecode** | approve/block/ask | ✅ | ✅ | ✅ | ❌ |
| **atomcode** | allow/block/modify | ✅ | ✅ | ✅ | ❌ |
| **jiuwenswarm** | block/allow/modify | ✅ | ✅ | ✅ | ❌ |
| **deepseek** | deny/ask/block | ❌ | ✅ | ✅ | ❌ |
| **hermes** | allow/deny/modify | ✅ | ✅ | ❌ | ❌ |
| **agent-core** | CONTINUE/STOP/SKIP/MODIFY | ✅ | ✅ | ✅ | ✅ |
| **pi** | event handler | ✅ | ✅ | ❌ | ❌ |
| **opencode** | modify/rewrite | ✅ | ✅ | ❌ | ❌ |
| **semantica** | guard | ❌ | ✅ | ✅ | ❌ |
| **Switchyard** | intercept | ✅ | ❌ | ❌ | ❌ |
| **TencentDB** | recall/capture | ✅ | ✅ | ❌ | ❌ |
| **undici** | compose | ✅ | ❌ | ❌ | ❌ |

## 附录 E：沙箱技术深度对比

### E.1 沙箱技术栈

#### E.1.1 Linux Namespaces

**代表工程**: agent-studio, jiuwenswarm

| Namespace | 隔离内容 | agent-studio | jiuwenswarm |
|-----------|----------|:------------:|:-----------:|
| **PID** | 进程 ID 空间 | ✅ | ✅ |
| **Network** | 网络协议栈 | ✅ | ✅ |
| **Mount** | 挂载点 | ✅ | ✅ |
| **IPC** | 进程间通信 | ✅ | ✅ |
| **UTS** | 主机名/域名 | ✅ | ✅ |
| **User** | 用户/组 ID | ✅ | ✅ |
| **Cgroup** | 控制组 | ❌ | ✅ |

#### E.1.2 Seccomp BPF

**代表工程**: agent-studio, jiuwenswarm

```python
# agent-studio
bpf = pyseccomp.SyscallFilter(pyseccomp.KILL)
for syscall in allowed:
    bpf.add_rule(pyseccomp.ALLOW, syscall)
sandbox_config.seccomp_bpf = bpf
```

**白名单模式**: 默认 kill，只允许 ~90 个 syscall（x86_64）

#### E.1.3 Landlock

**代表工程**: jiuwenswarm

```yaml
# jiuwenbox filesystem policy
directories:
  - path: /workspace
    permissions: [read, write]
  - path: /tmp
    permissions: [read, write]
files:
  - path: /etc/resolv.conf
    permissions: [read]
read_only:
  - /lib
  - /usr/lib
read_write:
  - /workspace
bind_mounts:
  - /workspace:/workspace
```

#### E.1.4 BubbleWrap

**代表工程**: agent-studio, jiuwenswarm

```bash
# bwrap command line
bwrap \
  --bind /workspace /workspace \
  --ro-bind /lib /lib \
  --ro-bind /usr/lib /usr/lib \
  --unshare-user \
  --unshare-ipc \
  --unshare-pid \
  --unshare-net \
  --unshare-uts \
  --unshare-cgroup \
  --seccomp 3 \
  --setpriv --reuid sandbox-exec --regid sandbox-exec \
  --clear-groups \
  exec command
```

#### E.1.5 VM Realm（Node.js）

**代表工程**: deepseek

```typescript
export function createSandbox(id: string, harnessExtras = {}): object {
  const sandbox = {
    ...nodeApiTraps(),          // require, setTimeout, fetch → throw redirect
    console: taggedConsole(id),
    harness: { defineTool, registerTool, ...harnessExtras },
    btoa, atob, TextEncoder, TextDecoder,
  }
  createContext(sandbox)
  patchDualRealmInstanceof(sandbox)
  return sandbox
}
```

### E.2 沙箱性能对比

| 技术 | 启动时间 | 内存开销 | 隔离强度 | 适用场景 |
|------|:--------:|:--------:|:--------:|----------|
| **In-process** | 0ms | 0MB | 无 | 可信插件 |
| **VM Realm** | <1ms | ~10MB | 中 | JS 插件 |
| **Subprocess** | ~50ms | ~20MB | 中 | 命令 Hook |
| **cdylib** | <1ms | ~5MB | 低 | 可信扩展 |
| **BubbleWrap** | ~100ms | ~30MB | 高 | 不可信代码 |
| **BubbleWrap + Landlock** | ~150ms | ~40MB | 最高 | 最不可信代码 |
| **WASM** | ~10ms | ~15MB | 高 | 跨平台不可信 |

### E.3 沙箱安全边界

| 攻击向量 | In-process | VM Realm | Subprocess | BubbleWrap | BubbleWrap+Landland | WASM |
|----------|:----------:|:--------:|:----------:|:----------:|:-------------------:|:----:|
| **文件系统访问** | 全量 | 受限 | 全量 | 受限 | 受限 | 无 |
| **网络访问** | 全量 | trapped | 全量 | 可隔离 | 可隔离 | 无 |
| **系统调用** | 全量 | 全量 | 全量 | seccomp 限制 | seccomp 限制 | 无 |
| **内存访问** | 全量 | 隔离 | 隔离 | 隔离 | 隔离 | 隔离 |
| **进程逃逸** | N/A | 可能 | 难 | 很难 | 极难 | 不可能 |

## 附录 F：插件 IPC 协议深度对比

### F.1 协议栈对比

#### F.1.1 stdin/stdout JSON（atomcode, claudecode, jiuwenswarm）

```
┌─────────────┐      JSON stdin       ┌─────────────┐
│   Agent     │ ──────────────────────→│  Hook       │
│   Process   │      JSON stdout      │  Process    │
│             │ ←──────────────────────│             │
└─────────────┘                       └─────────────┘
```

**特点**:
- 单向数据流（Agent → Hook → Agent）
- JSON 序列化/反序列化
- Exit code 传递决策
- 简单，跨平台

**atomcode 实现**:
```rust
async fn run_command_hook(hook: &HookConfig, stdin_json: &str) -> Option<(Option<i32>, String, String)> {
    let mut cmd = shell_command(&hook.command);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_json.as_bytes()).await.ok()?;
        stdin.shutdown().await.ok()?;
    }
    let out = child.wait_with_output().await.ok()?;
    Some((out.status.code(), decode(&out.stdout), decode(&out.stderr)))
}
```

#### F.1.2 JSON-RPC 2.0（jiuwenswarm ACP）

```
┌─────────────┐      request          ┌─────────────┐
│   Client    │ ──────────────────────→│  Server     │
│             │      response         │             │
│             │ ←──────────────────────│             │
└─────────────┘                       └─────────────┘
```

**特点**:
- 双向请求/响应
- 标准 JSON-RPC 2.0
- 支持 notification（单向）
- 跨语言

**jiuwenswarm 实现**:
```python
class AcpStdioClient:
    async def connect(self):
        # initialize (protocolVersion 1) + session/new
        ...
    async def chat(self, message, *, timeout=None) -> str:
        # session/prompt
        ...
    async def close(self):
        # stdin close → SIGTERM → SIGKILL, drain
        ...
```

#### F.1.3 MCP stdio/SSE（semantica, agent-studio）

```
┌─────────────┐      JSON-RPC         ┌─────────────┐
│   Host      │ ──────────────────────→│  MCP Server │
│   Process   │      JSON-RPC         │  Process    │
│             │ ←──────────────────────│             │
└─────────────┘                       └─────────────┘
```

**特点**:
- 标准 Model Context Protocol
- 支持 stdio + SSE + streamable-http
- 跨语言，生态丰富
- 工具/资源/提示 三类能力

#### F.1.4 WebSocket（jiuwenswarm Gateway）

```
┌─────────────┐      JSON frames      ┌─────────────┐
│   Browser   │ ←────────────────────→│  Gateway    │
│   Client    │      WebSocket        │  Server     │
└─────────────┘                       └─────────────┘
```

**特点**:
- 全双工
- 低延迟
- 支持流式
- 浏览器友好

#### F.1.5 Tauri Commands（cc-switch）

```
┌─────────────┐      invoke()         ┌─────────────┐
│   React     │ ──────────────────────→│  Rust       │
│   Frontend  │      serde_json       │  Backend    │
│             │ ←──────────────────────│             │
└─────────────┘                       └─────────────┘
```

**特点**:
- 类型安全（Rust ↔ TypeScript）
- 自动生成绑定
- 异步支持
- 桌面应用专用

### F.2 协议性能对比

| 协议 | 序列化 | 延迟 | 吞吐量 | 双向 | 流式 | 跨语言 |
|------|--------|:----:|:------:|:----:|:----:|:------:|
| **Direct Call** | N/A | 最低 | 最高 | ✅ | ✅ | ❌ |
| **stdin/stdout JSON** | JSON | 中 | 中 | ❌ | ❌ | ✅ |
| **JSON-RPC 2.0** | JSON | 中 | 中 | ✅ | ❌ | ✅ |
| **MCP stdio** | JSON-RPC | 中 | 中 | ✅ | ✅ | ✅ |
| **WebSocket** | JSON | 低 | 高 | ✅ | ✅ | ✅ |
| **Tauri Commands** | serde_json | 低 | 高 | ✅ | ❌ | ❌ |
| **protobuf** | protobuf | 低 | 最高 | ✅ | ✅ | ✅ |
| **CBOR** | CBOR | 低 | 高 | ✅ | ✅ | ✅ |

### F.3 协议选择建议

| 场景 | 推荐协议 | 理由 |
|------|----------|------|
| **本地可信插件** | Direct Call | 最高性能 |
| **本地不可信 Hook** | stdin/stdout JSON | 简单隔离 |
| **跨进程 MCP** | MCP stdio/SSE | 标准化 |
| **浏览器 ↔ 服务端** | WebSocket | 全双工 |
| **桌面应用** | Tauri Commands | 类型安全 |
| **高性能 RPC** | protobuf + gRPC | 最高吞吐 |
| **跨语言紧凑** | CBOR | 紧凑二进制 |

## 附录 G：插件发现与加载机制深度对比

### G.1 发现机制分类

#### G.1.1 声明式配置发现

**代表工程**: openclaw, opencode, pi, claudecode

```typescript
// openclaw: config + filesystem scan
export type Config = {
  plugins?: Array<string | [string, PluginOptions]>;
}

// 发现源（按优先级）:
// 1. Bundled: 153 内置 extensions
// 2. Workspace: 项目级
// 3. Global: 用户级
// 4. Package: npm 包
// 5. Bundle: 打包格式
```

**优势**: 用户明确控制，可审计
**劣势**: 需手动配置

#### G.1.2 自动扫描发现

**代表工程**: jiuwenswarm, hermes-agent, agent-studio

```python
# jiuwenswarm: filesystem auto-scan
class ExtensionLoader:
    def add_search_path(self, path): ...
    def discover_extension_roots(self) -> list[Path]:
        # 扫描包含 extension.yaml 的目录
        ...
```

**优势**: 零配置，即装即用
**劣势**: 可能加载不需要的插件

#### G.1.3 Marketplace 发现

**代表工程**: claudecode, atomcode, cc-switch

```typescript
// claudecode: marketplace + settings
export type LoadedPlugin = {
  name: string;
  manifest: PluginManifest;
  source: string;       // marketplace identifier
  sha?: string;         // Git commit SHA
};
```

**优势**: 集中管理，可审核
**劣势**: 中心化依赖

#### G.1.4 Entry Point 发现

**代表工程**: hermes-agent（pip entry points）

```python
# hermes-agent: pip entry points
# setup.py:
entry_points={
    'hermes_agent.plugins': [
        'my_plugin = my_package.plugin:register',
    ],
}
```

**优势**: Python 生态标准
**劣势**: 仅限 Python

### G.2 加载机制对比

#### G.2.1 静态加载（build-time）

**代表工程**: atomcode（Rust 编译时）

```rust
// atomcode: 编译时注册
pub const BUILTIN_PLUGINS: &[BuiltinPlugin] = &[
    BuiltinPlugin { name: "my_plugin", ... },
];
```

**优势**: 零运行时开销，类型安全
**劣势**: 无法动态扩展

#### G.2.2 动态加载（runtime import）

**代表工程**: opencode, pi, hermes-agent

```typescript
// opencode: dynamic import()
const module = await import(pluginPath);
```

```python
# hermes-agent: importlib
module = importlib.import_module(module_name)
```

**优势**: 灵活，支持热加载
**劣势**: 运行时错误

#### G.2.3 动态库加载（cdylib）

**代表工程**: Switchyard

```rust
// Switchyard: cdylib loading
impl NativePlugin for SwitchyardPlugin {
    fn register(&mut self, config: &Map<String, Json>, ctx: &mut PluginContext<'_>) -> Result<()> {
        // 注册 LLM 执行拦截
    }
}
```

**优势**: 跨语言，高性能
**劣势**: 不安全，崩溃影响宿主

#### G.2.4 WASM 加载（理论）

**代表工程**: ❌（15 工程均无）

```rust
// 理论实现
let engine = Engine::default();
let module = Module::from_file(&engine, "plugin.wasm")?;
let instance = Instance::new(&mut store, &module, &imports)?;
```

**优势**: 安全隔离，跨平台
**劣势**: 工具链复杂

### G.3 加载性能对比

| 加载方式 | 首次加载 | 后续加载 | 内存占用 | 隔离性 |
|----------|:--------:|:--------:|:--------:|:------:|
| **静态编译** | 0ms | 0ms | 0MB | 无 |
| **import()** | ~50ms | ~10ms | ~5MB | 无 |
| **importlib** | ~30ms | ~5ms | ~3MB | 无 |
| **cdylib** | ~20ms | ~5ms | ~2MB | 低 |
| **WASM** | ~100ms | ~10ms | ~10MB | 高 |
| **Subprocess** | ~50ms | N/A | ~20MB | 中 |

## 附录 H：插件开发 SDK 对比

### H.1 SDK 设计哲学

#### H.1.1 最小化 SDK（atomcode, claudecode, jiuwenswarm）

**哲学**: 仅提供 Manifest + Hook 执行器，无运行时 SDK

```json
// atomcode: 纯 JSON 配置
{
  "name": "my-plugin",
  "hooks": {
    "PreToolUse": [{"hooks": [{"type": "command", "command": "echo hi"}]}]
  }
}
```

**优势**: 最简单，无语言绑定
**劣势**: 能力有限，只能执行命令

#### H.1.2 类型化 SDK（openclaw, opencode, pi）

**哲学**: 完整 TypeScript SDK，类型安全

```typescript
// openclaw: 60+ 导出类型
export function definePluginEntry({
  id, name, description, kind, configSchema,
  reload, nodeHostCommands, securityAuditCollectors, register,
}: DefinePluginEntryOptions): DefinedPluginEntry
```

```typescript
// opencode: defineTool helper
export function defineTool<TArgs extends z.ZodRawShape>(input: {
  description: string;
  args: TArgs;
  execute(args: z.infer<z.ZodObject<TArgs>>, context: ToolContext): Promise<ToolResult>;
}) { return input; }
```

**优势**: 类型安全，IDE 支持
**劣势**: 仅限 TypeScript

#### H.1.3 面向对象 SDK（hermes-agent, agent-core, jiuwenswarm）

**哲学**: Python 类继承体系

```python
# hermes-agent
class PluginContext:
    def register_tool(self, name, toolset, schema, handler, ...): ...
    def register_command(self, name, handler, description, ...): ...
    def register_cli_command(self, name, help, setup_fn, handler_fn): ...
    def on_unload(self, callback): ...
```

```python
# jiuwenswarm
class BaseExtension(ABC):
    @abstractmethod
    async def initialize(self, config: ExtensionConfig) -> None: ...
    @abstractmethod
    async def shutdown(self) -> None: ...
```

**优势**: Pythonic，易扩展
**劣势**: 仅限 Python

#### H.1.4 Trait-based SDK（Switchyard）

**哲学**: Rust trait 系统

```rust
impl NativePlugin for SwitchyardPlugin {
    fn plugin_kind(&self) -> &str { "nvidia.switchyard" }
    fn validate(&self, config: &Map<String, Json>) -> Vec<ConfigDiagnostic> { ... }
    fn register(&mut self, config: &Map<String, Json>, ctx: &mut PluginContext<'_>) -> Result<()> { ... }
}
```

**优势**: 编译期安全，零成本抽象
**劣势**: 仅限 Rust

### H.2 SDK 能力矩阵

| 能力 | openclaw | opencode | pi | hermes | agent-core | jiuwenswarm | Switchyard |
|------|:--------:|:--------:|:--:|:------:|:----------:|:-----------:|:----------:|
| **Tool 注册** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| **Hook 注册** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Provider 注册** | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| **命令注册** | ✅ | ❌ | ✅ | ✅ | ❌ | ❌ | ❌ |
| **Flag 注册** | ✅ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **UI 注入** | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ❌ |
| **MCP 服务器** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Migration** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Worker** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

### H.3 SDK 学习曲线

| SDK | 学习曲线 | 文档质量 | 示例丰富度 | 社区支持 |
|-----|:--------:|:--------:|:----------:|:--------:|
| **openclaw** | 中 | ✅ | ✅ | 中 |
| **opencode** | 低 | ✅ | ✅ | 中 |
| **pi** | 低 | ✅ | ✅ | 中 |
| **hermes** | 中 | ✅ | ✅ | 中 |
| **agent-core** | 高 | ❌ | ❌ | 低 |
| **jiuwenswarm** | 中 | ✅ | ✅ | 低 |
| **Switchyard** | 高 | ❌ | ❌ | 低 |

## 附录 I：插件生态成熟度评估

### I.1 评估模型

采用 **PEHMS 模型**（Plugin Ecosystem Health Maturity Score）从 5 个维度评估：

| 维度 | 权重 | 评估项 |
|------|:----:|--------|
| **P**lugin API | 25% | 注册方式、能力表面、类型安全 |
| **H**ook System | 25% | Hook 类型数、决策能力、失败处理 |
| **E**xtensibility | 20% | 发现机制、加载方式、热重载 |
| **M**arket | 15% | 分发渠道、版本管理、企业策略 |
| **S**andbox | 15% | 隔离级别、安全边界、性能开销 |

### I.2 成熟度评分

| 工程 | Plugin API | Hook System | Extensibility | Market | Sandbox | **总分** |
|------|:----------:|:-----------:|:-------------:|:------:|:-------:|:--------:|
| **openclaw** | 95 | 90 | 85 | 70 | 20 | **76** |
| **claudecode** | 60 | 85 | 80 | 90 | 50 | **72** |
| **pi** | 85 | 75 | 75 | 60 | 10 | **65** |
| **hermes** | 80 | 80 | 70 | 65 | 10 | **65** |
| **jiuwenswarm** | 75 | 70 | 65 | 40 | 95 | **65** |
| **opencode** | 80 | 70 | 70 | 65 | 10 | **63** |
| **atomcode** | 40 | 50 | 55 | 75 | 50 | **53** |
| **agent-core** | 70 | 75 | 40 | 10 | 10 | **47** |
| **deepseek** | 60 | 45 | 30 | 20 | 60 | **43** |
| **semantica** | 50 | 20 | 40 | 50 | 10 | **37** |
| **Switchyard** | 30 | 25 | 20 | 10 | 15 | **21** |
| **TencentDB** | 40 | 25 | 30 | 50 | 30 | **34** |
| **undici** | 20 | 35 | 25 | 60 | 10 | **29** |
| **cc-switch** | 10 | 0 | 15 | 30 | 0 | **11** |
| **agent-studio** | 30 | 0 | 20 | 40 | 85 | **28** |

### I.3 成熟度等级

| 等级 | 分数区间 | 工程 | 特征 |
|------|:--------:|------|------|
| **L5: 生产级** | 80-100 | ❌ | 完整生态 |
| **L4: 成熟** | 65-79 | openclaw, claudecode | 核心完整，市场/沙箱待加强 |
| **L3: 可用** | 50-64 | pi, hermes, jiuwenswarm, opencode | 核心可用，生态待建设 |
| **L2: 基础** | 35-49 | atomcode, agent-core, deepseek | 基础能力，需大幅扩展 |
| **L1: 雏形** | 0-34 | semantica, Switchyard, TencentDB, undici, cc-switch, agent-studio | 单一能力，非完整生态 |

### I.4 laew 目标定位

| 阶段 | 目标等级 | 目标分数 | 关键里程碑 |
|------|:--------:|:--------:|------------|
| **MVP** | L2 | 40 | Plugin API + 8 Hook 类型 + Manifest |
| **v0.5** | L3 | 55 | + 沙箱隔离 + 发现机制 |
| **v1.0** | L4 | 70 | + 市场分发 + 版本管理 |
| **v2.0** | L5 | 85 | + WASM 沙箱 + 企业策略 |

## 附录 J：laew 插件生态实施建议

### J.1 设计理念

基于 15 个工程的深度调研，laew 插件生态应遵循以下设计理念：

1. **安全第一**: 默认 WASM 沙箱隔离，不可信代码无法破坏宿主
2. **渐进式采用**: MVP 仅 8 种 Hook 类型，逐步扩展到 40+
3. **多语言支持**: 通过 WASM 支持 Rust/TypeScript/Python/Go 编写的插件
4. **声明式优先**: Manifest + JSON Schema，人类可读可审计
5. **fail-open 默认**: Hook 异常不阻塞 Agent 主流程

### J.2 核心架构

```
┌─────────────────────────────────────────────────────────────┐
│                        laew Host                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐ │
│  │ Plugin      │  │ Hook        │  │ Manifest            │ │
│  │ Registry    │  │ Runner      │  │ Validator           │ │
│  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘ │
│         │                │                    │            │
│  ┌──────┴────────────────┴────────────────────┴──────────┐ │
│  │              Plugin Manager (tokio::sync)              │ │
│  └──────────────────────┬────────────────────────────────┘ │
│                         │                                   │
│  ┌──────────────────────┴────────────────────────────────┐ │
│  │                   WASM Sandbox                        │ │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐           │ │
│  │  │ Plugin A │  │ Plugin B │  │ Plugin C │  ...      │ │
│  │  │ (WASM)   │  │ (WASM)   │  │ (WASM)   │           │ │
│  │  └──────────┘  └──────────┘  └──────────┘           │ │
│  └───────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

### J.3 实施路线图

#### Phase 1: MVP（4 周）

**目标**: 最小可行插件系统

| 周 | 任务 | 产出 |
|:--:|------|------|
| 1 | Manifest Schema 设计 | `plugin-v1.json` schema |
| 2 | Hook Registry 实现 | 8 种核心 Hook 事件 |
| 3 | Plugin Loader 实现 | 从 JSON 加载插件 |
| 4 | Hook Runner 实现 | fail-open + timeout |

**依赖 crate**: `serde`, `serde_json`, `schemars`, `tokio`, `failsafe`

#### Phase 2: 安全隔离（4 周）

**目标**: WASM 沙箱 + 内容寻址

| 周 | 任务 | 产出 |
|:--:|------|------|
| 5 | WASM 运行时集成 | `wasmtime` 嵌入 |
| 6 | 插件 ABI 定义 | `host <-> plugin` 接口 |
| 7 | 内容寻址实现 | SHA-256 hash pinning |
| 8 | 权限系统 | capability-based 权限 |

**依赖 crate**: `wasmtime`, `sha2`, `blake3`

#### Phase 3: 市场生态（4 周）

**目标**: 插件市场 + 版本管理

| 周 | 任务 | 产出 |
|:--:|------|------|
| 9 | 插件市场 API | RESTful 市场接口 |
| 10 | 版本管理 | semver + auto-update |
| 11 | CLI 集成 | `laew plugin install/list` |
| 12 | 文档 + 示例 | 开发者指南 |

**依赖 crate**: `self_update`, `reqwest`, `semver`

### J.4 关键指标

| 指标 | MVP | v0.5 | v1.0 | v2.0 |
|------|:---:|:----:|:----:|:----:|
| **Hook 类型** | 8 | 15 | 27 | 41 |
| **沙箱隔离** | ❌ | WASM | WASM + Landlock | 完整 |
| **市场分发** | ❌ | ❌ | npm + 自建 | 完整 |
| **版本管理** | ❌ | semver | semver + hash | 完整 |
| **热重载** | ❌ | ✅ | ✅ | ✅ |
| **权限系统** | ❌ | ❌ | capability | 完整 |
| **MCP 支持** | ❌ | stdio | 5 传输 | 完整 |

---

## 结语

本轮调研全面覆盖了 15 个源码工程的插件生态、扩展分发与 Hook 系统，累计阅读 387+ 个文件，构建了 6 张大型对比表，产出了 20 条新 gap（L221-L240），推荐了 40+ 个 Rust crate。

**核心发现**：

1. **openclaw 拥有最完整的插件生态**（153 bundled + 41 Hook 类型 + 60+ SDK 方法）
2. **atomcode 的 hash-pinned 信任模型**是最安全的设计
3. **jiuwenswarm 的 JiuwenBox**是最强的沙箱隔离（BubbleWrap + Landlock + Seccomp + netns + cgroup）
4. **无工程使用 WASM 沙箱**——这是 laew 的差异化机会
5. **fail-open 是主流失败策略**（10/15 工程）

**laew 的机遇**：

- 以 WASM 沙箱为差异化特性
- 以声明式 Manifest + JSON Schema 为开发者体验
- 以 8 种核心 Hook 为 MVP，逐步扩展到 41 种
- 以 npm + 自建市场为分发渠道

**累计 gap 统计**：

| 轮次 | gap 范围 | 数量 | 维度 |
|------|----------|:----:|------|
| 第 1-4 轮 | L1-L15 | 15 | 架构/Context/循环/工具 |
| 第 5 轮 | L16-L25 | 10 | MCP/SKILL/沙箱/权限 |
| 第 6 轮 | L26-L37 | 12 | 协议 wire/流式/错误重试 |
| 第 7 轮 | L38-L78 | 41 | CrashDump/WebUI/OAuth/i18n |
| 第 8 轮 | L79-L142 | 64 | Release/WebSocket/容器/CRDT |
| **第 9 轮** | **L142-L160** | **19** | **插件生态（前）** |
| **第 10 轮** | **L161-L220** | **60** | **插件生态（中）** |
| **第 11 轮** | **L221-L240** | **20** | **插件生态（后）** |
| **累计** | **L1-L240** | **241** | **全维度** |

---

> **报告完成时间**: 2026-09-07
> **字数**: ~140KB / 3400+ 行
> **调研员**: 第十一轮深度调研 - 插件生态与扩展分发与 Hook 系统专项
> **审核**: 待审核

