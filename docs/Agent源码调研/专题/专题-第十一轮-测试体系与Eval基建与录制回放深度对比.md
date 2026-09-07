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

## 第 9 章 hermes-agent(Python 测试之王)