//! 项目上下文注入的五级发现链(取代 e2e §5b 的 19 条断言)。
//!
//! 这条链的每级都依赖真实文件系统布局,所以放在集成测试里用真目录跑,
//! 而不是在单测里 mock 掉 `read_dir` —— 探测顺序一旦写错,mock 是测不出来的。
//!
//! 链路:非空 `CLAUDE.md` → 非空 `AGENTS.md` → 非空 `README.md`
//!      →(都没有但有其它 *.md)程序化分析后自动生成 `README.md`
//!      → 全空则不注入。

use lsm_agent::agent::project_context;
use lsm_agent::session::Session;

fn write(dir: &std::path::Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).unwrap();
}

// ---------------------------------------------------------------------------
// 优先级:CLAUDE.md > AGENTS.md > README.md
// ---------------------------------------------------------------------------

#[test]
fn claude_md_优先于其它说明文件() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "CLAUDE.md", "# Claude 说明\nCLAUDE_BODY");
    write(d.path(), "AGENTS.md", "AGENTS_BODY");
    write(d.path(), "README.md", "README_BODY");

    let ctx = project_context::load(d.path());
    let text = ctx.content;
    assert!(text.contains("CLAUDE_BODY"), "应命中 CLAUDE.md,实际:\n{text}");
    assert!(!text.contains("AGENTS_BODY"), "AGENTS.md 不应被同时注入");
    assert!(!text.contains("README_BODY"), "README.md 不应被同时注入");
}

#[test]
fn 无claude时降级到_agents_md() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "AGENTS.md", "AGENTS_BODY");
    write(d.path(), "README.md", "README_BODY");

    let text = project_context::load(d.path()).content;
    assert!(text.contains("AGENTS_BODY"), "应降级命中 AGENTS.md,实际:\n{text}");
    assert!(!text.contains("README_BODY"), "README.md 不应被同时注入");
}

#[test]
fn 只有readme时命中readme() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "README.md", "README_BODY");

    let text = project_context::load(d.path()).content;
    assert!(text.contains("README_BODY"), "应命中 README.md,实际:\n{text}");
}

// ---------------------------------------------------------------------------
// 空文件不算命中(否则空 CLAUDE.md 会把项目说明整个吞掉)
// ---------------------------------------------------------------------------

#[test]
fn 空说明文件_不阻断降级到下一级() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "CLAUDE.md", ""); // 空文件
    write(d.path(), "README.md", "README_BODY");

    let text = project_context::load(d.path()).content;
    assert!(
        text.contains("README_BODY"),
        "空 CLAUDE.md 不应视为命中,应继续降级到 README.md,实际:\n{text}"
    );
}

#[test]
fn 全空目录_不注入任何内容() {
    let d = tempfile::tempdir().unwrap();
    let ctx = project_context::load(d.path());
    assert!(
        ctx.content.trim().is_empty(),
        "无任何说明文件时不应注入内容,实际:\n{}",
        ctx.content
    );
    assert!(
        project_context::build_message(&ctx).is_none(),
        "无内容时不应构造出注入消息"
    );
}

// ---------------------------------------------------------------------------
// 自动生成 README.md(第四级)
// ---------------------------------------------------------------------------

#[test]
fn 无说明文件但有其它md_自动生成readme() {
    let d = tempfile::tempdir().unwrap();
    // 没有任何 *README*/*CLAUDE*/*AGENTS*,但有其它 md 文件 → 触发程序化分析
    write(d.path(), "design-notes.md", "# 设计说明\n这是本项目的架构说明。");

    let ctx = project_context::load(d.path());
    let text = ctx.content;
    assert!(
        !text.trim().is_empty(),
        "存在其它 md 时应自动生成说明,实际为空"
    );
    let generated = d.path().join("README.md");
    assert!(generated.exists(), "应把生成的 README.md 落盘: {generated:?}");
    assert!(
        std::fs::read_to_string(&generated).unwrap().contains("design-notes"),
        "生成内容应基于既有 md 文件"
    );
}

// ---------------------------------------------------------------------------
// 注入幂等:同一 Session 只注入一次,且不污染用户消息
// ---------------------------------------------------------------------------

#[test]
fn 注入幂等_重复调用只生效一次() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "CLAUDE.md", "CLAUDE_BODY");

    let mut session = Session::new();
    assert!(project_context::inject_once(&mut session, d.path()), "首次应注入");
    let after_first = session.context().len();

    // 再调两次都不应重复注入
    assert!(!project_context::inject_once(&mut session, d.path()), "第二次不应重复注入");
    assert!(!project_context::inject_once(&mut session, d.path()), "第三次不应重复注入");

    assert_eq!(
        session.context().len(),
        after_first,
        "重复调用不得让上下文增长"
    );
    assert!(project_context::is_injected(session.context()), "应能探测到已注入标记");
}

#[test]
fn 注入内容带隔离标记_不与用户提示词混淆() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "CLAUDE.md", "CLAUDE_BODY_MARKER");

    let mut session = Session::new();
    project_context::inject_once(&mut session, d.path());

    // 项目上下文必须是**独立的 user 消息**,不能和用户提示词拼成一条
    let first = &session.context()[0];
    let text = first
        .content
        .iter()
        .filter_map(|b| match b {
            lsm_agent::llm::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    assert!(
        text.contains("<<<LAEW:PROJECT_CONTEXT>>>"),
        "注入块应带 PROJECT_CONTEXT 标记,实际:\n{text}"
    );
    assert!(text.contains("CLAUDE_BODY_MARKER"), "注入块应含说明文件正文");
}

#[test]
fn 无内容时不注入_上下文保持为空() {
    let d = tempfile::tempdir().unwrap();
    let mut session = Session::new();
    assert!(
        !project_context::inject_once(&mut session, d.path()),
        "空目录不应触发注入"
    );
    assert!(session.context().is_empty(), "不应留下空消息");
}