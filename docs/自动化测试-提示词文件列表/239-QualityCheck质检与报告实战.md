# 239 QualityCheck 质检与报告实战

> 编号段 HZ01–HZ10 · 聚焦「laew Quality-Check Agent(质检层)」:质检结论 Verdict(Pass/Fail) / 质检报告 QualityReport{verdict, source, issues[], suggestion, retryable, evidence} / 质检触发时机(SubAgent-Work / Main-Work / Plan 单元完成后) / 质检形式(LLM 评估 + 静态规则) / 失败回流与重试 / 质检解析失败 fail-closed / 质检报告落盘 / 质检 metrics 统计 / 质检与 WorkFlow QualityGate 协同 / 质检与 SessionContext 协同
>
> 与现有维度互补说明:
> - `20-测试自动化与质量工程`(BF 维)是**外部测试工程**;本文件是**laew 内部质检层**。
> - `42-API设计艺术与错误语义`(BN 维)是**API 设计**;本文件是**质检设计**。
> - `38-代码审查与团队协作工程`(DJ 维)是**代码审查**;本文件是**Agent 产出审查**。
> - `133-自动化测试框架与测试工程实践`(EB 维)是**测试框架**;本文件是**Agent 质检**。
> - `50-软件架构模式与设计系统`(BK 维)是**架构模式**;本文件是**质检架构**。
>
> **本文件独特主题**:`Verdict` 枚举 Pass/Fail / `QualityReport{verdict, source: AgentRole, issues: Vec<String>, suggestion: String, retryable: bool, evidence: String}` / `QualityReport::pass(source)` / `QualityReport::fail(source, issues, suggestion, retryable)` / 质检触发时机(SubAgent-Work 完成 / Main-Work 完成 / Plan 完成) / 质检形式(LLM 评估 + 静态规则:空输出检查 / 失败措辞检查 / 期望关键词检查) / 失败回流与重试(retryable=true 时重试) / 质检解析失败 fail-closed(解析失败视为 Fail) / 质检报告落盘 SQLite / 质检 metrics 统计(总质检数 / Pass 数 / Fail 数 / 重试数) / 质检与 WorkFlow QualityGate 协同 / 质检与 SessionContext 协同。

---

### HZ01 质检结论基础结构

- **预期档位**: simple
- **考察维度**: Verdict 枚举 / QualityReport 结构 / 构造函数
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hz01_verdict.py`:质检结论模拟器(参考 `src/agent/quality.rs`):(a) `Verdict` 枚举:Pass/Fail;(b) `QualityReport{verdict, source: AgentRole, issues: Vec<String>, suggestion: String, retryable: bool, evidence: String}`;(c) `QualityReport::pass(source)` 构造 Pass 报告;(d) `QualityReport::fail(source, issues, suggestion, retryable)` 构造 Fail 报告;(e) `is_pass() -> bool` / `is_retryable() -> bool` 便捷方法。
  2. Bash:构造 1 条 Pass 报告 + 2 条 Fail 报告(1 条 retryable,1 条 non-retryable),断言:verdict 正确、issues 列表正确、retryable 标志正确。
  3. Read 源码 + Bash 断言:含 Verdict 枚举、含 QualityReport 结构、含构造函数。

### HZ02 静态规则质检

- **预期档位**: medium
- **考察维度**: 空输出检查 / 失败措辞检查 / 期望关键词检查
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hz02_static_rules.py`:静态规则质检模拟器:(a) `check_empty_output(output) -> Option<String>`:空输出返回 Some("输出为空");(b) `check_failure_indicators(output) -> Vec<String>`:检测「失败/错误/error/failed/无法」等措辞;(c) `check_expected_keywords(output, expected) -> Vec<String>`:检测缺失的期望关键词(分号分隔);(d) 综合 `static_check(output, expected) -> QualityReport`。
  2. Bash:跑 4 组 output(空/含失败措辞/缺关键词/全通过),断言:空→Fail+「输出为空」;含失败措辞→Fail+issues 列表;缺关键词→Fail+缺失列表;全通过→Pass。
  3. Read 源码 + Bash 断言:含空输出检查、含失败措辞、含关键词检查。

### HZ03 LLM 评估质检

- **预期档位**: medium
- **考察维度**: LLM 调用评估产出质量 / 解析 JSON 结论
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hz03_llm_eval.py`:LLM 评估模拟器:(a) `evaluate_with_llm(output, criteria) -> Result<QualityReport>`:模拟 LLM 调用返回 JSON `{verdict, issues[], suggestion}`;(b) JSON 解析失败 → fail-closed 返回 Fail;(c) verdict 字段非法 → fail-closed 返回 Fail;(d) 正常解析 → 返回对应 QualityReport;(e) LLM 调用失败 → 降级静态规则。
  2. Bash:跑 3 组(正常 Pass JSON / 正常 Fail JSON / 非法 JSON),断言:正常解析正确,非法 JSON → fail-closed Fail。
  3. Read 源码 + Bash 断言:含 LLM 调用模拟、含 JSON 解析、含 fail-closed、含降级。

### HZ04 质检触发时机

- **预期档位**: medium
- **考察维度**: SubAgent-Work 完成 / Main-Work 完成 / Plan 完成时触发
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hz04_trigger.py`:质检触发模拟器:(a) `TriggerPoint` 枚举:SubAgentComplete / MainWorkComplete / PlanComplete;(b) `should_trigger(trigger_point) -> bool`:每个触发点都触发质检;(c) `run_quality_check(trigger_point, output, expected) -> QualityReport`;(d) 质检耗时记录(metrics)。
  2. Bash:模拟 3 个触发点各产出 1 条结果,断言:每个触发点都触发质检,报告 source 字段正确(SubAgentWork/MainWork/Plan)。
  3. Read 源码 + Bash 断言:含触发点枚举、含触发逻辑、含 source 字段。

### HZ05 失败回流与重试

- **预期档位**: medium
- **考察维度**: retryable=true 时重试 / retryable=false 时直接失败
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hz05_retry.py`:失败回流模拟器:(a) QualityReport.retryable=true → 重试(SubAgent 重新执行);(b) QualityReport.retryable=false → 直接失败回流 Yolo;(c) 重试次数上限 3 次,超过不再重试;(d) 每次重试后重新质检。
  2. Bash:跑 3 条 Fail 报告(2 条 retryable,1 条 non-retryable),断言:retryable 触发重试,non-retryable 直接失败,重试 3 次后不再重试。
  3. Read 源码 + Bash 断言:含 retryable 判断、含重试逻辑、含重试上限。

### HZ06 质检解析失败 fail-closed

- **预期档位**: medium
- **考察维度**: LLM 返回非法 JSON / 字段缺失 → fail-closed 视为 Fail
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hz06_fail_closed.py`:fail-closed 模拟器:(a) LLM 返回非法 JSON → fail-closed 返回 Fail+「质检解析失败,按 Fail 处理」;(b) verdict 字段缺失 → fail-closed Fail;(c) verdict 字段非法(非 Pass/Fail) → fail-closed Fail;(d) 正常 Pass JSON → Pass;(e) 正常 Fail JSON → Fail。
  2. Bash:跑 5 组(非法 JSON / 缺 verdict / 非法 verdict / 正常 Pass / 正常 Fail),断言:前 3 组 fail-closed Fail,后 2 组正常解析。
  3. Read 源码 + Bash 断言:含 fail-closed 默认 Fail、含字段校验、含错误提示。

### HZ07 质检报告落盘

- **预期档位**: medium
- **考察维度**: QualityReport 持久化到 SQLite / 查询历史
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hz07_persistence.py`:质检持久化模拟器:(a) `save_report(session_id, report) -> Result<()>` 写入 SQLite `quality_reports` 表;(b) `load_reports(session_id) -> Vec<QualityReport>` 查询历史;(c) 表结构:`id, session_id, verdict, source, issues(JSON), suggestion, retryable, evidence, created_at`;(d) 按 created_at 倒序查询。
  2. Bash:写入 5 条报告(3 Pass + 2 Fail) → 查询 → 断言 5 条全在,按时间倒序,字段完整。
  3. Read 源码 + Bash 断言:含写入、含查询、含字段完整、含倒序。

### HZ08 质检 metrics 统计

- **预期档位**: simple
- **考察维度**: 总质检数 / Pass 数 / Fail 数 / 重试数 / 通过率
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hz08_metrics.py`:质检统计模拟器:(a) `QualityMetrics{total, pass_count, fail_count, retry_count, pass_rate}`;(b) `record_report(report)` 更新统计;(c) `get_metrics() -> QualityMetrics` 查询;(d) `pass_rate = pass_count / total`。
  2. Bash:写入 10 条报告(7 Pass + 3 Fail,其中 2 条 Fail 触发重试),断言:total=10, pass=7, fail=3, retry=2, pass_rate=0.7。
  3. Read 源码 + Bash 断言:含统计更新、含查询、含通过率计算。

### HZ09 质检与 WorkFlow QualityGate 协同

- **预期档位**: hard
- **考察维度**: QualityGate 调用 Quality-Check Agent / 四级门禁
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hz09_gate_integration.py`:协同模拟器:(a) `QualityGate` 四级:Task/Squad/Phase/Goal;(b) 每级调用 Quality-Check Agent 执行质检;(c) QualityGate 失败 → 按 QualityReport.retryable 决定是否重试;(d) QualityGate 通过 → 继续下一 Phase;(e) 模拟 5 个 Phase 各跑 1 次 QualityGate,其中 Phase 3 触发 Fail + retryable → 重试成功。
  2. Bash:跑上述场景,断言:Phase 1/2/4/5 一次通过,Phase 3 重试后通过,全部 Phase 完成。
  3. Read 源码 + Bash 断言:含四级门禁、含质检调用、含重试逻辑。

### HZ10 质检与 SessionContext 协同

- **预期档位**: medium
- **考察维度**: 质检结论写入 SessionContext 摘要 / 下次任务参考
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hz10_session_ctx.py`:协同模拟器:(a) 任务完成后 SessionContext 生成摘要;(b) 摘要包含:任务目标 / 执行结果 / 质检结论(Pass/Fail 数)/ 重试次数 / 总耗时;(c) 写入 `session_memory` 表;(d) 下次任务时 Yolo 注入最近 3 条历史摘要(含质检结论)。
  2. Bash:跑 3 个任务 → 每个任务生成摘要(含质检结论) → 第 4 个任务注入最近 3 条摘要,断言:摘要含质检结论,第 4 个任务 system prompt 含 3 条历史摘要。
  3. Read 源码 + Bash 断言:含摘要生成、含质检结论、含历史注入。
