# atomcode 源码调研报告

> 调研对象:`/usr/local/LsmGitOpenSource/atomcode`(v5.0.9,2026-08)
> 调研日期:2026-09-04
> 调研目的:为 LsmAgentEmergentWork 提供 Rust 端编码 Agent 的设计参照

---

## 1. 项目元信息

| 维度 | 内容 |
| --- | --- |
| 定位 | 开源终端 AI 编码 Agent(TUI / WebUI / Daemon / ACP / Mobile),AtomGit 自托管闭源 CodingPlan 配套 |
| 语言 | **Rust 2021 edition**,1.88+;WebUI 为 TypeScript(`webui/`,构建后由二进制 embed) |
| 仓库组织 | Cargo **workspace**,根 `Cargo.toml` 用 `members = ["crates/*"]` 收集;`closed-source overlay` 通过 `crates/atomcode-codingplan-crypto` 走 `codingplan-crypto` feature 引入 |
| Crate 总数 | 14 个:`atomcode-auth`、`atomcode-capabilities`、`atomcode-cli`(主 binary `atomcode`)、`atomcode-clix`、`atomcode-coding`、`atomcode-codingplan`(闭源 placeholder)、`atomcode-codingplan-crypto`、`atomcode-config`、`atomcode-daemon`、`atomcode-kernel`、`atomcode-review`、`atomcode-telemetry`、`atomcode-tuix`、`atomcode-updater` |
| 入口点 | `crates/atomcode-cli/src/main.rs`(二进制 `atomcode`);同时 lib `src/lib.rs`(`default-run = "atomcode"`,方便 `tests/` 集成) |
| Daemon | 独立 binary `atomcode-daemon`,监听 HTTP,暴露 session history + SSE 聊天 |
| ACP | 独立子命令 `atomcode acp`,基于 `agent-client-protocol = "=2.0.0"`(feature `unstable_protocol_v2`) |
| 构建/发布 | `cargo install --path crates/atomcode-cli --locked`;`profile.release = opt-level "z" + lto + codegen-units=1 + strip + panic="abort"`(追求极致小体积) |
| License | MIT |
| 协议 | 强 serde 可序列化(`AgentCommand`/`AgentEvent` 双向),便于本地 + 跨进程(daemon/web)统一 |
| 注释量 | 极重 RustDoc(单文件动辄上千行内嵌说明),关注点:kernel 边界契约 + 失败模式 + 中间件 load-bearing 语义 |

---

## 2. 目录树(crates 维度)

```
atomcode/
├── crates/
│   ├── atomcode-kernel/       ← L0: 中性 Agent SDK(核心)
│   │   └── src/  agent.rs, provider.rs, message.rs, stream.rs,
│   │             event.rs, tool.rs, hook.rs, middleware.rs,
│   │             checkpoint.rs, clock.rs, request.rs, conformance/, testkit.rs
│   │
│   ├── atomcode-capabilities/ ← L1: 可插拔能力(核心)
│   │   └── src/  provider/(openai_compat|anthropic|ollama|atomgit_sign|reasoning|retry|sign)
│   │             tools/(bash|read|write|edit|task|todo|web_fetch|web_search|approval|...)
│   │             mcp/(client|config|registry|oauth|transport_http|transport_stdio|trust|tool)
│   │             skills/(registry|render|use_skill|catalog_hook)
│   │             codeintel/(graph|index|list_symbols|read_symbol|trace_*|lsp_tool|file_deps|blast_radius)
│   │             session/(manager|snapshot|transcript|recall|rewind|context|presentation|status_reminder)
│   │             subagent/(claude_code|codex|proc|tool)
│   │             memory/, cc_hooks.rs, datalog.rs, compaction.rs, plugin/,
│   │             team.rs, fs.rs, hooks.rs, proxy.rs, file_index.rs
│   │
│   ├── atomcode-coding/       ← L2: 编码特化(核心)
│   │   └── src/  runtime.rs, parts.rs, assemble.rs, persona.rs,
│   │             controllers.rs, plan_mode.rs, todo.rs,
│   │             discipline/verify.rs, team/(manager|runner|tool),
│   │             init_prompt.rs, subagent_tiers.rs, skill_first.rs,
│   │             next_prompt_suggestion.rs, mcp_instructions.rs,
│   │             provider_factory.rs, rate_limit.rs, vision.rs,
│   │             telemetry.rs, execution_policy.rs, plugin_hooks.rs
│   │
│   ├── atomcode-tuix/         ← TUI: 终端 UI(核心)
│   │   └── src/  event_loop/{mod,commands,bg_runtime,monitor,oauth_poll,...}.rs
│   │             render/{retained,plain,cell,interaction,worker,screen}.rs
│   │             modals/(model_picker|session_picker|diff_viewer|provider_panel|rewind|...).rs
│   │             input/, highlight/, i18n/, git/, plus
│   │             state.rs, terminal.rs, markdown.rs, commands.rs, session.rs, team.rs
│   │
│   ├── atomcode-cli/          ← 辅助:CLI 入口 + clap 子命令 + 调度器
│   ├── atomcode-daemon/       ← 辅助:HTTP/SSE 服务
│   ├── atomcode-config/       ← 辅助:配置、模型目录、settings、store
│   ├── atomcode-auth/         ← 辅助:OAuth/SSO/CodingPlan 登录
│   ├── atomcode-telemetry/    ← 辅助:匿名遥测
│   ├── atomcode-review/       ← 辅助:`code_review` 子代理实现
│   ├── atomcode-clix/         ← 辅助:CLI 实验包
│   ├── atomcode-codingplan/   ← CodingPlan 模型目录与配额窗口
│   ├── atomcode-codingplan-crypto/ ← 闭源 request signer(开源 stub)
│   └── atomcode-updater/      ← 辅助:自更新
│
├── webui/                     ← TypeScript WebUI(构建后由 atomcode-daemon embed)
├── docs/                      ← 设计文档 + plan + 报告
├── evals/                     ← 评测(对抗性 prompt 集等)
├── examples/                  ← Rust 用法示例
├── extensions/                ← 扩展
├── docker/                    ← Docker 镜像
├── scripts/                   ← 安装/构建脚本
└── Cargo.toml                 ← workspace root
```

**职责标注**:
- **核心**:kernel / capabilities(L1)/ coding(L2)/ tuix(TUI)
- **辅助**:cli / daemon / config / auth / telemetry / review / clix / codingplan / updater / webui
- **文档/支撑**:docs / evals / examples / extensions / docker / scripts

---

## 3. 架构骨架

| 关注点 | 文件位置 | 备注 |
| --- | --- | --- |
| **Agent 主循环**(turn loop + 中间件 + 流消费) | `crates/atomcode-kernel/src/agent.rs`(5966 行) | `Agent` + `AgentBuilder` + `AgentHandle` + `AutoRespond`,驱动 turn → round → round;支持多 round 续推、 `offer_continuation` 钩子串接、Tool 循环检测(provider `retryable` Err、empty-response retry、`MAX_RATE_LIMIT_WAITS`、`MAX_REPEAT_ROUNDS`、opt-in `ToolLoopPolicy`) |
| **消息模型**(Role + Message + ToolCall + ImageContent + SessionSnapshot + 内部 `internal_origin` 通道) | `kernel/src/message.rs` | 角色枚举 `Role`、中性 inline `ImageContent`、`ReasoningBlock`(带 opaque signature,可跨 provider echo)、legacy cold summary origin 常量 |
| **Provider 抽象**(`LlmProvider` trait) | `kernel/src/provider.rs` | 仅 `chat_stream(&[Message], &[ToolDef], &ChatOptions)`,**流式**为唯一协议;`ChatOptions` 中性携带 `reasoning_effort`/`tool_choice`/`temperature`/`max_tokens`,适配器自行映射到 OpenAI/Anthropic/Ollama |
| **工具 + 中间件 + 钩子** | `kernel/src/tool.rs`、`kernel/src/middleware.rs`、`kernel/src/hook.rs` | `Tool` trait + `ToolMiddleware::before/after`(可短路、注册顺序即语义)+ `LifecycleHooks::pre_request/post_response/on_reasoning_delta/offer_continuation/turn_complete/...` |
| **Context 管理**(快照 + 压缩 + session 持久化) | `kernel/src/checkpoint.rs`、`kernel/src/message.rs`(SessionSnapshot、CompactTrigger、SNAPSHOT_VERSION),`capabilities/src/session/(manager\|snapshot\|transcript\|recall\|rewind).rs` | 二级持久化:`<id>.snapshot` 压缩快照(供 RESUME)+ `<id>.jsonl` append-only transcript(供 RECALL);`StubCompaction` + `OverflowCompaction`(stub→truncate→drain+LLM-summary 三段式 overflow recovery) |
| **TUI / REPL** | `crates/atomcode-tuix/src/event_loop/mod.rs`(31321 行!)+ `render/retained.rs`(26012 行)+ `render/worker.rs` | 自研 `RetainedRenderer` + `TerminalGuard`(panic-safe,支持 kitty 协议 / iTerm2 / WezTerm / OSC99/OSC777);`bg_runtime.rs` 后台 slot 调度(`/bg` 不阻塞主 REPL);`modals/` 含 model/session/provider/diff/rewind 浮层 |
| **WebUI / HTTP daemon** | `crates/atomcode-daemon/src/`(live_api.rs,live_hub.rs,api_provider.rs,api_codingplan.rs,api_auth.rs,webui.rs) | 仅 loopback + 一次性 token,SSE 流式聊天 |
| **ACP(Agent Client Protocol)** | `crates/atomcode-cli/src/acp/`(14 个文件,engine/dispatch/sessions/v2/replay/translate/permission/...) | 双协议路由:v1 稳定 + v2 draft(opt-in);client-injected `mcpServers` 由 driver 作为信任边界注入 |

**关键 trait 一览**:
- `LlmProvider`:唯一协议是流式 `chat_stream`,模型名 + context window + session-id 一次性 bind
- `Tool` + `ToolMiddleware` + `LifecycleHooks`:三段式旋钮,中间件注册顺序即 load-bearing 语义
- `CompactionStrategy`:内核按 `bytes_after < bytes_before` 网关拒绝 net-loss plan

---

## 4. 核心特征清单

| 能力 | 是否支持 | 关键证据 |
| --- | --- | --- |
| **多 Agent / Team** | ✅ | `team/(manager\|runner\|tool)`(1247 + 492 + 416 行);`capabilities/team.rs`(617 行,中立 TeamEvent/TeamMemberId/TeamPermission);`TeamRunManager` 支持并发、cancel grace、completion 历史;Team Tool 作为内核 `Tool` 挂载 |
| **外部子代理** | ✅ | `subagent/(claude_code\|codex\|proc\|tool)`,驱动 Claude Code / Codex 作为命名 `subagent_<name>` 工具;`external_subagent_profiles` 把 `[[subagent.external]]` 解析为 profile,`bypass` 权限降级逻辑(仅 attended + profile opt-in 才保留) |
| **任务分类 / 拆解** | ✅ | `controllers.rs`(1247 行)`GoalPhase` + `GoalProgress` + `GoalTerminal`(自主循环 + evaluator LLM 调一次判 `Verdict: yes/no`);`MAX_UNPRODUCTIVE=5` 上限;`goal_cap_stop_note` 处理超限 |
| **质检(verify)** | ✅ | `discipline/verify.rs`(992 行)`VerifyCadenceHook`,`offer_continuation` 钩子:edit 后未跑构建就注入一次 nudge;`bash` denylist 排除 `ls/echo/cat` 等读命令;`attended=true` 时不强制 |
| **Plan / Build mode** | ✅ | `plan_mode.rs`(334 行)`PlanModeGate`,只读探索;`/plan` / `/build` slash 命令切换 |
| **MCP** | ✅ | `mcp/(client\|config\|registry\|oauth\|transport_http\|transport_stdio\|trust\|tool)`,stdio + HTTP(SSE) 双传输,OAuth 登录 + `mcp_trust.json` 信任存储(原子写);`/mcp reload` 触发 prepare+respawn |
| **Skill** | ✅ | `skills/(registry\|render\|use_skill\|catalog_hook)`,Claude-Code 兼容 frontmatter 加载;`use_skill` / `list_skills` 工具;运行时优先级 home+project;`skill_first.rs` 检测 prompt 中是否首句 `@skill_name` |
| **Session 持久化** | ✅ | 二级 `.snapshot` + `.jsonl`;`rewind.rs`(1160 行) 支持 turn-by-turn 事务回滚;`recall.rs`(640 行) `recall` 工具做跨 session 检索;`status_reminder.rs` 在长 session 注入上下文提醒 |
| **多 Provider** | ✅ | `provider/(openai_compat\|anthropic\|ollama\|atomgit_sign)`;OpenAI 兼容通吃 DeepSeek/GLM/Qwen/SiliconFlow/Any;`sign.rs` CodingPlan 请求签名(闭源 overlay) |
| **流式响应** | ✅ | `kernel/stream.rs` `StreamEvent`(`TextDelta/Reasoning/ReasoningSignature/ToolCall/ToolCallDelta/Usage/Error/Malformed`);`anthropic.rs` 拆分输入早到、输出累计的 usage,`TokenUsage::merge_max` 字段级合并(防双计) |
| **思考 / Reasoning echo** | ✅ | `ReasoningBlock`(opaque + provider attribution),内核"store + live 转发";Anthropic signature / OpenAI encrypted_content / Gemini thoughtSignature 由 adapter 各自负责 echo |
| **Vision(VL 预处理)** | ✅ | `vision.rs`(225 行)+ `coding/vision.rs`(220 行);VL 模型识别图像后转文本,失败时驱动重新粘贴;编码 config `supports_vision` 全链路贯穿 |
| **Todo / Progress** | ✅ | `todo.rs`(853 行)`TodoHook`(system-reminder 每轮注入 todo)+ `TodoEagerHook`(模型自动维护);`ATOMCODE_TODO=0` 整链关闭 |
| **数据日志 / 回放** | ✅ | `capabilities/datalog.rs`(856 行)turn 级 markdown + per-round JSONL;`/snapshot` `AgentCommand::Snapshot` 拉取 `SessionSnapshot` |
| **Plugin 扩展** | ✅ | `plugin/(bootstrap\|installer\|loader\|manifest\|marketplace)`,git marketplace;`hook_trust.rs` 控制生命周期钩子 |
| **Memory** | ✅ | `memory/(hook\|store)`,`$ATOMCODE_HOME/memory.md` + `<root>/.atomcode/memory.md`;v1 **不**暴露给模型的工具,仅 system-reminder 注入 |
| **/goal 自主目标模式** | ✅ | `controllers.rs` evaluator LLM 判 `Verdict: yes/no`,`MAX_UNPRODUCTIVE=5` |
| **后台 slot** | ✅ | `tuix/event_loop/bg_runtime.rs`(2345 行),`/bg` 把任务移到 detached slot,TUI 不阻塞 |
| **原子 Remote 访问** | ✅ | `/app` 反向 WSS 隧道 + 二维码 + 双向实时同步 + 远程命令(`/status` `/diff`) |
| **WebUI** | ✅ | `atomcode webui`,loopback + 一次性 token,frontend build 后 embed 到 daemon 二进制 |
| **协议 (ACP)** | ✅ | `agent-client-protocol = "=2.0"`,`unstable_protocol_v2` 双路由,支持 elicitation |
| **原子安全 / 许可** | ✅ | `tools/(approval\|credential_bash_gate\|atomgit_bash_gate\|bash_workspace_gate\|sensitive_path\|write_approval)`,`policy/approval.rs` 多层 middleware + per-path 永久放单;`/undo` 通过文件历史快照回滚 |
| **destructive command 检测** | ✅ | `rm -rf`、`git push --force`、`DROP TABLE` 强制 prompt;源码文件 rm 永不自动通过 |
| **Code Review** | ✅ | `crates/atomcode-review`,`/review` / `/review staged` / `/review <base>`,作为 `code_review` 子代理挂载 |
| **Code Graph(codeintel)** | ✅ | tree-sitter 12 语言,`list_symbols/read_symbol/find_references/trace_callers/trace_callees/trace_chain/file_deps/blast_radius` + LSP(`lsp_tool.rs`) |
| **任务子代理 tier** | ✅ | `subagent_tiers.rs` 按 `capable_model` 排名选 fast/capable 双 tier |

---

## 5. 关键文件清单(按职责归类)

> 行数为 `wc -l` 实测,挑至 25 个最具代表性

| 路径(绝对) | 行数 | 职责 |
| --- | --- | --- |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/agent.rs` | 5966 | **L0 主循环**:turn loop + retry ladder + tool-loop guard + 续推 |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/event.rs` | 531 | **Command/Event 协议**:`AgentCommand`/`AgentEvent` serde 可序列化 |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/message.rs` | (查略,大文件) | **消息 + snapshot + compaction view** |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/stream.rs` | 280 | **流事件 + TokenUsage merge_max + Overflow 识别** |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/provider.rs` | 269 | **LlmProvider trait + ChatOptions + ReasoningEffort** |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/middleware.rs` | (中) | **ToolMiddleware:before/after 注册链(load-bearing)** |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/runtime.rs` | **15778** | **L2 driver runtime**:`CodingRuntime` + `CodingRuntimeEvent` + `CodingRuntimeHandle`(driver 控制面) |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/parts.rs` | 3511 | **两阶段装配**:`prepare`(异步、加载 MCP/Skill/Session)+ `assemble`(纯组合) |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/assemble.rs` | 324 | **同步装配**:provider + tools + middleware + 钩子串接 |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/persona.rs` | 1928 | **系统提示词(coding_persona)+ todo/request_user_input 开关** |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/discipline/verify.rs` | 992 | **edit-then-verify hook**:语言无关,只识别工具名 + bash denylist |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/controllers.rs` | 1247 | **`/goal` `/loop` 自主控制**:GoalPhase + evaluator LLM |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/todo.rs` | 853 | **TodoHook + TodoEagerHook**(模型自动维护 todo) |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/team/manager.rs` | 1435 | **TeamRunManager**(并发 + cancel grace + activity sink) |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/session/manager.rs` | **7055** | **二级持久化**:`<id>.meta + .snapshot + .jsonl` + rewind transaction |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/session/snapshot.rs` | 1928 | SessionSnapshot 编解码 + 版本兼容 |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/session/rewind.rs` | 1160 | **事务化 rewind**(按 turn 回退) |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/provider/openai_compat.rs` | **4092** | **OpenAI 兼容适配器**(通用 OpenAI-shape 适配器,涵盖 DeepSeek/GLM/Qwen/任意兼容 API) |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/provider/anthropic.rs` | 2024 | Anthropic Messages API(拆分 usage + thinking signature) |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/provider/retry.rs` | 1080 | **HTTP 429 重试 + Retry-After 解析** |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/registry.rs` | 1638 | MCP 服务注册表(stdio + HTTP 双传输) |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/config.rs` | 1075 | MCP 配置(`<working_dir>/.mcp.json` + global) |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/oauth.rs` | 845 | MCP OAuth 登录 + token 存 |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/compaction.rs` | 2345 | **三级 overflow ladder**:stub → truncate → drain + LLM-summary |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/tools/bash.rs` | **4303** | bash 工具主体 + workspace gate |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/event_loop/mod.rs` | **31321** | TUI 主循环 + `run_loop` + App + 输入分发 |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/event_loop/commands.rs` | 9326 | TUI slash 命令调度 |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/render/retained.rs` | **26012** | **RetainedRenderer**(保留式渲染 + worker 线程) |

---

## 6. 与 LsmAgentEmergentWork 的差异亮点

1. **三层垂直架构(L0 kernel / L1 capabilities / L2 coding),方向性边缘被 cargo 编译强制**。
   `atomcode-capabilities` 只依赖 `atomcode-kernel` + 第三方,绝不依赖 `atomcode-core`(注释里明确写出 `cargo tree -p atomcode-capabilities must not contain atomcode-core`)。能力按 feature flag 选(`provider`/`tools`/`mcp`/`skills`/`session`/`memory`/`codeintel`...),driver 只编译用到的子树。LsmAgentEmergentWork 当前是扁平模块,边界由模块注释维持而无编译保护。

2. **`AgentCommand`/`AgentEvent` 全部 serde,同一份协议既能 TUI 内、又能 daemon HTTP/SSE、又能 ACP 跨进程**。
   命令/事件都带 `#[non_exhaustive]` + `#[serde(default)]`(ADDITIVE),保证旧 wire 形态永远可反序列化。`SendMessage` 可选 `images`、`SendMessageWithContext` 把合成上下文塞进同一个 turn(避免产生额外自动化 turn),`Compact { focus }` 强制触发手动压缩,`Cancel` 抢占等都是这套协议承载。LsmAgentEmergentWork 当前 TUI / WebUI 各自有独立控制面,缺一份中立序列化协议。

3. **`tool-loop` 与 `repeat-loop` 双保险 + 三层 overflow recovery ladder**(stub 替换老 tool 结果 → 截断 → drain + LLM-summary)。
   `tool_loop_policy`(默认 3 warn / 4 stop)+ `MAX_REPEAT_ROUNDS=6` 保险;`MAX_RATE_LIMIT_WAITS=5` + `SILENT_FIRST_RATE_LIMIT_RETRY=1s` 静默首 429,然后才升级显示倒计时;`MAX_TRUNCATION_CONTINUATIONS=4` 截断续推;`EMPTY_RESPONSE_MAX_RETRIES=5` 空 200 单独重试;每条都写明理由和 v1 历史渊源。LsmAgentEmergentWork 当前仅做基本 max-rounds,缺这种细分失败模式策略。

4. **`prepare → assemble` 两阶段装配 + `Arc<Parts>` 跨 respawn 持久授权状态**。
   `prepare` 异步做 I/O(MCP 后台连接 + skill 加载 + session bind + 分配 uuid),`assemble` 纯组合;返回的 `CodingParts` 持有 `Arc<ApprovalMiddleware>`、`Arc<HookChain>`、`Arc<SkillRegistry>` 等。`/mcp reload` 重跑 prepare 但保留 grants;**模型 swap** 只重建 provider,**复用同一个 Parts**(B2 respawn)—session_id 单一来源,`driver` 不需要手传。LsmAgentEmergentWork 没有这种"外部 I/O 与内核组合分离"的边界,所有能力混在主程序。

6. **TUI 自研 `RetainedRenderer` + 大量 `TerminalGuard` Drop 守护 panic-safe 还原**。
   `panic = "abort"` profile 下不 unwind,Drops 不跑,所以用全局 panic hook 强制恢复 raw mode、Kitty keyboard protocol、cursor visibility、autowrap、DECSTBM;同时 `bg_runtime` 把长任务移到 detached slot(类似后台 job,主 REPL 不阻塞),`/bg` 是关键 UX。LsmAgentEmergentWork 的 TUI 是 ratatui 默认渲染,缺这种保留式 diff 渲染与背景 slot 调度。

7. **`session/snapshot.rs` + `transcript.rs` 二级持久化:压缩 snapshot 供 RESUME、append-only JSONL 供 RECALL**。
   RESUME 用低体积压缩版本保证续接可用,RECALL 用永不压缩的 JSONL 全文检索;`<id>.meta` 用于 `/resume` 选择列表的快速 metadata(< 4MB ceiling)。`rewind` 走事务(`<id>.rewind.json`),`recall` 工具跨 session 检索,`status_reminder.rs` 在长 session 中按比例注入上下文提醒。LsmAgentEmergentWork 当前 session/上下文是单文件 JSON,没有 recall/rewind 分级策略。

---

## 7. 启发(供 LsmAgentEmergentWork 借鉴)

- **协议序列化**:`AgentCommand/AgentEvent` serde 化为单协议,TUI/daemon/WebUI/ACP 共享一份。
- **三层 architecture + cargo feature gating**:把 provider/tools/mcp/skills/session/memory/codeintel 解耦,编译时强制依赖方向。
- **失败模式细粒度**:tool-loop / repeat-loop / overflow ladder / rate-limit 静默首 429 / empty-response retry 各自独立 budget。
- **两阶段装配**:prepare(异步 I/O)+ assemble(纯组合)+ `Arc<…>` 跨 respawn 持久 grants,允许 hot-swap provider / hot-reload MCP。
- **TUI 保留式渲染 + bg slot**:ratatui 默认全量重绘;atomcode 自研 RetainedRenderer + bg_runtime,长操作 UX 差异显著。
- **Reasoning 跨轮 echo**:`ReasoningBlock { text, opaque, provider }` 三元组,Anthropic signature / OpenAI encrypted_content / Gemini thoughtSignature 各自 echo,语义干净。
- **Tool 中间件 load-bearing 顺序**:`RepairToolArgs → CredentialGate → Approval`,前一个改写的 args 后一个必须看见,所以 `before` 注册顺序就是契约。

