# JiuwenSwarm 源码深度调研报告

> **调研日期**: 2026-09-05
> **工程路径**: `/usr/local/LsmGitOpenSource/jiuwenswarm`
> **语言/框架**: Python 3.11+ / asyncio / openjiuwen(内部框架)
> **代码规模**: ~33.8 万行 Python(jiuwenswarm 主包), 858 个 .py 文件, 54MB
> **许可证**: Apache 2.0（华为技术有限公司主导）
> **定位**: 让多智能体真正协作起来的 Agent 系统，支持 Leader 分解任务/组建团队/多 Agent 专业化协作/SwarmFlow 确定性多阶段工作流

---

## 目录

1. [工程结构](#1-工程结构)
2. [核心架构](#2-核心架构)
3. [多 Agent 协作](#3-多-agent-协作)
4. [Agent 通信协议](#4-agent-通信协议)
5. [Skill 系统](#5-skill-系统)
6. [SwarmFlow](#6-swarmflow)
7. [多渠道接入](#7-多渠道接入)
8. [网关层](#8-网关层)
9. [运行时](#9-运行时)
10. [服务端](#10-服务端)
11. [记忆系统](#11-记忆系统)
12. [MCP 运行时](#12-mcp-运行时)
13. [沙箱执行](#13-沙箱执行)
14. [安全控制](#14-安全控制)
15. [上下文管理](#15-上下文管理)
16. [可观测性](#16-可观测性)
17. [多模型支持](#17-多模型支持)
18. [对 laew 工程的借鉴建议](#18-对-laew-工程的借鉴建议)

---

## 1. 工程结构

### 1.1 顶层布局

```
jiuwenswarm/
├── jiuwenswarm/          # 主包 (~33.8 万行)
│   ├── app.py            # 编排 AgentServer + Gateway 双进程
│   ├── start_services.py # 多实例启动/停止/重启 CLI
│   ├── agents/           # Agent harness 层
│   │   ├── harness/      # 单 agent 装配 (rails/tools/team)
│   │   └── swarm/        # 多 agent 声明式装配
│   ├── channels/         # 多渠道接入 (web/tui/cli/desktop/browser/acp)
│   ├── gateway/          # 网关层 (channel_manager/message_handler/routing)
│   ├── runtime/          # 运行时 (service/context/plan/session_provisioner)
│   ├── server/           # 服务端 (agent_manager/warm_pool/runtime/sandbox)
│   ├── symphony/         # 记忆/检索/索引/进化子系统
│   ├── extensions/       # 扩展系统 (hooks/loader/manager/registry)
│   ├── common/           # 公共库 (config/utils/mode_matrix/e2a/mcp)
│   ├── observability/    # 可观测性 (store/sink/projection)
│   └── cli/              # 薄 CLI 入口
├── jiuwenbox/            # 沙箱运行器 (独立子包)
├── docs/                 # 文档 (中/英)
├── tests/                # 测试
└── pyproject.toml        # 包配置
```

### 1.2 分层架构总览

```
┌─────────────────────────────────────────────────────────────────┐
│                     Channels (多渠道接入)                         │
│  Web │ TUI │ CLI │ Desktop │ Browser │ ACP │ 10+ IM 平台适配器   │
└────────────────────────────┬────────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────────┐
│                      Gateway (网关层)                            │
│  ChannelManager │ MessageHandler │ Routing │ Cron │ Hooks       │
└────────────────────────────┬────────────────────────────────────┘
                             │ WebSocket
┌────────────────────────────▼────────────────────────────────────┐
│                   AgentServer (服务端)                           │
│  AgentManager │ AgentWarmPool │ Runtime │ Skill │ MCP │ A2UI    │
└────────────────────────────┬────────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────────┐
│                    Agent Harness (Agent 层)                      │
│  DeepAgent │ CodeAgent │ Team(Leader/Teammate) │ Swarm          │
└────────────────────────────┬────────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────────┐
│              Symphony (记忆/检索/索引/进化) + LLM               │
└─────────────────────────────────────────────────────────────────┘
```

### 1.3 启动流程

`start_services.py` 是统一入口，支持多实例管理（`--name/--list/--status/--stop/--restart`）。核心流程：

1. 早期解析 `--dotenv`（`dotenv_early.parse_dotenv_early`）
2. 准备运行时工作区（`prepare_runtime_workspace`）
3. 加载 `.env` 与 `config.yaml`
4. 启动子进程：`jiuwenswarm.app`（AgentServer+Gateway 编排）+ `jiuwenswarm.channels.web.app_web`（前端）
5. 端口冲突自动回退（`_resolve_ports_with_fallback`）
6. 等待服务就绪（`_wait_for_services_ready`）

`app.py` 进一步分裂为两个子进程：
- `jiuwenswarm.server.app_agentserver`：Agent WebSocket 服务端
- `jiuwenswarm.gateway.app_gateway`：网关 HTTP/WebSocket 服务端

---

## 2. 核心架构

### 2.1 设计哲学

JiuwenSwarm 的核心设计哲学是 **"纯声明式装配 + 跨序列化边界重建"**：

1. **声明式 Spec**：Agent 能力（rails/tools/subagents）全部声明为 `RailSpec`/`BuiltinToolSpec`/`SubAgentSpec`，由 `type` + `params` 描述
2. **属性/环境分离**：`params` 承载 config 派生的**属性值**（换请求不变），`SwarmBuildContext` 承载**环境值**（随请求/会话变化）
3. **跨边界重建**：通过 `build_context_seed` + `register_build_context_factory` 实现分布式 spawn/热恢复

### 2.2 核心数据流

```
用户输入 → Channel → Gateway ChannelManager
         → MessageHandler.handle_message()
         → AgentServer WebSocket
         → AgentManager.get_or_create_agent()
         → DeepAgent.process_message_stream()
         → ReAct 循环 (LLM → tool_calls → 执行 → 回填)
         → 响应流回传 Channel
```

### 2.3 关键抽象

| 抽象 | 文件 | 职责 |
|------|------|------|
| `AgentRuntime` | `runtime/service.py` | 进程级生命周期 owner |
| `AgentManager` | `server/runtime/agent_manager.py` | Agent 实例缓存/热池/热重载 |
| `MessageHandler` | `gateway/message_handler/message_handler.py` | 消息路由/会话/模式 |
| `ChannelManager` | `gateway/channel_manager/channel_manager.py` | Channel 注册/派发 |
| `PlanModeController` | `runtime/plan.py` | Plan 模式状态机 |
| `RuntimeSessionProvisioner` | `runtime/session_provisioner.py` | Session 删除事务 |

### 2.4 模式矩阵

`common/mode_matrix.py` 定义了 **8 种 canonical 模式**（三段命名）：

```
{agent|team}.{work|code}.{normal|plan}
```

- `agent.work.normal` — 单 agent 工作档
- `agent.code.plan` — 单 agent 代码规划
- `team.work.normal` — 集群工作档
- `team.code.plan` — 集群代码规划

Web 前端组合 `mode + work_mode`，TUI/CLI/IM 直接发送完整模式串。

---

## 3. 多 Agent 协作

### 3.1 Leader-Teammate 模式

`agents/swarm/` 实现了声明式多 Agent 装配：

```python
# assembly.py
def enrich_team_spec_for_swarm(spec, *, session_id, mode, project_dir, ...):
    register_swarm_providers()                     # 注册所有 provider
    ctx = SwarmBuildContext(...)                   # 构造环境载体
    spec.agents["leader"] = build_member_deep_agent_spec(config, mode, "leader")
    spec.agents["teammate"] = build_member_deep_agent_spec(config, mode, "teammate")
    spec.build_context_seed = ctx.to_seed()        # 跨边界序列化
```

### 3.2 任务分解

Leader 通过 `core.task_planning` rail 进行任务分解。`config_specs.py` 按 mode/role 分派能力：

```python
def build_member_capability_specs(config, mode, role) -> (rails, tools):
    # team 档: _COMMON_RAIL_NAMES / _COMMON_TOOL_NAMES
    # code 档: _CODE_RAIL_NAMES / _CODE_SHARED_RAIL_NAMES
    # leader: team_skill_evolution + team_skill_create
    # teammate: member_skill_evolution
```

### 3.3 团队组建

`TeamManager`（`agents/harness/team/team_manager.py`）负责：
- 团队 workspace 初始化
- 成员 spawn（本地/分布式）
- 动态协商（`agent_group.py`）
- 共享 skill 可见性（`skills-visibility.json`）

### 3.4 分布式运行时

`agents/harness/team/distributed_runtime.py` + `remote_member_bootstrap.py` 支持：
- 跨进程/跨机器部署 Leader/Teammate
- WebSocket 通信
- `build_context_seed` 跨边界重建

### 3.5 36 个 Harness 元素

Swarm 声明了 **36 个 harness 元素**（Tool 8 / Rail 18+3 / Sub-agent 1）：

| 类别 | 代表元素 | 模式 |
|------|----------|------|
| Tool | `swarm.skill_toolkit`, `swarm.cron_tools`, `swarm.send_file` | T+K |
| Rail | `swarm.team_skill_evolution`, `swarm.context_processor` | T+K |
| Sub-agent | `swarm.code_agent` | K |

---

## 4. Agent 通信协议

### 4.1 A2A (Agent-to-Agent)

`gateway/channel_manager/protocol/a2a/` 实现 Agent 间通信协议，支持：
- 跨 Agent 消息路由
- 任务委派
- 状态同步

### 4.2 ACP (Agent Communication Protocol)

`acp/stdio_client.py` 实现 **ACP JSON-RPC over stdin/stdout**：

```python
# 关键特性
- JSON-RPC 2.0 over UTF-8
- 多行 pretty-printed JSON 缓冲解析 (json.JSONDecoder.raw_decode)
- 自动审批权限 (ACP_AUTO_APPROVE_PERMISSIONS)
- 子进程生命周期管理 (SIGTERM/SIGKILL 分级)
- session/update 文本提取
```

`channels/acp/app_acp.py` + `gateway/channel_manager/protocol/acp/` 实现 ACP 网关桥接。

### 4.3 E2A (External-to-Agent)

`common/e2a/` 实现外部系统到 Agent 的协议转换：

| 文件 | 职责 |
|------|------|
| `models.py` | E2A 数据模型 |
| `wire_codec.py` | 编解码 |
| `gateway_normalize.py` | 网关归一化 |
| `adapters.py` | 协议适配 |
| `agent_compat.py` | Agent 兼容层 |

`gateway/routing/e2a_proxy.py` 实现 E2A 代理（`fetch_agent_unary`）。

### 4.4 A2UI (Agent-to-UI)

`server/runtime/a2ui/` 实现 **A2UI v0.8 协议**：

```python
A2UI_ACTIVE_PROTOCOL_VERSION = VERSION_0_8

class A2UIProtocolSpec:
    - schema_manager: A2uiSchemaManager
    - catalog: BasicCatalog
    - build_prompt(): 生成系统提示（含 schema）
    - 支持 beginRendering/surfaceUpdate/dataModelUpdate
```

支持富交互 UI 组件：列表、卡片、表单、确认、可点击操作。

---

## 5. Skill 系统

### 5.1 Skill 生命周期

`server/runtime/skill/skill_manager.py`（~32.7 万行）管理：
- 加载/安装/卸载
- SkillNet marketplace 下载
- Team Skills Hub 流通
- 版本归档（`archive_store.py`）
- 内置技能（`get_builtin_skills_dir`）

### 5.2 Skill 自演进

`agents/swarm/providers/evolution_rails.py` 实现：
- `team_skill_evolution` — Leader 级 skill 进化
- `team_skill_create` — Leader 级 skill 创建
- `member_skill_evolution` — 成员级 skill 进化

触发信号：执行出错 / 用户不满 → 自动优化 Skill 定义。

### 5.3 Skill Hub

- **Swarm Skills Hub**: `https://swarmskills.openjiuwen.com/`
- **Team Skills Hub**: `https://teamskills.openjiuwen.com/`
- 支持搜索、安装、组合、二次创作、发布

### 5.4 SkillDev 流水线（12+ 阶段）

`server/runtime/skill/skilldev/` 实现 **确定性工程流水线**：

```
INIT → PLAN → PLAN_CONFIRM* → GENERATE → VALIDATE
    → TEST_DESIGN → TEST_RUN → EVALUATE → REVIEW*
    → IMPROVE → (循环回 TEST_RUN)
    → PACKAGE → DESC_OPTIMIZE_CONFIRM* → DESC_OPTIMIZE → COMPLETED
```

**7 个 RPC 接口**：
- `skilldev.start` — 发起任务
- `skilldev.respond` — 统一确认入口
- `skilldev.status` — 查询状态
- `skilldev.download` — 下载产物
- `skilldev.cancel` — 取消
- `skilldev.file.list` — 文件树
- `skilldev.file.read` — 读取文件

**挂起点机制**（HITL）：
- `PLAN_CONFIRM` — 计划确认
- `REVIEW` — 评测审阅
- `DESC_OPTIMIZE_CONFIRM` — 描述优化确认

**状态机设计**：
- Pipeline 不长驻内存
- StateStore checkpoint 断点续传
- 每阶段独立 Agent（工具/Prompt/内存隔离）

---

## 6. SwarmFlow

### 6.1 确定性多阶段工作流

SwarmFlow 用 **Python 工作流脚本** 做确定性多阶段编排：

```python
# 工作流定义（Python 脚本）
@swarmflow.stage
def research_stage(query: str) -> Team:
    return Team(researcher=..., analyst=...)

@swarmflow.stage
def synthesis_stage(research_output: Team) -> Team:
    return Team(writer=..., reviewer=...)
```

### 6.2 HITL 实现

支持两种人工介入模式：
- `human` — 单步人工确认
- `human_session` — 会话级人工介入

### 6.3 Team Token 预算

`common/cron_team_completion.py` + `team_artifacts.py` 实现：
- Token 预算分配
- 阶段间 token 流转
- 超限保护

### 6.4 TUI 运行树监控

TUI `/swarmflows` 命令提供运行树监控：
- 阶段进度可视化
- 成员状态
- 中间产物浏览

---

## 7. 多渠道接入

### 7.1 渠道矩阵

| 区域 | 渠道 | 路径 |
|------|------|------|
| 中国 | 小翼、飞书、钉钉、企微、个人微信 | `channels/web`, `gateway/channel_manager/im_platforms/` |
| 国际 | Telegram、Discord、Slack、WhatsApp | 同上 |
| 本地 | Web、TUI、CLI、Desktop、Browser | `channels/` |

### 7.2 10+ IM 平台适配器

`gateway/channel_manager/im_platforms/`：

```
im_platforms/
├── xiaoyi/       # 华为小翼
├── feishu/       # 飞书
├── dingtalk/     # 钉钉
├── wecom/        # 企微
├── wechat/       # 个人微信
├── telegram/     # Telegram
├── discord/      # Discord
├── slack/        # Slack
├── whatsapp/     # WhatsApp
└── platform_adapter/  # 平台适配器基类
```

### 7.3 Web Channel

`channels/web/app_web.py`（~92KB）：
- FastAPI + WebSocket
- 前端 SPA（Vite）
- 会话管理
- 文件上传/下载

### 7.4 TUI Channel

`channels/tui/frontend/` — 终端 UI（npm 包 `jiuwenswarm-tui`）

### 7.5 CLI Channel

`channels/cli/chat.py`（~49.5KB）：
- 完整 REPL
- 流式渲染
- 事件处理

### 7.6 Desktop Channel

`channels/desktop/desktop_app.py`（~105KB）：
- Electron/原生桌面端
- 自动更新（`common/updater.py`）

---

## 8. 网关层

### 8.1 Gateway 架构

`gateway/app_gateway.py`（~155KB）是独立进程：

```python
# 启动组件
- MessageHandler + ChannelManager
- WebChannel WebSocket server
- Heartbeat service
- Cron scheduler service
- 连接远程 AgentServer WebSocket
```

### 8.2 ChannelManager

`gateway/channel_manager/channel_manager.py`：

```python
class ChannelManager(ABC):
    - register_channel()       # 注册 + 消息回调
    - _on_channel_message()    # 入站消息 → MessageHandler
    - deliver_to_message_handler()
    - 运行出队派发循环（AgentServer 响应 → Channel）
    - 配置热更新（set_config）
    - Channel 连接事件订阅
```

### 8.3 MessageHandler

`gateway/message_handler/message_handler.py`（~224KB）：
- 消息路由（按 method/session/channel）
- 会话创建/恢复/取消
- 模式解析（`mode_matrix`）
- 进化审批（`evolution_approval.py`）
- 加入/退出处理（`join_exit_handlers.py`）

### 8.4 Routing

`gateway/routing/`：

| 文件 | 职责 |
|------|------|
| `agent_client.py` | AgentServer WebSocket 客户端 |
| `agent_http_bridge.py` | HTTP 桥接 |
| `e2a_proxy.py` | E2A 代理 |
| `session_sharing.py` | 会话共享 |
| `session_map.py` | 会话映射 |
| `route_binding.py` | 路由绑定 |
| `keys.py` | ChannelKey |

### 8.5 IM Pipeline

`gateway/im_pipeline/`：
- `im_inbound.py` — 入站处理（~27.9KB）
- `im_outbound.py` — 出站处理（~17.4KB）

### 8.6 Cron

`gateway/cron/`：
- `scheduler.py` — 调度器（~93.8KB）
- `controller.py` — 控制器（~31.8KB）
- 支持 cron 表达式、定时任务、主动推送

---

## 9. 运行时

### 9.1 AgentRuntime

`runtime/service.py`（~47.3KB）：

```python
class AgentRuntime:
    - agent_manager: AgentManager
    - plan_controller: PlanModeController
    - session_provisioner: RuntimeSessionProvisioner
    - start()  # 初始化 checkpointer/Runner/extensions
    - create_or_resume_session()
    - prepare_chat_turn()
    - cancel_request()
```

进程级单例，管理：
- 共享依赖（checkpointer/Runner）引用计数
- 扩展（ExtensionManager/ExtensionRegistry）
- 生命周期锁

### 9.2 Context

`runtime/context.py`：

```python
_CURRENT_RUNTIME_CONTEXT: ContextVar[RuntimeExecutionContext | None]

def set_runtime_context(runtime, agent_manager) -> Token[...]
def get_current_runtime() -> AgentRuntime | None
def get_current_agent_manager() -> AgentManager | None
```

通过 `ContextVar` 在 asyncio 任务间传播运行时上下文。

### 9.3 Plan Mode

`runtime/plan.py`：

```python
class PlanModeController:
    - _active_sessions: set[str]
    - _exited_sessions: set[str]
    - ensure_state()  # 同步 plan 状态
    - inject_activation_reminder()  # 注入 plan 约束
    - check_post_process_exit()  # 检测 exit_plan_mode
```

Plan 模式约束：
- 只规划，不修改
- 只读工具直接可用
- 写操作被阻止
- `enter_plan_mode` / `exit_plan_mode` 工具

### 9.4 SessionProvisioner

`runtime/session_provisioner.py`（~15.5KB）：

```python
class RuntimeSessionProvisioner:
    - delete_session()  # 事务性删除
    - commit_session_delete()  # 提交删除
    - 支持 SessionDeleteLifecycle 参与者
    - 保证 plan/cache/binding 状态一致
```

---

## 10. 服务端

### 10.1 AgentServer

`server/app_agentserver.py`（~20KB）：
- 启动 Agent WebSocket Server
- 配置日志（`logging.yaml`）
- 安装 shell 工具安全钩子
- SSE 兼容补丁

### 10.2 AgentManager

`server/runtime/agent_manager.py`（~65.9KB）：

```python
class AgentManager:
    - agents: dict[str, dict[str, JiuWenSwarm]]  # channel → cache_key → agent
    - warm_pool: AgentWarmPool
    - create_session()
    - get_or_create_agent()
    - recreate_agent()  # 热重载
    - reload_config()  # 配置热更新
    - pin/unpin_agent()  # 生命周期管理
    - borrow/return 模式
```

### 10.3 AgentWarmPool

`server/runtime/agent_warm_pool.py`（~30KB）：

```python
class AgentWarmPool:
    - WarmKey(channel_id, project_id, project_dir, work_mode, is_swarm)
    - WarmSlot(key, session_id, revision, agent, ready_at)
    - 后台预热（JIUWENSWARM_AGENT_PREWARM 环境变量控制）
    - 有界并发（max_concurrency/max_ready_slots）
    - 配置指纹（config_fingerprint）失效重建
```

### 10.4 Session 子系统

`server/runtime/session/`：

| 文件 | 职责 |
|------|------|
| `session_manager.py` | Session CRUD |
| `session_history.py` | 历史记录 |
| `session_metadata.py` | 元数据 |
| `project_store.py` | 项目存储 |
| `project_git.py` | Git 集成 |
| `git_diff_watcher.py` | Git diff 监控 |
| `work_mode.py` | 工作模式 |
| `kv_cache/` | KV 缓存 |

### 10.5 Agent Adapter

`server/runtime/agent_adapter/`：

| 文件 | 大小 | 职责 |
|------|------|------|
| `interface.py` | 194KB | JiuWenSwarm 主接口 |
| `interface_deep.py` | 692KB | DeepAgent 适配 |
| `interface_code.py` | 121KB | CodeAgent 适配 |
| `team_helpers.py` | 168KB | Team 辅助 |
| `sysop_builder.py` | 36.6KB | 系统操作构建 |
| `evolution_helpers.py` | 28.2KB | 进化辅助 |
| `user_turn.py` | 4KB | 用户轮次 |

---

## 11. 记忆系统

### 11.1 Symphony 子系统

`symphony/` 是独立的 **记忆/检索/索引/进化** 子系统：

```
symphony/
├── service.py          # 编排入口 (~43.4KB)
├── build.py            # 图构建 (~29KB)
├── llm.py              # LLM 集成 (~20.3KB)
├── config.py           # 配置 (~8.2KB)
├── adapter.py          # 适配器 (~5.6KB)
├── graph_state.py      # 图状态 (~6.5KB)
├── graph_storage.py    # 图存储 (~4.8KB)
├── indexing/           # 索引
├── retrieval/          # 检索
├── evolution/          # 进化
└── skill_retrieval/    # Skill 检索
```

### 11.2 图构建

`build.py`：

```python
class SymphonyGraphBuilder:
    - scan() → SkillFolderScanner
    - fingerprint_service() → FingerprintService
    - build() → 全量/增量构建
    - status() → GraphStatus (exists/stale/added/changed/removed)
```

**GraphStatus**：
- `success`, `graph_dir`, `exists`, `stale`
- `skill_count`, `changed_count`, `added_count`, `removed_count`
- `resume_available`, `checkpoint_dir`

### 11.3 检索

`symphony/retrieval/` 实现：
- 语义检索
- 关键词检索
- RRF 融合（`ONLINE_SEARCH_RRF_K = 60`）
- 多源检索（skillnet/teamskillshub/clawhub）

### 11.4 进化

`symphony/evolution/` 实现：
- Skill 进化记录（`evolutions.json`）
- 动态 overlay（`load_dynamic_overlay`）
- 经验积累与应用

### 11.5 记忆分层

| 层级 | 实现 | 用途 |
|------|------|------|
| 任务记忆 | `session_history.py` | 单次任务上下文 |
| 编码记忆 | `code_coding_memory` rail | 项目级代码记忆 |
| 项目记忆 | `code_project_memory` rail | 项目文档/结构 |
| 个人记忆 | `personal_context/` | 用户级持久记忆 |
| 团队记忆 | `team_workspace/` | 团队共享记忆 |

---

## 12. MCP 运行时

### 12.1 MCP 注册中心

`server/runtime/mcp/registry.py`（~60KB）：

```python
# 集成类型检测
def _detect_integration_type(pkg_dir) -> str:
    # cli.json → "cli"
    # mcp.json + command → "stdio-mcp"
    # mcp.json + url → "remote-mcp"
    # skills/ 目录 → "skill-only"
```

**四种 MCP 形态**：
- **A. remote-mcp** — 远程 HTTP/SSE
- **B. stdio-mcp** — 本地 stdio
- **C. cli** — CLI 二进制管理（飞书/钉钉等）
- **D. skill-only** — 纯 Skill 无 MCP server

### 12.2 MCP 状态

`server/runtime/mcp/state_store.py`：
- `state.json` 持久化
- 启用/禁用状态
- 连接状态

### 12.3 MCP CLI Driver

`server/runtime/mcp/cli_driver.py`（~26.3KB）：
- CLI MCP 连接
- 安装/卸载
- 凭证管理（`credential.py`）

### 12.4 Skill 安装

`server/runtime/mcp/skill_installer.py`：
- MCP 包内 Skill 安装
- 版本管理

---

## 13. 沙箱执行

### 13.1 JiuwenBox

`jiuwenbox/` 是独立沙箱子包：

```
jiuwenbox/
├── src/jiuwenbox/       # 沙箱核心
├── server/              # uvicorn 服务入口
├── docker/              # Docker 支持
└── tests/
```

### 13.2 JiuwenBox Runner

`server/sandbox/jiuwenbox_runner.py`（~21.5KB）：

```python
class JiuwenBoxRunner:
    - ensure_running()  # 启动/复用
    - health_check()    # /health 探活
    - _owns_process: bool  # 是否拥有子进程
    - startup_mode: internal/external
    - policy_path: 安全策略文件
```

**启动命令**：
```bash
uvicorn jiuwenbox.server.app:app --host 127.0.0.1 --port 8321
```

**安全特性**：
- `JIUWENBOX_POLICY_PATH` 策略注入
- Linux `PR_SET_PDEATHSIG` 父子进程联动
- stderr 滚动缓冲（80 行）
- 端口冲突自动换端口

### 13.3 沙箱策略

`no_host_fallback_jiuwenbox.py` + `sandbox_no_host_fallback.py`：
- 无 host 回退
- 隔离执行
- 资源限制

---

## 14. 安全控制

### 14.1 工具审批

`common/permission_tools.py` + `agents/swarm/providers/code_rails.py`：
- `permission_interrupt` rail — 工具执行前审批
- 三态策略（允许/询问/拒绝）
- 用户确认持久化

### 14.2 文件访问白名单

`agents/swarm/providers/code_rails.py`：
- `code_project_memory` — `additional_directories` 白名单
- `code_coding_memory` — 项目目录约束
- `workspace_paths.py` — 工作区路径安全

### 14.3 敏感操作拦截

`agents/harness/common/tools/bash_tool_safety.py`：
- Shell 命令黑名单
- 危险命令拦截
- `install_shell_tool_safety_hooks()`

### 14.4 安全日志

`app_agentserver.py` 配置专用安全日志：
```python
_sec_logger = logging.getLogger("openjiuwen.harness.security")
_perm_ns_logger = logging.getLogger("jiuwenswarm.agents.harness.common.rails.permissions")
```

### 14.5 WebSocket 安全

`common/security/ws_origin.py`：
- Origin 白名单
- CSRF 防护

### 14.6 密钥脱敏

`observability/store.py` + `debug_trace/stream_logger.py`：
- `_SECRET_TOKENS = {token, password, passwd, pwd, secret, apikey, ...}`
- `_looks_secret()` 智能识别
- `_masked_with_fp()` 脱敏

---

## 15. 上下文管理

### 15.1 RuntimeExecutionContext

`runtime/context.py`：

```python
@dataclass(frozen=True)
class RuntimeExecutionContext:
    runtime: AgentRuntime
    agent_manager: AgentManager

_CURRENT_RUNTIME_CONTEXT: ContextVar[RuntimeExecutionContext | None]
```

通过 `ContextVar` 在 asyncio 任务树中传播。

### 15.2 Personal Context

`server/personal_context/`：

| 文件 | 职责 |
|------|------|
| `host_api.py` (~42.8KB) | 个人上下文 API |
| `ws_handler.py` (~13.8KB) | WebSocket 处理 |

### 15.3 Context Engine

`agents/swarm/providers/member_rails.py`：
- `context_processor` rail
- `context_engine_enabled` / `context_engine_config`
- 上下文窗口管理（`common/context_window.py`）

### 15.4 Context 压缩

`server/runtime/agent_adapter/compact_partial_prompts.py`：
- 部分提示压缩
- 上下文窗口优化

### 15.5 Team Context

`agents/swarm/context.py`：

```python
class SwarmBuildContext(BuildContext):
    - session_id, request_id, channel_id, mode
    - project_dir, team_id, team_ws_root
    - team_skill_visibility_path, global_skills_dir
    - config, trajectory_span_processor
    - extras: dict  # 运行时句柄
```

---

## 16. 可观测性

### 16.1 存储层

`observability/store.py`（~85.7KB）：

```python
# SQLite 存储 OTLP 记录
_SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS otlp_span_records (
    trace_id, span_id, parent_span_id, session_id, request_id, run_id,
    agent_mode, start_time_unix_nano, end_time_unix_nano,
    raw_json BLOB, raw_sha256,
    UNIQUE(trace_id, span_id)
);
CREATE TABLE IF NOT EXISTS trajectory_current_records (...);
CREATE TABLE IF NOT EXISTS trajectory_changes (...);
"""
```

**Schema Version 3**：
- `otlp_span_records` — 原始 span
- `trajectory_current_records` — 当前轨迹
- `trajectory_changes` — 变更日志
- `otlp_record_conflicts` — 冲突记录

### 16.2 Sink

`observability/sink.py`（~37KB）：
- OTLP 数据接收
- 批量写入
- 投影处理

### 16.3 Projection

`observability/projection.py`：
- `TrajectoryScope` 轨迹范围
- `project_trajectory_scope()` 投影

### 16.4 Debug Trace

`server/runtime/debug_trace/`：

| 文件 | 职责 |
|------|------|
| `stream_logger.py` (~24.1KB) | 流式日志 |
| `config.py` | 配置 |
| `context.py` | 上下文 |
| `directives.py` | 指令 |
| `subagent_capture.py` | Sub-agent 捕获 |
| `task_tool_patch.py` | 任务工具补丁 |

**Chunk 类型词汇**：
```python
_CHUNK_LLM_OUTPUT = "llm_output"
_CHUNK_LLM_REASONING = "llm_reasoning"
_CHUNK_LLM_USAGE = "llm_usage"
_CHUNK_ANSWER = "answer"
_CHUNK_TOOL_CALL = "tool_call"
_CHUNK_TOOL_UPDATE = "tool_update"
_CHUNK_TOOL_RESULT = "tool_result"
```

### 16.5 Session 删除

`observability/session_delete.py`：
- 轨迹 session 删除
- 级联清理

---

## 17. 多模型支持

### 17.1 Model Vendor Registry

`common/model_vendor_registry.py`（~23.2KB）：

```python
@dataclass(frozen=True)
class VendorPreset:
    vendor_key: str            # "alibaba", "baidu"
    display_name: str          # "阿里云百炼"
    plan: PlanKind             # TOKEN_PLAN/CODING_PLAN/CUSTOM_API
    client_provider: str       # OpenAI/DashScope/DeepSeek/...
    api_base: str              # OpenAI-format base_url
    anthropic_base: str | None # Anthropic-format base
    default_model: str
    model_options: tuple[str, ...]
    icon_key: str
    models_endpoint: str | None
    endpoint_profile: str | None
```

**PlanKind**：
- `TOKEN_PLAN` — 预付费套餐/资源包/订阅
- `CODING_PLAN` — 编码场景
- `CUSTOM_API` — 通用 Token（按量计费）

**已验证厂商**（2026-08-05 curl 验证）：
- 阿里云百炼、MiniMax、Maas 盘古、百度智能云
- DeepSeek、OpenRouter、SiliconFlow、Anthropic
- 华为云 MaaS、OpenAI、DashScope

### 17.2 多模型客户端

`common/external_cli_runtime.py`（~36.2KB）：
- 外部 CLI 运行时
- 多模型适配

### 17.3 推理配置

`common/reasoning_config.py` + `reasoning_injector.py`：
- 推理模式（extended thinking）
- 推理 token 注入
- 厂商特定规则（`get_provider_reasoning_rules`）

### 17.4 模型配置验证

`common/model_config_validation.py`：
- 配置校验
- 默认模型回退

---

## 18. 对 laew 工程的借鉴建议

### 18.1 架构层面

| 维度 | JiuwenSwarm 实践 | laew 现状 | 借鉴建议 |
|------|------------------|-----------|----------|
| 分层 | Channel/Gateway/Runtime/Server 四层分离 | main/tui/agent/llm 四层 | 增加独立 Gateway 层，解耦 UI 与 Agent |
| 协议 | A2A/ACP/E2A/A2UI 多协议 | Anthropic/OpenAI 双协议 | 引入 ACP 协议支持外部 Agent 协作 |
| 模式 | 8 种 canonical 模式 | simple/medium/hard 三档 | 引入更细粒度的模式矩阵 |
| 装配 | 声明式 Spec + 36 harness 工具 | 3 个内置工具 | 声明式工具/rail 注册机制 |

### 18.2 多 Agent 协作

| 维度 | JiuwenSwarm 实践 | 建议 |
|------|------------------|------|
| 编排 | Leader-Teammate + SwarmBuildContext | 引入 Leader/Teammate 角色 + BuildContext |
| 装配 | 纯声明式（config → spec → runtime） | 从硬编码 profile 迁移到声明式装配 |
| 分布式 | build_context_seed 跨边界重建 | 支持跨进程/跨机器 Agent 部署 |
| 进化 | Skill 自演进 rails | 引入 Skill 自动优化机制 |

### 18.3 Skill 系统

| 维度 | JiuwenSwarm 实践 | 建议 |
|------|------------------|------|
| 生命周期 | 加载/安装/卸载/市场/版本归档 | 建立 Skill 注册中心 |
| 自演进 | 错误信号检测 + 自动优化 | 引入 Skill 质量评估 + 自动修复 |
| SkillDev | 12+ 阶段确定性流水线 | 提供 Skill 开发模式 |
| Hub | Swarm Skills Hub + Team Skills Hub | 建立 Skill 共享生态 |

### 18.4 运行时

| 维度 | JiuwenSwarm 实践 | 建议 |
|------|------------------|------|
| 预热 | AgentWarmPool 后台预热 | 引入 Agent 实例预热池 |
| 模式 | PlanModeController 状态机 | 增强 Plan 模式（HITL 约束） |
| 会话 | SessionProvisioner 事务删除 | 引入 Session 生命周期事务 |
| 上下文 | ContextVar 传播 + 个人/团队记忆 | 引入多层记忆系统 |

### 18.5 安全控制

| 维度 | JiuwenSwarm 实践 | 建议 |
|------|------------------|------|
| 审批 | 工具执行前审批 | 引入工具权限三态策略 |
| 白名单 | 文件访问 additional_directories | 引入路径白名单机制 |
| 拦截 | bash_tool_safety 黑名单 | 引入 Shell 命令安全钩子 |
| 脱敏 | _looks_secret + _masked_with_fp | 增强密钥脱敏（已部分实现） |
| 沙箱 | JiuwenBox 独立沙箱 | 引入可选沙箱执行 |

### 18.6 可观测性

| 维度 | JiuwenSwarm 实践 | 建议 |
|------|------------------|------|
| 存储 | SQLite OTLP 无损存储 | 引入轨迹持久化 |
| 轨迹 | trajectory_current + trajectory_changes | 引入变更日志 |
| Debug | stream_logger + chunk 类型词汇 | 增强 TUI 调试视图 |
| 投影 | TrajectoryScope 投影 | 引入轨迹范围查询 |

### 18.7 多渠道

| 维度 | JiuwenSwarm 实践 | 建议 |
|------|------------------|------|
| 渠道 | Web/TUI/CLI/Desktop/Browser/10+ IM | 增加 Telegram/Discord 适配器 |
| 协议 | A2UI 富交互 UI | 引入 A2UI 协议支持 |
| 网关 | ChannelManager 统一注册 | 引入 Channel 抽象 |

### 18.8 优先级路线图

**P0（立即）**：
1. 引入声明式工具注册（从 hardcoded 迁移）
2. 增加 Plan 模式状态机
3. 引入 Session 生命周期事务

**P1（短期）**：
1. 建立 Skill 注册中心
2. 引入 Agent 预热池
3. 增加工具审批三态策略
4. 引入轨迹持久化

**P2（中期）**：
1. Leader-Teammate 多 Agent 协作
2. ACP 协议支持
3. SwarmFlow 确定性工作流
4. JiuwenBox 沙箱集成

**P3（长期）**：
1. Skill 自演进
2. 分布式 Agent 部署
3. A2UI 富交互 UI
4. 多 IM 平台适配

---

## 附录 A：关键文件索引

| 文件 | 大小 | 核心类/函数 |
|------|------|-------------|
| `app.py` | 5.7KB | `main()` - 双进程编排 |
| `start_services.py` | 40.4KB | `InstanceCommand`, `_run()`, `_start_named_instance()` |
| `gateway/app_gateway.py` | 155KB | Gateway 入口 |
| `gateway/channel_manager/channel_manager.py` | 35.5KB | `ChannelManager` |
| `gateway/message_handler/message_handler.py` | 224KB | `MessageHandler` |
| `runtime/service.py` | 47.3KB | `AgentRuntime` |
| `runtime/plan.py` | 11.8KB | `PlanModeController` |
| `runtime/session_provisioner.py` | 15.5KB | `RuntimeSessionProvisioner` |
| `server/app_agentserver.py` | 20.4KB | AgentServer 入口 |
| `server/runtime/agent_manager.py` | 65.9KB | `AgentManager` |
| `server/runtime/agent_warm_pool.py` | 30KB | `AgentWarmPool` |
| `server/runtime/agent_adapter/interface.py` | 194KB | `JiuWenSwarm` |
| `server/runtime/agent_adapter/interface_deep.py` | 692KB | DeepAgent 适配 |
| `server/runtime/skill/skill_manager.py` | 326KB | `SkillManager` |
| `server/runtime/skill/skilldev/` | ~110KB | SkillDev 流水线 |
| `server/runtime/mcp/registry.py` | 60KB | MCP 注册中心 |
| `server/sandbox/jiuwenbox_runner.py` | 21.5KB | `JiuwenBoxRunner` |
| `symphony/service.py` | 43.4KB | `SwarmSymphonyService` |
| `symphony/build.py` | 29KB | `SymphonyGraphBuilder` |
| `agents/swarm/assembly.py` | 18.4KB | `enrich_team_spec_for_swarm()` |
| `agents/swarm/config_specs.py` | 37.8KB | `build_member_capability_specs()` |
| `agents/swarm/context.py` | 12.3KB | `SwarmBuildContext` |
| `observability/store.py` | 85.7KB | OTLP SQLite 存储 |
| `common/mode_matrix.py` | 18.6KB | `ResolvedMode`, `resolve_request_mode()` |
| `common/model_vendor_registry.py` | 23.2KB | `VendorPreset`, `_PRESETS` |
| `acp/stdio_client.py` | 25.3KB | ACP JSON-RPC 客户端 |

---

## 附录 B：架构图

### B.1 启动流程

```
jiuwenswarm-start
    │
    ├── parse_dotenv_early()
    ├── prepare_runtime_workspace()
    ├── load_dotenv_runtime()
    │
    ├── _build_commands(mode)
    │       ├── jiuwenswarm.app
    │       │       ├── AgentServer (app_agentserver)
    │       │       │       ├── AgentWebSocketServer
    │       │       │       ├── AgentManager
    │       │       │       ├── AgentWarmPool
    │       │       │       └── Runtime
    │       │       └── Gateway (app_gateway)
    │       │               ├── ChannelManager
    │       │               ├── MessageHandler
    │       │               ├── CronScheduler
    │       │               └── Heartbeat
    │       └── jiuwenswarm.channels.web.app_web
    │               ├── FastAPI
    │               ├── WebSocket
    │               └── Frontend (Vite SPA)
    │
    └── _wait_for_services_ready()
```

### B.2 消息流

```
User → Channel → ChannelManager._on_channel_message()
      → MessageHandler.handle_message()
      → AgentServer WebSocket
      → AgentManager.get_or_create_agent()
      → JiuWenSwarm.process_message_stream()
      │
      ├── resolve_request_mode() → ResolvedMode
      ├── PlanModeController.ensure_state()
      ├── DeepAgent.process_message_stream()
      │       ├── LLM call
      │       ├── tool_calls 解析
      │       ├── tool execution (bash/read/write/...)
      │       └── tool_result 回填
      │
      └── Response stream → Channel → User
```

### B.3 多 Agent 协作

```
Team Mode
    │
    ├── Leader (DeepAgent)
    │       ├── core.task_planning (任务分解)
    │       ├── swarm.team_skill_evolution (Skill 进化)
    │       └── swarm.team_skill_create (Skill 创建)
    │
    ├── Teammate 1 (DeepAgent)
    │       ├── member_skill_evolution
    │       └── specialized tools
    │
    ├── Teammate 2 (DeepAgent)
    │       └── ...
    │
    └── SwarmBuildContext (共享环境)
            ├── session_id, channel_id, mode
            ├── team_id, team_ws_root
            └── build_context_seed (跨边界)
```

---

> **总结**: JiuwenSwarm 是一个 **工业级多 Agent 协作系统**，其声明式装配、跨边界重建、Skill 自演进、SwarmFlow 确定性工作流、多渠道接入等设计，为 laew 工程提供了丰富的参考。建议 laew 从 **声明式工具注册**、**Plan 模式状态机**、**Agent 预热池** 三个方向优先借鉴，逐步构建多 Agent 协作能力。
