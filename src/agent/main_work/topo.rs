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
    topo_layers_impl(workflows, true).map(|(layers, _)| layers)
}

/// 宽容拓扑分层(第 174 轮):未知依赖**降级为警告并丢弃该边**后继续分层,
/// 循环依赖仍然报错。返回 `(分层, 被丢弃的未知依赖描述)`。
///
/// 背景(实测 `llaew_20261022_110733.log`,百度地图任务):档位级重试时
/// Main-Work 会**局部重拆**(已完成单元不再回传),后发的 wf-N 仍声明
/// `depends_on wf-1`(已完成的单元)→ 严格 topo 报「依赖未知 wf=wf-1」,
/// 整份计划作废、整条任务终止 —— 一个悬空依赖边烧掉 3 轮重试 120k tokens。
/// 悬空边的真实语义是「前置已完成/不存在」,丢弃后按无依赖执行即可,
/// 每单元 QC 仍对真实产物把守,不会放行质量损失。
pub fn topo_layers_lenient(
    workflows: &[WorkFlowSpec],
) -> Result<(Vec<Vec<WorkFlowSpec>>, Vec<String>)> {
    topo_layers_impl(workflows, false)
}

fn topo_layers_impl(
    workflows: &[WorkFlowSpec],
    strict: bool,
) -> Result<(Vec<Vec<WorkFlowSpec>>, Vec<String>)> {
    let mut by_id: std::collections::HashMap<&str, &WorkFlowSpec> =
        std::collections::HashMap::new();
    for w in workflows {
        by_id.insert(w.id.as_str(), w);
    }
    // 入度 = 它依赖的 wf 数;先校验所有依赖已知
    let mut in_degree: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut dropped: Vec<String> = Vec::new();
    for w in workflows {
        for dep in &w.depends_on {
            if !by_id.contains_key(dep.as_str()) {
                if strict {
                    return Err(AgentError::WorkflowTopology(format!(
                        "wf={} 依赖未知 wf={}",
                        w.id, dep
                    )));
                }
                dropped.push(format!("wf={} 依赖未知 wf={}(边已丢弃,按无依赖执行)", w.id, dep));
                continue;
            }
        }
        // 入度只数已知依赖;宽容模式下未知依赖已丢弃,不入度。
        let known = w.depends_on.iter().filter(|d| by_id.contains_key(d.as_str())).count();
        in_degree.insert(w.id.as_str(), known);
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
    Ok((layers, dropped))
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

/// 「产出该文件」的动词(用于判定某 WorkFlow **生产**某产物)。
const PRODUCE_VERBS: &[&str] = &[
    "写入", "写到", "写为", "保存到", "保存为", "存到", "存为", "生成", "落盘", "产出", "输出到", "导出",
];

/// 「消费该文件」的动词(用于判定某 WorkFlow **读取**某产物)。
const CONSUME_VERBS: &[&str] = &[
    "读取", "读入", "基于", "根据", "输入为", "引用", "依据", "结合", "参照", "见上", "如上",
];

/// 动词与文件名的最大字符间距:超过则认为该动词与文件无关(宁可漏边也不误加边)。
const DEP_PROXIMITY_CHARS: usize = 24;

/// 切出「类文件名」记号并返回其在原文中的**字节起点**(用于就近回溯动词)。
///
/// 记号判定:含 `.`、不以 `.` 结尾、长度 ≥4、扩展名 1~8 位 ASCII 字母数字。
fn file_token_starts(text: &str) -> Vec<(usize, String)> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '/') {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            spans.push((s, i));
        }
    }
    if let Some(s) = start {
        spans.push((s, text.len()));
    }
    spans
        .into_iter()
        .filter_map(|(s, e)| {
            let tok = &text[s..e];
            if !tok.contains('.') || tok.ends_with('.') || tok.len() < 4 {
                return None;
            }
            let ext = tok.rsplit('.').next().unwrap_or("");
            if ext.is_empty() || ext.len() > 8 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                return None;
            }
            Some((s, tok.to_string()))
        })
        .collect()
}

/// 该文件记号前 `DEP_PROXIMITY_CHARS` 字符内是否出现过任一 `verbs`。
fn verb_near(text: &str, token_start: usize, verbs: &[&str]) -> bool {
    let head = &text[..token_start];
    // 按字符回溯窗口(中文 1 字符 = 3 字节,按字节截会切坏 UTF-8)。
    let window: String = head
        .chars()
        .rev()
        .take(DEP_PROXIMITY_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    verbs.iter().any(|v| window.contains(v))
}

/// 单元对某文件的读写角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArtifactRole {
    produces: bool,
    consumes: bool,
}

/// 扫描单个 WorkFlow 的 name + steps,得出「本单元产出哪些文件 / 消费哪些文件」。
fn artifact_roles(w: &WorkFlowSpec) -> std::collections::HashMap<String, ArtifactRole> {
    let mut map: std::collections::HashMap<String, ArtifactRole> = std::collections::HashMap::new();
    for text in std::iter::once(w.name.as_str()).chain(w.steps.iter().map(|s| s.as_str())) {
        for (start, tok) in file_token_starts(text) {
            let entry = map
                .entry(tok.clone())
                .or_insert(ArtifactRole { produces: false, consumes: false });
            entry.produces |= verb_near(text, start, PRODUCE_VERBS);
            entry.consumes |= verb_near(text, start, CONSUME_VERBS);
        }
    }
    map
}

/// 是否存在 `from → ... → to` 的路径(加边前的成环预检)。
fn has_path(edges: &[(String, String)], from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    let mut stack = vec![from.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur.clone()) {
            continue;
        }
        for (a, b) in edges {
            if a == &cur {
                if b == to {
                    return true;
                }
                stack.push(b.clone());
            }
        }
    }
    false
}

/// 串行链兜底开关(第 165 轮):`LAEW_WF_SERIAL_FALLBACK=off|0|false|no` 关闭,
/// 默认开启(对齐 `LAEW_PARALLEL_TOOLS` / `LAEW_REACT_GUARD` 惯例)。
fn serial_fallback_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            std::env::var("LAEW_WF_SERIAL_FALLBACK")
                .unwrap_or_default()
                .trim()
                .to_lowercase()
                .as_str(),
            "off" | "0" | "false" | "no"
        )
    })
}

/// 零依赖多单元计划的**串行链兜底**(第 165 轮)。
///
/// 背景(实测 `llaew_20261013_102358.log`,百度地图公交路线任务):Plan 文档明确声明
/// 依赖链 wf-1→wf-2→{wf-3,wf-4}→wf-5,但行级解析对全角冒号失明使 `depends_on`
/// 全部丢失;[`infer_depends_on`] 只能从文件产物推边,浏览器交互链(无文件)推不出
/// → 5 单元全部落同一层并发,Semaphore 槽位释放顺序取代逻辑依赖:wf-5(生成文档)
/// 早于 wf-4(提取数据)执行,产出凭常识编造的内容;上游失败局部重试又与在飞单元
/// **并发互踩同一浏览器页面**(一方 reload 摧毁另一方等待的 DOM)→ 重试风暴 17 分钟
/// 直到人工取消 —— 用户观察即「产物已生成后 Agent 又自己重新执行」。
///
/// 确定性兜底(纯结构性判定,不涉及价值判断):**所有**单元 `depends_on` 全空且
/// 单元数 ≥2 时,按数组顺序补 `wf[i] depends_on wf[i-1]`。全空 = 无任何「可并行」
/// 证据;LLM 拆解顺序即逻辑顺序;并行是猜测,串行是安全默认(第 163 轮文件依赖
/// 事故与本轮页面状态依赖事故同构:乱序并发的代价远大于串行耗时)。杀开关
/// `LAEW_WF_SERIAL_FALLBACK=off` 可退回旧行为(真独立单元可并行的场景)。
pub fn fallback_serial_chain(plan: &mut WorkFlowPlan) {
    if !serial_fallback_enabled() {
        return;
    }
    if !serial_chain_applies(&plan.workflows) {
        // 单单元 / 任一单元有显式或推断依赖边 → 已有拓扑证据,不兜底
        return;
    }
    for i in 1..plan.workflows.len() {
        let prev = plan.workflows[i - 1].id.clone();
        plan.workflows[i].depends_on.push(prev);
    }
    tracing::warn!(
        units = plan.workflows.len(),
        "[串行链兜底] 所有 WorkFlow 的 depends_on 均为空:按声明顺序串行执行(全空=无并行证据,乱序互踩风险 > 并行收益;LAEW_WF_SERIAL_FALLBACK=off 可关闭)"
    );
}

/// 串行兜底判定(独立出来便于单测,不含环境开关)。
pub fn serial_chain_applies(workflows: &[WorkFlowSpec]) -> bool {
    workflows.len() >= 2 && workflows.iter().all(|w| w.depends_on.is_empty())
}

/// 从**产物引用**反推并补齐 `depends_on` 缺边(第 163 轮)。
///
/// 背景(实测 `llaew_20261012_175823.log`,闲鱼全品类遍历任务):Main-Work 拆出的
/// 5 个 WorkFlow 里 wf-3/wf-4/wf-5 依次读取上一阶段的产物文件,但
/// **`depends_on` 全部为空**,Kahn 分层因此把 5 个单元判成**同一层并发执行**。
/// wf-3 在 18:06:04 就启动读 `闲鱼-物品类型列表.md`,而该文件直到 18:16:14 才由
/// wf-2 落盘 —— wf-3 只能轮询等待:实测 `for i in 1..6; do sleep 20; ...` 单条
/// Bash **120088ms** 空等 + `sleep 75` 75032ms,合计约 **195 秒纯浪费**,
/// 并最终以 Read 失败告终;wf-2 写盘时又撞上 Write 参数截断(第 148 轮)写失败。
///
/// 关键在于:QC **已经点名**了这个问题却仍判 `pass` —— 其 issues 原文写着
/// 「wf-2~wf-5 存在明确的数据流依赖……但 depends_on 全部为空,未声明链式依赖
/// ……并行调度时可能乱序执行」,verdict 却是 pass(retryable=false),
/// 于是这个已被点名的缺陷原样进入执行层。
///
/// 本函数做**确定性**补边(不涉及任何价值判断):某单元的文本里消费了另一样本
/// 单元产出的同名文件 → 补一条 `生产者 → 消费者` 边。逐条加边前做成环预检,
/// 宁可漏边也不制造环(环会让 `topo_layers` 整批报错、计划作废)。
pub fn infer_depends_on(plan: &mut WorkFlowPlan) {
    if plan.workflows.len() < 2 {
        return;
    }
    let roles: Vec<std::collections::HashMap<String, ArtifactRole>> =
        plan.workflows.iter().map(artifact_roles).collect();
    // 候选边(生产者 → 消费者),按 (生产者, 消费者) 字典序保证确定性。
    let mut candidates: Vec<(String, String, String)> = Vec::new();
    for (ci, consumer) in plan.workflows.iter().enumerate() {
        for (file, role) in &roles[ci] {
            if !role.consumes {
                continue;
            }
            for (pi, producer) in plan.workflows.iter().enumerate() {
                if pi == ci || !roles[pi].get(file).map(|r| r.produces).unwrap_or(false) {
                    continue;
                }
                // 消费者自己也产出同名文件 → 同单元内闭环,不加边
                if role.produces {
                    continue;
                }
                candidates.push((producer.id.clone(), consumer.id.clone(), file.clone()));
            }
        }
    }
    candidates.sort();
    candidates.dedup();

    let mut edges: Vec<(String, String)> = plan
        .workflows
        .iter()
        .flat_map(|w| w.depends_on.iter().map(move |d| (d.clone(), w.id.clone())))
        .collect();
    let mut added: Vec<(String, String)> = Vec::new();
    for (producer, consumer, file) in candidates {
        if consumer == producer {
            continue;
        }
        let target = plan
            .workflows
            .iter_mut()
            .find(|w| w.id == consumer)
            .expect("consumer 来自同一 workflows");
        if target.depends_on.iter().any(|d| *d == producer) {
            continue;
        }
        // 成环预检:补边 producer → consumer 后,consumer 不得已能到达 producer
        if has_path(&edges, &consumer, &producer) {
            tracing::warn!(
                "[depends_on 推断] 跳过成环候选边 {} → {}(产物 {};补边会形成环,宁可漏边)",
                producer,
                consumer,
                file
            );
            continue;
        }
        target.depends_on.push(producer.clone());
        edges.push((producer.clone(), consumer.clone()));
        added.push((producer, consumer));
    }
    if !added.is_empty() {
        tracing::info!(
            "[depends_on 推断] 从产物引用补齐 {} 条缺边(避免有数据依赖的单元被并发调度): {:?}",
            added.len(),
            added
        );
    }
}

