//! window 工具模块单元测试(2026-09-17 自 tools/window.rs 拆分)。

    use super::*;

    fn deep_tree(depth: usize) -> ControlNode {
        let mut node = ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "root".into(),
            ..Default::default()
        };
        let mut cur = &mut node;
        for i in 0..depth {
            cur.children.push(ControlNode {
                path: format!("/{i}"),
                role: "Pane".into(),
                name: format!("n{i}"),
                ..Default::default()
            });
            cur = cur.children.last_mut().unwrap();
        }
        node
    }

    #[test]
    fn count_nodes_counts_all() {
        let t = deep_tree(5);
        assert_eq!(count_nodes(&t), 6);
    }

    #[test]
    fn prune_nodes_respects_budget() {
        let mut t = ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "root".into(),
            ..Default::default()
        };
        for i in 0..10 {
            t.children.push(ControlNode {
                path: format!("/{i}"),
                role: "Button".into(),
                name: format!("b{i}"),
                ..Default::default()
            });
        }
        let mut budget = 4;
        prune_nodes(&mut t, &mut budget);
        assert_eq!(count_nodes(&t), 5); // root + 4
    }

    #[test]
    fn tree_to_json_truncates_huge_tree() {
        // 构造超过节点上限的树
        let mut root = ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "root".into(),
            ..Default::default()
        };
        for i in 0..600 {
            root.children.push(ControlNode {
                path: format!("/{i}"),
                role: "Button".into(),
                name: format!("btn-{i}"),
                ..Default::default()
            });
        }
        let s = tree_to_json(root);
        assert!(s.contains("[truncated]"), "应含截断标注");
    }

    #[tokio::test]
    async fn window_action_validates_params() {
        let t = WindowActionTool;
        // 缺 path
        let err = t
            .execute(json!({"window_id": "1", "action": "click"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("path"));
        // set_text 缺 text
        let err = t
            .execute(json!({"window_id": "1", "path": "/", "action": "set_text"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("text"));
        // 未知 action
        let err = t
            .execute(json!({"window_id": "1", "path": "/", "action": "explode"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("未知 action"));
    }

    #[tokio::test]
    async fn window_inspect_validates_window_id() {
        let t = WindowInspectTool;
        let err = t.execute(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("window_id"));
    }

    #[tokio::test]
    async fn window_find_validates_query() {
        let t = WindowFindTool;
        let err = t.execute(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("query"));
    }

    #[test]
    fn damerau_levenshtein_basic() {
        // 完全相等
        assert_eq!(damerau_levenshtein("abc", "abc"), 0);
        // 单字符插入
        assert_eq!(damerau_levenshtein("abc", "abcd"), 1);
        // 单字符替换
        assert_eq!(damerau_levenshtein("abc", "abd"), 1);
        // 完全不等
        assert!(damerau_levenshtein("abc", "xyz") > 0);
        // 上界剪枝:长度差 > 8 直接返回 max_len("abcdefghijklmnop" = 16 字符)
        let d = damerau_levenshtein("ab", "abcdefghijklmnop");
        assert_eq!(d, 16);
    }

    #[test]
    fn match_mode_parse_aliases() {
        assert_eq!(MatchMode::parse("exact"), MatchMode::Exact);
        assert_eq!(MatchMode::parse("EXACT"), MatchMode::Exact);
        assert_eq!(MatchMode::parse("contains"), MatchMode::Contains);
        assert_eq!(MatchMode::parse("fuzzy"), MatchMode::Fuzzy);
        assert_eq!(MatchMode::parse("unknown"), MatchMode::Contains); // 默认 contains
    }

    fn make_wins() -> Vec<WindowInfo> {
        vec![
            WindowInfo {
                id: "w-1".into(),
                title: "微信(WeChat)".into(),
                process_name: "WeChat".into(),
                pid: 100,
                bounds: Default::default(),
                cg_window_id: Some(1001),
                hwnd: None,
                wmctrl_id: None,
            },
            WindowInfo {
                id: "w-2".into(),
                title: "无标题.txt - 记事本".into(),
                process_name: "Notepad".into(),
                pid: 101,
                bounds: Default::default(),
                cg_window_id: None,
                hwnd: None,
                wmctrl_id: None,
            },
            WindowInfo {
                id: "w-3".into(),
                title: "Settings".into(),
                process_name: "System Preferences".into(),
                pid: 102,
                bounds: Default::default(),
                cg_window_id: None,
                hwnd: None,
                wmctrl_id: None,
            },
        ]
    }

    #[test]
    fn pick_top_hit_exact_matches() {
        let wins = make_wins();
        // "wechat" 精确匹配 w-1 的 process_name(WeChat) —— title 是 "微信(WeChat)"
        // 含括号,大小写归一后不等;process 字段命中。
        let hit = pick_top_hit(&wins, "wechat", MatchMode::Exact).unwrap();
        assert_eq!(hit.info.id, "w-1");
        assert_eq!(hit.matched_field, "process");
        assert!((hit.score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn pick_top_hit_contains_substring() {
        let wins = make_wins();
        // "记事本" 是 "无标题.txt - 记事本" 的子串
        let hit = pick_top_hit(&wins, "记事本", MatchMode::Contains).unwrap();
        assert_eq!(hit.info.id, "w-2");
        assert_eq!(hit.matched_field, "title");
    }

    #[test]
    fn pick_top_hit_fuzzy_finds_typo() {
        let wins = make_wins();
        // "WeChqt"(拼错一个字符) 在 contains 模式无命中,在 fuzzy 模式能找到
        assert!(pick_top_hit(&wins, "WeChqt", MatchMode::Contains).is_none());
        let hit = pick_top_hit(&wins, "WeChqt", MatchMode::Fuzzy).unwrap();
        assert_eq!(hit.info.id, "w-1");
        assert!(hit.score > 0.5);
    }

    #[test]
    fn pick_top_hit_no_match_returns_none() {
        let wins = make_wins();
        let q = "完全不存在的应用xyz";
        assert!(pick_top_hit(&wins, q, MatchMode::Contains).is_none());
        assert!(pick_top_hit(&wins, q, MatchMode::Fuzzy).is_none());
        assert!(pick_top_hit(&wins, q, MatchMode::Exact).is_none());
    }

    #[test]
    fn window_query_aliases_cover_localized_wechat() {
        // 2026-09-16 第 67 轮:新增 Weixin 别名(Windows 微信 4.x 进程名 Weixin.exe)
        assert_eq!(
            expand_window_query("WeChat"),
            vec![
                "WeChat".to_string(),
                "微信".to_string(),
                "Weixin".to_string()
            ]
        );
        assert_eq!(
            expand_window_query("微信"),
            vec![
                "微信".to_string(),
                "WeChat".to_string(),
                "Weixin".to_string()
            ]
        );
        assert_eq!(
            expand_window_query("weixin"),
            vec![
                "weixin".to_string(),
                "WeChat".to_string(),
                "微信".to_string(),
            ]
        );
        assert_eq!(expand_window_query("Safari"), vec!["Safari".to_string()]);
    }

    #[tokio::test]
    #[ignore = "会启动/激活本机微信,仅人工桌面环境验证;不发送消息"]
    async fn window_open_finds_localized_wechat_without_sending() {
        let output = WindowOpenTool
            .execute(json!({"query":"WeChat","wait_seconds":8}))
            .await
            .expect("应能打开/激活微信并定位窗口");
        let value: Value = serde_json::from_str(&output).expect("WindowOpen 应返回 JSON");
        assert_eq!(value["ok"], json!(true));
        assert!(value["window_id"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(
            value["matched_query"].as_str() == Some("WeChat")
                || value["matched_query"].as_str() == Some("微信")
        );
    }

    #[test]
    fn pick_top_hit_picks_higher_score_field() {
        // 测试同 query 在不同字段都有命中
        let wins = vec![
            WindowInfo {
                id: "a".into(),
                title: "abc".into(), // contains "abc"
                process_name: "x".into(),
                pid: 1,
                bounds: Default::default(),
                cg_window_id: None,
                hwnd: None,
                wmctrl_id: None,
            },
            WindowInfo {
                id: "b".into(),
                title: "x".into(),
                process_name: "abc".into(),
                pid: 2,
                bounds: Default::default(),
                cg_window_id: None,
                hwnd: None,
                wmctrl_id: None,
            },
        ];
        let hit = pick_top_hit(&wins, "abc", MatchMode::Contains).unwrap();
        assert!(hit.info.id == "a" || hit.info.id == "b");
        assert!(hit.score > 0.3);
    }


    // ========== 2026-09-16 第 60 轮:macOS 辅助功能权限请求测试 ==========

    /// 测试 AxPermissionResult 枚举的创建和匹配。
    #[cfg(target_os = "macos")]
    #[test]
    fn ax_permission_result_granted() {
        let result = AxPermissionResult::Granted;
        match result {
            AxPermissionResult::Granted => {} // 正确
            _ => panic!("应为 Granted"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ax_permission_result_denied() {
        let msg = "测试拒绝消息".to_string();
        let result = AxPermissionResult::Denied {
            message: msg.clone(),
        };
        match result {
            AxPermissionResult::Denied { message } => {
                assert_eq!(message, msg);
            }
            _ => panic!("应为 Denied"),
        }
    }

    /// 测试 driver_preflight 在 macOS 上的权限请求行为。
    /// 注意:此测试仅在 macOS 上运行,且会触发系统权限弹窗(如果未授权)。
    /// 为避免干扰正常测试流程,使用 #[ignore] 标记,需要时手动运行:
    ///   cargo test --lib -- --ignored
    #[tokio::test]
    #[ignore = "会触发系统权限弹窗,需手动运行"]
    async fn driver_preflight_requests_ax_permission_on_macos() {
        if cfg!(target_os = "macos") {
            unsafe {
                std::env::set_var("LAEW_AX_WAIT_SECS", "0");
            }
            // 调用 driver_preflight,如果未授权应触发弹窗
            let result = driver_preflight("TestTool").await;
            // 结果取决于用户是否授权:
            // - 已授权 → Ok(())
            // - 未授权但用户弹窗后授权 → Ok(())
            // - 未授权且用户拒绝 → Err(...)
            // 不断言具体结果,只确保不 panic
            match result {
                Ok(()) => println!("权限已授权"),
                Err(e) => println!("权限请求结果: {e}"),
            }
        }
    }
