# 2026-09-09_07 TUI Debug 日志噪音 + Provider 子屏端点溢出 报告

> 测试环境：根目录 `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork`，工作目录 `TestWorkSpace/`，
> 跑 2 条 simple 档位多轮提示词（A04 TCP 三次握手 / A06 数据库 ACID）+ 1 次 TUI 多轮对话 + 1 次
> `/provider list` 子屏验证 + 1 次 `-debug` 跑 A04/A06。
> Mock LLM：`scripts/mock_llm_server.py` 起在 127.0.0.1:18905（`claude-ovf` provider）。
> 关联提交：本次会话的第 07 轮。

---

## 一、测试用例

| 编号 | 提示词 | 预期档位 | 实际档位 | Debug 报告 |
|------|--------|----------|----------|-----------|
| A04_q1 | 解释一下 TCP 三次握手的过程。 | simple | ✅ simple | `DebugReport/debug_report_20260909_143803_3e6ec1.md` |
| A06_q1 | 数据库事务的 ACID 分别指什么？ | simple | ✅ simple | `DebugReport/debug_report_20260909_143814_ccf487.md` |

两个用例均走通完整链路：Yolo → SubAgent → Quality-Check → SessionContext，
Yolo 分类命中预期，QC pass，SessionContext 摘要写入。
两个调试报告均生成在 `DebugReport/` 根目录（已 .gitignore，不入库）。

---

## 二、发现的问题

### P0-1 TUI 多轮对话时 INFO 日志直接喷到屏幕（视觉混乱 + 抖动）

**复现步骤：**
```bash
# 1) 启动 mock
python3 scripts/mock_llm_server.py 18905 /tmp/A04_mock.jsonl &
# 2) TUI 启动后输入真实问题(非 mock 同样会出现)
tmux new-session -d -s tui -x 120 -y 36 "./laew"
# 输入"你好"回车
```

**当前实际输出**（TUI 屏内）：
```
  [orchestrator 调度中... Ctrl-C 取消]
2026-09-09T06:39:24.477203Z  INFO 已注入项目上下文(首次处理) source="README.md"
2026-09-09T06:39:24.477469Z  INFO agent step iteration=0
2026-09-09T06:39:24.478673Z  INFO agent finished with text answer
2026-09-09T06:39:24.485707Z  INFO agent step iteration=0
2026-09-09T06:39:24.486935Z  INFO agent finished with text answer
... (5 轮 agent 全部"瞬间"完成)
```

**问题分析：**
1. `tracing-subscriber` 默认会把 INFO 级日志打到 stdout，TUI/REPL 没有过滤。
2. `agent step iteration=0` 在多 Agent 编排下没有任何区分度（Yolo/Main/Sub/QC/Session 都从 0 开始），徒增噪音。
3. mock LLM 太快（每轮 1 ms），用户根本看不到 `[orchestrator 调度中...]` 的"工作态"——视觉上是「瞬间全部完成」。
4. **真实 LLM 场景**每条 INFO 行会被交错到 SSE 流式 token 输出中，导致屏幕闪烁、对话内容与日志串行错位。

**期望行为：**
- TUI 模式下 INFO 日志**不直接打到屏幕**，要么写到 DebugReport（-debug 模式）或重定向到 stderr（标准做法）。
- REPL 在 orchestrator 调度中**维持一个 loading 指示**（如 `▏▎▍▌▋▊▉█` 旋转字符 + 步骤名），而不是「瞬间打印然后立刻消失」。
- `agent step iteration=0` 这类低信息量日志应当降到 DEBUG 级。

---

### P0-2 Debug Agent 评估失败时报告无降级结构

**代码位置：** `src/agent/debug.rs:528-531`
```rust
let evaluation = match DebugRunner::new(llm).evaluate(&trace, &stats).await {
    Ok(text) => text,
    Err(e) => format!("(Debug Agent 评估失败: {e})"),
};
```

**问题分析：**
- 当 Debug Agent 自身调用失败（网络异常、API key 失效、mock 未启动等）时，整个「二、Debug Agent 评估」节就被退化成一行括号文本，**完全没有章节结构**。
- 用户/研发看不到 P0/P1/P2 分级，也无法判断「是真的没事」还是「Debug Agent 本身坏了」。
- 当前报告骨架（统计总览 / Debug Agent 评估 / 原始 Trace 附录）只有 3 个一级章节，但 Debug Agent 的 prompt 明确要求它输出 4 个二级章节（任务评估 / 质量报告 / 问题报告 / 优化建议），评估失败时这 4 个章节全部丢失。

**期望行为：**
- 评估失败时报告仍按骨架生成（统计总览、原始 trace 仍是真实数据），并且在「二、Debug Agent 评估」节内显式声明：
  - 失败原因（e.g. `NetworkError` / `AuthError`）
  - 兜底分析（基于 trace 自检得出 P0/P1 问题列表）
  - 用户可见的红色 ⚠️ 提示
- 报告头加一行 `⚠️ Debug Agent 评估失败 / 已降级到自检模式` 醒目提示。

---

### P1-1 Provider 子屏端点 URL 过长无截断 / 缩进

**复现步骤：**
```bash
tmux new-session -d -s tui "./laew"
# 输入 "/provider list" 回车 (TUI alternate screen)
```

**当前渲染**（子屏内）：
```
│ end_point:          http://127.0.0.1:29000/Anthropic                                                                              │
```

**问题分析：**
- 端点 URL 紧贴左右 `│` 框，无 ellipsis 或智能换行。
- 未来接入真实 endpoint（如 `https://api.anthropic.com/v1/messages` 或本地代理路径更长）会顶到边框。
- `context_max_size: 800000 (800K)` 这一行**没有视觉对齐**——它是 ProviderForm 表单外的额外字段，但与上面 5 个 Tab 字段挤在一列。

**期望行为：**
- 端点 URL 超过 N 字符时折行（按 `/` 切分）或省略中段（`http://127.0.0.1:29000/An…`）。
- `context_max_size` 行改为右对齐 value 列或者单独成行（与 5 个 Tab 字段视觉分层）。

---

### P2-1 `-f` 单轮模式不会显示用量估算前缀

**复现：** `laew -f prompt.md`

**当前输出末尾：**
```
[laew] 用量: input=30  output=20
```

**问题分析：**
- 用户看不到总耗时（41 ms）和 cache_read / cache_creation 细分。
- Debug 报告有完整四指标（input/output/cache_read/cache_creation），但终端只显示两个。

**期望行为：**
- 同步终端输出也展示 `cache_read=N` / `cache_creation=N` / `elapsed=XXms`，方便无 Debug 报告时也能判断缓存命中率。

---

## 三、本轮修复范围（仅 P0-1 / P0-2 / P1-1）

### F-007-1 TUI 屏蔽 stdout INFO 日志 + 编排 loading 动画
- `src/tui/mod.rs::run`: 启动 `tracing-subscriber` 时过滤到 WARN 级（只在非 TUI 模式显示 INFO）。
- 编排中增加 `orchestrator` 的步骤级事件总线 (`tokio::sync::mpsc`)，TUI 收到事件后渲染 `▏→ SubAgent wf-1 → ...` 进度行。
- `agent step iteration=` 日志降级到 DEBUG。

### F-007-2 Debug Agent 评估失败降级到自检骨架
- `src/agent/debug.rs`: 评估失败时填充「⚠️ Debug Agent 评估失败」+ 兜底 4 章节（基于 trace 自检：QC 失败/工具调用错误率/截断续接次数等指标归类到 P0/P1/P2）。
- 报告头加一行降级提示横幅。

### F-007-3 ProviderList 端点 URL 智能折行 / 截断
- `src/tui/screen/provider_list.rs`: 对 `end_point` 渲染前 `truncate_with_ellipsis(url, max_width=58)`，按 `/` 切分保留前 1 段 + 中段省略 + 末段。
- `context_max_size` 行单独 label 与 value 分行展示。

---

## 四、待办（不本轮处理）

- P2-1：`-f` 单轮模式用量扩展（低优，等下轮整体重设计时一并改）。

---

## 五、测试用例标记

- `docs/多轮对话问题知识库/01-问题知识库-第01-50问.md` 中 A04 / A06 用作本轮验证 → 标记状态「✅ 2026-09-09_07 已跑测」。
- A02/A03/B09/C02/C09/D01 此前已跑测（git status commit `42e9c8e`），保持标记。
