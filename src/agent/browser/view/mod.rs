//! 有头帧查看器(阶段三原生窗口的先行落地)。
//!
//! 架构与 `__hitl-dialog` 同一模式:内核进程**零 UI 依赖**(AppKit/Win32 不进
//! 引擎线程),软件渲染产物(RGBA→PNG)经 stdin 长度前缀帧协议推给独立查看器
//! 子进程 `laew __browser-view`,由子进程原生窗口显示。
//!
//! 帧协议(stdin,二进制,小端):
//! `[u32 总长][u8 kind][payload]`
//! kind `'i'` = PNG 帧;`'t'` = 标题 JSON({"title": "..."});`'c'` = 关闭;
//! `'u'` = 地址栏状态 JSON({"url","can_back","can_forward"});
//! `'d'` = DevTools 状态 JSON(58 号:console 台账快照等)。
//! 写失败即视为查看器退出,静默降级为纯无头(不影响任何工具行为)。
//!
//! 命令通道(stdout,查看器 → 宿主,按行 JSON):
//! `{"cmd":"navigate","url":"..."}` / `{"cmd":"back"}` / `{"cmd":"forward"}` /
//! `{"cmd":"reload"}` / `{"cmd":"home"}` / `{"cmd":"resize","w":1920,"h":1080}` /
//! `{"cmd":"input","act":"move|down|up|wheel|key",...}`(人类键鼠,39 号设计文档);
//! `{"cmd":"devtools","panel":"console|none|…"}`(58 号:一级菜单栏面板切换)/
//! `{"cmd":"console_eval","expr":"…"}`(58 号:Console 实时 JS 交互)。
//! 浏览器窗口的工具栏(后退/前进/刷新/首页)与地址栏都经这条通道驱动宿主引擎,
//! 窗口因此是**可交互的真浏览器形态**,而不只是被动显示渲染帧。
//! `resize` 是人工拖动窗口后查看器上报的新内容区尺寸(已扣工具栏),
//! 宿主据此重排重渲染 → 新帧按 1:1 铺满内容区(见 `view::resize`)。
//! `input` 是人工在页面区内的鼠标/键盘事件,宿主经 `human_input` 管线
//! 转成真实页面交互(控制权 `user` 态放行,`agent` 态忽略)。

#[cfg(target_os = "macos")]
pub mod macos;
pub mod bridge;
/// DevTools 一级菜单栏 + Console 面板的平台无关共享层(58 号设计文档)。
pub(crate) mod devtools_common;
pub mod resize;
#[cfg(target_os = "windows")]
pub mod windows;

/// 查看器子进程名(隐藏子命令,与 `__hitl-dialog` 同列)。
pub const VIEWER_ARG: &str = "__browser-view";

/// 帧种类标记。
const KIND_IMAGE: u8 = b'i';
const KIND_TITLE: u8 = b't';
const KIND_CLOSE: u8 = b'c';
/// 地址栏状态帧(URL + 前进/后退可用性)。
const KIND_URLBAR: u8 = b'u';
/// 聚焦地址栏请求(⌘/Ctrl+L;39 号 P2b)。
const KIND_FOCUS_ADDRBAR: u8 = b'f';
/// 光标形状帧(P2c:`"default"|"pointer"|"text"`,payload 为原始字符串)。
pub(crate) const KIND_CURSOR: u8 = b'k';
/// DevTools 状态帧(58 号设计文档:一级菜单栏 + Console 面板)。
/// payload 为 JSON,首字段 `"kind"` 区分:`"console_sync"` = console 台账整份快照
/// (`{"kind":"console_sync","page_id":..,"entries":[{level,text,at_ms}…]}`)。
pub(crate) const KIND_DEVTOOLS: u8 = b'd';

/// 查看器 → 宿主的交互命令(工具栏按钮 / 地址栏回车 / 人类键鼠)。
#[derive(Debug, Clone, PartialEq)]
pub enum ViewCommand {
    /// 在地址栏输入的 URL(宿主负责补全协议并导航)。
    Navigate(String),
    /// 后退。
    Back,
    /// 前进。
    Forward,
    /// 刷新当前页。
    Reload,
    /// 回到空白首页。
    Home,
    /// 人工拖动窗口后新的内容区尺寸(已扣工具栏;= 宿主侧的新视口尺寸)。
    Resize { width: u32, height: u32 },
    /// 人工在页面区内的鼠标/键盘事件(39 号设计文档)。
    Input(crate::agent::browser::human_input::HumanInput),
    /// 标签页快捷键:新标签(⌘/Ctrl+T)。
    NewTab,
    /// 标签页快捷键:关闭当前标签(⌘/Ctrl+W;最后一个标签改为打开空白页)。
    CloseTab,
    /// 标签页快捷键:切换到同窗口下一个标签(Ctrl+Tab)。
    NextTab,
    /// 控件级剪贴板(⌘/Ctrl+C/X/V 的右键菜单入口同路径)。
    Clip(crate::agent::browser::human_input::ClipOp),
    /// ⌘/Ctrl+L:聚焦地址栏(39 号 P2b 顺带补齐)。
    FocusAddrbar,
    /// ⌘/Ctrl+F:切换页面查找模式(39 号 P2c)。
    ToggleFind,
    /// P2d:人工缩放系数变化(查看器侧已生效,宿主只记账展示——
    /// `page_health` 如实上报,Agent 能解释「为什么视口变了」)。
    Zoom(f64),
    /// 58 号:一级菜单栏面板切换。`panel` = `"console"` / `"none"` /
    /// `"elements"|"sources"|…`(占位面板,宿主只认 console/none)。
    DevTools { panel: String },
    /// 58 号:Console 输入框回车执行 JS 表达式(作用于显示页)。
    ConsoleEval(String),
    /// 查看器窗口被人工关闭(`windowWillClose:` 退出前上报)。
    /// 宿主据此**不自动重生**查看器(不与用户抢窗口);进程意外死亡
    /// (崩溃,无此信号)则按自愈策略重生(见 `bridge::push_frame`)。
    ViewerClosed,
}

/// P2d:浏览器级缩放档位表(Chrome 同款步进;查看器侧生效)。
///
/// 缩放语义 = Chrome:CSS 像素变大、可视 CSS 视口变小(媒体查询重估),
/// 由查看器把内容区尺寸 ÷ 缩放系数上报为视口实现;帧等比居中绘制天然
/// 完成放大(既有 letterbox 机制复用)。Agent 截图取证仍是未缩放基准。
pub(crate) const ZOOM_STEPS: &[f64] = &[0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// P2d:缩放步进(纯函数,双平台共用)。`dir > 0` 放大、`dir < 0` 缩小;
/// 已在端点返回原值;当前值不在档位表内时先吸附到最近档位再步进。
pub(crate) fn zoom_step(cur: f64, dir: i32) -> f64 {
    if !cur.is_finite() || cur <= 0.0 {
        return 1.0;
    }
    let nearest = ZOOM_STEPS
        .iter()
        .copied()
        .min_by(|a, b| {
            (a - cur).abs().total_cmp(&(b - cur).abs())
        })
        .unwrap_or(1.0);
    match ZOOM_STEPS.iter().position(|&z| z == nearest) {
        Some(i) => {
            if dir > 0 {
                ZOOM_STEPS[(i + 1).min(ZOOM_STEPS.len() - 1)]
            } else if dir < 0 {
                ZOOM_STEPS[i.saturating_sub(1)]
            } else {
                nearest
            }
        }
        None => nearest,
    }
}

/// 解析查看器发来的一行 JSON 命令。非法行返回 None(忽略,不打断通道)。
pub fn parse_command(line: &str) -> Option<ViewCommand> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let cmd = v.get("cmd")?.as_str()?;
    match cmd {
        "navigate" => Some(ViewCommand::Navigate(
            v.get("url")?.as_str()?.trim().to_string(),
        )),
        "back" => Some(ViewCommand::Back),
        "forward" => Some(ViewCommand::Forward),
        "reload" => Some(ViewCommand::Reload),
        "home" => Some(ViewCommand::Home),
        // 人工拖动窗口 → 新内容区尺寸;非法值整行忽略(不 panic)。
        "resize" => {
            let (w, h) = resize::size_from_json(&v)?;
            if !resize::valid_view_size(w, h) {
                return None;
            }
            Some(ViewCommand::Resize {
                width: w,
                height: h,
            })
        }
        // 人工键鼠事件;解析规则见 `human_input::parse_event`(非法行忽略)。
        "input" => crate::agent::browser::human_input::parse_event(&v).map(ViewCommand::Input),
        // 标签页快捷键与控件级剪贴板(39 号设计文档阶段二)。
        "new_tab" => Some(ViewCommand::NewTab),
        "close_tab" => Some(ViewCommand::CloseTab),
        "next_tab" => Some(ViewCommand::NextTab),
        "clip" => {
            let act = v.get("act")?.as_str()?.trim().to_ascii_lowercase();
            let op = match act.as_str() {
                "copy" => crate::agent::browser::human_input::ClipOp::Copy,
                "cut" => crate::agent::browser::human_input::ClipOp::Cut,
                "paste" => crate::agent::browser::human_input::ClipOp::Paste,
                _ => return None,
            };
            Some(ViewCommand::Clip(op))
        }
        "focus_addrbar" => Some(ViewCommand::FocusAddrbar),
        "find_toggle" => Some(ViewCommand::ToggleFind),
        // P2d:人工缩放记账(查看器侧已生效;非法值整行忽略)。
        "zoom" => {
            let z = v.get("z")?.as_f64()?;
            if !z.is_finite() || !(0.25..=4.0).contains(&z) {
                return None;
            }
            Some(ViewCommand::Zoom(z))
        }
        // 58 号:一级菜单栏面板切换(console/none/占位面板名)。
        "devtools" => {
            let panel = v.get("panel")?.as_str()?.trim().to_ascii_lowercase();
            if panel.is_empty() || panel.len() > 32 {
                return None;
            }
            Some(ViewCommand::DevTools { panel })
        }
        // 58 号:Console 实时 JS 交互(表达式必非空;超长拒绝)。
        "console_eval" => {
            let expr = v.get("expr")?.as_str()?;
            if expr.trim().is_empty() || expr.len() > 64 * 1024 {
                return None;
            }
            Some(ViewCommand::ConsoleEval(expr.to_string()))
        }
        "viewer_closed" => Some(ViewCommand::ViewerClosed),
        _ => None,
    }
}

/// 父进程侧的查看器句柄。查看器死亡后所有方法变 no-op。
pub struct ViewSink {
    stdin: std::sync::Mutex<Option<std::process::ChildStdin>>,
}

impl ViewSink {
    /// 启动查看器子进程。失败(无 exe/无桌面会话)返回 None,调用方静默降级。
    ///
    /// `cmd_tx` 接收查看器发来的交互命令(工具栏/地址栏)。
    pub fn start(
        width: u32,
        height: u32,
        title: &str,
        cmd_tx: tokio::sync::mpsc::UnboundedSender<ViewCommand>,
    ) -> Option<Self> {
        // 查看器可执行文件:默认当前进程(laew 本体);
        // `LAEW_BROWSER_VIEWER_BIN` 可指向另一个二进制(自检/测试时让库进程
        // 拉起真正的 laew 查看器,而不是当前的可执行文件)。
        let exe = match std::env::var("LAEW_BROWSER_VIEWER_BIN") {
            Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
            _ => std::env::current_exe().ok()?,
        };
        let mut child = std::process::Command::new(exe)
            .arg(VIEWER_ARG)
            .arg(width.to_string())
            .arg(height.to_string())
            .arg(title)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // 查看器 stderr 默认转发进宿主运行日志(panic 文案/CrashDump 提示
            // 曾全部进 /dev/null,2026-11-04 查看器崩溃事故取证因此少了第一现场);
            // LAEW_BROWSER_VIEW_DEBUG=1 改为直连宿主 stderr(开发调试用)。
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let stderr = child.stderr.take();
        if std::env::var("LAEW_BROWSER_VIEW_DEBUG").is_ok() {
            // 调试模式:不再转发(上面 piped 收走的 stderr 改印到宿主 stderr)。
            if let Some(err) = stderr {
                std::thread::spawn(move || {
                    use std::io::BufRead as _;
                    for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                        eprintln!("[bview] {line}");
                    }
                });
            }
        } else if let Some(err) = stderr {
            std::thread::spawn(move || {
                use std::io::BufRead as _;
                for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                    tracing::warn!(target: "viewer_stderr", "{line}");
                }
            });
        }
        std::thread::spawn(move || {
            // 持有 Child 直到其退出,避免僵尸;stdin drop 后查看器自然结束。
            // 退出状态记账:2026-11-04 两度「无 CrashDump 的不明死亡」无从查因,
            // 此后每种死法(信号/退出码)都有唯一日志线索。
            match child.wait() {
                Ok(status) => {
                    tracing::warn!(target: "viewer_stderr", "查看器子进程退出:{status}");
                }
                Err(err) => {
                    tracing::warn!(target: "viewer_stderr", "查看器子进程 wait 失败:{err}");
                }
            }
        });
        // 命令通道:查看器 stdout 按行 JSON → 宿主命令队列。
        std::thread::spawn(move || {
            use std::io::BufRead as _;
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Some(cmd) = parse_command(&line) {
                    if cmd_tx.send(cmd).is_err() {
                        break;
                    }
                }
            }
        });
        Some(Self {
            stdin: std::sync::Mutex::new(Some(stdin)),
        })
    }

    /// 写通道是否仍可交互;人工关窗导致写失败后允许宿主重建查看器。
    pub fn is_alive(&self) -> bool {
        self.stdin.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    fn send(&self, kind: u8, payload: &[u8]) {
        if let Ok(mut guard) = self.stdin.lock() {
            if let Some(w) = guard.as_mut() {
                let total = (payload.len() as u32 + 1).to_le_bytes();
                let ok = w.write_all(&total).is_ok()
                    && w.write_all(&[kind]).is_ok()
                    && w.write_all(payload).is_ok()
                    && w.flush().is_ok();
                if !ok {
                    // 写失败:查看器已退出,关闭句柄,后续 no-op。
                    *guard = None;
                }
            }
        }
    }

    /// 推一帧(视口尺寸 PNG)。
    pub fn send_frame(&self, png: &[u8]) {
        self.send(KIND_IMAGE, png);
    }

    /// 更新窗口标题(通常 = page_id · title · url)。
    pub fn send_title(&self, title: &str) {
        let payload = serde_json::json!({ "title": title }).to_string();
        self.send(KIND_TITLE, payload.as_bytes());
    }

    /// 同步地址栏内容与前进/后退可用性。
    pub fn send_urlbar(&self, url: &str, can_back: bool, can_forward: bool) {
        let payload = serde_json::json!({
            "url": url,
            "can_back": can_back,
            "can_forward": can_forward,
        })
        .to_string();
        self.send(KIND_URLBAR, payload.as_bytes());
    }

    /// 请求查看器关闭。
    pub fn send_close(&self) {
        self.send(KIND_CLOSE, b"");
    }

    /// ⌘/Ctrl+L:请求查看器把焦点交给地址栏(39 号 P2b)。
    /// 查看器侧收到后调平台原生 API(`makeFirstResponder:` / `SetFocus`)。
    pub fn focus_addrbar(&self) {
        self.send(KIND_FOCUS_ADDRBAR, b"");
    }

    /// P2c:上报光标形状(`"default"|"pointer"|"text"`),查看器跟随变形。
    pub fn send_cursor(&self, kind: &str) {
        self.send(KIND_CURSOR, kind.as_bytes());
    }

    /// 58 号:推一条 DevTools 状态帧(payload = 原始 JSON 字符串)。
    /// 首字段 `"kind"` 区分语义(当前:`"console_sync"` console 台账整份快照)。
    pub fn send_devtools(&self, payload_json: &str) {
        self.send(KIND_DEVTOOLS, payload_json.as_bytes());
    }
}

impl std::fmt::Debug for ViewSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewSink")
            .field("alive", &self.stdin.lock().map(|g| g.is_some()).unwrap_or(false))
            .finish()
    }
}

use std::io::Write as _;

/// 从 stdin 读一帧。EOF/长度异常返回 None(查看器应退出)。
pub fn read_frame<R: std::io::Read>(r: &mut R) -> Option<(u8, Vec<u8>)> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).ok()?;
    let total = u32::from_le_bytes(len_buf) as usize;
    if total == 0 || total > 64 * 1024 * 1024 {
        return None;
    }
    let mut buf = vec![0u8; total];
    r.read_exact(&mut buf).ok()?;
    Some((buf[0], buf[1..].to_vec()))
}

/// 查看器进程主入口(main.rs 的 `__browser-view` 子命令调用)。
/// 阻塞运行原生窗口事件循环,直到收到 'c' 或 stdin EOF。
pub fn run_viewer_main(width: u32, height: u32, title: &str) -> i32 {
    #[cfg(target_os = "macos")]
    {
        macos::run(width, height, title)
    }
    #[cfg(target_os = "windows")]
    {
        windows::run(width, height, title)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux/其它平台:阶段三后续接入;先消费 stdin 防止父进程写管道阻塞。
        let _ = (width, height, title);
        let mut buf = [0u8; 4096];
        use std::io::Read as _;
        let mut stdin = std::io::stdin();
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 {
                break;
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::browser::human_input;

    #[test]
    fn 解析命令_覆盖全部动作() {
        assert_eq!(
            parse_command(r#"{"cmd":"navigate","url":"https://a.test/x"}"#),
            Some(ViewCommand::Navigate("https://a.test/x".to_string()))
        );
        assert_eq!(parse_command(r#"{"cmd":"back"}"#), Some(ViewCommand::Back));
        assert_eq!(
            parse_command(r#"{"cmd":"forward"}"#),
            Some(ViewCommand::Forward)
        );
        assert_eq!(parse_command(r#"{"cmd":"reload"}"#), Some(ViewCommand::Reload));
        assert_eq!(parse_command(r#"{"cmd":"home"}"#), Some(ViewCommand::Home));
    }

    #[test]
    fn 已关闭写通道报告查看器不可用() {
        let sink = ViewSink {
            stdin: std::sync::Mutex::new(None),
        };
        assert!(!sink.is_alive());
    }

    #[test]
    fn 解析命令_非法行返回空() {
        assert_eq!(parse_command(""), None);
        assert_eq!(parse_command("not json"), None);
        assert_eq!(parse_command(r#"{"cmd":"unknown"}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"navigate"}"#), None);
    }

    #[test]
    fn 解析命令_输入事件与标签页剪贴板命令() {
        // 键鼠事件(阶段一)。
        assert_eq!(
            parse_command(r#"{"cmd":"input","act":"move","x":10,"y":20}"#),
            Some(ViewCommand::Input(human_input::HumanInput::Move { x: 10.0, y: 20.0 }))
        );
        // 标签页快捷键与剪贴板(阶段二)。
        assert_eq!(parse_command(r#"{"cmd":"new_tab"}"#), Some(ViewCommand::NewTab));
        assert_eq!(parse_command(r#"{"cmd":"close_tab"}"#), Some(ViewCommand::CloseTab));
        assert_eq!(parse_command(r#"{"cmd":"next_tab"}"#), Some(ViewCommand::NextTab));
        assert_eq!(
            parse_command(r#"{"cmd":"clip","act":"copy"}"#),
            Some(ViewCommand::Clip(human_input::ClipOp::Copy))
        );
        assert_eq!(
            parse_command(r#"{"cmd":"clip","act":"cut"}"#),
            Some(ViewCommand::Clip(human_input::ClipOp::Cut))
        );
        assert_eq!(
            parse_command(r#"{"cmd":"clip","act":"paste"}"#),
            Some(ViewCommand::Clip(human_input::ClipOp::Paste))
        );
        // 非法 clip 动作忽略。
        assert_eq!(parse_command(r#"{"cmd":"clip"}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"clip","act":"fly"}"#), None);
        // P2c:页面查找开关(⌘/Ctrl+F)。
        assert_eq!(
            parse_command(r#"{"cmd":"find_toggle"}"#),
            Some(ViewCommand::ToggleFind)
        );
        // P2b:聚焦地址栏。
        assert_eq!(
            parse_command(r#"{"cmd":"focus_addrbar"}"#),
            Some(ViewCommand::FocusAddrbar)
        );
    }

    #[test]
    fn 光标帧常量与命令互斥() {
        // 帧 kind 不得与既有种类冲突('i'/'t'/'c'/'u'/'f'/'d')。
        assert_ne!(KIND_CURSOR, KIND_IMAGE);
        assert_ne!(KIND_CURSOR, KIND_TITLE);
        assert_ne!(KIND_CURSOR, KIND_CLOSE);
        assert_ne!(KIND_CURSOR, KIND_URLBAR);
        assert_ne!(KIND_CURSOR, KIND_FOCUS_ADDRBAR);
        assert_ne!(KIND_CURSOR, KIND_DEVTOOLS);
    }

    #[test]
    fn 解析命令_devtools面板与console求值() {
        // 面板切换:console / none / 占位面板名(统一小写归一)。
        assert_eq!(
            parse_command(r#"{"cmd":"devtools","panel":"console"}"#),
            Some(ViewCommand::DevTools {
                panel: "console".to_string()
            })
        );
        assert_eq!(
            parse_command(r#"{"cmd":"devtools","panel":"none"}"#),
            Some(ViewCommand::DevTools {
                panel: "none".to_string()
            })
        );
        assert_eq!(
            parse_command(r#"{"cmd":"devtools","panel":"Elements"}"#),
            Some(ViewCommand::DevTools {
                panel: "elements".to_string()
            })
        );
        // 非法行:缺 panel / 空串 / 超长。
        assert_eq!(parse_command(r#"{"cmd":"devtools"}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"devtools","panel":"  "}"#), None);
        assert_eq!(
            parse_command(&format!(r#"{{"cmd":"devtools","panel":"{}"}}"#, "x".repeat(40))),
            None
        );
        // console_eval:表达式保留原文;空/纯空白/缺字段拒绝。
        assert_eq!(
            parse_command(r#"{"cmd":"console_eval","expr":"1 + 2"}"#),
            Some(ViewCommand::ConsoleEval("1 + 2".to_string()))
        );
        assert_eq!(
            parse_command(r#"{"cmd":"console_eval","expr":"var x = 1; x"}"#),
            Some(ViewCommand::ConsoleEval("var x = 1; x".to_string()))
        );
        assert_eq!(parse_command(r#"{"cmd":"console_eval"}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"console_eval","expr":"   "}"#), None);
    }

    #[test]
    fn 解析命令_人工关窗信号() {
        // 查看器 windowWillClose: 退出前上报;宿主据此不自动重生窗口。
        assert_eq!(
            parse_command(r#"{"cmd":"viewer_closed"}"#),
            Some(ViewCommand::ViewerClosed)
        );
    }

    #[test]
    fn 解析命令_人工缩放() {
        assert_eq!(
            parse_command(r#"{"cmd":"zoom","z":1.25}"#),
            Some(ViewCommand::Zoom(1.25))
        );
        assert_eq!(parse_command(r#"{"cmd":"zoom","z":1}"#), Some(ViewCommand::Zoom(1.0)));
        // 非法值忽略(缺字段 / 越界 / 非数值)。
        assert_eq!(parse_command(r#"{"cmd":"zoom"}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"zoom","z":0}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"zoom","z":8}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"zoom","z":"big"}"#), None);
    }

    #[test]
    fn 缩放档位步进_端点与吸附() {
        // 基准档位步进。
        assert_eq!(zoom_step(1.0, 1), 1.1);
        assert_eq!(zoom_step(1.0, -1), 0.9);
        assert_eq!(zoom_step(0.9, -1), 0.8);
        // 端点饱和(不再外推)。
        assert_eq!(zoom_step(ZOOM_STEPS[0], -1), ZOOM_STEPS[0]);
        assert_eq!(zoom_step(*ZOOM_STEPS.last().unwrap(), 1), *ZOOM_STEPS.last().unwrap());
        // 表外当前值先吸附最近档再步进(1.05 ≈ 1.0 → 放大到 1.1)。
        assert_eq!(zoom_step(1.05, 1), 1.1);
        // 非法当前值回到 1.0 基准;dir=0 只吸附。
        assert_eq!(zoom_step(f64::NAN, 1), 1.0);
        assert_eq!(zoom_step(0.0, 1), 1.0);
        assert_eq!(zoom_step(1.24, 0), 1.25);
    }

    #[test]
    fn 解析命令_窗口尺寸变化() {
        assert_eq!(
            parse_command(r#"{"cmd":"resize","w":1920,"h":1080}"#),
            Some(ViewCommand::Resize {
                width: 1920,
                height: 1080
            })
        );
        // 长名别名。
        assert_eq!(
            parse_command(r#"{"cmd":"resize","width":800,"height":600}"#),
            Some(ViewCommand::Resize {
                width: 800,
                height: 600
            })
        );
    }

    #[test]
    fn 解析命令_尺寸缺字段或越界忽略() {
        // 缺一不可。
        assert_eq!(parse_command(r#"{"cmd":"resize","w":1920}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"resize","h":1080}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"resize"}"#), None);
        // 越界 / 非法值。
        assert_eq!(parse_command(r#"{"cmd":"resize","w":0,"h":1080}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"resize","w":1920,"h":0}"#), None);
        assert_eq!(
            parse_command(r#"{"cmd":"resize","w":7681,"h":1080}"#),
            None
        );
        assert_eq!(
            parse_command(r#"{"cmd":"resize","w":1920,"h":4321}"#),
            None
        );
        assert_eq!(parse_command(r#"{"cmd":"resize","w":"1920","h":1080}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"resize","w":-5,"h":1080}"#), None);
    }
}
