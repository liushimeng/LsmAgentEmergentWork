# 专题-第十六轮-pi-深度分析

> **调研日期**：2026-09-09
> **调研范围**：pi 缓存分析、扩展加载、AI 适配器、溢出检测、重试策略
> **本轮新增 gap**：L1044-L1045 / L1083-L1094（36 个）

---

## 一、cache-stats 浪费检测（core/cache-stats.ts 164 行）

```typescript
const NOISE_FLOOR_TOKENS = 1024;  // 低于此值的 cache-read 视为噪声不计
// sticky reportedCache：对"仅 cache-read 不 cache-write"的 provider，一旦 reportedCache > 0 就置 sticky
// compaction/branch_summary 仅重置 prev，不触发 modelChanged
// missedCost = missedTokens * (inputRate - cacheReadRate) / 1_000_000
```

**laew gap L1083**：无 prompt-cache miss 检测，无法量化 compaction/模型切换导致的缓存浪费。

---

## 二、自定义 undici 调度器（core/http-dispatcher.ts 113 行）

```typescript
const DEFAULT_HTTP_IDLE_TIMEOUT_MS = 300_000;  // 5 分钟
// proxyTunnel: true + autoSelectFamilyAttemptTimeout: 2_000（Happy Eyeballs IPv6 回退 2s 限制）
// Node 26 fetch/undici 压缩 workaround：显式 patch undici.setGlobalDispatcher 绕过
```

**laew gap L1084**：使用 reqwest 默认 dispatcher，无空闲超时、无 Happy Eyeballs 控制。

---

## 三、三层 Provider 组合（core/provider-composer.ts 581 行）

```typescript
applyModelsJson()        // 第 1 层：用户 models.json
  → applyExtension()     // 第 2 层：扩展注册
  → extensionOAuth.modifyModels()  // 第 3 层：OAuth 扩展修改
  → modelOverrides()     // 第 4 层：最终覆盖
```

- mergeCompat 深度合并（递归合并嵌套 routing 对象）
- OAuth-only provider 不伪造 API-key 登录
- Eager validation：getModels() 在注册时立即调用一次

**laew gap L1085**：无 provider 组合层。

---

## 四、项目信任模型（core/trust-manager.ts + project-trust.ts）

```typescript
type ProjectTrustDecision = boolean | null;  // true=信任, false=不信任, null=未决定
// 目录向上查找：从 cwd 向上遍历，找到最近的信任决策
// 需要信任的资源：settings.json / extensions / skills / prompts / themes / SYSTEM.md
```

**laew gap L1086**：无项目信任模型，无法区分用户全局扩展 vs 项目本地扩展的安全边界。

---

## 五、jiti 扩展加载（core/extensions/loader.ts 805 行）

```typescript
// 三种运行时模式
isBunBinary / isNodeSeaBinary / isBundledNode / isTypeScriptSourceRuntime

// 虚拟模块映射
const VIRTUAL_MODULES = {
  "typebox", "@earendil-works/pi-agent-core", "pi-tui", "pi-ai", ...
};

// InlineExtension：支持 {name, factory, hidden} 内联扩展
```

**laew gap L1087**：无 jiti 加载、无虚拟模块、无内联扩展、无工厂缓存。

---

## 六、扩展事件系统（core/extensions/types.ts 1797 行）

```typescript
type ExtensionEvent =
  | ProjectTrustEvent | ResourcesDiscoverEvent | SessionEvent
  | ContextEvent | BeforeProviderRequestEvent | BeforeProviderHeadersEvent
  | AfterProviderResponseEvent | BeforeAgentStartEvent | AgentStartEvent
  | AgentEndEvent | AgentSettledEvent | UIPromptStartEvent | UIPromptEndEvent
  | TurnStartEvent | TurnEndEvent | MessageStartEvent | MessageUpdateEvent
  | MessageEndEvent | ToolExecutionStartEvent | ToolExecutionUpdateEvent
  | ToolExecutionEndEvent | ModelSelectEvent | ThinkingLevelSelectEvent
  | UserBashEvent | InputEvent | ToolCallEvent | ToolResultEvent;
```

**laew gap L1088**：23 种事件类型全部缺失。

---

## 七、OAuth 凭证存储（ai/auth/resolve.ts）

```typescript
// 双检锁 + credentials.modify
const DEFAULT_OAUTH_MINIMUM_VALIDITY_MS = 5 * 60 * 1_000;  // 5 分钟
const DEFAULT_OAUTH_REFRESH_TIMEOUT_MS = 15_000;            // 15 秒
```

**laew gap L1091**：无 OAuth 凭证存储，无双检锁刷新、无跨进程安全 modify。

---

## 八、上下文溢出检测（ai/utils/overflow.ts）

```typescript
// 匹配 20+ provider 溢出正则
const patterns = [
  /prompt is too long/i, /request_too_large/i, /input is too long/i,
  /exceeds the context window/i, /maximum context length is \d+/i, ...
];
// NON_OVERFLOW_PATTERNS 排除节流误报
// 静默溢出检测：usage.input > contextWindow
```

**laew gap L1044 [P0]**：无上下文溢出检测。

---

## 九、provider 重试策略（ai/utils/retry.ts）

```typescript
retryAssistantCall(policy: {
  baseDelayMs * 2^(attempt-1);  // 指数退避
  onRetryScheduled / onRetryAttemptStart / onRetryFinished;
  isRetryableAssistantError(error);  // quota/billing 不可重试；overloaded/network 可重试
})
```

**laew gap L1045 [P0]**：无 provider 重试策略。

---

## 十、新增 gap 清单

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1044 | 溢出检测 | 无上下文溢出检测（20+ provider 溢出正则） | P0 |
| L1045 | 重试策略 | 无 provider 重试策略（指数退避 + 可重试错误分类） | P0 |
| L1083 | 缓存分析 | 无 prompt-cache miss 检测 | P1 |
| L1084 | HTTP | 无自定义 undici 调度器 | P1 |
| L1085 | Provider | 无三层 provider 组合 | P1 |
| L1086 | 信任 | 无项目信任模型 | P1 |
| L1087 | 扩展 | 无 jiti 扩展加载 | P1 |
| L1088 | 扩展 | 无扩展事件系统（23 种事件） | P1 |
| L1089 | 扩展 | 无扩展运行器 | P1 |
| L1090 | 扩展 | 无插件生命周期状态机 | P1 |
| L1091 | OAuth | 无 OAuth 凭证存储 | P1 |
| L1092 | 错误 | 无 provider 错误归一化 | P1 |
| L1093 | 工具 | 无工具调用参数验证 | P1 |
| L1094 | 测试 | 无录制回放系统 | P1 |

---

**报告完成日期**：2026-09-09
**分析基础**：pi core/cache-stats.ts + http-dispatcher.ts + provider-composer.ts + extensions/ + ai/ 逐行分析
