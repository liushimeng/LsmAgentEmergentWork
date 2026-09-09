# 专题-第十六轮-deepseek-harness-深度分析

> **调研日期**：2026-09-09
> **调研范围**：Cordis 反应式 IoC 内核、Goal 严格事件源化、Workflow Worker 线程、Ralph 固定脚本、SubAgent 11 包矩阵
> **本轮新增 gap**：L1048 / L1095-L1110+

---

## 一、Cordis Epoch 算法（vendor/cordis/src/fiber.ts 755 行）

```typescript
_refresh() {
  let epoch = ''
  for (const name of Object.keys(this.inject)) {
    const impl = this._store[name]
    if (!impl) { epoch = INACTIVE; break }   // 任一必需服务缺失 → 停用
    epoch += ':' + impl.friver.uid            // 拼接依赖 fiber 的 uid
  }
  this._setEpoch(epoch)
}
```

| Epoch 值 | 含义 |
|---------|------|
| `INACTIVE = '__INACTIVE__'` | 必需服务缺失 |
| `''`（空串） | 无依赖的根/独立插件 |
| `':3:7:42'` | 依赖 fiber uid 为 3、7、42 的服务全部到位 |

**惰性切换**：`inertia` 字段把并发多次 epoch 变更折叠成一次过渡。`_reload` 开头 `await Promise.resolve()` 让位，再检查 `this._runner.epoch === oldEpoch`，若已被覆盖就放弃本次加载。

**与传统 IoC 差异**：NestJS/InversifyJS 预编译模块图+启动排序；Cordis 用反应式 epoch——依赖变化时通过 `notify()` 级联刷新。

**laew gap L1048 [P0]**：无反应式依赖图机制。

---

## 二、ReflectService Proxy 三段式（reflect.ts 419 行）

```typescript
get: (target, prop, ctx) => {
  if (isSpecialProperty(prop)) return Reflect.get(target, prop, ctx)
  if (Reflect.has(target, prop)) return getTraceable(ctx, ...)
  // 隔离感知的向上回溯
  while (true) {
    const impl = fiber.store?.[prop]
    if (impl) return getTraceable(ctx, impl.value)
    if (prop in fiber.inject) { throw error }
    if (fiber.parent[symbols.isolate][prop] !== key) throw error
    fiber = fiber.parent.fiber
  }
}
```

---

## 三、EventsService 5 种调度模式（events.ts 353 行）

| 模式 | 语义 | 返回值 |
|------|------|--------|
| `emit` | 同步触发，不等待 Promise | `void` |
| `parallel` | `Promise.allSettled` 并发 | `AggregateError` 聚合 |
| `serial` | 顺序 await，遇 bail 值停止 | 首个 bail 值 |
| `bail` | 同步顺序，遇 bail 值停止 | 首个 bail 值 |
| `waterfall` | 外层包裹内层，`next()` 延续 | 最外层返回值 |

**waterfall 洋葱模型**：监听器不调用 `next()` 就 veto 后续所有层。

---

## 四、Goal 严格重放验证（fold.ts 350 行）

| 操作 | 源状态 | 目标状态 | 约束 |
|------|--------|---------|------|
| `create` | (无) | `active` | revision=1 |
| `pause` | `active` | `paused` | — |
| `resume` | `active`/`paused`/`blocked` | `active` | `roundsStarted < maxGoalRounds` |
| `complete` | `active`/`paused`/`blocked` | `complete` | — |
| `block` | `active` | `blocked` | 需要 blockedReason |

**严格验证**：revision 单调递增，时间戳保序，计数器保留。

---

## 五、Workflow Worker 线程协议（protocol.ts 102 行）

**13 种消息类型**（两端各 7 种），纯 JSON，通过 MessagePort 传输：

| 方向 | 标签 |
|------|------|
| Worker→Host | `Ready` / `Phase` / `Log` / `AgentStart` / `AgentEnd` / `ChildStart` / `ChildDispose` / `Result` |
| Host→Worker | `Go` / `Cancel` / `ChildStarted` / `ChildStartError` / `ChildSettled` / `ChildFailed` / `ChildDisposed` |

**vm.Script 编译**：`lineOffset` 补偿包装行号，使 stack trace 指向脚本自身行号。

**取消纪律**：取消后每个 hook 调用都抛 CANCELLED——取消是下一个 hook 边界。

---

## 六、Ralph 固定脚本（tool-ralph/src/index.ts 479 行）

```javascript
// 部署拥有的固定脚本，模型只供应数据
for (let round = 1; round <= args.maxRounds; round += 1) {
  const rawReport = await agent(prompt, { label: 'Ralph round ' + round, schema: reportSchema })
  if (report.status === 'complete') return { status: 'complete', roundsStarted: round, report }
  if (report.status === 'blocked') return { status: 'blocked', roundsStarted: round, report }
  previous = report
}
```

| 维度 | 设计 |
|------|------|
| Fresh Agent | 每轮全新子 agent，requireFreshProvider 强制 inheritsParentContext=false |
| 结构化传递 | 轮次间只传递有界报告（maxHandoffChars=16384） |
| 不可变目标 | 每轮都传递原始 objective |

---

## 七、SubAgent 延续管理器（continuation.ts ~1483 行）

- **激活图**：childId/parentSession/handle/ancestry/ownedChildren/observer/disposal
- **冷恢复**（coldResume）：从持久化 Session 恢复驻留 Agent
- **子优先释放**（finishDisposal）：取消→等待空闲→释放子代理的子代理→释放自身
- **所有权图**：acquireOwnership/releaseOwnership 防止循环委托

---

## 八、新增 gap 清单

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1048 | 反应式 IoC | 无 Cordis Epoch 算法（依赖图拓扑指纹） | P0 |
| L1095 | 事件 | 无 5 种调度模式（waterfall 洋葱模型） | P1 |
| L1096 | 状态机 | 无 Goal 严格重放验证（revision CAS） | P1 |
| L1097 | Worker | 无 Worker 线程协议（13 种消息类型） | P1 |
| L1098 | 脚本 | 无 Ralph 固定脚本（fresh-agent 循环） | P1 |
| L1099 | SubAgent | 无延续管理器（冷恢复 + 子优先释放） | P1 |
| L1100 | 取消 | 无 Hook 边界取消纪律 | P2 |

---

**报告完成日期**：2026-09-09
**分析基础**：deepseek-harness vendor/cordis/ + packages/goal/ + packages/workflow/ + packages/subagent/ 逐行分析
