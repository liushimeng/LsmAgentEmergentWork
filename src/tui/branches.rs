//! 对话分支存储(D3,2026-09-10 第二十四轮)。
//!
//! 知识库来源:第十八轮 D3 维度(openclaw rewind/fork/switch 三 action + pi `/fork`
//! `/clone` 轻量派,方案 `tmpPlan/2026-09-10_07-D3对话Rewind与分支方案.md`)。
//!
//! 「分支」= 某一时刻会话状态的完整快照(Session 上下文 + transcript + 累计用量)。
//! `/rewind` `/fork` `/switch` `/clear` 在破坏性操作前自动快照,用户可随时
//! `/switch <name>` 整体找回 —— 内存态,退出 TUI 即失效,容量上限淘汰最旧。

use crate::llm::Usage;
use crate::session::Session;

use super::export::TranscriptEntry;

/// 分支容量上限(超出淘汰最旧;快照含完整上下文,需有界)。
const MAX_BRANCHES: usize = 10;

/// 一个分支快照:某时刻会话状态的完整拷贝。
#[derive(Debug, Clone)]
pub struct BranchSnapshot {
    /// 分支名(计数命名,如 `rewind-1` / `fork-2` / `clear-3` / `switch-4`)。
    pub name: String,
    /// 快照时间(HH:MM:SS 本地)。
    pub created_at: String,
    /// 来源说明(如「/rewind 2 回退前(共 5 轮)」)。
    pub note: String,
    /// 快照时的完整 Session(含原 Session ID 与上下文)。
    pub session: Session,
    /// 快照时的 transcript(导出/轮次列表同源)。
    pub transcript: Vec<TranscriptEntry>,
    /// 快照时的会话累计用量。
    pub usage: Usage,
}

/// 内存分支存储。
pub struct BranchStore {
    snaps: Vec<BranchSnapshot>,
    counter: u64,
    cap: usize,
}

impl BranchStore {
    pub fn new() -> Self {
        Self {
            snaps: Vec::new(),
            counter: 0,
            cap: MAX_BRANCHES,
        }
    }

    /// 保存当前会话状态为分支,返回生成的分支名。
    ///
    /// 命名 `{prefix}-{counter}`,counter 全局递增保证唯一;
    /// 超出容量时淘汰最旧分支(内存有界,方案文档 §2.4)。
    pub fn snapshot(
        &mut self,
        prefix: &str,
        note: &str,
        session: &Session,
        transcript: &[TranscriptEntry],
        usage: Usage,
        created_at: String,
    ) -> String {
        self.counter += 1;
        let name = format!("{prefix}-{}", self.counter);
        self.snaps.push(BranchSnapshot {
            name: name.clone(),
            created_at,
            note: note.to_string(),
            session: session.clone(),
            transcript: transcript.to_vec(),
            usage,
        });
        while self.snaps.len() > self.cap {
            self.snaps.remove(0);
        }
        name
    }

    /// 按名取分支(只读)。
    pub fn get(&self, name: &str) -> Option<&BranchSnapshot> {
        self.snaps.iter().find(|s| s.name == name)
    }

    /// 恢复分支:返回 (Session, transcript, usage) 的完整拷贝。
    pub fn restore(&self, name: &str) -> Option<(Session, Vec<TranscriptEntry>, Usage)> {
        self.get(name)
            .map(|s| (s.session.clone(), s.transcript.clone(), s.usage))
    }

    /// 全部分支(新→旧排列,列表展示用)。
    pub fn list(&self) -> Vec<&BranchSnapshot> {
        self.snaps.iter().rev().collect()
    }

    /// 分支数。
    pub fn len(&self) -> usize {
        self.snaps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.snaps.is_empty()
    }

    /// 分支末轮预览(列表展示;无轮次返回空串)。
    pub fn last_turn_preview(s: &BranchSnapshot) -> String {
        s.transcript
            .last()
            .map(|e| e.raw_input.clone())
            .unwrap_or_default()
    }
}

impl Default for BranchStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ChatMessage;

    fn sample_session(turns: usize) -> Session {
        let mut s = Session::new();
        for i in 0..turns {
            s.context_mut().push(ChatMessage::user(format!("第{i}轮")));
        }
        s
    }

    fn sample_transcript(n: usize) -> Vec<TranscriptEntry> {
        (0..n)
            .map(|i| TranscriptEntry {
                ts: format!("00:00:0{i}"),
                raw_input: format!("输入{i}"),
                prompt: format!("输入{i}"),
                response: format!("回答{i}"),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
                outcome: super::super::export::OutcomeKind::DirectAnswer,
            })
            .collect()
    }

    #[test]
    fn snapshot_names_increment() {
        let mut store = BranchStore::new();
        let a = store.snapshot("rewind", "n1", &sample_session(1), &[], Usage::default(), "t".into());
        let b = store.snapshot("rewind", "n2", &sample_session(2), &[], Usage::default(), "t".into());
        assert_eq!(a, "rewind-1");
        assert_eq!(b, "rewind-2");
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn restore_returns_full_state() {
        let mut store = BranchStore::new();
        let session = sample_session(3);
        let transcript = sample_transcript(3);
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };
        let name = store.snapshot("fork", "n", &session, &transcript, usage, "t".into());

        let (s2, t2, u2) = store.restore(&name).expect("分支存在");
        assert_eq!(s2.context.len(), 3);
        assert_eq!(s2.id, session.id, "恢复保留快照时的 Session ID");
        assert_eq!(t2.len(), 3);
        assert_eq!(t2[2].raw_input, "输入2");
        assert_eq!(u2.input_tokens, 100);
    }

    #[test]
    fn restore_unknown_name_is_none() {
        let store = BranchStore::new();
        assert!(store.restore("nope").is_none());
        assert!(store.get("nope").is_none());
    }

    #[test]
    fn capacity_evicts_oldest() {
        let mut store = BranchStore::new();
        store.cap = 3;
        let mut names = Vec::new();
        for i in 0..5 {
            names.push(store.snapshot(
                "rewind",
                &format!("n{i}"),
                &sample_session(1),
                &[],
                Usage::default(),
                "t".into(),
            ));
        }
        assert_eq!(store.len(), 3, "超出容量淘汰最旧");
        assert!(store.get(&names[0]).is_none(), "rewind-1 已被淘汰");
        assert!(store.get(&names[4]).is_some(), "最新分支保留");
    }

    #[test]
    fn counter_survives_eviction_no_name_reuse() {
        let mut store = BranchStore::new();
        store.cap = 1;
        let a = store.snapshot("rewind", "n", &sample_session(1), &[], Usage::default(), "t".into());
        let b = store.snapshot("rewind", "n", &sample_session(1), &[], Usage::default(), "t".into());
        assert_eq!(a, "rewind-1");
        assert_eq!(b, "rewind-2", "淘汰后 counter 不回退,分支名不复用");
    }

    #[test]
    fn list_is_newest_first() {
        let mut store = BranchStore::new();
        store.snapshot("rewind", "n1", &sample_session(1), &[], Usage::default(), "t".into());
        store.snapshot("fork", "n2", &sample_session(1), &[], Usage::default(), "t".into());
        let names: Vec<&str> = store.list().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["fork-2", "rewind-1"]);
    }

    #[test]
    fn last_turn_preview_from_transcript() {
        let mut store = BranchStore::new();
        let name = store.snapshot(
            "rewind",
            "n",
            &sample_session(2),
            &sample_transcript(2),
            Usage::default(),
            "t".into(),
        );
        let snap = store.get(&name).unwrap();
        assert_eq!(BranchStore::last_turn_preview(snap), "输入1");

        let empty = store.snapshot("clear", "n", &sample_session(0), &[], Usage::default(), "t".into());
        assert_eq!(
            BranchStore::last_turn_preview(store.get(&empty).unwrap()),
            "",
            "无轮次返回空串"
        );
    }
}
