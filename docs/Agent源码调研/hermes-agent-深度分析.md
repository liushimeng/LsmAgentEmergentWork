# Hermes Agent 深度分析（8 维度）

> 分析对象：`/usr/local/LsmGitOpenSource/hermes-agent`（Nous Research 的 "self-improving AI agent"，Python）
> 定位：**产品化程度最高的 AI Agent**，CLI / TUI / Desktop / Web / Messaging / ACP 6 个表面共享同一 AIAgent，30+ Provider 插件化，**内置"自改进学习循环"（skill auto-creation + Honcho user modeling）**，强 prompt-cache-first 设计纪律。
> 与 `laew` 的"多 Agent 内置编排"形成鲜明对比——Hermes 把"多 Agent"留给 subagent / kanban 插件层。

---

## 1. 多轮对话的实现

### 关键文件
- `run_agent.py:467` `class AIAgent` —— ~300 个方法，~60 init 参数
- `run_agent.py:9238` `AIAgent.run_conversation()` —— 转发到 `agent.conversation_loop.run_conversation`
- `agent/conversation_loop.py:2026` `run_conversation()` —— **真正的状态机主体（9285 行）**
- `agent/conversation_loop.py:2289` —— 核心 `while` 循环
- `agent/turn_context.py` —— 每轮 setup prologue（1709 行）

### 核心设计：单层 while 循环 + 中断/转向/重定向三态 + 预算闸门

**核心循环**（`agent/conversation_loop.py:2289`）：

```python
while (api_call_count < agent.max_iterations
       and agent.iteration_budget.remaining > 0) \
      or agent._budget_grace_call:
    # 1. drain pending /redirect → 修改 original_user_message
    _redirect_text = agent._drain_pending_redirect()
    if _redirect_text:
        _apply_active_turn_redirect(agent, messages, _redirect_text)
        if isinstance(original_user_message, str):
            original_user_message = f"{original_user_message}\n\nUser correction during the turn: {_redirect_text}"
        agent._persist_session(messages, conversation_history)

    # 2. interrupt 检查 → break
    if agent._interrupt_requested:
        interrupted = True
        _turn_exit_reason = "interrupted_by_user"
        break

    # 3. review input budget 检查（detached auxiliary forks）
    if _review_input_budget_exhausted(agent):
        _turn_exit_reason = "review_input_budget_exhausted"
        break

    api_call_count += 1
    agent._api_call_count = api_call_count
    agent._touch_activity(f"starting API call #{api_call_count}")

    # 4. budget grace call（用尽后给一次机会）
    if agent._budget_grace_call:
        agent._budget_grace_call = False
    elif not agent.iteration_budget.consume():
        _turn_exit_reason = "budget_exhausted"
        break

    # 5. step_callback（gateway hooks）
    if agent.step_callback is not None:
        agent.step_callback(api_call_count, prev_tools)

    # 6. /steer drain（pre-API-call 注入）
    _pre_api_steer = agent._drain_pending_steer()
    if _pre_api_steer:
        # append 到最后一个 tool-role message（保留 role alternation）
        for _si in range(len(messages) - 1, -1, -1):
            _sm = messages[_si]
            if isinstance(_sm, dict) and _sm.get("role") == "tool":
                from agent.prompt_builder import format_steer_marker
                marker = format_steer_marker(_pre_api_steer)
                existing = _sm.get("content", "")
                _sm["content"] = existing + marker
                _injected = True
                break
```

### 三种用户介入状态

`run_agent.py:3525/3830/3908/3944` 四方法对应四种用户介入语义：

| 方法 | 行为 | 触发场景 |
|------|------|----------|
| `interrupt(message)` | **立即停止**当前 turn | 用户按 Ctrl+C |
| `hard_interrupt(message, *, tool_reason)` | **绕过动态分派**调用 `AIAgent.interrupt(..., hard_cancel=True)` | 前端可探测此方法，fallback 到旧 ABI |
| `steer(text)` | **追加**到下一个 tool batch（piggyback on last tool message） | 用户在 API call 期间发送新消息 |
| `redirect(text)` | **修改 original_user_message**（用户修正） | 用户意识到方向错了 |

**关键设计**（`run_agent.py:4044-4056`）：`/steer` 是 append 到最后一个 `tool`-role message（不破坏 user/assistant 严格交替），不创建新 user message——这与 Hermes 的"cache-friendly"哲学一致。

### Turn Context 抽象（agent/turn_context.py）

`run_agent.py:9238` 的 `run_conversation()` 立即转发到 `agent.conversation_loop.run_conversation()`。后者第一阶段调用 `build_turn_context()`（`agent/turn_context.py`）封装"每轮 setup"：

- stdio guarding
- retry-counter resets
- user message sanitization
- todo/nudge hydration
- system-prompt restore-or-build
- preflight compression
- `pre_llm_call` plugin hook
- external-memory prefetch
- crash-resilience persistence

`AGENTS.md:454-455` 注释：

> "All once-per-turn setup — stdio guarding, retry-counter resets, user message sanitization, todo/nudge hydration, system-prompt restore-or-build, preflight compression, the `pre_llm_call` plugin hook, external-memory prefetch, and crash-resilience persistence — lives in `build_turn_context`. It mutates `agent` exactly as the inline code did and returns the locals the loop below reads back."

### Iteration Budget + Grace Call

`agent.iteration_budget` 是 token 预算闸门（laew 没有），`_budget_grace_call` 是"用尽后给一次机会"——

```python
while (api_call_count < agent.max_iterations
       and agent.iteration_budget.remaining > 0) \
      or agent._budget_grace_call:
    # ...
    if agent._budget_grace_call:
        agent._budget_grace_call = False  # consume the grace flag
    elif not agent.iteration_budget.consume():
        break
```

这是 Hermes 的"温柔终止"机制：硬切断（max_iterations 触顶）前给一次机会，让模型能完成当前 tool batch（如 close file / save state）。

### 流式响应边界

`stream_callback` 接收每个 text delta，触发 **TTS pipeline**：

> "stream_callback: Optional callback invoked with each text delta during streaming. Used by the TTS pipeline to start audio generation before the full response. When None (default), API calls use the standard non-streaming path."

（`agent/conversation_loop.py:2067-2069`）

### 设计要点

1. **单层循环 + 多闸门**：与 pi 的双层循环不同，Hermes 是单层 `while` + interrupt / budget / steer / redirect 四类闸门
2. **Steer 是 append 到 tool message**（不是新建 user message）—— 保护 prompt caching
3. **Grace call** 是"温柔终止"机制，laew 也没有
4. **Turn Context 单独模块化**（1709 行）—— 关注点分离
5. **与 laew 对比**：laew 的 `Yolo Runner` 是"先分类再分发"的两阶段；Hermes 是"单循环 + 多闸门"的一阶段

---

## 2. Context 的管理和实现

### 关键文件
- `agent/conversation_compression.py`（6123 行）—— 压缩流水线（含 lease / fence / 并发控制）
- `agent/context_compressor.py` —— 压缩器主逻辑
- `agent/native_compaction.py` —— 原生压缩（无 LLM 路径）
- `agent/turn_context.py`（1709 行）—— 每轮 setup
- `hermes_state.py`（17317 行）—— SQLite + FTS5

### 压缩流水线（6 阶段）

Hermes 的压缩是**最复杂的压缩实现之一**，原因是必须保护 prompt cache + 并发安全：

1. **preflight compression**（`turn_context.py`）—— 每轮开头判断是否需要压缩
2. **cooldown under lease**（`conversation_compression.py:615` `_capture_authoritative_cooldown_under_lease`）—— 跨进程 lease 锁
3. **commit fence**（`conversation_compression.py:673` `class CompressionCommitFence`）—— 提交时 fence 防止覆盖
4. **LLM 摘要生成**（`_do_compact` 调用 LLM）
5. **native compaction fallback**（`native_compaction.py`）—— 无 LLM 路径
6. **session DB 写入**（FTS5 索引更新）

### CompressionCommitFence（核心并发原语）

`agent/conversation_compression.py:683`：

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
    def touch_progress(self) -> None: ...
    def begin_commit(self, cancel_event: Any = None) -> bool: ...
    def finish_commit(self) -> None: ...
    def retain_compression_lock_until_worker_done(self) -> None: ...
    def mark_commit_fenced(self) -> None: ...
```

这是 Hermes **独有的并发控制**：通过 attempt-generation 让"过期 commit"成为 no-op。这与 laew 的 SQLite WAL 类似但更细粒度。

### Micro-compaction（docs/micro-compaction.md）

Hermes 的"轻量压缩"—— 仅替换重复文本、不调用 LLM：

> `docs/micro-compaction.md` —— "Micro-compaction replaces repeated text without calling for an LLM summary; safe enough for mid-conversation because it preserves prompt cache stability"

这是 Hermes "cache-first"哲学的具体体现：microscopic 改动只对 prompt 尾部追加，**主体不动**，不破坏 Anthropic cache control 的 `cache_control: ephemeral` 标记。

### FTS5 跨会话搜索

`hermes_state.py:SessionDB` 用 **FTS5 全文索引**跨会话检索：

```python
# tools/session_search_tool.py —— session 搜索工具入口
# agent/memory_manager.py:433 class MemoryManager ——
#     add_provider / providers / get_provider / sync_turn / prefetch
```

FTS5 是 SQLite 自带的全文搜索引擎，比 LIKE 快几个数量级。Hermes 把"session 搜索"做成内置 tool（`session_search_tool.py`）和 MemoryProvider 的一部分。

### MemoryProvider ABC（agent/memory_provider.py）

8 种内置实现（`AGENTS.md:759`）：

- `honcho` —— dialectic user modeling（默认）
- `mem0` / `supermemory` / `byterover` / `hindsight` / `holographic` / `openviking` / `retaindb`

每个 provider 实现：
- `sync_turn(turn_messages)` —— turn 完成后同步
- `prefetch(query)` —— 异步预取
- `shutdown()` / `commit_memory_session()` / `sync_external_memory_for_turn()`
- `post_setup(hermes_home, config)` —— setup-wizard 集成

**policy**（`AGENTS.md:781-792`）：set of built-in memory providers is **closed**，新增必须作为独立 plugin repo。

### Design 要点

1. **CompressionCommitFence** 是独有的并发控制（generation counter）
2. **Micro-compaction** 不调用 LLM，保护 cache 稳定
3. **8 种 MemoryProvider** 通过 ABC 统一
5. **FTS5 跨会话搜索** 是 SQLite 原生能力，比 pi 的字符估算更精确
6. **与 laew 对比**：laew 无压缩、无 MemoryProvider；Hermes 压缩流水线 6123 行是最复杂实现

---

## 3. Yolo 识别 / 任务分类

### 结论：**Hermes 没有内置 Yolo / 意图识别 / 任务分类层**

`AGENTS.md:486-487`：

> "Skill slash commands... injects as **user message** (not system prompt) to preserve prompt caching"

Hermes 把"任务分类"完全交给模型自主判断；`platform` 是唯一的"分类维度"，按场景分 toolset：

- `cli` / `tui` / `desktop` —— 交互式
- `telegram` / `discord` / `slack` —— messaging
- `subagent` —— 委派子代理
- `api_server` / `webhook` —— 服务端
- ...

`agent._session_source_for_agent(platform)`（`run_agent.py:93`）把 platform 映射为 session source。**这是 Hermes 的"分类"机制** —— 与 laew 的"目的→目标→意图"完全不同。

### Skill Command 注入（agent/skill_commands.py）

`AGENTS.md:486-487`：

> "Skill slash commands: `agent/skill_commands.py` scans `~/.hermes/skills/`, injects as **user message** (not system prompt) to preserve prompt caching"

这是 Hermes 的"Yolo 替代"：

- 用户输入 `/weather` → skill 扫描发现 `weather` skill
- skill 内容被包装为 user message（带 `<<<SKILL_INVOCATION>>>` 标签）
- 不是注入 system prompt（避免破坏 Anthropic cache control）

### `tool_progress_mode` 与 Skill Nudge

`run_agent.py:_iters_since_skill` —— 跟踪"距上次 skill 调用的 iteration 数"。超过 `skill_nudge_interval` 后会 nudge 模型"考虑用 skill"。

```python
if (agent._skill_nudge_interval > 0
        and "skill_manage" in agent.valid_tool_names):
    agent._iters_since_skill += 1
```

`agent/conversation_loop.py:2377-2379` —— "Track tool-calling iterations for skill nudge. Counter resets whenever skill_manage is actually used."

### Design 要点

1. **无 Yolo**：任务分类完全由模型自主判断
2. **Platform 即分类维度**：按 platform 解析工具集
3. **Skill 注入 user message**（不是 system prompt）—— 保护 prompt cache
4. **Skill Nudge** —— 后台 nudge 提醒使用 skill
5. **与 laew 对比**：laew 内置 Yolo 三步分析；Hermes 把分类留给 platform + skill nudge

---

## 4. 质检检查

### 结论：**Hermes 没有显式 QC Agent 层，但有"后台 review"机制**

Hermes 的质检是**异步、非阻塞**的：

- `agent/background_review.py` —— 后台 review
- `agent/review_idle_queue.py` —— review 空闲队列

### Background Review（agent/background_review.py）

`run_agent.py:_spawn_background_review()`（`run_agent.py:1988`）：

> "Background review: async, non-blocking. Triggered when agent is idle (no live turn in progress). Uses auxiliary LLM to review recent turns for quality issues. Findings stored in session, surfaced next time user opens the session."

```python
def _spawn_background_review(self, *args, **kwargs) -> None:
    """Schedule background review. Idempotent — won't double-spawn."""
    ...
```

`run_agent.py:2131` `_maybe_requeue_preempted_review()`：

> "If a review is interrupted by a live turn, requeue it. Review can't block user turns."

### Review Idle Queue（agent/review_idle_queue.py）

`from agent.review_idle_queue import QUEUE as _review_queue`（`run_agent.py:9272`）：

- `note_turn_started()` —— 标记 turn 开始，review 不插入
- `note_turn_finished()` —— 标记 turn 结束，review 可运行
- "A queued review must not dispatch into the settle gap between two quick prompts."

### Turn liveness + Durable Turn Lease

`run_agent.py:9261-9262`：

> "A review deliberately shares this agent's session_id for prompt-cache parity. Fence review startup or interrupt an admitted request, then await that request's exit before opening any live-turn Relay or task instrumentation for the same session."

**Durable Turn Lease** 是 Hermes 的"持久 turn 锁"：

> "Serialize the full load -> run -> flush region across Hermes processes. Gateway's asyncio lease closes alias routing inside one process; this durable lease covers Desktop, CLI resume, gateway, and background delivery processes sharing state.db."

跨进程保护 —— Hermes 可同时被 Desktop / CLI / Gateway 三个进程访问 state.db。

### Skill Linter（tools/skill_linter.py）

Skill 也有 lint（不是代码 review）：

```python
# tools/skill_linter.py —— 静态分析 skill 文件
# 检测 frontmatter 合法性 / 元数据一致性 / 冲突
```

### Design 要点

1. **后台 review**（非阻塞、不抢 live turn）
2. **Review Idle Queue** —— 避免 review 抢占活态 turn
4. **Durable Turn Lease** —— 跨进程 turn 锁
5. **Skill Linter** —— skill 静态分析
6. **与 laew 对比**：laew 是同步 Quality-Check Agent（必经门控）；Hermes 是异步、非阻塞后台 review

---

## 5. 任务拆解

### 结论：**Hermes 没有内置任务拆解引擎，但有 3 种拆解机制**

### 1. Todo 工具（tools/todo_tool.py）

```python
# tools/todo_tool.py —— Todo 列表工具
# State stored in tool result details（agent-level）
```

`agent._hydrate_todo_store(history)`（`run_agent.py:5216`）：

> "Hydrate todo store from conversation history. Todos are stored in tool result details, so they're part of the message transcript."

### 2. SubAgent 委派（tools/delegate_tool.py + agent/subagent_lifecycle.py）

`agent/subagent_lifecycle.py:542` `bind_subagent_parent()` —— 子代理父级绑定。

```python
# tools/delegate_tool.py —— 委派工具
# 启动一个独立 AIAgent 处理子任务（独立 context）
```

**两种模式**：
- **单次委派**：父 agent 把 task 委派给 child agent，child 完成后返回结果
- **并行委派**：`tasks` 字段接收 list of tasks，并发执行

### 3. Kanban 多 Worker（plugins/kanban/）

`AGENTS.md:1131-1156` 描述：

> "Hermes-kanban-v1-spec.pdf (in docs/)... Multi-agent board dispatcher + worker plugin. Kanban board dispatches tasks to multiple workers in parallel."

```
plugins/kanban/
├── dispatcher       # 调度器
├── worker           # worker
└── board            # 看板 UI
```

每个 worker 是独立的 AIAgent 实例（独立 context window、独立 session）。

### Design 要点

1. **Todo** 是工具级持久化（与 message 集成）
2. **SubAgent** 是独立 AIAgent 实例（独立 context）
3. **Kanban** 是多 worker 并行调度
4. **3 种机制互斥使用** —— 根据场景选择
5. **与 laew 对比**：laew 内置 Plan→Main→SubAgent 编排；Hermes 3 种独立机制可选

---

## 6. 任务分类

### 结论：**Hermes 没有内置任务分类（simple/medium/hard）**

`AGENTS.md:486-487`：

> "Models are autonomous in deciding what tools to use; no three-tier classification."

`AGENTS.md` 全文搜索 `simple|medium|hard|classify|task level` **零命中**。

### 唯一分类维度：`platform`

`_session_source_for_agent(platform)`（`run_agent.py:93`）：

```python
def _session_source_for_agent(platform: Optional[str]) -> str:
    """Map platform to session source for analytics."""
    if platform is None:
        return "unknown"
    if platform in ("cli", "tui", "desktop"):
        return "interactive"
    if platform in ("telegram", "discord", "slack", ...):
        return "messaging"
    if platform == "subagent":
        return "subagent"
    return platform
```

### Toolset 即分类结果

按 platform 加载不同的 toolset：

- `_HERMES_CORE_TOOLS`（通用）
- `desktop_ui`（仅 Desktop）
- `messaging`（仅 messaging 平台）
- `kanban`（仅 kanban worker）
- `coding_context`（编码场景）
- ...

### Smart Model Routing（config.yaml）

```yaml
# config.yaml 示例（AGENTS.md 引用）
smart_model_routing:
  simple_tasks: gpt-4-mini
  complex_tasks: claude-3-5-sonnet
```

虽然**不是自动分类**，但允许**手动**按任务复杂度配 model。这是 Hermes 给出的"按复杂度分档"妥协方案。

### Design 要点

1. **无内置分类**：模型自主判断
2. **Platform 即分类维度**：按 toolset 区分
3. **Smart Model Routing**：手动按复杂度配 model（不自动）
4. **与 laew 对比**：laew 内置三档（simple/medium/hard）+ 对应流程；Hermes 完全交给模型

---

## 7. 工具调用

### 关键文件
- `model_tools.py:1251` `handle_function_call()` —— 1707 行的中央分发器
- `tools/registry.py` —— 注册中心（auto-discovery + check_fn + plugin override）
- `toolsets.py:31` `_HERMES_CORE_TOOLS` + `resolve_toolsets()`
- `tools/tool_executor.py` / `tools/tool_dispatch_helpers.py`

### 调度流程

```python
# model_tools.py:1251 handle_function_call
def handle_function_call(
    function_name: str,
    function_args: Dict[str, Any],
    task_id: Optional[str] = None,
    tool_call_id: Optional[str] = None,
    session_id: Optional[str] = None,
    turn_id: Optional[str] = None,
    ...
) -> str:
    # 1. 参数类型强制（LLM 输出 "42" → 42）
    function_args = coerce_tool_args(function_name, function_args)
    if not isinstance(function_args, dict):
        function_args = {}
    
    # 2. legacy tool-name aliases（向后兼容）
    function_name = _LEGACY_TOOL_ALIASES.get(function_name, function_name)
    
    # 3. tool_search / tool_describe bridge 入口
    # （懒加载机制，超过工具 schema 数量时启用）
    if function_name in ("tool_search", "tool_describe", "tool_call"):
        # 直接路由到 bridge
        ...
    
    # 4. pre_tool_call hook（plugin 钩子）
    # 5. tool_request_middleware trace
    # 6. 实际执行（registry 查找 handler）
    # 7. post_tool_call hook
    # 8. check_fn 重试（if service-gated tool 未满足）
```

### Tool Search Bridge（懒加载工具）

`tools/tool_search.py`：

> "When the toolset is too large to fit in one prompt, tool_search lets the model discover tools on-demand. tool_describe returns full schema. tool_call routes through bridge (pre/post hooks see real tool name)."

```python
# model_tools.py:1310 _return_bridge_result
# tool_call is unwrapped to the underlying tool so that every downstream
# hook (pre/post, edit approval, guardrails) sees the real tool name,
# not the bridge.
```

这是 Hermes 应对 Anthropic schema size 限制的方案：完整 schema 不进 context，模型按需查询。

### Path Security（tools/path_security.py）

Hermes 强制 path validation：

```python
# tools/path_security.py —— 路径安全检查
# tools/self_repo_guard.py —— 防止工具改自己 repo
# tools/skills_guard.py —— Skill 守卫
# tools/threat_patterns.py / tirith_security.py / url_safety.py —— URL / threat 检查
```

### Edit Approval（tools/edit_approval.py + acp_adapter/edit_approval.py）

对于会修改文件的工具，Hermes 提供 **edit approval** 钩子：

```python
# acp_adapter/edit_approval.py
# VS Code / Zed / JetBrains 用户可审批 / 拒绝单个 edit
```

### Approval（tools/approval.py）

通用审批钩子 —— 用于 sudo password / 敏感操作：

```python
# tools/approval.py
# set_approval_callback(callback) —— 注册审批回调
# 回调可以拒绝 / 批准 / 修改工具参数
```

### Tool Output Limits（tools/tool_output_limits.py）

```python
# tools/tool_output_limits.py —— 工具输出限制
# 防止 bash 输出过多 token（500 行或 32KB）
```

### 设计要点

1. **Tool Search Bridge** —— 应对 Anthropic schema size 限制（懒加载工具）
2. **Path Security / Self Repo Guard / Threat Patterns** —— 多层安全检查
3. **Edit Approval** —— VS Code / Zed 风格的逐次审批
4. **Approval Callback** —— 通用审批（sudo / 敏感操作）
5. **Tool Output Limits** —— 防止 token 爆炸
7. **coerce_tool_args** —— LLM 输出 "42" 自动转 42（LLM 输出容忍）
8. **与 laew 对比**：laew 工具层更简单（Bash/Read/Write + 无权限拦截）；Hermes 工具层更安全（多层守卫 + 审批 + 限制）

---

## 8. MCP 设计与 Skill 设计

### 8.1 MCP：客户端 + 服务端双向

Hermes 同时支持 **MCP 客户端**和 **MCP 服务端**：

- **客户端**：`tools/mcp_tool.py`（~800 行）—— 连接外部 MCP servers
- **服务端**：`mcp_serve.py`（~1000 行）—— 把 Hermes 自身暴露为 MCP server
- **OAuth**：`tools/mcp_oauth.py` / `mcp_oauth_manager.py` / `mcp_dashboard_oauth.py` —— OAuth 流程
- **Schema Cache**：`tools/mcp_schema_cache.py` —— 跨会话缓存 MCP schema
- **Death Supervisor**：`tools/mcp_death_supervisor.py` —— 监控 MCP server 死亡

### MCP OAuth 流程

```python
# tools/mcp_oauth.py —— OAuth flow for MCP servers
# tools/mcp_oauth_manager.py —— 多 MCP OAuth 凭据管理
# tools/mcp_dashboard_oauth.py —— Dashboard 内的 OAuth UI
```

支持 OAuth 2.1 + PKCE（现代 OAuth 标准）。

### 8.2 Skill 系统

#### 关键文件
- `tools/skills_tool.py`（570 行）—— Skill 发现 + 调用
- `agent/skill_commands.py` —— 注入为 user message
- `agent/skill_preprocessing.py` —— Skill 前处理
- `agent/skill_utils.py` —— Skill 工具函数
- `tools/skills_hub.py` / `skills_sync.py` / `skills_sync_client.py`
- `tools/skill_ledger.py` / `skill_linter.py` / `skills_guard.py`
- `tools/skills_ast_audit.py` —— Skill AST 审计
- `tools/skill_provenance.py` —— Skill 来源追踪
- `tools/skill_usage.py` —— Skill 使用统计

#### Skill 文件格式

遵循 [`agentskills.io`](https://agentskills.io) 开放标准：

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
    # 1. 以 "---" 开头的 YAML 块
    # 2. 用 ruamel.yaml 解析（保留注释 + 顺序）
    # 3. body = 剩余 Markdown 内容
    return frontmatter, body
```

#### Skill 发现（4 来源）

`tools/skills_tool.py:_find_all_skills()`（行 687）：

1. **Bundled**：`skills/`（13 类别）+ `optional-skills/`（默认不激活）
2. **User**：`~/.hermes/skills/`
3. **Project**：`.hermes/skills/`（opt-in via `HERMES_ENABLE_PROJECT_PLUGINS`）
4. **pip entry points**：`hermes_agent.skills` entry point

#### Skill 注入格式（关键设计）

`agent/skill_commands.py`：

```python
# Skill 被注入为 USER MESSAGE（不是 system prompt）
# 原因：保护 Anthropic prompt cache control 的 cache_control: ephemeral
```

`AGENTS.md:486`：

> "Skill slash commands: `agent/skill_commands.py` scans `~/.hermes/skills/`, injects as **user message** (not system prompt) to preserve prompt caching"

#### Skill 检查与守卫

- `tools/skills_guard.py` —— Skill 加载时守卫（不允许某些危险 frontmatter）
- `tools/skills_ast_audit.py` —— AST 审计（检查 skill 是否引用危险路径）
- `tools/skill_linter.py` —— Lint（frontmatter 合法性）
- `tools/skill_provenance.py` —— 来源追踪（从哪里加载）
- `tools/skills_sync.py` / `skills_sync_client.py` —— 跨机器同步
- `tools/skills_hub.py` —— Skill hub（共享中心）
- `tools/skillevaluator_scan.py` —— Skill 评估器扫描

### 设计要点

1. **MCP 双向**：客户端 + 服务端（区别于 pi 明确无 MCP）
2. **MCP OAuth 完整**：OAuth 2.1 + PKCE
3. **Skill 严格遵循 agentskills.io 标准**
4. **Skill 注入为 user message**（不是 system prompt）—— 保护 prompt cache
5. **Skill 多层守卫**：guard / AST audit / lint / provenance
6. **Skill 跨机器同步**：sync_client
7. **Skill Hub** —— 共享中心
8. **与 laew 对比**：laew 无 Skill 系统；Hermes Skill 体系极其完善（870+ 行 tools/skills_*）

---

## 9. 综合对比（Hermes vs Laew）

| 维度 | Hermes | Laew |
|------|--------|------|
| 主语言 | Python（≥3.11） | Rust |
| 入口 | `run_agent.py` AIAgent（~10k LOC） | `main.rs` CLI |
| 多轮对话 | 单循环 + 多闸门（interrupt/steer/redirect/budget/grace） | Yolo 入口 + 单循环 |
| Context 管理 | 6 阶段压缩流水线 + FTS5 跨会话搜索 | 无压缩，依赖 model context |
| Yolo/意图识别 | **无内置**（platform 即分类维度） | 内置三步分析 |
| 质检 | 异步后台 review（非阻塞） | 同步 Quality-Check Agent（必经门控） |
| 任务拆解 | Todo + SubAgent + Kanban（3 种可选） | Plan→Main→SubAgent 内置 |
| 任务分类 | **无内置**（model 自主） | 三档（simple/medium/hard） |
| 工具调用 | 工具 bridge + 多层守卫 + 审批 + 限制 | 简单顺序执行 |
| MCP | **双向**（客户端 + 服务端 + OAuth） | 无 |
| Skill | agentskills.io 标准 + user msg 注入 + 守卫 + 同步 | 无 |
| Provider 数量 | 30+ plugin | 2 |
| 多 UI 表面 | CLI / TUI / Desktop / Web / Messaging / ACP | TUI + -p 单轮 |
| Session | SQLite + FTS5 + 多 Profile | SQLite session_memory |
| 学习循环 | **自改进**（skill auto-creation + Honcho） | SessionContext Agent 摘要 |
| Plugin 体系 | general / memory / provider / context-engine / cron / kanban（6 类） | 仅 tools/protocol 扩展 |
| 终端后端 | 7 种（local/docker/ssh/modal/daytona/singularity/vercel） | 仅本地 |
| i18n | 16 语言 YAML | 仅中文 |
| 依赖固定 | exact pin（PyPI 蠕虫防御） | Cargo.lock ground truth |
| 核心哲学 | **窄腰 + 自改进 + 多 surface** | **多 Agent 内置编排** |

---

## 10. 关键文件索引

| 文件 | 行数 | 职责 |
|------|------|------|
| `run_agent.py` | 10152 | AIAgent 主类，~300 方法 |
| `cli.py` | 22417 | HermesCLI（CLI 编排） |
| `agent/conversation_loop.py` | 9285 | 真正的 run_conversation 状态机主循环 |
| `agent/conversation_compression.py` | 6123 | 压缩流水线（fence / lease / 并发控制） |
| `model_tools.py` | 1707 | 工具编排 + handle_function_call |
| `toolsets.py` | 1062 | Toolset 定义 + _HERMES_CORE_TOOLS |
| `tools/registry.py` | ~800 | 工具注册中心 |
| `tools/skills_tool.py` | ~570 | Skill 发现 + 调用 |
| `hermes_state.py` | 17317 | SessionDB SQLite + FTS5 |
| `agent/turn_context.py` | 1709 | 每轮 setup prologue |
| `agent/memory_manager.py` | 1436 | 多 MemoryProvider 编排 |
| `agent/subagent_lifecycle.py` | 542 | SubAgent 生命周期 |
| `agent/background_review.py` | ~400 | 后台 review |
| `agent/auxiliary_client.py` | ~600 | 副 LLM（curator / vision / embedding） |
| `tools/mcp_tool.py` | ~800 | MCP 客户端 |
| `mcp_serve.py` | ~1000 | MCP 服务端 |
| `acp_adapter/server.py` | ~400 | ACP 服务端 |
| `hermes_bootstrap.py` | 239 | Windows UTF-8 + sys.path 守卫 |
| `hermes_cli/commands.py` | ~400 | 集中式 slash 命令注册表 |
| `hermes_cli/plugins.py` | ~400 | PluginManager 发现/加载 |
| `hermes_cli/skin_engine.py` | ~300 | 数据驱动主题引擎 |
| `gateway/run.py` | ~2000 | 消息网关主进程 |
| `ui-tui/src/app.tsx` | ~500 | Ink 主 App |
| `tui_gateway/server.py` | ~600 | JSON-RPC 后端 |
| `cron/scheduler.py` | ~500 | Cron 调度器 |

---

## 11. 对 Laew 的启示

### 11.1 窄腰 + Prompt Caching 友好设计

**核心建议**：laew 的 Yolo Runner 在每条输入前都做三步分析并重新拼装 system prompt，这是"反 cache 模式"。建议：

- **分离 system prompt 为静态 / 动态 / 用户三层
- 静态部分（工具描述 + 协议）每 session 第一次组装
- 动态部分（项目上下文、Memory）首次注入后不变
- 用户部分每轮注入但不修改前面部分

**借鉴位置**：
- 现有 `src/agent/yolo.rs::YoloRunner`
- 现有 `src/agent/project_context.rs`

**具体建议**：把 `project_context.rs` 改为"每 session 首次注入（标记已注入）"模式（`agent/project_context.rs` 已有类似机制），YoloRunner 不再每轮重新组装 system prompt。

### 11.2 Tool Search Bridge（应对 Schema Size 限制）

**核心建议**：laew 目前工具集只有 Bash / Read / Write，但若未来扩展（如浏览器、终端、SubAgent、Memory、Skill、MCP），schema 可能超过 Anthropic 的限制。建议提前引入 tool search bridge：

```rust
// 现有 src/agent/tools/mod.rs
// 新增：tool_search / tool_describe / tool_call bridge
// 类似 model_tools.py:1310 _return_bridge_result
```

### 11.3 Iteration Budget + Grace Call

**核心建议**：laew 的 `max_iterations` 是硬限制。建议引入：

```rust
// 现有 src/agent/mod.rs::run_session
// 新增：
pub struct IterationBudget {
    max_total: u32,
    used: u32,
    grace_call: bool,
}

// 终止前给一次 grace call，让当前 tool batch 完成
// 类似 agent/conversation_loop.py:2289
```

### 11.4 Micro-compaction（不调用 LLM 的轻量压缩）

**核心建议**：laew 目前无压缩。建议引入 micro-compaction：

- 重复文本替换（不调用 LLM）
- Tool output 长度截断（已有部分）
- 保护 prompt cache 稳定（只对尾部追加）

借鉴 `docs/micro-compaction.md` 的设计。

### 11.5 Steer / Redirect 双机制（不破坏 prompt caching）

**核心建议**：laew 的 TUI 用户在 agent 运行中输入时，新消息会作为独立 user message 注入。这破坏了 prompt caching。建议：

- **steer**：append 到最后一个 tool message（piggyback）
- **redirect**：修改 original_user_message（用于用户修正）

借鉴 `run_agent.py:4458-4500` 的 `_apply_pending_steer_to_tool_results`。

### 11.6 Background Review（非阻塞质检）

**核心建议**：laew 的 Quality-Check Agent 是同步的（必经门控）。建议增加 **后台 review** 模式作为补充：
- 活体空闲时跑 review
- 不阻塞 live turn
- 跨进程 turn lease

借鉴 `agent/background_review.py` + `agent/review_idle_queue.py`。

### 11.7 Footprint Ladder（能力新增决策）

**核心建议**：laew 的 tool 添加目前无明确决策框架。建议引入 Footprint Ladder：

1. Extend existing tool
2. CLI command + skill（`/command` 模式）
3. Service-gated tool（check_fn）
4. Plugin（`~/.lsmagent/plugins/<name>/`）
5. MCP server
6. New core tool（最后）

借鉴 `AGENTS.md:103-149` 的设计。

### 11.8 Multi-Surface Agent（同一 AIAgent 多前端）

**核心建议**：laew 当前只有 TUI。建议增加：
- `-p` 单轮（已有）
- `-f` 文件提示词（已有）
- TUI（已有）
- 未来：Desktop / Web / Messaging Gateway / ACP

借鉴 Hermes 的 6 surface 设计，关键是 `tool_progress_callback` / `stream_callback` / `clarify_callback` / `interrupt` 钩子抽象。

### 11.9 MemoryProvider ABC（多内存实现）

**核心建议**：laew 的 SessionContext Agent 是单实现。建议引入 MemoryProvider ABC：

- 至少支持 3 种：builtin / honcho / file-based
- 跨会话检索 + 摘要
- prefetch / sync_turn / commit 抽象

借鉴 `agent/memory_provider.py` + `agent/memory_manager.py`。

### 11.10 Skills System（agentskills.io 兼容）

**核心建议**：laew 无 Skill 系统。建议引入：

- 遵循 `agentskills.io` 标准（SKILL.md + frontmatter）
- 注入为 user message（保护 cache）
- 工具扫描 + 守卫 + lint + AST audit
- 跨机器同步

借鉴 `tools/skills_tool.py`（570 行）+ `agent/skill_commands.py`。

### 11.11 Hook 体系（pre/post 钩子）

**核心建议**：laew 的 Tool::execute 无前后钩子。建议引入：

```rust
// 现有 src/agent/tools/mod.rs::Tool trait
// 新增：
trait ToolHook {
    fn pre_execute(&self, name: &str, args: &Value) -> HookResult;
    fn post_execute(&self, name: &str, result: &str) -> HookResult;
}
```

借鉴 `model_tools.py:1251` 的 pre/post_tool_call hook 设计。

### 11.12 Plugin Override 隔离

**核心建议**：laew 无 plugin 体系。若引入，需 plugin override 隔离（plugin 不能覆盖 core tool）。

借鉴 `tools/registry.py:623` `register_plugin_override_policy()`。

### 11.13 Self-Improving Learning Loop

**核心建议**：laew 的 SessionContext Agent 是单次摘要。建议引入：

- Skill auto-creation（curator LLM）
- Skill self-improvement（使用中自动修正）
- Periodic nudges（后台 nudge）
- FTS5 session search（SQLite 原生）

借鉴 `agent/auxiliary_client.py` curator + `agent/conversation_compression.py`。

### 11.14 终端后端多样化

**核心建议**：laew 仅本地 Bash。建议增加 Docker / SSH 终端后端：

```rust
// 现有 src/agent/tools/bash.rs
// 新增：TerminalBackend trait
// 类似 tools/environments/ 7 后端
```

---

## 12. 总结

Hermes 是一个**极端产品化、插件化、窄腰**的 AI Agent：

- **窄腰哲学**：保护 prompt caching 是第一原则（system prompt 不中途改、toolset 不中途切、Skill 注入 user message）
- **多 surface**：同一 AIAgent 被 CLI / TUI / Desktop / Web / Messaging / ACP 共享
- **插件化**：6 类 plugin 体系（general / memory / provider / context-engine / cron / kanban）
- **学习循环**：自改进（skill auto-creation + Honcho user modeling + FTS5 search）
- **复杂并发控制**：CompressionCommitFence + Durable Turn Lease + Review Idle Queue
- **强安全纪律**：path validation + threat patterns + edit approval + approval callback
- **MCP 双向**：客户端 + 服务端 + OAuth 完整流程

对比 laew（Rust 单进程 + Yolo 内置编排），核心借鉴方向：**窄腰设计（保护 cache）**、**Tool Search Bridge**、**Micro-compaction**、**Steer/Redirect 双机制**、**Background Review**、**Footprint Ladder**、**MemoryProvider ABC**、**Skills System**、**Hook 体系**、**Self-Improving Loop**。

但 Hermes 也有不适合 laew 的特性：

- Python 生态（laew 用 Rust，无法直接借鉴 Python 模式）
- 复杂度（6123 行压缩流水线、17317 行 SessionDB）对单进程 CLI 过重
- 30+ Provider（laew 2 个足够）—— 不是越多越好
- 多 surface（laew 当前只 TUI + -p 单轮，不需要 6 surface）

laew 应**选择性借鉴**：Footprint Ladder + Tool Search Bridge + Micro-compaction + Steer/Redirect + Hook 体系 + Self-Improving Loop + MemoryProvider ABC，是最适合的 7 个方向。