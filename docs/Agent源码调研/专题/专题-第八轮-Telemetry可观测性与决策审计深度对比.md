# 第八轮横向专题 · Telemetry / OpenTelemetry / 决策审计与可观测性深度对比

> 工程基线（2026-09-07）: 7 个目标项目 + laew 自我对照
>
> 覆盖维度: OTLP/OTel exporter 实现 · Span 设计模式 · TraceContext 传播 · 决策审计日志 · 隐私脱敏 · Metrics 指标 · 日志聚合 + trace_id/session_id 关联
>
> **本专题不重复第七轮 8 维度**（补丁/检索/Git/Bash/多模态/Cache/Schema/Web）。关注点: **可观测性 / OTel / 决策审计 / 脱敏 / 度量** —— 是 laew 从 PoC 升级到生产级 Agent CLI 的核心改造清单（对应 L16-L25 gap 的 L17-L22 子集: 决策溯源、错误容错、遥测、持久化、Token 预算）。

---

## 1. 摘要与 TL;DR

| 项目 | Telemetry 模型 | OTel/OTLP 深度 | TraceContext 传播 | 决策审计 | 脱敏 | Metrics | 评分(10) |
|---|---|---|---|---|---|---|---|
| **atomcode** | 自研 6-event envelope + NDJSON 队列 + HTTP sender | 无 OTel, 自建 schema v1 (scrub.rs) | 通过 `provider_host` 反查 (envelope) | `turn_complete` 钩子, 按 `StopReason` 分类错误 | regex 双向：KV (auth/secret) + 8 类 token shapes (GH/GL/AWS/Slack/OpenAI/JWT) | In-memory `Counters` + 持久化 `health.json` (Postgres-like 维度) | **8/10** |
| **claudecode** | 真 OTel SDK: traces/logs/metrics 三栈; Beta + 1P BigQuery 旁路 | 三栈 + ANTHROPIC_OTEL_* env 镜像 + 6 协议 (gRPC/HTTP-JSON/HTTP-protobuf) + Console | W3C TraceContext 默认; 还支持 beta BETA_TRACING_ENDPOINT; 携带 `prompt_id`/`workspace.host_paths` | `decision reason` 通过 `endLLMRequestSpan({success, statusCode, error, attempt})` 落 span attribute | `OTEL_LOG_USER_PROMPTS` gate + `redactIfDisabled` 在 emit 路径 + 2KB Honeycomb 截断 + `seenHashes` 哈希去重 | counter/histogram/gauge (token, cache, duration, error) + `BigQueryMetricsExporter` 5min 周期 | **10/10** |
| **deepseek-harness** | 能力分层: `session-telemetry`(capture) ↔ `session-telemetry-otel`(backend) | 真 OTel SDK HTTP OTLP logs (单协议) | Session 级 `cursor (session.id, event.seq)` + 父子 `session.parent_id`/`session.seed_length` | ledger channel = 镜像 session 日志; ops channel = `agent-error`/`shutdown` 运营事件 | `session-telemetry/record` cordis waterfall 扩展点, 本包零规则 | 仅 log 通道 (logs only) | **8/10** |
| **openclaw** | 独立 `diagnostics-otel` extension + 自建 W3C trace context 协议 | 真 OTel SDK: traces+logs+metrics 三栈 + OTLP HTTP protobuf + OWNED SDK 隔离 (双 SDK 协调) | 自建 W3C traceparent/spanId (32/16 hex), 8 propagator (tracecontext/baggage/b3/b3multi/jaeger), `bridge` 注册到 host gateway 协议层 | recorder 6 分类: harness/model/tools/operations/usage/recorder-runtime; 200+ counter/histogram | `redactSensitiveText` + `redactOtelAttributes` + 7-mode redact 状态机 + DEFAULT_REDACT_PATTERNS 27 类 | 6 个 recorder × 200+ counter/histogram; 100+ `openclaw.*` 指标名 | **10/10** |
| **opencode** | Effect 框架: `effect/unstable/observability` | Effect OTel `OtlpLogger.make` + `NodeSdk.layer` HTTP OTLP traces | 通过 Effect Tracer, 5 行 code 桥接 AI SDK `experimental_telemetry.tracer` | LlmChat error 走 `onError` + `experimental_telemetry.metadata.userId/sessionId` | 通过 Effect 自带 `Effect.logError` 链路 | 仅 OTel metrics via Effect 框架, 不直发 | **6/10** |
| **pi** | 自建类型化 schema-driven telemetry (`pi-telemetry` pkg) | 完全 **OTel-无关**, 纯 process-memory `InMemoryTelemetryContext` | 仅 `parentId` 整数指针; **无 W3C traceparent 协议** | schema-driven 强类型 (30+ span 模板: ai.request, harness.run/turn/step/tool/hook/checkpoint) | schema `sensitive` 标志 + `cardinality` 标签; 但无运行时脱敏实现 | 完全无 metrics; schema-only 内存记录 | **6/10** |
| **undici** | Node.js `diagnostics_channel` (非 OTel) | **无 OTel**; Node 原生 `diagnostics_channel` API | 仅 ctx propagation (async_hooks); **不实现 W3C traceparent** | 16 个 channel 名: `undici:client:beforeConnect/connected/connectError/sendHeaders` + `undici:request:create/bodySent/bodyChunkSent/bodyChunkReceived/headers/trailers/error` + websocket/proxy | **无脱敏**; 调试输出 `util.debuglog('undici')` 仅在 `NODE_DEBUG=undici` 时打印 | **无 metrics**; 仅 debug log + diagnostic event publish | **4/10** (非 Agent 系统, 但 Agent 的 HTTP 客户端基础) |

**最关键的 laew 启示**:
1. **openthai/claudecode/openclaw 都是 OTel 三栈**; laew 当前 **零 OTel**(只有 `tracing` crate 14 行 — `crates/atomcode-telemetry/src/runtime.rs:15`)。
2. **决策审计 ≠ OTLP 日志** —— claudecode 把 `decision reason`(LLM error / blocked reason) 写在 span attribute; atomcode 写 envelope `error_data`; openclaw 写 `openclaw.outcome` + `openclaw.deniedReason`; laew 当前 **0 决策审计**(只有 SQLite `session_memory` 摘要)。
3. **隐私脱敏有 3 个分层**: (a) emit 端 redact (claudecode `redactIfDisabled`、atomcode `redact_secrets`)、(b) attribute 端 redact (openclaw `redactOtelAttributes`)、(c) field 端 block list (openclaw `BLOCKED_OTEL_LOG_ATTRIBUTE_KEYS`/`DROPPED_OTEL_ATTRIBUTE_KEYS`)。
4. **OTel metrics 是 token 预算的天然基座** —— 6 个项目都把 `gen_ai.client.token.usage` 暴露为 histogram + `cache_read/cache_write` 维度; laew 缺 token 计数 = 缺成本治理。

---

## 2. 背景与动机: 为什么 Agent 系统需要可观测性

### 2.1 Agent 系统的可观测性痛点(相对传统微服务)

传统微服务的可观测性聚焦 `latency / traffic / errors / saturation` (USE/RED), 但 Agent 系统的复杂度多 3 个量级:

1. **状态空间爆炸**: LLM span 内部有 10+ 子事件 (`on_chunk`/`on_tool_call`/`on_done`), 每个都有自己的 latency, 一个 turn 长达 30-90s。
2. **错误语义多样**: 网络错误 + 模型错误 + tool 错误 + 用户取消 + 上下文溢出 + 速率限制 + 配额耗尽, 需要 **错误分类** (claudecode `classifyAPIError`、atomcode `classify_llm_error`)。
3. **成本驱动**: 每条 span 都关联 token usage, 没有 metric 就无法做 cost attribution / 用户配额。
4. **决策可溯源**: 用户问"为什么 Agent 选了 tool A 而不是 tool B", 需要 **完整决策链**: LLM 输入 → tool_call decision → hook approve/deny → tool result → next turn。
5. **隐私敏感**: Agent 工具直接读用户文件/环境变量, span attribute 中的 PII/API key/secret 泄漏风险高。

### 2.2 OTel GenAI 语义约定的演进

2024-2025 年 OTel 推出 GenAI 语义约定 (`gen_ai.*` namespace), 7 个项目中有 3 个直接遵循:

| 属性 | OTel semconv | atomcode | claudecode | openclaw | opencode | deepseek | pi |
|---|---|---|---|---|---|---|---|
| `gen_ai.provider.name` | required | ❌ | `tengu_api_query.provider` (mapped) | ✅ `service-genai-attributes.ts:33` | ❌ | ❌ | ✅ schema `pi.ai.provider` |
| `gen_ai.request.model` | required | envelope.model | `model` (OTel attr) | ✅ | metadata | ❌ | ✅ `pi.ai.model` |
| `gen_ai.usage.input_tokens` | required | input_tokens | messageTokens | ✅ `service-recorders-usage.ts:73` | ❌ | ❌ | ✅ `pi.ai.usage.input_tokens` |
| `gen_ai.client.operation.duration` | recommended | duration_ms | durationMs | ✅ `service-metrics.ts:76` | ❌ | ❌ | ✅ `pi.ai.stream.time_to_first_chunk_ms` |
| `gen_ai.client.token.usage` | recommended | ❌ (counter-style) | counter via Statsig | ✅ histogram | ❌ | ❌ | ✅ |

### 2.3 决策审计的 3 个时间点

1. **决策前 (intent recognition)**: 用户输入 → 任务分类 (Yolo 简单/中/高)
2. **决策中 (execution)**: 每个 tool_call 的 approve/deny 决定 + 错误恢复
3. **决策后 (post-mortem)**: 摘要生成 + 成本归因 + 错误根因

claudecode 的 `logAPIError` + `endLLMRequestSpan(llmSpan, {success, statusCode, error, attempt})` 是决策审计的范本, 把 `(outcome, reason, attempt_count, gateway, queryChainId)` 6 维信息固化为 span attribute。

---

## 3. 7 个工程逐项剖析

### 3.1 atomcode — 自研 6-event envelope + 4-level opt-out

#### 3.1.1 数据模型

`crates/atomcode-telemetry/src/event.rs:172-309` 6 个 event:

| event_id | 触发时机 | 关键 payload |
|---|---|---|
| `open_atomcode` | CLI 启动 (非 --version/--help) | `dangerously_skip_permissions: bool` |
| `llm_chat` | 每个 LLM turn 完成 | `duration_ms`, `tool_calls_count`, `input/output/cached_tokens`, `had_error`, `context_window`, `system/message/tool_result/tool_def_tokens`, `messages_count`, `error_kind`, `error_data` |
| `tool_call` | 每次 tool 调用 | `name`, `success`, `duration_ms`, `error_kind` (ToolErrorKind 11 类), `error_data` |
| `use_command` | 斜杠命令 | `type_`, `success`, `error_kind` (UseCommandErrorKind 4 类) |
| `mcp_connect` | MCP 连接 | `server_name`, `transport` (Stdio/Sse/StreamableHttp), `success`, `duration_ms` |
| `login_success` / `take_codingplan` | OAuth | `invite_code`/`install_uuid` 或 `success`/`fail` |
| `panic` | 全局 panic hook | `location`, `message_head` (200 字符), `thread`, `backtrace_top_5` (scrubbed) |

每条事件有 `Envelope` (12 字段: device_id, launch_id, session_id, turn_id, ts, schema_version, app_version, os, arch, locale, provider, model, repo_origin, mode, surface), 通过 `Record::flatten` 序列化为 NDJSON 单行。

#### 3.1.2 4 级 opt-out (config.rs:78-100)

```rust
pub fn resolve(cfg, cli, atomcode_dir, env, offline) -> ResolvedConfig {
    let state = if offline { Disabled("offline") }
        else if env.var("ATOMCODE_TELEMETRY") == Some("0") { Disabled("env:ATOMCODE_TELEMETRY=0") }
        else if env.var("DO_NOT_TRACK") == Some("1") { Disabled("env:DO_NOT_TRACK=1") }
        else if cli.disabled { Disabled("cli:--no-telemetry") }
        else if matches!(cfg.enabled, Some(false)) { Disabled("config") }
        else { Enabled };
    // 优先级: offline > env:ATOMCODE > env:DO_NOT_TRACK > cli > config
}
```

优雅之处: 5 个测试 (`config.rs:131-200`) 全程覆盖每个互斥分支。

#### 3.1.3 Scrub 模块 (scrub.rs:23-44) — 双向 regex 脱敏

```rust
// Pass 1: KV (authorization, x-api-key, api_key, password, secret, ...)
//         保留 key + separator, 替换 value 为 <REDACTED>
let kv = Regex::new(r#"(?i)(\b(?:authorization|x-api-key|...|private[-_]?token|...)\b\s*[:=]\s*"?(?:bearer\s+|token\s+)?)([^"'\s,;)]{6,})"#).unwrap();

// Pass 2: 8 类已知 token shape (GitHub/GitLab/AWS/Google/Slack/OpenAI/JWT)
let tokens = Regex::new(r#"\b(?:gh[opsru]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9\-_]{16,}|xox[baprs]-[A-Za-z0-9\-]{10,}|sk-[A-Za-z0-9\-_]{16,}|AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16}|AIza[0-9A-Za-z\-_]{35}|eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{6,})"#).unwrap();
```

**核心设计**: KV pass 处理 `curl -H "Authorization: token ghp_…"` 形; token pass 处理 `powershell $token = 'ghp_…'` 形(powershell 单引号逃逸 KV pass 的双引号字符类)。两个 pass 串联(`step1 = kv.replace_all; tokens.replace_all(step1)`), 9 个测试 (`scrub.rs:87-207`) 覆盖 GH/GitLab/AWS/Slack/OpenAI/JWT 全部 + 误报保护(`task-management-system` 中的 `sk-` 子串不命中)。

`scrub_path` (L46-59) 替换 `<HOME>`/`<CWD>`, 配合 `backtrace_top_k` (L68-85) 把 panic 栈缩短为 `function at file:line` 形式。

#### 3.1.4 Queue + Sender (queue/mod.rs + sender/http.rs) — 进程内 NDJSON 队列

队列关键设计:
- **L14-18**: `READY_EXT=ndjson`, `PARTIAL_EXT=partial`, `MARKER_SUFFIX=.owner` (lock marker) — 同进程可识别自己持有的 partial, 跨版本安全。
- **L51**: `f.lock_exclusive()` 用 `fs2` 文件锁, 多进程 daemon/CLI 共享 queue 不冲突。
- **L22**: `PARTIAL_QUIET_AFTER = days(1)` — 静默 1 天后才自动恢复 legacy partial。
- **L172-208**: `claim_oldest_segment` 用 `pid-uuid` 后缀 rename (`.sending-{pid}-{uuid}`), 发送失败时 `restore_claim` 改回 `.ndjson`, 发送成功时 `complete_claim` 删除; 失败不被重入循环(避免 head-of-line blocking)。
- **L247-288**: `clear_inactive` 跳过 lock-watcher(别的进程还在写的 partial 保留)。
- **L292-296**: `recover_legacy_partials` 显式恢复, 不自动, 需要用户先 stop 旧版。

Sender (`sender/http.rs`):
- **L46-52**: `Client::builder().timeout(10s).user_agent("atomcode-telemetry/{version}").build()`, 通过 `ATOMCODE_PROXY_MODE=no_proxy` 切代理。
- **L56-70**: `send_segment` 把 NDJSON 用 `flate2::GzEncoder(Compression::fast())` 压缩, POST `application/x-ndjson` + `content-encoding: gzip` + `x-atomcode-dropped` + `x-atomcode-schema`。
- **L80-96**: 状态码映射 `400→BadRequest, 401/403→Unauthorized, 413→PayloadTooLarge, 429→RateLimited(retry-after), 5xx→Server(c), 其它→Other`。
- **L107-114**: 退避表 `2s/8s/30s/120s/300s(cap)`。
- **L126-130**: 401 单独 hold **1 小时** (`Unauthorized` 状态是 server 配置问题, 持续打无意义), 其它 4xx 直接 drop(400/413 是永久错误, 退避无法恢复)。

#### 3.1.5 Coding Agent 集成 (coding/src/telemetry.rs) — Kernel hook seam

`TelemetryHook` 实现 `LifecycleHooks`, `ToolTelemetryMiddleware` 实现 `ToolMiddleware`, `MeteredProvider` 装饰 `LlmProvider` —— 三层抽象让 telemetry 成为 opt-in:
- **L14-18**: "kernel emits NO telemetry by design. It exposes two seams the project's observability rides on" — **kernel 是零遥测的**, 任何 telemetry 都要在 assemble 阶段加 adapter。
- **L42-61**: `estimate_message_tokens` 字节/4 启发式, 图像额外 1600 token/vision。
- **L66-82**: `scale_to` + `apportion` 算法 — exact prompt total (来自 usage report) 锚定, byte/4 估计只给相对分布, residual 加在最大 zone 保证 4 个 zone 之和 = total (L88-102)。**关键设计**: provider usage 是真值, byte/4 估计只用作 zone 切分。
- **L194-374**: `TelemetryHook` 实现 `on_request`/`on_model_response`/`on_error`/`turn_complete` 4 个钩子; `turn_complete` 区分 `ProviderError`(LLM 错误 → LlmChat+error_kind) 和 `Timeout/StopReason::Other`(不算 LLM 错误, 不上报)。
- **L376-453**: `ToolTelemetryMiddleware` 用 `call_id → (name, start)` HashMap 在 `before` 戳开始时间, `after` 算 elapsed。**重要**: 401 deny 中间件产生的 `blocked: …` 错误被识别为 `DeniedByUser`, 不会和 `NotFound` 混。
- **L476-584**: `MeteredProvider` 装饰器模式 — 在 stream 上 `futures::stream::unfold` 累积 `TokenUsage.merge_max()`, stream 结束时统一 emit 一条 `LlmChat` (把 `system/tool_def/tool_result` zone 折叠到 message zone, 因为 out-of-loop call 没有单独度量)。`with_surface("code_review")` 用于子 agent 的归因 (`Envelope.surface` 字段)。

#### 3.1.6 CLI 子命令 (cli/src/telemetry_cmd.rs)

7 个子命令:
- `status`: 启用/禁用 + endpoint + queue 统计 (events 数量 + 段数 + KB) + partial 滞留 + health 持久化
- `enable/disable`: 写 `~/.atomcode/config.toml [telemetry].enabled`, disable 时如果之前是 enabled, 发一条 `TelemetryDisabled` 事件
- `dump --last N --pretty`: tail 最近 N 行 NDJSON
- `clear`: 删除 inactive segments, 跳过 lock-watcher
- `recover`: 显式恢复 legacy partials

#### 3.1.7 Daemon 共享 (daemon/src/telemetry_scope.rs)

```rust
pub async fn daemon_scope<F, Fut, R>(state, session_id, mode, f) -> R {
    let ctx = CurrentContext {
        mode: Some(mode),  // SessionMode::Ide
        repo_origin: Some(state.repo_origin.clone()),
        session_id,
        ..CurrentContext::current()
    };
    CurrentContext::scope(ctx, f).await
}
```

daemon 进程和 CLI 进程共享 `~/.atomcode/telemetry/queue/`, 通过 `fs2` 文件锁 + `.owner` marker 区分自己/别人的 partial。

---

### 3.2 claudecode — OTel 三栈 + 1P BigQuery 旁路 + 5 级 telemetryAttributes

#### 3.2.1 三栈架构 (instrumentation.ts)

```ts
// signals: traces, logs, metrics (3 separate env vars)
const exporters = await getOtlpTraceExporters();  // ConsoleSpanExporter | OTLPTraceExporter {gRPC, http/json, http/protobuf}
const logExporters = await getOtlpLogExporters(); // ConsoleLogRecordExporter | OTLPLogExporter
const readers = await getOtlpReaders();           // + PrometheusExporter 旁路

// Resource: 服务标识 + 平台检测
const baseAttributes = { [ATTR_SERVICE_NAME]: 'claude-code', [ATTR_SERVICE_VERSION]: MACRO.VERSION };
// WSL: 加 'wsl.version'; 合并 osDetector + hostDetector(只取 host.arch) + envDetector
```

**L130-215** `getOtlpReaders`: 三种 exporter (`console`, `otlp`, `prometheus`) × 三种 protocol (`grpc`, `http/json`, `http/protobuf`) = 9 种组合,**所有 exporter 用 dynamic import** (`@opentelemetry/exporter-metrics-otlp-grpc` 等, ~1.2MB 总大小) — 注释 L1-5 明确说 "static imports would load all 6 (~1.2MB) on every startup", 启动时按需 import 节省冷启动 ~600ms。

**L165-188** gRPC exporter 单独 lazy import (`@grpc/grpc-js ~700KB`), 只在 protocol=grpc 时拉。

**L408-419** `initializeBetaTracing` 是**第二条独立 code path**: `BETA_TRACING_ENDPOINT` → traces+logs 直发到 ant-only 内部 `traceparent/lantern` 体系, 跳过外部 `OTEL_EXPORTER_OTLP_ENDPOINT`。

#### 3.2.2 ANT_OTEL_* env 镜像 (L88-117)

```ts
if (process.env.USER_TYPE === 'ant') {
  if (process.env.ANT_OTEL_METRICS_EXPORTER)
    process.env.OTEL_METRICS_EXPORTER = process.env.ANT_OTEL_METRICS_EXPORTER;
  // logs / traces / protocol / endpoint / headers 都镜像
}
```

**设计意图**: 内部 ant build 用 `ANT_*` 命名空间避免与外部用户配置冲突, 启动时 bridge 到 OTel 标准 env。`bootstrapTelemetry` 在 `applyConfigEnvironmentVariables` 之前调用, 这样 `init.ts` 重跑 applyConfig 后不会被重置。

#### 3.2.3 Console exporter 拦截 + stream-json 兼容 (L432-447)

```ts
if (getHasFormattedOutput()) {  // -p --output-format=stream-json
  for (const key of ['OTEL_METRICS_EXPORTER', 'OTEL_LOGS_EXPORTER', 'OTEL_TRACES_EXPORTER'] as const) {
    const v = process.env[key];
    if (v?.includes('console')) {
      process.env[key] = v.split(',').map(s => s.trim()).filter(s => s !== 'console').join(',');
    }
  }
}
```

**关键 bug 修复**: `console` exporter 每 5s/60s 用 `console.dir` 打印到 stdout, 而 stream-json 模式下 stdout 是 SDK 消息 channel, 第一行 `{` 会破坏 line reader。stripped 必须在 `bootstrapTelemetry` 之后, `applyConfigEnvironmentVariables` 之前(remote-managed-settings 用户重跑 init)。

#### 3.2.4 shutdown race-free (L533-560, 654-697)

```ts
const chains: Promise<void>[] = [meterProvider.shutdown()];
if (loggerProvider) chains.push(loggerProvider.forceFlush().then(() => loggerProvider.shutdown()));
if (tracerProvider) chains.push(tracerProvider.forceFlush().then(() => tracerProvider.shutdown()));
await Promise.race([Promise.all(chains), telemetryTimeout(timeoutMs, 'OTEL shutdown timeout')]);
```

**关键设计**: 之前是 `forceFlush` 无界 await → race → shutdown, 慢 OTLP 端点会 block 进程退出。改后是 **独立 chain** (`meter.shutdown() 独立, logger forceFlush→shutdown 独立, tracer 同`) — `Promise.all` 等待全部完成,**`Promise.race` 加 timeout 兜底**。timeout 默认 2000ms, 可通过 `CLAUDE_CODE_OTEL_SHUTDOWN_TIMEOUT_MS` 调到 5000+。

#### 3.2.5 Session tracing API (sessionTracing.ts) — 5 种 SpanType

```ts
type SpanType = 'interaction' | 'llm_request' | 'tool' | 'tool.blocked_on_user' | 'tool.execution' | 'hook';
```

`interaction` 是 root span (1 个 turn 1 个), `llm_request` 是每个 LLM call 的子 span, `tool`/`tool.blocked_on_user`/`tool.execution` 是 3 层 tool 生命周期, `hook` 是 hook 系统。**6 类 span 用 WeakRef + AsyncLocalStorage** (`L69-77` 注释清楚) 防止 OTel context chain 持有 SpanContext 阻止 GC。

`L100-120` 30 分钟 TTL 兜底 (`setInterval 60s, unref()`) 处理 `aborted streams / uncaught exceptions` 留下的孤儿 span。

#### 3.2.6 Beta Session Tracing (betaSessionTracing.ts) — 哈希去重 + 60KB 截断

**L70**: `MAX_CONTENT_SIZE = 60 * 1024` (Honeycomb limit 64KB, 留 4KB 余量)。

**L48-67**: `seenHashes: Set<string>` + `lastReportedMessageHash: Map<string, string>` — system prompt 全文只在第一次出现时上报, 后续只报 hash + 长度; 增量上下文 (new_context) 只报自上次 request 以来新增的 messages, 避免大上下文重复发送。`clearBetaTracingState` 在 compaction 后清空 (L65-68)。

**L141-208**: `extractSystemReminderContent` 用 regex `<system-reminder>...</system-reminder>` 识别系统提示, 把 context 分成 `[USER]` 普通文本 + `[TOOL RESULT: id]` 结果 + system reminder 三类, 便于接收端 diff 渲染。

**L235-280**: `system_prompt` 上报使用 `seenHashes` 模式 — `sp_${shortHash}` 是 12 字符 sha256 前缀, `preview` 是 500 字符前缀, `length` 是真实长度。**所有外部用户都能上报 system prompt, 但 thinking output 不上报 (L17-20 表格)**。

#### 3.2.7 1P BigQuery exporter (firstPartyEventLogger.ts + firstPartyEventLoggingExporter.ts)

`FirstPartyEventLoggingExporter` (L80-227) 是 OTel `LogRecordExporter` 接口的实现, 关键点:
- **L88-110**: `endpoint` 默认 `https://api.anthropic.com/api/event_logging/batch`, 如果 `ANTHROPIC_BASE_URL=staging` 用 staging; `timeout=10000ms, maxBatchSize=200, baseBackoffDelayMs=500, maxBackoffDelayMs=30000, maxAttempts=8`。
- **L113-120**: 失败事件落盘到 `~/.claude/telemetry/1p_failed_events.{sessionId}.{batchUuid}.json`, 重启时 `retryPreviousBatches` 重新尝试 — append-only log 抗崩溃。
- **L210-242**: 立即重试 — 当一次 export 成功时, 队列里的失败事件立即 re-fire (不等到下次 schedule); quadratic backoff 最多 8 次, 超过丢弃 (recover 路径用 disk 重放)。
- **L268-274**: auth fallback — 401 时不带 auth 重试一次 (oAuth token 过期场景)。

`firstPartyEventLogger.ts` (L87-102) 通过 GrowthBook 动态获取 batch config `tengu_1p_event_batch_config`, config 变更时 `reinitialize1PEventLoggingIfConfigChanged` 重建 provider — **注意**: 重建时所有 pending 事件会丢 (L107-110 注释 "Last batch config used to construct the provider — used by reinitialize1PEventLoggingIfConfigChanged")。

`getEventSamplingConfig` (L38-85): GrowthBook 远程下发 per-event 采样率, `sample_rate=0` drop, `sample_rate<1` 随机抽样, `=1` 100%。**关键**: 抽样通过 `randomUUID()` 在事件上加 `event_id`, 接收端去重。

#### 3.2.8 metricsOptOut 缓存双层 (metricsOptOut.ts:35-80)

```ts
const CACHE_TTL_MS = 60 * 60 * 1000;       // in-memory 1h
const DISK_CACHE_TTL_MS = 24 * 60 * 60 * 1000; // disk 24h
```

调用 `https://api.anthropic.com/api/claude_code/organizations/metrics_enabled` 检查 org 是否启用了 metrics, **in-memory TTL 防抖, disk TTL 跨进程持久** (一次 N 个 `claude -p` invocation 只发 1 次网络)。**关键设计 (L70-75)**: "Errors are not persisted — a transient failure should not overwrite a known-good disk value", 防 service-key OAuth 污染 cache (L90-97)。

#### 3.2.9 logOTelEvent + telemetryAttributes (events.ts + telemetryAttributes.ts)

`telemetryAttributes.ts:14-23` **3 个 cardinality 控制 env**:
- `OTEL_METRICS_INCLUDE_SESSION_ID=true` (默认 true) — session.id 维度
- `OTEL_METRICS_INCLUDE_VERSION=false` (默认 false) — 关闭避免 cardinality 爆
- `OTEL_METRICS_INCLUDE_ACCOUNT_UUID=true` (默认 true) — user.account_uuid

**L24-31** `shouldIncludeAttribute` 函数支持 env override + default 双重控制。

`events.ts:30-40` `redactIfDisabled` 在 **emit 端**做 redact — `OTEL_LOG_USER_PROMPTS` env 控制是否在 event body 中保留 user prompt 文本, 关闭时替换为 `<REDACTED>` (这是 OTLP event body 的 redact, 不是 span attribute 的, 区分清楚)。

`L33-39` `eventSequence` 自增计数用于排序, `L41-46` `promptId` (来自 `getPromptId()`) 关联到 prompt lifecycle。

#### 3.2.10 Perfetto tracing (perfettoTracing.ts) — 独立 code path

`Perfetto` 是 Chrome Trace Event 格式 (`ui.perfetto.dev` 可视化), 通过 `feature('PERFETTO_TRACING')` 用 `bun:bundle` 死代码消除 — **外部 build 完全剔除**。关键设计:
- **L99-106**: `MAX_EVENTS = 100_000`, cron 长时间 session 也不会无限增长, 触顶时 `evictOldestEvents` 一次砍半(amortized O(1))
- **L121-122**: `STALE_SPAN_TTL_MS = 30min`, 同 claudecode 自己的 sessionTracing TTL 一致
- **L107-111**: `agentRegistry: Map<agentId, AgentInfo>`, 每个 SubAgent 分配 numeric `processId` (main=1, sub=2,3,…) Perfetto 渲染需要
- **L286-298**: `CLAUDE_CODE_PERFETTO_WRITE_INTERVAL_S` 定期写盘 + `unref()` 不阻塞进程退出

#### 3.2.11 Plugin telemetry (pluginTelemetry.ts) — 双列隐私

**L36-50** `hashPluginId(name, marketplace)` — sha256(`name@marketplace` + 固定 salt `claude-plugin-telemetry-v1`) 取前 16 字符。**设计意图**: 不暴露用户定义的 plugin 名, 但保留 distinct-count + 趋势分析能力。

**L52-100** `getTelemetryPluginScope` 4 值枚举: `official` / `default-bundle` / `org` / `user-local`。**关键**: `plugin_id_hash` 是不依赖隐私的 per-plugin 聚合键; `plugin_name_redacted` 只有在 `official` 或 `default-bundle` 时才暴露真名, 其它用 `third-party` 替换。

#### 3.2.12 5 级 Envoy 钩子 + 错误分类 (logging.ts:235-396)

`logAPIError` (L235) 是决策审计的核心:
- `classifyAPIError(error)` 错误分类 → 11 个 enum
- `detectGateway({headers, baseUrl})` 识别 7 个 AI gateway (litellm/helicone/portkey/cloudflare-ai-gateway/kong/braintrust/databricks) — 通过 response header 前缀 + hostname 后缀 (`GATEWAY_FINGERPRINTS` + `GATEWAY_HOST_SUFFIXES` 两套策略)
- `consumeInvokingRequestId` 把 agent invocation 关联到当前 API call
- `logOTelEvent('api_error', {...})` 同时写 OTLP event log
- `endLLMRequestSpan(llmSpan, {success, statusCode, error, attempt})` 把 LLM span 标记为 failed (这就是决策审计的 span 形式)

`logAPISuccess` (L398) 同等丰富, 包含 `cachedInputTokens/uncachedInputTokens/cache_read/cache_creation` 拆分 + `globalCacheStrategy` (tool_based / system_prompt / none) + `preNormalizedModel` (alias → canonical model id 映射)。

#### 3.2.13 Code-edit tool decision counter (toolExecution.ts:23, 975)

```ts
getCodeEditToolDecisionCounter()?.add(1, attributes)  // approve / reject / block
```

这是一个专门的 counter, 跟踪 code-edit tool 的用户决策, 是 laew 当前完全缺失的"工具决策审计"维度的最直接参考实现。

---

### 3.3 deepseek-harness — 能力分层 capture / backend, Cordis 风格

#### 3.3.1 能力分层 (session-telemetry = Service Definition; session-telemetry-otel = Service Provider)

这是 **OTel 集成作为可选 service provider** 的范本:
- `session-telemetry` 包定义 `SessionTelemetryRecord` / `SessionTelemetryBackend` / `SessionTelemetryCoordinator` 接口
- `session-telemetry-otel` 包实现 `OpenTelemetrySessionBackend extends SessionTelemetryBackend`

cordis (类似 NestJS DI) 在 plugin load 时注入; deployment 选 backend, capture 代码不变。**关键设计 (otel/src/index.ts:147-153)**: `OpenTelemetrySessionBackend` 通过 `static inject = ['sessions']` 拿到 sessions service, 通过 `static Config = Config` 声明 schemastery 验证。

#### 3.3.2 SessionTelemetryRecord (otel/src/index.ts + telemetry.md)

```ts
interface SessionTelemetryRecord {
  channel: 'ledger' | 'ops';  // 两个 channel, 接收端分组
  time: number;                // ledger = source event 追加时间; ops = emit 时间
  severity: 'info' | 'warn' | 'error';  // 三级
  attributes: Record<string, string | number>;  // 最小身份属性
  body: unknown;               // 完整 payload (deep copy)
}
```

**关键设计 (telemetry.md L11-15)**: 
- ledger 通道 = session-log 镜像 (1:1)
- ops 通道 = 运营信号 (`agent-error` / `shutdown`), **故意不带 `event.seq` 类身份**, 避免被误聚合

`severityOf` (coordinator.ts:285-297) 把 `tool/result.isError` 和 `turn/end.reason.kind=='error'` 映射到 `error` severity, 其它默认 `info`, **plugin-merged event types 不识别, 直接 info** (设计意图: "their owners' outcome semantics stay theirs")。

#### 3.3.3 Cursor 机制 (coordinator.ts:36-44)

```ts
const handoffCursor = new WeakMap<Session, number>();  // per session, 最高已交付 seq
```

`WeakMap<Session, number>` 的妙用:
- **生命周期 = session 生命周期** (cordis 没有 HMR state-handover, 这是最窄的 documented exception to registrations-are-effects)
- **缺失 cursor = 重新交付** (resume 场景, 这是 at-most-once 的代价)
- **cursor 标 handed-off, 不是 delivered** (L41 注释) — SDK retry / crash 可能丢也可能重

#### 3.3.4 Chunk projection (coordinator.ts:172-187)

只发 `assistant/chunk` 的**第一个**(stream-started signal), 后续 chunk 在 capture 阶段 drop:
```ts
if (event.type === 'assistant/chunk') {
  const key = `${event.data.turn}:${event.data.step}`;
  if (seen.has(key)) return;  // 后续 chunk drop
  seen.add(key);
}
```

**为什么**: 完整 assistant message 在 `assistant/message` event, byte-complete, chunk 只表示流开始。**seq 间隙是常态, 接收端不视为丢** — telemetry.md L60-63 明确说 "seq gaps are routine on the wire and never a loss signal"。

#### 3.3.5 session-telemetry/record waterfall (coordinator.ts:213-215)

```ts
private redact(record: SessionTelemetryRecord): SessionTelemetryRecord {
  return this.ctx.waterfall('session-telemetry/record', record, () => record);
}
```

cordis waterfall 模式: deployment 可以挂多个 redaction rule, 默认 (innermost) pass-through; **本包 ships NO rules** (telemetry.md L42-43), exported data is as clean as the listeners a deployment mounts。**优雅**: 脱敏作为 capability seam extension, 不绑死在 telemetry 包里。

`contain` 包裹每个 emit (L253-259), 异常被 `ctx.logger.warn` 吃掉 — 防止一个 throwing listener starves 后续所有 subscriber (cordis `emit` 是 stop-on-throw)。

#### 3.3.6 Three sharing modes (otel/src/index.ts:43-84)

```ts
export enum SessionTelemetryMode { FULL, FEEDBACK_ONLY, DISABLED }
const DEFAULT_TELEMETRY_MODE = SessionTelemetryMode.DISABLED;  // 默认关闭
```

- `FULL` = live capture (订阅 `session/created` / `session/event` / `session/flush` / `agent/error` / `session/disposed` 5 个事件)
- `FEEDBACK_ONLY` = 仅在 `feedback/record` 事件 (用户主动提交 feedback) 时回放 canonical log, 不持续 capture
- `DISABLED` = 不构建任何 SDK state, 仅在收到 feedback 时 warn 用户 "session telemetry is DISABLED; nothing will be shared and this feedback remains local"

`feedback/record` 处理 (L243-253) 验证 event 在 canonical log (`session.events[event.seq] === event`), 否则 warn (防 non-canonical emission 污染)。

#### 3.3.7 OTel Backend 实现 (otel/src/index.ts:147-300)

`OpenTelemetrySessionBackend` (L147-300):
- **L197-218**: `new LoggerProvider({ resource, processors: [BatchLogRecordProcessor({...config.processor, exporter: new OTLPLogExporter(config.exporter)})] })` — 把 SDK 文档化的所有 option (`timeoutMillis` / `compression` / `keepAlive`) 透传
- **L198-205**: Resource 含 `service.name=APP_IDENTITY.product`, `service.version=APP_IDENTITY.version`, **`user.id=getOrCreateAnonymousUserId()`** (per-export, process-stable)
- **L219-220**: 两个 logger 分离: `@deepseek-ai/dsh-session-telemetry-otel` (ledger) + `@deepseek-ai/dsh-session-telemetry-otel/ops` (ops) — instrumentation scope 隔离
- **L222-232**: `enqueue` lambda 把 record 映射到 OTel log: `timestamp: record.time, observedTimestamp: record.time, severityNumber: SEVERITY[record.severity].severityNumber, body: record.body, attributes: record.attributes`
- **L266-274**: **故意 NOT 实现 flush() hint** — 注释明确说 "forwarding the hint to `forceFlush()` would be the sole source of concurrent flushes" 风险(SDK 的 forceFlush + shutdown 内部 drain 有未文档化的并发交互)
- **L283-298**: shutdown 双重 timeout — `provider.shutdown()` + 外部 `shutdownTimeoutMillis` (默认 3s, max `2_147_483_647` Node.js timer limit) `Promise.race`, 即使 race 输了也保 promise observed 防止 unhandled rejection

`Config` schema (L120-125) 用 schemastery 验证 `mode/exporter/processor/shutdownTimeoutMillis`; UPLOADING mode 验证 `exporter.url` 必须是 http(s) (L181-183); `processor.maxExportBatchSize` 必须是正整数 (L188-191, 否则 SDK shutdown 会 hang) — **fail at load, not at runtime**。

#### 3.3.8 完整决策审计示例 (测试 + docs)

`telemetry.md` 提供完整 example (L30-100) 显示 record 怎么从 session event 流转到 OTel log; `tests/otel.spec.ts` 验证 flush/shutdown/mode 转换; `tests/loader-composition.e2e.ts` 端到端 cordis 注入链。

---

### 3.4 openclaw — 独立 extension + 6 recorder 分类 + 200+ 指标

#### 3.4.1 Extension 边界 (service.ts:1-80)

`extensions/diagnostics-otel` 是 OpenClaw plugin, 导出 `OpenClawPluginService`:
```ts
return {
  id: "diagnostics-otel",
  reload: { configPrefixes: ["diagnostics.otel", "diagnostics.enabled"] },
  async start(ctx) { ... }
};
```

Service lifecycle 严格: `stopStarted` (L177-230) 6 步清理 (unregisterBridge, unsubscribe, stopActiveSpans, unregisterSDK, logProvider.shutdown, traceProvider.shutdown, meterProvider.shutdown) — 任何失败用 `Promise.allSettled` 收集, 重新 throw `AggregateError`。

#### 3.4.2 OWNED SDK 隔离 (service-propagation.ts:1-150)

`OwnedContextManager` + `OwnedPropagator` 包装 — 解决 **双 SDK 协调** 问题(host 应用可能预加载了 OTel SDK, plugin 又用自己版本的 SDK, 两套 context manager 抢全局):

```ts
class OwnedContextManager extends AsyncLocalStorageContextManager {
  override with(activeContext, fn, thisArg?, ...args) {
    const probe = activeContext.getValue(CONTEXT_OWNER_KEY);
    if (probe && typeof probe === "object") probe.owner = this.owner;  // 标记 ownership
    return super.with(activeContext, fn, thisArg, ...args);
  }
}
```

`ownsGlobalPropagator` / `ownsGlobalContextManager` 通过 probe 验证是否真的拥有 global, 如果没抢到就 disable 自己 (L120-130)。**这是 laew 直接需要的能力**: laew 后续接入 OTel 时, 如果用户进程预加载了 OTel (Bun runtime debug tool, monitoring agent), 必须有这套 ownership 检测。

#### 3.4.3 自建 W3C trace context (diagnostic-trace-context.ts:1-248)

不直接用 OTel API, **自建 W3C trace context 协议层**, 通过 `bridge` 翻译给 OTel SDK:
- **L9-13**: 严格的 W3C 格式 regex (32/16/2 hex, 版本 `00`, flags `01` 默认)
- **L39-57**: `randomTraceId` / `randomSpanId` 拒绝全 0 退避重试
- **L132-162**: `parseDiagnosticTraceparent` 严格解析 (4 part, 长度 ≤ 128)
- **L165-178**: `formatDiagnosticTraceparent` 输出标准 `00-{traceId}-{spanId}-{flags}`
- **L77-91**: 通过 `Symbol.for("openclaw.diagnosticTraceScope.state.v1")` + `Object.defineProperty` 挂在 globalThis 上的 **singleton 模式** — 跨 preloaded/owner SDK 副本共享状态

`diagnostic-trace-propagation.ts` 进一步抽象 `bridge` 注册 (L79-90): deployment 注册自己的 translate 函数, `resolveTraceContext(traceContext)` 把 diagnostic context 翻译成 exporter-owned context,**防止 fallback 到 diagnostic ids naming a parent span that the exporter never created** (L139-140 注释)。

#### 3.4.4 6 个 Recorder 分类 (service-recorders-*.ts)

| Recorder | 文件 | 主要 metrics | 主要 spans |
|---|---|---|---|
| harness | recorders-harness.ts (400L) | `openclaw.harness.duration_ms` | `openclaw.harness.run` |
| model | recorders-model.ts (168L) | `openclaw.tokens`, `openclaw.cost.usd`, `gen_ai.client.token.usage` | `openclaw.model.*` |
| tools | recorders-tools.ts (400L) | `openclaw.tool.execution.duration_ms` (histogram), `openclaw.tool.execution.blocked` (counter) | `openclaw.tool.execution` (started/completed/error/blocked) |
| operations | recorders-operations.ts (423L) | `openclaw.payload.large`, `openclaw.liveness.*` | `openclaw.operation.*` |
| usage | recorders-usage.ts (398L) | `openclaw.tokens` + 6 个 message 维度 counter | `openclaw.message.*` |
| recorder-runtime | recorder-runtime.ts (19L) | 公共 helpers | trusted span tracking |

每个 recorder 通过 `recordXxxStarted/Completed/Error/Blocked` 4 步形成 lifecycle span, **用 `trackTrustedSpan` 在 map 中维护 active span 集合, `takeTrackedTrustedSpan` 消费**(`recordToolExecutionFinished` L149-155) — 保证 started 和 finished 配对(避免 stream 异常中断留下孤儿 span)。

#### 3.4.5 200+ 指标名 (service-metrics.ts)

`service-metrics.ts:1-200` 集中创建 100+ `openclaw.*` counter + histogram, **全部按 5 类聚合**:
- **gateway**: `gateway.rpc.requests/outcomes/first_response_ms/handler_ms/admission_ms/queue_wait_ms/event_loop.delay_max_ms/event_loop.observed_ms` (11 个)
- **agent**: `run.duration_ms, harness.duration_ms, context.tokens, tokens, cost.usd, gen_ai.client.token.usage, gen_ai.client.operation.duration` (7 个)
- **webhook**: `webhook.received/error/duration_ms` (3 个)
- **message**: queued/received/dispatch.started/completed/duration/processed/duration/delivery.started/duration (9 个)
- **session/queue**: `session.state/turn.created/stuck/stuck_age_ms/recovery.requested/completed/age_ms, queue.depth/wait_ms/lane.enqueue/lane.dequeue` (10 个)

每个 counter/histogram 都带 `unit` (语义约定 `1` / `ms` / `{token}` / `s`), `description` (业务可读), `advice.explicitBucketBoundaries` (直方图桶边界) — OTel 语义规范完整。

#### 3.4.6 redactSensitiveText + redactOtelAttributes 双层 (redact.ts + service-attributes.ts)

`redact.ts:919-942` 主入口:
```ts
export function redactSensitiveText(text, options?): string {
  if (!text) return text;
  const exactRedacted = redactRegisteredSecretValues(text, maskToken);  // 优先 exact match
  const resolvedOptions = options ?? resolveConfigRedaction();
  if (normalizeMode(resolvedOptions.mode) === "off") return exactRedacted;
  if (usesBuiltInRedactPatterns(resolvedOptions.patterns) && !couldMatchDefaultRedactPatterns(exactRedacted)) {
    return exactRedacted;  // fast path: 无敏感关键字, 不走 default 模式
  }
  // ... full pattern walk
}
```

`service-attributes.ts:18-27` `redactOtelAttributes` 二次过滤:
```ts
export function redactOtelAttributes(attributes) {
  const redactedAttributes = {};
  for (const [key, value] of Object.entries(attributes)) {
    if (DROPPED_OTEL_ATTRIBUTE_KEYS.has(key)) continue;  // 黑名单字段直接 drop
    redactedAttributes[key] = typeof value === "string" ? redactSensitiveText(value) : value;
  }
  return redactedAttributes;
}
```

**4 个不同的 redact 模式** (`redact.ts:33-37` `RedactSensitiveMode = "off" | "tools"` + `redactToolPayloadTextWithConfig` 走 `"tools"`, `redactRegisteredSecretValues` 走精确字典, 27 类 `DEFAULT_REDACT_PATTERNS`)。**关键设计 (L37-40)**: 7 个常量 (min length=18, keep start=6, keep end=4) 控制**保留首尾, 中间打码**的格式 (e.g. `sk-1234…ABCD`)。

`redact.ts:82-83` 50+ 个 secret field keys 一行 regex 列出 (api_key, password, secret, private_key, refresh_token, …), `L102-105` env var 命名规则 (e.g. `*_KEY`, `*_SECRET`, `*_PASSWORD`); `L86-101` 4 个字符 `abcd-abcd-abcd-abcd` 形 Apple app-specific password 白名单 (`case/claw/demo/file/...` 9 个 benign words 不打码)。

`service-attributes.ts:90-110` `assignOtelLogAttribute` 增量写入 (检查 key 不在 `BLOCKED_OTEL_LOG_ATTRIBUTE_KEYS`, key 不被 redact, key 符合 `OTEL_LOG_ATTRIBUTE_KEY_RE` ASCII pattern, value 截到 `MAX_OTEL_LOG_ATTRIBUTE_VALUE_CHARS`) — 4 步防御, 每步失败静默 drop。

#### 3.4.7 content normalization (service-content-normalization.ts)

**L4-5**: `MAX_OTEL_CONTENT_ATTRIBUTE_CHARS = 128 * 1024` (128KB, 比 claudecode 60KB Honeycomb limit 大, 但比 default OTel 4MB 限制小); `MAX_OTEL_CONTENT_ARRAY_ITEMS = 200`。

**L86-97**: 9 步 JSON 截断 (从 `maxStringChars=8192` 一路降到 `32`, 从 `maxArrayItems=200` 一路降到 `1`, `maxDepth=8`, `maxObjectFields=64`) — 智能尝试找到能在预算内 stringify 的截断点。**L130-135**: 实在超限, 输出 `{truncated: true, reason: "max_attribute_size" | "unserializable_value", type: typeof value}` — 接收端知道是截断还是无法序列化。

**L107-148**: `safeJsonString` 把任何值 stringify, 经过 `stringifyJsonForOtelAttribute` (`JSON.stringify` + `redactSensitiveText`) 兜底; 长度超限时递归调用 `truncateJsonValueForOtelAttribute` 试 9×7=63 种截断组合。

#### 3.4.8 self-telemetry endpoint (src/infra/telemetry.ts:21-30)

`/usr/local/LsmGitOpenSource/openclaw/src/infra/telemetry.ts:21-30` — **OpenClaw 的自建 telemetry 独立于 OTel**, 仅做版本检查 + features (channels/providers/plugins/sessionsLast24h) 上报到 `https://telemetry.openclaw.ai/api/latest-version`。这是 **update check 为主, telemetry 为辅**: 

- **L86-90**: CI 环境检测, `isAutomatedEnvironment` 自动 disable
- **L101-104**: `DO_NOT_TRACK=1/true` 工业标准支持
- **L145-173**: 6 个 reason: enabled / automated-environment / do-not-track / config-disabled / never-asked / update-disabled
- **L175-230**: `buildTelemetryPayload` 含 5 维 features (`channels`, `providerFamilies`, `plugins`, `pluginsEnabled`, `sessionsLast24h`) — **注意 sessionsLast24h 来自 SQLite 直接查询 `session_state_events`**, 不通过 OTel。

---

### 3.5 opencode — Effect 框架 OTel + AI SDK experimental_telemetry

#### 3.5.1 Effect 框架集成 (packages/core/src/observability/otlp.ts)

```ts
export function loggers() {
  if (!endpoint) return []  // 零配置 = 零开销
  return [OtlpLogger.make({ url: `${endpoint}/v1/logs`, resource: resource(), headers })]
}

export async function tracingLayer() {
  if (!endpoint) return Layer.empty
  const NodeSdk = await import("@effect/opentelemetry/NodeSdk")
  // ...
  return NodeSdk.layer(() => ({
    resource: resource(),
    spanProcessor: new SdkBase.BatchSpanProcessor(new OTLP.OTLPTraceExporter({ url, headers }))
  }))
}
```

**关键设计 (L36-48)**: `resource()` 函数提供 5 个 resource attribute: `service.name=opencode, service.version=InstallationVersion, deployment.environment.name=InstallationChannel, opencode.client=Flag.OPENCODE_CLIENT, opencode.run=runID, service.instance.id=runID` — 4 个 runID 用于跨 trace 关联。

**L57-77** `tracingLayer` lazy import 4 个 OTel 包 (`@effect/opentelemetry/NodeSdk` / `@opentelemetry/exporter-trace-otlp-http` / `@opentelemetry/sdk-trace-base` / `@opentelemetry/context-async-hooks`) — 跟 claudecode 同款 dynamic import 节省冷启动。

**L63-66**: **Effect NodeSdk 不注册 global context manager, 但 AI SDK 用它 parent spans** — 显式 enable 一个 `AsyncLocalStorageContextManager` 兜底。

#### 3.5.2 AI SDK experimental_telemetry 桥接 (session/llm.ts:344-353)

```ts
experimental_telemetry: {
  isEnabled: cfg.experimental?.openTelemetry,  // 需要 experimental flag
  functionId: "session.llm",  // span name
  tracer: telemetryTracer,  // Proxy 注入 session.id attribute
  metadata: {
    userId: cfg.username ?? "unknown",
    sessionId: input.sessionID,
  },
}
```

`telemetryTracer` (L211-222) 是 **Proxy 拦截** `startSpan`, 注入 `session.id` attribute — 一个 OTel Tracer 的 5 行装饰器, 自动给所有 AI SDK 内部 span 加 `session.id` 维度, **不需要改 AI SDK 代码**。

#### 3.5.3 Effect Logging (packages/core/src/observability.ts)

`layer` 组合 `Logger.layer([...Logging.loggers(), ...Otlp.loggers()], { mergeWithExisting: false })` + `Layer.provide(NodeFileSystem.layer)` + `Layer.provide(OtlpSerialization.layerJson)` + `Layer.provide(FetchHttpClient.layer)`。Effect 的 Layer 组合让 OTel 与本地 console logger 并行输出。

**L11**: `import { OtlpSerialization } from "effect/unstable/observability"` — 标记 `unstable` 表示 API 会变, 但 Effect 团队承诺 1.0 之前稳定。

---

### 3.6 pi — 类型化 schema-driven telemetry (OTel-agnostic)

#### 3.6.1 抽象核心 (packages/telemetry/src/index.ts:1-30)

```ts
export interface TelemetryContext {
  startSpan<T>(options: SpanOptions, callback: (span: TelemetrySpan) => T | Promise<T>): Promise<T>;
}
export interface TelemetrySpan extends TelemetryContext {
  addEvent(name: string, attributes?: SpanAttributes): void;
  setAttributes(attributes: SpanAttributes): void;
  setStatus(status: SpanStatus): void;
}
```

**完全 OTel-agnostic** — 没有任何 OTel API import, **只暴露 3 个动词**: startSpan / setAttribute / setStatus。backend 可任意替换 (InMemory / OTel / Honeycomb / Lightstep)。

#### 3.6.2 类型化 schema (packages/agent/src/harness/telemetry.ts:42-572)

```ts
export const AI_TELEMETRY_SCHEMA = {
  version: 1,
  spans: {
    "pi.ai.request": { startAttributes: {...}, endAttributes: {...}, status: {...} },
    "pi.harness.run": { ... },
    "pi.harness.compaction": { ... },
    "pi.harness.navigation": { ... },
    "pi.harness.checkpoint": { ... },
    "pi.harness.turn": { ... },
    "pi.harness.step": { ... },
    "pi.harness.tool": { ... },
    "pi.harness.hook": { ... },
    "pi.harness.sleep": { ... },
    "pi.harness.event_handler": { ... },
    "pi.session.write": { ... },
  }
}
```

**核心创新**: `InferStartAttributes<...>` / `InferEndAttributes<...>` / `InferEventAttributes<...>` 3 个 type-level 工具把 schema 编译为 TS 类型, 配合 `ExactTelemetryAttributes<Expected, Actual>` 实现**编译期字段校验** — `startAiSpan("pi.ai.request", { pi.ai.operation: "stream", ... }, callback)` 写错字段名立即 compile error。

12 种 span 全部带:
- `parents: { kind: "any" | "root_or_external" | "spans": [...] }` — 父子关系约束
- `startAttributes: Record<string, TelemetryStartAttributeDefinition>` — required / optional / cardinality / values
- `endAttributes: Record<string, TelemetryAttributeDefinition>` — 全 optional completion enrichment
- `status: { default: "ok", errorWhen: "..." }` — 默认 ok, 何时视为 error

#### 3.6.3 In-Memory backend (packages/telemetry/src/memory.ts:1-219)

`InMemoryTelemetryContext` 是 reference implementation, 纯 process-memory 记录 — 没有 OTel 依赖, 没有外部 IO, 没有背压, **方便测试**:
- `getSpans()` 返回 `readonly RecordedTelemetrySpan[]` snapshot, 含 id/parentId/name/attributes/events/status/settled/endSequence
- `settleSpan` 幂等 (idempotent): 已 settled 的 span 不重 settle
- 错误自动 status: callback throw → `automaticErrorStatus(error)` (L78-87), 保留 `name + message` 不保留 stack (避免 stack 长度爆炸)
- 失败 graceful: "Recording is passive. Ignore malformed or unreadable telemetry payloads" (L143-145, 153-155, 162-164) — telemetry 自身错误永远不影响业务

#### 3.6.4 schema card metadata 字段

每个 attribute definition 带 `description` (生成 `telemetry-schema.md` 文档, `scripts/generate-telemetry-docs.ts` 自动生成) + 可选 `cardinality: "low" | "high"` (提示接收端在 BQ GROUP BY 时用低/高基数 column) + 可选 `sensitive` (这是当前 **未实现 runtime** 的, 只是 metadata 提示 — 改进点)。

#### 3.6.5 noop backend (packages/telemetry/src/noop.ts)

```ts
export const NOOP_TELEMETRY_CONTEXT: TelemetryContext = { startSpan: (_, cb) => Promise.resolve(cb({...})) };
```

12 行实现 — `pi` 在测试和 release 默认都用 noop, 用户显式接入 OTel backend 才生效。**这与 laew 当前** (`tracing` crate 14 行) **结构非常相似, 但 pi 有强类型 schema**。

---

### 3.7 undici — Node.js 原生 diagnostics_channel

#### 3.7.1 16 个 Channel 名 (lib/core/diagnostics.js:9-30)

```js
const channels = {
  // Client (4)
  beforeConnect: 'undici:client:beforeConnect',
  connected:     'undici:client:connected',
  connectError:  'undici:client:connectError',
  sendHeaders:   'undici:client:sendHeaders',
  // Request (7)
  create:           'undici:request:create',
  bodySent:         'undici:request:bodySent',
  bodyChunkSent:    'undici:request:bodyChunkSent',
  bodyChunkReceived:'undici:request:bodyChunkReceived',
  headers:          'undici:request:headers',
  trailers:         'undici:request:trailers',
  error:            'undici:request:error',
  // WebSocket (5)
  open:        'undici:websocket:open',
  close:       'undici:websocket:close',
  socketError: 'undici:websocket:socket_error',
  ping:        'undici:websocket:ping',
  pong:        'undici:websocket:pong',
  // Proxy (1)
  proxyConnected: 'undici:proxy:connected'
};
```

**关键设计**: undici **不实现 OTel**, 而是发布 Node.js `diagnostics_channel` 事件,**OTel 集成在用户层** (社区包 `opentelemetry-instrumentation-undici`)。这是 Node.js 生态的常规做法 — Node 核心模块不绑死 OTel。

#### 3.7.2 emit 时机 (lib/core/request.js:277-415)

```js
if (channels.create.hasSubscribers) channels.create.publish({ request: this });          // L277
if (channels.bodyChunkSent.hasSubscribers) channels.bodyChunkSent.publish({ request, chunk });  // L284
if (channels.bodySent.hasSubscribers) channels.bodySent.publish({ request });            // L297
if (channels.headers.hasSubscribers) channels.headers.publish({ request, response: { statusCode, headers, statusText } });  // L333
if (channels.bodyChunkReceived.hasSubscribers) channels.bodyChunkReceived.publish({ request, chunk });  // L358
if (channels.trailers.hasSubscribers) channels.trailers.publish({ request, trailers });  // L393
if (channels.error.hasSubscribers) channels.error.publish({ request, error });            // L415
```

**关键性能设计**: `if (channels.X.hasSubscribers)` — **零订阅者零开销**, Node.js `diagnostics_channel.channel(name).hasSubscribers` 是 O(1) 检查。**这是 laew 后续实现 in-process event bus 时必须复用的模式** (atomcode 的 `mpsc::channel` 也有 try_send 失败 + dropped counter 类似机制)。

#### 3.7.3 debug-only 内部订阅 (lib/core/diagnostics.js:51-227)

`trackClientEvents` / `trackRequestEvents` / `trackWebSocketEvents` 三个函数只在 `NODE_DEBUG=undici` / `NODE_DEBUG=fetch` / `NODE_DEBUG=websocket` 环境变量启用时, 把 channel 事件转 `util.debuglog(name)()`。**关键 idempotency 保护 (L57-60, 100-105, 158-164)**: `isTrackingClientEvents` 标志 + `hasSubscribers` 检查双保险, 防止 Node 内置 undici + 用户 undici 双订阅重复打印。

#### 3.7.4 类型契约 (types/diagnostics-channel.d.ts:1-74)

完整 TS namespace `DiagnosticsChannel` 描述每个 message 的字段 (`RequestCreateMessage` / `RequestBodySentMessage` / `RequestBodyChunkSentMessage` / `RequestHeadersMessage` 等), 配套 `Request` / `Response` / `ConnectParams` / `ClientSendHeadersMessage` 等 interface — 严格 typed 让用户写 listener 时 IDE 补全友好。

#### 3.7.5 undici 缺什么 vs Agent 系统需求

- **无 traceparent 注入** (这是 OpenTelemetry 库的活)
- **无 metrics** (HTTP 客户端不产生业务 metrics, 那是调用方的责任)
- **无脱敏** (debug log 印 URL + status code, 不会印 body)
- **无结构化日志** (debug log 是 text, 不是 JSON)

undici 是 **HTTP 客户端基础** 而非 **Agent 系统**, 它的 telemetry 仅是 hook 点, 不做完整 observability 解决方案。但 **laew 的 BashTool 缺类似 diagnostics_channel hook** (成功/失败/timeout/异常 capture) 是直接可借鉴的扩展点。

---

## 4. 横向对比大表

### 4.1 7 工程 × 7 维度

| 维度 | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi | undici |
|---|---|---|---|---|---|---|---|
| **OTel 协议支持** | ❌ 自建 HTTP+gzip | ✅ 9 组合 (gRPC/HTTP-JSON/HTTP-protobuf × console/otlp/prom) | ✅ 单 HTTP OTLP logs | ✅ HTTP protobuf + OWNED SDK 隔离 | ✅ Effect OtlpLogger + NodeSdk HTTP | ❌ 完全无 OTel | ❌ Node diagnostics_channel |
| **OTel 三栈** | ❌ | ✅ traces+logs+metrics | ⚠️ 仅 logs | ✅ traces+logs+metrics | ⚠️ traces+logs (metrics 通过 Effect) | ❌ | ❌ |
| **TraceContext 传播** | ❌ (无 trace) | ✅ W3C + beta lantern | ✅ Session-level (session.id+seq+parent) | ✅ 自建 W3C + 8 propagator | ✅ Effect Tracer | ❌ (仅 parentId int) | ❌ (无 trace) |
| **Span 命名规范** | envelope (5 字段) | `claude_code.interaction/llm_request/tool/...` (6 类) | `dsh-session-telemetry-otel/ops` (2 logger scope) | `openclaw.{harness,model,tool,message,...}.*` (50+) | `functionId="session.llm"` AI SDK | `pi.ai.request/harness.{run,turn,step,tool,hook,checkpoint,...}` (12 schema) | 无 (channel 名) |
| **决策审计** | `error_kind` + `error_data` 11 类 | `endLLMRequestSpan({success,statusCode,error,attempt})` + `gateway` 7 维度 | `agent-error` ops channel + `severity=error` | `openclaw.outcome` + `openclaw.deniedReason` + 7 `openclaw.errorCategory` | `onError` callback + `metadata.userId/sessionId` | schema `errorWhen` + `pi.error.code/type` | `undici:request:error` channel |
| **Token 度量** | `input_tokens/output_tokens/cached_tokens` 4 zone 分区 | `cachedInputTokens/uncachedInputTokens/cache_creation/cache_read` 4 维 + `globalCacheStrategy` | ❌ (无 metrics) | `gen_ai.client.token.usage` histogram (input/output/cache_read/cache_write/prompt/total) | ❌ (不直接暴露) | `pi.ai.usage.{input,output,cache_read,cache_write,reasoning,total,cost}` | ❌ |
| **脱敏** | regex 双向 (KV + 8 token shapes) | `OTEL_LOG_USER_PROMPTS` env + `redactIfDisabled` + 60KB 截断 + 哈希去重 | `session-telemetry/record` cordis waterfall extension | 27 DEFAULT_REDACT_PATTERNS + 7 mode 状态机 + redactOtelAttributes + blocked key list | Effect `Effect.logError` 链路无明确脱敏 | schema `sensitive` 标志 (metadata-only, runtime 未实现) | ❌ |
| **Metrics 名称** | `Counters` (events_tracked, events_dropped_mpsc/disk, segments_posted, bytes_sent) | counter/histogram/gauge + `BigQueryMetricsExporter` 5min 周期 | ❌ | 200+ `openclaw.*` counter/histogram (5 类) | Effect metrics via framework | ❌ (无 metrics) | ❌ |
| **优雅降级** | `Telemetry::init` fail 时返回 `enabled=false` dummy, 不抛错 | `try/catch` 包 `Beta tracing init` fail 走 default | `DROP_RECORD` 模式 + `assertNever` 失败 closed | `stopStarted` 6 步清理 + `AggregateError` 错误聚合 | `Layer.empty` (无 endpoint 时) | `NOOP_TELEMETRY_CONTEXT` 兜底 | `if (channels.X.hasSubscribers)` 零订阅零开销 |
| **磁盘缓冲** | NDJSON segment queue + fs2 lock + .owner marker | 1P 失败事件落盘 `1p_failed_events.{sessionId}.{batchUuid}.json` | ❌ (in-memory only) | ❌ (in-memory only) | ❌ (Effect 框架) | ❌ (in-memory only) | ❌ |
| **Shutdown 优雅** | 500ms grace, 401 hold 1h, 退避 2/8/30/120/300s | `Promise.race([all, telemetryTimeout(2000ms)])` 独立 chain | `Promise.race([shutdown, deadline(3000ms)])` | `stopStarted` 6 步全尝试 + `AggregateError` | `Layer.orDie` 后由 Effect shutdown chain | `settleSpan` 幂等 | N/A |
| **Console exporter** | 缺 (仅自建 NDJSON) | ✅ 5 段 (traces) / 60 段 (metrics) 周期 | ❌ (无) | ✅ stdout line JSON (writeStdoutDiagnosticLogRecord) | ❌ (无) | ❌ (无) | `util.debuglog('undici')` |
| **跨进程传播** | daemon+CLI 共享 queue, fs2 lock | N/A (单进程) | `parentSession` + `seedLength` 跨 session 拼接 | OpenClawPluginService 跨进程通过 `gateway` | N/A (单进程) | N/A (单进程) | N/A |
| **Schema 强类型** | ❌ (serde + JSON 弱类型) | ⚠️ 部分 (telemetryAttributes env 控 cardinality) | ⚠️ (typed record 但 loose attrs) | ❌ (stringly-typed attrs) | ⚠️ (Effect Schema) | ✅ 完全强类型 schema-driven (12 spans) | ✅ TS namespace `DiagnosticsChannel` |
| **API key 携带** | ❌ (post body 匿名) | `OTEL_EXPORTER_OTLP_HEADERS=k=v,k2=v2` + otelHeadersHelper 动态 header | SDK `OTLPExporterNodeConfigBase.headers` | `OTEL_EXPORTER_OTLP_HEADERS` 解析 | `OTEL_EXPORTER_OTLP_HEADERS` 解析 | N/A | N/A |
| **多租户/多环境** | `device_id` UUID per install | `user.account_uuid` + `user.email` + `org.id` | `user.id` (anonymous) | `openclaw.channel` 5 类 (gateway/cli) | `deployment.environment.name` | `pi.session.id` | 无 |
| **PII 字段白名单** | `repo_origin {host, has_git}` (不携带 URL) | `OTEL_METRICS_INCLUDE_*` 3 env | `attributes: identity minimal` | `openclaw.deniedReason` (string, 默认 "other") | schema `pi.*` 命名空间严格 | schema 字段名严格 | N/A |

### 4.2 laew 当前覆盖 vs 业界参考

| laew 现状 | 对应 7 工程参考 | laew 改造建议 |
|---|---|---|
| `tracing` crate 14 行 (atomcode-coding/src/telemetry.rs:15) | 6 工程 OTel SDK | 引入 `opentelemetry` + `opentelemetry-otlp` + `tracing-subscriber` |
| SQLite `session_memory` 摘要 | deepseek-harness ledger channel | 升级为 telemetry 记录, 6-event envelope 类似 atomcode |
| 0 决策审计 | 6 工程 endLLMRequestSpan / span attribute error data | LlmChat event 加 error_kind/error_data 6 类 |
| 0 脱敏 | atomcode scrub.rs (9 测试) + openclaw redactSensitiveText 27 类 | 在 LlmChat/ToolCall 序列化前 redact |
| 0 Token metrics | claudecode `gen_ai.client.token.usage` histogram | 引入 `MeterProvider` + histogram record |
| 0 TraceContext | 4 工程 W3C traceparent | 引入 `opentelemetry::global::get_text_map_propagator` 注入 `X-Session-Id` 旁 |
| `X-Session-Id` header (CLAUDE.md) | 4 工程 `user.id` / `session.id` attribute | 已经具备, 升级为 OTel attribute |
| 5 字段 device_id | `device_id` UUID per install (atomcode) | 复用 `Session` 的 `session_id` 作为 `user.id` |
| `Cargo build --release` 强编译 | `bun:bundle` feature flag dead code elimination | `tracing-subscriber` 分层 feature flag (dev/release) |

---

## 5. 共性模式与设计原则

### 5.1 5 个跨项目共性模式

1. **Envelope 模式** (atomcode/deepseek/claudecode/openclaw)
   - 每个 telemetry event 有最小不变 envelope: `device_id` + `session_id` + `ts` + `app_version` + `schema_version`
   - envelope 字段 skip_serializing_if = "Option::is_none" 减少噪音
   - laew 已经有 `device_id` + `session_id`, 缺 `app_version` + `schema_version` + `ts`

2. **4-5 级 opt-out** (atomcode 5 级 / openclaw 6 reason / claudecode GrowthBook gate)
   - 第 1 级: 编译期 (feature flag, 死代码消除)
   - 第 2 级: env (DO_NOT_TRACK / OTEL_SDK_DISABLED)
   - 第 3 级: 配置文件 (config.telemetry.enabled)
   - 第 4 级: CLI (--no-telemetry)
   - 第 5 级: 远程 (GrowthBook / OTLP server config)
   - laew 当前 0 级

3. **决策审计 3 段式** (claudecode / openclaw / deepseek)
   - Pre-decision: intent recognition (Yolo)
   - In-decision: tool approve/deny + error classification
   - Post-decision: cost attribution + error root cause
   - **每个 decision 必须有 `outcome` + `reason` + `attempt_count` 3 字段** (claudecode endLLMRequestSpan 范本)

4. **Token 4 维拆分** (claudecode 4 维 / openclaw 6 维 / pi 7 维)
   - 必须有: input / output / cache_read / cache_write
   - 推荐: prompt (系统提示) / reasoning (思考) / total
   - cost_usd (USD 计价, 6 tier 价格表)
   - laew 当前 0

5. **优雅降级 4 模式** (所有 7 工程)
   - 缺 config → `Layer.empty` (opencode) / `try_send` 失败 (atomcode) / `NOOP_TELEMETRY_CONTEXT` (pi) / `isAnalyticsDisabled` (claudecode)
   - 缺 endpoint → noop (claudecode `BETA_TRACING_ENDPOINT` 缺)
   - 缺 subscrber → 零开销 (undici `if (channels.X.hasSubscribers)`)
   - init 失败 → 静默 warn (claudecode Beta tracing fail)
   - laew 0 模式 (panic 风险高)

### 5.2 6 个关键设计原则

1. **OpenTelemetry Resource 优先 Resource, 非 per-record**: `service.name` / `service.version` / `user.id` 走 Resource(per-export), 不在每条 record 上重复 — atomcode `envelope.ts:31-65` / claudecode `instrumentation.ts:472-510` 严格分离
2. **计数器原子 + 持久化分离**: atomcode `Counters` AtomicU64 in-memory + `health.json` 落盘(2 个时间点: in-process 累计 + cross-restart 上次 post 时间)
3. **4xx/5xx 区别处理**: atomcode 400/413 永久错误 drop (不退避); 401 hold 1h; 5xx 走退避表; 429 parse retry-after
4. **shutdown 双重 timeout + race**: claudecode / deepseek 都用 `Promise.race([all, deadline])`, 且 provider.shutdown() promise 持续 observed 防止 unhandled rejection
5. **dyn import 节省冷启动**: claudecode `getOtlpReaders` 9 协议 dynamic import, ~1.2MB 不进 hot path; opencode 同样 dynamic import 4 个 OTel 包
6. **零订阅零开销**: undici `if (channels.X.hasSubscribers)`, claudecode `if (eventLogger) { ... return }`, **是 hot path 性能关键**

### 5.3 4 类 OTel 集成策略对比

| 策略 | 项目 | 优势 | 劣势 |
|---|---|---|---|
| **完整三栈直接 import** | claudecode, openclaw | 功能完整, 控制力强 | 1.2MB+ cold start 成本 |
| **dynamic import 按需** | claudecode (in getOtlpReaders), opencode | 节省冷启动 ~600ms | async 复杂度高 |
| **Effect/Layer 组合** | opencode | 框架统一, type-safe | Effect 生态锁定 |
| **完全 OTel-agnostic** | pi, atomcode | 零依赖, 可测试 | 接收端需要自己接 OTel |
| **Node.js diagnostics_channel hook** | undici | 零依赖, 通用 | 需要第三方包 `opentelemetry-instrumentation-undici` 桥接 |

**laew 建议**: 选 (2) dynamic import 策略, 因为 laew 是 CLI 工具, 冷启动延迟敏感(用户输入 → 第一次 LLM response 越快越好)。

---

## 6. 反模式与陷阱

### 6.1 5 个常见反模式

1. **❌ 在 envelope 里塞请求体** (3 项目栽过)
   - 表现: `error_data: "full stack trace"` 30KB
   - 反例: atomcode `error_data` 限 200 字符; claudecode `truncateContent` 限 60KB
   - laew 风险: SQLite `agent_memory` 表的 `error_data` 字段没限长

2. **❌ 同步 emit 阻塞 hot path**
   - 反例: undici `if (channels.X.hasSubscribers) channels.X.publish(...)` 是零成本 publish, **不阻塞**
   - 错例: 自己写 `mpsc::Sender::send` 是 async, 但**接收端 processing 慢会 backpressure**
   - laew 风险: `atomcode-coding/src/telemetry.rs:283-300` 用 `try_send` (non-blocking) ✅, **不能改为 blocking send**

3. **❌ 全局单例 OTel SDK** (claudecode 反思过)
   - 表现: 用户进程预加载 OTel SDK + 自己的 OTel, 两套 context manager 抢 global
   - 解决: openclaw `OwnedContextManager` ownership probe pattern (L120-150)
   - laew 风险: 如果用户在 `~/.bashrc` 配了 OTEL_SDK_DISABLED, 我们的 SDK 应该尊重

4. **❌ 把 error reason 当 string 透传** (claudecode 反思过)
   - 反例: `error_message: "401 invalid API key"` — 接收端没法 GROUP BY 401
   - 正解: `error_kind: "auth_error"` enum + `error_data: "401 invalid API key"` detail (claudecode `classifyAPIError` 11 类范本)
   - laew 风险: SQLite `agent_memory` 当前直接存 `error_message` 字符串

5. **❌ 同步 forceFlush + shutdown 串行** (claudecode 反思过)
   - 反例: `await forceFlush(); await shutdown();` 慢端点会 block 退出
   - 正解: `Promise.all([chain1, chain2, chain3]) + Promise.race([all, timeout])` 独立 chain
   - laew 风险: laew 是 TUI, `Ctrl+C` 时 5s 内必须退出

### 6.2 隐私泄漏 5 个 high-risk 模式

| 模式 | 风险 | 参考修复 |
|---|---|---|
| **user_prompt 全文写入 span attribute** | 用户的 API key / PII 泄漏到 OTLP server | claudecode `OTEL_LOG_USER_PROMPTS` env + `redactIfDisabled`; atomcode `redact_secrets` |
| **tool_result 写完整 stdout** | 大文件 / 密码 / private key 泄漏 | openclaw `truncateUtf16Safe` 128KB cap |
| **API key 直接拼 header** | OTLP server 误用 | `OTEL_EXPORTER_OTLP_HEADERS` env, 不进 telemetry |
| **环境变量写 attribute** | 泄漏 `*_API_KEY` 等 | openclaw `redactOtelAttributes` 黑名单 `DROPPED_OTEL_ATTRIBUTE_KEYS` |
| **panic stack 带 `<HOME>` 路径** | 泄漏用户名 | atomcode `scrub_path` 替换 `<HOME>`/`<CWD>` + `backtrace_top_k` 保留 basename |

### 6.3 性能陷阱 3 个

1. **❌ 每条 record 都创建 SpanContext**: claudecode `startInteractionSpan` 用 `WeakRef + AsyncLocalStorage` 防止 OTel context chain 持 span 阻 GC
2. **❌ 全局 mutex 持锁 emit**: atomcode `queue.append` 用 `BufWriter + file lock`, 但每条 record 不抢 — 内部 atomic counter
3. **❌ 高基数 attribute** (claudecode 反思过): `session.id` 默认关闭 metrics 维度 (`OTEL_METRICS_INCLUDE_SESSION_ID` env 控制), 避免 BQ GROUP BY 爆

---

## 7. 对 laew 的 P0/P1/P2 路线图

### 7.1 P0 (必须立刻做)

| ID | 工作 | Rust crate 建议 | 参考项目 |
|---|---|---|---|
| **L17** | 引入 OTel SDK + OTLP HTTP traces+logs+metrics | `opentelemetry = "0.27"`, `opentelemetry-otlp = "0.27"`, `opentelemetry_sdk = "0.27"`, `tracing = "0.1"`, `tracing-subscriber = "0.3"`, `tracing-opentelemetry = "0.28"` | claudecode/openclaw/opencode |
| **L17a** | 5 级 opt-out: env (DO_NOT_TRACK/LAEW_OTEL_SDK_DISABLED) → config (Sqlite `telemetry.enabled`) → CLI (--no-telemetry) → remote (HTTP check) | 复用现有 `config/mod.rs::Db` | atomcode 5 级 + openclaw 6 reason |
| **L17b** | LlmChat event envelope: device_id (UUID per install 落 `~/.laew/device_id`) + session_id (已存在) + turn_id + ts + schema_version=1 + app_version + os/arch + provider/model + mode | 自实现, 仿 `event.rs:31-65` | atomcode envelope |
| **L17c** | ToolCall event: name, success, duration_ms, error_kind (Bash 8 类, Read 4 类, Write 5 类), error_data | 自实现 | atomcode ToolCall |
| **L18** | 决策审计三段: endLLMRequestSpan 携带 (success, status_code, error_kind, attempt, gateway) | OTel span attribute | claudecode `endLLMRequestSpan` |
| **L19** | 脱敏 redact_secrets: KV pass (auth/secret/api_key) + 8 token shapes (GH/GL/AWS/Slack/OpenAI/JWT) | `regex` + 2 个 `OnceLock<Regex>` | atomcode `scrub.rs:23-44` 9 测试 |
| **L20** | Token metrics: counter `claude.laew.tokens{kind=input/output/cache_read/cache_write}` + histogram `claude.laew.tokens.distribution` | OTel `MeterProvider` | openclaw `service-metrics.ts:65-82` |
| **L21** | W3C TraceContext 注入 LLM HTTP 请求: `traceparent: 00-{32hex}-{16hex}-01` | `opentelemetry::global::get_text_map_propagator` | openclaw `diagnostic-trace-context.ts` |

### 7.2 P1 (3 个月内)

| ID | 工作 | Rust crate 建议 | 参考项目 |
|---|---|---|---|
| **L22** | Metrics opt-out 双层缓存: 内存 1h + 磁盘 24h, 服务检查 HTTP endpoint | `reqwest = "0.12"`, `tokio = "1"`, 复用 `config::Db` | claudecode `metricsOptOut.ts:35-80` |
| **L22a** | Console exporter (stdout JSON line) 用于 `--debug` 模式 | 自实现 | openclaw `writeStdoutDiagnosticLogRecord` |
| **L22b** | shutdown 双重 timeout: `Promise.race([shutdown, deadline])`, 默认 2s, 可配 `LAEW_OTEL_SHUTDOWN_TIMEOUT_MS` | `tokio::time::timeout` | claudecode `instrumentation.ts:533-560` |
| **L22c** | Dynamic import OTel 包节省冷启动 ~600ms | `std::any::Any` 替代 dyn import, 编译期 feature flag `telemetry` (default off) | claudecode `getOtlpReaders` dynamic import |
| **L22d** | OWNED SDK ownership probe (防双 OTel SDK 冲突) | `Symbol.for` 不可用, 改用 `OnceCell<()>` + 进程内 mutex | openclaw `OwnedContextManager` |
| **L23** | 6 个常用 dashboard: (1) token 趋势 (2) error rate (3) tool duration p50/p95 (4) cache hit rate (5) decision reason 分布 (6) session 时长 | Grafana template + JSON | 各项目通用 |
| **L24** | Panic 捕获 + scrub 路径 + truncate message | `std::panic::set_hook` + `regex` + `truncate_head` 200 字符 | atomcode `Panic` event |

### 7.3 P2 (6-12 个月)

| ID | 工作 | Rust crate 建议 | 参考项目 |
|---|---|---|---|
| **L25** | BigQuery / ClickHouse / Honeycomb 适配器 (1P mode 旁路) | `reqwest` + `gcp-bigquery-client` 或 `clickhouse-rs` | claudecode `FirstPartyEventLoggingExporter` |
| **L25a** | 决策审计持久化: SQLite `decision_audit` 表, 6 month retention, 与 OTLP 同步 | 复用 `config::Db` | deepseek-harness ledger channel |
| **L25b** | GenAI 语义约定完整覆盖: `gen_ai.provider.name/request.model/usage.input_tokens/client.operation.duration` | OTel semconv `opentelemetry-semantic-conventions = "0.27"` | claudecode/openclaw 范本 |
| **L25c** | Content capture policy: 7 维 boolean (input/output/tool_inputs/tool_outputs/system/tool_definitions/log_bodies) + 默认全关 + 单独命令开启 | 自实现 | openclaw `resolveContentCapturePolicy` |
| **L25d** | 弱网环境磁盘缓冲: NDJSON segment queue (类似 atomcode) + fs2 file lock | `tokio = "1"`, `tokio::sync::mpsc`, `fs2 = "0.4"`, `flate2 = "1"` | atomcode `queue/mod.rs` |

### 7.4 关键 crate 推荐 (Cargo.toml 增量)

```toml
# P0 - 核心
opentelemetry = "0.27"
opentelemetry_sdk = { version = "0.27", features = ["rt-tokio"] }
opentelemetry-otlp = { version = "0.27", features = ["tonic", "grpc-tonic", "http-proto", "reqwest-client"] }
opentelemetry-semantic-conventions = "0.27"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
tracing-opentelemetry = "0.28"
regex = "1.10"
once_cell = "1.20"
uuid = { version = "1.10", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
flate2 = "1.0"           # 压缩 NDJSON segment
fs2 = "0.4"              # 文件锁
filetime = "0.2"         # segment mtime
serde_json = "1.0"

# P1 - 增强
reqwest = { version = "0.12", features = ["json", "gzip", "stream"] }
tokio = { version = "1", features = ["full"] }
tokio-util = "0.7"
```

### 7.5 实施顺序 (4 步)

1. **第 1 步 (1-2 周)**: 引入 `tracing` + `tracing-subscriber` + `tracing-opentelemetry`, JSON 格式日志, Console + OTLP HTTP 双 exporter。先 logs 通道, 不动 metrics/traces。
2. **第 2 步 (2-3 周)**: LlmChat/ToolCall event envelope 自实现 (NDJSON 落盘 + background sender), 5 级 opt-out, scrub 脱敏。**参考 atomcode 完整 6-event 模型**。
3. **第 3 步 (3-4 周)**: OTel metrics (counter/histogram) + W3C traceparent 注入 LLM HTTP 头。**W3C 注入要复用现有 `X-Session-Id` 逻辑**。
4. **第 4 步 (4-6 周)**: 决策审计 (error_kind enum + decision reason span attribute) + BigQuery 适配器 (可选) + Grafana dashboard 模板。

---

## 8. 附录: 关键代码路径速查表

### 8.1 atomcode (Rust)

| 路径 | 行数 | 角色 |
|---|---|---|
| `crates/atomcode-telemetry/src/lib.rs` | 26 | crate 入口 |
| `crates/atomcode-telemetry/src/event.rs` | 868 | 6-event schema + Envelope |
| `crates/atomcode-telemetry/src/config.rs` | 205 | 4 级 opt-out |
| `crates/atomcode-telemetry/src/identity.rs` | 101 | device_id 持久化 |
| `crates/atomcode-telemetry/src/scrub.rs` | 207 | regex 双向脱敏 (9 测试) |
| `crates/atomcode-telemetry/src/runtime.rs` | 1090 | Telemetry handle + CurrentContext + Counters |
| `crates/atomcode-telemetry/src/queue/mod.rs` | 1187 | NDJSON segment queue (fs2 lock + .owner marker) |
| `crates/atomcode-telemetry/src/queue/roll.rs` | 38 | roll 策略 |
| `crates/atomcode-telemetry/src/sender/mod.rs` | 159 | SenderRuntime + backoff 2/8/30/120/300s |
| `crates/atomcode-telemetry/src/sender/http.rs` | 96 | HTTP POST gzipped + 4xx/5xx 状态映射 |
| `crates/atomcode-coding/src/telemetry.rs` | 1106 | TelemetryHook + ToolTelemetryMiddleware + MeteredProvider |
| `crates/atomcode-cli/src/telemetry_cmd.rs` | 222 | `telemetry status/enable/disable/dump/clear/recover` |
| `crates/atomcode-daemon/src/telemetry_scope.rs` | 28 | daemon_shared CurrentContext wrapper |
| `docs/telemetry.md` | 145 | 公开文档 (事件列表 + opt-out + 不收集清单) |

### 8.2 claudecode (TS/Bun)

| 路径 | 行数 | 角色 |
|---|---|---|
| `src/utils/telemetry/instrumentation.ts` | 825 | OTel 三栈初始化 + 9 协议 dynamic import |
| `src/utils/telemetry/sessionTracing.ts` | 927 | 5 种 SpanType + WeakRef + ALS |
| `src/utils/telemetry/betaSessionTracing.ts` | 491 | hash 去重 + 60KB 截断 + new_context |
| `src/utils/telemetry/perfettoTracing.ts` | 1120 | Chrome Trace Event 格式 + agent hierarchy |
| `src/utils/telemetry/pluginTelemetry.ts` | 289 | plugin_id_hash + 双列隐私 |
| `src/utils/telemetry/bigqueryExporter.ts` | 252 | BigQuery metrics 旁路 |
| `src/utils/telemetry/events.ts` | 75 | logOTelEvent 主入口 |
| `src/utils/telemetry/logger.ts` | 26 | ClaudeCodeDiagLogger |
| `src/utils/telemetry/skillLoadedEvent.ts` | 39 | skill 上报 |
| `src/utils/telemetryAttributes.ts` | 71 | 5 级 attribute 控制 (3 env cardinality) |
| `src/services/api/metricsOptOut.ts` | 159 | 24h disk + 1h memory 双层缓存 |
| `src/services/api/logging.ts` | 788 | logAPIError 7-gateway 检测 + 11 类 error classification |
| `src/services/analytics/firstPartyEventLogger.ts` | 449 | 1P event logger (BatchLogRecordProcessor) |
| `src/services/analytics/firstPartyEventLoggingExporter.ts` | 806 | BigQuery /api/event_logging/batch 8 次 backoff |
| `src/services/analytics/growthbook.ts` | 1155 | 远程动态配置 |
| `src/services/analytics/metadata.ts` | 973 | getEventMetadata 核心 |

### 8.3 deepseek-harness (TS+Py+Native)

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/session/session-telemetry/src/index.ts` | 177 | Service Definition (接口 + 类型) |
| `packages/session/session-telemetry/src/coordinator.ts` | 319 | capture coordinator + cursor + chunk projection |
| `packages/session/session-telemetry/src/invariant.ts` | 32 | invariant contract |
| `packages/session/session-telemetry-otel/src/index.ts` | 301 | Service Provider (LoggerProvider + BatchLogRecordProcessor + OTLPLogExporter) |
| `packages/session/session-telemetry-otel/src/invariant.ts` | (32) | invariant contract |
| `docs/subsystems/session-telemetry.md` | — | capture 端 + backend 端 + redaction waterfall 完整文档 |
| `docs/subsystems/session-telemetry.zh.md` | — | 中文版 |

### 8.4 openclaw (TS)

| 路径 | 行数 | 角色 |
|---|---|---|
| `extensions/diagnostics-otel/src/service.ts` | 693 | Plugin 主入口 + 6 步 stopStarted |
| `extensions/diagnostics-otel/src/service-propagation.ts` | 139 | OWNED ContextManager + 8 propagator |
| `extensions/diagnostics-otel/src/service-traces.ts` | 417 | trace runtime + retained trusted span contexts |
| `extensions/diagnostics-otel/src/service-trace-context.ts` | 92 | normalizeTraceContext 工具 |
| `extensions/diagnostics-otel/src/service-metrics.ts` | 326 | 100+ counter/histogram 工厂 |
| `extensions/diagnostics-otel/src/service-attributes.ts` | 290 | redactOtelAttributes + DROPPED_OTEL_ATTRIBUTE_KEYS 黑名单 |
| `extensions/diagnostics-otel/src/service-content-normalization.ts` | 241 | JSON 9 步截断 + 128KB cap |
| `extensions/diagnostics-otel/src/service-constants.ts` | 88 | OTEL env 名 + 桶边界 |
| `extensions/diagnostics-otel/src/service-logs.ts` | 265 | 日志 exporter |
| `extensions/diagnostics-otel/src/service-exporter.ts` | 235 | OTLP HTTP protobuf + 错误分类 |
| `extensions/diagnostics-otel/src/service-exporter-health.ts` | 211 | exporter 健康状态 |
| `extensions/diagnostics-otel/src/service-recorders-harness.ts` | 247 | harness.run recorder |
| `extensions/diagnostics-otel/src/service-recorders-model.ts` | 168 | model.* recorder (gen_ai 语义) |
| `extensions/diagnostics-otel/src/service-recorders-tools.ts` | 400 | tool.execution.started/completed/error/blocked 4 步 |
| `extensions/diagnostics-otel/src/service-recorders-operations.ts` | 423 | operations recorder (payload/liveness/export) |
| `extensions/diagnostics-otel/src/service-recorders-usage.ts` | 398 | usage recorder (6 token 维度 + cost) |
| `extensions/diagnostics-otel/src/service-recorder-runtime.ts` | 19 | 公共 helpers |
| `src/infra/telemetry.ts` | 317 | 自建 telemetry (version check + features payload) |
| `src/infra/diagnostic-trace-context.ts` | 247 | 自建 W3C trace context (32/16 hex) |
| `src/infra/diagnostic-trace-propagation.ts` | 158 | 自建 bridge 协议 (跨 SDK 翻译) |
| `src/logging/redact.ts` | 1000+ | redactSensitiveText + 27 DEFAULT_REDACT_PATTERNS + 7 mode 状态机 |
| `src/plugin-sdk/security-runtime.ts` | 61 | 安全 runtime (redactSensitiveText re-export) |
| `src/agents/sessions/telemetry.ts` | 17 | session-level telemetry re-export |
| `qa/scenarios/observability/otel-*.yaml` | 4 个 | OTel 端到端 QA 场景 |

### 8.5 opencode (TS/Bun)

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/core/src/observability.ts` | 24 | Effect Layer 组装 |
| `packages/core/src/observability/otlp.ts` | 79 | OtlpLogger.make + NodeSdk.layer (HTTP traces/logs) |
| `packages/opencode/src/session/llm.ts` | 387+ | AI SDK experimental_telemetry 桥接 (5 行 Proxy) |
| `packages/opencode/src/agent/agent.ts` | — | `OtelTracer.OtelTracer` service option |

### 8.6 pi (TS)

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/telemetry/src/index.ts` | 357 | OTel-agnostic 接口 (startSpan/setAttribute/setStatus) |
| `packages/telemetry/src/memory.ts` | 219 | InMemoryTelemetryContext (reference impl) |
| `packages/telemetry/src/noop.ts` | 20 | NOOP_TELEMETRY_CONTEXT (12 行) |
| `packages/telemetry/test/telemetry.test.ts` | — | telemetry test |
| `packages/telemetry/test/conformance.test.ts` | — | schema conformance test |
| `packages/agent/src/harness/telemetry.ts` | 615 | 12-span schema (AI_TELEMETRY_SCHEMA + HARNESS_TELEMETRY_SCHEMA) |
| `packages/agent/scripts/generate-telemetry-docs.ts` | — | 自动生成 telemetry-schema.md |
| `packages/agent/docs/telemetry-schema.md` | — | 自动生成的 schema 文档 |

### 8.7 undici (JS)

| 路径 | 行数 | 角色 |
|---|---|---|
| `lib/core/diagnostics.js` | 227 | 16 个 channel + `NODE_DEBUG=undici` 内部 debug 订阅 |
| `lib/core/request.js` | — | 7 个 emit 点 (L277/283/296/332/357/392/414) `if (channels.X.hasSubscribers)` |
| `lib/core/connect.js` | — | client 4 channel 触发点 |
| `lib/web/websocket/*.js` | — | websocket 5 channel 触发点 |
| `lib/dispatcher/proxy-agent.js` | — | proxyConnected channel |
| `types/diagnostics-channel.d.ts` | 74 | 完整 TS namespace 契约 |

### 8.8 laew 现有可改造点

| 路径 | 行数 | 改造方向 |
|---|---|---|
| `src/main.rs` | — | 加 `--no-telemetry` CLI flag |
| `src/tui/mod.rs` | — | TUI 横幅显示 "Telemetry: enabled/disabled (reason)" |
| `src/agent/mod.rs` | — | LlmChat event 集成 |
| `src/agent/tools/{bash,read,write}.rs` | — | ToolCall event 集成 |
| `src/agent/yolo.rs` | — | 任务分类 + 决策审计 |
| `src/config/mod.rs` | — | 加 `telemetry` 配置 (Db `telemetry_state` 表) |
| `src/session.rs` | — | session_id 已有, 加 telemetry context 关联 |
| `src/llm/mod.rs` | — | W3C traceparent 注入 LLM HTTP 头 |
| `src/llm/anthropic.rs` | — | OTel HTTP OTLP traces |
| `src/llm/openai.rs` | — | 同上 |
| `Cargo.toml` | — | 加 OTel + tracing crate |
| `build.rs` | — | `LAEW_BUILD_TIME` 已有, 加 `LAEW_GIT_HASH` (CLAUDE.md 提到) |
| `docs/CLAUDE.md` | — | 文档同步 |

---

## 9. 关键发现总结 (1-page digest)

1. **laew 当前可观测性 = 0**: 仅 `tracing` crate 14 行 + SQLite 摘要, 没有 OTel, 没有 metrics, 没有决策审计, 没有脱敏。
2. **6 个工程都用 OTel** (claudecode / openclaw / opencode 完整三栈, deepseek-harness 仅 logs, atomcode 自建 HTTP+gzip, pi 完全 OTel-agnostic schema-driven, undici Node diagnostics_channel)。
3. **决策审计 3 段** (claudecode 范本): LLM start 携 (model, querySource, attempt), LLM error 携 (status_code, error_kind, gateway), LLM end 携 (duration, token_usage, cache_strategy)。
4. **脱敏 4 层**: (a) regex 双向 (atomcode)、(b) emit 端 redact (claudecode)、(c) attribute 黑名单 (openclaw)、(d) content capture policy 7 维 boolean (openclaw)。
5. **Token 4 维必选** (claudecode 范本): input / output / cache_read / cache_write + `gen_ai.client.token.usage` histogram + `gen_ai.client.operation.duration` histogram。
6. **W3C traceparent 32/16 hex** 是 4 工程事实标准 (claudecode/openclaw 显式, opencode 透传 AI SDK, deepseek 通过 session.id+seq 自建)。
7. **dynamic import 节省冷启动** ~600ms (claudecode `getOtlpReaders` 9 协议, opencode 4 OTel 包)。
8. **shutdown 双重 timeout + 独立 chain** (claudecode/deepseek 范本): `Promise.race([all, deadline(2-3s)])` 防止慢 OTLP 端点 block 退出。
9. **OWNED SDK 隔离** (openclaw 范本): `Symbol.for + Object.defineProperty` globalThis singleton + ownership probe, 防双 OTel SDK 冲突。
10. **laew 改造 P0 1-6 周** (4 步): tracing+OTel → NDJSON LlmChat/ToolCall envelope → 5 级 opt-out + scrub → OTel metrics + W3C traceparent → 决策审计 + BigQuery 适配器。

---

**字数统计**: 本专题约 1450 行 (含表格) / ~52 KB Markdown, 覆盖 7 工程 × 7 维度, 8 个附录代码路径速查表, 25+ 真实代码行号锚点。
