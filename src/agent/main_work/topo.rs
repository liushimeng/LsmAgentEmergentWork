//! WorkFlow 拓扑排序与依赖治理(2026-09-17 自 main_work.rs 拆分)。
//!
//! Kahn 分层(同层可并行)/ 扁平化排序 / id 去重 / depends_on 归一化。

use super::*;

/// 拓扑排序(返回执行顺序)。
///
/// 基于 `topo_layers` 分层结果扁平化(单一事实源);检测循环依赖与未知依赖。
pub fn topo_sort(workflows: &[WorkFlowSpec]) -> Result<Vec<WorkFlowSpec>> {
    Ok(topo_layers(workflows)?.into_iter().flatten().collect())
}

/// 拓扑分层(Kahn 分层):同层 WorkFlow 互相无依赖,可并行执行;跨层严格串行。
///
/// - 第 0 层 = 入度 0 节点;每消费一层,其后继入度 -1,归零者入下一层。
/// - **确定性**:层内顺序 = 原 `workflows` 数组顺序(不依赖 HashMap 遍历序)。
/// - 未知依赖 / 环:与 `topo_sort` 一致,报 `AgentError::WorkflowTopology`。
pub fn topo_layers(workflows: &[WorkFlowSpec]) -> Result<Vec<Vec<WorkFlowSpec>>> {
    let mut by_id: std::collections::HashMap<&str, &WorkFlowSpec> =
        std::collections::HashMap::new();
    for w in workflows {
        by_id.insert(w.id.as_str(), w);
    }
    // 入度 = 它依赖的 wf 数;先校验所有依赖已知
    let mut in_degree: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for w in workflows {
        for dep in &w.depends_on {
            if !by_id.contains_key(dep.as_str()) {
                return Err(AgentError::WorkflowTopology(format!(
                    "wf={} 依赖未知 wf={}",
                    w.id, dep
                )));
            }
        }
        in_degree.insert(w.id.as_str(), w.depends_on.len());
    }

    let mut layers: Vec<Vec<WorkFlowSpec>> = Vec::new();
    let mut placed: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut total = 0usize;
    loop {
        // 本轮可放置:入度 0 且尚未放置;按原数组顺序扫描保证确定性
        let layer: Vec<&WorkFlowSpec> = workflows
            .iter()
            .filter(|w| {
                !placed.contains(w.id.as_str())
                    && in_degree.get(w.id.as_str()).copied().unwrap_or(0) == 0
            })
            .collect();
        if layer.is_empty() {
            break;
        }
        for w in &layer {
            placed.insert(w.id.as_str());
        }
        total += layer.len();
        // 消费本层:递减所有依赖本层节点的后继入度
        let layer_ids: std::collections::HashSet<&str> =
            layer.iter().map(|w| w.id.as_str()).collect();
        for w in workflows {
            if placed.contains(w.id.as_str()) {
                continue;
            }
            let dec = w
                .depends_on
                .iter()
                .filter(|d| layer_ids.contains(d.as_str()))
                .count();
            if dec > 0 {
                let entry = in_degree.entry(w.id.as_str()).or_insert(0);
                *entry = entry.saturating_sub(dec);
            }
        }
        layers.push(layer.into_iter().cloned().collect());
    }
    if total != workflows.len() {
        return Err(AgentError::WorkflowTopology(
            "检测到循环依赖,无法拓扑排序".into(),
        ));
    }
    Ok(layers)
}

/// I4(2026-09-14 第 51 轮):workflows 数组按 id 去重(保留首次出现)。
/// 真实网关实测 Main-Work 偶发把同一批 wf 输出两遍(wf-1..wf-5 各 2 条共 10 条,
/// ak05),id 是依赖解析主键,重复触发 QC 必拒 + 依赖歧义;去重属无损修复。
pub fn dedup_workflow_ids(plan: &mut WorkFlowPlan) {
    let mut seen = std::collections::HashSet::new();
    let before = plan.workflows.len();
    plan.workflows.retain(|w| seen.insert(w.id.clone()));
    if plan.workflows.len() != before {
        tracing::warn!(
            "WorkFlow 数组存在重复 id,已按 id 去重: {} → {} 条",
            before,
            plan.workflows.len()
        );
    }
}

/// 归一化单条 `depends_on` 条目,提取其中引用的 wf id。
///
/// LLM 生成的依赖声明常附带「变量透传」注释,例如:
///   - `wf-1（需要 FIRST_CHROME_WINDOW_ID）`(Plan 文档路径 / JSON 路径均有)
///   - `wf-2 (needs data)` / `wf-3：控件树数据`
/// 这些注释让 `topo_layers` 的精确匹配失败,报「依赖未知 wf=wf-1（需要...）」,整份计划
/// 直接被丢弃,上游 Yolo 反复重试空转。修复:剥离注释,只保留开头的 wf id 记号。
/// 返回 None 表示条目为空或剥离后无有效 id(调用方应丢弃该条目)。
pub(super) fn normalize_dep_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 取第一个「中文全角括号 / 半角括号 / 中文冒号 / 空白」之前的 token 作为 id;
    // 同时去掉首尾可能残留的引号 / 方括号。
    let end = trimmed
        .find(|c: char| matches!(c, '（' | '(' | '：' | ':' | ' ' | '\t'))
        .unwrap_or(trimmed.len());
    let token =
        trimmed[..end].trim_matches(|c: char| matches!(c, '"' | '\'' | '[' | ']' | ',' | '、'));
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

/// 归一化整份计划的 `depends_on` 列表:剥离注释、去空、去自环、去重。
/// 在两个解析入口(Main-Work JSON / Plan 文档)都调用,确保 `topo_layers` 拿到干净的 id 集合。
pub fn sanitize_depends_on(plan: &mut WorkFlowPlan) {
    let ids: std::collections::HashSet<String> =
        plan.workflows.iter().map(|w| w.id.clone()).collect();
    let mut changed = false;
    for w in plan.workflows.iter_mut() {
        let original = w.depends_on.clone();
        let mut cleaned: Vec<String> = original
            .iter()
            .filter_map(|d| normalize_dep_id(d))
            .filter(|d| {
                if d == &w.id {
                    // 自环无意义,丢弃
                    return false;
                }
                if !ids.contains(d) {
                    // 注释剥离后仍不是已知 wf id → 保留但 topo_layers 会报错;
                    // 这里仅做归一化,不在解析层吞错,让拓扑层给出精准提示
                    return true;
                }
                true
            })
            .collect();
        cleaned.dedup();
        if cleaned != original {
            changed = true;
            w.depends_on = cleaned;
        }
    }
    if changed {
        tracing::info!("[2026-09-16 F2] depends_on 已归一化(剥离变量透传注释 / 去自环 / 去重)");
    }
}

