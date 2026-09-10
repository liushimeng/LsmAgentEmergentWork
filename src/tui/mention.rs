//! D1 @提及实时路径补全后端(2026-09-10 第二十八轮,L1427 简化实现)。
//!
//! 光标位于 `@token` 内时,`FileSuggester` 基于工作目录的 walkdir 快照给出
//! 路径候选,复用 input.rs 的补全浮层渲染;Tab 接受(目录继续钻取/文件闭合)。
//!
//! 设计来源:claudecode `src/hooks/fileSuggestions.ts`(5s 节流 + 候选上限 15),
//! 见 docs/Agent源码调研/专题/专题-第十八轮-claudecode-深度分析.md §D1.2.6。
//! 简化:不读 .git/index mtime、不做 nucleo 模糊匹配,用「路径前缀 ∪ 文件名前缀」
//! 确定性匹配 —— CLI 单用户场景下 20k 条目快照的暴力过滤 < 5ms,足够流畅。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use walkdir::WalkDir;

use super::completion::CompletionItem;

/// 快照节流(claudecode REFRESH_THROTTLE_MS = 5s 同款)。
const REFRESH_THROTTLE: Duration = Duration::from_secs(5);
/// 快照遍历上限(防爆:巨型 monorepo 下 20000 条足够覆盖常用路径)。
const MAX_ENTRIES: usize = 20_000;
/// 遍历深度上限。
const MAX_DEPTH: usize = 6;
/// 浮层候选上限(claudecode MAX_SUGGESTIONS = 15 同款)。
const MAX_SUGGESTIONS: usize = 15;
/// 遍历跳过的目录名(隐藏目录由 is_hidden 统一拦截,此处列高频重目录)。
const SKIP_DIRS: &[&str] = &["target", "node_modules", ".git", "dist", "build"];

/// @ 提及文件补全器:懒加载快照 + 节流重建 + 前缀匹配。
pub struct FileSuggester {
    work_dir: PathBuf,
    /// 相对路径快照(目录带尾随 `/`,统一 `/` 分隔符)。
    cache: Vec<String>,
    built_at: Option<Instant>,
}

impl FileSuggester {
    pub fn new(work_dir: &Path) -> Self {
        Self {
            work_dir: work_dir.to_path_buf(),
            cache: Vec::new(),
            built_at: None,
        }
    }

    /// 快照过期(或尚未构建)时重建。遍历失败(权限等)保留旧快照静默降级。
    fn refresh_if_stale(&mut self) {
        if let Some(t) = self.built_at {
            if t.elapsed() < REFRESH_THROTTLE {
                return;
            }
        }
        let mut entries: Vec<String> = Vec::new();
        let walker = WalkDir::new(&self.work_dir)
            .max_depth(MAX_DEPTH)
            .into_iter()
            .filter_entry(should_keep);
        for entry in walker.flatten() {
            let path = entry.path();
            if path == self.work_dir {
                continue;
            }
            let Ok(rel) = path.strip_prefix(&self.work_dir) else {
                continue;
            };
            let mut s = rel.to_string_lossy().replace('\\', "/");
            if entry.file_type().is_dir() {
                s.push('/');
            }
            entries.push(s);
            if entries.len() >= MAX_ENTRIES {
                break;
            }
        }
        entries.sort();
        self.cache = entries;
        self.built_at = Some(Instant::now());
    }

    /// 给出 @fragment 的候选(fragment = `@` 之后、不含引号的已输入文本)。
    ///
    /// 匹配规则:
    /// - fragment 为空 → 顶层条目(深度 1);
    /// - fragment 含 `/` → 相对路径前缀匹配(钻取语义);
    /// - 否则 → 相对路径前缀 ∪ 文件名前缀(双通道,兼顾"记得文件名不记得目录")。
    ///
    /// 排序:目录优先、短路径优先;上限 15 条。
    pub fn suggest(&mut self, fragment: &str) -> Vec<CompletionItem> {
        self.refresh_if_stale();
        let frag = fragment.trim_start_matches('"');
        let frag_lower = frag.to_lowercase();
        let mut dirs: Vec<&String> = Vec::new();
        let mut files: Vec<&String> = Vec::new();
        for cand in &self.cache {
            let is_dir = cand.ends_with('/');
            let body = cand.trim_end_matches('/');
            let matched = if frag.is_empty() {
                // 顶层:相对路径不含 '/'
                !body.contains('/')
            } else if frag.contains('/') {
                body.to_lowercase().starts_with(&frag_lower)
            } else {
                let name = body.rsplit('/').next().unwrap_or(body).to_lowercase();
                body.to_lowercase().starts_with(&frag_lower) || name.starts_with(&frag_lower)
            };
            if !matched {
                continue;
            }
            if is_dir {
                dirs.push(cand);
            } else {
                files.push(cand);
            }
        }
        let by_len = |a: &&String, b: &&String| a.len().cmp(&b.len()).then(a.cmp(b));
        dirs.sort_by(by_len);
        files.sort_by(by_len);
        dirs.into_iter()
            .chain(files)
            .take(MAX_SUGGESTIONS)
            .map(|p| self.item_for(p))
            .collect()
    }

    /// 候选 → CompletionItem。replacement 是「拼回 token 的完整路径文本」:
    /// 含空白自动加引号;目录尾随 `/`(Tab 继续钻取),文件尾随空格(闭合)。
    ///
    /// D1 第二十八轮修订(2026-09-10):replacement **保留 `@` 前缀**。
    /// 旧实现 Tab 后去掉 `@`(变为 `src/main.rs `),导致提交到 Orchestrator
    /// 时 `expand_mentions` 看不到 `@`、附件块不展开。修复后:
    /// - Tab 接受:buffer 变为 `@src/main.rs `(可继续编辑,空格让下一次
    ///   字符走普通文本路径);
    /// - 提交到 dispatch_prompt 时仍含 `@`,触发附件展开(与 CLI `-p` 行为一致)。
    fn item_for(&self, rel: &str) -> CompletionItem {
        let is_dir = rel.ends_with('/');
        let body = rel.trim_end_matches('/');
        let quoted = body.contains(char::is_whitespace);
        let replacement = if is_dir {
            // 目录:尾随 `/` 继续钻取,引号形态包整段路径
            if quoted {
                format!("@\"{body}/\"")
            } else {
                format!("@{body}/")
            }
        } else if quoted {
            // 文件(含空白):完整引号 + 尾随空格闭合
            format!("@\"{body}\" ")
        } else {
            // 文件(普通):@ 前缀 + 尾随空格闭合
            format!("@{body} ")
        };
        let description = if is_dir {
            "目录".to_string()
        } else {
            match fs::metadata(self.work_dir.join(body)) {
                Ok(m) => format!("文件 {}", human_size(m.len())),
                Err(_) => "文件".to_string(),
            }
        };
        CompletionItem {
            display: format!("@{rel}"),
            replacement,
            description,
            usage: String::new(),
        }
    }
}

/// 遍历剪枝:隐藏目录(.xxx)与 SKIP_DIRS 不进入(但工作目录本身放行)。
/// filter_entry 语义:返回 true 才保留并继续下钻。
fn should_keep(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    if entry.file_type().is_dir() {
        if SKIP_DIRS.contains(&name.as_ref()) || (name.starts_with('.') && name != ".") {
            return false;
        }
    }
    true
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

/// 从 buffer 与光标位置提取「光标前的 @token」。
///
/// 返回 (token 起点字节偏移, fragment):
/// - 裸形态:向前扫到空白为止,token 以 `@` 开头且 `@` 前是行首/空白;
/// - 引号形态:`@"` 未闭合 → fragment 可含空白(token 起点 = `@` 处);
/// - 其它 → None(非 @ 补全场景)。
pub fn mention_token_at(buffer: &str, cursor: usize) -> Option<(usize, String)> {
    let head = &buffer[..cursor];
    // 引号形态优先:找最后一个 `@"`,且其后无闭合 `"`
    if let Some(at) = head.rfind("@\"") {
        let before_ok = at == 0 || head[..at].ends_with(char::is_whitespace);
        let after = &head[at + 2..];
        if before_ok && !after.contains('"') {
            return Some((at, after.to_string()));
        }
    }
    // 裸形态:光标前最后一个空白之后的 token
    let start = head
        .rfind(char::is_whitespace)
        .map(|i| i + 1)
        .unwrap_or(0);
    let token = &head[start..];
    // 注意:token 自身以 @ 开头,链式吞噬检查必须看 @ 之后的部分
    if token.starts_with('@') && token.len() > 1 && !token[1..].contains('@') {
        return Some((start, token[1..].to_string()));
    }
    // 刚输入 `@`(token == "@")也激活,给出顶层候选
    if token == "@" {
        return Some((start, String::new()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn build_tree() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("src/agent")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::create_dir_all(root.join("my docs")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main(){}").unwrap();
        fs::write(root.join("src/agent/mod.rs"), "// mod").unwrap();
        fs::write(root.join("README.md"), "# readme").unwrap();
        fs::write(root.join("target/debug/x.o"), "obj").unwrap();
        fs::write(root.join(".hidden/secret.txt"), "s").unwrap();
        fs::write(root.join("my docs/note.md"), "n").unwrap();
        tmp
    }

    #[test]
    fn empty_fragment_lists_top_level() {
        let tmp = build_tree();
        let mut s = FileSuggester::new(tmp.path());
        let items = s.suggest("");
        let texts: Vec<&str> = items.iter().map(|i| i.display.as_str()).collect();
        assert!(texts.contains(&"@src/"));
        assert!(texts.contains(&"@docs/"));
        assert!(texts.contains(&"@README.md"));
        assert!(!texts.iter().any(|t| t.contains("target")));
        assert!(!texts.iter().any(|t| t.contains("hidden")));
        // 顶层不递归:src/main.rs 不应出现
        assert!(!texts.contains(&"@src/main.rs"));
    }

    #[test]
    fn path_prefix_drills_into_directory() {
        let tmp = build_tree();
        let mut s = FileSuggester::new(tmp.path());
        let items = s.suggest("src/");
        assert!(items.iter().any(|i| i.display == "@src/main.rs"));
        assert!(items.iter().any(|i| i.display == "@src/agent/"));
        assert!(!items.iter().any(|i| i.display == "@README.md"));
    }

    #[test]
    fn filename_prefix_matches_across_dirs() {
        let tmp = build_tree();
        let mut s = FileSuggester::new(tmp.path());
        let items = s.suggest("mod");
        assert!(items.iter().any(|i| i.display == "@src/agent/mod.rs"));
    }

    #[test]
    fn directory_replacement_keeps_trailing_slash() {
        let tmp = build_tree();
        let mut s = FileSuggester::new(tmp.path());
        let items = s.suggest("src/a");
        let dir = items.iter().find(|i| i.display == "@src/agent/").unwrap();
        assert_eq!(dir.replacement, "@src/agent/");
        assert_eq!(dir.description, "目录");
    }

    #[test]
    fn file_replacement_closes_with_space() {
        let tmp = build_tree();
        let mut s = FileSuggester::new(tmp.path());
        let items = s.suggest("README");
        let f = items.iter().find(|i| i.display == "@README.md").unwrap();
        assert_eq!(f.replacement, "@README.md ");
        assert!(f.description.starts_with("文件"));
    }

    #[test]
    fn whitespace_path_is_quoted() {
        let tmp = build_tree();
        let mut s = FileSuggester::new(tmp.path());
        let items = s.suggest("my docs/");
        let f = items
            .iter()
            .find(|i| i.display == "@my docs/note.md")
            .unwrap();
        assert_eq!(f.replacement, "@\"my docs/note.md\" ");
    }

    #[test]
    fn file_replacement_keeps_at_prefix() {
        // 第二十八轮 P0 回归钉子:replacement 必须含 @ 前缀,
        // 否则提交到 dispatch_prompt 后 expand_mentions 看不到 @,附件块不展开。
        let tmp = build_tree();
        let mut s = FileSuggester::new(tmp.path());
        let items = s.suggest("README");
        let f = items.iter().find(|i| i.display == "@README.md").unwrap();
        assert!(f.replacement.starts_with('@'), "@ prefix must be preserved");
    }

    #[test]
    fn dir_replacement_keeps_at_prefix_for_drilling() {
        let tmp = build_tree();
        let mut s = FileSuggester::new(tmp.path());
        let items = s.suggest("src/");
        let dir = items.iter().find(|i| i.display == "@src/agent/").unwrap();
        assert!(dir.replacement.starts_with('@'), "@ prefix must be preserved for dirs");
        assert!(dir.replacement.ends_with('/'), "trailing slash for drilling");
    }

    #[test]
    fn suggestions_capped_at_15() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..30 {
            fs::write(tmp.path().join(format!("file{i:02}.txt")), "x").unwrap();
        }
        let mut s = FileSuggester::new(tmp.path());
        assert_eq!(s.suggest("file").len(), MAX_SUGGESTIONS);
    }

    // ---------- mention_token_at ----------

    #[test]
    fn token_at_bare() {
        let buf = "总结 @src/ma";
        let (start, frag) = mention_token_at(buf, buf.len()).unwrap();
        assert_eq!((start, frag.as_str()), (7, "src/ma"));
    }

    #[test]
    fn token_at_just_at_sign() {
        let (start, frag) = mention_token_at("@", 1).unwrap();
        assert_eq!((start, frag.as_str()), (0, ""));
    }

    #[test]
    fn token_at_quoted_unclosed() {
        let buf = "看 @\"my docs/no";
        let (start, frag) = mention_token_at(buf, buf.len()).unwrap();
        assert_eq!((start, frag.as_str()), (4, "my docs/no"));
    }

    #[test]
    fn token_at_quoted_closed_is_none() {
        let buf = "看 @\"a.txt\" 后";
        assert!(mention_token_at(buf, buf.len()).is_none());
    }

    #[test]
    fn token_at_email_is_none() {
        assert!(mention_token_at("user@host", 9).is_none());
    }

    #[test]
    fn token_at_plain_text_is_none() {
        assert!(mention_token_at("hello world", 11).is_none());
    }

    #[test]
    fn token_at_mid_token_cursor() {
        // 光标在 token 中间:只取光标前部分
        let (start, frag) = mention_token_at("@src/main.rs", 7).unwrap();
        assert_eq!((start, frag.as_str()), (0, "src/ma"));
    }
}
