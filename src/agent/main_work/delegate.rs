//! delegate_to 委派推断(2026-09-17 自 main_work.rs 拆分)。
//!
//! 基于步骤 / branches / 验收文本的关键词匹配,按「GUI 优先」语义纠正委派:
//! GUI 关键词 → WindowUse;网页关键词且无 GUI 词 → WebUse;仅 shell 词 → SubAgent。

use super::*;

// ========== delegate_to 推断(2026-09-16 第 54 轮补丁 B;第 67 轮语义修正) ==========
//
// 2026-09-16 第 54 轮:实测「打开微信发消息」任务 Main-Work 把含 osascript 步骤的工作流
// delegate_to=windowuse,但当时 WindowUse 没有 Bash 工具,导致 SubAgent 循环无法执行命令。
// 修复:解析 WorkFlowPlan 后,基于步骤文本 + branches 文本 + 验收文本的关键词匹配纠正。
//
// 2026-09-16 第 67 轮语义修正(微信 4.x 实测复盘):WindowUse 已带 Bash 白名单 +
// SendInput/OCR 原生能力,推断优先级改为「GUI 优先」:
//   - 命中 GUI 关键词(WindowList/WindowInspect/WindowAction/WindowOCR/点击按钮/
//     控件树/微信/鼠标/滚轮/通讯录 等)→ WindowUse(即便同时出现 osascript 等 shell 词);
//   - 仅命中 shell 关键词(osascript/xdotool/wmctrl/adb 等)→ SubAgent;
//   - 网页关键词且无 GUI 词 → WebUse;
//   - 都没命中 → 保持 Main-Work 显式选择(不强行覆盖)。
// 旧版把 "powershell/shell/uiautomation" 列在 shell 侧且双命中判 SubAgent,是本次
// 「微信任务被路由到 Bash 路线 → 判微信未安装」的直接根因。

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

pub(super) const WINDOW_USE_KEYWORDS: &[&str] = &[
    "windowlist",
    "windowinspect",
    "windowaction",
    "windowopen",
    "windowocr",
    "控件树",
    "枚举窗口",
    "ui automation",
    "axpress",
    "set_text",
    "控件路径",
    "无障碍",
    "invoke pattern",
    // 2026-09-16 第 67 轮:GUI 动作词与桌面应用词扩充(对齐用户「OS API 优先」原则)
    "鼠标",
    "滚轮",
    "通讯录",
    "聊天窗口",
    "输入框",
    "点击按钮",
    "桌面应用",
    "微信",
    "wechat",
    "weixin",
    "钉钉",
    "dingtalk",
    "飞书",
    "feishu",
];

/// 2026-09-16 第 61 轮:浏览器/网页操控关键词(Chromium-WebUse,第 11 角色)。
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
];

pub(super) fn text_contains_any_ci(text: &str, keywords: &[&str]) -> bool {
    let lower = text.to_lowercase();
    keywords.iter().any(|k| lower.contains(k))
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
pub fn infer_delegate_to(spec: &WorkFlowSpec) -> Option<AgentRole> {
    let text = gather_spec_text(spec);
    let shell_hit = text_contains_any_ci(&text, SUBAGENT_KEYWORDS);
    let gui_hit = text_contains_any_ci(&text, WINDOW_USE_KEYWORDS);
    let web_hit = text_contains_any_ci(&text, WEB_USE_KEYWORDS);
    // 2026-09-16 第 61 轮:网页/浏览器操控 → WebUse(shell 仍最优先,WebUse 无 Bash 工具;
    // web+gui 同命中按网页处理,网页语境也有"控件"表述)。
    if web_hit && !shell_hit && !gui_hit {
        return Some(AgentRole::WebUse);
    }
    match (shell_hit, gui_hit) {
        // shell 命令占主导 → 必须 SubAgent(纯 shell 流程)
        (true, false) => Some(AgentRole::SubAgent),
        // GUI 控件占主导 → WindowUse
        (false, true) => Some(AgentRole::WindowUse),
        // 2026-09-16 第 67 轮:双命中改判 WindowUse(原 SubAgent)。
        // 微信任务实测根因:steps 同时出现「点击通讯录」与「osascript/剪贴板」时
        // 被误判 SubAgent → 8 次 bash 进程检查全失败 →「微信客户端未安装」。
        // WindowUse 自带 Bash 白名单(桌面操控类命令可执行),GUI 优先不丢 shell 能力,
        // 且符合「优先 OS API(UIA/OCR/SendInput),Bash 其次」的产品原则。
        (true, true) => Some(AgentRole::WindowUse),
        // 都没命中 → 保持原样
        (false, false) => None,
    }
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
        };
        assert_eq!(infer_delegate_to(&spec), Some(AgentRole::SubAgent));
    }
}
