# 专题-第十六轮-atomcode-深度分析

> **调研日期**：2026-09-09
> **调研范围**：atomcode daemon 基础设施（ActiveChatRegistry、空闲看门狗、LiveViewHub、OAuth 状态机、权限桥接）
> **本轮新增 gap**：L1118-L1125+

---

## 一、ActiveChatRegistry 单飞准入（lib.rs ~200 行）

```rust
struct ActiveChatOperation {
    session_id: Option<String>,
    aliases: Vec<String>,
    cancellation: CancellationToken,
    stopped: bool,
}

// admit() 在单次 write lock 下：
// 1. 检查 session_id / request_id 是否已存在 alias
// 2. 生成 uuid operation_id
// 3. 创建新的 CancellationToken
// 4. 同时插入 operations 和 aliases 表
```

**取消传播**：`stop_alias(alias)` → 标记 stopped=true → `cancellation.cancel()`

**laew gap L1118**：无单飞准入机制，同 session 双请求会互相覆盖。

---

## 二、空闲超时看门狗（lib.rs ~40 行）

```rust
fn spawn_idle_timeout_task(
    idle_timeout_secs: u64,
    last_activity: Arc<AtomicI64>,
    active_connections: Arc<AtomicUsize>,
    active_chats: ActiveChatRegistry,
    shutdown_tx: watch::Sender<bool>,
)
// 每 60s tick：
// - active_connections > 0 → 跳过
// - active_chats.has_active_operations() → 跳过
// - elapsed >= timeout_ms → shutdown_tx.send(true)
```

**laew gap L1119**：无空闲超时看门狗。

---

## 三、LiveViewHub 实时同步（live_hub.rs 2487 行）

```rust
pub struct LiveViewHub {
    state: Mutex<HubState>,
    events: broadcast::Sender<LiveObservation>,  // 容量 1024
}

// publish_view_locked()：
// - 在 state 锁内递增 cursor
// - 构造 LiveObservation
// - 推入 replay 缓冲（若 replay=true）
// - events.send(observation)
```

**generation 检测**：runtime 重启/重建后的 stale binding 检测。

**laew gap L1048 [P0]**：无实时事件多播机制。

---

## 四、OAuth 登录状态机（login_state.rs 254 行）

```
[Pending] --begin_poll--> [Polling {generation}]
 [Polling] --Pending--> [Pending]
 [Polling] --AuthorizationReady--> [Persisting]
 [Persisting] --begin_poll--> [Committing {generation}]
 [Committing] --Authorized--> [Authorized] (terminal)
 [Pending/Polling/Persisting] --cancel--> [Cancelled] (terminal)
 [any] --Failed--> [Failed] (terminal)
 [terminal] --60s--> removable
```

**关键常量**：LOGIN_TTL=600s，TERMINAL_RETENTION=60s，LOGIN_RETRY_AFTER_MS=2000

**laew gap L1120**：无 OAuth 登录状态机。

---

## 五、权限桥接（permission_bridge.rs 128 行）

```rust
pub struct PermissionResponders {
    inner: Arc<RwLock<HashMap<String, UnboundedSender<PermissionDecision>>>>,
}
// register(session_id, tx) — /chat 启动时注册
// deliver(session_id, decision) — 把决策送给对应 session
// unregister(session_id) — /chat 结束时清理
```

**laew gap L1121**：无权限桥接机制。

---

## 六、WebUI Token 鉴权（auth_token.rs 305 行）

```rust
// Cookie 端口隔离
fn webui_cookie_name(port: u16) -> String {
    format!("atomcode_webui_{}", port)
}
// 解决多实例共享 localhost cookie jar 问题
```

**laew gap L1122**：无 Cookie 端口隔离。

---

## 七、新增 gap 清单

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1048 | 实时同步 | 无 LiveViewHub 实时事件多播 | P0 |
| L1118 | 单飞准入 | 无 ActiveChatRegistry 单飞准入与取消传播 | P1 |
| L1119 | 看门狗 | 无空闲超时看门狗 | P1 |
| L1120 | OAuth | 无 OAuth 登录状态机 | P1 |
| L1121 | 权限 | 无权限桥接机制 | P1 |
| L1122 | 鉴权 | 无 Cookie 端口隔离 | P2 |

---

**报告完成日期**：2026-09-09
**分析基础**：atomcode crates/atomcode-daemon/src/lib.rs + live_hub.rs + login_state.rs + permission_bridge.rs + auth_token.rs 逐行分析
