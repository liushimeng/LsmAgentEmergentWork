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
    pub kind_label: String,
    pub message: String,
    pub url: String,
    pub page_id: String,
    pub options: Vec<String>,
    pub image_path: String,
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
            kind_label,
            message: str_field("message"),
            url: str_field("url"),
            page_id: str_field("page_id"),
            options,
            image_path: str_field("image_path"),
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
            "kind": "sms", "kind_label": "短信验证码",
            "message": "请输入验证码", "options": ["继续", "取消任务"],
            "url": "https://example.com/login", "page_id": "p_1",
            "image_path": "/tmp/x.png", "timeout_ms": 300000,
            "started_at_ms": 1759991525000u64
        });
        let p = DialogPayload::from_json(&v);
        assert_eq!(p.kind_label, "短信验证码");
        assert_eq!(p.options.len(), 2);
        assert_eq!(p.timeout_ms, 300_000);
        assert!(p.info_text().contains("页面: https://example.com/login"));
        assert!(p.info_text().contains("页面ID: p_1"));
    }

    #[test]
    fn fmt_dur_shapes() {
        assert_eq!(fmt_dur(0), "0 秒");
        assert_eq!(fmt_dur(59_999), "59 秒");
        assert_eq!(fmt_dur(60_000), "1 分 00 秒");
        assert_eq!(fmt_dur(125_000), "2 分 05 秒");
    }
}
