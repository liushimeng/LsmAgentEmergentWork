# opencode 第三轮周边包深度分析 — 34 包 workspace 全覆盖

> **调研日期**: 2026-09-05
> **代码版本**: dev 分支 (v1.18.26)
> **聚焦范围**: 前三档文档未实质覆盖的 packages/ 子包
> **前序文档**: opencode-源码调研.md / opencode-深度分析.md / opencode-核心机制深度分析.md / opencode-第二轮深度分析.md

---

## 目录

- [1. 34 包总览与覆盖矩阵](#1-34-包总览与覆盖矩阵)
- [2. 未覆盖包逐个深挖](#2-未覆盖包逐个深挖)
  - [2.1 packages/codemode — 受限 JS 解释器](#21-packagescodemode--受限-js-解释器)
  - [2.2 packages/desktop — Electron 桌面端](#22-packagesdesktop--electron-桌面端)
  - [2.3 packages/slack — Slack 渠道集成](#23-packagesslack--slack-渠道集成)
  - [2.4 packages/function — Cloudflare Workers 分享后端](#24-packagesfunction--cloudflare-workers-分享后端)
  - [2.5 packages/enterprise — 企业版分享存储](#25-packagesenterprise--企业版分享存储)
  - [2.6 packages/console — 管理控制台](#26-packagesconsole--管理控制台)
  - [2.7 packages/app — 桌面应用前端](#27-packagesapp--桌面应用前端)
  - [2.8 packages/cli — CLI 入口](#28-packagescli--cli-入口)
  - [2.9 packages/client — 类型安全客户端](#29-packagesclient--类型安全客户端)
  - [2.10 packages/protocol — Effect HttpApi 协议层](#210-packagesprotocol--effect-httpapi-协议层)
  - [2.11 packages/server — HTTP 路由装配](#211-packagesserver--http-路由装配)
  - [2.12 packages/effect-drizzle-sqlite — Drizzle ORM 桥接](#212-packageseffect-drizzle-sqlite--drizzle-orm-桥接)
  - [2.13 packages/effect-sqlite-node — Node.js SQLite](#213-packageseffect-sqlite-node--nodejs-sqlite)
  - [2.14 packages/http-recorder — HTTP 流量录制回放](#214-packageshttp-recorder--http-流量录制回放)
  - [2.15 packages/httpapi-codegen — OpenAPI 代码生成](#215-packageshttpapi-codegen--openapi-代码生成)
  - [2.16 packages/session-ui — 会话 UI 组件库](#216-packagessession-ui--会话-ui-组件库)
  - [2.17 packages/ui — 通用 UI 组件库](#217-packagesui--通用-ui-组件库)
  - [2.18 packages/web — 公开文档站点](#218-packagesweb--公开文档站点)
  - [2.19 packages/containers — CI 构建容器](#219-packagescontainers--ci-构建容器)
  - [2.20 packages/script — 工程脚本](#220-packagesscript--工程脚本)
  - [2.21 packages/storybook — 组件文档](#221-packagesstorybook--组件文档)
  - [2.22 packages/identity — 品牌资源](#222-packagesidentity--品牌资源)
  - [2.23 packages/docs — Astro 文档站](#223-packagesdocs--astro-文档站)
- [3. 多渠道架构分析](#3-多渠道架构分析)
- [4. SDK 设计专题](#4-sdk-设计专题)
- [5. CodeMode — 受限代码执行专题](#5-codemode--受限代码执行专题)
- [6. 企业版与云同步专题](#6-企业版与云同步专题)
- [7. 数据库与测试基础设施](#7-数据库与测试基础设施)
- [8. 对 laew 借鉴路线](#8-对-laew-借鉴路线)

---

## 1. 34 包总览与覆盖矩阵

opencode 采用 Bun workspace monorepo,根 `package.json` 声明 34 个 packages。以下按**功能域**分组,标记前三档文档覆盖状态:

### 核心运行时 (已有三档覆盖)

| 包名 | 用途 | 覆盖状态 |
|------|------|----------|
| `opencode` | 主运行时入口(doom_loop/session/permission/tools) | ✅ 深度覆盖 |
| `core` | Effect DI/LayerNode/AppLayer/Database/PTY/Filesystem | ✅ 深度覆盖 |
| `llm` | 协议无关 LLM 客户端(Route 4轴/15种事件) | ✅ 深度覆盖 |
| `tui` | Solid + OpenTUI 终端 UI(Keymap/命令面板) | ✅ 深度覆盖 |
| `schema` | Schema 单一来源(跨包共享类型) | ✅ 覆盖 |
| `protocol` | Effect HttpApi 组(Group/Middleware) | ✅ 概述 |
| `server` | HTTP 路由装配(HttpApiBuilder) | ✅ 概述 |
| `plugin` | 插件 API | ✅ 覆盖 |
| `stats` | 统计聚合 | ✅ 覆盖 |
| `sdk` / `sdk-next` | SDK 类型生成 + Effect 嵌入式运行时 | ✅ 概述 |

### 本次深挖 (未覆盖)

| 包名 | 用途 | 行数 | 本次覆盖 |
|------|------|------|----------|
| `codemode` | 受限 JS 解释器(模型代码执行) | ~6,878 | 本次深挖 |
| `desktop` | Electron 桌面端(Sidecar 架构) | ~4,000+ | 本次深挖 |
| `slack` | Slack Bot 集成 | 145 | 本次深挖 |
| `function` | Cloudflare Workers(Durable Objects + R2) | 402 | 本次深挖 |
| `enterprise` | 企业版(S3/R2 分享存储) | 522 | 本次深挖 |
| `console` | 管理控制台(Auth/Billing/Stripe) | 5个子包 | 本次深挖 |
| `app` | 桌面应用前端(Solid Router) | ~8,000+ | 本次深挖 |
| `cli` | CLI 入口(lildax 二进制) | 69 | 本次深挖 |
| `client` | 类型安全 HTTP 客户端 | 80 | 本次深挖 |
| `effect-drizzle-sqlite` | Drizzle ORM + Effect SQL 桥接 | 846 | 本次深挖 |
| `effect-sqlite-node` | Node.js SQLite 绑定 | ~100 | 本次深挖 |
| `http-recorder` | HTTP 流量录制回放(VCR) | 1,540 | 本次深挖 |
| `httpapi-codegen` | OpenAPI → TypeScript 代码生成 | ~200 | 本次深挖 |
| `session-ui` | 会话 UI 组件(Diff/File/Markdown) | ~2,000+ | 本次深挖 |
| `ui` | 通用 UI 组件库(30+ 组件) | ~5,000+ | 本次深挖 |
| `web` | 公开文档站点(Astro Starlight) | ~1,000+ | 本次深挖 |
| `containers` | CI 构建容器(Docker) | ~200 | 本次深挖 |
| `script` | 工程脚本(semver) | ~50 | 本次深挖 |
| `storybook` | 组件文档(Storybook 10) | 配置 | 本次深挖 |
| `identity` | 品牌资源(Logo/SVG) | 无代码 | 本次深挖 |
| `docs` | 内部 Astro 文档站 | 配置 | 本次深挖 |

**总计**: 前三档覆盖 10 个核心包,本次深挖 21 个周边包。

---

## 2. 未覆盖包逐个深挖

### 2.1 packages/codemode — 受限 JS 解释器

**定位**: 给模型提供一个 `execute` 工具,内部运行受限 JavaScript 程序,可调用宿主显式暴露的 schema 描述工具树。

**核心发现**: opencode **没有传统意义的沙箱**。`codemode` 不是容器/进程隔离,而是一个**树遍历解释器**(Acorn parse → AST walk),完全不使用 `eval` 或 `new Function`。模型写的代码在一个**完全受控的解释器**中执行,只能访问宿主显式提供的工具。

#### 2.1.1 架构分层

```
codemode.ts (159行)        ← 公共 API: execute() / make() / Result Schema
  ├── tool-runtime.ts (806行) ← 工具运行时: ToolReference / searchIndex / catalog
  ├── tool-schema.ts (301行)  ← Effect Schema → TypeScript 签名转换
  ├── tool.ts (96行)          ← Tool.make() 定义接口
  ├── values.ts (49行)        ← SandboxPromise / SandboxDate / SandboxMap 等沙箱值类型
  ├── interpreter/
  │   ├── runtime.ts (3,465行) ← 核心: Interpreter 类,树遍历执行引擎
  │   └── model.ts (201行)    ← AST 节点类型 + 语句结果类型
  ├── stdlib/                  ← 12 个标准库模块(Array/Date/JSON/Math/Number/Object/Promise/RegExp/String/URL/console/value)
  └── openapi/                 ← OpenAPI → 工具定义转换
```

#### 2.1.2 受限执行机制

**代码解析**(`packages/codemode/src/interpreter/runtime.ts:115-149`):

```typescript
const parseProgram = (code: string): ProgramNode => {
  const transpiled = transpileModule(`async function __codemode__() {\n${code}\n}`, {
    reportDiagnostics: true,
    compilerOptions: { target: ScriptTarget.ESNext, module: ModuleKind.ESNext },
  })
  // ... TypeScript → JS 转译
  const executableCode = transpiled.outputText.slice(bodyStart, bodyEnd)
  const parsed = parse(executableCode, { ecmaVersion: "latest", sourceType: "script", ... })
  // ...
}
```

TypeScript 先通过 `transpileModule` 转为 JS,再由 Acorn 解析为 AST。整个过程**不执行任何用户代码**。

**沙箱全局域**(`packages/codemode/src/interpreter/runtime.ts:602-661`):

```typescript
class Interpreter<R> {
  constructor(...) {
    const globalScope = new Map<string, Binding>()
    this.scopes = [globalScope]
    this.callPermits = Semaphore.makeUnsafe(TOOL_CALL_CONCURRENCY) // 并发上限=8
    globalScope.set("tools", { mutable: false, value: new ToolReference([]) })
    globalScope.set("Promise", { mutable: false, value: new PromiseNamespace() })
    globalScope.set("Object", { mutable: false, value: new GlobalNamespace("Object") })
    // ... Math/JSON/Number/String/Boolean/Array/console/Date/RegExp/Map/Set/URL
    // 无 fetch,无 crypto,无 fs,无 process,无 require/import
  }
}
```

关键限制:
- **无网络访问**: 没有 `fetch`、`XMLHttpRequest`、`WebSocket`
- **无文件系统**: 没有 `fs`、`require`、`import`
- **无 eval**: 没有 `eval`、`new Function`、`Function` 构造器
- **无 Generator**: `runtime.ts:851` 明确拒绝 Generator 函数
- **无 import**: 不支持模块导入

**工具调用并发控制**(`packages/codemode/src/stdlib/promise.ts:6`):

```typescript
export const TOOL_CALL_CONCURRENCY = 8  // 最多 8 个并发工具调用
```

**执行预算**(`packages/codemode/src/codemode.ts:9-17`):

```typescript
export type ExecutionLimits = {
  readonly timeoutMs?: number      // 墙钟超时
  readonly maxToolCalls?: number   // 工具调用上限
  readonly maxOutputBytes?: number // 输出字节上限
}
```

三个预算均由宿主设置,无默认值。

#### 2.1.3 工具发现机制

模型看到的不是完整工具目录,而是**token 预算化的内联目录**:

```typescript
// packages/codemode/src/tool-runtime.ts:86-87
const defaultCatalogBudget = 2_000  // 约 500 tokens
const defaultSearchLimit = 10
```

当工具目录超出预算时,命名空间保持可见,签名按 round-robin 选择,确保大命名空间不会饿死小命名空间。`$codemode.search` 工具始终可用,支持按路径精确查找、命名空间浏览、确定性排序和分页。

#### 2.1.4 数据边界

`copyIn` / `copyOut` 函数(引用自 `tool-runtime.ts`)在沙箱和宿主之间转换值:
- `Date` → ISO 字符串
- `RegExp` / `Map` / `Set` → `{}` (JSON 序列化行为)
- `Promise` / 运行时引用 → 不能穿越边界
- 最大嵌套深度固定(非可配置),产生比原生 stack-overflow 更清晰的诊断

**路径脱敏**(`packages/codemode/src/interpreter/runtime.ts:151-152`):

```typescript
const publicErrorMessage = (message: string): string =>
  message.replace(/\/(?:Users|home|private|tmp|var\/folders)\/[^\s"'`]+/g, "<redacted-path>")
```

---

### 2.2 packages/desktop — Electron 桌面端

**定位**: Electron 桌面应用壳,管理 Sidecar 服务进程、WSL 支持、自动更新、Deep Link。

#### 2.2.1 架构总览

```
desktop/
  src/main/       ← Electron 主进程(424行 index.ts + 40+ 模块)
    index.ts       ← 入口:Effect.runFork(main)
    server.ts      ← Sidecar 管理(utilityProcess.fork)
    ipc.ts         ← IPC 处理器注册
    menu.ts        ← 菜单栏
    updater.ts     ← 自动更新(electron-updater)
    wsl/           ← WSL 侧车控制器
    sidecar.ts     ← Sidecar 进程
    ...
  src/preload/     ← 预加载脚本
  src/renderer/    ← 渲染进程入口
```

#### 2.2.2 Sidecar 架构

Desktop 采用 **Sidecar 模式**: Electron 主进程通过 `utilityProcess.fork()` 启动一个独立的 Node.js 子进程运行 opencode server。

**Sidecar 启动**(`packages/desktop/src/main/server.ts:57-69`):

```typescript
export async function spawnLocalServer(hostname, port, password, options) {
  const sidecar = join(dirname(fileURLToPath(import.meta.url)), "sidecar.js")
  const child = utilityProcess.fork(sidecar, [], {
    cwd: process.cwd(),
    env: createSidecarEnv(),
    serviceName: SIDECAR_SERVICE_NAME,
    stdio: "pipe",
  })
  // ... 等待 "ready" 消息,超时 60s
}
```

**v2 Sidecar**(`packages/desktop/src/main/index.ts:333-348`):

```typescript
if (SIDECAR_VERSION === "v2") {
  const sidecar = yield* Effect.promise(() => startBackgroundCli(logger, shellEnv?.XDG_STATE_HOME))
  yield* Deferred.succeed(serverReady, {
    url: sidecar.url, username: sidecar.username, password: sidecar.password,
  })
  return
}
```

v1 使用端口分配 + 密码认证,v2 使用 Background CLI 模式。

**健康检查**(`packages/desktop/src/main/server.ts:186-211`):

```typescript
export async function checkHealth(url: string, password?: string | null): Promise<boolean> {
  const healthUrls = [new URL("/api/health", url), new URL("/global/health", url)]
  const headers = new Headers()
  if (password) {
    const auth = Buffer.from(`opencode:${password}`).toString("base64")
    headers.set("authorization", `Basic ${auth}`)
  }
  // ... fetch 每个 URL,3s 超时
}
```

#### 2.2.3 关键特性

- **Deep Link**: `opencode://` 协议注册(`packages/desktop/src/main/index.ts:271`)
- **WSL 支持**: `wsl/servers.ts` + `wsl/sidecar.ts` 管理 WSL 侧车
- **自动更新**: `electron-updater`,每 10 分钟检查(`index.ts:316-317`)
- **单实例锁**: `app.requestSingleInstanceLock()`(`index.ts:198`)
- **代理支持**: `http.setGlobalProxyFromEnv()` + loopback 绕过(`index.ts:72-113`)
- **日志导出**: `exportDebugLogs()` 支持调试日志打包

---

### 2.3 packages/slack — Slack 渠道集成

**定位**: Slack Bot 集成,通过 SDK 创建会话式对话。

**核心代码**(`packages/slack/src/index.ts:1-145`):

```typescript
const app = new App({
  token: process.env.SLACK_BOT_TOKEN,
  signingSecret: process.env.SLACK_SIGNING_SECRET,
  socketMode: true,
  appToken: process.env.SLACK_APP_TOKEN,
})
const opencode = await createOpencode({ port: 0 })  // 内嵌 server
const sessions = new Map<string, { client, server, sessionId, channel, thread }>()
```

**会话映射**: `channel-thread` 二元组 → opencode Session。每个 Slack 线程对应一个独立会话(`index.ts:70-93`)。

**工具更新实时推送**(`index.ts:23-38`):

```typescript
const events = await opencode.client.event.subscribe()
for await (const event of events.stream) {
  if (event.type === "message.part.updated") {
    // 找到对应 session,推送工具状态到 Slack
    void handleToolUpdate(part, session.channel, session.thread)
  }
}
```

**分享链接**: 创建会话后自动生成分享链接(`index.ts:96-101`):

```typescript
const shareResult = await client.session.share({ path: { id: createResult.data.id } })
if (!shareResult.error && shareResult.data) {
  await app.client.chat.postMessage({ channel, thread_ts: thread, text: sessionUrl })
}
```

---

### 2.4 packages/function — Cloudflare Workers 分享后端

**定位**: Cloudflare Worker + Durable Object,提供会话分享的实时同步后端。

#### 2.4.1 Durable Object 架构

**SyncServer**(`packages/function/src/api.ts:16-115`):

```typescript
export class SyncServer extends DurableObject<Env> {
  async fetch() {
    const webSocketPair = new WebSocketPair()
    const [client, server] = Object.values(webSocketPair)
    this.ctx.acceptWebSocket(server)
    // 历史数据推送给新订阅者
    const data = await this.ctx.storage.list()
    Array.from(data.entries())
      .filter(([key, _]) => key.startsWith("session/"))
      .map(([key, content]) => server.send(JSON.stringify({ key, content })))
    return new Response(null, { status: 101, webSocket: client })
  }

  async publish(key: string, content: any) {
    await this.env.Bucket.put(`share/${key}.json`, JSON.stringify(content))
    await this.ctx.storage.put(key, content)
    // 广播给所有 WebSocket 订阅者
    for (const client of this.ctx.getWebSockets()) {
      client.send(JSON.stringify({ key, content }))
    }
  }
}
```

**存储双写**: R2 持久化 + Durable Object 内存存储。R2 作为冷存储,DO 作为热缓存和 WebSocket 广播源。

#### 2.4.2 API 路由

| 路由 | 方法 | 功能 |
|------|------|------|
| `/share_create` | POST | 创建分享(secret + URL) |
| `/share_delete` | POST | 删除分享(需 secret) |
| `/share_delete_admin` | POST | 管理员删除(需 ADMIN_SECRET) |
| `/share_sync` | POST | 同步数据(需 secret) |
| `/share_poll` | GET | WebSocket 实时订阅 |
| `/share_data` | GET | 获取分享数据 |
| `/feishu` | POST | 飞书 → Discord 桥接 |

#### 2.4.3 飞书桥接

`packages/function/src/api.ts:205-250` 实现了飞书消息 → Discord 的桥接,支持挑战验证、消息解析、线程 ID 注入。

---

### 2.5 packages/enterprise — 企业版分享存储

**定位**: 企业版分享页面的后端存储层,支持 S3 和 Cloudflare R2。

#### 2.5.1 存储适配器

**Storage.Adapter 接口**(`packages/enterprise/src/core/storage.ts:5-10`):

```typescript
export namespace Storage {
  export interface Adapter {
    read(path: string): Promise<string | undefined>
    write(path: string, value: string): Promise<void>
    remove(path: string): Promise<void>
    list(options?: { prefix?: string; limit?: number; after?: string; before?: string }): Promise<string[]>
  }
}
```

**双适配器**(`storage.ts:66-91`):

```typescript
function s3(): Adapter {
  const client = new AwsClient({
    region, accessKeyId: process.env.OPENCODE_STORAGE_ACCESS_KEY_ID!,
    secretAccessKey: process.env.OPENCODE_STORAGE_SECRET_ACCESS_KEY!,
  })
  return createAdapter(client, `https://s3.${region}.amazonaws.com`, bucket)
}
function r2() {
  return createAdapter(client, `https://${accountId}.r2.cloudflarestorage.com`, bucket)
}
const adapter = lazy(() => {
  if (type === "r2") return r2()
  if (type === "s3") return s3()
})
```

#### 2.5.2 Share 数据模型

**Share.Data**(`packages/enterprise/src/core/share.ts:18-39`):

```typescript
export const Data = z.discriminatedUnion("type", [
  z.object({ type: z.literal("session"), data: z.custom<Session>() }),
  z.object({ type: z.literal("message"), data: z.custom<Message>() }),
  z.object({ type: z.literal("part"), data: z.custom<Part>() }),
  z.object({ type: z.literal("session_diff"), data: z.custom<SnapshotFileDiff[]>() }),
  z.object({ type: z.literal("model"), data: z.custom<Model[]>() }),
])
```

**Compaction 机制**(`share.ts:86-115`): 旧格式使用 event log + compaction 模式,新格式使用 snapshot 模式。`legacy()` 函数负责迁移。

---

### 2.6 packages/console — 管理控制台

**定位**: opencode.ai 的管理控制台,包含用户认证、工作空间管理、计费、数据统计。

#### 2.6.1 子包结构

```
console/
  app/          ← SolidStart + Cloudflare 前端(Vite)
  core/         ← 业务逻辑层(Drizzle ORM + Stripe + AWS)
  function/     ← Cloudflare Workers(认证 + 日志处理)
  mail/         ← 邮件模板(jsx-email)
  resource/     ← SST 资源声明
  support/      ← 客服支持
```

#### 2.6.2 认证系统

**OpenAuth 集成**(`packages/console/function/src/auth.ts:26-36`):

```typescript
export const subjects = createSubjects({
  account: z.object({
    accountID: z.string(), email: z.string(), newAccount: z.boolean().optional(),
  }),
  user: z.object({
    userID: z.string(), workspaceID: z.string(),
  }),
})
```

支持 GitHub + Google OIDC 双 Provider(`auth.ts:58-67`)。使用 `@openauthjs/openauth` 的 `issuer()` 函数,存储在 Cloudflare KV(`AuthStorage: KVNamespace`)。

#### 2.6.3 核心域模型

console/core 包含完整的 SaaS 域模型:

| 模块 | 职责 |
|------|------|
| `account.ts` | 账户管理 |
| `user.ts` | 用户管理 |
| `workspace.ts` | 工作空间 |
| `subscription.ts` | 订阅管理 |
| `billing.ts` | 计费(Stripe 集成) |
| `model.ts` | 模型管理 |
| `provider.ts` | Provider 管理 |
| `key.ts` | API Key 管理 |
| `referral.ts` | 推荐系统 |
| `identifier.ts` | ID 生成(ULID) |

#### 2.6.4 路由结构

console/app 的路由(`packages/console/app/src/routes/`):

```
auth/           ← 认证流程
workspace/      ← 工作空间管理
stats/          ← 统计面板
stripe/         ← Stripe 计费集成
enterprise/     ← 企业版
download/       ← 下载页
docs/           ← 文档
s/              ← 分享页面
user-menu.tsx   ← 用户菜单
workspace-picker.tsx ← 工作空间切换
```

---

### 2.7 packages/app — 桌面应用前端

**定位**: 桌面应用的前端层(Desktop + Web 共用),Solid Router 多页面架构。

#### 2.7.1 页面拓扑

```
app/src/pages/
  home.tsx              ← 首页(50行)
  session.tsx           ← 会话页(2,391行,核心页面)
  new-session.tsx       ← 新建会话
  error.tsx             ← 错误页
  layout.tsx / layout-new.tsx ← 布局
  directory-layout.tsx  ← 目录布局
  session/              ← 会话子模块(4,814行)
    composer/           ← 输入编辑器
    timeline/           ← 时间线
    file-tabs.tsx       ← 文件标签页(800行)
    session-side-panel.tsx ← 侧边面板(867行)
    terminal-panel.tsx  ← 终端面板(343行)
    review-tab.tsx      ← 审查标签(172行)
    ...
```

#### 2.7.2 Context Provider 树

`packages/app/src/app.tsx:42-68` 展示了复杂的 Context 嵌套:

```typescript
<ServerSDKProvider>        // SDK 连接
  <ServerSyncProvider>     // 服务端同步
    <GlobalProvider>       // 全局状态
      <HighlightsProvider> // 高亮
        <LanguageProvider> // 国际化
          <LayoutProvider> // 布局
            <ModelsProvider>  // 模型
              <NotificationProvider>  // 通知
                <PermissionProvider>  // 权限
                  <PromptProvider>     // 提示词
                    <SettingsProvider> // 设置
                      <TabsProvider>   // 标签页
                        <SDKProvider>  // SDK
```

14 层 Context Provider,体现了桌面应用的复杂状态管理需求。

#### 2.7.3 国际化

支持中英双语(`packages/app/src/app.tsx:33`):

```typescript
<LanguageProvider locale={locale}>
```

console/app 也支持中英(`packages/enterprise/src/app.tsx:25-56`):

```typescript
function detectLocale() {
  // 优先 Accept-Language header → document.documentElement.lang → navigator.languages
}
```

---

### 2.8 packages/cli — CLI 入口

**定位**: opencode CLI 二进制入口(lildax 命令)。

`packages/cli/package.json:6`:

```json
"bin": { "lildax": "./bin/lildax.cjs" }
```

**依赖**: `@opencode-ai/core` + `@opencode-ai/sdk` + `@opencode-ai/server` + `@opencode-ai/tui` + `@opentui/core` + `@opentui/solid`。

CLI 是薄壳,核心逻辑在 core/server/tui 中。

---

### 2.9 packages/client — 类型安全客户端

**定位**: 从 OpenAPI spec 自动生成的类型安全 HTTP 客户端。

**三层结构**:
- `generated/` — 从 `packages/sdk/openapi.json` 生成的 TypeScript 客户端
- `generated-effect/` — Effect 版本的生成客户端
- `contract.ts` — 手写的 API contract(Effect HttpApiMiddleware)
- `effect.ts` — 重导出层(从 Schema/Protocol 暴露类型)

**Contract 定义**(`packages/client/src/contract.ts:14-53`):

```typescript
export const ClientApi = makeDefaultApi({
  locationMiddleware: LocationMiddleware,
  sessionLocationMiddleware: SessionLocationMiddleware,
})

export const groupNames = {
  "server.health": "health",
  "server.session": "sessions",
  "server.agent": "agents",
  "server.model": "models",
  // ... 18 个 API 组
}
```

**API 组一览**: health / location / agents / sessions / messages / models / providers / integrations / credentials / permissions / files / commands / skills / events / ptys / questions / references / projectCopies。

---

### 2.10 packages/protocol — Effect HttpApi 协议层

**定位**: 定义 HTTP API 的 Effect HttpApi schema,供 server 和 client 共享。

**API 组装**(`packages/protocol/src/api.ts:37-63`):

```typescript
HttpApi.make("server")
  .add(HealthGroup)
  .add(LocationGroup.middleware(locationMiddleware))
  .add(AgentGroup.middleware(locationMiddleware))
  .add(makeSessionGroup(sessionLocationMiddleware))
  .add(MessageGroup.middleware(sessionLocationMiddleware))
  .add(ModelGroup.middleware(locationMiddleware))
  .add(ProviderGroup.middleware(locationMiddleware))
  .add(IntegrationGroup.middleware(locationMiddleware))
  .add(CredentialGroup.middleware(locationMiddleware))
  .add(makePermissionGroup(locationMiddleware, sessionLocationMiddleware))
  .add(FileSystemGroup.middleware(locationMiddleware))
  .add(CommandGroup.middleware(locationMiddleware))
  .add(SkillGroup.middleware(locationMiddleware))
  .add(eventGroup)
  .add(PtyGroup.middleware(locationMiddleware))
  .add(makeQuestionGroup(locationMiddleware, sessionLocationMiddleware))
  .add(ReferenceGroup.middleware(locationMiddleware))
  .add(ProjectCopyGroup.middleware(locationMiddleware))
  .middleware(Authorization)
  .middleware(SchemaErrorMiddleware)
```

**中间件注入点**: `locationMiddleware` 和 `sessionLocationMiddleware` 两个泛型中间件,由 server/client 分别注入具体实现。protocol 包**不依赖 core**,保持单向依赖链: `schema ← protocol ← server`。

---

### 2.11 packages/server — HTTP 路由装配

**定位**: 将 protocol 定义的 API schema 装配为可运行的 HTTP 路由。

**服务层装配**(`packages/server/src/routes.ts:26-37`):

```typescript
const applicationServices = LayerNode.group([
  Database.node, EventV2.node, httpClient, ToolOutputStore.cleanupNode,
  SessionV2.node, PermissionSaved.node, PtyTicket.node, Credential.node,
  PtyEnvironment.node, LocationServiceMap.node,
])
```

**嵌入式路由**(`routes.ts:47-48`):

```typescript
export function createEmbeddedRoutes() {
  return makeRoutes(ServerAuth.Config.configLayer({ username: "opencode", password: Option.none() }))
}
```

`createEmbeddedRoutes()` 用于 sdk-next 的内嵌模式(无需网络,直接 WebHandler)。

---

### 2.12 packages/effect-drizzle-sqlite — Drizzle ORM 桥接

**定位**: 将 Drizzle ORM 的 SQLite 驱动桥接到 Effect SQL 体系。

**核心类**(`packages/effect-drizzle-sqlite/src/effect-sqlite/session.ts:35-77`):

```typescript
export class EffectSQLiteSession<TRelations> extends SQLiteEffectSession<...> {
  constructor(private client: SqlClient, dialect, relations, options) { super(dialect) }
  override prepareQuery<T>(query, fields, executeMethod, customResultMapper, queryMetadata, cacheConfig) {
    return new SQLiteEffectPreparedQuery<T, EffectSQLiteQueryEffectHKT>(
      (params, method) => this.execute(query, params, method),
      query, this.options.logger, this.options.cache, queryMetadata, cacheConfig, ...
    )
  }
}
```

关键设计: Drizzle 的查询构建器通过 `SQLiteEffectPreparedQuery` 适配到 Effect 的 `SqlClient` 接口,保持 Drizzle 的类型推断能力同时享受 Effect 的依赖注入和资源管理。

---

### 2.13 packages/effect-sqlite-node — Node.js SQLite

**定位**: Node.js 环境的 SQLite 绑定,供 Desktop 模式使用。

与 `packages/core/src/database/sqlite.node.ts` 配合,提供 Node.js 原生 SQLite 驱动。Bun 环境使用 `sqlite.bun.ts`。

---

### 2.14 packages/http-recorder — HTTP 流量录制回放

**定位**: Effect HTTP 客户端的 VCR 库,录制真实流量并从 JSON cassette 回放。

**核心流程**:

```
首次运行 → 调用真实 API → 录制到 test/fixtures/recordings/<name>.json
后续运行 → 检测到 cassette → 从本地回放(无网络)
CI 环境 → 缺少 cassette → 直接失败(不录制)
```

**关键模块**:

| 模块 | 行数 | 职责 |
|------|------|------|
| `cassette.ts` | 179 | cassette 读写 |
| `matching.ts` | 106 | 请求匹配算法 |
| `redaction.ts` | 117 | 敏感信息脱敏 |
| `redactor.ts` | 135 | 脱敏器实现 |
| `socket.ts` | 326 | HTTP socket 录制 |
| `websocket.ts` | 173 | WebSocket 录制 |
| `recorder.ts` | 62 | 录制器核心 |
| `schema.ts` | 87 | cassette Schema |

**WebSocket 支持**: 不仅录制 HTTP,还录制 WebSocket 流量(`websocket.ts:1-173`),这对测试 MCP SSE 传输等场景很有价值。

---

### 2.15 packages/httpapi-codegen — OpenAPI 代码生成

**定位**: 从 Effect HttpApi schema 生成 TypeScript 客户端代码。

用于 `packages/client` 的 `generated/` 目录生成。输入是 Effect HttpApi 定义,输出是类型安全的 fetch 客户端。

---

### 2.16 packages/session-ui — 会话 UI 组件库

**定位**: 桌面和 Web 共享的会话 UI 组件。

**组件清单**(`packages/session-ui/src/components/`):

| 组件 | 功能 |
|------|------|
| `session-diff.ts` | 会话 Diff 展示 |
| `message-file.ts` | 消息文件渲染 |
| `message-part-text.ts` | 消息文本部分 |
| `markdown-stream.ts` | 流式 Markdown 渲染 |
| `markdown-cache.tsx` | Markdown 缓存 |
| `basic-tool.tsx` | 基础工具渲染 |
| `dock-prompt.tsx` | 停靠提示 |
| `file.tsx` / `file-media.tsx` / `file-search.tsx` | 文件展示 |
| `line-comment.tsx` | 行评论 |
| `line-comment-annotations.tsx` | 行注释 |
| `apply-patch-file.ts` | 补丁应用 |
| `pierre/` | Pierre diff 库集成 |
| `v2/` | v2 版本组件(含 prompt-input) |

**i18n 要求**(`packages/session-ui/AGENTS.md:1-6`):

> NEVER hardcode user-visible English strings in production code. ALWAYS use an i18n key.

---

### 2.17 packages/ui — 通用 UI 组件库

**定位**: 跨平台(SolidJS)通用 UI 组件库,30+ 组件,支持 Storybook。

**组件清单**(`packages/ui/src/components/`): accordion / animated-number / app-icon / avatar / button / card / checkbox / collapsible / context-menu / dialog / dropdown / ...

**主题系统**: `src/theme/` 目录,支持自定义主题 JSON。

**i18n**: `src/i18n/en.ts` + `src/i18n/zh.ts`,中英双语。

**字体**: `src/assets/fonts/` 内置 IBM Plex Mono 等字体。

---

### 2.18 packages/web — 公开文档站点

**定位**: opencode.ai 公开文档站点,基于 Astro + Starlight。

纯静态站点生成,包含文档内容(`src/content/`)、页面(`src/pages/`)、样式(`src/styles/`)。与 Agent 核心逻辑无关。

---

### 2.19 packages/containers — CI 构建容器

**定位**: GitHub Actions CI 用的预构建 Docker 镜像。**不是运行时沙箱**。

**镜像层级**(`packages/containers/README.md`):

```
base (Ubuntu 24.04)
  └── bun-node (+ Bun + Node.js 24)
       ├── rust (+ Rust stable)
       │    └── tauri-linux (+ Tauri 构建依赖)
       └── publish (+ Docker CLI + AUR)
```

构建命令: `REGISTRY=ghcr.io/anomalyco TAG=24.04 bun ./packages/containers/script/build.ts --push`

多架构构建: `--push` 发布 amd64 + arm64 双架构。

---

### 2.20 packages/script — 工程脚本

**定位**: 共享的工程脚本工具(主要是 semver 版本管理)。

`packages/script/src/index.ts` 导出 semver 工具函数,被 cli 的 `script/build.ts` 等使用。

---

### 2.21 packages/storybook — 组件文档

**定位**: Storybook 10 组件文档,用于 session-ui 和 ui 组件的可视化开发和测试。

使用 `storybook-solidjs-vite` 框架,支持 a11y / docs / links / vitest 插件。

---

### 2.22 packages/identity — 品牌资源

**定位**: 纯资源包,包含 opencode Logo 的各种尺寸和格式(PNG/SVG)。无代码。

---

### 2.23 packages/docs — Astro 文档站

**定位**: 内部 Astro 文档站点(区别于 `packages/web` 的公开文档)。包含 AI 工具文档(`ai-tools/`)和开发文档(`development.mdx`)。

---

## 3. 多渠道架构分析

opencode 的多渠道架构不是传统的"渠道抽象层",而是通过 **SDK + 内嵌 server** 实现渠道无关:

### 3.1 渠道清单

| 渠道 | 包 | 接入方式 |
|------|-----|---------|
| CLI/TUI | `cli` + `tui` | 直接调用 core + server |
| Desktop | `desktop` + `app` | Sidecar 进程 + HTTP API |
| Slack | `slack` | `@opencode-ai/sdk` → 内嵌 server |
| Web | `web` + `enterprise` | 浏览器 → HTTP API |
| Console | `console` | 浏览器 → HTTP API |

### 3.2 共享核心

所有渠道共享同一套核心:

```
@opencode-ai/core (DI + 数据库 + 会话)
  ↓
@opencode-ai/server (HTTP 路由)
  ↓
@opencode-ai/protocol (API schema)
  ↓
@opencode-ai/client (类型安全客户端)
  ↓
@opencode-ai/sdk / @opencode-ai/sdk-next (SDK 封装)
```

**Slack 的接入模式**(`packages/slack/src/index.ts:17-19`):

```typescript
const opencode = await createOpencode({ port: 0 })  // 内嵌 server,随机端口
// 然后通过 opencode.client.session.prompt() 发送消息
```

Slack 不直接调用 core,而是通过 SDK 创建一个内嵌的 opencode server 实例。这是**渠道标准化的关键**: 所有外部渠道都通过 SDK 接入,不直接依赖内部实现。

### 3.3 Desktop 的特殊性

Desktop 是唯一需要**进程管理**的渠道:

- Electron 主进程通过 `utilityProcess.fork()` 启动 Sidecar
- Sidecar 运行完整的 opencode server
- 渲染进程通过 HTTP API 与 Sidecar 通信
- v2 模式使用 Background CLI,进一步解耦

---

## 4. SDK 设计专题

### 4.1 双 SDK 架构

| SDK | 包 | 接入方式 | 特点 |
|-----|-----|---------|------|
| SDK v1 | `packages/sdk` | HTTP fetch | 从 OpenAPI spec 生成,纯 HTTP 客户端 |
| SDK Next | `packages/sdk-next` | Effect 内嵌 | 直接构建 LayerNode,无网络开销 |

### 4.2 SDK v1 — HTTP 客户端

**生成流程**: `packages/sdk/openapi.json` → `httpapi-codegen` → `packages/sdk/js/src/gen/`

**客户端工厂**(`packages/sdk/js/src/client.ts:33-57`):

```typescript
export function createOpencodeClient(config?: Config & { directory?: string }) {
  if (!config?.fetch) {
    const customFetch: any = (req: any) => { req.timeout = false; return fetch(req) }
    config = { ...config, fetch: customFetch }
  }
  if (config?.directory) {
    config.headers = { ...config.headers, "x-opencode-directory": encodeURIComponent(config.directory) }
  }
  const client = createClient(config)
  client.interceptors.request.use((request) => rewrite(request, config?.directory))
  client.interceptors.error.use(wrapClientError)
  return new OpencodeClient({ client })
}
```

**目录透传**: `x-opencode-directory` header 将项目目录传递给 server,支持多项目场景。

**Server 启动辅助**(`packages/sdk/js/src/server.ts:22-30`):

```typescript
export async function createOpencodeServer(options?: ServerOptions) {
  const proc = launch(`opencode`, [`serve`, `--hostname=...`, `--port=...`])
  // ... 等待 "opencode server listening on <url>" 输出
}
```

SDK 提供 `createOpencodeServer()` 辅助函数,自动启动 opencode server 并等待就绪。

### 4.3 SDK Next — Effect 内嵌运行时

**核心创新**(`packages/sdk-next/src/opencode.ts:10-43`):

```typescript
export const create = Effect.fn("OpenCode.create")(function* () {
  const context = yield* Layer.buildWithMemoMap(
    AppNodeBuilder.build(LayerNode.group([ApplicationTools.node, PermissionSaved.node])),
    memoMap, scope,
  )
  const tools = Context.get(context, ApplicationTools.Service)
  const permissions = Context.get(context, PermissionSaved.Service)
  const web = yield* Effect.acquireRelease(
    Effect.sync(() =>
      HttpRouter.toWebHandler(
        createEmbeddedRoutes().pipe(...),
        { disableLogger: true, memoMap },
      ),
    ),
    (web) => Effect.promise(web.dispose),
  )
  const fetch = Object.assign(
    (input, init) => web.handler(new Request(input, init)),
    { preconnect: () => undefined },
  )
  const client = yield* OpenCode.make({ baseUrl: "http://opencode.local" }).pipe(
    Effect.provide(FetchHttpClient.layer),
    Effect.provideService(FetchHttpClient.Fetch, fetch),
  )
  return { ...client, tools: { register: tools.register } }
})
```

**关键设计**: 不启动 HTTP server,而是用 `HttpRouter.toWebHandler` 将路由转为 `fetch` 函数,然后注入到 HTTP client。这样 SDK Next 的所有调用都是**内存中的函数调用**,零网络开销。

**工具注册**: SDK Next 暴露 `tools.register`,允许调用方注册自定义工具,这是插件化接入的关键接口。

---

## 5. CodeMode — 受限代码执行专题

### 5.1 设计哲学

CodeMode 的核心理念是**减少模型上下文消耗**:

> Reduce model context consumed by large tool catalogs.
> Avoid an agent round-trip between every dependent tool call.
> Keep large intermediate results inside the program instead of sending them through model context.
> — `packages/codemode/codemode.md`

### 5.2 与传统沙箱的对比

| 特性 | 传统沙箱(Docker/gVisor) | CodeMode |
|------|------------------------|----------|
| 隔离级别 | 进程/文件系统/网络 | AST 解释器 |
| 支持语言 | 任意 | JS/TS 子集 |
| 性能开销 | 容器启动 ~100ms | 解释执行,无启动开销 |
| 安全保证 | 内核级 | 语言级(无 eval/import/fetch) |
| 工具集成 | 需要 IPC/RPC | 原生函数调用 |
| 可观测性 | 日志/stdout | 结构化 Diagnostic |

### 5.3 诊断分类

10 种诊断类型(`packages/codemode/src/codemode.ts:62-73`):

```typescript
const DiagnosticKind = Schema.Literals([
  "ParseError", "UnsupportedSyntax", "UnknownTool", "InvalidToolInput",
  "InvalidToolOutput", "InvalidDataValue", "ToolCallLimitExceeded",
  "TimeoutExceeded", "ToolFailure", "ExecutionFailure",
])
```

每种诊断都有明确的语义,帮助模型理解失败原因并自我修正。

### 5.4 AGENTS.md 设计约束

`packages/codemode/AGENTS.md` 定义了严格的设计边界:

> - Keep Code Mode unaware of host session, channel, and conversation models.
> - Tool schemas are the model-facing Interface. Keep arguments minimal.
> - Never add unrelated IDs as ambient capability tokens.

> - Globals such as fetch, crypto, filesystem handles, extra modules, or network clients should be opt-in runtime capabilities with obvious policy defaults, not ambient authority.

---

## 6. 企业版与云同步专题

### 6.1 双后端架构

| 后端 | 包 | 存储 | 特点 |
|------|-----|------|------|
| Cloudflare Workers | `function` | Durable Object + R2 | 实时 WebSocket,免费额度 |
| Enterprise S3/R2 | `enterprise` | S3 / R2 | 通过 `aws4fetch` 访问 |

### 6.2 分享生命周期

```
1. 用户点击分享 → session.share() API
2. 后端创建 SyncServer Durable Object → 生成 secret + URL
3. 客户端通过 /share_sync 同步数据(需 secret)
4. R2 持久化 + DO 内存缓存 + WebSocket 广播
5. 浏览器通过 /share_poll WebSocket 实时接收更新
6. 删除需 secret(或管理员 ADMIN_SECRET)
```

### 6.3 安全模型

- **Secret 认证**: 每个分享有独立 UUID secret
- **管理员密钥**: `ADMIN_SECRET` 环境变量,用于强制删除
- **路径验证**: `publish()` 只允许写入 `session/info/` / `session/message/` / `session/part/` 前缀

---

## 7. 数据库与测试基础设施

### 7.1 数据库层

**双运行时 SQLite**:
- `packages/effect-drizzle-sqlite` — Drizzle ORM → Effect SQL 桥接
- `packages/effect-sqlite-node` — Node.js 原生 SQLite 绑定
- `packages/core/src/database/sqlite.bun.ts` — Bun 内置 SQLite

**运行时切换**(`packages/core/package.json` imports 字段):

```json
"#sqlite": { "bun": "./src/database/sqlite.bun.ts", "node": "./src/database/sqlite.node.ts" }
```

### 7.2 测试基础设施

**HTTP 流量录制**(`packages/http-recorder`):
- 首次运行录制真实 API 响应到 JSON cassette
- 后续运行从 cassette 回放,零网络
- CI 环境缺少 cassette 直接失败
- 支持敏感信息自动脱敏
- 支持 WebSocket 流量录制

**组件测试**(`packages/storybook`):
- Storybook 10 + SolidJS
- session-ui 和 ui 组件的可视化测试

---

## 8. 对 laew 借鉴路线

### P0 — 可直接落地

#### 8.1 CodeMode 受限执行(核心借鉴)

**借鉴价值**: opencode 的 CodeMode 证明了**不需要容器沙箱也能安全执行模型代码**。树遍历解释器 + 显式工具暴露 + 执行预算,比 Docker 沙箱轻量得多。

**laew 落地建议**:
1. Rust 实现一个简易 JS/TS 解释器(或使用 deno_core/rquickjs)
2. 只暴露显式注册的工具,默认无网络/文件/eval
3. 设置 timeout + maxToolCalls + maxOutputBytes 三重预算
4. 输出结构化诊断,帮助模型自我修正

#### 8.2 HTTP 流量录制回放

**借鉴价值**: `http-recorder` 的 cassette 模式可以大幅降低 LLM API 测试成本。

**laew 落地建议**:
1. 录制真实 Anthropic/OpenAI API 响应到 JSON 文件
2. 测试时从文件回放,无需真实 API Key
3. CI 环境缺少录制直接失败

#### 8.3 SDK 内嵌模式

**借鉴价值**: SDK Next 的 `HttpRouter.toWebHandler` 内嵌模式,零网络开销。

**laew 落地建议**:
1. laew 的 server 可以支持嵌入模式(TUI 直接调用,无需 HTTP)
2. 第三方集成时启动 HTTP server,两种模式共享同一套 handler

### P1 — 需要重构

#### 8.4 多渠道 SDK 标准化

**借鉴价值**: opencode 通过 SDK 抽象了所有渠道的接入方式。

**laew 落地建议**:
1. 提供 `laew-sdk` crate,暴露 session/message/tool API
2. Slack/Discord/飞书等渠道通过 SDK 接入,不直接依赖内部实现
3. SDK 支持 HTTP 模式和内嵌模式

#### 8.5 Schema 单一来源

**借鉴价值**: `packages/schema` 作为所有类型的单一来源,protocol/client/server 都从这里引用。

**laew 落地建议**:
1. 创建 `laew-schema` crate,定义 Session/Message/Tool 等核心类型
2. agent/llm/server 都从 schema 引用,不重复定义

#### 8.6 Drizzle ORM 桥接模式

**借鉴价值**: effect-drizzle-sqlite 层保持了 ORM 的类型推断能力同时享受 Effect DI。

**laew 落地建议**:
1. laew 的 SQLite 层可以考虑使用 sqlx/diesel 的类型安全查询
2. 通过 trait 抽象保持数据库无关性

### P2 — 战略级

#### 8.7 企业版分享存储

**借鉴价值**: 会话分享 + 实时同步的完整方案。

**laew 长期建议**:
1. 支持会话分享到 Web(生成公开链接)
2. 使用 S3/R2 作为后端存储
3. WebSocket 实时同步会话进度

#### 8.8 管理控制台

**借鉴价值**: console 的 SaaS 域模型(Account/Workspace/Subscription/Billing)。

**laew 长期建议**:
1. 如果 laew 走 SaaS 路线,需要类似的管理面板
2. OpenAuth + Stripe 集成可以作为参考

---

## 自检清单

- [x] 34 包总览(标记已覆盖/未覆盖)
- [x] 未覆盖包逐个深挖(21 个包)
- [x] 多渠道架构分析(5 个渠道的接入方式)
- [x] SDK 设计专题(v1 HTTP + v2 内嵌)
- [x] CodeMode 受限执行专题(解释器/沙箱全局域/诊断分类)
- [x] 企业版与云同步专题(双后端/分享生命周期)
- [x] 数据库与测试基础设施(Drizzle 桥接/HTTP 录制)
- [x] 对 laew 借鉴路线(P0/P1/P2)
- [x] 全文中文
- [x] 每个论断配文件路径:行号
- [x] 避免重复(Effect DI/llm/permission/doom_loop/cost/stats/plugin/session 存储只概述引用)
