//! macos_legacy 子模块单元测试(2026-09-20 第 97 轮):
//! - 原 shallow_tree_tests + key_combo_tests 逐行搬入;
//! - 第 97 轮新增 `click_drag_combo_tests` 模块:覆盖点击/拖拽复合行为的
//!   修饰键解析(`parse_modifier_flags`)与别名前后兼容。

#![allow(non_snake_case)]

use super::ControlNode;
use super::*;

// =====================================================================
// 原 shallow_tree_tests(第 81 轮搬入,逐行不变)
// =====================================================================

#[cfg(test)]
mod shallow_tree_tests {
    use super::*;

    fn node(role: &str, children: Vec<ControlNode>) -> ControlNode {
        ControlNode {
            role: role.into(),
            children,
            ..Default::default()
        }
    }

    #[test]
    fn wechat_chrome_only_tree_is_shallow() {
        // 第 81 轮根因场景:微信 4.x 实测空壳树 = 根 AXWindow + 3 个红绿灯 AXButton。
        // 必须判为浅树以触发 AXEnhancedUserInterface 建树等待重试。
        let tree = node(
            "AXWindow",
            vec![
                node("AXButton", vec![]),
                node("AXButton", vec![]),
                node("AXButton", vec![]),
            ],
        );
        assert!(
            inspect::tree_is_shallow(&tree),
            "窗口 + 3 红绿灯按钮的空壳树应判为浅树"
        );
    }

    #[test]
    fn container_only_tree_is_shallow() {
        let tree = node(
            "AXWindow",
            vec![node("AXGroup", vec![node("AXScrollArea", vec![])])],
        );
        assert!(inspect::tree_is_shallow(&tree), "纯容器树应判为浅树");
    }

    #[test]
    fn tree_with_content_control_is_not_shallow() {
        let tree = node(
            "AXWindow",
            vec![
                node("AXButton", vec![]),
                node("AXButton", vec![]),
                node("AXButton", vec![]),
                node("AXTextField", vec![]),
            ],
        );
        assert!(
            !inspect::tree_is_shallow(&tree),
            "含输入框等真实控件树不应判为浅树"
        );
    }

    #[test]
    fn action_pure_trees_are_not_shallow() {
        // 常规应用窗口:大量内容控件 → 深树
        let tree = node(
            "AXWindow",
            vec![
                node("AXToolbar", vec![node("AXButton", vec![])]),
                node(
                    "AXList",
                    vec![node("AXStaticText", vec![]), node("AXStaticText", vec![])],
                ),
            ],
        );
        assert!(!inspect::tree_is_shallow(&tree));
    }
}

// =====================================================================
// 原 key_combo_tests(第 90 轮搬入,逐行不变)
// =====================================================================

#[cfg(test)]
mod key_combo_tests {
    use super::cg_event::*;

    #[test]
    fn parse_single_key_without_modifier() {
        assert_eq!(parse_key_combo("enter"), Some((36, 0)));
        assert_eq!(parse_key_combo("f"), Some((0x03, 0)));
        assert_eq!(parse_key_combo("tab"), Some((48, 0)));
    }

    #[test]
    fn parse_cmd_letter_combo() {
        // 微信搜索联系人关键组合:cmd+f
        let (kc, flags) = parse_key_combo("cmd+f").expect("cmd+f 应可解析");
        assert_eq!(kc, 0x03);
        assert_eq!(flags, 0x0010_0000);
    }

    #[test]
    fn parse_multi_modifier_combo() {
        let (kc, flags) = parse_key_combo("ctrl+shift+t").expect("ctrl+shift+t 应可解析");
        assert_eq!(kc, 0x11);
        assert_eq!(flags, 0x0004_0000 | 0x0002_0000);
    }

    #[test]
    fn parse_modifier_aliases() {
        assert_eq!(parse_key_combo("command+enter").map(|(_, f)| f), Some(0x0010_0000));
        assert_eq!(parse_key_combo("option+space").map(|(_, f)| f), Some(0x0008_0000));
        assert_eq!(parse_key_combo("cmd+0").map(|(k, _)| k), Some(0x1D));
    }

    #[test]
    fn parse_unknown_key_rejected() {
        assert_eq!(parse_key_combo("cmd+notakey"), None);
        assert_eq!(parse_key_combo("banana+f"), None);
        assert_eq!(parse_key_combo(""), None);
    }

    // ===== 第 90 轮:纯修饰键规格(修饰键 + 鼠标同时操作) =====

    #[test]
    fn parse_modifier_flags_round90() {
        assert_eq!(parse_modifier_flags("ctrl"), Some(0x0004_0000));
        assert_eq!(parse_modifier_flags("shift"), Some(0x0002_0000));
        assert_eq!(parse_modifier_flags("alt"), Some(0x0008_0000));
        assert_eq!(parse_modifier_flags("cmd"), Some(0x0010_0000));
        assert_eq!(
            parse_modifier_flags("ctrl+shift"),
            Some(0x0004_0000 | 0x0002_0000)
        );
        assert_eq!(
            parse_modifier_flags("Cmd+Option"),
            Some(0x0010_0000 | 0x0008_0000)
        );
        // 主键混入修饰键规格 → None(规格语义非法)
        assert_eq!(parse_modifier_flags("ctrl+a"), None);
        assert_eq!(parse_modifier_flags("enter"), None);
        assert_eq!(parse_modifier_flags(""), None);
        assert_eq!(parse_modifier_flags("unknown"), None);
    }
}

// =====================================================================
// 第 97 轮新增:click_drag_combo_tests
// 覆盖点击/拖拽复合行为:多修饰键组合 + 别名前后兼容 + 修饰键与点击/拖拽共用 flags
// =====================================================================

#[cfg(test)]
mod click_drag_combo_tests {
    use super::cg_event::*;

    /// 修饰键常量(与 ax_event.rs parse_modifier_flags / modifier_flag_for_name 的位定义保持一致)。
    const FLAG_SHIFT: u64 = 0x0002_0000;
    const FLAG_CTRL: u64 = 0x0004_0000;
    const FLAG_ALT: u64 = 0x0008_0000;
    const FLAG_CMD: u64 = 0x0010_0000;

    #[test]
    fn parse_three_modifier_combo() {
        // 三修饰键组合(浏览器 devtools 的 cmd+ctrl+alt+r 等类似组合)
        let flags = parse_modifier_flags("ctrl+shift+alt").expect("三修饰键组合应可解析");
        assert_eq!(flags, FLAG_CTRL | FLAG_SHIFT | FLAG_ALT);
    }

    #[test]
    fn parse_four_modifier_combo() {
        // 全四修饰键:cmd+ctrl+shift+alt(几乎不实际使用,但应解析)
        let flags = parse_modifier_flags("cmd+ctrl+shift+alt")
            .expect("全四修饰键组合应可解析");
        assert_eq!(
            flags,
            FLAG_CMD | FLAG_CTRL | FLAG_SHIFT | FLAG_ALT
        );
    }

    #[test]
    fn parse_modifier_with_whitespace_tolerated() {
        // 空格容忍:两侧空格 + 多空格;trim() 后应等价
        let flags_a = parse_modifier_flags("  ctrl + shift  ").expect("带空格应可解析");
        let flags_b = parse_modifier_flags("ctrl+shift").expect("标准写法应可解析");
        assert_eq!(flags_a, flags_b);
        assert_eq!(flags_a, FLAG_CTRL | FLAG_SHIFT);
    }

    #[test]
    fn parse_modifier_case_insensitive() {
        // 大小写不敏感(LLM 偶发大写)
        let lower = parse_modifier_flags("ctrl+shift").unwrap();
        let upper = parse_modifier_flags("CTRL+SHIFT").unwrap();
        let mixed = parse_modifier_flags("Ctrl+SHIFT").unwrap();
        assert_eq!(lower, upper);
        assert_eq!(lower, mixed);
        assert_eq!(lower, FLAG_CTRL | FLAG_SHIFT);
    }

    #[test]
    fn parse_modifier_emoji_aliases() {
        // emoji 别名(部分 LLM 训练数据中混用):⇧ shift / ⌃ ctrl / ⌥ alt / ⌘ cmd
        let shift = parse_modifier_flags("⇧").unwrap();
        let ctrl = parse_modifier_flags("⌃").unwrap();
        let alt = parse_modifier_flags("⌥").unwrap();
        let cmd = parse_modifier_flags("⌘").unwrap();
        assert_eq!(shift, FLAG_SHIFT);
        assert_eq!(ctrl, FLAG_CTRL);
        assert_eq!(alt, FLAG_ALT);
        assert_eq!(cmd, FLAG_CMD);

        // emoji + 英文名混用
        let combo = parse_modifier_flags("⌘+⇧").expect("emoji 混用应可解析");
        assert_eq!(combo, FLAG_CMD | FLAG_SHIFT);
    }

    #[test]
    fn parse_modifier_all_aliases_map_to_same_flag() {
        // 同一修饰键的所有别名必须映射到相同的 flags 位 —— 这是点击/拖拽
        // 修饰键一致性的核心:cg_click_at_ex 与 cg_drag 共用 parse_modifier_flags
        // 产出的 flags 位,任一别名改另一别名行为应不变。
        let shift_variants = ["shift", "SHIFT", "Shift", "⇧"];
        for v in shift_variants {
            assert_eq!(
                parse_modifier_flags(v),
                Some(FLAG_SHIFT),
                "shift 别名 {v:?} 必须映射到 0x{:X}",
                FLAG_SHIFT
            );
        }
        let ctrl_variants = ["ctrl", "control", "Control", "CTRL", "⌃"];
        for v in ctrl_variants {
            assert_eq!(
                parse_modifier_flags(v),
                Some(FLAG_CTRL),
                "ctrl 别名 {v:?} 必须映射到 0x{:X}",
                FLAG_CTRL
            );
        }
        let alt_variants = ["alt", "option", "opt", "ALT", "Option", "⌥"];
        for v in alt_variants {
            assert_eq!(
                parse_modifier_flags(v),
                Some(FLAG_ALT),
                "alt 别名 {v:?} 必须映射到 0x{:X}",
                FLAG_ALT
            );
        }
        let cmd_variants = ["cmd", "command", "meta", "CMD", "Command", "Meta", "⌘"];
        for v in cmd_variants {
            assert_eq!(
                parse_modifier_flags(v),
                Some(FLAG_CMD),
                "cmd 别名 {v:?} 必须映射到 0x{:X}",
                FLAG_CMD
            );
        }
    }

    #[test]
    fn cg_mod_flags_empty_spec_returns_zero() {
        // 空规格 = 0;None / Some("") / 纯空白 都应返回 0(零行为变更,与原实现一致)
        assert_eq!(cg_mod_flags(None).unwrap(), 0);
        assert_eq!(cg_mod_flags(Some("")).unwrap(), 0);
        assert_eq!(cg_mod_flags(Some("   ")).unwrap(), 0);
    }

    #[test]
    fn cg_mod_flags_valid_spec_returns_correct_bits() {
        // 合法修饰键规格应返回对应 flags 位
        assert_eq!(cg_mod_flags(Some("ctrl")).unwrap(), FLAG_CTRL);
        assert_eq!(
            cg_mod_flags(Some("ctrl+shift")).unwrap(),
            FLAG_CTRL | FLAG_SHIFT
        );
        assert_eq!(cg_mod_flags(Some("cmd")).unwrap(), FLAG_CMD);
    }

    #[test]
    fn cg_mod_flags_invalid_spec_returns_platform_err() {
        // 非法修饰键规格 → AgentError(包装了 macos 平台错误)
        // 主键混入 → 拒绝
        let err = cg_mod_flags(Some("ctrl+a")).expect_err("主键混入应拒绝");
        let msg = format!("{err}");
        assert!(msg.contains("macos"), "错误应标记平台为 macos: {msg}");
        assert!(msg.contains("ctrl+a"), "错误应包含原始 spec: {msg}");

        // 完全未知段
        let err = cg_mod_flags(Some("unknown")).expect_err("未知修饰键应拒绝");
        let msg = format!("{err}");
        assert!(msg.contains("macos"));
        assert!(msg.contains("unknown"));
    }

    #[test]
    fn cg_mod_note_for_human_readable_text() {
        // cg_mod_note:修饰键规格 → 返回文案后缀(LLM 视角)
        let s_ctrl = Some("ctrl".to_string());
        assert_eq!(cg_mod_note(&s_ctrl), " + 按住 ctrl");
        let s_multi = Some("cmd+shift".to_string());
        assert_eq!(cg_mod_note(&s_multi), " + 按住 cmd+shift");
        let s_none: Option<String> = None;
        assert_eq!(cg_mod_note(&s_none), "");
    }

    #[test]
    fn parse_modifier_flags_click_and_drag_share_bits() {
        // 核心契约:parse_modifier_flags 的输出 flags 既被 cg_click_at_ex 消费(点击修饰键),
        // 也被 cg_drag 消费(拖拽修饰键),且与 CGEventSetFlags 接口位宽完全一致(u64);
        // 这里只断言位值一致(实际投递需在真机),并验证组合的 4 修饰键总和位数正确。
        let all = parse_modifier_flags("cmd+ctrl+shift+alt").unwrap();
        // 4 个独立位各占 1bit → 4 bits set
        assert_eq!(all.count_ones(), 4);
        // 任何子集都是子集
        assert!(all & FLAG_CMD == FLAG_CMD);
        assert!(all & FLAG_CTRL == FLAG_CTRL);
        assert!(all & FLAG_SHIFT == FLAG_SHIFT);
        assert!(all & FLAG_ALT == FLAG_ALT);
    }
}
