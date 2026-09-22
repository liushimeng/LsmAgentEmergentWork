//! agent 模块单元测试(2026-09-17 自 mod.rs 拆分)。

    use super::*;
    use serde_json::json;

    // ========== 取消传播(第 07 轮,方案 tmpPlan/2026-09-08_07) ==========

    /// 永远挂起的 LLM(模拟长时间未响应的请求)。
    struct HangLlm;

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for HangLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            std::future::pending().await
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    /// 第 1 次返回 bash 工具调用(长睡眠),之后返回最终文本。
    struct SlowToolLlm {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for SlowToolLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(Completion {
                    text: String::new(),
                    tool_calls: vec![crate::llm::ToolCallReq {
                        id: "call-slow-1".into(),
                        // 注意:注册表键为 "Bash"(工具 name() 原样)
                        name: "Bash".into(),
                        arguments: json!({"command": "sleep 30"}),
                    }],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            } else {
                Ok(Completion {
                    text: "done".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn cancel_during_llm_call_returns_promptly() {
        let agent = Agent::new(
            std::sync::Arc::new(HangLlm),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("慢任务"));
        let token = CancelToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            t2.cancel();
        });
        let res = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            agent
                .run_session_cancellable(&mut session, Some(&token))
                .await
        })
        .await
        .expect("取消后应及时返回,而非等 LLM 挂起")
        .unwrap_err();
        assert!(matches!(res, AgentError::Cancelled));
        // 没有工具调用发生,上下文不应被塞入取消回填
        assert_eq!(session.context().len(), 1);
    }

    #[tokio::test]
    async fn cancel_during_tool_execution_backfills_orphan() {
        let agent = Agent::new(
            std::sync::Arc::new(SlowToolLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("跑个长命令"));
        let token = CancelToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            // 等 bash sleep 30 已启动后再取消,验证工具执行中断 + 进程清理
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            t2.cancel();
        });
        let start = std::time::Instant::now();
        let res = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            agent
                .run_session_cancellable(&mut session, Some(&token))
                .await
        })
        .await
        .expect("工具执行中取消应及时返回,而非等 sleep 30 跑完")
        .unwrap_err();
        assert!(matches!(res, AgentError::Cancelled));
        // 及时性:远小于 sleep 30(允许 mock LLM 与进程启动开销)
        assert!(start.elapsed() < std::time::Duration::from_secs(4));
        // 消息一致性:末条应为 is_error 的取消 tool_result(无 orphan tool_use)
        let last = session.context().last().expect("应有取消回填");
        assert_eq!(last.role, crate::llm::Role::Tool);
        match &last.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "call-slow-1");
                assert!(content.contains("cancelled"));
                assert!(*is_error);
            }
            other => panic!("末条应为 ToolResult,实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn precancelled_token_short_circuits() {
        let agent = Agent::new(
            std::sync::Arc::new(HangLlm),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session
            .context_mut()
            .push(ChatMessage::user("已取消的任务"));
        let token = CancelToken::new();
        token.cancel();
        let res = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            agent.run_session_cancellable(&mut session, Some(&token)),
        )
        .await
        .expect("预取消应立即短路")
        .unwrap_err();
        assert!(matches!(res, AgentError::Cancelled));
    }

    #[test]
    fn is_truncation_stop_reason_detects_max_tokens() {
        assert!(is_truncation_stop_reason(Some("max_tokens")));
        assert!(is_truncation_stop_reason(Some("length")));
        assert!(!is_truncation_stop_reason(Some("end_turn")));
        assert!(!is_truncation_stop_reason(Some("stop")));
        assert!(!is_truncation_stop_reason(None));
    }

    // 关联报告: 20260908_203854 D-001
    #[test]
    fn stable_json_string_is_key_order_independent() {
        let a = json!({"path": "/tmp/missing", "limit": 10});
        let b = json!({"limit": 10, "path": "/tmp/missing"});
        assert_eq!(stable_json_string(&a), stable_json_string(&b));
    }

    #[test]
    fn stable_json_string_nested_objects() {
        let a = json!({"outer": {"a": 1, "b": [1, 2, 3]}});
        let b = json!({"outer": {"b": [1, 2, 3], "a": 1}});
        assert_eq!(stable_json_string(&a), stable_json_string(&b));
    }

    #[test]
    fn stable_json_string_different_values_produce_different_keys() {
        let a = json!({"path": "/tmp/missing_a"});
        let b = json!({"path": "/tmp/missing_b"});
        assert_ne!(stable_json_string(&a), stable_json_string(&b));
    }

    #[test]
    fn stable_json_string_arrays_preserve_order() {
        // 数组按设计保持顺序(LLM 调换数组元素顺序确实代表不同输入)
        let a = json!({"items": [1, 2, 3]});
        let b = json!({"items": [3, 2, 1]});
        assert_ne!(stable_json_string(&a), stable_json_string(&b));
    }

    #[test]
    fn stable_json_string_emits_valid_json_keys_quoted() {
        // 第 87 轮:key 必须带引号(下游 tool_args_digest / tool_args_brief 按
        // 合法 JSON 消费);此前 `{action:"x"}` 无引号形态导致参数摘要静默丢失。
        let s = stable_json_string(&json!({"action": "capability_probe", "x": 1}));
        assert_eq!(s, r#"{"action":"capability_probe","x":1}"#);
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("必须是合法 JSON");
        assert_eq!(parsed["action"], "capability_probe");
    }

    // ========== ExecutionTrace 累计(2026-09-09 第 05 轮) ==========

    /// 模拟 1 个工具调用成功(第 1 次返回工具,第 2 次返回最终文本)。
    struct OneOkToolLlm {
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for OneOkToolLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(Completion {
                    text: String::new(),
                    tool_calls: vec![crate::llm::ToolCallReq {
                        id: "call-ok".into(),
                        name: "Bash".into(),
                        arguments: json!({"command": "echo ok"}),
                    }],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            } else {
                Ok(Completion {
                    text: "工具成功完成".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn run_session_returns_trace_with_tool_ok() {
        let agent = Agent::new(
            std::sync::Arc::new(OneOkToolLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("跑命令"));
        let (text, _usage, trace) = agent.run_session(&mut session).await.unwrap();
        // 第 1 轮 mock LLM 返回 Bash echo ok → 累计 1 次成功;第 2 轮返回最终文本
        assert_eq!(trace.tool_calls_ok, 1, "应累计 1 次工具成功");
        assert_eq!(trace.tool_calls_err, 0);
        assert_eq!(trace.tool_calls, 1);
        assert_eq!(text, "工具成功完成");
        assert!(!trace.is_failed());
        assert!(trace.failure_signals.iter().any(|s| s == "ok"));
    }

    /// 模拟连续相同失败 → trace 应累计 early_terminated=true。
    struct SameFailLlm;
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for SameFailLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            // 始终尝试调用一个 sandbox 违规路径,触发 Write 沙箱失败,
            // 3 次连续 → 早终止。
            Ok(Completion {
                text: String::new(),
                tool_calls: vec![crate::llm::ToolCallReq {
                    id: "call-bad".into(),
                    name: "Bash".into(),
                    arguments: json!({"command": "rm -rf /"}), // 必触发 dangerous 命令拦截
                }],
                usage: Usage::default(),
                stop_reason: None,
            })
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn repeated_tool_failure_returns_error_and_trace_populated() {
        let agent = Agent::new(
            std::sync::Arc::new(SameFailLlm),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("跑命令"));
        let res = agent.run_session(&mut session).await;
        // 早终止返回 Err(RepeatedToolFailure),此处不是 trace 路径,而是 Agent 错误
        assert!(res.is_err(), "RepeatedToolFailure 应上抛 Err");
        match res.unwrap_err() {
            AgentError::RepeatedToolFailure { tool, attempts, .. } => {
                assert_eq!(tool, "Bash");
                assert!(attempts >= 3);
            }
            other => panic!("预期 RepeatedToolFailure,实际 {other:?}"),
        }
    }

    // ========== 无文本收敛短路(第 05 轮 E-001,方案 tmpPlan/2026-09-09_05) ==========

    /// 永远只产生 Bash tool_use、text 为空的 LLM。模拟「死循环不停探查」场景。
    struct OnlyToolLlm {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for OnlyToolLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Completion {
                text: String::new(),
                tool_calls: vec![crate::llm::ToolCallReq {
                    id: format!("call-{n}"),
                    name: "Bash".into(),
                    arguments: json!({"command": "echo only-tool"}),
                }],
                usage: Usage::default(),
                stop_reason: None,
            })
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    /// 验证: 持续 tool_use 无 text 应在阈值(默认 8)轮内触发「无文本收敛短路」,
    /// 返回 Ok 而不是 MaxIterationsExceeded。
    #[tokio::test]
    async fn no_text_converge_short_circuit_returns_ok() {
        let agent = Agent::new(
            std::sync::Arc::new(OnlyToolLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("陷入循环"));
        let (text, _usage, trace) = agent.run_session(&mut session).await.unwrap();
        // 应返回 Ok 而非 MaxIterationsExceeded
        assert!(
            trace.early_terminated,
            "无文本收敛短路应标记 early_terminated"
        );
        assert!(
            trace.early_terminate_reason.starts_with("no_text_converge"),
            "早终止原因应为 no_text_converge,实际 {}",
            trace.early_terminate_reason
        );
        // 兜底文本应包含「无文本收敛上限」字样
        assert!(
            text.contains("无文本收敛上限"),
            "兜底文本应提示无文本收敛,实际: {text}"
        );
        // 短路触发时,Bash 工具应至少被调用了阈值次数
        assert!(
            trace.tool_calls >= 8,
            "应至少执行 8 次 Bash 才短路,实际 {}",
            trace.tool_calls
        );
    }

    /// 验证: 工具调用 + 文本混合的正常 LLM 不应触发短路。
    struct MixedTextToolLlm {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for MixedTextToolLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < 3 {
                Ok(Completion {
                    text: format!("第 {} 轮说明", n + 1),
                    tool_calls: vec![crate::llm::ToolCallReq {
                        id: format!("call-{n}"),
                        name: "Bash".into(),
                        arguments: json!({"command": "echo mix"}),
                    }],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            } else {
                Ok(Completion {
                    text: "已完成".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn mixed_text_and_tool_does_not_short_circuit() {
        let agent = Agent::new(
            std::sync::Arc::new(MixedTextToolLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("混合任务"));
        let (_text, _usage, trace) = agent.run_session(&mut session).await.unwrap();
        // 混合文本+工具不应触发无文本收敛短路
        assert!(
            !trace.early_terminate_reason.starts_with("no_text_converge"),
            "混合场景不应触发无文本收敛短路,实际 reason={}",
            trace.early_terminate_reason
        );
    }

    // ========== 上下文溢出三级恢复(第 06 轮,方案 tmpPlan/2026-09-09_06) ==========

    /// 预置一条超长 tool_result 的会话(模拟上一轮循环产生的大工具输出)。
    fn session_with_fat_tool_result(fat_chars: usize) -> Session {
        let mut s = Session::new();
        s.context_mut().push(ChatMessage::user("跑个大命令"));
        s.context_mut()
            .push(ChatMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t-fat".into(),
                name: "Bash".into(),
                input: json!({"command": "seq 1 5000"}),
            }]));
        s.context_mut().push(ChatMessage::tool_result(
            "t-fat",
            "x".repeat(fat_chars),
            false,
        ));
        s
    }

    fn overflow_err() -> AgentError {
        AgentError::LlmHttp {
            status: 400,
            retry_after_ms: None,
            message: "HTTP 400: {\"error\":{\"message\":\"This model's maximum context length is 8192 tokens. However, your messages resulted in 54321 tokens\",\"code\":\"context_length_exceeded\"}}".into(),
        }
    }

    /// 第 1 次返回溢出 400,之后返回最终文本(验证排水后重试成功)。
    struct OverflowOnceLlm {
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for OverflowOnceLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Err(overflow_err())
            } else {
                Ok(Completion {
                    text: "溢出恢复后成功".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn overflow_recovers_via_drain_and_retries() {
        let agent = Agent::new(
            std::sync::Arc::new(OverflowOnceLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = session_with_fat_tool_result(20_000);
        let (text, _usage, trace) = agent.run_session(&mut session).await.unwrap();
        assert_eq!(text, "溢出恢复后成功");
        assert_eq!(trace.overflow_recoveries, 1, "应恰好排水恢复 1 次");
        assert!(
            trace
                .failure_signals
                .iter()
                .any(|s| s.starts_with("overflow_recovered:")),
            "恢复应产生弱失败信号,实际 {:?}",
            trace.failure_signals
        );
        assert!(!trace.is_failed(), "恢复成功不算失败");
        // 排水真实生效:超长 tool_result 已被截短
        match &session.context()[2].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("overflow-drain"));
                assert!(content.chars().count() < overflow::DRAIN_MIN_CHARS);
            }
            other => panic!("应为 ToolResult,实际 {other:?}"),
        }
    }

    /// 永远返回溢出 400(验证排水 + 折叠两级穷尽后按三级暴露上抛)。
    struct AlwaysOverflowLlm;
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for AlwaysOverflowLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            Err(overflow_err())
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn overflow_exhausts_recovery_and_exposes_error() {
        let agent = Agent::new(
            std::sync::Arc::new(AlwaysOverflowLlm),
            AgentProfile::sub_agent_work_profile(),
        );
        // 会话带可折叠的胖历史(触发 Level 2) + 超长工具结果(触发 Level 1)
        let mut session = session_with_fat_tool_result(20_000);
        for i in 0..6 {
            session.context_mut().insert(
                1,
                ChatMessage::user(format!("历史{i} {}", "话".repeat(400))),
            );
        }
        let res = agent.run_session(&mut session).await;
        let err = res.expect_err("两级恢复穷尽后应上抛原始溢出错误");
        assert!(
            matches!(&err, AgentError::LlmHttp { status: 400, .. }),
            "应原样上抛 LlmHttp 400,实际 {err:?}"
        );
        // Level 1 + Level 2 都真实执行过:工具结果被截短 + 折叠标记存在
        let has_drained = session.context().iter().any(|m| {
            m.content.iter().any(
                |b| matches!(b, ContentBlock::ToolResult { content, .. } if content.contains("overflow-drain")),
            )
        });
        assert!(has_drained, "排水应真实执行");
        let has_folded = session.context().iter().any(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text.contains(compact::COMPACT_MARKER_START)))
        });
        assert!(has_folded, "折叠应真实执行");
    }

    #[tokio::test]
    async fn overflow_budget_zero_disables_recovery() {
        let agent = Agent::new(
            std::sync::Arc::new(OverflowOnceLlm {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            AgentProfile::sub_agent_work_profile(),
        )
        .with_max_overflow_recoveries(0);
        let mut session = session_with_fat_tool_result(20_000);
        let res = agent.run_session(&mut session).await;
        assert!(res.is_err(), "预算为 0 时不恢复,直接上抛");
        // 未排水:工具结果保持原长
        match &session.context()[2].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content.chars().count(), 20_000);
            }
            other => panic!("应为 ToolResult,实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_overflow_error_not_recovered() {
        let agent = Agent::new(
            std::sync::Arc::new(FailGenericLlm),
            AgentProfile::sub_agent_work_profile(),
        );
        let mut session = session_with_fat_tool_result(20_000);
        let res = agent.run_session(&mut session).await;
        let err = res.expect_err("非溢出错误应原样上抛");
        assert!(matches!(err, AgentError::Llm(_)));
        // 上下文未被排水
        match &session.context()[2].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content.chars().count(), 20_000, "非溢出错误不应触发排水");
            }
            other => panic!("应为 ToolResult,实际 {other:?}"),
        }
    }

    /// 普通失败(非溢出),不应触发任何恢复。
    struct FailGenericLlm;
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for FailGenericLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            Err(AgentError::Llm("mock down".into()))
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    // ========== Agent 身份逐请求注入(第 08 轮,方案 tmpPlan/2026-09-09_08) ==========

    /// 捕获 complete() 收到的 RequestMeta(含 User-Agent),验证 Agent 循环逐请求注入。
    struct MetaCaptureLlm {
        seen: std::sync::Mutex<Vec<RequestMeta>>,
    }
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for MetaCaptureLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            meta: &RequestMeta,
        ) -> Result<Completion> {
            self.seen.lock().expect("meta capture").push(meta.clone());
            Ok(Completion {
                text: "done".into(),
                tool_calls: vec![],
                usage: Usage::default(),
                stop_reason: None,
            })
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn run_session_injects_profile_user_agent_per_request() {
        // Yolo profile → 请求 UA 必须是 Yolo 的(而非共享客户端的构造期默认)
        let cap = std::sync::Arc::new(MetaCaptureLlm {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(cap.clone(), AgentProfile::yolo_profile());
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("分类一下"));
        agent.run_session(&mut session).await.unwrap();
        let seen = cap.seen.lock().expect("meta capture");
        assert!(!seen.is_empty(), "应至少发起一次 LLM 调用");
        for meta in seen.iter() {
            assert!(
                meta.user_agent.starts_with("LsmAgentEmergentWork-Yolo/"),
                "UA 应逐请求注入为 Yolo,实际: {}",
                meta.user_agent
            );
        }
    }

    #[tokio::test]
    async fn run_session_injects_subagent_user_agent() {
        // SubAgent-Work profile → 同一注入路径下角色名跟随 profile 变化
        let cap = std::sync::Arc::new(MetaCaptureLlm {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(cap.clone(), AgentProfile::sub_agent_work_profile());
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user("跑命令"));
        agent.run_session(&mut session).await.unwrap();
        let seen = cap.seen.lock().expect("meta capture");
        assert!(
            seen.iter().all(|m| m
                .user_agent
                .starts_with("LsmAgentEmergentWork-SubAgent-Work/")),
            "UA 应为 SubAgent-Work,实际: {:?}",
            seen.iter()
                .map(|m| m.user_agent.clone())
                .collect::<Vec<_>>()
        );
    }

    // ========== L1037 + L771 联动(2026-09-09 第 09 轮) ==========

    #[test]
    fn runtime_hints_empty_when_no_counters_active() {
        // 所有计数器为 0 时,返回空字符串(零开销,不影响 LLM 上下文)
        let t = ExecutionTrace::default();
        let h = build_runtime_hints(&t, 0);
        assert!(h.is_empty(), "全 0 应返回空串,实际: {h:?}");
        let h = build_runtime_hints(&t, 1);
        assert!(
            h.is_empty(),
            "consecutive_failures < 2 也不应触发,实际: {h:?}"
        );
    }

    #[test]
    fn runtime_hints_truncation_when_resumes_present() {
        let mut t = ExecutionTrace::default();
        t.truncation_resumes = 2;
        let h = build_runtime_hints(&t, 0);
        assert!(h.contains("已续接 2 次"));
        assert!(h.contains("LAEW:RUNTIME_HINTS"));
        assert!(h.contains("END"));
    }

    #[test]
    fn runtime_hints_overflow_when_recoveries_present() {
        let mut t = ExecutionTrace::default();
        t.overflow_recoveries = 1;
        let h = build_runtime_hints(&t, 0);
        assert!(h.contains("自动恢复 1 次"));
        assert!(h.contains("排水/折叠"));
    }

    #[test]
    fn runtime_hints_max_tokens_when_upscalings_present() {
        let mut t = ExecutionTrace::default();
        t.max_tokens_upscalings = 2;
        let h = build_runtime_hints(&t, 0);
        assert!(h.contains("max_tokens 已升级 2 次"));
    }

    #[test]
    fn runtime_hints_consecutive_failures_threshold() {
        // consecutive_failures >= 2 触发
        let t = ExecutionTrace::default();
        assert!(build_runtime_hints(&t, 2).contains("连续 2 次"));
        assert!(build_runtime_hints(&t, 5).contains("连续 5 次"));
        // < 2 不触发
        assert!(build_runtime_hints(&t, 1).is_empty());
    }

    #[test]
    fn runtime_hints_multiple_lines_joined() {
        // 多个信号并存时全部出现
        let mut t = ExecutionTrace::default();
        t.truncation_resumes = 1;
        t.overflow_recoveries = 1;
        t.max_tokens_upscalings = 1;
        let h = build_runtime_hints(&t, 3);
        assert!(h.contains("已续接 1 次"));
        assert!(h.contains("自动恢复 1 次"));
        assert!(h.contains("max_tokens 已升级 1 次"));
        assert!(h.contains("连续 3 次"));
    }

    #[test]
    fn finalize_trace_records_max_tokens_state() {
        // 验证 finalize_with_max_tokens 把 MaxTokensState 写入 trace
        use crate::agent::max_tokens_state::MaxTokensState;
        let state = MaxTokensState::new();
        state.note_truncated(); // 8K → 16K
        state.note_truncated(); // 16K → 32K
        let mut t = ExecutionTrace::default();
        t.iterations = 2;
        let (text, _u, t) =
            crate::agent::Agent::finalize_with_max_tokens(t, "hello", Usage::default(), &state)
                .unwrap();
        assert_eq!(text, "hello");
        assert_eq!(t.max_tokens_upscalings, 2);
        assert_eq!(t.max_tokens_history, vec![(8192, 16384), (16384, 32768)]);
    }

    // ========== 结构化输出通道短路(L6/L19,2026-09-09 第 13 轮) ==========

    /// 返回固定 Completion 的 mock LLM(记录每次收到的 meta.forced_tool)。
    struct EmitLlm {
        replies: std::sync::Mutex<Vec<Completion>>,
        seen_forced: std::sync::Mutex<Vec<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for EmitLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            meta: &RequestMeta,
        ) -> Result<Completion> {
            self.seen_forced
                .lock()
                .expect("seen_forced")
                .push(meta.forced_tool.clone());
            let mut replies = self.replies.lock().expect("replies");
            if replies.is_empty() {
                return Err(AgentError::Other("mock 耗尽".into()));
            }
            Ok(replies.remove(0))
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    fn emit_completion(calls: Vec<(&'static str, serde_json::Value)>, text: &str) -> Completion {
        Completion {
            text: text.to_string(),
            tool_calls: calls
                .into_iter()
                .enumerate()
                .map(|(i, (name, args))| crate::llm::ToolCallReq {
                    id: format!("call_{i}"),
                    name: name.to_string(),
                    arguments: args,
                })
                .collect(),
            usage: Usage::default(),
            stop_reason: Some("tool_use".into()),
        }
    }

    #[tokio::test]
    async fn emit_tool_short_circuits_loop() {
        // Yolo profile + 模型返回 submit_task_classification tool_use:
        // 循环应 1 轮终止,最终文本含 ```json 块,trace.structured_emits = 1。
        // 2026-09-22 ReAct 延迟强制:Yolo 在默认 max_iterations(20) 下首轮为探索轮,
        // 不注入 forced_tool —— 模型当轮自愿提交 emit 后 1 轮短路,从未到达强制轮。
        let llm = std::sync::Arc::new(EmitLlm {
            replies: std::sync::Mutex::new(vec![emit_completion(
                vec![(
                    "submit_task_classification",
                    json!({
                        "task_level": "medium",
                        "purpose": "验证",
                        "goal_summary": "结构化输出验证",
                        "intent": "verify",
                        "decomposition_plan": ["步骤一"],
                        "direct_answer": null
                    }),
                )],
                "这是中等难度任务,已提交分类。",
            )]),
            seen_forced: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(llm.clone(), AgentProfile::yolo_profile());
        let (text, _usage, trace) = agent.run_once("测试任务").await.unwrap();

        // 最终文本 = 模型文本 + emit input 的 ```json 块
        assert!(
            text.contains("结构化输出验证"),
            "文本应含 emit input: {text}"
        );
        assert!(text.contains("```json"), "应输出 json 代码块: {text}");
        // 下游解析链直接命中
        let c = crate::agent::yolo::parse_classification(&text).unwrap();
        assert_eq!(c.task_level, crate::agent::yolo::TaskLevel::Medium);
        assert!(!c.yolo_degraded, "emit 路径不应降级");
        // 1 轮终止 + 计数
        assert_eq!(trace.iterations, 1);
        assert_eq!(trace.structured_emits, 1);
        assert_eq!(trace.tool_calls, 0, "emit 通道不计入工具执行计数");
        // 协议层:探索轮未注入 forced(model 自愿提交 emit → 短路,从未触发强制逻辑)
        let seen = llm.seen_forced.lock().expect("seen_forced");
        assert!(
            seen.iter().all(|f| f.is_none()),
            "Yolo 探索轮(非最终轮)不应注入 forced,实际: {seen:?}"
        );
    }

    #[tokio::test]
    async fn emit_tool_mixed_parallel_calls_ignored() {
        // emit + Read 并行调用(emit 在前):emit 之后的 Read 不执行,仅回填「已忽略」;
        // 上下文 tool_use 与 tool_result 一一配对(无孤儿)。
        // 注:Read 在 emit 之前的场景是正常工作流(先探查后提交),由
        // emit_tool_short_circuits_loop 与 non_emit_profile_never_forced 覆盖。
        let llm = std::sync::Arc::new(EmitLlm {
            replies: std::sync::Mutex::new(vec![emit_completion(
                vec![
                    (
                        "submit_quality_report",
                        json!({
                            "verdict": "pass",
                            "source": "subagent",
                            "retryable": false
                        }),
                    ),
                    ("Read", json!({"file_path": "/definitely/not/exists.txt"})),
                ],
                "",
            )]),
            seen_forced: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(llm, AgentProfile::quality_check_profile());
        let mut session = crate::session::Session::new();
        session.context_mut().push(ChatMessage::user("质检"));
        let (text, _usage, trace) = agent.run_session(&mut session).await.unwrap();

        assert_eq!(trace.structured_emits, 1);
        assert_eq!(trace.tool_calls, 0, "emit 之后的 Read 不应被执行");
        assert!(text.contains("\"verdict\": \"pass\""));

        // 上下文配对检查:assistant ToolUse × 2 ↔ tool_result × 2
        let ctx = session.context();
        let tool_uses: Vec<&ContentBlock> = ctx
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
            .collect();
        let tool_results: Vec<&ContentBlock> = ctx
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
            .collect();
        assert_eq!(tool_uses.len(), 2, "assistant 应含 2 个 tool_use");
        assert_eq!(
            tool_results.len(),
            2,
            "每个 tool_use 都应有 tool_result 回填"
        );
        // 忽略回填的内容标记
        let ignored = tool_results
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .any(|c| c.contains("已忽略"));
        assert!(ignored, "Read 的回填应标记「已忽略」");
    }

    // ========== 2026-09-22 Yolo ReAct 延迟强制 ==========

    #[tokio::test]
    async fn yolo_react_emits_force_only_on_final_round() {
        // ReAct 延迟强制:探索轮 forced=None(模型自由调用 Bash 收集);
        // 最后一轮 forced=emit(保证结构化收口)。
        let llm = std::sync::Arc::new(EmitLlm {
            replies: std::sync::Mutex::new(vec![
                emit_completion(
                    vec![("Bash", json!({"command": "echo react-probe"}))],
                    "先收集信息。",
                ),
                emit_completion(
                    vec![(
                        "submit_task_classification",
                        json!({
                            "task_level": "simple",
                            "goal_summary": "回显验证",
                            "intent": "verify",
                            "decomposition_plan": [],
                            "direct_answer": null
                        }),
                    )],
                    "信息已足够,提交分类。",
                ),
            ]),
            seen_forced: std::sync::Mutex::new(Vec::new()),
        });
        let agent =
            Agent::new(llm.clone(), AgentProfile::yolo_profile()).with_max_iterations(2);
        let (_text, _usage, trace) = agent.run_once("echo react-probe").await.unwrap();

        let seen = llm.seen_forced.lock().expect("seen_forced");
        let seen: Vec<Option<String>> = seen.iter().cloned().collect();
        assert_eq!(
            seen,
            vec![None, Some("submit_task_classification".to_string())],
            "探索轮不强制 / 最终轮强制 emit,实际: {seen:?}"
        );
        assert_eq!(trace.tool_calls, 1, "Bash 探索调用应真实执行");
        assert_eq!(trace.structured_emits, 1);
        assert_eq!(trace.iterations, 2);
    }

    #[tokio::test]
    async fn quality_check_always_forces_emit() {
        // 对照试验:Quality-Check 不开 defer_emit_force,每轮都强制。
        let llm = std::sync::Arc::new(EmitLlm {
            replies: std::sync::Mutex::new(vec![
                emit_completion(
                    vec![(
                        "submit_quality_report",
                        json!({"verdict": "pass", "source": "subagent", "retryable": false}),
                    )],
                    "",
                ),
            ]),
            seen_forced: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(llm.clone(), AgentProfile::quality_check_profile())
            .with_max_iterations(2);
        let _ = agent.run_once("质检").await.unwrap();
        let seen = llm.seen_forced.lock().expect("seen_forced");
        let seen: Vec<_> = seen.iter().map(|f| f.as_deref().map(|s| s.to_string())).collect();
        assert!(
            seen.iter().all(|f| f.as_deref() == Some("submit_quality_report")),
            "Quality-Check 每轮都应强制 submit_quality_report,实际: {seen:?}"
        );
    }

    #[tokio::test]
    async fn non_emit_profile_never_forced() {
        // 无 emit_tool 的 profile(SubAgent)不注入 forced,模型普通工具照常执行
        let llm = std::sync::Arc::new(EmitLlm {
            replies: std::sync::Mutex::new(vec![
                emit_completion(vec![("Bash", json!({"command": "echo forced-check"}))], ""),
                Completion {
                    text: "done".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                },
            ]),
            seen_forced: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(llm.clone(), AgentProfile::sub_agent_work_profile());
        let (text, _usage, trace) = agent.run_once("echo 测试").await.unwrap();
        assert_eq!(text.trim(), "done");
        assert_eq!(trace.tool_calls, 1, "Bash 应正常执行");
        assert_eq!(trace.structured_emits, 0);
        let seen = llm.seen_forced.lock().expect("seen_forced");
        assert!(
            seen.iter().all(|f| f.is_none()),
            "无 emit_tool 的 profile 不应注入 forced,实际: {seen:?}"
        );
    }

    #[test]
    fn forced_tools_switch_parsing() {
        // 环境变量取值解析(off/0/false/no 关闭,其余含空值开启)
        assert!(!forced_tools_enabled_from("off".into()));
        assert!(!forced_tools_enabled_from("0".into()));
        assert!(!forced_tools_enabled_from("FALSE".into()));
        assert!(!forced_tools_enabled_from(" no ".into()));
        assert!(forced_tools_enabled_from(String::new()));
        assert!(forced_tools_enabled_from("on".into()));
    }

    // ========== 第 114 轮(2026-09-22):自感知 SubAgent 动态启动 ==========

    /// 取 tool 消息里的 tool_result 文本(content_text 只看 Text 块)。
    fn tool_result_text(msg: &ChatMessage) -> String {
        msg.content
            .iter()
            .filter_map(|b| match b {
                crate::llm::ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 父 Agent 依次调用 `SubAgent(action="list")` → `action="launch"` → 终答;
    /// 子 Agent(新上下文)直接返回文本。用于验证 Agent 循环里的 task-local 作用域注入。
    struct SpawnCallingLlm {
        parent_calls: std::sync::atomic::AtomicUsize,
        child_calls: std::sync::atomic::AtomicUsize,
        seen_child_system: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for SpawnCallingLlm {
        async fn complete(
            &self,
            system: &str,
            messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            // 子 Agent 身份用**子 Agent 专属**标记识别(父提示词也会提到「子 Agent」,
            // 因此必须用只在 SubAgentType::system_prompt 里出现的句子)
            if system.contains("你只拿到**任务描述**这一个输入") {
                self.child_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.seen_child_system
                    .lock()
                    .expect("child system")
                    .push(system.to_string());
                return Ok(Completion {
                    text: "CHILD_REPORT:模块划分清单已产出".into(),
                    tool_calls: vec![],
                    usage: Usage {
                        input_tokens: 21,
                        output_tokens: 9,
                        ..Usage::default()
                    },
                    stop_reason: None,
                });
            }
            let n = self
                .parent_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match n {
                0 => Ok(Completion {
                    text: String::new(),
                    tool_calls: vec![crate::llm::ToolCallReq {
                        id: "call-spawn-1".into(),
                        name: "SubAgent".into(),
                        arguments: serde_json::json!({"action": "list"}),
                    }],
                    usage: Usage::default(),
                    stop_reason: None,
                }),
                1 => {
                    // list 的结果必须已在上下文里(自感知快照)
                    let has_snapshot = messages.iter().any(|m| tool_result_text(m).contains("can_spawn"));
                    assert!(has_snapshot, "action=list 的 tool_result 应含自感知快照");
                    Ok(Completion {
                        text: String::new(),
                        tool_calls: vec![crate::llm::ToolCallReq {
                            id: "call-spawn-2".into(),
                            name: "SubAgent".into(),
                            arguments: serde_json::json!({
                                "action": "launch",
                                "task": "调研 src/agent 的模块划分,输出清单",
                                "agent_type": "explore"
                            }),
                        }],
                        usage: Usage::default(),
                        stop_reason: None,
                    })
                }
                _ => {
                    let child_report = messages
                        .iter()
                        .map(tool_result_text)
                        .find(|t| t.contains("CHILD_REPORT"))
                        .unwrap_or_default();
                    assert!(
                        child_report.contains("CHILD_REPORT"),
                        "子 Agent 结果应回填到父上下文: {child_report}"
                    );
                    Ok(Completion {
                        text: "汇总:子 Agent 给出了 src/agent 模块划分清单。".into(),
                        tool_calls: vec![],
                        usage: Usage::default(),
                        stop_reason: None,
                    })
                }
            }
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn agent_loop_injects_runtime_and_runs_dynamic_child() {
        let llm = std::sync::Arc::new(SpawnCallingLlm {
            parent_calls: std::sync::atomic::AtomicUsize::new(0),
            child_calls: std::sync::atomic::AtomicUsize::new(0),
            seen_child_system: std::sync::Mutex::new(Vec::new()),
        });
        let agent = Agent::new(llm.clone(), AgentProfile::sub_agent_work_profile());
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user(
            "启动 1 个 SubAgent 调研 src/agent 的模块划分",
        ));
        let (text, usage, trace) = agent.run_session(&mut session).await.unwrap();
        assert!(text.contains("汇总"), "父 Agent 应汇总子 Agent 结果: {text}");
        assert_eq!(
            llm.child_calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "子 Agent 应真实执行一次"
        );
        assert_eq!(trace.tool_calls, 2, "父 Agent 共 2 次工具调用(list + launch)");
        assert_eq!(trace.tool_calls_err, 0, "两次工具调用都应成功");
        // 子 Agent 用量进入会话台账(单元/任务边界由 Runner/Orchestrator 排空)
        let drained =
            crate::agent::dynamic_subagent::drain_usage(session.id());
        assert_eq!(drained.input_tokens, 21, "子 Agent 用量应被记账");
        assert_eq!(drained.output_tokens, 9);
        // 父 Agent 自身的 usage 不因记账被污染(仍来自父轮次)
        assert_eq!(usage.input_tokens, 0);
        // 子 Agent 的系统提示词是叶子语义 + 类型身份
        let child_system = llm.seen_child_system.lock().unwrap()[0].clone();
        assert!(child_system.contains("不能再启动子 Agent"));
        assert!(child_system.contains("只读侦察"));
    }

    /// 叶子 Agent 的注册表不含 `SubAgent` 工具:即使模型幻觉调用也应得到
    /// ToolNotFound 的回填提示(而非无限递归)。
    #[tokio::test]
    async fn leaf_child_registry_excludes_subagent_tool() {
        let llm = std::sync::Arc::new(super::tests::SpawnCallingLlm {
            parent_calls: std::sync::atomic::AtomicUsize::new(0),
            child_calls: std::sync::atomic::AtomicUsize::new(0),
            seen_child_system: std::sync::Mutex::new(Vec::new()),
        });
        // 直接检查动态子 Agent 的工具面(不发起 LLM 调用)
        let child_profile = AgentProfile::dynamic_child(
            "leaf",
            "prompt".into(),
            crate::agent::tools::sub_agent_work_registry().subset(&[], &["SubAgent"]),
            crate::agent::self_awareness::SpawnPolicy::Disabled,
        );
        assert!(
            !child_profile.tools.names().contains(&"SubAgent"),
            "叶子注册表不得含 SubAgent"
        );
        let rendered = child_profile
            .system_prompt
            .render(crate::config::Protocol::Anthropic);
        assert!(
            !rendered.contains("你可启动的子 Agent 类型"),
            "叶子提示词不应含可启动名册"
        );
        drop(llm);
    }
