//! 弹窗脚本动态加载 + payload 落盘(设计见 03 文档 §4.3/§6)。
//!
//! 脚本查找序(**每次呈现重读,不缓存 —— 改完即生效**):
//! 1. `LAEW_HUMAN_UI_SCRIPT` 显式路径(企业定制/调试);
//! 2. `{工作目录}/.laew/human_ui/{name}`;
//! 3. `{根目录}/.laew/human_ui/{name}`(laew 二进制所在目录);
//! 4. `~/.laew/human_ui/{name}`;
//! 5. 内置默认脚本(`include_str!`)。
//!
//! 其中 `name` = `macos.js` / `windows.ps1`(用户覆盖文件名,短名好记);
//! 内置脚本本体在 `src/agent/human_ui/scripts/`。

use std::path::{Path, PathBuf};

use serde_json::Value;

/// 脚本来源(日志/降级提示用)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptOrigin {
    /// 内置默认(执行前需落盘临时文件)。
    Embedded,
    /// 用户覆盖文件(直接执行原路径)。
    Path(PathBuf),
}

/// 已解析的弹窗脚本。
pub struct ResolvedScript {
    pub origin: ScriptOrigin,
    pub body: String,
}

/// 已物化的可执行脚本(内置脚本的临时文件随 Drop 清理)。
pub struct MaterializedScript {
    pub path: PathBuf,
    /// true = 内置脚本临时文件(Drop 删除);false = 用户覆盖脚本原路径(保留)。
    pub(super) owned: bool,
}

impl Drop for MaterializedScript {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl ResolvedScript {
    /// 内置脚本写临时文件;用户脚本直接复用原路径。
    pub fn materialize(&self) -> Result<MaterializedScript, String> {
        match &self.origin {
            ScriptOrigin::Path(p) => Ok(MaterializedScript {
                path: p.clone(),
                owned: false,
            }),
            ScriptOrigin::Embedded => {
                let path = std::env::temp_dir().join(format!(
                    "laew_human_ui_{}.{}",
                    std::process::id(),
                    script_file_suffix()
                ));
                std::fs::write(&path, &self.body)
                    .map_err(|e| format!("弹窗脚本落盘失败 {}: {e}", path.display()))?;
                Ok(MaterializedScript { path, owned: true })
            }
        }
    }
}

/// 用户覆盖脚本文件名(`.laew/human_ui/` 下的短名)。
fn override_script_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macos.js"
    }
    #[cfg(target_os = "windows")]
    {
        "windows.ps1"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "unsupported.txt"
    }
}

fn script_file_suffix() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "js"
    }
    #[cfg(target_os = "windows")]
    {
        "ps1"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "txt"
    }
}

fn embedded_body() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        include_str!("scripts/macos_dialog.js")
    }
    #[cfg(target_os = "windows")]
    {
        include_str!("scripts/windows_dialog.ps1")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        ""
    }
}

/// 按查找序解析弹窗脚本;全 miss 回落内置默认。
pub fn resolve_script() -> Result<ResolvedScript, String> {
    let name = override_script_name();
    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1. 显式环境变量
    if let Ok(explicit) = std::env::var("LAEW_HUMAN_UI_SCRIPT") {
        let p = PathBuf::from(explicit.trim());
        if p.is_file() {
            return load_path(&p);
        }
        return Err(format!(
            "LAEW_HUMAN_UI_SCRIPT 指向的脚本不存在: {}",
            p.display()
        ));
    }

    // 2. 工作目录  3. 根目录(二进制目录)  4. 用户目录
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".laew").join("human_ui").join(name));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(".laew").join("human_ui").join(name));
        }
    }
    if let Some(home) = home_dir() {
        candidates.push(home.join(".laew").join("human_ui").join(name));
    }

    for p in &candidates {
        if p.is_file() {
            return load_path(p);
        }
    }

    // 5. 内置默认
    Ok(ResolvedScript {
        origin: ScriptOrigin::Embedded,
        body: embedded_body().to_string(),
    })
}

fn load_path(p: &Path) -> Result<ResolvedScript, String> {
    let body = std::fs::read_to_string(p)
        .map_err(|e| format!("弹窗脚本读取失败 {}: {e}", p.display()))?;
    Ok(ResolvedScript {
        origin: ScriptOrigin::Path(p.to_path_buf()),
        body,
    })
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// payload JSON 落盘到临时文件(弹窗进程入参;调用方负责删除)。
pub fn write_payload(payload: &Value, id: u64) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "laew_human_ui_payload_{}_{}.json",
        std::process::id(),
        id
    ));
    let body = serde_json::to_string(payload).map_err(|e| format!("payload 序列化失败: {e}"))?;
    std::fs::write(&path, body).map_err(|e| format!("payload 落盘失败 {}: {e}", path.display()))?;
    Ok(path)
}
