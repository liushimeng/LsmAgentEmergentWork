# CC Switch v3.20.0 源码调研报告

> 工程: `farion1231/cc-switch` — All-in-One Assistant for Claude Code / Codex / Gemini CLI / Grok / OpenCode / OpenClaw / Hermes Agent / Pi Agent
> 技术栈: Tauri 2 (Rust 后端) + React 18 + TypeScript + Vite
> 仓库体量: 后端约 70+ Rust 源文件,核心模块 `src-tauri/src/proxy/forwarder.rs` 5267 行、`handlers.rs` 3581 行、`services/proxy.rs` 10002 行,前端 7915 行 hook + 20+ 组件目录
> 调研维度: 16 项

---

## 一、工程结构

CC Switch 采用典型的 Tauri 2 双端工程布局:

```
cc-switch/
├── src/                       # React 前端
│   ├── App.tsx               # 主入口,66KB,挂载所有页面与路由
│   ├── types.ts              # 25KB 全局类型定义(Provider/Mcp/Skill/Settings/Sync…)
│   ├── components/           # 20+ 业务组件目录(providers/mcp/skills/proxy/usage/sessions…)
│   ├── hooks/                # 25 个自定义 hook(useProviderActions/useSkills/useMcp…)
│   ├── lib/                  # 平台/查询/Schema/版本工具
│   └── contexts/, i18n/      # i18next 多语言,主题/状态上下文
├── src-tauri/                # Rust 后端
│   ├── src/
│   │   ├── lib.rs            # 主入口(2403 行),Tauri 应用组装
│   │   ├── main.rs           # 二进制入口(Linux WebKit 兜底)
│   │   ├── commands/         # 35 个 #[tauri::command] 命令文件
│   │   ├── proxy/            # 本地代理(28 个模块,核心)
│   │   ├── services/         # 业务服务层(38 个模块)
│   │   ├── database/         # SQLite + DAO
│   │   ├── mcp/              # MCP 跨工具适配
│   │   ├── deeplink/         # ccswitch:// 协议解析
│   │   ├── session_manager/  # 各工具会话历史解析
│   │   └── *config.rs        # 8 款工具的配置适配器
│   ├── tauri.conf.json
│   └── Cargo.toml
├── docs/                     # 用户文档(ZH/EN/JA/DE)
├── tests/                    # 集成测试
└── package.json              # 前端依赖(Tauri 2.8 + Radix UI + CodeMirror + recharts + framer-motion)
```

**工程特征**:
- 后端模块按"领域概念"拆分:`provider`、`mcp`、`deeplink`、`proxy`、`session_manager`、`database`,每个领域内聚。
- 前端用 Radix UI primitives + Tailwind + shadcn/ui 风格 + framer-motion,组件按功能分组(`providers/mcp/skills/proxy/usage/sessions/hermes/openclaw/deeplink/...`)。
- `Cargo.toml` 显示 `reqwest + hyper + axum + tower-http + rquickjs + rusqlite + tokio` 全栈:Rust 自己做 HTTP 客户端/服务端、自带 DB、还内置 QuickJS 运行时(用于"用量查询脚本"——前端写 JS 解析各家订阅套餐返回)。

---

## 二、核心架构:Tauri 命令系统与前后端通信

### 2.1 应用启动流程(`src-tauri/src/lib.rs:342 run()`)

```
panic_hook::setup_panic_hook()
└─ tauri::Builder::default()
   ├─ plugin(single_instance): 复用已有实例,聚焦窗口
   ├─ plugin(deep_link): 注册 ccswitch:// 协议
   ├─ plugin(process/dialog/opener/store/window_state)
   ├─ on_window_event: 拦截关闭→最小化到托盘(由 settings.minimize_to_tray_on_close 控制)
   └─ setup(|app| {
       ├─ panic_hook + log 初始化(tauri-plugin-log,Rotate 4 归档 × 20MB)
       ├─ Database::init(): 打开 SQLite + Schema 迁移(版本 v0..v17)
       ├─ TrayIconBuilder: 系统托盘菜单(macOS/Linux/Windows 平台分支)
       ├─ usage_events::init(): 注入 AppHandle,日志事件可推送前端
       └─ invoke_handler: 批量注册所有 #[tauri::command]
   })
```

### 2.2 Tauri 命令组织(`src-tauri/src/commands/`)

`commands/` 下 35 个文件按业务垂直划分:

| 文件 | 命令组 | 关键命令 |
|------|--------|---------|
| `provider.rs` | 供应商 CRUD | `get_providers` / `add_provider` / `update_provider` / `delete_provider` / `switch_provider` |
| `proxy.rs` | 代理启停 | `start_proxy` / `stop_proxy` / `get_proxy_status` / `update_proxy_config` |
| `mcp.rs` | MCP 跨工具同步 | `sync_mcp_to_app` / `import_mcp_from_app` |
| `skill.rs` | Skill 安装 | `install_skill` / `remove_skill` / `update_skill` / `list_skills` |
| `usage.rs` | 用量统计 | `query_usage` / `get_usage_summary` / `get_request_logs` |
| `webdav_sync.rs` / `s3_sync.rs` | 云同步 | `webdav_upload` / `webdav_download` / `s3_upload` |
| `deeplink.rs` | 深链 | `handle_deeplink_url` / `import_from_deeplink` |
| `auth.rs` | OAuth 流 | `claude_oauth_login` / `codex_oauth_login` / `xai_oauth_login` |
| `failover.rs` | 故障转移 | `set_failover_queue` / `test_failover` |
| `coding_plan.rs` | 套餐用量 | `query_coding_plan` |
| `import_export.rs` | 配置迁移 | `export_config` / `import_config` |

注册方式:`commands::mod.rs` 显式 `pub use ...::*` + `lib.rs` 再次 `pub use commands::*`,最终在 `lib.rs` 的 `invoke_handler` 中 `tauri::generate_handler![命令1, 命令2, ...]` 一行搞定。

### 2.3 前后端通信模式

- **命令调用**: 前端 `import { invoke } from "@tauri-apps/api/core"` 调用后端命令。
- **事件订阅**: 后端 `app.emit("deeplink-import", &payload)` → 前端 `listen<T>("deeplink-import", handler)`。用于用量实时刷新、深链导入提示、托盘状态变更等。
- **状态共享**: 后端核心对象(`Database`、`ProxyServer`、`ProviderRouter`)用 `Arc<RwLock<T>>` 包装,挂在 `tauri::State` 上,跨命令调用。
- **混合刷新**: 用 `react-query(@tanstack/react-query)` 拉取命令结果 + 后端事件触发 query invalidation,保证 dashboard 实时性。

### 2.4 启动期多平台兼容

`lib.rs` 中对 Linux 的特殊处理堪称教科书:

```rust
// main.rs: Linux WebKit DMA-BUF 与 Wayland 兼容
WEBKIT_DISABLE_DMABUF_RENDERER=1
WEBKIT_DISABLE_COMPOSITING_MODE=1
// 钩子逃生:让用户覆盖 GDK_BACKEND
if let Ok(backend) = std::env::var("CC_SWITCH_GDK_BACKEND") {
    std::env::set_var("GDK_BACKEND", backend);
}
```

---

## 三、Provider 管理:8 款工具 × 50+ 预设

### 3.1 Provider 数据模型

`src/types.ts:11-31` 定义 React 端:

```typescript
export interface Provider {
  id: string; name: string;
  settingsConfig: Record<string, any>;  // 应用配置对象
  websiteUrl?: string;
  category?: ProviderCategory;  // official | cn_official | cloud_provider | aggregator | third_party | custom | omo
  createdAt?: number; sortIndex?: number;
  notes?: string; isPartner?: boolean;
  meta?: ProviderMeta;        // 含 apiFormat / custom_endpoints / costMultiplier / authBinding / isFullUrl / promptCacheKey / providerType …
  icon?: string; iconColor?: string;
  inFailoverQueue?: boolean;
}
```

后端 Rust 镜像定义在 `src-tauri/src/provider.rs`,用 `serde_json::Value` 存 `settings_config` 直接透传各工具原生配置 JSON,不做强制 schema。

### 3.2 8 款工具的配置适配器

每个工具有独立的 `*_config.rs`(80KB~225KB 不等):

| 文件 | 工具 | 行数 | 职责 |
|------|------|------|------|
| `claude_desktop_config.rs` | Claude Desktop | 2250 | settings.json / 3P gateway token / ONE_M_CONTEXT_MARKER |
| `codex_config.rs` | OpenAI Codex CLI | 5267 | auth.json + config.toml + model_catalog_json |
| `gemini_config.rs` | Google Gemini CLI | ~600 | settings.json + .env |
| `grok_config.rs` | xAI Grok | ~700 | 订阅绑定 |
| `opencode_config.rs` | OpenCode AI | ~450 | opencode.json (npm 包式 provider) |
| `openclaw_config.rs` | OpenClaw | ~900 | models.providers.* + agents.defaults |
| `hermes_config.rs` | Hermes Agent | ~2200 | provider 块 + 内嵌 MCP |
| `pi_config/mod.rs` | Pi Agent | ~570 | pi 专属 schema |

**统一抽象模式**:
1. `read_*_live_settings()` → 读取工具原生配置目录(如 `~/.claude/settings.json`、`~/.codex/auth.json`)返回 `serde_json::Value`。
2. `write_*_live_atomic()` → 原子写入(先写 tmp + rename),防写入中断损坏原文件。
3. `MultiAppConfig`(`src-tauri/src/app_config.rs`,44KB)统一管理 8 款应用的 schema 校验、版本检测、目录覆盖(settings 中可指定 `claudeConfigDir` 等)。
4. `providerType` 区分:`codex_oauth` / `claude_oauth` / `xai_oauth` / `bedrock` / `copilot` / `generic`,每种走不同字段映射。

### 3.3 50+ 预设 Provider

实际预设由 `src-tauri/resources/codex_deepseek_catalog_template.json`(76KB,大量模型目录条目)+ `gpt5_5_template.json`(46KB)提供,**结构化的模型目录 + Codex Responses API 模板**——CC Switch 把"预设 Provider"建模为 JSON 模板,可被 `model_fetch.rs` 自动从 models.dev 同步(见 `src/lib/modelsDevAutoSync.ts`),用户开箱即可用 PackyCode / DMXAPI / 302.AI / OhMyOpenCode 等 50+ 中转服务。

---

## 四、本地代理服务(proxy/)

CC Switch 最核心、最复杂的子系统。一个 HTTP 代理把 Claude Code/Codex/Gemini 的请求改写后转发到任意 Provider。

### 4.1 模块拓扑(`src-tauri/src/proxy/mod.rs`)

```
proxy/
├── mod.rs               # 模块索引
├── server.rs            # Axum HTTP 服务器 + 手动 hyper accept loop
├── handlers.rs          # 5 大端点 handler(claude/codex chat/codex responses/codex compact/codex alpha/gemini)
├── forwarder.rs         # 请求转发核心(5267 行)
├── provider_router.rs   # 故障转移选择 + 熔断器调度
├── circuit_breaker.rs   # 三态熔断器
├── model_mapper.rs      # 模型别名映射(haiku/sonnet/opus/fable)
├── handler_context.rs   # RequestContext:贯穿生命周期的请求元数据
├── handler_config.rs    # UsageParserConfig:各家 usage 解析策略
├── response_processor.rs # 响应处理 + 用量收集 + SSE 透传
├── providers/           # 协议转换层(适配每个工具的请求/响应)
│   ├── claude.rs / codex.rs / gemini.rs / opencode.rs / hermes.rs / copilot_auth.rs
│   ├── transform.rs              # OpenAI Chat → Anthropic
│   ├── transform_responses.rs    # OpenAI Responses → Anthropic
│   ├── transform_gemini.rs       # Gemini Native → Anthropic
│   ├── transform_codex_*.rs      # Codex Responses ↔ Chat ↔ Anthropic 互转
│   └── streaming*.rs             # SSE 流式协议转换
├── usage/               # usage 解析(parser.rs / calculator.rs / logger.rs)
├── thinking_*.rs        # thinking 签名/budget 整流器
├── copilot_optimizer.rs # Copilot 反优化(防 premium quota 偷跑)
├── content_encoding.rs  # gzip/brotli/zstd 解压
└── …
```

### 4.2 入口服务(`server.rs`)

```rust
pub struct ProxyServer {
    config: ProxyConfig, state: ProxyState,
    shutdown_tx: Arc<RwLock<Option<oneshot::Sender<()>>>>,
    server_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
}

impl ProxyServer {
    pub async fn start(&self) -> Result<ProxyServerInfo, ProxyError> {
        // 1. 绑 127.0.0.1:15721(默认,可用低冲突高端口)
        // 2. build_router() 注册路由
        // 3. 手动 hyper HTTP/1.1 accept loop + preserve_header_case(true)
        //    → 把客户端原始 header 大小写记到 extensions,转发时还原给上游
        //    → 让 wire 层看起来像 CLI 直连,避免被某些反作弊识别为代理
        // 4. oneshot 通道用于优雅停机
    }
}
```

**路由表**(同一 endpoint 支持多种前缀,容忍 Codex CLI 各种变体):

| 路径 | Handler |
|------|---------|
| `/health`, `/status` | 健康/状态 |
| `/v1/messages`, `/claude/v1/messages`, `/claude-desktop/v1/messages` | Claude Messages API |
| `/v1/chat/completions`, `/codex/v1/chat/completions` | OpenAI Chat Completions |
| `/v1/responses`, `/codex/v1/responses`, `/grokbuild/v1/responses` | OpenAI Responses API |
| `/v1/responses/compact` | Responses 远程压缩 |
| `/alpha/search`, `/codex/v1/alpha/search` | Codex Alpha Search(加密搜索协议) |
| `/v1beta/*path`, `/gemini/v1beta/*path` | Gemini 原生(含 `:generateContent` / `:streamGenerateContent` / `:countTokens` / `models`) |

**关键设计**: 默认监听 `127.0.0.1:15721`(仅本机访问),接管时把各工具的 live config 改写为指向本代理。

### 4.3 转发核心(`forwarder.rs`, 5267 行)

`RequestForwarder::forward_with_retry()` 是核心方法,流程:

```
1. 拿共享 ProviderRouter.select_providers() → 故障转移候选列表(P1..Pn)
2. 逐个 Provider 调用:
   a. allow_provider_request() → 申请熔断器探测名额(HalfOpen 限流)
   b. 构造上游 URL:
      - claude: provider.meta.apiFormat 决定 anthropic/openai_chat/openai_responses/gemini_native
      - codex: base_url + endpoint;若 isFullUrl 则直接用上游 URL(避免拼接错)
   c. 注入 Authorization(优先 provider.api_key,其次 env 中的 API_KEY/ANTHROPIC_AUTH_TOKEN)
   d. 转发 headers(保留客户端原始大小写)
   e. 应用 request overrides(headers/body,来自 meta.localProxyRequestOverrides)
   f. hyper POST + 流式 or 非流式读取
   g. 读响应头 → 写 usage 日志 → 流式转发给客户端
   h. record_result(provider, success, err) → 更新熔断器 + DB 健康度
3. 失败时:
   a. 超时/网络错误:尝试下一个 provider(max_retries)
   b. 4xx 业务错误:通常直接返回(不切)
   c. 全部失败:返回 AllProvidersCircuitOpen 错误
```

**SSE 流式处理**(`response_processor.rs` 47KB):用 `futures::Stream` + `tokio::select!` 监听 `first_byte_timeout` 与 `idle_timeout`,双超时独立计时。`SseUsageCollector` 在流末端聚合 `usageMetadata`/`message_delta` 等事件,关闭时落库。

### 4.4 Provider 路由(`provider_router.rs`)

```rust
pub async fn select_providers(&self, app_type: &str) -> Result<Vec<Provider>, AppError> {
    // 1. 查 app 当前 provider(current_provider)
    // 2. 读 proxy_config.auto_failover_enabled
    //    - 关:仅返回当前 provider
    //    - 开:遍历 failover_queue(providers 表 WHERE in_failover_queue=1 ORDER BY sort_index)
    // 3. 对每个候选调 breaker.is_available()
    //    → 跳过 Open 状态的 provider
    // 4. Codex OAuth 账号永不入队(provider_supports_failover 检查,
    //    避免把别人账号的 Authorization 拿去 P2 重试)
}
```

### 4.5 故障转移管理器(`failover_switch.rs`)

独立的 `FailoverSwitchManager`,用于"热切换"场景:故障转移开启时,P1 失败后**主动**把当前 provider 切到 P2(更新 DB),让后续请求直接命中 P2(避免每次都先打到坏 provider 浪费 1 次尝试)。

---

## 五、熔断器(`circuit_breaker.rs`)

### 5.1 三态机

```rust
pub enum CircuitState { Closed, Open, HalfOpen }
```

### 5.2 配置(`AppProxyConfig` 每 app 独立)

| 字段 | 默认值 | 含义 |
|------|--------|------|
| `circuit_failure_threshold` | 4 | Closed→Open 连续失败阈值 |
| `circuit_success_threshold` | 2 | HalfOpen→Closed 连续成功阈值 |
| `circuit_timeout_seconds` | 60 | Open→HalfOpen 等待时间 |
| `circuit_error_rate_threshold` | 0.6 | 错误率阈值 |
| `circuit_min_requests` | 10 | 计算错误率前最小请求数 |

每 app 独立配置(`proxy_config` 表 4 行:claude/codex/gemini/grokbuild),claude 默认最激进(8 次失败开,90 秒恢复)。

### 5.3 状态转移

`is_available()` 与 `allow_request()` 是两个独立 API:
- `is_available()`:只读,不消耗 HalfOpen 探测名额(用于路由选择阶段)。
- `allow_request()`:真正发起请求前调用,HalfOpen 状态限流 1 个探测名额。

```rust
pub async fn record_failure(&self, used_half_open_permit: bool) {
    let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
    match state {
        HalfOpen => transition_to_open(),  // 探测失败 → 立即回 Open
        Closed => {
            if failures >= config.failure_threshold { transition_to_open() }
            else if total >= config.min_requests {
                let error_rate = failed as f64 / total as f64;
                if error_rate >= config.error_rate_threshold { transition_to_open() }
            }
        }
    }
}
```

**巧妙之处**: `release_half_open_permit()` 让"中性"调用方(整流器重试、Reformat 等)释放探测名额而不计入健康统计,避免 HalfOpen 卡死。

---

## 六、模型映射(`model_mapper.rs`)

### 6.1 映射规则

把 Provider 的 `settings_config.env` 中的环境变量抽出:

```rust
ANTHROPIC_DEFAULT_HAIKU_MODEL   → haiku_model
ANTHROPIC_DEFAULT_SONNET_MODEL  → sonnet_model
ANTHROPIC_DEFAULT_OPUS_MODEL    → opus_model
ANTHROPIC_DEFAULT_FABLE_MODEL   → fable_model   // Claude Code 4.6+ 新模型档
CLAUDE_CODE_SUBAGENT_MODEL      → subagent_model
ANTHROPIC_MODEL                 → default_model
```

`map_model("claude-sonnet-4-5")` → 优先匹配子串 "fable" > "haiku" > "opus" > "sonnet" > 默认。fable 未单独配置时**降级到 opus**(与 Claude Code 官方分类器降级方向一致)。

### 6.2 1M 上下文后缀剥离

Claude Code 用 `[1M]` 后缀声明 100 万上下文能力,**上游 API 不识别**,转发前必须剥离:`claude-fable-5[1m]` → `claude-fable-5`。

### 6.3 整流器(Rectifier)

CC Switch 设计了一组"请求整流器"处理上游 API 拒绝/截断的边界 case:

- `thinking_rectifier.rs`:Claude Code 发的 `thinking` block 带签名,某些中转 API 会拒收(报错 `Invalid 'signature' in 'thinking' block`),整流器剥离签名。
- `thinking_budget_rectifier.rs`:处理 `budget_tokens` 与 `thinking` 约束冲突。
- `media_sanitizer.rs`:上游拒绝图片输入时,把 image 块替换为 `[Unsupported Image]` 占位,避免整个请求 422。
- `thinking_optimizer.rs` / `cache_injector.rs`:Bedrock provider 的优化(可选开启,默认关)。

---

## 七、协议转换层(`proxy/providers/`)

CC Switch 把"任意上游 → 任意客户端"的协议转换拆成独立模块。

### 7.1 适配器接口(`adapter.rs`)

```rust
pub trait ProviderAdapter {
    fn build_url(&self, base: &str, endpoint: &str, provider: &Provider) -> Result<String, _>;
    fn build_auth_header(&self, provider: &Provider) -> Result<Option<(String, String)>, _>;
    fn needs_transform(&self, provider: &Provider) -> bool;
    fn stream_extractor(&self) -> StreamUsageParser;
}
```

### 7.2 主要转换矩阵

| 客户端格式 | 上游格式 | 转换器 |
|-----------|---------|--------|
| Claude Messages | OpenAI Chat Completions | `transform.rs::openai_to_anthropic` |
| Claude Messages | OpenAI Responses | `transform_responses.rs::responses_to_anthropic` |
| Claude Messages | Gemini Native | `transform_gemini.rs::gemini_to_anthropic` (含 thoughtSignature shadow store) |
| Codex Responses | OpenAI Chat | `transform_codex_chat.rs` |
| Codex Responses | Anthropic Messages | `transform_codex_anthropic.rs` |
| Codex Responses | OpenAI Responses (xAI 等) | `transform_codex_responses_namespace.rs`(含 namespace 还原) |

**流式协议转换**(`streaming*.rs`):每种 SSE 输出格式都对应一个流转换器,把上游 chunk 实时翻译成 Claude/Codex 客户端期望的 SSE 事件,同时在末尾挂 `usage` 事件用于统计。

### 7.3 Codex Responses namespace 处理

Codex 的工具调用使用 `{namespace, name}` 二元名,但某些上游(xAI Responses)只接受扁平 `name`。`transform_codex_responses_namespace.rs` 在请求侧 flatten、响应侧 unflatten,保持客户端无感知。

### 7.4 Gemini thoughtSignature

Gemini 原生 thinking block 必须带 `thoughtSignature` 才能被多轮复用。CC Switch 用 `GeminiShadowStore`(内存 HashMap + 可选持久化)记录每 session 的签名,转发到 Anthropic 客户端时作为占位块封装,避免破坏兼容。

---

## 八、MCP 统一管理

### 8.1 跨工具适配矩阵

| 工具 | MCP 配置文件 | 格式 | 适配器 |
|------|-------------|------|--------|
| Claude Code | `~/.claude.json` 或 `~/.claude/mcp.json` | `{mcpServers: {id: {command,args,env,...}}}` | `mcp/claude.rs` + `claude_mcp.rs` |
| Codex | `~/.codex/config.toml` (TOML) | `[mcp_servers.id]\ncommand=...` | `mcp/codex.rs` (32KB,最复杂) |
| Gemini CLI | `~/.gemini/settings.json` | `{mcpServers: {...}}` | `mcp/gemini.rs` |
| Grok Build | 专用 | 简化 | `mcp/grokbuild.rs` |
| OpenCode | `opencode.json` | `{mcp: {id: {type:"local"|"remote",command[],environment,...}}}` | `mcp/opencode.rs` |
| OpenClaw | `openclaw.json` | 内嵌 | `mcp/mod.rs` |
| Hermes Agent | `hermes.json` 内嵌 | 自定义 | `mcp/hermes.rs` |

### 8.2 统一存储 vs 工具原生

`mcp_servers` 表(全局,不分 app),关键 schema:

```sql
CREATE TABLE mcp_servers (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, server_config TEXT NOT NULL,
    description TEXT, homepage TEXT, docs TEXT, tags TEXT DEFAULT '[]',
    enabled_claude INTEGER DEFAULT 0, enabled_codex INTEGER DEFAULT 0,
    enabled_gemini INTEGER DEFAULT 0, enabled_grokbuild INTEGER DEFAULT 0,
    enabled_opencode INTEGER DEFAULT 0, enabled_hermes INTEGER DEFAULT 0
);
```

每个 MCP 条目有 **per-app 启用开关**(v3.7.0 引入 `McpApps`),用户勾选 → `sync_single_server_to_*` 函数把配置写入对应工具的原生文件。

### 8.3 双向同步

- **CC Switch → 工具**: `sync_single_server_to_claude()` 等,按各工具 schema 序列化。
- **工具 → CC Switch**: `import_from_claude()` 等,读取工具原生配置并入表。

### 8.4 Deep Link 一键安装

`ccswitch://v1/import?resource=mcp&apps=claude,codex&config=...` 在前端 `DeepLinkImportDialog` 弹窗确认后调用 `import_from_deeplink`,一步跨工具安装。

---

## 九、Skills 管理

### 9.1 数据模型

`src-tauri/src/database/schema.rs:83-106`:

```sql
CREATE TABLE skills (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT,
    directory TEXT NOT NULL,        -- 本地路径
    repo_owner TEXT, repo_name TEXT, repo_branch TEXT DEFAULT 'main',
    readme_url TEXT,
    enabled_claude INTEGER DEFAULT 0,
    enabled_codex INTEGER DEFAULT 0,
    enabled_gemini INTEGER DEFAULT 0,
    enabled_grokbuild INTEGER DEFAULT 0,
    enabled_opencode INTEGER DEFAULT 0,
    enabled_hermes INTEGER DEFAULT 0,
    installed_at INTEGER, content_hash TEXT, updated_at INTEGER
);
```

### 9.2 安装源

- **GitHub repo**:`services/skill.rs` 内 `install_from_github(owner, name, branch)` → clone 到 `~/.claude/skills/<owner>-<name>/`(或 `~/.agents/skills/`,由 `skillStorageLocation` 设置决定)。
- **ZIP 上传**:用户上传 → 解压到指定目录 → 计算 `content_hash`(用 `sha2`)。
- **自定义仓库**:`skill_repos` 表存额外 GitHub repo,登录时全仓库递归扫描(支持 monorepo 多 skill)。

### 9.3 跨工具同步

`SkillSyncMethod` 设置(`auto`/`symlink`/`copy`)决定同步方式:
- `symlink`(默认):用 symlink 让各工具指向同一份物理文件,省空间、即时更新。
- `copy`:每个工具目录各拷一份,完全隔离。
- `auto`:优先 symlink,失败时降级到 copy。

### 9.4 更新检测

`migrate_v6_to_v7` 加 `content_hash` 字段,启动时比 upstream HEAD 与本地 hash,更新弹窗提示。

### 9.5 Skill 同步总入口

`SkillService`(在 `services/skill.rs` 5989 行)统一管理:安装/卸载/启用/同步/导入导出/Deep Link 接收,前后端通过 `commands/skill.rs` 的 10+ 命令交互。

---

## 十、用量统计

### 10.1 数据流

```
proxy 拦截请求
  ↓ handle_xxx 记录 start_time + first_token_ms + usage
  ↓ usage_logger 解析 SSE/JSON usage 块
  ↓ 写 proxy_request_logs 表(每请求一行)
  ↓ 定时 rollup 写 usage_daily_rollups 表(按 date + app + provider + model + request_model + pricing_model 聚合)
  ↓ 前端 dashboard 读 rollup 表出图表
```

### 10.2 关键表

```sql
-- proxy_request_logs: 每次请求明细(支撑 request detail)
proxy_request_logs (
    request_id TEXT PK, provider_id, app_type, model, request_model, pricing_model,
    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, input_token_semantics,
    input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
    latency_ms, first_token_ms, duration_ms,
    status_code, error_message, session_id, provider_type, is_streaming,
    cost_multiplier, created_at, data_source
)
-- 索引: provider, created_at, model, session, status

-- usage_daily_rollups: 仪表盘聚合
usage_daily_rollups (
    date, app_type, provider_id, model, request_model, pricing_model,
    request_count, success_count, input_tokens, output_tokens,
    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
)
```

### 10.3 模型定价(`services/model_pricing.rs`,28KB)

- 内置 100+ 主流模型价格,首次启动时 `seed_model_pricing()` 写入 `model_pricing` 表。
- `modelsDevAutoSync.ts`(前端)按周从 models.dev 同步最新价格。
- 用户自定义:可在 UI 添加任意模型定价。
- `costMultiplier`(每 provider 一个倍率,默认 1.0)用于聚合后乘——支持中转商抽成。

### 10.4 用量查询脚本(QuickJS)

`UsageScript`(`types.ts:55-79`)让用户用 JavaScript 编写自家订阅套餐的查询逻辑,后端用 `rquickjs`(QuickJS 绑定)执行。设计目标:不写 Rust 就能接入 302.AI / AnyRouter / 新 API 等中转。

```typescript
UsageScript {
  enabled: boolean; language: "javascript";
  code: string; timeout?: number;
  templateType?: "generic" | "newapi" | "volcengine" | "zhipu";
  apiKey?: string; baseUrl?: string; accessToken?: string; userId?: string;
  // ... 各 provider 差异化字段
}
```

### 10.5 仪表盘

前端用 `recharts` 库画图(`components/usage/`),展示:
- 按 app 分组的请求数/成功率/成本柱状图
- 按 model 分组的成本饼图
- 时间序列趋势折线图
- Provider 健康状态表

---

## 十一、会话管理

### 11.1 跨工具解析(`session_manager/providers/`)

每个工具有独立解析器,把原生会话 JSON 转为统一 `SessionMeta` + `SessionMessage`:

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

### 11.2 会话浏览 + 搜索

- `useSessionSearch.ts`(前端):支持全文搜索(`flexsearch` 索引本地 JSONL)。
- 显示会话元信息(标题、摘要、项目目录、创建时间、最后活跃)。
- 一键恢复:生成 `claude --resume <session-id>` 等命令,在 `terminal/mod.rs` 中打开系统终端执行(支持 iTerm2 / Warp / Alacritty / Kitty / Ghostty / Wezterm 等)。

### 11.3 Codex state.db

Codex 用 SQLite 存会话索引,CC Switch 通过 `codex_state_db.rs`(独立 rusqlite 连接)只读打开,不在写入路径。

### 11.4 session_usage_*.rs

8 个 `session_usage_*.rs`(对应 8 个工具)做"从会话日志反算用量",因为某些工具/场景下用户**关闭了 CC Switch 代理**,但 session 文件里仍有 usage 信息——这些模块把日志回填到 `proxy_request_logs`,保证 dashboard 完整。

---

## 十二、云同步

### 12.1 抽象层(`services/sync_protocol.rs` 25KB)

统一协议:

```rust
pub struct SyncSnapshot {
    version: u32; protocol_version: u32;
    device_name: String; created_at: String;
    artifacts: Vec<SyncArtifact>;  // 加密块
}
pub struct SyncArtifact {
    name: String; content_type: String;
    encrypted_payload: Vec<u8>;
    nonce: [u8; 12];  // ChaCha20-Poly1305
}
```

### 12.2 WebDAV 同步(`services/webdav.rs` + `webdav_sync.rs`)

- 任意兼容 WebDAV 的服务(Nexcloud / 坚果云 / 自建 Apache)。
- 配置文件:`~/.cc-switch/webdav-{profile}.json`(profile 隔离多账号)。
- 上传流程:序列化为 JSON → 用密码派生 key(argon2/scrypt)→ 加密 → PUT 到 `${remoteRoot}/${profile}/${date}/${snapshotId}/manifest.json` + 各 artifact。
- 下载流程:GET manifest → 校验 etag → 解密 → 写本地。
- 自动同步(可选):`webdav_auto_sync.rs` 后台 tokio 任务,定时(默认 15 分钟)上传变更。
- Manifest 哈希:`SHA256(artifacts 拼接)`,对比本地/远端判断是否需要同步。

### 12.3 S3 同步(`services/s3.rs` + `s3_sync.rs`)

- 任意 S3 兼容(MinIO / R2 / AWS S3 / 阿里 OSS)。
- 走 `rusoto` 风格自定义签名(V4 签名手写,不依赖 SDK 避免 Rust 生态绑定)。
- `s3_auto_sync.rs` 同 WebDAV 的后台调度器。

### 12.4 DropBox / OneDrive / iCloud

通过本地路径挂载实现(不调 SDK,直接读写 `~/Dropbox/Apps/CC-Switch/`、`~/Library/Mobile Documents/.../iCloud~cc~switch/`)。这套设计的好处是零 OAuth 流程、零凭据同步。

---

## 十三、Deep Link:`ccswitch://` 协议

### 13.1 URL 格式

```
ccswitch://v1/import?resource={type}&...
```

`resource` ∈ `provider` / `prompt` / `mcp` / `skill`,详细字段见 `deeplink/parser.rs`:

- **provider**:`app` `name` `homepage` `endpoint`(逗号分隔多个) `apiKey` `icon` `model` `notes` `haikuModel` `sonnetModel` `opusModel` `config` `configFormat` `configUrl` `usageScript` `usageApiKey` `usageBaseUrl` `usageAccessToken` `usageUserId` `usageAutoInterval` …
- **prompt**:`app` `name` `content` `description`
- **mcp**:`apps`(逗号分隔) `config`(JSON)
- **skill**:`repo`(owner/name) `directory` `branch`

### 13.2 注册平台路由

`tauri.conf.json` + 各平台 `Info.plist` / `.desktop` / Windows registry 注册 `ccswitch` scheme。

### 13.3 解析 → 事件 → 前端弹窗

```rust
// deeplink/mod.rs
handle_deeplink_url(app, url_str, focus_main_window, source) {
    if !url_str.starts_with("ccswitch://") { return false; }
    parse_deeplink_url(url_str) → DeepLinkImportRequest
    app.emit("deeplink-import", &request)  // 前端 DeepLinkImportDialog 监听
}
```

### 13.4 提供商导入(`deeplink/provider.rs` 44KB)

完整的"解析 + 校验 + 落 DB + 接管 live config"流程,支持:
- 直接 base64 嵌入 config JSON(`configFormat=json`)
- 从 URL 拉取 config(`configUrl=https://...`)
- 自动校验 endpoints / apiKey / 模型字段
- 自动启用(若 `enabled=true`)
- 自动设置 usage script(若提供了 usageScript)

---

## 十四、数据库设计(SQLite)

### 14.1 17 个 Schema 版本(`schema.rs`)

迁移链 v0 → v1 → ... → v17,每个版本独立函数 `migrate_vN_to_vN1`,用 SAVEPOINT 包裹确保原子。

主要里程碑:
- v0→v1:补齐缺失列(初版 schema 不完整)
- v1→v2:引入代理 + 用量统计(proxy_request_logs / model_pricing)
- v2→v3:Skills 统一管理架构(添加 app_type)
- v3→v4:OpenCode 支持
- v4→v5:计费模式(cost_multiplier / limit_daily_usd)
- v5→v6:Copilot 模板统一
- v6→v7:Skill content_hash 更新检测
- v7→v8:会话日志用量追踪
- v8→v9:全面补定价
- v9→v10:Hermes Agent
- v10→v11:usage_daily_rollups 保留 request_model 维度
- v11→v12:项目 Profiles(全应用共享项目实体)
- v12→v13:输入 token 缓存语义
- v13→v14:Grok Build 代理配置
- v14→v15:Skills/MCP 添加 Grok Build
- v15→v16:重建 Codex 会话用量
- v16→v17:会话用量持久去重账本

### 14.2 核心表清单

| 表 | 用途 |
|----|------|
| `providers` | 供应商主表(主键 `id + app_type`) |
| `provider_endpoints` | 多端点候选(provider_id 外键) |
| `mcp_servers` | MCP 跨工具配置 |
| `prompts` | 自定义 prompt 模板 |
| `skills` | 已安装 skill |
| `skill_repos` | 自定义 GitHub 仓库 |
| `settings` | 应用设置 KV 存储 |
| `proxy_config` | 4 行:claude/codex/gemini/grokbuild 各自代理配置 |
| `provider_health` | 每 provider 健康度 |
| `proxy_request_logs` | 请求明细(15 列) |
| `model_pricing` | 模型定价 |
| `stream_check_logs` | 流式连通性测试日志 |
| `proxy_live_backup` | live 配置接管前的备份 |
| `usage_daily_rollups` | 每日用量聚合 |
| `session_log_sync` | 会话日志增量同步状态 |
| `session_usage_dedup` | fork/rewrite 去重账本 |
| `profiles` | 项目 Profiles(各 app 共享) |

### 14.3 备份系统(`database/backup.rs` 130KB)

按时间轮转的全量备份:
- 触发时机:应用启动 / 用户手动 / 定时(`backupIntervalHours`)。
- 保留数:`backupRetainCount`(默认 10)。
- 备份格式:序列化所有表 → JSON + `manifest.json`(签名) → 写到 `~/.cc-switch/backups/backup-{timestamp}.json`。
- 还原:用户从备份文件选 → 校验签名 → 原子替换 DB。

---

## 十五、跨工具适配层总结

| 维度 | 设计模式 | 关键代码 |
|------|---------|---------|
| Provider 切换 | `provider_router.rs` 统一调度,8 款工具共享一套熔断/健康度 | `select_providers()` |
| MCP 同步 | 单一存储 + per-app 启用字段 + 各 app 独立 adapter 序列化 | `mcp_servers` 表 + `services/mcp.rs` |
| Skill 同步 | symlink/copy 二选一,共享一份磁盘文件 | `skillSyncMethod` 设置 |
| 协议转换 | Adapter trait + 各上游独立 streaming transform | `proxy/providers/adapter.rs` |
| 配置读取 | 每工具独立 `*_config.rs`,统一返回 `serde_json::Value` | `claude_*_config.rs` |
| 会话浏览 | 每工具独立 parser,统一 SessionMeta 接口 | `session_manager/providers/*.rs` |
| OAuth 流 | 每工具独立 OAuth 模块(Claude/Codex/xAI) | `commands/auth.rs` + `commands/codex_oauth.rs` |
| 用量反算 | 每工具独立 session_usage_*.rs | `services/session_usage_*.rs` |
| QuickJS 用量脚本 | 通用模板 + 用户自定义 JS | `UsageScript` + `rquickjs` |

---

## 十六、关键设计借鉴(对 laew 的建议)

CC Switch 在以下方面对 laew 工程(我们的 Rust Agent CLI)有直接借鉴价值:

### 16.1 本地代理(强烈建议 P0)

CC Switch 的代理层解决了三个 laew 当前没有的能力:

1. **统一模型路由**: 用户用 `laew -p "任务"` 时,可以根据模型自动选择最便宜的可用 provider,不必每次手动切换。**借鉴点**:
   - `provider_router.rs` 的"故障转移队列"(P1→P2→P3)可以直接移植到 laew,作为 `agent/yolo.rs` 的扩展。
   - `circuit_breaker.rs` 的三态机配合 `AllowResult { used_half_open_permit }` 是教科书级别,laew 可以给 `MultiAgentOrchestrator` 加 health-aware 调度。

2. **协议转换**: laew 当前只支持 Anthropic Messages + OpenAI Chat Completions 两种,接入新 provider 时常需写代码。CC Switch 的 `transform.rs` 把"OpenAI Chat → Anthropic" / "OpenAI Responses → Anthropic" 等抽象成纯函数,值得在 `src/llm/` 下加 `transform/` 子目录,支持运行时格式转换(让 laew 用 Anthropic SDK 调用 OpenAI 兼容 API,反之亦然)。

3. **请求整流**: `thinking_rectifier` / `media_sanitizer` 这类"上游不兼容时反应式重写"模式,laew 在跨 provider 切换时肯定会遇到,提前预留模块比出问题时再补好。

### 16.2 MCP 适配(强烈建议 P0)

CC Switch 用一个 SQLite 表 + per-app 启用字段,把 Claude/Codex/Gemini/OpenCode/Hermes 的 MCP 配置统一管理。**借鉴点**:

- laew 当前的 `SessionContext` 概念可以和 CC Switch 的 `mcp_servers` 表对位——把 MCP 配置从"散落在各 provider 配置文件"改成"集中存储 + per-provider 启用"。
- 双向同步逻辑(`import_from_claude` / `sync_single_server_to_claude`)展示了"读懂每工具原生格式"的代码模式,laew 接入新工具时可参照。
- Deep Link 协议(`ccswitch://v1/import?resource=mcp`)是非常优雅的"免登录分享配置"机制,laew 可以加 `laew://` 协议让用户一键共享 provider/MCP 配置。

### 16.3 跨工具集成(P1)

- **8 款工具的配置适配器模式**(每工具一个 `*_config.rs`)适合 laew 拓展"同时支持多家 LLM provider 工具链"的场景。
- **Models.dev 自动同步**(`modelsDevAutoSync.ts` + `model_pricing.rs`):每周自动从 models.dev 拉最新模型+价格,可以借鉴到 laew 的"模型目录自动维护"功能,避免人工维护成本。
- **QuickJS 用量脚本**:rquickjs 让用户用 JS 写"我家套餐的查询逻辑",laew 可用于让用户自定义 token 用量上报策略(无需重新编译)。

### 16.4 Schema 迁移(P2)

CC Switch 的 17 个 schema 版本(`migrate_vN_to_vN1` + SAVEPOINT)展示了 SQLite 增量迁移的最佳实践——laew 当前只用 `config/mod.rs` 里的 `Db::new()`,还没有迁移系统,等业务变复杂后会需要。建议学习 CC Switch 的:
- 每个迁移独立函数,便于审计
- `add_column_if_missing` 工具函数,统一处理"列已存在"错误
- SAVEPOINT 包裹,失败可回滚

### 16.5 平台兼容(P2)

`main.rs` 中 Linux WebKit 的环境变量兜底(`WEBKIT_DISABLE_DMABUF_RENDERER`、`WEBKIT_DISABLE_COMPOSITING_MODE`、`CC_SWITCH_GDK_BACKEND` 钩子),是 Tauri 应用在 Linux 上跑稳的标配。laew 如果将来加 GUI 版本,可以直接复用。

### 16.6 不建议借鉴的方面

- **过度复杂的代理层**: 5267 行 forwarder + 3581 行 handlers + 10002 行 services/proxy.rs,合计近 2 万行的代理代码,远超 laew 当前的 agent 核心。laew 只需借鉴"路由 + 熔断 + 模型映射"的核心抽象,不必全套移植。
- **桌面前端**: Tauri + React 是大工程,laew 是 CLI,无需 UI。但 hook 层(`useProviderActions.ts`、`useSkills.ts`)的"命令包装 + 乐观更新"思路可在 laew 的 `-p` 单轮模式借鉴。
- **Codex/Gemini 等专属适配**: 这些工具是 CC Switch 的核心场景,laew 主战场是 Anthropic + OpenAI,可降低适配广度。

---

## 附:关键文件路径速查

| 类别 | 文件路径 | 行数 |
|------|---------|------|
| 应用入口 | `src-tauri/src/lib.rs` | 2403 |
| 二进制入口 | `src-tauri/src/main.rs` | 95 |
| Provider 模型 | `src-tauri/src/provider.rs` | 61KB |
| 数据库 schema | `src-tauri/src/database/schema.rs` | 3412 |
| 数据库备份 | `src-tauri/src/database/backup.rs` | 130KB |
| 代理转发 | `src-tauri/src/proxy/forwarder.rs` | 5267 |
| 代理 handler | `src-tauri/src/proxy/handlers.rs` | 3581 |
| 代理服务层 | `src-tauri/src/services/proxy.rs` | 10002 |
| 熔断器 | `src-tauri/src/proxy/circuit_breaker.rs` | 496 |
| 模型映射 | `src-tauri/src/proxy/model_mapper.rs` | 429 |
| 路由 | `src-tauri/src/proxy/provider_router.rs` | 638 |
| 服务配置 | `src-tauri/src/proxy/handler_config.rs` | 229 |
| 请求上下文 | `src-tauri/src/proxy/handler_context.rs` | 378 |
| 协议转换 | `src-tauri/src/proxy/providers/*.rs` | 30+ 文件 |
| MCP 适配 | `src-tauri/src/mcp/*.rs` | 8 文件 |
| Deep Link | `src-tauri/src/deeplink/{parser,provider}.rs` | 75KB |
| Skill 管理 | `src-tauri/src/services/skill.rs` | 5989 |
| 用量统计 | `src-tauri/src/services/usage_stats.rs` | 110KB |
| 用量定价 | `src-tauri/src/services/model_pricing.rs` | 28KB |
| 同步协议 | `src-tauri/src/services/sync_protocol.rs` | 25KB |
| 命令模块 | `src-tauri/src/commands/*.rs` | 35 文件 |
| 8 工具配置 | `src-tauri/src/*_config.rs` (8 文件) | 80KB~225KB |
| 前端类型 | `src/types.ts` | 754 |
| 前端入口 | `src/App.tsx` | 1829 |
| 前端 hooks | `src/hooks/*.ts` | 25 文件 |
| 前端组件 | `src/components/*/` | 20+ 目录 |

