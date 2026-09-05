# cc-switch 深度分析

> 调研对象：`/usr/local/LsmGitOpenSource/cc-switch`（v3.x 桌面代理 + 统一管理工具）
> 项目定位：一站式中转/切换 Claude Code、Codex、Gemini、GitHub Copilot、GrokBuild、OpenCode、Hermes、Claude Desktop 等 8+ 种 CLI/桌面 Agent 的供应商配置；内置本地 HTTP 反向代理 + 自动故障转移 + 用量统计 + MCP 跨工具同步 + Skill 仓库管理 + DeepLink 一键导入。
> 技术栈：Rust（Tauri 2 后端）+ React + TypeScript + Tailwind + TanStack Query + Zod（前端）+ SQLite（rusqlite，本地存储）+ Hyper（自定义 HTTP 服务器）+ tokio（异步运行时）+ rquickjs（JS 引擎，用量脚本）。
> 本文聚焦 **8 个深度分析维度**：本地代理服务架构、Provider 配置同步、MCP 多工具适配、Skills 管理、数据库架构、用量统计、Rust Tauri 后端架构、前端架构。所有结论都基于实际源码阅读，并给出对 laew 的可借鉴路线图。

---

## 1. 本地代理服务架构深度

### 1.1 总体拓扑

`src-tauri/src/proxy/` 是 cc-switch 的核心代理子系统（合计超过 1.7 MB Rust 源码），包含 24 个模块。最关键的是 5 个核心组件：

| 组件 | 路径 | 行数/字节数 | 职责 |
|---|---|---|---|
| `server.rs` | `proxy/server.rs` | 25 KB | HTTP 服务器生命周期 + 路由 + 状态 |
| `forwarder.rs` | `proxy/forwarder.rs` | 217 KB（最大） | 转发主循环 + 整流器 + 故障转移 |
| `handlers.rs` | `proxy/handlers.rs` | 146 KB | 各端点 HTTP handler |
| `provider_router.rs` | `proxy/provider_router.rs` | 23 KB | 供应商选择 + 熔断器调度 |
| `circuit_breaker.rs` | `proxy/circuit_breaker.rs` | 17 KB | 熔断器三态机 |

整个代理模块由 `proxy/mod.rs` 汇总导出（见 `mod.rs:5-36`），使用 `pub(crate)` 严格控制可见性，`#[allow(unused_imports)]` 注释表明模块导出是"按需引导"风格，避免外部 API 爆炸。

### 1.2 Forwarder 主循环——`forward_with_retry_inner`（forwarder.rs:429-1156）

主循环的设计哲学是 **"per-provider 独立重试 + 跨 provider 故障转移"**，单次客户端请求最多尝试 `max_attempts = max_retries + 1` 个供应商（forwarder.rs:186-190、261）。关键控制流：

```rust
// forwarder.rs:464-540（简化）
for provider in providers.iter() {
    // 1) 上限检查（早 break，避免熔断器名额浪费）
    if attempted_providers >= self.max_attempts { break; }

    // 2) 熔断器放行许可（HalfOpen 占名额）
    let permit = self.router.allow_provider_request(&provider.id, app_type_str).await;
    if !permit.allowed { continue; }

    // 3) Pre-Send 优化器（Bedrock 专属，不污染其他 provider）
    let mut provider_body = if self.optimizer_config.enabled && is_bedrock_provider(provider) {
        optimize(body.clone())  // clone 是为了避免优化字段跨 provider 泄漏
    } else { body.clone() };

    // 4) 实际转发
    match self.forward(...).await {
        Ok(...) => {
            self.record_success_result(...);  // 异步记录，普通成功
            return Ok(...);
        }
        Err(e) => {
            // 错误分类：Retryable → 记录失败 + continue
            //            NonRetryable / ClientAbort → 释放 permit neutral + return
            // 然后串联三层整流器重试（仅 Anthropic 供应商）：
            //   4a) media_retry_should_trigger（图片降级）
            //   4b) thinking_signature 整流
            //   4c) thinking_budget 整流
        }
    }
}
```

**4 个值得借鉴的设计点**：

1. **per-provider 独立重试标记**（forwarder.rs:466-469）：`rectifier_retried / budget_rectifier_retried / media_rectifier_retried` 都是 per-provider 的局部变量。这是**极关键**的设计——首家 provider 整流后被 5xx/timeout 击落时，下一家 provider 仍能用整流后的请求体走自己的整流流程，避免标记"短路"故障转移（forwarder.rs:466 注释明示）。

2. **熔断器放行检查早于 max_attempts 检查的反向**（forwarder.rs:472-492）：把"已尝试次数上限"放在熔断器放行 *之前*。这是反直觉但正确的选择——避免在已经超限时还占用宝贵的 HalfOpen 探测名额。注释（forwarder.rs:471-473）解释得很清楚。

3. **"错误分类 + 中性释放 HalfOpen" 模式**（forwarder.rs:1050-1112）：错误被分为 3 类：
   - `Retryable`：真正 provider 故障 → 记录失败 + `update_provider_health` + 继续 failover
   - `NonRetryable`：客户端层错误（400/401/422 等）→ **不污染健康度**，仅 `release_permit_neutral`
   - `ClientAbort`：客户端断连 → 同 NonRetryable
   
   `release_permit_neutral`（provider_router.rs:204-216）这个中性接口专门用于整流器等"请求结果不应计入 Provider 健康度"的场景，仅释放 HalfOpen 名额、不触发 record_success/record_failure。这是**对熔断器反作用的优雅治理**。

4. **ActiveConnectionGuard RAII**（forwarder.rs:130-156）：进入 forward_with_retry 时构造 guard，Drop 时调度 tokio 任务把 `active_connections` -1。流式响应 body 是 future，guard move 进 body future 后随其一起 drop，避免 UI 连接计数过早归零（forwarder.rs:121-128 注释详细解释动机）。

### 1.3 熔断器——`CircuitBreaker`（circuit_breaker.rs:76-388）

实现经典 Closed / Open / HalfOpen 三态机，使用 `Arc<RwLock>` + `Arc<AtomicU32>` 混合同步原语：

```rust
pub struct CircuitBreaker {
    state: Arc<RwLock<CircuitState>>,                  // RwLock：状态转移
    consecutive_failures: Arc<AtomicU32>,              // Atomic：热路径无锁
    consecutive_successes: Arc<AtomicU32>,
    total_requests: Arc<AtomicU32>,
    failed_requests: Arc<AtomicU32>,
    last_opened_at: Arc<RwLock<Option<Instant>>>,
    config: Arc<RwLock<CircuitBreakerConfig>>,         // 热更新
    half_open_requests: Arc<AtomicU32>,                // HalfOpen 限流
}
```

关键设计：

1. **`AllowResult { allowed, used_half_open_permit }` 语义**（circuit_breaker.rs:99-103）：把"是否允许"和"是否占用了 HalfOpen 探测名额"分开，调用方在请求结束后必须把 `used_half_open_permit` 传回 `record_success/failure` 才能正确释放名额（circuit_breaker.rs:97-98 注释强调）。

2. **HalfOpen 状态限流**：max_half_open_requests = 1（circuit_breaker.rs:317），通过 `half_open_requests.fetch_add(1)` 原子抢占。`release_half_open_permit` 用 CAS 循环（circuit_breaker.rs:339-356）防御性减量。

3. **双触发条件**：连续失败次数 ≥ threshold 触发，或 total ≥ min_requests 且 error_rate ≥ threshold 触发（circuit_breaker.rs:257-286）。

4. **`transition_to_half_open` 幂等保护**（circuit_breaker.rs:367-377）：写锁内先检查 `*state != Open`，避免并发调用重置 `half_open_requests` 计数（注释解释动机）。`test_half_open_transition_does_not_reset_inflight_permit`（circuit_breaker.rs:454-475）专门验证此场景。

5. **按 app_type 独立配置**：`From<&AppProxyConfig>`（circuit_breaker.rs:51-61）+ `update_app_configs` 按前缀过滤（provider_router.rs:227-234），让 Claude / Codex / Gemini 各自一套阈值。

6. **Codex Official 不可参与 failover**（provider_router.rs:18-21）：`provider_supports_failover` 守门，因为 Codex 官方请求携带账户 native Authorization，跨账户路由会越权。`test_codex_official_current_stays_single_route_when_failover_is_stale`（provider_router.rs:472-498）专门测此场景。

### 1.4 整流器——`thinking_rectifier` + `thinking_budget_rectifier`

两个模块分别处理两类上游容错：

#### 1.4.1 Thinking Signature 整流（thinking_rectifier.rs:118-189）

`should_rectify_thinking_signature`（thinking_rectifier.rs:26-109）检测 7 种错误模式：
1. "Invalid 'signature' in 'thinking' block"
2. "Thought signature is not valid"
3. "must start with a thinking block"
4. "Expected `thinking` or `redacted_thinking`, but found `tool_use`"
5. "signature: Field required"
6. "signature ... Extra inputs are not permitted"
7. "thinking ... cannot be modified" + 非法/illegal request 兜底

整流动作（rectify_anthropic_request）：
- 遍历 `messages[*].content[]`，删除 type=thinking / type=redacted_thinking 的 block
- 删除非 thinking block 上的 signature 字段
- 兜底：如果顶层 thinking.type=enabled 且最后一条 assistant 消息首块不是 thinking 且存在 tool_use，**删除顶层 thinking 字段**（thinking_rectifier.rs:192-237 should_remove_top_level_thinking）

#### 1.4.2 Thinking Budget 整流（thinking_budget_rectifier.rs:81-122）

`rectify_thinking_budget` 强制把 `thinking.budget_tokens = 32000`，`max_tokens = 64000`（常量 MAX_THINKING_BUDGET/MAX_TOKENS_VALUE 在 budget_rectifier.rs:10-13）。Adaptive 模式跳过整流（budget_rectifier.rs:84-91）。`applied` 通过 `before != after` 派生（budget_rectifier.rs:118），无副作用时不污染调用方决策。

#### 1.4.3 Media 降级整流（forwarder.rs:194-237 + media_sanitizer.rs）

两种触发模式：
- **预防式**（apply_media_prevention，forwarder.rs:199-218）：发送前对 text-only 模型把图片块替换为 `UNSUPPORTED_IMAGE_MARKER` 标记。受 `request_media_fallback` 总开关 + `request_media_heuristic` 子开关管辖。
- **反应式**（media_retry_should_trigger，forwarder.rs:224-237）：上游 4xx 后，对同一 provider 重试一次，替换图片块为标记。仅 `Claude | Codex` 适配器 + contains_image_blocks + is_unsupported_image_error 三条件 AND。

### 1.5 模型映射——`model_mapper.rs`

`ModelMapping::from_provider`（model_mapper.rs:21-56）从 Provider settings_config.env 提取 6 类映射：
- ANTHROPIC_DEFAULT_HAIKU_MODEL / SONNET_MODEL / OPUS_MODEL / FABLE_MODEL
- CLAUDE_CODE_SUBAGENT_MODEL
- ANTHROPIC_MODEL（默认兜底）

`map_model`（model_mapper.rs:69-113）按包含匹配（case-insensitive contains）：fable → haiku → opus → sonnet → subagent → default。**Fable 特殊降级链**（model_mapper.rs:73-82）：未单独配置 fable 档时归入 opus 档（与 Claude Code 官方分类器降级方向一致），避免落到 default 失去层级。

`strip_one_m_suffix_for_upstream`（model_mapper.rs:149-159）剥离 Claude Code 的 `[1M]` 后缀（本地能力声明），上游 API 不接受。issue #3980 专门描述了 `claude-fable-5[1m]` 形态的映射场景（model_mapper.rs:260-266 测试用例）。

### 1.6 Codex OAuth 鉴权透传——forwarder.rs:1200-1243

Codex 官方 provider 走 OAuth 透传：`validate_codex_official_authorization`（forwarder.rs:57-99）校验请求携带的 Authorization 不含 `PROXY_MANAGED` 占位符、不为空；若 provider meta 关联了 `managed_account_id`，还要校验请求的 `chatgpt-account-id` header 与之匹配且 session 校验通过。这套机制保证"切换 OAuth 账号后必须重启 Codex"——通过占位符检测强制避免跨账户路由。

### 1.7 Hyper HTTP/1.1 accept loop + Header case preservation——server.rs:138-200

绕开 axum 默认行为，手写 hyper accept loop，并在每个连接内 `stream.peek()` 抓取原始 TCP 头大小写，存入 `OriginalHeaderCases` extension。目的：**保持客户端请求头的 wire-level 大小写**（典型场景：CLI 用户配置了 `X-Custom-Case` 而上游 gateway 鉴权检查大小写敏感）。注释（server.rs:6-9）解释动机。Header 大小写再由 `hyper_client.rs` 透传到上游。

---

## 2. Provider 配置同步机制深度

### 2.1 8 款工具的配置格式差异

`src-tauri/src/` 顶层有 9 个 `<tool>_config.rs`（app_config、claude_desktop_config、claude_mcp、codex_config、gemini_config、grok_config、hermes_config、openclaw_config、opencode_config），分别处理不同 CLI 工具的配置格式。差异点（基于阅读整理）：

| 工具 | 主配置 | MCP 配置 | 鉴权字段 |
|---|---|---|---|
| Claude Code | `~/.claude/settings.json` | `~/.claude.json` 或 `~/.claude/.mcp.json` | `ANTHROPIC_AUTH_TOKEN` |
| Codex | `~/.codex/config.toml` (TOML) | `config.toml` 内 `[mcp_servers.*]` | `OPENAI_API_KEY` in `auth` table |
| Gemini | TOML | `~/.gemini/settings.json` | `GEMINI_API_KEY` / `GOOGLE_API_KEY` |
| GrokBuild | TOML | 嵌入 config | 嵌入 config |
| Hermes | YAML (`config.yaml`) | 同上 | 顶层 snake_case |
| OpenClaw | JSON | JSON | JSON 嵌套 |
| OpenCode | JSON | JSON local/remote | JSON |
| Claude Desktop | JSON | JSON | API key |

### 2.2 配置原子写入——`atomic_write_with_unix_mode`（config.rs:336-498）

cc-switch 实现了生产级原子写入，三步走：
1. **临时文件创建**：在目标文件父目录下生成 `.tmp.<pid>.<nanos>.<counter>` 路径（config.rs:365-368），counter 是进程级 AtomicU64，避免 nanos 碰撞；最多重试 16 次。
2. **写入 + flush + 设置权限**（config.rs:389-408）：Unix 上若指定 unix_mode 则用 `OpenOptionsExt::mode()` 在创建时设置权限；否则继承目标文件已有权限。`atomic_write_private`（config.rs:332-334）专用于 API key 等凭据，强制 0600。
3. **替换目标**（config.rs:488-497）：Unix 上 `fs::rename`（POSIX 语义原子）；**Windows 上优先用 `ReplaceFileW`**（config.rs:410-486），若返回 `ERROR_NOT_SUPPORTED`（WSL UNC 路径特有）则 fallback 到 `fs::rename`，整体最多重试 3 次。

> **细节**：JSON 写入还会先调用 `sort_json_keys`（config.rs:277-291）按字母序递归排序所有 key，保证输出确定性，避免 git diff 噪音（config.rs:313 注释）。

### 2.3 JSON 排序输出——`sort_json_keys`

`write_json_file_with_contents`（config.rs:294-311）排序 + pretty-print + atomic_write 三步：
```rust
let value = serde_json::to_value(data)?;
let sorted_value = sort_json_keys(&value);  // 递归排序
let json = serde_json::to_string_pretty(&sorted_value)?;
atomic_write(path, json.into_bytes())?;
```

### 2.4 Codex MCP TOML 同步——`sync_enabled_to_codex`（mcp/codex.rs:286-）

Codex 用 TOML 格式，且只接受顶层 `[mcp_servers]` 表（旧错误格式 `[mcp.servers]` 被主动清理）。实现要点：
- 使用 `toml_edit`（而非 `toml` crate）允许保留未触及字段的格式与注释（mcp/codex.rs:290）。
- `read_and_validate_codex_config_text` 读取后若语法无效，**直接返回错误而非覆盖**（mcp/codex.rs:284 注释强调）。
- 收集启用项 → 用 toml_edit DocumentMut 替换 `mcp_servers` 表 → 保留其它键。

`should_sync_codex_mcp`（mcp/codex.rs:16-20）：Codex 未初始化时（`~/.codex` 不存在）**跳过同步**，按用户偏好不创建任何文件——这是用户友好的"宁可不写也不乱创建"策略。

### 2.5 与预设 Provider 库集成——`database/dao/providers_seed.rs`

虽然没完整读源码，但从 schema.rs:142-183 的 proxy_config 三行 seed insert（claude/codex/gemini/grokbuild 各一套默认阈值）以及 `mod.rs:36-37` 导出 `CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID / CODEX_OFFICIAL_PROVIDER_ID / GROKBUILD_OFFICIAL_PROVIDER_ID`，可以推断预设 Provider 库（providers_seed.rs）以常量 ID 形式嵌入官方账号，再被 Database 启动时 insert 或检查存在性。

---

## 3. MCP 多工具适配深度

### 3.1 统一抽象——`src-tauri/src/mcp/`

`mod.rs:14-20` 列出 6 个 per-tool 模块 + 1 个 `validation.rs`，每个模块导出 4 个标准函数（mod.rs:23-41）：
```rust
pub use claude::{ import_from_claude, remove_server_from_claude, sync_enabled_to_claude, sync_single_server_to_claude };
```

这套"导入 / 删除 / 批量同步 / 单条同步"四函数统一了跨工具 API 边界。`MultiAppConfig`（schema.rs 中推断）持有 `McpConfig { servers: HashMap<String, McpServer> }`，每个 `McpServer` 有 `apps: McpApps { claude, codex, gemini, grokbuild, opencode, hermes }` 6 个 bool 字段（mcp/claude.rs:95-103），实现"一次添加、按需启用"。

### 3.2 验证抽象——`validation.rs`（mcp/validation.rs）

`validate_server_spec`（validation.rs:8-51）只校验 3 种 type（stdio/http/sse），必填字段分别是 command / url。`extract_server_spec`（validation.rs:54-69）从 `McpServer.server` 提取 spec JSON。这一层把所有 per-tool 适配器的"通用合法性"集中处理。

### 3.3 Codex TOML → JSON 转换——`import_from_codex`（mcp/codex.rs:52-276）

关键实现：
- **双格式支持**（mcp/codex.rs:257-273）：同时处理 `[mcp_servers.*]` 正确格式和 `[mcp.servers.*]` 旧错误格式，import 时合并到统一结构。
- **核心字段 vs 扩展字段分流**（mcp/codex.rs:84-91、155-211）：core_fields 列举已处理的 type/command/args/env/cwd/url/headers/http_headers，其余字段通用 TOML→JSON 转换。**`headers` 和 `http_headers` 都是核心字段**——注释（mcp/codex.rs:86-88）强调两者都必须视为核心字段，避免鉴权值落入通用日志路径。
- **错误容忍**（mcp/codex.rs:217-219）：单项失败 `continue` 不中止整批；最终在 log 输出跳过的项数（mcp/codex.rs:108-110 errors 收集）。

### 3.4 Hermes YAML 适配——`mcp/hermes.rs`

Hermes 与众不同：
- **没有 type 字段**——靠是否存在 `command` / `url` 推断（hermes.rs:124-144）。
- **Hermes 专有字段**：`enabled`、`timeout`、`connect_timeout`、`tools`、`sampling`、`roots`、`auth`（hermes.rs:32-40 HERMES_EXTRA_FIELDS）。注释（hermes.rs:28-31）专门强调 `auth` 字段（OAuth 声明）即使 cc-switch 没有 OAuth UI 也必须保留 round-trip，否则会降级到未认证调用。
- **写时剥离 + 写时保留**：导出时 Hermes→CC Switch 剥离 EXTRA_FIELDS，导入时 Hermes→CC Switch 也剥离；但**写回 Hermes 时保留 EXTRA_FIELDS**（merge-on-write 逻辑）。

### 3.5 DeepLink 导入的安全解析——`deeplink/mod.rs`

`DeepLinkImportRequest`（deeplink/mod.rs:34-139）是 40+ 字段的扁平结构，覆盖 provider/prompt/mcp/skill 四种资源 + config 文件 + usage script。`parse_deeplink_url`（mod.rs:25）是 URL 解析入口。安全设计：

- **凭据字段明确隔离**（deeplink/mod.rs:62-65）：`api_key` / `usage_api_key` / `usage_access_token` / `usage_user_id` 分别独立。
- **容量控制**：`config` 字段是 Base64 编码（deeplink/mod.rs:106），避免 URL 长度爆炸。
- **`usage_script` 不默认启用**（deeplink/mod.rs:117-120）：注释明确——携带脚本本身不意味着运行，必须显式 `usageEnabled=true`。这是经典的"opt-in by default"安全策略。

`commands/deeplink.rs` 入口处理 import_deeplink 命令，从 URL Scheme `ccswitch://` 接收导入请求。

---

## 4. Skills 管理深度

### 4.1 统一存储模型——v3.10.0+

`schema.rs:84-106` 定义 `skills` 表统一结构：
```sql
skills(id PK, name, description, directory, repo_owner, repo_name, repo_branch='main',
       readme_url,
       enabled_claude, enabled_codex, enabled_gemini, enabled_grokbuild, enabled_opencode, enabled_hermes,
       installed_at, content_hash, updated_at)
```

每个 skill 的 6 个 `enabled_<app>` 字段决定其启用范围，SSOT 存储在 `~/.cc-switch/skills/`。

### 4.2 仓库管理——`skill_repos`（schema.rs:109-116）

```sql
skill_repos(owner, name, branch DEFAULT 'main', enabled, PRIMARY KEY (owner, name))
```

仓库以 GitHub owner/name 标识，可启用/禁用全局。

### 4.3 安装入口——`commands/skill.rs`

`commands/skill.rs` 暴露 14 个 `#[tauri::command]`：
- 统一管理：get_installed_skills / install_skill_unified / uninstall_skill_unified / toggle_skill_app / restore_skill_backup
- 发现：discover_available_skills / check_skill_updates / update_skill
- 跨工具扫描：scan_unmanaged_skills / import_skills_from_apps
- 仓库管理：get_skill_repos / add_skill_repo / remove_skill_repo
- ZIP 安装：install_skills_from_zip
- 公共目录搜索：search_skills_sh

### 4.4 仓库引用安全校验——`SkillService::validate_repo_ref`

`commands/skill.rs:305-313` 注释明确：owner/name/branch 会拼接到归档下载 URL，主防线在 `download_repo`，但参数非法时**当场报错**而不是沉淀进表（"不要让垃圾数据进入数据库"）。

### 4.5 向后兼容的旧 API

`commands/skill.rs:184-291` 保留 5 个兼容旧 API（get_skills / get_skills_for_app / install_skill / uninstall_skill / uninstall_skill_for_app），转发到统一 API 即可。这是"新 API 替代旧 API 时保留兼容层"的标准做法。

---

## 5. 数据库架构深度

### 5.1 表清单——schema.rs

`schema.rs` 共 18 张表 + 17 个 schema 迁移版本（`SCHEMA_VERSION: i32 = 17`，database/mod.rs:56）。按职责分组：

| 表 | 行 | 职责 |
|---|---|---|
| `providers` | 25-44 | 供应商主表（PK: id+app_type） |
| `provider_endpoints` | 49-60 | 多端点（FK: providers ON DELETE CASCADE） |
| `mcp_servers` | 64-74 | MCP 服务器（6 个 enabled_* 字段） |
| `prompts` | 77-81 | 提示词（PK: id+app_type） |
| `skills` / `skill_repos` | 84-116 | Skill + 仓库 |
| `settings` | 119-122 | KV 配置 |
| `proxy_config` | 126-183 | 三行结构 app_type PK，4 套默认配置（claude/codex/gemini/grokbuild） |
| `provider_health` | 186-192 | 健康度（PK: provider_id+app_type） |
| `proxy_request_logs` | 197-232 | 请求日志（含 5 个索引） |
| `model_pricing` | 235-244 | 模型定价 |
| `stream_check_logs` | 247-259 | 流式连通性测试日志 |
| `proxy_live_backup` | 264-270 | Live 配置备份（PK: app_type） |
| `usage_daily_rollups` | 276-296 | 日聚合（PK: date+app+provider+model+request_model+pricing_model） |
| `session_log_sync` | 299+ | 会话日志同步状态 |

### 5.2 Schema 迁移策略——`apply_schema_migrations` + `user_version`

使用 SQLite 原生 `PRAGMA user_version` 管理 schema 版本。每次启动：
1. `Database::init`（mod.rs:100-167）先 `get_user_version`
2. 若 `version > 0 && version < SCHEMA_VERSION` → **先做 pre-migration 备份**（mod.rs:128-140 `backup_database_file`）
3. 再 `apply_schema_migrations` 升级
4. 升级失败也不阻断（备份失败仅 warn，mod.rs:136-138）

`stored_user_version_exceeds_supported`（mod.rs:174-183）专门处理"数据库版本比应用新"的反向场景——返回 `Some(version)` 让 UI 引导用户升级应用而非反复弹无效重试对话框。

### 5.3 备份系统——`backup.rs`（135 KB）

#### 5.3.1 SQL 导出

`export_sql_string`（backup.rs:118-121）→ `snapshot_to_memory` → `dump_sql` 输出标准 SQLite SQL 文本。`export_sql_string_for_sync`（backup.rs:124-127）在导出时跳过本地专属表。

#### 5.3.2 SQL 导入的 authorizer 防御——`import_authorizer`（backup.rs:63-83）

**这是 cc-switch 安全设计的精华**：导入 SQL 时拒绝一切"越界动作"：
- `AuthAction::Attach` / `Detach`（ATTACH DATABASE / VACUUM INTO）→ 拒绝
- `AuthAction::CreateVtable` / `DropVtable`（csvfile/zipfile 等 vtable 模块能读写任意路径）→ 拒绝
- `AuthAction::Unknown` → 拒绝（防御未来 SQLite 新增的跨文件语句）
- `Pragma` → 只放行 `foreign_keys` / `user_version` 白名单

注释（backup.rs:39-62）长篇解释为什么用 authorizer 而非关键字扫描：
- 字符串扫描会被 `/*x*/ATTACH`、大小写、换行绕过
- authorizer 在 prepare 阶段按"解析结果"回调，绕不过语法层
- `ATTACH DATABASE 'x'`、`VACUUM INTO 'x'`、裸 `VACUUM` **三者都**报 `AuthAction::Attach`，所以拒 Attach 一条即可覆盖

#### 5.3.3 临时数据库 + Backup API 两段式

`import_sql_string_inner_with_hook`（backup.rs:173-249）：
1. `validate_cc_switch_sql_export` 头注释校验
2. 创建 `NamedTempFile` + 设置 `auto_vacuum=INCREMENTAL`（**关键**：避免导入把主库 auto_vacuum 模式降级，backup.rs:195-198 注释）
3. 装上 authorizer → `execute_batch` → 卸 authorizer
4. `validate_imported_schema` 校验 schema（必须在 `create_tables_on_conn` *之前*，否则迁移可能补齐缺失表，让截断文件伪装成合法，backup.rs:218-220 注释）
5. 走 `create_tables_on_conn + apply_schema_migrations_on_conn` 补齐缺失表/迁移
6. 加 `BACKUP_FILE_OPERATION_LOCK`（backup.rs:26 全局静态锁，保证 "安全快照 + 本地表读 + 最终替换" 期间无并发写入）
7. `Backup::new(&temp_conn, &mut main_conn)` + `complete_backup`——使用 SQLite 官方的 Backup API 做 pages 复制

`complete_backup` 是循环 `backup.step(N)` 直到 `StepResult::Done` 的工具函数。

#### 5.3.4 WebDAV / S3 同步的本地表保留——`SYNC_SKIP_TABLES` + `SYNC_PRESERVE_TABLES`

backup.rs:86-105 定义两组表名：
- `SYNC_SKIP_TABLES`：导出时跳过这些表的数据（`proxy_request_logs / stream_check_logs / provider_health / proxy_live_backup / usage_daily_rollups / session_log_sync / session_usage_dedup`）
- `SYNC_PRESERVE_TABLES`：导入时这些表的数据从当前主库 *回填* 到导入结果上

注释（backup.rs:94-95）：`proxy_request_logs` 等是设备本地数据，多设备同步时不能跨设备覆盖。

### 5.4 `Database::conn` 用 `Mutex<Connection>` 包装——mod.rs:80-82

注释（mod.rs:78-80）解释：因为 `rusqlite::Connection` 本身不是 `Sync`，需要 Mutex 包装才能在 Tauri State 多线程共享。`lock_conn!` 宏（mod.rs:65-71）避免 `Mutex::lock().unwrap()` 的 panic。

### 5.5 `register_db_change_hook`——变更通知

mod.rs:84-93 在 Connection 上注册 update_hook：
```rust
Action::SQLITE_INSERT | SQLITE_UPDATE | SQLITE_DELETE =>
    crate::services::webdav_auto_sync::notify_db_changed(table);
    crate::services::s3_auto_sync::notify_db_changed(table);
```
任意表写操作触发 WebDAV / S3 自动同步。这是"数据库层自动触发跨设备同步"的精巧设计。

### 5.6 增量 vacuum 与启动清理——mod.rs:142-164

启动后：
1. `apply_schema_migrations`
2. `ensure_incremental_auto_vacuum`：检测 auto_vacuum 模式，非 INCREMENTAL 时**先 `backup_database_file` 备份**再 `VACUUM` 重建（mod.rs:255-263）
3. `ensure_model_pricing_seeded`：内置模型定价种子
4. `cleanup_old_stream_check_logs(7)`：7 天前日志清理
5. `rollup_and_prune(30)`：30 天前的请求日志聚合到 `usage_daily_rollups` 后删除
6. `PRAGMA incremental_vacuum`：回收空间

---

## 6. 用量统计系统深度

### 6.1 TokenUsage 模型——`parser.rs`

`TokenUsage`（parser.rs:58-71）：
```rust
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    pub model: Option<String>,
    pub message_id: Option<String>,  // 用于跨源去重
}
```

`has_billable_tokens`（parser.rs:94-99）过滤全 0 用量——避免流式响应里 OpenAI 兼容上游省略 usage 时插入无意义空行。

### 6.2 6 套解析器——Claude / OpenAI / Codex / Gemini / DeepSeek / GrokBuild

parser.rs:104-200+ 给出：
- `from_claude_response`（非流式）
- `from_claude_stream_events`（流式，需要合并 message_start / message_delta 的 usage，parser.rs:194-199 处理"上游同时给 start 完整上下文 + delta 修正 fresh input"的去重）
- `openai_cache_read_tokens` / `openai_cache_write_tokens`（parser.rs:12-35）：4 个字段 fallback——`cache_read_input_tokens` → `input_tokens_details.cached_tokens` → `prompt_tokens_details.cached_tokens` → `prompt_cache_hit_tokens`（DeepSeek 文档化字段，注释解释为什么 fallback）
- Claude/Codex/Gemini 三家分别有 `from_*_response` 和 `from_*_stream_events`

`dedup_request_id`（parser.rs:77-87）生成稳定的 request_id 用于跨源去重：Claude / Claude Desktop 共享 `session:{message_id}` namespace（与 session JSONL 主键收敛）；其他 app 加入 `app:provider` 作用域避免 envelope id 复用导致覆盖。

### 6.3 成本计算——`calculator.rs`

`CostCalculator::calculate_for_app`（calculator.rs:56-70）按 app_type 选择 cache 语义：
- **OpenAI/Codex/Responses/Gemini**：`input_tokens` 包含 cache_read + cache_creation（`is_cache_inclusive_app`），需先 `saturating_sub` 两者再按输入价计费（calculator.rs:82-89）
- **Anthropic/Claude**：`input_tokens` 已经是 fresh input，直接按输入价计费

使用 `rust_decimal::Decimal` 高精度计算（calculator.rs:6、78），避免浮点误差。`cost_multiplier`（calculator.rs:104）只乘到总价，不乘到各项明细——保留明细可审计。

### 6.4 Usage Logger——`logger.rs`

`log_request`（logger.rs:101-）实现幂等写入：
1. 计算 `input_token_semantics`（FRESH=0 / TOTAL=1）由 `is_cache_inclusive_app` 决定
2. `load_existing_semantic` 查重——若已有相同 request_id + data_source=proxy + semantic 完全相同，直接返回（logger.rs:140-145）
3. 若不匹配，生成 `fallback = "request_id:collision:{sha256(semantic)}"` 二次写入（logger.rs:147-160）—— sha256 防碰撞
4. SQL：`INSERT OR REPLACE` (replace_session_log) 或 `INSERT OR IGNORE`

### 6.5 日聚合（usage_daily_rollups）——schema.rs:276-296

主键包含 `(date, app_type, provider_id, model, request_model, pricing_model)` 六元组，特别记录 `request_model`（路由接管的客户端别名）+ `pricing_model`（写入时实际计价模型）。注释（schema.rs:273-275）强调：明细被 prune 后接管计费不可审计；历史行迁移时填 `''`（未知）。

---

## 7. Rust Tauri 后端架构深度

### 7.1 Tauri 2 命令设计

所有 UI 入口都是 `#[tauri::command]`，分模块放在 `src-tauri/src/commands/`：
- `commands/provider.rs` (45 KB) — Provider CRUD + 切换
- `commands/proxy.rs` (14 KB) — 启停代理 + 状态
- `commands/mcp.rs` (6 KB) — MCP 导入/同步
- `commands/skill.rs` (10 KB) — Skills 安装/卸载
- `commands/auth.rs` (16 KB) — OAuth（Codex / Copilot / X.AI）
- `commands/failover.rs` (9 KB) — 故障转移配置
- `commands/import_export.rs` (8 KB) — 配置导入导出
- `commands/misc.rs` (291 KB!) — 其他杂项命令

`State<'_, AppState>` 是标准 Tauri 状态注入模式，AppState 在 `lib.rs` 注册。

### 7.2 AppState 与 ProxyService

`src-tauri/src/store.rs` 定义 `AppState`（推断）：
- `db: Arc<Database>`
- `proxy_service: ProxyService`（启动/停止/查询代理服务器）
- `failover_manager: Arc<FailoverSwitchManager>`（failover_switch.rs）
- `proxy_status: Arc<RwLock<ProxyStatus>>`

ProxyService 持有 `ProxyServer`（server.rs:54-92），调用 `start/stop` 操作生命周期。

### 7.3 异步任务调度

`tokio` 全异步，关键模式：
- `tokio::spawn` 用于 fire-and-forget（如 forwarder.rs:573-575 故障转移切换后异步通知 UI/托盘）
- `tokio::select!` 用于 accept loop 监听 shutdown signal（server.rs:144-152）
- `oneshot::channel` 用于优雅停止（server.rs:106）
- `ActiveConnectionGuard::Drop`（forwarder.rs:144-156）通过 `tokio::runtime::Handle::try_current().spawn` 把同步减量变成异步

### 7.4 IPC 消息流——前端 → 后端

前端通过 `@tauri-apps/api/core` 的 `invoke` 调用，类型由 `src/types.ts` 定义（62 KB）。典型调用链：
```
React Component → @/lib/api/<module>.ts (Zod 校验)
  → invoke<Args, Result>('command_name', args)
    → Tauri IPC
      → #[tauri::command] fn name(state: State<AppState>, args) -> Result<T, String>
        → AppError → String
```

错误转 String 是 Tauri 命令返回值的常见模式（cmd 返回 `Result<T, E>` 时 E 必须实现 `Serialize`，String 是最稳的选择）。

### 7.5 rquickjs——JS 引擎用于用量脚本

`src-tauri/src/usage_script.rs` (26 KB) 暴露了 JS 引擎执行用户脚本（DeepLink 携带的 `usage_script` 字段）。变量注入 `{{apiKey}}` / `{{baseUrl}}` / `{{providerId}}` 让脚本读取凭据（provider.rs:135-198 `resolve_usage_credentials` 的设计就是为这个服务）。`usage_api_key` / `usage_base_url` / `usage_access_token` / `usage_user_id` 字段（deeplink/mod.rs:124-138）允许脚本用与 provider 不同的凭据查询用量（如 NewAPI 模板）。

`commands/usage.rs` (9 KB) 暴露 5 个命令：脚本注册、测试、执行、列表、删除。

### 7.6 WebDAV / S3 自动同步——services/

`commands/webdav_sync.rs` (15 KB) + `commands/s3_sync.rs` (15 KB) + `services/webdav_auto_sync.rs` / `services/s3_auto_sync.rs` —— 通过 `register_db_change_hook`（mod.rs:84-93）监听所有表写入，异步触发远端同步。这是 cc-switch "多设备无缝同步"的核心架构。

---

## 8. 前端架构深度

### 8.1 App.tsx 主壳

`src/App.tsx` 是 66 KB 的应用主组件，导入 30+ Lucide 图标 + 100+ 内部模块。设计模式：
- ViewState 集中在 `type View = "providers" | "settings" | "prompts" | "skills" | ...`（App.tsx:116-130），通过 setState 切换
- `STORAGE_KEY = "cc-switch-last-app"`（App.tsx:141）持久化最后访问的 app 和 view
- 跨 View 的 Provider 状态通过 `useProviderActions` hook 共享

### 8.2 TanStack React Query + 强类型 api 层

`src/lib/query/`（queries.ts、mutations.ts、queryClient.ts）是 React Query 封装：
- `queryClient.ts` 定义全局 QueryClient
- `queries.ts` 用 `queryKey` + `queryFn` 定义所有查询（如 `proxyKeys` / `openclawKeys` / `hermesKeys`）
- `mutations.ts` 定义 mutation（invalidateQueries 联动）

`src/lib/api/`（providers.ts / proxy.ts / mcp.ts / skills.ts 等 20+ 文件）是 invoke 封装，每文件对应 commands/ 子模块。例如 `lib/api/providers.ts` 暴露 `providersApi.list(appId)` → `invoke<Provider[]>("get_providers", { appId })`。

### 8.3 Zod 表单验证——`lib/schemas/provider.ts`

`providerSchema`（schemas/provider.ts:38-58）定义 Provider 表单：
- `name: z.string()` — 必填校验移至 submit 时 toast
- `websiteUrl: z.string().url().optional().or(z.literal(""))` — 空字符串也合法
- `notes: z.string().optional()`
- `settingsConfig: z.string().min(1).superRefine((v, ctx) => { try JSON.parse(v) catch {...} })` — JSON 内容必须可解析

`parseJsonError`（schemas/provider.ts:6-36）解析 Chrome V8 与 Firefox 两种 JSON 错误格式（`at position N` vs `line N column N`），返回中文友好提示。

### 8.4 shadcn/ui 组件复用

`src/components/ui/` 包含 button / dialog / input / select / tabs / toast 等 30+ shadcn 组件，是项目风格统一的基础。

### 8.5 i18next 国际化

`src/i18n/`（locales/ 目录）放置 zh / en / ja 等 8+ 语言翻译。`useTranslation()` hook 在 App.tsx:3 导入，所有可见文案走 t("...") 包裹。

### 8.6 错误边界 + 日志上报

`src/components/FrontendErrorBoundary.tsx` + `src/lib/frontendLogger.ts` —— 前端 panic 时捕获并上报到后端日志服务。

### 8.7 Framer Motion + Sonner Toast

`framer-motion` 用于动画（App.tsx:3），`sonner` 用于全局 toast 提示（App.tsx:4）—— `toast.success("已切换到 X")`。

---

## 9. 对 laew 的深度借鉴建议（按优先级）

### P0（一周内落地，价值最高）

1. **熔断器模式 + Per-Provider 状态机**
   - 路径：参考 `circuit_breaker.rs` + `provider_router.rs`
   - 落地：在 `agent/provider_router.rs` 实现 `CircuitBreaker`（Closed / Open / HalfOpen）+ AllowResult { used_half_open_permit }
   - 价值：laew 当前 LLM 调用失败后无限重试，浪费 token；熔断后能让 Yolo / Main-Work agent 快速 failover
   - 验证：单元测试覆盖三个状态转换 + 并发探测名额竞争

2. **"中性释放 HalfOpen permit"接口**
   - 路径：参考 `provider_router.rs:204-216 release_permit_neutral`
   - 落地：laew 整流器重试失败时（如 thinking signature 修复失败），调用 release_permit_neutral **不**污染健康度
   - 价值：避免 laew 的 client-side error 错误计入熔断器导致"全网段屏蔽"

3. **Provider 选择时的"队列 vs 当前"语义分离**
   - 路径：参考 `provider_router.rs:72-130 select_providers`
   - 落地：在 laew 的 Yolo Runner 添加 `auto_failover_enabled` 开关，开启时仅按队列顺序 P1→P2→…（忽略 current），关闭时只用 current
   - 价值：用户可显式启用"按队列重试"而非"先 current 失败再 fallback"

### P1（一个月内落地，价值中等）

4. **Thinking Signature 整流器**
   - 路径：参考 `thinking_rectifier.rs` 7 种错误模式识别
   - 落地：在 laew 的 anthropic-protocol adapter 中加 `rectify_anthropic_request`，触发后自动删除 thinking/redacted_thinking block 并对同一 provider 重试
   - 价值：解决"Anthropic API 第三方中转报 Invalid signature"导致的死循环

5. **Model Mapper 多档映射**
   - 路径：参考 `model_mapper.rs` 6 档映射 + fable 降级链
   - 落地：laew 的 Provider 配置扩展为支持 `ANTHROPIC_DEFAULT_HAIKU/SONNET/OPUS/FABLE_MODEL` 独立映射，`map_model` 在请求前改写 model 字段
   - 价值：用户用第三方中转（DeepSeek / GLM）时能让 haiku/sonnet/opus 落到不同国产模型

6. **ActiveConnectionGuard RAII**
   - 路径：参考 `forwarder.rs:130-156`
   - 落地：laew 的 SubAgent 工作流目前用"手动 +1 / -1"，改为 guard 模式，guard move 进流式 future 后自动 drop
   - 价值：避免 `active_connections` 计数过早归零导致 UI 显示错误

7. **错误分类三态 + 中性释放**
   - 路径：参考 `forwarder.rs:1050-1112 categorize_proxy_error`
   - 落地：把 laew 的 `ProxyError` 分类为 Retryable / NonRetryable / ClientAbort，NonRetryable 跳过熔断器
   - 价值：用户取消请求 / 输入校验失败不会污染 provider 健康度

8. **原子写入 + JSON 排序**
   - 路径：参考 `config.rs:336-498 atomic_write_with_unix_mode` + `sort_json_keys`
   - 落地：laew 的 Provider 持久化从 `fs::write` 升级为 atomic_write（写临时文件 + rename），JSON 输出按 key 排序
   - 价值：避免 laew 写入过程中崩溃导致 SQLite/JSON 半损坏；JSON diff 可读性提升

9. **用量统计 + 成本计算**
   - 路径：参考 `usage/calculator.rs` + `usage/logger.rs` + `usage/parser.rs`
   - 落地：在 laew 添加可选的 `usage_stats` feature，捕获 input/output/cache tokens，按 app_type 选择 cache 语义（Claude FRESH vs Codex TOTAL），用 `rust_decimal` 高精度计算
   - 价值：让用户看到每次任务的真实成本，支持按 provider/model 分桶计费

### P2（半年内落地，战略价值）

10. **本地 HTTP 反向代理**
    - 路径：参考 `proxy/server.rs` + `proxy/handlers.rs`
    - 落地：laew 暴露 `127.0.0.1:<port>` HTTP 代理端口，把所有 Anthropic / OpenAI 请求统一走代理，自动 failover
    - 价值：可独立作为"Claude Code 路由器"使用，与 laew CLI 解耦

11. **Hyper HTTP/1.1 accept loop + Header case preservation**
    - 路径：参考 `server.rs:138-200`
    - 落地：laew 代理场景下保留客户端请求头原始大小写，避免上游 gateway 大小写敏感鉴权失败
    - 价值：与企业版 gateway 兼容

12. **MCP 多工具适配层**
    - 路径：参考 `mcp/{claude,codex,gemini,opencode,hermes}.rs` 统一抽象
    - 落地：laew 添加 `mcp/` 子模块，定义 `validate_server_spec` + 4 个标准函数（import/remove/sync/sync_single），按 AppType 路由到不同 adapter
    - 价值：让 laew 像 cc-switch 一样能管理 8 种 CLI 的 MCP 配置

13. **DeepLink 安全导入**
    - 路径：参考 `deeplink/mod.rs`
    - 落地：laew 实现 `laew://import?resource=provider&app=claude&...` URL Scheme，base64 编码配置内容，usage_script 不默认启用
    - 价值：社区分享 Provider 配置一键导入

14. **Skills 仓库管理**
    - 路径：参考 `commands/skill.rs` + `schema.rs:84-116`
    - 落地：laew 添加 `skills` 子系统，统一存储在 `~/.laew/skills/`，支持 GitHub repo / ZIP 安装、6 个 app 启用开关
    - 价值：把"Skill"概念从单工具抽象为跨工具资产

15. **数据库备份的 authorizer 防御**
    - 路径：参考 `backup.rs:63-83 import_authorizer`
    - 落地：laew 未来若支持"导入导出全部配置"功能，导入 SQL 必须安装 authorizer 拒绝 ATTACH / VACUUM INTO / 未知 vtable
    - 价值：阻断"恶意 SQL 备份文件"在导入时执行任意路径写入的攻击向量

16. **WebDAV / S3 多设备同步**
    - 路径：参考 `commands/webdav_sync.rs` + `register_db_change_hook`（mod.rs:84-93）
    - 落地：laew 在 SQLite 上注册 update_hook，任何表写入异步触发 WebDAV / S3 同步
    - 价值：用户多设备无缝共享 Provider / Skill / Prompt 配置

### 反模式警示

- **不要照搬 `forwarder.rs` 的 217 KB 单文件**：laew 应在引入 failover 之前先把"主循环 + 整流器 + 适配器"分文件（cc-switch 这 1.7 MB 是历史包袱）。
- **不要照搬 `commands/misc.rs` 291 KB 单文件**：command 注册应按业务域分文件（provider/proxy/mcp/skill）。
- **不要把 18 张表平铺**：laew 当前 6 张表已经够用，过度规范化会增加维护成本。
- **不要把所有 per-app 配置塞进 `proxy_config` 三行结构**：laew 当前按 `app_type` 索引的 SQLite 模型已足够简洁。

---

## 10. 关键代码索引（速查表）

| 关注点 | 路径 | 行号 / 函数 |
|---|---|---|
| 主转发循环 | `src-tauri/src/proxy/forwarder.rs` | `forward_with_retry_inner` (429-1156) |
| 熔断器三态机 | `src-tauri/src/proxy/circuit_breaker.rs` | `CircuitBreaker::allow_request` (157-200) |
| 供应商选择 | `src-tauri/src/proxy/provider_router.rs` | `ProviderRouter::select_providers` (45-131) |
| 错误分类 | `src-tauri/src/proxy/forwarder.rs` | `categorize_proxy_error` (1054-) |
| 中性释放 | `src-tauri/src/proxy/provider_router.rs` | `release_permit_neutral` (204-216) |
| Thinking 整流 | `src-tauri/src/proxy/thinking_rectifier.rs` | `rectify_anthropic_request` (118-189) |
| Budget 整流 | `src-tauri/src/proxy/thinking_budget_rectifier.rs` | `rectify_thinking_budget` (81-122) |
| 模型映射 | `src-tauri/src/proxy/model_mapper.rs` | `apply_model_mapping` (119-145) |
| HTTP 服务器 | `src-tauri/src/proxy/server.rs` | `ProxyServer::start` (94-200) |
| 故障转移切换 | `src-tauri/src/proxy/failover_switch.rs` | `FailoverSwitchManager::try_switch` (41-72) |
| Codex OAuth 校验 | `src-tauri/src/proxy/forwarder.rs` | `validate_codex_official_authorization` (57-99) |
| 原子写入 | `src-tauri/src/config.rs` | `atomic_write_with_unix_mode` (336-498) |
| SQL 导入 authorizer | `src-tauri/src/database/backup.rs` | `import_authorizer` (63-83) |
| WebDAV/S3 hook | `src-tauri/src/database/mod.rs` | `register_db_change_hook` (84-93) |
| MCP 验证抽象 | `src-tauri/src/mcp/validation.rs` | `validate_server_spec` (8-51) |
| Codex MCP 同步 | `src-tauri/src/mcp/codex.rs` | `sync_enabled_to_codex` (286-) |
| Hermes MCP 转换 | `src-tauri/src/mcp/hermes.rs` | `convert_to_hermes_format` (61-105) |
| DeepLink 模型 | `src-tauri/src/deeplink/mod.rs` | `DeepLinkImportRequest` (34-139) |
| Token 解析 | `src-tauri/src/proxy/usage/parser.rs` | `from_claude_response` (104-127) |
| 成本计算 | `src-tauri/src/proxy/usage/calculator.rs` | `calculate_for_app` (56-70) |
| 用量幂等写入 | `src-tauri/src/proxy/usage/logger.rs` | `log_request` (101-200) |
| Provider 数据结构 | `src-tauri/src/provider.rs` | `Provider` (10-44) |
| 凭据解析 | `src-tauri/src/provider.rs` | `resolve_usage_credentials` (135-198) |
| Schema 迁移 | `src-tauri/src/database/schema.rs` | `create_tables_on_conn` (24-300+) |
| 备份导出 | `src-tauri/src/database/backup.rs` | `export_sql_string` (118-121) |
| 备份导入 | `src-tauri/src/database/backup.rs` | `import_sql_string_inner_with_hook` (173-249) |
| Skills 命令入口 | `src-tauri/src/commands/skill.rs` | 14 个 `#[tauri::command]` (30-340) |
| 前端主壳 | `src/App.tsx` | View 类型 + STORAGE_KEY (116-150) |
| Zod schema | `src/lib/schemas/provider.ts` | `providerSchema` (38-58) |

---

## 11. 总结

cc-switch 是一个**生产级**的"CLI Agent 路由器 + 统一配置中心"项目，技术亮点集中在四个层面：

1. **代理层**（proxy/）—— 熔断器、整流器、模型映射、Hyper 手写 accept loop、OAuth 透传等都是教科书级的容错设计。
2. **持久层**（database/）—— SQL 导入 authorizer 防御、双向同步的 SYNC_SKIP/SYNC_PRESERVE 表设计、pre-migration 备份、incremental vacuum 都是工程化的体现。
3. **跨工具适配**（mcp/）—— 把 8 种 CLI 的 MCP 配置抽象为统一 McpServer + 6 个 enabled 字段，再按 per-tool adapter 投影。
4. **安全**——atomic_write、authorizer、validate_repo_ref、usage_script 默认禁用等细节展示了"信任边界外层加厚"的工程文化。

对 laew 而言，**最有借鉴价值的是前 5 个 P0/P1 项**（熔断器、中性释放、整流器、错误分类、原子写入），它们能在不改变 laew 核心架构的前提下显著提升生产稳定性。P2 项（HTTP 反向代理 / MCP 多工具 / DeepLink / Skills / 多设备同步）属于"产品演进方向"，可作为 laew v2.x 的中长期规划。
