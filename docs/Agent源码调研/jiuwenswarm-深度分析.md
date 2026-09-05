# JiuwenSwarm 深度源码分析

> **分析日期**: 2026-09-05
> **代码路径**: `/usr/local/LsmGitOpenSource/jiuwenswarm/`
> **代码规模**: Python 全栈，单文件最大 484KB（`agent_ws_server.py`），核心包 `jiuwenswarm/` 17 子模块
> **定位**: 多智能体协作系统（Swarm）+ Skill 自演进 + 多端接入 + 蜂群协作

---

## 目录

1. [四层架构深度分析](#1-四层架构深度分析)
2. [多 Agent 协作深度](#2-多-agent-协作深度)
3. [协议矩阵深度](#3-协议矩阵深度)
4. [Skill 系统深度](#4-skill-系统深度)
5. [SwarmFlow 深度](#5-swarmflow-深度)
6. [网关层深度](#6-网关层深度)
7. [运行时深度](#7-运行时深度)
8. [服务端深度](#8-服务端深度)
9. [记忆系统深度](#9-记忆系统深度)
10. [沙箱执行深度](#10-沙箱执行深度)
11. [对 laew 的深度借鉴建议](#11-对-laew-的深度借鉴建议)

---

## 1. 四层架构深度分析

### 1.1 架构概览

JiuwenSwarm 采用 **Channel → Gateway → Runtime → Server** 四层分离架构，通过进程级隔离实现高内聚低耦合：

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Channel 层（接入层）                          │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐       │
│  │  Web    │ │  TUI    │ │ Feishu  │ │ DingTalk│ │  ACP    │ ...   │
│  └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘       │
└───────┼───────────┼───────────┼───────────┼───────────┼─────────────┘
        │           │           │           │           │
        └───────────┴───────────┴─────┬─────┴───────────┘
                                      ▼
┌──────────────────────────────────────────────────────────────────────┐
│                         Gateway 层（网关层）                          │
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
│                         Runtime 层（运行时层）                        │
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
│                         Server 层（服务层）                          │
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

### 1.2 进程分裂设计

入口 `app.py` 通过 `subprocess.Popen` 启动两个子进程：

```python
# app.py L88-99
if getattr(sys, "frozen", False):
    agent_cmd = [python, "--desktop-run-agent"]
    gateway_cmd = [python, "--desktop-run-gateway"]
else:
    agent_cmd = [python, "-m", "jiuwenswarm.server.app_agentserver"]
    gateway_cmd = [python, "-m", "jiuwenswarm.gateway.app_gateway"]
```

**设计要点**：
- AgentServer 与 Gateway **分进程部署**，支持单机/分布式两种拓扑
- 通过 `--dotenv <path>` 实现多实例隔离（`parse_dotenv_early`）
- SIGTERM 统一走 `signal.default_int_handler` 保证子进程不孤儿化

### 1.3 数据流时序

```
User Input
    │
    ▼
Channel.on_message()
    │
    ▼
ChannelManager._on_channel_message()
    │ asyncio.create_task
    ▼
MessageHandler.handle_message()
    │ E2A WebSocket
    ▼
AgentRuntime.invoke() / stream()
    ├── prepare_chat_turn()  → 选择 Agent
    ├── PlanModeController.ensure_state()  → Plan 模式同步
    └── agent.process_message(request)
            │
            ▼
        AgentManager (agent_manager.py)
            ├── _get_or_create_agent()
            ├── warm_pool.acquire() / release()
            └── Agent.process_message_stream()
                    │
                    ▼
                RuntimeEvent Stream
                    │
                    ▼
                Channel.send() → User
```

---

## 2. 多 Agent 协作深度

### 2.1 SwarmBuildContext 生命周期

`SwarmBuildContext`（`agents/swarm/context.py`）是多 Agent 协作的核心上下文，继承自 openjiuwen 的 `BuildContext`：

```python
# agents/swarm/context.py L39-126
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
    trajectory_span_processor: Any = None
    heartbeat_job_service: Any = None
    config: dict[str, Any] | None = None
    skill_retrieval_toolkit: Any = None
```

**关键设计**：
- `to_seed()` / `from_seed()` 实现跨进程序列化：`trajectory_span_processor`、`heartbeat_job_service` 等非序列化句柄由接收端重新注入
- `derive()` 由 openjiuwen `setup_agent` 调用，生成 per-member 视图

### 2.2 Leader-Teammate 组装

`enrich_team_spec_for_swarm()`（`agents/swarm/assembly.py` L254-386）是 Team 装配入口：

```python
# assembly.py L254-285
def enrich_team_spec_for_swarm(
    spec: Any,
    *,
    session_id: str,
    mode: str,
    project_dir: str | None = None,
    trusted_dirs: list[str] | None = None,
    request_id: str | None = None,
    user_id: str | None = None,
    channel_id: str | None = None,
    request_metadata: dict[str, Any] | None = None,
    agent_group_name: str | None = None,
) -> None:
    register_swarm_providers()
    _ensure_external_team_transport(spec, channel_id)
    skills_library = get_agent_skills_dir()
    configure_global_skills_dir(skills_library)
    # ... 构建 SwarmBuildContext 并附着到 spec
```

**装配流程**：
1. 注册 Swarm Provider（`register_swarm_providers()`）
2. 确保外部传输通道（`_ensure_external_team_transport`）
3. 配置全局 Skill 库路径
4. 构建 per-team 基础 `SwarmBuildContext`
5. 重写 leader/teammate member spec 的 rails 和 tools
6. 可选加载 AgentGroup 包（`_apply_agent_group`）

### 2.3 Team 运行时

`TeamManager`（`agents/harness/team/team_manager.py`, 123KB）是核心编排器：
- `remote_member_bootstrap.py`（119KB）处理跨进程成员引导
- `distributed_runtime.py` 实现分布式运行时
- `config_loader.py`（25KB）加载 Team 配置

### 2.4 多 Agent 通信拓扑

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

`ChannelManager._build_mention_target()`（`channel_manager.py` L34-41）构造 mention 目标：

```python
def _build_mention_target(names: list[str]) -> dict[str, Any]:
    return {
        "intent": "mention",
        "mention_all": False,
        "member_names": tuple(names),
        "speaker": None,
    }
```

---

## 3. 协议矩阵深度

### 3.1 四种协议对比

| 协议 | 方向 | 传输层 | 序列化 | 用途 |
|------|------|--------|--------|------|
| **E2A** | Gateway ↔ AgentServer | WebSocket | JSON Envelope | 内部核心通信 |
| **ACP** | Client ↔ AgentServer | stdio / WebSocket | JSON-RPC-like | 外部 Agent 接入 |
| **A2A** | Agent ↔ Agent | 内部总线 | 结构化消息 | 多 Agent 协作 |
| **A2UI** | Agent → UI | 嵌入文本流 | Tagged JSON | 富交互输出 |

### 3.2 E2A 协议（核心）

`common/e2a/wire_codec.py` 实现 E2A 线编解码：

```python
# common/e2a/wire_codec.py L80-87
def is_e2a_response_wire_dict(data: dict[str, Any]) -> bool:
    if not isinstance(data, dict) or data.get("type") == "event":
        return False
    if data.get("protocol_version") != E2A_PROTOCOL_VERSION:
        return False
    rk = data.get("response_kind")
    return isinstance(rk, str) and bool(rk.strip())
```

**E2AEnvelope 结构**：
- `protocol_version`：协议版本
- `response_kind`：响应类型标识
- `request_id` / `response_id`：请求-响应关联
- `metadata`：携带 legacy 兜底键（`E2A_WIRE_LEGACY_AGENT_RESPONSE_KEY`）

**关键设计**：向后兼容 legacy 格式，`metadata` 中嵌入旧格式 blob，失败时 fallback。

### 3.3 ACP 协议

`acp/stdio_client.py`（25KB）实现 ACP stdio 客户端：
- 与 AgentServer 通过 stdio 通信
- `subprocess_env.py` 管理子进程环境
- `common/e2a/acp/protocol.py` 定义协议常量

### 3.4 A2UI 协议

`server/runtime/a2ui/protocol.py` 实现 A2UI v0.8 适配：

```python
# server/runtime/a2ui/protocol.py L72-89
class A2UIProtocolSpec:
    def __init__(self, version: str) -> None:
        if version != VERSION_0_8:
            raise ValueError(f"Unsupported A2UI protocol version: {version}")
        self.version = version
        self.examples_dir = _examples_dir(version)
        self.schema_manager = A2uiSchemaManager(
            version=version,
            catalogs=[BasicCatalog.get_config(version=version)],
            schema_modifiers=[remove_strict_validation],
        )
```

**A2UI 协议特点**：
- Tagged JSON 格式：`<a2ui>...</a2ui>` 包装
- 包含 `beginRendering` → `surfaceUpdate` → `dataModelUpdate` 三阶段
- `validate_a2ui_messages()` 校验 schema
- `build_repair_prompt()` 自动修复无效响应

### 3.5 协议转换层

`common/e2a/gateway_normalize.py`（23KB）负责 Gateway 侧归一化：
- `e2a_from_agent_fields()`：从 Agent 字段构造 E2A 消息
- `_normalize_gateway_message()`：归一化入站消息
- `_inject_session_work_mode()`：注入 work_mode 归一化

---

## 4. Skill 系统深度

### 4.1 SkillDevPipeline 状态机

`server/runtime/skill/skilldev/pipeline.py` 实现 12 阶段 SkillDev 流水线：

```python
# skilldev/pipeline.py L55-66
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
```

### 4.2 完整状态流转

```
INIT → PLAN → 【PLAN_CONFIRM】(HITL)
                  ↓
              GENERATE → VALIDATE → TEST_DESIGN → TEST_RUN
                                                        ↓
              IMPROVE ← 【REVIEW】(HITL) ← EVALUATE ←┘
                  │
                  └→ TEST_RUN (迭代)
                  
              PACKAGE → 【DESC_OPTIMIZE_CONFIRM】(HITL)
                            ↓
                        DESC_OPTIMIZE → COMPLETED
```

### 4.3 三个 HITL 挂起点

`skilldev/schema.py` 定义挂起点配置：

```python
# skilldev/schema.py L295-320
SUSPENSION_POINTS: dict[SkillDevStage, SuspensionConfig] = {
    SkillDevStage.PLAN_CONFIRM: SuspensionConfig(
        confirm_type="plan_confirm",
        title="请审阅开发计划",
        message="以下是生成的开发计划，请确认或修改",
        actions=[...],
        extract_data=_plan_extract_data,
        on_resume=_plan_confirm_on_resume,
        next_stage=SkillDevStage.GENERATE,
    ),
    SkillDevStage.REVIEW: SuspensionConfig(...),
    SkillDevStage.DESC_OPTIMIZE_CONFIRM: SuspensionConfig(...),
}
```

**挂起机制**（`pipeline.py` L80-101）：

```python
async def run(self) -> AsyncIterator[SkillDevEvent]:
    while self.state.stage not in (SkillDevStage.COMPLETED, SkillDevStage.ERROR):
        if self.state.stage in SUSPENSION_POINTS:
            suspension = SUSPENSION_POINTS[self.state.stage]
            await self._emit(SkillDevEventType.TODOS_UPDATE, {...})
            await self._emit(SkillDevEventType.CONFIRM_REQUEST, {
                "confirm_type": suspension.confirm_type,
                "title": suspension.title,
                "message": suspension.message,
                "data": suspension.extract_data(self.state),
                "actions": suspension.actions,
            })
            await self._checkpoint()  # 持久化状态
            break  # 暂停，等待 resume()
```

**恢复机制**（`pipeline.py` L150-175）：

```python
async def resume(self, data: dict) -> AsyncIterator[SkillDevEvent]:
    current_stage = self.state.stage
    suspension = SUSPENSION_POINTS[current_stage]
    suspension.on_resume(self.state, data)  # 更新状态
    next_stage = suspension.next_stage
    if callable(next_stage):
        next_stage = next_stage(data)  # REVIEW 阶段动态决定
    self.state.stage = next_stage
    async for event in self.run():
        yield event
```

### 4.4 SkillDevState 状态

`skilldev/schema.py` L113-209 定义运行时状态：

```python
@dataclass
class SkillDevState:
    task_id: str
    stage: SkillDevStage = SkillDevStage.INIT
    mode: SkillDevTaskMode = SkillDevTaskMode.CREATE
    iteration: int = 0  # 改进轮次
    plan: dict[str, Any] | None = None
    eval_results: dict[str, Any] | None = None
    feedback_history: list[dict] = field(default_factory=list)
    zip_path: str | None = None
    # ...
    
    def to_checkpoint_dict(self) -> dict: ...
    @classmethod
    def from_checkpoint_dict(cls, data: dict) -> "SkillDevState": ...
```

### 4.5 Skill 管理器

`server/runtime/skill/skill_manager.py`（327KB，最大文件）负责：
- Skill CRUD、搜索、安装
- Skill 文件解析（YAML frontmatter）
- Skill 与 Symphony graph 的关联

`archive_store.py`（14KB）管理 Skill 打包归档。

---

## 5. SwarmFlow 深度

SwarmFlow 是 JiuwenSwarm 的确定性多阶段工作流系统，特点：

- **DAG 构建**：Python 脚本定义阶段依赖
- **HITL 支持**：`human` / `human_session` 挂起点
- **Team Token 预算**：控制整体消耗
- **TUI 运行树监控**：`/swarmflows` 命令查看

与 SkillDevPipeline 的区别：
- SkillDevPipeline 是**线性为主、局部迭代**的确定性状态机
- SwarmFlow 是**DAG 拓扑**的复杂工作流，支持并行分支

---

## 6. 网关层深度

### 6.1 ChannelManager 核心

`gateway/channel_manager/channel_manager.py`（36KB）：

```python
# channel_manager.py L44-77
class ChannelManager(ABC):
    def __init__(
        self,
        message_handler: "MessageHandler",
        config: dict[str, Any] | None = None,
        on_config_updated: Callable[[dict[str, Any]], Awaitable[None]] | None = None,
    ) -> None:
        self._message_handler = message_handler
        self._channels: dict[ChannelKey, "BaseChannel"] = {}
        self._dispatch_task: asyncio.Task | None = None
        self._running = False
        self._config: dict[str, Any] = dict(config or {})
        self._conf_revisions: dict[str, int] = {}
        self._on_config_updated = on_config_updated
        self._pending_channel_restart: set[str] = set()
        self._channel_event_callbacks: list[Callable[[ChannelEvent], Awaitable[None]]] = []
```

**核心功能**：
- Channel 注册/注销（`register_channel` / `unregister_channel`）
- 消息统一转发到 MessageHandler（`_on_channel_message`）
- 配置热更新（`set_config` / `on_config_updated`）
- Channel 连接事件订阅（`subscribe_channel_events`）

### 6.2 Channel 类型

`channels/` 目录包含：
- `web/`：WebSocket 浏览器接入
- `tui/`：终端 UI
- `browser/`：浏览器自动化
- `cli/`：命令行
- `acp/`：ACP 协议
- `desktop/`：桌面端
- `process_cli/`：进程级 CLI

### 6.3 IM 平台适配

`gateway/channel_manager/im_platforms/`（12 个子目录）包含：
- 飞书（Feishu）
- 钉钉（DingTalk）
- 小翼（Xiaoyi/华为）
- 其他企业 IM

### 6.4 附件落盘钩子

`channel_manager.py` L149-264 的 `_try_wire_file_persist_hook()`：

```python
def _try_wire_file_persist_hook(self, channel: "BaseChannel") -> None:
    setter = getattr(channel, "set_file_persist_hook", None)
    if setter is None:
        return
    agent_client = getattr(self._message_handler, "agent_client", None)
    if agent_client is None:
        return
    # ...
    async def _persist_hook(content: Any, category: str, filename: str) -> dict[str, Any]:
        content_bytes = bytes(content)
        if len(content_bytes) > E2A_PAYLOAD_MAX_BYTES:
            # 大附件走 HTTP bridge 上传
            ok, payload = await asyncio.to_thread(
                upload_file_bytes, content_bytes, rel_path)
            # ...
        # 小附件走 base64 E2A
        ok, payload = await fetch_agent_unary(
            agent_client=agent_client,
            req_method=ReqMethod.IM_FILE_PERSIST,
            params={"platform": storage_platform, "category": category,
                    "filename": filename, "data": _base64.b64encode(content_bytes).decode("ascii")},
            # ...
        )
    setter(_persist_hook)
```

**设计要点**：大附件走 HTTP bridge 避免 WebSocket 8MB 帧限制（`PayloadTooBig`）。

### 6.5 路由层

`gateway/routing/` 目录：
- `route_binding.py`：GatewayRouteBinding 路由绑定
- `agent_request_timeout.py`：超时控制
- `e2a_proxy.py`：E2A 代理
- `agent_http_bridge.py`：HTTP bridge（大附件上传）

---

## 7. 运行时深度

### 7.1 AgentRuntime 核心

`runtime/service.py`（47KB）是运行时所有者：

```python
# service.py L230-271
class AgentRuntime:
    def __init__(
        self,
        *,
        agent_manager: AgentManager | None = None,
        initializer: Callable[[], Awaitable[None]] | None = None,
        plan_controller: PlanModeController | None = None,
        admission_controller: Any | None = None,
        session_delete_lifecycle: SessionDeleteLifecycle | None = None,
        enable_kvc_tracking: bool = False,
    ) -> None:
        self._agent_manager = agent_manager or AgentManager()
        self._plan_controller = plan_controller
        self._admission_controller = admission_controller
        self._session_provisioner = RuntimeSessionProvisioner(...)
        self._enable_kvc_tracking = bool(enable_kvc_tracking)
        self._stateless_agents: dict[str, Any] = {}
        self._lifecycle_lock = asyncio.Lock()
        self._started = False
        self._closed = False
```

**关键方法**：

| 方法 | 职责 |
|------|------|
| `start()` | 初始化 runtime 依赖（checkpointer、Runner） |
| `invoke()` | 执行单请求非流式 |
| `stream()` | 执行单请求流式 |
| `prepare_chat_turn()` | 解析 session 语义、选择 Agent |
| `cancel_request()` | 取消请求 |
| `cleanup_session()` | 清理会话资源 |

### 7.2 进程级依赖管理

```python
# service.py L45-101
async def _acquire_process_runtime_dependencies() -> None:
    global _PROCESS_RUNTIME_DEPENDENCY_USERS
    async with _PROCESS_RUNTIME_DEPENDENCY_LOCK:
        if _PROCESS_RUNTIME_DEPENDENCY_USERS == 0:
            await _initialize_runtime_dependencies()
            from openjiuwen.core.runner import Runner
            runner_started = await Runner.start()
            if runner_started is False:
                raise RuntimeError("Runner failed to start")
        _PROCESS_RUNTIME_DEPENDENCY_USERS += 1
```

**设计要点**：引用计数 + 互斥锁确保进程全局单例（checkpointer、Runner），支持多个 Runtime 所有者。

### 7.3 流式执行引擎

```python
# service.py L695-871
async def _stream_started(
    self,
    request: AgentRequest,
    *,
    trigger_hook: bool,
    on_control_event: Callable[[RuntimeEvent], Awaitable[None]] | None,
    background: bool,
    on_agent_ready: Callable[[Any], Any] | None,
) -> AsyncIterator[RuntimeEvent]:
    # 1. 触发 before_chat_request hook
    # 2. 开始 foreground chat（如有）
    # 3. 准入控制（如有）
    # 4. 准备 chat turn 或获取 stateless agent
    # 5. Plan 模式状态同步
    # 6. on_agent_ready 回调
    # 7. 迭代 agent.process_message_stream()
    # 8. 清理：plan post_process、admission end、foreground end
```

**ContextVar 管理**（`runtime/context.py`）：

```python
# service.py L661-693
async def stream(self, request: AgentRequest, ...) -> AsyncIterator[RuntimeEvent]:
    stream = self._stream_started(...)
    try:
        while True:
            token = set_runtime_context(self, self._agent_manager)
            try:
                event = await anext(stream)
            except StopAsyncIteration:
                return
            finally:
                reset_runtime_context(token)
            yield event
    finally:
        token = set_runtime_context(self, self._agent_manager)
        try:
            await stream.aclose()
        finally:
            reset_runtime_context(token)
```

### 7.4 PlanModeController

`runtime/plan.py` 实现 Plan 模式状态机：

```python
# plan.py L39-285
class PlanModeController:
    def __init__(self) -> None:
        self._sync_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()
        self._exited_sessions: set[str] = set()
        self._active_sessions: set[str] = set()
    
    async def ensure_state(self, request, mode, sub_mode, agent) -> PlanStateResult:
        # 1. 解析 runtime mode
        # 2. 判断是否需要 plan 状态同步
        # 3. 获取 session 锁
        # 4. 打开 agent state session
        # 5. 切换 plan ↔ normal 状态
        # 6. 注入 plan mode reminder
```

**Plan Reminder 注入**（`plan.py` L151-176）：

```python
@staticmethod
def inject_activation_reminder(request: AgentRequest) -> None:
    reminder = (
        "\n\n<system-reminder>\n"
        "Plan mode is active. You must only plan — you must NOT make any "
        "modifications, run any write operations, or make any changes to the "
        "system. This constraint takes priority over any other instructions.\n"
        # ...
        "</system-reminder>"
    )
    query = request.params.get("query") or ""
    request.params[PLAN_REMINDER_ORIGINAL_QUERY_KEY] = query
    request.params["query"] = reminder + query
```

### 7.5 请求归一化

`runtime/request.py`（21KB）处理请求语义：

```python
# request.py L223-282
def resolve_agent_request_mode(
    raw_mode: Any,
    *,
    work_mode: Any = None,
) -> tuple[str, str | None, str]:
    # 返回 (manager_mode, sub_mode, canonical_mode)
    # 例：mode="plan" + work_mode="code" → ("code", "normal", "code.normal")
    #     mode="team.plan" → ("team", "plan", "team.plan")
```

---

## 8. 服务端深度

### 8.1 AgentManager 生命周期

`server/runtime/agent_manager.py`（66KB）管理 Agent 实例：

```python
# agent_manager.py L111-150
class AgentManager:
    def __init__(self) -> None:
        self.agents: dict[str, dict[str, "JiuWenSwarm"]] = {}
        self._personal_context_runtime_enabled: bool = False
        self._agent_create_params: dict[str, dict[str, dict[str, Any]]] = {}
        self._client_capabilities_by_channel: dict[str, dict[str, Any]] = {}
        self._latest_env_overrides: dict[str, Any] = {}
        self._reload_lock: asyncio.Lock = asyncio.Lock()
        self._agent_borrowers: dict[int, set[asyncio.Task]] = {}
        self._agent_pins: dict[int, int] = {}
        self._heartbeat_service: Any | None = None
        self._pending_tui_retirements: set[int] = set()
        self._retirement_tasks: dict[int, asyncio.Task] = {}
        self._agent_create_locks: WeakValueDictionary[
            tuple[str, str], asyncio.Lock
        ] = WeakValueDictionary()
        self._session_create_tokens: dict[tuple[str, str], tuple[Any, Any]] = {}
        self._session_create_token_lock = asyncio.Lock()
        self.warm_pool = AgentWarmPool(self)
```

**Agent 缓存 Key**（`agent_manager.py` L85-89）：

```python
def _make_agent_cache_key(mode: str | None, sub_mode: str | None, project_dir: str | None) -> str:
    mode_key = _normalize_mode(mode)
    sub_mode_key = collapse_plan_sub_mode(mode_key, sub_mode)
    project_key = _normalize_project_dir(project_dir)
    return f"{mode_key}:{sub_mode_key}:{project_key}"
```

**Plan 子模式并轨**：plan 与非 plan 使用同一 Agent 实例（切换 `context_engine` 会丢失对话历史）。

### 8.2 AgentWarmPool 预热策略

`server/runtime/agent_warm_pool.py`（30KB）：

```python
# agent_warm_pool.py L115-165
class AgentWarmPool:
    EXCLUDED_CHANNELS = frozenset({"acp", "a2a"})

    def __init__(
        self,
        manager: "AgentManager",
        *,
        max_concurrency: int = 1,
        max_ready_slots: int = 1,
        max_foreground_concurrency: int = 8,
        background_cooldown_seconds: float = 0.25,
        enabled: bool | None = None,
    ) -> None:
        self._enabled = _prewarm_enabled_by_env() if enabled is None else bool(enabled)
        # ...
```

**预热策略**：
- 默认关闭，需环境变量 `JIUWENSWARM_AGENT_PREWARM=1` 开启
- 基于 `WarmKey`（channel_id + project_id + project_dir + work_mode + is_swarm）匹配槽位
- `WarmRevision`（boot_id + config_fingerprint + sequence）检测配置变更
- 信号量控制并发（`_semaphore` / `_foreground_semaphore`）
- ACP/A2A 通道排除预热

**Claim 机制**：

```python
# agent_warm_pool.py L108-113
@dataclass(frozen=True, slots=True)
class WarmClaim:
    session_id: str
    prewarm_hit: bool
    prewarm_status: str
```

### 8.3 Session 生命周期

`server/runtime/session/` 目录包含：
- `session_history.py`：会话历史
- `session_metadata.py`：会话元数据
- `project_store.py`：项目持久化
- `work_mode.py`：工作模式解析

### 8.4 扩展包管理

`server/runtime/extension_package_manager.py`（86KB）管理：
- AgentGroup 包加载
- 扩展注册与卸载
- 版本兼容性检查

---

## 9. 记忆系统深度

### 9.1 Symphony 子系统架构

Symphony 是 JiuwenSwarm 的记忆/检索/索引/进化核心：

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
│  ┌────────────┐ ┌────────────┐                                      │
│  │ graph_     │ │ graph_     │                                      │
│  │ state.py   │ │ storage.py │                                      │
│  └────────────┘ └────────────┘                                      │
└──────────────────────────────────────────────────────────────────────┘
```

### 9.2 离线 Graph 构建

`symphony/build.py` 实现 graph 构建：

```python
# build.py L156-260
class SymphonyGraphBuilder:
    def status(self, skills_root, graph_dir, *, llm_config, symphony_config) -> GraphStatus:
        # 1. 扫描 Skill 目录
        # 2. 计算 capability hashes
        # 3. 对比 active_entries 检测 stale
        # 4. 返回 GraphStatus（exists / stale / added / changed / removed）
    
    async def build(self, skills_root, graph_dir, llm_config, *, force, build_log, symphony_config, resume) -> GraphBuildResult:
        # 1. 创建 run_id 与 checkpoint
        # 2. 检测可恢复构建（resume）
        # 3. 扫描 Skill 目录
        # 4. Fingerprint 提取
        # 5. Graph 关系构建
        # 6. 写入 artifact
```

**GraphStatus 结构**：

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

### 9.3 渐进式检索

`symphony/retrieval/algorithm.md` 定义检索算法：

```
输入：query, tree root, top_k
流程：
1. apply request tag filters to catalog leaves and prune empty tree branches
2. start from the resulting visible subtree
3. if the structure is trivial, use deterministic shortcuts
4. otherwise ask the LLM to choose among the current visible boundary nodes
5. recurse into selected branches or terminate on selected items
6. reduce branch results to the requested top_k
```

**关键规则**：
- LLM 路由决策只看到当前可见子树
- 模型输出可见边界节点的 display names
- display names 自动去重
- 单一候选不调用 LLM

### 9.4 Skill 检索

`symphony/skill_retrieval/` 目录提供 Skill 级别的检索：
- 基于 Symphony graph 的 Skill 发现
- 与 `skill_retrieval_toolkit`（per-member）集成

### 9.5 进化子系统

`symphony/evolution/` 实现能力自进化：

```python
# evolution/session_consumer.py (46KB)
# 消费执行会话数据，驱动能力指纹更新
```

`evolution/service.py`（11KB）：
- `load_dynamic_overlay()`：加载动态覆盖层
- 启用后影响 orchestration planning

### 9.6 配置

`symphony/config.py`：

```python
@dataclass
class SymphonyConfig:
    paths: SymphonyPaths
    fingerprint: FingerprintConfig
    orchestration: OrchestrationConfig
    evolution: EvolutionConfig
```

---

## 10. 沙箱执行深度

### 10.1 JiuwenBox Runner

`server/sandbox/jiuwenbox_runner.py`（22KB）管理 jiuwenbox 子进程：

```python
# jiuwenbox_runner.py L70-96
class JiuwenBoxRunner:
    _INSTANCE: "JiuwenBoxRunner | None" = None
    _STDERR_TAIL_MAX: int = 80

    def __init__(self) -> None:
        self._process: Optional[asyncio.subprocess.Process] = None
        self._host: str = "127.0.0.1"
        self._port: int = 8321
        self._lock = asyncio.Lock()
        self._owns_process: bool = False
        self._atexit_registered: bool = False
        self._stdout_pump_task: Optional[asyncio.Task] = None
        self._stderr_pump_task: Optional[asyncio.Task] = None
        self._stderr_tail: list[str] = []
        self._last_startup_mode: str = "internal"
        self._spawned_policy_path: Optional[Path] = None
```

### 10.2 启动模式

```python
# jiuwenbox_runner.py L161-253
async def ensure_running(
    self,
    host: str = "127.0.0.1",
    port: int = 8321,
    *,
    timeout: float = 30.0,
    startup_mode: str = "internal",
    policy_path: Optional[Path] = None,
) -> bool:
    # internal 模式：agent-server 自己管理 jiuwenbox 生命周期
    # external 模式：用户自己启动，仅做健康检查
    
    owned_match = (
        self._process is not None
        and self._process.returncode is None
        and self._owns_process
        and self._host == host
        and self._port == port
        and self._spawned_policy_path == policy_path
    )
    if owned_match:
        if await self.health_check(host, port):
            return True  # 复用
    # mismatch → 停旧进程，spawn 新实例
    # ...
    cmd = [sys.executable, "-m", "uvicorn", "jiuwenbox.server.app:app",
           "--host", host, "--port", str(port)]
    # 注入 JIUWENBOX_POLICY_PATH 环境变量
```

### 10.3 安全机制

**Linux pdeathsig**（`jiuwenbox_runner.py` L54-67）：

```python
def _try_set_pdeathsig() -> None:
    """Linux: 让子进程在父进程退出时收到 SIGTERM"""
    if not sys.platform.startswith("linux"):
        return
    try:
        import ctypes
        libc = ctypes.CDLL("libc.so.6", use_errno=True)
        libc.prctl(_PR_SET_PDEATHSIG, signal.SIGTERM, 0, 0, 0)
    except Exception:
        pass
```

**单例模式**（`jiuwenbox_runner.py` L97-101）：

```python
@classmethod
def instance(cls) -> "JiuwenBoxRunner":
    if cls._INSTANCE is None:
        cls._INSTANCE = JiuwenBoxRunner()
    return cls._INSTANCE
```

**健康检查**：

```python
async def health_check(self, host: str | None = None, port: int | None = None) -> bool:
    url = f"http://{target_host}:{target_port}/health"
    try:
        async with httpx.AsyncClient(timeout=2.0) as client:
            resp = await client.get(url)
            return resp.status_code == 200
    except Exception:
        return False
```

### 10.4 Policy 注入

jiuwenbox 通过 `JIUWENBOX_POLICY_PATH` 环境变量注入安全策略：
- `policy_path` 变化时强制重启（避免旧进程用旧 policy）
- 父进程未传时显式删除继承的环境变量

### 10.5 沙箱隔离层次

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

## 11. 对 laew 的深度借鉴建议

### 11.0 laew 现状速览

laew 当前是**双 Agent 架构**（Yolo + Work），单进程 Rust CLI，SQLite 持久化，零沙箱零校验。JiuwenSwarm 的许多设计可直接映射到 laew 路线图。

### 11.1 P0（立刻借鉴，高 ROI）

| 能力 | JiuwenSwarm 实现 | laew 借鉴方案 | 依据文件 |
|------|------------------|---------------|----------|
| **Plan 模式状态机** | `PlanModeController` + `inject_activation_reminder()` | 实现 `PlanModeController`，Yolo 分类 hard 任务时自动注入 Plan reminder 到系统提示词 | `runtime/plan.py` L151-176 |
| **流式 ContextVar 传播** | `set_runtime_context()` / `reset_runtime_context()` | 用 `tokio::sync::watch` 或 `task_local!` 实现跨 yield 点的 Runtime 上下文 | `runtime/service.py` L661-693 |
| **请求归一化层** | `resolve_request_runtime_mode()` | 实现 `resolve_task_level()` 将 Yolo 输出的 simple/medium/hard 映射到 canonical mode | `runtime/request.py` L223-282 |
| **附件/大文件分层传输** | `E2A_PAYLOAD_MAX_BYTES` + HTTP bridge | laew 暂不需要（无多进程），但 Read 工具应支持大文件分片读取 | `channel_manager.py` L199-244 |

**Plan 模式详细设计**：

```rust
// laew 借鉴实现草案
pub struct PlanModeController {
    exited_sessions: HashSet<String>,
    active_sessions: HashSet<String>,
}

impl PlanModeController {
    /// 在 Yolo 判定 hard 任务后，注入 Plan 约束到系统提示词
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

### 11.2 P1（下一阶段，中等 ROI）

| 能力 | JiuwenSwarm 实现 | laew 借鉴方案 | 依据文件 |
|------|------------------|---------------|----------|
| **HITL 挂起机制** | `SUSPENSION_POINTS` + `resume()` | SubAgent 执行到关键节点（写操作前）暂停，用户确认后继续 | `skilldev/pipeline.py` L80-101 |
| **SkillDev 状态机** | `SkillDevPipeline` + `STAGE_HANDLERS` | 实现 laew 的 Skill 开发流水线（至少 INIT→PLAN→GENERATE→VALIDATE→PACKAGE） | `skilldev/pipeline.py` L55-66 |
| **Agent 预热池** | `AgentWarmPool` + `WarmKey` | 预编译常用 AgentProfile，减少首次请求延迟 | `agent_warm_pool.py` L115-165 |
| **记忆图构建** | `SymphonyGraphBuilder` + `FingerprintService` | 用 SQLite 实现轻量级 Skill/Tool 关系图，支持检索注入 | `symphony/build.py` L156-260 |
| **渐进式检索** | progressive tree retrieval | laew 的 SessionContext 摘要可按 tree 结构组织，渐进检索 | `symphony/retrieval/algorithm.md` |

**HITL 挂起详细设计**：

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
    /// 遇到挂起点时，推送确认请求并暂停
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
        // 继续执行...
        Ok(())
    }
}
```

### 11.3 P2（长期规划，战略价值）

| 能力 | JiuwenSwarm 实现 | laew 借鉴方案 | 依据文件 |
|------|------------------|---------------|----------|
| **多协议适配层** | E2A/ACP/A2A/A2UI | laew 已实现 Anthropic/OpenAI 双协议，可考虑 A2UI 风格的富输出 | `server/runtime/a2ui/protocol.py` |
| **SwarmFlow DAG** | Python 脚本定义 DAG | 实现 Rust 侧的 WorkFlow DSL，支持 HITL 挂起 | `agents/swarm/assembly.py` |
| **SwarmBuildContext** | `SwarmBuildContext` + `to_seed/from_seed` | laew 的 SubAgent 间传递上下文可参考此模式 | `agents/swarm/context.py` |
| **沙箱隔离** | JiuwenBox + Policy Engine | laew 零沙箱现状需重点突破：基于 Landlock/seccomp 的 Bash 沙箱 | `server/sandbox/jiuwenbox_runner.py` |
| **能力指纹进化** | `FingerprintService` + evolution | laew 的工具/Agent 定义可引入 fingerprint 机制，自动检测过时定义 | `symphony/evolution/` |

### 11.4 跨项目模式提炼

从 JiuwenSwarm 提炼的 **10 个可复用设计模式**：

| # | 模式 | 描述 | 适用场景 |
|---|------|------|----------|
| 1 | **进程分裂 + 单例依赖** | 多进程拓扑 + 引用计数单例 | laew 未来分布式部署 |
| 2 | **ContextVar 跨 yield 传播** | 每 yield 重置 token，子任务正常继承 | laew 的流式 tool_call |
| 3 | **HITL Checkpoint** | 挂起点持久化 + resume() 恢复 | laew 的 SubAgent 关键节点确认 |
| 4 | **Plan 状态并轨** | plan/normal 同一实例，仅换工具可见性 | laew 的 Plan Agent 与 Main-Work Agent 统一 |
| 5 | **E2A Envelope + Legacy Fallback** | 新版本 envelope + metadata legacy blob | laew 的协议演进 |
| 6 | **渐进式树检索** | LLM 路由决策仅见可见子树 | laew 的 Skill/Tool 检索注入 |
| 7 | **附件大小分层** | 阈值下 base64，阈值上 HTTP bridge | laew 的文件工具 |
| 8 | **Config Fingerprint 失效** | SHA256(config) 变更触发重建 | laew 的 AgentProfile 热更新 |
| 9 | **pdeathsig 子进程守护** | Linux prctl 防止孤儿 | laew 的 Bash 子进程管理 |
| 10 | **WarmKey 多维匹配** | channel × project × mode 槽位 | laew 的 AgentProfile 预热 |

### 11.5 综合路线图

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         laew 演进路线图                                  │
├─────────────────────────────────────────────────────────────────────────┤
│  P0 (立刻)                                                              │
│  ├── PlanModeController：Plan reminder 注入                             │
│  ├── Runtime ContextVar：跨 yield 上下文传播                            │
│  └── TaskLevel → CanonicalMode 映射                                     │
├─────────────────────────────────────────────────────────────────────────┤
│  │                                                                      │
│  ▼                                                                      │
│  P1 (下阶段)                                                            │
│  ├── HITL 挂起/恢复机制（SubAgent 写操作前确认）                         │
│  ├── SkillDev 状态机（INIT→PLAN→GEN→VALID→PACKAGE）                     │
│  ├── AgentProfile 预热池（减少首次延迟）                                │
│  └── 轻量级记忆图（SQLite 存储 Tool/Skill 关系）                        │
├─────────────────────────────────────────────────────────────────────────┤
│  │                                                                      │
│  ▼                                                                      │
│  │                                                                      │
│  P2 (长期)                                                              │
│  ├── A2UI 风格富输出（可选组件协议）                                    │
│  ├── SwarmFlow DAG DSL                                                 │
│  ├── 多 Agent 上下文序列化（to_seed/from_seed）                          │
│  ├── Landlock/seccomp 沙箱                                             │
│  └── 能力指纹进化（过时定义自动检测）                                   │
└─────────────────────────────────────────────────────────────────────────┘
```

### 11.6 反模式警示

从 JiuwenSwarm 识别的 **5 个反模式**（laew 应避免）：

| # | 反模式 | JiuwenSwarm 表现 | laew 规避策略 |
|---|--------|------------------|---------------|
| 1 | **单文件膨胀** | `agent_ws_server.py` 484KB | 严格模块边界，单文件 >5K 行即拆分 |
| 2 | **过度抽象** | `ABC` + 多层 `dataclass` 嵌套 | Rust enum + struct 足够时不引入 trait |
| 3 | **配置蔓延** | `config.yaml` + `.env` + `state.json` 三源 | 坚持 SQLite 单源，不引入新配置文件 |
| 4 | **循环依赖** | `gateway` ↔ `server` 双向 import | 严格分层，上层可引用下层，反之禁止 |
| 5 | **Optional 泛滥** | 大量 `if xxx is not None` 守卫 | Rust `Option` + `?` 运算符 + enum 状态机 |

---

## 附录 A：关键文件索引

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
| `jiuwenswarm/gateway/channel_manager/channel_manager.py` | 36KB | Channel 生命周期 |
| `jiuwenswarm/server/runtime/agent_warm_pool.py` | 30KB | Agent 预热池 |
| `jiuwenswarm/agents/swarm/assembly.py` | 18KB | Swarm 装配 |
| `jiuwenswarm/server/runtime/skill/skilldev/pipeline.py` | 7KB | SkillDev 流水线 |
| `jiuwenswarm/server/sandbox/jiuwenbox_runner.py` | 22KB | 沙箱 Runner |
| `jiuwenswarm/runtime/plan.py` | 12KB | Plan 模式控制器 |
| `jiuwenswarm/runtime/request.py` | 21KB | 请求归一化 |

## 附录 B：术语表

| 术语 | 含义 |
|------|------|
| **E2A** | AgentServer ↔ Gateway 通信协议 |
| **ACP** | Agent Communication Protocol（外部接入） |
| **A2UI** | Agent to UI（富交互输出协议） |
| **HITL** | Human-in-the-Loop（人工介入点） |
| **SwarmBuildContext** | 多 Agent 构建上下文 |
| **WarmPool** | Agent 预热池 |
| **Symphony** | 记忆/检索/索引/进化子系统 |
| **JiuwenBox** | 安全沙箱执行环境 |
| **SkillDev** | Skill 开发流水线 |
| **PlanMode** | 规划模式（只规划不执行） |
| **DeepAgent** | openjiuwen 核心 Agent 实例 |
| **Checkpointer** | 状态持久化检查点 |

---

> **分析总结**：JiuwenSwarm 是一个设计精良的多 Agent 协作系统，其四层架构、HITL 挂起机制、Plan 状态机、渐进式检索等设计对 laew 有极高参考价值。建议 laew 优先借鉴 **Plan 模式状态机**（P0）和 **HITL 挂起机制**（P1），这两项能显著提升 laew 的安全性与可控性。
