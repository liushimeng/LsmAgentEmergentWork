//! 可取消 LLM 客户端装饰器:让全部角色的 LLM 调用可被 Ctrl-C / SIGINT 即时中断。
//!
//! 设计:orchestrator 构造时把 llm 包一层 [`CancellableLlmClient`],token 通过
//! [`CancelGate`] 在**每个用户任务**开始时注入、结束时清除(句柄守卫保证清理)。
//! 这样 Yolo / Plan / Main-Work / SubAgent / Quality / SessionContext / Compact /
//! Debug 八个角色的 LLM 调用一处包裹、零侵入各 runner。
//!
//! 知识库依据:`docs/Agent源码调研/专题-第五轮-中断取消与后台任务深度分析.md`
//! §3.1(取消原语三家族)+ 第六轮 SubAgent 调度专题 §11.2。
//! 方案:`tmpPlan/2026-09-08_07-取消传播与优雅中断方案.md` §3.3。

use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::agent::cancel::CancelToken;
use crate::config::Protocol;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Completion, LlmClient, RequestMeta, ToolDef};

/// 每任务取消门:内层 token 在任务开始时 set、结束时 clear。
///
/// `CancellationToken` 是一次性原语(不可 reset),而 TUI 一个 Session 内要跑
/// 多个任务,所以不能把 token 固化在 `OrchestratorConfig`(第六轮 §11.2 原方案),
/// 改为经由此门动态注入。
#[derive(Debug, Default)]
pub struct CancelGate {
    inner: RwLock<Option<CancelToken>>,
}

impl CancelGate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 注入当前任务的取消 token(覆盖旧值)。
    pub fn set(&self, token: CancelToken) {
        let mut guard = self.inner.write().expect("cancel gate poisoned");
        *guard = Some(token);
    }

    /// 清除 token(恢复「不可取消」直通语义)。
    pub fn clear(&self) {
        let mut guard = self.inner.write().expect("cancel gate poisoned");
        *guard = None;
    }

    /// 快照当前 token(供 select 使用)。
    fn current(&self) -> Option<CancelToken> {
        self.inner.read().expect("cancel gate poisoned").clone()
    }

    /// 注入 token 并返回自动清理守卫(任务路径持有的唯一句柄)。
    pub fn guard(self: &Arc<Self>, token: CancelToken) -> CancelGateGuard {
        self.set(token);
        CancelGateGuard { gate: self.clone() }
    }
}

/// Drop 时自动 clear 的守卫:任务路径无论正常返回 / 提前 return / panic 展开,
/// gate 都会复位,避免上一次任务的取消状态泄漏到下一次。
pub struct CancelGateGuard {
    gate: Arc<CancelGate>,
}

impl Drop for CancelGateGuard {
    fn drop(&mut self) {
        self.gate.clear();
    }
}

/// 可取消 LLM 客户端:`gate` 无 token 时完全直通(零行为变化);
/// 有 token 时 `complete` 被 `tokio::select!` 包裹,取消即时返回
/// [`AgentError::Cancelled`]——挂起中的 HTTP/SSE 请求 future 被 drop,
/// 连接随之关闭。
pub struct CancellableLlmClient {
    inner: Arc<dyn LlmClient>,
    gate: Arc<CancelGate>,
}

impl CancellableLlmClient {
    pub fn new(inner: Arc<dyn LlmClient>, gate: Arc<CancelGate>) -> Self {
        Self { inner, gate }
    }
}

#[async_trait]
impl LlmClient for CancellableLlmClient {
    async fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
        meta: &RequestMeta,
    ) -> Result<Completion> {
        match self.gate.current() {
            Some(token) => {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => Err(AgentError::Cancelled),
                    r = self.inner.complete(system, messages, tools, meta) => r,
                }
            }
            // 不可取消窗口:直通内层(含 Resilient 重试 / Debug 采集装饰器)
            None => self.inner.complete(system, messages, tools, meta).await,
        }
    }

    fn protocol(&self) -> Protocol {
        self.inner.protocol()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OkLlm;

    #[async_trait]
    impl LlmClient for OkLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            Ok(Completion {
                text: "ok".into(),
                tool_calls: vec![],
                usage: Default::default(),
                stop_reason: None,
            })
        }
        fn protocol(&self) -> Protocol {
            Protocol::Anthropic
        }
    }

    struct HangLlm;

    #[async_trait]
    impl LlmClient for HangLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            std::future::pending().await
        }
        fn protocol(&self) -> Protocol {
            Protocol::Anthropic
        }
    }

    fn test_meta() -> RequestMeta {
        RequestMeta {
            session_id: "test".into(),
            device_id: "d".into(),
        }
    }

    #[tokio::test]
    async fn passthrough_when_gate_empty() {
        let gate = CancelGate::new();
        let client = CancellableLlmClient::new(Arc::new(OkLlm), gate);
        let out = client.complete("", &[], &[], &test_meta()).await.unwrap();
        assert_eq!(out.text, "ok");
    }

    #[tokio::test]
    async fn cancel_returns_promptly_even_if_inner_hangs() {
        let gate = CancelGate::new();
        let client = CancellableLlmClient::new(Arc::new(HangLlm), gate.clone());
        let token = CancelToken::new();
        gate.set(token.clone());
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            token.cancel();
        });
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.complete("", &[], &[], &test_meta()),
        )
        .await
        .expect("取消后应及时返回,而非挂起")
        .unwrap_err();
        assert!(matches!(res, AgentError::Cancelled));
    }

    #[tokio::test]
    async fn clear_restores_passthrough() {
        let gate = CancelGate::new();
        let client = CancellableLlmClient::new(Arc::new(OkLlm), gate.clone());
        let token = CancelToken::new();
        gate.set(token);
        let _guard = CancelGateGuard { gate: gate.clone() };
        drop(_guard); // 立即释放,验证 clear 后恢复直通
        let out = client.complete("", &[], &[], &test_meta()).await.unwrap();
        assert_eq!(out.text, "ok");
    }
}
