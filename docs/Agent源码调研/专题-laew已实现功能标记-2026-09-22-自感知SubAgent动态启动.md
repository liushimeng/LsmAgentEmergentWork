# laew 已实现功能标记 — 自感知 SubAgent 动态启动(2026-09-22 第 114 轮)

> **本轮**:第 114 轮(2026-09-22)
> **作者**:laew 自动化 Agent 任务
> **关联调研**:
> - `专题/专题-第六轮-SubAgent调度与并发模型深度对比.md`(§2.2 laew 四条缺口 / §11.3 后台 SubAgent / §11.4 嵌套 SubAgent / §16 对比表「无 task 工具」)
> - `专题/专题-第十一轮-Agent协作与多Agent通信协议深度对比.md`(§2.2 openclaw Subagent Registry / §5.1 claudecode AgentTool)
> - `专题/专题-SubAgent与多Agent架构深度分析.md`(§4 模式 1 中央编排 vs 模型自主 / §4.2 工具集限制)
> - `专题-第十九轮-A2A协议与多Agent互操作深度对比.md`(L1704 / L1705 / L1740 / L1746)
> - `docs/自动化测试-提示词文件列表/85-元编程与反射编程范式.md`(CC03 ToolRegistry 自省)
> **关联方案**:`tmpPlan/2026-09-22_04-自感知SubAgent动态启动方案.md`
> **设计文档**:`docs/自感知SubAgent动态启动/01-设计与解决方案.md`
> **状态**:✅ 已实现 + 单测/e2e 通过

---

## 一、本轮落地清单(需求:用户通过提示词自动启动 SubAgent 及相关 Agent)

| 能力 | laew 改造前 | 知识库对标 | 本轮增量 |
|------|------------|-----------|---------|
| **模型自主委派入口** | ❌ 无任务工具(第六轮对比表:「禁止(无 task 工具)」) | claudecode `Agent` / opencode `TaskTool` / atomcode `TaskTool` | 🆕 `SubAgent` 工具(单工具 + 5 action) |
| **子 Agent 类型** | ❌ 仅 1 种执行者 | opencode 6 内置 Agent / claudecode 4 种 `subagent_type` | 🆕 6 类型名册(general-purpose / explore / researcher / plan / code-reviewer / operator) |
| **Agent 自感知** | ❌ 提示词手写常量,与实际工具面可漂移 | CC03 ToolRegistry 自省 / codex 工具自描述 | 🆕 系统提示词段落**由注册表程序化生成** + `action="list"` 运行时快照 |
| **嵌套深度限制** | 🟡 靠工具集隐式禁止 | openclaw `MAX_SPAWN_DEPTH=1` / opencode `subagent_depth` + task deny | 🆕 默认 1 层 + **双重防御**(叶子不注册工具 + 运行时 2002) |
| **并发控制** | ❌ 动态启动无并发概念 | atomcode `Semaphore(3)` / openclaw Swarm `maxConcurrent` | 🆕 会话级 `Semaphore(3)` + batch `buffer_unordered` |
| **会话级预算** | ❌ 无 | openclaw `reserveSwarmRun` / deepseek `ChildLock` | 🆕 `LAEW_SUBAGENT_MAX_TOTAL`(默认 8)+ 全有或全无预扣 |
| **后台 SubAgent** | ❌ 无(必须同步等) | claudecode `run_in_background`(+SendMessage) | 🆕 `background=true` → `run_id` + `result` / `cancel` 反查 |
| **权限继承** | ❌ 无(不存在子 Agent) | opencode `deriveSubagentSessionPermission` | 🆕 `SpawnPolicy` 三档 + 工具**交集收窄** + 剔除项如实上报 |
| **取消传播到动态子 Agent** | — | atomcode CancellationToken / claudecode AbortController | 🆕 父 token → `child_token()` 逐子传递(复用既有 H9 链) |
| **用量归属** | — | — | 🆕 会话级 usage 台账 + 单元/任务两级 drain → `/cost` |
| **可观测** | — | openclaw Subagent Registry 事件 | 🆕 事件环形缓冲(64)+ `tracing` 日志 + TUI `/agents` |
| **持久化 resume** | ❌ | openclaw Restorer / pi Operation | ❌ 本轮不做(P2,与 L1706 同批) |
| **跨进程 SubAgent** | ❌ | deepseek 6 传输 / claudecode CCR | ❌ 本轮不做(P3,见第六轮 §11.6) |

---

## 二、实现细节

### 2.1 新增/修改文件

| 文件 | 类型 | 行数 | 说明 |
|------|------|------|------|
| `src/agent/self_awareness.rs` | 新建 | ~560 | 配置(6 环境变量)/ 6 类型名册 / 静态身份段渲染 / 策略过滤 |
| `src/agent/dynamic_subagent.rs` | 新建 | ~1100 | Governor(并发/预算/台账/作业表/事件环)+ task-local 运行时 + 子 Agent 组装/运行/回收 |
| `src/agent/tools/subagent.rs` | 新建 | ~420 | `SubAgentTool`(schema + 5 action 分发 + 统一 JSON 信封) |
| `src/agent/profile.rs` | 改 | +90 | `spawn_policy` 字段 + `dynamic_child()` + 自感知段注入(`build_self_aware_prompt`) |
| `src/agent/tools/mod.rs` | 改 | +60 | 4 注册表登记 `SubAgent` + `ToolRegistry::subset()` + 4 项注册面单测 |
| `src/agent/agent_loop.rs` | 改 | +30 | `run_session_inner` 拆 wrapper/body + `dynamic_subagent::scope` 作用域 |
| `src/agent/system_prompt/mod.rs` | 改 | +15 | 4 角色工具说明补 `SubAgent` + Yolo「保留多 Agent 要求」规则 |
| `src/agent/orchestrator/mod.rs` | 改 | +60 | 任务边界 `drain_usage` + `merge_dynamic_usage`(3 种结局) |
| `src/agent/subagent.rs` | 改 | +6 | 单元边界 `drain_usage` → `SubFlowOutcome.usage` |
| `src/llm/mod.rs` | 改 | +15 | `Usage::merge`(4 字段饱和加) |
| `src/tui/{slash,completion,format}.rs` | 改 | +75 | `/agents`(=`/subagents`)面板:名册 / 上限 / 最近作业表 |
| `testReport/run_e2e.sh` | 改 | +170 | §4n 16 项断言(含抓包级 wire 断言)+ §7 `/agents` 冒烟 4 项 |
| `docs/自感知SubAgent动态启动/01-设计与解决方案.md` | 新建 | ~330 | 设计与解决方案 |
| `tmpPlan/2026-09-22_04-自感知SubAgent动态启动方案.md` | 新建 | ~60 | 实施方案 |
| `AGENTS.md` | 改 | +20 | 架构 / 环境变量 / 命令表 / 文档地图 |

### 2.2 五个 action 的统一信封

| code | 语义 |
|------|------|
| 0 | 成功 |
| 1001 | 参数非法(缺 task / 未知 action / 未知类型 / batch 超 8) |
| 2000 | 子 Agent 执行失败(报告 `status=failed/timeout` + `error`) |
| 2001 | 会话级预算耗尽(消息含已用/上限) |
| 2002 | 深度超限(叶子 Agent 尝试再启动) |
| 2003 | 并发槽位等待 60s 超时 |
| 4001 | 当前上下文不支持(未开启 / 非委派角色;**要求模型不要重试**) |
| 4002 | 已取消 |
| 4003 | `run_id` 不存在 |

### 2.3 测试覆盖

- `self_awareness`:**12** 项(配置默认/边界 clamp/开关解析/类型宽松解析/策略名册过滤/提示词段注入与关闭条件/叶子提示词)
- `dynamic_subagent`:**13** 项(launch 端到端 + usage 台账 drain / 工具收窄 + 剔除上报 / 策略地板 / 深度 2002 / 预算 2001 不超扣 / batch 3 子并行 / batch 超限与超预算全有或全无 / 快照字段 / 后台作业轮询与幂等 cancel / 父取消即时中止 / **max_depth=2 子仍可委派** / 作用域同名字复用防 depth 二次 +1)
- `tools::subagent`:**6** 项(无运行时 4001 / 7 类参数错误 1001 / 未知 run_id 4003 / list 快照 / launch + launch→batch 自动升级 / 深度信封 / schema 与实现一致性)
- `agent::tests`:**2** 项(Agent 循环内作用域注入 + 子 Agent 真实执行 + usage 记账 / 叶子注册表与提示词双验证)
- `profile` self_awareness:**5** 项(策略与角色职责对齐 / 委派角色注入自感知 / 只读角色名册收窄 / 非委派角色零注入 / 动态子 Agent 叶子)
- 注册面:**4** 项(委派角色持工具 / 非委派角色不持 / schema 与 description 契约 / `ToolRegistry::subset`)
- 合计:lib 单测 **1484 passed**(基线 1455,新增 ~30 项,零回退)

### 2.4 e2e(testReport/run_e2e.sh §4n,16 项 + §7 冒烟 4 项)

- 工具面:SubAgent-Work 的 wire `tools[]` 含 `SubAgent`;
- 自感知:`action="list"` 的 tool_result 含 `can_spawn` / `roster` / `remaining`;
- 真实派发:`action="batch"` 报告 `count=2` / `ok=2`,并产生 **2 个子 Agent 的独立 LLM 请求**(抓包可见);
- 叶子语义:子 Agent 的 system 含自身身份与「不能再启动子 Agent」,**不含**可启动名册;
- 结果回填:合并文本 `### [general-purpose]` ×2 出现在父上下文 tool_result;
- 向后兼容:`LAEW_SELF_SPAWN=off` 时 `tools[]` 不含 `SubAgent`、提示词不含自感知段。

---

## 三、不破坏现有(向后兼容)

- `LAEW_SELF_SPAWN=off` / `LAEW_SUBAGENT_MAX_DEPTH=0`:**工具不注册 + 提示词不注入 + 运行时不建**,
  工具面、提示词、耗时与改造前一致(e2e §4n-14..16 断言);
- 非委派角色(Quality-Check / SessionContext / Debug / Compact)工具面与提示词**零变化**;
- 默认 `generic` 系统提示词(`default_tools_hint`)保持不变;
- 不引入新 crate;不改 SQLite schema;沙箱与权限面不扩大(子 Agent 工具 ⊆ 策略上限,写操作仍走 `sandbox_hook`);
- 提示词静态段只含会话内恒定内容(不含剩余额度等易变值)→ 不破坏 prompt cache 前缀。

---

## 四、已知边界(坦白记录)

| 边界 | 说明 | 后续 |
|------|------|------|
| Yolo 的 forced tool_choice | Yolo 声明了 `emit_tool`,`forced_tool_choice` 生效时模型必须直接返回分类,当轮**无法**调用 `SubAgent` 侦察;仅在 `LAEW_FORCED_TOOLS=off` 或 Provider 拒绝 forced 自动降级 auto 时可用 | 若需「Yolo 先侦察再分类」,应改为分轮预算(P2) |
| 动态子 Agent 不写记忆 | 不入 `agent_memory` / `session_memory`(避免污染既有记忆检索语义) | P2:按 `subagent_run` 表落库 |
| 后台作业无跨进程恢复 | 进程退出即丢(内存作业表) | P2:与 L1706 同批 |
| 嵌套 ≥2 层时并发槽位可能互等 | 已用「60s 超时 + 2003」防死锁,但极端配置下会拒绝而非排队成功 | 需要时改成按深度分层信号量 |

---

## 五、下一轮候选(P2/P3,知识库同族未做项)

- **L1706 / L1739 / L1740**:SubAgent 持久化 + resume + 孤儿恢复(openclaw Registry 100+ 文件);
- **L1746 / 第六轮 §11.3**:动态子 Agent 结果跨任务复用(swarm active/queued + Workboard);
- **第六轮 §11.6 / L1701-L1705**:跨进程委派(ACP / A2A transport);
- **CC06/CC10**:工具/Agent 装饰器与生命周期钩子(用户自定义 Agent 类型而不改核心代码)。
