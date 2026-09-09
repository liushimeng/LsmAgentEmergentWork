# 专题-第十六轮-claude-code-深度分析

> **调研日期**：2026-09-09
> **调研范围**：claudecode 多轮对话恢复机制、压缩管线、记忆系统、流式工具执行
> **本轮新增 gap**：L1036-L1065（20 个）

---

## 一、七阶段上下文管线（query.ts 1729 行）

```typescript
while (true) {
  // 阶段 1: applyToolResultBudget（工具结果预算裁剪）
  // 阶段 2: snipCompactIfNeeded（历史裁剪）
  // 阶段 3: microcompact（微压缩 - 工具结果清理）
  // 阶段 4: applyCollapsesIfNeeded（上下文折叠 - 90%/95% 水位）
  // 阶段 5: autocompact（自动压缩 - LLM 摘要）
  // 阶段 6: callModel（API 调用 + 流式处理）
  // 阶段 7: runTools（工具执行）
}
```

**8 种 transition reason**：collapse_drain_retry / reactive_compact_retry / max_output_tokens_escalate / max_output_tokens_recovery / stop_hook_blocking / token_budget_continuation / model_fallback_triggered / completed

---

## 二、三级恢复机制 ×2

### 2.1 max_output_tokens 三级恢复（L1186-1252）
1. 8k → 64k 升级（单次，静默）
2. 注入 "Resume directly" 消息（最多 3 次）
3. 暴露错误

### 2.2 prompt-too-long 三级恢复（L1062-1183）
1. 上下文折叠排水（drain staged collapses）
2. 响应式压缩（reactive compact）
3. 暴露错误

---

## 三、cached microcompact（microCompact.ts 530 行）

- 不修改本地消息内容——cache_reference 和 cache_edits 在 API 层添加
- 使用 `cache_edits` API 直接删除工具结果，不破坏 prompt cache
- 只对 COMPACTABLE_TOOLS 生效：FileRead/Bash/Grep/Glob/WebSearch/WebFetch/FileEdit/FileWrite
- 时间触发器：距离上次助手消息超过阈值则跳过 cached MC

---

## 四、后台记忆提取（extractMemories.ts 600+ 行）

- **闭包状态模式**：所有可变状态封装在 `initExtractMemories()` 闭包内
- **互斥机制**：主 agent 已写入记忆文件则跳过
- **尾随运行**：提取过程中有新调用到达，暂存上下文，完成后立即执行尾随提取
- **权限沙箱**：只能读取全局文件，只能写入 memory 目录

---

## 五、流式工具执行器（query.ts 561-862）

- 模型仍在流式输出时，StreamingToolExecutor 可并行执行已完成工具
- `getCompletedResults()` 返回已完成结果，`getRemainingResults()` 返回所有剩余结果

---

## 六、模型回退完整清理（query.ts 893-953）

1. 为孤立 tool_use 块生成错误结果
2. 丢弃失败尝试的待处理结果
3. 更新 tool use context 为新模型
4. 剥离 thinking signatures（模型绑定）
5. 记录回退事件
6. 通知用户

---

## 七、新增 gap 清单

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1036 | 多轮对话 | 无七阶段上下文管线 | P0 |
| L1037 | 多轮对话 | 无 max_output_tokens 三级恢复 | P0 |
| L1038 | 多轮对话 | 无 prompt-too-long 三级恢复 | P0 |
| L1039 | 压缩系统 | 无 cached microcompact | P0 |
| L1040 | 记忆系统 | 无后台记忆提取 agent | P0 |
| L1051 | 权限 | 无 PermissionContext 工厂 | P1 |
| L1052 | 权限 | 无 Bash 分类器推测执行 | P1 |
| L1053 | 工具 | 无 interruptBehavior 声明 | P1 |
| L1054 | 工具 | 无 contextModifier 机制 | P1 |
| L1055 | 工具 | 无流式工具执行器 | P1 |
| L1056 | 压缩 | 无 compact 请求 PTL 重试 | P1 |
| L1057 | 压缩 | 无图片/附件剥离 | P1 |
| L1058 | 记忆 | 无 SessionMemory 双阈值触发 | P1 |
| L1059 | 记忆 | 无团队记忆同步 | P1 |
| L1060 | MCP | 无 MCP session 过期检测 | P1 |
| L1061 | Skill | 无 MCP Skill 发现 | P2 |
| L1062 | 测试 | 无 TestingPermissionTool | P2 |
| L1063 | 多轮对话 | 无模型回退完整清理 | P2 |
| L1064 | 工具 | 无工具结果预算持久化 | P2 |
| L1065 | 记忆 | 无 extractMemories 闭包状态 | P2 |

---

**报告完成日期**：2026-09-09
**分析基础**：claudecode src/query.ts / Tool.ts / microCompact.ts / compact.ts / extractMemories.ts / sessionMemory.ts / teamMemorySync/ 逐行分析
