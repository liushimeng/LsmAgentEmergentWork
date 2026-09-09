# 自动化测试提示词 — 云原生与 DevOps 工程实战（AC01–AC10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。

## 维度说明

本维度考察 Agent 在**云原生工具链与交付流程**中的实战能力：容器镜像优化、
compose 编排、Kubernetes 部署与排障、CI/CD 流水线、GitOps、Terraform IaC、
监控告警、日志管道、发布策略。与 V 维度（分布式架构理论）和 C 维度（单机系统管理）
互补：本维度聚焦"从代码到生产"的工程化交付实操，
覆盖"工作日常-Work"中研发效能与运维协作的高频场景。

---

### AC01 Dockerfile 优化：从 1.2GB 到 80MB
- **预期档位**: medium
- **考察维度**: 镜像分层 + 构建优化
- **对话脚本**:
  1. 一份 Python 应用的 Dockerfile 构建出 1.2GB 镜像：分析层缓存失效的原因与体积都花在了哪里。
  2. 基于分析结果重构为多阶段构建：构建期装编译依赖、运行期只带产物；解释每个阶段的职责边界。
  3. 继续瘦身：slim 基础镜像、合并 RUN 清理 apt 缓存、.dockerignore 排除；给出每步的体积对照表。
  4. 安全加固：换 distroless/无 shell 基础镜像、以非 root 用户运行、镜像漏洞扫描工具怎么选？

### AC02 docker-compose 本地开发全家桶
- **预期档位**: medium
- **考察维度**: 本地编排 + 依赖就绪
- **对话脚本**:
  1. 为一个 Web 应用编写 docker-compose.yml：应用 + PostgreSQL + Redis 三服务，用健康检查保证依赖就绪后再启动应用。
  2. 数据持久化与配置分离：卷挂载数据库、环境变量注入、用 override 文件区分 dev 与 prod。
  3. 热重载开发体验：源码以卷挂载进容器、应用自动重载；讨论数据库迁移这种"一次性任务"在容器里怎么跑。
  4. 模拟故障演练：`docker stop` 数据库容器后应用表现如何？加上优雅停止与健康检查重连策略。

### AC03 Kubernetes 部署一个 Web 服务
- **预期档位**: medium
- **考察维度**: K8s 核心对象 + 探针配置
- **对话脚本**:
  1. 讲清 Pod/Deployment/Service/ConfigMap/Ingress 各自的职责与关系，画出一张访问链路拓扑。
  2. 为应用写一套 YAML：2 副本 Deployment、ClusterIP Service、ConfigMap 注入配置。
  3. 配置 liveness 与 readiness 探针：两者应该探测什么路径？把 readiness 误配成 liveness 会发生什么？
  4. 资源管理：requests/limits 设置不当导致的 CPU throttling 与 OOMKilled；给出容量估算方法。

### AC04 K8s 故障排查实战
- **预期档位**: hard
- **考察维度**: 集群排障方法论
- **对话脚本**:
  1. Pod 状态 CrashLoopBackOff：给出从 `kubectl describe` / `logs --previous` / events 入手的完整排查清单。
  2. 服务内部能通、外部访问 502：从 Service/Endpoint/Ingress/NetworkPolicy 逐层定位的思路。
  3. OOMKilled 与 CPU 限流：怎么从监控数据区分是 limits 配太紧还是代码内存泄漏？
  4. 写一个"发布后偶发 5xx"的排查剧本：滚动更新、就绪探针误配、旧连接排空三个要素怎么串成一条因果链？

### AC05 GitHub Actions：从手工发布到流水线
- **预期档位**: medium
- **考察维度**: CI/CD + 密钥管理
- **对话脚本**:
  1. 为一个 Rust 项目写 CI：push 时跑 fmt/clippy/test，并用缓存把 cargo 依赖下载加速到秒级。
  2. 加构建矩阵：三个平台的交叉编译产物；再补失败重试与超时控制。
  3. 发布自动化：打 tag 触发构建、生成 changelog、上传 Release 附件并附 checksum 校验文件。
  4. 密钥管理：secrets 的作用域、用 OIDC 替代长期 token 的原理；branch protection 怎么与必过检查联动？

### AC06 GitOps：ArgoCD 声明式发布
- **预期档位**: medium
- **考察维度**: 声明式交付 + 环境管理
- **对话脚本**:
  1. GitOps 与传统 CI 驱动部署的核心区别是什么？"集群状态 = 仓库状态"这句口号意味着什么？
  2. 用 ArgoCD 部署一个应用：Application 资源、自动同步、自愈能力（手工 kubectl 改动被还原）逐项演示。
  3. 多环境管理：overlays 目录结构、dev 预览分支与 prod 主干分支的差异、发布审批门禁怎么加？
  4. 回滚演练：发布坏版本后，ArgoCD 内置回滚与 Git revert 两种路径的差异与各自风险。

### AC07 Terraform：基础设施即代码
- **预期档位**: hard
- **考察维度**: IaC + 状态管理
- **对话脚本**:
  1. 讲清 Terraform 的声明式模型：provider/resource/state 三者关系，plan 与 apply 两阶段的意义。
  2. 写一组配置创建云服务器 + 安全组 + 对象存储；解释 state 文件为什么不能提交进 git、远程 backend 怎么配置。
  3. 模块化：把网络层抽成可复用 module，设计输入输出变量；讨论"漂移检测"为什么重要。
  4. 团队协作：state 锁、命名规范、workspace 与目录分环境两种方案的取舍。

### AC08 Prometheus + Grafana 监控告警
- **预期档位**: medium
- **考察维度**: 指标体系 + 告警设计
- **对话脚本**:
  1. 讲清 Pull 模型、exporter、PromQL 基本查询；counter/gauge/histogram/summary 四种指标类型分别适合什么？
  2. 为一个 HTTP 服务接指标：QPS、P99 延迟、错误率；直方图的桶边界怎么规划才合理？
  3. 写告警规则：基于错误率与延迟的多窗口燃烧率（burn rate）告警，避免毛刺误报。
  4. 用 Grafana 出一块"黄金四信号"仪表盘；讨论告警分级与值班收敛——谁应该在什么时候被打扰？

### AC09 日志管道：从 print 到可检索
- **预期档位**: medium
- **考察维度**: 日志工程 + 成本治理
- **对话脚本**:
  1. 应用日志为什么要结构化（JSON）？统一时间/级别/trace_id 字段对检索与关联分析的意义。
  2. 搭一套 Promtail + Loki + Grafana：采集容器 stdout；标签设计为什么要避开高基数标签？
  3. 采样与保留策略：DEBUG 日志采样、按团队分级保留周期；日志与指标的成本权衡怎么做？
  4. 设计一次故障的排障动线：从告警指标跳到日志、再跳到 trace，三者的时间轴怎么对齐？

### AC10 发布策略：滚动、蓝绿、金丝雀
- **预期档位**: medium
- **考察维度**: 发布工程 + 回滚设计
- **对话脚本**:
  1. 对比滚动、蓝绿、金丝雀三种发布策略的停机时间、回滚速度、资源成本；各举一个最适合的业务场景。
  2. 金丝雀实操：按流量比例逐步放量，关键指标异常时自动回切的判断条件怎么定？
  3. 数据库变更与代码发布的解耦：expand-contract（先加列、双写、再删列）模式讲解。
  4. 为一个"每周两次发布"的小团队设计发布 SOP：检查单、灰度观察窗口、回滚演练频率各定多少？
