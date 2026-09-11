# 117 跨平台 Shell 脚本移植与健壮性工程

> 编号段 DI01–DI10 · 聚焦「把一份 Shell 脚本安全地跑在多种操作系统上」：GNU 与 BSD 工具差异 / bash·zsh·fish 方言 / POSIX 可移植子集 / shebang 与换行符陷阱 / trap 错误处理链 / 参数解析惯例 / 子 shell 变量丢失 / bats 测试 / PowerShell 对译 / 分发与环境锁定
>
> 与现有维度互补说明：
> - `04-软件与命令行工具使用`（D 维）聚焦**工具怎么用**；本文件聚焦**脚本怎么跨平台写对**。
> - `36-自动化办公与脚本工程`的 AK06 聚焦**单个 Bash 脚本工程化**（shellcheck / `set -euo pipefail` 入门 / Shell vs Python 取舍）；本文件聚焦**跨发行版、跨 shell、跨操作系统的移植性**与更深的 trap / 子 shell 语义，不重复 shellcheck 基础。
> - `66-跨平台电脑使用与虚拟化实战`（BO 维）聚焦**平台使用**（PowerShell 日常 / WSL2 / macOS）；本文件的 PowerShell 条目聚焦 **bash→PowerShell 对译工程**，视角相反。
> - `40-正则表达式与文本处理工程`的 AO07 聚焦 **grep/sed/awk 文本处理技巧**；本文件聚焦 sed/date 等工具的**平台差异与可移植替写**。
>
> **本文件独特主题**：GNU vs BSD 命令差异地图 / bash·zsh·fish 方言兼容 / POSIX sh 子集 / CRLF 与 shebang 陷阱 / trap 清理链 / getopts 与 CLI 惯例 / 管道子 shell 变量丢失 / bats 测试与 Shell CI / bash→PowerShell 对译 / 脚本分发与环境锁定。

---

### DI01 GNU 与 BSD 命令差异地图

- **预期档位**: medium
- **考察维度**: coreutils 差异 / 可移植替写 / 差异探测
- **对话脚本**:
  1. 同一条 `sed -i 's/a/b/' file` 在 Debian 和 macOS 上行为完全不同：GNU 的 `-i` 可不带参数、BSD 的 `-i` 必须跟备份后缀；列出 sed / date / readlink / stat / cp / head 这六个命令在 GNU（Linux）与 BSD（macOS）下的典型差异。
  2. 针对上面的差异逐条给出可移植写法：`sed` 用临时文件重写、`date` 加载判断 `-v` 还是 `-d`、`readlink -f` 用 `realpath` 或循环 `cd` 替代；解释"探测一次、缓存到变量"的写法为什么要优于到处写 `if [[ $(uname) == Darwin ]]`。
  3. 基于上面的探测思路，写一个 `portable_env_detect` 函数骨架：探测 OS、包管理器、coreutils 前缀（macOS 上 `gsed`/`gdate` 的 Homebrew 命名），并导出 `SED`、`DATE` 等命令变量供后续使用。
  4. 动手：把 `ls -1 *.log | head -n 3 | xargs gzip` 改写成同时兼容 GNU/BSD 的版本，并用 Docker 起 alpine（BusyBox！）镜像验证 BusyBox 又是第三种行为，记录差异表。

---

### DI02 bash / zsh / fish 方言差异与兼容策略

- **预期档位**: medium~hard
- **考察维度**: shell 方言 / 数组与分词 / 兼容边界
- **对话脚本**:
  1. 为什么"在交互终端 zsh 里试得好好的脚本，放进 CI 的 `/bin/sh` 就炸"？从数组下标（bash 从 0、zsh 默认从 1）、字符串分词（zsh 默认不做 word splitting）、`==` 与 `=` 模式匹配三个例子讲清方言差异。
  2. 针对上面的差异给出工程策略：脚本一律声明 `#!/usr/bin/env bash` 并 `set -euo pipefail`；数组的写法差异如何用显式 `"${arr[@]}"` 规避；fish 根本不是 POSIX 后代，为什么"给 fish 写脚本"通常应转译而不是翻译。
  3. 基于上面的结论，讨论"公司里同事默认 shell 五花八门"的现实：发布给团队的脚本应该假设什么解释器？`#!/usr/bin/env bash` 在 Nix、Homebrew、WSL 上的解析路径有何不同？
  4. 动手：写一个 `countdown.sh`，在 bash 3.2（macOS 自带）与 bash 5 上都可用——不用关联数组、不用 `${var,,}` 小写展开，列出你为了兼容 bash 3.2 砍掉了哪些语法。

---

### DI03 POSIX sh 可移植子集编程

- **预期档位**: medium
- **考察维度**: POSIX 子集 / 可移植构造 / 何时值得
- **对话脚本**:
  1. POSIX sh 是"最小公共分母"：解释 `local` 并非 POSIX 标准、`[[ ]]` 是 bash/ksh 扩展、`echo -n` 行为不可指定；给出每条的 POSIX 替代写法（子函数、`case`/`test`、`printf`）。
  2. 针对上面的替代写法，讨论可移植性的代价：没有数组怎么办（`set --` 位置参数当数组用）、没有管道失败检测怎么办；用 `set -- a b c; for x do echo "$x"; done` 展示这一惯用法。
  3. 基于上面的对比，建立决策树：什么场景值得写纯 POSIX sh（Docker ENTRYPOINT、initramfs、OpenBSD rc 脚本），什么场景应果断换语言；解释"Alpine 镜像默认没有 bash"对 Dockerfile 里脚本选择的影响。
  4. 动手：把一段 30 行的 bash 脚本（含数组、`[[ ]]`、进程替换）降级为 dash 可执行的 POSIX 版本，用 `dash -n` 与 `shellcheck --shell=sh` 验证，并总结降级过程中最痛的三处改动。

---

### DI04 shebang、换行符与编码陷阱

- **预期档位**: simple~medium
- **考察维度**: shebang 解析 / CRLF / 文件编码
- **对话脚本**:
  1. `#!/bin/bash` 与 `#!/usr/bin/env bash` 差在哪？为什么带空格的安装路径（如 macOS 的 `/Applications/My Tools/bin`）能击毁固定路径 shebang；解释内核对 shebang 行"只拆一个参数"的规则。
  2. 针对上面的解析规则，进入换行符陷阱：Windows 上编辑的脚本带 CRLF，报错 `\r: command not found`；解释为什么错误信息里的 `\r` 几乎不可见，以及 `.gitattributes` 里 `*.sh text eol=lf` 如何在仓库层面根治。
  3. 基于上面的两个坑，整理"脚本跨机器跑不起来"的第一时间排查清单：`file` 看编码、`cat -A` 看 CRLF、`which bash` 看解释器、`bash -x` 看执行轨迹；再补充 UTF-8 BOM 在老版本 bash 上的坑。
  4. 动手：写一个 `scriptdoctor` 函数：输入脚本路径，自动检测 CRLF、BOM、shebang 是否存在、解释器是否安装、文件是否可执行，输出诊断报告并给出修复命令（`dos2unix`、`chmod +x`）。

---

### DI05 trap 错误处理链与清理顺序

- **预期档位**: hard
- **考察维度**: trap 语义 / 清理链 / ERR 陷阱边界
- **对话脚本**:
  1. 临时文件用完就删：`trap 'rm -f "$tmp"' EXIT` 的执行时机覆盖正常退出、`set -e` 触发的失败、Ctrl-C 吗？分别解释 EXIT / INT / TERM / ERR 四个信号或伪信号的触发条件与典型用途。
  2. 针对上面的基础，深入两个经典坑：(a) ERR 陷阱在 `if` 条件、`&&` 链、管道非最后一段里**不触发**；(b) 同一 trap 重复注册会覆盖而非追加——如何在保留前一个清理动作的前提下追加清理（读取 `trap -p` 再拼接）。
  3. 基于上面的坑，设计一个"分层清理栈"：`push_cleanup 'umount /mnt/iso'`、`push_cleanup 'rm -rf "$tmpdir"'`，退出时后进先出执行；讨论它与 `trap` 单槽覆盖的本质矛盾以及为什么需要这个抽象。
  4. 动手：实现 `push_cleanup` / `pop_cleanup` / 错误行号上报（`$LINENO` 与 `caller` 内建），写一个使用示例：挂载临时镜像 → 复制文件 → 任一步失败都完整回滚，并构造三个失败注入点验证清理顺序。

---

### DI06 参数解析与 CLI 惯例

- **预期档位**: medium
- **考察维度**: getopts / 长选项 / POSIX CLI 约定
- **对话脚本**:
  1. 命令行脚本的"民间标准"：`-h/--help`、`-v` 与 `--verbose` 撞车时怎么办、`--` 分隔符的作用、参数后接值的三种形态（`-ofile` / `-o file` / `--output=file`）；对照 `ls`、`tar`、`curl` 的真实行为说明 GNU 风格约定。
  2. 针对上面的约定，用 `getopts` 实现短选项解析：`while getopts "vo:" opt` 循环、`$OPTARG` 取值、`$OPTIND` 之后 `shift $((OPTIND-1))` 收集位置参数；说明 getopts 为什么**不支持长选项**以及 `vo:` 中冒号位置的含义。
  3. 基于上面的 getopts 版本，手写一个长选项解析器：处理 `--output=x`、`--output x`、未知选项报错并指向 usage、`--` 终止选项扫描；对比两种实现的代码量与健壮性，讨论何时该换 argparse（Python）而不是硬撑。
  4. 动手：给脚本加规范 usage 与退出码约定（0 成功 / 2 用法错误 / 3 运行时错误），实现 `usage()` 函数自动从注释生成；验证 `mytool --help | head -5` 与 `mytool -q 2>&1; echo $?` 的行为。

---

### DI07 管道子 shell 变量丢失与进程替换

- **预期档位**: hard
- **考察维度**: 子 shell 边界 / 进程替换 / 数据传递
- **对话脚本**:
  1. 经典陷阱：`cat list | while read -r line; do count=$((count+1)); done; echo $count` 永远输出 0——解释管道右侧是子 shell、变量修改不回传的原理；画出父子进程与变量副本的关系图。
  2. 针对上面的原理给出四种修复：进程替换 `while ... done < <(cat list)`、here-string 重定向、把结果写文件再读、用 `last` 命令数组累积（`mapfile -t arr < file`）；说明 `mapfile`/`readarray` 为何是 bash 4+ 的正解。
  3. 基于上面的进程替换，扩展它在跨平台的另一面：`<(...)` 在 macOS bash 3.2 可用但在 POSIX sh、dash、busybox sh 里不可用；给出"进程替换降级"策略（FIFO 命名管道 + trap 清理，或临时文件）。
  4. 动手：实现 `csv_stats.sh`：流式读取 CSV 统计行列数与某列汇总，要求不把整个文件读进内存、兼容 bash 3.2；用 `yes | head -c 20m` 生成 2000 万字符测试流式处理时的内存占用（`/usr/bin/time -v` 观察 RSS）。

---

### DI08 bats 单元测试与 Shell CI

- **预期档位**: medium~hard
- **考察维度**: bats 框架 / 命令 mock / CI 门禁
- **对话脚本**:
  1. Shell 脚本也能写单元测试：bats 的 `@test` 块、`run` 捕获输出、`[ "$status" -eq 0 ]` 断言三件套；为一个 `slugify()` 函数写第一个测试，说明 bats 如何把 stdout/stderr/status 收进 `$output`/`$status`。
  2. 针对上面的第一个测试，处理"被测函数内部调用了外部命令"的情况：把 `date`、`curl` mock 掉——用测试专用 `bin/` 目录 + `PATH` 前置注入假命令，对比 mock 命令与 mock 函数两种手法的适用边界。
  3. 基于上面的 mock 手法，补齐负面用例：非法参数应退出码 2、输出用法到 stderr；讨论 bats 的 `setup`/`teardown` 与临时目录（`mktemp -d` + trap 清理）的配合惯例。
  4. 动手：为 DI07 的 `csv_stats.sh` 写 5 个 bats 用例（空文件 / 表头 / 带引号字段 / CRLF / 大文件抽样），并写一段 GitHub Actions：矩阵跑 `ubuntu + macos` 两个平台的 bats，失败时上传失败用例名。

---

### DI09 bash→PowerShell 对译工程

- **预期档位**: medium
- **考察维度**: 文本流 vs 对象流 / 错误流 / 对译方法论
- **对话脚本**:
  1. 两种哲学的根本分叉：bash 管道流的是**文本**、一切靠解析；PowerShell 管道流的是**对象**、属性直接点出来；用"取占用空间前 5 的目录"同一个任务，写出 `du | sort | head` 与 `Get-ChildItem | Sort-Object Length -Desc | Select -First 5` 的对比例子。
  2. 针对上面的分叉，对译错误处理：bash 的退出码 `$?` 与 `set -e`，对应 PowerShell 的 `$?`、`$LASTEXITCODE`（只在调外部 exe 时更新）与 `$ErrorActionPreference='Stop'`；解释为什么"PS 里 grep 失败脚本继续跑"是最常见对译 bug。
  3. 基于上面的对照，建立一张对译速查表：grep→`Select-String`、sed→`-replace`、awk→`ForEach-Object`/`Where-Object`、xargs→`ForEach-Object -Parallel`、trap→`try/finally`、`$(...)`→`$()`+子表达式；标注每条中"形似神不似"的坑。
  4. 动手：把 DI05 的"挂载镜像→复制→失败回滚"脚本完整对译成 PowerShell（`Mount-DiskImage`、`Copy-Item`、`try/catch/finally`），在 Windows 上验证回滚路径，并列出对译过程中你不得不改变设计结构的三处。

---

### DI10 脚本分发与环境锁定

- **预期档位**: hard
- **考察维度**: 环境探测 / 版本横幅 / 降级矩阵
- **对话脚本**:
  1. 把脚本发给十台机器，十台环境各不相同：最低 bash 版本、必需命令（jq/curl/openssl）、操作系统、CPU 架构（arm64 服务器上 `uname -m` 差异）——设计一个 `preflight()` 启动自检：缺什么就报什么、给出安装指引，而不是跑到一半神秘失败。
  2. 针对上面的自检，加版本要求：脚本用了 `mapfile`（bash 4+）与 `readarray`，如何比较版本字符串（把 `4.2.46` 拆位比较而不是字符串比较）；解释 macOS 系统 bash 3.2 的历史包袱与 `brew install bash` 后 shebang 解析路径。
  3. 基于上面的版本矩阵，讨论分发形态选型：单文件脚本 vs `curl | bash` 安装器 vs 打成 Homebrew formula / deb 包 vs 干脆 Docker 镜像兜底（`docker run --rm mytool`），从依赖可控性、更新成本、离线环境三个维度对比。
  4. 动手：为脚本加上 `--self-check`（打印环境矩阵：OS/bash 版本/依赖版本/架构）与 `--version`（嵌入发布号与 git hash），写一个发布 checklist：shellcheck 通过 → bats 全绿 → 三平台 smoke → 打 tag，说明为什么 CI 应拒绝跳过 smoke 直接发版。
