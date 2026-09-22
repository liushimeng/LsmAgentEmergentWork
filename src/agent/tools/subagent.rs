//! `SubAgent` 工具 —— 自感知 + 动态启动子 Agent(单工具 + action 枚举分发)。
//!
//! 第 114 轮(2026-09-22)新增,设计见
//! `docs/自感知SubAgent动态启动/01-设计与解决方案.md` §5.2。
//!
//! 五个动作:
//! - `launch`  启动 1 个子 Agent(可 `background=true`)
//! - `batch`   并行启动 ≤8 个子 Agent(批内 `buffer_unordered`)
//! - `list`    自感知快照(身份 / 工具面 / 名册 / 深度 / 余额 / 运行中作业)
//! - `result`  取回后台作业结果
//! - `cancel`  取消后台作业
//!
//! 运行时(LLM 客户端 / 深度 / 预算 / 取消 token)来自 `dynamic_subagent` 的
//! task-local 作用域;取不到 → 统一信封 `code=4001`,并**明确要求模型不要重试**。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::dynamic_subagent::{self, report_json, SpawnError, SubAgentRequest};
use crate::agent::self_awareness::{self as sa, SubAgentType};
use crate::agent::tools::Tool;
use crate::error::Result;

/// 工具名(wire schema 名)。
pub const SUBAGENT_TOOL: &str = "SubAgent";

/// 动态子 Agent 工具(无状态;运行时经 task-local 注入)。
pub struct SubAgentTool;

fn envelope(code: u16, message: &str, data: Value) -> String {
    json!({"code": code, "message": message, "data": data}).to_string()
}

fn ok_envelope(data: Value) -> String {
    envelope(0, "ok", data)
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// 解析单个子任务(launch 与 batch.tasks[] 共用)。
fn parse_request(v: &Value, require_task: bool) -> std::result::Result<SubAgentRequest, SpawnError> {
    let task = str_arg(v, "task").unwrap_or("").to_string();
    if require_task && task.is_empty() {
        return Err(SpawnError::InvalidArgs(
            "launch / batch.tasks[] 必须提供非空 `task`(子 Agent 看不到父上下文,任务必须自包含)".into(),
        ));
    }
    let type_raw = str_arg(v, "agent_type").unwrap_or("general-purpose");
    let agent_type = SubAgentType::parse(type_raw).ok_or_else(|| {
        SpawnError::InvalidArgs(format!(
            "未知 agent_type `{type_raw}`;可选:{}",
            SubAgentType::ALL
                .iter()
                .map(|t| t.id())
                .collect::<Vec<_>>()
                .join(" / ")
        ))
    })?;
    let tools = v
        .get("tools")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let max_iterations = v
        .get("max_iterations")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(4, 32));
    Ok(SubAgentRequest {
        task,
        agent_type,
        name: str_arg(v, "name").map(|s| s.to_string()),
        system_prompt: str_arg(v, "system_prompt").map(|s| s.to_string()),
        tools,
        expected_output: str_arg(v, "expected_output").unwrap_or("").to_string(),
        max_iterations,
    })
}

#[async_trait]
impl Tool for SubAgentTool {
    fn name(&self) -> &str {
        SUBAGENT_TOOL
    }

    fn description(&self) -> &str {
        "启动 / 管理动态子 Agent(自感知委派工具)。子 Agent 拥有**独立上下文**(看不到你的对话历史),\
         完成后把结果回填给你,由你汇总成最终回答。\
         \n\naction 语义:\
         \n- `list`  :自感知快照 —— 你的身份 / 工具面 / 可启动类型名册 / 当前深度 / 剩余额度 / 运行中作业。零成本,建议在委派前先看一眼。\
         \n- `launch`:启动 1 个子 Agent 并**等待结果**(必填 `task`;可给 `agent_type` / `name` / `tools` / `expected_output` / `max_iterations` / `background`)。`background=true` 时立即返回 `run_id`,稍后用 `action=\"result\"` 取回。\
         \n- `batch` :并行启动 ≤8 个子 Agent(必填 `tasks` 数组,元素字段同 launch;可用 `max_concurrency` 限制批内并发,默认 3)。返回逐子报告 + 合并文本。\
         \n- `result`:取回后台作业(必填 `run_id`);`running` 表示仍在跑。\
         \n- `cancel`:取消后台作业(必填 `run_id`)。\
         \n\n子 Agent 类型(agent_type):general-purpose(通用执行,可写文件/跑命令) / explore(只读侦察) / researcher(资料研究,可用浏览器) / plan(方案设计) / code-reviewer(代码审查) / operator(浏览器/桌面窗口操控)。\
         \n工具收窄:`tools` 只能**收窄**不能扩权(超出父 Agent 能力的会被剔除,并在报告的 dropped_tools 里说明)。\
         \n\n【何时启动】任务可拆成 2 个以上**相互独立**的子任务,或用户提示词明确要求「启动 SubAgent / 并行 / 分工 / 分别调研 / 多个 Agent」时;\
         多个独立子任务请**一次 batch**(比多次 launch 快);有先后依赖时用多次 launch。\
         \n【何时不要启动】单步任务、步骤强依赖、需要你自己看中间结果再决策的任务 —— 直接自己做,启动子 Agent 要额外付一次完整 LLM 成本。\
         \n【硬性要求】task 必须自包含(目标 + 输入路径 + 期望产物 + 验收口径);子 Agent 结果只是中间产物,你**必须**整合成面向用户的最终回答,禁止只回答「已委派」。\
         \n\n返回统一 JSON 信封:`{code, message, data}`。code:0 成功 / 1001 参数非法 / 2001 预算耗尽 / 2002 深度超限 / 2003 并发排队超时 / 4001 当前上下文不支持动态启动(不要重试,自己完成) / 4002 已取消 / 4003 run_id 不存在。"
    }

    fn parameters(&self) -> Value {
        let task_item_schema = json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "minLength": 1,
                          "description": "子任务描述(必须自包含:目标 + 输入路径 + 期望产物 + 验收口径;子 Agent 看不到你的上下文)" },
                "agent_type": {
                    "type": "string",
                    "enum": ["general-purpose", "explore", "researcher", "plan", "code-reviewer", "operator"],
                    "description": "子 Agent 类型,默认 general-purpose"
                },
                "name": { "type": "string", "description": "子 Agent 名称(日志/报告用,可选)" },
                "system_prompt": { "type": "string", "description": "追加到子 Agent 系统提示词的额外指令(可选)" },
                "tools": { "type": "array", "items": { "type": "string" },
                           "description": "限定子 Agent 工具名(只能收窄,超出父能力会被剔除)" },
                "expected_output": { "type": "string", "description": "期望产出(可选,拼进子任务提示词)" },
                "max_iterations": { "type": "integer", "minimum": 4, "maximum": 32,
                                    "description": "子 Agent 迭代上限,默认取全局配置(12)" }
            },
            "required": ["task"]
        });
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["launch", "batch", "list", "result", "cancel"],
                    "description": "操作类型"
                },
                "task": { "type": "string", "description": "launch:子任务描述(必填)" },
                "agent_type": {
                    "type": "string",
                    "enum": ["general-purpose", "explore", "researcher", "plan", "code-reviewer", "operator"],
                    "description": "launch:子 Agent 类型,默认 general-purpose"
                },
                "name": { "type": "string", "description": "launch:子 Agent 名称(可选)" },
                "system_prompt": { "type": "string", "description": "launch:追加指令(可选)" },
                "tools": { "type": "array", "items": { "type": "string" },
                           "description": "launch:限定工具名(只能收窄)" },
                "expected_output": { "type": "string", "description": "launch:期望产出(可选)" },
                "max_iterations": { "type": "integer", "minimum": 4, "maximum": 32,
                                    "description": "launch:迭代上限(可选)" },
                "background": { "type": "boolean",
                                "description": "launch:true=后台运行,立即返回 run_id(默认 false=同步等待结果)" },
                "tasks": { "type": "array", "minItems": 1, "maxItems": 8, "items": task_item_schema,
                           "description": "batch:并行子任务数组(≤8)" },
                "max_concurrency": { "type": "integer", "minimum": 1, "maximum": 8,
                                     "description": "batch:批内并发上限(默认会话上限 3)" },
                "run_id": { "type": "string", "description": "result / cancel:目标作业句柄" }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let Some(rt) = dynamic_subagent::current() else {
            return Ok(SpawnError::Unavailable.envelope());
        };
        let action = match str_arg(&args, "action") {
            Some(a) => a.to_string(),
            None => {
                return Ok(SpawnError::InvalidArgs(
                    "缺少必填字段 `action`(launch / batch / list / result / cancel)".into(),
                )
                .envelope())
            }
        };

        match action.as_str() {
            // ---------- 自感知快照 ----------
            "list" | "status" => Ok(ok_envelope(rt.snapshot_json())),

            // ---------- 单/批启动 ----------
            "launch" => {
                // 便捷:launch 带 tasks 时自动升级为 batch(减少一次往返)
                if args.get("tasks").and_then(Value::as_array).is_some() {
                    return self.run_batch(&rt, &args).await;
                }
                let req = match parse_request(&args, true) {
                    Ok(r) => r,
                    Err(e) => return Ok(e.envelope()),
                };
                let background = args
                    .get("background")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if background {
                    return match rt.launch_background(req) {
                        Ok(v) => Ok(ok_envelope(v)),
                        Err(e) => Ok(e.envelope()),
                    };
                }
                match rt.launch(req).await {
                    Ok(report) => {
                        let failed = report.status != "ok";
                        let data = report_json(&report);
                        let msg = if failed {
                            format!(
                                "子 Agent {} 未成功:{}",
                                report.run_id,
                                report.error.as_deref().unwrap_or("未知原因")
                            )
                        } else {
                            "ok".to_string()
                        };
                        Ok(envelope(0, &msg, data))
                    }
                    Err(e) => Ok(e.envelope()),
                }
            }

            "batch" => self.run_batch(&rt, &args).await,

            // ---------- 后台作业 ----------
            "result" => match str_arg(&args, "run_id") {
                None => Ok(SpawnError::InvalidArgs("result 需要 `run_id`".into()).envelope()),
                Some(id) => match rt.job_result(id) {
                    Ok(v) => Ok(ok_envelope(v)),
                    Err(e) => Ok(e.envelope()),
                },
            },
            "cancel" => match str_arg(&args, "run_id") {
                None => Ok(SpawnError::InvalidArgs("cancel 需要 `run_id`".into()).envelope()),
                Some(id) => match rt.job_cancel(id) {
                    Ok(v) => Ok(ok_envelope(v)),
                    Err(e) => Ok(e.envelope()),
                },
            },

            other => Ok(SpawnError::InvalidArgs(format!(
                "未知 action `{other}`;可选:launch / batch / list / result / cancel"
            ))
            .envelope()),
        }
    }
}

impl SubAgentTool {
    async fn run_batch(
        &self,
        rt: &std::sync::Arc<dynamic_subagent::SubAgentRuntime>,
        args: &Value,
    ) -> Result<String> {
        let Some(tasks) = args.get("tasks").and_then(Value::as_array) else {
            return Ok(SpawnError::InvalidArgs("batch 需要非空 `tasks` 数组".into()).envelope());
        };
        let mut reqs = Vec::with_capacity(tasks.len());
        for (i, t) in tasks.iter().enumerate() {
            match parse_request(t, true) {
                Ok(r) => reqs.push(r),
                Err(e) => {
                    return Ok(SpawnError::InvalidArgs(format!("tasks[{i}]:{}", e.message()))
                        .envelope())
                }
            }
        }
        let max_concurrency = args
            .get("max_concurrency")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        match rt.batch(reqs, max_concurrency).await {
            Ok(reports) => {
                let ok = reports.iter().filter(|r| r.status == "ok").count();
                let failed = reports.len() - ok;
                let merged = dynamic_subagent::merge_batch_text(&reports);
                let tool_usage: Vec<Value> = reports.iter().map(report_json).collect();
                let data = json!({
                    "count": reports.len(),
                    "ok": ok,
                    "failed": failed,
                    "max_concurrency": max_concurrency.unwrap_or(0),
                    "children": tool_usage,
                    "text": merged,
                });
                Ok(envelope(
                    0,
                    &format!(
                        "批量启动 {} 个子 Agent:成功 {} / 失败 {};请把下面的结果整合成最终回答(不要只说已委派)。",
                        reports.len(),
                        ok,
                        failed
                    ),
                    data,
                ))
            }
            Err(e) => Ok(e.envelope()),
        }
    }
}

/// 描述文本中提到的动作名(供注册面单测与提示词一致性检查)。
pub fn action_names() -> &'static [&'static str] {
    &["launch", "batch", "list", "result", "cancel"]
}

/// 供系统提示词渲染:一句话说明工具用途(避免各处硬编码)。
pub fn hint_line() -> &'static str {
    "- SubAgent(action, task?, agent_type?, name?, tools?, expected_output?, max_iterations?, \
     background?, tasks?, max_concurrency?, run_id?): 启动/管理动态子 Agent;\
     action=list 可自感知(身份/名册/额度),batch 并行启动 ≤8 个。"
}

/// 自感知段的配置(便于单测注入)。
pub fn roster_ids() -> Vec<&'static str> {
    sa::SubAgentType::ALL.iter().map(|t| t.id()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::dynamic_subagent;
    use crate::agent::self_awareness::{SelfAwarenessConfig, SpawnPolicy};
    use crate::config::Protocol;
    use crate::llm::{ChatMessage, Completion, LlmClient, RequestMeta, ToolDef, Usage};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct EchoLlm {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmClient for EchoLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[ToolDef],
            _meta: &RequestMeta,
        ) -> crate::error::Result<Completion> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Completion {
                text: format!("子 Agent#{n} 完成"),
                tool_calls: vec![],
                usage: Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                    ..Usage::default()
                },
                stop_reason: None,
            })
        }
        fn protocol(&self) -> Protocol {
            Protocol::Anthropic
        }
    }

    fn test_session(tag: &str) -> String {
        static N: AtomicUsize = AtomicUsize::new(0);
        format!("sess-tool-{tag}-{}", N.fetch_add(1, Ordering::SeqCst))
    }

    fn envelope_of(s: &str) -> Value {
        serde_json::from_str(s).expect("工具返回必须是 JSON 信封")
    }

    /// 无运行时(功能关闭 / 非委派角色)→ 4001,且要求模型不要重试。
    #[tokio::test]
    async fn without_runtime_returns_4001_unavailable() {
        assert!(dynamic_subagent::current().is_none(), "测试须在作用域外");
        let out = SubAgentTool.execute(json!({"action": "list"})).await.unwrap();
        let v = envelope_of(&out);
        assert_eq!(v["code"], 4001);
        assert!(v["message"].as_str().unwrap().contains("不要重试"));
    }

    #[tokio::test]
    async fn invalid_actions_and_missing_task_return_1001() {
        let session = test_session("invalid");
        let rt = dynamic_subagent::test_runtime(
            Arc::new(EchoLlm {
                calls: AtomicUsize::new(0),
            }),
            &session,
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        dynamic_subagent::scope(rt, async {
            // 缺 action
            let v = envelope_of(&SubAgentTool.execute(json!({})).await.unwrap());
            assert_eq!(v["code"], 1001);
            assert!(v["message"].as_str().unwrap().contains("action"));
            // 未知 action
            let v = envelope_of(&SubAgentTool.execute(json!({"action": "explode"})).await.unwrap());
            assert_eq!(v["code"], 1001);
            // launch 缺 task
            let v = envelope_of(&SubAgentTool.execute(json!({"action": "launch"})).await.unwrap());
            assert_eq!(v["code"], 1001);
            assert!(v["message"].as_str().unwrap().contains("task"));
            // 未知 agent_type
            let v = envelope_of(
                &SubAgentTool
                    .execute(json!({"action": "launch", "task": "x", "agent_type": "ninja"}))
                    .await
                    .unwrap(),
            );
            assert_eq!(v["code"], 1001);
            assert!(v["message"].as_str().unwrap().contains("general-purpose"));
            // batch 缺 tasks
            let v = envelope_of(&SubAgentTool.execute(json!({"action": "batch"})).await.unwrap());
            assert_eq!(v["code"], 1001);
            // result / cancel 缺 run_id
            let v = envelope_of(&SubAgentTool.execute(json!({"action": "result"})).await.unwrap());
            assert_eq!(v["code"], 1001);
            let v = envelope_of(&SubAgentTool.execute(json!({"action": "cancel"})).await.unwrap());
            assert_eq!(v["code"], 1001);
            // 未知 run_id
            let v = envelope_of(
                &SubAgentTool
                    .execute(json!({"action": "result", "run_id": "sa-zzz"}))
                    .await
                    .unwrap(),
            );
            assert_eq!(v["code"], 4003);
        })
        .await;
    }

    #[tokio::test]
    async fn list_returns_self_awareness_snapshot() {
        let session = test_session("list");
        let rt = dynamic_subagent::test_runtime(
            Arc::new(EchoLlm {
                calls: AtomicUsize::new(0),
            }),
            &session,
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        dynamic_subagent::scope(rt, async {
            let v = envelope_of(&SubAgentTool.execute(json!({"action": "list"})).await.unwrap());
            assert_eq!(v["code"], 0);
            assert_eq!(v["data"]["agent"]["can_spawn"], true);
            assert_eq!(v["data"]["limits"]["remaining"], 8);
            assert_eq!(v["data"]["roster"].as_array().unwrap().len(), 6);
            assert_eq!(v["data"]["limits"]["max_parallel"], 3);
            // status 为 list 的别名(容错)
            let v2 = envelope_of(&SubAgentTool.execute(json!({"action": "status"})).await.unwrap());
            assert_eq!(v2["code"], 0);
        })
        .await;
    }

    #[tokio::test]
    async fn launch_and_batch_return_child_reports_and_usage() {
        let session = test_session("launch");
        let rt = dynamic_subagent::test_runtime(
            Arc::new(EchoLlm {
                calls: AtomicUsize::new(0),
            }),
            &session,
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
        );
        dynamic_subagent::scope(rt, async {
            // 单启动
            let v = envelope_of(
                &SubAgentTool
                    .execute(json!({
                        "action": "launch",
                        "task": "调研 A 模块并给出结论",
                        "agent_type": "Explore",
                        "expected_output": "一段结论"
                    }))
                    .await
                    .unwrap(),
            );
            assert_eq!(v["code"], 0);
            assert_eq!(v["data"]["status"], "ok");
            assert_eq!(v["data"]["agent_type"], "explore");
            assert!(v["data"]["text"].as_str().unwrap().contains("子 Agent#0"));
            assert_eq!(v["data"]["usage"]["input_tokens"], 7);

            // launch + tasks 自动升级为 batch
            let v = envelope_of(
                &SubAgentTool
                    .execute(json!({
                        "action": "launch",
                        "tasks": [
                            {"task": "调研 B", "agent_type": "explore"},
                            {"task": "调研 C", "agent_type": "explore"}
                        ],
                        "max_concurrency": 2
                    }))
                    .await
                    .unwrap(),
            );
            assert_eq!(v["code"], 0);
            assert_eq!(v["data"]["count"], 2);
            assert_eq!(v["data"]["ok"], 2);
            assert!(v["data"]["text"].as_str().unwrap().contains("### [explore]"));
            // 汇总提示必须要求整合(防「已委派」空答)
            assert!(v["message"].as_str().unwrap().contains("整合成最终回答"));
        })
        .await;
    }

    #[tokio::test]
    async fn depth_limit_reported_through_tool_envelope() {
        let session = test_session("depth");
        let cfg = SelfAwarenessConfig {
            max_depth: 1,
            ..SelfAwarenessConfig::default()
        };
        let rt = dynamic_subagent::test_runtime(
            Arc::new(EchoLlm {
                calls: AtomicUsize::new(0),
            }),
            &session,
            SpawnPolicy::FullChildren,
            1,
            cfg,
        );
        dynamic_subagent::scope(rt, async {
            let v = envelope_of(
                &SubAgentTool
                    .execute(json!({"action": "launch", "task": "再开一个"}))
                    .await
                    .unwrap(),
            );
            assert_eq!(v["code"], 2002);
            assert!(v["message"].as_str().unwrap().contains("叶子"));
        })
        .await;
    }

    #[test]
    fn schema_and_helpers_are_consistent() {
        let tool = SubAgentTool;
        let schema = tool.parameters();
        let actions: Vec<String> = schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            actions,
            action_names().iter().map(|s| s.to_string()).collect::<Vec<_>>()
        );
        // batch 上限与实现常量一致
        assert_eq!(schema["properties"]["tasks"]["maxItems"], 8);
        assert_eq!(roster_ids().len(), 6);
        assert!(hint_line().contains("SubAgent"));
        assert!(hint_line().contains("action=list"));
    }
}
