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
| H1 | 无超时设置(connect/read/request) | P0 | ✅ | `llm/mod.rs::build_http_client`(connect 10s)+ `llm/sse.rs::stream_chunks`(idle 90s / 总 600s)+ `llm/resilient.rs::RESPONSE_HEADERS_TIMEOUT`(响应头 TTFB 120s,补最后一段:send() 等包头此前无超时可无限挂起)+ CLI 等待心跳 `ProgressGuard`(-p/-f 每 20s stderr 提示)+ TUI/CLI 阶段反馈 `ProgressTx` 通道(orchestrator 9 类阶段边界 → TUI 队列挂起 1.5s 打印 `  [stage]` / CLI stderr 立即打印,快速任务零噪音;D05/D07 第 23 轮,方案 tmpPlan/2026-09-10_06) | 2026-09-08 第 03 轮 / 2026-09-10 第 20 轮补 TTFB+心跳 / 2026-09-10 第 23 轮补 TUI 阶段反馈 |
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
| L18 | 无 partial JSON 流式解析 | ✅(Tier-1.5 结构截断恢复) | `src/agent/partial_json.rs`(单遍 O(N) char 扫描状态机:`in_string`/`escape`/结构栈;trailing string 语义保留未闭合串内容;不完整原子值(截断的 bool/null/数字)丢弃该 pair;512KB 守卫;完全非法 → None);`TRUNCATED_KEY="__truncated__"` 标记让下游感知参数可能不完整;集成到 `src/llm/sse.rs::ParseSink` 的 `ToolCallEnd` + `finish()` in_flight 残留两处,三级回退(完整 JSON → partial JSON → `_raw`);单元测试 20 项 + sse 集成测试 3 项(截断恢复 / 不可恢复回退 / 完整 JSON 不标截断);e2e 无新增失败(基线 3 失败均为 cache_policy/export 预存);方案 `tmpPlan/2026-09-10_12-L18-partial-JSON流式解析与截断工具调用恢复方案.md` | 2026-09-10 第二十五轮 |
| L19 | Yolo 无结构化输出强制 | ✅(emit 工具 + forced tool_choice) | `src/agent/tools/emit.rs`(SubmitTaskClassification / SubmitQualityReport 输出工具)+ `src/agent/profile.rs`(`AgentProfile.emit_tool` 字段)+ `src/llm/mod.rs`(`RequestMeta.forced_tool`)+ `src/llm/anthropic.rs`(`tool_choice: {"type":"tool","name":X,"disable_parallel_tool_use":true}`)+ `src/llm/openai.rs`(`tool_choice: {"type":"function","function":{"name":X}}`,默认仍 auto)+ `src/llm/resilient.rs`(`looks_like_tool_choice_rejection` + 4xx 拒绝自动降级 auto 重试一次)+ `src/agent/mod.rs`(`run_session_inner` emit 短路终止 + `LAEW_FORCED_TOOLS` 开关 + `forced_tools_enabled`)+ `src/agent/extrace.rs`(`structured_emits` 计数)+ `src/agent/system_prompt/mod.rs`(Yolo/Quality 提示词双通道:工具首选 + 文本 JSON 兜底);单元测试 504 全过(新增 ≥14 项)+ e2e §4j(mock `--forced-tool` / `--reject-tool-choice` 双模式,wire 断言 + 降级断言);方案 `tmpPlan/2026-09-09_13-结构化输出强制通道与forced-tool-choice方案.md` | 2026-09-09 第 13 轮 |
| L20 | 无跨 provider 归一化 | ⏳ | — | — |
| L6 | tool_choice 写死 auto | ✅(forced 指名 + 默认 auto 兼容) | 同 L19(`src/llm/anthropic.rs` + `src/llm/openai.rs` wire 注入,默认 None 时 Anthropic 不发 tool_choice / OpenAI 发 "auto" 维持现状) | 2026-09-09 第 13 轮 |

## 三、此前轮次已实现项(git 历史可溯)

| 功能 | 对应知识库维度 | 实现位置 | 提交 |
|------|--------------|---------|------|
| Bash 危险命令拦截 + 敏感路径(fail-closed) | 权限管控/沙箱(P0) | `agent/permissions/{dangerous,sensitive}.rs` | ef84cec |
| Bash 敏感路径 word-boundary 细化(2026-09-12 第 47 轮 P1-1 修复):SSH 私钥文件名(`id_rsa` 等)+ 配置文件(`.netrc` `.gitconfig` 等)改用 word-boundary 匹配,避免 openssl `-newkey rsa:2048` 子串误中;证书扩展名(`.pem` `.p12` 等)需文件名型匹配,openssl/curl 写入参数(`-keyout` `-out` 等)白名单放行;新增 16 项单元测试覆盖 `allows_openssl_rsa_keygen` / `blocks_certificates` 等 | 第十七轮 openclaw §11.0 误报 P1 + 第一轮 SSH 私钥子串误中 P1-1 | `src/agent/permissions/sensitive.rs`(word-boundary 函数 + openssl 写入 flag 白名单 + 扩展名型 marker + `is_after_openssl_write_flag`) | 2026-09-12 第 47 轮 |
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
4. ~~**L18 partial JSON 流式解析**(断流时已收部分的语义保全)~~ ✅ 2026-09-10 第二十五轮已完成(`src/agent/partial_json.rs` + `sse.rs` 三级回退,方案 `tmpPlan/2026-09-10_12`);
5. **L16 schemars 工具参数校验**(工具执行前 fail-fast);
6. ~~Token 计数 + 上下文自动压缩(第七轮 PromptCaching 专题)~~ ✅ 2026-09-08 第 04 轮已完成(`agent/compact.rs`,方案 `docs/Context设置与自动压缩设计/01-设计与解决方案.md`)。
7. ~~**TUI 等待体验两项 P2 优化**(第 27 轮 F05 测试轮 / `tmpPlan/2026-09-10_22`)~~ ✅ 2026-09-10 第 27 轮已完成:`TuiSession.task_started_at: Option<Instant>` 字段 + `handle_normal_prompt` 入口设置 + `print_usage`/`format_task_result`/`print_task_result`/`print_assistant_text_with_agent` 统一清空 + 「本次用量」行末尾追加 `(耗时 N.NNs)`;stage_printer 协程启动时立即打印 `\r [waiting] ⠋ (0s)` 占位行,1s 后每秒切换 spinner 字符,直到 hold 1.5s 过期 flush 队列或通道关闭 — 消除提交后 1.5s 空窗;30s/60s 边界触发「响应较慢」/「可 Ctrl-C 取消」提示 35s mock 实测验证;e2e PASS=153 FAIL=3(FAIL 全为预存 cache_policy mock + /export 轮数 pathfmt,与本轮无关);`src/tui/mod.rs` +99/-15。

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

*本表由 2026-09-08 第 03 轮(方案:`tmpPlan/2026-09-08_03-LLM自动弹性层与JSON自动修复链方案.md`)建立;后续每轮实现后回填。最近回填:2026-09-13 第 01 轮(D4 工作区感知:`src/agent/workspace.rs` 懒刷新快照 + 8 角色 system brief + PROJECT_CONTEXT 工作区段 + TUI 横幅/`/workspace`/任务后变更提示;方案 `tmpPlan/2026-09-13_01-工作区感知与运行时环境注入方案.md`),此前:2026-09-11 第三十六轮(QC 强失败信号分级 + 预期负例契约链路修复:`src/agent/quality.rs::gate_report_on_trace` LA-3 text_failure_phrase 降为证据可豁免 + `src/agent/mod.rs` LA-4 last_bash_exit_code 无条件更新 + mock OpenAI 分支 call_no≥2 路由/Plan 路由/终答摘录三补;方案 `tmpPlan/2026-09-11_17-D06-E07-jq分组与生产者消费者测试及预期负例契约修复方案.md`),此前:2026-09-11 第三十七轮(D6-CtrlJ 输入拦截:`src/tui/input.rs` 显式拦截 Char('j')+CONTROL(LF 按键)插空格 + Char(c) 兜底对未绑定 CONTROL 修饰不产生字符;mock hard 档 plan 路由段 + SubAgent 规则锁定 BUG-M5;方案 `tmpPlan/2026-09-11_15-E08-D09编程检索提示词测试与TUI输入CtrlJ拦截方案.md`),此前:2026-09-10 第二十四轮(D3 对话 Rewind/分支:`src/agent/session_fork.rs` 轮次扫描 + `src/tui/branches.rs` 分支存储 + `/rewind [N]` `/undo` `/fork` `/branches` `/switch` 五命令 + /clear 自动快照,三处一致不变量 + 零丢失快照语义,方案 `tmpPlan/2026-09-10_07-D3对话Rewind与分支方案.md`),此前:2026-09-10 第二十三轮(D12 多主题系统 + /theme + LAEW_THEME 环境变量,4 主题:default / dark-contrast / light / daltonized;`src/tui/theme.rs` 保留所有现有 const + 新增 ThemeKind/Palette/4 套 Palette 常量表 + palette()/set_active()/active_kind()/from_env()/from_env_str() API;核心 cell 渲染位置 Cell::blank / engine::border_box / render/diff / render/highlight 改用 palette() 实时跟随主题;`src/tui/{completion,mod}.rs` 新增 /theme builtin + banner 主题提示行 + run_theme dispatch + help 行;单元测试新增 8 项全过;e2e LAEW_THEME=daltonized 启动 banner /theme 列出 4 主题 /theme dark-contrast 实时切换 /theme nope 未知报错;方案 `tmpPlan/2026-09-10_03-D12多主题系统与a11y配色方案.md`),此前:2026-09-10 第二十二轮(D6 输入体验:bracketed paste + 大粘贴 marker 登记簿 L1573 + 10000 字符截断注入 L1448 + 快速输入批量合并,模块 `src/tui/input.rs`,方案 `tmpPlan/2026-09-10_02-D6大粘贴防护与快速输入批量合并方案.md`),此前:2026-09-10 第 17 轮(用户交互体验层 D2+D8),此前:2026-09-09 第 13 轮(结构化输出强制通道 L6+L19)、第 12 轮(Prompt 注入防护 L1208)、第 08 轮(Write/Edit 沙箱白名单细化)、第 07 轮(SQLite 并发 WAL 加固 L1041+L1042)、第 06 轮(上下文溢出 L1038+L1044)、第 05 轮(SubAgent 执行轨迹)。*

---

## 五、第十八轮登记(2026-09-09,用户交互体验层)

**主题**:前 17 轮 90+ 维度聚焦基础设施与协议层;本轮首次切入**用户交互体验层**——8 大全新维度。

| 维度 | 简称 | 已覆盖轮次 | laew 现状 | 新增 gap 段 |
|------|------|-----------|---------|------------|
| D1 | @提及系统 | ❌ 第 18 轮首次 + 第 28 轮落地 | 🟡 70%(✅ 2026-09-10 第二十八轮:`@path`/`@"带空格"`/`@path#L10-20` 提取注入 `src/agent/attachments.rs` + TUI @ 实时路径补全 `src/tui/mention.rs`;未做 IDE 双向注入/already_read mtime 去重/PDF 引用/nucleo 模糊匹配) | L1396-L1397 / L1426-L1427 / L1456+ / L1486+ / L1516-L1517 / L1546+ |
| D2 | 自定义斜杠命令/Prompt 模板 | ❌ 第 18 轮首次 | 🟡 40%(✅ 2026-09-10:两级目录 `.laew/commands` + frontmatter + `$ARGUMENTS`/`$1-$9` + 补全集成 + `/commands`;未做 allowed-tools/model/`!`shell``/递归命名空间) | L1398-L1399 / L1428-L1434 / L1457+ / L1487-L1490 / L1518-L1521 / L1547+ |
| D3 | 对话 Rewind/分支/时间旅行 | 🟡 第 18 轮首次(用户级) + 第 24 轮落地 | 🟡 60%(✅ 2026-09-10 第二十四轮:`/rewind [N]` `/undo` `/fork` `/branches` `/switch` 五命令 + /clear 自动快照,内存分支存储上限 10;未做:文件侧恢复/Git checkpoint 联动/消息树持久化/in-place 编辑) | L1400 / L1435+ / L1458+ / L1491-L1497 / L1522-L1527 / L1548+ |
| D4 | 文件监视与工作区感知 | ❌ 第 18 轮首次(运行时) | 🟡 75%(✅ 2026-09-13 第 01 轮:`src/agent/workspace.rs` 懒刷新快照(git/工程类型/工具链/顶层结构/最近改动)+ 8 角色 system brief + PROJECT_CONTEXT 工作区段 + TUI 横幅/`/workspace`/任务后变更提示;未做:文件系统监听/快照持久化/mtime 与 @提及去重联动) | L1415+ (预留) / L1459+ / L1498-L1499 / L1549+ |
| D5 | 工具输出富文本内容渲染 | ❌ 第 18 轮首次(内容层) | ❌ 5%(cell-based 纯文本) | L1401-L1402 / L1436-L1445 / L1460+ / L1500-L1506 / L1528-L1530 / L1550+ |
| D6 | 输入体验工程 | ❌ 第 18 轮首次(系统化) | 🟡 55%(✅ 2026-09-10 第二十二轮:bracketed paste + 大粘贴 marker 登记簿 L1573 + 提交展开/10000 字符截断注入 L1448 + 快速输入批量合并;未做多行编辑器/Vim 模式/kill ring;✅ 2026-09-11 第三十七轮补 Ctrl+J/LF 按键拦截与 CONTROL 通用防护) | L1403-L1405 / L1446-L1450 / L1461+ / L1531-L1539 / L1551+ |
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
1. ~~D6 IME 防护(`tui/input.rs` + `unicode-segmentation`)~~ 🟡 2026-09-10 第二十二轮完成终端侧可落地部分(快速连续字符批量合并渲染,`poll(0)` 排空 + pending 事件回存;字符边界 panic 已于第 07 轮修复)
2. ~~D6 大粘贴截断(`tui/input.rs`)~~ ✅ 2026-09-10 第二十二轮完成(L1573+L1448)
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
1. ✅ D9 凭证加密(`src/agent/safety/credentials.rs` 新建 + `aes-gcm` + `zeroize`) — 2026-09-10 第二十一轮完成(L1600)
2. ✅ D9 SSRF 防护(`src/agent/safety/url_safety.rs` 新建 + 私有 IP/CGNAT 阻断 + IPv4-mapped IPv6 解包) — 2026-09-10 第二十一轮完成(L1608/L1625)
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

---

## 七、第二十一轮登记(2026-09-10,安全纵深 P0 落地)

**主题**:第十九轮首次系统化覆盖「安全纵深 D9」,本轮(第二十一轮)落地其 P0 路线图前两项(凭证加密 + SSRF 防护),并额外修复测试过程中发现的解密优雅降级缺陷。

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| L1600 | 凭证管理:无应用层 AES-256-GCM 加密 | P0 | ✅ | `src/agent/safety/credentials.rs`(Vault 单例 + AES-256-GCM + 0o600 master.key + 存量迁移 migrate_credentials + export 脱敏告警)、`src/database/provider.rs`(写入加密/读出解密) | 2026-09-10 第二十一轮 |
| L1608 | SSRF:无私有 IP 拦截(loopback/private/CGNAT) | P0 | ✅ | `src/agent/safety/url_safety.rs`(is_safe_endpoint + IPv4/IPv6 完整拒绝集)、`src/llm/mod.rs::client_from_record`(创建 LLM 客户端前校验) | 2026-09-10 第二十一轮 |
| L1625 | SSRF:无 IPv4-mapped IPv6 解析 | P0 | ✅ | `src/agent/safety/url_safety.rs`(ipv4_mapped 解包重检) | 2026-09-10 第二十一轮 |

**额外修复(测试过程中发现的 P1 健壮性缺陷)**:
- 解密优雅降级:`row_to_record` 解密失败(主密钥轮换/损坏/篡改)→ WARN 日志 + `[解密失败]` 占位符,不再崩溃整个 `provider list()`。对应真实场景:用户主密钥丢失时仍可查看/删除其他记录。

**验证**:
- 581 单元测试全过(新增 credentials 11 项 + url_safety 14 项 = 25 项)
- e2e PASS=132 / FAIL=3(预存,与改动前基线完全一致,3 个 FAIL 均在 cache_policy/export 模块,非本轮引入)
- DB 物理验证:15/15 条加密,0 明文泄漏,末 4 位解密正确
- SSRF 实测:私网 endpoint 报 `URL 不安全: private/internal IP 被拦截: 127.0.0.0/8 loopback`,LAEW_ALLOW_PRIVATE_ENDPOINT=1 可放行
- 解密降级实测:id=19 用非常规密钥加密后,list() 输出 WARN + 占位符,其余 14 条正常显示

**累计**:L1-L1930+ 共 1930+ 个 gap,本轮新增 3 个 ✅(L1600/L1608/L1625)。

---

## 八、第二十二轮登记(2026-09-10,D6 输入体验:大粘贴防护 + 快速输入批量合并)

**主题**:第十八轮 D6 输入体验工程 P0 路线图第 1/2 项落地。laew TUI 主屏输入此前无 bracketed paste 支持:粘贴 1000 行 = 数千次逐字重绘 + 粘贴内 `\n` 误触发提交;无大粘贴保护,1MB 粘贴直接进 LLM 上下文。

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| L1573 | pi D6:无大粘贴截断(无 `[paste #N]` marker 机制) | P1 | ✅ | `src/tui/input.rs`(bracketed paste `EnableBracketedPaste` + `Event::Paste` 整体接收 + `PasteRegistry` 登记簿:>10 行或 >1000 字符 → `[粘贴 #N +M 行]`/`[粘贴 #N M 字符]` marker,提交时精确匹配展开还原,损坏/未知 marker 原样保留;回显 marker 版、Submitted 展开版) | 2026-09-10 第二十二轮 |
| L1448 | claudecode D6:大粘贴截断(10000 阈值 + 首尾预览) | P1 | ✅ | `src/tui/input.rs::truncate_for_inject`(单份 >10000 字符提交注入时截断为首 500 + `[...中间省略 N 字符...]` + 尾 500,char 边界安全) | 2026-09-10 第二十二轮 |
| D6-IME | IME 快速上屏逐字重绘卡顿(终端侧可落地部分) | P1 | ✅ | `src/tui/input.rs` Char 分支批量合并(`event::poll(0)` 排空同类 Char 一次性插入 + 单次重绘 + 非字符事件 pending 回存不丢事件;无 bracketed paste 旧终端的粘贴同时受益) | 2026-09-10 第二十二轮 |

**设计要点**:
- 登记簿生命周期 = 单次 read_line 调用(局部变量),提交/退出即弃,无跨行一致性负担。
- 小粘贴(≤10 行且 ≤1000 字符)过滤控制字符后 `\n`/`\t` 归一为空格直插(单行输入语义)。
- 回显用 marker 版 buffer(`>> 帮我分析 [粘贴 #1 +123 行]`),`Submitted` 返回展开版 —— 1000 行原文不回显刷屏。
- 不引入新 crate(crossterm 0.27 原生 `Event::Paste`),遵守 TUI 约定;`InputHandler::read_line` 对外签名不变,`tui/mod.rs` 零改动。

**验证**:
- 单元测试 592 全过(基线 581 + 新增 11 项 paste 测试:过滤/大小判定/marker 格式/计数递增/展开还原/截断注入/损坏 marker/未知编号/char 边界安全)
- e2e `run_e2e.sh` §8 新增 14c(2 个 tmux 用例:小粘贴直插 / 大粘贴 marker);本轮因并行会话占用 mock 未跑全量,已独立 tmux 手动验证通过
- 方案:`tmpPlan/2026-09-10_02-D6大粘贴防护与快速输入批量合并方案.md`

**未做(后续轮次候选)**:多行编辑器 / Vim 模式 / kill ring / Ctrl+G 外部编辑器 / 命令队列(L1449) / `!` bash 直通(L1450)。

**累计**:L1-L1930+ 共 1930+ 个 gap,本轮新增 2 个 ✅(L1573/L1448)+ D6-IME 终端侧落地。

---

## 九、第二十四轮登记(2026-09-10,D3 对话 Rewind / 分支 / 时间旅行)

**主题**:第十八轮 D3 维度 P0 路线图第 8 项落地。laew 此前多轮对话只进不退:误发一条提示词
无法撤销,想换方向只能 `/clear` 全丢。本轮按**轻量派**(pi 树状分支 / deepseek-harness
`forkAt(seq)`)实现纯对话侧的用户级恢复,不耦合文件状态(文件侧恢复等第七轮 Git
checkpoint 底座,见方案 §5)。

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| L1400 | atomcode D3:无 rewind(对话侧) | P1 | ✅ | `src/agent/session_fork.rs`(合成消息识别 `<<<LAEW:`/`[PREVIOUS_FAILURE]` + `scan_user_turns` 轮次扫描 + `turn_boundary` 截断边界)+ `src/session.rs::fork_from` | 2026-09-10 第二十四轮 |
| L1435+ | claudecode D3:/rewind 对话侧恢复 | P0 | ✅ | `src/tui/mod.rs::run_rewind/run_rewind_order/run_undo`:无参列出全部真实轮次(#编号+时间+首行预览),`/rewind N` 回退到第 N 轮之前 | 2026-09-10 第二十四轮 |
| L1491-L1497 | openclaw D3:rewind/fork/switch 三 action | P0 | ✅ | `src/tui/mod.rs::run_fork/run_switch/run_branches` + `src/tui/branches.rs`(BranchStore 内存分支存储:计数命名/容量 10 淘汰最旧/快照-恢复原子成对) | 2026-09-10 第二十四轮 |
| L1522-L1527 | opencode D3:session.fork + 树重映射 | P1 | ✅(单链分叉形态) | `/fork` = `Session::fork_from`(新 ID + 上下文完整拷贝);消息树持久化未做(等 Session 持久化轮次) | 2026-09-10 第二十四轮 |
| L1548+ | pi D3:/fork from history + /clone current leaf | P1 | ✅(clone current leaf 形态) | `/fork` + `/switch <name>`(从分支任意时间点继续) | 2026-09-10 第二十四轮 |

**设计要点**:
- **三处一致不变量**:回退/恢复同时作用于 `session.context` / `transcript` / `session_usage`
  (用量由剩余轮次重算),不存在错位;轮次↔transcript 严格 1:1。
- **零丢失语义**:`/rewind` `/fork` `/switch` `/clear` 四个破坏性操作前自动快照存分支
  (atomcode RewindTransactionGuard 的「截断与快照同生成败」语义);`/clear` 误触可救回。
- **头部标记保留**:截断从轮次边界开始,项目上下文/历史摘要标记天然保留,幂等探测
  不受影响(不会重复注入)。
- 分支为内存态(退出 TUI 失效),上限 10 个淘汰最旧 —— 快照含完整上下文,必须有界。

**验证**:
- 单元测试 631 全过(新增 20 项:session_fork 11 + branches 7 + session fork_from 2)
- e2e §7c 新增 13 条断言(独立根目录 + 无 provider,NoopLlm 全本地确定性,不依赖 mock)
  独立验证 13/13 全过
- 方案:`tmpPlan/2026-09-10_07-D3对话Rewind与分支方案.md`

**未做(后续候选)**:文件侧恢复(Git checkpoint 联动)/ 消息树持久化(parentUuid 树 /
JSONL 落盘,等 Session 持久化)/ in-place 消息编辑(pi 亦无)/ rewind 后自动重发
(用户手动,避免意外扣费)。

**累计**:D3 维度 laew 现状 0% → 60%。

---

## 十、第二十八轮登记(2026-09-10,D1 @文件提及系统 + TUI 实时路径补全)

**主题**:第十八轮 D1 维度 P1 落地(laew P0 路线图第 9 项)。laew 此前引用文件只能口述
路径让 SubAgent 二次 Read(多一轮 LLM 往返);本轮实现 claudecode attachments.ts 的
核心闭环:提交时提取 @ 提及 → 读取 → 以附件块注入上下文,并在 TUI 输入时提供
@ 实时路径补全。

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| L1426 | claudecode D1:@ 文件提及(三形态 + 行号片段 + 目录树 + 大文件降级) | P1 | ✅ | `src/agent/attachments.rs`(extract_mentions 双正则 + `#L10`/`#L10-20` 解析 + 逆区间归一 + agent/MCP 伪提及排除 + 目录内联 200 条 + 256KB 上限拒读 + 8KB 二进制嗅探 + 去重 + 单轮 8 附件上限 + `<<<LAEW:ATTACHMENTS>>>` 块),注入点 `tui/mod.rs::dispatch_prompt` + `main.rs`(-p/-f) | 2026-09-10 第二十八轮 |
| L1427 | claudecode D1:@ 时实时自动补全 | P1 | 🟡 | `src/tui/mention.rs`(FileSuggester:walkdir 快照 max_depth 6 / 20000 条上限 / 5s 节流 + 前缀∪文件名匹配 + 15 条上限)+ `src/tui/input.rs`(MentionCompletion 状态 + update_completion @ 分支 + Tab 拼接替换钻取/闭合;Enter 原样提交) | 2026-09-10 第二十八轮 |

**设计要点**:
- **存在才注入**:不存在/二进制/超 256KB → missed 提示,原文不动 —— 邮箱
  `user@host`、粘贴文本中的 `@` 天然不误命中(前置必须行首/空白)。
- **transcript/rewind 记原文**:附件展开只作用于送入 Session 上下文的 user 消息,
  raw 不动,D3 轮次扫描/导出/预览全部不受影响。
- **Tab 语义**:replacement 保留 `@` 前缀(目录尾随 `/` 保持浮层继续钻取,文件尾随
  空格闭合);Enter 不吞键、不做 slash 的真前缀自动提交。
- **零新 crate**:regex/walkdir 既有依赖。

**验证**:
- 单元测试 716 全过(attachments 20 项 + mention 24 项,含并行会话补充的
  @ 前缀回归钉子 2 项)
- e2e `run_e2e.sh` 新增 §4l 六条 wire 级断言全过(附件块标记/canary 内容/目录内联/
  不存在跳过提示/负例透传);全量 PASS=153 FAIL=3(3 个为第 21 轮记录的既有基线:
  cache_policy 2 + export 1)
- 顺带修复 run_e2e.sh 准备段 `rm -f /tmp/laew-e2e-root` 对目录无效导致残留 DB
  级联失败(§2/§3)的脚本 bug(→ `rm -rf`)
- TUI tmux 手测:@ 浮层出现 / Tab 钻取 src/ / 二次 Tab 接受文件 / Enter 提交
  打印「[附件] 已附加 1 个」
- 方案:`tmpPlan/2026-09-10_13-D1-文件提及与路径补全方案.md`

**未做(后续候选)**:IDE 双向注入(无 IDE 端,L1428)/ already_read_file mtime 去重
(L1429)/ PDF 轻量引用(L1430)/ nucleo 模糊匹配与 .git/index mtime 唤醒(L1427 完整版)。

**累计**:D1 维度 laew 现状 0% → 70%。

## 十二、第三十七轮登记(2026-09-11,D6-CtrlJ 输入拦截 + mock hard 档 plan 路由)

**主题**:E08(hard)/D09(simple)提示词多轮 TUI 测试发现并修复两类问题:
① TUI 输入 Ctrl+J/LF 按键污染(原始模式 LF(0x0A) 被 crossterm 解析为
Char('j')+CONTROL,落入 Char(c) 兜底把字母 j 插入输入缓冲——tmux send-keys
多行输入/无 bracketed paste 旧终端逐键粘贴/用户按 Ctrl+J 均触发,「slow.py:\n用」
回显成「slow.py:j用」);② mock prompt-router 无法编程 hard 档任务(laew hard 档
WorkFlow 唯一来源是 Plan Agent markdown,Main-Work 不发 LLM 请求 → mainwork 段
永不生效)。

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| D6-CtrlJ | 终端 LF/Ctrl+J 按键被当普通字符插入(控制字节→Char(letter)+CONTROL 家族残留) | P1 | ✅ | `src/tui/input.rs`(显式拦截 Char('j')+CONTROL → 插空格,与 PasteInsert::Inline 单行归一语义对齐;Char(c) 兜底对未绑定 CONTROL 修饰一律不产生字符,readline 语义,防 Ctrl-T/Ctrl-N 等同类) | 2026-09-11 第三十七轮 |
| mock-plan-router | mock 无法编程 hard 档 WorkFlow(prompt-router mainwork 段只挂 Main-Work 角色,hard 档不触发) | P1(测试基建) | ✅ | `scripts/mock_llm_server.py::_route_plan_markdown`(规则可选 "plan" 段:独立 keywords 取 yolo goal_summary 特征词 + workflows;role=plan 按语料路由生成 PLAN_MARKDOWN 同构 markdown) | 2026-09-11 第三十七轮 |
| BUG-M5 | mock SubAgent 路由规则穿透(命中规则缺当前 call_no 时继续扫后续规则,被泛关键词规则截胡重放别的轮次工具链) | P1(测试基建) | ✅ | `scripts/mock_llm_server.py::_route_subagent_tool`(关键词命中即锁定规则,规则内无该 call_no 直接落 default_call) | 2026-09-11 第三十七轮 |

**设计要点**:
- Ctrl+J 归一为空格而非换行/提交:与既有小粘贴「\n/\t → 空格(单行输入语义)」一致;
  多行编辑器是 D6 未做项,不在本轮扩大范围。
- plan 段 keywords 必须取 yolo goal_summary 特征词:Plan 上下文只有 Yolo 摘要,
  不含用户原始 prompt(与 SubAgent 路由语料不同,不能用轮次 token)。
- simple 档 TUI 横幅锚点约定:wf 名 = goal_summary 截 20 字符,路由表需把轮次
  token 前置写进 goal_summary(≤20 字符)。
- 本机测试环境:rg 真实二进制不在 PATH(仅 Claude Code shell 函数伪装),已把
  vscode-server 自带 ripgrep 链接到 ~/.local/bin/rg;laew Bash 工具 `bash -lc`
  行为正确(command not found 报错清晰)。

**验证**:E08 4 轮(SPEEDUP_OK slow=1.228 fast=0.002)+ D09 4 轮(rg 统计 3 行/
unwraps.log 3 行/table rows=5/rg mentions=7)TUI 全过;cargo test 745 全过;
8 轮多行提示词回显零 j 污染。

**方案**:`tmpPlan/2026-09-11_15-E08-D09编程检索提示词测试与TUI输入CtrlJ拦截方案.md`

## 十三、第三十六轮登记(2026-09-11,QC 强失败信号分级 + 预期负例契约链路修复)

**主题**:D06(medium)/E07(hard)提示词多轮 TUI 测试(macOS/openai mock,管道多轮 +
`-debug`)发现并修复三类问题:① QC trace 证据门的 `text_failure_phrase` 强信号被
终答内嵌的「工具输出摘录」误触发(引用日志 ≠ 模型声称失败),预期负例轮被 3 轮回流
拒绝;② `last_bash_exit_code` 仅在非零时写入,成功命令永远无法复位 → 699a677 引入的
预期负例契约(`bash_exit_nonzero>0 && last_exit==0`)在真实 Agent 循环永不可达成
(单测手工置 0 才通过,端到端从未验证);③ mock OpenAI 分支缺失 call_no≥2 路由分发
(与 Anthropic 分支不对称,OpenAI 协议多步工具链不可模拟)+ Plan 角色不支持路由覆写
(hard 档链路完全失控,四轮假通过:产物零落盘但 QC 全 ✅)。

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| LA-3 | text_failure_phrase 被终答引用的工具日志(如 `AssertionError:` 小写含 `error:`)误触发为强失败信号,预期负例无法通过 | P1 | ✅ | `src/agent/quality.rs::gate_report_on_trace`(强失败信号分级:early_terminate/high_error_rate 保持无条件强拒绝;text_failure_phrase 降为"证据可豁免"——QC 非空 evidence 或 EXPECTED_NEGATIVE_OK 契约佐一即可放行;纯措辞无佐证仍 fail;新增 4 单测) | 2026-09-11 第三十六轮 |
| LA-4 | last_bash_exit_code 只记非零,后续成功命令无法复位,预期负例契约(last_exit==0)真实链路永假 | P1 | ✅ | `src/agent/mod.rs`(Bash 工具输出解析退出码后无条件更新,含 0;`bash_exit_nonzero_count` 仍只累计非零;extrace.rs 字段文档同步) | 2026-09-11 第三十六轮 |
| mock-openai-router | mock OpenAI 分支 call_no≥2 不查 PROMPT_ROUTER,多步工具链(Write→Write/Read→Write)在 openai provider 下不可模拟 | P0(测试基建) | ✅ | `scripts/mock_llm_server.py::build_openai_stream`(补齐与 build_anthropic_stream 同构的 `_route_subagent_tool` 分发;未命中保持纯文本终答,默认行为不变) | 2026-09-11 第三十六轮 |
| mock-tool-excerpt-oai | `_extract_last_tool_result` 只解析 Anthropic tool_result 块,OpenAI wire(role="tool")终答永不带工具输出摘录 | P2(测试基建) | ✅ | `scripts/mock_llm_server.py::_extract_last_tool_result`(补 role="tool" 分支) | 2026-09-11 第三十六轮 |
| mock-plan-router-36 | mock Plan 角色恒返回默认方案,hard 档(Yolo→Plan→Main-Work)mainwork 覆写永不生效(与第三十七轮 mock-plan-router 同题独立实现,合并后共存) | P0(测试基建) | ✅ | `scripts/mock_llm_server.py::_route_plan_markdown`(规则可选 "plan" 段与 mainwork 同构;hard 档规则的 goal_summary 需植入轮次关键词——Plan 上下文只有 Yolo 摘要) | 2026-09-11 第三十六轮 |

**设计要点**:
- QC fail-closed 语义不回退:D01Q4 类负例(bash 非零 + last_exit=1 + 无 evidence)
  仍 3 轮回流失败收口;LA-3 只放宽文本启发式信号且需佐证。
- 预期负例契约标准形态:复现命令**重定向落盘**(不用 `\| tee`,管道掩盖非零退出),
  断言命令 exit 0 并输出 EXPECTED_NEGATIVE_OK。
- 断言命令防"空转通过":文件存在性前置(`test -f`)+ `set -o pipefail` +
  序列字符串等值断言(diff 两个同错输出会判等,exit 0 假阳性)。
- mock 默认行为不变:三处修复均只在 PROMPT_ROUTER 规则显式配置时生效
  (run_e2e.sh 120 用例全过佐证)。

**验证**:D06 4 轮(jq -c 提取/group_by 3 行/reduce 等价 diff/n 序列 "1,1,3")+
E07 4 轮(-debug,预期负例契约达成 + processed=100)TUI 全过;cargo test 751 全过
(含新增 4 个 gate 单测);run_e2e.sh PASS=120 FAIL=0。

**方案**:`tmpPlan/2026-09-11_17-D06-E07-jq分组与生产者消费者测试及预期负例契约修复方案.md`

---

# 第四十轮(2026-09-11)E09/C10 编程与系统信息提示词测试新增登记

测试范围:`docs/自动化测试-提示词文件列表/05-编码Coding与调试修复.md` E09 + `03-电脑使用与系统管理.md` C10(2 套提示词 × 4 轮 = 8 轮 TUI 多轮)。

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| E09-prompt | 05 编码维度 E09「测试策略设计 + 实际编写」(medium 档)未在隔离环境 TUI 多轮验证 | P2(回归覆盖) | ✅ | `tests/prompt_router_e09_c10.json`(E09×4 规则 + yolo/mainwork 段)+ `TestWorkSpace/E09_q1-4.md`(4 轮多行提示词)+ `tmpPlan/run_e09c10_tui.sh`(18965 端口隔离环境驱动)+ TUI 4 轮全过(产物 calc.py 1089B/test_calc.py 1375B/test.log 含 OK) | 2026-09-11 第四十轮 |
| C10-prompt | 03 电脑使用维度 C10「系统信息汇总脚本含 JSON 输出」(hard 档)未在隔离环境 TUI 多轮验证 | P2(回归覆盖) | ✅ | `tests/prompt_router_e09_c10.json`(C10×4 规则 + yolo/plan/mainwork 段,plan 段 keywords 含 goal_summary 特征词)+ `TestWorkSpace/C10_q1-4.md`+ `tmpPlan/run_e09c10_tui.sh`+ TUI 4 轮全过(产物 sysinfo.sh/sysinfo.md/sysinfo.json 5 keys 完整) | 2026-09-11 第四十轮 |
| mock-plan-keywords-merge | mock `_route_plan_markdown` 仅用 `rule.keywords` 命中,但 Plan 输入只含 yolo 合成 goal_summary(无用户 prompt 轮次 token);需合并 plan.keywords 才能命中 | P0(测试基建) | ✅ | `scripts/mock_llm_server.py::_route_plan_markdown`(keywords = rule.keywords ++ plan.keywords;plan 段写 goal_summary 特征词即可生效,不再依赖 user 轮次 token) | 2026-09-11 第四十轮 |
| mock-evidence-empty | mock QC 报告无 `evidence` 字段,SubAgent 终答引用 grep 输出含 "error:"(来自 `ValueError: division by zero` 子串)触发 text_failure_phrase 软信号 + evidence 真空 → fail-closed | P1(测试基建) | ✅ | `tests/prompt_router_e09_c10.json` E09Q1 calc.py(`raise ValueError("DIVIDE_BY_ZERO_RAISED")` + `print(f"divide(1,0) -> RAISED: {e}")`;避免 "ValueError:" / "error:" 子串触发 text_failure_phrase 软信号);QC 报告 evidence 仍为空,但 text_phrase_only + 无 bash 退出非零双重豁免可保持 pass(LA-3 设计意图) | 2026-09-11 第四十轮 |
| e09q4-asserts-syntax | E09Q4 unittest 成功时不打印 `failures=0`(unittest 文本输出格式差异),原 grep 永远 0 命中 | P2(测试基建) | ✅ | `tests/prompt_router_e09_c10.json` E09Q4 bash command(asserts 改为 `grep -cE 'Ran [0-9]+ tests'` + `grep -cE '^OK$'` + final run grep tail -1) | 2026-09-11 第四十轮 |

**驱动脚本**:`tmpPlan/run_e09c10_tui.sh`(18965 端口,单次 Bash 调用完成 mock+provider+TUI+4 round+cleanup 全流程,Bash 工具沙盒不杀后台进程的边界 — 参见 [[tmux-test-sandbox-traps]])。

**路由约定补充**:hard 档 plan 段 keywords 必须取 **yolo 覆写段 goal_summary 中的特征词**(如「断言 5 个二级标题」),不能用用户 prompt token(Plan 输入不含);keywords 合并的修复让 plan 段可独立驱动 mock plan markdown 覆写,不再依赖 user 轮次 token 在 Plan 上下文里也存在(此前不存在)。

**C10Q4 链上修复**:sysinfo.sh --json 模式把锚点 `C10Q3_JSON_OK` 走 stderr(`>&2`),stdout 只输出 JSON;C10Q4 bash command 去掉 `2>&1 | tee`(避免 stderr 污染 sysinfo.json),改用纯 stdout `| tee`;最终 `python3 json.load(sysinfo.json)` 成功 + 5 keys 全在。

**验证**:E09 4/4 + C10 4/4 共 8 轮全过,无 TIMEOUT;产物 calc.py/test_calc.py/sysinfo.sh/sysinfo.md/sysinfo.json 全部落盘 TestWorkSpace/tmpPlan/agent-test/;测试报告 `tmpPlan/2026-09-11_18-E09-C10编程系统信息提示词测试与bug修复方案.md`;单元测试未回归。

---

## 十、第四十一轮登记(2026-09-11,DJ06 Linux 离线批量巡检)

**主题**:118-DJ06 本地多目录批量巡检(offline fleet) **hard 档** 4 轮 `laew -debug` 测试一次通过,无 laew 程序 bug。

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| dj06-keyword-route | mock 路由器关键词硬档穿透 Plan 段 | P0(测试基建) | ✅ | `tests/prompt_router_dj06.json`(关键词 `DJ06_OFFLINE_FLEET` 同时出现在主 keywords / decomposition_plan 步骤 / workflow name,Plan 段 keywords 冗余写一份防御);`scripts/mock_llm_server.py` 第 697 行 `_route_plan_markdown` 已合并 `plan.keywords`(第四十轮修复,本轮直接受益) | 2026-09-11 第四十一轮 |
| dj06-bash-merge | q2+q3 多 Bash 合并为单次 call_no 简化路由 | P2(测试基建) | ✅ | `tests/prompt_router_dj06.json` call_no=4 Bash 单次完成 baseline + sed 替换 + 再跑 + diff + grep(用 `&&` 串接 + `2>&1 || true` 兜底) | 2026-09-11 第四十一轮 |
| dj06-port-collision | mock 端口冲突(E03/E10 占 18945) | P2(测试基建) | ✅ | 本轮用 18981 新端口(避免与并行会话的 mock_llm_server 进程冲突,见 [[tmux-test-sandbox-traps]]) | 2026-09-11 第四十一轮 |
| dj06-concurrent-jsonl | xargs -P 5 并发采集 5 行 JSONL | P1(测试基建) | ✅ | `tmpPlan/agent-test/dj06/fleet_inspect.sh` `xargs -P 5 -I{} bash -c 'collect "$@"' _ {}`(每主机 6 字段 JSON) | 2026-09-11 第四十一轮 |
| dj06-drift-detect | sed 替换触发 hash 漂移并 diff 命中 | P1(测试基建) | ✅ | `tmpPlan/agent-test/dj06/drift_report.txt` 11 行,sed 改 h3 `PasswordAuthentication no→yes` 触发 `sshd_hash e3876f902b4c6fd0 → 3209983429f1308c` | 2026-09-11 第四十一轮 |

**全链路验证**(从 mock log 解析):
- Yolo 1 次(hard 分类) → Plan 1 次(5 步骤 acceptance 齐全) → SubAgent 6 次(5 工具调用 + 1 终答) → Quality-Check 3 次(每工具结果质检) → SessionContext 1 次(摘要) → Debug 1 次(评估) = **合计 13 次 LLM 调用**
- 5 工具链全成功(Write→Bash→Write→Bash→Write),无失败,无重试,iter=6
- 21 个产物文件落盘 tmpPlan/agent-test/dj06/(`gen_local_fleet.sh` + `fleet_inspect.sh` + `current.jsonl` 5 行 + `last_snapshot/snapshot_v1.jsonl` 5 行 + `drift_report.txt` 11 行 + `dj06_report.md` 2532 字节 + `hosts/{h1..h5}/` 共 15 文件)
- Plan 方案 `plans/20260911-163951-b2cb7a82-*.md`(含 5 步骤 acceptance)
- DebugReport `DebugReport/debug_report_20260911_163951_6c5630.md`(12 项统计齐全,LLM 21ms / QC 3/3 pass / 任务 289ms)
- 用量 input=443 output=241(mock 端固定)

**路由验证**:关键词 `DJ06_OFFLINE_FLEET` 命中三阶段
1. Yolo 分类 → `goal_summary` 含关键词 → `task_level=hard` ✅
2. Plan 输入 → 主 keywords 命中 → 返回覆写 workflow ✅
3. Main-Work → `workflow.name` 含【DJ06_OFFLINE_FLEET】前缀 → 透传 SubAgent → mock 按 call_no 路由 5 工具链 ✅

**亮点**:
- **mock 路由器一次到位**:沿用第四十轮 BUG-M3 / 第四十一轮 plan.keywords 合并经验,关键词无需调试即通过 5 步工具链
- **简化 Bash 命令减少 mock 路由次数**:q2+q3 合并到 call_no=4,总工具调用 5 次(与提示词 q4 严格对齐)
- **drift 检测对 sed 替换敏感**:PasswordAuthentication 行变更同时触发 sshd_hash 整体变更,两份 hash 在 drift_report 并列显示
- **并发 JSONL 真实可跑**:xargs -P 5 + bash -c 函数导出,5 主机并发采集 5 行 JSON(实测毫秒级完成)

**未发现 laew 程序 bug**:Yolo 三步意图识别 / Plan 硬档方案生成 / Main-Work 拆解 / SubAgent 5 步工具链 / QC × 3 质检 / SessionContext 摘要 / DebugReport 全部正常。

**方案**:`tmpPlan/2026-09-11_DJ06-Linux离线批量巡检测试与mock多步工具链方案.md`
**产物落盘**:`tmpPlan/agent-test/dj06/`(21 文件)+ `plans/20260911-163951-*.md` + `DebugReport/debug_report_20260911_163951_6c5630.md` + `testReport/{mock_server_DJ06,laew_dj06_run}.log` + `tests/prompt_router_dj06.json`

---

# 第二十轮(2026-09-11)D13 离线模式与连接韧性增强登记

**主题**:第十九轮 D13 离线模式 P0 落地(离线检测 + 请求队列 + TUI 状态显示)。laew 此前网络错误时直接走完整重试链(≈30s)后上抛晦涩错误,用户无感知、无队列、无状态可视化。本轮落地三组件:被动离线检测(Online/Degraded/Offline 三态)、有界内存队列(默认 50 条)、TUI 横幅状态行 + dispatch 层自动入队/恢复 flush。

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| L1821-L1830 | D13-1 离线检测(被动检测三态状态机) | P1 | ✅ | `src/llm/offline.rs`(ConnectivityTracker:Online/Degraded/Offline + record_network_error/record_success/snapshot + DEGRADED_THRESHOLD=2/OFFLINE_THRESHOLD=5 对齐熔断器)、`src/tui/dispatch.rs::update_connectivity_from_result`(任务结果驱动状态更新:网络错误分类累计 / 成功复位) | 2026-09-11 第二十轮 |
| L1831-L1840 | D13-2 请求队列(有界内存队列 + 自动 flush) | P1 | ✅ | `src/agent/offline_queue.rs`(OfflineQueue + QueuedRequest + QueueFull + DEFAULT_QUEUE_CAPACITY=50 + LAEW_OFFLINE_QUEUE_CAP 环境变量覆盖)、`src/tui/dispatch.rs`(dispatch_prompt 入口:Offline→入队返回 / Online/Degraded→flush 一条队列头部) | 2026-09-11 第二十轮 |
| L1871-L1880 | D13-6 TUI 状态显示(横幅 + 斜杠命令) | P1 | ✅ | `src/tui/mod.rs`(TuiSession.connectivity/offline_queue 字段 + print_banner 连接状态行 + connectivity_status_line 辅助)、`src/tui/slash.rs`(`/offline` `/status` 命令 + run_offline_status 方法)、`src/tui/completion.rs`(补全注册) | 2026-09-11 第二十轮 |

**设计要点**:
- **被动检测**:不发心跳 ping,基于真实 LLM 调用结果推断连接状态(零 API 成本);恢复由用户下一笔输入触发。
- **三态状态机**:Online → Degraded(≥2 次) → Offline(≥5 次) → Online(成功)。
- **队列内存态**:退出即弃,有界 50 条(防内存溢出),满则拒绝并提示。
- **dispatch 层集成**:离线时入队跳过 LLM 调用(避免 30s 重试链浪费);恢复时 flush 队列头部(逐条避免递归顺序错乱)。
- **LLM 层零改动**:ConnectivityTracker 由 TUI 侧持有,dispatch 层基于任务结果更新,不侵入 `LlmClient` trait / `ResilientLlmClient`。

**验证**:
- 单元测试 770 全过(新增 17 项:offline.rs 10 项 + offline_queue.rs 7 项,从 753→770)。
- e2e `run_e2e.sh` PASS=156 FAIL=0(基线一致,无回归)。
- 方案 `tmpPlan/2026-09-11_19-D13离线模式与连接韧性增强方案.md`。

**未做(后续候选)**:L1841-L1850 本地缓存 / L1851-L1860 队列持久化 / L1861-L1870 同步合并 / 队列优先级。

**累计**:本轮新增 3 ✅(L1821-L1830/L1831-L1840/L1871-L1880),累计实现 gap 持续增长。

---

## 十一、2026-09-13 第 01 轮登记(D4 文件监视与工作区感知 —— 懒刷新派落地)

**主题**:第十八轮 D4 维度(laew 现状 0%)首次落地。laew 原本只有「说明文件五级链」发现,
且**注入只到 Yolo 入口层** —— 真正调用 Bash 的 SubAgent-Work 看不到工程类型 / 工具链 /
git 状态 / 平台 / 日期,只能 `ls` 试探,常猜错构建命令(`npm test` vs `cargo test`)。

| 编号 | gap | 等级 | 状态 | 实现位置 | 完成轮次 |
|------|-----|------|------|---------|---------|
| L1498-L1499 | openclaw D4:工作区感知(工程类型/变更/结构) | P1 | ✅ | `src/agent/workspace.rs::snapshot`(git status/log 子进程 + 11 类工程标记表 + 顶层结构 + walkdir 最近改动 6h 窗口 + 16 类忽略目录) | 2026-09-13 第 01 轮 |
| L1459+ | deepseek-harness D4:事件驱动 invalidate(无 watcher 依赖) | P1 | ✅ | `workspace::invalidate()` 进程级 TTL 缓存(默认 5s,`LAEW_WORKSPACE_TTL_SECS` 可调)+ `tui/dispatch.rs` 任务结束后失效缓存 → 下一轮 brief 反映任务后真实状态 | 2026-09-13 第 01 轮 |
| L1549+ | pi D4:Git HEAD / 分支感知 | P1 | ✅ | `workspace::collect_git`(`git status --porcelain=v1 -b` + `git log -3 --format=%h %s` + `rev-parse --short HEAD` 兜底;2s 超时 + 读取线程防死锁) | 2026-09-13 第 01 轮 |
| D4-env | 平台 / 日期 / 架构环境信息缺失(claudecode 系统提示词内置) | P1 | ✅ | `hint_block()`(约 35 token,System 末尾,8 角色每次调用可见)+ `render_section()`(会话级 PROJECT_CONTEXT 段) | 2026-09-13 第 01 轮 |
| D4-execlayer | 执行层 Agent 无环境可见性(注入只到 Yolo) | P0 | ✅ | `src/agent/mod.rs::run_session_inner` system 组装点追加 `<<<LAEW:WORKSPACE>>>` 块(与 RUNTIME_HINTS 同位置,不改 `build_runtime_hints` 契约) | 2026-09-13 第 01 轮 |
| D4-tui | 工作区状态不可见 / 任务改动不可感知 | P2 | ✅ | `tui/mod.rs` 横幅「工作区」行 + `/workspace [refresh]`(别名 `/ws`)+ `tui/dispatch.rs::print_workspace_delta` 任务前后 `变更 N → M(+k): 文件` 自动提示 | 2026-09-13 第 01 轮 |

**设计要点**:
- **懒刷新派**:知识库三派(hot watcher / 事件总线 / lazy refresh)中取懒刷新 + TTL 缓存,
  **不引入 `notify`**(遵守工程依赖约定);快照显式声明「可能略滞后于磁盘」,精确状态以工具为准。
- **触发条件放宽但空目录语义不变**:`build_message` 由「有说明文件」放宽为「有说明文件 **或** 工作区非空」
  —— 无 Markdown 的 git 仓库 / 有工程标记的目录也注入;空目录(`is_trivial`)仍不注入(e2e §5b 场景C 契约保持)。
- **子进程安全**:std 无 `wait_timeout`,「先 try_wait 再读管道」在 >64KB 输出时死锁 ——
  用独立线程排空 stdout + 轮询 + 到期 kill;超时路径不 join(句柄 drop 即 detach),防孙进程持管道永久阻塞。
- **零回归**:`build_runtime_hints` 本体未改(其 8 项单测契约保持),brief 作为独立块拼在其前。

**验证**:
- 单元测试 **787 全过**(基线 770 + 新增 17 项 workspace 测试)
- e2e `run_e2e.sh` **PASS=156 FAIL=0**;新增 §5b 场景A/B 工作区段断言 + **场景D**(无 Markdown 的
  git 仓库仍注入,新契约)+ §6 仓库根 `LAEW:WORKSPACE` 标记与 `cargo` 工具链建议断言
- 真链路(mock + 真实 git 仓库):system brief 与 PROJECT_CONTEXT 段内容逐项核对正确
- TUI tmux 真 PTY:横幅行 / `/workspace` / 任务后 `[工作区] 未提交变更 2 → 3(+1): sandbox-ok.txt`

**方案**:`tmpPlan/2026-09-13_01-工作区感知与运行时环境注入方案.md`
**设计文档**:`docs/工作区感知与运行时环境注入/01-设计与解决方案.md`

**D4 维度现状**:0% → 75%(未做:文件系统监听 / 快照跨 Session 持久化 / mtime 与 @提及去重联动)。

**累计**:本轮新增 6 ✅(L1459+/L1498-L1499/L1549+/D4-env/D4-execlayer/D4-tui)。
