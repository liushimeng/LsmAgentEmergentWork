//! 斜杠命令补全引擎。
//!
//! 提供命令注册、前缀匹配、候选项生成等功能，
//! 供 input.rs 的自定义输入处理器使用。
//!
//! 2026-09-10(第 17 轮 D2):`SlashCommand` 字段 owned 化,支持动态注册自定义命令
//! (`reload_custom`,来源 `tui/commands.rs::discover`);补全列表内置在前、自定义在后,
//! 内置命令不可被自定义遮蔽。

use std::path::Path;

use super::commands;

/// 单个斜杠命令的定义（含别名、描述、用法）。
#[derive(Debug, Clone)]
pub struct SlashCommand {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub usage: String,
}

impl SlashCommand {
    fn builtin(name: &str, aliases: &[&str], description: &str, usage: &str) -> Self {
        Self {
            name: name.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            description: description.to_string(),
            usage: usage.to_string(),
        }
    }
}

/// 补全候选项。
#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// 显示文本（命令名）。
    pub display: String,
    /// 替换文本（不含 '/' 前缀，用于填入输入行）。
    pub replacement: String,
    /// 命令描述（灰色显示）。
    pub description: String,
    /// 用法提示。
    pub usage: String,
}

/// 补全引擎：根据输入前缀匹配命令。
pub struct CompletionEngine {
    /// 内置命令(静态注册)。
    builtin: Vec<SlashCommand>,
    /// 自定义命令(每行输入前经 `reload_custom` 刷新)。
    custom: Vec<SlashCommand>,
}

impl CompletionEngine {
    /// 创建默认补全引擎，注册所有内置斜杠命令。
    pub fn new() -> Self {
        let builtin = vec![
            SlashCommand::builtin("help", &["h", "?"], "显示帮助信息", "/help"),
            SlashCommand::builtin("exit", &["quit", "q"], "退出 TUI", "/exit"),
            SlashCommand::builtin("clear", &["c"], "清空对话历史并开启新会话", "/clear"),
            SlashCommand::builtin("new", &["n"], "开启新会话(同 /clear)", "/new"),
            SlashCommand::builtin("model", &[], "显示当前使用的模型", "/model"),
            SlashCommand::builtin("export", &[], "导出当前会话为 Markdown/JSON", "/export [path]"),
            SlashCommand::builtin("commands", &[], "列出可用的自定义斜杠命令", "/commands"),
            SlashCommand::builtin("provider", &["p"], "管理大模型接入记录", "/provider <sub>"),
            SlashCommand::builtin("provider list", &["provider ls"], "列出所有接入记录", "/provider list"),
            SlashCommand::builtin("provider add", &[], "交互式新增接入记录", "/provider add"),
            SlashCommand::builtin("provider use", &[], "切换当前模型", "/provider use <id>"),
            SlashCommand::builtin(
                "provider del",
                &["provider delete", "provider rm"],
                "删除接入记录",
                "/provider del <id>",
            ),
        ];
        Self { builtin, custom: Vec::new() }
    }

    /// 重扫自定义命令目录并替换补全快照(主循环每行输入前调用)。
    ///
    /// 内置命令不可遮蔽:与内置主名/别名同名的自定义命令跳过。
    pub fn reload_custom(&mut self, work_dir: &Path) {
        self.custom = commands::discover(work_dir)
            .into_iter()
            .filter(|c| !commands::is_builtin(&c.name))
            .filter(|c| {
                !self
                    .builtin
                    .iter()
                    .any(|b| b.aliases.iter().any(|a| a == &c.name))
            })
            .map(|c| SlashCommand {
                usage: if c.argument_hint.is_empty() {
                    format!("/{}", c.name)
                } else {
                    format!("/{} {}", c.name, c.argument_hint)
                },
                name: c.name,
                description: c.description,
                aliases: Vec::new(),
            })
            .collect();
    }

    /// 根据输入内容返回匹配的补全候选项。
    ///
    /// # 参数
    /// - `input`: 用户输入，可能包含 '/' 前缀，可能是子命令（如 "provider l"）
    ///
    /// # 返回
    /// 匹配的 `CompletionItem` 列表，内置在前、自定义在后。
    /// 每个候选项的 `replacement` 已包含 `/` 前缀,可直接替换输入缓冲区。
    pub fn complete(&self, input: &str) -> Vec<CompletionItem> {
        let raw = input;
        let input = input.trim().trim_start_matches('/').trim();
        // 空输入时所有命令匹配(starts_with("")),无需单独分支
        let collect = |cmds: &Vec<SlashCommand>| {
            cmds.iter()
                .filter(|cmd| command_matches(cmd, input))
                .map(|cmd| item_from_command(cmd, raw))
                .collect::<Vec<_>>()
        };
        // 内置在前、自定义在后
        let mut out = collect(&self.builtin);
        out.extend(collect(&self.custom));
        out
    }
}

/// 检查命令是否匹配输入前缀。
fn command_matches(cmd: &SlashCommand, input: &str) -> bool {
    // 主名匹配
    if cmd.name.starts_with(input) && cmd.name != input {
        return true;
    }
    // 别名匹配
    cmd.aliases.iter().any(|a| a.starts_with(input) && a.as_str() != input)
}

/// 从 SlashCommand 构造 CompletionItem。
/// `replacement` 保留原始输入中的 `/` 前缀(0 或 1 个),避免 Tab 接受后吞掉斜杠。
fn item_from_command(cmd: &SlashCommand, raw: &str) -> CompletionItem {
    // 统计用户原始输入开头的 '/' 数量,作为 replacement 的前缀
    let slash_prefix: String = raw.chars().take_while(|c| *c == '/').collect();
    let prefix = if slash_prefix.is_empty() { "/".to_string() } else { slash_prefix };
    CompletionItem {
        display: format!("/{}", cmd.name),
        replacement: format!("{}{} ", prefix, cmd.name),
        description: cmd.description.clone(),
        usage: cmd.usage.clone(),
    }
}

impl Default for CompletionEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_complete_empty_input_returns_all() {
        let engine = CompletionEngine::new();
        let items = engine.complete("");
        assert!(!items.is_empty());
        // 应包含 /help(replacement 保留 / 前缀与尾随空格)
        assert!(items.iter().any(|i| i.replacement == "/help "));
    }

    #[test]
    fn test_complete_slash_only_returns_all() {
        let engine = CompletionEngine::new();
        let items = engine.complete("/");
        assert!(!items.is_empty());
        // 用户已输过 '/',replacement 不再额外加 '/'
        assert!(items.iter().any(|i| i.replacement == "/help "));
    }

    #[test]
    fn test_complete_provider_prefix() {
        let engine = CompletionEngine::new();
        let items = engine.complete("/provider");
        // 应匹配 provider 子命令(replacement 含 / 前缀与尾随空格)
        assert!(items.iter().any(|i| i.replacement == "/provider list "));
        assert!(items.iter().any(|i| i.replacement == "/provider add "));
    }

    #[test]
    fn test_complete_pro_prefix() {
        let engine = CompletionEngine::new();
        let items = engine.complete("pro");
        // 用户未输 '/',replacement 自动补上
        assert!(items.iter().any(|i| i.replacement == "/provider "));
        assert!(items.iter().any(|i| i.replacement == "/provider list "));
    }

    #[test]
    fn test_complete_double_slash() {
        // 保留用户已输入的多个 '/'(防止 0.1.2 的 //help bug 复发)
        let engine = CompletionEngine::new();
        let items = engine.complete("//");
        assert!(items.iter().any(|i| i.replacement == "//help "));
    }

    #[test]
    fn test_complete_no_match() {
        let engine = CompletionEngine::new();
        let items = engine.complete("xyz");
        assert!(items.is_empty());
    }

    #[test]
    fn test_complete_backspace_scenario() {
        // 模拟退格场景：/provider lis → /provider li → /provider l → ...
        // 每次退格后补全引擎应正确返回匹配项
        let engine = CompletionEngine::new();

        // /provider lis → 匹配 "provider list"(以 "provider lis" 开头)
        let items = engine.complete("/provider lis");
        assert!(items.iter().any(|i| i.replacement == "/provider list "));

        // /provider li → 匹配 "provider list"（"ls" 不以 "li" 开头）
        let items = engine.complete("/provider li");
        assert!(items.iter().any(|i| i.replacement == "/provider list "));
        assert!(!items.iter().any(|i| i.replacement == "/provider ls "));

        // /provider l → 匹配 "provider list"("ls" 是别名,不出现在 replacement 中)
        let items = engine.complete("/provider l");
        assert!(items.iter().any(|i| i.replacement == "/provider list "));
        assert!(!items.iter().any(|i| i.replacement == "/provider ls "));

        // /provider  → 匹配所有 provider 子命令
        let items = engine.complete("/provider ");
        assert!(items.len() >= 4); // list, add, use, del

        // /provider → 匹配所有 provider 子命令
        let items = engine.complete("/provider");
        assert!(items.len() >= 4);

        // /provid → 匹配 provider
        let items = engine.complete("/provid");
        assert!(items.iter().any(|i| i.replacement == "/provider "));

        // /pro → 匹配 provider 及其子命令
        let items = engine.complete("/pro");
        assert!(items.iter().any(|i| i.replacement == "/provider "));

        // /pr → 匹配 provider 及其子命令
        let items = engine.complete("/pr");
        assert!(items.iter().any(|i| i.replacement == "/provider "));

        // /p → 匹配 provider 及其子命令
        let items = engine.complete("/p");
        assert!(items.iter().any(|i| i.replacement == "/provider "));
    }

    // ===== 自定义命令补全(D2,2026-09-10) =====

    #[test]
    fn test_custom_command_completion() {
        // TempDir 须在断言期间保持存活(Drop 即删除),故测试内联创建
        let tmp = tempfile::tempdir().expect("tmpdir");
        let dir = tmp.path().join(".laew").join("commands");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("review.md"),
            "---\ndescription: 代码评审\nargument-hint: [file]\n---\n评审 $ARGUMENTS",
        )
        .unwrap();
        // 与内置主名同名:应被跳过(不遮蔽)
        std::fs::write(dir.join("help.md"), "假 help").unwrap();
        // 与内置别名同名:应被跳过
        std::fs::write(dir.join("p.md"), "假 p").unwrap();

        let mut engine = CompletionEngine::new();
        engine.reload_custom(tmp.path());

        // 前缀命中自定义命令,带 argument-hint 生成 usage
        let items = engine.complete("/rev");
        assert!(items.iter().any(|i| i.replacement == "/review "));
        let hit = items.iter().find(|i| i.replacement == "/review ").unwrap();
        assert_eq!(hit.description, "代码评审");
        assert_eq!(hit.usage, "/review [file]");

        // 空输入:内置在前、自定义在后;内置 help 不被自定义遮蔽
        let items = engine.complete("");
        assert_eq!(
            items.iter().filter(|i| i.display == "/help").count(),
            1,
            "内置 /help 唯一"
        );
        assert!(items.iter().any(|i| i.display == "/review"));

        // 同名内置/别名遮蔽检查:自定义 help/p 不进入补全
        assert!(!items
            .iter()
            .any(|i| i.display == "/p" && i.description == "假 p"));
    }

    #[test]
    fn test_custom_command_without_hint() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let dir = tmp.path().join(".laew").join("commands");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("deploy.md"), "---\ndescription: 部署\n---\n部署模板").unwrap();
        let mut engine = CompletionEngine::new();
        engine.reload_custom(tmp.path());
        let items = engine.complete("/dep");
        let hit = items.iter().find(|i| i.replacement == "/deploy ").unwrap();
        assert_eq!(hit.usage, "/deploy");
    }
}
