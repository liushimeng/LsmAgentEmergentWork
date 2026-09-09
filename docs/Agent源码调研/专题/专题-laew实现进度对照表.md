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
| L16 | 无 jsonschema 校验 | ⏳ | — | — |
| L18 | 无 partial JSON 流式解析 | ⏳ | — | — |
| L19 | Yolo 无结构化输出强制 | ⏳ | — | — |
| L20 | 无跨 provider 归一化 | ⏳ | — | — |
| L6 | tool_choice 写死 auto | ⏳ | — | — |

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
| L1041-L1043 | SQLite 全栈（WAL/完整性/租约） | 3 | ⏳ |
| L1044-L1045 | 溢出检测与重试策略 | 2 | ⏳ |
| L1046-L1047 | LLM 协议栈（Route 五层/Cache Policy） | 2 | ⏳ |
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
- L1037 max_output_tokens 三级恢复 → 抄 claudecode `query.ts:1186`
- L1038 prompt-too-long 三级恢复 → 抄 claudecode `query.ts:1062`
- L1039 cached microcompact → 抄 claudecode `microCompact.ts`
- L1041 SQLite WAL 配置 → 抄 openclaw `infra/sqlite-wal.ts`
- L1042 SQLite 完整性检测 → 抄 openclaw `infra/sqlite-integrity.ts`
- L1043 跨进程租约协调 → 抄 openclaw `state/openclaw-state-lease.ts`
- L1044 上下文溢出检测 → 抄 pi `ai/utils/overflow.ts`
- L1045 provider 重试策略 → 抄 pi `ai/utils/retry.ts`
- L1046 Route 五层抽象 → 抄 opencode `llm/src/route/client.ts`
- L1047 Cache Policy 自动注入 → 抄 opencode `llm/src/cache-policy.ts`
- L1048 反应式 IoC → 抄 deepseek-harness `vendor/cordis/src/fiber.ts`

---

*本表由 2026-09-08 第 03 轮(方案:`tmpPlan/2026-09-08_03-LLM自动弹性层与JSON自动修复链方案.md`)建立;后续每轮实现后回填。最近回填:2026-09-09 第 05 轮(SubAgent 执行轨迹 ExecutionTrace 与多维失败检测),同表累计回填含第 16 轮(130+ 个新 gap，L1-L1165+)。*
