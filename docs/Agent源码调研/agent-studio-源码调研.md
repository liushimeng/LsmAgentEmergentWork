# openJiuwen Studio 源码深度调研报告

> **工程定位**：openJiuwen Studio（九文 Agent Studio）是华为技术团队开源的一站式 AI Agent 开发平台，提供从开发到部署的全栈解决方案。采用低代码/零代码可视化设计与编排工具，支持开发者快速打造和调试智能体和工作流。
>
> **技术栈**：Python(FastAPI) + React + SQLAlchemy + SQLite/MySQL + Milvus/Chroma + MCP + APScheduler
>
> **代码规模**：后端 ~20 万行 Python，前端 React + TypeScript，多微服务架构

---

## 目录

1. [工程结构与多微服务组织](#1-工程结构与多微服务组织)
2. [核心架构与数据流](#2-核心架构与数据流)
3. [后端核心组织](#3-后端核心组织)
4. [执行引擎](#4-执行引擎)
5. [工作流引擎](#5-工作流引擎)
6. [DSL 转换](#6-dsl-转换)
7. [Agent 管理](#7-agent-管理)
8. [插件系统](#8-插件系统)
9. [知识库与 RAG](#9-知识库与-rag)
10. [记忆管理](#10-记忆管理)
11. [评估体系](#11-评估体系)
12. [多渠道连接](#12-多渠道连接)
13. [沙箱服务](#13-沙箱服务)
14. [前端实现](#14-前端实现)
15. [提示词工程](#15-提示词工程)
16. [多模型接入](#16-多模型接入)
17. [调度器](#17-调度器)
18. [数据库与迁移](#18-数据库与迁移)
19. [对 laew 工程的借鉴建议](#19-对-laew-工程的借鉴建议)

---

## 1. 工程结构与多微服务组织

### 1.1 顶层目录结构

```
agent-studio/
├── backend/                    # 主后端服务（FastAPI）
│   ├── openjiuwen_studio/      # 核心业务代码
│   ├── tests/                  # 后端测试
│   └── upgrade/                # 数据库迁移脚本（Alembic）
├── frontend/                   # React 前端
│   ├── src/                    # 主应用源码
│   └── packages/               # 独立包（workflow-canvas / api-client / base-ui）
├── connect/                    # 多渠道连接层
│   ├── adapters/               # 适配器（channels + mcp_server）
│   └── client/                 # 外部调用 SDK
├── plugin_server/              # 独立插件服务
├── sandbox_server/             # 代码沙箱服务
│   ├── gateway/                # 沙箱网关
│   └── sandbox/                # 沙箱执行器
├── docker/                     # Docker 部署配置
├── helm/                       # Helm Chart（K8s 部署）
├── scripts/                    # 辅助脚本
└── docs/                       # 文档（中英文）
```

### 1.2 微服务交互架构

```
┌─────────────────────────────────────────────────────────────────┐
│                        frontend (React)                          │
│  ┌─────────────────┐  ┌──────────────┐  ┌───────────────────┐  │
│  │ workflow-canvas │  │  api-client  │  │     base-ui       │  │
│  │ (FlowGram.ai)   │  │              │  │                   │  │
│  └────────┬────────┘  └──────┬───────┘  └───────────────────┘  │
└───────────┼─────────────────┼───────────────────────────────────┘
            │                 │ HTTP/REST
            ▼                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                  backend (FastAPI :8000)                         │
│  ┌──────────┐ ┌───────────┐ ┌──────────┐ ┌──────────────────┐  │
│  │ routers  │ │  manager  │ │ executor │ │  dsl_converter   │  │
│  └──────────┘ └───────────┘ └──────────┘ └──────────────────┘  │
│  ┌──────────┐ ┌───────────┐ ┌──────────┐ ┌──────────────────┐  │
│  │scheduler │ │  memory   │ │   ops    │ │  knowledge_base  │  │
│  └──────────┘ └───────────┘ └──────────┘ └──────────────────┘  │
└───────────┼─────────────────┼───────────────────────────────────┘
            │                 │
   ┌────────┼─────────┐       └──────────────────────┐
   ▼        ▼         ▼                              ▼
┌──────┐ ┌──────┐ ┌──────────┐  ┌──────────────┐  ┌──────────┐
│SQLite│ │MySQL │ │  Milvus  │  │   ChromaDB   │  │  Redis   │
└──────┘ └──────┘ └──────────┘  └──────────────┘  └──────────┘

┌──────────────────┐  ┌──────────────────┐  ┌─────────────────────┐
│  plugin_server   │  │  sandbox_server  │  │     connect         │
│  (独立 RESTful)  │  │  gateway+sandbox │  │  MCP Server / Channel│
└──────────────────┘  └──────────────────┘  └─────────────────────┘
```

### 1.3 设计要点

- **主从服务模式**：backend 是核心编排者，plugin_server/sandbox_server 是可插拔的辅助服务
- **存储抽象**：通过 `DB_TYPE` 环境变量切换 SQLite/MySQL，支持轻量开发与企业部署
- **向量库双轨**：`INDEX_MANAGER_TYPE` 环境变量切换 Milvus/Chroma
- **依赖注入**：`ops/dependencies.py` 提供多库会话管理（agent/ops 双库分离）

---

## 2. 核心架构与数据流

### 2.1 分层架构

```
┌─────────────────────────────────────────────────────────┐
│                    API 层（routers/）                    │
│  agents / workflows / plugins / knowledge_base / ...    │
├─────────────────────────────────────────────────────────┤
│                 业务管理层（core/manager/）               │
│  agent.py / workflow.py / plugin.py / memory.py / ...   │
├─────────────────────────────────────────────────────────┤
│                 执行引擎层（core/executor/）              │
│  agent/ / workflow/ / plugin/ / evaluation/ / component/│
├─────────────────────────────────────────────────────────┤
│              核心服务层（core/ 公共模块）                  │
│  common/ / database/ / utils/ / config.py               │
├─────────────────────────────────────────────────────────┤
│                  数据持久层（models/ + SQLite/MySQL）     │
└─────────────────────────────────────────────────────────┘
```

### 2.2 请求处理流水线

```python
# routers/agents.py → core/manager/agent.py → core/executor/agent/agent_runner.py
# 典型调用链：
# 1. Router 接收 HTTP 请求，解析为 Pydantic Schema
# 2. Manager 层处理业务逻辑（权限校验、数据转换、引用检查）
# 3. Executor 层执行具体推理（AgentRunner.run → Runner.run_agent_streaming）
# 4. 流式返回 AsyncGenerator 结果
```

### 2.3 核心数据流（Agent 执行）

```
用户输入
  │
  ▼
AgentRunner.run()
  │
  ├─ 1. _fetch_agent_dl()          # 从 DB 获取 Agent DL 配置
  ├─ 2. 知识库检索（可选）          # retrieve_multi_kb → 注入 prompt_template
  ├─ 3. get_agent_instance()       # 获取/缓存编译后的 Agent 实例
  ├─ 4. initialize_trace_context() # 初始化追踪上下文
  └─ 5. Runner.run_agent_streaming()  # 流式执行
        │
        └─ async for chunk in ...   # 逐 chunk 处理追踪信息并 yield
```

---

## 3. 后端核心组织

### 3.1 入口与生命周期

**代码路径**：`backend/openjiuwen_studio/main.py`

```python
@asynccontextmanager
async def lifespan_func(app: FastAPI):
    # 1. 创建数据库表（Base.metadata.create_all）
    # 2. 初始化记忆引擎（MemoryEngineManager.init）
    # 3. 检查 Alembic 版本
    # 4. 初始化 Runner（支持 Redis checkpoint）
    # 5. 启动触发器调度器（APScheduler）
    yield
    # 关闭：停止调度器
```

**设计要点**：
- 使用 FastAPI 的 `lifespan` 上下文管理器统一管理启动/关闭
- 纯 ASGI 中间件 `LogMiddleware` 处理请求日志（避免 BaseHTTPMiddleware 与 StreamingResponse 的兼容性问题）
- CORS 仅允许 localhost:3000（前端开发服务器）

### 3.2 配置管理

**代码路径**：`backend/openjiuwen_studio/core/config.py`

```python
# 环境变量驱动的配置
DB_TYPE = os.getenv("DB_TYPE", "sqlite")        # sqlite | mysql
INDEX_MANAGER_TYPE = os.getenv("INDEX_MANAGER_TYPE", "milvus")  # milvus | chroma
```

### 3.3 公共模块（core/common/）

| 文件 | 职责 |
|------|------|
| `dsl.py` | 工作流 DSL 定义（ComponentType 枚举、Connection、Component、BaseFlow） |
| `message.py` | 统一消息格式（ExecuteResponse、ExecuteResponseType） |
| `status_code.py` | 全局状态码定义 |
| `exceptions.py` | 异常体系（BaseError、JiuWenComponentException） |
| `agent_defaults.py` | Agent 默认配置 |
| `mcp_transport_utils.py` | MCP 传输层工具 |
| `url_validator.py` | URL 校验（防 SSRF） |

### 3.4 数据库连接

**代码路径**：`backend/openjiuwen_studio/core/database.py`

```python
def get_database_url() -> str:
    if settings.db_type.lower() == "mysql":
        return f"mysql+pymysql://{user}:{pwd}@{host}:{port}/{db}?charset=utf8mb4"
    elif settings.db_type.lower() == "sqlite":
        return f"sqlite:///{path}/{db_file}"
```

**设计要点**：
- 支持 SQLite（开发）/ MySQL（生产）双模式
- MinIO 懒加载客户端（`LazyMinioClient`），失败不影响主服务
- 连接池配置通过 SQLAlchemy engine 管理

---

## 4. 执行引擎

### 4.1 执行器总览

**代码路径**：`backend/openjiuwen_studio/core/executor/`

```
executor/
├── agent/              # Agent 执行器
│   ├── agent.py              # Agent 包装类
│   ├── agent_runner.py       # AgentRunner（核心执行）
│   ├── agent_dl_adapter.py   # DL 配置适配
│   └── agent_trace_utils.py  # 追踪工具
├── workflow/           # 工作流执行器
│   ├── workflow.py           # Workflow 编译
│   ├── workflow_runner.py    # WorkflowRunner
│   ├── workflow_execution_manager.py  # 执行管理（取消/冲突检测）
│   ├── context.py            # 执行上下文
│   └── pregel_graph_adapter.py  # Pregel 图算法适配
├── component/          # 组件执行器
│   ├── component_runner.py   # 单组件调试执行
│   ├── component_impl/       # 组件实现
│   │   ├── react_agent_comp.py   # ReAct Agent 组件
│   │   ├── code_comp.py          # 代码组件
│   │   ├── tool_comp.py          # 工具组件
│   │   ├── http_request_comp.py  # HTTP 请求组件
│   │   └── ...
│   └── compile/              # 组件编译器
│       ├── llm_comp_compiler.py
│       ├── react_agent_comp_compiler.py
│       └── ...
├── plugin/             # 插件执行器
│   ├── plugin_mgr.py         # PluginManager
│   └── plugin_tools.py       # ServiceTool / CodeTool / McpTool
├── evaluation/         # 评估执行器
│   ├── evaluation_harness.py # 评估编排
│   ├── grader_engine.py      # 评分引擎
│   ├── pattern_validator.py  # 模式验证
│   ├── perturbations.py      # 扰动注入
│   ├── safety_grader.py      # 安全评分
│   ├── metrics.py            # 指标计算
│   └── reliability_metrics.py
└── util/
    └── utils.py          # 通用工具
```

### 4.2 AgentRunner 核心实现

**代码路径**：`core/executor/agent/agent_runner.py`

```python
class AgentRunner:
    def __init__(self, flow_mgr: WorkflowRunner, plugin_mgr: PluginManager):
        self.flow_mgr = flow_mgr
        self.plugin_mgr = plugin_mgr
        self._agent_instances: Dict[str, Dict[str, Any]] = {}  # 实例缓存

    async def run(self, id, version, inputs, conversation_id, space_id, current_user):
        # 1. 参数验证
        # 2. 获取 Agent DL 配置
        agent_dl_json = await _fetch_agent_dl(id, version, space_id, current_user)
        agent_config = AgentDlAdapter.convert_to_agent_config(agent_dl_json)
        
        # 3. 知识库检索（可选）
        kb_ids, retrieval_config = AgentDlAdapter.get_knowledge_config(agent_dl_json)
        if kb_ids:
            kb_results = await retrieve_multi_kb(kbs, query, config)
            # 将检索结果注入 prompt_template
            for result_text in kb_results:
                agent_config.prompt_template.append({"role": "system", "content": result_text})
        
        # 4. 获取/缓存 Agent 实例
        invokable_agent = await self.get_agent_instance(...)
        
        # 5. 流式执行
        async for chunk in Runner.run_agent_streaming(agent, inputs, session):
            rsp = await process_chunk_trace(chunk, trace_context)
            if rsp:
                yield rsp
```

**设计要点**：
- **实例缓存**：按 `user_id + agent_id + version` 三维缓存，避免重复编译
- **知识库集成**：执行前自动检索知识库，将结果作为 system message 注入 prompt
- **追踪上下文**：完整的 trace 链路（agent → workflow → component）
- **取消机制**：通过 `workflow_execution_manager` 支持执行中取消

### 4.3 WorkflowRunner 核心实现

**代码路径**：`core/executor/workflow/workflow_runner.py`

```python
class WorkflowRunner(IWorkflowLoader):
    async def run(self, id, version, inputs, conversation_id, space_id, current_user):
        # 1. 冲突检测
        if workflow_execution_manager.is_executing(conversation_id):
            await workflow_execution_manager.cancel_execution(conversation_id)
        
        # 2. 编译工作流
        flow = await self.get_compiled_workflow(Context(), id, version, space_id, current_user)
        
        # 3. 创建 Session
        session = create_workflow_session(session_id=conversation_id)
        
        # 4. 注册执行
        workflow_execution_manager.register_execution(registration)
        
        # 5. 流式执行
        async for chunk in Runner.run_workflow_streaming(workflow, inputs, session, context):
            rsp, trace_log, _ = result_convert(chunk, business_type="WORKFLOW")
            if trace_log:
                trace_logs.append(trace_log)
            if rsp:
                yield rsp
```

### 4.4 组件执行器

**代码路径**：`core/executor/component/component_runner.py`

```python
class ComponentExecutor(WorkflowRunner):
    """单组件调试执行器"""
    async def run(self, workflow_id, workflow_version, inputs, component_id, ...):
        # 1. 获取工作流
        workflow = Workflow(await _fetch_workflow_dl(...), space_id, current_user)
        
        # 2. 查找目标组件
        target_comp = next(comp for comp in component_list if comp.id == component_id)
        
        # 3. 编译组件
        compiled_comp = await workflow.compile_component(Context(), workflow.dl_workflow, target_comp)
        
        # 4. 执行
        executor = compiled_comp.to_executable()
        data = await executor.invoke(inputs, session, context)
```

---

## 5. 工作流引擎

### 5.1 工作流编译核心

**代码路径**：`core/executor/workflow/workflow.py`

```python
class Workflow:
    # 组件编译器映射表
    COMPILER_HANDLERS = {
        ComponentType.COMPONENT_TYPE_LLM: '_compile_llm_component',
        ComponentType.COMPONENT_TYPE_QUESTION: '_compile_question_component',
        ComponentType.COMPONENT_TYPE_INTENT: '_compile_intent_component',
        ComponentType.COMPONENT_TYPE_CODE: '_compile_code_component',
        ComponentType.COMPONENT_TYPE_HTTP_REQUEST: '_compile_http_request_component',
        ComponentType.COMPONENT_TYPE_REACT_AGENT: '_compile_react_agent_component',
        ComponentType.COMPONENT_TYPE_KNOWLEDGE_RETRIEVAL: '_compile_knowledge_retrieval_component',
        # ...
    }
    
    # 特殊组件（需要异步处理）
    SPECIAL_COMPONENT_TYPES = {
        ComponentType.COMPONENT_TYPE_IF,
        ComponentType.COMPONENT_TYPE_SUB_WORKFLOW,
        ComponentType.COMPONENT_TYPE_LOOP,
        ComponentType.COMPONENT_TYPE_PLUGIN,
    }

    async def compile(self, context, loader) -> InvokableWorkflow:
        card = WorkflowCard(id=self.id, version=self.version, name=self.name, input_params=self.inputs)
        flow = InvokableWorkflow(card=card)
        flow = await self.process_components(context, flow, self.dl_workflow, loader)
        flow = await self.process_stream_connections(flow)
        flow = await self.process_connections(flow, self.dl_workflow.connections)
        return flow
```

### 5.2 Pregel 图算法适配

**代码路径**：`core/executor/workflow/pregel_graph_adapter.py`

```python
class PregelGraphAdapter:
    """将用户定义的 workflow 图转换为适配 Pregel 算法的图"""
    
    def convert(self) -> BaseFlow:
        self._workflow.connections = []
        self._pre_process_graph()   # 为分支节点增加空子节点
        self._validate_graph()      # 检查环
        self._travel_all_nodes()    # 拓扑遍历
        return self._workflow
    
    def _pre_process_graph(self):
        # 为 IF/INTENT 等分支节点的出边插入空节点
        # 为 BREAK/CONTINUE 连接到 END
    
    def _validate_graph(self):
        # 1. 检查孤立起始节点
        # 2. 检查连通性（start → end 可达）
        # 3. 检查环（nx.simple_cycles）
    
    def _travel_all_nodes(self):
        # 拓扑排序遍历，处理分支消减（cba 算法）
```

**设计要点**：
- 使用 NetworkX 的 MultiDiGraph 表示工作流
- **cba（closest branch ancestor）算法**：处理多分支汇合
- 支持循环（Loop）、条件分支（IF）、子工作流（SubWorkflow）

### 5.3 工作流执行管理器

**代码路径**：`core/executor/workflow/workflow_execution_manager.py`

```python
class WorkflowExecutionManager:
    """管理所有进行中的工作流执行"""
    def register_execution(self, registration: WorkflowExecutionRegistration): ...
    def unregister_execution(self, conversation_id: str): ...
    def cancel_execution(self, conversation_id: str) -> bool: ...
    def is_executing(self, conversation_id: str) -> bool: ...
    def is_cancelled(self, conversation_id: str) -> bool: ...
```

---

## 6. DSL 转换

### 6.1 DSL 转换器架构

**代码路径**：`core/dsl_converter/`

```
dsl_converter/
├── converter/
│   ├── converter.py        # 抽象基类 + 工厂
│   ├── converter_native.py # 原生格式转换
│   ├── converter_n8n.py    # n8n 格式转换
│   ├── detector.py         # 格式检测
│   ├── importer.py         # 导入器
│   ├── validator.py        # 校验器
│   ├── reporter.py         # 转换报告
│   └── n8n_mappings.py     # n8n 节点映射
├── components/             # 组件转换
└── experimental/           # 实验性功能
```

### 6.2 抽象转换器

**代码路径**：`core/dsl_converter/converter/converter.py`

```python
class WorkflowConverter(ABC):
    @abstractmethod
    def convert(self, json_data: Dict[str, Any]) -> WorkflowImportResult:
        ...

class ConverterFactory:
    @staticmethod
    def create(format_type: WorkflowFormat) -> WorkflowConverter:
        if format_type == WorkflowFormat.OPENJIUWEN_NATIVE:
            return NativeWorkflowConverter()
        elif format_type == WorkflowFormat.N8N:
            return N8nWorkflowConverter()
```

### 6.3 工作流代码生成器

**代码路径**：`core/manager/workflow_code_generator.py`

```python
class WorkflowCodeGenerator:
    """将 DSL Workflow 转换为可运行的 Python 脚本"""
    
    def generate(self) -> str:
        sections = [
            self._gen_header(),               # 文件头注释
            self._gen_imports(),              # 动态 import
            self._gen_workflow_metadata(),    # 元数据常量
            self._gen_model_config_helper(),  # 模型配置辅助函数
            self._gen_all_component_functions(),  # 每个组件一个函数
            self._gen_build_workflow(),       # 组装工作流
            self._gen_main(),                 # 入口函数
        ]
        return "\n".join(sections)
```

**设计要点**：
- **低代码↔代码双向转换**：可视化编排 → Python 代码，反之亦可
- **动态 import**：根据组件类型按需生成 import 语句
- **模型配置解耦**：API Key 通过环境变量注入

---

## 7. Agent 管理

### 7.1 Agent 数据模型

**代码路径**：`models/agent.py`

```python
class AgentDBMixin:
    space_id: str                    # 工作空间 ID（多租户）
    agent_name: str                  # 名称
    agent_type: str                  # 类型
    edit_mode: str                   # 编辑模式
    prompt_template: List[Dict]      # 提示词模板
    configs: Dict                    # 配置
    plugins: List[Dict]              # 关联插件
    workflows: List[Dict]            # 关联工作流
    model_id: int                    # 模型 ID
    agent_model_config: Dict         # 模型参数
    prompt_tuning: Dict              # 提示词调优
    triggers: List[str]              # 触发器
    knowledge: List[str]             # 知识库
    memory: Dict                     # 记忆配置
    constraint: Dict                 # 约束配置

class AgentBaseDB(AgentDBMixin, Base, DBFunBase):
    __tablename__ = "agent"
    agent_id: str                    # Agent ID
    agent_version: str               # 版本（draft 表示草稿）
    latest_publish_version: str      # 最新发布版本
    
    # 关系
    agent_executions                 # 执行日志
    prompts                          # 关联提示词
    agent_publish_list               # 发布版本列表
    agent_workflow_relations         # 工作流关联
```

### 7.2 Agent 管理器

**代码路径**：`core/manager/agent.py`

**核心功能**：
- CRUD：`agent_create` / `agent_get` / `agent_update` / `agent_delete`
- 版本管理：`agent_publish` / `agent_list_versions`
- 导入导出：`agent_export` / `agent_import`（含依赖检查）
- 引用检查：`extract_agent_references` / `check_referenced_dependencies`

```python
@with_exception_handling
def agent_create(req: AgentCreate, current_user: dict) -> ResponseModel:
    # 1. 权限校验
    # 2. 名称唯一性检查
    # 3. 创建 Agent 记录
    # 4. 处理关联的工作流/插件/知识库引用
    # 5. 返回 AgentId
```

### 7.3 Agent DL 适配

**代码路径**：`core/executor/agent/agent_dl_adapter.py`

```python
class AgentDlAdapter:
    @staticmethod
    def convert_to_agent_config(agent_dl_json: str) -> ReActAgentConfig:
        # 将 JSON DL 配置转换为 ReActAgentConfig / WorkflowAgentConfig
    
    @staticmethod
    def get_knowledge_config(agent_dl_json: str) -> Tuple[List[str], RetrievalConfig]:
        # 提取知识库 ID 和检索配置
```

---

## 8. 插件系统

### 8.1 插件类型

**代码路径**：`schemas/plugin.py`

```python
class PluginType(IntEnum):
    PLUGIN_TYPE_CLOUD_RESTFUL = 1    # RESTful API 插件
    PLUGIN_TYPE_CLOUD_CODE = 2       # 代码插件
    PLUGIN_TYPE_CLOUD_MCP = 3        # MCP 插件

class PluginMcpTransport(IntEnum):
    PLUGIN_MCP_TRANSPORT_STDIO = 1          # stdio
    PLUGIN_MCP_TRANSPORT_SSE = 2            # SSE
    PLUGIN_MCP_TRANSPORT_STREAMABLE_HTTP = 3 # Streamable HTTP
    PLUGIN_MCP_TRANSPORT_OPENAPI = 4        # OpenAPI
    PLUGIN_MCP_TRANSPORT_PLAYWRIGHT = 5     # Playwright
```

### 8.2 插件管理器

**代码路径**：`core/manager/plugin.py`

**核心功能**：
- `plugin_create`：创建插件（支持 RESTful/Code/MCP）
- `plugin_discover_mcp_tools`：连接 MCP Server 发现工具
- `plugin_get` / `plugin_update` / `plugin_delete`
- `plugin_publish` / `plugin_list_versions`

```python
async def _discover_and_create_mcp_tools(config, plugin_id, space_id, current_user):
    # 1. 根据传输类型创建客户端
    if mcp_transport_enum == PluginMcpTransport.PLUGIN_MCP_TRANSPORT_STDIO:
        client = StdioClient(server_config)
    elif mcp_transport_enum == PluginMcpTransport.PLUGIN_MCP_TRANSPORT_SSE:
        client = SseClient(server_config)
    # ...
    
    # 2. 连接并发现工具
    await client.connect()
    tool_cards = await client.list_tools()
    
    # 3. 持久化到数据库
    for card in tool_cards:
        plugin_create_mcp_tool(mcp_req, current_user)
```

### 8.3 插件工具

**代码路径**：`core/executor/plugin/plugin_tools.py`

```python
class ServiceTool:
    """RESTful API 工具"""
    def compile(self) -> RestfulApi:
        # 从 DlRestfulApiSchema 编译为 RestfulApi

class CodeTool:
    """代码插件工具"""
    def compile(self):
        # 从 PluginCodeConfig 编译

class McpTool:
    """MCP 工具"""
    def compile(self):
        # 从 McpConfig 编译
```

### 8.4 插件服务

**代码路径**：`plugin_server/openjiuwen_plugin_server/`

```python
# 独立运行的插件 RESTful 服务
app = FastAPI(title="Plugin Server")
app.include_router(system_router.router, prefix="/system")
app.include_router(demo_router.router, prefix="/demo")
```

### 8.5 Marketplace

**代码路径**：`marketplace/`

```
marketplace/
├── ready_plugins/        # 预置插件
│   ├── entertainment/    # 娱乐
│   ├── productivity/     # 生产力
│   ├── data/             # 数据
│   ├── ecommerce/        # 电商
│   ├── finance/          # 金融
│   ├── developer/        # 开发
│   └── ...
├── plugins_creator/      # 插件创建器
│   └── from_swagger/     # 从 Swagger 创建
└── benchmarks/           # 基准测试
```

---

## 9. 知识库与 RAG

### 9.1 知识库管理器

**代码路径**：`core/manager/knowledge_base.py`

**核心功能**：
- `knowledge_base_create`：创建知识库
- `knowledge_base_delete`：删除知识库（含向量库清理）
- `knowledge_base_update`：更新知识库
- 文档上传/处理/索引
- WebLink 抓取

```python
@with_exception_handling
def knowledge_base_create(req: KnowledgeBaseCreate, current_user: dict):
    # 1. 权限校验
    # 2. 名称唯一性检查
    # 3. Embedding 模型校验
    # 4. 索引连接检查
    # 5. 生成 KB ID（UUID hex）
    # 6. 保存到数据库
```

### 9.2 RAG 检索

**代码路径**：`core/executor/agent/agent_runner.py`（集成部分）

```python
# 知识库检索配置
@dataclass
class RetrievalConfig:
    topk: int = 5
    use_graph: bool = False      # 图检索
    graph_expansion: bool = False # 图扩展
    agentic: bool = False         # Agentic 检索
    score_threshold: float = 0.0

# 多知识库检索
kb_results = await retrieve_multi_kb(kbs=kb_instances, query=query, config=config)
```

### 9.3 向量存储

**代码路径**：`openjiuwen.core.retrieval`（外部包）

```python
# 支持的向量库
from openjiuwen.core.retrieval.vector_store.milvus_store import MilvusVectorStore
from openjiuwen.core.retrieval.vector_store.chroma_store import ChromaVectorStore

# 索引器
from openjiuwen.core.retrieval.indexing.indexer.milvus_indexer import MilvusIndexer
from openjiuwen.core.retrieval.indexing.indexer.chroma_indexer import ChromaIndexer
```

### 9.4 文档处理流水线

```
文档上传 → 解析（AutoFileParser） → 分块（TextChunker） → 向量化（OpenAIEmbedding） → 索引
```

**设计要点**：
- 支持多知识库联合检索
- 图检索（GraphRAG）支持三元组提取（TripleExtractor）
- 文档状态机：UPLOADED → PARSING → CHUNKING → INDEXING → INDEXED

---

## 10. 记忆管理

### 10.1 记忆管理器

**代码路径**：`core/manager/memory.py`

```python
@with_exception_handling
async def get_longterm_mem(req: SearchLongtermMem):
    memory_engine = get_memory_engine()
    memory_data = await memory_engine.get_user_mem_by_page(
        user_id=req.user_id,
        scope_id=req.group_id,
        page_size=req.num,
        page_idx=req.page,
        memory_type=safe_get_memory_type(req.memory_type)
    )
    return {"longterm_mem_data": memory_data}

@with_exception_handling
async def get_user_variable(req: GetUserVar):
    memory_data = await memory_engine.get_variables(
        user_id=req.user_id, scope_id=req.group_id, names=req.names
    )
```

### 10.2 记忆引擎

**代码路径**：`memory_engine_start.py`

```python
class MemoryEngineManager:
    """记忆引擎单例管理"""
    _instance = None
    
    @classmethod
    async def init(cls):
        cls._instance = MemoryEngine(...)
    
    @classmethod
    def get_instance(cls):
        return cls._instance
```

### 10.3 记忆类型

```python
class MemoryType(Enum):
    UNKNOWN = "unknown"
    LONGTERM = "longterm"   # 长期记忆
    VARIABLE = "variable"   # 变量
    SUMMARY = "summary"     # 摘要
```

**设计要点**：
- 长期记忆按用户+作用域（group_id）隔离
- 支持分页查询
- 记忆引擎独立初始化，支持多种后端

---

## 11. 评估体系

### 11.1 评估编排器

**代码路径**：`core/executor/evaluation/evaluation_harness.py`

```python
class EvaluationHarness:
    def __init__(self):
        self._grader = GraderEngine()
        self._pattern_validator = PatternValidator()
        self._perturbation_coordinator = PerturbationCoordinator()
        self._safety_grader = SafetyGrader()

    async def execute_evaluation(self, config: EvaluationRunConfig):
        # 1. 加载评估任务
        tasks = evaluation_task_repository.list_by_evaluation(config.evaluation_id)
        
        # 2. 执行每个任务
        for task in tasks:
            await self._execute_task(task_config)
        
        # 3. 计算聚合指标
        metrics = compute_aggregate_metrics(results, custom_metric_defs)
        
        # 4. 回归检测
        alerts = self._detect_regressions(metrics, prev_metrics, prev_run_id)
        
        # 5. 更新状态
        evaluation_run_repository.update_status(run_id, COMPLETED, metrics)
```

### 11.2 评估执行流程

```
评估运行（Run）
  ├── 任务 1（Task）
  │   ├── Trial 1（nominal）
  │   ├── Trial 2（prompt_perturbed）
  │   ├── Trial 3（env_perturbed）
  │   └── Trial 4（fault_injected）
  ├── 任务 2（Task）
  └── ...
```

### 11.3 评分引擎

**代码路径**：`core/executor/evaluation/grader_engine.py`

```python
class GraderEngine:
    async def run_graders(self, graders_cfg, execution_trace, expected, space_id):
        # 支持多种评分器：
        # - LLM-as-Judge
        # - 精确匹配
        # - 模式匹配
        # - 自定义评分器
```

### 11.4 扰动注入

**代码路径**：`core/executor/evaluation/perturbations.py`

```python
class PerturbationCoordinator:
    """扰动协调器"""
    async def generate_prompt_variants(self, prompt, num_variants):
        # 生成提示词变体
    
    def perturb_environment(self, inputs):
        # 扰动输入数据
    
    def inject_fault(self):
        # 故障注入
```

### 11.5 安全评分

**代码路径**：`core/executor/evaluation/safety_grader.py`

```python
class SafetyGrader:
    async def evaluate(self, output_str, context):
        # 返回 (violations, max_severity)
```

**设计要点**：
- **多 Trial 评估**：支持多次运行取平均
- **扰动测试**：prompt 改写、环境扰动、故障注入
- **回归检测**：与历史运行对比，自动发现性能退化
- **安全评估**：独立的安全评分模块

---

## 12. 多渠道连接

### 12.1 Connect 架构

**代码路径**：`connect/`

```
connect/
├── adapters/
│   ├── channels/           # 渠道适配器
│   │   ├── platforms/      # 平台基类
│   │   │   ├── base.py         # BasePlatform 文档接口
│   │   │   └── command_context.py
│   │   └── run.py          # 渠道启动
│   └── mcp_server/         # MCP 服务器
│       ├── server.py       # MCP Server 入口
│       └── tools/
│           └── registrator.py  # 工具注册
└── client/                 # 外部调用 SDK
    ├── client.py           # OpenJiuwenClient
    ├── auth/               # 认证
    ├── agents/             # Agent 调用
    ├── workflows/          # Workflow 调用
    └── general/            # 通用（health check）
```

### 12.2 MCP Server

**代码路径**：`connect/adapters/mcp_server/server.py`

```python
def main():
    # 1. 解析参数（token、backend-url、transport）
    # 2. 创建 OpenJiuwenClient
    # 3. 创建 FastMCP 实例
    mcp = FastMCP(name="OpenJiuwen", ...)
    
    # 4. 自动选择 space
    resp = get_spaces(client)
    client.set_space_id(space_list[0]['space_id'])
    
    # 5. 注册所有工具
    register_all(mcp, client)
    
    # 6. 启动（stdio 或 sse）
    mcp.run(transport=args.transport)
```

### 12.3 工具注册器

**代码路径**：`connect/adapters/mcp_server/tools/registrator.py`

```python
def register_all(mcp, client):
    # 注册以下 MCP 工具：
    # - health_check
    # - list_agents / search_agents / get_agent / run_agent / reset_agent
    # - list_workflows / search_workflows / get_workflow / run_workflow
```

### 12.4 客户端 SDK

**代码路径**：`connect/client/`

```python
class OpenJiuwenClient:
    def __init__(self, base_url):
        self.base_url = base_url
        self.token = None
        self.space_id = None
    
    def set_token(self, token): ...
    def set_space_id(self, space_id): ...
    
    # 封装所有 API 调用
```

**设计要点**：
- **MCP 双向支持**：既可作为 MCP Server 暴露工具，也可作为 MCP Client 调用外部
- **多传输协议**：stdio / SSE
- **Token 管理**：支持持久化存储、自动刷新
- **Space 隔离**：多租户支持

---

## 13. 沙箱服务

### 13.1 沙箱架构

**代码路径**：`sandbox_server/`

```
sandbox_server/
├── gateway/                    # 沙箱网关
│   └── openjiuwen_sandbox_gateway/
│       └── app/
│           └── gateway.py      # HTTP 网关
└── sandbox/                    # 沙箱执行器
    └── openjiuwen_sandbox_server/
        └── app/
            ├── base.py         # 抽象基类
            ├── bwrap.py        # BubbleWrap 实现
            ├── local.py        # 本地执行
            ├── sandbox.py      # 沙箱工厂
            ├── network_guard.py # 网络防护
            ├── dependency_manager.py  # 依赖管理
            └── util.py         # 工具函数
```

### 13.2 沙箱基类

**代码路径**：`sandbox_server/sandbox/openjiuwen_sandbox_server/app/base.py`

```python
class BaseSandbox(ABC):
    _registry: dict[str, type['BaseSandbox']] = {}
    
    def __init_subclass__(cls, sandbox_type: str | None = None, **kwargs):
        if sandbox_type is not None:
            BaseSandbox._registry[sandbox_type] = cls
    
    @classmethod
    def get_class(cls, sandbox_type: str) -> type['BaseSandbox']:
        return BaseSandbox._registry[sandbox_type]
    
    @abstractmethod
    def run(self, raw_code, base_code, lang, timeout=0, dep_name=None) -> ExecutionResult:
        ...
```

### 13.3 BubbleWrap 实现

**代码路径**：`sandbox_server/sandbox/openjiuwen_sandbox_server/app/bwrap.py`

```python
class BubbleWrapRunner(BaseSandbox, sandbox_type='bwrap'):
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
            return result
    
    def _namespace_params(self):
        # user/ipc/pid/net/uts/cgroup 命名空间隔离
        return [flag for ns, flag in NAMESPACE_FLAGS.items() 
                if self._sandbox_config.namespace.get(ns, False)]
```

### 13.4 安全机制

```python
# 1. 命名空间隔离
NAMESPACE_FLAGS = {
    'user': '--unshare-user',
    'ipc': '--unshare-ipc',
    'pid': '--unshare-pid',
    'net': '--unshare-net',
    'uts': '----unshare-uts',
    'cgroup': '--unshare-cgroup',
}

# 2. Seccomp BPF 系统调用过滤
bpf = pyseccomp.SyscallFilter(pyseccomp.KILL)
for syscall in allowed:
    bpf.add_rule(pyseccomp.ALLOW, syscall)

# 3. 文件系统隔离（只读挂载）
MOUNT_MODES = {'read': '--ro-bind', 'write': '--bind', 'dev': '--dev-bind'}

# 4. 网络隔离
# - 默认禁用外部网络
# - 可选允许内部网络访问
```

### 13.5 网关

**代码路径**：`sandbox_server/gateway/openjiuwen_sandbox_gateway/app/gateway.py`

```python
async def remote_server(lang, code, inputs, session, timeout=10.0):
    payload = {"session": session, "language": lang, "code": code, "timeout": timeout, "inputs": inputs}
    async with httpx.AsyncClient() as cli:
        r = await cli.post(SANDBOX_SERVER_URL, json=payload)
        return r.json()
```

**设计要点**：
- **多层隔离**：命名空间 + seccomp + 文件系统 + 网络
- **可插拔沙箱**：通过注册表机制支持多种沙箱实现
- **依赖管理**：`dependency_manager` 处理代码依赖安装
- **超时控制**：执行超时自动 kill

---

## 14. 前端实现

### 14.1 前端架构

**代码路径**：`frontend/`

```
frontend/
├── src/
│   ├── pages/              # 页面
│   │   ├── Agents/         # Agent 管理
│   │   ├── Workflows/      # 工作流管理
│   │   ├── Plugins/        # 插件管理
│   │   ├── KnowledgeBase/  # 知识库
│   │   ├── Models/         # 模型配置
│   │   ├── Prompts/        # 提示词
│   │   ├── Evaluation/     # 评估
│   │   ├── Triggers/       # 触发器
│   │   ├── MemoryBase/     # 记忆
│   │   ├── Runtime/        # 运行时
│   │   └── Executions/     # 执行日志
│   ├── components/         # 公共组件
│   ├── stores/             # 状态管理
│   ├── hooks/              # 自定义 Hooks
│   ├── contexts/           # React Context
│   ├── i18n/               # 国际化
│   └── locales/            # 多语言
└── packages/
    ├── workflow-canvas/    # 工作流画布（基于 FlowGram.ai）
    ├── api-client/         # API 客户端
    └── base-ui/            # UI 组件库
```

### 14.2 Workflow Canvas

**代码路径**：`frontend/packages/workflow-canvas/`

```
workflow-canvas/src/
├── editor.tsx              # 编辑器主入口
├── nodes/              # 节点定义
├── components/         # 画布组件
├── hooks/              # 画布 Hooks
├── stores/             # 画布状态
├── services/           # 服务（validation、custom）
├── plugins/            # 画布插件
├── shortcuts/          # 快捷键
├── form-components/    # 表单组件
├── i18n/               # 国际化
├── styles/             # 样式
├── typings/            # 类型定义
└── utils/              # 工具函数
```

**编辑器入口**：

```tsx
// editor.tsx
import { FreeLayoutEditorProvider, EditorRenderer } from '@flowgram.ai/free-layout-editor'

const Editor: React.FC = () => {
  return (
    <FreeLayoutEditorProvider>
      <EditorRenderer />
      <Tools />
      <WorkflowOperation />
      <HistoryPanel />
    </FreeLayoutEditorProvider>
  )
}
```

**设计要点**：
- 基于 **FlowGram.ai**（字节开源）的 Free Layout Editor
- 支持节点拖拽、连线、版本切换
- 集成测试运行（testRunRuntimeService）
- 历史版本管理

### 14.3 状态管理

```typescript
// stores/useWorkflowStore.ts
interface WorkflowStore {
  context: WorkflowContext | null;
  selectedVersion: string | null;
  setSelectedVersion: (version: string) => void;
  // ...
}
```

---

## 15. 提示词工程

### 15.1 提示词架构

**代码路径**：`ops/modules/prompt/`

```
ops/modules/prompt/
├── application/            # 应用层
│   ├── service.py          # PromptService（核心服务）
│   ├── debug_service.py    # 调试服务
│   ├── trace_sdk_interface.py  # 追踪 SDK 接口
│   └── exception.py        # 异常定义
├── domain/                 # 领域层
│   ├── entities.py         # 实体定义
│   ├── services.py         # 领域服务
│   ├── repositories.py     # 仓储接口
│   └── debug_entity.py     # 调试实体
└── infra/                  # 基础设施层
    ├── database.py         # 数据库
    └── repositories/       # 仓储实现
        ├── orm_repo.py     # ORM 模型
        ├── job_repo.py     # 任务仓储
        └── agent_repo.py    # Agent 仓储
```

### 15.2 提示词服务

**代码路径**：`ops/modules/prompt/application/service.py`

```python
class PromptService:
    def __init__(self, prompt_repo, prompt_user_draft_repo, prompt_commit_repo, agent_repo):
        self.draft_domain_service = DraftDomainService(...)
        self.commit_domain_service = CommitDomainService(...)
        self.batch_prompt_domain_service = BatchPromptDomainService(...)
        self.get_prompt_detail_service = GetPromptDetailService(...)
    
    def create_prompt(self, new_prompt: CreatePromptRequest) -> CreatePromptResponse:
        # 1. 名称唯一性检查
        # 2. 创建 PromptBasicModel
    
    def get_prompt(self, prompts: dict) -> GetPromptResponse:
        # 从所有关联表中获取 prompt 信息
    
    def list_prompts(self, list_prompt: ListPromptRequest) -> ListPromptResponse:
        # 分页列表查询
    
    def update_prompt(self, new_prompt: UpdatePromptRequest) -> UpdatePromptResponse:
        # 更新基础信息 + 同步更新 Agent 关联
    
    def delete_prompt(self, prompts: DeletePromptRequest) -> DeletePromptResponse:
        # 逻辑删除（设置 deleted_at）
    
    def clone_prompt(self, ori_prompt_id, new_prompt: ClonePromptRequest) -> ClonePromptResponse:
        # 克隆 prompt
```

### 15.3 提示词调优

**代码路径**：`ops/modules/prompt/domain/services.py`

```python
class DraftDomainService:
    """草稿领域服务"""
    ...

class CommitDomainService:
    """提交领域服务"""
    ...

class BatchPromptDomainService:
    """批量提示词服务"""
    ...

class JobDomainService:
    """任务领域服务"""
    ...
```

### 15.4 提示词版本管理

```python
# ORM 模型
class PromptBasicModel:
    prompt_key: str          # 唯一标识
    name: str                # 名称
    description: str         # 描述
    space_id: str            # 工作空间
    created_by: str          # 创建者
    updated_by: str          # 更新者
    deleted_at: datetime     # 删除时间（逻辑删除）

class PromptCommitModel:
    """提交版本"""
    ...

class PromptUserDraftModel:
    """用户草稿"""
    ...
```

**设计要点**：
- **DDD 分层**：application / domain / infra 三层架构
- **草稿+提交**：支持草稿编辑和版本提交
- **批量操作**：批量提示词管理
- **逻辑删除**：软删除机制

---

## 16. 多模型接入

### 16.1 LLM 管理器

**代码路径**：`ops/modules/llm/llm_manager.py`

```python
class LLMConfigService:
    """LLM 配置服务"""
    def get_llm_model_info(self, model_id, source="config"):
        # 从数据库或配置文件获取模型信息

def get_llm_client(model_id: str, source="config") -> Model:
    cfg = _config_service.get_llm_model_info(model_id, source)
    protocol = cfg.get("protocol_config", "")
    model_client_config = ModelClientConfig(
        client_provider=compatible_provider(protocol.get("provider")),
        api_key=protocol.get("api_key", ""),
        api_base=protocol.get("base_url", ""),
        timeout=protocol.get("timeout", 60),
    )
    return Model(model_client_config, ModelRequestConfig())

def get_llm_client_by_protocol(protocol: Dict) -> Model:
    """通过协议配置创建客户端"""
    ...

def get_openai_client(model_id, source="config") -> OpenAI:
    """获取 OpenAI 兼容客户端"""
    ...

def get_async_openai_client(model_id, source="config") -> AsyncOpenAI:
    """获取异步 OpenAI 客户端"""
    ...
```

### 16.2 模型配置

**代码路径**：`models/model_config.py`

```python
class ModelConfig(Base):
    __tablename__ = "model_config"
    id: int
    name: str                # 配置名称
    provider: str            # 提供商（openai/anthropic/...）
    model_type: str          # 模型类型
    base_url: str            # API 地址
    api_key: str             # 加密存储
    timeout: int
    is_active: bool
    space_id: str            # 多租户
```

### 16.3 支持的提供商

```python
# 提供商映射
_CLIENT_PROVIDER_MAP = {
    "siliconflow": "SiliconFlow",
    "openai": "OpenAI",
    "azure": "Azure",
    "ollama": "Ollama",
}
```

**设计要点**：
- **多提供商支持**：OpenAI / Azure / Ollama / SiliconFlow 等
- **API Key 加密**：`SecurityUtils.encrypt_api_key` 加密存储
- **配置热切换**：通过 `is_active` 控制模型启用
- **客户端缓存**：`@lru_cache(maxsize=32)` 缓存客户端实例

---

## 17. 调度器

### 17.1 调度器实现

**代码路径**：`core/scheduler/scheduler.py`

```python
from apscheduler.jobstores.sqlalchemy import SQLAlchemyJobStore
from apscheduler.schedulers.asyncio import AsyncIOScheduler

def init_scheduler(database_url: str) -> AsyncIOScheduler:
    jobstores = {
        "default": SQLAlchemyJobStore(url=database_url, engine_options=engine_options)
    }
    _scheduler = AsyncIOScheduler(
        jobstores=jobstores,
        job_defaults={
            "coalesce": True,             # 错过多次只触发一次
            "max_instances": 1,           # 同一任务不并发
            "misfire_grace_time": 86400,  # 容错时间
        },
        timezone="UTC",
    )
    return _scheduler
```

### 17.2 任务定义

**代码路径**：`core/scheduler/jobs.py`

```python
# 触发器任务
async def trigger_job(trigger_id: str, ...):
    """执行触发器任务"""
    ...
```

### 17.3 同步机制

**代码路径**：`core/scheduler/sync.py`

```python
async def sync_triggers_to_scheduler(scheduler):
    """将数据库中的触发器同步到调度器"""
    # 1. 从数据库加载所有启用的触发器
    # 2. 对比调度器中的任务
    # 3. 添加/更新/删除任务
```

### 17.4 触发器模型

**代码路径**：`models/trigger.py`

```python
class TriggerDB(Base):
    __tablename__ = "trigger"
    trigger_id: str
    name: str
    trigger_type: str        # 触发类型（schedule/webhook/...）
    cron_expression: str     # Cron 表达式
    agent_id: str            # 关联 Agent
    workflow_id: str         # 关联 Workflow
    is_enabled: bool
    space_id: str
```

**设计要点**：
- **持久化存储**：任务存储在数据库，重启不丢失
- **动态同步**：触发器变更自动同步到调度器
- **多类型支持**：Cron 调度、Webhook 触发
- **容错机制**：misfire_grace_time 处理错过执行

---

## 18. 数据库与迁移

### 18.1 数据模型

**代码路径**：`models/`

```
models/
├── agent.py                # Agent 模型
├── workflow.py             # 工作流模型
├── plugin.py               # 插件模型
├── knowledge_base.py       # 知识库模型
├── knowledge_base_document.py  # 知识库文档
├── model_config.py         # 模型配置
├── embedding_model_config.py   # Embedding 模型配置
├── user.py                 # 用户模型
├── space.py                # 工作空间模型
├── trigger.py              # 触发器模型
├── evaluation.py           # 评估模型
├── memory_base.py          # 记忆模型
├── tag.py                  # 标签模型
├── trace_detail.py         # 追踪详情
├── trace_summary.py        # 追踪摘要
├── prompt_relation.py      # 提示词关系
├── reference.py            # 引用关系
├── db_fun_base.py          # 数据库基类
└── ...
```

### 18.2 数据库基类

**代码路径**：`models/db_fun_base.py`

```python
class DBFunBase:
    __version_none__ = "draft"
    
    @classmethod
    def sqlalchemy_to_pydantic(cls, exclude=None):
        # SQLAlchemy 模型 → Pydantic 模型转换

class Base(DeclarativeBase):
    pass
```

### 18.3 Alembic 迁移

**代码路径**：`upgrade/`

```
upgrade/
├── mysql/
│   ├── alembic_agent/      # Agent 库迁移
│   │   └── versions/       # 迁移版本
│   └── alembic_ops/        # Ops 库迁移
│       └── versions/
└── sqlite/
    ├── alembic_agent/
    └── alembic_ops/
```

### 18.4 核心表结构

```sql
-- Agent 表
CREATE TABLE agent (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id VARCHAR(100) UNIQUE NOT NULL,
    agent_version VARCHAR(100) DEFAULT 'draft',
    space_id VARCHAR(100),
    agent_name VARCHAR(255),
    prompt_template JSON,
    plugins JSON,
    workflows JSON,
    knowledge JSON,
    memory JSON,
    configs JSON,
    create_time BIGINT,
    update_time BIGINT
);

-- 工作流表
CREATE TABLE workflow (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workflow_id VARCHAR(100) UNIQUE NOT NULL,
    workflow_version VARCHAR(100) DEFAULT 'draft',
    space_id VARCHAR(100),
    workflow_name VARCHAR(255),
    schema TEXT,              -- JSON schema
    input_parameters JSON,
    output_parameters JSON
);

-- 插件表
CREATE TABLE plugin (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    plugin_id VARCHAR(100) UNIQUE NOT NULL,
    plugin_version VARCHAR(100),
    space_id VARCHAR(100),
    name VARCHAR(255),
    url VARCHAR(512),
    plugin_type INTEGER,
    auth JSON,                -- 加密存储
    inputs JSON
);

-- 知识库表
CREATE TABLE knowledge_base (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kb_id VARCHAR(100) UNIQUE NOT NULL,
    space_id VARCHAR(100),
    name VARCHAR(255),
    embedding_model_config_id INTEGER,
    index_manager_type VARCHAR(50),
    config JSON
);
```

### 18.5 多库支持

```python
# ops/dependencies.py
def get_db_ops():
    """获取 ops 数据库会话"""
    ...

def get_db_agent():
    """获取 agent 数据库会话"""
    ...

def get_db_jw():
    """获取 Jiuwen 基础库会话"""
    ...
```

**设计要点**：
- **双库分离**：agent 库 + ops 库（提示词/执行日志）
- **多租户**：所有核心表都有 `space_id` 字段
- **版本管理**：draft + 发布版本机制
- **JSON 字段**：大量使用 JSON 字段存储配置（灵活但牺牲部分查询性能）

---

## 19. 对 laew 工程的借鉴建议

### 19.1 架构设计借鉴

| 特性 | agent-studio 实现 | laew 现状 | 借鉴建议 |
|------|-------------------|-----------|----------|
| **分层架构** | routers → manager → executor → models | agent → tools → llm | 可借鉴 manager 层做业务逻辑抽象 |
| **执行引擎** | 独立的 Agent/Workflow/Plugin/Evaluation 执行器 | 单一 Agent 循环 | 可引入 Workflow 编排层 |
| **组件化** | 20+ 种组件类型 | 3 个工具（Bash/Read/Write） | 可设计可扩展的组件注册机制 |
| **流式处理** | AsyncGenerator 流式返回 | 支持流式 | 可借鉴 chunk 追踪机制 |

### 19.2 工作流引擎借鉴

**agent-studio 的 Pregel 图算法**：
- 使用 NetworkX 的 MultiDiGraph 表示工作流
- cba（closest branch ancestor）算法处理分支汇合
- 支持条件分支、循环、子工作流

**对 laew 的建议**：
1. **引入 Workflow 编排**：将复杂任务分解为可编排的工作流
2. **组件注册机制**：设计 `COMPILER_HANDLERS` 类似的组件注册表
3. **图执行引擎**：支持 DAG 执行、并行分支、条件跳转

```python
# 可借鉴的组件注册模式
COMPILER_HANDLERS = {
    ComponentType.LLM: '_compile_llm_component',
    ComponentType.CODE: '_compile_code_component',
    ComponentType.HTTP_REQUEST: '_compile_http_request_component',
    # 新增组件只需添加映射 + 实现编译方法
}
```

### 19.3 知识库与 RAG 借鉴

**agent-studio 的 RAG 集成**：
- 执行前自动检索知识库
- 检索结果作为 system message 注入 prompt
- 支持多知识库联合检索

**对 laew 的建议**：
1. **知识库工具**：新增 `KnowledgeBaseTool`，支持向量检索
2. **自动注入**：Yolo 分类为需要知识的任务时，自动检索相关知识
3. **多知识库**：支持按任务类型选择不同知识库

### 19.4 评估体系借鉴

**agent-studio 的评估机制**：
- 多 Trial 评估取平均
- 扰动测试（prompt 改写、环境扰动、故障注入）
- 回归检测（与历史运行对比）

**对 laew 的建议**：
1. **质量评估 Agent**：在 Quality-Check 中引入 LLM-as-Judge
2. **回归检测**：对比历史执行结果，发现性能退化
3. **扰动测试**：对关键任务进行鲁棒性测试

### 19.5 插件系统借鉴

**agent-studio 的插件体系**：
- 三种插件类型：RESTful / Code / MCP
- MCP 支持 5 种传输协议
- 插件市场（Marketplace）

**对 laew 的建议**：
1. **MCP 支持**：作为 MCP Client 接入外部工具
2. **插件注册**：设计可扩展的工具注册机制
3. **工具发现**：支持从 OpenAPI/Swagger 自动生成工具

### 19.6 沙箱安全借鉴

**agent-studio 的沙箱**：
- BubbleWrap 命名空间隔离
- Seccomp BPF 系统调用过滤
- 网络隔离

**对 laew 的建议**：
1. **代码执行沙箱**：Code 组件在沙箱中执行
2. **权限分级**：不同任务级别使用不同沙箱策略
3. **资源限制**：CPU/内存/网络限制

### 19.7 多渠道连接借鉴

**agent-studio 的 Connect 模块**：
- MCP Server 暴露工具
- 支持 stdio/SSE 传输
- Token 管理

**对 laew 的建议**：
1. **MCP Server 模式**：将 laew 的能力暴露为 MCP Server
2. **多渠道接入**：支持 Slack/Discord 等渠道
3. **SDK 提供**：为外部调用提供客户端 SDK

### 19.8 记忆管理借鉴

**agent-studio 的记忆系统**：
- 长期记忆按用户+作用域隔离
- 支持分页查询
- 记忆引擎独立初始化

**对 laew 的建议**：
1. **分层记忆**：会话记忆 / 用户记忆 / 全局记忆
2. **记忆检索**：基于相关性的记忆检索
3. **记忆压缩**：长期记忆自动摘要压缩

### 19.9 提示词工程借鉴

**agent-studio 的提示词体系**：
- DDD 分层（application/domain/infra）
- 草稿+提交版本机制
- 批量操作

**对 laew 的建议**：
1. **提示词版本**：支持提示词的版本管理
2. **提示词优化**：集成自动优化功能
3. **提示词调试**：支持 A/B 测试

### 19.10 调度器借鉴

**agent-studio 的调度器**：
- APScheduler 实现
- 数据库持久化
- Cron/Webhook 触发

**对 laew 的建议**：
1. **定时任务**：支持定时执行 Agent 任务
2. **Cron 表达式**：灵活的调度配置
3. **任务队列**：异步任务执行

### 19.11 具体实施路线图

#### P0（核心能力）
1. **组件注册机制**：设计可扩展的组件注册表
2. **Workflow 编排**：支持 DAG 任务编排
3. **MCP 支持**：作为 MCP Client 接入外部工具

#### P1（增强能力）
1. **知识库集成**：向量检索 + 自动注入
2. **评估体系**：LLM-as-Judge + 回归检测
3. **沙箱安全**：代码执行隔离

#### P2（高级能力）
1. **多渠道接入**：MCP Server / Slack / Discord
2. **记忆系统**：分层记忆 + 自动压缩
3. **调度器**：定时任务 + 任务队列

---

## 附录：核心类与函数速查表

| 模块 | 核心类/函数 | 代码路径 |
|------|-------------|----------|
| 入口 | `lifespan_func`, `main` | `main.py` |
| Agent 执行 | `AgentRunner.run` | `core/executor/agent/agent_runner.py` |
| Workflow 执行 | `WorkflowRunner.run` | `core/executor/workflow/workflow_runner.py` |
| Workflow 编译 | `Workflow.compile` | `core/executor/workflow/workflow.py` |
| 图算法 | `PregelGraphAdapter.convert` | `core/executor/workflow/pregel_graph_adapter.py` |
| 组件编译 | `Workflow.compile_component` | `core/executor/workflow/workflow.py` |
| Agent 管理 | `agent_create`, `agent_update` | `core/manager/agent.py` |
| Workflow 管理 | `workflow_create`, `workflow_update` | `core/manager/workflow.py` |
| 插件管理 | `plugin_create`, `plugin_discover_mcp_tools` | `core/manager/plugin.py` |
| 知识库 | `knowledge_base_create`, `retrieve_multi_kb` | `core/manager/knowledge_base.py` |
| 记忆 | `get_longterm_mem`, `get_user_variable` | `core/manager/memory.py` |
| 评估 | `EvaluationHarness.execute_evaluation` | `core/executor/evaluation/evaluation_harness.py` |
| DSL 转换 | `ConverterFactory.create` | `core/dsl_converter/converter/converter.py` |
| 代码生成 | `WorkflowCodeGenerator.generate` | `core/manager/workflow_code_generator.py` |
| MCP Server | `main`, `register_all` | `connect/adapters/mcp_server/server.py` |
| 沙箱 | `BubbleWrapRunner.run` | `sandbox_server/sandbox/.../bwrap.py` |
| 调度器 | `init_scheduler`, `sync_triggers_to_scheduler` | `core/scheduler/scheduler.py` |
| LLM 管理 | `get_llm_client`, `get_llm_client_by_protocol` | `ops/modules/llm/llm_manager.py` |
| 提示词 | `PromptService.create_prompt` | `ops/modules/prompt/application/service.py` |

---

> **总结**：openJiuwen Studio 是一个功能完备的企业级 AI Agent 开发平台，其分层架构、组件化设计、工作流引擎、插件系统、评估体系等都值得 laew 工程借鉴。特别是在工作流编排、知识库集成、评估体系、沙箱安全等方面，agent-studio 提供了成熟的实现方案。
>
> **建议优先级**：Workflow 编排 > 组件注册 > MCP 支持 > 知识库集成 > 评估体系 > 沙箱安全 > 多渠道接入
