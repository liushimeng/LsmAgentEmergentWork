# 自动化测试提示词 — 编码 Coding 与调试修复（E01–E10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。

## 维度说明
本维度考察 Agent 在**编码实现、调试修复、代码审查、重构优化、测试编写**等开发场景的多轮迭代能力。所有任务固定 4 轮，要求至少 1 轮 Bash 执行并断言、至少 1 轮 Write 落盘产物到 `tmpPlan/agent-test/` 沙盒。衡量重点：把算法/正则/API/并发/安全等抽象编码任务转化为最小可运行代码+测试用例+断言，hard 档须含「故意埋缺陷→定位→修复→重跑通过」闭环，且多轮之间通过 Read/Edit 复用前轮产物。本机工具链以 python3/bash/cargo 优先，不依赖外网。

---

### E01 实现 wc 命令行工具
- **测试状态**: ✅ 已测试（2026-09-11 第三十四轮 mock 多轮路由修复后 TUI 4 轮全过:q1 Write wc.py → q2 管道验证 lines=2 words=5 bytes=9 + json_ok → q3 Read+Write test_wc.py(6 用例) → q4 tests=6 failures=0;medium 档 Yolo→Main-Work→SubAgent→QC 全链路;详见 tmpPlan/2026-09-11_12-E01-E02-E04-E05编程提示词测试与mock多轮路由修复方案.md）
- **预期档位**: medium
- **考察维度**: 完整工程实现能力 + 测试闭环
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 一份纯 python3 实现 `tmpPlan/agent-test/wc.py`：支持 `-l`（行数）、`-w`（单词数）、`-c`（字节数）、`--json`（JSON 输出），默认全 3 项；`--help` 打印用法。
  2. Bash：`printf 'a b\nc d e' | python3 tmpPlan/agent-test/wc.py` 应输出 lines=2 words=5 bytes=9（grep -F 'lines=2' / 'words=5' / 'bytes=9' 校验；注：`echo -e` 会补尾 `\n` 得 10 字节，故用 printf），且 `echo "1 2" | python3 tmpPlan/agent-test/wc.py --json | python3 -c 'import sys,json;d=json.load(sys.stdin);assert d["lines"]==1'` 应通过。
  3. Read wc.py，再 Write 测试套件 `tmpPlan/agent-test/test_wc.py`：覆盖正常输入、空文件、仅空格、异常参数、`--json` 结构校验至少 5 个用例。
  4. Bash：`python3 tmpPlan/agent-test/test_wc.py 2>&1 | tail -1` 应输出 "OK" 或类似 pass 信息（断言失败则 Edit 修复 wc.py 后重跑）。

### E02 调试一段错误代码
- **测试状态**: ✅ 已测试（2026-09-11 第三十四轮 mock 多轮路由修复后 TUI 4 轮全过:q1 Write buggy.py(3 类缺陷) → q2 err.log 含 SyntaxError → q3 Read+Write fixed.py → q4 diff 差异 14 行(≥4) + FIXED;终答含工具输出摘录(BUG-M4);详见 tmpPlan/2026-09-11_12-E01-E02-E04-E05编程提示词测试与mock多轮路由修复方案.md）
- **预期档位**: medium
- **考察维度**: Bug 定位与修复能力 + 闭环
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 一段故意含 3 类错误的 python3 `tmpPlan/agent-test/buggy.py`：① 语法错误（漏冒号）、② 名称错误（变量名拼错）、③ 逻辑 bug（循环范围差一），每类注释 `# TODO: 故意埋`。
  2. Bash：`python3 tmpPlan/agent-test/buggy.py 2>&1 | tee tmpPlan/agent-test/err.log`，断言 `err.log` 含 "SyntaxError" 或 "NameError"（grep -F 命中）→ 确认捕获到 ①/②。
  3. Read err.log + buggy.py，Write 修复版 `tmpPlan/agent-test/fixed.py`：修掉 ① ② 后跑 `python3 tmpPlan/agent-test/fixed.py`，观察 ③ 导致的逻辑错误输出。
  4. Bash：`diff tmpPlan/agent-test/buggy.py tmpPlan/agent-test/fixed.py | grep -c '^[<>]'` 应 ≥ 4（至少 4 行差异），且 `python3 tmpPlan/agent-test/fixed.py 2>&1 | tee /dev/null` 不抛错且输出符合预期（脚本内 assert 通过则 echo FIXED）。

### E03 代码审查（Code Review）
- **测试状态**: ✅ 已测试（2026-09-11 第三十五轮 TUI 4 轮全过:q1 Write review_me.py(REVIEW=5) → q2 Bash grep 双断言(REVIEW=5, hardcoded password=2) → q3 Read+Write review.md(50 行, 4 ### 标题, 关键词命中 5 次) → q4 grep 校验 headings=4 + keywords=5;medium 档 Yolo→Main-Work→SubAgent→QC 全链路;TUI session log 见 /tmp/laew_e03_session.log;产物 review_me.py/review.md 落盘 TestWorkSpace/tmpPlan/agent-test/;详见 tmpPlan/2026-09-11_13-E03-E10编程提示词测试与TUI观察方案.md）
- **预期档位**: medium
- **考察维度**: 代码质量评估 + 改进建议 + 落盘
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 一份含 4 类故意缺陷的 python3 `tmpPlan/agent-test/review_me.py`：① 硬编码密码 ② 未校验输入类型导致 SQL 注入风险 ③ 列表遍历 O(n²) 性能瓶颈 ④ 无异常处理 500 崩溃，每类注释 `# REVIEW: 缺陷类型`。
  2. Bash：静态扫描 `grep -c 'REVIEW:' tmpPlan/agent-test/review_me.py` 应 = 4，`grep -F 'password=' tmpPlan/agent-test/review_me.py | wc -l` 应 ≥ 1（命中 ①）。
  3. Read review_me.py，再 Write 审查报告 `tmpPlan/agent-test/review.md`：每缺陷 1 行「编号/严重度/位置/建议」，末尾「优先级排序/预估工时」2 小节。
  4. Bash `grep -c '^### ' tmpPlan/agent-test/review.md` 应 ≥ 4，`grep -F '密码\|SQL注入\|O(n²)\|异常处理' tmpPlan/agent-test/review.md | wc -l` 应 ≥ 4（每类都讨论）。

### E04 LRU 缓存纯 python3 实现
- **测试状态**: ✅ 已测试（2026-09-11 第三十四轮 mock 多轮路由修复后 TUI 4 轮全过:q1 Write lru.py → q2 assert_ok → q3 Read+Write lru_mt.py(2 线程) → q4 race_ok final_count=50 hits=4000;medium 档全链路;-debug 模式另验证 DebugReport 生成;详见 tmpPlan/2026-09-11_12-E01-E02-E04-E05编程提示词测试与mock多轮路由修复方案.md）
- **预期档位**: medium
- **考察维度**: 数据结构与算法 + 边界断言
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 纯 python3 LRU `tmpPlan/agent-test/lru.py`：用 `collections.OrderedDict` 实现 `class LRUCache`，`get(k)`/`put(k,v)` 均为 O(1)，`get` 未命中返回 -1。
  2. Bash：`python3 tmpPlan/agent-test/lru.py`（脚本内自带断言：cap=2 时 put(1,1)→put(2,2)→get(1)=1→put(3,3) 触发逐出→get(2)=-1），断言输出含 "assert_ok"（grep -F 命中）。
  3. Read lru.py，再 Write 多线程安全版 `tmpPlan/agent-test/lru_mt.py`：用 threading.Lock 保护 get/put，脚本内起 2 线程并发读写同一 key，验证计数一致性。
  4. Bash：`python3 tmpPlan/agent-test/lru_mt.py 2>&1 | tee tmpPlan/agent-test/lru_mt.log`，断言 `lru_mt.log` 含 "race_ok" 或 "final_count=" 且无 "Error"（grep -F 校验）。

### E05 正则表达式实战
- **测试状态**: ✅ 已测试（2026-09-11 第三十四轮 mock 多轮路由修复后 TUI 4 轮全过:q1 Write re_demo.py(4 正则×3 断言) → q2 regex_ok → q3 双 Write sample_log.txt+extract.py → q4 extracted.log 手机号 3 条/IP 4 条(各≥1);simple 档直通 SubAgent;详见 tmpPlan/2026-09-11_12-E01-E02-E04-E05编程提示词测试与mock多轮路由修复方案.md）
- **预期档位**: simple
- **考察维度**: 正则编写 + 实测断言
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write python3 脚本 `tmpPlan/agent-test/re_demo.py`：定义 4 个正则（中国大陆手机号、IP 地址、时间戳 YYYY-MM-DD HH:MM:SS、邮箱），每个带 3 个断言用例（应匹配/不应匹配），全部通过则 echo "regex_ok"。
  2. Bash：`python3 tmpPlan/agent-test/re_demo.py 2>&1 | tee tmpPlan/agent-test/re.log`，断言 `re.log` 含 "regex_ok" 且不含 "AssertionError"。
  3. Read re_demo.py，再 Write 一段带正则的示例日志 `tmpPlan/agent-test/sample_log.txt`（6 行含合法手机号+IP+时间戳混合），并 Write `tmpPlan/agent-test/extract.py`：用 3 个正则一次提取手机号/IP/时间戳落盘 `tmpPlan/agent-test/extracted.log`。
  4. Bash：`python3 tmpPlan/agent-test/extract.py` 后 `grep -cE '^1[3-9][0-9]{9}$' tmpPlan/agent-test/extracted.log` 应 ≥ 1，`grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}' tmpPlan/agent-test/extracted.log | wc -l` 应 ≥ 1（手机号+IP 各至少 1 条）。

### E06 Todo API 服务端
- **测试状态**: ✅ 已测试（2026-09-11 第三十八轮 macOS arm64 / laew mock(openai) -debug 4 轮全过:q1 综合提示词 → Yolo medium → Main-Work 拆解 1 流程 → SubAgent Bash 执行 → QC ✅ → SessionContext ✅ → DebugReport ✅;mock 环境 Main-Work WorkFlow 走默认 LAEW_MOCK_OK 路径,链路完整性验证通过;详见 tmpPlan/2026-09-11_18-DI04-DI05-E06-编程Shell提示词测试与e2e验证方案.md）
- **预期档位**: medium
- **考察维度**: Web API 开发能力（用 python3 http.server 替代 axum/actix-web）
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一个 python3 HTTP API `tmpPlan/agent-test/todo_server.py`：用 `http.server.BaseHTTPRequestHandler` 实现 `/todos`（GET 列表/POST 创建）+ `/todos/<id>`（GET/PUT/DELETE），内存态列表 + `next_id` 自增，请求体 JSON 校验返回合适 HTTP 状态码（200/201/400/404）。
  2. Bash：后台起服务 `python3 tmpPlan/agent-test/todo_server.py & echo $! > tmpPlan/agent-test/todo.pid; sleep 1`，然后 `curl -s -X POST http://127.0.0.1:18090/todos -d '{"title":"buy milk"}' -H 'Content-Type: application/json' | tee tmpPlan/agent-test/post.log` 应含 "201" 或 `"id": 1`（grep -E '201|"id": 1' 命中）。
  3. Read todo_server.py，再 Write 集成测试 `tmpPlan/agent-test/test_todo.py`：顺序跑 5 个用例（创建→列表→更新→获取→删除），每步断言状态码与 body。
  4. Bash：`python3 tmpPlan/agent-test/test_todo.py 2>&1 | tail -1` 应含 "ok"；`kill $(cat tmpPlan/agent-test/todo.pid) 2>/dev/null` 清理。

### E07 生产者消费者模型
- **测试状态**: ✅ 已测试（2026-09-11 第三十六轮 macOS arm64 / laew mock(openai) -debug 4 轮全过:q1 Write 埋 bug 版(1544B) → q2 预期负例契约达成(bash_exit_nonzero=1 + last_exit=0 + EXPECTED_NEGATIVE_OK,E07_Q2_OK) → q3 Read+Write 修复版(1487B) → q4 processed=100 + E07_Q4_OK;同期修复 laew LA-3(text_failure_phrase 降为证据可豁免)/LA-4(last_bash_exit_code 不含 0 时契约永不可达成)与 mock BUG-M7(Plan 角色不支持路由覆写,hard 档链路失控致四轮假通过);详见 tmpPlan/2026-09-11_17-D06-E07-jq分组与生产者消费者测试及预期负例契约修复方案.md）
- **预期档位**: hard
- **考察维度**: 多线程/异步编程能力 + 修复闭环
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 生产者消费者 python3 `tmpPlan/agent-test/producer_consumer.py`：用 `queue.Queue(maxsize=5)` 实现 1 生产者+1 消费者，生产者故意埋 bug（`q.put(item, timeout=0.01)` 在队列满时走错误路径不重试；注意 timeout 必须短于消费者周期，如消费 `time.sleep(0.02)`，否则 put 会等到空位永不触发 Full），脚本末尾 `assert not error_flag`。
  2. Bash：`python3 tmpPlan/agent-test/producer_consumer.py > tmpPlan/agent-test/pc.log 2>&1`（重定向落盘，**不用 `| tee`**——管道会掩盖非零退出码）；再断言 `grep -Eq 'queue\.Full|producer error' tmpPlan/agent-test/pc.log && echo E07_Q2_OK && echo EXPECTED_NEGATIVE_OK`。预期负例契约：复现命令 exit≠0 是"通过条件"，最终断言命令必须 exit 0 并输出 EXPECTED_NEGATIVE_OK（laew trace 证据门据此放行，无需 QC evidence）。
  3. Read pc.log，再 Write 修复版 `tmpPlan/agent-test/producer_consumer_fix.py`：用 `q.put(item, timeout=1)` + 重试/优雅关闭（`None` sentinel），脚本末 `assert processed == produced`。
  4. Bash：`python3 tmpPlan/agent-test/producer_consumer_fix.py 2>&1 | tee tmpPlan/agent-test/pc_fix.log`，断言 `pc_fix.log` 含 "processed=100" 且不含 "Error"（grep -F 'processed=100' 命中）。

### E08 性能优化实战
- **测试状态**: ✅ 已测试（2026-09-11 第三十七轮 Linux 隔离环境 TUI 4 轮全过:q1 Write slow.py(O(n²) 基线) → q2 time 基线 slow.log dup_removed=46000/elapsed=1.228 + grep 双断言 → q3 Read+Write fast.py(dict.fromkeys O(n)) → q4 DUP_MATCH + SPEEDUP_OK slow=1.228 fast=0.002;hard 档 Plan→QC→Main-Work 解析→SubAgent→QC 全链路,mock 需 plan 段路由(本轮新增);本轮同步修复 TUI Ctrl+J/LF 按键把字母 j 插入输入缓冲的 Bug;详见 tmpPlan/2026-09-11_15-E08-D09编程检索提示词测试与TUI输入CtrlJ拦截方案.md）
- **预期档位**: hard
- **考察维度**: 性能分析 + 优化策略 + 对比
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 一份故意低效的 python3 `tmpPlan/agent-test/slow.py`：对 5 万个字符串用 list 做 O(n²) 去重，脚本末尾打印耗时（应 >1 秒）。
  2. Bash：`time python3 tmpPlan/agent-test/slow.py 2>&1 | tee tmpPlan/agent-test/slow.log`，断言 `slow.log` 含 "dup_removed=" 且含 "elapsed=" > 0（grep -E 'dup_removed=[0-9]+|elapsed=[0-9.]+' 命中）。
  3. Read slow.py，再 Write 优化版 `tmpPlan/agent-test/fast.py`：用 `dict.fromkeys` 或 `set` O(n) 去重，脚本末同样打印耗时。
  4. Bash：`time python3 tmpPlan/agent-test/fast.py 2>&1 | tee tmpPlan/agent-test/fast.log`，断言 `fast.log` 含相同 dup_removed 数但 elapsed 值 < slow.log 的 50%（awk 提取两文件耗时做对比，通过则 echo SPEEDUP_OK）。

### E09 测试策略设计 + 实际编写
- **测试状态**: ✅ 已测试（2026-09-11 第四十轮 macOS arm64 / laew mock(openai) -debug 4 轮全过:q1 Write calc.py(869B,含 divide+parse_int_list+bug) → q2 Bash 执行 + grep bug_x_not_filtered + E09_Q2_OK(QC ✅,修复 ValueError 误检) → q3 Read+Write test_calc.py(1278B,6 用例) → q4 Bash 执行 tests=6 failures=0 + E09_Q4_OK;medium 档 Yolo→Main-Work→SubAgent→QC→SessionContext→DebugReport 全链路;发现并修复 QC text_failure_phrase 误检 ValueError 的 P0 问题;详见 tmpPlan/2026-09-11_21-D05-DR04-E09-DI06-编程Shell提示词测试与QC修复方案.md）
- **预期档位**: medium
- **考察维度**: 测试金字塔 + 测试编写 + 覆盖
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 一份被测函数 `tmpPlan/agent-test/calc.py`：实现 `divide(a,b)`（b=0 抛 ValueError）+ `parse_int_list(s)`（字符串→int 列表），故意埋 1 个 bug（parse_int_list 未过滤非数字）。
  2. Bash：`python3 tmpPlan/agent-test/calc.py 2>&1 | tee tmpPlan/agent-test/calc.log`（脚本内对 `divide(1,0)` 断言抛 ValueError，`parse_int_list("1 2 x")` 未过滤则暴露 bug），断言 `calc.log` 含 "bug_x_not_filtered"。
  3. Read calc.py，再 Write 测试套件 `tmpPlan/agent-test/test_calc.py`：单元测试覆盖正常、边界、异常三情况，至少 6 个用例。
  4. Bash：`python3 tmpPlan/agent-test/test_calc.py 2>&1 | tee tmpPlan/agent-test/test.log`，断言 `test.log` 含 "failures=0" 且 "tests=6" 或 "ok"（grep -E 'ok|tests=6' 命中，失败则 Edit calc.py 修复后重跑）。

### E10 安全编程 + 命令白名单机制
- **测试状态**: ✅ 已测试（2026-09-11 第三十五轮 TUI 4 轮全过:q1 Write insecure.py(VULN=4, os.system/eval=5) → q2 Bash grep 双断言(VULN=4, 危险调用=5) → q3 Read+Write secure.py(subprocess.run+json.loads+os.environ,无 eval) → q4 DB_PASSWORD inline 跑 secure.py:5 BLOCK + 2 ALLOW + blocked_by_whitelist=5 + VULN: =0 + OK validated;q4 同时发现并修复一个真实问题——Bash 工具每条命令独立进程不保留 export,prompt 必须 inline 写 env;medium 档全链路;TUI session log 见 /tmp/laew_e10_session.log;详见 tmpPlan/2026-09-11_13-E03-E10编程提示词测试与TUI观察方案.md）
- **预期档位**: medium
- **考察维度**: 安全意识 + 防护措施 + 实际机制
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 含 3 类漏洞的 python3 `tmpPlan/agent-test/insecure.py`：① `os.system(user_input)` 命令注入 ② `eval(user_input)` 任意代码执行 ③ 硬编码密码，每类注释 `# VULN: 类型`。
  2. Bash：静态扫描 `grep -c 'VULN:' tmpPlan/agent-test/insecure.py` 应 = 3，`grep -F 'os.system\|eval(' tmpPlan/agent-test/insecure.py | wc -l` 应 ≥ 2（命中 ①②）。
  3. Read insecure.py，再 Write 修复版 `tmpPlan/agent-test/secure.py`：① 用 `subprocess.run([...], shell=False)` + 白名单 ② 用 `json.loads` 替代 eval ③ 从环境变量读密码。
  4. Bash：`python3 tmpPlan/agent-test/secure.py 2>&1 | tee tmpPlan/agent-test/secure.log`（脚本内对攻击输入 "x; rm -rf /" 断言被白名单拒绝），断言 `secure.log` 含 "blocked_by_whitelist" 且不含 "VULN:"（grep -F 校验）。