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
    /// id 唯一性守卫退役的旧节点数(诊断用)。
    pub retired: usize,
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
            // replaceChildren() 的清空半程:移除目标节点全部子节点。
            // 只清 JS 侧覆盖层不清宿主的话,静态 SSR 子树会与重建副本
            // 叠加渲染(实测猎聘页头双副本根因)。
            "clear_children" => {
                let Some(node) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                for child in node.children().collect::<Vec<_>>() {
                    child.detach();
                }
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
            "append_html" | "insert_html" => {
                let Some(node) = find(doc, &action.selector) else {
                    summary.failed += 1;
                    continue;
                };
                let fragments = parse_fragment_nodes(&action.value);
                // id 唯一性守卫(结构级退役):动态挂载引入的 id 若宿主已存在,
                // 先退役**结构对应**的旧子树再插入。依据:① 重复 id 本就是非法
                // HTML;② 框架水化失败静默回退客户端渲染时,新挂载的子树才是
                // 框架持有引用、绑事件、会继续操作的活树,旧 SSR 骨架是死标记
                // —— 两者并存会把页头整块叠印(实测猎聘:静态 SSR 菜单 +
                // React 重建副本同坐标,布局文本整段重复两遍;载体退役后残余
                // li 壳的 padding/margin 还会把新导航右推 340px 与 quick-menu
                // 叠印);③ 浏览器对水化失败的处理同样是「清旧挂新」,宿主侧
                // 收敛到单副本与 Chrome 结果一致。
                retire_superseded_subtrees(doc, &fragments, &node, &mut summary);
                for frag in fragments {
                    if action.kind == "append_html" {
                        node.append(frag);
                    } else {
                        node.insert_before(frag);
                    }
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
            // 动态样式表:运行期创建的 <link rel=stylesheet> 取回后原位内联成
            // <style data-lsm-href=…>(与静态外链样式同一真源、同层级联序)。
            // 少了这一步,webpack 5 的 CSS chunk 装载 Promise 永不 resolve,
            // 入口模块永不执行,React 应用永不 mount(实测猎聘登录页整页白屏)。
            "append_style" => {
                let href = if action.key.is_empty() {
                    action.url.as_str()
                } else {
                    action.key.as_str()
                };
                let css = action.value.replace("</style", "<\\/style");
                let style_html = format!(
                    "<style data-lsm-href=\"{}\">{}</style>",
                    escape_attr(href),
                    css
                );
                let fragments = parse_fragment_nodes(&style_html);
                if fragments.is_empty() {
                    summary.failed += 1;
                    continue;
                }
                // 插到 <head> 末尾:动态样式晚于静态样式,与浏览器级联序一致。
                let head = doc.first("head").ok().flatten();
                match head {
                    Some(h) => {
                        for frag in fragments {
                            h.append(frag);
                        }
                    }
                    None => {
                        for frag in fragments {
                            doc.root().append(frag);
                        }
                    }
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

/// 属性值转义(写进 `<style data-lsm-href="…">` 之类标记时用)。
fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn set_attr_value(node: &NodeRef, key: &str, value: &str) {
    if let Some(el) = node.as_element() {        // html5ever 解析时属性名统一小写,写入侧保持一致。
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

/// id 唯一性守卫(结构级退役):即将插入的片段若携带宿主已存在的 id,不只
/// 退役 id 载体,而是沿「宿主祖先链 × 片段祖先链」**锁步上溯**找结构对应点
/// ——同标签、同类名的最高祖先对,且旧祖先子树里的 id 全部被新片段重新声明
/// (= 完全被取代),把旧祖先**整棵**退役。见 `append_html` 分支注释:水化失败
/// 静默回退时框架新挂的子树是活树,旧 SSR 骨架是死标记,并存即叠印。
///
/// 只退役载体本身会留下空壳(实测猎聘 p5 复盘):静态 `ul.header-nav-menu` 的
/// `a#header-nav-menu-*` 退役后,残余 li 壳仍带 padding/margin 占位,把 React
/// 重建的导航整体右推 340px,与右锚定(`position:absolute;right:0`)的
/// quick-menu 叠印成一团。整棵退役结构对应的旧子树,宿主才收敛到与 Chrome
/// 水化成功一致的单一副本。
///
/// 安全边界:旧子树里存在未被新片段重新声明的 id 时,Containment 检查阻断
/// 上溯,退回只退役 id 载体(不误删仍被引用的兄弟结构);body/html 永不退役。
fn retire_superseded_subtrees(
    doc: &Document,
    fragments: &[NodeRef],
    insertion_target: &NodeRef,
    summary: &mut ApplySummary,
) {
    // 片段侧:全量 id 集合 + id → 携带节点(取其祖先链做锁步上溯)。
    let mut frag_ids: Vec<String> = Vec::new();
    let mut frag_carriers: Vec<(String, NodeRef)> = Vec::new();
    for frag in fragments {
        for node in frag.descendants() {
            let Some(id) = document::attr(&node, "id") else { continue };
            if id.is_empty() || id.len() > 128 || id.contains('"') {
                continue;
            }
            if !frag_ids.contains(&id) {
                frag_ids.push(id.clone());
            }
            frag_carriers.push((id, node));
        }
    }
    if frag_ids.is_empty() {
        return;
    }
    // 已退役的根去重:多个 id 命中同一结构对应祖先时只退役一次。
    let mut retired_roots: Vec<NodeRef> = Vec::new();
    for (id, carrier) in &frag_carriers {
        let selector = format!("[id=\"{id}\"]");
        let Ok(matches) = doc.select(&selector) else { continue };
        for old in matches {
            // 退役根:初始为 id 载体本身,锁步上溯通过全部检查可整棵扩大。
            let mut root = old.clone();
            let mut host_cur = old.clone();
            let mut frag_cur = carrier.clone();
            for _ in 0..8 {
                let (Some(host_parent), Some(frag_parent)) = (host_cur.parent(), frag_cur.parent())
                else {
                    break;
                };
                // 到达插入容器即停:片段根节点对应的是容器的**子级层**,
                // 上溯越过容器等于把插入目标自己退役掉。
                if host_parent == *insertion_target {
                    break;
                }
                let (Some(host_el), Some(frag_el)) = (host_parent.as_element(), frag_parent.as_element())
                else {
                    break;
                };
                if host_el.name.local != frag_el.name.local {
                    break;
                }
                if document::attr(&host_parent, "class").unwrap_or_default().trim()
                    != document::attr(&frag_parent, "class").unwrap_or_default().trim()
                {
                    break;
                }
                let tag = host_el.name.local.as_ref();
                if tag == "body" || tag == "html" {
                    break;
                }
                // Containment:旧祖先子树里的 id 必须全部被新片段重新声明;
                // 有 id 不在片段里 → 旧结构还有活内容,上溯到此为止。
                let fully_superseded = host_parent.descendants().all(|node| {
                    document::attr(&node, "id")
                        .map(|x| frag_ids.contains(&x))
                        .unwrap_or(true)
                });
                if !fully_superseded {
                    break;
                }
                root = host_parent.clone();
                host_cur = host_parent;
                frag_cur = frag_parent;
            }
            if retired_roots.contains(&root) {
                continue;
            }
            retired_roots.push(root.clone());
            root.detach();
            summary.retired += 1;
        }
    }
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
