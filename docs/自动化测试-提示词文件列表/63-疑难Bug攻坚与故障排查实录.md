# 63 疑难 Bug 攻坚与故障排查实录

> 编号段 BL01–BL10 · 聚焦 真实疑难 Bug 排查：段错误 / 内存泄漏 / 死锁 / 数据竞争 / OOM / CPU 100% / fd 耗尽 / 时钟异常 / 乱码 / 变更归因
> 与现有 39 维「性能调优与 Profiling 实战」互补：39 写方法论（perf / eBPF / 火焰图 / 工具链）；63 写**具体疑难 Bug 的排查实录**（一个 Bug = 一个 BL，给出真实排查路径）
> 与现有 AD01「ELF 文件结构」等专题互补：AD 偏文件格式；63 偏**运行时故障**与生产事故

---

### BL01 段错误 SIGSEGV 定位：core dump 与符号还原
- **预期档位**: medium
- **考察维度**: core dump 捕获 + addr2line 符号化 + 栈回溯
- **对话脚本**:
  1. 一个 Rust 服务在生产环境偶发段错误（每天 1-2 次），日志只有 `signal 11 (SIGSEGV), invalid memory reference`。第一步：在 Linux 上启用 core dump（`ulimit -c unlimited` + `sysctl kernel.core_pattern=/tmp/core.%e.%p.%t`），容器环境用 `prlimit --pid 1 --core=unlimited:unlimited`。
  2. 基于上面的 core dump，用 `gdb` 加载二进制 + core 文件：`gdb ./laew /tmp/core.laew.12345.1700000000`，`bt full` 看完整栈，找出崩溃点。给出一段真实的崩溃栈样例（如 `agent::compact::CompactRunner::run` 第 248 行）。
  3. core 文件缺符号表怎么办？编译时必须加 `debug = 1` + `[profile.release] debug = true`（Cargo 字段），发布构建也要保留符号；用 `addr2line -e ./laew -f -i 0x7f8a3c001234` 把地址翻译回 `fn compact::run` + 行号。
  4. Rust 特有的段错误来源：unwrap 空指针（少见但 FFI 时常见）、slice 越界（slice index panic 在 release 下可能段错误）、循环引用 `Rc::strong_count` 溢出。给 laew 加一个 panic hook 把堆栈持久化到 SQLite `crash_dump` 表（设计见第十七轮专题：崩溃恢复与取证）。

### BL02 内存泄漏围剿：从增长曲线到调用栈
- **预期档位**: medium
- **考察维度**: RSS 监控 + heap profiling + jemalloc stat
- **对话脚本**:
  1. 一个 Rust 服务跑 3 天后 RSS 从 80MB 涨到 1.5GB，触发 OOM 被杀。第一步：在 `/proc/{pid}/status` 监控 VmRSS，每 10s 采样一次，画时间序列图（用 `gnuplot` 或 `pandas`），确认是**线性增长**（泄漏）还是**锯齿**（缓存，正常）。
  2. 基于上面的增长曲线，加 jemalloc 统计：`MALLOC_CONF="stats_print:true"` 启动，每小时 dump 一次 stat 输出，关注 `Allocated`（当前活跃）与 `Retained`（jemalloc 自己保留的，**不等于泄漏**）的差值。
  3. 进一步定位：用 `heaptrack`（Linux KDE 工具，记录所有 malloc/free + 调用栈）跑服务 1 小时后 `heaptrack_print` 看 top 增长点；Rust 服务的常见泄漏源：`tokio::spawn` 的 task 没结束（channel 一端持有），`mpsc::Sender` 循环引用，`sqlx` 连接池未释放。
  4. 给 laew 排查可能的泄漏点：MultiAgentOrchestrator 的 WorkflowHandle 列表（每次 SubAgent 完成 push 但忘 remove）、DebugReport 的 trace collector 累积的 LLM 输入输出未截断。设计一个定时器每 5 分钟打印 `jemalloc::stats::stats_print` 到日志。

### BL03 死锁与锁顺序反转
- **预期档位**: hard
- **考察维度**: lockdep + 死锁检测 + 锁顺序规约
- **对话脚本**:
  1. Rust 服务偶发卡死，所有 worker 线程停在 `futex_wait_queue`，CPU 占用降到 0。第一步：发 `SIGQUIT` 给进程（`kill -3 <pid>`，Rust 默认 panic handler 会 dump 所有线程栈），找到两个线程在 `pthread_mutex_lock` 互相等待。
  2. 基于上面的栈，识别锁顺序反转：线程 A 持 lock1 等 lock2，线程 B 持 lock2 等 lock1。Rust 里常见于 `Arc<Mutex<T>>` 跨回调闭包调用，例如 ProviderRegistry 持 config 锁时回调内又去拿 Session 锁。
  3. 修复方案：强制锁顺序（如「先 Session 后 Provider」），把多个锁合并为单个粗粒度锁，或改用 `parking_lot::RwLock`（更便宜但同样死锁）。Rust async 场景特别注意：`tokio::sync::Mutex` 不能跨 `.await` 持锁（容易死锁）。
  4. 加 `tracing` 字段自动记录每次 lock 获取：`#[instrument]` 注解 + `Mutex::lock_with_span`，或集成 `tokio-console` 实时看锁等待图。最后评估 laew 的 `agent_memory` 表读写锁是否需要类似规约。

### BL04 数据竞争与 Heisenbug：只在生产复现
- **预期档位**: hard
- **考察维度**: TSan + 内存模型 + 调试技巧
- **对话脚本**:
  1. 一个并发 Bug 在测试环境跑 1000 次都不出现，**只在生产偶发**（Heisenbug）。第一步：本地用 `cargo test --features tsan` 开 ThreadSanitizer 跑 10000 次（TSan 把竞争检测放慢 5-15 倍但能找到所有 race）。
  2. 基于 TSan 报告，Rust 常见 race 模式：`Arc<RefCell<T>>` 跨线程共享（必须 `Arc<Mutex<T>>` 或 `Arc<RwLock<T>>`）、`Cell` / `UnsafeCell` 在多线程读写（UB）、`static mut` 未加 `Sync` wrapper。给出一段真实误用 `Rc<Vec<T>>` 跨 `tokio::spawn` 的代码。
  3. 生产复现：开启 `MALLOC_CONF="prof:true,prof_active:true"` + jemalloc profile，配置采样率 1/1000，记录 race 时刻的栈；用 `RUST_BACKTRACE=full RUST_LOG=trace` 跑灰度发布，再用 `eBPF` 的 `uprobe` 跟踪关键函数。
  4. Rust 内存模型细节：`Send` / `Sync` 是编译期检查，但 UnsafeCell 内部操作仍是 UB；`OnceCell` 的 `get_or_init` 可能在并发下 double-init（除非用 `LazyLock`）。最后评估 laew 的 SessionContext 在跨 SubAgent 传递时是否需要加 TSan 集成测试。

### BL05 生产 OOM：容器内存限额与堆外内存
- **预期档位**: medium
- **考察维度**: cgroup 内存 + RSS vs 堆外 + OOM Killer 日志
- **对话脚本**:
  1. K8s pod 内存设限 512Mi，但 Rust 服务运行 6 小时后被 OOMKilled（`dmesg | grep -i oom` 看到 `Killed process 12345 (laew) total-vm:... anon-rss:524288kB`）。第一步：区分 RSS（`/proc/{pid}/status` VmRSS）与 cgroup 实际计数（`/sys/fs/cgroup/memory/memory.usage_in_bytes`）。
  2. 基于上面的诊断，常见堆外内存：jemalloc arena（`stats.allocated` vs `stats.resident`）、mmap 大块（tokio 的 buffer pool）、第三方库的 native heap（rusqlite 的 page cache、curl 的 DNS 缓存）。用 `cat /proc/{pid}/smaps` 看每段 VMA 的 RSS / Pss。
  3. 修复方向：j 配 `MALLOC_CONF="narenas:1,background_thread:true"` 减小 arena；调小 tokio `max_blocking_threads`；把 page cache 改成 mmap 落盘；设 RSS 软上限触发主动释放（`mallocx(0, MALLOCX_TRIM)`）。
  4. 给 laew 加 OOM 前置保护：在 `agent_memory` 表加 size 列，每 1 分钟统计缓存大小，超过阈值主动清理 + 写日志；启动时 `setrlimit(RLIMIT_AS, ...)` 软限 RSS（80% cgroup limit）。

### BL06 CPU 打满 100%：从 top 到火焰图
- **预期档位**: medium
- **考察维度**: perf + 火焰图 + 热点函数定位
- **对话脚本**:
  1. 生产服务 CPU 单核 100%（其他核闲置），响应慢 10 倍。第一步：`top -H -p <pid>` 找占用最高的线程 ID（TID），`printf '%x\n' <TID>` 转十六进制。
  2. 基于上面的 TID，用 `perf record -p <pid> -F 99 -g -- sleep 30` 采样 30 秒（每秒 99 次，9 指令窗口），生成 `perf.data`；`perf script | stackcollapse-perf.pl | flamegraph.pl > flame.svg` 生成火焰图。
  3. 看火焰图找到热点：常见于 JSON 序列化（`serde_json` 默认非零拷贝）、正则回溯（灾难性回溯 O(2^n)）、锁竞争自旋、LLM streaming chunk 解析的字符串分配。Rust 优化：换 `simd-json` / `sonic-rs` 提速 3-5 倍，正则改用 `regex_automata` 的 non-backtracking DFA。
  4. 持续监控：`cargo install cargo-flamegraph` 集成到 CI，给 laew 的 e2e 测试加 CPU 采样断言（任何 Agent 循环步骤 CPU > 80% 持续 5s 报警）。最后列出 laew 已知的潜在热点（session_memory 摘要、MultiAgentOrchestrator 拓扑排序）。

### BL07 文件句柄与连接泄漏：fd 耗尽排查
- **预期档位**: medium
- **考察维度**: lsof + ulimit + fd 监控
- **对话脚本**:
  1. Rust 服务报 `Too many open files`（`/var/log/syslog` 里 `OS error 24`）。第一步：`ulimit -n` 看软硬限制（默认 1024，生产应改 65536）；`cat /proc/{pid}/limits` 看进程实际限制。
  2. 基于上面的诊断，`ls -l /proc/{pid}/fd | wc -l` 看当前 fd 数（突破 1024 即快爆）；`lsof -p <pid>` 列所有 fd，按类型统计（socket / pipe / file / anon_inode）。常见泄漏：HTTP 连接未 close、SQLite 连接池未归还、tokio TcpStream 没用 `drop`。
  3. Rust 修复：`reqwest` Client 复用 + 显式 `response.bytes().await?` 后让 response drop；`tokio::net::TcpListener` accept 后没 spawn；`tokio::select!` 缺 `_ = tokio::time::sleep(timeout)` 分支导致 TcpStream 永驻；`tokio::spawn` 的 task 因 channel 阻塞永不结束。
  4. 给 laew 加 fd 监控：每 5 分钟 `lsof -p $$` 统计 socket 数，超过 5000 写日志 + 触发紧急连接池重建（`hyper::client::Client::rebuild`）。最后排查 laew 的 LLM client 是否在 Anthropic / OpenAI 双协议下都有连接泄漏。

### BL08 时钟、时区与闰秒引发的诡异 Bug
- **预期档位**: hard
- **考察维度**: monotonic vs wall clock + tz data + leap second
- **对话脚本**:
  1. 一个分布式日志服务在凌晨 3 点集体错乱 1 小时，原因：俄罗斯时区调整（DST 切换）。第一步：审计所有时间相关代码：`std::time::SystemTime`（wall clock，可跳变）vs `Instant`（monotonic，永不减），二者**绝不能混用做差值**。
  2. 基于上面的诊断，Rust 时间 API 选择：`chrono::Utc::now()` 用于业务时间戳存 DB、`tokio::time::Instant` 用于超时 / 计时、`time::OffsetDateTime` 替代 chrono（更快更安全）。存储用 UTC，显示按用户时区转换。
  3. 闰秒问题：2017 年 1 月 1 日的闰秒让部分 Linux 内核把 `clock_gettime(CLOCK_REALTIME)` 倒回 1 秒，导致基于 wall clock 的定时器触发两次。Google 的解决方案是「smear」：闰秒前 24 小时逐步加 1ms。Rust 项目建议：所有计时全用 `tokio::time::Instant` + `Duration`，不要用 `SystemTime` 做差。
  4. laew 的相关风险：SessionContext 摘要的「最近 3 条」按 `created_at` 排序，如果 DB 时区与系统时区不一致会跨日错位；`-debug` 模式 trace 里时间戳必须 UTC + ISO8601 + 纳秒精度。给 laew 加一个 `chrono::Utc::now()` 替换所有 `Local::now()` 的 audit 报告。

### BL09 乱码与编码地狱：从字节到字形
- **预期档位**: medium
- **考察维度**: charset detection + encoding 转换 + 字形回退
- **对话脚本**:
  1. 一个 CSV 导入工具读 UTF-8 文件显示正常，读 GBK 文件全乱码。第一步：用 `chardetng`（Mozilla 的 Rust 实现）做编码嗅探，看 `Encoding` 字段；区分 BOM（`EF BB BF` UTF-8 / `FF FE` UTF-16LE / `FE FF` UTF-16BE）。
  2. 基于上面的编码识别，用 `encoding_rs::decode(bytes, encoding)` 转换到 UTF-8，统一存到 SQLite TEXT 字段（强制 NOCASE 之外的列都用 `COLLATE BINARY` + UTF-8）。导出时反之。
  3. Rust 字符串边界：C++ 风格 `&str` 是字节切片，`s.len()` 返回**字节数**（不是字符数），`s.chars().count()` 才是字符数；`from_utf8` 严格校验，`from_utf8_lossy` 用 `U+FFFD` 替换错字节。中日韩字符占 3 字节，CJK 表意文字遍历时性能陷阱。
  4. 字形回退问题：emoji `🎉` 4 字节 + ZWJ 序列（`👨‍👩‍👧`）在终端显示宽度可能是 1 也可能是 2，用 `unicode-width` crate 计算列数；CJK 半角 / 全角混排容易错位。给 laew 的 TUI 渲染（crossterm）加一个 `unicode-width` 集成测试，覆盖 100 个 CJK + emoji 边界用例。

### BL10 改一行代码引发的雪崩：变更归因与二分定位
- **预期档位**: hard
- **考察维度**: git bisect + 回归测试 + 变更影响分析
- **对话脚本**:
  1. 上线一个看似无害的小改动（某 Agent 工具的 prompt 措辞微调）后，5 个 e2e 用例失败，3 个性能指标劣化。第一步：`git log --oneline -20` 看最近变更，`git diff HEAD~5 HEAD -- src/` 逐行 review。
  2. 基于上面的 diff，用 `git bisect start; git bisect bad; git bisect good <commit>` 自动二分定位引发回归的提交；每个候选 commit 跑一遍 `bash testReport/run_e2e.sh`，自动化测试结果作为「good / bad」判定。
  3. 找到可疑 commit 后，深入分析：可能是隐藏的 prompt 副作用（某子句让 LLM 多调一次工具）、未考虑的边界条件（空字符串 / Unicode / 大数）、并发时序变化（某 sleep 时间让 race 概率变化）。给出真实案例：`asyncio.gather` 改成 `asyncio.gather(*, return_exceptions=True)` 后下游处理逻辑没改导致的崩溃。
  4. 给 laew 加变更归因机制：每个 SubAgent 任务记录「输入 prompt 哈希 + 工具调用序列 + 关键决策点」，出现回归时通过哈希快速定位是否同一种 prompt 模式；CI 阶段强制跑基线对比（base branch vs 当前分支 e2e 差异 < 5%）。最后给出一个 laew 的真实回归排查纪要：Compact Agent 触发阈值从 80% 改到 75% 后某场景下频繁压缩的归因过程。
