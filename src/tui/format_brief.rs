//! TUI 工具输出摘要子模块(2026-09-18 第 88 轮新增)。
//!
//! 拆分背景:`format.rs` 已达 1700+ 临界线,按「单源码文件 ≤1800 行」规范,
//! 新增的 MCP_Window_Use 输出摘要函数落到本职责子模块;
//! 既有 `browser_output_brief` / `tool_args_brief` 仍在 format.rs(未机械搬移,
//! 后续轮次如需继续增长可整体迁入)。

use super::format::clip_cols;

/// MCP_Web_Use `{code,message,data}` 信封的精简摘要。
///
/// ★ 2026-09-17 第 79 轮 P2-5 引入(format.rs);第 89 轮随 MCP_Web_Use 更名;
/// **第 99 轮迁入本子模块**(format.rs 已达临界线)+ 扩展:
/// - `save_path` → `out=<文件名>`、`byte_size` → `bytes=N`(screenshot/download 落盘);
/// - `reused=true` → `reused`(open 复用一眼可辨);
/// - `ocr_text` → `ocr_len=N`、`ocr_error` → `err=...`(验证码 OCR 结果);
/// - `result`(eval_js)→ `result=<头 24 字>`;`result_truncated` → 长度标注;
/// - `closed`(close all)→ `closed=N`。
/// 解析失败回退 None(调用方走普通截断)。
pub(crate) fn browser_output_brief(output_summary: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(output_summary.trim()).ok()?;
    if !v.is_object() {
        return None;
    }
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    let message = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
    let data = v.get("data").cloned().unwrap_or(serde_json::Value::Null);
    let mut parts: Vec<String> = vec![format!("code={code}")];
    if !message.is_empty() {
        parts.push(clip_cols(message, 24));
    }
    if let Some(d) = data.as_object() {
        for key in ["page_id", "url", "title", "spawned_page_id"] {
            if let Some(s) = d.get(key).and_then(|x| x.as_str()) {
                if !s.is_empty() {
                    parts.push(format!("{key}={}", clip_cols(s, 30)));
                }
            }
        }
        // 文本类字段只报长度(内容本身不该在 trace 区刷屏)
        for key in ["text", "outer_html", "markdown", "content"] {
            if let Some(s) = d.get(key).and_then(|x| x.as_str()) {
                let n = s.chars().count();
                if n > 0 {
                    parts.push(format!("{key}_len={n}"));
                }
            }
        }
        // 第 103 轮:浏览器任务核心 data 字段摘要(消除 TUI trace 区信息黑洞)
        // dom 返回: node_count / truncated
        if let Some(n) = d.get("node_count").and_then(|x| x.as_i64()) {
            parts.push(format!("nodes={n}"));
        }
        // elements 返回: count(匹配元素数) / tag(首元素标签)
        if let Some(n) = d.get("count").and_then(|x| x.as_i64()) {
            parts.push(format!("count={n}"));
        }
        if let Some(s) = d.get("tag").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
            parts.push(format!("tag={s}"));
        }
        // console/network 返回: count(事件数)
        // (console 和 network 的 count 通过 events 数组长度体现)
        // blockers 返回: blocked + 命中 kind 列表
        if let Some(true) = d.get("blocked").and_then(|x| x.as_bool()) {
            if let Some(arr) = d.get("blockers").and_then(|x| x.as_array()) {
                let kinds: Vec<&str> = arr.iter()
                    .filter_map(|b| b.get("kind").and_then(|k| k.as_str()))
                    .collect();
                if !kinds.is_empty() {
                    parts.push(format!("blocked=[{}]", kinds.join(",")));
                }
            }
        }
        // page_meta/url/title: 已在顶层处理,此处跳过避免重复
        // ocr 返回: ocr_text 已在文本类字段处理为 ocr_len
        // ===== 第 99 轮扩展 =====
        if d.get("reused").and_then(|x| x.as_bool()) == Some(true) {
            parts.push("reused".to_string());
        }
        if let Some(n) = d.get("closed").and_then(|x| x.as_i64()) {
            parts.push(format!("closed={n}"));
        }
        if let Some(s) = d.get("save_path").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
            let name = std::path::Path::new(s)
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or(s);
            parts.push(format!("out={}", clip_cols(name, 24)));
        }
        if let Some(n) = d.get("byte_size").and_then(|x| x.as_i64()) {
            parts.push(format!("bytes={n}"));
        }
        if let Some(s) = d.get("ocr_text").and_then(|x| x.as_str()) {
            let n = s.chars().count();
            if n > 0 {
                // 第 106 轮:验证码等短文本直接显示内容(≤20字符),长文本只显示长度
                if n <= 20 {
                    parts.push(format!("ocr={s}"));
                } else {
                    parts.push(format!("ocr_len={n}"));
                }
            }
        }
        if let Some(s) = d.get("ocr_error").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
            parts.push(format!("ocr_err={}", clip_cols(s, 30)));
        }
        match d.get("result") {
            Some(serde_json::Value::String(s)) => {
                // 第 106 轮:eval_js 结果显示前 40 字符(足够看到关键返回值)
                parts.push(format!("result={}", clip_cols(s, 40)));
            }
            Some(other @ (serde_json::Value::Number(_) | serde_json::Value::Bool(_))) => {
                parts.push(format!("result={other}"));
            }
            _ => {}
        }
        if d.get("result_truncated").and_then(|x| x.as_bool()) == Some(true) {
            let n = d.get("result_len").and_then(|x| x.as_i64()).unwrap_or(0);
            let saved = d.get("saved_to").and_then(|x| x.as_str())
                .map(|p| std::path::Path::new(p).file_name()
                    .and_then(|n| n.to_str()).unwrap_or(p))
                .unwrap_or("");
            if saved.is_empty() {
                parts.push(format!("result=[已落盘 共{n}字符]"));
            } else {
                parts.push(format!("result=[已落盘→{saved}]"));
            }
        }
        // 第 106 轮:elements 首个元素 text 预览(快速看到按钮/链接文字)
        if let Some(s) = d.get("text").and_then(|x| x.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                parts.push(format!("el_text={}", clip_cols(t, 30)));
            }
        }
    }
    Some(parts.join(" "))
}

/// MCP_Window_Use 成功输出的关键字段摘要(第 88 轮;第 90 轮 +input_batch)。
///
/// 原始 output_summary 是 pretty JSON,截 80 字只看得到 `"ok": true` 前缀噪声。
/// 这里按 action 产物提取排查最关心的字段(解析失败回退 None → 调用方截断):
/// - chat_send:`route / verified / frontmost_acquired`;
/// - chat_loop:`rounds / sent / replies / focus_aborted`;
/// - input_batch(第 90 轮):`steps_total / steps_ok`;
/// - run_sequence(第 91 轮):`steps_total / steps_ok` + `focus_lost_count / retried_steps / focus_aborted`;
/// - osascript_run:`exit_code / stderr 首行`;
/// - ocr:`block_count`;open/find:`window_id`;任意失败:`error`。
pub(crate) fn window_use_output_brief(_action: &str, output_summary: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(output_summary.trim()).ok()?;
    if !v.is_object() {
        return None;
    }
    let get_s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    let get_b = |k: &str| v.get(k).and_then(|x| x.as_bool());
    let get_n = |k: &str| v.get(k).and_then(|x| x.as_i64());
    let mut parts: Vec<String> = Vec::new();
    // 第 100 轮:Doom Loop 触发时(返回 ok=false + doom_loop=true),醒目标记。
    if get_b("doom_loop") == Some(true) {
        let n = get_n("doom_loop_count").unwrap_or(0);
        parts.push(format!("⚠doom_loop={n}"));
    }
    // 第 100 轮:explore 摘要 —— snapshot_id / 路径 / actionable 数 / 能力路线。
    // 第 101 轮:self_drawn 场景用 ⚠️ 醒目标记 + 推荐路线(放在 actionable 之前,
    // 避免 route 被 80 字符截断,route 是自绘 UI 场景最关键的排查字段)。
    if let Some(sid) = Some(get_s("snapshot_id")).filter(|s| !s.is_empty()) {
        parts.push(format!("snap={sid}"));
        // 第 101 轮:route 放在最前面(紧接 snap),确保自绘 UI 场景可见
        // route 截断至 12 字符,避免总输出超 80 字符被截断
        if let Some(route) = v.get("recommended_route").and_then(|x| x.as_str()) {
            if !route.is_empty() && route != "none" {
                parts.push(format!("route={}", clip_cols(route, 12)));
            }
        }
        if let Some(sp) = Some(get_s("snapshot_path")).filter(|s| !s.is_empty()) {
            let name = std::path::Path::new(sp)
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or(sp);
            // filename 截断至 16 字符,避免总输出超 80 字符
            parts.push(format!("snapshot={}", clip_cols(name, 16)));
        }
        // actionable_count 从 tree_summary 提取(explore 返回结构)
        let actionable_count = v.get("tree_summary")
            .and_then(|x| x.as_object())
            .and_then(|sd| sd.get("actionable_count"))
            .and_then(|x| x.as_i64());
        if let Some(n) = actionable_count {
            parts.push(format!("actionable={n}"));
        }
        if let Some(sd) = v.get("tree_summary").and_then(|x| x.as_object()) {
            if sd.get("self_drawn").and_then(|x| x.as_bool()) == Some(true) {
                // 第 101 轮:自绘 UI 醒目标记,帮助快速识别降级场景
                parts.push("⚠自绘UI".into());
            }
            if sd.get("ocr_used").and_then(|x| x.as_bool()) == Some(true) {
                parts.push("ocr_fallback".into());
            }
            // 第 109 轮:web 内容正向信号(0 = Electron 树未长出)+ 建树等待时长
            match sd.get("web_content").and_then(|x| x.as_bool()) {
                Some(false) => parts.push("web=0".into()),
                Some(true) => parts.push("web=1".into()),
                None => {}
            }
            if let Some(w) = sd.get("warmup_ms").and_then(|x| x.as_i64()) {
                if w > 0 {
                    parts.push(format!("warmup={w}ms"));
                }
            }
        }
    }
    // 第 109 轮:read_text 摘要 —— 路线 / 字符数 / 关键词命中
    if get_s("action") == "read_text" || v.get("chars").is_some() {
        let strategy = get_s("strategy");
        if !strategy.is_empty() {
            parts.push(format!("strategy={}", clip_cols(strategy, 10)));
        }
        if let Some(n) = get_n("chars") {
            parts.push(format!("chars={n}"));
        }
        match get_b("contains_hit") {
            Some(true) => parts.push("contains=✓".into()),
            Some(false) => parts.push("contains=✗".into()),
            None => {}
        }
    }
    if let Some(route) = Some(get_s("route")).filter(|s| !s.is_empty()) {
        parts.push(format!("route={route}"));
        if let Some(vf) = get_b("verified") {
            parts.push(format!("verified={vf}"));
        }
        if let Some(fm) = get_b("frontmost_acquired") {
            parts.push(format!("frontmost={}", if fm { "✓" } else { "✗" }));
        }
    }
    // 第 90 轮:input_batch 批处理摘要(总步数 / 成功步数)。
    if let Some(total) = get_n("steps_total") {
        let ok = get_n("steps_ok").unwrap_or(0);
        parts.push(format!("steps={ok}/{total}"));
    }
    // 第 91 轮:run_sequence 连续工作模式摘要(焦点丢失 / 重试 / 止损)。
    if let Some(n) = get_n("focus_lost_count") {
        if n > 0 {
            parts.push(format!("focus_lost={n}"));
        }
    }
    if let Some(n) = get_n("retried_steps") {
        if n > 0 {
            parts.push(format!("retries={n}"));
        }
    }
    // 第 93 轮:纯 wait 批次(零 UI 动作)醒目标记 —— 纯等待不是完成证据。
    if get_b("wait_only") == Some(true) {
        parts.push("⚠wait_only".to_string());
    }
    if get_b("focus_aborted") == Some(true) && get_n("total_sent").is_none() {
        parts.push("focus_aborted!".to_string());
    }
    if let Some(sent) = get_n("total_sent") {
        parts.push(format!(
            "rounds={} sent={sent} replies={}",
            get_n("total_rounds").unwrap_or(0),
            get_n("total_replies").unwrap_or(0)
        ));
        if get_b("focus_aborted") == Some(true) {
            parts.push("focus_aborted!".to_string());
        }
    }
    if let Some(code) = get_n("exit_code") {
        parts.push(format!("exit={code}"));
        if code != 0 {
            let err_first = get_s("stderr").lines().next().unwrap_or("");
            if !err_first.is_empty() {
                parts.push(format!("err=\"{}\"", clip_cols(err_first, 36)));
            }
        }
    }
    if let Some(n) = get_n("block_count") {
        parts.push(format!("blocks={n}"));
    }
    let wid = get_s("window_id");
    if !wid.is_empty() && parts.is_empty() {
        parts.push(format!("wid={wid}"));
    }
    if let Some(err) = Some(get_s("error")).filter(|s| !s.is_empty()) {
        parts.push(format!("err=\"{}\"", clip_cols(err, 40)));
    }
    if parts.is_empty() {
        return None;
    }
    Some(clip_cols(&parts.join(" "), 80))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_chat_send_route_fields() {
        let out = r#"{
            "ok": true,
            "window_id": "44978:0",
            "route": "osascript_fallback",
            "verified": false,
            "frontmost_acquired": true,
            "chat_log_path": "/tmp/x.log"
        }"#;
        let b = window_use_output_brief("", out).expect("应能解析");
        assert!(b.contains("route=osascript_fallback"), "{b}");
        assert!(b.contains("verified=false"), "{b}");
        assert!(b.contains("frontmost=✓"), "{b}");
    }

    #[test]
    fn brief_chat_loop_rounds_and_focus_abort() {
        let out = r#"{"ok": true, "total_rounds": 5, "total_sent": 4, "total_replies": 1, "focus_aborted": true}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("rounds=5 sent=4 replies=1"), "{b}");
        assert!(b.contains("focus_aborted!"), "{b}");
    }

    #[test]
    fn brief_osascript_run_exit_code_with_stderr() {
        let out = r#"{"ok": false, "exit_code": 1, "stderr": "17:18: syntax error (-2741)\nmore"}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("exit=1"), "{b}");
        assert!(b.contains("syntax error"), "{b}");
        // 成功时不带 err
        let ok = window_use_output_brief("", r#"{"ok": true, "exit_code": 0}"#).unwrap();
        assert_eq!(ok, "exit=0");
    }

    // ===== 第 109 轮:read_text / explore web 内容摘要 =====

    #[test]
    fn brief_read_text_strategy_chars_contains() {
        let out = r#"{"ok": true, "action": "read_text", "strategy": "clipboard", "chars": 1520, "contains_hit": true}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("strategy=clipboard"), "{b}");
        assert!(b.contains("chars=1520"), "{b}");
        assert!(b.contains("contains=✓"), "{b}");
        // 未命中
        let miss = r#"{"ok": false, "action": "read_text", "strategy": "clipboard", "chars": 0, "contains_hit": false}"#;
        let b2 = window_use_output_brief("", miss).unwrap();
        assert!(b2.contains("contains=✗"), "{b2}");
    }

    #[test]
    fn brief_explore_web_content_and_warmup() {
        let out = r#"{"ok": true, "snapshot_id": "ab12cd", "tree_summary": {"actionable_count": 0, "self_drawn": true, "web_content": false, "warmup_ms": 4800}}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("web=0"), "{b}");
        assert!(b.contains("warmup=4800ms"), "{b}");
        assert!(b.contains("⚠自绘UI"), "{b}");
        // web 内容已长出
        let ok = r#"{"ok": true, "snapshot_id": "ab12cd", "tree_summary": {"actionable_count": 12, "web_content": true}}"#;
        let b2 = window_use_output_brief("", ok).unwrap();
        assert!(b2.contains("web=1"), "{b2}");
    }

    #[test]
    fn brief_fallback_for_unparseable_or_empty() {
        assert!(window_use_output_brief("", "not json").is_none());
        assert!(window_use_output_brief("", "[]").is_none());
        // 纯 ok:true 无任何关键字段 → None(调用方回退截断)
        assert!(window_use_output_brief("", r#"{"ok": true}"#).is_none());
        // ocr blocks
        let b = window_use_output_brief("", r#"{"block_count": 42}"#).unwrap();
        assert_eq!(b, "blocks=42");
    }

    #[test]
    fn brief_input_batch_round90() {
        // 第 90 轮:input_batch 摘要 steps=成功/总数。
        let out = r#"{"ok": true, "action": "input_batch", "steps_total": 5, "steps_ok": 5}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("steps=5/5"), "{b}");
        // 部分失败
        let out = r#"{"ok": false, "action": "input_batch", "steps_total": 4, "steps_ok": 2}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("steps=2/4"), "{b}");
    }

    #[test]
    fn brief_run_sequence_round91() {
        // 第 91 轮:run_sequence 摘要 steps + focus_lost + retries。
        let out = r#"{"ok": true, "action": "run_sequence", "steps_total": 12, "steps_ok": 12, "focus_lost_count": 1, "retried_steps": 2, "focus_aborted": false}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("steps=12/12"), "{b}");
        assert!(b.contains("focus_lost=1"), "{b}");
        assert!(b.contains("retries=2"), "{b}");
        assert!(!b.contains("focus_aborted"), "{b}");
        // 止损中止
        let out = r#"{"ok": false, "action": "run_sequence", "steps_total": 7, "steps_ok": 4, "focus_lost_count": 3, "retried_steps": 0, "focus_aborted": true}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("steps=4/7"), "{b}");
        assert!(b.contains("focus_aborted!"), "{b}");
    }

    #[test]
    fn brief_run_sequence_wait_only_round93() {
        // 第 93 轮:纯 wait 批次(零 UI 动作)摘要带 ⚠wait_only 警示。
        let out = r#"{"ok": true, "action": "run_sequence", "steps_total": 18, "steps_ok": 16, "wait_only": true, "focus_lost_count": 0, "retried_steps": 2, "focus_aborted": false}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("steps=16/18"), "{b}");
        assert!(b.contains("⚠wait_only"), "{b}");
        // 有 UI 动作的批次不带警示
        let out = r#"{"ok": true, "action": "run_sequence", "steps_total": 6, "steps_ok": 6, "wait_only": false}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(!b.contains("wait_only"), "{b}");
    }

    #[test]
    fn brief_explore_round100() {
        // 第 100 轮:explore 摘要含 snapshot 标记 / actionable 数 / 自绘 UI 警示。
        // 第 101 轮:自绘 UI 标记改为 ⚠自绘UI,新增 recommended_route 展示。
        // 注意:actionable_count 在 tree_summary 内部,format_brief 从 tree_summary 提取。
        let out = r#"{
            "ok": true,
            "action": "explore",
            "snapshot_id": "a1b2c3",
            "snapshot_path": "/Users/x/ll/laew_ui_snapshot_1700000000.json",
            "window_info": {"id": "w"},
            "recommended_route": "osascript_fallback",
            "tree_summary": {"total_nodes": 87, "actionable_count": 12, "self_drawn": false, "ocr_used": false}
        }"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("snap=a1b2c3"), "{b}");
        // route 截断至 12 字符(osascript_f…),放在 snap 之后确保可见
        assert!(b.contains("route=osascript_f"), "{b}");
        // 文件名截断至 16 字符(laew_ui_snapsho…)
        assert!(b.contains("snapshot=laew_ui_snapsho"), "{b}");
        // actionable_count 从 tree_summary 提取
        assert!(b.contains("actionable=12"), "{b}");
        assert!(!b.contains("⚠自绘UI"), "非自绘 UI 不应带 ⚠自绘UI: {b}");
        // 自绘 UI + OCR 兜底(第 101 轮:标记改为 ⚠自绘UI)
        let out = r#"{
            "ok": true,
            "action": "explore",
            "snapshot_id": "zzz",
            "snapshot_path": "/tmp/x.json",
            "tree_summary": {"actionable_count": 0, "self_drawn": true, "ocr_used": true}
        }"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("⚠自绘UI"), "{b}");
        assert!(b.contains("ocr_fallback"), "{b}");
    }

    #[test]
    fn brief_doom_loop_round100() {
        // 第 100 轮:Doom Loop 触发时醒目标记。
        let out = r#"{
            "ok": false,
            "action": "control",
            "doom_loop": true,
            "doom_loop_count": 3,
            "error": "Doom Loop..."
        }"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("⚠doom_loop=3"), "{b}");
        assert!(b.contains("err=\"Doom Loop"), "{b}");
    }
}

#[cfg(test)]
mod browser_output_brief_tests {
    use super::*;

    #[test]
    fn brief_reused_and_page() {
        let out = r#"{"code":0,"message":"ok","data":{"page_id":"p_a1b2c3d4","reused":true,"final_url":"http://10.0.0.1:20122/"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("reused"), "{b}");
        assert!(b.contains("page_id=p_a1b2c3d4"), "{b}");
    }

    #[test]
    fn brief_screenshot_save_path_and_bytes() {
        let out = r#"{"code":0,"message":"ok","data":{"save_path":"/tmp/laew/captcha.png","byte_size":104527,"format":"png"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("out=captcha.png"), "{b}");
        assert!(b.contains("bytes=104527"), "{b}");
    }

    #[test]
    fn brief_ocr_short_text_shows_content() {
        // 第 106 轮:短文本(≤20字符)直接显示内容,方便看验证码
        let out = r#"{"code":0,"message":"ok","data":{"ocr_text":"4a7c","ocr_block_count":4,"save_path":"/tmp/x.png"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("ocr=4a7c"), "短验证码应直接显示内容: {b}");
    }

    #[test]
    fn brief_ocr_long_text_shows_len_only() {
        // 长文本只报长度
        let long_text = "a".repeat(100);
        let out = format!(r#"{{"code":0,"message":"ok","data":{{"ocr_text":"{long_text}","ocr_block_count":4,"save_path":"/tmp/x.png"}}}}"#);
        let b = browser_output_brief(&out).unwrap();
        assert!(b.contains("ocr_len=100"), "长文本只报长度: {b}");
        assert!(!b.contains(&long_text), "长文本内容不应进摘要");
    }

    #[test]
    fn brief_saved_to_filename() {
        let out = r#"{"code":0,"message":"ok","data":{"result_truncated":true,"result_len":140000,"saved_to":"/tmp/laew_web_result_12345.captcha.png"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("captcha.png"), "应显示落盘文件名: {b}");
    }

    #[test]
    fn brief_elements_text_preview() {
        let out = r#"{"code":0,"message":"ok","data":{"tag":"button","count":5,"text":"  登录  "}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("el_text=登录"), "应显示元素文字: {b}");
    }

    #[test]
    fn brief_eval_js_result_head_and_truncated() {
        let out = r#"{"code":0,"message":"ok","data":{"result":"https://example.com/page"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("result=https://example.com/p"), "应显示 result 前40字符: {b}");
        // 超长落盘(第 106 轮:格式改为 result=[已落盘→文件名])
        let out = r#"{"code":0,"message":"ok","data":{"result_truncated":true,"result_len":140000,"saved_to":"/tmp/laew_web_result_1.png"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("result=[已落盘"), "应显示已落盘: {b}");
        assert!(b.contains("laew_web_result_1.png"), "应显示文件名: {b}");
    }

    #[test]
    fn brief_close_all_count() {
        let out = r#"{"code":0,"message":"closed","data":{"page_id":"all","closed":3}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("closed=3"), "{b}");
    }

    #[test]
    fn brief_data_envelope_still_works() {
        // 存量行为回归:文本类字段只报长度
        let out = r#"{"code":0,"message":"ok","data":{"page_id":"p_1","text":"一二三四五"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("text_len=5"), "{b}");
    }
}
