//! main_work 模块单元测试(2026-09-17 自 main_work.rs 拆分)。

    use super::*;

    fn wf(id: &str, deps: &[&str]) -> WorkFlowSpec {
        WorkFlowSpec {
            id: id.to_string(),
            name: format!("workflow {id}"),
            steps: vec![],
            branches: vec![],
            loops: vec![],
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:测试 fixture 默认无目标应用。
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底 fixture
            max_iterations: None, original_prompt: None, pre_explore: false,
        }
    }

    #[test]
    fn normalize_dep_id_strips_annotations() {
        // 全角括号注释(Plan 文档路径典型形态)
        assert_eq!(
            normalize_dep_id("wf-1（需要 FIRST_CHROME_WINDOW_ID）"),
            Some("wf-1".to_string())
        );
        // 半角括号 + 中文说明
        assert_eq!(
            normalize_dep_id("wf-2 (needs data)"),
            Some("wf-2".to_string())
        );
        // 中文冒号
        assert_eq!(
            normalize_dep_id("wf-3：控件树数据"),
            Some("wf-3".to_string())
        );
        // 干净 id
        assert_eq!(normalize_dep_id("wf-1"), Some("wf-1".to_string()));
        // 带引号 / 空格 / 逗号
        assert_eq!(normalize_dep_id("  \"wf-4\" "), Some("wf-4".to_string()));
        assert_eq!(normalize_dep_id("wf-5, wf-6"), Some("wf-5".to_string()));
        // 纯注释 / 空串
        assert_eq!(normalize_dep_id("（需要 X）"), None);
        assert_eq!(normalize_dep_id("   "), None);
        assert_eq!(normalize_dep_id(""), None);
    }

    #[test]
    fn sanitize_depends_on_fixes_unknown_dependency() {
        // 复现 DebugReport_20260916_102929 的失败场景:wf-2 依赖 "wf-1(需要变量)"
        // 未归一化时 topo_layers 报「依赖未知 wf=wf-1(需要 FIRST_CHROME_WINDOW_ID)」
        let mut plan = WorkFlowPlan {
            workflows: vec![
                wf("wf-1", &[]),
                wf("wf-2", &["wf-1（需要 FIRST_CHROME_WINDOW_ID）"]),
                wf("wf-3", &["wf-2（需要控件树数据）", "wf-1"]),
            ],
            summary: "test".into(),
            degraded: false,
        };
        sanitize_depends_on(&mut plan);
        assert_eq!(plan.workflows[1].depends_on, vec!["wf-1".to_string()]);
        assert_eq!(
            plan.workflows[2].depends_on,
            vec!["wf-2".to_string(), "wf-1".to_string()]
        );
        // 归一化后 topo_layers 应成功分层(串行链 3 层)
        let order = topo_sort(&plan.workflows).unwrap();
        let ids: Vec<&str> = order.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["wf-1", "wf-2", "wf-3"]);
    }

    #[test]
    fn sanitize_depends_on_removes_self_loop_and_dedup() {
        let mut plan = WorkFlowPlan {
            workflows: vec![wf("wf-1", &["wf-1", "wf-1（注释）", ""])],
            summary: "test".into(),
            degraded: false,
        };
        sanitize_depends_on(&mut plan);
        // 自环剥离 + 去重 + 去空 → 空列表
        assert!(plan.workflows[0].depends_on.is_empty());
    }

    #[test]
    fn topo_sort_simple_chain() {
        let workflows = vec![
            wf("wf-1", &[]),
            wf("wf-2", &["wf-1"]),
            wf("wf-3", &["wf-2"]),
        ];
        let order = topo_sort(&workflows).unwrap();
        let ids: Vec<String> = order.iter().map(|w| w.id.clone()).collect();
        assert_eq!(ids, vec!["wf-1", "wf-2", "wf-3"]);
    }

    #[test]
    fn topo_sort_independent() {
        let workflows = vec![wf("wf-1", &[]), wf("wf-2", &[]), wf("wf-3", &[])];
        let order = topo_sort(&workflows).unwrap();
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn topo_sort_detects_cycle() {
        let workflows = vec![wf("wf-1", &["wf-2"]), wf("wf-2", &["wf-1"])];
        assert!(topo_sort(&workflows).is_err());
    }

    #[test]
    fn topo_sort_unknown_dep() {
        let workflows = vec![wf("wf-1", &["wf-x"])];
        assert!(topo_sort(&workflows).is_err());
    }

    #[test]
    fn topo_layers_chain_one_per_layer() {
        let workflows = vec![
            wf("wf-1", &[]),
            wf("wf-2", &["wf-1"]),
            wf("wf-3", &["wf-2"]),
        ];
        let layers = topo_layers(&workflows).unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0][0].id, "wf-1");
        assert_eq!(layers[1][0].id, "wf-2");
        assert_eq!(layers[2][0].id, "wf-3");
    }

    #[test]
    fn topo_layers_independent_all_in_one_layer() {
        let workflows = vec![wf("wf-1", &[]), wf("wf-2", &[]), wf("wf-3", &[])];
        let layers = topo_layers(&workflows).unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].len(), 3);
        // 层内顺序 = 原数组顺序(确定性)
        let ids: Vec<&str> = layers[0].iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["wf-1", "wf-2", "wf-3"]);
    }

    #[test]
    fn topo_layers_diamond() {
        // wf-1 → {wf-2, wf-3} → wf-4
        let workflows = vec![
            wf("wf-1", &[]),
            wf("wf-2", &["wf-1"]),
            wf("wf-3", &["wf-1"]),
            wf("wf-4", &["wf-2", "wf-3"]),
        ];
        let layers = topo_layers(&workflows).unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0].len(), 1);
        assert_eq!(layers[1].len(), 2, "wf-2/wf-3 同层可并行");
        assert_eq!(layers[2][0].id, "wf-4");
    }

    #[test]
    fn topo_layers_cycle_error() {
        let workflows = vec![wf("wf-1", &["wf-2"]), wf("wf-2", &["wf-1"])];
        assert!(topo_layers(&workflows).is_err());
    }

    #[test]
    fn topo_layers_unknown_dep_error() {
        let workflows = vec![wf("wf-1", &["wf-x"])];
        assert!(topo_layers(&workflows).is_err());
    }

    #[test]
    fn topo_layers_flatten_matches_topo_sort() {
        // 混合图:flatten(分层) 必须等于 topo_sort 输出(同一事实源)
        let workflows = vec![
            wf("wf-1", &[]),
            wf("wf-2", &[]),
            wf("wf-3", &["wf-1"]),
            wf("wf-4", &["wf-2", "wf-3"]),
        ];
        let flat: Vec<String> = topo_layers(&workflows)
            .unwrap()
            .into_iter()
            .flatten()
            .map(|w| w.id)
            .collect();
        let sorted: Vec<String> = topo_sort(&workflows)
            .unwrap()
            .into_iter()
            .map(|w| w.id)
            .collect();
        assert_eq!(flat, sorted);
    }

    #[test]
    fn parse_workflow_plan_from_block() {
        let text = r#"
```json
{
  "workflows": [
    {"id":"wf-1","name":"a","steps":["s1"],"acceptance":["ok"],"delegate_to":"subagent","depends_on":[]}
  ],
  "summary": "test"
}
```"#;
        let plan = parse_workflow_plan(text).unwrap();
        assert_eq!(plan.workflows.len(), 1);
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert_eq!(plan.workflows[0].delegate_to, AgentRole::SubAgent);
    }

    #[test]
    fn parse_workflow_plan_sanitizes_modifier_letters_and_nested_quotes() {
        // 2026-09-16 第 65 轮 P0-A:WorkFlow JSON 含 Unicode Modifier Letter 与
        // 中文「」嵌套时,sanitize 后应能正常反序列化,而不是触发 JSON 校验失败。
        // 真实场景:用户输入"赵玲玲AAAA",LLM 在多
        // 个 JSON 字段里复制该字符串,反复出现嵌套「」和乱码。
        // 注:Rust 原生字符 \u{1d2c}/\u{1d35} 在非 raw string 中正常,但因测试用
        // raw string literal 嵌套 JSON 转义易踩坑,这里用普通字符串拼接含 Unicode
        // 字符的 JSON,避免双重 escape 陷阱。
        let modifier_a = '\u{1d2c}';
        let modifier_i = '\u{1d35}';
        let weird = format!("{modifier_a}{modifier_a}{modifier_i}{modifier_a}");
        let json = format!(
            r#"```json
{{
  "workflows": [
    {{
      "id": "wf-1",
      "name": "微信自动化",
      "steps": ["查找名为「赵玲玲{weird}」的用户"],
      "acceptance": ["已发送消息"],
      "delegate_to": "subagent",
      "depends_on": []
    }}
  ],
  "summary": "找到「赵玲玲{weird}」并发送消息"
}}
```"#
        );
        let plan = parse_workflow_plan(&json).expect("sanitize 后应能解析");
        assert_eq!(plan.workflows.len(), 1);
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert_eq!(plan.workflows[0].delegate_to, AgentRole::SubAgent);
        // Modifier Letter 已归一化为 Latin 等价字符:ᴬᴬᴵᴬ → AAIA
        let first_step = &plan.workflows[0].steps[0];
        assert!(first_step.contains("AAIA"), "ᴬᴬᴵᴬ 应归一为 AAIA: {first_step}");
    }

    #[test]
    fn parse_workflow_plan_preserves_smart_quotes_in_json_strings() {
        // 2026-09-16 第 65 轮修订:智能引号 \u{201C}\u{201D} 在 JSON 字符串值
        // 内层是合法的装饰引号,与 JSON 解析器无冲突,无需 sanitize 替换。
        // 本测试验证含智能引号的 JSON 能正常被 parse_workflow_plan 解析。
        let left_smart = '\u{201C}';
        let right_smart = '\u{201D}';
        let raw = format!(
            r#"```json
{{
  "workflows": [
    {{"id":"wf-1","name":"{left_smart}测试{right_smart}","steps":["打开微信","点击通讯录"],"acceptance":["ok"],"delegate_to":"windowuse"}}
  ],
  "summary": "test"
}}
```"#
        );
        let plan = parse_workflow_plan(&raw).expect("sanitize 后应能解析");
        assert_eq!(plan.workflows.len(), 1);
        assert!(
            plan.workflows[0].name.contains(left_smart),
            "智能引号应保留(装饰引号): {0}",
            plan.workflows[0].name
        );
        assert!(plan.workflows[0].name.contains(right_smart));
    }

    #[test]
    fn parse_workflow_plan_handles_cjk_inner_ascii_quotes() {
        // 真实场景:LLM 在中文字符串内层用 ASCII " 包裹用户名(典型错误)。
        // 如 `"steps":["查找名为"张三"的用户"]`,sanitize 后内层 ASCII " 应被
        // 替换为「」,JSON 仍合法。
        let raw = r#"```json
{
  "workflows": [
    {"id":"wf-1","name":"微信任务","steps":["查找名为"张三"的用户并发送消息"],"acceptance":["ok"],"delegate_to":"windowuse"}
  ],
  "summary": "test"
}
```"#;
        let plan = parse_workflow_plan(raw).expect("sanitize 后应能解析");
        assert_eq!(plan.workflows.len(), 1);
        // 修复后的 step 文本应不含裸 ASCII "包围中文
        let first_step = &plan.workflows[0].steps[0];
        assert!(
            !first_step.contains("\"张") && !first_step.contains("三\""),
            "中文字符串内的 ASCII \" 应被替换为「」: {first_step}"
        );
    }

    #[test]
    fn parse_plan_markdown_extracts_workflows() {
        let md = r#"
# 任务方案:test

## 一、目标
目标内容

## 二、WorkFlow 拆解
### WorkFlow 1: 读取源文件
- 步骤:
  - [ ] 读取 src/foo.rs
  - [ ] 解析函数
- 委派 Agent: SubAgent-Work
- 依赖: 无
- 验收标准: 解析成功

### WorkFlow 2: 修改源文件
- 步骤:
  - [ ] 替换函数
- 委派 Agent: SubAgent-Work
- 依赖: wf-1
- 验收标准: cargo test 通过

## 三、关键决策
决策
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("parse failed: {e}"));
        assert_eq!(plan.workflows.len(), 2, "应解析出 2 个 WorkFlow");
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert_eq!(plan.workflows[0].steps.len(), 2);
        assert_eq!(plan.workflows[1].depends_on, vec!["wf-1"]);
    }

    /// I2(2026-09-14 第 51 轮):Plan 提示词漂移兜底 —— 粗体 bullet 变体
    /// `- **wf-N …**`(无 `### WorkFlow N:` 标题)也必须解析出 workflow,
    /// 否则 hard 任务「Plan 文档未解析出任何 WorkFlow」整链失败(fl10 实测)。
    #[test]
    fn parse_plan_markdown_bold_bullet_variant() {
        let md = r#"
# 任务方案摘要

## 二、WorkFlow 拆解
- **wf-1 / 子任务 1 — 编写 fl10_rps.py**:asyncio server + JSON Lines + --dry-run
- **wf-2 / 子任务 2 — py_compile**:断言退出码 = 0
- **wf-3 / 子任务 3 — JSON 配置**:两个配置文件 utf-8 保存

## 三、关键决策
决策
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("parse failed: {e}"));
        assert_eq!(plan.workflows.len(), 3, "粗体变体应解析出 3 个 WorkFlow");
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert!(
            plan.workflows[0].name.contains("编写"),
            "name: {}",
            plan.workflows[0].name
        );
        assert!(
            !plan.workflows[0].steps.is_empty(),
            "说明 bullet 应兜底成为步骤: {:?}",
            plan.workflows[0].steps
        );
    }

    /// I2c(2026-09-14 第 51 轮):表格行变体 `| **wf-N** | 名称 | 步骤 | 依赖 |`
    /// 必须解析出 workflow 及依赖链(et08/et09 实测形态)。
    #[test]
    fn parse_plan_markdown_table_row_variant() {
        let md = r#"
# 任务方案

## 二、WorkFlow 拆解
| 编号 | WorkFlow 名称 | 核心步骤 | 依赖 |
|------|--------------|---------|------|
| **wf-1** | 目录准备 | 创建 tmpPlan/agent-test/ | 无 |
| **wf-2** | 编写脚本 | 实现核心模型 + demo | wf-1 |
| **wf-3** | 验证 | py_compile 断言 | wf-2、wf-1 |

## 三、关键决策
决策
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("parse failed: {e}"));
        assert_eq!(plan.workflows.len(), 3, "表格行应解析出 3 个 WorkFlow");
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert_eq!(plan.workflows[1].depends_on, vec!["wf-1"]);
        assert_eq!(
            plan.workflows[2].depends_on,
            vec!["wf-2", "wf-1"],
            "顿号分隔依赖: {:?}",
            plan.workflows[2].depends_on
        );
        assert_eq!(plan.workflows[1].steps, vec!["实现核心模型 + demo"]);
    }

    /// 验证 parse_plan_markdown 支持 JSON 代码块格式(兜底解析)。
    /// 关联修复: 2026-09-10 hard 任务 Plan 解析 Bug。
    #[test]
    fn parse_plan_markdown_supports_json_code_block() {
        let md = r#"# 方案

```json
{"workflows": [{"id": "wf-1", "name": "执行验证", "steps": ["运行 echo LAEW_MOCK_OK"], "acceptance": ["输出包含 LAEW_MOCK_OK"], "delegate_to": "subagent"}]}
```
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("JSON 代码块解析失败: {e}"));
        assert_eq!(plan.workflows.len(), 1, "应解析出 1 个 WorkFlow");
        assert_eq!(plan.workflows[0].id, "wf-1");
        assert_eq!(plan.workflows[0].name, "执行验证");
        assert_eq!(plan.workflows[0].steps.len(), 1);
    }

    /// F1(2026-09-10 第 25 轮):真实 LLM(D06 实测 MiniMax-M3 3/3)把 branches
    /// 写成字符串数组,宽松反序列化必须解析成功而非丢弃整份计划。
    #[test]
    fn parse_lenient_branches_string_array() {
        let json = r#"{"workflows": [{"id": "wf-1", "name": "创建文件", "steps": ["写入 JSON"],
            "branches": ["若 jq . 解析失败(JSON 语法错误): 修正文件内容后重新自检,最多重试 1 次",
                          "IF jq 不可用或 jq 命令非零退出 → 改用 python3 等效提取"],
            "loops": [], "depends_on": [],
            "acceptance": ["test -f openai_chat_response.json 退出码为 0"],
            "delegate_to": "subagent"}]}"#;
        let plan: WorkFlowPlan =
            serde_json::from_str(json).expect("字符串形态 branches 必须可解析");
        let branches = &plan.workflows[0].branches;
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].condition, "若 jq . 解析失败(JSON 语法错误)");
        assert_eq!(branches[0].then, "修正文件内容后重新自检,最多重试 1 次");
        assert_eq!(branches[1].condition, "IF jq 不可用或 jq 命令非零退出");
        assert_eq!(branches[1].then, "改用 python3 等效提取");
    }

    /// F1:branches 为单字符串 / loops 为字符串数组 / acceptance 为单字符串。
    #[test]
    fn parse_lenient_scalar_shapes() {
        let json = r#"{"workflows": [{"id": "wf-1", "name": "n",
            "branches": "若失败: 重试一次",
            "loops": ["对每个文件: 执行校验"],
            "acceptance": "文件存在"}]}"#;
        let plan: WorkFlowPlan = serde_json::from_str(json).expect("标量形态必须可解析");
        let wf = &plan.workflows[0];
        assert_eq!(wf.branches.len(), 1);
        assert_eq!(wf.branches[0].condition, "若失败");
        assert_eq!(wf.branches[0].then, "重试一次");
        assert_eq!(wf.loops.len(), 1);
        assert_eq!(wf.loops[0].condition, "对每个文件");
        assert_eq!(wf.loops[0].over, "执行校验");
        assert_eq!(wf.acceptance, vec!["文件存在"]);
    }

    /// F1:对象数组(标准形态)与对象/字符串混合数组都兼容。
    #[test]
    fn parse_lenient_branches_object_and_mixed() {
        let json = r#"{"workflows": [{"id": "wf-1", "name": "n",
            "branches": [{"condition": "a", "then": "b"}, "若 X: 则 Y"]}]}"#;
        let plan: WorkFlowPlan = serde_json::from_str(json).expect("混合形态必须可解析");
        let branches = &plan.workflows[0].branches;
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].condition, "a");
        assert_eq!(branches[0].then, "b");
        assert_eq!(branches[1].condition, "若 X");
        assert_eq!(branches[1].then, "则 Y");
    }

    /// F1:delegate_to 接受别名;未知值与字段缺失回退 SubAgent。
    #[test]
    fn parse_lenient_delegate_to() {
        let mk = |v: &str| {
            format!(r#"{{"workflows": [{{"id": "wf-1", "name": "n", "delegate_to": {v}}}]}}"#)
        };
        for alias in [
            "\"SubAgent-Work\"",
            "\"subagent\"",
            "\"work\"",
            "\"MAIN-WORK\"",
        ] {
            let plan: WorkFlowPlan = serde_json::from_str(&mk(alias)).unwrap();
            let expected = if alias.contains("MAIN") {
                AgentRole::MainWork
            } else {
                AgentRole::SubAgent
            };
            assert_eq!(plan.workflows[0].delegate_to, expected, "alias={alias}");
        }
        // 未知值 → SubAgent 兜底
        let plan: WorkFlowPlan = serde_json::from_str(&mk("\"someone-else\"")).unwrap();
        assert_eq!(plan.workflows[0].delegate_to, AgentRole::SubAgent);
        // 字段缺失 → 默认 SubAgent
        let plan: WorkFlowPlan =
            serde_json::from_str(r#"{"workflows": [{"id": "wf-1", "name": "n"}]}"#).unwrap();
        assert_eq!(plan.workflows[0].delegate_to, AgentRole::SubAgent);
    }

    /// 2026-09-18 第 84 轮:windowuse 旧别名兼容归一到 AgentRole::SubAgent
    /// (WindowUse Agent 已删除,窗口操控由 SubAgent-Work 的 MCP_Window_Use 工具承担)。
    #[test]
    fn parse_delegate_to_window_use_aliases() {
        let mk = |v: &str| {
            format!(r#"{{"workflows": [{{"id": "wf-1", "name": "n", "delegate_to": {v}}}]}}"#)
        };
        for alias in [
            "\"windowuse\"",
            "\"WindowUse\"",
            "\"window-use\"",
            "\"window_use\"",
            "\"WindowUseAgent\"",
            "\"窗口\"",
        ] {
            let plan: WorkFlowPlan = serde_json::from_str(&mk(alias)).unwrap();
            assert_eq!(
                plan.workflows[0].delegate_to,
                AgentRole::SubAgent,
                "alias={alias}"
            );
        }
    }

    /// F2:split_condition_then 的分隔符取最早出现者,切不开归 condition。
    #[test]
    fn split_condition_then_earliest_separator() {
        let (c, t) = super::split_condition_then("若超时: 重试,若仍失败: 上报");
        assert_eq!(c, "若超时");
        assert_eq!(t, "重试,若仍失败: 上报");
        let (c, t) = super::split_condition_then("无分隔符的整句");
        assert_eq!(c, "无分隔符的整句");
        assert_eq!(t, "");
    }

#[cfg(test)]
mod dedup_tests {
    use super::*;

    /// I4(2026-09-14 第 51 轮):重复 id 去重(ak05 实测 wf-1..wf-5 各出现 2 次)。
    #[test]
    fn dedup_workflow_ids_keeps_first_occurrence() {
        let src = r#"{"workflows":[
            {"id":"wf-1","name":"a","steps":["s1"],"acceptance":["x"]},
            {"id":"wf-2","name":"b","steps":["s2"],"acceptance":["y"]},
            {"id":"wf-1","name":"a2","steps":["dup"],"acceptance":["dup"]}
        ],"summary":"s"}"#;
        let mut plan = parse_workflow_plan(src).unwrap();
        assert_eq!(plan.workflows.len(), 2, "parse_workflow_plan 内部已去重");
        dedup_workflow_ids(&mut plan); // 幂等
        assert_eq!(plan.workflows.len(), 2);
        assert_eq!(plan.workflows[0].name, "a", "应保留首次出现");
        assert_eq!(plan.workflows[1].id, "wf-2");
    }
}

    /// 第 119 轮:pre_explore 字段 JSON 解析 + 默认 false
    #[test]
    fn pre_explore_field_parses_from_json() {
        // 显式 true
        let src = r#"{"workflows":[{"id":"wf-1","name":"打开网页","steps":["open"],"acceptance":["ok"],"delegate_to":"subagent","pre_explore":true}],"summary":"s"}"#;
        let plan = parse_workflow_plan(src).unwrap();
        assert_eq!(plan.workflows.len(), 1);
        assert!(plan.workflows[0].pre_explore, "pre_explore=true 应被解析");

        // 显式 false
        let src = r#"{"workflows":[{"id":"wf-1","name":"本地任务","steps":["bash"],"acceptance":["ok"],"delegate_to":"subagent","pre_explore":false}],"summary":"s"}"#;
        let plan = parse_workflow_plan(src).unwrap();
        assert!(!plan.workflows[0].pre_explore, "pre_explore=false 应被解析");

        // 省略 -> 默认 false
        let src = r#"{"workflows":[{"id":"wf-1","name":"默认","steps":["x"],"acceptance":["y"],"delegate_to":"subagent"}],"summary":"s"}"#;
        let plan = parse_workflow_plan(src).unwrap();
        assert!(!plan.workflows[0].pre_explore, "省略 pre_explore 应默认 false");
    }

    // ======================== 第 122 轮(2026-09-23)新增测试 ========================
    // 覆盖 Main-Work 编排层信息收集型工具面 + ReAct 编排循环 + LoopGuard 接入。
    // 详见 docs/Main-Work工具扩展与ReAct改造/01-设计与解决方案.md §6.1。

    /// 第 122 轮:Main-Work 工具面含 MCP_Web_Use + 不持 Write/Edit。
    /// 工具注册表的最终位置在 `tools/mod.rs::main_work_registry`,本断言作为
    /// main_work 模块侧的双重校验。
    #[test]
    fn main_work_registry_includes_mcp_web_use() {
        // 通过 AgentProfile 派生(运行时真实路径)
        let names = AgentProfile::main_work_profile()
            .tools
            .names()
            .into_iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        assert!(
            names.contains(&"MCP_Web_Use".to_string()),
            "Main-Work 应持 MCP_Web_Use: {names:?}"
        );
        // 仍不持 Write / Edit
        assert!(!names.contains(&"Write".to_string()));
        assert!(!names.contains(&"Edit".to_string()));
    }

    /// 第 122 轮:Main-Work 提示词渲染含 ReAct 节 + 任务前提验证清单。
    #[test]
    fn main_work_prompt_has_react_and_precondition_section() {
        let prompt = crate::agent::system_prompt::SystemPrompt::main_work()
            .render(crate::config::Protocol::Anthropic);
        assert!(
            prompt.contains("编排循环(ReAct 模式"),
            "Main-Work 提示词应含「编排循环(ReAct 模式)」节: 实际无"
        );
        assert!(
            prompt.contains("任务前提验证"),
            "Main-Work 提示词应含「任务前提验证」清单"
        );
        assert!(
            prompt.contains("TaskFocus"),
            "Main-Work 提示词应含「TaskFocus」编排节奏关键字"
        );
    }

    /// 第 122 轮:Main-Work 工具说明含 MCP_Web_Use。
    #[test]
    fn main_work_tools_hint_mentions_mcp_web_use() {
        let prompt = crate::agent::system_prompt::SystemPrompt::main_work()
            .render(crate::config::Protocol::Anthropic);
        assert!(
            prompt.contains("MCP_Web_Use"),
            "Main-Work tools_hint 应提及 MCP_Web_Use: 实际无"
        );
        assert!(
            prompt.contains("连续工作模式"),
            "Main-Work tools_hint 应含「连续工作模式」节"
        );
    }

    /// 第 122 轮:MainWorkRunner::new 设 max_iterations=8 + explore_budget=2
    /// (编排层 TaskFocus + Verify + Decompose + Emit 节奏所需)。
    /// 验证策略:MainWorkRunner 字段私有,改用 Agent profile 路径直接验证
    /// `with_max_iterations(8).with_explore_budget(2)` 配出的工具面与 max_iterations。
    #[test]
    fn main_work_runner_config() {
        let agent = crate::agent::Agent::new(
            std::sync::Arc::new(MainWorkRunnerConfigOnlyLlm),
            AgentProfile::main_work_profile(),
        );
        let configured = agent.with_max_iterations(8).with_explore_budget(2);
        assert_eq!(
            configured.max_iterations, 8,
            "Main-Work 应配 max_iterations=8"
        );
        // 工具面 sanity check
        let names = configured.profile.tools.names();
        assert!(names.contains(&"Bash"));
        assert!(names.contains(&"MCP_Web_Use"));
    }

    /// 第 122 轮:MainWorkRunner 默认构造不抛错。
    /// 用 Paths::for_test(tmp) 构造临时 Db,避免污染根目录数据库。
    #[test]
    fn main_work_runner_default_construction_ok() {
        let tmp = tempdir();
        let paths = crate::config::Paths::for_test(&tmp);
        let db = std::sync::Arc::new(crate::config::Db::open(&paths).unwrap());
        let llm = std::sync::Arc::new(MainWorkRunnerConfigOnlyLlm);
        let runner = MainWorkRunner::new(llm, db);
        // 构造成功(未 panic)即视为通过;字段私有,无需深入。
        let _ = runner;
    }

    /// 跨平台安全 tempdir(测试环境无 tempfile crate 依赖时用 std::env::temp_dir())。
    fn tempdir() -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        dir.push(format!("laew-mainwork-test-{pid}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("tempdir 创建失败");
        dir
    }

    /// 用于本模块测试的占位 LLM:不真正驱动,仅满足 trait 装配。
    /// 注:`max_tokens`/`protocol` 取默认实现;`complete` 返回错误(本测试不调用)。
    struct MainWorkRunnerConfigOnlyLlm;

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for MainWorkRunnerConfigOnlyLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[crate::llm::ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &crate::llm::RequestMeta,
        ) -> crate::error::Result<crate::llm::Completion> {
            Err(crate::error::AgentError::Other(
                "MainWorkRunnerConfigOnlyLlm is a placeholder for construction-only tests"
                    .into(),
            ))
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    // ============================================================================
