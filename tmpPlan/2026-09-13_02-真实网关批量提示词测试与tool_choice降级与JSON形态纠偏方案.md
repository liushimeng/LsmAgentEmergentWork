# 真实网关批量提示词测试与修复方案(2026-09-13 第 50 轮)

> 测试执行环境:Linux 本机,真实网关 `realGW-D06AI09`(provider 16,liusm191-server-model,loopback + SSRF 放行)。
> 测试驱动:`TestWorkSpace/batch/driver.py`(tmux 多会话并行,每条提示词独立工作目录,4 轮对话,空闲判定完成)。
> 测试范围:30 条 bash/编程向未测试提示词 —— 文件 03(C01-C09,Linux 系统管理)、
> 文件 09(I01-I10,SQLite 数据库)、文件 10(J04-J10,网络协议)、文件 94(CL01/05/08/10,套接字编程)。

## 一、发现问题清单(按严重度)

### P0-1 forced tool_choice 与 thinking 互斥,每次结构化调用都先吃一次 400

**现象**(logs/laew-tui.log,高频复现):
```
WARN forced tool_choice 被 Provider 拒绝,自动降级为 auto 重试 tool="submit_task_classification"
  error=LLM HTTP 错误(status=400): tool_choice 'specified' is incompatible with thinking enabled
WARN forced tool_choice 被 Provider 拒绝,自动降级为 auto 重试 tool="submit_quality_report" (同上)
```
- Yolo 分类与 Quality-Check 每次调用都先发 forced `tool_choice` → 网关(thinking 常开)返回
  400 → resilient 降级 auto 重发。**每个结构化调用双倍延迟与 token**(Yolo+QC 每任务至少 2 次)。
- 连锁反应:降级 auto 后模型**可能不调用 emit 工具**而自由文本 → 「未找到合法的 JSON 分类结果」
  → Yolo 降级 simple(本轮实测 30 条中 c03/c04 多轮命中降级路径)。

**根因**:Anthropic 协议中 forced tool_choice 与 extended thinking 互斥;本网关模型服务端
常开 thinking,laew 每次调用都重新尝试 forced,拒绝是**确定性**的,旧逻辑却每次都重新踩。

**修复**(F1,`src/llm/resilient.rs`):拒绝记忆化 —— Provider 实例首次命中
`looks_like_tool_choice_rejection` 后置 `forced_tool_rejected` 标志,后续带 forced 的请求
直接以 auto 发出,省掉注定 400 的往返。客户端实例与 Provider 一一对应,天然按 Provider 隔离。

### P0-2 JSON 形态漂移:语法合法但字段类型不匹配,修复链全部漏接

**现象**(logs/laew-tui.log):
```
WARN Yolo 分类解析失败,降级为 simple: invalid type: sequence, expected a string at line 3 column 4;
     自动修复后仍失败: ...(purpose 等字符串字段被模型给成数组)
WARN Quality 报告解析失败,fail-closed:返回 Verdict::Fail 触发回流
     error=invalid type: string "[\"...\", \"...\"]", expected a sequence
     (issues 字段被双重编码成 JSON 字符串)
```
- Tier-1(语法修复)/Tier-2(截断补全)对「语法合法但形态不对」无能为力;
- 触发 Yolo 降级(simple 直答)与 QC fail-closed(无谓回流重试,一轮 QC 拆成两轮);
- 与知识库既有教训一致(「mock 应答天然合法,真实 LLM 才暴露形态漂移,须配宽松反序列化」);
  main_work.rs 的 WorkFlowSpec 早已为 steps/branches/acceptance 配了 lenient 反序列化,
  **Yolo 与 Quality 的结构体是遗留盲区**。

**修复**(F2,`src/agent/json_repair.rs`):新增 **Tier-3 类型纠偏** —— 直解失败后把文本
parse 成 `Value`,按「已知字段名 → 期望形态」保守词表(STRING_FIELDS / STRING_VEC_FIELDS /
workflows 双重编码解包)整型再反序列化。只跑在失败路径,不发明缺失内容,Quality 的
fail-closed 语义不变;同时修正 lenient 入口的 Tier-2 门槛(类型错误文案会掩盖截断特征,
改为「repaired 解析不成 Value」判定)。

### P1-1 Yolo 降级后 goal_summary 是占位符,SubAgent 自行发挥产物路径翻倍

**现象**(c03 实测):Yolo 解析失败降级后,`goal_summary="(解析失败,已降级)"`,
SubAgent 拿不到真实任务,凭模糊上下文自由发挥 —— 工作目录出现
`tmpPlan/agent-test/tmpPlan/agent-test/backups/` 翻倍嵌套产物,后续轮次路径混乱。

**修复**(F3,`src/agent/yolo.rs`):降级分类统一走 `degraded_classification(context)` ——
goal_summary 取最近一条 user 消息原文(折叠空白 + 300 字符截断),占位符挪到 purpose;
无 user 文本才回落占位符。SubAgent 在降级场景下依然拿到完整任务描述。

### P2-1 观察项(不修码,记录在案)

1. **Main-Work 拆解偶发一轮重试**(「第 1 轮重试当前档位…」):真实 LLM 拆解 JSON 首解
   失败触发重试,F2 上线后其中形态类失败应被纠偏消化,重试率预期下降;
2. **Yolo auto 兜底后尝试越权工具**:「tool failed tool=Bash error=工具不存在: Bash」
   —— Yolo 仅持 Read,auto 模式下模型偶发尝试 Bash,循环层正确拒绝(报错回填),
   无害但说明系统提示词的工具边界说明可再强化;
3. **simple 任务摘要链路含 Plan**:SessionContext 摘要(LLM 生成)在 degraded simple
   任务上写出「Agent 链路: Yolo → Plan → WorkFlow → …」,属提示词层小噪声,
   F3 落地后降级摘要的输入信息更完整,暂不改码;
4. **本轮实测每轮延迟 40-300s**(真实网关 + thinking 常开),F1 上线后每任务省
   2-4 次注定失败的 400 往返,整体延迟预期下降 10-30%。

## 二、修复实现清单

| # | 文件 | 改动 | 测试 |
|---|------|------|------|
| F1 | `src/llm/resilient.rs` | `ResilientLlmClient` 新增 `forced_tool_rejected: AtomicBool`;complete() 前置短路 + 拒绝命中时置位 | `forced_tool_rejection_memoized_for_subsequent_calls` |
| F2 | `src/agent/json_repair.rs` | 新增 Tier-3:`coerce_known_shapes` / `coerce_to_string` / `coerce_to_string_vec` / `deserialize_coerced`;try_parse / try_parse_lenient 失败路径接入;lenient Tier-2 门槛改为「Value 解析失败」 | `tier3_sequence_coerced_to_string_field` / `tier3_double_encoded_string_vec_field` / `tier3_plain_string_becomes_single_element_vec` / `tier3_double_encoded_workflows_array_unwrapped` / `tier3_does_not_invent_missing_content` / `tier3_lenient_truncation_still_works_with_coercion` |
| F3 | `src/agent/yolo.rs` | 新增 `degraded_goal_from_context` / `degraded_classification`,两处降级分支统一收敛 | `degraded_goal_keeps_raw_user_prompt` / `degraded_goal_falls_back_to_placeholder_without_user_text` / `degraded_goal_truncates_long_prompt` |
| F4 | `src/agent/system_prompt/mod.rs` | SubAgent 提示词加「路径保真」规则(逐字使用用户/上游指定路径) | system_prompt 单测 7 全过 |
| F5 | `src/agent/system_prompt/mod.rs` | Main-Work 提示词加「路径保真」规则(steps/acceptance 逐字保留用户路径,不得改写为绝对路径/省略层级) | 同上 |

**回归基线**:`cargo test --lib` 799 全过(修复后);无既有测试改动。

## 三、验证计划与执行结果

1. ✅ `./rebuild_restart_app.sh` 重建 release 二进制(含 F1-F4);
2. ✅ `bash testReport/run_e2e.sh` 全量回归:**PASS=156 FAIL=0**(零回归);
3. ✅ 批量实测(真实网关,TUI 4 轮/条,独立工作目录,并行 4 会话):

| 批次 | 结果 |
|------|------|
| C01 磁盘排查 | ✅ 4/4 轮,断言 PASS=5(首跑旧二进制曾被 QC 诚实拦截,见发现 5) |
| C02 进程监控 | ✅ 4/4,PASS=4(轮均 76-108s,F1 后延迟大降) |
| C03 定时备份 | ✅ 4/4,PASS=4(保留恰好 7 份) |
| C04 PATH 探测 | ✅ 4/4,PASS=3(含 1 次 QC 回流后通过) |
| I01 索引实测 | ⚠️ 4/4 轮,db+5 万行正确但索引/报告跨轮路径漂移(见发现 6) |
| I02 覆盖索引 | ✅ 4/4,PASS=3 |
| I03 WAL 并发 | ✅ 4/4,PASS=3 WARN=1(journal_mode 停在 delete 属实验后预期) |
| I04 PRAGMA+FTS5 | ⚠️ 4/4 轮但产物碎片化(3 个 db,notes_fts 缺失,见发现 6) |
| I05 迁移 hard | ✅ 重跑 4/4,PASS=5(首跑 r3 拆解长等待 507s+) |
| I06 DuckDB 对比 | ✅ 4/4,PASS=4(duckdb 不可用正确降级 baseline) |
| I07 备份校验 | ✅ 4/4,PASS=4(integrity=ok) |
| I08 ORM 决策 | ✅ 4/4,PASS=5 |
| I09 权限建模 | ✅ 4/4,PASS=4(r4 669s) |
| I10 EXPLAIN hard | ✅ 4/4,PASS=4(50 万行实库走 SEARCH USING INDEX;r3/r4 1325s/1281s) |
| J04 WebSocket | ✅ 4/4,PASS=3 WARN=1(断言时服务进程尚在退出,复检无残留) |

4. ✅ 通过的提示词已在知识库条目补 `**测试状态**: ✅ 已测试` 标记(防重复处理);
   J05-J10 / CL01-CL10 后续批次滚动推进。

### 3b. 全量最终战果(30/30 条,含修复版重跑)

| 提示词 | 结果 | 备注 |
|--------|------|------|
| C01 磁盘排查 | ✅ | F1-F3 版重跑通过(首跑被 QC 诚实拦截) |
| C02-C04 | ✅ | 一次通过 |
| I01 索引实测 | ✅ | F5 版通过(路径漂移+QC 假想缺陷两轮迭代) |
| I02/I03 | ✅ | 一次通过 |
| I04 PRAGMA+FTS5 | 🔄 | 4 轮完成,PRAGMA/summary 过;FTS 表名自造(bench_*),留待回归 |
| I05 迁移(hard) | ✅ | 重跑通过(首跑拆解长等待) |
| I06-I10 | ✅ | 一次通过(I10 实库 50 万行走 SEARCH USING INDEX) |
| J04-J10 | ✅ | 一次通过(J04/J05/J09/J10 各 1 条良性 WARN) |
| CL01/CL05/CL08/CL10 | ✅ | 一次通过 |

**汇总:29/30 全绿,1 条 🔄(指令遵循回归项);修复版二进制单轮延迟 -65%;
Yolo 降级 0 次;e2e 156/156;单元测试 799+ 全过。**

### 修复效果量化
- forced tool_choice 400 往返:每结构化调用 1 次 → 进程内 0 次;
- 单轮延迟:C02 首跑 287s → 修复后轮均 76-108s(约 -65%);
- Yolo 降级:修复前 c03/c04 多轮命中降级路径 → 修复后批次内 0 次降级;
- e2e 回归 156/156 全过,`cargo test --lib` 799+ 全过(含 10 个新单测)。

### 能力发现(非 laew 代码 bug,记录给提示词/产品迭代)
5. **Main-Work 自造前置依赖**:C01 首跑 r3 计划凭空引入 `du_output.txt` 文件依赖
   (用户脚本只要求读 du 输出),SubAgent 零执行 → QC fail-closed 诚实拦截并给出
   可行建议(回流机制工作正常);修复版二进制重跑通过。
6. **执行层路径漂移(根因链完整定位)**:I01/I04 多轮各自在不同 db 上工作。
   F4 版重跑证实漂移根因在 **Main-Work 改写用户路径**:i04 r2 的 SubAgent 汇报
   明确连接 `…/run_i04/i04.db`(工作区根,用户指定 `tmpPlan/agent-test/i04.db`)
   —— Main-Work 拆解时把相对路径改写为根目录绝对路径,SubAgent「忠实执行错误
   上游」,QC 无原始路径对照而误判通过。F4(SubAgent 路径保真)+ F5(Main-Work
   路径保真,本次新增)双端约束;长效方案可让 Orchestrator 把用户原始路径清单
   作为对照锚点传给 QC。
7. **拆解长等待**:thinking 网关下 Main-Work 拆解偶发 273-507s+(含重试),
   TUI 有「⚠ 等待超过 1 分钟」提示;P2 建议:拆解超时(如 8 分钟)自动降级为
   单 WorkFlow 直委派(目标=用户原文),避免整轮卡死。
8. **轮次耗时分布**:simple 档 40-140s;medium 80-400s;hard(多流程)可达
   1300s+;并行 4-5 会话未见 429/熔断。
9. **QC 对计划的假想性缺陷过度严苛(P1 建议)**:i01 F4 版重跑中 QC 以
   「wf-1 未验证 sqlite3 CLI 可用性…若系统仅有 Python 模块将失败」(假想性
   前提,实际 CLI 存在)与「验收标准不够机器可验证」反复拒绝计划,循环烧掉
   2 万 output token 后由「连续相同失败短路」诚实失败。建议 QC 判据区分
   「实际验证失败」与「假想性健壮性担忧」——后者降级为 warning 不阻断。
10. **测试基建自反思**:批量驱动器完成检测经三版迭代(v1 用量计数被滚屏破坏 /
    v2 全面板 busy 判定被会话滚动内容钉死 / v3 底部 10 行 busy+用量增量双条件);
    断言脚本两次踩 pgrep 自匹配假阳性(`[x]` 括号技巧修复)与 bash 双引号
    层级吞噬(python 走 argv 传 SQL 修复)——与记忆 tmux-test-sandbox-traps
    既有教训同源,已回填记忆。


## 四、遗留与后续

- forced tool_choice 记忆化目前进程内有效;跨进程持久化(如 providers 表加列)收益低
  (每进程多付一次 400),暂不做;
- Tier-3 词表是「已知字段名」驱动,新增结构化输出字段时需同步维护;
  长期可考虑 schemars 生成词表(知识库 gap L16-L25 既有条目);
- SessionContext 摘要的链路描述建议后续在系统提示词里给模板约束(P2-1/3)。
