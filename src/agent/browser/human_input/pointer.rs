//! 指针事件统一与 hover 兼容事件(39 号设计文档 P2d)。
//!
//! Chrome 的鼠标输入会同时派发 **PointerEvent 与 MouseEvent 兼容事件**
//! (pointer 事件先行)。现代图表/拖拽库(ECharts、Chart.js、d3、swiper)
//! 特性检测 `window.PointerEvent` 后**只挂 pointer 监听**——内核此前只派发
//! mouse 系事件,这类站点的人工交互整条链静默失效。本模块补齐:
//!
//! * `pointerdown/pointermove/pointerup`(与 mouse 事件同载荷 + pointer 字段);
//! * hover 目标切换时的 `pointerout/mouseout`(旧目标)与
//!   `pointerover/mouseover`(新目标)——JS 驱动的悬停菜单/tooltip 可用;
//! * `wheel` 事件派发(`preventDefault` 阻止原生滚动,Chrome 语义)。
//!
//! 事件对象仍是 PRELUDE `__mkEvent` 普通对象(shallow merge):页面读
//! `e.pointerType/e.pointerId` 可用,`instanceof PointerEvent` 与 mouse
//! 事件同样为 false(既有边界,不扩大)。PointerEvent 构造器 shim 在
//! `script/env_js.rs`,捕获语义的防崩 stub 也在 PRELUDE(见
//! `script/prelude.rs` 的 `setPointerCapture`)。

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

/// 成对批量派发(带 2s 上限,与手势处理器同口径)。
async fn fire_seq(
    mgr: &LsmBrowserManager,
    pid: &str,
    selector: &str,
    events: Vec<(&'static str, Option<Value>)>,
) {
    if let Some(reply) = api::js_dispatch_seq_bounded(mgr, pid, selector, events).await {
        api::apply_js_reply(mgr, pid, &reply).await;
    }
}

impl LsmBrowserManager {
    /// hover 目标切换:旧目标 `pointerout`+`mouseout`,新目标
    /// `pointerover`+`mouseover`(指针事件先于 mouse 兼容事件,WHATWG 顺序)。
    pub(super) async fn dispatch_hover_change(
        &self,
        pid: &str,
        old: Option<&str>,
        new: Option<&str>,
        x: f64,
        y: f64,
    ) {
        if old == new {
            return;
        }
        // 同目标的兼容事件对合并一次往返(指针事件先于 mouse 兼容事件)。
        if let Some(old_sel) = old {
            fire_seq(self, pid, old_sel, vec![
                ("pointerout", Some(mouse_payload(x, y, MouseButton::Left, false, 0))),
                ("mouseout", Some(mouse_payload(x, y, MouseButton::Left, false, 0))),
            ])
            .await;
        }
        if let Some(new_sel) = new {
            fire_seq(self, pid, new_sel, vec![
                ("pointerover", Some(mouse_payload(x, y, MouseButton::Left, false, 0))),
                ("mouseover", Some(mouse_payload(x, y, MouseButton::Left, false, 0))),
            ])
            .await;
        }
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
}
