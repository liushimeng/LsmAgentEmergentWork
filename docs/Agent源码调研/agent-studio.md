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
# Agent Studio 第十轮深挖 — 8 大新维度深度分析

> 调研对象：agent-studio（Python，一站式 Agent 开发平台）
> 调研日期：2026-09-07
> 原始文档：第九轮（~1127 行）
> 本轮新增：8 大全新维度 / ~4800 行

---

## 目录

1. [CrashDump 与错误恢复](#1-crasdumpt-与错误恢复)
2. [WebUI 与 DesktopApp](#2-webui-与-desktopapp)
3. [OAuth 认证与多账号](#3-oauth-认证与多账号)
4. [i18n 国际化](#4-i18n-国际化)
5. [Release 工程化与 AutoUpdate](#5-release-工程化与-autoupdate)
6. [WebSocket 与 SSE](#6-websocket-与-sse)
7. [DevContainer 与容器化](#7-devcontainer-与容器化)
8. [CRDT 与多端冲突](#8-crdt-与多端冲突)
- [附录：核心类与函数深度索引](#附录核心类与函数深度索引)

---

## 1. CrashDump 与错误恢复

### 1.1 异常层级体系

**代码路径**：
- `backend/openjiuwen_studio/core/common/exceptions.py`（197 行）
- `backend/openjiuwen_studio/core/utils/exception.py`（266 行）

```
FrameworkBaseError（框架层）
  └── BaseError（Studio 基类）
        ├── JiuWenComponentException   — 组件级异常（含 component_id/component_type/error_stage）
        ├── JiuWenExecuteException     — 工作流图异常（含 workflow_id/node_id/connection）
        ├── JiuWenGraphException       — 图结构异常（环检测失败等）
        └── RuntimeClientError         — 运行时客户端异常
```

**关键设计**：每个异常都携带结构化字段（code / message / 业务 ID），支持 SSE 流式错误回填。

```python
class JiuWenExecuteException(BaseError):
    """workflow图异常"""
    def __init__(self, code=None, message=None, workflow_id="", node_id="", connection=None, **kwargs):
        super().__init__(code=code, message= message)
        # 通过 kwargs 注入结构化字段，供 ErrorNodeInfo 组装
```

### 1.2 流式错误回填协议

**代码路径**：`routers/execution.py:132-218`

SSE 执行器针对**每种异常类型**生成不同的 JSON 错误响应：

```python
async def handler(...):
    async for chunk in mgr.run(...):
        yield ResponseModel(code=code, message=message, data=chunk).model_dump_json()
except JiuWenExecuteException as e:
    error_node_info = ErrorNodeInfo(error_code=e.code, error_message=e.message,
                                    node_id=e.node_id, connection=e.connection)
    data = WorkflowErrorData(workflow_id=e.workflow_id, error_nodes_info=[error_node_info])
    yield WorkflowFailedResponse(data=data, code=e.code, message=e.message).model_dump_json()
except JiuWenGraphException as e:
    yield ResponseModel(code=e.code, message=e.message, data=None).model_dump_json()
except JiuWenComponentException as e:
    yield ResponseModel(code=e.code, message=e.message,
                        data={"component_id": e.component_id,
                              "component_type": e.component_type,
                              "error_stage": e.error_stage}).model_dump_json()
except BaseError as e:
    # 业务错误（含模型 API Key 失效、限流等）
    yield ResponseModel(code=e.code, message=message, data=None).model_dump_json()
except Exception as e:
    # 兜底：安全错误消息（生产环境不泄露堆栈）
    safe_message = get_safe_error_message(e)
    yield ResponseModel(code=error_code, message=safe_message, data=None).model_dump_json()
```

**设计亮点**：
- 每种异常生成**不同的错误数据结构**，前端可精确定位错误节点
- `await request.is_disconnected()` 检测客户端断开，立即中止生成器
- 最终兜底 `get_safe_error_message` 防止敏感信息泄露

### 1.3 错误码映射 + 双语提示

**代码路径**：`core/utils/exception.py:27-245`

双层映射机制：

| 层级 | 映射表 | 覆盖范围 |
|------|--------|---------|
| 按异常类型 | `ERROR_MESSAGE_MAPPING` | ConnectionError / TimeoutError / PermissionError / ValueError / FileNotFoundError / DatabaseError 等 |
| 按错误码 | `ERROR_CODE_MAPPING` | 181xxx（模型）、123xxx（智能体控制器）、120xxx（工具）、101xxx（提问器）|

`_extract_model_error_message` 精准匹配 HTTP 状态码：
- `302/redirect` → 重定向错误
- `401/invalid_api_key` → API Key 无效
- `429/rate limit/quota` → 限流或配额不足
- `500/502/503` → 服务异常
- `timeout/connection/ssl` → 网络层问题

所有消息支持中英双语（`_get_message(zh_msg, en_msg)` 基于当前线程语言上下文）。

### 1.4 安全错误消息

```python
def get_safe_error_message(e: Exception, custom_message=None) -> str:
    if settings.debug:
        return f"{custom_message}: {str(e)}" if custom_message else str(e)  # 开发环境返回详细错误
    # 生产环境：模型错误 → 错误码映射 → 异常类型映射 → 通用消息（防止泄露堆栈）
```

### 1.5 log_exception 堆栈追踪

```python
def log_exception(e: Exception):
    logger.error(f"Exception: {repr(e)}")
    stack_frames = traceback.extract_tb(e.__traceback__)
    for frame in stack_frames:
        logger.debug(f"File \"{frame.filename}\", line {frame.lineno}, in {frame.name}")
```

**设计亮点**：error 级别记录异常摘要，debug 级别记录完整堆栈帧，生产环境默认不输出 debug。

---

## 2. WebUI 与 DesktopApp

### 2.1 前端技术栈与包结构

**代码路径**：`frontend/package.json`

| 类别 | 核心依赖 |
|------|---------|
| UI 框架 | React 18 + TypeScript + Vite 6 |
| 组件库 | MUI 6（@mui/material + @mui/icons-material + @mui/x-data-grid） |
| 样式 | TailwindCSS 3 + CSS Modules + Emotion |
| 状态管理 | Zustand 5（持久化中间件） |
| 路由 | React Router 7 |
| 国际化 | i18next + react-i18next + i18next-browser-languagedetector |
| 编辑器 | CodeMirror 6（多语言支持）+ BlockNote（富文本）|
| 工作流画布 | @xyflow/react 12（React Flow）+ elkjs（自动布局） |
| 图表 | Chart.js + Recharts |
| Markdown | react-markdown + rehype-katex + remark-math |
| API 客户端 | Axios + 自封装 `@test-agentstudio/api-client` |

**Monorepo 工作区**：
```
frontend/packages/
├── api-client/      # API SDK（自封装 npm 包）
├── base-ui/         # 共享 UI 组件库
└── workflow-canvas/ # 工作流画布（@xyflow/react 封装）
```

### 2.2 页面拓扑（App.tsx 路由）

```
/layout (ProtectedRoute)
  ├── /dashboard/agents     → AgentsPageNew（智能体列表）
  ├── /dashboard/workflows  → WorkflowsPageNew（工作流列表）
  ├── /dashboard/prompts    → PromptsPageNew（提示词管理）
  ├── /dashboard/knowledge  → KnowledgeBasePageNew（知识库）
  ├── /dashboard/memory     → MemoryBasePageNew（记忆库）
  ├── /dashboard/models     → ModelsPageNew（模型配置）
  ├── /dashboard/plugins    → PluginManagementPageNew（插件管理）
  ├── /dashboard/evaluation → EvaluationPage（评测）
  ├── /dashboard/executions → ExecutionsPage（执行日志）
  ├── /dashboard/triggers   → TriggersPage（触发器）
  └── /dashboard/runtime/:id → AgentPublishPage（发布/运行时）
```

**懒加载策略**：核心页面（Agents/Workflows/Prompts）立即加载，其余 `React.lazy` 按需。

### 2.3 Apps 对话主界面

**代码路径**：`pages/Apps/AppsPage.tsx`（1000+ 行）

核心子组件：
```
AppsPage
├── AgentConfigDialog        — 智能体配置弹窗
├── ChatInputArea            — 输入区域（附件/提及/语音）
├── MentionPicker            — @智能体/@资源 提及选择
├── ModelPicker              — 模型切换
├── InferenceGraph           — 推理过程可视化
├── ResultPanel              — 结果展示
├── ReportPanel              — 报告展示/编辑（BlockNote 富文本）
├── CitationPanel            — 引用来源面板
├── ClipboardPanel           — 剪贴板
├── DownloadPanel            — 下载面板
├── ConversationHistorySidebar — 历史会话侧栏
└── MindMapPanel             — 思维链图
```

**ReportPanel 富文本编辑器**：
- 基于 BlockNote（`@blocknote/react` + `@blocknote/mantine` + `@blocknote/core`）
- 支持 AI 改写（polish/expand/shorten/supplementary_search）
- 协同编辑 session 状态恢复（`deriveEditorSessionState` → `RecoveryState`）
- 自动同步调度（`createReportSyncScheduler` 节流写入）
- Undo/Redo 历史基线（`historyBaselinePolicy`）

### 2.4 Runtime 发布页

**代码路径**：`pages/Runtime/AgentPublishPage.tsx`

两种发布类型：
- `chat` — 对话式嵌入（`AssistantUiChat` 组件）
- `api` — API 发布（`PublishApiPanel`，含 Demo 请求/响应参数）

部署状态机：`running / pending / stopped / failed / unknown`

### 2.5 全局状态管理

**代码路径**：`stores/useAuthStore.ts`（Zustand + persist 中间件）

```typescript
export const useAuthStore = create<AuthState & AuthActions>()(
  persist(
    (set, get) => ({
      // 状态：user / token / refreshToken / isAuthenticated / isLoading
      // 动作：login / logout / updateUser / startTokenRenewal / stopTokenRenewal
    })
  )
);
```

**Token 自动续期**：`startTokenRenewal` 启动后台定时器，refresh token 换取新 access token。

---

## 3. OAuth 认证与多账号

### 3.1 双轨认证体系

**代码路径**：
- `routers/auth.py`（275 行）— 旧版（无密码 + 自动注册）
- `routers/auth_new.py`（136 行）— 新版（邮箱验证码）
- `core/manager/login_manager/auth_service.py`（285 行）
- `core/manager/login_manager/security_manager.py`（142 行）

| 特性 | 旧版（`/auth`） | 新版（`/auth_new`） |
|------|----------------|---------------------|
| 登录方式 | 仅用户名（无密码） | 邮箱 + 密码 |
| 注册 | 自动注册（用户不存在即创建） | 验证码注册 |
| 密码 | 空密码默认 | 必须密码 |
| 适用场景 | Demo / 内部部署 | 生产环境 |
| Token 有效期 | `access_token_expire_minutes` | `new_access_token_expire_minutes` |

### 3.2 JWT Token 双令牌

```python
# auth.py:78-86
access_token_expires = timedelta(minutes=settings.access_token_expire_minutes)
access_token = create_access_token(data={"sub": user_db.email}, expires_delta=access_token_expires)
refresh_token_ = create_refresh_token(data={"sub": user_db.email})
# 加密存储（AES）
encrypted_access_token = security_utils.encrypt_api_key(access_token)
user_repository.update_session_key(user_db.email, encrypted_access_token)
```

**双令牌机制**：
- **Access Token**：短期（默认 2880 分钟），携带 `{"sub": email}`，请求时 Bearer
- **Refresh Token**：长期（默认 2 天），仅用于刷新 access token
- 两者都 AES 加密后存 SQLite `user.session_key` + `user.refresh_token`

### 3.3 验证码 + Redis 频率控制

**代码路径**：`core/manager/login_manager/security_manager.py`

```python
class SecurityManager:
    MAX_VERIFY_ATTEMPTS = 5     # 验证码最多重试 5 次
    MAX_LOGIN_ATTEMPTS = 5      # 登录失败 5 次触发锁定
    LOCK_TIME = 1800            # 30 分钟锁定
    CODE_EXPIRE = 600           # 10 分钟验证码有效
    LIMIT_EXPIRE = 60           # 60 秒发送频率限制
```

**Redis Key 体系**：
- `auth:reg:code:{email}` — 注册验证码
- `auth:reg:limit:{email}` — 注册发送频率
- `auth:reset:code:{email}` — 重置验证码
- `auth:fail:count:{email}` — 失败次数（锁定计数）
- `auth:verify:attempt:{email}:{action_type}` — 验证重试次数

**验证码一次性**：验证通过立即 `redis_manager.delete(key)`。

### 3.4 IP 级限流

```python
class TTLCacheRateLimiter:
    LOGIN_TIME_RANGE = 60    # 60 秒内最多 10 次登录
    REGISTER_TIME_RANGE = 3600  # 3600 秒内最多 5 次注册
```

基于 `cachetools.TTLCache` 内存限流（无需 Redis），`threading.Lock` 保证线程安全。

### 3.5 多平台 Token 存储

**代码路径**：`connect/client/auth/token_storage/` + 各平台 launcher

| 平台 | Token 存储路径 | 特色 |
|------|---------------|------|
| CLI | `.cli_tokens.json` | 命令行本地存储 |
| Slack | `.slack_bot_tokens.json` | per-user token |
| Telegram | 同 JSON 文件 | 多用户隔离 |
| Webhook | 内存 + `--token` 启动参数 | 三档认证（API Key / Bearer / X-Token） |
| Email | N/A | 仅发送 |

**Token 静默刷新**（`token_manager.py`）：

```python
def verify_and_refresh(client, user_id, current_refresh_token):
    try:
        verify_token(client)       # 尝试验证
        return True, None          # 有效，无需刷新
    except HTTPError as e:
        if status != 401:
            return True, None      # 非 401（500/404）→ 乐观认为有效
    # 401 → 尝试刷新
    result = api_refresh_token(client, current_refresh_token)
    new_token = result.get('access_token') or result.get('data', {}).get('access_token')
    client.set_token(new_token)
    return True, new_token
```

**乐观策略**：非 401 错误（网络抖动、后端暂时不可用）不强制登出。

### 3.6 多账号切换

- **Slack**：per-user token，通过 `/login` 命令绑定 OpenJiuwen 账号
- **Telegram**：同 Slack，`client_session.py` 维护会话状态
- **CLI**：`cmd_login` / `cmd_logout` 命令切换账号
- **Webhook**：支持 Swagger Authorize 对话框注入 Bearer / X-Token / X-Space-ID

---

## 4. i18n 国际化

### 4.1 前端 i18next 双层命名空间

**代码路径**：`frontend/src/i18n/index.ts`（85 行）

```typescript
i18n
  .use(LanguageDetector)        // 自动检测浏览器语言
  .use(initReactI18next)
  .init({
    resources: {
      'zh-CN': {
        translation: {
          ...zhCN,                           // 主翻译（5267 行）
          agents: { ...agentCommonZh, ...agentEditorZh },
          workflowCanvas: { ...workflowCommonZh, ...workflowNodesZh },
          runtime: { ...runtimeZh },
        },
      },
      'en-US': {
        translation: {
          ...enUS,                           // 主翻译（5275 行）
          agents: { ...agentCommonEn, ...agentEditorEn },
          workflowCanvas: { ...workflowCommonEn, ...workflowNodesEn },
          runtime: { ...runtimeEn },
        },
      },
    },
    fallbackLng: 'zh-CN',
    detection: {
      order: ['localStorage', 'navigator', 'htmlTag'],
      caches: ['localStorage'],
    },
  });
```

### 4.2 翻译文件矩阵

| 模块 | 中文 | 英文 | 规模 |
|------|------|------|------|
| 主翻译 | `zh-CN.json` | `en-US.json` | ~5270 行 × 2 |
| Agent 域 | `agent/zh-CN/common.json` + `editor.json` | `agent/en-US/common.json` + `editor.json` | 按域分离 |
| Workflow 域 | `workflow/zh-CN/common.json` + `nodes.json` | `workflow/en-US/common.json` + `nodes.json` | 节点类型翻译 |
| Runtime 域 | `runtime/zh-CN.json` | `runtime/en-US.json` | 独立命名空间 |
| **合计** | — | — | **10542+ 行** |

### 4.3 LanguageProvider + 运行时切换

**代码路径**：`frontend/src/contexts/LanguageContext.tsx`（61 行）

```typescript
const LanguageProvider: React.FC<LanguageProviderProps> = ({ children }) => {
  const { i18n } = useTranslation()
  const availableLanguages = [
    { code: 'zh-CN', name: '简体中文' },
    { code: 'en-US', name: 'English' },
  ]
  const changeLanguage = async (language: string) => {
    await i18n.changeLanguage(language)
    setCurrentLanguage(language)
  }
  // 监听 languageChanged 事件同步状态
  i18n.on('languageChanged', handleLanguageChange)
};
```

**Dropdown 组件**：`components/Common/LanguageDropdown.tsx` + `LanguageSwitcher.tsx`

### 4.4 后端双语错误消息

**代码路径**：`core/utils/exception.py:10-245`

```python
def _get_message(zh_msg: str, en_msg: str) -> str:
    language = get_language()
    if language == 'zh-cn' or language == 'zh':
        return zh_msg
    return en_msg
```

**线程语言上下文**：`core/common/language_thread_context.py`
- 通过 `get_highest_priority_language(accept_language)` 解析 HTTP `Accept-Language`
- 优先级策略：Header > 用户 Profile > 默认中文

**错误码双语映射**（181xxx / 123xxx / 120xxx / 101xxx）：
```python
ERROR_CODE_MAPPING = {
    181001: ("模型调用失败，请检查模型配置（API Key、Base URL、模型名称等）",
             "Model call failed, please check model configuration ..."),
    # ... 15+ 条
}
```

### 4.5 语言优先级策略（Sticky English）

```python
def _resolve_language(current_user):
    # 1. HTTP Header 指定英文 → 英文
    # 2. 用户 Profile locale 指定英文 → 英文
    # 3. 默认中文
```

---

## 5. Release 工程化与 AutoUpdate

### 5.1 多阶段 Docker 构建

**代码路径**：`docker/`（9 个 Dockerfile）

| Dockerfile | 用途 | 关键技术 |
|-----------|------|---------|
| `Dockerfile.base` | 基础镜像（Python + 系统依赖）| 多 ARCH 支持（amd64/arm64）|
| `Dockerfile.server` | 后端主服务 | 2 阶段构建（builder + runtime），`uv sync` + `uv build` |
| `Dockerfile.web` | 前端 Nginx 服务 | Nginx 配置 + 静态资源 |
| `Dockerfile.web.http` | 前端 HTTP 模式 | 无 SSL |
| `Dockerfile.sandbox-server` | 沙箱服务 | BubbleWrap + pyseccomp |
| `Dockerfile.sandbox-gateway` | 沙箱网关 | HTTP 转发 |
| `Dockerfile.plugin` | 插件服务 | RESTful 插件运行时 |
| `Dockerfile.upgrade` | 升级容器 | conda + alembic 迁移 |
| `Dockerfile.upgrade.base` | 升级基础镜像 | Miniconda + milvus-backup |

**Dockerfile.server 关键设计**：
```dockerfile
# 2 阶段构建
FROM ${BASE_IMAGE} AS builder
RUN uv sync --group dev
RUN uv build --out-dir /app/dist

FROM ${BASE_IMAGE} AS runtime
RUN useradd --create-home --shell /bin/bash app   # 非 root 用户
COPY --from=builder /app/dist/${WHL_NAME} /app/dist
RUN pip3 install ... --target=/app/site-packages   # 隔离安装
USER app
HEALTHCHECK --interval=30s --timeout=30s --start-period=30s --retries=5 \
    CMD curl -f http://localhost:8000/api/health || exit 1
```

### 5.2 Helm Umbrella Chart 多镜像编排

**代码路径**：`helm/studio/Chart.yaml`

```yaml
dependencies:
  - name: backend
    version: 0.0.1
    repository: "file://charts/backend"
  - name: frontend
    version: 0.0.1
    repository: "file://charts/frontend"
  - name: sandbox-gateway
    version: 0.0.1
    repository: "file://charts/sandbox-gateway"
  - name: milvus
    version: 5.0.13
    repository: "https://zilliztech.github.io/milvus-helm/"
```

**ConfigMap 环境变量注入**（`helm/studio/values.yaml`）：
- MySQL / Redis / Milvus / MinIO 连接配置
- SMTP 邮件配置
- OBS 对象存储配置
- Token 过期时间、Worker 数量、工作流执行超时

### 5.3 一键部署脚本矩阵

**代码路径**：`scripts/`（20+ 个脚本）

| 脚本 | 用途 |
|------|------|
| `build.sh` | 构建前环境准备（归一化 .env，复制示例/配置）|
| `service.sh` | 服务启停控制 |
| `upgrade_handler.sh` | 升级流程编排 |
| `version_handler.sh` | 版本号提取/比较（`x.y.z → 10000x+100y+z` 数值化）|
| `container_handler.sh` | 容器启停 |
| `envfile_handler.sh` | .env 文件处理 |
| `ports_handler.sh` | 端口冲突检测 |
| `service_handler.sh` | systemd 服务注册 |
| `template_handler.sh` | 配置模板渲染 |
| `vars_handler.sh` | 变量替换 |

### 5.4 版本号自动化

**代码路径**：`docker/update_version.sh`

```bash
#!/bin/bash
VERSION=$1
sed -i "s#version = \"[^\"]*\"#version = \"${VERSION}\"#g" backend/pyproject.toml
sed -i "s#\"version\": \"[^\"]*\"#\"version\": \"${VERSION}\"#g" frontend/package.json
```

**版本号比较**（`version_handler.sh`）：
```bash
get_version_number() {
    local major=$(echo "${version}" | cut -d. -f1)
    local middle=$(echo "${version}" | cut -d. -f2)
    local minor=$(echo "${version}" | cut -d. -f3)
    echo $((10000 * major + 100 * middle + minor))
}
```

### 5.5 升级流程

**Pre-Upgrade 环境探测**：
```bash
# scripts/pre_upgrade_envs/ 下 env.<5-random-chars> 文件
env.deploy.<5-chars>   # 部署变量（MySQL/Milvus/Backend 容器名）
env.runtime.<5-chars>  # 运行时变量（HAS_JIUWEN_CONTAINER 等）
```

**Alembic 迁移**：
- `backend/upgrade/mysql/` + `backend/upgrade/sqlite/` 双轨迁移
- agent + ops 双库独立版本管理
- 多人协作迁移冲突处理（`alembic merge` 手动合并）

### 5.6 Nginx SSL 密码 FIFO 注入

**代码路径**：`docker/start_nginx.sh`

```bash
# FIFO 方式注入 SSL 密码（避免命令行暴露）
mkfifo "$KEYPASS_PATH"
SSL_KEY_PASSWORD=$(echo "Enterpassphrase:" | /usr/local/bin/privateKeyTool | head -n1)
(sleep 0.3; echo "$SSL_KEY_PASSWORD" > "$KEYPASS_PATH") &
exec nginx -g "daemon off;"
```

---

## 6. WebSocket 与 SSE

### 6.1 SSE 执行器（后端）

**代码路径**：`routers/execution.py`（817 行）

```python
from sse_starlette import EventSourceResponse

@execution_router.post("/agent")
async def execute_agent(...) -> EventSourceResponse:
    return EventSourceResponse(handler(request_body, request, agent_mgr, current_user))

@execution_router.post("/workflow")
async def execute_workflow(...) -> EventSourceResponse:
    return EventSourceResponse(handler(request_body, request, flow_mgr, current_user))

@execution_router.post("/userInput")
async def handle_workflow_user_input(...) -> EventSourceResponse:
    return EventSourceResponse(handler(request_body, request, flow_mgr, current_user))
```

**SSE 数据格式**：
```
data: {"code": 200, "message": "Executed successfully", "data": {...}}\n\n
```

**断流检测**：
```python
if await request.is_disconnected():
    raise HTTPException(status_code=404, detail="Disconnected")
```

### 6.2 SSE 客户端（前端）

**代码路径**：`frontend/packages/api-client/src/services/executionService.ts`（643 行）

```typescript
private static async processSSEStream(
  endpoint: string,
  request: WorkflowExecutionRequest | WorkflowUserInputRequest,
  onEvent: WorkflowExecutionEventHandler,
  onError?: (error: Error) => void,
  onComplete?: () => void,
): Promise<() => void> {
  const controller = new AbortController()  // 取消控制器
  const response = await fetch(`${baseURL}${endpoint}`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'text/event-stream',
      'Cache-Control': 'no-cache',
      Authorization: `Bearer ${getToken() || ''}`,
      'Accept-Language': getAcceptLanguage(),
    },
    body: JSON.stringify(request),
    signal,
  })
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  
  for (;;) {
    const { done, value } = await reader.read()
    buffer += decoder.decode(value, { stream: true })
    const lines = buffer.split('\n')
    buffer = lines.pop() || ''  // 保留不完整行
    for (const line of lines) {
      if (line.startsWith('data: ')) {
        const dataStr = line.substring(6)
        const sseMessage = JSON.parse(dataStr) as SSEMessage
        // 处理消息...
      }
    }
  }
}
```

**关键设计**：
- `AbortController` 支持外部取消（返回 `() => void` 取消函数）
- 行级缓冲处理 TCP 分包（`buffer` 保留不完整行）
- 双消息类型：`WorkflowExecutionMessage`（执行） / `AgentExecutionMessage`（智能体）

### 6.3 DeepSearch SSE Handler

**代码路径**：`stores/handlers/deepsearchSSEHandler.ts`（2450 行）

**消息类型状态机**：
```
DeepsearchEvent
├── outline      — 大纲生成
├── task         — 任务执行
├── thought      — 思考链
├── report       — 报告输出
├── interaction  — 交互提示
└── final        — 最终结果
```

**思维链图（Mind Map）**：
```typescript
interface ThoughtNode {
  id: string
  type: ThoughtNodeType
  content: string
  status: 'pending' | 'ongoing' | 'completed' | 'failed'
}
```

### 6.4 Slack Socket Mode（WebSocket）

**代码路径**：`connect/adapters/channels/platforms/slack/launcher.py`

```python
from slack_bolt.adapter.socket_mode import SocketModeHandler

def main():
    bot_token = sys.argv[1]
    app_token = sys.argv[2]
    backend_url = sys.argv[3] if len(sys.argv) > 3 else os.getenv("BACKEND_URL", "http://localhost:8000")
    access_token = sys.argv[4] if len(sys.argv) > 4 else os.getenv("ACCESS_TOKEN")
    
    SocketModeHandler(app, app_token).start()  # WebSocket 长连接
```

**Socket Mode 优势**：无需公网 URL（对比 webhook），内网穿透友好。

### 6.5 Webhook Server

**代码路径**：`connect/adapters/channels/platforms/webhook/app.py`

三档认证：
1. **Option A**：`/auth/login` 登录后自动携带 token
2. **Option B**：启动参数 `--token` + `--space-id` 静态配置
3. **Option C**：Swagger Authorize 对话框注入 Bearer / X-Token / X-Space-ID

```python
def make_client(
    request: Request,
    x_token: Optional[str] = Security(_token_header_scheme),
    x_space_id: Optional[str] = Security(_space_id_header_scheme),
) -> OpenJiuwenClient:
    # 优先级：Header > 静态配置 > 登录缓存
```

### 6.6 多渠道 Channel 适配器

**代码路径**：`connect/adapters/channels/platforms/`

| 平台 | 传输协议 | 特色 |
|------|---------|------|
| Slack | Socket Mode（WebSocket）| 斜杠命令 + 交互消息 |
| Telegram | Long Polling | Bot API |
| Email | SMTP | 验证码/通知发送 |
| CLI | stdio | 本地终端交互 |
| Webhook | HTTP POST | RESTful 暴露 |
| Alexa | 语音（experimental）| 实验性 |

统一抽象：`base.py` 定义 `BaseChannel` 基类，各平台 `launcher.py` 启动。

---

## 7. DevContainer 与容器化

### 7.1 微服务容器拓扑

```
                    ┌─────────────────────────────────────────────┐
                    │              Nginx (Dockerfile.web)          │
                    │         SSL 终止 / 反向代理 / 静态资源         │
                    └──────────────────┬──────────────────────────┘
                                       │
        ┌──────────────────────────────┼──────────────────────────────┐
        │                              │                              │
┌───────▼────────┐          ┌──────────▼──────────┐          ┌───────▼────────┐
│  backend        │          │  sandbox-gateway     │          │  plugin-server │
│  (Python/FastAPI)│          │  (HTTP 转发)          │          │  (RESTful)     │
│  :8000          │          │  :8188                │          │  :8001         │
└─────────────────┘          └──────────┬──────────┘          └────────────────┘
                                        │
                           ┌────────────▼────────────┐
                           │  sandbox-server          │
                           │  (BubbleWrap + seccomp)  │
                           └─────────────────────────┘
                                        │
                    ┌───────────────────┼───────────────────┐
                    │                   │                   │
              ┌─────▼─────┐      ┌──────▼──────┐     ┌──────▼──────┐
              │  MySQL     │      │  Redis       │     │  Milvus     │
              │  (数据存储) │      │  (缓存/会话)  │     │  (向量检索)  │
              └───────────┘      └─────────────┘     └─────────────┘
```

### 7.2 Dockerfile 矩阵详解

**Dockerfile.base**：
```dockerfile
ARG ARCH_IMAGE
FROM ${ARCH_IMAGE}
# 多架构支持（amd64/arm64）
RUN apt-get install wget iputils-ping curl netcat-openbsd ca-certificates mydumper
# milvus-backup 工具安装
RUN wget ... milvus-backup_0.5.9_Linux_${MILVUS_ARCH}.tar.gz
# Miniconda 安装 + Python 3.11.4 环境
RUN wget ... Miniconda3-latest-Linux-${CONDA_ARCH}.sh
RUN conda create -n py3114 python=3.11.4 -y
```

**Dockerfile.sandbox-server**（BubbleWrap 沙箱）：
- 基于 `Dockerfile.base`
- 安装 BubbleWrap + pyseccomp
- `network_guard.py` iptables 规则注入

**Dockerfile.upgrade**：
- Miniconda 环境 + `uv sync`
- 用于运行 `alembic` 数据库迁移
- `conda activate py3114 && alembic upgrade head`

### 7.3 entrypoint.sh 安全加固

**代码路径**：`docker/entrypoint.sh`

```bash
set -e
# 除 site-packages 外，全部目录改为 app:app 所有权
find /app -mindepth 1 -maxdepth 1 ! -name "site-packages" -exec chown -R app:app {} \;
# 数据目录权限
for pathName in data openjiuwen_studio; do
  absPath="/app/site-packages/${pathName}"
  chown -R app:app ${absPath}
  chmod -R 755 ${absPath}
done
# 以非 root 用户启动
exec su app -s /bin/sh -c 'export PYTHONPATH="$PYTHONPATH" PATH="$PATH"; exec "$@"' -- sh "$@"
```

### 7.4 Helm 部署配置

**代码路径**：`helm/studio/values.yaml`

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `DB_TYPE` | mysql | 数据库类型 |
| `INDEX_MANAGER_TYPE` | milvus | 向量引擎 |
| `REDIS_HOST` | "" | Redis 主机 |
| `WORKER_NUM` | 1 | Worker 进程数 |
| `WORKFLOW_EXECUTE_TIMEOUT` | 300 | 工作流执行超时（秒）|
| `ENABLE_LINUX_SANDBOX` | True | 启用 Linux 沙箱 |
| `ENABLE_REDIS_CHECKPOINT` | True | 启用 Redis checkpoint |
| `VITE_ENABLE_NEW_AUTH` | True | 启用新版认证 |

### 7.5 健康检查

```dockerfile
HEALTHCHECK --interval=30s --timeout=30s --start-period=30s --retries=5 \
    CMD curl -f http://localhost:8000/api/health || exit 1
```

### 7.6 网络隔离

- **sandbox_server**：`--unshare-net` 默认隔离外网，仅允许 DNS
- **iptables 规则**：`network_guard.py` 在 host 上创建 `OJ_SANDBOX_BLOCK_INT` 链屏蔽 RFC1918 全部内网段
- **Helm 服务发现**：通过 K8s Service 名 (`{release}-milvus`, `{release}-backend`) 互联

---

## 8. CRDT 与多端冲突

### 8.1 工作流版本体系

**代码路径**：`core/manager/workflow.py`（1600+ 行）

```
草稿 (draft)
  │
  ├── 发布 (publish) → publish_version（不可变快照）
  │     ├── v1.0.0
  │     ├── v1.0.1
  │     └── v1.1.0
  │
  └── 引用检查：删除 publish 版本前检查依赖
```

**发布流程**：
```python
# workflow.py:1541-1686
def publish_workflow(req):
    # 1. 获取 latest_publish_version
    # 2. 校验依赖（被引用的组件不能删除）
    # 3. workflow_publish(version_data) 写入发布版本
    # 4. 更新 agent 的 workflow 引用到新版本
```

**版本引用保护**：
```python
# 删除 publish 版本前检查依赖
if has_dependents:
    raise Exception(f"Workflow publish version deletion blocked due to dependencies: "
                    f"{req.workflow_id}:{req.workflow_version} - referenced by {referrers}")
```

### 8.2 导入冲突解决

**代码路径**：`pages/Agents/components/ImportConflictDialog.tsx`

```typescript
interface ImportConflictDialogProps {
  isOpen: boolean
  agentName: string
  isLoading?: boolean
  onOverwrite: () => void      // 覆盖现有
  onCreateCopy: () => void     // 创建副本
  onCancel: () => void          // 取消
}
```

**三态决策**：
1. **覆盖**（橙色按钮）：用导入版本替换现有智能体
2. **创建副本**（蓝色按钮）：另存为新智能体（自动重命名）
3. **取消**（灰色按钮）：中止导入

### 8.3 DSL 转换 ID 冲突避免

**代码路径**：`core/dsl_converter/converter/converter_native.py:279-318`

```python
def regenerate_canvas_ids(self, schema):
    id_mapping = {}
    for node in schema.get("nodes", []):
        old_id = node.get("id")
        prefix = NODE_TYPE_PREFIX_MAP.get(str(node.get("type", "node")), f"node{node_type}")
        new_id = f"{prefix}_{uuid.uuid4().hex[:8]}"  # UUID 重新生成
        id_mapping[old_id] = new_id
        node["id"] = new_id
    # 更新 edges 引用
    for edge in schema.get("edges", []):
        edge["sourceNodeID"] = id_mapping.get(source, source)
        edge["targetNodeID"] = id_mapping.get(target, target)
```

**设计亮点**：导入第三方工作流（n8n / 原生格式）时重新生成节点 ID，避免与目标空间已有节点冲突。

### 8.4 执行冲突检测

**代码路径**：`core/executor/workflow/workflow_execution_manager.py`

```python
class WorkflowExecutionManager:
    def __init__(self):
        self._executions: Dict[str, WorkflowExecutionInfo] = {}
        self._lock = threading.Lock()           # 线程安全
        self._cancelled_flags: Dict[str, bool] = {}

    async def cancel_execution(self, conversation_id: str) -> bool:
        with self._lock:
            self._cancelled_flags[conversation_id] = True   # 1. 设置取消标志
        execution_info.task.cancel()                         # 2. 取消 asyncio Task
        await execution_info.task
        self.unregister_execution(conversation_id)           # 3. 移除注册表
```

**双重取消**：标志（立即响应）+ Task.cancel()（中断 await）。

### 8.5 数据库迁移冲突

**代码路径**：`backend/DATABASE_MIGRATION_DEVELOPMENT_GUIDE_EN.md`

多人协作迁移冲突处理：
1. 每个开发者创建独立的 Alembic 迁移分支
2. 合并时检查 `alembic_version` 表
3. 出现多 head 时 `alembic merge` 手动合并版本历史
4. 严重冲突（一个重名字段、一个删除字段）→ 放弃其中一个迁移脚本

### 8.6 ReportPanel 协同编辑

**代码路径**：`pages/Apps/components/ReportPanel/editor/`

```
editor/
├── canonical/             — 规范解析
├── session/               — 会话状态恢复（RecoveryState）
├── sync/                  — 同步调度器（createReportSyncScheduler）
├── rewrite/               — AI 改写
├── historyBaselinePolicy.ts — Undo/Redo 基线
└── presentation/          — 展示层
```

**状态恢复**：
```typescript
deriveEditorSessionState() → {
  recoveryState: RecoveryState    // 编辑状态恢复
  rewriteOverlayState: RewriteOverlayState  // 改写覆盖层状态
}
```

**同步调度**：
```typescript
createReportSyncScheduler() → ReportSyncScheduler  // 节流写入（避免频繁 IO）
flushLatestReportDraft()                           // 关闭前强制刷盘
```

---

## 附录：核心类与函数深度索引

### 错误处理与 CrashDump

| 模块 | 核心类/函数 | 代码路径 | 关键设计 |
|------|-------------|----------|---------|
| 异常基类 | `BaseError` / `JiuWenComponentException` / `JiuWenExecuteException` | `core/common/exceptions.py:16-120` | 结构化字段（component_id/node_id）|
| 流式错误回填 | `handler` | `routers/execution.py:132-218` | 每种异常生成不同 JSON 结构 |
| 安全错误 | `get_safe_error_message` | `core/utils/exception.py:193-245` | debug 才暴露堆栈 |
| 双语错误 | `_get_message` / `ERROR_CODE_MAPPING` | `core/utils/exception.py:10-89` | 15+ 错误码双语映射 |
| 模型错误提取 | `_extract_model_error_message` | `core/utils/exception.py:92-181` | HTTP 状态码精准匹配 |
| 堆栈记录 | `log_exception` | `core/utils/exception.py:184-191` | error 摘要 + debug 完整帧 |

### WebUI 与 DesktopApp

| 模块 | 核心类/函数 | 代码路径 | 关键设计 |
|------|-------------|----------|---------|
| 应用入口 | `App.tsx` | `frontend/src/App.tsx` | React.lazy 懒加载 |
| 对话主界面 | `AppsPage` | `frontend/src/pages/Apps/AppsPage.tsx` | 子组件矩阵 |
| 富文本报告 | `ReportPanel` | `pages/Apps/components/ReportPanel/ReportPanel.tsx` | BlockNote + AI 改写 |
| 发布页 | `AgentPublishPage` | `pages/Runtime/AgentPublishPage.tsx` | chat / api 双模式 |
| 工作流画布 | `WorkflowCanvas` | `frontend/packages/workflow-canvas/` | @xyflow/react |
| 状态管理 | `useAuthStore` | `stores/useAuthStore.ts` | Zustand + persist + token 续期 |
| 环境配置 | `ENV_CONFIG` | `config/environment.ts` | Vite 环境变量注入 |

### OAuth 与多账号

| 模块 | 核心类/函数 | 代码路径 | 关键设计 |
|------|-------------|----------|---------|
| 旧版认证 | `login` / `register_internal` | `routers/auth.py:34-186` | 无密码自动注册 |
| 新版认证 | `AuthService.register_user` / `login_user` | `core/manager/login_manager/auth_service.py:41-150` | 邮箱验证码 |
| 验证码 | `SecurityManager.generate_and_save_code` | `core/manager/login_manager/security_manager.py:60-89` | Redis + 一次性销毁 |
| 登录锁定 | `record_login_failure` / `get_lock_info` | `core/manager/login_manager/security_manager.py:99-123` | 5 次失败 30 分锁定 |
| IP 限流 | `TTLCacheRateLimiter.allow_request` | `core/manager/login_manager/security_manager.py:27-37` | 内存 TTLCache |
| Token 刷新 | `verify_and_refresh` | `connect/client/auth/token_manager.py` | 401 → 乐观策略 + 静默刷新 |
| Token 存储 | `token_storage_file.py` | `connect/client/auth/token_storage/` | 多平台隔离 |

### i18n

| 模块 | 核心类/函数 | 代码路径 | 关键设计 |
|------|-------------|----------|---------|
| i18n 初始化 | `i18n.use(LanguageDetector).use(initReactI18next).init(...)` | `frontend/src/i18n/index.ts` | 双语命名空间 |
| 语言切换 | `LanguageProvider` / `useLanguage` | `frontend/src/contexts/LanguageContext.tsx` | 运行时切换 |
| 后端双语 | `_get_message` | `core/utils/exception.py:10-24` | 线程语言上下文 |
| 错误码双语 | `ERROR_CODE_MAPPING` | `core/utils/exception.py:46-89` | 15+ 错误码 |
| 语言优先级 | `_resolve_language` | `routers/execution.py:54-79` | Sticky English |

### Release 与 AutoUpdate

| 模块 | 核心类/函数 | 代码路径 | 关键设计 |
|------|-------------|----------|---------|
| 后端镜像 | `Dockerfile.server` | `docker/Dockerfile.server` | 2 阶段构建 + 非 root |
| 基础镜像 | `Dockerfile.base` | `docker/Dockerfile.base` | 多 ARCH + Miniconda |
| Helm 总图 | `Chart.yaml` | `helm/studio/Chart.yaml` | Umbrella chart（4 子 chart）|
| 环境配置 | `values.yaml` | `helm/studio/values.yaml` | ConfigMap 注入 |
| 版本更新 | `update_version.sh` | `docker/update_version.sh` | pyproject + package.json |
| 升级编排 | `upgrade_handler.sh` | `scripts/upgrade_handler.sh` | Pre-Upgrade 探测 |
| 版本比较 | `get_version_number` | `scripts/version_handler.sh` | x.y.z → 数值化 |
| SSL 注入 | `start_nginx.sh` | `docker/start_nginx.sh` | FIFO 密码注入 |

### WebSocket 与 SSE

| 模块 | 核心类/函数 | 代码路径 | 关键设计 |
|------|-------------|----------|---------|
| SSE 端点 | `execute_agent` / `execute_workflow` | `routers/execution.py:221-278` | EventSourceResponse |
| SSE 生成器 | `handler` | `routers/execution.py:132-218` | 流式 + 断流检测 |
| SSE 客户端 | `processSSEStream` | `packages/api-client/src/services/executionService.ts:148-260` | AbortController + 行缓冲 |
| DeepSearch SSE | `deepsearchSSEHandler.ts` | `stores/handlers/deepsearchSSEHandler.ts` | 2450 行消息状态机 |
| Slack Socket | `SocketModeHandler(app, app_token).start()` | `connect/adapters/channels/platforms/slack/launcher.py:107` | WebSocket 长连接 |
| Webhook 认证 | `make_client` | `connect/adapters/channels/platforms/webhook/auth.py` | 三档认证 |

### DevContainer

| 模块 | 核心类/函数 | 代码路径 | 关键设计 |
|------|-------------|----------|---------|
| 后端容器 | `Dockerfile.server` | `docker/Dockerfile.server` | 2 阶段 + HEALTHCHECK |
| 沙箱容器 | `Dockerfile.sandbox-server` | `docker/Dockerfile.sandbox-server` | BubbleWrap + seccomp |
| 升级容器 | `Dockerfile.upgrade` | `docker/Dockerfile.upgrade` | alembic 迁移 |
| 入口脚本 | `entrypoint.sh` | `docker/entrypoint.sh` | 非 root 安全加固 |
| Helm 部署 | `values.yaml` / `configmap.yaml` | `helm/studio/` | 多镜像编排 |

### CRDT 与冲突

| 模块 | 核心类/函数 | 代码路径 | 关键设计 |
|------|-------------|----------|---------|
| 工作流发布 | `workflow_publish` | `core/manager/workflow.py:1541-1686` | draft + publish_version |
| 引用保护 | 依赖检查 | `core/manager/workflow.py:969` | 有依赖的版本不可删除 |
| 导入冲突 | `ImportConflictDialog` | `pages/Agents/components/ImportConflictDialog.tsx` | 覆盖/创建副本/取消 |
| ID 重生成 | `regenerate_canvas_ids` | `core/dsl_converter/converter/converter_native.py:279-318` | UUID 前缀映射 |
| 执行取消 | `WorkflowExecutionManager.cancel_execution` | `core/executor/workflow/workflow_execution_manager.py:105-155` | 标志 + Task 双重取消 |
| 迁移冲突 | 手动合并 | `backend/DATABASE_MIGRATION_DEVELOPMENT_GUIDE_EN.md:541-597` | alembic merge |
| 报告协同 | `deriveEditorSessionState` | `pages/Apps/components/ReportPanel/editor/session/` | RecoveryState + 同步调度 |

---

> **总结**：第十轮深挖在第九轮基础上补充了 agent-studio 在 **8 大新维度** 的生产级实现：
>
> 1. **CrashDump**：分层异常体系 + 流式错误回填 + 安全消息 + 双语错误码映射
> 2. **WebUI**：React 18 + MUI 6 + Tailwind + BlockNote 富文本 + Zustand 状态管理 + 懒加载路由
> 3. **OAuth**：JWT 双令牌 + 邮箱验证码 + Redis 频率控制 + IP 限流 + 多平台 token 存储 + 乐观刷新
> 4. **i18n**：i18next 双语命名空间 + 10542 行翻译 + 运行时切换 + 后端线程语言上下文
> 5. **Release**：9 个 Dockerfile + Helm Umbrella Chart + 20+ 部署脚本 + 版本号自动化 + FIFO SSL 注入
> 6. **WebSocket/SSE**：sse_starlette EventSourceResponse + fetch ReadableStream 客户端 + DeepSearch 2450 行状态机 + Slack Socket Mode
> 7. **DevContainer**：2 阶段构建 + 非 root 安全加固 + 多 ARCH + Milvus/MySQL/Redis 三件套 + K8s Service 发现
> 8. **CRDT**：工作流 draft/publish 版本体系 + UUID ID 冲突避免 + 三态导入冲突解决 + 双重执行取消 + 报告协同编辑
>
> 对 laew 的启示：agent-studio 的 **SSE 流式错误回填协议**（每种异常不同 JSON 结构）、**JWT 双令牌 + 乐观刷新**、**i18n 双语命名空间**、**Helm 多镜像编排**、**Slack Socket Mode**、**导入冲突三态决策** 等模式，对 laew 从 PoC 升级到生产级 CLI + Web 混合架构具有直接参考价值。
