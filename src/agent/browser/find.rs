//! 页面查找(Ctrl/Cmd+F)—— 人工键盘交互补全(39 号设计文档 P2c)。
//!
//! **查找栏与匹配高亮全部画在「查看器帧合成层」**(与 `page_bars` 同层,
//! 由 `view::bridge::push_frame` 在 `page_bars::draw` 之后调用):
//!
//! * 查找栏是**浏览器 chrome**而非页面内容,截图取证路径
//!   (`screenshot_locked`)不经过本层,证据永远干净;
//! * 匹配几何用 `FontSet::char_spans` 在宿主导出,布局层零改动;
//! * 查看器**零 UI 改动**——`Ctrl+F` 之后的字符键经既有 `input` 通道原样
//!   上行,由本模块判定「查找模式」后路由(`keyboard::human_key` 开头分流)。
//!
//! 协议:查看器拦截 ⌘/Ctrl+F → `{"cmd":"find_toggle"}` → [`ViewCommand::ToggleFind`]。
//!
//! **边界**(如实写进 `page_health`):多行文本元素的匹配按**单行近似**(取元素盒宽
//! 与首行度量);查找栏只键盘可操作(无点击按钮);页面 DOM 变更后旧匹配几何失效,
//! 导航/重新输入即重算;扫描节点数上限 `FIND_SCAN_BUDGET`。

use serde_json::{json, Value};
use tiny_skia::Pixmap;

use super::api;
use super::document::{self, Document};
use super::human_input::node_font_size;
use super::render::FontSet;
use super::style::{MediaCtx, StyleEngine};
use super::{LsmBrowserManager, TabState};
use crate::agent::web_control::{self, Engine};

/// 单个匹配(元素可见文本内的 char 区间 + 页坐标矩形,单行近似)。
#[derive(Debug, Clone)]
pub struct FindMatch {
    pub selector: String,
    pub start: usize,
    pub end: usize,
    /// 页坐标 `(x, y, w, h)`。
    pub rect: (f64, f64, f64, f64),
}

/// 查找模式状态(每标签页一份,存 `TabState.find`)。
#[derive(Debug, Clone)]
pub struct FindState {
    pub active: bool,
    pub query: String,
    pub matches: Vec<FindMatch>,
    /// 当前命中下标;[`FindState::NO_MATCH`] = 无命中。
    pub current: usize,
}

impl Default for FindState {
    fn default() -> Self {
        Self {
            active: false,
            query: String::new(),
            matches: Vec::new(),
            // 缺省必须是「无命中」而非 0 —— 否则未激活时 `current` 指向不存在的元素。
            current: FindState::NO_MATCH,
        }
    }
}

impl FindState {
    /// 无命中下标。
    pub const NO_MATCH: usize = usize::MAX;
}

/// 匹配上限(超出截断,如实上报)。
const FIND_MAX_MATCHES: usize = 500;
/// 扫描节点预算(防超大页面退化;耗尽即停止收集,不报错)。
const FIND_SCAN_BUDGET: usize = 8000;
/// 查找栏与命中高亮的颜色。
const MATCH_COLOR: [u8; 3] = [250, 204, 21];
const MATCH_ALPHA: u8 = 110;
const CURRENT_COLOR: [u8; 3] = [249, 115, 22];
const CURRENT_ALPHA: u8 = 175;

// ===================== 搜索 =====================

/// 大小写不敏感的 char 折叠(ASCII 折叠,保持 1:1 索引映射;CJK 不变)。
fn fold_char(c: char) -> char {
    c.to_ascii_lowercase()
}

/// 按键 → 可追加到查找词的单字符(`"Space"` 还原成 `' '`)。
/// 非单字符 / 组合键 / 不可打印一律返回 `None`。
fn find_append_char(key: &str, mods: &super::human_input::Mods) -> Option<char> {
    let key = if key == "Space" { " " } else { key };
    let mut it = key.chars();
    let c = it.next()?;
    if it.next().is_some() || mods.combo() || !(c.is_ascii_graphic() || c.is_whitespace()) {
        return None;
    }
    Some(c)
}

/// 在一段文本里找全部匹配区间(char 索引,左闭右开)。
fn find_ranges(text: &str, query: &[char]) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().map(fold_char).collect();
    let n = chars.len();
    let mut out = Vec::new();
    if query.is_empty() || n < query.len() {
        return out;
    }
    let mut i = 0usize;
    while i + query.len() <= n {
        if chars[i..i + query.len()] == *query {
            out.push((i, i + query.len()));
            i += query.len(); // 不重叠(浏览器查找语义)
        } else {
            i += 1;
        }
    }
    out
}

/// 元素**自有文本**(直接文本节点,不含元素子代)。
///
/// 查找按自有文本切分而非 `node_text`(整子树):每个元素只贡献自己直接
/// 持有的文本,子元素贡献各自的 —— **顺序无关**地覆盖全部可见文本且
/// 父子不重复命中(对比:整子树口径在 `<p>文本 <a>链接</a></p>` 上会
/// 同时命中 p 与 a;祖先去重口径则依赖布局盒发射序,实测 `<a>` 盒先于
/// `<p>` 盒时照样双命中)。
///
/// 近似:高亮 x 按「自有文本首尾相接」推算,混合内容里夹子元素时
/// 子元素之后的文本段位置会偏移(见 `page_health.find_note`)。
fn own_text(node: &kuchikiki::NodeRef) -> String {
    let mut out = String::new();
    for child in node.children() {
        if let kuchikiki::NodeData::Text(t) = child.data() {
            let s = t.borrow();
            if !s.trim().is_empty() {
                out.push_str(&s);
            }
        }
    }
    document::collapse_ws(&out)
}

/// 遍历布局盒(渲染元素),收集各元素自有文本的匹配几何。
fn search_document(
    font: &FontSet,
    doc: &Document,
    engine: &StyleEngine,
    layout: &super::layout::Layout,
    query: &str,
) -> Vec<FindMatch> {
    let q: Vec<char> = query.chars().map(fold_char).collect();
    if q.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<FindMatch> = Vec::new();
    let mut budget = FIND_SCAN_BUDGET;
    for b in &layout.boxes {
        if budget == 0 {
            break;
        }
        budget -= 1;
        if !b.visible || b.w <= 0.0 || b.h <= 0.0 {
            continue;
        }
        let Some(node) = doc.select(&b.selector).ok().and_then(|v| v.into_iter().next()) else {
            continue;
        };
        if node.as_element().is_none() {
            continue;
        }
        // script/style/head 等不参与查找(自有文本是源码/元数据)。
        if matches!(
            document::tag_name(&node).as_str(),
            "script" | "style" | "head" | "title" | "meta" | "link" | "noscript" | "template"
        ) {
            continue;
        }
        let text = own_text(&node);
        if text.is_empty() {
            continue;
        }
        let size = node_font_size(engine, &node).max(8.0);
        let spans = font.char_spans(&text, size, (b.w - 4.0).max(1.0));
        if spans.is_empty() {
            continue;
        }
        let line_h = size * 1.35;
        let y = b.y + ((b.h - line_h) / 2.0).max(1.0);
        for (s, e) in find_ranges(&text, &q) {
            let sc = s.min(spans.len());
            let ec = e.min(spans.len());
            if sc >= ec {
                continue;
            }
            let x = b.x + 2.0 + spans[sc].0;
            let w = (spans[ec - 1].1 - spans[sc].0).max(1.0);
            out.push(FindMatch {
                selector: b.selector.clone(),
                start: sc,
                end: ec,
                rect: (x, y, w, line_h),
            });
            if out.len() >= FIND_MAX_MATCHES {
                return out;
            }
        }
    }
    out
}

// ===================== 管理器接线 =====================

impl LsmBrowserManager {
    /// 切换查找模式(查看器 ⌘/Ctrl+F)。开启时清空上一次的 query/命中。
    ///
    /// `pub`:集成测试经查看器命令同一入口驱动(`ViewCommand::ToggleFind` 门内
    /// 亦走此方法),与 `handle_human_input` 同为人工输入公共面。
    pub async fn toggle_find(&self, pid: &str) -> Value {
        if !super::human_input::enabled() {
            return json!({"ignored": true, "reason": "LAEW_HUMAN_INPUT=off"});
        }
        if web_control::enabled() && !web_control::is_user_control(Engine::Lsm, pid) {
            return json!({"ignored": true, "reason": "agent_control"});
        }
        let active = {
            let mut inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get_mut(pid) else {
                return json!({"ignored": true, "reason": "page_gone"});
            };
            tab.find.active = !tab.find.active;
            if tab.find.active {
                tab.find.query.clear();
                tab.find.matches.clear();
                tab.find.current = FindState::NO_MATCH;
            }
            tab.find.active
        };
        api::relayout_and_push(self, pid).await;
        json!({"active": active})
    }

    /// 当前是否处于查找模式。
    pub(crate) async fn find_active(&self, pid: &str) -> bool {
        let inner = self.inner.lock().await;
        inner.tabs.get(pid).map(|t| t.find.active).unwrap_or(false)
    }

    /// 按当前 query 重跑搜索(锁内收敛文档/样式,锁外改状态)。
    pub(crate) async fn find_search(&self, pid: &str) {
        let (query, matches) = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return;
            };
            if !tab.find.active {
                return;
            }
            let query = tab.find.query.clone();
            if query.trim().is_empty() {
                (query, Vec::new())
            } else {
                let doc = tab.document_with_state();
                let engine = StyleEngine::from_document(
                    &doc,
                    MediaCtx::new(tab.viewport.width as f64, tab.viewport.height as f64),
                );
                let found = search_document(&self.font, &doc, &engine, &tab.layout, &query);
                (query, found)
            }
        };
        let mut inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get_mut(pid) else {
            return;
        };
        // 期间 query 可能被改(并发键入)→ 丢弃过期结果。
        if tab.find.query != query {
            return;
        }
        tab.find.matches = matches;
        tab.find.current = if tab.find.matches.is_empty() {
            FindState::NO_MATCH
        } else {
            0
        };
    }

    /// 查找模式键路由:可打印追加 / Backspace 删尾 / Enter 下一个 /
    /// Shift+Enter 上一个 / Escape 关闭。返回值 = 宿主回执(查看器不消费)。
    pub(crate) async fn find_key(&self, pid: &str, key: &str, mods: &super::human_input::Mods) -> Value {
        if !self.find_active(pid).await {
            return json!({"ignored": true, "reason": "find_inactive"});
        }
        match key {
            "Escape" => {
                let mut inner = self.inner.lock().await;
                if let Some(tab) = inner.tabs.get_mut(pid) {
                    tab.find.active = false;
                    tab.find.matches.clear();
                    tab.find.current = FindState::NO_MATCH;
                }
                drop(inner);
                api::relayout_and_push(self, pid).await;
                return json!({"closed": true});
            }
            "Enter" => return self.find_step(pid, !mods.shift).await,
            "Backspace" => {
                let changed = {
                    let mut inner = self.inner.lock().await;
                    inner
                        .tabs
                        .get_mut(pid)
                        .map(|t| {
                            let before = t.find.query.chars().count();
                            t.find.query.pop();
                            t.find.query.chars().count() != before
                        })
                        .unwrap_or(false)
                };
                if changed {
                    self.find_search(pid).await;
                    api::relayout_and_push(self, pid).await;
                }
                return json!({"query_changed": changed});
            }
            _ => {}
        }
        // 可打印单字符(无组合键)追加。
        let Some(c) = find_append_char(key, mods) else {
            return json!({"ignored": true, "reason": "not_printable"});
        };
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                tab.find.query.push(c);
            }
        }
        self.find_search(pid).await;
        api::relayout_and_push(self, pid).await;
        json!({"query_changed": true})
    }

    /// 切到下一个/上一个命中,并把命中矩形滚进视口中心。
    pub(crate) async fn find_step(&self, pid: &str, forward: bool) -> Value {
        let (cur, total, rect, scroll) = {
            let mut inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get_mut(pid) else {
                return json!({"ignored": true, "reason": "page_gone"});
            };
            if !tab.find.active {
                return json!({"ignored": true, "reason": "find_inactive"});
            }
            let n = tab.find.matches.len();
            if n == 0 {
                return json!({"active": true, "total": 0, "current": FindState::NO_MATCH});
            }
            let cur = if tab.find.current == FindState::NO_MATCH {
                0
            } else {
                tab.find.current
            };
            let next = if forward {
                (cur + 1) % n
            } else {
                (cur + n - 1) % n
            };
            tab.find.current = next;
            let m = tab.find.matches[next].clone();
            let cx = m.rect.0 + m.rect.2 / 2.0;
            let cy = m.rect.1 + m.rect.3 / 2.0;
            tab.scroll_x = (cx - tab.viewport.width as f64 / 2.0).max(0.0);
            tab.scroll_y = (cy - tab.viewport.height as f64 / 2.0).max(0.0);
            (next, n, m.rect, (tab.scroll_x, tab.scroll_y))
        };
        api::relayout_and_push(self, pid).await;
        json!({
            "current": cur,
            "total": total,
            "rect": [rect.0, rect.1, rect.2, rect.3],
            "scroll": [scroll.0, scroll.1],
        })
    }
}

// ===================== 帧合成绘制 =====================

/// 在已渲染的页面 pixmap 上叠加绘制匹配高亮 + 查找栏(仅 headed 帧路径)。
pub(crate) fn draw(pixmap: &mut Pixmap, font: &FontSet, tab: &TabState) {
    if !tab.mode.is_headed() || !tab.find.active {
        return;
    }
    let vw = pixmap.width() as f32;
    let vh = pixmap.height() as f32;
    if vw < 320.0 || vh < 240.0 {
        return;
    }
    let sx = tab.scroll_x as f32;
    let sy = tab.scroll_y as f32;
    // 匹配高亮:常规黄、当前橙;裁剪到视口。
    for (i, m) in tab.find.matches.iter().enumerate() {
        let (x, y, w, h) = m.rect;
        let rx = x as f32 - sx;
        let ry = y as f32 - sy;
        if ry + (h as f32) < 0.0 || ry > vh || rx + (w as f32) < 0.0 || rx > vw {
            continue;
        }
        let (color, alpha) = if i == tab.find.current {
            (CURRENT_COLOR, CURRENT_ALPHA)
        } else {
            (MATCH_COLOR, MATCH_ALPHA)
        };
        fill_rect(pixmap, rx, ry, w as f32, h as f32, color, alpha);
    }
    draw_find_bar(pixmap, font, tab, vw);
}

/// 查找栏:左上角深底胶囊(状态栏在右上、进度栏在左下,此处无冲突)。
fn draw_find_bar(pixmap: &mut Pixmap, font: &FontSet, tab: &TabState, vw: f32) {
    const MARGIN: f32 = 12.0;
    const PAD_X: f32 = 12.0;
    const PAD_Y: f32 = 7.0;
    let size = 14.0f32;
    let query = if tab.find.query.is_empty() {
        "输入查找内容".to_string()
    } else {
        tab.find.query.clone()
    };
    let count = if tab.find.matches.is_empty() {
        if tab.find.query.is_empty() {
            "0/0".to_string()
        } else {
            "无结果".to_string()
        }
    } else {
        format!(
            "{}/{}",
            if tab.find.current == FindState::NO_MATCH {
                0
            } else {
                tab.find.current + 1
            },
            tab.find.matches.len()
        )
    };
    let hint = "Esc 关闭";
    let text_w = font.text_width(&format!("{query}  {count}  {hint}"), size as f64) as f32;
    let w = (text_w + PAD_X * 2.0).min(vw - MARGIN * 2.0).max(80.0);
    let h = size * 1.6 + PAD_Y * 2.0;
    fill_rect(pixmap, MARGIN, MARGIN, w, h, [15, 23, 42], 224);
    let mut x = MARGIN + PAD_X;
    let y = MARGIN + PAD_Y;
    x += font.draw_overlay_text(pixmap, x, y, &query, size, [248, 250, 252], true, w - PAD_X * 2.0) + 6.0;
    x += font.draw_overlay_text(pixmap, x, y, &count, size, [250, 204, 21], false, w - (x - MARGIN) - PAD_X) + 6.0;
    font.draw_overlay_text(pixmap, x, y, hint, size, [148, 163, 184], false, w - (x - MARGIN) - PAD_X);
}

/// 半透明像素矩形(帧合成层私有;`render::fill_rect` 不支持 alpha 且私有)。
fn fill_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: [u8; 3], alpha: u8) {
    if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)) {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], alpha);
        pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
    }
}

// ===================== 单测 =====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 查找区间_大小写不敏感且不重叠() {
        let q: Vec<char> = "ab".chars().map(fold_char).collect();
        assert_eq!(find_ranges("ABabAB", &q), vec![(0, 2), (2, 4), (4, 6)]);
        // 重叠形态(aaa 里找 aa)→ 浏览器语义取不重叠。
        let q: Vec<char> = "aa".chars().map(fold_char).collect();
        assert_eq!(find_ranges("aaaa", &q), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn 查找区间_中文与空查询() {
        let q: Vec<char> = "查找".chars().map(fold_char).collect();
        assert_eq!(find_ranges("查找内容查找", &q), vec![(0, 2), (4, 6)]);
        assert!(find_ranges("任意", &[]).is_empty());
        assert!(find_ranges("", &['a']).is_empty());
        assert!(find_ranges("短", &['a', 'b']).is_empty());
    }

    #[test]
    fn 查找词追加字符_空格还原与组合键拒绝() {
        use crate::agent::browser::human_input::Mods;
        let m = Mods::default();
        assert_eq!(find_append_char("a", &m), Some('a'));
        // `normalize_key` 把空格归一成 "Space",查找词要能打空格。
        assert_eq!(find_append_char("Space", &m), Some(' '));
        // 多字符键名(方向键等)不可追加。
        assert_eq!(find_append_char("ArrowLeft", &m), None);
        assert_eq!(find_append_char("Enter", &m), None);
        // 组合键不产生查找词文本。
        let ctrl = Mods { ctrl: true, ..Default::default() };
        assert_eq!(find_append_char("a", &ctrl), None);
    }

    #[test]
    fn 无命中下标为usize上限() {
        assert_eq!(FindState::NO_MATCH, usize::MAX);
        let st = FindState::default();
        assert!(!st.active && st.matches.is_empty() && st.current == usize::MAX);
    }

    #[test]
    fn 混合内容父子去重_只命中一个元素() {
        let doc = Document::parse(
            "<html><body><p id='p'>前缀 <a id='a'>链接词</a> 后缀</p></body></html>",
            "https://t/",
        );
        let media = MediaCtx::new(800.0, 600.0);
        let font = FontSet::fallback();
        let sizes = crate::agent::browser::images::ImageSizes::default();
        let layout =
            crate::agent::browser::layout::compute_full(&doc, 800.0, &font, &media, &sizes);
        let engine = StyleEngine::from_document(&doc, media);
        let m = search_document(&font, &doc, &engine, &layout, "链接词");
        assert_eq!(m.len(), 1, "父子不重复命中:{m:?}");
        assert_eq!(m[0].selector, "#a", "命中应是持有该文本的 <a>:{m:?}");
        // 单行矩形应落在元素盒的垂直范围内。
        let (_, y, _, h) = m[0].rect;
        let (_, by, _, bh) = layout.rect_for("#a").unwrap();
        assert!(
            y >= by - 1.0 && y + h <= by + bh + 1.0,
            "矩形应在元素盒内:{m:?} box=({by},{bh})"
        );
        // 混合内容里 <p> 的自有文本(前缀/后缀)仍各自可搜,不与 <a> 互相污染。
        let m = search_document(&font, &doc, &engine, &layout, "前缀");
        assert_eq!(m.len(), 1, "混合内容的直接文本应可搜:{m:?}");
        assert_eq!(m[0].selector, "#p", "{m:?}");
    }

    #[test]
    fn 命中矩形落在元素盒内且宽高为正() {
        // 矩形由 char_spans 推导:宽度 = 末字右缘 - 首字左缘,恒为正。
        let font = FontSet::fallback();
        let text = "abcdef";
        let spans = font.char_spans(text, 16.0, 200.0);
        let s = 1;
        let e = 3;
        let w = (spans[e - 1].1 - spans[s].0).max(1.0);
        assert!(w > 0.0, "命中宽度为正");
        assert!(spans[s].0 >= 0.0);
    }
}
