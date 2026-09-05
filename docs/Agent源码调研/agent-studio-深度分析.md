# openJiuwen Studio 深度分析报告

> **分析日期**：2026-09-05
> **分析目标**：`/usr/local/LsmGitOpenSource/agent-studio`
> **前置依据**：`agent-studio-源码调研.md`
>
> 本报告在源码调研基础上，对 10 个核心维度进行**深度层面**分析——聚焦具体代码路径、函数实现、算法细节、数据结构设计，并给出对 laew 工程的 P0/P1/P2 借鉴路线图。

---

## 目录

1. [多微服务架构深度](#1-多微服务架构深度)
2. [执行引擎深度](#2-执行引擎深度)
3. [工作流引擎深度](#3-工作流引擎深度)
4. [DSL 转换深度](#4-dsl-转换深度)
5. [插件系统深度](#5-插件系统深度)
6. [知识库深度](#6-知识库深度)
7. [沙箱服务深度](#7-沙箱服务深度)
8. [评估体系深度](#8-评估体系深度)
9. [提示词工程深度](#9-提示词工程深度)
10. [多模型接入深度](#10-多模型接入深度)
11. [对 laew 的深度借鉴建议](#11-对-laew-的深度借鉴建议)

---

## 1. 多微服务架构深度

### 1.1 服务拓扑与通信协议

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                           frontend (React + TypeScript)                        │
│  ┌────────────────────┐  ┌─────────────────┐  ┌───────────────────────────┐  │
│  │ workflow-canvas    │  │ api-client      │  │ base-ui                  │  │
│  │ (FlowGram.ai 画布)  │  │ (REST SDK)      │  │ (组件库)                  │  │
│  └─────────┬──────────┘  └────────┬────────┘  └───────────────────────────┘  │
└────────────┼──────────────────────┼──────────────────────────────────────────┘
             │ HTTP/REST (JSON)     │ HTTP/REST (JSON)
             ▼                      ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                    backend (FastAPI :8000) — 核心编排层                        │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ Routers (API 层)                                                        │  │
│  │  agents / workflows / plugins / knowledge_base / prompts / evaluation   │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
│                                    │                                         │
│                                    ▼                                         │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ Managers (业务逻辑层)                                                    │  │
│  │  agent.py / workflow.py / plugin.py / knowledge_base.py / memory.py    │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
│                                    │                                         │
│                                    ▼                                         │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ Executors (执行引擎层)                                                    │  │
│  │  agent/agent_runner.py  ──→ AgentRunner (单例 agent_mgr)                │  │
│  │  workflow/workflow_runner.py ──→ WorkflowRunner (单例 flow_mgr)         │  │
│  │  component/component_runner.py ──→ ComponentExecutor                   │  │
│  │  plugin/plugin_mgr.py ──→ PluginManager                                 │  │
│  │  evaluation/evaluation_harness.py ──→ EvaluationHarness                 │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
│                                    │                                         │
│              ┌─────────────────────┼─────────────────────┐                   │
│              ▼                     ▼                     ▼                   │
│  ┌───────────────────┐  ┌───────────────────┐  ┌───────────────────────────┐│
│  │ SQLite / MySQL    │  │ Milvus / Chroma   │  │ Redis (可选)              ││
│  │ (SQLAlchemy ORM)  │  │ (向量检索)         │  │ (checkpoint)             ││
│  └───────────────────┘  └───────────────────┘  └───────────────────────────┘│
└──────────────────────────────────────────────────────────────────────────────┘
             │                              │                           │
             ▼                              ▼                           ▼
┌──────────────────────┐  ┌─────────────────────────┐  ┌────────────────────────┐
│  plugin_server       │  │  sandbox_server         │  │  connect               │
│  (独立 RESTful)      │  │  gateway + sandbox      │  │  MCP Server / Channel  │
│  :8001               │  │  BubbleWrap + Seccomp   │  │  SDK + Adapters        │
└──────────────────────┘  └─────────────────────────┘  └────────────────────────┘
```

### 1.2 服务间通信协议深度

**backend → plugin_server**：HTTP/JSON RESTful 调用。Plugin Server 是独立部署的 FastAPI 应用，backend 通过 HTTP 请求获取插件的 RESTful API 定义。

**backend → sandbox_server**：HTTP/JSON 异步调用（`httpx.AsyncClient`）。

**代码路径**：`sandbox_server/gateway/openjiuwen_sandbox_gateway/app/gateway.py`

```python
async def remote_server(lang, code, inputs, session, timeout=10.0):
    payload = {"session": session, "language": lang, "code": code, "timeout": timeout, "inputs": inputs}
    async with httpx.AsyncClient() as cli:
        r = await cli.post(SANDBOX_SERVER_URL, json=payload)
        return r.json()
```

**backend ↔ connect**：双向 MCP 协议。

- **作为 MCP Server**：`connect/adapters/mcp_server/server.py` 使用 FastMCP 暴露工具，支持 stdio/SSE 传输。
- **作为 MCP Client**：`PluginMcpTool.invoke()` 在 `plugin_tools.py` 内连接外部 MCP Server。

### 1.3 数据一致性机制

**多租户隔离**：所有核心表通过 `space_id` 字段实现数据隔离。

**版本管理一致性**：

- Agent/Workflow/Plugin 均使用 `draft` + 发布版本机制
- 缓存失效：`AgentRunner.clear_agent_cache_for_all_conversations()` 在配置变更时清除所有会话缓存

```python
def clear_agent_cache_for_all_conversations(self, agent_id: str, agent_version: str = "draft") -> int:
    agent_key = f"{agent_id}_{agent_version}"
    cleared_count = 0
    for conversation_id in list(self._agent_instances.keys()):
        if agent_key in self._agent_instances[conversation_id]:
            # 清理工作流资源
            if hasattr(catch_instance, 'agent_config') and hasattr(catch_instance.agent_config, 'workflows'):
                catch_instance.remove_workflows(old_workflows)
            del self._agent_instances[conversation_id][agent_key]
            cleared_count += 1
    return cleared_count
```

**双库分离**：agent 库（业务配置）+ ops 库（提示词/执行日志），通过 `ops/dependencies.py` 管理。

---

## 2. 执行引擎深度

### 2.1 AgentRunner 核心循环

**代码路径**：`core/executor/agent/agent_runner.py` — `AgentRunner.run()`

```python
async def run(self, id, version, inputs, conversation_id, space_id, current_user) -> AsyncGenerator[Any, None]:
    # 1. 参数验证
    if isinstance(inputs, InteractiveInput):
        inputs = {"conversation_id": conversation_id, "query": inputs}
    elif "conversation_id" not in inputs:
        raise BaseError(...)

    # 2. 获取Agent配置 (DL → ReActAgentConfig/WorkflowAgentConfig)
    agent_dl_json = await _fetch_agent_dl(id, version, space_id, current_user)
    agent_config = AgentDlAdapter.convert_to_agent_config(agent_dl_json)

    # 2.1 知识库检索 → 注入 prompt_template
    kb_ids, retrieval_config = AgentDlAdapter.get_knowledge_config(agent_dl_json)
    if kb_ids:
        kb_results = await retrieve_multi_kb(kbs, query, config)
        for result_text in kb_results:
            agent_config.prompt_template.append({"role": "system, "content": result_text})

    # 3. 获取/缓存 Agent 实例 (三维缓存: user_id + agent_id + version)
    invokable_agent = await self.get_agent_instance(...)

    # 4. 组件 id-name 映射表 (递归提取嵌套子工作流)
    mapping = await self._create_mapping_table(agent_config, space_id)

    # 5. 初始化追踪上下文
    trace_context = initialize_trace_context(space_id, id, version, mapping)

    # 6. 流式执行
    async for chunk in Runner.run_agent_streaming(agent=invokable_agent, inputs=inputs, session=conversation_id):
        rsp = await process_chunk_trace(chunk, trace_context)
        if rsp:
            yield rsp
```

**关键设计**：

1. **实例缓存**（`get_agent_instance`）：按 `user_id → agent_key` 两级字典缓存，配置变更时通过 JSON 序列化比较触发重建。
2. **知识库集成**：执行前自动检索，结果作为 system message 注入 prompt_template。
3. **追踪链路**：完整的 trace_context 包含 agent/workflow/component 三层 span。

### 2.2 WorkflowRunner 核心循环

**代码路径**：`core/executor/workflow/workflow_runner.py` — `WorkflowRunner.run()`

```python
async def run(self, id, version, inputs, conversation_id, space_id, current_user):
    # 1. 冲突检测 — 同一 conversation_id 已有执行则取消
    if workflow_execution_manager.is_executing(conversation_id):
        await workflow_execution_manager.cancel_execution(conversation_id)

    # 2. 编译工作流 (含 Pregel 图转换)
    flow = await self.get_compiled_workflow(Context(), id, version, space_id, current_user)

    # 3. 创建 Session
    session = create_workflow_session(session_id=conversation_id)

    # 4. 注册执行 (支持取消)
    workflow_execution_manager.register_execution(registration)

    # 5. 流式执行
    async for chunk in Runner.run_workflow_streaming(workflow, inputs, session, context):
        if workflow_execution_manager.is_cancelled(conversation_id):
            break
        rsp, trace_log, _ = result_convert(chunk, business_type="WORKFLOW", mapping=component_name_map)
        if rsp:
            yield rsp
```

### 2.3 执行管理器（取消/冲突检测）

**代码路径**：`core/executor/workflow/workflow_execution_manager.py`

`WorkflowExecutionManager` 使用线程安全的字典 + `threading.Lock()` 管理执行状态：

```python
class WorkflowExecutionManager:
    def __init__(self):
        self._executions: Dict[str, WorkflowExecutionInfo] = {}
        self._lock = threading.Lock()
        self._cancelled_flags: Dict[str, bool] = {}

    async def cancel_execution(self, conversation_id: str) -> bool:
        # 1. 设置取消标志 (优先)
        with self._lock:
            self._cancelled_flags[conversation_id] = True
        # 2. 取消异步任务
        execution_info.task.cancel()
        await execution_info.task
        # 3. 从注册表移除
        self.unregister_execution(conversation_id)
```

**关键设计**：取消标志 + 异步任务取消双重机制，流式输出可快速响应。

---

## 3. 工作流引擎深度

### 3.1 Pregel 图算法实现

**代码路径**：`core/executor/workflow/pregel_graph_adapter.py` — `PregelGraphAdapter`

#### 3.1.1 图数据结构

使用 NetworkX 的 `MultiDiGraph`（多重有向图）表示工作流：

```python
class PregelGraphAdapter:
    def __init__(self, workflow: BaseFlow) -> None:
        self._workflow: BaseFlow = workflow
        self._graph: nx.MultiDiGraph = nx.MultiDiGraph()
        for component in self._workflow.components:
            self._graph.add_node(component.id, type=component.type)
        for connection in self._workflow.connections:
            self._graph.add_edge(connection.source, connection.target, visited=False, branch_id=connection.branch_id)
```

**MultiDiGraph 选择原因**：支持同一对节点间的多条边（分支场景），这是 `DiGraph` 无法实现的。

#### 3.1.2 转换流水线

```python
def convert(self) -> BaseFlow:
    self._workflow.connections = []
    self._pre_process_graph()    # 1. 预处理：为分支节点插入空子节点
    self._validate_graph()       # 2. 校验：孤立节点、连通性、环检测
    self._travel_all_nodes()     # 3. 拓扑遍历：cba 消减 + 连接重建
    return self._workflow
```

#### 3.1.3 cba（closest branch ancestor）分支消减算法

这是 Pregel 适配的核心算法，处理多分支汇合场景：

```python
# 分支起始节点：为出边增加 cba 信息
def _split_branch(self, node: str) -> None:
    for u, v, d in self._graph.out_edges(node, data=True):
        d['cba'] = node                              # 记录最近分支祖先
        d['total_branches'] = len(self._graph.out_edges(node))  # 总分支数
        d['cur_branches'] = 1                          # 当前分支数

# 节点存在 cba 且出度为1：透传 cba 信息
def _passthrough_branch(self, node: str) -> None:
    if self._graph.nodes[node].get('cba', False):
        for u, v, d in self._graph.out_edges(node, data=True):
            d['cba'] = self._graph.nodes[node]['cba']
            d['total_branches'] = self._graph.nodes[node]['total_branches']
            d['cur_branches'] = self._graph.nodes[node]['cur_branches']

# cba 消减：当同一 cba 的所有分支都汇合时，移除该 cba
def _reduce_cba_map(self, cba_map: Dict[str, Dict[str, Any]]) -> None:
    need_reduce = True
    while need_reduce:
        need_reduce = False
        for k, v in list(cba_map.items()):
            if v['cur_branches'] == v['total_branches']:  # 所有分支已汇合
                cba_map.pop(k)
                d = self._graph.nodes.data()[k]
                PregelGraphAdapter._add_cba_map(cba_map, d)  # 向上传播
                need_reduce = True
```

**算法核心思想**：

1. 每个分支节点记录 `cba`（最近分支祖先）和分支总数。
2. 当所有分支汇合到同一节点时，消减该 cba（分支完成）。
3. 消减后向上层传播，检查更高层分支是否也完成汇合。
4. 最终 `cba_map` 中只剩 1 个祖先时，节点的 `cba` 属性被设置。

#### 3.1.4 依赖计算

```python
def _multiple_dependency(self, node: str) -> None:
    normal_parents: List[str] = []
    branch_parents: Dict[str, List[str]] = {}
    for u, v, d in self._graph.in_edges(node, data=True):
        if d.get('cba', False):
            cba = d.get('cba')
            if self._is_switch_like_component(cba):
                branch_parents[cba].append(u)
            else:
                normal_parents.append(u)
        # 合并祖先-子孙关系
    if len(branch_parents) > 1:
        branch_parents = self._merge_ancestor_descendant_in_branch_parents(branch_parents)
    # 笛卡尔积生成完整依赖
    cartesian_results = PregelGraphAdapter._cartesian_product(list(branch_parents.values()))
    for cartesian_result in cartesian_results:
        self._workflow.connections.append(
            Connection(source=normal_parents + cartesian_result, target=node, branch_id=None))
```

**设计亮点**：通过笛卡尔积处理多分支汇合，确保 Pregel 超步执行时正确等待所有前驱完成。

### 3.2 组件编译机制

**代码路径**：`core/executor/workflow/workflow.py` — `Workflow`

```python
class Workflow:
    # 组件编译器映射表 — 注册表模式
    COMPILER_HANDLERS = {
        ComponentType.COMPONENT_TYPE_LLM: '_compile_llm_component',
        ComponentType.COMPONENT_TYPE_QUESTION: '_compile_question_component',
        ComponentType.COMPONENT_TYPE_INTENT: '_compile_intent_component',
        ComponentType.COMPONENT_TYPE_CODE: '_compile_code_component',
        ComponentType.COMPONENT_TYPE_HTTP_REQUEST: '_compile_http_request_component',
        ComponentType.COMPONENT_TYPE_REACT_AGENT: '_compile_react_agent_component',
        ComponentType.COMPONENT_TYPE_KNOWLEDGE_RETRIEVAL: '_compile_knowledge_retrieval_component',
        # ... 共 11 种标准组件
    }

    # 特殊组件（异步处理）
    SPECIAL_COMPONENT_TYPES = {
        ComponentType.COMPONENT_TYPE_IF,
        ComponentType.COMPONENT_TYPE_SUB_WORKFLOW,
        ComponentType.COMPONENT_TYPE_LOOP,
        ComponentType.COMPONENT_TYPE_PLUGIN,
    }

    async def compile_component(self, context, workflow_dl, comp, loader):
        # 1. 空组件
        if comp.type in self.EMPTY_COMPONENT_TYPES:
            return EmptyComponent()
        # 2. BREAK 组件
        if comp.type == ComponentType.COMPONENT_TYPE_BREAK:
            return LoopBreakComponent()
        # 3. 特殊组件
        if comp.type in self.SPECIAL_COMPONENT_TYPES:
            return await self._compile_special_component(context, workflow_dl, comp, loader)
        # 4. 注册表编译
        handler_name = self.COMPILER_HANDLERS.get(comp.type)
        if handler_name:
            handler = getattr(self, handler_name)
            return await handler(comp, workflow_dl)
```

**编译流水线**：

```python
async def compile(self, context, loader) -> InvokableWorkflow:
    card = WorkflowCard(id=self.id, version=self.version, name=self.name, input_params=self.inputs)
    flow = InvokableWorkflow(card=card)
    flow = await self.process_components(context, flow, self.dl_workflow, loader)  # 编译组件
    flow = await self.process_stream_connections(flow)   # 处理流式连接
    flow = await self.process_connections(flow, self.dl_workflow.connections)  # 处理普通连接
    return flow
```

**设计模式亮点**：

1. **注册表模式**：`COMPILER_HANDLERS` 字典映射组件类型到编译方法，新增组件只需添加映射 + 实现编译方法。
2. **策略模式**：不同组件类型使用不同编译器（LLMCompCompiler / CodeCompCompiler 等）。
3. **模板方法**：`compile()` 定义骨架，具体组件编译由子方法完成。

---

## 4. DSL 转换深度

### 4.1 抽象转换器 + 工厂模式

**代码路径**：`core/dsl_converter/converter/converter.py`

```python
class WorkflowConverter(ABC):
    @abstractmethod
    def convert(self, json_data: Dict[str, Any]) -> WorkflowImportResult:
        pass

class ConverterFactory:
    @staticmethod
    def create(format_type: WorkflowFormat) -> WorkflowConverter:
        if format_type == WorkflowFormat.OPENJIUWEN_NATIVE:
            return NativeWorkflowConverter()
        elif format_type == WorkflowFormat.N8N:
            return N8nWorkflowConverter()
        else:
            raise ValueError(f"Unsupported workflow format: {format_type}")
```

### 4.2 n8n 兼容策略

**代码路径**：`core/dsl_converter/converter/converter_n8n.py` + `n8n_mappings.py`

n8n 转换器通过**节点映射表**实现兼容：

- n8n 节点类型 → openJiuwen 组件类型
- n8n 参数格式 → openJiuwen 配置格式
- 连接关系转换（n8n 的 `main` 输出 → openJiuwen 的分支连接）

### 4.3 工作流代码生成器

**代码路径**：`core/manager/workflow_code_generator.py`

```python
class WorkflowCodeGenerator:
    """将 DSL Workflow 转换为可运行的 Python 脚本"""

    def generate(self) -> str:
        sections = [
            self._gen_header(),               # 文件头注释
            self._gen_imports(),              # 动态 import（根据组件类型按需生成）
            self._gen_workflow_metadata(),    # 元数据常量
            self._gen_model_config_helper(),  # 模型配置辅助函数（API Key 环境变量注入）
            self._gen_all_component_functions(),  # 每个组件一个函数
            self._gen_build_workflow(),       # 组装工作流
            self._gen_main(),                 # 入口函数
        ]
        return "\n".join(sections)
```

**低代码↔代码双向转换**：可视化编排生成 Python 代码，反之亦可。动态 import 根据组件类型按需生成，避免不必要的依赖。

---

## 5. 插件系统深度

### 5.1 三种插件类型统一抽象

**代码路径**：`core/executor/plugin/plugin_tools.py`

```python
class ServiceTool:
    """RESTful API 工具 — 编译为 RestfulApi"""
    def compile(self) -> RestfulApi:
        input_params = convert_params_to_json_schema(self.restfulapischema.params)
        restfulapi_card = RestfulApiCard(name=tool_name, description=..., input_params=input_params,
                                         url=url, method=self.restfulapischema.method,
                                         headers=headers, queries=queries)
        return RestfulApi(restfulapi_card)

class CodeTool:
    """代码插件工具 — 编译为 PluginCodeTool"""
    def compile(self) -> Tool:
        return PluginCodeTool.create(self.codeschema)

class McpTool:
    """MCP 工具 — 编译为 PluginMcpTool"""
    def compile(self) -> Tool:
        return PluginMcpTool.create(self.mcpconfig)
```

### 5.2 5 种 MCP 传输实现

**代码路径**：`core/executor/plugin/plugin_tools.py` — `PluginMcpTool.invoke()`

```python
async def invoke(self, inputs: Input, **kwargs) -> Output:
    # 1. 传输类型映射
    _transport_to_client_type = {
        McpTransport.STDIO: "stdio",
        McpTransport.SSE: "sse",
        McpTransport.STREAMABLE_HTTP: "streamable-http",
        McpTransport.OPENAPI: "openapi",
        McpTransport.PLAYWRIGHT: "playwright",
    }

    # 2. 根据传输类型创建客户端
    if conf.transport == McpTransport.STDIO:
        client = StdioClient(server_config)
    elif conf.transport == McpTransport.SSE:
        client = SseClient(server_config)
    elif conf.transport == McpTransport.OPENAPI:
        client = OpenApiClient(server_config)
    elif conf.transport == McpTransport.PLAYWRIGHT:
        client = PlaywrightClient(server_config)
    else:  # STREAMABLE_HTTP
        client = StreamableHttpClient(server_config)

    # 3. 连接 → 发现工具 → 查找目标 → 调用 → 断开
    await client.connect()
    tool_cards = await client.list_tools()
    target_card = next((c for c in tool_cards if c.name == tool_name), None)
    mcp_tool = MCPTool(mcp_client=client, tool_info=target_card)
    result = await mcp_tool.invoke(arguments)
    await client.disconnect()
```

**5 种传输协议**：

| 传输类型 | 实现类 | 适用场景 |
|----------|--------|----------|
| `STDIO` | `StdioClient` | 本地进程通信 |
| `SSE` | `SseClient` | Server-Sent Events 流式 |
| `STREAMABLE_HTTP` | `StreamableHttpClient` | HTTP 流式传输 |
| `OPENAPI` | `OpenApiClient` | OpenAPI/Swagger 接口 |
| `PLAYWRIGHT` | `PlaywrightClient` | 浏览器自动化 |

### 5.3 插件市场（Marketplace）

**代码路径**：`marketplace/`

预置插件按领域分类：entertainment / productivity / data / ecommerce / finance / developer。支持从 Swagger 自动生成插件。

---

## 6. 知识库深度

### 6.1 RAG 检索集成

**代码路径**：`core/executor/agent/agent_runner.py` — 知识库检索集成

```python
# RetrievalConfig 数据类
@dataclass
class RetrievalConfig:
    topk: int = 5
    use_graph: bool = False      # 图检索
    graph_expansion: bool = False # 图扩展
    agentic: bool = False         # Agentic 检索
    score_threshold: float = 0.0

# 多知识库检索
kb_results = await retrieve_multi_kb(kbs=kb_instances, query=query, config=config)

# 检索结果注入 prompt_template
for result_text in kb_results:
    agent_config.prompt_template.append({"role": "system", "content": result_text})
```

### 6.2 Milvus/Chroma 双轨实现

通过环境变量 `INDEX_MANAGER_TYPE` 切换：

```python
from openjiuwen.core.retrieval.vector_store.milvus_store import MilvusVectorStore
from openjiuwen.core.retrieval.vector_store.chroma_store import ChromaVectorStore
from openjiuwen.core.retrieval.indexing.indexer.milvus_indexer import MilvusIndexer
from openjiuwen.core.retrieval.indexing.indexer.chroma_indexer import ChromaIndexer
```

### 6.3 文档处理流水线

```
文档上传 → 解析（AutoFileParser） → 分块（TextChunker） → 向量化（OpenAIEmbedding） → 索引
```

**文档状态机**：`UPLOADED → PARSING → CHUNKING → INDEXING → INDEXED`

**图检索（GraphRAG）支持**：三元组提取（TripleExtractor）构建知识图谱，支持图扩展检索。

---

## 7. 沙箱服务深度

### 7.1 BubbleWrap + Seccomp BPF 实现

**代码路径**：`sandbox_server/sandbox/openjiuwen_sandbox_server/app/bwrap.py`

```python
class BubbleWrapRunner(BaseSandbox, sandbox_type='bubblewrap'):
    @staticmethod
    def pre_init(sandbox_config):
        arch = platform.machine()
        # 1. 网络防护
        if not sandbox_config.allow_internal_network_access:
            apply_internal_network_guard(BWRAP_RUN_USER)
        # 2. Seccomp BPF 过滤
        allowed = sandbox_config.seccomp['allow'].get(arch, [])
        bpf = pyseccomp.SyscallFilter(pyseccomp.KILL)  # 默认 KILL 策略
        for syscall in allowed:
            bpf.add_rule(pyseccomp.ALLOW, syscall)
        sandbox_config.seccomp_bpf = bpf

    def run(self, raw_code, base_code, lang, timeout=0, dep_name=None):
        with tempfile.TemporaryDirectory(prefix='bwrap_workdir_', dir='/tmp') as workdir:
            # 1. 应用 seccomp BPF
            seccomp_fd = self._apply_seccomp(workdir, dst_code_dir, lang)
            # 2. 构建 bwrap 命令
            cmd = self._sandbox_command()
            cmd += self._mount_params(workdir, dst_code_dir, dep_paths)
            cmd += self._namespace_params()
            # 3. 执行
            result = self._execute_process(cmd, envs, effective_timeout, pass_fds)
            # retcode=159 表示 Bad syscall
            if result.retcode == 159:
                result = ExecutionResult(result.retcode, result.stdout, result.stderr + '\nBad syscall detected.')
            return result
```

### 7.2 命名空间隔离

```python
NAMESPACE_FLAGS = {
    'user': '--unshare-user',
    'ipc': '--unshare-ipc',
    'pid': '--unshare-pid',
    'net': '--unshare-net',
    'uts': '--unshare-uts',
    'cgroup': '--unshare-cgroup',
}

def _namespace_params(self):
    return [flag for ns, flag in NAMESPACE_FLAGS.items()
            if self._sandbox_config.namespace.get(ns, False)]
```

### 7.3 文件系统隔离

```python
MOUNT_MODES = {
    'read': '--ro-bind',    # 只读挂载
    'write': '--bind',      # 读写挂载
    'dev': '--dev-bind',    # 设备挂载
}
```

### 7.4 多层安全机制

| 层级 | 机制 | 实现 |
|------|------|------|
| 命名空间 | user/ipc/pid/net/uts/cgroup | `bwrap --unshare-*` |
| 系统调用 | Seccomp BPF 过滤 | `pycomp.SyscallFilter(KILL)` |
| 文件系统 | 只读挂载 + 临时目录 | `--ro-bind` + `TemporaryDirectory` |
| 网络 | 默认禁用外部网络 | `apply_internal_network_guard` |
| 用户隔离 | 非 root 用户运行 | `setpriv --reuid sandbox-exec` |
| 超时控制 | 执行超时自动 kill | `_execute_process(timeout)` |

### 7.5 Python 代码的 Seccomp 注入

**代码路径**：`bwrap.py` — `_build_py_seccomp_loader()`

```python
def _build_py_seccomp_loader(bpf_path):
    return f'''
import struct, ctypes, json
def _load_seccomp(bpf_file):
    PR_SET_NO_NEW_PRIVS = 38
    PR_SET_SECCOMP = 22
    SECCOMP_MODE_FILTER = 2
    # ... ctypes 加载 BPF 字节码
    libc.prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)
    libc.prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, ctypes.byref(prog))
_load_seccomp("{bpf_path}")
'''
```

**巧妙设计**：Python 代码通过内联的 ctypes 代码在运行时加载 BPF 过滤器，无需外部依赖。

---

## 8. 评估体系深度

### 8.1 多 Trial 评估架构

**代码路径**：`core/executor/evaluation/evaluation_harness.py`

```python
class EvaluationHarness:
    async def execute_evaluation(self, config: EvaluationRunConfig) -> None:
        # 1. 加载评估任务
        tasks = evaluation_task_repository.list_by_evaluation(config.evaluation_id)

        # 2. 执行每个任务
        for task in tasks:
            await self._execute_task(_make_task_cfg(task))

        # 3. 计算聚合指标
        metrics = compute_aggregate_metrics(results, custom_metric_defs)

        # 4. 回归检测
        alerts = self._detect_regressions(metrics, prev_metrics, prev_run_id)

        # 5. 更新状态
        evaluation_run_repository.update_status(run_id, COMPLETED, metrics)
```

### 8.2 Trial 执行矩阵

```
评估运行（Run）
  ├── 任务 1（Task）
  │   ├── Trial 1（nominal）           — 正常执行
  │   ├── Trial 2（prompt_perturbed）  — 提示词改写
  │   ├── Trial 3（env_perturbed）     — 环境扰动
  │   └── Trial 4（fault_injected）    — 故障注入
  ├── 任务 2（Task）
  └── ...
```

```python
async def _execute_task(self, config: _TaskRunConfig) -> None:
    perturbation_types = ["nominal"]
    if config.enable_perturbations:
        perturbation_types.extend(["prompt_perturbed", "env_perturbed", "fault_injected"])

    for perturbation_type in perturbation_types:
        for trial_num in range(1, trials + 1):
            await self._execute_trial(trial_cfg)
```

### 8.3 扰动注入算法

**代码路径**：`core/executor/evaluation/perturbations.py`

#### 8.3.1 提示词扰动（PromptPerturber）

```python
async def paraphrase(self, prompt: str, num_variants: int = 3) -> List[str]:
    # 优先使用 LLM 改写
    if self.model_id and self.space_id:
        return await self._llm_paraphrase(prompt, num_variants)
    # 回退到规则改写
    return self._rule_based_paraphrase(prompt, num_variants)
```

**LLM 策略**：调用外部 LLM 生成语义等价改写。
**规则策略**：同义词替换 + 句子重排序 + 主动/被动转换。

#### 8.3.2 环境扰动（EnvironmentPerturber）

```python
def perturb_input(self, input_data: Dict[str, Any]) -> Dict[str, Any]:
    perturbations = [
        self._reorder_fields,      # JSON 字段重排序
        self._rename_fields,       # snake_case ↔ camelCase
        self._change_date_formats, # 日期格式转换
        self._add_optional_fields, # 添加可选字段
    ]
    # 随机应用 1-2 种扰动
    selected = random.sample(perturbations, random.randint(1, 2))
    for perturb_fn in selected:
        data = perturb_fn(data)
    return data
```

#### 8.3.3 故障注入（FaultInjector）

```python
def generate_fault(self) -> Dict[str, Any]:
    fault_types = ['timeout', 'error', 'malformed', 'slow']
    fault_type = random.choice(fault_types)
    if fault_type == 'timeout':
        return {'type': 'timeout', 'message': 'Request timeout after 30 seconds', 'code': 'TIMEOUT'}
    elif fault_type == 'error':
        code = random.choice([500, 502, 503, 504])
        return {'type': 'error', 'message': f'HTTP {code}: Internal Server Error', 'code': code}
    elif fault_type == 'malformed':
        return {'type': 'malformed', 'data': '{"incomplete": "json"'}
    else:  # slow
        return {'type': 'slow', 'delay_ms': random.randint(5000, 15000)}
```

### 8.4 评分引擎（GraderEngine）

**代码路径**：`core/executor/evaluation/grader_engine.py`

```python
class GraderEngine:
    async def run_graders(self, graders_cfg, execution_trace, expected, space_id):
        for grader_cfg in graders_cfg:
            grader_type = int(grader_cfg.get("grader_type", GraderType.DETERMINISTIC))
            if grader_type == GraderType.DETERMINISTIC:
                result = self._run_deterministic(grader_cfg, execution_trace, expected)
            elif grader_type == GraderType.MODEL_BASED:
                result = await self._run_model_based(grader_cfg, execution_trace, expected, space_id)
            elif grader_type == GraderType.CODE_BASED:
                result = self._run_code_based(grader_cfg, execution_trace, expected)
```

**三种评分器**：

| 类型 | 实现 | 说明 |
|------|------|------|
| `DETERMINISTIC` | `_check_output` / `_check_state` / `_check_tool_calls` / `_check_pattern_regex` / `_check_transcript` | 确定性检查（精确匹配、正则、路径检查） |
| `MODEL_BASED` | `_run_model_based` | LLM-as-Judge，调用外部模型评分 |
| `CODE_BASED` | `_run_code_based` | 执行自定义 Python 代码评分 |

**权重感知聚合**：

```python
# weight=0 的评分器仅信息性，不参与 pass/fail
active = [r for r in grader_results if r.get("weight", 1.0) > 0]
passed = all(r.get("passed", False) for r in active)
score = sum(r.get("score", 0.0) * r.get("weight", 1.0) for r in active) / total_weight
```

### 8.5 回归检测算法

**代码路径**：`evaluation_harness.py` — `_detect_regressions()`

```python
@staticmethod
def _detect_regressions(current, previous, prev_run_id) -> List[Dict[str, Any]]:
    alerts = []
    # 成功率下降 >10pp → high severity
    if curr_sr is not None and prev_sr is not None:
        delta = curr_sr - prev_sr
        if delta < -0.10:
            alerts.append({"type": "regression", "metric": "success_rate", "severity": "high", ...})
    # 延迟增加 >500ms → medium severity
    if curr_lat is not None and prev_lat is not None and prev_lat > 0:
        delta = curr_lat - prev_lat
        if delta > 500:
            alerts.append({"type": "regression", "metric": "avg_latency_ms", "severity": "medium", ...})
    # 分数下降 >15pp → high severity
    if curr_score is not None and prev_score is not None:
        delta = curr_score - prev_score
        if delta < -0.15:
            alerts.append({"type": "regression", "metric": "avg_score", "severity": "high", ...})
    return alerts
```

---

## 9. 提示词工程深度

### 9.1 DDD 分层设计

**代码路径**：`ops/modules/prompt/`

```
ops/modules/prompt/
├── application/            # 应用层
│   ├── service.py          # PromptService（核心服务）
│   ├── debug_service.py    # 调试服务
│   ├── trace_sdk_interface.py  # 追踪 SDK 接口
│   └── exception.py        # 异常定义
├── domain/                 # 领域层
│   ├── entities.py         # 实体定义（CreatePromptRequest 等）
│   ├── services.py         # 领域服务（DraftDomainService / CommitDomainService / BatchPromptDomainService）
│   ├── repositories.py     # 仓储接口
│   └── debug_entity.py     # 调试实体
└── infra/                  # 基础设施层
    ├── database.py         # 数据库
    └── repositories/       # 仓储实现
        ├── orm_repo.py     # ORM 模型（PromptBasicModel / PromptCommitModel / PromptUserDraftModel）
        ├── job_repo.py     # 任务仓储
        └── agent_repo.py    # Agent 仓储
```

### 9.2 草稿+提交版本管理

```python
class PromptService:
    def __init__(self, prompt_repo, prompt_user_draft_repo, prompt_commit_repo, agent_repo):
        self.draft_domain_service = DraftDomainService(prompt_user_draft_repo, prompt_commit_repo)
        self.commit_domain_service = CommitDomainService(prompt_user_draft_repo, prompt_commit_repo, agent_repo)
        self.batch_prompt_domain_service = BatchPromptDomainService(prompt_repo)
        self.get_prompt_detail_service = GetPromptDetailService(prompt_repo, agent_repo)
```

**ORM 模型**：

```python
class PromptBasicModel:
    prompt_key: str          # 唯一标识
    name: str                # 名称
    description: str         # 描述
    space_id: str            # 工作空间
    created_by: str          # 创建者
    updated_by: str          # 更新者
    deleted_at: datetime     # 删除时间（逻辑删除）

class PromptCommitModel:
    """提交版本 — 每次提交创建一个版本"""

class PromptUserDraftModel:
    """用户草稿 — 编辑中的草稿"""
```

### 9.3 批量操作与同步更新

```python
def update_prompt(self, new_prompt: UpdatePromptRequest) -> UpdatePromptResponse:
    ori_prompt.name = new_prompt.prompt_name
    ori_prompt.description = new_prompt.prompt_description
    self.prompt_repo.update(ori_prompt)

    # 同步更新 agent 中关联提示词模版名称
    update_field_dict = {"prompt_name": new_prompt.prompt_name}
    self.agent_repo.update_field_by_prompt_id(new_prompt.prompt_id, update_field_dict, orm_repo.AgentModel)
```

**设计亮点**：提示词更新自动同步到关联 Agent，保持一致性。

---

## 10. 多模型接入深度

### 10.1 LLM 管理器架构

**代码路径**：`ops/modules/llm/llm_manager.py`

```python
# 客户端缓存 — LRU 策略
@lru_cache(maxsize=32)
def _create_client(base_url: str, api_key: str) -> OpenAI:
    return OpenAI(api_key=api_key, base_url=base_url)

@lru_cache(maxsize=32)
def _create_async_client(base_url: str, api_key: str) -> AsyncOpenAI:
    return AsyncOpenAI(api_key=api_key, base_url=base_url)

# 提供商映射
_CLIENT_PROVIDER_MAP = {
    "siliconflow": "SiliconFlow",
    "openai": "OpenAI",
    "azure": "Azure",
    "ollama": "Ollama",
}

# 统一客户端创建
def get_llm_client(model_id: str, source="config") -> Model:
    cfg = _config_service.get_llm_model_info(model_id, source)
    protocol = cfg.get("protocol_config", "")
    model_client_config = ModelClientConfig(
        client_provider=compatible_provider(protocol.get("provider")),
        api_key=protocol.get("api_key", ""),
        api_base=protocol.get("base_url", ""),
        timeout=protocol.get("timeout", 60),
        verify_ssl=os.getenv("LLM_SSL_VERIFY", "true") == "false",
    )
    return Model(model_client_config, ModelRequestConfig())
```

### 10.2 API Key 加密存储

**代码路径**：`models/model_config.py` + `SecurityUtils`

```python
class ModelConfig(Base):
    __tablename__ = "model_config"
    id: int
    name: str                # 配置名称
    provider: str            # 提供商
    model_type: str          # 模型类型
    base_url: str            # API 地址
    api_key: str             # 加密存储 (SecurityUtils.encrypt_api_key)
    timeout: int
    is_active: bool
    space_id: str            # 多租户
```

### 10.3 调用参数构建

```python
def build_call_kwargs(params: ModelCallParams, cfg: Dict[str, Any]) -> Dict[str, Any]:
    # 清理 messages，只保留支持的字段
    cleaned_messages = []
    for msg in params.messages:
        cleaned_msg = {"role": msg.get("role"), "content": msg.get("content")}
        if "tool_calls" in msg:
            cleaned_msg["tool_calls"] = msg["tool_calls"]
        if "tool_call_id" in msg:
            cleaned_msg["tool_call_id"] = msg["tool_call_id"]
        cleaned_messages.append(cleaned_msg)

    # 优先级：前端显式值 > 配置文件默认值
    call_kwargs = {
        "messages": cleaned_messages,
        "temperature": params.temperature if params.temperature is not None else _default("temperature", float, 1.0),
        "top_p": params.top_p if params.top_p is not None else _default("top_p", float, 1.0),
        "max_tokens": params.max_tokens if params.max_tokens is not None else _default("max_tokens", int, 2048),
    }
    if params.tools:
        call_kwargs["tools"] = tools
    return call_kwargs
```

**多提供商支持**：OpenAI / Azure / Ollama / SiliconFlow 等，通过 `compatible_provider()` 适配协议差异。

---

## 11. 对 laew 的深度借鉴建议

### P0（核心能力 — 必须实现）

#### P0-1: 组件注册机制

**借鉴点**：`Workflow.COMPILER_HANDLERS` 注册表模式

**laew 现状**：3 个工具（Bash/Read/Write），硬编码在 registry 中。

**建议实现**：

```rust
// src/agent/tools/registry.rs
type CompilerFn = fn(&ToolConfig) -> Box<dyn Tool>;

pub struct ToolRegistry {
    handlers: HashMap<String, CompilerFn>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool_type: &str, handler: CompilerFn) {
        self.handlers.insert(tool_type.to_string(), handler);
    }

    pub fn compile(&self, config: &ToolConfig) -> Result<Box<dyn Tool>> {
        match self.handlers.get(&config.tool_type) {
            Some(handler) => Ok(handler(config)),
            None => Err(AgentError::UnsupportedTool(config.tool_type.clone())),
        }
    }
}
```

**收益**：新增工具只需实现 `Tool` trait + 一行注册代码，无需修改核心循环。

#### P0-2: 知识库集成 + RAG 工具

**借鉴点**：`AgentRunner.run()` 中的知识库检索集成

**laew 现状**：无知识库能力。

**建议实现**：

1. 新增 `KnowledgeBaseTool`，封装向量检索
2. Yolo 分类为需要知识的任务时，自动检索相关知识
3. 检索结果作为 system message 注入 prompt（与 agent-studio 相同模式）

```rust
pub struct KnowledgeBaseTool {
    vector_store: Arc<dyn VectorStore>,  // Milvus/Chroma 抽象
    embedding: Arc<dyn EmbeddingModel>,
}

#[async_trait]
impl Tool for KnowledgeBaseTool {
    async fn invoke(&self, input: KnowledgeBaseInput) -> Result<ToolOutput> {
        let embedding = self.embedding.embed(&input.query).await?;
        let results = self.vector_store.search(&embedding, input.top_k).await?;
        Ok(ToolOutput::KnowledgeBase { chunks: results })
    }
}
```

#### P0-3: MCP Client 支持

**借鉴点**：`PluginMcpTool` 的 5 种传输实现

**laew 现状**：无 MCP 支持。

**建议实现**：

1. 实现 `McpClient` trait（stdio / SSE / StreamableHTTP 三选一起步）
2. 封装 `McpTool`，将 MCP 工具调用适配为 laew 的 Tool trait
3. 支持从 MCP Server 动态发现工具

```rust
#[async_trait]
pub trait McpClient: Send + Sync {
    async fn connect(&mut self) -> Result<()>;
    async fn list_tools(&self) -> Result<Vec<McpToolCard>>;
    async fn call_tool(&self, name: &str, args: Value) -> Result<Value>;
    async fn disconnect(&self);
}
```

### P1（增强能力 — 重要）

#### P1-1: 评估体系 — LLM-as-Judge

**借鉴点**：`GraderEngine` 的 Model-Based Grader

**laew 现状**：Quality-Check Agent 使用固定规则检查。

**建议实现**：

1. 引入 `JudgeGrader`，调用 LLM 对执行结果评分
2. 支持确定性检查（精确匹配、正则）+ 模型检查双模式
3. 权重感知聚合评分

```rust
pub enum GraderType {
    Deterministic(DeterministicConfig),
    ModelBased(ModelBasedConfig),  // LLM-as-Judge
    CodeBased(String),             // 自定义 Rust 脚本（可选）
}

pub trait Grader {
    async fn grade(&self, trace: &ExecutionTrace, expected: &Value) -> Result<GradeResult>;
}
```

#### P1-2: 沙箱安全 — 代码执行隔离

**借鉴点**：`BubbleWrapRunner` 的多层隔离机制

**laew 现状**：零沙箱，Bash 工具直接执行。

**建议实现**（分阶段）：

1. **P1a**: 命令黑名单 + 用户确认（低成本快速实现）
2. **P1b**: 命名空间隔离（Linux namespaces）
3. **P1c**: Seccomp BPF 系统调用过滤

```rust
pub struct BashTool {
    sandbox: Option<Box<dyn Sandbox>>,  // 可选沙箱
    allowlist: PathAllowlist,           // 路径白名单
    blocklist: CommandBlocklist,        // 命令黑名单
}

impl BashTool {
    pub async fn invoke(&self, input: BashInput) -> Result<ToolOutput> {
        // 1. 命令解析 & 黑名单检查
        // 2. 路径白名单检查
        // 3. 用户确认（敏感操作）
        // 4. 沙箱执行（可选）
        // 5. 结果返回
    }
}
```

#### P1-3: 提示词版本管理

**借鉴点**：`PromptService` 的草稿+提交版本机制

**laew 现状**：系统提示词硬编码在 `profile.rs`。

**建议实现**：

1. 提示词模板存储在 SQLite `prompts` 表
2. 支持草稿编辑 + 提交发布
3. 支持版本回滚

```rust
// prompts 表
CREATE TABLE prompts (
    id TEXT PRIMARY KEY,
    key TEXT UNIQUE NOT NULL,
    content TEXT NOT NULL,
    version TEXT DEFAULT 'draft',
    created_by TEXT,
    created_at INTEGER,
    updated_at INTEGER
);
```

### P2（高级能力 — 可选）

#### P2-1: Workflow 编排引擎

**借鉴点**：`PregelGraphAdapter` 的 cba 算法 + `Workflow.compile()` 组件编译

**laew 现状**：单一 Agent 循环，无工作流编排。

**建议实现**：

1. DAG 执行引擎（使用 `petgraph` crate）
2. 条件分支、循环、子工作流
3. 组件注册表 + 编译器模式

```rust
pub struct WorkflowEngine {
    graph: DiGraph<ComponentNode, Connection>,
    compilers: HashMap<ComponentType, Box<dyn ComponentCompiler>>,
}

impl WorkflowEngine {
    pub async fn compile(&self, workflow: &WorkflowDefinition) -> Result<CompiledWorkflow> {
        // 1. 图校验（环检测、连通性）
        // 2. 拓扑排序
        // 3. 组件编译
        // 4. 连接建立
    }

    pub async fn execute(&self, inputs: Value) -> impl Stream<Chunk> {
        // Pregel 超步执行
    }
}
```

#### P2-2: 多渠道接入 — MCP Server 模式

**借鉴点**：`connect/adapters/mcp_server/server.py` 的 FastMCP 实现

**建议实现**：

1. 将 laew 的核心能力（Bash/Read/Write 等）暴露为 MCP Server
2. 支持 stdio/SSE 传输
3. 允许 Claude Desktop 等客户端调用

#### P2-3: 记忆系统 — 分层记忆 + 自动压缩

**借鉴点**：`MemoryEngineManager` 的记忆分层

**建议实现**：

1. 会话记忆（短期）→ 用户记忆（长期）→ 全局记忆
2. 基于相关性的记忆检索（向量检索）
3. 长期记忆自动摘要压缩

#### P2-4: 调度器 — 定时任务

**借鉴点**：`core/scheduler/scheduler.py` 的 APScheduler 实现

**建议实现**：

1. 支持 Cron 表达式调度
2. 数据库持久化（SQLite）
3. Webhook 触发

---

## 附录：核心类与函数深度索引

| 模块 | 核心类/函数 | 代码路径 | 关键设计 |
|------|-------------|----------|----------|
| Agent 执行 | `AgentRunner.run` | `core/executor/agent/agent_runner.py` | 三维缓存 + KB 注入 + 流式追踪 |
| Agent 缓存 | `AgentRunner.get_agent_instance` | 同上 | JSON 序列化比较触发重建 |
| Agent 映射 | `AgentRunner._create_mapping_table` | 同上 | 递归提取嵌套子工作流组件名 |
| Workflow 执行 | `WorkflowRunner.run` | `core/executor/workflow/workflow_runner.py` | 冲突检测 + 取消机制 |
| Pregel 适配 | `PregelGraphAdapter.convert` | `core/executor/workflow/pregel_graph_adapter.py` | cba 分支消减算法 |
| cba 消减 | `PregelGraphAdapter._reduce_cba_map` | 同上 | 向上传播的分支汇合算法 |
| 依赖计算 | `PregelGraphAdapter._multiple_dependency` | 同上 | 笛卡尔积生成完整依赖 |
| Workflow 编译 | `Workflow.compile` | `core/executor/workflow/workflow.py` | 注册表 + 流式连接处理 |
| 组件编译 | `Workflow.compile_component` | 同上 | 注册表模式（11 种标准组件） |
| 执行管理 | `WorkflowExecutionManager.cancel_execution` | `workflow_execution_manager.py` | 标志 + 任务取消双重机制 |
| 评估编排 | `EvaluationHarness.execute_evaluation` | `evaluation_harness.py` | 多 Trial + 扰动 + 回归检测 |
| 评分引擎 | `GraderEngine.run_graders` | `grader_engine.py` | 三种评分器 + 权重聚合 |
| 扰动注入 | `PerturbationCoordinator` | `perturbations.py` | Prompt/Env/Fault 三类扰动 |
| 回归检测 | `EvaluationHarness._detect_regressions` | `evaluation_harness.py` | 成功率/延迟/分数阈值检测 |
| 插件工具 | `PluginMcpTool.invoke` | `plugin_tools.py` | 5 种 MCP 传输统一抽象 |
| 插件编译 | `ServiceTool/CodeTool/McpTool.compile` | 同上 | 三种插件类型编译策略 |
| 沙箱执行 | `BubbleWrapRunner.run` | `sandbox_server/sandbox/.../bwrap.py` | 命名空间 + Seccomp + 文件隔离 |
| Seccomp 注入 | `_build_py_seccomp_loader` | 同上 | Python 内联 ctypes 加载 BPF |
| DSL 转换 | `ConverterFactory.create` | `dsl_converter/converter/converter.py` | 抽象基类 + 工厂模式 |
| LLM 管理 | `get_llm_client` | `ops/modules/llm/llm_manager.py` | LRU 缓存 + 多提供商适配 |
| 提示词服务 | `PromptService.create_prompt` | `ops/modules/prompt/application/service.py` | DDD 分层 + 草稿/提交版本 |
| MCP Server | `main` / `register_all` | `connect/adapters/mcp_server/server.py` | FastMCP + stdio/SSE 传输 |

---

> **总结**：openJiuwen Studio 在**组件注册机制、Pregel 图算法、多层安全沙箱、多 Trial 评估、5 种 MCP 传输**等方面有成熟的深度实现。对 laew 而言，**P0 优先级**应放在组件注册表、知识库集成、MCP Client 支持；**P1** 放在 LLM-as-Judge、沙箱安全、提示词版本管理；**P2** 放在 Workflow 编排、多渠道接入、记忆系统、调度器。
