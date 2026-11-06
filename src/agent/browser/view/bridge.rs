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
    document, images, inspect, layout, page_bars, style, urlutil, LsmBrowserManager, TabState,
    ViewFollow,
};
use super::ViewCommand;

impl LsmBrowserManager {
    /// 设置查看器跟随目标(动作落账/人工工具栏操作/加载首帧/显示收尾)。
    /// 叶子锁:绝不与 `inner` 同时持有(心跳反序拿锁,防死锁)。
    pub(crate) fn set_view_follow(&self, f: Option<ViewFollow>) {
        if let Ok(mut g) = self.view_follow.lock() {
            *g = f;
        }
    }

    /// 读取查看器跟随目标(克隆返回,调用方不长期持锁)。
    pub(crate) fn view_follow(&self) -> Option<ViewFollow> {
        self.view_follow.lock().ok().and_then(|g| g.clone())
    }

    /// 确保查看器子进程与命令通道存在;已关闭的句柄不阻止后续重建。
    ///
    /// 自动重生限次:首启免费,此后每次(重)启动计数,第 4 次起拒绝并 WARN
    /// ——防「一启动就崩」的查看器把任务拖进无限重生循环;`close_view` 收口时
    /// 清零预算(2026-11-04 查看器崩溃事故的自愈策略)。
    fn ensure_view_process(&self, width: u32, height: u32, title: &str) {
        let mut guard = self.view.lock().unwrap();
        let needs_start = guard.as_ref().map(|s| !s.is_alive()).unwrap_or(true);
        if !needs_start {
            return;
        }
        let starts = self
            .view_respawns
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if starts >= 3 {
            *guard = None;
            tracing::warn!(
                "查看器子进程反复死亡,自动重生已达上限(3 次),回退纯无头;崩溃现场见工作目录 CrashReport/"
            );
            return;
        }
        if starts > 0 {
            tracing::warn!(
                "查看器子进程已死亡,自动重生(第 {starts} 次);若反复出现请查工作目录 CrashReport/"
            );
        }
        *guard = None;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ViewCommand>();
        *self.view_cmd_tx.lock().unwrap() = Some(tx.clone());
        *guard = super::ViewSink::start(width, height, title, tx);
        if guard.is_some() {
            // 窗口已重新在位:清「人工关窗」旗标(新一轮显示意图)。
            self.view_user_closed
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
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
        // 第 57 轮:动画帧同步 ticker(活跃动画时 40ms 重排推帧,`anim.rs`)。
        self.ensure_anim_ticker();
        // 推帧(push_frame)同步落跟随页。
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
                // 加载帧已入查看器:置 PendingLoading 让 1Hz 心跳暂停推送,
                // 否则旧跟随页的帧一秒后就把加载页盖掉(open/navigate 收尾的
                // ensure_view 会落回 Page,心跳随之恢复)。
                self.set_view_follow(Some(ViewFollow::PendingLoading {
                    since_ms: page_bars::now_ms_public(),
                }));
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
            0.0,
            false,
            &images::ImageStore::new(),
        )?;
        pixmap.encode_png().ok()
    }

    /// 处理查看器发来的交互命令(后退/前进/刷新/首页/地址栏导航/窗口尺寸变化/
    /// 人类键鼠)。
    pub(in crate::agent::browser) async fn handle_view_command(
        self: &Arc<Self>,
        cmd: ViewCommand,
    ) {
        // 人类键鼠:作用于查看器正在显示的页(跟随页,缺省前台页);
        // 不改显示跟随(输入不换页),也不做导航收尾 refresh。
        if let ViewCommand::Input(ev) = cmd {
            let target = self.input_target_page().await;
            if let Some(pid) = target {
                self.handle_human_input(&pid, ev).await;
            }
            return;
        }
        // 人工关窗:置旗标,`push_frame` 不再自动重生窗口(不与用户抢窗口);
        // 进程意外死亡(崩溃,无此信号)才走自愈重生。
        if let ViewCommand::ViewerClosed = cmd {
            self.view_user_closed
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        // 标签页快捷键与控件级剪贴板(39 号设计文档阶段二):作用于显示页。
        match cmd {
            ViewCommand::NewTab => {
                self.viewer_new_tab().await;
                return;
            }
            ViewCommand::CloseTab => {
                let target = self.input_target_page().await;
                self.viewer_close_tab(target.as_deref()).await;
                return;
            }
            ViewCommand::NextTab => {
                let target = self.input_target_page().await;
                self.viewer_next_tab(target.as_deref()).await;
                return;
            }
            ViewCommand::Clip(op) => {
                let target = self.input_target_page().await;
                if let Some(pid) = target {
                    let _ = self.handle_human_clip(&pid, op).await;
                }
                return;
            }
            ViewCommand::ToggleFind => {
                // ⌘/Ctrl+F:页面查找(39 号 P2c)。作用于显示页,门内做控制权闸门。
                let target = self.input_target_page().await;
                if let Some(pid) = target {
                    let _ = self.toggle_find(&pid).await;
                }
                return;
            }
            ViewCommand::FocusAddrbar => {
                // ⌘/Ctrl+L:查看器侧直接把焦点交给地址栏(命令由查看器本地拦截,
                // 宿主只落「显示跟随」语义,无页面动作)。
                if let Ok(guard) = self.view.lock() {
                    if let Some(sink) = guard.as_ref() {
                        sink.focus_addrbar();
                    }
                }
                return;
            }
            _ => {}
        }
        let page_id = self.foreground_page_id().await;
        let Some(page_id) = page_id else { return };
        // 人工在查看器里操作(工具栏/地址栏):显式表达了「看这页」,
        // 跟随页立刻切到前台页,Agent 后续动作再拉回工作页。
        self.set_view_follow(Some(ViewFollow::Page(page_id.clone())));
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
            ViewCommand::Zoom(z) => {
                // P2d:人工缩放记账(缩放本身已在查看器侧生效——视口按新系数
                // 换算另行走 resize);前台页存档,page_health 带出,Agent 能
                // 解释「为什么视口变小了」。
                let mut inner = self.inner.lock().await;
                if let Some(tab) = inner.tabs.get_mut(&page_id) {
                    tab.human_zoom = z;
                }
                return;
            }
            ViewCommand::Input(_)
            | ViewCommand::ViewerClosed
            | ViewCommand::NewTab
            | ViewCommand::CloseTab
            | ViewCommand::NextTab
            | ViewCommand::Clip(_)
            | ViewCommand::FocusAddrbar
            | ViewCommand::ToggleFind => unreachable!("已在函数开头单独处理"),
        };
        self.refresh_view(&page_id).await;
    }

    // ---------- 标签页快捷键(⌘/Ctrl+T/W、Ctrl+Tab;39 号设计文档阶段二) ----------

    /// ⌘/Ctrl+T:从前台页派生新标签(空白页)并显示。
    async fn viewer_new_tab(self: &Arc<Self>) {
        let Some(from) = self.foreground_page_id().await else {
            return;
        };
        if let Ok(out) = self.new_tab(&from, "about:blank", 45_000).await {
            self.refresh_view(&out.page_id).await;
        }
    }

    /// ⌘/Ctrl+W:关显示页;**最后一个标签改为打开空白页**(查看器保活,
    /// 与真浏览器「关最后一页不关窗口」一致)。
    async fn viewer_close_tab(self: &Arc<Self>, pid: Option<&str>) {
        let pid = match pid {
            Some(p) => p.to_string(),
            None => match self.foreground_page_id().await {
                Some(p) => p,
                None => return,
            },
        };
        let (total, next) = {
            let inner = self.inner.lock().await;
            if inner.tabs.len() <= 1 {
                (inner.tabs.len(), None)
            } else {
                // 同窗口的下一个标签优先,缺省任一存活页。
                let win_next = inner
                    .windows
                    .iter()
                    .find_map(|w| {
                        let idx = w.tabs.iter().position(|t| t == &pid)?;
                        if w.tabs.len() < 2 {
                            return None;
                        }
                        Some(w.tabs[(idx + 1) % w.tabs.len()].clone())
                    })
                    .or_else(|| inner.tabs.keys().find(|k| *k != &pid).cloned());
                (inner.tabs.len(), win_next)
            }
        };
        if total <= 1 {
            let _ = self.navigate(&pid, "about:blank", 45_000).await;
            self.refresh_view(&pid).await;
            return;
        }
        let _ = self.close_page(&pid).await;
        if let Some(next) = next {
            let _ = self.bring_to_front(&next).await;
            self.refresh_view(&next).await;
        }
    }

    /// Ctrl+Tab:切到同窗口的下一个标签(单标签 no-op)。
    async fn viewer_next_tab(self: &Arc<Self>, pid: Option<&str>) {
        let pid = match pid {
            Some(p) => p.to_string(),
            None => match self.foreground_page_id().await {
                Some(p) => p,
                None => return,
            },
        };
        let next = {
            let inner = self.inner.lock().await;
            inner.windows.iter().find_map(|w| {
                let idx = w.tabs.iter().position(|t| t == &pid)?;
                if w.tabs.len() < 2 {
                    return None;
                }
                Some(w.tabs[(idx + 1) % w.tabs.len()].clone())
            })
        };
        if let Some(next) = next {
            let _ = self.bring_to_front(&next).await;
            self.refresh_view(&next).await;
        }
    }

    /// 人类输入的目标页:查看器正在显示的页(跟随页)优先,回退前台页。
    ///
    /// 跟随页是「最近一次有意义交互」的落账(`push_frame`/工具动作/人工工具栏
    /// 操作),人工的键鼠必然作用于他看到的那一页;侦察页在前台时也不会把
    /// 输入误投给看不见的页。
    pub(in crate::agent::browser) async fn input_target_page(&self) -> Option<String> {
        if let Some(ViewFollow::Page(id)) = self.view_follow() {
            let inner = self.inner.lock().await;
            if inner.tabs.contains_key(&id) {
                return Some(id);
            }
        }
        self.foreground_page_id().await
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
    ///
    /// 推帧即声明显示意图:跟随页在此同步落账(open/navigate 收尾、动作收尾、
    /// api.rs 各交互补泵点都经此),引擎直调路径(api::control)无需依赖
    /// 工具层落账也能让心跳跟上。
    pub(in crate::agent::browser) fn push_frame(&self, tab: &TabState) {
        self.set_view_follow(Some(ViewFollow::Page(tab.id.clone())));
        let alive = self
            .view
            .lock()
            .map(|g| g.as_ref().map(|s| s.is_alive()).unwrap_or(false))
            .unwrap_or(false);
        if !alive {
            // 查看器死亡处置:人工关窗(`viewer_closed` 信号)不重生(不与用户抢
            // 窗口);意外死亡(崩溃)自动重生(限次见 `ensure_view_process`)并
            // 照常补推当前帧——此前死后窗口永不再现,任务只剩纯无头
            // (2026-11-04 查看器崩溃事故)。无头页不启查看器。
            if !tab.mode.is_headed()
                || self
                    .view_user_closed
                    .load(std::sync::atomic::Ordering::Relaxed)
            {
                return;
            }
            let title = format!("🌐 {} · {} — {}", tab.id, tab.title, tab.url);
            self.ensure_view_process(tab.viewport.width, tab.viewport.height, &title);
            let alive = self
                .view
                .lock()
                .map(|g| g.as_ref().map(|s| s.is_alive()).unwrap_or(false))
                .unwrap_or(false);
            if !alive {
                return;
            }
        }
        // 站点加载/风控遮罩软化(查看器帧**常开**,不再限用户控制态):
        // 整幅不透明遮罩(实测猎聘 `/zhaopin/` 的 `.searchjobs-spin-loading-box`
        // 白底 z=9999,搜索 API 400 → React loading 永真 → 遮罩永驻)会让人工
        // 只看到一片白屏(2026-11-04 真机报障「页面显示还是空白」;此前仅
        // `request_human` 用户控制态软化,Agent 工作期无人管)。
        // 查看器不是取证面:命中遮罩才克隆布局,把遮罩降到 25% 白纱 + 琥珀
        // 提示胶囊;取证截图(`screenshot_locked`)不经此处,证据图保持站点原貌。
        // 检测先行:无遮罩零克隆零额外开销。
        let mut overlay_softened = 0usize;
        let has_overlay = inspect::detect_loading_overlay(
            &tab.layout.display,
            tab.viewport.width as f64,
            tab.viewport.height as f64,
        )
        .is_some();
        let rendered = if has_overlay {
            let mut layout = tab.layout.clone();
            overlay_softened = page_bars::soften_loading_overlays(
                &mut layout.display,
                tab.viewport.width as f64,
                tab.viewport.height as f64,
            );
            self.font.render_pixmap(
                &layout,
                tab.viewport.width,
                tab.viewport.height,
                tab.scroll_x,
                tab.scroll_y,
                false,
                &tab.images,
            )
        } else {
            self.font.render_pixmap(
                &tab.layout,
                tab.viewport.width,
                tab.viewport.height,
                tab.scroll_x,
                tab.scroll_y,
                false,
                &tab.images,
            )
        };
        let Some(mut pixmap) = rendered else {
            return;
        };
        // 屏蔽页面状态栏/进度栏:查看器帧合成层叠加(不进截图取证路径)。
        page_bars::draw(&mut pixmap, &self.font, tab);
        if overlay_softened > 0 {
            page_bars::draw_overlay_note(&mut pixmap, &self.font);
        }
        // 页面查找(Ctrl/Cmd+F):匹配高亮 + 查找栏,同在帧合成层(39 号 P2c)。
        crate::agent::browser::find::draw(&mut pixmap, &self.font, tab);
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
        // 收口清理:自动重生预算清零、人工关窗旗标复位,供后续全新任务重新计数。
        self.view_respawns
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.view_user_closed
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut guard) = self.view.lock() {
            if let Some(sink) = guard.take() {
                sink.send_close();
            }
        }
    }

    /// 有头模式下复活查看器(幂等,供工具层调用)。
    ///
    /// 查看器被人工误关/子进程崩溃后,登录/验证码类 HITL 要求人工在窗口里
    /// 操作,但窗口已经不在了。此前 request_human 只能如实提示「隐藏模式,
    /// 人工无法操作」;现在工具层先尝试复活,复活成功弹窗照常指向窗口,
    /// 只有真·无头页面才走隐藏模式提示(2026-10-30 京东登录链实测)。
    pub async fn revive_viewer(&self) -> bool {
        let Some(page_id) = self.foreground_page_id().await else {
            return false;
        };
        let inner = self.inner.lock().await;
        match inner.tabs.get(&page_id) {
            Some(tab) if tab.mode.is_headed() => {
                self.ensure_view(tab);
                true
            }
            _ => false,
        }
    }
}
