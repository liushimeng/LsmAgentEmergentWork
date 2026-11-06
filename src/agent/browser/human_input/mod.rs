//! 人类鼠标键盘交互:查看器页面区输入 → 宿主事件管线(39 号设计文档)。
//!
//! 有头查看器(`laew __browser-view`)此前是**被动帧显示窗**——人工能看、
//! 能用工具栏/地址栏导航,但页面区域内没有任何键鼠通道。本模块补齐这条
//! 输入链路的宿主半边:
//!
//! * 查看器把原始鼠标/键盘事件经 stdout 命令通道(`{"cmd":"input",...}`)上报,
//!   `view::parse_command` 解析成 [`HumanInput`],`view::bridge` 路由到
//!   [`LsmBrowserManager::handle_human_input`];
//! * 本模块做**控制权闸门**(agent 态忽略)+ **坐标命中**(`human_hit_test`) +
//!   **复用既有 Agent 侧交互管线**(`api::click_inner` / JS 事件派发 /
//!   控件原生语义 / 导航),不另造第二套事件系统;
//! * 键盘/剪贴板语义在 [`keyboard`] 子模块(P2a 起为光标/选区/编辑模型:
//!   光标处插入删除、选区替换、Ctrl 词跳、Ctrl+A/Z/Y 等),编辑纯逻辑在
//!   [`text_edit`] 子模块。
//!
//! 设计与阶段边界:`docs/MCP_Web_LsmAgentBrowser_Use/05-窗口交互与页面观察/
//! 39-人类鼠标键盘交互与真浏览器输入链路设计.md`(IME/页面级自由选区/浏览器
//! 级缩放属后续阶段)。

use serde_json::{json, Value};

use super::api;
use super::document::{self, CaretState, Document};
use super::script;
use super::layout;
use super::style::{MediaCtx, StyleEngine};
use super::{page_bars, DatalistState, LsmBrowserManager, TabState};
use crate::agent::web_control::{self, Engine};

pub mod clipboard;
pub mod text_edit;

mod datalist;
mod gestures;
mod keyboard;
mod pointer;
pub use keyboard::{edit_op_for, EditOp};
pub use pointer::pointer_payload;

#[cfg(test)]
mod tests;

/// 查看器上报的人类输入事件(协议见 39 号设计文档 §2.1)。
#[derive(Debug, Clone, PartialEq)]
pub enum HumanInput {
    /// 指针移动(查看器侧已节流 ≥30ms 且位移 ≥1px)。
    Move { x: f64, y: f64 },
    /// 按下。`mods` 供 Cmd/Ctrl+点击新标签等组合语义。
    Down { x: f64, y: f64, button: MouseButton, mods: Mods },
    /// 抬起(宿主侧合成 click/dblclick)。
    Up { x: f64, y: f64, button: MouseButton, mods: Mods },
    /// 滚轮;`dy>0` = 向上滚(两平台语义已归一),单位为归一化档位
    /// (macOS 原始像素 / Windows 120 为一档,查看器侧已除齐)。
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    /// 键按下(按键为"完成态"语义,查看器不单独上报 keyup)。
    Key { key: String, mods: Mods },
}

/// 鼠标键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// 光标形状(39 号 P2c:人工悬停时查看器跟随变形)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    Default,
    Pointer,
    Text,
}

impl CursorKind {
    /// 帧 kind `'k'` 的 payload 取值。
    pub fn as_str(self) -> &'static str {
        match self {
            CursorKind::Default => "default",
            CursorKind::Pointer => "pointer",
            CursorKind::Text => "text",
        }
    }
}

/// 由节点判定光标形状(纯函数,单测覆盖)。
///
/// 文本类控件 → `Text`(I 型);链接/按钮/控件/`role`/`onclick`/`tabindex`/
/// `contenteditable` → `Pointer`(手型);其余 `Default`(箭头)。
pub(crate) fn cursor_kind_for_node(
    node: &kuchikiki::NodeRef,
    tag: &str,
    typ: &str,
) -> CursorKind {
    if is_text_like(tag, typ) {
        return CursorKind::Text;
    }
    if tag == "a" && document::attr(node, "href").is_some() {
        return CursorKind::Pointer;
    }
    if matches!(tag, "button" | "select" | "summary" | "label" | "option" | "input") {
        return CursorKind::Pointer;
    }
    if let Some(role) = document::attr(node, "role") {
        let r = role.to_ascii_lowercase();
        if matches!(
            r.as_str(),
            "button" | "link" | "checkbox" | "radio" | "tab" | "menuitem" | "combobox"
                | "slider" | "option"
        ) {
            return CursorKind::Pointer;
        }
    }
    if document::attr(node, "onclick").is_some()
        || document::attr(node, "tabindex").is_some()
        || document::attr(node, "contenteditable").is_some()
    {
        return CursorKind::Pointer;
    }
    CursorKind::Default
}

impl MouseButton {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "left" | "0" => Some(MouseButton::Left),
            "right" | "2" => Some(MouseButton::Right),
            "middle" | "1" => Some(MouseButton::Middle),
            _ => None,
        }
    }
}

/// 修饰键状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Mods {
    fn from_json(v: &Value) -> Self {
        let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        Mods {
            shift: b("shift"),
            ctrl: b("ctrl"),
            alt: b("alt"),
            meta: b("meta"),
        }
    }

    /// 组合键(非纯 Shift)是否按下:组合键不产生页面文本/导航语义。
    pub(super) fn combo(&self) -> bool {
        self.ctrl || self.meta || self.alt
    }
}

/// 人类输入是否启用(`LAEW_HUMAN_INPUT=off|0|false|no` 全关:查看器停止
/// 上报 + 宿主拒绝,双保险)。
pub fn enabled() -> bool {
    !matches!(
        std::env::var("LAEW_HUMAN_INPUT")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 按键会话态(按 page_id 分桶:多标签页各持一套指针状态,互不串扰;
/// 查看器单指针,同一页内的天然串行)。
#[derive(Debug, Default)]
pub struct HumanSession {
    pages: std::collections::HashMap<String, PageInputState>,
    /// agent 态下被忽略的输入计数(台账落账节流用;全局计数)。
    ignored: u64,
}

/// 单页输入状态。
#[derive(Debug, Default)]
struct PageInputState {
    /// 当前按下(mousedown 已发、mouseup 未到)。
    press: Option<PressState>,
    /// 最近一次有效单击(双击判定窗口 500ms/6px;`count` 供双击/三击语义)。
    last_click: Option<ClickMark>,
    /// 当前 hover 目标(selector;同元素不重排)。
    hover: Option<String>,
    /// 最近一次 JS mousemove 派发时刻(毫秒;100ms 节流)。
    last_js_move_ms: u128,
    /// HTML5 drag 近似:按下后位移超阈值时已派发 dragstart。
    drag_started: bool,
    /// P2a:文本控件内的按下(= 选区拖选会话;文本拖动不触发 HTML5 drag)。
    text_drag: Option<TextDragState>,
    /// P2a:拖选已发生(位移超阈值)——收尾不合成 click(浏览器语义)。
    text_dragging: bool,
    /// P2b:页面级自由选区的拖选会话(跨元素;与 `text_drag` 互斥)。
    page_drag: Option<PageDragState>,
    /// P2b:页面拖选已越阈值(收尾不合成 click)。
    page_dragging: bool,
    /// P2a:单控件撤销/重做历史(key = stable selector)。
    edits: std::collections::HashMap<String, text_edit::EditHistory>,
    /// P2c:最近一次上报查看器的光标形状(仅在变化时发帧)。
    pub(crate) last_cursor: Option<CursorKind>,
}

/// P2b:页面级自由选区的拖选会话(锚点固定,头随指针)。
#[derive(Debug, Clone)]
struct PageDragState {
    /// 锚点元素(稳定选择器)与锚点 char 索引。
    anchor_sel: String,
    anchor_idx: usize,
    /// 起始视口坐标(阈值判定用)。
    start_x: f64,
    start_y: f64,
}

impl HumanSession {
    fn page(&mut self, pid: &str) -> &mut PageInputState {
        self.pages.entry(pid.to_string()).or_default()
    }
}

#[derive(Debug, Clone)]
struct PressState {
    selector: Option<String>,
    x: f64,
    y: f64,
}

#[derive(Debug, Clone)]
struct ClickMark {
    selector: String,
    at_ms: u128,
    /// 连击计数(1=单击,2=双击,3+=三击全选)。
    count: u32,
}

/// P2a:文本控件内按下后的选区拖选会话。
#[derive(Debug, Clone)]
struct TextDragState {
    selector: String,
    /// 选区锚点(按下点;Shift+点击沿用旧锚点)。
    anchor: usize,
}

/// 命中结果(锁内收敛成 owned 数据,锁外零 DOM 句柄)。
#[derive(Debug, Clone)]
struct HitTarget {
    selector: String,
    tag: String,
    typ: String,
    /// 布局盒(页坐标)。
    rect: (f64, f64, f64, f64),
    /// 目标到根的 hover 链(与 Agent 侧 hover 同语义)。
    hover_chain: Vec<String>,
    /// `label[for]` 转发目标 id(点 label 等价于点关联控件)。
    label_for: Option<String>,
    /// 自身或祖先 `position:fixed`(fixed 子树坐标以视口为原点)。
    fixed: bool,
    /// 控件字号(文本光标映射用;非文本控件为 0)。
    font_size: f64,
    /// 光标形状(P2c:悬停时查看器跟随变形)。
    pub(crate) cursor: CursorKind,
}

/// 焦点上下文(锁内收敛)。
struct FocusCtx {
    focused: Option<String>,
    ftag: String,
    ftyp: String,
    text_like: bool,
    editable: bool,
    focusables: Vec<String>,
}

/// 双击判定窗口(毫秒)与位移容差(px)。
///
/// 窗口按**事件到达时刻**计(见 `handle_human_input`);值取 1500 而非浏览器
/// 常见的 500:点击处理链(命中 + 重排 + JS 派发)在重站上可达数百毫秒,
/// 串行投递(自动化用例)与人工事件积压都可能把相邻点击的到达间隔拉长,
/// 500ms 会把正常双击/三击判成单击。
const DBLCLICK_MS: u128 = 1500;
const DBLCLICK_DIST: f64 = 6.0;
/// 单击合成容差:down/up 位移超过此值视为拖拽而非点击。
const CLICK_DIST: f64 = 6.0;
/// 视口滚轮步长(px/档)。
const WHEEL_STEP: f64 = 48.0;
/// 翻页步长比例(Space/PageUp·Down)。
const PAGE_RATIO: f64 = 0.85;
/// JS mousemove 派发节流(毫秒)。
const JS_MOVE_THROTTLE_MS: u128 = 100;
/// agent 态忽略计数的台账落账节奏:第 1 次与其后每 50 次。
const IGNORE_NOTE_EVERY: u64 = 50;
/// 命中第二阶段(node 解析)的候选上限,防大页面退化。
const HIT_RESOLVE_LIMIT: usize = 24;

fn now_ms() -> u128 {
    page_bars::now_ms_public()
}

// ===================== 协议解析(纯函数,单测覆盖) =====================

fn pick_f64(v: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(n) = v.get(*key).and_then(Value::as_f64) {
            if n.is_finite() {
                return Some(n);
            }
        }
    }
    None
}

/// 坐标合理性:有限、量级合理(容许轻微越界,命中阶段自然落空)。
fn sane_pos(x: f64, y: f64) -> bool {
    x.abs() <= 20_000.0 && y.abs() <= 20_000.0
}

/// 解析查看器上报的一条输入事件;非法行返回 None(与 `resize` 同容错口径)。
pub fn parse_event(v: &Value) -> Option<HumanInput> {
    if v.get("cmd")?.as_str()? != "input" {
        return None;
    }
    let act = v.get("act")?.as_str()?.trim().to_ascii_lowercase();
    let ev = match act.as_str() {
        "move" => {
            let (x, y) = (pick_f64(v, &["x"])?, pick_f64(v, &["y"])?);
            if !sane_pos(x, y) {
                return None;
            }
            HumanInput::Move { x, y }
        }
        "down" | "up" => {
            let (x, y) = (pick_f64(v, &["x"])?, pick_f64(v, &["y"])?);
            if !sane_pos(x, y) {
                return None;
            }
            let button = MouseButton::parse(v.get("btn")?.as_str()?)?;
            let mods = Mods::from_json(v.get("mods").unwrap_or(&Value::Null));
            if act == "down" {
                HumanInput::Down { x, y, button, mods }
            } else {
                HumanInput::Up { x, y, button, mods }
            }
        }
        "wheel" => {
            let (x, y) = (pick_f64(v, &["x"])?, pick_f64(v, &["y"])?);
            let (dx, dy) = (pick_f64(v, &["dx"]).unwrap_or(0.0), pick_f64(v, &["dy"])?);
            if !sane_pos(x, y)
                || !dx.is_finite()
                || !dy.is_finite()
                || dx.abs() > 100.0
                || dy.abs() > 100.0
            {
                return None;
            }
            HumanInput::Wheel { x, y, dx, dy }
        }
        "key" => {
            let key = v.get("key")?.as_str()?.trim().to_string();
            // 键名或单字符;超长视为非法(协议噪音)。
            if key.is_empty() || key.chars().count() > 24 {
                return None;
            }
            let mods = Mods::from_json(v.get("mods").unwrap_or(&Value::Null));
            HumanInput::Key { key, mods }
        }
        _ => return None,
    };
    Some(ev)
}

/// 键名归一化:查看器侧已映射平台键码,这里统一大小写与别名。
/// 单字符键保留原样(Shift 大小写由本模块按 mods 补正)。
pub fn normalize_key(raw: &str) -> String {
    // 空格键以原样单空格到达;trim 会把它吃掉,先特判。
    if raw == " " {
        return "Space".to_string();
    }
    let k = raw.trim();
    match k.to_ascii_lowercase().as_str() {
        "enter" | "return" => "Enter".to_string(),
        "tab" => "Tab".to_string(),
        "escape" | "esc" => "Escape".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" | "del" => "Delete".to_string(),
        "arrowleft" | "left" => "ArrowLeft".to_string(),
        "arrowright" | "right" => "ArrowRight".to_string(),
        "arrowup" | "up" => "ArrowUp".to_string(),
        "arrowdown" | "down" => "ArrowDown".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        "space" => "Space".to_string(),
        _ => k.to_string(),
    }
}

// ===================== 宿主输入管线 =====================

impl LsmBrowserManager {
    /// 人类输入统一入口(查看器命令通道 / 测试直调)。
    ///
    /// 返回小结 JSON(测试断言与日志用;查看器不消费回执)。
    pub async fn handle_human_input(&self, page_id: &str, ev: HumanInput) -> Value {
        // 事件到达时刻:连击窗口按到达时刻计,不被命中/重排等处理延迟吃掉
        //(人工连点与自动化用例同受益)。
        let arrived = now_ms();
        if !enabled() {
            return json!({"ignored": true, "reason": "LAEW_HUMAN_INPUT=off"});
        }
        // 页面不在场:静默忽略(查看器关闭竞态)。
        {
            let inner = self.inner.lock().await;
            if !inner.tabs.contains_key(page_id) {
                return json!({"ignored": true, "reason": "page_gone"});
            }
        }
        // 控制权闸门:agent 态(蒙层在位)人工输入一律忽略;状态机关闭时放行
        //(回到「人工可直接操作」的旧语义,与 LAEW_WEB_CONTROL=off 口径一致)。
        if web_control::enabled() && !web_control::is_user_control(Engine::Lsm, page_id) {
            let count = {
                let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                s.ignored = s.ignored.saturating_add(1);
                s.ignored
            };
            // 落进度栏事件时间线(第 1 次与其后每 50 次;1Hz 心跳自然带出)。
            if count == 1 || count % IGNORE_NOTE_EVERY == 0 {
                let mut inner = self.inner.lock().await;
                if let Some(tab) = inner.tabs.get_mut(page_id) {
                    tab.bars.push_event(page_bars::BarEvent {
                        t: now_ms(),
                        label: format!("人工输入被忽略(Agent 控制中,第 {count} 次)"),
                        ok: false,
                        ms: 0,
                    });
                }
            }
            return json!({"ignored": true, "reason": "agent_control"});
        }
        match ev {
            HumanInput::Move { x, y } => self.human_move(page_id, x, y).await,
            HumanInput::Down { x, y, button, mods } => {
                self.human_down(page_id, x, y, button, mods).await
            }
            HumanInput::Up { x, y, button, mods } => {
                self.human_up(page_id, x, y, button, mods, arrived).await
            }
            HumanInput::Wheel { x, y, dx, dy } => self.human_wheel(page_id, x, y, dx, dy).await,
            HumanInput::Key { key, mods } => {
                let key = normalize_key(&key);
                self.human_key(page_id, &key, &mods).await
            }
        }
    }

    // ---------- 运行时值 / 光标映射 / 视口滚动(鼠标与键盘共用) ----------

    /// 写运行时表单值(form_values)。
    async fn set_runtime_value(&self, pid: &str, selector: &str, value: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(tab) = inner.tabs.get_mut(pid) {
            tab.form_values.insert(selector.to_string(), value.to_string());
        }
    }

    /// 当前运行时值(form_values 优先,缺省回落 HTML value 属性)。
    async fn current_value(&self, pid: &str, selector: &str) -> String {
        let inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get(pid) else {
            return String::new();
        };
        if let Some(v) = tab.form_values.get(selector) {
            return v.clone();
        }
        let doc = tab.document_with_state();
        doc.select(selector)
            .ok()
            .and_then(|v| v.into_iter().next())
            .and_then(|n| document::attr(&n, "value"))
            .unwrap_or_default()
    }

    /// P2a:点击 → 文本控件内 char 索引(与控件文本绘制同源的字体度量;
    /// fixed 子树免补滚动;password 按等宽子弹串映射,索引与值 1:1)。
    async fn text_caret_at(&self, pid: &str, hit: &HitTarget, x: f64, y: f64) -> Option<usize> {
        let inner = self.inner.lock().await;
        let tab = inner.tabs.get(pid)?;
        if hit.font_size <= 0.0 {
            return None;
        }
        let doc = tab.document_with_state();
        let mut value = tab
            .form_values
            .get(&hit.selector)
            .cloned()
            .or_else(|| {
                doc.select(&hit.selector)
                    .ok()
                    .and_then(|v| v.into_iter().next())
                    .and_then(|n| {
                        if hit.tag == "textarea" {
                            Some(
                                document::attr(&n, "value")
                                    .unwrap_or_else(|| n.text_contents()),
                            )
                        } else {
                            document::attr(&n, "value")
                        }
                    })
            })
            .unwrap_or_default();
        if hit.tag == "textarea" {
            value = value.replace("\r\n", "\n");
        }
        let display = if hit.typ == "password" {
            "•".repeat(value.chars().count())
        } else {
            value
        };
        let size = hit.font_size;
        let x_rel = (if hit.fixed { x - hit.rect.0 } else { x + tab.scroll_x - hit.rect.0 }) - 3.0;
        if hit.tag == "textarea" {
            let y_rel = (if hit.fixed { y - hit.rect.1 } else { y + tab.scroll_y - hit.rect.1 }) - 2.0;
            let rows = self.font.wrap_text_rows(&display, size, (hit.rect.2 - 6.0).max(1.0), usize::MAX);
            Some(text_edit::index_from_rows(
                &self.font,
                &rows,
                size,
                x_rel,
                y_rel,
                size * 1.35,
            ))
        } else {
            let spans = self.font.char_spans(&display, size, (hit.rect.2 - 6.0).max(0.0));
            Some(text_edit::index_from_spans(&spans, x_rel))
        }
    }

    /// 模态 dialog 遮罩点击关闭:命中元素不在 dialog 子树内时关掉顶层模态框。
    ///
    /// 与 `api::act_key_press` 的 Esc 关闭同一条落点(移除 `open` +
    /// `data-lsm-modal`),保证「点遮罩」与「按 Esc」语义一致。
    /// 当前页是否存在**模态** dialog(`open` + `data-lsm-modal`;遮罩判定用)。
    async fn has_modal_dialog(&self, pid: &str) -> bool {
        let inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get(pid) else {
            return false;
        };
        let doc = tab.document_with_state();
        dialog_selectors(&doc).into_iter().next().is_some()
    }

    async fn try_close_modal_backdrop(&self, pid: &str) -> bool {
        // 顶层模态 dialog(文档序**最后一个** open 且带 `data-lsm-modal`;
        // 与 `close_top_dialog` 同口径)。
        let target = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return false;
            };
            let doc = tab.document_with_state();
            dialog_selectors(&doc).into_iter().next()
        };
        let Some(sel) = target else {
            return false;
        };
        // 走与页面脚本 `dialog.close()` 相同的 DOM 动作路径。
        let actions = vec![
            script::JsAction {
                kind: "remove_attr".to_string(),
                selector: sel.clone(),
                key: "open".to_string(),
                value: String::new(),
                url: String::new(),
                event: String::new(),
            },
            script::JsAction {
                kind: "remove_attr".to_string(),
                selector: sel.clone(),
                key: "data-lsm-modal".to_string(),
                value: String::new(),
                url: String::new(),
                event: String::new(),
            },
        ];
        let (html, url) = {
            let inner = self.inner.lock().await;
            match inner.tabs.get(pid) {
                Some(t) => (t.html.clone(), t.url.clone()),
                None => return false,
            }
        };
        let (final_html, _) = api::apply_dom_actions(&html, &url, &actions);
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                tab.set_html(final_html);
            }
        }
        true
    }

    /// P2b:页面自由选区的**元素可见文本** char 索引映射(跨元素手势的索引空间)。
    ///
    /// 与 [`text_caret_at`] 的差异:那里的索引空间是控件的 `value`(单值字符串),
    /// 这里是元素**可见文本**(去标签、折叠空白)——跨元素拼接时才不会错位。
    /// 几何与控件内选区同源(`FontSet::char_spans`)。
    async fn page_text_caret_at(&self, pid: &str, hit: &HitTarget, x: f64, y: f64) -> Option<usize> {
        let (text, size, rect, fixed, scroll_x, scroll_y) = {
            let inner = self.inner.lock().await;
            let tab = inner.tabs.get(pid)?;
            let doc = tab.document_with_state();
            let node = doc.select(&hit.selector).ok().and_then(|v| v.into_iter().next())?;
            let text = document::node_text(&node);
            (
                text,
                node_font_size(
                    &StyleEngine::from_document(
                        &doc,
                        MediaCtx::new(tab.viewport.width as f64, tab.viewport.height as f64),
                    ),
                    &node,
                ),
                hit.rect,
                hit.fixed,
                tab.scroll_x,
                tab.scroll_y,
            )
        };
        if text.is_empty() || size <= 0.0 {
            return None;
        }
        let x_rel = (if fixed { x - rect.0 } else { x + scroll_x - rect.0 }) - 3.0;
        // 页面文本按**单行**近似取最近字符边界(跨元素拖选不折行;多行块元素
        // 的精确行定位留后续阶段,见方案 §7.2)。
        let spans = self.font.char_spans(&text, size, f64::MAX);
        Some(text_edit::index_from_spans(&spans, x_rel))
    }

    /// P2a:落点击光标(Shift+点击 = 沿用旧锚点扩选),返回新光标态。
    async fn place_text_caret(
        &self,
        pid: &str,
        hit: &HitTarget,
        x: f64,
        y: f64,
        extend: bool,
    ) -> Option<CaretState> {
        let idx = self.text_caret_at(pid, hit, x, y).await?;
        let mut inner = self.inner.lock().await;
        let tab = inner.tabs.get_mut(pid)?;
        let st = if extend {
            CaretState {
                pos: idx,
                anchor: tab.form_caret.get(&hit.selector).map(|c| c.anchor).unwrap_or(idx),
            }
        } else {
            CaretState::collapsed(idx)
        };
        tab.form_caret.insert(hit.selector.clone(), st);
        Some(st)
    }

    /// 视口滚动(offset 为增量 px;dy 正 = 向上)。
    async fn scroll_viewport_by(&self, pid: &str, dx: f64, dy: f64) -> Value {
        let mut inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get_mut(pid) else {
            return json!({"ignored": true});
        };
        tab.scroll_x = (tab.scroll_x + dx).max(0.0);
        tab.scroll_y = (tab.scroll_y + dy).max(0.0);
        let (sx, sy) = (tab.scroll_x, tab.scroll_y);
        drop(inner);
        self.push_frame_if_headed(pid).await;
        json!({"scrollX": sx, "scrollY": sy})
    }

    /// 视口滚动到绝对位置。
    async fn scroll_viewport_to(&self, pid: &str, x: f64, y: f64) -> Value {
        let mut inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get_mut(pid) else {
            return json!({"ignored": true});
        };
        tab.scroll_x = x.max(0.0);
        tab.scroll_y = y.max(0.0);
        drop(inner);
        self.push_frame_if_headed(pid).await;
        json!({"scrollX": x, "scrollY": y})
    }

    /// input[type=range] 点击/拖拽:按视口 x 在轨道矩形内的比例设值(吸附 step)。
    /// `rect` 为命中测试带出的布局盒(页坐标)。
    async fn set_range_by_x(&self, pid: &str, selector: &str, rect: (f64, f64, f64, f64), x: f64) {
        let computed = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return;
            };
            let doc = tab.document_with_state();
            let Some(node) = doc.select(selector).ok().and_then(|v| v.into_iter().next())
            else {
                return;
            };
            let (min, max, step) = range_params(&node);
            (min, max, step, tab.scroll_x)
        };
        let (min, max, step, scroll_x) = computed;
        if rect.2 <= 0.0 {
            return;
        }
        // rect 是页坐标,人工输入是视口坐标:补回滚动量(滑块通常在常规流)。
        let ratio = ((x + scroll_x - rect.0) / rect.2).clamp(0.0, 1.0);
        let raw = min + ratio * (max - min);
        let snapped = if step > 0.0 {
            ((raw - min) / step).round() * step + min
        } else {
            raw
        };
        let val = format_num(snapped.clamp(min.min(max), max.max(min)));
        self.set_runtime_value(pid, selector, &val).await;
        if let Some(reply) = api::js_dispatch_bounded(self, pid, selector, "input").await {
            api::apply_js_reply(self, pid, &reply).await;
        }
    }

    // ---------- 查询小工具(锁内短临界区) ----------

    async fn attr_of(&self, pid: &str, selector: &str, name: &str) -> Option<String> {
        let inner = self.inner.lock().await;
        let tab = inner.tabs.get(pid)?;
        let doc = tab.document_with_state();
        doc.select(selector)
            .ok()
            .and_then(|v| v.into_iter().next())
            .and_then(|n| document::attr(&n, name))
    }

    async fn resolve_page_url(&self, pid: &str, href: &str) -> Option<String> {
        let inner = self.inner.lock().await;
        let tab = inner.tabs.get(pid)?;
        super::urlutil::resolve(&tab.url, href)
    }

    async fn selector_by_id(&self, pid: &str, id: &str) -> Option<String> {
        let inner = self.inner.lock().await;
        let tab = inner.tabs.get(pid)?;
        let doc = tab.document_with_state();
        doc.select(&format!("#{id}"))
            .ok()
            .and_then(|v| v.into_iter().next())
            .map(|n| Document::stable_selector(&n))
    }

    /// P2d:option 是否属于 `select[multiple]`(列表多选的人工修饰键语义判定)。
    pub(super) async fn option_in_multi_select(&self, pid: &str, option_sel: &str) -> bool {
        let inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get(pid) else {
            return false;
        };
        let doc = tab.document_with_state();
        let Some(opt) = doc.select(option_sel).ok().and_then(|v| v.into_iter().next()) else {
            return false;
        };
        let mut cur = opt.parent();
        while let Some(n) = cur {
            if document::tag_name(&n).eq_ignore_ascii_case("select") {
                return document::attr(&n, "multiple").is_some();
            }
            cur = n.parent();
        }
        false
    }

    async fn push_frame_if_headed(&self, pid: &str) {
        {
            // 查看器不在位(hidden/已退出)时无需推帧。
            let alive = match self.view.lock() {
                Ok(g) => g.as_ref().map(|s| s.is_alive()).unwrap_or(false),
                Err(_) => false,
            };
            if !alive {
                return;
            }
        }
        let inner = self.inner.lock().await;
        if let Some(tab) = inner.tabs.get(pid) {
            if tab.mode.is_headed() {
                self.push_frame(tab);
            }
        }
    }

    /// P2c:上报光标形状给查看器(仅在变化时发帧,避免每 move 一帧)。
    async fn push_cursor_kind(&self, pid: &str, kind: CursorKind) {
        let changed = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            if st.last_cursor == Some(kind) {
                false
            } else {
                st.last_cursor = Some(kind);
                true
            }
        };
        if !changed {
            return;
        }
        if let Ok(guard) = self.view.lock() {
            if let Some(sink) = guard.as_ref() {
                sink.send_cursor(kind.as_str());
            }
        }
    }

    /// 人类输入落进度栏事件时间线(task_local 台账只覆盖 Agent 任务树,
    /// 人工输入发生在树外,直接写 `TabState.bars`)。best-effort:锁被占时
    /// 跳过本条,不阻塞输入路径。
    fn note_human_event(&self, pid: &str, label: &str) {
        if let Ok(mut inner) = self.inner.try_lock() {
            if let Some(tab) = inner.tabs.get_mut(pid) {
                tab.bars.push_event(page_bars::BarEvent {
                    t: now_ms(),
                    label: label.to_string(),
                    ok: true,
                    ms: 0,
                });
            }
        }
    }

    /// 坐标命中:视口坐标 → 元素(39 号设计文档 §2.2)。
    ///
    /// 双假设(fixed 子树按视口坐标,常规流按 视口+scroll 页坐标)→ 候选
    /// 按面积升序解析(顶层浮层优先)→ 首个通过 `is_rendered` + fixed
    /// 假设校验的候选胜出。近似边界:不做祖先 overflow 裁剪交集与
    /// transform 逆映射。
    async fn human_hit_test(&self, pid: &str, vx: f64, vy: f64) -> Option<HitTarget> {
        let (doc, engine, geo, open_panels) = {
            let inner = self.inner.lock().await;
            let tab = inner.tabs.get(pid)?;
            let (sx, sy) = (tab.scroll_x, tab.scroll_y);
            let open_panels: Vec<String> = tab.widget_open.keys().cloned().collect();
            let doc = tab.document_with_state();
            let engine = StyleEngine::from_document(
                &doc,
                MediaCtx::new(tab.viewport.width as f64, tab.viewport.height as f64),
            );
            // 第一阶段:纯几何筛(不碰 DOM),按面积升序。
            let mut geo: Vec<(layout::ElementBox, bool)> = Vec::new(); // (box, fixed 假设)
            for b in &tab.layout.boxes {
                if !b.visible || b.w <= 0.0 || b.h <= 0.0 {
                    continue;
                }
                if contains(b, vx, vy) {
                    geo.push((b.clone(), true));
                }
                if contains(b, vx + sx, vy + sy) {
                    geo.push((b.clone(), false));
                }
            }
            geo.sort_by(|a, b| {
                let area_a = a.0.w * a.0.h;
                let area_b = b.0.w * b.0.h;
                area_a
                    .partial_cmp(&area_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.0.z_index.cmp(&a.0.z_index))
            });
            (doc, engine, geo, open_panels)
        };
        // 第二阶段:解析节点(限候选上限,防大页面退化)。
        let mut resolved: Vec<HitTarget> = Vec::new();
        for (b, fixed_guess) in geo.iter().take(HIT_RESOLVE_LIMIT) {
            let Some(node) = doc.select(&b.selector).ok().and_then(|v| v.into_iter().next())
            else {
                continue;
            };
            let tag = document::tag_name(&node);
            // P2d:`option` 的 UA 默认 display:none(不参与常规流,由控件
            // 内部浮层/列表形态发射真实盒)——盒子存在即代表可见,不能因
            // 样式层的 none 而跳过,否则 select 列表/下拉面板/datalist 的
            // 人工点击永远落不到 option 上。
            if tag != "option" && !engine.is_rendered(&node) {
                continue;
            }
            let fixed = node_under_fixed(&engine, &node);
            if fixed != *fixed_guess {
                continue;
            }
            let typ = document::attr(&node, "type")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let label_for = if tag == "label" {
                document::attr(&node, "for").filter(|s| !s.trim().is_empty())
            } else {
                None
            };
            let font_size = if is_text_like(&tag, &typ) {
                node_font_size(&engine, &node)
            } else {
                0.0
            };
            let cursor = cursor_kind_for_node(&node, &tag, &typ);
            resolved.push(HitTarget {
                selector: b.selector.clone(),
                tag,
                typ,
                rect: (b.x, b.y, b.w, b.h),
                hover_chain: api::hover_chain_selectors(&node),
                label_for,
                fixed,
                font_size,
                cursor,
            });
            if resolved.len() >= 6 {
                break;
            }
        }
        pick_top_layer(&doc, &open_panels, resolved)
    }
}

// ===================== 纯函数辅助(单测覆盖) =====================

fn contains(b: &layout::ElementBox, x: f64, y: f64) -> bool {
    x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h
}

/// range 的 min/max/step 解析(缺省 0/100/1)。
fn range_params(node: &kuchikiki::NodeRef) -> (f64, f64, f64) {
    let min = document::attr(node, "min").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let max = document::attr(node, "max").and_then(|v| v.parse::<f64>().ok()).unwrap_or(100.0);
    let step = document::attr(node, "step")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(1.0)
        .abs();
    (min, max, step)
}

/// P2d:select[multiple] 的人工点击语义(纯函数,单测覆盖)。
///
/// * 普通点击 = 独占选中 `idx` 项;
/// * `Ctrl`(含 Cmd)= 翻转 `idx` 项(保留其余);
/// * `Shift` = 从锚点(上次普通/Ctrl 点击项;无锚点按独占)到 `idx` 闭区间。
///
/// 返回重算后的选中值集合(**按 `all` 的 DOM 序**,供 CSV 拼接与
/// `select.value` = 首个选中项的浏览器语义)。
pub fn multi_pick(
    all: &[String],
    current: &[String],
    idx: usize,
    ctrl: bool,
    shift: bool,
    anchor: Option<usize>,
) -> Vec<String> {
    let contains = |v: &str| current.iter().any(|c| c == v);
    let picked = if ctrl {
        // 翻转:已在集合 → 移除;不在 → 追加。
        let target = all.get(idx).map(String::as_str).unwrap_or("");
        if contains(target) {
            current
                .iter()
                .filter(|c| c.as_str() != target)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            let mut out = current.to_vec();
            out.push(target.to_string());
            out
        }
    } else if shift {
        match anchor {
            Some(a) => {
                let (lo, hi) = if a <= idx { (a, idx) } else { (idx, a) };
                all[lo..=hi.min(all.len().saturating_sub(1))].to_vec()
            }
            // 无锚点:退化为独占。
            None => all.get(idx).cloned().into_iter().collect(),
        }
    } else {
        // 独占。
        all.get(idx).cloned().into_iter().collect()
    };
    // 按全集 DOM 序重排(去重),保证 CSV 顺序稳定。
    all.iter()
        .filter(|v| picked.iter().any(|p| p == *v))
        .cloned()
        .collect()
}

/// 节点自身或任一祖先 `position:fixed`(fixed 子树坐标以视口为原点)。
fn node_under_fixed(engine: &StyleEngine, node: &kuchikiki::NodeRef) -> bool {
    let mut chain: Vec<kuchikiki::NodeRef> = Vec::new();
    let mut cur = Some(node.clone());
    while let Some(n) = cur {
        if n.as_element().is_some() {
            chain.push(n.clone());
        }
        cur = n.parent();
    }
    // 从根向下计算,任一层 fixed 即整棵子树按视口坐标布局。
    let mut st = engine.initial_style();
    for n in chain.iter().rev() {
        st = engine.computed(n, &st);
        if st.pos.position == "fixed" {
            return true;
        }
    }
    false
}

/// 顶层浮层优先:候选中属「open dialog 子树 / 展开 select 面板(option 或
/// 展开中的 select 自身)」的压过面积更小的常规流元素;否则取面积最小者
///(`resolved` 已按面积升序解析)。
fn pick_top_layer(
    doc: &Document,
    open_panels: &[String],
    resolved: Vec<HitTarget>,
) -> Option<HitTarget> {
    let mut smallest: Option<HitTarget> = None;
    for h in resolved {
        let top = h.tag == "option" // option 只在面板展开/列表形态时有盒,天然顶层。
            || open_panels.iter().any(|p| *p == h.selector)
            || in_open_dialog(doc, &h.selector);
        if top {
            return Some(h);
        }
        if smallest.is_none() {
            smallest = Some(h);
        }
    }
    smallest
}

pub(super) fn in_open_dialog(doc: &Document, selector: &str) -> bool {
    doc.select(selector)
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|n| {
            let mut cur = n.parent();
            while let Some(p) = cur {
                if document::tag_name(&p) == "dialog" && document::attr(&p, "open").is_some() {
                    return true;
                }
                cur = p.parent();
            }
            false
        })
        .unwrap_or(false)
}

/// P2b:元素是否可参与**页面级自由选区**(跨元素拖选)。
///
/// 判据:有可见文本、非交互控件(控件内选区归 `text_drag` 管)、非 `user-select:none`。
/// 标签白名单近似(块级文本容器 + 行内文本),避免把 `<html>`/`<body>`/`<div>` 布局容器
/// 当成选区单元(它们的文本是子代之和,会导致索引空间与渲染错位)。
fn is_page_selectable(tag: &str, typ: &str) -> bool {
    let _ = typ;
    matches!(
        tag,
        "p" | "span"
            | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            | "li" | "td" | "th" | "caption" | "label" | "a" | "b" | "i" | "em"
            | "strong" | "small" | "code" | "pre" | "blockquote" | "dd" | "dt"
            | "figcaption" | "legend" | "summary" | "option"
    )
}

/// 元素链上是否显式 `user-select:none`(页面自由选区要跳过)。
///
/// 样式引擎**未建模** `user-select`(见 style/values.rs),这里只读**内联
/// `style`** 与 `selectstart` 的常见站点写法 —— 保守口径:读不到就放行,
/// 与既有命中近似的边界一致(宁可多选,别让拖选完全不可用)。
fn user_select_none(engine: &StyleEngine, node: &kuchikiki::NodeRef) -> bool {
    let _ = engine;
    let mut cur = Some(node.clone());
    let mut guard = 0;
    while let Some(n) = cur {
        guard += 1;
        if guard > 64 {
            return false;
        }
        if let Some(el) = n.as_element() {
            let attrs = el.attributes.borrow();
            for key in ["style", "unselectable"] {
                if let Some(v) = attrs.get(key) {
                    let v = v.to_ascii_lowercase();
                    if v.contains("user-select") && v.contains("none")
                        || v.eq_ignore_ascii_case("on")
                    {
                        return true;
                    }
                }
            }
        }
        cur = n.parent();
    }
    false
}

/// 页面上所有**模态** dialog 的稳定选择器(文档序;最后一个 = 顶层)。
pub(super) fn dialog_selectors(doc: &Document) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Ok(dialogs) = doc.select("dialog") {
        for d in dialogs {
            if document::attr(&d, "open").is_some()
                && document::attr(&d, "data-lsm-modal").is_some()
            {
                out.push(Document::stable_selector(&d));
            }
        }
    }
    out
}

/// 命中元素是否位于**打开的** `<dialog>` 子树内(遮罩点击判定的辅助)。
async fn in_open_dialog_hit(mgr: &LsmBrowserManager, pid: &str, selector: &str) -> bool {
    let inner = mgr.inner.lock().await;
    let Some(tab) = inner.tabs.get(pid) else {
        return false;
    };
    let doc = tab.document_with_state();
    in_open_dialog(&doc, selector)
}

/// 控件类标签(disabled 属性对其生效的集合;与 `click_inner` 的判定同口径)。
fn is_disabled_control(tag: &str, _typ: &str) -> bool {
    matches!(tag, "input" | "button" | "select" | "textarea" | "option")
}

/// 节点的 `disabled` 属性是否存在(锁内短临界区调用)。
fn control_disabled(tab: &TabState, selector: &str) -> bool {
    let doc = tab.document_with_state();
    doc.select(selector)
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|n| document::attr(&n, "disabled").is_some())
        .unwrap_or(false)
}

/// 元素是否可聚焦(mousedown 聚焦语义;与 Tab 循环同判定)。
fn is_focusable(tag: &str, typ: &str) -> bool {
    match tag {
        "a" => true,
        "input" => typ != "hidden",
        "select" | "textarea" | "button" | "summary" => true,
        _ => false,
    }
}

/// 文本类控件判定(input 文本系 / textarea;光标/选区/编辑语义的适用面,
/// 与 `keyboard::collect_text_ctx` 同口径)。
pub(crate) fn is_text_like(tag: &str, typ: &str) -> bool {
    matches!(tag, "input" | "textarea")
        && (tag == "textarea"
            || !matches!(
                typ,
                "checkbox" | "radio" | "range" | "file" | "button" | "submit" | "reset"
                    | "image" | "hidden" | "color" | "date" | "time" | "datetime-local"
                    | "month" | "week"
            ))
}

/// 节点计算字号(从根下推继承链;文本光标映射与键路由折行共用)。
pub(super) fn node_font_size(engine: &StyleEngine, node: &kuchikiki::NodeRef) -> f64 {
    let mut chain: Vec<kuchikiki::NodeRef> = Vec::new();
    let mut cur = Some(node.clone());
    while let Some(n) = cur {
        if n.as_element().is_some() {
            chain.push(n.clone());
        }
        cur = n.parent();
    }
    let mut st = engine.initial_style();
    for n in chain.iter().rev() {
        st = engine.computed(n, &st);
    }
    st.font_size.max(8.0)
}

/// 文档序可聚焦选择器列表(有布局盒才算;tabindex 排序未建模,按文档序近似)。
pub(super) fn focusable_selectors(tab: &TabState, doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(all) = doc.select("*") {
        for node in all {
            let tag = document::tag_name(&node);
            let typ = document::attr(&node, "type").unwrap_or_default().to_ascii_lowercase();
            let focusable =
                is_focusable(&tag, &typ) || document::attr(&node, "tabindex").is_some();
            if !focusable || document::attr(&node, "disabled").is_some() {
                continue;
            }
            if tag == "input" && matches!(typ.as_str(), "hidden" | "file") {
                continue;
            }
            if tag == "a" && document::attr(&node, "href").is_none() {
                continue;
            }
            let sel = Document::stable_selector(&node);
            if tab.layout.rect_for(&sel).is_some() {
                out.push(sel);
            }
        }
    }
    out
}

/// Tab 循环的下一个聚焦目标。
fn next_focusable(list: &[String], current: Option<&str>, forward: bool) -> Option<String> {
    if list.is_empty() {
        return None;
    }
    let idx = current.and_then(|c| list.iter().position(|s| s == c));
    let next = match (idx, forward) {
        (Some(i), true) => (i + 1) % list.len(),
        (Some(0), false) => list.len() - 1,
        (Some(i), false) => i - 1,
        (None, true) => 0,
        (None, false) => list.len() - 1,
    };
    Some(list[next].clone())
}

/// Shift + 字符的大小写补正(查看器送 charactersIgnoringModifiers)。
fn shift_char(c: char) -> char {
    if c.is_ascii_lowercase() {
        c.to_ascii_uppercase()
    } else {
        c
    }
}

/// 数值格式化:整数不带小数点,浮点保留至多 3 位。
fn format_num(v: f64) -> String {
    if (v - v.round()).abs() < f64::EPSILON {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.3}")
    }
}

// ---------- 事件载荷(39 号设计文档阶段二) ----------

/// 鼠标键 → DOM `button` 值(0=左 1=中 2=右)。
fn dom_button(b: MouseButton) -> i64 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

/// 鼠标事件载荷(clientX/Y 视口坐标、pageX/Y 页坐标、button/buttons/detail)。
fn mouse_payload(x: f64, y: f64, button: MouseButton, pressed: bool, detail: i64) -> Value {
    json!({
        "clientX": x.round(), "clientY": y.round(),
        "screenX": x.round(), "screenY": y.round(),
        "button": dom_button(button),
        "buttons": if pressed { 1 } else { 0 },
        "detail": detail,
    })
}

/// DOM `KeyboardEvent.key`(命名键与 DOM 规范一致;可打印字符为产出的字符)。
fn js_key_name(key: &str, mods: &Mods) -> String {
    if key == "Space" {
        return " ".to_string();
    }
    if key.chars().count() == 1 && mods.shift {
        return shift_char(key.chars().next().unwrap()).to_string();
    }
    key.to_string()
}

/// 遗留 `keyCode` 近似(命名键查表;字母取大写 ASCII;数字 48-57)。
pub(crate) fn legacy_key_code(key: &str) -> i64 {
    match key {
        "Enter" => 13,
        "Tab" => 9,
        "Escape" => 27,
        "Backspace" => 8,
        "Delete" => 46,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        "Home" => 36,
        "End" => 35,
        "PageUp" => 33,
        "PageDown" => 34,
        "Space" | " " => 32,
        _ => {
            let c = key.chars().next().unwrap_or('\0');
            if c.is_ascii_alphabetic() {
                c.to_ascii_uppercase() as i64
            } else if c.is_ascii_digit() {
                c as i64
            } else {
                0
            }
        }
    }
}

/// 键盘事件载荷(e.key/keyCode/which/charCode + 修饰键布尔)。
fn key_payload(key: &str, mods: &Mods) -> Value {
    let name = js_key_name(key, mods);
    let code = legacy_key_code(key);
    let printable = name.chars().count() == 1;
    json!({
        "key": name,
        "keyCode": code,
        "which": code,
        "charCode": if printable { code } else { 0 },
        "shiftKey": mods.shift,
        "ctrlKey": mods.ctrl,
        "altKey": mods.alt,
        "metaKey": mods.meta,
        "repeat": false,
    })
}

/// 剪贴板操作(⌘/Ctrl+C/V/X;查看器右键菜单同路径)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipOp {
    Copy,
    Cut,
    Paste,
}

/// 由按键+修饰键判定剪贴板操作(ctrl/meta + c/v/x)。
pub fn clip_op_for(key: &str, mods: &Mods) -> Option<ClipOp> {
    if !(mods.ctrl || mods.meta) {
        return None;
    }
    match key.to_ascii_lowercase().as_str() {
        "c" => Some(ClipOp::Copy),
        "x" => Some(ClipOp::Cut),
        "v" => Some(ClipOp::Paste),
        _ => None,
    }
}

fn op_name(op: ClipOp) -> &'static str {
    match op {
        ClipOp::Copy => "copy",
        ClipOp::Cut => "cut",
        ClipOp::Paste => "paste",
    }
}
