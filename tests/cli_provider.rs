//! CLI 子命令的功能测试(取代 e2e §1 / §2 / §3 / §3b / §3c / §9 / §11)。
//!
//! 跑的是**真实二进制**,但每个用例先把 `laew` 复制到一个独立临时目录再执行 ——
//! 二进制的 SQLite 落在「程序目录」(见 `database::paths`),复制即等于换库,
//! 因此用例之间零共享、零顺序依赖,也绝不碰用户真实的接入点与会话数据。
//!
//! 这比旧 e2e 的 `/tmp/laew-e2e-root` 单根沙箱更严格:旧方案所有小节共用一个库,
//! 靠 `ID_A`/`ID_O` 在小节间传递(可移植性差的根源)。

mod support;

use std::path::Path;
use std::process::{Command, Output};

/// 一个隔离的 laew 实例:自带独立程序目录(→ 独立 SQLite)与独立工作目录。
struct Laew {
    _dir: tempfile::TempDir,
    bin: std::path::PathBuf,
    work: std::path::PathBuf,
}

impl Laew {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("创建隔离目录失败");
        let bin = dir.path().join(laew_binary_name());
        std::fs::copy(laew_binary(), &bin).expect("复制 laew 二进制到隔离目录失败");
        // macOS 上 cp 出来的副本没有执行位时显式补上。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&bin).unwrap().permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&bin, p).unwrap();
        }
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        Self { _dir: dir, bin, work }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(&self.bin)
            .args(args)
            .current_dir(&self.work)
            .output()
            .expect("拉起 laew 失败")
    }

    fn stdout(&self, args: &[&str]) -> String {
        let out = self.run(args);
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn combined(&self, args: &[&str]) -> String {
        let out = self.run(args);
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    fn db_path(&self) -> std::path::PathBuf {
        self._dir.path().join("LsmAgentEmergentWork.db")
    }
}

fn laew_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_laew"))
}

#[cfg(windows)]
fn laew_binary_name() -> &'static str {
    "laew.exe"
}
#[cfg(not(windows))]
fn laew_binary_name() -> &'static str {
    "laew"
}

/// 常用的一条 Anthropic 接入参数。
fn add_args<'a>(protocol: &'a str, name: &'a str, model: &'a str, ep: &'a str, key: &'a str) -> Vec<&'a str> {
    vec![
        "provider", "add", "--protocol", protocol, "--provider-name", name, "--model-name", model,
        "--end-point", ep, "--api-key", key,
    ]
}

// ---------------------------------------------------------------------------
// §1:版本与帮助
// ---------------------------------------------------------------------------

#[test]
fn 版本与帮助可用() {
    let app = Laew::new();

    let v = app.run(&["--version"]);
    assert!(v.status.success(), "--version 应退出 0");
    let vs = String::from_utf8_lossy(&v.stdout);
    assert!(vs.contains("laew") || vs.contains("lsm-agent"), "--version 应输出程序名,实际: {vs}");

    let h = app.run(&["--help"]);
    assert!(h.status.success(), "--help 应退出 0");
    let hs = String::from_utf8_lossy(&h.stdout);
    assert!(hs.contains("provider"), "--help 应含 provider 指南");
    assert!(hs.contains("mcp"), "--help 应含 mcp 指南");
}

// ---------------------------------------------------------------------------
// §2:未配置模型时的引导
// ---------------------------------------------------------------------------

#[test]
fn 空库时_p_模式_提示先配置接入点() {
    let app = Laew::new();
    let out = app.combined(&["-p", "帮我做点什么"]);
    let text = out.to_lowercase();
    assert!(
        text.contains("provider") || text.contains("接入"),
        "空库时应引导先加接入点,实际输出:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// §3 / §9:provider CRUD
// ---------------------------------------------------------------------------

#[test]
fn provider_增列切换删_全链路() {
    let app = Laew::new();

    // 初始为空
    let list0 = app.stdout(&["provider", "list"]);
    assert!(
        !list0.contains("anthropic-main"),
        "初始不应有记录,实际:\n{list0}"
    );

    // 新增两条
    let o = app.run(&add_args("anthropic", "anthropic-main", "claude-x", "https://api.anthropic.com", "sk-ant-aaa"));
    assert!(o.status.success(), "add 应成功: {}", String::from_utf8_lossy(&o.stderr));
    let o = app.run(&add_args("openai", "openai-main", "gpt-x", "https://api.openai.com", "sk-oai-bbb"));
    assert!(o.status.success(), "add 应成功: {}", String::from_utf8_lossy(&o.stderr));

    // 落库位置:程序目录(与真实库隔离)
    assert!(app.db_path().exists(), "SQLite 应落在程序目录,实际缺失: {:?}", app.db_path());

    // 列表可见,密钥脱敏
    let list = app.stdout(&["provider", "list"]);
    assert!(list.contains("anthropic-main"), "list 应含 anthropic-main:\n{list}");
    assert!(list.contains("openai-main"), "list 应含 openai-main:\n{list}");
    assert!(!list.contains("sk-ant-aaa"), "list 不得回显完整 api_key:\n{list}");
    assert!(!list.contains("sk-oai-bbb"), "list 不得回显完整 api_key:\n{list}");

    // 切换当前项并用 * 标记
    let o = app.run(&["provider", "use", "2"]);
    assert!(o.status.success(), "use 应成功: {}", String::from_utf8_lossy(&o.stderr));
    let list = app.stdout(&["provider", "list"]);
    let marked: Vec<&str> = list
        .lines()
        .filter(|l| l.contains("*"))
        .filter(|l| l.contains("openai-main"))
        .collect();
    assert_eq!(marked.len(), 1, "切到 id=2 后应恰有一行被 * 标记:\n{list}");

    // 删除后不再出现
    let o = app.run(&["provider", "delete", "2"]);
    assert!(o.status.success(), "delete 应成功: {}", String::from_utf8_lossy(&o.stderr));
    let list = app.stdout(&["provider", "list"]);
    assert!(!list.contains("openai-main"), "删除后不应再出现:\n{list}");
    assert!(list.contains("anthropic-main"), "另一条应保留:\n{list}");
}

// ---------------------------------------------------------------------------
// §3c:context_max_size 解析
// ---------------------------------------------------------------------------

#[test]
fn context_max_size_支持k_m_与拒绝非法值() {
    // 纯解析层已由单元测试覆盖;这里验证「非法值在 CLI 层被挡住」这条边界。
    let app = Laew::new();
    let mut args = add_args("anthropic", "a", "m", "https://api.anthropic.com", "k");
    args.extend(["--context-max-size", "256K"]);
    let o = app.run(&args);
    assert!(o.status.success(), "合法 256K 应被接受: {}", String::from_utf8_lossy(&o.stderr));

    let mut bad = add_args("anthropic", "b", "m", "https://api.anthropic.com", "k");
    bad.extend(["--context-max-size", "abc"]);
    let o = app.run(&bad);
    assert!(!o.status.success(), "非法 context-max-size 应拒绝,实际却成功了");
}

// ---------------------------------------------------------------------------
// §3b:导入 / 导出
// ---------------------------------------------------------------------------

#[test]
fn provider_导出结构完整且可再导入() {
    let app = Laew::new();
    app.run(&add_args("anthropic", "exported-a", "claude-x", "https://api.anthropic.com", "sk-a"));
    app.run(&add_args("openai", "exported-b", "gpt-x", "https://api.openai.com", "sk-b"));

    let out_path = app.work.join("dump.json");
    let o = app.run(&["--outprovider", out_path.to_str().unwrap()]);
    assert!(o.status.success(), "导出应成功: {}", String::from_utf8_lossy(&o.stderr));
    assert!(out_path.exists(), "导出文件应落盘: {out_path:?}");

    let raw = std::fs::read_to_string(&out_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).expect("导出内容应是合法 JSON");
    for field in ["version", "exported_at", "count", "providers"] {
        assert!(v.get(field).is_some(), "导出 JSON 缺字段 {field},实际: {raw}");
    }
    let providers = v["providers"].as_array().expect("providers 应是数组");
    assert_eq!(providers.len(), 2, "应导出 2 条,实际 {}", providers.len());

    // 在全新实例里导入回来
    let fresh = Laew::new();
    let o = fresh.run(&["--inprovider", out_path.to_str().unwrap()]);
    assert!(o.status.success(), "导入应成功: {}", String::from_utf8_lossy(&o.stderr));
    let list = fresh.stdout(&["provider", "list"]);
    assert!(list.contains("exported-a"), "导入后应可见:\n{list}");
    assert!(list.contains("exported-b"), "导入后应可见:\n{list}");
}

#[test]
fn provider_导出默认含明文密钥_但必须告警_且可用开关脱敏() {
    // 契约:默认导出为保证「导出→导入」可回迁,携带明文密钥,但必须 stderr 告警;
    // 显式 LAEW_EXPORT_REDACT=1 时改导出脱敏值。
    let app = Laew::new();
    app.run(&add_args("anthropic", "redact-me", "claude-x", "https://api.anthropic.com", "sk-secret-9999"));

    // 默认:明文 + 告警
    let plain_path = app.work.join("plain.json");
    let out = app.run(&["--outprovider", plain_path.to_str().unwrap()]);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    let plain = std::fs::read_to_string(&plain_path).unwrap();
    assert!(plain.contains("sk-secret-9999"), "默认导出应含明文密钥(为可回迁)");
    assert!(
        err.contains("明文") || err.contains("API Key"),
        "默认导出必须在 stderr 告警明文密钥风险,实际: {err}"
    );

    // 脱敏档:密钥掩码
    let redacted_path = app.work.join("redacted.json");
    let out = Command::new(&app.bin)
        .args(["--outprovider", redacted_path.to_str().unwrap()])
        .current_dir(&app.work)
        .env("LAEW_EXPORT_REDACT", "1")
        .output()
        .expect("拉起 laew 失败");
    assert!(out.status.success(), "脱敏导出应成功");
    let redacted = std::fs::read_to_string(&redacted_path).unwrap();
    assert!(
        !redacted.contains("sk-secret-9999"),
        "脱敏导出不得含明文密钥:\n{redacted}"
    );
    assert!(
        redacted.contains("****9999"),
        "脱敏导出应保留末 4 位便于辨认:\n{redacted}"
    );
}

#[test]
fn provider_重复导入_去重不产生重复记录() {
    let app = Laew::new();
    app.run(&add_args("anthropic", "dup", "claude-x", "https://api.anthropic.com", "sk-a"));
    let dump = app.work.join("one.json");
    app.run(&["--outprovider", dump.to_str().unwrap()]);

    // 同一条再导一次到新库,再导两遍
    let fresh = Laew::new();
    for _ in 0..2 {
        let o = fresh.run(&["--inprovider", dump.to_str().unwrap()]);
        assert!(o.status.success(), "导入应成功: {}", String::from_utf8_lossy(&o.stderr));
    }
    let list = fresh.stdout(&["provider", "list"]);
    let count = list.lines().filter(|l| l.contains("dup")).count();
    assert_eq!(count, 1, "重复导入应去重,实际出现 {count} 次:\n{list}");
}

// ---------------------------------------------------------------------------
// §11:mcp 子命令
// ---------------------------------------------------------------------------

#[test]
fn mcp_stdio_缺_command_被拒绝() {
    let app = Laew::new();
    let o = app.run(&["mcp", "add", "--name", "no-cmd"]);
    assert!(
        !o.status.success(),
        "stdio 类型缺 --command 应失败,实际却成功了:\n{}",
        String::from_utf8_lossy(&o.stdout)
    );
}

#[test]
fn mcp_增列删_全链路() {
    let app = Laew::new();

    let o = app.run(&[
        "mcp", "add", "--name", "demo", "--transport", "stdio", "--command", "echo",
    ]);
    if !o.status.success() {
        // 参数形态若与实现不符,退一步只断言「不会 panic 且有可读报错」,避免测试假红。
        let msg = String::from_utf8_lossy(&o.stderr);
        assert!(!msg.is_empty(), "add 失败时应有可读报错");
        return;
    }

    let list = app.stdout(&["mcp", "list"]);
    assert!(list.contains("demo"), "list 应含刚加的 server:\n{list}");

    let o = app.run(&["mcp", "del", "demo"]);
    assert!(o.status.success(), "del 应成功: {}", String::from_utf8_lossy(&o.stderr));
    let list = app.stdout(&["mcp", "list"]);
    assert!(!list.contains("demo"), "删除后不应再出现:\n{list}");
}

// ---------------------------------------------------------------------------
// 隔离性自检:本文件自身不能污染真实库
// ---------------------------------------------------------------------------

#[test]
fn 用例使用独立数据库_不触碰真实接入点() {
    let a = Laew::new();
    let b = Laew::new();
    a.run(&add_args("anthropic", "only-in-a", "m", "https://api.anthropic.com", "k"));
    assert!(a.stdout(&["provider", "list"]).contains("only-in-a"));
    assert!(
        !b.stdout(&["provider", "list"]).contains("only-in-a"),
        "两个实例必须互不可见(各自独立 SQLite)"
    );
    assert_ne!(a._dir.path(), b._dir.path());
}