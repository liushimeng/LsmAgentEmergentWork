# 78 基础设施可观测性与 SRE

> 编号段 CA01–CA10 · 聚焦 SRE 文化、可观测性工程与错误预算治理
>
> **互补性说明**：与已存在的编号段分工明确——
> - `39-性能调优与Profiling实战`（AN01–AN10）聚焦**单机性能分析工具**（perf/pprof/Valgrind/火焰图），本文件聚焦**生产级可观测体系与 SRE 流程**（三大支柱/SLO/Incident/混沌工程）。
> - `22-分布式系统与微服务架构`（V01–V10）含 V09 分布式追踪，本文件聚焦**SRE 治理面**（on-call/告警疲劳/容量规划/平台工程融合）。
> - `29-云原生与DevOps工程实战` 聚焦 CI/CD 与容器编排，本文件聚焦**运行时的可靠性工程**（错误预算/burn rate/blameless postmortem）。
> - `50-软件架构模式与设计系统` 含可观测性模式提及，本文件聚焦**SRE 实战落地**（Grafana 技术栈搭建/混沌实验设计/eBPF 持续剖析）。
>
> **本文件独特主题**：用户导向可观测性、OpenTelemetry Collector/SDK 资源属性工程化、SLO/SLI 错误预算与 burn rate 策略、Incident Management 分级与 blameless 复盘、告警疲劳治理、容量规划方法论、混沌工程原理与工具、Continuous Profiling 与 eBPF、SRE 与平台工程融合、Grafana+Loki+Tempo+Prometheus 一站式搭建。

---

### CA01 可观测性三大支柱与用户导向可观测
- **预期档位**: simple
- **考察维度**: 三大支柱概念 + 用户导向可观测理念
- **对话脚本**:
  1. 可观测性（Observability）的三大支柱——Logs、Metrics、Traces——各自解决什么问题？用一个电商下单场景分别举例说明。
  2. 有人说"三大支柱不够"，要加上 Profiles 和 Events 成为五支柱。你同意吗？为什么可观测性不等于监控（Monitoring）？
  3. 什么是"用户导向可观测"（User-Centric Observability）？它和传统"机器导向可观测"的区别在哪？举个例子：后端服务 CPU 正常但用户投诉页面打不开，机器导向会漏掉什么？
  4. 结合上一轮的用户导向理念，给我三个"用户旅程埋点"的例子——不是后端 RED 指标，而是真正反映用户体验的信号。

### CA02 OpenTelemetry 工程化落地
- **预期档位**: medium
- **考察维度**: OTel Collector/SDK/资源属性工程化
- **对话脚本**:
  1. OpenTelemetry 的 SDK、Collector、Exporter 三层各自职责是什么？为什么需要 Collector 而不是 SDK 直连后端？
  2. OTel Collector 的 `receivers`、`processors`、`exporters`、`pipelines` 怎么用 config.yaml 串起来？写一个接收 OTLP → 批量处理 → 输出到 Jaeger + Prometheus 的管道配置。
  3. 资源属性（Resource Attributes）在 OTel 里起什么作用？用 `service.name`、`service.version`、`k8s.pod.name` 举例说明它在后端聚合时的价值。
  4. 接上一轮，如果生产环境有 200 个微服务，怎么用 Resource Attributes 做"按服务版本对比错误率"的查询？写出对应的 Prometheus 或 Tempo 查询思路。

### CA03 SLO/SLI 与错误预算策略
- **预期档位**: medium
- **考察维度**: SLO/SLI 定义 + 错误预算 + burn rate
- **对话脚本**:
  1. SLI、SLO、SLA 三者的区别是什么？为一个"用户登录 API"定义一个 SLI、一个 SLO、一个对应的错误预算（Error Budget）。
  2. 错误预算用完了该怎么办？给出三种常见策略（冻结功能发布/全员 on-call 修稳定性/接受风险并记录），并分析各自的适用场景。
  3. Burn Rate（消耗速率）是什么？为什么 Google SRE 书里用 burn rate 而不是直接用错误率来触发告警？计算一个 30 天 SLO 99.9% 的服务，burn rate = 2 时，意味着每小时允许的错误预算消耗百分比。
  4. 接上一轮的计算，设计一个基于 burn rate 的多窗口告警：快速消耗（1 小时窗口 burn rate > 14.4）和慢速消耗（3 天窗口 burn rate > 1）分别对应什么严重级别？

### CA04 Incident Management 与 Blameless Postmortem
- **预期档位**: medium
- **考察维度**: 事件分级 + on-call + 无责复盘
- **对话脚本**:
  1. 一个线上事故的分级（Severity 1/2/3）通常怎么定义？每级的响应时间、升级路径、通知范围有什么不同？
  2. On-call 轮值制度怎么设计才公平且可持续？谈一谈"跟随太阳"（Follow-the-Sun）轮值和"值班补偿"的实践。
  3. Blameless Postmortem（无责复盘）的核心原则是什么？为什么强调"人不该背锅，流程该背锅"？给出一个复盘文档应该包含的 5 个必要章节。
  4. 接上一轮的复盘章节，给我一个"数据库主从延迟导致写入丢失"事故的虚构复盘——重点写"系统怎么允许这件事发生"而不是"谁按错了按钮"。

### CA05 告警疲劳治理
- **预期档位**: medium
- **考察维度**: 告警质量 + 复合告警 + 降噪策略
- **对话脚本**:
  1. 告警疲劳（Alert Fatigue）是什么？列出三种常见的"垃圾告警"类型，并分别给出治理方法。
  2. 什么是"可行动告警"（Actionable Alert）？它和"信息性通知"的边界在哪？把"磁盘使用率 80%"这个告警改写成可行动版本。
  3. 复合告警（Composite Alert）或"告警聚合"怎么做？举例：50 台机器同时 CPU 飙升，应该合成一条"机房异常"而不是 50 条独立告警——设计聚合规则。
  4. 接上一轮，用分组（grouping）、抑制（inhibition）、静默（silencing）三个机制设计一个"依赖上游数据库的 10 个服务同时报错"场景的降噪方案。

### CA06 容量规划与负载测试
- **预期档位**: hard
- **考察维度**: 容量模型 + 负载测试 + 弹性伸缩
- **对话脚本**:
  1. 容量规划（Capacity Planning）的核心步骤是什么？给出一个"从当前流量到双十一 10 倍扩容"的规划框架。
  2. 负载测试、压力测试、浸泡测试（Soak Test）、尖峰测试（Spike Test）四种测试类型各自的目标是什么？各举一个对应的生产事故类型。
  3. 怎么建立"吞吐量 vs 延迟 vs 资源利用率"的容量模型？解释 Little's Law（L = λW）在容量规划里的用法。
  4. 接上一轮的容量模型，如果当前单实例 QPS 上限 1000、P99 延迟 200ms，目标是支持 10000 QPS 且 P99 < 500ms，最少需要多少实例？要不要考虑冷启动惩罚？

### CA07 混沌工程原理与工具实践
- **预期档位**: hard
- **考察维度**: 混沌工程原则 + 实验设计 + 工具选型
- **对话脚本**:
  1. 混沌工程（Chaos Engineering）的五大原则是什么？为什么"生产环境做实验"反而比"不试"更安全？
  2. 设计一个混沌实验：验证"单可用区故障时服务自动切换"。写明实验假设、注入故障、稳态假说、回滚条件、观测指标。
  3. ChaosMesh、LitmusChaos、ChaosBlade、Gremlin 四款工具的定位差异是什么？Kubernetes 场景首选哪个？为什么？
  4. 接上一轮的实验设计，做完实验后发现"自动切换"耗时 5 分钟（超过 SLO 的 1 分钟），列出三个后续改进方向并评估优先级。

### CA08 Continuous Profiling 与 eBPF 可观测
- **预期档位**: medium
- **考察维度**: 持续剖析 + eBPF 无侵入采集
- **对话脚本**:
  1. Continuous Profiling（持续剖析）和传统的按需 Profiling 有什么本质区别？为什么在生产环境持续开 5% CPU 开销做采样是值得的？
  2. Parca 和 Pyroscope 两款开源持续剖析工具各自的数据模型、采集方式、存储后端有什么不同？
  3. eBPF 在可观测性领域为什么是无侵入的？用 `bpftrace` 或 `BCC` 写一个追踪所有 `openat` 系统调用并统计每个进程打开次数的一行脚本。
  4. 接上一轮，如果通过 eBPF 发现某个 Go 服务每秒调用 `openat` 10 万次（远超正常），列出三个可能的原因及对应的排查命令。

### CA09 SRE 与平台工程融合
- **预期档位**: medium
- **考察维度**: SRE 文化 + 平台工程 + 内部开发者平台
- **对话脚本**:
  1. SRE（Site Reliability Engineering）和平台工程（Platform Engineering）的关系是什么？为什么说"平台工程是 SRE 文化的规模化输出"？
  2. 内部开发者平台（IDP）应该为开发团队提供哪些"可靠性自助能力"？列举 5 个并说明它如何减少对 SRE 团队的依赖。
  3. Golden Path（黄金路径）是什么？它怎么在"标准化"和"灵活性"之间取得平衡？举例：一条部署黄金路径长什么样？
  4. 接上一轮的黄金路径，如果开发团队走"非标准路径"绕过了 Golden Path，作为 SRE 怎么治理？给出三种策略并按强制程度排序。

### CA10 实战：用 Grafana 技术栈搭建可观测平台
- **预期档位**: hard
- **考察维度**: Grafana + Loki + Tempo + Prometheus 一站式搭建
- **对话脚本**:
  1. 用 Prometheus、Loki、Tempo、Grafana 四个开源组件搭一套可观测平台，分别承担什么角色？画出数据流向：应用 → 采集 → 存储 → 查询 → 展示。
  2. 给出一个 docker-compose.yml 的最小可用部署：四个容器各需要什么端口、什么环境变量、怎么互相引用？
  3. 应用侧怎么用 OTel SDK 同时发送 Metrics 到 Prometheus、Traces 到 Tempo、Logs 到 Loki？Resource Attributes 怎么保证三者在 Grafana 里能关联（TraceID 注入日志）？
  4. 接上一轮部署好后，创建一个 Grafana Dashboard 展示"下单请求"的黄金信号（RED 指标 + 日志 + 追踪）：描述面板布局、查询语句、变量过滤怎么设计。
  5. 最后一轮升级：如果这套平台本身挂了怎么办？谈谈"可观测平台的可观测性"——自监控（Self-Monitoring）应该怎么设计？
