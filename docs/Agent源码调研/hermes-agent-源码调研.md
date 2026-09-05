# Hermes Agent 源码调研报告

> 调研对象：`/usr/local/LsmGitOpenSource/hermes-agent`（Nous Research 出品的"自改进 AI Agent"）
> 调研日期：2026-09-04
> 调研目的：对比 LsmAgentEmergentWork（Rust laew CLI，YoloAgent 多 Agent 内置架构）与 Hermes 的实现差异

---

## 1. 项目元信息

| 项 | 值 |
|---|---|
| 仓库名 | `hermes-agent`（GitHub: `NousResearch/hermes-agent`） |
| 版本 | `0.21.0`（`pyproject.toml:3`） |
| 主语言 | Python（`requires-python = ">=3.11,<3.14"`，`pyproject.toml:13`） |
| 辅助语言 | TypeScript / React（Ink TUI + Electron Desktop）、JS / JSX |
| 包管理 | `uv`（Astral 出品，基于 Rust 的 Python 包管理器） |
| 依赖锁定 | `uv.lock` ground truth（`pyproject.toml:46-49` 注释"PyPI exact pin 是出于 Mini Shai-Hulud 蠕虫事件后的策略"） |
| 协议支持 | OpenAI Chat Completions / OpenAI Codex Responses / Anthropic Messages（多 plugin 适配） |
| 部署目标 | CLI / TUI（Ink）/ Electron Desktop / Messaging Gateway（20+ 平台）/ Web Dashboard |
| Python 文件数 | 5109 个（`find . -name "*.py"`，含 tests） |
| TS/JS 文件数 | 2933 个（含 `tests-js`） |
| 仓库大小 | 859 MB（含 `tests/`、`tests-js/`、`mcp-research-data/` 等大目录） |
| 单测规模 | "约 17k 测试，约 900 文件"（`AGENTS.md:218`） |
| 顶层入口 | `cli.py`（CLI 模式）、`run_agent.py`（AIAgent 类）、`hermes_bootstrap.py`（UTF-8 / sys.path 守卫） |
| 构建工具 | `setup.py` + `pyproject.toml` + `uv` |
| 国际化 | 16 语言 YAML（`locales/`：en/zh/zh-hant/es/fr/de/ja/ko/pt/ru/uk/it/nl/af/ar/tr/hu/ur 等） |
| 文档 | `docs/`（含 `design/` `rfcs/` `middleware/` `security/` `observability/` `kanban/` 等子目录）+ `website/`（Docusaurus 站点） |

### 仓库主标语

> "The self-improving AI agent built by Nous Research. It's the only agent with a built-in learning loop — it creates skills from experience, improves them during use, nudges itself to persist knowledge, searches its own past conversations, and builds a deepening model of who you are across sessions."（`README.md:14-15`）

### 与其他 6 个调研对象的对比定位

| 项目 | 主语言 | 入口 | 主循环位置 | 主循环行数 | 核心规模 |
|---|---|---|---|---|---|
| `atomcode` | Rust | `src/main.rs` | `crates/agent/src/lib.rs` | ~12k | ~150k 行（atomcode 主体） |
| `claudecode` | TS/Bun | `bin/claude-code.js` | 主循环散在 ~30 文件 | ~3k 核心 | ~218k 行 |
| `deepseek-harness` | TS | `harness/src/main.ts` | `Harness.run()` | ~3k | ~80+ 包 |
| `openclaw` | TS | `gateway/src/index.ts` | `Gateway.ts` | ~5k | ~201 万行 |
| `opencode` | TS/Bun | `packages/opencode/src/index.ts` | `Session` 类 | ~3k | ~18k 行 |
| `pi` | TS | `packages/coding-agent/src/main.ts` | `runAgentLoop` | ~800 | ~65 文件 |
| **`hermes-agent`** | **Python** | **`run_agent.py`** | **`run_conversation` (9285 行)** | **~12k 入口** | **~10k Python 主模块** |

---

## 2. 目录树（顶层）

```
hermes-agent/
├── run_agent.py             # AIAgent 类 — 核心对话循环（10152 行，~12k LOC）
├── cli.py                   # HermesCLI 类 — 交互式 CLI 编排（22417 行，~11k LOC）
├── model_tools.py                 # 工具编排、discover_builtin_tools()、handle_function_call()（1707 行）
├── toolsets.py                    # Toolset 定义、_HERMES_CORE_TOOLS 列表（1062 行）
├── hermes_state.py                # SessionDB — SQLite 会话存储（FTS5 搜索，17317 行）
├── hermes_constants.py            # get_hermes_home() — profile-aware 路径
├── hermes_logging.py              # setup_logging() — agent.log/errors.log/gateway.log（profile-aware）
├── hermes_bootstrap.py            # Windows UTF-8 / sys.path 守卫（239 行，每个 entry 顶部导入）
├── hermes_state_*.py            # 7 个 state 相关模块（schema / search / portability / holders 等）
├── hermes_time.py                  # 时间相关
├── batch_runner.py                # 并行批处理
├── mini_swe_runner.py             # SWE-bench mini 评测
├── trajectory_compressor.py       # 轨迹压缩（用于训练）
├── registration_lifecycle.py      # 注册生命周期
├── run_agent.py                   # AIAgent — 见上
│
├── agent/                          # Agent 内部模块（~170 文件，~7.4M）
│   ├── conversation_loop.py        # 真正的 run_conversation 主体（9285 行）—— 状态机 + tool 调度
│   ├── conversation_compression.py # 压缩流水线（6123 行）
│   ├── turn_context.py             # 每轮上下文构建（1709 行）
│   ├── context_engine.py            # context_engine plugin 抽象
│   ├── context_compressor.py       # 压缩器
│   ├── context_breakdown.py        # 上下文分解（用于 UI 展示）
│   ├── context_references.py       # 上下文引用（harness/文件参考）
│   ├── native_compaction.py        # 原生压缩实现
│   ├── memory_manager.py           # 内存管理器（1436 行）
│   ├── memory_provider.py          # MemoryProvider ABC
│   ├── subagent_lifecycle.py       # SubAgent 生命周期（542 行）
│   ├── tool_executor.py            # 工具执行器
│   ├── tool_dispatch_helpers.py    # 调度辅助
│   ├── tool_guardrails.py          # 工具守卫
│   ├── tool_result_classification.py # 工具结果分类
│   ├── coding_context.py           # 编码上下文
│   ├── anthropic_adapter.py        # Anthropic 适配器
│   ├── anthropic_message_convert.py # Anthropic 消息转换
│   ├── openai_client.py            # OpenAI 客户端
│   ├── openai_adapter.py           # OpenAI 适配
│   ├── codex_responses_adapter.py  # Codex Responses API 适配
│   ├── codex_runtime.py            # Codex 运行时
│   ├── codex_headers.py            # Codex HTTP 头
│   ├── bedrock_adapter.py          # AWS Bedrock 适配
│   ├── anthropic_endpoints.py      # Anthropic endpoints
│   ├── anthropic_credentials.py    # Anthropic 凭据
│   ├── azure_identity_adapter.py   # Azure Identity
│   ├── acp_openai_bridge.py        # ACP ↔ OpenAI 桥
│   ├── skill_commands.py           # skill 命令注入
│   ├── skill_bundles.py            # skill bundle
│   ├── skill_preprocessing.py      # skill 预处理
│   ├── skill_utils.py              # skill 工具函数
│   ├── context_breakdown.py        # 上下文分解
│   ├── delegation_context.py       # 委派上下文
│   ├── relay_tools.py              # 中继工具
│   ├── background_review.py        # 后台 review（活体空闲时跑 review）
│   ├── review_idle_queue.py        # review 空闲队列
│   ├── aux_accounting.py           # 副 LLM 任务统计
│   ├── auxiliary_client.py         # 副 LLM 客户端（curator / vision / embedding / title 等）
│   ├── account_usage.py            # 账户用量
│   ├── billing_usage.py            # 计费用量
│   ├── billing_view.py             # 计费视图
│   ├── billing_links.py            # 计费链接
│   ├── computer_use/                # 计算机使用子目录
│   ├── checkpoint_manager.py       # 检查点
│   ├── async_utils.py              # 异步工具
│   ├── display.py                  # 显示（KawaiiSpinner 等）
│   ├── interruption.py             # 中断处理
│   └── ... (100+ 文件)
│
├── hermes_cli/                     # CLI 子命令、setup 向导、skin 引擎、插件加载器
│   ├── main.py                     # `hermes` 主入口（argparse 子命令）
│   ├── config.py                   # DEFAULT_CONFIG / load_config()（含 16 节）
│   ├── commands.py                 # 集中式 slash 命令注册表（COMMAND_REGISTRY）
│   ├── plugins.py                  # PluginManager（discover_plugins）
│   ├── setup_wizard.py             # 首次安装向导
│   ├── skin_engine.py              # 数据驱动主题引擎
│   ├── moa_config.py               # Mixture-of-Agents 配置
│   ├── pty_bridge.py               # Web Dashboard → TUI 桥
│   ├── web_server.py               # Web Dashboard HTTP / WebSocket
│   ├── observability/              # 可观测性后端
│   ├── tui_gateway/                # TUI 的 JSON-RPC 后端（见下）
│   └── ... (50+ 文件)
│
├── tools/                          # 工具实现（143 文件，~7.5M）—— 通过 tools/registry.py 自动发现
│   ├── registry.py                 # 工具注册中心（含 check_fn 缓存、plugin override）
│   ├── bash / terminal_tool.py      # 终端执行
│   ├── file_tools.py               # 文件操作
│   ├── read_extract.py             # 读取 + 抽取
│   ├── working_diff.py             # 工作区 diff
│   ├── write_approval.py                 # 写入审批
│   ├── path_security.py            # 路径安全
│   ├── approval.py                 # 通用审批
│   ├── delegate_tool.py            # 子代理委派
│   ├── async_delegation.py         # 异步委派
│   ├── delegation_live_log.py      # 委派实时日志
│   ├── delegation_output_schema.py # 委派输出 schema
│   ├── subagent_worktree.py        # 子代理 git worktree
│   ├── todo_tool.py                # Todo 工具
│   ├── memory_tool.py              # Memory 工具
│   ├── session_search_tool.py      # Session FTS 搜索
│   ├── skill_manager_tool.py        # Skill 管理
│   ├── skills_tool.py              # Skill 工具入口（570 行）
│   ├── skills_hub.py               # Skill hub
│   ├── skills_sync.py              # Skill 同步
│   ├── skills_sync_client.py       # Skill 同步客户端
│   ├── skills_guard.py             # Skill 守卫
│   ├── skills_ast_audit.py         # Skill AST 审计
│   ├── skill_ledger.py             # Skill ledger
│   ├── skill_linter.py             # Skill linter
│   ├── skill_provenance.py         # Skill provenance
│   ├── skill_usage.py              # Skill 使用统计
│   ├── skillevaluator_scan.py      # Skill 评估器扫描
│   ├── mcp_tool.py                 # MCP 客户端
│   ├── mcp_oauth.py / mcp_oauth_manager.py / mcp_dashboard_oauth.py # MCP OAuth
│   ├── mcp_schema_cache.py         # MCP schema 缓存
│   ├── mcp_death_supervisor.py     # MCP 死亡监督
│   ├── browser_tool.py             # 浏览器（CDP）
│   ├── browser_cdp_tool.py / browser_lightpanda.py / browser_camofox.py
│   ├── browser_dialog_tool.py / browser_extension_router.py / browser_use_cli.py
│   ├── browser_supervisor.py       # 浏览器监督
│   ├── browser_registry.py / browser_provider.py # 注册
│   ├── terminal_tool.py / terminal_scope.py / terminal_hints.py
│   ├── interrupt.py                # 中断
│   ├── code_execution_tool.py / code_kernel.py / code_kernel_remote.py
│   ├── interpreter_shutdown.py
│   ├── environments/                # 终端后端（local / docker / ssh / modal / daytona / singularity）
│   ├── desktop_ui.py / focus_pane_tool.py # Desktop UI 工具
│   ├── plugin_guard.py / plugin_storage.py # Plugin 工具
│   ├── tirith_security.py / threat_patterns.py / url_safety.py # 安全
│   ├── osv_check.py                # OSV 漏洞扫描
│   ├── web_tools.py / web_search_provider.py / web_search_registry.py
│   ├── image_generation_tool.py / image_source.py
│   ├── video_generation_tool.py / video_gen_provider.py / video_gen_registry.py
│   ├── voice_mode.py / wake_word.py / wakewords/ / transcription_tools.py / tts_tool.py / tts_streaming.py
│   ├── tts_text_normalize.py / neutts_synth.py / voice_client_config.py
│   ├── yuanbao_tools.py            # 元宝工具
│   ├── x_search_tool.py            # X (Twitter) 搜索
│   ├── homeassistant_tool.py       # Home Assistant
│   ├── microsoft_graph_auth.py / microsoft_graph_client.py
│   ├── feishu_doc_tool.py / feishu_drive_tool.py
│   ├── discord_tool.py / send_message_tool.py / react_to_message_tool.py
│   ├── kanban_tools.py             # 看板
│   ├── cronjob_tools.py            # Cron job 工具
│   ├── setup_mcp_tool.py           # MCP 设置
│   ├── clarify_tool.py / clarify_gateway.py
│   ├── process_registry.py         # 进程注册
│   ├── managed_tool_gateway.py     # 托管工具网关
│   ├── tip_tool.py / tour_tool.py
│   ├── osv_check.py
│   ├── blueprint / blueprints.py
│   ├── patch_parser.py
│   ├── tool_search.py              # 工具搜索（懒加载桥）
│   ├── tool_output_limits.py       # 工具输出限制
│   ├── tool_result_storage.py      # 工具结果存储
│   ├── thread_context.py           # 线程上下文
│   ├── spill_safety.py / hook_output_spill.py
│   ├── schema_sanitizer.py
│   ├── fuzzy_match.py
│   ├── binary_extensions.py
│   ├── ansi_strip.py
│   ├── bot_relay.py / bot_mode_dm.py / bot_mode_probe.py / bot_failure_reasons.py
│   ├── self_repo_guard.py
│   ├── env_probe.py / env_passthrough.py / credential_files.py
│   ├── budget_config.py
│   ├── debug_helpers.py / checkpoint_manager.py
│   ├── lazy_deps.py                # 懒加载依赖
│   ├── openrouter_client.py
│   ├── annotate_preview_tool.py / preview_tool.py / open_preview_tool.py / close_preview_tool.py
│   ├── drive_preview_tool.py / apply_layout_tool.py / read_preview_tool.py / read_window_tool.py / audio_container.py
│   ├── xai_http.py / xai_video_tools.py
│   └── ... (更多)
│
├── gateway/                        # 消息网关（72 文件，~6M）
│   ├── run.py                      # 网关主进程（asyncio）
│   ├── session.py                  # 会话管理
│   ├── config.py                   # 网关配置加载
│   ├── hooks.py                    # 网关钩子
│   ├── authz_mixin.py              # 授权 mixin
│   ├── delivery.py / delivery_ledger.py
│   ├── control_socket.py           # 控制 socket
│   ├── disk_status.py / cgroup_cleanup.py
│   ├── channel_directory.py        # 频道目录
│   ├── browser_control_broker.py / browser_control_artifacts.py
│   ├── agent_cache_pressure.py
│   ├── code_skew.py / drain_control.py
│   ├── cwd_placeholder.py / dead_targets.py
│   ├── display_config.py / hosted_room_discussion.py
│   ├── builtin_hooks/              # 总是注册的钩子（空）
│   └── platforms/                  # 20+ 平台适配器
│       ├── telegram.py / discord.py / slack.py / whatsapp.py / signal.py
│       ├── matrix.py / mattermost.py / irc.py / smpp_sms.py
│       ├── email.py / webhook.py / api_server.py
│       ├── homeassistant.py / dingtalk.py / wecom.py / weixin.py / feishu.py / qqbot.py
│       ├── bluebubbles.py / yuanbao.py / teams.py / simplex.py / line.py / ntfy.py
│       ├── google_meet.py / photon.py / raft.py / buzz.py
│       └── ... (新增平台见 ADDING_A_PLATFORM.md)
│
├── plugins/                        # 插件系统（28 子目录）
│   ├── memory/                     # 内存插件（honcho / mem0 / supermemory / byterover / hindsight / holographic / openviking / retaindb）
│   ├── context_engine/             # context_engine 插件
│   ├── model-providers/            # 推理 backend 插件（openrouter / anthropic / gmi / deepseek / nvidia / nous / openai-codex / ...）
│   ├── cron_providers/             # cron provider 插件
│   ├── kanban/                     # 多代理看板 dispatcher + worker
│   ├── hermes-achievements/        # 游戏化成就
│   ├── observability/              # 指标/追踪/日志
│   ├── image_gen/                  # 图像生成 providers
│   ├── video_gen/                  # 视频生成
│   ├── platform/                   # messaging 平台插件
│   ├── browser/                    # 浏览器 providers
│   ├── dashboard_auth/             # dashboard 认证
│   ├── disk-cleanup/               # 磁盘清理
│   ├── google_meet/                # Google Meet
│   ├── spotify/                    # Spotify
│   ├── teams_pipeline/             # Teams pipeline
│   ├── security-guidance/          # 安全指导
│   └── web/                        # web 插件
│
├── skills/                         # 内置 skill（13 类别目录）
│   ├── apple / autonomous-ai-agents / creative / devops / email
│   ├── index-cache / media / note-taking / productivity / research
│   ├── social-media / software-development / web
│
├── optional-skills/                # 重/小众 skill（默认不激活）
│
├── optional-mcps/                  # 可选 MCP servers
│
├── mcp-research-data/              # MCP 研究数据
│
├── providers/                      # 旧 provider 路径（向后兼容，plugins/model-providers/ 新路径）
│   └── base.py / __init__.py / README.md
│
├── ui-tui/                         # TypeScript Ink TUI
│   ├── package.json
│   ├── src/
│   │   ├── entry.tsx               # Ink 入口
│   │   ├── app.tsx                 # 主 App 组件
│   │   ├── gatewayClient.ts        # Python gateway client
│   │   ├── gatewayTypes.ts         # JSON-RPC 类型
│   │   ├── theme.ts                # 主题
│   │   ├── components/             # UI 组件
│   │   ├── hooks/                  # hooks
│   │   ├── lib/                    # lib
│   │   ├── domain/                 # 领域类型
│   │   ├── content/                # 内容渲染
│   │   ├── config/                 # 配置
│   │   ├── protocol/               # 协议
│   │   ├── sdk/                    # SDK（hermes-ink）
│   │   ├── app/                    # 应用路由
│   │   │   └── slash/              # slash 命令
│   │   └── types.ts                # 类型
│   ├── packages/
│   │   └── hermes-ink/             # @hermes/ink 自研 Ink 组件库
│   └── README.md
│
├── tui_gateway/                    # TUI 的 Python JSON-RPC 后端
│   ├── server.py                   # JSON-RPC 方法 / 事件目录
│   ├── slash_worker.py             # 持续运行的 slash 命令子进程
│   ├── entry.py                    # 入口
│   └── ...
│
├── acp_adapter/                    # ACP server（VS Code / Zed / JetBrains 集成）
│   ├── server.py / entry.py / session.py / tools.py
│   ├── auth.py / events.py / permissions.py / edit_approval.py / provenance.py
│   └── __init__.py / __main__.py
│
├── cron/                           # 调度器
│   ├── scheduler.py
│   └── jobs.py
│
├── docs/                           # 设计文档（深 + 专题）
│   ├── ADR.md / design/ / rfcs/ / middleware/ / observability/ / security/ / kanban/
│   ├── billing-lifecycle.md / chronos-managed-cron-contract.md / cron-doctor-spec.md
│   ├── hermes-kanban-v1-spec.pdf
│   ├── micro-compaction.md / profile-routing.md / streaming-tts.md
│   ├── relay-connector-contract.md / rca-ssl-cacert-post-git-pull.md / session-lifecycle.md / state-db-recovery.md
│
├── scripts/                        # 辅助脚本（run_tests.sh / release.py / ...）
│
├── tests/                          # pytest 测试套件（~17k 测试）
├── tests-js/                       # TypeScript 测试
│
├── website/                        # Docusaurus 文档站点
│   ├── docs/
│   └── ...
│
├── apps/                           # 子应用（Desktop）
│   └── desktop/                    # Electron + React 桌面
│
├── locales/                        # i18n YAML（16 语言）
├── assets/                         # 资源文件（图片等）
├── constraints-termux.txt          # Termux 平台约束
├── cli-config.yaml.example         # CLI 配置示例
├── docker/                         # Docker 镜像（immutable）
├── docker-compose.yml / docker-compose.windows.yml
├── Dockerfile
├── flake.nix / flake.lock          # Nix 包
├── nix/                            # Nix 配置
├── setup-hermes.sh / setup.py
├── pyproject.toml
├── uv.lock                         # 依赖锁（ground truth）
├── package.json / package-lock.json
├── SOUL.md                         # 设计哲学 / 灵魂
└── README.md / README.zh-CN.md / README.es.md / README.ur-pk.md
```

---

## 3. 架构骨架

### 3.1 分层

```
┌──────────────────────────────────────────────────────────┐
│ UI 表面（任选其一）                                       │
│   · CLI (Rich + prompt_toolkit)         cli.py 22417 行   │
│   · TUI (Ink / React / TS)             ui-tui/src/       │
│   · Desktop (Electron + React)         apps/desktop/     │
│   · Web Dashboard (React + SPA)       website/web/ + PTY │
│   · Messaging Gateway (20+ 平台)       gateway/run.py   │
│   · ACP server (VS Code / Zed / JetBrains) acp_adapter/ │
├──────────────────────────────────────────────────────────┤
│ AIAgent 主类（run_agent.py, ~10k LOC）                    │
│   ↳ ~300 个方法、~60 init 参数、~90 字段                 │
│   ↳ run_conversation() → agent/conversation_loop.py      │
├──────────────────────────────────────────────────────────┤
│ conversation_loop.py（9285 行）—— 真正的状态机主循环     │
│   ↳ turn_context.py（1709 行）每轮 setup                 │
│   ↳ conversation_compression.py（6123 行）压缩流水线     │
│   ↳ memory_manager.py（1436 行）多 provider 内存         │
│   ↳ subagent_lifecycle.py（542 行）子代理生命周期         │
│   ↳ tool_executor.py / tool_dispatch_helpers / guardrails│
│   ↳ auxiliary_client.py（curator / vision / embedding）  │
├──────────────────────────────────────────────────────────┤
│ LLM Provider 适配层                                       │
│   ↳ OpenAI (chat + responses) / Anthropic / Codex       │
│   ↳ Bedrock / Azure（adapter 形式）                       │
│   ↳ plugins/model-providers/ 30+ 推理后端                 │
├──────────────────────────────────────────────────────────┤
│ 工具层（tools/ —— 143 文件自动发现）                       │
│   ↳ tools/registry.py —— check_fn + 权限 + plugin 隔离  │
│   ↳ toolsets.py —— _HERMES_CORE_TOOLS + toolset 解析     │
│   ↳ environments/ —— 7 终端后端（local/docker/ssh/modal/ │
│     daytona/singularity/vercel）                          │
├──────────────────────────────────────────────────────────┤
│ Skill 系统（skills/ + optional-skills/）                  │
│   ↳ skills_tool.py —— 570 行发现/调用                     │
│   ↳ skill_commands.py —— 注入为 user msg（保护缓存）      │
│   ↳ skill_ledger.py / skill_linter.py / skills_guard.py  │
├──────────────────────────────────────────────────────────┤
│ 持久化（hermes_state.py 17317 行，SQLite + FTS5）          │
│   ↳ SessionDB —— 会话存储、消息、FTS5 跨会话搜索          │
│   ↳ plugin/memory/ —— 8 种 MemoryProvider                │
│   ↳ plugin/context_engine/ —— 上下文压缩插件             │
└──────────────────────────────────────────────────────────┘
```

### 3.2 主循环入口

- `run_agent.py:467` `class AIAgent` —— ~300 个方法，~60 个 `__init__` 参数，~90 个字段
- `run_agent.py:9238` `AIAgent.run_conversation(...)` —— 转发到 `agent.conversation_loop.run_conversation`
- `agent/conversation_loop.py:2026` `run_conversation(...)` —— 真正的状态机主体（9285 行），**同步循环 + 中断检查 + 预算跟踪 + 一轮 grace call**

```python
# agent/conversation_loop.py:2289 — 核心 loop
while (api_call_count < agent.max_iterations
       and agent.iteration_budget.remaining > 0) \
      or agent._budget_grace_call:
    if self._interrupt_requested: break
    response = client.chat.completions.create(
        model=model, messages=messages, tools=tool_schemas,
    )
    if response.tool_calls:
        for tool_call in response.tool_calls:
            result = handle_function_call(
                tool_call.name, tool_call.args, task_id,
            )
            messages.append(tool_result_message(result))
        api_call_count += 1
    else:
        return response.content
```

### 3.3 消息模型

Hermes **严格遵循 OpenAI Chat Completions 格式**（`AGENTS.md` 注释 "Messages follow OpenAI format: `{"role": "system/user/assistant/tool", ...}`"），通过适配器层转换为对应 Provider 格式：

| 角色 | 字段 | 备注 |
|------|------|------|
| `system` | `content` | 来自 `_build_system_prompt()`（`run_agent.py:5355`） |
| `user` | `content` / 多模态 | Telegram / Discord / CLI 多种来源 |
| `assistant` | `content` + `tool_calls` + `reasoning` | reasoning 存于 `assistant_msg["reasoning"]`（`AGENTS.md:498`） |
| `tool` | `content` + `tool_call_id` | OpenAI 风格 |

### 3.4 工具系统

- **注册中心**：`tools/registry.py` —— `registry.register(name, toolset, schema, handler, check_fn, requires_env)`（`AGENTS.md:534-558`）
- **自动发现**：`discover_builtin_tools()`（`tools/registry.py:111`）扫描 `tools/*.py` 自动 import 含 `registry.register()` 调用的文件
- **Toolset 分组**：`toolsets.py:_HERMES_CORE_TOOLS`（`toolsets.py:31`）+ `resolve_toolset()` / `resolve_multiple_toolsets()` 解析包含关系
- **调度**：`model_tools.py:handle_function_call()`（1707 行，`model_tools.py:1251`） —— 中央分发器
- **插件工具**：`PluginManager.discover_plugins()` 发现 `~/.hermes/plugins/`、pip entry points、`./.hermes/plugins/`，通过 `ctx.register_tool(...)` 注册
- **Plugin override 隔离**：`tools/registry.py:623` `register_plugin_override_policy()` —— 防止 plugin 覆盖同名 core tool

### 3.5 Skill 系统

Hermes 严格遵循 [`agentskills.io`](https://agentskills.io) 开放标准（`README.md:23`）：

- `SKILL.md`（声明式 skill 文件，YAML frontmatter + Markdown 正文）
- 用户目录：`~/.hermes/skills/`
- 项目目录：`.hermes/skills/`
- 内置目录：`skills/`（13 类别）+ `optional-skills/`（默认不激活）
- 注入方式：**作为 user message 注入**（`AGENTS.md:486` "Skill slash commands... injects as **user message** (not system prompt) to preserve prompt caching"）
- CLI 暴露为 slash 命令：`agent/skill_commands.py` 扫描并生成 `/skill-name` 命令

### 3.6 Provider 适配

- OpenAI（Chat Completions + Codex Responses）：内置（`agent/openai_*`）
- Anthropic（Messages API）：内置（`agent/anthropic_adapter.py` / `anthropic_message_convert.py` / `anthropic_endpoints.py` / `anthropic_credentials.py`）
- AWS Bedrock：内置（`agent/bedrock_adapter.py`）
- Azure Identity：内置（`agent/azure_identity_adapter.py`）
- **30+ provider** 通过 `plugins/model-providers/<name>/` lazy discovery（`providers/__init__.py._discover_providers()`）

### 3.7 Session / 持久化

- **存储**：`hermes_state.py`（17317 行），SQLite + **FTS5 全文搜索**
- **多 Profile**：`hermes_constants.py:get_hermes_home()` —— 支持 `~/.hermes/<profile_name>/` 多 profile（互相隔离）
- **跨会话搜索**：`tools/session_search_tool.py` + `agent/memory_manager.py` 提供 FTS5 + LLM 摘要

### 3.8 内存 / 学习循环

Hermes 的核心差异是**"自改进"**：

- **MemoryProvider ABC**（`agent/memory_provider.py`）：8 种内置实现（honcho/mem0/supermemory/byterover/hindsight/holographic/openviking/retaindb）
- **Honcho dialectic user modeling**（`README.md:23`）—— 跨会话构建"用户身份认知"
- **Skill auto-creation**：任务完成后自动提炼 skill（`agent/auxiliary_client.py` curator + `agent/conversation_compression.py`）
- **Skill self-improve during use**（`README.md:22`）—— 使用中自动修正
- **Periodic nudges**（`README.md:22`） —— 后台 nudge 提醒持久化知识
- **FTS5 session search**（`hermes_state.py` + `tools/session_search_tool.py`）

---

## 4. 核心特征

| 能力 | 支持 | 位置 / 说明 |
|------|------|-------------|
| 多 Agent | **强** | `Multi-agent orchestrator` + `subagent_lifecycle.py` + `tools/delegate_tool.py` + `plugins/kanban/`（看板多 worker） |
| 任务分类 | **无内置分档** | 模型自主判断；不强制三档 |
| 任务拆解 | **有 Todo** | `tools/todo_tool.py` —— 与 message 持久化集成 |
| 质检 | **后台 review** | `agent/background_review.py` —— 活体空闲时跑 review（不阻塞 turn） |
| MCP | **客户端 + 服务端** | `tools/mcp_tool.py`（客户端）+ `mcp_serve.py`（服务端） |
| Skill | **支持，一等公民** | 遵循 `agentskills.io` 标准；前端作为 user message（保护缓存） |
| Session | **强** | SQLite + FTS5；多 Profile；JSON-RPC resume |
| 多 Provider | **广泛** | 30+ plugin provider；OpenAI / Anthropic / Bedrock / Azure 内置 |
| 流式响应 | **是** | `assistant_message["reasoning"]` 存推理；stream_callback 触发 TTS |
| 并发执行工具 | **部分** | 主要串行；浏览器 / 委派并行 |
| Plugin 系统 | **强** | general / memory / model-provider / cron / kanban 四大类插件表面 |
| 项目信任 | **是** | `tools/self_repo_guard.py` / `tools/path_security.py` / `tools/skills_guard.py` |
| 沙箱 | **多后端** | `tools/environments/` 7 后端（local/docker/ssh/modal/daytona/singularity/vercel） |
| i18n | **16 语言** | `locales/*.yaml` |
| 桌面 / Web / 移动 / 消息 | **是** | Electron Desktop + Web Dashboard + 20+ messaging platforms + CLI + TUI |

---

## 5. 关键文件清单（25 个核心源文件）

| # | 路径 | 行数 | 职责 |
|---|---|---|---|
| 1 | `run_agent.py` | 10152 | AIAgent 主类，~300 方法 |
| 2 | `cli.py` | 22417 | HermesCLI —— 交互式 CLI 编排（Rich + prompt_toolkit） |
| 3 | `agent/conversation_loop.py` | 9285 | 真正的 run_conversation 状态机主循环 |
| 4 | `agent/conversation_compression.py` | 6123 | 压缩流水线（含 lease / fence / 并发控制） |
| 5 | `model_tools.py` | 1707 | 工具编排、handle_function_call |
| 6 | `toolsets.py` | 1062 | Toolset 定义 + _HERMES_CORE_TOOLS |
| 7 | `tools/registry.py` | ~800 | 工具注册中心 + check_fn 缓存 + plugin override |
| 8 | `tools/skills_tool.py` | ~570 | Skill 发现 + 注入 |
| 9 | `hermes_state.py` | 17317 | SessionDB SQLite + FTS5 |
| 10 | `agent/turn_context.py` | 1709 | 每轮上下文构建（prelogue） |
| 11 | `agent/memory_manager.py` | 1436 | 多 MemoryProvider 编排 |
| 12 | `agent/subagent_lifecycle.py` | 542 | SubAgent 生命周期 |
| 13 | `agent/context_engine.py` | ~500 | ContextEngine 插件抽象 |
| 14 | `agent/tool_executor.py` | ~400 | 工具执行 + 中断 + 守卫 |
| 15 | `agent/tool_guardrails.py` | ~300 | 工具守卫（path security / threat patterns） |
| 16 | `agent/background_review.py` | ~400 | 后台 review（活体空闲时跑） |
| 17 | `agent/auxiliary_client.py` | ~600 | 副 LLM 客户端（curator / vision / embedding / title） |
| 18 | `agent/context_compressor.py` | ~400 | 压缩器主逻辑 |
| 19 | `agent/native_compaction.py` | ~300 | 原生压缩（无 LLM 路径） |
| 20 | `agent/anthropic_adapter.py` | ~600 | Anthropic 适配 |
| 21 | `agent/openai_adapter.py` | ~500 | OpenAI 适配 |
| 22 | `agent/codex_responses_adapter.py` | ~500 | Codex Responses API 适配 |
| 23 | `agent/coding_context.py` | ~300 | 编码上下文 |
| 24 | `hermes_cli/commands.py` | ~400 | 集中式 slash 命令注册表 |
| 25 | `hermes_cli/skin_engine.py` | ~300 | 数据驱动主题引擎 |
| 26 | `hermes_cli/plugins.py` | ~400 | PluginManager 发现/加载 |
| 27 | `hermes_cli/setup_wizard.py` | ~600 | 首次安装向导 |
| 28 | `hermes_bootstrap.py` | 239 | Windows UTF-8 + sys.path 守卫（每个入口顶部导入） |
| 29 | `hermes_constants.py` | ~600 | profile-aware 路径常量 |
| 30 | `gateway/run.py` | ~2000 | 消息网关主进程（asyncio） |
| 31 | `ui-tui/src/app.tsx` | ~500 | Ink 主 App 组件 |
| 32 | `ui-tui/src/gatewayClient.ts` | ~300 | JSON-RPC 客户端 |
| 33 | `tui_gateway/server.py` | ~600 | JSON-RPC 后端 |
| 34 | `acp_adapter/server.py` | ~400 | ACP 服务端（VS Code 集成） |
| 35 | `tests/` | ~17k 测试 | pytest 套件 |

---

## 6. 独特设计（对比 LsmAgentEmergentWork / YoloAgent）

### 6.1 "窄腰"哲学（Per-conversation Prompt Caching is Sacred）

Hermes 的 AGENTS.md 第一句话：

> "Per-conversation prompt caching is sacred. A long-lived conversation reuses a cached prefix every turn. Anything that mutates past context, swaps toolsets, or rebuilds the system prompt mid-conversation invalidates that cache and multiplies the user's cost. We do not do it (the one exception is context compression)."

`AGENTS.md:9-13`

这导致以下设计：

- **Skill 注入为 user message 而非 system prompt**（`AGENTS.md:486`）—— 保护 system prompt 缓存
- **System prompt 中途不重建**（仅在压缩时例外）
- **Toolset 中途不切换**（避免 schema 变化破坏缓存）
- **多 profile 隔离** —— 不同 profile 完全独立的 conversation

与 laew 相比：laew 的 `Yolo Runner` 在每条输入前都做三步分析并重新拼装 system prompt —— 对 Hermes 来说这是"反缓存模式"。laew 的 Yolo 层如果未来要做 prompt caching，需要分离"工具描述 / 系统提示 / 用户上下文"为独立层。

### 6.2 "工具集即 Surface Gate"（The Toolset is the Surface Gate）

`AGENTS.md:152-184`：

> "The toolset is the surface gate. Keep the tools off `_HERMES_CORE_TOOLS` (nobody else should pay their schema) and put them in a named toolset — `desktop_ui`, `project`. The GUI gateway's `_load_enabled_toolsets(platform)` folds that toolset in when the session's platform says GUI."

核心思想：**能力按"使用上下文"分桶，而非按"是否内置"分桶**。同一份工具集中：

- CLI / TUI 不加载 desktop_ui
- messaging 不加载本地终端工具
- 只有 Desktop 后端加载 desktop_ui

`check_fn` 仅用于"可达性"（"已配置 token？"），不是"是否显示"。

与 laew 相比：laew 的 `_HERMES_CORE_TOOLS` 是静态的。Hermes 模式更接近 laew 的"按 Agent 类型分配 toolset"（Yolo 仅 Read、Work Agent 有 Bash+Read），但 Hermes 还按 platform 加 layer。

### 6.3 "Footprint Ladder"（能力新增的 6 级决策）

`AGENTS.md:103-149`：新能力按"留痕量"由小到大分 6 级：

1. **Extend existing code** —— 零新表面
2. **CLI command + skill** —— 零 model-tool 留痕（`hermes webhook` / `hermes cron`）
3. **Service-gated tool (`check_fn`)** —— 仅当 prerequisites 配置时出现
4. **Plugin** —— `~/.hermes/plugins/<name>/`（pip entry point 也可）
5. **MCP server (in catalog)** —— 通过内置 MCP 客户端连接
6. **New core tool** —— 仅当"terminal + file 不可达"时才考虑

这是 Hermes **强约束**的工程纪律 —— 大部分 PR 会在第 1-3 级被重定向。

### 6.4 自改进学习循环（"The only agent with a built-in learning loop"）

`README.md:14-15`：

- **Skill auto-creation** —— 复杂任务完成后自动提炼 skill（curator LLM）
- **Skill self-improvement** —— 使用中自动修正（skill_linter / skill_ledger）
- **Periodic nudges** —— 后台 nudge 提醒持久化知识
- **FTS5 session search** —— 跨会话检索 + 摘要
- **Honcho dialectic user modeling** —— 跨会话构建用户身份认知

与 laew 相比：laew 有 SessionContext Agent（任务后摘要）+ agent_memory 表（持久化记忆），但没有"skill 自改进"和"自动提炼 skill"机制。laew 的 Skill 系统目前是空的。

### 6.5 Plugin 多 surface 分类

Hermes 不止一个 plugin 系统：

- `plugins/<name>/` —— 通用 PluginManager（hooks + tools + CLI subcommand）
- `plugins/memory/<name>/` —— MemoryProvider（bundled-first，与通用 system 顺序相反）
- `plugins/model-providers/<name>/` —— 推理 backend（lazy discovery，独立于通用 PluginManager）
- `plugins/context_engine/<name>/` —— 上下文压缩插件
- `plugins/cron_providers/<name>/` —— cron 任务源
- `plugins/kanban/` —— 多 agent 看板 dispatcher + worker
- `plugins/observability/` —— metrics/log
- `plugins/image_gen/`、`plugins/video_gen/` —— 图像/视频生成 provider

与 laew 相比：laew 的扩展只有 `agent/tools/` 注册 + `llm/` 协议适配，无 plugin 体系。

### 6.6 多 UI 表面共享同一 AIAgent

同一个 `AIAgent` 被：

- 经典 CLI（`cli.py` HermesCLI，Rich + prompt_toolkit）
- TUI（`ui-tui/src/` Ink + `tui_gateway/server.py` JSON-RPC）
- Desktop（`apps/desktop/` Electron + React，spawn `hermes serve` 后端）
- Web Dashboard（`hermes_cli/web_server.py` + xterm.js，PTY bridge 到 `hermes --tui`）
- Messaging Gateway（`gateway/run.py` asyncio，20+ 平台适配器）
- ACP server（`acp_adapter/server.py`，VS Code / Zed / JetBrains 集成）
- Batch（`batch_runner.py`，并行批处理）
- Mini-SWE runner（`mini_swe_runner.py`，SWE-bench mini 评测）

每种"前端"通过 `tool_progress_callback` / `stream_callback` / `clarify_callback` / `interrupt` 等钩子注入，而不是 fork AIAgent。

### 6.7 Toolset 替代 Yolo 三档分类

Hermes **没有 Yolo 入口层**，但通过 toolset 实现了"按场景分桶"：

- `_HERMES_CORE_TOOLS` —— 通用
- `desktop_ui` —— 仅 Desktop / GUI
- `messaging` —— messaging 平台
- `kanban` —— 多 agent 看板
- `coding_context` —— 编码场景
- ...

每次开启 Session 时，根据 `platform` 解析 toolset。**没有"按任务难度分类"**（simple/medium/hard），也没有 Yolo 三步分析（目的→目标→意图）。

与 laew 相比：laew 的 Yolo 入口是"先分类再分发"的两阶段模型；Hermes 是"按 platform 直接拼 toolset"的单阶段模型。

### 6.8 严苛的依赖固定（Exact Pin 政策）

`pyproject.toml:46-49` 注释：

> "Rationale: ranges allow PyPI to ship a fresh version of a transitive at any time without a code review on our side. Exact pins mean the only way a new package version reaches a user is via an intentional update on our end (bump the pin in this file, regenerate uv.lock). This was tightened on 2026-05-12 in response to the Mini Shai-Hulud worm hitting mistralai 2.4.6 on PyPI"

与 laew 相比：laew 用 Cargo.toml + Cargo.lock 也是同等严格策略（lockfile ground truth）。

### 6.9 极强的本地化（16 语言 i18n）

`locales/` 含 16 种语言（en/zh/zh-hant/es/fr/de/ja/ko/pt/ru/uk/it/nl/af/ar/tr/hu/ur）。`TUI busy-indicator styles` 在 `hermes_constants.py` 是"前后端单一事实源" —— Python 端 `INDICATOR_STYLES` / `DEFAULT_INDICATOR_STYLE` 与 TypeScript 端 `ui-tui/src/app/interfaces.ts` 的 `INDICATOR_STYLES` / `DEFAULT_INDICATOR_STYLE` 保持同步。

与 laew 相比：laew 目前仅中文 UI（`hermes_constants.py` 注释说明），未做 i18n。

---

## 7. 小结

Hermes 是一个**高度产品化、插件化、多 surface**的 Python AI Agent：

- 同一 AIAgent 类被 8 种前端（CLI/TUI/Desktop/Web/Gateway/ACP/Batch/Mini-SWE）共享
- **窄腰哲学**：system prompt 中途不重建、toolset 不切换（仅压缩例外）以保护 Anthropic prompt caching
- **Footprint Ladder**：能力新增按留痕量分 6 级，强制走 plugin/MCP 路径
- **自改进学习循环**：skill auto-creation + self-improvement + Honcho user modeling
- **多 plugin 表面**：general / memory / model-provider / context-engine / cron / kanban
- **30+ Provider** 通过 lazy plugin discovery
- **强沙箱纪律**：7 种 terminal backend（local/docker/ssh/modal/daytona/singularity/vercel）
- **严苛依赖固定**：exact pin（防止 supply-chain attack）

对比 laew（Rust 单进程 + Yolo 内置编排 + SQLite session_memory），核心借鉴方向有：**窄腰 + prompt caching 友好的 system prompt 设计**、**多 surface 共享同一 AIAgent**、**plugin 体系（特别是 memory / provider plugin）**、**Footprint Ladder 的"克制核心"哲学**、**学习循环（skill auto-creation）**。

但 Hermes 与 laew 架构有显著差异：

- **语言**：Python vs Rust（Hermes 选择 Python 是因为生态成熟、开发者多、Anthropic SDK 优先 Python）
- **入口设计**：Hermes 无 Yolo 三档分类，laew 内置
- **Provider 数量**：Hermes 30+ plugin，laew 2
- **Skill 系统**：Hermes 遵循 agentskills.io 标准 + user msg 注入；laew 无 Skill
- **学习循环**：Hermes 内置"自改进"，laew 仅 SessionContext Agent 摘要
- **沙箱**：Hermes 7 backend；laew 仅本地 + TODO