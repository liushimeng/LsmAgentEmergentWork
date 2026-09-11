# 127 容器编排与 Kubernetes 运维实战

> 编号段 DS01–DS10 · 聚焦「容器化部署、Kubernetes 集群管理、微服务运维」：Dockerfile 编写与多阶段构建 / Docker Compose 服务编排 / Kubernetes Pod/Deployment/Service/Ingress / kubectl 运维命令 / Helm Chart 打包 / ConfigMap/Secret 管理 / 持久化存储 PV/PVC / 滚动更新与回滚 / HPA 自动扩缩容
>
> 与现有维度互补说明：
> - `29-云原生与DevOps工程实战`（AC 维）聚焦**云原生工具链全景**；本文件聚焦 **Docker + K8s 日常运维操作**。
> - `22-分布式系统与微服务架构`（V 维）聚焦**架构设计**；本文件聚焦 **容器编排的工程实操**。
> - `118-Linux服务器治理进阶systemd服务化与批量运维`（DJ 维）聚焦 **systemd 服务**；本文件聚焦 **容器化服务编排**。
> - `33-嵌入式系统与物联网开发`（AG 维）聚焦**嵌入式 Linux**；本文件聚焦 **服务端容器运维**。
>
> **本文件独特主题**：Dockerfile 多阶段构建与层优化 / Compose 服务依赖与健康检查 / K8s 资源清单 YAML / kubectl 调试与排障 / Helm values 覆盖与模板 / ConfigMap/Secret 热更新 / PV/PVC 存储类 / 滚动更新策略 / HPA 指标与行为 / 命名空间与 RBAC。

---

### DS01 Dockerfile 多阶段构建

- **预期档位**: medium
- **考察维度**: 多阶段构建 / 层优化 / 最小镜像
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一份 `tmpPlan/agent-test/Dockerfile.multi`：多阶段构建 python3 应用——第一阶段 `python:3.12-slim` 安装依赖（requirements.txt），第二阶段 `python:3.12-alpine` 仅拷贝虚拟环境，`COPY --from=0`，`EXPOSE 8080`，`CMD ["python","app.py"]`。
  2. Bash：`docker build -f tmpPlan/agent-test/Dockerfile.multi -t laew-test:multi tmpPlan/agent-test/ 2>&1 | tee tmpPlan/agent-test/ds01_build.log`（若 docker 不可用则模拟），断言含 "Successfully built" 或层数信息。
  3. Read Dockerfile.multi，再 Write `tmpPlan/agent-test/Dockerfile.optimize`：合并 RUN 指令减少层数，用 `.dockerignore` 排除不需要的文件，用 `--no-cache` 演示干净构建。
  4. Bash：`diff <(grep '^RUN' tmpPlan/agent-test/Dockerfile.multi) <(grep '^RUN' tmpPlan/agent-test/Dockerfile.optimize)` 应显示优化后 RUN 行数减少。

### DS02 Docker Compose 服务编排

- **预期档位**: medium
- **考察维度**: Compose 服务定义 / 依赖管理 / 健康检查
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/docker-compose.yml`：定义 web（python:3.12）、db（postgres:16）、redis（redis:7）三个服务，含 depends_on + condition: service_healthy、ports 映射、volumes 挂载、environment 变量、healthcheck 配置。
  2. Bash：`docker compose -f tmpPlan/agent-test/docker-compose.yml config 2>&1 | tee tmpPlan/agent-test/ds02_config.log`（验证 YAML 解析），断言含 "web" 和 "db" 服务名。
  3. Read docker-compose.yml，再 Write `tmpPlan/agent-test/ds02_scale.sh`：用 `docker compose up -d --scale web=3` 演示扩容，`docker compose ps` 列出服务状态，`docker compose logs` 查看日志。
  4. Bash：`bash tmpPlan/agent-test/ds02_scale.sh` 后 `grep -c 'web' tmpPlan/agent-test/ds02_scale.log` 应 ≥ 3（3 个 web 实例）。

### DS03 Kubernetes Pod 与 Deployment

- **预期档位**: medium
- **考察维度**: 资源清单 / kubectl apply / 副本管理
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ds03_deploy.yml`：Deployment 资源——apiVersion apps/v1、metadata name laew-nginx、replicas 3、selector matchLabels app=nginx、template 含 container image nginx:1.27、ports 80、resources limits cpu/memory。
  2. Bash：`kubectl apply -f tmpPlan/agent-test/ds03_deploy.yml --dry-run=client -o yaml 2>&1 | tee tmpPlan/agent-test/ds03_dryrun.yaml`（dry-run 验证），断言含 "replicas: 3"。
  3. Read ds03_deploy.yml，再 Write `tmpPlan/agent-test/ds03_svc.yml`：Service 资源——type ClusterIP、selector app=nginx、port 80 targetPort 80，输出 ClusterIP。
  4. Bash：`kubectl apply -f tmpPlan/agent-test/ds03_svc.yml --dry-run=client -o yaml` 后 `grep -c 'ClusterIP' tmpPlan/agent-test/ds03_svc_dryrun.yaml` 应 ≥ 1。

### DS04 ConfigMap 与 Secret 管理

- **预期档位**: medium
- **考察维度**: 配置分离 / 密钥管理 / 热更新
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ds04_configmap.yml`：ConfigMap 资源——含 database_url/log_level/format 三个键值对，apiVersion v1、metadata name app-config。
  2. Bash：`kubectl apply -f tmpPlan/agent-test/ds04_configmap.yml --dry-run=client -o yaml 2>&1 | tee tmpPlan/agent-test/ds04_cm.yaml`，断言含 "database_url" 和 "log_level"。
  3. Read ds04_configmap.yml，再 Write `tmpPlan/agent-test/ds04_secret.yml`：Secret 资源——type Opaque、data 中的 username/password 用 base64 编码（echo -n "admin" | base64），标注需配合 Sealed Secrets 或 External Secrets 使用。
  4. Bash：`kubectl apply -f tmpPlan/agent-test/ds04_secret.yml --dry-run=client -o yaml` 后 `grep -c 'username\|password' tmpPlan/agent-test/ds04_secret.yaml` 应 ≥ 2。

### DS05 持久化存储 PV/PVC

- **预期档位**: medium
- **考察维度**: 存储类 / 持久卷声明 / 挂载
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ds05_pvc.yml`：PersistentVolumeClaim 资源——apiVersion v1、storageClassName standard、accessModes ReadWriteOnce、resources requests storage 1Gi。
  2. Bash：`kubectl apply -f tmpPlan/agent-test/ds05_pvc.yml --dry-run=client -o yaml 2>&1 | tee tmpPlan/agent-test/ds05_pvc.yaml`，断言含 "ReadWriteOnce" 和 "1Gi"。
  3. Read ds05_pvc.yml，再 Write `tmpPlan/agent-test/ds05_pod.yml`：Pod 资源——containers 挂载 PVC 到 /data 路径、volumes 引用 PVC 名称。
  4. Bash：`kubectl apply -f tmpPlan/agent-test/ds05_pod.yml --dry-run=client -o yaml` 后 `grep -c 'persistentVolumeClaim\|/data' tmpPlan/agent-test/ds05_pod.yaml` 应 ≥ 2。

### DS06 滚动更新与回滚

- **预期档位**: medium
- **考察维度**: 更新策略 / 回滚操作 / 健康探针
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ds06_deploy.yml`：Deployment 含 strategy type=RollingUpdate、maxSurge 1、maxUnavailable 0、minReadySeconds 10，template 含 livenessProbe + readinessProbe（httpGet path=/health port=8080）。
  2. Bash：`kubectl apply -f tmpPlan/agent-test/ds06_deploy.yml --dry-run=client -o yaml 2>&1 | tee tmpPlan/agent-test/ds06_deploy.yaml`，断言含 "RollingUpdate" 和 "livenessProbe"。
  3. Read ds06_deploy.yml，再 Write `tmpPlan/agent-test/ds06_rollback.sh`：`kubectl rollout undo deployment/laew-app`、`kubectl rollout history deployment/laew-app`、`kubectl rollout status` 演示回滚流程。
  4. Bash：`bash tmpPlan/agent-test/ds06_rollback.sh --dry-run 2>&1`（模拟），断言脚本含 "undo" 和 "history" 关键字。

### DS07 HPA 自动扩缩容

- **预期档位**: medium
- **考察维度**: 水平扩缩容 / 指标阈值 / 行为配置
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ds07_hpa.yml`：HorizontalPodAutoscaler 资源——scaleTargetRef 指向 Deployment、minReplicas 2、maxReplicas 10、metrics type=Resource resource=cpu targetAverageUtilization 70。
  2. Bash：`kubectl apply -f tmpPlan/agent-test/ds07_hpa.yml --dry-run=client -o yaml 2>&1 | tee tmpPlan/agent-test/ds07_hpa.yaml`，断含 "minReplicas: 2" 和 "maxReplicas: 10"。
  3. Read ds07_hpa.yml，再 Write `tmpPlan/agent-test/ds07_behavior.yml`：HPA behavior 配置——scaleUp stabilizationWindowSeconds 60、pods 扩容速度、scaleDown stabilizationWindowSeconds 300 防止抖动。
  4. Bash：`kubectl apply -f tmpPlan/agent-test/ds07_behavior.yml --dry-run=client -o yaml` 后 `grep -c 'stabilizationWindowSeconds' tmpPlan/agent-test/ds07_behavior.yaml` 应 ≥ 2。

### DS08 Helm Chart 打包与部署

- **预期档位**: hard
- **考察维度**: Chart 结构 / values 覆盖 / 模板渲染
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write Helm Chart 目录结构 `tmpPlan/agent-test/helloworld/`：Chart.yaml（apiVersion v2、name helloworld、version 0.1.0）、values.yaml（replicaCount 1、image repository/tag/pullPolicy）、templates/deployment.yaml（用 {{ .Values.image.repository }} 模板变量）。
  2. Bash：`helm template tmpPlan/agent-test/helloworld 2>&1 | tee tmpPlan/agent-test/ds08_template.yaml`，断言含 "helloworld" 和 "Deployment"。
  3. Read Chart.yaml，再 Write `tmpPlan/agent-test/ds08_values_prod.yaml`：生产环境 values 覆盖——replicaCount 3、image tag 指定版本、resources limits 配置。
  4. Bash：`helm template tmpPlan/agent-test/helloworld -f tmpPlan/agent-test/ds08_values_prod.yaml` 后 `grep -c 'replicaCount: 3' tmpPlan/agent-test/ds08_prod.yaml` 应 ≥ 1。

### DS09 命名空间与 RBAC 权限

- **预期档位**: medium
- **考察维度**: 多租户隔离 / 角色绑定 / 最小权限
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ds09_ns.yml`：Namespace 资源——apiVersion v1、metadata name laew-dev、labels env=dev team=platform。
  2. Bash：`kubectl apply -f tmpPlan/agent-test/ds09_ns.yml --dry-run=client -o yaml 2>&1 | tee tmpPlan/agent-test/ds09_ns.yaml`，断言含 "laew-dev"。
  3. Read ds09_ns.yml，再 Write `tmpPlan/agent-test/ds09_rbac.yml`：Role + RoleBinding——Role 允许 get/list/watch pods 和 logs，RoleBinding 绑定到 ServiceAccount laew-developer。
  4. Bash：`kubectl apply -f tmpPlan/agent-test/ds09_rbac.yml --dry-run=client -o yaml` 后 `grep -c 'Role\|RoleBinding\|pods\|logs' tmpPlan/agent-test/ds09_rbac.yaml` 应 ≥ 4。

### DS10 Ingress 与网络策略

- **预期档位**: hard
- **考察维度**: 入口路由 / TLS 终止 / 网络隔离
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ds10_ingress.yml`：Ingress 资源——apiVersion networking.k8s.io、rules host api.laew.local http paths 指向 backend service、tls secretName laew-tls。
  2. Bash：`kubectl apply -f tmpPlan/agent-test/ds10_ingress.yml --dry-run=client -o yaml 2>&1 | tee tmpPlan/agent-test/ds10_ingress.yaml`，断言含 "api.laew.local" 和 "laew-tls"。
  3. Read ds10_ingress.yml，再 Write `tmpPlan/agent-test/ds10_netpol.yml`：NetworkPolicy 资源——podSelector matchLabels app=api、ingress 仅允许来自 namespace label env=prod 的流量、egress 允许 DNS 和特定端口。
  4. Bash：`kubectl apply -f tmpPlan/agent-test/ds10_netpol.yml --dry-run=client -o yaml` 后 `grep -c 'NetworkPolicy\|ingress\|egress' tmpPlan/agent-test/ds10_netpol.yaml` 应 ≥ 3。
