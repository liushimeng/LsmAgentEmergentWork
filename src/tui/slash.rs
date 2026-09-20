//! 斜杠命令处理(自 tui/mod.rs 拆分,2026-09-11,单文件 ≤1800 行规范)。
//!
//! 职责:`handle_slash` 内置命令路由(help / exit / clear / new / model / provider /
//! export / commands / diff / theme / rewind / undo / fork / branches / switch)
//! + 自定义命令 fallback,以及各命令的 run_* 处理器。

use anyhow::Result;

use super::atty;
use super::branches::BranchStore;
use super::commands;
use super::export;
use super::export::{outcome_from_store_str, TranscriptEntry};
use super::format::{first_line_preview, merge_usage, print_help, suggest_similar_commands};
use super::pathfmt;
use super::TuiSession;
use crate::database::chat_store::{
    resolve_chat_session, ChatTurnRow, ResumeResolution, SessionSummary,
};
use crate::llm::{ChatMessage, ContentBlock, Usage};
use crate::session::Session;

impl TuiSession {
    /// 处理斜杠命令。`cmd` 为去掉 `/` 前缀的命令串;`full_line` 为用户原始输入行
    /// (自定义命令 dispatch 时作为 transcript 的 raw_input,保留 `/cmd args` 原文)。
    pub(crate) async fn handle_slash(&mut self, cmd: &str, full_line: &str) -> Result<bool> {
        let mut it = cmd.split_whitespace();
        let head = it.next().unwrap_or("");
        // 头部之后的剩余参数(保留原始空白语义,自定义命令 /export 路径均使用)
        let rest_args = cmd[head.len()..].trim();
        match head {
            "help" | "h" | "?" => {
                print_help();
            }
            "exit" | "quit" | "q" => {
                println!("  再见。");
                return Ok(true);
            }
            "clear" | "c" => {
                let saved = self.reset_session();
                // TUI 主屏下真正清屏:滚动区清空(ANSI 100 行上滚) + 重新打印 banner。
                // 修复前只 print 一行 Session ID,旧对话历史与提示词残留在视觉上不被清除,
                // 用户体感「清屏没生效」。
                if atty() {
                    print!("\x1b[2J\x1b[H");
                    self.print_banner();
                } else {
                    println!(
                        "  已清空对话历史并开启新会话, Session ID: {}",
                        self.session.id
                    );
                }
                if let Some(name) = saved {
                    println!("  (已自动保存分支 {name},可用 /switch {name} 找回)");
                }
            }
            "new" | "n" => {
                let saved = self.reset_session();
                if atty() {
                    print!("\x1b[2J\x1b[H");
                    self.print_banner();
                } else {
                    println!("  已开启新会话, Session ID: {}", self.session.id);
                }
                if let Some(name) = saved {
                    println!("  (已自动保存分支 {name},可用 /switch {name} 找回)");
                }
            }
            "model" => {
                if let Some(r) = self
                    .db
                    .lock()
                    .expect("db")
                    .get_active_or_env()
                    .map_err(anyhow::Error::from)?
                {
                    println!(
                        "  [{}] {} / {}  @ {}",
                        r.protocol.as_str(),
                        r.provider_name,
                        r.model_name,
                        r.end_point
                    );
                } else {
                    // 第 71 轮:空态补操作指引,与横幅/dispatch 守门文案一致
                    println!("  当前未配置模型。请先执行 /provider add 添加接入记录。");
                }
            }
            "provider" | "p" => {
                let sub = it.next().unwrap_or("");
                match sub {
                    "list" | "ls" => {
                        // 进入 ProviderList 屏
                        self.run_provider_list_screen().await?;
                    }
                    "add" => {
                        // 进入 ProviderForm 屏(add 模式)
                        self.run_provider_add_screen().await?;
                    }
                    "use" => {
                        let id_str = it.next().unwrap_or("");
                        if let Ok(id) = id_str.parse::<i64>() {
                            self.switch_provider(id)?;
                            println!("  ✓ 已切换到 id={id}");
                        } else {
                            println!("  用法: /provider use <id>");
                        }
                    }
                    "del" | "delete" | "rm" => {
                        // 进入 ProviderDelPicker 屏
                        self.run_provider_del_screen().await?;
                    }
                    "" => {
                        // 单独 /provider 默认路由到 list
                        self.run_provider_list_screen().await?;
                    }
                    other => println!("  未知 /provider 子命令: {other}"),
                }
            }
            // D8 会话导出:默认落工作目录 Markdown,显式路径按后缀定格式
            "export" => {
                self.run_export(rest_args);
            }
            // D2 自定义命令自观测:列出已加载命令与来源
            "commands" => {
                self.print_custom_commands();
            }
            "diff" => {
                // /diff <old_file> <new_file>:并排 diff 两个文件(行级+字符级着色)
                let parts: Vec<&str> = rest_args.split_whitespace().collect();
                if parts.len() < 2 {
                    println!("  用法: /diff <旧文件路径> <新文件路径>");
                    println!("  示例: /diff a.rs b.rs");
                } else {
                    let old_path = parts[0];
                    let new_path = parts[1];
                    match crate::tui::render::diff::diff_files(old_path, new_path) {
                        Ok(hunk) => {
                            self.print_diff_hunk(&hunk);
                        }
                        Err(e) => {
                            println!("  [diff 错误] 无法读取文件: {e}");
                        }
                    }
                }
            }
            "theme" | "t" => {
                // D12 多主题切换命令(2026-09-10 第二十三轮)
                self.run_theme(rest_args);
            }
            // D3 对话 Rewind / 分支(2026-09-10 第二十四轮)
            "rewind" => {
                self.run_rewind(rest_args);
            }
            "undo" => {
                self.run_undo();
            }
            "fork" => {
                self.run_fork();
            }
            "branches" | "branch" => {
                self.run_branches();
            }
            "switch" => {
                self.run_switch(rest_args);
            }
            // 会话持久化(第 96 轮,2026-09-19):跨进程历史会话列表与恢复
            "sessions" | "hist" => {
                self.run_sessions();
            }
            "resume" => {
                self.run_resume(rest_args);
            }
            // D13 离线模式状态查看(2026-09-11):显示连接状态、最近错误、队列深度。
            "offline" | "status" => {
                self.run_offline_status();
            }
            // D8 会话成本估算面板(2026-09-17 第 76 轮)
            "cost" | "usage" => {
                self.run_cost();
            }
            // D4 工作区感知(2026-09-13):查看/刷新工作区快照。
            "workspace" | "ws" => {
                self.run_workspace(rest_args);
            }
            "" => {}
            other => {
                // 内置未命中 → 查自定义命令(D2);命中则渲染模板并送编排。
                // 内置命令不可遮蔽:与内置同名时根本走不进此分支。
                let customs = commands::discover(&self.paths.work_dir);
                match commands::render_by_name(&customs, other, rest_args) {
                    Some(rendered) => {
                        let source = customs
                            .iter()
                            .find(|c| c.name == other)
                            .map(|c| c.source.display().to_string())
                            .unwrap_or_default();
                        println!("  [custom command] /{other} ← {source}");
                        return self.dispatch_prompt(full_line, &rendered).await;
                    }
                    None => {
                        println!("  未知斜杠命令: /{other}");
                        let suggestions = suggest_similar_commands(other);
                        if !suggestions.is_empty() {
                            println!("  您是否想输入: {}", suggestions.join(", "));
                        }
                        println!("  输入 /help 查看所有命令, /commands 查看自定义命令。");
                    }
                }
            }
        }
        Ok(false)
    }

    /// `/theme [kind]`(D12,2026-09-10 第二十三轮):列出或切换主题。
    ///
    /// - 无参数:列出全部主题 + 当前活跃主题 + 实时生效范围说明
    /// - 有参数:解析 → 切换;返回旧主题供输出「✓ 已从 X 切换到 Y」
    ///
    /// 实时生效范围(本轮实现):
    ///   - 主屏 banner 主题行:下次会话或 `/clear` 重打 banner
    ///   - 边框 (engine::border_box):下次子屏重绘
    ///   - diff 标题/行号/前后缀/字符级着色:下次 `/diff` 调用
    ///   - 语法高亮 token:下次围栏渲染
    ///   - Cell::blank() 空白底:全屏实时
    /// 不实时(需重启):子屏硬编码 const(SELECTED_FG / INPUT_BG 等),所以打印友好提示
    fn run_theme(&self, arg: &str) {
        let current = crate::tui::theme::active_kind();
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            println!("  当前主题: {}", current.as_str());
            println!("  可用主题:");
            for k in crate::tui::theme::all_kinds() {
                let marker = if *k == current { " * " } else { "   " };
                println!("    {marker}{:<14} {}", k.as_str(), k.describe());
            }
            println!("  切换主题:");
            println!("    /theme <kind>   例如: /theme dark-contrast");
            println!("    LAEW_THEME=dark-contrast ./laew   (启动期初始化)");
            println!("  注:核心 cell 渲染(banner / 边框 / diff / 高亮)实时生效;");
            println!("      子屏局部配色需重启会话才能完整生效。");
            return;
        }
        match crate::tui::theme::ThemeKind::from_env_str(trimmed) {
            Some(kind) => {
                let prev = crate::tui::theme::set_active(kind);
                println!("  ✓ 已切换主题: {} → {}", prev.as_str(), kind.as_str());
                println!("    核心 cell 渲染实时生效(下次重绘可见)。");
                println!("    子屏局部配色需重启会话才能完整生效。");
            }
            None => {
                println!("  未知主题: {trimmed}");
                println!("  可选: default | dark-contrast | light | daltonized");
                println!("  输入 /theme 查看主题列表与说明。");
            }
        }
    }

    /// `/offline` 或 `/status`(D13,2026-09-11):显示连接状态与离线队列。
    fn run_offline_status(&self) {
        let snap = self.connectivity.snapshot();
        println!("  连接状态: {}", snap.state.as_str());
        match snap.state {
            crate::llm::Connectivity::Online => {
                if snap.consecutive_network_errors > 0 {
                    println!(
                        "  (近期错误已复位,连续 {} 次)",
                        snap.consecutive_network_errors
                    );
                }
            }
            crate::llm::Connectivity::Degraded | crate::llm::Connectivity::Offline => {
                println!("  连续网络错误: {} 次", snap.consecutive_network_errors);
                if let (Some(kind), Some(secs)) = (
                    &snap.last_network_error_kind,
                    snap.last_network_error_ago_secs,
                ) {
                    println!("  最近错误: {kind} ({secs}s 前)");
                }
                println!(
                    "  阈值: {} 次 → Degraded, {} 次 → Offline",
                    crate::llm::DEGRADED_THRESHOLD,
                    crate::llm::OFFLINE_THRESHOLD
                );
            }
        }
        let qlen = self.offline_queue.len();
        if qlen > 0 {
            println!(
                "  离线队列: {}/{} 条 (恢复后输入任意提示词触发 flush)",
                qlen,
                self.offline_queue.capacity()
            );
        } else {
            println!("  离线队列: 空 (容量 {})", self.offline_queue.capacity());
        }
        println!("  提示:离线时输入自动排队,恢复连接后逐条自动处理。");
    }

    /// `/cost`(别名 `/usage`,D8,2026-09-17 第 76 轮):会话用量与成本估算面板。
    ///
    /// 成本语义:
    /// - 「会话实记累计」= transcript 逐轮收口时按当时 active 模型内置价估出的成本之和
    ///   (/rewind 截断、/switch 分支恢复后自动重算,与 session_usage 同源);
    /// - 「成本分解」= 按**当前**模型价对 session_usage 总量估价(中途切过模型时
    ///   仅供分量参考,以实记累计为准);
    /// - 模型无内置参考价(本地模型/未收录)→ 只统计 token,不虚报 $0。
    fn run_cost(&self) {
        let active = self.db.lock().expect("db").get_active_or_env().ok().flatten();
        let model_line = active
            .as_ref()
            .map(|r| {
                format!(
                    "[{}] {}/{} @ {}",
                    r.protocol.as_str(),
                    r.provider_name,
                    r.model_name,
                    r.end_point
                )
            })
            .unwrap_or_else(|| "<未配置>".to_string());
        println!("  会话成本(估算,内置参考价 2026-09,非账单依据):");
        println!("    模型: {model_line}");
        let u = &self.session_usage;
        if u.input_tokens == 0 && u.output_tokens == 0 {
            println!("    (本会话尚无 LLM 调用)");
            return;
        }
        println!(
            "    累计用量: input={}  output={}  cache_read={}  cache_creation={}",
            u.input_tokens, u.output_tokens, u.cache_read_input_tokens, u.cache_creation_input_tokens
        );
        if let Some(rate) = crate::llm::pricing::cache_hit_rate(u) {
            println!("    缓存命中率: {:.1}%", rate * 100.0);
        }
        let (recorded, partial) = self.session_cost_summary();
        // 按当前模型价的总量分解(当前模型有价时)
        if let Some(model) = active.as_ref().map(|r| r.model_name.as_str()) {
            if let Some(b) = crate::llm::cost_breakdown(model, u) {
                println!(
                    "    成本分解(按当前模型价): input {} + output {} + cache_read {} + cache_write {} ≈ {}",
                    crate::llm::format_usd(b.input_usd),
                    crate::llm::format_usd(b.output_usd),
                    crate::llm::format_usd(b.cache_read_usd),
                    crate::llm::format_usd(b.cache_write_usd),
                    crate::llm::format_usd(b.total()),
                );
            }
        }
        if recorded > 0.0 {
            let suffix = if partial {
                "(下限:部分轮次模型无内置价,未计入)"
            } else {
                ""
            };
            println!(
                "    会话实记累计: ≈{} {}",
                crate::llm::format_usd(recorded),
                suffix
            );
        } else {
            let name = active
                .as_ref()
                .map(|r| r.model_name.as_str())
                .unwrap_or("<未配置>");
            println!("    模型 {name} 无内置参考价,仅统计 token。");
        }
    }

    /// `/workspace [refresh|ws]`(D4,2026-09-13 第 01 轮):查看工作区快照。
    ///
    /// - 无参数 / 默认:命中进程级 TTL 缓存(5s)后展示,零额外开销
    /// - `refresh`(别名 `rf` / `-f`):强制失效缓存并按磁盘真实状态重采集
    ///
    /// 展示内容与注入给 Agent 的 `<<<LAEW:WORKSPACE>>>` / PROJECT_CONTEXT 工作区段一致,
    /// 便于用户核对「模型看到的运行环境」。
    fn run_workspace(&self, arg: &str) {
        let arg = arg.trim().to_ascii_lowercase();
        let force = matches!(arg.as_str(), "refresh" | "rf" | "-f" | "--force");
        if force {
            crate::agent::workspace::invalidate();
        }
        let snap = crate::agent::workspace::snapshot(&self.paths.work_dir);
        println!(
            "  工作区快照(/workspace){}:",
            if force { " · 已刷新" } else { "" }
        );
        for line in snap.render_section().lines() {
            println!("  {line}");
        }
        if snap.is_trivial() {
            println!("  (空目录:既不注入 PROJECT_CONTEXT,也不注入运行时环境 brief)");
        } else {
            println!("  注入状态: 会话级已并入 PROJECT_CONTEXT;每次 LLM 调用另附运行时 brief。");
        }
        println!("  用法: /workspace refresh  强制重新采集(默认命中 5s 缓存)。");
    }

    /// `/rewind [N]`(D3,2026-09-10 第二十四轮):列出轮次或回退到第 N 轮之前。
    ///
    /// - 无参数:列出当前会话全部真实轮次(#编号 + 时间 + 首行预览)
    /// - `/rewind N`:第 N..末轮全部移除(回退前自动快照存分支),
    ///   context / transcript / session_usage 三处一致截断
    fn run_rewind(&mut self, arg: &str) {
        let turns = crate::agent::session_fork::scan_user_turns(self.session.context());
        if turns.is_empty() {
            println!("  当前会话还没有可回退的对话轮次。");
            return;
        }
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            println!("  可回退的对话轮次(共 {} 轮,从旧到新):", turns.len());
            for t in &turns {
                let ts = self
                    .transcript
                    .get(t.order - 1)
                    .map(|e| e.ts.as_str())
                    .unwrap_or("-");
                let preview = first_line_preview(&t.prompt, 48);
                println!("    #{} [{}] {}", t.order, ts, preview);
            }
            println!(
                "  用法: /rewind <编号>  回退到该轮之前(该轮及其后全部移除,原对话自动存为分支)"
            );
            println!(
                "        /undo           撤销最后一轮(等价 /rewind {})",
                turns.len()
            );
            return;
        }
        match trimmed.parse::<usize>() {
            Ok(order) => self.run_rewind_order(order),
            Err(_) => {
                println!("  无效轮次编号: {trimmed}(应为 1..={} 的整数)", turns.len());
                println!("  输入 /rewind 查看全部轮次。");
            }
        }
    }

    /// 执行回退到第 `order` 轮之前(编号已由调用方给出,此处负责校验与三处一致截断)。
    fn run_rewind_order(&mut self, order: usize) {
        let turns = crate::agent::session_fork::scan_user_turns(self.session.context());
        if order == 0 || order > turns.len() {
            println!(
                "  无效轮次编号: {order}(当前共 {} 轮,编号 1..={})",
                turns.len(),
                turns.len()
            );
            println!("  输入 /rewind 查看全部轮次。");
            return;
        }
        let boundary = turns[order - 1].ctx_index;
        let removed_turns = turns.len() - order + 1;
        let removed_msgs = self.session.context().len() - boundary;
        // 破坏性操作前快照(atomcode RewindTransactionGuard 语义:截断与快照同生成败)
        let name = self
            .snapshot_current("rewind", &format!("/rewind {order} 回退前"))
            .expect("调用方已确认存在真实轮次");
        self.session.context_mut().truncate(boundary);
        self.transcript.truncate(order - 1);
        // 累计用量由剩余轮次重算(被移除轮次的 token 不再计入「当前会话」)
        self.session_usage = self
            .transcript
            .iter()
            .fold(crate::llm::Usage::default(), |acc, e| {
                merge_usage(acc, e.usage)
            });
        println!(
            "  ✓ 已回退到第 {order} 轮之前(移除 {removed_turns} 轮对话 / {removed_msgs} 条上下文消息)"
        );
        if order == 1 {
            println!("    已清空全部真实轮次(项目上下文等内部标记保留,不会重复注入)。");
        } else {
            println!(
                "    保留第 1..={} 轮,当前上下文 {} 条消息。",
                order - 1,
                boundary
            );
        }
        println!("    原对话已存为分支 {name},可用 /switch {name} 找回。");
    }

    /// `/undo`(D3):撤销最后一轮(最常用路径一键化,等价 `/rewind <末轮>`)。
    fn run_undo(&mut self) {
        let n = crate::agent::session_fork::scan_user_turns(self.session.context()).len();
        if n == 0 {
            println!("  当前会话还没有可回退的对话轮次。");
            return;
        }
        self.run_rewind_order(n);
    }

    /// `/fork`(D3,pi `/clone` current leaf 语义):从当前对话分叉出新 Session。
    ///
    /// 上下文完整拷贝 + 新 Session ID,后续对话在新会话上进行;
    /// 原对话自动存分支(两个方向都不会丢)。
    fn run_fork(&mut self) {
        let turns = crate::agent::session_fork::scan_user_turns(self.session.context());
        if turns.is_empty() {
            println!("  当前会话没有对话轮次,无需分叉(直接输入提示词即可)。");
            return;
        }
        let name = self
            .snapshot_current("fork", "fork 前原会话")
            .expect("调用方已确认存在真实轮次");
        let forked = Session::fork_from(&self.session);
        let new_id = forked.id.clone();
        let msg_count = forked.context.len();
        self.session = forked;
        println!("  ✓ 已从当前对话分叉出新会话: {new_id}");
        println!(
            "    上下文 {msg_count} 条消息 / {} 轮完整保留,后续对话在新会话上进行。",
            turns.len()
        );
        println!("    原对话已存为分支 {name},可用 /switch {name} 找回。");
    }

    /// `/branches`(D3):列出已存分支(新→旧)。
    fn run_branches(&self) {
        if self.branches.is_empty() {
            println!("  暂无分支。/rewind <n>、/fork、/switch、/clear 在改动前会自动保存分支。");
            return;
        }
        println!("  已存分支({} 个,新→旧):", self.branches.len());
        for s in self.branches.list() {
            let preview = if BranchStore::last_turn_preview(s).is_empty() {
                "-".to_string()
            } else {
                first_line_preview(&BranchStore::last_turn_preview(s), 32)
            };
            println!(
                "    {} [{}] {} 轮 | {} | 末轮: {}",
                s.name,
                s.created_at,
                s.transcript.len(),
                s.note,
                preview
            );
        }
        println!(
            "  切换: /switch <分支名>;分支保存在内存中(最多 10 个,超出淘汰最旧),退出 TUI 后失效。"
        );
    }

    /// `/switch <name>`(D3):切换到指定分支;切换前当前对话自动快照(零丢失)。
    fn run_switch(&mut self, arg: &str) {
        let name = arg.trim();
        if name.is_empty() {
            println!("  用法: /switch <分支名>(输入 /branches 查看可用分支)");
            return;
        }
        if self.branches.get(name).is_none() {
            println!("  未找到分支: {name}(输入 /branches 查看可用分支)");
            return;
        }
        let cur = self.snapshot_current("switch", &format!("/switch {name} 切换前"));
        let (session, transcript, usage) = self.branches.restore(name).expect("上面已校验分支存在");
        let restored_id = session.id.clone();
        let restored_turns = transcript.len();
        self.session = session;
        self.transcript = transcript;
        self.session_usage = usage;
        println!("  ✓ 已切换到分支 {name}(Session {restored_id}, {restored_turns} 轮对话)");
        if let Some(cur) = cur {
            println!("    切换前的对话已自动存为分支 {cur},可再切回。");
        } else {
            println!("    切换前会话无真实轮次,未产生快照。");
        }
    }

    /// `/export [path]`(D8):导出当前会话 transcript。
    fn run_export(&mut self, path_arg: &str) {
        let model = match self.db.lock().expect("db").get_active_or_env() {
            Ok(Some(r)) => format!(
                "[{}] {}/{}",
                r.protocol.as_str(),
                r.provider_name,
                r.model_name
            ),
            _ => "<未配置>".to_string(),
        };
        let default_ts = export::now_export_stamp(); // YYYYMMDD-HHMMSS
        let explicit = if path_arg.trim().is_empty() {
            None
        } else {
            Some(path_arg.trim())
        };
        // D8 成本(2026-09-17 第 76 轮):全轮实记合计;全无价 → None(导出不显示)。
        let (recorded_cost, cost_partial) = self.session_cost_summary();
        let meta = export::ExportMeta {
            session_id: self.session.id.clone(),
            session_created_at: export::humanize_compact(&self.session.created_at),
            exported_at: export::now_export_human(),
            model,
            turns: self.transcript.len(),
            total_usage: self.session_usage,
            cost_partial,
            total_cost_usd: (recorded_cost > 0.0).then_some(recorded_cost),
        };
        match export::resolve_target(&self.paths.work_dir, explicit, "laew-export", &default_ts) {
            Ok((path, fmt)) => {
                let fmt_name = if fmt == export::ExportFormat::Json {
                    "JSON"
                } else {
                    "Markdown"
                };
                match export::write_export(&meta, &self.transcript, &path, fmt) {
                    Ok(()) => println!(
                        "{}",
                        pathfmt::fit_line(
                            &format!("  ✓ 已导出 {fmt_name}: "),
                            &pathfmt::display_path(&self.paths, &path),
                            &format!(" ({} 轮对话)", meta.turns)
                        )
                    ),
                    Err(e) => eprintln!("  导出失败: {e}"),
                }
            }
            Err(e) => eprintln!("  导出失败: {e}"),
        }
    }

    // ========================================================================
    // 会话持久化与跨进程恢复(第 96 轮,2026-09-19,tmpPlan/2026-09-19_02)
    // ========================================================================

    /// `/sessions`(别名 `/hist`):列出持久化历史会话(新→旧)。
    ///
    /// 数据来自根目录 SQLite `chat_sessions`/`chat_turns`(每轮任务收口自动整快照落盘),
    /// 跨进程存活;自动保留最近 [`crate::database::chat_store::CHAT_SESSIONS_KEEP`] 个,
    /// 超出随新会话落盘淘汰。
    fn run_sessions(&self) {
        let sessions = match self.db.lock().expect("db").list_chat_sessions(200) {
            Ok(s) => s,
            Err(e) => {
                println!("  [sessions] 读取历史会话失败: {e}");
                return;
            }
        };
        if sessions.is_empty() {
            println!("  暂无历史会话。TUI 内完成首轮对话后自动保存,之后可用 /resume 恢复。");
            return;
        }
        let shown: Vec<&SessionSummary> = sessions.iter().take(15).collect();
        println!(
            "  历史会话(跨进程持久化,显示最近 {} / 共 {} 个,新→旧):",
            shown.len(),
            sessions.len()
        );
        for (i, s) in shown.iter().enumerate() {
            let model = s.model_name.as_deref().unwrap_or("-");
            let preview = if s.title.is_empty() {
                "-".to_string()
            } else {
                first_line_preview(&s.title, 24)
            };
            println!(
                "    #{} [{}] {} 轮 | {} | 首条: {} | {}",
                i + 1,
                s.updated_at,
                s.turn_count,
                model,
                preview,
                s.session_id
            );
        }
        println!("  恢复: /resume <序号> 或 /resume <session-id 前缀>;CLI 启动恢复: laew --resume(最近一次)/ laew -c 2。");
        println!(
            "  保留策略: 自动保留最近 {} 个会话,超出自动清理。",
            crate::database::chat_store::CHAT_SESSIONS_KEEP
        );
    }

    /// `/resume [N|id前缀]`:恢复历史会话(当前对话自动快照存分支,零丢失)。
    fn run_resume(&mut self, arg: &str) {
        let spec = arg.trim();
        if spec.is_empty() {
            println!("  用法: /resume <序号|session-id 前缀>(输入 /sessions 查看可用会话)");
            println!("        /resume latest  恢复最近一次会话");
            return;
        }
        self.resume_by_spec(spec, "[resume]");
    }

    /// `--resume` / `-c` 启动期恢复(由 dispatch.rs `handle_user_input` 首次输入时调用)。
    pub(crate) fn apply_startup_resume(&mut self, spec: &str) {
        self.resume_by_spec(spec, "[resume]");
    }

    /// 恢复规格解析与执行:`latest` / 列表序号 / session-id 唯一前缀。
    fn resume_by_spec(&mut self, spec: &str, tag: &str) {
        let sessions = match self.db.lock().expect("db").list_chat_sessions(200) {
            Ok(s) => s,
            Err(e) => {
                println!("  {tag} 读取历史会话失败: {e}");
                return;
            }
        };
        if sessions.is_empty() {
            println!("  {tag} 暂无历史会话可恢复(TUI 内完成首轮对话后自动保存)。");
            return;
        }
        match resolve_chat_session(&sessions, spec) {
            ResumeResolution::Latest => {
                self.apply_resume(&sessions[0]);
            }
            ResumeResolution::Index(i) | ResumeResolution::Prefix(i) => {
                self.apply_resume(&sessions[i]);
            }
            ResumeResolution::Ambiguous(ids) => {
                let shown: Vec<&str> = ids.iter().take(3).map(|s| s.as_str()).collect();
                println!(
                    "  {tag} 前缀命中 {} 个会话,请补长前缀或改用序号: {}",
                    ids.len(),
                    shown.join(" / ")
                );
            }
            ResumeResolution::NotFound => {
                println!(
                    "  {tag} 未找到会话「{spec}」;输入 /sessions 查看可用会话(支持序号或 session-id 前缀)。"
                );
            }
        }
    }

    /// 执行恢复:重建 context/transcript/usage 三处一致(D3 不变量),保持原 Session ID
    /// (session_memory 历史摘要链连续);恢复前当前对话自动快照存分支(零丢失)。
    ///
    /// 返回是否成功(失败时当前会话保持原状)。
    fn apply_resume(&mut self, s: &SessionSummary) -> bool {
        let turns = match self.db.lock().expect("db").load_chat_turns(&s.session_id) {
            Ok(t) => t,
            Err(e) => {
                println!("  [resume] 读取会话 {} 轮次失败: {e}", s.session_id);
                return false;
            }
        };
        if turns.is_empty() {
            println!("  [resume] 会话 {} 没有对话轮次,无需恢复。", s.session_id);
            return false;
        }
        // 破坏性操作前快照(D3 零丢失语义,与 /rewind /fork /switch 一致)
        let cur = self.snapshot_current("resume", "resume 前原会话");
        let (context, transcript, usage) = rebuild_from_turns(&turns);
        let restored_turns = transcript.len();
        self.session = Session {
            id: s.session_id.clone(),
            device_id: crate::session::device_id().to_string(),
            created_at: s.created_at.clone(),
            context,
        };
        self.transcript = transcript;
        self.session_usage = usage;
        println!(
            "  ✓ 已恢复会话 {}({restored_turns} 轮,input={} output={})",
            s.session_id, usage.input_tokens, usage.output_tokens
        );
        println!("    创建于 {};最近更新 {}。", s.created_at, s.updated_at);
        if let Some(name) = cur {
            println!("    恢复前的对话已自动存为分支 {name},可用 /switch {name} 找回。");
        }
        println!("    可直接继续对话;项目说明文件将在下个任务按当前工作目录重新探测注入。");
        true
    }

    /// `/commands`(D2):列出已加载的自定义命令(名称/描述/来源)。
    fn print_custom_commands(&self) {        let customs = commands::discover(&self.paths.work_dir);
        if customs.is_empty() {
            println!("  当前无自定义命令。创建方法(Markdown 模板):");
            println!(
                "    项目级: {}/.laew/commands/<命令名>.md",
                self.paths.work_dir.display()
            );
            println!("    用户级: ~/.laew/commands/<命令名>.md");
            println!("    模板内可用 $ARGUMENTS(全量参数)与 $1-$9(位置参数)");
            // 静默失败可诊断化(第 23 轮):列出「存在 .md 但文件名非法被跳过」的项
            let ignored = commands::scan_invalid_names(&self.paths.work_dir);
            if !ignored.is_empty() {
                println!("  ⚠ 发现未加载的命令文件(文件名不合法):");
                for line in ignored {
                    println!("    {line}");
                }
            }
            return;
        }
        println!("  自定义命令({} 个,用户级优先于项目级):", customs.len());
        for c in &customs {
            let hint = if c.argument_hint.is_empty() {
                String::new()
            } else {
                format!(" {}", c.argument_hint)
            };
            // 2026-09-12 第 44 轮:Windows 上 path.display() 输出 `C:\foo\.laew\commands\...`
            // (反斜杠),e2e §7b 与脚本测试习惯匹配 POSIX 风格(正斜杠),统一替换
            // 成正斜杠;Linux/macOS 路径本来就是 `/`,replace 后无变化。
            let src = c.source.display().to_string().replace('\\', "/");
            println!("  /{}{hint}", c.name);
            println!("    {} — {}", c.description, src);
        }
    }
}

/// 持久化轮次 → 恢复态重建(第 96 轮):context 交替 user(prompt) / assistant(上下文版)
/// + transcript 逐字段还原 + usage 求和。纯函数,与 TuiSession 解耦便于单测。
///
/// - `context_response` 优先于 `response`(Executed 轮两版分流,第 30 轮语义);
/// - 空 `context_response` 容错回退 `response`(历史数据防御);
/// - 未知 outcome 字符串容错为 Error,不炸恢复链路。
fn rebuild_from_turns(turns: &[ChatTurnRow]) -> (Vec<ChatMessage>, Vec<TranscriptEntry>, Usage) {
    let mut context: Vec<ChatMessage> = Vec::with_capacity(turns.len() * 2);
    let mut transcript: Vec<TranscriptEntry> = Vec::with_capacity(turns.len());
    let mut usage = Usage::default();
    for t in turns {
        let reply = if t.context_response.is_empty() {
            t.response.clone()
        } else {
            t.context_response.clone()
        };
        context.push(ChatMessage::user(t.prompt.clone()));
        context.push(ChatMessage::assistant(vec![ContentBlock::text(reply.clone())]));
        usage.input_tokens = usage.input_tokens.saturating_add(t.usage.input_tokens);
        usage.output_tokens = usage.output_tokens.saturating_add(t.usage.output_tokens);
        usage.cache_read_input_tokens = usage
            .cache_read_input_tokens
            .saturating_add(t.usage.cache_read_input_tokens);
        usage.cache_creation_input_tokens = usage
            .cache_creation_input_tokens
            .saturating_add(t.usage.cache_creation_input_tokens);
        transcript.push(TranscriptEntry {
            ts: t.ts.clone(),
            raw_input: t.raw_input.clone(),
            prompt: t.prompt.clone(),
            response: t.response.clone(),
            context_response: (!t.context_response.is_empty()).then(|| t.context_response.clone()),
            usage: t.usage,
            cost_usd: t.cost_usd,
            outcome: outcome_from_store_str(&t.outcome),
        });
    }
    (context, transcript, usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(seq: i64, prompt: &str, response: &str, ctx: &str, outcome: &str) -> ChatTurnRow {
        ChatTurnRow {
            seq,
            ts: "14:00:00".into(),
            raw_input: format!("原始{seq}"),
            prompt: prompt.into(),
            response: response.into(),
            context_response: ctx.into(),
            outcome: outcome.into(),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 40,
                cache_read_input_tokens: 5,
                cache_creation_input_tokens: 2,
            },
            cost_usd: Some(0.5),
        }
    }

    #[test]
    fn rebuild_alternating_roles_and_usage_sum() {
        let turns = vec![
            row(1, "问一", "人类版答一", "上下文版答一", "executed"),
            row(2, "问二", "答二", "答二", "direct"),
        ];
        let (context, transcript, usage) = rebuild_from_turns(&turns);
        assert_eq!(context.len(), 4, "每轮 user+assistant 两条");
        assert_eq!(context[0].content_text(), "问一");
        assert_eq!(context[1].content_text(), "上下文版答一", "assistant 用上下文回填版");
        assert_eq!(context[2].content_text(), "问二");
        assert_eq!(context[3].content_text(), "答二");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].response, "人类版答一", "transcript 保留人类版");
        assert_eq!(
            transcript[0].context_response.as_deref(),
            Some("上下文版答一"),
            "持久化字段原样回填"
        );
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 80);
        assert_eq!(usage.cache_read_input_tokens, 10);
        assert_eq!(usage.cache_creation_input_tokens, 4);
    }

    #[test]
    fn rebuild_falls_back_when_context_response_empty() {
        let turns = vec![row(1, "问", "答(两版相同)", "", "direct")];
        let (context, transcript, _) = rebuild_from_turns(&turns);
        assert_eq!(context[1].content_text(), "答(两版相同)", "空上下文版回退人类版");
        assert!(transcript[0].context_response.is_none(), "空串不写成 Some");
    }

    #[test]
    fn rebuild_unknown_outcome_tolerated_as_error() {
        let turns = vec![row(1, "问", "答", "答", "???")];
        let (_, transcript, _) = rebuild_from_turns(&turns);
        assert_eq!(transcript[0].outcome, export::OutcomeKind::Error);
    }

    #[test]
    fn rebuild_empty_turns_yields_empty_state() {
        let (context, transcript, usage) = rebuild_from_turns(&[]);
        assert!(context.is_empty());
        assert!(transcript.is_empty());
        assert_eq!(usage.input_tokens, 0);
    }
}
