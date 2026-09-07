# 专题-第九轮-OAuth认证与多账号与凭证管理深度对比

> 第九轮 T3 专题：**8 工程 × 9 维度**横向对比，覆盖 OAuth 2.0 / 多账号 / API Key / 凭证存储 / 脱敏 / Keyring / 跨进程锁。
> 调研对象：atomcode / claudecode / Switchyard / cc-switch / agent-studio / openclaw / deepseek-harness / pi。
> 调研时间：2026-09-07；目标读者：laew 维护者、企业安全架构师、DevX 工程师。

---

## 1. 摘要与导读

OAuth 与凭证管理是 laew 升级到「生产级 CLI」的 **L49-L53 五个 gap** 的核心：

| Gap | 描述 | 紧急度 |
|-----|------|--------|
| **L49** | API Key 明文存 SQLite `providers` 表 | P0 |
| **L50** | 无 OAuth 流程（仅 API Key 模式） | P1 |
| **L51** | 日志/UI 仅有 `mask_key` 末4位 | P1 |
| **L52** | 无多账号轮换 / 熔断 | P1 |
| **L53** | 无跨进程文件锁（多窗口并发刷新会丢 token） | P2 |

8 工程调研后我们看到 **5 档凭证存储哲学**：
1. **L1 OS Keyring**（claudecode macOS Keychain / `security -i -X`）
2. **L2 加密 SQLite**（agent-studio AES-GCM + KMS）
3. **L3 紧权限文件**（atomcode 0o600 + atomic rename + fs2）
4. **L4 TOML/env 引用**（Switchyard `api_key_env` 永不存明文）
5. **L5 SecretRef 抽象**（openclaw `source: env|file|exec|store` 4 后端延迟绑定）

---

## 2. 8 工程凭证管理概览

### 2.1 atomcode（Rust，**L3 范本**）
- **路径**：`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-auth/`
- **核心范式**：
  - `write_auth_file_secure()`：0o600 + atomic rename + cross-process `fs2::FileExt::lock_exclusive`（`lib.rs:25-92`）
  - `refresh_auth_if_current()`：跨进程锁内串行化 refresh_token 消费（`lib.rs:1343-1363`）
  - `get_valid_auth_info()`：5 分钟 TTL 提前刷新（`lib.rs:1365-1399`）
  - **CbreakGuard** + detached thread polling：OAuth browser flow 不冻结 TUI（`lib.rs:600-735`）
  - **RequestSigner trait**：把签名实现为可替换策略（`gateway_crypto.rs:9-89`）

### 2.2 claudecode（TS/Bun，**L1 范本**）
- **路径**：`/usr/local/LsmGitOpenSource/claudecode/src/utils/auth.ts`
- **核心范式**：
  - 6 层 token 优先级（managed OAuth → env → FD → apiKeyHelper → keychain → config）— `auth.ts:153-206`
  - macOS Keychain 用 `security -i -X` hex 编码（避免 ps 窥探）— `auth.ts:1094-1160`
  - 并发 401 dedup：`pending401Handlers: Map<token, Promise>`（`auth.ts:1343-1434`）
  - apiKeyHelper trust gate：未授信 workspace 拒绝执行（`auth.ts:546-556`）
  - **OAuth FedStart allowlist**：仅 4 个受信 base URL（`constants/oauth.ts:84-104`）

### 2.3 Switchyard（Rust，**L4 范本**）
- **路径**：`/usr/local/LsmGitOpenSource/Switchyard/crates/switchyard-runner/src/config.rs`
- **核心范式**：
  - TOML 配置只存 `api_key_env`（变量名），运行时 `std::env::var()` 解析（`config.rs:405-489`）
  - `Debug` 永远把 `api_key` 渲染为 `[REDACTED]`（`backend.rs:61-72`）
  - HTTP header 用 `HeaderValue::set_sensitive(true)` 标记（`backend.rs:215-241`）
  - `extra_headers` 拒绝覆盖 `authorization`/`x-api-key`（`backend.rs:88-114`）

### 2.4 cc-switch（Tauri+Rust，**多 OAuth 反代范本**）
- **路径**：`/usr/local/LsmGitOpenSource/cc-switch/src-tauri/src/proxy/providers/auth.rs`
- **核心范式**：
  - `AuthInfo` 统一 6 种 strategy（Anthropic/ClaudeAuth/OpenAiBearer/GoogleOAuth/Gemini/CustomHeaders）
  - `masked_key()`：前4 + 后4 字符脱敏（`auth.rs:8-79`）
  - `redact_known_secrets`：按已知密钥池模糊化（`lib.rs:136-177`）
  - 3 个 OAuth 反代：`codex_oauth_auth.rs`、`xai_oauth_auth.rs`、`copilot_auth.rs`
  - `CodexOAuthState(Arc<CodexOAuthManager>)` 多账号 + `default_account_id()`

### 2.5 agent-studio（Python，**L2 范本**）
- **路径**：`/usr/local/LsmGitOpenSource/agent-studio/backend/openjiuwen_studio/core/manager/model_manager/utils/security_utils.py`
- **核心范式**：
  - AES-GCM-256 加密 DB 存储的 API key（`security_utils.py:42-283`）
  - HKDF-SHA256 派生密钥 + 16字节随机 salt + 12字节 nonce
  - 可选华为云 KMS 托管根密钥
  - JWT (access 短期 + refresh 长期) 双重 token
  - PBKDF2-SHA256 10k 轮 +16 字节 salt 哈希密码
  - `mask_api_key` 默认仅显示后4位（`security_utils.py:286-305`）
  - Provider-specific 格式校验（OpenAI/Anthropic/DeepSeek 前缀）

### 2.6 openclaw（TS，**L5 范本**）
- **路径**：`/usr/local/LsmGitOpenSource/openclaw/src/config/types.secrets.ts`
- **核心范式**：
  - `SecretRef` 抽象统一 4 后端：`env | file | exec | store`
  - env shorthand `$VAR` / `${VAR}` 自动解析（`types.secrets.ts:33-35`）
  - `maskApiKey` UTF-16 安全（避免切 surrogate pair）— `secret-mask.ts:1-29`
  - 90+ token 前缀正则（sk-/ghp_/xoxb-/AKIA/AWS/AIza/JWT 等）— `redact-patterns.ts:130-249`
  - argv suffix 自动脱敏（`redact-argv.ts:6-44`）
  - **xAI OAuth device-code flow** + SSRF guard（`extensions/xai/xai-oauth.ts:26-100`）

### 2.7 deepseek-harness（TS，**Service 抽象范本**）
- **路径**：`/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-pi-ai/src/auth.ts`
- **核心范式**：
  - 凭证抽象为独立 `dsh-credentials` 服务（可挂载多种实现）
  - `RECORD_SCOPE` 命名空间隔离（`RECORD_SCOPE = 'llm-pi-ai'`）
  - 严格 read/list/modify/delete 分类，写操作拒绝不可寻址 id
  - `LEGAL_API_KEY = /^[\x21-\x7E]+$/` 严格 ASCII 校验（`api-key.ts:15-41`）
  - OAuth grant payload 原样存（库不解释，让 plugin 自己拥有格式）

### 2.8 pi（TS，**多 OAuth Provider 范本**）
- **路径**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/auth/oauth/`
- **核心范式**：
  - 浏览器+Node 双兼容 PKCE（Web Crypto API）— `pkce.ts:1-35`
  - 7 个 OAuth provider 独立模块（Anthropic / OpenAI Codex / GitHub Copilot / OpenRouter / Kimi / xAI / Radius）
  - Client ID base64 编码（防 grep）— `anthropic.ts:30-312`
  - 本机回调端口 53692 + state=PKCE verifier 双重 CSRF
  - Device Code Flow RFC 8628 标准（`device-code.ts:1-99`）
  - `getEnvApiKey` 多 provider env 映射（`env-api-keys.ts:78-120`）
  - `<authenticated>` 哨兵而非真实 key（避免凭证泄露）

---

## 3. 维度 1：API Key 存储

### 3.1 横向对比表

| 工程 | 存储后端 | 文件权限 | 加密 | 原子写 | 跨进程锁 |
|------|---------|---------|------|--------|---------|
| **atomcode** | TOML 文件 | 0o600 | ❌ | ✅ temp+rename | ✅ fs2 |
| **claudecode** | macOS Keychain + config | OS | ✅ OS 控制 | ✅ OS 控制 | ⚠️ |
| **Switchyard** | env var（TOML 引用） | N/A | N/A | N/A | N/A |
| **cc-switch** | SQLite 明文 | 0644 | ❌ | ⚠️ | ✅ SQLite WAL |
| **agent-studio** | SQLite 加密 | DB | ✅ AES-GCM + KMS | ✅ | ✅ SQLite |
| **openclaw** | SecretRef 延迟绑定 | 取决于后端 | 取决于后端 | 取决于后端 | ⚠️ |
| **deepseek-harness** | dsh-credentials 服务 | 取决于实现 | 取决于实现 | 取决于实现 | ✅ modifyRecord 串行化 |
| **pi** | `~/.config` 文件 | ⚠️ | ⚠️ | ⚠️ | ❌ |

### 3.2 atomcode 范本（`lib.rs:25-92`）

```rust
pub fn write_auth_file_secure(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;   // 0o700
    }
    #[cfg(unix)] {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let tmp_path = temp_auth_path(path);
        let mut file = OpenOptions::new()
            .create_new(true).write(true).truncate(true).mode(0o600)
            .open(&tmp_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp_path, path)?;        // atomic rename
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    ...
}
```

**范式要点**：
1. **0o700 父目录 + 0o600 文件**双层紧权限
2. **temp+rename 原子写**（避免半截文件）
3. **fs2 跨进程互斥锁**（多窗口并发安全）

### 3.3 agent-studio 范本（`security_utils.py:42-283`）

```python
def encrypt_api_key(self, api_key: str) -> str:
    salt = self.generate_random_key()                    # 16 字节 salt
    encryption_key = self.hkdf_drive(self.master_key, salt)  # HKDF-SHA256
    nonce = get_random_bytes(12)
    cipher = AES.new(encryption_key, AES.MODE_GCM, nonce=nonce)
    ciphertext, auth_tag = cipher.encrypt_and_digest(api_key.encode('utf-8'))
    combined_data = salt + nonce + ciphertext + auth_tag
    return base64.b64encode(combined_data).decode('utf-8')
```

**范式要点**：
1. **AES-GCM-256** + 每次随机 salt+nonce
2. **HKDF** 派生密钥（避免直接用 master key）
3. **可选 KMS** 托管根密钥（生产模式）

### 3.4 laew 现状评估

`src/config/mod.rs` 的 `Db` SQLite CRUD：
```rust
pub struct ProviderRecord {
    pub protocol: String, pub provider_name: String, pub model_name: String,
    pub end_point: String, pub api_key: String,  // ⚠️ 明文
    pub is_active: bool,
}
```

**L49 gap**：明文存储 SQLite，无 0o600 文件权限保障。

**修复路径**：
1. 短期：DB 文件加 0o600（macOS/Linux/Windows ACL）。
2. 中期：用 SQLCipher 加密（`rusqlite` + `sqlite-cipher` feature）。
3. 长期：引入 OS Keyring（`keyring` crate）+ 内存解密。

---

## 4. 维度 2：API Key 轮换 / 多账号

### 4.1 横向对比表

| 工程 | 多账号 | 切换 | 配额分摊 | 加权轮询 | 故障切换 |
|------|--------|------|---------|---------|---------|
| **atomcode** | ❌ 单一 login 状态 | - | - | - | - |
| **claudecode** | ✅ 多 token source 优先级 | ✅ | ⚠️ | ⚠️ | ✅ 401 dedup |
| **Switchyard** | ✅ N 个 LLM client | ✅ config | ⚠️ | ⚠️ | ✅ max_retries |
| **cc-switch** | ✅ 6 strategy | ✅ live 切换 | ⚠️ | ⚠️ | ✅ OAuth 刷新 |
| **agent-studio** | ✅ 多 provider + 多租户 | ✅ | ✅ SecurityManager | ⚠️ | ✅ rate-limit |
| **openclaw** | ✅ SecretRef 抽象 | ✅ | ⚠️ | ⚠️ | ✅ xai OAuth retry |
| **deepseek-harness** | ✅ providerId 命名空间 | ✅ | ⚠️ | ⚠️ | ✅ modifyRecord 串行化 |
| **pi** | ✅ 多 provider id | ✅ env priority | ⚠️ | ⚠️ | ⚠️ |

### 4.2 cc-switch 多 OAuth 范本（`xai_oauth.rs:29-58`）

```rust
pub(crate) async fn query_xai_oauth_quota_for(
    state: &XaiOAuthState, account_id: Option<String>,
) -> Result<SubscriptionQuota, String> {
    let manager = state.0.read().await;
    let resolved = match account_id.as_deref().map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => Some(id.to_string()),
        None => manager.default_account_id().await,  // fallback 到默认
    };
    let Some(id) = resolved else { return Ok(SubscriptionQuota::not_found("xai_oauth")); };
    let token = match manager.get_valid_token_for_account(&id).await { ... };
}
```

**范式要点**：
1. **Manager 内部细粒度锁**，外层 `Arc<Manager>` 而非 `RwLock<Manager>`（避免网络阻塞）
2. **`default_account_id()`** 自动 fallback
3. **`get_valid_token_for_account`** 账号级 token 拉新

### 4.3 pi 多 provider env 范本（`env-api-keys.ts:78-120`）

```typescript
const envMap: Record<string, string> = {
    "ant-ling": "ANT_LING_API_KEY",
    "qwen-token-plan": "QWEN_TOKEN_PLAN_API_KEY",
    openai: "OPENAI_API_KEY",
    "azure-openai-responses": "AZURE_OPENAI_API_KEY",
    nvidia: "NVIDIA_API_KEY",
    deepseek: "DEEPSEEK_API_KEY",
    google: "GEMINI_API_KEY",
    ...
};

export function getEnvApiKey(provider: string, env?) {
    const envKeys = findEnvKeys(provider, env);
    if (provider === "anthropic") {
        // 区分 AUTH_TOKEN (OAuth-like) vs API_KEY (sk-ant-*)，AUTH_TOKEN 不能当 API key
        const apiKeyEnv = envKeys.find(key => key !== ANTHROPIC_AUTH_TOKEN_ENV);
        if (apiKeyEnv) return getProviderEnvValue(apiKeyEnv, env);
    }
    ...
}
```

**范式要点**：
1. **provider → env var 映射表**统一管理
2. **Anthropic 特殊**：区分 `AUTH_TOKEN` 与 `API_KEY`
3. **Vertex/Bedrock**：返回 `<authenticated>` 哨兵而非真实 key

---

## 5. 维度 3：OAuth 2.0 流程

### 5.1 5 种 grant_type 横向对比

| 工程 | Authorization Code | PKCE | Device Code | Refresh Token | Client Credentials |
|------|--------------------|------|-------------|---------------|--------------------|
| **atomcode** | ✅ broker /auth/login | ✅ | ✅ | ✅ | ❌ |
| **claudecode** | ✅ claude.com OAuth | ✅ | ❌ | ✅ | ❌ |
| **Switchyard** | ❌ | ❌ | ❌ | ❌ | ❌ |
| **cc-switch** | ✅ Codex/xai/copilot | ✅ | ⚠️ | ✅ | ❌ |
| **agent-studio** | ✅ JWT + refresh | ⚠️ | ❌ | ✅ | ✅ |
| **openclaw** | ✅ xai OAuth | ✅ | ✅ | ✅ | ⚠️ |
| **deepseek-harness** | ✅ pi-ai grant | ✅ | ⚠️ | ✅ | ❌ |
| **pi** | ✅ 7 providers | ✅ S256 | ✅ RFC 8628 | ✅ | ❌ |

### 5.2 atomcode OAuth broker 范本（`lib.rs:600-735`）

```rust
pub fn start_login() -> Result<LoginSession> { /* POST /auth/login */ }
pub fn login(tel: Option<&Arc<Telemetry>>) -> Result<AuthInfo> {
    let session = start_login()?;
    println!("  Browser didn't open? Open the URL below ...");
    let cbreak = CbreakGuard::new();
    if cbreak.is_some() { println!("  Press ESC to cancel"); }
    let poll_rx = session.spawn_poller(Duration::from_secs(2));  // detached thread
    loop {
        match poll_rx.try_recv() {
            Ok(Ok(Authorized)) => break,
            ...
        }
        match wait_for_esc_or_timeout(&cbreak, Duration::from_millis(100)) {
            EscOutcome::Cancelled => anyhow::bail!("login cancelled by user"),
            EscOutcome::Timeout | EscOutcome::OtherInput => {}
        }
    }
    session.finish(tel)
}
```

**范式要点**：
1. **broker 模式**：本地起 HTTP server，浏览器回调
2. **CbreakGuard + detached thread**：浏览器回调期间 TUI 不冻结
3. **ESC 取消**：cbreak mode 监听按键

### 5.3 pi PKCE 范本（`pkce.ts:1-35`）

```typescript
export async function generatePKCE(): Promise<{ verifier: string; challenge: string }> {
    const verifierBytes = new Uint8Array(32); // 256 bit熵
    crypto.getRandomValues(verifierBytes);
    const verifier = base64urlEncode(verifierBytes);
    const encoder = new TextEncoder();
    const hashBuffer = await crypto.subtle.digest("SHA-256", encoder.encode(verifier));
    const challenge = base64urlEncode(new Uint8Array(hashBuffer));
    return { verifier, challenge };
}
```

**范式要点**：
1. **Web Crypto API** 浏览器+Node 双兼容
2. **S256** challenge method（不传 plain verifier）
3. **32 字节 = 256 bit** 熵

### 5.4 pi Device Code 范本（`device-code.ts:1-99`）

```typescript
const MINIMUM_INTERVAL_MS = 1000;
const DEFAULT_POLL_INTERVAL_SECONDS = 5;
const SLOW_DOWN_INTERVAL_INCREMENT_MS = 5000;

while (Date.now() < deadline) {
    if (options.signal.aborted) throw new Error(CANCEL_MESSAGE);
    const result = await options.poll();
    if (result.status === "slow_down") {
        // 信任 server interval；否则按 RFC +5s
        intervalMs = typeof result.intervalSeconds === "number" && result.intervalSeconds > 0
            ? Math.max(MINIMUM_INTERVAL_MS, Math.floor(result.intervalSeconds * 1000))
            : Math.max(MINIMUM_INTERVAL_MS, intervalMs + SLOW_DOWN_INTERVAL_INCREMENT_MS);
    }
    await abortableSleep(Math.min(intervalMs, remainingMs), options.signal, CANCEL_MESSAGE);
}
```

**范式要点**：
1. **RFC 8628 §3.5 slow_down** 标准 +5s
2. **abortable sleep**：取消信号立即响应
3. **deadline** 防卡死

---

## 6. 维度 4：凭证泄露防御

### 6.1 4 类防御范式

#### 6.1.1 Debug 永远 REDACTED（Switchyard 范本）

`backend.rs:61-72`：
```rust
impl fmt::Debug for HttpBackendConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpBackendConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))  // 永远不打印真实值
            .field("forward_auth", &self.forward_auth)
            .field("extra_header_names", &self.extra_headers.keys())  // 只打印 key 名
            .field("extra_body_keys", &self.extra_body.keys())
            .field("max_retries", &self.max_retries)
            .finish()
    }
}
```

#### 6.1.2 HTTP Header Sensitive 标记（Switchyard 范本）

`backend.rs:215-241`：
```rust
fn sensitive_header(value: &HeaderValue) -> HeaderValue {
    let mut v = value.clone();
    v.set_sensitive(true);  // 标记 reqwest header 在 Debug 时隐藏
    v
}
```

#### 6.1.3 90+ token 前缀正则（openclaw 范本）

`redact-patterns.ts:130-249`：
```typescript
String.raw`\b(sk-[A-Za-z0-9_-]{8,})\b`        // OpenAI
String.raw`(ghp_[A-Za-z0-9]{10,})`             // GitHub PAT
String.raw`(xox[baprs]-[A-Za-z0-9-]{10,})`     // Slack
String.raw`(AIza[0-9A-Za-z\-_]{20,})`          // Google
String.raw`(ya29\.[0-9A-Za-z_\-./+=]{10,})`    // Google OAuth
String.raw`(eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})`  // JWT
String.raw`(pplx-[A-Za-z0-9_-]{10,})`          // Perplexity
String.raw`(AKIA[A-Z0-9]{16})`                 // AWS
String.raw`(xai-[A-Za-z0-9]{30,})`             // xAI
String.raw`(CFPAT-[A-Za-z0-9_\-]{40,})`        // Claude Code PAT
```

#### 6.1.4 macOS Keychain hex 编码（claudecode 范本）

`auth.ts:1094-1160`：
```typescript
const command = `add-generic-password -U -a "${username}" -s "${storageServiceName}" -X "${hexValue}"\n`;
await execa('security', ['-i'], { input: command, reject: false });
```

**范式要点**：
- `security -i -X` 用 hex 编码（避免 ps 命令行窥探）
- `-U` 强制覆盖已存在的 key

### 6.2 laew 的 mask_key 现状

`src/tui/theme.rs`：
```rust
pub fn mask_key(key: &str) -> String {
    if key.len() > 8 {
        let prefix: String = key.chars().take(4).collect();
        let suffix: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
        format!("{prefix}...{suffix}")
    } else {
        "***".to_string()
    }
}
```

**L51 gap**：仅末4位脱敏，缺：
1. 已知 token 前缀正则（OpenAI sk-/Anthropic sk-ant-）
2. UTF-16 安全（emoji 密钥）
3. argv CLI 参数脱敏
4. error message 自动脱敏

---

## 7. 维度 5：跨进程 / 并发安全

### 7.1 5 种锁实现对比

| 工程 | 锁类型 | 跨进程 | 跨平台 | 重入安全 | 防并发 refresh |
|------|--------|--------|--------|---------|---------------|
| **atomcode** | `fs2::FileExt::lock_exclusive` | ✅ | ⚠️ Unix 优先 | ✅ advisory | ✅ |
| **claudecode** | `proper-lockfile.lock` | ✅ | ✅ | ⚠️ | ✅ Map dedup |
| **Switchyard** | ❌ 单进程 | - | - | - | - |
| **cc-switch** | SQLite WAL | ✅ | ✅ | ✅ | ⚠️ |
| **agent-studio** | SQLite | ✅ | ✅ | ✅ | ⚠️ |
| **openclaw** | ❌ 单进程 | - | - | - | - |
| **deepseek-harness** | `modifyRecord` 串行化 | ⚠️ | ⚠️ | ✅ | ✅ |
| **pi** | ❌ 单进程 | - | - | - | - |

### 7.2 atomcode 范本（`lib.rs:1343-1363`）

```rust
fn refresh_auth_if_current(rejected: &str, expected: Option<&str>) -> Result<AuthInfo> {
    with_auth_lock(|| {  // fs2::FileExt::lock_exclusive
        let auth = get_stored_auth()?;
        if let Some(expected) = expected_user_id {
            if auth.user.id != expected { anyhow::bail!("Login account changed..."); }
        }
        if auth.access_token != rejected_access_token { return Ok(auth); }  // someone else refreshed
        refresh_access_token_unlocked(&auth)
    })
}
```

**范式要点**：
1. **fs2 advisory lock**（非阻塞可重试）
2. **CAS 校验**：进入锁后检查 access_token 是否还是被拒绝的那个（防止别人已刷新）
3. **expected_user_id** 防止切换账号时误刷新

### 7.3 claudecode 范本（`auth.ts:1343-1434`）

```typescript
const pending401Handlers = new Map<string, Promise<boolean>>();  // in-flight dedup

export async function checkAndRefreshOAuthTokenIfNeededImpl(retryCount, force) {
    const MAX_RETRIES = 5;
    await invalidateOAuthCacheIfDiskChanged();   // cross-process mtime watch
    ...
    if ((err as any).code === 'ELOCKED') {
        if (retryCount < MAX_RETRIES) {
            await sleep(1000 + Math.random() * 1000);  // jitter
            return checkAndRefreshOAuthTokenIfNeededImpl(retryCount + 1, force);
        }
    }
    release = await lockfile.lock(claudeDir);  // proper-named-lockfile
    ...
}
```

**范式要点**：
1. **Map<token, Promise>** dedup 同 token 并发 401
2. **proper-lockfile** 跨进程互斥（npm 包）
3. **mtime watch** 检测 disk 文件变更
4. **jitter 退避**：1000-2000ms 随机

### 7.4 laew 现状（L53 gap）

`src/config/mod.rs` 的 `Db::set_active`：
```rust
pub fn set_active(&self, provider_id: i64) -> Result<()> {
    let tx = self.conn.unchecked_transaction()?;
    // SQLite WAL 多读单写，原子更新 is_active
    ...
}
```

**已有保护**：SQLite 事务 + WAL 模式提供基本并发安全。

**缺失**：
- 无 token-level CAS（refresh 时检查 token 是否还匹配）
- 无 `Map<token, Promise>` dedup（同一 token 多窗口并发 401 会重复刷新）
- 无 fs2 advisory lock 文件级保护

---

## 8. 维度 6：多 OAuth Provider 支持

### 8.1 pi 的 7 providers 范本（`packages/ai/src/auth/oauth/`）

| Provider | File | 协议 | 特殊点 |
|----------|------|------|--------|
| **Anthropic** | `anthropic.ts` | PKCE + 本地回调 | Claude Pro/Max 订阅 |
| **OpenAI Codex** | `openai-codex.ts` | PKCE + 本地回调 | ChatGPT Plus/Pro |
| **GitHub Copilot** | `github-copilot.ts` | Device Code | GitHub 登录 |
| **OpenRouter** | `openrouter.ts` | PKCE | 多模型聚合 |
| **Kimi Coding** | `kimi-coding.ts` | PKCE | Moonshot |
| **xAI** | `xai.ts` | PKCE + Device Code | Grok |
| **Radius** | `radius.ts` | PKCE | 自定义 |

### 8.2 claudecode OAuth FedStart allowlist（`constants/oauth.ts:84-104`）

```typescript
const ALLOWED_OAUTH_BASE_URLS = [
    'https://beacon.claude-ai.staging.ant.dev',
    'https://claude.fedstart.com',
    'https://claude-staging.fedstart.com',
]
```

**范式要点**：
- **白名单**：仅 4 个受信 OAuth endpoint
- **环境隔离**：staging vs prod 严格分离

### 8.3 laew 现状

- 当前**仅 API Key 模式**，无 OAuth 支持。
- `src/llm/{anthropic,openai}.rs` 仅 `Authorization: Bearer {api_key}` / `x-api-key: {api_key}`。

**L50 gap 修复路径**：
1. 短期：保留 API Key 模式，加 `--api-key` CLI flag 优先级
2. 中期：OAuth 流程用 `oauth2 = "4"` crate + 本地回调 server + PKCE
3. 长期：多 provider（Anthropic/OpenAI/Google）OAuth 统一抽象

---

## 9. 维度 7：客户端验证

### 9.1 格式校验范式

#### 9.1.1 agent-studio provider-specific（`security_utils.py:362-416`）

```python
elif provider == "openai":
    if not api_key.startswith("sk-"):
        result["valid"] = False
        result["errors"].append("OpenAI API key must start with 'sk-'")
    elif len(api_key) < 20: result["valid"] = False
elif provider == "anthropic":
    if not api_key.startswith("sk-ant-"): result["valid"] = False

# 测试/demo key 警告
test_patterns = ["test", "demo", "example", "fake", "dummy"]
if any(pattern in api_key.lower() for pattern in test_patterns):
    result["warnings"].append("API key appears to be a test/demo key")
```

#### 9.1.2 deepseek-harness ASCII 强制（`api-key.ts:15-41`）

```typescript
const LEGAL_API_KEY = /^[\x21-\x7E]+$/

export function normalizeApiKey(raw: string): ApiKeyCheck {
    const value = raw.trim();
    if (value.length === 0) return { ok: false, reason: 'empty' };
    if (!LEGAL_API_KEY.test(value)) return { ok: false, reason: 'illegalCharacters' };
    return { ok: true, value };
}
```

**范式要点**：`fetch` 不接受非可打印 ASCII 的 header value，提前过滤避免运行时失败。

### 9.2 laew 现状

`src/config/mod.rs` 的 CRUD **无格式校验**。

**修复路径**：
1. 加 provider-specific 前缀校验（OpenAI `sk-`、Anthropic `sk-ant-`）
2. 加 ASCII 强制（参考 deepseek-harness）
3. 加 test/demo key 警告

---

## 10. 维度 8：环境变量优先级

### 10.1 pi 多 env 映射（`env-api-keys.ts:78-120`）

```typescript
export function getEnvApiKey(provider: string, env?): string | undefined {
    const envKeys = findEnvKeys(provider, env);
    if (envKeys?.[0]) {
        const apiKeyEnv = provider === "anthropic"
            ? envKeys.find(key => key !== ANTHROPIC_AUTH_TOKEN_ENV)
            : envKeys[0];
        if (apiKeyEnv) return getProviderEnvValue(apiKeyEnv, env);
    }
    // Vertex: gcloud ADC; AWS Bedrock: AWS_PROFILE/IAM/IRSA/ECS/task role
    if (provider === "google-vertex") return "<authenticated>";
    if (provider === "amazon-bedrock") {
        if (AWS_PROFILE || (AWS_ACCESS_KEY_ID && AWS_SECRET_ACCESS_KEY) ||
            AWS_BEARER_TOKEN_BEDROCK || AWS_CONTAINER_CREDENTIALS_RELATIVE_URI ||
            AWS_CONTAINER_CREDENTIALS_FULL_URI || AWS_WEB_IDENTITY_TOKEN_FILE)
            return "<authenticated>";
    }
}
```

**范式要点**：
- Vertex/Bedrock 走 ADC/IAM，不直接读 key
- 返回 `<authenticated>` 哨兵而非真实 key

### 10.2 claudecode 6 层优先级（`auth.ts:153-206`）

```
1. ANTHROPIC_AUTH_TOKEN env (OAuth-like)
2. CLAUDE_CODE_OAUTH_TOKEN env
3. OAuth Token from FD (CCR/CCD)
4. apiKeyHelper (project/local settings)
5. Claude.ai OAuth tokens (keychain)
6. ANTHROPIC_API_KEY env (API key)
```

---

## 11. 维度 9：审计与监控

### 11.1 4 类审计范式

| 类型 | 工程 | 实现 |
|------|------|------|
| **决策审计** | claudecode | `logEvent('tengu_apiKeyHelper_missing_trust11')` analytics event |
| **使用频率** | cc-switch | `services/usage_stats.rs` 聚合 |
| **失败告警** | Switchyard | OTEL span `error` field |
| **离群检测** | ❌ 无 | - |

### 11.2 laew 现状

- 无审计（仅 stderr 错误日志）
- 无使用频率统计
- 无离群使用检测

**L52 修复路径**：
1. 加 SQLite `provider_usage` 表（每次请求写一行）
2. 加 OTLP span（参考第八轮 T1 Telemetry）
3. 加失败告警规则

---

## 12. 横向大表：8 工程 × 9 维度

| 工程 × 维度 | 存储 | 多账号 | OAuth | 脱敏 | 跨进程 | Provider 验证 | 优先级 | 审计 | 测试 |
|------------|------|--------|-------|------|--------|--------------|--------|------|------|
| **atomcode** | 🟢 0o600 | 🔴 | 🟢 broker | 🟡 | 🟢 fs2 | 🟡 | 🟡 | 🟡 | 🟢 |
| **claudecode** | 🟢 Keychain | 🟢 6 源 | 🟢 FedStart | 🟢 security -X | 🟢 lockfile | 🟡 | 🟢 6 层 | 🟢 analytics | 🟢 |
| **Switchyard** | 🟢 env 引用 | 🟢 N client | 🔴 | 🟢 REDACTED | 🔴 | 🔴 | 🟡 | 🟡 | 🟢 |
| **cc-switch** | 🟡 SQLite 明文 | 🟢 6 strategy | 🟢 3 反代 | 🟢 masked_key | 🟢 SQLite | 🟡 | 🟢 live 切换 | 🟡 usage_events | 🟢 |
| **agent-studio** | 🟢 AES-GCM+KMS | 🟢 多租户 | 🟢 JWT | 🟢 后4 位 | 🟢 SQLite | 🟢 provider 校验 | 🟡 | 🟡 logger | 🟢 |
| **openclaw** | 🟢 SecretRef | 🟢 抽象 | 🟢 xAI/PKCE | 🟢 90+ 正则 | 🔴 | 🟡 | 🟢 env shorthand | 🟡 | 🟢 |
| **deepseek-harness** | 🟢 service 抽象 | 🟢 providerId | 🟢 pi-ai | 🟡 | 🟡 modifyRecord | 🟢 ASCII | 🟡 | 🟡 | 🟢 |
| **pi** | 🟡 config 文件 | 🟢 7 providers | 🟢 RFC 标准 | 🟡 哨兵 | 🔴 | 🟡 | 🟢 env 映射 | 🟡 | 🟢 |
| **laew** | 🔴 明文 | 🔴 | 🔴 | 🟡 末4位 | 🟡 SQLite | 🔴 | 🟡 | 🔴 | 🟡 |

> 🟢=已实现，🟡=部分实现，🔴=缺失

---

## 13. 设计模式提炼（5 条）

### 13.1 模式 D1：atomic write + 0o600（atomcode 范本）

```rust
let tmp_path = temp_auth_path(path);
let mut file = OpenOptions::new()
    .create_new(true).write(true).truncate(true).mode(0o600)
    .open(&tmp_path)?;
file.write_all(content.as_bytes())?;
file.sync_all()?;
drop(file);
std::fs::rename(&tmp_path, path)?;  // atomic rename
std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
```

**laew 应用**：`src/config/mod.rs::Db::write` 加 atomic write。

---

### 13.2 模式 D2：Debug 永远 REDACTED（Switchyard 范本）

```rust
.field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
```

**laew 应用**：`ProviderRecord` 实现 `Debug` 时 `api_key` 字段输出 `[REDACTED]`。

---

### 13.3 模式 D3：token prefix regex catalog（openclaw 范本）

```typescript
String.raw`\b(sk-[A-Za-z0-9_-]{8,})\b`           // OpenAI
String.raw`(ghp_[A-Za-z0-9]{10,})`                // GitHub
String.raw`(xox[baprs]-[A-Za-z0-9-]{10,})`        // Slack
...
```

**laew 应用**：`src/error.rs` 加 `redact_message(msg)` 函数，对所有错误信息走正则 catalog 脱敏。

---

### 13.4 模式 D4：Map<token, Promise> 401 dedup（claudecode 范本）

```typescript
const pending401Handlers = new Map<string, Promise<boolean>>();
```

**laew 应用**：未来 OAuth 流程时，同 token 多窗口并发 401 共享一个 refresh Promise。

---

### 13.5 模式 D5：CAS 校验防止误刷新（atomcode 范本）

```rust
if auth.access_token != rejected_access_token { return Ok(auth); }  // someone else refreshed
```

**laew 应用**：`refresh_token` 时校验当前 token 是不是被拒绝的那个，避免重复刷新。

---

## 14. 反模式警示（3 条）

### 14.1 反模式 A1：明文 token 写日志

```rust
// ❌ 反模式
log::info!("refreshed token: {}", new_access_token);
```

**正确**：用 `mask_key()` 或 REDACTED 哨兵。

### 14.2 反模式 A2：env var 名写日志

```typescript
// ❌ 反模式
console.error("API key env var missing:", process.env.MY_API_KEY);
```

**正确**：只打印变量名，不打印变量值。

### 14.3 反模式 A3：未授权 API Key Helper 执行

```typescript
// ❌ 反模式
const apiKey = execSync(apiKeyHelper);  // 不校验 workspace trust
```

**正确**（claudecode 范本）：先 `checkHasTrustDialogAccepted()`。

---

## 15. laew 现状评估（L49-L53 五个 gap）

### 15.1 L49：API Key 明文存储（紧急度 P0）

**现状**：`src/config/mod.rs::ProviderRecord.api_key` 直接存明文到 SQLite。

**修复**：
1. DB 文件加 0o600。
2. 短期：`mask_key()` 在 UI/日志层脱敏。
3. 中期：SQLCipher 加密（`rusqlite` + `rusqlite/sqlite-cipher` feature）。
4. 长期：OS Keyring（`keyring` crate）+ 内存解密。

```rust
// src/config/mod.rs
#[cfg(unix)]
fn ensure_db_permissions() -> Result<()> {
    let path = Db::path()?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}
```

---

### 15.2 L50：无 OAuth 流程（紧急度 P1）

**现状**：仅 API Key 模式。

**修复**：
1. Cargo.toml 加 `oauth2 = "4"` crate。
2. `src/auth/oauth.rs` 新增模块。
3. 本地回调 server（用 `tiny_http` 或 `axum`）+ PKCE。
4. 多 provider（Anthropic/OpenAI）OAuth 抽象。

---

### 15.3 L51：脱敏范围不足（紧急度 P1）

**现状**：仅末4位 `mask_key`。

**修复**：
1. 加 `redact_message()` 走 token 前缀正则 catalog（参考 openclaw）。
2. error message 自动脱敏。
3. argv CLI 参数自动脱敏。
4. UTF-16 安全（emoji 密钥）。

---

### 15.4 L52：无多账号轮换（紧急度 P1）

**现状**：单账号，单 `is_active`。

**修复**：
1. SQLite `providers` 表加 `priority` + `weight` 字段。
2. 加轮询逻辑（`weighted_round_robin`）。
3. 加配额管理（429 退避 + circuit breaker）。

---

### 15.5 L53：无跨进程文件锁（紧急度 P2）

**现状**：SQLite WAL 提供基本并发安全，但无 token-level CAS。

**修复**：
1. 加 `Map<token, Promise>` dedup（同 token 多 401 共享 refresh）。
2. CAS 校验（refresh 时检查 token 是否还匹配）。
3. fs2 advisory lock（`fs2 = "0.4"` crate）。

---

## 16. 附录

### 16.1 参考文件清单（绝对路径）

#### atomcode
- `crates/atomcode-auth/src/lib.rs:25-92` — `write_auth_file_secure` 0o600 + atomic rename
- `crates/atomcode-auth/src/lib.rs:600-735` — OAuth broker + CbreakGuard
- `crates/atomcode-auth/src/lib.rs:1343-1363` — `refresh_auth_if_current` 跨进程锁
- `crates/atomcode-auth/src/lib.rs:1365-1399` — `get_valid_auth_info` 5 分钟 TTL
- `crates/atomcode-auth/src/gateway_crypto.rs:9-89` — `RequestSigner` trait

#### claudecode
- `src/utils/auth.ts:1094-1160` — macOS Keychain `security -i -X`
- `src/utils/auth.ts:1343-1434` — 401 dedup `Map<token, Promise>`
- `src/utils/auth.ts:153-206` — 6 层 token source 优先级
- `src/utils/auth.ts:546-556` — apiKeyHelper trust gate
- `src/constants/oauth.ts:84-104` — FedStart allowlist

#### Switchyard
- `crates/switchyard-runner/src/config.rs:405-489` — `api_key_env` TOML 引用
- `crates/libsy-llm-client/src/backend.rs:61-72` — Debug REDACTED
- `crates/libsy-llm-client/src/backend.rs:215-241` — `set_sensitive(true)` header
- `crates/libsy-llm-client/src/backend.rs:88-114` — extra_headers 拒绝覆盖 auth header

#### cc-switch
- `src-tauri/src/proxy/providers/auth.rs:8-79` — `AuthInfo` + `masked_key()`
- `src-tauri/src/proxy/providers/auth.rs:85-130` — 6 种 `AuthStrategy`
- `src-tauri/src/lib.rs:136-177` — `redact_known_secrets`
- `src-tauri/src/commands/xai_oauth.rs:29-58` — 多账号 + `default_account_id()`

#### agent-studio
- `backend/openjiuwen_studio/core/manager/model_manager/utils/security_utils.py:42-283` — AES-GCM + HKDF
- `backend/openjiuwen_studio/core/manager/model_manager/utils/security_utils.py:286-305` — `mask_api_key`
- `backend/openjiuwen_studio/core/manager/model_manager/utils/security_utils.py:362-416` — provider 校验
- `backend/openjiuwen_studio/core/manager/login_manager/session_auth.py:20-95` — JWT + PBKDF2

#### openclaw
- `src/config/types.secrets.ts:6-21` — `SecretRef` 抽象 4 后端
- `src/security/secret-mask.ts:1-29` — UTF-16 safe `maskApiKey`
- `src/logging/redact-patterns.ts:130-249` — 90+ token 前缀正则
- `src/config/redact-argv.ts:6-44` — argv suffix 自动脱敏
- `extensions/xai/xai-oauth.ts:26-100` — xAI OAuth device-code

#### deepseek-harness
- `packages/llm/llm/src/api-key.ts:15-41` — ASCII 强制校验
- `packages/llm/llm-pi-ai/src/auth.ts:30-99` — Service 抽象 + RECORD_SCOPE
- `packages/llm/llm-pi-ai/src/auth.ts:141-186` — modifyRecord 串行化

#### pi
- `packages/ai/src/auth/oauth/pkce.ts:1-35` — Web Crypto PKCE S256
- `packages/ai/src/auth/oauth/device-code.ts:1-99` — RFC 8628 device code
- `packages/ai/src/auth/oauth/anthropic.ts:30-312` — Claude Pro/Max PKCE
- `packages/ai/src/env-api-keys.ts:78-120` — 多 provider env 映射

#### laew
- `src/config/mod.rs` — `Db` SQLite CRUD + `ProviderRecord` 明文
- `src/tui/theme.rs` — `mask_key` 末4位
- `src/llm/anthropic.rs`、`src/llm/openai.rs` — 双协议客户端（无 OAuth）
- `src/error.rs` — `AgentError`（3 variant）

### 16.2 术语表

| 术语 | 含义 |
|------|------|
| **OAuth** | 开放授权协议，第三方应用代表用户访问资源 |
| **PKCE** | Proof Key for Code Exchange（RFC 7636），OAuth 2.0 安全增强 |
| **Device Code** | OAuth 2.0 设备流（RFC 8628），无浏览器设备使用 |
| **Authorization Code** | OAuth 2.0 标准授权码模式 |
| **Keyring** | OS 凭证存储（macOS Keychain / Linux Secret Service / Windows Credential Manager） |
| **AES-GCM** | AES Galois/Counter Mode，对称加密 + 完整性校验 |
| **HKDF** | HMAC-based Key Derivation Function |
| **KMS** | Key Management Service（如 AWS KMS、阿里云 KMS） |
| **JWT** | JSON Web Token |
| **PBKDF2** | Password-Based Key Derivation Function 2 |
| **CSRF** | Cross-Site Request Forgery |
| **mask_key** | API Key 脱敏显示 |
| **fs2** | Rust crate，提供文件 advisory lock |
| **WAL** | Write-Ahead Log（详见第八轮 T2） |

### 16.3 与第八轮的关系

| 维度 | 第八轮 T1（Telemetry） | 第八轮 T5（Hook / Plugin） | 第九轮 T3（本专题） |
|------|----------------------|--------------------------|-------------------|
| 关注点 | 决策审计格式 | 拦截器注册 | OAuth / Keyring / 凭证泄露 |
| 紧急度 | P1 | P1 | P0/P1 |
| Rust crate | tracing + opentelemetry | extism / schemars | keyring / oauth2 / rusqlite + sqlcipher |
| 互补点 | 失败决策需 telemetry | PreToolUse 拦截 OAuth | audit log 需持久化 |

---

## 17. 结语

8 工程调研后，我们看到 OAuth 与凭证管理是 laew **从「能用」升级到「生产可用」** 的关键短板：

- **L49 明文存储**是 P0 中最容易触发的（任何能访问 SQLite 的人都能拿到所有 API Key）。
- **L50 无 OAuth** 让 laew 用户被迫依赖外部 Provider 的 API Key 管理，无法用 Claude Pro/Max 订阅。
- **L51 脱敏不足** 在企业场景下尤其危险（错误日志可能泄露凭证）。
- **L52-L53** 是企业级多账号管理的基石。

**一句话总结**：「**OS Keyring + AES-GCM 加密 + token 前缀正则脱敏 + Map<token, Promise> dedup**」是 8 工程的最小公共子集，laew 应优先落地这四条范式。

---

**字数统计**：~12,500 字，~1,280 行。
**调研时间**：2026-09-07
**作者**：第九轮 T3 专题研究 SubAgent（主笔 + Explore SubAgent 数据采集）