# Agent Core 综合深度分析

> 调研对象:agent-core(Python,openJiuwen Core SDK)
> 调研日期:2026-09-05
> 原始文档:3 份(agent-core-源码调研.md + agent-core-深度分析.md + agent-core-核心机制深度分析.md)
> 原始总行数:~5,017 行(合并后约 3,200 行,去重压缩率 ~36%)

---

## 目录

1. [项目元信息](#一项目元信息)
2. [ReAct 循环](#二react-循环)
3. [ContextEngine](#三contextengine)
4. [多类型记忆系统](#四多类型记忆系统)
5. [PermissionEngine 三级防护](#五permissionengine-三级防护)
6. [TeamAgent 双 Spawn](#六teamagent-双-spawn)
7. [Pregel 图执行](#七pregel-图执行)
8. [Rails 铁轨](#八rails-铁轨)
9. [OTLP Trajectory](#九otlp-trajectory)
10. [RL 训练](#十rl-训练)
11. [对 laew 的借鉴](#十一对-laew-的借鉴)

---

## 一、项目元信息

### 1.1 工程定位

**openJiuwen Core**（包名 `openjiuwen`）是华为开源的 Python AI Agent SDK（v0.1.17），Python `>=3.11,<3.14`，**Apache-2.0** 许可证。

核心价值：
- 提供 Agent 创建、Workflow 编排、LLM 调用、工具调用的多层次 SDK
- 内置高性能异步执行引擎，支持流式处理与状态保存/中断恢复
- 提供 Prompt 自动优化、全链路可观测性等调试优化工具

### 1.2 构建系统

- **后端**：`setuptools>=61`，包管理 `uv`，镜像源阿里云 PyPI
- **入口点**：
  - `openjiuwen` → CLI 主命令
  - `team-member` → 团队成员运行
  - `openjiuwen-team-mcp` → MCP 服务
  - `openjiuwen-rl-service` → RL 训练服务

### 1.3 核心依赖

| 分类 | 依赖 |
|------|------|
| HTTP 客户端 | `aiohttp>=3.11.12`、`requests>=2.32.3` |
| 数据库 ORM | `sqlalchemy[asyncio]>=2.0.41`、`sqlmodel>=0.0.37` |
| AI/LLM | `anthropic>=0.120.2`、`openai>=1.108.0`、`transformers>=4.52.4`、`dashscope>=1.25.6` |
| 向量数据库 | `pymilvus>=2.6.2,<2.6.10` |
| MCP | `fastmcp>=2.14.2,<3.0`、`mcp>=1.26.0` |
| 可观测性 | `opentelemetry-api/sdk/exporter-otlp-*` |
| 沙箱 | `agent-sandbox>=0.0.26`（可选） |

### 1.4 目录组织

```
agent-core/
├── openjiuwen/
│   ├── core/                      # 公共 SDK/运行时
│   │   ├── single_agent/          # ReActAgent (3251行)
│   │   ├── context_engine/        # 上下文引擎 (726行)
│   │   ├── memory/                # LongTermMemory (1585行)
│   │   ├── session/               # 会话管理
│   │   ├── workflow/              # 工作流引擎
│   │   ├── retrieval/             # RAG 检索
│   │   ├── graph/                 # Pregel 图引擎 (277行)
│   │   ├── foundation/            # LLM/工具/消息基础
│   │   ├── multi_agent/           # 多 Agent 基础
│   │   ├── security/              # 安全基础
│   │   └── kv_cache/              # KV 缓存
│   ├── harness/                   # 编码 Agent 框架 (~20000行)
│   │   ├── deep_agent.py          # DeepAgent (3999行)
│   │   ├── tools/                 # 工具集 (ability_manager.py 1512行)
│   │   ├── rails/                 # 铁轨机制
│   │   ├── security/              # PermissionEngine (445行)
│   │   ├── task_loop/             # 任务循环
│   │   └── goal/                  # 目标管理
│   ├── agent_teams/               # 多 Agent 团队协作 (~15000行)
│   │   ├── agent/team_agent.py    # TeamAgent (1756行)
│   │   ├── spawn/                 # 双 Spawn (inprocess/external)
│   │   └── mcp/                   # 团队 MCP
│   ├── extensions/                # 扩展集成 (~10000行)
│   │   ├── observability/         # OpenTelemetry (callback_handler.py 91KB)
│   │   ├── tracer_otel/           # OTel 追踪
│   │   └── context_evolver/       # 上下文进化
│   └── agent_evolving/            # Agent 进化 (~8000行)
│       ├── agent_rl/              # 在线 RL 训练
│       ├── trajectory/            # 轨迹收集 (model.py 276行)
│       ├── rl_trainer/            # verl_executor.py (615行)
│       ├── optimizer/             # 优化器
│       └── evaluator/             # 评估器
└── tests/                         # 单元测试 + 系统测试
```

### 1.5 四层架构

```
┌─────────────────────────────────────────────────────────┐
│                    CLI / API 入口层                       │
├─────────────────────────────────────────────────────────┤
│                    Agent 编排层                          │
│          DeepAgent → ReActAgent → BaseAgent             │
│          WorkflowAgent / TeamAgent                      │
├─────────────────────────────────────────────────────────┤
│                    能力管理层                             │
│   AbilityManager (工具/工作流/MCP/Agent 统一注册)         │
│   ContextEngine (上下文管理 + 压缩 + 溢出恢复)            │
├─────────────────────────────────────────────────────────┤
│                    基础设施层                            │
│   Model (LLM) / Tool / Memory / Store / Retrieval      │
└─────────────────────────────────────────────────────────┘
```

---

## 二、ReAct 循环

### 2.1 核心代码路径

**核心代码位置**：`openjiuwen/core/single_agent/agents/react_agent.py`（3251 行）

```
ReActAgent.invoke(inputs, session=None)
   │
   ▼  (L2480)
_inner_invoke(session, inputs, query, ...)
   │
   ├─ 解析输入 + 自动创建 Session（L2509-2529）
   ├─ 构建 AgentCallbackContext（L2573-2626）
   ├─ fire BEFORE_INVOKE / AFTER_INVOKE 铁轨（L2628）
   ▼  (L2641)
_load_interruption_state(session)  ── 加载中断状态支持 resume
   │
   ▼  (L2653)
_init_context(session)
   └─► context_engine.create_context()
   │
   ▼  (L2657-2672)
   ├─ _build_rendered_system_prompt(inputs)
   ├─ _update_skill_prompt_builder_section()
   ├─ ability_manager.list_tool_info()
   │
   ▼  (L2722) ReAct 主循环
for iteration in range(start_iteration, max_iterations):
   │
   ├─ ctx.consume_force_finish()           ── 边界检查
   ├─ _drain_steering_batch(ctx)           ── 排空转向队列
   │
   ├─► _call_model(ctx, context, tools)    ── L2748
   │     ├─► _recover_from_model_exception()   ── L1020 上下文溢出重试
   │     └─► _railed_model_call(ctx)            ── L1482 @rail 装饰器
   │           ├─ prompt_builder.build()
   │           ├─ context.get_context_window()  ── 应用所有 ContextProcessor
   │           ├─ llm.invoke() 或 llm.stream()   ── L1663
   │           └─ 计算 TTFT / TPOT 性能指标
   │
   ├─► context.add_messages(AssistantMessage)
   ├─ 若 !ai_message.tool_calls → break
   │
   ├─► _execute_tool_call(ctx, tool_calls, session, context)
   │     └─► ability_manager.execute(ctx, tool_call, session, parallel=True)
   │
   ├─ _after_execute_tool_call_for_hitl() ── 检查 HITL 中断
   ├─ _after_execute_tool_call()            ── 检查工作流中断
   │
   └─ ctx.fire(AFTER_REACT_ITERATION)
```

### 2.2 关键函数签名

```python
class ReActAgent(BaseAgent):
    async def invoke(self, inputs: Any, session: Optional[Session] = None, **kwargs) -> Dict[str, Any]:
        """ReAct 入口。inputs 支持 dict/str，kwargs._streaming 切换 invoke/stream。"""

    @with_session()
    async def _inner_invoke(self, session, inputs, query, ...): ...

    async def _call_model(self, ctx, context, tools) -> AssistantMessage:
        """调用 LLM，含上下文溢出自动恢复"""

    @rail(before=BEFORE_MODEL_CALL, after=AFTER_MODEL_CALL, on_exception=ON_MODEL_EXCEPTION)
    async def _railed_model_call(self, ctx) -> AssistantMessage:
        """实际 LLM 调用点（被 @rail 装饰器包装钩子）"""

    async def _execute_tool_call(self, ctx, tool_calls, session, context) -> list:
        """执行工具调用，返回 (tool_result, tool_message) 元组列表"""
```

### 2.3 上下文溢出自动恢复

```python
# react_agent.py L1014-1047
try:
    ai_message = await self._railed_model_call(ctx)
except Exception as exc:
    if ctx.extra.get("_model_exception_recovery_attempted"):
        raise
    recovered = await self._recover_from_model_exception(
        ctx, context=context, exception=exc)
    if not recovered:
        raise
    # Rebuild preview before retry
    ctx.extra["_model_exception_recovery_attempted"] = True
    ctx.inputs = ModelCallInputs(
        messages=self._build_preview_messages(context),
        tools=list(tools) if tools else None, ...)
    ai_message = await self._railed_model_call(ctx)  # 重试
```

### 2.4 流式输出 + 性能指标

```python
# react_agent.py L1654-1772
async for chunk in llm.stream(model=self._config.model_name, ...):
    accumulated_chunk = accumulated_chunk + chunk  # 累加器
    if call_first_token_time is None:
        call_first_token_time = time.monotonic()
    call_last_token_time = time.monotonic()

    if chunk.reasoning_content:
        await session.write_stream(OutputSchema(type="llm_reasoning", ...))
    if chunk.content:
        await session.write_stream(OutputSchema(type="llm_output", ...))

# 结束：合并 + 计算 TTFT/TPOT
if ai_message.usage_metadata and output_tokens > 1:
    perf_metrics["tpot_ms"] = round(
        (call_last_token_time - call_first_token_time) / (output_tokens - 1) * 1000, 2)
```

### 2.5 双中断机制

```
IDLE ──invoke──→ RUNNING ──工具中断──→ INTERRUPTED
                                      │
                用户回复 ──resume──→ RESUMING
                                      │
                                      └──→ RUNNING（继续循环）
```

```python
# 中断检测
def _is_interrupted(self, tool_result):
    if isinstance(tool_result, WorkflowOutput):
        return tool_result.state == WorkflowExecutionState.INPUT_REQUIRED
    ...

# 中断提交：写入占位 ToolMessage + 持久化状态 + 保存上下文
async def _commit_interrupt(self, interrupt, context, session, invoke_inputs, ...):
    pending_entry = interrupt.interrupted_workflows[interrupt.pending_workflow_id]
    await context.add_messages(ToolMessage(
        tool_call_id=pending_entry.tool_call.id,
        content="[INTERRUPTED - Waiting for user input]"))
    await self.context_engine.save_contexts(session)
    self._save_interruption_state(interrupt, session)

# 恢复：收集所有中断点的反馈后并发恢复
async def _handle_resume(self, interruption_state, user_input, ...):
    pending_entry.collected_input = interactive_input
    all_collected = all(entry.collected_input is not None ...)
    if all_collected:
        results = await self._execute_tool_call(ctx, all_tool_calls, ...)
```

### 2.6 设计亮点

| 特性 | 实现位置 | 说明 |
|------|----------|------|
| **铁轨装饰器** | `@rail` (L1482) | BEFORE_MODEL_CALL / AFTER_MODEL_CALL / ON_MODEL_EXCEPTION 钩子 |
| **转向队列** | `_drain_steering_batch` (L932) | 允许用户在 Agent 运行中插入新指令 |
| **force_finish** | `ctx.consume_force_finish()` | 铁轨可在迭代边界触发优雅退出 |
| **上下文溢出重试** | `_recover_from_model_exception` (L1056) | 检测 context-overflow → 自动压缩 → 重试 |
| **TTFT/TPOT 指标** | `_railed_model_call` (L1752-1761) | 实时计算首 token 延迟与每 token 延迟 |
| **HITL 工具中断** | `_after_execute_tool_call_for_hitl` (L2269) | 与工作流中断并列的双层中断检查 |

---

## 三、ContextEngine

### 3.1 完整架构

**核心代码位置**：`openjiuwen/core/context_engine/context_engine.py`（726 行）

```
ContextEngine
├─ _context_pool: Dict[str, ModelContext]    ── 缓存池（key=session_id_context_id）
├─ _window_mutators: List[Callable]          ── 窗口变异器
├─ create_context(...)                        ── 创建/复用 ModelContext
├─ compress_context(...)                      ── 主动压缩
├─ recover_from_model_exception(...)          ── 被动溢出恢复
├─ get_context_window(...)                    ── 应用所有 ContextProcessor
└─ _OVERFLOW_PHRASES / _OVERFLOW_FIELDS       ── 上下文溢出检测器
```

### 3.2 Token 计数器多级 fallback

```python
# context_engine.py L102-141
@staticmethod
def _select_token_counter(config: ContextEngineConfig) -> TokenCounter:
    has_model_tokenizer_target = bool(
        config.model_name or config.model_provider
        or config.tokenizer_spec or config.tokenizer_registry)
    try:
        return TokenizerSelector(
            provider=config.model_provider or "",
            model=config.model_name or "",
            spec=config.tokenizer_spec,
            manager=TokenizerArtifactManager(offline=True),
            allow_tiktoken_fallback=(
                config.enable_tiktoken_counter and not has_model_tokenizer_target),
        ).select()
    except Exception:
        # 最终兜底：按字符长度估算
        return StringLengthCounter(
            model=config.model_name or "",
            fallback_reason="local_tokenizer_selection_failed")
```

### 3.3 上下文创建与缓存

```python
# context_engine.py L174-251
async def create_context(self, context_id, session, *, processors, ...):
    context_id = self._process_context_id(context_id)
    session_id = session.get_session_id() if session else "default_session_id"
    full_context_id = f"{session_id}_{context_id}"

    # 缓存命中 → 直接复用（更新 session 引用）
    if full_context_id in self._context_pool:
        context = self._context_pool.get(full_context_id)
        context.set_session_ref(session)
        self._load_state_from_session(context, session, history_messages)
        return context

    processor_instances = [
        self._create_processor(ptype, pconfig)
        for ptype, pconfig in (processors or [])]
    if token_counter is None:
        token_counter = self._select_token_counter(self._config)

    context = SessionModelContext(
        context_id, session_id, self._config,
        history_messages=history_messages or [],
        processors=processor_instances,
        token_counter=token_counter, ...)
    self._load_state_from_session(context, session, history_messages)
    self._context_pool[full_context_id] = context
    return context
```

### 3.4 溢出检测与恢复

```python
# context_engine.py L372-410
_CONTEXT_OVERFLOW_PHRASES = (
    "context length", "context window", "maximum context",
    "context limit", "prompt is too long", "prompt too long",
    "input is too long", "input too long",
)
_CONTEXT_OVERFLOW_FIELDS = (
    "message", "body", "details", "error", "errors", "response",
    "code", "type", "param", "status_code", "status", "text",
    "content", "cause", "__cause__", "__context__",
)

async def recover_from_model_exception(self, *, context_id, session, exception, streaming, stream_chunks_emitted):
    if streaming and stream_chunks_emitted > 0:
        return False  # 流式输出已发送 prefix，跳过恢复
    if not self.is_context_overflow_error(exception):
        return False
    result = await self.compress_context(context_id=context_id, session=session)
    result_code = result.get("result") if isinstance(result, dict) else result
    return result_code == "compressed"
```

### 3.5 ContextProcessor 链

**处理器基类**：

```python
class ContextProcessor(metaclass=MetaContextProcessor):
    """两个生命周期切入点：on_add_messages / on_get_context_window"""

    async def on_add_messages(self, context, messages_to_add, **kwargs):
        return None, messages_to_add

    async def trigger_add_messages(self, context, messages_to_add, **kwargs):
        return False

    async def on_get_context_window(self, context, context_window, **kwargs):
        return None, context_window

    async def trigger_get_context_window(self, context, context_window, **kwargs):
        return False
```

**已实现处理器**：

| 分类 | 处理器 | 功能 |
|------|--------|------|
| 压缩 | `FullCompactProcessor` | 全量摘要压缩（触发阈值默认 180000 tokens） |
| 压缩 | `MicroCompactProcessor` | 微压缩 |
| 压缩 | `DialogueCompressor` | 对话轮次压缩 |
| 压缩 | `RoundLevelCompressor` | 轮次级压缩 |
| 卸载 | `MessageOffloader` | 消息卸载到外部存储 |
| 卸载 | `MessageSummaryOffloader` | 摘要卸载 |
| 卸载 | `ToolResultBudgetProcessor` | 工具结果预算控制 |
| 卸载 | `ToolResultWindowProcessor` | 工具结果窗口控制 |
| 守护 | `BudgetGuardProcessor` | 预算守护 |
| Fork | `forked.support.*` | Fork 上下文支持 |

### 3.6 FullCompactProcessor 压缩流程

```python
class FullCompactProcessorConfig(BaseModel):
    trigger_total_tokens: int = 180000    # 触发压缩的 token 阈值
    compression_call_max_tokens: int = 200000  # 摘要生成的 token 预算
    messages_to_keep: int = 10            # 压缩后保留的最近消息数
    session_memory_enabled: bool = True   # 是否启用会话记忆
```

```
原始消息序列: [msg_0, msg_1, ..., msg_n]
                    │
                    ▼
           ┌─────────────────┐
           │  FullCompact    │
           │  Processor      │
           └─────────────────┘
                    │
    ┌───────────────┼───────────────┐
    ▼               ▼               ▼
 系统消息       对话历史         最近 N 条
 (保留)    → 摘要生成  →    (保留原文)
    │               │               │
    └───────────────┼───────────────┘
                    ▼
压缩后: [FULL_COMPACT_BOUNDARY, 摘要, 最近 N 条消息]
```

### 3.7 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **池化缓存** | `_context_pool: Dict[str, ModelContext]` | 按 `session_id_context_id` 复用 |
| **Token 计数器多级 fallback** | `_select_token_counter` | 本地 tokenizer → tiktoken → StringLengthCounter |
| **窗口变异器** | `_window_mutators: List[Callable]` | Provider 特定的 prompt attachment 注入点 |
| **上下文重绑定** | `rebind_context_model` | 切换 Provider 时保留历史 |
| **溢出多字段遍历** | `_CONTEXT_OVERFLOW_FIELDS` | 兼容各家 Provider 错误结构 |

---

## 四、多类型记忆系统

### 4.1 LongTermMemory 架构

**核心代码位置**：`openjiuwen/core/memory/long_term_memory.py`（1585 行）

```python
class LongTermMemory(metaclass=Singleton):
    """全局唯一记忆引擎（Singleton 模式）"""

    def __init__(self):
        # 存储后端（4 种）
        self.kv_store: BaseKVStore | None = None           # 快速结构化数据
        self.vector_store: BaseVectorStore | None = None    # 向量相似度搜索
        self.db_store: BaseDbStore | None = None            # 持久化存储
        self.message_store: BaseMessageStore | None = None  # 消息持久化
        self.memory_index: BaseMemoryIndex | None = None    # 记忆索引
        self._storage_codec: AesStorageCodec | None = None  # 加密编解码

        # 5 个记忆管理器
        self.message_manager: MessageManager
        self.fragment_memory_manager: FragmentMemoryManager  # 片段记忆
        self.variable_manager: VariableManager               # 变量
        self.write_manager: WriteManager                     # 写入协调
        self.summary_manager: SummaryManager                 # 摘要
        self.search_manager: SearchManager                   # 检索
        self.generator: Generator                            # 记忆生成器

        # LLM 与 Embedding
        self._base_llm: Model | None = None
        self._base_embed: Embedding | None = None
        self._scope_embedding: dict[str, Embedding] = {}    # scope 级缓存
```

### 4.2 五种记忆类型

```python
class MemoryType(Enum):
    USER_PROFILE = "user_profile"       # 用户画像（Vector + DB，语义搜索）
    EPISODIC_MEMORY = "episodic_memory" # 情景记忆（Vector + DB，时间+语义）
    SEMANTIC_MEMORY = "semantic_memory" # 语义记忆（Vector + DB，语义搜索）
    VARIABLE = "variable"               # 变量（KV Store，Key 精确查找）
    SUMMARY = "summary"                 # 摘要（Vector + DB，语义搜索）
```

| 记忆类型 | 存储位置 | 检索方式 | 用途 |
|----------|----------|----------|------|
| User Profile | Vector + DB | 语义搜索 | 用户偏好、习惯 |
| Episodic Memory | Vector + DB | 时间+语义 | 历史事件、经验 |
| Semantic Memory | Vector + DB | 语义搜索 | 概念、知识 |
| Variable | KV Store | Key 精确查找 | 配置、状态 |
| Summary | Vector + DB | 语义搜索 | 会话摘要 |

### 4.3 add_messages 流程（带分布式锁）

```python
# long_term_memory.py L550-687
@_fw.emit_before(MemoryEvents.MEMORY_ADDED)
async def add_messages(self, messages, agent_config, *, user_id, scope_id, session_id,
                       timestamp=None, gen_mem=True, gen_mem_with_history_msg_num=2):
    if timestamp is None:
        timestamp = datetime.now(timezone.utc)
    msg_id = "-1"
    llm = await self._get_scope_llm(scope_id)
    scope_config = await self._get_scope_config(scope_id)
    await self._apply_scope_embedding(scope_id)

    # 用户级分布式锁（防并发写入冲突）
    lock = DistributedLock(self.kv_store, f"user/{user_id}")
    async with lock:
        if not llm:
            raise build_error(...)

        history_messages = await self._get_history_messages(
            user_id, scope_id, session_id,
            history_window_size=gen_mem_with_history_msg_num)
        await self.scope_user_mapping_manager.add(user_id=user_id, scope_id=scope_id)

        # 逐条写入原始消息
        for i, msg in enumerate(messages):
            msg_timestamp = timestamp + timedelta(milliseconds=i)
            add_req = MessageAddRequest(
                user_id=user_id, scope_id=scope_id, role=msg.role,
                content=msg.content, session_id=session_id, timestamp=msg_timestamp)
            if self.message_manager:
                msg_id = await self.message_manager.add(add_req)

        if not gen_mem:
            return AddMemResult()

        check_res, messages = self._check_messages(messages=messages)
        if not check_res:
            return AddMemResult()

        # 通过 Generator 生成各类型记忆
        all_memory = await self.generator.gen_all_memory(
            scope_id=scope_id, user_id=user_id, messages=messages,
            history_messages=history_messages, session_id=session_id,
            config=agent_config, base_chat_model=llm, message_mem_id=msg_id, ...)

        # 写入各类型存储
        write_result = await self.write_manager.add_memories(
            user_id=user_id, scope_id=scope_id, memories=all_memory, llm=llm)

    return AddMemResult(
        variables=[w for w in write_result if w.mem_type.value == MemoryType.VARIABLE.value],
        user_profile=[w for w in write_result if w.mem_type.value == MemoryType.USER_PROFILE.value],
        semantic_memory=[...], episodic_memory=[...], summary=[...])
```

### 4.4 AES 加密存储

```python
# long_term_memory.py L385-408
async def set_scope_config(self, scope_id, memory_scope_config) -> bool:
    encrypted_config = copy.deepcopy(memory_scope_config)

    # API key 加密
    if encrypted_config.model_client_cfg and encrypted_config.model_client_cfg.api_key:
        encrypted_config.model_client_cfg.api_key = self._storage_codec.encode(
            encrypted_config.model_client_cfg.api_key)
    if encrypted_config.embedding_cfg and encrypted_config.embedding_cfg.api_key:
        encrypted_config.embedding_cfg.api_key = self._storage_codec.encode(
            encrypted_config.embedding_cfg.api_key)

    self._scope_config[scope_id] = encrypted_config
    config_key = f"{self.SCOPE_CONFIG_KEY}/{scope_id}"
    config_json = encrypted_config.model_dump_json(by_alias=True)
    await self.kv_store.set(config_key, config_json)
    if scope_id in self._scope_embedding:
        del self._scope_embedding[scope_id]
    return True
```

### 4.5 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **Singleton 模式** | `metaclass=Singleton` | 全局唯一记忆引擎 |
| **5 类型记忆分层** | USER_PROFILE/EPISODIC/SEMANTIC/VARIABLE/SUMMARY | 不同存储后端 + 不同检索策略 |
| **4 种存储后端** | KV/Vector/DB/Message | 按需组合，自动注册 SimpleMemoryIndex |
| **分布式锁** | `DistributedLock(kv_store, f"user/{user_id}")` | 防并发写入冲突 |
| **AES 加密** | `AesStorageCodec` | API Key / 敏感记忆加密 |
| **scope 配置** | 每个 scope 独立 LLM/Embedding | 多租户隔离 |
| **scope embedding 缓存** | `_scope_embedding` dict | 避免重复加载 |
| **历史窗口** | `gen_mem_with_history_msg_num=2` | 生成记忆时参考最近 N 条消息 |
| **可插拔索引** | `register_plugin` | 支持自定义 BaseMemoryIndex |

---

## 五、PermissionEngine 三级防护

### 5.1 三级防护架构

**核心代码位置**：`openjiuwen/harness/security/permission_engine/core.py`（445 行）

```
                     工具调用 (tool_name, tool_args)
                          │
                          ▼
        ┌─────────────────────────────────────┐
        │       PermissionEngine.check_permission()
        │       (L266-393)
        └─────────────────────────────────────┘
                          │
        ┌─────────────────┼─────────────────┐
        ▼                 ▼                 ▼
   Pipeline A        Pipeline B        Pipeline C
   tiered_policy     FileGuard         NetGuard
   (tool_policy)     (file_guard)      (net_guard)
        │                 │                 │
        └─────────────────┼─────────────────┘
                          ▼
              strictest(A, B, C)
              (tiered_policy_strictest)
                          │
                          ▼
                  PermissionLevel
              (ALLOW / ASK / DENY)
```

### 5.2 关键函数签名

```python
class PermissionEngine:
    def __init__(self, config, llm=None, model_name=None,
                 workspace_root=None, trusted_dirs=None): ...

    def update_config(self, config) -> None:
        """热更新权限配置"""

    def update_trusted_dirs(self, trusted_dirs: list[Path]) -> None:
        """更新受信任目录列表"""

    def evaluate_global_policy_directly(self, tool_name, tool_args, *,
                                        include_external_directory=True) -> tuple[...]:
        """直接评估全局策略（绕过 enabled 开关短路）"""

    async def check_permission(self, tool_name, tool_args) -> PermissionResult:
        """权限检查主入口（串行执行 Pipeline A → B → C）"""
```

### 5.3 check_permission 主入口

```python
# core.py L266-393
async def check_permission(self, tool_name: str, tool_args: dict) -> PermissionResult:
    if not self._enabled:
        return PermissionResult(permission=PermissionLevel.ALLOW,
                                reason="Permission system is disabled")

    # Pipeline A: 工具规则（含内置规则）
    external_paths: list[str] | None = None
    permission, matched_rule = self.evaluate_global_policy_directly(
        tool_name, tool_args, include_external_directory=False)
    if permission is None:
        permission = PermissionLevel.ASK
        matched_rule = "default"

    # Pipeline B: 文件路径防护
    if self._file_guard is not None:
        path_result = self._file_guard.evaluate(tool_name, tool_args)
        if path_result is not None:
            permission = tiered_policy_strictest(permission, path_result.permission)
            matched_rule = f"{matched_rule}|{path_result.matched_rule or 'file_guard'}"
            external_paths = path_result.external_paths

    # Pipeline C: 网络防护
    if self._net_guard is not None:
        net_result = self._net_guard.evaluate(tool_name, tool_args)
        if net_result is not None:
            permission = tiered_policy_strictest(permission, net_result.permission)
            matched_rule = f"{matched_rule}|{net_result.matched_rule or 'net_guard'}"

    return PermissionResult(permission=permission, matched_rule=matched_rule,
                           reason=self._get_reason(permission, tool_name, matched_rule),
                           external_paths=external_paths)
```

### 5.4 三态权限严格度比较

```python
class PermissionLevel(Enum):
    ALLOW = "allow"   # 允许执行
    ASK = "ask"       # 询问用户
    DENY = "deny"     # 拒绝执行

def strictest(a: PermissionLevel, b: PermissionLevel) -> PermissionLevel:
    # DENY > ASK > ALLOW
    if a == PermissionLevel.DENY or b == PermissionLevel.DENY:
        return PermissionLevel.DENY
    if a == PermissionLevel.ASK or b == PermissionLevel.ASK:
        return PermissionLevel.ASK
    return PermissionLevel.ALLOW
```

### 5.5 Shell AST 解析（双后端）

**核心代码位置**：`openjiuwen/harness/security/permission_engine/toolguard/shell_ast.py`（386 行）

```python
# shell_ast.py L82-107
def parse_shell_for_permission(command: str) -> ShellAstParseResult:
    text = canonicalize_shell_command_for_permission((command or "").strip())
    if not text:
        return ShellAstParseResult(kind="simple", backend="fallback")

    parser = _get_tree_sitter_bash_parser()
    if parser is not None:
        try:
            return _parse_with_tree_sitter(text, parser)
        except Exception:
            logger.warning("[PermissionEngine] permission.shell_ast.parse_failed")

    return _parse_with_conservative_fallback(text)  # fail closed
```

**tree-sitter 解析器懒加载**：

```python
# shell_ast.py L110-132
def _get_tree_sitter_bash_parser():
    global _TREE_SITTER_BASH_READY, _TREE_SITTER_PARSER
    if _TREE_SITTER_BASH_READY is False:
        return None  # 已确认不可用，跳过
    try:
        from tree_sitter import Language, Parser
        import tree_sitter_bash
        language = Language(tree_sitter_bash.language())
        parser = Parser(language)
        _TREE_SITTER_PARSER = parser
        _TREE_SITTER_BASH_READY = True
        return parser
    except Exception:
        _TREE_SITTER_BASH_READY = False
        return None
```

**保守扫描器（fail-closed）**：

```python
# shell_ast.py L135-160
def _parse_with_conservative_fallback(command: str) -> ShellAstParseResult:
    flags = _scan_shell_structure(command)
    if flags.has_risky_structure():
        # fail closed：检测到风险结构时返回 parse_unavailable
        return ShellAstParseResult(
            kind="parse_unavailable",
            flags=flags,
            reason="tree-sitter backend unavailable and fallback detected shell structure",
            backend="fallback")
    try:
        argv = tuple(shlex.split(command, posix=(os.name != "nt")))
    except ValueError:
        return ShellAstParseResult(kind="parse_unavailable", flags=flags, ...)
    subcommand = ShellSubcommand(text=command, argv=argv)
    return ShellAstParseResult(kind="simple", subcommands=(subcommand,))
```

**风险结构标志**：

```python
@dataclass(frozen=True)
class ShellStructureFlags:
    has_compound_operators: bool = False    # && || ;
    has_pipeline: bool = False              # |
    has_subshell: bool = False              # ()
    has_command_group: bool = False         # {}
    has_command_substitution: bool = False  # $() ``
    has_process_substitution: bool = False  # <() >()
    has_parameter_expansion: bool = False   # ${
    has_heredoc: bool = False               # << <<<
    has_input_redirection: bool = False     # <
    has_output_redirection: bool = False    # > >>

    def has_risky_structure(self) -> bool:
        return any((self.has_compound_operators, self.has_pipeline, ...))
```

### 5.6 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **三态策略** | `ALLOW/ASK/DENY` | DENY > ASK > ALLOW，三层串行取最严格 |
| **三级防护管线** | tiered_policy → file_guard → net_guard | 每层独立可关闭 |
| **配置热更新** | `update_config()` | 运行时切换权限规则无需重启 |
| **tree-sitter 双后端** | bash 语法树 + 保守扫描器 | 不可用时降级为保守扫描器（fail-closed） |
| **敏感路径加密** | `AesStorageCodec` | API Key / 加密存储 |
| **敏感路径自动合并** | `merge_package_sensitive_paths` | 旧 host YAML 自动补齐内置路径规则 |

---

## 六、TeamAgent 双 Spawn

### 6.1 完整架构

**核心代码位置**：`openjiuwen/agent_teams/agent/team_agent.py`（1756 行）

```
TeamAgent (BaseAgent)
├─ _configurator: AgentConfigurator              ── 装配蓝图
├─ _state: TeamAgentState                        ── 运行时状态
├─ _spawn_manager: SpawnManager                  ── 子 Agent 派生
├─ _recovery_manager: RecoveryManager            ── 故障恢复
├─ _session_manager: SessionManager              ── 会话管理
├─ _stream_controller: StreamController          ── 流控
├─ _coordination: CoordinationKernel             ── 协调内核
│   └─ event_bus: EventBus
└─ _named_checkpoints: Dict[str, dict]           ── 命名检查点
```

### 6.2 In-Process Spawn（协程级）

**核心代码位置**：`openjiuwen/agent_teams/spawn/inprocess_spawn.py`（160 行）

```python
async def inprocess_spawn(team_agent, ctx, *, initial_message=None, session_id=None, fork_from=None):
    """Spawn a teammate as a coroutine (asyncio.Task) within the current event loop."""
    spec = team_agent.spec
    agent_spec = spec.agents.get(ctx.role.value) or spec.agents["leader"]
    card = agent_spec.card or AgentCard(...)
    teammate = _TeamAgent(card)

    # 共享 workspace cache（避免重复扫描）
    team_agent.share_workspace_cache_with(teammate)
    teammate.configure(spec, ctx)

    # 共享 checkpoint 字典（引用传递）
    team_agent.share_checkpoints_with(teammate)
    if teammate.team_backend is not None:
        teammate.team_backend.set_store_checkpoint_fn(team_agent.set_checkpoint)

    # Fork 上下文注入
    if fork_from and not fork_from.is_empty():
        native = teammate.resources.harness.get_deep_agent()
        await native.create_new_context_engine(
            session_id=session_id,
            messages=fork_from.to_messages())
        if fork_from.compact_split is not None:
            from openjiuwen.agent_teams.fork_compact import compact_context
            await compact_context(native, split_at=fork_from.compact_split,
                                  session_id=session_id,
                                  direction=fork_from.compact_direction)

    # contextvars 复制 → asyncio.Task 启动
    run_ctx = contextvars.copy_context()
    async def _run():
        if session_id:
            set_session_id(session_id)
        return await Runner.run_agent_team(teammate, inputs, member=True, session=session_id)

    task = run_ctx.run(asyncio.get_running_loop().create_task, _run())
    handle = InProcessSpawnHandle(process_id=f"inproc-{member_name}",
                                  _task=task, agent_ref=teammate)
    return handle
```

### 6.3 External CLI Spawn（进程级）

```python
# openjiuwen/agent_teams/spawn/external_cli_spawn.py
# 通过 Runner.spawn_agent → child_process 模式
spawn_config = SpawnConfig(
    command="openjiuwen",
    args=["--member", member_name],
    env={...},
)
```

### 6.4 Fork 上下文

**核心代码位置**：`openjiuwen/agent_teams/fork.py`

```python
@dataclass
class ForkContext:
    """可序列化的对话历史快照（可跨进程传输）"""
    messages: list[dict]
    compact_split: int | None = None     # 压缩分割点
    compact_direction: str = "before"    # 保留方向

    @classmethod
    def from_agent(cls, agent, *, session_id=None, checkpoint=None, keep="before"):
        msgs = agent.get_current_context(session_id=session_id)
        # 剥离 SystemMessage（防止角色泄露）
        msgs = [m for m in msgs if not isinstance(m, SystemMessage)]

        if checkpoint is not None:
            if keep == "after":
                msgs = cls._trim_leading_orphan_tool_messages(msgs[checkpoint:])
            else:
                truncated = msgs[:checkpoint]
                last = truncated[-1] if truncated else None
                # 携带边界处的 ToolMessage（避免悬空 tool call）
                if isinstance(last, AssistantMessage) and getattr(last, "tool_calls", None):
                    i = checkpoint
                    while i < len(msgs) and isinstance(msgs[i], ToolMessage):
                        truncated.append(msgs[i])
                        i += 1
                msgs = truncated
        return cls(messages=[encode_message(m) for m in msgs])
```

### 6.5 团队协调内核

```python
class CoordinationKernel:
    """事件分发、成员生命周期、任务分配与追踪"""

    def __init__(self, team_agent):
        self.team_agent = team_agent
        self.event_bus = EventBus()  # 事件总线（实现观察者模式）
        # 调度器、生命周期管理、Mailbox 消息路由
```

### 6.6 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **组合优于继承** | TeamAgent 组合 6 个 manager | 各 manager 单一职责，易于测试 |
| **双 Spawn 机制** | InProcess（asyncio.Task）/ External CLI（子进程） | 同进程内协程级 + 跨进程级 |
| **Fork 上下文** | 可序列化的对话快照 | 子 Agent 继承父 Agent 历史，跨进程可传输 |
| **Workspace Cache 共享** | `share_workspace_cache_with` | 避免父子 Agent 重复扫描 |
| **Checkpoint 引用传递** | `share_checkpoints_with` | 字典引用共享，子 Agent 写入即父可见 |
| **contextvars 复制** | `contextvars.copy_context()` | 会话 ID 等 contextvar 正确传播到新 task |
| **成员判定** | `member=True` 跳过 activate/dispatch | 派生成员不进 leader 池 |

---

## 七、Pregel 图执行

### 7.1 BSP 执行模型

**核心代码位置**：`openjiuwen/core/graph/pregel/engine.py`（277 行）

```
Superstep 1: 所有起始节点并行（START 节点激活）
        │
        ▼  (Channel.flush)
Superstep 2: 接收消息的节点并行
        │
        ▼  (Channel.flush)
Superstep 3: ...
        │
        ▼  (无 active node 或 manager.is_empty())
END
```

### 7.2 关键函数签名

```python
class PregelLoop:
    def __init__(self, graph: Pregel, config: PregelConfig):
        self.graph = graph
        self.manager = ChannelManager(graph.channels)
        self.step: int = 0
        self.max_step: int = 0
        self.active_nodes: List[str] = []
        self.executor: TaskExecutorPool | None = None
        self._retry_pending_nodes: Dict[str, PendingNode] = {}
        self.node_version: Dict[str, int] = defaultdict(int)

    async def init(self) -> None:
        """初始化：恢复检查点 / 触发 START 节点"""

    async def run_step(self) -> bool:
        """单步执行入口（包装 _run_step + 错误状态保存）"""

    async def _run_step(self) -> bool:
        """单步执行核心：ready nodes → 提交 → 等待 → flush"""

    async def _save_state_on_error(self, exception: Exception):
        """错误时保存 GraphState（含 pending_node / pending_buffer）"""


class Pregel:
    def __init__(self, nodes, channels, initial=START, store=None, after_step=None): ...

    async def run(self, config: Optional[PregelConfig] = None):
        """主入口：init → while loop.run_step()"""
```

### 7.3 BSP 单步执行

```python
# engine.py L88-163
async def _run_step(self) -> bool:
    graph_logger.debug(f"Start to run graph super-step[{self.step}]", ...)

    # 1. Determine tasks for this round
    tasks_to_run = []
    if self._retry_pending_nodes:
        self.active_nodes = list(self._retry_pending_nodes.keys())
        self._retry_pending_nodes.clear()
    else:
        ready_nodes = self.manager.get_ready_nodes()
        self.active_nodes = []
        for n in ready_nodes:
            if n in self.graph.nodes and n != END:
                self.active_nodes.append(n)
                self.node_version[n] += 1

    if not self.active_nodes:
        if self.manager.is_empty():
            return False  # End: 无节点可激活 + 通道清空
        self.manager.flush()
        self.step += 1
        return True

    if self.step > self.max_step:
        raise RecursionError(f"Recursion limit of {self.max_step} reached at step {self.step}")

    for name in self.active_nodes:
        self.manager.consume(name)       # 消费节点输入
        tasks_to_run.append(self.graph.nodes[name])

    # 2. Execute tasks (parallel)
    for node in tasks_to_run:
        self.executor.submit(node, self.node_version[node.name])
    await self.executor.wait_all()

    # 3. Summarize results
    for msg in self.executor.succeed_messages:
        self.manager.buffer_message(msg)
    self.manager.flush()
    self.executor.clear()

    # Hook
    if self.graph.after_step:
        callback = self.graph.after_step
        if asyncio.iscoroutinefunction(callback):
            await callback(self)
        else:
            callback(self)
    self.step += 1
    return True
```

### 7.4 主循环与异常恢复

```python
# engine.py L222-277
async def run(self, config: Optional[PregelConfig] = None):
    inner_config: InnerPregelConfig = create_inner_config(config or DEFAULT_PREGEL_CONFIG)
    is_top_level = not inner_config.get(PARENT_NS)

    loop = PregelLoop(self, inner_config)
    try:
        await loop.init()
        await trigger(WorkflowEvents.LOOP_STARTED, graph_id=inner_config.get(NS))
        while await loop.run_step():
            pass
        await trigger(WorkflowEvents.LOOP_FINISHED, ...)
        return {}
    except GraphInterrupt as e:
        await trigger(WorkflowEvents.LOOP_FINISHED, ...)
        if is_top_level:
            return {TASK_STATUS_INTERRUPT: e.value}
        else:
            raise e
```

### 7.5 状态保存与恢复

```python
# engine.py L37-60
async def init(self) -> None:
    self.executor = TaskExecutorPool(self.config)
    self.max_step = self.config[RECURSION_LIMIT]
    state = None
    if self.config.get(SESSION_ID) and self.config.get(NS) and self.saver:
        state = await self.saver.get(self.config[SESSION_ID], self.config[NS])

    if self._is_resume(state):
        # 恢复屏障通道 + 节点版本 + step
        self.manager.restore(state.channel_values)
        self.node_version = state.node_version
        self.step = state.step
        self.max_step = state.step + self.config[RECURSION_LIMIT]
        # 恢复 pending_buffer 中的待处理消息
        for msg in state.pending_buffer:
            self.manager.buffer_message(msg)
        if state.pending_node:
            self._retry_pending_nodes = state.pending_node
    else:
        # 触发起始节点
        self.manager.buffer_message(TriggerMessage(sender=self.graph.initial,
                                                   target=self.graph.initial))
        self.manager.flush()
```

### 7.6 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **BSP 同步屏障** | `_run_step` flush | 每个 superstep 内所有节点并行执行，下一个 superstep 开始前全部完成 |
| **通道模型** | `ChannelManager` | 节点间通过 Channel 传递消息，避免显式边依赖 |
| **节点版本号** | `node_version` | 节点多次激活时记录版本号（处理循环） |
| **状态持久化** | `saver.save/load` | 支持中断恢复（step + channel_values + pending_node + pending_buffer） |
| **递归深度限制** | `RECURSION_LIMIT` | 防止死循环 |
| **end 检测** | `manager.is_empty() and not active_nodes` | 双重终止条件 |
| **错误保存** | `_save_state_on_error` | 即便异常也持久化状态，便于下次重试 |

---

## 八、Rails 铁轨

### 8.1 铁轨机制

铁轨是 openJiuwen 最强大的扩展机制，允许在不修改核心代码的情况下注入横切关注点。

```python
@rail(
    before=AgentCallbackEvent.BEFORE_MODEL_CALL,
    after=AgentCallbackEvent.AFTER_MODEL_CALL,
    on_exception=AgentCallbackEvent.ON_MODEL_EXCEPTION,
)
async def _railed_model_call(self, ctx: AgentCallbackContext) -> AssistantMessage:
    # 实际 LLM 调用逻辑
    ...
```

### 8.2 标准事件序列

```
BEFORE_INVOKE → BEFORE_MODEL_CALL → [LLM调用] → AFTER_MODEL_CALL →
BEFORE_TOOL_CALL → [工具执行] → AFTER_TOOL_CALL → AFTER_REACT_ITERATION →
... → AFTER_INVOKE
```

### 8.3 铁轨事件枚举

```rust
// laew 借鉴参考
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCallbackEvent {
    BeforeInvoke,
    AfterInvoke,
    BeforeModelCall,
    AfterModelCall,
    BeforeToolCall,
    AfterToolCall,
    AfterReactIteration,
    OnUserMessage,
}
```

### 8.4 铁轨 trait 设计（laew 借鉴）

```rust
pub trait Rail: Send + Sync {
    fn before_events(&self) -> Vec<AgentCallbackEvent>;
    fn after_events(&self) -> Vec<AgentCallbackEvent>;
    async fn on_before(&self, ctx: &mut AgentCallbackContext, event: AgentCallbackEvent) -> Result<()>;
    async fn on_after(&self, ctx: &mut AgentCallbackContext, event: AgentCallbackEvent) -> Result<()>;
}
```

---

## 九、OTLP Trajectory

### 9.1 Trajectory 不可变值对象

**核心代码位置**：`openjiuwen/agent_evolving/trajectory/model.py`（276 行）

```python
class Trajectory:
    """不可变值对象，拥有单一 OTLP 轨迹有效载荷"""

    __slots__ = ("_payload", "_sealed")

    def __init__(self, payload: Mapping[str, object], *, _allow_missing_session: bool = False):
        if not isinstance(payload, Mapping):
            raise TypeError("trajectory payload must be a mapping")
        resource_spans = payload.get("resourceSpans")
        if not isinstance(resource_spans, list) or not resource_spans:
            raise ValueError("trajectory payload must contain non-empty resourceSpans")
        # ... 校验 attributes 含 trajectory_id
        if not _allow_missing_session:
            has_team_or_member = any(key in attributes for key in (TEAM_ID, MEMBER_ID))
            has_session = SESSION_ID in attributes
            if has_team_or_member and not has_session:
                raise ValueError("team_id/member_id requires session_id")

        # 深拷贝 + 密封
        object.__setattr__(self, "_payload", _copy_json(payload))
        object.__setattr__(self, "_sealed", True)

    @property
    def trajectory_id(self) -> str:
        return str(self.resource_attributes[TRAJECTORY_ID])

    def to_otlp(self) -> dict[str, object]:
        """返回独立的 JSON-like 副本"""
        return _copy_json(self._payload)

    def __setattr__(self, name: str, value: Any) -> None:
        if getattr(self, "_sealed", False):
            raise AttributeError("Trajectory is immutable")
        object.__setattr__(self, name, value)
```

### 9.2 轨迹事件

```python
class TrajectoryEvents:
    AGENT_INVOKE = "agent.invoke"
    AGENT_STREAM = "agent.stream"
    TOOL_CALL = "tool.call"
    TOOL_RESULT = "tool.result"
    LLM_CALL = "llm.call"
    LLM_RESPONSE = "llm.response"
    CONTEXT_RETRIEVED = "context.retrieved"
    CONTEXT_COMPRESSED = "context.compressed"
    MEMORY_ADDED = "memory.added"
    MEMORY_SEARCHED = "memory.searched"
```

### 9.3 Span 上下文

```python
class SpanContext:
    trace_id: str
    span_id: str
    parent_span_id: Optional[str]
    attributes: Dict[str, Any]

    def set_attribute(self, key, value):
        self.attributes[key] = value

    def add_event(self, name, attributes=None):
        ...
```

### 9.4 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **OTLP 标准格式** | `Trajectory._payload` | 轨迹采用 OpenTelemetry Protocol JSON |
| **不可变 Trajectory** | `__slots__` + `_sealed` | 防止意外修改，支持安全共享 |
| **FastAPI 独立服务** | `build_rl_service_app` | 解耦训练与推理，可独立部署 |
| **verl 集成** | `RayPPOTrainer` 继承 | 基于华为开源 verl 框架（Ray + PPO/GRPO） |
| **REMAX baseline** | `compute_baseline` | 无 critic 时用 REMAX 估计 baseline |
| **rollout 资源管理** | `sleep_rollout` / `wake_up_rollout` | 训练/推理 GPU 资源复用 |
| **两阶段 RL** | offline + online | offline 离线批量学习 + online 在线渐进 |

---

## 十、RL 训练

### 10.1 进化系统架构

```
agent_evolving/
├── agent_rl/                # RL 训练
│   ├── online/service.py    # 在线 RL 服务（FastAPI）
│   ├── offline/             # 离线学习
│   ├── rl_trainer/
│   │   ├── verl_executor.py # verl RayPPOTrainer 包装（615 行）
│   │   └── verl_converter.py
│   └── reward/              # 奖励函数
├── trajectory/              # 轨迹收集
│   ├── model.py             # Trajectory 不可变值对象（276 行）
│   ├── schema.py
│   └── spans.py
├── optimizer/               # 优化器
├── evaluator/               # 评估器
├── experience/              # 经验回放
└── prompts/                 # 提示词进化
```

### 10.2 RL 训练服务（FastAPI）

**核心代码位置**：`openjiuwen/agent_evolving/agent_rl/online/service.py`

```python
def build_rl_service_app(*, model_id, redis, trajectory_store,
                         task_registry, capture_pipeline,
                         training_runner, trajectory_api):
    app = FastAPI(title="OpenJiuwen RL Service", lifespan=lifespan)

    @app.post("/v1/rl/tasks/start")
    async def start_task(payload, agent_session_id, gateway_task_id, ...):
        spec = TaskSpec(rl_task_id=task_id, agent_session_id=...,
                        model_id=model_id, policy_lora_name=policy_name, ...)
        result = await task_registry.start(spec)
        return JSONResponse(status_code=201, content=result.task.to_dict())

    @app.post("/v1/rl/tasks/{rl_task_id}/reward")
    async def reward_task(rl_task_id, payload):
        return {"sample_count":
                await capture_pipeline.submit_reward(rl_task_id, payload["reward"])}

    @app.post("/v1/rl/training/runs")
    async def start_training_run(payload):
        result = await training_runner.start()
        return JSONResponse(status_code=201, content=_record(result.run))
```

### 10.3 PPO/GRPO 训练执行

**核心代码位置**：`openjiuwen/agent_evolving/agent_rl/rl_trainer/verl_executor.py`（615 行）

```python
class BaseVerlTrainingExecutor(RayPPOTrainer):
    """Extends verl's RayPPOTrainer to provide full PPO/GRPO training pipeline"""

    def __init__(self, config, tokenizer, processor, role_worker_mapping,
                 resource_pool_manager, ray_worker_group_cls, reward_fn,
                 val_reward_fn, train_dataset, val_dataset, collate_fn, train_sampler):
        super().__init__(...)  # verl RayPPOTrainer
        self.mini_batch_size = config.actor_rollout_ref.actor.ppo_mini_batch_size
        self.global_steps = 0
        if self.use_reference_policy:
            logger.info("Reference policy ENABLED for KL computation")
        else:
            logger.warning("Reference policy NOT available. KL penalty will be ineffective.")

    @abstractmethod
    def sleep_rollout(self):
        """释放 rollout GPU 资源供训练使用"""

    @abstractmethod
    def wake_up_rollout(self) -> list:
        """重新获取 rollout 资源并返回 vLLM server 地址"""

    def compute_baseline(self, origin_batch, batch):
        """REMAX-style baseline（仅当 adv_estimator == REMAX）"""
        if self.config.algorithm.adv_estimator == AdvantageEstimator.REMAX:
            remax_input = deepcopy(origin_batch)
            remax_input.meta_info["do_sample"] = False
            remax_output = self.actor_rollout_wg.generate_sequences(remax_input)
            batch = batch.union(remax_output)
            r_baseline = self.reward_fn(batch).sum(dim=-1)
            batch.batch["reward_baselines"] = r_baseline
        return batch

    def compute_reward(self, batch, metrics):
        """准备 response masks 并计算 RM scores"""
        batch.non_tensor_batch["uid"] = batch.non_tensor_batch["data_id_list"]
        resp_mask = compute_response_mask(batch)
        # ... KL penalty, advantage estimation, actor/critic loss
```

### 10.4 关键 RL 训练子步骤

1. `compute_baseline`：REMAX baseline
2. `compute_reward`：奖励 + 响应掩码
3. `compute_old_logprob`：旧策略 log 概率
4. `compute_ref_logprob`：参考策略 log 概率（KL 计算）
5. `compute_values`：价值估计
6. `compute_advantage`：优势估计（GAE 等）
7. `update_actor`：actor 网络更新
8. `update_critic`：critic 网络更新
9. 指标记录 + 检查点保存

### 10.5 训练数据流

```
Agent 执行 → CapturePipeline 捕获轨迹 → TrajectoryStore 存储
                    │
                    ▼
         TrainingRunner 采样批次 → 策略更新 → LoRA 权重
                    │
                    ▼
         新策略部署 → Agent 继续执行（循环）
```

---

## 十一、对 laew 的借鉴

### 11.1 P0 优先级（必须实现）

| 借鉴机制 | 具体改造 | 工作量 | 预期收益 |
|----------|----------|--------|----------|
| **铁轨装饰器** | `src/agent/rail.rs` 增加 `Rail` trait，支持 `before_model_call` / `after_model_call` / `on_exception` 注册；MultiAgentOrchestrator 集成 | 5d | 可扩展性大幅提升（日志/限流/缓存可插拔） |
| **三态权限引擎** | `src/agent/security/permission.rs` 实现 `PermissionLevel::{Allow, Ask, Deny}` + 三级防护管线（ToolGuard/FileGuard/NetGuard） | 7d | 安全性基础保障 |
| **上下文溢出自动重试** | 在 `LlmClient` 调用包装层检测 `context_length_exceeded`，触发 ContextEngine 压缩后重试 | 3d | 长对话稳定性 |
| **Token 计数器 fallback 链** | `src/agent/token_counter.rs`：本地 tokenizer → tiktoken-rs → 字符串长度兜底 | 2d | 精确控制上下文窗口 |
| **ContextEngine 池化** | `session.rs` 增加 `_context_pool: HashMap<String, ModelContext>` 缓存复用 | 2d | 减少上下文重建开销 |

### 11.2 P1 优先级（应该实现）

| 借鉴机制 | 具体改造 | 工作量 | 预期收益 |
|----------|----------|--------|----------|
| **Lane-based 资源调度** | `ToolRegistry::execute_batch` 按资源 key 分 lane（文件路径归一化） | 4d | 解决 read-modify-write 冲突 |
| **渐进式工具暴露** | 当工具数 > 20 时仅传 `tool_search` + 工具摘要 | 3d | 避免工具过多导致上下文溢出 |
| **Shell AST 解析** | `tree_sitter_bash` crate 集成 + 保守扫描器兜底 | 5d | Bash 权限检查准确率提升 |
| **5 类型记忆分层** | SQLite 增加 `semantic_memory` / `episodic_memory` 表 + AES 加密 | 7d | 跨会话知识积累 |
| **Fork 上下文序列化** | SubAgent 委派前序列化父 context（含 `<<<LAEW:FORK_BOUNDARY>>>` 标记） | 4d | 子 Agent 上下文连续性 |
| **TTFT/TPOT 指标** | TUI 状态栏显示实时延迟 | 1d | 用户体验提升 |

### 11.3 P2 优先级（可选实现）

| 借鉴机制 | 具体改造 | 工作量 | 预期收益 |
|----------|----------|--------|----------|
| **Workflow BSP 执行** | `src/agent/workflow.rs` 实现 BSP 图执行引擎 | 14d | 支持复杂任务编排 |
| **TeamAgent 双 Spawn** | `SpawnMode::{InProcess, ExternalCli}`，支持 Tokio task + 子进程 | 7d | 多 Agent 协作灵活性 |
| **OpenTelemetry 集成** | `opentelemetry` crate 集成，标准 Span 事件 | 5d | 调试与监控能力 |
| **Trajectory 收集** | `trajectory` 表存储 OTLP 格式轨迹 | 4d | 为 RL 训练铺路 |
| **Reward 函数** | `Reward` trait，初期复用 QC 评分 | 3d | 自我进化基础 |
| **CoordinationKernel** | 多 Agent 协调内核（事件总线 + 生命周期） | 10d | 复杂多 Agent 任务 |

### 11.4 实现顺序建议

```
Phase 1 (P0 基础, 1-2 个月)
  ├─ 1. Rail 装饰器 + MultiAgentOrchestrator 集成
  ├─ 2. PermissionEngine 三态 + 三级防护
  ├─ 3. ContextEngine 池化 + Token 计数器
  └─ 4. 上下文溢出自动重试

Phase 2 (P1 增强, 2-3 个月)
  ├─ 5. Lane-based 资源调度
  ├─ 6. 渐进式工具暴露
  ├─ 7. Shell AST 解析
  ├─ 8. 5 类型记忆分层（含加密）
  └─ 9. Fork 上下文序列化

Phase 3 (P2 高级, 3-6 个月)
  ├─ 10. Workflow BSP 执行
  ├─ 11. TeamAgent 双 Spawn
  ├─ 12. OpenTelemetry 集成
  └─ 13. Trajectory + Reward
```

### 11.5 laew 现状与 agent-core 对比表

| 维度 | agent-core | laew | 差距 |
|------|------------|------|------|
| Agent 类型 | ReAct/Workflow/Team/Deep | Yolo/Plan/Main-Work/Sub-Work/QC/SessionContext | laew 已有 6 角色，但缺统一基类 |
| 工具系统 | AbilityManager + MCP | Bash/Read/Write | laew 缺 Lane 调度、渐进式暴露 |
| 记忆系统 | 5 类型 + 向量检索 | session_memory 表 | laew 严重落后 |
| 安全控制 | 三级权限引擎 | 零校验 | laew 严重落后 |
| 上下文管理 | ContextEngine + 压缩 | 简单消息列表 | laew 落后 |
| 可观测性 | OpenTelemetry | 无 | laew 落后 |
| 进化能力 | RL 训练 + Prompt 优化 | 无 | laew 缺 |
| 铁轨机制 | 完整 | 无 | laew 落后 |
| 中断恢复 | 工作流/工具双中断 | 无 | laew 落后 |

### 11.6 核心 Rust 实现参考

#### 铁轨 trait

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCallbackEvent {
    BeforeInvoke, AfterInvoke,
    BeforeModelCall, AfterModelCall,
    BeforeToolCall, AfterToolCall,
    AfterReactIteration, OnUserMessage,
}

pub trait Rail: Send + Sync {
    fn before_events(&self) -> Vec<AgentCallbackEvent>;
    fn after_events(&self) -> Vec<AgentCallbackEvent>;
    async fn on_before(&self, ctx: &mut AgentCallbackContext, event: AgentCallbackEvent) -> Result<()>;
    async fn on_after(&self, ctx: &mut AgentCallbackContext, event: AgentCallbackEvent) -> Result<()>;
}
```

#### 权限引擎

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionLevel { Allow, Ask, Deny }

pub struct PermissionEngine {
    tool_rules: Vec<ToolRule>,
    file_guard: FileGuard,
    net_guard: NetGuard,
}

impl PermissionEngine {
    pub async fn check_permission(&self, tool_name: &str, tool_args: &Value) -> PermissionResult {
        let result_a = self.evaluate_tool_policy(tool_name, tool_args);
        let result_b = self.file_guard.evaluate(tool_name, tool_args);
        let result_c = self.net_guard.evaluate(tool_name, tool_args);
        strictest(result_a, result_b, result_c)
    }
}
```

#### ContextEngine 池化

```rust
pub struct ContextEngine {
    context_pool: HashMap<String, ModelContext>,
    token_counter: Arc<dyn TokenCounter>,
    processors: Vec<Box<dyn ContextProcessor>>,
}

#[async_trait]
pub trait ContextProcessor: Send + Sync {
    async fn trigger_get_context_window(&self, ctx: &ModelContext, window: &ContextWindow) -> bool;
    async fn on_get_context_window(&self, ctx: &ModelContext, window: ContextWindow) -> (Option<ContextEvent>, ContextWindow);
}
```

#### 多类型记忆

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryType { UserProfile, Episodic, Semantic, Variable, Summary }

pub struct LongTermMemory {
    kv_store: Arc<dyn KvStore>,
    vector_store: Arc<dyn VectorStore>,
    db_store: Arc<dyn DbStore>,
}

impl LongTermMemory {
    pub async fn add_messages(&self, messages: &[Message], scope_id: &str, user_id: &str) -> Result<AddMemResult> {
        // 1. 获取分布式锁
        // 2. 生成记忆（通过 Generator）
        // 3. 写入各类型存储
    }

    pub async fn search(&self, query: &str, scope_id: &str, memory_types: &[MemoryType], top_k: usize) -> Vec<MemResult> {
        // 混合检索：向量 + 关键词
    }
}
```

#### 双 Spawn 机制

```rust
pub enum SpawnMode { InProcess, ExternalCli }

pub struct TeamAgent {
    members: HashMap<String, MemberHandle>,
    coordination: CoordinationKernel,
    spawn_manager: SpawnManager,
}

impl TeamAgent {
    pub async fn spawn_member(&self, role: &str, fork_context: Option<ForkContext>) -> Result<MemberHandle> {
        // 1. 创建子 Agent
        // 2. 共享 workspace 缓存
        // 3. Fork 上下文注入
        // 4. 启动 Tokio task 或子进程
    }
}
```

#### Trajectory 收集

```rust
pub struct Trajectory {
    states: Vec<State>,
    actions: Vec<Action>,
    rewards: Vec<f64>,
}

pub struct ExperienceBuffer {
    buffer: VecDeque<(Trajectory, f64)>,
    capacity: usize,
}

pub struct Trainer {
    model: Arc<dyn Model>,
    optimizer: Arc<dyn Optimizer>,
}

impl Trainer {
    pub async fn train_step(&mut self, trajectory: Trajectory) -> Result<f64> {
        // 1. 计算奖励
        // 2. 存储经验
        // 3. 采样批次
        // 4. 更新策略
    }
}
```

---

## 总结

agent-core 在 10 个核心机制上展现出工业级成熟度：

1. **ReAct 循环** - 通过 `@rail` 装饰器 + `force_finish` + `steering queue` + 上下文溢出自动重试实现工业级鲁棒性
2. **ContextEngine** - 多级 Token 计数器 fallback + 池化缓存 + 处理器链
3. **PermissionEngine** - 三级防护管线 + 三态策略 + tree-sitter shell AST 解析（fail-closed）
4. **AbilityManager** - Lane-based 资源调度（防 read-modify-write）+ 并行安全声明 + 渐进式暴露
5. **TeamAgent** - 组合模式 + In-Process/External CLI 双 Spawn + Fork 上下文序列化
6. **Pregel 图执行** - BSP 同步屏障 + 通道模型 + 状态持久化 + 节点版本号
7. **LongTermMemory** - 5 类型记忆分层 + 4 种存储后端 + 分布式锁 + AES 加密
8. **Agent 进化** - OTLP Trajectory 不可变对象 + FastAPI RL 服务 + verl RayPPOTrainer 集成
9. **Rails 铁轨** - 可插拔的钩子链，职责清晰
10. **OTLP Trajectory** - 全链路可观测性

对 laew 而言，P0 的 5 项借鉴（铁轨/权限/上下文引擎/Token 计数器/溢出重试）能在 1-2 个月内显著提升系统能力，是最具 ROI 的改造方向。

---

**分析完成日期**：2026-09-05
**分析人**：Claude Code Agent
**源码版本**：openJiuwen Core 0.1.17（HEAD 截至分析时）
**已读取关键文件**：
- `openjiuwen/core/single_agent/agents/react_agent.py` (3251 行)
- `openjiuwen/core/single_agent/ability_manager.py` (1512 行)
- `openjiuwen/core/context_engine/context_engine.py` (726 行)
- `openjiuwen/agent_teams/agent/team_agent.py` (1756 行)
- `openjiuwen/core/graph/pregel/engine.py` (277 行)
- `openjiuwen/core/memory/long_term_memory.py` (1585 行)
- `openjiuwen/harness/security/permission_engine/core.py` (445 行)
- `openjiuwen/harness/security/permission_engine/toolguard/shell_ast.py` (386 行)
- `openjiuwen/agent_teams/spawn/inprocess_spawn.py` (160 行)
- `openjiuwen/agent_evolving/trajectory/model.py` (276 行)
- `openjiuwen/agent_evolving/agent_rl/rl_trainer/verl_executor.py` (615 行)

---

## 第八轮深挖 — 企业级抽象层级最高的 Agent 框架 + 双层 Guardrail + 父子 Session + 进程全局 i18n

> 调研时间：2026-09-07。第八轮在第七轮基础上补充华为 openjiuwen 框架的真实实现。所有引用路径均为绝对路径 + 行号。

### 1. 整体架构（补充）

`openjiuwen/` 主包 12 个子包：
- `core/` 12 个子包：`application/`、`common/`、`context_engine/`、`controller/`、`foundation/`（LLM 客户端、工具抽象）、`graph/`（图记忆）、`kv_cache/`、`memory/`、`multi_agent/`（team_runtime, teams）、`operator/`、`retrieval/`、`runner/`（callback）、`security/`（guardrail）、`session/`（tracer/state/checkpointer/interaction/stream）、`single_agent/`（agents/rail/skills/agents/legacy）、`sys_operation/`（shell_operation）、`workflow/`
- `agent_teams/` 团队协作（含 `i18n.py`、`timefmt.py`、`reliability/`、`session_controller/`）
- `agent_evolving/` RL 与轨迹系统
- `dev_tools/` skill 创作工具链
- `harness/` Rails（context_engineer, worktree, browser_move）
- `symphony/` 经验蒸馏

### 2. 第八轮 8 维度真实代码锚点

| 维度 | 路径 | 范式要点 |
|---|---|---|
| **Telemetry** | `core/session/tracer/tracer.py:16-61` | `TracerHandlerRegistry` 全局注册 OTEL handler；`Span`/`TraceAgentSpan`/`TraceWorkflowSpan` Pydantic 模型带 traceId/parent/child |
| **Session 持久化** | `core/session/agent.py:33-76` | `Session` 构造含 session_id/parent_session_id/source_metadata/stream_writer_manager；`core/session/state/agent_state.py:9-43` `StateCollection` 分 global/agent/trace 三段 |
| **Tool 权限** | `core/single_agent/ability_manager.py`、`core/security/guardrail/guardrail.py:42-100` | `AbilityManager` 注册 Tool/McpServerConfig；`BaseGuardrail` **双层架构**（事件处理 + GuardrailBackend 检测） |
| **LSP/Hook** | `core/security/guardrail/guardrail.py:35-39` | 集成 `runner.callback.enums.HookType`、`errors.AbortError`；Guardrail 通过 callback framework 注册事件 |
| **Skill 一等公民** | `core/single_agent/skills/skill_manager.py:10-87` | `Skill` Pydantic 模型；`SkillManager` 注册 YAML 技能（name/description/directory） |
| **多租户** | `core/session/agent.py:62-66` | `parent_session_id` 用于产品级会话嵌套；`kv_cache/kv_cache_metadata.py:15-19` `KV_CACHE_AFFINITY_PARENT_SESSION_ID_ENV` 父子会话共享 KV cache |
| **TUI 渲染** | `agent_teams/inbound_render.py` | 入站消息渲染；`agent_teams/team_context.py`、`timefmt.py` 人类可读时间格式 |
| **Runtime Telemetry** | `core/common/logging.py` | `session_logger` + `LogEventType` |

### 3. 第九轮新维度真实代码锚点

| 维度 | 路径 | 范式要点 |
|---|---|---|
| **Crash/Recovery** | `agent_evolving/checkpointing/manager.py`、`checkpointing/state.py` | Agent checkpoint 恢复；`session/checkpointer/` CheckpointerFactory |
| **OAuth** | `core/foundation/llm/model_clients/openai_account_model_client.py:53,79,307` | "Validate static config while allowing OAuth-based credentials"；通过 `bearer token` 走 ChatGPT 反代模型列表 |
| **i18n** | `agent_teams/i18n.py:1-100+` | **进程全局中英翻译**（`Language = Literal["cn","en"]`），`STRINGS` 字典覆盖 timefmt/blueprint/team/checkpoint/reliability/dispatcher |
| **Release** | `pyproject.toml` 标准 pyproject；`__init__.py` 版本号 |
| **WS/SSE** | `core/session/stream/` | `StreamWriterManager`、`BaseStreamMode`、`OutputSchema` |
| **Dev Container** | `agent_evolving/evaluator/evaluator_pipeline/docker_env.py` | 评测 docker 环境 |
| **CRDT** | `multi_agent/teams/` `team.py` `team_runtime/` | 团队共享状态 + `kv_cache/` 父子 cache affinity 实现软协同 |

### 4. 关键代码片段

#### 4.1 全局 Trace handler 注册中心（`core/session/tracer/tracer.py:16-61`）

```python
class TracerHandlerRegistry:
    """External handlers registered here get auto-picked-up by Tracer."""
    _agent_handlers: dict[str, TraceExtAgentHandler] = {}
    _workflow_handlers: dict[str, TraceExtWorkflowHandler] = {}
    _RESERVED_NAMES = {name.value for name in TracerHandlerName}
    @classmethod
    def register_handler(cls, handler_name, handler):
        if handler_name in cls._RESERVED_NAMES:
            raise ValueError(f"Handler '{handler_name}' is a reserved name...")
        ...
```

#### 4.2 双层 Guardrail 抽象（`core/security/guardrail/guardrail.py:42-100`）

```python
class BaseGuardrail(ABC):
    """Two-layer: BaseGuardrail (event handling) + GuardrailBackend (detection)."""
    DEFAULT_EVENTS: List[Any] = []
    DEFAULT_PRIORITY: int = 100
    NAMESPACE: str = "guardrail"
    def __init__(self, *, events=None, backend=None, priority=None, enable_logging=True):
        self._backend = backend
        ...
```

#### 4.3 父子 Session 嵌套 + KV cache affinity（`core/session/agent.py:33-68`）

```python
class Session:
    def __init__(self, session_id=None, envs=None, card=None, *,
                 stream_writer_manager=None, close_stream_on_post_run=True,
                 source_metadata=None, parent_session_id=None,
                 kv_cache_runtime: KVCacheRuntimeProtocol | None = None):
        if session_id is None: session_id = str(uuid.uuid4())
        self._parent_session_id = (parent_session_id.strip()
            if isinstance(parent_session_id, str) and parent_session_id.strip()
            else None)
        self._team_cache_scope: tuple[str, str] | None = None
        ...
```

#### 4.4 YAML Skill 注册 + 远程加载（`core/single_agent/skills/skill_manager.py:48-87`）

```python
class SkillManager:
    def __init__(self, sys_operation_id: str):
        self._registry: Dict[str, Skill] = {}
        self._sys_operation_id = sys_operation_id
    async def _load_yaml(self, path: Path, session_id: str):
        sys_operation = Runner.resource_mgr.get_sys_operation(self._sys_operation_id)
        result = await sys_operation.fs().read_file(str(path), mode="text", encoding="utf-8")
        ...
        if text.startswith("---"):
            _, yaml_block, body = text.split("---", 2)
```

#### 4.5 进程全局 i18n（`agent_teams/i18n.py:1-35`）

```python
"""Process-global i18n for agent team runtime strings."""
Language = Literal["cn", "en"]
_DEFAULT_LANGUAGE: Language = "cn"
_current_language: Language = _DEFAULT_LANGUAGE
STRINGS: dict[str, dict[str, str]] = {
    "cn": {
        "time.just_now": "刚刚",
        "time.seconds_ago": "{value} 秒前",
        "blueprint.default_desc": "天才项目管理专家",
        ...
    }
}
```

### 5. 设计哲学

agent-core (openjiuwen) 是 **「企业级抽象层级极高」** 的代表，把 LLM Agent 框架的所有易变点都做成可插拔策略接口：

1. **Tracer 扩展性**：`TracerHandlerRegistry` 允许运行时注册第三方 handler，且用 `_RESERVED_NAMES` 保留内建 handler 名避免冲突，`Span` 用 Pydantic + populate_by_name 支持 camelCase JSON 序列化。
2. **父子 Session 模型**通过 `parent_session_id` + `team_cache_scope` tuple 把 KV cache affinity 跟父子关系绑死，子会话复用父会话的 prefix cache 而无需重新计算。
3. **Guardrail 的"事件驱动 + Backend 抽象"** 两层架构，通过 callback framework 的 `HookType` 解耦，比硬编码 if-else 干净得多。
4. **i18n 设计**：直接用 dict + `set_language()` 而不是 gettext/babel，避开二进制 `.mo` 文件构建更轻量。
5. **SkillManager 通过 sys_operation 抽象**把"文件读写"也变成可注入后端，未来切到远程技能源时无需改核心代码。

**对 laew 的启示**：agent-core 是「可插拔策略接口」的工业级范本。laew 升级时可参考其 **TracerHandlerRegistry + 父子 Session KV cache affinity + Guardrail 双层 + 进程全局 i18n dict** 四大抽象。

---

> **字数**：本文档 agent-core 第八轮深挖章节新增约 700 行。
# Agent Core 第十轮深挖 — 8 大新维度深度分析

> 调研对象：agent-core（Python，openJiuwen Core SDK v0.1.17）
> 调研日期：2026-09-07
> 前 9 轮已覆盖：ReAct 循环 / ContextEngine / 记忆系统 / PermissionEngine / TeamAgent / Pregel / Rails / OTLP / RL 训练 / Telemetry / Session 持久化 / Tool 权限沙箱 / LSP / Hook / Skill / 多租户 / TUI 渲染 / CrashDump / WebUI / OAuth / i18n / Release / WebSocket / DevContainer / CRDT（前 9 轮部分涉及）
> 本轮 **全新深挖** 8 维度：**CrashDump 与错误恢复 / WebUI 与 DesktopApp / OAuth 认证与多账号 / i18n 国际化 / Release 工程化与 AutoUpdate / WebSocket 与 SSE / DevContainer 与容器化 / CRDT 与多端冲突**

---

## 目录

1. [CrashDump 与错误恢复](#一crashdump-与错误恢复)
2. [WebUI 与 DesktopApp](#二webui-与-desktopapp)
3. [OAuth 认证与多账号](#三oauth-认证与多账号)
4. [i18n 国际化](#四i18n-国际化)
5. [Release 工程化与 AutoUpdate](#五release-工程化与-autoupdate)
6. [WebSocket 与 SSE](#六websocket-与-sse)
7. [DevContainer 与容器化](#七devcontainer-与容器化)
8. [CRDT 与多端冲突](#八crdt-与多端冲突)
9. [laew gap 清单与借鉴建议](#九laew-gap-清单与借鉴建议)

---

## 一、CrashDump 与错误恢复

### 1.1 核心代码路径

| 模块 | 路径 | 行数 | 职责 |
|------|------|------|------|
| Checkpoint Manager | `openjiuwen/agent_evolving/checkpointing/manager.py` | 183 行 | 进化训练检查点管理 |
| Checkpoint State | `openjiuwen/agent_evolving/checkpointing/state.py` | 26 行 | 检查点数据结构 |
| Store Records | `openjiuwen/agent_evolving/checkpointing/store_records.py` | 467 行 | 原子写入 + 回滚恢复 |
| Store File | `openjiuwen/agent_evolving/checkpointing/store_file.py` | ~150 行 | 文件持久化层 |
| Reliability Handler | `openjiuwen/agent_teams/reliability/handler.py` | 113 行 | 异常检测与修复 |
| External Runtime Handler | `openjiuwen/agent_teams/reliability/external_handler.py` | 74 行 | 三方运行时重试处理 |

### 1.2 进化训练检查点（CheckpointManager）

**核心设计**：`DefaultCheckpointManager` 实现了训练进度的快照保存与恢复，支持「改进时保存」和「每 N 轮保存」双策略。

```python
# checkpointing/manager.py L31-128
class DefaultCheckpointManager:
    def __init__(
        self,
        *,
        run_id: Optional[str] = None,
        checkpoint_version: str = "v1",
        save_every_n_epochs: int = 1,
        save_on_improve: bool = True,
    ):
        self._run_id = run_id or str(uuid.uuid4())
        self._ckpt_version = checkpoint_version
        self._save_every_n_epochs = max(int(save_every_n_epochs), 1)
        self._save_on_improve = bool(save_on_improve)
        self._pending: Dict[str, List[PendingChange]] = {}

    def should_save(self, *, epoch: int, improved: bool) -> bool:
        if self._save_on_improve and improved:
            return True
        return (epoch % self._save_every_n_epochs) == 0

    def build_checkpoint(self, *, agent, progress, updater_state=None) -> EvolveCheckpoint:
        operators_state = self._snapshot_operators_state(agent)
        step = {"epoch": int(getattr(progress, "current_epoch", 0)),
                "batch": int(getattr(progress, "current_batch_iter", 0))}
        best = {"best_score": float(getattr(progress, "best_score", 0.0))}
        return EvolveCheckpoint(
            version=self._ckpt_version, run_id=self._run_id,
            step=step, best=best, seed=...,
            operators_state=operators_state,
            updater_state=updater_state or {}, searcher_state={},
            last_metrics={"current_epoch_score": ...})

    def restore(self, *, agent, checkpoint) -> Dict[str, Any]:
        self._restore_operators_state(agent, checkpoint.operators_state)
        return {"start_epoch": ..., "best_score": ..., "run_id": checkpoint.run_id}
```

**检查点数据结构**：

```python
# checkpointing/state.py L11-26
@dataclass
class EvolveCheckpoint:
    version: str
    run_id: str
    step: Dict[str, int]           # {epoch, batch}
    best: Dict[str, Any]           # {best_score}
    seed: Optional[int]
    operators_state: Dict[str, Dict[str, Any]]  # 各进化算子状态
    updater_state: Dict[str, Any]
    searcher_state: Dict[str, Any]
    last_metrics: Dict[str, Any]
```

### 1.3 原子写入与回滚恢复（StoreRecordsHelper）

**核心机制**：`append_record_transactional` 实现了事务型记录写入——失败时自动回滚所有相关文件。

```python
# checkpointing/store_records.py L138-185
async def append_record_transactional(self, name, record, *, skill_dir=None, subject_kind=None):
    target_dir = skill_dir or self._store.resolve_skill_dir(name, create=True, subject_kind=subject_kind)
    evo_path = target_dir / _EVOLUTION_FILENAME
    had_log = evo_path.exists()
    old_log_content = evo_path.read_text(encoding="utf-8") if had_log else None
    projection_backups = self._snapshot_projection_files(target_dir)  # 快照所有文件

    try:
        prepared_record = copy.deepcopy(record)
        if prepared_record.change.target == EvolutionTarget.SCRIPT:
            await self.persist_script(target_dir, prepared_record)
        evo_log = await self.load_full_evolution_log(name, subject_kind=subject_kind)
        self._append_or_merge_record(evo_log, prepared_log)
        await self.save_evolution_log(name, evo_log, skill_dir=target_dir)
        await self._store.render_evolution_markdown(name, subject_kind=subject_kind)
    except Exception:
        # 失败时回滚所有文件
        await self._restore_projection_files(target_dir, projection_backups)
        await self._restore_text_file(evo_path, old_log_content if had_log else None)
        raise
```

**原子文件写入**（temp + rename 模式）：

```python
# checkpointing/store_records.py L124-136
async def _write_file_text_atomic(self, path, content):
    tmp_path = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    try:
        await self._store.write_file_text(tmp_path, content)
        tmp_path.replace(path)  # 原子替换
    except Exception as exc:
        with suppress(OSError):
            tmp_path.unlink()
        raise_error(...)
```

### 1.4 可靠性处理（ReliabilityHandler）

**异常检测与修复策略**：

```python
# reliability/handler.py L30-113
class ReliabilityHandler(BaseCoordinationHandler):
    EVENT_METHOD_MAP = {
        TeamEvent.ANOMALY_DETECTED: "on_anomaly_detected",
        TeamEvent.MESSAGE: "on_message",
        TeamEvent.BROADCAST: "on_message",
    }

    async def on_anomaly_detected(self, event):
        # 接收跨进程异常流，按策略路由
        await self._route(Severity(payload.severity), self._format(payload))

    async def _route(self, severity, summary):
        actions = self._policy.actions_for(severity)
        if RemediationAction.ESCALATE_USER in actions:
            await self._round.deliver_input(t("reliability.escalate_user", ...))
        elif RemediationAction.REPORT_LEADER in actions:
            await self._round.deliver_input(t("reliability.report_leader", ...))
```

**三方运行时重试处理**：

```python
# reliability/external_handler.py L32-74
class ExternalRuntimeHandler(BaseCoordinationHandler):
    async def on_external_retry(self, event):
        # Claude/Codex SDK 仍在自动重试时，向 leader 投递非持久化进度提示
        await self._round.deliver_input(
            t("reliability.external_runtime_retrying",
              member_name=..., agent_kind=..., category=..., summary=...))
```

### 1.5 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **双策略保存** | `save_on_improve` + `save_every_n_epochs` | 改进时立即保存 + 定期保存 |
| **事务写入** | `append_record_transactional` | 失败自动回滚所有相关文件 |
| **原子替换** | temp + `os.replace` | 防止写入中途崩溃导致文件损坏 |
| **文件快照** | `_snapshot_projection_files` | 写入前备份所有文件，支持完整回滚 |
| **Pending 队列** | `_pending: Dict[str, List[PendingChange]]` | 在线进化的暂存变更管理 |
| **异常分级** | `Severity` + `RemediationPolicy` | REPORT_LEADER → ESCALATE_USER 两级升级 |
| **三方运行时解耦** | `ExternalRuntimeHandler` | 独立处理 Claude/Codex SDK 重试/失败信号 |

---

## 二、WebUI 与 DesktopApp

### 2.1 现状分析

**agent-core 本身不提供 WebUI 或 DesktopApp 实现**。它是一个纯后端 Python SDK，UI 层由上层应用（如 jiuwenclaw/jiuwenswarm）提供。但源码中存在与 UI 通信的关键接口：

| 模块 | 路径 | 职责 |
|------|------|------|
| Stream Emitter | `core/session/stream/emitter.py` | 流式输出发射器 |
| Stream Writer | `core/session/stream/writer.py` | 类型化流写入器 |
| Stream Manager | `core/session/stream/manager.py` | 多模式流管理 |
| WebSocket 客户端 | `agent_evolving/agent_rl/online/backends/rollouter/docker_runtime.py` | SFT 容器通过 WebSocket 与 jiuwenclaw 通信 |

### 2.2 流式输出架构（供 WebUI 消费）

```python
# core/session/stream/emitter.py L133-160
class StreamEmitter:
    END_FRAME = "all streaming outputs finish"

    def __init__(self):
        self._stream_queue = AsyncStreamQueue()
        self._closed = False

    async def emit(self, stream_data):
        if self._closed:
            raise RuntimeError("Can not emit data after the stream emitter is closed.")
        await self._stream_queue.send(stream_data)

    async def close(self):
        if self._closed:
            return
        self._closed = True
        if not self._stream_queue.is_closed:
            await self._stream_queue.send(self.END_FRAME)
```

**三模式流写入器**：

```python
# core/session/stream/base.py L24-50
class BaseStreamMode(StreamMode):
    OUTPUT = ("output", "Standard stream data defined by the framework")
    TRACE = ("trace", "Trace stream data produced by the graph")
    CUSTOM = ("custom", "Custom stream data defined by the runnable")

class OutputSchema(BaseModel):
    type: str
    index: int
    payload: Any

class TraceSchema(BaseModel):
    type: str
    payload: Any
```

**流管理器**：

```python
# core/session/stream/manager.py L18-111
class StreamWriterManager:
    def __init__(self, stream_emitter, modes=None):
        self._default_modes = modes or [BaseStreamMode.OUTPUT, BaseStreamMode.TRACE, BaseStreamMode.CUSTOM]
        self._writers: Dict[StreamMode, StreamWriter] = {}
        self._add_default_writers()

    async def stream_output(self, first_frame_timeout=-1, timeout=-1, need_close=True):
        # 首帧超时 + 帧间隔超时双控制
        while True:
            data = await self._stream_emitter.stream_queue.receive(timeout=...)
            if data == StreamEmitter.END_FRAME:
                break
            yield data
```

### 2.3 WebSocket 通信（SFT 容器 → jiuwenclaw）

```python
# agent_evolving/agent_rl/online/backends/rollouter/docker_runtime.py L510-566
# SFT 任务容器通过 WebSocket 与 jiuwenclaw WebUI 通信
async def main():
    ws = await connect_with_retry()
    async with ws:
        req_id = "chat-" + uuid.uuid4().hex[:12]
        frame = {
            "type": "req", "id": req_id, "method": "chat.send",
            "is_stream": True,
            "params": {
                "session_id": session_id, "content": prompt,
                "mode": os.environ.get("SFT_TASK_MODE", "agent.fast"),
                "cwd": cwd, "trusted_dirs": [cwd],
                "session_done": True, "close_session": True,
            },
        }
        await ws.send(json.dumps(frame, ensure_ascii=False))
        while time.time() < deadline:
            msg = json.loads(await asyncio.wait_for(ws.recv(), timeout=60))
            if event == "chat.processing_status":
                if payload.get("is_processing") is False:
                    return
```

### 2.4 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **三模式流** | OUTPUT / TRACE / CUSTOM | 标准输出、追踪、自定义流分离 |
| **首帧超时** | `first_frame_timeout` | 防止首帧卡死 |
| **帧间隔超时** | `timeout` | 检测流式输出中断 |
| **END_FRAME 哨兵** | `StreamEmitter.END_FRAME` | 优雅关闭信号 |
| **WebSocket 重连** | `connect_with_retry` | 120s  deadline 内自动重连 |
| **Schema 校验** | Pydantic `OutputSchema/TraceSchema` | 流数据强类型校验 |

### 2.5 laew gap

- **laew 无 WebUI**：纯 TUI 应用，无 Web 界面
- **laew 无流式 Schema**：`llm/anthropic.rs` 和 `llm/openai.rs` 的流式输出无统一 Schema
- **laew 无 WebSocket**：无实时双向通信能力

---

## 三、OAuth 认证与多账号

### 3.1 核心代码路径

| 模块 | 路径 | 行数 | 职责 |
|------|------|------|------|
| Auth Manager | `extensions/external_provider/openai_auth/openai_account_auth.py` | 841 行 | OAuth 认证全流程 |
| Account Login | `extensions/external_provider/openai_auth/openai_account_login.py` | 189 行 | CLI 登录脚本 |
| Account Models | `extensions/external_provider/openai_auth/openai_account_models.py` | 301 行 | 模型列表发现与缓存 |
| Model Client | `core/foundation/llm/model_clients/openai_account_model_client.py` | ~350 行 | OAuth 模型客户端 |

### 3.2 OAuth Device Code 完整流程

```python
# openai_account_auth.py L376-400
def login_openai_account_oauth(*, on_device_code, timeout_seconds, max_wait_seconds, ...):
    device_code = request_openai_account_device_code(timeout_seconds=timeout_seconds)
    if on_device_code:
        on_device_code(device_code)  # 回调显示 user_code + verification_uri
    authorization = poll_openai_account_device_authorization(
        device_code, timeout_seconds=timeout_seconds, max_wait_seconds=max_wait_seconds, ...)
    return exchange_openai_account_device_authorization(authorization, ...)
```

**Device Code 请求**：

```python
# openai_account_auth.py L403-472
def request_openai_account_device_code(*, timeout_seconds, max_attempts, sleep):
    for attempt in range(1, attempts + 1):
        response = client.post(
            OPENAI_ACCOUNT_DEVICE_USER_CODE_URL,
            json={"client_id": OPENAI_ACCOUNT_OAUTH_CLIENT_ID},
            headers={"Content-Type": "application/json"})
        if response.status_code != 429:
            break
        if attempt < attempts:
            retry_after = _parse_retry_after_seconds(...)
            delay = retry_after if retry_after is not None else 2 ** attempt
            sleep(max(1, min(int(delay), 60)))
    return OpenAIAccountDeviceCode(user_code=..., device_auth_id=..., interval=..., expires_in=...)
```

**Token 轮询**：

```python
# openai_account_auth.py L507-542
def poll_openai_account_device_authorization(device_code, *, timeout_seconds, max_wait_seconds, ...):
    start = monotonic()
    while monotonic() - start < max_wait_seconds:
        sleep(device_code.interval)
        try:
            authorization = poll_openai_account_device_authorization_once(device_code, ...)
        except OpenAIAccountAuthError as exc:
            if exc.code != _OPENAI_ACCOUNT_DEVICE_POLL_NETWORK_ERROR_CODE:
                raise
            continue
        if authorization is None:
            continue
        return authorization
    raise OpenAIAccountAuthError("OpenAI account device login timed out.", ...)
```

### 3.3 Token 存储与刷新

**Token 数据结构**：

```python
# openai_account_auth.py L34-111
@dataclass(frozen=True, slots=True)
class OpenAIAccountTokens:
    access_token: str
    refresh_token: str
    id_token: Optional[str] = None
    expires_at: Optional[float] = None
    token_type: Optional[str] = None
    scope: Optional[str] = None
    last_refresh: Optional[float] = None

    def is_expiring(self, *, now, skew_seconds=120) -> bool:
        expires_at = self.expires_at if self.expires_at is not None else _jwt_exp(self.access_token)
        if expires_at is None:
            return False
        return expires_at <= now + skew_seconds
```

**Token 刷新**：

```python
# openai_account_auth.py L592-665
def refresh_openai_account_oauth(access_token, refresh_token, *, timeout_seconds, now):
    del access_token  # 不使用旧 access_token
    response = client.post(
        OPENAI_ACCOUNT_OAUTH_TOKEN_URL,
        data={"grant_type": "refresh_token", "refresh_token": refresh_token,
              "client_id": OPENAI_ACCOUNT_OAUTH_CLIENT_ID})
    # 解析 + 校验 + 保留旧 refresh_token（如果响应中无新值）
    refreshed_payload["refresh_token"] = next_refresh if next_refresh else refresh_token
    return OpenAIAccountTokens.from_mapping(refreshed_payload, now=refreshed_at)
```

**文件锁保护**：

```python
# openai_account_auth.py L326-360
def _file_lock(self) -> FileLock:
    self.auth_path.parent.mkdir(parents=True, exist_ok=True)
    return FileLock(str(self.auth_path) + ".lock")

def _save_store_unlocked(self, store):
    self.auth_path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = self.auth_path.with_name(f"{self.auth_path.name}.tmp")
    tmp_path.write_text(json.dumps(store, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    os.chmod(tmp_path, 0o600)  # 仅 owner 可读写
    os.replace(tmp_path, self.auth_path)
    os.chmod(self.auth_path, 0o600)
```

### 3.4 多账号模型发现

```python
# openai_account_models.py L55-196
class OpenAIAccountModelCatalog:
    def list_model_ids(self, *, auth_manager=None, access_token=None, force_refresh=False):
        token = access_token or auth_manager.resolve_access_token(force_refresh=force_refresh)
        if token:
            try:
                payload, model_ids = self.fetch_models(access_token=token)
                self.write_cache(payload=payload, model_ids=model_ids)
                return model_ids
            except OpenAIAccountModelListError:
                pass
        cached = self.read_cache_model_ids()  # 缓存兜底
        if cached:
            return cached
        return _add_forward_compat_models(DEFAULT_OPENAI_ACCOUNT_MODELS)  # 内置兜底

    def fetch_models(self, *, access_token):
        response = client.get(self._models_url(),
                              headers={"Authorization": f"Bearer {access_token}", ...})
        model_ids = parse_openai_account_model_ids(payload)
        return payload, model_ids
```

**前向兼容模型**：

```python
# openai_account_models.py L33-38
_FORWARD_COMPAT_TEMPLATE_MODELS = [
    ("gpt-5.5", ("gpt-5.4", "gpt-5.4-mini", "gpt-5.3-codex", "gpt-5-codex")),
    ("gpt-5.4-mini", ("gpt-5.3-codex", "gpt-5-codex")),
    ("gpt-5.4", ("gpt-5.3-codex", "gpt-5-codex")),
    ("gpt-5.3-codex-spark", ("gpt-5.3-codex",)),
]
```

### 3.5 错误处理体系

```python
# openai_account_auth.py L139-157
class OpenAIAccountAuthError(Exception):
    def __init__(self, message, *, code, relogin_required=False, status_code=None):
        self.message = message
        self.code = code                    # 结构化错误码
        self.relogin_required = relogin_required  # 是否需要重新登录
        self.status_code = status_code      # HTTP 状态码
```

**错误码分类**：
- `openai_account_auth_missing` — 无凭证
- `openai_account_device_code_request_failed` — device code 请求失败
- `openai_account_device_code_timeout` — 轮询超时
- `openai_account_auth_refresh_failed` — 刷新失败
- `openai_account_auth_rate_limited` — 429 限流
- `openai_account_auth_invalid_store` — 存储文件损坏

### 3.6 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **Device Code 流程** | OAuth 2.0 Device Authorization Grant | 无浏览器环境登录 |
| **Token 自动刷新** | `is_expiring` + `refresh_tokens` | 提前 120s 刷新 |
| **文件锁** | `FileLock` | 防并发写入冲突 |
| **原子写入** | temp + `os.replace` | 防写入中途崩溃 |
| **权限控制** | `os.chmod(0o600)` | 仅 owner 可读写 |
| **三级 fallback** | 实时 → 缓存 → 内置 | 模型列表发现 |
| **前向兼容** | `_FORWARD_COMPAT_TEMPLATE_MODELS` | 新模型自动合成 |
| **结构化错误** | `code + relogin_required` | 调用方可决策是否重新登录 |

### 3.7 laew gap

- **laew 无 OAuth**：API Key 明文存储在 SQLite
- **laew 无多账号**：单 provider 模式
- **laew 无 Token 刷新**：无自动凭证刷新能力

---

## 四、i18n 国际化

### 4.1 核心代码路径

| 模块 | 路径 | 行数 | 职责 |
|------|------|------|------|
| i18n 核心 | `openjiuwen/agent_teams/i18n.py` | 812 行 | 进程全局中英翻译 |
| 时间格式 | `openjiuwen/agent_teams/timefmt.py` | ~100 行 | 相对时间格式化 |
| 入站渲染 | `openjiuwen/agent_teams/inbound_render.py` | ~200 行 | XML 入站消息渲染 |
| 团队上下文 | `openjiuwen/agent_teams/team_context.py` | ~400 行 | 团队状态多语言渲染 |

### 4.2 进程全局 i18n 设计

```python
# i18n.py L27-812
Language = Literal["cn", "en"]
_DEFAULT_LANGUAGE: Language = "cn"
_current_language: Language = _DEFAULT_LANGUAGE

STRINGS: dict[str, dict[str, str]] = {
    "cn": {
        "time.just_now": "刚刚",
        "time.seconds_ago": "{value} 秒前",
        "dispatcher.member_online": "[成员事件] 成员 {target_id} 已上线",
        "reliability.steer_self_correct": "⚙️[可靠性] 检测到 {kind}：{summary}。请停止重复无效操作...",
        "swarmflow.budget_exhausted.workflow_guidance": "该上限为本次工作流的单次额度...",
        # ... 100+ 键
    },
    "en": {
        "time.just_now": "just now",
        "time.seconds_ago": "{value}s ago",
        "dispatcher.member_online": "[Member Event] Member {target_id} is online",
        "reliability.steer_self_correct": "[reliability] Detected {kind}: {summary}. Stop repeating...",
        # ... 100+ 键
    }
}

def set_language(lang: Language) -> None:
    global _current_language
    _current_language = lang

def t(key: str, **kwargs) -> str:
    table = STRINGS[_current_language]
    raw = table[key]
    return raw.format_map(kwargs) if kwargs else raw
```

### 4.3 字符串分类覆盖

| 分类 | 键前缀 | 数量 | 示例 |
|------|--------|------|------|
| 时间格式 | `time.*` | 6 | `time.just_now`, `time.seconds_ago` |
| 蓝图 | `blueprint.*` | 2 | `blueprint.default_desc` |
| 团队工具 | `team.*` | 4 | `team.shutdown_request_content` |
| Checkpoint | `checkpoint.*` | 4 | `checkpoint.fork_not_found` |
| 可靠性 | `reliability.*` | 12 | `reliability.steer_self_correct` |
| 调度器 | `dispatcher.*` | 30+ | `dispatcher.member_online`, `dispatcher.stale_claim_self` |
| 调度器 | `scheduler.*` | 8 | `scheduler.leader_task_done` |
| HITT | `hitt.*` | 6 | `hitt.human_agent_display_name` |
| 工作流 | `workflow.*` | 4 | `workflow.started` |
| Swarmflow | `swarmflow.*` | 8 | `swarmflow.budget_exhausted` |
| 异步工具 | `async_tool.*` | 4 | `async_tool.launched` |

### 4.4 特殊场景处理

**User 发送者特殊提示**：

```python
# i18n.py L792-808
def reply_hint_for(sender: str) -> str:
    if sender == USER_PSEUDO_MEMBER_NAME:
        return t("dispatcher.reply_hint_user")  # 强制回复用户
    return t("dispatcher.reply_hint", sender=sender)  # 通用提示
```

**HITT 人类成员静默约束**：

```python
# i18n.py L280-286
"hitt.silence_note": (
    "**这是给控制者看的通知，不是要你执行的指令**，运行时已把它原样转给控制者。\n"
    "**严格禁止任何自主行为**：禁止主动回复发送方 / 指派方..."
),
```

### 4.5 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **进程全局** | `_current_language` 模块变量 | 简单高效 |
| **类型安全** | `Language = Literal["cn", "en"]` | 编译期检查 |
| **参数注入** | `str.format_map(kwargs)` | 支持动态内容 |
| **Key 缺失保护** | `raise KeyError` | 快速失败 |
| **不覆盖已有双语模块** | 注释明确排除 `prompts/sections.py` | 避免重复 |
| **User 特殊处理** | `reply_hint_for` | 防止用户消息被忽略 |

### 4.6 laew gap

- **laew 无 i18n**：TUI 文案硬编码中文
- **laew 无多语言切换**：无运行时语言切换能力
- **laew 无参数化字符串**：无 `t()` 函数

---

## 五、Release 工程化与 AutoUpdate

### 5.1 核心代码路径

| 模块 | 路径 | 职责 |
|------|------|------|
| pyproject.toml | `pyproject.toml` | 标准 pyproject 构建配置 |
| `__init__.py` | `openjiuwen/__init__.py` | 版本号动态解析 |
| Makefile | `Makefile` | 构建/测试/lint 任务 |
| uv 配置 | `pyproject.toml` `[tool.uv]` | 包管理器镜像源 |

### 5.2 版本管理

```python
# openjiuwen/__init__.py L1-23
from importlib.metadata import PackageNotFoundError, version

try:
    __version__ = version("openjiuwen")  # 从 Wheel 元数据读取
except PackageNotFoundError:
    import tomllib
    from pathlib import Path
    __version__ = "unknown"
    pyproject = Path(__file__).parents[1] / "pyproject.toml"
    if pyproject.exists():
        project_metadata = tomllib.loads(pyproject.read_text(encoding="utf-8")).get("project", {})
        package_name = project_metadata.get("name")
        version_parsed = project_metadata.get("version")
        if package_name == "openjiuwen" and version_parsed:
            __version__ = version_parsed  # 源码安装时从 pyproject.toml 读取
```

**pyproject.toml 版本**：

```toml
[project]
name = "openjiuwen"
version = "0.1.17"
requires-python = ">=3.11,<3.14"
```

### 5.3 构建系统

```toml
[build-system]
requires = ["setuptools>=61"]
build-backend = "setuptools.build_meta"
```

**uv 镜像源配置**：

```toml
[[tool.uv.index]]
name = "aliyun"
url = "https://mirrors.aliyun.com/pypi/simple"
default = true
```

### 5.4 可选依赖组

```toml
[project.optional-dependencies]
all-a2a = ["a2a-sdk[http-server]==1.0.0"]
pulsar = ["pulsar-client>=3.5.0"]
redis = ["redis>=7.1.0"]
sqlite = ["aiosqlite>=0.22.1"]
postgres = ["asyncpg>=0.30.0"]
sandbox = ["agent-sandbox>=0.0.26"]
claude = ["claude-agent-sdk>=0.2.115"]
codex = ["openai-codex>=0.144.4"]
observability = ["opentelemetry-api>=1.25.0", ...]
online-rl = ["openjiuwen[redis]", "fastapi>=0.100", ...]
```

### 5.5 CLI 入口点

```toml
[project.scripts]
openjiuwen = "openjiuwen.harness.cli.cli:cli"
team-member = "openjiuwen.agent_teams.skill.cli:run"
openjiuwen-team-mcp = "openjiuwen.agent_teams.mcp.server:main"
openjiuwen-rl-service = "openjiuwen.agent_evolving.agent_rl.online.service:main"
```

### 5.6 Makefile 任务

```makefile
# Makefile 核心目标
help       - 显示帮助
install    - 安装依赖（uv/pip 自动检测）
test       - pytest 测试
format     - ruff 格式检查
lint       - ruff lint 检查
pylint     - pylint 深度检查
spelling   - codespell 拼写检查
fix-format - ruff 自动修复格式
fix-lint   - ruff 自动修复 lint
type-check - mypy 类型检查
check      - 全量检查（format + spelling + lint + pylint）
fix        - 全量自动修复
flame      - pyinstrument 火焰图
speedscope - speedscope 性能分析
```

### 5.7 测试配置

```toml
[tool.pytest.ini_options]
asyncio_default_fixture_loop_scope = "function"
testpaths = ["tests"]
addopts = ["-v", "--html=report/index.html", "--self-contained-html"]
markers = [
    "level0: smoke / happy-path coverage; PR gate must stay green",
    "level1: feature branches, error paths, and lifecycle edges",
]
```

### 5.8 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **双版本源** | Wheel 元数据 + pyproject.toml fallback | 开发/生产环境兼容 |
| **可选依赖组** | `[project.optional-dependencies]` | 按需安装 |
| **镜像源** | aliyun PyPI | 国内加速 |
| **多入口点** | 4 个 CLI 命令 | 不同场景独立入口 |
| **测试分级** | level0/level1 markers | PR 门禁 + 深度测试 |
| **跨平台 Makefile** | Windows/Unix 双兼容 | 开发者友好 |

### 5.9 laew gap

- **laew 无 AutoUpdate**：无自动更新机制
- **laew 无版本 fallback**：`build.rs` 注入固定版本
- **laew 无可选依赖**：所有功能编译进单一二进制

---

## 六、WebSocket 与 SSE

### 6.1 核心代码路径

| 模块 | 路径 | 职责 |
|------|------|------|
| Stream Emitter | `core/session/stream/emitter.py` | 流式数据发射器 |
| Stream Queue | `core/session/stream/emitter.py` `AsyncStreamQueue` | 异步流队列 |
| Stream Writer | `core/session/stream/writer.py` | 类型化写入器 |
| Stream Manager | `core/session/stream/manager.py` | 多模式流管理 |
| WebSocket 客户端 | `agent_evolving/agent_rl/online/backends/rollouter/docker_runtime.py` | SFT 容器 WS 通信 |

### 6.2 异步流队列

```python
# core/session/stream/emitter.py L11-130
class AsyncStreamQueue:
    DEFAULT_SEND_ATTEMPT_TIMEOUT = 0.2
    DEFAULT_MAX_SEND_RETRIES = 5
    DEFAULT_RECEIVE_TIMEOUT = -1
    DEFAULT_CLOSE_TIMEOUT = 5.0

    def __init__(self, maxsize: int = 0):
        self._stream_queue = asyncio.Queue(maxsize=maxsize)
        self._closed = False
        self._sent_count = 0
        self._received_count = 0

    async def send(self, data, attempt_timeout=0.2, max_retries=5):
        if self._closed:
            raise RuntimeError("StreamQueue is already closed")
        for attempt in range(max_retries):
            try:
                await asyncio.wait_for(self._stream_queue.put(data), attempt_timeout)
                self._sent_count += 1
                return
            except asyncio.TimeoutError:
                continue
        # 超过重试次数后记录错误

    async def receive(self, timeout=-1):
        stream_item = await asyncio.wait_for(
            self._stream_queue.get(), timeout if timeout and timeout > 0 else None)
        self._received_count += 1
        return stream_item

    async def close(self, timeout=5.0):
        self._closed = True
        try:
            await asyncio.wait_for(self._stream_queue.join(), timeout)
        except asyncio.TimeoutError:
            self._force_clear()  # 超时后强制清空
```

### 6.3 流式输出消费

```python
# core/session/stream/manager.py L40-76
class StreamWriterManager:
    async def stream_output(self, first_frame_timeout=-1, timeout=-1, need_close=True):
        is_first_frame = True
        yielded_count = 0
        while True:
            if is_first_frame:
                data = await self._stream_emitter.stream_queue.receive(timeout=first_frame_timeout)
                is_first_frame = False
            else:
                data = await self._stream_emitter.stream_queue.receive(timeout=timeout)
            if data is not None:
                if data == StreamEmitter.END_FRAME:
                    if need_close:
                        await self._stream_emitter.stream_queue.close(timeout=timeout)
                    break
                else:
                    yielded_count += 1
                    yield data
```

### 6.4 WebSocket 客户端（SFT 容器）

```python
# agent_evolving/agent_rl/online/backends/rollouter/docker_runtime.py L510-566
async def connect_with_retry():
    deadline = time.time() + 120
    while time.time() < deadline:
        try:
            return await websockets.connect(url, max_size=16 * 2**20, close_timeout=2)
        except Exception:
            await asyncio.sleep(1)
    raise RuntimeError(f"failed to connect jiuwenclaw websocket {url}")

async def main():
    ws = await connect_with_retry()
    async with ws:
        frame = {"type": "req", "id": req_id, "method": "chat.send",
                 "is_stream": True, "params": {...}}
        await ws.send(json.dumps(frame))
        while time.time() < deadline:
            msg = json.loads(await asyncio.wait_for(ws.recv(), timeout=60))
            if event == "chat.processing_status":
                if payload.get("is_processing") is False:
                    return
```

### 6.5 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **重试发送** | `max_retries=5` | 防止临时背压丢数据 |
| **首帧超时** | `first_frame_timeout` | 检测 LLM 调用卡死 |
| **帧间隔超时** | `timeout` | 检测流式输出中断 |
| **强制清空** | `_force_clear` | 关闭时防止队列泄漏 |
| **END_FRAME 哨兵** | 特殊字符串 | 优雅关闭信号 |
| **WebSocket 重连** | 120s deadline | SFT 容器启动等待 |

### 6.6 laew gap

- **laew 无 WebSocket**：无实时双向通信
- **laew 无 SSE**：无 Server-Sent Events 推送
- **laew 无帧超时**：流式输出无首帧/帧间隔超时检测

---

## 七、DevContainer 与容器化

### 7.1 核心代码路径

| 模块 | 路径 | 职责 |
|------|------|------|
| Docker Environment | `agent_evolving/evaluator/evaluator_pipeline/docker_env.py` | 评测 Docker 环境管理 |
| Docker Runtime | `agent_evolving/agent_rl/online/backends/rollouter/docker_runtime.py` | SFT 训练容器编排 |
| Observability Compose | `deploy/observability/docker-compose.yml` | Langfuse 可观测性栈 |
| Sandbox | `extensions/sys_operation/sandbox/` | 沙箱提供者 |

### 7.2 DockerEnvironment 评测环境

```python
# agent_evolving/evaluator/evaluator_pipeline/docker_env.py L16-270
class DockerEnvironment:
    def __init__(self, image_tag, container_name=None, cpus=1, memory_mb=2048, timeout=900):
        self.image_tag = image_tag
        self._cpus = cpus
        self._memory_mb = memory_mb
        self._timeout = timeout
        self._container_id = None

    def build(self, dockerfile_path, build_context, build_timeout=600, no_cache=False, build_args=None):
        cmd = [self._docker_path(), "build"]
        if no_cache:
            cmd.append("--no-cache")
        if build_args:
            for key, value in build_args.items():
                cmd.extend(["--build-arg", f"{key}={value}"])
        cmd.extend(["-t", self.image_tag, "-f", str(dockerfile_path), str(build_context)])
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=build_timeout)
        if result.returncode != 0:
            raise RuntimeError(f"docker build failed (rc={result.returncode}): {result.stderr[:2000]}")

    async def start(self):
        locale_name = self._get_utf8_locale_name()
        cmd = [self._docker_path(), "run", "-d",
               "-e", f"LANG={locale_name}", "-e", f"LC_ALL={locale_name}",
               "--memory", f"{self._memory_mb}m", "--cpus", str(self._cpus),
               self.image_tag, "tail", "-f", "/dev/null"]
        result = await self._run_command(cmd, timeout=60)
        self._container_id = result.stdout.strip()

    async def exec(self, command, *, timeout=300, workdir=None, env=None):
        cmd = [self._docker_path(), "exec"]
        if workdir:
            cmd.extend(["-w", workdir])
        if env:
            for k, v in env.items():
                cmd.extend(["-e", f"{k}={v}"])
        cmd.append(self._container_id)
        cmd.extend(["bash", "-c", command])
        return await self._run_command(cmd, timeout=timeout)
```

**UTF-8 Locale 自动检测**：

```python
# docker_env.py L33-71
@staticmethod
def _get_utf8_locale_name() -> str:
    preferred_locales = ["C.utf8", "C.UTF-8", "en_US.utf8", "en_US.UTF-8"]
    locale_cmd = DockerEnvironment._locale_path()
    result = subprocess.run([locale_cmd, "-a"], capture_output=True, encoding="utf-8")
    available_locales = result.stdout.split()
    for locale in preferred_locales:
        if locale in available_locales:
            return locale
    return "en_US.utf8"  # fallback
```

### 7.3 SFT 训练容器编排

```python
# agent_evolving/agent_rl/online/backends/rollouter/docker_runtime.py L97-130
def build_jiuwenclaw_docker_command(request):
    docker_mounts, pythonpath, command_prefix = docker_runtime_mounts()
    env = build_jiuwenclaw_docker_env(request, ...)
    return [
        "docker", "run", "--rm", *docker_mounts,
        *_docker_env_args(env),
        request.image, "bash", "-lc", wrapped_command,
    ]

def build_jiuwenclaw_docker_env(request, *, dataset_case_json, pythonpath, data_dir):
    env = {
        "SFT_TASK_PROMPT": request.task_prompt,
        "API_BASE": f"{request.supervisor_url}/v1",
        "API_KEY": request.supervisor_token,
        "TRAJECTORY_GATEWAY_URL": request.gateway_url,
        "USE_RL_ONLINE_RAIL": "1",
        "TRAIN_BACKEND": "SFT",
        "PYTHONPATH": pythonpath,
        "JIUWENSWARM_DATA_DIR": data_dir,
        "SFT_TASK_MODE": os.getenv("SFT_TASK_MODE", "agent.fast"),
        "SFT_TASK_MAX_ITERATIONS": os.getenv("SFT_TASK_MAX_ITERATIONS", ""),
        "SFT_TASK_CHAT_TIMEOUT": os.getenv("SFT_TASK_CHAT_TIMEOUT", "600"),
    }
    env.update(_context_env_from_host())  # 继承宿主机上下文压缩配置
    return env
```

**并发控制**：

```python
# docker_runtime.py L302-322
async def run_docker_command_specs(specs, *, concurrency):
    semaphore = asyncio.Semaphore(max(1, int(concurrency)))
    async def _run_one(spec):
        async with semaphore:
            return await run_docker_command_spec(spec)
    return list(await asyncio.gather(*(_run_one(spec) for spec in specs)))
```

### 7.4 可观测性 Docker Compose

```yaml
# deploy/observability/docker-compose.yml
services:
  otel-collector:    # OpenTelemetry Collector（4317/4318 端口）
  langfuse-web:      # Langfuse Web UI（3000 端口）
  langfuse-worker:   # Langfuse 后台 worker
  postgres:          # PostgreSQL 17
  clickhouse:        # ClickHouse 分析数据库
  redis:             # Redis 7
  minio:             # MinIO 对象存储
```

### 7.5 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **资源限制** | `--memory` + `--cpus` | 防止容器资源耗尽 |
| **UTF-8 自动检测** | `_get_utf8_locale_name` | 避免容器内编码问题 |
| **并发控制** | `asyncio.Semaphore` | 限制并行容器数 |
| **挂载共享** | conda + 代码目录 | 容器复用宿主机环境 |
| **环境变量注入** | 30+ 个 SFT 环境变量 | 灵活配置训练任务 |
| **超时控制** | `timeout` 参数 | 防止容器无限运行 |
| **优雅清理** | `trap cleanup EXIT` | 容器退出时 kill 子进程 |

### 7.6 laew gap

- **laew 无容器化**：无 Docker/DevContainer 配置
- **laew 无资源限制**：Bash 工具无 CPU/内存限制
- **laew 无沙箱**：无隔离执行环境

---

## 八、CRDT 与多端冲突

### 8.1 现状分析

**agent-core 无显式 CRDT 实现**。但源码中存在多处**冲突避免与状态同步**机制，可作为 CRDT 的替代方案：

| 模块 | 路径 | 机制 |
|------|------|------|
| 原子写入 | `checkpointing/store_records.py` | temp + rename |
| 文件锁 | `extensions/external_provider/openai_auth/openai_account_auth.py` | `FileLock` |
| 分布式锁 | `core/memory/long_term_memory.py` | `DistributedLock` |
| 记录合并 | `checkpointing/store_records.py` | `merge_records` |
| 团队状态追踪 | `agent_teams/team_context.py` | `TeamContextTracker` |
| 进化记录合并 | `checkpointing/store_records.py` | `_append_or_merge_record` |

### 8.2 原子写入与事务

```python
# checkpointing/store_records.py L138-185
async def append_record_transactional(self, name, record, ...):
    projection_backups = self._snapshot_projection_files(target_dir)  # 快照
    try:
        prepared_record = copy.deepcopy(record)
        evo_log = await self.load_full_evolution_log(name)
        self._append_or_merge_record(evo_log, prepared_record)
        await self.save_evolution_log(name, evo_log)
    except Exception:
        await self._restore_projection_files(target_dir, projection_backups)  # 回滚
        await self._restore_text_file(evo_path, old_log_content)
        raise
```

### 8.3 记录合并

```python
# checkpointing/store_records.py L187-203
@staticmethod
def _append_or_merge_record(evo_log, record):
    merge_target = record.change.merge_target
    if not merge_target:
        evo_log.entries.append(record)
        return
    for idx, existing in enumerate(evo_log.entries):
        if existing.id == merge_target:
            evo_log.entries[idx] = record  # 替换已有记录
            return
    evo_log.entries.append(record)
```

### 8.4 团队状态追踪（TeamContextTracker）

```python
# agent_teams/team_context.py L93-156
class TeamContextTracker:
    """Render the team state one member has not been told about yet."""
    TEAM_CONTEXT_STATE_KEY = "team_prompt_context"
    _IDENTITY_EMITTED = "identity_emitted"
    _TEAM_INFO_MTIME = "team_info_mtime"
    _ROSTER_MTIME = "roster_mtime"
    _ROSTER = "roster"
    _MEMBER_PROMPT_MTIME = "member_prompt_mtime"

    async def pending_text(self, session):
        baseline = self._read_baseline(session)
        updated = dict(baseline)
        blocks = []
        identity_body = await self._identity_body(baseline, updated)
        info_body = await self._team_info_body(baseline, updated)
        if identity_body or info_body:
            blocks.append(render_team_context_with_identity(...))
        roster_body = await self._roster_body(baseline, updated)
        if roster_body:
            blocks.append(roster_body)
        if not blocks:
            self._write_baseline(session, updated)
            return None
        self._uncommitted = updated
        return "\n".join(blocks)

    async def commit(self, session):
        if self._uncommitted is not None:
            self._write_baseline(session, self._uncommitted)
            self._uncommitted = None
```

**设计要点**：
- **Baseline 持久化**：每个成员的状态基线存储在 `AgentSession` 中
- **增量更新**：仅发送变化的部分
- **先提交后发送**：`commit` 在 `pending_text` 之后调用，防止消息丢失
- **mtime 驱动**：通过修改时间戳判断是否需要更新

### 8.5 分布式锁

```python
# core/memory/long_term_memory.py L527
lock = DistributedLock(self.kv_store, f"user/{user_id}")
async with lock:
    # 写入原始消息
    for msg in messages:
        await self.message_manager.add(add_req)
    # 生成记忆
    all_memory = await self.generator.gen_all_memory(...)
    # 写入各类型存储
    write_result = await self.write_manager.add_memories(...)
```

### 8.6 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **原子写入** | temp + `os.replace` | 防写入中途崩溃 |
| **文件锁** | `FileLock` | 防并发写入冲突 |
| **分布式锁** | `DistributedLock` | 防多用户并发写入 |
| **记录合并** | `_append_or_merge_record` | 同 ID 记录替换 |
| **事务回滚** | `_snapshot_projection_files` | 失败时恢复所有文件 |
| **增量同步** | `TeamContextTracker` | 仅发送变化部分 |
| **先提交后发送** | `commit` 在 `pending_text` 后 | 防消息丢失 |

### 8.7 laew gap

- **laew 无 CRDT**：无冲突自由数据类型
- **laew 无多端同步**：无多设备状态同步
- **laew 无分布式锁**：SQLite 单文件无并发控制
- **laew 无增量更新**：TUI 全量重绘

---

## 九、laew gap 清单与借鉴建议

### 9.1 P0 紧急（必须实现）

| Gap | agent-core 实现 | laew 现状 | 建议 Rust crate |
|-----|-----------------|-----------|-----------------|
| **L79 无 panic hook** | `checkpointing/manager.py` 事务回滚 | 无 | `human-panic` |
| **L80 无 OAuth** | `openai_account_auth.py` 完整流程 | API Key 明文 | `oauth2` + `keyring` |
| **L81 无 Token 刷新** | `refresh_openai_account_oauth` | 无 | `oauth2` |
| **L82 无原子写入** | temp + `os.replace` | SQLite 直接写 | `tempfile` + `fsync` |
| **L83 无文件锁** | `FileLock` | 无 | `fs2` + `filelock` |

### 9.2 P1 重要（应该实现）

| Gap | agent-core 实现 | laew 现状 | 建议 Rust crate |
|-----|-----------------|-----------|-----------------|
| **L84 无 i18n** | `i18n.py` 进程全局 dict | 硬编码中文 | `rust-i18n` + `fluent` |
| **L85 无 WebSocket** | `docker_runtime.py` WS 客户端 | 无 | `tokio-tungstenite` |
| **L86 无 SSE** | `StreamEmitter` 流式输出 | 无 | `eventsource-client` |
| **L87 无容器化** | `DockerEnvironment` 完整管理 | 无 | `docker-api` |
| **L88 无事务写入** | `append_record_transactional` | 无 | `tempfile` + `atomicwrites` |
| **L89 无增量同步** | `TeamContextTracker` baseline | TUI 全量重绘 | `ratatui` diff |

### 9.3 P2 进阶（可选实现）

| Gap | agent-core 实现 | laew 现状 | 建议 Rust crate |
|-----|-----------------|-----------|-----------------|
| **L90 无 CRDT** | `merge_records` 记录合并 | 无 | `yrs` / `automerge` |
| **L91 无多账号** | `OpenAIAccountModelCatalog` | 单 provider | `keyring` |
| **L92 无模型发现** | `fetch_models` + 缓存 | 手动配置 | `reqwest` |
| **L93 无前向兼容** | `_FORWARD_COMPAT_TEMPLATE_MODELS` | 无 | `semver` |
| **L94 无沙箱** | `DockerEnvironment` 隔离执行 | 无 | `landlock` + `seccompiler` |
| **L95 无资源限制** | `--memory` + `--cpus` | 无 | `cgroups-rs` |

### 9.4 实现路线图

```
Phase 1 (P0 基础, 1 个月)
  ├─ 1. panic hook + 错误恢复（human-panic）
  ├─ 2. 原子写入 + 文件锁（tempfile + fs2）
  ├─ 3. OAuth 认证流程（oauth2 + keyring）
  └─ 4. Token 自动刷新

Phase 2 (P1 增强, 2 个月)
  ├─ 5. i18n 运行时切换（rust-i18n）
  ├─ 6. WebSocket 实时通信（tokio-tungstenite）
  ├─ 7. SSE 流式推送（eventsource-client）
  └─ 8. 事务写入 + 回滚

Phase 3 (P2 高级, 3 个月)
  ├─ 9. CRDT 多端同步（yrs）
  ├─ 10. 多账号轮换（keyring）
  ├─ 11. 容器化沙箱（landlock）
  └─ 12. 资源限制（cgroups-rs）
```

### 9.5 核心 Rust 实现参考

#### OAuth 认证

```rust
pub struct OAuthManager {
    client_id: String,
    auth_url: String,
    token_url: String,
    auth_store_path: PathBuf,
}

impl OAuthManager {
    pub async fn login_with_device_code(&self) -> Result<Tokens> {
        let device_code = self.request_device_code().await?;
        println!("Open this URL: {}", device_code.verification_uri);
        println!("Enter code: {}", device_code.user_code);
        let authorization = self.poll_device_authorization(&device_code).await?;
        self.exchange_device_authorization(authorization).await
    }

    pub async fn refresh_tokens(&self, refresh_token: &str) -> Result<Tokens> {
        // POST grant_type=refresh_token
    }

    pub fn resolve_access_token(&self, force_refresh: bool) -> Result<String> {
        let tokens = self.load_tokens()?;
        if !force_refresh && !tokens.is_expiring() {
            return Ok(tokens.access_token);
        }
        let refreshed = self.refresh_tokens(&tokens.refresh_token)?;
        self.save_tokens(&refreshed)?;
        Ok(refreshed.access_token)
    }
}
```

#### 原子写入

```rust
pub async fn write_file_atomic(path: &Path, content: &str) -> Result<()> {
    let tmp_path = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4()));
    tokio::fs::write(&tmp_path, content).await?;
    tokio::fs::rename(&tmp_path, path).await?;
    Ok(())
}
```

#### i18n

```rust
pub struct I18n {
    language: Language,
    strings: HashMap<String, HashMap<String, String>>,
}

impl I18n {
    pub fn t(&self, key: &str, kwargs: &[(&str, &str)]) -> String {
        let table = self.strings.get(&self.language.to_string()).unwrap();
        let template = table.get(key).unwrap();
        // 简单替换 {key} → value
        let mut result = template.to_string();
        for (k, v) in kwargs {
            result = result.replace(&format!("{{{}}}", k), v);
        }
        result
    }
}
```

#### 增量同步

```rust
pub struct TeamContextTracker {
    baseline: HashMap<String, serde_json::Value>,
}

impl TeamContextTracker {
    pub fn pending_text(&mut self, current: &TeamState) -> Option<String> {
        let mut blocks = Vec::new();
        // 仅比较变化的部分
        if current.roster_mtime != self.baseline["roster_mtime"] {
            blocks.push(self.render_roster_delta(current));
        }
        if blocks.is_empty() {
            None
        } else {
            Some(blocks.join("\n"))
        }
    }

    pub fn commit(&mut self, current: &TeamState) {
        self.baseline = serde_json::to_value(current).unwrap();
    }
}
```

---

## 总结

agent-core 在第十轮深挖的 8 大维度上展现出以下特征：

1. **CrashDump 与错误恢复** — 事务写入 + 文件快照 + 回滚恢复 + 异常分级处理，工业级鲁棒性
2. **WebUI 与 DesktopApp** — 纯后端 SDK，但提供三模式流（OUTPUT/TRACE/CUSTOM）+ WebSocket 通信接口
3. **OAuth 认证与多账号** — 完整 Device Code 流程 + Token 自动刷新 + 三级 fallback 模型发现
4. **i18n 国际化** — 进程全局 dict + `t()` 函数 + 100+ 键覆盖，简洁高效
5. **Release 工程化** — 标准 pyproject + 可选依赖组 + 跨平台 Makefile + 测试分级
6. **WebSocket 与 SSE** — 异步流队列 + 首帧/帧间隔超时 + WebSocket 重连
7. **DevContainer 与容器化** — DockerEnvironment 完整管理 + SFT 容器编排 + 可观测性 Compose
8. **CRDT 与多端冲突** — 无显式 CRDT，但通过原子写入 + 文件锁 + 分布式锁 + 增量同步实现冲突避免

对 laew 而言，P0 的 5 项借鉴（panic hook / 原子写入 / OAuth / Token 刷新 / 文件锁）能在 1 个月内显著提升系统安全性与鲁棒性，是最具 ROI 的改造方向。

---

**分析完成日期**：2026-09-07
**分析人**：Claude Code Agent
**源码版本**：openJiuwen Core 0.1.17（HEAD 截至分析时）
**已读取关键文件**：
- `openjiuwen/agent_evolving/checkpointing/manager.py` (183 行)
- `openjiuwen/agent_evolving/checkpointing/state.py` (26 行)
- `openjiuwen/agent_evolving/checkpointing/store_records.py` (467 行)
- `openjiuwen/agent_teams/reliability/handler.py` (113 行)
- `openjiuwen/agent_teams/reliability/external_handler.py` (74 行)
- `openjiuwen/core/session/stream/emitter.py` (160 行)
- `openjiuwen/core/session/stream/manager.py` (111 行)
- `openjiuwen/core/session/stream/writer.py` (82 行)
- `openjiuwen/core/session/stream/base.py` (50 行)
- `openjiuwen/extensions/external_provider/openai_auth/openai_account_auth.py` (841 行)
- `openjiuwen/extensions/external_provider/openai_auth/openai_account_login.py` (189 行)
- `openjiuwen/extensions/external_provider/openai_auth/openai_account_models.py` (301 行)
- `openjiuwen/agent_teams/i18n.py` (812 行)
- `openjiuwen/agent_teams/team_context.py` (400+ 行)
- `openjiuwen/agent_evolving/evaluator/evaluator_pipeline/docker_env.py` (270 行)
- `openjiuwen/agent_evolving/agent_rl/online/backends/rollouter/docker_runtime.py` (573 行)
- `openjiuwen/__init__.py` (23 行)
- `pyproject.toml` (314 行)
- `Makefile` (221 行)
- `deploy/observability/docker-compose.yml` (176 行)

---

> **字数**：本文档第十轮深挖新增约 10,000 字 / ~450 行代码片段。
