//! Chromium-WebUse Agent(第 11 角色,浏览器操控层):执行网页浏览与浏览器操作。
//!
//! 与 SubAgent-Work 平级的「专项执行单元」:由 Main-Work 拆解 WorkFlow 时按
//! `delegate_to = "webuse"` 委派,持 Read + BrowserNew / BrowserList / BrowserClose /
//! BrowserControl / BrowserInspect 工具集,CDP 协议与浏览器进程管理封闭在
//! `agent::browser` 驱动层(chromiumoxide,Chrome / Edge / Chromium,Windows/macOS/Linux)。
//!
//! 结构与 [`crate::agent::subagent::SubAgentRunner`] 完全一致:独立 sub-Session、
//! `run_session_cancellable`、ExecutionTrace、Agent-Memory 落库 —— QC /
//! SessionContext / Debug / 取消传播 / 并行调度全链路复用。
//!
//! 设计见 `docs/浏览器CDP工具/04-Chromium-WebUse-Agent设计与解决方案.md`。

use std::sync::Arc;

use serde_json::Value;
use tracing::{info, warn};

use crate::agent::agent_message::AgentMessageManager;
use crate::agent::cancel::CancelToken;
use crate::agent::context::AgentRole;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::memory;
use crate::agent::subagent::{SubFlowInput, SubFlowOutcome};
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Role, Usage};

/// 2026-09-16 第 64 轮:从 BrowserNew tool_result JSON 中提取 page_id 的正则。
///
/// 仅匹配 BrowserNew 成功时返回的 `data.page_id:"p_xxxxxxxx"`(8 位 hex),
/// 防止误匹配其它字段。BrowserNew 失败(code=3001 等)时无 page_id,正则不命中。
pub fn extract_page_id_from_text(text: &str) -> Option<String> {
    const PREFIX: &str = "\"page_id\":\"p_";
    let start = text.find(PREFIX)?;
    let after = &text[start + PREFIX.len()..];
    let end = after
        .find('"')
        .map(|i| start + PREFIX.len() + i + 1)
        .unwrap_or(text.len());
    let pid_start = start + PREFIX.len() - 2; // 含 "p_"
    Some(text[pid_start..end].trim_matches('"').to_string())
}

/// 2026-09-17 第 78 轮 P0-3 扩展:从 JSON Value 中递归提取字符串内容。
///
/// 解决 AI 对话网站(文心一言 / ChatGPT / DeepSeek)返回的嵌套字段:
/// `data.choices[0].message.content` / `data.markdown` / `data.reply_text` /
/// `data.result.text` / `data.content` 等异构命名。
///
/// 策略:
/// - 字符串 → 直接返回
/// - 数组 → 遍历元素递归查找
/// - 对象 → 优先 `content` / `message` / `text` 字段;都失败则深度优先遍历
/// - 其它类型(数字/布尔/null) → None
fn extract_nested_string(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(s) = extract_nested_string(item) {
                return Some(s);
            }
        }
        return None;
    }
    if let Some(obj) = v.as_object() {
        // 优先 message.content (OpenAI / DeepSeek 风格)
        if let Some(m) = obj.get("message") {
            if let Some(s) = extract_nested_string(m) {
                return Some(s);
            }
        }
        // 优先 content (直接字段)
        if let Some(c) = obj.get("content") {
            if let Some(s) = extract_nested_string(c) {
                return Some(s);
            }
        }
        // 优先 text
        if let Some(t) = obj.get("text") {
            if let Some(s) = extract_nested_string(t) {
                return Some(s);
            }
        }
        // 优先 result (DeepSeek / 自研 API 风格)
        if let Some(r) = obj.get("result") {
            if let Some(s) = extract_nested_string(r) {
                return Some(s);
            }
        }
        // 优先 markdown
        if let Some(m) = obj.get("markdown") {
            if let Some(s) = extract_nested_string(m) {
                return Some(s);
            }
        }
        // 优先 reply / answer / reply_text / ai_answer
        for key in &["reply", "answer", "reply_text", "ai_answer", "value", "outer_html", "html", "body"] {
            if let Some(found) = obj.get(*key).and_then(extract_nested_string) {
                return Some(found);
            }
        }
        // 兜底:深度优先遍历所有字段
        for (_, v) in obj {
            if let Some(s) = extract_nested_string(v) {
                return Some(s);
            }
        }
    }
    None
}

/// 2026-09-17 第 76 轮:从 WebUse 子会话中提取「真实从浏览器抓到的可读文本」。
///
/// 业务背景(来自 llaew_20260917_130701.log):
/// WebUse 抓取 AI 回复后,LLM 终答(iter=23)常输出模板句"任务已完成,AI回复已提取,
/// 内容超过 500 字符",把真实回复吞掉。Runner 出口必须把 BrowserInspect/eval_js
/// 抓到的真实文本追加到 outcome.text 末尾,让 QC 与 TUI 看到原文。
///
/// 提取策略:
/// 1. 倒序遍历 sub_session,过滤 role=Tool 的 ChatMessage;
/// 2. 对每个 tool_result.content 做 JSON 解析,寻找 `code==0 && data.{text|outer_html|...}`;
/// 3. ★ 2026-09-17 第 78 轮 P0-3 扩展:同时支持嵌套字段(`data.choices[0].message.content` /
///    `data.markdown` / `data.content` / `data.result.text` 等异构命名),通过
///    `extract_nested_string` 递归展开;
/// 4. 取长度最长且 ≥ 50 字符的候选,跳过纯 CSS/JSON 短串;
/// 5. 返回 `Some((text, source_tool))` —— text 已 trim,长度截断 8000 字符。
pub fn extract_page_reply_from_session(messages: &[ChatMessage]) -> Option<(String, String)> {
    // ★ 第 82 轮 P0-3:从 50 字符提升到 200,且过滤 placeholder/UI 文本启发式白名单,
    // 避免抓取 input placeholder / 热搜推荐词 / 弹窗广告等 UI 文本误判为 AI 回复。
    // 实测 2026-09-17 文心一言任务,placeholder="潍坊市寒亭区委书记王勇被查"(实时热搜)
    // 被当 AI 回复贴出,Runner 出口出现"任务已完成,内容超过 500 字符"占位句。
    const MIN_CHARS_AI_REPLY: usize = 200;
    let mut best: Option<(String, String, usize)> = None; // (text, source_tool, len)

    // 倒序遍历,优先取最近一次抓取结果(避免旧结果覆盖)
    for msg in messages.iter().rev() {
        if msg.role != Role::Tool {
            continue;
        }
        for block in &msg.content {
            let (tool_use_id, content, _is_error) = match block {
                crate::llm::ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => (tool_use_id.clone(), content.clone(), *is_error),
                _ => continue,
            };
            if content.trim().is_empty() {
                continue;
            }
            // 仅解析 JSON 信封
            let parsed: Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // 失败码直接跳过
            if parsed.get("code").and_then(Value::as_i64) != Some(0) {
                continue;
            }
            let data = match parsed.get("data") {
                Some(d) => d,
                None => continue,
            };

            // ★ 第 78 轮 P0-3:先用候选字段集快速匹配,失败再走递归嵌套提取
            // 候选字段涵盖主流 CDP 响应形态
            const CANDIDATE_KEYS: &[&str] = &[
                "text", "outer_html", "value", "html", "body",
                "reply", "answer", "markdown", "content",
                "reply_text", "ai_answer", "result", "result_text",
            ];
            let mut picked: Option<(String, &'static str)> = None;
            for key in CANDIDATE_KEYS {
                if let Some(s) = data.get(key).and_then(Value::as_str) {
                    let s = s.trim();
                    // ★ 第 82 轮 P0-3:阈值 50 → 200,且过滤 placeholder/UI 文本
                    if s.chars().count() >= MIN_CHARS_AI_REPLY && !is_likely_ui_text(s) {
                        picked = Some((s.to_string(), *key));
                        break;
                    }
                }
            }
            // ★ 候选字段都没命中 → 递归嵌套提取(覆盖 choices[0].message.content /
            //   result.text / data.text / message.content 等异构命名)
            if picked.is_none() {
                if let Some(extracted) = extract_nested_string(data) {
                    let trimmed = extracted.trim();
                    // ★ 第 82 轮 P0-3:阈值 50 → 200,且新增 placeholder/UI 文本启发式过滤,
                    // 避免抓取 placeholder/快捷短语/输入框 placeholder 等 UI 文本误判为 AI 回复。
                    // 实测 2026-09-17 16:25 文心一言任务,placeholder="潍坊市寒亭区委书记王勇被查"
                    // (实时热搜新闻)被当 AI 回复贴出,用户看到的是"任务已完成,内容超过 500 字符"。
                    if trimmed.chars().count() >= MIN_CHARS_AI_REPLY
                        && !is_likely_ui_text(trimmed)
                    {
                        picked = Some((trimmed.to_string(), "nested"));
                    }
                }
            }

            if let Some((text, label)) = picked {
                let len = text.chars().count();
                let better = match &best {
                    Some((_, _, l)) => len > *l,
                    None => true,
                };
                if better {
                    best = Some((text, format!("tool_result[{}]→{}", tool_use_id, label), len));
                }
            }
        }
    }

    best.map(|(text, source, len)| {
        // 截断到 8000 字符,避免 LLM 上下文中爆
        const MAX_CHARS: usize = 8000;
        let truncated = text.chars().count() > MAX_CHARS;
        let kept: String = if truncated {
            text.chars().take(MAX_CHARS).collect::<String>() + "\n...(已截断,原长="
                + &len.to_string()
                + "字符)"
        } else {
            text.to_string()
        };
        info!(
            extracted_chars = kept.chars().count(),
            source = %source,
            "WebUse 真实页面文本已抓取,准备追加到 Runner 出口"
        );
        (kept, source)
    })
}

/// ★ 第 82 轮 P0-3:启发式判定一段文本是否是 UI 占位文本(input placeholder / 热搜推荐 /
/// 弹窗广告 / 输入框默认提示),而非 AI 生成的真实回复。
///
/// 触发任一即视为 UI 文本,Runner 出口丢弃:
/// - 文本以常见 UI 占位开头("请输入"/"搜索"/"你好,我是" 等)
/// - 文本包含 input/textarea placeholder 特征属性名(placeholder= / data-placeholder=)
/// - 文本以列表形态开始(常见热搜推荐:"- 标题1\n- 标题2")
/// - 文本里以"你可能想"等推荐短语打头
/// - 文本是 url 列表(每行 < 100 字且每行含 http:// 或 https://)
fn is_likely_ui_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    const UI_PREFIXES: &[&str] = &[
        "请输入",
        "搜索",
        "search",
        "你好",
        "您好",
        "你可能想",
        "试试问",
        "示例:",
        "热门",
        "推荐",
        "placeholder=",
        "data-placeholder=",
    ];
    if UI_PREFIXES
        .iter()
        .any(|p| trimmed.to_lowercase().starts_with(p))
    {
        return true;
    }
    // 文本以列表形态开始(每行 `- xxx` 或 `1. xxx`)
    let first_line = trimmed.lines().next().unwrap_or("");
    if first_line.starts_with("- ") || first_line.starts_with("• ") {
        return true;
    }
    // URL 列表:行数 ≥ 3 且每行 < 100 字且每行都含 http:// 或 https://
    let lines: Vec<&str> = trimmed.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() >= 3 && lines.iter().all(|l| l.chars().count() < 100) {
        if lines
            .iter()
            .all(|l| l.contains("http://") || l.contains("https://") || l.contains("www."))
        {
            return true;
        }
    }
    // 短文本(< 100 字)直接视为 UI
    if trimmed.chars().count() < 100 {
        return true;
    }
    false
}

/// Chromium-WebUse 执行器(浏览器网页操控专项单元)。
pub struct WebUseRunner {
    agent: Agent,
    /// ★ 2026-09-17 第 79 轮 P1-3:多轮复用副本(不带首迭代强制 BrowserNew)。
    /// 已有存活浏览器页面时用本副本跑单元,让 LLM 首迭代自由选择
    /// (BrowserInspect 直接观察已有页面),避免重复开页/丢登录态。
    agent_reuse: Agent,
    db: Arc<Db>,
    #[allow(dead_code)]
    max_iterations: usize,
    /// Agent 间消息管理器。
    msg_mgr: AgentMessageManager,
}

impl WebUseRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        // 2026-09-16 第 63 轮:WebUse 首迭代强制调用 BrowserNew。
        // 解决"16 次迭代 tool_calls=0"的根因问题:LLM 第 1 轮经常返回纯文本
        // ("让我先...")而不是直接调用 BrowserNew,导致 Agent 循环把纯文本
        // 当作成功回答返回,trace.tool_calls=0 → QC 判失败。
        // 首迭代强制 tool_choice={"type":"tool","name":"BrowserNew"},
        // 后续轮次恢复 auto,让 LLM 自由决策。
        let agent = Agent::new(llm, AgentProfile::web_use_profile())
            .with_first_iter_forced_tool("BrowserNew");
        // 2026-09-17 第 74 轮:WebUse 默认迭代上限从 16 → 32。
        // 环境变量 LAEW_WEBUSE_MAX_ITER 可覆盖(范围 8-128)。
        let default_max = std::env::var("LAEW_WEBUSE_MAX_ITER")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n >= 8 && *n <= 128)
            .unwrap_or(32);
        let agent = agent.with_max_iterations(default_max);
        let agent_reuse = agent.replicate_without_forced_tool();
        let max_iterations = agent.max_iterations();
        let msg_mgr = AgentMessageManager::new(db.clone());
        Self {
            agent,
            agent_reuse,
            db,
            max_iterations,
            msg_mgr,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self.agent = self.agent.with_max_iterations(n);
        self.agent_reuse = self.agent_reuse.with_max_iterations(n);
        self
    }

    /// 跑一次浏览器操控单元(不可取消版本,语义对齐 SubAgentRunner::run_unit)。
    #[allow(dead_code)]
    pub async fn run_unit(&self, input: &SubFlowInput, session_id: &str) -> Result<SubFlowOutcome> {
        self.run_unit_inner(input, session_id, None, None).await
    }

    /// 可取消版本:任务级取消 token 传入 Agent 循环,LLM/工具即时中断。
    ///
    /// 2026-09-16 第 66 轮:单元总超时(默认 300s,环境变量 LAEW_WEBUSE_TIMEOUT),
    /// 防止 Agent 循环 16 轮迭代总耗时过长(用户反馈 WebUse 任务卡住 58.8s)。
    /// ★ 2026-09-17 第 78 轮:可选 progress 通道,Runner 出口抓取到真实页面文本时,
    ///   通过 progress 通道发一条 [laew] 通知给 TUI 用户「正在抓取 AI 回复」+ 前 200 字预览,
    ///   避免 TUI 静默期超过 30s 让用户以为卡死。
    pub async fn run_unit_with_cancel(
        &self,
        input: &SubFlowInput,
        session_id: &str,
        cancel: &CancelToken,
    ) -> Result<SubFlowOutcome> {
        self.run_unit_with_cancel_progress(input, session_id, cancel, None).await
    }

    /// 带进度通道的可取消版本(2026-09-17 第 78 轮 P1-3 新增)。
    pub async fn run_unit_with_cancel_progress(
        &self,
        input: &SubFlowInput,
        session_id: &str,
        cancel: &CancelToken,
        progress: Option<crate::agent::orchestrator::ProgressTx>,
    ) -> Result<SubFlowOutcome> {
        let unit_timeout = std::env::var("LAEW_WEBUSE_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(300);
        tokio::time::timeout(
            std::time::Duration::from_secs(unit_timeout),
            self.run_unit_inner(input, session_id, Some(cancel), progress),
        )
        .await
        .map_err(|_| {
            crate::error::AgentError::Llm(format!(
                "WebUse 单元执行超时({}s),请检查网络或稍后重试",
                unit_timeout
            ))
        })?
    }

    async fn run_unit_inner(
        &self,
        input: &SubFlowInput,
        session_id: &str,
        cancel: Option<&CancelToken>,
        progress: Option<crate::agent::orchestrator::ProgressTx>,
    ) -> Result<SubFlowOutcome> {
        // ★ 2026-09-17 第 79 轮 P1-3:探测存活页面(顺带让 list_pages 清理失效 entry)。
        // 有存活页面 → 注入「已打开页面」提示 + 用无强制 BrowserNew 的 Agent 副本;
        // 无存活页面 → 维持第 63 轮行为(首迭代强制 BrowserNew,防纯文本空转)。
        let live_pages = crate::agent::browser::BrowserManager::global().list_pages().await;
        let pages_hint = build_existing_pages_hint(&live_pages);
        let agent_for_run: &Agent = if live_pages.is_empty() {
            &self.agent
        } else {
            info!(
                live_pages = live_pages.len(),
                "WebUse 多轮复用:注入已打开页面,跳过首迭代强制 BrowserNew"
            );
            &self.agent_reuse
        };

        // 复用 SubFlowInput 的 prompt 构造(原始 prompt 优先 + 摘要 + 上下游产物),
        // 尾部追加浏览器操控作业规范。
        let mut prompt = input.to_user_prompt();
        if let Some(hint) = &pages_hint {
            prompt.push_str("\n\n");
            prompt.push_str(hint);
        }
        prompt.push_str(
            "\n\n【浏览器操控作业规范】\n\
             1. 第一步用 BrowserNew 打开目标页面拿到 page_id(默认无头内存浏览器 mode=hidden);\
                ★若上方有「已打开的浏览器页面」列表,优先直接操作那些页面,不要重复开页;\n\
             2. 后续所有操作都带 page_id:BrowserControl 执行动作、BrowserInspect 观察结果;\n\
             3. 点击链接/新开标签页时,注意响应里的 spawned_page_id,操作新页面要用新 id;\n\
             4. 错误码对策:2000 → 重新 BrowserList 同步索引;2002 → 换 selector 或换 \
                input_text 的 use_js 路径;3001 → 本机未安装 Chrome/Edge/Chromium,如实告知用户;\n\
             5. 截图优先 save_path 落盘;DOM 提取注意 truncated 标记,分段提取;\n\
             6. 禁止对疑似支付/删除/确认提交类按钮做无把握点击;只读操作优先;\n\
             7. 任务完成后关闭确定不再需要的页面;对话型页面(文心一言/ChatGPT 等,用户可能\
                继续追问)可保留——后续任务会自动复用「已打开的浏览器页面」,进程退出时回收。\n\
             8. 浏览器启动模式:BrowserNew 默认 mode=hidden(纯 CDP 无窗口,不会弹出 macOS 系统浏览器)。\n\
                只有当用户明确要求「看截图/可视化调试」时才用 mode=headed。\n\
             9. BrowserNew 成功返回的 next_steps 字段是关键引导,里面列了 input_text/click/wait/elements\n\
                四步最常见动作的 selector_hint,严格按 next_steps 顺序执行可大幅提升成功率。\n\
             10. 复杂页面(微信公众号后台、电商后台)务必先 BrowserInspect(info=elements) 探测真实\n\
                DOM 结构(尤其是动态加载的输入框/按钮),不要凭 selector 名字硬猜。\n\
             11. ★2026-09-17 第 75 轮:若 BrowserNew 返回 code=3001(未检测到 Chrome/Edge/Chromium),\n\
                这是确定性失败(浏览器不可执行),请立即在最终回答里直接告知用户安装引导,\n\
                **不要再尝试别的浏览器启动方式**(xdg-open/open/etc 都不在 Bash 白名单),\n\
                不要循环重试 BrowserNew,不要改 mode 重试。本机没浏览器 = 任务不可完成。\n\
             12. ★2026-09-17 第 76 轮:终答必须包含真实页面文本,禁止敷衍。\n\
                - 任务完成后,必须用 BrowserInspect(info=elements, include_text=true) 或\n\
                  BrowserControl(action=eval_js) 把目标节点的真实文本抓到工具返回值里;\n\
                - 最终回答里**必须把抓到的真实文本完整贴出来**(200-4000 字,带结构),\n\
                  不得仅用「AI回复已提取,内容超过 N 字符」「任务已完成,内容涵盖...」\n\
                  等描述性占位句。Runner 会从 sub_session 抓取最长 tool_result 文本兜底\n\
                  追加,但请你主动把真实内容写到终答里,避免二次抽象漂移。\n\
             13. ★2026-09-17 第 79 轮:你的第一个动作必须是**工具调用**(BrowserNew / \
                BrowserInspect / BrowserControl 均可),不允许先输出纯文本描述。",
        );

        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        // 注入待处理的 Agent 消息(如有)。
        if let Some(msg) = self.msg_mgr.peek(session_id, AgentRole::WebUse).await {
            let agent_msg = format!(
                "【来自其他 Agent 的消息】\n{}\n请基于此消息继续操作。",
                msg.hint()
            );
            sub_session
                .context_mut()
                .push(ChatMessage::user(&agent_msg));
        }

        // ★2026-09-17 第 75 轮:提取 Runner 角色信息,供 collect_failure_signals 计算
        // delegate_mismatch 弱信号 + TUI [路由错配] 诊断行。
        let runner_role = Some(AgentRole::WebUse);
        let intended_role = input.intended_role;

        // 早终止路径语义与 SubAgentRunner 对齐:包装成失败摘要文本 + trace,
        // 交给 Quality-Check 判定,而不是直接升级为 Error。
        // ★ 第 79 轮 P1-3:按存活页面状态选择 Agent(冷启动强制 BrowserNew / 多轮自由决策)。
        let (text, usage, mut trace) = match agent_for_run
            .run_session_cancellable(&mut sub_session, cancel)
            .await
        {
            Ok((t, u, tr)) => (t, u, tr),
            Err(AgentError::RepeatedToolFailure {
                tool,
                attempts,
                last_error,
                trace: carried,
            }) => {
                let summary = format!(
                    "[RepeatedToolFailure] 工具 {tool} 连续 {attempts} 次失败;last_error: {last_error}"
                );
                // 2026-09-17 第 75 轮:使用 Agent 循环携带的真实 trace(工具调用历史不丢;
                // early_terminated/reason/max_consecutive_failures 已在抛出点设置)
                let mut tr = *carried;
                tr.runner_role = runner_role;
                tr.intended_role = intended_role;
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(AgentError::MaxIterationsExceeded {
                iterations: n,
                trace: carried,
            }) => {
                let summary = format!("[MaxIterationsExceeded] 迭代达到 {n} 次上限未得到最终答案");
                // 2026-09-17 第 75 轮:使用 Agent 循环携带的真实 trace(工具调用历史不丢)
                let mut tr = *carried;
                tr.runner_role = runner_role;
                tr.intended_role = intended_role;
                tr.iterations = n;
                tr.early_terminated = true;
                tr.early_terminate_reason = format!("max_iter:{n}");
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(e) => return Err(e), // Cancelled / Llm 等真正错误依然上抛
        };

        // 2026-09-17 第 75 轮:把 Runner 实际角色 + WorkFlow 期望角色写入 trace,
        // 供 collect_failure_signals 计算 delegate_mismatch 弱信号。
        trace.runner_role = runner_role;
        trace.intended_role = intended_role;
        trace.collect_failure_signals(&text);

        // Runner 出口兜底:0 工具调用且无动作关键词 → 强制标 failed。
        let looks_like_action = looks_like_web_ops_action(&text);
        if trace.tool_calls == 0 && !looks_like_action {
            warn!(
                web_use_runner = "no_tool_use_no_action_text",
                text_len = text.len(),
                "WebUse 单元未发出任何工具调用,触发出口兜底"
            );
            trace.early_terminated = true;
            trace.early_terminate_reason =
                format!("no_tool_use_no_action_text:text_len={}", text.len());
            trace.collect_failure_signals(&text);
        }
        let failed = trace.is_failed();

        let error_summary_owned: Option<String> = if failed {
            if !trace.early_terminate_reason.is_empty() {
                Some(trace.early_terminate_reason.clone())
            } else {
                Some(trace.failure_signals.join(","))
            }
        } else {
            None
        };
        let error_summary = error_summary_owned.as_deref();

        // 2026-09-17 第 76 轮:Runner 出口文本强制附加「真实从浏览器抓取的可读文本」。
        // 解决 llaew_20260917_130701.log 中"Agent 显示提取内容成功了,但是相关的结果
        // 没有显示出来呀"的根本原因:LLM 终答常输出"任务已完成,AI回复已提取,内容
        // 超过 500 字符"模板句,把真实回复吞掉;Runner 必须从 sub_session 的 tool_result
        // 中反查最长一段可读文本,作为最终产物追加。
        let text = {
            let extracted = extract_page_reply_from_session(sub_session.context());
            match extracted {
                Some((reply, source)) if !reply.trim().is_empty() => {
                    // ★ 2026-09-17 第 78 轮 P1-3:通过 progress 通道发 [laew] 通知,
                    // 让 TUI 任务执行中就能看到「正在抓取 AI 回复」+ 前 200 字预览,
                    // 避免 LLM 终答全是描述性占位句时 TUI 静默让用户以为卡死。
                    if let Some(tx) = &progress {
                        let preview: String = reply.chars().take(200).collect();
                        let tail = if reply.chars().count() > 200 { "..." } else { "" };
                        let _ = tx.send(format!(
                            "[laew] WebUse 出口兜底 | 抓取 {} 字符(来源: {})\n预览: {}{}",
                            reply.chars().count(),
                            source,
                            preview,
                            tail
                        ));
                    }
                    let mut combined = text;
                    if !combined.trim().is_empty() {
                        combined.push_str("\n\n");
                    }
                    combined.push_str(&format!(
                        "[来自浏览器抓取的真实页面回复,共 {} 字符,来源: {}]\n{}",
                        reply.chars().count(),
                        source,
                        reply,
                    ));
                    combined
                }
                _ => text,
            }
        };

        let _ = memory::record_entry(
            &self.db,
            AgentRole::WebUse,
            session_id,
            &input.description,
            &text,
            error_summary,
            serde_json::json!({
                "subflow_id": &input.id,
                "expected": &input.expected_output,
                "trace": &trace,
            }),
        );

        Ok(SubFlowOutcome {
            text,
            usage,
            failed,
            trace,
        })
    }
}

/// ★ 2026-09-17 第 79 轮 P1-3:构造「已打开的浏览器页面」注入提示块。
///
/// 输入为 `BrowserManager::list_pages()` 的 `(page_id, url, title, created_at)` 列表;
/// 空列表返回 None(冷启动,不注入)。纯函数,便于单测。
pub fn build_existing_pages_hint(pages: &[(String, String, String, String)]) -> Option<String> {
    if pages.is_empty() {
        return None;
    }
    let mut out = String::from(
        "【已打开的浏览器页面(可直接复用,免重新打开/登录)】\n",
    );
    for (id, url, title, _ts) in pages.iter().take(10) {
        let title_disp = if title.trim().is_empty() { "(无标题)" } else { title.as_str() };
        out.push_str(&format!("  - page_id={id} | 标题: {title_disp} | URL: {url}\n"));
    }
    out.push_str(
        "优先用 BrowserInspect / BrowserControl 直接操作上述页面;仅当任务需要其它网址\n\
         或页面已失效(code=2000)时才 BrowserNew 新开。你的第一个动作必须是工具调用。",
    );
    Some(out)
}

/// 浏览器操作动作关键词探测(出口兜底用)。
///
/// 语义:无工具调用时,文本含中英文
/// 动作关键词或超过阈值,视为「有实质内容」,否则强制标 failed。
///
/// 2026-09-16 第 63 轮:阈值从 200 → 100 字符,避免"伪动作描述"逃过 QC
/// (如"我会先用 BrowserNew 打开页面..."这种纯文本意图描述)。
fn looks_like_web_ops_action(text: &str) -> bool {
    if text.len() > 100 {
        return true;
    }
    const KEYWORDS: &[&str] = &[
        "已打开",
        "已点击",
        "已输入",
        "已截图",
        "已抓取",
        "已采集",
        "已登录",
        "已导航",
        "页面标题",
        "page_id",
        "spawned_page_id",
        "opened",
        "clicked",
        "typed",
        "screenshot",
        "navigated",
        "crawled",
        "browser_",
        "BrowserNew",
        "BrowserControl",
        "BrowserInspect",
        "title",
    ];
    KEYWORDS.iter().any(|k| text.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::profile::WEB_USE_AGENT_NAME;
    use crate::agent::AgentProfile;

    // 2026-09-16 第 64 轮:验证 page_id 提取函数对常见 BrowserNew 返回格式生效。
    #[test]
    fn extract_page_id_parses_success_envelope() {
        let txt = r#"{"code":0,"message":"ok","data":{"page_id":"p_a1b2c3d4","title":"文心一言","final_url":"https://wenxin.baidu.com/"}}"#;
        let pid = extract_page_id_from_text(txt);
        assert_eq!(pid.as_deref(), Some("p_a1b2c3d4"));
    }

    #[test]
    fn extract_page_id_returns_none_for_failure() {
        // code=3001 时不含 page_id
        let txt = r#"{"code":3001,"message":"未检测到浏览器","data":{"install":"请安装 Chrome"}}"#;
        assert_eq!(extract_page_id_from_text(txt), None);
    }

    #[test]
    fn extract_page_id_handles_non_json() {
        assert_eq!(extract_page_id_from_text("plain text"), None);
        assert_eq!(extract_page_id_from_text(""), None);
    }

    // ★ 第 82 轮 P0-3:UI 文本启发式过滤单元测试
    #[test]
    fn is_likely_ui_text_filters_placeholders() {
        // 输入框 placeholder
        assert!(is_likely_ui_text("请输入你的问题"));
        assert!(is_likely_ui_text("搜索"));
        assert!(is_likely_ui_text("潍坊市寒亭区委书记王勇被查")); // 热搜词,80字
        // URL 列表(每行短且含 http)
        assert!(is_likely_ui_text(
            "- https://example.com/page1\n- https://example.com/page2\n- https://example.com/page3"
        ));
        // 列表开头
        assert!(is_likely_ui_text("- 新对话\n- 工作任务\n- 知识库"));
        // 短文本
        assert!(is_likely_ui_text("登录"));
        assert!(is_likely_ui_text(""));
        // 真实 AI 回复:长文 + 非 UI 开头 + 无 URL 列表
        assert!(!is_likely_ui_text(
            "根据最近3个月的黄金白银走势分析:7月份国际金价从853元/克上涨至906元/克,8月份突破1000元大关达到1012.85元/克的历史高点,9月份有所回落收于926.86元/克。白银方面,7月份在54-58美元区间震荡,8月份突破70美元后回落至63.80美元。综合来看,近期金银价格波动较大,投资者需注意风险控制。"
        ));
    }

    #[test]
    fn extract_page_reply_skips_placeholder_with_50_chars() {
        // ★ 第 82 轮 P0-3:旧阈值 50 字符下,placeholder "潍坊市寒亭区委书记王勇被查"(13字符)
        // 长度不够不会进 best,但若 LLM 抓一段 80 字符的搜索推荐词,旧版会误判;
        // 新版 is_likely_ui_text 拦截 + 阈值 200 双重保险。
        let placeholder_80chars = "潍坊市寒亭区委书记王勇被查,某某某最新消息,某某某官方回应,持续关注中";
        let msgs = vec![ChatMessage::tool_result(
            "t1",
            &format!(
                r#"{{"code":0,"message":"ok","data":{{"text":"{}"}}}}"#,
                placeholder_80chars
            ),
            false,
        )];
        // 80 字符 < 200 阈值,直接 None
        let result = extract_page_reply_from_session(&msgs);
        assert!(result.is_none(), "短 placeholder 文本不应被采纳");
    }

    #[test]
    fn extract_page_reply_skips_url_list() {
        // URL 列表型文本即使 200+ 字符也应被 UI 过滤拦截
        let url_list = (0..10)
            .map(|i| format!("- https://example.com/page{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let msgs = vec![ChatMessage::tool_result(
            "t1",
            &format!(
                r#"{{"code":0,"message":"ok","data":{{"text":"{}"}}}}"#,
                url_list
            ),
            false,
        )];
        let result = extract_page_reply_from_session(&msgs);
        assert!(result.is_none(), "URL 列表型 UI 文本不应被采纳");
    }

    // 2026-09-17 第 76 轮:验证从 sub_session 中提取最长可读文本。
    // ★ 第 82 轮 P0-3:阈值提升到 200,文本相应加长。
    #[test]
    fn extract_page_reply_picks_longest_text() {
        let long_text = "黄金近3个月走势详细分析报告:7月份国际金价从853元/克持续上涨至906元/克,8月份突破1000元大关达到1012.85元/克的历史高点,9月份有所回落收于926.86元/克。白银方面,7月份在54-58美元区间震荡,8月份突破70美元后回落至63.80美元。综合来看,近期金银价格波动较大,投资者需密切关注美联储利率政策、地缘政治风险以及美元指数走势,合理配置资产以分散风险,以上分析仅供参考。";
        let short = "ok";
        let msgs = vec![
            ChatMessage::tool_result("t1", short, false),
            ChatMessage::tool_result("t2", &format!(
                r#"{{"code":0,"message":"ok","data":{{"text":"{}"}}}}"#,
                long_text
            ), false),
        ];
        let result = extract_page_reply_from_session(&msgs);
        assert!(result.is_some(), "应能提取到长文本(实际: {result:?})");
        let (text, _) = result.unwrap();
        assert!(text.contains("1012.85"), "应包含真实原文片段");
    }

    #[test]
    fn extract_page_reply_skips_failure_code() {
        // code=2002 失败时不应被当作提取源;文本长度 ≥ 200(第 82 轮阈值提升)
        let long_text = "实际回复文本长度足够长通过门槛测试,这是文心一言生成的金银价格走势详细分析报告,包含国内国际金价白银价格数据。7月份国际金价从853元/克持续上涨至906元/克,8月份突破1000元大关达到1012.85元/克的历史高点,9月份回落至926.86元/克,白银方面7月份54-58美元区间震荡,8月突破70美元后回落63.80美元。投资者需密切关注美联储利率政策、地缘政治风险以及美元指数走势,合理配置资产以分散风险,以上分析仅供参考。";
        let msgs = vec![
            ChatMessage::tool_result("t1", r#"{"code":2002,"message":"err","data":{}}"#, false),
            ChatMessage::tool_result("t2", &format!(r#"{{"code":0,"message":"ok","data":{{"text":"{}"}}}}"#, long_text), false),
        ];
        let result = extract_page_reply_from_session(&msgs);
        assert!(result.is_some());
    }

    #[test]
    fn extract_page_reply_returns_none_when_no_text() {
        // 只有 metadata,没有 text 字段
        let msgs = vec![
            ChatMessage::tool_result("t1", r#"{"code":0,"message":"ok","data":{"page_id":"p_x"}}"#, false),
        ];
        let result = extract_page_reply_from_session(&msgs);
        assert!(result.is_none());
    }

    #[test]
    fn web_use_profile_uses_web_registry() {
        let p = AgentProfile::web_use_profile();
        assert_eq!(p.name, WEB_USE_AGENT_NAME);
        let names: Vec<_> = p.tools.names().iter().map(|s| s.to_string()).collect();
        assert!(names.contains(&"Read".to_string()));
        assert!(names.contains(&"BrowserNew".to_string()));
        assert!(names.contains(&"BrowserList".to_string()));
        assert!(names.contains(&"BrowserClose".to_string()));
        assert!(names.contains(&"BrowserControl".to_string()));
        assert!(names.contains(&"BrowserInspect".to_string()));
        // WebUse 不带 Bash/Write(网页操控收窄权限面)
        assert!(!names.contains(&"Bash".to_string()));
        assert!(!names.contains(&"Write".to_string()));
        assert!(p.emit_tool.is_none());
    }

    #[test]
    fn runner_flags_zero_tool_calls_as_failed() {
        // 2026-09-16 第 63 轮:阈值从 200 → 100,纯意图描述不算动作
        let text = "我需要先打开页面然后才能继续操作";
        assert!(!looks_like_web_ops_action(text), "无关键词不应判为动作");
        assert!(text.len() <= 100, "短文本不算有实质内容");
    }

    #[test]
    fn runner_passes_action_keyword_text_as_success() {
        for text in [
            "已打开 example.com 并截图",
            "已点击登录按钮",
            "page_id=p_ab12cd34 页面标题:Example",
            "screenshot saved",
        ] {
            assert!(
                looks_like_web_ops_action(text),
                "动作关键词应判为有动作:{text}"
            );
        }
    }

    #[test]
    fn runner_passes_long_text_as_success_via_length_fallback() {
        let long_text = "x".repeat(250);
        assert!(looks_like_web_ops_action(&long_text));
    }

    // ================== 2026-09-17 第 78 轮 P0-3 新增:嵌套字段提取测试 ==================

    #[test]
    fn extract_nested_string_handles_choices_message_content() {
        // 模拟 OpenAI 风格嵌套响应:data.choices[0].message.content
        let v = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "content": "AI 回复内容长度足够长通过门槛测试,实际是文心一言给的金价分析"
                    }
                }
            ]
        });
        let result = extract_nested_string(&v);
        assert!(result.is_some());
        assert!(result.unwrap().contains("金价分析"));
    }

    #[test]
    fn extract_nested_string_handles_markdown_field() {
        let v = serde_json::json!({
            "markdown": "# 标题\n这是 AI 生成的 markdown 内容,长度足够长通过门槛测试,包含代码块和列表"
        });
        let result = extract_nested_string(&v);
        assert!(result.is_some());
        assert!(result.unwrap().contains("代码块"));
    }

    #[test]
    fn extract_nested_string_handles_reply_text() {
        let v = serde_json::json!({
            "reply_text": "AI 直接回复文本,长度足够长通过门槛测试"
        });
        let result = extract_nested_string(&v);
        assert!(result.is_some());
    }

    #[test]
    fn extract_nested_string_returns_none_for_non_string_types() {
        let v = serde_json::json!({"count": 42, "flag": true, "items": null});
        let result = extract_nested_string(&v);
        assert!(result.is_none(), "纯数字/布尔/null 应返回 None");
    }

    #[test]
    fn extract_page_reply_handles_nested_choices() {
        // 实际嵌套:tool_result.content = {"code":0, "data": {"choices": [{"message": {"content": "..."}}]}}
        // ★ 第 82 轮 P0-3:文本加长到 ≥ 200 字符,通过 MIN_CHARS_AI_REPLY 阈值
        let nested = r#"{"code":0,"message":"ok","data":{"choices":[{"message":{"content":"AI 回复: 根据最近3个月的黄金白银价格走势详细分析报告。国内金价从7月份的853元/克持续上涨至8月份的1012.85元/克历史高点,9月份回落至926.86元/克。国际金价目前4301.95美元/盎司,国际白银63.80美元/盎司,沪银主力15586元/千克。整体来看,近期金银价格波动较大,投资者需密切关注美联储利率政策、地缘政治风险以及美元指数走势,合理配置资产以分散风险。数据来源文心一言实时查询,以上价格仅供参考,实际交易以市场为准。"}}]}}"#;
        let msgs = vec![ChatMessage::tool_result("t1", nested, false)];
        let result = extract_page_reply_from_session(&msgs);
        assert!(result.is_some(), "嵌套字段应能提取,实际值: {result:?}");
    }

    // ================== 2026-09-17 第 79 轮 P1-3:已打开页面注入提示 ==================

    #[test]
    fn build_existing_pages_hint_empty_returns_none() {
        assert!(build_existing_pages_hint(&[]).is_none(), "空列表不应注入提示");
    }

    #[test]
    fn build_existing_pages_hint_lists_page_id_url_title() {
        let pages = vec![
            (
                "p_ab12cd34".to_string(),
                "https://wenxin.baidu.com/".to_string(),
                "文心一言".to_string(),
                "1758100000000".to_string(),
            ),
            (
                "p_ff00ee11".to_string(),
                "https://example.com/".to_string(),
                String::new(), // 空标题 → (无标题)
                "1758100000001".to_string(),
            ),
        ];
        let hint = build_existing_pages_hint(&pages).expect("非空列表应返回提示块");
        assert!(hint.contains("已打开的浏览器页面"), "应含标题行: {hint}");
        assert!(hint.contains("page_id=p_ab12cd34"));
        assert!(hint.contains("文心一言"));
        assert!(hint.contains("https://wenxin.baidu.com/"));
        assert!(hint.contains("(无标题)"), "空标题应有占位: {hint}");
        assert!(hint.contains("第一个动作必须是工具调用"), "应含首步工具要求: {hint}");
        assert!(hint.contains("BrowserNew"), "应说明何时才新开页面: {hint}");
    }

    #[test]
    fn build_existing_pages_hint_caps_at_ten_pages() {
        let pages: Vec<(String, String, String, String)> = (0..15)
            .map(|i| (
                format!("p_{i:08x}"),
                format!("https://example.com/{i}"),
                format!("标题{i}"),
                "1758100000000".to_string(),
            ))
            .collect();
        let hint = build_existing_pages_hint(&pages).unwrap();
        assert!(hint.contains("p_00000009"), "前 10 个页面应列出");
        assert!(!hint.contains("p_0000000a"), "第 11 个起应截断");
    }

    // ================== 2026-09-17 第 79 轮 P1-3:多轮复用 Agent 副本 ==================

    #[test]
    fn runner_holds_reuse_agent_without_forced_tool() {
        // Agent::replicate_without_forced_tool 语义:副本无 forced tool,原 Agent 保留。
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
                    text: String::new(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
            fn protocol(&self) -> crate::config::Protocol {
                crate::config::Protocol::Anthropic
            }
        }
        let agent = crate::agent::Agent::new(
            std::sync::Arc::new(NoopLlm),
            AgentProfile::web_use_profile(),
        )
        .with_first_iter_forced_tool("BrowserNew");
        assert_eq!(agent.first_iter_forced_tool(), Some("BrowserNew"));
        let reuse = agent.replicate_without_forced_tool();
        assert_eq!(reuse.first_iter_forced_tool(), None, "副本不应带首迭代强制工具");
        assert_eq!(reuse.max_iterations(), agent.max_iterations(), "迭代上限应复制");
        assert_eq!(reuse.profile().name, agent.profile().name, "profile 应复制");
    }

    #[test]
    fn web_use_profile_tools_hint_lists_round76_actions() {
        // 第 79 轮 P1-4:工具说明必须告知第 76 轮扩展动作,否则 LLM 不知道能力存在。
        let rendered = AgentProfile::web_use_profile()
            .system_prompt
            .render(crate::config::Protocol::Anthropic);
        for action in ["drag", "focus", "blur", "mouse_move", "dispatch_event"] {
            assert!(
                rendered.contains(action),
                "WebUse 系统提示词应包含动作 {action}(与 BrowserControl Schema 对齐)"
            );
        }
    }
}
