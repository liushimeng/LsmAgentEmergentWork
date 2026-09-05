# openJiuwen Studio 核心机制深度分析

> **分析日期**：2026-09-05
> **分析目标**：`/usr/local/LsmGitOpenSource/agent-studio`（openJiuwen Studio）
> **前置依据**：
> - `agent-studio-源码调研.md`（第一轮源码调研）
> - `agent-studio-深度分析.md`（第二轮深度分析）
>
> 本报告在两轮调研基础上，进入**核心机制层面**——聚焦真实代码路径、关键函数签名、代码片段、算法细节与设计模式，并给出对 laew 工程的 P0/P1/P2 借鉴路线图。
>
> **注意**：本文档所有代码路径均来自真实源码（已实际 Read 验证），代码片段保留 Python 原貌，不做改写。

---

## 目录

1. [AgentRunner 核心代码路径](#1-agentrunner-核心代码路径)
2. [WorkflowRunner 核心代码路径](#2-workflowrunner-核心代码路径)
3. [DSL 转换器核心代码路径](#3-dsl-转换器核心代码路径)
4. [MCP 插件核心代码路径](#4-mcp-插件核心代码路径)
5. [沙箱核心代码路径](#5-沙箱核心代码路径)
6. [评估体系核心代码路径](#6-评估体系核心代码路径)
7. [组件注册表核心代码路径](#7-组件注册表核心代码路径)
8. [对 laew 的核心机制借鉴路线图](#8-对-laew-的核心机制借鉴路线图)

---

## 1. AgentRunner 核心代码路径

**代码路径**：`backend/openjiuwen_studio/core/executor/agent/agent_runner.py`（795 行）

### 1.1 入口函数 `AgentRunner.run()`

```python
class AgentRunner:
    def __init__(
            self,
            flow_mgr: WorkflowRunner,
            plugin_mgr: PluginManager
    ) -> None:
        self.flow_mgr = flow_mgr
        self.plugin_mgr = plugin_mgr
        # Agent实例缓存：{user_id: {agent_key: (config, instance)}}
        self._agent_instances: Dict[str, Dict[str, Any]] = {}
```

**核心循环**（第 559-788 行）：

```python
async def run(
        self,
        id: str,
        version: str,
        inputs: Any,
        conversation_id: str,
        space_id: str,
        current_user: Dict[str, Any]
) -> AsyncGenerator[Any, None]:
    # 1. 参数验证 - 确保输入包含必要的conversation_id
    if isinstance(inputs, InteractiveInput):
        inputs = {"conversation_id": conversation_id, "query": inputs}
    elif "conversation_id" not in inputs:
        raise BaseError(StatusCode.AGENT_MISSING_CONVERSATION_ID.code,
                                  StatusCode.AGENT_MISSING_CONVERSATION_ID.errmsg)

    # 2. 获取Agent配置
    agent_dl_json = await _fetch_agent_dl(id, version, space_id, current_user)
    agent_config = AgentDlAdapter.convert_to_agent_config(agent_dl_json)

    # 2.1 使用知识库检索（如果配置了知识库）
    kb_ids, retrieval_config = AgentDlAdapter.get_knowledge_config(agent_dl_json)
    if kb_ids:
        # ... 调用 retrieve_multi_kb，将结果注入 prompt_template
        agent_config.prompt_template.append({"role": "system", "content": result_text})

    # 3. 获取Agent实例（带缓存机制）
    invokable_agent: InvokableAgent = await self.get_agent_instance(
        conversation_id, id, version, agent_config, space_id, current_user
    )

    # 4. 创建组件id-name映射表
    mapping = await self._create_mapping_table(agent_config, space_id)

    # 5. 初始化追踪上下文
    trace_context = initialize_trace_context(space_id, id, version, mapping)

    try:
        # 6. 执行Agent流式推理
        inputs["user_id"] = space_id
        inputs["group_id"] = agent_config.id

        async for chunk in Runner.run_agent_streaming(
            agent=invokable_agent,
            inputs=inputs,
            session=conversation_id
        ):
            rsp = await process_chunk_trace(chunk, trace_context)
            if rsp:
                yield rsp

        await finalize_trace(trace_context)
    except (BaseError, JiuWenGraphException) as e:
        await handle_trace_error(trace_context, e.code, e.message)
        raise
    except Exception as e:
        await handle_trace_error(trace_context, -1, str(e))
        raise
```

**设计要点**：
- **6 步执行流水线**：参数验证 → 配置获取 → KB 检索 → 实例缓存 → 映射表 → 流式执行
- **异常分级处理**：`BaseError/JiuWenGraphException` 用业务码；`Exception` 用 `-1`；`CancelledError` 用 `-2`
- **KB 检索前移**：在调用 LLM 前完成，结果直接注入 system message（与 laew 的 SessionContext 摘要注入模式相似）

### 1.2 三维缓存 `get_agent_instance()`

```python
async def get_agent_instance(
        self,
        user_id: str,
        agent_id: str,
        agent_version: str,
        agent_config: Union[ReActAgentConfig, WorkflowAgentConfig],
        space_id: str,
        current_user: Dict[str, Any]
) -> InvokableAgent:
    # 初始化用户的缓存空间
    if user_id not in self._agent_instances:
        self._agent_instances[user_id] = {}

    agent_key = generate_agent_key(agent_id, agent_version)  # "{agent_id}_{agent_version}"

    if agent_key not in self._agent_instances[user_id]:
        self._agent_instances[user_id][agent_key] = ("", None)

    # 检查缓存 - 使用 JSON 序列化来比较配置
    (cache_config_json, catch_instance) = self._agent_instances[user_id][agent_key]
    try:
        current_config_json = agent_config.model_dump_json() if hasattr(agent_config, 'model_dump_json') else ""
    except Exception as e:
        logger.warning(f"Failed to serialize agent_config for cache comparison: {e}")
        current_config_json = ""

    if cache_config_json == current_config_json and catch_instance is not None:
        return catch_instance  # 配置未变更，直接返回缓存实例

    # 配置已变更或首次创建：清理旧实例的工作流
    if (catch_instance
            and hasattr(catch_instance, 'agent_config')
            and hasattr(catch_instance.agent_config, 'workflows')):
        old_workflows = [(w.id, w.version) for w in catch_instance.agent_config.workflows]
        if old_workflows:
            catch_instance.remove_workflows(old_workflows)

    # 重新编译Agent
    invokable_agent = await self.create_new_agent(agent_config, space_id, current_user)
    if catch_instance:
        invokable_agent._context_engine._context_pool = catch_instance._context_engine._context_pool

    # 更新缓存（存储 JSON 字符串用于比较）
    self._agent_instances[user_id][agent_key] = (current_config_json, invokable_agent)
    return invokable_agent
```

**三维缓存机制**：
- **第一维**：`user_id`（多租户隔离，对应 `conversation_id` 作为当前会话的唯一标识）
- **第二维**：`agent_key = "{agent_id}_{agent_version}"`（草稿/发布版本）
- **第三维**：JSON 序列化比较触发重建

**JSON 序列化比较的精髓**：
- **为什么用 JSON 而不是直接对象比较？** Pydantic v2 模型对象的 `__eq__` 对未识别字段不稳定，且无法感知子字段顺序变化。`model_dump_json()` 输出标准化的 JSON 字符串，逐字节比较可精确检测配置变更。
- **Context 复用**：重建时保留 `_context_engine._context_pool`，会话上下文不丢。
- **工作流清理**：调用 `remove_workflows(old_workflows)` 释放旧工作流绑定的组件（避免内存泄漏）。

### 1.3 缓存全清 `clear_agent_cache_for_all_conversations()`

```python
def clear_agent_cache_for_all_conversations(
        self,
        agent_id: str,
        agent_version: str = "draft"
) -> int:
    agent_key = f"{agent_id}_{agent_version}"
    cleared_count = 0

    for conversation_id in list(self._agent_instances.keys()):
        if agent_key in self._agent_instances[conversation_id]:
            try:
                (_, catch_instance) = self._agent_instances[conversation_id][agent_key]
                if hasattr(catch_instance, 'agent_config') and hasattr(catch_instance.agent_config, 'workflows'):
                    old_workflows = [(w.id, w.version) for w in catch_instance.agent_config.workflows]
                    if old_workflows:
                        catch_instance.remove_workflows(old_workflows)
                del self._agent_instances[conversation_id][agent_key]
                cleared_count += 1
            except Exception as e:
                logger.error(f"[AGENT_CACHE_CLEAR] Failed to clear agent cache for conversation {conversation_id}: {e}")

    return cleared_count
```

**应用场景**：在 Agent 配置变更（提示词/工具/工作流引用修改）后调用，清空所有会话的旧缓存。

### 1.4 全局单例

```python
# 全局Agent管理器实例
plugin_manager = PluginManager()
agent_mgr = AgentRunner(WorkflowRunner(plugin_manager), plugin_manager)
```

进程级单例，注入式依赖（`flow_mgr` + `plugin_mgr`）。

### 1.5 对 laew 借鉴

| 借鉴点 | laew 现状 | 建议实现 |
|--------|-----------|----------|
| **JSON 序列化缓存比较** | 无 Agent 缓存机制，每次都新编译 | 在 `MultiAgentOrchestrator` 中加入 `_agent_cache: HashMap<(user_id, agent_key), (config_json, instance)>`；Rust 用 `serde_json::to_string` |
| **KB 检索注入 prompt** | Yolo 注入 `<<<LAEW:SESSION_HISTORY>>>` 历史摘要 | 扩展为多 KB 联合检索（向量 + 图检索），结果以 `<<<LAEW:KNOWLEDGE>>>` 注入 |
| **实例重建保留 Context** | `agent_context` 是瞬时态，每次重建从 0 开始 | 在 `AgentContext` 上实现 `clone_from()`，重建时复用 `_context_pool` |

---

## 2. WorkflowRunner 核心代码路径

**代码路径**：
- `backend/openjiuwen_studio/core/executor/workflow/workflow_runner.py`（409 行）
- `backend/openjiuwen_studio/core/executor/workflow/pregel_graph_adapter.py`（361 行）
- `backend/openjiuwen_studio/core/executor/workflow/workflow_execution_manager.py`（164 行）

### 2.1 `WorkflowRunner.run()` 主循环

```python
class WorkflowRunner(IWorkflowLoader):
    def __init__(self, plugin_mgr=None) -> None:
        self.plugin_mgr = plugin_mgr

    async def run(
            self,
            id: str,
            version: str,
            inputs: Any,
            conversation_id: str,
            space_id: str,
            current_user: Dict[str, Any],
    ) -> AsyncGenerator[Any, None]:

        # 检查当前 conversation_id 是否处于执行状态
        if workflow_execution_manager.is_executing(conversation_id):
            all_execution_infos = workflow_execution_manager.list_executions()
            cancel_success = await workflow_execution_manager.cancel_execution(conversation_id)
            if not cancel_success:
                raise JiuWenExecuteException(
                    code=StatusCode.WORKFLOW_EXECUTION_CONFLICT_ERROR.code,
                    message=StatusCode.WORKFLOW_EXECUTION_CONFLICT_ERROR.errmsg.format(msg=conversation_id),
                    workflow_id=id,
                )

        # ... 收集 trace_logs
        try:
            # 编译工作流
            flow = await self.get_compiled_workflow(Context(), id, version, space_id, current_user)
            # 创建 Session 用于 workflow 执行, 使用 conversation_id 作为 session_id
            session = create_workflow_session(session_id=conversation_id)

            task = asyncio.current_task()
            registration = WorkflowExecutionRegistration(
                conversation_id=conversation_id,
                workflow_id=id,
                workflow_version=version,
                space_id=space_id,
                session=session,
                task=task
            )
            workflow_execution_manager.register_execution(registration)

            # 共享 context：两次调用复用同一实例
            context = SessionModelContext(
                context_id=f"{id}_{version}",
                session_id=conversation_id,
                config=ContextEngineConfig(),
                history_messages=[],
                processors=[],
            )

            # 使用 Runner.run_workflow_streaming() 执行工作流
            async for chunk in Runner.run_workflow_streaming(
                    workflow=flow,
                    inputs=inputs,
                    session=session,
                    context=context,
            ):
                # 双层取消检测
                if task and task.cancelled():
                    break
                if workflow_execution_manager.is_cancelled(conversation_id):
                    break

                # 过滤 trace chunk
                if isinstance(chunk, TraceSchema) and chunk.type == "tracer_workflow":
                    wf = TraceWorkflowSpan.model_validate(chunk.payload)
                    if hasattr(wf, 'workflow_id') and wf.invoke_id == wf.workflow_id:
                        continue

                rsp, trace_log, _ = result_convert(chunk, business_type="WORKFLOW", mapping=component_name_map)
                if trace_log:
                    trace_logs.append(trace_log)
                if rsp:
                    yield rsp

            if trace_logs:
                await save_execution_traces(flow_index, trace_logs)
            trace_id = trace_logs[0].trace_id if trace_logs else None
            if trace_id is not None:
                trace_summary_repository.create_trace_summary_from_workflow_execution(trace_id)
        except asyncio.CancelledError:
            pass
        finally:
            workflow_execution_manager.unregister_execution(conversation_id)
```

**核心设计**：
1. **冲突检测**：`is_executing` → `cancel_execution` → 重新启动（同一会话取消旧执行）
2. **双层取消检测**：`task.cancelled()` + `is_cancelled(conversation_id)` 双重保险
3. **trace 过滤**：跳过 `invoke_id == workflow_id` 的 chunk（避免重复）
4. **finally 清理**：`unregister_execution` 确保管理器不留垃圾

### 2.2 `WorkflowExecutionManager` 冲突检测 + 取消

```python
class WorkflowExecutionManager:
    def __init__(self):
        self._executions: Dict[str, WorkflowExecutionInfo] = {}
        self._lock = threading.Lock()
        self._cancelled_flags: Dict[str, bool] = {}

    def is_executing(self, conversation_id: str) -> bool:
        with self._lock:
            execution_info = self._executions.get(conversation_id)
            if not execution_info:
                return False
            if execution_info.task:
                return not execution_info.task.done()
            return True

    async def cancel_execution(self, conversation_id: str) -> bool:
        execution_info = self.get_execution(conversation_id)
        if not execution_info:
            return False
        try:
            # 1. 设置取消标志（优先执行，让流式输出能快速响应）
            with self._lock:
                self._cancelled_flags[conversation_id] = True

            # 2. 取消异步任务
            if execution_info.session and execution_info.task and not execution_info.task.done():
                execution_info.task.cancel()
                try:
                    await execution_info.task
                except asyncio.CancelledError:
                    pass

            # 3. 从注册表中移除
            self.unregister_execution(conversation_id)
            return True
        except Exception as e:
            logger.error(f"Error cancelling execution: {e}", exc_info=True)
            return False
```

**取消机制**：
- **标志 + 任务双重取消**：`cancelled_flags[conversation_id] = True` + `task.cancel()` + `await execution_info.task`
- **线程安全**：所有操作加 `threading.Lock`
- **快速响应**：标志先设置，流式输出在循环检测时立即退出

### 2.3 Pregel 图算法适配 `PregelGraphAdapter.convert()`

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

    def convert(self) -> BaseFlow:
        self._workflow.connections = []
        self._pre_process_graph()    # 1. 预处理：插入空子节点
        self._validate_graph()       # 2. 校验：连通性 + 环检测
        self._travel_all_nodes()     # 3. 拓扑遍历：cba 消减
        self._dfx()
        return self._workflow
```

**MultiDiGraph 选择原因**：支持同一对节点间的多条边（分支场景），这是 `DiGraph` 无法实现的。

### 2.4 cba（closest branch ancestor）分支消减算法

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
1. **每个分支节点**记录 `cba`（最近分支祖先）+ `total_branches`（分支总数）+ `cur_branches`（已汇合数）
2. **当所有分支汇合到同一节点**时，消减该 cba（分支完成）
3. **消减后向上层传播**，检查更高层分支是否也完成汇合
4. **最终 `cba_map` 中只剩 1 个祖先**时，节点的 `cba` 属性被设置

**`_multiple_dependency` 依赖计算**：

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

**设计亮点**：通过**笛卡尔积**处理多分支汇合，确保 Pregel 超步执行时正确等待所有前驱完成。

### 2.5 `_validate_graph` 图校验

```python
def _validate_graph(self) -> None:
    self._validate_isolated_source_nodes()    # 孤立起始节点必须是 START
    self._validate_connectivity()              # 连通性 start → end
    cycles: List[List[str]] = list(nx.simple_cycles(self._graph))
    if cycles:
        raise JiuWenGraphException(code=StatusCode.WORKFLOW_GRAPH_CIRCLE_ERROR.code, ...)

def _validate_isolated_source_nodes(self) -> None:
    for node in self._graph.nodes():
        if node.startswith('block_start_'):    # 跳过循环体内部节点
            continue
        out_degree = self._graph.out_degree(node)
        in_degree = self._graph.in_degree(node)
        if out_degree > 0 and in_degree == 0:
            node_type = self._graph.nodes[node]['type']
            if node_type != ComponentType.COMPONENT_TYPE_START:
                raise JiuWenComponentException(
                    code=StatusCode.WORKFLOW_GRAPH_START_NODE_ERROR.code, ...)
```

**三层校验**：
1. **孤立起始节点校验**：所有入度=0 的节点必须是 START
2. **连通性校验**：`nx.has_path(start, end)`
3. **环检测**：`nx.simple_cycles()`

### 2.6 对 laew 借鉴

| 借鉴点 | laew 现状 | 建议实现 |
|--------|-----------|----------|
| **冲突检测 + 取消** | 无 | `SessionState` 加 `is_running` 标志 + `tokio::sync::Mutex`；新输入到达时先 cancel 旧 task |
| **JSON 任务注册** | 无 | `WorkflowExecutionManager` 改造为 Rust 的 `DashMap<String, Arc<Task>>` |
| **cba 分支消减算法** | 无 Workflow | P2 优先级：引入 `petgraph::algo` + 实现 cba + 笛卡尔积 |

---

## 3. DSL 转换器核心代码路径

**代码路径**：
- `backend/openjiuwen_studio/core/dsl_converter/converter/converter.py`（73 行）
- `backend/openjiuwen_studio/core/dsl_converter/converter/converter_native.py`（449 行）
- `backend/openjiuwen_studio/core/manager/workflow_code_generator.py`（600+ 行）

### 3.1 抽象转换器 + 工厂模式

```python
from abc import ABC, abstractmethod

@dataclass
class WorkflowImportResult:
    workflow_data: Dict[str, Any]    # OpenJiuwen 格式工作流数据
    warnings: List[str] = field(default_factory=list)    # 非致命问题
    metadata: Dict[str, Any] = field(default_factory=dict)  # 原始来源信息


class WorkflowConverter(ABC):
    @abstractmethod
    def convert(self, json_data: Dict[str, Any]) -> WorkflowImportResult:
        pass


class ConverterFactory:
    @staticmethod
    def create(format_type: WorkflowFormat) -> WorkflowConverter:
        if format_type == WorkflowFormat.OPENJIUWEN_NATIVE:
            from openjiuwen_studio.core.dsl_converter.converter.converter_native import NativeWorkflowConverter
            return NativeWorkflowConverter()
        elif format_type == WorkflowFormat.N8N:
            from openjiuwen_studio.core.dsl_converter.converter.converter_n8n import N8nWorkflowConverter
            return N8nWorkflowConverter()
        else:
            raise ValueError(f"Unsupported workflow format: {format_type}")
```

**设计模式**：
- **抽象基类**（`WorkflowConverter`）定义接口
- **工厂方法**（`ConverterFactory.create`）按 `WorkflowFormat` 枚举分发
- **延迟导入**（`from ... import ...` 在函数体内）避免循环依赖

### 3.2 低代码→代码：`NativeWorkflowConverter.convert()`

**8 步流水线**：

```python
def convert(self, json_data: Dict[str, Any]) -> WorkflowImportResult:
    warnings = []
    json_data = copy.deepcopy(json_data)

    # Step 0: 格式归一化（顶层 nodes/edges → schema 字段）
    if "schema" not in json_data and "nodes" in json_data and "edges" in json_data:
        schema_obj = {
            "nodes": json_data.pop("nodes"),
            "edges": json_data.pop("edges")
        }
        json_data["schema"] = schema_obj

    # Step 1: schema 字段归一化（object → JSON string）
    schema_field = json_data.get("schema")
    if schema_field and not isinstance(schema_field, str):
        json_data["schema"] = json.dumps(schema_field)

    # Step 2: 补默认值
    current_time = milliseconds()
    if "workflow_id" not in json_data or not json_data["workflow_id"]:
        json_data["workflow_id"] = str(uuid.uuid4())
    json_data["space_id"] = ""  # 总是清空（由 importer 设置目标空间）

    defaults = {
        "name": "Imported Workflow", "desc": "Imported Workflow",
        "url": "", "icon_uri": "",
        "input_parameters": [], "output_parameters": [],
        "create_time": current_time, "update_time": current_time
    }
    for key, default_value in defaults.items():
        if key not in json_data or json_data[key] is None:
            json_data[key] = default_value

    # Step 3: Pydantic 校验
    WorkflowBase.model_validate(json_data)

    # Step 4: 重新生成 workflow_id（避免冲突）
    new_workflow_id = str(uuid.uuid4())
    json_data["workflow_id"] = new_workflow_id

    # Step 5: 重新生成画布节点 ID
    schema_str = json_data.get("schema")
    if schema_str:
        schema = json.loads(schema_str) if isinstance(schema_str, str) else schema_str
        schema, id_mapping = self.regenerate_canvas_ids(schema)
        json_data["schema"] = json.dumps(schema)

    # Step 6: 更新时间戳
    json_data["create_time"] = current_time
    json_data["update_time"] = current_time

    # Step 7: 清空版本字段（导入后创建 draft）
    json_data.pop("workflow_version", None)
    json_data.pop("latest_publish_version", None)
    json_data.pop("latest_publish_time", None)

    return WorkflowImportResult(
        workflow_data=json_data,
        warnings=warnings,
        metadata={"original_workflow_id": ..., "source_format": "openjiuwen_native", ...}
    )
```

**设计亮点**：
- **节点 ID 重新生成**（`regenerate_canvas_ids`）：避免与目标空间已有节点冲突
- **Space 清空**：`space_id` 总是从源清空，由 importer 用目标空间填充
- **schema 双向格式**：接受 string/object/顶层 nodes-edges 三种格式

### 3.3 节点 ID 重生成算法 `regenerate_canvas_ids()`

```python
NODE_TYPE_PREFIX_MAP = {
    "1": "start", "2": "end", "3": "llm", "4": "condition",
    "5": "code", "6": "knowledge", "11": "loop", "14": "subworkflow",
    # ... 20 种类型
}

def regenerate_canvas_ids(self, schema: Dict[str, Any]) -> tuple[Dict[str, Any], Dict[str, str]]:
    id_mapping = {}

    # 1. 生成新 ID for 每个节点
    for node in schema.get("nodes", []):
        old_id = node.get("id")
        if not old_id:
            continue
        node_type = str(node.get("type", "node"))
        prefix = NODE_TYPE_PREFIX_MAP.get(node_type, f"node{node_type}")
        new_id = f"{prefix}_{uuid.uuid4().hex[:8]}"
        id_mapping[old_id] = new_id
        node["id"] = new_id

    # 2. 更新 edges 引用
    for edge in schema.get("edges", []):
        source = edge.get("sourceNodeID")
        target = edge.get("targetNodeID")
        if source in id_mapping:
            edge["sourceNodeID"] = id_mapping[source]
        if target in id_mapping:
            edge["targetNodeID"] = id_mapping[target]

    # 3. 更新嵌套结构（blocks / inputParameters）
    self._update_node_references(schema.get("nodes", []), id_mapping)
    return schema, id_mapping
```

**为何 ID 前缀映射重要**：引擎通过前缀识别节点类型（如 `start_` 标识 START 节点），不是用 `type` 字段。导入时必须重建前缀。

### 3.4 代码→低代码（反向）：`WorkflowCodeGenerator.generate()`

```python
class WorkflowCodeGenerator:
    def __init__(self, workflow: dsl.Workflow) -> None:
        self.workflow = workflow
        self._non_branch_connections: List[Tuple[str, str]] = []
        self._branch_targets: Dict[Tuple[str, str], List[str]] = {}
        self._build_connection_maps()

        # 动态导入标志位
        self._need_llm_imports = False
        self._need_intent_imports = False
        self._need_plugin_service_imports = False
        # ... 共 13 个 _need_*_imports 标志

    def generate(self) -> str:
        # First pass: 生成组件函数（填充 import flags）
        comp_function_blocks = self._gen_all_component_functions()

        sections: List[str] = [
            self._gen_header(),              # 文件头注释
            self._gen_imports(),             # 动态 import（按需）
            self._gen_workflow_metadata(),    # 元数据常量
            self._gen_model_config_helper(), # 模型配置辅助函数
            comp_function_blocks,            # 每个组件一个函数
            self._gen_build_workflow(),      # 组装工作流
            self._gen_main(),                # 入口函数
        ]
        return "\n".join(sections)
```

**7 段式代码生成**：
1. **header**：Python shebang + 使用说明 + pip install 提示
2. **imports**：核心 `openjiuwen.core.workflow` import + LLM/Tool 条件 import（根据 `_need_*_imports` 标志）
3. **metadata**：`WORKFLOW_ID`、`WORKFLOW_NAME`、`WORKFLOW_INPUTS` 常量
4. **model_config_helper**：`_build_model_configs` 函数（API key 从环境变量读）
5. **component_functions**：每个组件一个 `create_X_component()` 函数
6. **build_workflow**：组装 `Workflow` 实例
7. **main**：入口 `async def main()` + `asyncio.run`

**关键技巧**：API Key 通过环境变量注入：
```python
client_config = ModelClientConfig(
    client_provider=client_provider,
    api_key=os.getenv(api_key_env_var, ''),
    api_base=api_base,
    timeout=timeout,
)
```

### 3.5 组件函数生成 `_gen_component_function()`

```python
def _gen_component_function(self, comp: dsl.Component, func_name: str) -> Optional[str]:
    ctype = comp.type

    if ctype == ComponentType.COMPONENT_TYPE_START:
        return self._gen_start_fn(comp, func_name)
    elif ctype == ComponentType.COMPONENT_TYPE_END:
        return self._gen_end_fn(comp, func_name)
    elif ctype == ComponentType.COMPONENT_TYPE_LLM:
        self._need_llm_imports = True
        return self._gen_llm_fn(comp, func_name)
    # ... 共 18 种组件类型
```

**统一的分发模式**：与 `Workflow.COMPILER_HANDLERS` 异曲同工，都是注册表模式。

### 3.6 对 laew 借鉴

| 借鉴点 | laew 现状 | 建议实现 |
|--------|-----------|----------|
| **抽象转换器 + 工厂** | 无 DSL 概念 | P2：在 `laew-dsl` 子模块定义 `WorkflowConverter` trait + `ConverterFactory::create` |
| **代码生成（DSL→Rust）** | 无 | P2：实现 `WorkflowCodeGenerator::generate()`，输出可直接 `cargo run` 的 Rust 文件 |
| **节点 ID 重建算法** | 无 | P2：导入外部 DSL 时复用 |

---

## 4. MCP 插件核心代码路径

**代码路径**：`backend/openjiuwen_studio/core/executor/plugin/plugin_tools.py`（383 行）

### 4.1 三种插件类型统一抽象

```python
class ServiceTool:
    """RESTful API 工具 — 编译为 RestfulApi"""
    def __init__(self, restfulapischema: DlRestfulApiSchema) -> None:
        self.restfulapischema: DlRestfulApiSchema = restfulapischema

    def compile(self) -> RestfulApi:
        queries = {}
        headers = self.restfulapischema.headers
        url = self.restfulapischema.path

        # 非运行时默认值 → URL / queries / headers
        for i in self.restfulapischema.params:
            if not i.runtime:
                if i.method == "query" and i.default_value is not None:
                    queries[i.name] = i.default_value
                elif i.method == "header" and i.default_value is not None:
                    headers[i.name] = i.default_value
                elif i.method == "path" and i.default_value is not None:
                    placeholder = f"{{{i.name}}}"
                    if placeholder in url:
                        url = url.replace(placeholder, str(i.default_value))

        tool_name = self.restfulapischema.name or self.restfulapischema.tool_id
        input_params = convert_params_to_json_schema(self.restfulapischema.params)
        restfulapi_card = RestfulApiCard(
            name=tool_name, description=self.restfulapischema.description,
            input_params=input_params, url=url,
            method=self.restfulapischema.method,
            headers=headers, queries=queries,
        )
        return RestfulApi(restfulapi_card)


class CodeTool:
    """代码插件工具 — 编译为 PluginCodeTool"""
    def __init__(self, codeschema: DlPluginCodeConfig) -> None:
        self.codeschema: DlPluginCodeConfig = codeschema

    def compile(self) -> Tool:
        return PluginCodeTool.create(self.codeschema)


class McpTool:
    """MCP 工具 — 编译为 PluginMcpTool"""
    def __init__(self, mcpconfig: DlMcpConfig) -> None:
        self.mcpconfig: DlMcpConfig = mcpconfig

    def compile(self) -> Tool:
        return PluginMcpTool.create(self.mcpconfig)
```

**统一抽象**：三种插件类型的 `compile()` 都返回 `Tool`，符合 LSP（里氏替换）。

### 4.2 5 种 MCP 传输实现 `PluginMcpTool.invoke()`

```python
async def invoke(self, inputs: Input, **kwargs) -> Output:
    from openjiuwen.core.foundation.tool.mcp.base import MCPTool
    from openjiuwen.core.foundation.tool.mcp.client.stdio_client import StdioClient
    from openjiuwen.core.foundation.tool.mcp.client.sse_client import SseClient
    from openjiuwen.core.foundation.tool.mcp.client.streamable_http_client import StreamableHttpClient
    from openjiuwen.core.foundation.tool.mcp.client.playwright_client import PlaywrightClient
    from openjiuwen.core.foundation.tool.mcp.client.openapi_client import OpenApiClient

    conf = self.conf
    tool_name = conf.mcp_tool_name or conf.name
    arguments = dict(inputs) if inputs else {}
    server_name = conf.tool_id or tool_name
    auth_headers = self._build_auth_headers(inputs)

    try:
        from openjiuwen.core.foundation.tool.mcp.base import McpServerConfig

        # 1. 传输类型映射表
        _transport_to_client_type = {
            McpTransport.STDIO: "stdio",
            McpTransport.SSE: "sse",
            McpTransport.STREAMABLE_HTTP: "streamable-http",
            McpTransport.OPENAPI: "openapi",
            McpTransport.PLAYWRIGHT: "playwright",
        }
        client_type = _transport_to_client_type.get(conf.transport)

        mcp_params = dict(conf.params or {})
        if conf.transport == McpTransport.STDIO:
            cmd = mcp_params.get("command") or conf.url or ""
            extra_args = list(mcp_params.get("args") or [])
            # 若 .py 文件 → 用 sys.executable 包装
            if cmd.endswith(".py"):
                mcp_params["command"] = sys.executable
                mcp_params["args"] = [cmd] + extra_args
            else:
                mcp_params["command"] = cmd
                mcp_params["args"] = extra_args
            mcp_params.setdefault("env", None)
            mcp_params.setdefault("cwd", os.getcwd())
            mcp_params.setdefault("encoding_error_handler", "strict")
            server_url = conf.url or ""
        else:
            server_url, url_auth_query = merge_mcp_server_url_query_params(conf.url, None)

        server_config = McpServerConfig(
            server_name=server_name,
            server_path=(conf.url or "") if conf.transport == McpTransport.STDIO else server_url,
            client_type=client_type,
            params=mcp_params,
            auth_headers=auth_headers or {},
            auth_query_params=url_auth_query if conf.transport != McpTransport.STDIO else {},
        )

        # 2. 按传输类型创建客户端
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
        connected = await client.connect()
        if not connected:
            raise JiuWenExecuteException(...)

        try:
            tool_cards = await client.list_tools()
            target_card = next((c for c in tool_cards if c.name == tool_name), None)
            if target_card is None:
                raise JiuWenExecuteException(...)

            mcp_tool = MCPTool(mcp_client=client, tool_info=target_card)
            result = await mcp_tool.invoke(arguments)
        finally:
            await client.disconnect()

    except JiuWenExecuteException as e:
        return {'code': e.code, 'message': e.message, 'data': None}
    except Exception as e:
        return {'code': StatusCode.PLUGIN_CODE_TOOL_INVOKE_ERROR.code, 'message': ..., 'data': None}

    return {'code': 0, 'message': 'success', 'data': result}
```

**5 种 MCP 传输**：

| 传输类型 | 实现类 | 适用场景 |
|----------|--------|----------|
| `STDIO` | `StdioClient` | 本地进程通信（如 `.py` 自动包装为 `sys.executable`） |
| `SSE` | `SseClient` | Server-Sent Events 流式 |
| `STREAMABLE_HTTP` | `StreamableHttpClient` | HTTP 流式传输 |
| `OPENAPI` | `OpenApiClient` | OpenAPI/Swagger 接口 |
| `PLAYWRIGHT` | `PlaywrightClient` | 浏览器自动化 |

**统一抽象亮点**：
1. **`McpServerConfig`** 统一参数：`server_name`、`server_path`、`client_type`、`params`、`auth_headers`
3. **stdio 自动识别 `.py`**：`if cmd.endswith(".py")` 自动用 `sys.executable` 包装
4. **`finally: disconnect()`**：哪怕调用失败也断开连接，避免僵尸进程

### 4.3 流式契约 `PluginMcpTool.stream()`

```python
async def stream(self, inputs: Input, **kwargs):
    """Satisfy the abstract stream() contract by delegating to invoke().
    PluginMcpTool is not a streaming source — it performs a single
    request/response cycle.  Yielding the complete result as one chunk
    lets the workflow's streaming pipeline proceed normally.
    """
    result = await self.invoke(inputs, **kwargs)
    yield result
```

**巧妙设计**：MCP 不是流式源，但基类要求实现 `stream()`。`stream()` 委托给 `invoke()`，单 chunk 提交，让工作流的流式管道正常工作。

### 4.4 对 laew 借鉴

| 借鉴点 | laew 现状 | 建议实现 |
|--------|-----------|----------|
| **5 种 MCP 传输** | 无 MCP 支持 | P0：实现 `McpClient` trait + 5 种 client；封装 `PluginMcpTool`，从 MCP Server 动态发现工具 |
| **stdio `.py` 自动包装** | - | `if cmd.ends_with(".py") { cmd = format!("{} {}", python_path, cmd); }` |
| **`finally: disconnect()`** | - | 用 Rust `Drop` trait 自动断开 |
| **统一 `Tool` trait** | laew 已有 `Tool` trait | 直接套用，laew 已有 `BashTool/ReadTool/WriteTool`，MCP 可作为新注册 |

---

## 5. 沙箱核心代码路径

**代码路径**：
- `sandbox_server/sandbox/openjiuwen_sandbox_server/app/base.py`（70 行）
- `sandbox_server/sandbox/openjiuwen_sandbox_server/app/bwrap.py`（199 行）
- `sandbox_server/sandbox/openjiuwen_sandbox_server/app/network_guard.py`
- `sandbox_server/sandbox/openjiuwen_sandbox_server/app/sandbox_config.py`

### 5.1 抽象基类 + 自动注册 `BaseSandbox`

```python
class BaseSandbox(ABC):
    """Base class for all sandbox implementations.
    New sandbox types register automatically via subclassing:
        class MyRunner(BaseSandbox, sandbox_type='my_sandbox'):
            ...
    Then 'my_sandbox' becomes available through BaseSandbox.get_class().
    """

    _registry: dict[str, type['BaseSandbox']] = {}

    def __init_subclass__(cls, sandbox_type: str | None = None, **kwargs):
        super().__init_subclass__(**kwargs)
        if sandbox_type is not None:
            BaseSandbox._registry[sandbox_type] = cls

    @classmethod
    def get_class(cls, sandbox_type: str) -> type['BaseSandbox']:
        if sandbox_type not in cls._registry:
            available = list(cls._registry.keys())
            raise ValueError(
                f"Unknown sandbox type: '{sandbox_type}'. Available: {available}"
            )
        return cls._registry[sandbox_type]

    @abstractmethod
    def run(self, raw_code, base_code, lang, timeout=0, dep_name=None) -> ExecutionResult:
        ...

    @staticmethod
    def _execute_process(cmd, envs, timeout, pass_fds=()):
        """Run a subprocess with timeout, returning ExecutionResult."""
        try:
            popen_kwargs = dict(
                env=envs,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                text=True,
            )
            if sys.platform == 'win32':
                popen_kwargs['creationflags'] = subprocess.CREATE_NEW_PROCESS_GROUP
            else:
                popen_kwargs['start_new_session'] = True
                popen_kwargs['pass_fds'] = pass_fds
            process = subprocess.Popen(cmd, **popen_kwargs)
            stdout, stderr = process.communicate(timeout=timeout)
            return ExecutionResult(process.returncode, stdout, stderr)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            return ExecutionResult(-1, '', 'code execution timeout.')
```

**自动注册机制**（`__init_subclass__`）：
- 新增沙箱实现只需 `class MyRunner(BaseSandbox, sandbox_type='xxx')`
- `_registry` 自动填充
- **`get_class()`** 工厂方法返回对应实现
- **`_execute_process`** 统一超时控制：超时自动 kill

### 5.2 BubbleWrap 命名空间隔离 `BubbleWrapRunner._namespace_params()`

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
    return [
        flag for ns, flag in NAMESPACE_FLAGS.items()
        if self._sandbox_config.namespace.get(ns, False)
    ]
```

**6 种命名空间**：user（用户ID）/ ipc（信号量）/ pid（进程ID）/ net（网络）/ uts（主机名）/ cgroup（资源限制）

### 5.3 文件系统只读挂载 `_mount_params()`

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
        if not flag:
            raise ValueError(f"Unknown mount mode: {mount['mode']}")
        params += [flag, mount['src'], mount['dst']]

    # 工作目录强制只读
    params += ['--ro-bind', workdir, dst_code_dir]

    # 依赖路径只读
    for path in extra_paths:
        params += ['--ro-bind', path, path]

    return params
```

**3 种挂载模式**：read / write / dev；工作目录+依赖强制只读。

### 5.4 Seccomp BPF 加载 `pre_init()` + `_apply_seccomp()`

```python
@staticmethod
def pre_init(sandbox_config):
    arch = platform.machine()
    if arch not in ('x86_64', 'aarch64'):
        raise RuntimeError(f"Unsupported architecture: {arch}")

    if not sandbox_config.allow_internal_network_access:
        apply_internal_network_guard(BWRAP_RUN_USER)

    allowed = sandbox_config.seccomp['allow'].get(arch, [])
    if not allowed:
        sandbox_config.seccomp_bpf = None
        return

    bpf = pyseccomp.SyscallFilter(pyseccomp.KILL)  # 默认 KILL 策略
    for syscall in allowed:
        bpf.add_rule(pyseccomp.ALLOW, syscall)
    sandbox_config.seccomp_bpf = bpf

def _apply_seccomp(self, workdir, dst_code_dir, lang):
    """Write seccomp BPF and return an fd for JS, or None."""
    if not self._sandbox_config.seccomp_bpf:
        return None

    src_bpf = os.path.join(workdir, SECCOMP_BPF_FILENAME)
    with open(src_bpf, 'wb') as f:
        self._sandbox_config.seccomp_bpf.export_bpf(f)

    if lang == 'javascript':
        return os.open(src_bpf, os.O_RDONLY)
    return None
```

**KILL 策略 + ALLOW 白名单**：默认 KILL 所有系统调用，逐个添加白名单。

### 5.5 Python 内联 Seccomp 加载器 `_build_py_seccomp_loader()`

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
        _fields_ = [
            ("code", ctypes.c_ushort),
            ("jt",   ctypes.c_ubyte),
            ("jf",   ctypes.c_ubyte),
            ("k",    ctypes.c_uint32),
        ]
    class SockFprog(ctypes.Structure):
        _fields_ = [
            ("len",    ctypes.c_ushort),
            ("filter", ctypes.POINTER(SockFilter))
        ]
    with open(bpf_file, 'rb') as f:
        bpf_data = f.read()
    struct_size = ctypes.sizeof(SockFilter)
    inst_cnt = len(bpf_data) // struct_size
    FilterArrayType = SockFilter * inst_cnt
    filter_array = FilterArrayType.from_buffer_copy(bpf_data)
    prog = SockFprog()
    prog.len = inst_cnt
    prog.filter = ctypes.cast(filter_array, ctypes.POINTER(SockFilter))
    libc = ctypes.CDLL(None, use_errno=True)
    ret = libc.prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)
    if ret != 0:
        raise OSError(f"prctl(NO_NEW_PRIVS) failed. Errno: {{ctypes.get_errno()}}")
    ret = libc.prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, ctypes.byref(prog))
    if ret != 0:
        raise OSError(f"prctl(SECCOMP) failed. Errno: {{ctypes.get_errno()}}")
_load_seccomp("{bpf_path}")
del _load_seccomp
'''
```

**巧妙设计**：Python 代码通过**内联的 ctypes 代码**在运行时加载 BPF 过滤器，无需外部依赖。这是**代码生成作为运行时机制**的经典例子——把 BPF 加载逻辑字符串直接拼接到执行代码前。

### 5.6 主流程 `BubbleWrapRunner.run()`

```python
def run(self, raw_code, base_code, lang, timeout=0, dep_name=None):
    with tempfile.TemporaryDirectory(prefix='bwrap_workdir_', dir='/tmp') as workdir:
        os.chmod(workdir, 0o755)
        dst_code_dir = '/code'

        seccomp_fd = None
        try:
            seccomp_fd = self._apply_seccomp(workdir, dst_code_dir, lang)
            if lang == 'python' and self._sandbox_config.seccomp_bpf:
                dst_bpf = os.path.join(dst_code_dir, SECCOMP_BPF_FILENAME)
                base_code = _build_py_seccomp_loader(dst_bpf) + '\n' + base_code

            envs = self._sandbox_config.environment.copy()
            dep_paths = []
            if self._dep_mngr:
                dep_envs, dep_paths = self._dep_mngr.get_dependency_setting(lang, dep_name)
                envs = merge_environments(envs, dep_envs)

            cmd = self._sandbox_command()
            cmd += self._mount_params(workdir, dst_code_dir, dep_paths)
            cmd += self._namespace_params()
            if self._sandbox_config.options:
                cmd += self._sandbox_config.options
            if seccomp_fd is not None:
                cmd += ['--seccomp', str(seccomp_fd)]
            cmd += generate_eval_command(lang, None, base_code, raw_code)

            pass_fds = (seccomp_fd,) if seccomp_fd is not None else ()
            effective_timeout = max(timeout, int(self._sandbox_config.timeout))
            result = self._execute_process(cmd, envs, effective_timeout, pass_fds)

            # retcode=159 表示 Bad syscall（来自 seccomp KILL）
            if result.retcode == 159:
                result = ExecutionResult(
                    result.retcode, result.stdout,
                    result.stderr + '\nBad syscall detected.',
                )
            return result
        finally:
            if seccomp_fd is not None:
                os.close(seccomp_fd)
```

**9 步执行**：
1. 创建临时工作目录（`/tmp/bwrap_workdir_xxx`）
2. 应用 seccomp BPF（`--seccomp` FD 或内联 Python 加载器）
3. Python 场景：拼接 `_build_py_seccomp_loader(bpf_path) + '\n' + base_code`
4. 合并环境变量（依赖 env + 用户 env）
5. 构建 bwrap 命令（5 段拼接）
6. 设置 pass_fds（传递 seccomp fd 到子进程）
7. 取最大超时（用户 timeout vs 配置 timeout）
8. 执行（带超时 kill）
9. 检测 retcode=159 → 标记为 `Bad syscall detected.`

### 5.7 `retcode=159` 含义

Linux 上 `SECCOMP_RET_KILL` 默认终止信号导致退出码为 `128 + 9 = 137`，但 BPF KILL 模式下子进程被 SIGSYS 终止，不同 shell 表现不同。bwrap 的 `--seccomp` 标志传递 fd 时，seccomp 违规会让进程以 159 退出（SIGSYS = 12 + 128 = 140；但 bwrap 包装后是 159）。检测 159 → 在 stderr 追加 "Bad syscall detected."

### 5.8 对 laew 借鉴

| 借鉴点 | laew 现状 | 建议实现 |
|--------|-----------|----------|
| **自动注册基类** | 硬编码 registry | 用 Rust trait + `inventory` crate 自动收集 |
| **BubbleWrap 命名空间** | 零沙箱 | P1b：用 `nix` crate 实现 user/pid/net 隔离 |
| **Seccomp BPF** | 零系统调用过滤 | P1c：用 `libseccomp-sys` 或 `seccompiler` crate |
| **retcode 检测** | - | 检测到 137/159 → "Bad syscall detected" |
| **Bash 工具改造** | 直接 shell exec | 在 `BashTool` 上挂 `sandbox: Option<Box<dyn Sandbox>>`，可选启用 |

---

## 6. 评估体系核心代码路径

**代码路径**：
- `backend/openjiuwen_studio/core/executor/evaluation/evaluation_harness.py`（709 行）
- `backend/openjiuwen_studio/core/executor/evaluation/grader_engine.py`（453 行）
- `backend/openjiuwen_studio/core/executor/evaluation/perturbations.py`（395 行）
- `backend/openjiuwen_studio/core/executor/evaluation/metrics.py`（521 行）

### 6.1 Trial × 4 扰动矩阵 `EvaluationHarness._execute_task()`

```python
async def _execute_task(self, config: _TaskRunConfig) -> None:
    task_id = config.task.get("task_id", "unknown")
    trials = int(config.task.get("trials") or 1)

    # 扰动类型
    perturbation_types = ["nominal"]
    if config.enable_perturbations:
        perturbation_types.extend(["prompt_perturbed", "env_perturbed", "fault_injected"])

    # 双层循环：扰动 × trial
    for perturbation_type in perturbation_types:
        for trial_num in range(1, trials + 1):
            trial_cfg = _TrialRunConfig(
                run_id=config.run_id,
                task=config.task,
                trial_num=trial_num,
                # ...
                perturbation_type=perturbation_type,
            )
            await self._execute_trial(trial_cfg)
```

**4 种扰动类型**：

| 扰动类型 | 实现 | 用途 |
|----------|------|------|
| `nominal` | 原始输入 | 基线 |
| `prompt_perturbed` | 提示词改写 | 测试语义鲁棒性 |
| `env_perturbed` | 输入数据格式变化 | 测试环境鲁棒性 |
| `fault_injected` | 故障注入 | 测试容错性 |

**Trial × 4 扰动 = 4×N 次执行**：每个 task 的 trials 数量（默认 1，可配置 5/10），与 4 种扰动笛卡尔积。

### 6.2 单 Trial 执行 `_execute_trial()`

```python
async def _execute_trial(self, config: _TrialRunConfig) -> None:
    task_id = config.task.get("task_id", "unknown")
    result_id = str(uuid.uuid4())
    trace_id = str(uuid.uuid4())
    start_time = milliseconds()

    try:
        # 唯一 conversation_id 防止执行冲突
        conversation_id = (
            f"eval_{config.run_id}_{task_id}_t{config.trial_num}"
            f"_{config.perturbation_type[:4]}_{trace_id[:8]}"
        )

        inputs = config.task.get("input_data") or {}
        expected = config.task.get("expected_output")
        graders_cfg = config.task.get("graders_config") or []

        # 应用扰动
        if config.perturbation_type == "prompt_perturbed":
            if "prompt" in inputs:
                paraphrases = await self._perturbation_coordinator.generate_prompt_variants(
                    inputs["prompt"], num_variants=1
                )
                inputs = inputs.copy()
                inputs["prompt"] = paraphrases[0] if paraphrases else inputs["prompt"]
        elif config.perturbation_type == "env_perturbed":
            inputs = self._perturbation_coordinator.perturb_environment(inputs)
        elif config.perturbation_type == "fault_injected":
            pass  # 后执行注入

        # 执行 workflow 或 agent
        if config.workflow_id:
            execution_trace = await self._run_workflow(...)
        elif config.agent_id:
            execution_trace = await self._run_agent(...)
        else:
            raise ValueError("Either workflow_id or agent_id must be specified")

        # fault_injected 在执行后注入
        if config.perturbation_type == "fault_injected":
            fault = self._perturbation_coordinator.inject_fault()
            if fault:
                execution_trace["_injected_fault"] = fault
                if fault["type"] in ["malformed", "error"]:
                    execution_trace["final_output"] = {"error": fault["message"]}

        end_time = milliseconds()
        latency_ms = end_time - start_time

        action_sequence = self._extract_action_sequence(execution_trace)
        confidence = self._extract_confidence(execution_trace)

        # 评分
        grader_results = await self._grader.run_graders(
            graders_cfg, execution_trace, expected, config.space_id
        )

        # 模式验证
        for pt in pattern_types_list:
            pattern_ok = await self._pattern_validator.validate_pattern(pt, execution_trace)
            grader_results.append({"grader_name": f"pattern_check_{pt}", ...})

        # 安全评分
        safety_violations, safety_severity = await self._evaluate_safety(execution_trace, inputs)

        # 权重感知聚合
        active = [r for r in grader_results if r.get("weight", 1.0) > 0]
        passed = all(r.get("passed", False) for r in active) if active else False
        total_weight = sum(r.get("weight", 1.0) for r in active)
        score = (
            sum(r.get("score", 0.0) * r.get("weight", 1.0) for r in active) / total_weight
            if active and total_weight > 0 else 0.0
        )

        token_usage = execution_trace.get("token_usage")

    except Exception as e:
        # 异常 → 标记为失败
        passed = False
        score = 0.0
        # ...

    # 持久化结果
    evaluation_task_result_repository.create({
        "result_id": result_id,
        "run_id": config.run_id,
        "task_id": task_id,
        "trial_number": config.trial_num,
        "trace_id": trace_id,
        "grader_results": grader_results,
        "passed": 1 if passed else 0,
        "score": score,
        "latency_ms": latency_ms,
        "token_usage": token_usage,
        "perturbation_type": config.perturbation_type,
        # ...
    })
```

**关键设计**：
- **unique conversation_id**：`eval_{run_id}_{task_id}_t{trial_num}_{perturbation[:4]}_{trace_id[:8]}`，避免 trial 之间冲突
- **扰动应用时机**：prompt/env 在执行前；fault 在执行后（注入到 final_output）
- **权重感知聚合**：weight=0 的评分器仅信息性，不参与 pass/fail 和 score
- **异常即失败**：trial 抛异常 → passed=False, score=0.0（避免评估跑挂）

### 6.3 三种评分器 `GraderEngine.run_graders()`

```python
async def run_graders(
        self,
        graders_config: List[Dict[str, Any]],
        execution_trace: Dict[str, Any],
        expected_output: Optional[Any],
        space_id: str,
) -> List[Dict[str, Any]]:
    results: List[Dict[str, Any]] = []

    for grader_cfg in graders_config:
        grader_name = grader_cfg.get("name", "unnamed_grader")
        try:
            grader_type = int(grader_cfg.get("grader_type") if grader_cfg.get("grader_type") is not None
                              else grader_cfg.get("type", GraderType.DETERMINISTIC))

            if grader_type == GraderType.DETERMINISTIC:
                result = self._run_deterministic(grader_cfg, execution_trace, expected_output)
            elif grader_type == GraderType.MODEL_BASED:
                result = await self._run_model_based(grader_cfg, execution_trace, expected_output, space_id)
            elif grader_type == GraderType.CODE_BASED:
                result = self._run_code_based(grader_cfg, execution_trace, expected_output)
            else:
                result = {"grader_name": grader_name, "passed": False, "score": 0.0, "error": f"Unknown grader_type: {grader_type}"}
        except Exception as e:
            result = {"grader_name": grader_name, "passed": False, "score": 0.0, "error": str(e)}

        result["weight"] = float(grader_cfg.get("weight", 1.0))
        results.append(result)

    return results
```

### 6.4 确定性评分器 `_run_deterministic()`

```python
def _run_deterministic(self, cfg: Dict, trace: Dict, expected: Any) -> Dict:
    grader_name = cfg.get("name", "deterministic")
    inner = cfg.get("config") or {}
    if not inner:
        _top_level_keys = {"name", "type", "grader_type", "weight", "config"}
        inner = {k: v for k, v in cfg.items() if k not in _top_level_keys}
    check_type = inner.get("check_type", "output_check")

    _aliases = {
        "output": "output_check", "state": "state_check",
        "tool_call": "tool_call_check", "pattern": "pattern_check",
        "transcript": "transcript_check",
        "contains": "pattern_check", "regex": "pattern_check",
    }
    check_type = _aliases.get(check_type, check_type)

    if check_type == "output_check":
        return self._check_output(grader_name, inner, trace, expected)
    elif check_type == "state_check":
        return self._check_state(grader_name, inner, trace)
    elif check_type == "tool_call_check":
        return self._check_tool_calls(grader_name, inner, trace)
    elif check_type == "pattern_check":
        return self._check_pattern_regex(grader_name, inner, trace)
    elif check_type == "transcript_check":
        return self._check_transcript(grader_name, inner, trace)
```

**5 种 check_type**：
1. **output_check**：比较 `final_output` 与 `expected_output`
2. **state_check**：在 `final_output` 中按 JSON path 取值比较
3. **tool_call_check**：验证指定工具被调用
4. **pattern_check**：正则匹配序列化 trace
5. **transcript_check**：计数工具调用/组件执行次数

**`_compare()` 通用比较器**：

```python
@staticmethod
def _compare(actual: Any, expected: Any, condition: str) -> bool:
    try:
        try: actual = int(actual)
        except (ValueError, TypeError): pass
        try: expected = int(expected)
        except (ValueError, TypeError): pass

        if condition == "eq": return actual == expected
        elif condition == "ne": return actual != expected
        elif condition == "gt": return actual > expected
        elif condition == "lt": return actual < expected
        elif condition == "ge": return actual >= expected
        elif condition == "le": return actual <= expected
        elif condition == "contains": return str(expected) in str(actual)
        elif condition == "not_contains": return str(expected) not in str(actual)
        elif condition == "regex": return bool(re.search(str(expected), str(actual)))
        elif condition == "is_not_empty":
            return actual is not None and actual != "" and actual != [] and actual != {}
```

**9 种比较条件**：eq/ne/gt/lt/ge/le/contains/not_contains/regex/is_not_empty

### 6.5 模型评分器 `_run_model_based()`（LLM-as-Judge）

```python
async def _run_model_based(self, cfg: Dict, trace: Dict, expected: Any, space_id: str) -> Dict:
    grader_name = cfg.get("name", "model_based")
    inner = cfg.get("config") or {}

    try:
        from openjiuwen_studio.core.manager.convertor.components.llm import build_dsl_model_config
        from openjiuwen.core.foundation.llm import Model, ModelClientConfig, ModelRequestConfig, UserMessage

        model_id = inner.get("model_id")
        if not model_id:
            raise ValueError("model_id is required for model-based grader")

        dsl_cfg = build_dsl_model_config(int(model_id), space_id)
        cc = dsl_cfg.model_client_config
        rc = dsl_cfg.request_config

        client_config = ModelClientConfig(
            client_provider=cc.client_provider or "openai",
            api_key=cc.api_key or "",
            api_base=cc.api_base or "",
            timeout=float(cc.timeout or 60.0),
            verify_ssl=False,
        )
        request_config = ModelRequestConfig(
            model=rc.model_name or "",
            temperature=rc.temperature if rc.temperature is not None else 0.0,
            top_p=rc.top_p if rc.top_p is not None else 0.9,
        )

        prompt = self._build_grading_prompt(trace, expected, inner)
        model = Model(model_client_config=client_config, model_config=request_config)
        result = await model.invoke([UserMessage(content=prompt)])
        response_text = result.content or ""

        parsed = self._parse_llm_response(response_text)
        return {
            "grader_name": grader_name,
            "grader_type": "model_based",
            "passed": parsed.get("passed", False),
            "score": float(parsed.get("score", 0.0)),
            "details": parsed,
        }
    except Exception as e:
        logger.error(f"Model-based grader '{grader_name}' failed: {e}", exc_info=True)
        return {"grader_name": grader_name, "grader_type": "model_based", "passed": False, "score": 0.0, "error": str(e)}
```

**评分提示词模板**：

```python
@staticmethod
def _build_grading_prompt(trace: Dict, expected: Any, cfg: Dict) -> str:
    rubric = cfg.get("rubric", "")
    assertions = cfg.get("assertions", [])

    lines = ["You are an evaluation judge for an AI workflow system.", ""]
    if rubric:
        lines += ["## Scoring Rubric", rubric, ""]
    if assertions:
        lines += ["## Assertions to verify"]
        lines += [f"- {a}" for a in assertions]

    final_output = json.dumps(trace.get("final_output"), default=str, indent=2)
    lines += [
        f"## Actual output\n{final_output}",
        f"## Expected output\n{json.dumps(expected, default=str, indent=2)}",
        "",
        'Respond with JSON only: {"passed": true/false, "score": 0.0-1.0, "feedback": "..."}',
    ]
    return "\n".join(lines)
```

**JSON 解析**：

```python
@staticmethod
def _parse_llm_response(text: str) -> Dict:
    try:
        m = re.search(r"\{.*\}", text, re.DOTALL)
        if m:
            return json.loads(m.group())
    except ValueError as parse_err:
        logger.debug(f"Failed to parse LLM response JSON: {parse_err}")
    return {"passed": False, "score": 0.0, "feedback": text[:500]}
```

**容错**：解析失败 → passed=False, score=0.0, 保留原 feedback 前 500 字符。

### 6.6 代码评分器 `_run_code_based()`

```python
@staticmethod
def _run_code_based(cfg: Dict, trace: Dict, expected: Any) -> Dict:
    grader_name = cfg.get("name", "code_based")
    inner = cfg.get("config", {})
    code = inner.get("code", "")
    fn_name = inner.get("function_name", "grade")

    if not code:
        return {"grader_name": grader_name, "passed": False, "score": 0.0, "error": "No code provided"}

    try:
        namespace: Dict[str, Any] = {}
        exec(compile(code, "<grader>", "exec"), namespace)  # nosec B102
        grade_fn = namespace.get(fn_name)
        if not callable(grade_fn):
            raise ValueError(f"Function '{fn_name}' not found or not callable")
        result = grade_fn(trace, expected)
        if not isinstance(result, dict):
            result = {"passed": bool(result), "score": 1.0 if result else 0.0}
        return {
            "grader_name": grader_name,
            "grader_type": "code_based",
            "passed": bool(result.get("passed", False)),
            "score": float(result.get("score", 0.0)),
            "details": result,
        }
    except Exception as e:
        return {"grader_name": grader_name, "grader_type": "code_based", "passed": False, "score": 0.0, "error": str(e), "traceback": traceback.format_exc()}
```

**`exec()` 沙箱**：用 Python 内置 `exec(compile(...))` 执行用户代码，函数签名 `def grade(trace, expected) -> Dict[str, Any]`。

### 6.7 扰动算法 `perturbations.py`

#### 6.7.1 提示词扰动 `PromptPerturber.paraphrase()`

```python
async def paraphrase(self, prompt: str, num_variants: int = 3) -> List[str]:
    if self.model_id and self.space_id:
        try:
            return await self._llm_paraphrase(prompt, num_variants)
        except Exception as e:
            import logging
            logging.getLogger(__name__).debug(f"LLM paraphrase failed, falling back to rule-based: {e}")
    return self._rule_based_paraphrase(prompt, num_variants)
```

**LLM 优先，规则回退**。

**`_rule_based_paraphrase` 三策略**：
1. **同义词替换**：`{'get': ['retrieve', 'fetch', 'obtain'], ...}`
2. **句子重排序**：分句 → shuffle → 重新拼接
3. **主动/被动转换**：简单正则启发式

#### 6.7.2 环境扰动 `EnvironmentPerturber.perturb_input()`

```python
def perturb_input(self, input_data: Dict[str, Any]) -> Dict[str, Any]:
    data = copy.deepcopy(input_data)
    perturbations = [
        self._reorder_fields,        # JSON 字段重排序
        self._rename_fields,         # snake_case ↔ camelCase
        self._change_date_formats,   # 日期格式转换
        self._add_optional_fields,   # 添加可选字段
    ]
    num_perturbations = random.randint(1, 2)
    selected = random.sample(perturbations, num_perturbations)
    for perturb_fn in selected:
        data = perturb_fn(data)
    return data
```

**4 种环境扰动**：字段重排/命名约定转换/日期格式/添加可选元数据。每次随机应用 1-2 种。

#### 6.7.3 故障注入 `FaultInjector.generate_fault()`

```python
@staticmethod
def generate_fault() -> Dict[str, Any]:
    fault_types = ['timeout', 'error', 'malformed', 'slow']
    fault_type = random.choice(fault_types)

    if fault_type == 'timeout':
        return {'type': 'timeout', 'message': 'Request timeout after 30 seconds', 'code': 'TIMEOUT'}
    elif fault_type == 'error':
        code = random.choice([500, 502, 503, 504])
        return {'type': 'error', 'message': f'HTTP {code}: Internal Server Error', 'code': code}
    elif fault_type == 'malformed':
        return {'type': 'malformed', 'message': 'Malformed response data', 'data': '{"incomplete": "json"'}
    else:  # slow
        return {'type': 'slow', 'message': 'Slow response', 'delay_ms': random.randint(5000, 15000)}
```

**4 种故障**：timeout / error(4 种 HTTP 码) / malformed(故意坏 JSON) / slow(5-15s 延迟)。

### 6.8 指标计算 `metrics.py`

**Pass@k 算法**：

```python
def compute_pass_at_k(results, k_values=None):
    """
    pass@k = probability that at least one of k independent samples passes.
    Formula: pass@k = 1 - C(n-c, k) / C(n, k)
    where n = total trials for a task, c = number of passing trials.
    """
    if k_values is None:
        k_values = [1, 3, 5]

    task_results: Dict[str, List[bool]] = defaultdict(list)
    for r in results:
        task_id = r.task_id if hasattr(r, "task_id") else r.get("task_id", "")
        passed = bool(r.passed if hasattr(r, "passed") else r.get("passed", False))
        task_results[task_id].append(passed)

    pass_at_k: Dict[int, float] = {}
    for k in k_values:
        task_probs = []
        for passes in task_results.values():
            n = len(passes)
            c = sum(passes)
            if n < k:
                continue
            denom = _comb(n, k)
            if denom == 0:
                continue
            prob = 1.0 - _comb(n - c, k) / denom
            task_probs.append(prob)
        pass_at_k[k] = sum(task_probs) / len(task_probs) if task_probs else 0.0

    return pass_at_k
```

**Pass^k 算法**（所有 k 个样本都通过的概率）：
```python
def compute_pass_pow_k(results, k_values=None):
    """
    Formula: pass^k = C(c, k) / C(n, k)
    """
    # ... 与 pass@k 几乎对称
    prob = _comb(c, k) / denom
```

**`_comb` 二项式系数**：
```python
def _comb(n: int, k: int) -> float:
    if k > n or k < 0: return 0.0
    if k == 0 or k == n: return 1.0
    return float(math.comb(n, k))
```

**10+ 种指标**：success_rate / passed / task_pass_rate / tasks_fully_passed_rate / tasks_never_passed_rate / avg_score / score_std / latency stats (min/max/median/p75/p95/std/cv) / pass_at_k / pass_pow_k / token_usage / perfect_score_rate / score_distribution / flakiness / per_grader_breakdown / tokens_efficiency

### 6.9 回归检测 `_detect_regressions()`

```python
@staticmethod
def _detect_regressions(current, previous, prev_run_id) -> List[Dict[str, Any]]:
    alerts: List[Dict[str, Any]] = []

    # 成功率下降 >10pp → high severity
    curr_sr = current.get("success_rate")
    prev_sr = previous.get("success_rate")
    if curr_sr is not None and prev_sr is not None:
        delta = curr_sr - prev_sr
        if delta < -0.10:
            alerts.append({"type": "regression", "metric": "success_rate", "severity": "high", ...})

    # 延迟增加 >500ms → medium severity
    curr_lat = current.get("avg_latency_ms")
    prev_lat = previous.get("avg_latency_ms")
    if curr_lat is not None and prev_lat is not None and prev_lat > 0:
        delta = curr_lat - prev_lat
        if delta > 500:
            alerts.append({"type": "regression", "metric": "avg_latency_ms", "severity": "medium", ...})

    # 分数下降 >15pp → high severity
    curr_score = current.get("avg_score")
    prev_score = previous.get("avg_score")
    if curr_score is not None and prev_score is not None:
        delta = curr_score - prev_score
        if delta < -0.15:
            alerts.append({"type": "regression", "metric": "avg_score", "severity": "high", ...})

    return alerts
```

**3 类回归阈值**：
- 成功率 -10pp → high
- 延迟 +500ms → medium
- 分数 -15pp → high

### 6.10 对 laew 借鉴

| 借鉴点 | laew 现状 | 建议实现 |
|--------|-----------|----------|
| **Trial × 4 扰动矩阵** | 无评估 | P1：`EvaluationHarness::execute_evaluation` 改造为 Rust async task |
| **3 种评分器** | Quality-Check 硬编码 | P1：实现 `Grader` trait + `DeterministicGrader / ModelGrader / CodeGrader` |
| **9 种比较条件** | 无 | 通用 `_compare(actual, expected, condition)` |
| **Pass@k / Pass^k** | 无 | 实现 `_comb(n, k)` + `pass_at_k` / `pass_pow_k` |
| **回归检测** | 无 | `_detect_regressions` 加 success_rate/latency/score 三个阈值 |

---

## 7. 组件注册表核心代码路径

**代码路径**：`backend/openjiuwen_studio/core/executor/workflow/workflow.py`（478 行）

### 7.1 `COMPILER_HANDLERS` 字典注册表

```python
class Workflow:
    # 组件编译器映射表 — 注册表模式
    COMPILER_HANDLERS = {
        ComponentType.COMPONENT_TYPE_LLM: '_compile_llm_component',
        ComponentType.COMPONENT_TYPE_QUESTION: '_compile_question_component',
        ComponentType.COMPONENT_TYPE_INTENT: '_compile_intent_component',
        ComponentType.COMPONENT_TYPE_INPUT: '_compile_input_component',
        ComponentType.COMPONENT_TYPE_OUTPUT: '_compile_output_component',
        ComponentType.COMPONENT_TYPE_TEXT_EDITOR: '_compile_text_editor_component',
        ComponentType.COMPONENT_TYPE_VARIABLE_MERGE: '_compile_variable_merge_component',
        ComponentType.COMPONENT_TYPE_CODE: '_compile_code_component',
        ComponentType.COMPONENT_TYPE_HTTP_REQUEST: '_compile_http_request_component',
        ComponentType.COMPONENT_TYPE_REACT_AGENT: '_compile_react_agent_component',
        ComponentType.COMPONENT_TYPE_KNOWLEDGE_RETRIEVAL: '_compile_knowledge_retrieval_component',
    }

    # 特殊组件类型（需要异步处理或特殊参数）
    SPECIAL_COMPONENT_TYPES = {
        ComponentType.COMPONENT_TYPE_IF,
        ComponentType.COMPONENT_TYPE_SUB_WORKFLOW,
        ComponentType.COMPONENT_TYPE_LOOP,
        ComponentType.COMPONENT_TYPE_PLUGIN,
    }

    # 空组件类型
    EMPTY_COMPONENT_TYPES = {
        ComponentType.COMPONENT_TYPE_EMPTY,
        ComponentType.COMPONENT_TYPE_CONTINUE,
        ComponentType.COMPONENT_TYPE_EMPTY_START,
        ComponentType.COMPONENT_TYPE_EMPTY_END,
    }
```

**3 套注册表**：
- **COMPILER_HANDLERS**：11 种标准组件 → 编译方法名（字符串）
- **SPECIAL_COMPONENT_TYPES**：4 种特殊组件（IF/SUB_WORKFLOW/LOOP/PLUGIN）走异步特殊路径
- **EMPTY_COMPONENT_TYPES**：4 种空组件（EMPTY/CONTINUE/EMPTY_START/EMPTY_END）直接返回空

### 7.2 编译入口 `compile_component()`

```python
async def compile_component(
        self,
        context: Context,
        workflow_dl: BaseFlow,
        comp: Component,
        loader: Optional[IWorkflowLoader] = None
) -> Any:
    """使用简单注册机制编译组件"""

    # 1. 处理空组件
    if comp.type in self.EMPTY_COMPONENT_TYPES:
        return EmptyComponent()

    # 2. 处理BREAK组件
    if comp.type == ComponentType.COMPONENT_TYPE_BREAK:
        return LoopBreakComponent()

    # 3. 处理特殊组件（保持原有逻辑）
    if comp.type in self.SPECIAL_COMPONENT_TYPES:
        return await self._compile_special_component(context, workflow_dl, comp, loader)

    # 4. 使用注册表编译标准组件
    handler_name = self.COMPILER_HANDLERS.get(comp.type)
    if handler_name:
        handler = getattr(self, handler_name)
        return await handler(comp, workflow_dl)
    else:
        logger.warning(f"Unsupported component type: {comp.type}")
        return None
```

**4 步编译流水线**：
1. 空组件 → `EmptyComponent()`
2. BREAK → `LoopBreakComponent()`
3. 特殊组件 → `_compile_special_component()` 异步处理
4. 标准组件 → 查 `COMPILER_HANDLERS` 字典 → `getattr(self, handler_name)` → 调用

### 7.3 11 种标准组件编译方法

```python
async def _compile_llm_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译LLM组件"""
    llm_compiler = LLMCompCompiler(comp.configs)
    return llm_compiler.compile()

async def _compile_question_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译提问者组件"""
    questioner_compiler = QuestionerCompCompiler(comp.configs)
    return questioner_compiler.compile()

async def _compile_intent_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译意图检测组件"""
    compiler = IntentDetectionCompCompiler(comp, workflow_dl.connections)
    return compiler.compile()

async def _compile_input_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译用户输入组件"""
    userinput_compiler = UserInputCompCompiler(comp.configs, comp.id)
    return userinput_compiler.compile()

async def _compile_output_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译用户输出组件"""
    useroutput_compiler = UserOutputCompCompiler(comp.id, comp.configs, comp.inputs, self.need_stream_output_comp)
    output_component, self.need_stream_output_comp = useroutput_compiler.compile()
    return output_component

async def _compile_text_editor_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译文本编辑器组件"""
    texteditor_compiler = TextEditorCompCompiler(comp.id, comp.configs, comp.outputs)
    return texteditor_compiler.compile()

async def _compile_variable_merge_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译变量合并组件"""
    varimerge_compiler = VariableMergeCompCompiler(comp.configs, comp.id)
    return varimerge_compiler.compile()

async def _compile_code_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译代码组件"""
    code_compiler = CodeCompCompiler(comp.id, comp.configs, workflow_dl.connections)
    return code_compiler.compile()

async def _compile_http_request_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译HTTP请求组件"""
    http_request_compiler = HttpRequestCompCompiler(comp.id, comp.configs, workflow_dl.connections)
    return http_request_compiler.compile()

async def _compile_react_agent_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译React智能体组件"""
    compiler = ReactAgentCompCompiler(
        comp.configs,
        plugin_mgr=self.plugin_mgr,
        workflow_mgr=self.workflow_mgr,
        space_id=self.space_id,
        current_user=self.current_user
    )
    result = await compiler.compile()
    return result

async def _compile_knowledge_retrieval_component(self, comp: Component, workflow_dl: BaseFlow):
    """编译知识检索组件"""
    kr_compiler = KnowledgeRetrievalCompCompiler(comp.id, comp.configs, self.space_id)
    return kr_compiler.compile()
```

**统一签名**：所有标准编译器都是 `async def _compile_X_component(self, comp, workflow_dl)` → 返回对应 Component 实例。

### 7.4 特殊组件编译 `_compile_special_component()`

```python
async def _compile_special_component(self, context: Context, workflow_dl: BaseFlow, comp: Component, loader):
    """处理特殊组件（保持原有逻辑）"""
    if comp.type == ComponentType.COMPONENT_TYPE_IF:
        branch_compiler = BranchCompCompiler(comp.id, comp.branches, workflow_dl.connections)
        return branch_compiler.compile()

    elif comp.type == ComponentType.COMPONENT_TYPE_SUB_WORKFLOW:
        return await self._create_exec_sub_workflow_component(context, comp.configs, loader, comp.id)

    elif comp.type == ComponentType.COMPONENT_TYPE_LOOP:
        return await self._create_loop_component(context, comp.configs, comp.outputs, loader)

    elif comp.type == ComponentType.COMPONENT_TYPE_PLUGIN:
        return await self._compile_plugin_component(comp)

    return None
```

**4 种特殊组件的特殊处理**：IF 走 `BranchCompCompiler`、SUB_WORKFLOW 需要 loader、LOOP 需要递归处理 body、PLUGIN 走 `ServiceTool/CodeTool/McpTool`。

### 7.5 主编译流水线 `compile()`

```python
async def compile(self, context: Context, loader: Optional[IWorkflowLoader] = None) -> InvokableWorkflow:
    card = WorkflowCard(
        id=self.id,
        version=self.version,
        name=self.name,
        input_params=self.inputs
    )
    flow = InvokableWorkflow(card=card)

    flow = await self.process_components(context, flow, self.dl_workflow, loader)
    flow = await self.process_stream_connections(flow)
    flow = await self.process_connections(flow, self.dl_workflow.connections)
    return flow
```

**3 步编译**：
1. `process_components` — 遍历所有组件，调用 `compile_component`
2. `process_stream_connections` — 处理流式连接（仅流式 Output 节点）
3. `process_connections` — 处理普通连接（跳过 branch_id 连接）

### 7.6 流式连接处理 `process_stream_connections()`

```python
async def process_stream_connections(self, flow: Any) -> Any:
    if not self.need_stream_output_comp:
        return flow
    for source_id, target_id in self.need_stream_output_comp.items():
        if isinstance(target_id, list):
            for tid in target_id:
                flow.add_stream_connection(source_id, tid)
        else:
            flow.add_stream_connection(source_id, target_id)
    return flow
```

**触发条件**：当 Output 组件设置了 `response_template` 或 `stream_output=True` 时，记录在 `self.need_stream_output_comp` 字典中。

### 7.7 普通连接处理 `process_connections()`

```python
async def process_connections(self, flow: Any, dl_wf_connections: List[Any]) -> Any:
    for conn in dl_wf_connections:
        # 跳过分支连接，因为分支连接由组件内部的add_branch方法处理
        if conn.branch_id:
            continue
        # 检查 conn.source 是否包含 self.need_stream_output_comp 中的任何 key
        source_list = conn.source if isinstance(conn.source, list) else [conn.source]
        need_stream_sources = set(self.need_stream_output_comp.keys())
        has_stream_source = bool(set(source_list) & need_stream_sources)

        if has_stream_source:
            if isinstance(conn.source, list):
                for sid in conn.source:
                    flow = await self.do_add_connection(flow, sid, conn.target)
            else:
                flow = await self.do_add_connection(flow, conn.source, conn.target)
        else:
            flow.add_connection(conn.source, conn.target)
    return flow
```

**设计**：
- **跳过 branch_id**：分支由 `BranchCompCompiler.add_branch()` 内部处理
- **流式源**：如果 source 来自流式节点，走 `do_add_connection` 检查是否已加流式连接

### 7.8 子工作流创建 `_create_exec_sub_workflow_component()`

```python
async def _create_exec_sub_workflow_component(
        self, context: Context, configs: Any,
        loader: Optional[IWorkflowLoader], comp_id: str,
) -> Any:
    sub_wf_info = ExecSubWfConfig.model_validate(configs).sub_workflow_info
    sub_id = sub_wf_info.id
    sub_version = sub_workflow_info.version
    sub_workflow = await loader.get_compiled_workflow(
        Context(context), sub_id, sub_version, self.space_id, self.current_user
    )
    cache_stream = False
    if self.need_stream_output_comp and comp_id in self.need_stream_output_comp.keys():
        cache_stream = True

    return SubWorkflowComponent(sub_workflow, cache_stream=cache_stream)
```

**关键点**：子工作流通过 `loader`（即 `WorkflowRunner`）递归调用 `get_compiled_workflow`，实现嵌套子工作流。

### 7.9 循环组件创建 `_create_loop_component()`

```python
async def _create_loop_component(
        self, context: Context, comp_configs: Any, comp_outputs: Any,
        loader: Optional[IWorkflowLoader] = None
) -> Any:
    loop_group = LoopGroup()
    loop_config = LoopConfig.model_validate(comp_configs)
    loop_body = loop_config.loop_body
    loop_group.start_nodes(loop_body.start_id)
    loop_group.end_nodes(loop_body.end_id)
    loop_group = await self.process_components(context, loop_group, loop_body, loader)
    loop_group = await self.process_connections(loop_group, loop_body.connections)

    # 添加SetVariable组件
    for comp in loop_body.components:
        if comp.type == ComponentType.COMPONENT_TYPE_SET_VARIABLE:
            set_variable_config = SetVariableConfig.model_validate(comp.configs)
            set_variable_comp = LoopSetVariableComponent(set_variable_config.inter_variable)
            loop_group.add_workflow_comp(comp.id, set_variable_comp)

    return LoopComponent(loop_group, comp_outputs)
```

**递归**：循环体本身是一个 `BaseFlow`，复用 `process_components` + `process_connections`。

### 7.10 对 laew 借鉴

| 借鉴点 | laew 现状 | 建议实现 |
|--------|-----------|----------|
| **COMPILER_HANDLERS 注册表** | 3 个工具硬编码 | P0：实现 `ToolRegistry` + 注册表 |
| **11 种标准组件类型** | 1 种 (Bash/Read/Write 同质) | 暂时不需要 11 种，但模式可借鉴 |
| **空/特殊/标准三层分类** | 无 | 在 laew 中：`BuiltinTools` / `SpecialTools` / `UserTools` |
| **`getattr` 反射调用** | - | Rust 用 `HashMap<String, Box<dyn ToolCompiler>>` + 静态分发 |

**Rust 实现示例**：
```rust
pub struct ToolRegistry {
    compilers: HashMap<String, Box<dyn ToolCompiler>>,
}

pub trait ToolCompiler: Send + Sync {
    fn compile(&self, config: &ToolConfig) -> Result<Box<dyn Tool>>;
}

impl ToolRegistry {
    pub fn register(&mut self, name: &str, compiler: Box<dyn ToolCompiler>) {
        self.compilers.insert(name.to_string(), compiler);
    }

    pub fn compile(&self, name: &str, config: &ToolConfig) -> Result<Box<dyn Tool>> {
        self.compilers.get(name)
            .ok_or(AgentError::UnsupportedTool(name.to_string()))?
            .compile(config)
    }
}
```

---

## 8. 对 laew 的核心机制借鉴路线图

### 8.1 P0（核心能力 — 必须实现）

#### P0-1: 工具注册表 + JSON 序列化缓存

```rust
// src/agent/tools/registry.rs
use std::collections::HashMap;
use std::sync::Arc;

type CompilerFn = Arc<dyn Fn(&ToolConfig) -> Result<Box<dyn Tool>> + Send + Sync>;

pub struct ToolRegistry {
    compilers: HashMap<String, CompilerFn>,
    // 三维缓存：{user_id: {tool_key: (config_json, instance)}}
    instances: HashMap<(String, String), (String, Arc<dyn Tool>)>,
}

impl ToolRegistry {
    pub fn compile_or_get_cached(
        &mut self,
        user_id: &str,
        tool_key: &str,
        config: &ToolConfig,
    ) -> Result<Arc<dyn Tool>> {
        let config_json = serde_json::to_string(config)?;
        let key = (user_id.to_string(), tool_key.to_string());

        if let Some((cached_json, instance)) = self.instances.get(&key) {
            if cached_json == &config_json {
                return Ok(instance.clone());
            }
        }

        // 重新编译
        let compiler = self.compilers.get(tool_key)
            .ok_or(AgentError::UnsupportedTool(tool_key.to_string()))?;
        let instance: Arc<dyn Tool> = Arc::from(compiler(config)?);

        self.instances.insert(key, (config_json, instance.clone()));
        Ok(instance)
    }
}
```

#### P0-2: 知识库集成（多 KB + RAG 注入 prompt）

借鉴 `AgentRunner.run()` 的 KB 检索前移模式：

```rust
// 在 YoloRunner 处理用户输入时，检索相关 KB
async fn preprocess_with_kb(&self, query: &str, kb_ids: &[String]) -> Vec<String> {
    let mut results = vec![];
    for kb_id in kb_ids {
        if let Some(embedding) = self.embedding_model.embed(query).await.ok() {
            if let Some(hits) = self.vector_store.search(kb_id, &embedding, 5).await.ok() {
                for hit in hits {
                    results.push(hit.content);
                }
            }
        }
    }
    // 注入 system message
    results
}
```

#### P0-3: MCP Client（5 种传输）

```rust
#[async_trait]
pub trait McpClient: Send + Sync {
    async fn connect(&mut self) -> Result<()>;
    async fn list_tools(&self) -> Result<Vec<McpToolCard>>;
    async fn call_tool(&self, name: &str, args: Value) -> Result<Value>;
    async fn disconnect(&mut self);
}

pub enum McpTransport {
    Stdio,
    Sse,
    StreamableHttp,
    OpenApi,
    Playwright,
}

pub struct PluginMcpTool {
    conf: McpConfig,
    transport: McpTransport,
}
```

### 8.2 P1（增强能力 — 重要）

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

```rust
pub enum NamespaceFlags {
    User = 1, Ipc = 2, Pid = 4, Net = 8, Uts = 16, Cgroup = 32,
}

pub struct BubbleWrapSandbox {
    config: SandboxConfig,
}

impl Sandbox for BubbleWrapSandbox {
    async fn run(&self, code: &str, lang: Lang, timeout_ms: u64) -> Result<ExecutionResult> {
        // 1. 创建临时目录
        let workdir = tempdir()?;
        // 2. 加载 seccomp BPF
        let bpf = self.load_seccomp()?;
        // 3. 拼接 bwrap 命令（namespace + mount + seccomp + cmd）
        let cmd = self.build_bwrap_cmd(&workdir, &bpf, code, lang);
        // 4. 执行（带 timeout kill）
        let result = tokio::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .kill_on_drop(true)
            .output_timeout(timeout_ms)
            .await?;
        // 5. 检测 retcode=159 → "Bad syscall detected."
        if result.retcode == 159 {
            return Ok(ExecutionResult::bad_syscall(result));
        }
        Ok(result)
    }
}
```

#### P1-3: 提示词版本管理（草稿+提交）

```rust
// SQLite 表
// prompts (id, key UNIQUE, content, version='draft', created_by, created_at, updated_at)
// prompt_commits (prompt_id, version, content_snapshot, commit_msg, created_at)

pub struct PromptService {
    draft_repo: PromptUserDraftRepo,
    commit_repo: PromptCommitRepo,
}
```

### 8.3 P2（高级能力 — 可选）

#### P2-1: Workflow 编排（Pregel + cba 算法）

```rust
// 用 petgraph crate
use petgraph::graph::DiGraph;

pub struct WorkflowEngine {
    graph: DiGraph<ComponentNode, Connection>,
    compilers: HashMap<ComponentType, Box<dyn ComponentCompiler>>,
}

impl WorkflowEngine {
    pub async fn compile(&self, workflow: &WorkflowDefinition) -> Result<CompiledWorkflow> {
        // 1. 转换为 DiGraph（MultiDiGraph 用 petgraph::graph::DiGraph with edges Vec）
        // 2. 应用 cba 消减算法
        // 3. 拓扑排序
        // 4. 编译组件
    }

    pub async fn execute(&self, inputs: Value) -> impl Stream<Chunk> {
        // Pregel 超步执行
    }
}
```

#### P2-2: DSL 转换器（抽象基类 + 工厂）

```rust
pub trait WorkflowConverter: Send + Sync {
    fn convert(&self, json: Value) -> Result<WorkflowImportResult>;
}

pub struct ConverterFactory;

impl ConverterFactory {
    pub fn create(format: WorkflowFormat) -> Box<dyn WorkflowConverter> {
        match format {
            WorkflowFormat::Native => Box::new(NativeWorkflowConverter),
            WorkflowFormat::N8n => Box::new(N8nWorkflowConverter),
        }
    }
}
```

#### P2-3: 评估指标 — Pass@k + Pass^k

```rust
pub fn compute_pass_at_k(results: &[TrialResult], k: usize) -> f64 {
    // pass@k = 1 - C(n-c, k) / C(n, k)
    let n = results.len();
    let c = results.iter().filter(|r| r.passed).count();
    if n < k { return 0.0; }
    1.0 - (comb(n - c, k) / comb(n, k))
}

pub fn compute_pass_pow_k(results: &[TrialResult], k: usize) -> f64 {
    // pass^k = C(c, k) / C(n, k)
    let n = results.len();
    let c = results.iter().filter(|r| r.passed).count();
    if n < k { return 0.0; }
    comb(c, k) / comb(n, k)
}

fn comb(n: usize, k: usize) -> f64 {
    if k > n { return 0.0; }
    (0..k).fold(1.0, |acc, i| acc * (n - i) as f64 / (i + 1) as f64)
}
```

### 8.4 总结：核心机制映射表

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

## 附录：核心类与函数深度索引

| 模块 | 核心类/函数 | 代码路径 | 行数 | 关键设计 |
|------|-------------|----------|------|----------|
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

---

> **总结**：openJiuwen Studio 在 7 个核心机制层面都有**生产级**的实现：AgentRunner 的三维 JSON 缓存、WorkflowRunner 的双重取消机制、PregelGraphAdapter 的 cba 消减算法、5 种 MCP 传输、BubbleWrap + Seccomp 多层隔离、4 扰动 × N trial 评估矩阵、11 种组件注册表。对 laew 而言，**P0 应优先实现**工具注册表、知识库注入、MCP Client；**P1** 重点放在 LLM-as-Judge、BubbleWrap 沙箱、提示词版本管理；**P2** 推进 Workflow 编排、DSL 转换、Pass@k 指标。