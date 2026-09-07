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

## 第 4 章 claudecode(TypeScript)

### 4.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/claudecode`
- **栈**:TypeScript / Bun、40+ 工具、Ink Fork TUI、Bridge 远程控制
- **测试框架**:vitest(主)、vitest-evals(Eval)、fixtures(录制)
- **入口**:`src/tools/testing/TestingPermissionTool.tsx`(测试专用权限工具)

### 4.2 测试架构剖析

claudecode 的测试体系在 15 工程中属于**中等偏上**,核心亮点是 **vitest-evals** 与 **pairwise comparison**:

- **vitest-evals 集成**:`describeEval` / `createJudge` / `evalHarnessTable` 三件套,支持 deterministic judge 与 model-backed judge 双轨。
- **pairwise comparison**:`evalHarnessTable()` 支持 baseline / candidate / candidates 三档对比,`repetitions` 默认 6 次,`judgeThreshold: null` 保留低分观察。
- **TestingPermissionTool**:测试专用权限工具,在测试环境绕过真实权限校验,避免 E2E 被权限弹窗阻断。
- **fixtures 目录**:`src/**/fixtures/` 提供录制回放基础数据。

### 4.3 关键测试文件

```
src/tools/testing/TestingPermissionTool.tsx  # 测试权限工具
src/**tests**/*.test.ts                       # 单元 + 集成
src/**/fixtures/**                            # 录制 fixture
```

### 4.4 Eval 实战

- **describeEval**:每个 eval suite 绑定一个 harness,harness 可以是 `createPiCodingAgentHarness` 风格。
- **createJudge**:`createJudge<string, string>("TargetTaskJudge", ({ output }) => ({ score: output === "expected" ? 1 : 0 }))` 提供 deterministic 评分。
- **evalHarnessTable**:`describe.for(harnessTable)` 实现参数化对比,`repetition` 维度自动展开。
- **lift 计算**:candidate pass rate - baseline pass rate,以百分比点表示。

### 4.5 评估最佳实践(可借鉴)

1. **TestingPermissionTool**:测试专用权限工具是 laew 可以借鉴的模式——在测试环境提供"测试模式"绕过真实权限。
2. **pairwise + repetitions**:6 次重复 + lift 计算是 Agent 评测的最佳实践,laew 的 Yolo 分类器对比可直接复用。
3. **judgeThreshold:null**:保留低分观察而非直接 fail,适合 CI 中的"趋势监控"场景。

### 4.6 测试不足与差距

- **零 E2E**:claudecode 是 15 工程中**唯一缺少端到端测试**的项目,没有 tmux / Playwright / Docker 自动化。
- **零 Mock LLM**:没有内置 Mock provider,所有 Eval 都依赖真实 LLM。
- **零性能基准**:没有 bench:test / hyperfine 脚本。
- **覆盖率门槛不明确**:没有 100% per-file gate。

---

## 第 5 章 deepseek-harness(TypeScript)

### 5.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/deepseek-harness`
- **栈**:TypeScript / pnpm workspace、Cordis Everything-is-a-Plugin、53 个 packages
- **测试框架**:vitest(7 套配置)、100% per-file 覆盖率门槛
- **入口**:`packages/test-support/` 6 大测试支持包

### 5.2 测试架构剖析

deepseek-harness 是 15 工程中**测试体系最完整**的项目,核心亮点是 **6 大测试支持包** 与 **100% per-file 覆盖率门槛**:

#### 5.2.1 6 大测试支持包

| 包名 | 路径 | 核心能力 |
|------|------|----------|
| `dsh-llm-mock-server` | `packages/test-support/llm-mock-server` | 脚本化 OpenAI 兼容故障服务器 |
| `dsh-llm-replay` | `packages/test-support/llm-replay` | 无 Key 录制回放 |
| `dsh-agent-loop-testkit` | `packages/test-support/agent-loop-testkit` | AgentLoop 测试依赖挂载 |
| `dsh-session-snapshot` | `packages/test-support/session-snapshot` | Session 快照录制/回放/刷新 |
| `dsh-loader-smoke` | `packages/test-support/loader-smoke` | 加载器冒烟测试 |
| `dsh-client-runtime` | `packages/test-support/client-runtime` | 客户端运行时测试 |

#### 5.2.2 7 套 vitest 配置

| 配置文件 | 用途 | 关键特性 |
|----------|------|----------|
| `vitest.config.ts` | 主单元测试 | 100% per-file 覆盖率门槛 |
| `vitest.e2e.config.ts` | E2E 测试 | 真实 API 调用,120s 超时,retry 2 |
| `vitest.snapshot.config.ts` | 快照测试 | replay / record / refresh 三模式 |
| `vitest.expected.config.ts` | 预期输出测试 | 预期输出对比 |
| `vitest.web.config.ts` | Web 测试 | 前端测试 |
| `vitest.web.perf.config.ts` | Web 性能 | 前端性能基准 |
| `vitest.web-stress.config.ts` | Web 压力 | 前端压力测试 |

#### 5.2.3 100% per-file 覆盖率门槛

`vitest.config.ts` 中的覆盖率配置是 15 工程中最严格的:

```typescript
thresholds: {
  perFile: true,        // 每个文件独立门槛
  statements: 100,      // 语句覆盖 100%
  branches: 100,        // 分支覆盖 100%
  functions: 100,       // 函数覆盖 100%
  lines: 100,           // 行覆盖 100%
}
```

**豁免机制**(coverage-exempt.ts):明确列出豁免文件,每个 `v8 ignore comment` 必须携带原因。

**分区模式**(coverage-partitions.ts):`COVERAGE_PARTITION_MODE_ENV` 开启后豁免 per-file 门槛,用于 CI 分区。

### 5.3 dsh-llm-mock-server 深度剖析

这是 15 工程中最完整的 **Mock LLM 实现**,核心特性:

- **脚本化行为**:`--sequence partial_disconnect,success` 定义 FIFO 行为序列。
- **20+ 种故障模式**:

| 类别 | 行为 |
|------|------|
| 连接故障 | connection_reset / connection_refused |
| 流故障 | stream_disconnect / partial_disconnect / stall |
| 协议故障 | malformed_json / malformed_event / empty_body / stream_eof / partial_eof |
| 限流故障 | rate_limit(429) / server_error(500) / service_unavailable(503) |
| 认证故障 | auth_error / invalid_request / context_overflow / quota_exceeded |
| 成功模式 | success / slow_success / reasoning_success / tool_call_success / max_tokens |
| 内容故障 | wrong_content_type |
| 随机模式 | random(加权随机) |

- **随机模式**:`--seed 42 --random-weights 'success=60,slow_success=10,...'` 提供可复现的混合压力测试。
- **请求捕获**:每个请求和结果都记录在返回的 handle 上,供测试断言。
- **CLI + 库双入口**:`pnpm run mock:llm` 独立运行,`startMockLlmServer` 嵌入测试。

### 5.4 dsh-llm-replay 深度剖析

这是 15 工程中最完整的 **录制回放实现**,核心特性:

- **JSONL fixture**:fixture 是 session log 的投影,保留 header 和 event payload,省略 `seq/time` 信封。
- **派生脚本**:`deriveReplayScript` 解析 JSONL header,按 `(turn, step)` 键拆分 `assistant/chunk` 事件。
- **嵌套 Agent 绑定**:parent + child session 按 first-call order 绑定,每个 session 独立 cursor。
- **Override 侧车**:`replay.override.json` 支持 throw / hang / patch 三种覆盖模式。
- **占位符解析**:`{{fromRequest:<regex>}}` 在 stream 时解析,取最后匹配的第一个捕获组。
- **assertConsumed()**:teardown 时验证所有脚本都被消费,防止"静默少调用"。

### 5.5 评估最佳实践(可借鉴)

1. **100% per-file 覆盖率门槛**:laew 应借鉴此模式,对 `src/llm/` / `src/agent/` / `src/tui/` 核心模块实施 100% 门槛。
2. **dsh-llm-mock-server 故障注入**:laew 的协议 wire 测试可直接复用此模式,测试 Anthropic / OpenAI 客户端的容错能力。
3. **dsh-llm-replay 录制回放**:laew 的 E2E 测试可借鉴 JSONL fixture + override 侧车模式。
4. **7 套 vitest 配置**:按测试类型拆分配置是最佳实践,laew 可拆分为 unit / integration / e2e / snapshot 四套。

### 5.6 测试不足与差距

- **零属性测试**:53 个 packages 中没有 proptest / fast-check 集成。
- **零 Fuzz 测试**:没有 cargo-fuzz / AFL 集成。
- **CI 仅 GitLab**:`.gitlab-ci.yml` 仅覆盖 Python SDK 发布,TS 测试 CI 配置未公开。

---

## 第 6 章 openclaw(TypeScript)

### 6.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/openclaw`
- **栈**:TypeScript / pnpm workspace、162 extensions、Gateway/Harness/Adapter 三层契约
- **测试框架**:vitest(100+ 配置)、100% per-file 覆盖率门槛
- **入口**:`test/` 目录 2000+ 文件

### 6.2 测试架构剖析

openclaw 是 15 工程中**测试配置最复杂**的项目,核心亮点是 **100+ 套 vitest 配置** 与 **live test**:

#### 6.2.1 测试目录结构

```
test/
├── vitest/                     # 100+ 套 vitest 配置
│   ├── vitest.agents.config.ts
│   ├── vitest.agents-core.config.ts
│   ├── vitest.agents-embedded-agent.config.ts
│   ├── vitest.auto-reply.config.ts
│   ├── vitest.channels.config.ts
│   ├── vitest.cli.config.ts
│   ├── vitest.gateway.config.ts
│   ├── vitest.skills.config.ts
│   └── ... (100+ 套)
├── e2e/                        # E2E 测试
├── fixtures/                   # 测试 fixtures
├── helpers/                    # 测试 helpers
├── mocks/                      # Mock 实现
├── contracts/                  # 契约测试
├── plugins/                    # 插件测试
└── scripts/                    # 测试脚本
```

#### 6.2.2 测试类型分类

| 类型 | 配置前缀 | 用途 |
|------|----------|------|
| 单元测试 | `vitest.unit-*` | 纯函数单元测试 |
| 集成测试 | `vitest.agents-*` | Agent 集成测试 |
| E2E 测试 | `*.e2e.test.ts` | 端到端测试 |
| Live 测试 | `*.live.test.ts` | 真实 API 调用测试 |
| 性能测试 | `vitest.performance-*` | 性能基准 |
| 边界测试 | `vitest.boundary-*` | 边界条件测试 |

#### 6.2.3 non-isolated-runner

`test/non-isolated-runner.ts` 是 openclaw 的核心测试基础设施:

- **resolve-mocks**:`non-isolated-runner.resolve-mocks.test.ts` 验证 mock 解析逻辑。
- **test-api-fixtures**:`non-isolated-runner.test-api-fixtures.ts` 提供 API 测试 fixtures。
- **31KB 测试**:`non-isolated-runner.test.ts` 是最大的单文件测试之一。

### 6.3 关键测试文件

```
test/cli-json-stdout.e2e.test.ts          # CLI JSON stdout E2E
test/cli-json-stdout.sessions.e2e.test.ts # Session E2E
test/gateway-steer-fifo.e2e.test.ts       # Gateway 转向 FIFO
test/gateway-hook-concurrency.e2e.test.ts # Hook 并发
test/openclaw-launcher.e2e.test.ts        # 启动器 E2E
test/plugin-clawhub-release.test.ts       # 插件发布
test/vitest-scoped-config.test.ts         # 作用域配置
test/vitest-performance-config.test.ts    # 性能配置
test/release-check.test.ts                # 发布检查
test/package-scripts.test.ts              # 包脚本
```

### 6.4 评估最佳实践(可借鉴)

1. **100+ 套 vitest 配置**:按模块 / 类型 / 环境拆分配置是最佳实践,laew 可参考此模式。
2. **live test**:`*.live.test.ts` 提供真实 API 调用测试,laew 可借鉴此模式做"可选真实调用"测试。
3. **non-isolated-runner**:非隔离运行器是测试复杂 Agent 架构的关键基础设施。
4. **contracts 目录**:契约测试是 laew 可以借鉴的模式,验证 Gateway/Harness/Adapter 三层契约。

### 6.5 测试不足与差距

- **零 Mock LLM**:没有内置 Mock provider,live test 依赖真实 LLM。
- **零录制回放**:没有统一的录制回放框架。
- **零 Eval 框架**:没有 pairwise / LLM-as-judge 集成。
- **CI 复杂度高**:100+ 套配置导致 CI 维护成本高。

---

## 第 7 章 opencode(TypeScript/Bun)

### 7.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/opencode`
- **栈**:TypeScript / Bun、Effect + Schema 全栈 DI、34 个 packages
- **测试框架**:bun:test(主)、@effect/vitest(Effect 集成)、http-recorder(录制回放)
- **入口**:`packages/*/test/` 38 个测试目录

### 7.2 测试架构剖析

opencode 是 15 工程中**录制回放最完整**的项目,核心亮点是 **http-recorder** 与 **性能优化**:

#### 7.2.1 http-recorder 深度剖析

`packages/http-recorder` 是 opencode 的核心录制回放框架:

- **双 API**:`HttpRecorder.http(name, options?)` 与 `HttpRecorder.socket(name, options?)`。
- **Cassette 格式**:JSON 文件,request 顺序存储,WebSocket 保留 client/server frame 时序。
- **Redaction 机制**:

| 选项 | 用途 |
|------|------|
| `headers` | 敏感 header 名,替换为 `[REDACTED]` |
| `allowRequestHeaders` | 保留的 request header |
| `allowResponseHeaders` | 保留的 response header |
| `queryParameters` | 敏感 URL 查询参数 |
| `jsonFields` | 递归脱敏 JSON 键 |
| `url` | URL 稳定化函数 |
| `body` | body 稳定化函数 |

- **匹配规则**:严格顺序匹配,JSON object keys 规范化,支持自定义 `match` 函数。
- **CI 模式**:`CI=true` 时 missing cassette 直接 fail,防止"静默录制"。
- **刷新机制**:删除 cassette 后重新运行即刷新,无公开 overwrite 模式(避免不可见覆盖)。

#### 7.2.2 性能优化实战

`perf/test-suite.md` 记录了 opencode 的性能优化历程:

- **初始状态**:250s/run 全量测试。
- **优化后**:186s/run(prompt / processor / PTY 优化)。
- **关键优化**:

| 优化项 | Before | After | 收益 |
|--------|--------|-------|------|
| Plugin install concurrency | 7.800s | 6.204s | -20% |
| httpapi-listen PTY git | 10.554s | 7.818s | -26% |
| workspace.waitForSync timeout | 12.949s | 8.305s | -36% |
| config.test sleep | 10.270s | 9.480s | -8% |
| SDK parity git repos | 8.011s | 5.180s | -35% |
| HTTP provider dependency-ready | 7.905s | 2.980s | -62% |
| TUI plugin lifecycle timeout | 7.330s | 1.507s | -79% |
| Skill tool git | 2.320s | 1.425s | -39% |
| Prompt shell git | 26.930s | 23.400s | -13% |
| Session processor git | 12.500s | 9.206s | -26% |
| Config load it.instance | 14.180s | 3.930s | -72% |
| CLI run subprocess concurrent | 11.870s | 4.130s | -65% |

- **核心模式**:`it.instance` Effect-aware fixture 替代手动 `tmpdir + withTestInstance`,减少 60-70% 测试时间。

### 7.3 关键测试文件

```
packages/llm/test/llm.test.ts                  # LLM 核心测试
packages/llm/test/executor.test.ts             # 执行器测试
packages/llm/test/recorded-test.ts             # 录制回放测试
packages/llm/test/recorded-scenarios.ts        # 录制场景
packages/llm/test/tool-runtime.test.ts        # 工具运行时
packages/core/test/agent.test.ts               # Agent 测试
packages/core/test/integration.test.ts         # 集成测试
packages/core/test/event.test.ts               # 事件测试
packages/core/test/database-migration.test.ts  # 数据库迁移
packages/opencode/test/config/config.test.ts   # 配置测试
packages/opencode/test/server/httpapi-listen.test.ts # HTTP API 监听
packages/http-recorder/test/record-replay.test.ts    # 录制回放
```

### 7.4 评估最佳实践(可借鉴)

1. **http-recorder**:laew 的协议 wire 测试可借鉴此模式,录制真实 Anthropic / OpenAI 响应后回放。
2. **it.instance Effect-aware fixture**:laew 可借鉴此模式,用 Rust 的 `#[test]` + `tempfile` crate 实现类似效果。
3. **性能优化方法论**:opencode 的"假设-验证-决策"性能优化循环是最佳实践,laew 可建立类似机制。
4. **CI 模式**:`CI=true` 时 missing cassette fail 是关键设计,防止"静默录制"污染测试。

### 7.5 测试不足与差距

- **零 Mock LLM**:http-recorder 是录制回放,不是 Mock LLM(无法注入故障)。
- **零 Eval 框架**:没有 pairwise / LLM-as-judge 集成。
- **零属性测试**:34 个 packages 中没有 proptest / fast-check 集成。
- **性能基准不完整**:`perf/test-suite.md` 仅记录优化历程,没有持续性能基准。

---

## 第 8 章 pi(TypeScript)

### 8.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/pi`
- **栈**:TypeScript、Lane 并发、一等公民 Skill、CBOR 二进制帧协议
- **测试框架**:vitest(主)、vitest-evals(Eval)
- **入口**:`packages/evals/` Eval 框架

### 8.2 测试架构剖析

pi 是 15 工程中**Eval 框架最完整**的项目,核心亮点是 **vitest-evals + evalHarnessTable**:

#### 8.2.1 packages/evals 结构

```
packages/evals/
├── src/
│   ├── pi-harness.ts      # Pi Coding Agent harness
│   ├── vitest-evals/      # vitest-evals 适配
│   │   ├── artifacts.ts   # 测试 artifacts
│   │   ├── harness-table.ts # harness 对比表
│   │   ├── reporter.ts    # 报告器
│   │   └── summary.ts     # 摘要
│   └── extensions.eval.ts # 扩展 eval 示例
├── test/                  # evals 测试
├── scripts/               # 运行脚本
├── vitest.config.ts       # vitest 配置
└── vitest.test.config.ts  # 测试配置
```

#### 8.2.2 createPiCodingAgentHarness

```typescript
const harness = createPiCodingAgentHarness({
  name: "claude-opus-4-6",
  model: { provider: "anthropic", id: "claude-opus-4-6" },
  noTools: "all",
  transformSystemPrompt: (prompt) => prompt,
  output: ({ response, session }) => ({
    response,
    activeTools: session.getActiveToolNames(),
    extensionErrors: session.resourceLoader.getExtensions().errors,
  }),
});
```

- **model 选择**:显式选择 model 使 model-comparison harnesses 独立于 runner 默认。
- **noTools**:Pi 的工具禁用配置。
- **transformSystemPrompt**:在 eval 开始前转换完整默认 prompt。
- **output**:暴露 scenario-specific JSON-safe 行为。

#### 8.2.3 evalHarnessTable

```typescript
const harnessTable = evalHarnessTable("target skill effectiveness", {
  baseline: withoutTargetSkillHarness,
  candidate: withTargetSkillSkillHarness,
  repetitions: 6,
});
describe.for(harnessTable)("$name repetition $repetition", ({ harness }) => {
  describeEval("target skill effectiveness", { harness, judges: [TargetTaskJudge], judgeThreshold: null }, (it) => {
    it("completes the target task", async ({ run }) => {
      await run("Complete the target task.");
    });
  });
});
```

- **repetitions**:6 次重复,降低随机性。
- **judgeThreshold:null**:保留低分观察,不直接 fail。
- **lift 计算**:candidate pass rate - baseline pass rate。

### 8.3 关键测试文件

```
packages/ai/test/                    # AI 包测试
packages/protocol/test/              # 协议测试
packages/coding-agent/test/          # Coding Agent 测试
packages/server/test/                # 服务器测试
packages/client/test/                # 客户端测试
packages/evals/test/                 # Evals 测试
packages/agent/test/                 # Agent 测试
packages/telemetry/test/             # Telemetry 测试
packages/tui/test/                   # TUI 测试
packages/session-backends/sqlite-node/test/ # SQLite 后端测试
```

### 8.4 评估最佳实践(可借鉴)

1. **createPiCodingAgentHarness**:laew 可借鉴此模式,为 Yolo / Main-Work / SubAgent-Work 分别创建 harness。
2. **evalHarnessTable**:pairwise comparison 是 Agent 评测的最佳实践,laew 可直接复用。
3. **repetitions + lift**:6 次重复 + lift 计算是标准做法。
4. **output 转换**:暴露 scenario-specific JSON-safe 行为是关键设计。

### 8.5 测试不足与差距

- **零 Mock LLM**:没有内置 Mock provider,所有 Eval 都依赖真实 LLM。
- **零录制回放**:没有统一的录制回放框架。
- **零性能基准**:没有 bench:test / hyperfine 脚本。
- **覆盖率门槛不明确**:没有 100% per-file gate。

---

## 第 9 章 hermes-agent(Python)

### 9.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/hermes-agent`
- **栈**:Python 3.13、859 MB、6 前端共享 AIAgent、38 provider
- **测试框架**:pytest(主)、17k tests、per-file subprocess isolation
- **入口**:`tests/` 40 个子目录

### 9.2 测试架构剖析

hermes-agent 是 15 工程中**测试规模最大**的项目,核心亮点是 **17k tests + per-file subprocess isolation + conftest 沙箱**:

#### 9.2.1 测试目录结构

```
tests/
├── agent/                  # Agent 测试(最大)
├── hermes_cli/             # CLI 测试(最大)
├── gateway/                # Gateway 测试(最大)
├── run_agent/              # 运行 Agent 测试
├── cli/                    # CLI 测试
├── tools/                  # 工具测试
├── providers/              # Provider 测试
├── plugins/                # 插件测试
├── skills/                 # 技能测试
├── integration/            # 集成测试
├── e2e/                    # E2E 测试
├── conformance/            # 一致性测试
├── fakes/                  # Fake 实现
├── fixtures/               # Fixtures
├── perf_guards/            # 性能守卫
├── security/               # 安全测试
├── docker/                 # Docker 测试
├── desktop/                # 桌面测试
├── ui-tui/src/__tests__/   # TUI 测试
└── ... (40+ 子目录)
```

#### 9.2.2 conftest.py 沙箱机制

`tests/conftest.py` 是 15 工程中最复杂的 conftest,核心机制:

1. **凭证环境变量过滤**:过滤 80+ 种凭证环境变量后缀/名,防止真实 API Key 泄漏到测试中。
2. **HERMES_HOME 沙箱**:每个测试 session 创建独立 tempdir,防止污染真实 `~/.hermes/`。
3. **确定性运行时**:`TZ=UTC`, `LANG=C.UTF-8`, `PYTHONHASHSEED=0`。
4. **per-file subprocess isolation**:`scripts/run_tests_parallel.py` 为每个测试文件 spawn 独立的 `python -m pytest <file>` 子进程。

#### 9.2.3 pytest 配置

```ini
[tool.pytest.ini_options]
testpaths = ["tests"]
markers = [
    "integration: marks tests requiring external services",
    "real_concurrent_gate: opt out of the autouse stub",
    "real_agent_prewarm: opt out of the autouse stub",
    "requires_wal: needs the runtime to actually enable SQLite WAL mode",
    "no_isolate: opt out of per-file subprocess isolation",
    "ssh: marks tests requiring a reachable SSH server",
    "linux_only: exercises Linux-specific behaviour",
    "macos_only: exercises macOS-specific behaviour",
    "windows_only: exercises native-Windows behaviour",
]
addopts = "-m 'not integration'"
```

### 9.3 关键测试文件

```
tests/conftest.py                    # 全局 conftest
tests/fakes/fake_ha_server.py        # Fake HA 服务器
tests/e2e/                           # E2E 测试
tests/integration/                   # 集成测试
tests/perf_guards/                   # 性能守卫
tests/security/                      # 安全测试
tests/conformance/                   # 一致性测试
evals/                               # Evals 目录
├── anthropic_proxy_thinking_replay.py
├── auxiliary_resource_exhausted.py
├── cli_deferred_notice.py
├── cli_fallback_add_picker_error.py
├── codex_masked_replay_review.py
├── fanout_resource_bench.py
├── slack_stream_wire_contract.py
├── compaction/                      # 压缩 eval
├── core_tool_deferral/              # 核心工具延迟
├── postmortem/                      # 事后分析
├── token_accounting/                # Token 计费
└── ... (20+ eval 脚本)
```

### 9.4 评估最佳实践(可借鉴)

1. **per-file subprocess isolation**:laew 可借鉴此模式,为每个集成测试文件提供独立进程。
2. **凭证环境变量过滤**:laew 的 E2E 测试可借鉴此模式,防止真实 API Key 泄漏到测试中。
3. **HERMES_HOME 沙箱**:laew 可借鉴此模式,为每个测试提供独立的 SQLite 数据库目录。
4. **evals/ 20+ 脚本**:hermes-agent 的 eval 脚本覆盖 compaction / token accounting / core tool deferral 等多个维度,laew 可参考。

### 9.5 测试不足与差距

- **零 Mock LLM**:没有内置 Mock provider,所有 Eval 都依赖真实 LLM。
- **零录制回放**:没有统一的录制回放框架。
- **零属性测试**:17k tests 中没有 Hypothesis / proptest 集成。
- **CI 复杂度高**:per-file subprocess isolation 导致 CI 时间长。

---

## 第 10 章 agent-core(Python)

### 10.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/agent-core`
- **栈**:Python、openJiuwen Core SDK、ReAct、ContextEngine、Pregel
- **测试框架**:pytest(主)、unit_tests + system_tests
- **入口**:`tests/` 目录

### 10.2 测试架构剖析

agent-core 是 15 工程中**测试分层最清晰**的项目,核心亮点是 **unit_tests + system_tests 双层结构**:

```
tests/
├── unit_tests/                # 单元测试
│   ├── agent_evolving/        # Agent 进化
│   │   └── evaluator/         # 评估器
│   │       └── test_metrics   # 指标测试
│   └── harness/               # 测试 harness
│       └── tools/             # 工具测试
│           ├── test_powershell
│           └── test_bash
├── system_tests/              # 系统测试
└── ... (其他测试)
```

### 10.3 评估最佳实践(可借鉴)

1. **unit_tests + system_tests 双层结构**:laew 可借鉴此模式,清晰分离单元测试与系统测试。
2. **skill_evaluator**:技能评估器是 laew 可以借鉴的模式,验证 Skill 的有效性。
3. **harness/tools**:工具测试 harness 是 laew 可以借鉴的模式。

### 10.4 测试不足与差距

- **零 Mock LLM**:没有内置 Mock provider。
- **零录制回放**:没有统一的录制回放框架。
- **零性能基准**:没有 pytest-benchmark 集成。
- **CI 配置不明确**:没有公开的 CI 配置。

---

## 第 11 章 agent-studio(Python+React)

### 11.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/agent-studio`
- **栈**:Python + React、一站式 Agent 平台、Pregel cba 消减
- **测试框架**:pytest(后端) + jest(前端) + Playwright(E2E)
- **入口**:`backend/tests/` + `frontend/src/test/`

### 11.2 测试架构剖析

agent-studio 是 15 工程中**前后端测试最完整**的项目,核心亮点是 **Playwright E2E**:

```
backend/tests/                   # 后端测试
├── openjiuwen_studio/
│   ├── models/tests/            # 模型测试
│   ├── core/
│   │   ├── dsl_converter/tests/ # DSL 转换器测试
│   │   └── executor/
│   │       └── evaluation/tests/ # 执行器评估测试
│   └── marketplace/
│       └── ready_plugins/testing/ # 插件测试
connect/adapters/mcp_server/tests/ # MCP 服务器测试
frontend/src/test/               # 前端测试
├── pages/
│   ├── Plugins/__tests__/       # 插件页面测试
│   └── Apps/components/
│       └── ReportPanel/editor/
│           ├── __tests__/       # 编辑器测试
│           ├── canonical/__tests/ # 规范化测试
│           ├── rewrite/__tests/ # 重写测试
│           ├── sync/__tests/    # 同步测试
│           └── session/__tests/ # 会话测试
frontend/packages/workflow-canvas/src/components/testrun/ # 工作流测试
```

### 11.3 评估最佳实践(可借鉴)

1. **Playwright E2E**:laew 的 TUI 测试可借鉴 Playwright 的模式,但需要适配 tmux control-mode。
2. **前后端分离测试**:laew 可借鉴此模式,分离 Rust 核心逻辑测试与 TUI 渲染测试。
3. **evaluation/tests**:执行器评估测试是 laew 可以借鉴的模式。

### 11.4 测试不足与差距

- **零 Mock LLM**:没有内置 Mock provider。
- **零录制回放**:没有统一的录制回放框架。
- **零性能基准**:没有 pytest-benchmark 集成。
- **CI 配置不明确**:没有公开的 CI 配置。

---

## 第 12 章 cc-switch(Tauri 2)

### 12.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/cc-switch`
- **栈**:Tauri 2 + Rust + React、8 款工具适配、熔断器三态
- **测试框架**:vitest(主) + jsdom + Playwright(E2E)
- **入口**:`tests/` 目录

### 12.2 测试架构剖析

cc-switch 是 15 工程中**Tauri 测试最完整**的项目,核心亮点是 **vitest + jsdom + Playwright**:

```
tests/
├── components/                # 组件测试
├── config/                    # 配置测试
├── hooks/                     # Hook 测试
├── integration/               # 集成测试
├── lib/                       # 库测试
├── msw/                       # MSW Mock 测试
├── types/                     # 类型测试
├── utils/                     # 工具测试
├── setupGlobals.ts            # 全局 setup
└── setupTests.ts              # 测试 setup
```

### 12.3 MSW Mock

`tests/msw/` 使用 MSW(Mock Service Worker)进行 HTTP Mock:

- **MSW**:Service Worker 级别的 HTTP Mock,拦截真实网络请求。
- **适用场景**:前端 API 调用 Mock。

### 12.4 评估最佳实践(可借鉴)

1. **MSW Mock**:laew 的协议 wire 测试可借鉴 MSW 的模式,但需要适配 Rust reqwest 客户端。
2. **vitest + jsdom**:laew 的 TUI 测试可借鉴 jsdom 的模式,但需要适配 crossterm。
3. **integration 目录**:集成测试目录是 laew 可以借鉴的模式。

### 12.5 测试不足与差距

- **零 Mock LLM**:MSW 仅 Mock HTTP,不提供 LLM 语义 Mock。
- **零录制回放**:没有统一的录制回放框架。
- **零 Eval 框架**:没有 pairwise / LLM-as-judge 集成。
- **覆盖率门槛不明确**:没有 100% per-file gate。

---

## 第 13 章 jiuwenswarm(Python)

### 13.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/jiuwenswarm`
- **栈**:Python、多 Agent 协作、Leader-Teammate、A2A/ACP/E2A/A2UI
- **测试框架**:pytest(主) + --cov
- **入口**:`tests/` 目录

### 13.2 测试架构剖析

jiuwenswarm 是 15 工程中**多 Agent 测试最完整**的项目,核心亮点是 **unit_tests + system_tests + ui_e2e 三层结构**:

```
tests/
├── agents/                    # Agent 测试
├── auth/                      # 认证测试
├── integration/               # 集成测试
├── symphony/                  # Symphony 测试
├── system_tests/              # 系统测试
├── ui_e2e/                    # UI E2E 测试
├── unit/                      # 单元测试
├── unit_tests/                # 单元测试(重复)
└── conftest.py                # conftest
```

### 13.3 pytest 配置

```ini
[pytest]
python_files = test_*.py *_test.py
python_classes = Test*
python_functions = test_*
testpaths = tests
addopts =
    -v
    --strict-markers
    --tb=short
    --cov=jiuwenswarm
    --cov-report=term-missing
    --cov-report=html
    --cov-report=xml
    --asyncio-mode=auto
markers =
    unit: Unit tests
    integration: Integration tests
    system: System tests
    slow: Slow running tests
    async: Async tests
```

### 13.4 评估最佳实践(可借鉴)

1. **unit_tests + system_tests + ui_e2e 三层结构**:laew 可借鉴此模式,清晰分离不同层次的测试。
2. **--cov + term-missing + html + xml**:覆盖率报告是最佳实践,laew 可借鉴。
3. **--asyncio-mode=auto**:异步测试自动模式是 laew 可以借鉴的模式。

### 13.5 测试不足与差距

- **零 Mock LLM**:没有内置 Mock provider。
- **零录制回放**:没有统一的录制回放框架。
- **零性能基准**:没有 pytest-benchmark 集成。
- **覆盖率门槛不明确**:没有 100% per-file gate。

---

## 第 14 章 semantica(Python)

### 14.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/semantica`
- **栈**:Python、图原生 AI 基础设施、Context Graph、Rete、Datalog
- **测试框架**:pytest(主)、30 tests/
- **入口**:`tests/` 目录

### 14.2 测试架构剖析

semantica 是 15 工程中**图算法测试最完整**的项目,核心亮点是 **30 tests/ 覆盖图算法**:

```
tests/                           # 30 个测试文件
explorer/tests/                  # 探索器测试
```

### 14.3 评估最佳实践(可借鉴)

1. **图算法测试**:laew 的 Yolo 分类器可借鉴此模式,测试决策图。
2. **30 tests/ 精简测试**:小而精的测试集是 laew 可以借鉴的模式。

### 14.4 测试不足与差距

- **零 Mock LLM**:没有内置 Mock provider。
- **零录制回放**:没有统一的录制回放框架。
- **零性能基准**:没有 pytest-benchmark 集成。
- **CI 配置不明确**:没有公开的 CI 配置。

---

## 第 15 章 Switchyard(Rust)

### 15.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/Switchyard`
- **栈**:Rust(edition 2024)、NVIDIA LLM 网关、协议 IR、PyO3
- **测试框架**:cargo test(主) + criterion(bench) + pytest(Python 绑定)
- **入口**:`tests/` + `benchmark/` + `crates/*/tests/`

### 15.2 测试架构剖析

Switchyard 是 15 工程中**Rust 测试最完整**的项目,核心亮点是 **benchmark/run-baseline.sh + Harbor TBLite eval**:

```
tests/
├── e2e/                       # E2E 测试
├── getting_started/           # 入门测试
├── relay_plugin/              # 中继插件测试
├── test_aiperf_runner.py      # AI 性能运行器
├── test_cli_reference_docs.py # CLI 参考文档
├── test_libsy_minimal_bindings.py # libsy 最小绑定
├── test_prepare_harbor_dataset.py # Harbor 数据集
├── test_routing_performance_report.py # 路由性能报告
├── test_run_baseline_script.py # 基线脚本
├── test_run_manifest.py       # 运行清单
└── ... (其他测试)
benchmark/
├── run-baseline.sh            # 基线运行脚本
├── prepare_harbor_dataset.py  # Harbor 数据集准备
├── run_manifest.py            # 运行清单
├── server-configs/            # 服务器配置
├── routing-profiles/          # 路由配置
└── patches/                   # 补丁
crates/
├── switchyard-runner/tests/   # Runner 测试
├── prefill-router/tests/      # 预填充路由器测试
├── switchyard-skill-distillation/tests/ # 技能蒸馏测试
├── switchyard-translation/tests/ # 翻译测试
├── libsy-llm-client/tests/    # LLM 客户端测试
├── switchyard-soak/tests/     # 浸泡测试
├── switchyard-server/tests/   # 服务器测试
└── ... (其他 crate 测试)
```

### 15.3 benchmark/run-baseline.sh 深度剖析

这是 15 工程中最完整的 **性能基准脚本**,核心特性:

- **Harbor 数据集**:`prepare_harbor_dataset.py` 准备 Terminal-Bench Lite / Terminal-Bench 2.0 / SWE-Bench Pro 数据集。
- **Docker 集成**:Docker + Compose 支持,任务容器隔离。
- **Switchyard 路由**:支持 LLM 分类器路由(strong / weak 两档)。
- **Closed-book / Open-book**:两种测试模式。
- **运行清单**:`run_manifest.py` 记录命令 / git 状态 / Harbor patch 来源 / 数据集 digest / 服务器配置 / 代理元数据 / 上游元数据 / agent 版本。
- **指标采集**:`server_metrics_final.prom` + `routing_stats_final.json`。

### 15.4 评估最佳实践(可借鉴)

1. **benchmark/run-baseline.sh**:laew 可借鉴此模式,建立 Yolo 分类器 / 协议 wire / TUI 渲染的性能基准。
2. **Harbor 数据集**:Terminal-Bench / SWE-Bench 是标准 Agent 评测集,laew 可参考。
3. **closed-book / open-book**:两种测试模式是最佳实践,laew 可借鉴。
4. **运行清单**:`run_manifest.json` 记录完整运行元数据,laew 可借鉴。

### 15.5 测试不足与差距

- **零 Mock LLM**:没有内置 Mock provider,所有测试依赖真实 LLM。
- **零录制回放**:没有统一的录制回放框架。
- **零属性测试**:11 个 crates 中没有 proptest 集成。
- **CI 配置不明确**:没有公开的 CI 配置。

---

## 第 16 章 TencentDB-Agent-Memory(TS+Python)

### 16.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory`
- **栈**:TypeScript + Python、团队记忆系统、L0-L3 管线
- **测试框架**:jest(TS) + pytest(Python)
- **入口**:`MemoryCore/` / `MemoryKnowledge/` / `MemoryPanel/` / `MemoryProxy/`

### 16.2 测试架构剖析

TencentDB-Agent-Memory 是 15 工程中**测试最薄弱**的项目之一:

- **零公开测试目录**:没有 `tests/` 目录。
- **零 CI 配置**:没有公开的 CI 配置。
- **零 Mock LLM**:没有内置 Mock provider。
- **零录制回放**:没有统一的录制回放框架。

### 16.3 潜在测试点

- **L0-L3 管线**:记忆管线测试。
- **SkillCore 6 写 4 读**:技能核心测试。
- **InjectionPipeline 8 注入点**:注入管线测试。
- **RRF 混合检索**:检索算法测试。

### 16.4 评估最佳实践(可借鉴)

1. **L0-L3 管线测试**:laew 可借鉴此模式,测试 Agent-Memory 管线。
2. **RRF 混合检索**:检索算法测试是 laew 可以借鉴的模式。

### 16.5 测试不足与差距

- **零测试**:TencentDB-Agent-Memory 是 15 工程中**唯一没有公开测试**的项目。
- **零 CI**:没有公开的 CI 配置。
- **零 Mock LLM**:没有内置 Mock provider。
- **零录制回放**:没有统一的录制回放框架。

---

## 第 17 章 undici(Node.js)

### 17.1 工程概览

- **路径**:`/usr/local/LsmGitOpenSource/undici`
- **栈**:Node.js、HTTP/1.1 客户端、llhttp WASM、8 拦截器
- **测试框架**:borp(主) + jest(部分) + c8(覆盖率) + fast-check(属性测试)
- **入口**:`test/` 1732 个文件

### 17.2 测试架构剖析

undici 是 15 工程中**测试最完整**的项目之一,核心亮点是 **borp + jest + c8 + fast-check**:

```
test/
├── autobahn/                  # Autobahn WebSocket 测试
├── busboy/                    # Busboy 测试
├── cache/                     # 缓存测试
├── cache-interceptor/         # 缓存拦截器测试
├── cookie/                    # Cookie 测试
├── fetch/                     # Fetch 测试
├── infra/                     # 基础设施测试
├── interceptors/              # 拦截器测试
├── jest/                      # Jest 测试
│   ├── instanceof-error.test.js
│   ├── issue-1757.test.js
│   ├── mock-agent.test.js
│   ├── mock-scope.test.js
│   ├── parser-timeout.test.js
│   ├── test.js
│   └── util-timers.test.js
├── node-fetch/                # Node Fetch 测试
├── node-test/                 # Node 测试
├── web-platform-tests/        # W3C Web Platform Tests
├── websocket/                 # WebSocket 测试
├── client.js                  # 客户端测试(59KB)
├── client-request.js          # 客户端请求测试(42KB)
├── client-stream.js           # 客户端流测试(21KB)
├── client-pipeline.js         # 客户端管道测试(25KB)
├── client-pipelining.js       # 客户端管道测试(19KB)
├── content-length.js          # 内容长度测试
├── decorator-handler.js       # 装饰器处理器测试
├── errors.js                  # 错误测试
└── ... (1732 个文件)
```

### 17.3 测试框架矩阵

| 框架 | 用途 | 覆盖率 |
|------|------|--------|
| borp | 主测试框架 | 180s 超时 |
| jest | 部分测试 | NODE_V8_COVERAGE |
| c8 | 覆盖率 | 100% |
| fast-check | 属性测试 | 属性覆盖 |
| tsd | 类型测试 | 类型覆盖 |

### 17.4 MockAgent 深度剖析

undici 内置 `MockAgent`,是 15 工程中最完整的 **HTTP Mock 实现**:

```javascript
mockAgent = new MockAgent()
setGlobalDispatcher(mockAgent)
const mockClient = mockAgent.get(baseUrl)
mockClient.intercept({
  path: '/foo?hello=there&see=ya',
  method: 'POST',
  body: 'form1=data1&form2=data2'
}).reply(200, { foo: 'bar' }, {
  headers: { 'content-type': 'application/json' },
  trailers: { 'Content-MD5': 'test' }
})
```

### 17.5 fast-check 属性测试

undici 使用 fast-check 进行属性测试:

- **属性测试**:验证 HTTP 客户端的通用属性。
- **适用场景**:协议解析、编码解码、边界条件。

### 17.6 评估最佳实践(可借鉴)

1. **borp + jest 双框架**:laew 可借鉴此模式,cargo test + 自定义 E2E 脚本。
2. **c8 覆盖率**:laew 可借鉴 c8 的模式,使用 cargo-tarpaulin。
3. **fast-check 属性测试**:laew 可借鉴此模式,使用 proptest 进行协议 wire 属性测试。
4. **MockAgent**:laew 可借鉴此模式,内置 HTTP Mock 客户端。
5. **W3C Web Platform Tests**:laew 可借鉴此模式,引入标准协议测试集。

### 17.7 测试不足与差距

- **零 Eval 框架**:undici 是 HTTP 客户端,不适用 Agent Eval。
- **零录制回放**:cache-tests 是部分录制回放,但不完整。
- **CI 配置不明确**:没有公开的 CI 配置。

---

## 第 18 章 Mock LLM 实现矩阵

### 18.1 横向对比表

| 工程 | Mock 类型 | 实现方式 | 故障注入 | 脚本化 | 随机模式 | 覆盖率 |
|------|-----------|----------|----------|--------|----------|--------|
| atomcode | ❌ 无 | - | - | - | - | - |
| claudecode | ⚠️ fixtures | 静态 fixture | ❌ | ❌ | ❌ | 低 |
| deepseek-harness | ✅ dsh-llm-mock-server | OpenAI 兼容 HTTP/SSE | ✅ 20+ 模式 | ✅ FIFO | ✅ 加权随机 | 高 |
| openclaw | ⚠️ non-isolated-runner | 非隔离运行器 | ❌ | ❌ | ❌ | 中 |
| opencode | ⚠️ http-recorder | 录制回放 | ❌ | ❌ | ❌ | 中 |
| pi | ⚠️ 模拟 provider | 本地 provider | ❌ | ❌ | ❌ | 低 |
| hermes-agent | ⚠️ fake_ha_server | Fake HA 服务器 | ❌ | ❌ | ❌ | 低 |
| agent-core | ❌ 无 | - | - | - | - | - |
| agent-studio | ❌ 无 | - | - | - | - | - |
| cc-switch | ⚠️ msw | Service Worker Mock | ❌ | ❌ | ❌ | 低 |
| jiuwenswarm | ❌ 无 | - | - | - | - | - |
| semantica | ❌ 无 | - | - | - | - | - |
| Switchyard | ⚠️ mock client | 模拟客户端 | ❌ | ❌ | ❌ | 低 |
| TencentDB-Agent-Memory | ❌ 无 | - | - | - | - | - |
| undici | ✅ MockAgent | HTTP Mock | ❌ | ❌ | ❌ | 高 |

### 18.2 deepseek-harness dsh-llm-mock-server 深度剖析

#### 18.2.1 架构设计

dsh-llm-mock-server 是 15 工程中最完整的 Mock LLM 实现,核心特性:

- **脚本化行为**:`--sequence partial_disconnect,success` 定义 FIFO 行为序列。
- **20+ 种故障模式**:连接故障 / 流故障 / 协议故障 / 限流故障 / 认证故障 / 成功模式 / 内容故障 / 随机模式。
- **随机模式**:`--seed 42 --random-weights 'success=60,...'` 提供可复现的混合压力测试。
- **请求捕获**:每个请求和结果都记录在返回的 handle 上,供测试断言。
- **CLI + 库双入口**:`pnpm run mock:llm` 独立运行,`startMockLlmServer` 嵌入测试。

#### 18.2.2 20+ 种故障模式详解

| 类别 | 行为 | 触发条件 | 恢复策略 |
|------|------|----------|----------|
| **连接故障** | connection_reset | socket destroy before headers | 自动重试 |
| | connection_refused | listener delay | 等待 listener |
| **流故障** | stream_disconnect | SSE headers then reset | 自动重试 |
| | partial_disconnect | text deltas then reset | 自动重试 |
| | stall | SSE headers then idle | 超时取消 |
| **协议故障** | malformed_json | invalid SSE JSON | 报错 |
| | malformed_event | invalid chunk shape | 报错 |
| | empty_body | no [DONE] | 报错 |
| | stream_eof | clean EOF no [DONE] | 报错 |
| | partial_eof | partial [DONE] | 报错 |
| **限流故障** | rate_limit | 429 JSON | 退避重试 |
| | server_error | 500 JSON | 退避重试 |
| | service_unavailable | 503 JSON | 退避重试 |
| **认证故障** | auth_error | 401 JSON | 终止 |
| | invalid_request | 400 JSON | 终止 |
| | context_overflow | 413 JSON | 终止 |
| | quota_exceeded | 429 JSON | 终止 |
| **成功模式** | success | 完整流 | - |
| | slow_success | 延迟流 | - |
| | reasoning_success | 推理 + 流 | - |
| | tool_call_success | 工具调用 | - |
| | max_tokens | length finish | - |
| **内容故障** | wrong_content_type | application/json | 报错 |
| **随机模式** | random | 加权随机 | - |

### 18.3 opencode http-recorder 深度剖析

#### 18.3.1 架构设计

HttpRecorder 是 opencode 的核心录制回放框架:

- **双 API**:`HttpRecorder.http(name, options?)` 与 `HttpRecorder.socket(name, options?)`。
- **Cassette 格式**:JSON 文件,request 顺序存储,WebSocket 保留 client/server frame 时序。
- **Redaction 机制**:7 种脱敏选项(headers / queryParameters / jsonFields / url / body / allowRequestHeaders / allowResponseHeaders)。
- **匹配规则**:严格顺序匹配,JSON object keys 规范化,支持自定义 `match` 函数。
- **CI 模式**:`CI=true` 时 missing cassette 直接 fail,防止"静默录制"。
- **刷新机制**:删除 cassette 后重新运行即刷新,无公开 overwrite 模式(避免不可见覆盖)。

### 18.4 undici MockAgent 深度剖析

undici 内置 MockAgent,是 15 工程中最完整的 HTTP Mock 实现:

```javascript
mockAgent = new MockAgent()
setGlobalDispatcher(mockAgent)
const mockClient = mockAgent.get(baseUrl)
mockClient.intercept({
  path: '/foo?hello=there&see=ya',
  method: 'POST',
  body: 'form1=data1&form2=data2'
}).reply(200, { foo: 'bar' }, {
  headers: { 'content-type': 'application/json' },
  trailers: { 'Content-MD5': 'test' }
})
```

### 18.5 laew Mock LLM 差距分析

laew 当前**零 Mock LLM**,是最大的测试短板。对比 15 工程:

- **最佳实践**:deepseek-harness dsh-llm-mock-server(20+ 故障模式 + 随机模式)。
- **次优实践**:opencode http-recorder(录制回放 + redaction)。
- **基础实践**:undici MockAgent(HTTP Mock)。

**laew 需要**:
1. 内置 Mock LLM 服务器,支持 Anthropic / OpenAI 双协议。
2. 20+ 故障模式(连接故障 / 流故障 / 协议故障 / 限流故障 / 认证故障)。
3. 脚本化行为序列(FIFO cursor)。
4. 随机模式(seeded PRNG + 加权随机)。
5. 请求捕获(每个请求和结果按到达顺序记录)。

---

## 第 19 章 录制回放实现矩阵

### 19.1 横向对比表

| 工程 | 录制回放 | 实现方式 | Fixture 格式 | Redaction | CI 模式 | 刷新机制 |
|------|----------|----------|--------------|-----------|---------|----------|
| atomcode | ⚠️ fixture replay | 静态 fixture | case.json | scrub() | - | - |
| claudecode | ⚠️ vitest-evals | 录制回放 | fixtures | - | - | - |
| deepseek-harness | ✅ dsh-llm-replay | JSONL 投影 | session.jsonl | - | keyless | record/refresh |
| openclaw | ❌ 无 | - | - | - | - | - |
| opencode | ✅ http-recorder | Cassette | JSON | 7 种脱敏 | CI=true fail | 删除重录 |
| pi | ⚠️ evalHarnessTable | 本地 fixture | - | - | - | - |
| hermes-agent | ⚠️ evals/ 20+ 脚本 | 独立 eval 脚本 | - | - | - | - |
| agent-core | ❌ 无 | - | - | - | - | - |
| agent-studio | ❌ 无 | - | - | - | - | - |
| cc-switch | ⚠️ msw | Service Worker | handlers | - | - | - |
| jiuwenswarm | ❌ 无 | - | - | - | - | - |
| semantica | ❌ 无 | - | - | - | - | - |
| Switchyard | ⚠️ Harbor fixture | 数据集 | - | - | - | - |
| TencentDB-Agent-Memory | ❌ 无 | - | - | - | - | - |
| undici | ⚠️ cache-tests | 缓存测试 | - | - | - | - |

### 19.2 deepseek-harness dsh-llm-replay 深度剖析

dsh-llm-replay 是 15 工程中最完整的录制回放实现,核心特性:

- **JSONL fixture**:fixture 是 session log 的投影,保留 header 和 event payload,省略 `seq/time` 信封。
- **派生脚本**:`deriveReplayScript` 解析 JSONL header,按 `(turn, step)` 键拆分 `assistant/chunk` 事件。
- **嵌套 Agent 绑定**:parent + child session 按 first-call order 绑定,每个 session 独立 cursor。
- **Override 侧车**:`replay.override.json` 支持 throw / hang / patch 三种覆盖模式。
- **占位符解析**:`{{fromRequest:<regex>}}` 在 stream 时解析,取最后匹配的第一个捕获组。
- **assertConsumed()**:teardown 时验证所有脚本都被消费,防止"静默少调用"。

### 19.3 opencode http-recorder 深度剖析

http-recorder 的 Cassette 格式与刷新机制:

- **Cassette 格式**:JSON 文件,request 顺序存储,WebSocket 保留 client/server frame 时序。
- **Redaction**:7 种脱敏选项(headers / queryParameters / jsonFields / url / body / allowRequestHeaders / allowResponseHeaders)。
- **刷新机制**:删除 cassette 后重新运行即刷新,无公开 overwrite 模式(避免不可见覆盖)。
- **CI 模式**:`CI=true` 时 missing cassette 直接 fail,防止"静默录制"。

### 19.4 laew 录制回放差距分析

laew 当前**零录制回放**,是第二大测试短板。对比 15 工程:

- **最佳实践**:deepseek-harness dsh-llm-replay(JSONL 投影 + override 侧车 + assertConsumed)。
- **次优实践**:opencode http-recorder(Cassette + redaction + CI 模式)。

**laew 需要**:
1. 录制真实 Anthropic / OpenAI 响应到 JSONL fixture。
2. 派生脚本:解析 JSONL,按 (turn, step) 键拆分 chunk。
3. Override 侧车:throw / hang / patch 三种覆盖模式。
4. Redaction:脱敏 API Key / Token / 敏感 header。
5. CI 模式:`CI=true` 时 missing cassette fail。
6. assertConsumed:验证所有 script 被消费。

---

## 第 20 章 Eval 框架与 pairwise 对比

### 20.1 横向对比表

| 工程 | Eval 框架 | Pairwise | LLM-as-judge | Golden Dataset | Repetitions | Lift 计算 |
|------|-----------|----------|--------------|----------------|-------------|-----------|
| atomcode | ✅ eval.py | ✅ official vs volcengine | ❌ | ✅ case.json | ✅ 3+ | ❌ |
| claudecode | ✅ vitest-evals | ✅ evalHarnessTable | ✅ createJudge | ❌ | ✅ 6 | ✅ |
| deepseek-harness | ✅ session-snapshot | ❌ | ❌ | ✅ session.jsonl | ❌ | ❌ |
| openclaw | ⚠️ 部分 | ❌ | ❌ | ❌ | ❌ | ❌ |
| opencode | ⚠️ 部分 | ❌ | ❌ | ❌ | ❌ | ❌ |
| pi | ✅ vitest-evals | ✅ evalHarnessTable | ✅ createJudge | ❌ | ✅ 6 | ✅ |
| hermes-agent | ✅ evals/ 20+ 脚本 | ❌ | ❌ | ✅ evals/ | ❌ | ❌ |
| agent-core | ✅ skill_evaluator | ❌ | ❌ | ❌ | ❌ | ❌ |
| agent-studio | ✅ evaluation/tests | ❌ | ❌ | ❌ | ❌ | ❌ |
| cc-switch | ❌ 无 | ❌ | ❌ | ❌ | ❌ | ❌ |
| jiuwenswarm | ⚠️ 部分 | ❌ | ❌ | ❌ | ❌ | ❌ |
| semantica | ⚠️ 部分 | ❌ | ❌ | ❌ | ❌ | ❌ |
| Switchyard | ✅ Harbor TBLite | ❌ | ❌ | ✅ TBLite / SWE-Bench | ❌ | ❌ |
| TencentDB-Agent-Memory | ❌ 无 | ❌ | ❌ | ❌ | ❌ | ❌ |
| undici | ❌ 不适用 | ❌ | ❌ | ❌ | ❌ | ❌ |

### 20.2 atomcode eval.py 深度剖析

atomcode 的 eval.py 是 15 工程中**唯一走 std-lib Python** 实现 Eval harness 的项目:

- **零外部依赖**:argparse / asyncio / concurrent.futures / hashlib / json / math / os / random / re / shutil / statistics / subprocess / sys / tempfile / time / uuid / dataclasses / datetime / pathlib / typing。
- **无 pytest / 无 requests / 无 anthropic SDK**。
- **Candidate 配对**:支持 `official` / `volcengine` 双向对比,要求 `selection` 不同(防作弊)。
- **Suite 配置**:`repetitions` / `pair_concurrency` / `timeout_seconds` / `start_skew_ms` / `random_seed` / `atomcode_bin` / `codex_bin` / `codex_model`。
- **Case 发现**:`discover_cases()` glob `*/case.json` 加载 prompt + fixture + verify + rubric。
- **路径安全**:fixture 必须 resolve 后仍在 case 目录内(`cfg.parent.resolve() not in fixture.parents` 报错防越界)。
- **原子写**:`atomic_json()` 用 `temp + replace` 实现原子落盘,防并发覆盖。
- **Secret 脱敏**:`SECRET_RE` 正则匹配 `authorization/api[_-]?key/token` 字段,`scrub()` 替换为 `[REDACTED]`。
- **Token 统计**:`TOKEN_RE` 正则匹配 `[tokens] prompt=X completion=Y cached=Z` 协议行。

### 20.3 pi vitest-evals 深度剖析

pi 的 vitest-evals 是 15 工程中**Eval 框架最完整**的实现:

- **createPiCodingAgentHarness**:创建 Pi Coding Agent harness,支持 model 选择 / noTools / transformSystemPrompt / output 转换。
- **evalHarnessTable**:pairwise comparison,支持 baseline / candidate / candidates 三档对比。
- **repetitions**:6 次重复,降低随机性。
- **judgeThreshold:null**:保留低分观察,不直接 fail。
- **lift 计算**:candidate pass rate - baseline pass rate,以百分比点表示。

### 20.4 laew Eval 框架差距分析

laew 当前**零 Eval 框架**,是第三大测试短板。对比 15 工程:

- **最佳实践**:pi vitest-evals(evalHarnessTable + repetitions + lift)。
- **次优实践**:atomcode eval.py(std-lib only + candidate 配对 + rubric)。

**laew 需要**:
1. 创建 Yolo / Main-Work / SubAgent-Work 三套 harness。
2. pairwise comparison:baseline vs candidate。
3. repetitions:6 次重复。
4. lift 计算:candidate pass rate - baseline pass rate。
5. rubric:评分维度 + 评分标准说明。
6. golden dataset:case.json + prompt + fixture + verify + rubric。

---

## 第 21 章 CI 集成与覆盖率

### 21.1 横向对比表

| 工程 | CI 平台 | 覆盖率工具 | 覆盖率门槛 | 分区模式 | 并行度 | 重试 |
|------|---------|------------|------------|----------|--------|------|
| atomcode | GitHub Actions | ❌ | ❌ | ❌ | ❌ | ❌ |
| claudecode | GitHub Actions | ⚠️ 部分 | ❌ | ❌ | ❌ | ❌ |
| deepseek-harness | GitLab CI | v8 | ✅ 100% per-file | ✅ | ✅ | ✅ retry 2 |
| openclaw | GitHub Actions | v8 | ✅ 100% per-file | ✅ | ✅ | ✅ |
| opencode | GitHub Actions | ⚠️ 部分 | ❌ | ❌ | ✅ | ❌ |
| pi | GitHub Actions | ⚠️ 部分 | ❌ | ❌ | ❌ | ❌ |
| hermes-agent | GitHub Actions | coverage.py | ❌ | ❌ | ✅ per-file | ❌ |
| agent-core | ⚠️ 待验证 | ❌ | ❌ | ❌ | ❌ | ❌ |
| agent-studio | ⚠️ 待验证 | ❌ | ❌ | ❌ | ❌ | ❌ |
| cc-switch | GitHub Actions | text+lcov | ❌ | ❌ | ❌ | ❌ |
| jiuwenswarm | GitHub Actions | term-missing+xml | ❌ | ❌ | ❌ | ❌ |
| semantica | GitHub Actions | ❌ | ❌ | ❌ | ❌ | ❌ |
| Switchyard | GitHub Actions | ❌ | ❌ | ❌ | ❌ | ❌ |
| TencentDB-Agent-Memory | ⚠️ GitHub Actions | ❌ | ❌ | ❌ | ❌ | ❌ |
| undici | GitHub Actions | c8 | ✅ 100% | ❌ | ✅ | ❌ |

### 21.2 deepseek-harness 100% per-file 覆盖率门槛

deepseek-harness 的覆盖率配置是 15 工程中最严格的:

```typescript
thresholds: {
  perFile: true,        // 每个文件独立门槛
  statements: 100,      // 语句覆盖 100%
  branches: 100,        // 分支覆盖 100%
  functions: 100,       // 函数覆盖 100%
  lines: 100,           // 行覆盖 100%
}
```

**豁免机制**(coverage-exempt.ts):明确列出豁免文件,每个 `v8 ignore comment` 必须携带原因。

**分区模式**(coverage-partitions.ts):`COVERAGE_PARTITION_MODE_ENV` 开启后豁免 per-file 门槛,用于 CI 分区。

**平台不支持**:Windows 上禁用 Bash-requiring suites,但保留 pwsh-requiring suites。

### 21.3 hermes-agent per-file subprocess isolation

hermes-agent 的 CI 是 15 工程中**测试规模最大**的:

- **per-file subprocess isolation**:`scripts/run_tests_parallel.py` 为每个测试文件 spawn 独立的 `python -m pytest <file>` 子进程。
- **conftest 沙箱**:凭证环境变量过滤(80+ 种凭证) + HERMES_HOME 沙箱(独立 tempdir) + 确定性运行时(TZ=UTC, LANG=C.UTF-8, PYTHONHASHSEED=0)。
- **pytest 配置**:`addopts = "-m 'not integration'"`,markers:integration / real_concurrent_gate / real_agent_prewarm / requires_wal / no_isolate / ssh / linux_only / macos_only / windows_only。

### 21.4 laew CI 差距分析

laew 当前 CI 较薄,仅 `.github/workflows/build.yml`。对比 15 工程:

- **最佳实践**:deepseek-harness(100% per-file + 7 套配置 + 分区模式)。
- **次优实践**:hermes-agent(per-file subprocess isolation + conftest 沙箱)。

**laew 需要**:
1. 100% per-file 覆盖率门槛(cargo-tarpaulin)。
2. 多套测试配置:unit / integration / e2e / snapshot。
3. conftest 沙箱:凭证环境变量过滤 + 独立 SQLite 目录。
4. per-file subprocess isolation:每个集成测试文件独立进程。
5. 分区模式:CI 分区豁免 per-file 门槛。

---

## 第 22 章 性能基准(criterion / hyperfine / k6)

### 22.1 横向对比表

| 工程 | 性能基准 | 工具 | 指标 | CI 集成 | 历史追踪 |
|------|----------|------|------|---------|----------|
| atomcode | ❌ 无 | - | - | - | - |
| claudecode | ❌ 无 | - | - | - | - |
| deepseek-harness | ⚠️ web-perf | vitest | 前端性能 | ✅ | ❌ |
| openclaw | ⚠️ performance-config | vitest | 性能配置 | ✅ | ❌ |
| opencode | ✅ perf/test-suite.md | bun bench:test | test_suite_seconds | ✅ | ✅ |
| pi | ❌ 无 | - | - | - | - |
| hermes-agent | ⚠️ perf_guards | 自定义 | 性能守卫 | ❌ | ❌ |
| agent-core | ❌ 无 | - | - | - | - |
| agent-studio | ❌ 无 | - | - | - | - |
| cc-switch | ❌ 无 | - | - | - | - |
| jiuwenswarm | ❌ 无 | - | - | - | - |
| semantica | ❌ 无 | - | - | - | - |
| Switchyard | ✅ benchmark/run-baseline.sh | Harbor | routing_stats | ✅ | ✅ |
| TencentDB-Agent-Memory | ❌ 无 | - | - | - | - |
| undici | ✅ benchmarks/ | 自定义 | HTTP 性能 | ❌ | ❌ |

### 22.2 opencode perf/test-suite.md 深度剖析

opencode 的性能优化是 15 工程中**最系统化**的:

- **初始状态**:250s/run 全量测试。
- **优化后**:186s/run(prompt / processor / PTY 优化)。
- **核心模式**:`it.instance` Effect-aware fixture 替代手动 `tmpdir + withTestInstance`,减少 60-70% 测试时间。

**关键优化**:

| 优化项 | Before | After | 收益 |
|--------|--------|-------|------|
| Plugin install concurrency | 7.800s | 6.204s | -20% |
| httpapi-listen PTY git | 10.554s | 7.818s | -26% |
| workspace.waitForSync timeout | 12.949s | 8.305s | -36% |
| config.test sleep | 10.270s | 9.480s | -8% |
| SDK parity git repos | 8.011s | 5.180s | -35% |
| HTTP provider dependency-ready | 7.905s | 2.980s | -62% |
| TUI plugin lifecycle timeout | 7.330s | 1.507s | -79% |
| Skill tool git | 2.320s | 1.425s | -39% |
| Prompt shell git | 26.930s | 23.400s | -13% |
| Session processor git | 12.500s | 9.206s | -26% |
| Config load it.instance | 14.180s | 3.930s | -72% |
| CLI run subprocess concurrent | 11.870s | 4.130s | -65% |

### 22.3 Switchyard benchmark/run-baseline.sh 深度剖析

Switchyard 的 benchmark 是 15 工程中**最完整的性能基准**:

- **Harbor 数据集**:`prepare_harbor_dataset.py` 准备 Terminal-Bench Lite / Terminal-Bench 2.0 / SWE-Bench Pro 数据集。
- **Docker 集成**:Docker + Compose 支持,任务容器隔离。
- **Switchyard 路由**:支持 LLM 分类器路由(strong / weak 两档)。
- **Closed-book / Open-book**:两种测试模式。
- **运行清单**:`run_manifest.py` 记录命令 / git 状态 / Harbor patch 来源 / 数据集 digest / 服务器配置 / 代理元数据 / 上游元数据 / agent 版本。
- **指标采集**:`server_metrics_final.prom` + `routing_stats_final.json`。

### 22.4 laew 性能基准差距分析

laew 当前**零性能基准**。对比 15 工程:

- **最佳实践**:Switchyard benchmark/run-baseline.sh(Harbor + Docker + Prometheus)。
- **次优实践**:opencode perf/test-suite.md(假设-验证-决策循环)。

**laew 需要**:
1. Yolo 分类器性能基准:分类延迟 / 准确率 / 召回率。
2. 协议 wire 性能基准:序列化 / 反序列化延迟。
3. TUI 渲染性能基准:帧率 / 延迟。
4. SQLite 持久层性能基准:读写延迟 / 并发吞吐。
5. 历史追踪:每次 commit 的性能趋势。

---

## 第 23 章 属性测试 / Fuzz 测试 / 契约测试

### 23.1 属性测试横向对比表

| 工程 | 属性测试 | 工具 | 覆盖范围 | 集成方式 |
|------|----------|------|----------|----------|
| atomcode | ❌ 无 | - | - | - |
| claudecode | ❌ 无 | - | - | - |
| deepseek-harness | ❌ 无 | - | - | - |
| openclaw | ❌ 无 | - | - | - |
| opencode | ❌ 无 | - | - | - |
| pi | ❌ 无 | - | - | - |
| hermes-agent | ❌ 无 | - | - | - |
| agent-core | ❌ 无 | - | - | - |
| agent-studio | ❌ 无 | - | - | - |
| cc-switch | ❌ 无 | - | - | - |
| jiuwenswarm | ❌ 无 | - | - | - |
| semantica | ❌ 无 | - | - | - |
| Switchyard | ❌ 无 | - | - | - |
| TencentDB-Agent-Memory | ❌ 无 | - | - | - |
| undici | ✅ | fast-check | 协议解析 / 编码解码 | 集成 |

### 23.2 undici fast-check 深度剖析

undici 使用 fast-check 进行属性测试:

- **属性测试**:验证 HTTP 客户端的通用属性。
- **适用场景**:协议解析、编码解码、边界条件。
- **示例**:验证任意 path / method 组合都不会 crash。

### 23.3 Fuzz 测试横向对比表

| 工程 | Fuzz 测试 | 工具 | 覆盖范围 |
|------|-----------|------|----------|
| atomcode | ❌ 无 | - | - |
| claudecode | ❌ 无 | - | - |
| deepseek-harness | ❌ 无 | - | - |
| openclaw | ❌ 无 | - | - |
| opencode | ❌ 无 | - | - |
| pi | ❌ 无 | - | - |
| hermes-agent | ❌ 无 | - | - |
| agent-core | ❌ 无 | - | - |
| agent-studio | ❌ 无 | - | - |
| cc-switch | ❌ 无 | - | - |
| jiuwenswarm | ❌ 无 | - | - |
| semantica | ❌ 无 | - | - |
| Switchyard | ❌ 无 | - | - |
| TencentDB-Agent-Memory | ❌ 无 | - | - |
| undici | ⚠️ | test/fuzzing | HTTP 解析 |

### 23.4 契约测试横向对比表

| 工程 | 契约测试 | 工具 | 覆盖范围 |
|------|----------|------|----------|
| atomcode | ❌ 无 | - | - |
| claudecode | ❌ 无 | - | - |
| deepseek-harness | ❌ 无 | - | - |
| openclaw | ✅ contracts/ | 自定义 | Gateway/Harness/Adapter |
| opencode | ❌ 无 | - | - |
| pi | ❌ 无 | - | - |
| hermes-agent | ❌ 无 | - | - |
| agent-core | ❌ 无 | - | - |
| agent-studio | ❌ 无 | - | - |
| cc-switch | ❌ 无 | - | - |
| jiuwenswarm | ❌ 无 | - | - |
| semantica | ❌ 无 | - | - |
| Switchyard | ❌ 无 | - | - |
| TencentDB-Agent-Memory | ❌ 无 | - | - |
| undici | ❌ 无 | - | - |

### 23.5 openclaw contracts/ 深度剖析

openclaw 的 contracts/ 目录是 15 工程中**唯一的契约测试实现**:

```
test/contracts/
├── gateway/                    # Gateway 契约
├── harness/                    # Harness 契约
├── adapter/                    # Adapter 契约
└── ... (其他契约)
```

- **Gateway/Harness/Adapter 三层契约**:验证 openclaw 的核心架构契约。
- **集成测试**:契约测试作为集成测试的一部分。

### 23.6 laew 属性测试差距分析

laew 当前**零属性测试 / 零 Fuzz 测试 / 零契约测试**。对比 15 工程:

- **最佳实践**:undici fast-check(协议解析 / 编码解码 / 边界条件)。
- **次优实践**:openclaw contracts/(Gateway/Harness/Adapter 三层契约)。

**laew 需要**:
1. 协议 wire 属性测试:proptest 验证 Anthropic / OpenAI 序列化 / 反序列化。
2. Yolo 分类器属性测试:proptest 验证分类器的鲁棒性。
3. SQLite 持久层属性测试:proptest 验证 CRUD 操作的正确性。
4. Fuzz 测试:cargo-fuzz 验证 HTTP 解析器的鲁棒性。
5. 契约测试:验证 Multi-Agent 架构的 6 角色契约。

---

## 第 24 章 laew 现状评估与 20 个新 Gap(L206-L225)

### 24.1 laew 测试现状

基于 `testReport/run_e2e.sh` 与 `src/` 源码阅读,laew 当前测试现状:

- **单元测试**:✅ `cargo test` 覆盖基础模块,但无 100% 门槛。
- **集成测试**:⚠️ `testReport/run_e2e.sh` 覆盖部分场景,但依赖真实 LLM。
- **E2E 测试**:⚠️ tmux control-mode 覆盖 /provider list/add/del 子屏,未覆盖协议 wire 真实回放。
- **Mock LLM**:❌ 无。
- **录制回放**:❌ 无。
- **Eval 框架**:❌ 无。
- **属性测试**:❌ 无。
- **Fuzz 测试**:❌ 无。
- **契约测试**:❌ 无。
- **性能基准**:❌ 无。
- **CI 集成**:⚠️ `.github/workflows/build.yml` 仅构建,无测试门槛。
- **覆盖率**:❌ 无 cargo-tarpaulin。

### 24.2 新增 20 个 Gap(L206-L225)

| Gap ID | 类别 | 描述 | 严重度 | 对标工程 | Rust crate 建议 |
|--------|------|------|--------|----------|-----------------|
| **L206** | Mock LLM | 无内置 Mock LLM 服务器,E2E 依赖真实 API Key | P0 | deepseek-harness | `mockito` / `wiremock-rs` |
| **L207** | Mock LLM | 无故障注入能力,无法测试协议客户端容错 | P0 | deepseek-harness | 自定义 |
| **L208** | 录制回放 | 无录制回放框架,无法做确定性回归测试 | P0 | opencode | 自定义 |
| **L209** | 录制回放 | 无 redaction 机制,敏感信息可能泄漏到 fixture | P1 | opencode | 自定义 |
| **L210** | Eval 框架 | 无 pairwise comparison,无法对比 Yolo 分类器版本 | P0 | pi | 自定义 |
| **L211** | Eval 框架 | 无 LLM-as-judge,无法自动评估 Agent 输出质量 | P1 | pi | 自定义 |
| **L212** | Eval 框架 | 无 golden dataset,无法做标准化评测 | P1 | atomcode | 自定义 |
| **L213** | 覆盖率 | 无 100% per-file 覆盖率门槛 | P1 | deepseek-harness | `cargo-tarpaulin` |
| **L214** | 覆盖率 | 无分区模式,CI 难以并行 | P2 | deepseek-harness | 自定义 |
| **L215** | CI 集成 | 无 conftest 沙箱,凭证可能泄漏 | P1 | hermes-agent | 自定义 |
| **L216** | CI 集成 | 无 per-file subprocess isolation,测试可能互相污染 | P1 | hermes-agent | 自定义 |
| **L217** | 性能基准 | 无 Yolo 分类器性能基准 | P2 | Switchyard | `criterion` |
| **L218** | 性能基准 | 无协议 wire 性能基准 | P2 | opencode | `criterion` |
| **L219** | 性能基准 | 无 TUI 渲染性能基准 | P2 | opencode | 自定义 |
| **L220** | 属性测试 | 无协议 wire 属性测试 | P1 | undici | `proptest` |
| **L221** | 属性测试 | 无 Yolo 分类器属性测试 | P2 | undici | `proptest` |
| **L222** | 属性测试 | 无 SQLite 持久层属性测试 | P2 | undici | `proptest` |
| **L223** | Fuzz 测试 | 无 HTTP 解析器 Fuzz 测试 | P2 | undici | `cargo-fuzz` |
| **L224** | 契约测试 | 无 Multi-Agent 6 角色契约测试 | P1 | openclaw | 自定义 |
| **L225** | 契约测试 | 无 Gateway/Harness/Adapter 三层契约测试 | P2 | openclaw | 自定义 |

### 24.3 Gap 优先级分布

| 优先级 | 数量 | Gap IDs |
|--------|------|---------|
| P0 | 4 | L206 / L207 / L208 / L210 |
| P1 | 8 | L209 / L211 / L212 / L213 / L215 / L216 / L220 / L224 |
| P2 | 8 | L214 / L217 / L218 / L219 / L221 / L222 / L223 / L225 |

### 24.4 与前 10 轮 Gap 的关系

- **L1-L15**(第六轮):协议 wire / SubAgent / Goal / TUI / Hook / Skill。
- **L16-L25**(第七轮):文件编辑 / 代码检索 / Git / Bash / 多模态 / PromptCaching / Schema / Web。
- **L26-L37**(第八轮):Telemetry / Session / Tool 权限 / LSP / Hook / Skill / 多租户 / TUI。
- **L38-L78**(第九轮):CrashDump / WebUI / OAuth / i18n / Release / WebSocket / 容器 / CRDT。
- **L79-L142**(第十轮):P0 紧急 / P1 重要 / P2 进阶。
- **L142-L162**(第十一轮早期):测试体系基础。
- **L206-L225**(本轮):测试体系进阶(Mock LLM / 录制回放 / Eval / 属性测试 / Fuzz / 契约)。

---

## 第 25 章 Rust crate 推荐与改造路线图

### 25.1 推荐 Rust crate 矩阵

| 类别 | crate | 用途 | 对标工程 | 优先级 |
|------|-------|------|----------|--------|
| **Mock LLM** | `mockito` | HTTP Mock 服务器 | undici MockAgent | P0 |
| **Mock LLM** | `wiremock-rs` | Wire Mock 服务器 | deepseek-harness | P0 |
| **Mock LLM** | `httptest` | 测试 HTTP 服务器 | opencode | P0 |
| **录制回放** | `vcr-rs` | VCR 录制回放 | opencode http-recorder | P0 |
| **录制回放** | `insta` | Snapshot 测试 | deepseek-harness | P1 |
| **Eval 框架** | `criterion` | 性能基准 | Switchyard | P1 |
| **Eval 框架** | `proptest` | 属性测试 | undici fast-check | P1 |
| **Eval 框架** | `quickcheck` | 属性测试 | undici fast-check | P2 |
| **Fuzz 测试** | `cargo-fuzz` | Fuzz 测试 | undici | P2 |
| **Fuzz 测试** | `afl` | American Fuzz Lop | undici | P2 |
| **覆盖率** | `cargo-tarpaulin` | 代码覆盖率 | deepseek-harness | P1 |
| **覆盖率** | `cargo-llvm-cov` | LLVM 覆盖率 | deepseek-harness | P1 |
| **契约测试** | `pact-rs` | 契约测试 | openclaw | P2 |
| **测试工具** | `tempfile` | 临时文件 | hermes-agent | P0 |
| **测试工具** | `serial_test` | 串行测试 | hermes-agent | P1 |
| **测试工具** | `test-log` | 测试日志 | 通用 | P2 |
| **测试工具** | `pretty_assertions` | 断言美化 | 通用 | P2 |

### 25.2 4 阶段改造路线图

#### 阶段 1:基础补齐(1-2 周)

**目标**:补齐 P0 Mock LLM 与录制回放。

| 任务 | 描述 | 产出 |
|------|------|------|
| T1.1 | 集成 `mockito` / `wiremock-rs` | Mock LLM 服务器 |
| T1.2 | 实现 20+ 故障模式 | 故障注入能力 |
| T1.3 | 实现脚本化行为序列 | FIFO cursor |
| T1.4 | 实现录制回放框架 | JSONL fixture + redaction |
| T1.5 | 实现 assertConsumed | 验证所有 script 被消费 |

#### 阶段 2:Eval 框架(2-3 周)

**目标**:建立 pairwise comparison 与 LLM-as-judge。

| 任务 | 描述 | 产出 |
|------|------|------|
| T2.1 | 创建 Yolo / Main-Work / SubAgent-Work 三套 harness | Harness 体系 |
| T2.2 | 实现 pairwise comparison | baseline vs candidate |
| T2.3 | 实现 repetitions + lift | 6 次重复 + lift 计算 |
| T2.4 | 实现 LLM-as-judge | 自动评估 Agent 输出质量 |
| T2.5 | 建立 golden dataset | case.json + prompt + fixture + verify + rubric |

#### 阶段 3:覆盖率与 CI(1-2 周)

**目标**:100% per-file 覆盖率门槛 + CI 集成。

| 任务 | 描述 | 产出 |
|------|------|------|
| T3.1 | 集成 `cargo-tarpaulin` | 覆盖率工具 |
| T3.2 | 实现 100% per-file 门槛 | 覆盖率门槛 |
| T3.3 | 实现 conftest 沙箱 | 凭证过滤 + 独立 SQLite |
| T3.4 | 实现 per-file subprocess isolation | 测试隔离 |
| T3.5 | 实现分区模式 | CI 并行 |

#### 阶段 4:进阶测试(2-4 周)

**目标**:属性测试 / Fuzz 测试 / 契约测试 / 性能基准。

| 任务 | 描述 | 产出 |
|------|------|------|
| T4.1 | 集成 `proptest` | 属性测试 |
| T4.2 | 实现协议 wire 属性测试 | Anthropic / OpenAI 序列化 |
| T4.3 | 实现 Yolo 分类器属性测试 | 分类器鲁棒性 |
| T4.4 | 集成 `cargo-fuzz` | Fuzz 测试 |
| T4.5 | 实现 HTTP 解析器 Fuzz 测试 | 解析器鲁棒性 |
| T4.6 | 实现 Multi-Agent 6 角色契约测试 | 架构契约 |
| T4.7 | 集成 `criterion` | 性能基准 |
| T4.8 | 实现 Yolo / 协议 wire / TUI 渲染基准 | 性能基准 |

### 25.3 预期收益

| 指标 | 当前 | 阶段 1 后 | 阶段 2 后 | 阶段 3 后 | 阶段 4 后 |
|------|------|-----------|-----------|-----------|-----------|
| Mock LLM | 0% | 100% | 100% | 100% | 100% |
| 录制回放 | 0% | 80% | 100% | 100% | 100% |
| Eval 框架 | 0% | 0% | 80% | 100% | 100% |
| 覆盖率门槛 | 0% | 0% | 0% | 100% | 100% |
| 属性测试 | 0% | 0% | 0% | 0% | 80% |
| Fuzz 测试 | 0% | 0% | 0% | 0% | 60% |
| 契约测试 | 0% | 0% | 0% | 0% | 80% |
| 性能基准 | 0% | 0% | 0% | 0% | 70% |

---

## 附录 A:15 工程测试目录清单

### A.1 atomcode

```
crates/atomcode-cli/tests
crates/atomcode-coding/tests
crates/atomcode-tuix/tests
crates/atomcode-kernel/tests
crates/atomcode-config/tests
crates/atomcode-telemetry/tests
crates/atomcode-capabilities/tests
crates/atomcode-daemon/tests
evals/deepseek-v4-flash/tests/test_eval.py
extensions/vscode/webview-ui/test
```

### A.2 claudecode

```
src/**tests**/*.test.ts
src/tools/testing/TestingPermissionTool.tsx
src/**/fixtures/**
```

### A.3 deepseek-harness

```
packages/test-support/llm-mock-server
packages/test-support/llm-replay
packages/test-support/agent-loop-testkit
packages/test-support/session-snapshot
packages/test-support/loader-smoke
packages/test-support/client-runtime
packages/*/*/tests/**/*.spec.{ts,tsx}
apps/*/tests/**/*.spec.ts
scripts/**/*.spec.ts
```

### A.4 openclaw

```
test/vitest/ (100+ 套配置)
test/e2e/
test/fixtures/
test/helpers/
test/mocks/
test/contracts/
test/plugins/
test/scripts/
```

### A.5 opencode

```
packages/llm/test/
packages/core/test/
packages/opencode/test/
packages/http-recorder/test/
packages/tui/test/
packages/enterprise/test/
packages/codemode/test/
packages/sdk-next/test/
packages/function/test/
packages/schema/test/
packages/protocol/test/
packages/effect-drizzle-sqlite/test/
packages/httpapi-codegen/test/
packages/console/core/test/
packages/console/app/test/
packages/sdk/js/test/
```

### A.6 pi

```
packages/ai/test/
packages/protocol/test/
packages/coding-agent/test/
packages/server/test/
packages/client/test/
packages/evals/test/
packages/agent/test/
packages/telemetry/test/
packages/tui/test/
packages/session-backends/sqlite-node/test/
```

### A.7 hermes-agent

```
tests/agent/
tests/hermes_cli/
tests/gateway/
tests/run_agent/
tests/cli/
tests/tools/
tests/providers/
tests/plugins/
tests/skills/
tests/integration/
tests/e2e/
tests/conformance/
tests/fakes/
tests/fixtures/
tests/perf_guards/
tests/security/
tests/docker/
tests/desktop/
tests/ui-tui/src/__tests__/
evals/ (20+ 脚本)
```

### A.8 agent-core

```
tests/unit_tests/
tests/system_tests/
openjiuwen/dev_tools/skill_evaluator/skills/skill_tester
```

### A.9 agent-studio

```
backend/tests/
frontend/src/test/
connect/adapters/mcp_server/tests/
frontend/packages/workflow-canvas/src/components/testrun/
```

### A.10 cc-switch

```
tests/components/
tests/config/
tests/hooks/
tests/integration/
tests/lib/
tests/msw/
tests/types/
tests/utils/
src-tauri/tests/
```

### A.11 jiuwenswarm

```
tests/agents/
tests/auth/
tests/integration/
tests/symphony/
tests/system_tests/
tests/ui_e2e/
tests/unit/
tests/unit_tests/
jiuwenswarm/tests/
jiuwenswarm/tests/unit_tests/
jiuwenswarm/tests/system_tests/
jiuwenswarm/extensions/video_duplex/tests/
jiuwenswarm/channels/web/frontend/tests/
jiuwenswarm/channels/tui/frontend/tests/
jiuwenswarm/channels/browser/frontend/tests/
```

### A.12 semantica

```
tests/ (30 个测试文件)
explorer/tests/
```

### A.13 Switchyard

```
tests/e2e/
tests/getting_started/
tests/relay_plugin/
tests/test_aiperf_runner.py
tests/test_cli_reference_docs.py
tests/test_libsy_minimal_bindings.py
tests/test_prepare_harbor_dataset.py
tests/test_routing_performance_report.py
tests/test_run_baseline_script.py
tests/test_run_manifest.py
benchmark/
crates/switchyard-runner/tests/
crates/prefill-router/tests/
crates/switchyard-skill-distillation/tests/
crates/switchyard-translation/tests/
crates/libsy-llm-client/tests/
crates/switchyard-soak/tests/
crates/switchyard-server/tests/
```

### A.14 TencentDB-Agent-Memory

```
(无公开测试目录)
```

### A.15 undici

```
test/autobahn/
test/busboy/
test/cache/
test/cache-interceptor/
test/cookie/
test/fetch/
test/infra/
test/interceptors/
test/jest/
test/node-fetch/
test/node-test/
test/web-platform-tests/
test/websocket/
test/client.js (59KB)
test/client-request.js (42KB)
test/client-stream.js (21KB)
test/client-pipeline.js (25KB)
test/client-pipelining.js (19KB)
test/content-length.js
test/decorator-handler.js
test/errors.js
test/*.js (1732 个文件)
benchmarks/
```

---

## 附录 B:测试框架版本信息

### B.1 TypeScript 工程

| 工程 | vitest | jest | borp | @effect/vitest | vitest-evals |
|------|--------|------|------|----------------|--------------|
| atomcode | - | - | - | - | - |
| claudecode | ✅ | - | - | - | ✅ |
| deepseek-harness | ✅ | - | - | - | - |
| openclaw | ✅ | - | - | - | - |
| opencode | - | - | - | ✅ | - |
| pi | ✅ | - | - | - | ✅ |
| cc-switch | ✅ | - | - | - | - |

### B.2 Python 工程

| 工程 | pytest | coverage.py | pytest-asyncio | pytest-timeout | Hypothesis |
|------|--------|-------------|----------------|----------------|------------|
| hermes-agent | ✅ | ✅ | ✅ | ✅ | - |
| agent-core | ✅ | - | - | - | - |
| agent-studio | ✅ | - | - | - | - |
| jiuwenswarm | ✅ | ✅ | ✅ | - | - |
| semantica | ✅ | - | - | - | - |

### B.3 Rust 工程

| 工程 | cargo test | criterion | proptest | cargo-tarpaulin | cargo-fuzz |
|------|------------|-----------|----------|-----------------|------------|
| atomcode | ✅ | - | - | - | - |
| Switchyard | ✅ | ✅ | - | - | - |

### B.4 Node.js 工程

| 工程 | borp | jest | c8 | fast-check | tsd |
|------|------|------|-----|------------|-----|
| undici | ✅ | ✅ | ✅ | ✅ | ✅ |

---

## 附录 C:CI 配置清单

### C.1 GitHub Actions

| 工程 | 配置文件 | 覆盖率 | 并行度 | 重试 |
|------|----------|--------|--------|------|
| atomcode | `.github/workflows/build.yml` | ❌ | ❌ | ❌ |
| claudecode | ✅ | ⚠️ 部分 | ❌ | ❌ |
| openclaw | ✅ | ✅ 100% | ✅ | ✅ |
| opencode | ✅ | ⚠️ 部分 | ✅ | ❌ |
| pi | ✅ | ⚠️ 部分 | ❌ | ❌ |
| hermes-agent | ✅ | ✅ | ✅ per-file | ❌ |
| cc-switch | ✅ | ⚠️ text+lcov | ❌ | ❌ |
| jiuwenswarm | ✅ | ✅ term-missing+xml | ❌ | ❌ |
| semantica | ✅ | ❌ | ❌ | ❌ |
| Switchyard | ✅ | ❌ | ❌ | ❌ |
| TencentDB-Agent-Memory | ⚠️ | ❌ | ❌ | ❌ |
| undici | ✅ | ✅ c8 100% | ✅ | ❌ |

### C.2 GitLab CI

| 工程 | 配置文件 | 用途 |
|------|----------|------|
| deepseek-harness | `.gitlab-ci.yml` | Python SDK 发布 |

---

## 附录 D:Eval 脚本清单

### D.1 hermes-agent evals/

```
evals/anthropic_proxy_thinking_replay.py
evals/auxiliary_resource_exhausted.py
evals/cli_deferred_notice.py
evals/cli_fallback_add_picker_error.py
evals/codex_masked_replay_review.py
evals/fanout_resource_bench.py
evals/slack_stream_wire_contract.py
evals/compaction/
evals/core_tool_deferral/
evals/postmortem/
evals/token_accounting/
evals/browser_use/
evals/codebase_navigability/
evals/gateway_status_render/
evals/native_compaction/
evals/readtool/
evals/session_search_schema/
evals/webhook_auth/
```

### D.2 atomcode evals/

```
evals/deepseek-v4-flash/eval.py
evals/deepseek-v4-flash/cases/agent-cases.json
evals/deepseek-v4-flash/cases/model-cases.json
evals/deepseek-v4-flash/prompts/codex-report.md
evals/deepseek-v4-flash/prompts/codex-judge.md
evals/deepseek-v4-flash/cases/agent-fixture/ (12 个 fixture)
```

### D.3 Switchyard benchmark/

```
benchmark/run-baseline.sh
benchmark/prepare_harbor_dataset.py
benchmark/run_manifest.py
benchmark/server-configs/ (TOML 配置)
benchmark/routing-profiles/ (路由配置)
benchmark/patches/ (Harbor 补丁)
benchmark/DATASETS.md
benchmark/README.md
```

---

## 附录 E:术语表

| 术语 | 定义 |
|------|------|
| **Mock LLM** | 模拟 LLM 响应的服务,支持 deterministic / scripted / fault injection |
| **录制回放** | 录制真实 LLM 响应到 fixture,后续测试回放 fixture |
| **Eval 框架** | 评估 Agent 输出的框架,支持 pairwise / LLM-as-judge |
| **Pairwise** | 两个候选版本的对比测试 |
| **LLM-as-judge** | 使用 LLM 作为评委评估输出质量 |
| **Golden Dataset** | 标准评测数据集 |
| **Lift** | candidate pass rate - baseline pass rate |
| **Repetitions** | 重复测试次数,降低随机性 |
| **Rubric** | 评分维度 + 评分标准说明 |
| **Redaction** | 脱敏处理,移除敏感信息 |
| **Cassette** | 录制回放的 fixture 文件 |
| **Override 侧车** | 覆盖录制回放的补充配置 |
| **assertConsumed** | 验证所有 script 被消费 |
| **per-file isolation** | 每个测试文件独立进程 |
| **conftest 沙箱** | pytest 测试前的环境隔离 |
| **100% per-file** | 每个文件独立覆盖率门槛 |
| **属性测试** | 验证代码的通用属性 |
| **Fuzz 测试** | 随机输入测试鲁棒性 |
| **契约测试** | 验证模块间的契约 |
| **Harbor** | Terminal-Bench / SWE-Bench 评测框架 |
| **TBLite** | Terminal-Bench Lite 数据集 |
| **SWE-Bench** | Software Engineering Benchmark |

---

## 附录 F:laew 测试改造检查清单

### F.1 阶段 1 检查清单

- [ ] T1.1 集成 `mockito` / `wiremock-rs`
- [ ] T1.2 实现 20+ 故障模式
- [ ] T1.3 实现脚本化行为序列
- [ ] T1.4 实现录制回放框架
- [ ] T1.5 实现 assertConsumed

### F.2 阶段 2 检查清单

- [ ] T2.1 创建 Yolo / Main-Work / SubAgent-Work 三套 harness
- [ ] T2.2 实现 pairwise comparison
- [ ] T2.3 实现 repetitions + lift
- [ ] T2.4 实现 LLM-as-judge
- [ ] T2.5 建立 golden dataset

### F.3 阶段 3 检查清单

- [ ] T3.1 集成 `cargo-tarpaulin`
- [ ] T3.2 实现 100% per-file 门槛
- [ ] T3.3 实现 conftest 沙箱
- [ ] T3.4 实现 per-file subprocess isolation
- [ ] T3.5 实现分区模式

### F.4 阶段 4 检查清单

- [ ] T4.1 集成 `proptest`
- [ ] T4.2 实现协议 wire 属性测试
- [ ] T4.3 实现 Yolo 分类器属性测试
- [ ] T4.4 集成 `cargo-fuzz`
- [ ] T4.5 实现 HTTP 解析器 Fuzz 测试
- [ ] T4.6 实现 Multi-Agent 6 角色契约测试
- [ ] T4.7 集成 `criterion`
- [ ] T4.8 实现 Yolo / 协议 wire / TUI 渲染基准

---

## 结语

本报告对 15 个 Agent 源码工程的测试体系、Eval 基建、Mock LLM、录制回放、测试金字塔、E2E 测试、单元测试、集成测试、视觉回归测试、性能基准进行了全面对比分析,产出了 4 张核心对比表(Mock LLM / 录制回放 / Eval 框架 / CI 集成)与 20 个新 gap(L206-L225)。

**核心发现**:

1. **deepseek-harness** 是 15 工程中测试体系最完整的项目,其 dsh-llm-mock-server(20+ 故障模式 + 随机模式)与 dsh-llm-replay(JSONL 投影 + override 侧车 + assertConsumed)是 Mock LLM 与录制回放的最佳实践。

2. **opencode** 的 http-recorder(Cassette + redaction + CI 模式)与 perf/test-suite.md(假设-验证-决策循环)是录制回放与性能优化的最佳实践。

3. **pi** 的 vitest-evals(evalHarnessTable + repetitions + lift)是 Eval 框架的最佳实践。

4. **hermes-agent** 的 17k tests + per-file subprocess isolation + conftest 沙箱是测试规模与隔离的最佳实践。

5. **undici** 的 borp + jest + c8 + fast-check 是 Node.js 测试的最佳实践。

6. **Switchyard** 的 benchmark/run-baseline.sh(Harbor + Docker + Prometheus)是性能基准的最佳实践。

**laew 改造建议**:

- **P0 紧急**:L206 / L207 / L208 / L210(Mock LLM + 录制回放 + pairwise)。
- **P1 重要**:L209 / L211 / L212 / L213 / L215 / L216 / L220 / L224(redaction + LLM-as-judge + golden dataset + 覆盖率 + CI + 属性测试 + 契约测试)。
- **P2 进阶**:L214 / L217 / L218 / L219 / L221 / L222 / L223 / L225(分区 + 性能基准 + Fuzz + 契约)。

**推荐 Rust crate**:

- Mock LLM:`mockito` / `wiremock-rs` / `httptest`
- 录制回放:`vcr-rs` / `insta`
- Eval 框架:`criterion` / `proptest` / `quickcheck`
- Fuzz 测试:`cargo-fuzz` / `afl`
- 覆盖率:`cargo-tarpaulin` / `cargo-llvm-cov`
- 契约测试:`pact-rs`
- 测试工具:`tempfile` / `serial_test` / `test-log` / `pretty_assertions`

通过 4 阶段改造,laew 可以从"零测试体系"升级到"生产级 Agent CLI 测试体系",覆盖 Mock LLM / 录制回放 / Eval 框架 / 覆盖率 / 属性测试 / Fuzz 测试 / 契约测试 / 性能基准 8 大维度。

---

**报告完成日期**:2026-09-07

**报告作者**:第十一轮深度调研 - 测试体系与 Eval 基建与录制回放专项调研员

**累计 gap 数**:162 个(L1-L162)

**本轮新增 gap 数**:20 个(L206-L225)

**推荐 Rust crate 数**:16 个

**改造阶段数**:4 阶段(预计 6-11 周)

---