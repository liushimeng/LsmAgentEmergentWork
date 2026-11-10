//! macos 查看器单元测试(从 `macos.rs` 的 `mod tests` 机械搬移)。

use super::*;


    #[test]
    fn mac特殊键映射_核心键位() {
        assert_eq!(mac_special_key(36), Some("Enter"));
        assert_eq!(mac_special_key(76), Some("Enter"), "小键盘回车同语义");
        assert_eq!(mac_special_key(48), Some("Tab"));
        assert_eq!(mac_special_key(53), Some("Escape"));
        assert_eq!(mac_special_key(51), Some("Backspace"));
        assert_eq!(mac_special_key(117), Some("Delete"));
        assert_eq!(mac_special_key(123), Some("ArrowLeft"));
        assert_eq!(mac_special_key(126), Some("ArrowUp"));
        assert_eq!(mac_special_key(115), Some("Home"));
        assert_eq!(mac_special_key(121), Some("PageDown"));
        // 可打印字符无特殊映射(走 charactersIgnoringModifiers)。
        assert_eq!(mac_special_key(0), None);
    }

    #[test]
    fn key字段转义_引号反斜杠与控制字符() {
        assert_eq!(key_json_str("a"), "a");
        assert_eq!(key_json_str("\""), "\\\"");
        assert_eq!(key_json_str("\\"), "\\\\");
        assert_eq!(key_json_str("A\r"), "A", "控制字符被剔除");
    }

    /// 事件处理链发送的全部实例选择器必须真实存在(与 SDK 对照)。
    ///
    /// 发不存在的选择器 = unrecognized selector → NSException(foreign exception)
    /// 穿越 nounwind 回调 → 整个查看器 abort(2026-11-04 `hasPreciseDeltas`
    /// 误拼事故的回归防线——该测试在事故前就应当存在)。用
    /// `+NSObject instancesRespondToSelector:` 查询,无需 GUI/NSApp,cargo test 可跑。
    #[test]
    fn 事件处理链选择器全部真实存在() {
        unsafe {
            assert!(ensure_frameworks(), "装载 Foundation/AppKit 失败");
            let nsevent = cls(b"NSEvent\0");
            assert!(!nsevent.is_null(), "拿不到 NSEvent 类");
            for name in [
                &b"locationInWindow\0"[..],
                &b"hasPreciseScrollingDeltas\0"[..],
                &b"deltaX\0"[..],
                &b"deltaY\0"[..],
                &b"buttonNumber\0"[..],
                &b"modifierFlags\0"[..],
                &b"keyCode\0"[..],
                &b"charactersIgnoringModifiers\0"[..],
            ] {
                let ok = send_i64_sel(
                    nsevent,
                    sel(b"instancesRespondToSelector:\0"),
                    sel(name),
                );
                assert!(
                    ok != 0,
                    "NSEvent 不响应选择器 {:?} —— 发出去就是 unrecognized selector 崩溃",
                    String::from_utf8_lossy(name)
                );
            }
            let nsview = cls(b"NSView\0");
            assert!(!nsview.is_null(), "拿不到 NSView 类");
            for name in [
                &b"convertPoint:fromView:\0"[..],
                &b"setFrame:\0"[..],
                &b"addTrackingArea:\0"[..],
            ] {
                let ok = send_i64_sel(
                    nsview,
                    sel(b"instancesRespondToSelector:\0"),
                    sel(name),
                );
                assert!(
                    ok != 0,
                    "NSView 不响应选择器 {:?}",
                    String::from_utf8_lossy(name)
                );
            }
        }
    }

    /// `respond` 守卫本身:实例上存在的选择器返回 true,瞎拼/不存在的返回
    /// false(而不是发出去崩掉),空对象安全。
    #[test]
    fn 选择器守卫_存在与否() {
        unsafe {
            assert!(ensure_frameworks(), "装载 Foundation/AppKit 失败");
            let obj = send_id(send_id(cls(b"NSObject\0"), sel(b"alloc\0")), sel(b"init\0"));
            assert!(!obj.is_null(), "NSObject 实例创建失败");
            assert!(respond(obj, b"description\0"), "存在选择器应返回 true");
            assert!(
                !respond(obj, b"hasPreciseDeltas\0"),
                "误拼旧名必须返回 false(而不是发出后 unrecognized selector 崩溃)"
            );
            assert!(!respond(std::ptr::null_mut(), b"description\0"), "空对象不响应");
        }
    }
