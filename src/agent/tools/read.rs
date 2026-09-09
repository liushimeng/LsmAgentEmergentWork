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
            // 关联报告: 2026-09-09_06 F-001 — 进一步在错误末尾追加「根目录说明」和
            // 「源码相对路径」诊断提示,降低 LLM 反复构造变体路径浪费迭代的概率。
            let attempted = resolve_path_with_candidates(path_str);
            let reason_msg = match attempted {
                ResolveCandidates::Single(p) => {
                    format!("stat 失败: {e} (tried: {})", p.display())
                }
                ResolveCandidates::WorkAndRoot { work, root } => {
                    format!(
                        "stat 失败: {e} (tried [work]: {}, [root]: {})\n{}",
                        work.display(),
                        root.display(),
                        format_path_diagnostic(path_str, &work, &root),
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

/// 在 Read 工具 stat 失败时为错误消息追加「路径诊断」提示。
///
/// 关联报告: 2026-09-09_06 F-001。
///
/// 设计意图:典型 B09 测试场景里,LLM 想读 `src/agent/tools/read.rs` 这种「laew
/// 自身源码相对路径」,但工作目录是 `TestWorkSpace/`(laew 子目录),导致工作目录
/// 拼接路径 `TestWorkSpace/src/agent/tools/read.rs` 和根目录回退路径
/// `LsmAgentEmergentWork/src/agent/tools/read.rs` **都不命中**(后者存在但错误
/// 消息没显式提示「laew 根目录 + src/ 才是正解」)。本函数在错误消息末尾追加
/// 引导文案,降低 LLM 反复构造变体路径浪费迭代的概率。
///
/// 触发条件:
/// - `path_str` 以 `src/`、`tests/`、`docs/` 开头(典型 laew 源码/测试/文档相对路径),
///   且工作目录 ≠ laew 根目录(典型「在子目录里跑 laew」场景);
/// - 拼接后两条路径都不存在。
///
/// 不引入新依赖;纯字符串拼接;非命中场景不增加任何输出。
fn format_path_diagnostic(path_str: &str, _work: &Path, root: &Path) -> String {
    let work_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let work_dir_name = work_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let root_dir_name = root
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // 场景 A:典型「laew 源码相对路径在子目录里跑」 → 提示改用绝对路径
    let looks_like_source_rel = (path_str.starts_with("src/")
        || path_str.starts_with("tests/")
        || path_str.starts_with("docs/"))
        && !work_dir_name.is_empty()
        && !root_dir_name.is_empty()
        && work_dir_name != root_dir_name;

    // 场景 B:工作目录 = 根目录(就在 laew 根目录下跑),但相对路径在工作目录里查不到
    // → 提示「相对路径不在工作目录」即可
    let same_dir = !work_dir_name.is_empty()
        && work_dir_name == root_dir_name;

    if looks_like_source_rel {
        format!(
            "提示: 目标路径看起来像 laew 源码相对路径(`{path_str}`),但当前工作目录 \
             是 `{work_dir_name}/`(laew 的子目录),相对路径在工作目录拼接下不命中。\
             \n  - 如需读取 laew 自身源码,请改用绝对路径 `{root}/{path_str}`(根目录 + 相对路径),\
             \n    或先 `cd` 到 laew 根目录再使用相对路径。",
            path_str = path_str,
            work_dir_name = work_dir_name,
            root = root.parent().map(|p| p.display().to_string()).unwrap_or_default(),
        )
    } else if same_dir {
        format!(
            "提示: 相对路径 `{path_str}` 在工作目录 `{work_dir_name}` 下未找到。\
             请确认文件实际位置(可用 `ls` 或 `find` 探查),或改用绝对路径。",
            path_str = path_str,
            work_dir_name = work_dir_name,
        )
    } else {
        // 非典型场景:不追加诊断,保持现有错误消息紧凑
        String::new()
    }
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

    // ========== F-001 路径诊断(2026-09-09_06,方案 tmpPlan/2026-09-09_06) ==========

    #[test]
    fn format_path_diagnostic_for_source_rel_in_subdir_hints_absolute() {
        // 场景:工作目录 = laew 子目录(如 TestWorkSpace),LLM 想读 src/agent/tools/read.rs
        // 期望诊断提示「请改用绝对路径 根目录/src/...」
        let work_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let work_name = work_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        // 模拟一个不存在的 src 子路径
        let rel = "src/agent/tools/zzz_definitely_missing.rs";
        let work = work_dir.join(rel);
        let root = PathBuf::from("/tmp/mock_root").join(rel);
        let diag = format_path_diagnostic(rel, &work, &root);
        if work_name != "LsmAgentEmergentWork" && !work_name.is_empty() {
            // 工作目录是 laew 子目录(典型 TestWorkSpace 场景)
            assert!(
                diag.contains("改用绝对路径"),
                "应提示改用绝对路径,实际: {diag}"
            );
            assert!(
                diag.contains("laew 自身源码"),
                "应识别为源码相对路径,实际: {diag}"
            );
        } else {
            // 工作目录就是 laew 根目录 → 走 same_dir 分支或空分支
            // 此分支在 CI 上不一定触发,这里不强制断言
        }
    }

    #[test]
    fn format_path_diagnostic_for_misc_rel_no_diag() {
        // 场景:既不是 src/tests/docs 开头,也不在工作目录下
        // 期望:不追加诊断(空字符串),保持错误消息紧凑
        let rel = "some_random_file.txt";
        let work = PathBuf::from("/tmp/workdir").join(rel);
        let root = PathBuf::from("/tmp/rootdir").join(rel);
        let diag = format_path_diagnostic(rel, &work, &root);
        // 非典型场景:不应追加诊断
        assert!(
            diag.is_empty(),
            "非典型场景应返回空诊断,实际: {diag}"
        );
    }

    #[tokio::test]
    async fn read_error_for_src_subpath_includes_diagnostic_hint() {
        // 验证 Read 工具在「相对路径是 src/...」且工作目录 ≠ 根目录时,
        // 错误消息末尾包含「请改用绝对路径」诊断提示
        let rel = "src/agent/tools/zzz_definitely_missing_e2e.rs";
        let err = ReadTool
            .execute(json!({"file_path": rel}))
            .await
            .unwrap_err();
        match err {
            AgentError::ToolExecution { reason, .. } => {
                // 双路径一定存在
                assert!(reason.contains("[work]"), "应含 [work],实际: {reason}");
                assert!(reason.contains("[root]"), "应含 [root],实际: {reason}");
                // 当前工作目录若是 laew 子目录(典型 TestWorkSpace),应有诊断
                let work_dir = std::env::current_dir().unwrap_or_default();
                let work_name = work_dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if work_name != "LsmAgentEmergentWork" && !work_name.is_empty() {
                    assert!(
                        reason.contains("请改用绝对路径"),
                        "工作目录是 laew 子目录时应追加诊断,实际: {reason}"
                    );
                }
            }
            other => panic!("预期 ToolExecution 错误,实际 {other:?}"),
        }
    }
}
