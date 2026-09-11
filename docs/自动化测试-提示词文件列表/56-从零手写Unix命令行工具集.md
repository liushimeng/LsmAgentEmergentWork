# 56 从零手写 Unix 命令行工具集

> 编号段 BE01–BE10 · 聚焦 Unix 核心工具的「动手实现」：流式 I/O / 目录遍历 / 正则匹配 / 表达式求值 / 递归绘制 / 磁盘统计 / 参数批处理 / 差异算法 / 终端控制
> 与现有 05 维「编码Coding与调试修复」互补：05 偏通用编码调试思路；56 偏 Unix 工具集的具体规范与实现细节
> 与现有 04 维「软件与命令行工具使用」互补：04 偏「调用现有命令」；56 偏「用 Rust/Go 从零实现命令的核心算法」

---

### BE01 手写 cat / head / tail：从文件流到 -n 与 -f 跟随
- **预期档位**: medium
- **考察维度**: 流式 I/O + 文件描述符 + inode watch
- **对话脚本**:
  1. 用 python3 写 `tmpPlan/agent-test/unix/cat.py`，支持 `python3 cat.py file1 file2 -`（`-` 表示 stdin），按行缓冲输出，实现 `cat file1 file2` 拼接、`-` 时从 `sys.stdin` 读。
  2. 在上面基础上加 `tmpPlan/agent-test/unix/head.py` 与 `tail.py`：默认各 10 行；`head -n 50` / `tail -n +5`（从第 5 行起输出，注意不是最后 5 行）；`Bash` 用 `seq 1 20 | python3 head.py -n 5` 断言恰好前 5 行 1-5。
  3. `Bash` 用真命令对照：`diff <(seq 1 20 | python3 head.py -n 5) <(seq 1 20 | head -n 5)` 必须空 diff；`diff <(seq 1 20 | python3 tail.py -n +15) <(seq 1 20 | tail -n +15)` 必须空 diff（15-20 共 6 行）。
  4. 故意把 `tail.py -n +X` 实现错成 `tail -N`(最后 N 行)，跑 `seq 1 20 | python3 tail.py -n +15` 期望输出 15-20，但实际输出 1-15，diff 非空，定位 bug（`+X` 语义 vs 末尾 N 行）修正后 diff 空。

### BE02 手写 ls：目录遍历、权限位与多列对齐
- **预期档位**: medium
- **考察维度**: getdents64 + termios + 列宽计算
- **对话脚本**:
  1. 用 python3 写 `tmpPlan/agent-test/unix/ls.py`：调用 `os.listdir` 与 `os.stat`，实现 `-a`(含隐藏文件)、`-t`(按 mtime 倒序) 两个选项；输出每行 `name<tab>mtime<tab>size`。
  2. `Bash` 跑 `python3 ls.py -a -t .` 与 `ls -a -t` 对比：`diff <(python3 ls.py -a -t . | awk -F'\t' '{print $1}' | head) <(ls -a -t | head)` 验证排序结果一致。
  3. 解析 Unix 权限位：`mode & 0o777` 转 `rwxrwxrwx` 字符串（SUID/SGID/Sticky 用 `s`/`t` 替换对应 `x`），`Bash` 跑 `python3 ls.py -a . | awk -F'\t' 'NR==1{print $4}'` 断言第一个文件权限串是 10 字符（含首字符 `d`/`-`）。
  4. 故意在权限函数里把 `r` 写成 `R`（大写），跑 `diff` 与真 `ls -l` 对照发现字母不符，修正后 `diff <(python3 ls.py . | awk -F'\t' '{print $4}') <(ls -l | awk '{print $1}')` 断言首 5 行一致。

### BE03 手写 wc：字节/字符/单词/行的四种计数口径
- **预期档位**: simple
- **考察维度**: 字节 vs Unicode 字符 + 状态机
- **对话脚本**:
  1. 用 python3 写 `tmpPlan/agent-test/unix/wc.py`：`-l` 行数、`-c` 字节数、`-m` Unicode 字符数、`-w` 单词数；默认输出 `l w c`。
  2. `Bash` 现场造数据：`printf 'hello 世界\n你好 python3\n' > /tmp/wc_test.txt`，跑 `python3 wc.py /tmp/wc_test.txt` 与 `wc /tmp/wc_test.txt` 对比，`diff <(python3 wc.py /tmp/wc_test.txt) <(wc /tmp/wc_test.txt)` 必须空 diff。
  3. 故意把 `-m` 实现成字节数(与 `-c` 相同)，跑 `python3 wc.py -m /tmp/wc_test.txt` 与 `wc -m` 对比 diff 非空，定位 bug 后用 `len(text.encode('utf-8'))` vs `len(text)` 区分，修正后 diff 空。
  4. 加 `-L` 最长行长（按字符列宽，CJK 算 2 列），`Bash` 跑 `python3 wc.py -L /tmp/wc_test.txt` 与手动 `awk '{if(length>max) max=length} END{print max}'` 验证，写入 `wc.md` 列四种口径公式。

### BE04 手写 grep：正则匹配、上下文行与彩色高亮
- **预期档位**: medium
- **考察维度**: NFA/DFA 正则 + ANSI 转义 + 多文件并行
- **对话脚本**:
  1. 用 python3 写 `tmpPlan/agent-test/unix/grep.py`：先支持字面量匹配（KMP 或 `in` 即可），`Bash` 造含 1000 行的测试文件 `seq 1 1000 > /tmp/g.txt; echo "needle" >> /tmp/g.txt`，跑 `python3 grep.py needle /tmp/g.txt` 断言只输出含 needle 的 1 行。
  2. 加 `-C N` 前后 N 行上下文、`--color=auto`（只在 tty 输出 ANSI `\x1b[01;31m` 红色 + `\x1b[0m` 重置），`Bash` 跑 `python3 grep.py -C 1 needle /tmp/g.txt` 断言输出 3 行（上+匹配+下）。
  3. `Bash` 与真 grep 对照：`diff <(grep -n needle /tmp/g.txt) <(python3 grep.py -n needle /tmp/g.txt)` 必须空 diff，否则调整正则转义后重跑。
  4. 故意在颜色输出里忘了重置（匹配后没有 `\x1b[0m`），把输出 `| cat -A` 会看到颜色码泄露到下一行（`^[` 序列），定位 bug 后加重置再跑 `grep -c '\x1b\[0m'` 验证每次匹配后都有重置码。

### BE05 手写 find：表达式求值与目录剪枝
- **预期档位**: hard
- **考察维度**: 表达式 AST + 剪枝优化 + xattr
- **对话脚本**:
  1. 用 python3 写 `tmpPlan/agent-test/unix/find.py`：解析 `find . -name '*.py' -size +1M -mtime -7 -print`，支持 `-name`/`-size`/`-mtime`/`-print` 叶子与 `-a`(AND)、`-o`(OR)、`!`(NOT) 内部节点。
  2. `Bash` 造测试树：`mkdir -p /tmp/ft/a/b; echo "x" > /tmp/ft/a/big.py; python3 find.py /tmp/ft -name '*.py' -print` 断言只输出 `.py` 文件。
  3. `Bash` 与真 find 对照：`diff <(python3 find.py /tmp/ft -name '*.py' -print | sort) <(find /tmp/ft -name '*.py' -print | sort)` 必须空 diff，否则调整通配匹配逻辑后重跑。
  4. 故意把 `-size +1M` 实现成 `>1K`（把单位 M 误按 K 算），跑 `diff` 与真 `find ... -size +1M` 不一致，定位单位换算 bug（1M=1024*1024）修正后 diff 空；加 `-exec` 占位（打印 `exec path` 即可）。

### BE06 手写 tree：递归绘制与 Unicode 制表符
- **预期档位**: medium
- **考察维度**: 深度优先 + UTF-8 box-drawing + ANSI 文件类型色
- **对话脚本**:
  1. 用 python3 写 `tmpPlan/agent-test/unix/tree.py`：深度优先遍历目录，输出 `├──` / `└──` / `│   ` 三符号；支持 `-L N` 深度限制。
  2. `Bash` 造测试树：`mkdir -p /tmp/tt/{src,test}; touch /tmp/tt/src/{a.py,b.py}; python3 tree.py -L 2 /tmp/tt`，断言输出 1 行目录名 + 2 行一级子目录。
  3. `Bash` 与真 tree 对照：`diff <(python3 tree.py -L 1 /tmp/tt) <(tree -L 1 /tmp/tt | tail -n +2 | head -n -1)` 必须空 diff（去除 tree 首行目录名与末行计数）。
  4. 故意把 `├──` 与 `└──`（4 长）写错，`diff` 与真 tree 不一致，`Bash` 跑 `grep -cE '├' tree_out.txt` 断言符号计数，修正后 diff 空；加 `-I 'test'` 忽略模式。

### BE07 手写 du 与 df：磁盘占用统计与硬链接去重
- **预期档位**: medium
- **考察维度**: statfs + inode 引用计数 + 并行聚合
- **对话脚本**:
  1. 用 python3 写 `tmpPlan/agent-test/unix/du.py`：递归统计 `st_blocks * 512`（注意不是 `st_size`），支持 `-h`（人类可读）、`-s`（仅汇总）。
  2. `Bash` 造测试树：`mkdir -p /tmp/du1/sub; dd if=/dev/zero of=/tmp/du1/sub/f.bin bs=1024 count=10 2>/dev/null; python3 du.py -s /tmp/du1`，断言输出 `>= 10K`。
  3. `Bash` 与真 du 对照：`diff <(python3 du.py -s /tmp/du1) <(du -s /tmp/du1)` 允许 ±10% 误差（`awk` 数值比较），否则检查 `st_blocks` 用法。
  4. 硬链接去重：`ln /tmp/du1/sub/f.bin /tmp/du1/sub/f2.bin` 造硬链接，`python3 du.py -s --no-dedup /tmp/du1` 与 `python3 du.py -s /tmp/du1` 对比，后者应更小（去重生效），`diff` 非空证明去重逻辑工作。

### BE08 手写 xargs：参数分批与并行执行
- **预期档位**: medium
- **考察维度**: ARG_MAX 探测 + 引号转义 + 并行调度
- **对话脚本**:
  1. 用 python3 写 `tmpPlan/agent-test/unix/xargs.py`：默认按空白切分参数、支持 `-I {}` 占位符替换、`-n 1` 每参数一条命令。
  2. `Bash` 造测试：`printf 'a b c\nd e\n' | python3 xargs.py -n 2 echo` 断意输出 3 行（a b / c d / e），与 `printf 'a b c\nd e\n' | xargs -n 2 echo` diff 空。
  3. 故意把 `-I {}` 实现成不替换（直接忽略 `{}`），跑 `echo 'foo' | python3 xargs.py -I {} echo prefix-{}` 期望输出 `prefix-foo`，实际输出 `prefix-{}`，diff 非空，修正替换逻辑后 diff 空。
  4. 加 `-P 2` 并行（`ThreadPoolExecutor(max_workers=2)`），`Bash` 跑 `seq 1 4 | python3 xargs.py -P 2 -I {} sleep 0.1; echo done` 断言 4 任务并行 2 路总耗时 ≈ 0.2s（`time` 验证）。

### BE09 手写 diff：最长公共子序列与补丁输出
- **预期档位**: hard
- **考察维度**: LCS DP + Myers diff + hunk 格式
- **对话脚本**:
  1. 用 python3 写 `tmpPlan/agent-test/unix/diff.py`：先实现朴素 LCS（O(mn) DP）输出相同行与差异行，支持 `-u` unified 格式。
  2. `Bash` 造测试文件：`printf 'A\nB\nC\nD\n' > /tmp/d1.txt; printf 'A\nX\nC\nD\n' > /tmp/d2.txt; python3 diff.py -u /tmp/d1.txt /tmp/d2.txt` 断言 hunk 含 `-B` + `+X`。
  3. `Bash` 与真 diff 对照：`diff <(python3 diff.py -u /tmp/d1.txt /tmp/d2.txt) <(diff -u /tmp/d1.txt /tmp/d2.txt)` 必须空 diff（hunk 行号与内容一致），否则调整上下文行数后重跑。
  4. 故意把 `-u` 的上下文行数固定成 0（不输出上下文），`diff` 与真 `diff -u` 不一致，定位 bug 后改成默认 3 行上下文，diff 空；再跑 `seq 1 100` 大文件 100 行测试，断言无超时。

### BE10 手写 watch 与 top：终端定时刷新与光标控制
- **预期档位**: medium
- **考察维度**: termios raw mode + 信号处理 + ANSI 局部重绘
- **对话脚本**:
  1. 用 python3 写 `tmpPlan/agent-test/unix/watch.py`：`watch.py -n 1 cmd` 每秒清屏执行命令输出，用 `print("\x1b[H\x1b[J")` 清屏，`print("\x1b[?25l")` 隐藏光标，退出时 `\x1b[?25h` 显示。
  2. `Bash` 后台跑 `python3 watch.py -n 1 'date +%T'` 3 秒后 kill，`tmpPlan/agent-test/unix/trace.txt` 记录输出，`grep -c ':' trace.txt` 断言至少 2 个时间戳。
  3. 故意忘记退出时显示光标，跑 `python3 watch.py -n 100 'echo test'` 后 Ctrl-C，终端光标消失；在脚本末尾加 `finally: print("\x1b[?25h", end='')` 重跑验证光标恢复。
  4. `Write` `top.py` 用 `os.listdir('/proc')` + 读 `/proc/[pid]/stat` 实现简化版 top，每 2 秒刷新，按 CPU 倒序打印前 5 进程，`Bash` 跑 `python3 top.py` 断言 5 行进程 + 1 行表头；`Write` `watch_top.md` 总结 ANSI 控制序列清单（清屏/定位/显隐光标/颜色）。