# JiuwenSwarm 核心机制深度分析

> **分析日期**: 2026-09-05
> **工程路径**: `/usr/local/LsmGitOpenSource/jiuwenswarm`
> **前置文档**: `jiuwenswarm-源码调研.md`、`jiuwenswarm-深度分析.md`
> **分析目标**: 在源码层面对 JiuwenSwarm 的 7 个核心机制进行可执行的代码路径剖析,定位关键函数/数据结构,并提炼对 laew 的借鉴点

---

## 目录

1. [多 Agent 协作核心代码路径](#1-多-agent-协作核心代码路径)
2. [协议矩阵核心代码路径](#2-协议矩阵核心代码路径)
3. [SkillDevPipeline 核心代码路径](#3-skilldevpipeline-核心代码路径)
4. [SwarmFlow 核心代码路径](#4-swarmflow-核心代码路径)
5. [AgentWarmPool 核心代码路径](#5-agentwarmpool-核心代码路径)
6. [Symphony 记忆核心代码路径](#6-symphony-记忆核心代码路径)
7. [沙箱执行核心代码路径](#7-沙箱执行核心代码路径)
8. [综合借鉴清单](#8-综合借鉴清单)

---

## 1. 多 Agent 协作核心代码路径

### 1.1 Leader 任务分解的函数链

JiuwenSwarm 的多 Agent 协作不是硬编码编排,而是**声明式 Spec → Runtime Build**。`enrich_team_spec_for_swarm()` 是单一入口。

**核心代码路径**:
- `agents/swarm/assembly.py::enrich_team_spec_for_swarm()` (入口, L254-403)
- `agents/swarm/assembly.py::preflight_team_mcps()` (MCP 探活, L405-457)
- `agents/swarm/config_specs.py::build_member_deep_agent_spec()` (成员装配)
- `openjiuwen.core.runner.Runner.run_agent_team_streaming()` (实际编排执行,openjiuwen 框架)

**关键函数签名与代码片段**:

```python
# agents/swarm/assembly.py (L254)
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
        session_id=session_id,
        request_id=request_id,
        user_id=user_id,
        channel_id=channel_id,
        channel=channel_id or "default",
        request_metadata=request_metadata,
        mode=mode,
        project_dir=project_dir,
        trusted_dirs=trusted_dirs,
        disable_teammate_worktree=str(channel_id or "").strip().lower() == "web",
        team_id=spec.team_name,
        team_ws_root=team_ws_root,
        task_workspace_root=task_workspace_root,
        team_outputs_dir=team_outputs_dir,
        team_skill_visibility_path=team_visibility_path,
        global_skills_dir=global_skills_dir,
        trajectory_span_processor=get_trajectory_span_processor(),
        heartbeat_job_service=get_heartbeat_job_service(),
        config=config,
    )
    mcp_configs = build_enabled_mcp_server_configs(
        config,
        server_id_scope=f"team:{spec.team_name}",
        resolve_credentials=True,
    )
    # 4) 装配 leader / teambte 成员(各自的能力差异由 role 决定)
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

**设计意图**: `enrich_team_spec_for_swarm` 通过**「属性 vs 环境」**分离实现跨边界重建——`spec.agents` 携带 config 派生的属性值(换请求不变),`SwarmBuildContext` 携带环境值(随请求/会话变化)。`build_context_seed` 是 to_dict 序列化产物,可在子进程/分布式机器上通过 `from_seed` 重建。

### 1.2 SwarmBuildContext 的构建和传递

**核心代码路径**:
- 定义: `agents/swarm/context.py::SwarmBuildContext` (L39-262)
- 序列化: `SwarmBuildContext.to_seed()` (L181-212)
- 反序列化: `SwarmBuildContext.from_seed()` (L214-255)

**关键字段与代码片段**:

```python
# agents/swarm/context.py (L39)
@dataclass
class SwarmBuildContext(BuildContext):
    """BuildContext subclass carrying jiuwenswarm runtime handles."""
    session_id: str = ""
    request_id: str | None = None
    user_id: str | None = None
    channel_id: str | None = None
    channel: str = "default"
    request_metadata: dict[str, Any] | None = None
    mode: str = "team"
    project_dir: str | None = None
    trusted_dirs: list[str] | None = None
    disable_teammate_worktree: bool = False
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
        """只导出可序列化的 per-team/per-process 字段。
        关键设计:心跳/处理器/配置不可序列化,接收端需自行获取。
        """
        return {
            "session_id": self.session_id,
            "request_id": self.request_id,
            ...
            "global_skills_dir": self.global_skills_dir,
            # 注意: trajectory_span_processor / heartbeat_job_service / config
            #       不在 seed 中,接收端用本进程的等价物补全
        }

    @classmethod
    def from_seed(cls, seed, *, config, trajectory_span_processor):
        """Rebuild a context from a to_dict 映射 + 本进程句柄。"""
        return cls(
            ...,
            trajectory_span_processor=trajectory_span_processor,
            heartbeat_job_service=get_heartbeat_job_service(),  # 本进程获取
            config=config,                                       # 本进程获取
        )

    def resolve_member_skill_visibility_path(self) -> str | None:
        """每成员路径解析,必须等到 setup_agent 通过 derive() 填充
        member_name 后才能计算,所以是方法而非字段。
        """
        workspace = self.workspace
        root_path = getattr(workspace, "root_path", None) if workspace else None
        if root_path:
            return str(Path(root_path) / ojw_paths.SKILL_VISIBILITY_FILENAME)
        ...
```

**设计意图**:
- **`build_context` vs `build_context_seed` 双轨**: 前者是活引用,后者是序列化结果。openjiuwen 的 `setup_agent` 在本进程通过 `build_context` 工作;分布式部署时通过 `seed` 重建。
- **方法 vs 字段**: `resolve_member_skill_visibility_path` 和 `resolve_member_work_dir` 必须用方法,因为它们依赖 per-member 数据(`member_name`/`workspace` 由 `setup_agent.derive()` 在调用时填入),无法预先在 to_seed 中序列化。

### 1.3 TeamManager 的编排主循环

**核心代码路径**:
- `agents/harness/team/team_manager.py::TeamManager` (L316-2929)
- 关键方法: `broadcast_event` (L440)、`begin_round`/`release_round` (L581/611)、`abort_round` (L643)
- 配置加载: `_load_team_spec` (L875)、`_lookup_bound_team_identity`

**关键函数签名与代码片段**:

```python
# agents/harness/team/team_manager.py (L316)
class TeamManager:
    """Manage team instances across sessions."""

    def __init__(self):
        self._team_agents: dict[str, TeamAgent] = {}
        self._runner_team_agents: dict[str, TeamAgent] = {}
        self._team_monitors: dict[str, TeamMonitorHandler] = {}
        self._stream_tasks: dict[str, asyncio.Task] = {}
        self._bootstrap_lock = asyncio.Lock()
        # ... 几十个字典用于 round/state 跟踪
        # session_id → (request_id, asyncio.Queue) waiters
        self._pending_waiters: dict[str, list[tuple[str, asyncio.Queue]]] = {}
        # 唯一被准入的活跃 round;负责 admission release 直到 terminal/cancel
        self._active_rounds: dict[str, _ActiveTeamRound] = {}
        # ... cron 协作完成状态、evolution watcher、rail context 等

    async def broadcast_event(self, session_id, event):
        """带背压的事件广播;尊重 exclusive waiter。
        终端事件 (chat.processing_status is_processing=False is_complete=True)
        在广播后调用 finish_round,保证 admission 在终端事件之后再释放。
        """
        event_type = str(event.get("event_type") or "")
        terminal = (
            event_type == "chat.processing_status"
            and event.get("is_processing") is False
            and event.get("is_complete") is True
        )
        current_round = self._active_rounds.get(session_id)
        if terminal and current_round is not None and not current_round.terminal_armed:
            # 旧 round 释放前的重复终端帧要丢弃,防止新 exclusive waiter 收到
            logger.debug(...)
            return
        if not terminal and current_round is not None:
            current_round.terminal_armed = True

        waiters = list(self._pending_waiters.get(session_id, ()))
        exclusive_request_id = self._exclusive_waiters.get(session_id)
        if exclusive_request_id is not None:
            # exclusive round 只接收自己的事件
            waiters = [(rid, q) for rid, q in waiters if rid == exclusive_request_id]

        async def _put_to_waiter(request_id, queue):
            """bounded put + 周期性 recheck 避免 disconnected waiter 阻塞"""
            queued_event = dict(event)
            while any(rid == request_id and registered_queue is queue
                      for rid, registered_queue in self._pending_waiters.get(session_id, ())):
                try:
                    await asyncio.wait_for(queue.put(queued_event),
                                           timeout=_WAITER_PUT_RECHECK_TIMEOUT_SEC)
                    break
                except asyncio.TimeoutError:
                    continue

        await asyncio.gather(*(_put_to_waiter(req, q) for req, q in waiters))

        if terminal:
            await self.finish_round(session_id)
        elif current_round is not None and not current_round.defer_terminal_release:
            self._observe_interactive_round_event(session_id, current_round, event)

    def begin_round(self, session_id, request_id, *, release_admission=None,
                    defer_terminal_release=False, terminal_armed=False):
        """绑定唯一被准入的 round 到它的 request_id。
        每个 session 同一时间只允许一个 round;后续 request 撞上时会抛
        RuntimeError。
        """
        current = self._active_rounds.get(session_id)
        if current is not None:
            raise RuntimeError(
                f"team round already active for session {session_id}: "
                f"{current.request_id}"
            )
        self._active_rounds[session_id] = _ActiveTeamRound(
            request_id=request_id,
            release_admission=release_admission,
            defer_terminal_release=defer_terminal_release,
            terminal_armed=terminal_armed,
        )

    async def abort_round(self, session_id, request_id) -> bool:
        """取消自动化 round 之前停止其所有权。
        Agent-core 暴露的是 runtime stop 而非每交互 abort。
        停止保留持久化 Team 状态;后续请求会冷恢复并收不到幽灵输出。
        """
        async with self._get_lifecycle_lock(session_id):
            if not self.is_round_owner(session_id, request_id):
                return False
            team_name = self._resolve_session_team_name(session_id)
            if team_name:
                try:
                    await Runner.stop_agent_team(team_name=team_name,
                                                 session_id=session_id)
                except Exception as exc:
                    logger.warning("[TeamManager] heartbeat round stop failed; ...")
            await self._cleanup_runtime_locals(session_id)
            await self._stop_runner_team_agent_transport(session_id)
            self.clear_active_runtime(session_id)
            self.clear_pending_runtime(session_id)
            self._clear_terminal_session_markers(session_id)
            await self.release_round(session_id, request_id)
            return True
```

**Round 终态判定**(分散在 `common/cron_team_completion.py`):

```python
# common/cron_team_completion.py (L191)
def apply_cron_team_round_event(state, event):
    """从 team 流事件更新 round 完成状态。
    决策路径:
    - workflow.updated → workflow_started=True;若 status==completed → workflow_completed=True
    - team.task.created → tasks_ever_created=True;open_team_tasks[task_id]=True
      同时清掉 leader_final_seen,让 cron 等待 post-delegation final
    - team.task.completed → 从 open_team_tasks 移除
    - team.member.status_changed → active_team_members 跟踪 busy/working/starting ↔ ready/idle/stopped
    """
    event_type = str(event.get("event_type") or "").strip()
    if event_type == "workflow.updated":
        workflow = event.get("workflow")
        if isinstance(workflow, dict):
            state["workflow_started"] = True
            status = str(workflow.get("status") or "").strip().lower()
            if status == "completed":
                state["workflow_completed"] = True

# 终态判定(L117)
def cron_team_round_should_end(state, *, chunk_complete=False) -> bool:
    if chunk_complete:
        if state.get("workflow_completed") and state.get("leader_final_after_workflow"):
            return True
        if state.get("team_round_completed") and cron_team_round_has_result_text(state):
            leader = str(state.get("leader_text") or "").strip()
            if leader and is_cron_leader_placeholder_text(leader):
                return False
            return True
        return _harness_round_can_end(state)
    ...

def _harness_round_can_end(state):
    leader = str(state.get("leader_text") or "").strip()
    return (
        not state.get("workflow_started")
        and not state.get("workflow_completed")
        and not cron_team_round_has_open_tasks(state)
        and not cron_team_round_has_active_members(state)
        and state.get("leader_final_seen")
        and bool(leader)
        and not is_cron_leader_placeholder_text(leader)
    )
```

**对 laew 借鉴**:
1. **属性/环境分离 + 序列化重建**: `params` 换请求不变,`BuildContext` 随会话变。laew 的 Yolo / Plan / Main-Work / SubAgent-Work 可以采用相同的"声明式 Spec + 跨边界 seed"模型,为将来分布式执行铺路。
2. **Round 准入与 exclusive waiter**: laew 当前 Quality-Check 是同步的,AgentMemory 是异步但没有显式 round admission;借鉴 `_active_rounds` 可让 TUI/CLI/Web 的多端请求在共享 round 上不打架。
3. **leader final 占位符检测**: `_cron_solo_harness_end_pending` 用来判断"harness 风格完成但任务可能还在委托"的临界窗口,避免 race condition。这是 laew 目前没有的——尤其 Quality-Check 同步完成时缺少"还在等异步反馈"的窗口检测。

---

## 2. 协议矩阵核心代码路径

JiuwenSwarm 在传输层有 **4 种协议**:E2A(External↔Agent WebSocket)、A2A(Agent-to-Agent)、ACP(Agent Communication Protocol stdio JSON-RPC)、A2UI(Agent-to-UI Tagged JSON)。

### 2.1 E2A 的编解码函数

**核心代码路径**:
- 入口: `common/e2a/wire_codec.py`(整文件 ~445 行)
- 模型: `common/e2a/models.py::E2AResponse`
- 归一化: `common/e2a/gateway_normalize.py`

**关键函数签名与代码片段**:

```python
# common/e2a/wire_codec.py (L80)
def is_e2a_response_wire_dict(data: dict) -> bool:
    """判别 JSON 是否为 E2A 响应线格式(与 E2AEnvelope 区分:须含非空 response_kind)。"""
    if not isinstance(data, dict) or data.get("type") == "event":
        return False
    if data.get("protocol_version") != E2A_PROTOCOL_VERSION:
        return False
    rk = data.get("response_kind")
    return isinstance(rk, str) and bool(rk.strip())

def parse_agent_server_wire_unary(data: dict) -> AgentResponse:
    """入站 WebSocket 非流式 JSON → AgentResponse。
    E2A 失败时回退到 metadata.legacy 字段,deprecation shape 也兼容。
    """
    rid = str(data.get("request_id", ""))
    if is_e2a_response_wire_dict(data):
        try:
            e2a = E2AResponse.from_dict(dict(data))
        except Exception as e:
            logger.exception("[E2A][wire][in][FAIL] stage=from_dict unary request_id=%s err=%s", rid, e)
            raise
        meta = dict(e2a.metadata or {})
        legacy = meta.get(E2A_WIRE_LEGACY_AGENT_RESPONSE_KEY)
        if legacy is not None and isinstance(legacy, dict):
            logger.warning("[E2A][wire][in][fallback] unary request_id=%s legacy_key=%s ...", rid, ...)
            return _raw_dict_to_agent_response(legacy)   # 整包 legacy 回退
        try:
            out = e2a_response_to_agent_response(e2a)
            return out
        except Exception as e:
            # 二次回退:从 metadata 拆 legacy
            legacy_inv = meta.get(E2A_WIRE_LEGACY_AGENT_RESPONSE_KEY)
            if isinstance(legacy_inv, dict):
                return _raw_dict_to_agent_response(legacy_inv)
            raise
    if _deprecated_unary_shape(data):
        logger.warning("[E2A][wire][in][deprecated_legacy_shape] unary request_id=%s ...", rid, ...)
        return _raw_dict_to_agent_response(data)
    raise ValueError(f"parse_agent_server_wire_unary: unrecognized wire shape keys={list(data.keys())[:32]}")

def encode_agent_response_for_wire(resp, *, response_id, sequence=0) -> dict:
    """AgentResponse → E2A 线 dict;失败时 metadata 塞入整包 legacy。
    双层 fallback:to_dict 失败 → envelope 包 E2A error;整层 encode 失败 → legacy 包 fallback。
    """
    rid = resp.request_id
    try:
        e2a = e2a_response_from_agent_response(resp, response_id=response_id, sequence=sequence)
        try:
            wire = e2a.to_dict()
        except Exception as te:
            logger.exception("[E2A][wire][out][FAIL] stage=to_dict unary request_id=%s ...", rid, ...)
            return _fallback_wire_unary_from_legacy(_json_safe(asdict(resp)), response_id=response_id,
                                                    sequence=sequence, exc=te)
        logger.info("[E2A][wire][out] unary request_id=%s response_id=%s response_kind=%s legacy_stashed=false",
                    rid, response_id, e2a.response_kind)
        return _json_safe(wire)
    except Exception as e:
        logger.exception("[E2A][wire][out][FAIL] stage=encode unary request_id=%s ... legacy_stashed=true", rid, ...)
        return _fallback_wire_unary_from_legacy(_json_safe(asdict(resp)), response_id=response_id,
                                                sequence=sequence, exc=e)
```

**legacy 兜底机制要点**:
- `_fallback_wire_unary_from_legacy` / `_fallback_wire_chunk_from_legacy` 把失败的 legacy 整包塞到 `metadata[E2A_WIRE_LEGACY_AGENT_RESPONSE_KEY]` 中,并发一个 `status=FAILED`、`response_kind=E2A_RESPONSE_KIND_E2A_ERROR` 的 envelope,接收端再倒出来。

### 2.2 ACP stdio client 的读写循环

**核心代码路径**:
- `acp/stdio_client.py::AcpStdioClient`(整文件 ~671 行)

**关键函数签名与代码片段**:

```python
# acp/stdio_client.py (L219)
class AcpStdioClient:
    """Spawn an ACP-compatible agent and exchange JSON-RPC 2.0 over subprocess stdio."""

    def __init__(self, command, args=None, cwd=None, env=None):
        self._command = (command or "").strip()
        self._args = list(args or [])
        self._cwd = cwd if cwd else None
        self._env = _merge_env(env)
        self._proc: asyncio.subprocess.Process | None = None
        self._session_id: str | None = None
        self._next_rpc_id = 1
        self._stderr_task: asyncio.Task[None] | None = None
        self._closed = False
        self._stdout_buf = ""                # 增量 JSON 缓冲(支持多行 pretty-printed)

    async def _read_one_message(self, timeout: float) -> dict | None:
        """读取 1 个 JSON-RPC 消息。stderr drain 在 _task_drain_stderr 并行。
        多行 pretty-printed JSON 由 json.JSONDecoder.raw_decode 处理,buffer 残留给下轮。
        """
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            while True:
                try:
                    obj, self._stdout_buf = _consume_one_json(self._stdout_buf)
                except RuntimeError as exc:
                    logger.error("%s", exc)
                    raise
                if obj is None:
                    break
                if not isinstance(obj, dict):
                    logger.warning("[AcpStdioClient] dropping non-object JSON-RPC message: %s", type(obj).__name__)
                    continue
                return obj
            now = asyncio.get_event_loop().time()
            if now >= deadline:
                return None
            remain = max(0.1, deadline - now)
            try:
                chunk = await asyncio.wait_for(
                    self._proc.stdout.read(_STDOUT_READ_CHUNK),  # 64KB 默认
                    timeout=remain,
                )
            except asyncio.TimeoutError:
                continue
            if not chunk:
                if self._stdout_buf.strip():
                    logger.warning("[AcpStdioClient] EOF while partial JSON buffered (%s bytes)", len(self._stdout_buf))
                return None
            self._stdout_buf += chunk.decode("utf-8", errors="replace")

    async def _rpc_call(self, method, params, *, timeout=120.0):
        """JSON-RPC 同步调用:写到 stdin,等待带 id 的响应。
        同时处理:
        - session/update(服务端推送,不匹配,继续轮询)
        - 客户端发出的请求(session/request_permission/fs/* 等)— 响应以避免阻塞
        - 错误消息 → RuntimeError
        """
        rid = self._next_id()
        payload = {"jsonrpc": "2.0", "id": rid, "method": method, "params": params}
        line = json.dumps(payload, ensure_ascii=False) + "\n"
        self._proc.stdin.write(line.encode("utf-8"))
        await self._proc.stdin.drain()

        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            remain = max(0.1, deadline - asyncio.get_event_loop().time())
            msg = await self._read_one_message(remain)
            if msg is None:
                proc = self._proc
                if proc is not None and proc.returncode is not None:
                    raise RuntimeError(f"ACP agent process exited ({proc.returncode}) while waiting for {method}.")
                raise RuntimeError(f"ACP agent closed stream or timed out waiting for {method} response")
            if msg.get("method") == "session/update":
                continue
            if _is_peer_jsonrpc_request(msg):
                await self._handle_peer_request(msg)
                continue
            if str(msg.get("id")) == str(rid):
                if "error" in msg:
                    raise RuntimeError(_jsonrpc_error_message(msg["error"]))
                return msg.get("result")

    async def _handle_peer_request(self, msg):
        """响应 agent 主动发的请求,避免阻塞 agent。
        - session/request_permission → 自动 allow-once(可配 _AUTO_APPROVE)
        - fs/read_text_file → 检查 path 必须落在 cwd 下(realpath 防 traversal)
        - fs/write_text_file → 同上,创建父目录
        - 其他 → -32601 unsupported method
        """
        req_id = msg.get("id")
        method = str(msg.get("method") or "").strip()

        if method == "session/request_permission":
            if _AUTO_APPROVE:
                await self._send_json_rpc_response(
                    req_id,
                    result={"outcome": {"outcome": "selected", "optionId": "allow-once"}},
                )
            else:
                await self._send_json_rpc_response(
                    req_id,
                    result={"outcome": {"outcome": "cancelled"}},
                )
            return
        if method == "fs/read_text_file":
            params = msg.get("params")
            raw_path = params.get("path")
            root = _resolved_session_root(self._cwd)
            abs_path = _resolved_path_inside_root(root, raw_path)   # 防 traversal
            if abs_path is None:
                await self._send_json_rpc_response(req_id, error={"code": -32001, "message": "path denied or invalid"})
                return
            ...
        if method == "fs/write_text_file":
            # 同样校验路径
            ...

    async def chat(self, message, *, timeout=None) -> str:
        """session/prompt 客户端;累积 session/update 文本,直到拿到带 id 的响应。
        """
        t_out = timeout if timeout is not None else _DEFAULT_CHAT_TIMEOUT  # 默认 600s
        rid = self._next_id()
        params = {"sessionId": self._session_id,
                  "prompt": [{"type": "text", "text": message}]}
        payload = {"jsonrpc": "2.0", "id": rid, "method": "session/prompt", "params": params}
        ...
        parts: list[str] = []
        deadline = asyncio.get_event_loop().time() + t_out
        while True:
            ...
            if msg.get("method") == "session/update":
                chunk = _extract_session_update_text(msg)   # 抽取 update.content.text
                if chunk:
                    parts.append(chunk)
                continue
            if _is_peer_jsonrpc_request(msg):
                await self._handle_peer_request(msg)
                continue
            if str(msg.get("id")) == str(rid):
                if "error" in msg:
                    raise RuntimeError(_jsonrpc_error_message(msg["error"]))
                return "".join(parts).strip() or _result_text_fallback(msg.get("result"))

    async def close(self):
        """分级关闭:close stdin → SIGTERM 5s → SIGKILL 8s → 排空管道。
        POSIX 上 killpg(_KILL_PG 默认 True)杀整个进程组,防止子进程孤立。
        """
        ...
        if proc.stdin is not None:
            proc.stdin.close()
            try:
                await asyncio.wait_for(proc.stdin.wait_closed(), timeout=_CLOSE_STDIN_WAIT_S)  # 4s
            ...
        if proc.returncode is None:
            _signal_process_leader(proc, brutal=False)
            await asyncio.wait_for(proc.wait(), timeout=_CLOSE_TERM_WAIT_S)  # 5s
        if proc.returncode is None:
            _signal_process_leader(proc, brutal=True)
            await asyncio.wait_for(proc.wait(), timeout=_CLOSE_KILL_WAIT_S)  # 8s
        ...
        await _bounded_drain_reader(proc.stdout, _CLOSE_DRAIN_PIPE_S)
        await _bounded_drain_reader(proc.stderr, _CLOSE_DRAIN_PIPE_S)
```

### 2.3 A2UI 的 Tagged JSON 序列化

**核心代码路径**:
- 协议: `server/runtime/a2ui/protocol.py::A2UIProtocolSpec`(整文件 ~32KB)
- 解析器: `server/runtime/a2ui/parser.py::parse_a2ui_response`
- 解析器辅助: `iter_tagged_block_bodies` / `strip_tagged_a2ui_blocks` / `parse_raw_json` / `parse_jsonl`

**关键函数签名与代码片段**:

```python
# server/runtime/a2ui/protocol.py (L36)
A2UI_ACTIVE_PROTOCOL_VERSION = VERSION_0_8
A2UI_CLIENT_EVENT_TYPE = "a2ui.client_event"

class A2UIProtocolSpec:
    """Versioned A2UI protocol adapter.
    注册表目前只有 v0.8;新版本应新增 A2UIProtocolSpec 实例而非修改调用点。
    """

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

    def build_prompt(self, language="en", *, include_browser_workflows=False) -> str:
        """生成系统提示(包含 schema);基于语言分流。
        关键约束:
        - 每个 A2UI block 必须用 OPEN_TAG/CLOSE_TAG 包装
        - block 内必须是 JSON list,每个元素是 server-to-client message
        - 必须先 beginRendering,再 surfaceUpdate
        - 数据绑定按需 dataModelUpdate
        - 父组件先于子组件
        """
        ...
        prompt = self.schema_manager.generate_system_prompt(
            role_description=role,
            workflow_description=workflow,
            ui_description=ui,
            include_schema=True,
            include_examples=False,
            validate_examples=True,
        )
        prompt = _normalize_prompt_contract(prompt)
        examples = self.render_examples(validate=True)
        if examples:
            prompt = f"{prompt}\n\n### Examples:\n{examples}"
        return prompt

    @staticmethod
    def parse_response(content: str) -> list[A2UIResponsePart]:
        return parse_a2ui_response(content)

    def validate_messages(self, messages: list[dict[str, Any]]) -> None:
        validate_a2ui_messages(self.catalog, messages)
```

**Tagged JSON 解析器**:

```python
# server/runtime/a2ui/parser.py (L34)
def iter_tagged_block_bodies(text: str) -> list[tuple[int, str]]:
    """扫描 OPEN_TAG..CLOSE_TAG 包裹的 body,带 pair index。"""
    blocks: list[tuple[int, str]] = []
    cursor = 0
    while True:
        start = text.find(A2UI_OPEN_TAG, cursor)
        if start < 0:
            return blocks
        body_start = start + len(A2UI_OPEN_TAG)
        end = text.find(A2UI_CLOSE_TAG, body_start)
        if end < 0:
            blocks.append((len(blocks), text[body_start:]))
            return blocks
        blocks.append((len(blocks), text[body_start:end]))
        cursor = end + len(A2UI_CLOSE_TAG)

def strip_tagged_a2ui_blocks(text: str) -> str:
    """剥掉所有 tagged blocks,只留纯文本。用于文本渠道 fallback。"""
    output: list[str] = []
    cursor = 0
    while True:
        start = text.find(A2UI_OPEN_TAG, cursor)
        if start < 0:
            output.append(text[cursor:])
            break
        output.append(text[cursor:start])
        end = text.find(A2UI_CLOSE_TAG, start + len(A2UI_OPEN_TAG))
        if end < 0:
            break
        cursor = end + len(A2UI_CLOSE_TAG)
    return "".join(output).strip()

def parse_raw_json(text: str) -> list[dict[str, Any]] | None:
    """整段 raw JSON(以 [ 或 { 开头)。"""
    stripped = text.strip()
    if not stripped.startswith(("[", "{")):
        return None
    try:
        return coerce_message_list(json.loads(stripped))
    except json.JSONDecodeError:
        return None

def parse_jsonl(text: str) -> list[dict[str, Any]] | None:
    """JSONL:每行一个 {..} A2UI message。"""
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if not lines or not all(line.startswith("{") and line.endswith("}") for line in lines):
        return None
    messages: list[dict[str, Any]] = []
    try:
        for line in lines:
            parsed = json.loads(line)
            if not is_a2ui_message(parsed):
                return None
            messages.append(parsed)
    except json.JSONDecodeError:
        return None
    return messages

def parse_a2ui_response(content: str) -> list[A2UIResponsePart]:
    """主入口:JSONL → raw JSON → tagged 三级 fallback。
    最终落到:若三种都不是,返回单 text part。
    """
    text = content or ""
    if not text.strip():
        return []
    jsonl_messages = parse_jsonl(text)
    if jsonl_messages is not None:
        return [A2UIResponsePart(kind="a2ui", messages=jsonl_messages)]
    raw_json_messages = parse_raw_json(text)
    if raw_json_messages is not None:
        return [A2UIResponsePart(kind="a2ui", messages=raw_json_messages)]
    parts: list[A2UIResponsePart] = []
    for part in parse_tagged_response(text):
        part_text = (part.text or "").strip()
        if part_text:
            parts.append(A2UIResponsePart(kind="text", text=part_text))
        messages = coerce_message_list(part.a2ui_json)
        if messages is not None:
            parts.append(A2UIResponsePart(kind="a2ui", messages=messages))
    return parts or [A2UIResponsePart(kind="text", text=text)]
```

**对 laew 借鉴**:
1. **Wire Codec 双层 legacy 兜底**: laew 的 `llm/anthropic.rs` 与 `llm/openai.rs` 已经实现协议隔离,但没有 wire-level legacy 兜底机制。在版本升级时借鉴 `metadata.{legacy_key}` 整包塞回的做法,可让 laew 升级不破坏老客户端。
2. **ACP JSON-RPC over stdio**: laew 是单进程 CLI,目前没有外部 agent 接入协议;如果未来要支持 Skill Marketplace / 第三方 agent 协作,ACP 这种成熟的 JSON-RPC over stdio(增量缓冲 + 自动权限应答)是最直接的方案。
3. **A2UI Tagged JSON 三级 fallback**: laew TUI 当前只有 `chat.final` 文本通道,如果要支持富交互(按钮/确认),可借鉴 A2UI 的 tagged block 格式,只在 TUI 渲染时按块切分。

---

## 3. SkillDevPipeline 核心代码路径

### 3.1 12 阶段状态机的转移函数

**核心代码路径**:
- 入口: `server/runtime/skill/skilldev/pipeline.py::SkillDevPipeline`(整文件 195 行)
- 数据模型: `server/runtime/skill/skilldev/schema.py::SkillDevStage` / `SUSPENSION_POINTS`
- 上下文: `server/runtime/skill/skilldev/context.py::SkillDevContext`

**完整阶段图**(schema.py L24-55):

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
# (*) = 挂起点
```

**关键函数签名与代码片段**:

```python
# server/runtime/skill/skilldev/pipeline.py (L47)
class SkillDevPipeline:
    """SkillDev 确定性状态机。
    生命周期:每次请求创建 → run()/resume() 执行 → checkpoint → 对象释放。
    不长驻内存,不持有 JiuWenSwarm 实例。
    """

    # 阶段 → Handler 注册表
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

    def __init__(self, task_id, state, deps):
        self.task_id = task_id
        self.state = state
        self._deps = deps
        self._event_queue = asyncio.Queue()

    async def run(self) -> AsyncIterator[SkillDevEvent]:
        """从当前阶段开始执行,直到遇到挂起点或终态。
        核心循环:
        1) 若当前 stage 在 SUSPENSION_POINTS,emit CONFIRM_REQUEST → checkpoint → break
        2) 找到 handler,execute,转 next_stage
        3) 每个阶段 emit STAGE_CHANGED + TODOS_UPDATE
        4) 阶段边界调 _checkpoint (持久化 + 同步 workspace)
        """
        while self.state.stage not in (SkillDevStage.COMPLETED, SkillDevStage.ERROR):
            if self.state.stage in SUSPENSION_POINTS:
                suspension = SUSPENSION_POINTS[self.state.stage]
                await self._emit(SkillDevEventType.TODOS_UPDATE,
                                 {"todos": compute_todos(self.state.stage, self.state.mode)})
                await self._emit(
                    SkillDevEventType.CONFIRM_REQUEST,
                    {
                        "confirm_type": suspension.confirm_type,
                        "title": suspension.title,
                        "message": suspension.message,
                        "data": suspension.extract_data(self.state),
                        "actions": suspension.actions,
                    },
                )
                await self._checkpoint()
                break

            handler_cls = self.STAGE_HANDLERS.get(self.state.stage)
            if handler_cls is None:
                raise RuntimeError(f"阶段 {self.state.stage} 没有对应的处理器")

            workspace = await self._deps.workspace_provider.ensure_local(self.task_id)
            ctx = SkillDevContext(
                task_id=self.task_id, deps=self._deps, state=self.state,
                workspace=workspace, event_queue=self._event_queue,
            )

            await self._emit(SkillDevEventType.STAGE_CHANGED,
                             {"stage": self.state.stage.value, "iteration": self.state.iteration})
            await self._emit(SkillDevEventType.TODOS_UPDATE,
                             {"todos": compute_todos(self.state.stage, self.state.mode)})

            try:
                handler = handler_cls()
                result = await handler.execute(ctx)
                self.state.stage = result.next_stage
                await self._checkpoint()
            except Exception as exc:
                logger.exception("[Pipeline] 阶段 %s 执行失败: %s", self.state.stage.value, exc)
                self.state.stage = SkillDevStage.ERROR
                self.state.error = str(exc)
                await self._emit(SkillDevEventType.ERROR, {"message": str(exc)})
                await self._checkpoint()
                break

        while not self._event_queue.empty():
            yield self._event_queue.get_nowait()

    async def resume(self, data: dict) -> AsyncIterator[SkillDevEvent]:
        """从挂起点恢复。
        REVIEW 的 next_stage 是函数,根据 action 动态决定(improve vs package);
        PLAN_CONFIRM/DESC_OPTIMIZE_CONFIRM 是固定 stage。
        """
        current_stage = self.state.stage
        if current_stage not in SUSPENSION_POINTS:
            raise ValueError(f"阶段 {current_stage} 不是挂起点,无法调用 resume()")

        suspension = SUSPENSION_POINTS[current_stage]
        suspension.on_resume(self.state, data)
        next_stage = suspension.next_stage
        if callable(next_stage):
            next_stage = next_stage(data)
        self.state.stage = next_stage

        async for event in self.run():
            yield event

    async def _emit(self, event_type, payload):
        """向事件队列写入一个事件。"""
        event = SkillDevEvent(event_type=event_type,
                              payload={"task_id": self.task_id, **payload},
                              task_id=self.task_id)
        await self._event_queue.put(event)

    async def _checkpoint(self):
        """阶段边界:持久化状态 + 同步 workspace 文件。"""
        await self._deps.state_store.save_state(self.task_id, self.state)
        await self._deps.workspace_provider.sync_to_remote(self.task_id)
```

### 3.2 3 个 HITL 挂起点的实现代码

**核心代码路径**:
- 配置: `server/runtime/skill/skilldev/schema.py::SuspensionConfig` + `SUSPENSION_POINTS` (L218-332)

**关键函数签名与代码片段**:

```python
# server/runtime/skill/skilldev/schema.py (L218)
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
    actions: list[dict[str, str]]  # [{"id": "confirm", "label": "确认", "style": "primary"}]
    extract_data: Callable    # (state) → dict,从 state 提取展示给前端的数据
    on_resume: Callable       # (state, data) → None,根据用户响应更新 state
    next_stage: SkillDevStage | Callable  # 下一阶段(可以是函数)

def _plan_extract_data(state: SkillDevState) -> dict:
    return {"plan": state.plan}

def _plan_confirm_on_resume(state, data):
    if "plan" in data:
        state.plan = data["plan"]
    state.plan_confirmed_at = _now_iso()

def _review_extract_data(state):
    return {
        "benchmark": (state.eval_results or {}).get("benchmark"),
        "report": (state.eval_results or {}).get("report"),
        "iteration": state.iteration,
    }

def _review_on_resume(state, data):
    if data.get("feedback"):
        state.feedback_history.append({
            "iteration": state.iteration,
            "feedback": data["feedback"],
        })

def _review_next_stage(data) -> SkillDevStage:
    """动态决定 REVIEW 后是 IMPROVE 还是 PACKAGE。"""
    action = data.get("action", "improve")
    return SkillDevStage.IMPROVE if action == "improve" else SkillDevStage.PACKAGE

def _desc_opt_extract_data(state):
    plan = state.plan or {}
    return {"current_description": plan.get("description", "")}

def _desc_optimize_confirm_next_stage(data) -> SkillDevStage:
    action = data.get("action", "skip")
    return (SkillDevStage.DESC_OPTIMIZE if action == "optimize"
            else SkillDevStage.COMPLETED)

SUSPENSION_POINTS: dict[SkillDevStage, SuspensionConfig] = {
    SkillDevStage.PLAN_CONFIRM: SuspensionConfig(
        confirm_type="plan_confirm",
        title="请审阅开发计划",
        message="以下是生成的开发计划,请确认或修改",
        actions=[
            {"id": "confirm", "label": "确认", "style": "primary"},
            {"id": "modify", "label": "修改", "style": "secondary"},
        ],
        extract_data=_plan_extract_data,
        on_resume=_plan_confirm_on_resume,
        next_stage=SkillDevStage.GENERATE,
    ),
    SkillDevStage.REVIEW: SuspensionConfig(
        confirm_type="review",
        title="评测结果审阅",
        message="请审阅评测结果并决定下一步",
        actions=[
            {"id": "accept", "label": "通过,进入打包", "style": "primary"},
            {"id": "improve", "label": "继续改进", "style": "secondary"},
        ],
        extract_data=_review_extract_data,
        on_resume=_review_on_resume,
        next_stage=_review_next_stage,
    ),
    SkillDevStage.DESC_OPTIMIZE_CONFIRM: SuspensionConfig(
        confirm_type="desc_optimize_confirm",
        title="描述优化",
        message="Skill 已打包完成。是否需要优化触发描述以提高触发准确率?",
        actions=[
            {"id": "optimize", "label": "优化", "style": "primary"},
            {"id": "skip", "label": "跳过", "style": "secondary"},
        ],
        extract_data=_desc_opt_extract_data,
        on_resume=_desc_optimize_confirm_on_resume,
        next_stage=_desc_optimize_confirm_next_stage,
    ),
}
```

### 3.3 Skill 自进化的触发和更新逻辑

**核心代码路径**:
- Skill Evolution Rail: `agents/swarm/providers/evolution_rails.py`
- 进化事件: `symphony/evolution/service.py::record_plan_outcome` / `rebuild_dynamic_overlay`
- 覆盖层加载: `symphony/evolution/service.py::load_dynamic_overlay`

**关键函数签名与代码片段**:

```python
# symphony/evolution/service.py (L45)
def load_dynamic_overlay(graph_dir) -> dict | None:
    """加载当前动态 overlay;若 event/overlay 元数据不一致则重建。
    核心路径:
    1) prepare_evolution_store(必要时从 legacy evolution/ 导入到 graph/evolution/)
    2) read_overlay + read_events
    3) _overlay_requires_rebuild → rebuild_dynamic_overlay
    """
    graph_root = prepare_evolution_store(graph_dir)
    with evolution_store_transaction():
        overlay = read_overlay(graph_root)
        events = read_events(graph_root)
        current_version = _current_graph_version(graph_root)
        if _overlay_requires_rebuild(overlay, events, current_version):
            return rebuild_dynamic_overlay(graph_root,
                                         base_graph_version=current_version)
        return overlay

def _overlay_requires_rebuild(overlay, events, current_version) -> bool:
    """overlay 元数据与 events 不一致时返回 True:
    - 没有 events → 不需要重建
    - overlay 不存在或 schema 不匹配 → 重建
    - overlay.events.count != len(events) → 重建
    - overlay.events.last_event_at != _last_event_ts(events) → 重建
    - overlay.base_graph_version != current_version → 重建
    """
    if not events:
        return False
    if overlay is None or overlay.get("schema_version") != OVERLAY_SCHEMA_VERSION:
        return True
    overlay_events = overlay.get("events")
    if not isinstance(overlay_events, dict):
        return True
    event_metadata_matches = (
        overlay_events.get("count") == len(events)
        and str(overlay_events.get("last_event_at") or "")
        == _last_event_ts(events)
    )
    graph_version_matches = (
        not current_version
        or str(overlay.get("base_graph_version") or "") == current_version
    )
    return not event_metadata_matches or not graph_version_matches

def rebuild_dynamic_overlay(graph_dir, *, base_graph_version=None) -> dict:
    """从 events.jsonl 重建并持久化 overlay。"""
    graph_dir = prepare_evolution_store(graph_dir)
    if base_graph_version is None:
        base_graph_version = _current_graph_version(graph_dir)
    with evolution_store_transaction():
        events = read_events(graph_dir)
        overlay = build_overlay_from_events(
            events, base_graph_version=base_graph_version,
        )
        write_overlay(graph_dir, overlay)
    return overlay

def record_plan_outcome(graph_dir, *, plan_id, outcome,
                        selected_skill_ids=None, selected_edges=None,
                        failed_edges=None, missing_inputs=None,
                        failure_attribution="", failure_type="", detail="",
                        evidence_id="", session_id="", request_id="",
                        rebuild_overlay=True) -> dict:
    """追加一个 plan outcome event,可选刷新 overlay。
    按 evidence_id 去重(同一 plan 多次失败不重复计数)。
    """
    graph_dir = prepare_evolution_store(graph_dir)
    normalized_outcome = normalize_outcome(outcome)
    normalized_edges = _annotate_failed_edges(
        normalize_edges(selected_edges or []),
        failed_edges=failed_edges or [],
    )
    attribution = _default_failure_attribution(
        normalized_outcome, normalized_edges, failure_attribution,
    )
    event = _base_event(PLAN_OUTCOME, plan_id=plan_id, query=query)
    event.update({
        "outcome": normalized_outcome,
        "failure_type": str(failure_type or "").strip(),
        "failure_attribution": attribution,
        "detail": str(detail or "").strip()[:1000],
        "selected_skill_ids": _clean_skill_ids(selected_skill_ids or []),
        "selected_edges": normalized_edges,
        "missing_inputs": [item for item in (missing_inputs or []) if isinstance(item, dict)],
    })
    ...
    with evolution_store_transaction():
        if clean_evidence_id:
            existing = next(
                (item for item in read_events(graph_dir)
                 if str(item.get("evidence_id") or "") == clean_evidence_id),
                None,
            )
            if existing is not None:
                return {**existing, "deduplicated": True}
        append_event(graph_dir, event)
        if rebuild_overlay:
            rebuild_dynamic_overlay(graph_dir)
    return event
```

**对 laew 借鉴**:
1. **声明式 stage 注册表**: laew 的"Yolo 入口层"目前是硬编码分类(simple/medium/hard),如果支持 skill marketplace,可以借鉴 `STAGE_HANDLERS` 注册表 + `SuspensionConfig` 让新 skill 类型由声明式 metadata 驱动,无需改主代码。
2. **挂起点的 `extract_data + on_resume + next_stage` 三段式**: 比单一 `confirm()` 灵活得多,适合复杂 skill dev。laew 可以借鉴用于 hard 任务的"Plan Agent 产出方案 → 用户确认 → Main-Work 执行"流程。
3. **证据 ID 去重**: `record_plan_outcome` 通过 `evidence_id` 防止同一 plan 失败被多次计入,这避免了重试风暴污染统计。laew 的 AgentMemory 没有这种去重,借鉴可让 Skill 评级更稳定。

---

## 4. SwarmFlow 核心代码路径

### 4.1 DAG 构建函数

**核心代码路径**:
- Workflow 状态: `agents/harness/team/handlers/workflow_state.py::WorkflowRunState`(整文件 ~1390 行)
- 状态应用: `apply(progress)` → kind dispatcher

**关键函数签名与代码片段**:

```python
# agents/harness/team/handlers/workflow_state.py (L335)
def apply(self, progress: WorkflowProgress) -> Optional[dict[str, Any]]:
    """应用一个 progress event,更新状态,返回增量 dict。
    返回 None 表示无需 push(log、未知 kind 等)。
    """
    kind = progress.kind
    handler = self._KIND_HANDLERS.get(kind)
    if handler is None:
        return None
    method = getattr(self, handler)
    return method(progress)

# kind 派发表(L240)
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

# 阶段切换 DAG(L480)
def _switch_to_phase(self, phase_name, iteration=None):
    """进入 phase_name(running),sealing 之前的 phase。
    Loop-aware:iteration 设置时,每轮迭代一张新卡(round 1/2/3 of phase("生成") → 3 张)。

    Returns: (target_phase, sealed_phase_or_None)
    """
    iter_key = iteration or 1
    target = self._find_phase_by_name(phase_name, iter_key)
    if target is None:
        # Fallback: 把 planned card(iteration=None)的当 iter_key=1
        if iter_key == 1:
            target = self._find_phase_by_name(phase_name, None)
            if target is not None and target.status == "planned":
                target.iteration = 1
        if target is None:
            phase_id = self._generate_phase_id(phase_name)
            target = WorkflowPhaseState(
                id=phase_id, name=phase_name,
                status="running", iteration=iter_key,
            )
            self.phases.append(target)
            logger.warning("[WF_DBG] phase %s#%s not in plan, created on the fly", ...)
            else:
                target.status = "running"
        else:
            target.status = "running"

    sealed = None
    prev = self._last_phase
    can_seal_prev = (
        prev is not None and prev is not target
        and prev.name != phase_name and prev.status == "running"
    )
    if can_seal_prev:
        # 状态镜像 agents 优先级:
        # any stopped → stopped(external termination wins)
        # all failed → failed(每个 agent 都错误)
        # otherwise → completed(至少一个 completed,partial OK)
        statuses = [a.status for a in prev.agents] if prev.agents else []
        if statuses and any(s == "stopped" for s in statuses):
            prev.status = "stopped"
        elif statuses and all(s == "failed" for s in statuses):
            prev.status = "failed"
        else:
            prev.status = "completed"
        self._finalize_running_agents(prev, prev.status)
        sealed = prev
    self._last_phase = target
    return target, sealed
```

### 4.2 HITL 节点的暂停/恢复实现

**核心代码路径**:
- HITL 节点状态: `workflow_state.py::WorkflowAgentState.status = "waiting_for_human"` (L141)
- 暂停处理: `_pause_running_agents` / `_finalize_running_agents` (L386/368)

**关键函数签名与代码片段**:

```python
# workflow_state.py (L386)
def _pause_running_agents(self, phase):
    """Pause 还在跑的 agents(包括 waiting_for_human)到 paused(非终态)。
    与 _finalize_running_agents 不同:不 stamp completed_at / duration_ms —
    paused agent 没完成,resume 时会重新激活。

    waiting_for_human 也被 paused(它的 pending reply 被 pause 的中止取消),
    前端不会一直转圈。
    """
    for agent in phase.agents:
        if agent.status in ("running", "waiting_for_human"):
            agent.status = "paused"
    self._refresh_phase_counts(phase)

def _finalize_running_agents(self, phase, terminal_status):
    """把还在跑的 agents(和 waiting_for_human)敲到 terminal_status。
    waiting_for_human 在 teardown(run 被拆而没有 workflow_completed 事件)
    时也要关闭 — 否则前端会一直等待一个永远不到的回复。

    然后 _refresh_phase_counts 刷新 derived counts,让 done/total 与
    run-level totals 一致。
    """
    for agent in phase.agents:
        if agent.status in ("running", "waiting_for_human"):
            self._stamp_agent_terminal(agent, terminal_status)
    self._refresh_phase_counts(phase)

def _finalize_running_phases(self, terminal_status):
    """把 running phase 和它们的 agents 敲到 terminal_status。
    仅 running phase 受影响 — planned phase 从未启动,故意不动。
    """
    for phase in self.phases:
        if phase.status == "running":
            phase.status = terminal_status
        self._finalize_running_agents(phase, terminal_status)
```

### 4.3 Team token 预算的控制逻辑

**核心代码路径**:
- budget 接收: `_bump_completion` (L789) → 写到 `self.budget` / `self.workflow_budget`
- cron 终态判断: `cron_team_completion.py::cron_team_round_should_end` (L117)

**关键函数签名与代码片段**:

```python
# workflow_state.py (L789)
def _bump_completion(self, phase, progress, agent):
    """Refresh phase counters, write tokens / budget, refresh run totals。"""
    if progress.tokens is not None:
        agent.token_count = progress.tokens
    if progress.budget is not None:
        self.budget = progress.budget                 # session 范围 ledger
    if progress.workflow_budget is not None:
        self.workflow_budget = progress.workflow_budget  # workflow 范围 ledger
    self.token_count = sum(a.token_count or 0 for ph in self.phases for a in ph.agents)
    if progress.tokens is not None and progress.tokens > 0:
        logger.debug("[WF_DBG tok] agent=%s tokens=%s run_sum=%s",
                     agent.name, progress.tokens, self.token_count)
    self._refresh_phase_counts(phase)
    if phase.phase_type == "child" and phase.parent_phase:
        parent = self._find_parent_author_phase(name=phase.parent_phase)
        if parent is not None:
            self._refresh_parent_counts(parent)
    self._refresh_run_agent_counts()

# cron_team_completion.py (L117)
def cron_team_round_should_end(state, *, chunk_complete=False) -> bool:
    """判断 round 是否可结束。三档决策:
    1) workflow 已完成 + leader_final 已发出 → True
    2) team_round_completed + 有 result_text(leader_text 或 workflow_text) → True
       但 leader_text 是占位符则 False
    3) _harness_round_can_end:没起 workflow + 没 open tasks + 没 active members
       + leader_final_seen + 有 leader text 且非占位符 → True
    """
    if chunk_complete:
        if state.get("workflow_completed") and state.get("leader_final_after_workflow"):
            return True
        if state.get("team_round_completed") and cron_team_round_has_result_text(state):
            leader = str(state.get("leader_text") or "").strip()
            if leader and is_cron_leader_placeholder_text(leader):
                return False
            return True
        return _harness_round_can_end(state)
    ...

def is_cron_leader_placeholder_text(text):
    normalized = str(text or "").strip()
    if not normalized:
        return True
    return any(marker in normalized for marker in CRON_LEADER_PLACEHOLDER_MARKERS)
# 占位符 markers: ("最终报告即将生成", "Integration 阶段进行中", "整合阶段进行中")
```

**对 laew 借鉴**:
1. **状态镜像优先级**: `any stopped → stopped; all failed → failed; else completed`。这是个简单又正确的衍生规则,laew Quality-Check 失败回流可以借鉴。
2. **`waiting_for_human` 节点在 teardown 时显式收尾**: 这是 laew 缺失的——目前 Yolo 失败回流只是建议给用户,没有"硬超时取消"机制。借鉴 `_finalize_running_agents` 处理 waiting 节点,可以防止 TUI 在用户离开后无限制等待。
3. **Token budget 的 session / workflow 双 ledger**: laew 当前没有 budget 控制,引入 model_router 后可以借鉴把 budget 字段写到每个 agent state,顶层做汇总展示。

---

## 5. AgentWarmPool 核心代码路径

### 5.1 WarmKey 多维匹配算法

**核心代码路径**:
- 池: `server/runtime/agent_warm_pool.py::AgentWarmPool`(整文件 ~30KB)
- 键: `WarmKey` 数据类(L75)

**关键函数签名与代码片段**:

```python
# server/runtime/agent_warm_pool.py (L75)
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

# 多维匹配优先级(L286)
@staticmethod
def _key_priority(key: WarmKey) -> tuple[int, int, int, str, str]:
    """优先让 web/work/DEFAULT_PROJECT_ID_WORK 在初始 global READY slot。
    排序键越小越优先:(channel, work_mode, project_id, channel_id, project_id)
    """
    return (
        0 if key.channel_id == "web" else 1,
        0 if key.work_mode == "work" else 1,
        0 if key.project_id == DEFAULT_PROJECT_ID_WORK else 1,
        key.channel_id,
        key.project_id,
    )

@staticmethod
def make_key(*, channel_id, project_id, project_dir, work_mode, is_swarm=False) -> WarmKey:
    """规范化构造 WarmKey。channel 兜底 default,work_mode 强制 code/work 二值化。"""
    return WarmKey(
        channel_id=str(channel_id or "default").strip() or "default",
        project_id=str(project_id or "").strip(),
        project_dir=_normalize_project_dir(project_dir),  # normcase + abspath + casefold
        work_mode="code" if str(work_mode).strip().lower() == "code" else "work",
        is_swarm=bool(is_swarm),
    )
```

**配置指纹**(L189):

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

def _next_revision(self, config, env=None) -> WarmRevision:
    self._sequence += 1
    return WarmRevision(
        self._boot_id, self.config_fingerprint(config, env), self._sequence,
    )
```

### 5.2 Agent 预热和回收的实现

**核心代码路径**:
- 同步入口: `sync()` (L297)
- 预热调度: `_pump_background_locked` / `_enqueue_prepare_locked` (L407/422)
- 预热体: `_prepare` (L494)
- 前台抢占: `begin_foreground` / `end_foreground` (L461/482)
- 领取: `claim` (L629)

**关键函数签名与代码片段**:

```python
# server/runtime/agent_warm_pool.py (L297)
async def sync(self, enabled_channels, *, config, env=None) -> dict:
    """sync enabled_channels with the prewarm pool.
    关键逻辑:
    1) 计算 desired = {(channel × project × work_mode) all combinations}
    2) revision: 基于 (boot_id, config_fingerprint, sequence)
    3) 把 stale slots/config-fingerprint 不同的 tasks 取消
    4) 为 desired 中未在 tasks / pending / slots 中的 key 入队
    5) pump background(只在 foreground_count==0 时)
    """
    if not self._enabled:
        return _zero_stats()
    channels: set[str] = set()
    for channel in enabled_channels:
        normalized_channel = str(channel).strip().lower()
        if normalized_channel and normalized_channel not in self.EXCLUDED_CHANNELS:
            channels.add(normalized_channel)  # EXCLUDED_CHANNELS = {"acp", "a2a"}
    revision = self._next_revision(config, env)
    desired = await asyncio.to_thread(self._desired_keys, channels)
    async with self._lock:
        if self._closed:
            return _zero_stats()
        if revision.sequence < self._revision.sequence:
            ...  # 旧 revision 直接返回统计
        config_changed = (
            self._revision.config_fingerprint != revision.config_fingerprint
        )
        self._enabled_channels = channels
        self._desired = desired
        self._revision = revision
        if config_changed:
            self._failed.clear()
        else:
            self._failed = {key: error for key, error in self._failed.items() if key in desired}
        stale_slots = []
        for key, slot in list(self._slots.items()):
            fingerprint_changed = (
                slot.revision.config_fingerprint != revision.config_fingerprint
            )
            if key not in desired or fingerprint_changed:
                stale_slots.append(slot)
        for slot in stale_slots:
            self._slots.pop(slot.key, None)
        for key, task in list(self._tasks.items()):
            ...
            if key not in desired or task_revision.config_fingerprint != revision.config_fingerprint:
                if not task.done():
                    task.cancel()
                ...
        # 按 _key_priority 顺序入队 + 启动 pump
        for key in sorted(desired, key=self._key_priority):
            slot = self._slots.get(key)
            if slot is None and key not in self._tasks and key not in self._pending:
                self._enqueue_prepare_locked(key, revision)
        self._pump_background_locked()
    ...

# 后台泵(L422)
def _pump_background_locked(self):
    """只在前台没在跑时启动下一批 bounded。"""
    if self._closed or self._foreground_count > 0:
        return
    while (self._pending
           and len(self._tasks) < self._max_concurrency
           and len(self._slots) + len(self._tasks) < self._max_ready_slots):
        key = next(iter(self._pending))
        revision = self._pending.pop(key)
        current = self._revision
        if (revision.boot_id != current.boot_id
            or revision.config_fingerprint != current.config_fingerprint
            or key not in self._desired):
            continue
        self._schedule_prepare_locked(key, revision)

# 前台抢占(L461)
async def begin_foreground(self):
    """真用户聊天开始时,取消所有 speculative 预热任务,防止与前台
    DeepAgent 共享 OpenJiuwen registry 时撞车。
    """
    cancelled = 0
    async with self._lock:
        self._foreground_count += 1
        self._foreground_idle.clear()
        for task in list(self._tasks.values()):
            if not task.done():
                task.cancel()
                cancelled += 1
    if cancelled:
        await asyncio.sleep(0)  # 给 cancellation 一点时间

async def end_foreground(self):
    """最后一波聊天结束后,恢复 background 预热(留 cooldown 间隔)。"""
    async with self._lock:
        self._foreground_count = max(0, self._foreground_count - 1)
        if self._foreground_count == 0:
            self._foreground_idle.set()
            self._schedule_background_pump_locked()

# 预热体(L494)
async def _prepare(self, key, session_id, revision, *, keep_as_slot):
    """后台预热 / 前台临建共用路径。
    关键点:
    - 双信号量:_semaphore(全局并发上限),_foreground_semaphore(前台并发)
    - 步骤: 拿信号量 → 初始化锁 → manager.get_agent → prepare_session
    - keep_as_slot=True 时,完成后尝试进 _slots;
      stale(closed/revision_changed/key not desired)则 dispose
    """
    agent = None
    pinned = False
    published = False
    cancelled = False
    foreground_registered = False
    if keep_as_slot:
        self._write_marker(session_id, key)
    else:
        await self.begin_foreground()
        foreground_registered = True
    started_at = time.monotonic()
    try:
        semaphore = self._semaphore if keep_as_slot else self._foreground_semaphore
        if keep_as_slot:
            await self._foreground_idle.wait()  # 防止 background 与 foreground 撞 OpenJiuwen registry
        async with semaphore:
            async with self._initialization_lock:
                if keep_as_slot:
                    await self._foreground_idle.wait()
                agent = await self._manager.get_agent(
                    channel_id=key.channel_id,
                    mode=key.agent_mode,
                    project_dir=key.project_dir or None,
                    sub_mode=key.agent_sub_mode,
                )
                ...
                await agent.prepare_session(
                    session_id=session_id,
                    channel_id=key.channel_id,
                    mode=("code.normal" if key.work_mode == "code" else "agent"),
                    project_dir=key.project_dir or None,
                )
        ...
        async with self._lock:
            promoted = session_id in self._promoted_sessions
            current = self._revision
            revision_changed = (
                current.boot_id != revision.boot_id
                or current.config_fingerprint != revision.config_fingerprint
            )
            stale = not promoted and (self._closed or revision_changed or key not in self._desired)
            if promoted:
                self._manager.pin_agent(agent)
                pinned = True
                self._claimed_pins[session_id] = agent
                pin_task = asyncio.create_task(
                    self._release_claim_pin_after(session_id, 300),  # 5min pin TTL
                    name=f"agent-prewarm-claim-pin-timeout-{session_id}",
                )
                self._pin_release_tasks.add(pin_task)
                pin_task.add_done_callback(self._pin_release_tasks.discard)
            elif not stale:
                self._manager.pin_agent(agent)
                pinned = True
                self._slots[key] = WarmSlot(
                    key=key, session_id=session_id, revision=revision,
                    agent=agent, ready_at=time.time(),
                )
                published = True
                self._failed.pop(key, None)
        if stale:
            await self._dispose_runtime(agent, key.channel_id, session_id, pinned=True)
            pinned = False
    except asyncio.CancelledError:
        cancelled = True
        if agent is not None:
            await self._dispose_runtime(agent, key.channel_id, session_id, pinned=pinned)
        raise
    except Exception as exc:
        logger.exception("Agent prewarm failed: key=%s session_id=%s", key, session_id)
        if agent is not None:
            await self._dispose_runtime(agent, key.channel_id, session_id, pinned=pinned)
        async with self._lock:
            self._failed[key] = str(exc)
    finally:
        promoted = session_id in self._promoted_sessions
        if keep_as_slot and not published and not promoted:
            self.clear_marker(session_id)
        async with self._lock:
            ...
            self._promoted_sessions.discard(session_id)
            if cancelled and keep_as_slot:
                # cancelled 后,如果 desired 还在 + 指纹匹配,重新优先入队
                fingerprint_matches = (revision.config_fingerprint == self._revision.config_fingerprint)
                if key in self._desired and fingerprint_matches:
                    self._enqueue_prepare_locked(key, self._revision, prioritize=True)
            if keep_as_slot:
                self._schedule_background_pump_locked()
        if foreground_registered:
            await self.end_foreground()

# 领取(L629)
async def claim(self, key: WarmKey) -> WarmClaim:
    """从 pool 领取一个 WarmSlot 或前台临建。
    优先选 _slots 已 ready 的;否则把 _tasks 中的 speculative 实例 promote
    (避免与前台构建第二个 DeepAgent 撞 registry)。
    """
    if not self._enabled or key.is_swarm:
        return WarmClaim(self._new_session_id(key.channel_id), False, "bypassed")
    async with self._lock:
        if self._closed:
            raise RuntimeError("agent warm pool is closed")
        is_desired = key in self._desired
        slot = self._slots.pop(key, None)
        if slot is not None:
            self._claimed_pins[slot.session_id] = slot.agent
            pin_task = asyncio.create_task(
                self._release_claim_pin_after(slot.session_id, 300),
                name=f"agent-prewarm-claim-pin-timeout-{slot.session_id}",
            )
            self._pin_release_tasks.add(pin_task)
            pin_task.add_done_callback(self._pin_release_tasks.discard)
            if key not in self._tasks:
                self._enqueue_prepare_locked(key, self._revision, prioritize=True)
            return WarmClaim(slot.session_id, True, "ready")
        if key in self._tasks:
            # promote in-flight:让该 task 拥有最终的 session_id
            task = self._tasks.pop(key)
            self._task_revisions.pop(key, None)
            sid = self._task_session_ids.pop(key)
            self._promoted_sessions.add(sid)
            self._session_tasks[sid] = task
            if is_desired:
                self._enqueue_prepare_locked(key, self._revision, prioritize=True)
        else:
            sid, _ = self._schedule_prepare_locked(
                key, self._revision, keep_as_slot=False,
            )
            if is_desired:
                self._enqueue_prepare_locked(key, self._revision, prioritize=True)
        return WarmClaim(sid, False, "warming")
```

**对 laew 借鉴**:
1. **多维匹配 + 配置指纹**: laew 当前 session → agent 是简单 dict 缓存,没有 WarmKey 五元组 + config_fingerprint。引入 Project + Mode + WorkMode 三维后,可以借鉴让 provider 切换时缓存批量失效。
2. **前台抢占**: `begin_foreground` 在真用户聊天时取消 background 预热,避免共享 registry 撞车。这是 laew 完全没考虑的问题——laew TUI 启动后每个 SubAgent-Work 都从零创建,耗时长。
3. **Promote in-flight task**: `claim` 时若 speculative task 正在跑,直接 promote 而非 cancel 重启,避免重复初始化。laew Quality-Check 同步执行,没有这个问题;但若引入异步 AgentWarmPool,这是个必学模式。
4. **Stale marker cleanup**: `_marker_path(session_id).unlink(missing_ok=True)` + boot_id 比较,在重启时清理未完成的预热资源。laew 重启时若有过期 session,可借鉴清理。

---

## 6. Symphony 记忆核心代码路径

### 6.1 SymphonyGraphBuilder 的离线构建

**核心代码路径**:
- 构建器: `symphony/build.py::SymphonyGraphBuilder`(整文件 ~840 行)
- 状态: `GraphStatus` / `GraphBuildResult` 数据类
- 入口函数: `build_graph` / `graph_status`

**关键函数签名与代码片段**:

```python
# symphony/build.py (L156)
class SymphonyGraphBuilder:
    """Build and refresh the offline Symphony graph."""

    def __init__(self, *, runtime_factory=None, state_builder=None):
        self.runtime_factory = runtime_factory or GraphBuildRuntimeFactory()
        self.state_builder = state_builder or GraphStateBuilder()

    def status(self, skills_root, graph_dir, *, llm_config=None, symphony_config=None) -> GraphStatus:
        """报告 graph 是否存在且是否与 skill 文件夹同步。
        步骤:
        1) 扫描 skills_root
        2) 用 state_builder 算 capability_hashes
        3) load_graph_state → active_entries
        4) 计算 added/changed/removed
        5) stale = (not exists) or (added/changed/removed 非空)
        6) 若 exists && !stale → 校验 SymphonyRuntime orchestration status (fresh?)
        7) 返回 GraphStatus(success, graph_dir, exists, stale, counts, resume_available, checkpoint_dir)
        """
        ...
        scan_result = self.runtime_factory.scan(skills_root, max_depth=runtime_config.fingerprint.scan.max_depth)
        current_hashes = self.state_builder.capability_hashes(scan_result.capabilities)
        state = load_graph_state(output_dir)
        active_entries = state.active_entries()
        exists = graph_exists(output_dir)
        added = [path for path in current_hashes if path not in active_entries]
        changed = [path for path, digest in current_hashes.items()
                   if path in active_entries and active_entries[path].content_hash != digest]
        removed = [path for path in active_entries if path not in current_hashes]
        stale = (not exists) or bool(added or changed or removed)
        if exists and not stale:
            try:
                artifact = (SymphonyRuntime(graph_artifact_root=output_dir, capability_provider=(), model=None)
                            .orchestration.read().to_dict())
                capabilities = [CapabilityFingerprint.model_validate(item)
                                for item in artifact.get("capabilities") or []]
                source_snapshot = artifact.get("source_snapshot")
                expected_snapshot = _graph_status_source_snapshot(...)
                expected_snapshot.update(scan_result.source_snapshot.model_dump(mode="json", exclude_none=True))
                identity_runtime = SymphonyRuntime(
                    graph_artifact_root=output_dir, capability_provider=(), model=None,
                    orchestration_config=graph_build_orchestration_config_from_swarm(runtime_config),
                    source_snapshot=expected_snapshot,
                    graph_config=graph_config_from_swarm(runtime_config),
                )
                stale = not identity_runtime.orchestration.status(expected_snapshot=expected_snapshot).fresh
            except (FileNotFoundError, ValueError):
                stale = True
        resume_from = latest_incomplete_build(output_dir)
        ...
        return GraphStatus(success=True, graph_dir=str(output_dir), exists=exists, stale=stale,
                           skill_count=..., changed_count=..., added_count=..., removed_count=...,
                           resume_available=resume_from is not None,
                           checkpoint_dir=str(resume_from) if resume_from is not None else "",
                           detail=detail)

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
        ...
        output_dir = Path(graph_dir).resolve()
        output_dir.mkdir(parents=True, exist_ok=True)
        reset_llm_token_usage()
        run_id = _new_run_id()
        checkpoint = _BuildCheckpoint(build_run_dir(output_dir, run_id))
        artifact_dir = build_run_dir(output_dir, run_id) / "artifacts"
        artifact_dir.mkdir(parents=True, exist_ok=True)
        resume_from = latest_incomplete_build(output_dir) if resume else None
        checkpoint.record("update.start", status="running", ...)
        ...
        fingerprint_service = self.runtime_factory.fingerprint_service(
            scan_result, output_dir,
            llm_config=llm_config, runtime_config=runtime_config,
        )
        fingerprint_artifact = await fingerprint_service.build(
            force=force,
            progress_callback=record_fingerprint_progress,
        )
        ...
        capabilities = list(fingerprint_artifact.fingerprints)
        source_snapshot = _graph_source_snapshot(
            capabilities=capabilities, current_hashes=current_hashes,
            fingerprint_artifact=fingerprint_artifact,
            fingerprint_signature=fingerprint_signature,
            runtime_config=runtime_config, llm_config=llm_config,
        )
        fingerprints_by_id = {item.capability_id: item for item in fingerprint_artifact.fingerprints}
        new_state = self.state_builder.next_state(
            capabilities=scan_result.capabilities,
            current_hashes=current_hashes,
            fingerprints_by_id=fingerprints_by_id,
            old_state=old_state,
            removed_paths=removed_paths,
        )
        prepared_graph = {}
        graph_resolution = {}

        def prepare_artifact(version_dir):
            """version_dir 完成时(agent-core 在切换 current.json 之前)被调用。
            graph.json 已经写好 → 读出来 → 更新 prepared_graph → 写 fingerprint copy
            → 写 llm_token_usage → 写 graph_state。
            """
            graph_payload = json.loads((version_dir / "graph.json").read_text(encoding="utf-8"))
            prepared_graph.update(graph_payload)
            edge_count = len(graph_payload.get("edges") or [])
            relation_reused_count, relation_resolved_count = _relation_cache_counts(graph_payload, force=force)
            ...
            _copy_fingerprint_artifact(artifact_dir, version_dir)
            _write_json_artifact(get_llm_token_usage_summary(), version_dir / "llm_token_usage.json")
            write_graph_state(new_state, version_dir)

        runtime = SymphonyRuntime(
            graph_artifact_root=output_dir,
            capability_provider=FingerprintArtifactCapabilityProvider(fingerprint_artifact),
            model=(model_from_config(llm_config) if llm_config is not None else None),
            ...
            prepare_artifact=prepare_artifact,
        )
        graph_build = await runtime.orchestration.build(
            force=force, progress=_public_progress_adapter(build_log, graph_resolution=graph_resolution),
        )
        ...
        return GraphBuildResult(success=True, graph_dir=str(output_dir),
                                skill_count=..., reused_count=..., extracted_count=...,
                                removed_count=..., edge_count=..., diagnostics_count=...,
                                relation_reused_count=..., relation_resolved_count=..., version=...)
```

### 6.2 progressive tree retrieval 的渐进检索

**核心代码路径**:
- 算法文档: `symphony/retrieval/algorithm.md`
- 索引: `symphony/indexing/doc.md`

**核心算法要点**(algorithm.md):

```markdown
## Progressive Tree Retrieval

Input: query, tree root, top_k

Process:
1. apply request tag filters to catalog leaves and prune empty tree branches
2. start from the resulting visible subtree
3. if the structure is trivial, use deterministic shortcuts
4. otherwise ask the LLM to choose among the current visible boundary nodes
5. recurse into selected branches or terminate on selected items
6. reduce branch results to the requested top_k

Important rules:
- every LLM routing decision sees only the current visible subtree
- the model outputs the visible boundary node display names only
- display names are uniquified automatically if collisions exist
- single-candidate situations do not call the LLM

## Main Implementations
- retrieval/tree/progressive.py
- retrieval/service/retriever.py
```

### 6.3 evolution 的进化触发和更新

**核心代码路径**:
- 入口: `symphony/service.py::SwarmSymphonyService`(整文件 ~43KB)
- 进化: `symphony/evolution/service.py`(整文件 ~350 行)

**关键函数签名与代码片段**:

```python
# symphony/service.py (L40)
class SwarmSymphonyService:
    """Own the process-local Symphony runtime used by all Agent tools."""

    def __init__(self):
        self._build_guard = asyncio.Lock()
        self._active_build_task: asyncio.Task | None = None
        self._runtime: SymphonyRuntime | None = None
        self._runtime_key: tuple[Any, ...] | None = None

    async def refresh_graph(self, *, force=False, progress=None) -> dict:
        """同步构建;若想异步用 start_refresh_graph。"""
        return await self._build_graph(force=force, progress=progress)

    async def start_refresh_graph(self, *, force=False) -> dict:
        """Start or reuse a process-local background graph build."""
        config = load_symphony_config()
        graph_dir = config.paths.graph_dir
        async with self._build_guard:
            task = self._active_build_task
            if task is not None and not task.done():
                payload = {"success": True, "background": True,
                           "build_status": "running", "graph_dir": str(graph_dir),
                           "detail": "技能总谱已在后台构建中。"}
                payload.update(_build_log_payload(graph_dir))
                ...
                return payload
            if task is not None:
                self._active_build_task = None
            build_logger = _BuildProcessLogger(graph_dir / "build_log.jsonl")
            build_logger.reset()
            build_logger.record("update.start", skills_root=str(...), out_dir=str(...), force=force)
            task = asyncio.create_task(
                self._build_graph(force=force, progress=None,
                                  prestarted=True, config=config),
                name="symphony-graph-build",
            )
            self._active_build_task = task
            task.add_done_callback(self._consume_background_build_result)
        ...

    async def cancel_build(self) -> dict:
        """取消后台构建;已完成的 cache 和 checkpoint 会保留。"""
        config = load_symphony_config()
        graph_dir = config.paths.graph_dir
        build_logger = _BuildProcessLogger(graph_dir / "build_log.jsonl")
        async with self._build_guard:
            task = self._active_build_task
            if task is None or task.done():
                if task is not None:
                    self._active_build_task = None
                _repair_interrupted_build_log(graph_dir)
                payload = {"success": False, "graph_dir": str(graph_dir),
                           "cancelled": False, "build_status": "idle",
                           "detail": "当前没有正在运行的技能总谱构建。"}
                ...
                return payload
            build_logger.record("update.cancel_requested")
            task.cancel("skills.graph.cancel")
            build_logger.record("update.cancelled")
        try:
            await task
        except asyncio.CancelledError:
            pass
        payload = {"success": True, "graph_dir": str(graph_dir),
                   "cancelled": True, "build_status": "cancelled",
                   "detail": "已取消技能总谱构建,已完成的缓存和 checkpoint 会保留。"}
        ...
```

**对 laew 借鉴**:
1. **离线 + 在线双层架构**: `indexing/` 离线构建 tree_index.yaml/catalog.jsonl/manifest.json;`retrieval/` 在线 progressive tree 路由。laew 的 AgentMemory 表可以借鉴——把"skill/task fingerprint"离线索引,在线路由按 tag/keyword/semantic 三路召回。
2. **Source Snapshot + Identity Runtime**: 用 `source_snapshot`(含 capabilities_sha256 + fingerprint_signature + llm_sha256)做 freshness 检测。laew 没有 fingerprint 比对,引入 model / provider 切换时可以让 AgentMemory 自动失效。
3. **后台 Build + Cancel 保 cache**: `start_refresh_graph` / `cancel_build` 保证取消后已完成的 cache 和 checkpoint 保留,这是 laew 在 SessionContext 摘要后立即清缓存时没有的考虑。
4. **Tag Filter → prune subtree → LLM 选择**: 这是渐进式检索的核心。laew 如果要做 Skill 调度,可以借鉴三层递进:tag 过滤 → 关键词匹配 → LLM 选择。

---

## 7. 沙箱执行核心代码路径

### 7.1 JiuwenBoxRunner 的启动和执行

**核心代码路径**:
- Runner: `server/sandbox/jiuwenbox_runner.py::JiuwenBoxRunner`(整文件 ~493 行)

**关键函数签名与代码片段**:

```python
# server/sandbox/jiuwenbox_runner.py (L37)
_PR_SET_PDEATHSIG = 1  # 来自 <linux/prctl.h>

class JiuwenBoxRunner:
    """单例形态管理本地 jiuwenbox uvicorn 子进程。"""

    _INSTANCE: "JiuwenBoxRunner | None" = None
    _STDERR_TAIL_MAX: int = 80  # stderr 滚动缓冲 80 行

    def __init__(self):
        self._process: Optional[asyncio.subprocess.Process] = None
        self._host: str = "127.0.0.1"
        self._port: int = 8321
        self._lock = asyncio.Lock()
        self._owns_process: bool = False
        self._atexit_registered: bool = False
        self._stdout_pump_task = None
        self._stderr_pump_task = None
        self._stderr_tail: list[str] = []
        self._last_startup_mode: str = "internal"
        self._spawned_policy_path: Optional[Path] = None

    @classmethod
    def instance(cls) -> "JiuwenBoxRunner":
        if cls._INSTANCE is None:
            cls._INSTANCE = JiuwenBoxRunner()
        return cls._INSTANCE

    @property
    def base_url(self) -> str:
        return f"http://{self._host}:{self._port}"

    def get_stderr_tail(self, lines=40) -> str:
        """返回最近 N 行 stderr,便于诊断。"""
        if not self._stderr_tail:
            return ""
        return "\n".join(self._stderr_tail[-lines:])

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
        async with self._lock:
            normalized_mode = (startup_mode or "internal").strip().lower()
            if normalized_mode not in ("internal", "external"):
                normalized_mode = "internal"
            self._last_startup_mode = normalized_mode

            if normalized_mode == "external":
                self._host = host
                self._port = port
                if await self.health_check(host, port):
                    logger.info("[JiuwenBoxRunner] external jiuwenbox alive at %s:%d ...", host, port, policy_path)
                    return True
                ...
                return False

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
                    logger.info("[JiuwenBoxRunner] reuse owned jiuwenbox at %s:%d ...", host, port, policy_path)
                    return True
                return await self._wait_until_ready(host, port, timeout=timeout)

            if self._process is not None and self._owns_process:
                logger.info("[JiuwenBoxRunner] stopping owned jiuwenbox before spawning new one ...")
                await self._stop_no_lock()

            self._host = host
            self._port = port

            cmd = [
                sys.executable, "-m", "uvicorn",
                "jiuwenbox.server.app:app",
                "--host", host, "--port", str(port),
            ]
            env = dict(os.environ)
            local_src = _resolve_jiuwenbox_src_dir()
            if local_src is not None:
                existing = env.get("PYTHONPATH", "")
                parts = [str(local_src)]
                if existing:
                    parts.append(existing)
                env["PYTHONPATH"] = os.pathsep.join(parts)
            if policy_path is not None:
                env["JIUWENBOX_POLICY_PATH"] = str(policy_path)
                logger.info("[JiuwenBoxRunner] injecting JIUWENBOX_POLICY_PATH=%s", policy_path)
            else:
                env.pop("JIUWENBOX_POLICY_PATH", None)

            logger.info("[JiuwenBoxRunner] spawning: %s", " ".join(cmd))
            try:
                spawn_kwargs = {
                    "stdout": asyncio.subprocess.PIPE,
                    "stderr": asyncio.subprocess.PIPE,
                    "env": env,
                }
                # Linux:父进程退出时让子进程收到 SIGTERM
                if sys.platform.startswith("linux"):
                    spawn_kwargs["preexec_fn"] = _try_set_pdeathsig
                self._process = await asyncio.create_subprocess_exec(*cmd, **spawn_kwargs)
                self._owns_process = True
                self._spawned_policy_path = policy_path
                self._register_atexit_once()
                self._stderr_tail = []
                self._stdout_pump_task = asyncio.create_task(self._pump_stream(self._process.stdout, "stdout"))
                self._stderr_pump_task = asyncio.create_task(self._pump_stream(self._process.stderr, "stderr"))
            except Exception as exc:
                logger.error("[JiuwenBoxRunner] spawn failed: %s", exc)
                self._process = None
                self._owns_process = False
                self._spawned_policy_path = None
                return False

            ok = await self._wait_until_ready(host, port, timeout=timeout)
            if not ok:
                tail = "\n".join(self._stderr_tail[-40:])
                ...
            return ok
```

### 7.2 pdeathsig 子进程守护的实现

**核心代码路径**:
- `jiuwenbox_runner.py::_try_set_pdeathsig` (L54)

**关键函数签名与代码片段**:

```python
# server/sandbox/jiuwenbox_runner.py (L54)
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
    except Exception:  # noqa: BLE001
        pass

# 同步退出兜底(L390)
def _register_atexit_once(self):
    if self._atexit_registered:
        return
    try:
        atexit.register(self._sync_terminate)
        self._atexit_registered = True
    except Exception as exc:
        logger.warning("[JiuwenBoxRunner] atexit register failed: %s", exc)

def _sync_terminate(self):
    """atexit / 异常退出场景:不依赖事件循环。
    - 若 stop() 已正常清理,则什么都不做
    - 否则尽可能 terminate / kill 子进程,避免 jiuwenbox 残留
    """
    proc = self._process
    if proc is None or not self._owns_process:
        return
    if proc.returncode is not None:
        return
    pid = proc.pid
    logger.info("[JiuwenBoxRunner] atexit: terminating subprocess pid=%s", pid)
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    except Exception as exc:
        logger.warning("[JiuwenBoxRunner] atexit SIGTERM failed: %s", exc)
        return
    deadline = time.monotonic() + 3.0
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)  # 0 信号探测进程是否存在
        except ProcessLookupError:
            return
        except Exception:
            return
        time.sleep(0.1)
    with contextlib.suppress(ProcessLookupError, Exception):
        os.kill(pid, signal.SIGKILL)

# 优雅停止(L435)
async def stop(self) -> None:
    async with self._lock:
        await self._stop_no_lock()

async def _stop_no_lock(self):
    """stop() 的去锁版本;ensure_running 复用(监测 policy_path 变更重启)。"""
    for task_attr in ("_stdout_pump_task", "_stderr_pump_task"):
        task = getattr(self, task_attr, None)
        if task is not None and not task.done():
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError, Exception):
                await task
            setattr(self, task_attr, None)

    proc = self._process
    if proc is None or proc.returncode is not None:
        self._process = None
        self._spawned_policy_path = None
        return
    if not self._owns_process:
        self._process = None
        self._spawned_policy_path = None
        return
    logger.info("[JiuwenBoxRunner] stopping subprocess pid=%s", proc.pid)
    try:
        proc.terminate()
    except ProcessLookupError:
        self._process = None
        self._spawned_policy_path = None
        return
    # uvicorn 收到 SIGTERM 后跑 FastAPI lifespan shutdown,期间调
    # SandboxManager.shutdown_all_sandboxes 给每个活的 sandbox 做
    # SIGTERM -> wait -> SIGKILL 三段式 teardown(每个最坏要 ~15s)。
    # 如果这里留的 grace 不够长, lifespan 还没清完就被 SIGKILL,
    # sandbox-daemon.py 会被 reparent 到 init 成为孤儿进程,留在 host 上一直跑。
    # 所以给一个相对宽松的 60s 上限。
    try:
        await asyncio.wait_for(proc.wait(), timeout=60.0)
    except asyncio.TimeoutError:
        logger.warning("[JiuwenBoxRunner] terminate timeout (60s); killing pid=%s ...", proc.pid)
        try:
            proc.kill()
            await proc.wait()
        except Exception as exc:
            logger.warning("[JiuwenBoxRunner] kill failed: %s", exc)
    self._process = None
    self._owns_process = False
    self._spawned_policy_path = None
```

### 7.3 策略注入的加载逻辑

**核心代码路径**:
- 注入位置: `jiuwenbox_runner.py::ensure_running` (env 注入 `JIUWENBOX_POLICY_PATH`)

**关键函数签名与代码片段**:

```python
# server/sandbox/jiuwenbox_runner.py (L268)
env = dict(os.environ)
local_src = _resolve_jiuwenbox_src_dir()
if local_src is not None:
    existing = env.get("PYTHONPATH", "")
    parts = [str(local_src)]
    if existing:
        parts.append(existing)
    env["PYTHONPATH"] = os.pathsep.join(parts)
    logger.info("[JiuwenBoxRunner] prepending local jiuwenbox src to PYTHONPATH: %s", local_src)

# 把 policy 路径通过环境变量传给 jiuwenbox-server(与 README 一致)。
# 如果调用方没给,显式删掉父进程继承下来的同名变量,避免误用旧值。
if policy_path is not None:
    env["JIUWENBOX_POLICY_PATH"] = str(policy_path)
    logger.info("[JiuwenBoxRunner] injecting JIUWENBOX_POLICY_PATH=%s", policy_path)
else:
    env.pop("JIUWENBOX_POLICY_PATH", None)
```

**对 laew 借鉴**:
1. **PR_SET_PDEATHSIG 子进程守护**: laew Bash 工具通过 `subprocess::Command::new(...).spawn()` 创建子进程,如果 laew 主进程 SIGKILL 退出,子进程会变孤儿。借鉴 `_try_set_pdeathsig` 在 Linux 上让子进程收到 SIGTERM,这是工业级做法。
2. **Atexit + 三阶段 SIGTERM→SIGKILL**: `_sync_terminate` 给 3 秒 grace 后强杀,`stop()` 给 60 秒(因为要等 sandbox-daemon)。laew 当前 TUI 退出时只 SIGINT 给活跃 task,没考虑子进程残留。
3. **策略注入通过环境变量**: `JIUWENBOX_POLICY_PATH` + `PYTHONPATH` 是 jiuwenbox 启动时读取的。laew 如果做 Skill 沙箱,可以借鉴把"策略文件路径"通过 env 注入,子进程启动时读一次,避免 IPC 复杂度。
4. **stderr 滚动缓冲 80 行 + tail dump**: 出错时 `get_stderr_tail(40)` 拿到最近 40 行用于诊断。laew TUI 错误展示可借鉴在崩溃时把最近 N 行错误堆栈 dump 出来。

---

## 8. 综合借鉴清单

下表归纳所有 7 个核心机制的可借鉴要点,按优先级和影响面排列:

| 优先级 | 机制 | laew 借鉴要点 | 预期收益 |
|--------|------|---------------|---------|
| **P0** | SwarmBuildContext | 引入"属性 vs 环境"分离的 Spec 模型,Yolo/Plan/Main/Sub 各 Agent 的能力描述改为声明式 | 跨进程 / 分布式部署就绪 |
| **P0** | SuspensionConfig 三段式 | Plan Agent 产出方案后用 `extract_data + on_resume + next_stage` 与用户交互 | 让 hard 任务的 plan 确认更灵活 |
| **P0** | PR_SET_PDEATHSIG | Bash 工具的子进程守护,主进程 SIGKILL 后不残留 | 工业级沙箱稳定性 |
| **P1** | TeamManager active_rounds | 引入 `_active_rounds` round 准入,避免 TUI/CLI/Web 多端 round 打架 | 多端一致体验 |
| **P1** | AgentWarmPool 预热池 | 引入 Project+Mode+WorkMode 三维 WarmKey + config_fingerprint | provider 切换秒级生效 |
| **P1** | Wire Codec legacy 兜底 | `metadata.legacy_key` 整包回退 + envelope 错误消息 | 协议升级向前兼容 |
| **P2** | ACP JSON-RPC over stdio | 未来接 Skill Marketplace / 第三方 agent 时直接用 | 生态接入 |
| **P2** | A2UI Tagged JSON | TUI 引入富交互(按钮/确认),三级 fallback 解析 | 复杂交互能力 |
| **P2** | SkillDev state machine | Skill 注册表 + 阶段挂起 + checkpoint | Skill 类型声明式扩展 |
| **P2** | Skill Evolution overlay | AgentMemory 加 `record_plan_outcome` + evidence_id 去重 | 重试风暴不污染统计 |
| **P2** | Progressive tree retrieval | AgentMemory 加 tag/keyword/semantic 三路召回 | Skill 调度能力 |
| **P3** | Workflow budget ledger | 引入 session/workflow 双 ledger,budget 写在每个 agent state | 资源控制 |
| **P3** | Foreground preempt | 后台预热任务被前台真用户聊天抢占(取消 + cancel-callback) | 共享 registry 不撞车 |
| **P3** | Stale marker cleanup | boot_id 比较 + 重启时清理未完成 session 资源 | 重启一致性 |
| **P3** | Placeholder text detection | leader final 占位符识别("最终报告即将生成"等) | 避免 cron 误判完成 |
| **P3** | 来源快照 source_snapshot | config fingerprint + model identity 联合判断 staleness | 自动失效缓存 |

---

## 附录 A: 关键代码路径索引

| 核心机制 | 主入口文件 | 关键函数 |
|---------|-----------|---------|
| 多 Agent 协作 | `agents/swarm/assembly.py` | `enrich_team_spec_for_swarm`, `preflight_team_mcps` |
| SwarmBuildContext | `agents/swarm/context.py` | `SwarmBuildContext`, `to_seed`, `from_seed` |
| TeamManager | `agents/harness/team/team_manager.py` | `TeamManager.broadcast_event`, `begin_round`, `release_round`, `abort_round` |
| Cron 终态 | `common/cron_team_completion.py` | `apply_cron_team_round_event`, `cron_team_round_should_end` |
| E2A 编解码 | `common/e2a/wire_codec.py` | `parse_agent_server_wire_unary`, `encode_agent_response_for_wire` |
| ACP stdio | `acp/stdio_client.py` | `AcpStdioClient.connect`, `_rpc_call`, `_handle_peer_request`, `chat`, `close` |
| A2UI 协议 | `server/runtime/a2ui/protocol.py` | `A2UIProtocolSpec.build_prompt`, `parse_response`, `validate_messages` |
| A2UI 解析 | `server/runtime/a2ui/parser.py` | `parse_a2ui_response`, `iter_tagged_block_bodies`, `parse_raw_json`, `parse_jsonl` |
| SkillDev 流水线 | `server/runtime/skill/skilldev/pipeline.py` | `SkillDevPipeline.run`, `resume`, `_checkpoint` |
| 挂起点配置 | `server/runtime/skill/skilldev/schema.py` | `SUSPENSION_POINTS`, `SuspensionConfig` |
| Skill Evolution | `symphony/evolution/service.py` | `load_dynamic_overlay`, `record_plan_outcome`, `rebuild_dynamic_overlay` |
| Workflow 状态 | `agents/harness/team/handlers/workflow_state.py` | `WorkflowRunState.apply`, `_switch_to_phase`, `_bump_completion`, `_finalize_running_agents` |
| WarmKey | `server/runtime/agent_warm_pool.py` | `WarmKey`, `make_key`, `_key_priority`, `config_fingerprint` |
| WarmPool 同步 | `server/runtime/agent_warm_pool.py` | `sync`, `_pump_background_locked`, `begin_foreground`, `end_foreground`, `claim`, `_prepare` |
| Symphony Graph 构建 | `symphony/build.py` | `SymphonyGraphBuilder.status`, `SymphonyGraphBuilder.build` |
| Symphony 服务 | `symphony/service.py` | `SwarmSymphonyService.refresh_graph`, `start_refresh_graph`, `cancel_build` |
| Progressive Retrieval | `symphony/retrieval/algorithm.md` | 算法描述(tree.py + retriever.py) |
| JiuwenBox Runner | `server/sandbox/jiuwenbox_runner.py` | `JiuwenBoxRunner.ensure_running`, `_try_set_pdeathsig`, `_sync_terminate`, `stop`, `_stop_no_lock` |

---

## 附录 B: 借鉴落地建议(与 laew 当前架构对接)

### B.1 即时落地(P0)

1. **`SwarmBuildContext` 模式** → 改造 `src/agent/profile.rs::AgentProfile`:
   - 把硬编码 `work_profile()` / `yolo_profile()` 工厂函数,改为 `AgentSpec::from_config(config_yaml) -> AgentSpec` 解析层
   - 引入 `BuildContext { session_id, request_id, mode, project_dir, ... }` 与 `BuildContext::to_seed() / from_seed()`
   - 为将来分布式 spawn / 冷恢复铺路

2. **`SuspensionConfig` 三段式** → 改造 `src/agent/yolo.rs` 的 Plan 阶段:
   - 在 hard 任务下 Plan Agent 产出方案后,emit `<<<LAEW:PLAN_CONFIRM>>>` 类似标记
   - TUI 弹出确认框(沿用现有 `/provider *` 子屏模式)
   - 复用 `extract_data + on_resume + next_stage` 三段式

3. **PR_SET_PDEATHSIG** → 改造 `src/agent/tools/bash.rs`:
   - Linux 平台 spawn 子进程时通过 `preexec_fn` 调用 `libc::prctl(PR_SET_PDEATHSIG, SIGTERM)`
   - 非 Linux 是 no-op
   - 同时给 atexit 注册 hook,在 laew 主进程异常退出时清理子进程

### B.2 短期落地(P1)

1. **Wire Codec legacy 兜底** → 改造 `src/llm/anthropic.rs` + `src/llm/openai.rs`:
   - 在 protocol version 升级时,`wire_dict` 加 `metadata.legacy_payload` 字段
   - 接收端先按新协议解码,失败回退到 legacy
   - 与 laew 现有 `LlmClient` trait 兼容

2. **AgentWarmPool 预热池** → 新增 `src/runtime/agent_warm_pool.rs`:
   - `WarmKey { channel_id, project_id, project_dir, work_mode, is_swarm }`
   - 后台预热 + config_fingerprint 失效
   - 与 `AgentManager.get_or_create_agent()` 集成

3. **TeamManager active_rounds** → 新增 `src/runtime/round_manager.rs`:
   - `_active_rounds: HashMap<session_id, _ActiveRound>`
   - `begin_round` / `release_round` / `abort_round`
   - 在 TUI/CLI/Web 入口处调用

### B.3 中期落地(P2)

1. **ACP JSON-RPC over stdio** → 新增 `src/acp/stdio_client.rs`:
   - 增量 JSON 缓冲 + JSON-RPC 调用循环
   - 自动 `_AUTO_APPROVE` 应答 `session/request_permission`
   - 路径校验防 traversal

2. **A2UI Tagged JSON** → 新增 `src/a2ui/protocol.rs` + `parser.rs`:
   - OPEN_TAG/CLOSE_TAG 块扫描
   - JSONL → raw JSON → tagged 三级 fallback
   - TUI 渲染层增加 A2UI 块切分

3. **Skill Evolution overlay** → 改造 `src/runtime/session_memory.rs`:
   - `record_plan_outcome(evidence_id, outcome, ...)` 防止重试风暴
   - `_overlay_requires_rebuild` 检测

---

> **总结**: JiuwenSwarm 是一个**工业级多 Agent 协作系统**,其声明式装配 + 跨边界重建 + Skill 自演进 + SwarmFlow 确定性工作流 + 多渠道接入 + 离线图构建 + 沙箱子进程守护的设计,为 laew 工程提供了从"三档分类的硬编码 Yolo + Plan"升级到"声明式 AgentSpec + Skill Marketplace + 沙箱执行"的完整蓝图。建议从 SwarmBuildContext 模式 + SuspensionConfig 三段式 + PR_SET_PDEATHSIG 三个 P0 改造开始,逐步构建多 Agent 协作能力。