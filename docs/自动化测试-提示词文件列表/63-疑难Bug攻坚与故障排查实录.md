# 63 疑难 Bug 攻坚与故障排查实录

> 编号段 BL01–BL10 · 聚焦 真实疑难 Bug 排查：段错误 / 内存泄漏 / 死锁 / 数据竞争 / OOM / CPU 100% / fd 耗尽 / 时钟异常 / 乱码 / 变更归因
> 与现有 39 维「性能调优与 Profiling 实战」互补：39 写方法论（perf / eBPF / 火焰图 / 工具链）；63 写**具体疑难 Bug 的排查实录**（一个 Bug = 一个 BL，给出真实排查路径）
> 与现有 AD01「ELF 文件结构」等专题互补：AD 偏文件格式；63 偏**运行时故障**与生产事故

---

### BL01 段错误 SIGSEGV：C 程序空指针解引用 + gdb 回溯
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_23-B09-B10-自动化测试与TUI慢链路显示错乱及wire合并修复方案.md）
- **预期档位**: medium
- **考察维度**: core dump 捕获 + addr2line 符号化 + 栈回溯
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/buggy.c` 用 Write 写一个故意缺陷的 C 程序：`int main() { char *p = NULL; printf("%c\n", *p); return 0; }`；用 `bash` 跑 `gcc -g -O0 -o buggy buggy.c && ./buggy` 应得到 `Segmentation fault (core dumped)`（非 0 退出码 + stderr 含 `Segmentation`）。
  2. 在 `tmpPlan/agent-test/test_segfault.py` 写一个 Python 复现 + 断言脚本：`subprocess.run(['./buggy'], capture_output=True)` 断言 returncode 不为 0 + stderr 含 `Segmentation fault`；记录 stderr 全文到 `tmpPlan/agent-test/segfault.log`。
  3. 用 `Bash` 跑 `ulimit -c unlimited && ./buggy 2>&1 | tee tmpPlan/agent-test/segfault.out` 触发 core dump；用 `gdb -batch -ex 'bt' ./buggy ./core` 拿到回溯断言包含 `main` + `printf` + 空指针地址 `0x0`。
  4. 修复：在 `tmpPlan/agent-test/fixed.c` 写 `if (!p) { fprintf(stderr, "null pointer\n"); return 1; }`；`gcc -g -o fixed fixed.c && ./fixed` 退出码 0 + stderr 含 `null pointer`；最后 `rm -f buggy.c buggy fixed.c fixed buggy core segfault.*` 清理。

### BL02 Python 内存泄漏：tracemalloc 定位到调用栈
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_23-B09-B10-自动化测试与TUI慢链路显示错乱及wire合并修复方案.md）
- **预期档位**: medium
- **考察维度**: RSS 监控 + tracemalloc 栈采样 + 泄漏检测
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/leaky.py` 用 Write 写一个故意泄漏的 Python 程序：`leak_list = []; def leaky_func(): x = bytearray(10_000_000); leak_list.append(x); return;` `for i in range(20): leaky_func()` 把 200MB 字节数组挂到全局 leak_list；用 `tracemalloc.start()` 在程序入口启动追踪。
  2. 跑 `python3 leaky.py` 记录 RSS（`/proc/<pid>/status` 的 VmRSS）+ tracemalloc 快照；断言 20 次迭代后 `tracemalloc.get_traced_memory()[0]` 增长到 ≥ 100MB（确认泄漏） + top 1 分配栈 `leaky.py:leaky_func:2` 贡献 > 50%。
  3. 在 `tmpPlan/agent-test/test_leak.py` 写断言脚本：`subprocess.run(['python3', 'leaky.py'], capture_output=True)` 解析 stdout 断言 RSS >= 100MB 且 top 1 栈定位 `leaky_func` 行号。
  4. 修复：在 `tmpPlan/agent-test/fixed_leak.py` 改成 `x = bytearray(...) ; return None`（不挂全局列表）+ 调 `gc.collect()` 释放；再跑 `python3 fixed_leak.py` 断言 RSS < 50MB 且 tracemalloc top 1 栈不是 `leaky_func`；写「laew `agent_memory` 累积未截断泄漏」设计说明到 `tmpPlan/agent-test/leak_design.md`，最后清理测试文件。

### BL03 Python 死锁：threading 锁顺序反转 + 调试输出
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_23-B09-B10-自动化测试与TUI慢链路显示错乱及wire合并修复方案.md）
- **预期档位**: hard
- **考察维度**: 锁顺序反转 + 信号 stack dump + 锁规约
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/deadlock.py` 用 Write 写两个线程互相等锁：`lock_a = threading.Lock(); lock_b = threading.Lock(); def t1(): acquire(lock_a); sleep(0.1); acquire(lock_b)`；`def t2(): acquire(lock_b); sleep(0.1); acquire(lock_a)`；主线程 5 秒后 `os.kill(os.getpid(), signal.SIGQUIT)`（Python 默认 `signal.signal` 会 dump 所有线程栈到 stderr）。
  2. 跑 `timeout 6 python3 deadlock.py 2>&1 | tee tmpPlan/agent-test/deadlock.out` 断言：6 秒后 SIGQUIT 触发 + stderr 含 2 个线程都在 `Lock.acquire` 阻塞 + Python stack dump 显示 `t1` 与 `t2`；进程 exit 非 0。
  3. 在 `tmpPlan/agent-test/test_deadlock.py` 写断言脚本：`subprocess.run(['timeout', '6', 'python3', 'deadlock.py'], capture_output=True)` 断言 stderr 含 `t1` 与 `t2` 线程名 + 至少 2 个 `Lock.acquire` 字样。
  4. 修复：在 `tmpPlan/agent-test/fixed_deadlock.py` 统一锁顺序（先 lock_a 后 lock_b）→ 跑 `timeout 6 python3 fixed_deadlock.py` 断言 stderr 无 `Lock.acquire` 阻塞 + exit 0；写「laew agent_memory 表读写锁顺序规约」设计说明到 `tmpPlan/agent-test/deadlock_design.md`，最后清理测试文件。

### BL04 Python 数据竞争：threading 非线程安全 dict + 原子性
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_23-B09-B10-自动化测试与TUI慢链路显示错乱及wire合并修复方案.md）
- **预期档位**: hard
- **考察维度**: 数据竞争 + GIL 边界 + 原子操作
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/race.py` 用 Write 写一个数据竞争程序：`shared = {}; def worker(i): for _ in range(1000): shared[i] = shared.get(i, 0) + 1`；启 10 个线程各跑 worker；期望最终 shared[i] 之和 == 10000，但因 `shared.get + 1 + 赋值` 非原子，实际可能 < 10000（竞争丢失更新）。
  2. 跑 `python3 race.py` 三次取最坏结果记录到 `tmpPlan/agent-test/race.out`；断言三次中有 ≥ 1 次 shared 总和 < 10000（证明竞争存在）；输出冲突计数器 `race_count = 10000 - sum(shared.values())`。
  3. 在 `tmpPlan/agent-test/test_race.py` 写断言：跑 10 次 race.py 断言 ≥ 5 次总和小于 10000（统计意义上确认竞争）。
  4. 修复：用 `collections.Counter` 替代 dict（Counter 的 `__setitem__` 在 C 层是原子操作）或加 `threading.Lock`；跑 `python3 fixed_race.py` 三次断言总和恒为 10000；写「laew SessionContext 跨 SubAgent 共享状态是否需加原子计数器」到 `tmpPlan/agent-test/race_design.md`，最后清理。

### BL05 fd 泄漏：open() 不 close + /proc/PID/fd 监控
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_23-B09-B10-自动化测试与TUI慢链路显示错乱及wire合并修复方案.md）
- **预期档位**: medium
- **考察维度**: fd 计数 + /proc fd 监控 + ResourceWarning
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/fd_leak.py` 用 Write 写一个 fd 泄漏程序：`for i in range(2000): f = open('/dev/null', 'r'); # 故意不 close`；跑 `python3 -W error::ResourceWarning fd_leak.py` 断言触发 ResourceWarning 报警 2000 次 + `len(os.listdir('/proc/self/fd'))` 增长到 ≥ 2002（标准 + 标准错误 + 2000 个 /dev/null）。
  2. 跑 `python3 fd_leak.py` 实际打开 2000 个 fd 不关闭 → `ls /proc/$$/fd | wc -l`（在子进程里）断言 ≥ 2000；同时 `ResourceWarning` 触发 ≥ 2000 条（python -W default 也可）。
  3. 在 `tmpPlan/agent-test/test_fd.py` 写断言脚本：spawn 子进程跑 `python3 fd_leak.py` → 跑中读 `/proc/<child_pid>/fd` 计数断言 ≥ 2000；捕获 stderr 断言含 ≥ 1000 条 `ResourceWarning`。
  4. 修复：用 `with open(...) as f:` 或显式 `f.close()`；跑 `python3 fixed_fd.py` 断言 `/proc/self/fd` 数量恒为 5（stdin/stdout/stderr + 2 个 ResourceWarning 自身）；写「laew LLM client fd 泄漏监控」设计说明到 `tmpPlan/agent-test/fd_design.md`，最后清理测试脚本。

### BL06 CPU 100%：灾难性正则回溯 + 防 ReDoS
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_23-B09-B10-自动化测试与TUI慢链路显示错乱及wire合并修复方案.md）
- **预期档位**: medium
- **考察维度**: ReDoS + regex 性能 + NFA/DFA 引擎
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/regex_dos.py` 用 Write 写一个 ReDoS 复现：模式 `^(a+)+$` + 输入 `aaaaX`；`time python3 regex_dos.py` 跑 `re.match('^(a+)+$', 'aaaa' * 5 + 'X')` 测耗时；预期在某些 Python 版本/输入长度下耗时 ≥ 1 秒（CPython 自带 RE 是回溯式）。
  2. 加防 ReDoS：用 `regex` 库的 `regex.match(pattern, text, timeout=0.5)`（`regex` crate Python 移植支持超时）→ 触发 `regex.TimeoutError`；断言未超时版本耗时长 + 超时版本立即抛 TimeoutError。
  3. 在 `tmpPlan/agent-test/test_re_dos.py` 写断言：跑 `re.match('^(a+)+$', 'a'*30 + 'X')` 用 `time.perf_counter()` 测耗时 ≥ 0.5s 视为 ReDoS 触发；用 `regex.match(..., timeout=0.2)` 断言抛 TimeoutError。
  4. 修复：在 `tmpPlan/agent-test/safe_regex.py` 改用 `regex_automata` Python 移植（非回溯 DFA）→ 跑 `python3 safe_regex.py` 测 `^(a+)+$` 在 30 个 `a` + `X` 输入耗时 < 0.05s（常数时间）；写「laew Bash 命令过滤用 NFA/DFA 自研引擎避免第三方 regex ReDoS」设计说明到 `tmpPlan/agent-test/regex_design.md`，最后清理。

### BL07 时区 off-by-one：DST 切换日 + Asia/Shanghai
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_23-B09-B10-自动化测试与TUI慢链路显示错乱及wire合并修复方案.md）
- **预期档位**: hard
- **考察维度**: monotonic vs wall clock + DST 切换 + tz data
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/tz_bug.py` 用 Write 写一个时区 bug 复现：本地时间 2024-03-10 02:30 America/New_York（美国 DST 开始日，02:00 → 03:00 跳过）；用 `datetime(2024,3,10,2,30)` `astimezone(ZoneInfo('America/New_York'))` 应抛 `NonExistentTimeError`（不存在时间）。
  2. `astimezone(ZoneInfo('Europe/Moscow'))` 2014-10-26 02:30 可能 `AmbiguousTimeError`（2 点到 3 点重复）；bug 程序捕获异常后错误地 `replace(fold=1)` 直接选后一次重复时间，未告知用户歧义。
  3. 在 `tmpPlan/agent-test/test_tz.py` 写断言：调用 buggy 函数断言在 NonExistent 时返回某个「默认」（如 03:30）而非报错；记录 buggy 行号 `tz_bug.py:15`；同时正确版本应抛异常。
  4. 修复：在 `tmpPlan/agent-test/fixed_tz.py` 改用 `datetime(2024,3,10,3,30)`（跳到 DST 后的等同时刻）+ 异常时主动 `logger.warning` 告知用户；跑测试断言 NonExistent 抛 `NonExistentTimeError` + Ambiguous 抛 `AmbiguousTimeError`；写「laew SessionContext `created_at` UTC + ISO8601 + 纳秒精度」设计说明到 `tmpPlan/agent-test/tz_design.md`，最后清理。

### BL08 乱码：GBK 文件读取为 UTF-8 + chardetng 嗅探
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_23-B09-B10-自动化测试与TUI慢链路显示错乱及wire合并修复方案.md）
- **预期档位**: medium
- **考察维度**: charset detection + encoding 转换 + BOM
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/buggy_encoding.py` 用 Write 写一个乱码 bug 复现：用 `open('sample_gbk.txt', encoding='utf-8').read()` 读 GBK 编码文件 → 抛 `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xd5 in position 0`。
  2. 用 Python `codecs.open` + `chardet.detect(bytes_data)` 先嗅探（`chardet` crate Python 移植）→ 返回 `{'encoding': 'GB2312', 'confidence': 0.99}` → 用正确编码重读 → 文本正确。
  3. 在 `tmpPlan/agent-test/test_encoding.py` 写：用 Write 把中文「你好世界」用 GBK 编码写到 `tmpPlan/agent-test/sample_gbk.txt`（`open(path, 'wb').write('你好世界'.encode('gbk'))`）；buggy 版断言抛 UnicodeDecodeError；fixed 版 `chardet.detect(open(path,'rb').read())` 断言返回 GB2312 + 重新读出字符串断言 == `你好世界`。
  4. 修复：在 `tmpPlan/agent-test/fixed_encoding.py` 写 `detect_and_read(path)` 函数：先嗅探 → 嗅探失败 fallback UTF-8 → 强制 UTF-8 读 + `errors='replace'`；跑测试断言 GBK 文件正确读出 + UTF-8 文件正常；写「laew Read 工具统一编码嗅探」设计说明到 `tmpPlan/agent-test/encoding_design.md`，最后清理 sample_gbk.txt。

### BL09 时钟异常：单调时钟 vs wall clock 混用 + Duration 负值
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_23-B09-B10-自动化测试与TUI慢链路显示错乱及wire合并修复方案.md）
- **预期档位**: medium
- **考察维度**: monotonic clock + duration 计算 + NTP 跳变
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/clock_bug.py` 用 Write 写一个时钟 bug 复现：用 `datetime.now()` (wall clock) 计算 duration：`start = datetime.now(); time.sleep(1); duration = datetime.now() - start`（应该 OK）；但若 NTP 把时钟向前调 60 秒 → `duration` 出现负值 `timedelta(seconds=-59)`；同样的代码用 `time.monotonic()` 永远单调递增，不会出现负值。
  2. 模拟 NTP 跳变：测试用 mock 把 `time.time()` 在两次调用之间向后调 5 秒 → buggy 版断言 `duration.total_seconds() < -3`；fixed 版用 `time.monotonic()` 断言 duration ≈ 1 秒。
  3. 在 `tmpPlan/agent-test/test_clock.py` 写断言脚本：mock `time.time` 模拟跳变 → buggy 版 `buggy_measure()` 返回负值 duration → 抛 `ValueError('negative duration')`；fixed 版 `safe_measure()` 返回正常 duration。
  4. 修复：在 `tmpPlan/agent-test/fixed_clock.py` 用 `time.monotonic()` + `time.monotonic_ns()`；跑测试断言 fixed 版在模拟跳变下不出现负值；写「laew Session 持续时间统计统一用 monotonic clock」设计说明到 `tmpPlan/agent-test/clock_design.md`，最后清理测试脚本。

### BL10 变更归因：git bisect 风格二分定位 + 回归断言
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_23-B09-B10-自动化测试与TUI慢链路显示错乱及wire合并修复方案.md）
- **预期档位**: hard
- **考察维度**: git bisect + 回归测试 + 变更影响分析
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bisect/` 用 Write 写 5 个版本的 `compute.py`：v1 正确实现 `def add(a,b): return a+b`；v2 改成 `def add(a,b): return a-b`（bug1：符号错）；v3 改回 `def add(a,b): return a+b`（修复）；v4 改成 `def add(a,b): return a*b`（bug2：乘号错）；v5 改回 `def add(a,b): return a+b`（修复）。
  2. 用 `Write` 写 `test_compute.py` 回归测试：`assert compute.add(2,3) == 5`；跑 v1/v3/v5 通过，跑 v2/v4 失败。
  3. 在 `tmpPlan/agent-test/bisect_run.py` 写二分查找脚本：`versions = [v1,v2,v3,v4,v5]` + 当前状态为「不通过」 + 已知 v1 通过 v5 通过 → 二分 mid 试 → 找到第一个引入 bug 的版本（v2）+ 第一个修复的版本（v3）。
  4. 跑 `python3 bisect_run.py` 断言：找到 bug 引入版本 == v2（add 改成 a-b）+ 找到修复版本 == v3；写「laew SubAgent 任务记录 prompt 哈希 + 工具调用序列用于回归归因」设计说明到 `tmpPlan/agent-test/bisect_design.md`，最后 `rm -rf bisect/` 清理。