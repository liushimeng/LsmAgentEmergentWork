# Hermes Agent 核心机制深度分析（第二轮）

- 分析日期：2026-09-04
- 源码根路径：`/usr/local/LsmGitOpenSource/hermes-agent`
- 核心文件数：约 200 个 Python 源文件 + 5109 个总 Python 文件（含 tests）
- 分析范围：`run_agent.py`、`agent/conversation_loop.py`、`agent/conversation_compression.py`、`tools/registry.py`、`model_tools.py`、`agent/turn_context.py`、`agent/memory_manager.py`、等核心运行时
- 前置文档：`hermes-agent-源码调研.md`（第一轮分析）、`hermes-agent-深度分析.md`（对应框架对比文档中 hermes 条目）

---

## 专题 1：Agent 主循环 —— 单层 while + 多闸门

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `run_agent.py:467` `class AIAgent` | ~300 方法的 AIAgent 主类 |
| `run_agent.py:9238` `AIAgent.run_conversation()` | 顶层入口（立即转发） |
| `agent/conversation_loop.py:2026` `run_conversation()` | 真正的状态机主体（9285 行） |
| `agent/conversation_loop.py:2289` | 核心 `while` 循环 |
| `agent/turn_context.py` | 每轮 setup prologue（1709 行） |
| `agent/iteration_budget.py` | IterationBudget 类 |

### 1.1 单层 while + 4 类闸门

`agent/conversation_loop.py:2289` 是核心循环骨架：

```python
while (api_call_count < agent.max_iterations
       and agent.iteration_budget.remaining > 0) \
      or agent._budget_grace_call:
    # 闸门 1: redirect 排水（修改 original_user_message）
    _redirect_text = agent._drain_pending_redirect()
    if _redirect_text:
        _apply_active_turn_redirect(agent, messages, _redirect_text)
        if isinstance(original_user_message, str):
            original_user_message = (
                f"{original_user_message}\n\n"
                f"User correction during the turn: {_redirect_text}"
            )
        agent._persist_session(messages, conversation_history)

    # 重置每轮 checkpoint dedup
    agent._checkpoint_mgr.new_turn()

    # 闸门 2: interrupt 检查
    if agent._interrupt_requested:
        interrupted = True
        _turn_exit_reason = "interrupted_by_user"
        if not agent.quiet_mode:
            agent._safe_print("\n⚡ Breaking out of tool loop due to interrupt...")
        break

    # 闸门 3: review input budget 检查（detached auxiliary forks）
    if _review_input_budget_exhausted(agent):
        _turn_exit_reason = "review_input_budget_exhausted"
        break

    api_call_count += 1
    agent._api_call_count = api_call_count
    agent._touch_activity(f"starting API call #{api_call_count}")

    # 闸门 4: budget grace call（用尽后给一次机会）
    if agent._budget_grace_call:
        agent._budget_grace_call = False  # consume the grace flag
    elif not agent.iteration_budget.consume():
        _turn_exit_reason = "budget_exhausted"
        if not agent.quiet_mode:
            agent._safe_print(f"\n⚠️  Iteration budget exhausted ({agent.iteration_budget.used}/{agent.iteration_budget.max_total} iterations used)")
        break

    # fire step_callback for gateway hooks（agent:step event）
    if agent.step_callback is not None:
        agent.step_callback(api_call_count, prev_tools)

    # /steer drain：pre-API-call 注入（保护 role alternation）
    _pre_api_steer = agent._drain_pending_steer()
    if _pre_api_steer:
        _injected = False
        for _si in range(len(messages) - 1, -1, -1):
            _sm = messages[_si]
            if isinstance(_sm, dict) and _sm.get("role") == "tool":
                from agent.prompt_builder import format_steer_marker
                marker = format_steer_marker(_pre_api_steer)
                existing = _sm.get("content", "")
                if isinstance(existing, str):
                    _sm["content"] = existing + marker
                _injected = True
                break
```

### 1.2 IterationBudget + Grace Call 双机制

`agent/iteration_budget.py`（基于 `run_agent.py:162` import）：

```python
from agent.iteration_budget import IterationBudget
```

设计核心：

- `consume()` 失败时设 `_budget_grace_call = True` —— 给"一次机会"完成当前 tool batch
- 进入下一次循环时 consume grace flag，**保证**这次循环结束后必退出
- "温柔终止"机制 —— 模型可以完成当前 tool batch（如 close file / save state），不硬切断

`agent/conversation_loop.py:2949/3071/3132/3188/7295` 多处 `agent.iteration_budget.refund()` —— 失败 refund（不浪费预算）：

```python
agent.iteration_budget.refund()  # dropped tool call 不消耗预算
```

### 1.3 Interrupt ABI 兼容性（双方法）

`run_agent.py:3525` `def interrupt(self, message=None, *, hard_cancel=False, tool_reason=None)`：

```python
def interrupt(
    self,
    message: Optional[str] = None,
    *,
    hard_cancel: bool = False,
    tool_reason: Optional[str] = None,
) -> None:
    """Legacy interrupt ABI — kwargs added later for hard-stop semantics."""
    self._interrupt_requested = True
    self._interrupt_message = message
    self._tool_interrupt_reason = tool_reason
    ...
    getattr(self, "_hard_interrupt_requested", threading.Event()).set()
    ...
```

`run_agent.py:3830` `def hard_interrupt(...)`：

```python
def hard_interrupt(
    self,
    message: Optional[str] = None,
    *,
    tool_reason: Optional[str] = None,
) -> None:
    """Request an explicit stop while preserving `interrupt()` ABI.

    Frontends can feature-detect this method and fall back to the legacy
    `interrupt(message=None)` signature for synthetic or third-party agents.
    """
    # Deliberately bypass dynamic dispatch: subclasses written against the
    # legacy interrupt(message=None) ABI may override interrupt without the
    # newer keyword-only hard_cancel argument.
    AIAgent.interrupt(
        self,
        message,
        hard_cancel=True,
        tool_reason=tool_reason,
    )
```

设计要点：
- **硬中断** vs **软中断** —— 软中断让当前 tool 完成；硬中断立即 abort
- **绕过动态分派** —— 子类若 override `interrupt()`，可能错过新的 `hard_cancel` 参数
- 前端可探测 `hard_interrupt` 方法存在性，fallback 到旧 ABI

### 1.4 Steer / Redirect 双机制（不破坏 Prompt Caching）

`run_agent.py:3908` `def steer(self, text: str) -> bool`：

```python
def steer(self, text: str) -> bool:
    """Queue a steer message for the next tool batch.
    
    Unlike a redirect, steer piggybacks on the LAST tool message in the
    transcript — preserving role alternation and prompt cache stability.
    """
    ...
    self._pending_steer = text
    ...
```

`run_agent.py:3944` `def redirect(self, text: str) -> bool`：

```python
def redirect(self, text: str) -> bool:
    """Queue a redirect — user realizes mid-turn that direction was wrong.
    
    Unlike steer, redirect mutates original_user_message (folded into the
    turn-start user message), so subsequent API calls see the correction
    immediately.
    """
    ...
    self._pending_redirect = text
    ...
```

`run_agent.py:4044-4056` `_drain_pending_redirect()` / `_drain_pending_steer()`：

```python
def _drain_pending_redirect(self) -> Optional[str]:
    with getattr(self, "_pending_redirect_lock", threading.Lock()):
        text = self._pending_redirect
        self._pending_redirect = None
        return text

def _drain_pending_steer(self) -> Optional[str]:
    with getattr(self, "_pending_steer_lock", threading.Lock()):
        text = self._pending_steer
        self._pending_steer = None
        return text
```

关键差异：

| 行为 | 注入时机 | 注入位置 | 影响 prompt cache |
|------|---------|---------|----------------|
| `steer` | 下一次 tool batch 后 | append 到 last tool message | **不破坏**（append-only） |
| `redirect` | 下一次 API call 前 | 修改 `original_user_message` | 可能破坏（取决于 cache 边界） |
| `interrupt` | 立即 | 不注入 | 不影响 |

### 1.5 Durable Turn Lease（跨进程 Turn 锁）

`run_agent.py:9270` 注释：

> "Serialize the full load -> run -> flush region across Hermes processes. Gateway's asyncio lease closes alias routing inside one process; this durable lease covers Desktop, CLI resume, gateway, and background delivery processes sharing state.db."

```python
# run_agent.py:9307 — durable lease
_turn_db = getattr(self, "_session_db", None)
_durable_session_exists = False
if _turn_db is not None:
    try:
        _durable_session_exists = _turn_db.has_session_metadata(session_id)
    except Exception:
        _durable_session_exists = False
```

`hermes_state.py` 提供 `_acquire_process_permit()`（行 630）+ `_acquire_path_permit()`（行 639）+ `_reclaim_idle()`（行 646）—— 三类 lease 协作。

### 1.6 Turn Liveness 标记（避免 review 抢占）

`run_agent.py:9265` 注释：

> "Turn liveness for the deferred-review idle queue: a queued review must not dispatch into the settle gap between two quick prompts. Marked inside the try below so the balancing note_turn_finished in its finally covers every exit."

```python
try:
    _review_queue.note_turn_started()
    ...
finally:
    _review_queue.note_turn_finished()
```

### 对 laew 的借鉴价值

1. **多闸门单循环**：laew 的 `src/agent/mod.rs::run_session` 当前是简单 while + max_iterations。建议增加 budget / interrupt / steer 三类闸门。
2. **Grace call 机制**：laew 的 max_iterations 是硬切断。建议增加 "用尽后给一次机会" 让当前 tool batch 完成（避免脏退出）。
3. **Steer/Redirect 双机制**：laew 的 TUI 在 agent 运行中输入时，新消息会作为独立 user message 注入（破坏 prompt cache）。建议改为：
   - `steer()` append 到最后一个 tool message（piggyback）
   - `redirect()` 修改原 user_message
4. **Interrupt 双方法**：laew 可借鉴 hard_interrupt 与 interrupt 的双方法设计（前端可探测，fallback 旧 ABI）。
5. **Durable Turn Lease**：laew 当前是单进程，不需要跨进程锁。但如果未来多前端（TUI + CLI + Gateway）共享 session_memory 表，需要类似机制。
7. **review/turn liveness 标记**：laew 若增加后台 review，需要 `_review_queue.note_turn_started()` / `note_turn_finished()` 防止 review 抢占活态 turn。

---

## 专题 2：Tool Registry 与 Plugin Override 隔离

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `tools/registry.py` | 工具注册中心（auto-discovery + check_fn + plugin override） |
| `model_tools.py:1251` | `handle_function_call()` 中央分发器 |
| `model_tools.py:323` | `get_tool_definitions()` + cache |
| `toolsets.py` | Toolset 定义 + `_HERMES_CORE_TOOLS` |

### 2.1 Registry 类

`tools/registry.py:480`：

```python
class _PluginOverridePolicy:
    """Identity-bearing authorization record for one plugin generation."""

    __slots__ = ("allowed",)

    def __init__(self, allowed: bool) -> None:
        self.allowed = bool(allowed)
```

`tools/registry.py:213` `ToolEntry`：

```python
class ToolEntry:
    def __init__(self, name, toolset, schema, handler, check_fn,
                 requires_env, is_async, description, emoji,
                 max_result_size_chars=None, dynamic_schema_overrides=None):
        self.name = name
        self.toolset = toolset
        self.schema = schema
        self.handler = handler
        self.check_fn = check_fn
        self.requires_env = requires_env
        self.is_async = async
        self.description = description
        self.emoji = emoji
        self.max_result_size_chars = max_result_size_chars
        # Optional zero-arg callable returning a dict of schema overrides
        # applied at get_definitions() time. Use for fields that depend on
        # runtime config (e.g. delegate_task's description must reflect the
        # user's current delegation.max_concurrent_children / max_spawn_depth
        # so the model isn't told the wrong limits). The callable is invoked
        # on every get_definitions() call; results are merged shallow on top
        # of the base schema before the {"type": "function", ...} wrap.
        self.dynamic_schema_overrides = dynamic_schema_overrides
```

### 2.2 Auto-Discovery（AST 扫描）

`tools/registry.py:74-111`：

```python
def _is_registry_register_call(node: ast.AST) -> bool:
    """Return True when *node* is a `registry.register(...)` call expression."""
    ...

def _module_registers_tools(module_path: Path) -> bool:
    """Return True when the module contains a top-level `registry.register(...)` call.
    
    Modules that only call `registry.register()` inside a function are not picked up.
    Top-level imports are stripped first because they can shadow the local
    `registry` symbol and cause the actual register call to be missed.
    """
    ...

def discover_builtin_tools(tools_dir: Optional[Path] = None) -> List[str]:
    """Walk tools/ for top-level `registry.register()` calls, import those files."""
    ...
```

设计要点：

- **AST 扫描**而非 import-all —— 节省内存、加快启动
- **顶层调用**约束 —— `if __name__ == "__main__"` 里的 register 不会被发现
- **shadows-aware** —— 如果 import 后 `registry` 被遮蔽，能识别

### 2.3 Plugin Override 隔离

`tools/registry.py:623` `register_plugin_override_policy(self, ...)`：

```python
def register_plugin_override_policy(
    self,
    plugin_name: str,
    tool_name: str,
    allowed: bool,
) -> None:
    """Mark whether a plugin is allowed to override a core tool by name.

    Core tools win by default. A plugin may only override a core tool if
    its `plugin.yaml` declares `override_core_tool: true` AND the policy
    registry has been told this specific override is allowed.
    """
    ...
```

`tools/registry.py:670` `_plugin_override_allowed`：

```python
def _plugin_override_allowed(
    self,
    handler: Callable,
    tool_name: str,
) -> bool:
    """Whether the plugin that registered this handler may override a core tool.
    
    Returns True only if the plugin declared `override_core_tool: true` and
    the policy is recorded. Otherwise core wins.
    """
    ...
```

设计哲学：

- **Core 默认赢** —— plugin 不能默认覆盖 core tool
- **显式声明** —— plugin 必须 `override_core_tool: true` + policy 注册
- **身份记录** —— `_PluginOverridePolicy` 用 `__slots__` 记录

### 2.4 Check_fn TTL 缓存（防止外部探测抖动）

`tools/registry.py:282-450`：

```python
def no_cache_check_fn(fn: Callable) -> Callable:
    """Decorator marking a check_fn as bypass-cache. Use sparingly."""
    ...

def _prune_check_fn_caches(now: float) -> None:
    """Drop cached results older than the TTL (~30s) for transient-failure suppression.
    
    A single `subprocess.run([docker, "version"], timeout=5)` that times out
    under load returns False for one call, which would silently strip the
    entire terminal+file toolset from whatever agent is being built at that
    instant — most visibly a delegate_task subagent, which then reports
    "Tool read_file missing".
    """
    ...
```

设计要点：

- **TTL ~30 秒** —— 与"人类时标"对齐（用户改 env / credentials 几秒后生效）
- **抖动抑制** —— 同一次"docker daemon 暂时 hang"不会让整个 toolset 消失
- **scope-aware** —— `check_fn_cache_scope()` 区分 user / profile

### 2.5 Tool Defs Cache（带 LRU eviction）

`model_tools.py:323-415`：

```python
def get_tool_definitions(
    enabled_toolsets=None,
    disabled_toolsets=None,
    quiet_mode=False,
    skip_tool_search_assembly=False,
) -> List[Dict[str, Any]]:
    """Get tool definitions for model API calls with toolset-based filtering."""
    
    cache_key = None
    if quiet_mode:
        # ... build cache_key ...
        cache_key = (
            registry.current_scope_key(),
            frozenset(enabled_toolsets) if enabled_toolsets is not None else None,
            frozenset(disabled_toolsets) if disabled_toolsets else None,
            registry._generation,
            cfg_fp,  # config mtime fingerprint
            bool(os.environ.get("HERMES_KANBAN_TASK")),
            bool(skip_tool_search_assembly),
            _is_delegated_child_context(),
            _is_dispatcher_owned_worker(),
            profile_scope,
        )
        ...
        if cached is not None:
            global _last_resolved_tool_names
            _last_resolved_tool_names = [t["function"]["name"] for t in cached]
            return list(cached)
    
    result = _compute_tool_definitions(...)
    ...
    if quiet_mode and cache_key is not None:
        with _tool_defs_cache_lock:
            cached = _tool_defs_cache.get(cache_key)
            if cached is None:
                # LRU eviction
                if len(_tool_defs_cache) >= _TOOL_DEFS_CACHE_MAX:
                    _tool_defs_cache.pop(next(iter(_tool_defs_cache)))
                _tool_defs_cache[cache_key] = result
                cached = result
        return list(cached)
```

设计要点：

- **Cache key = registry.generation + toolset + config mtime** —— 任何变更都能感知
- **LRU eviction** —— 长生命周期 Gateway 进程不内存爆炸
- **Shallow copy** —— 防止下游 mutation poison cache
- **Mtime fingerprint** —— 无需为每个 writer 注册 invalidate 钩子

### 2.6 Coerce Tool Args（LLM 输出容忍）

`model_tools.py:856` `coerce_tool_args(tool_name, args)`：

```python
def coerce_tool_args(tool_name: str, args: Dict[str, Any]) -> Dict[str, Any]:
    """Coerce LLM string outputs to schema-declared types.
    
    LLMs often output "42" for a number schema, or "true" for boolean.
    Without coercion, downstream JSON parsing fails.
    """
    schema = registry.get_schema(tool_name)
    if schema is None:
        return args
    return _coerce_value(args, schema)
```

`_coerce_value(value, expected_type, schema)`（行 1062）—— 递归处理嵌套 dict / list。

### 2.7 Tool Search Bridge（懒加载应对 Schema 限制）

`model_tools.py:1310` `_return_bridge_result(result)`：

```python
def _return_bridge_result(result: Any) -> Any:
    _emit_post_tool_call_hook(
        function_name=function_name,
        function_args=function_args,
        result=result,
        ...
    )
    return result
```

`model_tools.py:1310-1400` 注释：

> "tool_search and tool_describe are pure catalog reads — handle them inline. tool_call is unwrapped to the underlying tool so that every downstream hook (pre/post, edit approval, guardrails) sees the real tool name, not the bridge."

这是 Hermes 应对 Anthropic schema size 限制的方案 —— 完整 schema 不进 context，模型按需查询。

### 对 laew 的借鉴价值

1. **AST 扫描的 auto-discovery**：laew 的 `src/agent/tools/mod.rs::builtin_registry()` 是手动注册。建议改为 AST 风格（或 build.rs 风格）的自动发现。
2. **Plugin override 隔离**：laew 无 plugin 体系。若引入，core tool 必须默认赢，plugin override 必须显式声明。
3. **Check_fn TTL 缓存**：laew 的 tool 无 check_fn（不需要外部资源探测）。但若引入 terminal backend（docker / ssh），需要类似机制防止 docker daemon 抖动。
4. **Tool defs cache**：laew 的工具 schema 是静态的，无 cache 必要。但若引入动态 schema（runtime 变化），需要 mtime fingerprint + LRU。
5. **Coerce tool args**：laew 的 `serde` 反序列化更严格，但 LLM 输出容忍度低。建议增加 `_coerce_value()` 类似机制（在 serde 之前做 string→typed 转换）。
6. **Tool Search Bridge**：laew 目前工具集小（3 个），不需要 bridge。但若未来扩展到 10+，建议提前设计 bridge（与 Anthropic schema size 限制对齐）。

---

## 专题 3：Context 压缩 + CompressionCommitFence

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `agent/conversation_compression.py`（6123 行） | 压缩流水线（fence / lease / fence / 并发控制） |
| `agent/context_compressor.py` | 压缩器主逻辑 |
| `agent/native_compaction.py` | 原生压缩（无 LLM 路径） |
| `agent/context_breakdown.py` | 上下文分解（UI 展示） |
| `docs/micro-compaction.md` | 微压缩设计文档 |
| `hermes_state.py` | SQLite + FTS5 持久化 |

### 3.1 CompressionCommitFence（核心并发原语）

`agent/conversation_compression.py:673-880` 是 Hermes 最复杂的并发原语：

```python
class CompressionCommitFence:
    """Fence prevents a stale async commit from corrupting later work.

    Deadlock-class: a worker holding the compression lock crashes; a new
    worker acquires the lock; the old worker's deferred commit fires and
    overwrites the new worker's in-progress state with a stale summary.
    The fence uses an attempt-generation counter to make that race a no-op:
    a commit only proceeds if the generation it was captured under still
    matches the current generation on the compressor.
    """
    def __init__(self, total_ceiling_seconds: float | None = None) -> None: ...
    def set_total_ceiling_seconds(self, seconds: float) -> None: ...
    def touch_progress(self) -> None: ...
    def progress_observed(self) -> bool: ...
    def deadline_exceeded(self) -> bool: ...
    def deadline_monotonic(self) -> float | None: ...
    def seconds_since_progress(self) -> float: ...
    def cancel_before_commit(self, cancel_event: Any = None) -> bool: ...
    def try_cancel_before_commit(self) -> Optional[bool]: ...
    def begin_commit(self, cancel_event: Any = None) -> bool: ...
    def finish_commit(self) -> None: ...
    def commit_in_flight(self) -> bool: ...
    def is_cancelled(self) -> bool: ...
    def retain_compression_lock_until_worker_done(self) -> None: ...
    def mark_commit_fenced(self) -> None: ...
```

`agent/conversation_compression.py:430-490`：

```python
def _claim_compressor_attempt(compressor: Any) -> int:
    """Capture current attempt generation and bump it for next caller."""
    ...

def _compressor_attempt_is_current(compressor: Any, generation: int) -> bool:
    """Check whether `generation` is still the active attempt.
    
    A worker whose attempt generation no longer matches has been preempted;
    its commit must not run.
    """
    ...

def _restore_compressor_attempt_state(
    compressor: Any,
    snapshot: dict[str, Any],
    *,
    generation: int,
) -> None:
    """Restore compressor state from snapshot, but ONLY if generation still matches."""
    ...
```

### 3.2 Cooldown Under Lease（跨进程锁）

`agent/conversation_compression.py:615-670` `_capture_authoritative_cooldown_under_lease(agent)`：

```python
def _capture_authoritative_cooldown_under_lease(agent) -> ...:
    """Capture the authoritative cooldown state under the cross-process lease.
    
    Multiple Hermes processes (Desktop + CLI + Gateway) may share the same
    state.db. Without coordination, each would independently decide to
    compress and serialize conflicting summaries. The lease makes ONE
    process the authoritative compressor at a time.
    """
    ...
```

### 3.3 Micro-Compaction（不调用 LLM 的轻量压缩）

`docs/micro-compaction.md` 描述：

> "Micro-compaction replaces repeated text without calling for an LLM summary; safe enough for mid-conversation because it preserves prompt cache stability."

具体实现（`agent/native_compaction.py`）：

- **去重** —— 重复 system 段落合并
- **truncation** —— 长 tool output 截断到 N 行
- **stripping** —— 移除 known-noise（如 `<think>...</think>` 残段）
- **不调用 LLM** —— 微秒级完成

### 3.4 Token 估算（双轨制）

`agent/conversation_compression.py`：

```python
def _pressure_with_real_floor(compressor: Any, rough_tokens: int) -> int:
    """Cross-check rough token estimate against provider-reported usage.
    
    Real provider usage wins when available (it's accurate); otherwise
    fall back to chars/4 heuristic.
    """
    ...
```

`agent/conversation_loop.py:686-720`：

```python
def _maybe_grow_local_window(agent, compressor, ...):
    """Grow the local model's context window if its current limit is below
    what's needed for the compaction target. Ollama / llama.cpp support
    context-extension via runtime flags.
    """
    ...
```

### 3.5 Compression Pipeline（6 阶段）

```
┌─────────────────────────────────────────────────────────┐
│ 1. preflight check（turn_context.py）                    │
│    └── 每轮开头判断是否需要压缩                           │
├─────────────────────────────────────────────────────────┤
│ 2. Cooldown Under Lease                                  │
│    └── _capture_authoritative_cooldown_under_lease      │
│    └── 跨进程 lease 锁（Desktop + CLI + Gateway）         │
├─────────────────────────────────────────────────────────┤
│ 3. Attempt Generation Claim                             │
│    └── _claim_compressor_attempt → generation           │
├─────────────────────────────────────────────────────────┤
│ 4. CompressionCommitFence                               │
│    └── 提交时 fence 防止覆盖                              │
├─────────────────────────────────────────────────────────┤
│ 5. LLM 摘要生成（_do_compact）                          │
│    └── 或 micro-compaction（无 LLM）                     │
├─────────────────────────────────────────────────────────┤
│ 6. Session DB 写入 + FTS5 索引更新                       │
│    └── hermes_state.py                                    │
└─────────────────────────────────────────────────────────┘
```

### 3.6 FTS5 跨会话搜索

`hermes_state.py` 提供 FTS5 全文索引：

```sql
-- 示意（实际在 hermes_state.py:SQL）
CREATE VIRTUAL TABLE messages_fts USING fts5(
    content,
    tokenize='porter unicode61'
);
```

`tools/session_search_tool.py` + `agent/memory_manager.py:433 class MemoryManager`：

```python
class MemoryManager:
    def __init__(self, *, external_prefetch_timeout=None) -> None:
        ...
        # Futures are tracked by durability class so shutdown can give writes
        # the time they need to flush.
    
    def add_provider(self, provider: MemoryProvider) -> None: ...
    def providers(self) -> List[MemoryProvider]: ...
    def get_provider(self, name: str) -> Optional[MemoryProvider]: ...
```

### 3.7 MemoryProvider 抽象

8 种内置实现（`AGENTS.md:759`）：

```python
# plugins/memory/honcho / mem0 / supermemory / byterover
# / hindsight / holographic / openviking / retaindb

# 每个实现 MemoryProvider ABC：
class MemoryProvider(ABC):
    def sync_turn(self, turn_messages: List[Dict]) -> None: ...
    async def prefetch(self, query: str) -> List[Dict]: ...
    def shutdown(self) -> None: ...
    def commit_memory_session(self, messages: List[Dict]) -> None: ...
    def sync_external_memory_for_turn(self, ...) -> None: ...
    def post_setup(self, hermes_home: Path, config: Dict) -> None: ...
```

### 对 laew 的借鉴价值

1. **CompressionCommitFence + generation counter**：laew 的 SQLite WAL 已提供并发控制，但无 generation counter。若引入压缩，需要防止"过期 commit 覆盖新工作"。建议在 `src/config/mod.rs::Db` 中加 `attempt_generation` 字段。
2. **Cooldown Under Lease**：laew 单进程，不需要跨进程锁。但若多前端（TUI + CLI）共享 session_memory 表，需要 lease。
3. **Micro-compaction（不调用 LLM）**：laew 当前无压缩。建议先引入 micro-compaction：
   - 重复 system 段落合并
   - 长 tool output 截断（已有部分）
   - 移除 `<think>...</think>` 残段
4. **Token 估算双轨制**：laew 无 token 估算。建议在 `src/agent/yolo.rs` 增加 `estimate_context_tokens()` —— 有真实 usage 时用真实值，否则 `chars/4` 启发式。
5. **FTS5 跨会话搜索**：laew 的 session_memory 用 SQLite，但**无 FTS5 索引**。建议在 session_memory 表加 FTS5 虚拟表，支持跨会话检索。这是 SQL 现成能力。
6. **MemoryProvider ABC**：laew 无 MemoryProvider ABC。建议增加 `src/agent/memory_provider.rs::MemoryProvider` trait + 至少 2 种实现（builtin file-based + honcho-style stub）。

---

## 专题 4：Skill 系统 —— agentskills.io 兼容 + User Message 注入

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `tools/skills_tool.py`（570 行） | Skill 发现 + 调用 |
| `agent/skill_commands.py` | 注入为 user message（保护 cache） |
| `agent/skill_preprocessing.py` | Skill 前处理 |
| `agent/skill_utils.py` | Skill 工具函数 |
| `tools/skills_guard.py` | Skill 守卫（dangerous frontmatter） |
| `tools/skills_ast_audit.py` | AST 审计 |
| `tools/skill_linter.py` | Lint |
| `tools/skill_provenance.py` | 来源追踪 |
| `tools/skills_sync.py` / `skills_sync_client.py` | 跨机器同步 |
| `tools/skills_hub.py` | Skill hub（共享中心） |
| `tools/skill_ledger.py` | Skill ledger |
| `tools/skill_usage.py` | Skill 使用统计 |

### 4.1 Skill 文件格式

Hermes 严格遵循 [`agentskills.io`](https://agentskills.io) 开放标准：

```markdown
---
name: weather
description: Check current weather for a location
frontmatter_version: 1
---

# Weather Skill

This skill checks the weather using the wttr.in API.

## Steps
1. Parse the location from the user request
2. Call `curl wttr.in/<location>`
3. Format the output
```

`tools/skills_tool.py:570` `_parse_frontmatter(content)`：

```python
def _parse_frontmatter(content: str) -> Tuple[Dict[str, Any], str]:
    """Extract YAML frontmatter and Markdown body.
    
    Uses ruamel.yaml (not PyYAML) because ruamel preserves comments and
    ordering — important for round-trip if/when we ever rewrite skill files.
    """
    if not content.startswith("---"):
        return {}, content
    end = content.find("\n---", 3)
    if end == -1:
        return {}, content
    yaml_part = content[3:end].strip()
    body = content[end + 4:].lstrip("\n")
    try:
        from ruamel.yaml import YAML
        yaml = YAML(typ="safe")
        frontmatter = yaml.load(yaml_part) or {}
    except Exception:
        return {}, content
    return dict(frontmatter), body
```

### 4.2 Skill 发现（4 来源）

`tools/skills_tool.py:_find_all_skills()`（行 687）：

1. **Bundled**：`skills/`（13 类别）+ `optional-skills/`（默认不激活）
2. **User**：`~/.hermes/skills/`
3. **Project**：`.hermes/skills/`（opt-in via `HERMES_ENABLE_PROJECT_PLUGINS`）
4. **pip entry points**：`hermes_agent.skills` entry point

`tools/skills_tool.py:267-289`：

```python
def skill_matches_platform(frontmatter: Dict[str, Any]) -> bool:
    """Filter by platform frontmatter. Allows e.g. weather skill only on desktop."""
    ...

def skill_matches_environment(frontmatter: Dict[str, Any]) -> bool:
    """Filter by environment (docker / local / ssh)."""
    ...
```

### 4.3 Skill 注入为 User Message（核心设计）

`AGENTS.md:486` 注释：

> "Skill slash commands: `agent/skill_commands.py` scans `~/.hermes/skills/`, injects as **user message** (not system prompt) to preserve prompt caching"

为什么？这是 Hermes 的"窄腰哲学"具体体现：

- system prompt 是 Anthropic cache control 的 `cache_control: ephemeral` 标记覆盖范围
- 修改 system prompt 会让 cache 失效
- 注入 user message **不破坏** cache（user message 在尾部追加）

### 4.4 Skill Guards（多层安全）

```python
# tools/skills_guard.py
# 检测 dangerous frontmatter（如 attempts to override system prompt）

# tools/skills_ast_audit.py
# AST 审计（检查 skill 是否引用危险路径 / 可疑 import）

# tools/skill_linter.py
# Lint（frontmatter 合法性）

# tools/skill_provenance.py
# 来源追踪（从哪里加载）
```

### 4.5 Skill Sync（跨机器同步）

`tools/skills_sync.py` / `tools/skills_sync_client.py`：

- 一台机器创建的 skill 可同步到另一台
- 用 cryptographic signature 验证完整性
- `~/.hermes/skills/<name>/SKILL.md` 可 push/pull

### 4.6 Skill Hub（共享中心）

`tools/skills_hub.py`：

- 用户可发布 skill 到 hub
- Hub 用户可安装别人发布的 skill
- 类似 "skill registry"

### 4.7 Skill Provenance（来源追踪）

`tools/skill_provenance.py`：

```python
def skill_provenance(skill_name: str) -> Dict:
    """Return provenance chain: original author → review → install.
    
    Used by skill_linter to verify trust.
    """
    ...
```

### 4.8 Skill 评估器（skillevaluator_scan.py）

`tools/skillevaluator_scan.py`：

- 扫描 skill 是否被有效使用
- 评估 skill 效果（成功率）
- 自动标记低效 skill 待重写

### 对 laew 的借鉴价值

1. **agentskills.io 兼容**：laew 无 Skill 系统。建议引入：
   - 遵循 `agentskills.io` 标准（SKILL.md + frontmatter）
   - 注入为 user message（保护 prompt cache）
   - 用 `serde_yaml` 替代 `ruamel.yaml`（laew 是 Rust）
2. **Skill 发现 4 来源**：laew 引入 Skill 时应支持 Bundled + User + Project + pip（cargo crate 形式）。
3. **Skill guards + AST audit + linter**：laew 引入 Skill 时必须有多层安全检查。Rust 优势：AST 审计可用 syn 库（比 Python ast 强）。
4. **Skill sync（跨机器）**：laew 引入 Skill 时可借鉴 git-style sync 模式。
5. **Skill hub（共享中心）**：laew 引入 Skill 时可考虑 central hub，借鉴 crates.io 模式。
6. **Skill provenance（来源追踪）**：laew 引入 Skill 时必须有 provenance chain，记录"作者 → 审核 → 安装"。
8. **Skill evaluator（自动评估）**：laew 引入 Skill 时可借鉴 usage tracking + auto-evaluate 机制，标记低效 skill 待重写。

---

## 专题 5：Plugin 体系 —— 6 类 Surface

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `hermes_cli/plugins.py` | General PluginManager |
| `plugins/memory/<name>/` | MemoryProvider 插件 |
| `plugins/model-providers/<name>/` | 推理 backend 插件 |
| `plugins/context_engine/<name>/` | 上下文压缩插件 |
| `plugins/cron_providers/<name>/` | cron 任务源插件 |
| `plugins/kanban/` | 多 agent 看板 |

### 5.1 General PluginManager

`hermes_cli/plugins.py`：

```python
class PluginContext:
    """传递给每个 plugin 的 register(ctx) 函数。
    
    插件可注册：
    - Python-callback lifecycle hooks
    - Tools（通过 ctx.register_tool()）
    - CLI subcommands（通过 ctx.register_cli_command()）
    """
    def register_tool(self, ...): ...
    def register_cli_command(self, ...): ...
    def on_hook(self, hook_name: str, callback: Callable): ...
```

Hooks 列表：

```python
# plugin hooks
"pre_tool_call"          # tool 执行前
"post_tool_call"         # tool 执行后
"pre_llm_call"           # LLM 调用前
"post_llm_call"          # LLM 调用后
"on_session_start"       # session 开始
"on_session_end"         # session 结束
```

### 5.2 Memory Provider Plugins（bundled-first 顺序）

`AGENTS.md:781-792`：

> "Discovery covers the same four sources as the general `PluginManager` — bundled, `$HERMES_HOME/plugins/`, `./.hermes/plugins/` (opt-in via `HERMES_ENABLE_PROJECT_PLUGINS`), and `hermes_agent.memory_providers` entry points — but with **bundled-first** precedence, the reverse of the general system's later-wins order: a memory provider is activated by name, so a dropped-in directory must not be able to shadow a shipped one."

**关键设计**：memory provider 的发现顺序是 **bundled-first**，与 general plugin 的 later-wins 相反。原因：

- memory provider 通过名字激活
- 用户目录的同名 provider **不应覆盖** bundled provider
- 否则恶意 plugin 可伪装成内置 memory provider

### 5.3 Model-Provider Plugins（lazy discovery）

`providers/__init__.py._discover_providers()`：

> "**lazy, separate discovery system** — scanned on first `get_provider_profile()` or `list_providers()` call, NOT by the general PluginManager."

```python
# plugins/model-providers/<name>/__init__.py
# 每个 plugin 调用 providers.register_provider(ProviderProfile(...))
```

Scan order：

1. Bundled: `<repo>/plugins/model-providers/<name>/`
2. User: `$HERMES_HOME/plugins/model-providers/<name>/`
3. Legacy: `<repo>/providers/<name>.py`（back-compat）

### 5.4 Kanban Plugin（多 Worker 调度）

`plugins/kanban/`：

- dispatcher —— 调度器
- worker —— worker
- board —— 看板 UI

每个 worker 是独立 AIAgent 实例。

### 5.5 Cron Providers

`plugins/cron_providers/<name>/`：

- cron 任务源（filesystem / k8s / external API）
- 每个 provider 实现 cron provider ABC

### 5.6 Context Engine Plugins

`plugins/context_engine/<name>/`：

- 上下文压缩插件（区别于"压缩"—— context engine 是更宽的概念，包括语义检索、引用解析等）
- 实现 context_engine ABC

### 5.7 Native Plugin Compatibility Policy

`AGENTS.md:768-779`：

> "Keep documented plugin surfaces additive:
> - add hook payload data as keyword fields; signature-inspect callbacks so old narrow signatures receive only fields they declare, while `**kwargs` callbacks receive the complete payload;
> - do not remove or rename `PluginContext` methods; make new parameters optional with defaults and keyword-only where possible;
> - ignore unknown native manifest fields;
> - give new provider methods default implementations, and signature-inspect optional callback kwargs rather than forwarding them unconditionally;
> - use a local schema version only for a capability with a wire or persisted contract, and preserve old state/config/session replay or ship a migration."

设计哲学：

- **Additive only** —— 新参数必须有 default
- **signature-inspect** —— 老 narrow 签名插件不被新参数破坏
- **backward compat** —— 删除方法需要 2 个 minor release 缓冲

### 对 laew 的借鉴价值

1. **6 类 plugin surface**：laew 当前只有 tool / protocol 扩展。建议增加：
   - general plugin（hooks + tools + cli subcommand）
   - memory plugin
   - model-provider plugin
   - context-engine plugin
   - cron plugin
   - multi-agent worker plugin
2. **Bundled-first 顺序**：laew 引入 plugin 时，core tool 必须默认赢。
3. **Lazy discovery**：laew 引入 plugin 时，model-provider 应该 lazy discovery（不在启动时全部 import）。
4. **Native plugin compatibility policy**：laew 引入 plugin 时必须有严格的兼容性策略：
   - hook payload 用 keyword field
   - 不删除 PluginContext 方法
   - 忽略未知 manifest field
   - 新方法必须有 default impl
5. **Signature-inspect**：Rust 用 trait 自动 derive，不需要 Python 的 signature-inspect。但 laew 应该用 trait + Option 3 表达兼容性。

---

## 专题 6：Multi-Surface Agent（同一 AIAgent 多前端）

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `run_agent.py` AIAgent | 共享核心（~10k LOC） |
| `cli.py` HermesCLI | CLI 表面 |
| `ui-tui/src/app.tsx` + `tui_gateway/server.py` | TUI 表面（JSON-RPC） |
| `apps/desktop/` | Desktop 表面（Electron） |
| `hermes_cli/web_server.py` + `hermes_cli/pty_bridge.py` | Web Dashboard 表面 |
| `gateway/run.py` | Messaging 表面（20+ 平台） |
| `acp_adapter/server.py` | ACP 表面（VS Code / Zed） |

### 6.1 AIAgent 钩子接口

`run_agent.py:__init__` 列出全部 callback：

```python
tool_progress_callback: callable = None,
tool_start_callback: callable = None,
tool_complete_callback: callable = None,
thinking_callback: callable = None,
reasoning_callback: callable = None,
clarify_callback: callable = None,
read_terminal_callback: callable = None,
```

每个 callback 都被特定 surface 利用：

| Callback | CLI | TUI | Desktop | Web | Messaging | ACP |
|----------|-----|-----|---------|-----|-----------|-----|
| `tool_progress_callback` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `tool_start_callback` | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `tool_complete_callback` | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `thinking_callback` | ✓ | ✓ | ✓ | ✓ | ✗ | ✓ |
| `reasoning_callback` | ✓ | ✓ | ✓ | ✓ | ✗ | ✓ |
| `clarify_callback` | ✓ | ✓ | ✓ | ✓ | ✓（跨平台） | ✓ |
| `read_terminal_callback` | ✓ | ✓ | ✓ | ✓ | ✗ | ✓ |
| `interrupt` | ✓（Ctrl+C） | ✓（Esc） | ✓（按钮） | ✓ | ✓（平台特定） | ✓ |
| `stream_callback` | ✓ | ✓ | ✓ | ✓ | ✓（TTS 触发） | ✓ |

### 6.2 TUI Process Model

```
hermes --tui
  └─ Node (Ink)  ──stdio JSON-RPC──  Python (tui_gateway)
       │                                  └─ AIAgent + tools + sessions
       └─ renders transcript, composer, prompts, activity
```

TypeScript 拥有屏幕。Python 拥有 session、tools、model calls、slash command 逻辑。

**关键设计**：UI 与 backend 通过 **stdio JSON-RPC** 通信，新请求从 Ink，事件从 Python。

### 6.3 Desktop Process Model

`apps/desktop/` 是 Electron + React + nanostore，通过 `requestGateway(method, params)` 与 `tui_gateway` 后端通信：

```typescript
// apps/desktop — Electron + React
// 通过 WebSocket / JSON-RPC 与 Python tui_gateway 通信
// 不嵌入 hermes --tui（独立 chat surface）
```

特殊设计：spawn `hermes serve`（headless backend），`HERMES_SERVE_HEADLESS=1` 让 `mount_spa()` 禁用 SPA。

### 6.4 Web Dashboard 嵌入 TUI

`hermes_cli/web_server.py`：

> "Browser loads `web/src/pages/ChatPage.tsx`, which mounts xterm.js's Terminal with the WebGL renderer. The server spawns whatever `hermes --tui` would spawn, through `ptyprocess` (POSIX PTY — WSL works, native Windows does not)."

**关键设计**：Web Dashboard **不重写**聊天体验，而是嵌入真实的 `hermes --tui`。这是 Hermes 的设计哲学：

> "Do not re-implement the primary chat experience in React."

### 6.5 Messaging Gateway

`gateway/run.py` 是 asyncio 主进程，每条消息创建独立 AIAgent：

```python
# gateway/run.py —— asyncio + 20+ 平台适配器
# platforms/telegram.py / discord.py / slack.py / ...
```

**架构**：

- 每个平台有独立 adapter（`platforms/<name>.py`）
- adapter 把消息转成统一 event
- gateway 调度 AIAgent 处理 event
- 处理结果通过 adapter 发回平台

### 6.6 ACP Adapter

`acp_adapter/server.py`：

```python
# ACP (Agent Client Protocol) 是 VS Code / Zed / JetBrains 的标准协议
# acp_adapter 让 Hermes 作为 ACP server 与 IDE 集成
```

6 个文件：

- `server.py` —— ACP 服务端
- `entry.py` —— 入口
- `session.py` —— session 管理
- `tools.py` —— 工具桥
- `auth.py` —— 认证
- `events.py` —— 事件
- `permissions.py` —— 权限
- `edit_approval.py` —— 编辑审批

### 对 laew 的借鉴价值

1. **Callback 抽象**：laew 当前无 callback 抽象。建议增加：
   - `tool_progress_callback`（实时进度）
   - `tool_start_callback` / `tool_complete_callback`（UI 反馈）
   - `thinking_callback` / `reasoning_callback`（thinking 展示）
   - `clarify_callback`（用户澄清）
   - `interrupt` 方法（双 ABI）
2. **TUI Process Model**：laew 的 TUI 当前是单进程（Rust + crossterm）。若未来要拆分为"前端 + 后端"模式（前端 TS，后端 Rust），可借鉴 JSON-RPC + stdio 设计。
3. **Web Dashboard 嵌入 TUI**：laew 若增加 Web Dashboard，**不要重写**聊天体验，而是嵌入真实的 TUI（PTY bridge）。
4. **Messaging Gateway**：laew 当前无 messaging gateway。若增加，需要 20+ 平台适配器，建议用 adapter pattern。
5. **ACP Adapter**：laew 若要集成到 IDE，可借鉴 `acp_adapter/` 模式，让 laew 作为 ACP server。

---

## 专题 7：Footprint Ladder（能力新增决策）

### 核心设计（AGENTS.md:103-149）

每个 capability 新增按"留痕量"由小到大分 6 级：

| Level | 决策 | 留痕量 | 示例 |
|-------|------|--------|------|
| 1 | Extend existing code | 零 | 改 BashTool 加新 flag |
| 2 | CLI command + skill | 零 model-tool 留痕 | `hermes webhook` / `hermes cron` |
| 3 | Service-gated tool (`check_fn`) | 仅 prerequisites 配置时出现 | Home Assistant（gated on token） |
| 4 | Plugin | `~/.hermes/plugins/<name>/` | honcho / mem0 |
| 5 | MCP server (in catalog) | 通过内置 MCP 客户端连接 | custom MCP server |
| 6 | New core tool | **最后**才考虑 | terminal / read_file / web_search |

### 7.1 选择原则

> "Choose the highest (least-footprint) rung that correctly solves the problem"

### 7.2 Footprint Ladder 的实践约束

`AGENTS.md:146-149`：

> "When 3+ open PRs try to integrate the same *category* of thing (memory backends, providers, notifiers), don't merge them one at a time — design an ABC + orchestrator, wrap the existing built-in as the first provider, and turn the competing PRs into plugins against that interface."

即：同类能力达到 3+ PR 时，必须先设计 ABC + orchestrator，然后让 PR 改为 plugin against ABC。

### 7.3 What We Don't Want（禁止列表）

`AGENTS.md:36-100`：

- **Speculative infrastructure** —— 钩子无具体消费者会被拒
- **New core tool when terminal + file already do the job** —— 用现有工具可解决时不增 tool
- **Lazy-reading escape hatches on instructional tools** —— 禁止在 skill/prompt 加 offset/limit 分页（模型会读 page 1 跳过其余）
- **"Fixes" that destroy the feature they secure** —— 缓解措施不能杀功能
- **Outbound telemetry without opt-in** —— 新分析/追踪需有 opt-in gate
- **Plugins that touch core files** —— plugin 必须 live in own dir
- **Third-party products in tree** —— vendor SaaS 不进 plugins/

### 对 laew 的借鉴价值

1. **Footprint Ladder 决策框架**：laew 当前无明确决策框架。建议引入：
   - **L1**：extend existing tool（改 BashTool 加 flag）
   - **L2**：CLI command + skill（如 `/lsm-xxx` 模式）
   - **L3**：service-gated tool（check_fn）
   - **L4**：plugin（`~/.lsmagent/plugins/<name>/`）
   - **L5**：MCP server
   - **L6**：new core tool（最后）
2. **同类 3+ PR 必须 ABC + orchestrator**：laew 引入 plugin 时，若同类能力达到 3 个，必须先设计 trait + orchestrator。
3. **禁止列表**：laew 引入 plugin 时必须明确禁止列表：
   - 不允许 speculative infrastructure
   - 不允许 plugin 改 core 文件
   - 不允许 vendor SaaS 进 plugins/

---

## 专题 8：Self-Improving Learning Loop

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `agent/auxiliary_client.py` | 副 LLM 客户端（curator / vision / embedding / title） |
| `agent/conversation_compression.py` | 压缩时自动提炼 skill |
| `agent/skill_preprocessing.py` | Skill 前处理 |
| `agent/background_review.py` | 后台 review |
| `agent/review_idle_queue.py` | review 空闲队列 |
| `tools/skill_manager_tool.py` | Skill 管理 tool |

### 8.1 副 LLM 体系（auxiliary_client）

`agent/auxiliary_client.py:_resolve_auto()`：

> "Per-task overrides for side-LLM work (curator, vision, embedding, title generation, session_search, etc.) — each task can pin its own provider/model/base_url/max_tokens/reasoning_effort."

```python
# agent/auxiliary_client.py 示意
def _resolve_auto(task: str) -> ProviderConfig:
    """Auto-resolve provider for auxiliary task.
    
    Resolution order:
    1. explicit override (auxiliary.<task>.provider)
    2. smart_model_routing.<task>_tasks
    3. main provider
    """
    ...
```

任务类型：

- `curator` —— skill auto-creation
- `vision` —— 图片理解
- `embedding` —— 向量化
- `title` —— session 标题生成
- `session_search` —— FTS5 结果排序

### 8.2 Skill Auto-Creation（curator）

`agent/conversation_compression.py` + `agent/auxiliary_client.py`：

- 任务完成后自动调用 curator LLM
- curator 分析对话提炼 skill
- 生成的 skill 写到 `~/.hermes/skills/<name>/SKILL.md`

```python
# 示意，实际在 conversation_compression.py:_auto_create_skill
def _auto_create_skill(turn_messages, agent) -> None:
    """If turn completed a complex task, call curator LLM to extract skill."""
    if not _is_skill_worthy(turn_messages):
        return
    curator = auxiliary_client.get("curator")
    skill_md = curator.generate_skill(turn_messages)
    skill_path = Path(get_hermes_home()) / "skills" / skill_md["name"] / "SKILL.md"
    skill_path.write_text(skill_md["content"])
    _log_skill_creation(skill_md["name"])
```

### 8.3 Skill Self-Improvement（使用中修正）

`agent/skill_preprocessing.py` + `tools/skill_linter.py`：

- Skill 被使用时记录 usage
- 定期 review usage stats
- 低效 skill 自动重写
- 高频使用 skill 自动优化 frontmatter

### 8.4 Periodic Nudges（后台 nudge）

`agent/conversation_loop.py:_iters_since_skill`（行 2377）：

> "Track tool-calling iterations for skill nudge. Counter resets whenever skill_manage is actually used."

```python
if (agent._skill_nudge_interval > 0
        and "skill_manage" in agent.valid_tool_names):
    agent._iters_since_skill += 1

if agent._iters_since_skill > agent._skill_nudge_interval:
    nudge_text = "Consider whether any skill in ~/.hermes/skills/ applies here."
    messages.append({"role": "user", "content": nudge_text})
```

### 8.5 Background Review

`agent/background_review.py:_spawn_background_review_now()`（`run_agent.py:2051`）：

```python
def _spawn_background_review_now(self, *args, **kwargs) -> None:
    """Schedule background review NOW (synchronous trigger)."""
    ...
```

`agent/review_idle_queue.py:QUEUE`：

- `note_turn_started()` —— 标记 turn 开始，review 不插入
- `note_turn_finished()` —— 标记 turn 结束，review 可运行

### 8.6 Honcho Dialectic User Modeling

`README.md:23`：

> "[Honcho](https://github.com/plastic-labs/honcho) dialectic user modeling"

`plugins/memory/honcho/` 实现 Honcho 集成。Honcho 是"dialectic user modeling"服务，跨会话构建用户身份认知。

### 对 laew 的借鉴价值

1. **副 LLM 体系**：laew 无 auxiliary_client。建议引入：
   - `auxiliary` 配置块（curator / vision / embedding / title）
   - 每个 task 可 pin 独立 provider/model/reasoning
   - 分辨率顺序：explicit → smart_routing → main
2. **Skill auto-creation**：laew 的 SessionContext Agent 是单次摘要。建议增加：
   - 任务完成后自动调用 curator LLM 提炼 skill
   - 生成的 skill 写到 `~/.lsmagent/skills/<name>/SKILL.md`
3. **Skill self-improvement**：laew 引入 Skill 后增加：
   - usage tracking（哪些 skill 高频使用）
   - 自动 lint（frontmatter 合法性）
   - 低效 skill 自动重写
4. **Periodic nudges**：laew 的 YoloRunner 当前每条输入重新分析。建议增加：
   - `_iters_since_skill` 计数
   - 超过 `skill_nudge_interval` 时注入 nudge（"考虑用 skill 吗？"）
5. **Background review**：laew 当前无 background review。建议借鉴 Hermes 的"活体空闲时跑 review"模式。
6. **Honcho dialectic user modeling**：laew 引入 MemoryProvider 后，可借鉴 Honcho 模式（dialectic）跨会话构建用户身份认知。

---

## 总结：Hermes 的 7 个核心设计决策

1. **窄腰 + Prompt Cache 神圣**：Skill 注入 user message，system prompt 中途不重建，toolset 不中途切换
2. **Footprint Ladder**：能力新增按留痕量由小到大分 6 级，强制走 plugin/MCP 路径
3. **Multi-Surface AIAgent**：同一 AIAgent 被 CLI / TUI / Desktop / Web / Messaging / ACP 6 个前端共享
4. **CompressionCommitFence**：generation counter 防止过期 commit 覆盖新工作
5. **Plugin Override 隔离**：core 默认赢，plugin override 必须显式声明
6. **Self-Improving Loop**：skill auto-creation + self-improvement + periodic nudges + Honcho
7. **Cache-Aware Steer/Redirect**：steer append 到 tool message（不破坏 cache），redirect 修改原 user message

---

## 附录：Hermes 与 laew 架构对照速查

| 维度 | Hermes | Laew |
|------|--------|------|
| 主语言 | Python | Rust |
| 入口 | run_agent.py AIAgent | main.rs CLI |
| 主循环 | 单层 while + 4 闸门 | Yolo + 单循环 |
| 压缩 | CompressionCommitFence + 6 阶段 | 无 |
| 工具调度 | handle_function_call + check_fn + bridge | Tool trait + builtin_registry |
| Provider | 30+ plugin | Anthropic + OpenAI |
| Skill | agentskills.io + user msg 注入 + 多层守卫 | 无 |
| 质检 | 后台 review（异步、非阻塞） | Quality-Check Agent（同步、必经） |
| 多 Agent | SubAgent + Kanban（3 机制） | Plan → Main → SubAgent（编排） |
| Session | SQLite + FTS5 + 多 Profile | SQLite session_memory |
| Plugin | 6 类 surface | 仅 tools/protocol |
| 终端后端 | 7 种 | 仅本地 |
| 多 UI 表面 | 6 surface | 1（TUI） |
| 学习循环 | 自改进 + Honcho | SessionContext Agent |
| i18n | 16 语言 | 仅中文 |
| 依赖固定 | exact pin | Cargo.lock |

---

## 附录：关键源码文件路径索引

| 主题 | 文件路径 |
|------|----------|
| AIAgent 类 | `run_agent.py:467` |
| AIAgent.run_conversation 转发 | `run_agent.py:9238` |
| 核心 while 循环 | `agent/conversation_loop.py:2289` |
| turn_context prologue | `agent/turn_context.py` |
| IterationBudget import | `run_agent.py:162` |
| Interrupt 方法 | `run_agent.py:3525` |
| Hard Interrupt 方法 | `run_agent.py:3830` |
| Steer 方法 | `run_agent.py:3908` |
| Redirect 方法 | `run_agent.py:3944` |
| CompressionCommitFence | `agent/conversation_compression.py:673` |
| Cooldown Under Lease | `agent/conversation_compression.py:615` |
| Micro-Compaction 文档 | `docs/micro-compaction.md` |
| Token 估算 | `agent/conversation_loop.py:686` |
| FTS5 跨会话搜索 | `hermes_state.py` + `tools/session_search_tool.py` |
| MemoryProvider ABC | `agent/memory_provider.py` |
| MemoryManager 类 | `agent/memory_manager.py:433` |
| Skill frontmatter 解析 | `tools/skills_tool.py:570` |
| Skill 发现 | `tools/skills_tool.py:_find_all_skills`（行 687） |
| Skill 注入为 user msg | `agent/skill_commands.py` |
| Skill guards | `tools/skills_guard.py` |
| Skill AST audit | `tools/skills_ast_audit.py` |
| Skill linter | `tools/skill_linter.py` |
| Skill provenance | `tools/skill_provenance.py` |
| Skill sync | `tools/skills_sync.py` / `tools/skills_sync_client.py` |
| Skill hub | `tools/skills_hub.py` |
| Skill evaluator | `tools/skillevaluator_scan.py` |
| Plugin Manager | `hermes_cli/plugins.py` |
| Plugin hooks | `pre_tool_call` / `post_tool_call` / `pre_llm_call` / `post_llm_call` |
| Plugin Override 隔离 | `tools/registry.py:623` |
| Check_fn TTL 缓存 | `tools/registry.py:282-450` |
| Tool Defs Cache | `model_tools.py:323-415` |
| Tool Search Bridge | `model_tools.py:1310` |
| Coerce Tool Args | `model_tools.py:856` |
| Auto-Discovery (AST) | `tools/registry.py:74-111` |
| ToolEntry class | `tools/registry.py:213` |
| Memory Provider Plugin 发现 | `plugins/memory/<name>/` |
| Model-Provider Plugin | `plugins/model-providers/<name>/` |
| Kanban Plugin | `plugins/kanban/` |
| Cron Providers Plugin | `plugins/cron_providers/<name>/` |
| Context Engine Plugin | `plugins/context_engine/<name>/` |
| AIAgent hooks | `run_agent.py:__init__` |
| TUI Process Model | `ui-tui/src/app.tsx` + `tui_gateway/server.py` |
| Desktop Process Model | `apps/desktop/` |
| Web Dashboard 嵌入 TUI | `hermes_cli/web_server.py` + `pty_bridge.py` |
| Messaging Gateway | `gateway/run.py` |
| ACP Adapter | `acp_adapter/server.py` |
| 副 LLM 客户端 | `agent/auxiliary_client.py` |
| Skill Auto-Creation | `agent/conversation_compression.py` + `agent/auxiliary_client.py` |
| Background Review | `agent/background_review.py` |
| Review Idle Queue | `agent/review_idle_queue.py` |
| Honcho 集成 | `plugins/memory/honcho/` |
| Skill Nudge | `agent/conversation_loop.py:2377` |
| Footprint Ladder 文档 | `AGENTS.md:103-149` |
| Plugin Compatibility Policy | `AGENTS.md:768-779` |
| Native Plugin compatibility contract | `website/docs/developer-guide/plugins/index.md#native-plugin-compatibility-contract` |
| Hermes 整体哲学 | `AGENTS.md`（README 第一段） |
| Config Loaders 3 种 | `AGENTS.md:626-650` |
| 依赖固定策略 | `pyproject.toml:46-49` + `AGENTS.md:670-686` |