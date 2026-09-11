# 78 基础设施可观测性与 SRE

> 编号段 CA01–CA10 · 聚焦 SRE 文化、可观测性工程与错误预算治理
>
> **本批运行环境约束**：无生产服务 / 无 Prometheus / 无 Grafana / 无 Kubernetes / 无 OpenTelemetry Collector，**全部以纯 python3 + numpy + sqlite3 mini 实现 + markdown 跑通**。OTel 三支柱用 Metrics 直方图 + Logs JSON + Traces 父子 span ID 等价；SLO 计算器（错误预算 burn rate）纯 python；ChaosMesh 降级为故障注入脚本（python subprocess 杀进程 / 网络丢包用 socket）。产物落 `tmpPlan/agent-test/` 沙盒。

---

### CA01 可观测性三大支柱与用户导向可观测

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_08-S01-S10-AI工程LLM应用测试脚本重构方案.md）
- **预期档位**: simple
- **考察维度**: 三大支柱 + 用户导向理念
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca01/` 下用 Write 写 `three_pillars.md`：markdown 表格 3 行（Logs / Metrics / Traces），列：数据类型、典型查询、电商下单举例；Bash `grep -c '^|' three_pillars.md` ≥ 5 + `grep -F '下单' three_pillars.md` 命中。
  2. 写 `five_pillars.md`：markdown 解释 Profiles / Events 加入构成五支柱的合理性 + 可观测性 vs 监控的本质区别（前者能回答未知问题，后者只能回答预设问题）；Bash `wc -l five_pillars.md` ≥ 12 + `grep -F '监控' five_pillars.md` 命中。
  3. 写 `user_centric.py`：实现"用户导向 vs 机器导向"对比——函数 `user_journey_event(name, latency_ms)` 记录用户体验事件；Bash 跑后断言事件含 `latency_ms > 5000` 时分类为 "user_frustrated"，写入 `uc.out`。
  4. 写 `user_journey.md`：markdown 列 3 个用户旅程埋点例子——"页面 LCP > 2.5s" / "用户首次点击到响应 > 1s" / "提交按钮点击后无反馈 > 200ms"；Bash `grep -c '^- ' user_journey.md` ≥ 3 + `grep -F '用户' user_journey.md` 命中。

### CA02 OpenTelemetry 工程化落地（mini SDK + Resource）

- **预期档位**: medium
- **考察维度**: SDK/Collector/Exporter 三层 + Resource Attributes
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca02/` 下用 Write 写 `otel_sdk.py`：实现 mini OTel SDK——`Tracer.start_span(name, ctx)` 创建 span，`span.set_attribute(k, v)` / `span.end()` / `exporter.export(spans)` 把 span 输出 JSON；Bash 跑后断言导出 span 数 == 创建数，写入 `otel.out`。
  2. 写 `collector_config.yaml`：mini OTel Collector 配置——`receivers: [otlp]` / `processors: [batch]` / `exporters: [jaeger, prometheus]` / `pipelines: {traces: [otlp→batch→jaeger], metrics: [otlp→batch→prometheus]}`；Bash `python -c "import yaml; c=yaml.safe_load(open('collector_config.yaml')); print(c['receivers'])"` 断言解析正确。
  3. 写 `resource_attrs.py`：实现 Resource Attributes——`Resource({"service.name": "api", "service.version": "1.0", "k8s.pod.name": "api-7d8"})` 附加到每条 span；Bash 跑后断言 span 含资源属性，写入 `res.out`。
  4. 写 `version_compare.py`：模拟"按服务版本对比错误率"查询——给定不同版本 span 集合，按 `service.version` 分组统计 error 比例；Bash 跑后断言输出 dict 含 `v1.0` 与 `v1.1` 两条数据，写入 `cmp.out`。

### CA03 SLO/SLI 与错误预算策略（burn rate 计算）

- **预期档位**: medium
- **考察维度**: SLI/SLO/SLA + 错误预算 + burn rate
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca03/` 下用 Write 写 `slo_def.md`：markdown 定义 SLI/SLO/SLA——为"用户登录 API"给出 SLI（`success / total`）/ SLO（99.9%）/ 错误预算（30 天 = 43.2 分钟）；Bash `grep -F '99.9' slo_def.md` 命中 + `grep -F '错误预算' slo_def.md` 命中。
  2. 写 `burn_rate.py`：实现 burn rate 计算——`burn_rate = (errors / total) / (1 - slo_target)`；给定 30 天 SLO 99.9%，burn_rate=2 含义：每小时允许消耗 0.2% 错误预算；Bash 跑后断言计算正确，写入 `br.out`。
  3. 写 `multi_window_alert.py`：实现多窗口告警——快速窗口（1h burn > 14.4 = critical）/ 慢速窗口（3d burn > 1 = warning）；Bash 跑后断言告警分级正确，写入 `alert.out`。
  4. 写 `budget_policies.md`：markdown 列错误预算耗尽 3 种策略——冻结功能发布 / 全员 on-call 修稳定性 / 接受风险并记录，各适用场景；Bash `grep -c '^### ' budget_policies.md` ≥ 3 + `grep -F '错误预算' budget_policies.md` 命中。

### CA04 Incident Management 与 Blameless Postmortem

- **预期档位**: medium
- **考察维度**: 事件分级 + on-call + 无责复盘
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca04/` 下用 Write 写 `sev_levels.md`：markdown 表 3 行（Severity 1/2/3），列：定义、响应时间、升级路径、通知范围；Bash `grep -c '^|' sev_levels.md` ≥ 5 + `grep -F 'Severity 1' sev_levels.md` 命中。
  2. 写 `oncall_roster.py`：模拟"跟随太阳"轮值——给定 3 个时区团队（美东 / 欧洲 / 亚太），生成 30 天 on-call 日历，每 12 小时轮换；Bash 跑后断言每天 24h 都有团队值班，写入 `oncall.out`。
  3. 写 `postmortem_template.md`：markdown 写 blameless postmortem 5 个必要章节——事件时间线 / 根因 / 影响范围 / 改进项 / 经验教训；Bash `grep -c '^## ' postmortem_template.md` ≥ 5 + `grep -F 'Blameless' postmortem_template.md` 命中。
  4. 写 `db_postmortem.md`：虚构"数据库主从延迟导致写入丢失"事故复盘——重点写"系统怎么允许这件事发生"（主从监控缺失 / 同步超时过长 / 缺少重试补偿），而不是"谁按错按钮"；Bash `wc -l db_postmortem.md` ≥ 25 + `grep -F '系统' db_postmortem.md` 命中。

### CA05 告警疲劳治理（分组/抑制/静默）

- **预期档位**: medium
- **考察维度**: 可行动告警 + 复合告警 + 降噪
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca05/` 下用 Write 写 `alert_fatigue.md`：markdown 列 3 种垃圾告警类型——重复告警 / 长期未处理 / 信息性通知；Bash `grep -c '^- ' alert_fatigue.md` ≥ 3 + `grep -F '告警' alert_fatigue.md` 命中。
  2. 写 `actionable_alert.md`：markdown 把"磁盘使用率 80%"改写为可行动版（含操作指引 + 预期影响 + 升级路径）；Bash `wc -l actionable_alert.md` ≥ 8 + `grep -F '操作' actionable_alert.md` 命中。
  3. 写 `composite_alert.py`：实现复合告警聚合——50 台机器同时 CPU 飙升合成一条"机房异常"告警；Bash 跑后断言聚合后告警数 == 1，写入 `comp.out`。
  4. 写 `noise_reduce.py`：实现"依赖上游数据库的 10 个服务同时报错"降噪——grouping（按服务） + inhibition（上游恢复后抑制下游） + silencing（已通知的故障）三机制；Bash 跑后断言最终通知数从 10 降到 1，写入 `nr.out`。

### CA06 容量规划与负载测试（Little's Law）

- **预期档位**: hard
- **考察维度**: 容量模型 + 负载测试 + 弹性伸缩
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca06/` 下用 Write 写 `capacity_plan.md`：markdown 写容量规划 5 步——业务增长预测 / 当前容量基线 / 差距分析 / 扩容方案 / 验证测试；Bash `grep -c '^## ' capacity_plan.md` ≥ 5。
  2. 写 `load_types.md`：markdown 列 4 种测试类型（负载 / 压力 / 浸泡 / 尖峰）的目标与对应生产事故；Bash `grep -c '^### ' load_types.md` ≥ 4 + `grep -F '浸泡' load_types.md` 命中。
  3. 写 `little_law.py`：实现 Little's Law `L = λW`——给定到达率 λ=1000 req/s，平均时延 W=0.2s，平均在制品 L = 200；Bash 跑后断言 L = 200，写入 `ll.out`。
  4. 写 `sizing_calc.py`：容量估算——单实例 QPS 上限 1000、P99 延迟 200ms，目标 10000 QPS 且 P99 < 500ms；考虑冷启动 2x 惩罚，最少需要多少实例；Bash 跑后断言需要 14 实例（10×2/1.5 向下取整），写入 `size.out`。

### CA07 混沌工程原理与工具实践（python 故障注入）

- **预期档位**: hard
- **考察维度**: 五大原则 + 实验设计 + 工具选型
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca07/` 下用 Write 写 `chaos_principles.md`：markdown 解释混沌工程五大原则（建立稳态假说 / 多样化实验 / 生产环境 / 自动化 / 最小爆炸半径）+ 为什么生产环境做实验更安全；Bash `grep -c '^[0-9]\.' chaos_principles.md` ≥ 5 + `wc -l chaos_principles.md` ≥ 15。
  2. 写 `chaos_experiment.md`：markdown 设计混沌实验——假设"单可用区故障时服务自动切换"、注入故障（关停一个可用区所有节点）、稳态假说（请求成功率 > 99%）、回滚条件（成功率 < 95% 持续 1min）、观测指标（成功率 / P99 / 切换耗时）；Bash `grep -c '^## ' chaos_experiment.md` ≥ 5 + `grep -F '回滚' chaos_experiment.md` 命中。
  3. 写 `chaos_tools.md`：markdown 对比 ChaosMesh / LitmusChaos / ChaosBlade / Gremlin 4 款工具的定位差异，K8s 场景推荐 ChaosMesh；Bash `grep -c '^|' chaos_tools.md` ≥ 6 + `grep -F 'ChaosMesh' chaos_tools.md` 命中。
  4. 写 `fault_inject.py`：故障注入脚本——`inject_kill(pid)` 杀进程 / `inject_latency(iface, ms)` 加网络延迟（用 `tc` 命令）/ `inject_packet_loss(iface, pct)`；Bash 跑 `python fault_inject.py --check` 断言脚本可调用（无 root 时返回 "permission denied" 也算可识别），写入 `fi.out`。

### CA08 Continuous Profiling 与 eBPF 可观测（mini profiler）

- **预期档位**: medium
- **考察维度**: 持续剖析 + eBPF 等价
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca08/` 下用 Write 写 `continuous_profiling.md`：markdown 解释 continuous profiling vs 按需 profiling 的本质区别（5% CPU 开销持续采样 vs 偶发 dump）；Bash `grep -F '5%' continuous_profiling.md` 命中 + `wc -l continuous_profiling.md` ≥ 10。
  2. 写 `parca_pyroscope.md`：markdown 对比 Parca vs Pyroscope 数据模型 / 采集方式 / 存储后端；Bash `grep -c '^|' parca_pyroscope.md` ≥ 5 + `grep -F 'Parca' parca_pyroscope.md` 命中。
  3. 写 `mini_profiler.py`：用 python + signal 实现采样 profiler——`SIGALRM` 每 10ms 采样当前栈（`traceback.format_stack()`），跑 5 秒输出火焰图简化版 CSV `stack,count`；Bash 跑后断言采样数 ≈ 500，写入 `prof.out`。
  4. 写 `openat_count.py`：实现"统计进程 openat 系统调用"——`strace -c -e openat -p <pid>` 等价（python 模拟：监控 `/proc/<pid>/syscall` 文件）；Bash 跑后断言能读取某进程的 `openat` 调用计数，写入 `oa.out`。

### CA09 SRE 与平台工程融合（IDP + Golden Path）

- **预期档位**: medium
- **考察维度**: SRE 文化 + 平台工程 + IDP
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca09/` 下用 Write 写 `sre_pe_compare.md`：markdown 解释 SRE 与平台工程的关系——SRE 是文化（错误预算 / blameless），平台工程是规模化输出（IDP / Golden Path）；Bash `grep -F '平台工程' sre_pe_compare.md` 命中 + `wc -l sre_pe_compare.md` ≥ 12。
  2. 写 `idp_capabilities.md`：markdown 列 5 个 IDP 应提供的可靠性自助能力——一键创建服务 / 标准化部署 / 自动伸缩 / 监控告警模板 / 故障演练；Bash `grep -c '^[0-9]\.' idp_capabilities.md` ≥ 5 + `grep -F 'IDP' idp_capabilities.md` 命中。
  3. 写 `golden_path.md`：markdown 描述一条部署黄金路径——`git push → CI 跑测试 → 构建镜像 → 部署到 staging → 人工审批 → 部署到 prod`；Bash `grep -c '→' golden_path.md` ≥ 4 + `grep -F 'Golden Path' golden_path.md` 命中。
  4. 写 `off_path_governance.md`：markdown 列 3 种"非标准路径"治理策略（教育引导 / 渐进收紧 / 强制拦截），按强制程度排序；Bash `grep -c '^[0-9]\.' off_path_governance.md` ≥ 3 + `grep -F '强制' off_path_governance.md` 命中。

### CA10 实战：用 Grafana 技术栈搭建可观测平台（mini stack）

- **预期档位**: hard
- **考察维度**: 数据流向 + 应用埋点 + Self-Monitoring
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca10/` 下用 Write 写 `stack_arch.md`：markdown 画 Prometheus+Loki+Tempo+Grafana 数据流图——应用 → OTel SDK → Collector → 各存储 → Grafana 查询；Bash `grep -c '^|' stack_arch.md` ≥ 6 + `grep -F 'Grafana' stack_arch.md` 命中。
  2. 写 `mock_collector.py`：mock OTel Collector 127.0.0.1:18431，`POST /v1/traces` 与 `/v1/metrics` 与 `/v1/logs` 分别接收三类数据存 sqlite3 三张表；Bash 探活后断言 HTTP 200。
  3. 写 `dashboard.json`：mini Grafana dashboard 定义（JSON）——含 4 个面板：RED Rate / P99 Latency / Error Count / Active Traces；Bash `python -c "import json; d=json.load(open('dashboard.json')); print(len(d['panels']))"` 断言 panels 数 ≥ 4。
  4. 写 `app_instrument.py`：应用埋点——同时发送 metrics（counter++） / logs（json.dumps） / traces（span），Bash 跑后断言三类数据都到达 mock collector（sqlite3 查表行数 > 0）。
  5. 写 `self_monitor.md`：markdown 设计"可观测平台的可观测性"——自监控方案（Collector 上报自己的健康指标 / 存储容量监控 / 查询响应时间告警）；Bash `grep -c '^- ' self_monitor.md` ≥ 4 + `grep -F 'Self-Monitoring' self_monitor.md` 命中。
