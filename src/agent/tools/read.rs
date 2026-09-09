//! Read 工具:带行号读取文件,支持 offset/limit 分页。

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tools::Tool;
use crate::error::{AgentError, Result};

const DEFAULT_LIMIT: usize = 2000;
const MAX_LIMIT: usize = 4000;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str { "Read" }

    fn description(&self) -> &str {
        "读取文本文件并按 cat -n 风格返回带行号的内容。\n\
         - file_path 必须为绝对路径或可解析的相对路径(相对于工作目录)。\n\
         - offset/limit 用于分页,limit 默认 2000 行,最大 4000。\n\
         - 单行超长会被截断到 4000 字符并标注。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "待读取的文件路径" },
                "offset": { "type": "integer", "minimum": 1, "description": "从第 N 行开始(1-based)" },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT as i64, "description": "最多读取多少行" }
            },
            "required": ["file_path"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path_str = args
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "缺少 string 类型参数 file_path".into(),
            })?;
        let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_LIMIT as u64)
            .min(MAX_LIMIT as u64) as usize;

        let path = resolve_path(path_str);
        let metadata = fs::metadata(&path).map_err(|e| {
            // 关联报告: 2026-09-09_05 E-002 — 当路径不存在时,让错误消息同时显示
            // 「工作目录拼接路径」与「根目录回退路径」(若两者不同),便于 LLM / 用户快速
            // 定位路径解析方向(否则 LLM 可能反复构造根目录变体路径浪费迭代)。
            let attempted = resolve_path_with_candidates(path_str);
            let reason_msg = match attempted {
                ResolveCandidates::Single(p) => {
                    format!("stat 失败: {e} (tried: {})", p.display())
                }
                ResolveCandidates::WorkAndRoot { work, root } => {
                    format!(
                        "stat 失败: {e} (tried [work]: {}, [root]: {})",
                        work.display(),
                        root.display()
                    )
                }
            };
            AgentError::ToolExecution {
                tool: self.name().into(),
                reason: reason_msg,
            }
        })?;
        if !metadata.is_file() {
            return Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!("不是普通文件: {}", path.display()),
            });
        }

        let content = fs::read_to_string(&path).map_err(|e| AgentError::ToolExecution {
            tool: self.name().into(),
            reason: format!("读取失败: {e}"),
        })?;

        let lines: Vec<&str> = content.split_inclusive('\n').collect();
        let total = lines.len();
        let start_idx = (offset - 1).min(total);
        let end_idx = (start_idx + limit).min(total);

        let width = (total.max(1)).to_string().len();
        let mut buf = String::new();
        buf.push_str(&format!(
            "<<< {} (lines {}-{} / total {}) >>>\n",
            path.display(),
            offset,
            end_idx,
            total
        ));
        for (i, line) in lines[start_idx..end_idx].iter().enumerate() {
            let line_num = start_idx + i + 1;
            let display = if line.len() > 4000 {
                let cut = line
                    .char_indices()
                    .nth(4000)
                    .map(|(n, _)| n)
                    .unwrap_or(line.len());
                format!("{}...[截断]", &line[..cut])
            } else {
                line.trim_end_matches('\n').to_string()
            };
            buf.push_str(&format!("{:>width$}\t{}\n", line_num, display, width = width));
        }
        Ok(buf)
    }
}

/// 把工具入参里的相对路径解析为绝对路径(关联报告: 2026-09-09_05 E-002)。
///
/// 解析顺序(关联报告: 2026-09-09_04 D-003 + 2026-09-09_05 E-002):
/// 1. 已是绝对路径 → 原样返回;
/// 2. **工作目录**(env::current_dir())拼接;
/// 3. 若 2 不存在,**根目录**(laew 二进制所在目录,与 CLAUDE.md "根目录" 约定一致)拼接;
/// 4. 兜底返回 2 路径(让上层 stat 给出明确报错信息,而不是默默返回错路径)。
fn resolve_path(p: &str) -> PathBuf {
    resolve_path_with_candidates(p).into_path()
}

/// 路径解析的「候选路径」: 用于错误消息同时列出多个尝试路径。
///
/// 关联报告: 2026-09-09_05 E-002。
#[derive(Debug)]
pub(crate) enum ResolveCandidates {
    /// 仅一个候选路径(绝对路径且无根目录回退 / 工作目录直接命中)
    Single(PathBuf),
    /// 两个候选路径: 工作目录拼接 + 根目录拼接(均已尝试)
    WorkAndRoot { work: PathBuf, root: PathBuf },
}

impl ResolveCandidates {
    /// 取出最终选定的路径(用于实际 stat / read)。
    pub fn into_path(self) -> PathBuf {
        match self {
            ResolveCandidates::Single(p) => p,
            ResolveCandidates::WorkAndRoot { work, root } => {
                // 优先选实际存在的(供 caller 使用);理论上两者都不存在时返回 work
                if root.exists() { root } else { work }
            }
        }
    }
}

fn resolve_path_with_candidates(p: &str) -> ResolveCandidates {
    let path = Path::new(p);
    if path.is_absolute() {
        tracing::debug!(path = %path.display(), "resolve_path: 绝对路径");
        // 关联报告: 2026-09-09_04 D-003 —— 若 LLM 把「项目相对路径」误拼成「工作目录绝对路径」,
        // 而真实文件在根目录,做一次根目录替换重试。
        let work = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if let Ok(rel) = path.strip_prefix(&work) {
            // 该路径在工作目录下,但 stat 失败;尝试「根目录 + 相对路径」
            if let Ok(exe) = std::env::current_exe() {
                if let Some(root) = exe.parent() {
                    let root_path = root.join(rel);
                    if root_path.exists() && !path.exists() {
                        tracing::debug!(
                            orig = %path.display(),
                            tried = %root_path.display(),
                            "resolve_path 工作目录绝对路径 → 根目录回退"
                        );
                        return ResolveCandidates::WorkAndRoot {
                            work: path.to_path_buf(),
                            root: root_path,
                        };
                    }
                }
            }
        }
        return ResolveCandidates::Single(path.to_path_buf());
    }
    let work = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let work_path = work.join(path);
    let work_exists = work_path.exists();
    tracing::debug!(
        rel = %p,
        work = %work.display(),
        work_path = %work_path.display(),
        work_exists = %work_exists,
        "resolve_path 工作目录尝试"
    );
    if work_exists {
        return ResolveCandidates::Single(work_path);
    }
    // 根目录回退:从 current_exe() 父目录推导
    match std::env::current_exe() {
        Ok(exe) => {
            tracing::debug!(exe = %exe.display(), "resolve_path current_exe 成功");
            match exe.parent() {
                Some(root) => {
                    let root_path = root.join(path);
                    let root_exists = root_path.exists();
                    tracing::debug!(
                        rel = %p,
                        root = %root.display(),
                        root_path = %root_path.display(),
                        root_exists = %root_exists,
                        "resolve_path 根目录回退判断"
                    );
                    if root_exists {
                        return ResolveCandidates::Single(root_path);
                    }
                    // 关联报告: 2026-09-09_05 E-002 —— 工作目录与根目录均不存在,
                    // 返回两个候选路径以便错误消息同时列出。
                    return ResolveCandidates::WorkAndRoot {
                        work: work_path,
                        root: root_path,
                    };
                }
                None => {
                    tracing::debug!(exe = %exe.display(), "resolve_path current_exe 无父目录");
                }
            }
        }
        Err(e) => {
            tracing::debug!(err = %e, "resolve_path current_exe 失败");
        }
    }
    tracing::debug!(rel = %p, fallback = %work_path.display(), "resolve_path 兜底返回 work_path");
    ResolveCandidates::Single(work_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[tokio::test]
    async fn reads_with_line_numbers() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "alpha").unwrap();
        writeln!(f, "beta").unwrap();
        writeln!(f, "gamma").unwrap();
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
        assert!(out.contains("alpha"));
        assert!(out.contains("beta"));
        assert!(out.contains("gamma"));
        assert!(out.contains("total 3"));
    }

    #[tokio::test]
    async fn respects_offset_and_limit() {
        let mut f = NamedTempFile::new().unwrap();
        for i in 1..=5 {
            writeln!(f, "line{}", i).unwrap();
        }
        let p = f.path().to_str().unwrap().to_string();
        let out = ReadTool
            .execute(json!({"file_path": p, "offset": 2, "limit": 2}))
            .await
            .unwrap();
        assert!(out.contains("line2"));
        assert!(out.contains("line3"));
        assert!(!out.contains("line1"));
        assert!(!out.contains("line4"));
    }

    #[tokio::test]
    async fn missing_argument_errors() {
        let err = ReadTool.execute(json!({})).await.unwrap_err();
        assert!(matches!(err, AgentError::ToolExecution { .. }));
    }

    // ========== 错误信息双路径(第 05 轮 E-002,方案 tmpPlan/2026-09-09_05) ==========

    #[test]
    fn missing_rel_path_reports_work_and_root_attempts() {
        // 相对路径,工作目录与根目录拼接后都不存在 → 错误应同时显示两个尝试路径
        let work = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        // 构造一个绝对不会存在的相对路径
        let rel = "definitely_not_exists_e2e_test_12345/file.txt";
        let candidates = resolve_path_with_candidates(rel);
        match candidates {
            ResolveCandidates::WorkAndRoot { work: w, root: r } => {
                assert_eq!(w, work.join(rel), "工作目录路径应拼接正确");
                // 根路径由 current_exe().parent() 推导,在此仅校验非空
                assert!(!r.as_os_str().is_empty(), "根目录路径应存在");
            }
            other => panic!(
                "应返回 WorkAndRoot(两个路径均不存在),实际 {other:?}"
            ),
        }
    }

    #[test]
    fn single_resolve_when_work_dir_exists() {
        // 工作目录命中时,应返回 Single 而非 WorkAndRoot
        // 用绝对路径模拟「路径存在」(strip_prefix 不会命中,走 Single 分支)
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let p = tmp.path().to_str().unwrap();
        let candidates = resolve_path_with_candidates(p);
        match candidates {
            ResolveCandidates::Single(rp) => {
                assert_eq!(rp, std::path::PathBuf::from(p));
            }
            other => panic!("绝对路径应返回 Single,实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_error_message_contains_both_attempts_for_missing_rel_path() {
        // 验证 Read 工具在「相对路径均不存在」时,错误消息同时显示 work 与 root 路径
        let rel = "definitely_not_exists_e2e_test_12345/file.txt";
        let err = ReadTool
            .execute(json!({"file_path": rel}))
            .await
            .unwrap_err();
        match err {
            AgentError::ToolExecution { reason, .. } => {
                assert!(
                    reason.contains("[work]"),
                    "错误消息应标注 [work] 路径,实际: {reason}"
                );
                assert!(
                    reason.contains("[root]"),
                    "错误消息应标注 [root] 路径,实际: {reason}"
                );
            }
            other => panic!("预期 ToolExecution 错误,实际 {other:?}"),
        }
    }
}
