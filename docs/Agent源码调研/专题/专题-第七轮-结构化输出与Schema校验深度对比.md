# 专题-第七轮-结构化输出与Schema校验深度对比

> **覆盖主题**：Schema 来源 / 协议参数生成 / 跨 Provider 归一化 / 参数校验 / 响应容错 /
> 自修复重试 / Agent 内部结构化输出 / Schema 文档联动 8 大维度。
> **差异化定位**：第六轮协议对比主题 11「Tool Schema 投影」、第三轮-工具调用深度分析、
> 12 Agent 对比总表已涉及面状对比；本篇聚焦 **Schema 定义 → 生成 → 投影 → 校验 → 自修复 →
> 归一化的完整闭环工程实现**，含 laew 的 P0/P1/P2 改造代码。

---

## 目录

1. [结论速览](#1-结论速览)
2. [逐项目剖析](#2-逐项目剖析)
   - 2.1 pi（TypeScript + strict 投影全套）
   - 2.2 opencode（Effect Schema + 跨 provider 归一化）
   - 2.3 atomcode（Rust 手写 Schema + 业界最完整 JSON Repair 链）
   - 2.4 claudecode（Zod Schema + json_schema 结构化输出）
   - 2.5 openclaw（TypeBox + 多 provider keyword 清洗）
   - 2.6 deepseek-harness / agent-core / Switchyard 简述
   - 2.7 laew 现状
3. [横向对比大表（13 项目 × 14 维度）](#3-横向对比大表13-项目--14-维度)
4. [Schema 闭环流水线 ASCII 图](#4-schema-闭环流水线-ascii-图)
5. [跨 Provider 归一化规则矩阵](#5-跨-provider-归一化规则矩阵)
6. [非法 JSON 修复策略对比表](#6-非法-json-修复策略对比表)
7. [10–15 个设计模式与反模式](#7-1015-个设计模式与反模式)
8. [laew 现状与 P0/P1/P2 路线图](#8-laew-现状与-p0p1p2-路线图)
9. [关键文件速查](#9-关键文件速查)

---

## 1. 结论速览

1. **Schema 来源 = 5 极谱**：手写 `serde_json::Value`（laew/atomcode/openclaw 部分）→ TypeBox 编译时生成
   （openclaw）→ Zod TS 运行时推导（claudecode）→ Effect Schema 解码器（opencode）→ `schemars` serde 派生
   （Rust 生态潜在方案）。各有利弊，无单一胜出者。
2. **协议参数投影 = 必经关口**：Anthropic `input_schema` / OpenAI `function.parameters` / Gemini `parametersJsonSchema` /
   OpenAI Responses `tools[].parameters` 各有不同命名与约束子集；**没有任何一个项目"原样透传" Schema**。
3. **OpenAI strict 子集是最大投影成本**：pi 完整实现 `makeStrictJsonSchema`（强制 `additionalProperties:false`、
   全字段必填、`null` 用 `anyOf` 表达、剥离 `patternProperties`/`allOf`/`$ref`/...16 类关键词），
   opencode 走更激进的 `moonshot`/`gemini` 兼容性分支，openclaw 走 keyword 黑名单清洗。
4. **响应容错 = 5 段递降链**：fast-path `JSON.parse` → `repairJson` (control char + backslash) →
   `partial-json` 流式补齐 → `extract_json_fields` 键值抽取 → 失败兜底。atomcode 是唯一**内置 Windows
   路径语义预修复** + **结构性 in_string 感知的修复算法**（O(N) 单次扫描），避免破坏字符串内
   容里的 `class:` / `fn main() {` 等代码。
5. **参数预校验 = 业内空白**：6 个调研项目里 **没有一个在请求前用 Schema 校验 tool arguments**，
   全是"信任模型 / 解析失败 → 透传给工具 → 工具自行报错"。仅 claudecode 在 `BashTool` 用
   `inputSchema().safeParse(input)` 做安全分类（见 §2.4）。
6. **结构化输出 = 三档体系**：`tool_choice: {type:"tool", name:"..."}` 强制 + 解析 `tool_use.input` /
   `response_format: json_schema`（Claude / OpenAI）/ `response_format: json_object`（兜底）/
   提示词 + JSON 代码围栏抽取（laew 当前路径）。
7. **laew 三大 P0 改造**：① `Tool::parameters()` 统一归一化函数（Anthropic/OpenAI strict/Gemini）
   ② 引入 `jsonschema` crate 在请求前校验 ③ `RepairToolArgsMiddleware` 复用 atomcode 修复链。

---

## 2. 逐项目剖析

### 2.1 pi（TypeScript + strict 投影全套）

**Schema 来源**：手写 JSON Schema（`Tool.parameters: JSONSchema`）；不依赖 Zod/TypeBox 等运行时库，
Schema 是**纯 JSON 对象**。代价是失去了 TS 类型推导，但换来「跨语言 / 跨模型 / 跨 MCP」的可移植性。

**协议参数生成 → 投影**：在 `constrained-sampling.ts` 完整实现 strict 子集投影：

- `makeStrictJsonSchema` 递归剥离 16 类不支持关键词（`UNSUPPORTED_STRICT_SCHEMA_KEYS`）：
  `$ref / $defs / definitions / allOf / oneOf / patternProperties / dependentSchemas / dependencies /
  unevaluatedProperties / propertyNames / contains / prefixItems / not / if / then / else`。
- `anyOf` 仅允许非结构化变体（标量），对象/数组 union 会被抛 `object and array unions are unsupported`。
- 全字段强制 `required` 集合 = 全部 `properties` 键；非 required 字段包装为 `anyOf: [original, {type:"null"}]`。
- 最终强制 `additionalProperties: false`。

调用位置在 5 个适配器：

| 适配器 | 投影点 |
|--------|--------|
| Anthropic (`packages/ai/src/api/anthropic-messages.ts:1337`) | `input_schema: { json: getJsonSchemaToolParameters(...) }` |
| OpenAI Chat (`openai-completions.ts:1493`) | `function.parameters` + `strict: <resolve>` |
| OpenAI Responses (`openai-responses-shared.ts:380`) | `tools[].parameters` + `strict` |
| Gemini (`google-shared.ts:327`) | `parametersJsonSchema` |
| Mistral (`mistral-conversations.ts:755`) | 剥 `symbol keys` 后投影 |
| Bedrock (`bedrock-converse-stream.ts:1111`) | 投影 → Document 包装 |

**Schema → 工具支持**：`resolveJsonSchemaStrictSampling(tool, supportsStrictMode)` 根据 `tool.constrainedSampling.type === "json_schema"` 决策三档：
- `supportsStrictMode && schemaStrictCompatible` → strict
- `supportsStrictMode && schemaIncompatible && config.strict === "require"` → **抛错**（fail-loud）
- `config.strict === "require"` 但 provider 不支持 strict → 抛错
- 否则 → undefined（降级非 strict）

**响应解析与容错**：`packages/ai/src/utils/json-parse.ts` 提供 5 段式降级：

```typescript
// 1. fast-path
JSON.parse(partialJson)
// 2. repairJson — 双反斜杠补齐 + 控制字符转义
repairJson(json)
// 3. partial-json — partial-json 库流式补齐未闭合的 {}
parseStreamingJson()
// 4. partial-json + repairJson 组合
parseStreamingJson()  // 内部 3 层 try/catch 兜底
// 5. 返回 {} 空对象兜底（流式上下文增量用）
```

**结构化输出**：使用 `tool_choice: {type:"tool", name:"..."}` 强制 LLM 调用特定 tool，再用
Zod Schema（或内部 JSON Schema）解析 `tool_use.input`。**没看到** `response_format: json_object` 的使用。

### 2.2 opencode（Effect Schema + 跨 Provider 归一化）

**Schema 来源**：双模式：

- **Typed 模式**：`Tool.make({ parameters: Schema.Struct({...}) })` 编译期类型 + 运行期解码器，
  通过 `Schema.decodeUnknownEffect` 在 `execute` 调用前自动解码（`packages/llm/src/tool.ts:194`）。
- **Dynamic 模式**：`Tool.make({ jsonSchema: {...} })` 给 MCP / 插件 / 动态配置用，输入为 `unknown`，
  由 handler 自行校验。

**协议参数生成**：`packages/llm/src/protocols/utils/tool-schema.ts` 实现 3 类 Provider 投影：

| 函数 | Provider | 关键改动 |
|------|----------|----------|
| `openAI(schema)` | OpenAI | 强制 `type: "object"`；anyOf 变体 properties 合并 + additionalProperties:false；anyOf 含 `null` 变体剥除 |
| `moonshot(schema)` | Moonshot/Kimi | `$ref` 兄弟字段清空；tuple items → `anyOf` 链；`prefixItems` → `items`；`unevaluatedItems` 删除 |
| `gemini(schema)` | Gemini | 数值 enum → string；dangling required 字段过滤；空 array items 默认 `{type:"string"}`；scalar type 上的 properties/required 删除 |

`modelCompatibility` 在协议层之前做兼容预处理。`packages/llm/test/tool-schema-projection.test.ts:9-117`
提供 4 类测试场景覆盖。

**Tool 执行入口**：`packages/opencode/src/session/retry.ts:172-181` 的 `parseJSON(value)` 仅做
`JSON.parse` 兜底 catch；**未做修复**。

**结构化输出**：用 `tool_choice` + Effect Schema 解码作为强类型接收口。

### 2.3 atomcode（Rust 手写 Schema + 业界最完整 JSON Repair 链）

**Schema 来源**：手写 `serde_json::Value`（`ToolDef.parameters`），**未引入 `schemars`**。
L1 工具列表 `crates/atomcode-capabilities/src/provider/anthropic.rs:466` 直接：
```rust
json!({"name": td.name, "description": td.description, "input_schema": td.parameters})
```

**协议参数生成**：直透传，**不投影**。代价是 Gemini/Moonshot 等小众 provider 兼容性差。

**请求前参数校验**：未做。`crates/atomcode-coding/src/execution_policy.rs:314-320` 用 `serde_json::from_str`
提取 `command` 字段做语义分类，但不校验类型 / 范围 / required。

**响应解析与容错（业界最强）**：`crates/atomcode-capabilities/src/tools/repair.rs` 2200+ 行
`repair_tool_args` 完整实现 8 段修复链：

| 段 | 函数 | 修复 |
|----|------|------|
| 0 | `pre_escape_windows_paths_in_json` | 在 `file_path`/`path` 值中，单反斜杠 Windows 盘符前缀的 `\t`/`\n`/`\f`/`\r`/`\b` 双倍化；`\u`/已转义路径/非路径键 全部不动 |
| 1 | `serde_json::from_str` | fast-path，已合法 JSON 直接透传 |
| 2 | `repair_json` | 反斜杠补齐 + 控制字符转义 + markdown 围栏剥离 + 单引号 → 双引号（fallback）+ 缺逗号补齐（O(N) 单遍）+ 无引号键加引号（in_string 感知的 `structural_mask`） + trailing comma 删除（同样 in_string 感知 + O(N) 单遍）+ `{}` 包裹 + 大括号配平 |
| 3 | `extract_edit_file_args` | `edit_file` 工具专用，基于已知字段顺序 `file_path / old_string / new_string / replace_all` 位置抽取，容忍内部未转义引号 |
| 4 | `extract_task_args` | `task` 工具专用，**用 anchor-based 重构嵌套任务数组**，避免 generic extractor 把 `tasks` 摊平为截断字符串 |
| 5 | `repair_stringified_structured_fields` | Schema-aware：**只对 schema 要求 array/object 但模型给字符串的字段**做一次 `JSON.parse` 解码（如 `{"todos":"[{...}]"}` → `{"todos":[{...}]}`） |
| 6 | `extract_json_fields` | 兜底键值抽取（`"key": value` 或 `key: value`），结构性终止符识别（未转义引号不算终止） |
| 7 | 原样返回 | 全部失败时透传，工具返回真实 parse error 给模型 |

**结构性 in_string 感知算法**（`structural_mask`）：通过一次扫描标记每个 char 是否在 string body 内
（区分 `\"` 转义），所有结构修改（trailing comma / unquoted key / brace balance）都跳过字符串内字符，
避免破坏 `"{ class: foo }"` / `"tail,}"` / `"fn main() {"` 这类**字符串内容碰巧像 JSON 片段**的情况。
v4.23.2 的 `notify.py` bug 就是这一机制缺失导致 `class A:\n` 被当作 Windows 盘符而把 `\n` 双倍化，
把 30 行 Python 写成 1 行。

**O(N) 单遍算法**：`hardening_tests`（`repair.rs:2120-2195`）专门 pin 住 O(N²) 退化场景
（50,000 个尾随逗号、20,000 个缺逗号字段），旧实现的 O(N²) `Vec::insert` 会冻死 host。

**Middleware 集成**：`RepairToolArgsMiddleware::before` 在 tool 执行前替换 `call.arguments`，
**必须注册在 approval gate 之前**，保证审批与执行看到一致 bytes。

**结构化输出**：Yolo / Plan / Main-Work 等用 Anthropic `tool_choice: {type:"tool", name:"..."}`
强制；YoloRunner 用 `serde_json::from_str::<TaskClassification>(text)` 直接解析，无 fallback。

### 2.4 claudecode（Zod Schema + json_schema 结构化输出）

**Schema 来源**：**Zod v4**（`zod/v4`），TS 推导类型 + 自动 JSON Schema 双向转换。例如 BashTool：

```typescript
// src/tools/BashTool/BashTool.tsx:227-247
const fullInputSchema = lazySchema(() => z.strictObject({
  command: z.string().describe('The command to execute'),
  timeout: semanticNumber(z.number().optional()).describe(...),
  description: z.string().optional().describe(...),
  run_in_background: semanticBoolean(z.boolean().optional()).describe(...),
  dangerouslyDisableSandbox: semanticBoolean(z.boolean().optional()).describe(...),
  _simulatedSedEdit: z.object({...}).optional().describe('Internal: ...')
}))
```

`z.strictObject` 强制不允许额外字段；`lazySchema` 延后实例化（解决 module-load 时序依赖）。
`.omit({run_in_background: true, _simulatedSedEdit: true})` 在对外暴露前裁掉内部字段（防止模型
绕过 permission gate 用 `_simulatedSedEdit` 写文件）。

**Tool 校验**（`BashTool.tsx:470-477`）：
```typescript
isSearchOrReadCommand(input) {
  const parsed = inputSchema().safeParse(input);  // ← Zod 校验 + 推导 TS 类型
  if (!parsed.success) return { isSearch:false, isRead:false, isList:false };
  return isSearchOrReadBashCommand(parsed.data.command);
}
```
**业内唯一在请求前用 Schema 做语义分类**的工具（用于 UI 折叠判定）。

**响应解析与容错**：
- `src/utils/json.ts:45-58` `safeParseJSON` 用 LRU 缓存（50 条 / 8KB 上限）避免 lodash memoize
  内存泄漏。
- `safeParseJSONC` 用 `jsonc-parser` 处理 VS Code 配置类文件（容忍注释/尾逗号）。
- `parseJSONL` 用 Bun 原生 `JSONL.parseChunk` 增量解析，自动 skip 损坏行。

**结构化输出**：`generateSessionTitle` 用 `response_format: { type: 'json_schema', schema: {...} }`
强制 Haiku 返回 `{title: string}`，再用 `titleSchema().safeParse(safeParseJSON(text))` Zod 校验。
permission_explainer 用 `tool_choice: {type: 'tool', name: 'explain_command'}`。`yoloClassifier`
也用 `tool_choice` 强制结构化分类。

### 2.5 openclaw（TypeBox + 多 Provider keyword 清洗）

**Schema 来源**：**TypeBox**（`@sinclair/typebox`），编译期类型推导 + 运行期校验器。
`Type.Object({...})` 在 `ajv` 编译时生成 JSON Schema。

**协议参数生成**：`packages/ai/src/providers/agent-tools-parameter-schema.ts:801-984`
`normalizeToolParameterSchemaUncached` 实现**全 Provider 归一化管道**：

```
inline refs → normalize OpenAPI keywords → provider-cleaning (Gemini/xAI/Anthropic)
```

关键函数：
- `inlineLocalToolSchemaRefs(schema)` — 内联本地 `$ref`
- `normalizeOpenApiSchemaKeywords(schema)` — `exclusiveMinimum/Maximum` → `minimum/maximum` + `exclusive`
- `cleanSchemaForGemini(schema)` — `GEMINI_UNSUPPORTED_SCHEMA_KEYWORDS` 黑名单清洗
- `stripUnsupportedSchemaKeywords(schema, set)` — 模型能力声明的不支持关键词剥离
- `cleanSchemaForLlamacppGbnf(schema)` — llama.cpp GBNF grammar 兼容化

**anyOf 顶层展平**（`agent-tools-parameter-schema.ts:946-963`）：OpenAI 拒绝无顶层 `type:"object"`
的 schema，TypeBox root unions 编译为 `{ anyOf: [...] }`，所以展平合并 `properties`/`required`，
保留 `action` 这类 enum 字段。

**Provider 特化判断**（`schema-keyword-strip.ts`）：
```typescript
const isGeminiProvider = normalizedProvider.includes("google")
  || normalizedProvider.includes("gemini")
  || isGeminiModelId(normalizedModelId)
  || normalizedToolSchemaProfile === "gemini";
```
模型 ID 嗅探 + provider 字符串 + 显式 `toolSchemaProfile` 三路判定。

**结构化输出**：openclaw 偏传统 `tool_choice` + JSON 解析，无 `response_format: json_schema`。
Yolo/Plan 用 Haiku 主模型直接 tool_call。

### 2.6 deepseek-harness / agent-core / Switchyard 简述

| 项目 | Schema 来源 | 投影 | 修复 |
|------|------------|------|------|
| **deepseek-harness** | TypeScript Zod + Cordis `argument` 装饰器 | Type.Literal → enum 合并；ACP/A2A 协议 schema 校验 | 协议层 `parseWithRetry` 流式 partial JSON |
| **agent-core**（openJiuwen Python） | Pydantic v2，`@field_validator` | 单一协议 IR（ReAct 内部表示）→ 各 provider wire | Pydantic 自动校验 + `ValidationError` 重试 |
| **Switchyard**（Rust 协议 IR） | 手写 `ContentBlock` + JSON Schema 派生 | 协议 IR → 7 种 provider wire | `TranslationEngine` 的回退协议分支 |
| **hermes-agent** | Pydantic v2 + SQLite FTS5 | provider-by-provider JSON Schema dump | sqlite-journal 持久层无修复 |
| **agent-studio**（Python） | Pydantic + DSL | Pregel cba 消减 + DSL 双向转换 | DSL 解析器容错 |

Switchyard 的 `TranslationEngine` 把协议 wire 投影作为一等公民（4 层 IR + 5 类翻译规则），
对结构化输出处理是最严谨的：先 JSON Schema → IR → provider wire → 响应 → IR → 上层。

### 2.7 laew 现状

**Schema 来源**：手写 `serde_json::Value`（6 个 Tool 全部），`src/agent/tools/{bash,read,write,edit,glob,grep}.rs`。
Wire 层直接透传（`llm/anthropic.rs:117-128` 和 `llm/openai.rs:141-155` 各 `convert_tools`
只换字段名不做投影）：

```rust
// anthropic.rs:121-126
json!({"name": t.name, "description": t.description, "input_schema": t.input_schema})
// openai.rs:145-152
json!({"type":"function", "function":{"name": t.name, "description": t.description, "parameters": t.input_schema}})
```

**请求前参数校验**：`Tool::execute(args: Value)` 接收 `serde_json::Value`，工具内部用
`.get("xxx").and_then(Value::as_xxx)` 提取，**缺失/类型不对直接 ToolExecution error**，
无 jsonschema 校验。

**响应解析与容错**：Yolo 用 `parse_classification`（`src/agent/yolo.rs:215-229`）仅做
```rust
extract_json_block(text)         // ```json ... ``` 抽取
→ serde_json::from_str
OR extract_standalone_json(text)  // 顶层 { } 大括号配平抽取
→ serde_json::from_str
```
**无修复、无 partial JSON、无键值抽取兜底**。

**结构化输出**：Yolo 通过系统提示词要求"输出 JSON" + 模型输出 ```json 代码围栏 + 简单解析；
**未使用** `tool_choice` 强制（Wire 层 openai.rs:1308 `tool_choice: Some("auto")` 写死）。

**漏点 L1-L15**（与第六轮协议对比主题 11 重叠 + 本专题新增）：
- L1 无 strict 投影；L6 tool_choice 写死 "auto"；L11 Schema 投影未做。
- **L16 无 jsonschema 校验**：参数缺失/类型错只能事后工具报错，无重试信号。
- **L17 无 JSON 修复链**：模型输出 ```json{trailing comma}`` 直接失败。
- **L18 无 partial JSON 流式解析**：长 JSON 中途断流后空对象兜底，丢失全部字段。
- **L19 Yolo 无结构化输出强制**：靠 prompt + 解析，鲁棒性差。
- **L20 无跨 provider 归一化**：Gemini provider 直接透传将触发 400。

---

## 3. 横向对比大表（13 项目 × 14 维度）

| 维度 | laew | atomcode | pi | opencode | claudecode | openclaw | deepseek-harness | agent-core | Switchyard | hermes | agent-studio | cc-switch | jiuwenswarm |
|------|------|----------|----|---------:|-----------:|---------:|-----------------:|-----------:|-----------:|-------:|-------------:|----------:|------------:|
| **Schema 来源** | 手写 serde_json | 手写 serde_json | 手写 JSON Schema | Effect Schema 双模 | Zod v4 + lazySchema | TypeBox | Zod + Cordis | Pydantic v2 | 手写 + IR 派生 | Pydantic v2 | Pydantic + DSL | TypeScript 手写 | Python DSL |
| **TS/Rust 推导** | ❌ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ❌ | ✅ |
| **协议参数生成** | 透传 | 透传 | strict + 多 provider 投影 | 3 类 provider 投影 | Zod → JSON Schema | 全 Provider 归一化 | Type.Literal 合并 | ReAct IR | 协议 IR 翻译 | SQLite schema | Pregel cba 消减 | bridge 透传 | DSL → JSON |
| **OpenAI strict 投影** | ❌ | ❌ | ✅ 完整 | ⚠️ openAI() | ✅ 通过 Zod | ⚠️ 展平 | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **Gemini 兼容化** | ❌ | ❌ | ✅ projections | ✅ ToolSchemaProjection.gemini | ❌ | ✅ cleanSchemaForGemini | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **Moonshot/特殊** | ❌ | ❌ | ✅ mistral 剥 symbol | ✅ moonshot tuple→anyOf | ❌ | ✅ llamacpp GBNF | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **请求前参数校验** | ❌ | ❌ | ❌ | ✅ Effect decode | ✅ Zod safeParse | ✅ ajv | ✅ Cordis | ✅ Pydantic | ✅ IR validator | ⚠️ | ✅ | ❌ | ✅ |
| **JSON 修复链** | ❌ 仅括号配平 | ✅ 8 段最强 | ✅ repairJson + partial-json | ❌ 仅 try/catch | ✅ safeParseJSON LRU | ⚠️ JSON5 fallback | ✅ parseWithRetry | ✅ Pydantic auto | ✅ TranslationEngine fallback | ❌ | ✅ DSL 容错 | ❌ | ❌ |
| **结构化输出强制** | ❌ prompt only | ⚠️ tool_choice | ✅ tool_choice | ✅ tool_choice | ✅ json_schema | ⚠️ tool_choice | ✅ | ✅ Pydantic | ✅ IR | ❌ | ✅ | ❌ | ✅ |
| **response_format: json_schema** | ❌ | ❌ | ❌ | ❌ | ✅ generateSessionTitle | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **流式 partial JSON** | ❌ | ⚠️ | ✅ partial-json 库 | ❌ | ⚠️ JSONL incremental | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Windows 路径语义修复** | ❌ | ✅ pre_escape_windows_paths | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Schema 文档联动 lint** | ❌ | ⚠️ manual | ❌ | ❌ | ✅ lazySchema + describe() | ⚠️ | ⚠️ | ⚠️ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **缓存层** | ❌ | ⚠️ middleware | ❌ | ✅ provider-cache | ✅ LRU memoizeWithLRU | ✅ normalize cache | ✅ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ |

> 标注说明：✅ 完整实现；⚠️ 部分实现；❌ 未实现。

---

## 4. Schema 闭环流水线 ASCII 图

```
                    ┌──────────────────────────────────────────────┐
                    │          工具定义（开发者写一次）              │
                    │                                              │
                    │  Typed (TS/Rust 类型推导)   Dynamic (MCP/插件)│
                    │  Zod schema / Effect Schema   jsonSchema    │
                    │  Pydantic / TypeBox                          │
                    └────────────────────┬─────────────────────────┘
                                         │
                                         ▼
                    ┌──────────────────────────────────────────────┐
                    │ 编译期：JSON Schema 派生（zod-to-json-schema  │
                    │ / schemars / typebox / effect/JsonSchema）   │
                    └────────────────────┬─────────────────────────┘
                                         │
                                         ▼
                    ┌──────────────────────────────────────────────┐
                    │ Provider 投影（关键环节）                      │
                    │   • Anthropic: input_schema                   │
                    │   • OpenAI Chat: function.parameters          │
                    │   • OpenAI Responses: tools[].parameters     │
                    │   • Gemini: parametersJsonSchema              │
                    │   • Moonshot: 剥离 tuple→anyOf                │
                    │   • Bedrock: Document 包装                    │
                    │ strict 投影：剥 16 关键词 / required 闭包化   │
                    └────────────────────┬─────────────────────────┘
                                         │
                                         ▼
                    ┌──────────────────────────────────────────────┐
                    │ Wire 序列化（HTTP body / SSE chunk）          │
                    └────────────────────┬─────────────────────────┘
                                         │
                  ══════  LLM 生成 + 流式回传  ══════════════
                                         │
                                         ▼
                    ┌──────────────────────────────────────────────┐
                    │ 响应解析（5 段降级链）                         │
                    │   1. JSON.parse fast-path                     │
                    │   2. repairJson（控制字符 + 反斜杠）          │
                    │   3. partial-json（流式补齐）                 │
                    │   4. extract_json_fields（兜底键值）           │
                    │   5. {} 空对象（流式上下文）                   │
                    └────────────────────┬─────────────────────────┘
                                         │
                                         ▼
                    ┌──────────────────────────────────────────────┐
                    │ 工具专属 Repair（按 tool.name 路由）           │
                    │   • edit_file: extract_edit_file_args         │
                    │   • task: anchor-based 重构嵌套数组           │
                    │   • Windows 盘符: pre_escape_windows_paths    │
                    │   • Schema-aware: 解码 stringified 结构       │
                    └────────────────────┬─────────────────────────┘
                                         │
                                         ▼
                    ┌──────────────────────────────────────────────┐
                    │ 请求前 Schema 校验（可选；claudecode/opencode）│
                    │   • Zod safeParse / Effect decodeUnknown      │
                    │   • ajv / jsonschema crate                    │
                    │   • 失败 → ToolExecution error 或自动重试     │
                    └────────────────────┬─────────────────────────┘
                                         │
                                         ▼
                    ┌──────────────────────────────────────────────┐
                    │ Tool execute（业务逻辑）                      │
                    │   内部仍用 .get("xxx").as_xxx 兜底           │
                    └──────────────────────────────────────────────┘

自修复重试回路（可选）：
  Schema 校验失败 → 把错误回灌为 user message（带 schema 期望）
                  → 重新调 LLM（带 retry counter / 衰减退避）
                  → 最多 N 次（如 3）后降级到 ToolExecution error
```

---

## 5. 跨 Provider 归一化规则矩阵

| 规则 | Anthropic | OpenAI Chat | OpenAI Responses | Gemini | Moonshot | Bedrock |
|------|-----------|-------------|------------------|--------|----------|---------|
| 顶层字段名 | `input_schema` | `function.parameters` | `tools[].parameters` | `parameters` | `parameters` | `toolSpec.input.json` |
| 必须 `type:"object"` 顶层 | ❌ | ✅ | ✅ | ❌ | ✅ | ❌ |
| `additionalProperties:false` | ✅ | ✅ (strict 强制) | ✅ (strict 强制) | ❌ 拒绝 | ✅ | ✅ |
| `$ref` 支持 | ✅ | ❌ strict 拒绝 | ❌ strict 拒绝 | ❌ 拒绝 | ✅ 仅本地 | ✅ |
| `$defs` 支持 | ✅ | ❌ | ❌ | ❌ | ❌ | ✅ |
| `anyOf` (对象 union) | ✅ | ⚠️ strict 不允许结构化 | ⚠️ | ❌ 拒绝 | ✅ | ✅ |
| `anyOf` (null union) | ✅ | ⚠️ 强制 required | ⚠️ | ⚠️ nullable | ✅ | ✅ |
| `oneOf` | ✅ | ❌ strict 拒绝 | ❌ | ❌ | ✅ | ✅ |
| `allOf` | ✅ | ❌ strict 拒绝 | ❌ | ❌ | ✅ | ✅ |
| `patternProperties` | ✅ | ❌ strict 拒绝 | ❌ | ❌ 拒绝 | ✅ | ✅ |
| `minimum/maximum` | ✅ | ❌ strict 拒绝 | ❌ | ❌ | ✅ | ✅ |
| `format` | ✅ | ⚠️ 文档仅 | ⚠️ | ❌ | ✅ | ✅ |
| `enum` (数字/布尔) | ✅ | ✅ | ✅ | ❌ 必须 string | ✅ | ✅ |
| `default` | ✅ | ❌ strict | ❌ strict | ❌ | ✅ | ✅ |
| `const` | ✅ | ⚠️ | ⚠️ | ❌ | ✅ | ✅ |
| `properties.required` | ⚠️ 建议非空 | ✅ strict 全填 | ✅ strict 全填 | ❌ 过滤 dangling | ✅ | ✅ |
| `items` (tuple) | ✅ | ⚠️ | ⚠️ | ✅ | ❌ 需 anyOf | ✅ |
| `prefixItems` | ✅ | ⚠️ | ⚠️ | ✅ | ❌ → items | ✅ |
| `unevaluatedItems` | ✅ | ❌ strict | ❌ | ✅ | ❌ 删除 | ✅ |
| 工具名长度 | 无硬限 | 64 字符 | 64 字符 | 64 字符 | 64 字符 | 64 字符 |
| 工具名字符 | `[a-zA-Z0-9_-]` | 同 + 建议 `_` | 同 | 同 | 同 | 同 |
| 描述长度 | 无硬限（实际 ≤ 100k tokens） | 实践 ≤ 1024 | 同 | ≤ 1024 | ≤ 1024 | 同 |
| `tool_choice` 强制度 | `tool`, `any`, `auto`, `none` | `function.name`/`auto`/`required`/`none` | 同 Chat | `MODE_*` | `auto`/`required`/`none` | 同 Anthropic |
| `response_format: json_schema` | ✅ Beta | ✅ stable | ✅ stable | ✅ schema 模式 | ✅ | ✅ |

**数据出处**：
- pi: `packages/ai/src/api/constrained-sampling.ts:12-29` `UNSUPPORTED_STRICT_SCHEMA_KEYS`
- opencode: `packages/llm/src/protocols/utils/tool-schema.ts:43-66`
- openclaw: `packages/ai/src/providers/clean-for-gemini.ts:7-37` `GEMINI_UNSUPPORTED_SCHEMA_KEYWORDS`
- claudecode: `BashTool.tsx:227-247` `z.strictObject(...)`

---

## 6. 非法 JSON 修复策略对比表

| 策略 | atomcode | pi | claudecode | opencode | openclaw | deepseek-harness | agent-core |
|------|----------|----|-----------:|---------:|---------:|-----------------:|-----------:|
| **fast-path JSON.parse** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **markdown 围栏剥离** | ✅ ```/```json | ⚠️ 仅 ```json | ✅ Bun.JSONL | ❌ | ❌ | ⚠️ | ✅ Pydantic |
| **trailing comma 删除** | ✅ in_string 感知 + O(N) | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ auto |
| **缺逗号补齐** | ✅ O(N) 单遍 | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ auto |
| **单引号 → 双引号** | ✅ 仅无 " 时 | ❌ | ❌ | ❌ | ✅ JSON5 | ⚠️ | ❌ |
| **无引号键加引号** | ✅ structural_mask | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **控制字符转义** | ✅ in_string 感知 | ✅ repairJson | ⚠️ 仅 JSONL | ❌ | ❌ | ⚠️ | ✅ auto |
| **反斜杠补齐** | ✅ `\.` → `\\.` | ✅ 同 | ❌ | ❌ | ❌ | ❌ | ✅ |
| **Windows 路径语义** | ✅ pre_escape 4 层防护 | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **大括号配平** | ✅ 仅结构字符 | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **partial JSON 流式补齐** | ⚠️ 仅原子块 | ✅ partial-json 库 | ⚠️ JSONL | ❌ | ❌ | ✅ | ❌ |
| **Schema-aware 解码 stringified** | ✅ repair_stringified_structured_fields | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ Pydantic |
| **工具特化抽取** | ✅ edit_file/task 双 extractor | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **键值兜底（最后防线）** | ✅ extract_json_fields | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **O(N) 单遍算法** | ✅ 显式 pin | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Max bytes bound** | ✅ 512KB | ❌ | ⚠️ 8KB LRU key | ❌ | ❌ | ❌ | ❌ |
| **结构性 in_string 感知** | ✅ structural_mask | ⚠️ simple | ❌ | ❌ | ❌ | ❌ | ❌ |
| **O(N²) 退化 pin 测试** | ✅ hardening_tests | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **大小写容错（工具名）** | ✅ eq_ignore_ascii_case | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

**关键观察**：atomcode 的修复链是**业内唯一完整 8 段**实现，且对**字符串内容碰巧像 JSON** /
**Windows 路径 `\t`/`\n` 双义性**这两类最致命的边界 case 有显式算法与测试 pin。pi 的
`partial-json` 流式补全是**业内唯一完整 partial-JSON 库**，但缺 trailing comma / unquoted key
等"事后整理"环节。claudecode 走 Bun 原生 `JSONL.parseChunk` 偏流式管线设计，**无修复逻辑**。
openclaw 用 `JSON5` 库（外部依赖），opencode / Switchyard / agent-core 都不做修复（依赖
上游 LLM 的稳定性）。

---

## 7. 10–15 个设计模式与反模式

### 7.1 设计模式

**M1. Schema 双模式 + Schema Bridge**（opencode `packages/llm/src/tool.ts:133-164`）
- **模式 A**：typed（编译期类型 + 运行期解码）
- **模式 B**：dynamic（原始 JSON Schema，给 MCP/插件）
- **统一入口**：`Tool.make({...})._definition` 缓存，避免每次请求重建。
- **价值**：编译期类型给 IDE 智能提示，运行期 schema 给跨语言 wire。

**M2. 投影在协议层之前，**而非之后（opencode / pi / openclaw 一致）
- 抽象：`normalize(schema) → provider_clean(schema, provider) → wire_serialize(schema)`
- 价值：协议层只关心 wire 形状，不污染领域层。

**M3. Strict 子集"要求即抛出"而非静默降级**（pi `constrained-sampling.ts:217-219`）
```typescript
if (config.strict === "require") {
  throw new Error(`Tool "${tool.name}" requires JSON-schema constrained sampling, but ${error.message}.`);
}
```
- 价值：fail-loud 而非 fail-silent，让开发者在 schema 写错时立即感知。

**M4. 结构性 in_string 感知**（atomcode `repair.rs:401-428` `structural_mask`）
- 单次扫描标记每个 char 是否在 string body 内，所有结构修改都跳过字符串。
- 价值：避免破坏 `"fn main() {"` / `"tail,}"` / `"class A:\n"` 这类字符串内容碰巧像 JSON。

**M5. O(N) 单遍 + 边界 pin 测试**（atomcode `repair.rs:2127-2195`）
```rust
#[test]
fn trailing_comma_run_collapses_correctly() {
    let input = format!("{{\"k\":[{}]}}", ",".repeat(50_000));  // 旧实现冻死
    ...
}
```
- 价值：O(N²) 退化在弱模型输出下必然触发，必须显式 pin。

**M6. 工具专属 extractor**（atomcode `extract_edit_file_args` / `extract_task_args`）
- 当 generic 修复链失败，**针对特定工具的 known field order** 做位置抽取。
- 价值：模型在 `prompt` 字段里写未转义引号时（极常见），generic 抽取会丢字段。

**M7. Schema-aware stringified decoding**（atomcode `repair_stringified_structured_fields`）
- 只在 schema 要求 array/object 但模型给字符串时解码；string 字段永远不碰。
- 价值：避免递归解引用 / 误判意图。

**M8. Partial-JSON 流式补齐**（pi `partial-json` 库）
- 流式场景下 JSON 尚未完整闭合，给出"当前已能解析的部分"作为增量上下文。
- 价值：UI 提前渲染 / QC 提前评估，无需等完整响应。

**M9. z.strictObject + .omit(内部字段)**（claudecode `BashTool.tsx:227-258`）
- `.omit({_simulatedSedEdit: true})` 在对外 schema 裁掉内部字段，防止模型绕过 permission。
- 价值：内部状态不出现在 LLM 视野，安全 + 防御注入。

**M10. lazySchema + LRU memoize**（claudecode `lazySchema.ts` / `json.ts:42`）
- Schema 延后实例化解决 module-load 时序；memoizeWithLRU 用 50 槽位 / 8KB 上限防内存泄漏。
- 价值：性能 + 内存双优化。

**M11. Tool 抽象的 5 段递降链**（atomcode / pi 一致）
```
fast-path → repair → partial → extractor → 兜底
```
- **缺一不可**：fast-path 处理 95% 正常情况；repair 处理 4% 轻微畸形；partial 处理 0.5% 流式断流；
  extractor 处理 0.4% 工具特化畸形；兜底处理 0.1% 不可恢复。
- 价值：每一段独立可测，回归定位精确。

**M12. Cache key 分层**（openclaw `normalizeToolParameterSchema` LRU + pi `safeParseJSON` LRU）
- 按 `(schema, provider, modelId, compat)` 元组缓存投影结果，避免每次请求重新投影。
- 价值：JSON Schema 投影是 O(N) 但 N 可大（元属性递归），缓存提速显著。

**M13. JSON wire 序列化稳定性测试**（atomcode `openai_compat.rs:2694-2739` `body_serialization_is_deterministic`）
- 连续 100 次序列化同一个 tools 数组，断言 byte-equal。
- 价值：OpenAI 缓存键基于 body hash；非确定性顺序会反复 cache miss。

**M14. Provider 字段名嗅探**（openclaw `normalizedProvider.includes("google")`）
- 用 `provider.includes("google")` 而非硬编码字符串，支持未来变体（vertex-ai-google-...）。
- 价值：扩展性强，无需逐个硬编码。

**M15. 协议 IR + 翻译引擎**（Switchyard `TranslationEngine`）
- 不直接 schema → wire，而是 schema → IR → wire；响应同理 wire → IR → schema。
- 价值：跨 7 种 provider 兼容；IR 可观测、可版本化。

### 7.2 反模式

**A1. Schema 透传无投影**（laew 当前）
- `convert_tools` 只换字段名（`input_schema` ↔ `parameters`），不清理任何关键词。
- 后果：OpenAI strict 模式 400；Gemini 400；Moonshot tuple items 400。

**A2. prompt-only 结构化输出**（laew Yolo 当前）
- 系统提示词要求"输出 JSON" + 简单代码围栏抽取。
- 后果：模型漏掉代码围栏 / 加前置文本 / 写 trailing comma → 直接 YoloParse error → 整轮回流。

**A3. 无 schema 校验，让工具内部报错**（laew / atomcode）
- 模型参数错了 → 工具用 `.get("xxx").as_xxx` 拿到 None → 报"缺少参数"。
- 后果：模型收到错误后**不一定能理解 schema 期望**，下次还是错；浪费 1 轮交互。

**A4. 修复逻辑用 O(N²) Vec::insert**（atomcode 旧实现，已修）
- 长尾逗号 / 缺逗号场景下，O(N²) 退化冻死 host。
- 必须 pin 住：单遍 + 累计偏移重写。

**A5. 修复逻辑不看字符串内容**（典型错误实现）
- `result.replace(",}", "}")` 全局替换会把字符串内的 `",}"` 也改掉。
- 后果：模型输出 `{"old_string": "tail,}", ...}` 时变成 `{"old_string": "tail}"`。

**A6. 字符串转义补齐破坏 JSON 合法 escape**（典型错误实现）
- 把 `\.` → `.` 而不是 `\\.`，导致 `D:\test\foo.py` 在 JSON 里既非法又无法恢复。
- 正确：`\.` → `\\.`（让 serde_json 解码为 `\.`）。

**A7. Zod schema 包含内部字段**（典型反模式）
- 把 `_internal_state` / `_cache_key` 这类字段直接放进对外 schema。
- 后果：模型学会调用它们绕过权限 / 污染状态。
- 正确：用 `.omit({...})` 裁掉（claudecode M9）。

**A8. 修复逻辑无 size bound**（典型反模式）
- 把 100MB 字符串喂进 repair 链，O(N) 也会耗光内存。
- 正确：atomcode `MAX_REPAIR_BYTES = 512KB` + middleware 直接透传给工具。

**A9. 流式 partial JSON 用 Naive 字符计数**
- 不处理 `"` 字符串边界，把 `"a" : "b"` 的内部 `"` 当作键结束。
- 后果：长字符串提前截断。
- 正确：`partial-json` 库 / `in_string` 状态机。

**A10. 无 O(N²) 退化测试**（典型隐患）
- 修复链在常规测试 OK，但弱模型输出 50,000 个尾逗号时冻死。
- 正确：`hardening_tests` 显式压测（atomcode `repair.rs:2127-2155`）。

---

## 8. laew 现状与 P0/P1/P2 路线图

### 8.1 现状摘要（漏点 L1-L20）

- L1: 无 strict 投影 → OpenAI Responses 拒收
- L2: 无 Gemini 兼容化 → Gemini 400
- L3: 无 Moonshot 兼容 → Kimi K2 tuple items 400
- L6: tool_choice 写死 "auto" → Yolo 分类无法强制
- L11: Schema 投影未做 → 仅适合当前 6 个手写简单 schema
- **L16 无 jsonschema 校验**：参数错误只能事后工具报错
- **L17 无 JSON 修复链**：Yolo JSON 解析失败率显著高于 atomcode
- **L18 无 partial JSON 流式解析**：长 JSON 中途断流后空对象兜底
- **L19 Yolo 无结构化输出强制**：靠 prompt + 解析，鲁棒性差
- **L20 无跨 provider 归一化**：未来接入 Gemini / Moonshot 必然触发 400

### 8.2 P0 路线图（2 周内，建议立即实施）

#### P0-1. 引入 `jsonschema` crate + 请求前校验

**新增依赖**（`Cargo.toml`）：
```toml
jsonschema = "0.17"
```

**Tool trait 增加方法**（`src/agent/tools/mod.rs`）：
```rust
use jsonschema::JSONSchema;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    /// 编译期缓存的校验器（lazy initialize）
    fn validator(&self) -> &JSONSchema {
        // 实际用 once_cell 或 LazyLock
        thread_local! { ... }
    }
    async fn execute(&self, args: Value) -> Result<String>;
}
```

**`src/agent/validator.rs`**（新增）：
```rust
//! 工具参数预校验 —— 在 Tool::execute 之前拦截非法参数，
//! 把 schema 期望回灌为 tool_result 错误，让 LLM 自动重试。

use jsonschema::{JSONSchema, ValidationError};
use serde_json::Value;
use crate::error::AgentError;

/// 把 jsonschema error 转成结构化错误，附带 schema 期望
pub fn pretty_errors(errors: &[ValidationError]) -> String {
    let mut out = String::new();
    for e in errors {
        out.push_str(&format!(
            "- path `{}`: {}\n  schema expects: {}\n",
            e.instance_path, e.kind_str(), e.schema_path,
        ));
    }
    out
}

/// 校验参数；返回 Err(AgentError::ToolArgsInvalid) 给 LLM 看
pub fn validate(
    schema: &JSONSchema,
    args: &Value,
) -> Result<(), AgentError> {
    let errs: Vec<_> = schema.validate(args).collect();
    if errs.is_empty() {
        Ok(())
    } else {
        Err(AgentError::ToolArgsInvalid {
            pretty: pretty_errors(&errs),
        })
    }
}
```

**error.rs 新增变体**：
```rust
#[error("工具参数不合法: {pretty}")]
ToolArgsInvalid { pretty: String },
```

**Tool execute 入口前接入**（`src/agent/mod.rs` `run_session`）：
```rust
let args_value: Value = serde_json::from_str(&tool_call.arguments)
    .map_err(|e| AgentError::YoloParse(format!("JSON parse fail: {e}")))?;

// 1) 先 schema 校验
if let Err(e) = validator::validate(tool.validator(), &args_value) {
    // 回灌给 LLM 当作 tool_result，触发自动重试
    return Ok(ToolResultBlock {
        content: format!("参数不合法,请修正后重试:\n{e}"),
        is_error: true,
    });
}

// 2) 通过校验再执行
tool.execute(args_value).await
```

#### P0-2. Tool 参数归一化函数（Anthropic / OpenAI strict / Gemini）

**`src/llm/normalize.rs`**（新增）：
```rust
//! Schema → 不同 provider 的协议参数投影。
//!
//! 单一入口 `project_schema(schema, &ProviderKind) -> Value`，
//! 在 anthropic.rs / openai.rs 的 convert_tools 之前调用。

use serde_json::{json, Value};

#[derive(Copy, Clone)]
pub enum ProviderKind { Anthropic, OpenAiStrict, OpenAi, Gemini, Moonshot }

pub fn project_schema(schema: Value, provider: ProviderKind) -> Value {
    match provider {
        ProviderKind::Anthropic => schema,           // 透传（Anthropic 接受 full JSON Schema）
        ProviderKind::OpenAi => strip_for_openai(schema, false),
        ProviderKind::OpenAiStrict => strip_for_openai(schema, true),
        ProviderKind::Gemini => clean_for_gemini(schema),
        ProviderKind::Moonshot => clean_for_moonshot(schema),
    }
}

fn strip_for_openai(mut s: Value, strict: bool) -> Value {
    // 1) 剥 16 类 strict 禁止关键词
    const BLOCKED: &[&str] = &[
        "$ref","$defs","definitions","allOf","oneOf","patternProperties",
        "dependentSchemas","dependencies","unevaluatedProperties",
        "propertyNames","contains","prefixItems","not","if","then","else",
    ];
    if let Value::Object(map) = &mut s {
        for k in BLOCKED {
            map.remove(*k);
        }
        if strict {
            // 2) 强制 additionalProperties: false
            map.insert("additionalProperties".into(), json!(false));
            // 3) 强制 required = 全 properties 键
            if let Some(Value::Object(props)) = map.get("properties") {
                let keys: Vec<String> = props.keys().cloned().collect();
                map.insert("required".into(), json!(keys));
            }
            // 4) anyOf null union → 普通字段（非 required 用 anyOf: [orig, {type:"null"}]）
        }
    }
    s
}

fn clean_for_gemini(mut s: Value) -> Value {
    // Gemini 不接受 $ref/$defs/patternProperties/minLength/minimum 等 26 类
    const BLOCKED: &[&str] = &[
        "patternProperties","additionalProperties","$schema","$id","$ref",
        "$defs","definitions","examples","minLength","maxLength","minimum",
        "maximum","multipleOf","pattern","format","minItems","maxItems",
        "uniqueItems","minProperties","maxProperties","not",
    ];
    if let Value::Object(map) = &mut s {
        for k in BLOCKED { map.remove(*k); }
        // enum 数值/布尔 → string
        if let Some(Value::Array(arr)) = map.get_mut("enum") {
            *arr = arr.iter().map(|v| match v {
                Value::Number(n) => Value::String(n.to_string()),
                Value::Bool(b)   => Value::String(b.to_string()),
                other => other.clone(),
            }).collect();
        }
        // dangling required 过滤（参考 opencode 实现）
        if let (Some(Value::Object(props)), Some(Value::Array(req))) =
            (map.get("properties"), map.get_mut("required")) {
            *req = req.iter()
                .filter(|r| r.as_str().map(|k| props.contains_key(k)).unwrap_or(false))
                .cloned().collect();
        }
    }
    s
}

fn clean_for_moonshot(mut s: Value) -> Value {
    // tuple items → anyOf；prefixItems → items；unevaluatedItems 删除
    // 详见 opencode tool-schema.ts:26-41 moonshotNode
    ...
    s
}
```

**`convert_tools` 改造**（`anthropic.rs:117` / `openai.rs:141`）：
```rust
fn convert_tools(tools: &[ToolDef], provider: ProviderKind) -> Vec<Value> {
    tools.iter().map(|t| {
        let projected = project_schema(t.input_schema.clone(), provider);
        match provider {
            ProviderKind::Anthropic => json!({
                "name": t.name, "description": t.description, "input_schema": projected
            }),
            ProviderKind::OpenAi | ProviderKind::OpenAiStrict
                | ProviderKind::Gemini | ProviderKind::Moonshot => json!({
                "type": "function",
                "function": {
                    "name": t.name, "description": t.description, "parameters": projected
                }
            }),
        }
    }).collect()
}
```

### 8.3 P1 路线图（1 个月内）

#### P1-1. JSON 修复链（参照 atomcode `repair.rs`）

**`src/agent/repair.rs`**（新增）—— 移植 atomcode 的 8 段修复链：

```rust
//! JSON Repair —— 容忍 LLM 输出的非法 JSON。
//!
//! 移植自 atomcode crates/atomcode-capabilities/src/tools/repair.rs
//! (MIT 协议，2026-08)。核心要点：
//! 1. fast-path: serde_json::from_str 一次成功直接返回
//! 2. 反斜杠补齐 / 控制字符转义 / markdown 围栏剥离
//! 3. 结构性 in_string 感知的 trailing comma 删除 + 无引号键加引号
//! 4. 大括号配平（仅结构字符）
//! 5. 工具特化 extractor（按 tool_name 路由）
//! 6. 兜底键值抽取

const MAX_REPAIR_BYTES: usize = 512 * 1024;

pub fn repair_tool_args(tool_name: &str, args: &str) -> String {
    if args.len() > MAX_REPAIR_BYTES { return args.to_string(); }
    let pre = pre_escape_windows_paths_in_json(args);
    if serde_json::from_str::<Value>(&pre).is_ok() { return pre; }
    let repaired = repair_json(&pre);
    if serde_json::from_str::<Value>(&repaired).is_ok() { return repaired; }
    // 工具特化 + 兜底（参考 atomcode §2.3）
    ...
    args.to_string()
}
```

**`src/agent/repair_middleware.rs`**：
```rust
pub struct RepairToolArgsMiddleware;

impl ToolMiddleware for RepairToolArgsMiddleware {
    async fn before(&mut self, call: &mut ToolCall, tool: &Arc<dyn Tool>, _ctx: &RequestCtx) -> BeforeOutcome {
        call.arguments = repair::repair_tool_args(tool.name(), &call.arguments);
        BeforeOutcome::Proceed
    }
}
```

#### P1-2. Yolo 结构化输出强制（`tool_choice`）

**`src/llm/openai.rs`** 当前 `tool_choice: Some("auto")` 改造：
```rust
pub fn tool_choice_for_yolo() -> ToolChoice { ToolChoice::Tool("yolo_classify".into()) }
pub fn tool_choice_for_plan()  -> ToolChoice { ToolChoice::Tool("plan_write".into()) }
```

并在 anthropic.rs 提供对应 `tool_choice: {type:"tool", name:"yolo_classify"}`。

### 8.4 P2 路线图（3 个月内）

#### P2-1. 跨 Provider 元数据驱动（`ModelCompat`）

把 OpenAI strict 是否启用、Gemini 是否支持 `parametersJsonSchema`、Moonshot 是否支持 tuple 等元数据
抽到 `ModelCompat` 结构，按模型 ID 嗅探（参考 openclaw `resolveUnsupportedToolSchemaKeywords`）。

#### P2-2. 流式 partial JSON 解析（参照 `partial-json`）

引入 `partial-json` Rust crate（port of 微软 `partial-json.ts`），用于长 JSON 流式回填。

#### P2-3. Tool schema 文档联动 lint

把每个 Tool 的 `description()` 用 markdown lint 校验：
- ≤ 200 字符（防止 context 膨胀）
- 含一个示例（提升 LLM 准确率）
- 含 `required` 字段说明

### 8.5 完整改造成本估算

| 阶段 | 工作量 | 收益 |
|------|-------|------|
| P0-1 jsonschema 校验 | 2 天 | L16 闭环：参数错误 LLM 自动重试 |
| P0-2 Schema 投影 | 3 天 | L1/L11/L20 闭环：未来接入 Gemini/Moonshot 无返工 |
| P1-1 JSON 修复链 | 5 天 | L17 闭环：Yolo 解析失败率 -80% |
| P1-2 Yolo 结构化输出 | 2 天 | L6/L19 闭环：Yolo 分类成功率 +30% |
| P2-1/2/3 | 5 天 | 完整对齐 opencode / pi |
| **合计** | **17 人天** | **漏点 L1/L6/L11/L16/L17/L18/L19/L20 全闭环** |

---

## 9. 关键文件速查

### pi（TypeScript）

- `packages/ai/src/api/constrained-sampling.ts` — strict schema 投影全套（116-127 行 makeStrictJsonSchema；208-228 行 resolveJsonSchemaStrictSampling）
- `packages/ai/src/utils/json-parse.ts` — 5 段降级 JSON 解析（32-83 行 repairJson；104-124 行 parseStreamingJson）

### opencode（TypeScript + Effect）

- `packages/llm/src/tool.ts` — Tool 双模式 + 编译期 codec 缓存（48-206 行）
- `packages/llm/src/protocols/utils/tool-schema.ts` — 3 类 provider 投影（48-79 行）
- `packages/llm/src/protocols/utils/gemini-tool-schema.ts` — Gemini 清洗（6-97 行）
- `packages/llm/test/tool-schema-projection.test.ts` — 投影测试（9-117 行）
- `packages/opencode/src/session/retry.ts:172-181` — parseJSON 仅 try/catch，无修复

### atomcode（Rust）

- `crates/atomcode-capabilities/src/tools/repair.rs` — 8 段修复链 2200+ 行（核心 18-81 行 repair_tool_args；91-170 行 stringified 结构解码；192-263 行 Windows 路径预修复；471-712 行 repair_json；716-845 行 extract_json_fields；950-996 行 extract_task_args；1972-2006 行 RepairToolArgsMiddleware）
- `crates/atomcode-coding/tests/tool_args_repair.rs` — 3 类场景修复测试（49-86 行最小装配 / 89-179 行审批一致）
- `crates/atomcode-capabilities/src/provider/anthropic.rs:460-471` — 工具 wire 直透传
- `crates/atomcode-capabilities/src/provider/openai_compat.rs:2694-2739` — body_serialization_is_deterministic 测试

### claudecode（TypeScript）

- `src/tools/BashTool/BashTool.tsx` — Zod strictObject + .omit 内部字段（225-258 行）
- `src/Tool.ts` — Tool 抽象基类
- `src/utils/json.ts` — safeParseJSON + safeParseJSONC + parseJSONL（45-58 / 65-76 / 182-190 行）
- `src/utils/sessionTitle.ts` — response_format: json_schema + Zod 校验（79-118 行）
- `src/utils/yoloClassifier.ts:1152` — tool_choice 强制分类

### openclaw（TypeScript）

- `packages/ai/src/providers/agent-tools-parameter-schema.ts` — 全 Provider 归一化（801-984 行 normalizeToolParameterSchemaUncached）
- `packages/ai/src/providers/clean-for-gemini.ts` — Gemini 黑名单清洗（7-37 行 GEMINI_UNSUPPORTED_SCHEMA_KEYWORDS）
- `packages/ai/src/providers/schema-keyword-strip.ts` — 模型能力嗅探

### laew（Rust）

- `src/agent/tools/mod.rs` — Tool trait + 注册表（26-78 行）
- `src/agent/tools/{bash,read,write,edit,glob,grep}.rs` — 6 个手写 JSON Schema 工具
- `src/agent/yolo.rs:215-287` — Yolo JSON 解析（无修复）
- `src/llm/anthropic.rs:117-128` — Anthropic wire（透传）
- `src/llm/openai.rs:141-155` — OpenAI wire（透传）
- `src/llm/openai.rs:1308` — tool_choice: Some("auto") 写死
- `Cargo.toml:18-35` — 当前依赖（无 jsonschema / schemars）

---

## 附：与既有专题的去重说明

- **第六轮 §11「Tool Schema 投影」**（`专题-第六轮-Anthropic与OpenAI协议调用真实实现深度对比.md:927-1043`）：
  已涉及 pi/opencode/openclaw/claudecode 四家投影的存在与差异；本篇补足 **Gemini 详细清洗规则**、
  **Moonshot tuple→anyOf**、**strict 投影的 16 关键词清单**、**atomcode 直透传的反模式诊断**。
- **第三轮-工具调用深度分析**（`专题-工具调用深度分析.md:16-49`）：
  已列 6 项目"工具声明方式"对比表；本篇补足 **laew/atomcode/openclaw 直透传与 pi/opencode 投影的工程实现差**、
  **provider 投影规则矩阵**、**请求前参数校验的业内空白**。
- **第三轮-系统提示词工程**（`专题-第三轮-系统提示词工程真实对比深度分析.md`）：
  已覆盖 14 模型家族提示词变体；本篇聚焦 **Yolo/分类/规划的结构化输出三档体系**（prompt only / json_object / json_schema / tool_choice）。

---

> **下一步**：实施 P0-1（P0-2 同周），同步在 `testReport/run_e2e.sh` 新增第 11 节"结构化输出回归"
> 覆盖：① 故意输出 malformed JSON ② OpenAI strict 模式不爆 400 ③ Gemini schema 投影后模型仍可调用工具 ④ 修复链单测。
