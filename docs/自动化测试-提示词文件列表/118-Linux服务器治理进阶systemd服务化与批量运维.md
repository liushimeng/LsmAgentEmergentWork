# 118 Linux 服务器治理进阶：systemd 服务化与批量运维

> 编号段 DJ01–DJ10 · 聚焦「从单机排查到多机治理」：systemd 服务化部署 / 依赖与激活机制 / timer 替代 cron / journalctl 深度 / cgroups 资源配额 / 免 Agent 批量巡检 / SSH 密钥治理 / 升级窗口 / 幂等脚本 / 自愈预案
>
> 与现有维度互补说明：
> - `03-电脑使用与系统管理`（C 维）聚焦**单机排查与工具使用**（C01 磁盘占用 / C03 定时任务配置 / C05 SSH 使用 / C08 日志轮替 / C09 备份策略）；本文件聚焦**服务化、多机与治理**——timer 条目讲迁移与并发治理而非 crontab 语法，SSH 条目讲密钥批量分发轮换而非连接使用，不重复备份与磁盘排查。
> - `29-云原生与DevOps工程实战`（AC 维）聚焦**容器与云侧**（Dockerfile / K8s / Terraform）；本文件聚焦**裸机与 systemd 侧**。
> - `78-基础设施可观测性与SRE`（CA 维）聚焦**指标与告警体系**；本文件的 journalctl 条目聚焦本机日志查询治理而非观测管道。
> - `63-疑难Bug攻坚与故障排查实录`（BL 维）聚焦**故障根因**；本文件聚焦**治理配置与自动化**。
>
> **本文件独特主题**：unit 文件编写与沙箱选项 / socket·path 激活 / systemd timer 治理 / journalctl 磁盘预算 / cgroups v2 配额 / 免 Agent 批量巡检 / 密钥轮换 / 事务性更新 / 幂等运维脚本 / 自愈预案脚本化。

---

### DJ01 systemd unit 模板 + 沙箱自检

- **测试状态**: ✅ 已测试（2026-09-11 Linux x86_64 / laew mock(anthropic) -debug 全链路通过：Yolo medium → Main-Work → SubAgent 5 工具(Write→Bash→Write→Bash→Write) → QC ✅ → DebugReport ✅；systemd-analyze verify 在容器内因二进制不存在返回 exit=1 为预期行为,提示词明确「不可用则跳过」；详见 tmpPlan/2026-09-11_xx-DI06-DI07-DJ01-Linux测试方案.md）
- **预期档位**: medium
- **考察维度**: unit 三段式 / Restart / ProtectSystem
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/dj01/` 写 `myapp.service`（最小三段式：Description+After=network.target / ExecStart=/usr/local/bin/myapp / Restart=on-failure + RestartSec=5 + StartLimitBurst=3 / WantedBy=multi-user.target）；Bash `systemd-analyze verify myapp.service` 跑静态校验（不可用则跳过）。
  2. Write 写 `checker.py`：解析 .service 文件、按必填字段（Description/ExecStart/WantedBy）列表自检 → 输出缺失字段报告；Bash 跑：`python3 checker.py myapp.service` 断言无缺失字段。
  3. Write 加沙箱三件套：`NoNewPrivileges=true` / `ProtectSystem=strict` / `RuntimeDirectory=myapp`；再跑 `python3 checker.py myapp.service` 输出沙箱项检查，断言三项都已配置。
  4. Write 写 `dj01_report.md`：解释 `enable` 与 `start` 是两件事、`daemon-reload` 漏掉导致"改了 unit 但老配置还在跑"的常见事故、`EnvironmentFile` 与 `User=`/`Group=` 降权。

---

### DJ02 socket 激活 + path 触发 reload

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: medium
- **考察维度**: socket 激活 / Type=notify / path 单元
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/dj02/` 写 `myapp.socket` + `myapp.service`（socket 监听 8080、service 配 `Accept=no` + `Type=notify`）；Bash `systemd-analyze verify myapp.socket myapp.service` 断言无 syntax error（不可用则 skip）。
  2. Write 写 `notify_demo.py`：模拟服务监听 socket、收到 sd_notify READY=1 通知（实际写一个 fake notify socket 触发逻辑）；Bash 跑：`python3 notify_demo.py --service myapp` 输出 "READY=1 sent"（验证 Type=notify 流程的最小演示）。
  3. Write 加 `myapp.path` 单元监视 `myapp.conf` 变化触发 `myapp-reload.service`；Bash 写 `path_demo.sh`：模拟 path 单元的触发（`inotifywait myapp.conf` → 触发 reload）→ 跑脚本输出 "reload triggered"。
  4. Write 写 `dj02_report.md`：解释 After vs Requires vs Wants 语义、socket 激活的三大收益（零停机重启/按需启动/崩溃期间排队）、应用必须支持接收现成 fd 的前提。

---

### DJ03 timer 替代 cron 迁移脚本

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: medium
- **考察维度**: OnCalendar / Persistent / 并发防护
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/dj03/` 写 `backup.cron`（一行 `17 3 * * * /usr/local/bin/backup.sh`）；Bash `crontab -l < backup.cron` 模拟读取。
  2. Write 写 `migrate_cron.py`：解析 crontab 行 → 生成 `backup.timer`（`OnCalendar=*-*-* 03:17:00` + `RandomizedDelaySec=10m` + `Persistent=true`）和 `backup.service`（`TimeoutStartSec=3600` + `OnFailure=failure-notify@%n.service`）；Bash 跑：`python3 migrate_cron.py backup.cron` 输出生成的两个 unit 文件内容。
  3. Bash 校验生成内容：`grep -q 'Persistent=true' backup.timer` 断言存在；`grep -q 'TimeoutStartSec=3600' backup.service` 断言存在。
  4. Write 写 `dj03_report.md`：解释 cron 的三个治理缺陷（不知道上次成功/关机错过/日志散邮箱）、timer 的三个对应答案、systemd 同名单元天然互斥 vs cron flock 自锁。

---

### DJ04 journalctl 查询模拟器

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: medium
- **考察维度**: 过滤语法 / 磁盘预算 / 快照导出
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/dj04/` 写 `fake_journal.py`：生成 1000 行模拟 journal 日志到 `journal.log`（每行：timestamp + UNIT=xxx + PRIORITY=n + MESSAGE=...），含 10 条 -p err 高优先级；Bash 跑脚本、`wc -l journal.log` 断言 1000。
  2. Write 写 `journal_query.py`：实现 `-u UNIT` / `--since/--until` / `-p PRIORITY` / `_PID=` 过滤、`--disk-usage` / `--vacuum-size=200M` 模拟；Bash 跑：`python3 journal_query.py --since 2026-09-11T08:00 --until 2026-09-11T10:00 --priority err journal.log` 输出过滤后行数，断言 ≤ 10。
  3. Bash 用 awk 交叉验证：`awk '/2026-09-11T08:/ && /2026-09-11T10:/ && /err/' journal.log | wc -l` 断言与过滤脚本结果一致。
  4. Write 写 `dj04_report.md`：解释 journal 持久化（Storage=auto + /var/log/journal）、配额三道闸（SystemMaxUse/MaxFileSize/MaxRetentionSec）、`--show-cursor` 断点续查机制。

---

### DJ05 cgroups v2 配额（python 探测）

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: medium
- **考察维度**: cgroups v2 只读探测 / CPU/Memory/IO
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/dj05/` 写 `cgroup_probe.py`：只读探测 `/sys/fs/cgroup/`（含 `cgroup.controllers` 文件判断 v2）、读当前进程 cgroup 路径、解析 `cpu.max`/`memory.max`/`memory.high`/`io.weight`；Bash 跑：`python3 cgroup_probe.py` 输出当前 cgroup 路径与各资源值，断言在 cgroup v2 系统上输出含 `cpu.max`。
  2. Bash 验证当前 PID 在哪个 cgroup：`cat /proc/self/cgroup` 与 cgroup_probe 输出对照；Bash 跑 `python3 cgroup_probe.py --check v2` 断言输出 "v2 detected"。
  3. Write 加压力测试对比：`MemoryMax=512M` cgroup 下启一个会 leak 的子进程（用 `tr '/dev/zero' '/dev/null'` 拉内存），Bash 跑 `systemd-run --scope -p MemoryMax=512M python3 leaky.py &`（不可用则 Write 写 `cgroup_oom_demo.md` 解释演示流程 + 结论）。
  4. Write 写 `dj05_report.md`：解释 CPUQuota=200%（硬上限）vs CPUWeight=50（争抢权重）语义差异、MemoryMax 触发 cgroup OOM 与系统 OOM killer 的关系、pressure 文件读法。

---

### DJ06 本地多目录批量巡检（offline fleet）

- **测试状态**: ✅ 已测试（2026-09-11 第四十一轮 Linux x86_64 / laew mock(anthropic) -debug 4 轮全过：q1 Write gen_local_fleet.sh（5 主机目录,h3 故意不同 Debian/Port 2222/PasswordAuthentication no）→ q2 Bash DJ06_FLEET_GEN_OK hosts=5 + Write fleet_inspect.sh（xargs -P 5 并发采集）→ q3 Bash baseline+sed+再跑+diff 出 h3 sshd_hash 变更 → q4 Write dj06_report.md 2532 字节 4 章节齐全;Yolo hard → Plan 5 步 → Main-Work 1 流程 → SubAgent 5 工具链(Write→Bash→Write→Bash→Write)全成功 → QC ×3 全过 → SessionContext → DebugReport;13 次 LLM 调用全成功无重试,iter=6,任务 289ms;21 文件产物落盘 tmpPlan/agent-test/dj06/(含 5 行 JSONL + drift_report.txt 11 行);关键词 DJ06_OFFLINE_FLEET 三阶段穿透 Yolo/Plan/SubAgent;详见 tmpPlan/2026-09-11_DJ06-Linux离线批量巡检测试与mock多步工具链方案.md）
- **预期档位**: hard
- **考察维度**: 并发 ssh 模拟 / 漂移检测 / JSON 行输出
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/dj06/` 写 `gen_local_fleet.sh`：创建 5 个模拟主机目录 `hosts/{h1..h5}/`（各含 etc/os-release、etc/sshd_config、var/lib/myapp/state.json 等文件，故意让 h3 的 sshd_config 与 h1 不同）；Bash 跑：`bash gen_local_fleet.sh`、`ls hosts/h1/etc/` 断言含 os-release。
  2. Write 写 `fleet_inspect.sh`：遍历 5 目录并发收集指纹（OS 版本、内核、磁盘水位、配置 hash）→ JSON 行输出 → 与 `last_snapshot/` diff 生成漂移报告；Bash 跑：`bash fleet_inspect.sh hosts/ > current.jsonl`、`wc -l current.jsonl` 断言 = 5。
  3. Bash 第一次跑完存 baseline、跑 `cp current.jsonl snapshot_v1.jsonl`，再修改 h3 的 sshd_config 跑一次 → 输出 `drift_report.txt`；Bash 跑 `grep h3 drift_report.txt` 断言含 "sshd_config changed"。
  4. Write 写 `dj06_report.md`：解释 ssh -o BatchMode=yes 避免交互卡死、`xargs -P` 并发、本地多目录巡检是真实批量巡检的离线等价物。

---

### DJ07 SSH 密钥轮换脚本（本地模拟）

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: medium
- **考察维度**: 密钥生命周期 / 三步顺序 / 备份策略
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/dj07/` 写 `gen_keys.sh`：用 `ssh-keygen` 生成 ed25519 密钥对到 `keys/old_key` 与 `keys/new_key`（无密码），模拟「已部署密钥」与「新密钥」；Bash 跑：`bash gen_keys.sh`、`ls keys/` 断言含 old_key/new_key/old_key.pub/new_key.pub。
  2. Write 写 `rotate_keys.py`：三步轮换——追加新公钥到目标 authorized_keys（模拟）+ 验证新钥匙可登录（用 ssh -i 测试本地 fake 服务）+ 删除旧公钥条目；Bash 跑：`python3 rotate_keys.py --host host1` 输出三步执行日志，断言每步"ok"。
  3. Bash 故意倒序轮换（删旧后加新）：`python3 rotate_keys.py --host host2 --inverted` 断言失败并报"self lockout risk"。
  4. Write 写 `dj07_report.md`：解释 ed25519 优于 rsa 的理由（短、速度快、抗量子侧信道）、`ssh-keygen -R` 批量清理 known_hosts、sshd 基线（PasswordAuthentication no 等）的攻击面收敛。

---

### DJ08 apt 安全升级窗口脚本

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: medium
- **考察维度**: apt-mark hold / dry-run / 健康检查
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/dj08/` 写 `upgrade_window.sh`：四阶段——预检（`apt list --upgradable` 模拟）→ 备份/快照 → 升级（`-o Dir::Etc::sourcelist` 指向 security 源）→ 验证（systemd 单元 + 端口探测 + 关键 URL 200）；Bash 跑：`bash upgrade_window.sh --dry-run --check-only` 输出"将要做的事"清单。
  2. Write 写 `fake_apt.py`：模拟可升级包清单（apt list --upgradable 输出格式），含安全包与功能包；Bash 跑：`bash upgrade_window.sh --dry-run --use fake_apt` 断言只升级带 `-security` 源标记的包。
  3. Write 加健康检查：`fake_url_check.py` 模拟 HTTP 200 探测；Bash 跑：`bash upgrade_window.sh --run --use fakes` 断言每阶段 exit code = 0。
  4. Write 写 `dj08_report.md`：解释 apt-mark hold 锁内核/数据库版本、`apt upgrade` vs `dist-upgrade`、openSUSE/Ubuntu snapper/zsys 事务性快照回滚机制。

---

### DJ09 幂等运维脚本 + dry-run

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: medium
- **考察维度**: 先查后做 / 标记收敛 / 自动回滚
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/dj09/` 写 `nonidempotent.sh`：建用户、装包、改配置、开端口——每步都不检查直接做；Bash 跑：`bash nonidempotent.sh && bash nonidempotent.sh && bash nonidempotent.sh`，第二次会报 user exists 错误。
  2. Write 写 `idempotent.sh`：每步先查后做（`id -u app || useradd app` / `dpkg -l | grep -q pkg || apt install -y pkg` / `ufw status | grep -q '22/tcp' || ufw allow 22/tcp`）；Bash 连续跑 3 次：`bash idempotent.sh && bash idempotent.sh && bash idempotent.sh` 断言 echo "all no-op" 第三行。
  3. Write 加 `--dry-run` 与自动备份/回滚；Bash 跑：`bash idempotent.sh --dry-run` 输出 "将做：useradd app" 而不实际执行；故意在第 3 步注入失败（`false`）→ 跑 `bash idempotent.sh --run --inject-fail step3` 断言前 2 步回滚。
  4. Write 写 `dj09_report.md`：解释幂等四手法（先查后做/标记收敛/upsert/期望状态对比）、dry-run 实现陷阱（引号转义）、备份到带时间戳目录与失败回滚。

---

### DJ10 自愈脚本：探测→动作阶梯→熔断

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-10_20-A09-G01-I04-自动化测试与TUI验证方案.md）
- **预期档位**: hard
- **考察维度**: 分层探测 / 熔断上限 / 演练注入
- **工具链**: Write → Bash → Bash → Write
- **对话脚本**:
  1. Write 在 `tmpPlan/agent-test/dj10/` 写 `autoheal.sh`：三层探测（`systemctl is-active` / TCP 端口 / HTTP 业务 URL）、动作阶梯（失败 1 次→restart / 2 次→清缓存+restart / 3 次→熔断只告警）；Bash 跑：`bash autoheal.sh --check-only myapp` 输出三层探测结果到 stdout，断言 ≥ 2 行。
  2. Write 写 `fake_service.sh`：模拟一个会失败的服务（启动后 sleep 5 退出）；Bash 跑：`bash autoheal.sh --run --service fake` 三次观察日志；前两次触发 restart、第三次进入熔断。
  3. Write 加日志记录（模拟 journal）：Bash 跑 `bash autoheal.sh --run --service fake --log heallog.txt`，`grep 'circuit break' heallog.txt` 断言 ≥ 1 行熔断记录。
  4. Write 写 `dj10_report.md`：解释为什么自愈必须有"熔断次数上限"（防止重启风暴掩盖根因）、三层探测分别能抓住哪类故障、预案"平时不演练、战时跑不动"的问题。
