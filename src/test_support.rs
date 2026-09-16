//! Lib 单元测试专用的跨模块串行化锁。
//!
//! Rust 测试默认并行。sandbox_hook 的 cwd 测试会短暂切换进程工作目录,
//! BashTool 测试又会把当前目录作为子进程 current_dir;二者使用不同模块锁时
//! 会出现「Bash 捕获到临时目录,临时目录随后被删,spawn 报 ENOENT」的竞态。
//! 这里提供全局锁,让所有会读写进程级 cwd / 环境变量的关键测试串行。

#[cfg(test)]
pub static GLOBAL_ENV_CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
