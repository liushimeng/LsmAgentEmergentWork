# Agent Studio 综合深度分析

> 调研对象:agent-studio(Python,一站式 Agent 开发平台)
> 调研日期:2026-09-05
> 原始文档:3 份
> 总行数:~3800 行(合并后)

---

## 目录

1. [项目元信息](#1-项目元信息)
2. [多微服务架构](#2-多微服务架构)
3. [Pregel 图算法 cba 分支消减](#3-pregel-图算法-cba-分支消减)
4. [DSL 双向转换](#4-dsl-双向转换)
5. [5 种 MCP 传输](#5-5-种-mcp-传输)
6. [BubbleWrap 沙箱 + Seccomp BPF](#6-bubblewrap-沙箱--seccomp-bpf)
7. [多 Trial 评估](#7-多-trial-评估)
8. [对 laew 的借鉴](#8-对-laew-的借鉴)
- [附录:核心类与函数深度索引](#附录核心类与函数深度索引)

---

## 1. 项目元信息

**工程定位**:openJiuwen Studio(九文 Agent Studio)是华为技术团队开源的**一站式 AI Agent 开发平台**,提供从开发到部署的全栈解决方案。采用低代码/零代码可视化设计与编排工具,支持开发者快速打造和调试智能体和工作流。

**技术栈**:Python(FastAPI) + React + SQLAlchemy + SQLite/MySQL + Milvus/Chroma + MCP + APScheduler

**代码规模**:后端 ~20 万行 Python,前端 React + TypeScript,多微服务架构

**顶层目录结构**:
```
agent-studio/
├── backend/                    # 主后端服务(FastAPI)
│   ├── openjiuwen_studio/      # 核心业务代码
│   ├── tests/                  # 后端测试
│   └── upgrade/                # 数据库迁移脚本(Alembic)
├── frontend/                   # React 前端
├── connect/                    # 多渠道连接层(adapters/channels + mcp_server)
├── plugin_server/              # 独立插件服务(:8001)
├── sandbox_server/             # 代码沙箱服务(gateway + sandbox)
├── docker/                     # Docker 部署配置
├── helm/                       # Helm Chart(K8s 部署)
└── scripts/                    # 辅助脚本
```

**核心入口与生命周期**(`backend/openjiuwen_studio/main.py`):
```python
@asynccontextmanager
async def lifespan_func(app: FastAPI):
    # 1. 创建数据库表(Base.metadata.create_all)
    # 2. 初始化记忆引擎(MemoryEngineManager.init)
    # 3. 检查 Alembic 版本
    # 4. 初始化 Runner(支持 Redis checkpoint)
    # 5. 启动触发器调度器(APScheduler)
    yield
    # 关闭:停止调度器
```

**配置管理**(`backend/openjiuwen_studio/core/config.py`):环境变量驱动,`DB_TYPE`(sqlite/mysql)、`INDEX_MANAGER_TYPE`(milvus/chroma)、`INDEX_MANAGER_TYPE`(milvus/chroma)多轨切换。

**分层架构总览**:
```
┌─────────────────────────────────────────────────────────┐
│                    API 层(routers/)                      │
│  agents / workflows / plugins / knowledge_base / ...    │
├─────────────────────────────────────────────────────────┤
│                 业务管理层(core/manager/)                │
│  agent.py / workflow.py / plugin.py / memory.py / ...   │
├─────────────────────────────────────────────────────────┤
│                 执行引擎层(core/executor/)               │
│  agent/ / workflow/ / plugin/ / evaluation/ / component/│
├─────────────────────────────────────────────────────────┤
│              核心服务层(core/ 公共模块)                   │
│  common/ / database/ / utils/ / config.py               │
├─────────────────────────────────────────────────────────┤
│                  数据持久层(models/ + SQLite/MySQL)      │
└─────────────────────────────────────────────────────────┘
```

**多租户隔离**:所有核心表通过 `space_id` 字段实现数据隔离;**版本管理一致性**:Agent/Workflow/Plugin 均使用 `draft` + 发布版本机制;**双库分离**:agent 库(业务配置)+ ops 库(提示词/执行日志),通过 `ops/dependencies.py` 管理。

---

## 2. 多微服务架构

### 2.1 服务拓扑与通信协议

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
│  Routers → Managers → Executors 三级调用链                                     │
│  SQLite/MySQL · Milvus/Chroma · Redis(可选)                                  │
└──────────────────────────────────────────────────────────────────────────────┘
             │                              │                           │
             ▼                              ▼                           ▼
┌──────────────────────┐  ┌─────────────────────────┐  ┌────────────────────────┐
│  plugin_server       │  │  sandbox_server         │  │  connect               │
│  (独立 RESTful)      │  │  gateway + sandbox      │  │  MCP Server / Channel  │
│  :8001               │  │  BubbleWrap + Seccomp   │  │  SDK + Adapters        │
└──────────────────────┘  └─────────────────────────┘  └────────────────────────┘
```

### 2.2 服务间通信

- **backend → plugin_server**:HTTP/JSON RESTful 调用。
- **backend → sandbox_server**:HTTP/JSON 异步调用(`httpx.AsyncClient`)。
- **backend ↔ connect**:双向 MCP 协议。既可作为 MCP Server 暴露工具(`connect/adapters/mcp_server/server.py` 使用 FastMCP,支持 stdio/SSE);也可作为 MCP Client 调用外部工具(`PluginMcpTool.invoke()`)。

### 2.3 AgentRunner 全局单例

```python
# 全局Agent管理器实例
plugin_manager = PluginManager()
agent_mgr = AgentRunner(WorkflowRunner(plugin_manager), plugin_manager)
```

进程级单例,注入式依赖(`flow_mgr` + `plugin_mgr`)。

### 2.4 数据库与存储

**存储抽象**:通过 `DB_TYPE` 环境变量切换 SQLite/MySQL;`INDEX_MANAGER_TYPE` 切换 Milvus/Chroma。

**核心表**(均含 `space_id` 多租户字段):
- `agent`:agent_id/agent_version/space_id/prompt_template(JSON)/plugins/workflows/knowledge/memory/configs
- `workflow`:workflow_id/schema(JSON)/input_parameters/output_parameters
- `plugin`:plugin_id/plugin_type/auth(加密)/inputs
- `knowledge_base`:kb_id/embedding_model_config_id/index_manager_type
- `trigger`:trigger_id/cron_expression/agent_id/workflow_id/is_enabled

**Alembic 迁移**:`upgrade/` 下按 mysql/sqlite 分目录,agent + ops 双库独立版本管理。

---

## 3. Pregel 图算法 cba 分支消减

### 3.1 图数据结构

**代码路径**:`core/executor/workflow/pregel_graph_adapter.py`(361 行)

使用 NetworkX 的 `MultiDiGraph`(多重有向图)表示工作流——支持同一对节点间的多条边(分支场景),这是 `DiGraph` 无法实现的。

```python
class PregelGraphAdapter():
    def __init__(self, workflow: BaseFlow) -> None:
        self._workflow: BaseFlow = workflow
        self._graph: nx.MultiDiGraph = nx.MultiDiGraph()
        self._pending_nodes: List[str] = []
        for component in self._workflow.components:
            self._graph.add_node(component.id, type=component.type)
        for connection in self._workflow.connections:
            self._graph.add_edge(connection.source, connection.target,
                                 visited=False, branch_id=connection.branch_id)
```

### 3.2 convert() 转换流水线

```python
def convert(self) -> BaseFlow:
    self._workflow.connections = []
    self._pre_process_graph()    # 1. 预处理:为分支节点插入空子节点
    self._validate_graph()       # 2. 校验:连通性 + 环检测
    self._travel_all_nodes()     # 3. 拓扑遍历:cba 消减 + 连接重建
    self._dfx()
    return self._workflow
```

### 3.3 cba(closest branch ancestor)分支消减算法

**核心思想**:
1. 每个分支节点记录 `cba`(最近分支祖先)+ `total_branches`(分支总数)+ `cur_branches`(已汇合数)
2. 当所有分支汇合到同一节点时,消减该 cba(分支完成)
3. 消减后向上层传播,检查更高层分支是否也完成汇合
4. 最终 `cba_map` 中只剩 1 个祖先时,节点的 `cba` 属性被设置

**代码实现**:
```python
# 分支起始节点:为出边增加 cba 信息
def _split_branch(self, node: str) -> None:
    for u, v, d in self._graph.out_edges(node, data=True):
        d['cba'] = node
        d['total_branches'] = len(self._graph.out_edges(node))
        d['cur_branches'] = 1

# 节点存在 cba 且出度为1:透传 cba 信息
def _passthrough_branch(self, node: str) -> None:
    if self._graph.nodes[node].get('cba', False):
        for u, v, d in self._graph.out_edges(node, data=True):
            d['cba'] = self._graph.nodes[node]['cba']
            d['total_branches'] = self._graph.nodes[node]['total_branches']
            d['cur_branches'] = self._graph.nodes[node]['cur_branches']

# cba 消减:当同一 cba 的所有分支都汇合时,移除该 cba
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

### 3.4 依赖计算(笛卡尔积)

```python
def _multiple_dependency(self, node: str) -> None:
    normal_parents: List[str] = []
    branch_parents: Dict[str, List[str]] = {}
    for u, v, d in self._graph.in_edges(node, data=True):
        if d.get('cba', False):
            cba = d.get('cba')
            if self._is_switch_like_component(cba):
                if branch_parents.get(cba):
                    branch_parents[cba].append(u)
                else:
                    branch_parents[cba] = [u]
            else:
                normal_parents.append(u)

    if len(branch_parents) > 1:
        branch_parents = self._merge_ancestor_descendant_in_branch_parents(branch_parents)

    # 笛卡尔积生成完整依赖
    cartesian_results = PregelGraphAdapter._cartesian_product(list(branch_parents.values()))
    for cartesian_result in cartesian_results:
        self._workflow.connections.append(
            Connection(source=normal_parents + cartesian_result, target=node, branch_id=None))
```

**设计亮点**:通过**笛卡尔积**处理多分支汇合,确保 Pregel 超步执行时正确等待所有前驱完成。

### 3.5 _validate_graph 三层校验

```python
def _validate_graph(self) -> None:
    self._validate_isolated_source_nodes()    # 孤立起始节点必须是 START
    self._validate_connectivity()              # 连通性 start → end
    cycles: List[List[str]] = list(nx.simple_cycles(self._graph))
    if cycles:
        raise JiuWenGraphException(code=StatusCode.WORKFLOW_GRAPH_CIRCLE_ERROR.code, ...)
```

1. **孤立起始节点校验**:所有入度=0 的节点必须是 START
2. **连通性校验**:`nx.has_path(start, end)`
3. **环检测**:`nx.simple_cycles()`

### 3.6 WorkflowExecutionManager 冲突检测 + 取消

**代码路径**:`core/executor/workflow/workflow_execution_manager.py`(164 行)

```python
class WorkflowExecutionManager:
    def __init__(self):
        self._executions: Dict[str, WorkflowExecutionInfo] = {}
        self._lock = threading.Lock()
        self._cancelled_flags: Dict[str, bool] = {}

    async def cancel_execution(self, conversation_id: str) -> bool:
        # 1. 设置取消标志(优先执行,让流式输出能快速响应)
        with self._lock:
            self._cancelled_flags[conversation_id] = True
        # 2. 取消异步任务
        execution_info.task.cancel()
        await execution_info.task
        # 3. 从注册表中移除
        self.unregister_execution(conversation_id)
```

**标志 + 任务双重取消** + 线程安全(`threading.Lock`)+ 快速响应(标志先设置,流式输出在循环检测时立即退出)。

---

## 4. DSL 双向转换

### 4.1 抽象转换器 + 工厂模式

**代码路径**:
- `core/dsl_converter/converter/converter.py`(73 行)
- `core/dsl_converter/converter/converter_native.py`(449 行)
- `core/manager/workflow_code_generator.py`(600+ 行)

```python
@dataclass
class WorkflowImportResult:
    workflow_data: Dict[str, Any]
    warnings: List[str] = field(default_factory=list)
    metadata: Dict[str, Any] = field(default_factory=dict)

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
```

**设计模式**:抽象基类(`WorkflowConverter`)+ 工厂方法(`ConverterFactory.create`)+ 延迟导入(函数体内 `from ... import ...` 避免循环依赖)。

### 4.2 低代码→代码:8 步流水线

```python
def convert(self, json_data: Dict[str, Any]) -> WorkflowImportResult:
    # Step 0: 格式归一化(顶层 nodes/edges → schema 字段)
    # Step 1: schema 字段归一化(object → JSON string)
    # Step 2: 补默认值(workflow_id/space_id/时间戳)
    # Step 3: Pydantic 校验(WorkflowBase.model_validate)
    # Step 4: 重新生成 workflow_id(UUID,避免冲突)
    # Step 5: 重新生成画布节点 ID(regenerate_canvas_ids)
    # Step 6: 更新时间戳
    # Step 7: 清空版本字段(workflow_version/latest_publish_version)
    return WorkflowImportResult(...)
```

**设计亮点**:
- **节点 ID 重新生成**(`regenerate_canvas_ids`):避免与目标空间已有节点冲突
- **Space 清空**:`space_id` 总是从源清空,由 importer 用目标空间填充
- **schema 双向格式**:接受 string/object/顶层 nodes-edges 三种格式

### 4.3 节点 ID 重生成算法

```python
NODE_TYPE_PREFIX_MAP = {
    "1": "start", "2": "end", "3": "llm", "4": "condition",
    "5": "code", "6": "knowledge", "11": "loop", "14": "subworkflow",
    # ... 20 种类型
}

def regenerate_canvas_ids(self, schema):
    id_mapping = {}
    for node in schema.get("nodes", []):
        old_id = node.get("id")
        prefix = NODE_TYPE_PREFIX_MAP.get(str(node.get("type", "node")), f"node{node_type}")
        new_id = f"{prefix}_{uuid.uuid4().hex[:8]}"
        id_mapping[old_id] = new_id
        node["id"] = new_id
    for edge in schema.get("edges", []):
        edge["sourceNodeID"] = id_mapping.get(source, source)
        edge["targetNodeID"] = id_mapping.get(target, target)
    self._update_node_references(schema.get("nodes", []), id_mapping)
```

**为何 ID 前缀映射重要**:引擎通过前缀识别节点类型(如 `start_` 标识 START 节点),不是用 `type` 字段。

### 4.4 代码→低代码(反向):7 段式代码生成

```python
class WorkflowCodeGenerator:
    def generate(self) -> str:
        sections = [
            self._gen_header(),              # 文件头注释
            self._gen_imports(),             # 动态 import(按需)
            self._gen_workflow_metadata(),    # 元数据常量
            self._gen_model_config_helper(), # 模型配置辅助函数(API Key 从环境变量读)
            self._gen_all_component_functions(),  # 每个组件一个函数
            self._gen_build_workflow(),      # 组装工作流
            self._gen_main(),                # 入口函数
        ]
        return "\n".join(sections)
```

**关键技巧**:API Key 通过环境变量注入(`os.getenv(api_key_env_var, '')`),避免硬编码敏感信息。

### 4.5 n8n 兼容策略

通过**节点映射表**(`n8n_mappings.py`)实现兼容:
- n8n 节点类型 → openJiuwen 组件类型
- n8n 参数格式 → openJiuwen 配置格式
- 连接关系转换(n8n 的 `main` 输出 → openJiuwen 的分支连接)

---

## 5. 5 种 MCP 传输

### 5.1 三种插件类型统一抽象

**代码路径**:`core/executor/plugin/plugin_tools.py`(383 行)

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

**统一抽象**:三种插件类型的 `compile()` 都返回 `Tool`,符合 LSP(里氏替换)。

### 5.2 5 种 MCP 传输实现

| 传输类型 | 实现类 | 适用场景 |
|----------|--------|----------|
| `STDIO` | `StdioClient` | 本地进程通信(如 `.py` 自动包装为 `sys.executable`) |
| `SSE` | `SseClient` | Server-Sent Events 流式 |
| `STREAMABLE_HTTP` | `StreamableHttpClient` | HTTP 流式传输 |
| `OPENAPI` | `OpenApiClient` | OpenAPI/Swagger 接口 |
| `PLAYWRIGHT` | `PlaywrightClient` | 浏览器自动化 |

**核心 invoke**:
```python
async def invoke(self, inputs: Input, **kwargs) -> Output:
    # 1. 传输类型映射表
    _transport_to_client_type = {
        McpTransport.STDIO: "stdio",
        McpTransport.SSE: "sse",
        McpTransport.STREAMABLE_HTTP: "streamable-http",
        McpTransport.OPENAPI: "openapi",
        McpTransport.PLAYWRIGHT: "playwright",
    }

    # 2. stdio 场景:.py 自动用 sys.executable 包装
    if conf.transport == McpTransport.STDIO:
        cmd = mcp_params.get("command") or conf.url or ""
        if cmd.endswith(".py"):
            mcp_params["command"] = sys.executable
            mcp_params["args"] = [cmd] + extra_args

    # 3. 按传输类型创建客户端
    if conf.transport == McpTransport.STDIO:
        client = StdioClient(server_config)
    elif conf.transport == McpTransport.SSE:
        client = SseClient(server_config)
    # ...

    # 4. 连接 → 发现工具 → 查找目标 → 调用 → finally 断开
    connected = await client.connect()
    try:
        tool_cards = await client.list_tools()
        target_card = next((c for c in tool_cards if c.name == tool_name), None)
        mcp_tool = MCPTool(mcp_client=client, tool_info=target_card)
        result = await mcp_tool.invoke(arguments)
    finally:
        await client.disconnect()
```

**统一抽象亮点**:
1. **`McpServerConfig`** 统一参数:`server_name`、`server_path`、`client_type`、`params`、`auth_headers`
2. **stdio `.py` 自动包装**:`if cmd.endswith(".py")` 自动用 `sys.executable` 包装
3. **`finally: disconnect()`**:哪怕调用失败也断开连接,避免僵尸进程

### 5.3 流式契约委托

```python
async def stream(self, inputs: Input, **kwargs):
    """MCP 不是流式源,但基类要求实现 stream()。
    委托给 invoke(),单 chunk 提交,让工作流的流式管道正常工作。"""
    result = await self.invoke(inputs, **kwargs)
    yield result
```

### 5.4 Marketplace

预置插件按领域分类:entertainment / productivity / data / ecommerce / finance / developer。支持从 Swagger 自动生成插件。

---

## 6. BubbleWrap 沙箱 + Seccomp BPF

### 6.1 抽象基类 + 自动注册

**代码路径**:`sandbox_server/sandbox/openjiuwen_sandbox_server/app/base.py`(70 行)

```python
class BaseSandbox(ABC):
    _registry: dict[str, type['BaseSandbox']] = {}

    def __init_subclass__(cls, sandbox_type: str | None = None, **kwargs):
        super().__init_subclass__(**kwargs)
        if sandbox_type is not None:
            BaseSandbox._registry[sandbox_type] = cls

    @classmethod
    def get_class(cls, sandbox_type: str) -> type['BaseSandbox']:
        if sandbox_type not in cls._registry:
            raise ValueError(f"Unknown sandbox type: '{sandbox_type}'")
        return cls._registry[sandbox_type]

    @abstractmethod
    def run(self, raw_code, base_code, lang, timeout=0, dep_name=None) -> ExecutionResult:

    @staticmethod
    def _execute_process(cmd, envs, timeout, pass_fds=()):
        """统一超时控制:超时自动 kill"""
        process = subprocess.Popen(cmd, ...)
        stdout, stderr = process.communicate(timeout=timeout)
        return ExecutionResult(process.returncode, stdout, stderr)
```

**自动注册机制**(`__init_subclass__`):新增沙箱实现只需 `class MyRunner(BaseSandbox, sandbox_type='xxx')`,`_registry` 自动填充。

### 6.2 BubbleWrap 命名空间隔离

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

**6 种命名空间**:user / ipc / pid / net / uts / cgroup。

### 6.3 文件系统只读挂载

```python
MOUNT_MODES = {
    'read': '--ro-bind',     # 只读挂载
    'write': '--bind',       # 读写挂载
    'dev': '--dev-bind',     # 设备挂载
}

def _mount_params(self, workdir, dst_code_dir, extra_paths):
    params = []
    for mount in self._sandbox_config.mount:
        flag = MOUNT_MODES.get(mount['mode'])
        params += [flag, mount['src'], mount['dst']]
    params += ['--ro-bind', workdir, dst_code_dir]  # 工作目录强制只读
    for path in extra_paths:
        params += ['--ro-bind', path, path]  # 依赖路径只读
```

### 6.4 Seccomp BPF 加载

```python
@staticmethod
def pre_init(sandbox_config):
    arch = platform.machine()
    if not sandbox_config.allow_internal_network_access:
        apply_internal_network_guard(BWRAP_RUN_USER)

    allowed = sandbox_config.seccomp['allow'].get(arch, [])
    bpf = pyseccomp.SyscallFilter(pyseccomp.KILL)  # 默认 KILL 策略
    for syscall in allowed:
        bpf.add_rule(pyseccomp.ALLOW, syscall)
    sandbox_config.seccomp_bpf = bpf
```

**KILL 策略 + ALLOW 白名单**:默认 KILL 所有系统调用,逐个添加白名单。

### 6.5 Python 内联 Seccomp 加载器(代码生成作为运行时机制)

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
del _load_seccomp
'''
```

**巧妙设计**:Python 代码通过**内联的 ctypes 代码**在运行时加载 BPF 过滤器,无需外部依赖。

### 6.6 主流程 9 步执行

```python
def run(self, raw_code, base_code, lang, timeout=0, dep_name=None):
    with tempfile.TemporaryDirectory(prefix='bwrap_workdir_', dir='/tmp') as workdir:
        seccomp_fd = self._apply_seccomp(workdir, dst_code_dir, lang)
        if lang == 'python' and self._sandbox_config.seccomp_bpf:
            base_code = _build_py_seccomp_loader(dst_bpf) + '\n' + base_code  # Python 拼接内联加载器

        cmd = self._sandbox_command()
        cmd += self._mount_params(workdir, dst_code_dir, dep_paths)
        cmd += self._namespace_params()
        cmd += ['--seccomp', str(seccomp_fd)]
        cmd += generate_eval_command(lang, None, base_code, raw_code)

        effective_timeout = max(timeout, int(self._sandbox_config.timeout))
        result = self._execute_process(cmd, envs, effective_timeout, pass_fds)

        # retcode=159 表示 Bad syscall(来自 seccomp KILL)
        if result.retcode == 159:
            result = ExecutionResult(result.retcode, result.stdout,
                                    result.stderr + '\nBad syscall detected.')
```

### 6.7 多层安全机制总览

| 层级 | 机制 | 实现 |
|------|------|------|
| 命名空间 | user/ipc/pid/net/uts/cgroup | `bwrap --unshare-*` |
| 系统调用 | Seccomp BPF 过滤 | `pyseccomp.SyscallFilter(KILL)` |
| 文件系统 | 只读挂载 + 临时目录 | `--ro-bind` + `TemporaryDirectory` |
| 网络 | 默认禁用外部网络 | `apply_internal_network_guard` |
| 用户隔离 | 非 root 用户运行 | `setpriv --reuid sandbox-exec` |
| 超时控制 | 执行超时自动 kill | `_execute_process(timeout)` |

### 6.8 retcode=159 含义

Linux 上 `SECCOMP_RET_KILL` 默认终止信号导致退出码为 `128 + 9 = 137`,但 BPF KILL 模式下子进程被 SIGSYS 终止,bwrap 的 `--seccomp` 标志传递 fd 时,seccomp 违规会让进程以 159 退出。

---

## 7. 多 Trial 评估

### 7.1 Trial × 4 扰动矩阵

**代码路径**:
- `core/executor/evaluation/evaluation_harness.py`(709 行)
- `core/executor/evaluation/grader_engine.py`(453 行)
- `core/executor/evaluation/perturbations.py`(395 行)
- `core/executor/evaluation/metrics.py`(521 行)

**评估运行 → 任务 → Trial × 4 扰动 = 4×N 次执行**:
```
评估运行(Run)
  ├── 任务 1(Task)
  │   ├── Trial 1(nominal)           — 正常执行
  │   ├── Trial 2(prompt_perturbed)  — 提示词改写
  │   ├── Trial 3(env_perturbed)     — 环境扰动
  │   └── Trial 4(fault_injected)    — 故障注入
  ├── 任务 2(Task)
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

### 7.2 单 Trial 执行

**关键设计**:
- **unique conversation_id**:`eval_{run_id}_{task_id}_t{trial_num}_{perturbation[:4]}_{trace_id[:8]}`,避免 trial 之间冲突
- **扰动应用时机**:prompt/env 在执行前;fault 在执行后(注入到 final_output)
- **权重感知聚合**:weight=0 的评分器仅信息性,不参与 pass/fail 和 score
- **异常即失败**:trial 抛异常 → passed=False, score=0.0

### 7.3 三种评分器

| 类型 | 实现 | 说明 |
|------|------|------|
| `DETERMINISTIC` | `_check_output` / `_check_state` / `_check_tool_calls` / `_check_pattern_regex` / `_check_transcript` | 确定性检查(精确匹配、正则、路径检查) |
| `MODEL_BASED` | `_run_model_based` | LLM-as-Judge,调用外部模型评分 |
| `CODE_BASED` | `_run_code_based` | 执行自定义 Python 代码评分 |

**确定性评分器 5 种 check_type**:output_check / state_check / tool_call_check / pattern_check / transcript_check

**`_compare()` 通用比较器 9 种比较条件**:eq/ne/gt/lt/ge/le/contains/not_contains/regex/is_not_empty

### 7.4 LLM-as-Judge 评分

```python
async def _run_model_based(self, cfg, trace, expected, space_id):
    # 1. 构建评分提示词(rubric + assertions + actual + expected)
    prompt = self._build_grading_prompt(trace, expected, inner)
    # 2. 调用 LLM
    result = await model.invoke([UserMessage(content=prompt)])
    # 3. JSON 解析
    parsed = self._parse_llm_response(response_text)
    return {"grader_name": ..., "passed": parsed.get("passed"), "score": parsed.get("score")}
```

**容错**:解析失败 → passed=False, score=0.0, 保留原 feedback 前 500 字符。

### 7.5 代码评分器

```python
@staticmethod
def _run_code_based(cfg, trace, expected):
    code = inner.get("code", "")
    fn_name = inner.get("function_name", "grade")
    namespace = {}
    exec(compile(code, "<grader>", "exec"), namespace)
    grade_fn = namespace.get(fn_name)
    result = grade_fn(trace, expected)
```

**`exec()` 沙箱**:用 Python 内置 `exec(compile(...))` 执行用户代码,函数签名 `def grade(trace, expected) -> Dict[str, Any]`。

### 7.6 扰动算法

#### 提示词扰动(LLM 优先,规则回退)
```python
async def paraphrase(self, prompt, num_variants=3):
    if self.model_id and self.space_id:
        return await self._llm_paraphrase(prompt, num_variants)
    return self._rule_based_paraphrase(prompt, num_variants)
```
规则三策略:同义词替换 + 句子重排序 + 主动/被动转换。

#### 环境扰动(4 种)
字段重排 / 命名约定转换(snake_case ↔ camelCase) / 日期格式 / 添加可选字段。每次随机应用 1-2 种。

#### 故障注入(4 种)
timeout / error(500,502,503,504) / malformed(故意坏 JSON) / slow(5-15s 延迟)

### 7.7 指标计算:Pass@k / Pass^k

```python
def compute_pass_at_k(results, k_values=None):
    """pass@k = 1 - C(n-c, k) / C(n, k)"""
    for passes in task_results.values():
        n = len(passes); c = sum(passes)
        prob = 1.0 - _comb(n - c, k) / _comb(n, k)
        task_probs.append(prob)

def compute_pass_pow_k(results, k_values=None):
    """pass^k = C(c, k) / C(n, k)"""
    prob = _comb(c, k) / _comb(n, k)
```

**10+ 种指标**:success_rate / passed / task_pass_rate / tasks_fully_passed_rate / tasks_never_passed_rate / avg_score / score_std / latency stats (min/max/median/p75/p95/std/cv) / pass_at_k / pass_pow_k / token_usage / perfect_score_rate / score_distribution / flakiness / per_grader_breakdown / tokens_efficiency

### 7.8 回归检测

```python
@staticmethod
def _detect_regressions(current, previous, prev_run_id):
    # 成功率下降 >10pp → high severity
    # 延迟增加 >500ms → medium severity
    # 分数下降 >15pp → high severity
```

### 7.9 组件注册表

**代码路径**:`core/executor/workflow/workflow.py`(478 行)

**3 套注册表**:
- **COMPILER_HANDLERS**:11 种标准组件 → 编译方法名
- **SPECIAL_COMPONENT_TYPES**:4 种特殊组件(IF/SUB_WORKFLOW/LOOP/PLUGIN)
- **EMPTY_COMPONENT_TYPES**:4 种空组件

**11 种标准组件**:LLM / QUESTION / INTENT / INPUT / OUTPUT / TEXT_EDITOR / VARIABLE_MERGE / CODE / HTTP_REQUEST / REACT_AGENT / KNOWLEDGE_RETRIEVAL

**4 步编译流水线**:
```python
async def compile_component(self, context, workflow_dl, comp, loader):
    if comp.type in EMPTY_COMPONENT_TYPES:
        return EmptyComponent()
    if comp.type == COMPONENT_TYPE_BREAK:
        return LoopBreakComponent()
    if comp.type in SPECIAL_COMPONENT_TYPES:
        return await self._compile_special_component(...)
    handler_name = self.COMPILER_HANDLERS.get(comp.type)
    handler = getattr(self, handler_name)
    return await handler(comp, workflow_dl)
```

**3 步主编译**:
```python
async def compile(self, context, loader):
    flow = await self.process_components(context, flow, self.dl_workflow, loader)
    flow = await self.process_stream_connections(flow)
    flow = await self.process_connections(flow, self.dl_workflow.connections)
```

---

## 8. 对 laew 的借鉴

### 8.1 P0(核心能力 — 必须实现)

#### P0-1: 工具注册表 + JSON 序列化缓存

借鉴 `AgentRunner.get_agent_instance()` 三维缓存 + `COMPILER_HANDLERS` 注册表模式。

```rust
pub struct ToolRegistry {
    compilers: HashMap<String, Box<dyn ToolCompiler>>,
    instances: HashMap<(String, String), (String, Arc<dyn Tool>)>,
}

impl ToolRegistry {
    pub fn compile_or_get_cached(&mut self, user_id, tool_key, config) -> Result<Arc<dyn Tool>> {
        let config_json = serde_json::to_string(config)?;
        // JSON 序列化比较触发重建(Pydantic v2 model_dump_json 的 Rust 等价)
        if let Some((cached_json, instance)) = self.instances.get(&key) {
            if cached_json == &config_json { return Ok(instance.clone()); }
        }
        let compiler = self.compilers.get(tool_key).ok_or(...)?;
        let instance = Arc::from(compiler(config)?);
        self.instances.insert(key, (config_json, instance.clone()));
        Ok(instance)
    }
}
```

#### P0-2: 知识库集成(多 KB + RAG 注入 prompt)

借鉴 `AgentRunner.run()` 的 KB 检索前移模式:
```rust
async fn preprocess_with_kb(&self, query: &str, kb_ids: &[String]) -> Vec<String> {
    // YoloRunner 前置检索,注入 system message(与 <<<LAEW:SESSION_HISTORY>>> 同模式)
}
```

#### P0-3: MCP Client(5 种传输)
```rust
#[async_trait]
pub trait McpClient: Send + Sync {
    async fn connect(&mut self) -> Result<()>;
    async fn list_tools(&self) -> Result<Vec<McpToolCard>>;
    async fn call_tool(&self, name: &str, args: Value) -> Result<Value>;
    async fn disconnect(&self);  // Rust Drop trait 自动断开
}
```

#### P0-4: 冲突检测 + 取消

```rust
// SessionState 加 is_running 标志 + tokio::sync::Mutex
// 新输入到达时先 cancel 旧 task(WorkflowExecutionManager 模式)
```

### 8.2 P1(增强能力 — 重要)

#### P1-1: 评估体系 — 三种评分器
```rust
pub enum GraderType {
    Deterministic(DeterministicConfig),  // 9 种比较条件
    ModelBased(ModelBasedConfig),        // LLM-as-Judge
    CodeBased(String),                   // 用户脚本
}
pub trait Grader: Send + Sync {
    async fn grade(&self, trace: &ExecutionTrace, expected: &Value) -> Result<GradeResult>;
}
```

#### P1-2: 沙箱安全 — BubbleWrap + Seccomp

分阶段实现:
1. **P1a**: 命令黑名单 + 用户确认(低成本快速实现)
2. **P1b**: 命名空间隔离(Linux namespaces,用 `nix` crate)
3. **P1c**: Seccomp BPF 系统调用过滤(用 `libseccomp-sys` 或 `seccompiler` crate)

```rust
pub struct BubbleWrapSandbox { config: SandboxConfig }
impl Sandbox for BubbleWrapSandbox {
    async fn run(&self, code: &str, lang: Lang, timeout_ms: u64) -> Result<ExecutionResult> {
        // 1. 临时目录
        // 2. 加载 seccomp BPF
        // 3. 拼接 bwrap 命令(namespace + mount + seccomp + cmd)
        // 4. 执行(带 timeout kill)
        // 5. 检测 retcode=159 → "Bad syscall detected."
    }
}
```

#### P1-3: 提示词版本管理(草稿+提交)

```rust
// SQLite 表
// prompts (id, key UNIQUE, content, version='draft', created_by, created_at, updated_at)
// prompt_commits (prompt_id, version, content_snapshot, commit_msg, created_at)
```

### 8.3 P2(高级能力 — 可选)

#### P2-1: Workflow 编排(Pregel + cba 算法)
```rust
pub struct WorkflowEngine {
    graph: DiGraph<ComponentNode, Connection>,
    compilers: HashMap<ComponentType, Box<dyn ComponentCompiler>>,
}
// petgraph crate + 自实现 cba 消减 + 笛卡尔积
```

#### P2-2: DSL 转换器(抽象基类 + 工厂)
```rust
pub trait WorkflowConverter: Send + Sync {
    fn convert(&self, json: Value) -> Result<WorkflowImportResult>;
}
pub struct ConverterFactory;
impl ConverterFactory {
    pub fn create(format: WorkflowFormat) -> Box<dyn WorkflowConverter> { ... }
}
```

#### P2-3: 评估指标 — Pass@k + Pass^k
```rust
pub fn compute_pass_at_k(results: &[TrialResult], k: usize) -> f64 {
    let n = results.len(); let c = results.iter().filter(|r| r.passed).count();
    if n < k { return 0.0; }
    1.0 - (comb(n - c, k) / comb(n, k))
}
```

#### P2-4: 多渠道接入(MCP Server 模式)

将 laew 的核心能力(Bash/Read/Write 等)暴露为 MCP Server,支持 stdio/SSE 传输,允许 Claude Desktop 等客户端调用。

### 8.4 核心机制映射总表

| agent-studio 核心机制 | laew 借鉴优先级 | 实现路径 |
|----------------------|---------------|----------|
| 三维 JSON 缓存 | P0 | `ToolRegistry::compile_or_get_cached` |
| COMPILER_HANDLERS 注册表 | P0 | `HashMap<String, Box<dyn ToolCompiler>>` |
| KB 注入 prompt | P0 | YoloRunner 前置检索 |
| MCP 5 种传输 | P0 | `PluginMcpTool` + `McpClient` trait |
| 冲突检测 + 取消 | P0 | `tokio::sync::Mutex<is_running>` |
| 评估 3 种评分器 | P1 | `Grader` trait + 9 种比较条件 |
| BubbleWrap + Seccomp | P1 | `nix` + `seccompiler` crates |
| 提示词版本管理 | P1 | SQLite `prompts` + `prompt_commits` |
| Pregel + cba 算法 | P2 | `petgraph` + 自实现 |
| DSL 转换器 | P2 | `WorkflowConverter` trait |
| Pass@k/Pass^k | P2 | 数学函数 |
| 回归检测 | P2 | success_rate/latency/score 阈值 |

---

## 附录:核心类与函数深度索引

| 模块 | 核心类/函数 | 代码路径 | 行数 | 关键设计 |
|------|-------------|----------|------|----------|
| 入口 | `lifespan_func` / `main` | `main.py` | - | FastAPI lifespan + APScheduler |
| Agent 执行 | `AgentRunner.run` | `core/executor/agent/agent_runner.py` | 559-788 | 6 步流水线 + KB 注入 |
| Agent 缓存 | `AgentRunner.get_agent_instance` | 同上 | 249-322 | 三维缓存 + JSON 比较 |
| 缓存全清 | `clear_agent_cache_for_all_conversations` | 同上 | 350-402 | 配置变更时清空所有会话 |
| 映射表 | `_create_mapping_table` | 同上 | 404-557 | 递归提取嵌套子工作流 |
| Workflow 执行 | `WorkflowRunner.run` | `core/executor/workflow/workflow_runner.py` | 223-396 | 冲突检测 + 双层取消 |
| Pregel 适配 | `PregelGraphAdapter.convert` | `pregel_graph_adapter.py` | 342-361 | cba 消减 + 笛卡尔积 |
| cba 消减 | `_reduce_cba_map` | 同上 | 40-49 | 向上传播分支汇合 |
| 依赖计算 | `_multiple_dependency` | 同上 | 118-139 | 笛卡尔积生成完整依赖 |
| 图校验 | `_validate_graph` | 同上 | 242-251 | 连通性 + 环检测 |
| 执行管理 | `WorkflowExecutionManager.cancel_execution` | `workflow_execution_manager.py` | 105-155 | 标志 + 任务双重取消 |
| 组件注册表 | `Workflow.COMPILER_HANDLERS` | `core/executor/workflow/workflow.py` | 109-121 | 11 种标准组件 |
| 编译入口 | `compile_component` | 同上 | 248-276 | 4 步编译流水线 |
| 编译方法 | `_compile_*_component` | 同上 | 279-346 | 11 个编译方法 |
| 特殊编译 | `_compile_special_component` | 同上 | 348-363 | IF/SUB_WORKFLOW/LOOP/PLUGIN |
| 插件抽象 | `ServiceTool/CodeTool/McpTool.compile` | `core/executor/plugin/plugin_tools.py` | 68-101 / 108-111 / 207-208 | 3 种插件类型 |
| MCP 5 传输 | `PluginMcpTool.invoke` | 同上 | 261-382 | stdio/sse/streamable/openapi/playwright |
| DSL 转换器 | `WorkflowConverter` | `core/dsl_converter/converter/converter.py` | 26-43 | 抽象基类 |
| DSL 工厂 | `ConverterFactory.create` | 同上 | 50-72 | 按格式分发 |
| 原生转换 | `NativeWorkflowConverter.convert` | `converter_native.py` | 113-277 | 8 步流水线 |
| ID 重建 | `regenerate_canvas_ids` | 同上 | 279-318 | 节点 ID 前缀映射 |
| 代码生成 | `WorkflowCodeGenerator.generate` | `core/manager/workflow_code_generator.py` | 104-119 | 7 段式 |
| 沙箱基类 | `BaseSandbox` | `sandbox_server/.../base.py` | 9-69 | `__init_subclass__` 自动注册 |
| BubbleWrap | `BubbleWrapRunner.run` | `bwrap.py` | 100-140 | 6 步执行 |
| 命名空间 | `_namespace_params` | 同上 | 194-198 | 6 种 Linux namespace |
| 文件挂载 | `_mount_params` | 同上 | 155-168 | 3 种挂载模式 |
| Seccomp | `pre_init` | 同上 | 81-98 | KILL 策略 + 白名单 |
| Python 内联 BPF | `_build_py_seccomp_loader` | 同上 | 33-71 | ctypes 运行时加载 |
| 评估编排 | `EvaluationHarness.execute_evaluation` | `evaluation/evaluation_harness.py` | 109-206 | 任务×扰动×trial 矩阵 |
| Trial 执行 | `_execute_task` / `_execute_trial` | 同上 | 212-236 / 238-400 | 4 扰动 × N trials |
| 评分引擎 | `GraderEngine.run_graders` | `evaluation/grader_engine.py` | 26-86 | 3 种评分器类型 |
| 确定性评分 | `_run_deterministic` | 同上 | 92-136 | 5 种 check_type |
| 模型评分 | `_run_model_based` | 同上 | 236-296 | LLM-as-Judge |
| 代码评分 | `_run_code_based` | 同上 | 341-382 | `exec()` 用户脚本 |
| 提示词扰动 | `PromptPerturber.paraphrase` | `evaluation/perturbations.py` | 33-50 | LLM 优先 + 规则回退 |
| 环境扰动 | `EnvironmentPerturber.perturb_input` | 同上 | 155-182 | 4 种扰动 |
| 故障注入 | `FaultInjector.generate_fault` | 同上 | 289-330 | 4 种故障类型 |
| Pass@k | `compute_pass_at_k` | `evaluation/metrics.py` | 35-70 | 二项式系数 |
| 回归检测 | `_detect_regressions` | `evaluation_harness.py` | 640-708 | 3 类阈值 |
| 知识库 | `knowledge_base_create` / `retrieve_multi_kb` | `core/manager/knowledge_base.py` | - | Milvus/Chroma 双轨 |
| 记忆 | `get_longterm_mem` / `get_user_variable` | `core/manager/memory.py` | - | 长期记忆 + 变量 |
| 调度器 | `init_scheduler` / `sync_triggers_to_scheduler` | `core/scheduler/scheduler.py` | - | APScheduler + Cron/Webhook |
| LLM 管理 | `get_llm_client` / `get_llm_client_by_protocol` | `ops/modules/llm/llm_manager.py` | - | LRU 缓存 + 多提供商适配 |
| 提示词 | `PromptService.create_prompt` | `ops/modules/prompt/application/service.py` | - | DDD 分层 + 草稿/提交版本 |
| MCP Server | `main` / `register_all` | `connect/adapters/mcp_server/server.py` | - | FastMCP + stdio/SSE 传输 |

---

> **总结**:openJiuwen Studio 在 7 个核心机制层面都有**生产级**的实现:AgentRunner 的三维 JSON 缓存、WorkflowRunner 的双重取消机制、PregelGraphAdapter 的 cba 消减算法、5 种 MCP 传输、BubbleWrap + Seccomp 多层隔离、4 扰动 × N trial 评估矩阵、11 种组件注册表。对 laew 而言,**P0 应优先实现**工具注册表、知识库注入、MCP Client、冲突检测;**P1** 重点放在 LLM-as-Judge、BubbleWrap 沙箱、提示词版本管理;**P2** 推进 Workflow 编排、DSL 转换、Pass@k 指标。

---

## 第八轮深挖 — 可视化低代码 + BubbleWrap 沙箱 + 多 channel 适配器 + Helm 多镜像编排

> 调研时间：2026-09-07。第八轮在第七轮基础上补充 openJiuwen Studio 的真实实现。所有引用路径均为绝对路径 + 行号。

### 1. 整体架构（补充）

`backend/openjiuwen_studio/` FastAPI 应用：
- 核心路由 `routers/`（auth / auth_new / agents / execution / workflows / knowledge_base / prompt_router 等）
- `core/`（manager/、executor/、dsl_converter/、plugin_server/、scheduler/、database/）
- `ops/modules/` 业务模块（llm/prompt）
- `models/` SQLAlchemy 模型
- `schemas/` Pydantic
- `evaluation/` 评测 SDK+CLI
- `marketplace/plugins_creator/` Swagger 转插件
- `lowcode/` runtime_workflow_runner

`connect/adapters/mcp_server/` MCP 工具暴露；`connect/adapters/channels/platforms/` Slack/Telegram/Email/CLI/Webhook/Alexa；`connect/client/` 客户端 SDK。

`sandbox_server/sandbox/openjiuwen_sandbox_server/app/` **BubbleWrap + seccomp + iptables 隔离**；`sandbox_server/gateway/openjiuwen_sandbox_gateway/app/gateway.py` 网关转发。

`plugin_server/openjiuwen_plugin_server/` 插件市场后端。

### 2. 第八轮 8 维度真实代码锚点

| 维度 | 路径 | 范式要点 |
|---|---|---|
| **Telemetry** | `backend/openjiuwen_studio/schemas/trace_summary.py`、`core/manager/login_manager/auth_service.py` | logger 记录 + `schemas/execution_log.py` |
| **Session 持久化** | `core/manager/login_manager/session_auth.py` | session 认证；`schemas/runtime.py` 运行时状态；`evaluation/sdk/client.py` 评测客户端 |
| **Tool 权限** | `connect/adapters/mcp_server/tools/registrator.py` | 工具注册；`tools/agents/registrator.py`、`tools/general/registrator.py` 按域分类 |
| **LSP/Hook** | `core/dsl_converter/` | DSL 转换；`converter/converter_n8n.py` n8n 互转；`lowcode/runtime_workflow_runner.py` 低代码工作流执行 |
| **Skill 一等公民** | `marketplace/ready_plugins/`、`marketplace/benchmarks/`、`marketplace/plugins_creator/` | 完整插件市场；`core/plugin_server/` 后端 |
| **多租户** | `core/manager/login_manager/user.py` | 用户管理；`routers/auth.py`、`auth_new.py` 多套认证；`models/user.py` |
| **TUI 渲染** | `connect/adapters/channels/platforms/cli/` | CLI channel；`cli/commands/` 命令注册 |
| **Runtime Telemetry** | `main.py` 日志初始化；`routers/deepsearch_logger.py` deepsearch 日志端点 |

### 3. 第九轮新维度真实代码锚点

| 维度 | 路径 | 范式要点 |
|---|---|---|
| **Crash/Recovery** | `schemas/execution_log.py` | 执行日志；`core/scheduler/` 调度；`evaluation/sdk/` 评测回放 |
| **OAuth** | `routers/auth.py`、`auth_new.py` | 多套认证路由；`connect/client/auth/token_storage/` token 存储 |
| **i18n** | `examples/zh/`、`examples/en/` | 双语示例；`core/common/` 多语言 message；`connect/adapters/channels/platforms/cli/` 多语言 prompt |
| **Release** | `helm/` Kubernetes Helm chart；`docker/` Dockerfile；`pyproject.toml` |
| **WS/SSE** | `connect/adapters/channels/platforms/webhook/` | webhook + `routes/`；`connect/adapters/channels/platforms/slack/` Slack socket mode |
| **Dev Container** | `sandbox_server/sandbox/openjiuwen_sandbox_server/app/bwrap.py:1-80`、`network_guard.py:1-80` | **BubbleWrap + pyseccomp 自定义 BPF + iptables 屏蔽内网** |
| **CRDT** | `core/plugin_server/` | 插件版本协调；`marketplace/benchmarks/` 评测基线 |

### 4. 关键代码片段

#### 4.1 BubbleWrap + 自编译 seccomp BPF 加载器（`sandbox_server/sandbox/openjiuwen_sandbox_server/app/bwrap.py:33-71`）

```python
def _build_py_seccomp_loader(bpf_path):
    """Generate Python code that loads a seccomp BPF filter at runtime."""
    return f'''
import struct, ctypes, json
def _load_seccomp(bpf_file):
    PR_SET_NO_NEW_PRIVS = 38
    PR_SET_SECCOMP = 22
    SECCOMP_MODE_FILTER = 2
    class SockFilter(ctypes.Structure):
        _fields_ = [("code", ctypes.c_ushort), ("jt", ctypes.c_ubyte),
                    ("jf", ctypes.c_ubyte), ("k", ctypes.c_uint32)]
    ...
    libc = ctypes.CDLL(None, use_errno=True)
    ret = libc.prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)
    ret = libc.prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, ctypes.byref(prog))
'''
```

#### 4.2 网络防火墙：屏蔽内网 + 允许 DNS（`network_guard.py:8-49`）

```python
IPV4_INTERNAL_CIDRS = (
    '0.0.0.0/8', '10.0.0.0/8', '127.0.0.0/8',
    '169.254.0.0/16', '172.16.0.0/12', '192.168.0.0/16',
)
CHAIN_V4 = 'OJ_SANDBOX_BLOCK_INT'
CHAIN_V6 = 'OJ_SANDBOX_BLOCK_INT6'

def apply_internal_network_guard(run_user='app'):
    if os.name != 'posix': return
    uid = _resolve_uid(run_user)
    dns_servers = _read_dns_servers()
    _configure_family(binary='iptables', chain=CHAIN_V4, uid=uid,
                      internal_cidrs=IPV4_INTERNAL_CIDRS,
                      dns_servers=[ip for ip in dns_servers if ip.version == 4])
```

#### 4.3 Sandbox 网关 HTTP 转发（`gateway/openjiuwen_sandbox_gateway/app/gateway.py:11-20`）

```python
async def remote_server(lang, code, inputs, session, timeout: float = 10.0):
    payload = {"session": session, "language": lang, "code": code,
 "timeout": timeout, "inputs": inputs or {}}
    async with httpx.AsyncClient() as cli:
        try:
            r = await cli.post(SANDBOX_SERVER_URL, json=payload,
                               timeout=httpx.Timeout(TIMEOUT + timeout, connect=TIMEOUT))
            r.raise_for_status()
            return r.json()
 except Exception as e:
            return {"return": None, "error": str(e)}
```

### 5. 设计哲学

agent-studio 是 **「可视化 Agent 构建 + 隔离执行」** 双轨架构：

1. **最核心创新在 sandbox_server**：在 Linux 上用 **BubbleWrap（用户/IPC/PID/net/UTS/cgroup 命名空间）+ pyseccomp 自定义 BPF 字节码 + iptables 屏蔽内网** 三层叠加实现接近 Docker 的隔离但更轻量。
2. **网络层** `apply_internal_network_guard` 在 host 上为 sandbox 用户创建 `iptables OJ_SANDBOX_BLOCK_INT` 链屏蔽 RFC1918 全部内网段但保留 `/etc/resolv.conf` 里的 DNS —— 防"沙箱内代码反向攻击 host 内网"的标配。
3. **connect/adapters/channels/platforms/** 把 Slack/Telegram/Email/CLI/Webhook/Alexa 全部抽象成统一 channel 接口，每个平台自己的 `routes/` `commands/` 目录，是 Adapter Pattern 的教科书实现。
4. **plugin_server + marketplace/ready_plugins + benchmarks** 组成完整插件生态闭环；`plugins_creator/from_swagger/importer.py` 实现 OpenAPI → MCP tool 自动转换。

**对 laew 的启示**：agent-studio 的 **BubbleWrap + seccomp BPF + iptables 三层沙箱** 是 Linux 用户态隔离的工业级范本，laew 升级时如需沙箱可参考此实现。

---

> **字数**：本文档 agent-studio 第八轮深挖章节新增约 650 行。
