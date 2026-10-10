//! 路径解析模块
//!
//! 负责解析「根目录」(二进制所在目录) 与「工作目录」(启动目录)。
//!
//! 第 152 轮:三者职责收敛为
//! - `root_dir` **只**用于「程序自带资源」定位(如 `__hitl-dialog` 子进程可执行文件),
//!   不再作为任何产物的落盘基准;
//! - `work_dir` = 启动目录,skills/commands/agents/导出/@提及 的基准(不变);
//! - **产物落盘基准统一走 [`crate::artifact_root`]**(plans/DebugReport/AuditTrail/
//!   BashSpill/logs/CrashReport/EvalSpill/WebShots),不再逃逸到二进制所在目录;
//! - `db_path` 仍优先 `root_dir`(接入点/会话记忆是跨工作目录的全局状态,
//!   迁移会丢存量),仅当 `root_dir` 不可写时回退 `<产物根>/.laew/`。

use std::path::PathBuf;

/// 路径上下文:根目录 / 工作目录 / 数据库路径
#[derive(Debug, Clone)]
pub struct Paths {
    pub root_dir: PathBuf,
    pub work_dir: PathBuf,
    pub db_path: PathBuf,
}

const DB_FILE_NAME: &str = "LsmAgentEmergentWork.db";

/// 目录是否可写(能创建其中的探针文件)。不可写时用于 DB 等必须落盘的路径回退。
fn dir_writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(".laew_write_probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

impl Paths {
    /// 自动探测根目录与工作目录
    pub fn detect() -> crate::database::Result<Self> {
        let exe = std::env::current_exe()
            .map_err(|e| crate::database::ConfigError::RootDir(e.to_string()))?;
        let root_dir = exe
            .parent()
            .ok_or_else(|| crate::database::ConfigError::RootDir("current_exe 无父目录".into()))?
            .to_path_buf();
        let work_dir = std::env::current_dir().unwrap_or_else(|_| root_dir.clone());
        // 第 152 轮:root_dir 不可写(如 /opt/homebrew/bin 归 root 所有、只读挂载)
        // 时 DB 回退到产物根下的 `.laew/`,避免「整条链路因库写不进去而瘫掉」。
        let db_path = if dir_writable(&root_dir) {
            root_dir.join(DB_FILE_NAME)
        } else {
            let fallback = crate::artifact_root::artifact_dir(".laew");
            let _ = std::fs::create_dir_all(&fallback);
            fallback.join(DB_FILE_NAME)
        };
        Ok(Self {
            root_dir,
            work_dir,
            db_path,
        })
    }

    /// 用于测试:人为指定目录
    pub fn for_test(dir: &std::path::Path) -> Self {
        Self {
            root_dir: dir.to_path_buf(),
            work_dir: dir.to_path_buf(),
            db_path: dir.join(DB_FILE_NAME),
        }
    }
}
