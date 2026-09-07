# Hermes Agent 综合深度分析

> 调研对象:hermes-agent(Python,859 MB,6 前端共享 AIAgent 核心+38 provider)
> 调研日期:2026-09-04 ~ 2026-09-05
> 原始文档:4 份(源码调研 696 行 + 深度分析 915 行 + 核心机制 1435 行 + 第二轮 1184 行 = 4230 行)
> 合并后行数:~1850 行(去重后)

---

## 1. 项目元信息

| 项 | 值 |
|---|---|
| 仓库名 | `hermes-agent`（GitHub: `NousResearch/hermes-agent`） |
| 版本 | `0.21.0`（`pyproject.toml:3`） |
| 主语言 | Python（`requires-python = ">=3.11,<3.14"`，`pyproject.toml:13`） |
| 辅助语言 | TypeScript / React（Ink TUI + Electron Desktop）、JS / JSX |
| 包管理 | `uv`（Astral 出品，基于 Rust 的 Python 包管理器） |
| 依赖锁定 | `uv.lock` ground truth（`pyproject.toml:46-49` 注释"PyPI exact pin 是出于 Mini Shai-Hulud 蠕虫事件后的策略"） |
| 协议支持 | OpenAI Chat Completions / OpenAI Codex Responses / Anthropic Messages（多 plugin 适配） |
| 部署目标 | CLI / TUI（Ink）/ Electron Desktop / Messaging Gateway（20+ 平台）/ Web Dashboard |
| Python 文件数 | 5109 个（含 tests） |
| TS/JS 文件数 | 2933 个（含 `tests-js`） |
| 仓库大小 | 859 MB（含 `tests/`、`tests-js/`、`mcp-research-data/` 等大目录） |
| 单测规模 | "约 17k 测试，约 900 文件"（`AGENTS.md:218`） |
| 顶层入口 | `cli.py`（CLI 模式）、`run_agent.py`（AIAgent 类）、`hermes_bootstrap.py`（UTF-8 / sys.path 守卫） |
| 国际化 | 16 语言 YAML（`locales/`：en/zh/zh-hant/es/fr/de/ja/ko/pt/ru/uk/it/nl/af/ar/tr/hu/ur 等） |
| 文档 | `docs/`（含 `design/` `rfcs/` `middleware/` `security/` `observability/` `kanban/` 等子目录）+ `website/`（Docusaurus 站点） |

### 仓库主标语

> "The self-improving AI agent built by Nous Research. It's the only agent with a built-in learning loop — it creates skills from experience, improves them during use, nudges itself to persist knowledge, searches its own past conversations, and builds a deepening model of who you are across sessions."（`README.md:14-15`）

### 与其他调研对象的对比定位

| 项目 | 主语言 | 入口 | 主循环位置 | 主循环行数 | 核心规模 |
|---|---|---|---|---|---|
| `atomcode` | Rust | `src/main.rs` | `crates/agent/src/lib.rs` | ~12k | ~150k 行 |
| `claudecode` | TS/Bun | `bin/claude-code.js` | 主循环散在 ~30 文件 | ~3k 核心 | ~218k 行 |
| `deepseek-harness` | TS | `harness/src/main.ts` | `Harness.run()` | ~3k | ~80+ 包 |
| `openclaw` | TS | `gateway/src/index.ts` | `Gateway.ts` | ~5k | ~201 万行 |
| `opencode` | TS/Bun | `packages/opencode/src/index.ts` | `Session` 类 | ~3k | ~18k 行 |
| `pi` | TS | `packages/coding-agent/src/main.ts` | `runAgentLoop` | ~800 | ~65 文件 |
| **`hermes-agent`** | **Python** | **`run_agent.py`** | **`run_conversation` (9285 行)** | **~12k 入口** | **~10k Python 主模块** |

---

## 2. 6 前端共享 AIAgent 架构

### 2.1 分层架构

```
┌──────────────────────────────────────────────────────────┐
│ UI 表面（任选其一）                                       │
│   · CLI (Rich + prompt_toolkit)         cli.py 22417 行   │
│   · TUI (Ink / React / TS)             ui-tui/src/       │
│   · Desktop (Electron + React)         apps/desktop/     │
│   · Web Dashboard (React + SPA)       website/web/ + PTY │
│   · Messaging Gateway (20+ 平台)       gateway/run.py   │
│   · ACP server (VS Code / Zed / JetBrains) acp_adapter/ │
├──────────────────────────────────────────────────────────┤
│ AIAgent 主类（run_agent.py, ~10k LOC）                    │
│   ↳ ~300 个方法、~60 init 参数、~90 字段                 │
│   ↳ run_conversation() → agent/conversation_loop.py      │
├──────────────────────────────────────────────────────────┤
│ conversation_loop.py（9285 行）—— 真正的状态机主循环     │
│   ↳ turn_context.py（1709 行）每轮 setup                 │
│   ↳ conversation_compression.py（6123 行）压缩流水线     │
│   ↳ memory_manager.py（1436 行）多 provider 内存         │
│   ↳ subagent_lifecycle.py（542 行）子代理生命周期         │
│   ↳ tool_executor.py / tool_dispatch_helpers / guardrails│
│   ↳ auxiliary_client.py（curator / vision / embedding）  │
├──────────────────────────────────────────────────────────┤
│ LLM Provider 适配层                                       │
│   ↳ OpenAI (chat + responses) / Anthropic / Codex       │
│   ↳ Bedrock / Azure（adapter 形式）                       │
│   ↳ plugins/model-providers/ 38 推理后端                  │
├──────────────────────────────────────────────────────────┤
│ 工具层（tools/ —— 143 文件自动发现）                       │
│   ↳ tools/registry.py —— check_fn + 权限 + plugin 隔离  │
│   ↳ toolsets.py —— _HERMES_CORE_TOOLS + toolset 解析     │
│   ↳ environments/ —— 7 终端后端（local/docker/ssh/modal/ │
│     daytona/singularity/vercel）                          │
├──────────────────────────────────────────────────────────┤
│ Skill 系统（skills/ + optional-skills/）                  │
│   ↳ skills_tool.py —— 570 行发现/调用                     │
│   ↳ skill_commands.py —— 注入为 user msg（保护缓存）      │
│   ↳ skill_ledger.py / skill_linter.py / skills_guard.py  │
├──────────────────────────────────────────────────────────┤
│ 持久化（hermes_state.py 17317 行，SQLite + FTS5）          │
│   ↳ SessionDB —— 会话存储、消息、FTS5 跨会话搜索          │
│   ↳ plugin/memory/ —— 8 种 MemoryProvider                │
│   ↳ plugin/context_engine/ —— 上下文压缩插件             │
└──────────────────────────────────────────────────────────┘
```

### 2.2 AIAgent 钩子接口（~60 个 __init__ 参数）

`run_agent.py:467 class AIAgent` 的 `__init__` 暴露 callback 矩阵：

| Callback | 用途 | CLI | TUI | Desktop | Web | Messaging | ACP |
|----------|------|-----|-----|---------|-----|-----------|-----|
| `tool_progress_callback` | 工具进度 | Y | Y | Y | Y | Y | Y |
| `tool_start_callback` / `tool_complete_callback` | UI 反馈 | - | Y | Y | Y | Y | Y |
| `thinking_callback` / `reasoning_callback` | 推理展示 | Y | Y | Y | Y | - | Y |
| `clarify_callback` | 用户澄清 | Y | Y | Y | Y | Y | Y |
| `stream_callback` | 流式文本 delta | Y | Y | Y | Y | Y(TTS) | Y |
| `interrupt` / `hard_interrupt` | 中断 | Y | Y | Y | Y | Y | Y |
| `steer` / `redirect` | 运行时改向 | Y | Y | Y | - | - | - |

**核心代码片段**（`run_agent.py:3525` + `run_agent.py:3830`）：

```python
# run_agent.py:3525 interrupt() — 软中断
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

# run_agent.py:3830 hard_interrupt() — 硬中断,绕过子类 override
def hard_interrupt(self, message=None, *, tool_reason=None) -> None:
    """Request an explicit stop while preserving `interrupt()` ABI.
    Frontends can feature-detect this method and fall back to the legacy
    `interrupt(message=None)` signature for synthetic or third-party agents.
    """
    # Deliberately bypass dynamic dispatch: subclasses written against the
    # legacy interrupt(message=None) ABI may override interrupt without the
    # newer keyword-only hard_cancel argument.
    AIAgent.interrupt(
        self, message, hard_cancel=True, tool_reason=tool_reason,
    )
```

**设计要点**：
- **双方法设计**：`hard_interrupt` 故意 `AIAgent.interrupt(self, ...)` 直调父类，**绕过动态分派**，防止子类 override `interrupt()` 时漏接 `hard_cancel` 参数。
- **前端可探测**：前端代码 `hasattr(agent, 'hard_interrupt')` 探测此方法存在性，fallback 旧 ABI，这是**面向第三方/synthetic agent 的兼容契约**。

### 2.3 TUI Process Model（UI 与 Backend 进程边界）

TypeScript Ink TUI **不内嵌** AIAgent，而是通过 **stdio JSON-RPC** 调 Python backend：

```
hermes --tui
  └─ Node (Ink)  ──stdio JSON-RPC──  Python (tui_gateway/server.py)
       │                                  └─ AIAgent + tools + sessions
       └─ 渲染 transcript / composer / prompts / activity
```

**关键文件**：`ui-tui/src/app.tsx`（Ink App，~500 行）+ `tui_gateway/server.py`（JSON-RPC 后端，~600 行）+ `ui-tui/src/gatewayClient.ts`（RPC 客户端，~300 行）

**设计哲学**：
- **UI 拥有屏幕**，Python 拥有 session、tools、model calls、slash command 逻辑
- TypeScript 仅承担渲染 + 用户输入事件 → RPC 调用
- Python 推送状态/事件 → TypeScript 重绘

### 2.4 Web Dashboard 嵌入 TUI（不重写聊天体验）

`hermes_cli/web_server.py` + `hermes_cli/pty_bridge.py` —— Web Dashboard **不重写**聊天 UI，而是嵌入真实的 `hermes --tui`：

> "Browser loads `web/src/pages/ChatPage.tsx`, which mounts xterm.js's Terminal with the WebGL renderer. The server spawns whatever `hermes --tui` would spawn, through `ptyprocess` (POSIX PTY — WSL works, native Windows does not)."

**核心架构**：
```
Browser → React ChatPage → xterm.js (WebGL) ← ptyprocess(PTY) → hermes --tui 进程
                                          (spawn on demand)
```

设计哲学：**"Do not re-implement the primary chat experience in React."** Web Dashboard 与 TUI 共享同一份 chat 体验。

### 2.5 Messaging Gateway（20+ 平台适配器）

`gateway/run.py` 是 asyncio 主进程，`gateway/platforms/` 含 20+ 平台 adapter（`telegram.py` / `discord.py` / `slack.py` / `whatsapp.py` / `signal.py` / `matrix.py` / `mattermost.py` / `irc.py` / `smpp_sms.py` / `email.py` / `webhook.py` / `api_server.py` / `homeassistant.py` / `dingtalk.py` / `wecom.py` / `weixin.py` / `feishu.py` / `qqbot.py` / `bluebubbles.py` / `yuanbao.py` / `teams.py` / `simplex.py` / `line.py` / `ntfy.py` / `google_meet.py` / `photon.py` / `raft.py` / `buzz.py` 等）

`gateway/platforms/base.py` 定义统一抽象：

```python
"""
Base platform adapter interface.

All platform adapters (Telegram, Discord, WhatsApp, Weixin, and more) inherit
from this and implement the required methods.
"""
import asyncio
...
class BasePlatformAdapter(ABC):
    @abstractmethod
    async def start(self): ...
    @abstractmethod
    async def stop(self): ...
    @abstractmethod
    async def send_message(self, ...): ...
    ...
```

每个 adapter 把消息转成统一 `event`，gateway 调度 AIAgent 处理 event，处理结果通过 adapter 发回平台。**这就是"adapter pattern"在多 surface 上的经典应用**。

### 2.6 ACP Adapter（IDE 集成）

`acp_adapter/server.py`（~400 行）+ 6 个子文件：

- `server.py` —— ACP 服务端
- `entry.py` —— 入口
- `session.py` —— session 管理
- `tools.py` —— 工具桥
- `auth.py` —— 认证
- `events.py` —— 事件
- `permissions.py` —— 权限
- `edit_approval.py` —— 编辑审批

**ACP**（Agent Client Protocol）是 VS Code / Zed / JetBrains 的标准协议，`acp_adapter` 让 Hermes 作为 **ACP server** 与 IDE 集成。**这是 Hermes 的第 6 类前端**。

### 2.7 6 前端差异点隔离总结

| Frontend | 进程模型 | 通信 | 差异点 |
|----------|---------|------|--------|
| **CLI** | 单进程（Rich + prompt_toolkit） | 内存 callback | 标准 IO、Ctrl+C interrupt |
| **TUI** | 双进程（Ink + Python） | stdio JSON-RPC | 渲染、键位、Tab 补全 |
| **Desktop** | 三进程（Electron + Python） | WebSocket JSON-RPC | 原生窗口、文件拖拽 |
| **Web Dashboard** | 浏览器 + PTY bridge | xterm.js + ptyprocess | 嵌入 TUI 不重写 |
| **Messaging** | asyncio + 20+ adapter | platform webhook | 平台特定输入/输出 |
| **ACP** | stdio JSON-RPC（IDE 集成） | ACP 协议 | edit approval / permissions |

**核心抽象**：每个 surface 都通过**同一份 `AIAgent.__init__` callback 矩阵**接入，差异点完全在 surface 层实现，**AIAgent 主体代码 0 修改**。

---

## 3. ProviderProfile 38 Provider 声明式

### 3.1 `ProviderProfile` 抽象基类（`providers/base.py`）

`providers/base.py:38` 定义：

```python
@dataclass
class ProviderProfile:
    """Base provider profile — subclass or instantiate with overrides."""

    # ── Identity ─────────────────────────────────────────────
    name: str
    api_mode: str = "chat_completions"
    aliases: tuple = ()

    # ── Auth & endpoints ─────────────────────────────────────
    env_vars: tuple = ()
    base_url: str = ""
    models_url: str = ""
    auth_type: str = "api_key"   # api_key|oauth_device_code|oauth_external|copilot|aws_sdk

    # ── Vision support ────────────────────────────────────────
    supports_vision: bool = False
    supports_vision_tool_messages: bool = True
    supports_prompt_cache_key: bool = False

    # ── External-process providers ──────────────────────────
    process_command: str = ""
    process_args: tuple = ()
    process_command_env_vars: tuple = ()
    process_args_env_var: str = ""

    # ── Model catalog ─────────────────────────────────────────
    fallback_models: tuple = ()

    # ── Client-level quirks ───────────────────────────────────
    default_headers: dict[str, str] = field(default_factory=dict)

    # ── Request-level quirks ─────────────────────────────────
    fixed_temperature: Any = None      # None=default, OMIT_TEMPERATURE=不发
    default_max_tokens: int | None = None
    default_aux_model: str = ""

    # ── Override hooks ──────────────────────────────────────
    def resolve_aux_model(self, *, vision: bool = False) -> str: ...
    def prepare_messages(self, messages): ...
    def build_extra_body(self, *, session_id, **context) -> dict: ...
    def build_api_kwargs_extras(self, *, reasoning_config, **context) -> tuple: ...
    def default_vision_model(self) -> str | None: ...
    def get_max_tokens(self, model) -> int | None: ...
    def supported_reasoning_efforts(self, model) -> tuple | None: ...
    def create_client(self, **client_kwargs) -> Any | None: ...
    def fetch_models(self, *, api_key, base_url, timeout=8.0) -> list[str] | None: ...
```

**关键设计**：
- **声明式而非命令式**：`ProviderProfile` 只声明"这个 provider 是什么"，不拥有 client 构造、凭据轮换、streaming —— 这些仍归 AIAgent。
- **三状态温度**：`fixed_temperature = None` 用调用者默认 / `OMIT_TEMPERATURE` 不发 / 具体数值直接用 —— 解决"Kimi: 服务器管温度"的特殊场景。
- **覆盖钩子**：`create_client()` 返回 `None` 表示用 core 默认 OpenAI client；返回非 None 表示 provider 自带 transport（如 ACP subprocess shim）。
- **`fetch_models()` 默认实现**：对 OpenAI-compatible endpoint 用 `urllib.request` Bearer 拉 `/models` 列表，subclass 可 override（Anthropic 改用 `x-api-key` + `anthropic-version` 头）。

### 3.2 注册中心 + Lazy Discovery（`providers/__init__.py`）

```python
# providers/__init__.py
_REGISTRY: dict[str, ProviderProfile] = {}
_ALIASES: dict[str, str] = {}
_PROVIDER_LIST_CACHE: list[ProviderProfile] | None = None
_discovered = False

_BUNDLED_PLUGINS_DIR = (
    Path(__file__).resolve().parent.parent / "plugins" / "model-providers"
)

def register_provider(profile: ProviderProfile) -> None:
    """Register a provider profile by name and aliases.
    Later registrations with the same name replace earlier ones — so user
    plugins under ``$HERMES_HOME/plugins/model-providers/`` can override
    bundled profiles without editing repo code.
    """
    global _PROVIDER_LIST_CACHE
    _REGISTRY[profile.name] = profile
    for alias in profile.aliases:
        _ALIASES[alias] = profile.name
    _PROVIDER_LIST_CACHE = None

def get_provider_profile(name: str) -> ProviderProfile | None:
    if not _discovered:
        _discover_providers()
    canonical = _ALIASES.get(name, name)
    return _REGISTRY.get(canonical)
```

**关键设计**：
- **三处可注册**：
  1. Bundled plugins：`<repo>/plugins/model-providers/<name>/`
  2. User plugins：`$HERMES_HOME/plugins/model-providers/<name>/`
  3. Pip-installed plugins：`hermes_agent.plugins` entry point
- **Last-writer-wins**：同名 provider 后注册覆盖先注册，所以 user plugin 可覆盖 bundled plugin。
- **Lazy discovery**：第一次调用 `get_provider_profile()` 或 `list_providers()` 时才扫描 + import，启动时不需要加载所有 provider。
- **缓存失效**：`_PROVIDER_LIST_CACHE = None` 在每次 `register_provider()` 时失效。

### 3.3 Anthropic Profile 示例（声明式 + 极简 override）

`plugins/model-providers/anthropic/__init__.py`：

```python
class AnthropicProfile(ProviderProfile):
    """Native Anthropic — uses x-api-key header, not Bearer."""

    def fetch_models(self, *, api_key=None, base_url=None, timeout=8.0):
        """Anthropic uses x-api-key header and anthropic-version."""
        if not api_key:
            return None
        try:
            req = urllib.request.Request("https://api.anthropic.com/v1/models")
            req.add_header("x-api-key", api_key)
            req.add_header("anthropic-version", "2023-06-01")
            req.add_header("Accept", "application/json")
            with open_credentialed_url(req, timeout=timeout) as resp:
                data = json.loads(resp.read().decode())
            return [
                m["id"] for m in data.get("data", []) if isinstance(m, dict) and "id" in m
            ]
        except Exception as exc:
            logger.debug("fetch_models(anthropic): %s", exc)
            return None

anthropic = AnthropicProfile(
    name="anthropic",
    aliases=("claude", "claude-oauth", "claude-code"),
    api_mode="anthropic_messages",
    env_vars=("ANTHROPIC_API_KEY", "ANTHROPIC_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"),
    base_url="https://api.anthropic.com",
    auth_type="api_key",
    ...
)
```

**整个 Anthropic adapter 仅需声明 7 个字段 + override 1 个方法**。这是声明式抽象的力量。

### 3.4 38 Provider 清单（`plugins/model-providers/`）

```bash
ls /usr/local/LsmGitOpenSource/hermes-agent/plugins/model-providers/ | head -50
# actual / ai-gateway / alibaba / alibaba-coding-plan / anthropic / arcee
# / azure-foundry / bedrock / commandcode / copilot / copilot-acp / custom
# / deepinfra / deepseek / fireworks / gemini / gmi / huggingface / kilocode
# / kimi-coding / meta-ai / minimax / nebius-token-factory / nous / novita
# / nvidia / ollama-cloud / openai-codex / opencode-free / opencode-zen
# / openrouter / qwen-oauth / router / stepfun / upstage / vertex / xai
# / xiaomi / zai
```

共 **38 个** model provider。每个都是 `ProviderProfile` 子类 + 1 个 `__init__.py` + 1 个 `plugin.yaml`。

### 3.5 抽象层关键差异抹平

| Provider 特性 | 抹平方式 |
|---------------|---------|
| Auth: Bearer / x-api-key / AWS SigV4 / OAuth / ACP | `auth_type` 字段 + `default_headers` |
| Messages: chat_completions / anthropic_messages / codex_responses | `api_mode` 字段 + transport 路由 |
| 温度: 必须发 / 不能发 / 默认 | `fixed_temperature` = `None` / `OMIT_TEMPERATURE` / 具体值 |
| Multimodal: 接受图片 / 不接受 | `supports_vision` / `supports_vision_tool_messages` |
| prompt_cache_key: 支持 / 400 | `supports_prompt_cache_key` |
| Reasoning: 在 extra_body / 顶层 / 不支持 | `build_api_kwargs_extras()` split 拆分 |
| Vision 模型: hardcoded / 实时查 | `default_vision_model()` hook |
| 客户端: 标准 OpenAI / 自带 SDK / ACP 子进程 | `create_client()` 返回 None 或自带 client |
| Models 列表: 拉 endpoint / static / 无 | `fetch_models()` 默认 OpenAI-compatible + subclass override |

### 3.6 Provider Plugin 结构与 plugin.yaml

每个 provider 是独立目录：

```
plugins/model-providers/openrouter/
├── __init__.py          # register_provider(OpenRouterProfile())
├── plugin.yaml          # manifest: name/kind: model-provider/version/description
└── README.md            # 可选
```

**plugin.yaml 示例**（基于 `providers/__init__.py` 描述）：

```yaml
name: openrouter
kind: model-provider
version: 1.0.0
description: OpenRouter — multi-provider aggregator
```

---

## 4. CompressionCommitFence 四重防护

**这是 Hermes 最复杂、最有特色的并发原语**。它解决的不是普通并发问题，而是 **"detached worker 的 late commit 不能 clobber 后续 attempt"** 的 deadlock-class 问题。

### 4.1 为什么需要 Fence：死锁场景

`agent/conversation_compression.py:673-681` 文档明确说明：

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
```

**典型死锁场景**：
1. Worker A 持有 compression lock，正在做 LLM 摘要
2. Worker A 的 LLM 调用 hang（网络问题 / 客户端 bug）
3. 超时机制把 Worker A "踢出"，新 Worker B 抢到 lock
4. Worker B 开始新一轮压缩
5. **Worker A 的延迟 commit 触发**，把 A 的旧 summary 写入 session DB
6. Worker B 的 in-progress 状态被覆盖 → **数据损坏**

### 4.2 CompressionCommitFence 实现细节

`agent/conversation_compression.py:683-733` 的核心字段：

```python
def __init__(self, total_ceiling_seconds: float | None = None) -> None:
    self._lock = threading.Lock()
    self._cancelled = False
    self._commit_started = False
    # Lock-free commit-phase marker — begin_commit() RETAINS self._lock
    # until finish_commit, so any host-side observation that needs the lock
    # (try_cancel_before_commit) blocks for the whole commit. This Event is
    # set inside begin_commit while the lock is held but is READABLE WITHOUT
    # the lock, so a host can observe "a commit was admitted and may be in
    # flight" even while the commit itself is hung.
    self._commit_phase = threading.Event()
    # Lock-free admission revocation — set by revoke_commit_admission()
    # on ANY host unwind (KeyboardInterrupt, cancellation, unexpected
    # exception) without touching the fence lock, so a host that cannot
    # afford to block behind an in-flight commit can still guarantee no
    # FUTURE commit is admitted. Plain bool store — atomic in CPython.
    self._admission_revoked = False
    ...
    # Forward-progress telemetry: the compression worker touches this
    # whenever the streamed summary call produces a token.
    self._last_progress = time.monotonic()
    self._progress_observed = False
    self._deadline: float | None = None
    ...
    # Watermark fence — set by mark_commit_watermark_fenced() once the
    # commit path captures the session's active-row watermark at compression
    # start, so any row appended AFTER that point survives a late commit.
    self._commit_watermark_fenced = False
```

### 4.3 关键方法实现

`agent/conversation_compression.py:780-888`：

```python
def cancel_before_commit(self, cancel_event: Any = None) -> bool:
    """Cancel a pending commit, or wait for an active commit to finish."""
    with self._lock:
        if self._commit_started:
            if cancel_event is not None:
                cancel_event.set()
            return False  # 取消失败，commit 已经在边界
        self._cancelled = True
        if cancel_event is not None:
            cancel_event.set()
        return True  # 取消成功

def begin_commit(self, cancel_event: Any = None) -> bool:
    """Atomically admit commit unless a hard cancellation already won."""
    self._lock.acquire()
    if (
        self.is_cancelled
        or self._admission_revoked
        or (cancel_event is not None and bool(cancel_event.is_set()))
    ):
        self._cancelled = True
        self._lock.release()
        if self._admission_revoked:
            self.release_cancelled_compression_lock()
        return False
    self._commit_started = True
    self._commit_phase.set()
    return True

def finish_commit(self) -> None:
    """Leave a commit boundary entered by :meth:`begin_commit`."""
    self._commit_phase.clear()
    self._lock.release()
    if self._admission_revoked:
        self.release_cancelled_compression_lock()

@property
def commit_in_flight(self) -> bool:
    """Lock-free read: an admitted commit has begun and not yet finished."""
    return self._commit_phase.is_set()
```

### 4.4 双重并发原语：Generation Counter

`agent/conversation_compression.py:430-491` 还有一个独立的 generation counter，用于 compressor **属性**写入的并发控制：

```python
def _claim_compressor_attempt(compressor: Any) -> int:
    """Claim the compressor for a new attempt; returns its generation id.
    Monotonic per compressor instance. Any restore or cancelled-check
    mutation stamped with an OLDER generation becomes a no-op, so a
    detached, late-unwinding attempt cannot clobber its successor's state.
    """
    with _COMPRESSOR_ATTEMPT_LOCK:
        generation = int(getattr(compressor, "_compression_attempt_generation", 0) or 0) + 1
        try:
            compressor._compression_attempt_generation = generation
        except Exception:
            return 0  # sloted/frozen compressor, gen-0 disables guard
        return generation

def _compressor_attempt_is_current(compressor: Any, generation: int) -> bool:
    """True when *generation* still owns the compressor (or guard disabled)."""
    if not generation:
        return True
    with _COMPRESSOR_ATTEMPT_LOCK:
        return (
            int(getattr(compressor, "_compression_attempt_generation", 0) or 0)
            == generation
        )
```

**两条互不干扰的并发边界**：
- **Fence** 控制 commit **admission**（可以进入 commit 吗？）
- **Generation** 控制 compressor **属性写入**（可以改状态吗？）

文档原话（`agent/conversation_compression.py:421-424`）：
> "The commit fence still owns COMMIT admission; the generation owns compressor-ATTRIBUTE writes — two different boundaries."

### 4.5 Watermark Fence：防止覆盖活跃消息

`agent/conversation_compression.py:871-887`：

```python
def mark_commit_watermark_fenced(self) -> None:
    """Record that this attempt's commit is bounded by a start watermark.
    Called by the compression worker right after it captures
    ``get_active_message_watermark()`` under the durable compression
    lock (#75316/#87484). A watermark-fenced commit archives ONLY rows
    at or below the watermark; rows appended later — e.g. the user turn
    the host released at the turn-hold boundary (#97963) — are cloned
    as live concurrent tail.
    """
    self._commit_watermark_fenced = True

@property
def commit_watermark_fenced(self) -> bool:
    return self._commit_watermark_fenced
```

**核心思想**：commit 开始时记录"当前活跃行 watermark"，commit 只能归档 ≤ watermark 的行；> watermark 的行（如 user 在 turn-hold boundary 后追加的新消息）被**克隆**为 live concurrent tail，**不能被压缩覆盖**。

### 4.6 进度遥测：区分 SLOW vs HUNG

`agent/conversation_compression.py:742-779`：

```python
def touch_progress(self) -> None:
    """Record forward progress (e.g. a streamed summary token arriving).
    Called from the compression worker thread; read by async waiters via
    :meth:`seconds_since_progress`. A bare float store is atomic in
    CPython, so no lock is needed.
    """
    self._last_progress = time.monotonic()
    self._progress_observed = True

@property
def deadline_monotonic(self) -> float | None:
    """Publishing the instant itself lets the worker's stream consumer
    stop at exactly the moment the host stops waiting.
    """
    return self._deadline

def seconds_since_progress(self) -> float:
    return max(0.0, time.monotonic() - self._last_progress)
```

**核心思想**：slow-but-alive 的 summary 模型（还在产 token）不会被固定 wall-clock deadline kill；只有真正 hung（no progress for N seconds）的 worker 才会被超时机制清理。

### 4.7 防压缩截断的 Tool Pair 保护

`agent/context_compressor.py:4280-4313` 处理 **tool_call ↔ tool_result 配对** 跨压缩边界：

```python
def _truncate_tool_call_args_at(idx: int) -> bool:
    """Shrink large tool_call argument payloads at ``idx``."""
    msg = result[idx]
    if msg.get("role") != "assistant" or not msg.get("tool_calls"):
        return False
    new_tcs = []
    modified = False
    for tc in msg["tool_calls"]:
        if isinstance(tc, dict):
            args = tc.get("function", {}).get("arguments", "")
            if len(args) > 500:
                new_args = _truncate_tool_call_args_json(args)
                if new_args != args:
                    tc = {**tc, "function": {**tc["function"], "arguments": new_args}}
                    modified = True
        new_tcs.append(tc)
    if modified:
        result[idx] = {**msg, "tool_calls": new_tcs}
    return modified
```

**关键**：
- `write_file` with 50KB content 不能在压缩中被破坏（否则下游 provider 400）
- truncation 在 parsed JSON 结构内进行，**保证结果仍是合法 JSON**
- 受保护 tail（pass4 pressure demotion）之外的 tool_call 才允许截断

`agent/context_compressor.py:4315-4335` 多 pass 处理：
```python
# Pass 2: Replace old tool results with informative summaries
for i in range(max(0, prune_boundary)):
    _demote_tool_result_at(i)
# Pass 3: Truncate large tool_call arguments in assistant messages
for i in range(max(0, prune_boundary)):
    _truncate_tool_call_args_at(i)
# Pass 3.5: retire image payloads that pass 2 cannot reach
pruned += _retire_stale_tool_result_images(result)
# Pass 4: protected-tail pressure demotion
...
```

### 4.8 整体架构：6 阶段压缩流水线

```
1. preflight check (turn_context.py)        # 每轮开头判断是否需要压缩
   ↓
2. Cooldown Under Lease                     # _capture_authoritative_cooldown_under_lease (conversation_compression.py:615)
   ↓                                        # 跨进程 lease 锁（Desktop + CLI + Gateway）
3. Attempt Generation Claim                 # _claim_compressor_attempt → generation (line 430)
   ↓
4. CompressionCommitFence                   # begin_commit / finish_commit (line 673)
   ↓                                        # commit admission 控制
5. LLM 摘要生成 or Micro-compaction         # _do_compact or native_compaction
   ↓                                        # 进度遥测 → forward-progress heartbeat
6. Session DB 写入 + FTS5 索引更新          # hermes_state.py
                                            # watermark-fenced → 活跃行保护
```

---

## 5. Skill 系统（agentskills.io 标准）

Hermes 的 Skill 系统是 **`agentskills.io` 开放标准**的严格实现，把 skill 作为"可被模型动态加载的用户级指令包"，并通过**注入为 user message** 保护 Anthropic prompt cache。

### 5.1 Skill 文件格式（agentskills.io 标准）

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

YAML frontmatter + Markdown 正文。frontmatter 字段：`name` / `description` / `frontmatter_version` / `platform` / `environment`（可选）。

### 5.2 Frontmatter 解析（`tools/skills_tool.py:570`）

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

**关键**：
- 用 `ruamel.yaml` 而非 PyYAML，因为 ruamel 保留注释和顺序，支持 round-trip 写回
- frontmatter 解析失败时**回退到空 dict**（不抛异常），保证 skill 仍然可加载

### 5.3 Skill 发现 4 来源（`agent/skill_commands.py:427-465`）

```python
def scan_skill_commands() -> Dict[str, Dict[str, Any]]:
    """Scan ~/.hermes/skills/ and return a mapping of /command -> skill info."""
    global _skill_commands, _skill_commands_platform, _skill_commands_home
    platform = _resolve_skill_commands_platform()
    home = _resolve_skill_commands_home()
    commands: Dict[str, Dict[str, Any]] = {}
    try:
        from tools.skills_tool import _skills_dir, _parse_frontmatter, ...
        ...
        project_dirs = list(get_project_skills_dirs())
        dirs_to_scan = list(project_dirs)
        skills_dir = _skills_dir()
        if skills_dir.exists():
            dirs_to_scan.append(skills_dir)
        dirs_to_scan.extend(get_external_skills_dirs())

        for scan_dir in dirs_to_scan:
            _iter = (
                iter_project_skill_files(scan_dir)
                if scan_dir in project_dirs
                else iter_skill_index_files(scan_dir, "SKILL.md")
            )
            for skill_md in _iter:
                if any(part in {'.git', '.github', '.hub', '.archive'} for part in skill_md.parts):
                    continue
                try:
                    content = skill_md.read_text(encoding='utf-8')
                    frontmatter, body = _parse_frontmatter(content)
                    # Skip skills incompatible with the current OS platform
                    if not skill_matches_platform(frontmatter):
                        continue
                    # Skip skills not relevant to the current runtime env
                    if not skill_matches_environment(frontmatter):
                        continue
                    name = frontmatter.get('name', skill_md.parent.name)
                    ...
                    description = frontmatter.get('description', '')
                    ...
                    seen_names.add(name)
                    cmd_name = name.lower().replace(' ', '-').replace('_', '-')
                    cmd_name = _SKILL_INVALID_CHARS.sub('', cmd_name)
                    ...
```

**关键**：
- **优先级**：project dirs（`.hermes/skills/`）> 本地（`~/.hermes/skills/`）> 外部（`optional-skills/`）
- **平台过滤**：`skill_matches_platform(frontmatter)` 允许 weather skill 只在 desktop 上出现
- **环境过滤**：`skill_matches_environment(frontmatter)` 区分 docker / local / ssh
- **目录排除**：`.git` / `.github` / `.hub` / `.archive` 跳过

### 5.4 Skill 注入为 User Message（保护 Prompt Cache）

`agent/skill_commands.py:701-712` + `_build_skill_message`（line 311）：

```python
activation_note = (
    f'[IMPORTANT: The user has invoked the "{skill_name}" skill, indicating they want '
    "you to follow its instructions. The full skill content is loaded below.]"
)
return _build_skill_message(
    loaded_skill,
    skill_dir,
    activation_note,
    user_instruction=user_instruction,
    runtime_note=runtime_note,
    session_id=task_id,
)
```

**关键设计**：
- `agent/skill_commands.py:54` 定义前缀 sentinel：
  ```python
  _SKILL_INVOCATION_PREFIX = "[IMPORTANT: The user has invoked the "
  ```
- skill 内容被包装为 user message，**不修改 system prompt**
- 注入位置在 user message 序列的尾部追加，**不破坏 Anthropic prompt cache control 的 `cache_control: ephemeral` 标记**

### 5.5 Stacked Skill Invocation（多 Skill 一次调用）

`agent/skill_commands.py:729-843` 支持 `/skill-a /skill-b do XYZ` 多 skill 一次调用：

```python
_MAX_STACKED_SKILLS = 5

def split_stacked_skill_commands(rest: str) -> tuple[list[str], str]:
    """Consume additional leading ``/skill`` tokens from *rest*."""
    keys: list[str] = []
    remaining = rest or ""
    while len(keys) < _MAX_STACKED_SKILLS - 1:
        stripped = remaining.lstrip()
        if not stripped.startswith("/"):
            break
        parts = stripped.split(None, 1)
        token = parts[0]
        tail = parts[1] if len(parts) > 1 else ""
        cmd_key = resolve_skill_command_key(token.lstrip("/"))
        if cmd_key is None or cmd_key in keys:
            break
        keys.append(cmd_key)
        remaining = tail
    return keys, remaining.strip()

def build_stacked_skill_invocation_message(...):
    """Build the user message for a stacked multi-skill slash invocation."""
    ...
    header_lines = [
        f'[IMPORTANT: The user has invoked the "{typed}" stacked skill bundle, '
        f"loading {len(loaded_names)} skills together. Treat every skill below "
        "as active guidance for this turn.]",
        "",
        f"Skills loaded: {', '.join(loaded_names)}",
    ]
    ...
```

设计灵感来自 Claude Code v2.1.199（2026-07-02）。

### 5.6 Skill 多层守卫

| 守卫 | 文件 | 作用 |
|------|------|------|
| `skills_guard.py` | `tools/skills_guard.py` | 检测 dangerous frontmatter（如 attempts to override system prompt） |
| `skills_ast_audit.py` | `tools/skills_ast_audit.py` | AST 审计（检查 skill 是否引用危险路径 / 可疑 import） |
| `skill_linter.py` | `tools/skill_linter.py` | Lint（frontmatter 合法性） |
| `skill_provenance.py` | `tools/skill_provenance.py` | 来源追踪（从哪里加载） |
| `skills_sync.py` | `tools/skills_sync.py` | 跨机器同步 |
| `skills_hub.py` | `tools/skills_hub.py` | Skill hub（共享中心） |
| `skillevaluator_scan.py` | `tools/skillevaluator_scan.py` | Skill 评估器（扫描使用频率 + 自动标记低效 skill 待重写） |

### 5.7 Skill 与 Prompt 模板关系

`agent/skill_commands.py:_build_skill_message`（line 311-410）：
- **Template substitution**：支持 `{{variable}}` 替换
- **Inline shell expansion**：支持 `$(command)` 内联 shell 执行（可控 timeout）
- **Activation note**：标记 skill 被调用的状态
- **Skill directory injection**：把 skill 目录的绝对路径注入，模型可访问 `references/` / `templates/` / `scripts/` / `assets/`
- **Skill config injection**：把 frontmatter 的 config 字段解析后注入

```python
parts = [activation_note, "", content.strip()]
if skill_dir:
    parts.append("")
    parts.append(f"[Skill directory: {skill_dir}]")
    parts.append(
        "Resolve any relative paths in this skill (e.g. `scripts/foo.js`, "
        "`templates/config.yaml`) against that directory, then run them "
        "with the terminal tool using the absolute path."
    )
```

---

## 6. 记忆系统（FTS5 + Trigram）

Hermes 的会话与记忆系统是 **SQLite + FTS5 + 多 Profile + 8 种 MemoryProvider ABC** 的多层架构，支持跨会话全文检索和长期记忆。

### 6.1 SessionDB（SQLite + FTS5）

`hermes_state.py`（17317 行）+ `hermes_state_search.py` 提供完整 session 持久化：

- **存储**：SQLite，多 Profile（`hermes_constants.py:get_hermes_home()`）
- **FTS5 全文搜索**：`hermes_state_schema.py:265-272`：
  ```python
  self._ensure_fts_schema(cursor, "messages_fts", FTS_SQL)
  # FTS5 virtual table over messages table
  # LEGACY_FTS_TRIGRAM_SQL, whose CREATE VIRTUAL TABLE needs the trigram
  # tokenizer for CJK and other languages without word boundaries.
  self._ensure_fts_schema(cursor, "messages_fts_trigram", LEGACY_FTS_TRIGRAM_SQL)
  ```

- **CJK Trigram**：`hermes_state_schema.py:380-410` 处理 CJK 语言分词问题（中文/日文/韩文没有空格边界）
- **FTS5 trigger 迁移**：`_migrate_broad_fts_update_triggers()`（行 218）收敛 broad trigger 到 narrow `AFTER UPDATE OF` 提高性能

### 6.2 FTS5 Query 清理（`hermes_state_search.py:1197-1275`）

```python
def _sanitize_fts5_query(query: str) -> str:
    """Sanitize user input for safe use in FTS5 MATCH queries.
    - Truncate to MAX_FTS5_QUERY_CHARS
    - Strip unmatched FTS5-special characters that would cause errors
    - Wrap unquoted hyphenated and dotted terms in quotes so FTS5 treats them as literal phrases
    """
    # Step 1: Length cap
    query = query[:MAX_FTS5_QUERY_CHARS]
    # Step 2: Strip remaining FTS5-special characters
    sanitized = _FTS5_SPECIAL_RE.sub(" ", sanitized)
    # Step 2b: % exclusion
    ...
    # Step 3: Wrap hyphenated/dotted terms in quotes
    ...
```

`_FTS5_SPECIAL_CHARS = '+{}():"^@/#&|~[]<>,;!?$=\\'` —— FTS5 有自己 query grammar，用户输入的 `it's` / `gateway/run.py` / `user@host` / `a,b` / `50%` 都需要 escape。

### 6.3 跨会话搜索（`hermes_state_search.py:1738`）

`search_messages()`（行 1431）+ `search_sessions()`（`hermes_state.py:14887`）实现跨会话全文搜索。

`agent/memory_manager.py:433 class MemoryManager` 提供 FTS5 + LLM 摘要的结果排序：

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

### 6.4 MemoryProvider ABC（`agent/memory_provider.py:110`）

8 种内置实现（`AGENTS.md:759`）：

| Provider | 用途 |
|----------|------|
| `honcho` | dialectic user modeling（默认） |
| `mem0` | 自适应记忆 |
| `supermemory` | 超级记忆 |
| `byterover` | 字节游民 |
| `hindsight` | 后见之明 |
| `holographic` | 全息记忆 |
| `openviking` | 开放维京 |
| `retaindb` | 保留 DB |

`agent/memory_provider.py:1-32` 描述完整 lifecycle：

```python
"""Abstract base class for pluggable memory providers.

Memory providers give the agent persistent recall across sessions.
The MemoryManager enforces a one-external-provider limit to prevent
tool schema bloat and conflicting memory backends.

Lifecycle (called by MemoryManager, wired in run_agent.py):
  initialize()          — connect, create resources, warm up
  system_prompt_block()  — static text for the system prompt
  prefetch(query)        — background recall before each turn
  sync_turn(user, asst)  — async write after each turn
  get_tool_schemas()     — tool schemas to expose to the model
  handle_tool_call()     — dispatch a tool call
  shutdown()             — clean exit

Optional hooks (override to opt in):
  on_turn_start(turn, message, **kwargs) — per-turn tick with runtime context
  on_session_end(messages)               — end-of-session extraction
  on_session_switch(new_session_id, **kwargs) — mid-process session_id rotation
  on_pre_compress(messages) -> str       — extract before context compression
  on_memory_write(action, target, content, metadata=None) — mirror built-in memory writes
  on_delegation(task, result, **kwargs)  — parent-side observation of subagent work
  backup_paths() -> list[str]            — extra on-disk paths to include in `hermes backup`
"""
```

**Policy**（AGENTS.md:781-792）：set of built-in memory providers is **closed**，新增必须作为独立 plugin repo。

### 6.5 Trivial Prompt 过滤（`agent/memory_provider.py:81-107`）

```python
TRIVIAL_PROMPT_RE = re.compile(
    r'^(yes|no|ok|okay|sure|thanks|thank you|y|n|yep|nope|yeah|nah|'
    r'hi|hey|hello|yo|sup|'
    r'continue|go ahead|do it|proceed|got it|cool|nice|great|done|next|lgtm|k)'
    r'[\s!?.:;,"' + "'" + r'~' '""''—–…()\[\]{}<>*&^%$#@!+=` ]*$',
    re.IGNORECASE,
)

def is_trivial_prompt(text: Optional[str]) -> bool:
    """Return True if a user prompt is too trivial to warrant memory recall.
    Empty/whitespace-only input, slash commands, and bare greetings or
    acknowledgements (with optional trailing punctuation) all count as
    trivial. Callers use this to skip memory-provider prefetch/injection
    on turns that carry no semantic signal — saving a blocking network
    round-trip and preventing stale user-model context from derailing
    one-word replies.
    """
    if not text:
        return True
    stripped = text.strip()
    if not stripped:
        return True
    if stripped.startswith("/"):
        return True
    return bool(TRIVIAL_PROMPT_RE.match(stripped))
```

**核心思想**：trivial 提示词（empty / slash / yes/no/hi/thanks）不需要 memory prefetch —— 节省网络 round-trip + 防止 stale user-model 上下文破坏一句回复。

### 6.6 Pre-Compress Checkpoint API Version

`agent/memory_provider.py:48`：

```python
# Version 1 is the historical, implicit contract every provider is already
# on: best-effort on_pre_compress() with the raw message list. Version 2 is
# the opt-in fail-closed checkpoint contract (normalized evidence handoff +
# strict-mode failure propagation).
PRE_COMPRESS_CHECKPOINT_API_VERSION = 2
```

`agent/memory_provider.py:117`：

```python
class MemoryProvider(ABC):
    pre_compress_checkpoint_api_version = 1

    @property
    @abstractmethod
    ...
```

**核心设计**：
- **V1**：历史隐式契约，best-effort `on_pre_compress()` 传原始 messages
- **V2**：opt-in fail-closed 契约，规范化证据交接 + 严格模式失败传播

---

## 7. 多轮对话与循环

### 7.1 单层 while + 4 类闸门

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

### 7.2 IterationBudget + Grace Call 双机制

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

### 7.3 Steer / Redirect 双机制（不破坏 Prompt Caching）

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

### 7.4 Durable Turn Lease（跨进程 Turn 锁）

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

### 7.5 Turn Liveness 标记（避免 review 抢占）

`run_agent.py:9265` 注释：

> "Turn liveness for the deferred-review idle queue: a queued review must not dispatch into the settle gap between two quick prompts. Marked inside the try below so the balancing note_turn_finished in its finally covers every exit."

```python
try:
    _review_queue.note_turn_started()
    ...
finally:
    _review_queue.note_turn_finished()
```

### 7.6 Turn Context 抽象（agent/turn_context.py）

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

### 7.7 流式响应边界

`stream_callback` 接收每个 text delta，触发 **TTS pipeline**：

> "stream_callback: Optional callback invoked with each text delta during streaming. Used by the TTS pipeline to start audio generation before the full response. When None (default), API calls use the standard non-streaming path."

（`agent/conversation_loop.py:2067-2069`）

### 7.8 设计要点

1. **单层循环 + 多闸门**：与 pi 的双层循环不同，Hermes 是单层 `while` + interrupt / budget / steer / redirect 四类闸门
2. **Steer 是 append 到 tool message**（不是新建 user message）—— 保护 prompt caching
3. **Grace call** 是"温柔终止"机制，laew 也没有
4. **Turn Context 单独模块化**（1709 行）—— 关注点分离
5. **与 laew 对比**：laew 的 `Yolo Runner` 是"先分类再分发"的两阶段；Hermes 是"单循环 + 多闸门"的一阶段

---

## 8. 流式与终端渲染

### 8.1 流式响应

Hermes 的流式响应通过 `stream_callback` 实现，每个 text delta 触发 TTS pipeline：

> "stream_callback: Optional callback invoked with each text delta during streaming. Used by the TTS pipeline to start audio generation before the full response."

### 8.2 TUI 渲染（Ink + React）

`ui-tui/src/app.tsx`（~500 行）是 Ink 主 App 组件，通过 `gatewayClient.ts`（~300 行）与 Python backend 通信。

**关键设计**：
- TypeScript 拥有屏幕渲染
- Python 拥有 session、tools、model calls、slash command 逻辑
- 通过 stdio JSON-RPC 双向通信

### 8.3 Desktop 渲染（Electron + React）

`apps/desktop/` 是 Electron + React + nanostore，通过 `requestGateway(method, params)` 与 `tui_gateway` 后端通信：

```typescript
// apps/desktop — Electron + React
// 通过 WebSocket / JSON-RPC 与 Python tui_gateway 通信
// 不嵌入 hermes --tui（独立 chat surface）
```

特殊设计：spawn `hermes serve`（headless backend），`HERMES_SERVE_HEADLESS=1` 让 `mount_spa()` 禁用 SPA。

### 8.4 Web Dashboard 渲染（xterm.js）

Web Dashboard 通过 `ptyprocess` 嵌入真实的 `hermes --tui`，使用 xterm.js 的 WebGL renderer：

```
Browser → React ChatPage → xterm.js (WebGL) ← ptyprocess(PTY) → hermes --tui 进程
```

---

## 9. 错误处理与容错

### 9.1 工具输出限制（tools/tool_output_limits.py）

```python
# tools/tool_output_limits.py —— 工具输出限制
# 防止 bash 输出过多 token（500 行或 32KB）
```

### 9.2 Coerce Tool Args（LLM 输出容忍）

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

### 9.3 Check_fn TTL 缓存（防止外部探测抖动）

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

### 9.4 Iteration Budget Refund（失败不浪费预算）

`agent/conversation_loop.py:2949/3071/3132/3188/7295` 多处 `agent.iteration_budget.refund()`：

```python
agent.iteration_budget.refund()  # dropped tool call 不消耗预算
```

### 9.5 进度遥测 vs Wall-clock Deadline

CompressionCommitFence 的 `touch_progress()` / `seconds_since_progress()` 区分 SLOW vs HUNG：

- slow-but-alive 的 summary 模型（还在产 token）不会被固定 wall-clock deadline kill
- 只有真正 hung（no progress for N seconds）的 worker 才会被超时机制清理

---

## 10. 配置系统

### 10.1 配置加载（3 种 Loader）

`AGENTS.md:626-650` 描述 3 种配置加载方式：

1. **CLI 配置**：`cli-config.yaml.example` —— 用户级配置
2. **Gateway 配置**：`gateway/config.py` —— 网关级配置
3. **Plugin 配置**：`plugin.yaml` —— 插件级配置

### 10.2 多 Profile 隔离

`hermes_constants.py:get_hermes_home()` —— 支持 `~/.hermes/<profile_name>/` 多 profile（互相隔离）

### 10.3 依赖固定（Exact Pin 政策）

`pyproject.toml:46-49` 注释：

> "Rationale: ranges allow PyPI to ship a fresh version of a transitive at any time without a code review on our side. Exact pins mean the only way a new package version reaches a user is via an intentional update on our end (bump the pin in this file, regenerate uv.lock). This was tightened on 2026-05-12 in response to the Mini Shai-Hulud worm hitting mistralai 2.4.6 on PyPI"

与 laew 相比：laew 用 Cargo.toml + Cargo.lock 也是同等严格策略（lockfile ground truth）。

### 10.4 极强的本地化（16 语言 i18n）

`locales/` 含 16 种语言（en/zh/zh-hant/es/fr/de/ja/ko/pt/ru/uk/it/nl/af/ar/tr/hu/ur）。`TUI busy-indicator styles` 在 `hermes_constants.py` 是"前后端单一事实源" —— Python 端 `INDICATOR_STYLES` / `DEFAULT_INDICATOR_STYLE` 与 TypeScript 端 `ui-tui/src/app/interfaces.ts` 的 `INDICATOR_STYLES` / `DEFAULT_INDICATOR_STYLE` 保持同步。

与 laew 相比：laew 目前仅中文 UI（`hermes_constants.py` 注释说明），未做 i18n。

---

## 11. 遥测与可观测性

### 11.1 日志系统

`hermes_logging.py` —— `setup_logging()` 提供 profile-aware 日志（agent.log / errors.log / gateway.log）

### 11.2 副 LLM 任务统计

`agent/aux_accounting.py` —— 副 LLM（curator / vision / embedding / title）任务统计

### 11.3 账户用量

`agent/account_usage.py` / `agent/billing_usage.py` / `agent/billing_view.py` / `agent/billing_links.py` —— 完整的用量统计与计费视图

### 11.4 可观测性插件

`plugins/observability/` —— metrics / log / trace 插件

### 11.5 进度遥测（CompressionCommitFence）

`agent/conversation_compression.py:742-779` —— `touch_progress()` / `seconds_since_progress()` / `deadline_monotonic` 提供 forward-progress heartbeat

---

## 12. 对 laew 的借鉴

### 12.1 综合对比（Hermes vs Laew）

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
| Provider 数量 | 38 plugin | 2 |
| 多 UI 表面 | CLI / TUI / Desktop / Web / Messaging / ACP | TUI + -p 单轮 |
| Session | SQLite + FTS5 + 多 Profile | SQLite session_memory |
| 学习循环 | **自改进**（skill auto-creation + Honcho） | SessionContext Agent 摘要 |
| Plugin 体系 | general / memory / provider / context-engine / cron / kanban（6 类） | 仅 tools/protocol 扩展 |
| 终端后端 | 7 种（local/docker/ssh/modal/daytona/singularity/vercel） | 仅本地 |
| i18n | 16 语言 YAML | 仅中文 |
| 依赖固定 | exact pin（PyPI 蠕虫防御） | Cargo.lock ground truth |
| 核心哲学 | **窄腰 + 自改进 + 多 surface** | **多 Agent 内置编排** |

### 12.2 P0 必做（2-4 周可完成）

**P0-1 Callback 矩阵抽象**（`src/agent/mod.rs`）
- 给 `Agent::run_session` 增加 callback 矩阵：`tool_progress_callback` / `tool_start_callback` / `tool_complete_callback` / `stream_callback` / `clarify_callback` / `interrupt`
- 借鉴 `run_agent.py:467 AIAgent.__init__` 的 ~60 参数矩阵设计
- 让未来多 surface（TUI + Desktop + Web + Messaging）复用同一份 agent 主体
- 工作量：3-5 天

**P0-2 Interrupt 双方法**（改造 `src/agent/mod.rs`）
- 拆分 `interrupt()` / `hard_interrupt()` 双方法
- `hard_interrupt` 直调父类实现，绕过 trait 动态分派
- 前端可 `hasattr(agent, 'hard_interrupt')` 探测，fallback 旧 ABI
- 借鉴 `run_agent.py:3525` + `run_agent.py:3830`
- 工作量：1-2 天

**P0-3 FTS5 跨会话搜索**（`src/config/mod.rs::Db` + 迁移）
- 给 `session_memory` 表加 FTS5 虚拟表 + trigram tokenizer（CJK 支持）
- 借鉴 `hermes_state_schema.py:265-272` + `hermes_state_search.py:1197`
- 用户可 `grep "..."` 跨所有 session 检索历史对话
- 工作量：3-5 天

**P0-4 Skill 系统（agentskills.io 兼容）**（`src/agent/skills/`）
- 引入 `SKILL.md` + YAML frontmatter 标准，用 `serde_yaml` 解析
- 4 来源发现：Bundled + User + Project + Cargo feature gate
- 注入为 user message（保护 prompt cache），用 sentinel `<<<LAEW:SKILL_INVOCATION>>>`
- 多层守卫：guard / AST audit / lint / provenance（Rust 用 `syn`）
- 借鉴 `tools/skills_tool.py:570` + `agent/skill_commands.py:427`
- 工作量：7-10 天

### 12.3 P1 推荐做（4-8 周可完成）

**P1-1 Tool Pair 保护（压缩前）**（`src/agent/mod.rs` + 引入压缩时）
- `assistant(tool_calls) → tool` 必须成对存在或都不存在，不能"留一半"
- 大 tool_call arguments（> 500 字符）截断在合法 JSON 内
- 借鉴 `agent/context_compressor.py:4280-4313`
- 工作量：2-3 天（压缩引入时一起做）

**P1-2 Generation Counter 防止过期 Commit**（`src/agent/mod.rs` 引入压缩时）
- 给 compressor 加 `_compression_attempt_generation: AtomicU64` 字段
- 借鉴 `agent/conversation_compression.py:430-491`
- 防止 detached worker 的 late commit 覆盖 in-progress 状态
- 工作量：3-5 天

**P1-3 Trivial Prompt 过滤**（`src/tui/input.rs` + `src/agent/yolo.rs`）
- 加 trivial prompt 正则匹配（`/help` / `yes` / `hi` 等）
- 借鉴 `agent/memory_provider.py:81-107` 的 `TRIVIAL_PROMPT_RE`
- 跳过 trivial 提示的 memory prefetch / project context 注入（已注入则跳过）
- 工作量：1-2 天

**P1-4 MemoryProvider ABC**（`src/agent/memory_provider.rs`）
- 定义 `MemoryProvider` trait：`initialize` / `system_prompt_block` / `prefetch(query)` / `sync_turn` / `shutdown`
- 至少 2 种实现：`builtin file-based` + `honcho-style stub`
- 借鉴 `agent/memory_provider.py:110-117`
- 工作量：5-7 天

### 12.4 P2 长期规划（8 周+）

**P2-1 Web Dashboard 嵌入 TUI**（`apps/web/` + PTY bridge）
- 不重写聊天体验，嵌入真实 TUI（`portable-pty` crate 已成熟）
- xterm.js + WebGL renderer + ptyprocess → `laew --tui`
- 借鉴 `hermes_cli/web_server.py` + `hermes_cli/pty_bridge.py`
- 工作量：14-21 天

**P2-2 Provider Profile 声明式抽象**（`src/llm/mod.rs` + `src/llm/provider_profile.rs`）
- 把 `LlmClient` trait 改造为 `ProviderProfile` dataclass + 覆盖钩子
- 字段：`name` / `api_mode` / `aliases` / `base_url` / `auth_type` / `env_vars` / `fixed_temperature` / `supports_vision` / `supports_prompt_cache_key`
- Lazy discovery：第一次 `get_provider(name)` 时扫描 `~/.lsmagent/providers/<name>/` + Cargo feature gate
- 借鉴 `providers/base.py:38` + `providers/__init__.py:56-70`
- 工作量：14-21 天

**P2-3 Messaging Gateway**（adapter pattern）
- 定义 `PlatformAdapter` trait，20+ 平台 adapter（telegram / discord / slack / weixin / feishu ...）
- gateway 调度 AIAgent 处理 event，处理结果通过 adapter 发回平台
- 借鉴 `gateway/run.py` + `gateway/platforms/base.py`
- 工作量：30+ 天（每个平台 ~3 天）

**P2-4 Watermark Fence（保护活跃消息）**（`src/agent/mod.rs` 引入压缩时）
- commit 开始时记录 watermark，只能归档 ≤ watermark 的行
- 借鉴 `agent/conversation_compression.py:871-887`
- 与 Generation Counter 配合使用
- 工作量：5-7 天

### 12.5 核心借鉴方向总结

Hermes 的 7 个核心设计决策：

1. **窄腰 + Prompt Cache 神圣**：Skill 注入 user message，system prompt 中途不重建，toolset 不中途切换
2. **Footprint Ladder**：能力新增按留痕量由小到大分 6 级，强制走 plugin/MCP 路径
3. **Multi-Surface AIAgent**：同一 AIAgent 被 CLI / TUI / Desktop / Web / Messaging / ACP 6 个前端共享
4. **CompressionCommitFence**：generation counter 防止过期 commit 覆盖新工作
5. **Plugin Override 隔离**：core 默认赢，plugin override 必须显式声明
6. **Self-Improving Loop**：skill auto-creation + self-improvement + periodic nudges + Honcho
7. **Cache-Aware Steer/Redirect**：steer append 到 tool message（不破坏 cache），redirect 修改原 user message

laew 应**选择性借鉴**：**P0-1 Callback 矩阵 + P0-2 Interrupt 双方法 + P0-3 FTS5 搜索 + P0-4 Skill 系统** 是 4 周内可落地的关键改进，**P1-1/2/3/4 + P2-1/2/3/4** 是中长期演进方向。

**laew 与 Hermes 的核心差异**：laew 是 Rust 单进程内置 Yolo 编排 + SQLite session_memory；Hermes 是 Python 多 surface + 6 类 plugin 体系 + 自改进学习循环。**借鉴 Hermes 的"窄腰 + 多 surface + 插件化"哲学，但保持 laew 的"单进程 + Yolo + SQLite 简洁"优势**，是 laew 演进的最佳路径。

---

## 附录：关键源码路径索引

| 主题 | 文件路径 | 行号 |
|------|----------|------|
| AIAgent 类 | `run_agent.py` | 467 |
| AIAgent.run_conversation 转发 | `run_agent.py` | 9238 |
| 核心 while 循环 | `agent/conversation_loop.py` | 2289 |
| turn_context prologue | `agent/turn_context.py` | 全文 |
| IterationBudget import | `run_agent.py` | 162 |
| Interrupt 方法 | `run_agent.py` | 3525 |
| Hard Interrupt 方法 | `run_agent.py` | 3830 |
| Steer 方法 | `run_agent.py` | 3908 |
| Redirect 方法 | `run_agent.py` | 3944 |
| ProviderProfile 抽象 | `providers/base.py` | 38 |
| Provider 注册中心 | `providers/__init__.py` | 56 |
| Anthropic Profile | `plugins/model-providers/anthropic/__init__.py` | 全文 |
| OpenRouter Profile | `plugins/model-providers/openrouter/__init__.py` | 89 |
| CompressionCommitFence | `agent/conversation_compression.py` | 673 |
| Generation Counter | `agent/conversation_compression.py` | 430 |
| Watermark Fence | `agent/conversation_compression.py` | 871 |
| 进度遥测 | `agent/conversation_compression.py` | 742 |
| Tool Pair 截断 | `agent/context_compressor.py` | 4280-4313 |
| MemoryProvider ABC | `agent/memory_provider.py` | 110 |
| Trivial Prompt 过滤 | `agent/memory_provider.py` | 81-107 |
| Pre-Compress Checkpoint Version | `agent/memory_provider.py` | 48,117 |
| Skill 前缀 sentinel | `agent/skill_commands.py` | 54 |
| Skill frontmatter 解析 | `tools/skills_tool.py` | 570 |
| Skill 4 来源发现 | `agent/skill_commands.py` | 427-465 |
| Skill 注入为 user msg | `agent/skill_commands.py` | 701-712 |
| Stacked Skill 一次调用 | `agent/skill_commands.py` | 729-843 |
| Skill 多层守卫 | `tools/skills_guard.py` / `skills_ast_audit.py` / `skill_linter.py` / `skill_provenance.py` | 全文 |
| SessionDB SQLite | `hermes_state.py` | 17317 |
| FTS5 schema | `hermes_state_schema.py` | 265-272 |
| FTS5 query sanitize | `hermes_state_search.py` | 1197-1275 |
| 跨会话搜索 | `hermes_state_search.py` | 1431, 1738 |
| MemoryManager | `agent/memory_manager.py` | 433 |
| Messaging Gateway | `gateway/run.py` | 全文 |
| Platform Adapter 抽象 | `gateway/platforms/base.py` | 全文 |
| TUI Process Model | `ui-tui/src/app.tsx` + `tui_gateway/server.py` | 全文 |
| Web Dashboard PTY bridge | `hermes_cli/web_server.py` + `pty_bridge.py` | 全文 |
| ACP Adapter | `acp_adapter/server.py` | 全文 |
| 副 LLM 客户端 | `agent/auxiliary_client.py` | 全文 |
| Skill Auto-Creation | `agent/conversation_compression.py` + `agent/auxiliary_client.py` | 全文 |
| Background Review | `agent/background_review.py` | 全文 |
| Review Idle Queue | `agent/review_idle_queue.py` | 全文 |
| Honcho 集成 | `plugins/memory/honcho/` | 全文 |
| Skill Nudge | `agent/conversation_loop.py` | 2377 |
| Footprint Ladder 文档 | `AGENTS.md` | 103-149 |
| Plugin Compatibility Policy | `AGENTS.md` | 768-779 |
| Native Plugin compatibility contract | `website/docs/developer-guide/plugins/index.md#native-plugin-compatibility-contract` | 全文 |
| Hermes 整体哲学 | `AGENTS.md` | README 第一段 |
| Config Loaders 3 种 | `AGENTS.md` | 626-650 |
| 依赖固定策略 | `pyproject.toml` | 46-49 + `AGENTS.md` | 670-686 |

---

**报告完成日期**：2026-09-06
**总行数**：~1850 行（去重后）
**覆盖维度**：项目元信息 / 6 前端共享 / Provider 抽象 / CompressionCommitFence / Skill / 记忆系统 / 多轮对话 / 流式渲染 / 错误容错 / 配置系统 / 遥测 / 借鉴要点 12 条
# Hermes Agent 第十轮深挖 — 8 大新维度

> 调研对象: hermes-agent (Python, 859 MB, 6 前端共享 AIAgent 核心 + 38 provider)
> 调研日期: 2026-09-07
> 前 9 轮覆盖: 项目元信息 / 6 前端架构 / ProviderProfile / CompressionCommitFence / Skill / 记忆 FTS5 / 多轮对话 / 流式渲染 / 错误处理 / 配置 / 遥测
> 本轮 8 个全新维度: **CrashDump 与错误恢复 / WebUI 与 DesktopApp / OAuth 认证与多账号 / i18n 国际化 / Release 工程化与 AutoUpdate / WebSocket 与 SSE / DevContainer 与容器化 / CRDT 与多端冲突**

---

## 1. CrashDump 与错误恢复

### 1.1 启动存活看门狗 (`hermes_startup_watchdog.py`, 519 行)

守护进程线程,在进程入口武装、事件循环确认存活后 disarm,防止启动期死锁(OOF-298)。

```python
# hermes_startup_watchdog.py:44-79 — 关键常量
DEFAULT_STARTUP_WATCHDOG_TIMEOUT_S = 300.0   # 默认 5 分钟
_MIN_TIMEOUT_S = 30.0
_CPU_PROGRESS_MIN_S = 1.0                    # 窗口内 CPU 增量阈值
_MAX_CPU_EXTENSIONS = 3                      # CPU 回退最多 3 次
_MAX_LEASE_S = 900.0                         # 单次 phase lease 上限 15 min
_FIRE_EXIT_BOUND_S = 10.0                    # 取证路径硬上限
SERVICE_RESTART_EXIT_CODE = 75               # 供 s6/systemd 重启
```

**三级进度信号**(按权威顺序):
1. **Phase lease** (`report_startup_progress`) — 启动路径自身持有,可证明 I/O 密集阶段(如 state.db 损坏修复)仍活着
2. **Process-wide CPU** — 回退信号,最多 `_MAX_CPU_EXTENSIONS` 次
3. **Fire** — `faulthandler.dump_traceback(all_threads=True)` + JSON dump record + `os._exit(75)`

**取证 dump 格式** (`_write_dump_record`):
```json
{"ts":"2026-09-07T...", "tag":"startup_watchdog.fired", "pid":1234,
 "timeout_s":300, "elapsed_s":301.2, "extensions":0,
 "lease_count":0, "last_lease_phase":null, "exit_code":75}
```
落盘到 `<HERMES_HOME>/logs/gateway-startup-watchdog.log`,同时通过 `faulthandler.dump_traceback(file=fh)` 把全线程栈写入同一文件。

**退出护送** (`_exit_escort`): 独立守护线程,若取证路径自身卡住(日志锁被主线程持有 / 磁盘满),10s 后直接 `os._exit`,保证重启不被阻塞。

### 1.2 关闭取证 (`gateway/shutdown_forensics.py`, 269 行)

SIGTERM/SIGINT 同步处理,10ms 内完成快照:

```python
# gateway/shutdown_forensics.py:80-119
def snapshot_shutdown_context(received_signal=None) -> Dict[str, Any]:
    # 信号名/编号 + own/parent /proc 摘要 + systemd 父级推断
    # + TracerPid(调试器检测) + 1-min loadavg + takeover/planned-stop marker
```

**异步诊断** (`spawn_async_diagnostic`): fire-and-forget `ps auxf` / `pstree` / `dmesg` 走独立 subprocess(`start_new_session=True` 脱离 cgroup 避免被 `KillMode=control-group` 一起杀掉),5s 自超时。

**systemd 时序对齐** (`check_systemd_timing_alignment`): 启动时校验 `TimeoutStopSec` ≥ drain 预算,防止 systemd 在 drain 期间 SIGKILL cgroup。

### 1.3 重启循环断路器 (`gateway/restart_loop_guard.py`, 148 行)

持久化到 `<HERMES_HOME>/gateway/restart_loop.json`,链式 inter-boot gap 检测:

```python
DEFAULT_MAX_RESTARTS = 3       # 3 次重启即触发
DEFAULT_WINDOW_SECONDS = 60
DEFAULT_MAX_GAP_SECONDS = 300  # 间隔 >5min 视为新链
_MAX_STORED_BOOTS = 50
```

**链式算法** (`_chain_ending_at`): 倒序遍历 boot 时间戳,间隔 ≤ `max_gap` 则链连续;间隔 > gap 链断裂。触发后 **跳过 auto-resume**,打破 SIGTERM-respawn 循环(#30719, #81642)。任何 I/O 故障 fail open。

### 1.4 状态修复与 WAL 策略

#### 1.4.1 错误分类 (`hermes_state_errors.py`, 219 行)

```python
PERSISTENCE_ERROR_CAUSES = ("locked","compression","compression_closed",
                             "turn_lease","corrupt","replaced","disk","unknown")

class StateDbCorruptError(sqlite3.DatabaseError): ...      # 结构损坏,隔离 handle
class StateDbReplacedError(RuntimeError): ...               # 文件被 cp/mv 替换
class DeletedWalGenerationError(StateDbReplacedError): ...  # WAL inode 已删
class SessionTurnLeaseLostError(RuntimeError): ...          # fencing 失效
```

`classify_persistence_error(exc_or_str) -> str`: 先按类型匹配(lease 拒绝不含 "locked"/"busy"),再按短语匹配,**corruption 必须在 disk 之前**("disk image is malformed" 含 "disk")。

#### 1.4.2 修复策略 (`hermes_state_repair.py`, 1082 行)

**跨进程修复锁** (`_cross_process_repair_lock`): `flock` + pid+start_time 记录,持有者死锁自动打破;VACUUM 可能数分钟故获取有界。

**持久化尝试账本** (`_MAX_PERSISTENT_REPAIR_ATTEMPTS = 3`):
- 指纹 = `size + sha256(head64KiB + tail64KiB)[:32]`,屏蔽 volatile header 范围(24-28 commit counter, 92-95 version-valid-for)
- 同指纹失败 3 次后拒绝进一步手术,提示 `hermes sessions recover --source <db> --output recovered-state.db`
- 修复/替换改变指纹 → 计数重置

**取证备份去重** (`_backup_db_file`):
- 字节级 identity = 整个 main file + 所有 sidecar 的 sha256(用于去重)
- 与 `_db_fingerprint`(仅 head+tail 采样,用于 ledger key)严格区分
- 上限 `_MAX_MALFORMED_BACKUPS = 3`,最老的自动清理
- 磁盘预算: `max(256MiB, 2% volume)` headroom,否则 **HARD STOP**

#### 1.4.3 WAL 模式策略 (`hermes_state_wal.py`, 491 行)

**WAL-reset bug 检测** (`is_sqlite_wal_reset_vulnerable`): SQLite 3.7.0–3.51.2 存在 WAL-reset 损坏 bug,脆弱构建永不启用 WAL。

**回退消歧** (`_enable_wal`): "disk i/o error" 在 ZFS/APFS-CoW 是确定性不兼容,但也可能是瞬时 EIO → 重试 2 次后再判定,防止混合 journal 模式损坏。

**macOS 屏障**: `checkpoint_fullfsync=1` + `synchronous=FULL`,防 launchd 关机时 page cache 丢失导致 malformed image(#30636)。

### 1.5 Electron 主进程取证 (`apps/desktop/electron/crash-forensics.ts`, 52 行)

```typescript
export function installCrashForensics({ flush, log, target = process }): void {
  target.on('uncaughtException', record('Uncaught exception'))
  target.on('unhandledRejection', record('Unhandled rejection'))
}
```
同步写 `desktop.log` + 同步 flush(致命错误后无机会异步 flush)。

### 1.6 桌面修复循环守卫 (`apps/desktop/electron/bootstrap-repair-guard.ts`, 122 行)

纯决策 helper,区分 "venv 真坏" vs "运行时健康但 GIL 暂时卡住":

```typescript
export function decideBootstrapRepair(input: RepairDecisionInput): RepairDecision {
  // attempt ≤ maxSoftAttempts(默认 3) → soft restart(保留 venv)
  // attempt > maxSoftAttempts → hard reinstall
}
```

### 1.7 Session DB 可恢复缓存 (`gateway/session_db_recovery.py`, 146 行)

`RecoverableHandleCache`: 单 flight 打开 + 指数退避(1s→60s) + generation 防过时 handle 复活 + 健康状态发布到 `gateway/status`。

### 1.8 会话停滞通知 (`gateway/session_stall.py`, 75 行)

单一进度源 `AIAgent.get_activity_summary()`,notify-once 策略,idle ≥ `timeout_seconds` + 有 pending inbound 才发 "⚠️ Agent session appears stalled"。

---

## 2. WebUI 与 DesktopApp

### 2.1 Electron 桌面 (`apps/desktop/electron/main.ts`, 18,360 行)

**窗口拓扑**: 主聊天窗 + 多 session 子窗 + HUD 浮窗 + Quick Entry 全局热键小窗 + Pet 覆盖层 + 浏览器 popout。

**主窗创建** (`createWindow`, L14555):
```typescript
mainWindow = new BrowserWindow({
  ...computeWindowOptions(savedWindowState, screen.getAllDisplays()),
  titleBarStyle: 'hidden',
  titleBarOverlay: getTitleBarOverlayOptions(),
  show: false,  // 防 vibrancy 闪白
  webPreferences: chatWindowWebPreferences(PRELOAD_PATH)
})
```

**translucency 支持**: preload 同步 `sendSync('hermes:translucency:support')`,沙箱 preload 不能 import `node:os`,由主进程代查。

**Windows 沙箱回退**: GPU FATAL crash loop 时写 sticky `markerAfterSuccessfulBoot({fallbackActive, reason, appVersion})`,下次启动探测。

### 2.2 Preload 桥 (`apps/desktop/electron/preload.ts`, 541 行)

`contextBridge.exposeInMainWorld('hermesDesktop', {...})` 暴露:

| 命名空间 | 能力 |
|---------|------|
| `hud` | 浮窗 open/close/move/ignore-mouse/frost/session/game-overlay |
| `petOverlay` | 宠物覆盖层 bounds/focusable/state push/control |
| `quickEntry` | 全局热键小窗 settings/submit/pushState |
| `connections` | v2 多连接 registry list/save/remove/setPrimary/updateAll |
| `cloud` | Hermes Cloud status/login/logout/discover/agentSignIn |
| `mcpOauth` | MCP OAuth loopback listen/wait/cancel |
| `terminal` | PTY attach/start/write/onData/onExit |
| `git` | worktree/branch/diff/review(完整 CR list/stage/commit/push/PR) |
| `fs` | readDir/reveal/trash/rename/readFileDataUrl |
| `updates` | check/apply/branch/setBranch/onProgress |
| `findInPage` | Cmd/Cmd+F delegate |
| `zoom` | get/set/onChanged/factor(同步) |

### 2.3 共享 JSON-RPC 网关客户端 (`apps/shared/src/json-rpc-gateway.ts`, 755 行)

**GatewayEventName** (24 种):
```typescript
type GatewayEventName = 'gateway.ready' | 'session.info' | 'message.delta' |
  'message.interim' | 'message.complete' | 'thinking.delta' | 'reasoning.delta' |
  'reasoning.available' | 'status.update' | 'tool.start' | 'tool.progress' |
  'tool.complete' | 'tool.generating' | 'todo.updated' | 'clarify.request' |
  'approval.request' | 'sudo.request' | 'secret.request' | 'background.complete' |
  'error' | 'skin.changed' | ...
```

**JsonRpcGatewayClient** 核心机制:
- **心跳**: interval 15s, deadline 45s
- **请求超时**: 默认 120s,支持 per-call override + AbortSignal
- **重连回放**: `lastSeenSeq` per session + `replayEpoch`(进程重启后 seq 重置检测)
- **replayHold**: 回放期间 live 帧暂存,防重复派发
- **连接超时**: 15s 内未 open → fail to 'error' 供调用方重试

### 2.4 Web Dashboard (`web/`)

React SPA,`web/src/lib/gatewayClient.ts` 继承 `JsonRpcGatewayClient`,ticket 认证(非 legacy token)。

**事件重连** (`web/src/lib/events-reconnect.ts`, 85 行):
```typescript
EVENTS_RECONNECT_BASE_MS = 1_000
EVENTS_RECONNECT_MAX_MS = 30_000
EVENTS_MAX_RECONNECT_ATTEMPTS = 15
EVENTS_CONNECT_TIMEOUT_MS = 15_000
// 4401/4403 = auth rejection → 终止,提示 reload
// 1000 = normal close → 不重试
```

### 2.5 TUI (`ui-tui/`)

Ink/React 终端渲染,`ui-tui/packages/hermes-ink/src/ink/ink.tsx`(2850 行)是 fork 的 Ink 运行时。

### 2.6 引导安装器 (`apps/bootstrap-installer/`)

Tauri 2 应用,`src-tauri/src/update.rs` 编排:
1. 等旧桌面进程退出(20s,500ms poll)
2. `hermes update --yes --gateway`
3. `hermes desktop --build-only`
4. 启动新桌面

**UpdateMarkerGuard** (RAII): 拥有 `.hermes-update-in-progress` 标记,`Drop` 在任何退出路径(含 panic)删除标记。标记 payload `{pid}\n{started_at_unix}`,20min 陈旧上限。

---

## 3. OAuth 认证与多账号

### 3.1 RFC 8252 原生 PKCE 流 (`apps/desktop/electron/native-oauth.ts`, 256 行)

网关代理模式: 上游 IDP(Nous Portal) 签发 per-gateway `client_id`,只接受网关 origin 的 `redirect_uri`,所以桌面不能直连 Portal。网关暴露 `/auth/native/{authorize,token,refresh}`。

```typescript
export function generatePkcePair(): NativePkcePair {
  const verifier = b64url(randomBytes(32))   // 43 chars, RFC 7636 §4
  const challenge = b64url(sha256(verifier))
  return { verifier, challenge, method: 'S256' }
}

export function resolveLoginStrategy(status, opts): 'native' | 'embedded' {
  // forceEmbedded → embedded
  // 无 non-password provider → embedded
  // status.auth_flows 含 'native_pkce' → native
  // 否则 → embedded(兼容旧网关)
}
```

**回调解析** (`parseLoopbackCallback`): 严格 state 校验(CSRF),不匹配直接抛。

### 3.2 登录驱动 (`apps/desktop/electron/native-oauth-login.ts`)

依赖注入(`openExternal` / `postJson` / `createServer` / `now` / `timeoutMs`)可单元测试。

安全姿态:
- loopback 绑定 127.0.0.1 临时端口,收到回调或超时立即关闭
- state 校验后才 redeem code
- PKCE verifier 在 token POST 前不出进程
- 浏览器只看到 "You can close this window" HTML,不见 token

### 3.3 加密存储 (`apps/desktop/electron/native-token-store.ts`, 166 行)

`safeStorage.encrypt` → userData store file(mode 0600)。

**URL 脱敏** (`redactGatewayUrl`): 剥 userinfo 防 credential 泄漏到日志。

**双解析路径**: 网关响应 snake_case 用 `parseTokenResponse`;存储 blob camelCase 用 `parseStoredTokenSet`。混用导致每次重启 "signed out"(#73271)。

### 3.4 多账号/多连接

preload `connections` namespace 暴露 v2 registry: `list/save/remove/setPrimary/setLaunchMode/test/updateManaged/updateAll/onChanged`。

`cloud` namespace: Hermes Cloud 门户登录 → 发现 agent → 静默 per-agent 登录(cloud-auto-discovery Phase 3)。

---

## 4. i18n 国际化

### 4.1 CLI 静态消息目录 (`locales/`, 16 语言 YAML)

`locales/en.yaml`(474 行) 为 source of truth。仅覆盖 CLI approval prompt 与少量 gateway slash-command 回复,**不翻译** agent 输出 / 日志 / 错误 traceback / 工具输出。

**键结构**:  dotted path,`{placeholder}` token,新增键必须同一 commit 加到 en/zh/ja/de/es/fr/tr/uk(`tests/agent/test_i18n.py` 断言 catalog parity)。

16 语言: en / zh / zh-hant / es / fr / de / ja / ko / pt / ru / uk / it / ga / af / tr / hu / ar。

### 4.2 Web Dashboard i18n (`web/src/i18n/`, 21 个 locale 文件)

**类型安全** (`web/src/i18n/types.ts`, 876 行):
```typescript
export type Locale = "en" | "zh" | "zh-hant" | "ja" | "de" | "es" | "fr" |
  "tr" | "uk" | "af" | "ko" | "it" | "ga" | "pt" | "ru" | "hu" | "ar";
export interface Translations { common: {...}; app: {...}; status: {...};
  sessions: {...}; analytics: {...}; models: {...}; logs: {...}; cron: {...};
  pluginsPage: {...}; profiles: {...}; skills: {...}; config: {...}; env: {...};
  oauth: {...}; language: {...}; theme: {...}; achievements: {...}; kanban: {...} }
```

**部分翻译合并** (`define-locale.ts`): 新 locale 只覆写有翻译的键,缺失键回退英文,未知键仍 type-error。

**RTL 支持**: `RTL_LOCALES = new Set(["ar"])` → Tailwind logical utilities 翻转。

**语言切换器** (`LanguageSwitcher.tsx`): 显示 endonym(本族语名),**不用国旗**(语言≠国家)。窄屏 bottom sheet portaled to body。

### 4.3 文档站点翻译 (`website/i18n/`)

Docusaurus,`zh-Hans` 分支含 313 个翻译 `.md` 文件。

### 4.4 前后端同步

`hermes_constants.py` 的 `INDICATOR_STYLES` / `DEFAULT_INDICATOR_STYLE` 与 `ui-tui/src/app/interfaces.ts` 保持同步,是 TUI busy-indicator 的"前后端单一事实源"。

---

## 5. Release 工程化与 AutoUpdate

### 5.1 CalVer + Semver 发布 (`scripts/release.py`, 2648 行)

```bash
python scripts/release.py --bump minor --publish --date 2026.3.15
```

**双版本体系**:
- **CalVer tag**: `v2026.9.7` / `v2026.9.7.2`(同日多次 release 自动加后缀)
- **Semver**: `__init__.py` + `pyproject.toml` + `apps/desktop/package.json` 同步

**Changelog 生成**: conventional commit 分类 + PR 号提取 + co-author 解析 + contributor 目录合并(`contributors/emails/` 每邮箱一文件,无 merge conflict)。

**发布流程**: bump version files → git commit → tag → `gh release create` → 上传产物。

### 5.2 应用内更新 (Electron)

**更新门控** (`update-gate.ts`): 双信号 — 磁盘 marker + 进程内 `updateInFlight` flag。单 marker 不够(#73822): `applyUpdates` 早杀 backend 后 marker 写入前,renderer 1s 内重连会 spawn 新 backend 进入更新临界区。

**更新标记** (`update-marker.ts`): `HERMES_HOME/.hermes-update-in-progress` 两行 `{pid}\n{started_at_unix}`,20min 陈旧上限,自愈。

**远程检测** (`update-remote.ts`): 官方 repo SSH remote 在被动检查时替换为 HTTPS `ls-remote`,防 FIDO2/passkey SSH key 触发硬件触摸提示。

**更新器进程** (`updater-process.ts`): Windows 用 repo 内 `scripts/desktop-update/windows.ps1`(frozen-binary 逃逸舱口,updater 自身 bug 无需等新 binary),POSIX 用 `posix.sh`。

### 5.3 Rust/Tauri 更新编排 (`apps/bootstrap-installer/src-tauri/src/update.rs`)

`UpdateMarkerGuard` RAII + `UPDATE_RUNNING` AtomicBool 防 re-entrancy(React strict-mode 双调用 / 窗口 reload)。

**阶段 manifest**: handoff → update → rebuild(+ macOS install stage),前端看起来像短 bootstrap。

---

## 6. WebSocket 与 SSE

### 6.1 TUI 网关 WS 传输 (`tui_gateway/ws.py`, 380 行)

复用 `tui_gateway.server.dispatch`,与 stdio Ink 同一套 handler。

**Token 合并** (`WSTransport`):
```python
_STREAMING_EVENT_TYPES = {"message.delta", "reasoning.delta", "thinking.delta"}
_TOKEN_COALESCE_S = 0.033  # ~30 fps
```
流式 token 缓冲 + 定时器批量 flush,非流式帧(工具/审批/状态/完成)立即 flush 并清空缓冲,保序。

**写安全**: `write()` 从 worker 线程调度到 loop,`write_async()` 从 loop 线程 await。`_safe_send_many` 在 `_send_lock` 下发送不可分割 batch。

**UTF-8 净化** (`_sanitize_ws_text`): 替换 lone surrogate,单帧无效不关闭连接(#97288)。

**Scale-to-zero 心跳**: 每 15s touch marker,`gateway/scale_to_zero.py` 读取 mtime 判断是否有 dashboard/desktop/TUI 客户端附着。

### 6.2 事件回放 (`tui_gateway/event_replay.py`, 101 行)

每 session 单调 `seq` + 512 事件 ring buffer + 64 session FIFO 淘汰。

**Epoch 检测**: `_REPLAY_EPOCH = uuid.uuid4().hex`,进程重启后 seq 从 1 重置,客户端通过 `gateway.ready` / `session.events.since` 的 epoch 变化检测重启并重置高水位。

**truncated 标记**: `is_truncated` 告知客户端 ring 已绕回,需 refetch history 而非信任回放。

### 6.3 浏览器 JSON-RPC 客户端 (`apps/shared/src/json-rpc-gateway.ts`)

- 心跳 15s / deadline 45s
- 请求超时 120s,per-call override + AbortSignal
- 重连回放 `fetchReplay()` + `replayHold` 防重复派发
- 连接超时 15s

### 6.4 事件重连 (`web/src/lib/events-reconnect.ts`)

指数退避 1s→30s,15 次上限。4401/4403 auth 拒绝 → 终止提示 reload;1000 normal → 不重试;其他(网关重启/网络掉线/1006/proxy timeout)→ 重试。

---

## 7. DevContainer 与容器化

### 7.1 Dockerfile (466 行,多阶段构建)

**SQLite 定制编译** (`sqlite_build` stage): Debian 13 自带 3.46.1 仍有 WAL-reset bug → 编译 3.53.4 pinned + sha256 校验 + FTS5 trigram 自测试。

**s6-overlay 3.2.3.0**: PID 1 = `s6-svscan`,reap zombie + 监督 main-hermes / dashboard / per-profile gateways。多阶段校验和 + curl retry 防 GitHub CDN 抖动。

**Node 26 源**: bookworm-slim 镜像链接 glibc 2.36,兼容 Debian 13 runtime。

**Photon iMessage sidecar**: 独立 `node_modules`,baked 防运行时 EROFS。

**Python 依赖**: `uv sync --frozen --no-install-project --extra all --extra messaging --extra otlp --extra anthropic --extra bedrock --extra azure-identity --extra hindsight --extra matrix`。

**镜像溯源**: `HERMES_GIT_SHA` build-arg → `/opt/hermes/.hermes_build_sha` + `/etc/hermes/image-provenance.json`(schema 1, deployment_kind "image")。

**权限**: `--chmod=a+rX,go-w` 非 root hermes 用户(UID 10000)读+遍历,无写;`HERMES_UID` 运行时覆盖。

**Lazy install 重定向**: `HERMES_LAZY_INSTALL_TARGET=/opt/data/lazy-packages`,sealed venv 保证 + 可选后端 SDK 仍可安装。

**docker exec 特权 drop shim**: `/opt/hermes/bin/hermes` 检测 root → `s6-setuidgid hermes` 重执行 venv binary。

### 7.2 docker-compose.yml

`gateway` + `dashboard` 两服务,`network_mode: host`,`~/.hermes:/opt/data`。Dashboard 默认 127.0.0.1,远程访问走 SSH tunnel 或反向代理。

### 7.3 s6 服务定义

`docker/s6-rc.d/main-hermes/run`: 当前是 `exec sleep infinity`(s6-rc 需要至少一个 user service)。

`docker/s6-rc.d/dashboard/run`: `HERMES_DASHBOARD` truthy 才启动,否则 exit 0 + finish 125 = permanent failure slot down。非 loopback bind 强制 auth gate(不再接受 `HERMES_DASHBOARD_INSECURE`)。

### 7.4 cont-init.d

- `01-hermes-setup` → `stage2-hook.sh`(UID/GID remap, volume chown, config seeding, skills sync)
- `015-supervise-perms`
- `02-reconcile-profiles` — 容器重启后从 `$HERMES_HOME/profiles/<name>/` 重建 per-profile gateway s6 slots

### 7.5 入口调度

`entrypoint-dispatch.sh`: 真 PID 1 路径 exec `/init`(s6 监督树);平台自有 PID-1 init 时 fallback 到 `stage2-hook.sh` + `main-wrapper.sh`(#38349)。

---

## 8. CRDT 与多端冲突

### 8.1 Skill Sync — 类 Git 内容寻址同步 (`tools/skills_sync_client.py`, 568 行 + `skills_sync_client_wire.py`, 343 行)

**不是传统 CRDT(Yjs/automerge)**,而是 git 风格的对象模型 + CAS ref 更新。

**Wire 模型** (`skills_sync_client_wire.py`):
```python
WIRE_VERSION = "1"
KIND_BLOB, KIND_TREE, KIND_COMMIT = "blob", "tree", "commit"
MODE_FILE, MODE_EXEC, MODE_DIR = "file", "exec", "dir"

def wire_address(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()

def canonical_json_bytes(obj) -> bytes:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ...).encode()
```

**对象类型**: blob(文件内容) / tree(目录) / commit(tree + parents + author + ts + message)。

**Commit 结构**:
```python
{"type":"commit", "tree": tree_hash, "parents": [base, other],
 "author": {"owner": owner, "device": device},
 "ts": "2026-09-07T...", "message": "...", "artifact_type": "skill"}
```

**CAS ref** (`cas_ref`): `POST refs/:name` 带 `from` / `to`,409 → `SyncConflict(actual)` 触发三向合并。

**三向合并** (`merge_skill`):
```python
def merge_skill(base, ours, theirs) -> str:
    if ours == theirs: return "either" if ours else "none"
    if theirs == base: return "ours"
    if ours == base: return "theirs"
    return "overlap"  # 双方都改,需人工
```

**Sync manifest**: 根级 blob `sync-manifest` 记录 `{name: enabled}`,plane manifest 权威,本地 `.usage.json` 的 `sync` flag 仅是可编辑 intent(pull 时 reconciliation 来源)。

**Org scope**: 独立 `org/` 路由,personal routes hard-scoped to token owner,防 org 查询返回个人数据。

**Eligibility gate**: 非 bundled / 非 hub-installed / 非 external / 非 `_org/` 下;Nous admin JWT claim `tool_gateway_admin` 硬门槛。

### 8.2 会话事件排序与回放

`tui_gateway/event_replay.py` 的 per-session `seq` + ring buffer 是**单写者多读者**场景下的有序广播,客户端重连后 `events_since(last_seen)` 精确回放。

**不是 CRDT**: 事件流是线性的、服务端权威的,客户端不合并并发写,只补漏。

### 8.3 状态库并发控制

`hermes_state.py` 的 `turn_lease` + `CompressionSessionBusyError` + `SessionTurnLeaseLostError` 是**乐观并发控制**(lease-based fencing),不是 CRDT 的 converge 语义 — 后写者 fail fast 而非合并。

### 8.4 laew 差距总结

| 维度 | hermes 现状 | laew 差距 |
|------|------------|----------|
| CrashDump | faulthandler + JSON record + 退出护送 + 重启断路器 | 无 panic hook,无重启循环检测 |
| WebUI | 6 前端共享 AIAgent + 18k 行 Electron + 完整 preload 桥 | 仅 Rust TUI |
| OAuth | RFC 8252 PKCE + safeStorage + 多连接 registry | 仅 API Key |
| i18n | 16 语言 YAML + 21 语言 Web + 类型安全 + RTL | 中文硬编码 |
| Release | CalVer + semver + 应用内更新 + s6 容器 | 手动 cargo build |
| WebSocket | JSON-RPC + token 合并 + 事件回放 + 心跳 | 无 |
| DevContainer | 多阶段 Dockerfile + s6-overlay + 镜像溯源 | 无 |
| 多端同步 | 类 Git 内容寻址 + CAS + 三向合并 | 无 |

---

## 本轮关键发现(laew gap L79-L94)

**L79 无启动看门狗**: hermes 的 `hermes_startup_watchdog.py` 防启动期死锁,laew 无对应 → 启动卡死需人工发现。

**L80 无重启循环断路器**: hermes 的 `restart_loop_guard.py` 链式 inter-boot gap 检测,laew 无 → 崩溃-重启-崩溃无限循环。

**L81 无关闭取证**: hermes 的 `shutdown_forensics.py` 10ms 快照 + 异步 `ps`/`dmesg`,laew 无 → 外部 SIGTERM 原因不明。

**L82 无 Electron/桌面壳**: hermes 18,360 行 Electron main + 541 行 preload + HUD/Pet/Quick Entry,laew 仅 TUI。

**L83 无 RFC 8252 OAuth**: hermes 完整 PKCE + safeStorage + 多连接,laew 仅 API Key 明文。

**L84 无 i18n 框架**: hermes 16 语言 CLI + 21 语言 Web + 类型安全 + RTL,laew 中文硬编码。

**L85 无 CalVer 发布工程**: hermes `scripts/release.py` 2648 行 + 应用内更新 + Rust/Tauri 编排,laew 手动 cargo build。

**L86 无 WS 事件回放**: hermes per-session seq + 512 ring + epoch 检测 + 客户端 replayHold,laew 无 WS。

**L87 无容器化**: hermes 多阶段 Dockerfile + s6-overlay + 镜像溯源 + UID remap + lazy install 重定向,laew 无。

**L88 无多端同步**: hermes 类 Git 内容寻址 + CAS + 三向合并 + sync manifest,laew 无。

**L89 WAL-reset bug 防御**: hermes 检测脆弱 SQLite + 自动回退 DELETE + macOS F_FULLFSYNC,laew 无。

**L90 修复循环有界**: hermes `_MAX_PERSISTENT_REPAIR_ATTEMPTS=3` + 指纹账本 + 备份去重,laew 无状态修复。

**L91 Token 合并流式**: hermes WS 流式 token 30fps 批量 flush,laew 无流式。

**L92 镜像溯源**: hermes `image-provenance.json` + baked git SHA,laew 无。

**L93 Org scope 隔离**: hermes personal / org 路由 hard-scoped,laew 无多租户。

**L94 部分翻译回退**: hermes `defineLocale` 缺失键回退英文,laew 无。

**推荐 Rust crate**: `human-panic`(L79) / `backoff`(L80) / `tauri`(L82) / `oauth2`+`keyring`(L83) / `rust-i18n`(L84) / `cargo-dist`(L85) / `tokio-tungstenite`(L86) / `landlock`+`seccomp`(L87) / `yrs`(L88)。
