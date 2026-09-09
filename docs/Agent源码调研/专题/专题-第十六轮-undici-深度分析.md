# 专题-第十六轮-undici-深度分析

> **调研日期**：2026-09-09
> **调研范围**：undici HTTP 缓存后端、SSE 解析、multipart、WebSocket 性能、AbortSignal、TestAgent、FixedQueue、代理错误链
> **本轮新增 gap**：L1111-L1120（10 个）

---

## 一、SQLite 持久化缓存（lib/cache/sqlite-cache-store.js 469 行）

```sql
CREATE TABLE cache(method TEXT, url TEXT, headers BLOB, body BLOB, status INTEGER, ...)
INSERT OR REPLACE ... 
SELECT ... WHERE method=? AND url=?
```

**laew gap L1111**：无 SQLite 持久化缓存实现（跨进程缓存）。建议 `rusqlite` + `PRAGMA wal`。

---

## 二、SSE 解析状态机（lib/web/eventsource/eventsource-stream.js ~300 行）

- `event:` / `data:` / `id:` / `retry:` 四字段解析
- UTF-8 多字节字符跨 chunk 处理
- last-event-id 自动重连

**laew gap L1112**：无标准 SSE 解析状态机。

---

## 三、流式 multipart 解析（lib/web/fetch/formdata-parser.js 586 行）

- 边读边解析，不全量缓冲（避免 OOM）
- boundary 的转义处理（value 中包含 boundary 字符串）
- Content-Type 嵌套（子 multipart/mixed）

**laew gap L1113**：无流式 multipart 解析（大文件上传）。

---

## 四、WebSocket cork/uncork（lib/web/websocket/sender.js ~50 行）

```javascript
socket.cork()
write(head)
write(body)
socket.uncork()  // 合并 TCP 包
```

**laew gap L1114**：无 cork/uncork 批量发送。

---

## 五、AbortSignal 链（node:abort-controller）

- `AbortSignal.any(signals)` — 多个 signal 任一触发即取消
- `AbortSignal.timeout(ms)` — 自动超时取消
- abort 事件传播顺序：父 → 子

**laew gap L1115**：无 any() 组合 + timeout() 工厂。

---

## 六、FixedQueue 无锁队列（lib/dispatcher/fixed-queue.js ~100 行）

- 2048 桶循环缓冲 + 链表
- Client[kRunningIdx/kPendingIdx/kQueue] 三指针调度

**laew gap L1116**：无 FixedQueue 无锁环形队列实现。

---

## 七、代理错误链包装（lib/dispatcher/proxy-agent.js）

| 错误类型 | 语义 | 可重试 |
|---------|------|--------|
| `SecureProxyConnectionError` | 代理证书不匹配 | 否 |
| `ProxyConnectionError` | 隧道中途断开 | 是 |

**laew gap L1117**：无 SecureProxyConnectionError vs ProxyConnectionError 区分。

---

## 八、新增 gap 清单

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1111 | HTTP 缓存 | 无 SQLite 持久化缓存实现 | P1 |
| L1112 | SSE | 无标准 SSE 解析状态机 | P1 |
| L1113 | multipart | 无流式 multipart 解析 | P1 |
| L1114 | WebSocket | 无 cork/uncork 批量发送 | P2 |
| L1115 | AbortSignal | 无 any() 组合 + timeout() 工厂 | P1 |
| L1116 | 队列 | 无 FixedQueue 无锁环形队列 | P2 |
| L1117 | 代理 | 无 SecureProxyConnectionError 区分 | P1 |

---

**报告完成日期**：2026-09-09
**分析基础**：undici lib/cache/ + lib/web/eventsource/ + lib/web/fetch/ + lib/web/websocket/ + lib/dispatcher/ 逐行分析
