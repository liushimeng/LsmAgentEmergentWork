//! 用户原始提示词中的显式任务前置约束。
//!
//! 与目标锚点分工:这里只识别需要跨 Plan / Main-Work / SubAgent 保持一致的
//! 顺序语义,不抽取目标站点,也不做工具级政策拦截。

/// 用户原始提示词指定的浏览器内核;双指定/未指定交给模型选择。
pub fn browser_mandate(prompt: &str) -> Option<&'static str> {
    let lower = prompt.to_ascii_lowercase();
    let chrome = ["chrome", "chromium", "谷歌浏览器", "谷歌内核"]
        .iter()
        .any(|k| lower.contains(k));
    let lsm = ["lsmagentbrowser", "lsm 浏览器", "自研内核", "反反爬", "反-反爬"]
        .iter()
        .any(|k| lower.contains(k));
    match (chrome, lsm) {
        (true, false) => Some("chrome"),
        (false, true) => Some("lsm"),
        _ => None,
    }
}

/// Plan/Main-Work 共用的工具指定约束,防止规划层另开无登录态内核。
pub fn browser_constraint(prompt: &str) -> Option<&'static str> {
    match browser_mandate(prompt) {
        Some("chrome") => Some(
            "【用户指定浏览器(原始输入硬约束)】本任务的规划探查与全部网页操作必须使用\
             MCP_Web_Chrome_Use;不要改用 MCP_Web_LsmAgentBrowser_Use(登录态不共享)。",
        ),
        Some("lsm") => Some(
            "【用户指定浏览器(原始输入硬约束)】本任务的规划探查与全部网页操作必须使用\
             MCP_Web_LsmAgentBrowser_Use;不要改用 MCP_Web_Chrome_Use。",
        ),
        _ => None,
    }
}

/// 用户是否明确要求「先登录,再做后续工作」。
///
/// 「登录后」必须接续明确的后续动作;「登录后的网站有更多功能」这类能力说明
/// 不是顺序约束,不能据此阻断搜索或路线规划。
pub fn requires_login_first(prompt: &str) -> bool {
    let compact: String = prompt.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.contains("先登录") {
        return true;
    }
    const ACTIONS: &[&str] = &[
        "开始", "随后", "然后", "继续", "执行", "搜索", "采集", "遍历", "查看", "规划",
        "操作", "访问", "获取", "保存", "截图", "生成", "点击", "输入", "提交", "下载",
        "使用", "打开", "进入",
    ];
    for marker in ["登录完成后", "登录成功后", "登录之后", "登录后"] {
        for (start, _) in compact.match_indices(marker) {
            let after = &compact[start + marker.len()..];
            let after = after.trim_start_matches(|c: char| {
                matches!(c, '，' | ',' | '。' | '、' | '；' | ';' | '：' | ':' | '！' | '!')
            });
            let after = after
                .strip_prefix('再')
                .or_else(|| after.strip_prefix('才'))
                .unwrap_or(after);
            if ACTIONS.iter().any(|action| after.starts_with(action)) {
                return true;
            }
        }
    }
    false
}

/// Plan 专用约束:规划层识别并编排前置,但不能越权执行真实登录。
pub const PLAN_LOGIN_FIRST_CONSTRAINT: &str =
    "【用户显式登录前置(原始输入硬约束)】用户要求先完成登录再执行后续工作。\
     Plan 只规划、不实际登录,也不要调用 request_human:打开目标页确认未登录后即停止搜索、\
     遍历和评论/API探测,把「人工登录 + 页面验证登录态」写成首个 WorkFlow,其余网页单元\
     全部依赖它;实际扫码/账密登录由执行层通过 request_human 完成。";

/// 跨角色共享的登录前置原文,避免各层分别改写后语义漂移。
pub const LOGIN_FIRST_CONSTRAINT: &str =
    "【用户显式登录前置(原始输入硬约束)】用户要求先完成登录再执行后续工作。\
     若当前页面未登录,必须先用当前浏览器 control(request_human, reason=\"login\") 交人工,\
     并在人工完成后从页面验证登录态;验证前不得搜索、遍历、访问评论/API或采集数据,\
     也不得把登录仅记为风险延后。已验证登录则继续,不重复打扰人工。";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 实测京东提示词命中登录前置() {
        let prompt = "使用 Chrome 打开京东,必须先登录,需要人工、用户介入。之后搜索商品并查看评论";
        assert!(requires_login_first(prompt));
        assert!(requires_login_first("先人工扫码登录,登录后再采集"));
        assert!(requires_login_first("登录成功后开始遍历"));
    }

    #[test]
    fn 普通登录描述不误判为顺序约束() {
        for prompt in [
            "找到登录按钮",
            "页面支持账号登录",
            "检查登录态",
            "登录后的网站，才有一些复杂功能。",
            "登录之后可使用更多功能",
            "登录成功后会显示用户昵称",
            "",
        ] {
            assert!(!requires_login_first(prompt), "不应命中: {prompt}");
        }
    }

    #[test]
    fn 指定Chrome约束同时覆盖规划与执行() {
        let prompt = "使用 Chrome 打开京东,必须先登录再搜索";
        assert_eq!(browser_mandate(prompt), Some("chrome"));
        let constraint = browser_constraint(prompt).unwrap();
        assert!(constraint.contains("规划探查"));
        assert!(constraint.contains("MCP_Web_Chrome_Use"));
        assert!(constraint.contains("登录态不共享"));
        assert!(browser_constraint("分别用 Chrome 与 LsmAgentBrowser 对比").is_none());
    }
}
