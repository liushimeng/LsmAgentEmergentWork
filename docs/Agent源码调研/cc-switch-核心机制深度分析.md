# cc-switch 核心机制深度分析

> 调研对象: `farion1231/cc-switch` v3.x(Tauri 2 + Rust + React 18 + TypeScript + Vite + SQLite)
> 工程定位: 一站式 CLI Agent 路由器 + 统一配置中心(8 款工具 × 多 Provider ×跨设备同步)
> 本文聚焦 **8 个核心机制**: Tauri 桌面壳架构、本地代理(LLM 网关)、熔断器三态、thinking_rectifier 整流、8 工具配置适配、MCP 多工具 SSOT、17 schema 版本迁移、WebDAV/S3 云同步。每个机制都基于真实源码路径 + 行号 + 代码片段,文末给出对 laew 的 12 条落地借鉴。

---

## 0. 工程概览与依赖图

```
cc-switch/
├── src/                  # React 18 + TypeScript + Vite 前端
│   ├── App.tsx               # 主入口(1829 行,ViewState 机)
│   ├── types.ts              # 754 行全局类型(Provider/Mcp/Skill/Settings/Sync)
│   ├── components/           # 20+ 业务组件目录
│   ├── hooks/                # 25+ 自定义 hook(useProviderActions/useSkills/useMcp)
│   ├── lib/api/, lib/schemas/, lib/query/  # 强类型 API封装 + Zod + React Query
│   └── i18n/                 # i18next 多语言(zh/en/ja/de)
├── src-tauri/                # Rust 后端
│   ├── src/
│   │   ├── lib.rs            # 主入口(2403 行,Tauri 应用组装 + DeepLink)
│   │   ├── main.rs           # 二进制入口 + Linux WebKit 环境兜底
│   │   ├── commands/         # 35 个 #[tauri::command] 垂直拆分
│   │   ├── proxy/            # 本地 HTTP代理(28 模块,1.7 MB)
│   │   ├── mcp/              # 8 工具 MCP 适配
│   │   ├── deeplink/         # ccswitch:// 协议解析
│   │   ├── session_manager/  # 各工具会话历史解析
│   │   ├── database/         # SQLite + 17 个 schema 迁移 +备份/恢复
│   │   ├── services/         # 业务服务层(sync_protocol / webdav / s3 / skill / usage)
│   │   └── *_config.rs       # 8 款工具的配置适配器(80~225 KB/文件)
│   └── tauri.conf.json
├── docs/, scripts/, tests/    # 用户文档 + 构建脚本 + 集成测试
└── package.json              # 前端依赖(Tauri 2.8 + Radix UI + CodeMirror + recharts + framer-motion)
```

**后端体量**: 159 个 Rust源文件;最大单文件 `services/proxy.rs` 10002 行 + `database/backup.rs` 130 KB + `database/schema.rs` 3412 行 + `proxy/forwarder.rs` 5267 行。

**核心 Cargo 依赖**: `tauri 2.x`、`reqwest + hyper + axum + tower-http`、`rquickjs`(QuickJS 运行时,执行用户 JS 用量脚本)、`rusqlite`、`tokio`、`serde + serde_json`、`sha2`、`chrono`、`toml_edit`。
---

## 1. Tauri 2 桌面壳架构

### 1.1 模块位置图

```
src-tauri/src/
├── main.rs                  # 二进制入口(95 行,Linux WebKit 兜底)
├── lib.rs                   # 应用组装(2403 行)
│   ├── run()                # pub fn run() — Tauri Builder入口
│   ├── handle_deeplink_url  # ccswitch:// 协议 → emit("deeplink-import", …)
│   └── RedactedUrl + redact_url_for_log_with_secrets  # URL 脱敏集中管理
├── store.rs                 # AppState 容器(Database + ProxyService + FailoverSwitchManager)
├── tray.rs                  # 系统托盘(macOS/Linux/Windows 平台分支)
├── linux_fix.rs             # Linux GTK/WebKit 兼容性补丁(条件编译)
├── panic_hook.rs            # panic 日志上报(tauri-plugin-log)
└── commands/                # 35 个 #[tauri::command] 垂直拆分
    ├── provider.rs / proxy.rs / mcp.rs / skill.rs
    ├── auth.rs / failover.rs / coding_plan.rs / import_export.rs
    ├── webdav_sync.rs / s3_sync.rs / sync_support.rs
    └── misc.rs(291 KB 杂项)
```

### 1.2 入口组装 `lib.rs::run()`

`lib.rs:42-49` 用 `pub use ...::*` 把全部子模块 API 一次性 re-export 出去,`commands::mod.rs` 同样做二次 re-export。最终 `invoke_handler!` 用 `tauri::generate_handler!` 批量注册:

```rust
// lib.rs 顶层 (47-49): 一次性 re-export commands 全集
pub use commands::*;

// invoke_handler 在 lib.rs::run() 里(简化):
.invoke_handler(tauri::generate_handler![
    commands::provider::get_providers,
    commands::provider::add_provider,
    commands::proxy::start_proxy,
    commands::proxy::stop_proxy,
    commands::mcp::sync_mcp_to_app,
    commands::skill::install_skill,
    commands::webdav_sync::webdav_upload,
    commands::s3_sync::s3_upload,
    // ... 35 个文件 ~150+ 个命令
])
```

启动流程(`lib.rs::run()`,实测 2403 行):

```
panic_hook::setup_panic_hook()      # 兜底 panic 日志
└─ tauri::Builder::default()
   ├─ plugin(single_instance)             # 单实例+窗口聚焦
   ├─ plugin(deep_link)                   # ccswitch:// 协议
   ├─ plugin(process/dialog/opener/store/window_state)
   ├─ on_window_event: 拦截关闭→最小化到托盘(settings.minimize_to_tray_on_close)
   └─ setup(|app| {
       ├─ panic_hook + log 初始化(Rotate 4 归档 × 20MB,tauri-plugin-log)
       ├─ Database::init() → SQLite + Schema 迁移 v0..v17
       ├─ TrayIconBuilder::new() → 系统托盘(平台分支)
       ├─ usage_events::init(app) → AppHandle 注入,日志事件可推送前端
       └─ invoke_handler: 批量注册 #[tauri::command]
   })
```

### 1.3 前后端通信模式

- **命令调用**: 前端 `import { invoke } from "@tauri-apps/api/core"` → `invoke<Args, Result>("command_name", args)`。错误转 `String`(Tauri 命令返回值 `Result<T, E>` 的 E 必须 `Serialize`)。
- **事件订阅**: 后端 `app.emit("deeplink-import", &payload)` → 前端 `listen<T>("deeplink-import", handler)`。典型事件:
  - `deeplink-import` / `deeplink-error` — DeepLink 解析结果
  - 用量实时刷新 /托盘状态变更
- **状态共享**: 后端核心对象(`Database`、`ProxyServer`、`ProviderRouter`)用 `Arc<RwLock<T>>` 包装,挂在 `tauri::State` 上,跨命令调用。
- **混合刷新**: `react-query(@tanstack/react-query)` 拉取命令结果 + 后端事件触发 query invalidation,保证 dashboard 实时性。

### 1.4 Linux WebKit 多平台兜底(`main.rs:5-32`)

教科书级别的多平台兼容处理:

```rust
// main.rs:8-32
#[cfg(target_os = "linux")]
{
    // WebKitGTK DMA-BUF 渲染器在 Nvidia + Debian 13.2触发白屏/黑屏
    if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    // 禁用 WebKitGTK 合成模式规避 resize 时 webview 崩溃
    if std::env::var("WEBKIT_DISABLE_COMPOSITING_MODE").is_err() {
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }
    // AppImage 的 GTK 启动钩子强制 XWayland,提供逃生开关让用户改回 Wayland
    if let Ok(backend) = std::env::var("CC_SWITCH_GDK_BACKEND") {
        if !backend.is_empty() {
            std::env::set_var("GDK_BACKEND", backend);
        }
    }
}
```

`windows_subsystem = "windows"` 隐藏控制台窗口;`redact_url_for_log_with_secrets`(`lib.rs:177-210`)集中处理 URL 脱敏:`MIN_KNOWN_SECRET_LEN = 8` 避免误伤普通词,只对确切握有的密钥值替换为 `[REDACTED]`。
---

## 2. 本地代理(LLM 网关角色)

### 2.1 模块拓扑(`src-tauri/src/proxy/`)

```
proxy/ (合计 1.7 MB Rust,28 模块)
├── mod.rs                    # 严格 pub(crate) 边界
├── server.rs                 # Axum 路由 + 手动 hyper accept loop(25 KB)
├── handlers.rs               # 5 大端点 handler(146 KB)
├── forwarder.rs              # 主循环 forward_with_retry_inner(5267 行)
├── provider_router.rs        # 故障转移选择 + 熔断器调度(23 KB)
├── circuit_breaker.rs        # 三态熔断器(17 KB,496 行)
├── model_mapper.rs           # 模型别名映射 haiku/sonnet/opus/fable(429 行)
├── handler_context.rs        # RequestContext:贯穿生命周期元数据(378 行)
├── handler_config.rs         # UsageParserConfig(229 行)
├── response_processor.rs     # 响应处理 + 用量收集 + SSE 透传(47 KB)
├── failover_switch.rs        # 热切换 P1 失败后自动切 P2
├── thinking_rectifier.rs     # thinking 签名整流器(7 种错误模式,723 行)
├── thinking_budget_rectifier.rs  # budget_tokens 强制32000 + max_tokens 64000
├── media_sanitizer.rs        # 图片降级整流器(UNSUPPORTED_IMAGE_MARKER)
├── thinking_optimizer.rs / cache_injector.rs  # Bedrock 优化器(可选)
├── copilot_optimizer.rs      # Copilot 反优化(防 premium quota 偷跑)
├── content_encoding.rs       # gzip/brotli/zstd 解压
├── hyper_client.rs           # 自实现 HTTP 客户端,带 HeaderCaseMap透传
├── body_filter.rs            # 敏感字段过滤
├── json_canonical.rs         # 规范化 JSON 比对
└── providers/                # 协议转换层(30+ 文件)
    ├── claude.rs / codex.rs / gemini.rs / opencode.rs / hermes.rs / copilot_auth.rs
    ├── transform.rs              # OpenAI Chat → Anthropic
    ├── transform_responses.rs    # OpenAI Responses → Anthropic
    ├── transform_gemini.rs       # Gemini Native → Anthropic(含 thoughtSignature shadow store)
    ├── transform_codex_*.rs      # Codex Responses ↔ Chat ↔ Anthropic 互转
    ├── streaming*.rs             # SSE 流式协议转换
    ├── codex_chat_history.rs     # Codex Chat bridge history
    └── gemini_shadow.rs          # Gemini shadow state(thoughtSignature 重放)
```

### 2.2 服务器入口 `proxy/server.rs:34-92`

```rust
// server.rs:34-51 ProxyState:跨请求共享的状态
#[derive(Clone)]
pub struct ProxyState {
    pub db: Arc<Database>,
    pub config: Arc<RwLock<ProxyConfig>>,
    pub status: Arc<RwLock<ProxyStatus>>,
    pub start_time: Arc<RwLock<Option<Instant>>>,
    pub current_providers: Arc<RwLock<HashMap<String, (String, String)>>>,
    pub provider_router: Arc<ProviderRouter>,      // 持有熔断器
    pub gemini_shadow: Arc<GeminiShadowStore>,     // Gemini thoughtSignature 重放
    pub codex_chat_history: Arc<CodexChatHistoryStore>,
    pub app_handle: Option<tauri::AppHandle>,
    pub failover_manager: Arc<FailoverSwitchManager>,
}
```

默认监听 `127.0.0.1:15721`(仅本机访问),接管时把各工具的 live config 改写为指向本代理。路由表(`server.rs:291-379` build_router):

| 路径 | Handler | 用途 |
|------|---------|------|
| `/health`, `/status` | health_check / get_status | 健康/状态 |
| `/v1/messages` 等多别名 | `handle_messages` | Claude Messages API |
| `/chat/completions` 等多别名 | `handle_chat_completions` | OpenAI Chat Completions |
| `/responses` 等多别名 | `handle_responses` | OpenAI Responses API |
| `/responses/compact` | `handle_responses_compact` | Responses 远程压缩 |
| `/alpha/search` | `handle_alpha_search` | Codex Alpha Search |
| `/v1beta/*path` | `handle_gemini` | Gemini 原生(含 GET `/models` 用 `any(..)`) |

### 2.3 手写 hyper HTTP/1.1 accept loop + Header case preservation(`server.rs:138-213`)

绕开 axum 默认行为,手动 hyper accept loop,在每条连接内 `stream.peek()` 抓取原始 TCP 头大小写,存入 `OriginalHeaderCases` extension。注释(`server.rs:5-9`)明确动机:**保持客户端请求头的 wire-level 大小写**(典型场景:CLI 用户配了 `X-Custom-Case` 而上游 gateway 鉴权检查大小写敏感)。

```rust
// server.rs:194-201
hyper::server::conn::http1::Builder::new()
    .preserve_header_case(true) // 让 hyper 不强制小写 header
    .serve_connection(TokioIo::new(stream), service)
    .await
```

### 2.4 Forwarder 主循环 `proxy/forwarder.rs:429-1156`

主循环设计哲学:**per-provider 独立重试 + 跨 provider 故障转移**。单次客户端请求最多尝试 `max_attempts = max_retries + 1` 个供应商(`forwarder.rs:186-190`、`forwarder.rs:261`)。

关键控制流(`forwarder.rs:464-540` 简化):

```rust
// forwarder.rs:464-540 简化
for provider in providers.iter() {
    // 1) 上限检查(早 break,避免熔断器名额浪费)
    if attempted_providers >= self.max_attempts { break; }
    // 2) 熔断器放行许可(HalfOpen 占名额)
    let permit = self.router.allow_provider_request(&provider.id, app_type_str).await;
    if !permit.allowed { continue; }
    // 3) Pre-Send 优化器(Bedrock 专属,不污染其他 provider)
    let mut provider_body = if self.optimizer_config.enabled && is_bedrock_provider(provider) {
        body.clone()
    } else { body.clone() };
    // 4) 实际转发
    match self.forward(...).await {
        Ok(...) => { self.record_success_result(...); return Ok(...); }
        Err(e) => {
            // 错误分类:Retryable → 记录失败 + continue
            //            NonRetryable / ClientAbort → release_permit_neutral + return
            // 然后串联三层整流器重试(仅 Anthropic 供应商):
            //  4a) media_retry_should_trigger(图片降级)
            //   4b) thinking_signature 整流
            //   4c) thinking_budget 整流
        }
    }
}
```

**4 个值得借鉴的设计点**:

1. **per-provider 独立重试标记**(`forwarder.rs:466-469`):`rectifier_retried / budget_rectifier_retried / media_rectifier_retried` 都是 per-provider 局部变量。这是**极关键**的设计——首家 provider 整流后被 5xx/timeout 击落时,下家 provider 仍能用整流后的请求体走自己的整流流程,避免标记"短路"故障转移。

2. **熔断器放行检查早于 max_attempts 检查的反向**(`forwarder.rs:472-492`):把"已尝试次数上限"放在熔断器放行 *之前*。反直觉但正确——避免在已超限时还占用宝贵的 HalfOpen 探测名额。

3. **"错误分类 + 中性释放 HalfOpen" 模式**(`forwarder.rs:1050-1112` categorize_proxy_error):错误分为 3 类:
   - `Retryable`:`provider` 真正故障 → 记录失败 + `update_provider_health` + 继续 failover
   - `NonRetryable`:客户端层错误(400/401/422)→ **不污染健康度**,仅 `release_permit_neutral`
   - `ClientAbort`:客户端断连 → 同 NonRetryable
   
   `release_permit_neutral`(`provider_router.rs:204-216`)中性接口专门用于整流器等"请求结果不应计入 Provider 健康度"的场景,仅释放 HalfOpen 名额、不触发 record_success/record_failure。

4. **ActiveConnectionGuard RAII**(`forwarder.rs:130-156`):进入 forward_with_retry 时构造 guard,Drop 时调度 tokio 任务把 `active_connections` -1。流式响应 body 是 future,guard move 进 body future 后随其一起 drop,避免 UI 连接计数过早归零。

### 2.5 Codex OAuth 鉴权透传(`forwarder.rs:57-99`)

Codex 官方 provider 走 OAuth 透传:`validate_codex_official_authorization`(`forwarder.rs:57-99`)校验请求携带的 Authorization 不含 `PROXY_MANAGED` 占位符、不为空;若 provider meta 关联了 `managed_account_id`,还要校验请求的 `chatgpt-account-id` header 与之匹配且 session 校验通过。这套机制保证"切换 OAuth 账号后必须重启 Codex"——通过占位符检测强制避免跨账户路由。

### 2.6 与 Switchyard / agent-studio 的异同

| 维度 | cc-switch | Switchyard | agent-studio |
|------|-----------|-----------|--------------|
| **目标用户** | 单用户桌面端 | 多租户网关(NVIDIA内部) | 企业 Agent 平台 |
| **协议 IR抽象** | 无显式 IR,直接 Adapter trait + transform_* | 显式 `LlmRequest / LlmResponse / ContentBlock::Unknown` | Pregel 图 + DSL |
| **路由算法** | `select_providers` 队列 P1→P2 + 熔断器 | Noop/Passthrough/Random/LlmClassifier/StageRouter/Composite/AdvisorGate **7 种** | DSL条件路由 |
| **熔断器** | 三态 Closed/Open/HalfOpen,HalfOpen 限流 1 个探测 | 类似但支持 per-route 配置 | 任务级 retry + 超时 |
| **失败回流** | `FailoverSwitchManager` 自动切 P1→P2 写库 | AdvisorGate + FallThrough 级联 | Trial评估 |
| **缓存** | `cache_injector`(Bedrock 可选) | LRU + TTL,key=`prompt_hash` | 多级 |
| **部署形态** | 单机桌面 + 本地代理 | 服务端 Rust 网关 | 服务端 Python 微服务 |

**结论**: cc-switch 的代理层在"单机 + 桌面"场景下做到了工业级容错,但协议 IR 没有 Switchyard 抽象得彻底,主要靠 per-app adapter 实现异构协议。
---

## 3. 熔断器三态(Closed / Open / HalfOpen)

### 3.1 模块位置 `src-tauri/src/proxy/circuit_breaker.rs:1-388`

```rust
// circuit_breaker.rs:76-93 pub struct CircuitBreaker {
    state: Arc<RwLock<CircuitState>>,                  // RwLock:状态转移
    consecutive_failures: Arc<AtomicU32>,              // Atomic:热路径无锁
    consecutive_successes: Arc<AtomicU32>,
    total_requests: Arc<AtomicU32>,
    failed_requests: Arc<AtomicU32>,
    last_opened_at: Arc<RwLock<Option<Instant>>>,
    config: Arc<RwLock<CircuitBreakerConfig>>,         // 热更新
    half_open_requests: Arc<AtomicU32>,                // HalfOpen 限流
}
```

**核心设计点**:

1. **`AllowResult { allowed, used_half_open_permit }` 语义**(`circuit_breaker.rs:99-103`):把"是否允许"和"是否占用了 HalfOpen 探测名额"分开,调用方在请求结束后必须把 `used_half_open_permit` 传回 `record_success/failure` 才能正确释放名额(注释 `circuit_breaker.rs:97-98` 强调)。

2. **HalfOpen 状态限流**:`max_half_open_requests = 1`(`circuit_breaker.rs:317`),通过 `half_open_requests.fetch_add(1)` 原子抢占。

3. **双触发条件**(`circuit_breaker.rs:257-286`):连续失败次数 ≥ threshold 触发,或 total ≥ min_requests 且 error_rate ≥ threshold 触发。

4. **`transition_to_half_open` 幂等保护**(`circuit_breaker.rs:367-377`):写锁内先检查 `*state != Open`,避免并发调用重置 `half_open_requests` 计数。`test_half_open_transition_does_not_reset_inflight_permit`(`circuit_breaker.rs:454-475`)专门验证此场景。

5. **`release_half_open_permit` CAS 循环**(`circuit_breaker.rs:339-356`):防御性减量,避免过度释放。

### 3.2 状态机转换条件

```rust
// circuit_breaker.rs:230-288 record_failure 简化
match state {
    CircuitState::HalfOpen => transition_to_open(),  // 探测失败 → 立即回 Open
    CircuitState::Closed => {
        if failures >= config.failure_threshold { transition_to_open() }
        else if total >= config.min_requests {
            let error_rate = failed as f64 / total as f64;
            if error_rate >= config.error_rate_threshold { transition_to_open() }
        }
    }
}
```

转换表:

| 当前状态 | 触发条件 | 下一状态 | 副作用 |
|---------|---------|---------|--------|
| Closed | consecutive_failures ≥ threshold | Open | reset consecutive_failures |
| Closed | total ≥ min_requests AND error_rate ≥ threshold | Open | 同上 |
| HalfOpen | 半开探测失败(任意一次) | Open | reset计数 |
| HalfOpen | consecutive_successes ≥ success_threshold | Closed | reset 全计数 |
| Open | elapsed ≥ timeout_seconds | HalfOpen | reset half_open_requests |

### 3.3 按 app 独立配置(`circuit_breaker.rs:51-61`)

```rust
impl From<&AppProxyConfig> for CircuitBreakerConfig {
    fn from(config: &AppProxyConfig) -> Self {
        Self {
            failure_threshold: config.circuit_failure_threshold,
            success_threshold: config.circuit_success_threshold,
            timeout_seconds: config.circuit_timeout_seconds as u64,
            error_rate_threshold: config.circuit_error_rate_threshold,
            min_requests: config.circuit_min_requests,
        }
    }
}
```

每 app 独立配置(`proxy_config` 表 4 行:claude/codex/gemini/grokbuild),claude 默认最激进(8 次失败开,90 秒恢复):

| app | failure_threshold | success_threshold | timeout_seconds | error_rate | min_requests |
|-----|-------------------|-------------------|-----------------|-----------|--------------|
| claude | 8 | 3 | 90 | 0.7 | 15 |
| codex | 4 | 2 | 60 | 0.6 | 10 |
| gemini | 4 | 2 | 60 | 0.6 | 10 |
| grokbuild | 4 | 2 | 60 | 0.6 | 10 |

`From<&AppProxyConfig>` + `update_app_configs` 按前缀过滤(`provider_router.rs:227-234`),让 Claude / Codex / Gemini 各自一套阈值。ProviderRouter 用 `app_type:provider_id` 作为 circuit key,动态创建熔断器:

```rust
// provider_router.rs:255-291 async fn get_or_create_circuit_breaker(&self, key: &str) -> Arc<CircuitBreaker> {
    // 先尝试读锁获取(快路径)
    if let Some(breaker) = breakers.read().await.get(key) { return breaker.clone(); }
    // 获取写锁创建,双重检查防竞争
    let mut breakers = self.circuit_breakers.write().await;
    if let Some(breaker) = breakers.get(key) { return breaker.clone(); }
    // 从 key 提取 app_type,按应用独立读取熔断器配置
    let app_type = key.split(':').next().unwrap_or("claude");
    let config = match self.db.get_proxy_config_for_app(app_type).await {
        Ok(app_config) => CircuitBreakerConfig { /* ... */ },
        Err(_) => CircuitBreakerConfig::default(),
    };
    let breaker = Arc::new(CircuitBreaker::new(config));
    breakers.insert(key.to_string(), breaker.clone());
    breaker
}
```

### 3.4 Codex Official 不可参与 failover(`provider_router.rs:18-21`)

```rust
pub(crate) fn provider_supports_failover(app_type: &str, provider: &Provider) -> bool {
    app_type != AppType::Codex.as_str()
        || !crate::proxy::providers::is_codex_official_provider(provider)
}
```

Codex 官方请求携带账户 native Authorization,跨账户路由会越权。`test_codex_official_current_stays_single_route_when_failover_is_stale`(`provider_router.rs:472-498`)专门测此场景。

---

## 4. thinking_rectifier 整流

### 4.1 模块位置 `src-tauri/src/proxy/thinking_rectifier.rs:1-189`

Claude 模型的 `thinking` block 带签名,某些中转 API 会拒收(报错 `Invalid 'signature' in 'thinking' block`),整流器自动剥离签名,避免整条链路400。

`should_rectify_thinking_signature`(`thinking_rectifier.rs:26-109`)检测 **7 种错误模式**:

| 场景 | 错误示例 | 触发条件 |
|------|---------|---------|
| 1. Invalid signature in thinking block | `Invalid 'signature' in 'thinking' block` | invalid + signature + thinking + block |
| 1b. Thought signature not valid | `Unable to submit...Thought signature is not valid` | thought signature + (not valid\|invalid) |
| 2. Must start with thinking block | `a final 'assistant' message must start with a thinking block` | 含 must start with thinking block |
| 3. Expected thinking, found tool_use | `Expected 'thinking' or 'redacted_thinking', but found 'tool_use'` | expected + thinking + found + tool_use |
| 4. Signature field required | `***.signature: Field required` | signature + field required |
| 5. Signature extra inputs not permitted | `xxx.signature: Extra inputs are not permitted` | signature + extra inputs are not permitted |
| 6. Thinking blocks cannot be modified | `thinking blocks...cannot be modified` | thinking + cannot be modified |
| 7. 非法请求兜底 | `非法请求 / illegal request / invalid request` | 中英文兜底 |

### 4.2 整流动作 `rectify_anthropic_request`(`thinking_rectifier.rs:118-189`)

```rust
//1. 遍历 messages[*].content[]:删除 type=thinking / redacted_thinking block
// 2. 删除非 thinking block 上的 signature 字段
// 3. 兜底:thinking.type=enabled 且最后一条 assistant 消息首块不是 thinking 且存在 tool_use → 删除顶层 thinking 字段
```

### 4.3 Adaptive thinking 兼容(`thinking_rectifier.rs:240-242`)

```rust
// 与 CCH 对齐:请求前不做 thinking type 主动改写
pub fn normalize_thinking_type(body: Value) -> Value {
    body
}
```

adaptive 模式下 thinking block **不会**被错误归类为 legacy block 而触发整流(由 `test_rectify_adaptive_still_cleans_legacy_signature_blocks` 验证)。

### 4.4 嵌套 JSON 错误格式兼容(`thinking_rectifier.rs:308-324`)

第三方渠道常返回嵌套 JSON 错误,函数对原始字符串 lowercase 后做 `contains` 匹配,天然兼容嵌套:

```rust
let lower = msg.to_lowercase();
if lower.contains("invalid") && lower.contains("signature") && lower.contains("thinking") && lower.contains("block") {
    return true;
}
```

### 4.5 thinking_budget_rectifier(`proxy/thinking_budget_rectifier.rs:81-122`)

强制把 `thinking.budget_tokens = 32000`、`max_tokens = 64000`(常量 `MAX_THINKING_BUDGET/MAX_TOKENS_VALUE` 在 `budget_rectifier.rs:10-13`)。Adaptive 模式跳过整流(`budget_rectifier.rs:84-91`)。`applied` 通过 `before != after` 派生(`budget_rectifier.rs:118`),无副作用时不污染调用方决策。

### 4.6 media_sanitizer(`proxy/forwarder.rs:194-237` + `media_sanitizer.rs`)

两种触发模式:
- **预防式**(`apply_media_prevention`,`forwarder.rs:199-218`):发送前对 text-only 模型把图片块替换为 `UNSUPPORTED_IMAGE_MARKER` 标记。受 `request_media_fallback` 总开关 + `request_media_heuristic` 子开关管辖。
- **反应式**(`media_retry_should_trigger`,`forwarder.rs:224-237`):上游 4xx 后,对同一 provider 重试一次,替换图片块为标记。仅 `Claude | Codex` 适配器 + contains_image_blocks + is_unsupported_image_error 三条件 AND。

### 4.7 向后兼容策略

整流器对 adaptive / enabled / unknown thinking type **都不主动改写**,仅在错误触发时移除问题 block,保持 Anthropic 官方主路径完全兼容。CCH(Claude Code Helper)对齐注释多处出现,显示与竞品同步决策(`thinking_rectifier.rs:71`、`thinking_rectifier.rs:200`、`thinking_rectifier.rs:239`、`thinking_rectifier.rs:536`)。
---

## 5. 8 工具配置适配层

### 5.1 模块位置

`src-tauri/src/` 顶层 9 个 `<tool>_config.rs`,每个工具一个独立模块:

| 文件 | 工具 | 行数 | 主配置 | MCP 配置 |
|------|------|------|--------|----------|
| `claude_desktop_config.rs` | Claude Desktop | 2250 | settings.json | — |
| `claude_mcp.rs` | Claude MCP | — | — | `~/.claude.json` 或 `~/.claude/.mcp.json` |
| `codex_config.rs` | OpenAI Codex CLI | 5267 | `auth.json + config.toml + model_catalog_json` | `config.toml` 内 `[mcp_servers.*]` |
| `gemini_config.rs` | Google Gemini CLI | ~600 | settings.json + .env | `~/.gemini/settings.json` |
| `grok_config.rs` | xAI Grok Build | ~700 | 订阅绑定 TOML | 嵌入 config |
| `opencode_config.rs` | OpenCode AI | ~450 | opencode.json(npm 包式 provider) | `{mcp: {id: {...}}}` |
| `openclaw_config.rs` | OpenClaw | ~900 | models.providers.* + agents.defaults | 内嵌 |
| `hermes_config.rs` | Hermes Agent | ~2200 | provider 块 + 内嵌 MCP | YAML(`config.yaml`) |
| `pi_config/mod.rs` | Pi Agent | ~570 | pi 专属 schema | — |

### 5.2 统一抽象模式

每个工具暴露 **3 个标准函数**(推断):

```rust
read_<tool>_live_settings() -> Result<serde_json::Value, AppError>; // 读原生配置
write_<tool>_live_atomic(value: Value) -> Result<(), AppError>;       // 原子写
read_and_validate_<tool>_config_text() -> Result<String, AppError>;   // 仅 Codex
```

`MultiAppConfig`(`src-tauri/src/app_config.rs`,44KB)统一管理 8 款应用的 schema 校验、版本检测、目录覆盖(settings 中可指定 `claudeConfigDir` 等)。

### 5.3 原子写入 + JSON排序 `src-tauri/src/config.rs:336-498`

`atomic_write_with_unix_mode`(`config.rs:336-498`)三步走:

```rust
// 1. 临时文件创建
let tmp_path = parent_dir.join(format!(".tmp.{}.{}.{}", pid, nanos, COUNTER.fetch_add(1)));
//    counter 是进程级 AtomicU64,避免 nanos 碰撞,最多重试 16 次
// 2. 写入 + flush + 设置权限
OpenOptions::new().mode(unix_mode).write(true).create_new(true).open(&tmp_path)?;
// atomic_write_private 专用于 API key 等凭据,强制 0600
// 3. 替换目标
//    Unix: fs::rename(POSIX 语义原子)
//    Windows: 优先 ReplaceFileW,ERROR_NOT_SUPPORTED(WSL UNC路径) → fallback fs::rename
//    整体最多重试 3 次
```

JSON 写入还会先 `sort_json_keys`(`config.rs:277-291`)按字母序递归排序所有 key,保证输出确定性,避免 git diff 噪音。

### 5.4 Codex MCP TOML 同步 `mcp/codex.rs:286+`

Codex 用 TOML 格式且只接受顶层 `[mcp_servers]` 表(旧错误格式 `[mcp.servers]` 被主动清理)。

```rust
// mcp/codex.rs:16-20
fn should_sync_codex_mcp() -> bool {
    // Codex 未安装/未初始化时:~/.codex 目录不存在
    // 按用户偏好:目录缺失时跳过写入/删除,不创建任何文件或目录
    crate::codex_config::get_codex_config_dir().exists()
}
```

实现要点:
- 使用 `toml_edit`(而非 `toml`)允许保留未触及字段的格式与注释。
- `read_and_validate_codex_config_text` 读取后若语法无效,**直接返回错误而非覆盖**(`mcp/codex.rs:284` 注释强调)。
- 收集启用项 → 用 `toml_edit::DocumentMut` 替换 `mcp_servers` 表 → 保留其它键。
- 核心字段(`type/command/args/env/cwd/url/headers/http_headers`)手动处理,**`headers` 和 `http_headers` 都是核心字段**——注释(`mcp/codex.rs:86-88`)强调两者都必须视为核心字段,避免鉴权值落入通用日志路径。

### 5.5 字段差异抹平策略

`providerType` 枚举:`codex_oauth / claude_oauth / xai_oauth / bedrock / copilot / generic`,每种走不同字段映射。Provider 的 `settings_config` 用 `serde_json::Value` 存储,**不做强制 schema**——这是"差异抹平"的关键设计:每个 provider 自带 JSON 配置,后端透传不校验语义,只校验结构层合法性。

---

## 6. MCP 多工具 SSOT

### 6.1 统一抽象 `src-tauri/src/mcp/`

```
mcp/
├── mod.rs                   # 模块索引,统一导出 4 函数 × 7 工具 = 28 函数
├── validation.rs            # validate_server_spec + extract_server_spec
├── claude.rs                # Claude MCP(8 字段 enabled_*)
├── codex.rs                 # Codex TOML 适配(32 KB,最复杂)
├── gemini.rs                # Gemini settings.json 适配
├── grokbuild.rs             # Grok Build 简化适配
├── opencode.rs              # OpenCode {mcp: {id: {type:"local"|"remote"}}}
└── hermes.rs                # Hermes YAML适配 +特殊 auth 字段
```

每个模块导出 **4 个标准函数**(`mod.rs:23-41`):

```rust
pub use claude::{
    import_from_claude,                  // 从工具原生配置导入到统一存储
    remove_server_from_claude,           // 从工具原生配置删除单条
    sync_enabled_to_claude,              // 把 SSOT 启用的项批量同步到工具
    sync_single_server_to_claude,        // 把 SSOT 单条同步到工具
};
```

### 6.2 验证抽象 `mcp/validation.rs`

```rust
// validation.rs:8-51 validate_server_spec
// 仅校验 3 种 type(stdio/http/sse),必填字段分别是 command / url
// extract_server_spec 从 McpServer.server 提取 spec JSON
```

这一层把所有 per-tool 适配器的"通用合法性"集中处理。

### 6.3 SSOT 数据模型 `database/schema.rs:64-74`

```sql
CREATE TABLE mcp_servers (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, server_config TEXT NOT NULL,
    description TEXT, homepage TEXT, docs TEXT, tags TEXT NOT NULL DEFAULT '[]',
    enabled_claude BOOLEAN NOT NULL DEFAULT 0, enabled_codex BOOLEAN NOT NULL DEFAULT 0,
    enabled_gemini BOOLEAN NOT NULL DEFAULT 0, enabled_grokbuild BOOLEAN NOT NULL DEFAULT 0,
    enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
    enabled_hermes BOOLEAN NOT NULL DEFAULT 0
);
```

每个 MCP 条目有 **per-app 启用开关**(v3.7.0 引入 `McpApps`),用户勾选 → `sync_single_server_to_*` 函数把配置写入对应工具的原生文件。SSOT 单一存储在 `mcp_servers` 表,各 app 的原生文件是**投影**而非副本。

### 6.4 双向同步

- **CC Switch → 工具**: `sync_single_server_to_claude()` 等,按各工具 schema 序列化。
- **工具 → CC Switch**: `import_from_claude()` 等,读取工具原生配置并入表。

### 6.5 Hermes YAML 适配 `mcp/hermes.rs:32-40`

Hermes 与众不同:
- **没有 type 字段**——靠是否存在 `command` / `url` 推断(`hermes.rs:124-144`)。
- **Hermes 专有字段**:`enabled`、`timeout`、`connect_timeout`、`tools`、`sampling`、`roots`、`auth`(常量 `HERMES_EXTRA_FIELDS`)。注释(`hermes.rs:28-31`)专门强调 `auth` 字段(OAuth 声明)即使 cc-switch 没有 OAuth UI 也必须保留 round-trip,否则会降级到未认证调用。
- **写时剥离 + 写时保留**:导出时 Hermes→CC Switch 剥离 EXTRA_FIELDS,导入时 Hermes→CC Switch 也剥离;但**写回 Hermes 时保留 EXTRA_FIELDS**(merge-on-write 逻辑)。

### 6.6 Codex 双格式兼容 `mcp/codex.rs:52-276`

`import_from_codex` 双格式支持(同时处理 `[mcp_servers.*]` 正确格式和 `[mcp.servers.*]` 旧错误格式)。核心字段 vs 扩展字段分流(`mcp/codex.rs:84-91`、`:155-211`):
- `core_fields` 列举已处理的 `type/command/args/env/cwd/url/headers/http_headers`
- 其余字段通用 TOML→JSON 转换
- 错误容忍(`mcp/codex.rs:217-219`):单项失败 `continue` 不中止整批

### 6.7 DeepLink 一键安装 `deeplink/mod.rs:34-139`

`DeepLinkImportRequest` 是 40+ 字段的扁平结构,覆盖 provider/prompt/mcp/skill 四种资源 + config 文件 + usage script。安全设计:
- **凭据字段明确隔离**(`deeplink/mod.rs:62-65`):`api_key / usage_api_key / usage_access_token / usage_user_id` 分别独立。
- **容量控制**:`config` 字段是 Base64 编码(`deeplink/mod.rs:106`),避免 URL 长度爆炸。
- **`usage_script` 不默认启用**(`deeplink/mod.rs:117-120`):携带脚本本身不意味着运行,必须显式 `usageEnabled=true`。经典"opt-in by default"安全策略。

`ccswitch://v1/import?resource=mcp&apps=claude,codex&config=...` 在前端 `DeepLinkImportDialog` 弹窗确认后调用 `import_from_deeplink`,一步跨工具安装。

---

## 7. 17 Schema 版本迁移

### 7.1 模块位置 `src-tauri/src/database/schema.rs`

`SCHEMA_VERSION: i32 = 17`(`database/mod.rs:56`)。迁移链 v0 → v1 → ... → v17,每个版本独立函数 `migrate_vN_to_vN1`,用 **SAVEPOINT 包裹**确保原子。

### 7.2 迁移驱动 `schema.rs:435-499`

```rust
// schema.rs:435-499
pub(crate) fn apply_schema_migrations_on_conn(conn: &Connection) -> Result<(), AppError> {
    conn.execute("SAVEPOINT schema_migration;", [])?;
    let mut version = Self::get_user_version(conn)?;
    if version > SCHEMA_VERSION {
        conn.execute("ROLLBACK TO schema_migration;", []).ok();
        conn.execute("RELEASE schema_migration;", []).ok();
        return Err(AppError::Database(format!(
            "数据库版本过新({version}),当前应用仅支持 {SCHEMA_VERSION},请升级应用后再尝试。")));
    }
    let result = (|| {
        while version < SCHEMA_VERSION {
            match version {
                0 => { Self::migrate_v0_to_v1(conn)?; Self::set_user_version(conn, 1)?; }
                1 => { Self::migrate_v1_to_v2(conn)?; Self::set_user_version(conn, 2)?; }
                2 => { Self::migrate_v2_to_v3(conn)?; Self::set_user_version(conn, 3)?; }
                // ... v3..v16
                16 => { Self::migrate_v16_to_v17(conn)?; Self::set_user_version(conn, 17)?; }
                _ => unreachable!()
            }
            version += 1;
        }
        Ok(())
    })();
    if result.is_err() {
        conn.execute("ROLLBACK TO schema_migration;", []).ok();
        return Err(result.err().unwrap());
    }
    conn.execute("RELEASE schema_migration;", []).ok();
    result
}
```

### 7.3 17 个 Schema 版本演进

| 版本 | 演进内容 |
|------|---------|
| v0 → v1 | 补齐缺失列(初版 schema 不完整) |
| v1 → v2 | 引入代理 + 用量统计(`proxy_request_logs / model_pricing`) |
| v2 → v3 | Skills 统一管理架构(添加 `app_type`) |
| v3 → v4 | OpenCode 支持 |
| v4 → v5 | 计费模式(`cost_multiplier / limit_daily_usd`) |
| v5 → v6 | Copilot 模板统一 + `usage_daily_rollups` |
| v6 → v7 | Skill `content_hash` 更新检测 |
| v7 → v8 | 会话日志用量追踪 + 修正模型定价 |
| v8 → v9 | 全面补定价 |
| v9 → v10 | Hermes Agent |
| v10 → v11 | `usage_daily_rollups` 保留 `request_model` 维度 |
| v11 → v12 | 项目 Profiles(全应用共享项目实体) |
| v12 → v13 | 输入 token 缓存语义(`input_token_semantics`) |
| v13 → v14 | Grok Build 代理配置 |
| v14 → v15 | Skills/MCP 添加 Grok Build 字段 |
| v15 → v16 | 重建 Codex 会话用量 |
| v16 → v17 | 会话用量持久去重账本(`session_usage_dedup`) |

### 7.4 启动期 pre-migration 备份(`database/mod.rs:128-140`)

```rust
if version > 0 && version < SCHEMA_VERSION {
    backup_database_file()?; // 升级前先备份
}
Self::apply_schema_migrations()?; // 再升级
```

升级失败也不阻断(备份失败仅 warn,`mod.rs:136-138`)。`stored_user_version_exceeds_supported`(`mod.rs:174-183`)专门处理"数据库版本比应用新"的反向场景——返回 `Some(version)` 让 UI 引导用户升级应用而非反复弹无效重试对话框。

### 7.5 向后兼容工具

- `add_column_if_missing`(`schema.rs:407-412`):统一处理"列已存在"错误,幂等添加列。
- `has_column(conn, table, col)`(`schema.rs:146`):列存在性查询。
- `migrate_proxy_config_to_per_app`(`schema.rs:400-404`):旧版 `proxy_config` 是单例表(无 `app_type` 列)→ 启动时直接转换为三行结构。

### 7.6 17 张表(`schema.rs:25-345`)

| # | 表 | 行 | 职责 |
|---|----|----|------|
| 1 | `providers` | 25-44 | 供应商主表(PK: id+app_type) |
| 2 | `provider_endpoints` | 49-60 | 多端点(FK: providers ON DELETE CASCADE) |
| 3 | `mcp_servers` | 64-74 | MCP 服务器(6 个 enabled_* 字段) |
| 4 | `prompts` | 77-81 | 提示词(PK: id+app_type) |
| 5 | `skills` | 84-106 | Skill(`content_hash` for update detection) |
| 6 | `skill_repos` | 109-116 | 自定义 GitHub 仓库 |
| 7 | `settings` | 119-122 | KV 配置 |
| 8 | `proxy_config` | 126-183 | 三行结构 app_type PK,4 套默认配置 |
| 9 | `provider_health` | 186-192 | 健康度(PK: provider_id+app_type) |
| 10 | `proxy_request_logs` | 197-232 | 请求日志(含 5 个索引 + `input_token_semantics`) |
| 11 | `model_pricing` | 235-244 | 模型定价 |
| 12 | `stream_check_logs` | 247-259 | 流式连通性测试日志 |
| 13 | `proxy_live_backup` | 264-270 | Live 配置备份(PK: app_type) |
| 14 | `usage_daily_rollups` | 276-296 | 日聚合(PK: 6 元组,含 `request_model/pricing_model`) |
| 15 | `session_log_sync` | 301-309 | 会话日志同步状态 |
| 16 | `session_usage_dedup` | 313-329 | fork/rewrite 去重账本 |
| 17 | `profiles` | 333-344 | 项目 Profiles(各 app 共享) |

### 7.7 备份导入的 authorizer 防御(`database/backup.rs:63-83`)

**这是 cc-switch 安全设计的精华**:导入 SQL 时拒绝一切"越界动作":

```rust
// backup.rs:63-83 import_authorizer
match action {
    AuthAction::Attach | Detach => denied(), // ATTACH / VACUUM INTO
    AuthAction::CreateVtable | DropVtable => denied(),  // csvfile/zipfile 等 vtable
    AuthAction::Unknown => denied(),             // 防御未来 SQLite 新增跨文件语句
    Pragma => {
        // 只放行 foreign_keys / user_version 白名单
        if name == "foreign_keys" || name == "user_version" { ok } else { denied }
    }
    _ => ok
}
```

注释(`backup.rs:39-62`)长篇解释为什么用 authorizer 而非关键字扫描:
- 字符串扫描会被 `/*x*/ATTACH`、大小写、换行绕过
- authorizer 在 prepare 阶段按"解析结果"回调,绕不过语法层
- `ATTACH DATABASE 'x'`、`VACUUM INTO 'x'`、裸 `VACUUM` **三者都**报 `AuthAction::Attach`,所以拒 Attach 一条即可覆盖

### 7.8 临时数据库 + Backup API 两段式(`backup.rs:173-249`)

```rust
// backup.rs:173-249 简化
1. validate_cc_switch_sql_export 头注释校验
2. NamedTempFile + auto_vacuum=INCREMENTAL(避免导入把主库 auto_vacuum 模式降级)
3. 装上 authorizer → execute_batch → 卸 authorizer
4. validate_imported_schema 校验 schema(必须在 create_tables_on_conn 之前,
   否则迁移可能补齐缺失表,让截断文件伪装成合法)
5. create_tables_on_conn + apply_schema_migrations_on_conn 补齐缺失表/迁移
6. 加 BACKUP_FILE_OPERATION_LOCK(backup.rs:26 全局静态锁,
   保证"安全快照 + 本地表读 + 最终替换"期间无并发写入)
7. Backup::new(&temp_conn, &mut main_conn) + complete_backup
   ——使用 SQLite 官方的 Backup API 做 pages 复制
```
---

## 8. WebDAV / S3 云同步

### 8.1 协议抽象 `services/sync_protocol.rs:1-78`

```rust
// sync_protocol.rs:27-37
pub(crate) const PROTOCOL_FORMAT: &str = "cc-switch-webdav-sync";  // 历史命名保留
pub(crate) const PROTOCOL_VERSION: u32 = 2;
pub(crate) const DB_COMPAT_VERSION: u32 = 6;
pub(crate) const LEGACY_DB_COMPAT_VERSION: u32 = 5;
pub(crate) const REMOTE_DB_SQL: &str = "db.sql";
pub(crate) const REMOTE_SKILLS_ZIP: &str = "skills.zip";
pub(crate) const REMOTE_MANIFEST: &str = "manifest.json";
pub(crate) const MAX_DEVICE_NAME_LEN: usize = 64;
pub(crate) const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_SYNC_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
```

### 8.2 SyncManifest / ArtifactMeta(`sync_protocol.rs:106-130`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncManifest {
    pub format: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_compat_version: Option<u32>,
    pub device_name: String,
    pub created_at: String,
    pub artifacts: BTreeMap<String, ArtifactMeta>,  // 按 name 排序,稳定 hash
    pub snapshot_id: String,                        // sha256(artifacts 拼接)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArtifactMeta {
    pub sha256: String,
    pub size: u64,
}
```

### 8.3 全局互斥锁(`sync_protocol.rs:46-57`)

```rust
// 注释:WebDAV 和 S3 以前各自有 mutex,允许两个 transport 并发恢复数据库和 Skills
// 把锁提到 transport-agnostic 层后,未来 transport 自动共享
pub(crate) fn sync_mutex() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}
pub(crate) async fn run_with_sync_lock<T, Fut>(operation: Fut) -> Result<T, AppError>
where Fut: Future<Output = Result<T, AppError>>,
{
    let _guard = sync_mutex().lock().await;
    operation.await
}
```

`webdav_and_s3_operations_share_one_sync_mutex` 测试(`sync_protocol.rs:474-486`)验证两个 transport 共享同一个 lock。

### 8.4 增量同步触发表白名单(`sync_protocol.rs:64-78`)

```rust
pub(crate) fn should_trigger_auto_sync_for_table(table: &str) -> bool {
    let normalized = table.trim().to_ascii_lowercase();
    matches!(normalized.as_str(),
        "providers" | "provider_endpoints" | "mcp_servers" | "prompts"
        | "skills" | "skill_repos" | "profiles" | "settings" | "proxy_config"
    )
}
```

故意排除 `proxy_request_logs / provider_health / session_log_sync / model_pricing`——这些是设备本地数据,多设备同步时不能跨设备覆盖。`model_pricing` 是用户拥有的 SSOT(JSON sidecar)。

### 8.5 快照构建(`sync_protocol.rs:149-210`)

```rust
pub(crate) fn build_local_snapshot(db: &Database) -> Result<LocalSnapshot, AppError> {
    let _skill_state_guard = skill_state_read_guard();   // DB 行 + 文件 SSOT 时序一致
    let sql_string = db.export_sql_string_for_sync()?;   // 跳过本地专属表
    let db_sql = sql_string.into_bytes();
    let tmp = tempdir()?;
    let skills_zip_path = tmp.path().join(REMOTE_SKILLS_ZIP);
    zip_skills_ssot(&skills_zip_path)?; // skills 打包成确定性 ZIP
    let skills_zip = fs::read(&skills_zip_path)?;
    let mut artifacts = BTreeMap::new();
    artifacts.insert(REMOTE_DB_SQL.to_string(),
        ArtifactMeta { sha256: sha256_hex(&db_sql), size: db_sql.len() as u64 });
    artifacts.insert(REMOTE_SKILLS_ZIP.to_string(),
        ArtifactMeta { sha256: sha256_hex(&skills_zip), size: skills_zip.len() as u64 });
    let snapshot_id = compute_snapshot_id(&artifacts);   // sha256("name:hash|name:hash")
    let manifest = SyncManifest { /* ... */ };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let manifest_hash = sha256_hex(&manifest_bytes);
    Ok(LocalSnapshot { db_sql, skills_zip, manifest_bytes, manifest_hash })
}
```

### 8.6 快照应用 + Skills 回滚(`sync_protocol.rs:357-391`)

```rust
pub(crate) fn apply_snapshot(db: &Database, db_sql: &[u8], skills_zip: &[u8]) -> Result<(), AppError> {
    let _skill_state_guard = skill_state_write_guard();
    let skills_backup = backup_current_skills()?;
    restore_skills_zip(skills_zip)?;                        // 先还原 skills
    if let Err(db_err) = db.import_sql_string_for_sync(sql_str) {
        // DB 失败时回滚 skills
        if let Err(rollback_err) = restore_skills_from_backup(&skills_backup) {
            return Err(/*双重失败错误 */);
        }
        return Err(db_err);
    }
    Ok(())
}
```

**"先 skills 后 DB;失败则回滚 skills"**——保证一致性。

### 8.7 WebDAV 同步 `services/webdav_sync.rs`

- 任意兼容 WebDAV 的服务(Nextcloud / 坚果云 / 自建 Apache)。
- 配置文件 `~/.cc-switch/webdav-{profile}.json`(profile 隔离多账号)。
- 上传流程(`webdav_sync.rs:53-96`):
  - 序列化为 JSON → 用密码派生 key(argon2/scrypt)→ 加密 → PUT 到 `${remoteRoot}/${profile}/${date}/${snapshotId}/manifest.json` + 各 artifact
  - 上传顺序:**artifacts 先 → manifest 后**(best-effort consistency,`webdav_sync.rs:64-78`)
- 下载流程:`get_bytes` 拿 manifest → 校验 etag → `verify_artifact` 比 size + sha256 → 解密 → 写本地。
- 自动同步(可选):`webdav_auto_sync.rs` 后台 tokio 任务,定时(默认 15 分钟)上传变更。

### 8.8 S3 同步 `services/s3_sync.rs`

- 任意 S3 兼容(MinIO / R2 / AWS S3 / 阿里 OSS)。
- 走 V4 签名手写,不依赖 SDK。
- `s3_auto_sync.rs` 同 WebDAV 的后台调度器。

### 8.9 DropBox / OneDrive / iCloud

通过本地路径挂载实现(不调 SDK,直接读写 `~/Dropbox/Apps/CC-Switch/`、`~/Library/Mobile Documents/.../iCloud~cc~switch/`)。这套设计的好处是**零 OAuth 流程、零凭据同步**。

### 8.10 设备名探测(`sync_protocol.rs:401-417`)

```rust
pub(crate) fn detect_system_device_name() -> Option<String> {
    let env_name = ["CC_SWITCH_DEVICE_NAME", "COMPUTERNAME", "HOSTNAME"]
        .iter().filter_map(|key| std::env::var(key).ok())
        .find_map(|value| normalize_device_name(&value));
    if env_name.is_some() { return env_name; }
    let output = Command::new("hostname").output().ok()?;
    // ...
}
```

支持环境变量覆盖,默认从 hostname 命令获取。

---

## 9. 版本演进时间线

| 版本 | 演进内容 | 关键模块 |
|------|---------|---------|
| v3.0.x | 初版:仅 Claude/Codex 切换 | `commands/provider.rs` |
| v3.7.0 | MCP 多工具 SSOT + per-app 启用字段 | `mcp_servers` 表 + `mcp/<tool>.rs` |
| v3.9.x | Schema v2→v3:Skills 统一管理 | `skills` 表加 `app_type` |
| v3.10.0+ | 完整 8 款工具适配层 | `*_config.rs` 系列 |
| v3.15.x | Schema v9→v10:Hermes Agent | `hermes_config.rs` + `mcp/hermes.rs` |
| v3.16.x | Schema v10→v11:`usage_daily_rollups` 加 `request_model` 维度 | `usage_daily_rollups` 6 元组 PK |
| v3.18.x | Schema v11→v12:Profiles(全应用共享) | `profiles` 表 |
| v3.19.x | Schema v14→v15:Grok Build 适配 + `thinking_rectifier` 7模式 | `grok_config.rs` + `thinking_rectifier.rs` |
| v3.20.0 | 完整 Tauri 2 + React 18 改造,Linux WebKit 兜底,WebDAV v2 协议 | `main.rs:8-32` |

---

## 10. cc-switch 核心机制借鉴要点(给 laew)

### P0(一月内落地,价值最高)

**1. 熔断器模式 + Per-Provider 状态机**
- 路径参考: `src-tauri/src/proxy/circuit_breaker.rs:76-388` + `provider_router.rs:24-292`
- 落地: 在 `agent/provider_router.rs`(新建)实现 `CircuitBreaker`,返回 `AllowResult { allowed, used_half_open_permit }`
- 价值: laew 当前 LLM 调用失败后无限重试浪费 token;熔断后能让 Yolo / Main-Work agent 快速 failover

**2. "中性释放 HalfOpen permit"接口**
- 路径参考: `provider_router.rs:204-216 release_permit_neutral`
- 落地: laew 整流器重试失败时,调用 `release_permit_neutral` 不污染健康度
- 价值: 避免 laew 的 client-side error 错误计入熔断器导致"全网段屏蔽"

**3. Schema 迁移系统(SAVEPOINT 包裹 + user_version)**
- 路径参考: `database/schema.rs:435-499 apply_schema_migrations_on_conn` + `schema.rs:407-412 add_column_if_missing`
- 落地: laew 当前 `config/mod.rs::Db::new()` 没有迁移系统,业务变复杂后会需要
- 价值: 增量迁移、原子回滚、pre-migration 备份是生产级必备

**4. 原子写入 + JSON 排序输出**
- 路径参考: `config.rs:336-498 atomic_write_with_unix_mode` + `sort_json_keys`(配置写入)
- 落地: laew 的 Provider 持久化从 `fs::write` 升级为 atomic_write,tmp + rename,Windows 走 ReplaceFileW
- 价值: 避免 laew 写入过程中崩溃导致 SQLite/JSON 半损坏;JSON diff 可读性提升

**5. provider_supports_failover 守门**
- 路径参考: `provider_router.rs:18-21`
- 落地: laew 添加 OAuth-bound provider 概念,这种 provider跨账户路由会越权,必须 single route
- 价值: 与 laew 未来可能接入的 OAuth provider(Claude Max / ChatGPT Pro)兼容

### P1(三月内落地,价值中等)

**6. Thinking Signature 整流器**
- 路径参考: `thinking_rectifier.rs:26-109` 7 种错误模式 + `rectify_anthropic_request`(`thinking_rectifier.rs:118-189`)
- 落地: laew 的 anthropic-protocol adapter 中加 `rectify_anthropic_request`,触发后自动删除 thinking/redacted_thinking block 并对同一 provider 重试
- 价值: 解决"Anthropic API 第三方中转报 Invalid signature"导致的死循环

**7. ActiveConnectionGuard RAII**
- 路径参考: `forwarder.rs:130-156`
- 落地: laew 的 SubAgent 工作流目前用"手动 +1 / -1",改为 guard 模式,guard move 进流式 future 后自动 drop
- 价值: 避免 `active_connections` 计数过早归零导致 UI 显示错误

**8. 错误分类三态 + 中性释放**
- 路径参考: `forwarder.rs:1050-1112 categorize_proxy_error`
- 落地: 把 laew 的 `ProxyError` 分类为 Retryable / NonRetryable / ClientAbort,NonRetryable 跳过熔断器
- 价值: 用户取消请求 / 输入校验失败不会污染 provider 健康度

**9. SQL 导入 authorizer 防御**
- 路径参考: `backup.rs:63-83 import_authorizer`
- 落地: laew 未来若支持"导入导出全部配置"功能,导入 SQL 必须安装 authorizer 拒绝 ATTACH / VACUUM INTO / 未知 vtable
- 价值: 阻断"恶意 SQL 备份文件"在导入时执行任意路径写入的攻击向量

**10. MCP 多工具适配层抽象**
- 路径参考: `mcp/{claude,codex,gemini,opencode,hermes}.rs` 4 函数 × N 工具的统一模式
- 落地: laew 添加 `mcp/` 子模块,定义 `validate_server_spec` + 4 个标准函数,按 AppType 路由到不同 adapter
- 价值: 让 laew 像 cc-switch 一样能管理 8 种 CLI 的 MCP 配置

### P2(战略价值,长期规划)

**11. DeepLink 安全导入**
- 路径参考: `deeplink/mod.rs:34-139`
- 落地: laew 实现 `laew://import?resource=provider&app=claude&...` URL Scheme,base64 编码配置内容,usage_script 不默认启用
- 价值: 社区分享 Provider 配置一键导入

**12. WebDAV / S3 多设备同步**
- 路径参考: `commands/webdav_sync.rs` + `sync_protocol.rs` + `register_db_change_hook`
- 落地: laew 在 SQLite 上注册 update_hook,任何表写入异步触发 WebDAV / S3 同步
- 价值: 用户多设备无缝共享 Provider / Skill / Prompt 配置

### 反模式警示(不要照搬)

- **不要照搬 `forwarder.rs` 5267 行单文件**: laew 应在引入 failover之前先把"主循环 + 整流器 + 适配器"分文件。
- **不要照搬 `commands/misc.rs` 291 KB 单文件**: command 注册应按业务域分文件(provider/proxy/mcp/skill)。
- **不要把所有 per-app 配置塞进 `proxy_config` 三行结构**: laew 当前按 `app_type` 索引的 SQLite 模型已足够简洁。
- **不要18 张表平铺**: laew 当前 6 张表已经够用,过度规范化会增加维护成本。
- **Tauri 专属能力不能照搬**: Linux WebKit 兜底(`WEBKIT_DISABLE_DMABUF_RENDERER` 等)、系统托盘、深链注册——这些只对桌面应用有意义,laew 是 CLI 不必借鉴。
- **每工具独立5267 行 forwarder**: 协议转换适配器虽然可以借鉴"transform_*"思路,但不必全套 30+ 个文件。

### 借鉴优先级速查

| 优先级 | 借鉴点 | 文件路径 | laew 落地难度 |
|-------|--------|---------|--------------|
| P0 | 熔断器 | `circuit_breaker.rs:76-388` | ★★ |
| P0 | 中性释放 | `provider_router.rs:204-216` | ★ |
| P0 | Schema 迁移 | `schema.rs:435-499` | ★★★ |
| P0 | 原子写入 | `config.rs:336-498` | ★★ |
| P0 | provider_supports_failover | `provider_router.rs:18-21` | ★ |
| P1 | Thinking 整流 | `thinking_rectifier.rs:118-189` | ★★★ |
| P1 | ActiveConnectionGuard | `forwarder.rs:130-156` | ★★ |
| P1 | 错误分类三态 | `forwarder.rs:1050-1112` | ★★ |
| P1 | SQL authorizer | `backup.rs:63-83` | ★★ |
| P1 | MCP 适配层 | `mcp/{mod,validation}.rs` | ★★★ |
| P2 | DeepLink | `deeplink/mod.rs` | ★★ |
| P2 | WebDAV/S3 同步 | `sync_protocol.rs:1-78` | ★★★★ |

---

## 11. 关键文件路径速查表

| 关注点 | 路径 | 关键行号 / 函数 |
|------|------|---------------|
| Tauri 应用组装 | `src-tauri/src/lib.rs` | `run()` (342+) |
| 二进制入口 | `src-tauri/src/main.rs` | Linux WebKit 兜底 8-32 |
| 代理 HTTP 服务器 | `src-tauri/src/proxy/server.rs` | `ProxyServer::start` 94-223 |
| 手动 hyper accept loop | `src-tauri/src/proxy/server.rs` | 138-213 |
| Forwarder 主循环 | `src-tauri/src/proxy/forwarder.rs` | `forward_with_retry_inner` 429-1156 |
| ActiveConnectionGuard | `src-tauri/src/proxy/forwarder.rs` | 130-156 |
| Codex OAuth 校验 | `src-tauri/src/proxy/forwarder.rs` | `validate_codex_official_authorization` 57-99 |
| 熔断器三态机 | `src-tauri/src/proxy/circuit_breaker.rs` | `CircuitBreaker` 76-388 |
| 熔断器 AllowResult | `src-tauri/src/proxy/circuit_breaker.rs` | 99-103 |
| HalfOpen 限流 | `src-tauri/src/proxy/circuit_breaker.rs` | `allow_half_open_probe` 315-333 |
| provider_router | `src-tauri/src/proxy/provider_router.rs` | `select_providers` 45-131 |
| release_permit_neutral | `src-tauri/src/proxy/provider_router.rs` | 204-216 |
| provider_supports_failover | `src-tauri/src/proxy/provider_router.rs` | 18-21 |
| Thinking 整流 | `src-tauri/src/proxy/thinking_rectifier.rs` | `rectify_anthropic_request` 118-189 |
| Thinking 错误模式 7 | `src-tauri/src/proxy/thinking_rectifier.rs` | `should_rectify_thinking_signature` 26-109 |
| Budget 整流 | `src-tauri/src/proxy/thinking_budget_rectifier.rs` | `rectify_thinking_budget` 81-122 |
| Media 预防式 | `src-tauri/src/proxy/forwarder.rs` | `apply_media_prevention` 199-218 |
| Media 反应式 | `src-tauri/src/proxy/forwarder.rs` | `media_retry_should_trigger` 224-237 |
| 原子写入 | `src-tauri/src/config.rs` | `atomic_write_with_unix_mode` 336-498 |
| JSON key 排序 | `src-tauri/src/config.rs` | `sort_json_keys` 277-291 |
| Codex MCP 同步 | `src-tauri/src/mcp/codex.rs` | `import_from_codex` 52-276, `sync_enabled_to_codex` 286+ |
| Hermes MCP 适配 | `src-tauri/src/mcp/hermes.rs` | `convert_to_hermes_format` 61-105 |
| MCP 验证 | `src-tauri/src/mcp/validation.rs` | `validate_server_spec` 8-51 |
| MCP mod 索引 | `src-tauri/src/mcp/mod.rs` | 23-41 |
| Schema 迁移主循环 | `src-tauri/src/database/schema.rs` | `apply_schema_migrations_on_conn` 435-499 |
| Schema 17 张表 | `src-tauri/src/database/schema.rs` | 25-345 |
| pre-migration 备份 | `src-tauri/src/database/mod.rs` | 128-140 |
| add_column_if_missing | `src-tauri/src/database/schema.rs` | 407-412 |
| SQL 导入 authorizer | `src-tauri/src/database/backup.rs` | `import_authorizer` 63-83 |
| 临时 DB + Backup API | `src-tauri/src/database/backup.rs` | `import_sql_string_inner_with_hook` 173-249 |
| WebDAV 同步 | `src-tauri/src/services/webdav_sync.rs` | `upload` 53-96 |
| S3 同步 | `src-tauri/src/services/s3_sync.rs` | `upload` 38-79 |
| 同步协议抽象 | `src-tauri/src/services/sync_protocol.rs` | 27-78 (常量) |
| 全局 sync mutex | `src-tauri/src/services/sync_protocol.rs` | 46-57 |
| 增量同步触发表 | `src-tauri/src/services/sync_protocol.rs` | `should_trigger_auto_sync_for_table` 64-78 |
| SyncManifest | `src-tauri/src/services/sync_protocol.rs` | 106-130 |
| 快照构建 | `src-tauri/src/services/sync_protocol.rs` | `build_local_snapshot` 149-210 |
| 快照应用+回滚 | `src-tauri/src/services/sync_protocol.rs` | `apply_snapshot` 357-391 |
| DeepLink 模型 | `src-tauri/src/deeplink/mod.rs` | `DeepLinkImportRequest` 34-139 |
| AppType枚举 | `src-tauri/src/app_config.rs` | 整文件 |
| 多 App 配置 | `src-tauri/src/app_config.rs` | `MultiAppConfig` |
| Codex完整配置 | `src-tauri/src/codex_config.rs` | 5267 行 |

---

## 12. 总结

cc-switch 是一个**生产级**的"CLI Agent 路由器 + 统一配置中心"项目,核心机制集中在 4 层:

1. **代理层**(`proxy/`)—— 熔断器三态机(`AllowResult + release_half_open_permit`)、整流器(7 模式 thinking + 4 模式 media)、模型映射(haiku/sonnet/opus/fable)、Hyper 手写 accept loop + Header case preservation、OAuth 透传校验都是教科书级的容错设计。
2. **持久层**(`database/`)—— 17 个 schema 版本 + SAVEPOINT 包裹 + pre-migration 备份、SQL 导入 authorizer 防御(`AuthAction::Attach` / `CreateVtable` / `Unknown` 全拒绝)、双向同步的 `SYNC_SKIP/SYNC_PRESERVE` 表设计、incremental vacuum 都是工程化的体现。
3. **跨工具适配**(`mcp/` + `*_config.rs`)—— 把 8 种 CLI 的 MCP 配置抽象为统一 McpServer + 6 个 enabled 字段,再按 per-tool adapter 投影;Codex TOML / Hermes YAML / Claude JSON 的差异抹平展示了"协议适配器 + 4 函数模板"的标准模式。
4. **安全**——atomic_write、`sort_json_keys`、`redact_known_secrets`、`import_authorizer`、`validate_repo_ref`、`usage_script` 默认禁用等细节展示了"信任边界外层加厚"的工程文化。

对 laew 而言,**最有借鉴价值的是 P0 五项**(熔断器、中性释放、schema 迁移、原子写入、provider_supports_failover),它们能在不改变 laew 核心架构的前提下显著提升生产稳定性。P1 项(thinking 整流、ActiveConnectionGuard、错误分类、authorizer、MCP 适配)是面向"接入第三方中转 API"场景的容错加固。P2 项(DeepLink / WebDAV-S3 同步)属于"产品演进方向",可作为 laew v2.x 的中长期规划。**不应照搬** Tauri 专属能力(系统托盘、Linux WebKit 兜底)和1.7 MB 代理层单文件怪兽——这些只对 cc-switch 的"8 工具 + 多 Provider"场景有意义,laew 主战场是 Anthropic + OpenAI 双协议,可降低适配广度。
