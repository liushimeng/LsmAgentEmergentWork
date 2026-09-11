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

### DJ01 systemd 服务化部署自研程序

- **预期档位**: medium
- **考察维度**: unit 文件 / 重启策略 / 运行环境
- **对话脚本**:
  1. 把一个自己写的 Go/Rust 二进制变成开机自启服务：写一个最小 `[Unit]` + `[Service]` + `[Install]` 的 unit 文件，解释 `ExecStart` 的绝对路径要求、`WantedBy=multi-user.target` 的含义、`enable` 与 `start` 为什么是两件事。
  2. 针对上面的最小版本，补齐生产细节：`Restart=on-failure` + `RestartSec=5` + `StartLimitBurst` 三者如何配合避免"崩溃风暴无限重启"；`EnvironmentFile` 注入密钥、`User=`/`Group=` 降权、`WorkingDirectory` 固定工作目录。
  3. 基于上面的配置，进入运维视角：`systemctl status` 看什么、`journalctl -u myapp -f` 跟日志、改完 unit 必须 `daemon-reload`；解释为什么"改了 unit 文件但没 reload，老配置还在跑"是最常见的部署事故之一。
  4. 动手：给服务加上 `RuntimeDirectory=myapp`（systemd 自动创建 /run/myapp 并清理）与 `NoNewPrivileges=true`/`ProtectSystem=strict` 沙箱三件套，用 `systemd-analyze security myapp.service` 打暴露面分数并降到 5 分以下。

---

### DJ02 systemd 依赖与激活机制

- **预期档位**: medium~hard
- **考察维度**: 依赖语义 / socket 激活 / 就绪通知
- **对话脚本**:
  1. `After=` 与 `Requires=` 的区别：After 只管顺序、Requires 才管生死；`Wants=` 弱依赖失败不拖垮自己——用一个"应用依赖 Redis"的例子给出三种组合的失败传播矩阵（Redis 挂了/没挂但没就绪/启动失败）。
  2. 针对上面的"没就绪"问题，讲 `Type=` 家族：`simple`（fork 就算活了）/ `forking`（等主进程退出）/ `notify`（应用主动 sd_notify READY=1）；解释为什么数据库类服务必须用 notify 或 notify-ready 才能解决"端口开了但还不能接查询"。
  3. 基于上面的依赖模型，进入 socket 激活：`myapp.socket` 先监听端口、服务第一个连接到来才启动，systemd 把已监听的 fd 传给应用；讨论它带来的三个收益（零停机重启、按需启动、崩溃期间连接排队）与一个前提（应用必须支持接收现成 fd）。
  4. 动手：给自己的服务配一对 `.socket` + `.service`，验证 `systemctl stop myapp` 期间 `curl` 连接不报错而是挂起等待；再配一个 `path` 单元监视配置文件变化触发 reload，形成"改配置自动生效"的闭环。

---

### DJ03 systemd timer 替代 cron 的治理价值

- **预期档位**: medium
- **考察维度**: OnCalendar / 并发防护 / 迁移治理
- **对话脚本**:
  1. cron 的三个治理缺陷：不知道上次跑没跑成功、机器关机就错过、日志散落邮箱；systemd timer 对应的三个答案：`Persistent=true` 补跑、`journalctl -u myjob.timer` 天然日志、`OnFailure=` 失败钩子——逐条对比说明。
  2. 针对上面的迁移动机，写一个完整的 timer：`OnCalendar=*-*-* 03:17:00`（为什么避开整点）、`RandomizedDelaySec=10m`（为什么能防"百台机器同时打数据库"）、`AccuracySec=1min` 的触发精度语义。
  3. 基于上面的单元，处理定时任务的老大难——并发与重叠：`systemd` 单元同名服务天然互斥（第二次触发排队）与 cron 需要 `flock` 自锁的对比；再讲 `OnUnitInactiveSec`（上次结束后 30 分钟再跑）与固定时刻触发的语义差异。
  4. 动手：把一条遗留 crontab（备份任务）迁移成 timer：写迁移脚本解析 crontab 行生成 `.timer`+`.service` 对、设置 `Persistent=true` 验证"关机错过、开机补跑"、用 `systemctl list-timers` 确认下次触发时间，并给任务加超时 `TimeoutStartSec=3600`。

---

### DJ04 journalctl 深度查询与磁盘治理

- **预期档位**: medium
- **考察维度**: 持久化 / 光标与过滤 / 磁盘预算
- **对话脚本**:
  1. `journalctl` 不带参数只看到本次开机：解释 journal 的易失性默认（`Storage=auto` 且 `/var/log/journal` 不存在时全在内存）；如何开启持久化以及重启后 `-b -1` 看上一次启动日志的用法。
  2. 针对上面的持久化，进入过滤语法：按单元 `-u nginx`、按时间 `--since "2026-09-11 09:00" --until "10分钟前"`、按优先级 `-p err`、按字段 `_PID=`/`MESSAGE_ID=`、`-g` 正则匹配消息体；用"查一次凌晨三点 OOM 事件"的完整命令串起来。
  3. 基于上面的查询，治理磁盘：`SystemMaxUse=500M`/`SystemMaxFileSize`/`MaxRetentionSec` 三道闸，`journalctl --disk-usage` 巡检、`--vacuum-size=200M` 立即回收；解释为什么配额配在 journald 而不是等 logrotate 来清。
  4. 动手：把一次故障的日志导出归档——`--since` 圈定窗口后 `-o export` 二进制格式 vs `-o json-pretty` 的取舍，附上光标（`--show-cursor`）机制说明如何支持"断点续查"，写成一个 `journal_snapshot.sh` 交接包脚本。

---

### DJ05 cgroups v2 资源配额治理

- **预期档位**: medium~hard
- **考察维度**: cgroups v2 / 配额与权重 / OOM 评分
- **对话脚本**:
  1. 为什么测试机上一个脚本能把整机拖死：进程默认不受 CPU/内存限制；cgroups v2 统一层级下 CPU 配额两件套 `CPUQuota=200%`（硬上限，双核吃满）与 `CPUWeight=50`（争抢时的相对权重）语义区别。
  2. 针对上面的配额，治理内存：`MemoryMax=2G` 触发的行为不是"限速"而是"杀进程"（cgroup OOM）——与系统级 OOM killer 的关系；`MemoryHigh`（软限,到限后强回收与降速）与 `MemoryMax` 的搭配策略。
  3. 基于上面的机制，讲 IO 与观测：`IOWeight` 对磁盘争抢的影响、`systemd-cgtop` 实时看各 cgroup 的 CPU/内存消耗、`/sys/fs/cgroup/.../memory.pressure` 压力文件读法；讨论"CPU 打满但负载不高"时如何用 pressure 判断是算力不够还是 IO 等待。
  4. 动手：给一个会内存泄漏的演示程序配 `MemoryMax=512M` + `OOMScoreAdjust=500`，跑起来观察它被 cgroup OOM 杀掉且不拖垮同机其他服务；再用 `systemd-run --scope -p CPUQuota=50%` 临时限制任意一次性命令。

---

### DJ06 免 Agent 批量主机巡检

- **预期档位**: hard
- **考察维度**: ssh 并发 / 指纹收集 / 报告聚合
- **对话脚本**:
  1. 三十台机器逐台 ssh 手敲命令不可持续：写第一个巡检循环 `for h in $(cat hosts.txt); do ssh "$h" 'uptime; df -h /'; done`，然后指出它的三个问题——串行慢、一台超时拖垮全程、输出无法归属到主机。
  2. 针对上面的三个问题逐个修：`ssh -o ConnectTimeout=5 -o BatchMode=yes`（免交互+超时）、`xargs -P 10 -I{} ssh {} ...` 或 GNU `parallel --jobs 10` 并发、输出加 `[host]` 前缀前缀聚合；解释 `BatchMode=yes` 避免密码交互卡死的价值。
  3. 基于上面的执行框架，定义巡检指纹：每台机器收集 OS 版本、内核、磁盘水位、指定服务状态、配置文件 hash，本地存上次快照，diff 出"配置漂移"；讨论为什么"漂移检测"比"单次巡检"更有治理价值。
  4. 动手：实现 `fleet_inspect.sh`：并行采集 → JSON 行输出 → 本地按主机分目录存档 → 与上次快照 diff 生成漂移报告（变更服务/变更文件/新增告警水位），对一台故意改过 `sshd_config` 的虚拟机验证漂移被命中。

---

### DJ07 SSH 接入治理与密钥轮换

- **预期档位**: medium~hard
- **考察维度**: 密钥生命周期 / sshd 基线 / 证书登录
- **对话脚本**:
  1. 密钥不是发出去就完事：梳理生命周期——生成（`ed25519` 为什么优于 `rsa`）、授权（`authorized_keys`）、盘点（谁在哪些机器有我的钥匙）、轮换（换钥匙 vs 撤销）、回收（离职/换岗）；解释为什么"五年不换的部署密钥"是审计红线。
  2. 针对上面的生命周期，自动化轮换：写脚本批量给 N 台机器追加新公钥 → 验证新钥匙可登录 → 删除旧公钥条目（三步顺序不能乱，否则把自己锁外面）；讨论 `ssh-keygen -R` 与 `known_hosts` 在批量场景的连带处理。
  3. 基于上面的轮换，收敛 sshd 攻击面基线：`PasswordAuthentication no`、`PermitRootLogin prohibit-password`、`MaxAuthTries 3`、`AllowUsers` 白名单；解释每条对应的攻击面，以及改 `sshd_config` 时"保留一个已连接会话再 reload"的保命操作。
  4. 动手：进阶到 SSH 证书：用 CA 密钥 `ssh-keygen -s` 签发带过期时间的用户证书、服务端 `TrustedUserCAKeys` 一次配置全局生效；对比"逐台分发公钥"与"CA 签发"的运维成本，写一份两方案的适用场景决策记录。

---

### DJ08 包管理与系统升级窗口治理

- **预期档位**: medium
- **考察维度**: 升级策略 / 版本锁定 / 回滚
- **对话脚本**:
  1. 生产机升级的最大风险是"顺手全升"：区分安全补丁（要快）与功能升级（要稳）；Debian 系 `apt upgrade` vs `dist-upgrade` 的差别、`apt-mark hold` 锁住内核或数据库版本不被顺手带走、RHEL 系 `dnf versionlock` 对应能力。
  2. 针对上面的版本锁定，设计升级窗口：预检（`apt list --upgradable`、变更清单归档）→ 备份/快照 → 升级 → 验证（关键服务健康检查清单）→ 退出或回滚；解释"为什么要先归档变更清单"（出问题时能对账归因）。
  3. 基于上面的流程，讲两类高级机制：`unattended-upgrades` 只自动装安全更新的白名单配置；openSUSE/Ubuntu 的事务性更新（`snapper`/`zsys` 快照 + 重启即回滚）如何把"升级失败"从灾难变成"选旧快照启动"。
  4. 动手：写 `upgrade_window.sh`：收集可升级包清单存档 → 只升级安全来源的包（`-o Dir::Etc::sourcelist` 指向 security 源）→ 跑健康检查（systemd 单元状态 + 端口探测 + 关键 URL 200）→ 输出窗口报告；dry-run 模式只出清单不动系统。

---

### DJ09 运维脚本幂等化设计

- **预期档位**: medium~hard
- **考察维度**: 幂等 / 状态标记 / dry-run 与回滚
- **对话脚本**:
  1. "脚本跑一半失败了，再跑一遍会发生什么？"——同一用户建两次、防火墙规则插两条、注释行重复追加；给"幂等"下运维定义：任意次执行与一次执行效果相同，逐条检查上面三个例子的破坏方式。
  2. 针对上面的破坏点，给四种幂等化手法：先查后做（`id -u app || useradd app`）、标记收敛（文件内容用模板整体覆盖而非追加、`sshd_config` 用 drop-in 目录替代 sed 改原文）、upsert 语义（SQL 的 `ON CONFLICT`）、期望状态对比（`diff` 后才动手）。
  3. 基于上面的手法，补两个安全阀：`--dry-run` 输出"将要做的事"清单但不执行、变更前自动备份将修改的文件到带时间戳目录并在失败时回滚；讨论 dry-run 的实现陷阱（echo 出来的命令与真实执行的引号转义不一致）。
  4. 动手：把一段非幂等的"初始化机器"脚本（建用户/装包/改配置/开端口）重写为幂等版，连续跑三遍验证第二三遍全部 no-op；用 `--dry-run` 在干净机器上预演，并故意在第三步注入失败验证自动回滚。

---

### DJ10 故障自愈与应急预案脚本化

- **预期档位**: hard
- **考察维度**: 健康探测 / 自愈动作 / 预案与演练
- **对话脚本**:
  1. 凌晨三点磁盘写满、进程僵死——没人盯的时候只能靠脚本自己救自己：设计分层健康探测（进程在吗 `systemctl is-active`、端口通吗 TCP 探测、业务真通吗 HTTP 探测 + 响应体校验），说明三层探测分别能抓住哪类故障。
  2. 针对上面的探测结果，定义自愈动作阶梯：第一次失败→重启服务（`systemctl restart`）、连续失败→清缓存/清临时文件后重启、持续失败→停止自愈并告警（防止重启风暴掩盖根因）；解释为什么自愈必须有"熔断次数上限"。
  3. 基于上面的阶梯，讲应急预案脚本化：升级前"一键回滚到上一版本二进制+配置"脚本、数据库主从切换预案的检查清单脚本化（前置条件断言→切换→验证→通知）；讨论预案"平时不演练、战时跑不动"的问题与定期演练机制。
  4. 动手：实现 `autoheal.sh`：探测→动作阶梯→熔断→记录处置日志到 journal（`systemd-cat`），配成 1 分钟一次的 timer；用"杀掉服务进程""占满 tmp 目录"两个注入实验验证两档自愈都正确触发且第三次进入熔断只告警不动作。
