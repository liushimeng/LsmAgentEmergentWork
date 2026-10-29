//! 有头查看器 ↔ 引擎的桥接层(查看器生命周期 + 帧推送 + 命令处理)。
//!
//! 从 `browser/mod.rs` 机械搬移而来(单文件行数上限的拆分要求),逻辑零改写。
//! 涉及的三条通道:
//!
//! * **帧通道**(宿主 → 查看器):渲染好的 PNG 经 `view::ViewSink` 推给子进程;
//! * **命令通道**(查看器 → 宿主):工具栏/地址栏/窗口尺寸变化经 stdout 按行
//!   JSON 回传,由 `handle_view_command` 分发;
//! * **窗口生命周期**:headed 模式首个页面打开时拉起查看器,全部页面关闭时关掉。

use std::sync::Arc;

use super::super::{
    document, images, layout, page_bars, style, urlutil, LsmBrowserManager, TabState,
};
use super::ViewCommand;

impl LsmBrowserManager {
    /// 确保查看器子进程与命令通道存在;已关闭的句柄不阻止后续重建。
    fn ensure_view_process(&self, width: u32, height: u32, title: &str) {
        let mut guard = self.view.lock().unwrap();
        let needs_start = guard.as_ref().map(|s| !s.is_alive()).unwrap_or(true);
        if !needs_start {
            return;
        }
        *guard = None;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ViewCommand>();
        *self.view_cmd_tx.lock().unwrap() = Some(tx.clone());
        *guard = super::ViewSink::start(width, height, title, tx);
        // 命令处理任务:查看器工具栏/地址栏/尺寸变化 → 引擎。
        // 需要 `Arc<Self>`(弱引用升级);非 Arc 场景(单测直接 new())无命令处理,
        // 查看器仍可正常显示帧。
        let weak = self.self_weak.lock().unwrap().clone();
        if let Some(weak) = weak.upgrade() {
            tokio::spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    weak.handle_view_command(cmd).await;
                }
            });
        }
    }

    /// 有头模式:确保查看器子进程在跑(幂等),并推首帧。
    /// 任何失败(无桌面会话/spawn 失败/写管道失败)静默降级为纯无头,不影响工具行为。
    pub(in crate::agent::browser) fn ensure_view(&self, tab: &TabState) {
        let title = format!("🌐 {} · {} — {}", tab.id, tab.title, tab.url);
        self.ensure_view_process(tab.viewport.width, tab.viewport.height, &title);
        // 两条告知栏的 1Hz 心跳(全进程单实例,`page_bars.rs`)。
        self.ensure_bars_ticker();
        self.push_frame(tab);
    }

    /// 首屏脚本执行前先展示加载页,避免重站点长时间没有可见窗口。
    pub(in crate::agent::browser) fn ensure_loading_view(
        &self,
        url: &str,
        width: u32,
        height: u32,
    ) {
        self.ensure_view_process(width, height, &format!("正在打开 — {url}"));
        let Some(png) = self.loading_view_png(url, width, height) else {
            return;
        };
        if let Ok(guard) = self.view.lock() {
            if let Some(sink) = guard.as_ref() {
                sink.send_frame(&png);
                sink.send_title(&format!("正在打开 — {url}"));
                sink.send_urlbar(url, false, false);
            }
        }
    }

    /// 用同一套布局/渲染管线生成加载首帧,不引入另一套 UI 绘制实现。
    pub(in crate::agent::browser) fn loading_view_png(
        &self,
        url: &str,
        width: u32,
        height: u32,
    ) -> Option<Vec<u8>> {
        let escaped = url
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let html = format!(
            "<!doctype html><html><head><meta charset='utf-8'><title>正在打开</title></head>\
             <body style='margin:0;background:#f5f7fb;font-family:Arial,sans-serif'>\
             <div style='padding:48px 48px 12px;font-size:22px;color:#202124'>正在打开网页…</div>\
             <div style='padding:0 48px;font-size:14px;color:#5f6368'>{escaped}</div>\
             <div style='padding:16px 48px;font-size:13px;color:#80868b'>页面脚本执行完成后将自动显示网页内容</div>\
             </body></html>"
        );
        let doc = document::Document::parse(&html, "about:blank");
        let media = style::MediaCtx::new(width as f64, height as f64);
        let page = layout::compute_full(
            &doc,
            width as f64,
            &self.font,
            &media,
            &images::ImageSizes::default(),
        );
        let pixmap = self.font.render_pixmap(
            &page,
            width,
            height,
            0.0,
            false,
            &images::ImageStore::new(),
        )?;
        pixmap.encode_png().ok()
    }

    /// 处理查看器发来的交互命令(后退/前进/刷新/首页/地址栏导航/窗口尺寸变化)。
    pub(in crate::agent::browser) async fn handle_view_command(
        self: &Arc<Self>,
        cmd: ViewCommand,
    ) {
        let page_id = self.foreground_page_id().await;
        let Some(page_id) = page_id else { return };
        let timeout = 45_000u64;
        let _ = match cmd {
            ViewCommand::Navigate(url) => {
                let target = urlutil::resolve("", &url).unwrap_or_else(|| url.clone());
                self.navigate(&page_id, &target, timeout).await
            }
            ViewCommand::Back => self.history_nav(&page_id, "back", timeout).await,
            ViewCommand::Forward => self.history_nav(&page_id, "forward", timeout).await,
            ViewCommand::Reload => self.history_nav(&page_id, "reload", timeout).await,
            ViewCommand::Home => self.navigate(&page_id, "about:blank", timeout).await,
            ViewCommand::Resize { width, height } => {
                // 人工拖动窗口 → 视口自适应:重排 + 重渲染 + 推新帧(1:1 铺满)。
                self.apply_view_resize(&page_id, width, height).await;
                return;
            }
        };
        self.refresh_view(&page_id).await;
    }

    /// 前台标签页 id(多标签时工具栏作用于前台页)。
    pub(in crate::agent::browser) async fn foreground_page_id(&self) -> Option<String> {
        let inner = self.inner.lock().await;
        let mut fallback: Option<String> = None;
        for tab in inner.tabs.values() {
            if fallback.is_none() {
                fallback = Some(tab.id.clone());
            }
            if tab.foreground {
                return Some(tab.id.clone());
            }
        }
        fallback
    }

    /// 重新推送当前页面的帧与地址栏状态(导航/交互后调用)。
    pub(in crate::agent::browser) async fn refresh_view(&self, page_id: &str) {
        let inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get(page_id) else { return };
        // 无头模式不启查看器(否则纯无头任务会被凭空拉起一个浏览器窗口)。
        if !tab.mode.is_headed() {
            return;
        }
        self.ensure_view(tab);
    }

    /// 渲染当前布局为视口 PNG 并推给查看器(查看器不存在时 no-op)。
    pub(in crate::agent::browser) fn push_frame(&self, tab: &TabState) {
        let alive = self.view.lock().map(|g| g.is_some()).unwrap_or(false);
        if !alive {
            return;
        }
        let Some(mut pixmap) = self.font.render_pixmap(
            &tab.layout,
            tab.viewport.width,
            tab.viewport.height,
            tab.scroll_y,
            false,
            &tab.images,
        ) else {
            return;
        };
        // 屏蔽页面状态栏/进度栏:查看器帧合成层叠加(不进截图取证路径)。
        page_bars::draw(&mut pixmap, &self.font, tab);
        let png = pixmap.encode_png().unwrap_or_default();
        if png.is_empty() {
            return;
        }
        let can_back = tab.history_index > 0;
        let can_forward = tab.history_index + 1 < tab.history.len();
        if let Ok(guard) = self.view.lock() {
            if let Some(sink) = guard.as_ref() {
                sink.send_frame(&png);
                sink.send_title(&format!("🌐 {} · {} — {}", tab.id, tab.title, tab.url));
                sink.send_urlbar(&tab.url, can_back, can_forward);
            }
        }
    }

    /// 关闭查看器(所有页面关闭后)。
    pub(in crate::agent::browser) fn close_view(&self) {
        if let Ok(mut guard) = self.view.lock() {
            if let Some(sink) = guard.take() {
                sink.send_close();
            }
        }
    }
}
