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
use super::layout;
use super::style::{MediaCtx, StyleEngine};
use super::{page_bars, LsmBrowserManager, TabState};
use crate::agent::web_control::{self, Engine};

pub mod clipboard;
pub mod text_edit;

mod keyboard;
pub use keyboard::{edit_op_for, EditOp};

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
    fn combo(&self) -> bool {
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
    /// P2a:单控件撤销/重做历史(key = stable selector)。
    edits: std::collections::HashMap<String, text_edit::EditHistory>,
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

    /// 指针移动:hover 链更新(目标变化才重排)+ HTML5 drag 近似 +
    /// JS mousemove 节流派发(带坐标载荷)。
    async fn human_move(&self, pid: &str, x: f64, y: f64) -> Value {
        let hit = self.human_hit_test(pid, x, y).await;
        let hit_sel = hit.as_ref().map(|h| h.selector.clone());
        // HTML5 drag 近似:按下中且位移超阈值 → dragstart 一次,后续 dragover。
        // 无 dataTransfer 模型(页面拿不到拖拽数据),只保证事件序列与坐标。
        let drag_evt: Option<(String, String, bool)> = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            match (&st.press, &hit) {
                // P2a:文本控件内拖动是选区拖选,不触发 HTML5 drag。
                (Some(p), Some(h))
                    if p.selector.is_some()
                        && st.text_drag.is_none()
                        && (p.x - x).hypot(p.y - y) > CLICK_DIST =>
                {
                    let src = p.selector.clone().unwrap_or_default();
                    if !st.drag_started {
                        st.drag_started = true;
                        Some(("dragstart".to_string(), src, true))
                    } else {
                        Some(("dragover".to_string(), h.selector.clone(), false))
                    }
                }
                _ => None,
            }
        };
        if let Some((ev, sel, immediate)) = drag_evt {
            let dispatch = if immediate {
                api::js_dispatch_bounded_with(
                    self, pid, &sel, &ev,
                    Some(mouse_payload(x, y, MouseButton::Left, true, 0)),
                )
                .await
            } else {
                let due = {
                    let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                    let st = s.page(pid);
                    let now = now_ms();
                    if now.saturating_sub(st.last_js_move_ms) >= JS_MOVE_THROTTLE_MS {
                        st.last_js_move_ms = now;
                        true
                    } else {
                        false
                    }
                };
                if due {
                    api::js_dispatch_bounded_with(
                        self, pid, &sel, &ev,
                        Some(mouse_payload(x, y, MouseButton::Left, true, 0)),
                    )
                    .await
                } else {
                    None
                }
            };
            if let Some(reply) = dispatch {
                api::apply_js_reply(self, pid, &reply).await;
            }
        }
        // P2a:文本控件内拖动 = 选区拖选(锚点固定、头随指针;浏览器语义)。
        let text_drag = {
            let s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            s.pages.get(pid).and_then(|st| st.text_drag.clone())
        };
        if let Some(td) = text_drag {
            let pressed = {
                let s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                s.pages.get(pid).map(|st| st.press.is_some()).unwrap_or(false)
            };
            if pressed {
                if let Some(h) = hit.as_ref().filter(|h| h.selector == td.selector) {
                    if let Some(idx) = self.text_caret_at(pid, h, x, y).await {
                        let changed = {
                            let mut inner = self.inner.lock().await;
                            match inner.tabs.get_mut(pid).and_then(|t| t.form_caret.get_mut(&td.selector)) {
                                Some(st) if st.pos != idx => {
                                    st.pos = idx;
                                    true
                                }
                                _ => false,
                            }
                        };
                        if changed {
                            {
                                let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                                s.page(pid).text_dragging = true;
                            }
                            api::relayout_and_push(self, pid).await;
                        }
                    }
                }
            }
        }
        // 拖拽中的滑块:按住移动持续设值(浏览器拖拽滑块语义)。
        let press_range = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            match (&st.press, &hit) {
                (Some(p), Some(h))
                    if p.selector.as_deref() == Some(h.selector.as_str())
                        && h.tag == "input"
                        && h.typ == "range" =>
                {
                    Some((h.selector.clone(), h.rect))
                }
                _ => None,
            }
        };
        if let Some((sel, rect)) = &press_range {
            self.set_range_by_x(pid, sel, *rect, x).await;
            api::relayout_and_push(self, pid).await;
        }
        let mut focused = None;
        let changed = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            let changed = st.hover.as_deref() != hit_sel.as_deref();
            if changed {
                st.hover = hit_sel.clone();
            }
            changed
        };
        if changed {
            // 锁块收窄:relayout_and_push 内部会再锁 inner(tokio Mutex 非重入,
            // 持锁调用会自死锁 —— 拖拽测试首轮即命中,修复于阶段二)。
            {
                let mut inner = self.inner.lock().await;
                if let Some(tab) = inner.tabs.get_mut(pid) {
                    tab.mouse = Some((x, y));
                    focused = tab.focused.clone();
                    match &hit {
                        Some(h) => api::set_hover_chain(tab, &h.hover_chain),
                        None => api::set_pseudo_state(tab, "", api::PseudoDim::Hover, false),
                    }
                }
            }
            api::relayout_and_push(self, pid).await;
        }
        // JS mousemove:命中目标(无命中时聚焦元素)节流派发,页面 hover 菜单可用。
        if focused.is_none() {
            let inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get(pid) {
                focused = tab.focused.clone();
            }
        }
        let pressed = {
            let s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            s.pages.get(pid).map(|st| st.press.is_some()).unwrap_or(false)
        };
        let due = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            let now = now_ms();
            if now.saturating_sub(st.last_js_move_ms) >= JS_MOVE_THROTTLE_MS {
                st.last_js_move_ms = now;
                true
            } else {
                false
            }
        };
        if due {
            if let Some(sel) = hit_sel.as_deref().or(focused.as_deref()) {
                if let Some(reply) = api::js_dispatch_bounded_with(
                    self,
                    pid,
                    sel,
                    "mousemove",
                    Some(mouse_payload(x, y, MouseButton::Left, pressed, 0)),
                )
                .await
                {
                    api::apply_js_reply(self, pid, &reply).await;
                }
            }
        }
        json!({"x": x, "y": y, "hover": hit_sel, "restyled": changed})
    }

    /// 按下:hover 链 + `:active` + 可聚焦元素聚焦 + JS mousedown;
    /// P2a:文本控件内按下 = 定位光标(Shift+点击 = 扩选)并武装选区拖选。
    async fn human_down(&self, pid: &str, x: f64, y: f64, button: MouseButton, mods: Mods) -> Value {
        if button != MouseButton::Left {
            // 右键/中键在抬起时处理(contextmenu/新标签),按下不产生页面语义。
            return json!({"x": x, "y": y, "button": "non-left", "pressed": false});
        }
        let hit = self.human_hit_test(pid, x, y).await;
        {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            st.press = Some(PressState {
                selector: hit.as_ref().map(|h| h.selector.clone()),
                x,
                y,
            });
            st.text_drag = None;
            st.text_dragging = false;
        }
        let Some(hit) = hit else {
            return json!({"x": x, "y": y, "hit": false, "pressed": true});
        };
        let focusable = is_focusable(&hit.tag, &hit.typ);
        // P2a:文本控件点击定位光标(Shift+点击沿用旧锚点扩选)。
        let mut caret_out = Value::Null;
        if is_text_like(&hit.tag, &hit.typ) {
            if let Some(st) = self.place_text_caret(pid, &hit, x, y, mods.shift).await {
                {
                    let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                    s.page(pid).text_drag =
                        Some(TextDragState { selector: hit.selector.clone(), anchor: st.anchor });
                }
                caret_out = json!({"pos": st.pos, "anchor": st.anchor});
            }
        }
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                tab.mouse = Some((x, y));
                api::set_hover_chain(tab, &hit.hover_chain);
                api::set_pseudo_state(tab, &hit.selector, api::PseudoDim::Active, true);
                if focusable {
                    tab.focused = Some(hit.selector.clone());
                    api::set_pseudo_state(tab, &hit.selector, api::PseudoDim::Focus, true);
                }
            }
        }
        let sel = hit.selector.clone();
        let tag = hit.tag.clone();
        if let Some(reply) = api::js_dispatch_bounded_with(
            self,
            pid,
            &sel,
            "mousedown",
            Some(mouse_payload(x, y, MouseButton::Left, true, 1)),
        )
        .await
        {
            api::apply_js_reply(self, pid, &reply).await;
        }
        api::relayout_and_push(self, pid).await;
        json!({"x": x, "y": y, "hit": true, "selector": sel, "tag": tag, "focused": focusable, "caret": caret_out})
    }

    /// 抬起:同目标且位移小 → 完整点击语义(复用 `click_inner`);否则只发 mouseup。
    async fn human_up(
        &self,
        pid: &str,
        x: f64,
        y: f64,
        button: MouseButton,
        mods: Mods,
        arrived: u128,
    ) -> Value {
        let (press, drag_started, text_drag, text_dragging) = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            (
                st.press.take(),
                std::mem::take(&mut st.drag_started),
                st.text_drag.take(),
                std::mem::take(&mut st.text_dragging),
            )
        };
        // 释放按下态(无论是否成点击)。
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                api::release_active(tab);
            }
        }
        let Some(press) = press else {
            return json!({"x": x, "y": y, "released": false, "reason": "no_press"});
        };
        let dist = (press.x - x).hypot(press.y - y);
        let hit = self.human_hit_test(pid, x, y).await;

        // 非左键:右键 = contextmenu 事件;中键 = 链接新标签。
        match button {
            MouseButton::Right => {
                if let Some(h) = hit.as_ref() {
                    let sel = h.selector.clone();
                    if let Some(reply) = api::js_dispatch_bounded_with(
                        self,
                        pid,
                        &sel,
                        "contextmenu",
                        Some(mouse_payload(x, y, MouseButton::Right, false, 0)),
                    )
                    .await
                    {
                        api::apply_js_reply(self, pid, &reply).await;
                    }
                    self.note_human_event(pid, &format!("人工右键 ·{}", h.tag));
                    return json!({"button": "right", "selector": sel});
                }
                return json!({"button": "right", "hit": false});
            }
            MouseButton::Middle => {
                if let Some(h) = hit.as_ref().filter(|h| h.tag == "a") {
                    let href = self.attr_of(pid, &h.selector, "href").await;
                    let url = match href {
                        Some(href) => self.resolve_page_url(pid, &href).await,
                        None => None,
                    };
                    if let Some(url) = url {
                        let spawned = self.new_tab(pid, &url, 60_000).await;
                        self.note_human_event(pid, "人工中键新标签");
                        return match spawned {
                            Ok(o) => json!({"button": "middle", "new_tab": o.page_id}),
                            Err(e) => json!({"button": "middle", "error": e.message}),
                        };
                    }
                }
                return json!({"button": "middle", "hit": false});
            }
            MouseButton::Left => {}
        }

        // P2a:文本拖选收尾 = 只落选区,不合成 click / HTML5 drag(浏览器语义:
        // 文本框里按住拖动是选区手势,松开不算点击)。
        if text_dragging {
            if let Some(td) = &text_drag {
                if let Some(h) = hit.as_ref().filter(|h| h.selector == td.selector) {
                    if let Some(idx) = self.text_caret_at(pid, h, x, y).await {
                        let mut inner = self.inner.lock().await;
                        if let Some(tab) = inner.tabs.get_mut(pid) {
                            tab.form_caret.insert(
                                td.selector.clone(),
                                CaretState { pos: idx, anchor: td.anchor },
                            );
                        }
                    }
                }
                api::relayout_and_push(self, pid).await;
                self.note_human_event(pid, "人工拖选(文本选区)");
                return json!({"x": x, "y": y, "clicked": false, "selection": true});
            }
        }
        // 左键:同目标且位移小 → 点击;否则视为拖拽收尾。
        let same_target = press.selector.is_some()
            && hit.as_ref().map(|h| h.selector.as_str()) == press.selector.as_deref();
        if !same_target || dist > CLICK_DIST {
            // HTML5 drag 近似:dragstart 已派发 → drop(命中他者)+ dragend(源)。
            if drag_started {
                if let Some(src) = press.selector.clone() {
                    if let Some(h) = hit.as_ref() {
                        if h.selector != src {
                            let drop_sel = h.selector.clone();
                            if let Some(reply) = api::js_dispatch_bounded_with(
                                self,
                                pid,
                                &drop_sel,
                                "drop",
                                Some(mouse_payload(x, y, MouseButton::Left, false, 0)),
                            )
                            .await
                            {
                                api::apply_js_reply(self, pid, &reply).await;
                            }
                        }
                    }
                    if let Some(reply) = api::js_dispatch_bounded_with(
                        self,
                        pid,
                        &src,
                        "dragend",
                        Some(mouse_payload(x, y, MouseButton::Left, false, 0)),
                    )
                    .await
                    {
                        api::apply_js_reply(self, pid, &reply).await;
                    }
                    self.note_human_event(pid, "人工拖拽(dragstart→drop/dragend)");
                    return json!({"x": x, "y": y, "clicked": false, "drag": true});
                }
            }
            if let Some(sel) = press.selector.clone().or(hit.as_ref().map(|h| h.selector.clone())) {
                if let Some(reply) = api::js_dispatch_bounded_with(
                    self,
                    pid,
                    &sel,
                    "mouseup",
                    Some(mouse_payload(x, y, MouseButton::Left, false, 1)),
                )
                .await
                {
                    api::apply_js_reply(self, pid, &reply).await;
                }
                api::relayout_and_push(self, pid).await;
            }
            let reason = if dist > CLICK_DIST { "drag" } else { "target_changed" };
            return json!({"x": x, "y": y, "clicked": false, "reason": reason});
        }

        let mut target = hit.clone().unwrap();
        // label[for] 转发:点 label 等价于点关联控件(浏览器原生语义)。
        if target.tag == "label" {
            if let Some(for_id) = target.label_for.clone() {
                if let Some(fwd) = self.selector_by_id(pid, &for_id).await {
                    target.selector = fwd;
                }
            }
        }
        let sel = target.selector.clone();
        let tag = target.tag.clone();

        // 连击判定(同目标、500ms/6px 窗口):2 = 双击(选词 + JS dblclick),
        // 3+ = 三击(全选);窗口外重新计数。时刻用事件到达时刻(`arrived`)。
        let now = arrived;
        let count = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            let within = st
                .last_click
                .as_ref()
                .map(|m| {
                    m.selector == sel
                        && now.saturating_sub(m.at_ms) <= DBLCLICK_MS
                        && dist <= DBLCLICK_DIST
                })
                .unwrap_or(false);
            if within {
                let m = st.last_click.as_mut().unwrap();
                m.count = (m.count + 1).min(3);
                m.at_ms = now;
                m.count
            } else {
                st.last_click = Some(ClickMark {
                    selector: sel.clone(),
                    at_ms: now,
                    count: 1,
                });
                1
            }
        };
        let dbl = count == 2;
        let triple = count >= 3;

        // input[type=range]:点击轨道 = 按 x 比例设值(先设值再走点击,
        // 页面 click 监听可读到新值)。
        if tag == "input" && target.typ == "range" {
            self.set_range_by_x(pid, &sel, target.rect, x).await;
        }

        let mut params = json!({"selector": sel, "client_x": x, "client_y": y});
        if mods.ctrl || mods.meta {
            // Cmd/Ctrl+点击 = 新标签打开(点击语义保留,导航目标改 _blank)。
            params["force_target"] = json!("_blank");
        }
        let clicked = api::click_inner(self, pid, &params, "human").await;
        if dbl {
            if let Some(reply) = api::js_dispatch_bounded_with(
                self,
                pid,
                &sel,
                "dblclick",
                Some(mouse_payload(x, y, MouseButton::Left, false, 2)),
            )
            .await
            {
                api::apply_js_reply(self, pid, &reply).await;
            }
        }
        // P2a:双击选词 / 三击全选(仅文本控件;单击已在 down 落光标)。
        if (dbl || triple) && text_drag.is_some() {
            let value = self.current_value(pid, &sel).await;
            let pos = {
                let inner = self.inner.lock().await;
                inner
                    .tabs
                    .get(pid)
                    .and_then(|t| t.form_caret.get(&sel).map(|c| c.pos))
                    .unwrap_or_else(|| value.chars().count())
            };
            let caret = if triple {
                text_edit::select_all(&value)
            } else {
                text_edit::select_word(&value, pos)
            };
            {
                let mut inner = self.inner.lock().await;
                if let Some(tab) = inner.tabs.get_mut(pid) {
                    tab.form_caret.insert(sel.clone(), caret);
                }
            }
            api::relayout_and_push(self, pid).await;
        }
        let label = if triple {
            "人工三击"
        } else if dbl {
            "人工双击"
        } else {
            "人工单击"
        };
        self.note_human_event(pid, &format!("{label} ·{tag}"));
        match clicked {
            Ok(out) => json!({"x": x, "y": y, "clicked": true, "selector": sel, "double": dbl, "count": count, "outcome": out}),
            Err(e) => json!({"x": x, "y": y, "clicked": false, "selector": sel, "count": count, "error": e.message}),
        }
    }

    /// 滚轮:命中元素向上找最近可滚容器 → 元素内滚动;否则视口滚动。
    async fn human_wheel(&self, pid: &str, x: f64, y: f64, dx: f64, dy: f64) -> Value {
        let hit = self.human_hit_test(pid, x, y).await;
        // 命中元素的可滚祖先链(含自身);文档对象不跨 await 存活,锁内收敛。
        let scroll_target: Option<String> = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return json!({"ignored": true});
            };
            match &hit {
                Some(h) => {
                    let doc = tab.document_with_state();
                    let mut cur = doc.select(&h.selector).ok().and_then(|v| v.into_iter().next());
                    let mut found = None;
                    while let Some(n) = cur {
                        let sel = Document::stable_selector(&n);
                        if tab.layout.scroll_max.get(&sel).copied().unwrap_or(0.0) > 0.0 {
                            found = Some(sel);
                            break;
                        }
                        cur = n.parent();
                    }
                    found
                }
                None => None,
            }
        };
        let mut inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get_mut(pid) else {
            return json!({"ignored": true});
        };
        if let Some(sel) = scroll_target {
            let cur = tab.scroll_offsets.get(&sel).copied().unwrap_or(0.0);
            let max = tab.layout.scroll_max.get(&sel).copied().unwrap_or(0.0);
            // dy>0 = 向上滚 = scrollTop 减小;dx>0 = 向右滚。
            let next_y = (cur - dy * WHEEL_STEP).clamp(0.0, max.max(0.0));
            tab.scroll_offsets.insert(sel.clone(), next_y);
            drop(inner);
            api::relayout_and_push(self, pid).await;
            return json!({"mode": "element_scroll", "selector": sel, "scrollTop": next_y, "max": max});
        }
        tab.scroll_x = (tab.scroll_x + dx * WHEEL_STEP).max(0.0);
        tab.scroll_y = (tab.scroll_y - dy * WHEEL_STEP).max(0.0);
        let (sx, sy) = (tab.scroll_x, tab.scroll_y);
        drop(inner);
        self.push_frame_if_headed(pid).await;
        json!({"mode": "viewport_scroll", "scrollX": sx, "scrollY": sy})
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
            if !engine.is_rendered(&node) {
                continue;
            }
            let fixed = node_under_fixed(&engine, &node);
            if fixed != *fixed_guess {
                continue;
            }
            let tag = document::tag_name(&node);
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
            resolved.push(HitTarget {
                selector: b.selector.clone(),
                tag,
                typ,
                rect: (b.x, b.y, b.w, b.h),
                hover_chain: api::hover_chain_selectors(&node),
                label_for,
                fixed,
                font_size,
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

fn in_open_dialog(doc: &Document, selector: &str) -> bool {
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
fn node_font_size(engine: &StyleEngine, node: &kuchikiki::NodeRef) -> f64 {
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
fn focusable_selectors(tab: &TabState, doc: &Document) -> Vec<String> {
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
