# laew 已实现功能标记 — 自感知 SubAgent 自定义类型与运行持久化(2026-09-22 第 115 轮)

> **本轮**:第 115 轮(2026-09-22)
> **作者**:laew 自动化 Agent 任务
> **关联调研**:
> - `专题/专题-第十八轮-claudecode-深度分析.md` §2.2.4(项目级 `.claude/agents/foo.md` agent-as-command)
> - `专题/专题-第三轮-插件生态与扩展分发深度分析.md` §6.5(插件目录 `agents/` 一等公民)
> - `专题/专题-第六轮-SubAgent调度与并发模型深度对比.md` §2.2 / §4.2 / §11.3
> - `专题/专题-第十九轮-A2A协议与多Agent互操作深度对比.md` L1704 / L1706 / L1740 / L1745
> - `专题/专题-laew实现进度对照表.md`(D114 行两项 🟡)
> **关联方案**:`tmpPlan/2026-09-22_05-自感知SubAgent自定义类型与运行持久化方案.md`
> **设计文档**:`docs/自感知SubAgent自定义类型与运行持久化/01-设计与解决方案.md`
> **状态**:✅ 已实现 + 单测 / e2e 通过

---

## 一、本轮落地清单(需求:用户提示词驱动的自定义子 Agent + 可回看可续跑)

| 能力 | laew 改造前(D114) | 知识库对标 | 本轮增量 |
|------|------------------|-----------|---------|
| **子 Agent 类型定义** | ❌ 6 类枚举硬编码,新增类型要改 Rust 重编译 | claudecode `.claude/agents/*.md` / 插件 `agents/` | 🆕 `.laew/agents/*.md` 两级发现(项目级 + 用户级,用户级优先) |
| **类型字段** | — | claudecode frontmatter 13 字段 | 🆕 `label / description / extends / tools / readonly / name` 6 字段 + Markdown 正文=专属指令 |
| **类型解析** | `SubAgentType` 枚举 | opencode 内置 Agent 列表 | 🆕 `ResolvedAgentType{Builtin,Custom}` 统一抽象(组装路径零分支) |
| **工具面约束** | 策略上限 ∩ 注册表 | `deriveSubagentSessionPermission` | 🆕 三重收窄(+ `tools` 白名单 ∩ `extends` 默认)+ `dropped_tools` 如实上报 |
| **`agent_type` 校验** | JSON Schema `enum`(自定义 id **必然**被硬拒) | — | 🆕 改自由字符串,校验前移到实现层(`1001` + 可用名册 + 自定义指引) |
| **自感知同步** | 提示词段 / `list` 快照 / `/agents` 三处 | CC03 ToolRegistry 自省 | 🆕 三处同步含自定义类型(`(自定义)` 标注 + `custom_roster` + `custom_skipped` 原因) |
| **运行记录** | ❌ 仅内存作业表,进程退出即丢 | openclaw Subagent Registry(L1704) | 🆕 SQLite `subagent_run` 表(insert running → update finish,fail-open) |
| **跨任务查询** | ❌ | openclaw Workboard(L1745) | 🆕 `action="history"`(session / type / limit)+ TUI `/agents history [N\|all]` |
| **结论复用/续跑** | ❌ | claudecode `SendMessage` / opencode `task_id` | 🆕 `action="resume"`(前序任务+结论作种子上下文,记 `resumed_from` 血缘) |
| **孤儿恢复** | 🟡 仅父取消级联 | openclaw Restorer + Orphan Recovery(L1706/L1740) | 🆕 启动期把非当前会话的 `running` 改判 `orphaned`;结论仍可 `resume` |
| **容量治理** | — | openclaw session capacity / LRU | 🆕 `LAEW_SUBAGENT_RUN_KEEP`(默认 500)启动期 trim |
| **可关闭性** | `LAEW_SELF_SPAWN` | — | 🆕 `LAEW_SUBAGENT_PERSIST=off`:零 DB 读写,工具面/提示词不变 |
| **真·进程级恢复** | ❌ | openclaw suspended-delivery | ❌ 本轮不做(P3,需子 Agent 状态机) |

---

## 二、实现细节

### 2.1 新增 / 修改文件

| 文件 | 类型 | 说明 |
|------|------|------|
| `src/frontmatter.rs` | 新建 | 共享 frontmatter 解析器(agent + tui 复用;D2 既有测试成为回归网) |
| `src/agent/custom_agents.rs` | 新建 | 两级发现 / frontmatter→`AgentDef` / `Discovery`(含被忽略项)/ `ResolvedAgentType` / `resolve_in` / 名册可见性 |
| `src/config/subagent_run.rs` | 新建 | `subagent_run` DAO(insert/finish/list/get/count/count_orphans/mark_orphans/trim + 6 项单测) |
| `src/database/schema.rs` | 改 | `subagent_run` 表 + 2 个索引(`CREATE TABLE IF NOT EXISTS`,零迁移) |
| `src/agent/self_awareness.rs` | 改 | `roster_with/roster_view`(含 `SkippedEntry`)/ `prompt_section` 自定义段 / `common_rules_section` 抽取 / 2 个新环境变量 |
| `src/agent/dynamic_subagent.rs` | 改 | `ResolvedAgentType` 组装 / `persist_start|finish` / `PriorRun` 种子上下文 / `launch_with_origin` / `history` 查询 / 孤儿+trim 维护 / `run_id` 全局唯一修复 / `origin`・`resumed_from` 字段 |
| `src/agent/tools/subagent.rs` | 改 | 7 action(schema/enum/描述)/ `history` / `resume` / 自定义类型解析(按运行时 `work_dir`)/ `run_row_json` |
| `src/agent/profile.rs` / `src/agent/mod.rs` / `src/config/mod.rs` / `src/lib.rs` | 改 | 模块与提示词接线 |
| `src/tui/{slash,completion,format,mod}.rs` | 改 | `/agents` 自定义类型段 + `/agents history`;bootstrap 维护;帮助与补全 |
| `src/main.rs` | 改 | `-p`/`-f` 单轮模式同样做启动期维护(孤儿标记 + trim) |
| `scripts/mock_llm_server.py` | 改 | `$LAST_RUN_ID` 占位符替换 + 父/子实例计数分桶 + 子 Agent 直答(见 §2.4) |
| `testReport/run_e2e.sh` | 改 | 新增 §4o(21 项断言) |
| `docs/自感知SubAgent自定义类型与运行持久化/01-设计与解决方案.md` | 新建 | 设计与解决方案 |
| `tmpPlan/2026-09-22_05-自感知SubAgent自定义类型与运行持久化方案.md` | 新建 | 实施方案 |

### 2.2 定义文件规格

```markdown
---
label: 前端审查                      # 名册标签(缺省「自定义」)
description: 前端专项审查:…           # 名册描述(缺省取正文首行截 60)
extends: code-reviewer               # 继承内置骨架(缺省 general-purpose;readonly 时缺省 explore)
tools: Read, Glob, Grep              # 白名单(缺省取 extends 默认;与策略上限取交集)
readonly: true                       # 只读便捷声明(tools 收窄为 Read/Glob/Grep 且叠加只读边界)
---
逐条给出「文件:行 -> 问题 -> 建议」,按 P0/P1/P2 分级;不修改任何文件。   # 正文 = 专属职责提示词
```

### 2.3 工具契约(7 action)

| action | 新增? | 说明 |
|--------|------|------|
| `list` | — | 快照新增 `roster[].custom/source`、`custom_skipped[]`、`limits.persist` |
| `history` | 🆕 | 查已落库记录(`limit` ≤50 / `agent_type` / `all_sessions`);持久化关闭时 `code=0 + persist:false`(不报错,防重试) |
| `launch` | — | 报告新增 `origin` / `resumed_from` |
| `batch` | — | 同上(逐子报告) |
| `resume` | 🆕 | 取前序行 → 解析类型(失效给 1001 纠偏)→ 同一治理路径 → 种子上下文 → 记血缘 |
| `result` / `cancel` | — | 语义不变 |

### 2.4 两个隐藏坑(实现中实测发现并修复)

| 坑 | 现象 | 修复 |
|----|------|------|
| **run_id 跨会话重号** | `/new` `/clear` 产生新 Session(=新 `Governor`,seq 从 1 重来)→ 同进程两个会话生成**相同 run_id**;内存作业表看不出来,落库后主键冲突互相覆盖 | `next_run_id` 引入**进程级全局序号**:`sa-{pid:x}{全局序号:04x}-{会话内序号:x}` |
| **mock 路由计数被派生 Agent 打乱** | 动态子 Agent 与发起它的 SubAgent-Work 同属 mock 的 `role="subagent"`,子 Agent 新实例会把「实例内计数」归零 → 父 Agent 多步链路索引错位 | mock 端父/子**分桶计数**;并让子 Agent 在 router 模式直接给终答(其工具面可能是无 Bash 的只读自定义类型) |

> 第二条是**测试基建**问题,但第一条是产品缺陷 —— 由 e2e/单测并行落库暴露出来,属于本轮意外收获。

### 2.5 测试覆盖

- `frontmatter`:**8** 项(标准块 / 引号 / 无 frontmatter / 未闭合回退 / 未知 key / 重复 key / 布尔 / 描述兜底)
- `custom_agents`:**17** 项(两级覆盖 / 非法名 / 内置遮蔽 / extends 回退 / readonly 交集 / 分隔符 / 单行值边界 / pascal / 上限 / 提示词正文与叶子语义 / 策略可见性)
- `self_awareness`:**6** 项(空定义= D114 语义 / 自定义入名册带 source / 越权类型被 skipped 且给原因 / 提示词含自定义但不含路径 / 关闭态零注入 / 公共规则段)
- `config::subagent_run`:**6** 项(insert→finish / 过滤 / 幂等 / 缺行 no-op / 孤儿标记不误伤当前会话 / trim 保留最新)
- `dynamic_subagent`:**6** 项(自定义类型端到端 / 工具面被父策略收窄并上报 / 落库 running→终态 / `PERSIST=off` 不落库 / resume 种子+血缘 / 工具层 history→resume 闭环 / 快照 custom_skipped)
- `tools::subagent`:**2** 项扩充(1001 消息含自定义出路 + resume/history 校验;自定义类型经 `work_dir` 解析并派发)
- 合计:lib 单测 **1529 passed**(基线 1484,新增 45,零回退)

### 2.6 e2e(`run_e2e.sh` §4o,21 项)

在临时工作目录(定义文件即生效,不污染仓库)验证:
`list` 名册含自定义类型与来源路径 → `launch` 真实派发(`agent_type=fe-reviewer`) →
子 Agent system 含定义正文与「自定义定义文件」身份 + 叶子语义 + 工具面仅 Read/Glob/Grep →
`history` 读到落库记录(persist=true)→ `resume` 用 `$LAST_RUN_ID` 续跑并带
`resumed_from` 血缘 + 子 Agent user prompt 含「前序任务/前序结论」→
`PERSIST=off` 时 DB 行数零新增。

---

## 三、不破坏现有(向后兼容)

- `LAEW_SELF_SPAWN=off` / `LAEW_SUBAGENT_MAX_DEPTH=0`:工具不注册、提示词不注入、运行时不建、
  **DB 零写入**(持久化随运行时而消失);
- 工作目录无 `.laew/agents/`:名册 = 6 内置(与 D114 完全一致);
- `LAEW_SUBAGENT_PERSIST=off`:不写不读 `subagent_run`,工具面与提示词与开启态一致;
- DB 打不开 / 写入失败:fail-open(`warn!` 一次,任务链路不受影响);
- 非委派角色(QC / SessionContext / Debug / Compact):工具面、提示词、DB 三者零变化;
- 老库:增量表由 `CREATE TABLE IF NOT EXISTS` 自动补齐,无迁移脚本;
- 无新增 crate;写操作仍走既有沙箱;自定义类型工具面 ⊆ 父策略上限(不扩大权限面)。

---

## 四、已知边界(坦白记录)

| 边界 | 说明 | 后续 |
|------|------|------|
| `resume` ≠ 进程恢复 | 不恢复被冻结的 future(工具执行中途状态不可序列化),它是「带前序上下文重新起一个子 Agent」并记血缘 | 真恢复需子 Agent 状态机(L1739,P3) |
| 定义文件无显式热重载命令 | 文件改动后**下一次**构造提示词/名册即生效(不缓存),但运行中的子 Agent 不受影响 | 已够用;如需显式刷新可加 `/agents reload`(P2) |
| 自定义类型无独立模型覆盖 | claudecode 支持 frontmatter `model` 字段;laew 全仓单 Provider 单模型 | 与 Provider 多模型方案同批(P2) |
| `history` 无全文检索 | 只按 session / type / 时间倒序 | 需要时上 FTS5(P3) |
| 跨进程并发同名会话 | 孤儿标记按 `session_id != 当前` 保守判断,极端下会漏标 | 引入实例心跳(P3) |
| Workboard 仅「记录+血缘」 | 无共享看板/任务认领/跨进程 swarm 队列 | 见 L1745 / 第六轮 §11.6 |

---

## 五、下一轮候选(P2/P3)

- **L1739 / L1740 完整版**:子 Agent 状态机 + 暂停/恢复 + 启动期自动重跑孤儿作业(而非仅标记);
- **L1745 Workboard**:多 Agent 共享工作板(任务认领 / 依赖图 / 冲突检测);
- **CC06/CC10**:工具与 Agent 生命周期钩子(用定义文件挂 `on_start/on_tool/on_finish`);
- **L1701-L1712**:跨进程委派(A2A / ACP),让 laew 的子 Agent 可跨进程/跨机器;
- **定义文件 `model` 字段**:配合 Provider 多模型,让「便宜模型做侦察、强模型做实现」可配置。
