//! 用量与参数摘要辅助(2026-09-17 自 orchestrator.rs 拆分)。

use super::*;


/// 2026-09-16 第 67 轮:工具参数摘要(供 [laew] 工具调用明细行)。
///
/// 从 ToolCallLogEntry.args_json(稳定序列化)提取关键字段拼 `k=v` 列表:
/// query / filter / window_id / path / action / text / x / y / command / url / selector,
/// 截 60 字符 —— 微信视觉路线复盘时能直接看到「点了哪个坐标 / 输了什么文本」。
///
/// 2026-09-17 第 74 轮:差异化处理 MCP_* / Bash / Read / Write / Edit / Glob / Grep 工具,
/// 在 tool 前缀独立标识, 减少 TUI 信息密度(用户主诉「输出太多看不清」):
/// - MCP_Web_Use(第 89 轮单工具化):action=open url=X / control act=X sel=Y text=Z / inspect info=X
/// - MCP_Window_Use:action=X query=Y / control act=X path=Y
/// - Bash:command=X(截 40)
/// - Read/Write/Edit:path=X(截 50)
/// - Glob:pattern=X
/// - Grep:pattern=X
pub(super) fn tool_args_digest(tool_name: &str, args_json: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return String::new();
    };
    let Some(obj) = v.as_object() else {
        return String::new();
    };
    // 工具级差异化分支(2026-09-17 第 74 轮)
    match tool_name {
        "MCP_Web_Use" => {
            // ★ 2026-09-18 第 89 轮:浏览器操控统一入口差异化摘要(原 Browser* 四分支合并)。
            // 每个 action 只显示排查必需的最小字段,值统一截 ≤30 字符:
            // - open:url=X mode=Y;list:(无参);close:page_id=X
            // - control:act=X sel=Y text=Z url=W
            // - inspect:info=X sel=Y
            let g = |k: &str| {
                obj.get(k)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let action = g("action").unwrap_or_default();
            let mut parts = vec![format!("action={action}")];
            match action.as_str() {
                "open" => {
                    if let Some(u) = g("url") {
                        parts.push(format!("url={}", truncate_progress_text(&u, 36)));
                    }
                    if let Some(m) = g("mode") {
                        parts.push(format!("mode={m}"));
                    }
                }
                "close" => {
                    if let Some(p) = g("page_id") {
                        parts.push(format!("page={}", truncate_progress_text(&p, 14)));
                    }
                }
                "control" | "inspect" => {
                    if let Some(p) = g("page_id") {
                        parts.push(format!("page={}", truncate_progress_text(&p, 14)));
                    }
                    if action == "control" {
                        if let Some(a) = g("control_action") {
                            parts.push(format!("act={a}"));
                        }
                    } else if let Some(i) = g("info") {
                        parts.push(format!("info={i}"));
                    }
                    if let Some(params) = obj.get("params").and_then(|v| v.as_object()) {
                        let ps = |k: &str| {
                            params
                                .get(k)
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .map(str::to_string)
                        };
                        if let Some(sel) = ps("selector") {
                            parts.push(format!("sel={}", truncate_progress_text(&sel, 22)));
                        }
                        if let Some(text) = ps("text") {
                            parts.push(format!("text={}", truncate_progress_text(&text, 26)));
                        }
                        if let Some(url) = ps("url") {
                            // 第 99 轮:data: URL 只报长度(实测 140KB base64 撑爆摘要行)
                            if url.starts_with("data:") {
                                parts.push(format!("url=[data:{}字符]", url.chars().count()));
                            } else {
                                parts.push(format!("url={}", truncate_progress_text(&url, 28)));
                            }
                        }
                        if let Some(key) = ps("key") {
                            parts.push(format!("key={}", truncate_progress_text(&key, 14)));
                        }
                        // 第 99 轮:eval_js 表达式头(排查「到底执行了什么 JS」)
                        if let Some(expr) = ps("expression")
                            .or_else(|| ps("script"))
                            .or_else(|| ps("function"))
                        {
                            parts.push(format!("js={}", truncate_progress_text(&expr, 30)));
                        }
                        // 第 99 轮:OCR 标记(验证码任务一眼可辨)
                        if params.get("ocr").and_then(|v| v.as_bool()) == Some(true) {
                            parts.push("ocr".to_string());
                        }
                        // 第 99 轮:screenshot/download 落盘目标(只报文件名)
                        if let Some(sp) = ps("save_path").or_else(|| ps("save_dir")) {
                            let name = std::path::Path::new(&sp)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or(&sp);
                            parts.push(format!("out={}", truncate_progress_text(name, 20)));
                        }
                        if let Some(ms) = params.get("timeout_ms").and_then(|v| v.as_u64()) {
                            parts.push(format!("timeout={}ms", ms));
                        }
                    }
                }
                _ => {}
            }
            truncate_progress_text(&parts.join(" "), 90)
        }
        "MCP_Use" => {
            // ★ 2026-09-23 第 123 轮:通用 MCP 服务调用差异化摘要。
            // 只显示 action=X server=Y tool=Z / uri=U(值截 ≤30 字符),不回显 arguments
            // (可能是大 JSON / 敏感数据)。
            let g = |k: &str| {
                obj.get(k)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let action = g("action").unwrap_or_default();
            let mut parts = vec![format!("action={action}")];
            if let Some(s) = g("server") {
                parts.push(format!("server={}", truncate_progress_text(&s, 20)));
            }
            match action.as_str() {
                "call_tool" => {
                    if let Some(t) = g("tool") {
                        parts.push(format!("tool={}", truncate_progress_text(&t, 30)));
                    }
                }
                "read_resource" => {
                    if let Some(u) = g("uri") {
                        parts.push(format!("uri={}", truncate_progress_text(&u, 30)));
                    }
                }
                _ => {}
            }
            truncate_progress_text(&parts.join(" "), 70)
        }
        "Bash" => {
            // ★ 2026-09-17 第 78 轮 P1-1:大命令精简
            // ★ 第 99 轮:长命令不再倾倒正文前 80 字符 —— 实测 python base64 解码
            // 脚本把摘要行变成 140KB 乱码头;只保留首行 + heredoc 标注 + 总长度。
            let cmd = obj.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let cmd_chars = cmd.chars().count();
            let cmd_first_line = cmd.lines().next().unwrap_or("").to_string();
            if cmd_chars > 200 {
                let heredoc = if cmd.contains("<<") { " 含heredoc" } else { "" };
                let first_line_short = truncate_progress_text(&cmd_first_line, 40);
                truncate_progress_text(
                    &format!("cmd=[共{cmd_chars}字符{heredoc}] 首行:{first_line_short}"),
                    160,
                )
            } else {
                truncate_progress_text(
                    &format!("cmd={}", truncate_progress_text(cmd, 80)),
                    120,
                )
            }
        }
        "Read" => {
            // ★ 第 82 轮 P1-1 增强:Read 工具展示 path + offset + limit(便于排查大文件读取)
            let path = obj.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let mut parts = vec![format!("path={}", truncate_progress_text(path, 60))];
            if let Some(off) = obj.get("offset").and_then(|v| v.as_u64()) {
                parts.push(format!("off={}", off));
            }
            if let Some(lim) = obj.get("limit").and_then(|v| v.as_u64()) {
                parts.push(format!("limit={}", lim));
            }
            truncate_progress_text(&parts.join(" "), 160)
        }
        "Write" | "Edit" => {
            // ★ 2026-09-17 第 78 轮 P1-1:Write/Edit 大数据量精简
            // file_path 完整保留(< 80),但 Write/Edit 的 content 字段经常是 5KB+ 脚本,
            // 不能在 TUI stage 完整展示;改为展示 path + content 总字符数 + 首行
            let path = obj.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let content = obj.get("content").and_then(|v| v.as_str());
            let mut parts = vec![format!("path={}", truncate_progress_text(path, 50))];
            if let Some(c) = content {
                let total_chars = c.chars().count();
                if total_chars > 200 {
                    let first_line = c.lines().next().unwrap_or("").to_string();
                    parts.push(format!(
                        "content=[共{total_chars}字符] 首行:{}",
                        truncate_progress_text(&first_line, 40)
                    ));
                } else if total_chars > 0 {
                    parts.push(format!("content={}", truncate_progress_text(c, 60)));
                }
            }
            // Edit 工具展示 old_string 截断(供排查匹配位置)
            if tool_name == "Edit" {
                if let Some(old) = obj.get("old_string").and_then(|v| v.as_str()) {
                    let first_line = old.lines().next().unwrap_or("");
                    parts.push(format!(
                        "old=[{}字符] 首行:{}",
                        old.chars().count(),
                        truncate_progress_text(first_line, 30)
                    ));
                }
            }
            truncate_progress_text(&parts.join(" "), 200)
        }
        "Glob" => {
            let pat = obj.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            truncate_progress_text(&format!("pattern={}", truncate_progress_text(pat, 40)), 80)
        }
        "Grep" => {
            let pat = obj.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = obj.get("path").and_then(|v| v.as_str()).unwrap_or("");
            truncate_progress_text(
                &format!("pattern={} path={}", truncate_progress_text(pat, 30), truncate_progress_text(path, 30)),
                80,
            )
        }
        "MCP_Window_Use" => {
            // ★ 2026-09-18 第 84 轮:窗口操控统一入口差异化摘要。
            // 每个 action 只显示排查必需的最小字段,值统一截 ≤24 字符:
            // - open:query=X;list:filter=X;find:query=X mode=Y
            // - inspect:win=X depth=N filter=Y;ocr:win=X lang=Y;screenshot:win=X out=Y
            // - control:act=X path=Y(坐标动作附 @x,y;文本截 16)
            let g = |k: &str| {
                obj.get(k)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let action = g("action").unwrap_or_default();
            let mut parts = vec![format!("action={action}")];
            match action.as_str() {
                "open" | "find" => {
                    if let Some(q) = g("query") {
                        parts.push(format!("query={}", truncate_progress_text(&q, 24)));
                    }
                }
                "list" | "inspect" => {
                    if let Some(w) = g("window_id") {
                        parts.push(format!("win={}", truncate_progress_text(&w, 16)));
                    }
                    if let Some(f) = g("filter") {
                        parts.push(format!("filter={}", truncate_progress_text(&f, 16)));
                    }
                }
                "explore" | "run_sequence" | "input_batch" | "chat_send" | "chat_loop" => {
                    if let Some(w) = g("window_id") {
                        parts.push(format!("win={}", truncate_progress_text(&w, 16)));
                    }
                    push_window_composite_core(obj, &mut parts);
                    if matches!(action.as_str(), "chat_send" | "chat_loop") {
                        if let Some(t) = g("text") {
                            parts.push(format!("text={}", truncate_progress_text(&t, 16)));
                        }
                    }
                }
                "ocr" | "screenshot" => {
                    if let Some(w) = g("window_id") {
                        parts.push(format!("win={}", truncate_progress_text(&w, 16)));
                    }
                }
                "control" => {
                    if let Some(a) = g("control_action") {
                        parts.push(format!("act={a}"));
                    }
                    if let Some(p) = g("path") {
                        parts.push(format!("path={}", truncate_progress_text(&p, 12)));
                    }
                    if let Some(t) = g("text") {
                        parts.push(format!("text={}", truncate_progress_text(&t, 16)));
                    }
                    if let (Some(x), Some(y)) = (
                        obj.get("x").and_then(|v| v.as_i64()),
                        obj.get("y").and_then(|v| v.as_i64()),
                    ) {
                        parts.push(format!("@{x},{y}"));
                    }
                }
                _ => {}
            }
            // run_sequence/input_batch 的错误策略比步骤明细更能解释失败链路。
            if matches!(action.as_str(), "run_sequence" | "input_batch") {
                if let Some(e) = g("on_error") {
                    parts.push(format!("on_error={e}"));
                }
            }
            // 复合 action 需要多保留 log/snapshot 路径;外层仍强制截断,
            // 保持 TUI 一行可读。
            truncate_progress_text(&parts.join(" "), 110)
        }
        _ => {
            // 通用 fallback: 提取已知的几个关键字段
            const KEYS: &[&str] = &[
                "query", "filter", "window_id", "path", "action", "text", "x", "y", "command", "url",
                "selector", "max_depth", "lang",
            ];
            let mut parts = Vec::new();
            for k in KEYS {
                if let Some(val) = obj.get(*k) {
                    let vs = match val {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if vs.is_empty() {
                        continue;
                    }
                    parts.push(format!("{k}={}", truncate_progress_text(&vs, 24)));
                }
            }
            truncate_progress_text(&parts.join(" "), 60)
        }
    }
}

pub(super) fn add_usage(mut total: Usage, delta: Usage) -> Usage {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.cache_read_input_tokens = total
        .cache_read_input_tokens
        .saturating_add(delta.cache_read_input_tokens);
    total.cache_creation_input_tokens = total
        .cache_creation_input_tokens
        .saturating_add(delta.cache_creation_input_tokens);
    total
}

pub(super) fn failure_usage(_failure: &QualityFailure) -> Usage {
    // QC fail 时由 QualityFailure 携带 SubAgent + QC 用量;Agent 错误路径为默认零。
    // 外层调用方只累加一次,避免与下一次执行层返回值重复计算。
    _failure.usage.clone()
}

/// MCP_Window_Use 复合 action 的最小核心字段:定位窗口 + 批量规模 + 产物路径。
fn push_window_composite_core(
    obj: &serde_json::Map<String, serde_json::Value>,
    parts: &mut Vec<String>,
) {
    let g = |k: &str| {
        obj.get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    if let Some(q) = g("query") {
        parts.push(format!("query={}", truncate_progress_text(&q, 20)));
    }
    if let Some(app) = g("app_name") {
        parts.push(format!("app={}", truncate_progress_text(&app, 20)));
    }
    if let Some(label) = g("snapshot_label") {
        parts.push(format!("label={}", truncate_progress_text(&label, 16)));
    }
    if let Some(path) = g("snapshot_path")
        .or_else(|| g("log_path"))
        .or_else(|| g("chat_log_path"))
    {
        parts.push(format!("out={}", truncate_progress_text(&path, 36)));
    }
    if let Some(count) = obj
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .or_else(|| {
            obj.get("messages")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
        })
    {
        parts.push(format!("n={count}"));
    }
    if let Some(count) = obj.get("max_rounds").and_then(serde_json::Value::as_u64) {
        parts.push(format!("rounds<={count}"));
    }
    if let Some(secs) = obj
        .get("interval_seconds")
        .and_then(serde_json::Value::as_u64)
    {
        parts.push(format!("interval={secs}s"));
    }
    if let Some(path) = g("input_field_path") {
        parts.push(format!("path={}", truncate_progress_text(&path, 14)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-17 第 74 轮:tool_args_digest 差异化测试。
    // 2026-09-18 第 89 轮:Browser* 四分支合并为 MCP_Web_Use 单工具摘要。

    #[test]
    fn tool_args_digest_mcp_web_use_open() {
        let args = r#"{"action":"open","url":"https://wenxin.baidu.com/","mode":"hidden"}"#;
        let s = tool_args_digest("MCP_Web_Use", args);
        assert!(s.contains("action=open"), "实际: {s}");
        assert!(s.contains("mode=hidden"), "实际: {s}");
        assert!(s.contains("url=https://wenxin.baidu.com/"), "实际: {s}");
    }

    #[test]
    fn tool_args_digest_mcp_web_use_control() {
        let args = r#"{"action":"control","page_id":"p_abc","control_action":"input_text","params":{"selector":"textarea","text":"hello world"}}"#;
        let s = tool_args_digest("MCP_Web_Use", args);
        assert!(s.contains("act=input_text"), "实际: {s}");
        assert!(s.contains("sel=textarea"), "实际: {s}");
        assert!(s.contains("text=hello world"), "实际: {s}");
        assert!(s.contains("page=p_abc"), "实际: {s}");
    }

    #[test]
    fn tool_args_digest_mcp_web_use_inspect() {
        let args = r#"{"action":"inspect","page_id":"p_abc","info":"elements","params":{"selector":".response"}}"#;
        let s = tool_args_digest("MCP_Web_Use", args);
        assert!(s.contains("info=elements"), "实际: {s}");
        assert!(s.contains("sel=.response"), "实际: {s}");
    }

    #[test]
    fn tool_args_digest_mcp_web_use_close() {
        let args = r#"{"action":"close","page_id":"p_ff00ee11"}"#;
        let s = tool_args_digest("MCP_Web_Use", args);
        assert!(s.contains("action=close"), "实际: {s}");
        assert!(s.contains("page=p_ff00ee11"), "实际: {s}");
    }

    #[test]
    fn tool_args_digest_bash() {
        let args = r#"{"command":"git status"}"#;
        let s = tool_args_digest("Bash", args);
        assert!(s.contains("cmd=git status"));
    }

    #[test]
    fn tool_args_digest_read_write_edit() {
        let args = r#"{"file_path":"/tmp/foo.txt"}"#;
        let s = tool_args_digest("Read", args);
        assert!(s.contains("path=/tmp/foo.txt"));
        let s = tool_args_digest("Write", args);
        assert!(s.contains("path=/tmp/foo.txt"));
        let s = tool_args_digest("Edit", args);
        assert!(s.contains("path=/tmp/foo.txt"));
    }

    #[test]
    fn tool_args_digest_glob_grep() {
        let args = r#"{"pattern":"*.rs"}"#;
        let s = tool_args_digest("Glob", args);
        assert!(s.contains("pattern=*.rs"));
        let args = r#"{"pattern":"fn main","path":"src/"}"#;
        let s = tool_args_digest("Grep", args);
        assert!(s.contains("pattern=fn main"));
        assert!(s.contains("path=src/"));
    }

    #[test]
    fn tool_args_digest_unknown_tool_falls_back() {
        let args = r#"{"query":"hello","x":100,"y":200}"#;
        let s = tool_args_digest("UnknownTool", args);
        assert!(s.contains("query=hello"));
        assert!(s.contains("x=100"));
        assert!(s.contains("y=200"));
    }

    // ============== 2026-09-17 第 81 轮:窗口工具差异化摘要 ==============

    #[test]
    fn tool_args_digest_mcp_window_use() {
        // 2026-09-18 第 84 轮:MCP_Window_Use 统一入口摘要
        // open:action + query
        let s = tool_args_digest("MCP_Window_Use", r#"{"action":"open","query":"微信"}"#);
        assert!(s.contains("action=open"), "实际: {s}");
        assert!(s.contains("query=微信"), "实际: {s}");
        // list:无 filter 时只有 action;有 filter 时显示
        let s = tool_args_digest("MCP_Window_Use", r#"{"action":"list"}"#);
        assert!(s.contains("action=list"), "实际: {s}");
        assert!(!s.contains("filter="), "无 filter 的 list 摘要不应显示 filter: {s}");
        let s = tool_args_digest("MCP_Window_Use", r#"{"action":"list","filter":"微信"}"#);
        assert!(s.contains("filter=微信"), "实际: {s}");
        // inspect:win + filter
        let s = tool_args_digest(
            "MCP_Window_Use",
            r#"{"action":"inspect","window_id":"682:0","max_depth":6,"filter":"发送"}"#,
        );
        assert!(s.contains("win=682:0"), "实际: {s}");
        assert!(s.contains("filter=发送"), "实际: {s}");
        // ocr:win
        let s = tool_args_digest(
            "MCP_Window_Use",
            r#"{"action":"ocr","window_id":"682:0","lang":"zh-Hans-CN"}"#,
        );
        assert!(s.contains("win=682:0"), "实际: {s}");
        // control:act + path + 坐标
        let s = tool_args_digest(
            "MCP_Window_Use",
            r#"{"action":"control","window_id":"682:0","path":"/","control_action":"click_point","x":100,"y":200}"#,
        );
        assert!(s.contains("act=click_point"), "实际: {s}");
        assert!(s.contains("@100,200"), "实际: {s}");
    }

    #[test]
    fn tool_args_digest_window_action_compact() {
        // 2026-09-18 第 84 轮:MCP_Window_Use control 动作:act + path + text(截 16)
        let s = tool_args_digest(
            "MCP_Window_Use",
            r#"{"action":"control","window_id":"682:0","path":"/","control_action":"type_text_submit","text":"你好,这是一条超过十六个字符的测试消息内容"}"#,
        );
        assert!(s.contains("act=type_text_submit"), "实际: {s}");
        assert!(s.contains("path=/"), "实际: {s}");
        assert!(s.contains("text="), "实际: {s}");
        assert!(!s.contains("超过十六个字符的测试消息内容"), "text 必须截断: {s}");
        // 坐标动作:附 @x,y
        let s = tool_args_digest(
            "MCP_Window_Use",
            r#"{"action":"control","window_id":"682:0","path":"/","control_action":"click_point","x":812,"y":402}"#,
        );
        assert!(s.contains("@812,402"), "实际: {s}");
    }

    #[test]
    fn tool_args_digest_window_sequence_and_explore_core_fields() {
        // 第 107 轮:复合 action 不展示全量 steps,但必须能定位窗口/产物/规模。
        let s = tool_args_digest(
            "MCP_Window_Use",
            r#"{"action":"run_sequence","window_id":"918:0","steps":[1,2,3],"on_error":"retry","log_path":"/tmp/laew_sequence.log"}"#,
        );
        assert!(s.contains("action=run_sequence"), "实际: {s}");
        assert!(s.contains("win=918:0"), "实际: {s}");
        assert!(s.contains("n=3"), "实际: {s}");
        assert!(s.contains("on_error=retry"), "实际: {s}");
        assert!(s.contains("out=/tmp/laew_sequence.log"), "实际: {s}");

        let s = tool_args_digest(
            "MCP_Window_Use",
            r#"{"action":"explore","query":"豆包","snapshot_path":"/tmp/snapshot.json","snapshot_label":"doubao"}"#,
        );
        assert!(s.contains("action=explore"), "实际: {s}");
        assert!(s.contains("query=豆包"), "实际: {s}");
        assert!(s.contains("label=doubao"), "实际: {s}");
        assert!(s.contains("out=/tmp/snapshot.json"), "实际: {s}");
    }

    #[test]
    fn tool_args_digest_truncates_long_values() {
        let long = "x".repeat(200);
        let args = format!(r#"{{"command":"{long}"}}"#);
        let s = tool_args_digest("Bash", &args);
        // ★ 第 82 轮 P1-1:Bash cmd 截断上限从 80 提到 120(包含 cmd= 前缀)
        assert!(s.len() <= 130, "摘要应被截断到 130 字符内,实际 {} 字符", s.len());
        assert!(s.starts_with("cmd="));
    }

    #[test]
    fn tool_args_digest_handles_invalid_json() {
        assert_eq!(tool_args_digest("Bash", "not json"), "");
        assert_eq!(tool_args_digest("Read", ""), "");
    }

    #[test]
    fn tool_args_digest_handles_non_object() {
        assert_eq!(tool_args_digest("Bash", "[]"), "");
        assert_eq!(tool_args_digest("Bash", "null"), "");
        assert_eq!(tool_args_digest("Bash", "42"), "");
    }

    // ============== 2026-09-20 第 99 轮:效能复盘实测改进 ==============

    #[test]
    fn tool_args_digest_bash_heredoc_no_base64_dump() {
        // 实测 python base64 解码脚本把摘要行变成 140KB 乱码头;
        // 长命令只保留首行 + heredoc 标注,正文一律不出现。
        let script = format!(
            "python3 << 'PYEOF'\nimport base64\nb64 = \"{}\"\nprint(b64)\nPYEOF",
            "iVBORw0KGgo".repeat(30)
        );
        let args = serde_json::json!({ "command": script }).to_string();
        let s = tool_args_digest("Bash", &args);
        assert!(s.contains("含heredoc"), "应带 heredoc 标注: {s}");
        assert!(s.contains("首行:python3"), "应保留首行: {s}");
        assert!(!s.contains("iVBORw0KGgo"), "base64 正文不得出现在摘要: {s}");
    }

    #[test]
    fn tool_args_digest_web_eval_js_expression_head() {
        let args = r#"{"action":"control","page_id":"p_1","control_action":"eval_js","params":{"expression":"document.querySelector('.code img').src"}}"#;
        let s = tool_args_digest("MCP_Web_Use", args);
        assert!(s.contains("js=document.querySelector"), "应带表达式头: {s}");
    }

    #[test]
    fn tool_args_digest_web_ocr_marker_and_save_path() {
        let args = r#"{"action":"control","page_id":"p_1","control_action":"screenshot","params":{"save_path":"/tmp/laew/captcha.png","ocr":true}}"#;
        let s = tool_args_digest("MCP_Web_Use", args);
        assert!(s.contains("ocr"), "应带 ocr 标记: {s}");
        assert!(s.contains("out=captcha.png"), "应带落盘文件名: {s}");
        assert!(!s.contains("/tmp/laew"), "不应带完整路径: {s}");
    }

    #[test]
    fn tool_args_digest_web_data_url_collapsed() {
        // 实测 data: URL 直灌摘要行;应只报长度
        let args = format!(
            r#"{{"action":"control","page_id":"p_1","control_action":"download","params":{{"url":"data:image/png;base64,{}"}}}}"#,
            "iVBOR".repeat(200)
        );
        let s = tool_args_digest("MCP_Web_Use", &args);
        assert!(s.contains("url=[data:"), "data: URL 应折叠: {s}");
        assert!(!s.contains("iVBOR"), "data 载荷不得出现: {s}");
    }
}
