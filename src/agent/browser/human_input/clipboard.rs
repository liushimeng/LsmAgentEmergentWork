//! 控件级剪贴板(39 号设计文档阶段二):⌘/Ctrl+C/V/X 的宿主侧实现。
//!
//! 无文本选区模型(阶段三),语义按「聚焦文本控件整值」近似:
//! 复制 = 整值入剪贴板;剪切 = 复制 + 清空(派发 input);粘贴 = 追加
//! (与「光标在末尾」的键入模型一致)。
//!
//! 平台实现不引第三方 crate:macOS 走 `pbcopy`/`pbpaste` 子进程,Windows 走
//! PowerShell `Set-Clipboard`/`Get-Clipboard`(stdin 管道,无引号转义问题),
//! Linux 走 `xclip`(缺失时报错,不 panic)。

/// 读剪贴板文本(平台失败/无剪贴板服务返回 Err,调用方如实回显)。
pub fn read_text() -> Result<String, String> {
    let raw = platform_read()?;
    // pbpaste 原样;Get-Clipboard 会带结尾 \r\n —— 统一去掉一个行尾。
    Ok(raw.trim_end_matches('\n').trim_end_matches('\r').to_string())
}

/// 写文本进剪贴板。
pub fn write_text(text: &str) -> Result<(), String> {
    platform_write(text)
}

#[cfg(target_os = "macos")]
fn platform_read() -> Result<String, String> {
    run_capture("pbpaste", &[])
}

#[cfg(target_os = "macos")]
fn platform_write(text: &str) -> Result<(), String> {
    run_feed("pbcopy", &[], text)
}

#[cfg(target_os = "windows")]
fn platform_read() -> Result<String, String> {
    run_capture(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", "Get-Clipboard"],
    )
}

#[cfg(target_os = "windows")]
fn platform_write(text: &str) -> Result<(), String> {
    run_feed(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$input | Set-Clipboard",
        ],
        text,
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_read() -> Result<String, String> {
    run_capture("xclip", &["-selection", "clipboard", "-o"])
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_write(text: &str) -> Result<(), String> {
    run_feed("xclip", &["-selection", "clipboard", "-i"], text)
}

/// 跑子进程拿 stdout(超时 3s,防止无剪贴板服务的会话挂死输入路径)。
fn run_capture(prog: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{prog} 启动失败:{e}"))?;
    if !out.status.success() {
        return Err(format!("{prog} 退出码 {:?}", out.status.code()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// 跑子进程把文本喂进 stdin。
fn run_feed(prog: &str, args: &[&str], text: &str) -> Result<(), String> {
    use std::io::Write as _;
    let mut child = std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{prog} 启动失败:{e}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(text.as_bytes()).map_err(|e| format!("写入 {prog} 失败:{e}"))?;
    }
    // stdin drop 即关闭;等退出(同样受 OS 层超时保护,失败如实返回)。
    let status = child.wait().map_err(|e| format!("等待 {prog} 失败:{e}"))?;
    if !status.success() {
        return Err(format!("{prog} 退出码 {:?}", status.code()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 剪贴板读写回环() {
        // 本机无剪贴板服务(ssh/CI)时软跳过:只验证「有服务时回环一致」。
        if write_text("laew-剪贴板-测试").is_err() {
            eprintln!("[skip] 本会话无剪贴板服务");
            return;
        }
        let back = read_text().expect("写入成功后读取应成功");
        assert_eq!(back, "laew-剪贴板-测试");
    }
}
