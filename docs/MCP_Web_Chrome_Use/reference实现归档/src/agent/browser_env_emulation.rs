//! 无头档指针/悬停媒体特性与触摸点的**原生 CDP 环境仿真**（第 182 轮）。
//!
//! 背景（第 182 轮本机真机双档实测，Chrome 155 / Windows 11，完整数据见
//! `docs/MCP_Web_Chrome_Use/36` §一）：
//! - `matchMedia('(hover:hover)')` / `'(pointer:fine)'` / `'(any-hover:hover)'` /
//!   `'(any-pointer:fine)'` 四元：**有头档全 true，无头档全 false** —— 桌面 UA 下
//!   四元全 false 是无头环境的典型读数（风控一行即可判定「无鼠标」）。
//! - `navigator.maxTouchPoints`：**有头档 0，无头档 1** —— 桌面 UA 却报支持触摸
//!   同样指向无头环境。
//!
//! 处置刻意走 **CDP 原生仿真**而非 JS 覆写（与 26 轮「注入面即签名」总纲一致）：
//! **零注入面** —— 不碰 `window.matchMedia`、无 `Function.prototype.toString` 登记
//! 负担、无描述符形态风险、不进 `patched_fn_leak` 登记面；服务端也看不到任何差异。
//! 真机验证：无头档下发后四元全 true、`maxTouchPoints=0`，与有头档基线逐一对齐。
//!
//! 从 `browser_ns.rs` 拆出（该文件已达临界行数）：本模块与注入命名空间无关，
//! 纯 CDP 仿真自成一体。connect 模式零注入契约由调用方保证（全部调用点都在
//! launch 路径内）。

use crate::agent::browser_mode::BrowserMode;
use crate::agent::stealth::StealthMode;
use chromiumoxide::cdp::browser_protocol::emulation::{
    MediaFeature, SetEmulatedMediaParams, SetTouchEmulationEnabledParams,
};

/// 环境仿真动作决策（第 182 轮，纯函数单测锁死）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerEnvAction {
    /// 无头档 + 开启档：仿真桌面指针环境（四元媒体特性 + 触摸归零）。
    Apply,
    /// 关闭档：复位仿真（清空媒体特性 + 恢复触摸缺省态），排障后不留残余处置。
    Reset,
    /// 有头档：原生读数即真值，零动作（不动触控屏本机的真实形态）。
    Skip,
}

/// 决策唯一真源：有头档恒 `Skip`；无头档按档位 `Apply`/`Reset`。
pub(crate) fn pointer_env_action(
    mode: StealthMode,
    browser_mode: BrowserMode,
) -> PointerEnvAction {
    match (mode, browser_mode) {
        (_, BrowserMode::Headed) => PointerEnvAction::Skip,
        (StealthMode::Fidelity | StealthMode::Hardened, _) => PointerEnvAction::Apply,
        (StealthMode::Off, _) => PointerEnvAction::Reset,
    }
}

/// 下发指针/悬停媒体特性仿真 + 触摸点仿真（见模块文档）。全部幂等，可重复调用。
///
/// 门控见 [`pointer_env_action`]；全部 CDP 调用 5s 有界超时 fail-open
/// （同 `browser::apply_screen_sync_override` 语义）：仿真缺失只损反检测收益，
/// 绝不拖住导航。
pub async fn apply_pointer_env_emulation(
    page: &chromiumoxide::Page,
    mode: StealthMode,
    browser_mode: BrowserMode,
) {
    let (media, touch_enabled) = match pointer_env_action(mode, browser_mode) {
        PointerEnvAction::Skip => return,
        // 无头档 + 开启档：仿真桌面指针环境。
        PointerEnvAction::Apply => (
            vec![
                MediaFeature::new("hover", "hover"),
                MediaFeature::new("any-hover", "hover"),
                MediaFeature::new("pointer", "fine"),
                MediaFeature::new("any-pointer", "fine"),
            ],
            false,
        ),
        // 关闭档：复位（清空仿真特性 + 恢复触摸仿真缺省态）。
        PointerEnvAction::Reset => (Vec::new(), true),
    };
    let media_params = SetEmulatedMediaParams::builder().features(media).build();
    match tokio::time::timeout(std::time::Duration::from_secs(5), page.execute(media_params)).await
    {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!("指针媒体仿真下发失败(fail-open): {e}"),
        Err(_) => tracing::warn!("指针媒体仿真超时(5s,fail-open)"),
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        page.execute(SetTouchEmulationEnabledParams::new(touch_enabled)),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!("触摸点仿真下发失败(fail-open): {e}"),
        Err(_) => tracing::warn!("触摸点仿真超时(5s,fail-open)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 第 182 轮:指针/触摸环境仿真门控锁死 —— 有头档恒零动作(不动触控屏真机形态),
    /// 无头档开启=Apply、关闭=Reset。这是「有头原生即真值」真机基线的代码级断言。
    #[test]
    fn 指针环境仿真门控() {
        for m in [StealthMode::Off, StealthMode::Fidelity, StealthMode::Hardened] {
            assert_eq!(
                pointer_env_action(m, BrowserMode::Headed),
                PointerEnvAction::Skip,
                "有头档必须零动作(原生读数即真值)"
            );
        }
        for b in [BrowserMode::Hidden, BrowserMode::NewHeadless] {
            assert_eq!(
                pointer_env_action(StealthMode::Hardened, b),
                PointerEnvAction::Apply
            );
            assert_eq!(
                pointer_env_action(StealthMode::Fidelity, b),
                PointerEnvAction::Apply
            );
            assert_eq!(
                pointer_env_action(StealthMode::Off, b),
                PointerEnvAction::Reset,
                "关闭档必须复位仿真,排障后不留残余处置"
            );
        }
    }
}
