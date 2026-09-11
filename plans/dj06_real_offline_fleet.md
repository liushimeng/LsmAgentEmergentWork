# 任务方案:DJ06_OFFLINE_FLEET 离线多主机批量巡检演练(dj06_real)

> 由 LsmAgentEmergentWork-Plan 于 2026-09-11 生成
> Session: 20260911-164139-b2cb7a82-1789116099037-f7afcf

## 一、目标

在 `tmpPlan/agent-test/dj06_real/` 目录内(注意:与已有参考版 `tmpPlan/agent-test/dj06/` 完全隔离的独立副本),完成一次离线批量巡检模拟演练的全链路:用 `gen_local_fleet.sh` 生成 5 个模拟主机目录(h1..h5)、用 `fleet_inspect.sh` 通过 `xargs -P` / `&+wait` 并发采集指纹(baseline → 篡改 → current)、用 `jq` / `awk` diff 两个 JSONL 生成漂移报告、撰写 `dj06_report.md` 说明 `ssh -o BatchMode=yes`、`xargs -P` 并发原理、本地多目录巡检是真实批量巡检的离线等价物等关键技术点。最终产物需在 `dj06_real/` 下完整可见,且断言 `grep 'h3' drift_report.txt` 命中 `sshd_config changed`。

## 二、WorkFlow 拆解

### WorkFlow 1:参考资料勘察与目标目录初始化

- 步骤:
  - [ ] 读取 `tmpPlan/agent-test/dj06/` 下已有的 `gen_local_fleet.sh` / `fleet_inspect.sh` / `drift_report.txt` / `dj06_report.md`,提取可复用模式(sha256sum 截断 16 字符、JSONL 单行结构、xargs -P 用法),仅作风格参考,不修改原文件
  - [ ] 创建 `tmpPlan/agent-test/dj06_real/` 空目录,确认与参考版 `dj06/` 完全隔离
  - [ ] 确认工具链可用:`bash`、`sha256sum`、`jq`(或 `awk` 兜底)、`diff`、`grep`、`xargs`
- 委派 Agent: SubAgent-Work
- 依赖: 无
- 验收标准: `ls -la tmpPlan/agent-test/dj06_real/` 显示空目录(仅有 . / ..),且参考版 `dj06/` 的 4 个产物未被任何修改(`git diff tmp_plan/agent-test/dj06/` 为空)

### WorkFlow 2:实现 gen_local_fleet.sh(5 主机目录生成器)

- 步骤:
  - [ ] Write 创建 `tmpPlan/agent-test/dj06_real/gen_local_fleet.sh`,`#!/usr/bin/env bash` + `set -euo pipefail` + `cd "$(dirname "$0")"` 起手
  - [ ] 循环创建 `hosts/h{1..5}/etc` 与 `hosts/h{1..5}/var/lib/myapp` 目录
  - [ ] h1/h2/h4/h5 生成默认 `etc/os-release`(Ubuntu 24.04)、`etc/sshd_config`(Port 22 + PasswordAuthentication yes + PermitRootLogin no)、`var/lib/myapp/state.json`(同一份健康状态)
  - [ ] h3 故意与 h1 不同:`etc/sshd_config` 改 `Port 2222` + `PasswordAuthentication no`(或 `PermitRootLogin yes`),`etc/os-release` 用 Debian 12,`var/lib/myapp/state.json` 用 degraded
  - [ ] 脚本末尾加自验证断言:`test -f hosts/h1/etc/os-release && test -f hosts/h3/etc/sshd_config && test -f hosts/h5/var/lib/myapp/state.json`,失败 `exit 1` 并打印明确错误
  - [ ] 末尾追加成功哨兵 `echo DJ06_REAL_FLEET_GEN_OK hosts=5`
- 委派 Agent: SubAgent-Work
- 依赖: wf-1
- 验收标准: 脚本可执行 (`chmod +x` 后) ;`./gen_local_fleet.sh` 返回码 0;`ls hosts/h{1..5}/etc/sshd_config` 输出 5 行;`grep -l 'Port 2222' hosts/h3/etc/sshd_config` 命中而 `grep -l 'Port 2222' hosts/h1/etc/sshd_config` 不命中;末尾断言存在

### WorkFlow 3:实现 fleet_inspect.sh(并发指纹采集器)

- 步骤:
  - [ ] Write 创建 `tmpPlan/agent-test/dj06_real/fleet_inspect.sh`,`set -euo pipefail` + 位置参数 `[hosts_dir=hosts] [out=current.jsonl]`(参数化)
  - [ ] 内嵌 `collect()` 函数:从 `os-release` 读 `PRETTY_NAME`(或 `VERSION=` 回退);从 `sshd_config` 用 `sha256sum` 取 16 字符指纹;从 `state.json` 用 `sha256sum` 取 16 字符指纹
  - [ ] 输出单行 JSON,字段至少含 `host`、`os`、`sshd_hash`、`state_hash`、`collected_at`(时间戳,`date -u +%FT%TZ`),逐行 JSON 末尾追加时间戳
  - [ ] 实现并发:二选一均可 —— (A) `ls "$hosts_dir" | sort | xargs -P N -I{} bash -c 'collect "$@"' _ {}`(需 `export -f collect` 与 `export hosts_dir`),或 (B) `for h in ...; do collect "$h" & done; wait`,推荐 A(`xargs -P`),更贴近真实批量巡检语义
  - [ ] 并发数 N 默认 5,可由 `-p <N>` 参数或环境变量 `FLEET_PARALLEL` 覆盖
  - [ ] 增加 `-h / --help` 帮助输出
  - [ ] 末尾追加哨兵 `echo DJ06_REAL_INSPECT_OK lines=$(wc -l < "$out")`
- 委派 Agent: SubAgent-Work
- 依赖: wf-2
- 验收标准: 脚本可执行;`bash fleet_inspect.sh -h` 输出帮助;`bash fleet_inspect.sh hosts current.jsonl` 输出 5 行 JSONL,每行含 6 个字段,`collected_at` 字段非空;同一脚本二次运行得到 `sshd_hash` 一致(确定性);`time` 测得的并发耗时小于串行基线 1.5x(粗略)

### WorkFlow 4:跑完整采集 + 漂移检测链路

- 步骤:
  - [ ] (a) 执行 `./gen_local_fleet.sh` 初始化目录(已由 wf-2 完成,此处可重跑验证幂等)
  - [ ] (b) 执行 `./fleet_inspect.sh hosts baseline.jsonl`,落盘 baseline 快照
  - [ ] (d) 篡改 h3:例如 `sed -i 's/^Port 2222/Port 2223/' hosts/h3/etc/sshd_config` 或 `sed -i 's/^PasswordAuthentication no/PasswordAuthentication yes/' hosts/h3/etc/sshd_config`(至少改 1 行,使 `sshd_hash` 变化)
  - [ ] (e) 执行 `./fleet_inspect.sh hosts current.jsonl`
  - [ ] (f) 漂移检测:编写一次性 diff —— 用 `jq -S` 把 baseline / current 按 host 排序后 `diff -u`,或用 `awk` 解析 JSONL,逐 host 比对 `sshd_hash` / `state_hash` / `os`,命中差异时输出 `h<id> <field> changed (old=X new=Y)`,汇总写入 `drift_report.txt`
  - [ ] (g) 断言 `grep -q 'h3' drift_report.txt && grep -q 'sshd_config changed' drift_report.txt`,任一失败则 `exit 1`
  - [ ] 末尾追加 `echo DJ06_REAL_DRIFT_OK $(grep -c 'changed' drift_report.txt) deltas`
- 委派 Agent: SubAgent-Work
- 依赖: wf-3
- 验收标准: `diff -q baseline.jsonl current.jsonl` 返回非零(说明存在漂移);`cat drift_report.txt` 至少含 1 行 `h3 sshd_config changed`;断言脚本返回 0;`ls -la baseline.jsonl current.jsonl drift_report.txt` 三者均存在且大小 > 0

### WorkFlow 5:撰写 dj06_report.md(原理文档化)

- 步骤:
  - [ ] Write 创建 `tmpPlan/agent-test/dj06_real/dj06_report.md`,结构 6 段:
    1. **任务概述与目录拓扑图**:ASCII 树展示 `dj06_real/{gen_local_fleet.sh,fleet_inspect.sh,baseline.jsonl,current.jsonl,drift_report.txt,dj06_report.md,hosts/h{1..5}/...}`,说明每个 h1..h5 子目录的内容
    2. **`ssh -o BatchMode=yes` 的作用**:禁用密码 / 键盘交互 / 已知主机确认询问,避免 `xargs -P` 批量巡检被任意一台主机的交互卡死;真实批量巡检的防挂起关键
    3. **`xargs -P` 并发原理**:进程级并发(fork+exec)vs SSH ControlMaster 连接复用 vs 协程;为什么这里选进程级而非协程 —— 本地 `sha256sum` 是 CPU/IO 短任务,fork 开销可忽略,但隔离性最好,失败一台不波及其它
    4. **本地多目录巡检是真实批量巡检的离线等价物**:替换 `ssh user@h 'cmd'` → `cat hosts/h/etc/...`,保留并发模型与漂移检测语义;可在 CI / sandbox 离线验证、零网络抖动、零凭证泄露
    5. **测试输出与漂移报告示例摘录**:贴 `baseline.jsonl` 末 2 行 + `current.jsonl` 末 2 行 + `drift_report.txt` 全文
    6. **局限与扩展**:缺网络 / agent push 模式、缺 SSH host key 指纹采集、并发上限可叠加(默认 5,可拉到 20/50)、可加入 `--dry-run` 与退出码分级
- 委派 Agent: SubAgent-Work
- 依赖: wf-4
- 验收标准: 6 个一级标题全部存在;含至少 1 个 ASCII 目录树;含至少 1 处真实命令示例(`bash fleet_inspect.sh hosts current.jsonl`);贴出 `drift_report.txt` 实际内容并解释;篇幅 ≥ 80 行

### WorkFlow 6:验收与归档

- 步骤:
  - [ ] 确认 `tmpPlan/agent-test/dj06_real/` 下产物齐全:`gen_local_fleet.sh`、`fleet_inspect.sh`、`baseline.jsonl`、`current.jsonl`、`drift_report.txt`、`dj06_report.md` 共 6 个文件
  - [ ] 重跑关键断言:`grep 'h3' drift_report.txt` 包含 `sshd_config changed`;`bash -n fleet_inspect.sh` 与 `bash -n gen_local_fleet.sh` 无语法错
  - [ ] 运行 `ls -la tmpPlan/agent-test/dj06_real/` 列出全部产物(写入计划文件 `plans/dj06_real_artifacts.txt` 留存)
  - [ ] 与用户复述最终结果:产物路径清单 + 漂移报告关键行 + 报告 6 段要点
- 委派 Agent: SubAgent-Work
- 依赖: wf-5
- 验收标准: 全部 6 个产物落地;所有断言通过;复述内容与产物一致;参考版 `dj06/` 未被污染

## 三、关键决策

- 决策 1:产物完全隔离到 `tmpPlan/agent-test/dj06_real/`,**不修改**已有的 `tmpPlan/agent-test/dj06/` 参考版。两个目录独立存在,避免互相覆盖 / 误改。dj06/ 仅作风格参考(gen_local_fleet.sh 的 16 字符 sha256 截断、xargs -P + bash -c 包装、JSONL 单行结构)。
- 决策 2:`fleet_inspect.sh` 选用 `xargs -P` 路线(而非 `& + wait`),原因有三:(a) 与真实批量 SSH 巡检代码模式完全一致,迁移成本最低;(b) 进程数自动按输入流排队,无需手动管理 job pool;(c) `& + wait` 需自己处理子进程退出码与信号传播,xargs 已内建。报告里仍需保留 `& + wait` 作为对比说明。
- 决策 3:`baseline.jsonl` 作为独立文件而非塞进 `last_snapshot/` 子目录(dj06/ 用的是 `last_snapshot/snapshot_v1.jsonl`)。原因是用户步骤明确写的是 `baseline.jsonl`,平铺更直观、更便于 `diff`。
- 决策 4:漂移报告格式自定义为 `<host> <field> changed (old=X new=Y)`,而非 `diff -u` 的 unified format。原因是用户断言要求 `grep 'sshd_config changed'`,该字面串在自定义格式里更精确;`diff -u` 的 `-host +host` 行不含此字符串。
- 决策 5:`gen_local_fleet.sh` 的 h3 差异化至少包含 `Port` 变化 + `PasswordAuthentication` 二者之一(推荐改 `Port 2222 → 2223`)。这样 `sshd_hash` 必变,但 `ssh_port` 字段也跟着变,drift 报告可同时体现两种漂移粒度。
- 决策 6:报告(`dj06_report.md`)保留 6 段(用户明确指定),并在第 6 段补"局限与扩展",把"为什么这里用进程级并发而非协程 / SSH ControlMaster"明确说清楚,避免和 dj06/ 参考版雷同。

## 四、风险与缓解

- 风险 1:`jq` 工具缺失导致漂移检测失败
  - 缓解:在 wf-1 勘察阶段先 `which jq`;若缺失,改用纯 `awk` 解析 JSONL(按字段位置切片),并在报告里说明 fallback 路径;wf-4 的漂移检测脚本需 `jq || awk` 兼容
- 风险 2:`xargs -P` + `bash -c` 嵌套在某些 POSIX 严格环境下 `export -f` 失败
  - 缓解:wf-3 实施时若 `export -f collect` 不生效(罕见,多见于非交互 bash),回退到方案 B(`for h in ...; do collect "$h" & done; wait`),两种实现都准备好
- 风险 3:误改 `tmpPlan/agent-test/dj06/` 参考版
  - 缓解:wf-1 起就明确目标目录是 `dj06_real/`(带 `_real` 后缀),所有 Write 路径前置校验;wf-6 用 `git diff tmp_plan/agent-test/dj06/` 验证参考版零变化
- 风险 4:`drift_report.txt` 未命中 `sshd_config changed` 字面断言
  - 缓解:wf-4 的 diff 脚本必须先 echo 一行 `=== DJ06_REAL drift report ===` 头,再逐条 `printf '%s %s changed (old=%s new=%s)\n' "$host" "$field" ...`,并写入哨兵;断言在脚本尾部 `if ! grep -q 'h3' drift_report.txt || ! grep -q 'sshd_config changed' drift_report.txt; then exit 1; fi`
- 风险 5:并发采集时多个 bash 子进程同时写 `current.jsonl` 引发行交错
  - 缓解:每个子进程输出到独立临时文件 `/tmp/fleet_<host>.jsonl`,主进程最后 `cat /tmp/fleet_*.jsonl | sort > current.jsonl`;或保持 xargs 默认 stdout 串行化(各子进程 stdout 经 xargs 串行合并到主进程 stdout,本身就是原子的)
- 风险 6:`set -euo pipefail` 在 `grep` 未命中时让脚本异常退出
  - 缓解:漂移检测中的 `grep -q` 必须用 `... || true` 包裹,或用 `if grep -q ...; then ...; fi`;baseline 比对时 `diff` 的退出码需用 `diff ... || true` 捕获

## 五、验收总览

- [ ] 所有 WorkFlow 通过 Quality-Check(wf-1 ~ wf-6 各自的验收标准全数达标)
- [ ] 编译 / 测试通过:`bash -n gen_local_fleet.sh`、`bash -n fleet_inspect.sh`、`bash -n drift` 三个脚本零语法错;`./gen_local_fleet.sh` 与 `./fleet_inspect.sh` 退出码 0;`grep 'h3' drift_report.txt` 命中 `sshd_config changed`
- [ ] `tmpPlan/agent-test/dj06_real/` 下 6 个产物全部落地:`gen_local_fleet.sh`、`fleet_inspect.sh`、`baseline.jsonl`、`current.jsonl`、`drift_report.txt`、`dj06_report.md`
- [ ] 参考版 `tmpPlan/agent-test/dj06/` 4 个产物未被修改(`git diff` 为空或文件 mtime 未变)
- [ ] 与用户复述最终结果:产物路径清单 + drift_report.txt 关键行(h3 sshd_config changed)+ dj06_report.md 6 段要点摘要