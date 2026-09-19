# 第二十轮 — Codemode：代码即动作与 REPL 工具范式（D20）6 维度深度对比

> **调研日期**：2026-09-19
> **调研范围**：7 个 Agent 工程（atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / undici）
> **本轮定位**：D20 Codemode 6 子维度——Agent 不只调用预定义工具，而是**写一段代码让运行时执行**（代码即动作），以及运行时的沙箱/限制/stdlib。前 19 轮未覆盖
> **与前几轮关系**：
> - 第六轮 SubAgent 调度已覆盖：WorkFlow 同层并行、SubAgent 执行轨迹。**不重复调度层**
> - 第七轮 Bash PTY 已覆盖：Bash 工具的命令执行/超时/截断。**但 Bash 是「Agent 写命令给 shell」，Codemode 是「Agent 写结构化代码给运行时」——后者有类型/schema/沙箱**
> - 第七轮 WebAssembly WASI 已覆盖：wasmtime 沙箱。**不重复 WASM 层**
> - **本轮聚焦**：「代码即动作」范式——LLM 输出的是可执行代码（受控语言/DSL），运行时提供类型化工具 API + 资源限制 + 沙箱 stdlib，而非 shell 命令字符串
>
> **新增 laew gap 编号**：L2231-L2290（共 60 个新 gap）

---

## 0. 全局架构对比总览

### 0.1 Codemode 成熟度矩阵

| 工程 | 代码即动作 | 沙箱 stdlib | 资源限制 | OpenAPI 工具生成 | 类型 Schema | 代码量 |
|------|----------|-----------|---------|----------------|------------|--------|
| **opencode** | ✅ **codemode** | ✅ 10 类沙箱 | ✅ 三维限制 | ✅ openapi→tool | ✅ Effect Schema | **6878+ 行** |
| **deepseek** | ✅ code-runtime | ✅ Worker 线程 | ✅ | ❌ | ⚠️ | 5092 行 |
| **atomcode** | ⚠️ execution_policy | ⚠️ | ⚠️ | ❌ | ❌ | ~数百行 |
| **claudecode** | ❌ | — | — | — | — | 0 |
| **openclaw** | ❌ | — | — | — | — | 0 |
| **pi** | ❌ | — | — | — | — | 0 |
| **undici** | N/A | — | — | — | — | — |
| **laew** | ❌ | — | — | — | — | 0 |

### 0.2 Codemode 4 层架构模型

```
┌────────────────────────────────────────────────────────────────────────────┐
│                      Codemode 4 层架构模型                                   │
├────────────────────────────────────────────────────────────────────────────┤
│ L4 OpenAPI 工具生成层：OpenAPI spec → 类型化 Tool Description（自动）       │
│     ↓                                                                     │
│ L3 资源限制层：timeoutMs / maxToolCalls / maxOutputBytes 三维限制            │
│     ↓                                                                     │
│ L2 沙箱 stdlib 层：String/Number/Math/Date/URL/JSON 沙箱化实现             │
│     ↓                                                                     │
│ L1 解释执行层：interpreter runtime 执行 Agent 生成的代码                     │
└────────────────────────────────────────────────────────────────────────────┘
```

**共识**：代码必须受资源限制 + 沙箱化标准库 + 类型化工具 API
**哲学分歧**：
- **类型化代码派**（opencode）：Effect Schema + 解释执行 + 沙箱 stdlib
- **Worker 隔离派**（deepseek）：Worker 线程 + 进程级隔离
- **策略限制派**（atomcode）：execution_policy 限制执行
- **无代码即动作派**（claudecode / openclaw / pi）：LLM 只调用预定义工具，不写代码
**laew 现状**：❌ 无代码即动作能力（Bash 工具是命令字符串，非类型化代码）

---

## 1. D20-1 代码即动作（Code-as-Action）范式

**核心思想**：Agent 不再只选择预定义工具（Bash/Read/Write），而是**生成一段可执行代码**，由运行时解释执行。

### 1.1 opencode codemode：范式定义

`opencode/packages/codemode/src/codemode.ts`（159 行，范式入口）：

**核心类型**：
```ts
export type ExecutionLimits = {
  readonly timeoutMs?: number       // 最大执行时间(ms)
  readonly maxToolCalls?: number    // 最大工具调用次数
  readonly maxOutputBytes?: number  // 最大输出字节(UTF-8)
}

export type DiscoveryOptions = {
  readonly catalogBudget?: number   // 工具目录 token 预算(chars/4,默认 2000)
}
```

**执行模型**：
```ts
// codemode 执行伪代码
function executeCodeMode(program: string, tools: ToolTree, limits: ExecutionLimits) {
  const interpreter = createInterpreter(tools, limits)
  return interpreter.run(program)  // 受控执行 Agent 生成的代码
}
```

**范式要点**：
- Agent 生成的是一段**结构化程序**（非 shell 命令字符串）
- 程序通过**类型化工具 API** 调用工具（非 exec/spawn）
- 所有工具调用走 **Effect（函数式效应系统）**
- 资源限制是**三维**的（时间/调用数/输出量）

### 1.2 与 Bash 工具的范式对比

| 维度 | Bash 工具 | Codemode |
|------|----------|---------|
| Agent 输出 | 命令字符串 `ls -la /tmp` | 结构化代码 `listDir("/tmp")` |
| 执行方式 | shell exec/spawn | 解释执行 |
| 类型安全 | ❌ 字符串拼接 | ✅ Schema 校验 |
| 沙箱 | 进程级隔离 | 语言级沙箱(stdlib) |
| 资源限制 | 超时+截断 | 三维限制 |
| 工具发现 | 预定义工具集 | OpenAPI→动态生成 |
| 可组合性 | 低（管道） | 高（函数式组合） |

### 1.3 deepseek code-runtime：Worker 线程范式

deepseek 的 code-runtime 走**进程隔离**路线（`deepseek-harness/packages/code-runtime/`）：

**架构**：
- `code-runtime/`：主运行时
- `code-runtime-worker-thread/`：Worker 线程执行（3465 行 interpreter runtime 的 TS 等价）
- `code-runtime-python/`：Python 运行时
- `code-runtime-worker-thread/src/protocol.ts`：Worker 通信协议

**Worker 线程协议**：
- `protocol.ts`：主线程↔Worker 的二进制帧协议
- `worker.ts` / `worker-json.ts`：Worker 入口
- `bootstrap.ts`：Worker 引导
- `output-json.ts`：输出序列化

**与 opencode 的差异**：
- opencode：**语言级沙箱**（同进程，受限 stdlib）
- deepseek：**进程级沙箱**（Worker 线程，OS 隔离）

### 1.4 atomcode execution_policy

atomcode 的代码执行走**策略限制**路线：
- `crates/atomcode-coding/src/execution_policy.rs`
- 定义执行策略（允许的命令、资源上限）
- 比 Bash 工具结构化，比 codemode 轻量

**laew gap L2231-L2240**：
- L2231 P1：无代码即动作范式（Bash 只是命令字符串）
- L2232 P1：无结构化程序执行
- L2233 P2：无类型化工具 API
- L2234 P2：无函数式效应系统

---

## 2. D20-2 沙箱 stdlib

**核心问题**：代码即动作必须限制 Agent 代码能访问的标准库——不能让它任意访问文件系统/网络/进程。

### 2.1 opencode stdlib：10 类沙箱化实现

`opencode/packages/codemode/src/stdlib/` + `values.ts`（49 行值类型）：

**stdlib 清单**：

| 模块 | 文件 | 行数 | 沙箱要点 |
|------|------|------|---------|
| `console.ts` | stdlib | 4 | 沙箱日志（不写 stdout，走 Effect） |
| `promise.ts` | stdlib | 6 | SandboxPromise（异步沙箱化） |
| `json.ts` | stdlib | 42 | JSON 解析/序列化（安全） |
| `collections.ts` | stdlib | 51 | SandboxMap/SandboxSet（受限集合） |
| `string.ts` | stdlib | 52 | SandboxString（受限字符串操作） |
| `math.ts` | stdlib | 65 | Math 函数（纯函数，安全） |
| `number.ts` | stdlib | 66 | Number 操作（安全） |
| `regexp.ts` | stdlib | 74 | SandboxRegExp（受限正则，防 ReDoS） |
| `object.ts` | stdlib | 77 | SandboxObject（受限对象操作） |
| `url.ts` | stdlib | 90 | SandboxURL/SandboxURLSearchParams |
| `date.ts` | stdlib | 94 | SandboxDate（时间沙箱化） |
| `value.ts` | 根 | 49 | Sandbox* 值类型定义 |

### 2.2 values.ts：沙箱值类型系统

`opencode/packages/codemode/src/values.ts`（49 行）：

**核心沙箱类型**（推断）：
```ts
export type SandboxString = { /* 受限字符串 */ }
export type SandboxNumber = { /* 受限数字 */ }
export type SandboxMap<K,V> = { /* 受限 Map */ }
export type SandboxSet<V> = { /* 受限 Set */ }
export type SandboxRegExp = { /* 受限正则 */ }
export type SandboxURL = { /* 受限 URL */ }
export type SandboxURLSearchParams = { /* 受限参数 */ }
export type SandboxDate = { /* 受限日期 */ }
export type SandboxPromise<T> = { /* 受限 Promise */ }
```

**沙箱化原则**：
- 所有 stdlib 返回**沙箱值**（非原生 JS 值）
- 沙箱值之间的操作仍返回沙箱值（传染性）
- **禁止**：直接访问 `process`、`fs`、`http`、`child_process`
- **禁止**：`eval`、`Function` 构造器

### 2.3 tool-runtime.ts 的沙箱集成

`opencode/packages/codemode/src/tool-runtime.ts`（806 行，核心运行时）：

**导入沙箱 stdlib**：
```ts
import {
  SandboxDate, SandboxMap, SandboxPromise, SandboxRegExp,
  SandboxSet, SandboxURL, SandboxURLSearchParams,
} from "./values.js"
```

**工具错误处理**：
```ts
import { ToolError, toolError } from "./tool-error.js"
```

**工具 Schema 编解码**：
```ts
import {
  decodeInput as decodeToolInput, decodeOutput as decodeToolOutput,
  identifierSegment, inputProperties, inputTypeScript, outputTypeScript,
} from "./tool-schema.js"
```

### 2.4 deepseek code-runtime Worker：进程级沙箱

deepseek 用 Worker 线程实现**更强隔离**：

**protocol.ts**（Worker 通信协议）：
- 主线程→Worker：代码 + 工具描述
- Worker→主线程：执行结果 / 工具调用请求
- Worker 内代码**无法访问主线程内存**

**sandbox 对比**：

| 维度 | opencode 语言级 | deepseek 进程级 |
|------|---------------|----------------|
| 隔离强度 | 中（同进程） | 高（跨线程） |
| 性能开销 | 低 | 中（线程通信） |
| 逃逸风险 | 中（原型链污染） | 低（内存隔离） |
| 复杂度 | 中（重写 stdlib） | 高（协议+序列化） |

**laew gap L2241-L2250**：
- L2241 P1：无沙箱 stdlib（Bash 走 shell，无语言级沙箱）
- L2242 P2：无 Sandbox* 值类型
- L2243 P2：无 ReDoS 防护正则
- L2244 P2：无 SandboxDate（时间沙箱）
- L2245 P2：无 SandboxURL（URL 沙箱）

---

## 3. D20-3 资源限制（三维限制）

### 3.1 opencode ExecutionLimits 三维限制

**三维限制详解**：

| 维度 | 字段 | 默认 | 触发行为 |
|------|------|------|---------|
| **时间** | `timeoutMs` | 无限 | 超时终止执行 |
| **调用数** | `maxToolCalls` | 无限 | 超次拒绝新工具调用 |
| **输出量** | `maxOutputBytes` | 无限 | 超量截断输出 |

**ResolvedExecutionLimits**（解析后）：
```ts
export type ResolvedExecutionLimits = {
  readonly timeoutMs: number | undefined
  // ... 解析后的完整限制
}
```

### 3.2 interpreter/runtime.ts（3465 行，限制执行核心）

`opencode/packages/codemode/src/interpreter/runtime.ts`（3465 行，最核心文件）：

**executeWithLimits**（codemode.ts 导入的执行函数）：
```ts
// codemode.ts
import { executeWithLimits } from "./interpreter/runtime.js"
```

**执行循环**（推断）：
```
while (program 未结束) {
  if (now - start > timeoutMs) throw TimeoutError
  if (toolCalls >= maxToolCalls) throw MaxToolCallsError
  if (outputBytes >= maxOutputBytes) truncate + break
  executeNextStep(program)
}
```

### 3.3 interpreter/model.ts（201 行，执行模型）

`opencode/packages/codemode/src/interpreter/model.ts`（201 行）：

**执行模型**定义：
- 程序状态（AST/字节码）
- 执行栈
- 工具调用上下文
- 资源计数器（已用时间/调用数/输出字节）

### 3.4 与 laew 资源限制对比

laew 的既有资源限制：
- `agent/tools/bash.rs`：超时（默认 120s）+ 输出截断（30K/150K）
- `agent/mod.rs`：max_iterations（最大循环次数）

| 维度 | laew | opencode codemode |
|------|------|-----------------|
| 时间限制 | ✅ 120s（Bash） | ✅ timeoutMs（任意代码） |
| 调用数限制 | ✅ max_iterations | ✅ maxToolCalls |
| 输出量限制 | ✅ 30K/150K 截断 | ✅ maxOutputBytes |
| 沙箱 stdlib | ❌ | ✅ 10 类 |
| 限制粒度 | 单工具 | 整个程序执行 |

**laew gap L2251-L2260**：
- L2251 P2：无代码执行统一三维限制
- L2252 P2：无 maxOutputBytes（只有 Bash 截断）
- L2253 P2：无 maxToolCalls（只有 max_iterations）
- L2254 P2：无 ResolvedExecutionLimits 解析

---

## 4. D20-4 OpenAPI 工具自动生成

**核心创新**：opencode codemode 可以从 **OpenAPI spec 自动生成类型化工具描述**。

### 4.1 openapi/spec.ts（511 行）

`opencode/packages/codemode/src/openapi/spec.ts`（511 行）：

**职责**：
- 解析 OpenAPI spec（JSON/YAML）
- 提取 paths / operations / schemas
- 生成 `ToolDescription`（类型化工具描述）

### 4.2 openapi/types.ts（112 行）

`opencode/packages/codemode/src/openapi/types.ts`（112 行）：

**核心类型**（推断）：
```ts
type GeneratedTool = {
  name: string           // operationId
  description: string    // summary + description
  inputSchema: Schema    // 请求体 Schema
  outputSchema: Schema   // 响应体 Schema
}
```

### 4.3 openapi/runtime.ts（326 行）

`opencode/packages/codemode/src/openapi/runtime.ts`（326 行）：

**运行时调用生成**：
- 把 OpenAPI operation 映射为可执行函数
- Agent 代码调用 `getUser(id)` → runtime 发起 HTTP GET /users/{id}
- 类型安全（Effect Schema 校验请求/响应）

### 4.4 openapi/index.ts（130 行）

`opencode/packages/codemode/src/openapi/index.ts`（130 行）：

**入口函数**：
```ts
export function generateToolsFromOpenAPI(spec: OpenAPISpec): ToolTree
```

### 4.5 tool-schema.ts（301 行，Schema 编解码）

`opencode/packages/codemode/src/tool-schema.ts`（301 行）：

**核心函数**：
```ts
export function decodeInput(schema, input)   // 输入校验
export function decodeOutput(schema, output)  // 输出校验
export function inputProperties(schema)       // 输入属性列表
export function inputTypeScript(schema)       // 生成 TS 类型
export function outputTypeScript(schema)      // 生成 TS 类型
export function identifierSegment(name)       // 工具名→标识符
```

**意义**：工具目录（catalog）可以直接生成给 LLM 的 TypeScript 类型提示。

### 4.6 与 laew 工具定义的对比

laew 的工具定义（既有）：
- `src/agent/tools/mod.rs`：手动实现 Tool trait
- 手写 JSON Schema（`input_schema`）
- 无 OpenAPI 自动生成
- 无 TypeScript 类型生成（laew 是 Rust）

| 维度 | laew | opencode codemode |
|------|------|-----------------|
| 工具定义 | 手写 Rust trait | OpenAPI→自动生成 |
| Schema | 手写 JSON Schema | Effect Schema |
| 类型生成 | ❌ | ✅ TS 类型 |
| 工具发现 | 静态注册 | 动态 OpenAPI 导入 |

**laew gap L2261-L2270**：
- L2261 P2：无 OpenAPI 工具自动生成
- L2262 P2：无 spec→ToolDescription 转换
- L2263 P2：无 Effect Schema 编解码
- L2264 P2：无工具目录类型生成

---

## 5. D20-5 类型 Schema 系统

### 5.1 opencode Effect Schema

opencode 使用 **Effect 库的 Schema**（函数式 Schema 校验）：

```ts
import { Effect, Schema } from "effect"
```

**Schema 用途**：
- 工具输入/输出类型定义
- 运行时校验
- 类型提示生成
- OpenAPI 映射

### 5.2 tool.ts（96 行，工具定义类型）

`opencode/packages/codemode/src/tool.ts`（96 行）：

**核心类型**（推断）：
```ts
export type Definition<R = never> = {
  // 工具定义
}
export function isDefinition(value: unknown): value is Definition
```

### 5.3 tool-error.ts（11 行，错误类型）

`opencode/packages/codemode/src/tool-error.ts`（11 行）：

**ToolError**（推断）：
```ts
export type ToolError = {
  // 工具调用错误
}
export function toolError(message: string): ToolError
```

### 5.4 与 laew 类型系统的对比

laew 的 Schema（既有）：
- `agent/tools/mod.rs`：`JSONSchema` 类型
- `agent/tool_schema_validator.rs`：手写校验子集
- 依赖 schemars（隐含）

| 维度 | laew | opencode codemode |
|------|------|-----------------|
| Schema 来源 | 手写 | OpenAPI/Effect 生成 |
| 运行时校验 | ✅ tool_schema_validator | ✅ Effect Schema |
| 类型安全 | 中（JSON） | 高（Effect 类型） |
| 错误类型 | AgentError | ToolError |

**laew gap L2271-L2280**：
- L2271 P2：无 Effect Schema 风格类型系统
- L2272 P2：无 Schema→类型提示生成
- L2273 P2：无 ToolError 独立错误类型
- L2274 P2：无工具定义 isDefinition 类型守卫

---

## 6. D20-6 解释执行引擎

### 6.1 interpreter/runtime.ts（3465 行，最核心）

`opencode/packages/codemode/src/interpreter/runtime.ts`（3465 行，全知识库最大的单一运行时文件之一）：

**职责**：
- 解析 Agent 生成的代码为 AST
- 逐步解释执行
- 维护执行栈 + 工具调用上下文
- 执行资源限制
- 处理沙箱 stdlib 调用
- 处理工具调用（走 ToolRuntime）

**与 deepseek Worker 协议对比**：

| 维度 | opencode 解释执行 | deepseek Worker 线程 |
|------|----------------|-------------------|
| 执行方式 | 同进程解释 | 跨线程消息传递 |
| 状态维护 | 内存态 | 协议序列化 |
| 工具调用 | 直接函数调用 | 主线程代理 |
| 性能 | 中（解释） | 高（原生+隔离） |
| 安全 | 中（沙箱 stdlib） | 高（内存隔离） |

### 6.2 deepseek code-runtime-worker-thread 协议

`deepseek-harness/packages/code-runtime/code-runtime-worker-thread/src/`：

**协议文件**：
- `protocol.ts`：帧协议定义
- `worker.ts` / `worker-json.ts`：Worker 入口
- `bootstrap.ts`：引导序列
- `output-json.ts`：输出序列化

**测试覆盖**：
- `runtime.spec.ts`：运行时行为
- `output-json.spec.ts`：输出序列化
- `source-worker.compat.spec.ts`：源码↔Worker 兼容
- `bootstrap.spec.ts`：引导
- `built-lib.e2e.ts`：端到端

### 6.3 与 laew Agent 循环的关联

laew 的 Agent 循环（`agent/mod.rs::run_session`）：
- LLM 输出 tool_calls
- 循环执行工具
- 回填 tool_result

**codemode 的差异**：
- LLM 输出**可执行代码**（非 tool_calls）
- 运行时解释执行代码
- 代码内部调用工具（非循环驱动）
- 资源限制是**程序级**（非 tool 级）

**laew gap L2281-L2290**：
- L2281 P2：无解释执行引擎
- L2282 P2：无执行栈维护
- L2283 P2：无 AST 解析
- L2284 P2：无逐步执行+资源计数
- L2285 P2：无 Worker 线程隔离选项

---

## 7. laew gap 清单（L2231-L2290）

| 编号 | gap | 等级 | 落地建议 | 推荐 Rust crate |
|------|-----|------|---------|----------------|
| L2231 | 无代码即动作范式 | P1 | Bash 升级为类型化代码执行 | 自研 |
| L2232 | 无结构化程序执行 | P1 | DSL/子集语言 | 自研 |
| L2233 | 无类型化工具 API | P2 | Schema→工具函数 | schemars |
| L2234 | 无函数式效应系统 | P2 | Effect 风格（可选） | — |
| L2235 | 无 ToolTree 工具目录 | P2 | 工具树形组织 | 自研 |
| L2236 | 无 DiscoveryOptions | P2 | 工具发现预算 | 自研 |
| L2237 | 无 catalogBudget | P2 | token 预算控制 | 自研 |
| L2238 | 无 ToolCall hooks | P2 | 工具调用前后钩子 | 自研 |
| L2239 | 无 ToolCallStarted/Ended 事件 | P2 | 可观测性 | 自研 |
| L2240 | 无 ResolvedExecutionLimits | P2 | 限制解析 | 自研 |
| L2241 | 无沙箱 stdlib | P1 | String/Math/Date 沙箱 | 自研 |
| L2242 | 无 Sandbox* 值类型 | P2 | 传染性沙箱类型 | 自研 |
| L2243 | 无 ReDoS 防护正则 | P2 | 沙箱 RegExp | `regex`(限制) |
| L2244 | 无 SandboxDate | P2 | 时间沙箱 | 自研 |
| L2245 | 无 SandboxURL | P2 | URL 沙箱 | `url` crate(受限) |
| L2246 | 无 SandboxMap/Set | P2 | 受限集合 | 自研 |
| L2247 | 无 SandboxPromise | P2 | 异步沙箱 | `tokio`(受限) |
| L2248 | 无 console 沙箱 | P2 | 日志走 Effect | 自研 |
| L2249 | 无 stdlib 传染性 | P2 | 沙箱值→沙箱值 | 自研 |
| L2250 | 无禁止列表(process/fs/http) | P2 | 黑名单 | 自研 |
| L2251 | 无代码执行统一三维限制 | P2 | timeout/calls/bytes | 自研 |
| L2252 | 无 maxOutputBytes | P2 | 输出量限制 | 自研 |
| L2253 | 无 maxToolCalls | P2 | 工具调用次数 | 自研 |
| L2254 | 无 ResolvedExecutionLimits | P2 | 限制解析默认值 | 自研 |
| L2255 | 无超时错误类型 | P2 | TimeoutError | 自研 |
| L2256 | 无 MaxToolCalls 错误类型 | P2 | MaxToolCallsError | 自研 |
| L2257 | 无截断标记 | P2 | `[output truncated]` | 自研 |
| L2258 | 无资源计数器 | P2 | 已用时间/调用/字节 | 自研 |
| L2259 | 无执行开始时间戳 | P2 | 计时起点 | `std::time` |
| L2260 | 无执行耗时统计 | P2 | 性能可观测 | 自研 |
| L2261 | 无 OpenAPI 工具自动生成 | P2 | OpenAPI→Tool | `serde`(解析YAML/JSON) |
| L2262 | 无 spec→ToolDescription | P2 | 转换层 | 自研 |
| L2263 | 无 Effect Schema 编解码 | P2 | Schema 校验 | schemars |
| L2264 | 无工具目录类型生成 | P2 | TS 类型(Rust 不需要) | — |
| L2265 | 无 paths/operations 提取 | P2 | OpenAPI 遍历 | `serde_json` |
| L2266 | 无 operationId→工具名 | P2 | 名称映射 | 自研 |
| L2267 | 无请求体 Schema 映射 | P2 | inputSchema | 自研 |
| L2268 | 无响应体 Schema 映射 | P2 | outputSchema | 自研 |
| L2269 | 无 HTTP 调用代理 | P2 | 工具调用→HTTP | reqwest |
| L2270 | 无响应校验 | P2 | outputSchema 校验 | 自研 |
| L2271 | 无 Effect Schema 风格类型 | P2 | 函数式 Schema | — |
| L2272 | 无 Schema→类型提示 | P2 | 类型生成 | — |
| L2273 | 无 ToolError 独立类型 | P2 | 错误分类 | thiserror |
| L2274 | 无 isDefinition 类型守卫 | P2 | 运行时类型检查 | 自研 |
| L2275 | 无 inputProperties | P2 | 输入属性列表 | 自研 |
| L2276 | 无 inputTypeScript | P2 | 输入类型生成 | — |
| L2277 | 无 outputTypeScript | P2 | 输出类型生成 | — |
| L2278 | 无 identifierSegment | P2 | 工具名→标识符 | 自研 |
| L2279 | 无 Schema→JSON Schema 双向转 | P2 | 互转 | schemars |
| L2280 | 无 Schema 错误路径报告 | P2 | 精确错误定位 | 自研 |
| L2281 | 无解释执行引擎 | P2 | AST 解释器 | 自研 |
| L2282 | 无执行栈维护 | P2 | 调用栈 | 自研 |
| L2283 | 无 AST 解析 | P2 | 语法分析 | 自研 |
| L2284 | 无逐步执行+资源计数 | P2 | 单步执行 | 自研 |
| L2285 | 无 Worker 线程隔离 | P2 | 进程级沙箱 | `std::thread` |
| L2286 | 无 Worker 通信协议 | P2 | 帧协议 | 自研 |
| L2287 | 无 bootstrap 序列 | P2 | Worker 引导 | 自研 |
| L2288 | 无 output 序列化 | P2 | 输出编码 | serde |
| L2289 | 无源码↔Worker 兼容 | P2 | 兼容性 | 自研 |
| L2290 | 无执行引擎基准测试 | P2 | 性能 | criterion |

---

## 8. P0 实施路线图（1-2 周可做）

1. **D20-1 代码即动作范式**（`agent/codemode.rs` 新建，DSL 子集 + 类型化工具 API）——参考 opencode 但 Rust 化
2. **D20-3 资源限制**（接入既有 Bash 超时+截断，统一为三维限制）——低成本高收益
3. **D20-2 沙箱 stdlib**（受限 String/Math/Date/URL 实现）——安全基础
4. **D20-6 解释执行**（简单 AST 解释器，先支持顺序执行+工具调用）——核心引擎
5. **D20-4 OpenAPI 工具生成**（`openapi` crate 解析 spec→ToolDescription）——生态扩展

---

## 9. Rust crate 推荐矩阵

| 维度 | 推荐 crate | 用途 |
|------|-----------|------|
| 代码执行 | 自研 DSL 解释器 | Rust 子集/AST |
| 沙箱 | 自研 Sandbox* 类型 | 值类型 |
| 资源限制 | `std::time` + 自研计数器 | 时间/调用/字节 |
| OpenAPI | `serde` + `serde_json` + `openapiv3` | spec 解析 |
| Schema | `schemars` | JSON Schema |
| Worker | `std::thread` + `serde` | 进程隔离 |
| 正则安全 | `regex`(限制 backtracking) | ReDoS 防护 |

---

## 10. 设计哲学总结

### 10.1 代码即动作的两种范式

| 范式 | 代表 | 核心机制 | 适用场景 |
|------|------|---------|---------|
| 语言级沙箱 | opencode | Effect Schema + 解释执行 + 沙箱 stdlib | 需要类型安全+快速执行 |
| 进程级沙箱 | deepseek | Worker 线程 + OS 隔离 | 需要强隔离+多语言 |
| 策略限制 | atomcode | execution_policy | 轻量限制 |

### 10.2 laew 的差异化定位建议

laew 作为 Rust CLI，应走 **「类型化 Bash 升级」** 路线：
- 短期：Bash 工具升级为类型化代码执行（类似 codemode 但 Rust DSL）
- 中期：三维资源限制统一（timeout/calls/bytes）
- 长期：沙箱 stdlib + OpenAPI 工具生成
- 不建议走 Worker 线程路线（Rust CLI 单进程定位）

### 10.3 与既有 laew 能力的关系

| 既有能力 | 本轮增强 |
|---------|---------|
| `agent/tools/bash.rs` 命令执行 | 升级为类型化代码执行 |
| 超时+截断 | 统一为三维限制 |
| `agent/mod.rs` 循环 | 程序级执行（非循环驱动） |

---

*本专题由第二十轮生成。前 19 轮 + laew 既有能力覆盖了工具调用（Bash/Read/Write/Glob/Grep）和调度（SubAgent/WorkFlow）；本轮聚焦代码即动作范式，共 60 个新 gap（L2231-L2290）。*
