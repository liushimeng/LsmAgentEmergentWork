# 162 Homelab 自托管服务与家庭实验室治理

> 编号段 FB01–FB10 · 聚焦「家庭规模自托管服务的全生命周期治理」：服务目录校验 / 反代配置生成 / 内网穿透选型 / 证书到期巡检 / compose 编排 / 3-2-1 备份演练 / 资源功耗治理 / 入口加固审计 / ARM 兼容 / 迁移 runbook
>
> 与现有维度互补说明：
> - `122-智能家居与家庭自动化编程`（DN 维）聚焦**设备联动**（Zigbee/传感器/自动化）；本文件聚焦**服务器上的自托管服务治理**（反代/证书/备份/编排）。
> - `66-跨平台电脑使用与虚拟化实战`（BO 维）的 NAS 是**用户视角**一节；本文件是**管理员视角**的服务全生命周期。
> - `29-云原生与DevOps工程实战`（AC 维）聚焦**企业 K8s**；本文件聚焦**家庭规模**轻量栈（compose 级）。
> - `127-容器编排与Kubernetes运维实战`（DS 维）是 K8s 专题；本文件刻意**不引入 K8s**，用 compose 与裸进程解决问题。
>
> **本文件独特主题**：服务目录资产清单 / 反向代理配置生成 / 内网穿透选型 / TLS 证书巡检 / compose 服务编排 / 备份 3-2-1 与恢复演练 / 资源与功耗治理 / 远程访问加固 / ARM 兼容治理 / 迁移扩容 runbook。

---

### FB01 homelab 服务目录资产清单

- **预期档位**: medium
- **考察维度**: 资产建模 / 唯一性校验 / 备份策略约束
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `fb01_catalog.py`：服务目录校验器（JSON 输入）：字段 name/domain/port/host/data_dir/backup_policy/depends_on；校验 port 唯一、domain 唯一（internal 可无 domain）、有 data_dir 必须声明 backup_policy（none/daily/weekly）。有违规输出中文清单并退出码 1，通过则打印统计摘要。
  2. Bash：`python3 -m py_compile fb01_catalog.py`，断言退出码为 0。
  3. Read `fb01_catalog.py`，再 Write `fb01_services.json`：8 个 homelab 服务（jellyfin/gitea/nextcloud/pihole/immich 等），故意含一处两服务端口重复、一处「有 data_dir 无 backup_policy」。本轮必须显式复用第一轮的字段命名与校验规则，不允许另起无关主题。
  4. Bash：`python3 fb01_catalog.py fb01_services.json 2>&1 | tee fb01_report.txt`，断言退出码非 0、涉事服务名均被 `grep -F` 命中。

### FB02 反向代理配置生成

- **预期档位**: hard
- **考察维度**: 配置生成 / 双引擎等价 / 暴露面控制
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `fb02_proxygen.py`：读取 FB01 同构服务目录生成两份等价配置：Caddyfile（域名块内 reverse_proxy 到 host:port）与 nginx server 块（server_name/proxy_pass）；`internal: true` 默认不生成公网入口，仅当另带 `access_list: true` 才输出受控入口段；生成前检测与反代 80/443 端口冲突，冲突退出码 1。
  2. Bash：`python3 -m py_compile fb02_proxygen.py`，断言退出码为 0。
  3. Read `fb02_proxygen.py`，再 Write `fb02_services.json`：6 个带 domain 的服务：4 普通、1 个 `internal: true`（不应入配置）、1 个 +`access_list: true`（出现受控入口）；字段沿用第一轮。本轮必须显式复用第一轮的生成规则与字段，不允许另起无关主题。
  4. Bash：生成 `fb02_Caddyfile` 与 `fb02_nginx.conf` 落盘，断言 `grep -c 'reverse_proxy'` 与 `grep -c 'proxy_pass'` 相等且均为 5、`grep -F 'access_list'` 两份配置各命中、纯 internal 服务域名 `grep -c` 为 0。

### FB03 内网穿透方案选型与配置

- **预期档位**: medium
- **考察维度**: 方案对比决策 / 场景匹配 / frp 配置生成
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `fb03_tunnel.py`：内置 frp/cloudflared/tailscale/zerotier 决策表（协议/端口要求/延迟档/自托管程度/适用面）；输入场景（pure_web/all_ports/tcp_service）输出选型建议 JSON（每场景一个推荐+理由）；`--frpc` 按 server_addr/token/local_port/remote_port 生成 frpc.ini，缺必填字段退出码 1。
  2. Bash：`python3 -m py_compile fb03_tunnel.py`，断言退出码为 0。
  3. Read `fb03_tunnel.py`，再 Write `fb03_scenarios.json`：3 个场景（家中 web 相册对外、远程 SSH 进内网机器、仅组网内暴露 tcp 数据库）+ 一组 frp 参数（server_addr=frps.example.net、token 占位、本地 127.0.0.1:22 → remote_port 6022）。本轮必须显式复用第一轮的决策表字段与场景枚举值，不允许另起无关主题。
  4. Bash：跑选型输出 `fb03_advice.json`，`python3 -m json.tool` 校验且三场景各含推荐字段；`--frpc` 生成 `fb03_frpc.ini`，断言 `test -s` 非空、`grep -F 'server_addr'` 与 `grep -F '6022'` 命中。

### FB04 TLS 证书到期巡检

- **预期档位**: medium
- **考察维度**: 证书解析 / 到期天数分级 / 续期计划
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `fb04_certcheck.py`：解析模拟 `openssl x509 -enddate` 文本段（不联网，仅标准库）；`--today YYYY-MM-DD` 固定基准日算剩余天数保证测试确定性；分级：已过期或 <14 天 CRITICAL、<30 天 WARNING、否则 OK；输出逐证书行+汇总+续期计划表（markdown），有 CRITICAL 退出码 1。
  2. Bash：`python3 -m py_compile fb04_certcheck.py`，断言退出码为 0。
  3. Read `fb04_certcheck.py`，再 Write `fb04_certs.txt`：4 段模拟 openssl 输出（用 `=== cert <域名> ===` 分隔），域名依次为 expired.lan/crit.lan/warn.lan/ok.lan，notAfter 依次为 2026-09-01（已过期）、2026-10-06（剩 5 天）、2026-10-21（剩 20 天）、2027-04-01（剩 182 天）。本轮必须显式复用第一轮的输入格式与分级阈值，不允许另起无关主题。
  4. Bash：`python3 fb04_certcheck.py fb04_certs.txt --today 2026-10-01 2>&1 | tee fb04_report.txt`，断言退出码非 0、expired.lan 与 crit.lan 行为 CRITICAL（`grep 'CRITICAL' | grep -cF` 各 1）、warn.lan 行 WARNING、ok.lan 行 OK。

### FB05 compose 服务编排与环检测

- **预期档位**: hard
- **考察维度**: compose 生成 / 健康检查与依赖 / 循环依赖检测
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `fb05_composegen.py`：从服务目录生成 docker-compose.yml（纯文本拼装）：每服务含 image/ports/volumes（data_dir 映射 `./data/<name>`）/`restart: unless-stopped`/healthcheck（test 命令占位）；depends_on 拓扑排序输出启动顺序注释；生成前做环检测，成环打印环路径（形如 `a -> b -> c -> a`）并退出码 1。
  2. Bash：`python3 -m py_compile fb05_composegen.py`，断言退出码为 0。
  3. Read `fb05_composegen.py`，再 Write `fb05_services.json`：两份目录：`main` 为 5 服务合法依赖链（caddy 依赖 jellyfin/gitea，nextcloud 依赖 postgres）；`cycle_case` 为 a→b→c→a 三服务故意成环。本轮必须显式复用第一轮的字段与环路径输出格式，不允许另起无关主题。
  4. Bash：对 main 生成 `fb05_compose.yml`，断言 `grep -c 'restart: unless-stopped'` 为 5、`grep -c 'healthcheck'` ≥5、`grep -F './data/'` 命中；对 cycle_case 运行 `2>&1 | tee fb05_cycle.txt`，断言退出码非 0 且 `grep -F 'a -> b -> c -> a'` 命中。

### FB06 备份 3-2-1 与恢复演练

- **预期档位**: hard
- **考察维度**: 备份策略落地 / 校验和 / 恢复演练闭环
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `fb06_backup.sh`：POSIX sh 三子命令：`plan`（读服务目录，为每个有 data_dir 的服务列计划：每日归档 + 每周全量标记 + `<备份根>/offsite/` 异地副本，体现 3-2-1）；`backup <数据目录> <备份根>`（tar czf 带日期命名归档，并复制一份到 offsite/）；`restore <归档> <目标目录>`（解包后与源 sha256 清单逐文件比对，一致输出 `VERIFY OK`，否则输出差异并退出码 1）。
  2. Bash：`sh -n fb06_backup.sh`，断言 Shell 语法有效。
  3. Read `fb06_backup.sh`，再 Write `fb06_plan.json`：3 个服务（gitea/nextcloud/jellyfin）data_dir 指向 fb06_data/ 下子目录、备份根 fb06_backups/、保留策略（daily 7 / weekly 4）。本轮必须显式复用第一轮的子命令名与目录约定，不允许另起无关主题。
  4. Bash：真跑一轮：`mkdir -p` 造 3 个数据目录各写一个固定内容文件 → 逐服务 `backup` → `test -s` 断言归档与 offsite 副本存在 → `restore` 到临时目录并 tee 落盘日志 → 断言 `grep -c 'VERIFY OK'` 为 3 → `rm -rf` 清理。

### FB07 资源监控与功耗治理

- **预期档位**: medium
- **考察维度**: 真实采集 / 阈值分级 / 休眠取舍建议
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `fb07_health.sh`：POSIX sh 真实采集：`df -P`（awk 取使用率）、`free -m`、`/sys/class/thermal/thermal_zone*/temp`（不存在则输出「温度: SKIPPED」不报错）、`uptime`；按阈值（磁盘 >85%、内存 >90% 告警）输出 Markdown 报告：资源表+告警段+治理建议段（夜间降频、停非核心服务取舍）。
  2. Bash：`sh -n fb07_health.sh`，断言 Shell 语法有效。
  3. Read `fb07_health.sh`，再 Write `fb07_policy.json`：阈值表（disk_percent 85/mem_percent 90/temp_c 70）、安静时段 23:00-07:00、非核心服务（jellyfin/immich/bazarr）与核心服务（pihole/caddy）及取舍理由。本轮必须显式复用第一轮的阈值变量名与建议段结构，不允许另起无关主题。
  4. Bash：`sh fb07_health.sh > fb07_report.md 2>&1`，断言 `test -s` 非空、`grep -E '[0-9]+%'` 命中磁盘行、`grep -F '温度'` 命中（SKIPPED 或数值）、`grep -E '取舍|建议'` 命中、`grep -F '|'` 命中表头。

### FB08 远程访问入口加固审计

- **预期档位**: medium
- **考察维度**: 监听暴露面 / 失败登录聚合 / 密钥轮换提醒
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `fb08_audit.py`：解析模拟 `ss -tlnp` 文本：0.0.0.0/[::] 判「公开监听」并建议改绑 127.0.0.1 走反代、127.0.0.1 判「仅本机」；解析 auth.log 失败登录片段（正则提 IP）聚合输出 top 来源 IP 及次数；按 key_rotated_at 与 `--today` 基准日算轮换倒计时（超 90 天提醒）；产出加固 checklist markdown。
  2. Bash：`python3 -m py_compile fb08_audit.py`，断言退出码为 0。
  3. Read `fb08_audit.py`，再 Write `fb08_inputs.txt`：三段数据：`=== ss ===` 6 条监听（4 条 0.0.0.0/[::]、2 条 127.0.0.1）；`=== auth ===` 8 行失败登录（203.0.113.7 出现 5 次为 top）；`=== keys ===` 两服务轮换日期（其一超 90 天）。本轮必须显式复用第一轮的分段标记与判定规则，不允许另起无关主题。
  4. Bash：`python3 fb08_audit.py fb08_inputs.txt --today 2026-10-01 > fb08_checklist.md 2>&1`，断言 `grep -c '公开监听'` 为 4、`grep -F '203.0.113.7'` 命中且同行含 5、`grep -F '127.0.0.1'` 与 `grep -F '轮换'` 命中。

### FB09 树莓派 ARM 兼容治理

- **预期档位**: medium
- **考察维度**: 架构探测 / 镜像支持标注 / 替代方案表
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `fb09_armcheck.py`：`--uname <输出>` 解析 x86_64/aarch64/armv7l；对服务目录逐服务核对镜像 arm64 支持（字段 image + arm64: yes|no|unknown）；arm64:no 输出替代方案表（官方 arm 镜像 / 社区多架构替代 / qemu-user-static 模拟及性能代价）；unknown 标「待确认」。
  2. Bash：`python3 -m py_compile fb09_armcheck.py`，断言退出码为 0。
  3. Read `fb09_armcheck.py`，再 Write `fb09_services.json`：6 个服务：4 个 yes、1 个 no（frigate 旧版 x86 镜像）、1 个 unknown，均带 image 与 arm64 字段。本轮必须显式复用第一轮的字段与输出格式，不允许另起无关主题。
  4. Bash：`python3 fb09_armcheck.py fb09_services.json --uname aarch64 | tee fb09_report.txt`，断言报告含 aarch64、no 服务名在替代方案段命中（`grep -F`）、`grep -F 'qemu'` 与 `grep -F '待确认'` 命中。

### FB10 迁移与扩容 runbook

- **预期档位**: hard
- **考察维度**: runbook 生成 / 数据清单校验 / 切流与回退
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `fb10_migrate.py`：生成迁移 runbook markdown：逐服务「stop → backup → 新机恢复 → verify → 切流」五步，切流注明提前 24h 将 DNS TTL 降为 300，末尾必有回退预案段（DNS 回切+旧机保留期）；`--checksum <目录>` 生成 sha256 清单；`--compare <源清单> <目标清单>` 输出差异文件与缺失数，有差异退出码 1。
  2. Bash：`python3 -m py_compile fb10_migrate.py`，断言退出码为 0。
  3. Read `fb10_migrate.py`，再 Write `fb10_services.json`：3 个待迁服务（pihole 排最先、caddy 依赖其余服务排最后）含 host/data_dir/domain 与新主机名 homelab-v2。本轮必须显式复用第一轮的五步命名与清单格式，不允许另起无关主题。
  4. Bash：真跑：`mkdir -p` 造 `fb10_src/{app,config}` 写入 `app/notes.txt`（内容 dawn）与 `config/app.conf` → 生成 runbook 断言 `test -s` 且 `grep -F '回退'` 与 `grep -F '300'` 命中 → `--checksum` 生成源清单 → `cp -r` 到 fb10_dst 后 `echo dusk > fb10_dst/app/notes.txt` → `--compare` tee 落盘断言退出码非 0 且报告指出 `app/notes.txt` → 清理。
