# 246 DevOps 工具链与部署工程实战

> 编号段 DOP01–DOP10 · 聚焦「laew 搭建完整 DevOps 工具链」: Dockerfile 多阶段构建 / docker-compose 编排 / GitHub Actions / GitLab CI / 蓝绿部署 / 滚动更新 / 回滚策略 / 配置中心(vault / 环境变量) / 制品管理 / 发布检查清单 / ChatOps 通知。

> 与现有维度互补说明:
> - `29-云原生与DevOps工程实战`(CLOUD 维)是**云原生理论与工具**;本文件是**DevOps 工具链编程**。
> - `128-CI-CD流水线工程与GitOps实战`(CI 维)是**流水线配置**;本文件是**DevOps 工具链全面工程**。
> - `127-容器编排与Kubernetes运维实战`(K8S 维)是**K8S 运维**;本文件是**容器化 + CI/CD + 部署全栈**。
> - `129-基础设施即代码IaC与Ansible自动化运维`(IaC 维)是**IaC**;本文件是**应用交付工具链**。
> - `155-跨平台安装包构建与软件分发工程`(DIST 维)是**桌面端分发**;本文件是**服务端部署**。
>
> **本文件独特主题**:Dockerfile 多阶段(build + run 分离) / docker-compose(服务编排 + 健康检查 + 资源限制) / CI(GitHub Actions matrix + cache + secret) / 部署策略(蓝绿 / 滚动 / 金丝雀) / 回滚(版本制品 + DB 迁移回退) / 配置中心(环境变量 + Vault 注入) / 制品管理(镜像 tag 策略 + SBOM) / 发布前检查清单(冒烟 / 迁移 / 配置) / ChatOps(Slack/飞书通知)。

---

### DOP01 多阶段 Dockerfile 构建

- **预期档位**: medium
- **考察维度**: 多阶段构建 / 镜像瘦身 / 缓存优化 / .dockerignore
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dop-docker/Dockerfile` + `.dockerignore`:(a) 阶段 1 `builder`:`FROM python:3.12-slim` + `COPY requirements.txt` + `pip install --user --no-cache-dir -r requirements.txt`;(b) 阶段 2 `runtime`:`FROM python:3.12-slim` + `COPY --from=builder /root/.local /root/.local` + `COPY . /app` + `USER 1000` + `ENTRYPOINT ["python", "-m", "app"]`;(c).dockerignore:`__pycache__/` + `.git/` + `*.pyc` + `tests/` + `.env` + `*.md`;(d) 健康检查:`HEALTHCHECK --interval=30s CMD curl -f http://localhost:8000/health || exit 1`。
  2. Bash:`grep -c 'FROM' tmpPlan/agent-test/dop-docker/Dockerfile` 断言 = 2(两阶段);`grep -q 'AS builder' tmpPlan/agent-test/dop-docker/Dockerfile && grep -q 'COPY --from=builder' tmpPlan/agent-test/dop-docker/Dockerfile && echo OK`。
  3. Read:加"`.dockerignore` 验证脚本"(检查是否遗漏敏感文件如 `.env` / `*.pem`)。
  4. Bash:`grep -q 'HEALTHCHECK' tmpPlan/agent-test/dop-docker/Dockerfile && grep -q 'USER 1000' tmpPlan/agent-test/dop-docker/Dockerfile && test -f tmpPlan/agent-test/dop-docker/.dockerignore && echo PASS`。

### DOP02 Docker Compose 编排

- **预期档位**: medium
- **考察维度**: 服务编排 / 网络隔离 / 数据卷 / 健康检查 / 资源限制
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dop-compose/docker-compose.yml`:(a) 服务:`app`(build + ports + depends_on db/restart + healthcheck) + `db`(postgres:16 + volumes pgdata + POSTGRES_PASSWORD_FILE run/secrets) + `redis`(image + command redis-server --requirepass) + `nginx`(image + volumes nginx.conf + ports 80/443);(b) 网络:`frontend`(nginx+app) + `backend`(app+db+redis);(c) 数据卷:`pgdata`(db 持久化) + `nginx-certs`(TLS 证书);(d) 资源:`deploy.resources.limits.cpus: '0.5' memory: 512M`;(e) 重启策略:`restart: unless-stopped`。
  2. Bash:`grep -c '    [a-z_-]*:$' tmpPlan/agent-test/dop-compose/docker-compose.yml` 断言服务 ≥ 4;`grep -q 'depends_on' tmpPlan/agent-test/dop-compose/docker-compose.yml && grep -q 'healthcheck' tmpPlan/agent-test/dop-compose/docker-compose.yml && echo OK`。
  3. Read:加"日志驱动(logging driver json-file + 大小限制)",Write 回文件。
  4. Bash:`grep -q 'networks:' tmpPlan/agent-test/dop-compose/docker-compose.yml && grep -q 'volumes:' tmpPlan/agent-test/dop-compose/docker-compose.yml && grep -q 'deploy:' tmpPlan/agent-test/dop-compose/docker-compose.yml && echo PASS`。

### DOP03 GitHub Actions CI 流水线

- **预期档位**: medium
- **考察维度**: Job 矩阵 / Cache / Secret / Artifact / 条件触发
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dop-ci/.github/workflows/ci.yml`:(a) 触发:`push` 到 main/develop + `pull_request` 到 main + `schedule`(每周一凌晨);(b) jobs:`lint`(ruff + mypy, 快速反馈) + `test`(matrix: python: [3.10, 3.11, 3.12] × os: [ubuntu, macos, windows], service: postgres + redis) + `build`(test 通过后, docker build + push GHCR) + `deploy`(build 通过后, 推 staging → 审批 → prod);(c) cache:`pip` 缓存 key `${{ runner.os }}-pip-${{ hashFiles('requirements.txt') }}`;(d) secret:`DOCKER_USERNAME` / `DOCKER_PASSWORD` / `DEPLOY_KEY`;(e) artifact:测试覆盖率 HTML 上传 + 保留 7 天。
  2. Bash:`grep -c 'python-' tmpPlan/agent-test/dop-ci/.github/workflows/ci.yml` 断言 matrix ≥ 3;`grep -q 'cache' tmpPlan/agent-test/dop-ci/.github/workflows/ci.yml && grep -q 'secrets' tmpPlan/agent-test/dop-ci/.github/workflows/ci.yml && echo OK`。
  3. Read:加"安全扫描(job: trivy 镜像扫描 + bandit 代码扫描)",Write 回文件。
  4. Bash:`grep -q 'matrix:' tmpPlan/agent-test/dop-ci/.github/workflows/ci.yml && grep -q 'needs:' tmpPlan/agent-test/dop-ci/.github/workflows/ci.yml && grep -q 'workflow_dispatch' tmpPlan/agent-test/dop-ci/.github/workflows/ci.yml && echo PASS`。

### DOP04 蓝绿部署脚本

- **预期档位**: hard
- **考察维度**: 流量切换 / 健康验证 / 回滚 / 会话保持
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dop-bluegreen/deploy.sh`:(a) 部署流程:`1. 构建新镜像 v2 → 2. 启动 green 环境(端口 8001)→ 3. 健康检查(3 次 × 5s)→ 4. 切换 nginx upstream 到 green → 5. 等待 30s 观察错误率 → 6. 成功:销毁 blue / 失败:切回 blue + 销毁 green`;(b) 健康检查:`curl -sf http://green:8000/health || exit 1`;(c) 错误率监控:`curl -s http://nginx:9090/stats | jq '.error_rate'`;(d) 回滚:`rollback.sh`:切回上一版本 upstream + 销毁有问题的环境;(e) 通知:部署开始/成功/失败都发 Slack webhook。
  2. Bash:`bash -n tmpPlan/agent-test/dop-bluegreen/deploy.sh && echo SYNTAX OK`;`grep -q 'green' tmpPlan/agent-test/dop-bluegreen/deploy.sh && grep -q 'health' tmpPlan/agent-test/dop-bluegreen/deploy.sh && echo OK`。
  3. Read:加"会话保持策略(切换时等待现有长连接结束)",Write 回文件。
  4. Bash:`grep -q 'upstream' tmpPlan/agent-test/dop-bluegreen/deploy.sh && grep -q 'rollback' tmpPlan/agent-test/dop-bluegreen/deploy.sh && grep -q 'slack' tmpPlan/agent-test/dop-bluegreen/deploy.sh && echo PASS`。

### DOP05 金丝雀发布流水线

- **预期档位**: hard
- **考察维度**: 流量权重 / 指标对比 / 自动回滚 / 渐进放量
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dop-canary/canary.py`:金丝雀发布控制器:(a) 阶段:5% → 25% → 50% → 100%,每阶段持续 10 分钟;(b) 指标对比:`error_rate_new` vs `error_rate_old`(新版本错误率 > 老版本 2 倍 → 自动回滚);(c) 流量权重:`nginx.conf` 动态生成 `upstream { server old:8000 weight=95; server new:8001 weight=5; }`;(d) 自动推进:每阶段结束检查指标,通过则推进下一阶段,否则回滚;(e) 日志:每阶段记录 `{stage, weight, error_rate, p99_latency, decision}`。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/dop-canary/canary.py').read()); print('OK')"`;`grep -q 'weight' tmpPlan/agent-test/dop-canary/canary.py && grep -q 'error_rate' tmpPlan/agent-test/dop-canary/canary.py && echo OK`。
  3. Read:加"A/B 分流(基于用户 ID hash,同用户始终走同一版本)",Write 回文件。
  4. Bash:`grep -q 'canary' tmpPlan/agent-test/dop-canary/canary.py && grep -q 'rollback' tmpPlan/agent-test/dop-canary/canary.py && grep -q 'progress' tmpPlan/agent-test/dop-canary/canary.py && echo PASS`。

### DOP06 配置中心与 Secret 管理

- **预期档位**: medium
- **考察维度**: 环境变量层级 / Vault 注入 / Secret 加密 / 热更新
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dop-config/config.py`:(a) 优先级:`命令行参数 > 环境变量 > .env.{ENV} > .env > 默认值`;(b) BaseSettings:`pydantic-settings` 自动读 `APP_` 前缀;(c) Secret 引用:`DB_PASSWORD = vault://secret/data/db`(运行时从 Vault 拉取);(d) 热更新:`watchdog` 监听 `.env` 文件变化,debounce 1s 后重载;(e) 脱敏日志:`__repr__` 把 Secret 字段替换为 `***`;(f) 校验:启动时校验必填项 + 格式(URL/端口/邮箱)。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/dop-config/config.py').read()); print('OK')"`;`grep -q 'BaseSettings' tmpPlan/agent-test/dop-config/config.py && grep -q 'vault' tmpPlan/agent-test/dop-config/config.py && echo OK`。
  3. Read:加"多环境差异对比函数"(dev vs staging vs prod 配置 diff)。
  4. Bash:`grep -q 'environ' tmpPlan/agent-test/dop-config/config.py && grep -q 'watchdog' tmpPlan/agent-test/dop-config/config.py && grep -q '\*\*\*' tmpPlan/agent-test/dop-config/config.py && echo PASS`。

### DOP07 制品管理与版本策略

- **预期档位**: medium
- **考察维度**: 语义化版本 / 镜像 Tag / SBOM 生成 / 制品库
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dop-artifact/release.py`:(a) 版本生成:`git tag` 命名 `v{major}.{minor}.{patch}` + `git describe --tags` 含 commit hash 作为预发布版本;(b) 镜像 Tag:`app:latest` + `app:v1.2.3` + `app:sha-abc1234`;(c) SBOM 生成:`syft` 生成 SPDX JSON + `grype` 扫描漏洞;(d) 制品库:推送到 GHCR / Docker Hub / 私有 Harbor;(e) 版本锁定:`requirements.txt` 全部 hash 锁定(`pip-compile --generate-hashes`);(f) 变更日志:从 git log 自动生成 CHANGELOG.md(按 conventional commits 分类 feat/fix/docs/chore)。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/dop-artifact/release.py').read()); print('OK')"`;`grep -q 'semver' tmpPlan/agent-test/dop-artifact/release.py && grep -q 'sbom' tmpPlan/agent-test/dop-artifact/release.py && echo OK`。
  3. Read:加"签名验证(cosign 签名镜像 + 公钥验证)",Write 回文件。
  4. Bash:`grep -q 'v1\.' tmpPlan/agent-test/dop-artifact/release.py && grep -q 'CHANGELOG' tmpPlan/agent-test/dop-artifact/release.py && grep -q 'latest' tmpPlan/agent-test/dop-artifact/release.py && echo PASS`。

### DOP08 发布检查清单自动化

- **预期档位**: medium
- **考察维度**: 冒烟测试 / 配置校验 / 迁移检查 / 依赖健康
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dop-checklist/pre_deploy_check.py`:(a) 冒烟测试:`curl -sf $URL/health` + `/ready` + `/version`;(b) 配置校验:环境变量是否齐全 + 格式正确 + 无默认密码;(c) 迁移检查:DB 当前版本 vs 代码期望版本(不能回退);(d) 依赖健康:DB / Redis / 第三方 API 连通性;(e) 安全:HTTPS 证书有效期 > 7 天 + 安全头(CSP/HSTS)检查;(f) 输出:PASS/FAIL 列表 + 阻断项(FAIL → 中止部署)。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/dop-checklist/pre_deploy_check.py').read()); print('OK')"`;`grep -q 'health' tmpPlan/agent-test/dop-checklist/pre_deploy_check.py && grep -q 'migrate' tmpPlan/agent-test/dop-checklist/pre_deploy_check.py && echo OK`。
  3. Read:加"成本检查(云资源配额 + 预估月费是否超阈值)",Write 回文件。
  4. Bash:`grep -q 'PASS' tmpPlan/agent-test/dop-checklist/pre_deploy_check.py && grep -q 'FAIL' tmpPlan/agent-test/dop-checklist/pre_deploy_check.py && grep -q 'cert' tmpPlan/agent-test/dop-checklist/pre_deploy_check.py && echo PASS`。

### DOP09 ChatOps 机器人

- **预期档位**: hard
- **考察维度**: 命令解析 / 权限校验 / 部署触发 / 状态回调
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dop-chatops/bot.py`:(a) 命令:`/deploy <env> <service> <version>` + `/rollback <env> <service>` + `/status <env>` + `/logs <service> <lines>` + `/approve <deployment_id>`;(b) 权限:`ALLOWED_USERS` 列表 + 命令级别权限(deploy 需 admin,view 可 anyone);(c) 部署触发:调用 CI API 启动部署流水线,返回 deployment_id;(d) 状态回调:CI 完成时 webhook 推送到 bot,bot 发消息到频道;(e) 审批:prod 部署需 1 人 approve,bot 发审批消息 + 按钮;(f) 审计:所有命令记录到 `audit_log`(user, command, ts, result)。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/dop-chatops/bot.py').read()); print('OK')"`;`grep -q '/deploy' tmpPlan/agent-test/dop-chatops/bot.py && grep -q 'approve' tmpPlan/agent-test/dop-chatops/bot.py && echo OK`。
  3. Read:加"自然语言查询(用 LLM 解析 '把订单服务部署到 staging' → 命令)",Write 回文件。
  4. Bash:`grep -q 'ALLOWED_USERS' tmpPlan/agent-test/dop-chatops/bot.py && grep -q 'webhook' tmpPlan/agent-test/dop-chatops/bot.py && grep -q 'audit' tmpPlan/agent-test/dop-chatops/bot.py && echo PASS`。

### DOP10 完整 DevOps 工程套件

- **预期档位**: hard
- **考察维度**: 端到端集成 / 一键部署 / 文档 / 监控对接
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dop-suite/` 完整套件:(a) `Makefile`:`make build` + `make test` + `make deploy-staging` + `make deploy-prod` + `make rollback` + `make status` + `make logs`;(b) `scripts/`:build.sh + test.sh + deploy.sh + rollback.sh + notify.sh;(c) `config/`:dev.env + staging.env + prod.env + vault-policy.hcl;(d) `monitoring/`:prometheus-alerts.yml + grafana-dashboard.json + loki-config.yml;(e).github/workflows/:ci.yml + release.yml + security-scan.yml;(f) `README.md`:架构图 + 快速开始 + 故障排查 + 贡献指南。
  2. Bash:`find tmpPlan/agent-test/dop-suite -type f | wc -l` 断言 ≥ 15;`grep -q 'deploy' tmpPlan/agent-test/dop-suite/Makefile && echo OK`。
  3. Read:加"故障演练场景(scripts/chaos.sh 杀 pod / 断网 / 满盘)",Write 回文件。
  4. Bash:`test -f tmpPlan/agent-test/dop-suite/Makefile && test -f tmpPlan/agent-test/dop-suite/docker-compose.yml && test -f tmpPlan/agent-test/dop-suite/scripts/deploy.sh && echo PASS`。
