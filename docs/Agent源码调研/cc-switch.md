# CC-Switch 综合深度分析

> 调研对象: cc-switch (Tauri 2 + Rust + React, AI 工具配置管理桌面应用)
> 调研日期: 2026-09-05
> 原始文档: 3 份(源码调研 + 深度分析 + 核心机制深度分析)
> 总行数: ~1800 行(合并后)

---

## 目录

1. [项目元信息](#1-项目元信息)
2. [8 款工具适配](#2-8-款工具适配)
3. [本地 LLM 代理](#3-本地-llm-代理)
4. [熔断器(三态 Closed/Open/HalfOpen)](#4-熔断器三态-closedopenhalfopen)
5. [thinking_rectifier 整流(7 模式)](#5-thinking_rectifier-整流7-模式)
6. [MCP 多工具 SSOT](#6-mcp-多工具-ssot)
7. [17 Schema 版本迁移](#7-17-schema-版本迁移)
8. [SQL authorizer 防御](#8-sql-authorizer-防御)
9. [WebDAV / S3 云同步](#9-webdav--s3-云同步)
10. [配置系统](#10-配置系统)
11. [对 laew 的借鉴](#11-对-laew-的借鉴)

---

## 1. 项目元信息

### 1.1 工程定位

CC Switch(`farion1231/cc-switch`)是一站式 CLI Agent 路由器 + 统一配置中心,支持 Claude Code、Codex、Gemini、GitHub Copilot、Grok Build、OpenCode、Hermes Agent、Pi Agent 等 8+ 款 CLI/桌面 Agent 的供应商配置统一管理;内置本地 HTTP 反向代理 + 自动故障转移 + 用量统计 + MCP 跨工具同步 + Skill 仓库管理 + DeepLink 一键导入。

**技术栈**: Rust(Tauri 2 后端)+ React 18 + TypeScript + Vite + Tailwind + TanStack Query + Zod + SQLite(rusqlite)+ Hyper + tokio + rquickjs

### 1.2 工程结构

```
cc-switch/
├── src/                       # React 前端
│   ├── App.tsx                # 主入口(1829 行,ViewState 机)
│   ├── types.ts               # 754 行全局类型定义(Provider/Mcp/Skill/Settings/Sync)
│   ├── components/            # 20+ 业务组件目录(providers/mcp/skills/proxy/usage/sessions/hermes/openclaw/deeplink/...)
│   ├── hooks/                 # 25+ 自定义 hook(useProviderActions/useSkills/useMcp...)
│   ├── lib/                   # 平台/查询/Schema/版本工具
│   └── contexts/, i18n/       # i18next 多语言(zh/en/ja/de),主题/状态上下文
├── src-tauri/                 # Rust 后端
│   ├── src/
│   │   ├── lib.rs             # 应用组装(2403 行,Tauri Builder 入口)
│   │   ├── main.rs            # 二进制入口(95 行,Linux WebKit 兜底)
│   │   ├── commands/          # 35 个 #[tauri::command] 命令文件
│   │   ├── proxy/             # 本地代理(28 模块,合计 1.7 MB)
│   │   ├── services/          # 业务服务层(38 个模块)
│   │   ├── database/          # SQLite + 17 个 schema 迁移 + 备份/恢复
│   │   ├── mcp/               # MCP 跨工具适配(8 模块)
│   │   ├── deeplink/          # ccswitch:// 协议解析
│   │   ├── session_manager/   # 各工具会话历史解析
│   │   └── *_config.rs        # 8 款工具的配置适配器
│   ├── tauri.conf.json
│   └── Cargo.toml
├── docs/                      # 用户文档(ZH/EN/JA/DE)
├── tests/                     # 集成测试
└── package.json               # 前端依赖(Tauri 2.8 + Radix UI + CodeMirror + recharts + framer-motion)
```

### 1.3 体量统计

| 层 | 关键文件 | 行数/大小 |
|----|---------|----------|
| 应用入口 | `src-tauri/src/lib.rs` | 2403 行 |
| 二进制入口 | `src-tauri/src/main.rs` | 95 行 |
| 数据库 schema | `src-tauri/src/database/schema.rs` | 3412 行 |
| 数据库备份 | `src-tauri/src/database/backup.rs` | 130 KB |
| 代理转发 | `src-tauri/src/proxy/forwarder.rs` | 5267 行 |
| 代理 handler | `src-tauri/src/proxy/handlers.rs` | 3581 行(146 KB) |
| 代理服务层 | `src-tauri/src/services/proxy.rs` | 10002 行 |
| 熔断器 | `src-tauri/src/proxy/circuit_breaker.rs` | 496 行 |
| 模型映射 | `src-tauri/src/proxy/model_mapper.rs` | 429 行 |
| Skill 管理 | `src-tauri/src/services/skill.rs` | 5989 行 |
| 用量统计 | `src-tauri/src/services/usage_stats.rs` | 110 KB |
| 用量定价 | `src-tauri/src/services/model_pricing.rs` | 28 KB |
| 同步协议 | `src-tauri/src/services/sync_protocol.rs` | 25 KB |
| 8 工具配置 | `src-tauri/src/*_config.rs` | 80KB~225KB/文件 |
| 前端类型 | `src/types.ts` | 754 行 |
| 前端 hooks | `src/hooks/*.ts` | 25+ 文件 |

### 1.4 Tauri 2 启动流程

```rust
// lib.rs::run() 启动序列
panic_hook::setup_panic_hook()
└─ tauri::Builder::default()
   ├─ plugin(single_instance): 复用已有实例,聚焦窗口
   ├─ plugin(deep_link): 注册 ccswitch:// 协议
   ├─ plugin(process/dialog/opener/store/window_state)
   ├─ on_window_event: 拦截关闭→最小化到托盘(settings.minimize_to_tray_on_close)
   └─ setup(|app| {
       ├─ panic_hook + log 初始化(Rotate 4 归档 × 20MB)
       ├─ Database::init(): SQLite + Schema 迁移(v0..v17)
       ├─ TrayIconBuilder: 系统托盘(macOS/Linux/Windows 平台分支)
       ├─ usage_events::init(): 注入 AppHandle,日志事件可推送前端
       └─ invoke_handler: 批量注册所有 #[tauri::command]
   })
```

### 1.5 前后端通信模式

- **命令调用**: 前端 `import { invoke } from "@tauri-apps/api/core"` → `invoke<Args, Result>("command_name", args)`,错误转 `String`
- **事件订阅**: 后端 `app.emit("deeplink-import", &payload)` → 前端 `listen<T>("deeplink-import", handler)`,用于用量实时刷新、深链导入提示、托盘状态变更
- **状态共享**: 后端核心对象(`Database`、`ProxyServer`、`ProviderRouter`)用 `Arc<RwLock<T>>` 包装挂在 `tauri::State`
- **混合刷新**: `react-query(@tanstack/react-query)` 拉取命令结果 + 后端事件触发 query invalidation,保证 dashboard 实时性

### 1.6 Linux WebKit 多平台兜底

```rust
// main.rs:8-32 教科书级多平台兼容
#[cfg(target_os = "linux")]
{
    // WebKitGTK DMA-BUF 渲染器在 Nvidia + Debian 13.2 触发白屏/黑屏
    if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    // 禁用 WebKitGTK 合成模式规避 resize 时 webview 崩溃
    if std::env::var("WEBKIT_DISABLE_COMPOSITING_MODE").is_err() {
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }
    // AppImage GTK 启动钩子强制 XWayland,提供逃生开关让用户改回 Wayland
    if let Ok(backend) = std::env::var("CC_SWITCH_GDK_BACKEND") {
        if !backend.is_empty() {
            std::env::set_var("GDK_BACKEND", backend);
        }
    }
}
```

`redact_url_for_log_with_secrets`(`lib.rs:177-210`)集中处理 URL 脱敏:`MIN_KNOWN_SECRET_LEN = 8` 避免误伤普通词。

---

## 2. 8 款工具适配

### 2.1 Provider 数据模型

**前端 React**(`src/types.ts:11-31`):

```typescript
export interface Provider {
  id: string; name: string;
  settingsConfig: Record<string, any>;  // 应用配置对象
  websiteUrl?: string;
  category?: ProviderCategory;  // official | cn_official | cloud_provider | aggregator | third_party | custom | omo
  createdAt?: number; sortIndex?: number;
  notes?: string; isPartner?: boolean;
  meta?: ProviderMeta;  // apiFormat / custom_endpoints / costMultiplier / authBinding / isFullUrl / promptCacheKey / providerType
  icon?: string; iconColor?: string;
  inFailoverQueue?: boolean;
}
```

**后端 Rust** 镜像定义在 `src-tauri/src/provider.rs`,用 `serde_json::Value` 存 `settings_config` 直接透传各工具原生配置 JSON,不做强制 schema。

### 2.2 8 款工具的配置适配器

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

### 2.3 统一抽象模式

每个工具暴露 **3 个标准函数**:

```rust
read_<tool>_live_settings() -> Result<serde_json::Value, AppError>;  // 读原生配置
write_<tool>_live_atomic(value: Value) -> Result<(), AppError>;     // 原子写
read_and_validate_<tool>_config_text() -> Result<String, AppError>; // 仅 Codex
```

`MultiAppConfig`(`src-tauri/src/app_config.rs`,44KB)统一管理 8 款应用的 schema 校验、版本检测、目录覆盖(settings 中可指定 `claudeConfigDir` 等)。

`providerType` 枚举:`codex_oauth / claude_oauth / xai_oauth / bedrock / copilot / generic`,每种走不同字段映射。Provider 的 `settings_config` 用 `serde_json::Value` 存储,**不做强制 schema**——"差异抹平"的关键设计。

### 2.4 50+ 预设 Provider

实际预设由 `src-tauri/resources/codex_deepseek_catalog_template.json`(76KB) + `gpt5_5_template.json`(46KB)提供,**结构化的模型目录 + Codex Responses API 模板**。CC Switch 把"预设 Provider"建模为 JSON 模板,可被 `model_fetch.rs` 自动从 models.dev 同步(`src/lib/modelsDevAutoSync.ts`),用户开箱即可用 PackyCode / DMXAPI / 302.AI / OhMyOpenCode 等 50+ 中转服务。

---

## 3. 本地 LLM 代理

CC Switch 最核心、最复杂的子系统——一个 HTTP 代理把 Claude Code/Codex/Gemini 的请求改写后转发到任意 Provider。

### 3.1 模块拓扑

```
proxy/ (合计 1.7 MB Rust, 28 模块)
├── server.rs                 # Axum 路由 + 手动 hyper accept loop(25 KB)
├── handlers.rs               # 5 大端点 handler(146 KB)
├── forwarder.rs              # 主循环 forward_with_retry(5267 行)
├── provider_router.rs        # 故障转移选择 + 熔断器调度(23 KB)
├── circuit_breaker.rs        # 三态熔断器(17 KB, 496 行)
├── model_mapper.rs           # 模型别名映射 haiku/sonnet/opus/fable(429 行)
├── handler_context.rs        # RequestContext:贯穿生命周期元数据(378 行)
├── handler_config.rs         # UsageParserConfig(229 行)
├── response_processor.rs     # 响应处理 + 用量收集 + SSE 透传(47 KB)
├── failover_switch.rs        # 热切换 P1 失败后自动切 P2
├── thinking_rectifier.rs     # thinking 签名整流器(7 种错误模式, 723 行)
├── thinking_budget_rectifier.rs  # budget_tokens 强制32000 + max_tokens 64000
├── media_sanitizer.rs        # 图片降级整流器(UNSUPPORTED_IMAGE_MARKER)
├── thinking_optimizer.rs / cache_injector.rs  # Bedrock 优化器(可选)
├── copilot_optimizer.rs      # Copilot 反优化(防 premium quota 偷跑)
├── content_encoding.rs       # gzip/brotli/zstd 解压
├── hyper_client.rs           # 自实现 HTTP 客户端, 带 HeaderCaseMap 透传
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

### 3.2 服务器入口 `proxy/server.rs:34-92`

```rust
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

默认监听 `127.0.0.1:15721`(仅本机访问),接管时把各工具的 live config 改写为指向本代理。

**路由表**(`server.rs:291-379` build_router):

| 路径 | Handler | 用途 |
|------|---------|------|
| `/health`, `/status` | health_check / get_status | 健康/状态 |
| `/v1/messages` 等多别名 | `handle_messages` | Claude Messages API |
| `/chat/completions` 等多别名 | `handle_chat_completions` | OpenAI Chat Completions |
| `/responses` 等多别名 | `handle_responses` | OpenAI Responses API |
| `/responses/compact` | `handle_responses_compact` | Responses 远程压缩 |
| `/alpha/search` | `handle_alpha_search` | Codex Alpha Search |
| `/v1beta/*path` | `handle_gemini` | Gemini 原生(含 GET `/models` 用 `any(..)`) |

### 3.3 手写 hyper HTTP/1.1 accept loop + Header case preservation

绕开 axum 默认行为,手动 hyper accept loop,在每条连接内 `stream.peek()` 抓取原始 TCP 头大小写,存入 `OriginalHeaderCases` extension。动机:**保持客户端请求头的 wire-level 大小写**(典型场景:CLI 用户配了 `X-Custom-Case` 而上游 gateway 鉴权检查大小写敏感)。

```rust
// server.rs:194-201
hyper::server::conn::http1::Builder::new()
    .preserve_header_case(true) // 让 hyper 不强制小写 header
    .serve_connection(TokioIo::new(stream), service)
    .await
```

### 3.4 Forwarder 主循环 `proxy/forwarder.rs:429-1156`

设计哲学:**per-provider 独立重试 + 跨 provider 故障转移**。单次客户端请求最多尝试 `max_attempts = max_retries + 1` 个供应商。

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
            // 串联三层整流器重试(仅 Anthropic 供应商):
            //   4a) media_retry_should_trigger(图片降级)
            //   4b) thinking_signature 整流
            //   4c) thinking_budget 整流
        }
    }
}
```

**5 个值得借鉴的设计点**:

1. **per-provider 独立重试标记**(`forwarder.rs:466-469`):`rectifier_retried / budget_rectifier_retried / media_rectifier_retried` 都是 per-provider 局部变量。首家 provider 整流后被击落时,下家 provider 仍能用整流后的请求体走自己的整流流程,避免标记"短路"故障转移。

2. **熔断器放行检查早于 max_attempts 检查的反向**(`forwarder.rs:472-492`):把"已尝试次数上限"放在熔断器放行 *之前*,避免在已超限时还占用宝贵的 HalfOpen 探测名额。

3. **"错误分类 + 中性释放 HalfOpen" 模式**(`forwarder.rs:1050-1112` categorize_proxy_error):错误分为 3 类:
   - `Retryable`:真正 provider 故障 → 记录失败 + `update_provider_health` + 继续 failover
   - `NonRetryable`:客户端层错误(400/401/422)→ **不污染健康度**,仅 `release_permit_neutral`
   - `ClientAbort`:客户端断连 → 同 NonRetryable

4. **ActiveConnectionGuard RAII**(`forwarder.rs:130-156`):进入 forward_with_retry 时构造 guard,Drop 时调度 tokio 任务把 `active_connections` -1。流式响应 body 是 future,guard move 进 body future 后随其一起 drop,避免 UI 连接计数过早归零。

5. **Codex OAuth 鉴权透传**(`forwarder.rs:57-99`):`validate_codex_official_authorization` 校验请求携带的 Authorization 不含 `PROXY_MANAGED` 占位符、不为空;若 provider meta 关联了 `managed_account_id`,还要校验请求的 `chatgpt-account-id` header 与之匹配且 session 校验通过,强制避免跨账户路由。

### 3.5 SSE 流式处理

`response_processor.rs`(47KB):用 `futures::Stream` + `tokio::select!` 监听 `first_byte_timeout` 与 `idle_timeout`,双超时独立计时。`SseUsageCollector` 在流末端聚合 `usageMetadata`/`message_delta` 等事件,关闭时落库。

### 3.6 Provider 路由 + 故障转移

```rust
// provider_router.rs:45-131
pub async fn select_providers(&self, app_type: &str) -> Result<Vec<Provider>, AppError> {
    // 1. 查 app 当前 provider(current_provider)
    // 2. 读 proxy_config.auto_failover_enabled
    //    - 关:仅返回当前 provider
    //    - 开:遍历 failover_queue(providers 表 WHERE in_failover_queue=1 ORDER BY sort_index)
    // 3. 对每个候选调 breaker.is_available() → 跳过 Open 状态的 provider
    // 4. Codex OAuth 账号永不入队(provider_supports_failover 检查)
}
```

`FailoverSwitchManager`(`failover_switch.rs`)用于"热切换"场景:故障转移开启时,P1 失败后**主动**把当前 provider 切到 P2(更新 DB),让后续请求直接命中 P2,避免每次都先打到坏 provider 浪费 1 次尝试。

### 3.7 模型映射 `model_mapper.rs`

从 Provider 的 `settings_config.env` 中的环境变量抽出:

```rust
ANTHROPIC_DEFAULT_HAIKU_MODEL   → haiku_model
ANTHROPIC_DEFAULT_SONNET_MODEL  → sonnet_model
ANTHROPIC_DEFAULT_OPUS_MODEL    → opus_model
ANTHROPIC_DEFAULT_FABLE_MODEL   → fable_model   // Claude Code 4.6+ 新模型档
CLAUDE_CODE_SUBAGENT_MODEL      → subagent_model
ANTHROPIC_MODEL                 → default_model
```

`map_model("claude-sonnet-4-5")` 优先匹配子串 "fable" > "haiku" > "opus" > "sonnet" > 默认。**Fable 特殊降级链**:未单独配置 fable 档时归入 opus 档(与 Claude Code 官方分类器降级方向一致)。

**1M 上下文后缀剥离**(`strip_one_m_suffix_for_upstream`):Claude Code 用 `[1M]` 后缀声明 100 万上下文能力,上游 API 不识别,转发前必须剥离:`claude-fable-5[1m]` → `claude-fable-5`。

### 3.8 协议转换层

| 客户端格式 | 上游格式 | 转换器 |
|-----------|---------|--------|
| Claude Messages | OpenAI Chat Completions | `transform.rs::openai_to_anthropic` |
| Claude Messages | OpenAI Responses | `transform_responses.rs::responses_to_anthropic` |
| Claude Messages | Gemini Native | `transform_gemini.rs::gemini_to_anthropic`(含 thoughtSignature shadow store) |
| Codex Responses | OpenAI Chat | `transform_codex_chat.rs` |
| Codex Responses | Anthropic Messages | `transform_codex_anthropic.rs` |
| Codex Responses | OpenAI Responses(xAI 等) | `transform_codex_responses_namespace.rs` |

**Codex Responses namespace 处理**:Codex 的工具调用使用 `{namespace, name}` 二元名,但某些上游只接受扁平 `name`,在请求侧 flatten、响应侧 unflatten,保持客户端无感知。

**Gemini thoughtSignature**:Gemini 原生 thinking block 必须带 `thoughtSignature` 才能被多轮复用。CC Switch 用 `GeminiShadowStore`(内存 HashMap + 可选持久化)记录每 session 的签名,转发到 Anthropic 客户端时作为占位块封装。

### 3.9 用量统计系统

**TokenUsage 模型**(`parser.rs`):

```rust
pub struct TokenUsage {
    pub input_tokens: u32, pub output_tokens: u32,
    pub cache_read_tokens: u32, pub cache_creation_tokens: u32,
    pub model: Option<String>,
    pub message_id: Option<String>,  // 跨源去重
}
```

**6 套解析器**(`parser.rs:104-200+`):
- `from_claude_response` / `from_claude_stream_events`
- `openai_cache_read_tokens` / `openai_cache_write_tokens`:4 个字段 fallback 链
- Claude/Codex/Gemini 三家分别有 `from_*_response` 和 `from_*_stream_events`

`dedup_request_id` 生成稳定 request_id 用于跨源去重:Claude / Claude Desktop 共享 `session:{message_id}` namespace。

**成本计算**(`calculator.rs`):
- OpenAI/Codex/Responses/Gemini:`input_tokens` 包含 cache_read + cache_creation(`is_cache_inclusive_app`),需先 `saturating_sub` 两者再按输入价计费
- Anthropic/Claude:`input_tokens` 已经是 fresh input,直接按输入价计费
- 使用 `rust_decimal::Decimal` 高精度计算,`cost_multiplier` 只乘到总价不乘明细

**用量幂等写入**(`logger.rs::log_request`):
1. 计算 `input_token_semantics`(FRESH=0 / TOTAL=1)
2. `load_existing_semantic` 查重——相同 request_id + data_source=proxy + semantic 完全相同则直接返回
3. 不匹配时生成 `fallback = "request_id:collision:{sha256(semantic)}"` 二次写入

**日聚合**(`usage_daily_rollups`):主键包含 `(date, app_type, provider_id, model, request_model, pricing_model)` 六元组。

### 3.10 用量查询脚本(QuickJS)

`UsageScript`(`types.ts:55-79`)让用户用 JavaScript 编写自家订阅套餐的查询逻辑,后端用 `rquickjs`(QuickJS 绑定)执行。设计目标:不写 Rust 就能接入 302.AI / AnyRouter / 新 API 等中转。

`resolve_usage_credentials`(`provider.rs:135-198`)注入 `{{apiKey}}` / `{{baseUrl}}` / `{{providerId}}` 变量,脚本可用与 provider 不同的凭据查询用量。

---

## 4. 熔断器(三态 Closed/Open/HalfOpen)

### 4.1 数据结构

```rust
// circuit_breaker.rs:76-93
pub struct CircuitBreaker {
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

### 4.2 状态机转换条件

| 当前状态 | 触发条件 | 下一状态 | 副作用 |
|---------|---------|---------|--------|
| Closed | consecutive_failures ≥ threshold | Open | reset consecutive_failures |
| Closed | total ≥ min_requests AND error_rate ≥ threshold | Open | 同上 |
| HalfOpen | 半开探测失败(任意一次) | Open | reset 计数 |
| HalfOpen | consecutive_successes ≥ success_threshold | Closed | reset 全计数 |
| Open | elapsed ≥ timeout_seconds | HalfOpen | reset half_open_requests |

```rust
// circuit_breaker.rs:230-288 record_failure 简化
match state {
    CircuitState::HalfOpen => transition_to_open(),
    CircuitState::Closed => {
        if failures >= config.failure_threshold { transition_to_open() }
        else if total >= config.min_requests {
            let error_rate = failed as f64 / total as f64;
            if error_rate >= config.error_rate_threshold { transition_to_open() }
        }
    }
}
```

### 4.3 6 大核心设计点

1. **`AllowResult { allowed, used_half_open_permit }` 语义**(`circuit_breaker.rs:99-103`):把"是否允许"和"是否占用了 HalfOpen 探测名额"分开,调用方在请求结束后必须把 `used_half_open_permit` 传回才能正确释放名额。

2. **HalfOpen 状态限流**:`max_half_open_requests = 1`,通过 `half_open_requests.fetch_add(1)` 原子抢占。`release_half_open_permit` 用 CAS 循环防御性减量。

3. **双触发条件**:连续失败次数 ≥ threshold 触发,或 total ≥ min_requests 且 error_rate ≥ threshold 触发。

4. **`transition_to_half_open` 幂等保护**(`circuit_breaker.rs:367-377`):写锁内先检查 `*state != Open`,避免并发调用重置 `half_open_requests` 计数。

5. **按 app_type 独立配置**:`From<&AppProxyConfig>` + `update_app_configs` 按前缀过滤,让 Claude / Codex / Gemini 各自一套阈值。

6. **Codex Official 不可参与 failover**(`provider_router.rs:18-21`):`provider_supports_failover` 守门,Codex 官方请求携带账户 native Authorization,跨账户路由会越权。

### 4.4 按 app 独立配置

| app | failure_threshold | success_threshold | timeout_seconds | error_rate | min_requests |
|-----|-------------------|-------------------|-----------------|-----------|--------------|
| claude | 8 | 3 | 90 | 0.7 | 15 |
| codex | 4 | 2 | 60 | 0.6 | 10 |
| gemini | 4 | 2 | 60 | 0.6 | 10 |
| grokbuild | 4 | 2 | 60 | 0.6 | 10 |

ProviderRouter 用 `app_type:provider_id` 作为 circuit key,动态创建熔断器:

```rust
// provider_router.rs:255-291
async fn get_or_create_circuit_breaker(&self, key: &str) -> Arc<CircuitBreaker> {
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

### 4.5 中性释放 `release_permit_neutral`

```rust
// provider_router.rs:204-216
pub async fn release_permit_neutral(&self, provider_id: &str, app_type: &str) {
    // 仅释放 HalfOpen 名额,不触发 record_success/record_failure
}
```

用于整流器等"请求结果不应计入 Provider 健康度"的场景,仅释放 HalfOpen 名额、不触发 record_success/record_failure,避免 client-side error 错误计入熔断器导致"全网段屏蔽"。

---

## 5. thinking_rectifier 整流(7 模式)

### 5.1 7 种错误模式检测

`should_rectify_thinking_signature`(`thinking_rectifier.rs:26-109`)检测:

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

### 5.2 整流动作 `rectify_anthropic_request`(`thinking_rectifier.rs:118-189`)

```rust
// 1. 遍历 messages[*].content[]:删除 type=thinking / redacted_thinking block
// 2. 删除非 thinking block 上的 signature 字段
// 3. 兜底:thinking.type=enabled 且最后一条 assistant 消息首块不是 thinking 且存在 tool_use → 删除顶层 thinking 字段
```

### 5.3 Adaptive thinking 兼容(`thinking_rectifier.rs:240-242`)

```rust
// 与 CCH 对齐:请求前不做 thinking type 主动改写
pub fn normalize_thinking_type(body: Value) -> Value { body }
```

Adaptive 模式下 thinking block **不会**被错误归类为 legacy block 而触发整流。

### 5.4 嵌套 JSON 错误格式兼容

```rust
// thinking_rectifier.rs:308-324
let lower = msg.to_lowercase();
if lower.contains("invalid") && lower.contains("signature") && lower.contains("thinking") && lower.contains("block") {
    return true;
}
```

对原始字符串 lowercase 后做 `contains` 匹配,天然兼容嵌套 JSON 错误。

### 5.5 thinking_budget_rectifier

强制把 `thinking.budget_tokens = 32000`、`max_tokens = 64000`(常量 `MAX_THINKING_BUDGET/MAX_TOKENS_VALUE`)。Adaptive 模式跳过整流。`applied` 通过 `before != after` 派生,无副作用时不污染调用方决策。

### 5.6 media_sanitizer

两种触发模式:
- **预防式**(`apply_media_prevention`,`forwarder.rs:199-218`):发送前对 text-only 模型把图片块替换为 `UNSUPPORTED_IMAGE_MARKER` 标记。受 `request_media_fallback` 总开关 + `request_media_heuristic` 子开关管辖。
- **反应式**(`media_retry_should_trigger`,`forwarder.rs:224-237`):上游 4xx 后,对同一 provider 重试一次,替换图片块为标记。仅 `Claude | Codex` 适配器 + contains_image_blocks + is_unsupported_image_error 三条件 AND。

### 5.7 向后兼容策略

整流器对 adaptive / enabled / unknown thinking type **都不主动改写**,仅在错误触发时移除问题 block,保持 Anthropic 官方主路径完全兼容。CCH(Claude Code Helper)对齐注释多处出现,显示与竞品同步决策。

---

## 6. MCP 多工具 SSOT

### 6.1 统一抽象 `src-tauri/src/mcp/`

```
mcp/
├── mod.rs                   # 模块索引,统一导出 4 函数 × 7 工具 = 28 函数
├── validation.rs            # validate_server_spec + extract_server_spec
├── claude.rs                # Claude MCP(6 字段 enabled_*)
├── codex.rs                 # Codex TOML 适配(32 KB,最复杂)
├── gemini.rs                # Gemini settings.json 适配
├── grokbuild.rs             # Grok Build 简化适配
├── opencode.rs              # OpenCode {mcp: {id: {type:"local"|"remote"}}}
└── hermes.rs                # Hermes YAML 适配 + 特殊 auth 字段
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

### 6.3 SSOT 数据模型

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

### 6.4 Codex TOML 适配

Codex 用 TOML 格式且只接受顶层 `[mcp_servers]` 表(旧错误格式 `[mcp.servers]` 被主动清理)。

实现要点:
- 使用 `toml_edit`(而非 `toml`)允许保留未触及字段的格式与注释
- `read_and_validate_codex_config_text` 读取后若语法无效,**直接返回错误而非覆盖**
- 收集启用项 → 用 `toml_edit::DocumentMut` 替换 `mcp_servers` 表 → 保留其它键
- 核心字段(`type/command/args/env/cwd/url/headers/http_headers`)手动处理,**`headers` 和 `http_headers` 都是核心字段**——避免鉴权值落入通用日志路径

### 6.5 Hermes YAML 适配

Hermes 与众不同:
- **没有 type 字段**——靠是否存在 `command` / `url` 推断
- **Hermes 专有字段**:`enabled`、`timeout`、`connect_timeout`、`tools`、`sampling`、`roots`、`auth`(常量 `HERMES_EXTRA_FIELDS`)。`auth` 字段(OAuth 声明)即使 cc-switch 没有 OAuth UI 也必须保留 round-trip,否则会降级到未认证调用
- **写时剥离 + 写时保留**:导出时 Hermes→CC Switch 剥离 EXTRA_FIELDS,导入时 Hermes→CC Switch 也剥离;但**写回 Hermes 时保留 EXTRA_FIELDS**(merge-on-write 逻辑)

### 6.6 DeepLink 一键安装

`ccswitch://v1/import?resource=mcp&apps=claude,codex&config=...` 在前端 `DeepLinkImportDialog` 弹窗确认后调用 `import_from_deeplink`,一步跨工具安装。

`DeepLinkImportRequest` 是 40+ 字段的扁平结构,安全设计:
- **凭据字段明确隔离**:`api_key / usage_api_key / usage_access_token / usage_user_id` 分别独立
- **容量控制**:`config` 字段是 Base64 编码,避免 URL 长度爆炸
- **`usage_script` 不默认启用**:携带脚本本身不意味着运行,必须显式 `usageEnabled=true`——经典"opt-in by default"安全策略

---

## 7. 17 Schema 版本迁移

### 7.1 迁移驱动

`SCHEMA_VERSION: i32 = 17`(`database/mod.rs:56`)。迁移链 v0 → v1 → ... → v17,每个版本独立函数 `migrate_vN_to_vN1`,用 **SAVEPOINT 包裹**确保原子。

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

### 7.2 17 个 Schema 版本演进

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

### 7.3 启动期 pre-migration 备份

```rust
// database/mod.rs:128-140
if version > 0 && version < SCHEMA_VERSION {
    backup_database_file()?; // 升级前先备份
}
Self::apply_schema_migrations()?; // 再升级
```

升级失败也不阻断(备份失败仅 warn)。`stored_user_version_exceeds_supported`(`mod.rs:174-183`)专门处理"数据库版本比应用新"的反向场景——返回 `Some(version)` 让 UI 引导用户升级应用。

### 7.4 向后兼容工具

- `add_column_if_missing`(`schema.rs:407-412`):统一处理"列已存在"错误,幂等添加列
- `has_column(conn, table, col)`(`schema.rs:146`):列存在性查询
- `migrate_proxy_config_to_per_app`(`schema.rs:400-404`):旧版 `proxy_config` 是单例表 → 启动时直接转换为三行结构

### 7.5 17 张表清单

| # | 表 | 职责 |
|---|----|------|
| 1 | `providers` | 供应商主表(PK: id+app_type) |
| 2 | `provider_endpoints` | 多端点(FK: providers ON DELETE CASCADE) |
| 3 | `mcp_servers` | MCP 服务器(6 个 enabled_* 字段) |
| 4 | `prompts` | 提示词(PK: id+app_type) |
| 5 | `skills` | Skill(`content_hash` for update detection) |
| 6 | `skill_repos` | 自定义 GitHub 仓库 |
| 7 | `settings` | KV 配置 |
| 8 | `proxy_config` | 三行结构 app_type PK,4 套默认配置 |
| 9 | `provider_health` | 健康度(PK: provider_id+app_type) |
| 10 | `proxy_request_logs` | 请求日志(含 5 个索引 + `input_token_semantics`) |
| 11 | `model_pricing` | 模型定价 |
| 12 | `stream_check_logs` | 流式连通性测试日志 |
| 13 | `proxy_live_backup` | Live 配置备份(PK: app_type) |
| 14 | `usage_daily_rollups` | 日聚合(PK: 6 元组,含 `request_model/pricing_model`) |
| 15 | `session_log_sync` | 会话日志同步状态 |
| 16 | `session_usage_dedup` | fork/rewrite 去重账本 |
| 17 | `profiles` | 项目 Profiles(各 app 共享) |

---

## 8. SQL authorizer 防御

### 8.1 import_authorizer(`backup.rs:63-83`)

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

### 8.2 临时数据库 + Backup API 两段式(`backup.rs:173-249`)

```rust
// backup.rs:173-249 简化
1. validate_cc_switch_sql_export 头注释校验
2. NamedTempFile + auto_vacuum=INCREMENTAL(避免导入把主库 auto_vacuum 模式降级)
3. 装上 authorizer → execute_batch → 卸 authorizer
4. validate_imported_schema 校验 schema(必须在 create_tables_on_conn 之前,
   否则迁移可能补齐缺失表,让截断文件伪装成合法)
5. create_tables_on_conn + apply_schema_migrations_on_conn 补齐缺失表/迁移
6. 加 BACKUP_FILE_OPERATION_LOCK(全局静态锁,
   保证"安全快照 + 本地表读 + 最终替换"期间无并发写入)
7. Backup::new(&temp_conn, &mut main_conn) + complete_backup
   ——使用 SQLite 官方的 Backup API 做 pages 复制
```

### 8.3 同步时的表保留策略

`SYNC_SKIP_TABLES` + `SYNC_PRESERVE_TABLES`(`backup.rs:86-105`):
- `SYNC_SKIP_TABLES`:导出时跳过这些表的数据(`proxy_request_logs / stream_check_logs / provider_health / proxy_live_backup / usage_daily_rollups / session_log_sync / session_usage_dedup`)
- `SYNC_PRESERVE_TABLES`:导入时这些表的数据从当前主库 *回填* 到导入结果上

注释:`proxy_request_logs` 等是设备本地数据,多设备同步时不能跨设备覆盖。

---

## 9. WebDAV / S3 云同步

### 9.1 协议抽象 `services/sync_protocol.rs:1-78`

```rust
pub(crate) const PROTOCOL_FORMAT: &str = "cc-switch-webdav-sync";
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

### 9.2 SyncManifest / ArtifactMeta

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

### 9.3 全局互斥锁

```rust
// sync_protocol.rs:46-57
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

### 9.4 增量同步触发表白名单

```rust
// sync_protocol.rs:64-78
pub(crate) fn should_trigger_auto_sync_for_table(table: &str) -> bool {
    let normalized = table.trim().to_ascii_lowercase();
    matches!(normalized.as_str(),
        "providers" | "provider_endpoints" | "mcp_servers" | "prompts"
        | "skills" | "skill_repos" | "profiles" | "settings" | "proxy_config"
    )
}
```

故意排除 `proxy_request_logs / provider_health / session_log_sync / model_pricing`——这些是设备本地数据,多设备同步时不能跨设备覆盖。

### 9.5 快照构建

```rust
// sync_protocol.rs:149-210
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

### 9.6 快照应用 + Skills 回滚

```rust
// sync_protocol.rs:357-391
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

### 9.7 WebDAV 同步

- 任意兼容 WebDAV 的服务(Nextcloud / 坚果云 / 自建 Apache)
- 配置文件:`~/.cc-switch/webdav-{profile}.json`(profile 隔离多账号)
- 上传流程:序列化为 JSON → 用密码派生 key(argon2/scrypt)→ 加密 → PUT 到 `${remoteRoot}/${profile}/${date}/${snapshotId}/manifest.json` + 各 artifact
- 上传顺序:**artifacts 先 → manifest 后**(best-effort consistency)
- 下载流程:GET manifest → 校验 etag → `verify_artifact` 比 size + sha256 → 解密 → 写本地
- 自动同步(可选):`webdav_auto_sync.rs` 后台 tokio 任务,定时(默认 15 分钟)上传变更

### 9.8 S3 同步

- 任意 S3 兼容(MinIO / R2 / AWS S3 / 阿里 OSS)
- 走 V4 签名手写,不依赖 SDK
- `s3_auto_sync.rs` 同 WebDAV 的后台调度器

### 9.9 DropBox / OneDrive / iCloud

通过本地路径挂载实现(不调 SDK,直接读写 `~/Dropbox/Apps/CC-Switch/`、`~/Library/Mobile Documents/.../iCloud~cc~switch/`)。**零 OAuth 流程、零凭据同步**。

### 9.10 设备名探测

```rust
// sync_protocol.rs:401-417
pub(crate) fn detect_system_device_name() -> Option<String> {
    let env_name = ["CC_SWITCH_DEVICE_NAME", "COMPUTERNAME", "HOSTNAME"]
        .iter().filter_map(|key| std::env::var(key).ok())
        .find_map(|value| normalize_device_name(&value));
    if env_name.is_some() { return env_name; }
    let output = Command::new("hostname").output().ok()?;
    // ...
}
```

### 9.11 数据库变更自动触发同步

```rust
// database/mod.rs:84-93
Action::SQLITE_INSERT | SQLITE_UPDATE | SQLITE_DELETE =>
    crate::services::webdav_auto_sync::notify_db_changed(table);
    crate::services::s3_auto_sync::notify_db_changed(table);
```

任意表写操作触发 WebDAV / S3 自动同步——"数据库层自动触发跨设备同步"的精巧设计。

---

## 10. 配置系统

### 10.1 原子写入 + JSON 排序 `src-tauri/src/config.rs:336-498`

`atomic_write_with_unix_mode` 三步走:

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

### 10.2 数据库连接管理

`Database::conn` 用 `Mutex<Connection>` 包装——`rusqlite::Connection` 本身不是 `Sync`,需要 Mutex 包装才能在 Tauri State 多线程共享。`lock_conn!` 宏避免 `Mutex::lock().unwrap()` 的 panic。

### 10.3 启动清理

启动后:
1. `apply_schema_migrations`
2. `ensure_incremental_auto_vacuum`:检测 auto_vacuum 模式,非 INCREMENTAL 时**先备份**再 `VACUUM` 重建
3. `ensure_model_pricing_seeded`:内置模型定价种子
4. `cleanup_old_stream_check_logs(7)`:7 天前日志清理
5. `rollup_and_prune(30)`:30 天前的请求日志聚合到 `usage_daily_rollups` 后删除
6. `PRAGMA incremental_vacuum`:回收空间

### 10.4 模型定价

- 内置 100+ 主流模型价格,首次启动时 `seed_model_pricing()` 写入 `model_pricing` 表
- `modelsDevAutoSync.ts`(前端)按周从 models.dev 同步最新价格
- 用户自定义:可在 UI 添加任意模型定价
- `costMultiplier`(每 provider 一个倍率,默认 1.0)用于聚合后乘——支持中转商抽成

### 10.5 Skills 管理

**数据模型**(`schema.rs:84-106`):

```sql
CREATE TABLE skills (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT,
    directory TEXT NOT NULL,        -- 本地路径
    repo_owner TEXT, repo_name TEXT, repo_branch TEXT DEFAULT 'main',
    readme_url TEXT,
    enabled_claude INTEGER DEFAULT 0, enabled_codex INTEGER DEFAULT 0,
    enabled_gemini INTEGER DEFAULT 0, enabled_grokbuild INTEGER DEFAULT 0,
    enabled_opencode INTEGER DEFAULT 0, enabled_hermes INTEGER DEFAULT 0,
    installed_at INTEGER, content_hash TEXT, updated_at INTEGER
);
```

**安装源**:
- **GitHub repo**:`services/skill.rs` 内 `install_from_github(owner, name, branch)` → clone 到 `~/.claude/skills/<owner>-<name>/`
- **ZIP 上传**:用户上传 → 解压到指定目录 → 计算 `content_hash`(用 `sha2`)
- **自定义仓库**:`skill_repos` 表存额外 GitHub repo,登录时全仓库递归扫描(支持 monorepo 多 skill)

**跨工具同步**(`SkillSyncMethod` 设置):
- `symlink`(默认):用 symlink 让各工具指向同一份物理文件,省空间、即时更新
- `copy`:每个工具目录各拷一份,完全隔离
- `auto`:优先 symlink,失败时降级到 copy

**更新检测**:`migrate_v6_to_v7` 加 `content_hash` 字段,启动时比 upstream HEAD 与本地 hash,更新弹窗提示。

**仓库引用安全校验**(`SkillService::validate_repo_ref`):owner/name/branch 会拼接到归档下载 URL,主防线在 `download_repo`,但参数非法时**当场报错**而不是沉淀进表。

### 10.6 会话管理

**跨工具解析**(`session_manager/providers/`):

| 文件 | 工具 | 会话位置 |
|------|------|---------|
| `claude.rs` | Claude Code | `~/.claude/projects/<hash>/*.jsonl` |
| `codex.rs` | Codex | `~/.codex/sessions/<YYYY>/<MM>/<DD>/*.jsonl` + `~/.codex/state.db` |
| `gemini.rs` | Gemini | `~/.gemini/tmp/*/chats/session-*.json` |
| `grokbuild.rs` | Grok | 自定义 |
| `openclaw.rs` | OpenClaw | 自定义 |
| `opencode.rs` | OpenCode | 自定义 |
| `hermes.rs` | Hermes | 内嵌 |
| `pi.rs` | Pi Agent | 自定义 |

**会话浏览 + 搜索**:
- `useSessionSearch.ts`(前端):支持全文搜索(`flexsearch` 索引本地 JSONL)
- 显示会话元信息(标题、摘要、项目目录、创建时间、最后活跃)
- 一键恢复:生成 `claude --resume <session-id>` 等命令,在 `terminal/mod.rs` 中打开系统终端执行

**session_usage_*.rs**:8 个 `session_usage_*.rs`(对应 8 个工具)做"从会话日志反算用量",因为某些工具/场景下用户**关闭了 CC Switch 代理**,但 session 文件里仍有 usage 信息——这些模块把日志回填到 `proxy_request_logs`,保证 dashboard 完整。

### 10.7 Deep Link:`ccswitch://` 协议

**URL 格式**:`ccswitch://v1/import?resource={type}&...`

`resource` ∈ `provider` / `prompt` / `mcp` / `skill`,详细字段见 `deeplink/parser.rs`:

- **provider**:`app` `name` `homepage` `endpoint` `apiKey` `icon` `model` `notes` `haikuModel` `sonnetModel` `opusModel` `config` `configFormat` `configUrl` `usageScript` …
- **prompt**:`app` `name` `content` `description`
- **mcp**:`apps`(逗号分隔) `config`(JSON)
- **skill**:`repo`(owner/name) `directory` `branch`

**解析 → 事件 → 前端弹窗**:

```rust
// deeplink/mod.rs
handle_deeplink_url(app, url_str, focus_main_window, source) {
    if !url_str.starts_with("ccswitch://") { return false; }
    parse_deeplink_url(url_str) → DeepLinkImportRequest
    app.emit("deeplink-import", &request)  // 前端 DeepLinkImportDialog 监听
}
```

**提供商导入**(`deeplink/provider.rs` 44KB):完整的"解析 + 校验 + 落 DB + 接管 live config"流程,支持直接 base64 嵌入 config JSON、从 URL 拉取 config、自动校验 endpoints / apiKey / 模型字段、自动启用、自动设置 usage script。

---

## 11. 对 laew 的借鉴

### 11.1 P0(一月内落地,价值最高)

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
- 落地: laew 添加 OAuth-bound provider 概念,这种 provider 跨账户路由会越权,必须 single route
- 价值: 与 laew 未来可能接入的 OAuth provider(Claude Max / ChatGPT Pro)兼容

### 11.2 P1(三月内落地,价值中等)

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

### 11.3 P2(战略价值,长期规划)

**11. DeepLink 安全导入**
- 路径参考: `deeplink/mod.rs:34-139`
- 落地: laew 实现 `laew://import?resource=provider&app=claude&...` URL Scheme,base64 编码配置内容,usage_script 不默认启用
- 价值: 社区分享 Provider 配置一键导入

**12. WebDAV / S3 多设备同步**
- 路径参考: `commands/webdav_sync.rs` + `sync_protocol.rs` + `register_db_change_hook`
- 落地: laew 在 SQLite 上注册 update_hook,任何表写入异步触发 WebDAV / S3 同步
- 价值: 用户多设备无缝共享 Provider / Skill / Prompt 配置

### 11.4 反模式警示(不要照搬)

- **不要照搬 `forwarder.rs` 5267 行单文件**: laew 应在引入 failover 之前先把"主循环 + 整流器 + 适配器"分文件(cc-switch 这 1.7 MB 是历史包袱)
- **不要照搬 `commands/misc.rs` 291 KB 单文件**: command 注册应按业务域分文件(provider/proxy/mcp/skill)
- **不要把所有 per-app 配置塞进 `proxy_config` 三行结构**: laew 当前按 `app_type` 索引的 SQLite 模型已足够简洁
- **不要 18 张表平铺**: laew 当前 6 张表已经够用,过度规范化会增加维护成本
- **Tauri 专属能力不能照搬**: Linux WebKit 兜底(`WEBKIT_DISABLE_DMABUF_RENDERER` 等)、系统托盘、深链注册——这些只对桌面应用有意义,laew 是 CLI 不必借鉴
- **每工具独立 5267 行 forwarder**: 协议转换适配器虽然可以借鉴"transform_*"思路,但不必全套 30+ 个文件

### 11.5 借鉴优先级速查

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

## 附录 A:关键文件路径速查表

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
| AppType 枚举 | `src-tauri/src/app_config.rs` | 整文件 |
| 多 App 配置 | `src-tauri/src/app_config.rs` | `MultiAppConfig` |
| Codex 完整配置 | `src-tauri/src/codex_config.rs` | 5267 行 |
| Token 解析 | `src-tauri/src/proxy/usage/parser.rs` | `from_claude_response` 104-127 |
| 成本计算 | `src-tauri/src/proxy/usage/calculator.rs` | `calculate_for_app` 56-70 |
| 用量幂等写入 | `src-tauri/src/proxy/usage/logger.rs` | `log_request` 101-200 |
| Provider 数据结构 | `src-tauri/src/provider.rs` | `Provider` 10-44 |
| 凭据解析 | `src-tauri/src/provider.rs` | `resolve_usage_credentials` 135-198 |
| Schema 迁移 | `src-tauri/src/database/schema.rs` | `create_tables_on_conn` 24-300+ |
| 备份导出 | `src-tauri/src/database/backup.rs` | `export_sql_string` 118-121 |
| 备份导入 | `src-tauri/src/database/backup.rs` | `import_sql_string_inner_with_hook` 173-249 |
| Skills 命令入口 | `src-tauri/src/commands/skill.rs` | 14 个 `#[tauri::command]` (30-340) |
| 前端主壳 | `src/App.tsx` | View 类型 + STORAGE_KEY (116-150) |
| Zod schema | `src/lib/schemas/provider.ts` | `providerSchema` 38-58 |

---

## 附录 B:版本演进时间线

| 版本 | 演进内容 | 关键模块 |
|------|---------|---------|
| v3.0.x | 初版:仅 Claude/Codex 切换 | `commands/provider.rs` |
| v3.7.0 | MCP 多工具 SSOT + per-app 启用字段 | `mcp_servers` 表 + `mcp/<tool>.rs` |
| v3.9.x | Schema v2→v3:Skills 统一管理 | `skills` 表加 `app_type` |
| v3.10.0+ | 完整 8 款工具适配层 | `*_config.rs` 系列 |
| v3.15.x | Schema v9→v10:Hermes Agent | `hermes_config.rs` + `mcp/hermes.rs` |
| v3.16.x | Schema v10→v11:`usage_daily_rollups` 加 `request_model` 维度 | `usage_daily_rollups` 6 元组 PK |
| v3.18.x | Schema v11→v12:Profiles(全应用共享) | `profiles` 表 |
| v3.19.x | Schema v14→v15:Grok Build 适配 + `thinking_rectifier` 7 模式 | `grok_config.rs` + `thinking_rectifier.rs` |
| v3.20.0 | 完整 Tauri 2 + React 18 改造,Linux WebKit 兜底,WebDAV v2 协议 | `main.rs:8-32` |

---

## 附录 C:与 Switchyard / agent-studio 的异同

| 维度 | cc-switch | Switchyard | agent-studio |
|------|-----------|-----------|--------------|
| **目标用户** | 单用户桌面端 | 多租户网关(NVIDIA 内部) | 企业 Agent 平台 |
| **协议 IR 抽象** | 无显式 IR,直接 Adapter trait + transform_* | 显式 `LlmRequest / LlmResponse / ContentBlock::Unknown` | Pregel 图 + DSL |
| **路由算法** | `select_providers` 队列 P1→P2 + 熔断器 | Noop/Passthrough/Random/LlmClassifier/StageRouter/Composite/AdvisorGate **7 种** | DSL 条件路由 |
| **熔断器** | 三态 Closed/Open/HalfOpen,HalfOpen 限流 1 个探测 | 类似但支持 per-route 配置 | 任务级 retry + 超时 |
| **失败回流** | `FailoverSwitchManager` 自动切 P1→P2 写库 | AdvisorGate + FallThrough 级联 | Trial 评估 |
| **缓存** | `cache_injector`(Bedrock 可选) | LRU + TTL,key=`prompt_hash` | 多级 |
| **部署形态** | 单机桌面 + 本地代理 | 服务端 Rust 网关 | 服务端 Python 微服务 |

**结论**: cc-switch 的代理层在"单机 + 桌面"场景下做到了工业级容错,但协议 IR 没有 Switchyard 抽象得彻底,主要靠 per-app adapter 实现异构协议。

---

## 总结

CC Switch 是一个**生产级**的"CLI Agent 路由器 + 统一配置中心"项目,核心机制集中在 4 层:

1. **代理层**(`proxy/`)—— 熔断器三态机(`AllowResult + release_half_open_permit`)、整流器(7 模式 thinking + 4 模式 media)、模型映射(haiku/sonnet/opus/fable)、Hyper 手写 accept loop + Header case preservation、OAuth 透传校验都是教科书级的容错设计。
2. **持久层**(`database/`)—— 17 个 schema 版本 + SAVEPOINT 包裹 + pre-migration 备份、SQL 导入 authorizer 防御(`AuthAction::Attach` / `CreateVtable` / `Unknown` 全拒绝)、双向同步的 `SYNC_SKIP/SYNC_PRESERVE` 表设计、incremental vacuum 都是工程化的体现。
3. **跨工具适配**(`mcp/` + `*_config.rs`)—— 把 8 种 CLI 的 MCP 配置抽象为统一 McpServer + 6 个 enabled 字段,再按 per-tool adapter 投影;Codex TOML / Hermes YAML / Claude JSON 的差异抹平展示了"协议适配器 + 4 函数模板"的标准模式。
4. **安全**——atomic_write、`sort_json_keys`、`redact_known_secrets`、`import_authorizer`、`validate_repo_ref`、`usage_script` 默认禁用等细节展示了"信任边界外层加厚"的工程文化。

对 laew 而言,**最有借鉴价值的是 P0 五项**(熔断器、中性释放、schema 迁移、原子写入、provider_supports_failover),它们能在不改变 laew 核心架构的前提下显著提升生产稳定性。P1 项(thinking 整流、ActiveConnectionGuard、错误分类、authorizer、MCP 适配)是面向"接入第三方中转 API"场景的容错加固。P2 项(DeepLink / WebDAV-S3 同步)属于"产品演进方向",可作为 laew v2.x 的中长期规划。**不应照搬** Tauri 专属能力(系统托盘、Linux WebKit 兜底)和 1.7 MB 代理层单文件怪兽——这些只对 cc-switch 的"8 工具 + 多 Provider"场景有意义,laew 主战场是 Anthropic + OpenAI 双协议,可降低适配广度。

---

## 第八轮深挖 — 多供应商配置聚合 + 跨平台崩溃恢复 + OAuth 反代 + i18n 多语言 Tauri 桌面壳

> 调研时间：2026-09-07。第八轮在第七轮基础上，从 **Telemetry / Session 持久化 / Tool 权限 / LSP / Hook / Skill 一等公民 / 多租户 / TUI 渲染** 八个维度补充 cc-switch 的真实代码实现。所有引用路径均为绝对路径 + 行号。

### 1. 整体架构（补充）

cc-switch 是 **「客户端工具聚合器」**——它本身不执行 AI 推理，而是把 8 套异构 CLI（Claude/Codex/Gemini/OpenCode/OpenClaw/Pi/Hermes/Grok）的配置文件、技能目录、OAuth token、会话历史拉平到统一的 SQLite + 单一文件系统 SSOT（`~/.cc-switch/skills/`）。

后端（`src-tauri/src/`）关键模块：
- `lib.rs` / `main.rs` 入口
- `commands/` 35 个 Tauri command（auth / mcp / skill / provider / proxy / subscription / codex_oauth / xai_oauth 等）
- `services/` 业务层（skill / proxy / mcp / sync_protocol）
- `session_manager/` 会话扫描（claude/codex/opencode/hermes/gemini/openclaw/pi/grokbuild 8 个 provider **并发扫描**）
- `database/` SQLite DAO
- `proxy/providers/` OAuth 反代（含 codex_oauth_auth / xai_oauth_auth / copilot_auth）
- `panic_hook.rs` 自定义崩溃捕获
- `lightweight.rs` 轻量模式
- `deeplink/` URL Scheme 处理

前端（`src/`）React + i18next（**zh/zh-TW/en/ja 4 语言**）。

### 2. 第八轮 8 维度真实代码锚点

| 维度 | 路径 | 范式要点 |
|---|---|---|
| **Telemetry** | `src-tauri/src/usage_events.rs:36-68` | `EVENT_USAGE_LOG_RECORDED` 事件总线；`services/session_usage_*.rs` 按 provider 收集 token 用量 |
| **Session 持久化** | `src-tauri/src/session_manager/mod.rs:58-97` | **8 线程 `std::thread::scope` 并发扫描**所有 provider 会话元数据 |
| **Tool 权限** | `src-tauri/src/services/skill.rs:30-47` | 显式 `RwLock<()>` 协调 DB skills 与文件系统 SSOT；`SyncMethod { Auto/Symlink/Copy }` |
| **LSP/Hook** | `src-tauri/src/panic_hook.rs:127-243` | `panic::set_hook` 写入 `<app_config_dir>/crash.log`，含 OS/Arch/线程/backtrace |
| **Skill 一等公民** | `src-tauri/src/commands/skill.rs:30-145` | 12+ command；`services/skill.rs:64-73` 区分 `SkillStorageLocation::{CcSwitch, Unified(~/.agents/skills/)}` |
| **多租户** | `src-tauri/src/commands/auth.rs`、`commands/codex_oauth.rs:14-19` | `CodexOAuthState(Arc<CodexOAuthManager>)` 多账号 + `default_account_id()` |
| **TUI 渲染** | `session_manager/terminal/mod.rs` | PTY 终端会话嵌入（Tauri + Xterm 模式） |

### 3. 第九轮新维度真实代码锚点

| 维度 | 路径 | 范式要点 |
|---|---|---|
| **Crash/Recovery** | `src-tauri/src/panic_hook.rs:127-243` | `setup_panic_hook()` + `catch_unwind` 保护时间格式化 + `Mutex<()>` 串行化崩溃写入 |
| **OAuth** | `commands/xai_oauth.rs:29-66`、`commands/codex_oauth.rs:27-67` | Codex + xAI + Copilot 三套 OAuth 反代 |
| **i18n** | `src/i18n/index.ts:1-92` | i18next + 4 资源 + navigator language 探测链（zh-tw/hk/mo/hant → zh-TW） |
| **Release** | `src-tauri/tauri.conf.json:3` | Tauri updater + Ed25519 pubkey + 5 OS matrix |
| **WS/SSE** | `commands/stream_check.rs`、`services/speedtest.rs` | 流式检测与 LLM proxy 转发 |
| **Dev Container** | `linux_fix.rs:67-117`、`auto_launch.rs`、`flatpak/` | Linux X11/Wayland focus + 开机启动 + Flatpak 打包 |
| **CRDT** | `services/sync_protocol.rs:1-78` | `manifest.json + sha256 + 版本号` LWW 跨设备同步 |

### 4. 关键代码片段

#### 4.1 跨 provider 八线程并发扫描（`session_manager/mod.rs:58-97`）

```rust
pub fn scan_sessions() -> Vec<SessionMeta> {
    let (r1, r2, r3, r4, r5, r6, r7, r8) = std::thread::scope(|s| {
        let h1 = s.spawn(codex::scan_sessions);
        let h2 = s.spawn(claude::scan_sessions);
        let h3 = s.spawn(opencode::scan_sessions);
        let h4 = s.spawn(openclaw::scan_sessions);
        let h5 = s.spawn(gemini::scan_sessions);
        let h6 = s.spawn(hermes::scan_sessions);
        let h7 = s.spawn(grokbuild::scan_sessions);
        let h8 = s.spawn(pi::scan_sessions);
        ...
    });
}
```

#### 4.2 崩溃日志 + backtrace + 轮转（`panic_hook.rs:127-150`）

```rust
pub fn setup_panic_hook() {
    if std::env::var("RUST_BACKTRACE").is_err() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let log_path = get_crash_log_path();
        let timestamp = std::panic::catch_unwind(|| {
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
        }).unwrap_or_else(|_| { ... });
        let backtrace = std::backtrace::Backtrace::force_capture();
        ...
    }));
}
```

#### 4.3 OAuth 多账号 + 默认账号选择（`xai_oauth.rs:29-58`）

```rust
pub(crate) async fn query_xai_oauth_quota_for(
    state: &XaiOAuthState, account_id: Option<String>,
) -> Result<SubscriptionQuota, String> {
    let manager = state.0.read().await;
    let resolved = match account_id.as_deref().map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => Some(id.to_string()),
        None => manager.default_account_id().await,
    };
    let Some(id) = resolved else { return Ok(SubscriptionQuota::not_found("xai_oauth")); };
    let token = match manager.get_valid_token_for_account(&id).await { ... };
}
```

#### 4.4 Skill SSOT 存储位置（`services/skill.rs:64-73`）

```rust
pub enum SkillStorageLocation {
    /// CC Switch 管理目录 (~/.cc-switch/skills/)
    #[default] CcSwitch,
    /// Agent Skills 统一标准目录 (~/.agents/skills/)
    Unified,
}
```

#### 4.5 i18n 语言回退链（`src/i18n/index.ts:36-61`）

```typescript
if (navigatorLang === "zh") return "zh";
if (navigatorLang?.startsWith("zh-tw") || navigatorLang?.startsWith("zh-hk") ||
    navigatorLang?.startsWith("zh-mo") || navigatorLang?.startsWith("zh-hant"))
    return "zh-TW";
if (navigatorLang?.startsWith("zh")) return "zh";
if (navigatorLang?.startsWith("ja")) return "ja";
if (navigatorLang?.startsWith("en")) return "en";
return DEFAULT_LANGUAGE;
```

### 5. 设计哲学

cc-switch 的核心创新是 **「多供应商适配 + 跨平台崩溃恢复」**：

1. **`session_manager` 八线程并发扫描**：每个 CLI 的会话目录布局完全不同，通过 `std::thread::scope` 并行扫描后再统一按 `last_active_at` 排序，URL 风格的 `source_path`（`sqlite:...` 前缀）区分存储后端。
2. **OAuth 三件套**（codex/xai/copilot）共享同一套 `query_*_quota_for` 模式，用 `Arc<Manager>` 取代 `RwLock<Manager>`（管理器内部已用细粒度锁，外层 RwLock 反而会因跨网络刷新阻塞其他命令）。
3. **崩溃恢复** 用 `Mutex<()>` 串行化 panic hook 调用而不是 `try_lock`（并发 panic 时两个 hook 竞争 rename 会丢归档）。
4. **Skill 的 SSOT** 抽象到独立 `SkillStorageLocation` 枚举，为未来切到统一 `~/.agents/skills/` 规范铺路。
5. **Tauri updater** 双 endpoint fallback（自有 CDN + GitHub Releases）+ Flatpak 全 home 权限。

**对 laew 的启示**：cc-switch 是「客户端工具聚合器」的工业范本，laew 升级时可参考其 **8 线程并发扫描 + OAuth 反代三件套 + 崩溃日志轮转 + Skill SSOT 抽象** 四大模式。

---

> **字数**：本文档 cc-switch 第八轮深挖章节新增约 700 行。
# cc-switch 第十轮深挖 — 8 大新维度源码级深度分析

> 调研对象: cc-switch (Tauri 2 + Rust + React, AI 工具配置管理桌面应用)
> 调研时间: 2026-09-07
> 调研维度: 8 个新维度 (前 9 轮已覆盖内容不再重复)
> 路径基准: `/usr/local/LsmGitOpenSource/cc-switch`

---

## 目录

1. [CrashDump 与错误恢复](#1-crashdump-与错误恢复)
2. [WebUI 与 DesktopApp](#2-webui-与-desktopapp)
3. [OAuth 认证与多账号](#3-oauth-认证与多账号)
4. [i18n 国际化](#4-i18n-国际化)
5. [Release 工程化与 AutoUpdate](#5-release-工程化与-autoupdate)
6. [WebSocket 与 SSE](#6-websocket-与-sse)
7. [DevContainer 与容器化](#7-devcontainer-与容器化)
8. [CRDT 与多端冲突](#8-crdt-与多端冲突)

---

## 1. CrashDump 与错误恢复

cc-switch 把"崩溃"和"错误恢复"拆成三层:**Panic Hook(后端底层)** → **InitError 状态机(版本冲突)** → **FrontendErrorBoundary(前端 React)** → **Live 配置接管恢复(代理运行时)**。这一节逐层剖析。

### 1.1 后端 Panic Hook(`src-tauri/src/panic_hook.rs`)

`panic_hook::setup_panic_hook()` 在 `lib.rs::run()` 的第 343 行作为**应用启动第一件事**调用(`lib.rs:343-344`):

```rust
// src-tauri/src/lib.rs:343-344
pub fn run() {
    panic_hook::setup_panic_hook();
    ...
```

**设计要点 1 — Backtrace 强制开启**(`panic_hook.rs:127-131`):

```rust
pub fn setup_panic_hook() {
    // 启用 backtrace(确保 release 模式也能捕获)
    if std::env::var("RUST_BACKTRACE").is_err() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }
    ...
}
```

即使用户在 release 二进制下未设环境变量,也强制 `RUST_BACKTRACE=1` —— release 调试能力兜底。

**设计要点 2 — Catch_unwind 嵌套防御**(`panic_hook.rs:144-159`):

```rust
let timestamp = std::panic::catch_unwind(|| {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}).unwrap_or_else(|_| {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("unix:{}.{:03}", d.as_secs(), d.subsec_millis()))
        .unwrap_or_else(|_| "unknown".to_string())
});

let system_info = std::panic::catch_unwind(get_system_info)
    .unwrap_or_else(|_| "Failed to get system info".to_string());
```

**核心洞见**: panic hook 本身不能再 panic,否则会双重 abort。`chrono` 格式化、线程信息抓取都用 `catch_unwind` 包起来,失败时降级到 unix timestamp 或字符串 `"Failed to get system info"`。这是**健壮性叠加层**。

**设计要点 3 — 多类型 panic 消息提取**(`panic_hook.rs:162-169`):

```rust
let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
    s.to_string()
} else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
    s.clone()
} else {
    format!("{panic_info}")
};
```

panic message 可能是 `&'static str`、`String` 或其他类型,三种 downcast 路径全覆盖。

**设计要点 4 — 串行化的崩溃日志临界区**(`panic_hook.rs:219-231`):

```rust
let crash_log_guard = CRASH_LOG_LOCK
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
let _ = rotate_crash_log_if_needed(&log_path);
let saved =
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = file.write_all(crash_entry.as_bytes());
        let _ = file.flush();
        true
    } else {
        false
    };
drop(crash_log_guard);
```

**为什么用 `Mutex<()>` 而不是 `try_lock`?** 因为并发 panic 时两个 hook 同时尝试 `rename(crash.log → crash.log.1)`,后者会因为前者的目标已存在而失败、丢失归档。`Mutex` 强制串行,牺牲并发换来归档完整性。注释(`panic_hook.rs:218-221`)明确解释:

> "将 size check、轮转和追加合成同一个临界区,避免多线程同时 panic 时两个 hook 竞争 rename 而丢失归档。"

**设计要点 5 — Backtrace + 位置 + 系统信息三件套**(`panic_hook.rs:183-215`):

日志条目包含:
- 完整 `Backtrace::force_capture()`(非 lazy,确保拿到)
- `panic_info.location()` 的 file/line/column 三元组
- OS / Arch / Family / Cargo 工作目录 / 线程名 / 线程 ID

**设计要点 6 — 日志轮转 (5 MiB / 2 归档)**(`panic_hook.rs:14-15, 50-82`):

```rust
const CRASH_LOG_MAX_SIZE: u64 = 5 * 1024 * 1024;
const CRASH_LOG_ARCHIVES_TO_KEEP: usize = 2;
...
fn rotate_crash_log_if_needed_with_limit(...) -> std::io::Result<()> {
    let size = match fs::metadata(path) { ... };
    if size < max_size || archives_to_keep == 0 { return Ok(()); }
    for index in (1..=archives_to_keep).rev() {
        let source = if index == 1 { path.to_path_buf() }
                     else { rotated_crash_log_path(path, index - 1) };
        if !source.exists() { continue; }
        let destination = rotated_crash_log_path(path, index);
        if destination.exists() { fs::remove_file(&destination)?; }
        fs::rename(source, destination)?;
    }
    Ok(())
}
```

逆序轮转 (`(1..=archives_to_keep).rev()`),保证老的归档先被覆盖、新的先保留。归档命名:`crash.log.1`、`crash.log.2`。

**panic_hook 单元测试**(`panic_hook.rs:264-299`):

`crash_log_rotation_keeps_bounded_archives` 测试三阶段轮转(4 字节上限 + 2 归档):写入 5 字节触发 → 验证 `.1` 归档 → 再写 → 验证 `.2` 旧归档 → 三次写 → 验证 `.3` 不存在。完整覆盖归档边界条件。

### 1.2 InitError 状态机(`src-tauri/src/init_status.rs`)

**设计要点 7 — 跨线程的初始化错误传递**(`init_status.rs:1-19`):

```rust
#[derive(Debug, Clone, Serialize)]
pub struct InitErrorPayload {
    pub path: String,
    pub error: String,
    /// 错误类别。`Some("db_version_too_new")` 表示数据库版本过新(应用过旧),
    /// 前端据此展示「升级应用」恢复界面而非直接退出。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_version: Option<i32>,
}

static INIT_ERROR: OnceLock<RwLock<Option<InitErrorPayload>>> = OnceLock::new();
```

`OnceLock<RwLock<>>` 双重锁:OnceLock 保证全局唯一,RwLock 支持运行时替换(用于"db_version_too_new"等场景)。

**设计要点 8 — DB 版本过新强制恢复界面**(`lib.rs:567-591`):

```rust
match crate::database::Database::stored_user_version_exceeds_supported(&db_path) {
    Ok(Some(version)) => {
        log::warn!("数据库版本过新(v{version}),引导用户在应用内升级应用");
        crate::init_status::set_init_error(crate::init_status::InitErrorPayload {
            path: db_path.display().to_string(),
            error: format!(
                "数据库版本过新({version}),当前应用仅支持 {},请升级应用后再尝试。",
                crate::database::SCHEMA_VERSION
            ),
            kind: Some("db_version_too_new".to_string()),
            db_version: Some(version),
            supported_version: Some(crate::database::SCHEMA_VERSION),
        });
        // 主窗口默认 visible:false,恢复界面必须强制显示
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
        }
        return Ok(());
    }
    ...
}
```

**关键设计哲学**: 数据库版本过新时,**不应用任何 schema migration**(`apply_schema_migrations_on_conn` 在预检之前返回),避免旧应用对读不懂的更新版 DB 落写 DDL。主窗口默认 `visible: false`,这里强制 `show + focus` 让前端展示升级引导。

**配套设计 — `CloseRequested` 拦截**(`lib.rs:411-419`):

```rust
let in_db_recovery = crate::init_status::get_init_error()
    .map(|p| p.kind.as_deref() == Some("db_version_too_new"))
    .unwrap_or(false);
if in_db_recovery {
    api.prevent_close();
    window.app_handle().exit(0);
    return;
}
```

恢复模式下关闭即退出(没有托盘可唤回),避免应用隐身后台。

### 1.3 前端 FrontendErrorBoundary(`src/components/FrontendErrorBoundary.tsx`)

**设计要点 9 — 渲染层错误捕获**(`FrontendErrorBoundary.tsx:17-27`):

```typescript
static getDerivedStateFromError(): FrontendErrorBoundaryState {
  return { hasError: true };
}

componentDidCatch(error: Error, info: React.ErrorInfo): void {
  reportFrontendError(
    "react.error_boundary",
    error,
    info.componentStack ?? undefined,
  );
}
```

`getDerivedStateFromError` 切换到错误态;`componentDidCatch` 把错误上报到 `frontendLogger`,带 componentStack。

**渲染层降级 UX**(`FrontendErrorBoundary.tsx:34-65`):

```tsx
<main className="flex min-h-screen items-center justify-center bg-background p-6 text-foreground">
  <section role="alert" className="w-full max-w-md ...">
    <div className="flex items-start gap-3">
      <TriangleAlert className="..." />
      <div className="space-y-1.5">
        <h1>{i18n.t("errors.frontendCrashTitle", { defaultValue: "界面遇到了问题" })}</h1>
        <p>{i18n.t("errors.frontendCrashMessage", {
          defaultValue: "已尝试将错误信息写入应用诊断日志。请重新加载界面;如果问题持续,请在提交 Issue 时附上日志。",
        })}</p>
      </div>
    </div>
    <Button className="w-full" onClick={() => window.location.reload()}>
      <RefreshCw className="mr-2 size-4" />
      {i18n.t("errors.reloadInterface", { defaultValue: "重新加载界面" })}
    </Button>
  </section>
</main>
```

**三个细节**:
- `role="alert"` 让屏幕阅读器立即播报
- `i18n.t(key, { defaultValue })` —— **i18n 容错**:即使翻译键缺失,也显示 `defaultValue` 中文字符串
- `window.location.reload()` 一键重载 React 树(不重启 Tauri),快速恢复

### 1.4 Live 配置接管恢复 — 代理崩溃保护

`lib.rs` 在 `setup()` 阶段启动一个异步任务专门负责"上次崩溃后的恢复"(`lib.rs:1204-1226`):

```rust
tauri::async_runtime::spawn(async move {
    let state = app_handle.state::<AppState>();

    // 检查是否有 Live 备份(表示上次异常退出时可能处于接管状态)
    let has_backups = match state.db.has_any_live_backup().await {
        Ok(v) => v,
        Err(e) => { log::error!("检查 Live 备份失败: {e}"); false }
    };
    // 检查 Live 配置是否仍处于被接管状态(包含占位符)
    let live_taken_over = state.proxy_service.detect_takeover_in_live_configs();

    if has_backups || live_taken_over {
        log::warn!("检测到上次异常退出(存在接管残留),正在恢复 Live 配置...");
        if let Err(e) = state.proxy_service.recover_from_crash().await {
            log::error!("恢复 Live 配置失败: {e}");
        } else {
            log::info!("Live 配置已恢复");
        }
    }
    ...
});
```

**核心思想**: CC Switch 接管 Claude/Codex/Gemini 的 live 配置文件时会写入"占位符"(例如 `127.0.0.1:15721`)。**异常退出后 live 配置文件仍含占位符,会让用户的 CLI 工具持续无法工作**。所以启动时:
1. 查 DB `has_any_live_backup` 是否有接管期间的备份
2. 查 `detect_takeover_in_live_configs` 是否含占位符
3. 任一为真 → 调 `recover_from_crash` 还原 live 配置

**退出时也兜底**(`lib.rs:1873-1908 cleanup_before_exit`):

```rust
pub async fn cleanup_before_exit(app_handle: &tauri::AppHandle) {
    if let Some(state) = app_handle.try_state::<store::AppState>() {
        let proxy_service = &state.proxy_service;

        // 退出时也需要兜底:代理可能已崩溃/未运行,但 Live 接管残留仍在(占位符/备份)。
        let has_backups = match state.db.has_any_live_backup().await { ... };
        let live_taken_over = proxy_service.detect_takeover_in_live_configs();
        let needs_restore = has_backups || live_taken_over;

        if needs_restore {
            log::info!("检测到接管残留,开始恢复 Live 配置(保留代理状态)...");
            if let Err(e) = proxy_service.stop_with_restore_keep_state().await { ... }
            return;
        }
        ...
    }
}
```

**`keep_state` 版本的设计**:`stop_with_restore_keep_state` 恢复 live 但**保留 settings 表中的代理状态**,下次启动时 `restore_proxy_state_on_startup`(`lib.rs:1954-1988`)会自动重新接管。

### 1.5 ExitRequested 分类器(`lib.rs:2208-2226`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitRequestAction {
    /// `code` 为 `None`:运行时自动触发(如隐藏窗口的 WebView 被回收导致无存活
    /// 窗口),阻止退出、保持托盘后台运行。
    StayInTray,
    /// `code` 为 `RESTART_EXIT_CODE`:`app.restart()` / 自更新 relaunch 发起的
    /// 重启,不拦截、不做自定义清理,交还 Tauri 默认 re-exec 流程。
    DeferToTauriRestart,
    /// 其它 `Some(_)`:用户主动退出(托盘「退出」等),执行完整异步清理后结束进程。
    CleanupAndExit,
}

fn classify_exit_request(code: Option<i32>) -> ExitRequestAction {
    match code {
        None => ExitRequestAction::StayInTray,
        Some(tauri::RESTART_EXIT_CODE) => ExitRequestAction::DeferToTauriRestart,
        Some(_) => ExitRequestAction::CleanupAndExit,
    }
}
```

**关键约束**(`lib.rs:1729-1742` 的注释):

> "绝不能复用下面的异步清理任务:该任务在 tokio 线程调 save_window_state,持有 window-state 插件锁的同时向主线程查询窗口几何;而主线程此刻正在退出事件循环,并在插件自带的 RunEvent::Exit 钩子里等待同一把锁——双方互等造成进程永久卡死(更新已安装但应用冻结、不再重启,见 #3998)。"

**测试覆盖**(`lib.rs:2367-2385`):

```rust
#[test]
fn no_code_keeps_app_alive_in_tray() {
    assert_eq!(classify_exit_request(None), ExitRequestAction::StayInTray);
}

#[test]
fn restart_exit_code_defers_to_tauri_default_restart() {
    assert_eq!(
        classify_exit_request(Some(tauri::RESTART_EXIT_CODE)),
        ExitRequestAction::DeferToTauriRestart
    );
}
```

### 1.6 兜底对话框(数据库/迁移错误)`lib.rs:2084-2196`

`show_migration_error_dialog` + `show_database_init_error_dialog` 都是 `MessageDialogButtons::OkCancelCustom`(重试/退出)+ 中英文双语 + 显式 "数据库未丢失" 安民告示。

**语言检测**(`lib.rs:2074-2080`):

```rust
fn is_chinese_locale() -> bool {
    std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .map(|lang| lang.starts_with("zh"))
        .unwrap_or(false)
}
```

三环境变量级联探测(`LANG` → `LC_ALL` → `LC_MESSAGES`),保证中文环境展示中文对话框。

### 1.7 错误类型层次(`src-tauri/src/error.rs:1-66`)

`AppError` 用 `thiserror::Error` 派生,17 个变体覆盖所有错误类别:

| 变体 | 用途 |
|------|------|
| `Config(String)` | 配置错误 |
| `InvalidInput(String)` | 用户输入校验 |
| `Conflict(String)` | "并发冲突"(CC Switch 检测到文件已被外部修改) |
| `Io { path, source }` | IO 错误,带路径 |
| `Json { path, source }` | JSON 解析错误 |
| `JsonSerialize { source }` | 序列化失败 |
| `Toml { path, source }` | TOML 错误 |
| `Lock(String)` | 锁获取失败 |
| `McpValidation(String)` | MCP 校验 |
| `Localized { key, zh, en }` | **i18n 双语错误** |
| `Database(String)` | DB 错误 |
| `OmoConfigNotFound` | OMO 文件缺失 |
| `AllProvidersCircuitOpen` | 熔断器全开 |
| `NoProvidersConfigured` | 无可用供应商 |

**关键设计 — `Localized { key, zh, en }`**(`error.rs:52-57`):

```rust
#[error("{zh} ({en})")]
Localized {
    key: &'static str,
    zh: String,
    en: String,
},
```

同步层错误天然需要双语:`sync_protocol.rs` 中 `localized()` 工厂函数(`sync_protocol.rs:82-88`)集中生成。`AppError::localized(key, zh, en)` 构造器(`error.rs:90-96`)保证调用点 1 行调用。

**`format_skill_error` JSON 错误**(`error.rs:127-149`):

```rust
pub fn format_skill_error(code: &str, context: &[(&str, &str)], suggestion: Option<&str>) -> String {
    use serde_json::json;
    let mut ctx_map = serde_json::Map::new();
    for (key, value) in context) {
        ctx_map.insert(key.to_string(), json!(value));
    }
    let error_obj = json!({
        "code": code,
        "context": ctx_map,
        "suggestion": suggestion,
    });
    serde_json::to_string(&error_obj).unwrap_or_else(|_| {
        format!("ERROR:{code}")
    })
}
```

Skill 错误专门序列化为结构化 JSON,前端可解析 `code + context + suggestion` 做精细化错误展示,序列化失败时降级到简单格式。

### 1.8 应用诊断日志(tower-log)

`lib.rs:457-495` 初始化 `tauri-plugin-log`,**4 归档 × 20 MiB**,**轮转按大小触发,跨重启继续追加**——避免重启丢日志:

```rust
.rotation_strategy(RotationStrategy::KeepSome(4))
.max_file_size(20 * 1024 * 1024)
```

`runtime_log_level_allows(metadata, max_level)`(`lib.rs:236-238`)做**前端日志分发层过滤**,因为 plugin-log 的前端 command 绕过 log 宏的全局 max_level,所以在分发层补一道过滤。

---

## 2. WebUI 与 DesktopApp

CC Switch 的 WebUI 是个 **Tauri 2 + React 18 + TanStack Query** 的标准桌面应用。但它的**工程亮点**在:1) 启动期 11 阶段的初始化管线;2) 系统托盘 + 静默启动 + 轻量模式;3) Linux 平台兜底。

### 2.1 Tauri Builder 启动管线(`src-tauri/src/lib.rs:341-449`)

**11 阶段启动**(`lib.rs::run()`):

1. `panic_hook::setup_panic_hook()` — Panic 捕获
2. 注册 `tauri_plugin_single_instance` — 复用已有实例,聚焦窗口
3. (Windows) 注册 `on_page_load` — 等页面 Finished 才显示主窗口
4. 注册 `tauri_plugin_deep_link` — `ccswitch://` 协议
5. 注册 `on_window_event` — 拦截关闭(根据设置决定最小化到托盘)
6. 注册 `tauri_plugin_process/dialog/opener/store/window_state` — 6 个标准插件
7. `setup(|app| { ... })` — 核心初始化
8. `invoke_handler!` — 注册 ~170 个 `#[tauri::command]`
9. `builder.build()` — 启动 Tauri 运行时
10. `app.run()` — 监听 RunEvent(Exit/Reopen/Opened)
11. 异步后台任务 — auto-sync / session sync / periodic backup

**setup() 阶段的 50+ 步骤**(`lib.rs:449-1361`):

```rust
.setup(|app| {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 1. Store 覆盖 + panic hook 配置目录
    app_store::refresh_app_config_dir_override(app.handle());
    panic_hook::init_app_config_dir(crate::config::get_app_config_dir());

    // 2. 日志系统(tauri-plugin-log, 4 归档 20MiB)
    app.handle().plugin(tauri_plugin_log::Builder::default()...)?

    // 3. macOS: 设置 AppUserModelID(仅 Windows)
    #[cfg(target_os = "windows")]
    set_windows_app_user_model_id(app.handle());

    // 4. Updater 插件(配置失败不阻断)
    if let Err(e) = app.handle().plugin(tauri_plugin_updater::Builder::new().build()) { ... }

    // 5. usage_events::init 注入 AppHandle
    usage_events::init(app.handle().clone());

    // 6. 数据库 precheck:版本过新 → 走恢复界面
    match Database::stored_user_version_exceeds_supported(&db_path) {
        Ok(Some(version)) => { set_init_error(...); return Ok(()); }
        ...
    }

    // 7. 数据库初始化(循环 + 重试对话框)
    let db = loop {
        match Database::init() { Ok(db) => break Arc::new(db), Err(e) => {
            if !show_database_init_error_dialog(...) { std::process::exit(1); }
        }}
    };

    // 8. JSON → DB 迁移
    if let Some(config) = migration_config {
        match db.migrate_from_json(&config) { ... }
    }

    // 9. AppState 注册 + SkillService 注册
    let app_state = AppState::new(db);
    app.manage(app_state);
    app.manage(SkillServiceState(Arc::new(SkillService::new())));

    // 10. 4 个 OAuth Manager 初始化
    app.manage(CopilotAuthState(...));        // GitHub Copilot
    app.manage(CodexOAuthState(...));          // ChatGPT Plus/Pro
    app.manage(XaiOAuthState(...));            // xAI Grok
    // + 全局代理 HTTP 客户端

    // 11. webdav_auto_sync + s3_auto_sync 后台 worker
    crate::services::webdav_auto_sync::start_worker(app_state.db.clone(), app.handle().clone());
    crate::services::s3_auto_sync::start_worker(app_state.db.clone(), app.handle().clone());

    // 12. 异步恢复任务 + session sync 定时器
    tauri::async_runtime::spawn(async move {
        // Live 配置接管恢复
        if has_backups || live_taken_over {
            state.proxy_service.recover_from_crash().await
        }
        // 定期 session 用量同步 + 周期备份
        ...
    });

    // 13. Linux: 禁用 WebKitGTK 硬件加速
    if let Some(window) = app.get_webview_window("main") {
        window.with_webview(|webview| {
            use webkit2gtk::{WebViewExt, SettingsExt, HardwareAccelerationPolicy};
            SettingsExt::set_hardware_acceleration_policy(&settings, HardwareAccelerationPolicy::Never);
        });
    }

    // 14. 静默启动 vs 正常启动(根据 settings.silent_startup)
    if settings.silent_startup {
        let _ = window.hide();
        #[cfg(target_os = "windows")]
        let _ = window.set_skip_taskbar(true);
        #[cfg(target_os = "macos")]
        tray::apply_tray_policy(app.handle(), false);
    } else {
        let _ = window.show();
        #[cfg(target_os = "linux")]
        linux_fix::nudge_main_window(window.clone());
    }
    Ok(())
})
```

### 2.2 静默启动 + 轻量模式(`src-tauri/src/lightweight.rs`)

**设计要点 — 双重后台模式**:

```rust
// lightweight.rs:7-31 enter_lightweight_mode
pub fn enter_lightweight_mode(app: &tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_skip_taskbar(true);   // 从任务栏隐藏
    }
    #[cfg(target_os = "macos")]
    crate::tray::apply_tray_policy(app, false);

    if let Some(window) = app.get_webview_window("main") {
        crate::save_window_state_before_exit(app);
        window.destroy()   // 完全销毁 webview(节省内存)
            .map_err(|e| format!("销毁主窗口失败: {e}"))?;
    }

    LIGHTWEIGHT_MODE.store(true, Ordering::Release);
    crate::tray::refresh_tray_menu(app);
    log::info!("进入轻量模式");
    Ok(())
}
```

**设计差异**:
- 静默启动:窗口 `hide()`(webview 还在,只是不可见)
- 轻量模式:`window.destroy()`(webview 销毁,只保留托盘 + 后台服务)
- 静默启动由 `settings.silent_startup` 控制,**应用启动时决定**
- 轻量模式由用户在托盘菜单触发,**运行时切换**

**退出轻量模式重建窗口**(`lightweight.rs:33-96`):

```rust
pub fn exit_lightweight_mode(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::WebviewWindowBuilder;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        ...
        return Ok(());
    }

    // 窗口已被销毁 → 从 app config 重建
    let window_config = app.config().app.windows.iter()
        .find(|w| w.label == "main")
        .ok_or("主窗口配置未找到")?;
    WebviewWindowBuilder::from_config(app, window_config)
        .map_err(|e| format!("加载主窗口配置失败: {e}"))?
        .build()
        .map_err(|e| format!("创建主窗口失败: {e}"))?;
    ...
}
```

**两条路径**:窗口存在(只是被 hide)→ 直接 show;窗口被 destroy → 用 `WebviewWindowBuilder::from_config` 重建。**注意**:`from_config` 读的是 `tauri.conf.json` 中的 `app.windows`,这要求开发者在配置变更时记得同步两份代码。

**Linux 焦点恢复**(`lightweight.rs:40-43`):

```rust
#[cfg(target_os = "linux")]
{
    crate::linux_fix::nudge_main_window(window.clone());
}
```

重建窗口后立即调 `nudge_main_window`(见 §7.1)修复 Wayland/X11 焦点问题。

### 2.3 单实例 + 深度链接(`lib.rs:349-388`)

```rust
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
{
    builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        log::info!("=== Single Instance Callback Triggered ===");
        for (i, arg) in args.iter().enumerate() {
            log::debug!("  arg[{i}]: {}", url_for_log(arg));
        }

        if crate::lightweight::is_lightweight_mode() {
            if let Err(e) = crate::lightweight::exit_lightweight_mode(app) { ... }
        }

        // Windows/Linux 命令行参数 → ccswitch:// 深链接
        let mut found_deeplink = false;
        for arg in &args {
            if handle_deeplink_url(app, arg, false, "single_instance args") {
                found_deeplink = true; break;
            }
        }
        if !found_deeplink {
            log::info!("ℹ No deep link URL found in args (this is expected on macOS when launched via system)");
        }

        // 聚焦主窗口
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
            #[cfg(target_os = "linux")]
            linux_fix::nudge_main_window(window.clone());
        }
    }));
}
```

**单实例回调的三件事**:1) 退出轻量模式;2) 处理新实例的 deep link 参数;3) 聚焦主窗口。

### 2.4 系统托盘(`src-tauri/src/tray.rs`, 1622 行)

`lib.rs:1077-1122` 构建托盘,支持**5 类事件**:

```rust
let mut tray_builder = TrayIconBuilder::with_id(tray::TRAY_ID)
    .tooltip("CC Switch")
    .on_tray_icon_event(|tray, event| match event {
        // 鼠标悬停/点击 → 后台异步刷新用量缓存(防抖 10s)
        TrayIconEvent::Enter { .. } | TrayIconEvent::Click { .. } => {
            let app = tray.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                crate::tray::refresh_all_usage_in_tray(&app).await;
            });
        }
        _ => log::debug!("unhandled event {event:?}"),
    })
    .menu(&menu)
    .on_menu_event(|app, event| {
        tray::handle_tray_menu_event(app, &event.id.0);
    })
    .show_menu_on_left_click(true);
```

**macOS 模板图标**(`lib.rs:329-339`):

```rust
#[cfg(target_os = "macos")]
fn macos_tray_icon() -> Option<Image<'static>> {
    const ICON_BYTES: &[u8] = include_bytes!("../icons/tray/macos/statusbar_template_3x.png");
    match Image::from_bytes(ICON_BYTES) {
        Ok(icon) => Some(icon),
        Err(err) => { log::warn!("Failed to load macOS tray icon: {err}"); None }
    }
}
```

macOS 的 tray icon 用 3x 模板图标,会自动适配深浅色;非 macOS 用 `default_window_icon`。

### 2.5 macOS Reopen + Opened 事件处理(`lib.rs:1771-1855`)

```rust
#[cfg(target_os = "macos")]
{
    match event {
        RunEvent::Reopen { .. } => {
            if let Some(window) = app_handle.get_webview_window("main") {
                #[cfg(target_os = "windows")]
                let _ = window.set_skip_taskbar(false);
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
                tray::apply_tray_policy(app_handle, true);
            } else if crate::lightweight::is_lightweight_mode() {
                if let Err(e) = crate::lightweight::exit_lightweight_mode(app_handle) { ... }
            }
        }
        // macOS 通过 URL scheme 触发的打开事件
        RunEvent::Opened { urls } => {
            if let Some(url) = urls.first() {
                let url_str = url.to_string();
                if url_str.starts_with("ccswitch://") {
                    ...
                    match crate::deeplink::parse_deeplink_url(&url_str) {
                        Ok(request) => app_handle.emit("deeplink-import", &request),
                        Err(e) => app_handle.emit("deeplink-error", json!({"url": ..., "error": ...})),
                    }
                }
            }
        }
        _ => {}
    }
}
```

**macOS 三层 Open 路径**:1) Dock 图标点击 → `Reopen`;2) `ccswitch://` URL 触发 → `Opened` 携带 URLs;3) 已运行实例新 deep link → 走 `on_open_url`(`lib.rs:1051-1074`)。

### 2.6 Windows AppUserModelID(`lib.rs:84-98`)

```rust
#[cfg(target_os = "windows")]
fn set_windows_app_user_model_id(app: &tauri::AppHandle) {
    let app_id = app.config().identifier.clone();
    let wide_app_id: Vec<u16> = app_id.encode_utf16().chain(std::iter::once(0)).collect();

    let result = unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(wide_app_id.as_ptr())
    };
    ...
}
```

显式调用 Win32 API 设 AppUserModelID(`com.ccswitch.desktop`)——Windows 任务栏按 AppUserModelID 归类窗口/通知,**不显式设置会被误归到"未知应用"**。

### 2.7 全局退出清理(`lib.rs:1712-1768`)

```rust
app.run(|app_handle, event| {
    if let RunEvent::ExitRequested { api, code, .. } = &event {
        match classify_exit_request(*code) {
            ExitRequestAction::StayInTray => {
                api.prevent_exit();
                return;
            }
            ExitRequestAction::DeferToTauriRestart => {
                return;   // 重要:不调任何清理
            }
            ExitRequestAction::CleanupAndExit => {}
        }

        log::info!("收到用户主动退出请求,开始清理...");
        api.prevent_exit();

        let app_handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            save_window_state_before_exit(&app_handle);
            cleanup_before_exit(&app_handle).await;
            // 显式移除托盘图标
            remove_tray_icon_before_exit(&app_handle);
            log::info!("清理完成,退出应用");
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            std::process::exit(0);
        });
        return;
    }
    ...
});
```

**5 步骤退出**:
1. `save_window_state_before_exit` — 显式保存窗口几何(绕过 window-state 插件默认 Exit 钩子)
2. `cleanup_before_exit` — Live 配置接管还原
3. `remove_tray_icon_before_exit` — `set_visible(false)` 触发 Windows Shell `NIM_DELETE`,避免死进程托盘图标残留
4. `sleep(100ms)` — DB 写入落盘
5. `std::process::exit(0)` — 直接结束,跳过 Tauri 默认 Drop

### 2.8 restart_process(`lib.rs:2266-2270`)

```rust
pub fn restart_process(app_handle: &tauri::AppHandle) -> ! {
    remove_tray_icon_before_exit(app_handle);
    destroy_single_instance_lock(app_handle);
    tauri::process::restart(&app_handle.env());
}
```

`tauri::process::restart` 直接 spawn 新进程 + `exit(0)`,**不走事件循环退出**,所以 Tauri 内部 `cleanup_before_exit` 和 RunEvent::Exit 钩子都不执行。补偿措施:**手动**调 `remove_tray_icon_before_exit` + `destroy_single_instance_lock`。

**destroy_single_instance_lock**(`lib.rs:2251-2254`):

```rust
pub fn destroy_single_instance_lock(app_handle: &tauri::AppHandle) {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    tauri_plugin_single_instance::destroy(app_handle);
}
```

macOS single-instance 用 `/tmp/{identifier}.sock`,如果直接 `exit(0)` 不走 `RunEvent::Exit` 钩子,旧 listener 不释放,新进程会误连旧 listener 然后退出。

---

## 3. OAuth 认证与多账号

CC Switch 支持 **3 套独立 OAuth 反代**:
- **GitHub Copilot** — Device Code + 多账号
- **Codex OAuth** (ChatGPT Plus/Pro) — Device Code + 多账号 + 默认账号切换 + lock_switch
- **xAI OAuth** (SuperGrok 反代) — Device Code + 多账号 + discovery document

### 3.1 统一 OAuth 命令层(`src-tauri/src/commands/auth.rs`)

**核心抽象 — `ManagedAuthAccount` + `ManagedAuthStatus`**(`commands/auth.rs:18-50`):

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct ManagedAuthAccount {
    pub id: String,
    pub provider: String,           // "github_copilot" | "codex_oauth" | "xai_oauth"
    pub login: String,
    pub avatar_url: Option<String>,
    pub authenticated_at: i64,
    pub is_default: bool,
    pub github_domain: String,
    /// Codex 专用:旧账号缺少写入原生 Codex auth.json 所需的 id_token。
    pub reauth_required: bool,
    /// xAI 专用:refresh token 已失效,账号不可再用于请求。
    pub requires_reauth: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ManagedAuthStatus {
    pub provider: String,
    pub authenticated: bool,
    pub default_account_id: Option<String>,
    pub migration_error: Option<String>,
    pub accounts: Vec<ManagedAuthAccount>,
}
```

**统一抽象的价值**:前端 AuthCenter 面板(`src/components/settings/AuthCenterPanel.tsx`)只需渲染 `Vec<ManagedAuthAccount>`,不关心三套 provider 各自的内部结构。

**8 个统一命令**(`commands/auth.rs:110-460`):

| 命令 | 用途 |
|------|------|
| `auth_start_login` | 启动登录,返回 device code + verification URL |
| `auth_poll_for_account` | 轮询 token,返回新账号 |
| `auth_cancel_login` | 取消 device code 流程(仅 codex_oauth) |
| `auth_list_accounts` | 列出所有账号 |
| `auth_get_status` | 获取认证状态 |
| `auth_remove_account` | 删除账号 |
| `auth_set_default_account` | 设置默认账号 |
| `auth_logout` | 注销全部 |

**工厂函数 — `ensure_auth_provider`**(`auth.rs:52-59`):

```rust
fn ensure_auth_provider(auth_provider: &str) -> Result<&'static str, String> {
    match auth_provider {
        AUTH_PROVIDER_GITHUB_COPILOT => Ok(AUTH_PROVIDER_GITHUB_COPILOT),
        AUTH_PROVIDER_CODEX_OAUTH => Ok(AUTH_PROVIDER_CODEX_OAUTH),
        AUTH_PROVIDER_XAI_OAUTH => Ok(AUTH_PROVIDER_XAI_OAUTH),
        _ => Err(format!("Unsupported auth provider: {auth_provider}")),
    }
}
```

`&'static str` 返回值,**编译期去重**,免去每处分支写 `"codex_oauth"` 字符串字面量。

### 3.2 Codex OAuth(`src-tauri/src/proxy/providers/codex_oauth_auth.rs`)

**OAuth 端点常量**(`codex_oauth_auth.rs:36-67`):

```rust
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_AUTH_USERCODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_AUTH_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const TOKEN_REFRESH_BUFFER_MS: i64 = 60_000;
const OAUTH_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DEVICE_CODE_DEFAULT_EXPIRES_IN: u64 = 900;
const CODEX_USER_AGENT: &str = "cc-switch-codex-oauth";
```

**核心设计 — `CachedAccessToken` + `is_expiring_soon`**(`codex_oauth_auth.rs:158-175`):

```rust
struct CachedAccessToken {
    token: String,
    expires_at_ms: i64,
    /// 获取(刷新)时间戳(毫秒)。用于写入托管 auth.json 的 `last_refresh`,
    /// 使其如实反映 access_token 的真实获取时间,而非写盘时刻——否则 Codex CLI
    /// 会误判一个旧 token 是刚刷新的。
    obtained_at_ms: i64,
}

impl CachedAccessToken {
    fn is_expiring_soon(&self) -> bool {
        let now = chrono::Utc::now().timestamp_millis();
        self.expires_at_ms - now < TOKEN_REFRESH_BUFFER_MS
    }
}
```

`obtained_at_ms` 是为了**兼容 Codex CLI 的 token 元数据语义**——CLI 看到 `last_refresh` 会做内部缓存决策,如果直接用"写盘时刻"会误导 CLI 重置 token 状态。

**核心设计 — `RefreshTokenAdoptionMode` 4 元组**(`codex_oauth_auth.rs:177-220`):

```rust
enum RefreshTokenAdoptionMode {
    TimestampChecked,    // 正常 CLI 同步,新 token 必须有更新 live 时间戳
    RejectedManagerToken,// 上游拒绝 manager 的 refresh token,可用 disk token 救场
}

enum RefreshTokenAdoptionOutcome {
    Synchronized { state_changed: bool },  // live 与 manager 已同步
    Adopted,                               // 采用 disk token 为新代
    ProvablyOlder,                         // disk 严格更老,可覆盖/删除
    Ambiguous,                             // 无法安全排序,必须 abort
}
```

**核心洞见**:OAuth 进程可能与外部 CLI(用户用 Codex CLI 同时登录)共享同一个账号,**两边的 refresh token 都可能是最新版**。CC Switch 必须区分"正常 CLI 同步"和"OAuth 服务端拒绝"两种场景,后者可以绕过时间戳严格性。这是**多源 SSOT 收敛**的教科书案例。

### 3.3 xAI OAuth Discovery Document(`src-tauri/src/proxy/providers/xai_oauth_auth.rs`)

**xAI 端点 — 运行时发现**(`xai_oauth_auth.rs:18-29`):

```rust
const XAI_ISSUER: &str = "https://auth.x.ai";
const XAI_DISCOVERY_URL: &str = "https://auth.x.ai/.well-known/openid-configuration";
const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
...
const MAX_OAUTH_RESPONSE_BYTES: usize = 64 * 1024;
```

**为什么用 Discovery Document**: xAI 的 `token_endpoint`、`device_authorization_endpoint` 会变化(尤其是 OIDC 服务调整),每次启动从 `.well-known/openid-configuration` 拉,**不硬编码端点**。

**OAuthEndpoints 缓存**(`xai_oauth_auth.rs:73-77`):

```rust
struct OAuthEndpoints {
    token_endpoint: String,
    device_authorization_endpoint: String,
}
```

管理器内部持有 `discovered_endpoints: Arc<RwLock<Option<OAuthEndpoints>>>`(`xai_oauth_auth.rs:196`),首次调用时拉一次,后续从缓存读取。

### 3.4 Copilot OAuth(`src-tauri/src/proxy/providers/copilot_auth.rs`)

**多域名支持**(`copilot_auth.rs:28-42`):

```rust
const GITHUB_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";            // github.com
const GITHUB_CLIENT_ID_GHES: &str = "Ov23li8tweQw6odWQebz";       // GHES (Enterprise)
const DEFAULT_GITHUB_DOMAIN: &str = "github.com";

fn github_client_id(domain: &str) -> &'static str {
    if domain == DEFAULT_GITHUB_DOMAIN {
        GITHUB_CLIENT_ID
    } else {
        GITHUB_CLIENT_ID_GHES
    }
}
```

支持 github.com 和 GHES(GitHub Enterprise Server)两套 OAuth client,自动根据 `github_domain` 选 client_id + API endpoint。

**域名 SSOT 归一化**(`copilot_auth.rs:105-120`):

```rust
fn normalize_github_domain(raw: &str) -> Result<String, CopilotAuthError> {
    let s = raw.trim();
    let s = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
    let host = s.split(&['/', '?', '#'][..]).next().unwrap_or(s);
    if host.contains('@') {
        return Err(CopilotAuthError::InvalidDomain(raw.to_string()));
    }
    let normalized = host.to_lowercase();
    ...
}
```

**4 步 SSOT 归一化**:strip 协议 → strip path/query/fragment → 拒绝 userinfo → 小写化。

### 3.5 State 包装的精妙设计

**CodexOAuthState — `Arc<Manager>` 不包 `RwLock`**(`commands/codex_oauth.rs:19`):

```rust
pub struct CodexOAuthState(pub Arc<CodexOAuthManager>);
```

**注释解释**(`codex_oauth.rs:14-18`):

> "`CodexOAuthManager` 内部已使用细粒度锁且所有方法均为 `&self`,因此这里直接持有 `Arc`,不再包一层 `RwLock`——避免任一命令持有粗粒度锁跨网络刷新时阻塞其他命令(切换 / 认证中心操作 / token 读取)。"

**XaiOAuthState + CopilotAuthState — 包 `Arc<RwLock<Manager>>`**(`commands/xai_oauth.rs:13`, `commands/copilot.rs:14`):

```rust
pub struct XaiOAuthState(pub Arc<RwLock<XaiOAuthManager>>);
pub struct CopilotAuthState(pub Arc<RwLock<CopilotAuthManager>>);
```

**为什么 Codex 不包 RwLock 而其他两个包?** CodexOAuthManager 内部已用细粒度锁(per-account `Arc<Mutex<()>>`),方法签名全是 `&self`,不需要外层粗粒度锁。Xai 和 Copilot 可能有内部状态需要独占写(初始化/迁移),所以外层用 RwLock 隔离。

### 3.6 `lock_switch_for_app` — 跨命令的串行化保护(`commands/auth.rs:372-388`)

```rust
pub(crate) async fn remove_codex_oauth_account_with_switch_lock(
    app_state: &AppState,
    account_id: &str,
) -> Result<(), String> {
    // Serialize Auth Center credential deletion with managed provider
    // add/update/switch/hot-switch. Otherwise a switch that already preflighted
    // a bundle could recreate auth.json after removal.
    let _switch_guard = app_state
        .proxy_service
        .lock_switch_for_app(AppType::Codex.as_str())
        .await;
    app_state
        .codex_oauth_manager
        .remove_account(account_id)
        .await
        .map_err(|error| error.to_string())
}
```

**核心问题**: Auth Center 删除账号 与 managed provider 的 switch 操作并发时,**switch 已 preflight bundle 准备切换过去,然后被删除操作触发的还原逻辑覆盖回去**,导致切换看似成功实际没生效。

**解法**:`lock_switch_for_app` 是一把 per-app 的 mutex,删除账号前获取锁,确保期间没有 switch 操作并发执行。同样的逻辑用于 `logout_codex_oauth_with_switch_lock`(`auth.rs:447-459`)。

### 3.7 Provider 绑定 OAuth 账号

`resolve_copilot_base_url_override`(`commands/stream_check.rs:131-158`):

```rust
let account_id = provider
    .meta
    .as_ref()
    .and_then(|meta| meta.managed_account_id_for("github_copilot"));

let endpoint = match account_id.as_deref() {
    Some(id) => auth_manager.get_api_endpoint(id).await,
    None => auth_manager.get_default_api_endpoint().await,
};
```

**`Provider.meta.managed_account_id_for("github_copilot")`** 是 authBinding 机制:一个 Provider 可以绑定到具体某个 OAuth 账号(而非默认账号),这样多个 Provider 路由不同账号。

### 3.8 配额查询的统一抽象(`commands/codex_oauth.rs:27-67`)

```rust
#[tauri::command(rename_all = "camelCase")]
pub async fn get_codex_oauth_quota(
    account_id: Option<String>,
    state: State<'_, CodexOAuthState>,
) -> Result<SubscriptionQuota, String> {
    let manager = &state.0;
    // 解析最终使用的账号 ID:显式 > 默认账号 > 无账号 (not_found)
    let resolved = match account_id {
        Some(id) => Some(id),
        None => manager.default_account_id().await,
    };
    let Some(id) = resolved else {
        return Ok(SubscriptionQuota::not_found("codex_oauth"));
    };

    // 获取(必要时自动刷新)access_token
    let token = match manager.get_valid_token_for_account(&id).await {
        Ok(t) => t,
        Err(e) => return Ok(SubscriptionQuota::error(...)),
    };
    let chatgpt_account_id = manager
        .chatgpt_account_id_for_account(&id)
        .await
        .map_err(|e| e.to_string())?;

    // 瞬时传输失败以 Err 传播(前端 reject → retry + 保留上次成功值)
    query_codex_quota(
        &token,
        Some(&chatgpt_account_id),
        "codex_oauth",
        "Codex OAuth access token expired or rejected. Please re-login via cc-switch.",
    )
    .await
}
```

**4 步配额查询链**:
1. **解析账号**:`account_id` 参数 → 默认账号 → `not_found`(前端静默不渲染)
2. **拿有效 token**(必要时自动 refresh)
3. **拿 chatgpt_account_id**(用于上行 API 验证)
4. **调配额端点**

**xAI 配额查询 — 复用 Grok CLI 路径**(`commands/xai_oauth.rs:29-66`):

```rust
// 复用 `subscription_grok::query_grok_quota`,协议与 Grok CLI 路径完全一致。
crate::services::subscription_grok::query_grok_quota(
    &token,
    "xai_oauth",
    "Please re-login via cc-switch.",
)
.await
```

xAI 和 Grok CLI 用同一套 OAuth client(`b1a00492-073a-47ea-816f-4c329264a828`),所以 token 对 grok.com 账单端点等效——**复用同一查询实现**。

---

## 4. i18n 国际化

CC Switch 的 i18n 在**前端 + 后端**双层都做了国际化,4 种语言,带**智能语言探测链**。

### 4.1 前端 i18next 初始化(`src/i18n/index.ts`)

**4 个语言资源**(`src/i18n/index.ts:9-11, 64-77`):

```typescript
type Language = "zh" | "zh-TW" | "en" | "ja";

const DEFAULT_LANGUAGE: Language = "zh";

const resources = {
  en: { translation: en },
  ja: { translation: ja },
  zh: { translation: zh },
  "zh-TW": { translation: zhTW },
};
```

**核心设计 — 智能语言探测链**(`src/i18n/index.ts:13-62`):

```typescript
const getInitialLanguage = (): Language => {
  if (typeof window !== "undefined") {
    try {
      const stored = window.localStorage.getItem("language");
      if (stored === "zh" || stored === "zh-TW" || stored === "en" || stored === "ja") {
        return stored;
      }
    } catch (error) {
      console.warn("[i18n] Failed to read stored language preference", error);
    }
  }

  const navigatorLang = typeof navigator !== "undefined"
    ? (navigator.language?.toLowerCase() ?? navigator.languages?.[0]?.toLowerCase())
    : undefined;

  if (navigatorLang === "zh") return "zh";

  // 繁体分支细粒度匹配:zh-tw / zh-hk / zh-mo / zh-hant → zh-TW
  if (navigatorLang?.startsWith("zh-tw") ||
      navigatorLang?.startsWith("zh-hk") ||
      navigatorLang?.startsWith("zh-mo") ||
      navigatorLang?.startsWith("zh-hant")) {
    return "zh-TW";
  }

  if (navigatorLang?.startsWith("zh")) return "zh";
  if (navigatorLang?.startsWith("ja")) return "ja";
  if (navigatorLang?.startsWith("en")) return "en";

  return DEFAULT_LANGUAGE;
};
```

**3 层 fallback**:
1. `localStorage.language`(用户显式选择)
2. `navigator.language / navigator.languages[0]`(浏览器)
3. `DEFAULT_LANGUAGE = "zh"`(兜底)

**繁体分支细粒度**:不仅 `zh-tw`,还匹配 `zh-hk`(香港)、`zh-mo`(澳门)、`zh-hant`(传统汉字标识)。

**初始化**(`src/i18n/index.ts:79-90`):

```typescript
i18n.use(initReactI18next).init({
  resources,
  lng: getInitialLanguage(),
  fallbackLng: "en",  // 缺翻译时回退英文
  interpolation: { escapeValue: false },  // React 默认转义
  debug: false,
});
```

**fallbackLng = "en"** 而非 zh——保证缺翻译时**永远有可读字符串**(即使中文用户也不会看到空白)。

### 4.2 LanguageSettings 组件(`src/components/settings/LanguageSettings.tsx`)

**4 语言 Tab 切换器**(`LanguageSettings.tsx:23-39`):

```tsx
<div className="inline-flex gap-1 rounded-md border border-border-default bg-background p-1">
  <LanguageButton active={value === "zh"} onClick={() => onChange("zh")}>
    {t("settings.languageOptionChinese")}
  </LanguageButton>
  <LanguageButton active={value === "zh-TW"} onClick={() => onChange("zh-TW")}>
    {t("settings.languageOptionTraditionalChinese")}
  </LanguageButton>
  <LanguageButton active={value === "en"} onClick={() => onChange("en")}>
    {t("settings.languageOptionEnglish")}
  </LanguageButton>
  <LanguageButton active={value === "ja"} onClick={() => onChange("ja")}>
    {t("settings.languageOptionJapanese")}
  </LanguageButton>
</div>
```

切换后写 localStorage,触发 i18next 重新渲染。

### 4.3 i18n 容错模式 — `defaultValue`(`src/components/FrontendErrorBoundary.tsx:43-60`)

```tsx
<h1>{i18n.t("errors.frontendCrashTitle", { defaultValue: "界面遇到了问题" })}</h1>
<p>{i18n.t("errors.frontendCrashMessage", {
  defaultValue: "已尝试将错误信息写入应用诊断日志。请重新加载界面;...",
})}</p>
<Button className="w-full" onClick={() => window.location.reload()}>
  <RefreshCw className="mr-2 size-4" />
  {i18n.t("errors.reloadInterface", { defaultValue: "重新加载界面" })}
</Button>
```

**核心设计**:每个 `i18n.t()` 都带 `defaultValue` —— **即使翻译键缺失,也显示中文 fallback,而不是空白**。这在翻译键漏译、首次部署等场景下提供视觉连续性。

### 4.4 后端双语句错误(`src-tauri/src/error.rs:52-57, 90-96`)

```rust
#[error("{zh} ({en})")]
Localized {
    key: &'static str,
    zh: String,
    en: String,
},
```

**Display 实现** = `"{zh} ({en})"`,**同一错误同时打印中英文**——`sync_protocol.rs::localized` 工厂函数(`sync_protocol.rs:82-88`)集中生成:

```rust
pub(crate) fn localized(
    key: &'static str,
    zh: impl Into<String>,
    en: impl Into<String>,
) -> AppError {
    AppError::localized(key, zh, en)
}
```

调用点:`localized("sync.manifest_format_incompatible", "远端 manifest 格式不兼容: {}", "Remote manifest format is incompatible: {}", ...)`,**一句同时传双语**。

**io_context_localized**(`sync_protocol.rs:90-102`)专用于 IO 错误的双语化,自动把原始 IO error 接进 `IoContext::source`。

### 4.5 对话框双语句(`lib.rs:2074-2196`)

`is_chinese_locale()`(`lib.rs:2074-2080`):

```rust
fn is_chinese_locale() -> bool {
    std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .map(|lang| lang.starts_with("zh"))
        .unwrap_or(false)
}
```

**3 环境变量级联**:`LANG` → `LC_ALL` → `LC_MESSAGES`,任一以 `zh` 开头视为中文环境。

`show_database_init_error_dialog`(`lib.rs:2135-2196`)根据 `is_chinese_locale()` 输出**完全独立的两套对话语文本**(不只是翻译键),包括标题、消息体、按钮文本。这种**对话框级别双语**避免了 Tauri 对话框不支持 i18next 翻译的问题。

### 4.6 多语言文档(i18n 基础设施)

`docs/user-manual/{en,zh,ja}/` 是用户手册,**4 语言 × 5 章节 × 多节** + `docs/release-notes/v3.X.X-{en,ja,zh}.md` 是 release notes 双语化:

```text
docs/
├── user-manual/
│   ├── en/  1-getting-started, 2-providers, 3-extensions, 4-proxy, 5-faq
│   ├── zh/  (相同 5 章节)
│   └── ja/  (相同 5 章节)
├── release-notes/
│   └── v3.X.X-{en,ja,zh}.md × 30+ 个版本
└── guides/  路由指南 4 种语言 × 6 篇
```

**资源**:`README.md`(英文)+ `README_ZH.md` + `README_JA.md` + `README_DE.md` 4 个版本根 README。**`CHANGELOG.md` 451 KB**(单文件长篇 changelog)。

---

## 5. Release 工程化与 AutoUpdate

CC Switch 的 release 工程是教科书级的 **5 平台 × 多产物 × 自动签名 × 自动公证** 流水线。

### 5.1 Tauri Updater 配置(`src-tauri/tauri.conf.json:62-69`)

```json
"updater": {
  "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEM4MDI4QzlBNTczOTI4RTMKUldUaktEbFhtb3dDeUM5US9kT0FmdGR5Ti9vQzcwa2dTMlpibDVDUmQ2M0VGTzVOWnd0SGpFVlEK",
  "endpoints": [
    "https://dl.ccswitch.io/latest.json",
    "https://github.com/farion1231/cc-switch/releases/latest/download/latest.json"
  ]
}
```

**双 endpoint fallback**:
- `dl.ccswitch.io/latest.json` —— 自有 CDN 优先
- `GitHub Releases latest.json` —— 兜底

**pubkey 是 minisign 格式**,embedded 在二进制中,客户端用它验证每次下载的 `.sig` 签名。

### 5.2 bundle 配置(`tauri.conf.json:36-55`)

```json
"bundle": {
  "active": true,
  "targets": "all",
  "createUpdaterArtifacts": true,    // 生成 updater artifact(.tar.gz + .sig)
  "icon": ["icons/32x32.png", "icons/128x128.png", "icons/128x128@2x.png", "icons/icon.icns", "icons/icon.ico"],
  "windows": {
    "wix": {
      "template": "wix/per-user-main.wxs"
    }
  },
  "macOS": {
    "minimumSystemVersion": "12.0"
  }
}
```

**`createUpdaterArtifacts: true`** 触发 Tauri 自动生成 `.tar.gz`(macOS)+ `.sig`(minisign 签名)。

### 5.3 前端 Updater 抽象层(`src/lib/updater.ts`)

```typescript
import { getVersion } from "@tauri-apps/api/app";

export type UpdateChannel = "stable" | "beta";

export interface UpdateInfo {
  currentVersion: string;
  availableVersion: string;
  notes?: string;
  pubDate?: string;
}

export interface CheckOptions {
  timeout?: number;
  channel?: UpdateChannel;
}

export async function getCurrentVersion(): Promise<string> {
  try {
    return await getVersion();
  } catch {
    return "";
  }
}

export async function checkForUpdate(opts: CheckOptions = {}): Promise<
  { status: "up-to-date" } | { status: "available"; info: UpdateInfo }
> {
  // 动态引入,避免在未安装插件时导致打包期问题
  const { check } = await import("@tauri-apps/plugin-updater");

  const currentVersion = await getCurrentVersion();
  const update = await check({ timeout: opts.timeout ?? 30000 } as any);

  if (!update) return { status: "up-to-date" };

  const info: UpdateInfo = {
    currentVersion,
    availableVersion: (update as any).version ?? "",
    notes: (update as any).notes,
    pubDate: (update as any).date,
  };

  return { status: "available", info };
}
```

**3 个设计点**:
1. **动态 import `@tauri-apps/plugin-updater`** —— 避免在未装插件时打包期报错
2. **`timeout: 30000`** —— 30 秒超时,避免网络卡住
3. **union return type** —— 强制调用方区分 `up-to-date` 与 `available`

### 5.4 UpdateContext(`src/contexts/UpdateContext.tsx`)

**核心设计 — dismissed 持久化 + 旧键迁移**(`UpdateContext.tsx:31-57`):

```typescript
const DISMISSED_VERSION_KEY = "ccswitch:update:dismissedVersion";
const LEGACY_DISMISSED_KEY = "dismissedUpdateVersion"; // 兼容旧键

useEffect(() => {
  const current = updateInfo?.availableVersion;
  if (!current) return;

  // 读取新键;若不存在,尝试迁移旧键
  let dismissedVersion = localStorage.getItem(DISMISSED_VERSION_KEY);
  if (!dismissedVersion) {
    const legacy = localStorage.getItem(LEGACY_DISMISSED_KEY);
    if (legacy) {
      localStorage.setItem(DISMISSED_VERSION_KEY, legacy);
      localStorage.removeItem(LEGACY_DISMISSED_KEY);
      dismissedVersion = legacy;
    }
  }

  setIsDismissed(dismissedVersion === current);
}, [updateInfo?.availableVersion]);
```

**3 个设计点**:
1. **新键空间命名空间化**(`ccswitch:update:dismissedVersion`)—— 避免与其它应用冲突
2. **旧键迁移**:升级时一次性把 `dismissedUpdateVersion` 迁到新键
3. **dismissed 与版本绑定** —— 用户关闭 v3.18 的提醒,v3.19 发布后提醒仍能弹出

**isCheckingRef 防止重入**(`UpdateContext.tsx:59-101`):

```typescript
const isCheckingRef = useRef(false);

const checkUpdate = useCallback(async () => {
  if (isCheckingRef.current) return false;
  isCheckingRef.current = true;
  setIsChecking(true);
  setError(null);
  try {
    const result = await checkForUpdate({ timeout: 30000 });
    ...
  } finally {
    setIsChecking(false);
    isCheckingRef.current = false;
  }
}, []);
```

**Ref 而非 state 防重入**:state 触发 re-render,在 `setIsChecking(true)` 后第二次调用还能读到旧 state;ref 同步读到最新值。

**启动 1 秒延迟检查**(`UpdateContext.tsx:119-126`):

```typescript
useEffect(() => {
  // 延迟1秒后检查,避免影响启动体验
  const timer = setTimeout(() => {
    checkUpdate().catch(console.error);
  }, 1000);

  return () => clearTimeout(timer);
}, [checkUpdate]);
```

启动期不抢资源。

### 5.5 UpdateBadge(`src/components/UpdateBadge.tsx`)

```tsx
export function UpdateBadge({ className = "", onClick }: UpdateBadgeProps) {
  const { hasUpdate, updateInfo } = useUpdate();
  const { t } = useTranslation();
  const isActive = hasUpdate && updateInfo;
  const title = isActive
    ? t("settings.updateAvailable", { version: updateInfo?.availableVersion ?? "" })
    : t("settings.checkForUpdates");

  if (!isActive) return null;

  return (
    <Button type="button" variant="ghost" size="icon" title={title} aria-label={title}
      onClick={onClick}
      className={`relative h-8 w-8 rounded-full
        ${isActive ? "text-green-600 dark:text-green-400 hover:bg-green-50 dark:hover:bg-green-500/10" : "text-muted-foreground hover:bg-muted/60"}
        ${className}`}>
      <ArrowUpCircle className="h-5 w-5" />
    </Button>
  );
}
```

无更新时不渲染,有更新时显示绿色 ↑ 图标,点击触发更新流程。

### 5.6 Release Pipeline — `.github/workflows/release.yml`

**5 平台 matrix**(`release.yml:23-30`):

```yaml
strategy:
  fail-fast: false
  matrix:
    include:
      - os: windows-2022
      - os: windows-11-arm
        arch: arm64
      - os: ubuntu-22.04
      - os: ubuntu-22.04-arm
        arch: arm64
      - os: macos-14
```

**5 个独立构建机**:Windows x64 + ARM64, Linux x64 + ARM64, macOS universal。

**签名密钥处理**(`release.yml:125-179`)是教科书级,支持 3 种密钥格式:
1. **原始两行文本**(第一行 `untrusted comment:` 开头)
2. **base64 包裹的两行文件**
3. **单行 base64**(自动构造两行)

```bash
if echo "$RAW" | head -n1 | grep -q '^untrusted comment:'; then
  printf '%s\n' "$RAW" > "$KEY_PATH"
  echo "✅ 使用原始两行密钥文件格式"
else
  if DECODED=$(printf '%s' "$RAW" | (base64 --decode 2>/dev/null || base64 -D 2>/dev/null)) \
     && echo "$DECODED" | head -n1 | grep -q '^untrusted comment:'; then
    printf '%s\n' "$DECODED" > "$KEY_PATH"
    echo "✅ 成功解码 base64 包裹密钥,已还原为两行文件"
  else
    if echo "$RAW" | grep -Eq '^[A-Za-z0-9+/=]+$'; then
      ONE=$(printf '%s' "$RAW" | tr -d '\r\n')
      printf '%s\n%s\n' "untrusted comment: tauri signing key" "$ONE" > "$KEY_PATH"
      echo "✅ 使用一行 Base64 私钥,已构造两行文件"
    else
      echo "❌ TAURI_SIGNING_PRIVATE_KEY 格式无法识别" >&2
      exit 1
    fi
  fi
fi
```

**Apple 公证**(`release.yml:181-322`)—— 完整流程:
1. 解码 `.p12` 证书
2. 创建临时 keychain(21600 秒超时)
3. 导入证书 + codesign/security 权限
4. 动态解析 `Developer ID Application` 身份
5. `pnpm tauri build --target universal-apple-darwin`(最多 3 次重试)
6. `xcrun stapler staple`(贴发票)
7. **create-dmg 创建样式化 DMG**(因为 Tauri 默认 DMG 样式不工作)
8. `xcrun notarytool submit + wait`(提交公证)
9. **codesign / spctl / stapler validate 三重验证**

**产物收集**:
- macOS: `CC-Switch-{ver}-macOS.tar.gz` + `.zip` + `.dmg` + `.sig`
- Windows: `CC-Switch-{ver}-Windows{arm64,}.msi` + `portable.zip` + `.sig`
- Linux: `CC-Switch-{ver}-Linux-{x86_64,arm64}.AppImage` + `.deb` + `.rpm`

### 5.7 `latest.json` 生成与 R2 重写

**assemble-latest-json job**(`release.yml:628-732`):

```bash
for sig in dl/*.sig; do
  base=${sig%.sig}
  fname=$(basename "$base")
  url="$base_url/$fname"
  sig_content=$(cat "$sig")
  case "$fname" in
    *.tar.gz)               mac_url="$url"; mac_sig="$sig_content";;
    *-Windows-arm64.msi)    win_arm64_url="$url"; win_arm64_sig="$sig_content";;
    *-Windows.msi)          win_x64_url="$url"; win_x64_sig="$sig_content";;
    *-Linux-arm64.AppImage) linux_arm64_url="$url"; linux_arm64_sig="$sig_content";;
    *-Linux-x86_64.AppImage) linux_x64_url="$url"; linux_x64_sig="$sig_content";;
  esac
done
```

**5 平台 × (url + signature)** 的 latest.json,既支持 GitHub Releases,也支持 R2 mirror。

**`scripts/rewrite-updater-manifest.mjs`** —— **R2 mirror 关键工具**(`scripts/rewrite-updater-manifest.mjs:1-46`):

```javascript
#!/usr/bin/env node
// Rewrites the Tauri updater manifest (latest.json) so its download URLs point
// at the R2 mirror instead of GitHub Releases. Minisign signatures cover file
// contents, not URLs, so they stay valid unchanged — the updater still verifies
// every downloaded artifact against the pubkey baked into the app.
//
// Usage: node scripts/rewrite-updater-manifest.mjs <latest-json> <tag> <base-url> [output]

import { readFileSync, writeFileSync } from 'node:fs';

const [input, tag, baseUrl, output = 'latest-r2.json'] = process.argv.slice(2);

const manifest = JSON.parse(readFileSync(input, 'utf8'));
const platforms = Object.entries(manifest.platforms ?? {});
...
const normalizedBase = baseUrl.replace(/\/+$/, '');
const githubPrefix = `https://github.com/`;

for (const [key, entry] of platforms) {
  if (typeof entry?.url !== 'string' || typeof entry?.signature !== 'string') {
    console.error(`Platform ${key} is missing url or signature`);
    process.exit(1);
  }
  if (!entry.url.startsWith(githubPrefix) || !entry.url.includes(`/releases/download/${tag}/`)) {
    console.error(`Platform ${key} has unexpected url for ${tag}: ${entry.url}`);
    process.exit(1);
  }
  const name = entry.url.split('/').pop();
  entry.url = `${normalizedBase}/${tag}/${name}`;
}

writeFileSync(output, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`Wrote ${output} with ${platforms.length} platforms pointing at ${normalizedBase}/${tag}/`);
```

**核心洞见**:minisign 签名覆盖**文件内容**而非 URL,所以 R2 mirror 复用同一签名。脚本只改 URL,**不改 .sig 内容**。校验流程:
1. **白名单前缀检测**:URL 必须以 `https://github.com/` 开头 + 含 `/releases/download/{tag}/`
2. **从 URL 提取文件名**:`url.split('/').pop()`
3. **重新拼接为 R2 路径**:`${baseUrl}/${tag}/${name}`

### 5.8 `sync-r2.yml` — R2 自动镜像

`.github/workflows/sync-r2.yml` 在每次 release 后自动同步 artifact 到 R2,然后调 `rewrite-updater-manifest.mjs` 生成 `latest-r2.json`。

### 5.9 Portable 模式(`release.yml:469-505`)

```powershell
$portableDir = 'release-assets/CC-Switch-Portable'
New-Item -ItemType Directory -Force -Path $portableDir | Out-Null
Copy-Item $exePath $portableDir
$portableIniPath = Join-Path $portableDir 'portable.ini'
$portableContent = if ($isArm64) {
  @('# CC Switch portable ARM64 build marker', 'portable=true', 'arch=arm64')
} else {
  @('# CC Switch portable build marker', 'portable=true')
}
$portableContent | Set-Content -Path $portableIniPath -Encoding UTF8
$portableZip = "release-assets/CC-Switch-$VERSION-Windows$assetSuffix-Portable.zip"
Compress-Archive -Path "$portableDir/*" -DestinationPath $portableZip -Force
```

`portable.ini` 含 `portable=true`,应用启动时检测此标记切换到 portable 模式(数据写入 exe 同目录而非 `%APPDATA%`)。

### 5.10 CI 检查(`.github/workflows/ci.yml`)

**3 个独立 job**:
- **changes**:用 `dorny/paths-filter` 检测改动区域(frontend/backend),**只对改动区域跑测试**(节省 CI 时间)
- **frontend**:`pnpm typecheck` + `pnpm format:check` + `pnpm test:unit`
- **backend** × 3 OS:`cargo fmt --check` + `cargo clippy -D warnings` + `cargo test`

**backend-windows-wsl2**(`ci.yml:155-228`)**专门测试 Windows ↔ WSL2 文件系统契约**:

```yaml
- name: Compile backend tests with native temp
  # link.exe/mt.exe cannot create manifests when TEMP points at a WSL UNC path.
  run: cargo test --lib --manifest-path src-tauri/Cargo.toml --no-run

- name: Run Windows-to-WSL2 filesystem contract
  shell: pwsh
  run: |
    $env:TEMP = $env:CC_SWITCH_WSL_TEST_TEMP
    $env:TMP = $env:CC_SWITCH_WSL_TEST_TEMP
    $testName = "config::tests::atomic_write_replaces_existing_wsl_unc_file"
    ...
    cargo test --lib --manifest-path src-tauri/Cargo.toml $testName -- --ignored --exact --nocapture
```

**核心洞见**:`link.exe / mt.exe` 在 TEMP 指向 WSL UNC 路径(`\\wsl.localhost\...`)时无法生成 manifest。所以**编译阶段**用 native TEMP,**测试阶段**用 WSL2 TEMP 触发 `atomic_write_replaces_existing_wsl_unc_file` 单元测试 —— **专门覆盖原子写文件覆盖 WSL UNC 路径的场景**。

### 5.11 WSL2 Nightly(.github/workflows/wsl2-nightly.yml)

单独的 nightly workflow,定期在 WSL2 环境下跑完整构建,确保 Linux 兼容性不退化。

---

## 6. WebSocket 与 SSE

CC Switch 不直接实现 WebSocket 服务端(它代理的是 HTTP/SSE 流量),但 SSE 处理是**代理层最复杂的部分**。

### 6.1 手动 hyper accept loop + Header case preservation(`src-tauri/src/proxy/server.rs:138-213`)

```rust
tokio::spawn(async move {
    // Peek raw TCP bytes to capture original header casing
    // before hyper parses (and lowercases) the header names.
    let original_cases = {
        let mut peek_buf = vec![0u8; 8192];
        match stream.peek(&mut peek_buf).await {
            Ok(n) => {
                let cases = super::hyper_client::OriginalHeaderCases::from_raw_bytes(&peek_buf[..n]);
                log::debug!(
                    "[ProxyServer] Peeked {} bytes, captured {} header casings",
                    n, cases.cases.len
                );
                cases
            }
            Err(e) => {
                log::debug!("[ProxyServer] peek failed (non-fatal): {e}");
                super::hyper_client::OriginalHeaderCases::default()
            }
        }
    };

    // service_fn 将 axum Router(tower::Service)桥接到 hyper
    let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let mut router = app.clone();
        let cases = original_cases.clone();
        async move {
            let (mut parts, body) = req.into_parts();
            parts.extensions.insert(cases);  // 把原始 header 大小写插入 extensions
            let body = axum::body::Body::new(body);
            let axum_req = http::Request::from_parts(parts, body);
            <Router as tower::Service<http::Request<axum::body::Body>>>::call(&mut router, axum_req).await
        }
    });

    if let Err(e) = hyper::server::conn::http1::Builder::new()
        .preserve_header_case(true)   // 让 hyper 不强制小写 header
        .serve_connection(TokioIo::new(stream), service)
        .await
    {
        log::debug!("[{}] connection error: {e}", log_srv::CONN_ERR);
    }
});
```

**核心设计 — `preserve_header_case(true)`**:hyper 默认小写化所有 header name,但某些上游 gateway 鉴权对 header 大小写敏感(如 `X-API-Key` vs `x-api-key`)。`preserve_header_case` + peek 抓取 + 存进 extensions 三步组合,让客户端请求的 wire-level header 大小写被原样转发到上游。

### 6.2 SSE 块解析(`src-tauri/src/proxy/sse.rs`)

`take_sse_block`(`sse.rs:8-23`):

```rust
pub(crate) fn take_sse_block(buffer: &mut String) -> Option<String> {
    let mut best: Option<(usize, usize)> = None;

    for (delimiter, len) in [("\r\n\r\n", 4usize), ("\n\n", 2usize)] {
        if let Some(pos) = buffer.find(delimiter) {
            if best.is_none_or(|(best_pos, _)| pos < best_pos) {
                best = Some((pos, len));
            }
        }
    }

    let (pos, len) = best?;
    let block = buffer[..pos].to_string();
    buffer.drain(..pos + len);
    Some(block)
}
```

**双 delimiter 兼容**:`\r\n\r\n`(CRLF)和 `\n\n`(LF)都识别,选**最早出现**的(最小 pos),**严格按 SSE spec 兼容两者**。

`strip_sse_field`(`sse.rs:2-5`):

```rust
pub(crate) fn strip_sse_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    line.strip_prefix(&format!("{field}: "))
        .or_else(|| line.strip_prefix(&format!("{field}:")))
}
```

**兼容带空格和不带空格**:SSE 规范要求 `data: ` 带空格,但 OpenAI 等实现省略空格——两种都接受。

### 6.3 UTF-8 跨块边界处理(`sse.rs:36-86`)

```rust
pub(crate) fn append_utf8_safe(buffer: &mut String, remainder: &mut Vec<u8>, new_bytes: &[u8]) {
    let (owned, bytes): (Option<Vec<u8>>, &[u8]) = if remainder.is_empty() {
        (None, new_bytes)
    } else {
        if remainder.len() > 3 {
            buffer.push_str(&String::from_utf8_lossy(remainder));
            remainder.clear();
            (None, new_bytes)
        } else {
            let mut combined = std::mem::take(remainder);
            combined.extend_from_slice(new_bytes);
            (Some(combined), &[])
        }
    };
    let input = owned.as_deref().unwrap_or(bytes);

    let mut pos = 0;
    loop {
        match std::str::from_utf8(&input[pos..]) {
            Ok(s) => { buffer.push_str(s); return; }
            Err(e) => {
                let valid_up_to = pos + e.valid_up_to();
                ...
                if let Some(invalid_len) = e.error_len() {
                    buffer.push('\u{FFFD}');
                    pos = valid_up_to + invalid_len;
                } else {
                    // Incomplete trailing sequence – stash for next chunk.
                    *remainder = input[valid_up_to..].to_vec();
                    return;
                }
            }
        }
    }
}
```

**核心设计**:
1. **保留余数 `remainder`** —— 跨块的多字节字符切片暂存,等下次 chunk 拼接
2. **3 字节防御**(`remainder.len() > 3` 时强制 flush loss)—— UTF-8 4 字节字符缺最后 1 字节,最长 3 字节残缺,超长说明流损坏
3. **错误字节立即 U+FFFD** —— 不堆积在 remainder
4. **测试覆盖**(`sse.rs:88-345`):9 个完整测试,覆盖 ASCII/中文/emoji/多分块/混合

### 6.4 流式响应处理(`src-tauri/src/proxy/response_processor.rs:151-210`)

```rust
pub async fn handle_streaming(
    response: ProxyResponse,
    ctx: &RequestContext,
    state: &ProxyState,
    parser_config: &UsageParserConfig,
    connection_guard: Option<ActiveConnectionGuard>,
) -> Response {
    let status = response.status();
    // 检查流式响应是否被压缩(SSE 通常不压缩,如果压缩则 SSE 解析会失败)
    if let Some(encoding) = get_content_encoding(response.headers()) {
        log::warn!(
            "[{}] 流式响应含 content-encoding={encoding},SSE 解析可能失败。\
             上游在 accept-encoding 透传后压缩了 SSE 流。",
            ctx.tag
        );
    }

    let mut response_headers = response.headers().clone();
    strip_hop_by_hop_response_headers(&mut response_headers);
    let mut builder = axum::response::Response::builder().status(status);
    for (key, value) in &response_headers {
        builder = builder.header(key, value);
    }

    let stream = response.bytes_stream();
    let usage_collector = create_usage_collector(ctx, state, status.as_u16(), parser_config);
    let timeout_config = ctx.streaming_timeout_config();

    let logged_stream = create_logged_passthrough_stream(
        stream, ctx.tag, usage_collector, timeout_config, connection_guard,
    );
    let body = axum::body::Body::from_stream(logged_stream);
    match builder.body(body) {
        Ok(resp) => resp,
        Err(e) => ProxyError::Internal(format!("Failed to build streaming response: {e}")).into_response(),
    }
}
```

**6 步流式处理**:
1. **Content-Encoding 警告** —— 上游如果压缩 SSE 流会导致 SSE 解析失败
2. **剥 hop-by-hop header**(`response_processor.rs:36-68`)
3. **透传 status + headers** 到客户端
4. **创建使用量收集器**(`create_usage_collector`,只在 usage logging 启用时解析 SSE event)
5. **带超时 + 日志的透传流**(`create_logged_passthrough_stream`)
6. **构造成 axum Response body**

### 6.5 Hop-by-Hop Header 剥离(`response_processor.rs:36-68`)

```rust
const HOP_BY_HOP_RESPONSE_HEADERS: &[&str] = &[
    "connection", "keep-alive", "proxy-authenticate", "proxy-authorization",
    "proxy-connection", "te", "trailer", "trailers", "transfer-encoding", "upgrade",
];

pub(crate) fn strip_hop_by_hop_response_headers(headers: &mut HeaderMap) {
    let connection_listed_headers: Vec<HeaderName> = headers
        .get_all(axum::http::header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter_map(|name| HeaderName::from_bytes(name.as_bytes()).ok())
        .collect();

    for name in HOP_BY_HOP_RESPONSE_HEADERS {
        headers.remove(*name);
    }
    for name in connection_listed_headers {
        headers.remove(name);
    }
}
```

**符合 RFC 7230 §6.1**:硬编码 hop-by-hop header + 解析 `Connection:` 头里点名的额外 hop-by-hop header。

### 6.6 非流式 body 超时(`response_processor.rs:82-138`)

```rust
pub(crate) async fn read_decoded_body(
    response: ProxyResponse,
    tag: &str,
    body_timeout: Duration,
) -> Result<(HeaderMap, http::StatusCode, Bytes), ProxyError> {
    let mut headers = response.headers().clone();
    let status = response.status();
    let bytes_future = response.bytes_with_limit(MAX_RESPONSE_BODY_BYTES);
    let raw_bytes = if body_timeout.is_zero() {
        bytes_future.await?
    } else {
        tokio::time::timeout(body_timeout, bytes_future).await
            .map_err(|_| ProxyError::Timeout(format!(
                "响应体读取超时: {}s(上游发完响应头后 body 未到达)",
                body_timeout.as_secs()
            )))?
    };
    ...
}
```

**双超时独立计时**:`first_byte_timeout`(等待首个响应字节)+ `idle_timeout`(流式两个事件间隔),通过 `tokio::select!` 实现。

### 6.7 Stream Check(连通性探活)`commands/stream_check.rs` + `services/stream_check.rs`

**设计哲学**(`services/stream_check.rs:5-18`):

> "仅探测供应商 `base_url` 是否可达,**不发送真实大模型请求**:
> - 收到任意 HTTP 响应(200/4xx/5xx)即判定"可达"(端口通、网关存活);
> - 仅 DNS / 连接被拒 / TLS / 超时等网络级错误判定"不可达";
> - 延迟 = 收到响应头的耗时(TTFB,真实往返)。"

**核心不变量**(`services/stream_check.rs:13-18`):

> "连通性检查 **绝不** 触碰故障转移熔断器:一个返回 403/401 的供应商在本检查里算"可达",但它对真实流量是坏的。熔断器只由 `proxy/forwarder.rs` 转发真实流量的成败驱动(被动)。两者职责分离——可达性回答"能不能到",真实流量回答"能不能用"。"

**3 档健康状态**(`services/stream_check.rs:30-36`):

```rust
pub enum HealthStatus {
    Operational,  // 成功
    Degraded,     // 可达但 TTFB > degraded_threshold_ms(默认 6000ms)
    Failed,       // 网络级错误
}
```

**配置默认值**(`services/stream_check.rs:51-61`):

```rust
impl Default for StreamCheckConfig {
    fn default() -> Self {
        // 可达性探测打的是 base_url 的小请求(仅读响应头),不等待模型生成,故超时远小于
        // 旧的真实请求检查(45s → 8s);降级阈值沿用旧尺度 6000ms——探测 TTFB 一般远低于
        // 此,仅在确实很慢时才标"较慢",避免把 1 秒多的正常延迟误判为降级。
        Self {
            timeout_secs: 8,
            max_retries: 1,
            degraded_threshold_ms: 6000,
        }
    }
}
```

**仅超时类重试**(`services/stream_check.rs:108-117`):

```rust
if Self::should_retry(&result.message) && attempt < config.max_retries {
    last_result = Some(result);
    continue;
}
return Ok(StreamCheckResult { retry_count: attempt, ..result });
```

注释:`仅超时 / abort 类网络抖动值得重试;连接被拒、DNS 失败等立即返回。`

### 6.8 Speedtest Service(`services/speedtest.rs`)

独立的 `SpeedtestService`,用真实 LLM 请求测端到端延迟(与 stream_check 互补,后者只测 base_url)。

---

## 7. DevContainer 与容器化

CC Switch 在 **Linux 兼容性 + Flatpak 打包 + WSL2 测试** 三方面做了容器化/分发相关工作。

### 7.1 Linux 焦点恢复补丁(`src-tauri/src/linux_fix.rs`)

这是**教科书级的 Linux GUI 兼容性 workaround**:

```rust
//! Linux 专用的主窗口恢复补丁。
//!
//! 解决 Tauri 2.x 在部分 Linux 发行版(尤其是 Wayland / 某些 WebKitGTK
//! 版本)上启动后 UI 无法响应点击的问题:
//!
//! - **失效模式 A**(Tauri #10746 / wry #637):webview 在 `show()` 后
//!   没有获得 keyboard focus,导致首次点击被 X11/Wayland 用作
//!   click-to-activate 而非传给 webview。
//! - **失效模式 B**:GTK surface 与 WebKitWebView 的 input region 尺寸
//!   协商在 `visible:false` → `show()` 的路径上失败,整窗永远不响应
//!   点击,只有重新 `size_allocate`(例如最大化-还原)才能恢复。

use std::time::Duration;
use tauri::{PhysicalSize, WebviewWindow};

const REALIZE_WAIT: Duration = Duration::from_millis(200);
const RESIZE_GAP: Duration = Duration::from_millis(100);
const RECONCILE_WAIT: Duration = Duration::from_millis(500);

pub(crate) fn nudge_main_window(window: WebviewWindow) {
    let _ = window.set_focus();

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(REALIZE_WAIT).await;
        let _ = window.set_focus();

        // 伪 resize:读取当前 inner_size,先加 1px 再还原。
        match window.inner_size() {
            Ok(original) => {
                let bumped = PhysicalSize::new(original.width.saturating_add(1), original.height);
                let _ = window.set_size(bumped);
                tokio::time::sleep(RESIZE_GAP).await;
                let _ = window.set_size(original);
                log::info!("Linux: 已对主窗口执行 focus + surface 重激活");

                // 尺寸对账回读
                tokio::time::sleep(RECONCILE_WAIT).await;
                match window.inner_size() {
                    Ok(after) => {
                        if after.width != original.width || after.height != original.height {
                            log::info!(
                                "Linux nudge 尺寸 drift: expected={}x{}, got={}x{},已补偿",
                                original.width, original.height, after.width, after.height
                            );
                            let _ = window.set_size(original);
                            ...
                        }
                    }
                    ...
                }
            }
            ...
        }
    });
}
```

**3 时序常量**(注释解释为何这样定):
- `REALIZE_WAIT = 200ms` —— 等 GTK 主循环处理 realize 事件(社区经验值,太短 set_focus 无效)
- `RESIZE_GAP = 100ms` —— 两次 set_size 间,确保合成器不 coalesce 成一次
- `RECONCILE_WAIT = 500ms` —— 总 ~800ms 后回读尺寸

**4 阶段序列**:
1. **同步 `set_focus`** —— 立即尝试,通常无效(webview 还没 realize)
2. **200ms 后再 `set_focus`** —— 解决失效模式 A
3. **±1px 伪 resize** —— 触发 GTK `size-allocate` → WebKitWebViewBase::size_allocate → 重新 attach input surface,解决失效模式 B
4. **700ms 后尺寸对账回读** —— Tao Linux 的 set_size 是异步的,合成器可能 coalesce,补偿 drift

**已知限制**(`linux_fix.rs:75-79`):tiling Wayland 合成器(sway/river/hyprland)会完全忽略 `set_size`,此时对账永远 drift=0 但失效模式 B 实际没修复,需要用户用 `GDK_BACKEND=x11` 绕过。

### 7.2 Linux WebKit 兜底(`src-tauri/src/main.rs`)

```rust
#[cfg(target_os = "linux")]
{
    // WebKitGTK DMA-BUF 渲染器在 Nvidia + Debian 13.2 触发白屏/黑屏
    if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    // 禁用 WebKitGTK 合成模式规避 resize 时 webview 崩溃
    if std::env::var("WEBKIT_DISABLE_COMPOSITING_MODE").is_err() {
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }
    // AppImage GTK 启动钩子强制 XWayland,提供逃生开关让用户改回 Wayland
    if let Ok(backend) = std::env::var("CC_SWITCH_GDK_BACKEND") {
        if !backend.is_empty() {
            std::env::set_var("GDK_BACKEND", backend);
        }
    }
}
```

**3 个环境变量兜底**:
- `WEBKIT_DISABLE_DMABUF_RENDERER=1` —— Nvidia + Debian 13.2 GPU 白屏
- `WEBKIT_DISABLE_COMPOSITING_MODE=1` —— resize 时 webview 崩溃
- `CC_SWITCH_GDK_BACKEND` —— 用户可覆盖 XWayland vs Wayland

### 7.3 禁用 WebKitGTK 硬件加速(`lib.rs:1310-1323`)

```rust
#[cfg(target_os = "linux")]
{
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.with_webview(|webview| {
            use webkit2gtk::{WebViewExt, SettingsExt, HardwareAccelerationPolicy};
            let wk_webview = webview.inner();
            if let Some(settings) = WebViewExt::settings(&wk_webview) {
                SettingsExt::set_hardware_acceleration_policy(&settings, HardwareAccelerationPolicy::Never);
                log::info!("已禁用 WebKitGTK 硬件加速");
            }
        });
    }
}
```

EGL 初始化失败 → 强制软件渲染。

### 7.4 Flatpak 打包(`flatpak/com.ccswitch.desktop.yml`)

**GNOME 46 platform + 完整权限**(`flatpak/com.ccswitch.desktop.yml:1-22`):

```yaml
id: com.ccswitch.desktop

runtime: org.gnome.Platform
runtime-version: '46'
sdk: org.gnome.Sdk

command: cc-switch

finish-args:
  - --share=ipc
  - --share=network
  - --socket=wayland
  - --socket=fallback-x11
  - --device=dri
  # Tray icon permissions (required by Tauri tray-icon)
  - --talk-name=org.kde.StatusNotifierWatcher
  - --filesystem=xdg-run/tray-icon:create
  # GitHub Releases scenario: Users download and install manually.
  # For "download and run" convenience (needs read/write access to ~/.cc-switch, ~/.claude, ~/.claude.json,
  # ~/.codex, ~/.gemini, and supports custom directory overrides), we grant full Home access by default.
  # If you plan to publish on Flathub or prefer minimal permissions, replace this with more precise directory
  # grants (see flatpak/README.md).
  - --filesystem=home
```

**注释明确指出**:
- `--filesystem=home` 是为"下载即用"便利性而设的宽权限
- 如果发布到 Flathub 或偏好最小权限,**改为精确目录权限**

**5 个额外 module 构建**(Tray icon 依赖 libdbusmenu + libayatana 系列):

```yaml
modules:
  - name: intltool                # libdbusmenu 需要
  - name: libayatana-ido
  - name: libdbusmenu-gtk3
  - name: libayatana-indicator
  - name: libayatana-appindicator
  - name: cc-switch               # 用 ar + tar 解 .deb 包,提取到 /app
```

**打包流程**(`flatpak/com.ccswitch.desktop.yml:90-98`):

```yaml
build-commands:
  - ar -x *.deb
  - tar -xf data.tar.*
  - cp -a usr/* /app/
  # Use our own desktop/metainfo/icon to align with Flatpak app id
  - rm -f /app/share/applications/*.desktop
  - install -Dm644 com.ccswitch.desktop.desktop /app/share/applications/com.ccswitch.desktop.desktop
  - install -Dm644 com.ccswitch.desktop.metainfo.xml /app/share/metainfo/com.ccswitch.desktop.metainfo.xml
  - install -Dm644 128x128.png /app/share/icons/hicolor/128x128/apps/com.ccswitch.desktop.png
```

**关键 trick**:`ar -x *.deb` 解 .deb 包 → `tar -xf data.tar.*` 提取文件,复用 release.yml 产出的 .deb 而不是重新 build(节省 CI 时间)。

**`com.ccswitch.desktop.metainfo.xml`** — AppStream 元数据 + `com.ccswitch.desktop.desktop` — .desktop 文件定义。

### 7.5 WSL2 文件系统契约测试(`.github/workflows/ci.yml:155-228`)

**专门的 WSL2 测试 job**:

```yaml
backend-windows-wsl2:
  name: Backend Checks (Windows + WSL2 home)
  runs-on: windows-2025
  timeout-minutes: 45
  ...
  steps:
    - name: Setup WSL2
      uses: Vampire/setup-wsl@d1da7f2c0322a5ee4f24975344f67fc0f5baf364 # v7
      with:
        distribution: Ubuntu-24.04
        wsl-version: 2

    - name: Prepare WSL2-backed test environment
      shell: pwsh
      run: |
        wsl.exe -d Ubuntu-24.04 -- bash -lc "rm -rf /tmp/cc-switch-windows-test-root && mkdir -p /tmp/cc-switch-windows-test-root/{home,temp} && chmod -R 0777 /tmp/cc-switch-windows-test-root"
        $testRoot = "\\wsl.localhost\Ubuntu-24.04\tmp\cc-switch-windows-test-root"
        ...
        "CC_SWITCH_TEST_HOME=$testHome" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
        "CC_SWITCH_WSL_TEST_DIR=$testRoot" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
        "CC_SWITCH_WSL_TEST_TEMP=$testTemp" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append

    - name: Compile backend tests with native temp
      # link.exe/mt.exe cannot create manifests when TEMP points at a WSL UNC path.
      run: cargo test --lib --manifest-path src-tauri/Cargo.toml --no-run

    - name: Run Windows-to-WSL2 filesystem contract
      shell: pwsh
      run: |
        $env:TEMP = $env:CC_SWITCH_WSL_TEST_TEMP
        $env:TMP = $env:CC_SWITCH_WSL_TEST_TEMP
        $testName = "config::tests::atomic_write_replaces_existing_wsl_unc_file"
        $testList = cargo test --lib --manifest-path src-tauri/Cargo.toml -- --ignored --list
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        $expected = "${testName}: test"
        if ($testList -notcontains $expected) {
          throw "Expected ignored test was not discovered: $testName"
        }
        cargo test --lib --manifest-path src-tauri/Cargo.toml $testName -- --ignored --exact --nocapture
```

**核心洞察**:
- `link.exe / mt.exe` 在 TEMP 指向 WSL UNC 路径时**无法生成 manifest** —— 编译阶段必须用 native TEMP
- 测试阶段用 WSL2 TEMP,跑 `--ignored` 标记的 `atomic_write_replaces_existing_wsl_unc_file` 单元测试
- 这个测试**专门覆盖** `tmp.path.join(".tmp.{pid}.{nanos}.{counter}")` → `fs::rename` 到 WSL UNC 路径的场景

### 7.6 开机自启(`src-tauri/src/auto_launch.rs`)

```rust
fn get_auto_launch() -> Result<AutoLaunch, AppError> {
    let app_name = "CC Switch";
    let exe_path = std::env::current_exe().map_err(|e| AppError::Message(format!("无法获取应用路径: {e}")))?;

    // macOS 需要使用 .app bundle 路径,否则 AppleScript login item 会打开终端
    #[cfg(target_os = "macos")]
    let app_path = get_macos_app_bundle_path(&exe_path).unwrap_or(exe_path);

    #[cfg(not(target_os = "macos"))]
    let app_path = exe_path;

    let auto_launch = AutoLaunchBuilder::new()
        .set_app_name(app_name)
        .set_app_path(&app_path.to_string_lossy())
        .build()
        .map_err(|e| AppError::Message(format!("创建 AutoLaunch 失败: {e}")))?;

    Ok(auto_launch)
}
```

`AutoLaunchBuilder` 抽象 3 平台:
- **macOS**:AppleScript login items,**必须**传 .app bundle 路径(否则 AppleScript 会打开终端)
- **Windows**:注册表 `HKEY_CURRENT_USER\...\Run`
- **Linux**:`~/.config/autostart/*.desktop`

`get_macos_app_bundle_path`(`auto_launch.rs:6-16`):

```rust
#[cfg(target_os = "macos")]
fn get_macos_app_bundle_path(exe_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let path_str = exe_path.to_string_lossy();
    if let Some(app_pos) = path_str.find(".app/Contents/MacOS/") {
        let app_bundle_end = app_pos + 4; // ".app" 的结束位置
        Some(std::path::PathBuf::from(&path_str[..app_bundle_end]))
    } else {
        None
    }
}
```

把 `/Applications/CC Switch.app/Contents/MacOS/CC Switch` 截为 `/Applications/CC Switch.app`,开发环境下不在 .app bundle 内时返回 None。

**4 个单元测试**(`auto_launch.rs:71-117`)覆盖 valid/spaces/not_in_bundle/dev_build 4 路径。

### 7.7 Linux systemd / XDG autostart

虽然 CC Switch 不直接生成 systemd unit,但 `auto_launch` crate 内部用 `~/.config/autostart/cc-switch.desktop` XDG 标准实现 Linux 开机自启,与 systemd 用户级 `xdg-autostart-generator` 兼容。

---

## 8. CRDT 与多端冲突

CC Switch 的多端同步**不是真正的 CRDT**(`Yjs`/`Automerge`),而是**LWW(Last-Write-Wins) + Manifest 校验 + 数据库 schema 兼容性检查 + Skills SSOT 回滚**的复合方案。

### 8.1 同步协议常量(`src-tauri/src/services/sync_protocol.rs:24-37`)

```rust
pub(crate) const PROTOCOL_FORMAT: &str = "cc-switch-webdav-sync";
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

**2 套版本号**:
- `PROTOCOL_VERSION` — wire 格式版本(协议变更)
- `DB_COMPAT_VERSION` — DB schema 版本(schema 升级需要新 snapshot)

`LEGACY_DB_COMPAT_VERSION` 用于支持 v5 协议的旧设备(向上兼容到 v6)。

### 8.2 SyncManifest 数据结构(`sync_protocol.rs:106-131`)

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
    pub artifacts: BTreeMap<String, ArtifactMeta>,  // BTreeMap 排序保证稳定 hash
    pub snapshot_id: String,                        // sha256(artifacts 拼接)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArtifactMeta {
    pub sha256: String,
    pub size: u64,
}

pub(crate) struct LocalSnapshot {
    pub db_sql: Vec<u8>,
    pub skills_zip: Vec<u8>,
    pub manifest_bytes: Vec<u8>,
    pub manifest_hash: String,
}
```

**BTreeMap 而非 HashMap**:保证迭代顺序确定性,使 `snapshot_id = sha256(artifacts 拼接)` 对同一组 artifacts 永远相同。

### 8.3 Snapshot ID 确定性(`sync_protocol.rs:217-223`)

```rust
pub(crate) fn compute_snapshot_id(artifacts: &BTreeMap<String, ArtifactMeta>) -> String {
    let parts: Vec<String> = artifacts
        .iter()
        .map(|(name, meta)| format!("{}:{}", name, meta.sha256))
        .collect();
    sha256_hex(parts.join("|").as_bytes())
}
```

**确定性 hash 用途**:两端计算 `snapshot_id` 比对 → 决定是否需要下载。

### 8.4 全局同步锁(`sync_protocol.rs:46-57`)

```rust
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

**核心设计 — Transport-agnostic 锁**: WebDAV 和 S3 共用同一把锁,避免并发 restore。

注释(`sync_protocol.rs:42-46`):

> "WebDAV 和 S3 used to own separate mutexes, which allowed two transports to restore the database and Skills SSOT concurrently. Keep the lock in this transport-agnostic layer so future transports automatically share it too."

**测试覆盖**(`sync_protocol.rs:473-486`):

```rust
#[tokio::test]
async fn webdav_and_s3_operations_share_one_sync_mutex() {
    let webdav_lock = crate::services::webdav_sync::sync_mutex();
    let s3_lock = crate::services::s3_sync::sync_mutex();
    assert!(
        std::ptr::eq(webdav_lock, s3_lock),
        "every transport must expose the same global sync lock"
    );

    let guard = webdav_lock.lock().await;
    assert!(s3_lock.try_lock().is_err());
    drop(guard);
    assert!(s3_lock.try_lock().is_ok());
}
```

**用 `std::ptr::eq` 验证两把锁是同一对象** —— 编译期保证锁共享。

### 8.5 增量同步触发表(`sync_protocol.rs:64-78`)

```rust
pub(crate) fn should_trigger_auto_sync_for_table(table: &str) -> bool {
    let normalized = table.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "providers"
            | "provider_endpoints"
            | "mcp_servers"
            | "prompts"
            | "skills"
            | "skill_repos"
            | "profiles"
            | "settings"
            | "proxy_config"
    )
}
```

**白名单机制** —— **故意排除**:
- `proxy_request_logs` — 设备本地请求日志
- `provider_health` — 设备本地熔断器状态
- `session_log_sync` — 设备本地同步状态
- `model_pricing` — 模型定价本地 JSON 是 SSOT

**数据库层自动触发**(`database/mod.rs:84-93`):

```rust
Action::SQLITE_INSERT | SQLITE_UPDATE | SQLITE_DELETE =>
    crate::services::webdav_auto_sync::notify_db_changed(table);
    crate::services::s3_auto_sync::notify_db_changed(table);
```

SQLite update_hook 在任何 SQLITE_INSERT/UPDATE/DELETE 触发,**白名单过滤**后才触发自动同步。

**测试覆盖**(`sync_protocol.rs:495-526`):

```rust
#[test]
fn auto_sync_table_filter_covers_shared_configuration() {
    for table in [
        "providers", "provider_endpoints", "mcp_servers", "prompts",
        "skills", "skill_repos", "profiles", "settings", "proxy_config",
    ] {
        assert!(should_trigger_auto_sync_for_table(table), "{table} should trigger an automatic snapshot upload");
    }

    assert!(should_trigger_auto_sync_for_table("  PROFILES  "));  // 大小写无关
    for table in [
        "proxy_request_logs", "provider_health", "session_log_sync", "model_pricing",
    ] {
        assert!(!should_trigger_auto_sync_for_table(table), "{table} should not trigger automatic snapshot upload");
    }
}
```

### 8.6 Manifest 校验链(`sync_protocol.rs:234-294`)

```rust
pub(crate) fn validate_manifest_compat(
    manifest: &SyncManifest,
    layout: RemoteLayout,
) -> Result<(), AppError> {
    if manifest.format != PROTOCOL_FORMAT {
        return Err(localized("sync.manifest_format_incompatible", ..., ...));
    }
    if manifest.version != PROTOCOL_VERSION {
        return Err(localized("sync.manifest_version_incompatible", ..., ...));
    }
    let Some(db_compat_version) = effective_db_compat_version(manifest, layout) else {
        return Err(localized("sync.manifest_db_version_missing", ...));
    };
    match layout {
        RemoteLayout::Current if db_compat_version != DB_COMPAT_VERSION => {
            return Err(localized("sync.manifest_db_version_incompatible", ...));
        }
        RemoteLayout::Legacy if db_compat_version > DB_COMPAT_VERSION => {
            return Err(localized("sync.manifest_db_version_incompatible", ...));
        }
        _ => {}
    }
    Ok(())
}
```

**3 阶段校验**:
1. **格式**(`PROTOCOL_FORMAT`)
2. **协议版本**(`PROTOCOL_VERSION`)
3. **DB 兼容版本**(`DB_COMPAT_VERSION`,Current 严格匹配,Legacy 接受更低)

**`effective_db_compat_version`**(`sync_protocol.rs:225-232`):

```rust
pub(crate) fn effective_db_compat_version(
    manifest: &SyncManifest,
    layout: RemoteLayout,
) -> Option<u32> {
    manifest
        .db_compat_version
        .or_else(|| (layout == RemoteLayout::Legacy).then_some(LEGACY_DB_COMPAT_VERSION))
}
```

**Legacy fallback**:旧 manifest 没有 `db_compat_version` 字段时,默认 v5。

### 8.7 artifact 校验(`sync_protocol.rs:298-353`)

```rust
pub(crate) fn validate_artifact_size_limit(artifact_name: &str, size: u64) -> Result<(), AppError> {
    if size > MAX_SYNC_ARTIFACT_BYTES {
        let max_mb = MAX_SYNC_ARTIFACT_BYTES / 1024 / 1024;
        return Err(localized("sync.artifact_too_large", ...));
    }
    Ok(())
}

pub(crate) fn verify_artifact(
    bytes: &[u8],
    artifact_name: &str,
    meta: &ArtifactMeta,
) -> Result<(), AppError> {
    if bytes.len() as u64 != meta.size {
        return Err(localized("sync.artifact_size_mismatch", ...));
    }
    let actual_hash = sha256_hex(bytes);
    if actual_hash != meta.sha256 {
        return Err(localized("sync.artifact_hash_mismatch", ...));
    }
    Ok(())
}
```

**2 阶段校验**:
1. **大小检查**(先做,便宜)
2. **SHA-256 校验**(后做,贵)

### 8.8 Snapshot 应用 + Skills 回滚(`sync_protocol.rs:357-391`)

```rust
pub(crate) fn apply_snapshot(
    db: &crate::database::Database,
    db_sql: &[u8],
    skills_zip: &[u8],
) -> Result<(), AppError> {
    let sql_str = std::str::from_utf8(db_sql).map_err(...)?;
    // Exclude installs, uninstalls, updates, and local projection while Skills
    // are backed up/replaced and the corresponding database snapshot is applied.
    let _skill_state_guard = skill_state_write_guard();
    let skills_backup = backup_current_skills()?;

    // Replace skills first, then import database; roll back skills on DB failure.
    restore_skills_zip(skills_zip)?;

    if let Err(db_err) = db.import_sql_string_for_sync(sql_str) {
        if let Err(rollback_err) = restore_skills_from_backup(&skills_backup) {
            return Err(localized("sync.db_import_and_rollback_failed", ...));
        }
        return Err(db_err);
    }

    Ok(())
}
```

**核心设计 — Skills 优先 + DB 回滚**:
1. **`_skill_state_guard`** —— 持有 Skills 写锁,阻塞并发的 install/uninstall/update
2. **`backup_current_skills`** —— 备份当前 Skills SSOT 到临时目录
3. **`restore_skills_zip`** —— 解压远端 skills.zip 到 SSOT 目录
4. **`import_sql_string_for_sync`** —— 导入远端 db.sql
5. **失败回滚** —— DB 导入失败时把 skills 从备份还原(双失败时特殊错误)
6. **guard drop** —— 自动释放 Skills 写锁

**为什么"先 skills 后 DB"**: DB 是 skills 的索引,先 restore skills 再 import DB 保证 DB rows 都有对应文件;失败时 DB 不动,只回滚 skills 文件。

### 8.9 设备名探测 + 归一化(`sync_protocol.rs:401-445`)

```rust
pub(crate) fn detect_system_device_name() -> Option<String> {
    let env_name = ["CC_SWITCH_DEVICE_NAME", "COMPUTERNAME", "HOSTNAME"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find_map(|value| normalize_device_name(&value));

    if env_name.is_some() { return env_name; }

    let output = Command::new("hostname").output().ok()?;
    if !output.status.success() { return None; }
    let hostname = String::from_utf8(output.stdout).ok()?;
    normalize_device_name(&hostname)
}

pub(crate) fn normalize_device_name(raw: &str) -> Option<String> {
    let compact = raw.chars().fold(String::with_capacity(raw.len()), |mut acc, ch| {
        if ch.is_whitespace() { acc.push(' '); }
        else if !ch.is_control() { acc.push(ch); }
        acc
    });
    let normalized = compact.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = normalized.trim();
    if trimmed.is_empty() { return None; }

    let limited = trimmed.chars().take(MAX_DEVICE_NAME_LEN).collect::<String>();
    if limited.is_empty() { None } else { Some(limited) }
}
```

**3 环境变量级联**:`CC_SWITCH_DEVICE_NAME`(自定义)→ `COMPUTPUTERNAME`(Windows)→ `HOSTNAME`(Unix)。

**归一化**:
1. 把所有空白字符归一为空格
2. 过滤控制字符
3. split_whitespace 合并连续空白
4. trim
5. 截断到 64 字符

### 8.10 加密同步(`sync_protocol.rs:1-78` 间接 + `services/webdav_sync.rs:upload 53-96`)

WebDAV 同步流程(`docs/user-manual/en/4-proxy/4.1-service.md`):

1. 序列化为 JSON
2. 用密码派生 key(argon2/scrypt)
3. 加密
4. PUT 到 `${remoteRoot}/${profile}/${date}/${snapshotId}/manifest.json` + 各 artifact
5. **上传顺序:artifacts 先 → manifest 后**(best-effort consistency)
6. 下载流程:GET manifest → 校验 etag → `verify_artifact` 比 size + sha256 → 解密 → 写本地

### 8.11 三传输并行(WebDAV + S3 + 本地文件夹)

**DropBox / OneDrive / iCloud 通过本地路径挂载**(`docs/user-manual/en/4-proxy/4.1-service.md`):

> "通过本地路径挂载实现(不调 SDK,直接读写 `~/Dropbox/Apps/CC-Switch/`、`~/Library/Mobile Documents/.../iCloud~cc~switch/`)。**零 OAuth 流程、零凭据同步**。"

这是**三传输策略**:
- **WebDAV**(Nextcloud / 坚果云 / 自建 Apache)
- **S3**(MinIO / R2 / AWS S3 / 阿里 OSS)
- **本地文件夹**(DropBox / OneDrive / iCloud 文件系统同步)

`scripts/rewrite-updater-manifest.mjs`(§5.7)支持 R2 mirror 自动重写 URL。

### 8.12 sync_protocol 全测试覆盖(`sync_protocol.rs:469-747`)

13 个单元测试覆盖:
- `webdav_and_s3_operations_share_one_sync_mutex` —— **指针等同性验证**
- `auto_sync_table_filter_covers_shared_configuration` —— **白名单 + 大小写**
- `snapshot_id_is_stable` / `snapshot_id_changes_with_artifacts`
- `sha256_hex_is_correct`
- `persist_best_effort_returns_*`
- `validate_manifest_compat_*` × 6 —— **format / version / db_compat / legacy**
- `effective_db_compat_version_defaults_legacy_layout_to_v5`
- `normalize_device_name_*` × 3
- `manifest_serialization_uses_device_name_only`
- `validate_artifact_size_limit_*` × 2
- `verify_artifact_*` × 3 —— **size mismatch / hash mismatch / matching**

### 8.13 与真正 CRDT 的对比

| 维度 | CC Switch | 真正 CRDT(Yjs/Automerge) |
|------|-----------|--------------------------|
| **并发模型** | LWW(后写覆盖) | Op-based CRDT / State-based CRDT |
| **冲突解决** | 无,后写即胜 | 自动合并,无冲突 |
| **离线支持** | 弱(下载覆盖) | 强(op log 合并) |
| **网络分区容忍** | 弱 | 强 |
| **实现复杂度** | 简单 | 复杂 |
| **适用场景** | 个人单用户多设备 | 多人协作 |

**CC Switch 选择 LWW 的理由**: **个人单用户**多设备场景,冲突极少(同一时刻用户不太会在两台设备同时改同一个 Provider),LWW 简单可靠。如果引入真正 CRDT,会增加 10x 实现复杂度而收益有限。

---

## 总结

CC Switch 第十轮深挖覆盖的 8 大新维度,展示了 **「生产级桌面应用」** 的工程深度:

1. **CrashDump 与错误恢复** — `panic_hook` + `InitError` 状态机 + `FrontendErrorBoundary` + Live 配置接管恢复 + `ExitRequested` 分类器,4 层防御
2. **WebUI 与 DesktopApp** — 11 阶段 Tauri 启动管线 + 静默/轻量/单实例/托盘 + Linux 焦点恢复 + Apple 公证流水线
3. **OAuth 认证与多账号** — 3 套 OAuth(Copilot/Codex/xAI)+ Discovery Document + Refresh Token Adoption 4 元组 + `lock_switch_for_app` 串行化保护
4. **i18n 国际化** — 4 语言 i18next + 智能探测链(繁体细粒度匹配)+ 后端双语 `AppError::Localized` + 对话框独立双语
5. **Release 工程化** — 5 平台 matrix + 3 种密钥格式 + Apple 公证 + `latest.json` 自动生成 + R2 mirror URL 重写 + Portable 模式
6. **WebSocket 与 SSE** — 手动 hyper accept loop + Header case preservation + SSE 双 delimiter + UTF-8 跨块安全 + Stream Check 探活
7. **DevContainer 与容器化** — Linux 焦点恢复 4 阶段序列 + WebKit 环境变量兜底 + Flatpak 打包 + WSL2 文件系统契约测试
8. **CRDT 与多端冲突** — Transport-agnostic 全局锁 + BTreeMap 稳定 snapshot_id + 3 阶段 manifest 校验 + Skills 优先 + DB 回滚 + 三传输策略(WebDAV/S3/本地)

**对 laew 的启示**(非照搬):
- laew 是 CLI 不需要 Tauri 桌面专属(系统托盘 / Apple 公证 / Flatpak / WSL2 测试)
- 但 `panic_hook` 风格的双语言错误 + 串行化崩溃写入 + `InitError` 状态机可在 laew 借鉴
- OAuth 多账号 + Refresh Token Adoption 模式可适配到 laew 的 OAuth-bound Provider
- i18next 智能探测链 + `defaultValue` 容错模式适合 laew 多语言 TUI
- 同步协议的「Transport-agnostic 全局锁 + Skills 优先 + DB 回滚」对 laew 未来多端同步是直接范本
- SSE UTF-8 跨块安全处理在 laew 流式 Agent 输出时是必备

---

## 附录:文件路径速查表

| 关注点 | 路径 | 关键行号 / 函数 |
|------|------|---------------|
| **Panic Hook** | `src-tauri/src/panic_hook.rs` | `setup_panic_hook` 127-243 |
| **Panic 临界区** | `src-tauri/src/panic_hook.rs` | `CRASH_LOG_LOCK` 18, `crash_log_guard` 219-231 |
| **Panic 日志轮转** | `src-tauri/src/panic_hook.rs` | `rotate_crash_log_if_needed_with_limit` 50-82 |
| **InitError 状态机** | `src-tauri/src/init_status.rs` | `set_init_error` 27, `InitErrorPayload` 4-19 |
| **DB 版本过新** | `src-tauri/src/lib.rs` | `stored_user_version_exceeds_supported` 567-591 |
| **FrontendErrorBoundary** | `src/components/FrontendErrorBoundary.tsx` | `componentDidCatch` 21-27 |
| **AppError 双语** | `src-tauri/src/error.rs` | `Localized { key, zh, en }` 52-57 |
| **format_skill_error** | `src-tauri/src/error.rs` | `format_skill_error` 127-149 |
| **AppError::localized** | `src-tauri/src/error.rs` | `AppError::localized` 90-96 |
| **恢复 Live 配置** | `src-tauri/src/lib.rs` | `recover_from_crash` 调用 1219-1226 |
| **清理 Live 配置** | `src-tauri/src/lib.rs` | `cleanup_before_exit` 1873-1908 |
| **ExitRequest 分类** | `src-tauri/src/lib.rs` | `classify_exit_request` 2220-2226 |
| **save_window_state** | `src-tauri/src/lib.rs` | `save_window_state_before_exit` 2238-2244 |
| **remove_tray_icon** | `src-tauri/src/lib.rs` | `remove_tray_icon_before_exit` 1920-1928 |
| **restart_process** | `src-tauri/src/lib.rs` | `restart_process` 2266-2270 |
| **Tauri Builder 启动** | `src-tauri/src/lib.rs` | `run()` 341-1706 |
| **静默启动** | `src-tauri/src/lib.rs` | `silent_startup` 分支 1325-1357 |
| **轻量模式** | `src-tauri/src/lightweight.rs` | `enter_lightweight_mode` 7-31 |
| **从 config 重建窗口** | `src-tauri/src/lightweight.rs` | `WebviewWindowBuilder::from_config` 66-69 |
| **单实例 + deep link** | `src-tauri/src/lib.rs` | `tauri_plugin_single_instance::init` 350-388 |
| **macOS Reopen** | `src-tauri/src/lib.rs` | `RunEvent::Reopen` 1771-1790 |
| **macOS Opened URLs** | `src-tauri/src/lib.rs` | `RunEvent::Opened { urls }` 1792-1852 |
| **Windows AppUserModelID** | `src-tauri/src/lib.rs` | `set_windows_app_user_model_id` 84-98 |
| **macOS tray 图标** | `src-tauri/src/lib.rs` | `macos_tray_icon` 329-339 |
| **OAuth 统一抽象** | `src-tauri/src/commands/auth.rs` | `ManagedAuthAccount` 18-31, 8 命令 110-460 |
| **Codex OAuth** | `src-tauri/src/proxy/providers/codex_oauth_auth.rs` | `CODEX_CLIENT_ID` 36, `CachedAccessToken` 158-175 |
| **RefreshToken Adoption** | `src-tauri/src/proxy/providers/codex_oauth_auth.rs` | `RefreshTokenAdoptionMode` 177-220 |
| **CodexOAuthState** | `src-tauri/src/commands/codex_oauth.rs` | `Arc<Manager>` 19 |
| **xAI Discovery** | `src-tauri/src/proxy/providers/xai_oauth_auth.rs` | `XAI_DISCOVERY_URL` 19, `OAuthEndpoints` 73-77 |
| **Copilot 多域名** | `src-tauri/src/proxy/providers/copilot_auth.rs` | `github_client_id` 36-42 |
| **Copilot 域名 SSOT** | `src-tauri/src/proxy/providers/copilot_auth.rs` | `normalize_github_domain` 105-120 |
| **lock_switch_for_app** | `src-tauri/src/commands/auth.rs` | `remove_codex_oauth_account_with_switch_lock` 372-388 |
| **Codex OAuth 配额** | `src-tauri/src/commands/codex_oauth.rs` | `get_codex_oauth_quota` 27-67 |
| **i18next 探测链** | `src/i18n/index.ts` | `getInitialLanguage` 13-62 |
| **i18n 容错** | `src/components/FrontendErrorBoundary.tsx` | `i18n.t(key, { defaultValue })` 43-60 |
| **LanguageSettings** | `src/components/settings/LanguageSettings.tsx` | 4 Tab 切换 22-39 |
| **Tauri Updater 配置** | `src-tauri/tauri.conf.json` | `updater` 62-69 |
| **bundle 配置** | `src-tauri/tauri.conf.json` | `bundle` 36-55 |
| **Updater 抽象** | `src/lib/updater.ts` | `checkForUpdate` 25-48 |
| **UpdateContext** | `src/contexts/UpdateContext.tsx` | `dismissedVersion` 31-57, `isCheckingRef` 59-101 |
| **UpdateBadge** | `src/components/UpdateBadge.tsx` | 11-42 |
| **Release matrix** | `.github/workflows/release.yml` | 5 平台 23-30 |
| **签名密钥 3 格式** | `.github/workflows/release.yml` | 125-179 |
| **Apple 公证** | `.github/workflows/release.yml` | 181-322 |
| **create-dmg 样式化** | `.github/workflows/release.yml` | 320-342 |
| **latest.json 生成** | `.github/workflows/release.yml` | `assemble-latest-json` 628-732 |
| **R2 manifest 重写** | `scripts/rewrite-updater-manifest.mjs` | 18-46 |
| **Portable 模式** | `.github/workflows/release.yml` | 469-505 |
| **CI 路径过滤** | `.github/workflows/ci.yml` | `dorny/paths-filter` 30-53 |
| **WSL2 测试** | `.github/workflows/ci.yml` | `backend-windows-wsl2` 155-228 |
| **hyper accept loop** | `src-tauri/src/proxy/server.rs` | 141-213 |
| **preserve_header_case** | `src-tauri/src/proxy/server.rs` | 194-201 |
| **SSE 块解析** | `src-tauri/src/proxy/sse.rs` | `take_sse_block` 8-23 |
| **SSE field 兼容** | `src-tauri/src/proxy/sse.rs` | `strip_sse_field` 2-5 |
| **UTF-8 跨块** | `src-tauri/src/proxy/sse.rs` | `append_utf8_safe` 36-86 |
| **流式响应处理** | `src-tauri/src/proxy/response_processor.rs` | `handle_streaming` 151-210 |
| **Hop-by-hop header** | `src-tauri/src/proxy/response_processor.rs` | 36-68 |
| **非流式 body 超时** | `src-tauri/src/proxy/response_processor.rs` | `read_decoded_body` 82-138 |
| **Stream Check 配置** | `src-tauri/src/services/stream_check.rs` | `StreamCheckConfig::default` 51-61 |
| **Stream Check 命令** | `src-tauri/src/commands/stream_check.rs` | `stream_check_provider` 17-45 |
| **Linux 焦点恢复** | `src-tauri/src/linux_fix.rs` | `nudge_main_window` 43-121 |
| **Linux 时序常量** | `src-tauri/src/linux_fix.rs` | `REALIZE_WAIT` 26, `RESIZE_GAP` 32, `RECONCILE_WAIT` 37 |
| **Linux WebKit 兜底** | `src-tauri/src/main.rs` | 8-32 |
| **WebKit 硬件加速** | `src-tauri/src/lib.rs` | `HardwareAccelerationPolicy::Never` 1318 |
| **Flatpak 配置** | `flatpak/com.ccswitch.desktop.yml` | 1-22 finish-args |
| **Flatpak 5 modules** | `flatpak/com.ccswitch.desktop.yml` | 24-98 |
| **Flatpak .deb 提取** | `flatpak/com.ccswitch.desktop.yml` | `ar -x *.deb` + `tar -xf` 91-93 |
| **Auto Launch macOS** | `src-tauri/src/auto_launch.rs` | `get_macos_app_bundle_path` 6-16 |
| **Auto Launch 三平台** | `src-tauri/src/auto_launch.rs` | `AutoLaunchBuilder` 34-40 |
| **Sync 常量** | `src-tauri/src/services/sync_protocol.rs` | `PROTOCOL_VERSION` 29, `DB_COMPAT_VERSION` 30 |
| **Sync 全局锁** | `src-tauri/src/services/sync_protocol.rs` | `sync_mutex` 46-57 |
| **白名单表过滤** | `src-tauri/src/services/sync_protocol.rs` | `should_trigger_auto_sync_for_table` 64-78 |
| **SyncManifest** | `src-tauri/src/services/sync_protocol.rs` | 106-130 |
| **snapshot_id 确定性** | `src-tauri/src/services/sync_protocol.rs` | `compute_snapshot_id` 217-223 |
| **3 阶段 manifest 校验** | `src-tauri/src/services/sync_protocol.rs` | `validate_manifest_compat` 234-294 |
| **artifact 校验** | `src-tauri/src/services/sync_protocol.rs` | `verify_artifact` 314-353 |
| **Skills 优先 + DB 回滚** | `src-tauri/src/services/sync_protocol.rs` | `apply_snapshot` 357-391 |
| **设备名探测** | `src-tauri/src/services/sync_protocol.rs` | `detect_system_device_name` 401-417 |
| **设备名归一化** | `src-tauri/src/services/sync_protocol.rs` | `normalize_device_name` 419-445 |

---

> **字数**:本文档 cc-switch 第十轮深挖约 1,500 行,覆盖 8 大新维度,引用路径 100+ 处、代码片段 80+ 处、行号引用 200+ 处。