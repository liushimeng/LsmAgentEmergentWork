# 128 CI/CD 流水线工程与 GitOps 实战

> 编号段 DT01–DT10 · 聚焦「持续集成/持续部署、GitOps 工作流、自动化发布流水线」：GitLab CI YAML / GitHub Actions 工作流 / Jenkins Pipeline / 构建缓存优化 / 制品管理 / 环境晋升（dev→staging→prod）/ ArgoCD Flux 声明式部署 / 版本语义化与 Changelog / 质量门禁 / 密钥注入与 Vault
>
> 与现有维度互补说明：
> - `29-云原生与DevOps工程实战`（AC 维）聚焦**DevOps 全景工具链**；本文件聚焦 **CI/CD 流水线工程实操与 GitOps 部署模式**。
> - `112-构建系统与编译缓存工程`（DB 维）聚焦**编译构建缓存**；本文件聚焦 **CI/CD 流水线编排**。
> - `53-软件供应链与SBOM工程`（BC 维）聚焦**供应链安全**；本文件聚焦 **流水线工程实现**。
> - `38-代码审查与团队协作工程`（AL 维）聚焦**Git 协作流程**；本文件聚焦 **自动化流水线工程**。
>
> **本文件独特主题**：.gitlab-ci.yml 阶段与依赖 / GitHub Actions matrix 与 reusable workflow / Jenkinsfile 声明式流水线 / 构建缓存 key 与 fallback / Nexus/Artifactory 制品库 / 环境晋升与审批门 / ArgoCD Application CRD / Flux Kustomization / sem-release 语义化版本 / SonarQube 质量门禁 / HashiCorp Vault 密钥注入。

---

### DT01 GitLab CI 基础流水线

- **预期档位**: medium
- **考察维度**: stage 定义 / job 依赖 / artifacts 传递
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/.gitlab-ci.yml`：定义 build/test/deploy 三个 stage，build job 执行 `echo "building..."` 并生成 build.log 作为 artifacts，test job 依赖 build 的 artifacts 执行测试，deploy job 仅在 main 分支触发。
  2. Bash：`python3 -c "import yaml; yaml.safe_load(open('.gitlab-ci.yml'))" 2>&1 | tee tmpPlan/agent-test/dt01_lint.log`（YAML 语法校验），断言无报错（echo $? == 0）。
  3. Read .gitlab-ci.yml，再 Write `tmpPlan/agent-test/dt01_extend.yml`：增加 lint stage（shellcheck + yamllint）、security stage（trivy 扫描）、cache 配置（key 基于 commit SHA，paths 缓存 node_modules）。
  4. Bash：`diff tmpPlan/agent-test/.gitlab-ci.yml tmpPlan/agent-test/dt01_extend.yml | grep -c '^[<>]'` 应 ≥ 5（至少 5 行差异）。

### DT02 GitHub Actions 工作流

- **预期档位**: medium
- **考察维度**: 触发条件 / matrix 矩阵 / 步骤复用
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/.github/workflows/ci.yml`：name CI、on push+pull_request、jobs build 含 strategy matrix（os: [ubuntu, macos, windows]、python-version: [3.10, 3.11, 3.12]）、steps 含 checkout/setup-python/install-deps/run-tests。
  2. Bash：`python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" 2>&1 | tee tmpPlan/agent-test/dt02_lint.log`，断言无报错。
  3. Read ci.yml，再 Write `tmpPlan/agent-test/.github/workflows/release.yml`：release 工作流——on push tags v*、steps 含 build/create-release/upload-artifact、env 含 GITHUB_TOKEN。
  4. Bash：`grep -c 'tags\|release\|upload' tmpPlan/agent-test/.github/workflows/release.yml` 应 ≥ 3。

### DT03 构建缓存优化

- **预期档位**: medium
- **考察维度**: 缓存 key/fallback/失效策略/跨分支共享
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dt03_cache.yml`：GitHub Actions cache 配置——key: ${{ runner.os }}-pip-${{ hashFiles('requirements.txt') }}、restore-keys 含 fallback 前缀、paths 缓存 pip cache 目录。
  2. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('dt03_cache.yml')); assert 'cache' in d or 'steps' in d" 2>&1`，断言通过。
  3. Read dt03_cache.yml，再 Write `tmpPlan/agent-test/dt03_analysis.md`：分析缓存命中率优化策略——lockfile 精确匹配 vs 前缀 fallback 的取舍、缓存大小限制（10GB/repo）、定期清理策略。
  4. Bash：`wc -l tmpPlan/agent-test/dt03_analysis.md` 应 ≥ 10（至少 10 行分析内容）。

### DT04 制品管理与版本发布

- **预期档位**: medium
- **考察维度**: 语义化版本 / Changelog 生成 / 制品上传
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dt04_release.yml`：GitHub Actions release 工作流——on workflow_dispatch 输入 version（patch/minor/major）、steps 含 standard-version 自动 bump + changelog + tag + create release + 上传二进制制品。
  2. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('dt04_release.yml'))" 2>&1 | tee tmpPlan/agent-test/dt04_lint.log`，断言无报错。
  3. Read dt04_release.yml，再 Write `tmpPlan/agent-test/CHANGELOG.md`：含 Unreleased 节（Added/Changed/Fixed/Removed）和最近 3 个版本的变更记录，遵循 Keep a Changelog 格式。
  4. Bash：`grep -c '^## ' tmpPlan/agent-test/CHANGELOG.md` 应 ≥ 4（Unreleased + 3 个版本）。

### DT05 环境晋升流水线

- **预期档位**: hard
- **考察维度**: 多环境/审批门/配置分离
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dt05_pipeline.yml`：GitLab CI 多环境流水线——stages: build→test→deploy_staging→approve→deploy_prod，deploy_staging 自动触发，deploy_prod 需 manual approval，environment name 区分 staging/production。
  2. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('dt05_pipeline.yml')); assert 'stages' in d" 2>&1`，断言通过。
  3. Read dt05_pipeline.yml，再 Write `tmpPlan/agent-test/dt05_config/` 目录：含 staging.env（DEBUG=true, API_URL=https://staging.api.laew.local）和 prod.env（DEBUG=false, API_URL=https://api.laew.local），Write 脚本 `load_env.sh` 根据 ENV 变量加载对应配置。
  4. Bash：`bash tmpPlan/agent-test/dt05_config/load_env.sh staging 2>&1 | grep -c 'staging.api'` 应 ≥ 1。

### DT06 ArgoCD GitOps 部署

- **预期档位**: hard
- **考察维度**: Application CRD/同步策略/自愈
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dt06_app.yml`：ArgoCD Application 资源——metadata name laew-api、project default、source repoURL/path/targetRevision、destination server/namespace、syncPolicy automated prune/selfHeal、syncOptions CreateNamespace=true。
  2. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('dt06_app.yml')); assert d['kind']=='Application'" 2>&1`，断言通过。
  3. Read dt06_app.yml，再 Write `tmpPlan/agent-test/dt06_rollouts.yml`：ArgoRollout 资源——strategy canary steps（setWeight 20→pause→setWeight 50→pause→setWeight 100）、analysisTemplate 引用成功率指标。
  4. Bash：`grep -c 'canary\|setWeight\|analysisTemplate' tmpPlan/agent-test/dt06_rollouts.yml` 应 ≥ 3。

### DT07 质量门禁与代码扫描

- **预期档位**: medium
- **考察维度**: SonarQube/trivy/scorecard/质量阈值
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dt07_quality.yml`：GitHub Actions 质量门禁工作流——jobs: lint（shellcheck + hadolint）、test（pytest + coverage threshold 80%）、security（trivy container scan + SARIF 上传）、quality_gate（SonarQube 质量阈检查）。
  2. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('dt07_quality.yml')); assert 'quality_gate' in str(d)" 2>&1`，断言通过。
  3. Read dt07_quality.yml，再 Write `tmpPlan/agent-test/dt07_badge.sh`：用 shields.io 生成 Markdown 徽章字符串（build-passing/coverage-85%/v1.2.3），输出 README.md 格式的徽章行。
  4. Bash：`bash tmpPlan/agent-test/dt07_badge.sh 2>&1 | grep -c 'shield.io\|img\|badge'` 应 ≥ 2。

### DT08 密钥注入与 Vault 集成

- **预期档位**: medium
- **考察维度**: 密钥管理/动态凭据/安全注入
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dt08_vault.yml`：GitLab CI 集成 HashiCorp Vault——idTokens 配置、secrets 从 vault path 注入、before_script 中 export 环境变量。
  2. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('dt08_vault.yml')); assert 'secrets' in str(d) or 'vault' in str(d)" 2>&1`，断言通过。
  3. Read dt08_vault.yml，再 Write `tmpPlan/agent-test/dt08_secrets.sh`：Vault CLI 操作演示——vault login（token 方式）、vault kv get secret/laew/prod、vault kv put 写入新密钥、输出密钥列表。
  4. Bash：`bash tmpPlan/agent-test/dt08_secrets.sh --dry-run 2>&1 | grep -c 'vault\|secret\|kv'` 应 ≥ 3。

### DT09 Flux GitOps 工作流

- **预期档位**: hard
- **考察维度**: Kustomization/Source/自动同步
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dt09_source.yml`：Flux GitRepository 资源——metadata name laew-repo、interval 1m、url ssh://git@github.com/laew/repo、branch main、ssh secretRef。
  2. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('dt09_source.yml')); assert d['kind']=='GitRepository'" 2>&1`，断言通过。
  3. Read dt09_source.yml，再 Write `tmpPlan/agent-test/dt09_kustomization.yml`：Flux Kustomization 资源——metadata name laew-app、interval 5m、sourceRef GitRepository/laew-repo、path ./deploy/overlays/prod、prune true、healthChecks 引用 Deployment。
  4. Bash：`grep -c 'Kustomization\|prune\|healthChecks' tmpPlan/agent-test/dt09_kustomization.yml` 应 ≥ 2。

### DT10 流水线可观测性与通知

- **预期档位**: medium
- **考察维度**: 构建指标/Slack通知/失败告警
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/dt10_notify.yml`：GitHub Actions 通知工作流——on workflow_run（CI 完成后触发）、jobs notify 含 if failure() 条件、steps 用 slackapi/slack-github-action 发送失败通知含 workflow/commit/actor 信息。
  2. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('dt10_notify.yml')); assert 'workflow_run' in str(d) or 'failure' in str(d)" 2>&1`，断言通过。
  3. Read dt10_notify.yml，再 Write `tmpPlan/agent-test/dt10_metrics.py`：用 python3 解析 CI 运行日志，统计成功率/平均耗时/失败率，输出 Markdown 仪表盘表格。
  4. Bash：`python3 tmpPlan/agent-test/dt10_metrics.py 2>&1 | grep -c '成功率\|耗时\|失败率'` 应 ≥ 2。
