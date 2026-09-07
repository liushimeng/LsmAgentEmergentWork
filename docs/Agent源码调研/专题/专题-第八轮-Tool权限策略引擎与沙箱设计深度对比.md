---
title: 专题-第八轮-Tool权限策略引擎与沙箱设计深度对比
date: 2026-09-07
projects: [claudecode, atomcode, openclaw, opencode, deepseek-harness, pi, undici]
round: 8
scope: 三态策略状态机 / 规则匹配引擎 / 决策审计 / 多层防御 / OS 沙箱集成 / 细粒度控制 / HITL
related_topics:
  - 专题-权限管控深度分析.md(前序三态机总览,本篇深化)
  - 专题-沙箱设计深度分析.md(前序 bwrap/seccomp/landlock 总览,本篇深入 launcher 字节级)
  - 专题-第六轮-Hook系统与拦截器深度对比.md(Hook 通用机制)
  - 专题-第六轮-SubAgent调度与并发模型深度对比.md(子代理权限继承)
  - 专题-第六轮-Goal状态机与任务生命周期深度对比.md
  - 专题-第七轮-Bash命令执行与PTY进程管理深度对比.md
  - 专题-第七轮深挖合集.md
  - 专题-多Agent协作与权限管控深度分析.md
lines_target: 1000-1500
---

# 第八轮深挖专题:Tool 权限策略引擎与沙箱设计深度对比

> **本专题不与第七轮/前序专题重复**:第七轮已覆盖 Bash 进程管理、Git 检查点、文件编辑、Caching、Web 检索、多模态;前序专题-沙箱 (1812 行) 和专题-权限管控 (2266 行) 已系统对比 BubbleWrap/seccomp/landlock 与黑名单规则。本篇**专注 7 个新维度**:
>
> 1. **三态策略状态机** — `Allow / Ask / Deny` 的有限状态转移、Yolo/Default 模式的进入/退出、Hot-reload 与 Circuit-Breaker;
> 2. **规则匹配引擎** — regex/glob/path/command-hash 四种匹配维度、first-match vs all-match 语义、shadowing 检测;
> 3. **决策审计与回放** — 每次 permission 决策都落 trace,事后审计 + AI 复盘;
> 4. **多层防御** — 用户态策略 + 进程级沙箱 + OS 系统调用过滤三层协同;
> 5. **OS 沙箱集成** — macOS sandbox-exec (SBPL) / Linux bwrap + landlock / Windows AppContainer + Restricted Token;
> 6. **细粒度控制** — 网络出口、文件 R/W/D、进程 fork/exec、GPU/IO 资源;
> 7. **HITL (Human-in-the-Loop)** — 交互式 TUI 弹窗、二次确认、超时/取消策略。
>
> 本篇目标:为 laew 的 P0/P1/P2 路线图(尤其是 Rust `landlock`/`seccompiler` 集成)提供字节级蓝图。

---

## 目录

1. 结论速览(7 维度对比)
2. 三态策略状态机:从 6 源规则到二进制 lock
   - claudecode `PermissionMode` 6 态机 + `applyPermissionRulesToPermissionContext` (`permissions.ts:113-273`)
   - atomcode `PermissionDecision` 3 态 + `ToolMiddleware::BeforeOutcome` 4 态 (`approval.rs:78-82`)
   - deepseek-harness `SandboxMode` 3 态 + Escalation WIDER_MODES (`escalation.ts:18-26`)
   - opencode `PermissionV1.Action` 3 态 + `evaluate()` first-match 优先级 (`permission/index.ts:48-53`)
   - pi 委托给 `permission-gate.ts` 扩展(`extensions/permission-gate.ts:8-30`)
   - openclaw / undici 状态机嵌入到 transport + fetch
3. 规则匹配引擎:四维度 + first-match vs all-match
   - claudecode 4 维 (Tool + Content + glob + regex) `shellRuleMatching.ts:1-180`
   - opencode 2 维 (permission + pattern) `Wildcard.match` `util/wildcard.ts:5-15`
   - atomcode bash destructive 16 模式分类 `bash.rs:1660-2080`
   - deepseek-harness escalation 严格 widening `WIDER_MODES:read-only → workspace-write → danger-full-access`
   - shadowed rule detection `shadowedRuleDetection.ts:1-180`
4. 决策审计与回放
   - claudecode `logEvent()` 三元组(工具/决策/原因)`permissions.ts:130-145`
   - deepseek `approval/asked`+`approval/decided` 二元组+replay 幂等(`user-approval/src/index.ts:148-200`)
   - atomcode `LifecycleHooks::on_request` + `on_model_response` 双端记录(`hooks.rs:14-50`)
   - opencode `Event.Asked/Replied` Effect 事件流(`opencode/src/permission/index.ts:75-95`)
5. 多层防御:用户态策略 + 进程级沙箱 + OS 系统调用过滤
   - 三层架构图(ASCII)
   - claudecode:用户规则(6 源)→ `ApprovalMiddleware` → bwrap/seccomp/MITM proxy
   - atomcode:用户规则(`PERMISSION_RULE_SOURCES`)→ `ApprovalMiddleware` + `PlanModeGate` → 仅做用户态,无沙箱
   - opencode:用户规则(config.toml)→ `PermissionV2.Service.ask()` → 仅有协议层
   - deepseek:`sandboxPolicy` + `ApprovalPolicy` 双轴 → `LocalSandboxProvider` bwrap/landlock/seatbelt/acl
   - pi:无内建规则,扩展 + bwrap/seccomp
6. OS 沙箱集成字节级深挖
   - macOS SBPL:Chrome 同源策略基线(`macos-sandbox-utils.js:215-280` 60+ 个 sysctl 显式 allow)
   - Linux bwrap:`--ro-bind`/`--bind`/`--unshare-pid`/`--unshare-net`(`linux-sandbox-utils.js:480-600`)
   - Linux landlock:`landlock-run` 298 行 C11 自含(`landlock-run/main.c:1-298`)
   - Windows WRITE_RESTRICTED + workspace-SID + temp-SID(`workspace-sid.ts:1-50`)
7. 细粒度控制
   - 网络:7 仓库 HTTP proxy / SSRF / 域名 allowlist / `mitmProxy` Unix-socket 注入(`sandbox-config.js:1-180`)
   - 文件:glob + subpath + canonicalize 三重过滤(`macos-sandbox-utils.js:200-280`)
   - 进程:`process-exec`/`process-fork` 显式 allow,`signal (target same-sandbox)` 限制
   - 资源:bwrap `--rlimit-*` 缺省无;claudecode 自有内存限制在 sandbox 之外
8. HITL:交互式 TUI 弹窗、二次确认、超时
   - claudecode `PermissionDialog.tsx` 27 种请求 + `denialTracking.ts` 3 阈值降级
   - atomcode `AskUserQuestionPermissionRequest` (`claudecode 0 个,atomcode 在 components/permissions 中复用 11 个 Request 屏`)
   - deepseek answerer chain 顺序:`unavailable`→`cancelled`→`rejected`→`allowed-once`
   - opencode `WorkerPendingPermission.tsx` 主从架构:Worker 通过 Bridge 弹窗
9. 横向对比大表(7 工程 × 7 维度)
10. 共性模式:claudecode 6 源规则 → atomcode 三态机 → pi binary lock 借鉴链
11. laew P0/P1/P2 路线图(含 Rust crate:`landlock`/`seccompiler`/`capsicum`)
12. 关键代码路径速查表

---

## 1. 结论速览

| 维度 | claudecode | atomcode | opencode | deepseek-harness | pi | openclaw | undici |
|------|-----------|----------|----------|------------------|------|----------|--------|
| **三态状态机** | 6 态 (default/plan/acceptEdits/bypassPermissions/dontAsk/auto) | 3 态 (AllowOnce/AllowAlways/Deny) | 3 态 (ask/allow/deny) | 3 态 sandbox + 2 态 approval (ask/never) | 2 态 (block/allow) 扩展式 | 2 态 (allow/deny) SSRF/路径策略 | 仅 CORS / TAO / bad-port 三件套 |
| **规则匹配** | 4 维 first-match,glob+regex 联合 | exact/prefix/wildcard (3 态 per-rule) | permission + pattern (Wildcard.match) | sandbox mode strict-widening | 3 正则 (rm/sudo/777) | 整段 glob path 模式 | RFC 标定常量集合 |
| **决策审计** | `logEvent()` 三元组+analytics,无 trace 文件 | `on_request/on_model_response` JSON dump | `Event.Asked/Replied` Effect pub/sub | `approval/asked + approval/decided` session log 配对 | 扩展式 hook | per-frame `gateway-error-details` | 无(NOT a permission system) |
| **多层防御** | 6 源规则 + ApprovalMW + bwrap+seccomp+MITM | 用户态 + 敏感路径门 + workspace 门;无沙箱 | 仅用户态规则;无沙箱 | sandboxPolicy + ApprovalPolicy + 4 runner | 委托 bwrap 扩展 | 2 段 SSRF + inbound path policy | CORS + bad-ports + cross-origin 隔离 |
| **OS 沙箱** | macOS SBPL + Linux bwrap+seccomp-bpf + 缺 Windows | 无内建;声明 OS-level 是"embedder's responsibility" | 无内建 | bwrap + landlock + Seatbelt + Windows ACL | 扩展式 @anthropic-ai/sandbox-runtime(可选) | 进程级 inbound-path 沙箱 | 仅协议层 (bad-ports/CORS) |
| **细粒度** | 文件 R/W glob+subpath+canonical;进程 signal+exec;网络 MITM socket+HTTP/SOCKS proxy | 文件:bash destructive 16 模式;credential:12 标识 | 文件:read/edit/wildcard;doom_loop 死循环 | 文件:writableRoots 共享;进程:不限制 | 文件:bwrap 模式 | 文件:glob 段;网络:IP 段 deny list | URL scheme / port |
| **HITL** | 27 种 PermissionRequest 屏;denial 3 阈值降级;Statsig gate kill-switch | `AskUserQuestion` (单选/多选/输入);timeout→DenyTurn | WorkerPendingPermission 主从 + Bridge | answerer chain 4 outcome;Circuit-breaker;hook 拦截 | `ctx.ui.select` 二选一 | UI-driven HITL,经 `gateway-client` | n/a |

> **laew 现状(本轮调研口径,2026-09-07)**:`src/agent/sandbox_hook/` 仅对 `Write/Edit` 工具做路径白名单;Bash/Read/Glob/Grep/MCP **零沙箱零规则**;`permissions` 概念**完全不存在**。本专题为其 P0/P1/P2 路线提供字节级借鉴蓝图。

---

## 2. 三态策略状态机:从 6 源规则到二进制 lock

### 2.1 claudecode:6 态机 + auto 模式 (gating growth book)

`src/utils/permissions/PermissionMode.ts:24-78` 定义 6 态状态机:

```typescript
const PERMISSION_MODE_CONFIG = {
  default:        { color: 'text',     external: 'default' },           // 默认
  plan:           { color: 'planMode', external: 'plan' },              // 只读规划
  acceptEdits:    { color: 'autoAccept', external: 'acceptEdits' },     // 自动接受编辑
  bypassPermissions: { color: 'error', external: 'bypassPermissions' },  // 全自动
  dontAsk:        { color: 'error',    external: 'dontAsk' },           // 不问就拒
  auto:           { color: 'warning',  external: 'default' },           // (Ant-only) AI 分类器
} as const
```

**关键设计**:`getNextPermissionMode(ctx)`(`getNextPermissionMode.ts:31-100`)实现 Shift+Tab 状态切换,带 Statsig 远程配置 `tengu_auto_mode_config` 实时熔断——`isAutoModeGateEnabled()` Circuit-Breaker 在 `autoModeState.ts:14-19` 维护,任何 auto→manual 切换都会写 `autoModeCircuitBroken = true` 防止 SDK/Explicit 路径回滚。

**6 源规则优先级**(`settings/constants.ts:7-18`):`userSettings → projectSettings → localSettings → flagSettings → policySettings`,**后者覆盖前者**——企业管理的 `policySettings` 是最终防线。

### 2.2 atomcode:3 态 + 4 态 BeforeOutcome

`crates/atomcode-capabilities/src/tools/approval.rs:78-82` 三态 enum:

```rust
pub enum PermissionDecision {
    AllowOnce,    // 单次允许
    AllowAlways,  // 永远允许(写入 PermissionStore)
    Deny,         // 拒绝(从 parse 兜底:任何未知 JSON 字段 → Deny)
}
```

`PermissionDecision::from_value(v: &serde_json::Value)`(`approval.rs:85-95`)是**强类型 fail-closed 解析器**:任何未在 `{allow, allow_always, deny, remember}` 集合内的字段(包括 `Null`、缺失 decision、错别字)→ `Deny`,与 claudecode 兜底策略一致。

**4 态 BeforeOutcome**(`crates/atomcode-kernel/src/middleware.rs:71-110`)是更细的中间件语义:

```rust
pub enum BeforeOutcome {
    Proceed,                 // 通过
    Allow { .. },            // 短路 before 链
    Ask { .. },              // 请求 driver 弹窗
    Deny { reason },         // 阻塞本 call
    DenyTurn { reason },     // 阻塞 + 终止 turn
    DenyTurnWithIntervention { reason, intervention: PolicyIntervention },
}
```

`Allow` 会让 `ToolStarted` 事件不触发(no ghost row)——这是**claudecode 缺失的细节**。

### 2.3 deepseek-harness:3 态 sandbox + 2 态 approval + Escalation strict-widening

`packages/sandbox/sandbox/src/index.ts:38-49` 三态:

```typescript
export type SandboxMode = 'read-only' | 'workspace-write' | 'danger-full-access'
```

`WIDER_MODES: Record<string, readonly SandboxMode[]>`(`escalation.ts:18-26`)定义**严格 widening 关系**:

```typescript
export const WIDER_MODES = {
  'read-only': ['workspace-write', 'danger-full-access'],
  'workspace-write': ['danger-full-access'],
  // danger-full-access 无更宽
}
```

`approveEscalation()`(`escalation.ts:148-189`)执行严格检查:`requestedMode` 必须 ∈ `WIDER_MODES[effectiveMode]`,否则 throw `sandbox escalation to "X" is not strictly wider than this call's current "Y" mode`——**沙箱升级是不可逆决策**,被模型审计为"敢不敢要求宽权限"。

**2 态 approval**(`user-approval/src/index.ts:65-69`):

```typescript
export type ApprovalPolicy = 'ask' | 'never'
```

`'never'` 是 CI/headless 的 fail-closed 策略,任何 ask 立即 `'rejected'`,**不发送 approval/asked 事件**。

### 2.4 opencode:3 态 + evaluate() first-match

`packages/core/src/v1/config/permission.ts:9-23` 三态 Schema:

```typescript
export const Action = Schema.Literals(["ask", "allow", "deny"])
export const Rule = Schema.Union([Action, Object])  // "ask" | { pattern: action }
```

`opencode/src/permission/index.ts:48-53` 关键 first-match 实现:

```typescript
export function evaluate(permission: string, pattern: string, ...rulesets: PermissionV1.Ruleset[]): PermissionV1.Rule {
  return (
    rulesets
      .flat()
      .findLast((rule) => Wildcard.match(permission, rule.permission) && Wildcard.match(pattern, rule.pattern)) ?? {
      action: "ask",    // ← 默认兜底 ask
      permission,
      pattern: "*",
    }
  )
}
```

**`findLast` 是关键**——后定义的规则胜出,与多数命令式语言的"后写覆盖"心智模型一致。**默认兜底是 `ask`(即"不命中 → 询问用户"),不是 `deny`——claudecode 同理,atomcode fail-closed(deny),pi 委托给扩展。**

### 2.5 pi:委托给扩展 + `ctx.ui.select` 二选一

`packages/coding-agent/examples/extensions/permission-gate.ts:8-30` 是最简范本:

```typescript
pi.on("tool_call", async (event, ctx) => {
  if (event.toolName !== "bash") return undefined
  const isDangerous = dangerousPatterns.some(p => p.test(command))
  if (isDangerous) {
    if (!ctx.hasUI) return { block: true, reason: "Dangerous command blocked (no UI for confirmation)" }
    const choice = await ctx.ui.select(`⚠️ Dangerous command:\n\n  ${command}\n\nAllow?`, ["Yes", "No"])
    if (choice !== "Yes") return { block: true, reason: "Blocked by user" }
  }
  return undefined
})
```

pi **无内建权限引擎**,完全靠 `ExtensionAPI.on("tool_call")` 钩子+`ctx.ui.select` 弹窗。优势:**完全可插拔**;劣势:任何内建功能(自动补全、内置 Skill)都需用户写扩展。

### 2.6 openclaw:per-frame permission_profile

`packages/acp-core/src/types.ts:76-77`:

```typescript
export type AcpSession = {
  /** ACP runtime config option: permission profile id. */
  permissionProfile?: string;
  ...
}
```

权限配置经 ACP 协议在 session 创建时注入,运行期由 `packages/net-policy/src/ip.ts:46-90` 的 SSRF 策略(28 个 IPv4 段 + 13 个 IPv6 段)+ `packages/media-core/src/inbound-path-policy.ts:1-90` 的路径通配(whole-segment `*` only)执行。

### 2.7 undici:三件套 CORS/TAO/bad-port

`lib/web/fetch/util.js:219-234`:

```javascript
function crossOriginResourcePolicyCheck() { return 'allowed' }  // TODO
function corsCheck() { return 'success' }                         // TODO
function TAOCheck() { return 'success' }                         // TODO (TOFU)
```

三个核心 spec 钩子都是 stub;但 `lib/web/fetch/constants.js:14-22` 有**硬编码 89 个 bad-ports 集合**(`badPortsSet: Set<string>`),用于 fetch 请求时拒绝访问 SSH/SMTP/IRC 等敏感端口,这是**唯一强制的运行时权限**。`lib/web/fetch/index.js:583-586` 通过 `requestBadPort(request)` 检查。

---

## 3. 规则匹配引擎:四维度 + first-match vs all-match

### 3.1 claudecode:4 维匹配 + 字符级 escape

`src/utils/permissions/permissionRuleParser.ts:75-95` 解析 `Tool(content)`:

```typescript
export function permissionRuleValueFromString(ruleString: string): PermissionRuleValue {
  const openParenIndex = findFirstUnescapedChar(ruleString, '(')
  if (openParenIndex === -1) return { toolName: normalizeLegacyToolName(ruleString) }
  // escapeRuleContent:反斜杠先于圆括号
}
```

**escapeRuleContent**(`permissionRuleParser.ts:53-63`)先 escape `\\`,再 escape `(\\)`,顺序**不可逆**——这是与正则表达式 `findFirstUnescapedChar` 配合的关键,错误顺序会导致 `echo "test\n"` 误识别为 `echo \n` 内的换行。

`shellRuleMatching.ts:75-180` 完整 4 维匹配:

| 维度 | 语法 | 例子 |
|------|------|------|
| 工具级 | `Bash` | 整工具放行 |
| 精确 | `Bash(npm install)` | 字符串相等 |
| 前缀 | `Bash(npm:*)` | `:num` 语法=前缀(`*` 仅在末尾) |
| 通配 | `Bash(git *)` / `Bash(rm *)` | 正则编译为 `^pattern$` |

**关键设计**:`matchWildcardPattern`(`shellRuleMatching.ts:140-180`)生成 regex 时,若末尾是 ` .*` 且**仅有一个 unescaped `*`**,则 `*` 改为 `( .*)?`,**对齐前缀规则的语义**——`git *` 既能 match `git add` 也能 match `git`,因为前者加了一个可选 arg。

`hasWildcards(pattern: string)`(`shellRuleMatching.ts:60-72`)用**backtick 计数**判断 `*` 是否被 escape,避免 `\*`(字面量)误判为通配。

### 3.2 atomcode:bash destructive 16 模式

`crates/atomcode-capabilities/src/tools/bash.rs:1660-2080` 是按命令基名+危险模式的分类器,**16 个 pattern**:

| 命令 | 检测模式 | reason |
|------|---------|--------|
| `rm -r/-R/-rf` | flags + path tokens | recursive delete |
| `dd if=/dev/...` | `dd` 基础 + device operand | raw disk write |
| `:(){` | fork bomb | 显式字符串匹配 |
| `> /etc/passwd` | `scan_redirect_writes()` | critical system file overwrite |
| `mkfifo`/`mknod` | substring | named pipe / device node |
| `migrate:fresh` 等 | bigram window | schema reset |
| `apt remove`/`apt purge` | package manager | (高破坏性) |

**关键 API**:`BashTool::risk(args)`(`bash.rs:147-158`)在 tool 调用时**动态返回** `Safe / Risky`,**arg-aware 而非 tool-wide**——同一个 `Bash` 工具,`rm -rf /` 是 Risky,`ls -la` 是 Safe。这是 claudecode 缺失的精细度(CC 仅在 rule match 层做这件事)。

**normalization**:`normalize_command_for_grant()`(`bash.rs:1635-1642`)对 grant key 做**strip comments + collapse whitespace**,所以 cosmetic re-emit 的 `rm -rf / # 清理` 仍命中之前的 grant,不会反复弹窗。

### 3.3 opencode:Wildcard 跨平台

`packages/core/src/util/wildcard.ts:5-15` 是跨平台 wild-match:

```typescript
export function match(input: string, pattern: string) {
  const normalized = input.replaceAll("\\", "/")
  let escaped = pattern
    .replaceAll("\\", "/")
    .replace(/[.+^${}()|[\]\\]/g, "\\$&")
    .replace(/\*/g, ".*")
    .replace(/\?/g, ".")
  if (escaped.endsWith(" .*")) escaped = escaped.slice(0, -3) + "( .*)?"
  return new RegExp("^" + escaped + "$", process.platform === "win32" ? "si" : "s").test(normalized)
}
```

Windows 平台用 `si` 标志(case-insensitive + dotAll),Linux 用 `s`,**避免 `*.lock` 误匹配 newline**——**claudecode 也有同样的 `s` flag**(`shellRuleMatching.ts:180`)。

### 3.4 deepseek-harness:strict widening 算子

`escalation.ts:18-26` WIDER_MODES:

```typescript
'read-only'    → ['workspace-write', 'danger-full-access']
'workspace-write' → ['danger-full-access']
```

**没有反向**——`workspace-write → read-only` 是禁止的,避免模型"借用"更严的 sandbox 来绕过审批。

### 3.5 shadowed rule detection(所有项目共性)

`claudecode shadowedRuleDetection.ts:1-180` 主动扫描**不可达规则**——"ask" 或 "deny" 规则被前面的 "allow" 规则短路:

```typescript
export type ShadowType = 'ask' | 'deny'
export type UnreachableRule = {
  rule: PermissionRule
  reason: string
  shadowedBy: PermissionRule
  fix: string
}
```

**这功能罕见且宝贵**——`$DENY(dangerous:*)` 在 `Bash` 之前有 `$ASK(Bash:*=*)` 时不可达,UI 必须警告用户"你写的 deny 永远不会触发"。

### 3.6 横向对比:规则匹配能力

| 项目 | 精确 | 前缀 | 通配 | glob path | 跨平台 | shadow 检测 |
|------|------|------|------|-----------|--------|------------|
| claudecode | ✅ | ✅ (`:num`) | ✅ (`*` `?`) | ✅ | Win32 `si` flag | ✅ |
| atomcode | ✅ | n/a (per-call) | ✅ (bash classification) | n/a | case_key `to_lowercase` on Win/macOS | n/a |
| opencode | n/a (whole-rule) | n/a | ✅ (regex) | n/a | Win32 `si` | n/a |
| deepseek | ✅ (exact mode) | n/a (widening) | n/a | n/a | n/a | n/a |
| pi | n/a | n/a | ✅ (`/regex/`) | n/a | n/a | n/a |
| openclaw | n/a | n/a | ✅ (whole-segment `*`) | ✅ | case_key + posix | n/a |
| undici | n/a | n/a | n/a | n/a | n/a | bad-port 集合 |

---

## 4. 决策审计与回放

### 4.1 claudecode:三元组 analytics + DangerousPattern

`src/utils/permissions/permissions.ts:130-150`:

```typescript
logEvent('tengu_tool_decision', {
  tool: tool.name,
  decision: result.behavior,        // allow|deny|ask
  source: matchedRule?.source,      // userSettings|projectSettings|policySettings|...
  rule: permissionRuleValueToString(matchedRule.ruleValue),
  reason: decisionReason?.type,     // rule|classifier|hook|...
  cost: calculateCostFromTokens(...), // 决定成本同时上报
})
```

**7 个字段**进入 analytics,可在 ant-internal dashboard 回看,事后分析"用户拒绝最多的工具/规则"。

`dangerousPatterns.ts:1-80` 维护两个常量:
- `CROSS_PLATFORM_CODE_EXEC` 17 个跨平台解释器/包运行器(python, node, npx, ssh 等)
- `DANGEROUS_BASH_PATTERNS` 在 ant 内额外包含 16 个 `fa run`/`coo`/`gh`/`kubectl`/`aws` 等

`isDangerousBashPermission()`(`permissionSetup.ts:90-130`)在 auto 模式下,任何 allow 规则包含上述 interpreter 前缀,会**降级为 ask**——避免 auto mode 一次性通过 `python:*` 解放所有 python 调用。

### 4.2 deepseek-harness:二元组 session log 配对

`packages/interaction/user-approval/src/index.ts:148-200`:

```typescript
async request(req: ApprovalRequest): Promise<ApprovalOutcome> {
  if (!hasOpenTurn(session.events)) {
    throw new Error('approval.request() outside an open turn: ... must be turn-enclosed (a bare event between turns is crash-tail garbage on reload).')
  }
  const id = ApprovalRequestId(randomUUID())
  session.append('approval/asked', { id, toolName, callId, reason })  // ← 配对 1/2
  const outcome = await this.decide(req, session)
  session.append('approval/decided', { id, outcome })                 // ← 配对 2/2
  return outcome
}
```

**3 个 invariant**(注释明示):
1. **必须 in open turn**——孤立 ask 事件在 reload 时是"crash-tail garbage";
2. **必须 outcome 一定 append**——失败/取消/不可用都 append,留审计;
3. **audit pair 不可分离**——没有 ask 的 decided 或反之都是 corrupt state。

`approvalService.effectivePolicy(session)`(`user-approval/src/index.ts:225-240`)是 **log fold** 模式——policy 状态从 events 推导出,无独立 in-memory 副本,因此 resume 进程无需 catch-up machinery。

### 4.3 atomcode:Kernel Hooks 双端记录

`crates/atomcode-capabilities/src/hooks.rs:14-50` WireLogHooks 在 `on_request` dump 出 `messages` + `tools` + `options` + `round` + `cache_epoch`;`on_model_response` dump assistant message + tool_calls + reasoning + `meta`——**这就是 session 重建的最小信息集**。

`crates/atomcode-telemetry/src/queue/mod.rs:60-200` 维护**append-only NDJSON segment queue**:
- `X.partial` 是活动段,带 `X.partial.owner` marker 标识 lock-aware 版本;
- `X.sending-*` 是 HTTP POST 中的段,意外中断后 reclaim;
- fs2 `try_lock_exclusive` + sentinel 是双轨 lock;
- 旧版本(无 marker)需要 24h 无修改才被回收(跨版本保守策略)。

**这是 laew 缺失的——laew 无任何请求/响应 trace 持久化**。

### 4.4 opencode:Effect pub/sub 事件流

`packages/opencode/src/permission/index.ts:75-95`:

```typescript
yield* events.publish(Event.Asked, info)        // Effect 事件总线
yield* Deferred.succeed(item.deferred, undefined)  // 唤醒 ask 协程
```

`Event.Asked` / `Event.Replied` 是 schema-defined(`packages/schema/src/permission.ts`),通过 `@opencode-ai/core/event` 总线广播,客户端(sessions-ui)订阅后用 `permissions.saved` 表存盘:

`packages/core/src/permission/saved.ts:33-44`:

```typescript
yield* db.insert(PermissionTable).values(input.resources.map(resource => ({
  id: ID.create(), project_id, action, resource,
}))).onConflictDoNothing()
```

**onConflictDoNothing 关键**——同一 project 重复添加同 pattern 不会报错,幂等。PermissionTable 字段:`id, project_id, action, resource`,**4 列**足够审计 + replay。

### 4.5 横向对比:审计深度

| 项目 | 审计字段数 | 持久化层 | replay 幂等 |
|------|----------|---------|------------|
| claudecode | 7 (tool/decision/source/rule/reason/cost/duration) | analytics dashboard | n/a (stateful in-memory) |
| atomcode | 6 (messages/tools/options/round/cache_epoch/meta) | append-only NDJSON + fs2 lock | ✅ (sentinel 双轨) |
| opencode | 4 (id/project/action/resource) | SQLite PermissionTable | ✅ (onConflictDoNothing) |
| deepseek | 4 (id/toolName/callId/reason) | session log 配对 | ✅ (log fold) |
| pi | n/a | n/a | n/a |
| openclaw | gateway-error-details | per-frame log | n/a |
| undici | n/a (per spec) | n/a | n/a |

---

## 5. 多层防御:用户态策略 + 进程级沙箱 + OS 系统调用过滤

### 5.1 三层架构图

```
                    ┌─────────────────────────────────────────┐
   Layer 1          │  用户态策略                              │
  User-Land         │  - 6 源规则 (claudecode)                 │
                    │  - config.toml (opencode)                │
                    │  - PERMISSION_RULE_SOURCES (atomcode)    │
                    │  - sandbox/mode + approval/policy (dsh)  │
                    │  - 敏感路径 / destructive 模式分类        │
                    └────────────┬────────────────────────────┘
                                 │ PermissionDecision ∈ {AllowOnce, AllowAlways, Deny}
                                 ▼
                    ┌─────────────────────────────────────────┐
   Layer 2          │  进程级沙箱                              │
  Process           │  - ApprovalMiddleware / BeforeOutcome     │
                    │  - BashWorkspaceGate / SensitivePathGate │
                    │  - Opencode PermissionV2.Service         │
                    │  - LandlockRunner / bwrap                │
                    │  - BashTool::risk() arg-aware            │
                    └────────────┬────────────────────────────┘
                                 │ 进程 fork + execve
                                 ▼
                    ┌─────────────────────────────────────────┐
   Layer 3          │  OS 系统调用过滤                          │
  OS-Kernel         │  - macOS sandbox-exec (SBPL profile)     │
                    │  - Linux bwrap (--unshare-pid/net/ipc)   │
                    │  - Linux landlock ABI 1-5 (FS access)    │
                    │  - Linux seccomp-bpf (AF_UNIX 拦截)       │
                    │  - Windows WRITE_RESTRICTED + SID grant  │
                    └─────────────────────────────────────────┘
```

### 5.2 各项目层级覆盖

| 项目 | L1 用户态 | L2 进程级 | L3 OS |
|------|----------|---------|-------|
| claudecode | ✅ 6 源 + 6 模式 + dangerousPatterns | ✅ ApprovalMiddleware + 27 PermissionRequest | ✅ SBPL + bwrap + seccomp |
| atomcode | ✅ BashTool::risk + 6 个 gate 中间件 | ✅ ApprovalMiddleware + ToolMiddleware 链 | ❌ 显式声明"embedder's responsibility" |
| opencode | ✅ config.toml + Effect PermissionV2 | ✅ PermissionV2.Service.ask/reply | ❌ 仅协议层 |
| deepseek | ✅ sandboxPolicy + ApprovalPolicy + escalation | ✅ SandboxedFileSystem.fence + SandboxBashExecutor | ✅ bwrap + landlock + Seatbelt + Windows ACL |
| pi | ⚠️ 扩展式 | ⚠️ 委托 sandbox 扩展 | ⚠️ bwrap(扩展式) |
| openclaw | ✅ permissionProfile + IP/Path 策略 | ✅ per-frame gateway-error | ❌ |
| undici | n/a | ❌ | ❌ |

**关键观察**:
- claudecode 是**唯一三层全栈**项目;
- atomcode **L3 是用户职责**(注释明示),但 L2 的 6 个 gate(approval/sensitive_path/write_approval/bash_workspace_gate/credential_bash_gate/atomgit_bash_gate)覆盖细粒度;
- opencode 仅 L1+L2,**靠 Effect 抽象让 L3 可插拔**;
- deepseek 的 L2 + L3 协同是 7 项目中**最严谨**——`fs-sandbox` 用同样的 `writableRoots(policy)` 与 Seatbelt/bwrap/Landlock profile **共享根列表**,**drift 不可能**(测试 pinned)。

### 5.3 atomcode 6 个 gate 的细粒度

| Gate | 工具 | 策略 | 文件 |
|------|------|------|------|
| `ApprovalMiddleware` | all Risky | generic approval round-trip | `approval.rs:78-130` |
| `SensitivePathGate` | read/grep/glob/list | SSH/`.env`/`.aws`/`.ssh` 敏感标记 → 每次 prompt | `sensitive_path.rs:1-180` |
| `WriteApprovalGate` | edit/write/search_replace/parallel_edit | workspace 边界 + sensitive(每次) + dir-scope(always) | `write_approval.rs:1-200` |
| `BashWorkspaceGate` | bash destructive | 越 workspace 边界 + 每次 prompt | `bash_workspace_gate.rs:1-300` |
| `CredentialBashGate` | bash | 12 credential 标识 × 24 network commands → fail-closed | `credential_bash_gate.rs:1-200` |
| `AtomgitBashGate` | bash | api.atomgit.com / `$atomgit_token` → fail-closed | `atomgit_bash_gate.rs:1-120` |

**关键设计**:6 个 gate **共享 `PermissionStore`**——一次 `AllowAlways` 决定 6 个 gate 都生效(若适用)。`InMemoryPermissionStore`(`approval.rs:120-150`)是默认实现,支持 `Poison-recover`(unlock 后 `into_inner()` 取出,不让 mutex 污染中断整次 tool call)。

---

## 6. OS 沙箱集成字节级深挖

### 6.1 macOS SBPL(Chrome 同源策略)

`node_modules/@anthropic-ai/sandbox-runtime/dist/sandbox/macos-sandbox-utils.js:215-340` 生成完整 SBPL profile,从 `'(version 1)'` 起到 60+ 个 `(allow sysctl-read (sysctl-name "..."))` 显式 allow——**逐项白名单**是 Apple 推荐的姿势。

**关键提取**:
- `'(allow process-exec)'` `'(allow process-fork)'`——子进程能力放行
- `'(allow process-info* (target same-sandbox))'`——同沙箱内可查
- `'(allow signal (target same-sandbox))'`——同沙箱内可发信号
- `'(allow mach-lookup ...)'`——列 14 个 `com.apple.*` 服务的 global-name,**没有 wildcard**——这是 Apple 强烈建议
- `'(deny default (with message "${logTag}"))'`——**默认 deny** 加 logTag 字符串嵌入,每次 deny 时 log 显示 `LogTag: CMD64_xxx_END_xxx_SBX`
- `'(deny file-write-unlink (subpath /path))'`——**block move attack**:既 deny 写入也 deny unlink + literal ancestor,防止 `mv` 绕过 deny

### 6.2 Linux bwrap + socat + seccomp

`linux-sandbox-utils.js:431-480` `buildSandboxCommand()`:

```javascript
const socatCommands = [
  `socat TCP-LISTEN:3128,fork,reuseaddr UNIX-CONNECT:${httpSocketPath} >/dev/null 2>&1 &`,
  `socat TCP-LISTEN:1080,fork,reuseaddr UNIX-CONNECT:${socksSocketPath} >/dev/null 2>&1 &`,
  'trap "kill %1 %2 2>/dev/null; exit" EXIT',
]
// 应用 seccomp + exec 用户命令
const applySeccompCmd = shellquote.quote([applySeccompBinary, seccompFilterPath, shellPath, '-c', userCommand])
```

**3 层套娃**:
1. **外层 bwrap** 提供 `/`、`/dev`、`/proc`,默认 deny 网络;
2. **中间 socat** 暴露 3128/1080 端口,把流量通过 Unix socket 转发到 host 端 HTTP/SOCKS proxy;
3. **apply-seccomp** 应用 BPF filter,`PR_SET_NO_NEW_PRIVS` + `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER)`——只允许 AF_UNIX socket 之外的 socket 家族。

**seccomp BPF filter 限制**:32-bit x86 的 `socketcall` 单一 syscall 多路复用所有 socket 操作,**当前 filter 仅 block `socket(AF_UNIX, ...)`**,会**漏 socketcall 绕过**——`generate-seccomp-filter.js:60-75` 注释明示"32-bit x86 not currently supported"。

### 6.3 Linux Landlock 字节级

`native/landlock-run/packages/entry/src/main.c:1-298` 是**298 行纯 C11**:

```c
// 自含 Landlock UAPI(不依赖 <linux/landlock.h>)
struct landlock_ruleset_attr { uint64_t handled_access_fs; };
struct landlock_path_beneath_attr { uint64_t allowed_access; int32_t parent_fd; } __attribute__((packed));

#define LL_FS_EXECUTE     (UINT64_C(1) << 0)   // ABI 1
#define LL_FS_WRITE_FILE  (UINT64_C(1) << 1)
... (16 个 access bit,见 main.c:69-83)

#define MAX_ABI 5L
```

**probe 模式**(`main.c:259-275`):

```c
if (cli.probe) {
  static const char *probe_root = "/";
  struct cli probe = { .ro = &probe_root, .ro_count = 1 };
  int partial = 0;
  code = restrict_self(&probe, &partial);
  if (code != 0) return code;
  printf("landlock: %s\n", partial ? "partially enforced (older ABI)" : "fully enforced");
  return 0;
}
```

**probe = 实际 restrict_self**——`--version` 风格检查会漏"有 syscall 但不 enforce",**真 restrict 是唯一诚实信号**。

**fs_mask_for_abi 协商**(`main.c:184-190`):

```c
static uint64_t fs_mask_for_abi(long abi) {
  uint64_t mask = LL_ABI1_MASK;          // bits 0..12
  if (abi >= 2) mask |= LL_FS_REFER;     // ABI 2 加 refer (hard link)
  if (abi >= 3) mask |= LL_FS_TRUNCATE;  // ABI 3 加 truncate
  if (abi >= 5) mask |= LL_FS_IOCTL_DEV; // ABI 5 加 ioctl
  return mask;
}
```

**add_rule 中 stat 修正**(`main.c:201-208`):

```c
struct stat st;
if (fstat(path_fd, &st) == 0 && !S_ISDIR(st.st_mode)) {
  // file grant 只能 file-compatible bits
  access &= LL_FS_EXECUTE | LL_FS_WRITE_FILE | LL_FS_READ_FILE | LL_FS_TRUNCATE | LL_FS_IOCTL_DEV;
}
```

这就是 `--rw /dev/null` 为何 work——kernel 拒绝 directory-only access 在 non-directory rule(EINVAL),所以 file grant 自动剪到 file bits。

**fail-closed 全程**:`main.c:217-220` 创建 ruleset 失败时 ENOSYS(无 syscall)/ EOPNOTSUPP(被 disable)→ `landlock-run: landlock is not enforced by this kernel (ABI unsupported or disabled)` + exit 125,**绝不 exec unconfined**。

**why 不用 seccomp**:`main.c:50-70` 注释明示——Landlock 是 path-beneath 风格的 FS access 控制,**不阻塞 socket/进程**——`bwrap` + seccomp 才是完整组合,Landlock 是 fallback(`bwrap` 不可用时:unprivileged user namespace disabled、LSM 拒绝 mount)。

### 6.4 Windows WRITE_RESTRICTED + workspace-SID

`packages/sandbox/sandbox-windows-acl/src/workspace-sid.ts:18-30`:

```typescript
export function workspaceWriteSid(workspaceRoot: string): string {
  const digest = createHash('sha256').update(workspaceRoot, 'utf8').digest()
  const first = (digest.readUInt32LE(0) % (2 ** 30 - 1)) + 1
  const second = (digest.readUInt32LE(4) % (2 ** 30 - 1)) + 1
  return `S-1-4-${first}-${second}`
}
```

**per-workspace 确定性 SID**:
- 输入:workspace 路径(必须 `realpathSync.native` 后,见 `index.ts:1-50` 注释)
- 输出:`S-1-4-x-y` 自含 SID 字符串
- 关键:每个 workspace **固定 SID**——同一 workspace 跨 session 复用,**standing ACE 一次性建好**,后续 provision O(1) 命中 exact-ACE skip 缓存

`tempWriteSid(tempDir)`(`workspace-sid.ts:36-43`):per-temp-dir 随机 SID,**与 workspace 域分离**(3 段 vs 2 段,带 `1` 末段),防止 sibling session 写别的 session 的 temp 树。

`assertTempRootOutsideWorkspace(workspaceRoot, tempRoot)`(`path-boundary.ts:14-23`)**fail-closed 检查**——temp 必须在 workspace 外,否则 throw `Windows ACL temp root must be outside the workspace`。

**known boundaries**(`index.ts:35-50` 注释明示):
- WRITE_RESTRICTED 限制 write,read/network/process visibility **不受限**;
- console 隔离**不可用**——`CREATE_NO_WINDOW`/`CREATE_NEW_CONSOLE` 在 restriction 下 `STATUS_DLL_INIT_FAILED`;
- private temp dir **必须 caller-owned**;
- workspace grants **永不 revoke**(standing ACE 是 cross-session 缓存);
- temp grants **必须 revoke on dispose**(`dispose()` 中)。

### 6.5 字节级对比:各 OS 沙箱

| OS | 机制 | 字节数 | fail-closed | partial 报告 |
|----|------|--------|-------------|--------------|
| macOS | sandbox-exec SBPL | 629 (claudecode 整个 macos-sandbox-utils.js) | ✅ `deny default` | n/a |
| Linux bwrap | --bind/--unshare | 874 | ✅ 缺 bwrap binary 报错 | n/a |
| Linux landlock | path-beneath ABI 1-5 | 298 (pure C) | ✅ 缺 syscall / disabled 退出 125 | ✅ partial on older ABI |
| Linux seccomp-bpf | BPF filter | 262 (claudecode 生成器) | ✅ 32-bit x86 unsupported | n/a |
| Windows | WRITE_RESTRICTED + SID | 431 (windows-acl index) | ✅ Win32 错误 throw | ✅ partial(Everyone + hard link) |

---

## 7. 细粒度控制

### 7.1 网络出口

**claudecode 三层**:
- `NetworkConfigSchema.allowedDomains` / `deniedDomains`(`sandbox-config.js:48-92`)——域 allowlist,`*.example.com` 通配,**禁止 `*` 或 `*.com`** (过宽);
- `allowUnixSockets` (macOS only)——允许特定 Unix socket path;Linux 注释明示 "seccomp cannot filter by path";
- `mitmProxy.socketPath` + `domains: string[]`——特定域路由到上游 MITM proxy via Unix socket;
- HTTP proxy (port 3128) + SOCKS proxy (port 1080) 由 socat 在 sandbox 内起,**所有出栈流量都过 proxy**——`getMitmSocketPath()`(`sandbox-manager.js:73-90`)根据域名匹配决定路由;

`http-proxy.js` + `socks-proxy.js` + `sandbox-manager.js:84-100` 的 `filterNetworkRequest(port, host, askCallback)` 实现 ask 流程:**先看 denied,再 allowed,最后 askCallback**。

**openclaw SSRF 段**(`packages/net-policy/src/ip.ts:46-90`):
- 28 个 IPv4 段(unspecified/broadcast/multicast/linkLocal/loopback/carrierGradeNat/private/...)
- 13 个 IPv6 段(unspecified/loopback/linkLocal/uniqueLocal/multicast/reserved/benchmarking/...)
- 100.100.100.200 (Aliyun) + fd00:ec2::254 (AWS)——云 metadata IP 硬编码
- RFC 2544 198.18.0.0/15 benchmark——Sing-box/Clash fake-IP 用,**opt-in 放行**

**deepseek WIDER_MODES 升档**——从 read-only 主动要求 workspace-write 时,必须**经 ApprovalService.request** 双 audit event 落 session log。

### 7.2 文件 R/W/D

**claudecode 7 维**:
- 路径:glob + subpath + canonicalize 三重(`macos-sandbox-utils.js:11-100`);
- DANGEROUS_FILES 静态列表 + DANGEROUS_DIRECTORIES 子树扫描 `DEFAULT_MANDATORY_DENY_SEARCH_DEPTH = 3`;
- `.git/hooks` 永远 deny(security);
- `.git/config` 可选 deny(allowGitConfig 开关);
- 父目录保护:`getAncestorDirectories()` 防 `mv` 绕过——把 ancestor dirs 也 deny unlink;
- symlink 检测:`findSymlinkInPath` 查 `allowedWritePaths` 内的 symlink,reject 逃逸;
- `hasFileAncestor` 检测 `.git` 是 file 而非 dir——`.git/hooks` 不存在时,`--ro-bind /dev/null` mount 不必要;

**opencode**:仅 read/edit/external_directory/todowrite 5 维,远简。

**atomcode WriteApprovalGate 4 维**(in-workspace/out-workspace + sensitive/non-sensitive)→ 4 cell 决策表:
- in-workspace + non-sensitive: Proceed;
- in-workspace + sensitive: AskEveryTime(never remembered);
- out-workspace + non-sensitive: Ask + dir-scope Always;
- out-workspace + sensitive: AskEveryTime;

### 7.3 进程 fork/exec

**macOS**:`(allow process-exec)` + `(allow process-fork)`——子进程能力放行,但 signal 仅 same-sandbox 内(防 cross-sandbox 攻击)。

**Linux bwrap**:`--die-with-parent` 父进程退出时子进程全 kill;`--unshare-pid` PID namespace 隔离,沙箱内 PID 1 = bash。

**Linux Landlock**:**不限制进程**——只控 FS 访问(`main.c:50-70` 注释)。

**Windows WRITE_RESTRICTED**:**不限制进程可见性**——token restrict 仅写操作受影响,read/network/process 仍走 ambient identity。

**atomcode atomgit_bash_gate + credential_bash_gate**:对特定进程行为 deny(`raw AtomGit API` / `curl/wget/Invoke-WebRequest`),而非全部。

### 7.4 GPU/IO 资源

**claudecode**:无显式 GPU/IO rlimit——bwrap 默认不限,但 model API call 有 `anthropic-version` rate limit 在外层。

**atomcode bash.rs:120-160**:`timeout` 60s 默认 300s 上限,无 IO/CPU rlimit。

**opencode Effect**:无内建 rlimit,依赖 bwrap `--rlimit-*`(未用)。

**deepseek**:bwrap `--rlimit-nofile=1024` 之类无显式,默认用系统。

**pi**:无。

**关键发现:7 项目**全部**没有 GPU rlimit**——CUDA/cuDNN/tensor 资源无限使用。如果用 `laew -p "训练 YOLO 模型"` 会独占 GPU,无法限制——这是 laew 沙箱化路线的**已知缺口**。

---

## 8. HITL:交互式 TUI 弹窗、二次确认、超时

### 8.1 claudecode:27 种 Request + 3 阈值降级

`src/components/permissions/` 列出 27 个 Request 屏:AskUserQuestion / Bash / ComputerUse / EnterPlanMode / ExitPlanMode / Fallback / FileEdit / FilePermissionDialog / Filesystem / FileWrite / NotebookEdit / PermissionDialog / PermissionExplanation / PermissionPrompt / PermissionRequest / PermissionRequestTitle / PermissionRuleExplanation / PowerShell / Sandbox / SedEdit / Skill / WebFetch / WorkerBadge / WorkerPendingPermission / useShellPermissionFeedback 等。

`denialTracking.ts:1-45`:

```typescript
export const DENIAL_LIMITS = {
  maxConsecutive: 3,  // 连续 3 次 deny → fall back to prompt
  maxTotal: 20,        // 总共 20 次 deny → fall back to prompt
}
```

**3 阈值降级策略**——classifier 连续否认 3 次或总共 20 次,放弃 classifier 转回 prompt,**防止"AI 越否决越多"的死循环**。

`bypassPermissionsKillswitch.ts:14-46` 启动时 check Statsig `disable_bypass_permissions` 远程门,**enterprise IT 可远程关闭 bypass 模式**。

### 8.2 atomcode:AskUserQuestion 屏

`crates/atomcode-capabilities/src/tools/request_user_input.rs:629-1115` 是**结构化问题**屏,支持:
- 单选 (header 1-12 chars)
- 多选 (min 1, max 4 options)
- 自由文本 (multiline)
- 组合多个 questions 单屏

`REQUEST_USER_INPUT_KIND` 是与 approval 平级的 wire kind,通过 `permission_bridge.rs:1-127` 路由 session_id → decider → response_tx。

### 8.3 deepseek:answerer chain 4 outcome

`user-approval/src/index.ts:148-200`:

```typescript
const outcome = await this.decide(req, session)
session.append('approval/decided', { id, outcome })  // 一定 append
return outcome
```

**4 outcome**:`'allowed-once' | 'rejected' | 'cancelled' | 'unavailable'`,每个都落 audit event。`'unavailable'` = answerer 不可用(crash/throwing)→ **fail closed to rejected**(注释明示)。

**顺序路由**:`ctx.inject(['approval'], ...)` 注册 answerers,通过 `events.dispatch('approval/ask', req)` 让 answerer 链 claim 或 next(),scope-filtered。

### 8.4 opencode:WorkerPendingPermission 主从架构

`src/components/permissions/WorkerPendingPermission.tsx` 是**主从架构**:
- Worker 进程(实际 LLM agent)→ `Bridge` → 主进程 UI 弹窗;
- 用户点击 once/always/reject → 通过 `requestPermission()`(ACP 协议)`acp/permission.ts:1-50` 回 Worker;
- queue per session:`this.queues.set(sessionId, next)` 防并发弹窗冲突。

`packages/opencode/src/acp/permission.ts:30-50` 队列 + tryGet + reply 模式是**最清晰的主从通信范本**。

### 8.5 timeout 与取消

| 项目 | timeout 默认 | timeout 上限 | 取消语义 |
|------|------------|------------|---------|
| claudecode | 120s | 600s | Ctrl-C + abort signal |
| atomcode bash | 60s | 300s | abort signal + tokio |
| opencode | 120s | 600s | forceKillAfter 3s |
| deepseek | per-call | per-call | AbortSignal |
| pi | 无 | 2^31-1ms | AbortSignal |
| openclaw | per-policy | per-policy | AbortSignal |

---

## 9. 横向对比大表

| 维度 | claudecode | atomcode | opencode | deepseek | pi | openclaw | undici |
|------|-----------|----------|----------|----------|------|----------|--------|
| **三态状态机** | 6 态+Statsig gate | 3 态+4 BeforeOutcome | 3 态+findLast 优先级 | 3 态+strict widening | 2 态扩展式 | 2 态 SSRF/path | 89 端口集合 |
| **规则源数** | 6 (user/project/local/flag/policy/command) | 1 + 6 gate 中间件 | config.toml | session event log | 扩展注册 | permission profile | n/a |
| **first-match** | ✅ 4 维 | ✅ per-gate | ✅ findLast | ✅ strict widen | ✅ regex | ✅ glob | ✅ port set |
| **shadow 检测** | ✅ shadowedRuleDetection | n/a | n/a | n/a | n/a | n/a | n/a |
| **审计字段** | 7 | 6 (hook 端) | 4 (DB) | 4 (log 配对) | 0 | per-frame | 0 |
| **持久化** | analytics | NDJSON + fs2 lock | SQLite + ON CONFLICT | session log 配对 | 扩展 | gateway-error | n/a |
| **L1 用户态** | ✅ 6 源 | ✅ 6 gate | ✅ Effect Service | ✅ dual axis | ⚠️ 扩展 | ✅ SSRF/path | n/a |
| **L2 进程级** | ✅ ApprovalMW + 27 Req | ✅ 6 gate | ✅ PermissionV2 | ✅ fence | ⚠️ 扩展 | ✅ per-frame | n/a |
| **L3 OS 沙箱** | ✅ SBPL+bwrap+seccomp | ❌ embedder 职责 | ❌ | ✅ 4 runner | ⚠️ 扩展 | ❌ | ❌ |
| **macOS SBPL** | ✅ 60+ sysctl 白名单 | n/a | n/a | ✅ Seatbelt | n/a | n/a | n/a |
| **Linux bwrap** | ✅ | n/a | n/a | ✅ 首选 | 扩展式 | n/a | n/a |
| **Linux landlock** | ❌ (seccomp 替代) | n/a | n/a | ✅ fallback | n/a | n/a | n/a |
| **Linux seccomp-bpf** | ✅ apply-seccomp | n/a | n/a | ❌ | n/a | n/a | n/a |
| **Windows ACL** | ❌(主走 SBPL/bwrap) | n/a | n/a | ✅ WRITE_RESTRICTED | n/a | n/a | n/a |
| **网络细粒度** | domain allow/deny + MITM socket + HTTP/SOCKS proxy | 12 cred × 24 net cmd | external_directory | n/a | n/a | 28 IPv4 + 13 IPv6 + metadata | bad-ports 89 个 |
| **文件细粒度** | glob+subpath+canonical+ancestor | 4 cell 决策表 | 5 key | writableRoots 共享 | 0 | whole-segment glob | n/a |
| **进程细粒度** | process-exec/fork/signal (target same-sandbox) | n/a (工具级) | n/a | n/a | n/a | n/a | n/a |
| **GPU/IO rlimit** | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | n/a |
| **HITL 屏数** | 27 (含 Fallback/Skill/Worker) | 1 (AskUserQuestion) | 1 (WorkerPending) | answerer chain | `ctx.ui.select` | gateway-client | n/a |
| **denial 降级** | 3 阈值(3/20) | n/a | n/a | 4 outcome | n/a | n/a | n/a |
| **远程 kill-switch** | ✅ Statsig gate | n/a | n/a | sandbox/approval log 改 | n/a | n/a | n/a |
| **子代理权限继承** | n/a (无 SubAgent) | n/a (无 SubAgent) | ✅ deriveSubagentSessionPermission | delegation source 标记 | n/a | n/a | n/a |

---

## 10. 共性模式:claudecode 6 源规则 → atomcode 三态机 → pi binary lock 借鉴链

### 10.1 三种代表性范式

**范式 A:多源配置合并(以 claudecode 为代表)**
- 6 源规则,后者覆盖前者,policy 是最高;
- 用于 **多用户多场景** 产品(SMB/Enterprise/CLI flag/session override 都得照顾);
- 优点:灵活;缺点:用户难理解优先级。

**范式 B:中间件链(以 atomcode 为代表)**
- 6 个独立 gate 中间件,按注册顺序执行,共享 `PermissionStore`;
- 用于 **单进程多角色** 产品(同 session 内 coder/reviewer/plan 模式并存);
- 优点:关注点分离;缺点:顺序敏感,需在每个 gate 注释明示。

**范式 C:Widening Algebra(以 deepseek 为代表)**
- `read-only → workspace-write → danger-full-access` 严格 widening,升级必须 audit;
- 用于 **多租户/SaaS** 产品(管理员可远程 set,模型可申请升档,两者都有 audit);
- 优点:可推理、可证明;缺点:UI 复杂。

### 10.2 借鉴链

```
claudecode 6 源 ──────→ opencode config.toml 1 源 ──→ pi 扩展 0 源
       │                       │                          │
       │ 简化                  │ 简化                      │
       ▼                       ▼                          ▼
    atomcode 6 gate       deepseek 1 axis              laew 0 源
    (中间件链)             (widening)                  (需从 0 设计)
```

**laew 应采纳**:
- **L1**:deepseek 双轴 (`sandbox mode` + `approval policy`) + claudecode multi-source 简化版(`user` + `project` 2 源足够起步);
- **L2**:atomcode ToolMiddleware 链 + claudecode 27 PermissionRequest 屏(从 `BashPermissionRequest`/`WritePermissionRequest`/`FileEditPermissionRequest` 3 个核心开始);
- **L3**:deepseek LocalSandboxProvider 4 runner 设计(bwrap 首选 + landlock fallback + Seatbelt 唯一 + windows-acl 唯一),**单一 `writableRoots(policy)` 共享**——避免 drift。

### 10.3 5 大共性 invariant

1. **default-deny**(7 项目一致):规则不命中 → ask 或 deny;
2. **fail-closed 解析**(atomcode/deepseek):任何未知 JSON/字段 → deny;
3. **audit pair 完整**(deepseek/atomcode):ask + decision 必须都落;
4. **strict widening 不可逆**(deepseek):read-only → workspace-write OK,反向 throw;
5. **per-workspace 独立 state**(deepseek/windows-acl/atomcode):每个 workspace 一个 SID/grants,跨 session 复用。

---

## 11. laew P0/P1/P2 路线图

### P0 (Week 1-2,基础规则引擎)

**目标**:`Write/Edit/Bash/Read` 工具增加 `PERMISSION_RULE_SOURCES` 配置 + ToolMiddleware 链。

**Rust crate**:
```toml
[dependencies]
landlock = "0.4"  # path-beneath FS access
seccompiler = "0.4"  # BPF filter 编译
capsicum = "0.1"  # FreeBSD capability mode (可选)
```

**模块设计**:
```rust
// src/agent/permission/mod.rs
pub enum Action { AllowOnce, AllowAlways, Deny }
pub trait PermissionStore: Send + Sync {  // 镜像 atomcode
  fn is_granted(&self, key: &str) -> bool;
  fn grant(&self, key: &str);
}
pub struct Rule { pub tool: String, pub pattern: String, pub action: Action }
pub struct PermissionEngine {
  pub rules: Vec<Rule>,
  pub store: Arc<dyn PermissionStore>,
}
impl PermissionEngine {
  pub fn evaluate(&self, tool: &str, args: &str) -> Action {
    for rule in &self.rules {  // first-match
      if wildcard_match(tool, &rule.tool) && glob_match(args, &rule.pattern) {
        return match rule.action {
          Action::Deny => Action::Deny,
          Action::AllowOnce => Action::AllowOnce,
          Action::AllowAlways if self.store.is_granted(&format!("{}::{}", tool, args)) => Action::AllowOnce,
          Action::AllowAlways => Action::AllowAlways,
        }
      }
    }
    Action::AllowOnce  // ask 用户
  }
}
```

**关键参考**:
- atomcode `PermissionStore` + `PermissionDecision` + `InMemoryPermissionStore`(`approval.rs:78-150`);
- claudecode `Wildcard.match` + first-match(`shellRuleMatching.ts:140-180`);
- deepseek `approval/asked` + `approval/decided` 配对(`user-approval/src/index.ts:148-200`)——laew 应在 SQLite `permission_log` 表落 audit pair。

**P0 集成**:`src/agent/sandbox_hook/` 改为 `PermissionEngine` 驱动,把 `Write/Edit` 工具的 `SandboxConfig` 路径白名单作为 default `Deny` 规则起点。

### P1 (Week 3-4,OS 沙箱集成)

**目标**:Linux 首选 bwrap,fallback Landlock;macOS Seatbelt;Windows skip。

**Rust crate 用法**:
```rust
// bwrap via Command::new
let bwrap = Command::new("bwrap")
  .args(&["--ro-bind", "/", "/", "--dev", "/dev",
          "--unshare-pid", "--proc", "/proc",
          "--die-with-parent",
          "--bind", workspace_root, workspace_root,
          "--", "bash", "-c", &command])
  .spawn()?;

// landlock 直接 syscall
use landlock::{Ruleset, RulesetAttr, Access, path_beneath_rules};
let abi = Ruleset::new().handle_access(Access::FS_EXECUTE | Access::FS_READ_FILE)?;
abi.add_rule(path_beneath_rules(&[workspace_root], Access::FS_READ_FILE))?;
abi.restrict_self()?;
```

**关键参考**:
- deepseek `landlock-run/main.c:1-298` + `packages/sandbox/sandbox-local/profiles.ts:1-90`——**bwrap + landlock profile 共享 writableRoots**;
- claudecode `linux-sandbox-utils.js:480-600`——bwrap + socat + seccomp 三层;
- deepseek `workspace-sid.ts:1-50`——per-workspace 确定性 SID 思路(laew 可类比为 `~/.config/laew/sandboxes/<workspace-hash>/` 缓存)。

### P2 (Week 5-8,完整 HITL + 审计 + 子代理继承)

**目标**:
- TUI 弹窗(`/provider` 风格 4 Tab 表单 + 新增 `/permission` 子屏);
- SQLite `permission_log(id, session_id, tool, args, action, source, ts)`;
- 子代理权限继承(`deriveSubagentSessionPermission` 模式,opencode `opencode/src/agent/subagent-permissions.ts:1-30`);
- denial 降级阈值(`claudecode denialTracking.ts DENIAL_LIMITS`);
- seccomp-bpf 收口(屏蔽 `socket(AF_UNIX, ...)` for sandbox 沙箱内 proxy);

**关键参考**:
- opencode `opencode/src/permission/index.ts:48-95`——first-match + Effect 事件总线 + `permissions.saved` 表 + `Event.Asked/Replied`;
- atomcode 6 个 gate——逐个 port 到 laew(`CredentialBashGate` 是高 ROI);
- deepseek `WIDER_MODES`——laew 暂时 2 态(`auto` / `manual`)足够,future 加 `plan` 态;

### 优先级与代码量

| Phase | 代码量(行) | 关键文件 | 风险 |
|-------|----------|---------|------|
| P0 | ~500 | `src/agent/permission/{mod,store,rule,engine}.rs` | 低 (纯 Rust 逻辑) |
| P1 | ~800 | `src/agent/sandbox/{bwrap,landlock,seatbelt}.rs` + Cargo.toml 加 `landlock`/`seccompiler` | 中 (依赖系统工具链) |
| P2 | ~1200 | `src/agent/permission/{tui,audit,subagent}.rs` + TUI 屏 + SQLite migration | 中-高 (UI + 持久化) |

---

## 12. 关键代码路径速查表

| 项目 | 文件 | 行 | 内容 |
|------|------|---|------|
| claudecode | `node_modules/@anthropic-ai/sandbox-runtime/dist/sandbox/macos-sandbox-utils.js` | 215-340 | SBPL profile 生成(60+ sysctl) |
| claudecode | `node_modules/@anthropic-ai/sandbox-runtime/dist/sandbox/linux-sandbox-utils.js` | 431-480 | bwrap + socat + apply-seccomp 三层 |
| claudecode | `node_modules/@anthropic-ai/sandbox-runtime/dist/sandbox/generate-seccomp-filter.js` | 60-75 | 32-bit x86 socketcall 警告 |
| claudecode | `node_modules/@anthropic-ai/sandbox-runtime/dist/sandbox/sandbox-config.js` | 48-92 | NetworkConfigSchema + domainPattern |
| claudecode | `src/utils/permissions/PermissionMode.ts` | 24-78 | 6 态 PERMISSION_MODE_CONFIG |
| claudecode | `src/utils/permissions/permissions.ts` | 113-273 | applyPermissionRulesToPermissionContext |
| claudecode | `src/utils/permissions/shellRuleMatching.ts` | 75-180 | matchWildcardPattern + escape |
| claudecode | `src/utils/permissions/shadowedRuleDetection.ts` | 1-180 | Unreachable rule 扫描 |
| claudecode | `src/utils/permissions/denialTracking.ts` | 1-45 | DENIAL_LIMITS 3/20 阈值 |
| claudecode | `src/utils/permissions/dangerousPatterns.ts` | 1-80 | CROSS_PLATFORM_CODE_EXEC 17 + DANGEROUS_BASH 16 |
| claudecode | `src/utils/permissions/bypassPermissionsKillswitch.ts` | 14-46 | Statsig 远程 kill-switch |
| claudecode | `src/settings/constants.ts` | 7-18 | SETTING_SOURCES 6 源 |
| atomcode | `crates/atomcode-capabilities/src/tools/approval.rs` | 78-150 | PermissionDecision + InMemoryPermissionStore |
| atomcode | `crates/atomcode-capabilities/src/tools/sensitive_path.rs` | 1-180 | 25 个 SENSITIVE_MARKERS + .env 特判 |
| atomcode | `crates/atomcode-capabilities/src/tools/write_approval.rs` | 1-200 | 4 cell 决策表 |
| atomcode | `crates/atomcode-capabilities/src/tools/bash_workspace_gate.rs` | 1-300 | 越 workspace 边界 |
| atomcode | `crates/atomcode-capabilities/src/tools/credential_bash_gate.rs` | 1-200 | 12 cred × 24 net cmd |
| atomcode | `crates/atomcode-capabilities/src/tools/atomgit_bash_gate.rs` | 1-120 | AtomGit API deny |
| atomcode | `crates/atomcode-capabilities/src/tools/bash.rs` | 147-160 | BashTool::risk arg-aware |
| atomcode | `crates/atomcode-capabilities/src/tools/bash.rs` | 1660-2080 | 16 destructive 模式 |
| atomcode | `crates/atomcode-kernel/src/middleware.rs` | 71-110 | BeforeOutcome 4 态 |
| atomcode | `crates/atomcode-kernel/src/tool.rs` | 40-60 | RiskLevel Safe/Risky 声明 |
| atomcode | `crates/atomcode-cli/src/acp/permission.rs` | 1-294 | ACP wire 转换 + outcome_to_decision |
| atomcode | `crates/atomcode-cli/src/acp/turn.rs` | 1-160 | TurnWire trait + approval 路由 |
| atomcode | `crates/atomcode-daemon/src/permission_bridge.rs` | 1-127 | session_id → decider 路由 |
| atomcode | `crates/atomcode-telemetry/src/queue/mod.rs` | 60-200 | append-only NDJSON + fs2 lock |
| opencode | `packages/core/src/v1/permission.ts` | 1-50 | PermissionV1 + 3 错误类 |
| opencode | `packages/core/src/v1/config/permission.ts` | 1-50 | Action + Object + Rule schema |
| opencode | `packages/core/src/util/wildcard.ts` | 5-15 | Wildcard.match 跨平台 |
| opencode | `packages/opencode/src/permission/index.ts` | 48-95 | evaluate() findLast + Event pub/sub |
| opencode | `packages/opencode/src/permission/evaluate.ts` | 1-30 | re-export |
| opencode | `packages/opencode/src/agent/subagent-permissions.ts` | 1-30 | deriveSubagentSessionPermission |
| opencode | `packages/opencode/src/acp/permission.ts` | 1-100 | ACP 弹窗 + queue 防并发 |
| opencode | `packages/core/src/permission/saved.ts` | 1-79 | PermissionSaved SQLite service |
| opencode | `packages/server/src/handlers/permission.ts` | 1-90 | HTTP API |
| deepseek | `packages/sandbox/sandbox/src/index.ts` | 1-178 | SandboxMode + SandboxProvider seam |
| deepseek | `packages/sandbox/sandbox/src/escalation.ts` | 1-189 | WIDER_MODES + approveEscalation + 4 outcome |
| deepseek | `packages/sandbox/sandbox/src/roots.ts` | 1-55 | writableRoots(policy) 共享 |
| deepseek | `packages/sandbox/sandbox-policy/src/index.ts` | 1-180 | SandboxPolicyService + renderPolicyContext |
| deepseek | `packages/sandbox/sandbox-policy/src/session-mode.ts` | 1-80 | effectiveSandboxMode log fold |
| deepseek | `packages/sandbox/sandbox-local/src/index.ts` | 1-580 | LocalSandboxProvider + 4 runner chain |
| deepseek | `packages/sandbox/sandbox-local/src/profiles.ts` | 1-90 | bwrapProfileArgs + landlockProfileArgs + seatbeltProfileArgs |
| deepseek | `packages/sandbox/sandbox-windows-acl/src/index.ts` | 1-431 | AclSandbox + workspace SID |
| deepseek | `packages/sandbox/sandbox-windows-acl/src/workspace-sid.ts` | 1-50 | workspaceWriteSid + tempWriteSid |
| deepseek | `packages/sandbox/sandbox-windows-acl/src/path-boundary.ts` | 1-90 | canonical 包含检查 + assertTempRootOutsideWorkspace |
| deepseek | `packages/interaction/user-approval/src/index.ts` | 1-312 | ApprovalService + audit pair |
| deepseek | `packages/interaction/permission-presets/src/index.ts` | 1-449 | PermissionPresetService + KnobState |
| deepseek | `packages/fs/fs-sandbox/src/index.ts` | 1-147 | SandboxedFileSystem + fence |
| deepseek | `packages/fs/fs-sandbox/src/containment.ts` | 1-76 | isPathUnder lexical + identity |
| deepseek | `packages/shell/bash-sandbox/src/index.ts` | 1-200 | SandboxBashExecutor 包装 |
| deepseek | `packages/shell/bash-sandbox/src/helpers.ts` | 1-200 | classifyDenial + classifyRunnerFailure |
| deepseek | `native/landlock-run/packages/entry/src/main.c` | 1-298 | 298 行 C11 landlock-run launcher |
| deepseek | `native/landlock-run/packages/entry/src/index.ts` | 1-100 | JS API + grantArgs + LAUNCHER_FAILURE_EXIT=125 |
| pi | `packages/coding-agent/examples/extensions/permission-gate.ts` | 1-30 | 最小可行 permission-gate 范本 |
| pi | `packages/coding-agent/src/bun/restore-sandbox-env.ts` | 1-50 | /proc/self/environ sandbox env 恢复 |
| pi | `packages/coding-agent/examples/extensions/sandbox/index.ts` | 1-80 | SandboxManager + bwrap 集成 |
| pi | `packages/coding-agent/src/core/agent-session.ts` | 215-220 | allowedToolNames + excludedToolNames |
| openclaw | `packages/acp-core/src/types.ts` | 76-77 | permissionProfile field |
| openclaw | `packages/net-policy/src/ip.ts` | 1-300 | SSRF 28+13 IP 段 + RFC 2544 + metadata |
| openclaw | `packages/media-core/src/inbound-path-policy.ts` | 1-90 | whole-segment glob path policy |
| undici | `lib/web/fetch/util.js` | 219-234 | corsCheck + TAOCheck + CORP (stubs) |
| undici | `lib/web/fetch/constants.js` | 14-22 | 89 bad-ports 集合 |
| undici | `lib/web/fetch/index.js` | 583-586 | requestBadPort 强制检查 |
| undici | `lib/web/fetch/request.js` | 16-340 | credentials mode + Request 构造 |

---

## 13. 附录:第七轮已覆盖、本专题不重复的对照

| 主题 | 第七轮覆盖章节 | 本专题新切入 |
|------|--------------|------------|
| Bash 进程生命周期 | Bash命令执行专题 §2.1-2.6 完整 1206 行 | 仅在沙箱集成层引用,提供 `bwrap + socat + apply-seccomp` 字节级 5.1-5.5 |
| Sensitive 路径黑名单 | 权限管控专题 §2.4(16 个 marker) | 引用 + 加 atomcode 25 marker 完整列表(sensitive_path.rs:1-180) |
| Bash blacklist | 沙箱专题 §2.1-2.5(rm/sudo 等) | 引用 + 加 atomcode 16 destructive 模式 + 5-tuple credential 模式 |
| bwrap/seccomp 总览 | 沙箱专题 §3-§5 | 本专题深入到 landlock-run 298 行 C 字节级 + 6 维度对比表 |
| 多 Agent 权限继承 | 多 Agent 协作与权限管控专题 + SubAgent 调度 | 仅引用 + 加 opencode deriveSubagentSessionPermission 30 行范本 |
| 权限 6 源 | 权限管控专题 §2.1(完整 1812 行) | 本专题聚焦**第一性原理**(为什么 6 源,policy 最高为何关键)+ cross-project 横向对比 |
| 沙箱探针/失败回退 | 沙箱专题 §4(probe 流程) | 本专题深入**landlock ABI 协商 + Windows SID 确定性哈希** 两个新维度 |

**核心本专题新增 8 维度**:
1. 三态状态机 × 6 工程 first-match 范式对比(§2)
2. 4 维规则匹配引擎 + shadowed rule(§3)
3. 决策审计二元组 vs 三元组 vs Effect 事件(§4)
4. 多层防御架构图(§5)
5. **landlock-run 298 行 C 字节级 + Windows SID 哈希**(§6)
6. 细粒度 6 维度(网络/文件/进程/GPU)(§7)
7. 27 种 PermissionRequest + 4 outcome 路由(§8)
8. laew P0/P1/P2 Rust crate 蓝图(§11)

---

## 14. TL;DR

- **claudecode** 是唯一三层全栈项目,27 PermissionRequest + bwrap+SBPL+seccomp+landlock(无) + 6 源规则;
- **atomcode** L1+L2 极强,6 gate + RiskLevel arg-aware,显式放弃 L3;
- **opencode** L1+L2 极简 + Effect 抽象让 L3 可插拔,`deriveSubagentSessionPermission` 是 SubAgent 权限继承范本;
- **deepseek-harness** L2+L3 最严谨,`writableRoots(policy)` 跨 runner 共享 + **298 行 landlock-run C11** + Windows WRITE_RESTRICTED + strict widening algebra;
- **pi** 委托给扩展 + bwrap,完全可插拔但需用户自建;
- **openclaw** SSRF/path 双层 + per-frame permissionProfile;
- **undici** 非 Agent,是 HTTP 客户端,仅有 CORS/TAO/bad-port 三件套 spec 钩子 + 89 端口 deny;
- **laew** 当前 L1 0 源 0 规则,L2 仅有 Write/Edit 路径白名单,L3 完全缺失;P0 建议采纳 atomcode `PermissionStore` + deepseek `effectiveApprovalPolicy` log-fold 模式,P1 集成 bwrap+landlock 双层(`landlock` 0.4 + `seccompiler` 0.4 crate),P2 加 TUI 弹窗 + SQLite `permission_log` + 子代理权限继承。
