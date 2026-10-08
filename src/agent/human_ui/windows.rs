//! Windows 弹窗后端:PowerShell WinForms。
//!
//! 脚本本体见 `scripts/windows_dialog.ps1`(可被 `.laew/human_ui/windows.ps1` 动态覆盖);
//! 进程契约(payload/result JSON)见 03 文档 §4.3。`-STA` 是 WinForms 要求。

use std::path::Path;

use super::{run_script_process, UiResult};

/// 启动 WinForms 弹窗脚本并等待应答(进程化,drop 即 kill)。
pub(super) async fn run(script_path: &Path, payload_path: &Path) -> UiResult {
    let program = if powershell_path().is_some() {
        "powershell.exe"
    } else {
        "pwsh"
    };
    let args: Vec<std::ffi::OsString> = vec![
        "-NoProfile".into(),
        "-STA".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-File".into(),
        script_path.as_os_str().to_owned(),
        payload_path.as_os_str().to_owned(),
    ];
    run_script_process(program, &args).await
}

fn powershell_path() -> Option<std::path::PathBuf> {
    let sys_root = std::env::var("SystemRoot").ok()?;
    let p = std::path::PathBuf::from(sys_root)
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    p.exists().then_some(p)
}
