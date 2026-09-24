# Debug Agent 激活范围强化与软件工程场景收敛 —— 设计与解决方案

> 版本: v1.0 (2026-09-19, 新增设计文档, 与 01-设计与解决方案.md 互补)
> 状态: 已实现 (2026-09-19)
> 关联代码: src/agent/debug.rs / src/agent/yolo.rs / src/agent/profile.rs / src/agent/system_prompt/mod.rs / src/main.rs / src/tui/dispatch.rs

## 1. 背景与现状复盘

### 1.1 LsmAgentEmergentWork-Debug 已经在 Anthropic 协议层看不到 tools

AgentProfile::debug_profile() 内部使用 debug_registry(), 后者返回空的 ToolRegistry:

```rust
// src/agent/tools/mod.rs
/// Debug Agent 工具注册表:无工具(只做 trace 评估,不修改系统状态)
pub fn debug_registry() -> ToolRegistry {
    ToolRegistry::new()
}
```

debug_profile() 同时设置了 emit_tool: None, 所以即使有 tool, 也不会被强制走结构化输出通道:

```rust
// src/agent/profile.rs
pub fn debug_profile() -> Self {
    Self {
        name: DEBUG_AGENT_NAME.to_string(),
        system_prompt: SystemPrompt::debug(),
        tools: debug_registry(),
        emit_tool: None,
    }
}
```

最终在 Anthropic wire 层面, 协议客户端 (src/llm/anthropic.rs::convert_tools) 拿到的 tools 数组长度为 0, tool_choice 也保持为 None (无任何强制约束), Debug Agent 因此只能输出自然语言 Markdown, 完全无法触发任何工具调用 —— 这与设计意图一致。

### 1.2 当前激活范围过宽

-debug 模式 (laew -debug [-p "任务" | -f prompt.md | TUI]) 一旦开启, 每个用户任务都会:

1. 通过 DebugLlmClient 装饰器记录所有 Agent 的 LLM 调用 trace (输入/输出/耗时/token/错误);
2. 任务结束后调用 DebugRunner::evaluate() 让 LsmAgentEmergentWork-Debug 消耗一份真实 LLM 调用, 对 trace 做四章节评估 (任务评估/质量报告/问题报告/优化建议);
3. 把结果写到 DebugReport/debug_report_*.md。

这套流程对编码/调试/测试/排查问题类任务非常合适 —— 这是 LsmAgentEmergentWork-Debug 的本职。但对闲聊、知识问答、纯文件读取、窗口查看、网页浏览等非编码任务, 让 Debug Agent 评估"一次聊天的工具调用质量"既无意义也浪费 token。

## 2. 需求收敛

把 Debug Agent 的激活范围从"凡是 -debug 都跑"收敛为:

| 场景分类                  | 触发 Debug Agent | 示例                                                       |
| ------------------------- | ---------------- | ---------------------------------------------------------- |
| 编码 (代码/脚本/Bug 修复) | 是                | 重构 src/foo.rs / 修一下这段 Go 代码 / 新增单元测试          |
| 调试 (调试/排查/诊断)     | 是                | 这个报错怎么修 / 为什么编译失败 / 为什么超时                 |
| 测试 (单元/E2E/性能)      | 是                | 跑一遍测试 / 写一个 pytest 用例 / 压测这个接口               |
| 解决问题 (Issue 分析/修复)| 是                | Issue #123 的根因 / GitHub Actions 配置                      |
| 部署/工程化 (CI/Docker)   | 是                | 写个 Dockerfile / 配 GitLab CI / 部署脚本                    |
| 软件开发相关配置           | 是                | 改 Cargo.toml 依赖 / 配置 lsp/编辑器 / 项目脚手架            |
| 闲聊/概念问答             | 否                | 你叫什么 / 你好 / Rust 是什么                                |
| 信息查询                  | 否                | 查一下今天天气 / 股票价格 / 百科                              |
| 文件只读                  | 否                | 读一下这个文件 / 展示 README                                 |
| 窗口只读/查看             | 否                | 看看微信当前窗口标题 / 列出所有应用窗口                       |
| 网页浏览                  | 否                | 打开 example.com / 截图                                      |
| 写作/翻译/摘要            | 否                | 写一份 PRD / 翻译这段话 / 总结会议纪要                       |

判断标准: 任务最终是否会触发对代码、脚本、配置、测试、工程产物的写操作或诊断性调试。若是 -> 激活;若否 -> 跳过 Debug Agent LLM 评估, 但 trace 收集器继续工作 (用户仍然能看到 LLM 调用明细)。

## 3. 总体架构

```
[用户输入] -> Yolo 入口层 (LLM 分类 + 三步意图识别)
  Yolo 同时输出 debug_eligible: bool
  (LLM 自己判断任务是否属于软件工程相关)
       |
       v
[MultiAgentOrchestrator]
  DebugCollector (开启 -debug 时) 持续收集事件
    - LlmCall (全 Agent 适用)
    - Classification (含 debug_eligible)
    - QualityCheck (每个 WF 完成)
    - TaskEnd (成功/失败/取消)
       |
       v
[任务结束] -> finalize_report()
  if debug_eligible:
      DebugRunner::evaluate() -> LsmAgentEmergentWork-Debug 调一次真实 LLM, 输出四章节评估
  else:
      render_skipped_evaluation()
        - 产出"已跳过 Debug Agent"说明性章节
        - 不调用任何 LLM, 零 token 开销
  合并写入 DebugReport/debug_report_*.md
```

## 4. 详细设计

### 4.1 TaskClassification 新增字段

src/agent/yolo.rs:

```rust
pub struct TaskClassification {
    // ... 既有字段 ...
    /// Yolo 判定:本次任务是否属于软件工程相关 (编码/脚本/调试/测试/排查/部署/配置)
    /// 决定 Debug Agent 是否对 trace 做语义评估。
    ///
    /// 默认 true (向后兼容 —— 历史 trace 缺失时按"已激活"处理, 确保旧测试不回归);
    /// Yolo LLM 在分类阶段按 YOLO_BASE_PROMPT 新增章节规则填入。
    #[serde(default = "default_debug_eligible")]
    pub debug_eligible: bool,
}

fn default_debug_eligible() -> bool { true }
```

向后兼容: [serde(default = ...)] + 全局默认值 true 保证:
- 旧 trace / 旧 SQLite 库里的 JSON 反序列化不会失败;
- 旧测试若按"Debug Agent 总激活"断言, 不回归;
- Yolo LLM 在新一轮会话里被新 prompt 引导, 输出符合新语义的 debug_eligible。

### 4.2 Yolo 系统提示词新增章节

src/agent/system_prompt/mod.rs::YOLO_BASE_PROMPT 末尾追加:

```
---

debug_eligible 字段判定 (用于控制 LsmAgentEmergentWork-Debug 是否被激活):

该字段决定本次任务结束后, Debug Agent 是否对 trace 做任务评估 / 质量报告 / 问题报告 / 优化建议
四章节语义评估。判定标准是: 任务最终是否涉及对代码、脚本、配置、测试、工程产物的写操作或诊断性调试。

应设为 true (Debug Agent 激活) 的任务类型:
- 编码: 重构/新增/修改/删除代码, 写脚本, 改 Cargo.toml 等工程文件
- 调试: 修 Bug, 分析编译/运行错误, 性能调优, 诊断性问题排查
- 测试: 编写单元测试/E2E 测试, 跑测试, 压测
- 解决问题: 分析 Issue 根因, 修复线上问题, 排查链路失败
- 部署/工程化: 写 Dockerfile / CI 配置 / 部署脚本, 项目脚手架
- 软件开发相关配置: lsp / 编辑器 / IDE 配置, 工程脚手架

应设为 false (Debug Agent 跳过) 的任务类型:
- 闲聊 / 概念问答 / 通用知识问答
- 单纯的信息查询 (天气/股票/百科/翻译)
- 纯文件读取 (不带修改意图的 Read)
- 窗口只读 / 控件树查看 (没有操作意图)
- 网页浏览 / 信息收集 (不带工程意图)
- 写作 / 翻译 / 摘要 (非工程产物)

判定原则: 如果你犹豫, 优先设为 true —— 误激活的代价只是多一次 Debug LLM 调用;
但漏掉编码任务会导致测试报告缺失, 需要重跑任务才能补, 代价更高。
```

### 4.3 DebugCollector 暴露判定 API

src/agent/debug.rs 新增 helper (纯函数, 不依赖 collector 内部状态):

```rust
/// 判断本次任务是否应激活 Debug Agent 做 trace 语义评估。
///
/// 来源: TaskClassification::debug_eligible (由 Yolo LLM 在分类阶段填入)。
/// 默认值 true (向后兼容), Yolo 已在最新 prompt 中被告知按场景收敛。
pub fn should_invoke_debug_agent(classification: &TaskClassification) -> bool {
    classification.debug_eligible
}
```

### 4.4 finalize_report 接受 classification 并按需跳过

src/agent/debug.rs:

```rust
pub struct ReportMeta {
    pub mode: String,
    pub task: String,
    pub model: String,
    /// 新增: Yolo 的三步分类结果, 用于决定是否激活 Debug Agent。
    /// None 时按"已激活"处理 (向后兼容); Some 时按 debug_eligible 字段决定。
    pub classification: Option<TaskClassification>,
}

pub async fn finalize_report(
    collector: &Arc<DebugCollector>,
    llm: Arc<dyn LlmClient>,
    report_dir: &Path,
    meta: &ReportMeta,
) -> Result<PathBuf> {
    // ...
    let should_evaluate = meta
        .classification
        .as_ref()
        .map(should_invoke_debug_agent)
        .unwrap_or(true); // 向后兼容: 无分类时仍激活

    let (evaluation, degraded, skipped_reason) = if should_evaluate {
        match DebugRunner::new(llm).evaluate(&collector.session_id(), &trace, &stats).await {
            Ok(text) => (text, false, None),
            Err(e) => (render_degraded_evaluation(&anyhow::Error::from(e), collector), true, None),
        }
    } else {
        let reason = meta
            .classification
            .as_ref()
            .map(format_skip_reason)
            .unwrap_or_else(|| "任务未提供分类信息".to_string());
        (render_skipped_evaluation(reason.as_str(), collector), false, Some(reason))
    };
    // ... 后续: 把 evaluation 写入报告, 并在 banner 加 skipped 提示
}
```

### 4.5 新增 render_skipped_evaluation 函数

src/agent/debug.rs:

```rust
/// 当任务不属于软件工程相关场景时, 生成"已跳过 Debug Agent"的说明性章节。
/// 仅基于 trace 自检指标 + 跳过原因, 不调用任何 LLM, 零 token 开销。
pub fn render_skipped_evaluation(reason: &str, collector: &Arc<DebugCollector>) -> String {
    let events = collector.events();
    let llm_calls = events.iter().filter(|e| matches!(e, DebugEvent::LlmCall { .. })).count();
    let llm_errors = events.iter().filter(|e| matches!(e, DebugEvent::LlmCall { error: Some(_), .. })).count();
    let qc_total = events.iter().filter(|e| matches!(e, DebugEvent::QualityCheck { .. })).count();
    let qc_pass = events.iter().filter(|e| matches!(e, DebugEvent::QualityCheck { verdict: Verdict::Pass, .. })).count();

    format!(
        "## 任务评估\n\n- 评估来源: 跳过模式 (Debug Agent 未被激活)\n- 跳过原因: {reason}\n- 概述: 本次任务共 LLM 调用 {llm_calls} 次 (失败 {llm_errors})、QC {qc_total} 次 (通过 {qc_pass})。因任务不属于软件工程相关场景 (编码/调试/测试/排查), 跳过 Debug Agent 语义评估以节省 token。\n\n## 质量报告\n\n- LLM 调用: {llm_calls} 次 / 失败 {llm_errors} 次\n- QC 链路: {qc_pass}/{qc_total} 通过\n- 跳过判定: Yolo 在三步分类中标记 debug_eligible=false, 按设计不调用 Debug Agent\n\n## 问题报告\n\n- (无 —— 跳过模式不调用 LLM, 无语义问题可报告)\n\n## 优化建议\n\n- 本次任务为非工程场景, 无 Debug 优化建议\n- 若需语义评估, 可调整 Yolo prompt 或临时置 TaskClassification::debug_eligible=true 重跑"
    )
}
```

### 4.6 调用方更新

#### src/main.rs::run_one_shot

```rust
// 既有: finalize_report(&collector, llm.clone(), &report_dir, &meta)
// 新增: 把 handle_result 中的 classification 通过 meta 透传
let meta = ReportMeta {
    mode: mode.to_string(),
    task: prompt.clone(),
    model: format!(...),
    classification: extract_classification(&outcome), // 从 OrchestrationOutcome 提取
};
```

#### src/tui/dispatch.rs::emit_debug_report

```rust
let meta = ReportMeta {
    mode: "TUI 多轮".to_string(),
    task: task.to_string(),
    model,
    classification: extract_classification_from_result(handle_result),
};
```

## 5. 接入点清单

| 文件 | 改动 |
|------|------|
| src/agent/yolo.rs | TaskClassification 新增 debug_eligible: bool 字段 (serde default = true) + 测试 |
| src/agent/system_prompt/mod.rs | YOLO_BASE_PROMPT 末尾新增 debug_eligible 判定章节 |
| src/agent/debug.rs | 新增 should_invoke_debug_agent() / render_skipped_evaluation() / ReportMeta.classification; finalize_report 按 classification 决定是否调用 DebugRunner; 新增测试 |
| src/main.rs | -p / -f 模式下从 OrchestrationOutcome 提取 classification 透传给 ReportMeta |
| src/tui/dispatch.rs | TUI 模式下从 handle_result 提取 classification 透传给 ReportMeta |

## 6. 测试方案

1. 单元测试:
   - task_classification_debug_eligible_default_true: 反序列化旧 JSON 时 debug_eligible 默认 true (向后兼容)。
   - should_invoke_debug_agent_respects_field: debug_eligible=false 时返回 false。
   - finalize_report_skips_llm_when_not_eligible: 构造一个 debug_eligible=false 的分类, 调用 finalize_report 应不调用 LLM, 报告含 "跳过模式" banner 和 render_skipped_evaluation 输出。
   - render_skipped_evaluation_contains_four_sections: 输出包含四个章节标题 (任务评估/质量报告/问题报告/优化建议) + "跳过模式"标注。
2. 编译验证: cargo build --release 无警告增量; cargo test --lib --release 全部通过; cargo test --lib --release debug:: 全部通过。
3. e2e 验证: bash testReport/run_e2e.sh 跑通 (覆盖 mock LLM TUI + 单轮 + 文件模式)。

## 7. 兼容性 & 风险

- 向后兼容: [serde(default)] 保证旧 JSON 反序列化成功; 默认 true 保证旧测试不回归; Yolo LLM 旧实例输出无 debug_eligible 字段时, 反序列化后 true, 行为等同现状 (全部激活)。
- 风险: Yolo LLM 漏判 —— 编码任务被误标为 false, 导致报告缺失。已在 prompt 里注明"犹豫时优先 true"来缓解。
- 零开销保证: trace 收集器在 -debug 模式下无条件运行 (与现状一致); Debug Agent LLM 评估在被跳过时完全不发请求, 节省 LLM 调用成本。

## 8. 替代方案对比

| 方案 | 优点 | 缺点 | 选择 |
|------|------|------|------|
| A. Yolo LLM 判定 debug_eligible (本方案) | 灵活、捕获语义; 与现有 intent/purpose 字段一致 | 依赖 LLM 输出正确 | 是 |
| B. 启发式正则匹配用户输入 | 零 LLM 开销判定 | 误判率高, CJK/长 prompt 难处理 | 否 |
| C. CLI 子命令 (-debug-coding) | 完全用户控制 | 学习成本高, 用户每次要选 | 否 |
| D. 完全不激活 Debug Agent | 极简 | 失去调试能力 | 否 |

## 9. 关联报告

- docs/Debug模式与DebugAgent设计/01-设计与解决方案.md: Debug Agent 整体设计与第 7 角色引入
- docs/Agent架构对比与参考.md: 与 claudecode / atomcode / pi 的 telemetry 借鉴记录
- tmpPlan/2026-09-09_08: User-Agent 按 Agent 角色逐请求注入方案 (同走 RequestMeta.user_agent)
- tmpPlan/2026-09-19_06: MCP_Window_Use 第 91 轮方案 (同走 8 角色 LLM 调用 trace 采集)
