    use super::*;

    // ==================== 第 140 轮:QC 输入裁剪 ====================

    #[test]
    fn clip_qc_input_short_is_identity() {
        let s = "短输入原样返回";
        assert_eq!(clip_qc_input(s), s);
    }

    #[test]
    fn clip_qc_input_long_keeps_head_and_tail_with_marker() {
        let head: String = std::iter::repeat('H').take(6000).collect();
        let mid: String = std::iter::repeat('M').take(5000).collect();
        let tail: String = std::iter::repeat('T').take(2000).collect();
        let s = format!("{head}{mid}{tail}");
        let out = clip_qc_input(&s);
        assert!(out.starts_with(&head), "头部 6000 字符应保留");
        assert!(out.ends_with(&tail), "尾部 2000 字符应保留");
        assert!(out.contains("中段省略 5000 字符"), "应标注省略量,实际: {}", &out[6000..6100]);
        assert!(out.contains("原文 13000 字符"), "应标注原文总长");
        assert!(out.len() < s.len(), "裁剪后应更短");
    }

    #[test]
    fn quality_report_pass_default() {
        let r = QualityReport::pass(AgentRole::SubAgent);
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::SubAgent);
    }

    #[test]
    fn quality_report_fail_sets_retryable() {
        let r = QualityReport::fail(AgentRole::SubAgent, vec!["x".into()], "再试一次", true);
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
        assert_eq!(r.suggestion, "再试一次");
    }

    // ========== 第 93 轮:QC prompt 反降级(原始输入透传) ==========

    #[test]
    fn qc_prompt_includes_original_prompt_and_anti_degradation_rule() {
        let p = build_unit_qc_prompt(
            "启动微信保持10分钟",
            "打开微信并维持对话功能10分钟",
            "保活≥600s",
            "已保活612s",
            "iter=10 tools=9",
            Some("打开微信并主动与赵玲玲聊天,每分钟2次,保存 Markdown 报告"),
        );
        assert!(p.contains("第一性事实 · 用户原始输入"));
        assert!(p.contains("赵玲玲"));
        assert!(p.contains("反降级规则"));
        assert!(p.contains("纯等待/时间差/进程存活不构成任何 UI 操作的完成证据"));
    }

    #[test]
    fn qc_prompt_without_original_prompt_has_no_degradation_rule() {
        let p = build_unit_qc_prompt("g", "scope", "exp", "act", "trace", None);
        assert!(!p.contains("第一性事实"));
        assert!(!p.contains("反降级规则"));
        // 空白原始输入视同无
        let p2 = build_unit_qc_prompt("g", "scope", "exp", "act", "trace", Some("  \n "));
        assert!(!p2.contains("第一性事实"));
    }

    // ========== 第 109 轮:桌面目标保真(网页版不构成等效完成) ==========

    #[test]
    fn qc_prompt_desktop_fidelity_rule_present() {
        let p = build_unit_qc_prompt(
            "找到软件豆包,输入信息,读取对话返回的结果",
            "在豆包中输入查询并读取结果",
            "chat_log 含 [SEND] 行",
            "已在网页版完成",
            "iter=8 tools=MCP_Web_LsmAgentBrowser_Use",
            Some("找到软件 豆包 , 输入信息,读取对话返回的结果,显示相关的对话信息"),
        );
        // 反降级规则必须包含桌面目标保真条款
        assert!(p.contains("桌面目标保真"), "{p}");
        assert!(p.contains("不构成等效完成"), "{p}");
        // suggestion 层面禁止推荐网页版
        assert!(p.contains("禁止推荐「改用网页版"), "{p}");
        assert!(p.contains("MCP_Web_LsmAgentBrowser_Use"), "{p}");
    }

    // ========== 第 128 轮:目标一致性硬门(禁止改派目标站点) ==========

    #[test]
    fn qc_prompt_target_consistency_gate_present() {
        let p = build_unit_qc_prompt(
            "抓取 anthropic.com 最新 3 篇文章",
            "打开目标网站并 explore 首页结构",
            "final_url 主域 = anthropic.com",
            "已在 ithome.com 抓到 3 篇文章",
            "iter=4 tools=5 target_drift:ithome.com",
            Some("打开 https://www.anthropic.com/ ,找出最新的 3 篇文章,显示标题和 URL"),
        );
        assert!(p.contains("目标一致性"), "{p}");
        assert!(p.contains("最高优先级硬门"), "{p}");
        // 改派必须判不可重试(重试只会在错误目标上把产出做得更完整)
        assert!(p.contains("retryable=false"), "{p}");
        // 对账句式 + 禁止推荐替代品
        assert!(p.contains("用户指定 X,实际操作 Y"), "{p}");
        assert!(p.contains("严禁」推荐") || p.contains("严禁**推荐") || p.contains("严禁"), "{p}");
        assert!(p.contains("搜索引擎"), "{p}");
        // 机械判据必须写进提示词,QC 才能引用
        assert!(p.contains("6001"), "{p}");
        assert!(p.contains("target_drift"), "{p}");
        // 漂亮地完成错误目标仍是 Fail
        assert!(p.contains("完成得很漂亮"), "{p}");
        assert!(p.contains("session_memory"), "{p}");
    }

    #[test]
    fn qc_prompt_target_consistency_has_open_task_exception() {
        // 用户没指定站点时不得套用目标一致性门,否则合法开放任务会被误判
        let p = build_unit_qc_prompt(
            "搜索最新 Rust 新闻",
            "打开站点并抓取",
            "3 条标题+链接",
            "已在 ithome.com 抓到",
            "iter=3 tools=4",
            Some("搜一下最新的 Rust 语言新闻,给我 3 条标题和链接"),
        );
        assert!(p.contains("例外"), "{p}");
        assert!(p.contains("自主选择站点属正常行为"), "{p}");
    }

    #[test]
    fn qc_prompt_target_consistency_absent_without_original_prompt() {
        let p = build_unit_qc_prompt("g", "scope", "exp", "act", "trace", None);
        assert!(!p.contains("目标一致性"), "无原始输入时不得注入该门: {p}");
    }

    #[test]
    fn parse_quality_report_from_block() {
        let text = r#"
```json
{
  "verdict": "pass",
  "source": "subagent",
  "issues": [],
  "suggestion": "",
  "retryable": false,
  "evidence": ""
}
```"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
    }

    // ========== P0 fail-closed 测试 ==========
    // 验证 JSON 解析失败时返回 Verdict::Fail + retryable=true
    // (替代历史的 fail-open 默认 Pass)

    #[test]
    fn parse_quality_report_no_json_fails_closed() {
        // 完全无 JSON → 应返回 Err(让上游 fail-closed 转为 Fail)
        let text = "这是一段不含 JSON 的自然语言,无法被解析为质检报告。";
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_truncated_json_fails_closed() {
        // JSON 被截断 / 不合法 → 应返回 Err
        let text = r#"以下是质检报告:
```json
{
  "verdict": "pass",
  "source": "subagent",
  "issues": [],
  // 后面被截断"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_missing_required_field_fails_closed() {
        // 缺 verdict 字段 → serde 反序列化失败 → Err
        let text = r#"```json
{
  "source": "subagent",
  "issues": [],
  "retryable": false
}
```"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("verdict"));
    }

    #[test]
    fn quality_report_fail_constructor_sets_retryable() {
        // 验证 fail-closed 路径所需的构造函数
        let r = QualityReport::fail(
            AgentRole::SubAgent,
            vec!["Quality 报告解析失败: 字段缺失".to_string()],
            "请确保 LLM 输出合法 JSON verdict/report 字段",
            true,
        );
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
        assert!(r.issues.iter().any(|i| i.contains("解析失败")));
        assert!(r.suggestion.contains("JSON"));
    }

    // ========== 2026-09-17 第 82+ 轮 P0-3 测试 ==========
    // 文本关键词降级:JSON 完全无法提取时,从正文关键词推断 verdict / retryable / source,
    // 避免 Provider 网关截断 verdict 字段导致整轮 fail。

    #[test]
    fn parse_quality_report_text_fallback_pass() {
        // 自然语言结论 + 「通过」关键词 → 降级 Pass
        let text = "经过核查,本单元所有子任务均完成,验收材料齐全,结论:通过";
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::SubAgent);
        assert!(r.issues[0].contains("JSON 解析失败"));
    }

    #[test]
    fn parse_quality_report_text_fallback_fail_with_retry() {
        // 含「失败」+「重试」 → 降级 Fail + retryable=true
        let text = "子任务超时,前置未达成,结论:失败,建议重试";
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
    }

    // ========== 第 163 轮:verdict/issues 一致性消解 ==========

    #[test]
    fn qc_pass_with_issues_is_reconciled_to_fail() {
        // 实测事故形态:Plan 的 QC verdict=pass / retryable=false,issues 却点名
        // 「depends_on 全部为空,未声明链式依赖」—— 已被点名的硬缺陷原样进了执行层。
        let text = r#"```json
{"verdict":"pass","source":"plan","issues":["wf-2~wf-5 存在数据流依赖,但 depends_on 全部为空,未声明链式依赖,并行调度时可能乱序执行"],"suggestion":"","retryable":false,"evidence":"5 个 WorkFlow 已落盘"}
```"#;
        let r = parse_quality_report(text, AgentRole::Plan).unwrap();
        assert_eq!(r.verdict, Verdict::Fail, "pass + 非空 issues 必须消解为 Fail");
        // retryable 仍采信模型判定(不引入新的价值判断)
        assert!(!r.retryable);
        assert_eq!(r.issues.len(), 1);
        assert!(r.evidence.contains("一致性消解"), "消解必须留痕: {}", r.evidence);
        // 模型原有证据不被覆盖
        assert!(r.evidence.contains("5 个 WorkFlow 已落盘"));
    }

    #[test]
    fn qc_pass_without_issues_is_untouched() {
        let text = r#"{"verdict":"pass","source":"subagent","issues":[],"suggestion":"","retryable":false,"evidence":"产物齐全"}"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert!(!r.evidence.contains("一致性消解"), "正常 Pass 不该被加证据串");
        assert_eq!(r.evidence, "产物齐全", "模型证据原样保留");
    }

    #[test]
    fn qc_text_fallback_pass_is_not_reconciled() {
        // 降级路径的 issues 装的是「JSON 解析失败」这类降级说明,不是缺陷,
        // 不能拿它否定降级出来的 Pass。
        let text = "经过核查,本单元所有子任务均完成,验收材料齐全,结论:通过";
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert!(r.issues[0].contains("JSON 解析失败"));
    }

    // ===== 2026-10-12:反向一致性消解(实测 llaew_20261012_212828.log 文心一言任务) =====

    /// 实测事故形态:QC 判 `fail` 但一条阻断缺陷都没给,issues 里装的是它自己
    /// 都标了「非阻断」的观察,模型还给了 `retryable=false` —— 旧规则只认
    /// pass+issues→Fail 一个方向,这份被"按措辞判死"的合格方案一路放行到
    /// 任务以 `outcome=failed` 收口(340s / 19 万 input token、零执行单元)。
    /// 新增反向消解:判失败却无阻断缺陷 → 按 notes 放行。
    #[test]
    fn qc_fail_without_blocking_issues_is_reconciled_to_pass() {
        let text = r#"```json
{"verdict":"fail","source":"main","issues":[],"notes":["非阻断-范围越界:建议把素材研读移出关键路径","非阻断-依赖链风险:素材目录不可读会导致对话链停摆"],"suggestion":"","retryable":false,"evidence":"三项阻断条件均不成立"}
```"#;
        let r = parse_quality_report(text, AgentRole::MainWork).unwrap();
        assert_eq!(
            r.verdict,
            Verdict::Pass,
            "fail + 空 issues(仅 notes)应反向消解为 Pass,不能按措辞杀方案"
        );
        assert!(r.evidence.contains("反向一致性消解"), "消解必须留痕: {}", r.evidence);
        assert!(r.evidence.contains("范围越界"), "notes 原文应留痕供回看");
        // 模型原有证据不被覆盖
        assert!(r.evidence.contains("三项阻断条件均不成立"));
        // retryable 仍完全采信模型
        assert!(!r.retryable);
    }

    /// 反向消解的边界:`fail` + 非空 issues =模型确实认定了阻断缺陷,保持 Fail。
    #[test]
    fn qc_fail_with_blocking_issues_stays_fail() {
        let text = r#"{"verdict":"fail","source":"main","issues":["wf-3 素材路径写成裸文件名,Read 必然 FileNotFound"],"notes":["措辞口径可再统一"],"suggestion":"改用绝对路径","retryable":true,"evidence":"已回读方案文件"}"#;
        let r = parse_quality_report(text, AgentRole::MainWork).unwrap();
        assert_eq!(r.verdict, Verdict::Fail, "有阻断缺陷时保持 Fail");
        assert!(
            !r.evidence.contains("一致性消解"),
            "本来就没有矛盾,不该加消解痕迹: {}",
            r.evidence
        );
        assert!(r.retryable);
    }

    /// 新旧两个方向互不干扰:pass+issues(有阻断缺陷)→ Fail;
    /// fail+空 issues → Pass;pass+空 issues → 原样 Pass。
    #[test]
    fn qc_reconcile_both_directions_are_covered() {
        let pass_clean = r#"{"verdict":"pass","source":"plan","issues":[],"notes":["建议补充阈值"],"retryable":false,"evidence":"结构齐全"}"#;
        assert_eq!(
            parse_quality_report(pass_clean, AgentRole::Plan).unwrap().verdict,
            Verdict::Pass
        );

        let pass_with_blocking = r#"{"verdict":"pass","source":"plan","issues":["depends_on 全部为空,存在硬缺陷"],"retryable":false,"evidence":"已落盘"}"#;
        assert_eq!(
            parse_quality_report(pass_with_blocking, AgentRole::Plan).unwrap().verdict,
            Verdict::Fail
        );

        let fail_without_blocking = r#"{"verdict":"fail","source":"plan","issues":[],"notes":["非阻断观察"],"retryable":false,"evidence":"已落盘"}"#;
        assert_eq!(
            parse_quality_report(fail_without_blocking, AgentRole::Plan).unwrap().verdict,
            Verdict::Pass
        );
    }

    /// QC 提示词必须说清 issues(阻断)与 notes(非阻断)的区别,
    /// 否则模型仍会沿用"pass + 观察写进 issues"的旧习惯(第 163 轮规则的误杀源)。
    #[test]
    fn qc_prompt_distinguishes_blocking_issues_from_notes() {
        assert!(
            QC_JSON_SHAPE_HINT.contains("\"notes\""),
            "schema 提示必须给出 notes 字段"
        );
        assert!(
            QC_JSON_SHAPE_HINT.contains("阻断性") && QC_JSON_SHAPE_HINT.contains("非阻断"),
            "schema 提示必须区分阻断/非阻断"
        );
    }

    /// 实测故障(2026-10-13 闲鱼任务,llaew_20261013_084832.log 09:51:26):
    /// 模型把 notes 输出成「内嵌 JSON 数组的字符串」,严格反序列化失败 →
    /// fail-closed 判 Fail → 合格方案被杀、任务以 failed 收口。
    /// 归一化后必须按数组展开,verdict/retryable 语义不受影响。
    #[test]
    fn qc_notes_as_embedded_json_array_string_is_normalized() {
        let text = r#"{"verdict":"pass","source":"main","issues":[],"notes":"[\"wf-1 占位\",\"wf-3 放宽口径未量化\"]","suggestion":"","retryable":false,"evidence":"结构齐全"}"#;
        let r = parse_quality_report(text, AgentRole::MainWork).unwrap();
        assert_eq!(r.verdict, Verdict::Pass, "notes 形状修复后不得 fail-closed");
        assert_eq!(r.notes.len(), 2, "内嵌 JSON 数组字符串应展开为 2 条 notes");
        assert!(r.notes[0].contains("占位"));
        assert!(!r.retryable);
    }

    /// 同上形态的 issues 字符串化也要容错(阻断缺陷不能因为形状问题丢失)。
    #[test]
    fn qc_issues_as_plain_string_is_wrapped() {
        let text = r#"{"verdict":"fail","source":"subagent","issues":"产物文件不存在","notes":"无","suggestion":"重新生成","retryable":true,"evidence":"Read 失败"}"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
        assert_eq!(r.issues, vec!["产物文件不存在".to_string()]);
        assert_eq!(r.notes, vec!["无".to_string()]);
        assert!(r.retryable);
    }

    /// suggestion/evidence 输出成数组或数字时同样兜底,不再整条作废。
    #[test]
    fn qc_suggestion_and_evidence_shape_normalized() {
        let text = r#"{"verdict":"pass","source":"plan","issues":[],"notes":[],"suggestion":["改用绝对路径","补充验收"],"retryable":false,"evidence":123}"#;
        let r = parse_quality_report(text, AgentRole::Plan).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.suggestion, "改用绝对路径 补充验收");
        assert_eq!(r.evidence, "123");
    }

    /// null 形状归一化为缺省空数组,不阻断解析。
    #[test]
    fn qc_null_array_fields_default_to_empty() {
        let text = r#"{"verdict":"pass","source":"plan","issues":null,"notes":null,"suggestion":null,"retryable":false,"evidence":null}"#;
        let r = parse_quality_report(text, AgentRole::Plan).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert!(r.issues.is_empty() && r.notes.is_empty());
        assert!(r.suggestion.is_empty() && r.evidence.is_empty());
    }

    #[test]
    fn parse_quality_report_text_fallback_no_strong_signal_fails_closed() {        // 无强信号关键词 → 仍 fail-closed(避免误判)
        let text = "今天天气很好,适合出门散步";
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_text_fallback_source_inference() {
        // 含「quality_check」→ 推断 source=QualityCheck(即便传入其它角色)
        let text = "QC pass:quality_check 单元所有动作完成";
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::QualityCheck);
    }

    // ========== 2026-09-18 第 87 轮 ==========
    // JSON 块提取成功但顶层畸形(缺 verdict)时,不再立即 Err ——
    // 继续走文本关键词降级,正文里的 fail 信号不应被浪费(实测:QC 输出
    // `{"$text":...}` 畸形 JSON 触发无谓回流)。

    #[test]
    fn parse_quality_report_malformed_json_falls_back_to_text_keywords() {
        let text = r#"```json
{
  "$text": "subagent 自我归因错误:声称工具未暴露,但日志显示已真实执行",
  "item": {"$text": "期望输出缺失:发送记录 0 条,任务失败,建议重试"}
}
```"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
        assert!(r.issues[0].contains("JSON 解析失败"));
    }

    #[test]
    fn parse_quality_report_malformed_json_without_signal_still_fails_closed() {
        // 畸形 JSON 且正文无 verdict 强信号 → 仍 fail-closed Err(语义不变)
        let text = r#"```json
{
  "$text": "今天天气很好",
  "item": {"$text": "适合出门散步"}
}
```"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("verdict"));
    }

    #[test]
    fn trace_gate_rejects_empty_evidence_for_bash_nonzero() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 1;
        trace.collect_failure_signals("完成");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "完成",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.retryable);
        assert!(gated.evidence.contains("bash_exit_nonzero=1"));
    }

    #[test]
    fn trace_gate_allows_expected_bash_nonzero_with_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 2;
        trace.collect_failure_signals("负向用例按预期返回非零");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "exit=2 是本负向用例的预期结果".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "完成");

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_strong_failure_even_with_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.early_terminated = true;
        trace.early_terminate_reason = "max_iter:2".into();
        trace.collect_failure_signals("完成");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "模型解释这是预期情况".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "完成");

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("强失败信号"));
    }

    #[test]
    fn trace_gate_allows_explicit_negative_confirmation_without_qc_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 0;
        trace.collect_failure_signals("EXPECTED_NEGATIVE_OK");

        let output = "工具输出摘录\nEXPECTED_NEGATIVE_OK\n<exit_code>0</exit_code>";
        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            output,
        );

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_negative_marker_without_final_success() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 1;
        trace.collect_failure_signals("EXPECTED_NEGATIVE_OK");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "EXPECTED_NEGATIVE_OK",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("Bash 命令非零退出"));
    }

    #[test]
    fn trace_gate_allows_text_phrase_with_expected_negative_contract() {
        // LA-3:终答摘录引用了 AssertionError 日志(误触发 text_failure_phrase),
        // 但预期负例契约确认(先非零复现 + 最终断言命令 exit=0 + EXPECTED_NEGATIVE_OK)
        // 应保持 pass,不应反复回流重试。
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 0;
        trace.collect_failure_signals(
            "MOCK_FINAL_ANSWER: 验证通过。\n\n[工具输出摘录]\nAssertionError: producer error: 有 queue.Full 未处理",
        );
        assert!(trace
            .failure_signals
            .iter()
            .any(|s| s == "text_failure_phrase"));

        let output = "AssertionError 摘录\nEXPECTED_NEGATIVE_OK\n<exit_code>0</exit_code>";
        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            output,
        );

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_text_phrase_without_contract_or_evidence() {
        // LA-3:纯文本失败措辞(无契约确认、无 QC evidence)仍必须拒绝,
        // 防止模型口头上声称成功、引用失败日志蒙混过关。
        let mut trace = ExecutionTrace::default();
        trace.collect_failure_signals("执行失败: 无法完成队列写入");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "任务完成",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("失败措辞"));
    }

    #[test]
    fn trace_gate_allows_text_phrase_with_qc_evidence() {
        // LA-3:QC 用非空 evidence 说明"该失败措辞来自预期负例日志摘录"→ 放行
        let mut trace = ExecutionTrace::default();
        trace.collect_failure_signals("AssertionError: 预期复现摘录");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "终答中的 AssertionError 是负例脚本预期输出,断言已确认捕获".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "任务完成");

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_hard_failure_still_rejects_with_text_phrase_present() {
        // LA-3:early_terminate 与 text_failure_phrase 并存时按进程级硬信号无条件拒绝
        let mut trace = ExecutionTrace::default();
        trace.early_terminated = true;
        trace.early_terminate_reason = "max_iter:2".into();
        trace.collect_failure_signals("[MaxIterationsExceeded] 迭代达到 2 次上限未得到最终答案");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "模型解释这是预期情况".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "任务完成");

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("强失败信号"));
    }

    // ========== 2026-09-20 第 99 轮:verdict 缺失语义修复测试 ==========
    // 实测场景:QC LLM 输出合法 JSON 但漏 verdict → 反序列化失败 → fail-closed
    // 白耗一次局部重试预算。修复链在语法修复前按信号推断补字段。

    #[test]
    fn repair_quality_report_missing_verdict_with_result_string() {
        let text = r#"```json
{
  "source": "subagent",
  "result": "fail",
  "issues": ["未执行切换子账号操作"],
  "suggestion": "先 click 切换子账号 Tab"
}
```"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable, "verdict 推断为 fail 时 retryable 应补 true");
        assert!(r.issues.iter().any(|i| i.contains("子账号")));
    }

    #[test]
    fn repair_quality_report_missing_verdict_with_pass_bool() {
        let text = r#"{"pass": true, "issues": [], "retryable": false}"#;
        let r = parse_quality_report(text, AgentRole::MainWork).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::MainWork, "缺失 source 应被调用方补齐");
    }

    #[test]
    fn repair_quality_report_missing_verdict_with_nonempty_issues() {
        // issues 非空 + 无其它信号 → 推断 Fail
        let text = r#"{"issues": ["登录按钮未点击"], "suggestion": "重试"}"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
    }

    #[test]
    fn repair_quality_report_missing_all_signals_still_fails_closed() {
        // 无任何可推断信号 → 保持 fail-closed Err(不误判)
        let text = r#"{"source": "subagent", "issues": [], "retryable": false}"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("verdict"));
    }

    #[test]
    fn repair_quality_report_conclusion_chinese_keyword() {
        let text = r#"{"conclusion": "验收通过", "evidence": "菜单已抓取"}"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
    }

    #[test]
    fn quality_prompt_contains_json_shape_hint() {
        // 提示词硬化:三个 QC 提示词都必须携带 JSON 形状硬性要求
        let p = build_unit_qc_prompt("目标", "职责", "期望", "实际", "执行轨迹", None);
        assert!(p.contains("第一个字段必须是 verdict"), "单元 QC 提示词应含形状提示");
    }

    // ==================== 死循环硬门 + 根因裁决 ====================

    fn 样例诊断() -> crate::agent::dead_loop::DeadLoopDiagnosis {
        crate::agent::dead_loop::DeadLoopDiagnosis {
            kind: crate::agent::dead_loop::LoopKind::ShortCycle,
            period: Some(2),
            reps: 3,
            turns: 8,
            distinct_inputs: 3,
            distinct_outputs: 2,
            cycle_turns: vec![4, 5, 6, 7],
            sample_in: "采集结果:区块 A".into(),
            sample_out: "检查区块 A".into(),
            tools: vec!["Read".into()],
        }
    }

    #[test]
    fn 无诊断时死循环硬门段为空() {
        assert!(build_dead_loop_gate(None).is_empty(), "无诊断不得改变提示词");
    }

    #[test]
    fn 死循环硬门段含枚举与形态() {
        let g = build_dead_loop_gate(Some(&样例诊断()));
        assert!(g.contains("死循环硬门"), "{g}");
        for e in ["same_action", "same_target", "context_stuck", "budget_starved"] {
            assert!(g.contains(e), "硬门段必须给出根因枚举 {e}");
        }
        assert!(g.contains("一律判 Fail"), "必须写死判定规则:{g}");
        assert!(g.contains("最小周期 = 2"), "必须带诊断明细:{g}");
    }

    #[test]
    fn 单元提示词携带死循环硬门段() {
        let p = build_unit_qc_prompt_full(
            "目标",
            "职责",
            "期望",
            "实际",
            "执行轨迹",
            None,
            &build_dead_loop_gate(Some(&样例诊断())),
        );
        assert!(p.contains("死循环硬门"), "{p}");
    }

    #[test]
    fn 死循环硬门拦截通过() {
        // 进程级事实:轨迹带诊断 + QC 判 pass → 机械降级为 Fail。
        let mut t = ExecutionTrace::default();
        t.dead_loop = Some(样例诊断());
        let r = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &t,
            "我已完成全部任务",
        );
        assert_eq!(r.verdict, Verdict::Fail, "死循环诊断不得被 pass 覆盖");
        assert!(r.retryable);
        assert!(r.issues.iter().any(|i| i.contains("死循环")), "{:?}", r.issues);
        assert!(r.evidence.contains("dead_loop"), "evidence 须留完整诊断");
    }

    #[test]
    fn 死循环硬门不覆盖失败结论() {
        // QC 已经判 fail 时,机械门不做任何改写(不污染 issues / evidence)。
        let mut t = ExecutionTrace::default();
        t.dead_loop = Some(样例诊断());
        let r = gate_report_on_trace(
            QualityReport::fail(AgentRole::SubAgent, vec!["产物缺失".into()], "重做", false),
            AgentRole::SubAgent,
            &t,
            "",
        );
        assert_eq!(r.issues, vec!["产物缺失".to_string()]);
        assert!(!r.retryable, "机械门不得改写模型的 retryable 判定");
    }

    #[test]
    fn 根因枚举解析() {
        assert_eq!(LoopRootCause::parse("same_action"), Some(LoopRootCause::SameAction));
        assert_eq!(LoopRootCause::parse(" Same_Target "), Some(LoopRootCause::SameTarget));
        assert_eq!(LoopRootCause::parse("context_stuck"), Some(LoopRootCause::ContextStuck));
        assert_eq!(LoopRootCause::parse("budget_starved"), Some(LoopRootCause::BudgetStarved));
        assert_eq!(LoopRootCause::parse("随便编一个"), None);
    }

    #[test]
    fn 根因裁决解析正常输出() {
        let t = r#"{"root_cause":"same_action","retry_path":"改用 Grep 定位符号定义","retryable":true,"rationale":"连续 Read 同一路径"}"#;
        let r = parse_loop_resolution(t).expect("应解析成功");
        assert_eq!(r.root_cause, LoopRootCause::SameAction);
        assert!(r.retry_path.contains("Grep"));
        assert!(r.retryable);
    }

    #[test]
    fn 根因裁决解析代码块包裹() {
        let t = r#"分析如下:
```json
{"root_cause":"context_stuck","retry_path":"直接收口"}
```"#;
        let r = parse_loop_resolution(t).expect("应从代码块提取");
        assert_eq!(r.root_cause, LoopRootCause::ContextStuck);
        assert!(r.retryable, "context_stuck 是路径选择问题,应可重试");
    }

    #[test]
    fn 根因裁决缺retryable时按根因推导() {
        let t = r#"{"root_cause":"same_target","retry_path":"换到列表页"}"#;
        let r = parse_loop_resolution(t).expect("应解析成功");
        assert!(!r.retryable, "same_target 天然不可重试");
    }

    #[test]
    fn 根因裁决解析失败返回空() {
        assert!(parse_loop_resolution("完全不是 JSON").is_none());
        assert!(parse_loop_resolution(r#"{"root_cause":"编的","retry_path":"x"}"#).is_none());
        // 空 retry_path 视为无效输出(只写空话等于没给换路径)
        assert!(parse_loop_resolution(r#"{"root_cause":"same_action","retry_path":"  "}"#).is_none());
    }

    #[test]
    fn 机械兜底按是否调用过工具归因() {
        let mut d = 样例诊断();
        d.tools = vec!["Read".into()];
        let r = LoopResolution::mechanical_fallback(&d);
        assert_eq!(r.root_cause, LoopRootCause::SameAction);
        assert!(r.retryable, "兜底不得替模型做不可重试判断");
        assert!(r.retry_path.contains("Read"), "{}", r.retry_path);

        let mut d2 = 样例诊断();
        d2.tools.clear();
        let r2 = LoopResolution::mechanical_fallback(&d2);
        assert_eq!(r2.root_cause, LoopRootCause::ContextStuck);
        assert!(r2.retry_path.contains("收口"), "{}", r2.retry_path);
    }

    #[test]
    fn 裁决结论并入常规报告() {
        let mut rep = QualityReport::fail(AgentRole::SubAgent, vec!["i1".into()], "常规建议", false);
        LoopResolution {
            root_cause: LoopRootCause::SameAction,
            retry_path: "换工具重来".into(),
            retryable: true,
            rationale: "依据".into(),
        }
        .merge_into(&mut rep);
        assert!(rep.suggestion.contains("换工具重来"), "换路径须前置");
        assert!(rep.suggestion.contains("常规建议"), "常规建议须保留");
        assert!(rep.retryable, "retryable 取或,不得让有救的单元被判死");
    }

    #[test]
    fn 根因裁决提示词要求具体动作() {
        let p = build_dead_loop_prompt("职责", "期望", "实际", &样例诊断());
        assert!(p.contains("same_action"), "{p}");
        assert!(p.contains("禁止"), "必须写明禁止空话:{p}");
        assert!(p.contains("root_cause"), "{p}");
    }
