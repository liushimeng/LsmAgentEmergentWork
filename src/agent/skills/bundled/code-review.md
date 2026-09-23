---
name: code-review
description: 对当前 git diff 做静态代码评审(correctness / 安全 / 可读性 / 性能)
version: "1.0"
license: MIT
compatibility: "laew>=1.0"
allowed-tools: Read, Bash, Grep
user-invocable: true
tags: [review, quality, dev]
metadata:
  emoji: "🔎"
  category: dev
  os: [darwin, linux, windows]
---

# code-review —— 静态代码评审清单

你被要求对当前 git 改动做一次系统化代码评审。**只读不改**,输出结构化报告。

## 步骤

1. **收集 diff**(只读命令并行):
   - `git diff --stat HEAD` —— 改动规模
   - `git diff HEAD` —— 完整 diff(若很大,按文件分批 Read)
   - `git log --oneline -10` —— 近期 commit 风格参考
   - 对每个改动文件 `Read` 看完整内容(不是只看 diff)

2. **按维度评审**(每个维度 0~N 条 issue):

   - **Correctness(正确性)**
     - 边界条件(空 / 0 / 负数 / None / 空字符串 / 超大输入)
     - 错误处理路径(Err 路径是否覆盖)
     - 并发安全(共享状态 / 锁 / 异步取消)
     - 类型转换 / 强制解包 / `as` cast

   - **Security(安全)**
     - 用户输入 → 命令 / SQL / shell 注入(本仓有 `agent/safety/prompt_injection.rs`)
     - 凭证硬编码 / 密钥泄露(`api_key` / `password` 字面量)
     - 路径穿越(`..` / 符号链接)
     - 危险 API(rm -rf / chmod 777 / eval / pickle.loads)

   - **Readability(可读性)**
     - 函数 / 变量命名一致性
     - 注释与代码意图是否对齐
     - 死代码 / 未使用 import / 调试打印残留
     - 复杂表达式是否需要拆分

   - **Performance(性能)**
     - O(n²) 循环 / 不必要的 clone / String 重复分配
     - I/O 同步调用塞异步路径
     - 大文件是否需要流式处理
     - 数据库 / 网络请求是否需要批量化

   - **Style(风格)**
     - 项目既有约定(从 git log 推断)
     - 格式化工具(rustfmt / prettier)是否覆盖
     - 测试覆盖(改动是否有对应测试)

3. **优先级分级**(沿用本仓 `agent/quality.rs::gate_report_on_trace`):
   - **Critical**:必须修复,阻塞合入(数据丢失 / 安全漏洞 / 运行器空 panic)
   - **Major**:建议修复,有清晰证据(边界条件遗漏 / 性能热点)
   - **Minor**:可选(命名 / 注释 / 微小重构)

4. **输出报告**(Markdown 结构):

```
# Code Review Report

## Summary
- Files changed: N
- Lines: +A / -B
- Overall: ✅ LGTM / ⚠️ Changes Suggested / 🚫 Changes Required

## Critical Issues
1. `path/to/file.rs:LINE` —— <描述>
   - Evidence: <引用代码片段>
   - Fix: <具体修改建议>

## Major Issues
...

## Minor Issues
...

## Positive Observations
- ...
```

## 边界

- **不**改代码 / 文件 / commit。
- **不**跑测试(用户单独决策)。
- **不**调外部工具(`Bash` 只读、`Read` / `Grep` 检索)。
- 若 diff 极小(< 5 行),快速过一遍即可,不必全维度铺开。