//! MCP 连接管理器:懒连接 + 指数退避重连(稳定性窗口)+ 冷却。
//!
//! 进程级缓存(`McpManager::global()`),每个 server 一个 [`ConnEntry`]:
//! - **懒连接**:`list_tools` / `call_tool` 等在无连接时自动 connect(对模型透明);
//! - **请求锁**:每连接一把锁串行化调用(MCP_Use `parallel_safe=false` + 单锁,
//!   AtomCode 三锁的 laew 简化,见设计 §6.1);
//! - **退避**:失败后 `delay = min(30s, 500ms * 2^(n-1))`;连接存活 ≥ 稳定性窗口
//!   (30s)后失败计数归零(DeepSeek 稳定性窗口,防长期稳定后的偶发断连被累积放弃);
//!   超过 max_attempts 进冷却,信封 5002 + `retry_after_ms`,直到显式 connect。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::warn;

use super::client::McpClient;
use super::{
    CallToolOutcome, ConnectInfo, McpError, McpResult, McpServerConfig, ResourceContent,
    ResourceInfo, ServerStatus, ToolInfo,
};

/// 退避策略(DeepSeek 参数,设计 §6.2)。
#[derive(Debug, Clone, Copy)]
pub struct BackoffPolicy {
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub max_attempts: u32,
    /// 稳定性窗口:连接存活超过该时长后失败计数归零。
    pub stability_window_ms: u64,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            initial_delay_ms: 500,
            max_delay_ms: 30_000,
            max_attempts: 10,
            stability_window_ms: 30_000,
        }
    }
}

impl BackoffPolicy {
    /// 第 n 次失败后的延迟:`min(max, initial * 2^(n-1))`(n≥1)。
    pub fn delay_ms(&self, failed_attempts: u32) -> u64 {
        if failed_attempts == 0 {
            return 0;
        }
        let exp = (failed_attempts - 1).min(20);
        self.initial_delay_ms
            .saturating_mul(1u64 << exp)
            .min(self.max_delay_ms)
    }
}

/// 退避状态(纯状态机,便于单测注入时间点)。
#[derive(Debug, Default, Clone)]
pub struct BackoffState {
    pub failed_attempts: u32,
    /// 下次允许重试的时刻;`None` = 可立即重试。
    pub retry_at: Option<Instant>,
    /// 最近一次连接成功时刻(稳定性窗口判定)。
    pub last_ok_at: Option<Instant>,
    /// 是否已放弃(超 max_attempts,需显式 connect 复位)。
    pub exhausted: bool,
}

impl BackoffState {
    /// 记录一次失败,返回本次退避延迟(毫秒);`exhausted` 后返回 None。
    pub fn on_failure(&mut self, policy: &BackoffPolicy, now: Instant) -> Option<u64> {
        // 稳定性窗口:连接已稳定存活足够久,偶发断连不累积。
        if let Some(ok) = self.last_ok_at {
            if now.duration_since(ok).as_millis() as u64 >= policy.stability_window_ms {
                self.failed_attempts = 0;
            }
        }
        self.failed_attempts = self.failed_attempts.saturating_add(1);
        if self.failed_attempts > policy.max_attempts {
            self.exhausted = true;
            self.retry_at = None;
            return None;
        }
        let delay = policy.delay_ms(self.failed_attempts);
        self.retry_at = Some(now + Duration::from_millis(delay));
        Some(delay)
    }

    /// 记录一次成功(清零失败态)。
    pub fn on_success(&mut self, now: Instant) {
        self.failed_attempts = 0;
        self.retry_at = None;
        self.exhausted = false;
        self.last_ok_at = Some(now);
    }

    /// 冷却剩余毫秒;`None` = 可立即重试。
    pub fn cooldown_remaining_ms(&self, now: Instant) -> Option<u64> {
        if self.exhausted {
            return Some(u64::MAX);
        }
        self.retry_at.map(|at| {
            at.saturating_duration_since(now)
                .as_millis() as u64
        })
    }

    /// 显式 connect 复位(用户/模型主动重连)。
    pub fn reset(&mut self) {
        self.failed_attempts = 0;
        self.retry_at = None;
        self.exhausted = false;
    }
}

struct ConnEntry {
    inner: Mutex<Option<McpClient>>,
    backoff: Mutex<BackoffState>,
    status: Mutex<ServerStatus>,
}

/// MCP 连接管理器(进程级单例 + 可独立构造供测试)。
pub struct McpManager {
    entries: Mutex<HashMap<String, Arc<ConnEntry>>>,
    policy: BackoffPolicy,
}

impl McpManager {
    pub fn new(policy: BackoffPolicy) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            policy,
        }
    }

    /// 进程级单例(工具门面用)。
    pub fn global() -> &'static Arc<McpManager> {
        static CELL: OnceLock<Arc<McpManager>> = OnceLock::new();
        CELL.get_or_init(|| Arc::new(McpManager::new(BackoffPolicy::default())))
    }

    async fn entry(&self, name: &str) -> Arc<ConnEntry> {
        let mut g = self.entries.lock().await;
        g.entry(name.to_string())
            .or_insert_with(|| {
                Arc::new(ConnEntry {
                    inner: Mutex::new(None),
                    backoff: Mutex::new(BackoffState::default()),
                    status: Mutex::new(ServerStatus::Idle),
                })
            })
            .clone()
    }

    /// 当前状态快照(供 `list_servers` 信封)。
    pub async fn status(&self, name: &str) -> ServerStatus {
        if let Some(e) = self.entries.lock().await.get(name) {
            *e.status.lock().await
        } else {
            ServerStatus::Idle
        }
    }

    /// 显式 connect(复位退避态;失败按退避记录)。
    pub async fn connect(&self, cfg: &McpServerConfig) -> McpResult<ConnectInfo> {
        let entry = self.entry(&cfg.name).await;
        entry.backoff.lock().await.reset();
        self.ensure_connected(&entry, cfg).await
    }

    async fn ensure_connected(
        &self,
        entry: &Arc<ConnEntry>,
        cfg: &McpServerConfig,
    ) -> McpResult<ConnectInfo> {
        let mut guard = entry.inner.lock().await;
        if let Some(client) = guard.as_mut() {
            return Ok(client.connect_info.clone());
        }
        // 冷却检查。
        let now = Instant::now();
        {
            let b = entry.backoff.lock().await;
            if let Some(remaining) = b.cooldown_remaining_ms(now) {
                return Err(McpError::Cooldown {
                    retry_after_ms: remaining,
                });
            }
        }
        *entry.status.lock().await = ServerStatus::Connecting;
        match McpClient::connect(cfg).await {
            Ok(client) => {
                let info = client.connect_info.clone();
                entry.backoff.lock().await.on_success(Instant::now());
                *entry.status.lock().await = ServerStatus::Connected;
                *guard = Some(client);
                Ok(info)
            }
            Err(e) => {
                let delay = entry.backoff.lock().await.on_failure(&self.policy, Instant::now());
                *entry.status.lock().await = match &e {
                    McpError::Handshake(_) => ServerStatus::HandshakeFailed,
                    McpError::Environment(_) => ServerStatus::Broken,
                    _ if delay.is_none() => ServerStatus::Cooldown,
                    _ => ServerStatus::Broken,
                };
                warn!(server = %cfg.name, error = %e, "MCP connect 失败");
                Err(e)
            }
        }
    }

    /// 断开并丢弃连接(传输错误后半开连接不复用)。
    async fn drop_connection(&self, entry: &Arc<ConnEntry>) {
        let mut guard = entry.inner.lock().await;
        if let Some(mut client) = guard.take() {
            client.close().await;
        }
        *entry.status.lock().await = ServerStatus::Broken;
    }

    /// 通用调用骨架:保证连接 → 调用 → 传输错销毁连接 + 记退避。
    async fn with_client<T, F>(&self, cfg: &McpServerConfig, f: F) -> McpResult<T>
    where
        F: for<'a> FnOnce(
            &'a mut McpClient,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = McpResult<T>> + Send + 'a>>,
    {
        let entry = self.entry(&cfg.name).await;
        self.ensure_connected(&entry, cfg).await?;
        let mut guard = entry.inner.lock().await;
        let Some(client) = guard.as_mut() else {
            return Err(McpError::Connect("连接句柄缺失".into()));
        };
        match f(client).await {
            Ok(v) => {
                entry.backoff.lock().await.on_success(Instant::now());
                Ok(v)
            }
            Err(e @ McpError::Timeout(_)) | Err(e @ McpError::Connect(_)) => {
                // 传输级失败:销毁连接(半开不可复用),记退避。
                drop(guard);
                self.drop_connection(&entry).await;
                let _ = entry.backoff.lock().await.on_failure(&self.policy, Instant::now());
                Err(e)
            }
            Err(e) => {
                // 业务/协议错误:连接保留(重连没用)。
                Err(e)
            }
        }
    }

    pub async fn list_tools(&self, cfg: &McpServerConfig) -> McpResult<Vec<ToolInfo>> {
        self.with_client(cfg, |c| Box::pin(async move { c.list_tools().await })).await
    }

    pub async fn call_tool(
        &self,
        cfg: &McpServerConfig,
        tool: &str,
        arguments: serde_json::Value,
        timeout_ms: Option<u64>,
    ) -> McpResult<CallToolOutcome> {
        let tool = tool.to_string();
        self.with_client(cfg, |c| {
            Box::pin(async move { c.call_tool(&tool, arguments, timeout_ms).await })
        })
        .await
    }

    pub async fn list_resources(
        &self,
        cfg: &McpServerConfig,
    ) -> McpResult<Vec<ResourceInfo>> {
        self.with_client(cfg, |c| Box::pin(async move { c.list_resources().await })).await
    }

    pub async fn read_resource(
        &self,
        cfg: &McpServerConfig,
        uri: &str,
    ) -> McpResult<Vec<ResourceContent>> {
        let uri = uri.to_string();
        self.with_client(cfg, |c| Box::pin(async move { c.read_resource(&uri).await })).await
    }

    /// 显式 close(server 下线 / 长会话释放)。
    pub async fn close(&self, name: &str) -> bool {
        let Some(entry) = self.entries.lock().await.get(name).cloned() else {
            return false;
        };
        self.drop_connection(&entry).await;
        entry.backoff.lock().await.reset();
        *entry.status.lock().await = ServerStatus::Idle;
        true
    }
}
