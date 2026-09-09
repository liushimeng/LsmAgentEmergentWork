//! 结构化输出工具(emit tool)——「必须结构化输出」Agent 的协议级输出通道。
//!
//! 背景(2026-09-09 第 13 轮,方案 tmpPlan/2026-09-09_13):
//! Yolo(任务分类)与 Quality-Check(质检报告)此前依赖模型在正文输出合法 JSON,
//! 三重兜底(extract_json_block / extract_standalone_json / json_repair)之后
//! 仍会失败 → 降级(Yolo 强制 simple / QC fail-closed 回流)。
//!
//! 本模块把「文本 JSON 约定」升级为「协议级保证」:
//! - 协议层注入 forced `tool_choice`(Anthropic `{"type":"tool"}` / OpenAI
//!   `{"type":"function"}` 指名),模型**必须**以 tool_use 形式返回结构化结果;
//! - Agent 循环(`agent/mod.rs::run_session_inner`)命中 emit 工具时不执行,
//!   直接把 input 序列化为 ```json 代码块作为最终文本返回——下游解析链零改动。
//!
//! emit 工具的 `execute` 是防御性兜底:正常运行永远不会走到(循环短路在
//! schema 预校验之前),仅当未来有绕过 Agent 循环的直接调用时保持幂等无害。
//!
//! 设计参考:第七轮《专题-第七轮-结构化输出与Schema校验深度对比.md》
//! forced tool_choice 小节 + claudecode 结构化输出双通道(tool_use 首选 +
//! 文本 JSON 兜底)。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;

use super::Tool;

/// Yolo 结构化分类输出工具名(与 `profile.emit_tool` / wire `tool_choice.name` 对应)。
pub const SUBMIT_TASK_CLASSIFICATION: &str = "submit_task_classification";
/// Quality-Check 结构化质检报告输出工具名。
pub const SUBMIT_QUALITY_REPORT: &str = "submit_quality_report";

/// Yolo 任务分类输出通道:input schema 与 `yolo::TaskClassification`
/// 的 serde 表示严格对应。
pub struct SubmitTaskClassification;

#[async_trait]
impl Tool for SubmitTaskClassification {
    fn name(&self) -> &str {
        SUBMIT_TASK_CLASSIFICATION
    }

    fn description(&self) -> &str {
        "提交最终任务分类结果(结构化输出通道)。三步分析(目的→目标→意图)与难度分级\
         完成后,必须调用本工具提交分类;这是最终结果的唯一出口,不要在正文裸写 JSON。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_level": {
                    "type": "string",
                    "enum": ["simple", "medium", "hard"],
                    "description": "难度分级:simple=单步/直答,medium=多步流程,hard=需书面方案"
                },
                "purpose": {
                    "type": "string",
                    "description": "一句话概括用户的目的(为什么问这个)"
                },
                "goal_summary": {
                    "type": "string",
                    "description": "一句话概括用户的核心目标"
                },
                "intent": {
                    "type": "string",
                    "description": "意图分类英文标识,如 code_refactor / info_query / file_operation / chat / config / debug"
                },
                "agent_role": {
                    "type": "string",
                    "enum": ["subagent", "main", "plan"],
                    "description": "委派目标(可选,缺省按 task_level 推断)"
                },
                "decomposition_plan": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "分解步骤;simple 可为空数组"
                },
                "direct_answer": {
                    // 注:刻意不用 ["string","null"] 类型数组——部分第三方网关对
                    // input_schema 做严格校验时不认类型数组;纯 string + 描述引导,
                    // 模型仍会按描述输出 null(下游 Option<String> 两者都接受)。
                    "type": "string",
                    "description": "simple 且无需工具可直接回答时填答案;需要委派执行时填 null"
                },
                "user_suggestion_if_fail": {
                    "type": "string",
                    "description": "失败时给用户的备选建议(可空)"
                }
            },
            "required": ["task_level", "goal_summary", "intent"]
        })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        // 防御性兜底:Agent 循环在命中 emit 工具时会短路(不执行)。
        Ok("(结构化输出已接收:submit_task_classification)".to_string())
    }
}

/// Quality-Check 质检报告输出通道:input schema 与 `quality::QualityReport`
/// 的 serde 表示严格对应(source 枚举值与 `AgentRole` 的 snake_case serde 名对齐)。
pub struct SubmitQualityReport;

#[async_trait]
impl Tool for SubmitQualityReport {
    fn name(&self) -> &str {
        SUBMIT_QUALITY_REPORT
    }

    fn description(&self) -> &str {
        "提交最终质检报告(结构化输出通道)。基于实际输出与执行轨迹完成判定后,\
         必须调用本工具提交结论;这是最终结果的唯一出口,不要在正文裸写 JSON。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "verdict": {
                    "type": "string",
                    "enum": ["pass", "fail"],
                    "description": "质检结论"
                },
                "source": {
                    "type": "string",
                    "enum": ["yolo", "plan", "main", "subagent", "quality_check", "session_context", "compact"],
                    "description": "被检单元的来源角色"
                },
                "issues": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "发现的问题列表(可为空)"
                },
                "suggestion": {
                    "type": "string",
                    "description": "改进建议(可为空)"
                },
                "retryable": {
                    "type": "boolean",
                    "description": "失败是否可通过重试修复"
                },
                "evidence": {
                    "type": "string",
                    "description": "判定依据(引用实际输出/轨迹信号,可为空)"
                }
            },
            "required": ["verdict", "source", "retryable"]
        })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        // 防御性兜底:Agent 循环在命中 emit 工具时会短路(不执行)。
        Ok("(结构化输出已接收:submit_quality_report)".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_task_classification_def() {
        let t = SubmitTaskClassification;
        assert_eq!(t.name(), "submit_task_classification");
        let schema = t.parameters();
        // enum 三档 + 必填字段
        assert_eq!(schema["properties"]["task_level"]["enum"][0], "simple");
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "goal_summary"));
        assert!(required.iter().any(|v| v == "intent"));
    }

    #[test]
    fn submit_quality_report_def() {
        let t = SubmitQualityReport;
        assert_eq!(t.name(), "submit_quality_report");
        let schema = t.parameters();
        assert_eq!(schema["properties"]["verdict"]["enum"][1], "fail");
        // source 枚举与 AgentRole serde 名对齐(snake_case)
        let source_enum = schema["properties"]["source"]["enum"].as_array().unwrap();
        assert!(source_enum.iter().any(|v| v == "quality_check"));
        assert!(source_enum.iter().any(|v| v == "subagent"));
        assert!(!source_enum.iter().any(|v| v == "QualityCheck"));
    }

    #[tokio::test]
    async fn emit_tools_execute_is_defensive_noop() {
        // 防御性兜底:即使被直接调用也幂等无害
        let out1 = SubmitTaskClassification.execute(json!({})).await.unwrap();
        assert!(out1.contains("submit_task_classification"));
        let out2 = SubmitQualityReport.execute(json!({})).await.unwrap();
        assert!(out2.contains("submit_quality_report"));
    }
}
