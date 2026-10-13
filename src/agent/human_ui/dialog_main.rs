//! `laew __hitl-dialog <payload.json>` 原生弹窗子进程入口(第 137 轮)。
//!
//! 职责:读 payload → 分发平台原生弹窗(macOS AppKit FFI / Windows Win32)→
//! 把结果按契约(03 文档 §5)打印到 stdout 最后一行 JSON 后退出。
//! 由 `main.rs` 的隐藏子命令分发进入,**最小启动路径**:不打开数据库、不初始化
//! tracing/Agent(与 `__browser-watchdog` 同构)。

use std::path::Path;

/// 弹窗侧实际需要的 payload 投影(容错反序列化:字段缺失/类型不符一律回退默认,
/// 绝不因 payload 瑕疵 panic —— 失败语义统一走 `error` 结果 JSON)。
pub(super) struct DialogPayload {
    /// 请求 id(第 145 轮):「⏱ +2 分钟」延长中间行要携带它,父进程据此对号入座
    /// 到 hub 当前槽位。
    pub id: u64,
    pub kind_label: String,
    pub message: String,
    pub url: String,
    pub page_id: String,
    pub options: Vec<String>,
    /// 2026-10-13:选项是否多选(true = 复选框组,点「继续」收集勾选;false = 单选
    /// 按钮组,点击即应答)。
    pub multi_select: bool,
    /// 2026-10-13:页面当前是否已处于放行(open/无管控)态 —— 决定「解除屏蔽/恢复屏蔽」
    /// 切换按钮的初始标题(false=显示「🔓 解除屏蔽」,true=显示「🔒 恢复屏蔽」)。
    pub guard_unlocked: bool,
    pub timeout_ms: u64,
    pub started_at_ms: u64,
}

impl DialogPayload {
    fn from_json(v: &serde_json::Value) -> Self {
        let str_field = |key: &str| -> String {
            v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
        };
        let mut options: Vec<String> = v
            .get("options")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .take(6)
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if options.is_empty() {
            options.push("我已完成人工操作,继续".into());
        }
        let kind_label = {
            let label = str_field("kind_label");
            let kind = str_field("kind");
            if !label.is_empty() {
                label
            } else if !kind.is_empty() {
                kind
            } else {
                "人工介入".to_string()
            }
        };
        DialogPayload {
            id: v.get("id").and_then(|x| x.as_u64()).unwrap_or(0),
            kind_label,
            message: str_field("message"),
            url: str_field("url"),
            page_id: str_field("page_id"),
            options,
            multi_select: v
                .get("multi_select")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            // 2026-10-13:payload 中的 image_path 不再投影 —— 弹窗验证码图片功能
            // 已移除,未知字段按容错缺省忽略。
            guard_unlocked: v
                .get("guard_unlocked")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            timeout_ms: v
                .get("timeout_ms")
                .and_then(|x| x.as_u64())
                .filter(|ms| *ms > 0)
                .unwrap_or(120_000),
            started_at_ms: v.get("started_at_ms").and_then(|x| x.as_u64()).unwrap_or(0),
        }
    }

    /// 说明区正文:message + 页面 + 页面ID(与 TUI 蓝框信息面一致)。
    pub(super) fn info_text(&self) -> String {
        let mut info = self.message.trim_end().to_string();
        if !self.url.is_empty() {
            let url: String = self.url.chars().take(200).collect();
            info.push_str("\n\n页面: ");
            info.push_str(&url);
        }
        if !self.page_id.is_empty() {
            info.push_str("\n页面ID: ");
            info.push_str(&self.page_id);
        }
        info
    }

    /// 「📋 复制」写入剪贴板的完整文本(第 139 轮;macOS/Windows 共用)。
    ///
    /// 比 `info_text` 多带:类型标签、提出/截止时刻、候选选项列表 —— 人工要把
    /// 这些信息粘到工单/聊天窗口时,不必逐行从屏幕上捞(倒计时行每秒刷新,
    /// 拖选会被重绘冲掉)。
    pub(super) fn copy_text(&self) -> String {
        let mut s = format!("【人工介入 · {}】\n", self.kind_label);
        s.push_str(&self.info_text());
        s.push_str(&format!(
            "\n提出时间: {}   超时截止: {}",
            self.started_clock(),
            self.deadline_clock()
        ));
        if !self.options.is_empty() {
            s.push_str("\n候选选项:");
            for (i, o) in self.options.iter().enumerate() {
                s.push_str(&format!("\n{}. {}", i + 1, o));
            }
        }
        s
    }

    /// 提出时刻 `HH:MM:SS`(时间轴第一行;started_at_ms 未记录时由子进程自证)。
    pub(super) fn started_clock(&self) -> String {
        let ms = if self.started_at_ms > 0 {
            self.started_at_ms
        } else {
            now_unix_ms()
        };
        super::fmt_local_ms(ms).split(' ').nth(1).unwrap_or("").to_string()
    }

    /// 超时截止时刻 `HH:MM:SS`。
    pub(super) fn deadline_clock(&self) -> String {
        let ms = if self.started_at_ms > 0 {
            self.started_at_ms.saturating_add(self.timeout_ms)
        } else {
            now_unix_ms().saturating_add(self.timeout_ms)
        };
        super::fmt_local_ms(ms).split(' ').nth(1).unwrap_or("").to_string()
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 等待时长格式化:`X 分 YY 秒` / `Y 秒`(与 TUI/旧脚本口径一致)。
pub(super) fn fmt_dur(ms: u64) -> String {
    let s = ms / 1000;
    let (m, r) = (s / 60, s % 60);
    if m > 0 {
        format!("{m} 分 {r:02} 秒")
    } else {
        format!("{r} 秒")
    }
}

/// 倒计时行文案(两平台共用)。
pub(super) fn countdown_text(elapsed_ms: u64, remain_ms: u64) -> String {
    format!(
        "已等待: {}   超时剩余: {}",
        fmt_dur(elapsed_ms),
        fmt_dur(remain_ms)
    )
}

// ---------- 第 145 轮:「⏱ +2 分钟」延长等待(两平台共用) ----------

/// 每次延长按钮增加的等待毫秒数。
pub(super) const EXTEND_STEP_MS: u64 = 120_000;
/// 延长按钮标题(点击后标题追加 ×N 计数,见两平台实现)。
pub(super) const EXTEND_BTN_TITLE: &str = "⏱ +2分钟";
/// 总等待(原始 timeout_ms + 累计延长)封顶,与父进程
/// `human_assist::MAX_HUMAN_ASSIST_TIMEOUT_MS` 同源(同 binary,无版本漂移)。
pub(super) const MAX_TOTAL_WAIT_MS: u64 = crate::agent::human_assist::MAX_HUMAN_ASSIST_TIMEOUT_MS;

/// 延长中间行:`{"status":"extend","id":<请求id>,"ms":<实际延长毫秒>}`。
/// 子进程**不退出**,println! 即时flush(Stdout 行缓冲),父进程逐行读并接线
/// `HumanAssistHub::extend_timeout`。
pub(super) fn extend_line(id: u64, ms: u64) -> String {
    serde_json::json!({"status": "extend", "id": id, "ms": ms}).to_string()
}

// ---------- 2026-10-13:弹窗「解除屏蔽/恢复屏蔽」互切按钮(两平台共用) ----------

/// 管控切换按钮常态标题(页面处于 locked/partial 时):点击 → 整页 open 放行。
pub(super) const GUARD_BTN_UNLOCK_TITLE: &str = "🔓 解除屏蔽";
/// 管控切换按钮反态标题(页面已 open 时):点击 → 恢复本页解锁前管控快照。
pub(super) const GUARD_BTN_LOCK_TITLE: &str = "🔒 恢复屏蔽";
/// 管控切换中间行:`{"status":"guard","id":<请求id>,"unlock":<bool>}`。
/// 子进程**不退出**,父进程逐行读并接线 `HumanAssistHub::guard_toggle_from_ui`
/// → 对关联 page_id 应用整页 open(unlock=true)/恢复快照(unlock=false)。
pub(super) fn guard_line(id: u64, unlock: bool) -> String {
    serde_json::json!({"status": "guard", "id": id, "unlock": unlock}).to_string()
}

// ---------- 2026-10-13:多选应答编码(两平台共用) ----------

/// 把多选弹窗勾选的选项下标(0 基,升序)编码为应答文本:`1. aaa；3. ccc`
/// (全角分号连接)。空选 / 下标越界返回 None(调用方回退「选项 1 交棒」语义,
/// 与单选空输入点继续完全一致)。
///
/// 形状与单选 `N. label` 同构 —— `hitl_hint::is_option_pick_answer` 的数字前缀
/// 判定天然兼容,多选应答自动归入「选项应答」分流(不做填码处理)。
pub(super) fn join_multi_answer(options: &[String], picked: &[usize]) -> Option<String> {
    if picked.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::with_capacity(picked.len());
    for &i in picked {
        parts.push(format!("{}. {}", i + 1, options.get(i)?));
    }
    Some(parts.join("；"))
}

/// 子进程主入口:返回进程退出码(0 = 正常收口,含 timeout/error JSON)。
pub fn run(payload_path: &Path) -> i32 {
    let (status, text) = match read_payload(payload_path) {
        Ok(v) => {
            let p = DialogPayload::from_json(&v);
            #[cfg(target_os = "macos")]
            {
                super::macos_dialog::run_dialog(&p)
            }
            #[cfg(target_os = "windows")]
            {
                super::windows_dialog::run_dialog(&p)
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                let _ = &p;
                ("error".to_string(), "当前平台不支持桌面弹窗".to_string())
            }
        }
        Err(e) => ("error".to_string(), e),
    };
    let mut obj = serde_json::Map::new();
    obj.insert("status".into(), serde_json::Value::String(status));
    if !text.is_empty() {
        obj.insert("text".into(), serde_json::Value::String(text));
    }
    println!("{}", serde_json::Value::Object(obj));
    0
}

fn read_payload(path: &Path) -> Result<serde_json::Value, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| format!("payload 读取失败 {}: {e}", path.display()))?;
    serde_json::from_str(&body).map_err(|e| format!("payload 解析失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_defaults_are_fail_open() {
        let p = DialogPayload::from_json(&serde_json::json!({}));
        assert_eq!(p.options, vec!["我已完成人工操作,继续".to_string()]);
        assert_eq!(p.timeout_ms, 120_000);
        assert_eq!(p.kind_label, "人工介入");
        // info_text 全空也不 panic
        assert!(p.info_text().is_empty());
    }

    #[test]
    fn payload_projection_keeps_fields() {
        let v = serde_json::json!({
            "id": 7, "kind": "sms", "kind_label": "短信验证码",
            "message": "请输入验证码", "options": ["继续", "取消任务"],
            "url": "https://example.com/login", "page_id": "p_1",
            "image_path": "/tmp/x.png", "timeout_ms": 300000,
            "multi_select": true,
            "started_at_ms": 1759991525000u64
        });
        let p = DialogPayload::from_json(&v);
        assert_eq!(p.id, 7);
        assert_eq!(p.kind_label, "短信验证码");
        assert_eq!(p.options.len(), 2);
        assert_eq!(p.timeout_ms, 300_000);
        assert!(p.multi_select);
        assert!(p.info_text().contains("页面: https://example.com/login"));
        assert!(p.info_text().contains("页面ID: p_1"));
        // 缺省投影:multi_select 缺失 = 单选
        let dft = DialogPayload::from_json(&serde_json::json!({}));
        assert!(!dft.multi_select);
    }

    /// 2026-10-13:多选应答编码形状(两平台收口共用)。
    #[test]
    fn join_multi_answer_shapes() {
        let opts = vec!["选项甲".to_string(), "选项乙".to_string(), "选项丙".to_string()];
        // 常规多选:全角分号连接,编号 1 基
        assert_eq!(join_multi_answer(&opts, &[0, 2]).as_deref(), Some("1. 选项甲；3. 选项丙"));
        // 单个勾选退化为单选形态
        assert_eq!(join_multi_answer(&opts, &[1]).as_deref(), Some("2. 选项乙"));
        // 空选 = None(调用方回退选项 1 交棒语义)
        assert_eq!(join_multi_answer(&opts, &[]), None);
        // 下标越界 = None(防御,不 panic)
        assert_eq!(join_multi_answer(&opts, &[9]), None);
    }

    /// 第 145 轮:延长中间行必须是父进程 `parse_extend_line` 认识的形状。
    #[test]
    fn extend_line_roundtrips_with_parent_parser() {
        let line = extend_line(42, 120_000);
        assert_eq!(
            crate::agent::human_ui::parse_extend_line(&line),
            Some((42, 120_000))
        );
        // 常量成对:按钮标题 / 步长 / 总上限
        assert_eq!(EXTEND_BTN_TITLE, "⏱ +2分钟");
        assert_eq!(EXTEND_STEP_MS, 120_000);
        assert_eq!(MAX_TOTAL_WAIT_MS, 1_800_000);
    }

    /// 2026-10-13:管控切换中间行与父进程 `parse_guard_line` 互为往返;
    /// 按钮两个标题互异且与语义对应。
    #[test]
    fn guard_line_roundtrips_with_parent_parser() {
        assert_eq!(
            crate::agent::human_ui::parse_guard_line(&guard_line(7, true)),
            Some((7, true))
        );
        assert_eq!(
            crate::agent::human_ui::parse_guard_line(&guard_line(7, false)),
            Some((7, false))
        );
        assert_ne!(GUARD_BTN_UNLOCK_TITLE, GUARD_BTN_LOCK_TITLE);
    }

    #[test]
    fn fmt_dur_shapes() {
        assert_eq!(fmt_dur(0), "0 秒");
        assert_eq!(fmt_dur(59_999), "59 秒");
        assert_eq!(fmt_dur(60_000), "1 分 00 秒");
        assert_eq!(fmt_dur(125_000), "2 分 05 秒");
    }
}
