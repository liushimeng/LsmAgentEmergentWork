# 专题-第十六轮-openclaw-深度分析

> **调研日期**：2026-09-09
> **调研范围**：openclaw 内存加密、SQLite 全栈基础设施、Hook 系统、租约引擎
> **本轮新增 gap**：L1041-L1043 / L1061-L1082（51 个）

---

## 一、Secret Sentinel 内存加密（secrets/sentinel.ts 125 行）

```typescript
const SECRET_SENTINEL_CIPHER = "aes-256-gcm";
const SECRET_SENTINEL_NONCE_BYTES = 12;
const SECRET_SENTINEL_TAG_BYTES = 16;
const SECRET_SENTINEL_MAX_BYTES = 64 * 1024;

// 跨模块密钥共享
Symbol.for("openclaw.secretSentinel.keys")
```

- `sealSecretSentinel(plaintext)` → `oc-sent-v2.<base64>`
- `resolveSecretSentinel(sealed)` → 解密还原
- `swapSecretSentinelsInText()` → 文本中批量替换（日志脱敏）

**laew gap L1061**：API Key 在 Agent-Context 中以明文存储。建议 `aes-gcm` + `secrecy` + `zeroize`。

---

## 二、SQLite 基础设施全栈（12 个专用模块）

| 模块 | 文件 | 核心设计 |
|------|------|---------|
| WAL 配置 | `sqlite-wal.ts` | 跨 VM 文件系统检测（NFS magic 0x6969 / SMB 0x517b），回退 rollback journal |
| Busy Timeout | `sqlite-busy-timeout.ts` | 临时设置 busy_timeout，WeakMap 控制锁失败报告 |
| 完整性检测 | `sqlite-integrity.ts` | 区分持久性损坏 vs 瞬态锁冲突 |
| Post-Commit 事务 | `sqlite-post-commit.ts` | 嵌套 SAVEPOINT 正确处理 |
| 跨进程协调器 | `sqlite-coordinator.ts` | `/tmp/openclaw-state-locks-{uid}/{family}.{sha256}.lock.sqlite` |
| 只读快照 | `sqlite-readonly-location.ts` | TOCTOU 保护（固定 fd + fstat），检测 ENOSPC/EDQUOT |
| STRICT Schema | `sqlite-strict.ts` | 严格类型迁移 |
| 索引契约 | `sqlite-index-schema.ts` | 整文件 integrity 一次，仅对可修复损坏做表扫描 |
| Schema 契约 | `sqlite-schema-contract.ts` | 单次读事务内缓存事实 |
| 用户版本 | `sqlite-user-version.ts` | 拒绝 newer-schema |
| 文件指纹 | `sqlite-file-generation.ts` | SHA-256 + stat（双读检测并发修改） |

**laew gap L1041/L1042**：无 WAL 配置 + 无完整性检测。

---

## 三、Hook 系统（hooks/ 14699 行）

### 3.1 来源策略（policy.ts）

```
bundged (10) < plugin (20) < managed (30) < workspace (40)
```

- workspace hook 默认禁用（安全），需显式 opt-in
- bundled 不变源无缓存破坏，可变源 `?t=&c=&s=` 基于文件元数据

### 3.2 内部事件类型

```
agent:bootstrap / command:new / command:reset / command:stop
gateway:pre-restart / gateway:shutdown / gateway:startup
message:preprocessed / message:received / message:sent / message:transcribed
session:auto-reset / session:compact:after / session:compact:before / session:patch
```

### 3.3 代际加载（loader.ts）

- 原子性：先准备所有 handler，再统一 commit
- 失败回滚：unregister 所有已注册 handler

### 3.4 Fire-and-Forget

```typescript
const DEFAULT_MAX_CONCURRENT_FIRE_AND_FORGET_HOOKS = 16;
const DEFAULT_MAX_QUEUED_FIRE_AND_FORGET_HOOKS = 256;
const DEFAULT_FIRE_AND_FORGET_HOOK_TIMEOUT_MS = 2_000;
```

---

## 四、租约与 Worker 心跳（openclaw-state-lease*.ts ~600 行）

```typescript
interface OpenClawStateLeaseContext {
  renew(): Promise<void>;
  assertOwned(): void;
  heartbeat: "worker" | "main";
}

// Worker 线程心跳
SharedArrayBuffer + Atomics
状态：starting/ready/lost/closed
```

**laew gap L1043**：多进程部署时无法协调 SQLite 写入。

---

## 五、新增 gap 清单

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1041 | SQLite | 无 WAL 配置 | P0 |
| L1042 | SQLite | 无完整性检测与自动修复 | P0 |
| L1043 | 租约 | 无跨进程 SQLite 租约协调 | P0 |
| L1061 | 内存加密 | 无 AES-256-GCM 内存加密 | P1 |
| L1062 | SQLite | 无 Post-Commit 事务 | P1 |
| L1063 | SQLite | 无只读快照 + TOCTOU 保护 | P1 |
| L1064 | SQLite | 无 STRICT Schema 迁移 | P1 |
| L1065 | SQLite | 无索引契约验证 | P1 |
| L1066 | SQLite | 无 Schema 契约 | P1 |
| L1067 | SQLite | 无文件指纹 | P1 |
| L1068 | 状态DB | 无状态 DB 协调器 | P1 |
| L1069 | 状态DB | 无状态 DB 维护 | P1 |
| L1070 | 状态DB | 无状态 DB 缓存 | P1 |
| L1071 | 状态DB | 无 Schema 发布阻塞 | P1 |
| L1072 | 状态DB | 无隔离存储 | P1 |
| L1073 | 状态DB | 无 Schema 快速路径 | P1 |
| L1074 | 状态DB | 无 Schema 兼容性投影 | P1 |
| L1075 | 状态DB | 无内容版本跟踪 | P1 |
| L1076 | 状态DB | 无 Schema 修复 | P1 |
| L1077 | Hook | 无 Hook 系统核心 | P1 |
| L1078 | Hook | 无 Hook 安装事务 | P1 |
| L1079 | Hook | 无 Hook 缓存隔离 | P1 |
| L1080 | Hook | 无 Fire-and-Forget 机制 | P1 |
| L1081 | 设备 | 无设备配对认证 | P2 |
| L1082 | 守护进程 | 无堆内存自适应 | P2 |

---

**报告完成日期**：2026-09-09
**分析基础**：openclaw secrets/ + infra/sqlite-*.ts + hooks/ + state/openclaw-state-*.ts 逐行分析
