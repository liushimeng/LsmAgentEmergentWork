//! WorkFlow 规格数据模型(2026-09-17 自 main_work.rs 拆分,方案见 tmpPlan/2026-09-17_01)。
//!
//! `WorkFlowSpec` / `BranchSpec` / `LoopSpec` / `WorkFlowPlan` 与配套宽松反序列化
//! (真实 LLM 高频把 branches/acceptance/delegate_to 写成别名或单字符串,严格 serde 会丢弃整份计划)。

use super::*;

///
/// 2026-09-10 第 25 轮(F1):steps/branches/loops/depends_on/acceptance/delegate_to
/// 全部走宽松反序列化 —— 真实 LLM 高频把 branches 写成字符串数组、acceptance 写成
/// 单字符串、delegate_to 写成 "SubAgent-Work" 等别名,严格 serde 校验会丢弃整份计划
/// (D06 实测 3/3 失败,2605 token 高质量编排被整体浪费)。详见
/// `tmpPlan/2026-09-10_08-D06测试与MainWork解析宽松化及重试反馈修复方案.md`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkFlowSpec {
    pub id: String,
    pub name: String,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub steps: Vec<String>,
    #[serde(default, deserialize_with = "lenient_branches")]
    pub branches: Vec<BranchSpec>,
    #[serde(default, deserialize_with = "lenient_loops")]
    pub loops: Vec<LoopSpec>,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub depends_on: Vec<String>,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub acceptance: Vec<String>,
    #[serde(
        default = "default_delegate_to",
        deserialize_with = "lenient_delegate_to"
    )]
    pub delegate_to: AgentRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSpec {
    pub condition: String,
    pub then: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopSpec {
    pub condition: String,
    pub over: String,
    #[serde(default)]
    pub max_iterations: Option<usize>,
}

/// Main-Work 输出(一个任务拆出多个 WorkFlow)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkFlowPlan {
    #[serde(default)]
    pub workflows: Vec<WorkFlowSpec>,
    #[serde(default)]
    pub summary: String,
    /// 解析失败走了兜底单 WorkFlow(F2):`run_medium` 据此跳过 QC-main
    /// (兜底计划带自证失败的 summary,送 QC 必然 fail+retryable,形成必败重试循环),
    /// 直接进入执行层,由每 WorkFlow 的 QC 把守真实产物。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub degraded: bool,
}

/// 宽松 `Vec<String>` 反序列化:对象数组(标准)/ 单字符串 → 数组。
fn lenient_strings<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Many(Vec<String>),
        One(String),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Many(v) => Ok(v),
        Raw::One(s) => Ok(vec![s]),
    }
}

/// 把 "条件: 动作" 形态的自然语言字符串切成 (condition, then)。
/// 分隔符取 `：` `:` `→` `->` 中**最早出现**的一个;切不开则整体归 condition。
pub(super) fn split_condition_then(text: &str) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    let candidates = ["：", ":", "→", "->"];
    let mut best: Option<(usize, usize, usize)> = None; // (char_idx, sep_chars, sep_bytes)
    for sep in candidates {
        if let Some(byte_idx) = text.find(sep) {
            let char_idx = text[..byte_idx].chars().count();
            if best.map_or(true, |(b, _, _)| char_idx < b) {
                best = Some((char_idx, sep.chars().count(), sep.len()));
            }
        }
    }
    match best {
        Some((idx, sep_chars, _)) => (
            chars[..idx].iter().collect::<String>().trim().to_string(),
            chars[idx + sep_chars..]
                .iter()
                .collect::<String>()
                .trim()
                .to_string(),
        ),
        None => (text.trim().to_string(), String::new()),
    }
}

/// 宽松 branches 反序列化(F1):对象数组 / 字符串数组 / 单字符串。
/// 字符串形态 `"若 X 失败: 改用 Y"` → `BranchSpec { condition: "若 X 失败", then: "改用 Y" }`。
fn lenient_branches<'de, D>(deserializer: D) -> std::result::Result<Vec<BranchSpec>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum RawBranch {
        Object(BranchSpec),
        Text(String),
    }
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum RawList {
        Many(Vec<RawBranch>),
        One(RawBranch),
    }
    fn convert(r: RawBranch) -> BranchSpec {
        match r {
            RawBranch::Object(b) => b,
            RawBranch::Text(s) => {
                let (condition, then) = split_condition_then(&s);
                BranchSpec { condition, then }
            }
        }
    }
    Ok(match RawList::deserialize(deserializer)? {
        RawList::Many(v) => v.into_iter().map(convert).collect(),
        RawList::One(r) => vec![convert(r)],
    })
}

/// 宽松 loops 反序列化(F1):对象数组 / 字符串数组 / 单字符串。
/// 字符串形态 `"对每个文件: 执行 X"` → `LoopSpec { condition: "对每个文件", over: "执行 X" }`。
fn lenient_loops<'de, D>(deserializer: D) -> std::result::Result<Vec<LoopSpec>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum RawLoop {
        Object(LoopSpec),
        Text(String),
    }
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum RawList {
        Many(Vec<RawLoop>),
        One(RawLoop),
    }
    fn convert(r: RawLoop) -> LoopSpec {
        match r {
            RawLoop::Object(l) => l,
            RawLoop::Text(s) => {
                let (condition, over) = split_condition_then(&s);
                LoopSpec {
                    condition,
                    over,
                    max_iterations: None,
                }
            }
        }
    }
    Ok(match RawList::deserialize(deserializer)? {
        RawList::Many(v) => v.into_iter().map(convert).collect(),
        RawList::One(r) => vec![convert(r)],
    })
}

/// 宽松 delegate_to 反序列化(F1):接受 "SubAgent-Work" / "main-work" 等别名;
/// 未知变体回退执行层 SubAgent,绝不因该字段丢整份计划。
fn default_delegate_to() -> AgentRole {
    AgentRole::SubAgent
}

fn lenient_delegate_to<'de, D>(deserializer: D) -> std::result::Result<AgentRole, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let norm = raw.trim().to_lowercase().replace(['-', '_', ' '], "");
    let role = match norm.as_str() {
        "subagent" | "subagentwork" | "work" | "执行层" => AgentRole::SubAgent,
        // 2026-09-18 第 84 轮:WindowUse Agent 已删除,旧别名兼容映射 SubAgent-Work
        // (macOS / Windows 上 SubAgent-Work 持 MCP_Window_Use 工具承担窗口操控)。
        "windowuse" | "windowuseagent" | "window" | "窗口" => AgentRole::SubAgent,
        "webuse" | "webuseagent" | "chromium" | "chromiumwebuse" | "browser" | "web" | "浏览器"
        | "网页" => AgentRole::WebUse,
        "main" | "mainwork" | "mainworkagent" => AgentRole::MainWork,
        "yolo" => AgentRole::Yolo,
        "plan" => AgentRole::Plan,
        "quality" | "qualitycheck" | "qc" => AgentRole::QualityCheck,
        "session" | "sessioncontext" => AgentRole::SessionContext,
        "compact" => AgentRole::Compact,
        _ => AgentRole::SubAgent,
    };
    Ok(role)
}

