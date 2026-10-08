//! macOS 弹窗后端:osascript + JXA(AppKit NSAlert)。
//!
//! 脚本本体见 `scripts/macos_dialog.js`(可被 `.laew/human_ui/macos.js` 动态覆盖);
//! 进程契约(payload/result JSON)见 03 文档 §4.3。

use std::path::Path;

use super::{run_script_process, UiResult};

/// 启动 JXA 弹窗脚本并等待应答(进程化,drop 即 kill)。
pub(super) async fn run(script_path: &Path, payload_path: &Path) -> UiResult {
    let args: Vec<std::ffi::OsString> = vec![
        "-l".into(),
        "JavaScript".into(),
        script_path.as_os_str().to_owned(),
        payload_path.as_os_str().to_owned(),
    ];
    run_script_process("osascript", &args).await
}
