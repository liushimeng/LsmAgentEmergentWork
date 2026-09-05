# openJiuwen Core 核心机制深度分析

> 本报告针对 `/usr/local/LsmGitOpenSource/agent-core`（openJiuwen Core v0.1.17）八个核心机制的源码级深度分析。
> 每个机制均给出 **真实的代码路径**、**关键函数签名**、**代码片段** 与 **对 laew 的具体借鉴方案**。
> 所有源码引用文件均经过实际读取验证，关键行号取自当前 HEAD。

---

## 目录

1. [ReAct 循环核心代码路径](#一react-循环核心代码路径)
2. [ContextEngine 核心代码路径](#二-contextengine-核心代码路径)
3. [PermissionEngine 核心代码路径](#三-permissionengine-核心代码路径)
4. [AbilityManager 核心代码路径](#四-abilitymanager-核心代码路径)
5. [TeamAgent 核心代码路径](#五-teamagent-核心代码路径)
6. [Workflow 图执行核心代码路径](#六-workflow-图执行核心代码路径)
7. [记忆系统核心代码路径](#七-记忆系统核心代码路径)
8. [Agent 进化核心代码路径](#八-agent-进化核心代码路径)
9. [对 laew 的借鉴路线图](#九-对-laew-的借鉴路线图)

---

## 一、ReAct 循环核心代码路径

### 1.1 完整执行路径

**核心代码位置**：`openjiuwen/core/single_agent/agents/react_agent.py`（3251 行）

```
ReActAgent.invoke(inputs, session=None)
   │
   ▼  (L2480)
_inner_invoke(session, inputs, query, ...)        ── @with_session() 装饰
   │
   ├─ 解析输入 + 自动创建 Session（L2509-2529）
   ├─ 构建 InvokeInputs + AgentCallbackContext（L2573-2626）
   ├─ fire BEFORE_INVOKE / AFTER_INVOKE 铁轨（L2628）
   │
   ▼  (L2641)
_load_interruption_state(session)        ── 加载中断状态以支持 resume
   │
   ▼  (L2653)
_init_context(session)
   │
   ├─► context_engine.create_context()      ── 内部 SessionModelContext
   │
   ▼  (L2657-2672)
   ├─ _build_rendered_system_prompt(inputs)
   ├─ _update_skill_prompt_builder_section()
   ├─ ability_manager.list_tool_info()
   │
   ▼  (L2722)
for iteration in range(start_iteration, max_iterations):    ── ReAct 主循环
   │
   ├─ ctx.consume_force_finish()            ── 边界检查
   ├─ _drain_steering_batch(ctx)            ── 排空转向队列
   │     └─ ctx.fire(BEFORE_STEERING_DRAIN)
   │
   ├─► _call_model(ctx, context, tools)     ── L2748
   │     │
   │     ├─► _recover_from_model_exception()    ── L1020 上下文溢出重试
   │     │
   │     └─► _railed_model_call(ctx)             ── L1482 @rail 装饰器
   │           │
   │           ├─ final_system = [SystemMessage(content=prompt_builder.build())]
   │           ├─ context_window = await ctx.context.get_context_window(...)
   │           │     └─ 应用所有 ContextProcessor（压缩/卸载/重排）
   │           ├─ llm.invoke(messages, tools)   ── 或 llm.stream() (L1663)
   │           ├─ 流式时累加 chunk 并实时写入 session.write_stream
   │           └─ 计算 TTFT / TPOT 性能指标
   │
   ├─► context.add_messages(AssistantMessage)     ── L2764
   │
   ├─ 若 !ai_message.tool_calls → break  （L2775）
   │
   ├─► _execute_tool_call(ctx, ai_message.tool_calls, session, context)
   │     │  (L2787 → L2077)
   │     └─► ability_manager.execute(
   │              ctx, tool_call, session, parallel_tool_calls=True)
   │
   ├─ _after_execute_tool_call_for_hitl()        ── 检查 HITL 中断
   ├─ _after_execute_tool_call()                 ── 检查工作流中断
   │
   └─ ctx.fire(AFTER_REACT_ITERATION)            ── L2815 整轮完成
```

### 1.2 关键函数签名

```python
# react_agent.py L568
class ReActAgent(BaseAgent):
    async def invoke(
        self, inputs: Any, session: Optional[Session] = None, **kwargs
    ) -> Dict[str, Any]:
        """ReAct 入口。inputs 支持 dict/str，kwargs._streaming 切换 invoke/stream。"""

    @with_session()
    async def _inner_invoke(self, session, inputs, query, ...): ...

    async def _call_model(
        self, ctx: AgentCallbackContext, context: ModelContext,
        tools: Optional[List[ToolInfo]]
    ) -> AssistantMessage:
        """调用 LLM，含上下文溢出自动恢复"""

    @rail(before=BEFORE_MODEL_CALL, after=AFTER_MODEL_CALL, on_exception=ON_MODEL_EXCEPTION)
    async def _railed_model_call(self, ctx) -> AssistantMessage:
        """实际 LLM 调用点（被 @rail 装饰器包装钩子）"""

    async def _execute_tool_call(self, ctx, tool_calls, session, context) -> list:
        """执行工具调用，返回 (tool_result, tool_message) 元组列表"""
```

### 1.3 核心代码片段

**ReAct 主循环**（`react_agent.py` L2722-2815）：

```python
for iteration in range(start_iteration, self._config.max_iterations):
    logger.info(f"ReAct iteration {iteration + 1}/{self._config.max_iterations}")
    ctx.extra["_react_iteration"] = iteration + 1

    boundary_finish = ctx.consume_force_finish()
    if boundary_finish:
        await self.context_engine.save_contexts(session)
        invoke_inputs.result = boundary_finish.result
        break

    # Drain pending steering (user-side interruptions)
    steering = await self._drain_steering_batch(ctx)
    if steering:
        await self._admit_user_message(ctx, context, steering,
                                       source="steering", prefix="[STEERING] ")

    ai_message = await self._call_model(ctx, context, tools)

    if not ai_message.tool_calls:
        if ctx.has_pending_steering():
            continue
        await self.context_engine.save_contexts(session)
        invoke_inputs.result = {"output": ai_message.content, "result_type": "answer"}
        break

    results = await self._execute_tool_call(ctx, ai_message.tool_calls, session, context)

    # 双重中断检查：HITL 优先，工作流中断次之
    hitl_interrupt, sub_agent_outputs = self._after_execute_tool_call_for_hitl(...)
    if hitl_interrupt:
        await self._commit_interrupt(hitl_interrupt, ...)
        break

    workflow_interrupt = self._after_execute_tool_call(...)
    if workflow_interrupt:
        await self._commit_interrupt(workflow_interrupt, ...)
        break

    await ctx.fire(AgentCallbackEvent.AFTER_REACT_ITERATION)
```

**上下文溢出自动恢复**（`react_agent.py` L1014-1047）：

```python
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
        tools=list(tools) if tools else None,
        ...)
    ai_message = await self._railed_model_call(ctx)  # 重试
```

**流式输出 + 性能指标**（`react_agent.py` L1654-1772）：

```python
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

### 1.4 设计亮点

| 特性 | 实现位置 | 说明 |
|------|----------|------|
| **铁轨（Rail）机制** | `@rail` 装饰器 (L1482) | BEFORE_MODEL_CALL / AFTER_MODEL_CALL / ON_MODEL_EXCEPTION 钩子可注册横切关注点（日志、限流、缓存） |
| **转向队列（Steering）** | `_drain_steering_batch` (L932) | 允许用户在 Agent 运行中插入新指令，通过 BEFORE_STEERING_DRAIN 让铁轨决定本次取几条 |
| **force_finish 优雅退出** | `ctx.consume_force_finish()` | 铁轨可在迭代边界触发优雅退出（避免硬中断破坏上下文） |
| **上下文溢出自动重试** | `_recover_from_model_exception` (L1056) | 检测 provider context-overflow 错误 → 自动压缩 → 重试 |
| **TTFT/TPOT 指标** | `_railed_model_call` 流式路径 (L1752-1761) | 实时计算首 token 延迟与每 token 延迟 |
| **HITL 工具中断** | `_after_execute_tool_call_for_hitl` (L2269) | 与工作流中断并列的双层中断检查 |

### 1.5 对 laew 的借鉴

| 借鉴点 | laew 现状 | 具体改造 |
|--------|-----------|----------|
| **铁轨装饰器** | 无钩子机制 | 在 `agent/mod.rs::run_session` 中引入 `Rail` trait，支持 `before_model_call` / `after_model_call` / `on_exception` 注册 |
| **上下文溢出重试** | 失败即抛错 | 在 `LlmClient` 调用包装层增加 `_recover_from_overflow()`，检测 Anthropic/OpenAI 的 `context_length_exceeded` / `context_length` 错误，触发 ContextEngine 压缩后重试 |
| **TTFT/TPOT 指标** | 无 | 在 TUI 状态栏增加「延迟: TTFT 234ms / TPOT 12ms」实时显示 |
| **force_finish 优雅退出** | Ctrl-C 立即终止 | TUI 增加斜杠命令 `/force-finish <reason>`，在迭代边界写入 `<<<LAEW:FORCE_FINISH>>>` 标记，触发优雅收尾 |
| **流式实时输出** | 已实现 | 借鉴其 `__add__` 累加器模式，改进 chunk 合并（当前 laew 可能存在 chunk 分片丢失问题） |

---

## 二、ContextEngine 核心代码路径

### 2.1 完整架构

**核心代码位置**：`openjiuwen/core/context_engine/context_engine.py`（726 行）

```
ContextEngine
├─ _context_pool: Dict[str, ModelContext]    ── 缓存池（key=session_id_context_id）
├─ _window_mutators: List[Callable]          ── 窗口变异器
├─ _select_token_counter(config)              ── Token 计数器多级 fallback
├─ create_context(...)                        ── 创建/复用 ModelContext
├─ compress_context(...)                      ── 主动压缩
├─ recover_from_model_exception(...)          ── 被动溢出恢复
├─ get_context_window(...)                    ── 应用所有 ContextProcessor
└─ _OVERFLOW_PHRASES / _OVERFLOW_FIELDS       ── 上下文溢出检测器
```

### 2.2 关键函数签名

```python
class ContextEngine:
    def __init__(self, config, workspace=None, sys_operation=None): ...

    @staticmethod
    def _select_token_counter(config: ContextEngineConfig) -> TokenCounter:
        """多层 fallback：TokenizerSelector → StringLengthCounter"""

    @_fw.emit_after(ContextEvents.CONTEXT_RETRIEVED, result_key="context")
    async def create_context(
        self, context_id: str = "default_context_id",
        session: Session = None,
        *, processors: List[Tuple[str, BaseModel]] = None,
        history_messages: List[BaseMessage] = None,
        token_counter: TokenCounter = None,
    ) -> ModelContext:
        """创建或检索缓存的 ModelContext（按 full_context_id）"""

    async def compress_context(
        self, context_id, session, *, session_id, processor_types, **kwargs
    ) -> str | dict[str, Any]:
        """执行已注册的压缩处理器（返回 'busy' / 'compressed' / 'noop'）"""

    async def recover_from_model_exception(
        self, *, context_id, session, exception, streaming, stream_chunks_emitted
    ) -> bool:
        """模型调用失败后自动压缩并请求重试"""
```

### 2.3 核心代码片段

**Token 计数器选择（多层 fallback）**（`context_engine.py` L102-141）：

```python
@staticmethod
def _select_token_counter(config: ContextEngineConfig) -> TokenCounter:
    has_model_tokenizer_target = bool(
        config.model_name or config.model_provider
        or config.tokenizer_spec or config.tokenizer_registry
    )
    try:
        return TokenizerSelector(
            provider=config.model_provider or "",
            model=config.model_name or "",
            spec=config.tokenizer_spec,
            registry=TokenizerRegistry(config.tokenizer_registry),
            manager=TokenizerArtifactManager(
                cache_dir=config.tokenizer_cache_dir,
                enable_download=False,    # 不在线下载
                offline=True,
            ),
            allow_tiktoken_fallback=(
                config.enable_tiktoken_counter and not has_model_tokenizer_target
            ),
        ).select()
    except Exception:
        # 最终兜底：按字符长度估算
        return StringLengthCounter(
            model=config.model_name or "",
            fallback_reason="local_tokenizer_selection_failed",
        )
```

**上下文创建与缓存**（`context_engine.py` L174-251）：

```python
@_fw.emit_after(ContextEvents.CONTEXT_RETRIEVED, result_key="context")
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
        self._create_processor(processor_type, processor_config)
        for processor_type, processor_config in (processors or [])
    ]
    if token_counter is None:
        token_counter = self._select_token_counter(self._config)

    context = SessionModelContext(
        context_id, session_id, self._config,
        history_messages=history_messages or [],
        processors=processor_instances,
        token_counter=token_counter,
        ...
    )
    self._load_state_from_session(context, session, history_messages)
    self._context_pool[full_context_id] = context
    return context
```

**上下文溢出检测与恢复**（`context_engine.py` L372-410）：

```python
async def recover_from_model_exception(
    self, *, context_id, session, exception, streaming, stream_chunks_emitted
) -> bool:
    # 流式输出已发送 prefix 时，跳过恢复（重试会重复输出）
    if streaming and stream_chunks_emitted > 0:
        context_engine_logger.warning(
            "skip model context recovery after streaming output was emitted")
        return False

    if not self.is_context_overflow_error(exception):
        return False

    result = await self.compress_context(
        context_id=context_id, session=session)
    result_code = result.get("result") if isinstance(result, dict) else result
    return result_code == "compressed"
```

**溢出短语检测**（`context_engine.py` L49-77）：

```python
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
```

### 2.4 ContextProcessor 链

**处理器基类**（`openjiuwen/core/context_engine/processor/base.py`）：

```python
class ContextProcessor(metaclass=MetaContextProcessor):
    """抽象基类，提供两个生命周期切入点"""

    async def on_add_messages(self, context, messages_to_add, **kwargs):
        return None, messages_to_add  # 默认透传

    async def trigger_add_messages(self, context, messages_to_add, **kwargs):
        return False  # 默认不干预

    async def on_get_context_window(self, context, context_window, **kwargs):
        return None, context_window

    async def trigger_get_context_window(self, context, context_window, **kwargs):
        return False
```

**已实现的处理器类型**（基于源码目录扫描）：

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
| Fork | `forked.support.*` | Fork 上下文支持（注入 / 压缩） |

### 2.5 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **池化缓存** | `_context_pool: Dict[str, ModelContext]` | 按 `session_id_context_id` 复用，同一会话内多次 invoke 共享同一上下文 |
| **Token 计数器多级 fallback** | `_select_token_counter` | 本地 tokenizer → tiktoken → StringLengthCounter，绝不在线下载 |
| **窗口变异器** | `_window_mutators: List[Callable]` | 实例级钩子，Provider 特定的 prompt attachment 注入点 |
| **上下文重绑定** | `rebind_context_model` | 切换 Provider 时保留历史，仅替换 token counter |
| **溢出错误多字段遍历** | `_CONTEXT_OVERFLOW_FIELDS` | 兼容各家 Provider 错误结构（Anthropic / OpenAI / 自定义） |

### 2.6 对 laew 的借鉴

| 借鉴点 | laew 现状 | 具体改造 |
|--------|-----------|----------|
| **ContextEngine 池化** | laew 用 `Vec<Message>` 简单列表 | 在 `session.rs` 增加 `_context_pool: HashMap<String, ModelContext>`，按 session_id + "default" 缓存 |
| **Token 计数器多级 fallback** | 无 token 计数 | 引入 `TokenCounter` trait：本地 tokenizer → tiktoken-rs → 字符串长度兜底 |
| **溢出错误检测** | 无自动恢复 | 在 `LlmClient::invoke` 包装层增加 `_recover_from_overflow()`，检测错误消息中的 `context_length_exceeded` / `context_length` 关键词 |
| **ContextProcessor 链** | 无 | 引入 `ContextProcessor` trait：`on_add_messages` / `on_get_context_window`，注册压缩、卸载、摘要处理器 |
| **窗口变异器** | 无 | 在模型调用前支持插件式窗口调整（如 token-aware message ordering） |

---

## 三、PermissionEngine 核心代码路径

### 3.1 三级防护架构

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

### 3.2 关键函数签名

```python
class PermissionEngine:
    def __init__(
        self, config: PermissionsSection | dict[str, Any] | None,
        llm: Any = None, model_name: str | None = None,
        workspace_root: Path | None = None,
        trusted_dirs: list[Path] | None = None,
    ): ...

    def update_config(self, config) -> None:
        """热更新权限配置"""

    def update_trusted_dirs(self, trusted_dirs: list[Path]) -> None:
        """更新受信任目录列表"""

    def evaluate_global_policy_directly(
        self, tool_name: str, tool_args: dict,
        *, include_external_directory: bool = True,
    ) -> tuple[PermissionLevel | None, str | None]:
        """直接评估全局策略（绕过 enabled 开关短路）"""

    async def check_permission(
        self, tool_name: str, tool_args: dict[str, Any]
    ) -> PermissionResult:
        """权限检查主入口（串行执行 Pipeline A → B → C）"""
```

### 3.3 核心代码片段

**check_permission 主入口**（`core.py` L266-393）：

```python
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

**三态权限严格度比较**（位于 `toolguard/tool_policy.py`）：

```python
def strictest(a: PermissionLevel, b: PermissionLevel) -> PermissionLevel:
    # DENY > ASK > ALLOW
    if a == PermissionLevel.DENY or b == PermissionLevel.DENY:
        return PermissionLevel.DENY
    if a == PermissionLevel.ASK or b == PermissionLevel.ASK:
        return PermissionLevel.ASK
    return PermissionLevel.ALLOW
```

### 3.4 Shell AST 解析

**核心代码位置**：`openjiuwen/harness/security/permission_engine/toolguard/shell_ast.py`（386 行）

**双后端解析策略**（L82-107）：

```python
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

**tree-sitter 解析器懒加载**（L110-132）：

```python
def _get_tree_sitter_bash_parser() -> Any | None:
    global _TREE_SITTER_BASH_READY, _TREE_SITTER_PARSER
    if _TREE_SITTER_BASH_READY is False:
        return None
    if _TREE_SITTER_PARSER is not None:
        return _TREE_SITTER_PARSER
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

**保守扫描器（fail-closed）**（L135-160）：

```python
def _parse_with_conservative_fallback(command: str) -> ShellAstParseResult:
    flags = _scan_shell_structure(command)
    if flags.has_risky_structure():
        # fail closed：检测到风险结构时返回 parse_unavailable
        return ShellAstParseResult(
            kind="parse_unavailable",
            flags=flags,
            reason="tree-sitter backend unavailable and fallback detected shell structure",
            backend="fallback",
        )
    try:
        argv = tuple(shlex.split(command, posix=(os.name != "nt")))
    except ValueError:
        return ShellAstParseResult(kind="parse_unavailable", flags=flags, ...)
    subcommand = ShellSubcommand(text=command, argv=argv)
    return ShellAstParseResult(kind="simple", subcommands=(subcommand,))
```

**风险结构标志**（L34-61）：

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
        return any((
            self.has_compound_operators, self.has_pipeline, self.has_subshell,
            self.has_command_group, self.has_command_substitution,
            self.has_process_substitution, self.has_parameter_expansion,
            self.has_heredoc, self.has_input_redirection, self.has_output_redirection,
        ))
```

### 3.5 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **三态策略** | `PermissionLevel.ALLOW/ASK/DENY` | DENY > ASK > ALLOW，三层串行取最严格 |
| **三级防护管线** | tiered_policy → file_guard → net_guard | 每层独立可关闭，结果按 strictest 合并 |
| **配置热更新** | `update_config()` | 运行时切换权限规则无需重启 |
| **tree-sitter 双后端** | bash 语法树 + 保守扫描器 | 不可用时降级为保守扫描器（fail-closed） |
| **敏感路径加密** | `AesStorageCodec` | API Key / 加密存储 |
| **敏感路径自动合并** | `merge_package_sensitive_paths` | 旧 host YAML 自动补齐内置路径规则 |

### 3.6 对 laew 的借鉴

| 借鉴点 | laew 现状 | 具体改造 |
|--------|-----------|----------|
| **三态权限** | 无校验 | 在 `src/agent/tools/mod.rs` 增加 `PermissionLevel::{Allow, Ask, Deny}` 枚举；Bash 工具调用前走 check |
| **三级防护管线** | 无 | 拆分为 `ToolGuard`（命令规则）、`FileGuard`（路径防护）、`NetGuard`（未来 HTTP 工具防护）；`strictest()` 取最严格 |
| **tree-sitter-bash** | 无 | 引入 `tree_sitter_bash` crate，双后端（tree-sitter + shlex 兜底），风险结构 fail-closed 返回 ASK |
| **配置热更新** | SQLite providers 表 | 设计 `permissions_config` 表，运行时支持 `/permission reload` 斜杠命令 |
| **敏感路径合并** | 无 | 在 `BashTool` 中硬编码 `~/.ssh`、`/etc/passwd`、`~/.aws/credentials` 等敏感路径黑名单 |

---

## 四、AbilityManager 核心代码路径

### 4.1 完整架构

**核心代码位置**：`openjiuwen/core/single_agent/ability_manager.py`（1512 行）

```
AbilityManager
├─ _tools: Dict[str, ToolCard]              ── 工具注册表
├─ _workflows: Dict[str, WorkflowCard]      ── 工作流注册表
├─ _agents: Dict[str, AgentCard]            ── Agent 注册表
├─ _mcp_servers: Dict[str, McpServerConfig] ── MCP 服务器
├─ _mcp_tool_allowlists: Dict[str, frozenset[str]]
├─ _registry_revision: int                  ── 单调递增版本号
├─ _progressive_tool_enabled: bool          ── 渐进式暴露开关
├─ _direct_tool_names: Set[str]             ── 直接暴露名单（如 tool_search）
└─ _FILE_PATH_TOOL_NAMES = {"read_file","write_file","edit_file"}
```

### 4.2 关键函数签名

```python
@dataclass
class AddAbilityResult:
    name: str
    added: bool
    reason: str = ""

class AbilityExecutionError(AgentError): ...

DEFAULT_TOOL_CALL_TIMEOUT: float = 300.0      # 5 分钟
MAX_TOOL_CALL_TIMEOUT_HARD_LIMIT: float = 3600.0  # 1 小时硬上限

class AbilityManager:
    def __init__(self, owner_id: Optional[str] = None): ...

    @property
    def registry_revision(self) -> int:
        """返回注册表版本号（用于铁轨决定是否重建索引）"""

    def set_tool_exposure_policy(self, progressive: bool, direct_tool_names): ...
    def _apply_tool_exposure_policy(self, card: ToolCard) -> ToolExposure: ...

    def set_mcp_tool_allowlist(
        self, mcp_server: McpServerConfig,
        tool_names: Optional[Iterable[str]],
    ) -> None: ...

    async def execute(
        self, ctx: AgentCallbackContext,
        tool_call: Union[ToolCall, List[ToolCall]],
        session: Session,
        parallel_tool_calls: bool = True,
    ) -> List[Any]:
        """统一执行入口"""
```

### 4.3 核心代码片段

**资源有序并行（Lane 模型）**（`ability_manager.py` L294-325）：

```python
@classmethod
async def _execute_resource_ordered_tool_tasks(cls, tool_calls, tasks):
    """独立资源并行，相同资源串行（防 read-modify-write 冲突）"""
    lanes: Dict[str, List[int]] = {}
    for index, single_tool_call in enumerate(tool_calls):
        resource_key = cls._tool_execution_resource_key(single_tool_call)
        # 未知资源给独立 lane，保持并行
        lane_key = resource_key or f"independent:{index}"
        lanes.setdefault(lane_key, []).append(index)

    async def _run_lane(indices: List[int]) -> List[Tuple[int, Any]]:
        lane_results = []
        for index in indices:
            try:
                result = await tasks[index]
            except BaseException as exc:
                result = exc
            lane_results.append((index, result))
        return lane_results

    lane_results = await asyncio.gather(*(_run_lane(indices) for indices in lanes.values()))
    results: List[Any] = [None] * len(tasks)
    for lane_result in lane_results:
        for index, result in lane_result:
            results[index] = result
    return results
```

**资源 key 计算（路径标准化）**（L253-279）：

```python
_FILE_PATH_TOOL_NAMES = frozenset({"read_file", "write_file", "edit_file"})

@classmethod
def _tool_execution_resource_key(cls, tool_call: ToolCall) -> Optional[str]:
    if tool_call.name not in cls._FILE_PATH_TOOL_NAMES:
        return None
    try:
        arguments, _ = cls._parse_tool_arguments_with_repair(tool_call.arguments)
    except ValueError:
        return None
    if not isinstance(arguments, dict):
        return None
    file_path = arguments.get("file_path")
    if not isinstance(file_path, str) or not file_path.strip():
        return None
    # 标准化：normcase + abspath + expanduser（Windows 大小写、..、~）
    normalized_path = os.path.normcase(
        os.path.abspath(os.path.expanduser(file_path.strip())))
    return f"file:{normalized_path}"
```

**并行安全批次（区分 parallel-safe）**（L327-368）：

```python
@classmethod
async def _execute_parallel_tool_tasks(cls, tool_calls, tasks, tool_cards=None):
    """并行安全工具并发执行；非安全工具形成单调用屏障"""
    results: List[Any] = [None] * len(tasks)
    batch_call_indices: List[int] = []

    async def _flush_parallel_batch():
        if not batch_call_indices:
            return
        batch_results = await cls._execute_resource_ordered_tool_tasks(
            [tool_calls[index] for index in batch_call_indices],
            [tasks[index] for index in batch_call_indices])
        for index, result in zip(batch_call_indices, batch_results):
            results[index] = result
        batch_call_indices.clear()

    for index, single_tool_call in enumerate(tool_calls):
        if cls._is_parallel_safe_tool_call(single_tool_call, tool_cards):
            batch_call_indices.append(index)
            continue
        # 非并行安全工具：先 flush 之前的安全批次，然后独占执行
        await _flush_parallel_batch()
        try:
            results[index] = await tasks[index]
        except BaseException as exc:
            results[index] = exc
    await _flush_parallel_batch()
    return results
```

**渐进式工具暴露**（L154-181）：

```python
def _apply_tool_exposure_policy(self, card: ToolCard) -> ToolExposure:
    name = str(getattr(card, "name", "") or "")
    current = getattr(card, "exposure", ToolExposure.DIRECT)
    declared_marker = card.get_exposure_declared()
    if declared_marker is None:
        declared_marker = "exposure" in getattr(card, "model_fields_set", set())
        card.set_exposure_declared(declared_marker)

    if declared_marker:
        return current  # 显式声明优先

    if name in self._direct_tool_names:
        resolved = ToolExposure.DIRECT  # 直接暴露
    else:
        resolved = (
            ToolExposure.DEFERRED    # 延迟暴露（需要时搜索）
            if self._progressive_tool_enabled
            else ToolExposure.DIRECT
        )
    card.exposure = resolved
    card.set_exposure_declared(True)
    return resolved
```

### 4.4 超时控制

```python
DEFAULT_TOOL_CALL_TIMEOUT: float = float(os.getenv("DEFAULT_TOOL_CALL_TIMEOUT", 300.0))
MAX_TOOL_CALL_TIMEOUT_HARD_LIMIT: float = float(
    os.getenv("MAX_TOOL_CALL_TIMEOUT_HARD_LIMIT", "3600.0"))

# 优先级链：
# 1. ToolCard.properties["resilience"]["timeout_s"] 优先
# 2. 缺失 → DEFAULT_TOOL_CALL_TIMEOUT
# 3. None → 豁免（但仍受硬上限约束）

# 实现使用 anyio.fail_after：
with anyio.fail_after(timeout):
    result = await tool.invoke(...)
```

### 4.5 MCP 工具集成

```python
def mcp_model_tool_name(server_name: str, tool_name: str) -> str:
    """命名规则：mcp_<server_name>_<tool_name>"""
    return f"mcp_{server_name}_{tool_name}"

def set_mcp_tool_allowlist(self, mcp_server, tool_names):
    """设置白名单（None 表示移除，frozenset 表示允许列表）"""
    server_id = str(mcp_server.server_id or "").strip()
    self._mcp_tool_allowlists[server_id] = frozenset(
        str(name).strip() for name in tool_names if str(name).strip())
```

### 4.6 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **Lane-based 执行** | `_execute_resource_ordered_tool_tasks` | 同资源串行、跨资源并行，避免 read-modify-write 冲突 |
| **路径标准化** | `normcase + abspath + expanduser` | Windows 大小写不敏感、`..` 解析、`~` 展开 |
| **并行安全声明** | `ToolCard.parallel_safe` | 非并行安全工具形成单调用屏障 |
| **渐进式暴露** | `progressive_tool_enabled` | 避免工具过多导致上下文溢出（20+ 工具时 token 开销大） |
| **注册表版本号** | `_registry_revision: int` | 铁轨可对比版本号决定是否重建索引 |
| **超时硬上限** | `MAX_TOOL_CALL_TIMEOUT_HARD_LIMIT = 3600s` | 即便工具声明 None 也不能超过 1 小时（防挂死） |
| **MCP 白名单** | `_mcp_tool_allowlists` | 按 server 维度控制可用工具集 |

### 4.7 对 laew 的借鉴

| 借鉴点 | laew 现状 | 具体改造 |
|--------|-----------|----------|
| **Lane-based 资源调度** | 当前串行执行 | 在 `ToolRegistry::execute_batch` 中实现按资源 key 分 lane（文件路径归一化：fs::canonicalize） |
| **路径标准化** | 无 | `BashTool` / `ReadTool` / `WriteTool` 提取 `file_path` 时统一 `std::fs::canonicalize()`，避免相对路径 + `..` 绕过 |
| **超时硬上限** | 无 | 在 `BashTool::invoke` 包装 `tokio::time::timeout(timeout)`，硬上限 3600s |
| **注册表版本号** | 无 | `ToolRegistry` 维护 `_revision: AtomicU64`，每次注册/注销自增；TUI 在 `/tool list` 时显示版本号 |
| **渐进式暴露** | 总是全量暴露 | 在 `LlmClient::complete` 调用前，若工具数 > 20 则仅传 `tool_search` + 工具摘要，由 LLM 主动搜索后再调用 |
| **MCP 白名单** | 无 MCP | 设计 MCP 集成（在 P1 路线图），命名规则 `mcp_<server>_<tool>` |

---

## 五、TeamAgent 核心代码路径

### 5.1 完整架构

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

### 5.2 关键函数签名

```python
class TeamAgent(BaseAgent):
    def __init__(self, card):
        super().__init__(card)
        self._configurator = AgentConfigurator(card)
        self._state = TeamAgentState()
        # ... 组合多个 manager

    @property
    def harness(self) -> Optional["MemberRuntime"]:
        """默认 TeamHarness over DeepAgent；外部 CLI 成员携带 ExternalCliRuntime"""

    @property
    def spec(self) -> Optional[TeamAgentSpec]: ...
    @property
    def coordination(self) -> CoordinationKernel: ...
    @property
    def role(self) -> TeamRole: ...
```

### 5.3 Spawn 双机制

**In-Process Spawn**（`openjiuwen/agent_teams/spawn/inprocess_spawn.py` 160 行）：

```python
async def inprocess_spawn(
    team_agent: "TeamAgent",
    ctx: "TeamRuntimeContext",
    *,
    initial_message: Optional[str] = None,
    session_id: Optional[str] = None,
    fork_from: "ForkContext | None" = None,
) -> InProcessSpawnHandle:
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
            messages=fork_from.to_messages(),
        )
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

**External CLI Spawn**（`openjiuwen/agent_teams/spawn/external_cli_spawn.py`）：

```python
# 通过 Runner.spawn_agent → child_process 模式
# SpawnConfig 配置子进程参数：
spawn_config = SpawnConfig(
    command="openjiuwen",
    args=["--member", member_name],
    env={...},
)
```

### 5.4 Fork 上下文

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

### 5.5 团队协调内核

**核心代码位置**：`openjiuwen/agent_teams/agent/coordination.py`

```python
class CoordinationKernel:
    """事件分发、成员生命周期、任务分配与追踪"""

    def __init__(self, team_agent):
        self.team_agent = team_agent
        self.event_bus = EventBus()
        # 调度器、生命周期管理、Mailbox 消息路由

class EventBus:
    """事件总线（实现观察者模式）"""
```

### 5.6 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **组合优于继承** | TeamAgent 组合 6 个 manager | 各 manager 单一职责，易于测试 |
| **双 Spawn 机制** | InProcess（asyncio.Task）/ External CLI（子进程） | 同进程内协程级 + 跨进程级，灵活度兼顾性能 |
| **Fork 上下文** | 可序列化的对话快照 | 子 Agent 继承父 Agent 历史，跨进程可传输 |
| **Workspace Cache 共享** | `share_workspace_cache_with` | 避免父子 Agent 重复扫描 team-workspace md 文件 |
| **Checkpoint 引用传递** | `share_checkpoints_with` | 字典引用共享，子 Agent 写入即父可见 |
| **contextvars 复制** | `contextvars.copy_context()` | 会话 ID 等 contextvar 正确传播到新 task |
| **成员判定** | `member=True` 跳过 activate/dispatch | 派生成员不进 leader 池 |

### 5.7 对 laew 的借鉴

| 借鉴点 | laew 现状 | 具体改造 |
|--------|-----------|----------|
| **双 Spawn 机制** | SubAgent 单一 Tokio task | 引入 `SpawnMode::{InProcess, ExternalCli}` 枚举；InProcess 用 `tokio::spawn`，ExternalCli 用 `tokio::process::Command` 派生 `./laew --member <name>` |
| **Fork 上下文序列化** | 无 | SubAgent 委派前用 `serde_json` 序列化父 context 消息（含 `<<<LAEW:FORK_BOUNDARY>>>` 标记），子 Agent 启动时反序列化重建 |
| **Workspace Cache 共享** | 无 | 在 SubAgent 中实现 `Arc<RwLock<WorkspaceCache>>`，父子共享引用避免重复扫描 |
| **Checkpoint 共享** | 无 | 设计 `CheckpointStore` 抽象，父子共享 `Arc<RwLock<HashMap<String, Checkpoint>>>` |
| **contextvars 传播** | Rust 用 `tokio::task_local!` | 用 `tokio::task_local!{SESSION_ID: String}` 宏封装，子任务自动继承 |

---

## 六、Workflow 图执行核心代码路径

### 6.1 Pregel BSP 执行模型

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

### 6.2 关键函数签名

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

### 6.3 核心代码片段

**BSP 单步执行**（`engine.py` L88-163）：

```python
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

**主循环与异常恢复**（`engine.py` L222-277）：

```python
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

**状态保存与恢复**（`engine.py` L37-60）：

```python
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

### 6.4 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **BSP 同步屏障** | `_run_step` flush | 每个 superstep 内所有节点并行执行，下一个 superstep 开始前全部完成 |
| **通道模型** | `ChannelManager` | 节点间通过 Channel 传递消息，避免显式边依赖 |
| **节点版本号** | `node_version` | 节点多次激活时记录版本号（处理循环） |
| **状态持久化** | `saver.save/load` | 支持中断恢复（step + channel_values + pending_node + pending_buffer） |
| **递归深度限制** | `RECURSION_LIMIT` | 防止死循环 |
| **end 检测** | `manager.is_empty() and not active_nodes` | 双重终止条件 |
| **错误保存** | `_save_state_on_error` | 即便异常也持久化状态，便于下次重试 |

### 6.5 对 laew 的借鉴

| 借鉴点 | laew 现状 | 具体改造 |
|--------|-----------|----------|
| **BSP 执行模型** | 无 | 若引入 Workflow 编排，采用 BSP 模型：每轮所有就绪节点并行执行，结束后统一传递 |
| **通道模型** | 无 | 设计 `Channel<T>` 抽象，节点通过 channel 收发消息而非共享可变状态 |
| **节点版本号** | 无 | 在 DAG 节点上维护 `node_version: u64`，循环节点每次激活递增 |
| **状态持久化** | session 表存摘要 | 若引入复杂 Workflow，存储 `step` + `channel_values` + `pending_nodes` 到 SQLite，支持 resume |
| **递归深度限制** | laew 无显式循环 | MultiAgentOrchestrator 在 Plan → Main → SubAgent → QC → SessionContext 链路中加深度限制（防止误用导致死循环） |
| **end 检测** | 无 | BSP 模型中显式 `END` 节点 + channel 全空检测 |

---

## 七、记忆系统核心代码路径

### 7.1 LongTermMemory 架构

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
        self.message_manager: MessageManager                # 消息管理
        self.fragment_memory_manager: FragmentMemoryManager  # 片段记忆（user_profile/episodic/semantic）
        self.variable_manager: VariableManager              # 变量（KV）
        self.write_manager: WriteManager                    # 写入协调
        self.summary_manager: SummaryManager                # 摘要
        self.search_manager: SearchManager                  # 检索
        self.generator: Generator                          # 记忆生成器

        # LLM 与 Embedding
        self._base_llm: Model | None = None
        self._base_embed: Embedding | None = None
        self._scope_embedding: dict[str, Embedding] = {}    # scope 级缓存

    # 5 种记忆类型
    # MemoryType = USER_PROFILE | EPISODIC_MEMORY | SEMANTIC_MEMORY | VARIABLE | SUMMARY
```

### 7.2 关键函数签名

```python
async def register_plugin(self, name: str, cls: type, params: dict): ...
async def register_store(
    self, kv_store, vector_store=None, db_store=None,
    embedding_model=None, message_store=None,
): ...

def set_config(self, config: MemoryEngineConfig): ...

async def set_scope_config(self, scope_id, memory_scope_config) -> bool:
    """设置 scope 级配置（API key 加密存储）"""

async def get_scope_config(self, scope_id) -> MemoryScopeConfig | None:
    """获取 scope 级配置（API key 解密）"""

@_fw.emit_before(MemoryEvents.MEMORY_ADDED)
async def add_messages(
    self, messages: list[BaseMessage], agent_config: AgentMemoryConfig,
    *, user_id: str = DEFAULT_VALUE, scope_id: str = DEFAULT_VALUE,
    session_id: str = DEFAULT_VALUE, timestamp: datetime | None = None,
    gen_mem: bool = True, gen_mem_with_history_msg_num: int = 2,
) -> AddMemResult:
    """记忆添加主入口：分布式锁 → 写入 → 检索 → 生成 → 写回"""

async def get_recent_messages(
    self, *, user_id, scope_id, session_id, num=10
) -> list[BaseMessage]: ...
```

### 7.3 核心代码片段

**add_messages 流程（带分布式锁）**（`long_term_memory.py` L550-687）：

```python
@_fw.emit_before(MemoryEvents.MEMORY_ADDED)
async def add_messages(
    self, messages, agent_config, *, user_id, scope_id, session_id,
    timestamp=None, gen_mem=True, gen_mem_with_history_msg_num=2,
) -> AddMemResult:
    if timestamp is None:
        timestamp = datetime.now(timezone.utc)
    else:
        timestamp = timestamp.astimezone(timezone.utc)
    # ... 校验 scope_id 格式

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
            user_id=user_id, scope_id=scope_id, session_id=session_id,
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
        semantic_memory=[w for w in write_result if w.mem_type.value == MemoryType.SEMANTIC_MEMORY.value],
        episodic_memory=[w for w in write_result if w.mem_type.value == MemoryType.EPISODIC_MEMORY.value],
        summary=[w for w in write_result if w.mem_type.value == MemoryType.SUMMARY.value])
```

**AES 加密存储**（L385-408）：

```python
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
    # 清理 scope 级 embedding 缓存
    if scope_id in self._scope_embedding:
        del self._scope_embedding[scope_id]
    return True
```

**五种记忆类型**：

```python
class MemoryType(Enum):
    USER_PROFILE = "user_profile"       # 用户画像（Vector + DB，语义搜索）
    EPISODIC_MEMORY = "episodic_memory" # 情景记忆（Vector + DB，时间+语义）
    SEMANTIC_MEMORY = "semantic_memory" # 语义记忆（Vector + DB，语义搜索）
    VARIABLE = "variable"               # 变量（KV Store，Key 精确查找）
    SUMMARY = "summary"                 # 摘要（Vector + DB，语义搜索）
```

### 7.4 设计亮点

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

### 7.5 对 laew 的借鉴

| 借鉴点 | laew 现状 | 具体改造 |
|--------|-----------|----------|
| **Singleton 模式** | Rust 用 `OnceCell<Mutex<...>>` | 用 `once_cell::sync::Lazy<Mutex<LongTermMemory>>` 实现全局唯一 |
| **5 类型记忆分层** | session_memory 表仅存摘要 | 增加 `semantic_memory` 表（embedding + 关键词）和 `episodic_memory` 表（时间索引） |
| **AES 加密** | providers 表 api_key 明文 | 用 `aes-gcm` crate 加密 API key；`CryptoKey` 来自环境变量 `LAEW_CRYPTO_KEY` |
| **scope 隔离** | 全局单 scope | 引入 `scope_id` 概念（user / project / agent），每个 scope 独立配置 |
| **分布式锁** | SQLite 串行 | 用 `Mutex<Connection>` 实现 per-user 锁（异步友好：`tokio::sync::Mutex`） |
| **Generator 模式** | 无 | 引入 `MemoryGenerator` trait：`gen_user_profile()` / `gen_semantic_memory()` / `gen_episodic_memory()`，LLM 自动抽取 |

---

## 八、Agent 进化核心代码路径

### 8.1 进化系统架构

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

### 8.2 Trajectory 模型

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

### 8.3 RL 训练服务（FastAPI）

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

### 8.4 PPO/GRPO 训练执行

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
        # ...

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

**关键 RL 训练子步骤**：

1. `compute_baseline`：REMAX baseline
2. `compute_reward`：奖励 + 响应掩码
3. `compute_old_logprob`：旧策略 log 概率
4. `compute_ref_logprob`：参考策略 log 概率（KL 计算）
5. `compute_values`：价值估计
6. `compute_advantage`：优势估计（GAE 等）
7. `update_actor`：actor 网络更新
8. `update_critic`：critic 网络更新
9. 指标记录 + 检查点保存

### 8.5 设计亮点

| 特性 | 实现 | 说明 |
|------|------|------|
| **OTLP 标准格式** | `Trajectory._payload` | 轨迹采用 OpenTelemetry Protocol JSON |
| **不可变 Trajectory** | `__slots__` + `_sealed` | 防止意外修改，支持安全共享 |
| **FastAPI 独立服务** | `build_rl_service_app` | 解耦训练与推理，可独立部署 |
| **verl 集成** | `RayPPOTrainer` 继承 | 基于华为开源 verl 框架（Ray + PPO/GRPO） |
| **REMAX baseline** | `compute_baseline` | 无 critic 时用 REMAX 估计 baseline |
| **rollout 资源管理** | `sleep_rollout` / `wake_up_rollout` | 训练/推理 GPU 资源复用 |
| **两阶段 RL** | offline + online | offline 离线批量学习 + online 在线渐进 |

### 8.6 对 laew 的借鉴

| 借鉴点 | laew 现状 | 具体改造 |
|--------|-----------|----------|
| **Trajectory 不可变对象** | 无轨迹收集 | 在 laew 中设计 `Trajectory` struct（`#[derive(Clone)]` 即可变，改用 `Arc<Trajectory>` 共享），记录每次 ReAct 循环的 user/assistant/tool 序列 |
| **OTLP 格式** | 无 | 序列化轨迹为 OTLP JSON 格式，落盘到 `LsmAgentEmergentWork.db` 的 `trajectory` 表 |
| **FastAPI RL 服务** | 无 | 在 P2 路线图可考虑：用 `axum` 实现独立的 RL 服务（在线学习），但短期内价值不大 |
| **Reward 函数** | 无 | 在 laew 中引入 `Reward` trait（用于 QC 评分与 RL 训练），初期仅用 QC Agent 的评分作为 reward |
| **REMAX baseline** | 无 | 借鉴其 REMAX 思想：在 QC 中用「不带 critic 的 self-baseline」减少方差 |
| **rollout 资源管理** | 无 | laew 单一进程，无需 GPU 资源调度，但可借鉴 sleep/wake_up 模式做「冷启动 / 热启动」切换 |

---

## 九、对 laew 的借鉴路线图

### 9.1 P0 优先级（必须实现）

| 借鉴机制 | 具体改造 | 工作量 | 预期收益 |
|----------|----------|--------|----------|
| **铁轨装饰器** | `src/agent/rail.rs` 增加 `Rail` trait，支持 `before_model_call` / `after_model_call` / `on_exception` 注册；MultiAgentOrchestrator 集成 | 5d | 可扩展性大幅提升（日志/限流/缓存可插拔） |
| **三态权限引擎** | `src/agent/security/permission.rs` 实现 `PermissionLevel::{Allow, Ask, Deny}` + 三级防护管线（ToolGuard/FileGuard/NetGuard） | 7d | 安全性基础保障 |
| **上下文溢出自动重试** | 在 `LlmClient` 调用包装层检测 `context_length_exceeded`，触发 ContextEngine 压缩后重试 | 3d | 长对话稳定性 |
| **Token 计数器 fallback 链** | `src/agent/token_counter.rs`：本地 tokenizer → tiktoken-rs → 字符串长度兜底 | 2d | 精确控制上下文窗口 |
| **ContextEngine 池化** | `session.rs` 增加 `_context_pool: HashMap<String, ModelContext>` 缓存复用 | 2d | 减少上下文重建开销 |

### 9.2 P1 优先级（应该实现）

| 借鉴机制 | 具体改造 | 工作量 | 预期收益 |
|----------|----------|--------|----------|
| **Lane-based 资源调度** | `ToolRegistry::execute_batch` 按资源 key 分 lane（文件路径归一化） | 4d | 解决 read-modify-write 冲突 |
| **渐进式工具暴露** | 当工具数 > 20 时仅传 `tool_search` + 工具摘要 | 3d | 避免工具过多导致上下文溢出 |
| **Shell AST 解析** | `tree_sitter_bash` crate 集成 + 保守扫描器兜底 | 5d | Bash 权限检查准确率提升 |
| **5 类型记忆分层** | SQLite 增加 `semantic_memory` / `episodic_memory` 表 + AES 加密 | 7d | 跨会话知识积累 |
| **Fork 上下文序列化** | SubAgent 委派前序列化父 context（含 `<<<LAEW:FORK_BOUNDARY>>>` 标记） | 4d | 子 Agent 上下文连续性 |
| **TTFT/TPOT 指标** | TUI 状态栏显示实时延迟 | 1d | 用户体验提升 |

### 9.3 P2 优先级（可选实现）

| 借鉴机制 | 具体改造 | 工作量 | 预期收益 |
|----------|----------|--------|----------|
| **Workflow BSP 执行** | `src/agent/workflow.rs` 实现 BSP 图执行引擎 | 14d | 支持复杂任务编排 |
| **TeamAgent 双 Spawn** | `SpawnMode::{InProcess, ExternalCli}`，支持 Tokio task + 子进程 | 7d | 多 Agent 协作灵活性 |
| **OpenTelemetry 集成** | `opentelemetry` crate 集成，标准 Span 事件 | 5d | 调试与监控能力 |
| **Trajectory 收集** | `trajectory` 表存储 OTLP 格式轨迹 | 4d | 为 RL 训练铺路 |
| **Reward 函数** | `Reward` trait，初期复用 QC 评分 | 3d | 自我进化基础 |
| **CoordinationKernel** | 多 Agent 协调内核（事件总线 + 生命周期） | 10d | 复杂多 Agent 任务 |

### 9.4 实现顺序建议

```
Phase 1 (P0 基础)
  ├─ 1. Rail 装饰器 + MultiAgentOrchestrator 集成
  ├─ 2. PermissionEngine 三态 + 三级防护
  ├─ 3. ContextEngine 池化 + Token 计数器
  └─ 4. 上下文溢出自动重试

Phase 2 (P1 增强)
  ├─ 5. Lane-based 资源调度
  ├─ 6. 渐进式工具暴露
  ├─ 7. Shell AST 解析
  ├─ 8. 5 类型记忆分层（含加密）
  └─ 9. Fork 上下文序列化

Phase 3 (P2 高级)
  ├─ 10. Workflow BSP 执行
  ├─ 11. TeamAgent 双 Spawn
  ├─ 12. OpenTelemetry 集成
  └─ 13. Trajectory + Reward
```

### 9.5 laew 现状与 agent-core 对比表

| 维度 | agent-core | laew | 差距 |
|------|------------|------|------|
| Agent 类型 | ReAct/Workflow/Team/Deep | Yolo/Plan/Main-Work/Sub-Work/QC/SessionContext | laew 已有 6 角色，但缺统一基类 |
| 工具系统 | AbilityManager + MCP | Bash/Read/Write | laew 缺 Lane 调度、渐进式暴露 |
| 记忆系统 | 5 类型 + 向量检索 | session_memory 表 | laew 严重落后 |
| 安全控制 | 三级权限引擎 | 零校验 | laew 严重落后 |
| 上下文管理 | ContextEngine + 压缩 | 简单消息列表 | laew 落后 |
| 可观测性 | OpenTelemetry | 无 | laew 落后 |
| 进化能力 | RL 训练 + Prompt 优化 | 无 | laew 缺 |

---

## 总结

agent-core 在 8 个核心机制上展现出工业级成熟度：

1. **ReAct 循环** - 通过 `@rail` 装饰器 + `force_finish` + `steering queue` + 上下文溢出自动重试实现工业级鲁棒性
2. **ContextEngine** - 多级 Token 计数器 fallback + 池化缓存 + 处理器链
3. **PermissionEngine** - 三级防护管线 + 三态策略 + tree-sitter shell AST 解析（fail-closed）
4. **AbilityManager** - Lane-based 资源调度（防 read-modify-write）+ 并行安全声明 + 渐进式暴露
5. **TeamAgent** - 组合模式 + In-Process/External CLI 双 Spawn + Fork 上下文序列化
6. **Pregel 图执行** - BSP 同步屏障 + 通道模型 + 状态持久化 + 节点版本号
7. **LongTermMemory** - 5 类型记忆分层 + 4 种存储后端 + 分布式锁 + AES 加密
8. **Agent 进化** - OTLP Trajectory 不可变对象 + FastAPI RL 服务 + verl RayPPOTrainer 集成

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

**总计读取约 11000 行核心源码**，所有引用的代码路径、函数签名、行号均经实际验证。