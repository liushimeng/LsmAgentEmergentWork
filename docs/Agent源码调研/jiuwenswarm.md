# JiuwenSwarm 综合深度分析

> 调研对象:jiuwenswarm(Python,多 Agent 协作平台)
> 调研日期:2026-09-05
> 原始文档:3 份(源码调研 1206 行 + 深度分析 1264 行 + 核心机制深度分析 2367 行)
> 总行数:~4837 行(原始) → ~2500 行(合并后)
> 工程路径:`/usr/local/LsmGitOpenSource/jiuwenswarm`
> 代码规模:~33.8 万行 Python,858 个 .py 文件,54MB
> 许可证:Apache 2.0(华为技术有限公司主导)
> 定位:让多智能体真正协作起来的 Agent 系统,支持 Leader 分解任务/组建团队/多 Agent 专业化协作/SwarmFlow 确定性多阶段工作流

---

## 目录

1. [项目元信息与工程结构](#1-项目元信息与工程结构)
2. [四层架构总览](#2-四层架构总览)
3. [Leader-Teammate 模式](#3-leader-teammate-模式)
4. [A2A/ACP/E2A/A2UI 协议矩阵](#4-a2acp2aa2ui-协议矩阵)
5. [SkillDevPipeline 12 阶段](#5-skilldevpipeline-12-阶段)
6. [SwarmFlow DAG](#6-swarmflow-dag)
7. [AgentWarmPool 预热池](#7-agentwarmpool-预热池)
8. [Symphony 记忆子系统](#8-symphony-记忆子系统)
9. [JiuwenBox 沙箱](#9-jiuwenbox-沙箱)
10. [对 laew 的借鉴](#10-对-laew-的借鉴)

---

## 1. 项目元信息与工程结构

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

### 1.2 启动流程

`start_services.py` 是统一入口,支持多实例管理(`--name/--list/--status/--stop/--restart`)。核心流程:

1. 早期解析 `--dotenv`(`dotenv_early.parse_dotenv_early`)
2. 准备运行时工作区(`prepare_runtime_workspace`)
3. 加载 `.env` 与 `config.yaml`
4. 启动子进程:`jiuwenswarm.app`(AgentServer+Gateway 编排)+ `jiuwenswarm.channels.web.app_web`(前端)
5. 端口冲突自动回退(`_resolve_ports_with_fallback`)
6. 等待服务就绪(`_wait_for_services_ready`)

`app.py` 通过 `subprocess.Popen` 进一步分裂为两个子进程:
- `jiuwenswarm.server.app_agentserver`:Agent WebSocket 服务端
- `jiuwenswarm.gateway.app_gateway`:网关 HTTP/WebSocket 服务端

**设计要点**:
- AgentServer 与 Gateway **分进程部署**,支持单机/分布式两种拓扑
- 通过 `--dotenv <path>` 实现多实例隔离
- SIGTERM 统一走 `signal.default_int_handler` 保证子进程不孤儿化

### 1.3 模式矩阵

`common/mode_matrix.py` 定义了 **8 种 canonical 模式**(三段命名):

```
{agent|team}.{work|code}.{normal|plan}
```

- `agent.work.normal` — 单 agent 工作档
- `agent.code.plan` — 单 agent 代码规划
- `team.work.normal` — 集群工作档
- `team.code.plan` — 集群代码规划

Web 前端组合 `mode + work_mode`,TUI/CLI/IM 直接发送完整模式串。

### 1.4 多渠道接入

| 区域 | 渠道 | 路径 |
|------|------|------|
| 中国 | 小翼、飞书、钉钉、企微、个人微信 | `channels/web`,`gateway/channel_manager/im_platforms/` |
| 国际 | Telegram、Discord、Slack、WhatsApp | 同上 |
| 本地 | Web、TUI、CLI、Desktop、Browser | `channels/` |

`gateway/channel_manager/im_platforms/` 包含 10+ IM 平台适配器(飞书/钉钉/小翼/Telegram/Discord/Slack/WhatsApp 等)。

---

## 2. 四层架构总览

JiuwenSwarm 采用 **Channel → Gateway → Runtime → Server** 四层分离架构:

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Channel 层(接入层)                           │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐       │
│  │  Web    │ │  TUI    │ │ Feishu  │ │ DingTalk│ │  ACP    │ ...   │
│  └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘       │
└───────┼───────────┼───────────┼───────────┼───────────┼─────────────┘
        └───────────┴───────────┴─────┬─────┴───────────┘
                                      ▼
┌──────────────────────────────────────────────────────────────────────┐
│                         Gateway 层(网关层)                           │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                    app_gateway.py (155KB)                    │    │
│  │  ┌──────────────────┐  ┌──────────────────┐                 │    │
│  │  │  ChannelManager  │  │  MessageHandler  │                 │    │
│  │  │  (路由/注册)     │  │  (消息处理)      │                 │    │
│  │  └──────────────────┘  └──────────────────┘                 │    │
│  │  ┌──────────────────┐  ┌──────────────────┐                 │    │
│  │  │  Heartbeat       │  │  Cron Scheduler  │                 │    │
│  │  └──────────────────┘  └──────────────────┘                 │    │
│  └─────────────────────────────────────────────────────────────┘    │
└──────────────────────────────┬───────────────────────────────────────┘
                               │ E2A WebSocket
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│                         Runtime 层(运行时层)                         │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                    service.py (AgentRuntime)                 │    │
│  │  ┌────────────┐ ┌────────────┐ ┌────────────┐              │    │
│  │  │   plan.py  │ │ request.py │ │ context.py │              │    │
│  │  │(PlanMode   │ │(请求归一化)│ │(上下文Var) │              │    │
│  │  │ Controller)│ │            │ │            │              │    │
│  │  └────────────┘ └────────────┘ └────────────┘              │    │
│  └─────────────────────────────────────────────────────────────┘    │
└──────────────────────────────┬───────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│                         Server 层(服务层)                            │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │              agent_ws_server.py (484KB)                      │    │
│  │  ┌──────────────────────────────────────────────────────┐   │    │
│  │  │               agent_manager.py (66KB)                 │   │    │
│  │  │  ┌────────────┐ ┌────────────┐ ┌────────────┐       │   │    │
│  │  │  │ AgentWarm  │ │ Session    │ │ Team       │       │   │    │
│  │  │  │ Pool       │ │ Manager    │ │ Manager    │       │   │    │
│  │  │  └────────────┘ └────────────┘ └────────────┘       │   │    │
│  │  └──────────────────────────────────────────────────────┘   │    │
│  └─────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────┘
```

### 2.1 核心数据流

```
用户输入 → Channel → Gateway ChannelManager
         → MessageHandler.handle_message()
         → AgentServer WebSocket
         → AgentManager.get_or_create_agent()
         → DeepAgent.process_message_stream()
         → ReAct 循环 (LLM → tool_calls → 执行 → 回填)
         → 响应流回传 Channel
```

### 2.2 关键抽象

| 抽象 | 文件 | 职责 |
|------|------|------|
| `AgentRuntime` | `runtime/service.py`(47KB) | 进程级生命周期 owner |
| `AgentManager` | `server/runtime/agent_manager.py`(66KB) | Agent 实例缓存/热池/热重载 |
| `MessageHandler` | `gateway/message_handler/message_handler.py`(224KB) | 消息路由/会话/模式 |
| `ChannelManager` | `gateway/channel_manager/channel_manager.py`(36KB) | Channel 注册/派发 |
| `PlanModeController` | `runtime/plan.py`(12KB) | Plan 模式状态机 |
| `RuntimeSessionProvisioner` | `runtime/session_provisioner.py`(16KB) | Session 删除事务 |

### 2.3 设计哲学

JiuwenSwarm 的核心设计哲学是 **"纯声明式装配 + 跨序列化边界重建"**:

1. **声明式 Spec**:Agent 能力(rails/tools/subagents)全部声明为 `RailSpec`/`BuiltinToolSpec`/`SubAgentSpec`,由 `type` + `params` 描述
2. **属性/环境分离**:`params` 承载 config 派生的**属性值**(换请求不变),`SwarmBuildContext` 承载**环境值**(随请求/会话变化)
3. **跨边界重建**:通过 `build_context_seed` + `register_build_context_factory` 实现分布式 spawn/热恢复

---

## 3. Leader-Teammate 模式

### 3.1 SwarmBuildContext 生命周期

`SwarmBuildContext`(`agents/swarm/context.py`)是多 Agent 协作的核心上下文:

```python
@dataclass
class SwarmBuildContext(BuildContext):
    session_id: str = ""
    request_id: str | None = None
    user_id: str | None = None
    channel_id: str | None = None
    channel: str = "default"
    mode: str = "team"
    project_dir: str | None = None
    trusted_dirs: list[str] | None = None
    team_id: str = ""
    team_ws_root: str | None = None
    task_workspace_root: str | None = None
    team_outputs_dir: str | None = None
    team_skill_visibility_path: str | None = None
    global_skills_dir: str | None = None
    trajectory_span_processor: Any = None      # 进程级(不可序列化)
    heartbeat_job_service: Any = None           # 进程级(不可序列化)
    config: dict[str, Any] | None = None        # 进程级(不可序列化)
    skill_retrieval_toolkit: Any = None         # 进程级(不可序列化)

    def to_seed(self) -> dict[str, Any]:
        """只导出可序列化的 per-team/per-process 字段;
        trajectory_span_processor / heartbeat_job_service / config 不在 seed 中"""

    @classmethod
    def from_seed(cls, seed, *, config, trajectory_span_processor):
        """Rebuild a context from a to_dict 映射 + 本进程句柄"""
```

**关键设计**:
- `to_seed()` / `from_seed()` 实现跨进程序列化;非序列化句柄由接收端重新注入
- `derive()` 由 openjiuwen `setup_agent` 调用,生成 per-member 视图
- `resolve_member_skill_visibility_path` 必须用方法(依赖 per-member 数据)

### 3.2 Leader-Teammate 组装

`enrich_team_spec_for_swarm()`(`agents/swarm/assembly.py` L254-403)是 Team 装配入口:

```python
def enrich_team_spec_for_swarm(
    spec: Any,                                    # TeamAgentSpec (openjiuwen)
    *,
    session_id: str,
    mode: str,                                    # 来自 mode_matrix 的 ResolvedMode
    project_dir: str | None = None,
    trusted_dirs: list[str] | None = None,
    request_id: str | None = None,
    user_id: str | None = None,
    channel_id: str | None = None,
    request_metadata: dict[str, Any] | None = None,
    agent_group_name: str | None = None,
) -> None:
    """Enrich *spec* in place for provider-based swarm assembly."""
    register_swarm_providers()                    # 1) 注册所有 provider
    _ensure_external_team_transport(spec, channel_id)
    skills_library = get_agent_skills_dir()
    configure_global_skills_dir(skills_library)   # 2) 指向平台 Skill 目录

    config = get_config()
    workspace = spec.workspace
    team_ws_root = (
        workspace.root_path
        if workspace and workspace.root_path
        else str(team_home(spec.team_name) / "team-workspace")
    )
    # 3) 构造 SwarmBuildContext (环境载体)
    base = SwarmBuildContext(
        session_id=session_id, request_id=request_id, user_id=user_id,
        channel_id=channel_id, channel=channel_id or "default",
        request_metadata=request_metadata, mode=mode,
        project_dir=project_dir, trusted_dirs=trusted_dirs,
        disable_teammate_worktree=str(channel_id or "").strip().lower() == "web",
        team_id=spec.team_name, team_ws_root=team_ws_root,
        task_workspace_root=task_workspace_root,
        team_outputs_dir=team_outputs_dir,
        team_skill_visibility_path=team_visibility_path,
        global_skills_dir=global_skills_dir,
        trajectory_span_processor=get_trajectory_span_processor(),
        heartbeat_job_service=get_heartbeat_job_service(),
        config=config,
    )
    mcp_configs = build_enabled_mcp_server_configs(
        config, server_id_scope=f"team:{spec.team_name}", resolve_credentials=True,
    )
    # 4) 装配 leader / teammate 成员(各自的能力差异由 role 决定)
    for role in _MEMBER_ROLES:  # ("leader", "teammate")
        if role in spec.agents:
            member_spec = build_member_deep_agent_spec(
                config, mode, role, spec.agents[role],
                enable_permissions=spec.enable_permissions,
                mcp_configs=mcp_configs,
            )
            member_spec = _with_project_cwd(member_spec, project_dir)
            spec.agents[role] = member_spec

    if agent_group_name:
        _apply_agent_group(spec, agent_group_name)

    spec.build_context = base
    # 5) 跨边界序列化的 seed(关键!)
    spec.build_context_seed = base.to_seed()
```

**设计意图**:通过**「属性 vs 环境」**分离实现跨边界重建——`spec.agents` 携带 config 派生的属性值(换请求不变),`SwarmBuildContext` 携带环境值(随请求/会话变化)。`build_context_seed` 是 to_dict 序列化产物,可在子进程/分布式机器上通过 `from_seed` 重建。

### 3.3 TeamManager 编排主循环

`TeamManager`(`agents/harness/team/team_manager.py`, 123KB)是核心编排器:

```python
class TeamManager:
    """Manage team instances across sessions."""

    def __init__(self):
        self._team_agents: dict[str, TeamAgent] = {}
        self._runner_team_agents: dict[str, TeamAgent] = {}
        self._team_monitors: dict[str, TeamMonitorHandler] = {}
        self._stream_tasks: dict[str, asyncio.Task] = {}
        self._bootstrap_lock = asyncio.Lock()
        self._pending_waiters: dict[str, list[tuple[str, asyncio.Queue]]] = {}
        self._active_rounds: dict[str, _ActiveTeamRound] = {}

    async def broadcast_event(self, session_id, event):
        """带背压的事件广播;尊重 exclusive waiter。
        终端事件在广播后调用 finish_round,保证 admission 在终端事件之后再释放。"""

    def begin_round(self, session_id, request_id, *, release_admission=None,
                    defer_terminal_release=False, terminal_armed=False):
        """绑定唯一被准入的 round 到它的 request_id。
        每个 session 同一时间只允许一个 round;后续 request 撞上时会抛 RuntimeError。"""

    async def abort_round(self, session_id, request_id) -> bool:
        """取消自动化 round 之前停止其所有权。
        停止保留持久化 Team 状态;后续请求会冷恢复并收不到幽灵输出。"""
```

**Round 终态判定**(`common/cron_team_completion.py`):

```python
def apply_cron_team_round_event(state, event):
    """从 team 流事件更新 round 完成状态。
    - workflow.updated → workflow_started=True;若 status==completed → workflow_completed=True
    - team.task.created → tasks_ever_created=True;open_team_tasks[task_id]=True
    - team.task.completed → 从 open_team_tasks 移除
    - team.member.status_changed → active_team_members 跟踪 busy/working/starting ↔ ready/idle/stopped
    """

def cron_team_round_should_end(state, *, chunk_complete=False) -> bool:
    """判断 round 是否可结束。三档决策:
    1) workflow 已完成 + leader_final 已发出 → True
    2) team_round_completed + 有 result_text → True(但 leader_text 是占位符则 False)
    3) _harness_round_can_end:没起 workflow + 没 open tasks + 没 active members
       + leader_final_seen + 有 leader text 且非占位符 → True
    """
```

### 3.4 多 Agent 通信拓扑

```
                    ┌─────────────────┐
                    │   Leader Agent  │
                    │  (编排/拆解)    │
                    └────────┬────────┘
                             │ fan_out / mention
              ┌──────────────┼──────────────┐
              ▼              ▼              ▼
     ┌────────────┐  ┌────────────┐  ┌────────────┐
     │ Teammate 1 │  │ Teammate 2 │  │ Teammate N │
     │ (专业分工) │  │ (专业分工) │  │ (专业分工) │
     └────────────┘  └────────────┘  └────────────┘
              │              │              │
              └──────────────┼──────────────┘
                             ▼
                    ┌─────────────────┐
                    │  team_outputs/  │
                    │  (产物汇总)     │
                    └─────────────────┘
```

### 3.5 36 个 Harness 元素

Swarm 声明了 **36 个 harness 元素**(Tool 8 / Rail 18+3 / Sub-agent 1):

| 类别 | 代表元素 | 模式 |
|------|----------|------|
| Tool | `swarm.skill_toolkit`,`swarm.cron_tools`,`swarm.send_file` | T+K |
| Rail | `swarm.team_skill_evolution`,`swarm.context_processor` | T+K |
| Sub-agent | `swarm.code_agent` | K |

---

## 4. A2A/ACP/E2A/A2UI 协议矩阵

JiuwenSwarm 在传输层有 **4 种协议**:

| 协议 | 方向 | 传输层 | 序列化 | 用途 |
|------|------|--------|--------|------|
| **E2A** | Gateway ↔ AgentServer | WebSocket | JSON Envelope | 内部核心通信 |
| **ACP** | Client ↔ AgentServer | stdio / WebSocket | JSON-RPC-like | 外部 Agent 接入 |
| **A2A** | Agent ↔ Agent | 内部总线 | 结构化消息 | 多 Agent 协作 |
| **A2UI** | Agent → UI | 嵌入文本流 | Tagged JSON | 富交互输出 |

### 4.1 E2A 编解码

`common/e2a/wire_codec.py` 实现 E2A 线编解码:

```python
def is_e2a_response_wire_dict(data: dict) -> bool:
    """判别 JSON 是否为 E2A 响应线格式(须含非空 response_kind)"""

def parse_agent_server_wire_unary(data: dict) -> AgentResponse:
    """入站 WebSocket 非流式 JSON → AgentResponse。
    E2A 失败时回退到 metadata.legacy 字段,deprecation shape 也兼容。"""

def encode_agent_response_for_wire(resp, *, response_id, sequence=0) -> dict:
    """AgentResponse → E2A 线 dict;失败时 metadata 塞入整包 legacy。
    双层 fallback:to_dict 失败 → envelope 包 E2A error;整层 encode 失败 → legacy 包 fallback。"""
```

**legacy 兜底机制要点**:
- `_fallback_wire_unary_from_legacy` 把失败的 legacy 整包塞到 `metadata[E2A_WIRE_LEGACY_AGENT_RESPONSE_KEY]` 中
- 发一个 `status=FAILED`、`response_kind=E2A_RESPONSE_KIND_E2A_ERROR` 的 envelope,接收端再倒出来

### 4.2 ACP stdio client

`acp/stdio_client.py::AcpStdioClient`(~671 行)实现 ACP stdio 客户端:

```python
class AcpStdioClient:
    """Spawn an ACP-compatible agent and exchange JSON-RPC 2.0 over subprocess stdio."""

    async def _read_one_message(self, timeout: float) -> dict | None:
        """读取 1 个 JSON-RPC 消息。多行 pretty-printed JSON 由 json.JSONDecoder.raw_decode 处理"""

    async def _rpc_call(self, method, params, *, timeout=120.0):
        """JSON-RPC 同步调用:写到 stdin,等待带 id 的响应。
        同时处理 session/update(服务端推送)和客户端发出的请求"""

    async def _handle_peer_request(self, msg):
        """响应 agent 主动发的请求,避免阻塞 agent。
        - session/request_permission → 自动 allow-once(可配 _AUTO_APPROVE)
        - fs/read_text_file → 检查 path 必须落在 cwd 下(realpath 防 traversal)
        - fs/write_text_file → 同上,创建父目录
        """

    async def chat(self, message, *, timeout=None) -> str:
        """session/prompt 客户端;累积 session/update 文本,直到拿到带 id 的响应。"""

    async def close(self):
        """分级关闭:close stdin → SIGTERM 5s → SIGKILL 8s → 排空管道。
        POSIX 上 killpg(_KILL_PG 默认 True)杀整个进程组,防止子进程孤立。"""
```

### 4.3 A2UI Tagged JSON

`server/runtime/a2ui/protocol.py` 实现 A2UI v0.8 适配:

```python
A2UI_ACTIVE_PROTOCOL_VERSION = VERSION_0_8

class A2UIProtocolSpec:
    """Versioned A2UI protocol adapter. 注册表目前只有 v0.8"""

    def build_prompt(self, language="en", *, include_browser_workflows=False) -> str:
        """生成系统提示(包含 schema);基于语言分流。
        - 每个 A2UI block 必须用 OPEN_TAG/CLOSE_TAG 包装
        - block 内必须是 JSON list,每个元素是 server-to-client message
        - 必须先 beginRendering,再 surfaceUpdate
        - 数据绑定按需 dataModelUpdate
        - 父组件先于子组件
        """

    @staticmethod
    def parse_response(content: str) -> list[A2UIResponsePart]:
        return parse_a2ui_response(content)
```

**Tagged JSON 解析器**(`server/runtime/a2ui/parser.py`):

```python
def iter_tagged_block_bodies(text: str) -> list[tuple[int, str]]:
    """扫描 OPEN_TAG..CLOSE_TAG 包裹的 body,带 pair index"""

def strip_tagged_a2ui_blocks(text: str) -> str:
    """剥掉所有 tagged blocks,只留纯文本。用于文本渠道 fallback"""

def parse_a2ui_response(content: str) -> list[A2UIResponsePart]:
    """主入口:JSONL → raw JSON → tagged 三级 fallback。
    最终落到:若三种都不是,返回单 text part。"""
```

### 4.4 协议转换层

`common/e2a/gateway_normalize.py`(23KB)负责 Gateway 侧归一化:
- `e2a_from_agent_fields()`:从 Agent 字段构造 E2A 消息
- `_normalize_gateway_message()`:归一化入站消息
- `_inject_session_work_mode()`:注入 work_mode 归一化

---

## 5. SkillDevPipeline 12 阶段

`server/runtime/skill/skilldev/pipeline.py` 实现 12 阶段 SkillDev 流水线:

### 5.1 完整阶段图

```python
class SkillDevStage(str, Enum):
    INIT = "init"
    PLAN = "plan"
    PLAN_CONFIRM = "plan_confirm"            # 挂起点 1
    GENERATE = "generate"
    VALIDATE = "validate"                    # 校验 SKILL.md 格式
    TEST_DESIGN = "test_design"
    TEST_RUN = "test_run"
    EVALUATE = "evaluate"                    # grader + benchmark + analyst
    REVIEW = "review"                        # 挂起点 2
    IMPROVE = "improve"
    PACKAGE = "package"
    DESC_OPTIMIZE_CONFIRM = "desc_optimize_confirm"  # 挂起点 3
    DESC_OPTIMIZE = "desc_optimize"
    COMPLETED = "completed"
    ERROR = "error"

# 转移顺序:
# INIT → PLAN → PLAN_CONFIRM(*) → GENERATE → VALIDATE
#     → TEST_DESIGN → TEST_RUN → EVALUATE → REVIEW(*)
#     → IMPROVE → (回到 TEST_RUN 循环)
#     → PACKAGE → DESC_OPTIMIZE_CONFIRM(*) → DESC_OPTIMIZE → COMPLETED
```

### 5.2 状态机核心

```python
class SkillDevPipeline:
    """SkillDev 确定性状态机。
    生命周期:每次请求创建 → run()/resume() 执行 → checkpoint → 对象释放。
    不长驻内存,不持有 JiuWenSwarm 实例。
    """

    STAGE_HANDLERS = {
        SkillDevStage.INIT: InitStageHandler,
        SkillDevStage.PLAN: PlanStageHandler,
        SkillDevStage.GENERATE: GenerateStageHandler,
        SkillDevStage.VALIDATE: ValidateStageHandler,
        SkillDevStage.TEST_DESIGN: TestDesignStageHandler,
        SkillDevStage.TEST_RUN: TestRunStageHandler,
        SkillDevStage.EVALUATE: EvaluateStageHandler,
        SkillDevStage.IMPROVE: ImproveStageHandler,
        SkillDevStage.PACKAGE: PackageStageHandler,
        SkillDevStage.DESC_OPTIMIZE: DescOptimizeStageHandler,
    }

    async def run(self) -> AsyncIterator[SkillDevEvent]:
        """从当前阶段开始执行,直到遇到挂起点或终态。
        核心循环:
        1) 若当前 stage 在 SUSPENSION_POINTS,emit CONFIRM_REQUEST → checkpoint → break
        2) 找到 handler,execute,转 next_stage
        3) 每个阶段 emit STAGE_CHANGED + TODOS_UPDATE
        4) 阶段边界调 _checkpoint (持久化 + 同步 workspace)
        """

    async def resume(self, data: dict) -> AsyncIterator[SkillDevEvent]:
        """从挂起点恢复。
        REVIEW 的 next_stage 是函数,根据 action 动态决定(improve vs package);
        PLAN_CONFIRM/DESC_OPTIMIZE_CONFIRM 是固定 stage。"""

    async def _checkpoint(self):
        """阶段边界:持久化状态 + 同步 workspace 文件。"""
```

### 5.3 三个 HITL 挂起点

```python
@dataclass
class SuspensionConfig:
    """挂起点的声明式配置。
    Pipeline 到达挂起点时:
    1. 推送 CONFIRM_REQUEST 事件(前端据此弹出确认框)
    2. Checkpoint 当前状态并暂停
    恢复时(前端通过 skilldev.respond 统一入口):
    1. 调用 on_resume 更新状态
    2. 跳转到 next_stage
    """
    confirm_type: str     # 标识确认类型(前端用于区分弹框样式)
    title: str
    message: str
    actions: list[dict[str, str]]
    extract_data: Callable    # (state) → dict,从 state 提取展示给前端的数据
    on_resume: Callable       # (state, data) → None,根据用户响应更新 state
    next_stage: SkillDevStage | Callable  # 下一阶段(可以是函数)

SUSPENSION_POINTS: dict[SkillDevStage, SuspensionConfig] = {
    SkillDevStage.PLAN_CONFIRM: SuspensionConfig(
        confirm_type="plan_confirm",
        title="请审阅开发计划",
        message="以下是生成的开发计划,请确认或修改",
        actions=[{"id": "confirm", "label": "确认", "style": "primary"},
                 {"id": "modify", "label": "修改", "style": "secondary"}],
        extract_data=_plan_extract_data,
        on_resume=_plan_confirm_on_resume,
        next_stage=SkillDevStage.GENERATE,
    ),
    SkillDevStage.REVIEW: SuspensionConfig(
        confirm_type="review",
        title="评测结果审阅",
        message="请审阅评测结果并决定下一步",
        actions=[{"id": "accept", "label": "通过,进入打包", "style": "primary"},
                 {"id": "improve", "label": "继续改进", "style": "secondary"}],
        extract_data=_review_extract_data,
        on_resume=_review_on_resume,
        next_stage=_review_next_stage,  # 动态决定 IMPROVE 或 PACKAGE
    ),
    SkillDevStage.DESC_OPTIMIZE_CONFIRM: SuspensionConfig(
        confirm_type="desc_optimize_confirm",
        title="描述优化",
        message="Skill 已打包完成。是否需要优化触发描述以提高触发准确率?",
        actions=[{"id": "optimize", "label": "优化", "style": "primary"},
                 {"id": "skip", "label": "跳过", "style": "secondary"}],
        extract_data=_desc_opt_extract_data,
        on_resume=_desc_optimize_confirm_on_resume,
        next_stage=_desc_optimize_confirm_next_stage,
    ),
}
```

### 5.4 7 个 RPC 接口

- `skilldev.start` — 发起任务
- `skilldev.respond` — 统一确认入口
- `skilldev.status` — 查询状态
- `skilldev.download` — 下载产物
- `skilldev.cancel` — 取消
- `skilldev.file.list` — 文件树
- `skilldev.file.read` — 读取文件

### 5.5 Skill 自进化

`symphony/evolution/service.py` 实现能力自进化:

```python
def load_dynamic_overlay(graph_dir) -> dict | None:
    """加载当前动态 overlay;若 event/overlay 元数据不一致则重建。"""

def _overlay_requires_rebuild(overlay, events, current_version) -> bool:
    """overlay 元数据与 events 不一致时返回 True:
    - 没有 events → 不需要重建
    - overlay 不存在或 schema 不匹配 → 重建
    - overlay.events.count != len(events) → 重建
    - overlay.base_graph_version != current_version → 重建
    """

def record_plan_outcome(graph_dir, *, plan_id, outcome,
                        selected_skill_ids=None, selected_edges=None,
                        failed_edges=None, missing_inputs=None,
                        failure_attribution="", failure_type="", detail="",
                        evidence_id="", session_id="", request_id="",
                        rebuild_overlay=True) -> dict:
    """追加一个 plan outcome event,可选刷新 overlay。
    按 evidence_id 去重(同一 plan 多次失败不重复计数)。"""
```

---

## 6. SwarmFlow DAG

SwarmFlow 是 JiuwenSwarm 的确定性多阶段工作流系统,特点:

- **DAG 构建**:Python 脚本定义阶段依赖
- **HITL 支持**:`human` / `human_session` 挂起点
- **Team Token 预算**:控制整体消耗
- **TUI 运行树监控**:`/swarmflows` 命令查看

与 SkillDevPipeline 的区别:
- SkillDevPipeline 是**线性为主、局部迭代**的确定性状态机
- SwarmFlow 是**DAG 拓扑**的复杂工作流,支持并行分支

### 6.1 Workflow 状态机

`agents/harness/team/handlers/workflow_state.py::WorkflowRunState`:

```python
def apply(self, progress: WorkflowProgress) -> Optional[dict[str, Any]]:
    """应用一个 progress event,更新状态,返回增量 dict。
    返回 None 表示无需 push(log、未知 kind 等)。"""
    kind = progress.kind
    handler = self._KIND_HANDLERS.get(kind)
    if handler is None:
        return None
    method = getattr(self, handler)
    return method(progress)

_KIND_HANDLERS: dict[str, str] = {
    "workflow_started": "_on_workflow_started",
    "phase": "_on_phase",
    "agent_started": "_on_agent_started",
    "agent_completed": "_on_agent_completed",
    "agent_failed": "_on_agent_failed",
    "human_prompt": "_on_human_prompt",
    "human_replied": "_on_human_replied",
    "workflow_completed": "_on_workflow_completed",
    "workflow_failed": "_on_workflow_failed",
    "workflow_paused": "_on_workflow_paused",
    "workflow_stopped": "_on_workflow_stopped",
    "log": "_on_log",
}
_TERMINAL_STATUSES: ClassVar[frozenset[str]] = frozenset({"completed", "failed", "stopped"})
```

### 6.2 阶段切换 DAG

```python
def _switch_to_phase(self, phase_name, iteration=None):
    """进入 phase_name(running),sealing 之前的 phase。
    Loop-aware:iteration 设置时,每轮迭代一张新卡(round 1/2/3 of phase("生成") → 3 张)。
    """
    # 状态镜像 agents 优先级:
    # any stopped → stopped(external termination wins)
    # all failed → failed(每个 agent 都错误)
    # otherwise → completed(至少一个 completed,partial OK)
```

### 6.3 HITL 节点暂停/恢复

```python
def _pause_running_agents(self, phase):
    """Pause 还在跑的 agents(包括 waiting_for_human)到 paused(非终态)。
    与 _finalize_running_agents 不同:不 stamp completed_at / duration_ms —
    paused agent 没完成,resume 时会重新激活。"""

def _finalize_running_agents(self, phase, terminal_status):
    """把还在跑的 agents(和 waiting_for_human)敲到 terminal_status。
    waiting_for_human 在 teardown(run 被拆而没有 workflow_completed 事件)
    时也要关闭 — 否则前端会一直等待一个永远不到的回复。"""
```

### 6.4 Team Token 预算

```python
def _bump_completion(self, phase, progress, agent):
    """Refresh phase counters, write tokens / budget, refresh run totals。"""
    if progress.tokens is not None:
        agent.token_count = progress.tokens
    if progress.budget is not None:
        self.budget = progress.budget                 # session 范围 ledger
    if progress.workflow_budget is not None:
        self.workflow_budget = progress.workflow_budget  # workflow 范围 ledger
    self.token_count = sum(a.token_count or 0 for ph in self.phases for a in ph.agents)
```

---

## 7. AgentWarmPool 预热池

`server/runtime/agent_warm_pool.py::AgentWarmPool`(30KB)实现 Agent 预热池。

### 7.1 WarmKey 多维匹配

```python
@dataclass(frozen=True, slots=True)
class WarmKey:
    channel_id: str
    project_id: str
    project_dir: str
    work_mode: str
    is_swarm: bool = False

    @property
    def agent_mode(self) -> str:
        """work_mode → agent mode:code or agent。"""
        return "code" if self.work_mode == "code" else "agent"

    @property
    def agent_sub_mode(self) -> str | None:
        return "normal" if self.work_mode == "code" else None

@dataclass(frozen=True, slots=True)
class WarmRevision:
    boot_id: str             # 进程级 UUID,标识当前 AgentServer boot
    config_fingerprint: str  # config 哈希
    sequence: int            # 单调递增

@dataclass(slots=True)
class WarmSlot:
    key: WarmKey
    session_id: str
    revision: WarmRevision
    agent: "JiuWenSwarm"
    ready_at: float

# 多维匹配优先级
@staticmethod
def _key_priority(key: WarmKey) -> tuple[int, int, int, str, str]:
    """优先让 web/work/DEFAULT_PROJECT_ID_WORK 在初始 global READY slot。"""
    return (
        0 if key.channel_id == "web" else 1,
        0 if key.work_mode == "work" else 1,
        0 if key.project_id == DEFAULT_PROJECT_ID_WORK else 1,
        key.channel_id,
        key.project_id,
    )
```

### 7.2 配置指纹

```python
@staticmethod
def config_fingerprint(config: Any, env: Any = None) -> str:
    """把 config + env 序列化(排序 + ascii)做 sha256,作为变更检测。"""
    payload = json.dumps(
        {"config": config, "env": env if isinstance(env, dict) else {}},
        sort_keys=True, ensure_ascii=False, default=repr,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()
```

### 7.3 预热和回收

```python
EXCLUDED_CHANNELS = frozenset({"acp", "a2a"})

async def sync(self, enabled_channels, *, config, env=None) -> dict:
    """sync enabled_channels with the prewarm pool.
    关键逻辑:
    1) 计算 desired = {(channel × project × work_mode) all combinations}
    2) revision: 基于 (boot_id, config_fingerprint, sequence)
    3) 把 stale slots/config-fingerprint 不同的 tasks 取消
    4) 为 desired 中未在 tasks / pending / slots 中的 key 入队
    5) pump background(只在 foreground_count==0 时)
    """

async def begin_foreground(self):
    """真用户聊天开始时,取消所有 speculative 预热任务,防止与前台
    DeepAgent 共享 OpenJiuwen registry 时撞车。"""

async def end_foreground(self):
    """最后一波聊天结束后,恢复 background 预热(留 cooldown 间隔)。"""

async def claim(self, key: WarmKey) -> WarmClaim:
    """从 pool 领取一个 WarmSlot 或前台临建。
    优先选 _slots 已 ready 的;否则把 _tasks 中的 speculative 实例 promote
    (避免与前台构建第二个 DeepAgent 撞 registry)。"""
```

**预热策略**:
- 默认关闭,需环境变量 `JIUWENSWARM_AGENT_PREWARM=1` 开启
- 基于 `WarmKey`(channel_id + project_id + project_dir + work_mode + is_swarm)匹配槽位
- `WarmRevision`(boot_id + config_fingerprint + sequence)检测配置变更
- 信号量控制并发(`_semaphore` / `_foreground_semaphore`)
- ACP/A2A 通道排除预热

---

## 8. Symphony 记忆子系统

Symphony 是 JiuwenSwarm 的记忆/检索/索引/进化核心:

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Symphony 子系统                              │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                   service.py (43KB)                           │   │
│  │   SwarmSymphonyService：process-local runtime owner           │   │
│  └──────────────────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                   build.py (29KB)                             │   │
│  │   SymphonyGraphBuilder：离线 graph 构建                       │   │
│  └──────────────────────────────────────────────────────────────┘   │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐       │
│  │ indexing/  │ │ retrieval/ │ │ evolution/ │ │ skill_     │       │
│  │ (索引构建) │ │ (渐进检索) │ │ (自进化)   │ │ retrieval/ │       │
│  └────────────┘ └────────────┘ └────────────┘ └────────────┘       │
└──────────────────────────────────────────────────────────────────────┘
```

### 8.1 离线 Graph 构建

`symphony/build.py::SymphonyGraphBuilder`:

```python
class SymphonyGraphBuilder:
    """Build and refresh the offline Symphony graph."""

    def status(self, skills_root, graph_dir, *, llm_config=None, symphony_config=None) -> GraphStatus:
        """报告 graph 是否存在且是否与 skill 文件夹同步。
        步骤:
        1) 扫描 skills_root
        2) 用 state_builder 算 capability_hashes
        3) load_graph_state → active_entries
        4) 计算 added/changed/removed
        5) stale = (not exists) or (added/changed/removed 非空)
        6) 若 exists && !stale → 校验 SymphonyRuntime orchestration status (fresh?)
        """

    async def build(self, skills_root, graph_dir, llm_config=None, *,
                    force=False, build_log=None, symphony_config=None,
                    resume=True) -> GraphBuildResult:
        """构建或刷新 Symphony graph。
        关键步骤:
        1) reset_llm_token_usage
        2) new run_id, _BuildCheckpoint
        3) resume_from = latest_incomplete_build(graph_dir) if resume else None
        4) scan + compute hashes + diff (reused/extracted/removed)
        5) fingerprint_service.build(force, progress_callback)
        6) 写 fingerprint artifact
        7) graph build: SymphonyRuntime.orchestration.build(force, progress=...)
        8) prepare_artifact callback 在 version_dir 完成时把 fingerprint 拷贝过去
        9) write_graph_state
        10) 清理 _cleanup_published_build_artifacts
        """
```

**GraphStatus 结构**:

```python
@dataclass(frozen=True)
class GraphStatus:
    success: bool
    graph_dir: str
    exists: bool
    stale: bool
    skill_count: int
    changed_count: int
    added_count: int
    removed_count: int
    resume_available: bool = False
    checkpoint_dir: str = ""
```

### 8.2 渐进式检索

`symphony/retrieval/algorithm.md` 定义检索算法:

```
输入:query, tree root, top_k
流程:
1. apply request tag filters to catalog leaves and prune empty tree branches
2. start from the resulting visible subtree
3. if the structure is trivial, use deterministic shortcuts
4. otherwise ask the LLM to choose among the current visible boundary nodes
5. recurse into selected branches or terminate on selected items
6. reduce branch results to the requested top_k

关键规则:
- LLM 路由决策只看到当前可见子树
- 模型输出可见边界节点的 display names
- display names 自动去重
- 单一候选不调用 LLM
```

### 8.3 进化子系统

`symphony/evolution/` 实现能力自进化:

```python
# evolution/session_consumer.py (46KB)
# 消费执行会话数据,驱动能力指纹更新

# evolution/service.py (11KB)
def load_dynamic_overlay(graph_dir) -> dict | None:
    """加载当前动态覆盖层;启用后影响 orchestration planning"""
```

### 8.4 记忆分层

| 层级 | 实现 | 用途 |
|------|------|------|
| 任务记忆 | `session_history.py` | 单次任务上下文 |
| 编码记忆 | `code_coding_memory` rail | 项目级代码记忆 |
| 项目记忆 | `code_project_memory` rail | 项目文档/结构 |
| 个人记忆 | `personal_context/` | 用户级持久记忆 |
| 团队记忆 | `team_workspace/` | 团队共享记忆 |

---

## 9. JiuwenBox 沙箱

`jiuwenbox/` 是独立沙箱子包,`server/sandbox/jiuwenbox_runner.py::JiuwenBoxRunner` 管理子进程。

### 9.1 启动和执行

```python
_PR_SET_PDEATHSIG = 1  # 来自 <linux/prctl.h>

class JiuwenBoxRunner:
    """单例形态管理本地 jiuwenbox uvicorn 子进程。"""

    _INSTANCE: "JiuwenBoxRunner | None" = None
    _STDERR_TAIL_MAX: int = 80  # stderr 滚动缓冲 80 行

    async def ensure_running(self, host="127.0.0.1", port=8321, *,
                              timeout=30.0, startup_mode="internal",
                              policy_path=None) -> bool:
        """确保 jiuwenbox 在 host:port 就绪。
        internal 模式:AgentServer 自己 spawn 子进程并管理生命周期
        external 模式:仅做健康检查,不 spawn / 不 kill
        决策矩阵(internal):
        - 我们持有的进程 alive + host/port/policy_path 全匹配 → 复用
        - 否则:停掉旧进程 + 在新 host:port spawn 新实例
        policy_path 变更也会触发重启,以保证 JIUWENBOX_POLICY_PATH 生效。
        """
```

### 9.2 pdeathsig 子进程守护

```python
def _try_set_pdeathsig() -> None:
    """Linux:让子进程在父进程退出时收到 SIGTERM,避免 SIGKILL 父进程时 jiuwenbox 残留。
    通过 preexec_fn 调用;非 Linux 是 no-op。

    关键 Linux 调用链:
    libc.prctl(_PR_SET_PDEATHSIG, signal.SIGTERM, 0, 0, 0)
    - _PR_SET_PDEATHSIG = 1
    - 把子进程的"父进程退出时"信号设为 SIGTERM
    - 在 preexec_fn 里调用是因为此时子进程刚 fork 完,还没 exec,
      是唯一安全的时机
    """
    if not sys.platform.startswith("linux"):
        return
    try:
        import ctypes
        libc = ctypes.CDLL("libc.so.6", use_errno=True)
        libc.prctl(_PR_SET_PDEATHSIG, signal.SIGTERM, 0, 0, 0)
    except Exception:
        pass
```

### 9.3 优雅停止

```python
def _sync_terminate(self):
    """atexit / 异常退出场景:不依赖事件循环。
    - 若 stop() 已正常清理,则什么都不做
    - 否则尽可能 terminate / kill 子进程,避免 jiuwenbox 残留
    """

async def stop(self) -> None:
    """优雅停止:proc.terminate() → 等待 60s → proc.kill()。
    uvicorn 收到 SIGTERM 后跑 FastAPI lifespan shutdown,期间调
    SandboxManager.shutdown_all_sandboxes 给每个活的 sandbox 做
    SIGTERM -> wait -> SIGKILL 三段式 teardown(每个最坏要 ~15s)。
    所以给一个相对宽松的 60s 上限。"""
```

### 9.4 策略注入

```python
# 把 policy 路径通过环境变量传给 jiuwenbox-server
if policy_path is not None:
    env["JIUWENBOX_POLICY_PATH"] = str(policy_path)
else:
    env.pop("JIUWENBOX_POLICY_PATH", None)  # 避免误用旧值
```

### 9.5 沙箱隔离层次

```
┌─────────────────────────────────────────────────┐
│              AgentServer 进程                    │
│  ┌───────────────────────────────────────────┐  │
│  │           JiuwenBoxRunner                 │  │
│  │  ┌─────────────────────────────────────┐  │  │
│  │  │        jiuwenbox uvicorn 子进程      │  │  │
│  │  │   (HTTP API @ 127.0.0.1:8321)       │  │  │
│  │  │   ┌─────────────────────────────┐   │  │  │
│  │  │   │     Policy Engine           │   │  │  │
│  │  │   │   (文件/网络/能力限制)      │   │  │  │
│  │  │   └─────────────────────────────┘   │  │  │
│  │  └─────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

---

## 10. 对 laew 的借鉴

### 10.1 laew 现状速览

laew 当前是**双 Agent 架构**(Yolo + Work),单进程 Rust CLI,SQLite 持久化,零沙箱零校验。JiuwenSwarm 的许多设计可直接映射到 laew 路线图。

### 10.2 P0(立刻借鉴,高 ROI)

| 能力 | JiuwenSwarm 实现 | laew 借鉴方案 | 依据文件 |
|------|------------------|---------------|----------|
| **Plan 模式状态机** | `PlanModeController` + `inject_activation_reminder()` | 实现 `PlanModeController`,Yolo 分类 hard 任务时自动注入 Plan reminder 到系统提示词 | `runtime/plan.py` L151-176 |
| **SwarmBuildContext 模式** | `SwarmBuildContext` + `to_seed/from_seed` | 把硬编码 `work_profile()` / `yolo_profile()` 工厂函数改为声明式 `AgentSpec::from_config()` | `agents/swarm/context.py` |
| **SuspensionConfig 三段式** | `extract_data + on_resume + next_stage` | Plan Agent 产出方案后与用户交互,emit `<<<LAEW:PLAN_CONFIRM>>>` 标记 | `skilldev/schema.py` |
| **PR_SET_PDEATHSIG** | `_try_set_pdeathsig` | Bash 工具的子进程守护,主进程 SIGKILL 后不残留 | `jiuwenbox_runner.py` L54 |
| **流式 ContextVar 传播** | `set_runtime_context()` / `reset_runtime_context()` | 用 `tokio::sync::watch` 或 `task_local!` 实现跨 yield 点的 Runtime 上下文 | `runtime/service.py` L661-693 |
| **请求归一化层** | `resolve_request_runtime_mode()` | 实现 `resolve_task_level()` 将 Yolo 输出的 simple/medium/hard 映射到 canonical mode | `runtime/request.py` L223-282 |

**Plan 模式详细设计**:

```rust
// laew 借鉴实现草案
pub struct PlanModeController {
    exited_sessions: HashSet<String>,
    active_sessions: HashSet<String>,
}

impl PlanModeController {
    /// 在 Yolo 判定 hard 任务后,注入 Plan 约束到系统提示词
    pub fn inject_plan_reminder(&self, system_prompt: &mut String) {
        let reminder = "\n\n<system-reminder>\n\
            Plan mode is active. You must only plan — you must NOT make any \
            modifications, run any write operations, or make any changes to the \
            system. This constraint takes priority over any other instructions.\n\
            </system-reminder>";
        system_prompt.push_str(reminder);
    }
}
```

### 10.3 P1(下一阶段,中等 ROI)

| 能力 | JiuwenSwarm 实现 | laew 借鉴方案 | 依据文件 |
|------|------------------|---------------|----------|
| **HITL 挂起机制** | `SUSPENSION_POINTS` + `resume()` | SubAgent 执行到关键节点(写操作前)暂停,用户确认后继续 | `skilldev/pipeline.py` L80-101 |
| **SkillDev 状态机** | `SkillDevPipeline` + `STAGE_HANDLERS` | 实现 laew 的 Skill 开发流水线(至少 INIT→PLAN→GENERATE→VALIDATE→PACKAGE) | `skilldev/pipeline.py` L55-66 |
| **Agent 预热池** | `AgentWarmPool` + `WarmKey` | 预编译常用 AgentProfile,减少首次请求延迟 | `agent_warm_pool.py` L115-165 |
| **TeamManager active_rounds** | `_active_rounds` round 准入 | 避免 TUI/CLI/Web 多端 round 打架 | `team_manager.py` |
| **Wire Codec legacy 兜底** | `metadata.legacy_key` 整包回退 | 协议升级向前兼容 | `wire_codec.py` |
| **记忆图构建** | `SymphonyGraphBuilder` + `FingerprintService` | 用 SQLite 实现轻量级 Skill/Tool 关系图,支持检索注入 | `symphony/build.py` L156-260 |

**HITL 挂起详细设计**:

```rust
// laew 借鉴实现草案
pub enum SuspensionPoint {
    BeforeWrite,      // 写操作前
    BeforeBash,       // Bash 执行前
    QualityCheckFail, // QC 失败时
}

pub struct HitlController {
    suspended: HashMap<SessionId, SuspensionPoint>,
}

impl HitlController {
    /// 遇到挂起点时,推送确认请求并暂停
    pub async fn suspend(&mut self, session_id: &str, point: SuspensionPoint) -> SkillDevEvent {
        self.suspended.insert(session_id.to_string(), point);
        SkillDevEvent::ConfirmRequest {
            confirm_type: point.to_string(),
            title: point.title(),
            message: point.message(),
        }
    }

    /// 用户确认后恢复
    pub async fn resume(&mut self, session_id: &str, data: serde_json::Value) -> Result<()> {
        self.suspended.remove(session_id);
        Ok(())
    }
}
```

### 10.4 P2(长期规划,战略价值)

| 能力 | JiuwenSwarm 实现 | laew 借鉴方案 | 依据文件 |
|------|------------------|---------------|----------|
| **多协议适配层** | E2A/ACP/A2A/A2UI | laew 已实现 Anthropic/OpenAI 双协议,可考虑 A2UI 风格的富输出 | `server/runtime/a2ui/protocol.py` |
| **SwarmFlow DAG** | Python 脚本定义 DAG | 实现 Rust 侧的 WorkFlow DSL,支持 HITL 挂起 | `agents/swarm/assembly.py` |
| **ACP JSON-RPC over stdio** | `AcpStdioClient` | 未来接 Skill Marketplace / 第三方 agent 时直接用 | `acp/stdio_client.py` |
| **A2UI Tagged JSON** | `A2UIProtocolSpec` | TUI 引入富交互(按钮/确认),三级 fallback 解析 | `a2ui/parser.py` |
| **沙箱隔离** | JiuwenBox + Policy Engine | laew 零沙箱现状需重点突破:基于 Landlock/seccomp 的 Bash 沙箱 | `server/sandbox/jiuwenbox_runner.py` |
| **能力指纹进化** | `FingerprintService` + evolution | laew 的工具/Agent 定义可引入 fingerprint 机制,自动检测过时定义 | `symphony/evolution/` |
| **Skill Evolution overlay** | `record_plan_outcome` + evidence_id 去重 | AgentMemory 加去重,重试风暴不污染统计 | `evolution/service.py` |
| **Progressive tree retrieval** | tag/keyword/semantic 三路召回 | laew 的 Skill 调度能力 | `retrieval/algorithm.md` |

### 10.5 P3(远期)

| 能力 | JiuwenSwarm 实现 | laew 借鉴方案 |
|------|------------------|---------------|
| **Workflow budget ledger** | session/workflow 双 ledger | 引入 budget 控制 |
| **Foreground preempt** | 后台预热任务被前台抢占 | 共享 registry 不撞车 |
| **Stale marker cleanup** | boot_id 比较 + 重启清理 | 重启一致性 |
| **Placeholder text detection** | leader final 占位符识别 | 避免 cron 误判完成 |
| **来源快照 source_snapshot** | config fingerprint + model identity | 自动失效缓存 |

### 10.6 跨项目模式提炼

从 JiuwenSwarm 提炼的 **10 个可复用设计模式**:

| # | 模式 | 描述 | 适用场景 |
|---|------|------|----------|
| 1 | **进程分裂 + 单例依赖** | 多进程拓扑 + 引用计数单例 | laew 未来分布式部署 |
| 2 | **ContextVar 跨 yield 传播** | 每 yield 重置 token,子任务正常继承 | laew 的流式 tool_call |
| 3 | **HITL Checkpoint** | 挂起点持久化 + resume() 恢复 | laew 的 SubAgent 关键节点确认 |
| 4 | **Plan 状态并轨** | plan/normal 同一实例,仅换工具可见性 | laew 的 Plan Agent 与 Main-Work Agent 统一 |
| 5 | **E2A Envelope + Legacy Fallback** | 新版本 envelope + metadata legacy blob | laew 的协议演进 |
| 6 | **渐进式树检索** | LLM 路由决策仅见可见子树 | laew 的 Skill/Tool 检索注入 |
| 7 | **附件大小分层** | 阈值下 base64,阈值上 HTTP bridge | laew 的文件工具 |
| 8 | **Config Fingerprint 失效** | SHA256(config) 变更触发重建 | laew 的 AgentProfile 热更新 |
| 9 | **pdeathsig 子进程守护** | Linux prctl 防止孤儿 | laew 的 Bash 子进程管理 |
| 10 | **WarmKey 多维匹配** | channel × project × mode 槽位 | laew 的 AgentProfile 预热 |

### 10.7 反模式警示

从 JiuwenSwarm 识别的 **5 个反模式**(laew 应避免):

| # | 反模式 | JiuwenSwarm 表现 | laew 规避策略 |
|---|--------|------------------|---------------|
| 1 | **单文件膨胀** | `agent_ws_server.py` 484KB | 严格模块边界,单文件 >5K 行即拆分 |
| 2 | **过度抽象** | `ABC` + 多层 `dataclass` 嵌套 | Rust enum + struct 足够时不引入 trait |
| 3 | **配置蔓延** | `config.yaml` + `.env` + `state.json` 三源 | 坚持 SQLite 单源,不引入新配置文件 |
| 4 | **循环依赖** | `gateway` ↔ `server` 双向 import | 严格分层,上层可引用下层,反之禁止 |
| 5 | **Optional 泛滥** | 大量 `if xxx is not None` 守卫 | Rust `Option` + `?` 运算符 + enum 状态机 |

### 10.8 综合路线图

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         laew 演进路线图                                  │
├─────────────────────────────────────────────────────────────────────────┤
│  P0 (立刻)                                                              │
│  ├── PlanModeController:Plan reminder 注入                              │
│  ├── SwarmBuildContext:声明式 Spec + 跨边界 seed                        │
│  ├── SuspensionConfig 三段式:Plan 确认流程                              │
│  ├── PR_SET_PDEATHSIG:Bash 子进程守护                                   │
│  ├── Runtime ContextVar:跨 yield 上下文传播                             │
│  └── TaskLevel → CanonicalMode 映射                                     │
├─────────────────────────────────────────────────────────────────────────┤
│  P1 (下阶段)                                                            │
│  ├── HITL 挂起/恢复机制(SubAgent 写操作前确认)                          │
│  ├── SkillDev 状态机(INIT→PLAN→GEN→VALID→PACKAGE)                       │
│  ├── AgentProfile 预热池(减少首次延迟)                                  │
│  ├── TeamManager active_rounds(多端一致)                                │
│  ├── Wire Codec legacy 兜底                                             │
│  └── 轻量级记忆图(SQLite 存储 Tool/Skill 关系)                          │
├─────────────────────────────────────────────────────────────────────────┤
│  P2 (长期)                                                              │
│  ├── A2UI 风格富输出(可选组件协议)                                      │
│  ├── SwarmFlow DAG DSL                                                 │
│  ├── ACP JSON-RPC over stdio                                           │
│  ├── 多 Agent 上下文序列化(to_seed/from_seed)                           │
│  ├── Landlock/seccomp 沙箱                                             │
│  └── 能力指纹进化(过时定义自动检测)                                     │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 附录 A: 关键文件索引

| 文件 | 规模 | 核心职责 |
|------|------|----------|
| `jiuwenswarm/server/agent_ws_server.py` | 484KB | AgentServer WebSocket 主入口 |
| `jiuwenswarm/server/runtime/skill/skill_manager.py` | 327KB | Skill 全生命周期管理 |
| `jiuwenswarm/gateway/app_gateway.py` | 155KB | Gateway 主入口 |
| `jiuwenswarm/server/runtime/extension_package_manager.py` | 86KB | 扩展包管理 |
| `jiuwenswarm/server/runtime/agent_manager.py` | 66KB | Agent 实例管理 |
| `jiuwenswarm/runtime/service.py` | 47KB | AgentRuntime 生命周期 |
| `jiuwenswarm/symphony/service.py` | 43KB | Symphony 服务 |
| `jiuwenswarm/symphony/evolution/session_consumer.py` | 46KB | 进化会话消费 |
| `jiuwenswarm/gateway/message_handler/message_handler.py` | 224KB | 消息路由/会话/模式 |
| `jiuwenswarm/gateway/channel_manager/channel_manager.py` | 36KB | Channel 生命周期 |
| `jiuwenswarm/server/runtime/agent_warm_pool.py` | 30KB | Agent 预热池 |
| `jiuwenswarm/agents/swarm/assembly.py` | 18KB | Swarm 装配 |
| `jiuwenswarm/server/runtime/skill/skilldev/pipeline.py` | 7KB | SkillDev 流水线 |
| `jiuwenswarm/server/sandbox/jiuwenbox_runner.py` | 22KB | 沙箱 Runner |
| `jiuwenswarm/runtime/plan.py` | 12KB | Plan 模式控制器 |
| `jiuwenswarm/runtime/request.py` | 21KB | 请求归一化 |
| `jiuwenswarm/observability/store.py` | 86KB | OTLP SQLite 存储 |
| `jiuwenswarm/common/mode_matrix.py` | 19KB | `ResolvedMode`,`resolve_request_mode()` |
| `jiuwenswarm/common/model_vendor_registry.py` | 23KB | `VendorPreset`,`_PRESETS` |
| `jiuwenswarm/acp/stdio_client.py` | 25KB | ACP JSON-RPC 客户端 |

---

## 附录 B: 关键代码路径索引

| 核心机制 | 主入口文件 | 关键函数 |
|---------|-----------|---------|
| 多 Agent 协作 | `agents/swarm/assembly.py` | `enrich_team_spec_for_swarm`,`preflight_team_mcps` |
| SwarmBuildContext | `agents/swarm/context.py` | `SwarmBuildContext`,`to_seed`,`from_seed` |
| TeamManager | `agents/harness/team/team_manager.py` | `broadcast_event`,`begin_round`,`release_round`,`abort_round` |
| Cron 终态 | `common/cron_team_completion.py` | `apply_cron_team_round_event`,`cron_team_round_should_end` |
| E2A 编解码 | `common/e2a/wire_codec.py` | `parse_agent_server_wire_unary`,`encode_agent_response_for_wire` |
| ACP stdio | `acp/stdio_client.py` | `connect`,`_rpc_call`,`_handle_peer_request`,`chat`,`close` |
| A2UI 协议 | `server/runtime/a2ui/protocol.py` | `build_prompt`,`parse_response`,`validate_messages` |
| A2UI 解析 | `server/runtime/a2ui/parser.py` | `parse_a2ui_response`,`iter_tagged_block_bodies`,`parse_raw_json`,`parse_jsonl` |
| SkillDev 流水线 | `server/runtime/skill/skilldev/pipeline.py` | `run`,`resume`,`_checkpoint` |
| 挂起点配置 | `server/runtime/skill/skilldev/schema.py` | `SUSPENSION_POINTS`,`SuspensionConfig` |
| Skill Evolution | `symphony/evolution/service.py` | `load_dynamic_overlay`,`record_plan_outcome`,`rebuild_dynamic_overlay` |
| Workflow 状态 | `agents/harness/team/handlers/workflow_state.py` | `apply`,`_switch_to_phase`,`_bump_completion`,`_finalize_running_agents` |
| WarmKey | `server/runtime/agent_warm_pool.py` | `WarmKey`,`make_key`,`_key_priority`,`config_fingerprint` |
| WarmPool 同步 | `server/runtime/agent_warm_pool.py` | `sync`,`_pump_background_locked`,`begin_foreground`,`end_foreground`,`claim`,`_prepare` |
| Symphony Graph 构建 | `symphony/build.py` | `SymphonyGraphBuilder.status`,`SymphonyGraphBuilder.build` |
| Symphony 服务 | `symphony/service.py` | `refresh_graph`,`start_refresh_graph`,`cancel_build` |
| Progressive Retrieval | `symphony/retrieval/algorithm.md` | 算法描述(tree.py + retriever.py) |
| JiuwenBox Runner | `server/sandbox/jiuwenbox_runner.py` | `ensure_running`,`_try_set_pdeathsig`,`_sync_terminate`,`stop`,`_stop_no_lock` |

---

## 附录 C: 术语表

| 术语 | 含义 |
|------|------|
| **E2A** | AgentServer ↔ Gateway 通信协议(WebSocket + JSON Envelope) |
| **ACP** | Agent Communication Protocol(外部接入,JSON-RPC over stdio) |
| **A2UI** | Agent to UI(富交互输出协议,Tagged JSON) |
| **A2A** | Agent-to-Agent(内部总线) |
| **HITL** | Human-in-the-Loop(人工介入点) |
| **SwarmBuildContext** | 多 Agent 构建上下文 |
| **WarmPool** | Agent 预热池 |
| **Symphony** | 记忆/检索/索引/进化子系统 |
| **JiuwenBox** | 安全沙箱执行环境 |
| **SkillDev** | Skill 开发流水线 |
| **PlanMode** | 规划模式(只规划不执行) |
| **DeepAgent** | openjiuwen 核心 Agent 实例 |
| **Checkpointer** | 状态持久化检查点 |
| **Rail** | Agent 能力声明(声明式装配核心) |
| **Harness** | Agent 装配层(rails/tools/subagents) |

---

## 附录 D: 架构图

### D.1 启动流程

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

### D.2 消息流

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

### D.3 多 Agent 协作

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

> **总结**: JiuwenSwarm 是一个 **工业级多 Agent 协作系统**,其声明式装配、跨边界重建、Skill 自演进、SwarmFlow 确定性工作流、多渠道接入等设计,为 laew 工程提供了丰富的参考。建议 laew 从 **Plan 模式状态机**、**SwarmBuildContext 声明式 Spec**、**PR_SET_PDEATHSIG 子进程守护** 三个方向优先借鉴,逐步构建多 Agent 协作能力。
