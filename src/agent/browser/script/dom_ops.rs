//! 宿主侧动作应用:JS 线程回传的结构性 DOM 修改在此落到真实 kuchikiki 文档。
//!
//! 选择器定位用 `Document::select`;fragment 注入用 `parse_html` + `<body>` 包裹
//! (kuchikiki 的 `parse_fragment` 需要其内部 markup5ever 版本的 `QualName`,
//! 不引入第二套类型,零新依赖)。

use std::collections::{HashMap, HashSet};

use kuchikiki::NodeRef;

use super::JsAction;
use crate::agent::browser::document::{self, Document};

const MAX_INLINE_SCRIPTS: usize = 128;
const MAX_INLINE_BYTES: usize = 256 * 1024;
/// 单轮外链脚本上限。需覆盖大型站点的**全部**注入脚本:百度地图的 widget
/// 层一个包就注入数百个模块脚本,上限过小会让排在后面的脚本(地图图层面板
/// 依赖的 `getjsmods` 等)永远取不到,地图只剩空壳。
/// 真正的耗时约束是脚本管线的墙钟预算(`LAEW_WEB_SCRIPT_PIPELINE_MS`),
/// 数量上限只需防住病态页面。
const MAX_EXTERNAL_SCRIPTS: usize = 512;

/// 单轮应用结果:DOM 之外的副作用汇总,由调用方合并进 TabState/存储。
#[derive(Debug, Default, Clone)]
pub struct ApplySummary {
    pub applied: usize,
    pub failed: usize,
    /// selector → value(set_value/set_checked 合并)。
    pub form_values: HashMap<String, String>,
    pub title: Option<String>,
    /// 原样 Set-Cookie 行。
    pub cookies: Vec<String>,
    /// local storage 写入。
    pub storage_local: Vec<(String, String)>,
    pub storage_local_remove: Vec<String>,
    pub storage_local_clear: bool,
    /// session storage 写入。
    pub storage_session: Vec<(String, String)>,
    pub storage_session_remove: Vec<String>,
    pub storage_session_clear: bool,
    /// 滚动(set/offset)。
    pub scroll_to: Option<f64>,
    pub scroll_by: Option<f64>,
    pub focus: Option<String>,
    pub blur: bool,
    /// 非法动作示例(进 console 日志)。
    pub errors: Vec<String>,
}

/// 把 actions 应用到文档。导航类动作(click/navigate/new_tab/submit/back/forward/reload)
/// 不在此处理,由调用方在锁外执行。
pub fn apply(doc: &Document, actions: &[JsAction], base_url: &str) -> ApplySummary {
    let mut summary = ApplySummary::default();
    // LAEW_DOM_TRACE=1:诊断「页面空白/JS 建好的 UI 没回写」类问题 ——
    // 打出动作账本(kind/selector/前 N 条样本),验证完即用环境变量关掉。
    // LAEW_DOM_TRACE=2:全量模式,打出本批**所有**动作(重站定位个别动态节点
    // 的创建/样式写入链路时用;默认 25 条采样会漏掉后段动作)。
    let dom_trace = std::env::var("LAEW_DOM_TRACE").unwrap_or_default();
    let dom_trace_full = dom_trace == "2" || dom_trace.eq_ignore_ascii_case("full");
    if !dom_trace.is_empty() {
        let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
        for a in actions {
            *kinds.entry(a.kind.clone()).or_default() += 1;
        }
        eprintln!("[domtrace] apply actions={} kinds={:?}", actions.len(), kinds);
        let sample: Vec<&JsAction> = if dom_trace_full {
            actions.iter().collect()
        } else {
            actions.iter().take(25).collect()
        };
        for a in sample {
            eprintln!(
                "[domtrace]   kind={} selector={} key={} value={}",
                a.kind,
                a.selector,
                a.key,
                &a.value[..a.value.len().min(100)]
            );
        }
    }
    for action in actions {
        match action.kind.as_str() {
            "set_attr" => {
                let Some(node) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                set_attr_value(&node, &action.key, &action.value);
                summary.applied += 1;
            }
            "remove_attr" => {
                let Some(node) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                if let Some(el) = node.as_element() {
                    el.attributes.borrow_mut().remove(action.key.trim());
                }
                summary.applied += 1;
            }
            "set_text" => {
                let Some(node) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                for child in node.children().collect::<Vec<_>>() {
                    child.detach();
                }
                node.append(NodeRef::new_text(&action.value));
                summary.applied += 1;
            }
            // 追加文本节点(浏览器语义:appendChild(textNode) **不清空**既有子节点)。
            // set_text 会把子节点全换掉,用它做 appendChild 会误删元素原有内容。
            "append_text" => {
                let Some(node) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                node.append(NodeRef::new_text(&action.value));
                summary.applied += 1;
            }
            "set_html" | "replace_html" => {
                if action.selector.starts_with("@created:") {
                    // detached 元素上的 innerHTML 赋值:节点未入树,无处落地,静默忽略。
                    summary.failed += 1;
                    continue;
                }
                let Some(node) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                if action.kind == "replace_html" {
                    let Some(parent) = node.parent() else {
                        summary.failed += 1;
                        continue;
                    };
                    for frag in parse_fragment_nodes(&action.value) {
                        parent.insert_before(frag);
                    }
                    node.detach();
                } else {
                    for child in node.children().collect::<Vec<_>>() {
                        child.detach();
                    }
                    for frag in parse_fragment_nodes(&action.value) {
                        node.append(frag);
                    }
                }
                summary.applied += 1;
            }
            "append_html" => {
                let Some(node) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                for frag in parse_fragment_nodes(&action.value) {
                    node.append(frag);
                }
                summary.applied += 1;
            }
            // 把已有节点(快照节点)搬进另一个容器:按宿主真实 outerHTML 克隆子树。
            // 旧实现只推节点文本,站点 `container.appendChild(existingNode)`
            // 落进 DOM 的只有一段文字,控件整棵子树丢失(百度地图 widget 即如此)。
            "append_node" | "insert_node" => {
                let Some(src) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                let Some(dst) = find(doc, &action.key) else {
                    summary.failed += 1;
                    continue;
                };
                // 源是目标的祖先 → 搬运会成环,跳过。
                if src.ancestors().any(|a| a == dst) {
                    summary.failed += 1;
                    continue;
                }
                let html = document::outer_html(&src);
                if html.trim().is_empty() {
                    summary.failed += 1;
                    continue;
                }
                for frag in parse_fragment_nodes(&html) {
                    if action.kind == "insert_node" {
                        dst.insert_before(frag);
                    } else {
                        dst.append(frag);
                    }
                }
                summary.applied += 1;
            }
            "insert_html" => {
                let Some(node) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                for frag in parse_fragment_nodes(&action.value) {
                    node.insert_before(frag);
                }
                summary.applied += 1;
            }
            "remove_node" => {
                let Some(node) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                node.detach();
                summary.applied += 1;
            }
            "set_value" => {
                if let Some(node) = find(doc, &action.selector) {
                    set_attr_value(&node, "value", &action.value);
                    summary.applied += 1;
                }
                summary
                    .form_values
                    .insert(action.selector.clone(), action.value.clone());
            }
            "set_checked" => {
                if let Some(node) = find(doc, &action.selector) {
                    if action.value == "true" {
                        set_attr_value(&node, "checked", "");
                    } else if let Some(el) = node.as_element() {
                        el.attributes.borrow_mut().remove("checked");
                    }
                    summary.applied += 1;
                }
                summary
                    .form_values
                    .insert(action.selector.clone(), action.value.clone());
            }
            "set_title" => summary.title = Some(action.value.clone()),
            "set_cookie" => summary.cookies.push(action.value.clone()),
            "storage_local" => summary.storage_local.push((action.key.clone(), action.value.clone())),
            "storage_local_remove" => summary.storage_local_remove.push(action.key.clone()),
            "storage_local_clear" => summary.storage_local_clear = true,
            "storage_session" => summary.storage_session.push((action.key.clone(), action.value.clone())),
            "storage_session_remove" => summary.storage_session_remove.push(action.key.clone()),
            "storage_session_clear" => summary.storage_session_clear = true,
            "scroll" => {
                if let Ok(v) = action.value.parse::<f64>() {
                    summary.scroll_to = Some(v.max(0.0));
                }
            }
            "scroll_by" => {
                if let Ok(v) = action.value.parse::<f64>() {
                    summary.scroll_by = Some(v);
                }
            }
            "focus" => summary.focus = Some(action.selector.clone()),
            "blur" => summary.blur = true,
            _ => {
                // 导航类(click/navigate/submit/new_tab/back/forward/reload)与未知类型留给调用方。
            }
        }
    }
    let _ = base_url;
    summary
}

/// 应用非 DOM 副作用之外的提醒(fetch 类动作在台账已单独记录)。
pub fn note_errors(summary: &ApplySummary) -> Vec<String> {
    summary.errors.clone()
}

fn find(doc: &Document, selector: &str) -> Option<NodeRef> {
    if selector.is_empty() {
        return None;
    }
    doc.first(selector).ok().flatten()
}

fn set_attr_value(node: &NodeRef, key: &str, value: &str) {
    if let Some(el) = node.as_element() {
        // html5ever 解析时属性名统一小写,写入侧保持一致。
        el.attributes
            .borrow_mut()
            .insert(key.trim().to_ascii_lowercase(), value.to_string());
    }
}

/// 解析 HTML 片段为零散节点(不依赖 kuchikiki 内部 QualName)。
fn parse_fragment_nodes(html: &str) -> Vec<NodeRef> {
    use kuchikiki::traits::TendrilSink;
    let wrapped = format!("<body>{html}</body>");
    let parsed = kuchikiki::parse_html().one(wrapped);
    parsed
        .select_first("body")
        .map(|b| b.as_node().children().collect::<Vec<_>>())
        .unwrap_or_default()
}

/// 文档序脚本条目。内联脚本与外链 URL **必须保持 DOM 中的相对顺序**:
/// 浏览器按文档顺序同步执行脚本,装载器初始化(`mod.js` → `require.resourceMap`
/// → pkg 定义)依赖这一顺序;此前后收集成「全部内联 + 全部外链」两组再拼接,
/// 等于把后半段内联脚本提前到所有外链之前执行(实测百度地图 FIS 装载器因此
/// 拿不到 resourceMap 配置、pano 模块永不注册,页面白屏)。
#[derive(Debug, Clone)]
pub enum CollectedItem {
    Inline(super::ScriptSource),
    External(String),
}

/// 脚本收集产物。
#[derive(Debug, Default)]
pub struct CollectedScripts {
    /// 阻塞脚本(文档序):解析期按序执行。
    pub items: Vec<CollectedItem>,
    /// defer 脚本(文档序):真实浏览器在文档解析完、DOMContentLoaded 前整组
    /// 后置执行。此前管线无视 defer 全按文档序跑,SPA 站点「阻塞库脚本
    /// (vue-core/bossui)写在 defer 块之后」的顺序被打乱,defer 的 app 入口
    /// 先跑、`Vue is not defined` 整页停在加载中(实测 zhipin 搜索页)。
    pub deferred: Vec<CollectedItem>,
}

/// 收集文档中的 `<script>`:内联文本直接提取,外链 src 返回 URL 清单。
/// `executed` 为已执行内容指纹集合,跨轮去重(脚本注入的脚本不会重复执行)。
pub fn collect_scripts(doc: &Document, base_url: &str, executed: &mut HashSet<u64>) -> CollectedScripts {
    let mut out = CollectedScripts::default();
    let Ok(scripts) = doc.select("script") else {
        return out;
    };
    let mut inline_count = 0usize;
    let mut external_count = 0usize;
    // 调试账目:脚本元素总数 / 已执行跳过 / 上限截断(定位「模块没被取回」)。
    let mut scanned = 0usize;
    let mut skipped_done = 0usize;
    let mut capped = 0usize;
    for node in scripts {
        let src = document::attr(&node, "src");
        let typ = document::attr(&node, "type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !typ.is_empty()
            && !typ.contains("javascript")
            && !typ.contains("ecmascript")
            && !typ.contains("module")
        {
            continue;
        }
        if let Some(src) = src {
            // 线程内已同步装载过的动态脚本(见 prelude __lsmLoadDynScript):
            // 标记属性跳过,避免宿主轮次二次执行同一 chunk。
            if document::attr(&node, "data-lsm-executed").is_some() {
                continue;
            }
            scanned += 1;
            if external_count >= MAX_EXTERNAL_SCRIPTS {
                capped += 1;
                continue;
            }
            // defer 语义:显式 defer 属性或 type=module(规范隐式 defer)的
            // 外链脚本整组后置执行(见 CollectedScripts.deferred 注释)。
            let deferred = document::attr(&node, "defer").is_some() || typ.contains("module");
            if let Some(abs) = crate::agent::browser::urlutil::resolve(base_url, &src) {
                if abs.starts_with("http://") || abs.starts_with("https://") {
                    // 已执行过的外链不再取回:真实浏览器语义是「每个 script 元素
                    // 只执行一次」。此前外链从不进 executed,每轮(上限 24 轮)把
                    // 全部静态外链重新取回重新执行,重复初始化破坏页面启动时序
                    // (实测百度地图 widget 层因此整层停滞)。
                    if executed.contains(&fingerprint(&abs)) {
                        skipped_done += 1;
                        continue;
                    }
                    let seen = out.items.iter().chain(out.deferred.iter())
                        .any(|it| matches!(it, CollectedItem::External(u) if *u == abs));
                    if !seen {
                        let target = if deferred { &mut out.deferred } else { &mut out.items };
                        target.push(CollectedItem::External(abs));
                        external_count += 1;
                    }
                } else if let Some(rest) = abs.strip_prefix("data:") {
                    // data: URL 直接作为内联内容。
                    if inline_count < MAX_INLINE_SCRIPTS {
                        if push_inline(&mut out, rest, executed, &abs) {
                            inline_count += 1;
                        }
                    }
                }
            }
            continue;
        }
        let text = node.text_contents();
        if inline_count >= MAX_INLINE_SCRIPTS || text.len() > MAX_INLINE_BYTES {
            continue;
        }
        if push_inline(&mut out, &text, executed, "") {
            inline_count += 1;
        }
    }
    if std::env::var("LAEW_DOM_TRACE").is_ok() {
        eprintln!(
            "[domtrace] collect scanned={} skipped_executed={} capped={} inline={} external={}",
            scanned,
            skipped_done,
            capped,
            inline_count,
            external_count
        );
        for it in out.items.iter().take(6) {
            if let CollectedItem::External(u) = it {
                eprintln!("[domtrace]   picked external {}", u);
            }
        }
    }
    out
}

/// 追加一条内联脚本；空内容/已执行过返回 `false`。
fn push_inline(
    out: &mut CollectedScripts,
    content: &str,
    executed: &mut HashSet<u64>,
    src_url: &str,
) -> bool {
    let fp = fingerprint(content);
    if executed.contains(&fp) {
        return false;
    }
    executed.insert(fp);
    // data: URL 简化处理:取逗号后内容,base64 变体按原样跳过。
    let body = if let Some(rest) = content.strip_prefix("data:") {
        rest.split_once(',')
            .map(|(_, payload)| payload.to_string())
            .unwrap_or_default()
    } else {
        content.to_string()
    };
    if body.trim().is_empty() {
        return false;
    }
    out.items.push(CollectedItem::Inline(super::ScriptSource {
        kind: "inline",
        content: body,
        src_url: src_url.to_string(),
    }));
    true
}

/// 内容/URL 指纹:内联脚本与外链 URL 共用的执行记账键。
pub(super) fn fingerprint(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
