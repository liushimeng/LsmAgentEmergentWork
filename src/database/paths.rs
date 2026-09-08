//! 路径解析模块
//!
//! 负责解析「根目录」(二进制所在目录) 与「工作目录」(启动目录)。

use std::path::PathBuf;

/// 路径上下文:根目录 / 工作目录 / 数据库路径
#[derive(Debug, Clone)]
pub struct Paths {
    pub root_dir: PathBuf,
    pub work_dir: PathBuf,
    pub db_path: PathBuf,
}

const DB_FILE_NAME: &str = "LsmAgentEmergentWork.db";

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
        let db_path = root_dir.join(DB_FILE_NAME);
        Ok(Self { root_dir, work_dir, db_path })
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
