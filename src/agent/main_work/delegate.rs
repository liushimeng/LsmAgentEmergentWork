//! delegate_to 委派推断(2026-09-17 自 main_work.rs 拆分)。
//!
//! 基于步骤 / branches / 验收文本的关键词匹配备注与归一:第 89 轮(2026-09-18)
//! Chromium-WebUse Agent 删除后,执行器统一为 SubAgent-Work(桌面窗口操控走
//! MCP_Window_Use 工具,浏览器操控走 MCP_Web_Use 工具),本模块仅保留信号
//! 探测用于「LLM 写了历史遗留 delegate 值时的归一纠正」。

use super::*;

// ========== delegate_to 推断(2026-09-16 第 54 轮补丁 B;第 75 轮 WebUse 优先;
// ========== 第 84 轮收缩;第 89 轮归一为唯一执行器 SubAgent) ==========
//
// 2026-09-17 第 75 轮:把 GUI 关键词拆分为「桌面 GUI 强信号」(DESKTOP_GUI_STRICT_KEYWORDS)
// 与「Web DOM 通用词」(WEB_DOM_GUI_KEYWORDS)。
//
// 2026-09-18 第 84 轮:WindowUse Agent 删除。桌面 GUI 强信号不再路由到专项 Runner,
// 而是强制 SubAgent-Work(macOS / Windows 上持 MCP_Window_Use 工具,可完成
// 控件树 / 视觉坐标双路线窗口操控;Bash 全量可用,osascript 等降级路径天然兼容)。
//
// 2026-09-18 第 89 轮:Chromium-WebUse Agent 删除。浏览器操控同样降级为
// SubAgent-Work 的 MCP_Web_Use 工具,执行器唯一化 —— 推断结果只剩
// Some(SubAgent)(信号命中,归一纠正)或 None(无信号,保持原值)。

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
];

/// 2026-09-17 第 75 轮:桌面应用专属词 —— 命中基本可锁定桌面窗口操控任务。
///
/// 与 Web DOM 词汇无交集:微信/钉钉/飞书 等独立桌面应用,以及
/// `MCP_Window_Use/UIA/控件树` 等明确指代 OS 原生窗口/控件树的 API。
/// 命中后优先归一 SubAgent(防止把「https://example.com 这个链接粘到微信对话框」误判为浏览器操控流程)。
/// 2026-09-18 第 84 轮:命中改判 SubAgent-Work(原 WindowUse Agent 已删除)。
pub(super) const DESKTOP_GUI_STRICT_KEYWORDS: &[&str] = &[
    // 桌面 OS 窗口 API(UIA/AX/MCP_Window_Use)
    "mcp_window_use",
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
/// 但**不**进入 `DESKTOP_GUI_STRICT_KEYWORDS`,不再压制 web_hit。
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


/// 2026-09-16 第 61 轮:浏览器/网页操控关键词(原 Chromium-WebUse 信号)。
/// ★ 第 82 轮:补充 AI 对话类网站强信号 + 中文提交按钮 selector 名。
/// 第 89 轮:信号保留但仅用于「确认这是浏览器类流程 → 归一 SubAgent-Work
/// (持 MCP_Web_Use 工具)」,不再路由到专项 Runner。
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
/// 2026-09-18 第 89 轮(执行器唯一化后):
///   1. 桌面 GUI 强信号(微信/钉钉/飞书/MCP_Window_Use/UIA/控件树 等) → SubAgent
///      (SubAgent-Work 在 macOS / Windows 持 MCP_Window_Use 工具,Bash 全量可用)
///   2. web 命中(含 URL)→ SubAgent(SubAgent-Work 持 MCP_Web_Use 工具,
///      浏览器操控与 shell 辅助可同单元完成)
///   3. 仅 shell 词 → SubAgent
///   4. 都没命中 → None(保持 Main-Work 显式选择)
///
/// 三类信号命中都归一 SubAgent;保留分层结构是为了日志可读性(哪类信号触发纠正)
/// 与未来新增执行器时恢复差异化路由的空间。
/// 不用「输入框/按钮/鼠标/滚轮」等 Web DOM 词作为 GUI 强证据 —— 这些词在
/// HTML 页面里同样常见(第 75 轮 P0 修复)。
pub fn infer_delegate_to(spec: &WorkFlowSpec) -> Option<AgentRole> {
    let text = gather_spec_text(spec);
    let shell_hit = text_contains_any_ci(&text, SUBAGENT_KEYWORDS);
    let desktop_gui_hit = text_contains_any_ci(&text, DESKTOP_GUI_STRICT_KEYWORDS);
    let web_hit = text_contains_any_ci(&text, WEB_USE_KEYWORDS);
    // ★ 第 82 轮:URL 出现也作为 web 强信号(关键词表可能漏列新网站)
    let url_hit = text_contains_url(&text);

    // 规则 1:桌面 GUI 强信号 → SubAgent(MCP_Window_Use 承担窗口操控)。
    if desktop_gui_hit {
        return Some(AgentRole::SubAgent);
    }
    // 规则 2:web 命中(含 URL)→ SubAgent(MCP_Web_Use 承担浏览器操控,
    // LLM 若写了历史遗留 delegate 值在此归一)。
    if web_hit || url_hit {
        return Some(AgentRole::SubAgent);
    }
    // 规则 3:仅 shell 词 → SubAgent。
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
        // 纯 shell 流程(不含 GUI 词)判 SubAgent。
        let spec = WorkFlowSpec {
            id: "wf-1".into(),
            name: "音量查询".into(),
            steps: vec!["执行 osascript -e 'output volume of (get volume settings)'".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::MainWork, // 显式选错
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }

    #[test]
    fn gui_steps_route_to_subagent() {
        // 2026-09-18 第 84 轮:桌面 GUI 强信号 → SubAgent-Work(MCP_Window_Use 工具)。
        let spec = WorkFlowSpec {
            id: "wf-2".into(),
            name: "枚举窗口".into(),
            steps: vec![
                "调用 MCP_Window_Use 枚举窗口".into(),
                "遍历控件树找目标按钮".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec!["找到目标窗口 id".into()],
            delegate_to: AgentRole::MainWork, // 显式选错
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
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
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
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
                delegate_to: AgentRole::SubAgent,
                // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
                max_iterations: None, original_prompt: None,
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
    fn both_keywords_pick_subagent() {
        // 混合型(osascript + 桌面 GUI 强信号)→ SubAgent-Work:
        // MCP_Window_Use 承担控件树/视觉双路线,Bash 全量可用不丢 shell 能力。
        let spec = WorkFlowSpec {
            id: "wf-mix".into(),
            name: "混合".into(),
            steps: vec![
                "osascript -e 'tell application \"WeChat\" to activate'".into(),
                "MCP_Window_Use 遍历控件树检视".into(),
            ],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to: AgentRole::MainWork, // 显式选错
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }

    #[test]
    fn wechat_desktop_steps_route_to_subagent() {
        // 桌面应用强信号(微信)命中 → SubAgent-Work(MCP_Window_Use 承担窗口操控)。
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
            delegate_to: AgentRole::MainWork, // 显式选错
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
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
            delegate_to: AgentRole::MainWork, // 显式选错
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }

    // ========== 2026-09-17 第 75 轮回归用例(第 89 轮起断言统一 SubAgent) ==========

    #[test]
    fn wenxin_url_with_input_box_routes_to_webuse() {
        // 第 75 轮 P0 修复核心回归:用户真实失败任务。
        // steps 同时含 web 词(打开网址/wenxin.baidu.com)和 web DOM 词(输入框/按钮),
        // 第 89 轮:浏览器类流程统一 SubAgent-Work(MCP_Web_Use 工具)。
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
            delegate_to: AgentRole::SubAgent, // LLM 错判
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }

    #[test]
    fn pure_browser_open_url_routes_to_webuse() {
        // 通用 web 场景:无 strict GUI 词,只有 web 词 → SubAgent(MCP_Web_Use)
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
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }

    #[test]
    fn wechat_desktop_app_with_url_still_routes_to_subagent() {
        // 反向用例:步骤中包含 URL 但桌面应用强信号(微信)出现 → 仍判 SubAgent-Work,
        // 防止「在微信对话框里发送 https://example.com」任务被误判为浏览器流程。
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
            delegate_to: AgentRole::MainWork, // LLM 错判
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }

    #[test]
    fn web_with_shell_helper_still_routes_to_webuse() {
        // web 任务主体 + shell 辅助(常见模式):不开 GUI strict → SubAgent(MCP_Web_Use + Bash 同单元可用)。
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
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
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
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
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
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
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
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
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
        
            // 2026-09-19 第 91 轮 P0-6/P0-8:新字段兜底默认值
            max_iterations: None, original_prompt: None,
};
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }
}
