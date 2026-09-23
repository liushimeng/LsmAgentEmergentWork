---
name: git-commit
description: 按 Conventional Commits 规范起草 git commit message(中文)
version: "1.0"
license: MIT
compatibility: "laew>=1.0"
allowed-tools: Bash
user-invocable: true
tags: [git, dev, productivity]
metadata:
  emoji: "📝"
  category: dev
  os: [darwin, linux, windows]
---

# git-commit —— Conventional Commits 规范(中文版)

你被要求为当前 git 仓库起草一条规范的 commit message。
按照以下步骤执行,**绝不直接修改 / 落盘**;只输出 message 文本给用户审阅。

## 步骤

1. **收集上下文**(用 Bash 一组并行只读命令):
   - `git status --short` —— 改动文件清单
   - `git diff --stat HEAD` —— 改动规模概览
   - `git log --oneline -5` —— 最近 5 条 commit 保持风格连贯
   - `git diff HEAD -- <changed-files>` —— 详细 diff(若文件多可省略)

2. **判定 type**(从 diff 与 log 推断,7 选 1):
   - `feat` —— 新功能(用户可见行为变化)
   - `fix` —— 修复 bug
   - `docs` —— 仅文档变更(代码不动)
   - `style` —— 格式调整(空格 / 格式化,无逻辑变化)
   - `refactor` —— 重构(既不是 feat 也不是 fix)
   - `test` —— 新增 / 调整测试
   - `chore` —— 杂项(依赖、构建、CI)

3. **判定 scope**(可选,影响范围括号括起来):
   - 例:`feat(auth):`、`fix(api):`、`docs(readme):`
   - 单文件小改可省略 scope

4. **写 subject**(英文 ≤ 50 字符,祈使句,首字母小写,**无句号**):
   - ❌ `feat(auth): Added login feature.`
   - ✅ `feat(auth): add OAuth2 login flow`

5. **写 body**(可选,中文 / 英文皆可,每行 ≤ 72 字符):
   - **What**:做了什么(简要)
   - **Why**:为什么做(动机 / 关联 issue)
   - **Breaking change**:若有,在 body 末尾加 `BREAKING CHANGE: <说明>`

6. **写 footer**(可选):
   - `Refs: #123`、`Closes: #456`、`Co-authored-by: Name <email>`

## 输出格式

把最终 commit message 整段输出(包含空行分隔 header/body/footer),
**绝不调 git commit**,**绝不 git add**。
让用户手动 `git commit -F <file>` 或复制粘贴到 commit 编辑器。

```
<type>(<scope>): <subject>

<body line 1>
<body line 2>

<footer line 1>
```

## 边界

- **不要**替用户跑 `git commit`(尊重用户最终确认权)。
- **不要**改写现有代码 / 文件。
- 若 diff 完全为空,提示「无 staged changes,先 git add」。
- 若 commit message 超过 72 字符/行,自动折行(保留 markdown 风格)。