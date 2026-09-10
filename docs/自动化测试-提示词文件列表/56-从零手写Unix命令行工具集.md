# 56 从零手写 Unix 命令行工具集

> 编号段 BE01–BE10 · 聚焦 Unix 核心工具的「动手实现」：流式 I/O / 目录遍历 / 正则匹配 / 表达式求值 / 递归绘制 / 磁盘统计 / 参数批处理 / 差异算法 / 终端控制
> 与现有 05 维「编码Coding与调试修复」互补：05 偏通用编码调试思路；56 偏 Unix 工具集的具体规范与实现细节
> 与现有 04 维「软件与命令行工具使用」互补：04 偏「调用现有命令」；56 偏「用 Rust/Go 从零实现命令的核心算法」

---

### BE01 手写 cat / head / tail：从文件流到 -n 与 -f 跟随
- **预期档位**: medium
- **考察维度**: 流式 I/O + 文件描述符 + inode watch
- **对话脚本**:
  1. 用 Rust 写一个简化版 `cat`：支持 `cat file1 file2 -`（`-` 表示 stdin），按行缓冲输出到 stdout，处理 EPIPE（下游 `head` 提前关闭管道时不能 panic）。
  2. 在上面基础上加 `head` 与 `tail`：默认各 10 行；`head -n 50` / `tail -n +5`（从第 5 行起输出，含 `-c` 字节模式）；注意 `tail -n +5` 是「从 5 开始」不是「最后 5 行」，很多初学者搞混。
  3. 实现 `tail -f`：用 `inotify`（Linux）/ `FSEvents`（macOS）监听文件 inode 的 IN_MODIFY 事件，从上次读取位置继续读；处理「被 truncate 后重建」的情况（stat.st_ino 不变但 size 变小，重置偏移）。
  4. 用同样思路给 laew 加 `/show 路径` 斜杠命令：复用 `head`/`tail` 的逻辑让用户在 TUI 内分页查看长文件，按 `f` 进入 follow 模式（适合看 laew 自己的 DebugReport 实时增长）。

### BE02 手写 ls：目录遍历、权限位与多列对齐
- **预期档位**: medium
- **考察维度**: getdents64 + termios + 列宽计算
- **对话脚本**:
  1. 用 Rust 写 `ls`：调用 `getdents64` syscall（绕过 libc dirent 的名字长度 255 限制），处理 `.` / `..` 过滤、隐藏文件（`-a`）、按 mtime 排序（`-t`）。
  2. 解析 Unix 权限位：`mode & 0o777` 得到 9 位三元组，转 rwxrwxrwx 字符串（还要识别 SUID/SGID/Sticky 三种特殊位，分别用 `s`/`t` 替换对应位置的 `x`），给一段格式化函数。
  3. 多列对齐：根据终端宽度（用 `ioctl(TIOCGWINSZ)` 获取 col）与最长文件名计算列数；CJK 字符按 2 列宽（用 `unicode-width` crate）；输出方向 `-x`（按列填）vs 默认（按行填）的差异。
  4. 给 laew 的 TUI ProviderList 屏做参考：它需要按列对齐显示「协议 / 提供商 / 模型 / 端点 / 密钥末四位」5 列，复用 ls 的列宽算法 + termios 探测就能避免 CJK 截断。

### BE03 手写 wc：字节/字符/单词/行的四种计数口径
- **预期档位**: simple
- **考察维度**: 字节 vs Unicode 字符 + 状态机
- **对话脚本**:
  1. 用 Rust 写 `wc -l`：最简单，按 `\n` 计数（注意：末尾无换行的最后一行算不算？GNU wc 算 1 行，POSIX 严格定义无统一答案，要可配置）。
  2. 加 `-c`（字节）与 `-m`（字符）：`-c` 直接 `len(file)`；`-m` 需要按 Unicode 码点遍历（不要按 UTF-8 字节），CJK/Emoji 一个 codepoint 算一个字符，处理 broken UTF-8 时降级为字节。
  3. 加 `-w`（单词）：POSIX 定义「非空白字符组成的最大序列」；要处理 Unicode 空白（用 `unicode-general-category` 判定 White_Space），否则中文标点会被错误切词。
  4. 实现 `-L`（最长行长）：按字符列宽（不是字节数）统计，CJK 算 2 列、Tab 按下一个 8 的倍数对齐；这个口径在生成 `laew -h` 输出与 TUI 排版时很实用。

### BE04 手写 grep：正则匹配、上下文行与彩色高亮
- **预期档位**: medium
- **考察维度**: NFA/DFA 正则 + ANSI 转义 + 多文件并行
- **对话脚本**:
  1. 用 Rust 写 `grep`：先支持 literal（普通字符串）匹配，KMP 或 Boyer-Moore-Horspool 都行；再切换到正则（用 `regex` crate 的 DFA 后端，避免回溯爆栈）。
  2. 加 `-C 3`（前后 3 行上下文）、`-B 2 -A 5`、`--color=auto`（输出到 tty 才着色，重定向到管道不染色，避免污染日志）；颜色用 ANSI `\x1b[01;31m`（红粗体）+ `\x1b[0m` 重置，匹配部分高亮，行尾单独重置避免泄露到下一行。
  3. 多文件 `grep -rn pattern dir`：用 `walkdir` 递归 + `rayon` 并行（per-file 任务独立），性能可提升 4-8 倍；处理 SIGPIPE（多个文件时提前退出别浪费 CPU）。
  4. 给 laew 的 SessionContext Agent 设计一个 `grep_session.sh`：在 `session_memory` 表里全文搜索历史摘要关键词，复用 grep 的高亮与上下文输出逻辑，给 TUI 主屏的「查找历史任务」功能打底。

### BE05 手写 find：表达式求值与目录剪枝
- **预期档位**: hard
- **考察维度**: 表达式 AST + 剪枝优化 + xattr
- **对话脚本**:
  1. 用 Rust 写 `find` 的表达式解析：`find . -name '*.rs' -size +1M -mtime -7 -print` 是布尔表达式，需要先词法分析再递归下降建 AST（`-name` `-size` `-mtime` 是叶子，`-a` 隐式 AND / `-o` OR / `!` NOT 是内部节点）。
  2. 表达式求值 + 目录剪枝：在 walkdir 遍历时每访问一个 entry 调用 `eval(ast, entry)`；剪枝是关键：`-name 'node_modules'` 命中后整个子树不递归，否则大规模目录树性能崩盘。
  3. 实现 `-exec`：`find ... -exec cmd {} \;` 要把 `{}` 替换为路径，处理路径含空格的情况（GNU 用 `\;\+` 把多文件合并到一次 exec 调用，减少 fork 次数）；以及 `-prune`（剪枝 + 不打印）与 `-print` 的默认行为差异。
  4. 给 laew 的 Bash 工具加白名单：`BashTool` 调用前先用 find 风格的表达式扫描用户要执行的命令意图（如 `git status` 在哪个仓库、`cargo test` 哪个 crate），比纯字符串匹配更稳。

### BE06 手写 tree：递归绘制与 Unicode 制表符
- **预期档位**: medium
- **考察维度**: 深度优先 + UTF-8 box-drawing + ANSI 文件类型色
- **对话脚本**:
  1. 用 Rust 写 `tree`：深度优先遍历目录，输出形如 `├── src/ → agent/ → mod.rs`；绘制符号默认 ASCII（`|--` `+--` `|`），加 `-N` 用 Unicode box-drawing（`├──` `└──` `│  `）。
  2. 处理最后一层 vs 中间层的视觉差异：父节点用 `├──` 链接中间子项，用 `└──` 链接最后一个子项；递归时给下一层传「父前缀」字符串（`│   ` 或 `    `），错位就乱套。
  3. 加 `-L 2`（深度限制）、`-I 'target'`（忽略模式）、`-F`（目录加 `/`、可执行加 `*`、symlink 加 `@`）；颜色按文件类型分（蓝=目录、绿=可执行、品红=图片），通过 `LS_COLORS` 环境变量读取。
  4. 给 laew 的 `-p` 单轮模式加 `--tree` 选项：执行任务前先把工作目录结构打印出来作为上下文，让 LLM 看到项目骨架（比单纯 cd 后 ls 更高效）。

### BE07 手写 du 与 df：磁盘占用统计与硬链接去重
- **预期档位**: medium
- **考察维度**: statfs + inode 引用计数 + 并行聚合
- **对话脚本**:
  1. 用 Rust 写 `df`：调用 `statfs`/`statvfs` syscall 获取文件系统总块、空闲块、可用块；注意 `f_bavail`（普通用户可用）vs `f_bfree`（root 可见空闲）的差异，5% 预留空间不要算进去。
  2. 写 `du -sh dir`：递归遍历统计 `st_blocks * 512`（注意是 512 字节为单位，不是 `st_size`！一个空文件也可能占 4KB）；并行版用 rayon，每个目录独立 stat 后聚合，避免单线程 IO 瓶颈。
  3. 硬链接去重是关键：`du` 默认会把同一 inode 多次出现的目录项都算进去，导致共享子目录被重复计算；GNU du 用 `-l` 关闭去重（默认开启），即用 HashSet 记录见过的 dev+inode。
  4. 给 laew 加 `/disk` 斜杠命令：在 TUI 状态栏显示根目录与 SQLite 库的占用，配合 Compact Agent 的「按比例触发压缩」决策（占盘 > 80% 时主动压缩 session_memory）。

### BE08 手写 xargs：参数分批与并行执行
- **预期档位**: medium
- **考察维度**: ARG_MAX 探测 + 引号转义 + 并行调度
- **对话脚本**:
  1. 用 Rust 写 `xargs`：核心是「一行 stdin 算一个参数」还是「按空白切分多个参数」（默认行为），GNU 用 `-d` 改分隔符；遇到引号要正确处理（`'a b'` 算一个参数，含空格的引号字符串）。
  2. 参数分批：一次性构造 argv 受限于 `ARG_MAX`（Linux 通常 128KB-2MB，用 `sysconf(_SC_ARG_MAX)` 探测），超过就拆成多轮 exec；新版 `-n 1` 强制每参数一次调用。
  3. 实现 `xargs -P 4 -I {} cmd {}`：用线程池并行 4 个 worker，每个 worker 持有一个子进程；`-I {}` 把 `{}` 替换为参数（每条独立调用，不分批）。
  4. 用同样的分批模式给 laew 的 MultiAgentOrchestrator 改造：当 hard 任务的 Workflow 列表超过「单轮 LLM 调用上限」（如 50 个）时分批投喂，每批完成后等 Quality-Check 再投喂下一批。

### BE09 手写 diff：最长公共子序列与补丁输出
- **预期档位**: hard
- **考察维度**: LCS DP + Myers diff + hunk 格式
- **对话脚本**:
  1. 用 Rust 写 `diff`：最朴素版用动态规划算最长公共子序列（LCS），时间空间 O(mn) 太占内存；改成 Myers diff（O((m+n)d)，d 是编辑距离），Git 内部用的就是这个算法。
  2. 输出格式：普通模式（`<` `>` `|`）、unified diff（`--- a/f +++ b/f @@ -1,3 +1,4 @@` + ` ` `-` `+` 三种前缀）；hunk header 的 `-1,3` 表示「原文件第 1 行起 3 行」，合并相邻 hunk 时默认上下文 3 行。
  3. 大文件优化：用 patience diff（先匹配唯一行锚点，再 Myers）显著减少 hunk 数；Git 默认就是 patience diff；对超大文件再加 suffix array 或 kdiff3 三方合并算法。
  4. 给 laew 的 SessionContext 摘要做版本对比：每次任务后存一份「上轮摘要 vs 本轮摘要」的 unified diff，方便用户回溯「这次会话改了什么」，复用 Myers diff 实现避免引入额外依赖。

### BE10 手写 watch 与 top：终端定时刷新与光标控制
- **预期档位**: medium
- **考察维度**: termios raw mode + 信号处理 + ANSI 局部重绘
- **对话脚本**:
  1. 用 Rust 写 `watch -n 2 cmd`：核心是「隐藏光标 + 保存当前行 + 移到首行 + 清除到末尾 + 执行 cmd + 输出」循环；用 `write(STDOUT, "\x1b[?25l\x1b[H\x1b[J", ...)` 控制终端；恢复时务必 `\x1b[?25h` 显示光标（panic 也要做）。
  2. 处理信号：`SIGWINCH`（终端尺寸变化）需要重抓 col/row 重新布局；`Ctrl+C`（SIGINT）要恢复光标再退出，否则终端留下乱码；用 `crossterm` 的 raw mode 比直接 termios 安全。
  3. 写 `top`：从 `/proc/[pid]/stat` 与 `/proc/[pid]/status` 读取 CPU/内存占用，每秒重采样，进程列表按 CPU 倒序；CPU 占用算法要处理「上一个采样点没数据」的情况，用 `/proc/stat` 总时间差分。
  4. 给 laew 的 TUI 主屏加「实时状态栏」：复用 watch 的光标控制 + ANSI 重绘，显示当前活跃 Agent / Token 用量 / SQLite 写入速率 / 上下文占用百分比，1Hz 刷新，不打断主输入框。
