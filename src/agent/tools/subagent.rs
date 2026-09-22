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

use crate::agent::custom_agents::{self as ca, ResolvedAgentType};
use crate::agent::dynamic_subagent::{self, report_json, SpawnError, SubAgentRequest};
use crate::agent::self_awareness as sa;
use crate::agent::subagent_workflow::{self, WorkflowStep};
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

/// 解析 `agent_type`:内置 6 类(含别名)-> 自定义类型(`.laew/agents/*.md`)。
///
/// 第 115 轮:JSON Schema 的 `enum` 已移除(无法枚举用户文件,且会让
/// `tool_schema_validator` 硬拒自定义 id),校验前移到本函数 —— 未命中时
/// 报 `1001` 并**列出当前可用名册**,让模型一次纠正而不是反复猜。
fn resolve_agent_type(
    raw: &str,
    work_dir: &std::path::Path,
) -> std::result::Result<ResolvedAgentType, SpawnError> {
    let defs = ca::discover_in(work_dir).defs;
    if let Some(t) = ca::resolve_in(raw, &defs) {
        return Ok(t);
    }
    Err(SpawnError::InvalidArgs(format!(
        "未知 agent_type `{raw}`;可选内置类型:{};\
         也可在 {{工作目录}}/.laew/agents/ 或 ~/.laew/agents/ 放一个 `{raw}.md` 定义你自己的类型\
         (frontmatter 支持 extends/tools/readonly/label/description,正文即专属指令);\
         先用 action=\"list\" 查看当前实时名册(共 {} 个可用)",
        sa::SubAgentType::ALL
            .iter()
            .map(|t| t.id())
            .collect::<Vec<_>>()
            .join(" / "),
        ca::available_ids(&defs).len()
    )))
}

/// 解析单个子任务(launch 与 batch.tasks[] 共用)。
fn parse_request(
    v: &Value,
    require_task: bool,
    work_dir: &std::path::Path,
) -> std::result::Result<SubAgentRequest, SpawnError> {
    let task = str_arg(v, "task").unwrap_or("").to_string();
    if require_task && task.is_empty() {
        return Err(SpawnError::InvalidArgs(
            "launch / batch.tasks[] 必须提供非空 `task`(子 Agent 看不到父上下文,任务必须自包含)".into(),
        ));
    }
    let type_raw = str_arg(v, "agent_type").unwrap_or("general-purpose");
    let agent_type = resolve_agent_type(type_raw, work_dir)?;
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
        prior: None,
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
         \n- `history`:查询**已落库**的子 Agent 运行记录(可选 `limit` 默认 10 / `agent_type` / `all_sessions`)。零 LLM 成本,建议在新任务开头先看一眼「上一轮谁干过什么」,能复用结论就别重跑。\
         \n- `resume`:在某个历史结论上继续(必填 `run_id`,可选 `task`;缺省 task = 「基于前序结论继续推进」)。前序任务与结论会作为种子上下文注入新子 Agent,并记录血缘 `resumed_from`;**它消耗一次会话预算**,且是「重新起一个子 Agent」而非恢复被冻结的进程。\
         \n- `result`:取回后台作业(必填 `run_id`);`running` 表示仍在跑。\
         \n- `cancel`:取消后台作业(必填 `run_id`)。\
         \n\n子 Agent 类型(agent_type):general-purpose(通用执行,可写文件/跑命令) / explore(只读侦察) / researcher(资料研究,可用浏览器) / plan(方案设计) / code-reviewer(代码审查) / operator(浏览器/桌面窗口操控)。\
         \n除上述内置类型外,用户/项目可在 `{工作目录}/.laew/agents/*.md` 或 `~/.laew/agents/*.md` 定义**自定义类型**(frontmatter 支持 label / description / extends / tools / readonly,正文=专属指令);直接用其文件名作为 `agent_type` 即可。\
         \n工具收窄:`tools` 只能**收窄**不能扩权(超出父 Agent 能力的会被剔除,并在报告的 dropped_tools 里说明)。\
         \n\n【何时启动】任务可拆成 2 个以上**相互独立**的子任务,或用户提示词明确要求「启动 SubAgent / 并行 / 分工 / 分别调研 / 多个 Agent」时;\
         多个独立子任务请**一次 batch**(比多次 launch 快);有先后依赖时用多次 launch。\
         \n【何时不要启动】单步任务、步骤强依赖、需要你自己看中间结果再决策的任务 —— 直接自己做,启动子 Agent 要额外付一次完整 LLM 成本。\
         \n【硬性要求】task 必须自包含(目标 + 输入路径 + 期望产物 + 验收口径);子 Agent 结果只是中间产物,你**必须**整合成面向用户的最终回答,禁止只回答「已委派」。\
         \n\n返回统一 JSON 信封:`{code, message, data}`。code:0 成功 / 1001 参数非法(含未知 agent_type,消息里会给出可用名册) / 2001 预算耗尽 / 2002 深度超限 / 2003 并发排队超时 / 4001 当前上下文不支持动态启动(不要重试,自己完成) / 4002 已取消 / 4003 run_id 不存在。"
    }

    fn parameters(&self) -> Value {
        let task_item_schema = json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "minLength": 1,
                          "description": "子任务描述(必须自包含:目标 + 输入路径 + 期望产物 + 验收口径;子 Agent 看不到你的上下文)" },
                "agent_type": {
                    "type": "string",
                    "description": "子 Agent 类型 id,默认 general-purpose;内置:general-purpose / explore / researcher / plan / code-reviewer / operator,或 `.laew/agents/*.md` 自定义类型 id(用 action=\"list\" 看实时名册)"
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
        let mut workflow_item_schema = task_item_schema.clone();
        if let Some(obj) = workflow_item_schema.as_object_mut() {
            obj.insert(
                "id".into(),
                json!({
                    "type": "string", "minLength": 1, "maxLength": 64,
                    "description": "DAG 步骤唯一标识;其它步骤用 depends_on 引用它"
                }),
            );
            if let Some(props) = obj.get_mut("properties").and_then(Value::as_object_mut) {
                props.insert(
                    "depends_on".into(),
                    json!({
                        "type": "array", "items": { "type": "string" },
                        "description": "直接上游步骤 id 数组;全部成功后才执行本步骤"
                    }),
                );
            }
            if let Some(req) = obj.get_mut("required").and_then(Value::as_array_mut) {
                req.push(json!("id"));
            }
        }
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "history", "launch", "batch", "workflow", "resume", "result", "cancel"],
                    "description": "操作类型"
                },
                "task": { "type": "string", "description": "launch:子任务描述(必填);resume:追加任务(可选,缺省=基于前序结论继续)" },
                "agent_type": {
                    "type": "string",
                    "description": "launch:子 Agent 类型 id,默认 general-purpose;内置 6 类(id 见工具描述)或 `.laew/agents/*.md` 自定义类型 id(未知 id 返回 1001 并列出可用名册)"
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
                "steps": { "type": "array", "minItems": 1, "maxItems": 8, "items": workflow_item_schema,
                           "description": "workflow:有依赖 DAG 的步骤数组(≤8);id 唯一,depends_on 指向已声明 id" },
                "max_concurrency": { "type": "integer", "minimum": 1, "maximum": 8,
                                     "description": "batch:批内并发上限(默认会话上限 3)" },
                "run_id": { "type": "string", "description": "result / cancel / resume:目标作业句柄" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 50,
                           "description": "history:返回条数上限(默认 10)" },
                "all_sessions": { "type": "boolean",
                                  "description": "history:true=跨会话查询(默认 false=仅本会话)" }
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
                    "缺少必填字段 `action`(launch / batch / workflow / list / result / cancel)".into(),
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
                let req = match parse_request(&args, true, rt.work_dir()) {
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

            // ---------- 有依赖的多阶段 DAG ----------
            "workflow" => self.run_workflow(&rt, &args).await,

            // ---------- 运行记录(history:跨任务工作板视图) ----------
            "history" => self.run_history(&rt, &args),

            // ---------- 续跑(血缘) ----------
            "resume" => self.run_resume(&rt, &args).await,

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
                "未知 action `{other}`;可选:list / history / launch / batch / workflow / resume / result / cancel"
            ))
            .envelope()),
        }
    }
}

impl SubAgentTool {
    /// `workflow`:解析并执行一张小型 DAG。
    async fn run_workflow(
        &self,
        rt: &std::sync::Arc<dynamic_subagent::SubAgentRuntime>,
        args: &Value,
    ) -> Result<String> {
        let Some(raw_steps) = args.get("steps").and_then(Value::as_array) else {
            return Ok(SpawnError::InvalidArgs(
                "workflow 需要非空 `steps` 数组(每个步骤必须有 id 和自包含 task)".into(),
            )
            .envelope());
        };
        let mut steps = Vec::with_capacity(raw_steps.len());
        for raw in raw_steps {
            let Some(id) = str_arg(raw, "id").map(|s| s.to_string()) else {
                return Ok(SpawnError::InvalidArgs(
                    "workflow.steps[] 必须提供非空 `id`".into(),
                )
                .envelope());
            };
            let req = match parse_request(raw, true, rt.work_dir()) {
                Ok(req) => req,
                Err(e) => return Ok(e.envelope()),
            };
            let depends_on = raw
                .get("depends_on")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            steps.push(WorkflowStep {
                id,
                task: req.task,
                agent_type: req.agent_type,
                name: req.name,
                system_prompt: req.system_prompt,
                tools: req.tools,
                expected_output: req.expected_output,
                max_iterations: req.max_iterations,
                depends_on,
            });
        }
        match subagent_workflow::run_workflow(rt, steps).await {
            Ok(outcome) => {
                let data = serde_json::to_value(&outcome).unwrap_or(Value::Null);
                if outcome.status == "cancelled" {
                    return Ok(envelope(
                        4002,
                        "工作流在派发或执行前被取消;已完成部分见 data.steps。",
                        data,
                    ));
                }
                let msg = if outcome.status == "completed" {
                    format!(
                        "工作流完成:{} / {} 个步骤成功。",
                        outcome.succeeded, outcome.total_steps
                    )
                } else {
                    format!(
                        "工作流部分完成:{} 成功 / {} 失败 / {} 跳过 / {} 阻断;请整合成功结论并如实说明未完成阶段。",
                        outcome.succeeded, outcome.failed, outcome.skipped, outcome.blocked
                    )
                };
                Ok(envelope(0, &msg, data))
            }
            Err(e) => Ok(e.envelope()),
        }
    }

    /// `history`:查询已落库的运行记录(零 LLM 成本)。
    ///
    /// 持久化关闭或数据库不可用时返回 **code 0 + persist:false + 空列表** ——
    /// 这是「功能关闭」而非「参数错误」,不能让模型陷入重试循环。
    fn run_history(
        &self,
        rt: &std::sync::Arc<dynamic_subagent::SubAgentRuntime>,
        args: &Value,
    ) -> Result<String> {
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| (n as usize).clamp(1, 50))
            .unwrap_or(10);
        let agent_type = str_arg(args, "agent_type").map(|s| s.to_string());
        let all_sessions = args
            .get("all_sessions")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let session = if all_sessions {
            None
        } else {
            Some(rt.session_id())
        };
        let Some(rows) = dynamic_subagent::query_runs(session, agent_type.as_deref(), limit) else {
            return Ok(envelope(
                0,
                "本会话未开启子 Agent 运行记录持久化(LAEW_SUBAGENT_PERSIST=off)或数据库不可用;\
                 无历史可查,请直接 launch/batch,不要重试本 action。",
                json!({
                    "persist": false,
                    "count": 0,
                    "scope": if all_sessions { "all" } else { "session" },
                    "runs": [],
                }),
            ));
        };
        let runs: Vec<Value> = rows.iter().map(run_row_json).collect();
        let failed = rows.iter().filter(|r| r.status != "ok").count();
        let resumed = rows.iter().filter(|r| r.resumed_from.is_some()).count();
        let data = json!({
            "persist": true,
            "count": runs.len(),
            "failed": failed,
            "resumed": resumed,
            "scope": if all_sessions { "all" } else { "session" },
            "agent_type": agent_type,
            "runs": runs,
            "hint": "如需在某个 run 的结论上继续,调用 action=\"resume\", run_id=... (可带 task 追加要求)。",
        });
        Ok(envelope(
            0,
            &format!(
                "已落库子 Agent 运行记录 {} 条(失败 {});需要复用结论时用 action=\"resume\"。",
                rows.len(),
                failed
            ),
            data,
        ))
    }

    /// `resume`:在既有结论上继续(血缘续跑)。
    ///
    /// 前序任务 + 结论会作为种子上下文注入新子 Agent,并在报告/落库里记录
    /// `resumed_from`。**它消耗一次会话预算** —— 与 launch 走同一条治理路径。
    async fn run_resume(
        &self,
        rt: &std::sync::Arc<dynamic_subagent::SubAgentRuntime>,
        args: &Value,
    ) -> Result<String> {
        let Some(run_id) = str_arg(args, "run_id") else {
            return Ok(
                SpawnError::InvalidArgs("resume 需要 `run_id`(可先用 action=\"history\" 查询)".into())
                    .envelope(),
            );
        };
        let Some(row) = dynamic_subagent::get_run(run_id) else {
            return Ok(SpawnError::UnknownRunId(format!(
                "{run_id}(不存在,或本会话未开启运行记录持久化 LAEW_SUBAGENT_PERSIST)"
            ))
            .envelope());
        };
        // 类型可能已失效(自定义定义文件被删 / 改名):给出可执行的纠偏建议
        let Some(agent_type) = ca::resolve_in(&row.agent_type, &rt.defs()) else {
            return Ok(SpawnError::InvalidArgs(format!(
                "前序作业的类型 `{}` 已不可解析(定义文件被删除或改名?);\
                 {run_id} 的结论仍可在 history 中查看,请用 action=\"list\" 看当前名册后改用 launch。",
                row.agent_type
            ))
            .envelope());
        };

        let task = str_arg(args, "task").map(|s| s.to_string()).unwrap_or_else(|| {
            "基于前序结论,给出下一步可执行的推进(补漏 / 深挖 / 落地),不要重复前序已完成的工作。"
                .to_string()
        });
        // 工具面继承:沿用历史生效面,但剔除 SubAgent(委派权由策略决定,不靠继承),
        // 且仍会 ∩ 当前父策略(防越权)。
        let tools: Vec<String> = row
            .tools
            .iter()
            .filter(|t| t.as_str() != SUBAGENT_TOOL)
            .cloned()
            .collect();
        let req = SubAgentRequest {
            task,
            agent_type,
            name: Some(format!("resume-{run_id}")),
            system_prompt: None,
            tools,
            expected_output: str_arg(args, "expected_output").unwrap_or("").to_string(),
            max_iterations: args
                .get("max_iterations")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).clamp(4, 32)),
            prior: Some(dynamic_subagent::PriorRun {
                run_id: row.run_id.clone(),
                agent_type: row.agent_type.clone(),
                status: row.status.clone(),
                created_at: row.created_at.clone(),
                task: row.task.clone(),
                report: if row.report.trim().is_empty() {
                    row.error.clone().unwrap_or_default()
                } else {
                    row.report.clone()
                },
            }),
        };
        match rt.launch_with_origin(req, "resume").await {
            Ok(report) => {
                let failed = report.status != "ok";
                let data = report_json(&report);
                let msg = if failed {
                    format!(
                        "续跑子 Agent {} 未成功:{}",
                        report.run_id,
                        report.error.as_deref().unwrap_or("未知原因")
                    )
                } else {
                    format!("已在前序结论({run_id})上继续,请整合成最终回答。")
                };
                Ok(envelope(0, &msg, data))
            }
            Err(e) => Ok(e.envelope()),
        }
    }

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
            match parse_request(t, true, rt.work_dir()) {
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
    &[
        "list", "history", "launch", "batch", "workflow", "resume", "result", "cancel",
    ]
}

/// 供系统提示词渲染:一句话说明工具用途(避免各处硬编码)。
pub fn hint_line() -> &'static str {
    "- SubAgent(action, task?, agent_type?, name?, tools?, expected_output?, max_iterations?, \
     background?, tasks?, max_concurrency?, run_id?): 启动/管理动态子 Agent;\
     action=list 自感知(身份/名册/额度),history 查运行记录,resume 在前序结论上继续,\
     batch 并行启动 ≤8 个,workflow 执行 ≤8 步依赖 DAG;agent_type 支持 \
     `.laew/agents/*.md` 自定义类型。"
}

/// 运行记录行 -> JSON(工具 `history` 返回;文本按字符截断,避免撑爆上下文)。
fn run_row_json(r: &crate::config::SubAgentRunRow) -> Value {
    let clip = dynamic_subagent::clip_chars;
    json!({
        "run_id": r.run_id,
        "name": r.name,
        "agent_type": r.agent_type,
        "status": r.status,
        "origin": r.origin,
        "resumed_from": r.resumed_from,
        "task": clip(r.task.trim(), 200),
        "report": clip(r.report.trim(), 500),
        "error": r.error.as_deref().map(|e| clip(e, 300)),
        "tools": r.tools,
        "dropped_tools": r.dropped_tools,
        "iterations": r.iterations,
        "tool_calls": r.tool_calls,
        "wallclock_ms": r.wallclock_ms,
        "usage": {"input_tokens": r.input_tokens, "output_tokens": r.output_tokens},
        "created_at": r.created_at,
    })
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
            // 第 115 轮:错误消息必须给出「怎么自定义」与「怎么看清名册」的出路
            let msg = v["message"].as_str().unwrap();
            assert!(msg.contains(".laew/agents"), "应提示自定义定义文件位置");
            assert!(msg.contains("action=\"list\""), "应提示先用 list 看实时名册");
            // resume / history 参数校验
            let v = envelope_of(&SubAgentTool.execute(json!({"action": "resume"})).await.unwrap());
            assert_eq!(v["code"], 1001);
            let v = envelope_of(
                &SubAgentTool
                    .execute(json!({"action": "resume", "run_id": "sa-nope"}))
                    .await
                    .unwrap(),
            );
            assert_eq!(v["code"], 4003, "未知 run_id 应报 4003");
            // history:持久化未开启时返回 code 0 + persist=false(不得让模型重试)
            let v = envelope_of(&SubAgentTool.execute(json!({"action": "history"})).await.unwrap());
            assert_eq!(v["code"], 0);
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

    /// 第 115 轮:自定义类型(`.laew/agents/*.md`)经工具真实派发,并进实时名册。
    #[tokio::test]
    async fn custom_agent_type_resolved_and_dispatched_from_work_dir() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join(".laew").join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("fe-reviewer.md"),
            "---\nlabel: 前端审查\ntools: Read, Glob\n---\n只给问题清单,不写文件。\n",
        )
        .unwrap();
        let session = test_session("custom");
        let rt = dynamic_subagent::test_runtime_in(
            Arc::new(EchoLlm {
                calls: AtomicUsize::new(0),
            }),
            &session,
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
            dir.path().to_path_buf(),
        );
        dynamic_subagent::scope(rt, async {
            let v = envelope_of(
                &SubAgentTool
                    .execute(json!({
                        "action": "launch",
                        "task": "审查 src/tui 的最近改动",
                        "agent_type": "fe-reviewer"
                    }))
                    .await
                    .unwrap(),
            );
            assert_eq!(v["code"], 0, "自定义类型应可派发: {v}");
            assert_eq!(v["data"]["agent_type"], "fe-reviewer");
            assert_eq!(v["data"]["origin"], "launch");
            assert_eq!(v["data"]["tools"], json!(["Read", "Glob"]));

            // 实时名册:自定义类型在列且带 source 路径
            let s = envelope_of(&SubAgentTool.execute(json!({"action": "list"})).await.unwrap());
            let roster = s["data"]["roster"].as_array().unwrap();
            let custom = roster
                .iter()
                .find(|r| r["id"] == "fe-reviewer")
                .expect("自定义类型应进实时名册");
            assert_eq!(custom["custom"], true);
            assert!(custom["source"].as_str().unwrap().ends_with("fe-reviewer.md"));
            assert_eq!(roster.len(), 7, "内置 6 + 自定义 1");
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
        // 第 115 轮:自定义类型 + 记录/续跑
        assert!(hint_line().contains(".laew/agents"));
        assert!(hint_line().contains("resume"));
        assert!(hint_line().contains("history"));
        assert!(
            schema["properties"]["agent_type"].get("enum").is_none(),
            "agent_type 必须自由字符串(自定义类型无法枚举)"
        );
        for key in ["limit", "all_sessions"] {
            assert!(schema["properties"][key].is_object(), "history 参数 {key} 应存在");
        }
        // 第 116 轮:workflow 是同一工具里的轻量 DAG action
        assert_eq!(schema["properties"]["steps"]["maxItems"], 8);
        assert!(schema["properties"]["steps"]["items"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "id"));
        assert!(schema["properties"]["steps"]["items"]["properties"]["depends_on"].is_object());
        assert!(hint_line().contains("workflow"));
    }
}
