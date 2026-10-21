//! 有头帧查看器(阶段三原生窗口的先行落地)。
//!
//! 架构与 `__hitl-dialog` 同一模式:内核进程**零 UI 依赖**(AppKit/Win32 不进
//! 引擎线程),软件渲染产物(RGBA→PNG)经 stdin 长度前缀帧协议推给独立查看器
//! 子进程 `laew __browser-view`,由子进程原生窗口显示。
//!
//! 帧协议(stdin,二进制,小端):
//! `[u32 总长][u8 kind][payload]`
//! kind `'i'` = PNG 帧;`'t'` = 标题 JSON({"title": "..."});`'c'` = 关闭。
//! 写失败即视为查看器退出,静默降级为纯无头(不影响任何工具行为)。

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

/// 父进程侧的查看器句柄。查看器死亡后所有方法变 no-op。
pub struct ViewSink {
    stdin: std::sync::Mutex<Option<std::process::ChildStdin>>,
}

impl ViewSink {
    /// 启动查看器子进程。失败(无 exe/无桌面会话)返回 None,调用方静默降级。
    pub fn start(width: u32, height: u32, title: &str) -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        let mut child = std::process::Command::new(exe)
            .arg(VIEWER_ARG)
            .arg(width.to_string())
            .arg(height.to_string())
            .arg(title)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        std::thread::spawn(move || {
            // 持有 Child 直到其退出,避免僵尸;stdin drop 后查看器自然结束。
            let _ = child.wait();
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
