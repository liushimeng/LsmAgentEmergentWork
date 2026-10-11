//! 自定义输入处理器(基于 crossterm)—— 固定底部输入组件。
//!
//! 方案见 `tmpPlan/2026-09-09_03-TUI底部固定输入组件方案.md`:
//! - DECSTBM 滚动区(`ESC[1;{panel_top}r`)把输出限制在屏幕上方,
//!   底部 `theme::INPUT_AREA_HEIGHT` 行是输入面板;面板顶行**紧跟当前光标**
//!   (第 134 轮自适应,短输出不再留大片空白;查不到光标行时回退吸底 —— 见 `panel.rs`);
//! - 输入行整行铺 `theme::INPUT_BG` 底色 + `theme::INPUT_FG` 前景,
//!   与输出区(默认底色)一眼可辨;
//! - 补全浮层向上覆盖绘制在面板上方(底部无空间向下展开);
//! - 提交时在滚动区底行回显已提交内容,保留输入痕迹;
//! - 退出路径(Ctrl-D / `/exit` → teardown_pinned)还原滚动区并清空面板,不留残迹。
use crate::tui::completion::{CompletionEngine, CompletionItem};
use crate::tui::mention::{mention_token_at, FileSuggester};
use crate::tui::paste;
use crate::tui::textfit;
use panel::{query_cursor_row, teardown_pinned_layout};
use crossterm::{
    cursor::MoveTo,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    style::{Print, ResetColor},
    terminal,
};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// 返回 `cursor`(字节偏移,须在字符边界)前一个字符的起始字节偏移。
///
/// 输入循环的 cursor 恒为字节偏移并保持在字符边界上;退格 / 左移按「字符」
/// 语义移动,必须先换算到前一个字符的起始边界,否则 `String::remove/insert`
/// 会触发 `is_char_boundary` panic(修复:TUI 中文输入第 2 个字符必崩)。
fn prev_char_boundary(s: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut i = cursor - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 单字符近似显示宽度(列数)—— 转发 [`textfit::char_width`]。
///
/// 2026-09-24:宽度表唯一真源下移到 `textfit`(盒线渲染与输入行光标列号换算必须
/// 用同一套度量,否则横幅按 A 算、输入行按 B 算,同一字符两处错位)。
/// `LAEW_AMBIGUOUS_WIDE=1` 的歧义宽度策略因此对**输入行**同样生效。
pub fn char_width(c: char) -> u16 {
    textfit::char_width(c)
}

/// 近似显示宽度(列数),饱和到 `u16::MAX` 避免超长输入溢出。
///
/// 不引入 `unicode-width` 依赖的轻量近似:覆盖常用 CJK 区段,
/// 边缘字符(组合符等)按 1 列处理,可接受。
pub fn display_width(s: &str) -> u16 {
    textfit::width(s).min(u16::MAX as usize) as u16
}

/// 从头截取不超过 `max` 显示列的子串(不在双宽字符中间截断)。
fn fit_width(s: &str, max: u16) -> String {
    window_str(s, 0, max)
}

/// 从 `start` 字节偏移起截取不超过 `avail` 显示列的子串。
fn window_str(s: &str, start: usize, avail: u16) -> String {
    let mut out = String::new();
    let mut w = 0u16;
    for c in s[start..].chars() {
        let cw = char_width(c);
        if w + cw > avail {
            break;
        }
        out.push(c);
        w += cw;
    }
    out
}

/// 计算输入内容的水平滚动可视窗口,保证 cursor 列在 `avail` 宽度内可见。
/// 返回 (窗口起始字节偏移, 光标在窗口内的列号)。
fn visible_window(s: &str, cursor: usize, avail: u16) -> (usize, u16) {
    let avail = avail.max(1);
    let mut start = 0usize;
    while start < cursor && display_width(&s[start..cursor]) >= avail {
        start += s[start..].chars().next().map_or(1, |c| c.len_utf8());
    }
    (start, display_width(&s[start..cursor]))
}

// ========== 第 93 轮:无 bracketed paste 终端的 Enter 粘贴突发探测 ==========
//
// 背景(实测 llaew_20260919_121348.log):Windows conhost / 不支持 bracketed paste
// 的终端里,多行提示词粘贴以「逐键 Char 事件 + 行间 Enter」形式到达,旧实现 Enter
// 到达即提交 —— 8 条编号需求只有首行标题进入任务,其余行在任务结束后被当作新任务
// 逐条提交(微信聊天任务被截成「打开窗口保持10分钟」的主根因)。
// 本探测在 Enter 提交前检查输入队列:粘贴的后续行此刻必然已排在终端输入缓冲里,
// Enter 之后还有排队事件 → 判定为粘贴突发,Enter 按换行处理不提交,整段经 D6
// 粘贴管线进 buffer(小粘贴直插 / 多粘贴 [粘贴 #N] marker),最后一次 Enter
// (队列已空)或用户复查后再按 Enter 才提交完整提示词。
// 人类击键间隔 ≫15ms,正常单击 Enter 队列恒空,零误伤;bracketed paste 生效的
// 终端粘贴整体以 Event::Paste 送达,不经本路径。

// 第 128 轮加宽:15/2/8(有效窗口 ≈31ms)→ 40/3/20(有效窗口 ≈100ms)。
// 实测事故(llaew_20260924_151442.log):4 行 Markdown 提示词只有首行进入管线,
// 含目标 URL 的 2、3 行整体丢失(全量日志 grep "anthropic" 命中 0 次),
// 直接导致 Agent 无从知道目标站点、进而猜了个无关网站跑完整个任务。
// Windows conhost 对含 CJK 的长行分块投递,块间隔可能超过 31ms → 探测落空 →
// 首行的 CR 被当成用户提交意图。
// 加宽后仍然安全:人类击键间隔 ≫100ms,单击 Enter 时队列恒空,首证据 40ms 内必返回 None,
// 代价只是每次提交多等 ≤40ms(不可感知)。
//
// ★ 第 154 轮(2026-10-10 第三次同因实测事故,llaew_20261010_223313.log):
// 40/3/20 的纯时序探测**仍会落空** —— ConPTY 分块投递的块间隔可超 100ms 有效窗口;
// 且粘贴在任务运行期间到达时,主循环阻塞在 handle_user_input,残余行堆积 tty,
// 任务结束后逐个行间 CR 又被当成提交,最终只活下来 1 行(实测 9 行只剩 2 行,
// 含目标 URL 的第 3 行起整体丢失 → Agent 改派豆包桌面应用,浏览器/人工介入全程未触发)。
// 本轮在时序探测之外补第二信号「击键节奏武装」,见 [`char_burst_armed`] 与
// Enter 三级判定(使用点在 CompletionDecision::Submit 分支):
//   ① 40ms 队列探测命中 → 粘贴换行(余行已在缓冲,backlog 形态);
//   ② 未命中但武装态 → 延长观望 300ms(50ms×6 步),期间任意事件到达即判粘贴换行
//     (ConPTY 分块滞后形态);静默则提交(人类快速连打 / tmux send-keys 两条命令
//     的 e2e 形态,代价仅武装态提交多等 ≤300ms);
//   ③ 均无 → 正常提交(零额外延迟)。

/// Enter 突发探测:首个排队事件的等待窗口(毫秒)。
const PASTE_BURST_PROBE_MS: u64 = 40;
/// 粘贴尾部积累:队列静默多少毫秒即收口(分块投递的块间隔兜底)。
const PASTE_TAIL_QUIET_MS: u64 = 600;
/// 粘贴尾部积累:总时长上限(毫秒),防异常终端无限滴流。
const PASTE_TAIL_MAX_MS: u64 = 10_000;
/// 粘贴尾部积累:字节上限,防超长粘贴拖垮积累循环(超过部分原样留在队列)。
const PASTE_TAIL_MAX_BYTES: usize = 64 * 1024;
/// 武装态 Enter 延长观望:步长(毫秒)× 步数(合计 300ms)。
const BURST_ARMED_WATCH_STEP_MS: u64 = 50;
const BURST_ARMED_WATCH_STEPS: usize = 6;

// =================== 第 154 轮:击键节奏武装(粘贴第二信号) ===================

/// 节奏武装窗口(毫秒):尾部窗口内到达 ≥ [`BURST_CADENCE_MIN_CHARS`] 个字符即武装。
const BURST_CADENCE_WINDOW_MS: u64 = 300;
/// 节奏武装最少字符数:300ms 内 8 字 ≈ 27 字/秒 —— 人类连击极限 ≈10~12 字/秒,
/// IME 整词上屏 ≤6 字/短语,均不误伤;无 bracketed paste 终端的粘贴(逐键或整块)
/// 远超此速率。
const BURST_CADENCE_MIN_CHARS: usize = 8;
/// 节奏记录保留时长(毫秒):超过即丢弃,武装态在输入静默 300ms 后自然解除
/// (单行长粘贴粘完再手动按 Enter 的场景不受影响)。
const BURST_ARRIVAL_KEEP_MS: u64 = 1_200;

/// 记录一次字符到达时刻并修剪过期记录(保留最近 BURST_ARRIVAL_KEEP_MS)。
fn note_char_arrival(arrivals: &mut VecDeque<Instant>) {
    let now = Instant::now();
    arrivals.push_back(now);
    while let Some(front) = arrivals.front() {
        if now.duration_since(*front) > Duration::from_millis(BURST_ARRIVAL_KEEP_MS) {
            arrivals.pop_front();
        } else {
            break;
        }
    }
}

/// 击键节奏武装判定(纯函数,可单测):尾部 [`BURST_CADENCE_WINDOW_MS`] 窗口内
/// 到达的字符数 ≥ [`BURST_CADENCE_MIN_CHARS`] → 判定「正在粘贴」。
fn char_burst_armed(arrivals: &VecDeque<Instant>, now: Instant) -> bool {
    let window = Duration::from_millis(BURST_CADENCE_WINDOW_MS);
    arrivals
        .iter()
        .rev()
        .take_while(|t| now.duration_since(**t) <= window)
        .count()
        >= BURST_CADENCE_MIN_CHARS
}

/// 把一串已排队事件折叠为粘贴文本(纯函数,可单测):
/// - `Key(Press) Char(c)`(无 Ctrl)→ 收 `c`;
/// - `Key(Press) Enter` → 收 `\n`(行间换行);
/// - `Event::Paste(t)` → 收 `t`(混合形态兜底);
/// - 首个其它事件原样回吐(调用方存入 pending,不丢事件)。
fn burst_events_to_text(events: Vec<Event>) -> (String, Option<Event>) {
    let mut text = String::new();
    let mut spill = None;
    for ev in events {
        match ev {
            Event::Key(k)
                if k.kind == KeyEventKind::Press
                    && matches!(k.code, KeyCode::Char(_))
                    && !k.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if let KeyCode::Char(c) = k.code {
                    text.push(c);
                }
            }
            Event::Key(k) if k.kind == KeyEventKind::Press && k.code == KeyCode::Enter => {
                text.push('\n');
            }
            Event::Paste(t) => text.push_str(&t),
            other => {
                spill = Some(other);
                break;
            }
        }
    }
    (text, spill)
}

/// 收集粘贴突发事件(参数化窗口,`drain_paste_burst` / `watch_armed_paste_tail` 共用):
/// - `probe_ms`:首证据等待窗口(人类单击 Enter 时队列恒空,直接 None,零感知延迟);
/// - `quiet_ms`:队列排空后静默多少毫秒即收口(捕捉分块投递的尾部);
/// - `max_ms`:总时长上限(自首证据起算),异常终端滴流时强制收口。
///
/// 收集循环内逐次检查终端挂断(第 136 轮:`event::poll` 在 pty 关闭后恒返回 true,
/// `event::read()` 会陷入 crossterm 内部死转);字节上限触发即收口,剩余事件留给
/// 主循环按普通按键处理。非文本事件(Ctrl 组合 / Resize 等)存入 `pending` 回吐。
fn drain_paste_events(
    pending: &mut Option<Event>,
    probe_ms: u64,
    quiet_ms: u64,
    max_ms: u64,
) -> Option<String> {
    // 首证据:短暂等待
    if !matches!(
        event::poll(Duration::from_millis(probe_ms)),
        Ok(true)
    ) {
        return None;
    }
    let started = Instant::now();
    let mut events = Vec::new();
    loop {
        // 第 136 轮:终端挂断时 `event::poll` 会**恒返回 true**(POLLIN 一直就绪),
        // 紧接着的 `event::read()` 就进了 crossterm 的内部死转。这里提前收口。
        if crate::tui::term_guard::stdin_hangup() {
            break;
        }
        if started.elapsed() > Duration::from_millis(max_ms) {
            break;
        }
        match event::poll(Duration::ZERO) {
            Ok(true) => {
                if let Ok(ev) = event::read() {
                    events.push(ev);
                }
            }
            _ => {
                // 队列空:静默 quiet_ms 仍无事件 → 粘贴结束
                if !matches!(
                    event::poll(Duration::from_millis(quiet_ms)),
                    Ok(true)
                ) {
                    break;
                }
            }
        }
    }
    let (mut text, spill) = burst_events_to_text(events);
    if let Some(ev) = spill {
        *pending = Some(ev);
    }
    // 字节上限:回落到 ≤ 上限的最近字符边界再截(CJK 多字节安全,truncate 不落字符中间)
    if text.len() > PASTE_TAIL_MAX_BYTES {
        let mut idx = PASTE_TAIL_MAX_BYTES;
        while idx > 0 && !text.is_char_boundary(idx) {
            idx -= 1;
        }
        text.truncate(idx);
    }
    // ★ 第 94 轮修复(语义保留):空文本返回 None(而非 Some("")),避免外层因
    // `Some("")` 进入 burst 插入路径后 `continue`,既不提交也不反馈,表现为「卡死」。
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Enter 提交前探测粘贴突发:队列有后续事件 → Some(突发文本),无 → None(正常提交)。
///
/// 第 154 轮:探测命中后改用**长窗口尾部积累**(600ms 静默 × 10s 上限)—— 旧实现
/// ~100ms 宽限对 ConPTY 分块滞后会截出半份粘贴,余行再被当成普通输入污染编辑器。
fn drain_paste_burst(pending: &mut Option<Event>) -> Option<String> {
    drain_paste_events(
        pending,
        PASTE_BURST_PROBE_MS,
        PASTE_TAIL_QUIET_MS,
        PASTE_TAIL_MAX_MS,
    )
}

/// 武装态 Enter 的延长观望(第 154 轮三级判定第 ② 级):首证据未在 40ms 内出现时,
/// 按 50ms×N 步继续观望;期间任意事件到达即转入长窗口收集。全程静默 → None(提交)。
///
/// 与一次性 poll(300ms) 的差别:只在**有事件到达**时才转入收集,静默路径逐步提前
/// 退出,普通快速连打后的 Enter 最多多等一个步长。
fn watch_armed_paste_tail(pending: &mut Option<Event>) -> Option<String> {
    for _ in 0..BURST_ARMED_WATCH_STEPS {
        if crate::tui::term_guard::stdin_hangup() {
            return None;
        }
        if matches!(
            event::poll(Duration::from_millis(BURST_ARMED_WATCH_STEP_MS)),
            Ok(true)
        ) {
            // 事件已就绪(未消费):probe=0 立即取到,转入长窗口尾部积累
            return drain_paste_events(pending, 0, PASTE_TAIL_QUIET_MS, PASTE_TAIL_MAX_MS);
        }
    }
    None
}

/// 补全菜单 + Tab/Enter 行为的决策结果(2026-09-10 第 24 轮 Enter 吞键修复)。
/// - `Submit(text)`  : 调用方应以 text 作为输入行提交(进入 handle_user_input);
/// - `AcceptOnly(s)` : 调用方仅把 s 写入输入框,不提交(原「接受补全」语义,Tab 路径);
/// - `None`          : 无操作(补全菜单未开 + Tab,或边界场景)。
#[derive(Debug, Clone, PartialEq, Eq)]
enum CompletionDecision {
    Submit(String),
    AcceptOnly(String),
    None,
}

/// @ 提及补全状态(D1,2026-09-10 第二十八轮,L1427 简化实现)。
///
/// 与斜杠命令补全共用浮层渲染,但语义不同:
/// - 斜杠:replacement 替换**整个 buffer**;
/// - @ 提及:replacement 只拼接替换**当前 @token**(`token_start..cursor`),
///   Tab 接受(目录尾随 `/` 保持浮层继续钻取,文件尾随空格闭合),
///   Enter 原样提交 buffer(不触发 slash 的真前缀自动提交)。
///
/// suggester 懒加载:首次检测到 @token 才构建(首次走 walkdir 快照),
/// 生命周期 = 单次行编辑;工作目录 = 进程 cwd(与工程「工作目录」定义一致)。
struct MentionCompletion {
    suggester: Option<FileSuggester>,
    token_start: Option<usize>,
}

impl MentionCompletion {
    fn new() -> Self {
        Self {
            suggester: None,
            token_start: None,
        }
    }

    /// 检测光标前是否处于 @token 内;是则(必要时懒建 suggester)返回候选。
    fn suggest(&mut self, buffer: &str, cursor: usize) -> Vec<CompletionItem> {
        match mention_token_at(buffer, cursor) {
            Some((start, frag)) => {
                self.token_start = Some(start);
                let sugg = self.suggester.get_or_insert_with(|| {
                    let cwd = std::env::current_dir().unwrap_or_default();
                    FileSuggester::new(&cwd)
                });
                sugg.suggest(&frag)
            }
            None => {
                self.token_start = None;
                Vec::new()
            }
        }
    }

    fn deactivate(&mut self) {
        self.token_start = None;
    }
}

/// 决定 Tab/Enter 在补全菜单中的行为。
///
/// 规则(2026-09-10 第 24 轮):
/// - 补全未开:Enter → Submit(buffer);Tab → None。
/// - 补全已开 + buffer.trim() == replacement.trim():Enter/Tab → Submit(buffer)(已匹配,避免再加尾随空格)。
/// - 补全已开 + Enter + buffer 是 replacement 的真前缀:Submit(replacement)(一键补全并提交,
///   解决 `/provider` 等高频命令需要按 2 次 Enter 才能进入子屏的 UX 问题)。
/// - 补全已开 + Tab:AcceptOnly(replacement)(只补全不提交,符合 Tab 的传统语义)。
/// - 补全已开 + Enter + buffer 不是任何补全项真前缀:Submit(buffer)(兜底:不让 Enter 被吞)。
fn completion_enter_tab_decision(
    key: KeyCode,
    buffer: &str,
    completion_active: bool,
    completion_items: &[CompletionItem],
    completion_index: usize,
) -> CompletionDecision {
    // 补全菜单未开:Enter 直接提交当前 buffer;Tab 无操作
    if !completion_active || completion_items.is_empty() {
        return match key {
            KeyCode::Enter => CompletionDecision::Submit(buffer.to_string()),
            _ => CompletionDecision::None,
        };
    }
    let item = match completion_items.get(completion_index) {
        Some(i) => i,
        None => return CompletionDecision::None,
    };
    let trimmed_buf = buffer.trim();
    let trimmed_rep = item.replacement.trim();
    // 路径 A:已匹配(忽略尾随空格)→ 直接以 buffer 提交
    if trimmed_buf == trimmed_rep {
        return CompletionDecision::Submit(buffer.to_string());
    }
    // 路径 B:Enter + buffer 是补全项真前缀 → 用补全项内容直接提交
    if key == KeyCode::Enter
        && trimmed_rep.starts_with(trimmed_buf)
        && trimmed_rep.len() > trimmed_buf.len()
    {
        return CompletionDecision::Submit(item.replacement.clone());
    }
    // 路径 C:Tab → 接受补全但不提交;Enter 兜底 → 以 buffer 提交,不让 Enter 被吞成空操作
    match key {
        KeyCode::Tab => CompletionDecision::AcceptOnly(item.replacement.clone()),
        KeyCode::Enter => CompletionDecision::Submit(buffer.to_string()),
        _ => CompletionDecision::None,
    }
}


/// 输入处理结果。
pub enum InputResult {
    /// 用户提交了输入行。
    Submitted(String),
    /// 用户请求退出（Ctrl-D 或空输入时 Ctrl-D）。
    Exit,
    /// 中断（Ctrl-C），未提交。
    Interrupted,
}

/// 自定义输入处理器(固定底部输入组件)。
pub struct InputHandler;

impl InputHandler {
    pub fn new() -> Self {
        Self
    }

    /// 读取一行输入,集成补全浮层;输入组件固定屏幕底部。
    ///
    /// # 参数
    /// - `prompt`: 提示符字符串（如 ">> "）
    /// - `engine`: 补全引擎
    pub fn read_line(&self, prompt: &str, engine: &CompletionEngine) -> io::Result<InputResult> {
        // 进入原始模式 + 开启 bracketed paste(终端粘贴整体以 Event::Paste 送达,
        // 不再逐字符事件 —— 对齐 pi editor.ts bracketed paste 模式,L1573)
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout().lock();
        let _ = execute!(stdout, EnableBracketedPaste);
        drop(stdout);

        // 第 104 轮加固:read_line_inner 内部 panic 时(任意 bug 触发)
        // 必须保证 raw mode + bracketed paste 被关掉,主循环错误分支才能正常
        // 清理终端。否则一旦 read_line_inner panic,raw mode 残留 + 滚动区残留 +
        // 焦点失守,Ctrl-C 显式 ^C、Ctrl-D 不退。catch_unwind 把 panic 捕获为
        //    `Err(panic_payload)`,我们把它转成 IO 错误上抛给主循环。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.read_line_inner(prompt, engine)
        }));

        // 无论成功失败,都要退出原始模式 + 关闭 bracketed paste。
        // 这一段即使 panic 也会执行 —— 它在 catch_unwind 的外层。
        let mut stdout = io::stdout().lock();
        let _ = execute!(stdout, DisableBracketedPaste);
        drop(stdout);
        let _ = terminal::disable_raw_mode();

        match result {
            Ok(r) => r,
            Err(panic_payload) => {
                // 不要把 panic 抛出去炸进程,把 panic 信息塞进 IO 错误返回。
                // 主循环错误分支会调用 teardown_pinned + disable_raw_mode 二次兜底。
                let detail = if let Some(s) = panic_payload.downcast_ref::<&'static str>() {
                    Some(*s)
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    Some(s.as_str())
                } else {
                    None
                };
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("read_line_inner panicked: {:?}", detail),
                ));
            }
        }
    }

    fn read_line_inner(&self, prompt: &str, engine: &CompletionEngine) -> io::Result<InputResult> {
        let mut layout = Layout::detect();
        let mut buffer = String::new();
        let mut cursor: usize = 0;
        let mut completion_active = false;
        let mut completion_index: usize = 0;
        let mut completion_items: Vec<CompletionItem> = Vec::new();
        let mut overlay_lines: u16 = 0;
        // @ 提及补全状态(D1):token 起点 + 懒加载文件建议器
        let mut mention = MentionCompletion::new();
        // 粘贴登记簿:本次行编辑期间的多行/超长粘贴原文(提交时 marker 展开为原文)
        let mut pastes = paste::PasteRegistry::new();
        // 第 154 轮:字符到达时刻登记(击键节奏武装的第二信号数据源,见
        // [`char_burst_armed`])—— 无 bracketed paste 终端的粘贴以超人类速率到达,
        // 是「Enter = 粘贴换行 vs 用户提交」最可靠的判据(纯时序探测第三次实测落空)。
        let mut char_arrivals: VecDeque<Instant> = VecDeque::new();
        // 批量合并排空时暂存的非字符事件(crossterm 无 pushback,不能丢)
        let mut pending: Option<Event> = None;

        let stdout = io::stdout();
        let mut stdout = stdout.lock();

        // 第 103 轮:强制重置终端状态,消除前一轮 dispatch_prompt 大量输出后
        // 可能残留的滚动区/光标位置异常(焦点丢失 bug 的根因)。
        // DECRST 重置滚动区为全屏 + 确保光标可见,再进入 pinned 布局。
        // 第 134 轮:光标行查询**必须**在下面这条 DECRST 之前做 ——
        // 实测本机 tmux 下 `ESC [ r` 会让紧随其后的 `ESC [ 6 n` 回包变成 `1;1`
        // (shell 复现:无 ESC[r 时回 `21;1`,加 ESC[r 则无回包),据此定位会把面板
        // 顶到第 2 行并抹掉刚打印的启动横幅。查不到则传 None,enter_pinned 保持吸底。
        stdout.flush()?;
        let cur_row = query_cursor_row();
        execute!(stdout, ResetColor, Print("[r"))?;
        execute!(stdout, crossterm::cursor::Show)?;
        stdout.flush()?;
        // 设置滚动区 + 绘制固定面板 + 空输入行
        self.enter_pinned(&mut stdout, &mut layout, cur_row)?;
        self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;

        loop {
            // 第 136 轮:进 `event::read()` 之前先做一次 0 超时的终端挂断探测。
            // crossterm 0.27 的 `event::read()` 在「fd 可读却读不到字节」(pty master
            // 关闭 → `read()` 恒返回 EOF)时会在 `try_read` 里**无限重试**,既不返回
            // 事件也不返回错误 —— 外部表现为 100% CPU 空转且进程永不退出。
            // 这里提前判定并退出,把它挡在门外(纵深防御:主修复是 term_guard 看门狗,
            // 它负责「已经卡在 read() 里」的场景)。
            if crate::tui::term_guard::stdin_hangup() {
                break;
            }

            // 读取事件(优先取批量合并时暂存的 pending)
            let ev = match pending.take() {
                Some(e) => e,
                None => match event::read() {
                    Ok(e) => e,
                    Err(_) => break,
                },
            };

            match ev {
                // bracketed paste:终端粘贴整体送达(D6/L1573)
                Event::Paste(text) => {
                    let preview = match paste::handle_paste_text(&text, &mut pastes) {
                        paste::PasteInsert::Inline(s) => {
                            buffer.insert_str(cursor, &s);
                            cursor += s.len();
                            None
                        }
                        paste::PasteInsert::Marker(s) => {
                            buffer.insert_str(cursor, &s);
                            cursor += s.len();
                            // 多行/超长粘贴:输入行只放 marker,但用户必须看得见自己粘了什么
                            // (修 R5 可见性半边)—— 立刻在滚动区回显原文前几行预览。
                            let content = pastes.last_content();
                            let preview = paste::plan_paste_preview(
                                &s,
                                &content,
                                textfit::term_width_for_render(),
                            );
                            // 第 131 轮:预览已打印 → 提交时不再全量重复回显同一段原文
                            pastes.mark_previewed();
                            Some(preview)
                        }
                    };
                    overlay_lines = self.update_completion(
                        &mut stdout,
                        &layout,
                        prompt,
                        &buffer,
                        cursor,
                        &mut completion_active,
                        &mut completion_index,
                        &mut completion_items,
                        overlay_lines,
                        engine,
                        &mut mention,
                    )?;
                    if let Some(lines) = &preview {
                        self.print_in_scroll_region(&mut stdout, &layout, lines)?;
                        // 预览动过滚动区底行,重绘输入行确保光标列号与 buffer 一致
                        self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                    }
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-C 双语义(第 108 轮加强):
                            // - 空输入 + Ctrl-C → 触发 shutdown 协调器并退出程序
                            //   (Unix 语义,符合用户肌肉记忆)
                            // - 非空输入 + Ctrl-C → 清空当前行 + 触发 shutdown 协调器
                            //   (root cause:任务运行时 Ctrl+C 走 SIGINT 路径被旧实现
                            //   「只清行」吞掉,导致 Ctrl+C 多次按后 Ctrl-Z suspend 整个
                            //   laew → 终端僵死。本轮统一触发 shutdown 协调器,
                            //   由主循环统一处理退出与终端还原。)
                            if buffer.is_empty() {
                                self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                teardown_pinned_layout(&mut stdout, &layout)?;
                                crate::shutdown::global().trigger(crate::shutdown::ShutdownReason::UserInterrupt);
                                return Ok(InputResult::Exit);
                            } else {
                                self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                self.redraw_line(&mut stdout, &layout, prompt, "", 0)?;
                                execute!(stdout, MoveTo(0, layout.scroll_last_row()))?;
                                stdout.flush()?;
                                crate::shutdown::global().trigger(crate::shutdown::ShutdownReason::UserInterrupt);
                                return Ok(InputResult::Interrupted);
                            }
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-D: 若输入为空则退出(还原滚动区 + 清空面板)
                            // 第 108 轮:同时触发 shutdown 协调器,统一退出语义
                            if buffer.is_empty() {
                                self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                teardown_pinned_layout(&mut stdout, &layout)?;
                                crate::shutdown::global().trigger(crate::shutdown::ShutdownReason::UserInterrupt);
                                return Ok(InputResult::Exit);
                            }
                        }
                        // —— 以下是 readline 标准行编辑快捷键 ——
                        // 标准 readline 中 Ctrl-U=kill to beginning of line, Ctrl-K=kill to end of line,
                        // Ctrl-A=beginning of line, Ctrl-E=end of line, Ctrl-W=kill previous word。
                        // 在 tmux/普通终端下这些按键由 crossterm 映射为带 CONTROL 修饰的 Char,
                        // 必须显式拦截,否则会落入下方 Char(c) 兜底分支被当作普通字符插入(实测 u/model 残留 u 即此问题)。
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-U: 删除到行首(kill to beginning of line)
                            buffer.truncate(0);
                            cursor = 0;
                            overlay_lines = self.update_completion(
                                &mut stdout,
                                &layout,
                                prompt,
                                &buffer,
                                cursor,
                                &mut completion_active,
                                &mut completion_index,
                                &mut completion_items,
                                overlay_lines,
                                engine,
                                &mut mention,
                            )?;
                        }
                        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-K: 删除到行尾(kill to end of line)
                            buffer.truncate(cursor);
                            overlay_lines = self.update_completion(
                                &mut stdout,
                                &layout,
                                prompt,
                                &buffer,
                                cursor,
                                &mut completion_active,
                                &mut completion_index,
                                &mut completion_items,
                                overlay_lines,
                                engine,
                                &mut mention,
                            )?;
                        }
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-A: 光标移到行首(beginning of line)
                            cursor = 0;
                            self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                        }
                        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-E: 光标移到行尾(end of line)
                            cursor = buffer.len();
                            self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                        }
                        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl-W: 删除前一个 word(kill previous word)
                            // word 定义为连续非空白字符序列,前导空白一并清除。
                            let mut new_cursor = cursor;
                            while new_cursor > 0 {
                                let prev = prev_char_boundary(&buffer, new_cursor);
                                if buffer[prev..new_cursor]
                                    .chars()
                                    .next()
                                    .map_or(false, char::is_whitespace)
                                {
                                    new_cursor = prev;
                                } else {
                                    break;
                                }
                            }
                            while new_cursor > 0 {
                                let prev = prev_char_boundary(&buffer, new_cursor);
                                if buffer[prev..new_cursor]
                                    .chars()
                                    .next()
                                    .map_or(false, |c| !char::is_whitespace(c))
                                {
                                    new_cursor = prev;
                                } else {
                                    break;
                                }
                            }
                            if new_cursor != cursor {
                                buffer.replace_range(new_cursor..cursor, "");
                                cursor = new_cursor;
                                overlay_lines = self.update_completion(
                                    &mut stdout,
                                    &layout,
                                    prompt,
                                    &buffer,
                                    cursor,
                                    &mut completion_active,
                                    &mut completion_index,
                                    &mut completion_items,
                                    overlay_lines,
                                    engine,
                                    &mut mention,
                                )?;
                            }
                        }
                        // 2026-09-11 第三十七轮:Ctrl-J(LF,0x0A)拦截。
                        // 原始模式下终端发来的换行字节 LF 被 crossterm 解析为
                        // Char('j')+CONTROL —— tmux send-keys 逐键发送多行文本、
                        // 不支持 bracketed paste 的旧终端逐键粘贴、用户手按 Ctrl-J
                        // 都会到达这里;落入下方 Char(c) 兜底会把字母 j 插入输入缓冲,
                        // 污染多行提示词(实测「slow.py:\n用」回显成「slow.py:j用」)。
                        // 语义与 paste::PasteInsert::Inline 的单行归一(\n → 空格)对齐。
                        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            buffer.insert(cursor, ' ');
                            cursor += 1;
                            overlay_lines = self.update_completion(
                                &mut stdout,
                                &layout,
                                prompt,
                                &buffer,
                                cursor,
                                &mut completion_active,
                                &mut completion_index,
                                &mut completion_items,
                                overlay_lines,
                                engine,
                                &mut mention,
                            )?;
                        }
                        KeyCode::Esc => {
                            // Esc: 关闭补全浮层;非命令输入时清空输入(对齐 bash Esc 语义)
                            if completion_active {
                                // 补全浮层当前可见:关闭浮层,保留输入
                                overlay_lines =
                                    self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                completion_active = false;
                                completion_items.clear();
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            } else if !buffer.is_empty() && !buffer.trim_start().starts_with('/') {
                                // 非命令输入(不以 / 开头):清空输入缓冲区
                                buffer.clear();
                                cursor = 0;
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                            // 命令输入(以 / 开头)且补全未激活时:保留输入(用户可能想按 Enter 提交)
                        }
                        KeyCode::Up => {
                            // 上箭头：在补全列表中向上移动
                            if completion_active && !completion_items.is_empty() {
                                completion_index = if completion_index == 0 {
                                    completion_items.len() - 1
                                } else {
                                    completion_index - 1
                                };
                                overlay_lines = self.draw_overlay(
                                    &mut stdout,
                                    &layout,
                                    completion_index,
                                    &completion_items,
                                )?;
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Down => {
                            // 下箭头：在补全列表中向下移动
                            if completion_active && !completion_items.is_empty() {
                                completion_index = (completion_index + 1) % completion_items.len();
                                overlay_lines = self.draw_overlay(
                                    &mut stdout,
                                    &layout,
                                    completion_index,
                                    &completion_items,
                                )?;
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Tab | KeyCode::Enter => {
                            // @ 提及补全 + Tab(D1):拼接替换当前 token,不动 buffer 其余部分。
                            // 目录(尾随 /)保持浮层继续钻取;文件(尾随空格)闭合浮层。
                            // Enter 不做特殊处理,落到下方 slash 决策的兜底分支 = 原样提交。
                            if key.code == KeyCode::Tab
                                && completion_active
                                && mention.token_start.is_some()
                            {
                                if let (Some(start), Some(item)) =
                                    (mention.token_start, completion_items.get(completion_index))
                                {
                                    let drill = item.replacement.ends_with('/');
                                    let rep = item.replacement.clone();
                                    // start 指向 `@` 本身。第二十八轮修订:replacement 已含 `@` 前缀,
                                    // 故替换区间改为 `start..cursor`(整个 @token 含 @ 一起替换),
                                    // 避免重复 `@`;cursor 落在 replacement 末尾。
                                    buffer.replace_range(start..cursor, &rep);
                                    cursor = start + rep.len();
                                    if drill {
                                        // 继续钻取:按新 fragment 刷新候选
                                        overlay_lines = self.update_completion(
                                            &mut stdout,
                                            &layout,
                                            prompt,
                                            &buffer,
                                            cursor,
                                            &mut completion_active,
                                            &mut completion_index,
                                            &mut completion_items,
                                            overlay_lines,
                                            engine,
                                            &mut mention,
                                        )?;
                                    } else {
                                        completion_active = false;
                                        completion_items.clear();
                                        mention.deactivate();
                                        overlay_lines = self.clear_overlay(
                                            &mut stdout,
                                            &layout,
                                            overlay_lines,
                                        )?;
                                        self.redraw_line(
                                            &mut stdout,
                                            &layout,
                                            prompt,
                                            &buffer,
                                            cursor,
                                        )?;
                                    }
                                }
                                continue;
                            }
                            // Tab/Enter 在补全菜单打开时行为分流(2026-09-10 第 24 轮修复):
                            //  - Tab  永远只「接受补全」,不提交(用户预期);
                            //  - Enter 若 buffer 是当前选中补全项的「真前缀」,则用补全项的完整内容直接
                            //    提交(避免 `/provider` 等高频命令需要按 2 次 Enter 才能进入子屏);
                            //  - Enter 若 buffer 已等于补全项(忽略尾随空格),直接以 buffer 提交
                            //    (容错:与既有路径 A 一致);
                            //  - Enter 若 buffer 不是任何补全项的真前缀,则「接受补全 + 提交原 buffer」
                            //    (兜底:不让 Enter 被吞成空操作)。
                            match completion_enter_tab_decision(
                                key.code,
                                &buffer,
                                completion_active,
                                &completion_items,
                                completion_index,
                            ) {
                                CompletionDecision::Submit(text) => {
                                    // 第 93/94/128/154 轮:无 bracketed paste 终端的多行
                                    // 粘贴突发 —— Enter 是粘贴的行间换行而非用户提交意图
                                    // 时,整段并入粘贴管线,不提交。
                                    //
                                    // 第 154 轮三级判定(纯时序探测第三次实测落空后补第二信号):
                                    //   ① 40ms 队列探测(余行已在终端缓冲,backlog 形态);
                                    //   ② 击键节奏武装态延长观望 300ms(ConPTY 分块滞后形态);
                                    //   ③ 均无 → 正常提交(人类单击 Enter 队列恒空,零额外延迟)。
                                    if key.code == KeyCode::Enter {
                                        let armed =
                                            char_burst_armed(&char_arrivals, Instant::now());
                                        let burst = match drain_paste_burst(&mut pending) {
                                            Some(b) => Some(b),
                                            None if armed => {
                                                watch_armed_paste_tail(&mut pending)
                                            }
                                            None => None,
                                        };
                                        if let Some(burst) = burst {
                                            // ★ 第 94 轮修复(语义保留):burst 仅含空白/
                                            // 控制字符时不视为有效突发,直接提交,避免死循环。
                                            if burst
                                                .chars()
                                                .all(|c| c.is_whitespace() || c.is_control())
                                            {
                                                return self.submit(
                                                    &mut stdout,
                                                    &layout,
                                                    prompt,
                                                    text,
                                                    overlay_lines,
                                                    &pastes,
                                                );
                                            }
                                            // 第 154 轮:整段 marker 化 —— 触发 Enter 前的
                                            // buffer 作为首行前缀,与尾部全文拼成完整粘贴走
                                            // D6 管线。旧实现只登记 burst(余行),marker 展开后
                                            // 首行与第二行被拼成一行(分隔符随触发 Enter 一起
                                            // 被消费);前缀拼接后 buffer 只放 marker,UX 与
                                            // bracketed paste 路径完全一致。
                                            let prefix = buffer.clone();
                                            let full = if prefix.trim().is_empty() {
                                                burst
                                            } else {
                                                format!("{prefix}\n{burst}")
                                            };
                                            buffer.clear();
                                            cursor = 0;
                                            let preview =
                                                match paste::handle_paste_text(&full, &mut pastes)
                                                {
                                                    paste::PasteInsert::Inline(s) => {
                                                        buffer.insert_str(cursor, &s);
                                                        cursor += s.len();
                                                        None
                                                    }
                                                    paste::PasteInsert::Marker(s) => {
                                                        buffer.insert_str(cursor, &s);
                                                        cursor += s.len();
                                                        let content = pastes.last_content();
                                                        let preview = paste::plan_paste_preview(
                                                            &s,
                                                            &content,
                                                            textfit::term_width_for_render(),
                                                        );
                                                        // 第 131 轮:预览已打印 → 提交时不重复全量回显
                                                        pastes.mark_previewed();
                                                        Some(preview)
                                                    }
                                                };
                                            overlay_lines = self.update_completion(
                                                &mut stdout,
                                                &layout,
                                                prompt,
                                                &buffer,
                                                cursor,
                                                &mut completion_active,
                                                &mut completion_index,
                                                &mut completion_items,
                                                overlay_lines,
                                                engine,
                                                &mut mention,
                                            )?;
                                            if let Some(lines) = &preview {
                                                self.print_in_scroll_region(
                                                    &mut stdout,
                                                    &layout,
                                                    lines,
                                                )?;
                                                self.redraw_line(
                                                    &mut stdout,
                                                    &layout,
                                                    prompt,
                                                    &buffer,
                                                    cursor,
                                                )?;
                                            }
                                            continue;
                                        }
                                    }
                                    return self.submit(
                                        &mut stdout,
                                        &layout,
                                        prompt,
                                        text,
                                        overlay_lines,
                                        &pastes,
                                    );
                                }
                                CompletionDecision::AcceptOnly(replacement) => {
                                    buffer = replacement;
                                    cursor = buffer.len();
                                    completion_active = false;
                                    completion_items.clear();
                                    overlay_lines =
                                        self.clear_overlay(&mut stdout, &layout, overlay_lines)?;
                                    self.redraw_line(
                                        &mut stdout,
                                        &layout,
                                        prompt,
                                        &buffer,
                                        cursor,
                                    )?;
                                }
                                CompletionDecision::None => {
                                    // 补全未开 + Tab:无操作(保持静默);
                                    // 补全未开 + Enter:辅助函数已返回 Submit(text=buffer),
                                    // 不会到这里,所以下方兜底不会再跑到。
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            // 退格：删除光标前字符(cursor 为字节偏移,先回退到前一个
                            // 字符的起始边界再删,保证多字节 CJK 不触发 is_char_boundary panic)
                            if cursor > 0 {
                                cursor = prev_char_boundary(&buffer, cursor);
                                buffer.remove(cursor);
                                overlay_lines = self.update_completion(
                                    &mut stdout,
                                    &layout,
                                    prompt,
                                    &buffer,
                                    cursor,
                                    &mut completion_active,
                                    &mut completion_index,
                                    &mut completion_items,
                                    overlay_lines,
                                    engine,
                                    &mut mention,
                                )?;
                            }
                        }
                        // 兜底：某些终端环境下 crossterm 未将退格映射为 KeyCode::Backspace，
                        // 而是作为 Char('\x7f') (DEL) 或 Char('\x08') (BS) 传递。
                        // 此处显式拦截，避免退格字符落入 Char(c) 分支被当作普通字符插入。
                        KeyCode::Char('\x7f') | KeyCode::Char('\x08') => {
                            if cursor > 0 {
                                cursor = prev_char_boundary(&buffer, cursor);
                                buffer.remove(cursor);
                                overlay_lines = self.update_completion(
                                    &mut stdout,
                                    &layout,
                                    prompt,
                                    &buffer,
                                    cursor,
                                    &mut completion_active,
                                    &mut completion_index,
                                    &mut completion_items,
                                    overlay_lines,
                                    engine,
                                    &mut mention,
                                )?;
                            }
                        }
                        KeyCode::Delete => {
                            // Delete：删除光标处字符(cursor 在字符边界,remove 按字节偏移删一个字符)
                            if cursor < buffer.len() {
                                buffer.remove(cursor);
                                overlay_lines = self.update_completion(
                                    &mut stdout,
                                    &layout,
                                    prompt,
                                    &buffer,
                                    cursor,
                                    &mut completion_active,
                                    &mut completion_index,
                                    &mut completion_items,
                                    overlay_lines,
                                    engine,
                                    &mut mention,
                                )?;
                            }
                        }
                        KeyCode::Left => {
                            // 左箭头：移动光标(按字符,不按字节)
                            if cursor > 0 {
                                cursor = prev_char_boundary(&buffer, cursor);
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Right => {
                            // 右箭头：移动光标(前进一个 UTF-8 字符)
                            if cursor < buffer.len() {
                                cursor +=
                                    buffer[cursor..].chars().next().map_or(0, |c| c.len_utf8());
                                self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                            }
                        }
                        KeyCode::Home => {
                            cursor = 0;
                            self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                        }
                        KeyCode::End => {
                            cursor = buffer.len();
                            self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                        }
                        KeyCode::Char(c) => {
                            // 通用 CONTROL 防护(2026-09-11 第三十七轮):未被上方
                            // 显式绑定的 Ctrl 组合键(Ctrl-T/Ctrl-N/…)一律不作为
                            // 文本插入(readline 语义:未绑定的 Ctrl 组合不产生字符),
                            // 防止 crossterm 把控制字节解析成 Char(letter)+CONTROL
                            // 后落入此处插入字母垃圾。Shift/Alt/无修饰不受影响。
                            if key.modifiers.contains(KeyModifiers::CONTROL) {
                                continue;
                            }
                            // 可打印字符：插入到光标位置(cursor 为字节偏移,
                            // 增量按 len_utf8 推进——修复中文输入第 2 字符 panic)
                            buffer.insert(cursor, c);
                            cursor += c.len_utf8();
                            note_char_arrival(&mut char_arrivals);
                            // D6 快速输入批量合并:IME 一次上屏多字 / 无 bracketed paste
                            // 的旧终端粘贴,会以极快的连续 Char 事件到达;排空已到达的
                            // 同类事件一次性插入,最后统一一次重绘,避免逐字重绘卡顿。
                            // 非字符事件存入 pending,下轮循环优先处理(不丢事件)。
                            loop {
                                // 第 136 轮:同 read_line_inner 主循环,排空快速输入前
                                // 先确认终端没消失 —— 终端挂断时 `poll` 恒返回 true,
                                // 下面的 `event::read()` 会陷入 crossterm 内部死转。
                                if crate::tui::term_guard::stdin_hangup() {
                                    break;
                                }
                                match event::poll(Duration::ZERO) {
                                    Ok(true) => match event::read() {
                                        Ok(Event::Key(k2))
                                            if k2.kind == KeyEventKind::Press
                                                && matches!(k2.code, KeyCode::Char(_))
                                                && (k2.modifiers.is_empty()
                                                    || k2.modifiers == KeyModifiers::SHIFT) =>
                                        {
                                            if let KeyCode::Char(c2) = k2.code {
                                                buffer.insert(cursor, c2);
                                                cursor += c2.len_utf8();
                                                // 同批排空的字符与首字符同窗口到达,
                                                // 节奏登记一次即代表本批(粘贴整块到达)。
                                                note_char_arrival(&mut char_arrivals);
                                            }
                                        }
                                        Ok(other) => {
                                            pending = Some(other);
                                            break;
                                        }
                                        Err(_) => break,
                                    },
                                    _ => break,
                                }
                            }
                            overlay_lines = self.update_completion(
                                &mut stdout,
                                &layout,
                                prompt,
                                &buffer,
                                cursor,
                                &mut completion_active,
                                &mut completion_index,
                                &mut completion_items,
                                overlay_lines,
                                engine,
                                &mut mention,
                            )?;
                        }
                        _ => {}
                    }
                }
                Event::Resize(cols, rows) => {
                    // 终端尺寸变化:重算布局 → 重设滚动区 → 全量重绘面板/浮层/输入行
                    layout = Layout::compute(cols, rows);
                    let cur_row = query_cursor_row();
                    self.enter_pinned(&mut stdout, &mut layout, cur_row)?;
                    overlay_lines = if completion_active && !completion_items.is_empty() {
                        self.draw_overlay(
                            &mut stdout,
                            &layout,
                            completion_index,
                            &completion_items,
                        )?
                    } else {
                        0
                    };
                    self.redraw_line(&mut stdout, &layout, prompt, &buffer, cursor)?;
                }
                _ => {}
            }
        }

        Ok(InputResult::Exit)
    }
    /// 提交:清浮层 → 滚动区底行回显已提交内容 → 清空面板输入行 → 光标锚定滚动区底行。
    ///
    /// 2026-09-24 修 R5:回显改用 **展开版**逐行打印(首行 `>> `、续行 `.. `、
    /// 折行悬挂缩进,超 `SUBMIT_ECHO_MAX_LINES` 折叠并标注「内容已完整发送」)——
    /// 旧实现回显的是 marker 版单行 buffer,用户粘贴的多行提示词在屏幕上只剩一行,
    /// 与「送进模型的到底是什麼」完全对不上。返回的 `Submitted` 仍是展开还原版
    /// (marker → 原文,超大粘贴截断注入,见 `PasteRegistry::expand`)。
    ///
    /// 第 131 轮「粘贴只显示一遍」:粘贴路径的原文在**粘贴瞬间**已经打印过
    /// [`paste::plan_paste_preview`] 预览,此处若再全量回显等于同一段文字连着出现两次
    /// (实测 8 行提示词观感像「粘了两遍」)。故含已预览粘贴时改为回显输入行 marker
    /// 形态 + 一行「已按原文完整发送 N 行 / M 字」;纯键盘输入路径不变。
    #[allow(clippy::too_many_arguments)]
    fn submit(
        &self,
        stdout: &mut impl Write,
        layout: &Layout,
        prompt: &str,
        buffer: String,
        overlay_lines: u16,
        pastes: &paste::PasteRegistry,
    ) -> io::Result<InputResult> {
        self.clear_overlay(stdout, layout, overlay_lines)?;
        // 回显到滚动区底行(保留用户输入痕迹,随历史输出一起滚动):
        // 展开版逐行回显,多行提示词在屏幕上完整可读,不再只剩首行。
        let expanded = pastes.expand(&buffer);
        let echo = paste::plan_submit_echo_with_paste(
            prompt,
            &buffer,
            &expanded,
            textfit::term_width_for_render(),
            pastes.expands_previewed_paste(&buffer),
        );
        self.print_in_scroll_region(stdout, layout, &echo)?;
        // 面板输入行清空(组件常显)
        self.redraw_line(stdout, layout, prompt, "", 0)?;
        // 输出光标锚定滚动区底行,后续 println! 任务输出在滚动区内滚动
        execute!(stdout, MoveTo(0, layout.scroll_last_row()))?;
        stdout.flush()?;
        Ok(InputResult::Submitted(expanded))
    }

    /// 更新补全浮层（输入变化时调用）:先清旧浮层,再按新候选重绘,最后重绘输入行。
    /// 返回新的浮层占用行数。
    #[allow(clippy::too_many_arguments)]
    fn update_completion(
        &self,
        stdout: &mut impl Write,
        layout: &Layout,
        prompt: &str,
        buffer: &str,
        cursor: usize,
        active: &mut bool,
        index: &mut usize,
        items: &mut Vec<CompletionItem>,
        overlay_lines: u16,
        engine: &CompletionEngine,
        mention: &mut MentionCompletion,
    ) -> io::Result<u16> {
        let mut lines = self.clear_overlay(stdout, layout, overlay_lines)?;

        // 斜杠命令补全优先(输入以 '/' 开头且有后续字符);
        // 否则检测 @ 提及 token(D1,文件路径实时补全)
        let trimmed = buffer.trim_start();
        let new_items = if trimmed.starts_with('/') && trimmed.len() >= 2 {
            mention.deactivate();
            engine.complete(trimmed)
        } else {
            mention.suggest(buffer, cursor)
        };

        if new_items.is_empty() {
            *active = false;
            items.clear();
        } else {
            *items = new_items;
            *index = 0;
            *active = true;
            lines = self.draw_overlay(stdout, layout, *index, items)?;
        }

        self.redraw_line(stdout, layout, prompt, buffer, cursor)?;
        Ok(lines)
    }
}

impl Default for InputHandler {
    fn default() -> Self {
        Self::new()
    }
}

mod panel;
#[cfg(test)]
mod tests;

pub use panel::{teardown_pinned, Layout};
