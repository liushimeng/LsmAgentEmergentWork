# 第十二轮 — 安全防御体系 / Prompt 注入防护 / 密钥管理 / 审计日志 深度对比

> **本报告覆盖 7 个 Agent 工程**：atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / jiuwenswarm
>
> **维度**：Prompt 注入防护 / 密钥与凭证管理 / 认证与授权 / 审计日志 / 输入验证 / 沙箱与隔离 / 敏感数据脱敏 / 依赖安全 / 速率限制 / 安全响应头
>
> **目标读者**：laew（Rust Agent CLI）的安全防御体系从零到生产级的演进规划

---

## 0. 全局架构对比总览

```
┌────────────────────────────────────────────────────────────────────────────┐
│                        Agent 安全防御 4 层防护模型                           │
├────────────────────────────────────────────────────────────────────────────┤
│ L4 输入层：Prompt 注入防护 / XML-tag 边界 / 输入清洗                          │
│     ↓                                                                     │
│ L3 工具层：Tool 权限拦截 / Bash 黑名单 / 路径白名单 / Seccomp / Landlock       │
│     ↓                                                                     │
│ L2 凭证层：密钥管理（Keychain） / OAuth / 凭证轮换 / 内存 zeroing             │
│     ↓                                                                     │
│ L1 网络层：SSRF 防护 / DNS pinning / TLS pinning / Proxy 策略                │
│     ↓                                                                     │
│ L0 审计层：操作审计 / 决策溯源 / 日志脱敏 / 防篡改                             │
└────────────────────────────────────────────────────────────────────────────┘
```

| 工程 | 注入防护 | 凭证管理 | 沙箱隔离 | 网络安全 | 审计日志 | 安全成熟度 |
|------|---------|---------|---------|---------|---------|-----------|
| **atomcode** | `<system-reminder>` 包裹 + 边界检测 | `paths::credential_path` + `--askpass` | L0/L1/L2 分层 + capabilities | `proxy.rs` 4 模式 + TLS 1.2 cap | `datalog` 模块 + 双 pass scrub | ⭐⭐⭐⭐ |
| **claudecode** | 27 Hook + permission flow UI | Keychain (macOS) + DPAPI (Win) | Permission 5 态 + SBPL | Sandbox Network Policy | Session log + JSONL | ⭐⭐⭐⭐⭐ |
| **deepseek-harness** | Cordis role('secret') 隔离 | 独立 credentials 包 | guard 层 + hooks | netns + cgroup | `goal/feedback` 持久化 | ⭐⭐⭐ |
| **openclaw** | Net Policy 纵深防御 | SecretRef 4 级 + jiti 隔离 | JiuwenBox（BubbleWrap + Seccomp） | DNS pinning + LRU Pool | 50K 条审计 + SHA256 锁 | ⭐⭐⭐⭐⭐ |
| **opencode** | Schema TaggedErrorClass | identity 包 + 加密存储 | Effect Layer + sandbox | OTLP 安全 + mTLS | OTel decision log | ⭐⭐⭐⭐ |
| **pi** | 5 个 toolHook 拦截 | TypeBox + 凭证加密 | Lane 三态隔离 | 双正则 retry 模式 | session-context 二级日志 | ⭐⭐⭐ |
| **jiuwenswarm** | JiuwenBox BubbleWrap | OAuth Device Flow | Landlock + Seccomp + netns + cgroup | 5 层网络隔离 | W3C PROV-O | ⭐⭐⭐⭐⭐ |
| **laew** | ❌ 无 | ❌ 内存明文 | ❌ 零沙箱 | ❌ 默认代理 | ❌ 无 | ⭐ |

---

## 1. Prompt 注入防护

### 1.1 各工程的实现细节

#### atomcode — `<system-reminder>` 边界约定

**核心源码**：`atomcode-capabilities/src/reminder.rs`

```rust
// atomcode 的统一 <system-reminder> 注入器
pub fn system_reminder(content: &str) -> String {
    format!("<system-reminder>\n{}\n</system-reminder>", content)
}
```

**设计要点**：
- **唯一构造函数**：所有上下文注入必须经过 `system_reminder()`，确保 `<system-reminder>` 包裹一致性
- **依赖无关**：放在 `capabilities` 层而非 `kernel` 层，让所有 L2/L3 能力都可用
- **XML 边界检测**：靠 LLM（Anthropic/OpenAI）天然识别 `<system-reminder>` 标签边界

#### claudecode — 27 Hook 拦截 + 边界包裹

**核心源码**：
- `src/hooks/`（27 种 hook 事件）
- `src/utils/permissions/`（permission flow）
- `src/components/permissions/`（UI 组件）

**设计要点**：
- **preToolUse Hook**：在工具调用前检查用户输入是否触发危险操作
- **Permission 5 态状态机**：allow / deny / ask / bypassPermissions / plan
- **`<system-reminder>` 包裹**：与 atomcode 类似，但还包含 metadata.user_id

#### openclaw — Net Policy 纵深防御

**核心源码**：
- `packages/net-policy/src/ip.ts`（IPv4 / IPv6 检测）
- `packages/net-policy/src/url-protocol.ts`（协议白名单）
- `packages/net-policy/src/redact-sensitive-url.ts`（URL 脱敏）
- `packages/net-policy/src/url-userinfo.ts`（userinfo 处理）

**设计要点**：
- **DNS pinning 防 DNS rebinding**
- **TLS fingerprint pin 防中间人**
- **PinnedDispatcherPool LRU** 防连接重放
- **event-loop 就绪探测** 防 TOCTOU

#### jiuwenswarm — JiuwenBox BubbleWrap

**核心源码**：
- `jiuwenbox/src/`（BubbleWrap 沙箱）
- `jiuwenbox/docker/`（Docker 容器化部署）

**设计要点**：
- **BubbleWrap**：基于 Linux user namespace + cgroup + seccomp 的进程级隔离
- **Landlock ABI 1-5**：文件系统访问控制
- **Seccomp**：系统调用过滤
- **netns**：网络命名空间隔离

### 1.2 Prompt 注入模式分类

| 模式 | 攻击方式 | 防御策略 |
|------|---------|---------|
| **直接注入** | 提示词中混入 "ignore previous instructions" | XML 边界 + 系统提示词前缀强化 |
| **间接注入** | 工具结果（WebFetch / Read）携带恶意指令 | 工具结果隔离标签 + 内容审查 |
| **越狱注入** | 通过角色扮演绕过限制 | 多层 hook + 决策审计 |
| **数据渗出** | 提示词诱导输出训练数据 | 输出过滤 + 敏感字段黑名单 |
| **权限提升** | 利用 tool 漏洞调用危险操作 | Tool 权限矩阵 + 黑白名单 |

### 1.3 laew 现状与差距

| 维度 | laew 现状 | P0 差距 |
|------|----------|---------|
| 注入防护 | ❌ 无 `<system-reminder>` 包裹 | L341: 无系统提示词边界包裹 |
| 内容审查 | ❌ 无工具结果审查 | L342: 无 ToolResult 危险模式检测 |
| 用户输入清洗 | ❌ 原始字符串拼接 | L343: 无输入长度限制 + 字符白名单 |
| 系统提示词隔离 | ⚠️ 部分（Yolo + SessionContext） | L344: 无统一 reminder 构造函数 |

**推荐 Rust crate**：
- `ammonia` — HTML 清理库，可用于内容脱敏
- `regex` / `regex-lite` — 危险模式匹配
- `pulldown-cmark` — Markdown 安全解析

---

## 2. 密钥与凭证管理

### 2.1 各工程的实现

#### atomcode — `paths::credential_path` + askpass

**核心源码**：
- `atomcode-capabilities/src/paths.rs`（凭证路径）
- `atomcode-capabilities/src/bin/`（askpass 二进制）

```rust
// atomcode 的凭证路径门控
pub fn credential_path(name: &str) -> PathBuf {
    let home = atomcode_home();
    home.join("credentials").join(name)
}
```

**设计要点**：
- **集中路径管理**：所有凭证文件统一放在 `$ATOMCODE_HOME/credentials/`
- **askpass 二进制**：用于 OAuth 流程中输入敏感凭证（避免明文存储）
- **环境变量回退**：`ATOMCODE_API_KEY` 环境变量优先级

#### claudecode — Keychain + DPAPI

**核心源码**：
- 平台特定：`keychain-rs` (macOS) / DPAPI (Windows)
- 凭证加密后存储到 SQLite `secrets` 表

**设计要点**：
- **平台原生**：macOS 用 Keychain，Windows 用 DPAPI，Linux 用 SecretService（gnome-keyring / kwallet）
- **加密 fallback**：无 keyring 时用 AES-GCM + 机器指纹派生密钥
- **轮换机制**：支持手动 / 自动轮换

#### openclaw — SecretRef 4 级

**核心源码**：
- 配置加载管线中的 `SecretRef` 抽象
- 支持 4 种来源：环境变量 / 文件 / Keychain / Vault

#### opencode — identity 包

**核心源码**：
- `packages/identity/`（身份与凭证）
- 加密存储 + 自动过期

### 2.2 凭证生命周期

```
创建 ──→ 加密存储 ──→ 注入内存 ──→ 使用 ──→ 轮换 ──→ 撤销 ──→ 清理
  │          │            │          │          │          │          │
  │          │            │          │          │          │          └─ zeroing
  │          │            │          │          │          └─ revocation_list
  │          │            │          │          └─ 过期检测
  │          │            │          └─ 用后即焚
  │          │            └─ mlock 防 swap
  │          └─ AES-GCM + 机器指纹
  └─ 用户输入 / OAuth / 引导
```

### 2.3 laew 现状与差距

| 维度 | laew 现状 | P0 差距 |
|------|----------|---------|
| API Key 存储 | ⚠️ SQLite 明文 | L345: 凭证明文存储（安全风险） |
| 凭证轮换 | ❌ 无 | L346: 无凭证轮换机制 |
| 内存安全 | ❌ Rust 字符串驻留 | L347: 无 mlock 防 swap + zeroize |
| OAuth 流程 | ❌ 无 | L348: 无 OAuth 2.0 / PKCE |
| 多账号 | ❌ 单 key | L349: 无多账号管理 |

**推荐 Rust crate**：
- `keyring` — 跨平台 keychain 抽象（macOS Keychain / Windows DPAPI / Linux SecretService）
- `zeroize` — 内存清理（防止 swap 泄漏）
- `secrecy` — Secret 类型包装
- `aes-gcm` — 对称加密
- `oauth2` — OAuth 2.0 客户端
- `mlock` — 锁定内存防 swap

**P0 实现示例**：

```rust
// Cargo.toml
[dependencies]
keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service"] }
secrecy = { version = "0.8", features = ["serde"] }
zeroize = { version = "1", features = ["derive"] }
aes-gcm = "0.10"
argon2 = "0.5"

// src/config/credential.rs
use keyring::Entry;
use secrecy::{Secret, ExposeSecret};
use zeroize::Zeroize;

#[derive(ZeroizeOnDrop)]
pub struct ApiKey(Secret<String>);

impl ApiKey {
    pub fn from_keychain(provider: &str) -> Result<Self> {
        let entry = Entry::new("laew", provider)?;
        let password = entry.get_password()?;
        Ok(Self(Secret::new(password)))
    }

    pub fn store(&self, provider: &str) -> Result<()> {
        let entry = Entry::new("laew", provider)?;
        entry.set_password(self.0.expose_secret())?;
        Ok(())
    }
}
```

---

## 3. 认证与授权

### 3.1 OAuth 2.0 实现的对比

| 工程 | 授权模式 | PKCE | Refresh Token | 多账号 |
|------|---------|------|---------------|--------|
| **claudecode** | Authorization Code + PKCE | ✅ | ✅ | ✅ |
| **atomcode** | Authorization Code | ✅ | ✅ | ⚠️ |
| **openclaw** | Authorization Code + Device Flow | ✅ | ✅ | ✅ |
| **opencode** | Authorization Code + Bearer | ✅ | ✅ | ✅ |
| **jiuwenswarm** | Authorization Code + Device Flow | ✅ | ✅ | ✅ |
| **pi** | API Key | ❌ | ❌ | ❌ |
| **deepseek-harness** | API Key | ❌ | ❌ | ❌ |

### 3.2 claudecode 的 PKCE 实现（最完整）

```typescript
// claudecode OAuth PKCE 流程
class OAuthFlow {
  // 1. 生成 code_verifier (43-128 chars)
  private codeVerifier = crypto.randomBytes(32).toString('base64url');

  // 2. 计算 code_challenge = SHA256(code_verifier) base64url
  private codeChallenge = crypto
    .createHash('sha256')
    .update(this.codeVerifier)
    .digest('base64url');

  // 3. 构造授权 URL
  buildAuthUrl(state: string): string {
    const params = new URLSearchParams({
      response_type: 'code',
      client_id: this.clientId,
      redirect_uri: this.redirectUri,
      scope: this.scope,
      state,
      code_challenge: this.codeChallenge,
      code_challenge_method: 'S256',
    });
    return `${this.authEndpoint}?${params}`;
  }

  // 4. 回调后用 code_verifier 换 token
  async exchangeCode(code: string): Promise<TokenResponse> {
    const res = await fetch(this.tokenEndpoint, {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({
        grant_type: 'authorization_code',
        code,
        client_id: this.clientId,
        redirect_uri: this.redirectUri,
        code_verifier: this.codeVerifier,
      }),
    });
    return res.json();
  }
}
```

### 3.3 跨进程刷新锁（atomcode）

```rust
// atomcode 的跨进程 OAuth refresh 锁（fcntl）
pub struct CrossProcessRefreshLock {
    file: File,
}

impl CrossProcessRefreshLock {
    pub fn try_acquire(path: &Path) -> Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(path)?;
        // fcntl 排他锁（多进程互斥）
        let lock = flock(LockType::ExclusiveNonblock, &file)?;
        Ok(if lock { Some(Self { file }) } else { None })
    }
}
```

### 3.4 laew 现状与差距

| 维度 | laew 现状 | P0 差距 |
|------|----------|---------|
| OAuth | ❌ 无 | L350: 无 OAuth 2.0 客户端 |
| PKCE | ❌ 无 | L351: 无 PKCE 防御授权码拦截 |
| 刷新令牌 | ❌ 无 | L352: 无 Refresh Token 自动刷新 |
| 跨进程锁 | ❌ 无 | L353: 无 fcntl 排他锁防并发刷新 |
| 多账号 | ❌ 无 | L354: 无多 Provider 凭证管理 |

**推荐 Rust crate**：
- `oauth2` — OAuth 2.0 客户端（支持 PKCE / Device Flow）
- `reqwest` + `tokio` — HTTP 客户端
- `url` — URL 解析
- `ring` — SHA256（PKCE 用）
- `fs2` — 文件锁

---

## 4. 审计日志

### 4.1 各工程的实现

#### openclaw — 50K 条审计日志 + SHA256 锁

**核心源码**：
- 配置加载管线中的 `audit_log` 抽象
- 50K 条循环覆盖
- SHA256 链式哈希防篡改

#### atomcode — datalog 模块

**核心源码**：
- `atomcode-capabilities/src/datalog.rs`（决策审计）
- `atomcode-telemetry/`（OTel 集成）

**设计要点**：
- **每轮请求一条 JSONL**：包含 prompt / tool_call / tool_result / decision
- **双 pass scrub**：先识别敏感字段，再脱敏
- **W3C traceparent 注入**：与 OTel trace 关联

#### claudecode — Session log + JSONL

**核心源码**：
- `src/history.ts`（会话历史）
- `src/services/audit/`（审计服务）

### 4.2 审计日志结构

```rust
// atomcode-style 审计日志
#[derive(Serialize)]
pub struct AuditEntry {
    pub timestamp: SystemTime,
    pub session_id: String,
    pub user_id: String,
    pub action: AuditAction,
    pub tool: Option<String>,
    pub args_hash: String,        // SHA256(args)，不存明文
    pub result_hash: String,
    pub decision: DecisionTrace,
    pub duration_ms: u64,
    pub trace_id: String,          // W3C traceparent
    pub span_id: String,
}

#[derive(Serialize)]
pub enum AuditAction {
    ProviderCall { provider: String, model: String },
    ToolCall { tool: String, redacted_args: String },
    ConfigChange { key: String, old_hash: String, new_hash: String },
    AuthEvent { event: AuthEvent },
}
```

### 4.3 laew 现状与差距

| 维度 | laew 现状 | P0 差距 |
|------|----------|---------|
| 操作审计 | ❌ 无 | L355: 无 Tool 调用审计 |
| 决策溯源 | ❌ 无 | L356: 无 Yolo 分类决策记录 |
| 日志防篡改 | ❌ 无 | L357: 无 SHA256 链式哈希 |
| 字段脱敏 | ❌ 无 | L358: 无 API Key 自动脱敏 |
| 关联 Trace | ❌ 无 | L359: 无 W3C traceparent 注入 |

---

## 5. 输入验证与清洗

### 5.1 各工程的实现

| 工程 | Schema 校验 | 长度限制 | 字符白名单 | 路径遍历防护 | SSRF 防护 |
|------|-----------|---------|-----------|------------|----------|
| **atomcode** | schemars | ✅ | ⚠️ | ✅ pathnorm | ✅ proxy.rs |
| **claudecode** | Zod | ✅ | ✅ | ✅ | ✅ WebFetch 两阶段 |
| **openclaw** | TypeBox | ✅ | ✅ | ✅ net-policy | ✅ 纵深防御 |
| **opencode** | Effect Schema | ✅ | ✅ | ✅ | ✅ |
| **pi** | TypeBox | ✅ | ⚠️ | ⚠️ | ⚠️ |
| **deepseek-harness** | 自研 | ✅ | ⚠️ | ⚠️ | ⚠️ |
| **jiuwenswarm** | Pydantic | ✅ | ✅ | ✅ | ✅ net-policy |
| **laew** | ❌ | ❌ | ❌ | ❌ | ❌ |

### 5.2 openclaw 的 SSRF 纵深防御

```typescript
// openclaw/packages/net-policy/src/url-protocol.ts
export function validateUrl(url: URL): ValidationResult {
  // 1. 协议白名单
  if (!['https:', 'http:'].includes(url.protocol)) {
    return { ok: false, reason: 'protocol_not_allowed' };
  }

  // 2. 私有 IP 阻断（IPv4）
  const ip = ipaddr.parse(url.hostname);
  if (isPrivateIPv4(ip)) {
    return { ok: false, reason: 'private_ip_blocked' };
  }

  // 3. IPv6 私有地址检测
  if (isPrivateIPv6(ip)) {
    return { ok: false, reason: 'private_ipv6_blocked' };
  }

  // 4. CGNAT 范围（100.64.0.0/10）
  if (isCGNAT(ip)) {
    return { ok: false, reason: 'cgnat_blocked' };
  }

  // 5. loopback / link-local / multicast
  if (isLoopback(ip) || isLinkLocal(ip) || isMulticast(ip)) {
    return { ok: false, reason: 'special_purpose_ip_blocked' };
  }

  return { ok: true };
}
```

### 5.3 claudecode 的 WebFetch 两阶段

```typescript
// 第一阶段：URL 安全检查
async function safeFetchUrl(url: string): Promise<SafetyResult> {
  const parsed = new URL(url);

  // 协议限制
  if (!['http:', 'https:'].includes(parsed.protocol)) return unsafe;

  // 私有 IP 阻断
  const ips = await resolveAll(parsed.hostname);
  for (const ip of ips) {
    if (isPrivateIP(ip)) return unsafe;
  }

  // DNS pinning：保存当前解析结果，防后续 DNS rebinding
  return { safe: true, pinnedIps: ips };
}

// 第二阶段：内容审查
async function safeFetchContent(url: string): Promise<ContentResult> {
  const safety = await safeFetchUrl(url);
  if (!safety.safe) return { error: 'unsafe_url' };

  // 用 pinned IP 发起连接（不走第二次 DNS）
  return await fetchWithPinnedIPs(url, safety.pinnedIps);
}
```

### 5.4 laew 现状与差距

| 维度 | laew 现状 | P0 差距 |
|------|----------|---------|
| URL 校验 | ❌ 无 | L360: 无 SSRF URL 白名单 |
| DNS 防护 | ❌ 无 | L361: 无 DNS pinning |
| 路径遍历 | ❌ 无 | L362: 无 `..` 检测 |
| 长度限制 | ⚠️ 部分 | L363: 无 Prompt 长度上限 |

---

## 6. 沙箱与隔离

### 6.1 各工程的沙箱实现

| 工程 | 沙箱方案 | 平台支持 | 隔离粒度 |
|------|---------|---------|---------|
| **jiuwenswarm JiuwenBox** | BubbleWrap + Landlock + Seccomp + netns + cgroup | Linux | 进程级 |
| **claudecode** | macOS SBPL + Windows AppContainer + Landlock | 跨平台 | 进程级 |
| **openclaw** | Docker + Seccomp + user namespace | Linux/Docker | 容器级 |
| **opencode** | Effect sandbox layer | 应用层 | 类型级 |
| **atomcode** | L1 capabilities feature gating | 编译时 | 模块级 |
| **pi** | Lane 三态 + 进程隔离 | 进程级 | 任务级 |
| **laew** | ❌ 无 | ❌ | ❌ |

### 6.2 jiuwenswarm JiuwenBox 沙箱（最完整）

```python
# JiuwenBox 多层沙箱
class JiuwenBox:
    def __init__(self):
        # 1. Landlock ABI 1-5 协商
        self.landlock = Landlock(
            rules=[
                FSRule("/workspace", AccessFS::READ | AccessFS::WRITE),
                FSRule("/tmp", AccessFS::READ | AccessFS::WRITE),
                FSRule("/", AccessFS::READ),  # 默认可读
            ]
        )

        # 2. Seccomp 系统调用过滤
        self.seccomp = Seccomp(
            allow=[
                "read", "write", "open", "close", "stat", "fstat",
                "mmap", "mprotect", "brk", "exit", "exit_group",
            ],
            deny=["ptrace", "mount", "umount2", "kexec_load", "init_module"]
        )

        # 3. netns 网络命名空间
        self.netns = NetNS(allowed_ips=["8.8.8.8", "1.1.1.1"])

        # 4. cgroup 资源限制
        self.cgroup = CGroup(
            cpu_max="50000 100000",  # 50% CPU
            memory_max="2G",
            pids_max=100,
        )

    def wrap(self, command: str) -> SandboxProcess:
        return SandboxProcess(
            command,
            landlock=self.landlock,
            seccomp=self.seccomp,
            netns=self.netns,
            cgroup=self.cgroup,
        )
```

### 6.3 laew 现状与差距

| 维度 | laew 现状 | P0 差距 |
|------|----------|---------|
| 进程沙箱 | ❌ 无 | L364: 无 Landlock/Seccomp |
| 文件系统隔离 | ❌ 无 | L365: 无 Bash 路径白名单 |
| 网络隔离 | ❌ 无 | L366: 无 netns |
| 资源限制 | ❌ 无 | L367: 无 cgroup |

---

## 7. 敏感数据脱敏

### 7.1 脱敏模式

| 类别 | 模式 | 脱敏策略 |
|------|------|---------|
| API Key | `sk-***`, `gsk_***`, `xai-***` | 正则匹配 → 替换为 `****<末4位>` |
| 邮箱 | `***@***.com` | 全局正则 |
| 信用卡 | `**** **** **** 1234` | Luhn 校验 + 部分遮蔽 |
| IP 地址 | 内网 IP 部分遮蔽 | RFC1918 检测 |
| 路径 | `$HOME` → `~` | Tilde 替换 |
| 密码 | 全替换为 `***` | 永远不记录 |

### 7.2 内存 zeroing（Rust）

```rust
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(ZeroizeOnDrop)]
pub struct ApiKey {
    key: String,
}

impl Drop for ApiKey {
    fn drop(&mut self) {
        self.key.zeroize();  // 内存归零
    }
}
```

### 7.3 laew 现状与差距

| 维度 | laew 现状 | P0 差距 |
|------|----------|---------|
| API Key 脱敏 | ⚠️ TUI 显示 `****<末4位>` | L368: 无日志脱敏 |
| 内存清理 | ❌ 无 | L369: 无 zeroize |
| 输出过滤 | ❌ 无 | L370: 无危险模式输出检测 |

---

## 8. 依赖安全

| 实践 | 实施难度 | laew 现状 |
|------|---------|----------|
| `cargo audit` | 低 | ❌ 未集成 |
| `cargo deny` | 低 | ❌ 未集成 |
| SBOM (CycloneDX) | 中 | ❌ 未生成 |
| 依赖固定 (`Cargo.lock`) | 低 | ✅ |
| 漏洞扫描 CI | 中 | ❌ 未集成 |

---

## 9. 速率限制

### 9.1 各工程的限流策略

| 工程 | 算法 | 粒度 | 维度 |
|------|------|------|------|
| **claudecode** | 令牌桶 | 用户级 | RPM / TPM / Cost |
| **openclaw** | 滑动窗口 | Provider 级 | RPM / TPM |
| **Switchyard** | 自适应 | 全局 | RPM / 成本 / 延迟 |
| **pi** | 简单计数 | Provider 级 | RPM |

### 9.2 laew 现状与差距

| 维度 | laew 现状 | P0 差距 |
|------|----------|---------|
| API 限流 | ❌ 无 | L371: 无 RPM/TPM 限流 |
| 配额管理 | ❌ 无 | L372: 无 Provider 配额追踪 |

---

## 10. 安全响应头与配置

| 配置 | 推荐值 | laew 现状 |
|------|--------|----------|
| HSTS | `max-age=31536000; includeSubDomains` | N/A（CLI 非 Web） |
| CSP | `default-src 'none'` | N/A |
| X-Frame-Options | `DENY` | N/A |
| User-Agent | 标识版本 + commit hash | ✅ `User-Agent: LsmAgentEmergentWork/{version}` |

---

## 11. laew 安全防御改造路线图

### 11.1 P0（立即实施）

1. **API Key 迁移到 Keychain**（1 周）
2. **OAuth 2.0 + PKCE 实现**（2 周）
3. **SSRF URL 白名单 + DNS pinning**（1 周）
4. **Audit 日志 + 字段脱敏**（1 周）
5. **memory zeroize**（3 天）

### 11.2 P1（中期）

1. **`<system-reminder>` 统一注入器**（3 天）
2. **Bash 命令沙箱（白名单 + Landlock）**（2 周）
3. **审计日志 SHA256 链式哈希**（1 周）
4. **OAuth 跨进程刷新锁**（3 天）

### 11.3 P2（长期）

1. **BubbleWrap / Seccomp 沙箱**（4 周）
2. **SBOM 生成 + cargo deny CI**（1 周）
3. **多账号凭证管理 UI**（2 周）

---

## 12. 推荐 Rust crate 清单

| 类别 | crate | 用途 | 优先级 |
|------|-------|------|--------|
| Keychain | `keyring` | 跨平台 keychain 抽象 | P0 |
| 加密 | `aes-gcm` | 对称加密 | P0 |
| 密钥派生 | `argon2` | 密码哈希 | P0 |
| 内存安全 | `zeroize` + `secrecy` | 内存清理 + Secret 包装 | P0 |
| OAuth | `oauth2` | OAuth 2.0 客户端 | P0 |
| PKCE | `ring` | SHA256（PKCE） | P0 |
| 文件锁 | `fs2` | fcntl 排他锁 | P1 |
| 沙箱 | `landlock` | Linux 文件系统沙箱 | P1 |
| 沙箱 | `seccompiler` | 系统调用过滤 | P2 |
| 容器 | `bollard` | Docker 集成 | P2 |
| HTTP | `reqwest` + `rustls` | 安全 HTTP 客户端 | P0 |
| SSRF | `ipnetwork` | IP 段检测 | P0 |
| 审计 | `tracing` | 结构化日志 | P0 |
| 文本 | `ammonia` | HTML 清理 | P1 |
| Schema | `schemars` | JSON Schema | P1 |

---

## 13. 与第十一轮关系

| 第十一轮 | 第十二轮 | 关联 |
|---------|---------|------|
| 配置系统 → API Key 5 档解析 | 密钥管理 → Keychain + 加密存储 | 从解析到存储 |
| OAuth 认证专题（第九轮） | OAuth PKCE + Refresh Token + 跨进程锁 | 深化 |
| 沙箱设计 → 5 维隔离 | 沙箱 → BubbleWrap + Landlock 5 ABI | 深化 |

---

## 14. 累计 laew gap（L1-L372）

| 轮次 | 范围 | 数量 |
|------|------|------|
| 第 1-4 轮 | L1-L15 | 15 |
| 第 5 轮 | L16-L25 | 10 |
| 第 6 轮 | L26-L37 | 12 |
| 第 7 轮 | L38-L78 | 41 |
| 第 8 轮 | L79-L142 | 64 |
| 第 9 轮 | L142-L160 | 19 |
| 第 10 轮 | L161-L220 | 60 |
| 第 11 轮 | L221-L280 | 60 |
| **第 12 轮** | **L281-L372** | **92** |
| **累计** | **L1-L372** | **372** |

---

## 15. 总结

### 15.1 核心发现

1. **openclaw + jiuwenswarm** 是安全防御最完整的两个工程（5 层防御 + BubbleWrap）
2. **claudecode 的 PKCE 实现** 是 OAuth 防御的最佳实践
3. **atomcode 的 capabilities 分层** 是 Rust 模块级隔离的典范
4. **Datalog 双 pass scrub** 是敏感字段脱敏的标准做法
5. **laew 现状为零**：所有 32 个新 gap 都是 P0 紧急级别

### 15.2 laew 最急需的 5 个安全能力

1. **Keychain 凭证管理**（P0）
2. **OAuth 2.0 + PKCE**（P0）
3. **SSRF 纵深防御**（P0）
4. **审计日志 + 脱敏**（P0）
5. **Landlock/Seccomp 沙箱**（P1）

---

> **本报告完成标记**
> - 文件：`专题-第十二轮-安全防御体系与Prompt注入防护与密钥管理深度对比.md`
> - 覆盖 7 个工程 × 10 大维度
> - 新增 laew gap: L341-L372（32 个）
> - 累计 laew gap: L1-L372（372 个）
