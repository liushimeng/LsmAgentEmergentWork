# 第 58 轮方案 — WindowUse 微信任务修复 + TUI 详情强化 + axuielement

> 2026-09-16 第 58 轮

## 任务背景

用户跑任务 `帮我打开微信软件,找到赵玲玲,给她发一个消息,消息内容要充满 AI 的味道`,TUI 输出 `[stage] wf-1.step WindowUse 执行中…` + `[stage] wf-1.step QC:❌ 未通过`,**实际未打开微信、未发任何消息**。失败链路显示 `tool_calls=0`(LLM 在 WindowUse 单元里**完全没发出工具调用**)+ `MaxIterationsExceeded`。

第 57 轮已修复 WindowUse 委派错位(WindowFind/WindowScreenshot 加入工具集,delegate_to 自动纠正)+ TUI 阶段耗时统计(stage_durations / retry_log / layer_log / 工具明细)。但真机任务仍失败,根本原因不止 57 轮已覆盖范围。

## 关键根因(调研已确认)

1. **【最关键】Agent 循环对"LLM 只回纯文本不调工具"无兜底** — `src/agent/mod.rs:291-358` 的 `!completion.has_tool_calls() → return finalize_with_max_tokens` 直接把"我先查窗口"纯文本当成功返回;`no_text_converge` 只对"有 tool_use 但 text 为空"计数,不会触发短路
2. **macOS WeChat NSWindow title 为空** — `CGWindowListCopyWindowInfo` 拿不到 `kCGWindowName`(只能拿到 `kCGWindowOwnerName=WeChat`),WindowFind 返回 `title=""` 让 LLM 放弃后续 WindowAction
3. **Runner 作业规范与 system_prompt 顺序不一致** — `window_use.rs:129` 写"先 WindowList",system_prompt L794 写"WindowList / WindowFind"双引导
4. **Failed 分支 TUI 缺详情** — `dispatch.rs:335-376` 只 print QC reason,**不**打印 stage_durations / trace 工具明细 / early_terminate_reason / failure_signals
5. **`format.rs:113-116` 运算符优先级 bug** — `len>=2 || (any_parallel && non_empty)` 空 layer_log 永不入内
6. **Session ID 多轮已就位** — `window_state` 表 + DAO 已完整,WindowUseRunner 已按 session_id 加载/保存
7. **axuielement 未引入** — macOS 仅 `core-foundation` 0.9 手写 FFI,700 行,5 个 `extern "C"` 块

## 实施分组(P0 / P1 / P2)

### P0 — 必修(微信任务根因 + Failed 路径详情)

| # | file_path:line | 动作 |
|---|---|---|
| P0-A | `src/agent/mod.rs:run_session_inner` | 新增 `should_nudge_window_ops()` helper + `WINDOW_OPS_NUDGE_TEXT`;在 `!has_tool_calls()` 分支首部插入 nudge,iter==1 且 profile.tools 含 WindowList 时注入 context 续跑 |
| P0-B | `src/agent/window_use.rs:run_unit_inner` | 出口兜底:trace.tool_calls == 0 && !looks_like_window_ops_action(&text) → 强制 failed + early_terminate_reason |
| P0-C | `src/agent/orchestrator.rs:OrchestrationOutcome::Failed` | 加 `last_trace / stage_durations / retry_log / wallclock_ms` 字段(`#[serde(default)]`) |
| P0-D | `src/tui/dispatch.rs:Failed 分支` | 复用 `format_task_result` 渲染,失败原因拼接 trace.early_terminate_reason |
| P0-E | `src/tui/format.rs:113-116` | 修复运算符优先级 bug |
| P0-F | `tmpPlan/2026-09-16_03-...md` | 本方案文档落档 |

### P1 — 应做(Session ID 多轮强化 + LLM 引导对齐)

| # | file_path:line | 动作 |
|---|---|---|
| P1-A | `src/agent/window_use.rs:107-133` | 改用 `build_window_state_message`(带 WINDOW_STATE_MARKER)注入 sub_session 第一条 user 消息 |
| P1-B | `src/agent/tools/window.rs:WindowFind` | title="" 时输出 `note` 字段解释 WeChat NSWindow 已知行为 |
| P1-C | `src/agent/system_prompt/mod.rs` | 强化"先 WindowList/WindowFind"统一引导 + WeChat title 为空说明 |

### P2 — 锦上添花(axuielement)

| # | file_path:line | 动作 |
|---|---|---|
| P2-A | `Cargo.toml:67-68` | 加 `axuielement = "0.9"` |
| P2-B | `src/agent/window/macos_axui.rs` | 新建(~350 行) |
| P2-C | `src/agent/window/macos.rs` → `macos_legacy.rs` | 改名留档 |
| P2-D | `src/agent/window/mod.rs:current_driver` | `LAEW_MACOS_DRIVER=legacy` 切换 |

## 单元测试断言(9 个新增)

1. `nudge_for_window_ops_triggers_on_first_iter_window_use` — WindowUse 第 1 轮触发;SubAgent 第 1 轮不触发;WindowUse 第 2 轮不触发
2. `runner_flags_zero_tool_calls_as_failed` — 0 tool_calls + "我先查窗口" → failed=true
3. `runner_passes_action_keyword_text_as_success` — "已点击发送" → failed=false
4. `failed_outcome_carries_trace_and_durations` — 重试超限 → last_trace=Some,stage_durations.len()>=1,wallclock_ms>0
5. `layer_log_print_trigger_corrected` — 1 层 parallel=true → 打印;空 layer_log → 不打印
6. `failed_header_includes_wallclock_and_reason` — Failed 输出含 `总耗时 Xs` + `reason=...`
7. `state_injected_as_system_message_with_marker` — state 注入第一条 user 消息含 `WINDOW_STATE_MARKER_START`
8. `window_find_adds_note_for_empty_title` — title="" + matched_field=process → JSON 含 `note`
9. `window_use_base_prompt_mentions_wechat_empty_title` — prompt 含 "macOS 部分应用 title 为空是正常" 表述

## 完成判定(DoD)

- [ ] `cargo test` 全绿(新增 9 测试)
- [ ] `cargo build --release` 无 warning
- [ ] `./rebuild_restart_app.sh` 完成
- [ ] `bash testReport/run_e2e.sh` 通过
- [ ] 真机微信任务:trace.tool_calls > 0
- [ ] 失败场景 TUI 能定位失败模式
- [ ] git commit + push(中文)
