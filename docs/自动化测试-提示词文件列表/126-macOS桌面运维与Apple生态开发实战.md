# 126 macOS 桌面运维与 Apple 生态开发实战

> 编号段 DR01–DR10 · 聚焦「macOS 下的系统运维、Apple 原生开发、自动化配置」：Homebrew 包管理 / launchd 服务 / defaults 系统偏好配置 / AppleScript 自动化 / plist 文件管理 / 磁盘工具 diskutil / 系统扩展与权限 / Xcode 命令行工具 / Swift 脚本编写 / Keychain 密钥管理
>
> 与现有维度互补说明：
> - `03-电脑使用与系统管理`（C 维）聚焦**跨平台通用管理**；本文件聚焦 **macOS 特有**的 Homebrew/launchd/defaults/Keychain 等。
> - `117-跨平台Shell脚本移植与健壮性工程`（DI 维）聚焦**跨平台兼容**；本文件聚焦 **macOS 原生工具链**的深耕。
> - `118-Linux服务器治理进阶systemd服务化与批量运维`（DJ 维）聚焦 **Linux systemd**；本文件聚焦 **macOS launchd**，是 macOS 侧的对应物。
> - `25-桌面应用与跨平台开发实战`（X 维）聚焦**跨平台桌面开发**；本文件聚焦 **macOS 原生运维与开发**。
>
> **本文件独特主题**：Homebrew Formula 与 Cask / launchd plist 与 LaunchAgents / defaults 偏好读写 / AppleScript GUI 自动化 / diskutil 磁盘管理 / Keychain 安全存储 / xcode-select 与 SDK / xcrun 工具链 / notarization 公证 / Gatekeeper 签名验证。

---

### DR01 Homebrew 包管理与环境搭建

- **预期档位**: simple
- **考察维度**: Homebrew 安装/搜索/管理 / Brewfile 批量配置
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一份 `tmpPlan/agent-test/DR01 Brewfile`：含 brew/formula 安装 git/node/python3，cask 安装 visual-studio-code/iterm2，mas 安装 Xcode（App Store 应用）。文件语法遵循 Homebrew Bundle 规范。
  2. Bash：`brew bundle check --file=tmpPlan/agent-test/DR01\ Brewfile 2>&1 | tee tmpPlan/agent-test/dr01_check.log`（若 brew 不可用则模拟），断言 `dr01_check.log` 含 "The Brewfile's dependencies are satisfied" 或缺失列表。
  3. Read DR01 Brewfile，再 Write `tmpPlan/agent-test/dr01_install.sh`：用 `brew install --formula` 安装列表中的 formula，`brew install --cask` 安装 cask，`brew list | wc -l` 统计已安装包数。
  4. Bash：`bash tmpPlan/agent-test/dr01_install.sh 2>&1 | tee tmpPlan/agent-test/dr01_install.log`，断言 `dr01_install.log` 含 "installed" 或 "already installed" 字样。

### DR02 launchd 服务管理

- **预期档位**: medium
- **考察维度**: plist 配置 / launchctl 加载/卸载 / 定时任务
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 一份 launchd plist `tmpPlan/agent-test/com.laew.test.plist`：含 Label/com.laew.test、ProgramArguments（执行 echo hello）、StartInterval 3600（每小时）、RunAtLoad true，标准 out/err 日志路径。
  2. Bash：`plutil -lint tmpPlan/agent-test/com.laew.test.plist 2>&1 | tee tmpPlan/agent-test/dr01_plint.log`，断言含 "OK"（grep -F 命中）。
  3. Read plist，再 Write `tmpPlan/agent-test/dr02_launch.sh`：`launchctl load/unload` 管理服务，`launchctl list | grep laew` 验证状态，输出加载前后的服务列表对比。
  4. Bash：`bash tmpPlan/agent-test/dr02_launch.sh` 后 `grep -c 'laew' tmpPlan/agent-test/dr02_status.log` 应 ≥ 1。

### DR03 defaults 系统偏好读写

- **预期档位**: simple
- **考察维度**: 偏好域/键值类型/全局与域特定设置
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 脚本 `tmpPlan/agent-test/dr03_defaults.sh`：用 `defaults read com.apple.dock` 读取 Dock 配置，`defaults read NSGlobalDomain AppleInterfaceStyle` 检测深色模式，`defaults read com.apple.finder ShowPathbar` 检测 Finder 路径栏状态，输出 JSON 汇总。
  2. Bash：`bash tmpPlan/agent-test/dr03_defaults.sh 2>&1 | tee tmpPlan/agent-test/dr03_prefs.json`，断言含 "Dock" 或 "Finder" 键（grep -F 命中）。
  3. Read dr03_defaults.sh，再 Write `tmpPlan/agent-test/dr03_toggle.sh`：用 `defaults write com.apple.finder AppleShowAllFiles -bool true` 演示配置写入（加 `-WhatIf` 注释说明），`defaults read` 验证写入值。
  4. Bash：`bash tmpPlan/agent-test/dr03_toggle.sh` 后 `grep -c 'true\|false' tmpPlan/agent-test/dr03_toggle.log` 应 ≥ 1。

### DR04 AppleScript GUI 自动化

- **预期档位**: medium
- **考察维度**: GUI 脚本 / 应用交互 / 窗口操作
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write AppleScript `tmpPlan/agent-test/dr04_applescript.scpt`：用 `tell application "System Events"` 获取前台进程名，`tell application "Finder"` 获取桌面路径和窗口数量，输出 JSON 格式的窗口信息。
  2. Bash：`osascript tmpPlan/agent-test/dr04_applescript.scpt 2>&1 | tee tmpPlan/agent-test/dr04_windows.json`，断言含 "Finder" 或 "System Events" 相关输出。
  3. Read dr04_applescript.scpt，再 Write `tmpPlan/agent-test/dr04_screenshot.scpt`：用 `tell application "System Events"` 获取屏幕分辨率，用 `do shell script "screencapture"` 截图保存到指定路径，输出截图文件路径。
  4. Bash：`osascript tmpPlan/agent-test/dr04_screenshot.scpt` 后 `test -f tmpPlan/agent-test/screenshot.png` 断言截图文件存在。

### DR05 plist 文件解析与编辑

- **预期档位**: medium
- **考察维度**: plist XML/binary 格式 / plutil 转换 / 编程式读写
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 脚本 `tmpPlan/agent-test/dr05_plist.sh`：用 `plutil -p /System/Library/CoreServices/SystemVersion.plist` 打印系统版本 plist，`plutil -extract ProductVersion xml1 -o -` 提取特定键的 XML，输出 JSON 格式。
  2. Bash：`bash tmpPlan/agent-test/dr05_plist.sh 2>&1 | tee tmpPlan/agent-test/dr05_sysver.json`，断言含 "ProductVersion" 或 "ProductBuildVersion"（grep -F 命中）。
  3. Read dr05_plist.sh，再 Write `tmpPlan/agent-test/dr05_edit.sh`：用 `plutil -replace` 修改测试 plist 的键值，`plutil -p` 验证修改结果，`plutil -convert binary1` 转二进制格式。
  4. Bash：`bash tmpPlan/agent-test/dr05_edit.sh` 后 `grep -c 'ModifiedKey\|originalValue\|newValue' tmpPlan/agent-test/dr05_edit.log` 应 ≥ 2。

### DR06 磁盘工具 diskutil 与存储管理

- **预期档位**: medium
- **考察维度**: 磁盘列表/分区/挂载/卸载/修复
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 脚本 `tmpPlan/agent-test/dr06_disk.sh`：用 `diskutil list` 列出所有磁盘，`diskutil info disk0` 获取主磁盘信息（大小/协议/SMART 状态），`df -h` 获取挂载点使用率，输出 JSON 汇总。
  2. Bash：`bash tmpPlan/agent-test/dr06_disk.sh 2>&1 | tee tmpPlan/agent-test/dr06_disk.json`，断言含 "disk0" 或 "Size"（grep -F 命中）。
  3. Read dr06_disk.sh，再 Write `tmpPlan/agent-test/dr06_mount.sh`：用 `hdiutil attach` 挂载测试 DMG（若无则创建空 DMG），`diskutil mount/unmount` 演示挂载卸载，`diskutil eject` 弹出。
  4. Bash：`bash tmpPlan/agent-test/dr06_mount.sh` 后 `grep -cE 'attached|mounted|ejected' tmpPlan/agent-test/dr06_mount.log` 应 ≥ 2。

### DR07 Keychain 密钥管理

- **预期档位**: medium
- **考察维度**: 安全存储/密码读写/代码签名证书
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 脚本 `tmpPlan/agent-test/dr07_keychain.sh`：用 `security find-generic-password -s "test_service" -g` 读取密码（预期失败但演示流程），`security list-keychains` 列出所有钥匙串，`security find-identity -v` 列出代码签名身份。
  2. Bash：`bash tmpPlan/agent-test/dr07_keychain.sh 2>&1 | tee tmpPlan/agent-test/dr07_keychain.log`，断言含 "keychain" 或 "identities"（grep -F 命中）。
  3. Read dr07_keychain.sh，再 Write `tmpPlan/agent-test/dr07_store.sh`：用 `security add-generic-password -s "laew_test" -a "test_account" -w "test_password"` 添加测试密码，`security find-generic-password -s "laew_test" -w` 读取密码明文验证，`security delete-generic-password -s "laew_test"` 清理。
  4. Bash：`bash tmpPlan/agent-test/dr07_store.sh` 后 `grep -c 'test_password' tmpPlan/agent-test/dr07_store.log` 应 ≥ 1（密码被成功读取）。

### DR08 Xcode 命令行工具与 SDK 管理

- **预期档位**: medium
- **考察维度**: xcode-select/SDK 路径/编译工具链
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 脚本 `tmpPlan/agent-test/dr08_xcode.sh`：用 `xcode-select -p` 获取开发者目录，`xcrun --show-sdk-path` 获取 SDK 路径，`xcrun --sdk macosx --show-sdk-version` 获取 SDK 版本，`swift --version` 获取 Swift 版本，输出 JSON。
  2. Bash：`bash tmpPlan/agent-test/dr08_xcode.sh 2>&1 | tee tmpPlan/agent-test/dr08_xcode.json`，断言含 "swift" 或 "sdk"（grep -iF 命中）。
  3. Read dr08_xcode.sh，再 Write `tmpPlan/agent-test/dr08_build.sh`：用 `swiftc -o hello hello.swift` 编译一个简单的 Swift 程序（print "Hello from laew"），`./hello` 运行验证输出。
  4. Bash：`bash tmpPlan/agent-test/dr08_build.sh` 后 `grep -c 'Hello from laew' tmpPlan/agent-test/dr08_build.log` 应 ≥ 1。

### DR09 系统扩展权限与隐私保护

- **预期档位**: medium
- **考察维度**: TCC 权限/完全磁盘访问/屏幕录制/辅助功能
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 脚本 `tmpPlan/agent-test/dr09_tcc.sh`：用 `sqlite3 ~/Library/Application\ Support/com.apple.TCC/TCC.db "SELECT client, auth_value FROM access WHERE service='kTCCServiceSystemPolicyAllFiles'"` 查询完全磁盘访问权限列表（若无权限则演示替代方案），`tccutil list` 列出所有 TCC 配置。
  2. Bash：`bash tmpPlan/agent-test/dr09_tcc.sh 2>&1 | tee tmpPlan/agent-test/dr09_tcc.log`，断言含 "TCC" 或 "auth" 相关输出。
  3. Read dr09_tcc.sh，再 Write `tmpPlan/agent-test/dr09_perm.sh`：用 `ls -laOe` 检查文件的 flags 和 ACL，`csrutil status` 检查 SIP 状态，`spctl --status` 检查 Gatekeeper 状态，输出安全配置摘要。
  4. Bash：`bash tmpPlan/agent-test/dr09_perm.sh` 后 `grep -cE 'SIP|Gatekeeper|enabled|disabled' tmpPlan/agent-test/dr09_perm.log` 应 ≥ 2。

### DR10 macOS 系统信息综合审计

- **预期档位**: hard
- **考察维度**: 系统报告/硬件信息/软件清单/安全状态
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write 脚本 `tmpPlan/agent-test/dr10_audit.sh`：用 `system_profiler SPHardwareDataType` 获取硬件概览，`system_profiler SPSoftwareDataType` 获取软件概览，`system_profiler SPApplicationsDataType` 获取已安装应用列表（截取前 20 行）。
  2. Bash：`bash tmpPlan/agent-test/dr10_audit.sh 2>&1 | tee tmpPlan/agent-test/dr10_full.txt`，断含 "Model" 和 "Version"（grep -F 命中）。
  3. Read dr10_audit.sh，再 Write `tmpPlan/agent-test/dr10_report.py`：用 python3 解析 system_profiler 的 XML 输出（`system_profiler -xml`），提取硬件型号/内存/磁盘/OS 版本/安全补丁状态，生成 Markdown 审计报告。
  4. Bash：`python3 tmpPlan/agent-test/dr10_report.py` 后 `grep -c '^## ' tmpPlan/agent-test/dr10_report.md` 应 ≥ 4（至少 4 个二级标题：硬件/软件/安全/应用）。
