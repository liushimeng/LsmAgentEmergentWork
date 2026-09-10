# 专题-laew 实现进度对照表（防重复执行索引）

> **用途**:多轮「调研 → 实现」任务的中央状态账本。任何一轮要实现某个知识库 gap 前,
> 先查本表确认状态,避免重复实现。实现完成后**必须**回填本表 + 在对应专题原文条目旁
> 加 `✅ 已实现` 标记。
>
> 知识库存在三套 gap 编号,标注时注明出处:
> - **H 编号**:第十二轮 HTTP 专项(该专题附录 D.2,H1-H20)
> - **L 累计编号**:各轮合集索引(L1-L835,第十三/十四轮合集维护)
> - **L 独立编号**:第七轮结构化输出专题内部自带的 L1-L20(与累计制不连续)

## 状态图例

✅ 已实现 | 🟡 部分实现 | ⏳ 未实现(候选) | ⛔ 决策不做

## 一、LLM 调用层 / HTTP 客户端(第十二轮 H 编号)

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| H1 | 无超时设置(connect/read/request) | P0 | ✅ | `llm/mod.rs::build_http_client`(connect 10s)+ `llm/sse.rs::stream_chunks`(idle 90s / 总 600s) | 2026-09-08 第 03 轮 |
| H2 | 无重试机制 | P0 | ✅ | `llm/resilient.rs::ResilientLlmClient`(自动重试,最多 3 次) | 同上 |
| H3 | 无错误分类(可重试 vs 不可重试) | P0 | ✅ | `error.rs` 新增 `LlmHttp/LlmNetwork/LlmStream` + `resilient::is_retryable`(408/425/429/5xx/529 白名单) | 同上 |
| H10 | 无 idle 超时 watchdog | P1 | ✅ | `llm/sse.rs::stream_chunks`(tokio timeout 包裹 chunk 间读取) | 同上 |
| H4 | 无连接池调优 | P1 | ⏳ | — | — |
| H5 | 无代理 3 模式 | P1 | ⏳ | — | — |
| H6 | 无 TLS 3 层信任根 | P1 | ⏳ | — | — |
| H7 | 无中段流重开 | P1 | ⏳ | — | — |
| H8 | 无背压控制 | P2 | ⏳ | — | — |
| H9 | 无取消传播(CancellationToken) | P1 | ✅ | `agent/cancel.rs`(CancelToken + orphan tool_use backfill)、`llm/cancellable.rs`(CancelGate + CancellableLlmClient 装饰器,8 角色 LLM 调用一处包裹)、`agent/mod.rs::run_session_cancellable`(迭代边界/LLM/工具三处 select,drop 工具 future → bash kill_on_drop 级联清进程组)、`agent/orchestrator.rs::handle_cancellable`(Cancelled 短路不回流重试,并行层 spawn 传 token)、`tui/mod.rs` + `main.rs`(SIGINT 监听:第一次取消/第二次 exit 130) | 2026-09-08 第 07 轮 |
| H11 | 无首包超时 | P2 | 🟡 | connect_timeout 覆盖连接阶段;首字节后由 idle 90s 兜底 | 2026-09-08 第 03 轮 |
| H12 | 无连接健康检查 | P2 | ⏳ | — | — |
| H13 | 无熔断器 | P2 | ✅ | `llm/resilient.rs`(Closed/Open/HalfOpen 三态 + 连续 5 次重试耗尽失败熔断 + 30s 冷却 + 单探测 HalfOpen + generation 防过期并发结果回写 + RAII 探测位取消释放)、`error.rs::LlmCircuitOpen` | 2026-09-09 第 01 轮 |
| H14-H20 | DNS pinning / HappyEyeballs / 压缩 / 续传 / 去重 / 缓存 / Retry-After 解析 | P2 | ⏳ | (Retry-After 解析已随 H3 实现:`llm/mod.rs::parse_retry_after`,60s 封顶) | 部分 |

## 二、结构化输出 / JSON 修复(第七轮结构化输出专题独立 L 编号)

| 编号 | gap | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|---------|---------|
| L17 | 无 JSON 修复链 | ✅(Tier-1) | `agent/json_repair.rs`(智能引号/全角标点/单引号/裸控制字符/尾逗号/Python 常量,单遍 O(N)+512KB 守卫),接入 `yolo.rs` / `main_work.rs` / `quality.rs` 三处解析点;**刻意不做截断补全**(Quality fail-closed 语义保持,上轮 P0 测试钉死) | 2026-09-08 第 03 轮 |
| L16 | 无 jsonschema 校验 | ✅(轻量手写) | `agent/tool_schema_validator.rs`(type/required/properties/additionalProperties/enum/minimum/maximum/minLength/maxLength/items 校验子集)+ `error.rs::ToolSchemaValidation`+ `agent/mod.rs` 工具执行前接入 | 2026-09-09 第 08 轮 |
| L18 | 无 partial JSON 流式解析 | ⏳ | — | — |
| L19 | Yolo 无结构化输出强制 | ✅(emit 工具 + forced tool_choice) | `src/agent/tools/emit.rs`(SubmitTaskClassification / SubmitQualityReport 输出工具)+ `src/agent/profile.rs`(`AgentProfile.emit_tool` 字段)+ `src/llm/mod.rs`(`RequestMeta.forced_tool`)+ `src/llm/anthropic.rs`(`tool_choice: {"type":"tool","name":X,"disable_parallel_tool_use":true}`)+ `src/llm/openai.rs`(`tool_choice: {"type":"function","function":{"name":X}}`,默认仍 auto)+ `src/llm/resilient.rs`(`looks_like_tool_choice_rejection` + 4xx 拒绝自动降级 auto 重试一次)+ `src/agent/mod.rs`(`run_session_inner` emit 短路终止 + `LAEW_FORCED_TOOLS` 开关 + `forced_tools_enabled`)+ `src/agent/extrace.rs`(`structured_emits` 计数)+ `src/agent/system_prompt/mod.rs`(Yolo/Quality 提示词双通道:工具首选 + 文本 JSON 兜底);单元测试 504 全过(新增 ≥14 项)+ e2e §4j(mock `--forced-tool` / `--reject-tool-choice` 双模式,wire 断言 + 降级断言);方案 `tmpPlan/2026-09-09_13-结构化输出强制通道与forced-tool-choice方案.md` | 2026-09-09 第 13 轮 |
| L20 | 无跨 provider 归一化 | ⏳ | — | — |
| L6 | tool_choice 写死 auto | ✅(forced 指名 + 默认 auto 兼容) | 同 L19(`src/llm/anthropic.rs` + `src/llm/openai.rs` wire 注入,默认 None 时 Anthropic 不发 tool_choice / OpenAI 发 "auto" 维持现状) | 2026-09-09 第 13 轮 |

## 三、此前轮次已实现项(git 历史可溯)

| 功能 | 对应知识库维度 | 实现位置 | 提交 |
|------|--------------|---------|------|
| Bash 危险命令拦截 + 敏感路径(fail-closed) | 权限管控/沙箱(P0) | `agent/permissions/{dangerous,sensitive}.rs` | ef84cec |
| Bash 超时 + 进程组管理(setsid+killpg+kill_on_drop)+ 输出截断 | 第七轮 Bash 专题 | `agent/tools/bash.rs` | ef84cec |
| SubFlow 失败检测增强 | SubAgent 调度/质检专题 | `agent/subagent.rs` | d439f23 |
| Quality 解析失败 fail-closed | 质检机制专题(P0) | `agent/quality.rs` | d439f23 |
| SQLite 数据库重构 + Provider 导入/导出 | 数据迁移专题 | `database/*`、`config/provider.rs` | 897ea2d |
| Edit / Glob / Grep 工具 | 第七轮 文件编辑/代码检索专题 | `agent/tools/{edit,glob,grep}.rs` | 更早轮次 |
| Write 沙箱路径校验(SandboxViolation) | 沙箱设计专题 | `agent/sandbox_hook/mod.rs` | 更早轮次 |
| Yolo 三步意图识别 + 项目上下文五级链注入 | Yolo/上下文注入专题 | `agent/yolo.rs`、`agent/project_context.rs` | 更早轮次 |
| Session 历史摘要注入 | 记忆系统专题 | `agent/session_context.rs` | 更早轮次 |
| Agent-Memory 持久化(agent_memory 表) | 记忆系统专题 | `agent/memory.rs`、`config/agent_memory.rs` | 更早轮次 |
| SSE 流式解析管线(SseStream/ParseSink 双协议) | 流式输出专题 | `llm/sse.rs` | 更早轮次 |
| ContextMaxSize 上下文上限(默认 800K,DB/CLI/TUI/导入导出全链路 + 存量自动补全迁移) | 数据迁移专题 + Context 专题 | `database/{schema,models,provider}.rs`、`main.rs`、`tui/screen/provider_{form,list}.rs` | 2026-09-08 第 04 轮 |
| Compact Agent(第 8 角色):token 估算(字符/4+10%)、阈值触发(80%)、三档压缩率(80%/50%/20%)、保护段 + 尾部保留、LLM 失败硬截断降级、摘要超长守卫 | Context 专题(§1.1 对比表 6 项目借鉴)+ 第七轮 PromptCaching 专题 P0-3 | `agent/compact.rs`、`agent/orchestrator.rs` §0.2、`agent/system_prompt` COMPACT_BASE_PROMPT | 2026-09-08 第 04 轮 |
| WorkFlow 依赖分层 + 同层 SubAgent 自动并行调度(Kahn 分层 / `topo_sort` 复用 flatten / 层内 tokio::spawn + Semaphore(3) 有界并发 / 单层直通零开销 / 失败语义与串行一致按序回流 / 结果顺序确定) | 第六轮 SubAgent 调度与并发专题 §2.2(P0:SubAgent 并行 + WorkFlow 同层并行) | `agent/main_work.rs::topo_layers`、`agent/orchestrator.rs::execute_workflows` + `run_wf_unit`;e2e §4e(mock `--parallel-wfs` + ThreadingHTTPServer) | 2026-09-08 第 05 轮 |
| 自动截断续接:检测 `stop_reason = "max_tokens"/"length"` + 注入 nudge 续接 + 有界 4 次(对齐 AtomCode `MAX_TRUNCATION_RESUME`) + 累计完整文本返回 + 达到上限优雅降级 | 第六轮多轮对话与循环架构专题 §2.4(P1-2:截断续接) | `agent/mod.rs::run_session`(stop_reason 检测 + `is_truncation_stop_reason` + nudge 续接 + `accumulated_text` 累计 + `truncation_resumes` 计数器) | 2026-09-08 第 06 轮 |
| 取消传播与优雅中断(H9):CancellationToken 全链路 + LlmClient 装饰器一处包裹 8 角色 + Agent 循环三处 select(迭代边界/LLM/工具,drop 工具 future → bash kill_on_drop 级联杀进程组) + orphan tool_use backfill + Cancelled 短路不进 Yolo 回流 + 并行层 SubAgent 级联取消 + SIGINT 双次语义(第一次取消/第二次 exit 130) + `-p` 退出码 130 | 第五轮中断取消专题 §7.1 P0 三项(AbortController 通路/Bash 子进程 kill/消息层 backfill)+ 第六轮 SubAgent 调度专题 §11.2 P0(取消传播到 SubAgent)+ H9 | `agent/cancel.rs`、`llm/cancellable.rs`、`agent/mod.rs::run_session_cancellable`、`agent/subagent.rs::run_unit_with_cancel`、`agent/orchestrator.rs::handle_cancellable`、`tui/mod.rs`、`main.rs`;e2e §10(mock `--delay-ms` + kill -INT + 退出码/及时性断言);方案 `tmpPlan/2026-09-08_07-取消传播与优雅中断方案.md` | 2026-09-08 第 07 轮 |
| TUI 中文输入 UTF-8 光标修复(既有 P0 缺陷,本轮验证取消功能时发现):cursor 从「字符数」改为「字节偏移 + 字符边界不变量」,`prev_char_boundary` 回退/`display_width` 列号换算,修复中文第 2 字符必 panic(status 101) | TUI 渲染管线专题(第六轮 CJK 宽度算法维度) | `tui/input.rs` + 单元测试 4 项 + e2e §8 用例 14a(中文输入+退格+清空,tmux 真实按键) | 2026-09-08 第 07 轮 |
| LLM Provider 自动熔断器(H13/L188/L771):重试耗尽后统计连续可重试最终失败,5 次进入 Open 快速失败,30s 后自动 HalfOpen 单探测;成功关闭 / 失败重开;401/400 等请求侧错误与用户取消不计故障;并行 SubAgent 共享同 Provider 熔断状态 | 第十一轮错误处理与熔断专题 §10.2/§14 + 第十二轮 H13 + 第十四轮错误恢复 §4.4/L771 | `llm/resilient.rs` + `error.rs::LlmCircuitOpen`;单元测试覆盖 Open 快速失败 / 半开成功 / 半开失败重开 / 并发单探测 / 探测位取消释放 / 不可重试不熔断 / 成功重置;方案 `tmpPlan/2026-09-09_01-LLM熔断器自动三态防护方案.md` | 2026-09-09 第 01 轮 |
| SubAgent 执行轨迹(ExecutionTrace)与多维失败检测:Agent 循环内累计工具调用成功/失败/截断/早终止元数据;**早终止路径(RepeatedToolFailure/MaxIterationsExceeded)不再升级为 Error**,而是包装成失败摘要 + early_terminated=true 的 trace,让 QC 仍可判定;失败检测从二值文本启发式升级为多维(early_terminate / high_error_rate / text_failure_phrase 三种强信号);Agent-Memory artifacts 持久化 trace 全字段;QC 收到 ExecutionTrace 摘要辅助判据;Yolo 失败回流时引用 trace.failure_signals | SubAgent 调度专题 §6.1 执行轨迹 + 质检机制专题 + 任务拆解专题 L36 | `src/agent/extrace.rs`(新)+ `src/agent/mod.rs::run_session_inner` 累计 trace + `src/agent/subagent.rs::run_unit_inner` catch 早终止 + `src/agent/quality.rs::check_subagent` 新增 trace 参数 + `src/agent/orchestrator.rs` 三处串联(WorkflowResult 加 subflow_trace 字段 / QualityFailure 加 trace / run_yolo_with_failure 拼信号);单元测试 333 全过 + e2e 94 全过;方案 `tmpPlan/2026-09-09_05-SubAgent执行轨迹与多维失败检测方案.md` | 2026-09-09 第 05 轮 |
| Read 路径诊断(F-001) + 无文本收敛迭代可见性(F-002) + Yolo 降级告警(F-003):**Read 工具错误信息**在「相对路径在工作目录 + 根目录拼接都查不到」场景追加「请改用绝对路径」诊断提示(尤其 laew 子目录跑 laew 场景);**无文本收敛短路**从机械「次数统计」升级为「最近 N 次工具调用叙事化摘要」(含工具名 + 参数摘要 + 成功/失败状态)+ LLM 自助收敛指令;**Yolo 解析失败降级**时把降级警告 + actionable 建议追加到 SubAgent prompt,避免在空 goal_summary 上做无意义工具调用 | 多轮对话知识库 B09 跑测 + C09 跑测观察到的 LLM 反复构造变体路径 / 连续无文本 / Yolo 解析失败三类失败模式 | `src/agent/tools/read.rs::format_path_diagnostic`(新)+ `src/agent/mod.rs::run_session_inner` 累计 `recent_tool_history` 局部变量 + 短路命中块输出叙事化摘要 + `src/agent/extrace.rs::compact_args_digest`(新,PREFERRED 字段 + 长字符串截短) + `src/agent/yolo.rs::build_work_prompt` 末尾 `yolo_degraded` 分支追加警告 + 单元测试 13 项(F-001:3 + F-002:4 + F-003:2 + extrace digest 4);单元测试 366 全过 + e2e 97 全过;方案 `tmpPlan/2026-09-09_06-Read路径诊断与无文本收敛迭代可见性方案.md` | 2026-09-09 第 06 轮 |
| 上下文溢出自动检测与三级恢复(L1038+L1044):`is_context_overflow` 15+ provider 溢出正则(Anthropic `prompt is too long` / OpenAI `context_length_exceeded` / 网关变体)+ NON_OVERFLOW 排除集(限流/配额/鉴权误报)+ 状态码门槛(仅 400/413);Agent 循环 LLM 调用点一处包裹、8 角色全生效——Level 1 排水(超长 tool_result 原地截短,保头 2000+尾 1000,幂等)→ Level 2 折叠(非保护段历史 hard_truncate 合并为 `<<<LAEW:COMPACTED_CONTEXT>>>` 摘要,配对边界守卫防孤儿 tool_use)→ Level 3 暴露(原错误上抛);每次调用最多 1 排水+1 折叠(跨迭代重武装),全会话预算 4 次防打转;恢复不消耗 max_iterations;ExecutionTrace.overflow_recoveries 弱信号可观测;e2e mock `--overflow-once` wire 级验证 | 第十六轮 claudecode §2.2 prompt-too-long 三级恢复(L1038)+ 第十六轮 pi §8 溢出正则(L1044) | `src/agent/overflow.rs`(新)+ `src/agent/mod.rs::complete_with_overflow_recovery`(新)+ `src/agent/extrace.rs`(overflow_recoveries 字段)+ `src/agent/compact.rs`(render_messages/hard_truncate 开放 pub(crate));`scripts/mock_llm_server.py --overflow-once` + `run_e2e.sh` §4f;单元测试 356 全过 + e2e 97 全过;方案 `tmpPlan/2026-09-09_06-上下文溢出自动检测与三级恢复方案.md` | 2026-09-09 第 06 轮 |
| Write/Edit 沙箱白名单细化(Check-What-You-Write):**修复两类真实缺口**——①「检查路径 ≠ 实际写入路径」绕过(write.rs 的 D-003 根目录回退在 检查之后 改写落点,根目录 ≠ 工作目录时白名单外写入被放行;现移除写路径回退并改为「先解析后检查」,检查永远管辖最终落盘路径)②符号链接逃逸(normalize 升级为 fold + 最深已存在祖先 canonicalize 拼回,工作目录内 symlink 指向外部时新建/覆盖均被拦截);白名单根预计算 raw+canonical 双形态前缀;SandboxViolation 报「规范化后真实落点」;Read/Glob/Grep 不限、Bash 不接沙箱(需求明确保留权限);单元测试 418 全过 + e2e 104 全过(新增 §4g:mock `--write-outside` 负例 + `--write-inside` 正例,wire 级断言);方案 `tmpPlan/2026-09-09_08-沙箱权限管控细化方案.md` | 沙箱设计专题(细化) | `src/agent/sandbox_hook/mod.rs`、`src/agent/tools/{write,edit}.rs`、`scripts/mock_llm_server.py`、`testReport/run_e2e.sh` §4g | 2026-09-09 第 08 轮 |
| **Prompt 注入防护 L1208**:14 种正则模式(ignore_previous/system_override/role_hijack/curl_pipe_sh/drop_table/exfil_token/prompt_leak/fake_tool_call/boundary_escape/instruction_smuggle/markdown_exfil/env_dump/markdown_img_tracking),四级分级(Safe/Suspicious/Likely/Critical),`scan_and_wrap()` 在工具结果末尾追加 `<<<LAEW:INJECTION_ALERT>>>` 告警块(不阻断,LLM 自主判断 — 对齐 claudecode「flag it directly」);Bash/Read/Glob/Grep 4 个工具 execute 末尾一处包裹;环境变量 `LAEW_INJECTION_GUARD=off\|0\|false\|no` 关闭;OnceLock 编译期正则缓存;29 项单元测试 + e2e §4i mock `--inject-bash` wire 级验证;方案 `tmpPlan/2026-09-09_12-Prompt注入防护与外部内容净化方案.md` | 第十七轮 openclaw §11.0 14 种注入防护 + claudecode §11.1「flag it directly to the user」 + 跨项目缺口分析 §3.2 P0 表 L1208 | `src/agent/safety/{mod,prompt_injection}.rs`(新)+ `src/agent/mod.rs` `pub mod safety;` + `src/agent/tools/{bash,read,glob,grep}.rs` 各 1 处集成 + `scripts/mock_llm_server.py --inject-bash` + `testReport/run_e2e.sh` §4i;单元测试 488 全过(其中 safety 29 项)+ e2e PASS=113(+4);注意:L1213 进程级沙箱隔离(Landlock/Seccomp)仍 ⏳,本轮只做用户内容侧的注入防护 | 2026-09-09 第 12 轮 |

## 四、下一轮候选(按优先级)

1. ~~WorkFlow 同层并行调度(第六轮 SubAgent 并发专题 §2.2 P0)~~ ✅ 2026-09-08 第 05 轮已完成(方案 `tmpPlan/2026-09-08_05-WorkFlow依赖分层与SubAgent自动并行调度方案.md`)。同专题剩余 P0:**取消传播到 SubAgent**(H9 联动)。
2. ~~截断续接(第六轮多轮对话专题 §2.4 P1-2)~~ ✅ 2026-09-08 第 06 轮已完成(`agent/mod.rs::run_session`,方案 `tmpPlan/2026-09-08_06-自动截断续接与智能续轮方案.md`)。
3. ~~H9 取消传播~~ ✅ 2026-09-08 第 07 轮已完成(方案 `tmpPlan/2026-09-08_07-取消传播与优雅中断方案.md`)。~~H13 熔断器~~ ✅ 2026-09-09 第 01 轮已完成(方案 `tmpPlan/2026-09-09_01-LLM熔断器自动三态防护方案.md`);
4. **L18 partial JSON 流式解析**(断流时已收部分的语义保全);
5. **L16 schemars 工具参数校验**(工具执行前 fail-fast);
6. ~~Token 计数 + 上下文自动压缩(第七轮 PromptCaching 专题)~~ ✅ 2026-09-08 第 04 轮已完成(`agent/compact.rs`,方案 `docs/Context设置与自动压缩设计/01-设计与解决方案.md`)。

## 四-十五、第 15 轮新增 gap（L836-L1035，200 个）

> 日期:2026-09-09 | 维度:网络协议深度/编译器前端/OS内核交互/分布式共识/ML推理/形式化验证/图数据库/实时流处理

| 编号范围 | 维度 | 数量 | 状态 |
|---------|------|------|------|
| L836-L860 | 网络协议深度 | 25 | ⏳ |
| L861-L885 | 编译器前端 | 25 | ⏳ |
| L886-L910 | OS内核交互 | 25 | ⏳ |
| L911-L935 | 分布式共识 | 25 | ⏳ |
| L936-L960 | ML推理 | 25 | ⏳ |
| L961-L985 | 形式化验证 | 25 | ⏳ |
| L986-L1010 | 图数据库与知识图谱 | 25 | ⏳ |
| L1011-L1035 | 实时流处理 | 25 | ⏳ |

**P0 紧急(优先实现)**:
- L836 连接池空闲超时配置 → 抄 atomcode `retry.rs:54` POOL_IDLE_TIMEOUT=15s
- L838 mTLS 支持 → 抄 claude-code `mtls.ts` 全链路
- L840 指数退避重试 → 抄 claude-code `withRetry.ts`
- L843 沙箱 → 抄 deepseek-harness C11 Landlock launcher
- L861 Tree-sitter 语法解析 → 抄 opencode `tool/shell.ts`
- L862 LSP JSON-RPC 客户端 → 抄 opencode `lsp/client.ts`
- L886 Landlock/Seccomp 沙箱 → 抄 deepseek-harness `native/landlock-run`
- L887 进程树优雅终止 → 抄 openclaw `kill-tree.ts`
- L888 PTY 终端 → 抄 openclaw `terminal-pty.ts`
- L961 契约框架 → 抄 atomcode `conformance/` 模块

---

## 四-十六、第 16 轮新增 gap（L1036-L1165+，130+ 个）

> 日期:2026-09-09 | 维度:多轮对话恢复/压缩管线/内存加密/SQLite全栈/Hook系统/租约引擎/反应式IoC/LLM协议栈/扩展加载/录制回放

| 编号范围 | 维度 | 数量 | 状态 |
|---------|------|------|------|
| L1036-L1040 | 多轮对话恢复（七阶段管线/三级恢复×2/后台记忆） | 5 | ⏳ |
| L1041-L1042 | SQLite 全栈（WAL/完整性） | 2 | ✅ |
| L1043 | SQLite 跨进程租约 | 1 | ⛔ 决策不做(单用户单进程 CLI,无多进程形态;WAL+busy_timeout 已覆盖进程内并发) |
| L1044-L1045 | 溢出检测与重试策略 | 2 | ✅(L1044 第 06 轮;L1045 随 09-08 第 03 轮 resilient.rs,第 07 轮勘误) |
| L1046 | Route 五层抽象 | ⏳ | — | — |
| L1047 | Cache Policy 自动注入(Anthropic) | ✅ | `src/llm/cache_policy.rs`(CachePolicy/CacheHint/CacheTtl/CacheBreakpoints + apply_cache_policy 三元组 + 4 断点 cap)+ `src/llm/anthropic.rs::complete`(convert_system_blocks + apply 注入 last tool / last system / latest user)+ `src/llm/mod.rs::DEFAULT_CACHE_POLICY` + 8 单元测试 + 5 集成测试 + mock_llm_server `--cache-read/--cache-creation` + e2e §4h | 2026-09-09 第 10 轮 |
| L1048 | 反应式 IoC（Cordis Epoch） | 1 | ⏳ |
| L1049-L1050 | 实时同步与单飞准入 | 2 | ⏳ |
| L1051-L1065 | 权限/工具/压缩/记忆/MCP/Skill | 15 | ⏳ |
| L1066-L1082 | 内存加密/SQLite 12模块/Hook 14699行/租约 | 17 | ⏳ |
| L1083-L1094 | 缓存分析/HTTP/Provider/信任/扩展/OAuth | 12 | ⏳ |
| L1095-L1110 | 事件/状态机/Worker/脚本/SubAgent | 16 | ⏳ |
| L1111-L1120 | HTTP 缓存/SSE/multipart/WebSocket/AbortSignal | 10 | ⏳ |
| L1121-L1130+ | 协议/编译/状态持久化/工具安全 | 15+ | ⏳ |

**P0 紧急(优先实现)**:
- L1036 七阶段上下文管线 → 抄 claudecode `query.ts`
- L1037 max_output_tokens 三级恢复 → 抄 claudecode `query.ts:1186`(截断续接已于 2026-09-08 第 06 轮实现,差 max_tokens 静默升级 8k→64k)
- ~~L1037 max_tokens 静默升级 8K→64K~~ ✅ 2026-09-09 第 09 轮已完成(`src/agent/max_tokens_state.rs` 会话级 `MaxTokensState` 状态机 8K→16K→32K→64K 封顶;`RequestMeta.max_tokens_override` 协议层注入;`AnthropicClient::complete` 用 `meta.max_tokens_override.unwrap_or(DEFAULT_MAX_TOKENS)`;`OpenAiRequest` 新增 `max_tokens` 字段并在 `complete()` 注入,OpenAI 客户端此前根本不传此字段);`ExecutionTrace.max_tokens_upscalings` + `max_tokens_history` 写入;`run_session_inner` truncation 命中分支调 `note_truncated()` 翻倍;**联动 L771 失败计数预警**:`build_runtime_hints` 在 `<<<LAEW:RUNTIME_HINTS>>>` 标记下拼接 truncation_resumes / overflow_recoveries / max_tokens_upscalings / consecutive_failures ≥ 2 四类提示,仅在对应计数器 > 0 时追加,全 0 时零开销,追加到 system 末尾(不破坏 cache_control 缓存前缀);14 个新单元测试全过;方案 `tmpPlan/2026-09-09_09-max-tokens静默升级与失败计数预警方案.md`
- ~~L1038 prompt-too-long 三级恢复~~ ✅ 2026-09-09 第 06 轮已完成(`src/agent/overflow.rs`,方案 `tmpPlan/2026-09-09_06-上下文溢出自动检测与三级恢复方案.md`)
- L1039 cached microcompact → 抄 claudecode `microCompact.ts`
- ~~L1041 SQLite WAL 配置~~ ✅ 2026-09-09 第 07 轮已完成(`src/database/pragmas.rs` apply_pragmas:WAL/busy_timeout 5s/synchronous=NORMAL/cache 8MB/FK/启动 PASSIVE checkpoint;`Db::open`/`Db::clone` 共用工厂,clone 连接 PRAGMA 一致——修复并行 SubAgent 写 agent_memory `database is locked`;WAL 读回验证不支持时降级 rollback journal,方案 `tmpPlan/2026-09-09_07-SQLite并发WAL加固与完整性自愈方案.md`)
- ~~L1042 SQLite 完整性检测~~ ✅ 2026-09-09 第 07 轮已完成(`try_open_and_check` 三态:quick_check 报损坏/SQLITE_NOTADB(26)/CORRUPT(11) → 隔离 `{db}.corrupt-{时间戳}.bak` + 重建空库 fail-open;瞬态锁冲突不隔离防误报毁数据)
- ~~L1043 跨进程租约协调~~ ⛔ 2026-09-09 第 07 轮决策不做:laew 是单用户单进程 CLI,无多进程部署形态;WAL+busy_timeout 已覆盖进程内多连接并发,openclaw 式 SharedArrayBuffer 心跳租约对 CLI 属过度设计
- ~~L1044 上下文溢出检测~~ ✅ 2026-09-09 第 06 轮已完成(与 L1038 同轮,15+ 溢出正则 + NON_OVERFLOW 排除集;pi 的静默溢出检测 usage.input > contextWindow 事前预防未做,留作下一轮候选)
- ~~L1045 provider 重试策略~~ ✅ 实际已于 2026-09-08 第 03 轮随 `llm/resilient.rs` 完成(指数退避基数 500ms/倍数 2/上限 8s/±25% jitter/墙钟纳秒种子;账本原「差 jitter」描述过时,第 07 轮勘误)
- L1046 Route 五层抽象 → 抄 opencode `llm/src/route/client.ts`
- ~~L1047 Cache Policy 自动注入~~ ✅ 2026-09-09 第 10 轮(`src/llm/cache_policy.rs` + `anthropic.rs` 内化,Anthropic 路径 4 断点 cap 内置,详见 `tmpPlan/2026-09-09_10-L1047-Anthropic-PromptCaching自动注入方案.md`)
- ~~L1048 反应式 IoC~~ ⛔ 决策不做(deepseek-harness Cordis Epoch 是重度服务端架构,laew 单用户单进程 CLI 无需求)
- **Diff 渲染 + 语法高亮** ✅ 2026-09-10 第 18 轮(`src/tui/render/{mod,diff,highlight}.rs` + `theme.rs` 追加常量 + `main.rs` /diff 前置处理 + `mod.rs` 围栏高亮集成 + `/diff` 命令,`similar` crate 行级+字符级 diff + 手写 regex 8 类 token 高亮 + `lang_from_fence_tag` 围栏检测 + e2e §4k,7 项全过)
- L1048 反应式 IoC → 抄 deepseek-harness `vendor/cordis/src/fiber.ts`

---

*本表由 2026-09-08 第 03 轮(方案:`tmpPlan/2026-09-08_03-LLM自动弹性层与JSON自动修复链方案.md`)建立;后续每轮实现后回填。最近回填:2026-09-10 第 17 轮(用户交互体验层 D2+D8:自定义斜杠命令 `.laew/commands` 两级发现 + frontmatter + `$ARGUMENTS`/`$1-$9` 占位符 + 补全集成 + `/commands`;会话导出 `/export` Markdown/JSON + TUI 层 transcript + 用量;模块 `src/tui/{commands,export}.rs` + `completion.rs` 动态注册 + `tui/mod.rs` dispatch_prompt 统一收口,方案 `tmpPlan/2026-09-10_01-自定义斜杠命令与会话导出方案.md`),此前:2026-09-09 第 13 轮(结构化输出强制通道 L6+L19)、第 12 轮(Prompt 注入防护 L1208)、第 08 轮(Write/Edit 沙箱白名单细化)、第 07 轮(SQLite 并发 WAL 加固 L1041+L1042)、第 06 轮(上下文溢出 L1038+L1044)、第 05 轮(SubAgent 执行轨迹)。*

---

## 五、第十八轮登记(2026-09-09,用户交互体验层)

**主题**:前 17 轮 90+ 维度聚焦基础设施与协议层;本轮首次切入**用户交互体验层**——8 大全新维度。

| 维度 | 简称 | 已覆盖轮次 | laew 现状 | 新增 gap 段 |
|------|------|-----------|---------|------------|
| D1 | @提及系统 | ❌ 第 18 轮首次 | ❌ 0% | L1396-L1397 / L1426-L1427 / L1456+ / L1486+ / L1516-L1517 / L1546+ |
| D2 | 自定义斜杠命令/Prompt 模板 | ❌ 第 18 轮首次 | 🟡 40%(✅ 2026-09-10:两级目录 `.laew/commands` + frontmatter + `$ARGUMENTS`/`$1-$9` + 补全集成 + `/commands`;未做 allowed-tools/model/`!`shell``/递归命名空间) | L1398-L1399 / L1428-L1434 / L1457+ / L1487-L1490 / L1518-L1521 / L1547+ |
| D3 | 对话 Rewind/分支/时间旅行 | ❌ 第 18 轮首次(用户级) | ❌ 0% | L1400 / L1435+ / L1458+ / L1491-L1497 / L1522-L1527 / L1548+ |
| D4 | 文件监视与工作区感知 | ❌ 第 18 轮首次(运行时) | ❌ 0% | L1415+ (预留) / L1459+ / L1498-L1499 / L1549+ |
| D5 | 工具输出富文本内容渲染 | ❌ 第 18 轮首次(内容层) | ❌ 5%(cell-based 纯文本) | L1401-L1402 / L1436-L1445 / L1460+ / L1500-L1506 / L1528-L1530 / L1550+ |
| D6 | 输入体验工程 | ❌ 第 18 轮首次(系统化) | 🟡 30% | L1403-L1405 / L1446-L1450 / L1461+ / L1531-L1539 / L1551+ |
| D7 | Onboarding/目录信任/主题 | ❌ 第 18 轮首次 | ❌ 10%(1 套 ANSI) | L1406-L1409 / L1451+ / L1462+ / L1507-L1513 / L1540+ / L1552+ |
| D8 | 会话导出/Statusline/实时成本 | ❌ 第 18 轮首次 | 🟡 25%(✅ 2026-09-10:`/export [path]` Markdown/JSON + TUI 层 transcript + 每轮/累计用量;未做 Statusline/实时成本/脱敏/分享) | L1410-L1414 / L1452-L1455 / L1463+ / L1514-L1515 / L1541-L1545 / **L1564-L1575** |
| N1-N5 | undici 内容获取底座 | ❌ 第 18 轮首次(WebFetch 底座) | ❌ 0% | **L1576-L1590**(15 个,与 pi D8 冲突后修正) |

**统一编号总表**(已修正冲突,详见合集 §四):

| 区段 | 工程 | 维度 | 实际 gap 数 |
|------|------|------|------------|
| L1396-L1414 | atomcode | D1-D8(无 D4) | 19 |
| L1415-L1425 | _预留_ | _下一轮_ | _11 空位_ |
| L1426-L1455 | claudecode | D1-D8 | 30 |
| L1456-L1485 | deepseek-harness | D1-D8 | 30 |
| L1486-L1515 | openclaw | D1-D8 | 30 |
| L1516-L1545 | opencode | D1-D8 | 30 |
| L1546-L1563 | pi | D1-D7 主线 | 18 |
| L1564-L1575 | pi | D8(冲突修正) | 12 |
| L1576-L1590 | undici | N1-N5 底座 | 15 |
| **合计** | — | — | **184** |

**优先级分布**:P0=26 / P1=98 / P2=45 / P3=15

**laew P0 路线图**(1-2 周可做):
1. D6 IME 防护(`tui/input.rs` + `unicode-segmentation`)
2. D6 大粘贴截断(`tui/input.rs`)
3. D5 Diff 渲染行级(`tui/render/diff.rs` 新建 + `similar`)
4. D2 项目级 frontmatter 命令(`tui/completion.rs` + `agent/commands.rs` 新建 + `serde_yaml`)
5. D7 主题系统 4 主题(`tui/theme.rs` 拆分)
6. D8 实时 token/cost 显示(`tui/mod.rs` 横幅 + `tiktoken-rs`)
7. D7 Onboarding 向导(`tui/screen/onboarding.rs` 新建 + `dialoguer`)
8. D3 `/fork` 命令(`agent/session_fork.rs` 新建)
9. D1 @file 提及(`tui/input.rs` + `agent/attachments.rs` 新建)

**19 轮推荐(2026-09-09 之后)**:6 大新方向——
可访问性(a11y) / 多模态输出(图表/Mermaid)/ 安全与隐私 / A2A 协议 / 离线模式 / 跨设备同步。预计 50-80 个新 gap。

**累计**:L1-L1590+ 共 1590+ 个 gap(前 17 轮 1395+ + 第十八轮 184 + atomcode 预留 11 = 1590),19 轮预计突破 1670。

**相关专题文档**(本轮产出,共 8657 行):
- `专题/专题-第十八轮-{atomcode|claudecode|deepseek-harness|openclaw|opencode|pi|undici}-深度分析.md`(7 份,7988 行)

---

## 六、第十九轮登记(2026-09-09,用户交互体验层续 + 安全纵深 + 多模态 + A2A + a11y + 离线 + 同步)

**主题**:第十八轮首次切入「用户交互体验层」(D1-D8);本轮继续深挖该层剩余 6 维度(D9-D14),并首次系统化覆盖「安全纵深」「多模态输出」「A2A 协议」「可访问性」「离线模式」「跨设备同步」。

| 维度 | 简称 | 已覆盖轮次 | laew 现状 | 新增 gap 段 |
|------|------|-----------|---------|------------|
| D9 | 安全与威胁模型 | 🟡 第 14/17 轮部分 + 第 19 轮系统化 | 🟡 30%(Prompt 注入 L1208 ✅ / 溢出 L1044 ✅ / 熔断 H13 ✅ / 沙箱白名单 ✅) | L1591-L1640 |
| D10 | 多模态输出(图表/Mermaid/图片) | ❌ 第 19 轮首次系统化 | ❌ 5%(cell-based 纯文本) | L1641-L1700 |
| D11 | A2A 协议与多 Agent 互操作 | 🟡 第 16 轮部分 + 第 19 轮系统化 | ❌ 0% | L1701-L1760 |
| D12 | 可访问性 a11y / RTL / 屏幕阅读器 | ❌ 第 19 轮首次 | ❌ 10%(1 套 ANSI) | L1761-L1820 |
| D13 | 离线模式与本地优先 | ❌ 第 19 轮首次 | ❌ 0% | L1821-L1880 |
| D14 | 跨设备同步与会话漫游 | ❌ 第 19 轮首次 | ❌ 0% | L1881-L1930 |

**统一编号总表**:

| 区段 | 维度 | 实际 gap 数 |
|------|------|------------|
| L1591-L1640 | D9 安全与威胁模型 | 50 |
| L1641-L1700 | D10 多模态输出 | 60 |
| L1701-L1760 | D11 A2A 协议 | 60 |
| L1761-L1820 | D12 可访问性 | 60 |
| L1821-L1880 | D13 离线模式 | 60 |
| L1881-L1930 | D14 跨设备同步 | 50 |
| **合计** | — | **340** |

**优先级分布**:P0=45 / P1=180 / P2=90 / P3=25

**laew P0 路线图**(1-2 周可做):
1. D9 凭证加密(`src/agent/safety/credentials.rs` 新建 + `aes-gcm` + `secrecy` + `zeroize`)
2. D9 SSRF 防护(`src/agent/tools/webfetch.rs` 新建 + 私有 IP/CGNAT 阻断 + DNS pinning)
3. D10 Diff 渲染(`tui/render/diff.rs` 新建 + `similar` + ANSI 着色)
4. D10 语法高亮(`tui/render/highlight.rs` 新建 + `syntect` + 16 色 SGR)
5. D12 主题系统 4 主题(`tui/theme.rs` 拆分 + 高对比 + daltonized)
6. D13 离线检测(`src/llm/offline.rs` 新建 + heartbeat ping + 状态机)
7. D14 会话导出(`src/agent/session_export.rs` 新建 + JSON/Markdown/HTML)

**20 轮推荐(2026-09-09 之后)**:6 大新方向——
国际化 i18n 完整实现 / Web UI + Desktop App / OAuth 认证与多账号 / Release 工程化与 AutoUpdate / DevContainer 与容器化 / CRDT 与多端冲突。预计 60-100 个新 gap。

**累计**:L1-L1930+ 共 1930+ 个 gap(前 18 轮 1590 + 第十九轮 340 = 1930),20 轮预计突破 2030。

**相关专题文档**(本轮产出,约 7700 行):
- `专题/专题-第十九轮-安全与威胁模型深度对比.md`(D9,约 1500 行)
- `专题/专题-第十九轮-多模态输出深度对比.md`(D10,约 1200 行)
- `专题/专题-第十九轮-A2A协议与多Agent互操作深度对比.md`(D11,约 1200 行)
- `专题/专题-第十九轮-可访问性与RTL深度对比.md`(D12,约 1000 行)
- `专题/专题-第十九轮-离线模式与本地优先深度对比.md`(D13,约 1000 行)
- `专题/专题-第十九轮-跨设备同步与会话漫游深度对比.md`(D14,约 1000 行)
- `专题/专题-第十九轮-跨项目缺口分析.md`(综合,约 800 行)
- `专题/专题-第十八轮-跨项目缺口分析.md`(669 行,含编号冲突修正表)
- `专题/专题-第十八轮深挖合集.md`(本合集姊妹篇)
