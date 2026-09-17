//! orchestrator 模块单元测试(2026-09-17 自 orchestrator.rs 拆分)。

    use super::*;
    use crate::config::{Db, Paths};
    use tempfile::tempdir;

    fn fresh_orchestrator() -> (MultiAgentOrchestrator, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Arc::new(Db::open(&paths).unwrap());
        // 用 NoopLlm 即可
        struct NoopLlm;
        #[async_trait::async_trait]
        impl crate::llm::LlmClient for NoopLlm {
            async fn complete(
                &self,
                _system: &str,
                _messages: &[crate::llm::ChatMessage],
                _tools: &[crate::llm::ToolDef],
                _meta: &crate::llm::RequestMeta,
            ) -> Result<crate::llm::Completion> {
                Ok(crate::llm::Completion {
                    text: "noop".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
            fn protocol(&self) -> crate::config::Protocol {
                crate::config::Protocol::Anthropic
            }
        }
        let plans_dir = dir.path().join("plans");
        let orch = MultiAgentOrchestrator::new(Arc::new(NoopLlm), db, plans_dir);
        (orch, dir)
    }

    #[test]
    fn build_subflow_input_with_no_deps() {
        let wf = WorkFlowSpec {
            id: "wf-1".into(),
            name: "读取".into(),
            steps: vec!["读 a".into(), "读 b".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec!["OK".into()],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:测试 fixture 无目标应用。
            target_app: None,
        };
        let input = build_subflow_input(&wf, &std::collections::HashMap::new(), "");
        assert_eq!(input.id, "wf-1.step");
        assert!(input.description.contains("读 a"));
        assert_eq!(input.expected_output, "OK");
        // 2026-09-11 第三十三轮:#P-A 修复 — medium/hard 路径默认透传 wf.name 作为
        // original_prompt,SubAgent 不会丢失 WorkFlow 主题。
        assert_eq!(input.original_prompt.as_deref(), Some("读取"));
    }

    #[test]
    fn build_subflow_input_with_deps() {
        let mut deps = std::collections::HashMap::new();
        deps.insert("wf-1".into(), "已读取 a.rs".into());
        let wf = WorkFlowSpec {
            id: "wf-2".into(),
            name: "修改".into(),
            steps: vec!["改 a.rs".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec!["wf-1".into()],
            acceptance: vec!["修改完成".into()],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:测试 fixture 无目标应用。
            target_app: None,
        };
        let input = build_subflow_input(&wf, &deps, "");
        assert_eq!(input.depends_on_outputs.len(), 1);
        assert!(input.depends_on_outputs[0].contains("已读取 a.rs"));
    }

    #[test]
    fn orchestrator_constructs_with_all_components() {
        let (orch, _d) = fresh_orchestrator();
        // 各 runner 都已构造
        let _ = orch.yolo();
        let _ = orch.quality();
        let _ = orch.session_context();
        let _ = orch.sub_agent();
        let _ = orch.plan();
        let _ = orch.main_work();
    }

    // ========== actionable 失败文案(第 05 轮 E-003,方案 tmpPlan/2026-09-09_05) ==========

    #[test]
    fn fallback_suggestion_distinguishes_output_levels() {
        // output_token = 0: 任务描述不够具体
        let zero = fallback_suggestion(&Usage::default());
        assert!(
            zero.contains("未产出任何答复"),
            "output=0 应提示「未产出任何答复」,实际: {zero}"
        );

        // output_token 极小(< 50): 可能反复工具调用未收敛
        let tiny = fallback_suggestion(&Usage {
            output_tokens: 10,
            ..Usage::default()
        });
        assert!(
            tiny.contains("答复信息密度极低"),
            "output=10 应提示「答复信息密度极低」,实际: {tiny}"
        );

        // output_token 较大: 提示拆分 / 调整目标
        // F11(2026-09-10 第 25 轮):措辞不再把 output_tokens 谎报为「迭代次数」
        let ample = fallback_suggestion(&Usage {
            output_tokens: 200,
            ..Usage::default()
        });
        assert!(
            ample.contains("output tokens") && ample.contains("拆分"),
            "output=200 应如实描述产出并提示拆分,实际: {ample}"
        );
    }

    // ========== 占位字符串归一化(2026-09-10 第 27 轮 F12 / BUG-2026-09-10-TUI-NULL) ==========

    #[test]
    fn placeholder_direct_answer_normalizes() {
        // 字面量 "null" / "None" / "NULL" / "Nil" / 空字符串 → 视为未填(继续走委派)
        for s in [
            "", "  ", "null", "NULL", "Null", "None", "none", "NIL", "nil",
        ] {
            assert!(
                is_placeholder_direct_answer(s),
                "{s:?} 应被识别为占位字符串,实际未识别"
            );
        }
    }

    #[test]
    fn placeholder_direct_answer_keeps_real_answers() {
        // 真正含答案的字符串不应被误判为占位
        for s in [
            "答:巴黎",
            "1+1=2",
            "答案是42",
            "nullable", // 含 "null" 子串但不是占位
            "nonempty answer",
            "nullabc",
        ] {
            assert!(
                !is_placeholder_direct_answer(s),
                "{s:?} 不应被识别为占位,实际被误判"
            );
        }
    }

    // ========== 第 57 轮:阶段耗时 / 分层 / 重试数据结构 ==========

    #[test]
    fn stage_duration_serializes_roundtrip() {
        // 2026-09-16 第 57 轮:新增数据结构必须能 Serialize/Deserialize(任务写库
        // + 未来 transcript 导出走 JSON 时会消费这些字段)。
        let s = StageDuration {
            stage: "wf".to_string(),
            wf_id: Some("wf-1".to_string()),
            started_offset_ms: 100,
            elapsed_ms: 1234,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: StageDuration = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stage, "wf");
        assert_eq!(back.wf_id.as_deref(), Some("wf-1"));
        assert_eq!(back.elapsed_ms, 1234);
    }

    #[test]
    fn retry_record_and_layer_info_roundtrip() {
        let r = RetryRecord {
            retry_count: 2,
            retry_hint: "wf-3 步骤 2 缺空格".to_string(),
            started_offset_ms: 0,
            elapsed_ms: 8765,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: RetryRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.retry_count, 2);
        assert_eq!(back.elapsed_ms, 8765);

        let l = LayerInfo {
            layer_idx: 1,
            wf_ids: vec!["wf-1".into(), "wf-2".into()],
            parallel: true,
            started_offset_ms: 50,
            elapsed_ms: 41234,
        };
        let json = serde_json::to_string(&l).unwrap();
        let back: LayerInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.layer_idx, 1);
        assert!(back.parallel);
        assert_eq!(back.wf_ids.len(), 2);
    }

    #[test]
    fn task_result_serde_with_new_fields_roundtrip() {
        // 2026-09-16 第 57 轮:TaskResult 新增 stage_durations / retry_log /
        // layer_log / wallclock_ms 必须走 Serialize 完整往返(否则下次 db 升级
        // 或 transcript 导出时会丢字段)。
        use crate::agent::quality::{QualityReport, Verdict};
        let tr = TaskResult {
            goal: "测试目标".into(),
            classification: TaskClassification {
                task_level: TaskLevel::Simple,
                purpose: "p".into(),
                goal_summary: "g".into(),
                intent: "info".into(),
                agent_role: None,
                decomposition_plan: vec![],
                direct_answer: None,
                user_suggestion_if_fail: String::new(),
                yolo_degraded: false,
                suggested_delegate: None,
            },
            plan_doc: None,
            workflows: vec![WorkflowResult {
                id: "wf-1".into(),
                name: "n".into(),
                subflow_outcome: "out".into(),
                quality_report: QualityReport {
                    verdict: Verdict::Pass,
                    issues: vec![],
                    suggestion: String::new(),
                    retryable: false,
                    source: AgentRole::SubAgent,
                    evidence: String::new(),
                },
                usage: Usage::default(),
                subflow_trace: None,
                exec_role: AgentRole::SubAgent,
                wallclock_ms: 100,
                qc_wallclock_ms: 50,
            }],
            summary: String::new(),
            total_usage: Usage::default(),
            stage_durations: vec![StageDuration {
                stage: "yolo".into(),
                wf_id: None,
                started_offset_ms: 0,
                elapsed_ms: 8,
            }],
            retry_log: vec![RetryRecord {
                retry_count: 1,
                retry_hint: "x".into(),
                started_offset_ms: 0,
                elapsed_ms: 100,
            }],
            layer_log: vec![],
            wallclock_ms: 1000,
        };
        let json = serde_json::to_string(&tr).unwrap();
        let back: TaskResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.wallclock_ms, 1000);
        assert_eq!(back.stage_durations.len(), 1);
        assert_eq!(back.stage_durations[0].stage, "yolo");
        assert_eq!(back.retry_log.len(), 1);
        assert_eq!(back.retry_log[0].retry_count, 1);
        assert_eq!(back.workflows[0].exec_role, AgentRole::SubAgent);
        assert_eq!(back.workflows[0].wallclock_ms, 100);
    }
