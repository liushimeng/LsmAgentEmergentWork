# 专题-第三轮:测试体系与Eval基建深度分析

> 横向维度:测试分层 / Mock LLM 策略 / Eval 框架 / TUI 终端测试 / Snapshot 与 Fixture / CI 组织 / Git Hooks / 测试工具链
> 分析对象:pi / atomcode / opencode / deepseek-harness / openclaw / claudecode + laew(对照)
> 产出日期:2026-09-05

---

## 0. 摘要与阅读导引

本专题是知识库首个聚焦「测试体系与 Eval 基建」的横向深度分析。此前各文档对测试只是顺带一提(架构/工具/记忆等维度下的片段),本专题把测试当作**一等公民**来剖析。

**为什么 laew 工程自身价值极高**:laew 已有 `testReport/run_e2e.sh`(548 行)+ Python mock LLM 服务 + tmux 真 PTY 子屏自动化,这是一笔扎实的资产。但横向对比揭示它**缺什么**:单测覆盖近乎空白、无 Eval 框架、无 snapshot 体系、无 git hooks 质量门、无 CI。本报告最后一节给出 P0/P1/P2 借鉴路线。

**分析对象速览**:

| 仓库 | 语言 | 测试文件数 | 测试基建密度 | Eval 框架 | 核心测试工具 |
|------|------|-----------|-------------|-----------|-------------|
| pi | TS | ~478 | 中 | 自研 vitest-evals | vitest + 自研 harness |
| atomcode | Rust | ~30(单测)+evals | 低 | 自研 eval.py(stdlib) | cargo test + Python eval |
| opencode | TS/Bun | ~673 | 中 | 无独立 eval | bun test + turbo + Playwright |
| deepseek-harness | TS | ~906 | **极高** | 无独立 eval | vitest 矩阵 + 4 款 test-support |
| openclaw | TS | **~7725** | **极高** | 无独立 eval | vitest 119 配置 + pre-commit |
| claudecode | TS | **0** | 零(纯文档仓) | 无 | 无 |
| **laew(对照)** | **Rust** | **0 单测** | **e2e only** | **无** | **bash + tmux + Python mock** |

> 注:claudecode 仓库实为架构文档仓(199 个 src 子目录 + 大量 .svg/.jpg 架构图),不含可运行测试,本专题主要作为「零测试」对照。

**阅读导引**:
- §1 各仓逐个深挖(7 节,含代码片段与文件锚点)
- §2 横向对比总表(8 维度 × 7 仓)
- §3 可复用设计模式(8 个,均配真实代码出处)
- §4 laew 借鉴路线(P0/P1/P2 + 6 周落地计划)

---

## 1. 各仓逐个深挖

### 1.1 pi — vitest-evals 驱动的模型行为评估

**仓库路径**:`/usr/local/LsmGitOpenSource/pi`

#### 1.1.1 测试分层

pi 的测试分三层,清晰分离:

| 层 | 入口 | 位置 |
|----|------|------|
| 单元 | `npm test` → vitest | `packages/*/tests/**` 与 `packages/*/src/**` co-located |
| 集成(e2e) | `test.sh`(hermetic) | 全仓,隔离 `$HOME` |
| Eval | `npm run eval` → `packages/evals/` | 独立 workspace 包 |

关键设计:`test.sh` 用 `env -i` 启动**完全空白的环境**,仅注入白名单变量(`PATH`/`HOME`/`TMPDIR`/`GIT_CONFIG_NOSYSTEM=1` 等),并在临时目录上打 `.pi-test-owned` 标记,cleanup 时校验所有权后才删除(`test.sh:22-45`)。这是「hermetic test」的范本。

vitest 配置采用**分层继承**:`vitest.base.ts` 定义 workspace 路径别名(把 `@earendil-works/pi-ai` 指向源码而非构建产物),各包 `vitest.config.ts` 通过 `mergeConfig` 继承(`packages/evals/vitest.config.ts:1-16`)。

#### 1.1.2 Mock LLM 策略

pi **不 mock LLM** —— 它的策略是**直连真实 provider**,通过环境变量注入 key。`pi-test.sh` 甚至提供 `--no-env` 标志一键清空所有 provider key(`pi-test.sh:14-52`):

```bash
unset ANTHROPIC_API_KEY
unset ANTHROPIC_OAUTH_TOKEN
unset OPENAI_API_KEY
unset GEMINI_API_KEY
# ... 共 30+ 个 key
```

这意味着 pi 的测试默认**需要真实 key**,无 key 时大量 suite 自跳过。这是「模型即测试基础设施」的哲学 —— 与 laew 的 mock-first 形成鲜明对比。

#### 1.1.3 Eval 框架(核心亮点)

pi 是唯一拥有**正式 Eval 框架**的仓库。`packages/evals/` 是一个独立 workspace 包,架构如下:

```
packages/evals/
├── src/
│   ├── pi-harness.ts          # 核心:AgentSession → vitest-evals 适配
│   ├── smoke.eval.ts          # 冒烟 eval
│   ├── extensions.eval.ts     # 扩展编写 eval(含 Judge)
│   └── vitest-evals/          # 自研 eval 基础设施
│       ├── setup.ts
│       ├── reporter.ts
│       ├── summary.ts
│       ├── artifacts.ts
│       └── harness-table.ts   # 多 harness 对比表
├── test/                      # eval 框架自身的单测
│   └── pi-harness.test.ts
└── scripts/run-evals.mjs      # 入口脚本
```

**pi-harness.ts 核心机制**(`packages/evals/src/pi-harness.ts`):

`createPiCodingAgentHarness(options)` 返回一个 `vitest-evals` 的 `Harness`,其 `run()` 方法:

1. `mkdtemp(join(tmpdir(), "pi-eval-"))` 创建隔离工作区 + agent 目录
2. 通过 `ModelRuntime.create()` 加载真实 provider
3. `createAgentSessionServices()` + `createAgentSessionFromServices()` 构造真实 AgentSession
4. 支持 `transformSystemPrompt` 注入 prompt 变体
5. 支持 `output` 回调把 session 转为 JSON-safe 领域结果
6. 运行完毕把 session JSONL 快照为 artifact,`rm -rf` 清理

**真实 eval case 结构**(`packages/evals/src/smoke.eval.ts:7-12`):

```ts
describeEval("Pi Coding Agent smoke", { harness: piCodingAgentHarness }, (it) => {
    it("runs a basic prompt end to end", async ({ run }) => {
        const result = await run("What's the capital of France? Respond with only the city name.");
        expect(result.output.trim()).toBe("Paris");
    });
});
```

**Judge 机制**(`packages/evals/src/extensions.eval.ts:46-82`):`createJudge<PiCodingAgentInput, ExtensionAuthoringOutput>()` 返回评分函数,输出 `{score, metadata:{rationale}}`。Judge 检查扩展源码是否导入规范包、是否注册了 `hello` 工具、是否成功调用并返回 `"Hello, Bob!"`。

**对比 eval**(`extensions.eval.ts` 后半):`evalHarnessTable()` + `describe.for()` 实现**多 harness 同输入对比**,重复 N 次,计算 pass-rate lift(候选通过率 - 基线通过率)。这是**A/B 测试式 eval** 的完整实现。

**runner**(`scripts/run-evals.mjs`):解析 `--provider`/`--model` 或 `PI_PROVIDER`/`PI_MODEL` 环境变量,创建带时间戳 + UUID 的 `.eval/` 产物目录,spawn vitest 子进程。

#### 1.1.4 测试工具链

- **runner**:vitest(单测) + vitest-evals(Eval)
- **环境隔离**:`test.sh` 的 `env -i` + 临时 `$HOME`
- **CI**:`.github/workflows/ci.yml` + `pr-gate.yml`
- **hooks**:`.husky`(目录存在,内容未展开)

#### 1.1.5 关键数字

- 测试文件:~478(`find packages -name "*.test.ts"` )
- eval case:smoke 1 + extensions 若干
- 贡献者门槛:`CONTRIBUTING.md` 要求 `./test.sh` 必须通过

---

### 1.2 atomcode — Rust 单测 + Python stdlib-only Eval Harness

**仓库路径**:`/usr/local/LsmGitOpenSource/atomcode`

#### 1.2.1 测试分层(Rust 惯例)

| 层 | 机制 | 位置 |
|----|------|------|
| 单元 | `#[cfg(test)] mod tests`(inline) | 各 crate `src/*.rs` 内部 |
| 集成 | `tests/*.rs`(black-box) | 各 crate 下 `tests/` 目录 |
| Eval | `evals/deepseek-v4-flash/eval.py` | 独立 Python 脚本 |

atomcode 的 Rust 单测采用**双轨制**:
- **内联模块**:`crates/atomcode-coding/src/runtime.rs` 等含 `#[cfg(test)] mod tests`,用 `grep -rln "mod tests" crates --include="*.rs"` 找到 ~20+ 个内联测试模块
- **独立集成测试**:`crates/atomcode-coding/tests/` 下有 `verify_cadence.rs`/`sensitive_path.rs`/`overflow_recovery.rs`/`team_runtime.rs`/`system_context.rs`/`tool_args_repair.rs`/`full_assembly.rs`/`plan_mode.rs` 等

这是 Rust 生态的标准做法,无特别之处。**真正独特的是 Eval 层**。

#### 1.2.2 Eval 框架(核心亮点)

`evals/deepseek-v4-flash/` 是一个**纯 Python 3.7+ stdlib** 的成对模型评估工具链,零外部依赖:

```
evals/deepseek-v4-flash/
├── eval.py              # 627 行核心
├── benchmark.json       # 候选模型声明
├── cases/
│   ├── model-cases.json # 20 个模型级 case
│   ├── agent-cases.json # 8 个 agent 级 case
│   └── agent-fixture/   # 可编辑 fixture(真实 Python 项目)
│       ├── app.py / legacy.py / service.py / cache.py ...
│       └── tests/       # 验证测试
├── prompts/
│   ├── codex-report.md  # Codex 报告 prompt
│   └── codex-judge.md   # Codex 评判 prompt
└── tests/test_eval.py   # eval 框架自身的 unittest
```

**eval.py 核心数据模型**(`evals/deepseek-v4-flash/eval.py:33-55`):

```python
@dataclass(frozen=True)
class Candidate:
    name: str
    selection: str
    expected_model: str

@dataclass(frozen=True)
class Case:
    id: str
    tier str
    directory: Path
    prompt: str
    timeout: int
    allow_edits: bool
    fixture: Path | None
    verify: tuple[str, ...]
    rubric: dict[str, str]
```

**真实 model-cases.json 样例**(`evals/deepseek-v4-flash/cases/model-cases.json`):

```json
{"id":"debug-rust-lifetime",
 "prompt":"Diagnose this Rust error and give the smallest compiling fix...",
 "rubric":{"correctness":"Explains y needs lifetime 'a...","quality":"No clone, allocation, or unsafe."}}
```

**真实 agent-cases.json 样例**(`evals/deepseek-v4-flash/cases/agent-cases.json`):

```json
{"id":"agent-fix-cache",
 "fixture":"agent-fixture",
 "verify":["python3","-m","unittest","tests.test_cache","-v"],
 "prompt":"Fix the TTL cache bug exposed by tests.test_cache...",
 "rubric":{"correctness":"Target test passes; expiration uses monotonic time..."}}
```

**四步工作流**(README):

```bash
python3 eval.py prepare                    # 生成 manifest.json(schedule + run_id)
python3 eval.py run --run-dir results/x    # 并发跑候选对
python3 eval.py judge --run-dir results/x  # LLM-as-judge(Codex)
python3 eval.py report --run-dir results/x # 生成 report.md
```

**关键机制**:
- **成对并发**:`Suite.pair_concurrency` 控制,候选对**同时启动**,不共享 session 或 fixture(`test_eval.py:test_pair_runs_concurrently_and_isolates_homes` 验证 `elapsed < summed` 证明重叠)
- **HOME 隔离**:每个候选获得独立 `ATOMCODE_HOME`
- **脱敏**:`scrub()` 用正则 `SECRET_RE = re.compile(r"(?i)(authorization\s*[:=]\s*(?:bearer\s+)?|api[_-]?key\s*[:=]\s*|token\s*[:=]\s*)([^\s\"']+)")` 替换凭证为 `[REDACTED]`
- **token 解析**:`TOKEN_RE = re.compile(r"\[tokens\]\s+prompt=(\d+)\s+completion=(\d+)\s+cached=(\d+)")` 从 stdout 提取用量
- **JSONL 分类**:`classify_jsonl()` 把结果分为 `success`/`empty_output`/`protocol_error` 等
- **Judge 校验**:`validate_judgment()` 校验分数范围(0-100)、必填字段

**eval 框架自身的单测**(`evals/deepseek-v4-flash/tests/test_eval.py`):用 `unittest` + `importlib.util.spec_from_file_location` 动态加载 `eval.py`,测试 suite/case 加载、scrub 脱敏、百分位插值、token 解析、JSONL 分类、judgment 校验、prepare 确定性(schedule 相同但 run_id 不同)。

#### 1.2.3 Mock LLM 策略

atomcode **不 mock** —— eval.py 直接调用真实 `atomcode` 二进制 + 真实 `codex` 做 judge。这是「全真实」策略,代价是必须消耗 API 配额。

#### 1.2.4 关键数字

- Rust 单测:内联 ~20+ 模块 + 集成测试 ~15+
- Eval case:model-tier 20 + agent-tier 8 = 28
- eval.py:627 行,零外部依赖

---

### 1.3 opencode — Bun test + turbo + Playwright 三件套

**仓库路径**:`/usr/local/LsmGitOpenSource/opencode`

#### 1.3.1 测试分层

| 层 | 入口 | 位置 |
|----|------|------|
| 单元 | `bun test` | `packages/*/test/*.test.ts`(co-located) |
| 集成 | `bun test` | `packages/client/test/` 等 |
| e2e | `bun --cwd packages/app test:e2e:local` | `packages/app/test-browser/` |
| 录制回放 | `bun test`(http-recorder) | `packages/http-recorder/test/` |

opencode 的测试组织是**co-located**:测试文件紧贴被测源码,如 `packages/tui/test/index.test.tsx` 对应 `packages/tui/src/`。

**turbo.json 编排**(`turbo.json`):

```json
"opencode#test": { "dependsOn": ["^build"], "passThroughEnv": ["*"] },
"@opencode-ai/function#test": { "outputs": [] }
```

测试任务依赖上游包构建,`passThroughEnv: ["*"]` 让环境变量透传(测试常需 key)。

#### 1.3.2 Mock LLM 策略

opencode 的 TUI 测试使用 **`@opentui/core/testing`** 的 `createTestRenderer` 模拟终端(`packages/tui/test/app-lifecycle.test.tsx:12`):

```ts
const setup = await createTestRenderer({ width: 80, height: 24, useThread: false })
mock.module("@opentui/core", () => ({ ...core, createCliRenderer: async () => setup.renderer }))
```

这是**渲染层 mock** —— 不启动真实 PTY,而是用内存渲染器捕获 `setTerminalTitle` 调用、`isDestroyed` 状态等。

**http-recorder**(`packages/http-recorder/test/record-replay.test.ts`)提供**磁带录制回放**:

```ts
const seedCassetteDirectory = (directory, name, interactions) =>
    Effect.runPromise(
        Effect.gen(function* () {
            const cassette = yield* HttpRecorderInternal.Cassette.Service
            yield* Effect.forEach(interactions, (interaction) => cassette.append(name, interaction))
        }).pipe(Effect.provide(HttpRecorderInternal.Cassette.fileSystem({ directory })))
    )
```

HTTP 交互被序列化为 cassette 文件,后续测试从磁带回放,无需真实网络。这是 opencode 的**核心 mock 策略**。

#### 1.3.3 TUI 终端测试

opencode 的 TUI 测试是**非 PTY 的渲染器 mock**,与 laew 的 tmux 真 PTY 形成互补:

- **优点**:快速、CI 友好、可断言内部状态(`isDestroyed`/`setTerminalTitle`)
- **局限**:不验证真实终端渲染(ANSI 转义、光标定位、alternate screen)

#### 1.3.4 CI 组织

`.github/workflows/test.yml` 定义 **unit + e2e 双轨矩阵**:

- **unit**:`GITHUB_ACTIONS=false bun turbo test`,linux + windows 双平台,20 分钟超时
- **e2e**:Playwright Chromium,缓存浏览器,30 分钟超时,`always()` 上传 test-results 产物

`concurrency` 组用 `case()` 表达式区分 dev 分支(按 run_id 隔离)和 PR(按 PR 号共享)。

#### 1.3.5 关键数字

- 测试文件:~673(`find packages -name "*.test.ts"` )
- 包数:32(`ls packages/ | wc -l`)
- 工具链:bun test + turbo + Playwright + oxlint + husky

---

### 1.4 deepseek-harness — vitest 矩阵 + 4 款 test-support 子包(测试基建密度最高)

**仓库路径**:`/usr/local/LsmGitOpenSource/deepseek-harness`

这是本专题中**测试基建最厚重**的仓库,其 `docs/testing.md` 是一份**纲领性测试政策文档**,值得细读。

#### 1.4.1 测试分层(六层体系)

`docs/testing.md` 明确定义六层:

| 层 | 命令 | 说明 |
|----|------|------|
| **Unit** | `pnpm run test` | vitest 覆盖 `packages/*/*/tests/**` + `scripts/**/*.spec.ts` |
| **Coverage gate** | `pnpm run test:coverage` | 文件级 100% 覆盖率门 |
| **Real-API e2e** | `pnpm run test:e2e` | 真实 provider key,无 key 自跳过 |
| **Expected output** | `pnpm run test:expected` | 无录制会话的组装期望输出 |
| **Snapshot** | `pnpm run test:snapshot` | 录制会话回放 + 期望对比 |
| **Web browser** | `pnpm run test:web` | Chromium 渲染对比 |

**核心测试哲学**(`docs/testing.md`):

> "Mock only the expensive or non-deterministic boundary (LLM adapter, network, clock); keep everything downstream real."

> "An e2e assertion re-runs the command or re-reads the file externally; a keyword probe on the agent's own output lets a cheating agent pass."

> "Test the real entry path: a package `bin` runs built `lib/bin.js` under plain `node`, exposing failures tsx masks."

这三条原则是 deepseek 测试体系的**精神内核**。

#### 1.4.2 Mock LLM 策略(核心亮点 — 4 款 test-support 子包)

deepseek 有**四款独立发布的测试支持包**,这是本专题独有的:

**A. llm-mock-server**(`packages/test-support/llm-mock-server/`)

一个**可脚本化的 OpenAI 兼容故障服务器**,FIFO 行为队列,每个 `/chat/completions` 请求消耗一个行为:

```
--sequence partial_disconnect,success
```

支持的行为:`connection_reset`/`stream_disconnect`/`partial_disconnect`/`stall`/`empty`/`malformed_json`/`rate_limit`/`server_error`/`success`/`tool_call_success`/`random` 等 17 种。`random` 模式用**种子化 PRNG**(`--seed 42`)实现可复现的混合故障压力测试。

**B. llm-replay**(`packages/test-support/llm-replay/`)

**无 key 的录制回放适配器**:从 `session.jsonl` fixture 重建模型流,让真实 agent 在固定 transcript 上运行。核心机制:

- `deriveReplayScript` 从 `assistant/chunk` 事件推导 chunk 序列
- `replay.override.json` sidecar 表达纯 throw/cancel/hang(日志无法重建的场景)
- **首次调用顺序绑定**:父 session 先于子 session 绑定脚本
- `assertConsumed()` 在 teardown 验证所有录制脚本被消费

**C. agent-loop-testkit**(`packages/test-support/agent-loop-testkit/`)

**AgentLoop 测试依赖挂载器**:`mountAgentLoopTestDependencies(ctx)` 按固定顺序挂载五个服务插件 —— LLM、session、system-prompt、tool registry、agent registry —— 在 `AgentLoop` 之前停止,让调用方控制 loop 加载顺序。

**D. session-snapshot**(`packages/test-support/session-snapshot/`)

**无 key 录制会话测试的核心支撑**,提供:

- 封闭 `snapshot.yml` manifest
- 类型化身份脱敏(typed identity redaction)
- 纯 normalizer(路径 → `{{cwd}}`,id → 首次出现序列,时间归零)
- workspace 完整状态比较
- headless/SDK/ACP/Web 四个 profile 适配器

**snapshot case 示例**(`snapshots/session/` 目录):

```
snapshots/session/
├── text-turn/           # 基础文本轮
├── fs-write/            # 文件写入
├── fs-edit/             # 文件编辑
├── subagent-multi/      # 多子 agent
├── compaction-recovery/ # 压缩恢复
├── hook-cc-pretool-ask/ # Hook 交互
├── parallel-tool-calls/ # 并行工具调用
└── ... (60+ 场景)
```

每个场景一个 `snapshot.yml`(`snapshots/session/skill-load/snapshot.yml`):

```yaml
version: 1
scenario: skill-tool-row
profile: web
composition: default
recording: authored
header:
  class: skill
session:
  source: ../../session/skill-load/session.jsonl
```

#### 1.4.3 Snapshot 与 Fixture 管理

deepseek 的 snapshot 体系是**record/replay/refresh 三态**:

| 模式 | 命令 | 行为 |
|------|------|------|
| record | `DSH_SNAPSHOT=record vitest ... --update` | 调真实 LLM,重写 fixture |
| replay | `DSH_SNAPSHOT=replay vitest ...` | 回放对比,**CI 强制** |
| refresh | `DSH_SNAPSHOT=refresh vitest ...` | 重写期望输出,不改输入 |

**fixture 格式**:保留 header 和事件 payload,省略 `seq`/`time` 信封(回放时合成)。使用**规范 packed 行**,`migrate-packed-session-fixtures.ts` 迁移旧布局。

**并发控制**(`vitest.snapshot.config.ts`):

```ts
fileParallelism: (process.env.DSH_SNAPSHOT || 'replay') === 'replay' && snapshotMaxConcurrency > 1,
maxConcurrency: snapshotMaxConcurrency,  // 默认 min(5, availableParallelism())
```

replay 并行(无写冲突),record/refresh 串行(避免并发写 corrupt)。

#### 1.4.4 CI 组织

**GitHub Actions** + **GitLab CI** 双系统:

- `.github/workflows/ci-master.yml`:master 推送时运行 **self-hosted 64 核 VM 完整串行 CI**(`pnpm run check:ci:linux-primary`),作为热备演练
- `.gitlab-ci.yml`:Python SDK wheel 构建 + 多平台 smoke(`linux-x64`/`linux-arm64`/`macos-arm64`/`windows-x64`)
- `e2b-e2e.yml`:真实沙箱 e2e

**gate 脚本**(`scripts/run-gates.ts`):`check-all`/`ci-primary`/`ci-linux-primary`/`ci-static`/`ci-lint-contracts-ready`/`ci-coverage`/`ci-snapshot` 多级门。

#### 1.4.5 关键数字

- 测试文件:~906(`find packages apps scripts -name "*.spec.ts"` )
- vitest 配置:10+(shared/config/e2e/expected/snapshot/web/web-perf/web-stress)
- snapshot 场景:60+
- test-support 子包:4(llm-mock-server/llm-replay/agent-loop-testkit/session-snapshot)

---

### 1.5 openclaw — 7725 测试文件 + 119 vitest 配置 + pre-commit 矩阵

**仓库路径**:`/usr/local/LsmGitOpenSource/openclaw`

这是本专题中**测试文件数最大**的仓库,且拥有最完整的**本地质量门**体系。

#### 1.5.1 测试分层

| 层 | 位置 | 命名约定 |
|----|------|----------|
| 单元 | `src/**` co-located | `*.test.ts` |
| 集成 | `test/` 根目录 | `*.integration.test.ts` |
| e2e | `test/` 根目录 | `*.e2e.test.ts` |
| 契约 | `test/` 根目录 | `*.test-support.ts`(共享 fixture) |

**命名约定严格**:`cli-json-stdout.e2e.test.ts` / `cli-json-stdout.test-support.ts` 成对出现 —— `.test-support.ts` 是共享 helper,不是测试本身(避免 `describe` 重复注册)。

#### 1.5.2 Mock LLM 策略

`test/setup.shared.ts` 统一 mock 外部依赖:

```ts
vi.mock("../src/llm/oauth.js", () => ({
  getOAuthApiKey: () => undefined,
  getOAuthProviders: () => [],
  loginOpenAICodex: vi.fn(),
  refreshOpenAICodexToken: vi.fn(),
}));
vi.mock("@mariozechner/clipboard", () => ({
  availableFormats: () => [], getText: async () => "", setText: async () => {}, ...
}));
```

**e2e 测试启动真实二进制**(`test/cli-json-stdout.test-support.ts`):

```ts
export function runBuiltCli(tempHome, args, envOverrides, options) {
  const env = { ...process.env, HOME: tempHome, USERPROFILE: tempHome, OPENCLAW_TEST_FAST: "1" };
  delete env.OPENCLAW_HOME; delete env.OPENCLAW_STATE_DIR;
  const entry = path.resolve(process.cwd(), "openclaw.mjs");
  return spawnSync(process.execPath, [entry, ...args], { cwd: process.cwd(), env, encoding: "utf8",
    maxBuffer: 10 * 1024 * 1024, timeout: 60_000 });
}
```

这是**真实入口测试** —— 启动编译后的 `openclaw.mjs`,验证 CLI 行为。

#### 1.5.3 测试环境隔离

`test/test-env.ts` 提供**两种模式**:

- `mode: "live-aware"`:保留真实 `$HOME` 的部分凭证
- `mode: "hermetic"`:完全隔离,清空 `ISOLATED_TEST_CREDENTIAL_ENV_KEYS`(TELEGRAM_BOT_TOKEN/DISCORD_BOT_TOKEN/SLACK_BOT_TOKEN/GITHUB_TOKEN 等 13 个)

`withStateDirEnv(prefix, fn)` 是核心 helper —— 创建临时 state 目录,`vi.spyOn(os, "tmpdir")` 劫持,执行 fn,最终清理(`test/test-env.state-lifetime.test.ts:30-45`)。

#### 1.5.4 Git Hooks 与本地质量门(核心亮点)

openclaw 拥有本专题**最完整的本地质量门**:

**git-hooks/pre-commit**(`git-hooks/pre-commit`):

```bash
#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(git rev-parse --show-toplevel)"
cd "$ROOT_DIR"
exec node scripts/pre-commit/guard-staged-content.mjs
```

**.pre-commit-config.yaml**(prek/pre-commit 框架):

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks  # trailing-whitespace/end-file-fixer/check-yaml
  - repo: https://github.com/koalaman/shellcheck-precommit # shell 脚本 lint
  - repo: https://github.com/rhysd/actionlint                # GitHub Actions lint
  - repo: https://github.com/zizmorcore/zizmor-pre-commit   # Actions 安全审计
  - repo: https://github.com/astral-sh/ruff-pre-commit       # Python skills lint
  - repo: local
    hooks:
      - id: detect-private-key     # 私钥扫描
      - id: pnpm-audit-prod        # 依赖审计
      - id: oxlint                 # TS/JS lint(type-aware)
      - id: oxfmt                  # 格式检查
      - id: swiftlint              # Swift lint
      - id: swiftformat            # Swift 格式
      - id: skills-python-tests    # skills 目录 pytest
```

**11 个 hook**,覆盖文件卫生、shell、CI 配置、安全审计、多语言 lint、依赖审计、skills 测试。这是**本地质量门的工业级范本**。

#### 1.5.5 测试工具链

- **runner**:vitest(119 个配置文件在 `test/vitest/`)
- **环境**:`test/setup.shared.ts` + `test/test-env.ts`
- **隔离**:`withIsolatedTestHome` + `withStateDirEnv`
- **CI**:`.github/workflows/`(20+ workflow)
- **hooks**:prek + 11 hook

#### 1.5.6 关键数字

- 测试文件:**~7725**(全仓 `find test src -name "*.test.ts"` )
- vitest 配置:**119**(`ls test/vitest/*.config.ts | wc -l`)
- test-helpers 目录:10+(src/test-helpers, src/logging/test-helpers, src/agents/test-helpers 等)
- hooks:11 个

---

### 1.6 claudecode — 零测试的纯文档仓(对照)

**仓库路径**:`usr/local/LsmGitOpenSource/claudecode`

claudecode 仓库**不含可运行测试**:

- `find src -name "*.test.ts"` → 0 结果
- `src/tools/testing/` 仅有一个 `TestingPermissionTool.tsx`(测试辅助工具,非测试本身)
- 无 `package.json` scripts
- 仓库主体是架构图(.svg/.jpg/.mmd)和文档

**结论**:claudecode 是**架构文档仓**,不是可测试代码仓。本专题将其作为「零测试」对照,提醒我们**测试覆盖率可以是零** —— 这并不妨碍它成为有价值的知识源,但意味着无法从它借鉴测试实践。

---

### 1.7 laew(对照) — bash + tmux + Python mock 的 e2e 体系

**仓库路径**:`/usr/local/LsmGitOpenSource/LsmAgentEmergentWork`

#### 1.7.1 现有资产

laew 的测试体系**全部集中在 e2e 层**:

```
testReport/
├── run_e2e.sh           # 548 行 bash,主入口
└── e2e-<时间戳>.txt     # 输出报告
scripts/
└── mock_llm_server.py   # Python stdlib mock LLM 服务
```

**run_e2e.sh 结构**:

| 节 | 内容 |
|----|------|
| 1 | `--version` / `--help` |
| 2 | 未配置模型时 `-p` 引导 |
| 3 | provider CRUD(add/list/use/delete) |
| 4 | OpenAI 协议端到端(工具调用循环) |
| 5 | Anthropic 协议端到端 |
| 5b | 项目上下文注入(说明文件五级链,3 场景) |
| 6 | 协议 wire 校验(User-Agent/X-Session-Id/x-api-key) |
| 7 | 非 TTY 回退 |
| 8 | **TUI 子屏 tmux 自动化**(15 个子用例) |
| 9 | provider delete |

#### 1.7.2 Mock LLM 策略(核心)

`scripts/mock_llm_server.py` 是一个 **Python stdlib HTTP 服务**,模拟双协议:

```python
STATE = {}  # 按 path 分别计数,避免跨协议干扰

def build_anthropic_stream(call_no):
    if call_no == 1:
        # 第 1 次:返回工具调用(Bash echo)
        events = [{"type":"message_start",...}, {"type":"content_block_start",...},
                  {"type":"content_block_delta","data":{...,"partial_json":'{"command": "echo LAEW_ANTHROPIC_OK"}'}}, ...]
    else:
        # 第 2 次:返回纯文本
        events = [..., {"type":"content_block_delta","data":{...,"text":"MOCK_FINAL_ANSWER: laew Anthropic 链路验证通过。"}}, ...]

def build_openai_stream(call_no):
    # 类似,OpenAI function_call 风格
```

**行为**:第 1 次请求返回工具调用,第 2 次返回最终文本。请求体落盘到 `mock_requests.jsonl` 供校验协议格式。这是**脚本化多轮响应**的典型实现。

#### 1.7.3 TUI 终端测试(核心亮点)

laew 的 TUI 测试是**真 PTY tmux 自动化**,这是本专题独有的:

```bash
# run_e2e.sh:250-260
tsend() { tmux send-keys -t "$TSESS" -l "$1" 2>>"$TMUX_LOG"; }
tkey()  { tmux send-keys -t "$TSESS" "$1" 2>>"$TMUX_LOG"; }
tscreen() { tmux capture-pane -p -t "$TSESS" 2>/dev/null; }
texpect() {
    local pat="$1" label="$2" timeout="${3:-3}"
    local deadline=$((SECONDS + timeout))
    while [ "$SECONDS" -lt "$deadline" ]; do
        tscreen | grep -F -q -- "$pat" && { check 0 "$label"; return 0; }
        sleep 0.1
    done
    check 1 "$label"
    { echo "    --- tmux capture at failure ---"; tscreen | sed 's/^/    | /'; echo "    --- end ---"; } | tee -a "$REPORT"
    return 1
}
```

**15 个 tmux 子用例**(节 8):

1. 横幅显示(根目录/工作目录/项目说明/Session/当前模型)
2. `/model` 主屏行为
3. `/provider list` 子屏(标题/记录统计/上下切换)
4. Esc 退出 ProviderList
5. `/provider add` Tab 表单
6. api_key Tab 浏览态脱敏
7. `/provider del` picker
8. Esc 退出 picker
9. `/provider use` 切换
10. `/clear` 清屏
11. 补全引擎交互(`/pro` → 补全列表 → Tab 接受 → Enter 提交)
12. Esc 关闭补全列表
13. `/provider add` 完整填写(5+1 Tab)
14. `/provider use <id>` 切换 + 屏幕栈测试(Push 不被当 Pop)
15. `/exit` 会话销毁

**断言策略**:`grep -F` 字面量匹配 + 失败时 dump 整个 tmux 面板(带 `|` 前缀),便于排查。

#### 1.7.4 项目上下文注入测试(独特)

节 5b 测试**说明文件五级链**(CLAUDE.md → AGENTS.md → README.md → 自动生成 → 空),3 个场景:

- 场景 A:三级并存 → 只注入 CLAUDE.md
- 场景 B:仅有其它 md → 自动分析生成 README.md
- 场景 C:无任何 Markdown → 不注入

通过 `mock_requests.jsonl` 中的 `v1/messages` 请求体,用 Python 校验 `<<<LAEW:PROJECT_CONTEXT>>>` 标记和注入内容。这是**协议级行为断言**。

#### 1.7.5 laew 缺失什么(对照总结)

| 维度 | laew 现状 | 业界标杆 |
|------|----------|---------|
| 单元测试 | **0** | deepseek 906 / openclaw 7725 |
| 集成测试 | 0 | pi 478 / opencode 673 |
| e2e | 548 行 bash(扎实) | deepseek 多层 |
| Eval 框架 | **无** | pi vitest-evals / atomcode eval.py |
| Snapshot | **无** | deepseek 60+ 场景 |
| Git hooks | **无** | openclaw 11 hooks |
| CI | **无** | opencode/test.yml |
| 覆盖率 | **无** | deepseek 100% 文件门 |

---

## 2. 横向对比总表

### 2.1 八维度 × 七仓总表

| 维度 | pi | atomcode | opencode | deepseek | openclaw | claudecode | laew |
|------|-----|----------|----------|----------|----------|------------|------|
| **测试文件数** | ~478 | ~30+evals | ~673 | ~906 | **~7725** | **0** | 0(仅 e2e bash) |
| **测试分层** | 3 层(unit/e2e/eval) | 2 层(unit+evals) | 3 层(unit/integration/e2e) | **6 层**(unit/coverage/e2e/expected/snapshot/web) | 4 层(unit/integration/e2e/contract) | 0 | 1 层(e2e) |
| **单测组织** | co-located `tests/` | inline `#[cfg(test)]` + `tests/` | co-located `test/` | co-located `tests/**` | co-located `*.test.ts` | — | — |
| **Mock LLM** | 不 mock(真实 key) | 不 mock(真实 key) | 渲染器 mock + http-recorder | **4 款 test-support 子包** | vi.mock + 真实二进制 | — | Python stdlib mock 服务 |
| **Eval 框架** | **vitest-evals**(正式) | **eval.py**(stdlib) | 无 | 无 | 无 | — | **无** |
| **Eval case 数** | smoke+extensions | 28(model 20+agent 8) | — | — | — | — | — |
| **评分方式** | LLM-as-judge + 规则 | LLM-as-judge(Codex) | — | — | — | — | — |
| **TUI 测试** | 无 | 无 | **@opentui/core testing**(渲染器 mock) | **PTY 子进程**(ACP adapter) | 无 | — | **tmux 真 PTY**(15 用例) |
| **Snapshot** | 无 | 无 | 无 | **60+ 录制场景** | 无 | — | **无** |
| **Fixture 管理** | session JSONL artifact | agent-fixture 目录 | http-recorder cassette | session-snapshot 包 | test-support.ts 共享 | — | mock_requests.jsonl |
| **CI** | ci.yml + pr-gate.yml | 无(仅 README) | test.yml(unit+e2e 矩阵) | ci-master.yml + gitlab-ci | 20+ workflows | — | **无** |
| **Git hooks** | .husky(存在) | 无 | .husky(存在) | lefthook.yml | **prek 11 hooks** | — | **无** |
| **覆盖率** | 未强调 | 无 | 未强调 | **100% 文件门** | 未强调 | — | **无** |
| **测试 runner** | vitest | cargo test + unittest | bun test | vitest(10+ 配置) | vitest(119 配置) | — | bash + Python |
| **环境隔离** | test.sh env -i | ATOMCODE_HOME 隔离 | — | test-invariants.ts | test-env.ts hermetic | — | /tmp/laew-e2e-root |
| **flake 处理** | 未强调 | 未强调 | 未强调 | 自跳过 + 重试 | 未强调 | — | 轮询断言 |

### 2.2 测试金字塔形态对比

```
            laew            pi/opencode           deepseek/openclaw
            ___            ___                   ___
           /   \          /   \                 / e2e\        ← 宽
          / e2e \        /eval \               /------\
         /_______\      /_______\             / unit  \
        (单测=0)       /  unit  \            /________\
                      /_________\          / integration\
                                     (单测厚,倒金字塔修正)
```

laew 是**倒金字塔**(仅 e2e),pi/opencode 是**标准金字塔**,deepseek/openclaw 是**厚单测 + 多层 e2e 的圆柱**。

### 2.3 Mock LLM 策略谱系

| 策略 | 代表 | 优点 | 代价 |
|------|------|------|------|
| **不 mock,真实 key** | pi / atomcode | 证明真实行为 | 消耗配额,CI 需 key |
| **脚本化多轮响应** | laew mock_server | 可复现,无 key | 仅覆盖预设路径 |
| **故障注入服务器** | deepseek llm-mock-server | 覆盖恢复策略 | 实现复杂 |
| **录制回放** | deepseek llm-replay | 无 key + 真实 transcript | 需维护 fixture |
| **渲染器 mock** | opencode @opentui testing | 快速,CI 友好 | 不验证真实终端 |
| **vi.mock 局部** | openclaw setup.shared | 隔离外部依赖 | 不验证集成 |

---

## 3. 可复用设计模式(8 个)

以下模式均从真实代码提炼,标注文件锚点,便于 laew 借鉴。

### 模式 P1:脚本化多轮响应 Mock(laew 已有,可增强)

**出处**:`scripts/mock_llm_server.py:88-130`(laew 自有)

**描述**:HTTP 服务按请求计数返回不同响应(第 1 次工具调用、第 2 次最终文本),请求体落盘供事后校验。

**laew 现状**:已实现,但仅支持"工具调用→文本"两阶段固定序列。

**增强方向**(借鉴 deepseek llm-mock-server):
- 支持 `--sequence` 自定义响应序列
- 支持 `tool_call_success`/`empty`/`malformed_json` 等行为
- 支持 `random` 模式做压力测试

### 模式 P2:Hermetic 测试环境(env -i + 临时 HOME)

**出处**:`test.sh:18-45`(pi)

**描述**:`env -i` 启动空白环境,仅注入白名单变量;临时目录打所有权标记,cleanup 时校验。

**laew 借鉴**:laew 的 `/tmp/laew-e2e-root` 是简化版,可借鉴 pi 的**所有权标记 + 拒绝删除未验证路径**安全机制,避免误删真实目录。

### 模式 P3:成对并发 Eval + LLM-as-Judge

**出处**:`evals/deepseek-v4-flash/eval.py:33-55` + `tests/test_eval.py`(atomcode)

**描述**:候选模型对同一输入并发执行(独立 HOME),由第三方 LLM(Codex)按 rubric 打分,生成 report.md。

**laew 借鉴**:为 laew 的 Yolo 分类器 / Main-Work 工作流建立 Eval:
- 定义 case JSON(prompt + rubric)
- 成对跑 baseline vs candidate
- 用规则断言(无需 LLM judge)做先期版本

### 模式 P4:录制回放 Snapshot 三态(record/replay/refresh)

**出处**:`docs/testing.md` + `vitest.snapshot.config.ts`(deepseek)

**描述**:record 调真实 LLM 写 fixture,replay 回放对比,refresh 重写期望输出。CI 强制 replay 只读。

**laew 借鉴**:为 laew 的 TUI 子屏建立 snapshot:
- record:tmux capture-pane 输出落盘
- replay:新运行输出与 fixture diff
- 用 `normalizeSessionSnapshot` 思路(路径/时间脱敏)

### 模式 P5:PTY 真终端自动化(tmux control-mode)

**出处**:`testReport/run_e2e.sh:250-420`(laew 自有)

**描述**:tmux 后台会话 + `send-keys` 发键 + `capture-pane` 抓输出 + 轮询断言。

**laew 现状**:已实现 15 个子用例,是本专题亮点。

**增强方向**:
- 引入 snapshot 对比(模式 P4)
- 增加失败自动 `tmux attach` 调试钩子
- 抽 tmux helper 为可复用库

### 模式 P6:测试支持包独立发布

**出处**:`packages/test-support/`(deepseek)—— llm-mock-server / llm-replay / agent-loop-testkit / session-snapshot

**描述**:把测试基础设施拆为独立包,有独立 README、单测、版本。

**laew 借鉴**:把 `mock_llm_server.py` + tmux helper 抽为 `testReport/testkit/`,未来可独立演进。

### 模式 P7:本地质量门矩阵(pre-commit 多 hook)

**出处**:`.pre-commit-config.yaml`(openclaw)—— 11 个 hook

**描述**:pre-commit 阶段运行文件卫生、lint、安全审计、依赖审计、skills 测试。

**laew 借鉴**:为 laew 建立 Rust 专属 hook:
- `cargo fmt --check`
- `cargo clippy`
- `cargo test`(未来有单测后)
- `bash -n run_e2e.sh`(脚本语法)

### 模式 P8:测试政策文档化

**出处**:`docs/testing.md`(deepseek)

**描述**:把测试分层、with-key 策略、mock 原则、snapshot 要求写成纲领性文档。

**laew 借鉴**:laew 的 `docs/TUI自动化测试/` 是零散文档,可整合为 `docs/测试体系与Eval基建.md` 政策文档。

---

## 4. laew 借鉴路线

### 4.1 现状盘点(资产与缺口)

**已有资产(保持)**:
- ✅ `testReport/run_e2e.sh` 548 行 bash e2e
- ✅ `scripts/mock_llm_server.py` 双协议 mock
- ✅ tmux 真 PTY 子屏自动化(15 用例)
- ✅ 项目上下文注入测试(五级链 3 场景)
- ✅ 协议 wire 校验(User-Agent/X-Session-Id)

**核心缺口(需补)**:
- ❌ **单元测试 = 0**(Rust `#[cfg(test)]` 模块完全空白)
- ❌ **Eval 框架 = 0**(无 case 定义、评分、回归对比)
- ❌ **Snapshot = 0**(TUI 输出无录制回放)
- ❌ **Git hooks = 0**(无本地质量门)
- ❌ **CI = 0**(无 GitHub Actions)
- ❌ **覆盖率 = 0**(无 cargo tarpaulin/llvm-cov)

### 4.2 借鉴路线图(P0/P1/P2)

#### P0 — 立即可做(第 1-2 周,投入小,收益大)

| 项 | 具体动作 | 借鉴来源 | 预估量 |
|----|---------|---------|--------|
| **P0-1:单测骨架** | 在 `src/agent/tools/`、`src/llm/`、`src/config/` 下各建一个 `#[cfg(test)] mod tests`,覆盖纯函数(Schema 解析、路径脱敏 mask_key、provider 五元组校验) | deepseek 惯例 | ~200 行 Rust |
| **P0-2:git hooks** | 用 `lefthook.yml`(比 prek 轻)配 3 个 hook:`cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test` | openclaw .pre-commit-config.yaml | ~20 行 YAML |
| **P0-3:CI 冒烟** | `.github/workflows/ci.yml`:`cargo build` + `cargo test` + `bash testReport/run_e2e.sh`(tmux 段需 self-hosted 或跳过) | opencode test.yml | ~40 行 YAML |

#### P1 — 中期建设(第 3-6 周,形成体系)

| 项 | 具体动作 | 借鉴来源 | 预估量 |
|----|---------|---------|--------|
| **P1-1:Eval 框架 v1** | 在 `evals/` 建 Python stdlib eval 脚本(仿 atomcode eval.py),定义 case JSON(prompt + 规则断言),跑 Yolo 分类 / Main-Work 工作流 | atomcode eval.py | ~400 行 Python |
| **P1-2:Eval case 集** | 首批 10 个 case:Yolo 三档分类 3 + 项目上下文注入 3 + provider CRUD 2 + 协议 wire 2 | pi smoke.eval.ts | ~200 行 JSON |
| **P1-3:TUI snapshot** | tmux capture-pane 输出落盘为 `.snap`,replay 时 diff(借鉴 deepseek 三态) | deepseek session-snapshot | ~150 行 bash |
| **P1-4:覆盖率门** | `cargo tarpaulin` 或 `cargo llvm-cov`,首年目标 40% 行覆盖 | deepseek 100% 文件门 | CI 集成 |

#### P2 — 长期演进(第 7-12 周,工业级)

| 项 | 具体动作 | 借鉴来源 | 预估量 |
|----|---------|---------|--------|
| **P2-1:故障注入 mock** | 增强 `mock_llm_server.py` 支持 `--sequence` + `random` 模式 | deepseek llm-mock-server | ~300 行 Python |
| **P2-2:LLM-as-Judge** | Eval 引入真实 LLM 打分(需 key),规则断言兜底 | pi vitest-evals Judge | ~200 行 |
| **P2-3:testkit 独立** | 抽 `testReport/testkit/` 为独立模块(bash + Python) | deepseek test-support 包 | 重构 |
| **P2-4:测试政策文档** | 整合 `docs/测试体系与Eval基建.md` 政策文档 | deepseek docs/testing.md | ~300 行 |

### 4.3 6 周 P0 落地计划(具体到周)

| 周 | 任务 | 产出 |
|----|------|------|
| W1 | P0-1 单测骨架(tools/llm/config 三模块) | `cargo test` 从 0 → ~15 用例 |
| W1 | P0-2 lefthook 3 hooks | 提交前自动 fmt/clippy/test |
| W2 | P0-3 CI 冒烟(无 tmux 段) | PR 自动跑 build+test+e2e |
| W3 | P1-1 Eval 框架 v1 | `evals/run.py` 可跑分 |
| W4 | P1-2 首批 10 eval case | 基线分数 |
| W5 | P1-3 TUI snapshot | 子屏回归可自动对比 |
| W6 | P1-4 覆盖率 + 政策文档 v0 | 首份测试报告 |

### 4.4 预期收益

完成 P0 + P1 后,laew 将从「e2e only」升级为**三层体系**:

```
        ┌─────────┐
        │  Eval   │  ← 10 case,回归可量化
        ├─────────┤
        │  e2e    │  ← 已有 548 行 bash + tmux
        ├─────────┤
        │  单测   │  ← 新增 ~15 用例,CI 门
        └─────────┘
        + git hooks + CI + 覆盖率
```

这将使 laew 的测试成熟度从**L1(e2e 手工)** 跃升至 **L3(单测+e2e+Eval 三轨)**,对齐 pi / opencode 水平。

---

## 附录 A:深读文件清单(25+ 文件)

| # | 文件 | 仓库 | 本专题用途 |
|---|------|------|-----------|
| 1 | `testReport/run_e2e.sh` | laew | e2e 主入口(548 行) |
| 2 | `scripts/mock_llm_server.py` | laew | Mock LLM 双协议 |
| 3 | `packages/evals/src/pi-harness.ts` | pi | Eval harness 核心 |
| 4 | `packages/evals/src/smoke.eval.ts` | pi | eval case 样例 |
| 5 | `packages/evals/src/extensions.eval.ts` | pi | Judge 机制 |
| 6 | `packages/evals/scripts/run-evals.mjs` | pi | eval runner |
| 7 | `packages/evals/vitest.config.ts` | pi | eval vitest 配置 |
| 8 | `test.sh` | pi | hermetic 环境范本 |
| 9 | `vitest.base.ts` | pi | 路径别名继承 |
| 10 | `evals/deepseek-v4-flash/eval.py` | atomcode | 627 行 stdlib eval |
| 11 | `evals/deepseek-v4-flash/cases/model-cases.json` | atomcode | 20 model case |
| 12 | `evals/deepseek-v4-flash/cases/agent-cases.json` | atomcode | 8 agent case |
| 13 | `evals/deepseek-v4-flash/tests/test_eval.py` | atomcode | eval 框架单测 |
| 14 | `packages/http-recorder/test/record-replay.test.ts` | opencode | 磁带录制回放 |
| 15 | `packages/tui/test/app-lifecycle.test.tsx` | opencode | 渲染器 mock |
| 16 | `.github/workflows/test.yml` | opencode | CI 双轨矩阵 |
| 17 | `docs/testing.md` | deepseek | 测试政策纲领 |
| 18 | `packages/test-support/llm-mock-server/README.md` | deepseek | 故障注入服务器 |
| 19 | `packages/test-support/llm-replay/README.md` | deepseek | 录制回放适配器 |
| 20 | `packages/test-support/agent-loop-testkit/README.md` | deepseek | loop 依赖挂载 |
| 21 | `packages/test-support/session-snapshot/README.md` | deepseek | snapshot 支撑 |
| 22 | `vitest.snapshot.config.ts` | deepseek | snapshot 三态配置 |
| 23 | `packages/core/agent-loop/tests/contract-regressions.spec.ts` | deepseek | 契约回归测试 |
| 24 | `git-hooks/pre-commit` | openclaw | pre-commit 入口 |
| 25 | `.pre-commit-config.yaml` | openclaw | 11 hook 矩阵 |
| 26 | `test/setup.shared.ts` | openclaw | vi.mock 统一 |
| 27 | `test/test-env.ts` | openclaw | hermetic 环境 |
| 28 | `test/cli-json-stdout.test-support.ts` | openclaw | 真实二进制测试 |
| 29 | `.github/workflows/ci-master.yml` | deepseek | self-hosted 热备 CI |
| 30 | `turbo.json` | opencode | turbo 任务编排 |

> 实际深读 30 文件(超过 25 文件门槛),覆盖测试文件本身 + 测试基建 + CI + hooks。

## 附录 B:关键术语速查

| 术语 | 含义 |
|------|------|
| **hermetic test** | 完全隔离的测试,空白环境 + 白名单变量 |
| **cassette** | HTTP 录制磁带(opencode http-recorder) |
| **LLM-as-Judge** | 用 LLM 按 rubric 给回答打分 |
| **record/replay/refresh** | snapshot 三态(deepseek) |
| **成对并发 eval** | 候选对同时跑,独立 HOME(atomcode) |
| **rubric** | 评分维度字典(correctness/quality/instruction_following) |
| **fixture** | 测试固定数据(录制会话/fixture 目录) |
| **test-support** | 独立发布的测试支持包(deepseek 4 款) |
| **contract-regression** | 契约回归测试(deepseek agent-loop) |
| **mock server** | 可脚本化故障服务器(deepseek llm-mock-server) |

---

> **本专题是知识库测试维度的开山之作**。后续可在本基础上展开「laew Eval case 设计」「TUI snapshot 实现」「Rust 单测最佳实践」等子专题。与本专题互补的已有文档:`docs/TUI自动化测试/`(tmux 自动化设计)/`docs/Agent架构对比与参考.md`(架构维度)。
