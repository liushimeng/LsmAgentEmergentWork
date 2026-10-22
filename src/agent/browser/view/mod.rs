//! 有头帧查看器(阶段三原生窗口的先行落地)。
//!
//! 架构与 `__hitl-dialog` 同一模式:内核进程**零 UI 依赖**(AppKit/Win32 不进
//! 引擎线程),软件渲染产物(RGBA→PNG)经 stdin 长度前缀帧协议推给独立查看器
//! 子进程 `laew __browser-view`,由子进程原生窗口显示。
//!
//! 帧协议(stdin,二进制,小端):
//! `[u32 总长][u8 kind][payload]`
//! kind `'i'` = PNG 帧;`'t'` = 标题 JSON({"title": "..."});`'c'` = 关闭;
//! `'u'` = 地址栏状态 JSON({"url","can_back","can_forward"})。
//! 写失败即视为查看器退出,静默降级为纯无头(不影响任何工具行为)。
//!
//! 命令通道(stdout,查看器 → 宿主,按行 JSON):
//! `{"cmd":"navigate","url":"..."}` / `{"cmd":"back"}` / `{"cmd":"forward"}` /
//! `{"cmd":"reload"}` / `{"cmd":"home"}`。
//! 浏览器窗口的工具栏(后退/前进/刷新/首页)与地址栏都经这条通道驱动宿主引擎,
//! 窗口因此是**可交互的真浏览器形态**,而不只是被动显示渲染帧。

#[cfg(target_os = "macos")]
pub mod macos;
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

/// 查看器 → 宿主的交互命令(工具栏按钮 / 地址栏回车)。
#[derive(Debug, Clone, PartialEq, Eq)]
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
                        // 调试开关:LAEW_BROWSER_VIEW_DEBUG=1 时把查看器 stderr 接到宿主,
            // 便于抓 AppKit 侧的异常输出(默认丢弃,避免污染宿主日志)。
            .stderr(if std::env::var("LAEW_BROWSER_VIEW_DEBUG").is_ok() {
                std::process::Stdio::inherit()
            } else {
                std::process::Stdio::null()
            })
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        std::thread::spawn(move || {
            // 持有 Child 直到其退出,避免僵尸;stdin drop 后查看器自然结束。
            let _ = child.wait();
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
    fn 解析命令_非法行返回空() {
        assert_eq!(parse_command(""), None);
        assert_eq!(parse_command("not json"), None);
        assert_eq!(parse_command(r#"{"cmd":"unknown"}"#), None);
        assert_eq!(parse_command(r#"{"cmd":"navigate"}"#), None);
    }
}
