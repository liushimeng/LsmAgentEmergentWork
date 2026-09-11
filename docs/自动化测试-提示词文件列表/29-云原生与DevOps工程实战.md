# 自动化测试提示词 — 云原生与 DevOps 工程实战（AC01–AC10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。

## 维度说明

本维度从 Agent 能力视角考察**交付工程化**链路：Dockerfile/compose/YAML 生成与静态校验、CI/CD 脚本化流水线、可观测性配置。本机不允许真实 build/pull/run 容器（网络慢且有副作用），一律「写配置文件 → python3 解析/校验断言 → bash 自查脚本」路径；CI/CD 用 make/bash 在本机模拟多阶段。所有产物落在 `tmpPlan/agent-test/`，零外网依赖。

---

### AC01 Dockerfile 多阶段优化
- **预期档位**: medium
- **考察维度**: 镜像分层 / 体积量化
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 用 Write 产出「差」版 `Dockerfile.bad`：单阶段 `FROM python:3.11` + `apt-get install gcc` + `pip install` + `COPY . .` + `CMD`。用 python3 写 `DockerfileInspector` 解析它：按 `RUN` 切层、估算每层命令数与「不可清理 apt 缓存」标记，输出 `bad-layers.txt`。Read 验证标记为 `unsafe_apt_cache` 的层数 ≥ 1。
  2. 写 `Dockerfile.good` 多阶段版：`FROM python:3.11-slim AS build` 装编译依赖 → `FROM python:3.11-slim` 只 COPY `site-packages`。再跑同一 `DockerfileInspector`，输出 `good-layers.txt`。Read 对比两文件，期望 `good-layers.txt` 层数比 `bad-layers.txt` 少 30% 且无 `unsafe_apt_cache` 标记。
  3. Bash 自查脚本 `lint_dockerfile.sh`：要求每条 `RUN` 后跟 `rm -rf /var/lib/apt/lists/*` 或 `&&` 合并、`.dockerignore` 存在且含 `.git`/`__pycache__`/`*.pyc`。故意把 good 版里 `rm` 那行删掉，跑 lint 期望退出码 1 且 stderr 含 `apt cache not cleaned`；恢复后通过。
  4. 写 `dockerfile-security.md` 总结：换 distroless（`gcr.io/distroless/python3`）后体积变化、运行用户改 `USER 1000`、镜像扫描（`trivy`）的接入方式；表格形式不超过 10 行。

### AC02 docker-compose 本地编排
- **预期档位**: medium
- **考察维度**: 编排 YAML / 依赖就绪
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `compose.yml`：app + postgres + redis 三服务，app 加 `depends_on: condition: service_healthy`。用 python3 解析器（`yaml.safe_load`）跑 `validate_compose.py`：期望检测到 `postgres.healthcheck.test` 与 `redis.healthcheck.test` 字段均存在；故意删除 redis healthcheck，再跑期望报错 `redis: healthcheck required by app`。Read 验证错误定位精确到服务名。
  2. 加 `volumes:` + `env_file: .env.app` 配置分离；写 `dev.yml` 与 `prod.yml` 两个 override 文件。验证 `docker-compose -f compose.yml -f dev.yml config`（不实际跑，仅用 yaml merge 模拟 `merge_yaml.py`）期望 dev 启用 `DEBUG=1` 而 prod 不启用。
  3. Bash 写热重载检查脚本 `check_volume_mount.sh`：要求 app 服务至少 1 个 `./src:/app/src` 类型 bind mount；故意把 bind mount 改成匿名卷，跑脚本期望退出码 1；恢复后通过。
  4. 模拟故障演练：写 `chaos_test.sh` 不实际停容器，而是解析 compose YAML 检查 `restart: on-failure` 与 `healthcheck` 同时存在；故意把 app 的 restart 改成 `no`，跑脚本期望警告。写 `compose-resilience.md` 总结优雅停止与重连策略。

### AC03 Kubernetes 部署 YAML
- **预期档位**: medium
- **考察维度**: K8s 必填字段 / 探针配置
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `deploy.yml`：Deployment(2 副本) + ClusterIP Service + ConfigMap + Ingress。python3 `k8s_validator.py` 检查所有 `metadata.name`、`spec.selector.matchLabels` 与 `template.metadata.labels` 三者一致；故意把 matchLabels 改一个字符，期望报错并指出三处不一致。
  2. 加 liveness/readiness/startup 三类探针。写 `probe_lint.py` 校验：liveness 路径 ≠ readiness 路径（避免互相影响）、startup `failureThreshold` ≥ liveness `failureThreshold`。故意把 liveness 与 readiness 设为同路径 `/health`，跑 lint 期望报错；恢复后通过。Read 验证错误提示。
  3. 资源 requests/limits 设置：写 `resource_calc.py` 给定应用 QPS=200、平均 RT=200ms 推导 `requests.cpu` 与 `limits.memory` 公式化数值。故意把 limits.cpu 设成 `100m`（明显过低），跑 `resource_calc.py --check` 期望警告；改合理后通过。
  4. 写 `k8s-rollout.md`：画 ASCII 拓扑图（Pod→Service→Ingress→Client），列出 4 个常见排障起点（`describe pod` / `logs --previous` / `get events` / `top pod`），不超过 10 行。

### AC04 K8s 故障排查剧本
- **预期档位**: hard
- **考察维度**: 排障方法论 / 闭环修复
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `events.jsonl`（模拟 K8s events 流，含 CrashLoopBackOff/OOMKilled/ProbeFailed/NodeNotReady 几类）。python3 `triage.py` 按关键字归类：期望对每条事件输出「类别 + 排查起点 + kubectl 命令模板」。故意在 events 里混入噪声行（随机 `Normal` 事件），跑 triage 期望噪声行被过滤。
  2. 写「502 排障剧本」脚本 `playbook_502.sh`：分层检查 Service endpoints、Ingress backend、NetworkPolicy egress。模拟生成 `endpoints.json`（空列表表示 Service 无 endpoint）+ `ingress.yml`（backend serviceName 写错），跑 playbook 期望按层定位到「endpoints 空 → 上游 selector 不匹配」。Read 验证剧本分支齐全。
 3. OOM 与 CPU 限流区分：写 `metrics.jsonl`（含 `container_memory_usage_bytes` 与 `container_cpu_cfs_throttled_seconds_total`）。`oom_vs_throttle.py` 根据两条数据相对阈值给出判定；故意造一份「内存稳定但 throttled 时间线性增长」的指标，期望判为 CPU 限流而非 OOM。
 4. 「发布后偶发 5xx」剧本：写 `publish_sop.sh` 串接 rolling update → readiness 误配检测（旧 pod 还未 ready 就切流量）→ 旧连接排空（preStop hook sleep）。故意把 readiness 配成 `tcpSocket: {port: 9999}`（端口不存在），跑剧本期望报错步骤 2；修复后通过。写 `5xx-playbook.md` 总结三要素因果链。

### AC05 GitHub Actions 流水线（本地模拟）
- **预期档位**: medium
- **考察维度**: CI YAML / 本地阶段化执行
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 用 Write 产出 `.github/workflows/ci.yml`：含 fmt/clippy/test 三个 job。python3 `gh_actions_validator.py` 解析：要求每个 `run` 命令非空、`actions/checkout` 是 `step[0]`、有 `cache: cargo`；故意把 checkout 移到第二个 step，期望报错。
  2. 本地 `Makefile` 模拟 CI 阶段：`fmt` / `clippy` / `test` 三个 target。`make ci` 期望顺序执行且任一失败后续 stage 跳过（`make -k` 关闭验证）。故意让 `clippy` 故意报 warning（写 `#[allow(unused)]` 测试），期望 `clippy` 阶段非 0 退出。
  3. 构建矩阵：扩展 yml 加 `strategy.matrix.os: [ubuntu-latest, macos-latest, windows-latest]`。`matrix_lint.py` 要求三个 OS 都存在对应 runner 引用；故意删除 macos 期望报错。
  4. 发布自动化：tag 触发 + 生成 changelog + 上传 release。写 `release_simulate.sh`：本地模拟打 tag 时跑 `git log --oneline` 生成 CHANGELOG.md，跑 `sha256sum` 生成 checksum 文件。故意构造一个 tag 但仓库为空，期望脚本失败并指明原因；写 `release-flow.md` 总结 OIDC 替代 PAT 的原理。

### AC06 GitOps: ArgoCD 声明式发布（静态校验）
- **预期档位**: medium
- **考察维度**: GitOps 范式 / 配置静态校验
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `applicationset.yml`（ArgoCD ApplicationSet）：含 repoURL/path/targetRevision/kustomize.namespace。`argocd_lint.py` 校验必填字段；故意删除 `targetRevision`，跑期望报错。
  2. 写 overlays 目录结构：`base/` 含 kustomization + deployment；`overlays/dev/` 与 `overlays/prod/` 各加 `kustomization.yaml` 覆盖 replicas 与 image tag。`kustomize_build.py` 用纯 python 解析 YAML 合并（不实际跑 kustomize CLI），期望 dev replicas=2、prod replicas=5 且 image tag 不同。
  3. 自愈能力静态校验：写 `drift_check.py`，对比「git 仓库期望状态」与「模拟运行时状态」（一个手工改过的 runtime.yml）期望输出 diff 列表。故意让 runtime 与 git 一致，期望 diff 列表空。
  4. 回滚剧本：写 `rollback_plan.sh` 对比「revert commit」与「ArgoCD 内置 rollback」两种路径，输出选择建议（带原因）。故意标记一个回滚会触发「数据库迁移不匹配」的危险场景，期望脚本警告。写 `gitops-rollback.md` 总结两种回滚风险。

### AC07 Terraform IaC 静态校验
- **预期档位**: hard
- **考察维度**: 状态管理 / 模块化
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `main.tf`：含 provider + 2 个 resource（云服务器 + 安全组）+ `terraform { backend "local" {} }`。`tf_lint.py` 校验：所有 resource 都有 `tags = local.common_tags` 引用；故意把一个 resource 的 tags 写成内联值，期望报错提示「应使用 common_tags 变量」。
  2. 写 `variables.tf` + `outputs.tf` + `modules/network/` 子模块。`module_check.py` 要求网络模块的 `required_providers` 与根模块一致；故意在子模块换 provider 版本，期望报错。
  3. 漂移检测静态模拟：写 `state.json`（terraform state 期望值）与 `runtime.json`（实际值模拟），`drift_detect.py` 输出 diff。故意让 runtime 多一台服务器，期望检测出 `resource added without apply`。
  4. workspace vs 目录分环境：写 `strategy.md` 对比两方案（每个环境单独 workspace 还是单独目录）。Bash 跑 `lint_workspace.sh` 校验 `*.tfvars` 与环境目录一一对应；故意制造一份没有对应 tfvars 的环境，期望报错。写 `tf-iac.md` 总结选型决策（≤ 10 行）。

### AC08 Prometheus + Grafana 监控配置
- **预期档位**: medium
- **考察维度**: 指标体系 / 告警规则
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `prometheus.yml`（scrape config）+ `rules/*.yml`（告警规则）。`prom_lint.py` 校验：scrape_interval ∈ [15s, 5m]、alert rule 表达式非空且有 `summary`/`description` 字段。故意把 scrape_interval 设成 `1s`（过密），期望报错。
  2. 为 HTTP 服务配指标：`http_requests_total{handler,method,status}` counter + `http_request_duration_seconds` histogram。`histogram_lint.py` 要求桶边界单调递增且覆盖 [0.005, 10]；故意让桶边界缺 0.5，期望报错。
  3. 多窗口燃烧率告警：写规则 `HighErrorRate: http_requests_total{status=~"5.."}/http_requests_total > 0.05 for 5m` + `MultiBurnRate: ... for 1h and ... for 5m`。`burn_rate_check.py` 验证多窗口组合正确；故意改一个窗口表达式，期望测试用例失败。
  4. 写「黄金四信号」仪表盘 JSON：`dashboard.json` 含 QPS/P99/错误率/饱和度四面板。`dashboard_lint.py` 要求每个 panel 都有 `expr` 与 `legendformat`；故意把一个 panel 的 legendformat 删掉，期望报错。写 `golden-signals.md` 总结告警分级与值班收敛。

### AC09 日志管道：结构化 + 保留策略
- **预期档位**: medium
- **考察维度**: 日志工程 / 成本治理
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 用 Write 产出 `app.log.jsonl`（10 行结构化日志：timestamp/level/trace_id/msg）。`jsonl_lint.py` 校验每行合法 JSON、必含 4 字段；故意把一行 msg 字段去掉，期望报错指出具体行号。
  2. 写 Promtail 配置 `promtail.yml` + Loki 标签规则；`label_cardinality_lint.py` 校验 labels 不含高基数字段（`request_id`/`user_id` 禁用）；故意把 request_id 加进 static_labels，期望报错。
  3. 采样与保留策略：写 `retention_policy.yml`：DEBUG 采样率 10%、INFO 100%、保留周期 7 天/30 天/90 天分级。`retention_calc.py` 验证日志总量与保留成本的估算公式；故意把 DEBUG 采样设成 100%，跑脚本期望成本估算异常偏高。
  4. 排障动线：写 `trace_id_correlate.py` 给定 trace_id 在 metrics + logs 两份数据里查找返回联动时间轴。故意构造一份 metrics 有但 logs 无的 trace_id，期望提示「该 span 未采集到日志」。写 `trace-correlate.md` 总结三时间轴对齐方法。

### AC10 发布策略：滚动 + 蓝绿 + 金丝雀
- **预期档位**: simple
- **考察维度**: 发布工程 / 回滚设计
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `strategy_sim.py`：模拟滚动/蓝绿/金丝雀三种发布策略的「部署时间 / 资源成本 / 回滚时长」三维对比。给定 app 实例数=10、版本大小=1GB、新版本启动时间=30s，输出三策略表格。Read 验证三维数据齐全。
  2. 金丝雀放量脚本 `canary_release.sh`：1%→5%→20%→100% 四阶段，每阶段 sleep 5s 后查「关键指标」模拟数据。故意把模拟指标的 error_rate 配成 0.08（>0.05 阈值），期望脚本在某阶段自动退出并标注「触发回切」；修复后通过。
  3. expand-contract 数据库变更：写 `db_migration.md` 描述三步（先加列 → 双写 → 再删列），对应 schema v1 → v2 → v3 三文件。`schema_compat.py` 校验旧代码访问新 schema 不报错；故意把 v2 改成删除某列，跑脚本期望「旧代码会找不到列」报错。
  4. 小团队发布 SOP：写 `publish_sop.md` 含检查单（10 项）、灰度观察窗口（30min）、回滚演练频率（每月 1 次），每项给可勾选复选框 `[ ]`。Read 验证 SOP 文件可读且 10 项齐全。