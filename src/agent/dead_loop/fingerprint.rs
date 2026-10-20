//! 死循环指纹:确定性哈希 + 输入投影 + 哈希索引表。
//!
//! 见 `docs/LLM交互死循环指纹检测与质检裁决/01-设计与解决方案.md` §3~§4。

use std::collections::{BTreeSet, HashMap};

use serde_json::Value;

use crate::agent::runtime_hints::stable_json_string;
use crate::llm::{ChatMessage, ContentBlock, Role};

/// FNV-1a 64 位偏移基。
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64 位质数。
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 轮次序列上限:超出丢弃最早(迭代预算上限 32,留一倍余量)。
pub(crate) const MAX_STEPS: usize = 64;
/// 「模型已见信息集合」上限:超出丢弃最小(集合有序,便于稳定折叠)。
pub(crate) const MAX_DIGESTS: usize = 512;
/// 输入投影:每条消息参与哈希的取样字符数。
pub(crate) const IN_MSG_CHARS: usize = 320;
/// 诊断预览字符数。
const PREVIEW_CHARS: usize = 160;

/// FNV-1a 64。
///
/// **不用 `std::collections::hash_map::DefaultHasher`**:官方明确不保证跨
/// Rust 版本稳定,而指纹要写进 `ExecutionTrace` 供事后对账,必须是确定性算法。
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// 字符串指纹(十六进制,便于写进日志/trace 对账)。
pub(crate) fn digest_hex(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a(bytes))
}

/// 空白归一化:连续空白折叠为单空格 + 首尾去空白。
///
/// 防止同一内容因换行/缩进抖动而算出不同指纹(格式化差异不该被判成
/// 「模型换了思路」)。**不剥数字**:计数器变化是真实进展信号。
pub(crate) fn normalize_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(IN_MSG_CHARS * 2));
    let mut pending_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

/// 平台自注入消息前缀(`<<<LAEW:...>>>` 标记块)。
///
/// 这类消息由进程自己注入(项目上下文 / 会话历史 / 附件 / 本守卫的提醒),
/// **不计入「模型已见信息集合」** —— 否则守卫每推一条提醒就刷新一次状态
/// 哈希,把死循环伪装成有进展(自反馈污染)。
fn is_control_message(msg: &ChatMessage) -> bool {
    msg.content.iter().any(|b| match b {
        ContentBlock::Text { text } => text.trim_start().starts_with("<<<LAEW:"),
        _ => false,
    })
}

/// 单条消息的规范化取样文本(参与信息集合与输入指纹)。
fn message_sample(msg: &ChatMessage) -> String {
    let role = match msg.role {
        Role::User => "U",
        Role::Assistant => "A",
        Role::Tool => "T",
        Role::System => "S",
    };
    let mut body = String::new();
    for b in &msg.content {
        match b {
            ContentBlock::Text { text } => {
                if !body.is_empty() {
                    body.push('|');
                }
                body.push_str(&normalize_ws(text));
            }
            ContentBlock::ToolUse { name, input, .. } => {
                if !body.is_empty() {
                    body.push('|');
                }
                body.push_str(name);
                body.push(':');
                body.push_str(&stable_json_string(input));
            }
            ContentBlock::ToolResult {
                content,
                is_error,
                ..
            } => {
                if !body.is_empty() {
                    body.push('|');
                }
                // 总长入键:前 320 字符相同但总长不同(列表在增长)= 有进展。
                body.push_str(if *is_error { "E:" } else { "O:" });
                body.push_str(&content.len().to_string());
                body.push(':');
                body.push_str(&normalize_ws(content));
            }
        }
    }
    let clipped: String = body.chars().take(IN_MSG_CHARS).collect();
    format!("{role}|{clipped}")
}

/// 一次 LLM 交互的指纹(四元组)。
#[derive(Debug, Clone)]
pub(crate) struct LlmTurnFingerprint {
    /// **绝对**轮次号(单调递增,跨 `steps` 裁剪仍唯一)。
    ///
    /// 不能用 `steps` 的下标:`steps` 会 FIFO 裁剪到 [`MAX_STEPS`],下标会
    /// 随裁剪整体前移,用它当轮次号会让 `out_seen` 的「两次出现间隔」判错。
    pub round: usize,
    /// 输入状态指纹(「模型已见信息集合」折叠值)。
    pub in_hash: u64,
    /// 输出全文指纹(text + thinking + tool_calls)。
    pub out_hash: u64,
    /// 输出动作指纹(仅 tool_calls 名 + 参数),忽略措辞。
    pub action_hash: u64,
    pub in_bytes: usize,
    pub out_bytes: usize,
    pub in_preview: String,
    pub out_preview: String,
    pub tools: Vec<String>,
}

impl LlmTurnFingerprint {
    /// 序列匹配键:输入状态 + 输出全文。
    ///
    /// 循环判定用**两元组**而非单看输出:输出相同但输入状态在增长(模型明明
    /// 拿到了新信息却给出同样的回答)是硬自锁;单看输入则会被正常轮询误伤。
    pub(crate) fn key(&self) -> (u64, u64) {
        (self.in_hash, self.out_hash)
    }
}

/// 折叠「有序去重信息集合」为 64 位指纹。
///
/// 集合**排序后**折叠,因此对消息顺序平移与重复插入免疫 —— 这正是朴素
/// 「尾部 N 条消息」窗口方案失效的病根(死循环里窗口每轮整体错位)。
fn fold_digests(digests: &BTreeSet<String>, salt: &str) -> u64 {
    let mut h = FNV_OFFSET;
    for b in salt.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    for d in digests {
        // 每个摘要定长 16 hex,直接逐字节折叠(无需分隔符)。
        for b in d.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
    }
    h
}

/// 轮次指纹的宿主结构:哈希索引表 + 轮次序列。
#[derive(Debug, Clone)]
pub(crate) struct FingerprintIndex {
    /// 轮次序列(FIFO 裁剪到 [`MAX_STEPS`] 条)。
    pub steps: Vec<LlmTurnFingerprint>,
    /// 下一个绝对轮次号。
    next_round: usize,
    /// `in_hash` → 出现轮次。
    pub in_seen: HashMap<u64, Vec<usize>>,
    /// `out_hash` → 出现轮次。
    pub out_seen: HashMap<u64, Vec<usize>>,
    /// 「模型已见信息集合」(有序去重)。
    digests: BTreeSet<String>,
}

impl Default for FingerprintIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl FingerprintIndex {
    pub(crate) fn new() -> Self {
        Self {
            steps: Vec::new(),
            next_round: 0,
            in_seen: HashMap::new(),
            out_seen: HashMap::new(),
            digests: BTreeSet::new(),
        }
    }

    /// 累计本轮上下文摘要并算出输入指纹。
    ///
    /// `agent_tag` 与 `forced_tool` 参与折叠:同一会话内 `forced_tool` 只在
    /// 末轮变化,把它计入可让「强制工具切换」这一真实状态变化被看见。
    pub(crate) fn begin_turn(
        &mut self,
        agent_tag: &str,
        context: &[ChatMessage],
        forced_tool: Option<&str>,
        tool_names: &[String],
    ) -> usize {
        let round = self.next_round;
        self.next_round += 1;
        let mut in_bytes = 0usize;
        let mut last_sample = String::new();
        for msg in context {
            if is_control_message(msg) {
                // 控制消息豁免(见 is_control_message 文档)。
                continue;
            }
            in_bytes += msg.content_text().len();
            let sample = message_sample(msg);
            last_sample = sample.clone();
            self.digests.insert(digest_hex(sample.as_bytes()));
        }
        // 集合有界:超出上限丢弃最小摘要(有序集合,直接取首)。
        while self.digests.len() > MAX_DIGESTS {
            let Some(first) = self.digests.iter().next().cloned() else {
                break;
            };
            self.digests.remove(&first);
        }
        let salt = format!(
            "{agent_tag}#{}#{}",
            forced_tool.unwrap_or("-"),
            tool_names.join(",")
        );
        let in_hash = fold_digests(&self.digests, &salt);
        let preview: String = last_sample.chars().take(PREVIEW_CHARS).collect();
        // 输入半边先占位(输出要等 LLM 响应回来才填)。
        self.steps.push(LlmTurnFingerprint {
            round,
            in_hash,
            out_hash: 0,
            action_hash: 0,
            in_bytes,
            out_bytes: 0,
            in_preview: preview,
            out_preview: String::new(),
            tools: Vec::new(),
        });
        self.trim();
        round
    }

    /// 回填本轮输出指纹并入索引。
    ///
    /// 按**绝对轮次号**定位,不用 `steps` 下标(`steps` 可能已被 FIFO 裁剪)。
    pub(crate) fn end_turn(
        &mut self,
        round: usize,
        text: &str,
        thinking: &str,
        calls: &[(String, Value)],
    ) {
        let out_hash = fnv1a(
            format!(
                "{}\u{1}{}\u{1}{}",
                normalize_ws(text),
                normalize_ws(thinking),
                calls
                    .iter()
                    .map(|(n, a)| format!("{n}:{}", stable_json_string(a)))
                    .collect::<Vec<_>>()
                    .join("\u{2}")
            )
            .as_bytes(),
        );
        // 动作指纹:工具名 + 参数,**排序**折叠 —— 模型换工具顺序不算换动作。
        let mut actions: Vec<String> = calls
            .iter()
            .map(|(n, a)| format!("{n}#{}", stable_json_string(a)))
            .collect();
        actions.sort();
        let action_hash = fnv1a(actions.join("\u{2}").as_bytes());
        let out_preview: String = normalize_ws(text).chars().take(PREVIEW_CHARS).collect();
        let out_bytes = text.len() + thinking.len();

        let Some(pos) = self.steps.iter().position(|s| s.round == round) else {
            return;
        };
        let in_hash = self.steps[pos].in_hash;
        {
            let step = &mut self.steps[pos];
            step.out_hash = out_hash;
            step.action_hash = action_hash;
            step.out_bytes = out_bytes;
            step.out_preview = out_preview;
            step.tools = calls.iter().map(|(n, _)| n.clone()).collect();
        }
        self.in_seen.entry(in_hash).or_default().push(round);
        self.out_seen.entry(out_hash).or_default().push(round);
        self.trim();
    }

    /// FIFO 裁剪到 [`MAX_STEPS`] 条。
    fn trim(&mut self) {
        while self.steps.len() > MAX_STEPS {
            self.steps.remove(0);
        }
    }

    /// 最近一次「同一 `out_hash` 上一次出现」的绝对轮次号(无则 `None`)。
    pub(crate) fn prev_out_round(&self, round: usize) -> Option<(usize, usize)> {
        let hash = self.steps.iter().find(|s| s.round == round)?.out_hash;
        let seen = self.out_seen.get(&hash)?;
        let idx = seen.iter().position(|r| *r == round)?;
        if idx == 0 {
            return None;
        }
        Some((seen[idx - 1], round))
    }

    /// 全会话不同 `out_hash` 个数(诊断用)。
    pub(crate) fn distinct_outputs(&self) -> usize {
        self.out_seen.len()
    }

    /// 全会话不同 `in_hash` 个数(诊断用)。
    pub(crate) fn distinct_inputs(&self) -> usize {
        self.in_seen.len()
    }

    /// 最近 `window` 轮里不同 `in_hash` 的个数。
    pub(crate) fn distinct_inputs_in_window(&self, window: usize) -> (usize, usize, usize) {
        let start = self.steps.len().saturating_sub(window);
        let win = &self.steps[start..];
        let mut distinct = std::collections::HashSet::new();
        let mut frozen = 0usize;
        let mut prev: Option<u64> = None;
        for s in win {
            distinct.insert(s.in_hash);
            if prev == Some(s.in_hash) {
                frozen += 1;
            }
            prev = Some(s.in_hash);
        }
        (distinct.len(), frozen, win.len())
    }

    /// 取出参与循环判定的轮次(升序)。
    pub(crate) fn window(&self, from: usize) -> &[LlmTurnFingerprint] {
        let start = from.min(self.steps.len());
        &self.steps[start..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_已知向量() {
        // 经典 FNV-1a 64 测试向量
        assert_eq!(fnv1a(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn 空白归一化抹平格式抖动() {
        assert_eq!(normalize_ws("a\n  b\t c "), normalize_ws("a b c"));
        assert_eq!(normalize_ws(""), "");
    }

    #[test]
    fn 控制消息豁免() {
        let m = ChatMessage::user("<<<LAEW:PROJECT_CONTEXT>>>\nfoo");
        assert!(is_control_message(&m));
        let m = ChatMessage::user("正常消息 <<LAEW:x>> 中间出现不豁免");
        assert!(!is_control_message(&m), "非前缀不算控制消息");
    }
}
