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
    // 同时去掉首尾可能残留的引号 / 方括号。第 160 轮扩充噪声集:Markdown 粗体
    // (`**wf-1**`)、代码反引号、成对/残留括号与句读(`**不声明跨项引用**)` 实测形态)。
    let end = trimmed
        .find(|c: char| matches!(c, '（' | '(' | '：' | ':' | ' ' | '\t'))
        .unwrap_or(trimmed.len());
    let token = trimmed[..end].trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '[' | ']' | ',' | '、' | '*' | '`' | ')' | '）' | '。' | '，' | '.'
                | ';' | '；'
        )
    });
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

/// 「无依赖」占位符判定(第 160 轮)。LLM 常把空依赖写成中文占位词或英文 null 族,
/// 实测形态(`llaew_20261012_143803.log`):`depends_on: ["无"]` —— 这类条目
/// 不是任何 wf id 的拼写错误,是「没有依赖」的语义,应直接丢弃而非报拓扑错。
fn is_no_dep_placeholder(token: &str) -> bool {
    let l = token.trim().to_lowercase();
    matches!(
        l.as_str(),
        "无" | "没有" | "无依赖" | "不依赖" | "无前置" | "无前置依赖" | "空" | "none" | "null"
            | "nil" | "n/a" | "na" | "-" | "—" | "0"
    )
}

/// id 形状判定:纯 ASCII 标识符(`wf-9` / `step_2`)。形状像 id 但不在已知集合里 →
/// 大概率是真 id 的拼写错误,保留给 `topo_layers` 报精准错误(不吞);含 CJK/空格的
/// 散文碎片则走 [`extract_known_ids`] 抽取。
fn looks_like_wf_id(token: &str) -> bool {
    let mut chars = token.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 从散文条目里抽取全部已知 wf id(第 160 轮)。
///
/// 长度降序匹配防前缀吞并(`wf-10` 先于 `wf-1`);ASCII 词边界检查防 `mywf-1` 误命中;
/// 已消费跨度不再重叠。例:`"先完成 wf-1 与 wf-2"` → `[wf-1, wf-2]`。
fn extract_known_ids(raw: &str, known_sorted: &[&str]) -> Vec<String> {
    let mut consumed: Vec<(usize, usize)> = Vec::new();
    let mut found: Vec<String> = Vec::new();
    for k in known_sorted {
        let mut search_from = 0usize;
        while let Some(pos) = raw[search_from..].find(&**k) {
            let start = search_from + pos;
            let end = start + k.len();
            let boundary_ok = |c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_');
            let ok_left = raw[..start].chars().next_back().map_or(true, boundary_ok);
            let ok_right = raw[end..].chars().next().map_or(true, boundary_ok);
            let overlaps = consumed.iter().any(|(a, b)| start < *b && *a < end);
            if ok_left && ok_right && !overlaps {
                consumed.push((start, end));
                if !found.iter().any(|f| f == *k) {
                    found.push((*k).to_string());
                }
            }
            search_from = end;
        }
    }
    found
}

/// 归一化整份计划的 `depends_on` 列表:剥离注释、去空、去自环、去重。
/// 在两个解析入口(Main-Work JSON / Plan 文档)都调用,确保 `topo_layers` 拿到干净的 id 集合。
///
/// 第 160 轮(实测事故 `llaew_20261012_143803.log`):Main-Work/Plan 把空依赖写成
/// `["无"]` / `["无","与"]` / `["无","**不声明跨项引用**)"]` 这类中文占位与散文碎片,
/// 旧策略「未知条目一律保留给 topo 报错」让拓扑层反复整批失败 → 档位重试全链路
/// 重跑(Plan→QC→Main→QC 一轮 3~7 分钟),22 分钟零单元执行后被用户 Ctrl-C。
/// 新策略按条目形态分流:
/// - 占位符(无/none/null/…) → 静默丢弃(语义即「无依赖」);
/// - 已知 id → 保留;
/// - **id 形状但未知**(`wf-9`) → 保留,`topo_layers` 报精准错误(防真拼写错误被吞);
/// - 散文碎片 → 从原文抽取全部已知 id(`"先完成 wf-1 与 wf-2"` → 两条边);
///   抽不到任何 id 的纯散文 → 丢弃并 warn(可观测,不再阻断)。
pub fn sanitize_depends_on(plan: &mut WorkFlowPlan) {
    let ids: std::collections::HashSet<String> =
        plan.workflows.iter().map(|w| w.id.clone()).collect();
    // 长度降序的已知 id 表,供散文抽取防前缀吞并(wf-10 先于 wf-1);
    // 等长时按字典序,保证抽取结果顺序确定(HashSet 收集序不确定)
    let mut known_sorted: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
    known_sorted.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    let mut changed = false;
    for w in plan.workflows.iter_mut() {
        let original = w.depends_on.clone();
        let mut cleaned: Vec<String> = Vec::new();
        for raw in &original {
            let Some(token) = normalize_dep_id(raw) else {
                continue;
            };
            if is_no_dep_placeholder(&token) {
                continue;
            }
            if token == w.id {
                // 自环无意义,丢弃
                continue;
            }
            if ids.contains(&token) {
                cleaned.push(token);
                continue;
            }
            if looks_like_wf_id(&token) {
                // id 形状但未知 → 保留,让 topo_layers 给出精准报错(拼写错误通道)
                cleaned.push(token);
                continue;
            }
            // 散文碎片:从原始条目抽取已知 id;抽不到则丢弃(warn 可观测)
            let extracted = extract_known_ids(raw, &known_sorted);
            if extracted.is_empty() {
                tracing::warn!(
                    "[depends_on 净化] wf={} 丢弃无法解析的散文条目 {:?}",
                    w.id,
                    raw
                );
                continue;
            }
            for d in extracted {
                if d != w.id {
                    cleaned.push(d);
                }
            }
        }
        // 去重(保序;散文抽取可能引入与显式声明重复的边)
        let mut seen = std::collections::HashSet::new();
        cleaned.retain(|d| seen.insert(d.clone()));
        if cleaned != original {
            changed = true;
            w.depends_on = cleaned;
        }
    }
    if changed {
        tracing::info!(
            "[depends_on 净化] 已归一化(剥离注释 / 占位符丢弃 / 散文抽 id / 去自环 / 去重)"
        );
    }
}

