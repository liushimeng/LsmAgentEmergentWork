# OpenClaw 第三轮深挖: custodian-skills / deploy / 多渠道 / 边缘模块

> 日期: 2026-09-05
> 定位: 第二轮深度分析的补全,覆盖 custodian-skills / deploy / apps / extensions / git-hooks / 遗漏包
> 前置文档: openclaw-源码调研.md + openclaw-深度分析.md + openclaw-第二轮深度分析.md

---

## 一、未覆盖目录总览

第二轮已覆盖的核心模块(Gateway / Harness / Adapter / Lane / Workshop / 双向 MCP / Quarantine / 插件生命周期 / secret 隔离)不再重复,以下为本轮新增覆盖:

| 目录 | 行数(估) | 核心职责 | 第二轮状态 |
|------|----------|----------|-----------|
| `custodian-skills/` | 4 SKILL.md | 托管运维技能(Add Provider / Channel / Diagnose / Cloud Image) | 未覆盖 |
| `deploy/` | 1 file | Fly.io 私有部署模板 | 未覆盖 |
| `apps/` | 9 子目录 | 多平台客户端(iOS / Android / Linux-Tauri / macOS / Swabble / shared) | 未覆盖 |
| `extensions/` | 153 个插件 | 全量插件目录(24 channel + 2 memory + 2 provider + 其余功能) | 部分覆盖 |
| `git-hooks/` | pre-commit | 内容守卫 + 格式化 + 密钥检测 | 未覆盖 |
| `config/` | 18 files | 质量基线(断言行数/测试耗时/环境变量预算/knip/lint) | 未覆盖 |
| `examples/` | 1 example | `@openclaw/ai` 最小消费示例 | 未覆盖 |
| `packages/` | 23 包 | 第二轮仅覆盖核心包,遗漏: acp-core / net-policy / tool-call-repair / memory-host-sdk / workboard-contract / session-url-contract | 部分遗漏 |
| `src/` | 121 子模块 | 第二轮覆盖核心,遗漏: cron / fleet / trajectory / boards / auto-reply / link-understanding / polls | 部分遗漏 |

---

## 二、custodian-skills 深挖

### 2.1 概述

`custodian-skills/` 是 OpenClaw 的**托管运维技能目录**,包含 4 个 SKILL.md 文件,每个都是一个结构化的运维 Playbook。与 `skills/` 目录下的 52 个用户态技能不同,custodian-skills 专为**运维操作员**(而非终端用户)设计,覆盖 OpenClaw 部署生命周期的核心运维场景。

`custodian-skills/` 目录结构:
```
custodian-skills/
  add-model-provider/SKILL.md    -- 添加模型提供商
  cloud-image-bake/SKILL.md      -- 烘焙云 Worker 镜像
  configure-channel/SKILL.md     -- 配置聊天渠道
  diagnose-gateway/SKILL.md      -- 诊断 Gateway
```

### 2.2 技能格式: SKILL.md Front Matter + Phase 结构

每个 custodian-skill 遵循统一格式:

**Front Matter**(YAML):
```yaml
---
name: add-model-provider
description: Add and live-prove a model provider with non-interactive config one-liners, without exposing credentials.
---
```

**正文**: 固定 5 阶段结构(Gather → Mutate → Repair → Prove → Report):

```markdown
# Add a model provider

Never print or persist secret values; credentials enter config only as SecretRefs...
Every run ends with the observable Prove result or an exact explanation...

## Gather
openclaw config get models --json
openclaw models list --agent <agentId>
...

## Mutate
openclaw config set secrets.providers.openai_key_file --provider-source file ...

## Repair
openclaw doctor --lint

## Prove
openclaw agent --agent <agentId> --model openai/gpt-5.4 -m "Reply with exactly: PROVIDER-PROOF-OK"

## Report
State the provider added, the SecretRef path written (never the value)...
```

> 文件: `custodian-skills/add-model-provider/SKILL.md:1-72`

### 2.3 四个技能详解

#### add-model-provider

核心模式: **SecretRef 模式** -- 凭证永不明文进入配置,只通过 `--ref-provider` / `--ref-source` / `--ref-id` 三元组引用环境变量或文件源:

```bash
openclaw config set models.providers.openai.apiKey --ref-provider openai_key_file --ref-source file --ref-id value
```

支持两种认证契约: API-key providers 用 `SecretRef`; OAuth/订阅 providers 用 `openclaw models auth login`。

> 文件: `custodian-skills/add-model-provider/SKILL.md:28-34`

#### configure-channel

渠道配置的 SecretRef 模式:

```bash
openclaw config set channels.telegram.botToken --ref-provider default --ref-source env --ref-id TELEGRAM_BOT_TOKEN
```

核心安全约束: allowFrom 使用 numeric Telegram user ID(非 phone/username/chat ID); `dmPolicy=pairing` 时通过 `openclaw logs --follow` 读 `senderUserId` 自动发现。

> 文件: `custodian-skills/configure-channel/SKILL.md:29-38`

#### cloud-image-bake

云 Worker 镜像烘焙流程,支持三种后端:
- **AWS**: `crabbox checkpoint create` + `crabbox image promote`
- **Hetzner**: `hcloud image create --type snapshot`
- **Firecracker**: 通过 host 模板管道重建 rootfs

关键约束: 旧镜像保留直到证明通过;删除需硬确认。

> 文件: `custodian-skills/cloud-image-bake/SKILL.md:31-43`

#### diagnose-gateway

**纯只读**诊断 Playbook -- 不写配置、不重启服务、不 `doctor --fix`:

```bash
openclaw doctor --lint
openclaw gateway status --deep
openclaw config validate
openclaw channels status
openclaw models status
```

检查签名: 无效配置/schema 错误、退化 SecretRef owners、过期渠道认证、EADDRINUSE、crash loops。

> 文件: `custodian-skills/diagnose-gateway/SKILL.md:10-19`

### 2.4 custodian-skills 与 skills/ 的关系

| 维度 | `skills/`(52 个) | `custodian-skills/`(4 个) |
|------|-----------------|--------------------------|
| 目标用户 | 终端用户 / AI Agent | 运维操作员 |
| 格式 | SKILL.md front matter + 指令 | 同格式,但 5 阶段固定结构 |
| 注册 | 通过 `openclaw.plugins.json` skills 字段 | 通过 `custodianSkills` 配置项 |
| 执行 | Agent 在对话中触发 | 运维场景触发(部署/配置/诊断) |
| 安全约束 | 由具体技能定义 | 全局: SecretRef 不明文、doctor --lint 先行、Prove 步骤必有 |

### 2.5 对 laew 的借鉴

1. **SKILL.md 格式标准化**: laew 的 Skill 系统可采用 front matter + 正文结构,支持 `name` / `description` / `metadata.requires` / `metadata.install`
2. **5 阶段运维 Playbook**: Gather → Mutate → Repair → Prove → Report 是通用运维模板,laew 的 provider add/list/use/delete 可包装为类似 Playbook
3. **SecretRef 模式**: laew 当前 api_key 明文存储在 SQLite,可引入 SecretRef(环境变量 / 文件源引用)
4. **Prove 步骤**: 每次配置变更后自动验证(laew 可在 provider use 后自动发一条测试请求)

---

## 三、deploy 部署架构

### 3.1 部署目标总览

OpenClaw 支持 4 种部署目标:

| 目标 | 配置文件 | 特点 |
|------|---------|------|
| Fly.io 公网 | `fly.toml` | 公网 ingress + health check + auto-start |
| Fly.io 私有 | `deploy/fly.private.toml` | 无公网 IP,仅 fly proxy / WireGuard 访问 |
| Docker Compose | `docker-compose.yml` | Gateway + CLI 双容器,共享网络命名空间 |
| Render | `render.yaml` | Docker runtime + 持久化磁盘 |

### 3.2 Dockerfile: 7 阶段多构建

Dockerfile 是部署架构的核心,采用**7 阶段多构建**:

```dockerfile
# Stage 1: workspace-deps -- 提取 package.json,插件选择
# Stage 2: dependency-inputs -- 复制锁文件 + 补丁
# Stage 3: production-deps -- 生产依赖安装(--prod)
# Stage 4: build -- Bun 二进制 + 全量构建 + UI 构建
# Stage 5: runtime-build-output -- 删除 node_modules 后的构建产物
# Stage 6: runtime-assets -- 生产依赖 + 构建产物合并
# Stage 7: base-runtime → 最终镜像(bookworm-slim)
```

> 文件: `Dockerfile:30-436`

关键设计:

**插件选择**: 通过 `OPENCLAW_EXTENSIONS` build arg 按需选择插件:

```dockerfile
ARG OPENCLAW_EXTENSIONS=""
# docker build --build-arg OPENCLAW_EXTENSIONS="diagnostics-otel,matrix" .
```

`scripts/lib/docker-plugin-selection.mjs` 负责解析依赖图,只安装选中插件的依赖。

> 文件: `Dockerfile:11-13`

**安全加固**:
- 非 root 用户运行(`USER node`,uid 1000)
- `cap_drop: NET_RAW, NET_ADMIN`
- `security_opt: no-new-privileges:true`
- Docker CLI GPG 指纹验证(Dockerfile:356-388)
- 目录权限校验(stat 验证 700/755)

> 文件: `Dockerfile:411-414`, `docker-compose.yml:61-65`

**版本校验**: 运行时验证三处版本一致:

```dockerfile
RUN test "$(node -p "require('/app/package.json').version")" = "$OPENCLAW_DOCKER_BUILD_VERSION"
RUN test "$(node -p "require('/app/dist/build-info.json').version")" = "$OPENCLAW_DOCKER_BUILD_VERSION"
RUN test "$(node /app/openclaw.mjs --version | cut -d ' ' -f 2)" = "$OPENCLAW_DOCKER_BUILD_VERSION"
```

> 文件: `Dockerfile:279-284`

### 3.3 Docker Compose: Gateway + CLI 双容器

```yaml
services:
  openclaw-gateway:
    ports:
      - "${OPENCLAW_GATEWAY_PORT:-18789}:18789"
      - "${OPENCLAW_BRIDGE_PORT:-18790}:18790"
      - "${OPENCLAW_MSTEAMS_PORT:-3978}:3978"
    command: ["node", "dist/index.js", "gateway", "--bind", "lan", "--port", "18789"]
    healthcheck:
      test: ["CMD", "node", "dist/docker-healthcheck.js"]

  openclaw-cli:
    network_mode: "service:openclaw-gateway"  # 共享网络命名空间
    stdin_open: true
    tty: true
    entrypoint: ["node", "dist/index.js"]
    depends_on:
      - openclaw-gateway
```

> 文件: `docker-compose.yml:1-136`

三端口设计: 18789(Gateway WS) + 18790(Bridge) + 3978(MS Teams webhook)。

### 3.4 Fly.io: 公网 vs 私有

**公网模式**(`fly.toml`):
```toml
[http_service]
internal_port = 3000
force_https = true
auto_stop_machines = false  # 持久连接不自动停止
auto_start_machines = true
min_machines_running = 1

[[http_service.checks]]
path = "/startupz"
```

> 文件: `fly.toml:18-27`

**私有模式**(`deploy/fly.private.toml`):
```toml
# 无 [http_service] 块 = 无公网 ingress
# 仅通过 fly proxy / WireGuard / fly ssh 访问
```

> 文件: `deploy/fly.private.toml:28`

两种模式都使用 `shared-cpu-2x` + `2048mb` + `/data` 持久化卷。

### 3.5 Render 部署

```yaml
services:
  - type: web
    runtime: docker
    plan: starter
    dockerCommand: node openclaw.mjs gateway --allow-unconfigured
    healthCheckPath: /startupz
    disk:
      name: openclaw-data
      mountPath: /data
      sizeGB: 1
```

> 文件: `render.yaml:1-19`

### 3.6 对 laew 的借鉴

1. **多阶段 Docker 构建**: laew 当前单阶段 `cargo build --release`,可分离构建/运行阶段减小镜像
2. **插件选择构建**: `OPENCLAW_EXTENSIONS` 按需选择模式可借鉴到 laew 的 cargo feature gating
3. **健康检查三端点**: `/healthz`(存活性) + `/startupz`(启动准入) + `/readyz`(就绪性) 是标准模式
4. **公私分离部署**: Fly.io 公网/私有双模板是零成本安全加固
5. **版本三处校验**: package.json / build-info.json / CLI --version 一致性校验防止发布漂移

---

## 四、apps 多渠道客户端

### 4.1 平台矩阵

```
apps/
  android/       -- Kotlin/Gradle, Wear OS 支持, fastlane 发布
  ios/           -- Swift/XcodeGen, WatchApp + ShareExtension + ActivityWidget
  linux/         -- Tauri(Rust), 系统托盘 + 快捷聊天 + Gateway 管理
  macos/         -- Swift Package, native macOS 客户端
  macos-mlx-tts/ -- Swift Package, MLX 本地 TTS
  mobile/        -- version.json 共享版本
  shared/        -- OpenClawKit(Swift) + OpenClawMLXTTSProtocol + mermaid
  swabble/       -- Swift Package, 独立聊天 UI 库
  .i18n/         -- 国际化资源(native + native-source.json)
```

### 4.2 Linux Tauri 客户端: Rust 实现

`apps/linux/src-tauri/src/main.rs` 是 Tauri 2 桌面客户端,模块清单:

```rust
mod cli;
mod discovery;
mod gateway;
mod gateway_device_identity;
mod gateway_operation_queue;
mod gateway_sleep;         // Linux logind 睡眠监听
mod gateway_ws;            // WebSocket 连接
mod installer;
mod notify;
mod operation_executor;
mod pending_approvals;
mod quickchat;             // 快捷聊天
mod quickchat_widgets;
mod remote_gateway;
mod tray;                  // 系统托盘
mod updater;
```

> 文件: `apps/linux/src-tauri/src/main.rs:1-22`

关键设计:
- `gateway_sleep_logind` -- 监听 Linux logind 睡眠/唤醒事件,自动暂停/恢复 Gateway 连接
- `pending_approvals` -- 待审批通知管理
- `quickchat` + `quickchat_widgets` -- 桌面快捷聊天浮窗
- `remote_gateway` -- 远程 Gateway 代理请求

### 4.3 iOS 客户端: 多 Extension 架构

```
ios/Sources/
  Chat/          -- 聊天核心
  Gateway/       -- Gateway 连接
  Calendar/      -- 日历集成
  Camera/        -- 相机
  Contacts/      -- 联系人
  Desktop/       -- 桌面相关
  Device/        -- 设备管理
  EventKit/      -- 事件
  Health/        -- 健康数据
  ...
ios/ActivityWidget/   -- 锁屏/桌面小组件
ios/ShareExtension/   -- 系统分享扩展
ios/WatchApp/         -- Apple Watch
```

> 文件: `apps/ios/Sources/` 目录结构

### 4.4 apps/shared: 跨平台共享层

```
apps/shared/
  OpenClawKit/          -- Swift Package, 跨 iOS/macOS 共享
  OpenClawMLXTTSProtocol/ -- TTS 协议定义
  mermaid/              -- Mermaid 图表渲染
```

`OpenClawKit` 是 iOS/macOS 的共享 Swift 包,封装 Gateway 连接、消息模型、UI 组件等。

### 4.5 多渠道如何共享核心

核心共享路径:

```
packages/gateway-client/  -- WebSocket 客户端(TypeScript, Node/browser)
packages/gateway-protocol/ -- 协议 Schema(类型定义 + 校验)
src/gateway/              -- Gateway 服务端
extensions/               -- 24 个 channel 插件
apps/                     -- 原生客户端
```

- **协议层**: `gateway-protocol` 定义 WebSocket 帧格式,所有客户端共享
- **客户端层**: `gateway-client` 是 TypeScript 参考实现;原生客户端各自实现协议
- **渠道层**: 每个 channel(telegram/slack/discord/whatsapp/...) 是独立 extension,通过 `channel-plugin-api.ts` 接入

### 4.6 对 laew 的借鉴

1. **Tauri 桌面客户端**: laew 的 TUI 可升级为 Tauri 桌面应用(已有 Rust 基础),Linux Tauri 客户端是直接参考
2. **多平台共享层**: `apps/shared/OpenClawKit` 模式可借鉴到 laew 的未来 Web/桌面客户端
3. **Gateway 连接管理**: `gateway_sleep_logind` 睡眠/唤醒处理是桌面客户端的必备逻辑

---

## 五、extensions 扩展机制全貌

### 5.1 规模统计

OpenClaw 共 **153 个 extensions**,按类型分布:

| 类型 | 数量 | 代表 |
|------|------|------|
| Channel(聊天渠道) | 24 | telegram, slack, discord, whatsapp, signal, matrix, line, feishu, irc, nostr, msteams, imessage... |
| LLM Provider | ~60 | openai, anthropic, deepseek, google, cohere, groq, ollama, vllm, sglang, litellm... |
| Memory(记忆) | 3 | memory-core, memory-lancedb, active-memory |
| 工具/功能 | ~30 | browser, canvas, codex, diffs, file-transfer, firecrawl, tavily, workboard, voice-call, voice-call... |
| 协议 | 2 | a2a(Agent-to-Agent), acpx(ACP 扩展) |
| 基础设施 | ~10 | diagnostics-otel, diagnostics-prometheus, admin-http-rpc, policy, crabbox, device-pair... |
| 语音/媒体 | ~10 | elevenlabs, azure-speech, tts-local-cli, fish-audio-speech, talk-voice, video-generation... |

### 5.2 Extension 统一契约

每个 extension 遵循 `openclaw.plugin.json` + `index.ts` 双文件契约:

**openclaw.plugin.json** 声明式元数据:
```json
{
  "id": "whatsapp",
  "doctorContract": { "configRepair": true, "stateMigrations": true },
  "activation": { "onStartup": false },
  "contracts": { "tools": ["whatsapp_call", "whatsapp_login"] },
  "channels": ["whatsapp"],
  "skills": ["./skills"],
  "configSchema": { ... }
}
```

> 文件: `extensions/whatsapp/openclaw.plugin.json:1-34`

**index.ts** 编程式入口,通过 `definePluginEntry` 或 `defineBundledChannelEntry` 注册:

```typescript
export default definePluginEntry({
  id: "codex",
  name: "Codex",
  register(api) {
    api.registerAgentHarness(createCodexAppServerAgentHarness(options));
    api.registerTool(createCodexThreadsTool, { name: "codex_threads" });
    api.registerCommand(createCodexCommand(options));
    api.on("inbound_claim", handler);
    api.on("session_end", handler);
  },
});
```

> 文件: `extensions/codex/index.ts:68-424`

### 5.3 Channel Extension 结构

以 WhatsApp 为例,一个 channel extension 包含:

```
extensions/whatsapp/
  openclaw.plugin.json     -- 声明 channels: ["whatsapp"]
  channel-plugin-api.ts    -- 渠道插件接口
  channel-config-api.ts    -- 渠道配置接口
  config-api.ts            -- 配置 API
  secret-contract-api.ts   -- 密钥管理
  security-contract-api.ts -- 安全策略
  contract-api.ts          -- 契约 API
  setup-entry.ts           -- 设置入口
  login-qr-api.ts          -- WhatsApp 特有: QR 码登录
  skills/                  -- 渠道专属技能
  src/                     -- 实现源码
```

> 文件: `extensions/whatsapp/` 目录结构

### 5.4 Memory Extension 双轨

OpenClaw 有 3 个记忆扩展,形成**存储 + 检索 + 主动记忆**三层:

1. **memory-core**(`kind: "memory"`): 核心记忆存储,提供 `memory_get` / `memory_search` / `intent` 三工具,支持 Dreaming(梦境式记忆整合)
2. **memory-lancedb**(`kind: "memory"`): LanceDB 向量存储后端
3. **active-memory**: 主动记忆检索,在 `before_prompt_build` hook 中自动注入相关记忆

**active-memory 核心流程**:
```
before_prompt_build hook
  → 检查 toolAuthority / session 策略 / agent 策略
  → Lane-1: deterministic trigger recall(快速精确匹配)
  → Lane-2: blocking memory recall(完整子 Agent 检索)
  → 拼接上下文返回 prependContext
```

> 文件: `extensions/active-memory/index.ts:240-553`

**memory-core Dreaming** 机制:

memory-core 支持"梦境式"记忆整合,三阶段:
- **light**: 去重相似条目(dedupeSimilarity 阈值)
- **REM**: 模式识别(minPatternStrength)
- **deep**: 短期记忆提升到 MEMORY.md(maxPromotedSnippetTokens / maxPriorEntryLossFraction)

```json
"dreaming": {
  "enabled": true,
  "frequency": "0 3 * * *",
  "phases": {
    "light": { "enabled": true, "lookbackDays": 7, "dedupeSimilarity": 0.85 },
    "rem": { "enabled": true, "lookbackDays": 14, "minPatternStrength": 0.6 },
    "deep": { "enabled": true, "maxPriorEntryLossFraction": 0.25 }
  }
}
```

> 文件: `extensions/memory-core/openclaw.plugin.json:58-120`

### 5.5 workboard: 最大工具集 Extension

workboard 是 tools 最多的 extension(35 个工具),实现**任务看板系统**:

```
workboard_list, workboard_create, workboard_link, workboard_read,
workboard_claim, workboard_heartbeat, workboard_complete,
workboard_specify, workboard_decompose, workboard_dispatch,
workboard_proof, workboard_protocol_violation, workboard_move, ...
```

支持: 任务创建/认领/心跳/完成/证明/分解/分派/阻止/移动/通知订阅。

> 文件: `extensions/workboard/openclaw.plugin.json` contracts.tools

### 5.6 a2a / acpx: 协议扩展

**a2a**: Agent-to-Agent v1.0 协议 channel 插件,实现 Google A2A 标准:

```typescript
export default defineBundledChannelEntry({
  id: "a2a",
  name: "A2A",
  description: "A2A v1.0 Agent-to-Agent protocol channel plugin",
  plugin: { specifier: "./channel-plugin-api.js", exportName: "a2aChannelPlugin" },
});
```

> 文件: `extensions/a2a/index.ts:1-16`

**acpx**: ACP(Anthropic Claude Protocol)扩展,嵌入式 ACP 运行时后端:

```typescript
register(api) {
  api.registerService(createAcpxRuntimeService(options));
  api.on("reply_dispatch", tryDispatchAcpReplyHookWithTimeout, { timeoutMs });
}
```

> 文件: `extensions/acpx/index.ts:47-68`

### 5.7 admin-http-rpc: 最小 Extension 示例

仅 4 行注册代码:

```typescript
export default definePluginEntry({
  id: "admin-http-rpc",
  name: "Admin HTTP RPC",
  register(api) {
    api.registerHttpRoute({
      path: "/api/v1/admin/rpc",
      auth: "gateway",
      match: "exact",
      gatewayRuntimeScopeSurface: "trusted-operator",
      handler: handleAdminHttpRpcRequest,
    });
  },
});
```

> 文件: `extensions/admin-http-rpc/index.ts:1-17`

### 5.8 对 laew 的借鉴

1. **插件声明式 + 编程式双契约**: `openclaw.plugin.json`(声明式元数据) + `index.ts`(编程式注册)是可借鉴的插件模式
2. **active-memory 双 Lane 检索**: deterministic trigger recall(快车道) + blocking memory recall(完整检索)是高效的记忆注入策略
3. **Dreaming 记忆整合**: light/REM/deep 三阶段"睡眠式"记忆整合是长期记忆管理的创新模式
4. **workboard 任务看板**: 35 工具的任务看板系统可作为 laew 的 Workflow 执行参考
5. **a2a 协议**: Agent-to-Agent 标准化协议是多 Agent 协作的未来方向

---

## 六、git-hooks 与质量门

### 6.1 pre-commit 内容守卫

`git-hooks/pre-commit` 调用 `scripts/pre-commit/guard-staged-content.mjs`,实现**两阶段守卫**:

```javascript
// 阶段 1: 扫描阶段文件中的被阻断字面量
scan();  // git grep --cached --fixed-strings -f blocked-literals.txt
// 阶段 2: 格式化
const formatted = spawnSync("bash", ["scripts/pre-commit/format-staged.sh"]);
// 阶段 3: 重新扫描(格式化可能引入新内容)
scan();
```

> 文件: `scripts/pre-commit/guard-staged-content.mjs:170-189`

被阻断字面量从 `git config hooks.blockedLiteralsFile` 指向的私有文件加载(不入库),匹配时输出 `[REDACTED]` 脱敏。

### 6.2 .pre-commit-config.yaml: 多层质量门

```yaml
hooks:
  # 基础文件卫生
  - trailing-whitespace / end-of-file-fixer / check-yaml / check-added-large-files(500KB)

  # Shell 脚本 lint
  - shellcheck --severity=error

  # GitHub Actions 安全审计
  - actionlint
  - zizmor --min-severity=medium  # Actions 安全扫描

  # Python(skills 脚本)
  - ruff --config skills/pyproject.toml
  - pytest -c skills/pyproject.toml

  # 项目级检查
  - detect-private-key    # 私钥检测
  - pnpm-audit-prod       # 生产依赖安全审计
  - oxlint --type-aware   # TypeScript lint
  - oxfmt --check          # TypeScript 格式化
  - swiftlint              # Swift lint
  - swiftformat --lint     # Swift 格式化
```

> 文件: `.pre-commit-config.yaml:1-90`

### 6.3 scripts/pre-commit/ 工具集

```
scripts/pre-commit/
  guard-staged-content.mjs   -- 内容守卫(阻断字面量)
  filter-staged-files.mjs    -- 文件过滤
  format-staged.sh           -- 格式化已暂存文件
  pnpm-audit-prod.mjs        -- pnpm 生产依赖审计
  run-node-tool.sh           -- Node 工具运行包装
```

### 6.4 对 laew 的借鉴

1. **内容守卫**: 阻断字面量模式(私钥/token/密码)可直接用于 laew 的 pre-commit
2. **多层质量门**: lint + format + audit + 私钥检测的组合是 CI 前置质量门的最佳实践
3. **REDACTED 脱敏**: pre-commit hook 中对敏感内容的自动脱敏输出

---

## 七、遗漏包补全

### 7.1 packages/acp-core: ACP 会话核心

ACP(Anthropic Claude Protocol)会话管理核心包,提供:

```typescript
export type AcpSessionStore = {
  createSession: (params) => AcpSession;
  hasSession: (sessionId) => boolean;
  getSession: (sessionId) => AcpSession | undefined;
  setActiveRun: (sessionId, runId, abortController) => void;
  cancelActiveRun: (sessionId, expectedRunId?) => boolean;
};
```

默认 5000 session 上限,24 小时空闲 TTL。支持 runId 到 sessionId 的反向索引。

> 文件: `packages/acp-core/src/session.ts:1-60`

### 7.2 packages/net-policy: SSRF 防护

网络策略包,实现 IP 地址解析 + 特殊用途地址阻断 + URL 脱敏:

```typescript
const BLOCKED_IPV4_SPECIAL_USE_RANGES = new Set([
  "unspecified", "broadcast", "multicast", "linkLocal",
  "loopback", "carrierGradeNat", "private", "reserved",
]);
const CLOUD_METADATA_IP_ADDRESSES = new Set(["100.100.100.200", "fd00:ec2::254"]);
```

阻断 RFC2544 基准范围 + 云元数据地址(防 SSRF)。

> 文件: `packages/net-policy/src/ip.ts:36-67`

### 7.3 packages/tool-call-repair: 工具调用修复

修复 LLM 输出的纯文本工具调用(非标准 JSON 格式),支持三种语法:

```typescript
type PlainTextJsonToolCallSyntax = "harmony" | "named-bracket" | "tool-bracket";
```

- **Harmony**: `CHANNEL[tool_name]` 标记
- **named-bracket**: `[tool_name]{...}` 语法
- **tool-bracket**: `<tool_name>{...}` XML 风格

最大 256KB 载荷,120 字符工具名限制。

> 文件: `packages/tool-call-repair/src/payload.ts:1-50`

### 7.4 packages/gateway-client: WebSocket 客户端

参考 WebSocket 客户端,实现:
- 设备认证(Device Auth V3 + Ed25519 签名)
- TLS 指纹规范化
- 连接认证选择(gateway token / device token / Cloudflare Access)
- 协议版本协商(`MIN_CLIENT_PROTOCOL_VERSION` / `PROTOCOL_VERSION`)
- 重连策略(指数退避 + 暂停策略)

> 文件: `packages/gateway-client/src/client.ts:1-80`

### 7.5 packages/agent-core/src/agent-loop.ts: Agent 循环核心

Agent 循环实现,核心模式:

```typescript
while (true) {
  // 外层: 处理 follow-up 消息
  while (hasMoreToolCalls || pendingMessages.length > 0) {
    // 内层: 工具调用 + steering 消息
    const message = await streamAssistantResponse(context, config, signal, emit);
    if (message.stopReason === "toolUse") {
      const batch = await executeToolCalls(context, message, config, signal, emit);
      hasMoreToolCalls = !batch.terminate;
      pendingMessages = batch.steringMessages;
    }
  }
  pendingMessages = await config.getFollowUpMessages?.();
  if (pendingMessages.length === 0) break;
}
```

> 文件: `packages/agent-core/src/agent-loop.ts:295-530`

关键特性:
- **双层 while**: 外层处理 follow-up 消息,内层处理工具调用
- **steering 注入**: 用户排队消息可在工具执行间注入
- **并行/串行工具执行**: 按 `executionMode` 决定
- **工具循环恢复**: `toolLoopRecoveryState.criticalToolLoopSeen` 防止无限循环
- **turn taint 追踪**: 标记工具结果是否"污染"了当前 turn

### 7.6 src/cron: 定时任务系统

```typescript
type CronActiveJobMarker = {
  jobId: string;
  generation: number;
  token: number;
  cancellation?: { kind: "bound"; cancel } | { kind: "requested"; reason };
  scheduleMutated?: true;
  triggerMutated?: true;
};
```

全局单例 `Map<string, CronActiveJobMarker>` 追踪活跃任务,generation 机制防止重复执行。

> 文件: `src/cron/active-jobs.ts:1-55`

### 7.7 src/fleet: 容器舰队管理

```typescript
type FleetCellRecord = {
  tenantId: string;
  image: string;
  runtime: "docker" | "podman";
  hostPort: number;
  containerName: string;
  dataDir: string;
};

type FleetCellOperationName =
  "create" | "start" | "stop" | "restart" | "upgrade" | "backup" | "restore" | "rm";
```

SQLite 持久化 + Kysely 查询 + 操作租约(5 分钟 TTL)防并发冲突。

> 文件: `src/fleet/registry.ts:1-60`

### 7.8 src/trajectory: 执行轨迹记录

轨迹(trajectory)系统,SQLite 存储 + 文件导出:

```
src/trajectory/
  runtime-store.sqlite.ts  -- SQLite 存储
  export.ts                -- 导出
  metadata.ts              -- 元数据
  cleanup.ts               -- 清理
  command-export.ts        -- 命令导出
```

> 文件: `src/trajectory/` 目录结构

### 7.9 src/boards: 看板系统

Board 系统,SQLite 存储:

```typescript
src/boards/
  board-store.ts           -- Board 存储
  board-layout.ts          -- 布局管理
  board-capabilities.ts    -- 能力声明
  sqlite-board-store.ts    -- SQLite 实现
  sqlite-board-codec.ts    -- 编解码
  github-actions-capability.ts -- GitHub Actions 集成
```

> 文件: `src/boards/` 目录结构

### 7.10 packages/memory-host-sdk: 记忆宿主 SDK

```
packages/memory-host-sdk/src/
  engine-embeddings.ts     -- 嵌入引擎
  engine-foundation.ts     -- 基础引擎
  engine-sessions.ts       -- 会话引擎
  engine-storage.ts        -- 存储引擎
  query.ts                 -- 查询
  runtime-core.ts          -- 运行时核心
  secret.ts                -- 密钥管理
```

> 文件: `packages/memory-host-sdk/src/` 目录结构

### 7.11 对 laew 的借鉴

1. **tool-call-repair**: laew 的 LLM 输出解析可引入纯文本工具调用修复(Harmony/named-bracket/XML 语法)
2. **net-policy SSRF 防护**: 阻断特殊用途 IP + 云元数据地址是 Web 工具调用的必备安全层
3. **fleet 容器管理**: 操作租约(5 分钟 TTL)防并发是分布式任务执行的关键模式
4. **cron generation 机制**: 通过 generation 号防止定时任务重复执行

---

## 八、对 laew 借鉴路线总览

### P0(立即可做)

| 借鉴 | 来源 | laew 现状 | 行动 |
|------|------|----------|------|
| SKILL.md 格式 | custodian-skills/ | 无 Skill 系统 | 定义 front matter + 指令正文格式 |
| SecretRef 模式 | custodian-skills/ | api_key 明文存 SQLite | 引入环境变量/文件源引用 |
| 内容守卫 | git-hooks/ | 无 pre-commit | 添加阻断字面量扫描 |
| 健康检查端点 | deploy/ | 无 HTTP 端点 | Gateway 模式下添加 /healthz |

### P1(短期)

| 借鉴 | 来源 | 行动 |
|------|------|------|
| active-memory 双 Lane 检索 | extensions/active-memory/ | deterministic recall + full recall 双层 |
| 多阶段 Docker 构建 | Dockerfile | 分离构建/运行阶段 |
| net-policy SSRF 防护 | packages/net-policy/ | Bash 工具添加 IP/URL 安全检查 |
| 工具调用修复 | packages/tool-call-repair/ | 支持纯文本工具调用解析 |

### P2(中期)

| 借鉴 | 来源 | 行动 |
|------|------|------|
| Dreaming 记忆整合 | extensions/memory-core/ | 三阶段记忆整合机制 |
| workboard 任务看板 | extensions/workboard/ | 任务看板系统 |
| Tauri 桌面客户端 | apps/linux/ | laew 升级为桌面应用 |
| fleet 容器管理 | src/fleet/ | 沙箱化任务执行 |

### P3(长期)

| 借鉴 | 来源 | 行动 |
|------|------|------|
| a2a 协议 | extensions/a2a/ | Agent-to-Agent 标准化 |
| 多平台客户端矩阵 | apps/ | iOS/Android/macOS 客户端 |
| ACP 协议支持 | extensions/acpx/ | Anthropic Claude Protocol 集成 |

---

## 九、关键数据汇总

| 指标 | 数值 |
|------|------|
| extensions 总数 | 153 |
| Channel extensions | 24 |
| LLM Provider extensions | ~60 |
| Memory extensions | 3 |
| skills/ 技能数 | 52 |
| custodian-skills/ 技能数 | 4 |
| packages/ 包数 | 23 |
| src/ 子模块数 | 121 |
| apps/ 平台数 | 9 |
| Dockerfile 构建阶段 | 7 |
| 部署目标 | 4(Fly.io 公网/私有 + Docker Compose + Render) |
| pre-commit hooks | 12+ |
| workboard 工具数 | 35 |
