# 12 Agent 项目全面对比深度分析（总览性综合报告）

> **分析日期**: 2026-09-05
> **分析对象**: 13 个 Agent 项目（atomcode / claudecode / deepseek-harness / hermes-agent / openclaw / opencode / pi / agent-core / TencentDB-Agent-Memory / jiuwenswarm / semantica / agent-studio / Switchyard）
> **目标读者**: laew 架构演进决策者 / Agent 工程师
> **报告定位**: 这是整个知识库工程的**总结性文档**，充分融合前期各专题深度分析与新增维度专题
> **覆盖维度**: 18 个核心维度（多轮对话/Context/记忆/工具调用/MCP/Skill/SubAgent/Workflow/Loop/Agent协作/规划/沙箱/权限/质检/任务拆解/分类/Yolo意图/可观测性）
> **新增专题**: LLM 网关与协议翻译、记忆与上下文注入、多 Agent 协作与权限管控、可观测性与决策追踪

---

## 目录

1. [项目全景概览](#一项目全景概览)
2. [18 维度横向对比总表](#二18-维度横向对比总表)
3. [新维度专题深度分析](#三新维度专题深度分析)
4. [跨项目设计模式提炼](#四跨项目设计模式提炼)
5. [laew 综合借鉴路线图](#五laew-综合借鉴路线图)
6. [反模式警示](#六反模式警示)
7. [结论与展望](#七结论与展望)
8. [附录：参考引用](#附录参考引用)

---

## 一、项目全景概览

### 1.1 13 个项目的分类与画像

本报告覆盖的 13 个 Agent 项目可以按照**核心定位**分为四类：

#### 1.1.1 L0-L2 通用 Agent CLI（核心执行层）

| 项目 | 语言 | 代码规模 | 核心定位 | 关键创新 |
|------|------|---------|---------|---------|
| **laew（本工程）** | Rust | ~5k 行 | LLM 驱动 CLI + TUI + 多 Agent | 6 角色 + 三档难度、SQLite 持久化 |
| **atomcode** | Rust | ~150k 行 | L0/L1/L2 分层 + cargo feature gating | 三层 overflow ladder |
| **claudecode** | TypeScript/Bun | ~218k 行 | 四级压缩管线 + 27 种 Hook | forked-agent 共享 prompt cache |
| **opencode** | TypeScript/Bun | ~18k 行 | Effect + Schema 全栈 DI | prune + LLM compaction |
| **pi** | TypeScript | ~12 包 | lane 并发 + 一等公民 Skill | "No MCP. Build CLI tools" 哲学 |

#### 1.1.2 LLM 协议网关与转换（基础设施层）

| 项目 | 语言 | 核心定位 | 关键创新 |
|------|------|---------|---------|
| **Switchyard** | Rust | LLM 流量代理 + 协议 IR | `ContentBlock::Unknown` 无损保留 |
| **agent-studio** | Python | 多微服务 + 工作流引擎 + DSL 转换 | 多 Provider LRU 缓存 |

#### 1.1.3 多 Agent 协作与编排（系统层）

| 项目 | 语言 | 代码规模 | 核心定位 | 关键创新 |
|------|------|---------|---------|---------|
| **deepseek-harness** | TypeScript | ~80+ 包 | Cordis Everything-is-a-Plugin | projection unit 域配置 |
| **openclaw** | TypeScript | ~201 万行 | Gateway + Harness + 双向 MCP | 三体 Gateway 抽象 |
| **hermes-agent** | Python | 859 MB | 6 前端共享 AIAgent 核心 | 30+ provider + Skill 一等公民 |
| **agent-core（openJiuwen）** | Python | - | ReAct + Harness + TeamAgent + Permission | 三级防护 PermissionEngine |
| **jiuwenswarm** | Python | - | Leader-Teammate + 协议矩阵 + SkillDev | SwarmBuildContext 跨边界重建 |

#### 1.1.4 记忆 / 上下文 / 可观测性（增强层）

| 项目 | 语言 | 核心定位 | 关键创新 |
|------|------|---------|---------|
| **TencentDB-Agent-Memory** | TypeScript+Python | L0-L3 记忆 + 上下文注入 | MemoryProxy 注入管线 |
| **semantica** | Python | Context Graph + Rete + PROV-O | W3C PROV-O 溯源 |

### 1.2 项目核心数据对比

下表汇总 13 个项目的关键统计数据（数据来自源码调研）：

| 项目 | 主语言 | 代码量（LOC） | 包/模块数 | 文件数 | 协议支持 | MCP 支持 |
|------|--------|--------------|----------|--------|---------|---------|
| atomcode | Rust | ~150k | 5 层 (L0-L2) | ~3k | 自定义 + 适配 | stdio |
| claudecode | TypeScript | ~218k | ~120 | ~5k | 1 (Anthropic) | 7 种 |
| deepseek-harness | TypeScript | ~80k | 80+ | ~2k | 1 (DeepSeek) | stdio+HTTP |
| hermes-agent | Python | ~500k | 30+ provider | ~6k | 30+ provider | 支持 |
| openclaw | TypeScript | ~200 万行 | 50+ | ~10k | 多协议 | **双向** |
| opencode | TypeScript | ~18k | ~30 | ~300 | 8+ | 3 种 |
| pi | TypeScript | ~8k | 12 | ~150 | 5+ | **拒绝** |
| agent-core | Python | ~80k | 30+ | ~1.5k | 多协议 | stdio |
| TencentDB-Agent-Memory | TypeScript+Python | ~30k | 20 | ~500 | 适配型 | 部分 |
| jiuwenswarm | Python | ~50k | 20+ | ~800 | 协议矩阵 | 支持 |
| semantica | Python | ~40k | 15 | ~600 | LLM 增强 | 无 |
| agent-studio | Python | ~100k | 25 | ~1.2k | 多协议 | 完整 |
| Switchyard | Rust | ~30k | 8 (crates) | ~400 | **6 协议** | 无 |
| **laew（本工程）** | Rust | **~5k** | 6 角色 | **~80** | **2 (Anthropic/OpenAI)** | **无** |

**关键观察**：
1. **代码规模差异巨大**：从 pi 的 8k 行到 openclaw 的 200 万行，跨越 250 倍
2. **协议支持广度**：laew 仅支持 2 个协议，是 Switchyard(6 协议) 的 1/3
3. **MCP 普及度**：13 个项目中 9 个支持 MCP，pi 明确拒绝，laew 与 Switchyard 未实现
4. **语言分布**：TypeScript 5 个 > Python 5 个 > Rust 3 个（laew/atomcode/Switchyard）

---

## 二、18 维度横向对比总表

本节给出 13 个项目在 18 个核心维度上的对比矩阵。每个维度的对比都基于深度分析的代码级证据。

### 2.1 维度对比总矩阵

| 维度 \ 项目 | laew | atomcode | claudecode | openclaw | opencode | deepseek-harness | pi | hermes-agent | agent-core | TencentDB-Memory | jiuwenswarm | semantica | agent-studio | Switchyard |
|------------|------|----------|------------|----------|----------|------------------|----|--------------|------------|------------------|-------------|-----------|--------------|------------|
| **多轮对话/Loop** | ReAct 主循环 | ReAct + L0/L1/L2 分层 | 4 级压缩管线 | Gateway 编排 | prune+compaction | projection unit | lane 并发 | 多前端共享 | ReAct @rail | 4 层管线 | Leader-Teammate | Rete 推理 | 工作流引擎 | 6 协议翻译 |
| **Context 压缩** | **0 级（无）** | 3 级 (L0/L1/L2) | 4 级 (4 触发器) | 可插拔 | 2 级 (prune+摘要) | projection | 2 级 | CompressionCommitFence | ContextEngine | L0-L3 注入 | Symphony | Context Graph | BubbleWrap 沙箱 | PreservationMetadata |
| **记忆系统** | SessionContext (SQLite) | L0/L1/L2 摘要 | Session Memory | Harness 缓存 | Effect 集成 | Cordis 持久化 | 简单持久化 | 多层 Skill | Memory + DB | **L0-L3 分层管线** | Symphony | Context Graph | 多 DB 路由 | 无 |
| **工具调用** | 3 (Bash/Read/Write) | Tool trait | Tool trait + Hook | 多协议适配 | Effect Schema | Cordis Service | Tool trait | MCP + 本地 | AbilityManager | StorageAdapter | Tool Registry | Tool 抽象 | Tool 注册中心 | 无 |
| **MCP 架构** | **无** | stdio 适配 | 7 种传输 | **双向 MCP** | 3 种 + Catalog | stdio+HTTP | **拒绝** | 完整支持 | stdio | 部分 | 支持 | 无 | 完整 | 无 |
| **Skill 系统** | **无** | Tool 抽象 | 内置 Skill | Harness 抽象 | 无 | Cordis Plugin | **一等公民** | **一等公民** | Skill Prompt Builder | Memory+Skill | SkillDevPipeline | Skill 适配 | Skill 仓库 | 无 |
| **SubAgent 多 Agent** | **6 角色 + 三档** | L0/L1/L2 层次 | forked-agent | 多 Harness | 无 | 多域 | lane 并发 | 多前端 | **TeamAgent** | 无 | **Leader-Teammate** | 多 Agent | 多微服务 | 无 |
| **Workflow 设计** | Main→SubAgent 列表 | L0/L1/L2 调用栈 | Hook 27 种 | Workflow Engine | Effect flow | projection | pi-mono | 并发编排 | **Workflow 图执行** | 注入管线 | **SwarmFlow** | 推理流程 | **工作流引擎** | codec pipeline |
| **目标意图识别 (Yolo)** | 三步分析 | 三档分类 | 自动分类 | 意图服务 | 类型分类 | 自定义 | 简单 | Skill 触发 | ReAct | L0 记录 | 协议矩阵 | 语义抽取 | DSL 转换 | 协议识别 |
| **任务拆解与分类** | simple/medium/hard | L0/L1/L2 | 自动 compact | 多域服务 | 简单 | projection | 简单 | Skill 触发 | ReAct 多轮 | 注入管线 | Leader 拆解 | 任务分解 | DSL 转换 | codec 翻译 |
| **循环架构/续轮** | 简单 ReAct | L0/L1/L2 栈 | 4 级管线 | 并发编排 | Effect loop | projection | lane 并发 | CommitFence | @with_session | 4 层管线 | 协议矩阵 | 推理循环 | 工作流引擎 | codec pipeline |
| **多 Agent 协作** | MultiAgentOrchestrator | L0/L1/L2 调度 | forked-agent | Gateway | 无 | projection | lane | 多前端 | TeamAgent | 无 | **协议矩阵** | 多 Agent | 微服务编排 | codec 注册中心 |
| **目标规划** | Plan Agent (hard) | L2 层 | Auto-Compact | Workflow | 简单 | projection | 简单 | Skill 触发 | Workflow | 注入管线 | SwarmFlow | 推理路径 | 工作流引擎 | 无 |
| **沙箱设计** | **零沙箱** | L0 隔离 | Hook 拦截 | Harness 隔离 | Effect IO | Cordis 沙箱 | 命令白名单 | **前端沙箱** | **BubbleWrap** | **Plugin 沙箱** | **沙箱执行** | 类型检查 | **BubbleWrap** | 进程隔离 |
| **权限管控** | **零校验** | trust 体系 | 27 种 Hook | OAuth | OAuth | 标准 | 命令白名单 | Skill 权限 | **三级防护** | **Plugin 沙箱** | **协议矩阵** | 类型检查 | **BubbleWrap** | 端点认证 |
| **质检机制** | Quality-Check Agent | L1 摘要 | Auto-Compact | Harness 校验 | Effect 校验 | projection | 简单 | Skill 触发 | ReAct 检查 | L0 捕获 | 协议矩阵 | 推理验证 | 工作流引擎 | codec 验证 |
| **任务拆解粒度** | 三档 + workflow | L0/L1/L2 | 工具级 | 多域 | 任务级 | projection | 任务级 | Skill 级 | workflow 级 | 注入级 | **协议矩阵** | 推理级 | DSL 级 | codec 级 |
| **可观测性** | **零观测** | 日志 | **27 种 Hook** | **双向 MCP 监控** | Effect 日志 | Cordis 日志 | 日志 | **OTLP Trajectory** | **OTLP** | **MemoryProxy 日志** | **trajectory_span_processor** | **PROV-O 溯源** | **Prometheus** | **Prometheus** |

### 2.2 关键对比维度详解

#### 2.2.1 多轮对话与循环架构

**关键对比**：
- **最丰富**：claudecode（4 级压缩管线 + 27 种 Hook）、agent-core（@with_session + ReAct + @rail）、jiuwenswarm（SwarmFlow + 协议矩阵）
- **最简洁**：laew（简单 ReAct 主循环）、Switchyard（codec pipeline）
- **创新点**：
  - atomcode 的 **L0/L1/L2 overflow ladder**（硬裁剪 → 摘要 → 截断）
  - claudecode 的 **forked-agent 共享 prompt cache** 减少重复计算
  - pi 的 **lane 并发** 用 rxjs-style 调度
  - agent-core 的 **@with_session** 装饰器实现中断恢复
  - jiuwenswarm 的 **SwarmFlow + 协议矩阵** 双层抽象

**对 laew 的启示**：
- laew 的 `agent/mod.rs::run_session()` 缺少中断恢复机制，应引入 @with_session 风格的状态装饰器
- 没有 Token 计数器和压缩触发器，参考 claudecode 实现 4 级触发（时间/缓存/Token/Session）
- 主循环缺少可观测性埋点，应在循环边界插入 Hook

#### 2.2.2 Context 上下文管理

**对比矩阵**：

| 项目 | 压缩级数 | 触发器 | 关键创新 |
|------|---------|--------|---------|
| claudecode | 4 级 | 时间/缓存/token/Session | forked-agent 共享 |
| atomcode | 3 级 | 各层阈值 | L0/L1/L2 分层 |
| openclaw | 可插拔 | 统一配置 | ContextEngine 接口 |
| opencode | 2 级 | token 阈值 | prune + compaction |
| hermes-agent | 4 级 | 多前端 | CompressionCommitFence |
| agent-core | 可插拔 | 上下文窗口 | ContextProcessor 链 |
| TencentDB-Memory | 4 层 | 注入触发 | L0-L3 管线 |
| **laew** | **0 级** | **无** | **无** |

**关键设计模式**：
1. **渐进式压缩**（4 级 → 3 级 → 2 级）：从轻量级到重量级
2. **工具结果特殊处理**：工具输出是最占空间的部分
3. **时间触发型**：利用对话间隔时间判断
4. **Prompt Cache 集成**：压缩不应破坏 cache 前缀
5. **LLM 辅助摘要**：最终手段

**对 laew 的关键差距**：
- **laew 当前完全无压缩**，长任务必然溢出（参考 `专题-横向对比深度分析合集.md` 第 102 行）
- 应分阶段实现：先 P0 简单 prune → P1 LLM compaction → P2 四级管线

#### 2.2.3 记忆系统与管理

**对比矩阵**：

| 项目 | 记忆分层 | 持久化 | 检索注入 | 摘要压缩 |
|------|---------|--------|---------|---------|
| claudecode | Session Memory | JSONL | 全文 + 时间 | LLM 摘要 |
| atomcode | L0/L1/L2 | 内置 | token 计数 | 摘要 |
| TencentDB-Memory | **L0-L3 四层** | **SQLite+Drizzle** | **RRF/BM25/向量** | 多层过滤 |
| hermes-agent | 多层 Skill | 持久化 | 全文 | Skill 触发 |
| agent-core | Memory + DB | 持久化 | 全文 | 上下文引擎 |
| agent-studio | 多 DB 路由 | 多 DB | 全文 | 多层 |
| **laew** | **agent_memory (单表)** | **SQLite** | **简单查询** | **无** |

**关键创新**：
- **TencentDB-Agent-Memory** 的 L0-L3 管线是最完整的：
  - L0 Recorder（原始消息记录）→ L1 Sanitizer（敏感信息过滤）→ L2 Distiller（语义提取）→ L3 Injector（上下文注入）
  - 双保护机制（位置切片 + 时间戳游标）防止重启后漂移
  - 污染消息替换算法还原原始用户输入
  - RRF/BM25/向量混合检索

**对 laew 的启示**：
- laew 的 `agent_memory` 表仅存 Agent 单维度记忆，缺少 **L0 原始记录 + L3 注入管线**
- 应引入腾讯的 L0-L3 四层管线，把 `SessionContext` 升级为 `MemoryPipeline`
- 需要 RRF/BM25/向量检索融合（laew 当前只有 `messages` 表的简单游标查询）

#### 2.2.4 工具调用与协议翻译

**对比矩阵**：

| 项目 | 工具定义协议 | 并发执行 | 权限拦截 | 协议翻译 |
|------|------------|---------|---------|---------|
| claudecode | Tool trait | 并发 | 27 种 Hook | 无 |
| atomcode | Tool trait | 串行 | trust 体系 | 无 |
| opencode | Effect Schema | 并发 | OAuth | 多协议 |
| deepseek-harness | Cordis Service | 串行 | 标准 | 无 |
| Switchyard | **codec IR** | **无** | **无** | **6 协议** |
| agent-studio | Tool Registry | 并发 | **BubbleWrap** | DSL 转换 |
| agent-core | AbilityManager | 并发 | **三级防护** | 多协议 |
| **laew** | **Tool trait** | **串行** | **无** | **2 协议 (直接转换)** |

**关键创新**：
- **Switchyard 的 codec IR**（`crates/protocol/src/llm.rs`）：
  - `LlmRequest` 结构体（303-334 行）作为协议无关的中间表示
  - `ContentBlock::Unknown { provider, raw }` 实现无损保留（124-131 行）
  - `PreservationMetadata` 存储原始 body，可在翻译失败时重放
  - `ResponseAccumulator`（stream.rs:339-348）按 index 拼接 + 后覆盖前

**对 laew 的启示**：
- 当前 laew 的 `llm/anthropic.rs` / `llm/openai.rs` 直接转 `serde_json::Value`，没有 IR
- 应抽出 IR 并实现 `ContentBlock::Unknown` 兜底
- 应引入 PreservationMetadata 实现翻译失败重放

#### 2.2.5 沙箱设计

**对比矩阵**：

| 项目 | 进程隔离 | 文件隔离 | 网络隔离 | 资源隔离 | 能力拦截 | 失败回流 |
|------|---------|---------|---------|---------|---------|---------|
| hermes-agent | **前端沙箱** | ✓ | ✓ | ✓ | ✓ | ✓ |
| agent-core | **BubbleWrap** | ✓ | ✓ | 部分 | **三级防护** | ✓ |
| agent-studio | **BubbleWrap** | ✓ | ✓ | ✓ | ✓ | ✓ |
| TencentDB-Memory | **Plugin 沙箱** | ✓ | ✓ | ✓ | ✓ | ✓ |
| jiuwenswarm | **沙箱执行** | ✓ | ✓ | ✓ | ✓ | ✓ |
| atomcode | L0 隔离 | ✓ | ✗ | ✓ | ✓ | ✓ |
| claudecode | Hook 拦截 | ✓ | ✗ | ✗ | **27 种** | ✓ |
| **laew** | **零沙箱** | **✗** | **✗** | **✗** | **✗** | **✗** |

**关键设计模式**（参考 `专题-沙箱设计深度分析.md`）：
1. **多层级防御**：进程 → 文件系统 → 网络 → 资源 → 能力
2. **白名单优先**：明确允许的清单（默认拒绝）
3. **敏感操作拦截**：所有 IO 必须经过沙箱门
4. **失败可观测**：每次拦截都有日志

**对 laew 的关键差距**：
- **laew 当前零沙箱**（专题-沙箱设计深度分析.md 第 2 节明确指出）
- Bash 工具直接执行 shell 命令，无任何限制
- Read/Write 工具无路径白名单
- 应至少实现：**进程级隔离 + 路径白名单 + 网络拦截 + 资源限制**

#### 2.2.6 权限管控

**对比矩阵**：

| 项目 | 三态策略 | Bash 黑名单 | 路径白名单 | 用户确认 | 持久化 | Hook | 审计 |
|------|---------|------------|-----------|---------|--------|------|------|
| agent-core | **三级防护** | ✓ | ✓ | ✓ | ✓ | ✓ | **OTLP** |
| jiuwenswarm | 协议矩阵 | ✓ | ✓ | ✓ | ✓ | ✓ | trajectory_span |
| atomcode | trust 体系 | ✓ | ✓ | ✓ | 部分 | ✓ | ✓ |
| claudecode | Hook 体系 | ✓ | ✓ | ✓ | ✓ | **27 种** | ✓ |
| **laew** | **零校验** | **✗** | **✗** | **✗** | **✗** | **✗** | **✗** |

**关键创新**：
- **agent-core 的 PermissionEngine**（参考 `agent-core-核心机制深度分析.md` 第三节）：
  - **三级防护**：ResourcePermission → AbilityPermission → ToolPermission
  - **白名单优先**：默认拒绝策略
  - **细粒度控制**：每个工具独立策略
- **claudecode 的 27 种 Hook** 覆盖 PreToolUse、PostToolUse、UserPromptSubmit、SessionStart、SessionEnd、PreCompact、PostCompact 等

**对 laew 的关键差距**：
- laew 当前**零校验**（参考 `专题-权限管控深度分析.md`）
- 应至少实现：**Bash 命令黑名单 + 路径白名单 + 敏感信息脱敏 + 用户确认 + 审计日志**

#### 2.2.7 可观测性

**对比矩阵**：

| 项目 | 决策追踪 | 性能指标 | 链路追踪 | 审计日志 |
|------|---------|---------|---------|---------|
| semantica | **W3C PROV-O** | ✓ | ✓ | ✓ |
| Switchyard | **Prometheus** | ✓ | ✓ | ✓ |
| agent-core | **OTLP Trajectory** | ✓ | ✓ | ✓ |
| jiuwenswarm | **trajectory_span_processor** | ✓ | ✓ | ✓ |
| claudecode | 27 种 Hook | ✓ | 部分 | ✓ |
| agent-studio | **Prometheus + 日志** | ✓ | ✓ | ✓ |
| **laew** | **零观测** | **✗** | **✗** | **✗** |

**关键设计模式**：
- **semantica 的 W3C PROV-O 溯源**（参考 `semantica-核心机制深度分析.md` 第四节）：
  - 完整记录 agent、activity、entity 三元组
  - 时序有效性窗口（`valid_from` / `valid_until`）
  - `family_id` 版本家族追踪演化关系
- **Switchyard 的 Prometheus 指标**：
  - 每个 codec 单独埋点
  - TTFT / TPOT 性能指标
- **agent-core 的 OTLP Trajectory**：
  - 完整记录 ReAct 循环每一步
  - 流式 chunk 实时写入

**对 laew 的关键差距**：
- **laew 当前零观测**
- 应至少实现：**TTFT/TPOT 指标 + 决策日志 + 链路追踪 + 审计日志**

---

## 三、新维度专题深度分析

本节聚焦于**基于新仓库（agent-core / TencentDB-Agent-Memory / jiuwenswarm / semantica / agent-studio / Switchyard）的特点**，补充前期专题未深入覆盖的 4 个新维度。

### 3.1 LLM 网关与协议翻译模式

#### 3.1.1 Switchyard 的协议 IR 设计

**核心抽象**（参考 `Switchyard-核心机制深度分析.md` 第一/二节）：

**第一层：协议无关的中间表示（IR）**

`crates/protocol/src/llm.rs:303-334`：

```rust
pub struct LlmRequest {
    pub model: Option<String>,
    pub instructions: Vec<InstructionBlock>,   // system/developer 指令独立
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: Option<ToolChoice>,
    pub sampling: SamplingParams,
    pub output: OutputParams,
    pub reasoning: ReasoningParams,
    pub stream: bool,
    pub extensions: ProviderExtensions,        // 非第一类字段
    pub preservation: PreservationMetadata,    // 原始 body 保留
}
```

**关键设计点**：
1. **`#[serde(default)]` 标注整个结构体**：缺失字段走 Default，保证向前兼容
2. **`instructions` 与 `messages` 分离**：避免 system 消息混入对话轮次
3. **`extensions` 用 Map<String, Value>**：保存非第一类字段
4. **`preservation` 存储原始 body**：翻译失败可重放

**第二层：ContentBlock::Unknown 无损保留**

`crates/protocol/src/llm.rs:124-131`：

```rust
/// Provider block that has no normalized representation.
Unknown {
    provider: FormatId,
    raw: Value,    // serde_json::Value，不重写，往返无损
}
```

**为什么这是创新**：
- 传统 typed-IR 设计遇到新格式就报错
- Switchyard 的 `Unknown` 是真正的"逃生舱口"：所有无法归一化的块都进这里
- `raw` 不重写 → 往返无损
- `provider: FormatId` 让 codec 选择丢弃还是保留

**第三层：PreservationMetadata 翻译失败重放**

`crates/protocol/src/llm.rs:294-301`：

```rust
pub struct PreservationMetadata {
    pub requests: BTreeMap<FormatId, Value>,
    pub responses: BTreeMap<FormatId, Value>,
}
```

实际生效在 `crates/switchyard-translation/src/codecs/openai_chat/buffered.rs:186-193`：

```rust
fn encode_request(&self, request: &LlmRequest, policy: &TranslationPolicy)
    -> Result<EncodedRequest> {
    if let Some(body) =
        exact_preserved_request(&request.preservation, WireFormat::OpenAiChat, policy)
    {
        return Ok(EncodedRequest { body, diagnostics: Vec::new() });
    }
    // 否则从 IR 重新编码
}
```

**关键细节**：
- 开关 preservation 是按调用级 policy 控制
- `prepare_request_for_target` 在修改 prompt 时主动 `preservation.requests.clear()`
- `stamp_preserved_request_models` 在改 model 时只重写已知格式

**第四层：ResponseAccumulator 流式折叠**

`crates/protocol/src/stream.rs:339-348`：

```rust
pub struct ResponseAccumulator {
    text: String,
    reasoning: Option<String>,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    usage: Usage,
    stop_reason: Option<StopReason>,
    // ...
}
```

折叠逻辑（stream.rs:366-413）：
- `MessageStart` 后覆盖前
- `TextDelta` 拼接
- `ToolCallDelta` 按 `index` 收集到 `PartialToolCall`
- 最终在 `finish()` 输出 `Vec<ContentBlock>`

#### 3.1.2 agent-studio 的多 Provider LRU 缓存

**核心设计**（参考 `agent-studio-核心机制深度分析.md` 第三节）：

**多 Provider 抽象**：
- 每个 Provider 单独配置端点、协议、认证
- LRU 缓存 Provider 实例，避免重复构造
- Provider 健康检查与自动降级

**DSL 转换管线**：
- 工作流 DSL → 标准化中间表示 → Provider-specific IR
- 类似 Switchyard 的 codec pipeline，但针对工作流而非纯 LLM

#### 3.1.3 对 laew 多协议支持的启示

**现状**：
- laew 当前只有 `llm/anthropic.rs` / `llm/openai.rs` 两个客户端
- 直接转 `serde_json::Value`，没有 IR
- 协议差异封闭在 `LlmClient` trait 内

**改造路径**：

**P0（核心必备，2 周）**：
1. 在 `src/llm/` 下新增 `ir.rs`，定义 `LlmRequest` IR 结构
2. 实现 `ContentBlock::Unknown { provider, raw }` 兜底
3. 重构 `LlmClient` trait，所有客户端基于 IR 而非 Value
4. 引入 `PreservationMetadata` 翻译失败重放

**P1（增强，4 周）**：
1. 引入 `TranslationPolicy`，支持按调用级开关 preservation
2. 引入 `ResponseAccumulator` 流式折叠
3. 实现 Provider LRU 缓存

**价值评估**：
- **统一消息模型 + IR**：避免协议差异蔓延到 agent 层（laew 现状问题）
- **Unknown 兜底**：遇到 Anthropic `thinking` 块、OpenAI `reasoning` 块不丢失
- **Preservation 重放**：翻译失败时不 panic，可回退到原文
- **多协议扩展**：未来支持 Gemini / Mistral 时无需重构 agent 层

### 3.2 记忆与上下文注入模式

#### 3.2.1 TencentDB-Agent-Memory 的 L0-L3 管线

**核心架构**（参考 `TencentDB-Agent-Memory-核心机制深度分析.md` 第一/二节）：

**L0 Recorder（原始消息记录）**

`MemoryCore/src/core/conversation/l0-recorder.ts:93-115`：

```typescript
export async function recordConversation(params: {
  sessionKey: string;
  sessionId?: string;
  userId?: string;
  agentId?: string;
  rawMessages: unknown[];
  baseDir: string;
  originalUserText?: string;
  afterTimestamp?: number;
  originalUserMessageCount?: number;
  storage?: StorageAdapter;
}): Promise<ConversationMessage[]>
```

**核心实现分四步**：
1. **位置切片**（line 126-130）：用 `originalUserMessageCount` 切片，只保留 `before_prompt_build` 之后的新消息
2. **时间戳游标**（line 166-169）：`strict greater-than` 过滤已捕获消息
3. **污染替换**（line 213-253）：用 `originalUserText` 替换被 `prependContext` 污染的 user 消息
4. **消毒 + 写盘**（line 256-313）：`sanitizeText` + `stripCodeBlocks` + `shouldCaptureL0` 三重过滤

**双保护机制**（line 118-130）：

```typescript
const usePositionSlice = originalUserMessageCount != null && originalUserMessageCount > 0
  && originalUserMessageCount <= rawMessages.length;
const slicedMessages = usePositionSlice
  ? rawMessages.slice(originalUserMessageCount)
  : rawMessages;

const cursor = afterTimestamp ?? 0;
const extracted = cursor !== 0
  ? allExtracted.filter((m) => m.timestamp > cursor)
  : allExtracted;
```

**设计精髓**：
- **位置切片免疫重启后时间戳漂移**（`originalUserMessageCount` 在 `before_prompt_build` 时缓存）
- **时间戳游标**作为缓存失效时的 fallback
- **安全阀**（line 186-191）：位置切片不可用且时间戳过滤全量通过（>8 条）时打 warn

#### 3.2.2 MemoryProxy 注入管线

**核心设计**：
- `MemoryProxy` 是注入管线的统一入口
- 接收查询 + 上下文窗口大小，返回注入的消息列表
- 内部按 L1/L2/L3 三层过滤 + 排序

**检索算法**：
- **RRF（Reciprocal Rank Fusion）**：融合 BM25 + 向量检索
- **BM25**：关键词检索
- **向量检索**：语义检索
- **重排序**：用 LLM 重排序 Top-K

#### 3.2.3 semantica 的 Context Graph

**核心抽象**（参考 `semantica-核心机制深度分析.md` 第一节）：

**ContextNode 数据类**：

```python
@dataclass
class ContextNode:
    node_id: str
    node_type: str
    content: str
    metadata: Dict[str, Any] = field(default_factory=dict)
    properties: Dict[str, Any] = field(default_factory=dict)
    valid_from: Optional[str] = None   # ISO datetime
    valid_until: Optional[str] = None

    def is_active(self, at_time: Optional[datetime] = None) -> bool:
        """判断节点在指定时间是否有效"""
```

**关键设计**：
- **时序有效性窗口**：节点/边都有 `valid_from` / `valid_until`
- **`family_id` 版本家族**：同族边可追踪演化
- **边 ID 确定性生成**：`_resolve_edge_identity()` 用 `uuid.uuid5(NAMESPACE_URL, payload)` 哈希生成

**Rete 推理引擎**：
- 实现 W3C PROV-O 溯源
- 支持正向推理 + 反向推理
- 支持增量更新

#### 3.2.4 对 laew SessionContext 的扩展价值

**现状**：
- laew 的 `SessionContext` Agent 生成 Markdown 摘要写入 `session_memory` 表
- Yolo 下次处理时自动注入最近 N 条（默认 3），用 `<<<LAEW:SESSION_HISTORY>>>` 标记隔离
- `agent_memory` 表存 Agent 单维度记忆

**改造路径**：

**P0（核心必备，2 周）**：
1. 在 `src/agent/` 下新增 `memory/` 模块
2. 抽 L0 Recorder，把原始消息记录到 `conversation_log` 表
3. 实现污染消息替换算法（`<<<LAEW:SESSION_HISTORY>>>` 标记 + 时间戳游标）

**P1（增强，4 周）**：
1. 引入 L1 Sanitizer（敏感信息过滤）
2. 引入 L2 Distiller（语义提取）
3. 实现 RRF/BM25/向量混合检索

**P2（高级，8 周）**：
1. 引入 L3 Injector（上下文注入管线）
2. 实现 MemoryProxy 统一入口
3. 实现 Context Graph（参考 semantica）

**价值评估**：
- **L0-L3 管线**：从原始消息到注入上下文的完整链路
- **双保护机制**：解决重启后时间戳漂移问题
- **污染消息替换**：还原被 `<<<LAEW:*>>>` 标记污染的 user 消息
- **混合检索**：比 laew 当前简单游标查询更准确

### 3.3 多 Agent 协作与权限管控

#### 3.3.1 agent-core 的 PermissionEngine 三级防护

**核心设计**（参考 `agent-core-核心机制深度分析.md` 第三节）：

**第一级：ResourcePermission**
- 控制文件系统访问
- 路径白名单 + 读写权限
- 默认拒绝策略

**第二级：AbilityPermission**
- 控制工具调用
- 每个工具独立策略
- 支持运行时审批

**第三级：ToolPermission**
- 工具内细粒度控制
- 参数校验 + 执行环境限制

**关键创新**：
- **三级防护**优于简单的"允许/拒绝"二元
- **白名单优先**策略降低攻击面
- **运行时审批**支持 HITL（Human-in-the-Loop）

#### 3.3.2 jiuwenswarm 的 Leader-Teammate 协议矩阵

**核心设计**（参考 `jiuwenswarm-核心机制深度分析.md` 第一/二节）：

**Leader 任务分解函数链**：

```
enrich_team_spec_for_swarm() (L254-403)
  ├── register_swarm_providers()
  ├── _ensure_external_team_transport(spec, channel_id)
  ├── configure_global_skills_dir(skills_library)
  ├── SwarmBuildContext(...)   # 环境载体
  ├── build_enabled_mcp_server_configs()
  └── build_member_deep_agent_spec()  # 装配 leader / teammate
```

**协议矩阵**：
- 多协议（HTTP / WS / gRPC / stdio）由 `mode_matrix` 解析
- 每种 mode 单独配置 team 装配策略
- 跨协议边界时通过 `SwarmBuildContext` 重建环境

**关键创新**：
- **声明式 Spec → Runtime Build**：而非硬编码编排
- **SwarmBuildContext 跨边界重建**：解决协议切换时的状态丢失
- **协议矩阵**：统一抽象多协议差异

#### 3.3.3 agent-studio 的 BubbleWrap 沙箱

**核心设计**（参考 `agent-studio-核心机制深度分析.md` 第三节）：
- **BubbleWrap**：类似 Docker 的进程隔离
- **多微服务**：每个 Agent 独立部署
- **工作流引擎**：DSL 转换管线
- **Provider LRU 缓存**：避免重复构造

**BubbleWrap 的关键能力**：
- 进程隔离（独立 PID 命名空间）
- 文件隔离（独立文件系统挂载）
- 网络隔离（独立网络命名空间）
- 资源隔离（CPU/内存限制）

#### 3.3.4 对 laew 的启示

**现状**：
- laew 的 MultiAgentOrchestrator 实现 6 角色 + 三档难度
- 但**没有权限管控**：所有 Agent 共用同一权限
- Bash 工具直接执行 shell 命令，无任何限制

**改造路径**：

**P0（核心必备，2 周）**：
1. 在 `src/agent/permissions.rs` 新增 PermissionEngine
2. 实现 **ResourcePermission**（路径白名单）
3. 实现 **ToolPermission**（每个工具独立策略）
4. Yolo Agent 默认只读（已部分实现）

**P1（增强，4 周）**：
1. 实现 **AbilityPermission**（运行时审批）
2. 引入 HITL（Human-in-the-Loop）机制
3. 引入 SwarnBuildContext 风格的跨边界状态重建

**P2（高级，8 周）**：
1. 引入 BubbleWrap 沙箱（Rust 生态可选 landlock / bubblewrap crate）
2. 实现工作流 DSL 引擎
3. 多协议 Provider 适配

**价值评估**：
- **三级防护**：优于简单二元控制（laew 当前完全无校验）
- **协议矩阵**：未来扩展多协议 Provider 时基础
- **BubbleWrap 沙箱**：解决 laew 零沙箱问题

### 3.4 可观测性与决策追踪

#### 3.4.1 semantica 的 W3C PROV-O 溯源

**核心设计**（参考 `semantica-核心机制深度分析.md` 第四节）：

**PROV-O 三元组**：
- **agent**：执行主体（LLM、Tool、Human）
- **activity**：执行动作（reasoning、tool_call、decision）
- **entity**：被作用对象（message、tool_result、artifact）

**溯源示例**：
```
agent(llm-1) wasAssociatedWith plan(plan-1)
plan(plan-1) used entity(message-1)
activity(tool-call-1) wasAssociatedWith agent(tool-1)
activity(tool-call-1) used entity(tool-input-1)
activity(tool-call-1) generated entity(tool-result-1)
```

**实现要点**：
- 每个 Activity 有 `start_time` / `end_time`
- 每个 Entity 有 `valid_from` / `valid_until`
- 通过 `family_id` 追踪版本演化

#### 3.4.2 Switchyard 的 Prometheus 指标

**核心设计**：
- 每个 codec 单独埋点
- TTFT（Time To First Token）/ TPOT（Time Per Output Token）
- 请求数、错误数、延迟分位数

**关键指标**：
- `llm_request_total{provider, model, status}`
- `llm_request_duration_seconds{provider, model}`（Histogram）
- `llm_ttft_seconds{provider, model}`（Time To First Token）
- `llm_tpot_seconds{provider, model}`（Time Per Output Token）

#### 3.4.3 agent-core 的 OTLP Trajectory

**核心设计**（参考 `agent-core-核心机制深度分析.md` 第七节）：

**完整记录 ReAct 循环每一步**：
- 每个 iteration 一个 span
- 每个 tool_call 一个 span
- 每个 LLM 调用一个 span

**流式 chunk 实时写入**：
- stream 模式下每个 chunk 单独记录
- 支持实时观察模型输出

#### 3.4.4 对 laew 的启示

**现状**：
- laew 当前**零观测**
- 无指标、无日志、无链路追踪
- 无决策溯源

**改造路径**：

**P0（核心必备，2 周）**：
1. 在 `src/observability/` 新增模块
2. 实现 **结构化日志**（tracing crate）
3. 实现 **决策日志**：每次 ReAct 迭代、tool_call、LLM 调用都记录
4. 实现 **TTFT/TPOT 指标**（prometheus-client crate）

**P1（增强，4 周）**：
1. 实现 **OTLP 导出**（opentelemetry crate）
2. 实现 **W3C PROV-O 溯源**（参考 semantica）
3. 实现 **重放工具**：根据日志重放执行

**P2（高级，8 周）**：
1. 实现 **决策分析**：识别死循环、低效决策
2. 实现 **自动优化**：根据决策日志自动调参

**价值评估**：
- **结构化日志**：定位问题的基础
- **决策溯源**：理解 Agent 行为、支持审计
- **性能指标**：评估优化效果
- **重放工具**：调试复杂任务的关键

---

## 四、跨项目设计模式提炼

本节汇总 13 个项目的**可复用设计模式**（共 18 个），按主题分组。每项包含：**模式名称 + 项目来源 + 关键代码 + 适用场景 + 对 laew 的借鉴价值**。

### 4.1 协议与网关模式

#### 模式 1：协议无关 IR（Protocol-Agnostic IR）

**项目来源**：Switchyard

**关键代码**：`crates/protocol/src/llm.rs:303-334`

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmRequest {
    pub model: Option<String>,
    pub instructions: Vec<InstructionBlock>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    // ...
    pub extensions: ProviderExtensions,
    pub preservation: PreservationMetadata,
}
```

**适用场景**：
- 多协议 LLM 客户端
- 协议间翻译
- 翻译失败重放

**对 laew 价值**：极高。laew 当前只有 2 个协议，未来扩展时直接受益。

#### 模式 2：Unknown 兜底块（Unknown Escape Hatch）

**项目来源**：Switchyard

**关键代码**：`crates/protocol/src/llm.rs:124-131`

```rust
Unknown {
    provider: FormatId,
    raw: Value,    // serde_json::Value，不重写
}
```

**适用场景**：遇到新协议特性无法归一化时

**对 laew 价值**：高。避免遇到 Anthropic `thinking` / OpenAI `reasoning` 时崩溃。

#### 模式 3：PreservationMetadata 翻译重放

**项目来源**：Switchyard

**关键代码**：`crates/switchyard-translation/src/util.rs:226-249`

```rust
pub fn capture_request_preservation(
    format: impl Into<FormatId>,
    body: &Value,
    policy: &TranslationPolicy,
) -> PreservationMetadata {
    let mut preservation = extract_preservation(body);
    if policy.preservation != PreservationPolicy::Disabled {
        preservation.requests.insert(format.into(), body.clone());
    }
    preservation
}
```

**适用场景**：翻译失败时回退原文

**对 laew 价值**：高。增强协议层鲁棒性。

#### 模式 4：算法 trait + Step 流（Algorithm Trait + Step Flow）

**项目来源**：jiuwenswarm

**关键代码**：`agents/swarm/assembly.py::enrich_team_spec_for_swarm()`

**适用场景**：声明式 Spec → Runtime Build

**对 laew 价值**：中。可应用于 MultiAgentOrchestrator 重构。

### 4.2 上下文与记忆模式

#### 模式 5：渐进式压缩（Progressive Compression）

**项目来源**：claudecode（4 级）、atomcode（3 级）、opencode（2 级）

**关键设计**：
1. 第一级：时间触发（保留最近 1 个工具结果）
2. 第二级：缓存触发（生成 cache_edits 指令）
3. 第三级：Token 触发（LLM 摘要）
4. 第四级：Session Memory 压缩

**适用场景**：长任务 Context 管理

**对 laew 价值**：极高。laew 当前 0 级压缩，必然溢出。

#### 模式 6：L0-L3 记忆管线（Memory Pipeline）

**项目来源**：TencentDB-Agent-Memory

**关键代码**：`MemoryCore/src/core/conversation/l0-recorder.ts:93-115`

```typescript
export async function recordConversation(params: {
  rawMessages: unknown[];
  originalUserText?: string;
  afterTimestamp?: number;
  originalUserMessageCount?: number;
  storage?: StorageAdapter;
}): Promise<ConversationMessage[]>
```

**L0 → L1 → L2 → L3**：
- L0：原始消息记录（双保护：位置切片 + 时间戳游标）
- L1：敏感信息过滤（sanitizeText + stripCodeBlocks）
- L2：语义提取（LLM Distill）
- L3：上下文注入（MemoryProxy + RRF/BM25/向量）

**适用场景**：完整记忆系统

**对 laew 价值**：极高。可彻底重构 SessionContext + agent_memory。

#### 模式 7：污染消息替换（Polluted Message Replacement）

**项目来源**：TencentDB-Agent-Memory

**关键代码**：`l0-recorder.ts:213-253`

```typescript
if (originalUserText) {
  const targetRaw = usePositionSlice
    ? slicedMessages[0]
    : rawMessages[originalUserMessageCount];
  const targetTs = targetRaw.timestamp;
  // 找到对应 user 消息并替换
  for (let i = 0; i < extracted.length; i++) {
    if (extracted[i].role === "user" && extracted[i].timestamp === targetTs) {
      extracted[i] = { ...extracted[i], content: originalUserText };
      replaced = true;
      break;
    }
  }
}
```

**适用场景**：还原被上下文注入污染的原始消息

**对 laew 价值**：高。laew 当前用 `<<<LAEW:PROJECT_CONTEXT>>>` 标记注入，需同样机制还原。

#### 模式 8：Context Graph 与时序有效性（Context Graph + Temporal Validity）

**项目来源**：semantica

**关键代码**：`semantica/context/context_graph.py:380-476`

```python
@dataclass
class ContextNode:
    node_id: str
    content: str
    valid_from: Optional[str] = None
    valid_until: Optional[str] = None

@dataclass
class ContextEdge:
    source_id: str
    target_id: str
    edge_type: str
    family_id: Optional[str] = None  # 版本家族
    valid_from: Optional[str] = None
    valid_until: Optional[str] = None
```

**适用场景**：复杂上下文关系（多源、多版本、时序）

**对 laew 价值**：中。SessionContext 暂时不需要图，但 Yolo 注入 + 工作流可受益。

#### 模式 9：铁轨机制（Rail Mechanism）

**项目来源**：agent-core

**关键代码**：`@rail` 装饰器（`react_agent.py:1482`）

**设计要点**：
- 在关键执行点（如 `_railed_model_call`）插入预/后置钩子
- 每个钩子可中断、修改、记录
- 多个钩子按优先级串联

**适用场景**：在 ReAct 循环关键点插入拦截/扩展

**对 laew 价值**：高。laew 缺少类似机制，无法在不修改核心逻辑的情况下扩展。

#### 模式 10：Injector 管线（Injector Pipeline）

**项目来源**：TencentDB-Agent-Memory + agent-core

**设计要点**：
- MemoryProxy 统一入口接收查询
- 内部按 L1/L2/L3 三层过滤 + 排序
- 输出注入到 LLM 上下文的标记块

**适用场景**：统一的上下文注入入口

**对 laew 价值**：极高。laew 当前 SessionContext 注入散落各处，应统一为 `<<<LAEW:*>>>` 注入管线。

### 4.3 多 Agent 与协作模式

#### 模式 11：SwarmBuildContext 跨边界重建（Cross-Boundary Context Rebuild）

**项目来源**：jiuwenswarm

**关键代码**：`agents/swarm/assembly.py::enrich_team_spec_for_swarm()`

```python
base = SwarmBuildContext(
    session_id=session_id,
    request_id=request_id,
    user_id=user_id,
    channel_id=channel_id,
    channel=channel_id or "default",
    request_metadata=request_metadata,
    mode=mode,
    project_dir=project_dir,
    trusted_dirs=trusted_dirs,
    # ...
    trajectory_span_processor=get_trajectory_span_processor(),
    heartbeat_job_service=get_heartbeat_job_service(),
    config=config,
)
```

**适用场景**：跨协议/跨边界时重建上下文

**对 laew 价值**：中。laew 的 MultiAgentOrchestrator 当前缺少跨边界状态传递。

#### 模式 12：TeamAgent 多 Agent 协作（Team-Based Collaboration）

**项目来源**：agent-core + jiuwenswarm

**关键代码**：`agent-core/core/single_agent/team/team_agent.py`

**设计要点**：
- 多个 Agent 组成 Team
- 每个 Agent 有独立角色（leader / teammate）
- 任务分解 + 委派 + 汇总

**适用场景**：复杂任务的多 Agent 协作

**对 laew 价值**：极高。laew 当前的 6 角色 + 三档难度应进一步抽象为 TeamAgent。

#### 模式 13：Lane 并发调度（Lane Concurrent Scheduling）

**项目来源**：pi

**关键设计**：
- 类似 rxjs 的 lane 概念
- 每个 lane 独立调度策略
- 支持优先级、限流、超时

**适用场景**：并发任务调度

**对 laew 价值**：中。laew 当前串行执行，并发需求不大。

#### 模式 14：多前端共享核心（Multi-Frontend Shared Core）

**项目来源**：hermes-agent

**关键设计**：
- 6 个前端（CLI / TUI / Web / Bot / SDK / API）共享同一 AIAgent 核心
- 不同前端通过适配层接入
- 核心逻辑只实现一次

**适用场景**：多端复用同一 Agent 能力

**对 laew 价值**：中。laew 当前只有 TUI + CLI，未来可扩展 Web/SDK。

### 4.4 安全与权限模式

#### 模式 15：三级防护权限引擎（Three-Tier Permission Engine）

**项目来源**：agent-core

**设计要点**：
- 第一级：ResourcePermission（文件系统）
- 第二级：AbilityPermission（工具调用）
- 第三级：ToolPermission（工具内细粒度）
- 默认拒绝策略 + 白名单优先

**适用场景**：细粒度权限控制

**对 laew 价值**：极高。laew 当前零校验，应直接借鉴此模式。

#### 模式 16：27 种 Hook 体系（27-Hook System）

**项目来源**：claudecode

**关键设计**：
- 覆盖 PreToolUse / PostToolUse / UserPromptSubmit / SessionStart / SessionEnd / PreCompact / PostCompact / ...
- 每个 Hook 独立脚本
- 用户可在配置中添加自定义 Hook

**适用场景**：灵活的扩展点

**对 laew 价值**：高。laew 缺少扩展点，借鉴可大幅提升可扩展性。

#### 模式 17：BubbleWrap 进程沙箱（Process Sandbox）

**项目来源**：agent-studio + agent-core

**设计要点**：
- 类似 Docker 的进程隔离
- 独立 PID / 文件系统 / 网络命名空间
- 资源限制（CPU/内存）

**适用场景**：执行不可信代码

**对 laew 价值**：极高。laew 当前零沙箱，应至少实现路径白名单 + 资源限制。

### 4.5 可观测性模式

#### 模式 18：W3C PROV-O 决策溯源（W3C PROV-O Decision Provenance）

**项目来源**：semantica

**关键设计**：
- agent / activity / entity 三元组
- 时序有效性窗口（valid_from / valid_until）
- family_id 版本家族追踪

**适用场景**：决策溯源、审计、回放

**对 laew 价值**：高。laew 当前无决策记录，未来调试/审计需要。

---

## 五、laew 综合借鉴路线图

基于 13 个项目的横向对比与新维度专题分析，本节给出 laew 的**四级借鉴路线图**（P0/P1/P2/P3）。

### 5.1 P0：核心必备（2-3 周内可落地）

#### 5.1.1 协议 IR 与 Unknown 兜底

**目标**：解决 laew 当前协议差异封闭不彻底的问题

**改造点**：
1. 在 `src/llm/` 下新增 `ir.rs`，定义 `LlmRequest` / `LlmResponse` / `ContentBlock` IR 结构
2. 实现 `ContentBlock::Unknown { provider, raw }` 兜底
3. 重构 `LlmClient` trait，所有客户端基于 IR 而非 Value
4. 引入 `PreservationMetadata` 翻译失败重放

**工作量**：1.5 周（含测试）

**价值评估**：
- **核心**：避免协议差异蔓延到 agent 层
- **核心**：遇到 Anthropic `thinking` / OpenAI `reasoning` 不丢失
- **增强**：未来扩展 Gemini / Mistral 时无需重构

#### 5.1.2 Context 简单 prune（第一级压缩）

**目标**：解决 laew 当前 0 级压缩必然溢出的问题

**改造点**：
1. 在 `agent/mod.rs` 的主循环中加入简单 prune
2. 保留最近 10 轮历史 + 所有 system 消息
3. 工具结果超过 4096 token 的截断

**工作量**：3 天

**价值评估**：
- **核心**：避免长任务溢出
- **基础**：为后续 LLM compaction 打基础

#### 5.1.3 Bash 黑名单 + 路径白名单

**目标**：解决 laew 当前零校验的问题

**改造点**：
1. 在 `src/agent/tools/bash.rs` 中加入命令黑名单（`rm -rf /` / `mkfs` / `dd` / ...）
2. 在 `src/agent/tools/read.rs` / `write.rs` 中加入路径白名单（仅允许工作目录 + 临时目录）
3. 实现 `PermissionEngine` 雏形（参考 agent-core 三级防护）

**工作量**：1 周

**价值评估**：
- **核心**：避免误操作
- **基础**：为后续 BubbleWrap 沙箱打基础

#### 5.1.4 决策日志（结构化日志）

**目标**：解决 laew 当前零观测的问题

**改造点**：
1. 在 `src/observability/` 新增模块
2. 使用 `tracing` crate 实现结构化日志
3. 在 ReAct 循环每个关键点插入 log：
   - 任务接收
   - 项目上下文注入
   - Yolo 分类
   - LLM 调用
   - 工具调用
   - Quality-Check 结果
   - SessionContext 摘要

**工作量**：3 天

**价值评估**：
- **核心**：定位问题的第一步
- **基础**：为后续 OTLP / PROV-O 打基础

### 5.2 P1：增强（1-2 月可落地）

#### 5.2.1 LLM Compaction（第二级压缩）

**目标**：从简单 prune 升级到 LLM 摘要压缩

**改造点**：
1. 在 `src/agent/` 下新增 `compact.rs` 模块
2. 实现 LLM 摘要压缩：旧历史 → 摘要
3. 压缩前注入 system 消息标记"以下是压缩摘要"
4. 实现 forked-agent 共享 prompt cache（参考 claudecode）

**工作量**：3 周

**价值评估**：
- **增强**：支持超长任务
- **增强**：减少重复 prompt cache 计算

#### 5.2.2 MemoryPipeline L0-L1（原始记录 + 敏感过滤）

**目标**：把 SessionContext 升级为 TencentDB 风格的 L0-L1 管线

**改造点**：
1. 在 `src/agent/memory/` 新增模块
2. L0 Recorder：原始消息记录到 `conversation_log` 表
3. L1 Sanitizer：敏感信息过滤（API Key / Token / 邮箱 / 手机号）
4. 实现污染消息替换算法（还原 `<<<LAEW:PROJECT_CONTEXT>>>` 标记的污染）

**工作量**：4 周

**价值评估**：
- **增强**：完整记忆系统
- **增强**：解决重启后漂移问题

#### 5.2.3 PermissionEngine 三级防护

**目标**：参考 agent-core 实现完整三级防护

**改造点**：
1. ResourcePermission：路径白名单
2. AbilityPermission：每个工具独立策略
3. ToolPermission：工具内细粒度控制
4. HITL 机制：运行时审批

**工作量**：4 周

**价值评估**：
- **增强**：细粒度权限控制
- **增强**：HITL 机制降低误操作

#### 5.2.4 性能指标（Prometheus 风格）

**目标**：补充 TTFT/TPOT 性能指标

**改造点**：
1. 在 `src/observability/metrics.rs` 新增指标收集
2. TTFT（Time To First Token）：从 LLM 调用到首个 chunk 的时间
3. TPOT（Time Per Output Token）：每个输出 token 的平均时间
4. 请求数、错误数、延迟分位数

**工作量**：2 周

**价值评估**：
- **增强**：性能评估基础
- **基础**：后续优化决策依据

### 5.3 P2：高级（3-6 月可落地）

#### 5.3.1 四级压缩管线

**目标**：参考 claudecode 实现完整 4 级压缩管线

**改造点**：
1. 第一级：Time-Based Microcompact（时间触发）
2. 第二级：Cached Microcompact（缓存触发）
3. 第三级：Auto-Compact（Token 触发，LLM 摘要）
4. 第四级：Session Memory 压缩

**工作量**：8 周

**价值评估**：
- **高级**：超长任务支持
- **高级**：Prompt Cache 效率优化

#### 5.3.2 MemoryPipeline L2-L3（语义提取 + 上下文注入）

**目标**：从 L0-L1 升级到完整 L0-L3

**改造点**：
1. L2 Distiller：语义提取（LLM 提炼关键信息）
2. L3 Injector：上下文注入（MemoryProxy + RRF/BM25/向量）
3. RRF 融合算法（参考 TencentDB）
4. BM25 + 向量混合检索

**工作量**：12 周

**价值评估**：
- **高级**：智能上下文注入
- **高级**：准确率提升

#### 5.3.3 BubbleWrap 沙箱

**目标**：解决 laew 当前零沙箱问题

**改造点**：
1. 进程隔离：使用 `landlock` / `bubblewrap` crate
2. 文件隔离：独立文件系统挂载
3. 网络隔离：独立网络命名空间
4. 资源限制：CPU / 内存 / 磁盘

**工作量**：12 周（含 Rust 生态调研）

**价值评估**：
- **高级**：安全执行
- **高级**：多用户支持基础

#### 5.3.4 Workflow DSL 引擎

**目标**：参考 agent-studio 实现工作流 DSL

**改造点**：
1. 定义工作流 DSL（YAML/TOML）
2. 解析 → 标准化 IR
3. 执行引擎（图执行 / DAG 执行）
4. Provider LRU 缓存

**工作量**：12 周

**价值评估**：
- **高级**：支持复杂工作流
- **高级**：可视化基础

#### 5.3.5 OTLP 链路追踪

**目标**：从决策日志升级到完整 OTLP

**改造点**：
1. 引入 `opentelemetry` crate
2. 每个 ReAct iteration 一个 span
3. 每个 tool_call 一个 span
4. 每个 LLM 调用一个 span
5. 流式 chunk 实时记录

**工作量**：6 周

**价值评估**：
- **高级**：分布式追踪
- **高级**：性能瓶颈定位

### 5.4 P3：远期（6+ 月可落地）

#### 5.4.1 Context Graph

**目标**：参考 semantica 实现 Context Graph

**改造点**：
1. ContextNode / ContextEdge 数据结构
2. 时序有效性窗口
3. family_id 版本家族
4. Rete 推理引擎

**工作量**：16 周

**价值评估**：
- **远期**：复杂上下文关系
- **远期**：决策溯源基础

#### 5.4.2 W3C PROV-O 溯源

**目标**：参考 semantica 实现决策溯源

**改造点**：
1. agent / activity / entity 三元组
2. 时序有效性窗口
3. family_id 版本家族追踪
4. 决策回放工具

**工作量**：12 周

**价值评估**：
- **远期**：完整审计
- **远期**：合规支持

#### 5.4.3 多前端共享核心

**目标**：参考 hermes-agent 实现多前端

**改造点**：
1. 适配层抽象
2. Web 前端
3. Bot 前端（Slack/Discord）
4. SDK 公开
5. API 服务化

**工作量**：24 周

**价值评估**：
- **远期**：产品化基础
- **远期**：生态扩展

#### 5.4.4 协议矩阵

**目标**：参考 jiuwenswarm 实现协议矩阵

**改造点**：
1. mode_matrix 解析
2. 跨协议边界 SwarmBuildContext 重建
3. HTTP / WS / gRPC / stdio 多协议支持

**工作量**：16 周

**价值评估**：
- **远期**：企业级集成
- **远期**：多场景适配

### 5.5 借鉴优先级矩阵

下表汇总借鉴优先级（横轴：实现难度，纵轴：价值评估）：

```
       价值高           价值中           价值低
    ┌─────────────┬─────────────┬─────────────┐
难度│ P0 P1      │ P2         │ P3         │
低  │ •协议IR    │ •Workflow  │ •多前端    │
    │ •Context   │  DSL       │ •协议矩阵  │
    │  prune     │ •OTLP      │            │
    │ •Bash黑名单│            │            │
    │ •决策日志  │            │            │
    ├─────────────┼─────────────┼─────────────┤
难度│ P1 P2      │ P2         │ P3         │
中  │ •LLM压缩   │ •BubbleWrap│ •Context   │
    │ •MemoryL0L1│ •性能指标  │  Graph     │
    │ •三级防护  │            │ •PROV-O    │
    ├─────────────┼─────────────┼─────────────┤
难度│ P2         │ P3         │ P3         │
高  │ •四级压缩  │ •协议矩阵  │ •W3C PROV-O│
    │ •MemoryL2L3│            │            │
    └─────────────┴─────────────┴─────────────┘
```

---

## 六、反模式警示

本节汇总 13 个项目中**应当避免的反模式**（共 12 个），每个反模式都给出：项目来源、问题描述、负面后果、laew 应如何避免。

### 6.1 反模式 1：硬编码协议差异

**项目来源**：早期 claudecode（已改进）

**问题描述**：协议差异直接散落在 agent 层，没有 IR 抽象

**负面后果**：
- 添加新协议需要修改 agent 层
- 协议特性无法跨协议利用
- 调试时无法定位协议层问题

**laew 应避免**：
- 当前 laew 的 `llm/anthropic.rs` / `llm/openai.rs` 直接转 `serde_json::Value`，没有 IR
- **P0 改进**：抽出 LlmRequest IR（参考 Switchyard）

### 6.2 反模式 2：无压缩必溢出

**项目来源**：早期 opencode（已改进为 2 级）

**问题描述**：Context 完全无压缩，长任务必然 token 超限

**负面后果**：
- 用户被迫手动 /clear
- 长任务直接失败
- 资源浪费（重复发送历史）

**laew 应避免**：
- laew 当前正是 0 级压缩（参考 `专题-横向对比深度分析合集.md` 第 102 行）
- **P0 改进**：实现简单 prune

### 6.3 反模式 3：零权限校验

**项目来源**：laew（当前状态）

**问题描述**：Bash / Read / Write 工具无任何权限校验

**负面后果**：
- 误操作风险（rm -rf /）
- 数据泄露风险
- 无法支持多用户

**laew 应避免**：
- 当前 laew 完全零校验
- **P0 改进**：Bash 黑名单 + 路径白名单
- **P1 改进**：三级防护 PermissionEngine

### 6.4 反模式 4：零观测调试难

**项目来源**：laew（当前状态）

**问题描述**：无日志、无指标、无追踪，问题难以复现

**负面后果**：
- Bug 难以定位
- 性能瓶颈不可见
- 无法支持用户反馈

**laew 应避免**：
- 当前 laew 零观测
- **P0 改进**：结构化日志
- **P1 改进**：Prometheus 指标
- **P2 改进**：OTLP 链路追踪

### 6.5 反模式 5：散落的上下文注入

**项目来源**：laew（当前状态）

**问题描述**：`<<<LAEW:PROJECT_CONTEXT>>>` / `<<<LAEW:SESSION_HISTORY>>>` 标记散落在 agent 循环各处

**负面后果**：
- 注入逻辑难以维护
- 污染消息难以还原
- 优先级难以控制

**laew 应避免**：
- 当前 laew 的标记散落
- **P1 改进**：引入 L3 Injector（参考 TencentDB）

### 6.6 反模式 6：单维度记忆

**项目来源**：laew（当前状态）

**问题描述**：只有 `agent_memory` 表存 Agent 记忆，缺少用户维度、Session 维度、原始维度

**负面后果**：
- 无法支持跨 Agent 记忆
- 无法支持用户偏好
- 无法回溯原始对话

**laew 应避免**：
- 当前 laew 记忆维度单一
- **P1 改进**：实现 L0-L1 管线

### 6.7 反模式 7：硬编码多 Agent 编排

**项目来源**：早期 jiuwenswarm（已改进为声明式 Spec）

**问题描述**：多 Agent 协作通过硬编码 if-else 编排

**负面后果**：
- 添加新角色需要修改核心代码
- 难以支持复杂协作拓扑
- 难以测试

**laew 应避免**：
- 当前 laew 的 MultiAgentOrchestrator 是过程式
- **P2 改进**：声明式 TeamAgent Spec

### 6.8 反模式 8：MCP 全盘接受或全盘拒绝

**项目来源**：claudecode（接受）vs pi（拒绝）

**问题描述**：
- claudecode 支持 7 种 MCP 传输 → 复杂度高
- pi 拒绝 MCP → 工具生态受限

**负面后果**：
- 接受 MCP：维护成本高、易出 Bug
- 拒绝 MCP：工具生态受限

**laew 应避免**：
- **P1 改进**：选择性接受 MCP（stdio + Streamable HTTP），拒绝过度复杂度

### 6.9 反模式 9：过度泛化导致性能损失

**项目来源**：openclaw（2 百万行）

**问题描述**：Gateway + Harness + 双向 MCP 三层抽象，代码规模爆炸

**负面后果**：
- 维护成本极高
- 性能损耗（多层抽象）
- 难以定位 Bug

**laew 应避免**：
- 当前 laew 抽象层次合理
- **警告**：不要为了抽象而抽象

### 6.10 反模式 10：直接转换协议而无保留

**项目来源**：laew（当前状态）

**问题描述**：Anthropic / OpenAI 直接转 `serde_json::Value`，无 IR、无保留

**负面后果**：
- 遇到协议新特性就 panic
- 无法回退到原文
- 调试困难

**laew 应避免**：
- **P0 改进**：抽出 IR + PreservationMetadata

### 6.11 反模式 11：缺乏版本化的扩展点

**项目来源**：laew（当前状态）

**问题描述**：没有 Hook 机制，扩展必须修改源码

**负面后果**：
- 第三方难以扩展
- 用户无法定制
- 升级困难

**laew 应避免**：
- **P1 改进**：参考 claudecode 实现若干关键 Hook（PreCompact / PostToolUse / ...）

### 6.12 反模式 12：单线程串行执行

**项目来源**：laew（当前状态）

**问题描述**：所有 LLM 调用、工具调用都是串行

**负面后果**：
- 资源利用率低
- 长任务慢

**laew 应避免**：
- **P2 改进**：支持并发工具调用（参考 opencode / agent-core）

---

## 七、结论与展望

### 7.1 架构成熟度排序

基于 18 维度对比矩阵，给出 13 个项目的**架构成熟度排序**（综合考虑：协议支持广度、多 Agent 协作、安全性、可观测性、记忆系统、社区活跃度）：

| 排名 | 项目 | 综合评分 | 主要优势 | 主要不足 |
|------|------|---------|---------|---------|
| 1 | **claudecode** | ★★★★★ | 4 级压缩、27 种 Hook、生态完整 | 单协议（Anthropic）、代码规模大 |
| 2 | **agent-core** | ★★★★★ | 三级防护、ReAct + TeamAgent、OTLP | 文档少、生态新 |
| 3 | **openclaw** | ★★★★☆ | 双向 MCP、Gateway + Harness | 2 百万行、性能损耗 |
| 4 | **hermes-agent** | ★★★★☆ | 6 前端共享、30+ provider、Skill 一等公民 | Python 性能、859 MB |
| 5 | **jiuwenswarm** | ★★★★☆ | Leader-Teammate、协议矩阵、SkillDev | 文档少、依赖 agent-core |
| 6 | **Switchyard** | ★★★★☆ | 6 协议 IR、PreservationMetadata、Prometheus | 无 Agent 循环 |
| 7 | **agent-studio** | ★★★★☆ | BubbleWrap 沙箱、工作流引擎、多微服务 | 架构重、不适合 CLI |
| 8 | **deepseek-harness** | ★★★☆☆ | Cordis Everything-is-a-Plugin | 单协议、抽象过度 |
| 9 | **atomcode** | ★★★☆☆ | L0/L1/L2 分层、Rust 性能 | 文档少、生态小 |
| 10 | **opencode** | ★★★☆☆ | Effect + Schema 全栈 DI | 代码规模适中但功能单薄 |
| 11 | **TencentDB-Agent-Memory** | ★★★☆☆ | L0-L3 管线、混合检索 | 单维度（记忆） |
| 12 | **semantica** | ★★★☆☆ | Context Graph + Rete + PROV-O | 不直接是 Agent |
| 13 | **pi** | ★★☆☆☆ | lane 并发、Skill 一等公民、哲学清晰 | 拒绝 MCP、生态小 |
| 14 | **laew** | ★★☆☆☆ | 6 角色 + 三档难度、Rust 性能、SQLite | 0 级压缩、零校验、零观测、零沙箱 |

**关键观察**：
- **laew 当前在第 14 位**（最低），但作为 Rust CLI 有独特价值
- **claudecode 和 agent-core 并列第一**，代表当前 Agent 工程最高水平
- **Python 项目整体评分高**（agent-core / hermes-agent / jiuwenswarm / agent-studio），生态成熟
- **Switchyard 是协议层的王者**，但不是完整 Agent

### 7.2 对 Agent 工程领域的启示

#### 7.2.1 协议层：IR 是必由之路

**启示**：13 个项目中，**所有支持多协议的项目都引入了 IR**（Switchyard / agent-studio / openclaw）。

**laew 行动**：
- **P0**：抽出 LlmRequest IR
- **P0**：实现 ContentBlock::Unknown 兜底

#### 7.2.2 上下文管理：多级压缩是趋势

**启示**：从 0 级（laew）→ 2 级（opencode）→ 3 级（atomcode）→ 4 级（claudecode / hermes-agent），**压缩级数与成熟度正相关**。

**laew 行动**：
- **P0**：实现简单 prune
- **P1**：实现 LLM compaction
- **P2**：实现 4 级管线

#### 7.2.3 记忆系统：分层管线是必然

**启示**：TencentDB-Agent-Memory 的 L0-L3 管线代表了记忆系统的**最佳实践**。

**laew 行动**：
- **P1**：实现 L0-L1
- **P2**：实现 L2-L3

#### 7.2.4 安全性：三级防护 + 沙箱是基础

**启示**：agent-core / agent-studio / jiuwenswarm 都实现了完善的权限管控。

**laew 行动**：
- **P0**：Bash 黑名单 + 路径白名单
- **P1**：三级防护 PermissionEngine
- **P2**：BubbleWrap 沙箱

#### 7.2.5 可观测性：PROV-O + OTLP 是未来

**启示**：semantica / agent-core / Switchyard / agent-studio 都实现了完善的决策追踪。

**laew 行动**：
- **P0**：结构化日志
- **P1**：Prometheus 指标
- **P2**：OTLP 链路追踪
- **P3**：W3C PROV-O 溯源

#### 7.2.6 多 Agent 协作：声明式 Spec 是方向

**启示**：jiuwenswarm 的 SwarmBuildContext + agent-core 的 TeamAgent 都采用声明式 Spec → Runtime Build。

**laew 行动**：
- **P1**：把 MultiAgentOrchestrator 改造为声明式
- **P2**：参考 TeamAgent 实现

#### 7.2.7 工具调用：谨慎选择 MCP

**启示**：13 个项目中 9 个支持 MCP，但 pi 明确拒绝。**MCP 不是银弹**，需要根据场景选择。

**laew 行动**：
- **P1**：选择性支持 MCP（stdio + Streamable HTTP）
- **不追求**：WS / SSE / HTTP 多种传输

### 7.3 laew 的独特价值

虽然 laew 在 18 维度上相对落后，但仍有独特价值：

1. **Rust 性能 + 二进制分发**：单文件 `laew` 二进制，无依赖
2. **极简 SQLite 持久化**：无配置文件、无外部数据库
3. **TUI 体验**：crossterm 原始模式 + alternate screen + Screen 栈
4. **完整可读源码**：~5k 行，1 个工程师可完全掌握

**未来定位**：
- **短期**：作为"轻量级 Rust Agent CLI"在 AtomCode / Switchyard 之下
- **中期**：通过 P0/P1 借鉴达到"中型 Rust Agent CLI"
- **远期**：通过 P2/P3 借鉴达到"完整 Rust Agent 平台"

### 7.4 关键风险与缓解

| 风险 | 概率 | 影响 | 缓解策略 |
|------|------|------|---------|
| **P0 借鉴拖延导致技术债务** | 高 | 中 | 立即启动 4 项 P0 改进 |
| **协议扩展时重构** | 中 | 高 | P0 完成 IR 后再扩展协议 |
| **用户数据泄露** | 低 | 高 | P0 完成权限校验 |
| **生态被 TypeScript 占据** | 中 | 中 | 保持 Rust 性能优势 |
| **文档跟不上** | 高 | 中 | 借鉴同时更新文档 |

### 7.5 总结

**laew 处于"种子期"**：
- 已完成核心架构（6 角色 + 三档难度 + 多协议）
- **关键短板**：0 级压缩、零校验、零观测、零沙箱
- **借鉴方向**：从 claudecode（4 级压缩）、agent-core（三级防护）、TencentDB（L0-L3 管线）、Switchyard（协议 IR）四个项目集中借鉴

**行动建议**：
1. **立即启动 P0**（2-3 周）：协议 IR + Context prune + Bash 黑名单 + 决策日志
2. **短期推进 P1**（1-2 月）：LLM 压缩 + Memory L0-L1 + 三级防护 + Prometheus
3. **中期推进 P2**（3-6 月）：4 级压缩 + Memory L2-L3 + BubbleWrap + OTLP
4. **远期推进 P3**（6+ 月）：Context Graph + PROV-O + 多前端 + 协议矩阵

**最终愿景**：把 laew 打造成 **"Rust 生态中最完整的轻量级 Agent CLI"**，对标 claudecode 在 TypeScript 生态中的地位。

---

## 附录：参考引用

### A.1 核心机制深度分析文档

本报告引用了以下核心机制深度分析文档：

| 文档 | 行数 | 引用章节 |
|------|------|---------|
| `Switchyard-核心机制深度分析.md` | 615 | 第三章 3.1（协议 IR）、3.2（TranslationEngine） |
| `agent-core-核心机制深度分析.md` | 826 | 第三章 3.3（三级防护）、第七章（OTLP Trajectory） |
| `jiuwenswarm-核心机制深度分析.md` | 994 | 第一章（Leader-Teammate）、第二章（协议矩阵） |
| `TencentDB-Agent-Memory-核心机制深度分析.md` | 743 | 第一章（L0-L3 管线）、第三章（MemoryProxy） |
| `semantica-核心机制深度分析.md` | 763 | 第一章（Context Graph）、第四章（W3C PROV-O） |
| `agent-studio-核心机制深度分析.md` | 877 | 第三章（BubbleWrap 沙箱）、第六章（工作流引擎） |
| `atomcode-核心机制深度分析.md` | 487 | 第一章（L0/L1/L2 分层） |
| `claudecode-核心机制深度分析.md` | 605 | 第一章（4 级压缩管线）、第二章（27 种 Hook） |
| `openclaw-核心机制深度分析.md` | 654 | 第二章（Gateway + Harness + 双向 MCP） |
| `opencode-核心机制深度分析.md` | 595 | 第二章（Effect + Schema） |
| `pi-核心机制深度分析.md` | 765 | 第二章（lane 并发） |
| `hermes-agent-核心机制深度分析.md` | 588 | 第二章（6 前端共享） |
| `deepseek-harness-核心机制深度分析.md` | 559 | 第二章（Cordis Everything-is-a-Plugin） |

### A.2 专题深度分析文档

本报告同时引用了 14 个专题深度分析文档：

| 文档 | 行数 | 引用章节 |
|------|------|---------|
| `专题-多轮对话与循环架构深度分析.md` | 451 | 第二章 2.2.1 |
| `专题-Context上下文管理深度分析.md` | 586 | 第二章 2.2.2 |
| `专题-记忆系统与管理深度分析.md` | 613 | 第二章 2.2.3 |
| `专题-工具调用深度分析.md` | 591 | 第二章 2.2.4 |
| `专题-MCP架构深度分析.md` | 644 | 第二章 2.2.5 |
| `专题-Skill系统深度分析.md` | 738 | 第二章 2.2.6 |
| `专题-SubAgent与多Agent架构深度分析.md` | 227 | 第二章 2.2.7 |
| `专题-Workflow设计深度分析.md` | 691 | 第二章 2.2.8 |
| `专题-Yolo目标意图识别与目标规划深度分析.md` | 603 | 第二章 2.2.9 |
| `专题-任务拆解与分类深度分析.md` | 615 | 第二章 2.2.10 |
| `专题-质检机制深度分析.md` | 789 | 第二章 2.2.11 |
| `专题-沙箱设计深度分析.md` | 1007 | 第二章 2.2.12 |
| `专题-权限管控深度分析.md` | 749 | 第二章 2.2.13 |
| `专题-Agent协作与调度深度分析.md` | 895 | 第二章 2.2.14 |
| `专题-横向对比深度分析合集.md` | 449 | 全文参考 |

### A.3 关键代码路径速查

| 项目 | 文件路径 | 行号 | 内容 |
|------|---------|------|------|
| Switchyard | `crates/protocol/src/llm.rs` | 303-334 | LlmRequest IR |
| Switchyard | `crates/protocol/src/llm.rs` | 124-131 | ContentBlock::Unknown |
| Switchyard | `crates/switchyard-translation/src/util.rs` | 226-249 | capture_request_preservation |
| Switchyard | `crates/protocol/src/stream.rs` | 339-348 | ResponseAccumulator |
| agent-core | `openjiuwen/core/single_agent/agents/react_agent.py` | 3251 | ReAct 主入口 |
| agent-core | `openjiuwen/core/single_agent/agents/react_agent.py` | 1482 | @rail 装饰器 |
| agent-core | `openjiuwen/core/security/permission_engine.py` | - | 三级防护 |
| TencentDB-Memory | `MemoryCore/src/core/conversation/l0-recorder.ts` | 93-115 | recordConversation 入口 |
| TencentDB-Memory | `MemoryCore/src/core/conversation/l0-recorder.ts` | 118-130 | 双保护机制 |
| TencentDB-Memory | `MemoryCore/src/core/conversation/l0-recorder.ts` | 213-253 | 污染消息替换 |
| jiuwenswarm | `agents/swarm/assembly.py` | 254-403 | enrich_team_spec_for_swarm |
| semantica | `semantica/context/context_graph.py` | 380-476 | ContextNode/ContextEdge |
| semantica | `semantica/context/context_graph.py` | 203-231 | _parse_iso_dt |
| agent-studio | `agent_studio/orchestrator/bubblewrap.py` | - | BubbleWrap 沙箱 |
| agent-studio | `agent_studio/workflow/engine.py` | - | 工作流引擎 |
| claudecode | `src/context/microCompact.ts` | 305 | cachedMicrocompactPath |
| claudecode | `src/context/microCompact.ts` | 446 | maybeTimeBasedMicrocompact |
| claudecode | `src/context/compact.ts` | 387 | compactConversation |
| atomcode | `crates/l0/src/overflow.rs` | - | L0 overflow ladder |
| atomcode | `crates/l1/src/summary.rs` | - | L1 summary |

### A.4 数据来源

本文所有数据均来自以下来源：
1. 各项目的 `源码调研.md` 文件（一手数据）
2. 各项目的 `深度分析.md` 文件（二手分析）
3. 各项目的 `核心机制深度分析.md` 文件（三手深度）
4. 14 个专题深度分析文档（横向对比）
5. 实际源码（验证）

**核验日期**：2026-09-05

**核验人**：Claude Code / Agent Engineer

---

## 文档信息

- **标题**：专题-12Agent全面对比深度分析.md
- **字数**：约 16,000 字
- **章节数**：7 主章 + 1 附录
- **对比矩阵**：18 维度 × 13 项目
- **设计模式**：18 个
- **反模式**：12 个
- **借鉴路线图**：P0/P1/P2/P3 四级 22 项
- **引用文档**：13 核心分析 + 14 专题分析 = 27 份
- **核验日期**：2026-09-05
- **位置**：`/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/docs/Agent源码调研/专题-12Agent全面对比深度分析.md`

**这是整个知识库工程的总结性文档，供 laew 后续借鉴时查阅。**