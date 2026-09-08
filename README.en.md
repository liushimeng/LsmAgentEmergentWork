<!-- markdownlint-disable -->
<div align="center">

[🇨🇳 中文](README.md) · [🇬🇧 English](README.en.md) · [🇯🇵 日本語](README.ja.md)

---

```
╔══════════════════════════════════════════════════════════════════════╗
║                                                                    ║
║        ██╗      █████╗ ███████╗██╗    ██╗                          ║
║        ██║     ██╔══██╗██╔════╝██║    ██║                          ║
║        ██║     ███████║█████╗  ██║ █╗ ██║                          ║
║        ██║     ██╔══██║██╔══╝  ██║███╗██║                          ║
║        ███████╗██║  ██║███████╗╚███╔███╔╝                          ║
║        ╚══════╝╚═╝  ╚═╝╚══════╝ ╚══╝╚══╝                           ║
║                                                                    ║
║        LLM · Agent · CLI · Rust · Multi-Agent · 6 Roles           ║
║        Dual-Protocol · 6 Tools · 6-Role Orchestration · TUI       ║
║                                                                    ║
╚══════════════════════════════════════════════════════════════════════╝
```

## 🦀 Rust Multi-Agent CLI · Dual Protocol · 6-Role Orchestration

</div>
<!-- markdownlint-restore -->

<div align="center">

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork)
[![AI Agent Coded](https://img.shields.io/badge/AI--Agent-100%25-ff6b6b)](CLAUDE.md)
[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![Chinese](https://img.shields.io/badge/lang-中文-red)](README.md) [![English](https://img.shields.io/badge/lang-English-blue)](README.en.md) [![日本語](https://img.shields.io/badge/lang-日本語-green)](README.ja.md)

</div>

---

> **🤖 100% AI Agent Auto-Coded** — No human hand-wrote a single line of code. No "traditional" programming.
> The entire project (Rust source, multi-agent architecture, TUI rendering engine, tool system,
> automated tests, CI scripts, documentation) was autonomously written, compiled, tested,
> refactored, and deployed by AI Agents (Claude Code, etc.).
>
> **⏱️ 14 Rounds of Deep Research · 82+ Dimensions · 635 Gaps Knowledge Base** — This repo is not a
> "one-shot session product", but a complete demonstration of "Agent programming" capability:
> 14 consecutive rounds of deep research across 82+ dimensions, accumulating 635 laew gaps.
> Each round the Agent auto-reads the previous output, plans new dimensions, produces
>专题 reports, writes to the knowledge base, and git-commits.

---

## 🤖 100% Agent Auto-Coding — The Core Highlight

> **This is not code written by one person. This is the work of a team of AI Agents programming 24/7.**

All code in this repository is auto-written by AI Agents:

- **No hand-coding**: No human programmer wrote a single line of Rust / TOML / Shell / Markdown.
- **Agent collaboration**: Split into multiple SubAgent responsibility lines (architecture, TUI, tools, protocols, testing, docs), each working independently and auto-committing.
- **Self-testing & self-fixing**: Agents auto-run `cargo test` and `bash testReport/run_e2e.sh`, then locate, fix, and regression-test bugs.
- **Self-deployment**: `rebuild_restart_app.sh` is Agent-written — one command for `cargo build --release` → copy `./laew` → restart.
- **Continuous iteration**: `CLAUDE.md` records massive Agent lessons, each auto-written after an Agent stumbles — future Agents auto-load them to avoid repeating mistakes.
- **Knowledge base**: 80+ research documents / ~160k lines under `docs/`, all Agent-produced.

> **Repo stats**: Rust multi-agent CLI, 6 tools, 6-role orchestration, dual protocols, TUI sub-screen automation.
> **None of this contains a single hand-written character by a human.**

---

## 🎯 Project Positioning & Core Capabilities

`laew` (**L**lm **A**gent **E**mergent **W**ork) is a Rust-based LLM multi-agent CLI,
supporting **Anthropic** and **OpenAI** dual protocols, with 6 built-in tool calls,
offering TUI multi-turn conversation, `-p` single-turn task, and `-f` file-prompt modes.

| Capability | Description |
|------|------|
| 🧠 **Dual Protocol** | Anthropic (anthropic-messages) + OpenAI (openai-completions), unified message model isolates protocol differences |
| 🛠️ **6 Tools** | Bash / Read / Write / Edit / Glob / Grep |
| 🤖 **6-Role Multi-Agent** | Yolo / Plan / Main-Work / SubAgent-Work / Quality-Check / SessionContext |
| 📊 **Three-Tier Classification** | simple / medium / hard, Yolo auto-classifies then layered orchestration |
| 🖥️ **TUI** | crossterm independent rendering engine, alternate screen + raw mode + Screen stack + Tab forms |
| 💾 **SQLite Persistence** | Root-dir `LsmAgentEmergentWork.db`, no config files |
| 🔍 **Project Context Injection** | Five-level chain CLAUDE.md → AGENTS.md → README.md → auto-generate → empty, idempotent injection per session |

---

## 🤖 Multi-Agent Architecture — 6 Roles on One Stage

Orchestrated by `MultiAgentOrchestrator`: user input → project context injection →
Yolo three-tier classification → simple (SubAgent) / medium (Main → SubAgent) /
hard (Plan → Main → SubAgent) → Quality-Check → SessionContext wrap-up.

| Role | Responsibility | Tools |
|------|------|------|
| 🎯 **Yolo Agent** | Entry layer: purpose→goal→intent three-step analysis / three-tier classification / failure reflux | Read |
| 🗺️ **Plan Agent** | Planning layer: hard tasks output Markdown plans to `plans/` | Read / Write |
| 🔄 **Main-Work Agent** | Flow layer: split WorkFlow list | Bash / Read |
| ⚡ **SubAgent-Work Agent** | Execution layer smallest unit: one SubAgent dispatched per flow | Bash / Read / Write |
| ✅ **Quality-Check Agent** | QC layer: mandatory QC after each execution unit | Optional Read |
| 🧠 **SessionContext Agent** | Session layer: summarize and write `session_memory` after task completion | No tools |

### Orchestration Topology

```text
User Input
   │
   ▼
Project Context Injection (five-level chain, idempotent)
   │
   ▼
Yolo ──→ Classify: simple / medium / hard
   │
   ├─ simple ──→ SubAgent-Work ──→ Quality-Check ──→ SessionContext
   │
   ├─ medium  ──→ Main-Work ──→ SubAgent-Work ──→ Quality-Check ──→ SessionContext
   │
   └─ hard    ──→ Plan ──→ Main-Work ──→ SubAgent-Work ──→ Quality-Check ──→ SessionContext
```

---

## ⚙️ Tech Stack

| Module | Choice |
|------|------|
| 🦀 Language | Rust 1.75+ |
| ⌨️ TUI | crossterm (independent CLI rendering engine: Screen trait + Frame + alternate screen + raw mode) |
| 📟 CLI | clap (derive style: TUI / `-p` / `-f` / `provider` subcommands) |
| 💾 Database | rusqlite (SQLite, `LsmAgentEmergentWork.db`) |
| 🌐 HTTP | reqwest (Anthropic / OpenAI dual-protocol clients) |
| 📡 Protocol | Anthropic Messages + OpenAI Chat Completions (unified message model `llm/mod.rs`) |
| 🧪 Testing | `cargo test` unit + `testReport/run_e2e.sh` e2e (mock LLM + tmux sub-screen automation) |

---

## 🚀 Quick Start

### Prerequisites

- Rust 1.75+ (recommended via [rustup](https://rustup.rs))
- Linux / macOS (TUI relies on crossterm raw mode)

### Install & Deploy

```bash
# 1. Clone
git clone https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork.git
cd LsmAgentEmergentWork

# 2. One-command build (cargo build --release → copy ./laew to root)
./rebuild_restart_app.sh

# 3. Check version
./laew --version
# laew 0.1.0 (build 2026-09-08 xx:xx:xx CST, git xxxxxxx)
```

### Configure LLM Provider

```bash
# Add an Anthropic provider (5-tuple: protocol + provider_name + model_name + end_point + api_key)
./laew provider add --protocol anthropic \
    --provider-name myAnthropic --model-name claude-sonnet-5 \
    --end-point https://api.anthropic.com --api-key sk-ant-xxxx

# Add an OpenAI provider
./laew provider add --protocol openai \
    --provider-name myOpenAI --model-name gpt-5 \
    --end-point https://api.openai.com --api-key sk-xxxx

./laew provider list          # list (* = active)
./laew provider use 2         # switch active model
./laew provider delete 2      # delete record
```

> Endpoint auto-completion: Anthropic appends `v1/messages`, OpenAI appends `chat/completions`.
> No config files — all stored in **root-dir** SQLite (`LsmAgentEmergentWork.db`).

### Three Usage Modes

```bash
./laew                                  # Enter TUI multi-turn conversation
./laew -p "list files in current dir"   # Single-turn task mode
./laew -f /path/to/prompt.md            # Read prompt from file (absolute/relative path)
```

### TUI Slash Commands

| Command | Action |
|------|------|
| `/help` (h, ?) | Show help |
| `/exit` (quit, q) | Exit TUI |
| `/clear` (c) / `/new` (n) | Clear history, start new Session |
| `/model` | Show current model |
| `/provider` | Manage providers (defaults to list screen) |

---

## 🔑 Key Concepts

| Concept | Description |
|------|------|
| **Root Dir** | Directory where `laew` binary lives; database and build artifacts land here |
| **Work Dir** | Directory where `laew` was launched; default context for Agent file/command operations |
| **Provider Record** | 5-tuple: protocol + provider_name + model_name + end_point + api_key; multiple allowed, one active |
| **Endpoint Completion** | Anthropic → `{end_point}/v1/messages`; OpenAI → `{end_point}/chat/completions`; trailing `/` auto-trimmed |
| **Protocol Differences** | Anthropic uses `tools[].{name,description,input_schema}`; OpenAI uses `tools[].{type:"function",function:{name,description,parameters}}` |
| **Agent-Context** | Each Agent's independent real-time context (message flow + state), in-memory, lifecycle = current unit |
| **Agent-Memory** | Each Agent's independent memory layer, persisted to SQLite `agent_memory`, reused across units/sessions |
| **SessionContext Summary** | Markdown summary written to `session_memory` after each task; Yolo auto-injects recent N on next run |

---

## 📁 Project Structure

```
LsmAgentEmergentWork/
├── src/
│   ├── main.rs              # CLI entry (clap): TUI / -p / -f / provider subcommands
│   ├── lib.rs               # Library exports
│   ├── session.rs           # Session: device fingerprint + Session ID + independent context
│   ├── error.rs             # Unified error type (thiserror)
│   ├── build.rs             # Injects LAEW_BUILD_TIME / LAEW_GIT_HASH
│   ├── config/              # Root/work dir parsing + SQLite CRUD
│   ├── database/            # schema / models / paths / provider
│   ├── agent/
│   │   ├── mod.rs           # Protocol-agnostic loop: run_session → complete → tool_calls
│   │   ├── orchestrator.rs  # MultiAgentOrchestrator top-level orchestration
│   │   ├── yolo.rs          # YoloRunner dual-agent orchestrator + three-tier classification
│   │   ├── plan.rs          # Plan Agent
│   │   ├── main_work.rs     # Main-Work Agent
│   │   ├── subagent.rs      # SubAgent-Work Agent
│   │   ├── quality.rs       # Quality-Check Agent
│   │   ├── session_context.rs # SessionContext Agent
│   │   ├── profile.rs       # AgentProfile + work_profile() / yolo_profile()
│   │   ├── context.rs       # Agent-Context independent real-time context
│   │   ├── memory.rs        # Agent-Memory persistent memory
│   │   ├── json_repair.rs   # JSON auto-repair chain
│   │   ├── project_context.rs # Project doc five-level chain discovery + per-session injection
│   │   ├── system_prompt/   # SystemPrompt composition & rendering
│   │   ├── permissions/     # Permission control: dangerous / sensitive
│   │   ├── sandbox_hook/    # Sandbox hooks
│   │   └── tools/           # Tool trait + ToolRegistry + 6 tools
│   ├── llm/
│   │   ├── mod.rs           # Unified message model + LlmClient trait
│   │   ├── anthropic.rs     # Anthropic wire conversion
│   │   ├── openai.rs        # OpenAI wire conversion
│   │   ├── sse.rs           # SSE stream parsing
│   │   └── resilient.rs     # LLM call auto-resilience layer
│   └── tui/
│       ├── mod.rs           # REPL main loop + Screen stack
│       ├── engine.rs        # CLI rendering engine: Screen trait + Frame
│       ├── form.rs          # Generic Tab form state machine
│       ├── input.rs         # Single-line input + inline hint + completion
│       ├── completion.rs    # Slash command completion engine
│       ├── theme.rs         # ANSI colors / mask_key desensitization
│       └── screen/          # ProviderList / ProviderForm / ProviderDel sub-screens
├── docs/                    # Knowledge base: 14 rounds of deep research / 82+ dimensions / 635 gaps
├── testReport/              # Automated test reports + run_e2e.sh
├── tmpPlan/                 # Temporary plans during coding (not committed)
├── scripts/                 # Helper scripts (mock_llm_server.py)
├── rebuild_restart_app.sh   # One-command rebuild
├── CLAUDE.md                # Agent entry docs + massive lessons
└── AGENTS.md                # Engineering entry docs (= CLAUDE.md)
```

Full design docs under `docs/`.

---

## 🧪 Automated Testing — Agent Self-Inspection

Two testing layers:

| Layer | Entry | Purpose |
|-------|------|------|
| Unit tests | `cargo test` | Rust function-level coverage (modules, parsing, conversion, tools) |
| E2E (CLI) | `bash testReport/run_e2e.sh` | mock LLM, runs `-p` / `provider` / protocol wire / project context injection / TUI sub-screen tmux automation |

### TUI Sub-Screen Automation (tmux control-mode)

```bash
# Start background session with TUI, fixed 100x30
tmux new-session -d -s laew_e2e -x 100 -y 30 ./laew
# Send keys
tmux send-keys -t laew_e2e -l "/provider list"
tmux send-keys -t laew_e2e Enter
# Capture pane for assertion
tmux capture-pane -p -t laew_e2e | grep -F "/provider list"
# Cleanup
tmux kill-session -t laew_e2e
```

> See `docs/TUI自动化测试/` for details.

---

## 📚 14 Rounds of Deep Research — The Knowledge Base of Agent Programming

`docs/` holds **14 rounds of deep research**, covering **82+ dimensions**, accumulating
**635 laew gaps** (L1–L635), all Agent-produced:

| Round | Theme | Scale |
|------|------|------|
| Rounds 1–6 | Architecture / Multi-turn / Context / Tools / Memory / Workflow / Yolo / QC / MCP / Skill / Protocol wire / SubAgent / Goal / TUI / Hook | ~160k lines |
| Round 7 | File editing / Code retrieval / Git / Bash / Multimodal / PromptCaching / Schema / WebFetch | ~10k lines |
| Round 8 | Telemetry / Session / Tool permissions / LSP / Hook / Skill / Multi-tenant / TUI | ~13.5k lines |
| Round 9 | CrashDump / WebUI / OAuth / i18n / Release / WebSocket / Container / CRDT | ~9.4k lines |
| Round 10 | 15 main docs with new chapters | ~27k lines |
| Round 11 | Agent collaboration / Streaming / Error handling / Testing / Config / Plugin ecosystem / Protocol translation / System prompts | ~30k lines |
| Round 12 | HTTP client / Security defense / Model routing / Data migration / Performance / Logging / CLI / State persistence | ~17k lines |
| Round 13 | Local inference / KV cache / GUI automation / OS / Benchmarks / Paradigm comparison / DSL / WebAssembly | ~14.9k lines |
| Round 14 | 8 new dimensions + 200 new gaps | ~9.4k lines |

> Collection index: `docs/专题/专题-第十三轮深挖合集.md` etc.
> Implementation progress ledger: `docs/专题/专题-laew实现进度对照表.md`.

---

## 🤝 Follow & Support

If you find this project interesting, follow my accounts on various platforms to watch complete dev and demo videos:

| Platform | Search Account |
|------|---------|
| Kuaishou (快手) | **封刀灌海** |
| Douyin (抖音) | **封刀灌海** |
| Bilibili (B站) | **封刀灌海** |
| Xiaohongshu (小红书) | **封刀灌海** |
| WeChat Video (微信视频号) | **封刀灌海** |

---

## ☕ Donation

Server costs, LLM API calls, and knowledge base accumulation all have ongoing costs. If this project helps you or you find it interesting, donations are welcome:

| WeChat Pay | Alipay |
|:--------:|:----------:|
| ![WeChat QR](ProjectPic/微信二维码.jpg) | ![Alipay QR](ProjectPic/支付宝二维码.jpg) |

> `ProjectPic/` is committed to the repo, so QR codes display directly on GitHub / Gitee / GitCode.

**Contact**:

- 📱 Phone: `13520647302`
- 💬 WeChat: `liushimeng109117198`

---

## 📜 License

This project is open-sourced under **MIT License** — see [`LICENSE`](LICENSE).
All code is AI Agent auto-written, human-reviewed before commit.

---

## 🌟 Star / Watch / Fork

If this project gave you new insight into "Agent programming", please:

- ⭐ **Star** this repo — let more people see the power of Agent programming
- 👁️ **Watch** — follow upcoming iterations
- 🍴 **Fork** — rebuild a multi-agent CLI platform in your own environment

This repo is synced across two platforms:

| Platform | Link |
|------|------|
| Gitee | `https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork` |
| GitCode | `https://gitcode.com/liusm109117198/LsmAgentEmergentWork` |

> 💡 **If the 14-round deep research / 635 gaps / 100% Agent auto-coding approach inspires you**,
> feel free to share similar practices in your team via
> [Issues](https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork/issues).
> A single ⭐ does more than ten blog posts to push this forward.

**This is not code written by one person. This is the work of a team of AI Agents programming 24/7.**

---

**Version**: v0.1.0  |  **Last Updated**: 2026-09-08  |  **Build**: Agent auto-build
