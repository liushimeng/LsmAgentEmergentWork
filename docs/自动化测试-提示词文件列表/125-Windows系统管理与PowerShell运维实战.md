# 125 Windows 系统管理与 PowerShell 运维实战

> 编号段 DQ01–DQ10 · 聚焦「Windows 平台下的系统管理、PowerShell 脚本工程、运维自动化」：PowerShell  cmdlet 管道 / WMI/CIM 查询 / Windows 事件日志 / 注册表操作 / 服务与进程管理 / 组策略与权限 / Windows 远程管理 WinRM / 计划任务 / 性能计数器 / Windows 更新管理
>
> 与现有维度互补说明：
> - `03-电脑使用与系统管理`（C 维）聚焦**跨平台通用系统管理**；本文件聚焦 **Windows 特有**的 PowerShell/WMI/注册表/组策略等。
> - `117-跨平台Shell脚本移植与健壮性工程`（DI 维）聚焦 **bash 跨平台移植**；本文件聚焦 **PowerShell 原生运维**，是 Windows 侧的对应物。
> - `118-Linux服务器治理进阶systemd服务化与批量运维`（DJ 维）聚焦 **Linux systemd**；本文件聚焦 **Windows 服务+计划任务**，是 Windows 侧的对应物。
> - `66-跨平台电脑使用与虚拟化实战`（BO 维）聚焦**平台使用体验**；本文件聚焦**Windows 运维自动化工程**。
>
> **本文件独特主题**：PowerShell 管道与对象模型 / WMI/CIM 查询 / Windows 事件日志过滤 / 注册表 CRUD / 服务与进程管理 / NTFS 权限 ACL / WinRM 远程管理 / 计划任务编排 / 性能计数器 / Windows 更新 API。

---

### DQ01 PowerShell 管道与对象处理

- **预期档位**: medium
- **考察维度**: PowerShell 对象管道 / cmdlet 组合 / 格式化输出
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一份 PowerShell 脚本 `tmpPlan/agent-test/dq01_pipeline.ps1`：用 `Get-Process` 获取所有进程，`Where-Object` 筛选 CPU>100 的，`Sort-Object` 按内存降序，`Select-Object` 取前 10 行的 Name/Id/CPU/WS，`Format-Table` 输出；同时用 `Export-Csv` 导出 CSV。
  2. Bash：`pwsh -File tmpPlan/agent-test/dq01_pipeline.ps1 2>&1 | tee tmpPlan/agent-test/dq01_out.log`（若无 pwsh 则用 python3 模拟等效逻辑），断言 `dq01_out.log` 含 "Name" 和 "Id" 列头（grep -F 命中），且 `test -f tmpPlan/agent-test/processes.csv` 文件存在。
  3. Read dq01_pipeline.ps1，再 Write `tmpPlan/agent-test/dq01_report.ps1`：用 `Get-Service` 筛选 Automatic 但当前 Stopped 的服务，输出服务名+启动类型+状态，`ConvertTo-HTML` 生成 HTML 报告。
  4. Bash：`pwsh -File tmpPlan/agent-test/dq01_report.ps1` 后 `grep -c '<tr>' tmpPlan/agent-test/services_report.html` 应 ≥ 1（至少 1 行服务），断言通过则 echo DONE。

### DQ02 WMI/CIM 系统信息查询

- **预期档位**: medium
- **考察维度**: WMI 查询 / 系统硬件信息 / 类层次结构
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write PowerShell 脚本 `tmpPlan/agent-test/dq02_wmi.ps1`：用 `Get-CimInstance Win32_ComputerSystem` 获取总内存/型号/制造商，`Get-CimInstance Win32_Processor` 获取 CPU 核心/频率，`Get-CimInstance Win32_DiskDrive` 获取磁盘大小，输出 JSON 格式。
  2. Bash：`pwsh -File tmpPlan/agent-test/dq02_wmi.ps1 2>&1 | tee tmpPlan/agent-test/dq02_sysinfo.json`，断言 JSON 含 "TotalPhysicalMemory" 和 "NumberOfCores"（grep -F 命中）。
  3. Read dq02_wmi.ps1，再 Write `tmpPlan/agent-test/dq02_bios.ps1`：用 `Get-CimInstance Win32_BIOS` 获取 BIOS 版本/序列号/发布日期，`Get-CimInstance Win32_OperatingSystem` 获取 OS 版本/安装日期/最后启动时间。
  4. Bash：`pwsh -File tmpPlan/agent-test/dq02_bios.ps1` 后 `grep -cE 'BIOSVersion|OSVersion' tmpPlan/agent-test/dq02_bios_out.txt` 应 ≥ 2，断言通过。

### DQ03 Windows 事件日志分析

- **预期档位**: medium
- **考察维度**: 事件日志过滤 / 安全审计 / 时间范围查询
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
 1. Write PowerShell 脚本 `tmpPlan/agent-test/dq03_eventlog.ps1`：用 `Get-WinEvent -LogName Security` 获取最近 24 小时事件，`Where-Object` 筛选 EventID=4624（登录成功），`Group-Object` 按用户分组统计登录次数，输出 Top 10 用户。
  2. Bash：`pwsh -File tmpPlan/agent-test/dq03_eventlog.ps1 2>&1 | tee tmpPlan/agent-test/dq03_logins.txt`，断言输出含 "LogonType" 或 "TargetUserName"（grep -F 命中）。
  3. Read dq03_eventlog.ps1，再 Write `tmpPlan/agent-test/dq03_error.ps1`：用 `Get-WinEvent -LogName System` 筛选 Level=2（错误）的事件，按 ProviderName 分组统计，导出 CSV。
  4. Bash：`pwsh -File tmpPlan/agent-test/dq03_error.ps1` 后 `test -s tmpPlan/agent-test/sys_errors.csv` 断言文件非空，`head -1 tmpPlan/agent-test/sys_errors.csv` 含 "ProviderName" 列头。

### DQ04 注册表操作与系统配置

- **预期档位**: medium
- **考察维度**: 注册表 CRUD / 系统配置 / 环境变量
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write PowerShell 脚本 `tmpPlan/agent-test/dq04_reg.ps1`：用 `Get-ItemProperty` 读取 `HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion` 的 ProductName/CurrentBuild/InstallDate，用 `Set-ItemProperty`（需管理员则跳过）演示写入测试键，输出键值对。
  2. Bash：`pwsh -File tmpPlan/agent-test/dq04_reg.ps1 2>&1 | tee tmpPlan/agent-test/dq04_reg.txt`，断言含 "ProductName" 和 "CurrentBuild"（grep -F 命中）。
  3. Read dq04_reg.ps1，再 Write `tmpPlan/agent-test/dq04_env.ps1`：用 `[Environment]::GetEnvironmentVariables()` 列出所有环境变量，筛选 PATH 相关的，用 `Split(';')` 展开 PATH 数组逐行输出。
  4. Bash：`pwsh -File tmpPlan/agent-test/dq04_env.ps1` 后 `grep -c ':' tmpPlan/agent-test/dq04_path.txt` 应 ≥ 3（PATH 至少 3 个条目），断言通过。

### DQ05 服务与进程管理自动化

- **预期档位**: medium
- **考察维度**: 服务管理 / 进程监控 / 自动化启停
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write PowerShell 脚本 `tmpPlan/agent-test/dq05_service.ps1`：用 `Get-Service` 列出所有 Running 服务，`Select-Object Name/Status/DisplayName`，用 `Stop-Service -WhatIf`（仅演示不真停）输出将要停止的服务列表。
  2. Bash：`pwsh -File tmpPlan/agent-test/dq05_service.ps1 2>&1 | tee tmpPlan/agent-test/dq05_svc.txt`，断言含 "Running" 和 "DisplayName"（grep -F 命中）。
  3. Read dq05_service.ps1，再 Write `tmpPlan/agent-test/dq05_restart.ps1`：用 `Get-Service` 找 Automatic 但 Stopped 的服务，尝试 `Start-Service`，记录成功/失败到日志文件。
  4. Bash：`pwsh -File tmpPlan/agent-test/dq05_restart.ps1` 后 `test -f tmpPlan/agent-test/dq05_restart.log` 断言日志存在，`grep -cE 'Started|Failed|AlreadyRunning' tmpPlan/agent-test/dq05_restart.log` 应 ≥ 1。

### DQ06 NTFS 权限与 ACL 管理

- **预期档位**: medium
- **考察维度**: ACL 读取 / 权限审计 / 访问控制
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write PowerShell 脚本 `tmpPlan/agent-test/dq06_acl.ps1`：用 `Get-Acl` 读取 `C:\Windows\System32` 的 ACL，`Select-Object -ExpandProperty Access` 列出所有 Access Rule，输出 IdentityReference/AccessControlType/FileSystemRights。
  2. Bash：`pwsh -File tmpPlan/agent-test/dq06_acl.ps1 2>&1 | tee tmpPlan/agent-test/dq06_acl.txt`，断言含 "IdentityReference" 和 "Allow"（grep -F 命中）。
  3. Read dq06_acl.ps1，再 Write `tmpPlan/agent-test/dq06_audit.ps1`：用 `Get-ChildItem -Recurse` 遍历指定目录，`Get-Acl` 检查每个文件的 ACL，找出 Everyone 有 FullControl 的文件，输出路径+权限。
  4. Bash：`pwsh -File tmpPlan/agent-test/dq06_audit.ps1` 后 `test -f tmpPlan/agent-test/dq06_audit_result.txt` 断言结果文件存在，`wc -l` 输出行数（可能为 0 表示无风险文件，也合法）。

### DQ07 计划任务编排与管理

- **预期档位**: medium
- **考察维度**: 计划任务 CRUD / 触发器配置 / 任务调度
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write PowerShell 脚本 `tmpPlan/agent-test/dq07_task.ps1`：用 `Get-ScheduledTask` 列出所有计划任务，`Where-Object` 筛选 State=Ready 的，`Select-Object TaskName/State/Description`，输出 JSON。
  2. Bash：`pwsh -File tmpPlan/agent-test/dq07_task.ps1 2>&1 | tee tmpPlan/agent-test/dq07_tasks.json`，断言含 "TaskName" 和 "State"（grep -F 命中）。
  3. Read dq07_task.ps1，再 Write `tmpPlan/agent-test/dq07_create.ps1`：用 `New-ScheduledTaskTrigger` 创建每日触发器，`New-ScheduledTaskAction` 创建执行 action，`Register-ScheduledTask` 注册一个测试任务（名称 `LAEW_Test_Task`），然后 `Unregister-ScheduledTask` 清理。
  4. Bash：`pwsh -File tmpPlan/agent-test/dq07_create.ps1` 后 `grep -F 'LAEW_Test_Task' tmpPlan/agent-test/dq07_create.log` 应含 "Registered" 和 "Unregistered" 两状态。

### DQ08 性能计数器与系统监控

- **预期档位**: medium
- **考察维度**: 性能计数器 / 实时监控 / 阈值告警
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write PowerShell 脚本 `tmpPlan/agent-test/dq08_perf.ps1`：用 `Get-Counter '\Processor(_Total)\% Processor Time'` 采样 CPU 使用率 3 次（间隔 2 秒），`Get-Counter '\Memory\Available MBytes'` 采样可用内存，输出每次采样的 Timestamp/CounterValue。
  2. Bash：`pwsh -File tmpPlan/agent-test/dq08_perf.ps1 2>&1 | tee tmpPlan/agent-test/dq08_perf.txt`，断言含 "Processor" 和 "Available"（grep -F 命中），且采样行数 ≥ 3。
  3. Read dq08_perf.ps1，再 Write `tmpPlan/agent-test/dq08_alert.ps1`：用 `Get-Counter` 持续监控 CPU，当 >80% 时输出告警行 `[ALERT] CPU > 80%`，连续 5 次采样后输出统计摘要。
  4. Bash：`pwsh -File tmpPlan/agent-test/dq08_alert.ps1` 后 `grep -cE '\[ALERT\]|Sample|Summary' tmpPlan/agent-test/dq08_alert.log` 应 ≥ 3。

### DQ09 WinRM 远程管理与批量执行

- **预期档位**: hard
- **考察维度**: 远程管理 / 批量执行 / 凭据安全
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write PowerShell 脚本 `tmpPlan/agent-test/dq09_winrm.ps1`：用 `Test-WSMan` 检测 WinRM 服务状态，`Get-Service WinRM` 检查服务，`Enable-PSRemoting -Force`（演示用 -WhatIf），输出 WinRM 配置摘要。
  2. Bash：`pwsh -File tmpPlan/agent-test/dq09_winrm.ps1 2>&1 | tee tmpPlan/agent-test/dq09_winrm.txt`，断言含 "WinRM" 和 "Service"（grep -F 命中）。
  3. Read dq09_winrm.ps1，再 Write `tmpPlan/agent-test/dq09_batch.ps1`：用 `Invoke-Command -ComputerName localhost -ScriptBlock { Get-Process | Select-Object -First 5 }` 演示远程执行，用 `ForEach-Object` 批量处理多台主机列表（localhost 模拟 3 台），输出每台的结果。
  4. Bash：`pwsh -File tmpPlan/agent-test/dq09_batch.ps1` 后 `grep -c 'localhost' tmpPlan/agent-test/dq09_batch_result.txt` 应 ≥ 3（3 台主机各至少 1 行输出）。

### DQ10 Windows 更新管理与补丁审计

- **预期档位**: medium
- **考察维度**: 更新管理 / 补丁审计 / 合规检查
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write PowerShell 脚本 `tmpPlan/agent-test/dq10_update.ps1`：用 `Get-HotFix` 列出所有已安装补丁，`Sort-Object InstalledOn -Descending` 取最近 10 个，`Select-Object HotFixID/InstalledOn/Description`，输出 Markdown 表格。
  2. Bash：`pwsh -File tmpPlan/agent-test/dq10_update.ps1 2>&1 | tee tmpPlan/agent-test/dq10_patches.md`，断言含 "HotFixID" 和 "|"（grep -F 命中 Markdown 表格）。
  3. Read dq10_update.ps1，再 Write `tmpPlan/agent-test/dq10_audit.ps1`：用 `Get-HotFix` 检查关键补丁（如 KB 编号列表），`Where-Object` 筛选最近 30 天内的补丁，输出补丁数量+最早/最晚安装日期。
  4. Bash：`pwsh -File tmpPlan/agent-test/dq10_audit.ps1` 后 `grep -cE 'KB[0-9]+' tmpPlan/agent-test/dq10_audit.txt` 应 ≥ 1（至少 1 个 KB 编号），断言通过。
