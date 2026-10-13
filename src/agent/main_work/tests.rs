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
    fn sanitize_depends_on_drops_placeholder_prose() {
        // 第 160 轮实测事故(llaew_20261012_143803.log):Main-Work/Plan 把空依赖写成
        // 中文占位与散文碎片,旧策略保留给 topo 报错 → 整批拓扑失败 → 重试环空转
        // 22 分钟零单元执行。这些条目应被净化为「无依赖」,拓扑照常分层。
        let mut plan = WorkFlowPlan {
            workflows: vec![
                wf("wf-1", &[]),
                wf("wf-2", &["无"]),
                wf("wf-3", &["无", "与"]),
                wf("wf-4", &["无", "**不声明跨项引用**)"]),
            ],
            summary: "test".into(),
            degraded: false,
        };
        sanitize_depends_on(&mut plan);
        for w in &plan.workflows {
            assert!(
                w.depends_on.is_empty(),
                "wf={} 应净化为无依赖,实际 {:?}",
                w.id,
                w.depends_on
            );
        }
        // 全部无依赖 → 同层并行,拓扑必须成功(修复前这里报「依赖未知 wf=无」)
        let layers = topo_layers(&plan.workflows).unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].len(), 4);
    }

    #[test]
    fn sanitize_depends_on_placeholder_family() {
        let mut plan = WorkFlowPlan {
            workflows: vec![
                wf("wf-1", &["none"]),
                wf("wf-2", &["NULL"]),
                wf("wf-3", &["n/a"]),
                wf("wf-4", &["-"]),
                wf("wf-5", &["没有"]),
                wf("wf-6", &["null"]),
            ],
            summary: "test".into(),
            degraded: false,
        };
        sanitize_depends_on(&mut plan);
        for w in &plan.workflows {
            assert!(w.depends_on.is_empty(), "wf={} 占位符应丢弃", w.id);
        }
    }

    #[test]
    fn sanitize_depends_on_extracts_ids_from_prose() {
        // 散文条目内嵌真实 id:抽取而非丢弃
        let mut plan = WorkFlowPlan {
            workflows: vec![
                wf("wf-1", &[]),
                wf("wf-2", &[]),
                wf("wf-3", &["先完成 wf-1 与 wf-2"]),
                wf("wf-4", &["wf-1、wf-2 的产物"]),
            ],
            summary: "test".into(),
            degraded: false,
        };
        sanitize_depends_on(&mut plan);
        assert_eq!(
            plan.workflows[2].depends_on,
            vec!["wf-1".to_string(), "wf-2".to_string()]
        );
        assert_eq!(
            plan.workflows[3].depends_on,
            vec!["wf-1".to_string(), "wf-2".to_string()]
        );
        assert!(topo_sort(&plan.workflows).is_ok());
    }

    #[test]
    fn sanitize_depends_on_prose_extraction_prefers_longer_ids() {
        // wf-10 与 wf-1 并存时,散文抽取不得让 wf-1 抢 wf-10 的子串
        let mut plan = WorkFlowPlan {
            workflows: vec![
                wf("wf-1", &[]),
                wf("wf-10", &[]),
                wf("wf-11", &["依赖 wf-10 完成后执行"]),
            ],
            summary: "test".into(),
            degraded: false,
        };
        sanitize_depends_on(&mut plan);
        assert_eq!(plan.workflows[2].depends_on, vec!["wf-10".to_string()]);
    }

    #[test]
    fn sanitize_depends_on_keeps_id_shaped_typo_for_topo_error() {
        // id 形状但未声明(wf-9)→ 保留,让 topo_layers 报精准错误(真拼写错误通道不吞)
        let mut plan = WorkFlowPlan {
            workflows: vec![wf("wf-1", &[]), wf("wf-2", &["wf-9"])],
            summary: "test".into(),
            degraded: false,
        };
        sanitize_depends_on(&mut plan);
        assert_eq!(plan.workflows[1].depends_on, vec!["wf-9".to_string()]);
        let err = topo_sort(&plan.workflows).unwrap_err();
        assert!(format!("{err}").contains("wf-9"), "应报未知 wf-9,实际 {err}");
    }

    #[test]
    fn sanitize_depends_on_concurrent_note_is_not_a_dependency() {
        // 「无(与 wf-1 并发启动)」语义 = 无依赖(括号内是并行说明,非依赖边);
        // 首括号切分得占位符「无」→ 整条丢弃,wf-2 保持与 wf-1 同层并行
        let mut plan = WorkFlowPlan {
            workflows: vec![wf("wf-1", &[]), wf("wf-2", &["无(与 wf-1 并发启动)"])],
            summary: "test".into(),
            degraded: false,
        };
        sanitize_depends_on(&mut plan);
        assert!(plan.workflows[1].depends_on.is_empty());
        let layers = topo_layers(&plan.workflows).unwrap();
        assert_eq!(layers.len(), 1, "wf-1/wf-2 应同层并行");
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

    /// 第 165 轮事故复现(`llaew_20261013_102358.log` 百度地图任务):Plan Agent 输出
    /// 全角冒号(实测 70 处全角 `：` vs 2 处半角),旧解析只认半角 → `- 依赖：wf-1`
    /// 整行静默丢弃 → depends_on/acceptance 全空 → 5 单元同层并发乱序互踩。
    /// 全角冒号兼容后依赖链与验收标准必须完整解析,name 头部不得残留 `：`。
    #[test]
    fn 解析plan文档_全角冒号_依赖与验收标准不丢() {
        let md = r#"
# 任务方案：公交路线规划

## 一、目标
目标内容

## 二、WorkFlow 拆解
### WorkFlow 1：进入路线规划模式并输入起终点
- 步骤：
  - [ ] 点击路线规划按钮
  - [ ] 在起点输入框中输入「民院小区」
- 委派 Agent：SubAgent-Work
- 依赖：无
- 验收标准：路线规划面板展开

### WorkFlow 2：选择公共交通模式并搜索路线
- 步骤：
  - [ ] 点击「公共交通出行」
- 委派 Agent：SubAgent-Work
- 依赖：wf-1
- 验收标准：页面显示至少一条公共交通路线方案

### WorkFlow 3：截取地图全景截图
- 步骤：
  - [ ] 使用 screenshot 截图
- 委派 Agent：SubAgent-Work
- 依赖：wf-2
- 验收标准：截图文件成功生成

### WorkFlow 4：提取路线规划关键信息
- 步骤：
  - [ ] 提取预计总时长
- 委派 Agent：SubAgent-Work
- 依赖：wf-2
- 验收标准：成功提取完整的路线规划信息

### WorkFlow 5：生成出行计划文档
- 步骤：
  - [ ] 创建文件 出行计划-01.md
- 委派 Agent：SubAgent-Work
- 依赖：wf-3, wf-4
- 验收标准：出行计划-01.md 文件生成成功

## 三、关键决策
决策
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("parse failed: {e}"));
        assert_eq!(plan.workflows.len(), 5, "应解析出 5 个 WorkFlow");
        assert_eq!(plan.workflows[1].depends_on, vec!["wf-1"], "全角冒号依赖必须解析");
        assert_eq!(plan.workflows[2].depends_on, vec!["wf-2"]);
        assert_eq!(plan.workflows[3].depends_on, vec!["wf-2"]);
        assert_eq!(
            plan.workflows[4].depends_on,
            vec!["wf-3", "wf-4"],
            "半角逗号 + 全角冒号混合分隔依赖: {:?}",
            plan.workflows[4].depends_on
        );
        assert_eq!(
            plan.workflows[0].name, "进入路线规划模式并输入起终点",
            "name 头部不得残留全角冒号: {:?}",
            plan.workflows[0].name
        );
        assert!(
            plan.workflows.iter().all(|w| !w.acceptance.is_empty()),
            "全角冒号验收标准必须解析: {:?}",
            plan.workflows.iter().map(|w| &w.acceptance).collect::<Vec<_>>()
        );
        // 依赖完整后拓扑应为 4 层:[wf-1] → [wf-2] → [wf-3,wf-4] → [wf-5],
        // 而非事故中的 1 层 5 单元全并行
        let layers = topo_layers(&plan.workflows).expect("拓扑合法");
        assert_eq!(layers.len(), 4, "依赖链解析后应分 4 层: {layers:?}");
        assert_eq!(layers[0].len(), 1);
        assert_eq!(layers[2].len(), 2, "wf-3/wf-4 同层: {layers:?}");
        assert_eq!(layers[3][0].id, "wf-5");
    }

    /// 全角逗号分隔的多依赖形态(`- 依赖：wf-3，wf-4`)也必须完整解析。
    #[test]
    fn 解析plan文档_全角逗号分隔依赖() {
        let md = r#"
## 二、WorkFlow 拆解
### WorkFlow 1：准备
- 步骤：
  - [ ] 初始化
- 依赖：无
- 验收标准：完成

### WorkFlow 2：准备二
- 步骤：
  - [ ] 初始化二
- 依赖：无
- 验收标准：完成

### WorkFlow 3：汇总
- 步骤：
  - [ ] 汇总
- 依赖：wf-1，wf-2
- 验收标准：完成
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("parse failed: {e}"));
        assert_eq!(
            plan.workflows[2].depends_on,
            vec!["wf-1", "wf-2"],
            "全角逗号分隔依赖: {:?}",
            plan.workflows[2].depends_on
        );
    }

    /// 第 165 轮:零依赖多单元计划的串行链兜底 —— 依赖全空时按声明顺序补
    /// wf[i]→wf[i-1],拓扑退化的「1 层全并行」不再出现(乱序互踩重试风暴根治)。
    #[test]
    fn 依赖全空_多单元_串行链兜底() {
        let md = r#"
## 二、WorkFlow 拆解
### WorkFlow 1: 打开页面
- 步骤:
  - [ ] open
- 依赖: 无
- 验收标准: 页面打开

### WorkFlow 2: 输入起终点
- 步骤:
  - [ ] input
- 依赖: 无
- 验收标准: 输入完成

### WorkFlow 3: 截图
- 步骤:
  - [ ] screenshot
- 依赖: 无
- 验收标准: 截图生成
"#;
        let plan = parse_plan_markdown(md).unwrap_or_else(|e| panic!("parse failed: {e}"));
        assert_eq!(plan.workflows.len(), 3);
        assert_eq!(plan.workflows[0].depends_on, Vec::<String>::new(), "首单元无前置");
        assert_eq!(plan.workflows[1].depends_on, vec!["wf-1"], "全空应串行链兜底");
        assert_eq!(plan.workflows[2].depends_on, vec!["wf-2"]);
        let layers = topo_layers(&plan.workflows).expect("兜底后拓扑合法");
        assert_eq!(
            layers.len(),
            3,
            "兜底后应 3 层单元素串行,而非 1 层全并行: {layers:?}"
        );
    }

    /// 串行链兜底判定:单单元 / 任一单元有依赖(显式或推断)时不得触发。
    #[test]
    fn 兜底判定_单单元或有依赖不触发() {
        let spec = |id: &str, deps: Vec<&str>| WorkFlowSpec {
            id: id.into(),
            name: id.into(),
            steps: vec!["step".into()],
            branches: vec![],
            loops: vec![],
            depends_on: deps.into_iter().map(String::from).collect(),
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
            max_iterations: None,
            original_prompt: None,
            pre_explore: false,
        };
        // 单单元:不触发
        assert!(!serial_chain_applies(&[spec("wf-1", vec![])]));
        // 全空多单元:触发
        assert!(serial_chain_applies(&[spec("wf-1", vec![]), spec("wf-2", vec![])]));
        // 任一单元有依赖:不触发(已有拓扑证据)
        assert!(!serial_chain_applies(&[spec("wf-1", vec![]), spec("wf-2", vec!["wf-1"])]));
        // 幂等:已兜底过的计划再跑一次不重复加边
        let mut plan = WorkFlowPlan {
            workflows: vec![spec("wf-1", vec![]), spec("wf-2", vec![])],
            summary: String::new(),
            degraded: false,
        };
        fallback_serial_chain(&mut plan);
        assert_eq!(plan.workflows[1].depends_on, vec!["wf-1"]);
        fallback_serial_chain(&mut plan);
        assert_eq!(
            plan.workflows[1].depends_on,
            vec!["wf-1"],
            "二次兜底不得重复加边: {:?}",
            plan.workflows[1].depends_on
        );
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
    // 2026-09-23 Round 124:BashTool readonly 模式集成测试
    // I1: BashTool::readonly().mode() == BashMode::ReadOnly
    // I2: BashTool::default().mode() == BashMode::ReadWrite(向后兼容)
    // I3: BashTool::readonly().description() 含「只读」
    // I4: SystemPrompt::main_work().render(Anthropic) 含「Bash 只读硬性约束」+ 「LAEW_BASH_READONLY=off」
    // ============================================================================
    mod readonly_tests {
        use crate::agent::system_prompt::SystemPrompt;
        use crate::agent::tools::bash::{BashMode, BashTool};
        use crate::agent::tools::Tool;
        use crate::config::Protocol as CfgProtocol2;

        #[test]
        fn main_work_bash_is_readonly_mode() {
            // I1:BashTool::readonly() 标记为 ReadOnly
            let tool = BashTool::readonly();
            assert_eq!(tool.mode(), BashMode::ReadOnly);
        }

        #[test]
        fn builtin_bash_is_readwrite_mode() {
            // I2:默认 BashTool 是 ReadWrite(向后兼容;SubAgent-Work 仍可写)
            let tool = BashTool::new();
            assert_eq!(tool.mode(), BashMode::ReadWrite);
            let default_tool = BashTool::default();
            assert_eq!(default_tool.mode(), BashMode::ReadWrite);
        }

        #[test]
        fn readonly_bash_description_mentions_readonly() {
            // I3:readonly 模式的 description 红字提示「Round 124 readonly」+ 拦截清单
            let binding = BashTool::readonly();
            let desc = binding.description();
            assert!(
                desc.contains("Round 124 readonly"),
                "description 应包含 readonly 红字: {desc}"
            );
            assert!(
                desc.contains("只读") && desc.contains("拦截"),
                "description 应描述只读/拦截: {desc}"
            );
            assert!(
                desc.contains("LAEW_BASH_READONLY=off"),
                "description 应提及 env 旁路: {desc}"
            );
        }

        #[test]
        fn readwrite_bash_description_unchanged() {
            // I2':默认 BashTool 的 description 不含 readonly 红字(向后兼容)
            let binding = BashTool::new();
            let desc = binding.description();
            assert!(
                !desc.contains("Round 124 readonly"),
                "ReadWrite description 不应含 readonly 红字"
            );
            assert!(
                desc.contains("【大输出落盘(D17)】"),
                "ReadWrite description 应保留 D17 spill 提示"
            );
        }

        #[test]
        fn main_work_registry_uses_readonly_bash() {
            // I2':main_work_registry 的 Bash 是 readonly 实例
            let reg = crate::agent::tools::main_work_registry();
            let bash_arc = reg.get("Bash").expect("main_work 应持 Bash");
            // 通过 description 推断(无法直接拿 BashMode 字段)
            let desc = bash_arc.description();
            assert!(
                desc.contains("Round 124 readonly"),
                "main_work_registry 的 Bash 应是 readonly: {desc}"
            );
        }

        #[test]
        fn builtin_registry_uses_readwrite_bash() {
            // I2':builtin_registry 的 Bash 是 ReadWrite 实例
            let reg = crate::agent::tools::builtin_registry();
            let bash_arc = reg.get("Bash").expect("builtin 应持 Bash");
            let desc = bash_arc.description();
            assert!(
                !desc.contains("Round 124 readonly"),
                "builtin_registry 的 Bash 应是 ReadWrite: {desc}"
            );
        }

        #[test]
        fn main_work_prompt_renders_readonly_section() {
            // I4:SystemPrompt::main_work().render(Anthropic) 含 readonly 硬性约束
            let rendered = SystemPrompt::main_work().render(crate::config::Protocol::Anthropic);
            assert!(
                rendered.contains("Bash 只读硬性约束"),
                "main_work 提示词应含「Bash 只读硬性约束」段: {rendered}"
            );
            assert!(
                rendered.contains("LAEW_BASH_READONLY=off"),
                "main_work 提示词应提及 env 旁路: {rendered}"
            );
            assert!(
                rendered.contains("代码层只读"),
                "main_work 提示词应强调代码层只读: {rendered}"
            );
        }

        #[test]
        fn main_work_tools_hint_mentions_readonly_red_label() {
            // I4':main_work_tools_hint 渲染后 Bash 描述有 readonly 红字
            //     直接用 render_with_protocol 检查(Anthropic)
            let rendered = SystemPrompt::main_work().render(CfgProtocol2::Anthropic);
            assert!(
                rendered.contains("Round 124 readonly"),
                "main_work_tools_hint 应渲染 Bash readonly 红字: {rendered}"
            );
        }
    }

    // ============================================================================

// ============================================================================
// 第 135 轮:Main-Work 解析兜底(空 workflows / Markdown 第三通道 / 网页 pre_explore)
// ============================================================================

/// `workflows: []` **且 `summary` 为空** —— 没有任何可转达的内容,等同于
/// 什么都没产出,维持第 135 轮行为按解析失败交给既有兜底分支。
///
/// `spec.rs` 里 `workflows` 是 `#[serde(default)]`,所以空数组会「解析成功」;
/// 若直接放行,`execute_workflows` 会零单元收口并判成功(与 `plan_validate.rs`
/// 的空检查不一致 —— 那个检查只在 Plan 路径)。
#[test]
fn parse_workflow_plan_rejects_empty_workflows() {
    let err = parse_workflow_plan(r#"{"workflows":[],"summary":""}"#).unwrap_err();
    assert!(
        err.to_string().contains("workflows 为空"),
        "空 workflows 且无 summary 应报解析失败,实际: {err}"
    );
}

/// 第 156 轮:`workflows: []` **但 `summary` 带实质说明** —— 这是模型交付的
/// 一个**合法零单元计划**(典型形态「不予编排,需回到用户澄清/改题」),
/// 属**主动弃权**而非解析失败,必须原样上抛,绝不能退回「照原任务执行」的兜底。
///
/// 回归实测(`llaew_20261012_102053.log`):两者混为一谈导致白烧
/// **156.89s / 12.4 万 input token**,终态还记「✅ 成功」——用户要求打开
/// 豆包并提取系统提示词,实际一件没做成。
#[test]
fn parse_workflow_plan_surfaces_model_abstention() {
    let err = parse_workflow_plan(
        r#"{"workflows":[],"summary":"不予编排,需回到用户澄清/改题。原因:目标不可执行。"}"#,
    )
    .unwrap_err();
    match err {
        crate::error::AgentError::WorkflowAbstain { summary } => {
            assert!(summary.contains("不予编排"), "弃权说明应原样保留,实际: {summary}");
        }
        other => panic!("零单元 + 非空 summary 应判为主动弃权,实际: {other}"),
    }
}

/// 第 156 轮回归:纯空白 summary 等价于「无说明」,仍走解析失败兜底,
/// 避免把噪声当弃权理由转达给用户。
#[test]
fn parse_workflow_plan_blank_summary_still_parse_failure() {
    let err = parse_workflow_plan(r#"{"workflows":[],"summary":"   \n  "}"#).unwrap_err();
    assert!(
        err.to_string().contains("workflows 为空"),
        "空白 summary 应报解析失败,实际: {err}"
    );
}

/// 第 156 轮:正常非空计划不受零单元分流影响(防误伤回归)。
#[test]
fn parse_workflow_plan_accepts_nonempty_plan() {
    let plan = parse_workflow_plan(
        r#"{"workflows":[{"id":"wf-1","name":"执行","steps":["做一件事"],"acceptance":["产物存在"]}],"summary":"已拆解"}"#,
    )
    .expect("非空计划应解析成功");
    assert_eq!(plan.workflows.len(), 1);
    assert_eq!(plan.summary, "已拆解");
}

/// 第三通道:Main-Work 输出 Markdown 表格而非 JSON 时,复用 `parse_plan_markdown`。
///
/// 实测事故(`llaew_20261008_173357.log`):两条 JSON 通道同时落空,零重试直接返回
/// 「未找到合法的 WorkFlow JSON」,编排层退化成单 WorkFlow,wf-2 整个丢失。
#[test]
fn parse_workflow_plan_accepts_markdown_table() {
    let md = "## WorkFlow 拆解\n\n\
              | id | name | steps | depends_on |\n\
              | --- | --- | --- | --- |\n\
              | wf-1 | 打开页面并抓取 | MCP_Web_Use(action=open) + inspect | 无 |\n\
              | wf-2 | 渲染表格并落盘 | Bash 写 Markdown | wf-1 |\n";
    let plan = parse_workflow_plan(md).expect("Markdown 编排表应能被解析");
    assert_eq!(plan.workflows.len(), 2, "应还原出 2 个单元: {plan:?}");
    assert_eq!(plan.workflows[0].id, "wf-1");
    assert_eq!(plan.workflows[1].depends_on, vec!["wf-1".to_string()]);
}

/// 两条 JSON 通道 + Markdown 通道全灭时,错误文案要同时点出两种形态。
#[test]
fn parse_workflow_plan_error_mentions_both_shapes() {
    let err = parse_workflow_plan("随便一段散文,既没有 JSON 也没有编排表。").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("JSON"), "{msg}");
    assert!(msg.contains("Markdown"), "{msg}");
}

/// 网页任务判定(决定降级兜底要不要开 `pre_explore`)。
#[test]
fn is_web_task_detects_web_and_local_tasks() {
    use crate::agent::main_work::is_web_task;
    // 网页:显式 URL / 工具名 / 关键词
    assert!(is_web_task(Some("抓取 https://36kr.com/ 的最新文章"), &[]));
    assert!(is_web_task(Some("用 MCP_Web_Use 打开页面"), &[]));
    assert!(is_web_task(None, &["浏览网页并保存内容".to_string()]));
    // 本地任务不应误判
    assert!(!is_web_task(Some("重构 src/agent/mod.rs 并跑 cargo test"), &[]));
    assert!(!is_web_task(None, &["读取文件并生成报告".to_string()]));
    assert!(!is_web_task(None, &[]));
}

// ==================== 第 163 轮:depends_on 缺边推断 ====================

/// 造一个带 steps 的 WorkFlow(供产物引用推断用)。
fn wf_with_steps(id: &str, steps: &[&str]) -> WorkFlowSpec {
    let mut w = wf(id, &[]);
    w.steps = steps.iter().map(|s| s.to_string()).collect();
    w
}

#[test]
fn infer_depends_on_links_consumer_to_producer() {
    // 实测事故形态:wf-3 读取 wf-2 的产物文件,但 depends_on 全空 →
    // Kahn 把两者判成同层并发,消费方只能轮询等文件(约 195s 纯空等)。
    let mut plan = WorkFlowPlan {
        workflows: vec![
            wf_with_steps(
                "wf-1",
                &["打开 goofish.com,设定交易地区为北京,写入 地区确认.md"],
            ),
            wf_with_steps(
                "wf-2",
                &["读取 地区确认.md,遍历全品类,采集结果写入 闲鱼-物品类型列表.md"],
            ),
            wf_with_steps(
                "wf-3",
                &["读取 闲鱼-物品类型列表.md,筛选 800-1000 元品类,生成 成本可控.md"],
            ),
            wf_with_steps(
                "wf-4",
                &["根据 成本可控.md 筛出最适合倒卖的品类,生成 最适合.md"],
            ),
            wf_with_steps(
                "wf-5",
                &["基于 最适合.md 锁定 3 个目标物品,写入 目标物品.md"],
            ),
        ],
        summary: "test".into(),
        degraded: false,
    };
    infer_depends_on(&mut plan);
    let deps = |id: &str| {
        plan.workflows
            .iter()
            .find(|w| w.id == id)
            .unwrap()
            .depends_on
            .clone()
    };
    assert_eq!(deps("wf-2"), vec!["wf-1".to_string()]);
    assert_eq!(deps("wf-3"), vec!["wf-2".to_string()]);
    assert_eq!(deps("wf-4"), vec!["wf-3".to_string()]);
    assert_eq!(deps("wf-5"), vec!["wf-4".to_string()]);
    assert!(deps("wf-1").is_empty(), "无上游依赖");
    // 链式依赖必须分 5 层串行,不得再同层并发
    assert_eq!(topo_layers(&plan.workflows).unwrap().len(), 5);
}

#[test]
fn infer_depends_on_ignores_self_produced_and_consumed() {
    // 同一单元既读又写同名文件 → 单元内闭环,不得自环
    let mut plan = WorkFlowPlan {
        workflows: vec![
            wf_with_steps("wf-1", &["采集数据写入 data.md"]),
            wf_with_steps("wf-2", &["读取 data.md,处理后写入 data.md"]),
        ],
        summary: "test".into(),
        degraded: false,
    };
    infer_depends_on(&mut plan);
    assert!(plan.workflows[1].depends_on.is_empty());
    assert_eq!(topo_layers(&plan.workflows).unwrap().len(), 1);
}

#[test]
fn infer_depends_on_respects_existing_edges_and_avoids_cycles() {
    // 显式边 wf-3 → wf-2 已存在时,推断不得重复添加;反向候选边若会成环必须跳过。
    let mut plan = WorkFlowPlan {
        workflows: vec![
            wf_with_steps("wf-1", &["产出 a.md"]),
            wf_with_steps("wf-2", &["读取 a.md,产出 b.md"]),
            wf_with_steps("wf-3", &["读取 a.md,读取 b.md,产出 c.md"]),
        ],
        summary: "test".into(),
        degraded: false,
    };
    plan.workflows[2].depends_on = vec!["wf-2".to_string()];
    infer_depends_on(&mut plan);
    let deps3 = &plan.workflows[2].depends_on;
    assert!(deps3.contains(&"wf-1".to_string()), "wf-3 读 a.md → 应依赖 wf-1");
    assert_eq!(
        deps3.iter().filter(|d| *d == "wf-2").count(),
        1,
        "显式边不得重复添加"
    );
    // 补边后拓扑必须仍可解(无环)
    assert!(topo_layers(&plan.workflows).is_ok());
}

#[test]
fn infer_depends_on_no_false_edge_without_produce_verb() {
    // 只提到文件名但没有产出动词,不得凭空造边
    let mut plan = WorkFlowPlan {
        workflows: vec![
            wf_with_steps("wf-1", &["执行浏览器遍历,记录 www.goofish.com 页面情况"]),
            wf_with_steps("wf-2", &["读取 首页截图.png 的内容并汇总"]),
        ],
        summary: "test".into(),
        degraded: false,
    };
    infer_depends_on(&mut plan);
    assert!(plan.workflows[1].depends_on.is_empty());
}
