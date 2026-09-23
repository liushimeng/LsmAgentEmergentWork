---
name: test-runner
description: 检测项目类型并跑对应测试套件(优先增量、收口诊断)
version: "1.0"
license: MIT
compatibility: "laew>=1.0"
allowed-tools: Bash, Read
user-invocable: true
tags: [test, dev, ci]
metadata:
  emoji: "🧪"
  category: dev
  os: [darwin, linux, windows]
---

# test-runner —— 项目测试运行与诊断

你被要求跑当前项目的测试套件。按以下步骤执行。

## 步骤

1. **检测项目类型**(并行只读命令):
   - `ls package.json Cargo.toml pyproject.toml go.mod pom.xml build.gradle Makefile CMakeLists.txt 2>/dev/null`
   - 从命中文件读出测试命令约定(优先级):
     - Node / TS:`package.json` 的 `scripts.test`
     - Rust:`Cargo.toml` 旁的 `cargo test`(默认)
     - Python:`pyproject.toml` 的 `[tool.pytest.ini_options]` 或 `tox.ini`
     - Go:`go test ./...`
     - Makefile:`make test`

2. **默认优先增量测试**(用 `$1` 携带目标):
   - 全跑:`$ARGUMENTS` 为空时跑全量
   - 指定文件:`$1` 是文件名(如 `mod.rs`)→ 映射到对应测试目标
   - 指定模式:`$1` 是 grep pattern → 跑 `--filter` / `--grep` / `-k`
   - 指定子集:`tests::path::to::test` 完整路径

3. **执行**(单次 Bash 命令,带 timeout):
   - 输出截断阈值 50KB(超长测试输出尾部截断)
   - timeout 5 分钟(常规项目足够,CI 项目可拉长)
   - 环境变量:`RUST_BACKTRACE=1` / `FORCE_COLOR=1`(若支持)

4. **结果分析**:
   - 退出码 0 → 全过,统计耗时与用例数
   - 退出码非 0 → 提取失败用例(grep `FAILED`、`panic`、`AssertionError`)
   - 给出失败聚合(去重,避免 100 次同一 panic 复读)

5. **输出**:

```
# Test Report

## 概览
- 项目类型: <node/rust/python/...>
- 命令: <实际跑的 command>
- 退出码: <code>
- 耗时: <Xs>
- 用例: <passed>/<total>(含 skipped)

## 失败(若存在)
### 1. <test name>
- 位置: `path/to/test.rs:LINE`
- 失败原因: <错误摘要>
- 相关代码片段: <3-5 行上下文>

## 建议
- 重新跑:`<reproduce command>`
- 调试:`RUST_BACKTRACE=full <original command>`
- 标记 flaky: <候选用例>
```

## 边界

- **不**改任何代码(就算「显然」是测试 bug 也只回报)。
- **不**装依赖(用户单独决策)。
- **不**跳过测试(`--no-fail-fast` 等 flag 默认不启用,除非 `$ARGUMENTS` 显式带)。
- 不修改 `Cargo.lock` / `package-lock.json` / `poetry.lock`。
- 默认 timeout 5 分钟超时就报告,不无限重试。

## 常见项目命令

| 项目 | 检测标记 | 默认命令 |
|------|---------|---------|
| Rust | `Cargo.toml` | `cargo test --no-fail-fast` |
| Node | `package.json` 含 `scripts.test` | `npm test` |
| Python pytest | `pyproject.toml` 含 pytest | `pytest -x --tb=short` |
| Python tox | `tox.ini` | `tox` |
| Go | `go.mod` | `go test ./...` |
| Makefile | 顶层 `Makefile` 含 `test:` | `make test` |
| CMake | `CMakeLists.txt` | `cmake --build build && ctest --test-dir build` |