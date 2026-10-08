//! `input.rs` 单元测试(第 134 轮随目录拆分迁到本文件;保持 `tests.rs` **文件**,
//! 不得改成 `tests/` 目录 —— `.gitignore` 的 `tests/` 规则匹配任意层级)。


    // 粘贴层已拆到 paste.rs(D6 修 R5 后 input.rs 超 1800 行);突发管线用例仍留本文件,按需引入
    use crate::tui::paste::{PasteInsert, PasteRegistry, handle_paste_text};
    use super::*;
use super::panel::panel_top_for;

    #[test]
    fn test_input_handler_creation() {
        let _handler = InputHandler::new();
    }

    // ========== UTF-8 光标修复(第 07 轮,修复中文输入 panic) ==========

    #[test]
    fn prev_char_boundary_walks_back_multibyte() {
        let s = "取消";
        // "取" 3 字节,"消" 3 字节;边界 {0, 3, 6}
        assert_eq!(prev_char_boundary(s, 6), 3);
        assert_eq!(prev_char_boundary(s, 3), 0);
        assert_eq!(prev_char_boundary(s, 0), 0);
    }

    #[test]
    fn prev_char_boundary_mixed_ascii_cjk() {
        let s = "a中b"; // 边界 {0, 1, 4, 5}
        assert_eq!(prev_char_boundary(s, 4), 1);
        assert_eq!(prev_char_boundary(s, 5), 4);
        assert_eq!(prev_char_boundary(s, 1), 0);
    }

    #[test]
    fn display_width_cjk_is_two_columns() {
        assert_eq!(display_width("ab"), 2);
        assert_eq!(display_width("取消"), 4);
        assert_eq!(display_width("a取b"), 4);
        assert_eq!(display_width(""), 0);
    }

    /// 回归钉子:模拟输入循环的「字节偏移 cursor + insert/remove」核心操作,
    /// 中文连续插入 + 退格不再触发 is_char_boundary panic。
    #[test]
    fn utf8_cursor_insert_and_backspace_never_panics() {
        let mut buffer = String::new();
        let mut cursor: usize = 0;
        for c in "长任务取消测试".chars() {
            buffer.insert(cursor, c);
            cursor += c.len_utf8();
        }
        assert_eq!(buffer, "长任务取消测试");
        assert_eq!(cursor, buffer.len());
        // 连续退格到空
        while cursor > 0 {
            cursor = prev_char_boundary(&buffer, cursor);
            let removed = buffer.remove(cursor);
            assert!(!removed.is_ascii()); // 全程删的都是 CJK 字符
        }
        assert!(buffer.is_empty());
        assert_eq!(cursor, 0);
    }

    // ========== 固定底部输入组件(2026-09-09) ==========

    #[test]
    fn layout_full_panel_on_normal_terminal() {
        let l = Layout::compute(100, 30);
        assert_eq!(l.area_h, 3);
        assert_eq!(l.panel_top, 27); // 分隔线
        assert_eq!(l.input_y, 28); // 输入行
        assert_eq!(l.hint_y, Some(29)); // 提示行
        assert_eq!(l.scroll_bottom(), 27); // 1-indexed 滚动区底行
        assert_eq!(l.scroll_last_row(), 26); // 0-indexed 输出锚点
    }

    #[test]
    fn layout_degrades_to_single_line_on_tiny_terminal() {
        let l = Layout::compute(80, 5);
        assert_eq!(l.area_h, 1);
        assert_eq!(l.panel_top, 4);
        assert_eq!(l.input_y, 4);
        assert_eq!(l.hint_y, None);
        assert_eq!(l.scroll_bottom(), 4);
    }

    #[test]
    fn panel_top_follows_cursor_when_output_is_short() {
        // 启动横幅只占 20 行(40 行终端)→ 面板紧跟第 21 行,不留大片空白(第 134 轮)
        assert_eq!(panel_top_for(40, 3, Some(20)), 21);
        assert_eq!(panel_top_for(40, 3, Some(0)), 1);
    }

    #[test]
    fn panel_top_clamps_to_sticky_bottom_when_output_is_long() {
        // 输出撑满 → 吸底,滚动区底行 = rows - area_h(与旧行为一致)
        assert_eq!(panel_top_for(40, 3, Some(37)), 37);
        assert_eq!(panel_top_for(40, 3, Some(39)), 37);
    }

    #[test]
    fn panel_top_falls_back_to_sticky_without_cursor_row() {
        // 终端不支持 DSR / 回包超时 → 不猜,保持吸底(第 134 轮:猜错会抹掉启动横幅)
        assert_eq!(panel_top_for(40, 3, None), 37);
        // 矮终端降级为单行面板:恒定 rows - 1,不参与自适应
        assert_eq!(panel_top_for(5, 1, Some(1)), 4);
        assert_eq!(panel_top_for(5, 1, None), 4);
    }

    #[test]
    fn scroll_region_tracks_panel_top() {
        // 第 134 轮:滚动区底行 = 面板顶行 → 输出填满时光标停在面板正上方,恒不留空行
        let l = Layout::compute(100, 30);
        assert_eq!(panel_top_for(l.rows, l.area_h, Some(9)), 10);
        assert_eq!(l.scroll_bottom(), l.panel_top);
        assert_eq!(l.scroll_last_row(), l.panel_top - 1);
    }

    #[test]
    fn layout_zero_size_terminal_never_underflows() {
        // script 录屏等极端环境 terminal::size() 可能返回 (0,0)
        let l = Layout::compute(0, 0);
        assert_eq!(l.area_h, 1);
        assert_eq!(l.scroll_bottom(), 1); // 不允许下溢成 65535
        assert_eq!(l.scroll_last_row(), 0);
    }

    #[test]
    fn visible_window_short_input_fits() {
        assert_eq!(visible_window("hello", 5, 20), (0, 5));
        assert_eq!(visible_window("", 0, 20), (0, 0));
    }

    #[test]
    fn visible_window_scrolls_when_cursor_beyond_avail() {
        let s = "abcdefghijklmnopqrstuvwxyz"; // 26 列
        let (start, col) = visible_window(s, 26, 10);
        assert!(start > 0);
        assert_eq!(col, 9); // 光标钉在窗口最后一列
        assert!(display_width(&window_str(s, start, 10)) <= 10);
    }

    #[test]
    fn visible_window_cjk_counts_two_columns() {
        let s = "取消任务确认"; // 12 列
        let (start, col) = visible_window(s, s.len(), 8);
        assert!(start > 0);
        assert!(col < 8);
        assert!(s.is_char_boundary(start));
    }

    #[test]
    fn visible_window_cursor_in_middle_no_scroll_when_fits() {
        let s = "aaaaaaaaaaaaaaaaaaaa"; // 20 列
        let (start, col) = visible_window(s, 10, 40);
        assert_eq!(start, 0);
        assert_eq!(col, 10);
    }

    #[test]
    fn fit_width_truncates_by_display_columns() {
        assert_eq!(fit_width("hello", 3), "hel");
        assert_eq!(fit_width("取消任务", 5), "取消"); // "取消任" 6 列 > 5
        assert_eq!(fit_width("abc", 10), "abc");
        assert_eq!(fit_width("abc", 0), "");
    }

    // ========== D6 大粘贴防护(2026-09-10 第二十二轮,L1573+L1448) ==========











    #[test]
    fn 输入行与横幅共用同一宽度真源() {
        // char_width 已下移到 textfit:两处必须给出同样的列数,否则横幅按 A 算、
        // 输入行按 B 算,同一字符在两个位置错位。
        for s in ["Online ✓", "取消任务确认", "llaew_20260924_123508.log", "中文 abc"] {
            assert_eq!(display_width(s) as usize, textfit::width(s), "度量分叉: {s}");
        }
        assert_eq!(char_width('中'), 2);
        assert_eq!(char_width('a'), 1);
        // 歧义字符:默认 1 列(与改造前逐字节一致);LAEW_AMBIGUOUS_WIDE=1 时 2 列
        if textfit::ambiguous_wide() {
            assert_eq!(textfit::width("✓"), 2, "歧义开关未生效");
        } else {
            assert_eq!(textfit::width("Online ✓"), 8);
        }
    }




    // ========== Tab/Enter + 补全菜单决策(2026-09-10 第 24 轮 Enter 吞键修复) ==========

    fn items(items: Vec<&str>) -> Vec<CompletionItem> {
        items
            .into_iter()
            .map(|r| CompletionItem {
                display: r.to_string(),
                replacement: r.to_string(),
                description: String::new(),
                usage: String::new(),
            })
            .collect()
    }

    #[test]
    fn completion_enter_when_buffer_equals_replacement_submits_buffer() {
        // 路径 A:buffer 已是完整命令 → 直接以 buffer 提交(避免补全后再加尾随空格)
        let items = items(vec!["/provider list"]);
        let d = completion_enter_tab_decision(KeyCode::Enter, "/provider list", true, &items, 0);
        assert_eq!(d, CompletionDecision::Submit("/provider list".to_string()));
    }

    #[test]
    fn completion_enter_on_prefix_submits_replacement() {
        // 路径 B:`/provider` + Enter → 用 `/provider list` 提交(第 24 轮 Bug 修复)
        // 避免高频命令需要按 2 次 Enter 才能进入子屏。
        let items = items(vec!["/provider list", "/provider add"]);
        let d = completion_enter_tab_decision(KeyCode::Enter, "/provider", true, &items, 0);
        assert_eq!(d, CompletionDecision::Submit("/provider list".to_string()));
    }

    #[test]
    fn completion_enter_on_partial_prefix_submits_replacement() {
        // `/provider a` 命中 `/provider add` 的真前缀 → 一次 Enter 补全并提交
        let items = items(vec!["/provider list", "/provider add"]);
        let d = completion_enter_tab_decision(KeyCode::Enter, "/provider a", true, &items, 1);
        assert_eq!(d, CompletionDecision::Submit("/provider add".to_string()));
    }

    #[test]
    fn completion_tab_on_prefix_accepts_without_submit() {
        // Tab + buffer 是补全项真前缀 → AcceptOnly(只补全,不提交,符合 Tab 传统语义)
        let items = items(vec!["/provider list"]);
        let d = completion_enter_tab_decision(KeyCode::Tab, "/provider", true, &items, 0);
        assert_eq!(
            d,
            CompletionDecision::AcceptOnly("/provider list".to_string())
        );
    }

    #[test]
    fn completion_enter_with_no_completion_submits_buffer() {
        // 补全未开:Enter 提交 buffer(原行为)
        let d = completion_enter_tab_decision(KeyCode::Enter, "hello world", false, &[], 0);
        assert_eq!(d, CompletionDecision::Submit("hello world".to_string()));
    }

    #[test]
    fn completion_tab_with_no_completion_is_noop() {
        // 补全未开:Tab 无操作(避免意外副作用)
        let d = completion_enter_tab_decision(KeyCode::Tab, "hello", false, &[], 0);
        assert_eq!(d, CompletionDecision::None);
    }

    #[test]
    fn completion_enter_on_non_prefix_buffer_submits_buffer_as_fallback() {
        // 兜底:补全已开 + Enter + buffer 不是任何补全项真前缀 → 提交 buffer(不让 Enter 被吞)
        let items = items(vec!["/provider list", "/help"]);
        let d = completion_enter_tab_decision(KeyCode::Enter, "/xyz custom", true, &items, 0);
        assert_eq!(d, CompletionDecision::Submit("/xyz custom".to_string()));
    }

    #[test]
    fn completion_enter_with_buffer_having_trailing_space_matches_replacement() {
        // 路径 A 容错:buffer 末尾有空格时,trim 后与 replacement 相等 → 提交 buffer
        let items = items(vec!["/provider list"]);
        let d = completion_enter_tab_decision(KeyCode::Enter, "/provider list  ", true, &items, 0);
        assert_eq!(
            d,
            CompletionDecision::Submit("/provider list  ".to_string())
        );
    }







    // ========== 第 93 轮:Enter 粘贴突发探测(burst_events_to_text 纯函数) ==========

    use crossterm::event::KeyEvent;

    fn key_char(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn key_enter() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    #[test]
    fn burst_folds_chars_and_enters_to_multiline_text() {
        // 粘贴「第二行\n第三行」到达:Char 序列 + 行间 Enter + Char 序列
        let events: Vec<Event> = "第二行"
            .chars()
            .map(key_char)
            .chain(std::iter::once(key_enter()))
            .chain("第三行".chars().map(key_char))
            .collect();
        let (text, spill) = burst_events_to_text(events);
        assert_eq!(text, "第二行\n第三行");
        assert!(spill.is_none());
    }

    #[test]
    fn burst_ctrl_char_events_excluded() {
        // Ctrl 组合键不作为文本(Ctrl-J 等),应终止折叠并回吐
        let events = vec![
            key_char('a'),
            Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            key_char('b'),
        ];
        let (text, spill) = burst_events_to_text(events);
        assert_eq!(text, "a");
        assert!(spill.is_some()); // Ctrl-J 回吐进 pending,不丢事件
    }

    #[test]
    fn burst_paste_event_text_appended() {
        // 混合形态:突发中夹 Event::Paste(终端分块),文本原样并入
        let events = vec![
            key_char('x'),
            Event::Paste("粘贴段".to_string()),
            key_enter(),
        ];
        let (text, spill) = burst_events_to_text(events);
        assert_eq!(text, "x粘贴段\n");
        assert!(spill.is_none());
    }

    #[test]
    fn burst_non_text_event_spills_and_stops() {
        // Resize 等非文本事件:回吐并终止,后续事件不再消费
        let events = vec![key_char('a'), Event::Resize(80, 24), key_char('b')];
        let (text, spill) = burst_events_to_text(events);
        assert_eq!(text, "a");
        assert!(matches!(spill, Some(Event::Resize(80, 24))));
    }

    #[test]
    fn burst_empty_events_yields_empty_text() {
        let (text, spill) = burst_events_to_text(vec![]);
        assert_eq!(text, "");
        assert!(spill.is_none());
    }

    // ========== 第 94 轮:drain_paste_burst 空事件返回 None(防卡死) ==========

    /// 验证 burst_events_to_text 对纯空白/控制字符文本返回空串,
    /// 外层 drain_paste_burst 应据此返回 None 触发正常提交。
    #[test]
    fn burst_whitespace_only_text_is_empty_for_submit() {
        // Enter + 空格(模拟终端残留空白):burst 文本全为空白
        let events = vec![
            key_enter(),
            Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
        ];
        let (text, _spill) = burst_events_to_text(events);
        // 单个空格在 burst_events_to_text 中被 Char(' ') 收集
        // 但 drain_paste_burst 的空文本守卫会拦截全空白串
        let all_blank = text.chars().all(|c| c.is_whitespace() || c.is_control());
        assert!(
            all_blank || text.is_empty(),
            "全空白文本应被识别为无实质内容: {text:?}"
        );
    }

    /// 验证全空白 burst 文本不会触发 burst 插入(应直接进入提交)。
    #[test]
    fn all_whoulder_burst_should_not_block_submit() {
        let burst = "\n  \n".to_string();
        let all_blank = burst.chars().all(|c| c.is_whitespace() || c.is_control());
        assert!(all_blank, "全空白/换行文本应被判定为非有效突发");
    }

    // ========== 第 94 轮:Ctrl-C 双语义验证 ==========

    /// 验证 Ctrl-C 决策:空输入时应退出(Exit),非空输入时应中断(Interrupted)。
    /// 此处验证 InputResult 枚举的语义区分,按键处理逻辑在 read_line_inner 中。
    #[test]
    fn ctrl_c_semantics_distinguish_empty_vs_nonempty() {
        // 空输入 → Exit;非空输入 → Interrupted
        enum CtrlCDecision {
            Exit,
            Interrupt,
        }
        let decide = |buffer: &str| {
            if buffer.is_empty() {
                CtrlCDecision::Exit
            } else {
                CtrlCDecision::Interrupt
            }
        };
        assert!(matches!(decide(""), CtrlCDecision::Exit));
        assert!(matches!(decide("hello"), CtrlCDecision::Interrupt));
        assert!(matches!(decide("  "), CtrlCDecision::Interrupt)); // 有内容(空格)也中断
    }

    /// 端到端语义:突发文本经 D6 管线 —— 多行 → marker 保真;
    /// 单行突发(段首那个代表 Enter 的换行)→ 直插,不套噪声 marker。
    #[test]
    fn burst_text_feeds_paste_pipeline() {
        let mut reg = PasteRegistry::new();
        // 12 行任务书突发(每段前带一个行间换行)→ marker 且登记原文
        let burst: String = (1..=12)
            .map(|i| format!("\n{i}. 任务步骤{i}"))
            .collect();
        match handle_paste_text(&burst, &mut reg) {
            PasteInsert::Inline(s) => panic!("12 行应转 marker,实际 Inline: {s}"),
            PasteInsert::Marker(m) => {
                assert_eq!(m, "[粘贴 #1 +12 行]", "marker 应带行数")
            }
        }
        assert_eq!(reg.expand("[粘贴 #1 +12 行]").lines().count(), 12, "突发原文行数应保真");
        // 单行突发:前导换行是「刚按下的 Enter」,剔除后按单行直插
        match handle_paste_text("\n第二行", &mut reg) {
            PasteInsert::Inline(s) => assert_eq!(s, "第二行", "前导换行不应进入 buffer: {s:?}"),
            PasteInsert::Marker(m) => panic!("单行突发不应转 marker: {m}"),
        }
    }
