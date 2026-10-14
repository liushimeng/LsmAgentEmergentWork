//! 规划缺口信号识别(再规划 / 更新规划的机械路由通道)。
//!
//! 执行层(SubAgent-Work)按方案执行时,会遇到规划阶段没探索到的现场:
//! 登录墙 / 验证码 / 滑块、复杂表单填报、访问被拒、页面结构与规划假设不符等。
//! 这类失败的特征是「方案本身没覆盖现场」——同档位盲重试只会撞同一堵墙,
//! 需要把失败现场回灌 Plan 做**再规划 / 更新规划**。
//!
//! 本模块只做**机械信号识别**(关键词 + 执行层失败信号),与
//! `SameCauseBreaker` 归一化、`target_drift` 信号同性质:识别结果是
//! 路由提示,是否再规划、再规划成什么样,判断权始终在 Plan / Main-Work 模型。
//! 误触发的代价是「多问 Plan 一次」,漏触发的代价是「在死路上烧满重试预算」,
//! 取舍明确偏向前者。
//!
//! 设计见 `docs/PlanAgent再规划与更新规划/01-设计与解决方案.md` §3.1。

/// 规划缺口类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplanTrigger {
    /// 登录墙:登录 / 扫码 / 验证码 / 滑块 / 短信验证等凭证类阻碍。
    LoginWall,
    /// 表单流程:填报 / 提交申请 / 上传材料等多步表单。
    FormFlow,
    /// 访问受阻:403 / 拒绝访问 / 权限不足 / 风控 / WAF。
    AccessBlocked,
    /// 流程未知:页面结构与规划假设不符 / 找不到入口 / 未探索到。
    UnknownFlow,
    /// 人工验证:request_human / human_assist / manual_verify 等人工介入信号。
    HumanVerify,
}

impl ReplanTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReplanTrigger::LoginWall => "login_wall",
            ReplanTrigger::FormFlow => "form_flow",
            ReplanTrigger::AccessBlocked => "access_blocked",
            ReplanTrigger::UnknownFlow => "unknown_flow",
            ReplanTrigger::HumanVerify => "human_verify",
        }
    }

    /// 人类可读的类别标签(进度行 / 日志 / 再规划 prompt 用)。
    pub fn label(&self) -> &'static str {
        match self {
            ReplanTrigger::LoginWall => "登录墙/凭证验证",
            ReplanTrigger::FormFlow => "表单/填报流程",
            ReplanTrigger::AccessBlocked => "访问受阻/风控",
            ReplanTrigger::UnknownFlow => "现场与规划假设不符",
            ReplanTrigger::HumanVerify => "需人工验证/介入",
        }
    }
}

/// 识别出的规划缺口信号。
#[derive(Debug, Clone)]
pub struct ReplanSignal {
    pub trigger: ReplanTrigger,
    /// 命中证据摘录(截 120 字符),供日志与再规划 prompt 使用。
    pub evidence: String,
}

/// 各类别的关键词表(中英双语小写匹配)。顺序即优先级:排前的类别先判,
/// 同一文本命中多类时取首个(登录墙优先于表单——登录通常是表单的前置)。
const TRIGGER_KEYWORDS: &[(ReplanTrigger, &[&str])] = &[
    (
        ReplanTrigger::LoginWall,
        &[
            "登录墙", "需要登录", "请先登录", "扫码", "验证码", "滑块", "短信验证",
            "账号密码", "登陆", "captcha", "slider", "login required", "扫码登录",
            "二维码登录", "身份验证", "two-factor", "2fa",
        ],
    ),
    (
        ReplanTrigger::HumanVerify,
        &[
            "request_human", "human_assist", "manual_verify", "需要人工", "人工核验",
            "人工验证", "人工介入", "human verification",
        ],
    ),
    (
        ReplanTrigger::AccessBlocked,
        &[
            "403", "拒绝访问", "权限不足", "未授权", "风控", "访问受限", "waf",
            "forbidden", "access denied", "unauthorized", "http 400", "异常流量",
        ],
    ),
    (
        ReplanTrigger::FormFlow,
        &[
            "表单", "填报", "提交申请", "上传材料", "报名表", "申请表", "form",
            "字段未知", "必填字段",
        ],
    ),
    (
        ReplanTrigger::UnknownFlow,
        &[
            "页面结构与预期不符", "未探索到", "找不到入口", "无此元素", "流程未知",
            "与规划假设不符", "结构与规划", "没有该", "不存在此", "入口未知",
        ],
    ),
];

/// 证据摘录上限(字符)。
const EVIDENCE_CLIP: usize = 120;

/// 从文本中截取包含关键词的最早一段作为证据摘录。
fn extract_evidence(text: &str, keyword: &str) -> String {
    let Some(idx) = text.find(keyword) else {
        return text.chars().take(EVIDENCE_CLIP).collect();
    };
    // 以命中点为中心取窗口,保证证据可读。
    let chars: Vec<char> = text.chars().collect();
    let char_idx = text[..idx].chars().count();
    let start = char_idx.saturating_sub(40);
    let end = (char_idx + keyword.chars().count() + 60).min(chars.len());
    let win: String = chars[start..end.min(start + EVIDENCE_CLIP)].iter().collect();
    if start > 0 {
        format!("…{win}")
    } else {
        win
    }
}

/// 机械识别规划缺口信号。
///
/// 扫描源:`reason`(QC issues / 失败原因)与 `suggestion`(QC 建议)拼接文本 +
/// `failure_signals`(执行层机械信号,如 blocker 类)。任一命中即返回;
/// 未命中返回 None(走常规重试 / Yolo 回流路径)。
pub fn detect_replan_signal(
    reason: &str,
    suggestion: &str,
    failure_signals: &[String],
) -> Option<ReplanSignal> {
    let haystack = format!("{reason}\n{suggestion}");
    let low = haystack.to_lowercase();
    let low_signals: Vec<String> = failure_signals.iter().map(|s| s.to_lowercase()).collect();

    for (trigger, keywords) in TRIGGER_KEYWORDS {
        for kw in *keywords {
            // 主文本命中
            if low.find(kw).is_some() {
                return Some(ReplanSignal {
                    trigger: *trigger,
                    evidence: extract_evidence(&haystack, kw),
                });
            }
            // 执行层失败信号命中(blocker_alert / manual_verify 等结构化信号)
            if low_signals.iter().any(|s| s.contains(kw)) {
                return Some(ReplanSignal {
                    trigger: *trigger,
                    evidence: format!("failure_signal 命中「{kw}」"),
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 登录墙关键词命中() {
        let s = detect_replan_signal(
            "wf-2: 页面弹出登录框,需要登录后才能查看列表",
            "建议补充登录前置步骤",
            &[],
        )
        .expect("应命中登录墙");
        assert_eq!(s.trigger, ReplanTrigger::LoginWall);
        assert!(!s.evidence.is_empty());
    }

    #[test]
    fn 英文验证码命中登录墙() {
        let s = detect_replan_signal("encountered captcha challenge", "", &[])
            .expect("captcha 应命中登录墙");
        assert_eq!(s.trigger, ReplanTrigger::LoginWall);
    }

    #[test]
    fn 表单填报命中() {
        let s = detect_replan_signal("wf-3: 报名表单包含未知必填字段,无法提交", "", &[])
            .expect("表单应命中");
        assert_eq!(s.trigger, ReplanTrigger::FormFlow);
    }

    #[test]
    fn 访问受阻命中() {
        let s = detect_replan_signal("目标站返回 403 拒绝访问,疑似风控", "", &[])
            .expect("403 应命中访问受阻");
        assert_eq!(s.trigger, ReplanTrigger::AccessBlocked);
    }

    #[test]
    fn 执行层信号命中人工验证() {
        let s = detect_replan_signal(
            "SubAgent 执行失败(wf=wf-1): 工具链中断",
            "",
            &["blocker_alert:manual_verify".to_string()],
        )
        .expect("manual_verify 信号应命中人工验证");
        assert_eq!(s.trigger, ReplanTrigger::HumanVerify);
    }

    #[test]
    fn 普通失败不命中() {
        // 常规执行错误(文件不存在 / 编译失败)不应触发再规划。
        assert!(
            detect_replan_signal("wf-1: 文件不存在: /tmp/x.txt", "检查路径后重试", &[])
                .is_none()
        );
        assert!(
            detect_replan_signal("cargo build 失败: unresolved import", "", &[]).is_none()
        );
    }

    #[test]
    fn 登录优先于表单() {
        // 同时含登录与表单关键词时,登录墙先判(登录通常是表单的前置)。
        let s = detect_replan_signal("需要登录,登录后才是填报表单", "", &[])
            .expect("应命中");
        assert_eq!(s.trigger, ReplanTrigger::LoginWall);
    }

    #[test]
    fn 证据摘录有界() {
        let long = "x".repeat(500) + "需要登录";
        let s = detect_replan_signal(&long, "", &[]).expect("应命中");
        assert!(s.evidence.chars().count() <= EVIDENCE_CLIP + 2, "证据应截断: {}", s.evidence);
    }
}
