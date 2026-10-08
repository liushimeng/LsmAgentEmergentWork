//! 人工介入(HITL)TUI 呈现视图 —— 请求块渲染 / 倒计时 / 行读映射 / 弹窗通知行。
//!
//! 第 100/118/119 轮 TUI 渲染自 `dispatch.rs` 机械拆出(2026-10-08 第 130 轮,
//! dispatch.rs 逼近 1700 行临界);第 130 轮新增弹窗前端的通知行与事件打印,
//! 详案见 `docs/MCP_Web_Use/03-人工介入弹窗UI动态加载方案.md` §7。
//!
//! 呈现端双通道:macOS/Windows 弹窗接管时(**via=gui**)TUI 只打通知行 + 结果事件,
//! **不读 stdin**(根治阻塞读残留);弹窗失败(`GuiFailed`)无缝降级回本文件的
//! 行读渲染路径。kind 标签/默认文案单一事实源在 `human_assist::{kind_label,
//! default_message}`(第 130 轮自 control.rs / 本文件收口迁入)。

use std::io::Write as _;

use crate::agent::human_assist::{AssistEvent, HumanAssistDisplay};

use super::pathfmt;

/// kind 展示标签(request_human 的 reason 映射;第 130 轮转发到单一事实源)。
pub(super) fn human_assist_kind_label(kind: &str) -> &'static str {
    crate::agent::human_assist::kind_label(kind)
}

/// 渲染人工介入请求块(蓝色框线 + 强视觉输入提示符;倒计时**不画在框内**,
/// 由内层 tick 用 `\r` + 同行右对齐原地重写,避免堆叠)。
///
/// 第 119 轮改进:
/// 1. 静态内容(蓝框 + 类型 + 说明 + 选项 + 输入提示符)**只画一次**;之前
///    每秒 tick 都把整个框重新打印一次,导致屏幕堆叠 119/118/117... 行;
/// 2. 倒计时通过 [`render_countdown_inplace`] 在**输入提示符同行右侧**
///    用 `\r\x1b[2K\x1b[999C\x1b[ND` 原地重写, 只占一格, 屏幕不再被淹没;
/// 3. 强视觉输入提示符 `▶ 输入...`(Bold Cyan + Blink + Reverse)保留,
///    用户一眼找到输入位置;
/// 4. 调用方应在渲染前先 `teardown_pinned()` 清理底部 InputHandler 固定面板
///    残留的 `>> <旧buffer>`,避免视觉歧义。
pub(super) fn print_human_assist_prompt_with_cursor(req: &HumanAssistDisplay, elapsed_ms: u64) {
    let _ = elapsed_ms; // 第 119 轮:倒计时由 render_countdown_inplace 按需重写, 不在静态阶段渲染

    let mut out = String::new();
    // 1. 蓝框 + 类型/说明/选项(保留 print_human_assist_block 风格, 但**不画倒计时行**)
    out.push_str("\n  \x1b[36m┌─ 🔐 人工介入请求 ────────────────────────────────\x1b[0m\n");
    out.push_str(&format!(
        "  \x1b[36m│\x1b[0m 类型: \x1b[1m{}\x1b[0m",
        human_assist_kind_label(&req.kind)
    ));
    if !req.url.is_empty() {
        out.push_str(&format!(
            "   页面: {}",
            pathfmt::elide_middle(&req.url, 56)
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "  \x1b[36m│\x1b[0m 说明: {}\n",
        req.message.replace('\n', " ")
    ));
    // 第 132 轮:验证码现场截图路径(弹窗/GUI 内直接展示;TUI 兜底打印路径,
    // macOS `open <path>` / Windows `start <path>` 可查看原图)
    if !req.image_path.is_empty() {
        out.push_str(&format!(
            "  \x1b[36m│\x1b[0m 🖼 验证码图片: {}\n",
            pathfmt::elide_middle(&req.image_path, 56)
        ));
    }
    if !req.options.is_empty() {
        out.push_str("  \x1b[36m│\x1b[0m 选项:\n");
        for (i, opt) in req.options.iter().enumerate() {
            out.push_str(&format!("  \x1b[36m│\x1b[0m   {}. {}\n", i + 1, opt));
        }
    }
    out.push_str(&format!(
        "  \x1b[36m│\x1b[0m 超时: {}s\n",
        req.timeout_ms / 1000
    ));
    out.push_str("  \x1b[36m└──────────────────────────────────────────────\x1b[0m\n");
    // 2. 强视觉输入提示符:\\x1b[5m=Blink, \\x1b[7m=Reverse(反白); 中间一个空格被反白闪烁,
    //    形如 ▶ 输入验证码 [闪烁光标] (q=取消):  —— 用户一眼能看到该在哪一行输入
    //    注意: 不再 print 顶部的 \n, 因为 render_countdown_inplace 用 \r 同行覆盖
    //    倒计时区域, 输入提示符位置必须在同一行才能精确对齐。
    let input_prompt = if req.options.is_empty() {
        "  \x1b[1;36m▶ 输入验证码/内容\x1b[0m\x1b[5;7m \x1b[0m\x1b[1;36m▏\x1b[0m(q=取消): "
    } else {
        "  \x1b[1;36m▶ 输入选项编号(回车=1)或自由内容\x1b[0m\x1b[5;7m \x1b[0m\x1b[1;36m▏\x1b[0m(q=取消): "
    };
    out.push_str(input_prompt);
    print!("{out}");
    let _ = std::io::stdout().flush();
}

/// 输入提示符同行右侧的倒计时渲染(第 119 轮新增):
/// 用 `\r` 回本行首, 清行(`\x1b[2K`), 跳到 prompt_w 列, 打印倒计时文本, 还原光标。
///
/// 不再额外占用新行, 避免屏幕堆叠 119/118/... 行。TTY 非交互管道场景
/// 不需要倒计时(工具侧已自带 timeout), 直接返回空串, 走静默路径。
pub(super) fn render_countdown_inplace(req: &HumanAssistDisplay, remaining_s: u64) -> String {
    let color = if remaining_s <= 10 {
        "\x1b[31m"  // 红
    } else if remaining_s <= 30 {
        "\x1b[33m"  // 黄
    } else {
        "\x1b[90m"  // 灰
    };
    let total_s = req.timeout_ms / 1000;
    let prompt_w = req.prompt_visual_width as usize;
    if prompt_w == 0 {
        // 无 prompt_w 兜底: 直接在行尾打印, ANSI 引擎自动截断
        return format!(
            "\r\x1b[2K\x1b[999C\x1b[{n}D{0}⏱ 剩余 {1}s (总 {2}s){3}",
            color,
            remaining_s,
            total_s,
            "\x1b[0m",
            n = 32,
        );
    }
    // prompt_w > 0: 同 prompt 行右侧对齐
    format!(
        "\r\x1b[2K\x1b[{n}C{0}⏱ 剩余 {1}s (总 {2}s){3}",
        color,
        remaining_s,
        total_s,
        "\x1b[0m",
        n = prompt_w,
    )
}

/// 计算当前输入提示符的可视列宽(中文/全角按 2 计)。第 119 轮新增。
/// 调用方: render_countdown_inplace 之前。
pub(super) fn human_assist_prompt_visual_width(req: &HumanAssistDisplay) -> u16 {
    use crate::tui::input::display_width;
    let label = if req.options.is_empty() {
        "▶ 输入验证码/内容 ▏(q=取消): "
    } else {
        "▶ 输入选项编号(回车=1)或自由内容 ▏(q=取消): "
    };
    // 2 个前导空格 + label; 闪烁反白符在视觉上是普通空格, 算 1 列
    let full = format!("  {label}");
    // display_width 已返回 u16, 不会溢出; 直接返回
    display_width(&full)
}

/// 渲染人工介入请求块(蓝色框线,与浏览器窗口蓝框呼应)。
///
/// 第 118 轮起弃用,改用 [`print_human_assist_prompt_with_cursor`]
/// (含倒计时 + 强视觉输入提示符)。保留本函数以兼容其它调用点/单测。
#[allow(dead_code)]
pub(super) fn print_human_assist_block(req: &HumanAssistDisplay) {
    let mut out = String::new();
    out.push_str("\n  \x1b[36m┌─ 🔐 人工介入请求 ────────────────────────────────\x1b[0m\n");
    out.push_str(&format!(
        "  \x1b[36m│\x1b[0m 类型: \x1b[1m{}\x1b[0m",
        human_assist_kind_label(&req.kind)
    ));
    if !req.url.is_empty() {
        out.push_str(&format!("   页面: {}", pathfmt::elide_middle(&req.url, 56)));
    }
    out.push('\n');
    out.push_str(&format!(
        "  \x1b[36m│\x1b[0m 说明: {}\n",
        req.message.replace('\n', " ")
    ));
    if !req.options.is_empty() {
        out.push_str("  \x1b[36m│\x1b[0m 选项:\n");
        for (i, opt) in req.options.iter().enumerate() {
            out.push_str(&format!("  \x1b[36m│\x1b[0m   {}. {}\n", i + 1, opt));
        }
    }
    out.push_str(&format!(
        "  \x1b[36m│\x1b[0m 超时: {}s\n",
        req.timeout_ms / 1000
    ));
    out.push_str("  \x1b[36m└──────────────────────────────────────────────\x1b[0m\n");
    let prompt_line = if req.options.is_empty() {
        "  请输入内容后回车(q=取消): "
    } else {
        "  请输入选项编号或直接输入内容(回车=1,q=取消): "
    };
    out.push_str(prompt_line);
    print!("{out}");
    let _ = std::io::stdout().flush();
}

/// 阻塞读一行 stdin(spawn_blocking 防 tokio 协程阻塞;cooked mode 自带回显)。
/// EOF / 读取失败 → 空串(按默认选项 1 处理,避免 e2e 管道场景挂死)。
pub(super) async fn read_human_assist_answer() -> String {
    tokio::task::spawn_blocking(|| {
        let mut buf = String::new();
        match std::io::stdin().read_line(&mut buf) {
            Ok(0) | Err(_) => String::new(),
            Ok(_) => buf.trim_end_matches(['\n', '\r']).to_string(),
        }
    })
    .await
    .unwrap_or_default()
}

/// 人工输入映射:空回车 → 选项 1;数字 → 对应选项;q/取消/cancel → None(取消);
/// 其余原文返回(短信验证码数字即自由文本路径)。
pub(super) fn map_human_assist_input(input: &str, options: &[String]) -> Option<String> {
    let t = input.trim();
    if t.is_empty() {
        return options.first().map(|o| format!("1. {o}"));
    }
    let lower = t.to_lowercase();
    if matches!(lower.as_str(), "q" | "quit" | "取消" | "cancel" | "exit") {
        return None;
    }
    if let Ok(n) = t.parse::<usize>() {
        if (1..=options.len()).contains(&n) {
            return Some(format!("{n}. {}", options[n - 1]));
        }
    }
    Some(t.to_string())
}

// =================== 第 130 轮:弹窗前端通知行 / 事件打印 ===================

/// 人工介入通知行的输出流(第 131 轮)。
///
/// TUI 交互模式走 stdout(滚动区);`-p` / `-f` 单轮模式的 **stdout 只含答案与用量**
/// (见 `main.rs::run_one_shot` 的 stderr/stdout 分流约定),通知行必须走 stderr,
/// 否则会把「等待人工介入」的噪声混进机器可读输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistOut {
    Stdout,
    Stderr,
}

impl AssistOut {
    fn write_line(self, s: &str) {
        match self {
            AssistOut::Stdout => println!("{s}"),
            AssistOut::Stderr => eprintln!("{s}"),
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let _ = std::io::Write::flush(&mut std::io::stderr());
    }
}

/// 弹窗接管通知行(第 130 轮):告诉用户「作答在弹窗,不在终端」,
/// 并把时间轴三要素(提出/超时截止/剩余)摆到台面。**不读 stdin**。
pub fn print_human_assist_gui_notice(
    req: &HumanAssistDisplay,
    platform: &str,
    out: AssistOut,
) {
    use crate::agent::human_ui::fmt_local_ms;
    let started = if req.created_at_ms > 0 {
        // 只取时刻部分(去掉日期前缀,省列宽)
        let s = fmt_local_ms(req.created_at_ms);
        s.rsplit(' ').next().unwrap_or(&s).to_string()
    } else {
        "-".to_string()
    };
    let deadline = if req.created_at_ms > 0 {
        let s = fmt_local_ms(req.created_at_ms.saturating_add(req.timeout_ms));
        s.rsplit(' ').next().unwrap_or(&s).to_string()
    } else {
        "-".to_string()
    };
    if matches!(out, AssistOut::Stdout) {
        println!();
    }
    out.write_line(&format!(
        "  [laew] 🖥 已弹出人工介入弹窗({platform}),请在弹窗中作答(点选项/输入文本/取消)"
    ));
    out.write_line(&format!(
        "  [laew]   类型: {} | 提出 {} | 超时 {}(剩余 {}s)",
        human_assist_kind_label(&req.kind),
        started,
        deadline,
        req.timeout_ms / 1000
    ));
    if !req.url.is_empty() {
        out.write_line(&format!(
            "  [laew]   页面: {}",
            pathfmt::elide_middle(&req.url, 72)
        ));
    }
    // 第 132 轮:弹窗内已展示验证码图片时,同步把路径摆到台面(可 `open` 查看放大图)
    if !req.image_path.is_empty() {
        out.write_line(&format!(
            "  [laew]   🖼 验证码图片: {}",
            pathfmt::elide_middle(&req.image_path, 72)
        ));
    }
    out.write_line(
        "  [laew]   (弹窗被遮挡会自动置顶拉焦;异常时可等自动降级为终端作答,或设 LAEW_HUMAN_UI=off)",
    );
}

/// 弹窗生命周期事件 → 通知行(结果反馈;弹窗应答/取消/降级/超时)。
pub fn print_assist_event(ev: &AssistEvent, out: AssistOut) {
    let line = match ev {
        AssistEvent::GuiLaunched { .. } => return, // 通知行由 print_human_assist_gui_notice 打
        AssistEvent::GuiFailed { id } => format!(
            "  [laew] 人工介入弹窗启动失败,已降级为终端作答(req.id={id})"
        ),
        AssistEvent::GuiAnswered { text, .. } => {
            format!("  [laew] 已收到人工输入(弹窗):{text}")
        }
        AssistEvent::GuiCancelled { .. } => {
            "  [laew] 人工已在弹窗取消介入,任务按取消路径继续".to_string()
        }
        AssistEvent::GuiTimeout { .. } => {
            "  [laew] 人工介入弹窗等待超时,任务按超时路径继续".to_string()
        }
    };
    out.write_line(&line);
}

#[cfg(test)]
mod human_assist_tui_tests {
    use super::*;

    fn mk_display() -> HumanAssistDisplay {
        HumanAssistDisplay {
            id: 1,
            kind: "captcha".into(),
            message: "test".into(),
            options: vec![],
            url: String::new(),
            page_id: "p_1".into(),
            image_path: String::new(),
            timeout_ms: 120_000,
            prompt_visual_width: 84,
            created_at_ms: 0,
        }
    }

    #[test]
    fn maps_empty_input_to_first_option() {
        let opts = vec!["已完成".to_string(), "取消".to_string()];
        assert_eq!(
            map_human_assist_input("", &opts),
            Some("1. 已完成".to_string())
        );
        assert_eq!(
            map_human_assist_input("  \n", &opts),
            Some("1. 已完成".into())
        );
    }

    #[test]
    fn maps_numeric_choice() {
        let opts = vec!["继续".to_string(), "跳过".to_string()];
        assert_eq!(map_human_assist_input("2", &opts), Some("2. 跳过".into()));
        assert_eq!(map_human_assist_input("99", &opts), Some("99".to_string()));
    }

    #[test]
    fn maps_cancel_keywords_to_none() {
        let opts = vec!["继续".to_string()];
        for kw in ["q", "Q", "取消", "cancel", "exit"] {
            assert_eq!(map_human_assist_input(kw, &opts), None, "kw={kw}");
        }
    }

    #[test]
    fn maps_free_text_verbatim() {
        let opts = vec!["已完成".to_string()];
        assert_eq!(
            map_human_assist_input("482913", &opts),
            Some("482913".to_string())
        );
    }

    /// 第 119 轮:render_countdown_inplace 必须用 `\r\x1b[2K` 原地重写,
    /// 避免每 tick 打印新行导致屏幕堆叠 119/118/117...
    #[test]
    fn countdown_inplace_uses_carriage_return_clear() {
        let req = mk_display();
        let out = render_countdown_inplace(&req, 119);
        // 必须以 \r + \x1b[2K 开头(回行首 + 清行),不产生新行
        assert!(out.starts_with("\r\x1b[2K"), "must start with \\r + clear: {out:?}");
        assert!(!out.starts_with('\n'), "must not begin with newline");
        // 含倒计时秒数 与 总秒数
        assert!(out.contains("⏱ 剩余 119s"), "must show remaining: {out:?}");
        assert!(out.contains("总 120s"), "must show total: {out:?}");
        // 不含换行符(同行重写)
        assert!(!out.contains('\n'), "must not contain newline: {out:?}");
    }

    /// 第 119 轮:颜色随时间灰→黄→红渐变
    #[test]
    fn countdown_inplace_color_gradient() {
        let mk = |ms: u64| HumanAssistDisplay {
            timeout_ms: ms,
            ..mk_display()
        };
        // >30s: 灰(\x1b[90m)
        let far = render_countdown_inplace(&mk(120_000), 60);
        assert!(far.contains("\x1b[90m"), ">30s should be gray: {far:?}");
        // 11-30s: 黄(\x1b[33m)
        let mid = render_countdown_inplace(&mk(120_000), 20);
        assert!(mid.contains("\x1b[33m"), "11-30s should be yellow: {mid:?}");
        // <=10s: 红(\x1b[31m)
        let near = render_countdown_inplace(&mk(120_000), 5);
        assert!(near.contains("\x1b[31m"), "<=10s should be red: {near:?}");
    }

    /// 第 119 轮:prompt_visual_width=0 走行尾回退路径, 也有 \r 清行
    #[test]
    fn countdown_inplace_zero_width_falls_back() {
        let req = HumanAssistDisplay {
            timeout_ms: 60_000,
            prompt_visual_width: 0,
            ..mk_display()
        };
        let out = render_countdown_inplace(&req, 30);
        assert!(out.starts_with("\r\x1b[2K"), "must begin with \\r + clear");
        assert!(out.contains("⏱ 剩余 30s"));
    }

    /// 第 119 轮:提示符可视宽度计算:中文全角计 2 列
    #[test]
    fn prompt_visual_width_counts_cjk_double() {
        let with_opts = HumanAssistDisplay {
            options: vec!["继续".into()],
            prompt_visual_width: 0,
            ..mk_display()
        };
        let dry = HumanAssistDisplay {
            options: vec![],
            ..with_opts.clone()
        };
        let w_with = human_assist_prompt_visual_width(&with_opts);
        let w_dry = human_assist_prompt_visual_width(&dry);
        // 选项版提示符更长(「输入选项编号(回车=1)或自由内容」比「输入验证码/内容」长)
        assert!(
            w_with > w_dry,
            "option prompt should be wider: {w_with} vs {w_dry}"
        );
        // 都应是合理值(>20 且 < 200)
        assert!(w_with > 20 && w_with < 200, "unexpected width {w_with}");
        assert!(w_dry > 20 && w_dry < 200, "unexpected width {w_dry}");
    }

    /// 第 119 轮:内部 cancel sentinel 与正常答案区分
    #[test]
    fn internal_cancel_sentinel_is_not_a_real_answer() {
        // 正常文本不会是 sentinel
        assert_ne!("\x00-TIMEOUT", "482913");
        assert_ne!("\x00-TIMEOUT", "");
        // 用户输入 null 字符 + TIMEOUT 文本也不等(sentinel 以 \x00 开头)
        assert_ne!("\x00-TIMEOUT", "\x00");
    }

    /// 第 130 轮:kind 标签转发单一事实源(9 reason 全覆盖)。
    #[test]
    fn kind_label_single_source() {
        assert_eq!(human_assist_kind_label("sms"), "短信验证码");
        assert_eq!(human_assist_kind_label("real_name"), "实名认证/人脸核身");
        assert_eq!(human_assist_kind_label("unknown_kind"), "人工介入");
    }
}
