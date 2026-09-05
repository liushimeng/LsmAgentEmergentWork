# openJiuwen Core 源码深度调研

## 一、工程概览

### 1.1 工程定位

**openJiuwen Core**（包名 `openjiuwen`）是华为开源的 Python AI Agent SDK，为基于 openJiuwen 框架运行的大语言模型应用提供高性能运行时。版本 `0.1.17`，Python `>=3.11,<3.14`，采用 **Apache-2.0** 许可证。

工程的核心价值：
- 提供 Agent 创建、Workflow 编排、LLM 调用、工具调用的多层次 SDK
- 内置高性能异步执行引擎，支持流式处理与状态保存/中断恢复
- 提供 Prompt 自动优化、全链路可观测性等调试优化工具

### 1.2 构建系统

- **构建后端**：`setuptools>=61`
- **包管理**：`uv`（`uv.lock` 锁定依赖）
- **镜像源**：阿里云 PyPI 镜像（`mirrors.aliyun.com`）
- **入口点**：
  - `openjiuwen` → `openjiuwen.harness.cli.cli:cli`（CLI 主命令）
  - `team-member` → `openjiuwen.agent_teams.skill.cli:run`
  - `openjiuwen-team-mcp` → `openjiuwen.agent_teams.mcp.server:main`
  - `openjiuwen-rl-service` → `openjiuwen.agent_evolving.agent_rl.online.service:main`

### 1.3 核心依赖

| 分类 | 依赖 |
|------|------|
| HTTP 客户端 | `aiohttp>=3.11.12`、`requests>=2.32.3` |
| 数据库 ORM | `sqlalchemy[asyncio]>=2.0.41`、`sqlmodel>=0.0.37` |
| AI/LLM | `anthropic>=0.120.2`、`openai>=1.108.0`、`transformers>=4.52.4`、`dashscope>=1.25.6` |
| 向量数据库 | `pymilvus>=2.6.2,<2.6.10` |
| MCP | `fastmcp>=2.14.2,<3.0`、`mcp>=1.26.0` |
| 文档处理 | `beautifulsoup4`、`trafilatura`、`pdfplumber`、`openpyxl` |
| 可观测性 | `opentelemetry-api/sdk/exporter-otlp-*` |
| 沙箱 | `agent-sandbox>=0.0.26`（可选） |

### 1.4 目录组织

```
agent-core/
├── openjiuwen/                    # 主包
│   ├── core/                      # 公共 SDK/运行时
│   │   ├── single_agent/          # 单 Agent 实现
│   │   ├── context_engine/        # 上下文引擎
│   │   ├── memory/                # 记忆系统
│   │   ├── session/               # 会话管理
│   │   ├── workflow/              # 工作流引擎
│   │   ├── retrieval/             # RAG 检索
│   │   ├── foundation/            # LLM/工具/消息基础
│   │   ├── controller/            # 控制器
│   │   ├── multi_agent/           # 多 Agent 基础
│   │   ├── security/              # 安全基础
│   │   ├── graph/                 # 图执行引擎
│   │   ├── operator/              # 算子
│   │   ├── sys_operation/         # 系统操作
│   │   └── kv_cache/              # KV 缓存
│   ├── harness/                   # 编码 Agent 框架
│   │   ├── deep_agent.py          # DeepAgent 主实现（3999行）
│   │   ├── tools/                 # 工具集
│   │   ├── rails/                 # 铁轨机制
│   │   ├── security/              # 权限引擎
│   │   ├── prompts/               # 系统提示词
│   │   ├── cli/                   # CLI 实现
│   │   ├── subagents/             # 子 Agent 定义
│   │   ├── task_loop/             # 任务循环
│   │   ├── workspace/             # 工作区
│   │   └── goal/                  # 目标管理
│   ├── agent_teams/               # 多 Agent 团队协作
│   │   ├── agent/team_agent.py    # TeamAgent（28KB）
│   │   ├── spawn/                 # 子进程/协程 spawn
│   │   ├── tools/                 # 团队工具
│   │   ├── memory/                # 团队记忆
│   │   ├── workflow/              # 团队工作流
│   │   └── mcp/                   # 团队 MCP
│   ├── extensions/                # 扩展集成
│   │   ├── a2a/                   # Agent-to-Agent 协议
│   │   ├── store/                 # 存储后端
│   │   ├── checkpointer/          # 检查点
│   │   ├── observability/         # OpenTelemetry
│   │   ├── tracer_otel/           # OTel 追踪
│   │   └── context_evolver/       # 上下文进化
│   ├── agent_evolving/            # Agent 进化（RL）
│   │   ├── agent_rl/              # 在线 RL 训练
│   │   ├── optimizer/             # 优化器
│   │   ├── trajectory/            # 轨迹收集
│   │   └── evaluator/             # 评估器
│   ├── symphony/                  # 多 Agent 协作框架
│   ├── rsi/                       # 研发流程智能化
│   ├── dev_tools/                 # 开发工具
│   └── auto_harness/              # 自动评估
├── tests/                         # 测试
│   ├── unit_tests/                # 单元测试
│   └── system_tests/              # 系统测试
├── examples/                      # 示例
├── docs/                          # 文档
└── scripts/                       # 脚本
```

---

## 二、核心架构设计

### 2.1 分层架构

openJiuwen Core 采用清晰的四层架构：

```
┌─────────────────────────────────────────────────────────┐
│                    CLI / API 入口层                       │
│  (openjiuwen.harness.cli / core.single_agent / Runner)   │
├─────────────────────────────────────────────────────────┤
│                  Agent 编排层                              │
│  DeepAgent → ReActAgent → BaseAgent                     │
│  WorkflowAgent                                          │
│  TeamAgent (agent_teams)                                │
├─────────────────────────────────────────────────────────┤
│                  能力管理层                               │
│  AbilityManager (工具/工作流/MCP 注册)                    │
│  ContextEngine (上下文管理)                              │
│  Session (会话状态)                                      │
├─────────────────────────────────────────────────────────┤
│                  基础设施层                               │
│  Model (LLM 调用) / Tool / Memory / Store / Retrieval   │
└─────────────────────────────────────────────────────────┘
```

### 2.2 核心抽象

#### 2.2.1 Card/Config 分离

工程遵循 **Card 定义身份/元数据，Config 定义运行时行为** 的设计原则：

- `AgentCard`：Agent 身份（id/name/description）
- `ToolCard`：工具元数据（name/description/input_schema/exposure/properties）
- `WorkflowCard`：工作流卡片
- `McpServerConfig`：MCP 服务器配置

#### 2.2.2 统一消息模型

`openjiuwen.core.foundation.llm` 定义了协议无关的消息模型：

```python
BaseMessage
├── SystemMessage
├── UserMessage
├── AssistantMessage
└── ToolMessage
```

所有 Agent 循环与工具层只接触统一消息，协议差异封闭在 LLM 客户端内部。

#### 2.2.3 Runner 全局资源管理

`Runner.resource_mgr` 是进程全局状态管理器，负责：
- 工作流注册/获取
- 工具实例管理
- 系统操作（SysOperation）管理
- 回调框架（CallbackFramework）

### 2.3 关键设计模式

| 模式 | 应用 |
|------|------|
| 模板方法 | `BaseAgent.invoke()` → 子类实现具体逻辑 |
| 装饰器/铁轨 | `@rail(before, after, on_exception)` 钩子机制 |
| 观察者 | `AgentCallbackManager` 事件注册/触发 |
| 策略 | `ContextProcessor` 可插拔压缩策略 |
| 单例 | `LongTermMemory` 全局唯一记忆引擎 |
| 工厂 | `Runner.resource_mgr` 资源创建/获取 |
| 桥接 | `DeepAgent` 包装 `ReActAgent`，共享 AbilityManager |

---

## 三、Agent 实现

### 3.1 类继承体系

```
BaseAgent (core/single_agent/base.py)
├── ReActAgent (core/single_agent/agents/react_agent.py, 3251行)
│   └── 被 DeepAgent 内部包装
├── WorkflowAgent (core/application/workflow_agent.py)
└── MultiTaskAgent

DeepAgent (harness/deep_agent.py, 3999行)
    └── 包装 ReActAgent，增加：
        - 任务循环（TaskLoop）
        - 铁轨系统（Rails）
        - 交互管理（Interaction）
        - 子 Agent 管理
        - 目标管理（GoalManager）

TeamAgent (agent_teams/agent/team_agent.py, 28KB)
    └── 多 Agent 协作的 Team Leader
```

### 3.2 ReActAgent 核心实现

**文件路径**：`openjiuwen/core/single_agent/agents/react_agent.py`

**核心配置类**：`ReActAgentConfig`（Pydantic BaseModel）

```python
class ReActAgentConfig(BaseModel):
    max_iterations: int = 5
    model_name: str = ""
    model_provider: str = "openai"
    api_key: str = ""
    api_base: str = ""
    context_engine_config: ContextEngineConfig
    parallel_tool_calls: bool = True
    llm_return_token_ids: bool = False  # RL 轨迹收集
    llm_logprobs: bool = False
```

**核心循环**（invoke → _inner_invoke → _inner_loop）：

```python
async def invoke(self, inputs, session=None, **kwargs):
    # 1. 解析输入（dict/str）
    # 2. 自动创建 Session（如未提供）
    # 3. 调用 _inner_invoke
    ...

async def _inner_invoke(self, session, inputs, query, ...):
    # 1. 初始化 ModelContext
    # 2. 注入用户消息
    # 3. 进入 _inner_loop（ReAct 循环）
    ...

async def _inner_loop(self, ctx, context, ...):
    while iteration < max_iterations:
        # BEFORE_MODEL_CALL 铁轨
        ai_message = await self._call_model(ctx, context, tools)
        # AFTER_MODEL_CALL 铁轨
        if not ai_message.tool_calls:
            break  # 无工具调用 → 结束
        # BEFORE_TOOL_CALL 铁轨
        results = await self._execute_tool_call(ctx, tool_calls, session, context)
        # AFTER_TOOL_CALL 铁轨
        iteration += 1
```

**工具执行**：

```python
async def _execute_tool_call(self, ctx, tool_calls, session, context):
    results = await self.ability_manager.execute(
        ctx=ctx,
        tool_call=tool_calls,
        session=session,
        parallel_tool_calls=self._config.parallel_tool_calls,
    )
    for tool_result, tool_message in results:
        if tool_message is not None:
            await context.add_messages(tool_message)
    return results
```

### 3.3 DeepAgent 核心实现

**文件路径**：`openjiuwen/harness/deep_agent.py`

DeepAgent 是 harness 层的核心 Agent，包装 ReActAgent 并扩展：

**关键特性**：
1. **任务循环**（TaskLoop）：支持多轮任务迭代
2. **铁轨系统**（Rails）：可插拔的钩子链
3. **交互管理**（Interaction）：处理用户输入/输出
4. **子 Agent 管理**：通过 TaskTool 委派
5. **目标管理**（GoalManager）：追踪任务目标

**配置热更新**：

```python
def configure(self, config: DeepAgentConfig) -> "DeepAgent":
    if self._deep_config is None:
        self._initial_configure(config)
    else:
        self._hot_reconfigure(config)  # 热更新，无需重启
    return self
```

**铁轨桥接**：

```python
_BRIDGE_EVENTS = frozenset({
    AgentCallbackEvent.BEFORE_MODEL_CALL,
    AgentCallbackEvent.AFTER_MODEL_CALL,
    AgentCallbackEvent.BEFORE_TOOL_CALL,
    AgentCallbackEvent.AFTER_TOOL_CALL,
    AgentCallbackEvent.AFTER_REACT_ITERATION,
    AgentCallbackEvent.ON_USER_MESSAGE,
})
```

### 3.4 Agent 生命周期

```
创建 → 配置 → 注册工具/铁轨 → invoke/stream → 循环执行 → 返回结果 → 清理
  │        │          │             │            │          │
  │        │          │             │            │          └─ cleanup_task_resources
  │        │          │             │            └─ _inner_loop 迭代
  │        │          │             └─ _inner_invoke
  │        │          └─ register_rail / ability_manager.add
  │        └─ configure(DeepAgentConfig)
  └─ __init__(AgentCard)
```

---

## 四、多轮对话机制

### 4.1 对话循环架构

ReActAgent 的多轮对话基于 **ReAct（Reasoning + Acting）** 范式：

```
用户输入 → 系统提示词构建 → LLM 调用 → 解析响应
    ↑                              │
    │                              ↓
    └──────────── 观察 ←──── 工具执行 ←──── 思考/规划
```

### 4.2 消息管理

**消息注入**（`_admit_user_message`）：

```python
async def _admit_user_message(self, ctx, context, parts, *, source, prefix=""):
    # 1. 触发 ON_USER_MESSAGE 铁轨
    await ctx.fire(AgentCallbackEvent.ON_USER_MESSAGE)
    # 2. 同步 prompt 附件
    await self._sync_prompt_attachments(ctx, context)
    # 3. 拼接消息体
    body = "\n".join(parts)
    # 4. 写入 ModelContext
    await context.add_messages(UserMessage(content=f"{prefix}{body}", metadata={...}))
```

**转向队列（Steering Queue）**：

```python
async def _drain_steering_batch(self, ctx):
    # 从转向队列中取出一批消息
    # 触发 BEFORE_STEERING_DRAIN 铁轨
    # 返回本次要处理的消息列表
```

### 4.3 上下文维护

每次模型调用前，通过 `ContextEngine.get_context_window()` 获取当前上下文窗口：

```python
context_window = await ctx.context.get_context_window(
    system_messages=final_system,
    tools=ctx.inputs.tools,
)
```

上下文窗口包含：
- 系统消息（System Message）
- 对话历史（Context Messages）
- 工具定义（Tools）

### 4.4 中断与恢复

ReActAgent 支持两种中断机制：

1. **工作流中断**（`InterruptionState`）：工作流等待用户输入
2. **工具中断**（`ToolInterruptionState`）：工具执行需要人工确认

```python
async def _handle_resume(self, interruption_state, user_input, ...):
    # 1. 记录用户反馈
    # 2. 检查是否所有中断点都已收集反馈
    # 3. 全部收集 → 并发恢复所有工作流
    # 4. 部分收集 → 继续等待
```

---

## 五、Context 管理

### 5.1 ContextEngine 架构

**文件路径**：`openjiuwen/core/context_engine/context_engine.py`

ContextEngine 是上下文管理的核心入口：

```python
class ContextEngine:
    def __init__(self, config, workspace=None, sys_operation=None):
        self._config = config or ContextEngineConfig()
        self._context_pool: Dict[str, ModelContext] = dict()
        self._window_mutators: List[Callable] = []
```

**核心职责**：
1. 注册/配置消息处理器（ContextProcessor）
2. 创建隔离的 ModelContext（按 session_id + context_id）
3. 应用处理器链（窗口限制、压缩等）

### 5.2 上下文创建

```python
async def create_context(self, context_id, session=None, *, processors=None, ...):
    full_context_id = f"{session_id}_{context_id}"
    if full_context_id in self._context_pool:
        return self._context_pool[full_context_id]  # 缓存命中
    
    # 创建新的 SessionModelContext
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

### 5.3 上下文压缩

**主动压缩**：

```python
async def compress_context(self, context_id, session, *, processor_types=None):
    result = await context.compress_context(
        processor_types=processor_types,
        sys_operation=self._sys_operation,
    )
    # 压缩成功后保存到 session
    if result_code == "compressed":
        await self.save_contexts(effective_session, context_ids=[context_id])
```

**被动恢复**（模型溢出时）：

```python
async def recover_from_model_exception(self, *, context_id, session, exception, ...):
    # 检测是否为上下文溢出错误
    if not self.is_context_overflow_error(exception):
        return False
    # 执行压缩
    result = await self.compress_context(context_id=context_id, session=session)
    return result_code == "compressed"
```

### 5.4 Token 计数

支持多种 tokenizer 后端：

- **本地 tokenizer**：通过 `TokenizerSelector` 选择
- **tiktoken 回退**：`allow_tiktoken_fallback=True`
- **字符串长度回退**：`StringLengthCounter`（最终兜底）

### 5.5 上下文使用分析

`ContextUsageAnalyzer` 提供详细的上下文使用分析：

```python
analyzer = ContextUsageAnalyzer(
    ctx.context.token_counter(),
    model=model_name,
    context_window_limit=ctx.context.context_window_tokens(),
)
snapshot = analyzer.analyze(
    context_window,
    request_id=request_id,
    system_prompt_sections=usage_prompt_sections,
    ...
)
```

输出分类：
- `SYSTEM_PROMPT`：系统提示词占用
- `SKILLS`：技能占用
- `CONTEXT`：对话上下文占用
- `TOOLS`：工具定义占用

---

## 六、记忆系统

### 6.1 LongTermMemory 架构

**文件路径**：`openjiuwen/core/memory/long_term_memory.py`（64KB）

采用 **Singleton** 模式，全局唯一记忆引擎：

```python
class LongTermMemory(metaclass=Singleton):
    def __init__(self):
        # 存储后端
        self.kv_store: BaseKVStore | None = None
        self.vector_store: BaseVectorStore | None = None
        self.db_store: BaseDbStore | None = None
        self.message_store: BaseMessageStore | None = None
        # 记忆索引
        self.memory_index: BaseMemoryIndex | None = None
        # 管理器
        self.message_manager: MessageManager
        self.fragment_memory_manager: FragmentMemoryManager
        self.variable_manager: VariableManager
        self.write_manager: WriteManager
        self.summary_manager: SummaryManager
        self.search_manager: SearchManager
        self.generator: Generator
```

### 6.2 记忆类型

```python
class MemoryType(Enum):
    USER_PROFILE = "user_profile"       # 用户画像
    EPISODIC_MEMORY = "episodic_memory" # 情景记忆
    SEMANTIC_MEMORY = "semantic_memory" # 语义记忆
    VARIABLE = "variable"               # 变量
    SUMMARY = "summary"                 # 摘要
```

### 6.3 存储后端

| 后端 | 用途 | 实现 |
|------|------|------|
| KV Store | 快速结构化数据 | `BaseKVStore` |
| Vector Store | 向量相似度搜索 | `BaseVectorStore` |
| DB Store | 持久化存储 | `BaseDbStore` |
| Message Store | 消息持久化 | `BaseMessageStore` |

### 6.4 记忆操作

**添加记忆**：

```python
async def add_messages(self, messages, agent_config, *, user_id, scope_id, session_id, ...):
    # 1. 获取分布式锁
    lock = DistributedLock(self.kv_store, f"user/{user_id}")
    async with lock:
        # 2. 生成记忆（通过 Generator）
        # 3. 写入各类型存储
        ...
```

**搜索记忆**：

```python
search_params = SearchParams(
    query=query,
    user_id=user_id,
    scope_id=scope_id,
    memory_types=[MemoryType.SEMANTIC_MEMORY, MemoryType.EPISODIC_MEMORY],
    top_k=10,
)
results = await self.search_manager.search(search_params)
```

### 6.5 记忆加密

使用 `AesStorageCodec` 对敏感记忆进行加密存储：

```python
codec = AesStorageCodec(config.crypto_key)
encrypted_config.model_client_cfg.api_key = self._storage_codec.encode(...)
```

---

## 七、工具调用系统

### 7.1 AbilityManager 架构

**文件路径**：`openjiuwen/core/single_agent/ability_manager.py`（1512行）

AbilityManager 是工具/工作流/MCP 的统一注册中心：

```python
class AbilityManager:
    def __init__(self, owner_id=None):
        self._tools: Dict[str, ToolCard] = {}
        self._workflows: Dict[str, WorkflowCard] = {}
        self._agents: Dict[str, AgentCard] = {}
        self._mcp_servers: Dict[str, McpServerConfig] = {}
        self._context_engine = None
        self._progressive_tool_enabled = False
        self._direct_tool_names = {"tool_search"}
```

### 7.2 工具注册

```python
def add(self, ability: Union[Ability, List[Ability]]):
    if isinstance(_ability, ToolCard):
        self._apply_tool_exposure_policy(_ability)
        self._tools[_ability.name] = _ability
        self._mark_registry_changed()
```

**渐进式工具暴露**（Progressive Tool Loading）：

```python
def _apply_tool_exposure_policy(self, card: ToolCard) -> ToolExposure:
    if declared_marker:
        return current  # 显式声明优先
    if name in self._direct_tool_names:
        resolved = ToolExposure.DIRECT  # 直接暴露
    else:
        resolved = ToolExposure.DEFERRED if self._progressive_tool_enabled else ToolExposure.DIRECT
    card.exposure = resolved
```

### 7.3 工具执行

**并行执行**：

```python
async def execute(self, ctx, tool_call, session, parallel_tool_calls=True):
    # 1. 解析工具调用
    tool_calls = self._normalize_tool_calls(tool_call)
    # 2. 创建执行任务
    tasks = [self._invoke_tool(call, session) for call in tool_calls]
    # 3. 并行/串行执行
    if parallel_tool_calls:
        results = await self._execute_parallel_tool_tasks(tool_calls, tasks)
    else:
        results = await asyncio.gather(*tasks, return_exceptions=True)
    return results
```

**资源有序执行**（文件工具）：

```python
@classmethod
async def _execute_resource_ordered_tool_tasks(cls, tool_calls, tasks):
    lanes: Dict[str, List[int]] = {}
    for index, call in enumerate(tool_calls):
        resource_key = cls._tool_execution_resource_key(call)
        lane_key = resource_key or f"independent:{index}"
        lanes.setdefault(lane_key, []).append(index)
    # 不同资源并行，同资源内有序
    lane_results = await asyncio.gather(*(_run_lane(indices) for indices in lanes.values()))
```

### 7.4 工具超时

```python
DEFAULT_TOOL_CALL_TIMEOUT = 300.0  # 默认 5 分钟
MAX_TOOL_CALL_TIMEOUT_HARD_LIMIT = 3600.0  # 硬上限 1 小时

@staticmethod
def _resolve_call_timeout(tool_card):
    # 1. ToolCard.properties["resilience"]["timeout_s"] 优先
    # 2. 缺失 → DEFAULT_TOOL_CALL_TIMEOUT
    # 3. None → 豁免（但仍受硬上限约束）
```

### 7.5 MCP 集成

```python
def set_mcp_tool_allowlist(self, mcp_server, tool_names):
    server_id = str(mcp_server.server_id or "").strip()
    self._mcp_tool_allowlists[server_id] = normalized_names
```

MCP 工具命名规则：`mcp_<server_name>_<tool_name>`

---

## 八、SubAgent 系统

### 8.1 TaskTool 实现

**文件路径**：`openjiuwen/harness/tools/subagent/task_tool.py`

TaskTool 是子 Agent 委派的核心工具：

```python
class TaskTool(Tool):
    def __init__(self, card, parent_agent, language="cn"):
        super().__init__(card)
        self.parent_agent = parent_agent
        self.language = language
```

### 8.2 子 Agent 类型

DeepAgent 内置多种子 Agent 类型：

| 类型 | 用途 |
|------|------|
| `browser_agent` | 浏览器自动化 |
| `code_agent` | 代码执行 |
| `research_agent` | 研究调研 |
| `plan_agent` | 任务规划 |
| `explore_agent` | 探索分析 |

### 8.3 子会话管理

```python
@staticmethod
def _build_sub_session_id(parent_session_id, subagent_type, resume_task_id=""):
    if resume_task_id:
        return resume_task_id  # 恢复已有任务
    if kv_cache_subagent_lifecycle.is_sticky_subagent_type(subagent_type):
        return f"{parent_session_id}_sub_{normalized_type}"  # 确定性 ID
    return f"{parent_session_id}_sub_{normalized_type}_{uuid.uuid4().hex[:8]}"  # 随机 ID
```

### 8.4 子 Agent 执行

```python
async def _run_subagent_with_observable_stream(subagent, inputs, session):
    # 优先使用 stream 路径（获取真实首 token 时间）
    stream = getattr(subagent, "stream", None)
    if not callable(stream):
        return await subagent.invoke(inputs, session=session)
    
    async for chunk in stream(inputs, session=session):
        if chunk_type == "llm_output":
            output_parts.append(content)
        if chunk_type == "answer":
            terminal_result = dict(payload)
    return terminal_result
```

### 8.5 会话工具

**文件路径**：`openjiuwen/harness/tools/subagent/session_tools.py`

提供子 Agent 会话管理工具：
- `subagent_spawn`：创建子 Agent
- `subagent_wait`：等待子 Agent 完成
- `subagent_list`：列出活跃子 Agent
- `subagent_send_input`：向子 Agent 发送输入
- `subagent_close`：关闭子 Agent
- `subagent_resume`：恢复子 Agent

---

## 九、Workflow 引擎

### 9.1 Workflow 架构

**文件路径**：`openjiuwen/core/workflow/workflow.py`

Workflow 是图执行引擎，支持：
- 组件化节点（Start/End/LLMComponent 等）
- 连接边（Connection）
- 输入/输出映射
- 中断/恢复

```python
class Workflow:
    def __init__(self, card: WorkflowCard):
        self.card = card
        self._components: Dict[str, Component] = {}
        self._connections: List[Connection] = []
    
    def add_workflow_comp(self, name, comp, inputs_schema):
        ...
    
    def add_connection(self, from_comp, to_comp):
        ...
```

### 9.2 组件类型

| 组件 | 用途 |
|------|------|
| `Start` | 工作流入口 |
| `End` | 工作流出口 |
| `LLMComponent` | LLM 调用 |
| `WorkflowComponent` | 嵌套工作流 |
| `CodeComponent` | 代码执行 |

### 9.3 执行模式

**同步执行**：`Workflow.invoke(inputs)`

**流式执行**：`async for chunk in Workflow.stream(inputs):`

**中断恢复**：
```python
class WorkflowInterruptEntry:
    tool_call: Any
    component_ids: List[str]
    workflow_execution_state: Any
    collected_input: Any = None
```

### 9.4 输入/输出映射

```python
flow.set_start_comp("start", start, inputs_schema={"query": "${query}"})
flow.add_workflow_comp("llm", llm, inputs_schema={"query": "${start.query}"})
flow.set_end_comp("end", end, inputs_schema={"output": "${llm.output}"})
```

使用 `${}` 语法进行变量引用。

---

## 十、多 Agent 协作

### 10.1 TeamAgent 架构

**文件路径**：`openjiuwen/agent_teams/agent/team_agent.py`（28KB）

TeamAgent 是多 Agent 协作的核心：

```python
class TeamAgent(BaseAgent):
    def __init__(self, card):
        super().__init__(card)
        self.spec: TeamAgentSpec
        self.team_backend: TeamBackend
        self._members: Dict[str, MemberHandle]
```

### 10.2 Spawn 机制

**文件路径**：`openjiuwen/agent_teams/spawn/`

支持两种 spawn 方式：

1. **In-Process Spawn**（`inprocess_spawn.py`）：协程级 spawn
2. **External CLI Spawn**（`external_cli_spawn.py`）：外部 CLI 进程

```python
async def inprocess_spawn(team_agent, ctx, *, initial_message=None, session_id=None, fork_from=None):
    # 1. 创建 TeamAgent 实例
    teammate = _TeamAgent(card)
    # 2. 共享 workspace 缓存
    team_agent.share_workspace_cache_with(teammate)
    # 3. 配置
    teammate.configure(spec, ctx)
    # 4. Fork 上下文注入
    if fork_from:
        await native.create_new_context_engine(session_id, messages=fork_from.to_messages())
    # 5. 创建 asyncio.Task
    task = asyncio.get_running_loop().create_task(_run())
    return InProcessSpawnHandle(process_id=f"inproc-{member_name}", _task=task)
```

### 10.3 Fork 上下文

```python
class ForkContext:
    messages: List[BaseMessage]
    compact_split: Optional[int]
    compact_direction: Optional[str]
    
    def to_messages(self) -> List[BaseMessage]:
        return self.messages
```

Fork 机制允许子 Agent 继承父 Agent 的对话历史，并在分割点进行压缩。

### 10.4 团队工具

**文件路径**：`openjiuwen/agent_teams/tools/`

- `TeamBackend`：团队后端通信
- `spawn`：创建子 Agent
- `send_message`：发送消息
- `checkpoint`：检查点

---

## 十一、安全控制

### 11.1 PermissionEngine 架构

**文件路径**：`openjiuwen/harness/security/permission_engine/core.py`

```python
class PermissionEngine:
    def __init__(self, config, llm=None, model_name=None, workspace_root=None, trusted_dirs=None):
        self.config = prepare_permissions_for_engine(config)
        self._file_guard: FileGuardChecker = build_file_guard_checker(...)
        self._net_guard: NetGuardChecker = build_net_guard_checker(...)
```

### 11.2 权限判定管线

```python
def check_tool_permission_directly(self, tool_name, tool_args, ...):
    result_A = evaluate_tiered_policy(...)      # 工具权限（始终）
    result_B = FileGuardChecker.evaluate(...)   # 路径防护
    result_C = NetGuardChecker.evaluate(...)    # 网络防护
    return strictest(result_A, result_B, result_C)  # 取最严格
```

### 11.3 权限级别

```python
class PermissionLevel(Enum):
    ALLOW = "allow"   # 允许
    ASK = "ask"       # 询问用户
    DENY = "deny"     # 拒绝
```

### 11.4 防护层

| 防护层 | 文件 | 功能 |
|--------|------|------|
| ToolGuard | `toolguard/tool_policy.py` | 命令规则匹配 |
| FileGuard | `fileguard/file_guard.py` | 敏感路径保护 |
| NetGuard | `netguard/net_guard.py` | 网络 URL 过滤 |

### 11.5 内置规则

**文件路径**：`openjiuwen/harness/security/permission_engine/toolguard/builtin_rules.py`

```python
def inline_package_command_rules(cfg):
    # 内嵌包内命令规则
    ...

def load_package_command_rules():
    # 加载包内命令规则
    ...
```

### 11.6 命令规范化

**文件路径**：`openjiuwen/harness/security/permission_engine/toolguard/command_canonicalize.py`

```python
def canonicalize_command(cmd: str) -> str:
    # 解析命令，提取基础命令和参数
    ...
```

### 11.7 Shell AST 分析

**文件路径**：`openjiuwen/harness/security/permission_engine/toolguard/shell_ast.py`

使用 tree-sitter-bash 解析 shell 命令，检测：
- 命令注入
- 管道链式调用
- 反引号/$() 嵌套

---

## 十二、MCP 设计

### 12.1 MCP 集成架构

openJiuwen 通过 `fastmcp` 和 `mcp` 库集成 MCP 协议：

```python
class McpServerConfig:
    server_id: str
    server_url: str
    command: Optional[str]
    args: List[str]
    env: Dict[str, str]
```

### 12.2 MCP 工具注册

```python
# AbilityManager 中
self._mcp_servers: Dict[str, McpServerConfig] = {}
self._mcp_tool_allowlists: Dict[str, frozenset[str]] = {}

def set_mcp_tool_allowlist(self, mcp_server, tool_names):
    server_id = str(mcp_server.server_id or "").strip()
    self._mcp_tool_allowlists[server_id] = normalized_names
```

### 12.3 MCP 工具命名

```python
def mcp_model_tool_name(server_name, tool_name):
    return f"mcp_{server_name}_{tool_name}"

def mcp_model_tool_prefix(server_name):
    return f"mcp_{server_name}_"
```

### 12.4 MCP 工具白名单

支持按服务器设置工具白名单：

```python
# 设置白名单
ability_manager.set_mcp_tool_allowlist(mcp_server, ["tool1", "tool2"])

# 移除白名单（恢复无限制）
ability_manager.set_mcp_tool_allowlist(mcp_server, None)
```

---

## 十三、Skill 系统

### 13.1 Skill 架构

**文件路径**：`openjiuwen/harness/skills/`

```python
# __init__.py
class Skill:
    """技能基类"""
    name: str
    description: str
    prompt_template: str
```

### 13.2 Skill 库状态

**文件路径**：`openjiuwen/harness/skills/library_state.py`

```python
class LibraryState:
    """技能库状态管理"""
    def __init__(self):
        self._skills: Dict[str, Skill] = {}
        self._loaded: bool = False
```

### 13.3 Skill 注册

```python
# BaseAgent 中
async def register_skill(self, skill_path):
    self.lazy_init_skill()
    await self._skill_util.register_skills(skill_path, self)

async def register_remote_skills(self, skills_dir, github_tree, token=""):
    self.lazy_init_skill()
    await self._skill_util.register_remote_skills(skills_dir, github_tree, token=token)
```

### 13.4 Skill 提示词注入

```python
async def _update_skill_prompt_builder_section(self, rendered_system_prompt):
    if self._skill_util is None:
        return
    if not rendered_system_prompt or not self._skill_util.has_skill():
        self.prompt_builder.remove_section(_SKILLS_SECTION)
        return
    self.add_prompt_builder_section(
        _SKILLS_SECTION,
        self._skill_util.get_skill_prompt(),
        priority=_SKILLS_SECTION_PRIORITY,
        category=ContextCategory.SKILLS.value,
    )
```

---

## 十四、RAG 检索

### 14.1 检索架构

**文件路径**：`openjiuwen/core/retrieval/`

```
retrieval/
├── embedding/          # 嵌入模型
├── vector_store/       # 向量存储
├── reranker/           # 重排序
├── retriever/          # 检索器
├── indexing/           # 索引
├── query_rewriter/     # 查询重写
├── knowledge_base.py   # 知识库
├── graph_knowledge_base.py  # 图知识库
└── simple_knowledge_base.py # 简单知识库
```

### 14.2 嵌入模型

```python
class Embedding(ABC):
    @abstractmethod
    async def embed(self, texts: List[str]) -> List[List[float]]:
        ...

class APIEmbedding(Embedding):
    """基于 API 的嵌入模型"""
    ...
```

### 14.3 向量存储

```python
class BaseVectorStore(ABC):
    @abstractmethod
    async def add(self, vectors: List[List[float]], metadatas: List[dict]):
        ...

    @abstractmethod
    async def search(self, query_vector: List[float], top_k: int) -> List[SearchResult]:
        ...
```

### 14.4 重排序

**文件路径**：`openjiuwen/core/retrieval/reranker/`

```python
class BaseReranker(ABC):
    @abstractmethod
    async def rerank(self, query: str, documents: List[str]) -> List[Tuple[str, float]]:
        ...
```

### 14.5 知识库

```python
class KnowledgeBase:
    def __init__(self, vector_store, embedding_model, reranker=None):
        self.vector_store = vector_store
        self.embedding_model = embedding_model
        self.reranker = reranker

    async def add_documents(self, documents: List[str], metadatas: List[dict]):
        vectors = await self.embedding_model.embed(documents)
        await self.vector_store.add(vectors, metadatas)

    async def search(self, query: str, top_k: int = 10):
        query_vector = await self.embedding_model.embed([query])
        results = await self.vector_store.search(query_vector[0], top_k)
        if self.reranker:
            results = await self.reranker.rerank(query, results)
        return results
```

---

## 十五、可观测性

### 15.1 OpenTelemetry 集成

**文件路径**：`openjiuwen/extensions/observability/`

```
observability/
├── callback_handler.py      # 回调处理器（91KB）
├── span_context.py          # Span 上下文（40KB）
├── span_record_processor.py # Span 记录处理器
├── runtime.py               # 运行时
├── semconv.py               # 语义约定
├── trajectory_events.py     # 轨迹事件
├── context_compression_handler.py
├── file_exporter.py         # 文件导出
├── otlp_codec.py            # OTLP 编解码
├── backend_projection.py    # 后端投影
├── demand.py                # 需求
├── config.py                # 配置
└── redaction.py             # 脱敏
```

### 15.2 轨迹事件

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

### 15.3 Span 上下文

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

### 15.4 上下文压缩追踪

```python
class ContextCompressionHandler:
    def on_compression_start(self, context_id, reason):
        ...
    
    def on_compression_end(self, context_id, result):
        ...
```

---

## 十六、Agent 进化

### 16.1 进化架构

**文件路径**：`openjiuwen/agent_evolving/`

```
agent_evolving/
├── agent_rl/              # 在线 RL 训练
│   ├── online/            # 在线学习
│   ├── offline/           # 离线学习
│   └── reward/            # 奖励函数
├── optimizer/             # 优化器
├── trajectory/            # 轨迹收集
├── evaluator/             # 评估器
├── experience/            # 经验回放
├── prompts/               # 提示词进化
├── checkpointing/         # 检查点
├── trainer/               # 训练器
├── updater/               # 更新器
├── sharing/               # 经验分享
├── signal/                # 信号
├── tools/                 # 工具
└── dataset/               # 数据集
```

### 16.2 在线 RL 训练

```python
class OnlineRLService:
    def __init__(self, config):
        self.config = config
        self.trainer = Trainer(config)
        self.experience_buffer = ExperienceBuffer(config.buffer_size)
    
    async def train_step(self, trajectory):
        # 1. 计算奖励
        reward = await self.compute_reward(trajectory)
        # 2. 存储经验
        self.experience_buffer.add(trajectory, reward)
        # 3. 采样批次
        batch = self.experience_buffer.sample(self.config.batch_size)
        # 4. 更新策略
        await self.trainer.update(batch)
```

### 16.3 轨迹收集

```python
class Trajectory:
    states: List[State]
    actions: List[Action]
    rewards: List[float]
    metadata: Dict[str, Any]
    
    def compute_returns(self, gamma=0.99):
        ...
```

### 16.4 评估器

```python
class Evaluator:
    async def evaluate(self, agent, tasks):
        results = []
        for task in tasks:
            result = await agent.invoke(task)
            score = self.score_result(result, task)
            results.append(score)
        return sum(results) / len(results)
```

---

## 十七、CLI 实现

### 17.1 CLI 架构

**文件路径**：`openjiuwen/harness/cli/cli.py`（1114行）

基于 Click 框架：

```python
@click.group(invoke_without_command=True)
@click.version_option(version=__version__, prog_name="openjiuwen")
@click.option("--model", "-m")
@click.option("--provider")
@click.option("--api-key")
@click.option("--api-base")
@click.option("--remote")
@click.option("--verbose", "-v")
@click.option("--workspace", "-w")
def cli(ctx, **kwargs):
    """OpenJiuWen — terminal interactive AI programming assistant."""
    ...
```

### 17.2 命令

| 命令 | 功能 |
|------|------|
| `openjiuwen` | 默认进入交互式 REPL |
| `openjiuwen chat` | 显式进入交互式 REPL |
| `openjiuwen run PROMPT` | 非交互式单轮执行 |
| `openjiuwen auto-harness` | 自动评估 |

### 17.3 REPL 模式

```python
async def _run_chat(opts):
    cfg = load_config(...)
    backend = create_backend(cfg)
    store = SessionStore()
    store.new_session(getattr(backend, "_session_id", "cli"), cfg.model)
    await backend.start()
    await run_repl(backend, cfg, store)
    await backend.stop()
```

### 17.4 单轮模式

```python
async def _run_once(opts, prompt, output_format):
    cfg = load_config(...)
    return await run_once(cfg, prompt, output_format)
```

支持输出格式：`text`、`json`、`stream-json`

### 17.5 交互式引导

```python
def _interactive_setup():
    """首次使用时的 API Key 配置引导"""
    provider = click.prompt("LLM Provider", type=click.Choice(["OpenAI", "DashScope", "SiliconFlow"]))
    api_base = click.prompt("API Base URL", default=default_bases.get(provider))
    model = click.prompt("Model name", default="gpt-4o")
    api_key = click.prompt("API Key", hide_input=True)
    save_settings_json({...})
```

---

## 十八、对 laew 工程的借鉴建议

### 18.1 架构设计借鉴

#### 18.1.1 Card/Config 分离模式

**laew 现状**：AgentProfile 包含名称/系统提示词/工具集

**借鉴建议**：
- 将 Agent 身份（Card）与运行时配置（Config）进一步分离
- Card 定义身份元数据（id/name/description）
- Config 定义运行时行为（max_iterations/model/provider/tools）
- 支持热更新（Hot Reload）无需重启 Agent

#### 18.1.2 铁轨（Rails）机制

**laew 现状**：无钩子机制

**借鉴建议**：
- 引入 `@rail(before, after, on_exception)` 装饰器
- 定义标准事件：BEFORE_MODEL_CALL、AFTER_MODEL_CALL、BEFORE_TOOL_CALL、AFTER_TOOL_CALL
- 支持可插拔的钩子链，便于扩展：
  - 日志记录
  - 权限校验
  - 上下文压缩
  - 异常恢复

#### 18.1.3 能力管理器（AbilityManager）

**laew 现状**：ToolRegistry 简单注册

**借鉴建议**：
- 引入统一的 AbilityManager 管理 Tool/Workflow/MCP/Agent
- 支持渐进式工具暴露（Progressive Tool Loading）
- 支持工具执行超时控制
- 支持资源有序执行（文件工具防冲突）

### 18.2 Context 管理借鉴

#### 18.2.1 上下文引擎

**laew 现状**：简单消息列表

**借鉴建议**：
- 引入 ContextEngine 管理上下文生命周期
- 支持可插拔的 ContextProcessor（压缩/卸载/重排）
- 支持 Token 计数与上下文窗口管理
- 支持上下文使用分析（分类统计）

#### 18.2.2 上下文压缩策略

**借鉴建议**：
- 实现多种压缩策略：
  - 摘要压缩（Summary Compression）
  - 滑动窗口（Sliding Window）
  - 重要性采样（Importance Sampling）
- 支持主动压缩与被动恢复（模型溢出时）

### 18.3 记忆系统借鉴

#### 18.3.1 长期记忆

**laew 现状**：session_memory 表存储摘要

**借鉴建议**：
- 引入多类型记忆：
  - 语义记忆（Semantic Memory）
  - 情景记忆（Episodic Memory）
  - 用户画像（User Profile）
  - 变量（Variable）
- 支持向量检索与混合检索
- 支持记忆加密存储

### 18.4 安全控制借鉴

#### 18.4.1 权限引擎

**laew 现状**：零校验

**借鉴建议**：
- 引入 PermissionEngine 三级防护：
  - ToolGuard：命令规则匹配
  - FileGuard：敏感路径保护
  - NetGuard：网络 URL 过滤
- 支持 allow/ask/deny 三态策略
- 支持内置规则与自定义规则

#### 18.4.2 Shell AST 分析

**借鉴建议**：
- 使用 tree-sitter-bash 解析 shell 命令
- 检测命令注入、管道链式调用
- 支持命令规范化与白名单匹配

### 18.5 多 Agent 协作借鉴

#### 18.5.1 Spawn 机制

**laew 现状**：SubAgent-Work 简单委派

**借鉴建议**：
- 支持 In-Process Spawn（协程级）
- 支持 External CLI Spawn（外部进程）
- 支持 Fork 上下文注入
- 支持确定性子会话 ID（可恢复）

#### 18.5.2 团队Agent

**借鉴建议**：
- 引入 TeamAgent 作为 Team Leader
- 支持角色定义与任务分配
- 支持团队记忆与上下文共享
- 支持检查点与故障恢复

### 18.6 可观测性借鉴

#### 18.6.1 OpenTelemetry 集成

**laew 现状**：无可观测性

**借鉴建议**：
- 集成 OpenTelemetry SDK
- 定义标准 Span 事件
- 支持轨迹收集与导出
- 支持上下文压缩追踪

### 18.7 工具系统借鉴

#### 18.7.1 MCP 集成

**laew 现状**：无 MCP 支持

**借鉴建议**：
- 集成 fastmcp/mcp 库
- 支持 MCP 服务器注册与工具发现
- 支持工具白名单
- 统一 MCP 工具命名规范

#### 18.7.2 工具超时与重试

**借鉴建议**：
- 支持工具级超时配置
- 支持全局硬上限
- 支持幂等性标记
- 支持自动重试与错误恢复

### 18.8 进化系统借鉴

#### 18.8.1 RL 训练

**借鉴建议**：
- 支持在线 RL 训练
- 支持轨迹收集与奖励计算
- 支持经验回放
- 支持 Prompt 自动优化

---

## 十九、总结

### 19.1 工程规模

| 模块 | 代码量 | 核心文件 |
|------|--------|----------|
| core/single_agent | ~15000行 | react_agent.py (3251行)、ability_manager.py (1512行) |
| core/context_engine | ~5000行 | context_engine.py |
| core/memory | ~10000行 | long_term_memory.py (64KB) |
| harness | ~20000行 | deep_agent.py (3999行) |
| agent_teams | ~15000行 | team_agent.py (28KB) |
| extensions/observability | ~10000行 | callback_handler.py (91KB) |
| agent_evolving | ~8000行 | 多个子模块 |

### 19.2 设计亮点

1. **Card/Config 分离**：身份与行为解耦，支持热更新
2. **铁轨机制**：可插拔的钩子链，职责清晰
3. **渐进式工具暴露**：避免工具过多导致上下文溢出
4. **三级安全防护**：ToolGuard + FileGuard + NetGuard
5. **多类型记忆**：语义/情景/画像/变量/摘要
6. **Fork 上下文**：子 Agent 继承父 Agent 对话历史
7. **OpenTelemetry 集成**：全链路可观测
8. **在线 RL 训练**：支持 Agent 自我进化

### 19.3 与 laew 的对比

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

### 19.4 建议优先级

| 优先级 | 借鉴点 | 预期收益 |
|--------|--------|----------|
| P0 | 铁轨机制 | 可扩展性大幅提升 |
| P0 | 权限引擎 | 安全性基础保障 |
| P1 | ContextEngine | 长对话能力 |
| P1 | 多类型记忆 | 跨会话知识积累 |
| P1 | MCP 集成 | 工具生态扩展 |
| P2 | 可观测性 | 调试与优化 |
| P2 | RL 训练 | 自动优化 |
| P2 | TeamAgent | 复杂任务协作 |

---

**调研完成日期**：2026-09-05

**调研人**：Claude Code Agent
