//! 页面管控 control_action 子模块(第 143 轮自 control.rs 机械拆分)。
//!
//! 收纳「页面管控三档」在 control 层的全部落点(control.rs 逼近 1800 行硬线,
//! 按 CLAUDE.md 拆分规范把同职责函数搬到职责子模块,逐行机械搬移零改写):
//! - 管控让路辅助:locked「先解后锁」/ partial「先隐盾后复盾」(第 141/143 轮);
//! - `set_overlay`(第 141 轮 legacy 别名)与 `set_guard`(第 143 轮三档切换)。

use super::*;

// =================== 第 141/143 轮:管控让路「先解后锁 / 先隐盾后复盾」辅助 ===================

/// 输入类动作前的让路方式。
#[derive(Debug, Clone, Copy)]
pub(super) enum GuardLiftKind {
    /// locked 档(第 155 轮缺省):DOM 输入闸挂起(合成输入通过),动作后复位。
    GateSuspend,
    /// locked 档(闸不在场:杀开关/注入失败):CDP 输入解锁
    /// (`setIgnoreInputEvents=false`),动作后复锁(第 141 轮路径)。
    InputUnlock,
    /// partial 档:盾区挂起(隐藏盾罩,Agent 的 CDP 点击不被吞),动作后复盾。
    ShieldsSuspend,
}

/// 输入类动作前的让路:按当前管控档分流(第 141 轮解锁,第 143 轮隐盾,
/// 第 155 轮 locked 优先 DOM 闸挂起、闸不在场回退 CDP 解锁)。
///
/// - locked(headed):`__laewGuardInputSuspend(true)` 挂起输入闸;闸不在场
///   (杀开关关闭/脚本未注入)才走 `Input.setIgnoreInputEvents=false` 回退路径;
/// - partial(headed 非 connect):`__laewGuardSuspend(true)` 隐藏盾罩;
/// - open / hidden / connect / page_id 失效 → None(零开销;后者由动作自身报 2000)。
/// fail-open:让路失败也不阻断动作(等同未激活,动作照常执行)。
pub(super) async fn guard_lift_for_input(
    id: &str,
) -> Option<(chromiumoxide::Page, GuardLiftKind)> {
    use crate::agent::browser_overlay::PageGuardMode;
    let mgr = crate::agent::browser::BrowserManager::global();
    let mode = mgr.current_guard().await.mode;
    let page = mgr.page(id).await?;
    match mode {
        PageGuardMode::Locked if mgr.overlay_active().await => {
            if crate::agent::browser_overlay::gate_suspend_input(&page, true).await {
                Some((page, GuardLiftKind::GateSuspend))
            } else {
                // DOM 闸不在场(杀开关/注入失败)→ 第 141 轮 CDP 解锁回退路径
                use chromiumoxide::cdp::browser_protocol::input::SetIgnoreInputEventsParams;
                let _ = page.execute(SetIgnoreInputEventsParams::new(false)).await;
                Some((page, GuardLiftKind::InputUnlock))
            }
        }
        PageGuardMode::Partial if mgr.guard_visuals_active().await => {
            if crate::agent::browser_overlay::guard_suspend_visuals(&page, true).await {
                Some((page, GuardLiftKind::ShieldsSuspend))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 输入类动作后的恢复(fail-open:失败只记日志,不影响动作结果语义;
/// 下一个输入动作的「先解/先隐」会自愈残余状态)。
pub(super) async fn guard_restore_after_input(
    page: &chromiumoxide::Page,
    kind: GuardLiftKind,
) {
    match kind {
        GuardLiftKind::GateSuspend => {
            if !crate::agent::browser_overlay::gate_suspend_input(page, false).await {
                tracing::warn!("输入闸复位失败(fail-open;下一动作先解时自愈)");
            }
        }
        GuardLiftKind::InputUnlock => {
            use chromiumoxide::cdp::browser_protocol::input::SetIgnoreInputEventsParams;
            if let Err(e) = page.execute(SetIgnoreInputEventsParams::new(true)).await {
                tracing::warn!("蒙层输入复锁失败(fail-open):{e}");
            }
        }
        GuardLiftKind::ShieldsSuspend => {
            if !crate::agent::browser_overlay::guard_suspend_visuals(page, false).await {
                tracing::warn!("盾区恢复失败(fail-open)");
            }
        }
    }
}

/// control_action=set_overlay(第 141 轮 legacy):运行时开关可视化蒙层,
/// 第 143 轮起委托 `set_guard`(true=locked / false=open),响应附 deprecation 提示。
pub(super) async fn act_set_overlay(
    id: &str,
    p: &Value,
) -> std::result::Result<Value, String> {
    let enabled = p.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    crate::agent::browser::BrowserManager::global()
        .set_overlay(id, enabled)
        .await
}

/// control_action=set_guard(第 143 轮):运行时切换页面管控三档。
///
/// params:{mode?: locked/open/partial, allow_selectors?: [...], block_selectors?: [...],
/// note?: "..." } —— 任一字段缺省 = 保持现值,空数组/空串 = 清除;partial 的
/// 选择器联合校验同 open(互斥/必给其一/上限)。参数错误走 1001 信封
/// (不走通用 2002,便于 LLM 机械修正),page_id 失效走 2000。
pub(super) async fn act_set_guard(id: &str, p: &Value) -> crate::error::Result<String> {
    use crate::agent::browser_overlay::PageGuardMode;
    let mgr = BrowserManager::global();
    if mgr.page(id).await.is_none() {
        return envelope(2000, "page_id 不存在", json!({"page_id": id}));
    }
    let mut cfg = mgr.current_guard().await;
    if let Some(m) = str_arg(p, "mode") {
        match PageGuardMode::from_arg(m) {
            Some(mode) => cfg.mode = mode,
            None => {
                return envelope(
                    1001,
                    &format!("非法 mode「{m}」(允许 locked/open/partial)"),
                    json!({"mode": m, "allowed": ["locked", "open", "partial"]}),
                )
            }
        }
    }
    if let Some(list) = str_array_param(p, "allow_selectors") {
        cfg.allow_selectors = list;
    }
    if let Some(list) = str_array_param(p, "block_selectors") {
        cfg.block_selectors = list;
    }
    if let Some(n) = p.get("note").and_then(Value::as_str) {
        let t = n.trim();
        cfg.note = if t.is_empty() { None } else { Some(t.to_string()) };
    }
    // 「字段缺省=保持现值」与档位校验的衔接:切到 locked/open 时,上一个 partial
    // 档遗留的选择器对非 partial 档无意义,自动清除(否则 set_guard(mode="locked")
    // 会被「locked 不收选择器」卡死,真机端到端实测踩中)。
    if cfg.mode != PageGuardMode::Partial {
        cfg.allow_selectors.clear();
        cfg.block_selectors.clear();
    }
    let cfg = cfg.normalized();
    if let Err(e) = cfg.validate() {
        return envelope(
            1001,
            &e,
            json!({
                "mode": cfg.mode.as_str(),
                "allow_selectors": cfg.allow_selectors,
                "block_selectors": cfg.block_selectors,
                "hint": "partial 需要 allow_selectors(白名单)或 block_selectors(黑名单)之一;两者互斥;locked/open 不收选择器",
            }),
        );
    }
    match mgr.set_guard(id, cfg.clone()).await {
        Ok(mut v) => {
            // 第 157 轮:open 档响应附收口提示 —— 实测文心任务 Agent 因输入链路
            // 报错误判「锁定拦截输入」自行开档后未收口,人工看到页面可编辑且
            // 无任何提示。提示 Agent 人工操作完成后恢复 locked,减少观感落差。
            if cfg.mode == PageGuardMode::Open {
                if let Value::Object(ref mut m) = v {
                    m.insert("note".into(), Value::String(
                        "页面已对人工开放(人工可直接操作)。人工操作完成后建议 set_guard(mode=\"locked\") 恢复管控;如仅为排查输入失败,注意 locked 档不会拦截 Agent 输入(输入动作执行期自动挂闸)".to_string(),
                    ));
                }
            }
            envelope(0, "ok", v)
        }
        Err(e) => envelope(2002, &e, json!({})),
    }
}

/// 从 params 提取字符串数组(`None` = 未提供保持现值;数组 = 整体替换)。
fn str_array_param(p: &Value, key: &str) -> Option<Vec<String>> {
    let a = p.get(key)?.as_array()?;
    Some(
        a.iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
    )
}
