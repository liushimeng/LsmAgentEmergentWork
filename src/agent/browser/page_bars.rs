//! Lsm 内核侧**屏蔽页面状态栏 / 屏蔽页面进度栏**(双浏览器工具架构)。
//!
//! 进程内内核**不走页面注入**(注入 DOM 会触发 MutationObserver、污染页面脚本
//! 可见状态、进截图证据),两条栏在**查看器帧合成层**绘制:`push_frame` 渲染
//! 页面 pixmap 之后、`encode_png` 之前叠加绘制。截图取证路径
//! (`screenshot_locked`)不经过本层,证据永远干净。
//!
//! | 栏 | 位置 | 内容 |
//! | -- | ---- | ---- |
//! | **屏蔽页面状态栏** | 右上角 | 具体 Agent 名 + 档位行 + 静态注意事项 + guard_note |
//! | **屏蔽页面进度栏** | 左下角 | 当前动作 + 迭代/会话/耗时 + 进度条 + 任务摘要 + 事件时间线 |
//!
//! 数据源 = `crate::agent::web_activity`(task_local 活动台账):工具动作前后由
//! `note_action_begin/end` 把快照落进 `TabState.bars`(ticker 在 Agent 任务树外
//! 重绘,必须落账而不是实时读台账)。Chrome 侧姊妹实现在
//! `src/agent/chrome_browser/status.rs`(注入 JS)。
//!
//! 设计详案 `docs/双浏览器工具架构/02-屏蔽页面状态栏与进度栏.md`。

use tiny_skia::Pixmap;

use super::render::FontSet;
use super::{LsmBrowserManager, TabState};
use crate::agent::web_activity;

/// 事件时间线保留条数(与 Chrome 侧 JS `MAX_EVENTS` 同步)。
pub const EVENT_WINDOW: usize = 6;

/// `LAEW_PAGE_BARS=off|0|false|no` 关闭两条告知栏(两工具同参,默认开启)。
pub fn bars_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_PAGE_BARS")
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 单条进度事件(时间线滚动条目)。
#[derive(Debug, Clone)]
pub struct BarEvent {
    pub t: u128,
    pub label: String,
    pub ok: bool,
    pub ms: u64,
}

/// 标签页级两条栏状态快照(台账在 Agent 任务树内更新,渲染在帧合成层/ticker
/// 任务里读 —— 跨任务边界必须落账)。
#[derive(Debug, Clone, Default)]
pub struct PageBars {
    /// 具体 Agent 名(如 `LsmAgentEmergentWork-SubAgent-Work`)。
    pub agent: String,
    pub session: String,
    pub iter: usize,
    pub max_iter: usize,
    pub task: String,
    pub phase: String,
    pub tool: String,
    /// 当前/最近一次浏览器动作标签。
    pub action: String,
    /// 会话开始时刻(毫秒),耗时行按绘制时刻现算。
    pub started_ms: u128,
    /// 最近事件时间线(≤ EVENT_WINDOW,新事件在尾部)。
    pub events: Vec<BarEvent>,
}

impl PageBars {
    /// 用活动台账快照合并字段(空字段保持原值;iter/max_iter 恒覆盖)。
    pub fn apply_snapshot(&mut self, snap: &web_activity::ActivityState) {
        if !snap.agent.is_empty() {
            self.agent = snap.agent.clone();
        }
        if !snap.session.is_empty() {
            self.session = snap.session.clone();
        }
        self.iter = snap.iter;
        self.max_iter = snap.max_iter;
        if !snap.task.is_empty() {
            self.task = snap.task.clone();
        }
        if !snap.phase.is_empty() {
            self.phase = snap.phase.clone();
        }
        if !snap.tool.is_empty() {
            self.tool = snap.tool.clone();
        }
        if snap.started_ms > 0 {
            self.started_ms = snap.started_ms;
        }
    }

    /// 追加事件并维持环形窗口。
    pub fn push_event(&mut self, ev: BarEvent) {
        self.events.push(ev);
        while self.events.len() > EVENT_WINDOW {
            self.events.remove(0);
        }
    }
}

// ===================== 行文本组装(纯函数,可单测) =====================

/// 一行告知文本。
#[derive(Debug, Clone)]
pub struct BarRow {
    pub text: String,
    pub color: [u8; 3],
    pub bold: bool,
    pub size: f32,
}

/// Lsm 档位行:查看器是被动帧显示窗,人工确实只能观看(工具栏/地址栏除外),
/// 如实描述为「仅观看」—— 与 Chrome 侧(headed 真窗口,人工可直接操作)不同。
pub fn lsm_mode_text() -> (&'static str, &'static str) {
    (
        "页面已锁定(仅观看)",
        "反-反爬进程内内核;页面交互由 Agent 工具执行;如需人工介入请在终端对话中说明",
    )
}

/// 组装两条栏的全部行文本。返 `(状态栏行, 进度栏行, 进度百分比 0..=100)`。
pub fn assemble_rows(bars: &PageBars, guard_note: Option<&str>) -> (Vec<BarRow>, Vec<BarRow>, f32) {
    const TITLE: [u8; 3] = [0x60, 0xa5, 0xfa];
    const BRIGHT: [u8; 3] = [0xe2, 0xe8, 0xf0];
    const MUTED: [u8; 3] = [0x94, 0xa3, 0xb8];
    const NORMAL: [u8; 3] = [0xcb, 0xd5, 0xe1];
    const ACTION: [u8; 3] = [0xa7, 0xf3, 0xd0];

    // ---- 屏蔽页面状态栏(右上角):①谁在控制 ②档位 ③静态注意事项 ④note ----
    let agent = if bars.agent.is_empty() { "LAEW Agent" } else { &bars.agent };
    let mut status = vec![
        BarRow {
            text: format!("LAEW Agent 控制中 · {agent}"),
            color: TITLE,
            bold: true,
            size: 13.0,
        },
    ];
    let (mode, notice) = lsm_mode_text();
    status.push(BarRow { text: mode.to_string(), color: BRIGHT, bold: false, size: 12.0 });
    status.push(BarRow { text: notice.to_string(), color: MUTED, bold: false, size: 12.0 });
    if let Some(note) = guard_note.filter(|n| !n.trim().is_empty()) {
        status.push(BarRow {
            text: web_activity::clip_chars(note, 60),
            color: NORMAL,
            bold: false,
            size: 12.0,
        });
    }

    // ---- 屏蔽页面进度栏(左下角):①动作 ②迭代/会话/耗时 ③任务 ④事件时间线 ----
    let mut progress = Vec::with_capacity(4);
    let parts: Vec<&str> = [&bars.phase, &bars.action, &bars.tool]
        .into_iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.as_str())
        .collect();
    let action_line = if parts.is_empty() {
        "待命中 · 等待下一次浏览器动作".to_string()
    } else {
        format!("▶ {}", parts.join(" · "))
    };
    progress.push(BarRow { text: action_line, color: ACTION, bold: true, size: 12.0 });

    let mut meta = Vec::new();
    if bars.max_iter > 0 {
        meta.push(format!("迭代 {}/{}", bars.iter, bars.max_iter));
    }
    if !bars.session.is_empty() {
        let prefix: String = bars.session.chars().take(6).collect();
        meta.push(format!("会话 {prefix}"));
    }
    if bars.started_ms > 0 {
        meta.push(format!("已用 {}", dur_text(now_ms().saturating_sub(bars.started_ms))));
    }
    if !meta.is_empty() {
        progress.push(BarRow { text: meta.join(" · "), color: MUTED, bold: false, size: 12.0 });
    }
    if !bars.task.is_empty() {
        progress.push(BarRow {
            text: format!("任务:{}", bars.task),
            color: NORMAL,
            bold: false,
            size: 12.0,
        });
    }
    if !bars.events.is_empty() {
        let line = bars
            .events
            .iter()
            .map(|e| {
                format!(
                    "{} {} {}{}",
                    hhmmss(e.t),
                    if e.ok { "✓" } else { "✗" },
                    e.label,
                    if e.ms > 0 { format!(" {}", dur_text(e.ms as u128)) } else { String::new() }
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
        progress.push(BarRow { text: line, color: MUTED, bold: false, size: 12.0 });
    }
    let fill_pct = if bars.max_iter > 0 {
        ((bars.iter as f32 / bars.max_iter as f32) * 100.0).min(100.0)
    } else {
        0.0
    };
    (status, progress, fill_pct)
}

/// 毫秒 → 人读时长(`1分12秒`),与 Chrome 侧页面 JS `dur()` 同款。
pub fn dur_text(ms: u128) -> String {
    let s = (ms / 1000) as u64;
    if s < 60 {
        return format!("{s}秒");
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}分{}秒", s % 60);
    }
    format!("{}时{}分", m / 60, m % 60)
}

/// 毫秒时间戳 → `HH:MM:SS`(优先本地时区,与 Chrome 侧页面 JS
/// `new Date().getHours()` 同一语义;取不到本地偏移退化为 UTC —— 事件时间线
/// 只用于人工对时序,偶发退化不影响判读)。
pub fn hhmmss(ms: u128) -> String {
    let base = time::OffsetDateTime::from_unix_timestamp((ms / 1000) as i64)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    let dt = match time::UtcOffset::current_local_offset() {
        Ok(offset) => base.to_offset(offset),
        Err(_) => base,
    };
    format!("{:02}:{:02}:{:02}", dt.hour(), dt.minute(), dt.second())
}

// ===================== 帧合成绘制 =====================

/// 底色 `rgba(15,23,42,0.88)` 的 alpha。
const BG_ALPHA: u8 = 224;
const PAD_X: f32 = 12.0;
const PAD_Y: f32 = 7.0;
const MARGIN: f32 = 12.0;

/// 在已渲染的页面 pixmap 上叠加绘制两条栏(仅 headed 查看器帧路径调用)。
pub(crate) fn draw(pixmap: &mut Pixmap, font: &FontSet, tab: &TabState) {
    if !tab.mode.is_headed() || !bars_enabled() {
        return;
    }
    let vw = pixmap.width() as f32;
    let vh = pixmap.height() as f32;
    if vw < 320.0 || vh < 240.0 {
        return; // 视口过小(极端窗口),两条栏无处安放,跳过。
    }
    let (status, progress, fill_pct) = assemble_rows(&tab.bars, tab.guard.note.as_deref());
    let status_w = layout_bar(font, &status, vw, false);
    draw_bar(pixmap, font, &status, status_w, vw - MARGIN - status_w.0, MARGIN, None);
    // 进度栏:左下角锚定;含 3px 进度条,总高相应加高。
    let prog_w = layout_bar(font, &progress, vw, true);
    let bar_h = prog_w.1;
    let y0 = vh - MARGIN - bar_h;
    draw_bar(pixmap, font, &progress, prog_w, MARGIN, y0, Some(fill_pct));
}

/// 量算一条栏的 `(宽, 高)`(按最长行宽收敛,带上限防撑破视口)。
fn layout_bar(font: &FontSet, rows: &[BarRow], vw: f32, with_fill: bool) -> (f32, f32) {
    let cap = (vw * 0.48).min(560.0) - PAD_X * 2.0;
    let mut w = 0.0f32;
    for r in rows {
        w = w.max(font.text_width(&r.text, r.size as f64) as f32);
    }
    let w = (w + PAD_X * 2.0).min(cap + PAD_X * 2.0).max(120.0);
    let mut h = PAD_Y * 2.0;
    for r in rows {
        h += r.size * 1.6;
    }
    if with_fill {
        h += 3.0 + 4.0; // 进度条 + 上下留隙
    }
    (w, h)
}

/// 绘制一条栏:半透明深底 + 逐行文本;`fill` 非 None 时在第二行后画进度条。
fn draw_bar(
    pixmap: &mut Pixmap,
    font: &FontSet,
    rows: &[BarRow],
    dims: (f32, f32),
    x0: f32,
    y0: f32,
    fill: Option<f32>,
) {
    let (w, h) = dims;
    // 半透明底。
    if let Some(rect) = tiny_skia::Rect::from_xywh(x0, y0, w, h) {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(15, 23, 42, BG_ALPHA);
        pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
    }
    let text_max = w - PAD_X * 2.0;
    let mut y = y0 + PAD_Y;
    let mut drawn = 0usize;
    for r in rows {
        font.draw_overlay_text(
            pixmap,
            x0 + PAD_X,
            y,
            &r.text,
            r.size,
            r.color,
            r.bold,
            text_max,
        );
        y += r.size * 1.6;
        drawn += 1;
        // 进度条画在第二行(迭代/耗时行)之后。
        if fill.is_some() && drawn == 2 {
            let track_y = y + 1.0;
            if let Some(rect) = tiny_skia::Rect::from_xywh(x0 + PAD_X, track_y, text_max, 3.0) {
                let mut paint = tiny_skia::Paint::default();
                paint.set_color_rgba8(148, 163, 184, 90);
                pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
            }
            let fill_w = (text_max * (fill.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0)).max(2.0);
            if let Some(rect) = tiny_skia::Rect::from_xywh(x0 + PAD_X, track_y, fill_w, 3.0) {
                let mut paint = tiny_skia::Paint::default();
                paint.set_color_rgba8(96, 165, 250, 255);
                pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
            }
            y += 3.0 + 4.0;
        }
    }
}

// ===================== 管理器接线(动作落账 + 1Hz 心跳) =====================

impl LsmBrowserManager {
    /// 工具动作开始:把台账快照 + 动作标签落进目标标签页(`None` = 广播全部;
    /// ticker 在 Agent 任务树外,必须落账而不是渲染时实时读台账)。
    pub(crate) async fn note_action_begin(&self, page_hint: Option<&str>, label: &str) {
        if !bars_enabled() {
            return;
        }
        let snap = web_activity::current_snapshot();
        let mut inner = self.inner.lock().await;
        for (id, tab) in inner.tabs.iter_mut() {
            if let Some(hint) = page_hint {
                if id != hint {
                    continue;
                }
            }
            if let Some(s) = snap.as_ref() {
                tab.bars.apply_snapshot(s);
            }
            tab.bars.action = label.to_string();
        }
    }

    /// 工具动作结束:目标标签页事件时间线滚动追加(动作本身的收尾刷新走
    /// refresh_view,1Hz 心跳兜底耗时行跳动)。
    pub(crate) async fn note_action_end(
        &self,
        page_hint: Option<&str>,
        label: &str,
        ok: bool,
        elapsed_ms: u64,
    ) {
        if !bars_enabled() {
            return;
        }
        let ev = BarEvent {
            t: web_activity::now_ms(),
            label: web_activity::clip_chars(label, 60),
            ok,
            ms: elapsed_ms,
        };
        let mut inner = self.inner.lock().await;
        for (id, tab) in inner.tabs.iter_mut() {
            if let Some(hint) = page_hint {
                if id != hint {
                    continue;
                }
            }
            tab.bars.push_event(ev.clone());
        }
    }

    /// 启动两条栏的 1Hz 心跳(全进程单实例;查看器存活时每秒重推前台帧,
    /// 让「已用 1分12秒」等动态行持续跳动)。
    pub(crate) fn ensure_bars_ticker(&self) {
        use std::sync::atomic::Ordering;
        if !bars_enabled() {
            return;
        }
        if self.bars_ticker_started.swap(true, Ordering::Relaxed) {
            return;
        }
        let weak = self
            .self_weak
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| std::sync::Weak::new());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let Some(mgr) = weak.upgrade() else { break };
                mgr.bars_tick().await;
            }
        });
    }

    /// 心跳一拍:查看器存活且有 headed 前台页时重推一帧(无查看器零渲染开销)。
    async fn bars_tick(&self) {
        if !bars_enabled() {
            return;
        }
        let alive = self.view.lock().map(|g| g.is_some()).unwrap_or(false);
        if !alive {
            return;
        }
        let inner = self.inner.lock().await;
        let tab = inner
            .tabs
            .values()
            .find(|t| t.foreground && t.mode.is_headed())
            .or_else(|| inner.tabs.values().find(|t| t.mode.is_headed()));
        if let Some(tab) = tab {
            self.push_frame(tab);
        }
    }
}

fn now_ms() -> u128 {
    web_activity::now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars() -> PageBars {
        PageBars {
            agent: "LsmAgentEmergentWork-SubAgent-Work".into(),
            session: "sess-abcdef".into(),
            iter: 3,
            max_iter: 12,
            task: "调研双站价格".into(),
            phase: "第 3/12 轮 · 执行工具".into(),
            tool: "MCP_Web_LsmAgentBrowser_Use control click #buy".into(),
            action: "control click #buy".into(),
            started_ms: web_activity::now_ms(),
            events: vec![],
        }
    }

    #[test]
    fn 状态栏标题必须带具体Agent名() {
        let (status, _, _) = assemble_rows(&bars(), None);
        assert_eq!(
            status[0].text,
            "LAEW Agent 控制中 · LsmAgentEmergentWork-SubAgent-Work"
        );
        assert!(status[0].bold, "标题行应加粗");
    }

    #[test]
    fn 无台账时回退品牌名() {
        let (status, _, _) = assemble_rows(&PageBars::default(), None);
        assert_eq!(status[0].text, "LAEW Agent 控制中 · LAEW Agent");
    }

    #[test]
    fn 静态注意事项在状态栏且进度栏无固定声明() {
        let (status, progress, _) = assemble_rows(&bars(), None);
        let (_, notice) = lsm_mode_text();
        // 状态栏:档位行 + 注意事项行齐全。
        assert!(status.iter().any(|r| r.text.contains("仅观看")), "档位行应在状态栏");
        assert!(
            status.iter().any(|r| r.text == notice),
            "静态注意事项应整体迁入状态栏"
        );
        // 进度栏:全是动态内容,不含固定声明。
        assert!(!progress.iter().any(|r| r.text.contains("如需人工介入")));
    }

    #[test]
    fn guard_note进状态栏且截断() {
        let note = "自".repeat(80);
        let (status, _, _) = assemble_rows(&bars(), Some(&note));
        let note_rows: Vec<&BarRow> = status.iter().filter(|r| r.text.starts_with("自")).collect();
        assert_eq!(note_rows.len(), 1, "note 行应恰好一行");
        assert!(note_rows[0].text.chars().count() <= 60, "note 超 60 字符应截断");
        let (s2, _, _) = assemble_rows(&bars(), Some("  "));
        assert_eq!(s2.len(), 3, "空白 note 不产生行");
    }

    #[test]
    fn 进度栏动态行组装() {
        let mut b = bars();
        b.events = vec![
            BarEvent { t: 1_700_000_000_000, label: "open https://a".into(), ok: true, ms: 800 },
            BarEvent { t: 1_700_000_020_000, label: "click #buy".into(), ok: false, ms: 0 },
        ];
        let (_, progress, fill) = assemble_rows(&b, None);
        assert!(progress[0].text.starts_with("▶ 第 3/12 轮 · 执行工具 · control click #buy"));
        assert!(progress[1].text.contains("迭代 3/12"), "{}", progress[1].text);
        assert!(progress[1].text.contains("会话 sess-a"), "{}", progress[1].text);
        assert!(progress[1].text.contains("已用 "), "{}", progress[1].text);
        assert_eq!(fill, 25.0, "3/12 应为 25%");
        assert!(progress[2].text.contains("任务:调研双站价格"));
        let events_line = progress.last().unwrap();
        assert!(events_line.text.contains("✓ open https://a 0秒") || events_line.text.contains("✓ open https://a"));
        assert!(events_line.text.contains("✗ click #buy"));
    }

    #[test]
    fn 动作全空时进度栏占位() {
        let mut b = PageBars::default();
        b.agent = "LsmAgentEmergentWork-Yolo".into();
        let (_, progress, fill) = assemble_rows(&b, None);
        assert_eq!(progress[0].text, "待命中 · 等待下一次浏览器动作");
        assert_eq!(fill, 0.0);
    }

    #[test]
    fn 事件环形窗口() {
        let mut b = PageBars::default();
        for i in 0..10 {
            b.push_event(BarEvent { t: i as u128, label: format!("e{i}"), ok: true, ms: 0 });
        }
        assert_eq!(b.events.len(), EVENT_WINDOW);
        assert_eq!(b.events.last().unwrap().label, "e9");
        assert_eq!(b.events.first().unwrap().label, "e4");
    }

    #[test]
    fn 台账快照合并不覆盖非空字段() {
        let mut b = bars();
        let mut snap = web_activity::ActivityState::default();
        snap.iter = 5;
        snap.max_iter = 12;
        b.apply_snapshot(&snap);
        // 空 agent/task 保持原值,迭代恒覆盖。
        assert_eq!(b.agent, "LsmAgentEmergentWork-SubAgent-Work");
        assert_eq!(b.task, "调研双站价格");
        assert_eq!(b.iter, 5);
    }

    #[test]
    fn 时长格式() {
        assert_eq!(dur_text(800), "0秒");
        assert_eq!(dur_text(72_000), "1分12秒");
        assert_eq!(dur_text(3_780_000), "1时3分");
    }

    #[test]
    fn 时间戳转时分秒() {
        // 本地时区随机器而变,断言形状 + 秒字段(时区偏移恒为整分钟,秒不受影响)。
        let out = hhmmss(86_400_000 + 36_610_000);
        assert_eq!(out.len(), 8);
        assert!(out.as_bytes()[2] == b':' && out.as_bytes()[5] == b':', "应为 HH:MM:SS 形状");
        assert!(out.ends_with(":10"), "秒字段应为 10(时区偏移不改变秒):{out}");
        assert!(hhmmss(500).ends_with(":00"));
    }

    #[test]
    fn 绘制冒烟_无字体也不panic() {
        // FontSet::fallback() 无字体:两条栏退化为占位色条,仍必须不 panic、不空图。
        use super::super::{security, BrowserMode, Viewport};
        let font = FontSet::fallback();
        let mut pixmap = Pixmap::new(800, 600).unwrap();
        let tab = TabState {
            id: "p_test".into(),
            window_id: "w1".into(),
            url: "https://a.example".into(),
            title: "t".into(),
            status: 200,
            content_type: "text/html".into(),
            html: String::new(),
            redirect_chain: vec![],
            source_bytes: 0,
            history: vec![],
            history_index: 0,
            session_storage: Default::default(),
            viewport: Viewport { width: 800, height: 600, device_scale_factor: 1.0, mobile: false },
            scroll_x: 0.0,
            scroll_y: 0.0,
            network: vec![],
            console: vec![],
            form_values: Default::default(),
            focused: None,
            mouse: None,
            layout: Default::default(),
            created_at_ms: 0,
            last_active_ms: 0,
            foreground: true,
            user_agent: String::new(),
            mode: BrowserMode::Headed,
            guard: Default::default(),
            highlight: false,
            auto_follow: false,
            bars: bars(),
            js: None,
            executed_scripts: Default::default(),
            images: Default::default(),
            security: security::SecurityContext::new(),
            webgl_asked: 0,
        };
        draw(&mut pixmap, &font, &tab);
        // 底色已画:统计非零像素(半透明深底必然改变像素)。
        let changed = pixmap.pixels().iter().filter(|p| p.red() < 40 && p.blue() > 20).count();
        assert!(changed > 0, "两条栏底色应已绘制");
    }
}

