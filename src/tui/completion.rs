//! 斜杠命令补全引擎。
//!
//! 提供命令注册、前缀匹配、候选项生成等功能，
//! 供 input.rs 的自定义输入处理器使用。

/// 单个斜杠命令的定义（含别名、描述、用法）。
#[derive(Debug, Clone)]
pub struct SlashCommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub usage: &'static str,
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
    commands: Vec<SlashCommand>,
}

impl CompletionEngine {
    /// 创建默认补全引擎，注册所有内置斜杠命令。
    pub fn new() -> Self {
        let commands = vec![
            SlashCommand {
                name: "help",
                aliases: &["h", "?"],
                description: "显示帮助信息",
                usage: "/help",
            },
            SlashCommand {
                name: "exit",
                aliases: &["quit", "q"],
                description: "退出 TUI",
                usage: "/exit",
            },
            SlashCommand {
                name: "clear",
                aliases: &["c"],
                description: "清空当前对话历史",
                usage: "/clear",
            },
            SlashCommand {
                name: "model",
                aliases: &[],
                description: "显示当前使用的模型",
                usage: "/model",
            },
            SlashCommand {
                name: "provider",
                aliases: &["p"],
                description: "管理大模型接入记录",
                usage: "/provider <sub>",
            },
            SlashCommand {
                name: "provider list",
                aliases: &["provider ls"],
                description: "列出所有接入记录",
                usage: "/provider list",
            },
            SlashCommand {
                name: "provider add",
                aliases: &[],
                description: "交互式新增接入记录",
                usage: "/provider add",
            },
            SlashCommand {
                name: "provider use",
                aliases: &[],
                description: "切换当前模型",
                usage: "/provider use <id>",
            },
            SlashCommand {
                name: "provider del",
                aliases: &["provider delete", "provider rm"],
                description: "删除接入记录",
                usage: "/provider del <id>",
            },
        ];
        Self { commands }
    }

    /// 根据输入内容返回匹配的补全候选项。
    ///
    /// # 参数
    /// - `input`: 用户输入，可能包含 '/' 前缀，可能是子命令（如 "provider l"）
    ///
    /// # 返回
    /// 匹配的 `CompletionItem` 列表，按命令定义顺序排列。
    pub fn complete(&self, input: &str) -> Vec<CompletionItem> {
        let input = input.trim().trim_start_matches('/').trim();
        if input.is_empty() {
            // 空输入：返回所有一级命令（去重，优先主名）
            return self.commands.iter().map(|cmd| self.item_from_command(cmd)).collect();
        }

        // 前缀匹配（包括别名）
        self.commands
            .iter()
            .filter(|cmd| self.command_matches(cmd, input))
            .map(|cmd| self.item_from_command(cmd))
            .collect()
    }

    /// 检查命令是否匹配输入前缀。
    fn command_matches(&self, cmd: &SlashCommand, input: &str) -> bool {
        // 主名匹配
        if cmd.name.starts_with(input) && cmd.name != input {
            return true;
        }
        // 别名匹配
        cmd.aliases.iter().any(|a| a.starts_with(input) && *a != input)
    }

    /// 从 SlashCommand 构造 CompletionItem。
    fn item_from_command(&self, cmd: &SlashCommand) -> CompletionItem {
        CompletionItem {
            display: format!("/{}", cmd.name),
            replacement: cmd.name.to_string(),
            description: cmd.description.to_string(),
            usage: cmd.usage.to_string(),
        }
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
        // 应包含 /help
        assert!(items.iter().any(|i| i.replacement == "help"));
    }

    #[test]
    fn test_complete_slash_only_returns_all() {
        let engine = CompletionEngine::new();
        let items = engine.complete("/");
        assert!(!items.is_empty());
    }

    #[test]
    fn test_complete_provider_prefix() {
        let engine = CompletionEngine::new();
        let items = engine.complete("/provider");
        // 应匹配 provider 子命令
        assert!(items.iter().any(|i| i.replacement == "provider list"));
        assert!(items.iter().any(|i| i.replacement == "provider add"));
    }

    #[test]
    fn test_complete_pro_prefix() {
        let engine = CompletionEngine::new();
        let items = engine.complete("pro");
        assert!(items.iter().any(|i| i.replacement == "provider"));
        assert!(items.iter().any(|i| i.replacement == "provider list"));
    }

    #[test]
    fn test_complete_no_match() {
        let engine = CompletionEngine::new();
        let items = engine.complete("xyz");
        assert!(items.is_empty());
    }
}
