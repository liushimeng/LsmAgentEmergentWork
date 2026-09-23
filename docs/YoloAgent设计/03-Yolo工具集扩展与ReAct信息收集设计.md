# Yolo 工具集扩展与 ReAct 信息收集设计

> 关联需求:Yolo 入口层 / 任务三档分类 / 意图·目标识别 / 信息收集自主决策。
> 关联模块:`src/agent/yolo.rs`、`src/agent/profile.rs`、`src/agent/system_prompt/mod.rs`、
> `src/agent/tools/mod.rs`、`src/agent/agent_loop.rs`、`src/agent/self_awareness.rs`。
> 关联测试:`src/agent/tests.rs` / `testReport/run_e2e.sh` 4j。
> 工作清单:`tmpPlan/2026-09-22_01-Yolo工具扩展与ReAct模式改造方案.md`。

## 1. 背景与目标

### 1.1 动机

Yolo 是用户输入的第一站,负责「目的→目标→意图」三步分析、难度三档分类与失败回流。
其分类质量取决于**信息充分度**:用户输入经常含指代不明(「这个项目」「刚才那个文件」
「相关代码」)、提到具体文件/命令/网页、或需要事实依据才能判断难度——此时仅凭原始
prompt 难以稳定给出正确分类。

Yolo 此前**仅持有 Read 工具**,且 `forced tool_choice` 每轮强制指名
`submit_task_classification` —— 在结构上禁止多轮探索(模型第一轮必须交分类,
Yolo 的 Read 工具实际形同虚设)。

本轮目标:

1. 让 Yolo 在分类前**自主完成信息收集**,决定收集策略与停止时机。
2. 通过 **ReAct**(Thought→Action→Observation)循环实现多轮闭环探索。
4. 保持结构化分类输出的协议级保证(emit 工具短路 + 解析链零依赖)。

### 1.2 设计目标

| 目标 | 说明 |
|------|------|
| 信息自主收集 | Yolo 持 Read/Glob/Grep/Bash/MCP_Web_Use,自主决定何时调用、如何组合 |
| ReAct 多轮探索 | 探索轮不强制 emit,允许 Thought→Action→Observation 闭环 |
| 结构化输出保底 | 迭代预算最后一轮强制 emit;解析链与降级路径零改动 |
| 入口层边界 | Yolo 仍不持 Write/Edit/TodoWrite;Bash 仅用于只读侦察(MCP_Web_Use 仅观察类 action);SubAgent 仍只读子 Agent |
| 兼容回退 | `LAEW_FORCED_TOOLS=off` / QC / 旧 e2e 断言对应更新 |

## 2. 现状诊断(代码级)

| # | 现状 | 位置 |
|---|------|------|
| D1 | Yolo 工具面仅 Read + emit + SubAgent | `tools/mod.rs::yolo_registry` |
| D2 | `forced tool_choice` **每轮强制**指名 emit | `agent_loop.rs` 循环前 `meta.forced_tool = emit_tool` |
| D3 | `max_iterations = 4`(YoloRunner) | `yolo.rs::YoloRunner::new` |
| D4 | `with_max_iterations` 设 `explore_budget = n/4` | `agent/mod.rs::with_max_iterations` |
| D5 | 提示词「不得调用 Bash / Write」 | `system_prompt/mod.rs::YOLO_BASE_PROMPT` |
| D6 | 测试断言旧工具面 / 强制 channel | `tools/mod.rs` / `profile.rs` / `tests.rs` / `run_e2e.sh` 4j |

### 2.1 核心矛盾(本轮关键)

`agent_loop.rs` 现状(伪代码):

```rust
// 循环外,只设一次
if forced_tools_enabled() {
    meta.forced_tool = self.profile.emit_tool.clone();   // = "submit_task_classification"
}
for iter in 0..max_iterations { ... }
```

Anthropic `tool_choice: {"type":"tool","name":"submit_task_classification"}` 是
**强制指名调用**:模型在每一次响应中**必须**调用该指定工具,不允许调用任何其它工具
(也不允许不调用)。这与 ReAct 多轮探索在协议层互斥——模型**没有机会**先调用 Bash
再 Read 再 submit。

旧 e2e 4j-1 因此断言「Yolo 请求恰好 1 次」+ `tool_choice.type=tool`,
这是**现状**(强制直答),**不是**期望的 ReAct 行为。本轮将该断言改为 ReAct 语义。

## 3. 方案设计

### 3.1 工具面扩展(`yolo_registry`)

新增 `Bash`、`Glob`、`Grep`、`MCP_Web_Use`。新工具面:

```
Read, Glob, Grep, Bash, MCP_Web_Use, submit_task_classification, SubAgent
```

保持顺序(注册顺序决定 tools 列表顺序与 wire 工具数组):

| # | 工具 | 用途 |
|---|------|------|
| 1 | Read | 读取文本文件;带行号;offset/limit 分页 |
| 2 | Glob | 按通配符模式找文件路径(如 `src/**/*.rs`) |
| 3 | Grep | 按正则在文件内容中检索,定位符号/关键字 |
| 4 | Bash | **仅用于只读侦察**:`ls/cat/grep/git log/git status/cargo test --dry-run` 等 |
| 5 | MCP_Web_Use | **仅用于意图判断所需的网页信息**:`open/list/inspect/screenshot`(观察类 action) |
| 6 | submit_task_classification | 结构化输出通道,唯一出口 |
| 7 | SubAgent | 第 114 轮自感知只读并行侦察(`ReadOnlyChildren` 策略) |

**仍不持** `Write/Edit/TodoWrite`(不落盘、不管理任务清单)。
MCP_Window_Use 不在 Yolo 工具面(桌面窗口操控归 SubAgent-Work 专用)。

工具面与既有 `permissions::check_bash_command`(危险命令拦截,`rm -rf/sudo/dd/curl|bash`
等黑名单)对齐:即便用户/恶意 prompt 试图越过提示词约束,Bash 工具层仍有 fail-closed
防御。

### 3.2 ReAct 循环机制

Yolo 的 agent loop 复用既有 `Agent::run_session_body` 协议无关循环
(`complete → tool_calls → execute → tool_result → loop`)——这本身已是 ReAct 闭环。
本轮**补三件事**:

1. **探索轮不强制 emit**(见 §3.3),允许模型自由发起工具调用。
2. **提示词规定 Thought→Action→Observation 节奏**(见 §3.5)。
3. **最后一轮强制 emit + 倒数第二轮发收口预告**,保证结构化出口与平滑收敛。

`async` 路径不变;`with_max_iterations(n)` 同步上调 `explore_budget`(见 §3.4)。

### 3.3 延迟强制(`AgentProfile.defer_emit_force: bool`)

新增字段:

```rust
/// 结构化输出强制通道(L6/L19 + 2026-09-22 ReAct 延迟强制)。
///
/// false(现状,QC 等):每轮请求都注入 forced tool_choice —— 模型必须
///        以 emit 工具返回结构化结果(单轮强制)。
/// true(Yolo ReAct):探索轮(非最后一轮)不强制(tool_choice auto),
///        模型可自由 ReAct;仅最后一轮强制 emit 收口。
///        强制通道与多轮探索互斥,必须错峰。
pub defer_emit_force: bool,
```

| Profile | defer_emit_force |
|---------|------------------|
| Yolo | **true** |
| Plan / Main-Work / SubAgent-Work / WorkFlow / Quality-Check / SessionContext / Debug / Compact / 动态子 Agent | false |

`agent_loop.rs` 改为**每轮决策** `meta.forced_tool`:

```rust
meta.forced_tool = match self.profile.emit_tool.as_deref() {
    Some(emit) if forced_tools_enabled() => {
        if self.profile.defer_emit_force && iter + 1 < self.max_iterations {
            None                                              // 探索轮:auto
        } else {
            Some(emit.to_string())                            // 末轮或非 ReAct:强制 emit
        }
    }
    _ => None,                                                // 无 emit / forced_tools=off
};
```

**收口预告**(倒数第二轮,`iter + 2 == max_iterations`,且 Yolo defer 模式):
追加 `【收口提示】迭代预算即将耗尽(下一轮为最终轮,系统将强制要求提交 submit_task_classification)。
请立即停止新的探索,基于已收集的信息直接调用 submit_task_classification 提交分类结果。`
到 system 末尾(沿用 `runtime_hints` 的「system 后缀」路径,不破坏 cache_control 缓存前缀),
给模型**主动交卷**的机会,避免被硬强制时缺字段。

### 3.4 迭代预算

`YoloRunner::new` 改造:

```rust
let yolo = Agent::new(llm, AgentProfile::yolo_profile())
    .with_max_iterations(8)    // 4 → 8:ReAct 需要 glob→grep→read→submit 多轮
    .with_explore_budget(0);   // 第 118 轮 explore hint 文案面向浏览器操控
                               // (「禁止再开新 inspect/screenshot」),对 Yolo 语义误导,显式归零
```

**为什么 `with_explore_budget(0)`**:`Agent::with_max_iterations(n)` 会同步把 `explore_budget`
设为 `n/4`(第 118 轮惯例)。Yolo 取 `n=8` 则默认 `explore_budget=2`,在 `iter==2` 时
`build_runtime_hints` 会注入「已进入执行期...禁止再开新 inspect/screenshot/eval_js 探查」——
对 Yolo 的 `MCP_Web_Use(inspect)` 信息收集**直接矛盾**,必须关闭。

`explore_budget=0` 不影响 Yolo 的收口节奏——Yolo 的收口由 `defer_emit_force` 与
`max_iterations` 协同保障(末轮强制 emit)。

成本权衡:trivial 问题(「你好」)Yolo 通常 1 轮就自愿提交 emit(提示词规定
「信息充分时立即收口」),延迟影响可忽略;信息收集场景最长 8 轮 LLM 调用,边界明确。

### 3.5 系统提示词 ReAct 化

`system_prompt/mod.rs` 中 `YOLO_BASE_PROMPT` 与 `yolo_tools_hint` 重写:

**新增「信息收集(ReAct 模式)」节**(嵌于「分级标准」之前):

```text
## 信息收集(ReAct 模式)

分类质量取决于信息充分度。当用户输入存在指代不明(「这个项目」「刚才那个文件」
「相关代码」)、提及具体文件/命令/网页,或需要事实依据才能判断难度时,**不要
凭空猜测**,按 ReAct 循环自主收集:

- Thought(推理):我还缺什么信息?下一步查什么最省?
- Action(行动):调用一个工具(Glob/Grep 找文件与符号、Read 读内容、Bash 跑
  只读侦察命令、MCP_Web_Use 查网页信息、SubAgent 并行只读侦察)。
- Observation(观察):阅读工具结果,修正对任务的理解,决定继续收集还是收口。

约束:
- 每轮先输出 1~3 句 Thought 再发起工具调用,禁止无推理的盲调。
- 信息已足够完成三步分析与分级时,**立即**调用 submit_task_classification
  收口,不再继续探索。
- 整个收集阶段建议 ≤ 4 次工具调用;迭代预算耗尽前系统会强制收口,届时请
  基于已有信息提交。
- Bash 仅用于信息收集(ls/cat/grep/git log/git status/cargo test --dry-run 等
  只读侦察),禁止执行修改系统状态的命令(写文件/删文件/装依赖/改配置)——执行
  层会做这些。
- MCP_Web_Use 仅用于意图判断所需的网页信息(open/list/inspect/screenshot
  观察类 action),不要在分类阶段执行网页写操作(提交表单/发消息/下载)。
```

**删除**原「只持 Read 工具...不得调用 Bash / Write 等会修改系统状态的工具」句,
替换为:「**持有一组信息收集型工具**(Read/Glob/Grep/Bash/MCP_Web_Use/SubAgent)
用于在分类前侦察现状;不持有 Write/Edit 等文件写入工具,也不应执行修改系统状态
的操作——执行一律交给下游执行层。」

**`yolo_tools_hint`** 改为全工具清单 + ReAct 规范;双协议 tail 同步改写。

### 3.6 安全边界(纵深防御)

| 层 | 约束 |
|----|------|
| 工具注册面 | Yolo 不持 Write/Edit/MCP_Window_Use;TodoWrite 不在 Yolo |
| 提示词约束 | Bash 只读侦察;MCP_Web_Use 仅观察类 action;每轮先 Thought |
| 系统层 | `permissions::check_bash_command` 黑名单(`rm -rf/sudo/dd/curl|bash/~/.ssh/~/.aws/.env.production` 等) |
| SubAgent 子策略 | `SpawnPolicy::ReadOnlyChildren` —— 子 Agent 仍是只读侦察 |
| 收口保底 | 最后一轮强制 emit —— 解析链拿到结构化结果,降级路径兜底 |

即便提示词被绕过,Bash 工具层 fail-closed 黑名单仍拦截危险命令;
即便工具层黑名单被绕过,Yolo 不持 Write/Edit 写盘工具,仍无法写入文件
(只能通过 Bash 间接写 —— 被黑名单拦截)。

## 4. 改动清单(文件级)

| 文件 | 改动 |
|------|------|
| `src/agent/tools/mod.rs` | `yolo_registry` 加 Bash/Glob/Grep/MCP_Web_Use;`yolo_registry_names_only_read_and_emit` 改精确列表;`yolo_and_main_work_registries_exclude_mcp_web_use` 改为只 main_work/plan/quality 排除 |
| `src/agent/profile.rs` | `AgentProfile` 新增 `defer_emit_force` 字段,全部构造器补位;`yolo_profile` 设为 true;重写 `yolo_profile_only_has_read` 测试 |
| `src/agent/agent_loop.rs` | 循环外预置 forced_tool 块迁移到每轮决策;末尾收口预告 hint;`meta.forced_tool` 同步与 debug! 日志 |
| `src/agent/yolo.rs` | `YoloRunner::new/with_max_iterations` 用 `with_max_iterations(8).with_explore_budget(0)`;模块文档更新 |
| `src/agent/system_prompt/mod.rs` | `YOLO_BASE_PROMPT` / `yolo_tools_hint` / 双协议 tail 全部 ReAct 化 |
| `src/agent/tests.rs` | `emit_tool_short_circuits_loop` 断言改为探索轮不强制;新增 `yolo_react_emits_force_only_on_final_round` |
| `src/agent/self_awareness.rs` | `ReadOnlyChildren` 注释更新(Yolo 工具面 ≠ Read) |
| `testReport/run_e2e.sh` | 4j-1 断言改 ReAct 语义(探索轮 tool_choice 不指名 + 全工具面在列);4j 节注释;1271 行 Work/Bash 注释 |
| `docs/YoloAgent设计/01-设计与解决方案.md` | 修正 §2.1/§3.2/§3.3「仅 Read」「max_iterations=4」陈旧描述 + 指向 03 |
| `docs/YoloAgent设计/02-系统提示词设计.md` | 加头部更新说明;第 205 行 profile 注释 |
| `docs/多Agent架构重构/03-技术实现文档.md` | yolo_registry 注释 |

`CLAUDE.md`(AGENTS.md):Yolo 条目补「信息收集型工具面 + ReAct 延迟强制」;
文档地图 `YoloAgent设计` 增 / 03。

## 5. 兼容性与回退

| 场景 | 行为 |
|------|------|
| `LAEW_FORCED_TOOLS=off` | 全部 profile 不注入 forced,Yolo 靠提示词 + 短路逻辑,行为等同本轮之前 |
| `LAEW_FORCED_TOOLS=on`(默认) | QC 等:每轮强制 emit(行为不变);Yolo:探索轮 auto / 末轮强制 emit |
| JSON 解析链 | 不变;emit input 序列化为 ```json 块;降级路径不变 |
| `parse_classification` 多候选 | 不变 |
| `retry_classify_with_hint` | 不变(再次跑 agent,自动适用 defer 规则) |
| `run_yolo` legacy | 不变 |
| 旧 e2e mock `--forced-tool` | mock 当轮仍返回 emit tool_use → Yolo 1 轮终止;断言改为 ReAct 语义(见 §6) |

## 6. 测试计划

### 6.1 单元测试

| 测试 | 断言 |
|------|------|
| `tools::yolo_registry_names_only_read_and_emit`(改写) | names = `[Read, Glob, Grep, Bash, MCP_Web_Use, submit_task_classification, SubAgent]` |
| `tools::yolo_and_main_work_registries_exclude_mcp_web_use`(改写) | Yolo 含 MCP_Web_Use;main_work/plan/quality 不含 |
| `tools::todo_write_registered_in_subagent_and_main_work_only` | Yolo 仍不含 TodoWrite(不变) |
| `profile::yolo_profile_only_has_read`(改写) | Yolo 含 ReAct 工具面;不含 Write/Edit/TodoWrite;`defer_emit_force=true`;其他 profile=false |
| `tools::subagent_tool_registered_for_delegating_roles_only` | Yolo 仍含 SubAgent(不变) |
| `tests::emit_tool_short_circuits_loop`(改写) | yolo defer 模式探索轮 `seen_forced=[None]`;`structured_emits=1`;1 轮终止 |
| `tests::yolo_react_emits_force_only_on_final_round`(新增) | max_iterations=2:`seen_forced=[None, Some(emit)]`;tool_calls=1;structured_emits=1 |
| `tests::non_emit_profile_never_forced` | 不变(SubAgent 无 emit) |
| `tests::forced_tools_switch_parsing` | 不变 |

### 6.2 端到端(`testReport/run_e2e.sh`)

| 节 | 改动 |
|----|------|
| 4j-1 | Yolo `tool_choice.type != "tool"`(探索轮不强制);tools 含 Read/Glob/Grep/Bash/MCP_Web_Use/submit;仍 1 轮终止(mock 当轮自愿) |
| 4j-2 | Yolo round1 无 forced → mock 不拒绝;QC forced → mock 拒绝一次 → degrade,链路贯通(不变) |
| 4j header comment | 「Yolo/Quality 以 tool_use 返回」改为「Quality 每轮强制;Yolo ReAct 延迟强制(探索轮 auto)」
| 1271 | 「Yolo(入口层,仅 Read)」改为「Yolo(入口层,信息收集型工具面) |

## 7. 未来方向(本轮不做)

1. **QC 同样延迟强制**:QC 有 Read/Glob/Grep 工具但目前 1 轮强制,等同 Yolo 旧况;
   适用 defer 模式可让 QC 在合规判定前先看产物。需在 Yolo 上验证稳态后推广。
2. **`TaskClassification.evidence: Vec<String>`**:ReAct 收集到的关键事实下游可见,
   减少 Main-Work 重复收集;代价是 emit schema + 全量构造器字段扩展,本轮保守不动。
3. ~~**`build_runtime_hints` 角色化**:把 explore_budget 文案参数化,按 profile 选择
   浏览器/桌面/Yolo 信息收集三种文案;现在用 `with_explore_budget(0)` 绕开。~~
   **✅ 已于第 120 轮(2026-09-23)落地**:`runtime_hints.rs` 新增
   `HintRole{Ui,Execute,Gather,Judge}` + `RuntimeHintCtx` + `build_runtime_hints_with`,
   explore_budget 耗尽文案按角色分叉,角色由 `AgentProfile::hint_role(trace)` 派生
   (Yolo → `Gather`)。Yolo 侧仍保留 `with_explore_budget(0)` —— 信息收集阶段本就不该
   被「进入执行期」打断,角色化解决的是**执行层**收到浏览器文案的语义误导(D5)。
   见 `docs/SubAgentWork执行层ReAct与连续工作模式/01-设计与解决方案.md` §3.4。
4. **structured-output guarantee 防 degrade 后探索**:resilient `forced_tool_rejected`
   一旦置位后,后续全 auto。若模型当轮拒绝提交 emit,后续轮可能无限探索——可加
   per-session 的「once-rejected 显式提示」类补救。