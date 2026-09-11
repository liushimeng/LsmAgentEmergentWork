# 自动化测试提示词 — laew 元任务与工程特性（L01–L10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。

## 维度说明

本维度考察"用 laew 测 laew"的**自验证闭环**：每轮必须是 laew 可用 Bash/Read 具体执行的动作——跑 `./laew --version`、`bash testReport/run_e2e.sh` 后 grep 输出、`ls DebugReport/`、python3 内置 sqlite3 查根目录 `LsmAgentEmergentWork.db` 的表。不写"观察是否触发"空话，观察类目标转化为"跑完抓取输出并 grep 断言"。

---

### L01 laew 自测试：跑通 e2e 并定位失败
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_03-D03-L01-测试与按钮显示Bug修复方案.md）
- **预期档位**: medium
- **考察维度**: 工程自验证 / 失败定位闭环
- **工具链**: Read → Bash → Bash → Write
- **对话脚本**:
  1. Read `testReport/run_e2e.sh`，输出它包含哪几节测试（用 `grep -n 'section "'` 抓出节号与标题列表）。
  2. Bash 跑 `bash testReport/run_e2e.sh 2>&1 | tee tmpPlan/agent-test/l01_e2e.txt`，完成后 Bash 用 `grep -cE "check [0-9]+ ok|PASS|✓" tmpPlan/agent-test/l01_e2e.txt` 统计通过数，用 `grep -E "FAIL|✗|check [0-9]+ failed" tmpPlan/agent-test/l01_e2e.txt | head -20` 抓失败清单。
  3. 若有失败：Read 失败用例对应源码段，Write `tmpPlan/agent-test/l01_fix.md` 给修复建议（含失败原因 + 改动文件 + 预期回归方式）；若无失败：Write `l01_fullpass.md` 列出全部 PASS 节号。
  4. 把 mock LLM 工作原理总结写到 `tmpPlan/agent-test/l01_mock.md`：解释 mock server 如何按角色分流（参考 e2e 脚本里的 mock 路由）、以及"新增一个测试用例应改哪里"（指出具体函数/节号）。

### L02 多 Agent 编排验证：触发 hard 全流程
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_05-A02-E03-L02-S01-C05-自动化测试与hard任务Plan解析Bug修复方案.md）
- **预期档位**: hard
- **考察维度**: 触发完整多 Agent 流程 / 产物齐全
- **工具链**: Bash → Bash → Bash → Write
- **对话脚本**:
  1. Bash 跑 `./laew -p "为 laew 添加一个 Glob 工具,支持文件模式匹配" 2>&1 | tee tmpPlan/agent-test/l02_run.txt`，完成后用 `grep -iE "hard|plan|Plan Agent|Main-Work|SubAgent|Quality-Check|SessionContext" tmpPlan/agent-test/l02_run.txt` 抓各 Agent 触发日志。
  2. Bash 跑 `ls -la plans/` 检查是否有新 `.md` 方案文档产出，用 `ls -t plans/ | head -5` 列出最新 5 个；若没有，用 `grep -i "Yolo\|simple\|medium\|hard" tmpPlan/agent-test/l02_run.txt` 反查 Yolo 分类结果。
  3. Read 最新方案文档（若有）评估合理性（是否含目标/拆解/验证三节），把 Read 摘要落到 `tmpPlan/agent-test/l02_plan_review.md`；再用 `grep -c "SubAgent\|Quality-Check" tmpPlan/agent-test/l02_run.txt` 统计 SubAgent/QC 是否都触发。
  4. 写 `tmpPlan/agent-test/l02_report.md`：列出"Yolo 分类 / Plan 文档路径 / Main-Work 步骤数 / SubAgent 调用数 / QC 结论"五列事实，给出各 Agent 产物是否齐全的判定（✓/✗），不写空话。

### L03 上下文压缩触发验证
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_21-B08-C06-D10-E02-L03-自动化测试与Yolo分类验证方案.md）
- **预期档位**: hard
- **考察维度**: Compact Agent 触发 / 保护带保留
- **工具链**: Read → Bash → Bash → Write
- **对话脚本**:
  1. Read `src/agent/compact.rs`，输出压缩触发阈值（默认 context_max_size 的 80%）、三档压缩率（Light/Medium/Aggressive）的切换条件、以及保护带内容（项目上下文/历史摘要/最近 4 条）。
  2. Bash 跑 `./laew -p "请连续完成 5 个独立编码任务,每个任务写一个独立 python 函数" 2>&1 | tee tmpPlan/agent-test/l03_run.txt`，完成后用 `grep -iE "compact|compress|Compact Agent|压缩|token" tmpPlan/agent-test/l03_run.txt` 抓压缩相关日志。
  3. 若本轮未触发压缩：Write `tmpPlan/agent-test/l03_stress.py`，用 `python3` 调用 laew 的 stdin 一次性塞入一段 ≥ 200 字的 prompt（Read 构造），再跑 `./laew -p "$(cat long_prompt.txt)"` 尝试触发，把结果落 `l03_long.txt`；若仍无法在 mock 触发，说明 mock 环境限制并记录到报告。
  4. 写 `tmpPlan/agent-test/l03_report.md`：列出 token 估算方式（字符/4 +10%）、本轮是否触发压缩、触发/未触发的证据日志行号、以及"mock 环境是否真能触发压缩"的判定。

### L04 协议切换：查证请求端点
- **预期档位**: medium
- **考察维度**: Anthropic ↔ OpenAI 切换 / 端点核查
- **工具链**: Bash → Bash → Bash → Write
- **对话脚本**:
  1. Bash 跑 `./laew provider list 2>&1 | tee tmpPlan/agent-test/l04_list.txt`，输出当前活跃接入记录（grep `is_active=1` 行或用 `grep -A2 "活跃\|active" l04_list.txt`）。
  2. 若无 OpenAI 兼容记录：Bash 跑 `./laew provider add --protocol openai --provider-name mock-openai --model-name gpt-4o-mini --end-point http://127.0.0.1:8000 --api-key sk-test`（端点用本地 mock）；Bash 用 `./laew provider list` 再次验证新记录已出现（grep `mock-openai`）。
  3. Bash 跑 `./laew provider use <新记录 id>`，再用 `./laew provider list` 验证活跃位已切换（grep 新记录 is_active=1、旧记录 is_active=0）。
  4. 写 `tmpPlan/agent-test/l04_report.md`：列出"切换前活跃 / 切换后活跃 / 接入点补全规则（Anthropic→/v1/messages, OpenAI→/chat/completions）"，并给出两协议 wire 格式的关键差异（tools 字段位置 / Auth 头 / metadata.user_id）。

### L05 TUI 子屏自动化：生成 tmux 脚本并真跑断言
- **预期档位**: medium
- **考察维度**: tmux 自动化 / 子屏真渲染验证
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/l05_tui_test.sh`：tmux 脚本——`tmux new-session -d -s laew_l05 -x 100 -y 30 ./laew` → 等 2 秒 → `tmux send-keys -t laew_l05 -l "/provider list"` → `tmux send-keys -t laew_l05 Enter` → 等 1 秒 → `tmux capture-pane -p -t laew_l05 > tmpPlan/agent-test/l05_list_screen.txt` → `tmux send-keys -t laew_l05 Escape` → `tmux kill-session -t laew_l05`。
  2. Bash 跑 `bash tmpPlan/agent-test/l05_tui_test.sh`，完成后用 `grep -iE "provider|ProviderList|/provider list|接入记录|列表" tmpPlan/agent-test/l05_list_screen.txt` 断言子屏标题出现。
  3. 在脚本里再加一段：Escape 后 `send-keys "/provider add"` + Enter，再 `capture-pane` 到 `l05_add_screen.txt`，用 `grep -iE "Tab|protocol|provider_name|确认|protocol.*Tab" tmpPlan/agent-test/l05_add_screen.txt` 断言 Tab 表单渲染（至少出现 protocol/provider_name 任一字段）。
  4. 写 `tmpPlan/agent-test/l05_report.md`：列出两次抓屏各自的命中关键字、子屏是否真的渲染了 alternate screen（可通过抓屏是否带 ANSI 转义判断）、并解释"为什么子屏测试必须用 tmux 而不能用管道"（atty 分流原理，管道回退 print 输出非真实渲染）。

### L06 Debug 模式：端到端产出并读报告
- **预期档位**: hard
- **考察维度**: Debug Agent 评估 / 四章节齐全
- **工具链**: Bash → Bash → Read → Write
- **对话脚本**:
  1. Bash 跑 `./laew -debug -p "实现一个简单的计算器功能,支持加减乘除" 2>&1 | tee tmpPlan/agent-test/l06_run.txt`，完成后用 `grep -iE "Debug|debug_report|DebugReport|trace" tmpPlan/agent-test/l06_run.txt` 抓 Debug 日志。
  2. Bash 跑 `ls -lt DebugReport/ | head -10` 列出最新 Debug 报告，把最新文件路径写到 `tmpPlan/agent-test/l06_report_path.txt`（用 `ls -t DebugReport/ | head -1`）。
  3. Read 最新 DebugReport 文件，用 Bash `grep -cE "任务评估|质量报告|问题报告|优化建议|报告" <文件路径>` 断言四章节齐全。
  4. 写 `tmpPlan/agent-test/l06_summary.md`：列出四章节各自行数、P0/P1/P2 问题各几条、trace 采集机制（LLM 装饰器在各 Agent run_session 处写入 trace），并给出 Debug Agent 评分与真实质量是否匹配的判定。

### L07 SessionContext 记忆：跨任务注入验证
- **预期档位**: medium
- **考察维度**: 跨任务记忆 / SESSION_HISTORY 注入
- **工具链**: Bash → Bash → Bash → Write
- **对话脚本**:
  1. Bash 跑 `./laew -p "请用三句话解释什么是 Agent,并举一个 laew 里的例子" 2>&1 | tee tmpPlan/agent-test/l07_task1.txt`，完成后用 `grep -i "Agent\|Yolo\|SubAgent" tmpPlan/agent-test/l07_task1.txt` 确认任务 1 有实质输出。
  2. Bash 用 python3 内置 sqlite3 查根目录库：`python3 -c "import sqlite3;con=sqlite3.connect('LsmAgentEmergentWork.db');print(con.execute('SELECT COUNT(*) FROM session_memory').fetchone());print([r[0] for r in con.execute('SELECT DISTINCT session_id FROM session_memory').fetchall()])"` 落 `tmpPlan/agent-test/l07_mem.txt`，断言 `session_memory` 行数增加。
  3. Bash 跑 `./laew -p "基于上次关于 Agent 的讨论,现在深入比较 Yolo Agent 与 Main-Work Agent 的职责差异" 2>&1 | tee tmpPlan/agent-test/l07_task2.txt`，用 `grep -iE "上次|session|history|Yolo.*分类|Main-Work.*流程" tmpPlan/agent-test/l07_task2.txt` 抓注入证据。
  4. 写 `tmpPlan/agent-test/l07_report.md`：列出"task1 写入的 session_memory 行数 / task2 是否引用历史摘要 / `<<<LAEW:SESSION_HISTORY>>>` 标记的作用（隔离用户对话与历史摘要防混淆）"。

### L08 错误恢复与溢出处理：用 python 验证等价逻辑
- **预期档位**: hard
- **考察维度**: 三级恢复 / 溢出正则等价单测
- **工具链**: Read → Write → Bash → Write
- **对话脚本**:
  1. Read `src/agent/overflow.rs`，输出 `OVERFLOW_PATTERNS` 常量里的全部子串（至少 10 个，如 `prompt is too long` / `context_length_exceeded` / `maximum context length` / `exceeds the context window` 等）。
  2. Write `tmpPlan/agent-test/l08_overflow_test.py`：把第 1 步读出的子串列成一份本地数组（不依赖真实 provider），写一个 `is_overflow(msg: str) -> bool` 函数复现等价逻辑（统一 lower 后 any 匹配），并构造 8 条用例：3 条 Anthropic 风格、3 条 OpenAI/vLLM/Gemini 风格、2 条非溢出负样本（如 `internal server error` / `rate limit`）。
  3. Bash 跑 `python3 tmpPlan/agent-test/l08_overflow_test.py`，断言 8 条用例全部命中预期（3+3 正样本 True、2 负样本 False），打印 `8/8 passed`。
  4. 写 `tmpPlan/agent-test/l08_report.md`：列出"排水/折叠/暴露"三级恢复触发条件、4 次恢复预算耗尽后行为（原错误上抛、用户看到错误提示）、本次单测覆盖了哪些 provider 风格、以及"为什么在测试里用等价逻辑而非真实触发 provider 溢出"。

### L09 配置迁移：模拟旧库缺列并验证回填
- **预期档位**: medium
- **考察维度**: SQLite 迁移 / 默认值回填
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/l09_migration.py`：用 python3 内置 sqlite3 建一份"旧格式"库 `old.db`，表 `providers_old(id, protocol, provider_name, model_name, end_point, api_key)` 但故意缺 `context_max_size` 与 `is_active` 列；插入 2 行数据。
  2. Write `l09_check.py`：打开 `old.db`，检测是否缺 `context_max_size` 列（用 `PRAGMA table_info`），若缺则 `ALTER TABLE providers_old ADD COLUMN context_max_size INTEGER DEFAULT 800000`，再 `UPDATE` 回填默认值；Bash 跑完后断言两行 `context_max_size` 都等于 800000。
  3. 在 `l09_check.py` 里加一段"模拟回滚"：把回填的列 `ALTER TABLE ... DROP COLUMN`（若 sqlite 不支持 DROP COLUMN 则改为建新表+复制数据），Bash 跑一遍验证回滚后查询该列报错 `no such column`。
  4. 写 `tmpPlan/agent-test/l09_report.md`：列出缺列检测 SQL / 回填默认值 / 回滚脚本，并讨论 laew 真实迁移是"打开时自动迁移"（当前策略）vs"显式迁移命令" 的优劣。

### L10 版本升级：读 build.rs 输出并写迁移指南
- **预期档位**: medium
- **考察维度**: 升级流程 / 兼容性保证
- **工具链**: Read → Bash → Bash → Write
- **对话脚本**:
  1. Read `build.rs`，输出 `--version` 注入的编译期常量（`LAEW_BUILD_TIME` / `LAEW_GIT_HASH` / `LAEW_VERSION` 等）与注入方式（`println!("cargo:rustc-env=...")`）。
  2. Bash 跑 `./laew --version`，把输出落到 `tmpPlan/agent-test/l10_version.txt`；Bash 用 `grep -E "laew|版本|build|git|hash|时间|v[0-9]+\.[0-9]+" l10_version.txt` 断言至少含版本号与编译时间/git hash 中的两项。
  3. Bash 用 python3 内置 sqlite3 查根目录库当前 `providers` 表的列名（`PRAGMA table_info(providers)`），与 build.rs 版本对照，列出"从 0.1.x 升级到当前版本"哪些列是新增的（如 `context_max_size`）。
  4. 写 `tmpPlan/agent-test/l10_upgrade_guide.md`：含"兼容项 / 不兼容项 / 迁移步骤（备份库→替换二进制→启动自动迁移→验证 providers 行数不变）"，并给出语义化版本在 laew 中的应用规则（什么情况升 major：破坏性 schema 变更或 CLI 参数不兼容）。
