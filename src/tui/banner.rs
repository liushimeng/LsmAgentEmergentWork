//! 启动横幅(TUI 首屏信息盒)—— 声明式行集 + [`textfit::InfoBox`] 自适应渲染。
//!
//! 改造前(`src/tui/mod.rs::print_banner` 原实现):边框写死 `58` 内宽,每行内容却按
//! `45` / `46` 独立填充,`标签宽 + 预算` 凑不齐 55 的行短 1 列、被截断的行多 1 列,
//! 右边框参差;盒宽与终端宽度无关,100 列终端上照样砍掉 endpoint 与长路径尾部。
//! 现在行只声明「标签 + 值」,宽度由 `InfoBox` 按 `min(内容自然宽, 终端可用宽)` 统一算:
//! 宽终端一行看全,窄终端折行续排,任何情况下所有行严格等宽。
//!
//! 设计见 `tmpPlan/2026-09-24_02-TUI显示信息完整化与宽度自适应方案.md`。

use std::sync::Arc;

use crate::agent::offline_queue::OfflineQueue;
use crate::agent::workspace::WorkspaceSnapshot;
use crate::database::paths::Paths;
use crate::llm::{Connectivity, ConnectivityTracker};
use crate::tui::pathfmt;
use crate::tui::textfit::{BoxStyle, InfoBox};

/// 横幅一行的纯文本载荷(状态采集与排版解耦,渲染可单测)。
#[derive(Debug, Clone, Default)]
pub struct BannerData {
    /// 版本号(`CARGO_PKG_VERSION`)
    pub version: String,
    /// 编译时间(`LAEW_BUILD_TIME`)
    pub build_time: String,
    /// 启动时刻(已转 `YYYY-MM-DD HH:MM:SS`)
    pub startup_time: String,
    /// 根目录(二进制所在目录)
    pub root_dir: String,
    /// 工作目录(启动命令所在目录)
    pub work_dir: String,
    /// 项目说明文件探测结果
    pub project_doc: String,
    /// 工作区状态行
    pub workspace: String,
    /// 当前接入记录(`[协议] provider/model @ endpoint`)
    pub model: String,
    /// ⚠ 私网 endpoint 拦截提示(`Some` 时追加提示行 + 解锁指引行)
    pub private_endpoint: Option<String>,
    /// 与上者配套的解锁命令
    pub private_endpoint_unlock: Option<String>,
    /// Session ID
    pub session_id: String,
    /// 当前主题 + 切换指引
    pub theme: String,
    /// 连接状态
    pub connectivity: String,
    /// TODO 清单摘要(`Some` 且非空时显示)
    pub todo_summary: Option<String>,
    /// 运行日志文件(`--debug` / `--info` 时显示)
    pub log_file: Option<String>,
}

/// 渲染横幅为等宽行集合(不含 `\n`)。
pub fn render(data: &BannerData, indent: usize, term_w: usize) -> Vec<String> {
    let mut b = InfoBox::new(BoxStyle::Double)
        .span(format!(
            "LsmAgentEmergentWork  ·  laew  TUI  ·  v{}",
            data.version
        ))
        .kv("编译时间", data.build_time.clone())
        .kv("启动时间", data.startup_time.clone())
        .sep()
        // 路径类:拆开就不可复制 → 单行 + 中间省略保尾
        .kv_keep_tail("根目录", data.root_dir.clone())
        .kv_keep_tail("工作目录", data.work_dir.clone())
        .kv("项目说明", data.project_doc.clone())
        .kv("工作区", data.workspace.clone())
        // 模型行是最长也最不能砍的一行(endpoint 决定连的是谁)→ 折行保完整
        .kv("当前模型", data.model.clone());
    if let Some(warn) = &data.private_endpoint {
        b = b.kv("⚠ 私网", warn.clone());
    }
    if let Some(unlock) = &data.private_endpoint_unlock {
        b = b.kv("解 锁", unlock.clone());
    }
    b = b
        .kv("Session", data.session_id.clone())
        .kv("主题", data.theme.clone())
        .kv("连接", data.connectivity.clone());
    if let Some(todo) = data.todo_summary.as_deref().filter(|s| !s.is_empty()) {
        b = b.kv("任务", todo.to_string());
    }
    if let Some(log) = &data.log_file {
        b = b.kv("日志文件", log.clone());
    }
    b.render(indent, term_w)
}

/// 横幅之后的操作指引(不进盒,纯文本两行)。
pub fn footer_lines() -> &'static [&'static str] {
    &[
        "  输入提示词开始对话, 输入 / 查看可用命令。",
        "  快捷键: ↑↓ 选择补全  Enter 提交  Esc 关闭补全  Ctrl-D 退出  Ctrl-C 清行/退出",
    ]
}

/// D4 工作区感知:工作区状态行文本(形如 `[Rust] git:main · 3 未提交`)。
pub fn workspace_status_line(ws: &WorkspaceSnapshot) -> String {
    if ws.is_trivial() {
        return "空目录(不注入环境信息)".to_string();
    }
    let mut s = format!("[{}]", ws.project_label());
    if ws.is_git {
        let branch = ws.branch.as_deref().unwrap_or("?");
        let dirty_text = if ws.dirty() == 0 {
            "干净".to_string()
        } else {
            format!("{} 未提交", ws.dirty())
        };
        s.push_str(&format!(" git:{branch} · {dirty_text}"));
    } else {
        s.push_str(" 非 git");
    }
    s
}

/// 第 72 轮:「日志文件」行内容(路径相对化 + 级别后缀,纯函数便于测试)。
pub fn log_file_line(paths: &Paths, log: &crate::logging::AgentLogInfo) -> String {
    format!(
        "{} (级别: {})",
        pathfmt::display_path(paths, &log.path),
        log.level
    )
}

/// D13 离线模式:连接状态行文本。
pub fn connectivity_status_line(
    connectivity: &Arc<ConnectivityTracker>,
    queue: &OfflineQueue,
) -> String {
    let snap = connectivity.snapshot();
    match snap.state {
        Connectivity::Online => {
            if queue.is_empty() {
                "Online ✓".to_string()
            } else {
                format!("Online ✓ (队列残留 {} 条)", queue.len())
            }
        }
        Connectivity::Degraded => {
            let kind = snap
                .last_network_error_kind
                .as_deref()
                .unwrap_or("网络不稳");
            format!(
                "Degraded ⚠ ({kind}, {} 次)",
                snap.consecutive_network_errors
            )
        }
        Connectivity::Offline => {
            let queued = queue.len();
            if queued > 0 {
                format!("Offline ✗ (已排队 {queued} 条)")
            } else {
                "Offline ✗".to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::textfit;

    fn sample() -> BannerData {
        BannerData {
            version: "0.1.0".into(),
            build_time: "2026-09-24 12:35:08".into(),
            startup_time: "2026-09-24 12:35:08".into(),
            root_dir: "D:/MyLocalGit/LsmAgentEmergentWork".into(),
            work_dir: "D:/MyLocalGit/LsmAgentEmergentWork/TestWorkSpace".into(),
            project_doc: "未找到".into(),
            workspace: "[通用] 非 git".into(),
            model: "[anthropic] liusm191-laew-model/liusm191-laew-model @ https://gw.example.com/v1/messages".into(),
            private_endpoint: None,
            private_endpoint_unlock: None,
            session_id: "20260924-123508-35d8407d-1790224508748-6f5ddf".into(),
            theme: "default  (切换 /theme [kind])".into(),
            connectivity: "Online ✓".into(),
            todo_summary: None,
            log_file: Some("llaew_20260924_123508.log (级别: DEBUG)".into()),
        }
    }

    #[test]
    fn 所有行严格等宽_且不溢出终端() {
        for term_w in [40usize, 50, 60, 70, 80, 100, 120, 160, 200] {
            let lines = render(&sample(), 0, term_w);
            let w0 = textfit::width(&lines[0]);
            for l in &lines {
                assert_eq!(
                    textfit::width(l),
                    w0,
                    "终端 {term_w} 列右边框参差:\n{}",
                    lines.join("\n")
                );
                assert!(w0 <= term_w - 1, "盒宽 {w0} 溢出终端 {term_w}");
            }
            assert!(lines[0].starts_with("╔") && lines[0].ends_with("╗"));
            assert!(lines[lines.len() - 1].starts_with("╚"));
        }
    }

    #[test]
    fn 宽终端信息完整无省略号() {
        let lines = render(&sample(), 0, 200).join("\n");
        assert!(!lines.contains('…'), "宽终端不该再砍信息:\n{lines}");
        assert!(lines.contains("https://gw.example.com/v1/messages"), "endpoint 被砍");
        assert!(lines.contains("20260924-123508-35d8407d-1790224508748-6f5ddf"));
    }

    #[test]
    fn 窄终端折行而非砍尾_且一个字符都不丢() {
        let d = sample();
        let lines = render(&d, 0, 60);
        // 值宽 92 > 60 列预算 → 必然折成多行;收集到 Session 行为止的全部续行
        let first = lines.iter().position(|l| l.contains("当前模型")).expect("模型行存在");
        let end =
            first + 1 + lines[first + 1..].iter().position(|l| l.contains("Session:")).unwrap();
        assert!(end - first >= 2, "模型行未折行续排:\n{}", lines.join("\n"));
        assert!(
            lines[first + 1].starts_with("║           "),
            "续行未缩进到值列(应与首行值同列):\n{}",
            lines.join("\n")
        );
        // 折行零信息损失:各行去边框与空白后拼接 == 标签 + 原值
        let strip = |l: &str| -> String {
            l.trim_start_matches(['╔', '║', '╠'])
                .trim_end_matches(['╗', '║', '╣'])
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };
        let rebuilt: String = lines[first..end].iter().map(|l| strip(l)).collect();
        let want: String = "当前模型:"
            .chars()
            .chain(d.model.chars().filter(|c| !c.is_whitespace()))
            .collect();
        assert_eq!(rebuilt, want, "折行丢了字符:\n{}", lines[first..end].join("\n"));
    }

    #[test]
    fn 值列左边界全盒对齐() {
        // 视觉不变量:所有 kv 行的值从同一显示列开始(标签左侧对齐 + 冒号后补空格到定宽)
        let lines = render(&sample(), 0, 120);
        let value_col = |key: &str| -> usize {
            let l = lines.iter().find(|l| l.contains(key)).expect("行存在");
            let i = l.find(':').expect("标签冒号");
            let after = &l[i + 1..];
            let lead = after.len() - after.trim_start().len();
            textfit::width(&l[..i + 1]) + lead
        };
        let cols: Vec<usize> = ["根目录:", "工作目录:", "项目说明:", "工作区:", "Session:", "连接:"]
            .iter()
            .map(|k| value_col(k))
            .collect();
        assert!(cols.windows(2).all(|w| w[0] == w[1]), "值列不齐: {cols:?}\n{}", lines.join("\n"));
    }

    #[test]
    fn 私网提示行按需出现() {
        let mut d = sample();
        let base = render(&d, 0, 120).len();
        d.private_endpoint = Some("http://127.0.0.1:8000  ← SSRF 拦截将在任务时触发".into());
        d.private_endpoint_unlock = Some("laew provider allow-private 7  (然后重启 laew)".into());
        let with = render(&d, 0, 120);
        assert_eq!(with.len(), base + 2);
        assert!(with.iter().any(|l| l.contains("⚠ 私网")));
        let w0 = textfit::width(&with[0]);
        assert!(with.iter().all(|l| textfit::width(l) == w0));
    }

    #[test]
    fn 可选行缺省时整幅收窄但仍等宽() {
        let mut d = sample();
        d.log_file = None;
        d.todo_summary = Some("[✓]3 [→]1 [○]2 · last \"写 OCR 模块\"".into());
        let lines = render(&d, 0, 120);
        assert!(!lines.iter().any(|l| l.contains("日志文件")));
        assert!(lines.iter().any(|l| l.contains("任务")));
        let w0 = textfit::width(&lines[0]);
        assert!(lines.iter().all(|l| textfit::width(l) == w0));
    }

    #[test]
    fn 路径行单行中间省略保尾() {
        // 路径折成多行就不可复制了 → KvKeepTail:60 列下中间省略,尾部目录名保住
        let lines = render(&sample(), 0, 60);
        let work = lines.iter().find(|l| l.contains("工作目录:")).expect("行存在");
        assert!(work.contains('…'), "超长路径应中间省略: {work}");
        assert!(work.ends_with("TestWorkSpace ║"), "路径尾部被砍(应保末段): {work}");
    }

    #[test]
    fn 日志文件行_工作目录内相对化并带级别() {
        let p = Paths {
            root_dir: std::path::PathBuf::from("/opt/laew"),
            work_dir: std::path::PathBuf::from("/home/u/work"),
            db_path: std::path::PathBuf::from("/tmp/x.db"),
        };
        let info = crate::logging::AgentLogInfo {
            path: std::path::PathBuf::from("/home/u/work/llaew_20260917_150412.log"),
            level: "DEBUG",
        };
        assert_eq!(log_file_line(&p, &info), "llaew_20260917_150412.log (级别: DEBUG)");
    }

    #[test]
    fn 日志文件行_目录外回退绝对路径() {
        let p = Paths {
            root_dir: std::path::PathBuf::from("/opt/laew"),
            work_dir: std::path::PathBuf::from("/home/u/work"),
            db_path: std::path::PathBuf::from("/tmp/x.db"),
        };
        let info = crate::logging::AgentLogInfo {
            path: std::path::PathBuf::from("/var/tmp/llaew_20260917_150412.log"),
            level: "INFO",
        };
        assert_eq!(log_file_line(&p, &info), "/var/tmp/llaew_20260917_150412.log (级别: INFO)");
    }
}
