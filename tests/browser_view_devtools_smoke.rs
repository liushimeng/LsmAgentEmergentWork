//! DevTools 面板开合的查看器命令契约冒烟(58 号设计文档)。
//!
//! 跑真实 `laew __browser-view` 子进程(与生产同构),喂合成帧验证两件事:
//! 1. `LAEW_DEVTOOLS_AUTO=console` 启动即展开 Console 面板,查看器经命令通道
//!    上报 `{"cmd":"devtools","panel":"console"}`(宿主据此开快照同步);
//! 2. **视口收缩契约**(Chrome dock-to-bottom 语义):面板展开后上报的
//!    `resize` 高度 = argv 视口高 - 面板高(800 - 260 = 540,工具栏/菜单栏
//!    高度加进窗口内容区又被同量扣除,双平台同值)。
//!
//! 窗口创建需要桌面会话:默认跳过,`LAEW_SMOKE_DEVTOOLS=1` 显式启用
//! (与 `window_input_smoke` 的门控惯例一致)。仅 macOS 查看器有面板 UI
//! (Linux 查看器是 stdin 消费桩),故整文件 macOS 门控。
#![cfg(target_os = "macos")]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 复制真实 laew 二进制到隔离目录(CLI 集成测试惯例,见 `cli_provider.rs`)。
fn isolated_laew() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("创建隔离目录失败");
    let bin = dir.path().join("laew");
    std::fs::copy(Path::new(env!("CARGO_BIN_EXE_laew")), &bin).expect("复制二进制失败");
    (dir, bin)
}

/// 组一帧(stdin 协议:`[u32 总长][u8 kind][payload]`)。
fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.extend_from_slice(&((payload.len() + 1) as u32).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

#[test]
fn 查看器devtools面板开合_命令与视口收缩契约() {
    if std::env::var("LAEW_SMOKE_DEVTOOLS").ok().as_deref() != Some("1") {
        eprintln!("跳过:需桌面会话,LAEW_SMOKE_DEVTOOLS=1 启用");
        return;
    }
    let (_dir, bin) = isolated_laew();
    let mut child = Command::new(&bin)
        .arg("__browser-view")
        .arg("1200")
        .arg("800")
        .arg("devtools-smoke")
        .env("LAEW_DEVTOOLS_AUTO", "console")
        .env("LAEW_BROWSER_VIEW_DEBUG", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("拉起 __browser-view 失败");

    let mut stdin = child.stdin.take().expect("拿 stdin 失败");
    // 读线程:命令通道按行 JSON(viewer → 宿主)。
    let stdout = child.stdout.take().expect("拿 stdout 失败");
    let reader = std::thread::spawn(move || {
        let mut lines = Vec::new();
        let mut buf = String::new();
        let mut raw = stdout;
        let mut chunk = [0u8; 4096];
        loop {
            match raw.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    while let Some(pos) = buf.find('\n') {
                        lines.push(buf[..pos].to_string());
                        buf.drain(..=pos);
                    }
                }
                Err(_) => break,
            }
        }
        lines
    });

    // 写线程:console 快照帧 → 延迟 → 关闭帧(心跳 100ms 消费,给足时间)。
    let payload = br#"{"kind":"console_sync","page_id":"p_00000001","entries":[{"level":"log","text":"smoke","at_ms":1}]}"#;
    stdin
        .write_all(&frame(b'd', payload))
        .expect("写 console_sync 帧失败");
    let _ = stdin.flush();
    std::thread::sleep(Duration::from_millis(600));
    stdin.write_all(&frame(b'c', b"")).expect("写关闭帧失败");
    let _ = stdin.flush();

    // 收口:查看器必须在限时内干净退出(unrecognized selector 类崩溃 = 永不退出)。
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(st) = child.try_wait().expect("wait 失败") {
            break st;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("查看器 10s 内未退出(AppKit 异常/挂起?)");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(status.success(), "查看器应零退出码,实际 {status:?}");

    let lines = reader.join().expect("读线程失败");
    let has = |frag: &str| lines.iter().any(|l| l.contains(frag));
    assert!(
        has(r#""cmd":"devtools","panel":"console""#),
        "应上报 devtools 面板命令,实际命令行:{lines:?}"
    );
    // 视口收缩契约:800 - 面板 260 = 540(工具栏/菜单栏计入窗口内容区又同量扣除)。
    assert!(
        has(r#""cmd":"resize","w":1200,"h":540"#),
        "面板展开后视口应收缩为 1200x540,实际命令行:{lines:?}"
    );
}
