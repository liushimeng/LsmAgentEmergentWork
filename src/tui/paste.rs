//! 粘贴保真层(D6,2026-09-10 第二十二轮立项;2026-09-24 修 R5 扩展为「多行保真」)
//!
//! 从 `src/tui/input.rs` 拆出的职责子模块(单文件 ≤1800 行规范,input.rs 达 2100+):
//! - [`PasteRegistry`] —— 单次行编辑期间的粘贴登记簿(原文含换行完整保留,提交时展开);
//! - [`handle_paste_text`] —— 粘贴统一入口:过滤 → 保真判定 → 直插(单行)或 marker;
//! - [`plan_paste_preview`] / [`plan_submit_echo`] / [`plan_submit_echo_with_paste`] —— 纯函数回显排版(可单测,零 IO)。
//!
//! 参考:pi editor.ts handlePaste(>10 行或 >1000 字符转 marker,提交注入原文,L1573)
//!      + claudecode inputPaste.ts(TRUNCATION_THRESHOLD=10000,首尾各 500 截断,L1448)

use crate::tui::textfit;

/// 单行粘贴转 marker 的字符阈值(对齐 pi handlePaste);
/// 多行粘贴(≥2 行)**无条件**转 marker 保真,不受此阈值约束(修 R5)。
const PASTE_MARKER_MAX_CHARS: usize = 1000;
/// 提交注入截断阈值:单份粘贴超过此字符数时截断注入(对齐 claudecode TRUNCATION_THRESHOLD)。
const PASTE_TRUNCATE_CHARS: usize = 10000;
/// 截断注入保留首/尾字符数(对齐 claudecode PREVIEW_LENGTH/2)。
const PASTE_KEEP_HEAD_CHARS: usize = 500;
const PASTE_KEEP_TAIL_CHARS: usize = 500;

/// 从头截取不超过 `n` 个字符的子串(char 边界安全,不在多字节字符中间截断)。
fn head_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// 从尾截取不超过 `n` 个字符的子串(char 边界安全)。
fn tail_chars(s: &str, n: usize) -> &str {
    let total = s.chars().count();
    if total <= n {
        return s;
    }
    match s.char_indices().nth(total - n) {
        Some((idx, _)) => &s[idx..],
        None => s,
    }
}

/// 粘贴登记簿:生命周期 = 单次 read_line 调用,提交即展开。
///
/// 多行/超长粘贴不进输入行(避免 1000 行淹没编辑器 + 逐帧 O(N) 宽度换算卡顿),
/// 输入行只显示 `[粘贴 #N +M 行]`(多行) / `[粘贴 #N M 字符]`(超长单行) marker;
/// 提交时精确匹配 marker 展开还原原文(pi paste ID 校验思想:
/// 被用户编辑损坏的 marker 不匹配、原样保留)。
pub(crate) struct PasteRegistry {
    counter: usize,
    /// (marker, 完整原文)
    entries: Vec<(String, String)>,
    /// 已登记但**未在滚动区回显过预览**的 marker 集合(第 131 轮)。
    ///
    /// 当前两个登记调用点(`input.rs` bracketed paste 与粘贴突发)都在登记后立刻打印
    /// [`plan_paste_preview`],故正常路径下登记即预览;保留本字段是为「将来允许静默登记
    /// (不打印预览)」留位,提交回显据此判断能否跳过全量重复。
    unpreviewed: Vec<String>,
}

impl PasteRegistry {
    pub(crate) fn new() -> Self {
        Self {
            counter: 0,
            entries: Vec::new(),
            unpreviewed: Vec::new(),
        }
    }

    /// 过滤粘贴文本:先做换行归一化(CRLF/CR → LF),再剔除其余控制字符(保留 \n 与 \t)。
    ///
    /// 换行归一化的必要性(2026-09-10 第 23 轮实测):tmux 的 bracketed paste 把
    /// 粘贴内容按按键语义回放,LF 全部转为 CR(LF=0/CR=N,探针 hex 取证);桌面终端
    /// 则保留 LF/CRLF。若不归一化,CR 会在下方控制字符过滤中被剔除,换行信息全丢 →
    /// 大粘贴行数判定恒为 1 行、marker 永不触发,小粘贴「换行转空格」归一也失效,
    /// 内容被拼成一行(D6 防护在 tmux 下完全失效)。
    fn filter(text: &str) -> String {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        normalized
            .chars()
            .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
            .collect()
    }

    /// 判定是否需要转 marker —— **两条**任一命中:
///
/// 1. **多行(≥2 行)**。2026-09-24 修 R5:旧阈值是「>10 行」,于是用户最常做的
///    3~8 行 Markdown 提示词(带 `1.` `2.` `3.` 列表)被判「小粘贴」→ 走 Inline 归一,
///    **换行符全被替换成空格**:屏幕上只露出水平滚动窗口里的一行(用户抱怨的
///    「粘贴后看不见自己粘了什么」),送进模型的提示词也丢了列表结构。
///    现按「≥2 行即保真」处理:原文(含 `\n`)进登记表,输入行只放 marker。
/// 2. 单行但超 `PASTE_MARKER_MAX_CHARS`(旧 D6 语义不变,防超长刷屏)。
    fn is_large(filtered: &str) -> bool {
        filtered.contains('\n') || filtered.chars().count() > PASTE_MARKER_MAX_CHARS
    }

    /// 登记保真粘贴,返回输入行用 marker。
    ///
    /// 首尾换行剔除(逐键粘贴突发会补一个前导 `\n`,提交侧本就要 trim,提前归一
    /// 让 marker / 预览 / 回显三者与最终送进模型的文本完全一致)。
    fn register(&mut self, content: String) -> String {
        self.counter += 1;
        let content = content.trim_matches(['\n', '\r']).to_string();
        let lines = content.matches('\n').count() + 1;
        let marker = if lines > 1 {
            format!("[粘贴 #{} +{} 行]", self.counter, lines)
        } else {
            format!("[粘贴 #{} {} 字符]", self.counter, content.chars().count())
        };
        self.entries.push((marker.clone(), content));
        // 先记为「未预览」,由调用点在打印预览后调 [`Self::mark_previewed`] 销去。
        self.unpreviewed.push(marker.clone());
        marker
    }

    /// 最近一份登记的粘贴原文(供「粘贴即预览」回显用;不外露 `entries` 字段)。
    pub(crate) fn last_content(&self) -> String {
        self.entries.last().map(|(_, c)| c.clone()).unwrap_or_default()
    }

    /// 标记最近一份登记已回显过预览(登记调用点在打印 [`plan_paste_preview`] 后调用)。
    pub(crate) fn mark_previewed(&mut self) {
        if let Some((marker, _)) = self.entries.last() {
            self.unpreviewed.retain(|m| m != marker);
        }
    }

    /// 本次提交里是否发生过「已预览粘贴的展开」——
    /// 决定提交回显是走**输入行 marker 版**(不重复全文,只打一行说明)
    /// 还是走展开版全量回显(第 131 轮「粘贴只显示一遍」)。
    pub(crate) fn expands_previewed_paste(&self, buffer: &str) -> bool {
        self.entries.iter().any(|(marker, content)| {
            buffer.contains(marker.as_str())
                && !self.unpreviewed.iter().any(|m| m == marker)
                && !content.is_empty()
        })
    }

    /// 提交时展开:精确匹配 marker → 原文;单份 >10000 字符截断为
    /// 首 500 + 省略标注 + 尾 500(claudecode inputPaste 语义,防超大粘贴打爆上下文)。
    /// marker 被编辑损坏/编号未知则不展开,原样保留。
    pub(crate) fn expand(&self, buffer: &str) -> String {
        let mut out = buffer.to_string();
        for (marker, content) in &self.entries {
            if !out.contains(marker.as_str()) {
                continue;
            }
            let inject = Self::truncate_for_inject(content);
            out = out.replace(marker.as_str(), &inject);
        }
        out
    }

    /// 截断注入:超过阈值保留首尾各 500 字符,中间以省略标注替换。
    fn truncate_for_inject(content: &str) -> String {
        let total = content.chars().count();
        if total <= PASTE_TRUNCATE_CHARS {
            return content.to_string();
        }
        let omitted = total - PASTE_KEEP_HEAD_CHARS - PASTE_KEEP_TAIL_CHARS;
        format!(
            "{}\n[...中间省略 {} 字符...]\n{}",
            head_chars(content, PASTE_KEEP_HEAD_CHARS),
            omitted,
            tail_chars(content, PASTE_KEEP_TAIL_CHARS),
        )
    }
}

/// 粘贴入缓冲区的结果。
pub(crate) enum PasteInsert {
    /// 小粘贴:归一化文本直接插入(\n/\t 已转空格,单行输入语义)。
    Inline(String),
    /// 已登记进登记表:输入行只插入 marker。
    Marker(String),
}

/// 粘贴统一入口:过滤 → 保真判定 → 直插(仅单行)或登记 marker(多行/超长)。
///
/// 判定前剔除首尾换行:bracketed paste 路径可能在段首尾带一个代表 Enter 的换行,
/// 不剔就会把「一次 Enter + 一行内容」误判成多行粘贴、给短输入套上噪声 marker。
/// burst 路径(逐键粘贴突发)的 text 由 drain_paste_burst 拼接而来,内部已含原始
/// 换行(\n),由调用方保证首尾不含多余换行——handle_paste_text 统一 trim 后不影响
/// burst 路径的多行判定(内部 \n 仍在)。
pub(crate) fn handle_paste_text(text: &str, registry: &mut PasteRegistry) -> PasteInsert {
    let filtered = PasteRegistry::filter(text);
    let probe = filtered.trim_matches(|c| c == '\n' || c == '\r');
    if PasteRegistry::is_large(probe) {
        PasteInsert::Marker(registry.register(filtered))
    } else {
        // 走到这里必然不含换行(is_large 已把多行分流);制表符归一为空格 ——
        // 单行输入行的光标列号按空格换算,tab 会让列号错位。
        PasteInsert::Inline(probe.replace('\t', " ").to_string())
    }
}

/// 粘贴预览行数上限:注册 marker 后立刻在滚动区回显前 N 行,让用户当场确认粘对了。
const PASTE_PREVIEW_LINES: usize = 4;
/// 提交回显的视觉行数上限(超出折叠为一行「省略 N 行」标注)。
const SUBMIT_ECHO_MAX_LINES: usize = 20;

/// 粘贴预览行(纯函数,便于单测):marker 说明 + 原文前 [`PASTE_PREVIEW_LINES`] 行。
///
/// 只是回显,不改 marker 语义:输入行仍是 marker,提交时才展开原文。
pub(crate) fn plan_paste_preview(marker: &str, content: &str, term_w: usize) -> Vec<String> {
    let inner = term_w.saturating_sub(6).max(12);
    let lines: Vec<&str> = content.split('\n').collect();
    let mut out = vec![format!(
        "   {marker} 原文已保留({} 行 / {} 字),提交时完整回显:",
        lines.len(),
        content.chars().count()
    )];
    for src in lines.iter().take(PASTE_PREVIEW_LINES) {
        let parts = textfit::wrap(src, inner);
        let parts: Vec<String> = if parts.is_empty() { vec![String::new()] } else { parts };
        for (i, part) in parts.into_iter().enumerate() {
            let pfx = if i == 0 { "   .. " } else { "      " };
            out.push(format!("{pfx}{part}"));
        }
    }
    if lines.len() > PASTE_PREVIEW_LINES {
        out.push(format!("   .. [… 其余 {} 行未显示]", lines.len() - PASTE_PREVIEW_LINES));
    }
    out
}

/// 提交回显排版内核(纯函数):把 `text` 的每个源行按 `>> `/`.. ` 前缀 + 悬挂缩进折行,
/// 视觉行超 [`SUBMIT_ECHO_MAX_LINES`] 时折叠并标注省略了多少**源行**。
fn layout_submit_lines(prompt: &str, text: &str, term_w: usize) -> Vec<String> {
    let pfx_w = textfit::width(prompt).max(3);
    let inner = term_w.saturating_sub(pfx_w).max(12);
    let src: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    for (idx, line) in src.iter().enumerate() {
        let head = if idx == 0 { prompt.to_string() } else { ".. ".to_string() };
        let mut parts = textfit::wrap(line, inner);
        if parts.is_empty() {
            parts.push(String::new());
        }
        for (i, part) in parts.into_iter().enumerate() {
            out.push(if i == 0 { format!("{head}{part}") } else { format!("   {part}") });
        }
    }
    if out.len() > SUBMIT_ECHO_MAX_LINES {
        // 折叠:保留预算内的整行,尾部标注省略了多少**源行**(不是视觉行,便于用户核对)
        let shown_src = out[..SUBMIT_ECHO_MAX_LINES]
            .iter()
            .filter(|l| l.starts_with(prompt) || l.starts_with(".. "))
            .count();
        out.truncate(SUBMIT_ECHO_MAX_LINES - 1);
        out.push(format!(
            "   [… 省略 {} 行(共 {} 行),内容已完整发送]",
            src.len().saturating_sub(shown_src),
            src.len()
        ));
    }
    out
}

/// 提交回显行(纯函数,便于单测):首行 `prompt`(如 `>> `)前缀,其余源行 `.. ` 前缀,
/// 按终端宽度折行(悬挂缩进 3 空格),视觉行超 [`SUBMIT_ECHO_MAX_LINES`] 时折叠并标注。
///
/// 关键:传入的是 **marker 展开版**(送进模型的原文),所以屏幕上看到的即模型看到的。
/// 旧实现回显的是已把换行压成空格的 marker 版 buffer,多行提示词只剩一行。
pub(crate) fn plan_submit_echo(prompt: &str, expanded: &str, term_w: usize) -> Vec<String> {
    layout_submit_lines(prompt, expanded, term_w)
}

/// 提交回显行(第 131 轮「粘贴只显示一遍」版)。
///
/// 粘贴保真路径上同一段原文会被打印两遍:粘贴瞬间的 [`plan_paste_preview`] 前 4 行预览
/// + 提交时的全量展开回显。实测 8 行提示词在屏幕上连着出现两次,观感像「粘了两遍」。
///
/// 决策:**保留粘贴时的预览**(它是「我粘对了没」的即时反馈),提交时改为回显**输入行原样**
/// (marker 形态)+ 一行「已按原文完整发送 N 行 / M 字」说明,不再重复全文。
/// 纯键盘输入(无 marker 展开)路径完全不变,仍走 [`plan_submit_echo`] 全量回显。
pub(crate) fn plan_submit_echo_with_paste(
    prompt: &str,
    buffer: &str,
    expanded: &str,
    term_w: usize,
    skip_full_echo: bool,
) -> Vec<String> {
    if !skip_full_echo {
        return plan_submit_echo(prompt, expanded, term_w);
    }
    let mut out = layout_submit_lines(prompt, buffer, term_w);
    let src_lines = expanded.split('\n').count();
    out.push(format!(
        "   [粘贴原文已在粘贴时预览,此处不重复回显;已按原文完整发送 {src_lines} 行 / {} 字]",
        expanded.chars().count()
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_filter_strips_control_chars_keeps_newline() {
        assert_eq!(PasteRegistry::filter("a\x07b\x01c\nd"), "abc\nd");
        assert_eq!(PasteRegistry::filter("\x1b[31m红\x1b[0m"), "[31m红[0m");
        assert_eq!(PasteRegistry::filter("正常文本"), "正常文本");
    }

    #[test]
    fn paste_filter_normalizes_cr_and_crlf_to_lf() {
        // tmux bracketed paste 把 LF 转为 CR 按键语义回放(第 23 轮探针取证 LF=0/CR=N):
        // CR 必须归一为 LF,否则行数判定恒 1、marker 永不触发,内容拼成一行。
        assert_eq!(PasteRegistry::filter("a\r\nb\rc"), "a\nb\nc");
        // 纯 CR 多行 → 大粘贴行数判定依据
        let tmux_paste: String = (1..=15).map(|i| format!("第{i}行\r")).collect();
        assert_eq!(PasteRegistry::filter(&tmux_paste).matches('\n').count(), 15);
    }

    #[test]
    fn large_paste_with_tmux_cr_newlines_gets_marker() {
        // 复现本轮 Bug:15 行粘贴以 CR 分隔(tmux 实际形态)必须触发 marker
        let mut reg = PasteRegistry::new();
        let tmux_text: String = (1..=15)
            .map(|i| format!("第{i}行:大粘贴防护测试内容-{i}"))
            .collect::<Vec<_>>()
            .join("\r");
        match handle_paste_text(&tmux_text, &mut reg) {
            PasteInsert::Marker(m) => assert_eq!(m, "[粘贴 #1 +15 行]"),
            PasteInsert::Inline(_) => panic!("tmux CR 形态 15 行粘贴应转 marker"),
        }
        // 展开注入的原文应含 LF 归一化后的完整行
        let expanded = reg.expand("[粘贴 #1 +15 行]");
        assert_eq!(expanded.matches('\n').count(), 14);
    }

    #[test]
    fn 多行粘贴不再压平_改走_marker_保真() {
        // 修 R5:旧规则「≤10 行且 ≤1000 字符」把换行全替换成空格,用户 3~8 行的
        // Markdown 提示词在屏幕上只剩一行、送进模型也丢了列表结构。
        let mut reg = PasteRegistry::new();
        match handle_paste_text("第一行\n第二行\tend", &mut reg) {
            PasteInsert::Marker(m) => assert_eq!(m, "[粘贴 #1 +2 行]", "marker 应标出行数"),
            PasteInsert::Inline(s) => panic!("多行粘贴不应再被压平成: {s}"),
        }
        // 展开后与原文逐字节一致(含 \n 与 \t)—— 送进模型的就是用户粘的东西
        assert_eq!(reg.expand("[粘贴 #1 +2 行]"), "第一行\n第二行\tend");
    }

    #[test]
    fn 单行粘贴仍直插_仅制表符归一() {
        let mut reg = PasteRegistry::new();
        match handle_paste_text("hello 粘贴", &mut reg) {
            PasteInsert::Inline(s) => assert_eq!(s, "hello 粘贴"),
            PasteInsert::Marker(m) => panic!("单行短粘贴不应转 marker: {m}"),
        }
        assert!(reg.entries.is_empty());
        match handle_paste_text("a\tb", &mut reg) {
            PasteInsert::Inline(s) => assert_eq!(s, "a b", "单行 tab 归一为空格(光标列号)"),
            PasteInsert::Marker(_) => panic!("单行不应转 marker"),
        }
    }

    #[test]
    fn 粘贴突发的前导换行不入登记表() {
        // 逐键粘贴路径会补一个前导 \n 表示刚按下的 Enter;登记时须剔除,
        // 否则送进模型的文本莫名多一个空行。
        let mut reg = PasteRegistry::new();
        match handle_paste_text("\n第一行\n第二行\n", &mut reg) {
            PasteInsert::Marker(_) => {}
            PasteInsert::Inline(s) => panic!("应转 marker,实得直插 {s}"),
        }
        assert_eq!(reg.expand("[粘贴 #1 +2 行]"), "第一行\n第二行");
    }

    #[test]
    fn 粘贴预览行数受控且标注销去行数() {
        let content = (1..=9).map(|i| format!("第{i}行内容")).collect::<Vec<_>>().join("\n");
        let lines = plan_paste_preview("[粘贴 #1 +9 行]", &content, 80);
        assert!(lines[0].contains("原文已保留(9 行"));
        assert_eq!(lines.len(), 1 + 4 + 1, "应给 4 行正文 + 1 行省略标注:\n{lines:?}");
        assert!(lines.last().unwrap().contains("其余 5 行未显示"));
        assert!(lines.iter().all(|l| textfit::width(l) <= 80));
    }

    #[test]
    fn 提交回显逐行展开且首行带提示符() {
        let expanded = "### 标题\n1. 第一步\n2. 第二步";
        let lines = plan_submit_echo(">> ", expanded, 80);
        assert_eq!(lines[0], ">> ### 标题");
        assert_eq!(lines[1], ".. 1. 第一步");
        assert_eq!(lines[2], ".. 2. 第二步");
    }

    #[test]
    fn 提交回显长行折行用悬挂缩进() {
        let long = "x".repeat(200);
        let lines = plan_submit_echo(">> ", &long, 80);
        assert!(lines.len() > 1, "长行应折行");
        assert!(lines[0].starts_with(">> "));
        assert!(lines[1].starts_with("   "), "续行应悬挂缩进: {:?}", lines[1]);
        assert!(lines.iter().all(|l| textfit::width(l) <= 80));
        // 折行不丢字符:全部视觉行的 x 加起来仍是 200
        assert_eq!(lines.iter().map(|l| l.chars().filter(|c| *c == 'x').count()).sum::<usize>(), 200);
    }

    #[test]
    fn 提交回显超长内容折叠并标注已完整发送() {
        let content = (1..=40).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        let lines = plan_submit_echo(">> ", &content, 100);
        assert_eq!(lines.len(), SUBMIT_ECHO_MAX_LINES, "应恰好折到上限行数");
        assert!(
            lines[SUBMIT_ECHO_MAX_LINES - 1].contains("内容已完整发送"),
            "尾部应有省略标注: {:?}",
            lines[SUBMIT_ECHO_MAX_LINES - 1]
        );
    }

    #[test]
    fn large_paste_by_lines_gets_marker() {
        let mut reg = PasteRegistry::new();
        let text = (1..=11)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        match handle_paste_text(&text, &mut reg) {
            PasteInsert::Marker(m) => assert_eq!(m, "[粘贴 #1 +11 行]"),
            PasteInsert::Inline(_) => panic!("11 行应转 marker"),
        }
        assert_eq!(reg.entries.len(), 1);
    }

    #[test]
    fn large_paste_by_chars_gets_marker() {
        let mut reg = PasteRegistry::new();
        let text = "a".repeat(1001); // 单行超 1000 字符
        match handle_paste_text(&text, &mut reg) {
            PasteInsert::Marker(m) => assert_eq!(m, "[粘贴 #1 1001 字符]"),
            PasteInsert::Inline(_) => panic!("1001 字符应转 marker"),
        }
    }

    #[test]
    fn paste_counter_increments() {
        let mut reg = PasteRegistry::new();
        let text = "a".repeat(2000);
        let m1 = match handle_paste_text(&text, &mut reg) {
            PasteInsert::Marker(m) => m,
            _ => panic!(),
        };
        let m2 = match handle_paste_text(&text, &mut reg) {
            PasteInsert::Marker(m) => m,
            _ => panic!(),
        };
        assert!(m1.contains("#1"));
        assert!(m2.contains("#2"));
    }

    #[test]
    fn expand_restores_marker_to_original() {
        let mut reg = PasteRegistry::new();
        let text = "x".repeat(2000);
        let marker = reg.register(text.clone());
        let buf = format!("帮我分析 {marker} 谢谢");
        let expanded = reg.expand(&buf);
        assert_eq!(expanded, format!("帮我分析 {text} 谢谢"));
    }

    #[test]
    fn expand_truncates_oversized_paste() {
        let mut reg = PasteRegistry::new();
        let text = "0123456789".repeat(1200); // 12000 字符 > 10000 阈值
        let marker = reg.register(text.clone());
        let expanded = reg.expand(&format!("数据:{marker}"));
        assert!(expanded.contains("[...中间省略 11000 字符...]"));
        assert!(expanded.starts_with("数据:0123456789"));
        // 尾 500 字符保留
        assert!(expanded.ends_with("0123456789"));
        // 总长 ≈ 前缀 + 500 + 标注 + 500,远小于原文
        assert!(expanded.chars().count() < 1200);
    }

    #[test]
    fn expand_keeps_damaged_or_unknown_marker_as_is() {
        let mut reg = PasteRegistry::new();
        let marker = reg.register("y".repeat(2000));
        // 用户删了 marker 尾括号 → 不匹配,原样保留
        let damaged = marker.trim_end_matches(']').to_string();
        let buf = format!("看 {damaged}");
        assert_eq!(reg.expand(&buf), buf);
        // 未知编号 → 原样保留
        let buf2 = "看 [粘贴 #99 +5 行]".to_string();
        assert_eq!(reg.expand(&buf2), buf2);
    }

    #[test]
    fn expand_marker_removed_by_user_no_residue() {
        let mut reg = PasteRegistry::new();
        let _marker = reg.register("z".repeat(2000));
        // 用户把 marker 整段删了再提交 → 展开为空串,registry 内容不泄漏
        assert_eq!(reg.expand(""), "");
    }

    #[test]
    fn head_tail_chars_char_boundary_safe() {
        let s = "中文测试abcdef";
        assert_eq!(head_chars(s, 2), "中文");
        assert_eq!(tail_chars(s, 3), "def");
        assert_eq!(head_chars(s, 100), s);
        assert_eq!(tail_chars(s, 100), s);
        assert_eq!(head_chars("", 5), "");
    }

    #[test]
    fn truncate_for_inject_below_threshold_passthrough() {
        let s = "短文本";
        assert_eq!(PasteRegistry::truncate_for_inject(s), s);
        let exact = "a".repeat(PASTE_TRUNCATE_CHARS);
        assert_eq!(PasteRegistry::truncate_for_inject(&exact), exact);
    }

    // ===== 第 131 轮:粘贴内容「只显示一遍」 =====

    /// 复刻用户实测形态:8 行 Markdown 提示词粘贴 → 提交时不得再全量回显一遍。
    #[test]
    fn 粘贴预览后提交不重复回显原文() {
        let mut reg = PasteRegistry::new();
        let text = (1..=8).map(|i| format!("第{i}行内容")).collect::<Vec<_>>().join("\n");
        let marker = match handle_paste_text(&text, &mut reg) {
            PasteInsert::Marker(m) => m,
            PasteInsert::Inline(_) => panic!("8 行粘贴应转 marker"),
        };
        reg.mark_previewed(); // 调用点打印预览后登记
        let expanded = reg.expand(&marker);
        let skip = reg.expands_previewed_paste(&marker);

        let out = plan_submit_echo_with_paste(">> ", &marker, &expanded, 80, skip);
        // 输入行 marker 形态照常回显,让用户看见提交的是什么
        assert_eq!(out[0], format!(">> {marker}"));
        // 8 行原文一个都不再出现
        for i in 1..=8 {
            assert!(
                !out.iter().any(|l| l.contains(&format!("第{i}行内容"))),
                "第{i}行不应在提交回显里重复出现:\n{out:?}"
            );
        }
        // 末尾一行说明「已完整发送」,标明行数/字数
        let tail = out.last().unwrap();
        assert!(tail.contains("已按原文完整发送 8 行"), "{tail}");
        assert!(tail.contains("/ 47 字"), "{tail}");
    }

    #[test]
    fn 纯键盘输入提交仍全量回显() {
        // 无粘贴 → skip_full_echo=false → 与 plan_submit_echo 逐行一致
        let text = "第一行\n第二行\n第三行";
        let out = plan_submit_echo_with_paste(">> ", text, text, 80, false);
        assert_eq!(out, plan_submit_echo(">> ", text, 80));
        assert_eq!(out[0], ">> 第一行");
        assert_eq!(out[2], ".. 第三行");
        assert!(!out.iter().any(|l| l.contains("已按原文完整发送")));
    }

    #[test]
    fn 粘贴加手写文字时手写部分照常回显() {
        let mut reg = PasteRegistry::new();
        let text = "第一行\n第二行";
        let marker = match handle_paste_text(text, &mut reg) {
            PasteInsert::Marker(m) => m,
            PasteInsert::Inline(_) => panic!("应转 marker"),
        };
        reg.mark_previewed();
        let buffer = format!("帮我看看 {marker} 谢谢");
        let expanded = reg.expand(&buffer);
        let skip = reg.expands_previewed_paste(&buffer);
        assert!(skip, "buffer 含已预览 marker → 应跳过全量回显");

        let out = plan_submit_echo_with_paste(">> ", &buffer, &expanded, 80, skip);
        let joined = out.join("\n");
        assert!(joined.contains("帮我看看"), "手写前半段应保留:{joined}");
        assert!(joined.contains("谢谢"), "手写后半段应保留:{joined}");
        assert!(!joined.contains("第一行"), "粘贴原文不重复:{joined}");
    }

    #[test]
    fn 未预览的登记仍走全量回显() {
        // 登记后未调用 mark_previewed(为将来的静默登记路径留的兜底)
        let mut reg = PasteRegistry::new();
        let marker = reg.register("x\ny".to_string());
        let buffer = marker.clone();
        let expanded = reg.expand(&buffer);
        assert!(!reg.expands_previewed_paste(&buffer), "未预览不应跳过");
        let out = plan_submit_echo_with_paste(">> ", &buffer, &expanded, 80, false);
        assert_eq!(out, plan_submit_echo(">> ", &expanded, 80));
        assert!(out.iter().any(|l| l.contains("x")));
    }

    #[test]
    fn 粘贴回显仍受视觉行上限约束() {
        let mut reg = PasteRegistry::new();
        let text = (1..=40).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        let marker = match handle_paste_text(&text, &mut reg) {
            PasteInsert::Marker(m) => m,
            PasteInsert::Inline(_) => panic!("应转 marker"),
        };
        reg.mark_previewed();
        let expanded = reg.expand(&marker);
        let out = plan_submit_echo_with_paste(">> ", &marker, &expanded, 100, true);
        // 1 行 marker 回显 + 1 行说明,marker 版 buffer 本身很短,不应触发折叠
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out.iter().all(|l| textfit::width(l) <= 100));
    }
}
