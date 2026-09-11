# 129 基础设施即代码 IaC 与 Ansible 自动化运维

> 编号段 DU01–DU10 · 聚焦「用代码定义和管理基础设施、自动化配置与编排」：Terraform HCL 语法与状态管理 / Terraform module 复用 / Ansible Playbook 与 Role / Ansible Inventory 动态主机 / Cloud-Init 初始化 / Packer 镜像构建 / Pulumi 编程式 IaC / 基础设施测试（Terratest/Kitchen）/ Drift 检测与合规 / 多云编排策略
>
> 与现有维度互补说明：
> - `29-云原生与DevOps工程实战`（AC 维）聚焦**DevOps 工具链全景**；本文件聚焦 **IaC 工程实操**。
> - `22-分布式系统与微服务架构`（V 维）聚焦**微服务设计**；本文件聚焦**基础设施编排**。
> - `118-Linux服务器治理进阶systemd服务化与批量运维`（DJ 维）聚焦**单服务器运维**；本文件聚焦**多服务器编排**。
> - `112-构建系统与编译缓存工程`（DB 维）聚焦**编译缓存**；本文件聚焦**基础设施状态管理**。
>
> **本文件独特主题**：Terraform resource/data/provider 三要素 / state 远程锁与一致性 / module 输入输出变量 / Ansible task/handler/role 分层 / jinja2 模板与 filter / dynamic inventory 脚本 / cloud-init user-data / packer HCL builder/provisioner / 漂移检测与 import / checkov/tfsec 合规扫描 / 多 provider 多区域编排。

---

### DU01 Terraform 基础资源定义

- **预期档位**: medium
- **考察维度**: HCL 语法 / provider 配置 / resource 声明
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/main.tf`：provider "docker" 配置、resource "docker_image" nginx 拉取镜像、resource "docker_container" web 配置 image/ports/volumes、output 暴露容器 ID 和端口。
  2. Bash：`terraform -chdir=tmpPlan/agent-test init -backend=false 2>&1 | tee tmpPlan/agent-test/du01_init.log`（若 terraform 不可用则 `python3 -c "open('main.tf').read()"` 模拟），断含 "Initializing provider plugins" 或 HCL 解析成功。
  3. Read main.tf，再 Write `tmpPlan/agent-test/variables.tf`：定义变量 container_name（string, default "laew-web"）、host_port（number, default 8080）、environment（map(string)），在 main.tf 中引用 var.*。
  4. Bash：`grep -c 'variable\|var\.' tmpPlan/agent-test/main.tf tmpPlan/agent-test/variables.tf` 应 ≥ 4（变量定义+引用各至少 2 处）。

### DU02 Terraform Module 复用

- **预期档位**: medium
- **考察维度**: module 输入/输出/版本/源
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/modules/network/main.tf`：VPC module——输入变量 vpc_cidr/region/az_count，resource aws_vpc + aws_subnet * az_count，输出 vpc_id/subnet_ids。
  2. Bash：`python3 -c "import os; assert os.path.exists('modules/network/main.tf'); print('module structure ok')" 2>&1`，断言通过。
  3. Read network module，再 Write `tmpPlan/agent-test/production/main.tf`：module "network" source ../modules/network、传入变量、module "compute" 引用 network 输出的 subnet_ids。
  4. Bash：`grep -c 'module\|source\|vpc_id\|subnet_ids' tmpPlan/agent-test/production/main.tf` 应 ≥ 4。

### DU03 Ansible Playbook 编写

- **预期档位**: medium
- **考察维度**: task/handler/notify/条件/循环
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/playbook.yml`：hosts become yes、tasks 含 apt 安装 nginx、template 渲染 nginx.conf.j2、service 启动并 enable、handler 在配置变更时 restart nginx、notify 触发 handler。
  2. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('playbook.yml')); assert isinstance(d, list) and 'tasks' in d[0]" 2>&1`，断言通过。
  3. Read playbook.yml，再 Write `tmpPlan/agent-test/inventory`：INI 格式定义 webservers 和 dbservers 分组、ansible_host/ansible_user/ansible_ssh_private_key_file 变量。
  4. Bash：`grep -c '\[webservers\]\|\[dbservers\]\|ansible_host' tmpPlan/agent-test/inventory` 应 ≥ 3。

### DU04 Ansible Role 与 Jinja2 模板

- **预期档位**: medium
- **考察维度**: Role 目录结构 / 模板渲染 / 默认值
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/roles/nginx/` 目录结构：tasks/main.yml（安装+配置+启动）、handlers/main.yml、templates/nginx.conf.j2（用 {{ server_name }} {{ listen_port }} 变量）、defaults/main.yml（默认值）、meta/main.yml（依赖声明）。
  2. Bash：`ls tmpPlan/agent-test/roles/nginx/ 2>&1 | grep -cE 'tasks|handlers|templates|defaults|meta'` 应 ≥ 5（5 个标准子目录齐全）。
  3. Read roles/nginx/tasks/main.yml，再 Write `tmpPlan/agent-test/roles/nginx/templates/nginx.conf.j2`：含 server 块 listen {{ listen_port }} server_name {{ server_name }}、{% for upstream in upstreams %} 循环、{% if ssl_enabled %} SSL 配置。
  4. Bash：`grep -cE '\{\{.*\}\}|\{%.*%\}' tmpPlan/agent-test/roles/nginx/templates/nginx.conf.j2` 应 ≥ 3（至少 3 个 Jinja2 表达式）。

### DU05 Cloud-Init 实例初始化

- **预期档位**: medium
- **考察维度**: user-data / 用户创建 / 软件安装 / 启动脚本
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/cloud-init.yml`：#cloud-config 头、users 创建 laew-admin（ssh-authorized-keys）、package_update/package_upgrade true、packages 安装 docker.io/curl/vim、runcmd 执行启动命令、final_message 启动完成提示。
  2. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('cloud-init.yml')); assert d.get('#cloud-config') is None or True; assert 'users' in d" 2>&1`，断言通过。
  3. Read cloud-init.yml，再 Write `tmpPlan/agent-test/cloud-init-validate.sh`：用 `cloud-init devel schema --config-file` 校验语法（若不可用则用 python3 yaml 解析替代），输出校验结果。
  4. Bash：`bash tmpPlan/agent-test/cloud-init-validate.sh 2>&1 | grep -ci 'valid\|ok\|success'` 应 ≥ 1。

### DU06 Packer 镜像构建

- **预期档位**: hard
- **考察维度**: builder/provisioner/post-processor/HCL2
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/image.pkr.hcl`：packer 块 required_provisions、source "docker" "ubuntu" image="ubuntu:24.04" commit=true、build 块 sources 引用、provisioner "shell" 内联命令安装 python3/pip/nginx、post-processor "docker-tag" 标记镜像。
  2. Bash：`python3 -c "import re; t=open('image.pkr.hcl').read(); assert re.search(r'source.*docker', t) and re.search(r'provisioner.*shell', t)" 2>&1`，断言通过。
  3. Read image.pkr.hcl，再 Write `tmpPlan/agent-test/image-vars.pkr.hcl`：variable 定义 image_tag/version、在 source 中引用 var.image_tag、locals 定义 timestamp 标签。
  4. Bash：`grep -c 'variable\|var\.\|locals' tmpPlan/agent-test/image-vars.pkr.hcl` 应 ≥ 3。

### DU07 Pulumi 编程式 IaC

- **预期档位**: medium
- **考察维度**: TypeScript/Python 编程式资源 / Stack 状态 / 组件
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/Pulumi.yaml`：name laew-infra、runtime python、description "laew infrastructure"。Write `tmpPlan/agent-test/__main__.py`：import pulumi 和 pulumi_docker、定义 Image/Container/Network 资源、export 输出端口。
  2. Bash：`python3 -c "import ast; ast.parse(open('__main__.read()); print('syntax ok')" 2>&1`，断言通过。
  3. Read __main__.py，再 Write `tmpPlan/agent-test/Pulumi.production.yaml`：config 含 docker:host、app:replicas、environment: production。
  4. Bash：`python3 -c "import yaml; d=yaml.safe_load(open('Pulumi.production.yaml')); assert 'config' in d" 2>&1`，断言通过。

### DU08 基础设施测试与合规扫描

- **预期档位**: medium
- **考察维度**: Checkov/tfsec/OPA 策略即代码
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/du08_scan.sh`：用 checkov -d . 扫描 Terraform 配置（若不可用则模拟输出），检查 CKV_AWS_20（S3 公开访问）、CKV_AWS_23（安全组描述）、CKV_DOCKER_1（镜像 tag 非 latest），输出扫描报告。
  2. Bash：`bash tmpPlan/agent-test/du08_scan.sh 2>&1 | tee tmpPlan/agent-test/du08_scan.log`，断言含 "PASSED" 或 "FAILED" 或 "check" 字样。
  3. Read du08_scan.sh，再 Write `tmpPlan/agent-test/du08_rego.rego`：OPA Rego 策略——deny 公开 S3 bucket、deny 安全组 0.0.0.0/0 入站、allow 带 tag 的资源。
  4. Bash：`grep -c 'deny\|allow\|violation' tmpPlan/agent-test/du08_rego.rego` 应 ≥ 3。

### DU09 Drift 检测与状态修复

- **预期档位**: hard
- **考察维度**: terraform plan / state 漂移 / 资源 import
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/du09_drift.sh`：terraform plan -detailed-exitcode（0=无漂移/1=错/2=漂移）、解析退出码、输出 "DRIFT_DETECTED" 或 "IN_SYNC"、若漂移则 terraform import 恢复或 terraform apply 修复。
  2. Bash：`bash tmpPlan/agent-test/du09_drift.sh --dry-run 2>&1 | tee tmpPlan/agent-test/du09_drift.log`，断言含 "plan" 或 "drift" 或 "sync" 字样。
  3. Read du09_drift.sh，再 Write `tmpPlan/agent-test/du09_import.md`：列出手动创建的资源如何 import 到 state——terraform import 命令格式、多资源批量 import 脚本、state rm/mv 操作注意事项。
  4. Bash：`grep -cE 'terraform import|state rm|state mv' tmpPlan/agent-test/du09_import.md` 应 ≥ 2。

### DU10 多云编排与输出聚合

- **预期档位**: hard
- **考察维度**: 多 provider / 跨区域 / 输出聚合
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/du10_multi.tf`：provider "aws" alias east region us-east-1、provider "aws" alias west region us-west-2、resource 在双区域各部署一个 VPC、output 聚合两个区域的 VPC ID。
  2. Bash：`grep -cE 'provider.*aws|alias|region|output' tmpPlan/agent-test/du10_multi.tf` 应 ≥ 5。
  3. Read du10_multi.tf，再 Write `tmpPlan/agent-test/du10_aggregate.py`：用 python3 读取 terraform output -json，提取所有区域资源信息，生成 Markdown 多云资源清单表格。
  4. Bash：`python3 tmpPlan/agent-test/du10_aggregate.py 2>&1 | grep -c 'east\|west\|VPC\|区域'` 应 ≥ 2。
