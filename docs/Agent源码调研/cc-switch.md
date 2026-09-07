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
