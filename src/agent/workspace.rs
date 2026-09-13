//! 工作区感知(Workspace Awareness) —— D4 落地。
//!
//! 设计见 `tmpPlan/2026-09-13_01-工作区感知与运行时环境注入方案.md`,知识库出处:
//! `docs/Agent源码调研/专题/专题-第十八轮深挖合集.md` §维度4(文件监视与工作区感知)。
//!
//! 采集维度的取舍遵循知识库横向对比的「懒刷新派」结论(atomcode 主动放弃 hot watcher、
//! deepseek-harness 用事件驱动 invalidate):**不做文件监听**(不引入 notify),
//! 改为「按需采集 + 进程级 TTL 缓存」,三类消费方共用同一份快照:
//!
//! 1. [`hint_block`] —— 单/多行 brief,追加到 8 角色每次 LLM 调用的 system 末尾,
//!    让**执行层**(SubAgent-Work)首次拿到工程类型 / 工具链 / git 状态 / 平台 / 日期;
//! 2. [`render_section`] —— 多行 Markdown 段,并入 `<<<LAEW:PROJECT_CONTEXT>>>`
//!    会话级注入消息(Yolo 入口层);
//! 3. [`change_marker`] —— 任务执行前后的轻量变更标记,TUI 打印「变更 N → M」。
//!
//! 所有外部命令走 [`run_with_timeout`](stdout 读取线程 + 轮询 + 到期 kill),
//! git 缺失 / 超时 / 非仓库一律静默降级,绝不阻塞交互。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use walkdir::WalkDir;

/// 运行时 brief 注入标记(幂等探测锚点,风格对齐 `LAEW:RUNTIME_HINTS`)。
pub const HINT_MARKER_START: &str = "<<<LAEW:WORKSPACE>>>";
/// 运行时 brief 结束标记。
pub const HINT_MARKER_END: &str = "<<<END>>>";

/// git 子进程超时(极度宽松,仅用于兜底极端仓库 / 慢速文件系统)。
const GIT_TIMEOUT: Duration = Duration::from_secs(2);
/// 会话级注入段字符上限(超出截断并注明)。
const MAX_SECTION_CHARS: usize = 1500;
/// 最近提交条数。
const RECENT_COMMIT_LIMIT: usize = 3;
/// 提交标题截断长度(CJK 按字符)。
const COMMIT_SUBJECT_CHARS: usize = 60;
/// 顶层目录展示上限。
const TOP_DIR_LIMIT: usize = 12;
/// 顶层文件展示上限。
const TOP_FILE_LIMIT: usize = 6;
/// 「最近改动文件」时间窗(6 小时)。
const RECENT_WINDOW_SECS: u64 = 6 * 3600;
/// 「最近改动文件」展示上限。
const RECENT_FILE_LIMIT: usize = 5;
/// 扫描条目硬上限(防巨型工作区拖慢交互)。
const SCAN_ENTRY_CAP: usize = 4000;
/// 变更标记携带的文件路径上限(TUI 前后对比用)。
const DELTA_FILE_CAP: usize = 200;
/// 默认 TTL(秒);`LAEW_WORKSPACE_TTL_SECS=0` 表示每次重算。
const DEFAULT_TTL_SECS: u64 = 5;

/// 扫描时跳过的目录名(构建产物 / 依赖 / 版本库元数据)。
const IGNORED_DIRS: [&str; 16] = [
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".nuxt",
    "vendor",
    ".idea",
    ".vscode",
    ".cache",
    ".tox",
];

/// 工程类型探测表:(名称, 标记文件, 构建命令, 测试命令)。
const PROJECT_KINDS: [(&str, &[&str], &str, &str); 11] = [
    ("Rust", &["Cargo.toml"], "cargo build", "cargo test"),
    (
        "Python",
        &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"],
        "python -m pip install -e .",
        "pytest",
    ),
    ("Node", &["package.json"], "npm run build", "npm test"),
    ("Go", &["go.mod"], "go build ./...", "go test ./..."),
    ("Java/Maven", &["pom.xml"], "mvn -q package", "mvn -q test"),
    (
        "Java/Gradle",
        &["build.gradle", "build.gradle.kts"],
        "./gradlew build",
        "./gradlew test",
    ),
    ("Make", &["Makefile"], "make", "make test"),
    (
        "CMake",
        &["CMakeLists.txt"],
        "cmake -S . -B build",
        "ctest --test-dir build",
    ),
    ("Ruby", &["Gemfile"], "bundle install", "bundle exec rspec"),
    ("PHP", &["composer.json"], "composer install", "composer test"),
    (
        "容器",
        &["Dockerfile", "docker-compose.yml", "compose.yaml"],
        "docker build .",
        "docker compose up",
    ),
];

/// 命中的工程类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectKind {
    /// 展示名(如 `Rust`)
    pub name: &'static str,
    /// 命中的标记文件
    pub marker: String,
    /// 建议构建命令
    pub build: &'static str,
    /// 建议测试命令
    pub test: &'static str,
}

/// 最近改动文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentFile {
    /// 相对工作目录的路径
    pub path: String,
    /// mtime 距今秒数
    pub age_secs: u64,
}

impl RecentFile {
    /// 人类可读时间(刚刚 / N 分钟前 / N 小时前)。
    pub fn age_text(&self) -> String {
        format_age(self.age_secs)
    }
}

/// 工作区一次性快照(纯数据,可跨线程共享)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    /// 工作目录绝对路径
    pub work_dir: PathBuf,
    /// 平台标识(如 `linux/x86_64`)
    pub platform: String,
    /// 本地日期 `YYYY-MM-DD`
    pub today: String,
    /// 是否为 git 仓库(git 可用且命令成功)
    pub is_git: bool,
    /// 当前分支(detached 时为 `HEAD (分离)`)
    pub branch: Option<String>,
    /// 短 HEAD hash
    pub head: Option<String>,
    /// 暂存区变更数
    pub staged: usize,
    /// 工作区(未暂存)修改数
    pub modified: usize,
    /// 未跟踪文件数
    pub untracked: usize,
    /// 最近提交(短 hash + 标题)
    pub recent_commits: Vec<String>,
    /// 命中的工程类型(可多命中,如 Rust + 容器)
    pub projects: Vec<ProjectKind>,
    /// 顶层子目录名
    pub dirs: Vec<String>,
    /// 顶层文件总数
    pub file_count: usize,
    /// 顶层代表性文件名
    pub notable_files: Vec<String>,
    /// 最近改动的文件(6 小时内,按 mtime 倒序)
    pub recent_files: Vec<RecentFile>,
}

impl WorkspaceSnapshot {
    /// 未提交变更总数。
    pub fn dirty(&self) -> usize {
        self.staged + self.modified + self.untracked
    }

    /// 空目录判定:非 git 仓库、无工程标记、无任何顶层条目。
    ///
    /// 用于保持既有「无内容不注入」语义 —— 空目录依然不产生上下文。
    pub fn is_trivial(&self) -> bool {
        !self.is_git && self.projects.is_empty() && self.dirs.is_empty() && self.file_count == 0
    }

    /// 主工程类型(Rust / Node / …;未识别返回 `None`)。
    pub fn primary_project(&self) -> Option<&ProjectKind> {
        self.projects.first()
    }

    /// 工程类型短标签(无工程标记时返回 `通用`)。
    pub fn project_label(&self) -> String {
        if self.projects.is_empty() {
            "通用".to_string()
        } else {
            self.projects
                .iter()
                .map(|p| p.name)
                .collect::<Vec<_>>()
                .join("+")
        }
    }

    /// 单/多行运行时 brief(供 system 末尾注入)。
    ///
    /// 形如:
    /// ```text
    /// 工作区: /path | 工程: Rust | git: main @48e5bc5 · 3 未提交
    /// 建议命令: 构建 `cargo build` / 测试 `cargo test` | 平台: linux/x86_64 | 日期: 2026-09-13
    /// ```
    pub fn brief(&self) -> String {
        let mut line1 = format!("工作区: {} | 工程: {}", self.work_dir.display(), self.project_label());
        if self.is_git {
            let branch = self.branch.as_deref().unwrap_or("?");
            let head = self
                .head
                .as_deref()
                .map(|h| format!(" @{h}"))
                .unwrap_or_default();
            let dirty = self.dirty();
            let dirty_text = if dirty == 0 {
                "工作区干净".to_string()
            } else {
                format!("{dirty} 未提交")
            };
            line1.push_str(&format!(" | git: {branch}{head} · {dirty_text}"));
        }
        let mut line2 = String::new();
        if let Some(p) = self.primary_project() {
            line2.push_str(&format!(
                "建议命令: 构建 `{}` / 测试 `{}` | ",
                p.build, p.test
            ));
        }
        line2.push_str(&format!("平台: {} | 日期: {}", self.platform, self.today));
        format!("{line1}\n{line2}")
    }

    /// 多行 Markdown 段(供会话级 PROJECT_CONTEXT 注入),超长截断。
    pub fn render_section(&self) -> String {
        let mut out = String::new();
        out.push_str("### 工作区快照(系统自动采集)\n");
        out.push_str(&format!("- 工作目录: {}\n", self.work_dir.display()));
        out.push_str(&format!("- 平台 / 日期: {} / {}\n", self.platform, self.today));
        if self.projects.is_empty() {
            out.push_str("- 工程类型: 未识别(无 Cargo.toml / package.json / pyproject.toml 等标记文件)\n");
        } else {
            for p in &self.projects {
                out.push_str(&format!(
                    "- 工程类型: {}(标记 {};构建 `{}` / 测试 `{}`)\n",
                    p.name, p.marker, p.build, p.test
                ));
            }
        }
        if self.is_git {
            // 无提交的空仓库没有 HEAD,此时不显示 `@-`
            let head = self
                .head
                .as_deref()
                .map(|h| format!(" @{h}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "- Git: 分支 {}{} · 未提交 {} 个(暂存 {} / 修改 {} / 未跟踪 {})\n",
                self.branch.as_deref().unwrap_or("?"),
                head,
                self.dirty(),
                self.staged,
                self.modified,
                self.untracked
            ));
            if !self.recent_commits.is_empty() {
                out.push_str("- 最近提交:\n");
                for c in &self.recent_commits {
                    out.push_str(&format!("  - {c}\n"));
                }
            }
        } else {
            out.push_str("- Git: 非 git 仓库\n");
        }
        if !self.dirs.is_empty() || self.file_count > 0 {
            out.push_str(&format!(
                "- 顶层结构: {}(共 {} 个文件)\n",
                if self.dirs.is_empty() {
                    "-".to_string()
                } else {
                    self.dirs.join(" / ")
                },
                self.file_count
            ));
            if !self.notable_files.is_empty() {
                out.push_str(&format!("- 顶层文件: {}\n", self.notable_files.join(", ")));
            }
        }
        if !self.recent_files.is_empty() {
            out.push_str("- 最近改动(6 小时内):\n");
            for f in &self.recent_files {
                out.push_str(&format!("  - {} ({})\n", f.path, f.age_text()));
            }
        }
        out.push_str("- 说明: 以上为系统自动采集的工作区状态,可能略滞后于磁盘;涉及精确状态请用工具核对。\n");
        truncate_chars(out, MAX_SECTION_CHARS)
    }
}

/// 采集工作区快照。任何子步骤失败都静默降级,不 panic、不阻塞。
pub fn snapshot(work_dir: &Path) -> WorkspaceSnapshot {
    let mut snap = WorkspaceSnapshot {
        work_dir: work_dir.to_path_buf(),
        platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        today: today_local(),
        is_git: false,
        branch: None,
        head: None,
        staged: 0,
        modified: 0,
        untracked: 0,
        recent_commits: Vec::new(),
        projects: detect_projects(work_dir),
        dirs: Vec::new(),
        file_count: 0,
        notable_files: Vec::new(),
        recent_files: Vec::new(),
    };

    collect_git(&mut snap, work_dir);
    collect_top_entries(&mut snap, work_dir);
    snap.recent_files = collect_recent_files(work_dir);
    snap
}

/// 任务前后的轻量变更标记(git status + 分支,单次子进程)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDelta {
    /// 是否 git 仓库
    pub is_git: bool,
    /// 当前分支
    pub branch: Option<String>,
    /// 未提交变更总数
    pub dirty: usize,
    /// 变更文件路径(相对工作目录,上限 [`DELTA_FILE_CAP`])
    pub files: Vec<String>,
}

/// 采集变更标记(TUI 任务前后对比用)。非 git 目录返回 `is_git=false` 的空标记。
pub fn change_marker(work_dir: &Path) -> WorkspaceDelta {
    let mut delta = WorkspaceDelta {
        is_git: false,
        branch: None,
        dirty: 0,
        files: Vec::new(),
    };
    let Some(out) = run_with_timeout(
        "git",
        &["status", "--porcelain=v1", "-b"],
        work_dir,
        GIT_TIMEOUT,
    ) else {
        return delta;
    };
    let (branch, staged, modified, untracked, files) = parse_status(&out);
    delta.is_git = true;
    delta.branch = branch;
    delta.dirty = staged + modified + untracked;
    delta.files = files;
    delta
}

// ===================== 进程级 TTL 缓存 =====================

struct CachedSnapshot {
    at: Instant,
    dir: PathBuf,
    snap: Arc<WorkspaceSnapshot>,
}

static CACHE: OnceLock<Mutex<Option<CachedSnapshot>>> = OnceLock::new();

fn ttl() -> Duration {
    let secs = std::env::var("LAEW_WORKSPACE_TTL_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_TTL_SECS);
    Duration::from_secs(secs)
}

/// 取当前工作目录的缓存快照(TTL 内复用,避免每次 LLM 调用都跑 git)。
///
/// 工作目录不可用(Paths::detect 失败)时返回 `None`。
pub fn cached_snapshot() -> Option<Arc<WorkspaceSnapshot>> {
    let dir = crate::agent::project_context::current_work_dir()?;
    let cell = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cell.lock() {
        if let Some(c) = guard.as_ref() {
            if c.dir == dir && c.at.elapsed() < ttl() {
                return Some(c.snap.clone());
            }
        }
    }
    let snap = Arc::new(snapshot(dir));
    if let Ok(mut guard) = cell.lock() {
        *guard = Some(CachedSnapshot {
            at: Instant::now(),
            dir: dir.to_path_buf(),
            snap: snap.clone(),
        });
    }
    Some(snap)
}

/// 强制失效缓存(下一次访问重新采集)。
pub fn invalidate() {
    if let Some(cell) = CACHE.get() {
        if let Ok(mut guard) = cell.lock() {
            *guard = None;
        }
    }
}

/// 运行时 brief 注入块(8 角色每次 LLM 调用拼到 system 末尾)。
///
/// 空目录 / 缓存不可用时返回空串(零开销,不改变既有 system 语义)。
pub fn hint_block() -> String {
    let Some(snap) = cached_snapshot() else {
        return String::new();
    };
    if snap.is_trivial() {
        return String::new();
    }
    format!(
        "\n\n{HINT_MARKER_START}\n\
         [系统注入·运行环境快照](非用户输入,可能略滞后于磁盘)\n\
         {}\n\
         {HINT_MARKER_END}",
        snap.brief()
    )
}

// ===================== 采集实现 =====================

/// git 采集:分支 / HEAD / 变更计数 / 最近提交。git 不可用或非仓库时保持默认值。
fn collect_git(snap: &mut WorkspaceSnapshot, work_dir: &Path) {
    let status = run_with_timeout(
        "git",
        &["status", "--porcelain=v1", "-b"],
        work_dir,
        GIT_TIMEOUT,
    );
    let log = run_with_timeout(
        "git",
        &["log", "-3", "--format=%h %s"],
        work_dir,
        GIT_TIMEOUT,
    );
    let Some(status_out) = status else {
        return;
    };
    // 非仓库时 git status 退出码非 0(run_with_timeout 已过滤为 None);
    // 仓库内该命令必然输出 `## <branch>` 头,输出为空一律视为不可用。
    if status_out.trim().is_empty() {
        return;
    }
    let (branch, staged, modified, untracked, _files) = parse_status(&status_out);
    snap.is_git = true;
    snap.branch = branch;
    snap.staged = staged;
    snap.modified = modified;
    snap.untracked = untracked;

    if let Some(log_out) = log {
        for line in log_out.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let mut it = line.splitn(2, ' ');
            let hash = it.next().unwrap_or("").to_string();
            let subject = truncate_chars(it.next().unwrap_or("").to_string(), COMMIT_SUBJECT_CHARS);
            if snap.head.is_none() {
                snap.head = Some(hash.clone());
            }
            snap.recent_commits.push(format!("{hash} {subject}"));
            if snap.recent_commits.len() >= RECENT_COMMIT_LIMIT {
                break;
            }
        }
    }
    if snap.head.is_none() {
        snap.head = run_with_timeout(
            "git",
            &["rev-parse", "--short", "HEAD"],
            work_dir,
            GIT_TIMEOUT,
        )
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    }
}

/// 解析 `git status --porcelain=v1 -b` 输出。
///
/// 返回:(分支, 暂存数, 修改数, 未跟踪数, 变更路径列表)。
fn parse_status(out: &str) -> (Option<String>, usize, usize, usize, Vec<String>) {
    let mut branch = None;
    let (mut staged, mut modified, mut untracked) = (0usize, 0usize, 0usize);
    let mut files = Vec::new();
    for (i, raw) in out.lines().enumerate() {
        let line = raw.trim_end_matches(['\r', '\n']);
        if i == 0 && line.starts_with("## ") {
            branch = Some(parse_branch(&line[3..]));
            continue;
        }
        if line.len() < 3 {
            continue;
        }
        let bytes = line.as_bytes();
        let (x, y) = (bytes[0] as char, bytes[1] as char);
        let path = line[3..].trim().to_string();
        if x == '?' && y == '?' {
            untracked += 1;
        } else {
            if x != ' ' && x != '?' {
                staged += 1;
            }
            if y != ' ' {
                modified += 1;
            }
        }
        if files.len() < DELTA_FILE_CAP && !path.is_empty() {
            files.push(path);
        }
    }
    (branch, staged, modified, untracked, files)
}

/// 从 `## ` 行提取分支名。
fn parse_branch(s: &str) -> String {
    let s = s.trim();
    if s.starts_with("No commits yet on ") {
        return s.trim_start_matches("No commits yet on ").trim().to_string();
    }
    if s.starts_with("HEAD (no branch)") {
        return "HEAD (分离)".to_string();
    }
    // `main...origin/main [ahead 1]` / `main`
    let head = s.split("...").next().unwrap_or(s);
    let head = head.split_whitespace().next().unwrap_or(head);
    head.trim().to_string()
}

/// 顶层结构采集(不递归):目录名 + 文件计数 + 代表性文件名。
fn collect_top_entries(snap: &mut WorkspaceSnapshot, work_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(work_dir) else {
        return;
    };
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            dirs.push(format!("{name}/"));
        } else {
            files.push(name);
        }
    }
    dirs.sort();
    files.sort();
    snap.file_count = files.len();
    dirs.truncate(TOP_DIR_LIMIT);
    snap.dirs = dirs;
    files.truncate(TOP_FILE_LIMIT);
    snap.notable_files = files;
}

/// 最近改动文件采集:depth ≤ 2,忽略构建/依赖目录,条目上限 [`SCAN_ENTRY_CAP`]。
fn collect_recent_files(work_dir: &Path) -> Vec<RecentFile> {
    let now = SystemTime::now();
    let mut found: Vec<(SystemTime, RecentFile)> = Vec::new();
    let mut scanned = 0usize;

    let walker = WalkDir::new(work_dir)
        .max_depth(2)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_dir() && IGNORED_DIRS.contains(&name.as_ref()))
        });

    for entry in walker {
        scanned += 1;
        if scanned > SCAN_ENTRY_CAP {
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        let Ok(age) = now.duration_since(mtime) else {
            continue;
        };
        let secs = age.as_secs();
        if secs > RECENT_WINDOW_SECS {
            continue;
        }
        let path = entry
            .path()
            .strip_prefix(work_dir)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .into_owned();
        found.push((
            mtime,
            RecentFile {
                path,
                age_secs: secs,
            },
        ));
    }

    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.truncate(RECENT_FILE_LIMIT);
    found.into_iter().map(|(_, f)| f).collect()
}

/// 工程类型探测(标记文件存在于顶层即命中)。
fn detect_projects(work_dir: &Path) -> Vec<ProjectKind> {
    let mut out = Vec::new();
    for (name, markers, build, test) in PROJECT_KINDS.iter() {
        for m in markers.iter() {
            if work_dir.join(m).exists() {
                out.push(ProjectKind {
                    name,
                    marker: (*m).to_string(),
                    build,
                    test,
                });
                break;
            }
        }
    }
    out
}

// ===================== 工具函数 =====================

/// 带超时执行外部命令,返回 stdout 文本(**仅退出码为 0 时**)。
///
/// 失败语义与 [`run_capture`] 一致:启动失败 / 超时 → `None`;
/// 非零退出码 → `None`(调用方据此判定「不是 git 仓库」等)。
pub fn run_with_timeout(
    program: &str,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> Option<String> {
    let (ok, out) = run_capture(program, args, cwd, timeout)?;
    if ok {
        Some(out)
    } else {
        None
    }
}

/// 带超时执行外部命令,返回 `(是否成功, stdout)`。
///
/// std 无 `wait_timeout`,且「先 try_wait 再读管道」会因管道写满而死锁,
/// 故用独立线程持续排空 stdout,主线程轮询子进程状态,到期 kill。
/// 超时 / 启动失败一律返回 `None`(调用方静默降级)。
pub fn run_capture(
    program: &str,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> Option<(bool, String)> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    // 超时:杀进程后放弃读取线程(句柄 drop 即 detach,
                    // 避免孙进程仍持有管道时 join 永久阻塞)。
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let buf = reader.join().ok()?;
    Some((success, String::from_utf8_lossy(&buf).into_owned()))
}

/// 本地日期 `YYYY-MM-DD`(本地时区不可用时退化为 UTC)。
pub fn today_local() -> String {
    let now = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

fn format_age(secs: u64) -> String {
    if secs < 60 {
        "刚刚".to_string()
    } else if secs < 3600 {
        format!("{} 分钟前", secs / 60)
    } else {
        format!("{} 小时前", secs / 3600)
    }
}

/// 按字符截断(超出追加标注)。
fn truncate_chars(s: String, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s;
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{head}\n…(已截断,完整内容请用工具查看)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// git 是否可用(不可用时跳过 git 相关断言)。
    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("git 应可执行");
        assert!(out.status.success(), "git {args:?} 失败: {out:?}");
    }

    #[test]
    fn detects_rust_project_and_recommends_cargo() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
        let snap = snapshot(dir.path());
        let p = snap.primary_project().expect("应识别 Rust");
        assert_eq!(p.name, "Rust");
        assert_eq!(p.marker, "Cargo.toml");
        assert!(p.build.contains("cargo build"));
        assert!(p.test.contains("cargo test"));
        assert!(snap.brief().contains("cargo test"), "brief 应含测试命令");
        assert!(!snap.is_trivial());
    }

    #[test]
    fn detects_python_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]\n").unwrap();
        let snap = snapshot(dir.path());
        assert_eq!(snap.project_label(), "Python");
        assert!(snap.render_section().contains("pytest"));
    }

    #[test]
    fn empty_dir_is_trivial() {
        let dir = tempfile::tempdir().unwrap();
        let snap = snapshot(dir.path());
        assert!(snap.is_trivial(), "空目录应视为 trivial");
        assert!(!snap.render_section().is_empty(), "渲染仍可用");
        assert_eq!(snap.dirty(), 0);
    }

    #[test]
    fn dir_with_files_is_not_trivial() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "x").unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        let snap = snapshot(dir.path());
        assert!(!snap.is_trivial());
        assert_eq!(snap.file_count, 1);
        assert_eq!(snap.dirs, vec!["src/".to_string()]);
        assert_eq!(snap.notable_files, vec!["a.txt".to_string()]);
    }

    #[test]
    fn git_repo_branch_and_dirty_counts() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        fs::write(dir.path().join("tracked.txt"), "v1").unwrap();
        git(dir.path(), &["add", "tracked.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "初始提交"]);

        let clean = snapshot(dir.path());
        assert!(clean.is_git);
        assert_eq!(clean.dirty(), 0, "干净仓库不应有变更");
        assert_eq!(clean.recent_commits.len(), 1);
        assert!(clean.recent_commits[0].contains("初始提交"));
        assert!(clean.head.is_some());
        assert!(clean.brief().contains("工作区干净"));

        // 改一个 + 加一个未跟踪
        fs::write(dir.path().join("tracked.txt"), "v2").unwrap();
        fs::write(dir.path().join("new.txt"), "n").unwrap();
        let dirty = snapshot(dir.path());
        assert_eq!(dirty.modified, 1);
        assert_eq!(dirty.untracked, 1);
        assert_eq!(dirty.dirty(), 2);
        assert!(dirty.brief().contains("2 未提交"));
        assert!(dirty.render_section().contains("未跟踪 1"));

        let delta = change_marker(dir.path());
        assert!(delta.is_git);
        assert_eq!(delta.dirty, 2);
        assert_eq!(delta.files.len(), 2);
        assert!(delta.branch.is_some());
    }

    #[test]
    fn change_marker_on_non_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let delta = change_marker(dir.path());
        assert!(!delta.is_git);
        assert_eq!(delta.dirty, 0);
    }

    #[test]
    fn parse_branch_variants() {
        assert_eq!(parse_branch("main...origin/main [ahead 1]"), "main");
        assert_eq!(parse_branch("feature/x"), "feature/x");
        assert_eq!(parse_branch("HEAD (no branch)"), "HEAD (分离)");
        assert_eq!(parse_branch("No commits yet on main"), "main");
    }

    #[test]
    fn parse_status_classifies_and_lists_files() {
        // 注意:不能用行尾 `\` 续行(Rust 会吞掉下一行前导空格,
        // 让「工作区修改」被误判为「已暂存」),逐行显式拼接。
        let out = [
            "## main...origin/main",
            "M  staged.rs",
            " M modified.rs",
            "?? new.rs",
            "MM both.rs",
            "",
        ]
        .join("\n");
        let (branch, staged, modified, untracked, files) = parse_status(&out);
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(staged, 2, "M_ + MM 计入暂存");
        assert_eq!(modified, 2, "_M + MM 计入修改");
        assert_eq!(untracked, 1);
        assert_eq!(files.len(), 4);
        assert!(files.contains(&"new.rs".to_string()));
    }

    #[test]
    fn recent_files_tracked_and_ignored_dirs_skipped() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("target")).unwrap();
        fs::write(dir.path().join("target/ignored.rs"), "x").unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/fresh.rs"), "y").unwrap();

        let snap = snapshot(dir.path());
        let paths: Vec<&str> = snap.recent_files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.contains(&"src/fresh.rs"),
            "应含新写入文件,实际 {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.starts_with("target/")),
            "target/ 应被忽略,实际 {paths:?}"
        );
        assert!(!snap.recent_files[0].age_text().is_empty());
    }

    #[test]
    fn recent_files_beyond_depth_and_window_excluded() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        fs::write(dir.path().join("a/b/c/deep.txt"), "x").unwrap();
        let snap = snapshot(dir.path());
        assert!(
            snap.recent_files.iter().all(|f| f.path != "a/b/c/deep.txt"),
            "depth>2 不应被采集"
        );
    }

    #[test]
    fn format_age_buckets() {
        assert_eq!(format_age(5), "刚刚");
        assert_eq!(format_age(120), "2 分钟前");
        assert_eq!(format_age(7200), "2 小时前");
    }

    #[test]
    fn run_with_timeout_kills_on_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let out = run_with_timeout("sh", &["-c", "sleep 5"], dir.path(), Duration::from_millis(150));
        assert!(out.is_none(), "超时应返回 None");
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "不应等待子进程自然结束,实际 {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn run_with_timeout_drains_large_output() {
        let dir = tempfile::tempdir().unwrap();
        // 200KB 输出远超管道缓冲(默认 64KB),验证读取线程防死锁
        let out = run_with_timeout(
            "sh",
            &["-c", "head -c 200000 /dev/zero | tr '\\0' 'a'"],
            dir.path(),
            Duration::from_secs(5),
        );
        let out = out.expect("不应超时");
        assert_eq!(out.len(), 200000);
    }

    #[test]
    fn run_with_timeout_missing_program_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_with_timeout(
            "definitely-not-a-real-binary-laew",
            &[],
            dir.path(),
            Duration::from_millis(200),
        );
        assert!(out.is_none());
    }

    #[test]
    fn brief_and_section_include_platform_and_date() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("README.md"), "x").unwrap();
        let snap = snapshot(dir.path());
        assert!(snap.brief().contains(&snap.platform));
        assert!(snap.brief().contains(&snap.today));
        let section = snap.render_section();
        assert!(section.contains("工作区快照"));
        assert!(section.contains(&snap.work_dir.display().to_string()));
    }

    #[test]
    fn section_truncated_at_limit() {
        // 直接构造快照(不依赖当前工作目录的 git 状态,保证并行测试下确定性)
        let dir = tempfile::tempdir().unwrap();
        let mut snap = snapshot(dir.path());
        snap.is_git = true;
        snap.branch = Some("main".to_string());
        snap.head = Some("abcdef1".to_string());
        for i in 0..200 {
            snap.recent_commits.push(format!("{i:07} 很长的提交标题用于触发截断保护"));
        }
        let section = snap.render_section();
        assert!(section.contains("已截断"), "超限应截断并注明");
        assert!(section.chars().count() < MAX_SECTION_CHARS + 120);
    }

    #[test]
    fn hint_block_uses_cached_snapshot_for_work_dir() {
        // hint_block 依赖全局 work_dir(进程级 OnceLock,由 Paths::detect 决定),
        // 这里只验证「不 panic 且格式自洽」:非 empty 时必须包裹标记。
        let block = hint_block();
        if !block.is_empty() {
            assert!(block.contains(HINT_MARKER_START));
            assert!(block.contains(HINT_MARKER_END));
            assert!(block.contains("工作区: "));
        }
    }
}
