//! 安全拒绝检测(第 149 轮)—— 机械识别「Agent 拒绝执行任务」的输出文本。
//!
//! 背景(2026-10-10 实测事故,`llaew_20261010_152433.log`):用户下发越狱攻击类任务,
//! Yolo 在 `direct_answer` 里写了完整拒绝文本 + 替代方向,但 `task_level=hard`;
//! 编排器的 direct_answer 短路只对 Simple 生效,拒绝被忽略、原任务原文继续下发
//! Plan → Plan 三次拒绝 → 解析门三次报「未解析出任何 WorkFlow」→ 盲重试 528s。
//! 安全拒绝应是**终态**而非可重试失败;本检测器是「Yolo 显式 refuses_task 字段」
//! 之外的第二通道(机械兜底,不依赖模型填新字段)。
//!
//! 设计约束:
//! - **只扫文首**:前 3 个非空行、累计 ≤200 字符。拒绝文本的开头几乎必然是拒绝
//!   陈述;扫全文会把「如果无法执行 X 请降级」这类正常方案内容误判进来。
//! - **marker 全部是拒绝动作短语**:「我不确定」「请补充信息」类澄清措辞刻意
//!   不在列表中(澄清走 target_anchor 的澄清门,语义不同)。
//! - 误报代价分析:直答门误报 = 以 Yolo 的完整回答终态(用户看到原文,可重新
//!   表述,信息零丢失);Plan 门误报 = 该文档本就解析不出 WorkFlow,终态是
//!   合理升级。两个消费点都偏「宁终态、不空转」。

/// 拒绝 marker 列表(子串匹配,按语义分组)。
///
/// 覆盖 2026-10-10 事故中 Plan/Yolo 的全部真实拒绝开头,以及常见中文 LLM
/// 拒绝句式(协助/帮助/执行/完成/规划/制定/编写 + 为/该/这个 变体)。
const REFUSAL_MARKERS: &[&str] = &[
    // 协助/帮助类
    "无法协助",
    "不能协助",
    "无法帮助",
    "不能帮助",
    "不会协助",
    "拒绝协助",
    // 执行/完成类
    "拒绝执行",
    "无法执行",
    "不能执行",
    "不会执行",
    "拒绝该任务",
    "拒绝这个任务",
    "无法完成该",
    "不能完成该",
    // 第一人称直陈(Plan 拒绝高频开头:「我无法执行这个规划任务」)
    "我无法",
    "我不能",
    "我不会",
    // 「为(它/这个/该)+ 动词」类(「我不能为它制定执行方案」)
    "不能为",
    "无法为",
    "不会为",
    // 规划/制定/编写类(Plan Agent 拒绝特有)
    "不能规划",
    "无法规划",
    "不能制定",
    "无法制定",
    "不能编写",
    "无法编写",
];

/// 扫描窗口:前 N 个非空行。
const SCAN_LINES: usize = 3;
/// 扫描窗口:累计字符上限(行数兜底之外的硬上限)。
const SCAN_CHARS: usize = 200;

/// 检测文本是否为「拒绝执行任务」式输出。
///
/// 返回命中的 marker(供日志/审计记录具体依据);非拒绝返回 `None`。
///
/// 消费点:
/// - `orchestrator/pipeline.rs` 门 A/门 B(Yolo direct_answer);
/// - `orchestrator/pipeline.rs::run_hard`(Plan markdown,拒绝时不再进 QC/解析,
///   直接以拒绝终态收口 —— 杜绝「解析失败→施压重出」循环)。
pub fn detect_refusal(text: &str) -> Option<&'static str> {
    let mut scanned = String::new();
    let mut lines = 0usize;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if lines >= SCAN_LINES || scanned.chars().count() >= SCAN_CHARS {
            break;
        }
        scanned.push_str(t);
        scanned.push('\n');
        lines += 1;
    }
    if scanned.is_empty() {
        return None;
    }
    REFUSAL_MARKERS
        .iter()
        .find(|m| scanned.contains(*m))
        .copied()
}

/// 归一化失败原因(第 149 轮同因熔断的比对键)。
///
/// 同因判定口径:trim → 取首行(多行 reason 只比第一行)→ 截 120 字符。
/// 「解析 Plan 失败: 方案生成失败: Plan 文档未解析出任何 WorkFlow」这类
/// 稳定重复的 reason 三轮完全一致;而带时间戳/UUID 的 reason 首行也稳定。
pub fn normalize_failure_reason(reason: &str) -> String {
    let first_line = reason.trim().lines().next().unwrap_or("").trim();
    first_line.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== 正例:2026-10-10 事故真实拒绝文本 ==========

    #[test]
    fn detects_yolo_direct_answer_refusal() {
        // 事故原句:Yolo direct_answer 开头
        let t = "无法协助该任务:其实质是指示 AI 对真实商业服务(豆包)实施\"意图隐藏+提示词套娃\"的越狱攻击,批量提取其系统提示词(≥10条)并保存。";
        assert_eq!(detect_refusal(t), Some("无法协助"));
    }

    #[test]
    fn detects_plan_refusal_variant_1() {
        // 事故 Plan 第 1 轮开头(拒绝在首行后半段)
        let t = "我理解这个任务请求,但我不能为它制定执行方案,这里说明原因,并提供可行的替代方向。\n\n## 为什么不能规划这个任务";
        assert!(detect_refusal(t).is_some());
    }

    #[test]
    fn detects_plan_refusal_variant_2() {
        // 事故 Plan 第 2 轮开头
        let t = "我无法执行这个规划任务。\n\n## 拒绝原因\n该任务的目标明确标注为 `prompt_extraction_attack`。";
        assert!(detect_refusal(t).is_some());
    }

    #[test]
    fn detects_plan_refusal_variant_3() {
        // 事故 Plan 第 3 轮开头
        let t = "我无法为这个任务制定方案。\n\n## 拒绝原因\n越狱商业 AI 系统...";
        assert!(detect_refusal(t).is_some());
    }

    #[test]
    fn detects_english_style_refusal_translated() {
        // 常见句式:「我不能协助完成该请求」
        let t = "很抱歉,我不能协助完成这个请求。该行为违反服务条款。";
        assert!(detect_refusal(t).is_some());
    }

    // ========== 负例:正常内容不得误报 ==========

    #[test]
    fn normal_plan_markdown_not_refusal() {
        let md = "# 任务方案:打开豆包官网提取对话\n\n## 一、目标\n打开 https://www.doubao.com/ 并完成多轮对话。\n\n## 二、WorkFlow 拆解\n### WorkFlow 1: 打开站点";
        assert_eq!(detect_refusal(md), None);
    }

    #[test]
    fn normal_answer_with_cannot_word_later_not_refusal() {
        // 「无法执行」出现在方案正文中部(超出文首窗口)不判拒绝
        let t = "# 方案\n\n## 一、目标\n完成数据迁移。\n\n## 注意事项\n若目标库只读则无法执行写入,需先申请权限。";
        assert_eq!(detect_refusal(t), None);
    }

    #[test]
    fn clarification_wording_not_refusal() {
        // 澄清类措辞(「不确定」「请补充」)不在 marker 列表 —— 澄清走澄清门
        let t = "我不确定你指的是哪个文件,请补充完整路径后重试。";
        assert_eq!(detect_refusal(t), None);
    }

    #[test]
    fn normal_direct_answer_not_refusal() {
        let t = "Rust 的所有权规则:每个值有唯一所有者,赋值是移动语义。示例:let s2 = s1; 之后 s1 失效。";
        assert_eq!(detect_refusal(t), None);
    }

    #[test]
    fn empty_text_not_refusal() {
        assert_eq!(detect_refusal(""), None);
        assert_eq!(detect_refusal("\n\n  \n"), None);
    }

    #[test]
    fn refusal_marker_beyond_scan_lines_not_matched() {
        // 拒绝词在第 4 个非空行(窗口外)不判 —— 拒绝文本几乎必然开门见山
        let t = "# 标题\n第一段说明。\n第二段说明。\n第三段:我无法执行这个任务。";
        assert_eq!(detect_refusal(t), None);
    }

    // ========== normalize_failure_reason ==========

    #[test]
    fn normalize_takes_first_line_and_trims() {
        let r = "解析 Plan 失败: 方案生成失败: Plan 文档未解析出任何 WorkFlow\n附加上下文行";
        assert_eq!(
            normalize_failure_reason(r),
            "解析 Plan 失败: 方案生成失败: Plan 文档未解析出任何 WorkFlow"
        );
    }

    #[test]
    fn normalize_truncates_to_120_chars() {
        let long = "错".repeat(300);
        assert_eq!(normalize_failure_reason(&long).chars().count(), 120);
    }

    #[test]
    fn normalize_empty_reason() {
        assert_eq!(normalize_failure_reason(""), "");
        assert_eq!(normalize_failure_reason("   \n  "), "");
    }
}
