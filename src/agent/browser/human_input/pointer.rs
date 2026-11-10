//! 指针事件统一与 hover 兼容事件(39 号设计文档 P2d/P2e)。
//!
//! Chrome 的鼠标输入会同时派发 **PointerEvent 与 MouseEvent 兼容事件**
//! (pointer 事件先行)。现代图表/拖拽库(ECharts、Chart.js、d3、swiper)
//! 特性检测 `window.PointerEvent` 后**只挂 pointer 监听**——内核此前只派发
//! mouse 系事件,这类站点的人工交互整条链静默失效。本模块补齐:
//!
//! * `pointerdown/pointermove/pointerup`(与 mouse 事件同载荷 + pointer 字段);
//! * hover 目标切换的完整 Chrome 事件序(P2e):out/leave×N/over/enter×N
//!   × pointer+mouse 两轮,`leave/enter` 为**不冒泡**边界事件(`__fireNoBubble`),
//!   派发目标/冒泡性逐事件不同,走 `js_dispatch_multi_bounded` 单次往返;
//! * pointer capture 真语义(P2e):页面脚本 `setPointerCapture` 后,
//!   pointer 系事件重定向到捕获元素(`pointer_capture` 动作 → TabState),
//!   got/lostpointercapture 在状态转换点补发,pointerup 隐式释放;
//! * `wheel` 事件派发(`preventDefault` 阻止原生滚动,Chrome 语义)。
//!
//! 事件对象仍是 PRELUDE `__mkEvent` 普通对象(shallow merge):页面读
//! `e.pointerType/e.pointerId` 可用,`instanceof PointerEvent` 与 mouse
//! 事件同样为 false(既有边界,不扩大)。PointerEvent 构造器 shim 在
//! `script/env_js.rs`。

use super::*;

/// pointer 载荷:mouse 载荷 + PointerEvent 字段(单鼠标指针固定 pointerId=1)。
pub fn pointer_payload(
    x: f64,
    y: f64,
    button: MouseButton,
    pressed: bool,
    detail: i64,
) -> Value {
    let mut p = mouse_payload(x, y, button, pressed, detail);
    p["pointerId"] = json!(1);
    p["pointerType"] = json!("mouse");
    p["isPrimary"] = json!(true);
    // 鼠标指针:按下 0.5、悬停/抬起 0(规范对鼠标的建议值)。
    p["pressure"] = json!(if pressed { 0.5 } else { 0.0 });
    p["width"] = json!(1);
    p["height"] = json!(1);
    p["tiltX"] = json!(0);
    p["tiltY"] = json!(0);
    p
}

/// P2e:hover 切换的**边界事件序列**(纯函数,单测覆盖)。
///
/// Chrome 顺序(hover 链 old → new):
/// `pointerout(old)` → `pointerleave(被离开链,自内向外)` →
/// `pointerover(new)` → `pointerenter(新进入链,自内向外)` →
/// `mouseout` → `mouseleave` → `mouseover` → `mouseenter`。
///
/// * 被离开/新进入链 = **链差集**(target→root 序);公共祖先不派发 ——
///   同一祖先内移动不触发它的 enter/leave,这正是 jQuery/Bootstrap 菜单
///   依赖的语义;
/// * 移入后代:old 在 new 链内 → 只发 out(不发 leave);移入祖先:反向对称;
/// * 从空白进入:old 链空 → 只发 over+enter;退到空白:反向对称;
/// * `leave/enter` 不冒泡(true 位),`out/over` 冒泡。
///
/// 返回 `(selector, event, 不冒泡)`;载荷由调用方按 pointer/mouse 前缀补。
pub fn boundary_event_seq(old_chain: &[String], new_chain: &[String]) -> Vec<(String, &'static str, bool)> {
    let left: Vec<String> = old_chain
        .iter()
        .filter(|s| !new_chain.contains(s))
        .cloned()
        .collect();
    let entered: Vec<String> = new_chain
        .iter()
        .filter(|s| !old_chain.contains(s))
        .cloned()
        .collect();
    let mut out: Vec<(String, &'static str, bool)> = Vec::new();
    for (out_ev, leave_ev, over_ev, enter_ev) in [
        ("pointerout", "pointerleave", "pointerover", "pointerenter"),
        ("mouseout", "mouseleave", "mouseover", "mouseenter"),
    ] {
        if let Some(old) = old_chain.first() {
            out.push((old.clone(), out_ev, false));
        }
        for s in &left {
            out.push((s.clone(), leave_ev, true));
        }
        if let Some(new) = new_chain.first() {
            out.push((new.clone(), over_ev, false));
        }
        for s in &entered {
            out.push((s.clone(), enter_ev, true));
        }
    }
    out
}

impl LsmBrowserManager {
    /// hover 目标切换:派发 Chrome 完整事件序(P2e;out/over 冒泡,
    /// leave/enter 不冒泡,整条序列**一次 JS 往返** —— 逐事件往返的开销
    /// 会吃掉手势响应,旧实现 out/over 两对也要 2 次往返,新方案更省)。
    pub(super) async fn dispatch_hover_change(
        &self,
        pid: &str,
        old_chain: &[String],
        new_chain: &[String],
        x: f64,
        y: f64,
    ) {
        if old_chain == new_chain {
            return;
        }
        let events: Vec<(String, &str, Option<Value>, bool)> = boundary_event_seq(old_chain, new_chain)
            .into_iter()
            .map(|(sel, ev, no_bubble)| {
                let payload = if ev.starts_with("pointer") {
                    pointer_payload(x, y, MouseButton::Left, false, 0)
                } else {
                    mouse_payload(x, y, MouseButton::Left, false, 0)
                };
                (sel, ev, Some(payload), no_bubble)
            })
            .collect();
        if events.is_empty() {
            return;
        }
        if let Some(reply) = api::js_dispatch_multi_bounded(self, pid, events).await {
            api::apply_js_reply(self, pid, &reply).await;
        }
    }

    /// P2e:pointer capture 当前捕获元素(空 = 无)。
    pub(super) async fn pointer_capture_target(&self, pid: &str) -> Option<String> {
        let inner = self.inner.lock().await;
        inner.tabs.get(pid).and_then(|t| t.pointer_capture.clone())
    }

    /// P2e:捕获转换 reconcile —— 对比 JS 派发**前后**的捕获态,在转换点
    /// 补发 `gotpointercapture`/`lostpointercapture`(Chrome:对捕获元素
    /// 派发;换目标 = 旧元素先 lost、新元素后 got)。只在转换时多一次有界
    /// 派发,常态零开销。
    pub(super) async fn reconcile_pointer_capture(
        &self,
        pid: &str,
        before: Option<String>,
    ) {
        let after = self.pointer_capture_target(pid).await;
        if before == after {
            return;
        }
        if let Some(old) = before.as_deref() {
            if let Some(reply) =
                api::js_dispatch_bounded(self, pid, old, "lostpointercapture").await
            {
                api::apply_js_reply(self, pid, &reply).await;
            }
        }
        if let Some(new) = after.as_deref() {
            if let Some(reply) =
                api::js_dispatch_bounded(self, pid, new, "gotpointercapture").await
            {
                api::apply_js_reply(self, pid, &reply).await;
            }
        }
    }

    /// P2e:pointerup 隐式释放捕获(pointerup/mouseup 已按捕获元素派发后调用)。
    pub(super) async fn release_pointer_capture_implicit(&self, pid: &str) -> Option<String> {
        let cap = self.pointer_capture_target(pid).await?;
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                if tab.pointer_capture.as_deref() == Some(cap.as_str()) {
                    tab.pointer_capture = None;
                }
            }
        }
        if let Some(reply) = api::js_dispatch_bounded(self, pid, &cap, "lostpointercapture").await {
            api::apply_js_reply(self, pid, &reply).await;
        }
        Some(cap)
    }

    /// wheel 事件派发 + `preventDefault` 门:页面监听器拦截时返回 `true`
    /// (调用方跳过原生滚动)。短 bound(150ms):滚动是高频手势,页面
    /// 线程卡顿时不能拖死整条滚动链;超时按未拦截处理(保底可滚)。
    pub(super) async fn dispatch_wheel_event(&self, pid: &str, sel: &str, x: f64, y: f64, dx: f64, dy: f64) -> bool {
        let payload = json!({
            "clientX": x.round(), "clientY": y.round(),
            "screenX": x.round(), "screenY": y.round(),
            "deltaX": (dx * WHEEL_STEP).round(),
            "deltaY": (dy * WHEEL_STEP).round(),
            "deltaMode": 0, "button": 0, "buttons": 0,
            "cancelable": true,
        });
        let reply = tokio::time::timeout(
            std::time::Duration::from_millis(WHEEL_JS_BOUND_MS),
            api::js_dispatch(self, pid, sel, "wheel", Some(payload)),
        )
        .await
        .unwrap_or(None);
        let Some(reply) = reply else {
            return false;
        };
        let prevented = reply.default_prevented;
        api::apply_js_reply(self, pid, &reply).await;
        prevented
    }
}

/// wheel JS 派发的超时上限(毫秒)。
///
/// 与 `js_dispatch_bounded_with` 的 2s 同口径:dispatch 的固有往返
/// (快照 refresh + eval + 事件循环泵)在 debug/高负载下可远超数百毫秒,
/// 上限太小会把正常往返判成超时,preventDefault 语义失效;超时按未拦截
/// 处理(保底可滚,慢监听器不拖死滚动链)。
pub(super) const WHEEL_JS_BOUND_MS: u64 = 2000;

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pointer载荷_含指针字段且随按下态变化() {
        let down = pointer_payload(10.0, 20.0, MouseButton::Left, true, 1);
        assert_eq!(down["pointerId"], json!(1));
        assert_eq!(down["pointerType"], json!("mouse"));
        assert_eq!(down["isPrimary"], json!(true));
        assert_eq!(down["pressure"], json!(0.5));
        assert_eq!(down["width"], json!(1));
        assert_eq!(down["clientX"], json!(10.0));
        // 抬起/悬停:pressure 归零,坐标与 mouse 字段保留。
        let up = pointer_payload(11.0, 22.0, MouseButton::Left, false, 1);
        assert_eq!(up["pressure"], json!(0.0));
        assert_eq!(up["clientY"], json!(22.0));
        assert_eq!(up["button"], json!(0));
    }

    #[test]
    fn 边界事件序列_切换兄弟元素_完整八事件序() {
        // div > a(old) 切到 div > b(new):公共祖先 div 不派发 leave/enter。
        let old = chain(&["a", "div", "body"]);
        let new = chain(&["b", "div", "body"]);
        let seq = boundary_event_seq(&old, &new);
        let flat: Vec<&str> = seq.iter().map(|(s, e, _)| {
            let _ = s;
            *e
        }).collect();
        // Chrome 顺序:pointerout → pointerleave(a) → pointerover →
        // pointerenter(b) → mouseout → mouseleave(a) → mouseover → mouseenter(b)。
        assert_eq!(
            flat,
            vec![
                "pointerout", "pointerleave", "pointerover", "pointerenter",
                "mouseout", "mouseleave", "mouseover", "mouseenter"
            ]
        );
        // 目标与不冒泡位。
        assert_eq!(seq[0].0, "a");
        assert!(!seq[0].2, "out 冒泡");
        assert_eq!(seq[1].0, "a");
        assert!(seq[1].2, "leave 不冒泡");
        assert_eq!(seq[2].0, "b");
        assert_eq!(seq[3].0, "b");
        assert!(seq[3].2);
        // 公共祖先(div/body)不在序列里。
        assert!(seq.iter().all(|(s, _, _)| s != "div" && s != "body"));
    }

    #[test]
    fn 边界事件序列_移入后代_祖先只发out不发leave() {
        let old = chain(&["div", "body"]);
        let new = chain(&["p", "div", "body"]);
        let seq = boundary_event_seq(&old, &new);
        let names: Vec<&str> = seq.iter().map(|(_, e, _)| *e).collect();
        assert_eq!(
            names,
            vec![
                "pointerout", "pointerover", "pointerenter", // p 新进入
                "mouseout", "mouseover", "mouseenter"
            ]
        );
        // div 是公共祖先:不发 leave/enter,但 out/over 都以它为目标。
        assert_eq!(seq[0].0, "div");
        assert_eq!(seq[1].0, "p");
        assert_eq!(seq[2].0, "p");
    }

    #[test]
    fn 边界事件序列_移入祖先_不发enter() {
        let old = chain(&["p", "div", "body"]);
        let new = chain(&["div", "body"]);
        let seq = boundary_event_seq(&old, &new);
        let names: Vec<&str> = seq.iter().map(|(_, e, _)| *e).collect();
        // p 被离开:out+leave;div 已在旧链内:只发 over,不发 enter。
        assert_eq!(
            names,
            vec!["pointerout", "pointerleave", "pointerover", "mouseout", "mouseleave", "mouseover"]
        );
        assert_eq!(seq[0].0, "p");
        assert_eq!(seq[1].0, "p");
        assert_eq!(seq[2].0, "div");
    }

    #[test]
    fn 边界事件序列_退到空白与从空白进入() {
        // 退到空白:只发 out + leave 链(每个被离开的祖先各一条,自内向外),
        // 无 over/enter。
        let seq = boundary_event_seq(&chain(&["a", "div"]), &[]);
        let names: Vec<&str> = seq.iter().map(|(_, e, _)| *e).collect();
        assert_eq!(
            names,
            vec!["pointerout", "pointerleave", "pointerleave", "mouseout", "mouseleave", "mouseleave"]
        );
        assert_eq!(seq[1].0, "a");
        assert_eq!(seq[2].0, "div");
        // 从空白进入:只发 over + enter 链(整条新链,每个新进入的祖先各一条),
        // 无 out/leave。
        let seq = boundary_event_seq(&[], &chain(&["a", "div"]));
        let names: Vec<&str> = seq.iter().map(|(_, e, _)| *e).collect();
        assert_eq!(
            names,
            vec!["pointerover", "pointerenter", "pointerenter", "mouseover", "mouseenter", "mouseenter"]
        );
        assert_eq!(seq[1].0, "a");
        assert_eq!(seq[2].0, "div");
    }
}
