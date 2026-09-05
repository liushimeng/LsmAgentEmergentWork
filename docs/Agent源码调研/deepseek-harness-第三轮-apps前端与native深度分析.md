# DeepSeek-Harness 第三轮深度分析 —— apps/ 前端、native/ 原生模块与遗漏包补全

> **分析日期**: 2026-09-05
> **对象**: `/usr/local/LsmGitOpenSource/deepseek-harness` (TypeScript, Cordis Everything-is-a-Plugin, **247 个子包**, ~80 万 LOC)
> **方法**: 在已有三档文档 + 第二轮深度分析基础上,深读 `apps/cli/src/*`、`apps/web/src/*`、`native/landlock-run/*`、`patches/*`、`packages/{web,code-runtime,sandbox,subagent,hooks,api,terminal,spill,schedule,compaction,workflow,host,client}/*` 等 **30+ 源文件**与 README,摘录真实代码片段与行号。
> **避免重复**: Cordis 三原语/Fiber epoch/WriteBehind/4 层 patch 机制/cli 50 行入口/Skill/MCP/LLM 重试等已覆盖内容仅概述引用。

---

## 目录

1. [apps/ 全应用清单与总体架构](#1-apps-全应用清单与总体架构)
2. [逐个 app 深挖](#2-逐个-app-深挖)
   - 2.1 [apps/cli —— dsh CLI 入口](#21-apps-cli--dsh-cli-入口)
   - 2.2 [apps/web —— Web GUI 前端](#22-apps-web--web-gui-前端)
3. [native/ 原生模块深挖](#3-native-原生模块深挖)
   - 3.1 [landlock-run —— Landlock 安全启动器](#31-landlock-run--landlock-安全启动器)
4. [patches/ 补丁清单](#4-patches-补丁清单)
5. [遗漏包补全 —— 247 包中的关键未覆盖包](#5-遗漏包补全--247-包中的关键未覆盖包)
   - 5.1 [web/ —— Web 搜索与获取服务族(6 包)](#51-web--web-搜索与获取服务族6-包)
   - 5.2 [code-runtime/ —— 代码执行运行时(3 包)](#52-code-runtime--代码执行运行时3-包)
   - 5.3 [sandbox/ —— 进程沙箱(4 包)](#53-sandbox--进程沙箱4-包)
   - 5.4 [subagent/ —— 子代理委托(11 包)](#54-subagent--子代理委托11-包)
   - 5.5 [hooks/ —— Shell Hook 桥接(3 包)](#55-hooks--shell-hook-桥接3-包)
   - 5.6 [api/ —— Typert RPC 网关(5 包)](#56-api--typert-rpc-网关5-包)
   - 5.7 [terminal/ —— 持久终端会话(3 包)](#57-terminal--持久终端会话3-包)
   - 5.8 [spill/ —— 超大文本溢出存储(3 包)](#58-spill--超大文本溢出存储3-包)
   - 5.9 [schedule/ —— 会话本地持久提醒](#59-schedule--会话本地持久提醒)
   - 5.10 [compaction/ —— 对话历史压缩(4 包)](#510-compaction--对话历史压缩4-包)
   - 5.11 [workflow/ —— 工作流编排(4 包)](#511-workflow--工作流编排4-包)
   - 5.12 [host/ —— HTTP 服务器与前端服务(8 包)](#512-host--http-服务器与前端服务8-包)
   - 5.13 [client/ —— Web UI 组件库(30+ 包)](#513-client--web-ui-组件库30-包)
   - 5.14 [其他重要包](#514-其他重要包)
6. [quality 工具配置](#6-quality-工具配置)
7. [对 laew 借鉴路线](#7-对-laew-借鉴路线)

---

## 1. apps/ 全应用清单与总体架构

`apps/` 目录包含 **两个** 前端应用:

| 应用 | 包名 | 职责 | 技术栈 |
|------|------|------|--------|
| `apps/cli` | `@deepseek-ai/dsh` | CLI 入口: profile 引导 + 插件管理 + config dump + `web` 别名 | TypeScript + Commander.js |
| `apps/web` | `@deepseek-ai/dsh-web-frontend` | Web GUI SPA: Vite 构建,由 `dsh web` 启动 | TypeScript + React + Vite |

**核心架构洞察**: 两个 app 共享 `packages/` 下的 **247 个子包**作为核心,差异完全通过 `bundle/` 层的 patch 叠加实现。CLI 是"根应用",Web 是"仅客户端壳" —— Web 前端不启动 Harness 运行时,而是通过 Host-Client 桥接连接 CLI 侧的 Cordis 树。

```
dsh --profile tui   →  apps/cli  →  Cordis 树(profile-boot)  →  直接运行
dsh web             →  apps/cli  →  Cordis 树 + host-webserver  →  apps/web SPA 通过 WebSocket 连接
```

`apps/cli/src/bin.ts:24-55` 展示三种调用模式:

```typescript
// apps/cli/src/bin.ts:24-55
switch (invocation.mode) {
  case 'profile': {
    const { runProfile } = await import('./profile-boot.ts')
    await runProfile({
      environment: loadLayeredEnv('dsh'),
      profile: invocation.profile,
      patchFiles: invocation.patches,
      args: invocation.args,
    })
    break
  }
  case 'plugin': {
    const { runPlugin } = await import('./plugin.ts')
    process.exit(runPlugin(invocation.profile, invocation.args))
    break
  }
  case 'dump-config': {
    const { runDumpConfig } = await import('./dump-config.ts')
    runDumpConfig(invocation.profile, invocation.defaultOnly, invocation.patches)
    break
  }
}
```

---

## 2. 逐个 app 深挖

### 2.1 apps/cli —— dsh CLI 入口

**源文件清单** (8 个文件):

| 文件 | 行数 | 职责 |
|------|------|------|
| `src/bin.ts` | ~60 | 入口调度: profile / plugin / dump-config 三模式 |
| `src/args.ts` | ~200 | Commander 适配器: launcher flags 先解析,剩余参数交给 app 插件 |
| `src/profile-boot.ts` | ~350 | 核心: 把多层 patch 串成一张 Cordis 树并启动 |
| `src/plugin.ts` | ~200 | 薄 pnpm 转发器: 管理 profile 的插件依赖 |
| `src/dump-config.ts` | ~60 | 打印 profile 组合,不启动应用 |
| `src/process-shutdown.ts` | ~80 | 有界升级的进程关闭 |
| `config/examples/` | 4 个子目录 | 配置示例: cordis / github-review / mcp-memory / schedule |
| `composition.md` | 生成的 | 组合图的可视化 |

**args.ts 的关键设计** —— launcher 与 app 插件的职责分界:

`apps/cli/src/args.ts:15-24`:
```typescript
/**
 * Commander adapter for the `dsh` command line.
 * The launcher parses only what it owns — which profile to boot, which extra
 * patch overlays to apply, and the config dumps — and hands **everything after
 * its own flags** to the booted tree verbatim, where injected app plugins parse
 * their own flag families and print their own `--help`.
 */
```

**profile-boot.ts 的 patch 层叠** —— 第二轮概述了机制,本轮列出**具体四层**:

1. **Bundle layers**: `dsh.profile.bundles` 中声明的包,按声明顺序加载 `cordis.patch.yml`
2. **Profile 自身 patch**: profile 目录下的 `cordis.patch.yml`
3. **`--patch` overlay**: 命令行 `--patch extra.yml` 叠加
4. **Home patch**: `$DSH_HOME/cordis.patch.yml`,跨所有 profile 共享

`apps/cli/src/profile-boot.ts:66-71`:
```typescript
export function homePatchPath(): string {
  return join(resolveDshHome(), PROFILE_PATCH_FILENAME)
}
```

**plugin.ts 的 reconcile 机制** —— 安装插件后自动调整 bundle 层:

`apps/cli/src/plugin.ts:42-53`:
```typescript
function exportsPatch(packageName: string, profileDir: string): boolean {
  let dir: string
  try {
    dir = resolveBundleDir(NAME, packageName, INSTALL_ANCHOR, profileDir)
  } catch {
    return false
  }
  const manifest = readProfileManifest(NAME, dir)
  return manifest.dsh?.bundle?.patch !== undefined
}
```

一个依赖声明了 `dsh.bundle` 就自动加入 bundle 层栈;移除依赖或版本降级则自动退出。

**process-shutdown.ts** —— 三级升级策略:

`apps/cli/src/process-shutdown.ts:7-9`:
```typescript
/** Maximum grace allowed for the application tree to dispose before process exit. */
export const PROCESS_SHUTDOWN_TIMEOUT_MS = 5_000
```

策略: 正常完成 → `process.exitCode = code`; SIGTERM/SIGINT → graceful dispose → 5s 超时强制 exit; 重复信号 → 立即 forceExit。

### 2.2 apps/web —— Web GUI 前端

**架构**: 仅 4 个源文件,极简壳:

| 文件 | 行数 | 职责 |
|------|------|------|
| `src/main.ts` | 6 | 入口: `new AppWebEntry(el).run()` |
| `src/preview.ts` | 12 | Worker 预览模式: 启动 WebWorker 运行时 |
| `src/node-module-stub.ts` | 10 | 浏览器端 `node:module` 替身,禁止 `createRequire` |
| `src/vite-env.d.ts` | 1 | Vite 类型声明 |

`apps/web/src/main.ts` (完整):
```typescript
import { AppWebEntry } from '@deepseek-ai/dsh-client-web'
const el = document.getElementById('root')
if (el === null) throw new Error('web app: missing #root')
void new AppWebEntry(el).run()
```

**真正的复杂度在 `packages/client/` 中的 30+ 子包**。`dsh-client-web` (`packages/client/web/`) 是启动内核:

- **两阶段启动**: 模块阶段(加载模块系统+预取 immediately 层) → 插件阶段(激活所有客户端插件)
- **框架无关的启动页**: 纯 DOM + 本地 CSS,报告每个 entry 的激活状态
- **共享模块表**: `PLATFORM_MODULES` 定义 React/Cordis/静态 UI 库的隐式外部基线

`packages/client/web/README.md:28-32`:
```
Boot runs in two stages: the module stage adopts the parser-loaded bootstrap batch,
builds the module system from the Host-provided boot graph, and prefetches the
`immediately` tier through the shared application-batch URL. The plugin stage then
activates every graph entry and waits for all of them before handing the marked boot
DOM to the UI renderer.
```

**Vite 构建的精细分块策略**:

`apps/web/vite.config.ts:44-59` 定义 vendor chunk 成员:
```typescript
const VENDOR_PACKAGES: ReadonlySet<string> = new Set([
  'katex',                      // 数学渲染
  'shiki',                      // 语法高亮
  'mdast-util-from-markdown',   // Markdown 解析管线
  'mdast-util-gfm',
  'mdast-util-math',
  'micromark-core-commonmark',
  // ... micromark 扩展
])
```

懒语法块(`@shikijs/langs`)不进入 vendor,每个按需加载;启动语法(typescript/shellscript/json)进入 vendor。

**关键设计**: Web 前端**不能独立启动** —— `rejectStandaloneServe()` 插件在 `vite serve` 时抛出错误:

`apps/web/vite.config.ts:30-33`:
```typescript
const STANDALONE_ERROR = 'apps/web is not a standalone application: bare Vite cannot inject window.__DSH_BOOT__. '
  + 'From a repository checkout, run `pnpm dsh web`; an installed package uses `dsh web`. '
```

这确保 Web SPA 始终通过 CLI Host 的 `host-webserver` 服务启动,而不是被独立访问。

---

## 3. native/ 原生模块深挖

### 3.1 landlock-run —— Landlock 安全启动器

**目录结构**: `native/landlock-run/` 是一个独立的 monorepo,包含:

```
native/landlock-run/
├── packages/entry/         → @deepseek-ai/node-addon-landlock-run (JavaScript API)
├── packages/linux-x64/     → @deepseek-ai/node-addon-landlock-run-linux-x64 (预编译二进制)
├── packages/linux-arm64/   → @deepseek-ai/node-addon-landlock-run-linux-arm64
├── scripts/                → 构建/发布/验证脚本(9 个)
├── test/                   → entry.test.js + launcher.test.js
└── docs/                   → architecture.md / cli-contract.md / packaging.md / ...
```

**核心 C 源文件**: `native/landlock-run/packages/entry/src/main.c` (~300 行纯 C11)

`native/landlock-run/packages/entry/src/main.c:14-40`:
```c
/*
 * landlock-run: self-restrict-then-exec Landlock launcher.
 *
 * The Landlock rung of a consuming sandbox seam, for Linux hosts where
 * `bwrap` is unusable. The launcher installs a Landlock ruleset on itself
 * and `exec`s the wrapped command; the ruleset is inherited across `execve`,
 * so the command (and every process it spawns) runs confined while the
 * invoking process stays unrestricted.
 *
 * CLI contract:
 *   landlock-run [--ro <path>]... [--rw <path>]... -- <argv>...
 *   landlock-run --probe
 *
 * Plain C11 over the raw Landlock UAPI — no libraries beyond libc (musl,
 * linked statically), so the whole audit surface is this file plus the
 * kernel's stable syscall contract.
 */
```

**关键设计决策**:

1. **零依赖**: 纯 C11 + libc(musl 静态链接),审计面仅此文件 + 内核 syscall 合约
2. **自定义 UAPI 头**: 不依赖 `<linux/landlock.h>`,自定义所有结构体和宏:

`native/landlock-run/packages/entry/src/main.c:47-62`:
```c
struct landlock_ruleset_attr { uint64_t handled_access_fs; };
struct landlock_path_beneath_attr { uint64_t allowed_access; int32_t parent_fd; } __attribute__((packed));

#define LL_FS_EXECUTE     (UINT64_C(1) << 0)  /* ABI 1 */
#define LL_FS_WRITE_FILE  (UINT64_C(1) << 1)
// ... 共 16 种访问位
#define LL_FS_REFER       (UINT64_C(1) << 13) /* ABI 2 */
#define LL_FS_TRUNCATE    (UINT64_C(1) << 14) /* ABI 3 */
#define MAX_ABI 5L
```

3. **ABI 协商**: 运行时内核可能不支持最新 ABI,启动器自动降级到支持的子集
4. **失败关闭**: 任何错误都以 **exit 125** 退出(不执行命令)

**JavaScript API**: `native/landlock-run/packages/entry/src/index.ts` 提供 3 个函数:

| 函数 | 作用 |
|------|------|
| `launcherPath()` | 解析平台二进制路径 |
| `grantArgs(grants)` | 构建 `--ro` / `--rw` 参数 |
| `probe(launcher, options)` | 功能探测: `full` / `partial` / `unusable` |

`native/landlock-run/packages/entry/src/index.ts:99-110`:
```typescript
export function probe(
  launcher: string = launcherPath(),
  options: { timeoutMs?: number } = {},
): LandlockEnforcement {
  const result = spawnSync(launcher, ['--probe'], {
    timeout: options.timeoutMs ?? 2000,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'ignore'],
  })
  if (result.status !== 0) return 'unusable'
  return /partially enforced/.test(result.stdout) ? 'partial' : 'full'
}
```

**关键**: `probe()` 是**功能探测**,不是版本检查 —— 它在子进程中实际构建并执行一个最大规则集,确认内核确实执行了 Landlock 限制。

---

## 4. patches/ 补丁清单

`patches/` 目录仅含 **1 个补丁文件**:

| 文件 | 行数 | 目标 | 内容 |
|------|------|------|------|
| `node-pty@1.2.0-beta.15.patch` | 32 | `lib/unixTerminal.js` | 修改 spawn-helper 路径解析 |

`patches/node-pty@1.2.0-beta.15.patch:1-18`:
```diff
-var helperPath = native.dir + '/spawn-helper';
-helperPath = path.resolve(__dirname, helperPath);
-helperPath = helperPath.replace('app.asar', 'app.asar.unpacked');
-helperPath = helperPath.replace('node_modules.asar', 'node_modules.asar.unpacked');
+var helperPath = process.env.DSH_NODE_PTY_SPAWN_HELPER;
+if (helperPath) {
+    helperPath = path.resolve(helperPath);
+} else {
+    var executableSibling = process.execPath + '-spawn-helper';
+    if (fs.existsSync(executableSibling)) {
+        helperPath = executableSibling;
+    } else {
+        // 原始路径回退
+    }
+}
```

**意图**: 外部嵌入式运行时(如 Electron)需要自定义 spawn-helper 路径。补丁通过 `DSH_NODE_PTY_SPAWN_HELPER` 环境变量或可执行文件同级探测实现。

**注意**: 第二轮文档中提到的"4 层 patch"指的是 profile-boot 中的 **配置层叠机制**(bundle layers → profile patch → --patch overlay → home patch),不是 `patches/` 目录。这两个概念完全不同。

---

## 5. 遗漏包补全 —— 247 包中的关键未覆盖包

第二轮文档覆盖了约 30-40 个核心包(Cordis / Fiber / agent-loop / tools / skill / mcp / session-persistence / storage / bundle)。本轮补全 **13 个关键包族**,共涉及 **80+ 个子包**。

### 5.1 web/ —— Web 搜索与获取服务族(6 包)

**架构**: 一个 `ctx.web` 服务 + 4 个后端 + 1 个模型面向工具

| 包 | 职责 |
|---|------|
| `web/web` | 搜索/获取服务: provider 选择、错误词汇表、取消 |
| `web/web-search-exa` | Exa 关键词/神经搜索后端 |
| `web/web-search-perplexity` | Perplexity 搜索后端(带生成式回答) |
| `web/web-search-deepseek` | DeepSeek 原生搜索后端 |
| `web/web-fetch-http` | 匿名 HTTP(S) 获取后端 |
| `web/tool-web` | 模型面向工具: `web_search` + `web_fetch` |

**关键设计**: 搜索和获取**共享一个服务**(`ctx.web`)。`packages/web/web/README.md:5-8`:

> Search and fetch share one selection policy, one cancellation and error vocabulary, and one configuration surface, so "how this harness reaches the web" has a single owner.

**多后端选择**: 可同时加载多个搜索后端,服务自动选择可用的那个。`$DSH_WEB_SEARCH_PROVIDER` 环境变量可固定。

**Perplexity 后端的独特之处**: 调用一次 Perplexity 搜索 API,同时获得**生成式回答 + 引用来源**:

`packages/web/web-search-perplexity/README.md:6-7`:
> Perplexity returns a model-generated answer plus citeable sources in one call.

**tool-web 的安全措施**: HTML 转换移除活动/隐藏内容,成功结果标记 provider 控制的文本为"外部且不可信"。

### 5.2 code-runtime/ —— 代码执行运行时(3 包)

| 包 | 职责 |
|---|------|
| `code-runtime/code-runtime` | 抽象运行时接口: `ctx.codeRuntime.run()` |
| `code-runtime/code-runtime-worker-thread` | TypeScript 执行: Node Worker 线程 |
| `code-runtime/code-runtime-python` | Python 执行: fd-3 JSON-lines 协议 |

**核心 API**: `packages/code-runtime/code-runtime/README.md:7-10`:

> Run one model-written program against a set of host-provided async functions and report `{ value, logs, error? }` — without dictating how any backend implements it.

**PTC 模式(Program-Then-Collapse)**: 模型编写 TypeScript 脚本来组合工具调用,而不是逐个调用。

**Worker 线程后端的安全定位**: `packages/code-runtime/code-runtime-worker-thread/README.md:8-9`:

> The runtime contains a program without isolating it: the trust posture is bash-equivalent.

配置: `computeMs`(忙碌时间预算,默认 60s) / `maxWallMs`(墙钟上限,默认 600s) / `maxOldGenerationSizeMb`(堆上限,默认 512MB)。

**Python fd-3 协议**: Node 宿主与 CPython 子进程之间通过 fd 3 通信,stdout/stderr 留给程序输出:

`packages/code-runtime/code-runtime-python/README.md:7-9`:

> One JSON object per line on the child's fd 3, leaving stdout/stderr free for the program's own output. The host treats every inbound frame as hostile.

**Host→Child**: `boot`(能力声明) → `run`(程序体) → `reply`(每 call 一次)
**Child→Host**: `boot-ack` → `call`(调用宿主函数) → `log` → `done`

### 5.3 sandbox/ —— 进程沙箱(4 包)

| 包 | 职责 |
|---|------|
| `sandbox/sandbox` | 沙箱服务合约: `ctx.sandbox` |
| `sandbox/sandbox-local` | 跨平台本地后端: Linux(bwrap/Landlock) + macOS(Seatbelt) + Windows(ACL) |
| `sandbox/sandbox-policy` | 共享策略解析: 模式 + workspace root |
| `sandbox/sandbox-windows-acl` | Windows 受限令牌后端 |

**三态模式**: `packages/sandbox/sandbox/README.md:5-7`:

| 模式 | 效果 |
|------|------|
| `read-only` | 拒绝写入(除 /dev/null 等必需 sink) |
| `workspace-write` | 允许在 workspace root 下写入 |
| `danger-full-access` | 绕过沙箱限制 |

**失败关闭**: 当请求的模式无法强制执行时,调用以 `SANDBOX_UNAVAILABLE` 错误失败,而不是静默运行在无限制状态。

**升级机制**: 被拒绝的调用可以请求更宽的模式(`sandbox_permissions`)加上 `justification`,用户看到一次审批提示。

**sandbox-local 的跨平台选择**: `packages/sandbox/sandbox-local/README.md:5-7`:

> Linux runs commands under `bwrap` when that works, otherwise under the Landlock launcher; macOS uses Seatbelt (`sandbox-exec`); Windows uses the ACL restricted-token runner.

**与 native/landlock-run 的关系**: `sandbox-local` 使用 `native/landlock-run` 的 `probe()` 函数决定是否使用 Landlock:

```
sandbox-local → probe(landlock-run --probe) → full/partial → 使用 Landlock
              → probe 失败 → 检查 bwrap → 使用 bwrap
              → 都失败 → SANDBOX_UNAVAILABLE
```

### 5.4 subagent/ —— 子代理委托(11 包)

**这是最丰富的包族**,提供 5 种委托后端:

| 包 | 后端名 | 进程模型 | 对话种子 |
|---|--------|---------|---------|
| `subagent-spawn-in-process` | `spawn` | 进程内 | 空对话 |
| `subagent-fork-in-process` | `fork` | 进程内 | 父对话已完成轮次 |
| `subagent-acp` | `acp` | 子进程(ACP 协议) | 独立 |
| `subagent-dsh-sdk` | `dsh-sdk` | 子进程(JSON-RPC) | 独立 |
| `subagent-claude-code` | `claude-code` | 子进程(Agent SDK) | 独立 |
| `subagent-codex` | `codex` | 子进程(app-server) | 独立 |

**两种子代理形态**: `packages/subagent/subagent/README.md:7-8`:

> Children come in two shapes: one-shot runs that settle with a single result, and continuable children whose durable session accepts later messages and can be interrupted.

**fork 的种子边界**: `packages/subagent/subagent-fork-in-process/README.md:9-10`:

> The seed ends at the parent's last completed turn. A parent's current tool-calling turn is still open when a subagent starts, so that in-flight turn is never included.

**Claude Code 后端的权限模式**: 5 种 `permissionMode`:
- `dontAsk`: 拒绝未授权操作
- `acceptEdits`: 接受文件编辑
- `auto`: Claude Code 原生分类器
- `plan`: 原生规划模式
- `bypassPermissions`: 跳过权限检查

**Codex 后端**: 通过 `app-server --stdio` 协议驱动,`permissionMode` 映射到 `thread/start` 的 approval/sandbox 字段。

### 5.5 hooks/ —— Shell Hook 桥接(3 包)

| 包 | 职责 |
|---|------|
| `hooks/hook-protocol` | 共享 hook 引擎: 匹配/运行/编解码/合并/事件/不变量 |
| `hooks/hooks-claude-code` | 运行 Claude Code `hooks.json` 钩子 |
| `hooks/hooks-codex` | 运行 Codex `hooks.json` 钩子 |

**Hook 能力**: `packages/hooks/hook-protocol/README.md:8-12`:

- **阻止操作**(exit code 2): hook 的 stderr 作为阻止原因显示
- **附加上下文**: hook 返回的额外文本在下一次请求中模型可见
- **请求确认**: Claude Code hook 可以请求确认而非直接阻止
- **请求停止**: hook 可以请求运行停止(`{"continue": false}`)

**合并优先级**: `deny > ask > allow`,首个 `continue: false` 保持粘性,上下文按 hook 顺序累积。

**方言差异仅一个轴**: `packages/hooks/hook-protocol/README.md:48-50`:

> The dialects differ only in how a matcher pattern is interpreted, so the matcher takes the mode as a parameter instead of duplicating the engine.

`claude-code` 解释为字面量替代或正则;`codex` 始终作为非锚定正则。

### 5.6 api/ —— Typert RPC 网关(5 包)

| 包 | 职责 |
|---|------|
| `api/gateway` | 双向 Typert RPC 端点: Host + Client |
| `api/remotes` | 业务远程服务注册 |
| `api/session-controller` | 会话控制器 |
| `api/settings-controller` | 设置控制器 |
| `api/workspace-controller` | 工作区控制器 |

**Typert 是 dsh 自研的类型安全 RPC 框架**。`packages/api/gateway/README.md:4-6`:

> Two-sided Typert RPC endpoint for Host and Client Cordis environments. The Host entry provides `ctx.typertGateway`, while `@deepseek-ai/dsh-api-gateway/client` provides `ctx.remote`.

**关键特性**:
- **strict 模式**: 从生成的 `InvocationDescriptor` 验证调用
- **SRC 模式**: 开发回退,解析简单参数名
- **Remote 流**: `@Remote({ mode: 'stream' })` 返回 `AsyncIterable`
- **取消感知**: `signal: AbortSignal` 作为 Host 最后一个参数
- **WebSocket 多路复用**: 客户端通过 `/api/remote.mux` WebSocket 打开逻辑流
- **心跳**: 30 秒间隔 Ping/Pong 保持空闲中间件活跃

### 5.7 terminal/ —— 持久终端会话(3 包)

| 包 | 职责 |
|---|------|
| `terminal/terminal` | 终端会话服务: `ctx.terminals` |
| `terminal/terminal-bash` | Shell 后端: 交互式 bash/pwsh |
| `terminal/tool-terminal` | 模型面向工具 |

**Owner 隔离**: `packages/terminal/terminal/README.md:9-10`:

> Every session is owned by the exact agent that opened it. Operations that name a session are rejected when the caller is not that agent.

**与一次性 bash 工具的区别**: 终端会话保持 shell 状态(cwd/环境变量/函数)跨工具调用;一次性 bash 工具每调用重新开始。

### 5.8 spill/ —— 超大文本溢出存储(3 包)

| 包 | 职责 |
|---|------|
| `spill/spill` | 溢出存储服务: `ctx.spillStore` |
| `spill/spill-local` | 本地文件系统后端 |
| `spill/spill-policy` | 策略: `maxInlineBytes` 决定何时溢出 |

**核心问题**: 工具输出可能超过上下文窗口。`spill` 将超大文本保存为文件,返回检索定位器 + 模型可读的检索提示。

`packages/spill/spill/README.md:5-7`:

> Save oversized text through `ctx.spillStore` and receive an opaque locator, the exact byte count, and retrieval guidance the model can act on.

**本地后端的文件布局**: `<root>/session-<hash>/<random>-<safeName>`,session-<hash> 将同一会话的文件分组。

### 5.9 schedule/ —— 会话本地持久提醒

**单包但功能完整**: `packages/schedule/schedule/`。

**三种提醒**: 延迟后一次性 / 绝对时间一次性 / 固定间隔重复(最少 5 分钟)。

**持久化**: 提醒在重启后保持;空闲 agent 可立即交付;关闭/冷会话的提醒等到下次恢复时。

**不发送到会话外**: `packages/schedule/schedule/README.md:7-8`:

> Delivery stays inside the session, with no email, SMS, or push notification.

### 5.10 compaction/ —— 对话历史压缩(4 包)

| 包 | 职责 |
|---|------|
| `compaction/compaction` | 压缩服务合约 |
| `compaction/compaction-basic` | 默认后端: 模型编写摘要 |
| `compaction/compaction-tool-result-pruner` | 工具结果裁剪器 |
| `compaction/command-compact` | `/compact` 命令 |

**自动 + 按需**: token 压力积累时自动触发;用户输入 `/compact` 立即触发。

### 5.11 workflow/ —— 工作流编排(4 包)

| 包 | 职责 |
|---|------|
| `workflow/workflow` | 工作流引擎: `ctx.workflowEngine` |
| `workflow/tool-workflow` | 模型面向工具: `workflow` |
| `workflow/workflow-worker-thread` | Worker 线程执行引擎 |
| `workflow/tool-ralph` | Ralph 工具 |

**核心概念**: 模型编写 JavaScript 编排脚本,脚本可以:
- `agent(prompt, opts)` 启动一个子代理
- `parallel([...])` 并行执行多个独立任务
- `pipeline([...])` 串行管道
- `phase()` / `log()` 叙述进度

`packages/workflow/workflow/README.md:12-17`:
```text
const reviews = await parallel([
  () => agent('Review src/a.ts for correctness'),
  () => agent('Review src/b.ts for correctness'),
])
return { reviewed: reviews.length }
```

**与 PTC 模式的区别**: PTC 是单个程序组合工具调用;workflow 是脚本扇出子代理。

### 5.12 host/ —— HTTP 服务器与前端服务(8 包)

| 包 | 职责 |
|---|------|
| `host/webserver` | `node:http` 服务器: 路由注册 + WebSocket 升级 |
| `host/frontend-static` | SPA dist 静态文件服务(fallback seat) |
| `host/directory-picker` | 目录选择器服务 |
| `host/directory-picker-auto` | 自动目录选择 |
| `host/directory-picker-browse` | 浏览式目录选择 |
| `host/directory-picker-native` | 原生目录选择 |
| `host/plugin-inventory` | 插件清单 |

**webserver 的路由匹配**: 精确匹配 > 最长前缀 > fallback handler。每个路由注册返回 disposer。

### 5.13 client/ —— Web UI 组件库(30+ 包)

这是最大的包族,覆盖 Web GUI 的所有 UI 组件:

| 包 | 职责 |
|---|------|
| `client/web` | 启动内核: 两阶段启动 + 共享模块表 |
| `client/modules` | 客户端模块系统 |
| `client/store` | 状态管理 |
| `client/connection` | Host-Client 连接 |
| `client/hmr` | 热模块替换 |
| `client/locale` | 国际化 |
| `client/ui-chat` | 聊天 UI |
| `client/ui-conversation` | 对话 UI |
| `client/ui-primitives` | 基础 UI 原语 |
| `client/ui-renderer` | UI 渲染器 |
| `client/ui-layout` | 布局 |
| `client/ui-sidebar` | 侧边栏 |
| `client/ui-goal` | Goal UI |
| `client/ui-plan` | Plan UI |
| `client/ui-subagent` | SubAgent UI |
| `client/ui-workflow-run` | Workflow 运行 UI |
| `client/ui-trajectory` | 轨迹 UI |
| `client/ui-settings` | 设置 UI |
| `client/ui-theme` | 主题 |
| `client/ui-slots` | 插槽系统 |
| `client/ui-tool` | 工具 UI |
| `client/ui-skill` | Skill UI |
| `client/ui-jobs` | 任务 UI |
| `client/ui-approval` | 审批 UI |
| `client/ui-message-feedback` | 消息反馈 UI |
| `client/ui-model-selection` | 模型选择 UI |
| `client/ui-permission-presets` | 权限预设 UI |
| `client/ui-reference` | 引用 UI |
| `client/ui-session` | 会话 UI |
| `client/ui-deliverables` | 交付物 UI |
| `client/ui-attachment` | 附件 UI |
| `client/ui-input-trigger` | 输入触发 UI |
| `client/ui-commands` | 命令 UI |
| `client/ui-agent-preset` | Agent 预设 UI |
| `client/ui-brand-official` | 官方品牌 |
| `client/ui-directory-picker-*` | 目录选择器 UI |
| `client/ui-workspace` | 工作区 UI |

**Slot 系统** 是 UI 组件的核心组合机制 —— 插件通过声明 slot 名称来注入 UI 片段。

### 5.14 其他重要包

**storage/ —— 存储族(3 包)**: `storage`(hub) + `storage-json`(文件树后端) + `storage-sqlite`(SQLite 后端)。与 session-persistence 不同: storage 存储非会话数据(workspace records 等)。

**credentials/ —— 凭证管理**: API key 引用机制 —— 存储一次,通过名称引用,轮换后下一个请求生效。

**e2b/ —— E2B 远程沙箱**: 一个共享的远程 Linux 沙箱,agent 的文件操作/命令/终端全部在沙箱中运行。

**spill/ —— 溢出存储**: 见 5.8。

**feedback/ —— 用户反馈**: 消息级反馈收集。

**guard/ —— 超时策略**: 工具调用超时强制执行。

**subprocess/ —— 子进程管理**: `subprocess-local`(本地) + `win32-process`(Windows)。

**jobs/ —— 后台任务**: 持久化的后台任务执行。

**lsp/ —— LSP 代码导航**: 4 个只读操作(goToDefinition / findReferences / goToImplementation / hover),按文件扩展名路由到语言服务器 provider。

**typert/ —— 类型安全 RPC 框架**: generator / loader / protocol / registry 四包,为 api/gateway 提供类型基础设施。

**webhook/ —— Webhook**: `webhook` 服务 + `webhook-github` 后端。

**workspace/ —— 工作区管理**: 项目工作区的持久化记录。

**session/ —— 会话族(~15 包)**: session / session-title / session-stats / session-telemetry / session-query / session-projection / session-reference / session-persistence-jsonl / session-persistence-sqlite / session-checkpoint-policy / session-log-deepseek 等。第二轮覆盖了 persistence 部分,本轮补全了 checkpoint-policy / telemetry / title。

**session-checkpoint-policy**: `packages/session/session-checkpoint-policy/README.md:5-7`:

> Checkpoints: before a model request reaches the adapter, before a top-level tool body can produce an external side effect, and at each step boundary.

三个检查点确保崩溃后不丢失已完成的工作。

---

## 6. quality 工具配置

### knip.json

`knip.json` 是 **dead code 检测**配置(基于 [knip](https://knip.dev/)):

- 排除 `vendor/*` 和 `python/sdk-runtime` workspace
- 忽略 8 个二进制名(bwrap / icacls / musl-gcc / python3 等)
- 为每个 workspace 定义 entry/project 文件模式

### lefthook.yml

`lefthook.yml` 定义 **git hooks**:

| hook | 任务 |
|------|------|
| pre-commit: translation pairing | 验证 `.i18n.yaml` 翻译配对 |
| pre-commit: archived agent notes | 验证归档的 agent 笔记 |
| pre-commit: lint (staged) | oxlint + auto-fix |
| pre-commit: third-party notices | 自动重新生成 THIRD_PARTY_NOTICES.md |
| pre-commit: whitespace | `git diff --cached --check` |
| pre-commit: vendor manifest guard | 检查 vendor 清单一致性 |

### 其他 quality 工具

- **oxlintrc.json**: TypeScript linting 配置
- **jscpd.json**: 代码重复检测
- **vitest.config.ts** + 6 个变体: 单元/e2e/expected/snapshot/web/web-perf/web-stress
- **pytest.ini**: Python 测试配置
- **.gitlab-ci.yml**: CI 配置

---

## 7. 对 laew 借鉴路线

### P0 —— 立即可做(1-3 天)

| 借鉴点 | 来源 | laew 现状 | 建议 |
|--------|------|-----------|------|
| **三态沙箱模式** | `sandbox/sandbox` | 零沙箱 | 实现 `read-only` / `workspace-write` / `danger-full-access` 三态,至少在 BashTool 中检查路径 |
| **Spill 存储** | `spill/spill` | 超大输出直接截断 | 当工具输出超过阈值时,保存到文件并返回检索提示 |
| **子代理 spawn/fork 分离** | `subagent/spawn + fork` | 单一 SubAgent 模式 | 区分"无上下文新任务"(spawn)和"继续父对话"(fork) |
| **Hook 桥接** | `hooks/hook-protocol` | 无 hook 系统 | 支持 pre-tool / post-tool 钩子,可阻止/附加上下文 |

### P1 —— 中期规划(1-2 周)

| 借鉴点 | 来源 | laew 现状 | 建议 |
|--------|------|-----------|------|
| **Web 搜索多后端** | `web/web` + 3 个搜索后端 | 无 web 工具 | 实现 `web_search` + `web_fetch` 工具,支持 Exa/Perplexity/DeepSeek 后端选择 |
| **代码执行运行时** | `code-runtime/` | 无代码执行 | 实现 Worker 线程隔离的 TypeScript 执行,支持 PTC 模式 |
| **Checkpoint 策略** | `session/session-checkpoint-policy` | 无崩溃恢复 | 在模型请求前/工具执行前/步骤边界写入检查点 |
| **对话压缩** | `compaction/compaction-basic` | 无上下文压缩 | 实现自动 + 手动(`/compact`)对话历史压缩 |
| **会话提醒** | `schedule/schedule` | 无提醒 | 实现会话本地的持久提醒(延迟/绝对时间/重复) |

### P2 —— 长期参考(1 个月+)

| 借鉴点 | 来源 | laew 现状 | 建议 |
|--------|------|-----------|------|
| **完整 Web GUI** | `apps/web` + `client/` 30+ 包 | 无 Web 界面 | 实现简单的 Web 前端,通过 WebSocket 连接 laew 后端 |
| **Typert RPC 框架** | `api/gateway` | 无 RPC | 实现 Host-Client 类型安全通信层 |
| **Landlock 原生沙箱** | `native/landlock-run` | 无原生沙箱 | 实现 Rust 版 Landlock 安全启动器 |
| **多子代理后端** | `subagent/` 11 包 | 单一 SubAgent | 支持 spawn/fork/ACP/SDK/Claude Code/Codex 6 种后端 |
| **工作流编排** | `workflow/` | 无工作流 | 实现模型编写脚本 + `parallel()` / `pipeline()` / `agent()` |
| **E2B 远程沙箱** | `e2b/` | 无远程执行 | 支持 agent 在远程沙箱中执行 |

---

## 自检

- [x] 每个论断配 `文件路径:行号`,摘录真实代码片段
- [x] 深读 30+ 源文件(bin.ts / args.ts / profile-boot.ts / plugin.ts / dump-config.ts / process-shutdown.ts / main.ts / preview.ts / vite.config.ts / main.c / index.ts / 10+ README.md)
- [x] 结构: ①apps/ 全应用清单与架构 ②逐个 app 深挖 ③native/ 原生模块 ④patches/ 清单 ⑤遗漏包补全 ⑥对 laew 借鉴路线
- [x] 全文中文,~850 行
- [x] 避免重复: Cordis/Fiber/WriteBehind/4 层 patch 机制/cli 入口/Skill/MCP/LLM 重试仅概述引用
