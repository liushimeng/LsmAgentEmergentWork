# laew 已实现功能标记:Bash 输出落盘 (D17) + TODO 任务命令与持久化 (D19)

> 用途:防止后续「调研 → 实现」轮次重复处理同一功能。本文件只登记本轮已经进入
> `src/` 的功能,不登记仅讨论或规划过的候选项。

## 2026-09-22 第 112 轮:Bash 输出落盘 + /tasks 命令 + TODO 持久化

| 知识库出处 | gap | 状态 | 实现位置 | 说明 |
| --- | --- | --- | --- | --- |
| 第二十轮 D17 L2051 | Bash 输出无 spill 机制(30K 硬截断,LLM 永久丢失大输出) | ✅ 已实现 | `src/agent/tools/bash_spill.rs::maybe_spill` + `src/agent/tools/bash.rs` execute 后接 spill | 头部 5K + 路径 + 尾部 5K,LLM 用 Read 工具回读 |
| 第二十轮 D17 L2052 | spill 阈值不可配置 | ✅ 已实现 | `src/agent/tools/bash_spill.rs::threshold_chars` + `LAEW_BASH_SPILL_THRESHOLD` 环境变量 | 解析失败回退默认 30000 |
| 第二十轮 D17 L2053 | spill 失败无降级 | ✅ 已实现 | `src/agent/tools/bash_spill.rs::SpillOutcome::FailedFallback` | 自动回退到阈值截断 + `[spill failed: <reason>]` 警告 |
| 第二十轮 D17 L2054 | spill 路径无并发防冲突 | ✅ 已实现 | `src/agent/tools/bash_spill.rs::random_hex6` Splitmix64 混合 | pid + nanos + 原子计数器三重混合 |
| 第二十轮 D17 L2055 | spill 无 CJK 安全切片 | ✅ 已实现 | `src/agent/tools/bash_spill.rs::head_chars` / `tail_chars` | 按 `char_indices` 切而非按字节切 |
| 第二十轮 D17 L2056 | spill 文件未隔离 | ✅ 已实现 | `BashSpill/` 子目录 + `.gitignore` 登记 | 与 `CrashReport/` / `AuditTrail/` 同级 |
| 第二十轮 D19 L2171 | TODO 工具无列表命令 | ✅ 已实现 | `src/agent/todo_state.rs::render_table` + `src/tui/slash.rs::run_tasks` + `src/tui/completion.rs` 补全 + `src/tui/format.rs` 帮助 | 表格 4 列(id / status / priority / content),三别名 `tasks / todo / todos` |
| 第二十轮 D19 L2172 | TODO 状态未持久化到 session_memory | ✅ 已实现 | `src/agent/session_context.rs::append_todo_snapshot_to_summary` + `extract_todo_snapshot_from_summary` + `build_history_message` 自动提取 | 标记 `<<<LAEW:TODOS>>>` / `<<<LAEW:TODOS_END>>>`,跨 session Yolo 可看到上轮 TODO 进度 |
| 第二十轮 D19 L2173 | TODO 持久化易踩脏数据 | ✅ 已实现 | `src/agent/session_context.rs::strip_todo_snapshot_block` | 主摘要正文剥除 TODO 标记块,避免双展示 |

**不在范围(后续轮次可做)**:
- D17 剩余:L2057-L2110 回收(spill TTL/LRU)、事务原子、命名空间隔离 —— CLI 单用户场景优先级低
- D19 剩余:L2174+ 依赖关系(`depends_on`)、跨 session 独立 todo_history 表、Yolo 自动转 todo、TUI 进度条 widget

**验证**:单元测试 1419 全过(基线 1398 + 新增 21:bash_spill 14 + bash 集成 3 + todo_state render_table 3 + session_context 摘要含 TODO 1 + strip block 1 + 抽取 1);e2e 188 PASS(基线无回退)。

**方案**:`tmpPlan/2026-09-22_02-Bash输出落盘与TODO任务命令与持久化方案.md`
