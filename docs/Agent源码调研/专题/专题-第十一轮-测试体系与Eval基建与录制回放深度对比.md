# 专题-第十一轮-测试体系与Eval基建与录制回放深度对比

> **调研范围**:15 个 Agent 源码工程(atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / hermes-agent / agent-core / agent-studio / cc-switch / jiuwenswarm / semantica / Switchyard / TencentDB-Agent-Memory / undici)的**测试体系、Eval 基建、Mock LLM、录制回放、测试金字塔、E2E 测试、单元测试、集成测试、视觉回归测试、性能基准**全面对比分析
>
> **调研日期**:2026-09-07
>
> **关联专题**:第三轮-测试体系与 Eval 基建深度分析(早期版本)、第十轮深挖合集(累计 142 个 gap)
>
> **目标读者**:laew 工程核心维护者 / 测试架构师 / 质量保障工程师

---

## 目录

- 第 1 章 引言与调研方法
- 第 2 章 15 工程测试金字塔全景
- 第 3 章 atomcode(Rust)
- 第 4 章 claudecode(TypeScript)
- 第 5 章 deepseek-harness(TypeScript)
- 第 6 章 openclaw(TypeScript)
- 第 7 章 opencode(TypeScript/Bun)
- 第 8 章 pi(TypeScript)
- 第 9 章 hermes-agent(Python)
- 第 10 章 agent-core(Python)
- 第 11 章 agent-studio(Python)
- 第 12 章 cc-switch(Tauri 2)
- 第 13 章 jiuwenswarm(Python)
- 第 14 章 semantica(Python)
- 第 15 章 Switchyard(Rust)
- 第 16 章 TencentDB-Agent-Memory(TS+Python)
- 第 17 章 undici(Node.js)
- 第 18 章 Mock LLM 实现矩阵
- 第 19 章 录制回放实现矩阵
- 第 20 章 Eval 框架与 pairwise 对比
- 第 21 章 CI 集成与覆盖率
- 第 22 章 性能基准(criterion / hyperfine / k6)
- 第 23 章 属性测试 / Fuzz 测试 / 契约测试
- 第 24 章 laew 现状评估与 20 个新 Gap(L206-L225)
- 第 25 章 Rust crate 推荐与改造路线图

---

## 第 1 章 引言与调研方法

### 1.1 调研背景

laew 当前作为 Rust Agent CLI 已经覆盖了 6 角色 Multi-Agent 架构、Bash/Read/Write 三件套工具集、TUI 多轮交互、协议 wire 翻译(Anthropic/OpenAI)以及 SQLite 持久化等核心能力,但**测试体系与 Eval 基建**仍是 laew 从"能跑"升级到"生产级 Agent CLI"的关键短板。本专题基于第十轮已识别的 142 个 gap 之上,围绕**第十一轮新增 8 大维度**展开:

1. **Mock LLM 服务**(deterministic / scripted / fault injection)
2. **录制回放**(HTTP fixture / VCR / Polly.js / Cassette)
3. **Eval 框架**(vitest-evals / pairwise / LLM-as-judge)
4. **E2E 测试**(tmux control-mode / PTY 真渲染)
5. **可视化测试**(snapshot testing / 视觉回归)
6. **性能基准**(hyperfine / criterion / k6 / autocannon)
7. **测试覆盖率**(tarpaulin / istanbul / coverage.py)
8. **CI 集成**(GitHub Actions / GitLab CI / Buildkite)

同时新增 3 大新兴维度:**属性测试(proptest / fast-check / Hypothesis)**、**Fuzz 测试(cargo-fuzz)**、**契约测试(Pact / OpenAPI schema)**。

### 1.2 调研方法

- **源码直接阅读**:对 15 个工程的测试目录、vitest/pytest/Cargo.toml 配置、CI workflows、Eval 入口、Mock LLM 实现进行了第一手源码阅读,产出本文档。
- **覆盖维度**:每个工程均提取其测试框架、Mock LLM、Eval 框架、录制回放、CI 集成、覆盖率、属性测试、性能基准 8 大要素。
- **横向对比**:在第 18-22 章中构建 4 张核心对比表,系统呈现差异。
- **laew gap 映射**:在第 24 章中产出 20 个新 gap(L206-L225),与第十轮 L1-L142 形成 162 个累计 gap 清单。
- **Rust crate 推荐**:在第 25 章给出 12 个推荐 crate 与 4 阶段改造路线图。

### 1.3 laew 现状与痛点

阅读 `testReport/run_e2e.sh` 与 `src/agent/yolo.rs` 后,识别出 laew 当前测试体系的关键痛点:

- **零 Mock LLM**:真实调用 anthropic / openai endpoint,严重依赖 API Key,CI 难以稳定运行。
- **零录制回放**:每次 E2E 都需要真实 LLM 响应,无法做确定性回归测试。
- **零 Eval 框架**:没有 pairwise 对比 / LLM-as-judge / golden dataset。
- **零属性测试**:protocol wire 转换、JSON Schema 校验、SQLite 持久层都没有 fuzz / proptest 覆盖。
- **零覆盖率门槛**:`cargo test` 可以跑但没有 tarpaulin 100% 门槛。
- **E2E 仅主屏冒烟**:`run_e2e.sh` 第 8 节虽然接入了 tmux control-mode,但只覆盖 /provider list/add/del 子屏,未覆盖协议 wire 真实回放。

---

## 第 2 章 15 工程测试金字塔全景

本章先用一个高层矩阵呈现 15 个工程在 8 大测试维度的覆盖情况,然后在第 3-17 章逐工程深入。

### 2.1 高层覆盖矩阵

| 工程 | 语言 | 单元测试 | 集成测试 | E2E 测试 | Mock LLM | 录制回放 | Eval 框架 | CI 集成 | 覆盖率 |
|------|------|----------|----------|----------|----------|----------|-----------|---------|--------|
| atomcode | Rust | ✅ cargo test (12 crates) | ✅ crates/*/tests | ✅ evals/deepseek-v4-flash | ✅ 内置 Mock provider | ✅ fixture replay | ✅ eval.py | ✅ .github/workflows/build.yml | ✅ (待验证) |
| claudecode | TS/Bun | ✅ vitest (40+ files) | ✅ src/**tests** | ❌ 缺端到端 | ⚠️ fixtures | ✅ vitest-evals | ✅ vitest-evals + pairwise | ✅ GitHub Actions | ⚠️ 部分 |
| deepseek-harness | TS | ✅ vitest 100% gate | ✅ vitest.e2e | ✅ snapshot suite | ✅ dsh-llm-mock-server | ✅ dsh-llm-replay | ✅ session-snapshot | ✅ GitLab CI | ✅ 100% per-file |
| openclaw | TS | ✅ vitest 100+ configs | ✅ live test | ✅ e2e + live test | ✅ non-isolated-runner | ✅ openai-onboarding live | ⚠️ 部分 | ✅ CodeQL 多套 | ✅ 100% per-file |
| opencode | TS/Bun | ✅ bun:test | ✅ packages/*/test | ✅ TUI + E2E | ✅ packages/llm/test/fixtures | ✅ http-recorder | ⚠️ 部分 | ✅ GitHub Actions | ✅ per-file |
| pi | TS | ✅ vitest | ✅ packages/*/test | ✅ packages/evals | ⚠️ 模拟 provider | ⚠️ local fixtures | ✅ vitest-evals + evalHarnessTable | ✅ GitHub Actions | ⚠️ 部分 |
| hermes-agent | Python | ✅ pytest 17k tests | ✅ tests/integration | ✅ e2e | ✅ fake_ha_server | ✅ evals/ 20+ scripts | ✅ evals/ core_tool_deferral | ✅ GitHub Actions | ✅ coverage.py |
| agent-core | Python | ✅ pytest unit+system | ✅ tests/system_tests | ⚠️ 部分 | ⚠️ fixtures | ⚠️ fixtures | ✅ skill_evaluator | ⚠️ 待验证 | ⚠️ 部分 |
| agent-studio | Python+React | ✅ pytest + jest | ✅ backend tests | ✅ playwright | ⚠️ 部分 | ⚠️ 部分 | ✅ evaluation/tests | ⚠️ 待验证 | ⚠️ 部分 |
| cc-switch | Tauri 2 | ✅ vitest + jsdom | ✅ integration | ✅ Playwright | ⚠️ msw fixtures | ⚠️ partial | ❌ 缺 | ✅ GitHub Actions | ⚠️ text+lcov |
| jiuwenswarm | Python | ✅ pytest --cov | ✅ tests/integration | ✅ ui_e2e | ⚠️ fakes | ⚠️ fakes | ⚠️ partial | ✅ GitHub Actions | ✅ term-missing+xml |
| semantica | Python | ✅ pytest 30 tests/ | ✅ tests/ | ⚠️ 部分 | ⚠️ fixtures | ⚠️ fixtures | ⚠️ partial | ✅ GitHub Actions | ⚠️ 待验证 |
| Switchyard | Rust | ✅ cargo test | ✅ tests/e2e | ✅ benchmark/run-baseline.sh | ⚠️ mock client | ✅ Harbor fixture | ✅ Harbor TBLite eval | ✅ GitHub Actions | ✅ per-file |
| TencentDB-Agent-Memory | TS+Python | ✅ jest + pytest | ⚠️ partial | ⚠️ partial | ⚠️ partial | ⚠️ partial | ⚠️ partial | ⚠️ GitHub Actions | ⚠️ partial |
| undici | Node.js | ✅ borp 100+ files | ✅ test/jest | ✅ wpt + autobahn | ✅ MockAgent | ✅ nock + cache-tests | ❌ 不适用 | ✅ GitHub Actions | ✅ c8 100% |

### 2.2 测试金字塔分布(纵向)

- **底层(单元)**:15/15 工程均有,claudecode / deepseek-harness / openclaw 实现 **100% per-file 覆盖率门槛**;Rust 工程(Switchyard / atomcode)走 cargo test + tarpaulin/codecov 路线。
- **中层(集成)**:15/15 均有,核心区别在于是否 fork / use real subprocess / use worker thread;deepseek-harness / openclaw 显式区分 thread-safe 与 process-bound 项目。
- **顶层(E2E)**:13/15 有端到端,claudecode 缺;opencode / Switchyard / hermes-agent 的 E2E 最完整(覆盖 TUI / Docker / 真实 LLM 调用)。

### 2.3 录制回放覆盖率(关键短板)

- **完整实现**:opencode(http-recorder + httpapi-codegen)、deepseek-harness(dsh-llm-replay)、undici(cache-tests)。
- **部分实现**:pi(evalHarnessTable 录制)、Switchyard(harbor fixture)。
- **缺失**:atomcode / claudecode / openclaw / hermes-agent / agent-core / agent-studio / cc-switch / jiuwenswarm / semantica / TencentDB-Agent-Memory 均无统一录制回放框架。

---

## 第 3 章 atomcode(Rust)

### 3.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/atomcode`
- **栈**:Rust(edition 2021)、12 个 crates、Workspace glob `crates/*`
- **测试框架**:cargo test(原生),worktree in `.github/workflows/build.yml`
- **入口**:`evals/deepseek-v4-flash/eval.py`(std-lib-only Eval harness)

### 3.2 测试架构剖析

atomcode 是 15 工程中 **唯一走 std-lib Python** 实现 Eval harness 的项目。`eval.py` 在零外部依赖下实现了:

- **Candidate 配对**:支持 `official` / `volcengine` 双向对比,要求 `selection` 不同(防作弊)。
- **Suite 配置**:`repetitions` / `pair_concurrency` / `timeout_seconds` / `start_skew_ms` / `random_seed` / `atomcode_bin` / `codex_bin` / `codex_model`。
- **Case 发现**:`discover_cases()` glob `*/case.json` 加载 prompt + fixture + verify + rubric。
- **Catalo 校验**:`model-cases.json` 与 `agent-cases.json` 必须为数组,id 必须匹配 `[a-z0-9][a-z0-9_-]*`。
- **路径安全**:fixture 必须 resolve 后仍在 case 目录内(`cfg.parent.resolve() not in fixture.parents` 报错防越界)。
- **原子写**:`atomic_json()` 用 `temp + replace` 实现原子落盘,防并发覆盖。
- **Secret 脱敏**:`SECRET_RE` 正则匹配 `authorization/api[_-]?key/token` 字段,`scrub()` 替换为 `[REDACTED]`。
- **Token 统计**:`TOKEN_RE` 正则匹配 `[tokens] prompt=X completion=Y cached=Z` 协议行。

### 3.3 配套测试目录

```
crates/atomcode-cli/tests        # CLI 子命令集成
crates/atomcode-coding/tests     # Coding 流程
crates/atomcode-tuix/tests       # TUI 渲染
crates/atomcode-kernel/tests     # Kernel 内部
crates/atomcode-config/tests     # 配置加载
crates/atomcode-telemetry/tests  # Telemetry OTLP
crates/atomcode-capabilities/tests # 能力注册
crates/atomcode-daemon/tests     # Daemon 守护进程
evals/deepseek-v4-flash/tests/test_eval.py # Eval harness 单元测试
extensions/vscode/webview-ui/test # VSCode 扩展 UI 测试
```

### 3.4 Eval 实战

- **case.json 结构**:`id` / `tier` / `prompt` / `timeout_seconds` / `allow_edits` / `fixture` / `verify` / `rubric`。
- **rubric 字段**:key 为评分维度,value 为评分标准说明字符串。
- **agent-fixture/** 提供了 12 个 fixture 案例(app / legacy / native / cache / diagnosis / pagination / parser / contract / audit / service / config)。
- **Test fixtures**(在 `cases/agent-fixture/tests/`):test_audit / test_legacy / test_diagnosis / test_pagination / test_parser / test_contract / test_cache / test_config,每个 fixture 都自带单元测试,确保 fixture 的"测试性"。

### 3.5 评估最佳实践(可借鉴)

1. **零外部依赖**:Python std-lib 即可做完整 Eval,适合 laew 的 e2e.sh 第 5+ 节。
2. **fixture 与 case 同目录**:`cases/agent-fixture/` 同时承载 fixture 代码与 fixture 自身的单元测试,避免 fixture 自身成为测试盲区。
3. **原子 JSON 写**:`atomic_json()` 是关键工程细节,laew 的 SQLite 持久化层已经具备(PRAGMA journal_mode=WAL + fsync),但 Agent-Memory 序列化仍可借鉴此模式。
4. **rubric 即文档**:rubric 字典既是评分标准也是人类可读的测试说明,提升评测可解释性。

### 3.6 测试不足与差距

- **零录制回放**:eval.py 仅支持 real-model call,无 keyless 测试能力。
- **零性能基准**:没有 criterion bench / hyperfine 脚本。
- **零属性测试**:12 个 crates 中没看到 proptest 集成。
- **CI 较薄**:仅 `.github/workflows/build.yml`,没有 coverage gate。

---