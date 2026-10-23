//! 死循环守卫单测矩阵(设计文档 §10)。
//!
//! 反误伤回归是本模块的重点:误杀一条健康的多步单元,代价远大于漏放一次循环。

use super::detect::{
    detect_cycle, detect_low_novelty, min_periodic_suffix, reps_threshold, LoopKind,
};
use super::fingerprint::{fnv1a, normalize_ws, LlmTurnFingerprint};
use super::*;
use crate::llm::ChatMessage;

const WIN: usize = 12;

/// 构造一条健康轨迹:每轮上下文都有新信息(模拟助手消息 + 工具结果逐轮追加)。
fn healthy_guard(rounds: usize) -> DeadLoopGuard {
    let mut g = DeadLoopGuard::with_settings(true, WIN);
    let mut ctx: Vec<ChatMessage> = vec![ChatMessage::user("任务:排查构建失败")];
    for i in 0..rounds {
        g.begin_turn("Test", &ctx, None, &["Bash".into()]);
        ctx.push(ChatMessage::user(format!("第 {i} 轮观察:新的报错内容 {i}")));
        ctx.push(ChatMessage::user(format!("第 {i} 轮工具结果:编译错误 E{i}")));
        let _ = g.end_turn(i, &format!("我要修 E{i}"), "", &[]);
    }
    g
}

/// 构造一条硬自锁轨迹:模型逐轮产出**完全相同**的输出,工具结果逐字节相同。
fn locked_guard(rounds: usize) -> DeadLoopGuard {
    let mut g = DeadLoopGuard::with_settings(true, WIN);
    let mut ctx: Vec<ChatMessage> = vec![ChatMessage::user("任务:修 bug")];
    let calls = vec![(
        "Bash".to_string(),
        serde_json::json!({"command": "cargo build"}),
    )];
    for i in 0..rounds {
        g.begin_turn("Test", &ctx, None, &["Bash".into()]);
        ctx.push(ChatMessage::assistant(vec![crate::llm::ContentBlock::ToolUse {
            id: "t1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "cargo build"}),
        }]));
        ctx.push(ChatMessage::tool_result("t1", "error[E0308]: mismatched types", false));
        let _ = g.end_turn(i, "先重新构建确认", "", &calls);
    }
    g
}

/// 构造一条 p-周期轨迹:模型按固定序列产出 A/B/C/D 轮转。
///
/// 关键细节:**只在第一圈往上下文追加新内容**,第二圈起采集到的是同一批区块
/// 的同一份内容 —— 于是「模型已见信息集合」停止增长、输入指纹冻结,而输出
/// 按周期轮转。这正是真实「换序打转」的形态:第二次访问 Read(a) 拿到的
/// 和第一次一模一样。若每圈都追加带轮次号的新内容,那是在**造进展**,
/// 指纹理应每轮变化(见 `健康多步轨迹零误伤`)。
fn periodic_guard(period: usize, rounds: usize) -> DeadLoopGuard {
    let mut g = DeadLoopGuard::with_settings(true, WIN);
    let mut ctx: Vec<ChatMessage> = vec![ChatMessage::user("任务:遍历站点")];
    for i in 0..rounds {
        let round = g.begin_turn("Test", &ctx, None, &["Read".into()]);
        let letter = (b'A' + (i % period) as u8) as char;
        if i < period {
            ctx.push(ChatMessage::user(format!("采集结果:区块 {letter}")));
            ctx.push(ChatMessage::user(format!("区块 {letter} 正文内容")));
        }
        let _ = g.end_turn(round, &format!("检查区块 {letter}"), "", &[]);
    }
    g
}

// ============ 误伤防护(最重要)============

#[test]
fn 健康多步轨迹零误伤() {
    let mut g = healthy_guard(WIN + 4);
    assert_eq!(g.max_repeats(), 0, "健康轨迹不应命中任何重复链");
    assert!(g.diagnosis().is_none());
}

#[test]
fn 编译修复场景零误伤() {
    // 反误伤第一红线:`Bash cargo check` 连跑 6 次修 6 个编译错,
    // 工具调用序列高度重复,但每次结果都不同 —— 是健康工作,绝不能判死循环。
    let mut g = DeadLoopGuard::with_settings(true, WIN);
    let mut ctx: Vec<ChatMessage> = vec![ChatMessage::user("编译不过,帮我修")];
    let calls = vec![(
        "Bash".to_string(),
        serde_json::json!({"command": "cargo check"}),
    )];
    for i in 0..WIN {
        g.begin_turn("Test", &ctx, None, &["Bash".into()]);
        ctx.push(ChatMessage::tool_result("t1", format!("error[E0308]: mismatched types, 第 {i} 个"), false));
        let verdict = g.end_turn(i, &format!("修掉第 {i} 个类型错误"), "", &calls);
        assert_eq!(
            format!("{verdict:?}"),
            format!("{:?}", DeadLoopVerdict::Progress),
            "第 {i} 轮被误判"
        );
        // 输出文案在变、工具结果在变 → 信息集合持续增长 → 输入指纹每轮变。
        ctx.push(ChatMessage::assistant(vec![crate::llm::ContentBlock::text(format!(
            "修掉第 {i} 个类型错误"
        ))]));
    }
    assert_eq!(g.max_repeats(), 0);
}

#[test]
fn 正常轮询等待零误伤() {
    // `wait` 轮询:工具调用相同但每次等待结果不同(时间在走)。
    let mut g = DeadLoopGuard::with_settings(true, WIN);
    let mut ctx: Vec<ChatMessage> = vec![ChatMessage::user("等待页面加载")];
    let calls = vec![(
        "MCP_Web_LsmAgentBrowser_Use".to_string(),
        serde_json::json!({"action": "control", "params": {"control_action": "wait"}}),
    )];
    for i in 0..WIN {
        g.begin_turn("Test", &ctx, None, &["MCP_Web_LsmAgentBrowser_Use".into()]);
        ctx.push(ChatMessage::tool_result("t1", format!("等待 {i}0ms,剩余元素 {i}"), false));
        let verdict = g.end_turn(i, &format!("再看一眼页面 {i}"), "", &calls);
        assert!(matches!(verdict, DeadLoopVerdict::Progress), "第 {i} 轮: {verdict:?}");
        ctx.push(ChatMessage::assistant(vec![crate::llm::ContentBlock::text(format!(
            "再看一眼页面 {i}"
        ))]));
    }
}

#[test]
fn 周期门槛分档正确() {
    assert_eq!(reps_threshold(1), 2, "周期 1 两次即铁证");
    assert_eq!(reps_threshold(2), 3, "短周期需三圈");
    assert_eq!(reps_threshold(4), 3);
    assert_eq!(reps_threshold(5), 2, "长周期两圈即可");
    assert_eq!(reps_threshold(20), 2);
}

// ========== 最小周期后缀扫描 ==========

#[test]
fn 尾部周期扫描识别瞬态前缀后的循环() {
    // 整串最小周期(标准 KMP `n - pi[n-1]`)在这里恒为「无循环」:
    // 首轮 (H0,o) 与后续 (H1,o) 不同,整串不构成周期。
    let keys: Vec<(u64, u64)> = vec![(0, 9), (1, 9), (1, 9), (1, 9), (1, 9)];
    assert_eq!(min_periodic_suffix(&keys), Some((1, 4)));
}

#[test]
fn 尾部周期扫描识别二周期轮转() {
    let keys: Vec<(u64, u64)> = (0..6).map(|i| (i % 2, i % 2)).collect();
    assert_eq!(min_periodic_suffix(&keys), Some((2, 3)));
}

#[test]
fn 尾部周期扫描全互异判无循环() {
    let keys: Vec<(u64, u64)> = (0..8).map(|i| (i, i)).collect();
    assert_eq!(min_periodic_suffix(&keys), None);
    assert_eq!(min_periodic_suffix(&[(0, 0)]), None, "单轮无周期概念");
}

#[test]
fn 尾部周期扫描取最小周期() {
    // 2 的倍数也是 4 的倍数,必须报最小周期 2 而不是 4。
    let keys: Vec<(u64, u64)> = (0..6).map(|i| (i % 2, i % 2)).collect();
    let (p, _) = min_periodic_suffix(&keys).expect("应命中");
    assert_eq!(p, 2);
}

// ============ 小循环 ============

#[test]
fn 硬自锁第二次命中即软提醒() {
    let mut g = DeadLoopGuard::with_settings(true, WIN);
    let mut ctx: Vec<ChatMessage> = vec![ChatMessage::user("修 bug")];
    let calls = vec![(
        "Bash".to_string(),
        serde_json::json!({"command": "cargo build"}),
    )];
    let mut verdicts = Vec::new();
    for i in 0..4 {
        g.begin_turn("Test", &ctx, None, &["Bash".into()]);
        ctx.push(ChatMessage::assistant(vec![crate::llm::ContentBlock::ToolUse {
            id: "t1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "cargo build"}),
        }]));
        ctx.push(ChatMessage::tool_result("t1", "error[E0308]", false));
        verdicts.push(g.end_turn(i, "先重新构建确认", "", &calls));
    }
    assert!(
        matches!(verdicts[0], DeadLoopVerdict::Progress),
        "首轮必然有进展:{:?}",
        verdicts[0]
    );
    assert!(
        matches!(verdicts[1], DeadLoopVerdict::Nudge(_)),
        "第二次命中软提醒,实得 {:?}",
        verdicts[1]
    );
    assert!(
        matches!(verdicts[2], DeadLoopVerdict::Grace(_)),
        "第三次进宽限轮,实得 {:?}",
        verdicts[2]
    );
    match &verdicts[3] {
        DeadLoopVerdict::Abort(d) => {
            assert_eq!(d.kind, LoopKind::ShortCycle);
            assert_eq!(d.period, Some(1));
        }
        other => panic!("第四次应止损,实得 {other:?}"),
    }
}

#[test]
fn 软提醒文案只消费一次() {
    let mut g = locked_guard(2);
    assert!(g.take_nudge().is_some());
    assert!(g.take_nudge().is_none(), "软提醒只注入一次");
}

#[test]
fn 软提醒文案含形态与诊断摘要() {
    let g = locked_guard(4);
    let d = g.diagnosis().expect("应有诊断");
    let t = nudge_text(d);
    assert!(t.contains("循环警告"), "{t}");
    assert!(t.contains("短周期"), "{t}");
}

#[test]
fn 宽限轮文案指向新信息源() {
    let g = locked_guard(4);
    let t = grace_text(g.diagnosis().expect("应有诊断"));
    assert!(t.contains("宽限轮"), "{t}");
    assert!(t.contains("没有获得任何新信息"), "{t}");
    assert!(t.contains("Bash"), "应点名循环内工具:{t}");
}

#[test]
fn 换序打转命中二周期() {
    // A→B→A→B→A→B:每轮动作都不同,LoopGuard 的相邻比较永远抓不到。
    let g = periodic_guard(2, 8);
    let d = g.diagnosis().expect("应命中");
    assert_eq!(d.kind, LoopKind::ShortCycle);
    assert_eq!(d.period, Some(2));
    assert!(d.reps >= 3, "reps={}", d.reps);
}

#[test]
fn 三周期边界归入小循环() {
    let g = periodic_guard(3, 12);
    let d = g.diagnosis().expect("应命中");
    assert_eq!(d.kind, LoopKind::ShortCycle);
    assert_eq!(d.period, Some(3));
}

#[test]
fn 四周期边界仍归小循环() {
    let g = periodic_guard(4, 16);
    let d = g.diagnosis().expect("应命中");
    assert_eq!(d.kind, LoopKind::ShortCycle);
    assert_eq!(d.period, Some(4));
}

#[test]
fn 五周期边界归入大循环() {
    // 长周期两圈即命中。
    let g = periodic_guard(5, 15);
    let d = g.diagnosis().expect("应命中");
    assert_eq!(d.kind, LoopKind::LongCycle);
    assert_eq!(d.period, Some(5));
}

// ============ 大循环 · 低新颖度 ============

#[test]
fn 低新颖度窗口命中() {
    // 措辞每轮都变(输出指纹不重复 → 周期检测抓不到),
    // 但上下文信息集合冻结(模型没获得新信息)→ 低新颖度命中。
    let mut g = DeadLoopGuard::with_settings(true, 8);
    let ctx: Vec<ChatMessage> = vec![ChatMessage::user("任务")];
    for i in 0..8 {
        g.begin_turn("Test", &ctx, None, &["Read".into()]);
        // 不往 ctx 追加任何新消息 → 输入指纹冻结。
        let _ = g.end_turn(i, &format!("第 {i} 次换个说法重新规划一遍"), "", &[]);
    }
    let d = g.diagnosis().expect("应命中低新颖度");
    assert_eq!(d.kind, LoopKind::LowNovelty);
    assert_eq!(d.period, None);
}

#[test]
fn 低新颖度差一轮不命中() {
    let mut g = DeadLoopGuard::with_settings(true, 8);
    let ctx: Vec<ChatMessage> = vec![ChatMessage::user("任务")];
    for i in 0..7 {
        g.begin_turn("Test", &ctx, None, &["Read".into()]);
        let _ = g.end_turn(i, &format!("第 {i} 次换措辞"), "", &[]);
    }
    assert!(g.diagnosis().is_none(), "窗口未满不得误判");
}

#[test]
fn 健康轨迹不触发低新颖度检测() {
    let g = healthy_guard(20);
    let mut idx = FingerprintIndex::new();
    let mut ctx: Vec<ChatMessage> = vec![ChatMessage::user("任务")];
    for i in 0..20 {
        idx.begin_turn("Test", &ctx, None, &[]);
        ctx.push(ChatMessage::user(format!("新信息 {i}")));
        idx.end_turn(i, &format!("输出 {i}"), "", &[]);
    }
    assert!(
        detect_low_novelty(&idx, WIN).is_none(),
        "健康轨迹每轮都有新信息,输入指纹不可能冻结"
    );
    drop(g);
}

#[test]
fn 冻结轮数不足六轮不命中() {
    // distinct=1 但只有 3 轮冻结(窗口设为 4):低于 frozen≥6 门槛。
    let mut idx = FingerprintIndex::new();
    let ctx: Vec<ChatMessage> = vec![ChatMessage::user("任务")];
    for i in 0..4 {
        idx.begin_turn("Test", &ctx, None, &[]);
        idx.end_turn(i, &format!("换措辞 {i}"), "", &[]);
    }
    assert!(detect_low_novelty(&idx, 4).is_none());
}

#[test]
fn 窗口未满不检测低新颖度() {
    let idx = FingerprintIndex::new();
    assert!(detect_low_novelty(&idx, WIN).is_none());
}

// ============ 大循环裁决路径 ============

#[test]
fn 大循环跳过软提醒直达宽限轮() {
    let mut g = DeadLoopGuard::with_settings(true, WIN);
    let ctx: Vec<ChatMessage> = vec![ChatMessage::user("任务")];
    let mut first_hit: Option<DeadLoopVerdict> = None;
    for i in 0..WIN {
        g.begin_turn("Test", &ctx, None, &["Read".into()]);
        let v = g.end_turn(i, &format!("第 {i} 次换个说法重写计划"), "", &[]);
        if !matches!(v, DeadLoopVerdict::Progress) && first_hit.is_none() {
            first_hit = Some(v);
        }
    }
    match first_hit.expect("长窗口跑满应命中") {
        DeadLoopVerdict::Grace(t) => assert!(t.contains("宽限轮"), "{t}"),
        other => panic!("大循环首命中应直达宽限轮,实得 {other:?}"),
    }
    assert!(g.take_nudge().is_none(), "大循环不发软提醒");
}

#[test]
fn 宽限轮用尽后大循环止损() {
    let mut g = DeadLoopGuard::with_settings(true, 8);
    let ctx: Vec<ChatMessage> = vec![ChatMessage::user("任务")];
    let mut aborted = None;
    for i in 0..16 {
        g.begin_turn("Test", &ctx, None, &["Read".into()]);
        let v = g.end_turn(i, &format!("第 {i} 次换措辞"), "", &[]);
        if let DeadLoopVerdict::Abort(d) = v {
            aborted = Some(d);
            break;
        }
    }
    let d = aborted.expect("宽限轮后应止损");
    assert_eq!(d.kind, LoopKind::LowNovelty);
    assert!(g.diagnosis().is_some());
}

// ============ 指纹构造 ============

#[test]
fn 控制消息不改变输入指纹() {
    // 自反馈污染防护:守卫注入 <<<LAEW:...>>> 提醒后,状态哈希不得被自己刷新。
    let mut idx = FingerprintIndex::new();
    let ctx = vec![ChatMessage::user("原始任务")];
    idx.begin_turn("T", &ctx, None, &[]);
    let base = idx.steps[0].in_hash;
    let with_hint = vec![
        ChatMessage::user("原始任务"),
        ChatMessage::user("<<<LAEW:LOOP_NUDGE>>>\n【循环警告】你又在原地打转"),
        ChatMessage::user("<<<LAEW:RUNTIME_HINTS>>>\n- 某提示"),
    ];
    idx.begin_turn("T", &with_hint, None, &[]);
    assert_eq!(
        idx.steps[1].in_hash, base,
        "控制消息不得刷新「模型已见信息集合」"
    );
}

#[test]
fn 空白归一化抹平格式抖动() {
    let mut idx = FingerprintIndex::new();
    let a = vec![ChatMessage::user("阶段一: 读取   文件\n并解析")];
    let b = vec![ChatMessage::user("阶段一: 读取 文件 并解析")];
    idx.begin_turn("T", &a, None, &[]);
    idx.begin_turn("T", &b, None, &[]);
    assert_eq!(idx.steps[0].in_hash, idx.steps[1].in_hash);
    assert_eq!(idx.steps.len(), 2);
}

#[test]
fn 新增信息必然改变输入指纹() {
    let mut idx = FingerprintIndex::new();
    let mut ctx = vec![ChatMessage::user("任务")];
    let mut hashes = Vec::new();
    for i in 0..5 {
        let round = idx.begin_turn("T", &ctx, None, &[]);
        let pos = idx.steps.iter().position(|s| s.round == round).unwrap();
        hashes.push(idx.steps[pos].in_hash);
        ctx.push(ChatMessage::user(format!("新增信息 {i}")));
    }
    for w in hashes.windows(2) {
        assert_ne!(w[0], w[1], "有新增信息时输入指纹必须变化");
    }
}

#[test]
fn fnv1a_跨进程确定() {
    // 指纹会写进 ExecutionTrace 供事后对账,必须是确定性算法。
    assert_eq!(fnv1a(b""), 0xcbf29ce484222325);
    assert_eq!(fnv1a(b"laew"), fnv1a(b"laew"));
    assert_ne!(fnv1a(b"laew"), fnv1a(b"laex"));
    assert_eq!(normalize_ws(" a  b\nc "), "a b c");
}

#[test]
fn 输出动作指纹忽略措辞只认动作() {
    // 两条仅措辞不同的调用,action_hash 相同、out_hash 不同。
    let mut idx = FingerprintIndex::new();
    let ctx = vec![ChatMessage::user("任务")];
    idx.begin_turn("T", &ctx, None, &[]);
    let calls = vec![(
        "Read".to_string(),
        serde_json::json!({"file_path": "a.rs"}),
    )];
    idx.end_turn(0, "我来读一下", "", &calls);
    idx.begin_turn("T", &ctx, None, &[]);
    idx.end_turn(1, "让我读一下这个文件", "", &calls);
    assert_eq!(idx.steps[0].action_hash, idx.steps[1].action_hash);
    assert_ne!(idx.steps[0].out_hash, idx.steps[1].out_hash);
}

#[test]
fn 输出动作指纹忽略工具顺序() {
    let mut idx = FingerprintIndex::new();
    let ctx = vec![ChatMessage::user("任务")];
    idx.begin_turn("T", &ctx, None, &[]);
    idx.end_turn(
        0,
        "",
        "",
        &[
            ("Read".to_string(), serde_json::json!({"file_path": "a"})),
            ("Bash".to_string(), serde_json::json!({"command": "ls"})),
        ],
    );
    idx.begin_turn("T", &ctx, None, &[]);
    idx.end_turn(
        1,
        "",
        "",
        &[
            ("Bash".to_string(), serde_json::json!({"command": "ls"})),
            ("Read".to_string(), serde_json::json!({"file_path": "a"})),
        ],
    );
    assert_eq!(
        idx.steps[0].action_hash, idx.steps[1].action_hash,
        "工具顺序不同不算换动作"
    );
}

#[test]
fn 哈希索引表记录重复次数() {
    let g = locked_guard(4);
    let idx = &g.idx;
    assert!(
        idx.out_seen.values().any(|v| v.len() >= 2),
        "重复输出必须在索引表里可数"
    );
    assert!(idx.distinct_outputs() <= 2, "全程几乎只产出一份响应");
}

#[test]
fn 轮次序列有界() {
    // 超过 MAX_STEPS 后 FIFO 裁剪,不无限增长。
    let mut idx = FingerprintIndex::new();
    let ctx = vec![ChatMessage::user("任务")];
    for i in 0..(fingerprint::MAX_STEPS + 20) {
        let round = idx.begin_turn("T", &ctx, None, &[]);
        assert_eq!(round, i, "绝对轮次号必须单调且不被裁剪影响");
        idx.end_turn(round, &format!("输出 {i}"), "", &[]);
    }
    assert_eq!(idx.steps.len(), fingerprint::MAX_STEPS);
}

#[test]
fn 信息集合有界() {
    let mut idx = FingerprintIndex::new();
    let mut ctx: Vec<ChatMessage> = vec![ChatMessage::user("任务")];
    for i in 0..(fingerprint::MAX_DIGESTS + 50) {
        idx.begin_turn("T", &ctx, None, &[]);
        ctx.push(ChatMessage::user(format!("新消息 {i}")));
    }
    // 集合已裁剪,但结构仍可用(不 panic、指纹仍会变)。
    assert!(!idx.steps.is_empty());
}

// ============ 诊断结构 ============

#[test]
fn 诊断摘要可读() {
    let g = locked_guard(4);
    let d = g.diagnosis().expect("应有诊断");
    let h = d.headline();
    assert!(h.contains("short_cycle"), "{h}");
    assert!(h.contains("p=1"), "{h}");
    let r = d.render();
    assert!(r.contains("最小周期"), "{r}");
    assert!(r.contains("Bash"), "{r}");
}

#[test]
fn 兜底文案给出判定依据() {
    let g = locked_guard(4);
    let t = abort_fallback_text(g.diagnosis().expect("应有诊断"));
    assert!(t.contains("LLM 交互死循环止损"), "{t}");
    assert!(t.contains("模型已见信息集合"), "{t}");
    assert!(t.contains("Quality-Check"), "{t}");
    assert!(t.contains("Bash"), "{t}");
}

#[test]
fn 诊断结构可序列化() {
    // 要写进 ExecutionTrace 并跨进程反序列化,必须 serde 友好。
    let g = locked_guard(4);
    let d = g.diagnosis().expect("应有诊断");
    let s = serde_json::to_string(d).expect("序列化");
    let back: DeadLoopDiagnosis = serde_json::from_str(&s).expect("反序列化");
    assert_eq!(back.kind, d.kind);
    assert_eq!(back.period, d.period);
    assert_eq!(back.reps, d.reps);
}

// ============ 开关 ============

#[test]
fn 开关解析() {
    for off in ["off", "OFF", "0", "false", " no "] {
        assert!(!enabled_from(off.to_string()), "{off} 应关闭");
    }
    for on in ["", "on", "1", "true", "yes"] {
        assert!(enabled_from(on.to_string()), "{on} 应开启");
    }
}

#[test]
fn 长窗口解析() {
    assert_eq!(long_window_from(String::new()), 12);
    assert_eq!(long_window_from("20".into()), 20);
    for bad in ["abc", "0", "3", "-1"] {
        assert_eq!(
            long_window_from(bad.to_string()),
            12,
            "非法值 {bad} 应回落默认"
        );
    }
}

#[test]
fn 关闭后不裁决但统计不丢() {
    let mut g = DeadLoopGuard::with_settings(false, WIN);
    let mut ctx: Vec<ChatMessage> = vec![ChatMessage::user("修 bug")];
    let calls = vec![(
        "Bash".to_string(),
        serde_json::json!({"command": "cargo build"}),
    )];
    for i in 0..5 {
        g.begin_turn("Test", &ctx, None, &["Bash".into()]);
        ctx.push(ChatMessage::assistant(vec![crate::llm::ContentBlock::ToolUse {
            id: "t1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "cargo build"}),
        }]));
        ctx.push(ChatMessage::tool_result("t1", "error[E0308]", false));
        assert!(
            matches!(g.end_turn(i, "先重新构建确认", "", &calls), DeadLoopVerdict::Progress),
            "关闭后不得裁决"
        );
    }
    assert!(g.max_repeats() >= 2, "关闭后统计仍应更新");
    assert!(g.diagnosis().is_some(), "关闭后诊断仍应留痕");
    assert!(g.take_nudge().is_none());
}

#[test]
fn 指纹键是二元组() {
    let s = LlmTurnFingerprint {
        round: 0,
        in_hash: 1,
        out_hash: 2,
        action_hash: 3,
        in_bytes: 0,
        out_bytes: 0,
        in_preview: String::new(),
        out_preview: String::new(),
        tools: Vec::new(),
    };
    assert_eq!(s.key(), (1, 2));
}

#[test]
fn 周期检测要求至少两轮() {
    let idx = FingerprintIndex::new();
    assert!(detect_cycle(&idx).is_none());
}
