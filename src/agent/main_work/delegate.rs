//! delegate_to 委派推断(2026-09-17 自 main_work.rs 拆分)。
//!
//! 基于步骤 / branches / 验收文本的关键词匹配,按「GUI 优先」语义纠正委派:
//! GUI 关键词 → WindowUse;网页关键词且无 GUI 词 → WebUse;仅 shell 词 → SubAgent。

use super::*;

// ========== delegate_to 推断(2026-09-16 第 54 轮补丁 B;第 67 轮语义修正;2026-09-17 第 75 轮 WebUse 优先修正) ==========
//
// 2026-09-16 第 54 轮:实测「打开微信发消息」任务 Main-Work 把含 osascript 步骤的工作流
// delegate_to=windowuse,但当时 WindowUse 没有 Bash 工具,导致 SubAgent 循环无法执行命令。
// 修复:解析 WorkFlowPlan 后,基于步骤文本 + branches 文本 + 验收文本的关键词匹配纠正。
//
// 2026-09-16 第 67 轮语义修正(微信 4.x 实测复盘):WindowUse 已带 Bash 白名单 +
// SendInput/OCR 原生能力,推断优先级改为「GUI 优先」。
//
// 2026-09-17 第 75 轮(本轮):实测「打开网址 wenxin.baidu.com,中间有一个对话的输入框,
// 输入内容..., 找到搜素确认的按钮,点击按钮」任务被路由到 WindowUse,16 轮 0 工具调用
// 失败(Debot 报告 P0-1 / P0-2)。根因:第 67 轮为修微信任务把「输入框」「点击按钮」
// 「鼠标」「滚轮」等中文 GUI 词加进 WINDOW_USE_KEYWORDS,但这些词在 HTML Web 场景同样存在,
// 命中后压制 web_hit → 误判 WindowUse → WindowUseRunner 无 Browser* 工具 → 死循环。
// 修复:把 GUI 关键词拆分为「桌面 GUI 强信号」(WINDOW_USE_STRICT_KEYWORDS)与
// 「Web DOM 通用词」(WEB_DOM_GUI_KEYWORDS);Web 命中即优先 WebUse,仅当桌面应用
// 强信号(微信/钉钉/飞书/WindowList/UIA/控件树 等)出现时才覆盖到 WindowUse。

pub(super) const SUBAGENT_KEYWORDS: &[&str] = &[
    "osascript",
    "applescript",
    "screencapture",
    "cliclick",
    "pbcopy",
    "pbpaste",
    "system events",
    "keystroke",
    "坐标点击",
    "剪贴板",
    "xdotool",
    "wmctrl",
    "adb shell",
    "sendevent",
    "input keyevent",
    "input tap",
    "adb ",
    // 2026-09-16 第 67 轮:移除 "powershell" / "shell" / "uiautomation" / "sendinput" ——
    // 实测微信任务 Main-Work 在 steps 里写「用 PowerShell 检查进程」就会被这几个
    // 过宽词强制改判 SubAgent(GUI 任务被路由到 Bash 路线的直接根因)。
    // UIAutomation/SendInput 恰恰是 WindowUse 驱动层的本职能力,语义反转;
    // WindowUse 自带 Bash 白名单(桌面操控类命令),GUI 优先不会丢失 shell 能力。
];

/// 2026-09-17 第 75 轮:桌面应用专属词 —— 命中基本可锁定 WindowUse。
///
/// 与 Web DOM 词汇无交集:微信/钉钉/飞书 等独立桌面应用,以及
/// `WindowList/WindowInspect/UIA/控件树` 等明确指代 OS 原生窗口/控件树的 API。
/// 命中后将压制 web_hit(防止把「https://example.com 这个链接粘到微信对话框」误判为 WebUse)。
pub(super) const WINDOW_USE_STRICT_KEYWORDS: &[&str] = &[
    // 桌面 OS 窗口 API(UIA/AX/wmctrl 全部)
    "windowlist",
    "windowinspect",
    "windowaction",
    "windowopen",
    "windowocr",
    "windowscreenshot",
    "windowfind",
    "控件树",
    "枚举窗口",
    "ui automation",
    "uiautomation",
    "axpress",
    "set_text",
    "控件路径",
    "无障碍",
    "invoke pattern",
    "sendinput",
    // 桌面应用专属词(独立 App,非 Web)
    "微信",
    "wechat",
    "weixin",
    "钉钉",
    "dingtalk",
    "飞书",
    "feishu",
    "QQ",
    "telegram",
    "lark",
    "wecom",
    "企业微信",
];

/// 2026-09-17 第 75 轮:Web DOM 通用词 —— 在 Web 场景同样存在,不应作为 GUI 强证据。
///
/// 保留以供 LLM 进一步细粒度决策(例如纯「输入框填写 100」无 web/desktop 锚时,作次级辅助),
/// 但**不**进入 `WINDOW_USE_STRICT_KEYWORDS`,不再压制 web_hit。
pub(super) const WEB_DOM_GUI_KEYWORDS: &[&str] = &[
    "输入框",
    "搜索框",
    "对话框",
    "点击按钮",
    "提交按钮",
    "表单",
    "弹窗",
    "页面",
    "点击",
    "鼠标",
    "滚轮",
    "桌面应用",
    "通讯录",
    "聊天窗口",
];

/// 2026-09-17 第 75 轮:为了不破坏 `mod.rs` 兄弟模块 `use super::*` 取用的常量名,保留
/// `WINDOW_USE_KEYWORDS` 别名指向 `WINDOW_USE_STRICT_KEYWORDS`(合并语义,新版推断逻辑
/// 直接用 strict 列表,旧引用仍编译通过)。
pub(super) const WINDOW_USE_KEYWORDS: &[&str] = WINDOW_USE_STRICT_KEYWORDS;

/// 2026-09-16 第 61 轮:浏览器/网页操控关键词(Chromium-WebUse,第 11 角色)。
/// ★ 第 82 轮:补充 AI 对话类网站强信号 + 中文提交按钮 selector 名,覆盖
/// Plan agent 输出极简步骤(如"输入问题文本并提交")时的路由误判问题。
pub(super) const WEB_USE_KEYWORDS: &[&str] = &[
    "browsernew",
    "browsercontrol",
    "browserinspect",
    "browserlist",
    "browserclose",
    "浏览器",
    "网页",
    "网站",
    "网址",
    "爬虫",
    "抓取页面",
    "采集页面",
    "chrome",
    "chromium",
    "cdp",
    "devtools",
    "webdriver",
    "headless",
    "无头浏览器",
    "dom",
    "localstorage",
    "页面截图",
    "表单提交",
    "http://",
    "https://",
    // ★ 第 82 轮:AI 对话类网站 + 提交按钮 selector 强信号
    // 解决 Plan agent 输出"输入文本+点击提交"等极简描述时无 web 锚被误判 SubAgent 的问题。
    "wenxin", "baidu.com", "chatgpt", "deepseek", "kimi", "doubao", "claude.ai", "gemini",
    "ci-submit-button", "submit-button", "提交按钮", "搜素按钮", "搜索按钮", "发送按钮",
    "对话窗口", "对话输入框", "智能助手", "AI助手", "AI 回复", "AI回复",
];

pub(super) fn text_contains_any_ci(text: &str, keywords: &[&str]) -> bool {
    let lower = text.to_lowercase();
    keywords.iter().any(|k| lower.contains(k))
}

/// ★ 第 82 轮:检测文本中是否包含 HTTP/HTTPS URL(覆盖关键词表里没列出的网址)。
/// 用最简 URL regex 抓 `http(s)://xxx`,避开 false-positive(如命令行参数里的 -http)。
fn text_contains_url(text: &str) -> bool {
    let lower = text.to_lowercase();
    // 简单子串扫描 + 协议头判定,避免 regex crate 依赖
    lower.contains("http://") || lower.contains("https://") || lower.contains("www.")
}

pub(super) fn gather_spec_text(spec: &WorkFlowSpec) -> String {
    let mut s = String::new();
    s.push_str(&spec.name);
    s.push('\n');
    for st in &spec.steps {
        s.push_str(st);
        s.push('\n');
    }
    for b in &spec.branches {
        s.push_str(&b.condition);
        s.push('\n');
        s.push_str(&b.then);
        s.push('\n');
    }
    for l in &spec.loops {
        s.push_str(&l.condition);
        s.push('\n');
        s.push_str(&l.over);
        s.push('\n');
    }
    for a in &spec.acceptance {
        s.push_str(a);
        s.push('\n');
    }
    s
}

/// 基于步骤文本推断 delegate_to(返回 None 表示不强行纠正,保留原值)。
///
/// 2026-09-17 第 75 轮推断优先级(修复 web 任务误路由到 windowuse 的 P0 bug):
///   1. 桌面 GUI 强信号(微信/钉钉/飞书/WindowList/UIA/控件树 等) → 强制 WindowUse
///   2. web 命中 + 无 desktop-gui 强信号 → WebUse(无论 shell 是否同时出现,
///      也无论「输入框/按钮」等 Web DOM 词是否出现)
///   3. 仅 shell 词(无 web 无 desktop-gui) → SubAgent
///   4. shell + desktop-gui 强信号 → WindowUse(GUI 优先;WindowUse 自带 Bash 白名单)
///   5. 都没命中 → None(保持 Main-Work 显式选择)
///
/// 与第 67 轮版本的关键差异:不再用「输入框/按钮/鼠标/滚轮」等 Web DOM 词作为
/// GUI 强证据 —— 这些词在 HTML 页面里同样常见,误命中后会压制 web_hit 导致路由错误。
pub fn infer_delegate_to(spec: &WorkFlowSpec) -> Option<AgentRole> {
    let text = gather_spec_text(spec);
    let shell_hit = text_contains_any_ci(&text, SUBAGENT_KEYWORDS);
    let desktop_gui_hit = text_contains_any_ci(&text, WINDOW_USE_STRICT_KEYWORDS);
    let web_hit = text_contains_any_ci(&text, WEB_USE_KEYWORDS);
    // ★ 第 82 轮:URL 出现也作为 web 强信号(关键词表可能漏列新网站)
    let url_hit = text_contains_url(&text);

    // 规则 1:桌面 GUI 强信号 → 强制 WindowUse(覆盖 web/shell)。
    // 例:微信 + osascript 剪贴板 → 仍判 WindowUse(第 67 轮逻辑保留)。
    if desktop_gui_hit {
        return Some(AgentRole::WindowUse);
    }
    // 规则 2:web 命中(无 desktop-gui 强信号)→ WebUse。第 75 轮 P0 修复核心。
    // shell 词同时出现不构成压制(web 任务是主体,shell 是辅助)。
    // ★ 第 82 轮:URL 出现同样判 web(覆盖关键词漏列新域名)。
    if web_hit || url_hit {
        return Some(AgentRole::WebUse);
    }
    // 规则 3:仅 shell 词 → SubAgent(WebUse 没有 Bash,WindowUse 是白名单模式)。
    if shell_hit {
        return Some(AgentRole::SubAgent);
    }
    // 规则 4:都没命中 → 保持 Main-Work 显式选择(不强行覆盖)。
    None
}

/// 对整份 plan 做 delegate_to 推断 + 纠正 + 日志。
pub fn infer_delegate_to_for_plan(plan: &mut WorkFlowPlan) {
    for wf in plan.workflows.iter_mut() {
        if let Some(inferred) = infer_delegate_to(wf) {
            if inferred != wf.delegate_to {
                tracing::info!(
                    wf_id = %wf.id,
                    from = %wf.delegate_to.as_str(),
                    to = %inferred.as_str(),
                    "[2026-09-16 B] delegate_to 已自动纠正(基于步骤关键词推断)"
                );
                wf.delegate_to = inferred;
            }
        }
    }
}

#[cfg(test)]
mod infer_tests {
    use super::*;

    #[test]
    fn shell_steps_route_to_subagent() {
        // 2026-09-16 第 67 轮:纯 shell 流程(不含 GUI 词)仍判 SubAgent。
        // 注:原用例的 'tell application "WeChat" to activate' 因含 GUI 词 WeChat,
        // 新语义下正确改判 WindowUse(激活微信本就是窗口操控),已拆到
        // wechat_powershell_steps_route_to_windowuse / both_keywords_pick_windowuse。
        let spec = WorkFlowSpec {
            id: "wf-1".into(),
            name: "音量查询".into(),
            steps: vec!["执行 osascript -e 'output volume of (get volume settings)'".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::WindowUse, // 显式选错
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }

    #[test]
    fn gui_steps_route_to_windowuse() {
        let spec = WorkFlowSpec {
            id: "wf-2".into(),
            name: "枚举窗口".into(),
            steps: vec![
                "调用 WindowList 找目标窗口".into(),
                "调用 WindowInspect 遍历控件树".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec!["找到目标窗口 id".into()],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WindowUse));
    }

    #[test]
    fn no_keyword_keeps_explicit_choice() {
        let spec = WorkFlowSpec {
            id: "wf-3".into(),
            name: "通用操作".into(),
            steps: vec!["读取项目根目录结构".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), None);
    }

    #[test]
    fn same_wechat_window_chain_is_coalesced() {
        fn wf(id: &str, step: &str, dep: &str) -> WorkFlowSpec {
            WorkFlowSpec {
                id: id.into(),
                name: format!("微信-{step}"),
                steps: vec![step.into()],
                branches: vec![],
                loops: vec![],
                depends_on: if dep.is_empty() {
                    vec![]
                } else {
                    vec![dep.into()]
                },
                acceptance: vec![format!("完成{step}")],
                delegate_to: AgentRole::WindowUse,
                // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
                target_app: None,
            }
        }
        let mut plan = WorkFlowPlan {
            workflows: vec![
                wf("wf-1", "打开微信", ""),
                wf("wf-2", "搜索赵玲玲", "wf-1"),
                wf("wf-3", "发送消息", "wf-2"),
            ],
            summary: String::new(),
            degraded: false,
        };
        coalesce_same_app_window_workflows(&mut plan);
        assert_eq!(plan.workflows.len(), 1);
        assert_eq!(plan.workflows[0].steps.len(), 3);
        assert!(plan.workflows[0].acceptance.len() >= 3);
    }

    #[test]
    fn both_keywords_pick_windowuse() {
        // 2026-09-16 第 67 轮语义修正:混合型(osascript + 控件关键词)→ WindowUse。
        // 旧版判 SubAgent 是「微信任务被路由到 Bash 路线」的直接根因;
        // WindowUse 已带 Bash 白名单 + SendInput/OCR 原生能力,GUI 优先不丢 shell。
        let spec = WorkFlowSpec {
            id: "wf-mix".into(),
            name: "混合".into(),
            steps: vec![
                "osascript -e 'tell application \"WeChat\" to activate'".into(),
                "WindowInspect 检视".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WindowUse));
    }

    #[test]
    fn wechat_powershell_steps_route_to_windowuse() {
        // 第 67 轮核心回归:复现本次失败 —— steps 含「PowerShell 检查进程」+「点击通讯录」,
        // 旧版被 "powershell/shell" 宽词判 SubAgent;新版必须 WindowUse。
        let spec = WorkFlowSpec {
            id: "wf-wx".into(),
            name: "微信操控".into(),
            steps: vec![
                "用 PowerShell 确认微信客户端已启动".into(),
                "找到通讯录按钮并点击".into(),
                "遍历联系人列表找到目标用户".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec!["消息出现在会话窗口".into()],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WindowUse));
    }

    #[test]
    fn pure_shell_steps_still_route_to_subagent() {
        // 纯 shell 流程(无 GUI 词)仍判 SubAgent
        let spec = WorkFlowSpec {
            id: "wf-sh".into(),
            name: "统计".into(),
            steps: vec!["osascript -e 'get volume as string'".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::WindowUse,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }

    // ========== 2026-09-17 第 75 轮 新增回归用例(WebUse 优先 / 桌面 GUI 严判) ==========

    #[test]
    fn wenxin_url_with_input_box_routes_to_webuse() {
        // 第 75 轮 P0 修复核心回归:用户真实失败任务。
        // steps 同时含 web 词(打开网址/wenxin.baidu.com)和 web DOM 词(输入框/按钮),
        // 旧版被"输入框/点击按钮"压制 → WindowUse → 死循环;新版必须 WebUse。
        let spec = WorkFlowSpec {
            id: "wf-wenxin".into(),
            name: "文心一言对话".into(),
            steps: vec![
                "打开网址 https://wenxin.baidu.com/".into(),
                "中间有一个对话的输入框,输入内容:最新的最近3个月的黄金和白银的价格K线图帮忙生成一下".into(),
                "找到搜素确认的按钮,点击按钮".into(),
                "等待对话结束后,把文心一言输出的信息显示出来".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec!["对话窗口出现 AI 回复内容".into()],
            delegate_to: AgentRole::WindowUse, // LLM 错判
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WebUse));
    }

    #[test]
    fn pure_browser_open_url_routes_to_webuse() {
        // 通用 web 场景:无 strict GUI 词,只有 web 词 → WebUse
        let spec = WorkFlowSpec {
            id: "wf-browser-open".into(),
            name: "浏览器截图".into(),
            steps: vec![
                "用浏览器打开 https://example.com/".into(),
                "截屏后保存到本地".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WebUse));
    }

    #[test]
    fn wechat_desktop_app_with_url_still_routes_to_windowuse() {
        // 反向用例:步骤中包含 URL 但桌面应用强信号(微信)出现 → 仍判 WindowUse,
        // 防止「在微信对话框里发送 https://example.com」任务被误判 WebUse。
        let spec = WorkFlowSpec {
            id: "wf-wx-url".into(),
            name: "微信发送链接".into(),
            steps: vec![
                "打开微信".into(),
                "在聊天窗口输入 https://example.com 这个链接".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::WebUse, // LLM 错判
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WindowUse));
    }

    #[test]
    fn web_with_shell_helper_still_routes_to_webuse() {
        // web 任务主体 + shell 辅助(常见模式):不开 GUI strict → WebUse。
        // 旧版双命中会判 SubAgent(WebUse 没 Bash),新版明确 WebUse 优先。
        let spec = WorkFlowSpec {
            id: "wf-sh-web".into(),
            name: "网页辅助".into(),
            steps: vec![
                "打开网址 https://example.com/".into(),
                "用 curl 抓取首页 HTML 备用".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WebUse));
    }

    #[test]
    fn browser_dom_gui_words_alone_stay_subagent() {
        // 极端纯描述:仅 web DOM 词、无 web/desktop 锚,LLM 没选明确 → 保持 None。
        // 这条保证不引入过激 Web 判定(纯描述步骤不应被关键词单方面改判)。
        let spec = WorkFlowSpec {
            id: "wf-ambiguous".into(),
            name: "模糊描述".into(),
            steps: vec!["在输入框填写 100".into(), "点击按钮提交".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent, // explicit 选 SubAgent
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), None);
    }

    // ★ 第 82 轮:URL 出现也应判 WebUse(覆盖关键词漏列的新域名/未被白名单收录的网址)
    #[test]
    fn url_in_steps_routes_to_webuse() {
        let spec = WorkFlowSpec {
            id: "wf-url".into(),
            name: "新站点".into(),
            steps: vec!["访问 https://new-site-2026.com/".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WebUse));
    }

    // ★ 第 82 轮:AI 对话类网站关键词也应判 WebUse
    #[test]
    fn ai_chat_keywords_route_to_webuse() {
        let spec = WorkFlowSpec {
            id: "wf-ai".into(),
            name: "DeepSeek 对话".into(),
            steps: vec![
                "打开 deepseek 官网".into(),
                "输入问题".into(),
                "找到发送按钮点击".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WebUse));
    }

    // ★ 第 82 轮:极简步骤("输入文本+点击提交")无任何关键词,加 URL 后才判 WebUse
    #[test]
    fn minimal_input_click_with_url_routes_to_webuse() {
        // 模拟 2026-09-17 16:25 Plan agent 输出的极简步骤("输入问题文本并提交")。
        // 旧版会被判 SubAgent(LLM 默认值);新版因 acceptance 含 URL 仍判 WebUse。
        let spec = WorkFlowSpec {
            id: "wf-min".into(),
            name: "极简输入".into(),
            steps: vec![
                "定位对话输入框".into(),
                "输入问题文本".into(),
                "点击提交".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            // acceptance 里有 URL,触发 URL 检测
            acceptance: vec!["对话窗口出现 AI 回复,内容与 https://wenxin.baidu.com/ 一致".into()],
            delegate_to: AgentRole::SubAgent,
            // 2026-09-17 第 82+ 轮 P0-1:默认无目标应用。
            target_app: None,
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WebUse));
    }
}
