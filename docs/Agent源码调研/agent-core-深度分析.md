# openJiuwen Core 深度分析报告

## 一、架构设计决策深度剖析

### 1.1 分层架构设计

openJiuwen Core 采用**四层分离架构**，每一层职责清晰、依赖单向：

```
┌─────────────────────────────────────────────────────────────────┐
│                     CLI / API 入口层                             │
│    (openjiuwen.harness.cli / core.single_agent / Runner)        │
├─────────────────────────────────────────────────────────────────┤
│                    Agent 编排层                                  │
│    DeepAgent → ReActAgent → BaseAgent                           │
│    WorkflowAgent / TeamAgent                                    │
├─────────────────────────────────────────────────────────────────┤
│                    能力管理层                                    │
│    AbilityManager (工具/工作流/MCP 注册)                         │
│    ContextEngine (上下文管理)                                    │
│    Session (会话状态)                                            │
├─────────────────────────────────────────────────────────────────┤
│                    基础设施层                                    │
│    Model (LLM 调用) / Tool / Memory / Store / Retrieval        │
└─────────────────────────────────────────────────────────────────┘
```

**分层决策的核心考量**：

1. **协议无关性**：Agent 编排层与工具层永远不接触协议细节（Anthropic vs OpenAI），协议差异封闭在 `Model` 实现内部。这与 laew 的 `LlmClient` trait 设计异曲同工。

2. **资源统一管理**：`Runner.resource_mgr` 作为进程全局状态管理器，集中管理工具实例、工作流注册、系统操作（SysOperation），避免散弹式修改。

3. **可测试性**：每一层通过接口（Protocol/ABC）隔离，可独立 mock 测试。

**代码位置**：`openjiuwen/core/runner/runner.py`

### 1.2 Card/Config 分离

这是 openJiuwen 最核心的设计原则之一：

```python
# Card 定义身份/元数据（不可变/少变）
class AgentCard(BaseModel):
    id: str
    name: str
    description: str

class ToolCard(BaseModel):
    name: str
    description: str
    input_schema: dict
    exposure: ToolExposure = ToolExposure.DIRECT
    # properties 包含 resilience、parallel_safe 等运行时元数据
    properties: dict = {}

# Config 定义运行时行为（热更新）
class ReActAgentConfig(BaseModel):
    max_iterations: int = 5
    model_name: str
    model_provider: str = "openai"
    parallel_tool_calls: bool = True
    context_engine_config: ContextEngineConfig
```

**分离的深层原因**：

- **热更新支持**：`DeepAgent.configure()` 区分 `_initial_configure` 与 `_hot_reconfigure`，Config 变更无需重建 Agent 实例。
- **多 Agent 共享**：同一 ToolCard 可被多个 Agent 引用，但各自拥有独立 Config 状态。
- **持久化友好**：Card 适合序列化存储，Config 适合运行时动态调整。

**配置热更新实现**（`react_agent.py`）：

```python
def configure(self, config: ReActAgentConfig) -> 'BaseAgent':
    config = self._with_context_engine_model_name(config)
    config = self._with_context_engine_model_window(config)
    old_config = self._config
    self._config = config
    
    # 仅在模型 provider/key/base 变化时重置 LLM
    if (old_config.model_provider != config.model_provider or
            old_config.api_key != config.api_key or
            old_config.api_base != config.api_base):
        self._llm = None
    
    # 重建 context_engine（如上下文设置变化）
    context_engine_runtime_changed = (
        old_config.context_engine_config != config.context_engine_config
        or old_config.workspace != config.workspace
        or old_config.sys_operation_id != config.sys_operation_id
    )
    if context_engine_runtime_changed:
        self.context_engine = ContextEngine(...)
```

### 1.3 设计模式运用全景

| 模式 | 实现位置 | 运用方式 |
|------|----------|----------|
| **模板方法** | `BaseAgent.invoke()` | 骨架固定，子类实现具体逻辑 |
| **装饰器/铁轨** | `@rail(before, after, on_exception)` | 可插拔的钩子链 |
| **观察者** | `AgentCallbackManager` | 事件注册/触发机制 |
| **策略模式** | `ContextProcessor` | 可插拔的压缩策略 |
| **单例模式** | `LongTermMemory(metaclass=Singleton)` | 全局唯一记忆引擎 |
| **工厂模式** | `Runner.resource_mgr` | 资源创建/获取统一入口 |
| **桥接模式** | `DeepAgent` 包装 `ReActAgent` | 抽象与实现分离 |
| **元类注册** | `MetaContextProcessor` | 处理器自动注册 |

**铁轨（Rail）机制详解**：

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

铁轨是 openJiuwen 最强大的扩展机制，允许在不修改核心代码的情况下注入横切关注点。标准事件序列：

```
BEFORE_INVOKE → BEFORE_MODEL_CALL → [LLM调用] → AFTER_MODEL_CALL → 
BEFORE_TOOL_CALL → [工具执行] → AFTER_TOOL_CALL → AFTER_REACT_ITERATION → 
... → AFTER_INVOKE
```

---

## 二、ReActAgent 核心执行链路

### 2.1 完整执行时序

```
用户输入
   │
   ▼
ReActAgent.invoke(inputs, session)
   │
   ├─ 1. 解析输入（dict/str → query）
   ├─ 2. 自动创建 Session（如未提供）
   │      create_agent_session(session_id, card)
   │      session.pre_run(inputs)
   │
   ▼
_inner_invoke(session, inputs, query, ...)
   │
   ├─ 3. 构建 AgentCallbackContext
   │      bind_usage_invocation_id / bind_usage_attribution
   │
   ├─ 4. 加载中断状态（支持恢复）
   │      _hitl_handler.load(session) / _load_interruption_state(session)
   │
   ├─ 5. 初始化 ModelContext
   │      _init_context(session) → context_engine.create_context(...)
   │
   ├─ 6. 构建系统提示词
   │      _build_rendered_system_prompt(inputs) → PromptTemplate.format()
   │      _update_skill_prompt_builder_section() 注入技能
   │
   ├─ 7. 获取工具列表
   │      ability_manager.list_tool_info()
   │
   ├─ 8. 注入用户消息
   │      _admit_user_message(ctx, context, parts, source="query")
   │      ├─ fire(ON_USER_MESSAGE) 铁轨
   │      └─ context.add_messages(UserMessage(...))
   │
   ▼
_inner_loop (ReAct 循环核心)
   │
   └─ for iteration in range(start_iteration, max_iterations):
        │
        ├─ 9. 检查 force_finish 请求
        │
        ├─ 10. 排空转向队列（steering queue）
        │       _drain_steering_batch(ctx) → _admit_user_message
        │
        ├─ 11. 调用 LLM（_call_model）
        │       ├─ fire(BEFORE_MODEL_CALL) 铁轨
        │       ├─ _build_preview_messages(context)
        │       ├─ prompt_builder.build() → final_system
        │       ├─ context.get_context_window(system_messages, tools)
        │       │   └─ 应用所有 ContextProcessor（压缩/卸载/重排）
        │       ├─ llm.invoke(messages, tools) 或 llm.stream(...)
        │       └─ fire(AFTER_MODEL_CALL) 铁轨
        │
        ├─ 12. 写入 AssistantMessage
        │       context.add_messages(ai_message)
        │
        ├─ 13. 无工具调用 → 结束循环
        │       if not ai_message.tool_calls: break
        │
        ├─ 14. 执行工具调用（_execute_tool_call）
        │       ├─ fire(BEFORE_TOOL_CALL) 铁轨
        │       ├─ ability_manager.execute(ctx, tool_calls, session, parallel=True)
        │       │   └─ 详见工具系统深度分析
        │       ├─ context.add_messages(tool_message)  # 结果回填
        │       └─ fire(AFTER_TOOL_CALL) 铁轨
        │
        ├─ 15. 检查中断状态
        │       _after_execute_tool_call() → InterruptionState
        │
        └─ 16. fire(AFTER_REACT_ITERATION) 铁轨
```

### 2.2 关键状态机

ReActAgent 内部存在两个关键状态机：

**1. 中断恢复状态机**：

```
IDLE ──invoke──→ RUNNING ──工具中断──→ INTERRUPTED
                                          │
                    用户回复 ──resume──→ RESUMING
                                          │
                                          └──→ RUNNING（继续循环）
```

实现代码（`react_agent.py` L2480-2900）：

```python
# 中断检测
def _is_interrupted(self, tool_result: Any) -> bool:
    if isinstance(tool_result, WorkflowOutput):
        return tool_result.state == WorkflowExecutionState.INPUT_REQUIRED
    ...

# 中断提交：写入占位 ToolMessage + 持久化状态 + 保存上下文
async def _commit_interrupt(self, interrupt, context, session, invoke_inputs, ...):
    pending_entry = interrupt.interrupted_workflows[interrupt.pending_workflow_id]
    await context.add_messages(ToolMessage(
        tool_call_id=pending_entry.tool_call.id,
        content="[INTERRUPTED - Waiting for user input]",
    ))
    await self.context_engine.save_contexts(session)
    self._save_interruption_state(interrupt, session)

# 恢复：收集所有中断点的反馈后并发恢复
async def _handle_resume(self, interruption_state, user_input, ...):
    # 1. 记录当前 pending workflow 的反馈
    pending_entry.collected_input = interactive_input
    # 2. 检查是否所有中断点都已收集反馈
    all_collected = all(entry.collected_input is not None ...)
    # 3. 全部收集 → 并发恢复所有工作流
    if all_collected:
        results = await self._execute_tool_call(ctx, all_tool_calls, ...)
```

**2. 流式处理状态机**：

```python
# 流式调用路径
async for chunk in llm.stream(model, messages, tools, **extra_kwargs):
    accumulated_chunk = accumulated_chunk + chunk  # __add__ 累加
    
    # 实时写入推理内容
    if chunk.reasoning_content:
        await session.write_stream(OutputSchema(type="llm_reasoning", ...))
    
    # 实时写入输出内容
    if chunk.content:
        await session.write_stream(OutputSchema(type="llm_output", ...))

# 最终合并
ai_message = AssistantMessage(
    content=accumulated_chunk.content,
    tool_calls=accumulated_chunk.tool_calls,
    usage_metadata=accumulated_chunk.usage_metadata,
    ...
)
```

### 2.3 错误恢复机制

ReActAgent 实现了**双层错误恢复**：

```python
# 第一层：模型异常恢复（上下文溢出）
async def _recover_from_model_exception(self, ctx, *, context, exception):
    recover = self.context_engine.recover_from_model_exception(
        context_id=context.context_id(),
        session=ctx.session,
        exception=exception,
    )
    # 返回 True → 重试当前步骤；False → 抛出异常

# 第二层：取消/异常时的上下文保护
async def _handle_context_abort(self, session, *, marker, commit_session):
    await asyncio.shield(self._cleanup_context_after_abort(session, marker=marker))
    return await self._persist_context_after_abort(...)
```

---

## 三、ContextEngine 深度分析

### 3.1 架构总览

**文件位置**：`openjiuwen/core/context_engine/context_engine.py`

```python
class ContextEngine:
    def __init__(self, config, workspace=None, sys_operation=None):
        self._config = config or ContextEngineConfig()
        self._context_pool: Dict[str, ModelContext] = dict()  # 上下文缓存池
        self._window_mutators: List[Callable] = []           # 窗口变异器
```

**核心职责**：
1. **上下文生命周期管理**：创建、缓存、恢复、销毁
2. **处理器链编排**：按序应用所有注册的 ContextProcessor
3. **Token 计数策略**：多层 fallback（本地 tokenizer → tiktoken → 字符串长度）
4. **溢出恢复**：检测上下文溢出错误并自动压缩

### 3.2 上下文创建与缓存

```python
async def create_context(self, context_id, session=None, *, processors=None, ...):
    full_context_id = f"{session_id}_{context_id}"
    
    # 缓存命中 → 更新 session 引用并返回
    if full_context_id in self._context_pool:
        context = self._context_pool.get(full_context_id)
        context.set_session_ref(session)
        self._load_state_from_session(context, session, history_messages)
        return context

    # 创建新上下文
    processor_instances = [
        self._create_processor(processor_type, processor_config)
        for processor_type, processor_config in (processors or [])
    ]
    token_counter = self._select_token_counter(self._config)
    
    context = SessionModelContext(
        context_id, session_id, self._config,
        history_messages=history_messages or [],
        processors=processor_instances,
        token_counter=token_counter,
        ...
    )
    self._context_pool[full_context_id] = context
    return context
```

### 3.3 上下文处理器链

**处理器基类**（`processor/base.py`）：

```python
class ContextProcessor(metaclass=MetaContextProcessor):
    """抽象基类，提供两个生命周期切入点"""
    
    # 消息添加时触发
    async def on_add_messages(self, context, messages_to_add, **kwargs):
        return None, messages_to_add  # 默认透传
    
    async def trigger_add_messages(self, context, messages_to_add, **kwargs):
        return False  # 默认不干预
    
    # 获取上下文窗口时触发
    async def on_get_context_window(self, context, context_window, **kwargs):
        return None, context_window  # 默认透传
    
    async def trigger_get_context_window(self, context, context_window, **kwargs):
        return False  # 默认不干预
```

**已实现的处理器类型**：

| 分类 | 处理器 | 功能 |
|------|--------|------|
| 压缩 | `FullCompactProcessor` | 全量摘要压缩 |
| 压缩 | `MicroCompactProcessor` | 微压缩 |
| 压缩 | `DialogueCompressor` | 对话轮次压缩 |
| 压缩 | `RoundLevelCompressor` | 轮次级压缩 |
| 卸载 | `MessageOffloader` | 消息卸载 |
| 卸载 | `MessageSummaryOffloader` | 摘要卸载 |
| 卸载 | `ToolResultBudgetProcessor` | 工具结果预算控制 |
| 卸载 | `ToolResultWindowProcessor` | 工具结果窗口控制 |
| 守护 | `BudgetGuardProcessor` | 预算守护 |

### 3.4 FullCompactProcessor 实现细节

**触发条件**：当估计的上下文窗口 token 数超过 `trigger_total_tokens`（默认 180000）。

```python
class FullCompactProcessorConfig(BaseModel):
    trigger_total_tokens: int = 180000    # 触发压缩的 token 阈值
    compression_call_max_tokens: int = 200000  # 摘要生成的 token 预算
    messages_to_keep: int = 10            # 压缩后保留的最近消息数
    session_memory_enabled: bool = True   # 是否启用会话记忆
```

**压缩流程**：

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

**BASE_COMPACT_PROMPT 设计**：

```python
BASE_COMPACT_PROMPT = (
    NO_TOOLS_PREAMBLE  # 明确告知不使用工具
    + "Your task is to create a detailed summary of the conversation so far..."
    + DETAILED_ANALYSIS_INSTRUCTION  # 分析指令
    + """
Your summary should include the following sections:
1. Primary Request and Intent
2. Key Technical Concepts
3. Files and Code Sections
4. Errors and fixes
5. Problem Solving
6. All user messages
7. Pending Tasks
8. Current Work
9. Optional Next Step
"""
)
```

### 3.5 Token 计数算法

**多层 fallback 策略**：

```python
@staticmethod
def _select_token_counter(config: ContextEngineConfig) -> TokenCounter:
    # 1. 尝试本地 tokenizer（基于 model_name/provider）
    try:
        return TokenizerSelector(
            provider=config.model_provider,
            model=config.model_name,
            spec=config.tokenizer_spec,
            manager=TokenizerArtifactManager(offline=True),
        ).select()
    except Exception:
        # 2. 最终兜底：字符串长度计数
        return StringLengthCounter(model=config.model_name)
```

**TokenizerSelector 选择优先级**：
1. HuggingFace 本地 tokenizer（如果模型已下载）
2. tiktoken 远程 tokenizer（需下载，受 `offline=True` 控制）
3. StringLengthCounter（最终兜底）

### 3.6 上下文溢出恢复

```python
_CONTEXT_OVERFLOW_PHRASES = (
    "context length", "context window", "maximum context",
    "context limit", "prompt is too long", "prompt too long",
    "input is too long", "input too long",
)

async def recover_from_model_exception(self, *, context_id, session, exception, ...):
    # 1. 检测是否为上下文溢出错误（基于错误消息匹配）
    if not self.is_context_overflow_error(exception):
        return False
    # 2. 执行压缩
    result = await self.compress_context(context_id=context_id, session=session)
    return result_code == "compressed"
```

---

## 四、记忆系统深度分析

### 4.1 LongTermMemory 架构

**文件位置**：`openjiuwen/core/memory/long_term_memory.py`（64KB）

采用 **Singleton** 模式，全局唯一记忆引擎：

```python
class LongTermMemory(metaclass=Singleton):
    def __init__(self):
        # 存储后端（4 种）
        self.kv_store: BaseKVStore | None = None        # 快速结构化数据
        self.vector_store: BaseVectorStore | None = None # 向量相似度搜索
        self.db_store: BaseDbStore | None = None         # 持久化存储
        self.message_store: BaseMessageStore | None = None # 消息持久化
        
        # 记忆管理器（5 种类型各一个管理器）
        self.fragment_memory_manager: FragmentMemoryManager   # 片段记忆
        self.variable_manager: VariableManager                 # 变量
        self.write_manager: WriteManager                       # 写入协调
        self.summary_manager: SummaryManager                   # 摘要
        self.search_manager: SearchManager                     # 搜索
        self.generator: Generator                              # 记忆生成器
```

### 4.2 五种记忆类型

```python
class MemoryType(Enum):
    USER_PROFILE = "user_profile"       # 用户画像
    EPISODIC_MEMORY = "episodic_memory" # 情景记忆
    SEMANTIC_MEMORY = "semantic_memory" # 语义记忆
    VARIABLE = "variable"               # 变量
    SUMMARY = "summary"                 # 摘要
```

**各类记忆的实现差异**：

| 记忆类型 | 存储位置 | 检索方式 | 用途 |
|----------|----------|----------|------|
| User Profile | Vector Store + DB | 语义搜索 | 用户偏好、习惯 |
| Episodic Memory | Vector Store + DB | 时间+语义 | 历史事件、经验 |
| Semantic Memory | Vector Store + DB | 语义搜索 | 概念、知识 |
| Variable | KV Store | Key 精确查找 | 配置、状态 |
| Summary | Vector Store + DB | 语义搜索 | 会话摘要 |

### 4.3 记忆添加流程

```python
async def add_messages(self, messages, agent_config, *, user_id, scope_id, session_id, ...):
    # 1. 获取用户级分布式锁（防并发写入冲突）
    lock = DistributedLock(self.kv_store, f"user/{user_id}")
    async with lock:
        # 2. 获取 scope 配置的 LLM 和 Embedding 模型
        llm = await self._get_scope_llm(scope_id)
        scope_config = await self._get_scope_config(scope_id)
        
        # 3. 写入原始消息
        for msg in messages:
            add_req = MessageAddRequest(user_id, scope_id, role, content, ...)
            msg_id = await self.message_manager.add(add_req)
        
        # 4. 获取历史消息上下文
        history_messages = await self._get_history_messages(
            user_id, scope_id, session_id, 
            history_window_size=gen_mem_with_history_msg_num
        )
        
        # 5. 生成记忆（通过 Generator）
        all_memory = await self.generator.gen_all_memory(
            scope_id, user_id, messages, history_messages, 
            config=agent_config, base_chat_model=llm, ...
        )
        
        # 6. 写入各类型存储
        write_result = await self.write_manager.add_memories(
            user_id, scope_id, memories=all_memory, llm=llm
        )
```

### 4.4 记忆搜索与注入

```python
# 搜索参数
search_params = SearchParams(
    query=query,
    user_id=user_id,
    scope_id=scope_id,
    memory_types=[MemoryType.SEMANTIC_MEMORY, MemoryType.EPISODIC_MEMORY],
    top_k=10,
)

# 执行搜索
results = await self.search_manager.search(search_params)
```

**搜索结果结构**：

```python
class MemResult(BaseModel):
    mem_info: MemInfo          # 记忆信息（id/content/type/timestamp）
    score: float = 0.0         # 相关性得分
```

### 4.5 记忆加密

```python
# AES 加密存储
codec = AesStorageCodec(config.crypto_key)
encrypted_config.model_client_cfg.api_key = self._storage_codec.encode(...)
```

敏感记忆（如 API Key）使用 AES 加密后存储，读取时解密。

---

## 五、工具系统深度分析

### 5.1 AbilityManager 架构

**文件位置**：`openjiuwen/core/single_agent/ability_manager.py`（1512 行）

AbilityManager 是工具/工作流/MCP 的统一注册中心：

```python
class AbilityManager:
    def __init__(self, owner_id=None):
        self._tools: Dict[str, ToolCard] = {}
        self._workflows: Dict[str, WorkflowCard] = {}
        self._agents: Dict[str, AgentCard] = {}
        self._mcp_servers: Dict[str, McpServerConfig] = {}
        self._mcp_tool_allowlists: Dict[str, frozenset[str]] = {}
        self._context_engine = None
        self._progressive_tool_enabled = False
        self._direct_tool_names = {"tool_search"}
        self._registry_revision = 0  # 注册表版本号
```

### 5.2 渐进式工具暴露

```python
def _apply_tool_exposure_policy(self, card: ToolCard) -> ToolExposure:
    # 1. 显式声明优先
    if declared_marker:
        return current
    # 2. 直接暴露列表（如 tool_search）
    if name in self._direct_tool_names:
        resolved = ToolExposure.DIRECT
    # 3. 根据策略决定
    else:
        resolved = (
            ToolExposure.DEFERRED    # 延迟暴露（需要时搜索）
            if self._progressive_tool_enabled
            else ToolExposure.DIRECT  # 直接暴露
        )
    card.exposure = resolved
```

**渐进式暴露的动机**：避免工具过多导致上下文溢出。当工具数量超过 20+ 时，工具定义本身可能占用大量 token。

### 5.3 工具执行流程

```python
async def execute(self, ctx, tool_call, session, parallel_tool_calls=True):
    # 1. 标准化工具调用格式
    tool_calls = self._normalize_tool_calls(tool_call)
    
    # 2. 创建执行任务（每个调用包装为 coroutine）
    tasks = [self._invoke_tool(call, session) for call in tool_calls]
    
    # 3. 执行策略选择
    if parallel_tool_calls:
        results = await self._execute_parallel_tool_tasks(tool_calls, tasks)
    else:
        results = await asyncio.gather(*tasks, return_exceptions=True)
    
    return results
```

### 5.4 资源有序执行

**核心问题**：当 LLM 对同一文件发出多个操作（read → edit → write）时，并行执行会导致读写冲突。

**解决方案**：Lane-based 执行模型

```python
@classmethod
async def _execute_resource_ordered_tool_tasks(cls, tool_calls, tasks):
    # 1. 按资源 key 分组
    lanes: Dict[str, List[int]] = {}
    for index, call in enumerate(tool_calls):
        resource_key = cls._tool_execution_resource_key(call)
        # 未知资源的调用放入独立 lane，保持并行
        lane_key = resource_key or f"independent:{index}"
        lanes.setdefault(lane_key, []).append(index)
    
    # 2. 不同 lane 并行，同一 lane 内串行
    async def _run_lane(indices):
        lane_results = []
        for index in indices:  # 串行执行
            result = await tasks[index]
            lane_results.append((index, result))
        return lane_results
    
    lane_results = await asyncio.gather(*(_run_lane(indices) for indices in lanes.values()))
```

**资源 key 计算**：

```python
_FILE_PATH_TOOL_NAMES = frozenset({"read_file", "write_file", "edit_file"})

@classmethod
def _tool_execution_resource_key(cls, tool_call):
    if tool_call.name not in cls._FILE_PATH_TOOL_NAMES:
        return None
    arguments = cls._parse_tool_arguments(tool_call.arguments)
    file_path = arguments.get("file_path")
    # 路径标准化：normcase + abspath + expanduser
    normalized_path = os.path.normcase(os.path.abspath(os.path.expanduser(file_path)))
    return f"file:{normalized_path}"
```

### 5.5 工具超时控制

```python
DEFAULT_TOOL_CALL_TIMEOUT = 300.0  # 默认 5 分钟
MAX_TOOL_CALL_TIMEOUT_HARD_LIMIT = 3600.0  # 硬上限 1 小时

@staticmethod
def _resolve_call_timeout(tool_card):
    # 1. ToolCard.properties["resilience"]["timeout_s"] 优先
    # 2. 缺失 → DEFAULT_TOOL_CALL_TIMEOUT
    # 3. None → 豁免（但仍受硬上限约束）
```

**超时实现**：

```python
# 使用 anyio.fail_after 实现超时
with anyio.fail_after(timeout):
    result = await tool.invoke(...)
```

### 5.6 MCP 集成

```python
# MCP 工具命名规则
def mcp_model_tool_name(server_name, tool_name):
    return f"mcp_{server_name}_{tool_name}"

# 白名单机制
def set_mcp_tool_allowlist(self, mcp_server, tool_names):
    server_id = str(mcp_server.server_id or "").strip()
    self._mcp_tool_allowlists[server_id] = normalized_names
```

---

## 六、安全机制深度分析

### 6.1 PermissionEngine 三级防护架构

**文件位置**：`openjiuwen/harness/security/permission_engine/core.py`

```python
class PermissionEngine:
    def __init__(self, config, llm=None, model_name=None, workspace_root=None, trusted_dirs=None):
        self.config = prepare_permissions_for_engine(config)
        self._file_guard: FileGuardChecker = build_file_guard_checker(...)
        self._net_guard: NetGuardChecker = build_net_guard_checker(...)
```

**判定管线**：

```
                    工具调用
                       │
        ┌──────────────┼──────────────┐
        ▼              ▼              ▼
   result_A        result_B       result_C
  ToolGuard       FileGuard      NetGuard
  (工具规则)      (路径防护)     (网络防护)
        │              │              │
        └──────────────┼──────────────┘
                       ▼
              strictest(A, B, C)  ← 取最严格结果
                       │
                       ▼
              PermissionLevel
           (ALLOW / ASK / DENY)
```

### 6.2 三态权限策略

```python
class PermissionLevel(Enum):
    ALLOW = "allow"   # 允许执行
    ASK = "ask"       # 询问用户
    DENY = "deny"     # 拒绝执行
```

**严格度比较**：

```python
def strictest(a: PermissionLevel, b: PermissionLevel) -> PermissionLevel:
    # DENY > ASK > ALLOW
    if a == PermissionLevel.DENY or b == PermissionLevel.DENY:
        return PermissionLevel.DENY
    if a == PermissionLevel.ASK or b == PermissionLevel.ASK:
        return PermissionLevel.ASK
    return PermissionLevel.ALLOW
```

### 6.3 配置热更新

```python
def update_config(self, config):
    """热更新配置，无需重启"""
    self.config = prepare_permissions_for_engine(config)
    self._enabled = self.config.get("enabled", True)
    self._rebuild_file_guard()  # 重建防护器
```

### 6.4 Shell AST 分析

**文件位置**：`openjiuwen/harness/security/permission_engine/toolguard/shell_ast.py`

**双后端解析策略**：

```python
def parse_shell_for_permission(command: str) -> ShellAstParseResult:
    # 1. 规范化命令
    text = canonicalize_shell_command_for_permission((command or "").strip())
    
    # 2. 尝试 tree-sitter-bash 解析
    parser = _get_tree_sitter_bash_parser()
    if parser is not None:
        return _parse_with_tree_sitter(text, parser)
    
    # 3. 回退到保守扫描器
    return _parse_with_conservative_fallback(text)
```

**tree-sitter 后端**：

```python
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

**保守扫描器**：

```python
def _parse_with_conservative_fallback(command: str) -> ShellAstParseResult:
    flags = _scan_shell_structure(command)
    
    # 检测到风险结构 → 返回 parse_unavailable（fail closed）
    if flags.has_risky_structure():
        return ShellAstParseResult(
            kind="parse_unavailable",  # 不可信，需要用户确认
            flags=flags,
            reason="tree-sitter backend unavailable and fallback detected shell structure",
        )
    
    # 简单命令 → 使用 shlex 分词
    argv = tuple(shlex.split(command, posix=(os.name != "nt")))
    subcommand = ShellSubcommand(text=command, argv=argv)
    return ShellAstParseResult(kind="simple", subcommands=(subcommand,))
```

**结构标志**：

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
```

---

## 七、多 Agent 协作深度分析

### 7.1 TeamAgent 架构

**文件位置**：`openjiuwen/agent_teams/agent/team_agent.py`（28KB）

TeamAgent 采用**组合模式**而非继承：

```python
class TeamAgent(BaseAgent):
    def __init__(self, card):
        super().__init__(card)
        # 组合多个专职管理器
        self._configurator = AgentConfigurator(card)
        self._state = TeamAgentState()
        self._spawn_manager = SpawnManager(state=self._state, configurator=self._configurator, ...)
        self._recovery_manager = RecoveryManager(configurator=self._configurator, spawn_manager=self._spawn_manager)
        self._session_manager = SessionManager(state=self._state, configurator=self._configurator, ...)
        self._stream_controller = StreamController(...)
        self._coordination = CoordinationKernel(self)  # 协调内核
```

### 7.2 双 Spawn 机制

**文件位置**：`openjiuwen/agent_teams/spawn/`

#### In-Process Spawn（协程级）

```python
async def inprocess_spawn(team_agent, ctx, *, initial_message=None, session_id=None, fork_from=None):
    # 1. 创建 TeamAgent 实例
    teammate = _TeamAgent(card)
    
    # 2. 共享 workspace 缓存（避免重复扫描）
    team_agent.share_workspace_cache_with(teammate)
    
    # 3. 配置
    teammate.configure(spec, ctx)
    
    # 4. 共享 checkpoint 字典（引用传递）
    team_agent.share_checkpoints_with(teammate)
    
    # 5. Fork 上下文注入
    if fork_from and not fork_from.is_empty():
        native = teammate.resources.harness.get_deep_agent()
        await native.create_new_context_engine(
            session_id=session_id,
            messages=fork_from.to_messages(),
        )
        # 分割点压缩
        if fork_from.compact_split is not None:
            await compact_context(native, split_at=fork_from.compact_split, ...)
    
    # 6. 创建 asyncio.Task
    run_ctx = contextvars.copy_context()  # 复制上下文变量
    task = run_ctx.run(asyncio.get_running_loop().create_task, _run())
    
    return InProcessSpawnHandle(process_id=f"inproc-{member_name}", _task=task, agent_ref=teammate)
```

#### External CLI Spawn（进程级）

```python
# 使用 Runner.spawn_agent → child_process 模式
# 通过 SpawnConfig 配置子进程参数
spawn_config = SpawnConfig(
    command="openjiuwen",
    args=["--member", member_name],
    env={...},
)
```

### 7.3 Fork 上下文实现

**文件位置**：`openjiuwen/agent_teams/fork.py`

ForkContext 是可序列化的对话历史快照：

```python
@dataclass
class ForkContext:
    messages: list[dict]           # 编码后的消息字典（可跨进程传输）
    compact_split: int | None = None  # 压缩分割点
    compact_direction: str = "before"  # 保留方向

    @classmethod
    def from_agent(cls, agent, *, session_id=None, checkpoint=None, keep="before"):
        # 获取当前上下文
        msgs = agent.get_current_context(session_id=session_id)
        
        # 剥离 SystemMessage（防止角色泄露）
        msgs = [m for m in msgs if not isinstance(m, SystemMessage)]
        
        # 按 checkpoint 截断
        if checkpoint is not None:
            if keep == "after":
                # 保留后半部分，裁剪前导孤立的 ToolMessage
                msgs = cls._trim_leading_orphan_tool_messages(msgs[checkpoint:])
            else:
                # 保留前半部分，携带边界处的 ToolMessage
                truncated = msgs[:checkpoint]
                last = truncated[-1] if truncated else None
                if isinstance(last, AssistantMessage) and getattr(last, "tool_calls", None):
                    # 携带后续 ToolMessage，避免悬空 tool call
                    while i < len(msgs) and isinstance(msgs[i], ToolMessage):
                        truncated.append(msgs[i])
                msgs = truncated
        
        return cls(messages=[encode_message(m) for m in msgs])
```

**Fork 的意义**：子 Agent 继承父 Agent 的对话历史，具备上下文连续性，同时通过 compact_split 控制上下文长度。

### 7.4 团队协调内核

```python
class CoordinationKernel:
    def __init__(self, team_agent):
        self.team_agent = team_agent
        self.event_bus = EventBus()  # 事件总线
        # 调度器、生命周期管理等
```

**核心能力**：
- 事件分发（Mailbox 消息路由）
- 成员生命周期管理（启动/停止/故障恢复）
- 任务分配与追踪

---

## 八、Workflow 图执行引擎

### 8.1 Pregel 模式实现

**文件位置**：`openjiuwen/core/graph/graph.py`

Workflow 基于 **Pregel 图执行引擎**，实现 BSP（Bulk Synchronous Parallel）计算模型：

```python
class PregelGraph(Graph):
    def __init__(self):
        self.pregel: Pregel | None = None
        self.edges: list[Tuple[str | list[str]], str]] = []      # 普通边
        self.waits: set[str] = set()                             # 等待全部前驱的节点
        self.nodes: dict[str, Vertex] = {}                       # 顶点
        self.branches: defaultdict[str, dict[str, Branch]] = ... # 条件分支
        self.branch_targets: dict[str, set[str}] = {}            # 分支目标映射
```

**Pregel 执行模型**：

```
Superstep 1: 所有起始节点并行执行
      │
      ▼
Superstep 2: 接收消息的节点并行执行
      │
      ▼
Superstep 3: ...（直到无节点活跃或达到最大递归深度）
      │
      ▼
      END
```

### 8.2 组件化节点设计

```python
class Workflow:
    def set_start_comp(self, start_comp_id, component, inputs_schema, outputs_schema):
        self._internal.add_workflow_comp(start_comp_id, component, ...)
        self._internal.start_comp(start_comp_id)
    
    def add_workflow_comp(self, comp_id, workflow_comp, *, 
                          wait_for_all=False, inputs_schema=None, 
                          max_retries=0, timeout=-1.0, exception_config=None):
        ...
    
    def add_conditional_edges(self, source_node_id, router):
        # 条件分支
        ...
```

**组件能力（ComponentAbility）**：
- 流式输出
- 批量处理
- 中断恢复
- 错误处理

### 8.3 变量映射

```python
# 使用 ${} 语法进行变量引用
flow.set_start_comp("start", start, inputs_schema={"query": "${query}"})
flow.add_workflow_comp("llm", llm, inputs_schema={"query": "${start.query}"})
flow.set_end_comp("end", end, inputs_schema={"output": "${llm.output}"})
```

### 8.4 分支与条件执行

```python
# 条件边实现
def add_conditional_edges(self, source_node_id, router: Router):
    name = _get_callable_name(router)
    self.branches[source_node_id][name] = Branch(router)

# 分支屏障解析（CNF OR-groups）
def _resolve_barrier_groups(self, target_id, source_list):
    # 处理互斥前驱节点的合并逻辑
    # 嵌套分支节点提升到根分支祖先
    ...
```

---

## 九、Agent 进化机制

### 9.1 进化架构总览

**文件位置**：`openjiuwen/agent_evolving/`

```
agent_evolving/
├── agent_rl/              # 在线 RL 训练
│   ├── online/            # 在线学习（FastAPI 服务）
│   │   ├── service.py     # RL 服务主入口
│   │   ├── capture_pipeline.py  # 轨迹捕获管线
│   │   ├── task_registry.py     # 任务注册表
│   │   └── training_runner.py   # 训练运行器
│   ├── offline/           # 离线学习
│   └── reward/            # 奖励函数
├── optimizer/             # 优化器
├── trajectory/            # 轨迹收集
│   ├── model.py           # Trajectory 不可变值对象
│   ├── schema.py          # 轨迹模式定义
│   └── serialization.py   # 序列化
├── evaluator/             # 评估器
├── experience/            # 经验回放
└── prompts/               # 提示词进化
```

### 9.2 在线 RL 训练服务

**文件位置**：`openjiuwen/agent_evolving/agent_rl/online/service.py`

基于 FastAPI 的独立 HTTP 服务：

```python
def build_rl_service_app(*, model_id, redis, trajectory_store, task_registry, 
                         capture_pipeline, training_runner, trajectory_api):
    app = FastAPI(title="OpenJiuwen RL Service", lifespan=lifespan)
    
    @app.post("/v1/rl/tasks/start")
    async def start_task(payload, agent_session_id, gateway_task_id, ...):
        spec = TaskSpec(rl_task_id=task_id, agent_session_id=..., 
                        model_id=model_id, policy_lora_name=policy_name, ...)
        result = await task_registry.start(spec)
        return JSONResponse(status_code=201, content=result.task.to_dict())
    
    @app.post("/v1/rl/tasks/{rl_task_id}/reward")
    async def reward_task(rl_task_id, payload):
        # 提交奖励
        return {"sample_count": await capture_pipeline.submit_reward(rl_task_id, payload["reward"])}
    
    @app.post("/v1/rl/training/runs")
    async def start_training_run(payload):
        result = await training_runner.start()
        return JSONResponse(status_code=201, content=_record(result.run))
```

### 9.3 轨迹模型

**文件位置**：`openjiuwen/agent_evolving/trajectory/model.py`

Trajectory 是不可变值对象，拥有 OTLP JSON 格式的有效载荷：

```python
class Trajectory:
    """不可变值对象，拥有单一 OTLP 轨迹有效载荷"""
    __slots__ = ("_payload", "_sealed")
    
    def __init__(self, payload, *, _allow_missing_session=False):
        # 验证 payload 结构
        resource_spans = payload.get("resourceSpans")
        if not isinstance(resource_spans, list) or not resource_spans:
            raise ValueError("trajectory payload must contain non-empty resourceSpans")
        
        # 深拷贝 + 密封
        object.__setattr__(self, "_payload", _copy_json(payload))
        object.__setattr__(self, "_sealed", True)
    
    @property
    def trajectory_id(self) -> str:
        return str(self.resource_attributes[TRAJECTORY_ID])
    
    def to_otlp(self) -> dict:
        """返回独立的 JSON 副本"""
        return _copy_json(self._payload)
```

### 9.4 RL 训练循环

```python
class TrainingRunner:
    async def start(self):
        # 1. 检查样本充足性
        # 2. 启动训练运行
        # 3. 返回运行 ID
    
    async def stop(self, training_run_id):
        # 停止训练运行
```

**训练数据流**：

```
Agent 执行 → CapturePipeline 捕获轨迹 → TrajectoryStore 存储
                    │
                    ▼
         TrainingRunner 采样批次 → 策略更新 → LoRA 权重
                    │
                    ▼
         新策略部署 → Agent 继续执行（循环）
```

### 9.5 Prompt 优化

```python
# optimizer/ 目录
# - base.py: 优化器基类
# - llm_resilience.py: LLM 韧性优化器
```

---

## 十、性能优化深度分析

### 10.1 异步 IO 全面应用

openJiuwen 全链路采用 `asyncio` 异步 IO：

```python
# LLM 调用异步化
ai_message = await llm.invoke(model=model_name, messages=..., tools=...)

# 工具执行并行化
results = await asyncio.gather(*tasks, return_exceptions=True)

# 存储操作异步化
await self.kv_store.set(key, value)
await self.vector_store.add(vectors, metadatas)
```

### 10.2 流式处理

**LLM 流式调用**：

```python
# 流式路径：实时写入 session，首 token 时间可观测
async for chunk in llm.stream(model, messages, tools, **extra_kwargs):
    if accumulated_chunk is None:
        accumulated_chunk = chunk
    else:
        accumulated_chunk = accumulated_chunk + chunk  # 累加器模式
    
    # 实时输出
    if chunk.content:
        await session.write_stream(OutputSchema(type="llm_output", ...))
```

**性能指标收集**：

```python
# TTFT (Time To First Token)
if call_first_token_time is None:
    call_first_token_time = time.monotonic()

# TPOT (Time Per Output Token)
if call_first_token_time and call_last_token_time and output_tokens > 1:
    perf_metrics["tpot_ms"] = (call_last_token_time - call_first_token_time) / (output_tokens - 1) * 1000
```

### 10.3 连接池与缓存

**上下文缓存池**：

```python
class ContextEngine:
    def __init__(self, ...):
        self._context_pool: Dict[str, ModelContext] = dict()
    
    async def create_context(self, context_id, session, ...):
        full_context_id = f"{session_id}_{context_id}"
        if full_context_id in self._context_pool:
            return self._context_pool[full_context_id]  # 缓存命中
        # 创建并缓存
        self._context_pool[full_context_id] = context
```

**Embedding 模型缓存**：

```python
class LongTermMemory:
    def __init__(self):
        self._scope_embedding: dict[str, Embedding] = {}  # scope 级缓存
    
    async def _apply_scope_embedding(self, scope_id):
        if scope_id not in self._scope_embedding:
            self._scope_embedding[scope_id] = await self._load_embedding(scope_id)
```

### 10.4 注册表版本号

```python
class AbilityManager:
    def __init__(self, ...):
        self._registry_revision = 0  # 单调递增版本号
    
    def _mark_registry_changed(self):
        self._registry_revision += 1
    
    @property
    def registry_revision(self) -> int:
        return self._registry_revision
```

**用途**：铁轨（Rail）可通过比较版本号判断是否需要重建索引，避免每次模型调用都重建。

### 10.5 分布式锁

```python
from openjiuwen.core.memory.common.distributed_lock import DistributedLock

# 用户级分布式锁（防并发写入冲突）
lock = DistributedLock(self.kv_store, f"user/{user_id}")
async with lock:
    await self.write_manager.add_memories(user_id, scope_id, memories=all_memory, llm=llm)
```

### 10.6 上下文使用分析

```python
class ContextUsageAnalyzer:
    def analyze(self, context_window, request_id, system_prompt_sections, ...):
        # 分类统计 token 占用
        # - SYSTEM_PROMPT: 系统提示词
        # - SKILLS: 技能
        # - CONTEXT: 对话上下文
        # - TOOLS: 工具定义
        ...
```

**输出示例**：

```json
{
  "system_prompt_tokens": 2048,
  "skills_tokens": 512,
  "context_tokens": 8192,
  "tools_tokens": 4096,
  "total_tokens": 14848,
  "utilization": 0.74
}
```

---

## 十一、对 laew 的深度借鉴建议

### P0 优先级（基础能力，必须实现）

#### 1. 铁轨（Rails）机制

**laew 现状**：无钩子机制，扩展需修改核心代码。

**借鉴方案**：

```rust
// 定义铁轨事件
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

// 铁轨 trait
pub trait Rail: Send + Sync {
    fn before_events(&self) -> Vec<AgentCallbackEvent>;
    fn after_events(&self) -> Vec<AgentCallbackEvent>;
    async fn on_before(&self, ctx: &mut AgentCallbackContext, event: AgentCallbackEvent) -> Result<()>;
    async fn on_after(&self, ctx: &mut AgentCallbackContext, event: AgentCallbackEvent) -> Result<()>;
}
```

**预期收益**：
- 可扩展性大幅提升
- 支持日志、权限、压缩等横切关注点
- 第三方扩展无需修改核心代码

#### 2. 权限引擎（PermissionEngine）

**laew 现状**：零校验，Bash 工具可执行任意命令。

**借鉴方案**：

```rust
// 权限级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionLevel {
    Allow,
    Ask,
    Deny,
}

// 权限引擎核心
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

**预期收益**：
- 安全性基础保障
- 防止命令注入、敏感路径访问
- 支持 allow/ask/deny 三态策略

### P1 优先级（核心能力，建议实现）

#### 3. ContextEngine 上下文引擎

**laew 现状**：简单消息列表，无压缩/卸载策略。

**借鉴方案**：

```rust
// 上下文处理器 trait
#[async_trait]
pub trait ContextProcessor: Send + Sync {
    async fn trigger_get_context_window(&self, ctx: &ModelContext, window: &ContextWindow) -> bool;
    async fn on_get_context_window(&self, ctx: &ModelContext, window: ContextWindow) -> (Option<ContextEvent>, ContextWindow);
}

// 压缩处理器
pub struct SummaryCompressor {
    threshold_tokens: usize,
    max_recent_messages: usize,
}

#[async_trait]
impl ContextProcessor for SummaryCompressor {
    async fn on_get_context_window(&self, ctx: &ModelContext, mut window: ContextWindow) -> (Option<ContextEvent>, ContextWindow) {
        if window.total_tokens > self.threshold_tokens {
            // 触发摘要压缩
            let summary = self.summarize(&window.messages).await;
            window.messages = vec![SystemMessage::compact_boundary(), summary];
            window.messages.extend(window.recent_messages(self.max_recent_messages));
        }
        (None, window)
    }
}
```

**预期收益**：
- 长对话能力（突破 token 限制）
- 可插拔的压缩策略
- 上下文使用分析

#### 4. 多类型记忆系统

**laew 现状**：仅 session_memory 表存储摘要。

**借鉴方案**：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryType {
    UserProfile,
    Episodic,
    Semantic,
    Variable,
    Summary,
}

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

**预期收益**：
- 跨会话知识积累
- 用户画像构建
- 语义检索增强

#### 5. MCP 集成

**laew 现状**：无 MCP 支持。

**借鉴方案**：

```rust
pub struct McpServerConfig {
    server_id: String,
    server_url: String,
    command: Option<String>,
    args: Vec<String>,
    env: HashMap<String, String>,
}

pub struct McpTool {
    server_name: String,
    tool_name: String,
    description: String,
    input_schema: Value,
}

impl AbilityManager {
    pub fn register_mcp_server(&mut self, config: McpServerConfig) -> Result<()> {
        // 1. 连接 MCP 服务器
        // 2. 发现工具
        // 3. 注册到 AbilityManager
    }
}
```

**预期收益**：
- 工具生态扩展
- 标准化工具接入
- 支持工具白名单

### P2 优先级（增强能力，可选实现）

#### 6. 可观测性（OpenTelemetry）

**laew 现状**：无可观测性。

**借鉴方案**：

```rust
pub struct Tracer {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
}

impl Tracer {
    pub fn start_span(&self, name: &str) -> Span {
        Span::new(name, self.trace_id.clone(), self.span_id.clone())
    }
}

pub struct Span {
    name: String,
    trace_id: String,
    parent_span_id: Option<String>,
    start_time: Instant,
    attributes: HashMap<String, Value>,
}

impl Span {
    pub fn set_attribute(&mut self, key: &str, value: Value) {
        self.attributes.insert(key.to_string(), value);
    }
    
    pub fn end(self) -> SpanRecord {
        SpanRecord {
            name: self.name,
            trace_id: self.trace_id,
            duration: self.start_time.elapsed(),
            attributes: self.attributes,
        }
    }
}
```

**预期收益**：
- 全链路追踪
- 性能瓶颈定位
- 调试效率提升

#### 7. TeamAgent 多 Agent 协作

**laew 现状**：SubAgent-Work 简单委派。

**借鉴方案**：

```rust
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
        // 4. 启动 asyncio Task
    }
}
```

**预期收益**：
- 复杂任务协作
- 上下文继承
- 故障恢复

#### 8. RL 训练进化

**laew 现状**：无进化能力。

**借鉴方案**：

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

**预期收益**：
- Agent 自我进化
- Prompt 自动优化
- 性能持续提升

---

## 十二、总结

### 12.1 工程规模

| 模块 | 代码量 | 核心文件 |
|------|--------|----------|
| core/single_agent | ~15000 行 | react_agent.py (3251 行)、ability_manager.py (1512 行) |
| core/context_engine | ~5000 行 | context_engine.py、多个 processor |
| core/memory | ~10000 行 | long_term_memory.py (64KB) |
| harness | ~20000 行 | deep_agent.py (3999 行) |
| agent_teams | ~15000 行 | team_agent.py (28KB) |
| extensions/observability | ~10000 行 | callback_handler.py (91KB) |
| agent_evolving | ~8000 行 | 多个子模块 |

### 12.2 核心设计亮点

1. **Card/Config 分离**：身份与行为解耦，支持热更新
2. **铁轨机制**：可插拔的钩子链，职责清晰
3. **渐进式工具暴露**：避免工具过多导致上下文溢出
4. **三级安全防护**：ToolGuard + FileGuard + NetGuard
5. **多类型记忆**：语义/情景/画像/变量/摘要
6. **Fork 上下文**：子 Agent 继承父 Agent 对话历史
7. **OpenTelemetry 集成**：全链路可观测
8. **在线 RL 训练**：支持 Agent 自我进化
9. **资源有序执行**：文件工具防冲突
10. **上下文溢出恢复**：自动压缩重试

### 12.3 与 laew 的对比

| 维度 | openJiuwen Core | laew |
|------|-----------------|------|
| 语言 | Python | Rust |
| Agent 类型 | ReAct/Workflow/Team | Yolo/Plan/Main-Work/Sub-Work/QC/SessionContext |
| 工具系统 | AbilityManager + MCP | Bash/Read/Write |
| 记忆系统 | 多类型 + 向量检索 | session_memory 表 |
| 安全控制 | 三级权限引擎 | 零校验 |
| 可观测性 | OpenTelemetry | 无 |
| 进化能力 | RL 训练 | 无 |
| 上下文管理 | ContextEngine + 压缩 | 简单消息列表 |
| 铁轨机制 | 完整 | 无 |
| 中断恢复 | 工作流/工具双中断 | 无 |

### 12.4 最终建议

openJiuwen Core 是**目前最完整的开源 Agent SDK 之一**，其架构设计对 laew 有极高的参考价值。建议按以下顺序逐步借鉴：

1. **P0 阶段（1-2 个月）**：铁轨机制 + 权限引擎
2. **P1 阶段（2-3 个月）**：ContextEngine + 多类型记忆 + MCP 集成
3. **P2 阶段（3-6 个月）**：可观测性 + TeamAgent + RL 训练

每一步都应保持 laew 的 Rust 高性能优势，同时吸收 openJiuwen 的工程化最佳实践。

---

**分析完成日期**：2026-09-05

**分析人**：Claude Code Agent
