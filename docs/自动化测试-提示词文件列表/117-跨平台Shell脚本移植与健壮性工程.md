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

### DI01 GNU/BSD 工具差异探测

- **测试状态**: ✅ 已测试（2026-09-11 macOS arm64 / laew mock 4 轮全过:q1 Write GNU-only 脚本 → q2 Bash 捕获 BSD sed `undefined label` 与非零证据 + EXPECTED_NEGATIVE_OK → q3 portable_env_detect/type/run_demo BSD sed 回归 → q4 六命令差异报告;q1 同时验证新增预期负例 QC 豁免;产物与 4 份 DebugReport 落盘 TestWorkSpace/DI0102FinalRoot_*;详见 tmpPlan/2026-09-11_16-DI01-DI02-Shell测试与QC预期负例及失败用量修复方案.md）
- **预期档位**: medium
- **考察维度**: sed/date/readlink 差异 / 可移植替写
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/di01/` 写 `gen_diff_demo.sh`：故意用 `sed -i 's/a/b/' file`（无备份后缀）与 `date -d "2024-01-01" +%s`（GNU 语法），Bash 跑：`bash gen_diff_demo.sh` 在 GNU/Linux 上成功（断言 echo $? == 0）、`bash gen_diff_demo.sh 2>&1 | grep -i 'illegal option\|invalid'` 在 BSD/macOS 上预期失败（用 docker alpine 跑镜像不行 → 直接用 stderr 预演差异）。
  2. Write 写 `portable_env_detect()` 函数：探测 OS（`uname -s`）、包管理器（apt/brew/apk）、GNU 工具前缀（`command -v gsed gdate`）；Bash 跑：`bash -c 'source portable.sh; portable_env_detect'` 输出 `SED=/usr/bin/sed`、`DATE=date`、前缀提示。
  3. Bash 验证探测可移植：`bash -c 'source portable.sh; type portable_env_detect'` 断言输出函数定义；Write 加 `run_demo.sh`：根据探测结果切换 sed 写法，断言 echo "ok"。
  4. Write 写 `di01_report.md`：列出 sed/date/readlink/stat/cp/head 六命令 GNU vs BSD 差异表，解释"探测一次缓存到变量"优于到处写 uname 判断的原因。

---

### DI02 bash/zsh/fish 方言兼容

- **测试状态**: ✅ 已测试（2026-09-11 macOS arm64 / laew mock 4 轮全过:q1 bash=0-based + zsh=1-based + [[ glob → q2 countdown 在 bash/bash --posix 输出 5..1 → q3 bash/sh 语法与排除注释后的 Bashism 检查 → q4 Bash 3.2 兼容报告;产物与 4 份 DebugReport 落盘 TestWorkSpace/DI0102FinalRoot_*;详见 tmpPlan/2026-09-11_16-DI01-DI02-Shell测试与QC预期负例及失败用量修复方案.md）
- **预期档位**: medium
- **考察维度**: 数组下标 / word splitting / 模式匹配
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/di02/` 写 `dialect_demo.sh`：演示 bash 数组下标 0、`"${arr[@]}"` 引用、`[[ ]]` 模式匹配 `== glob`；Bash 跑：`bash dialect_demo.sh` 输出 0-based 索引；`zsh dialect_demo.sh`（如 zsh 可用）输出 1-based 差异。
  2. Write 写 `countdown.sh`：从 5 倒计时到 0，不使用关联数组与 `${var,,}`，Bash 跑：`bash countdown.sh` 输出 5/4/3/2/1；再 `bash --version` 验证能在 bash 3.2（macOS 自带）上跑（如当前 bash 较新，`bash --posix countdown.sh` 验证 POSIX 子集兼容性）。
  3. Bash 用 grep 检测兼容性：`bash -n countdown.sh && echo "syntax ok"` 断言无语法错。
  4. Write 写 `di02_report.md`：列出为兼容 bash 3.2 砍掉的语法（mapfile/关联数组/小写展开）、`#!/usr/bin/env bash` 在 Nix/Homebrew/WSL 上的解析路径差异。

---

### DI03 POSIX sh 子集降级

- **测试状态**: ✅ 已测试（2026-09-11 macOS arm64 / laew mock 4 轮全过:q1 Write bash_only.sh → q2 Write posix.sh → q3 Bash 静态校验 + Write 报告 → q4 bash 与 POSIX sh 双回归，输出 count=2 doubled=6 和 DI03_ALL_OK;medium 档 Yolo→Main-Work→SubAgent→QC 全链路;详见 tmpPlan/2026-09-11_14-DR02-DI03-macOS-Shell测试与Agent编排优化方案.md）
- **预期档位**: medium
- **考察维度**: POSIX 子集 / `[[ ]]` 替代 / dash
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/di03/` 写 `bash_only.sh`：含数组、`[[ ]]` 模式、进程替换 `<(...)`、local 声明；Bash `bash bash_only.sh` 跑通（断言 echo $? == 0）；`dash bash_only.sh 2>&1 | head -5` 预期报错。
  2. Write 写 `posix.sh`：改写 `bash_only.sh` 为 dash 可执行版本（用 `set --` 当数组、`case` 替代 `[[ ]]`、管道改文件、子函数替代 local）；Bash `dash posix.sh` 断言 exit 0。
  3. Bash 用 shellcheck 静态验证：`shellcheck --shell=sh posix.sh && echo "clean"` 断言无警告（若 shellcheck 不可用则 `dash -n posix.sh && echo "syntax ok"`）。
  4. Write 写 `di03_report.md`：列出降级过程中最痛的三处改动（local/数组/[[ ]]），解释 Alpine 镜像默认没 bash 对 Dockerfile 的影响。

---

### DI04 scriptdoctor 诊断脚本

- **测试状态**: ✅ 已测试（2026-09-11 第三十八轮 macOS arm64 / laew mock(openai) -debug 4 轮全过:q1 综合提示词 → Yolo simple → SubAgent 4 工具调用全成功(Write gen_bad_scripts.sh → Write scriptdoctor.py → Bash 诊断 → Write di04_report.md) → QC ✅ → SessionContext ✅ → DebugReport ✅;产物落盘 TestWorkSpace/tmpPlan/agent-test/di04/;全部 120 e2e 通过;详见 tmpPlan/2026-09-11_18-DI04-DI05-E06-编程Shell提示词测试与e2e验证方案.md）
- **预期档位**: simple
- **考察维度**: shebang/CRLF/BOM/可执行位
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/di04/` 写 `gen_bad_scripts.sh`：生成 3 个故意有问题的脚本 `crlf.sh`（CRLF 换行）、`no_shebang.sh`（无 shebang）、`not_exec.sh`（无可执行位）；Bash 跑脚本、`file crlf.sh` 断言 = "CRLF line terminators"。
  2. Write 写 `scriptdoctor.py`：检查 shebang 存在、CRLF（`\r` 字节）、UTF-8 BOM、可执行位、解释器是否在 `$PATH`；Bash 跑：`python3 scriptdoctor.py crlf.sh no_shebang.sh not_exec.sh` 输出诊断报告，断言 3 个脚本都至少命中 1 条问题。
  3. Bash 验证修复：给 `no_shebang.sh` 加 shebang、`chmod +x not_exec.sh`、`dos2unix crlf.sh`；再跑 `python3 scriptdoctor.py *.sh` 断言输出 "all clean"。
  4. Write 写 `di04_report.md`：整理"脚本跨机器跑不起来"的第一时间排查清单、`file`/`cat -A`/`which bash`/`bash -x` 四个工具的用法。

---

### DI05 trap 错误处理链 + 清理栈

- **测试状态**: ✅ 已测试（2026-09-11 第三十八轮 macOS arm64 / laew mock(openai) -debug 4 轮全过:q1 综合提示词 → Yolo hard → Plan 生成 → Main-Work 拆解 1 流程 → SubAgent Bash 执行 → QC ✅ → SessionContext ✅ → DebugReport ✅;mock 环境 Main-Work WorkFlow 走默认 LAEW_MOCK_OK 路径,链路完整性验证通过;详见 tmpPlan/2026-09-11_18-DI04-DI05-E06-编程Shell提示词测试与e2e验证方案.md）
- **预期档位**: hard
- **考察维度**: EXIT/ERR/INT / 清理栈 / 行号上报
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/di05/` 写 `cleanup_stack.sh`：实现 `push_cleanup` / `pop_cleanup`（数组存命令 + trap EXIT 倒序执行）+ `$LINENO` 错误行号上报；Bash 跑：`bash cleanup_stack.sh` 输出基本清理栈演示（断言最后一行 = "cleanup: tmpdir removed"）。
  2. Write 写 `mount_rollback.sh`：用 cleanup_stack 模拟"挂载 → 复制 → 失败回滚"流程；故意在 `cp` 后注入 `false` 让流程失败；Bash 跑：`bash mount_rollback.sh 2>&1` 断言输出 "umount /mnt/iso" 与 "rm -rf tmpdir" 两条清理。
  3. Bash 验证三个失败注入点：`bash -c "source mount_rollback.sh; mount_step; false_step"` 断言 EXIT 1 且清理栈全跑；`bash -c "source mount_rollback.sh; mount_step; cp_step; false_step"` 同样断言清理执行。
  4. Write 写 `di05_report.md`：解释 ERR trap 在 if/&&/管道非末段不触发、trap 重复注册覆盖而非追加的坑、分层清理栈与单槽 trap 的本质矛盾。

---

### DI06 getopts 参数解析

- **测试状态**: ✅ 已测试（2026-09-11 第四十轮 macOS arm64 / laew mock(openai) -debug 4 轮全过:q1 Write mytool.sh(371B,getopts vo:) → q2 Bash 验证 --help 输出 usage + -x exit=2 + DI06_Q2_OK → q3 Write mytool.sh(769B,手写 parse_long 长选项) → q4 Write di06_report.md(563B,解释 vo:/长选项/argparse);medium 档 Yolo→Main-Work→SubAgent→QC→SessionContext→DebugReport 全链路;真实 bash getopts + 长选项解析验证通过;Linux 环境同轮次通过;详见 tmpPlan/2026-09-11_21-D05-DR04-E09-DI06-编程Shell提示词测试与QC修复方案.md）
- **预期档位**: medium
- **考察维度**: getopts / 长选项 / 退出码
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/di06/` 写 `mytool.sh`：用 `getopts "vo:"` 解析 `-v`/`-o output`、`shift $((OPTIND-1))` 收集位置参数、`usage()` 输出 `-h` 帮助；Bash 跑：`bash mytool.sh -v -o out.txt file1 file2` 断言 echo $? == 0、参数被正确接收。
  2. Bash 验证长选项：`bash mytool.sh --help 2>&1 | head -3` 输出 usage；`bash mytool.sh -x 2>&1` 断言退出码 = 2（用法错误）。
  3. Write 加长选项解析（手写 parse_long 循环处理 `--output=x` / `--output x` / `--` 终止）；Bash 跑：`bash mytool.sh --output=out.txt file1` 断言 output=out.txt。
  4. Write 写 `di06_report.md`：解释 `vo:` 冒号位置语义、getopts 不支持长选项的原因、何时应换 argparse（Python）而不是硬撑。

---

### DI07 管道子 shell 变量丢失

- **预期档位**: hard
- **考察维度**: 子 shell 边界 / 进程替换 / 流式
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/di07/` 写 `subshell_trap.sh`：经典陷阱 `cat list | while read -r line; do count=$((count+1)); done; echo $count` 永远输出 0；Bash 跑：`bash subshell_trap.sh` 断言 echo 输出 = 0。
  2. Write 写 `subshell_fix.sh`：用 `while read ... done < <(cat list)` 进程替换修复；Bash 跑：`bash subshell_fix.sh` 断言 echo 输出 = 5（list 有 5 行）。
  3. Write 写 `csv_stats.sh`：流式读 CSV 统计行列数与某列汇总、用 `read` 替代 mapfile；Bash 跑：`bash csv_stats.sh < sample.csv` 断言输出"rows=10, sum=..."。
  4. Write 写 `di07_report.md`：解释子 shell 变量副本原理、四种修复方式、mapfile 是 bash 4+ 正解但需考虑 bash 3.2 兼容。

---

### DI08 bats 风格 shell 测试（python 替代）

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: medium
- **考察维度**: subprocess 断言 / 命令 mock / shell 行为验证
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/di08/` 写 `slugify.py`：slug 化函数（小写、空格转 -、去标点）；Bash 跑：`python3 slugify.py "Hello World!"` 断言输出 = "hello-world"。
  2. Write 写 `test_slugify.py`：5 个 unittest 用例（空串、纯字母、含空格、含特殊字符、Unicode），Bash 跑：`python3 -m unittest test_slugify.py -v` 断言 OK。
  3. Write 写 `test_shell_behavior.py`：用 subprocess 调 `bash csv_stats.sh < sample.csv` 断言 stdout 含 "rows=10"、stderr 为空、exit 0；Bash 跑 `python3 -m unittest test_shell_behavior.py -v` 断言 PASS。
  4. Write 写 `di08_report.md`：解释 bats 在不可用时用 python unittest + subprocess 是等效替代、命令 mock 通过 PATH 前置注入的技巧、setup/teardown 与 mktemp -d + trap 配合。

---

### DI09 bash↔PowerShell 对照表

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: medium
- **考察维度**: 文本流 vs 对象流 / 对译方法论
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/di09/` 写 `disk_usage.sh`：取占用空间前 5 的目录（`du -sh */ | sort -rh | head -5`）；Bash 跑：`bash disk_usage.sh | tee baseline.txt` 输出前 5 行。
  2. Write 写 `disk_usage.ps1`：PowerShell 版本（`Get-ChildItem | ForEach-Object { (Get-Item $_.FullName -Force).Length }` 简化版、`Sort-Object -Desc | Select -First 5`）；Bash 用 `which pwsh` 检查是否可用，如不可用则 Write 写 `disk_usage_pwsh_demo.txt` 伪输出对照格式。
  3. Bash 跑 `bash disk_usage.sh > bash_out.txt`、`cat bash_out.txt | wc -l` 断言 = 5（取前 5）。
  4. Write 写 `di09_report.md`：列出 bash↔PowerShell 对照表（grep→Select-String / sed→-replace / awk→ForEach-Object / xargs→ForEach-Object -Parallel / trap→try-finally / $(...)→$()子表达式），标注"形似神不似"的坑。

---

### DI10 preflight 自检 + 版本横幅

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: hard
- **考察维度**: 环境探测 / 版本拆位比较 / 依赖矩阵
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/di10/` 写 `preflight.sh`：检测 bash 版本（≥4.0）、必需命令（jq/curl/openssl）、OS、CPU 架构；`ver_ge()` 函数把版本字符串拆位比较（4.2.46 → [4,2,46]）；Bash 跑：`bash preflight.sh` 输出环境矩阵到 stdout，断言 bash≥4.0 行存在。
  2. Bash 故意在低 bash 测：`bash --version | head -1` 当前 bash 版本 > 4.0 时测试"硬编码低版本"分支：`bash -c 'source preflight.sh; bash_required=5.0; ver_ge "$BASH_VERSION" "$bash_required" || echo "blocked"'` 验证 ver_ge 在不满足时返回非零。
  3. Write 加 `--self-check`（打印环境矩阵）和 `--version`（嵌入发布号与 git hash）；Bash 跑：`bash preflight.sh --version` 输出含 "v1.0.0"，`bash preflight.sh --self-check` 输出含 "OS=" 行。
  4. Write 写 `di10_report.md`：对比单文件脚本/curl|bash 安装器/Homebrew formula/deb 包/Docker 镜像五种分发形态、CI 三平台 smoke 测试为何不可跳过。
