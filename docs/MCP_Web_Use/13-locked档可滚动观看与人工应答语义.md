# 13 · locked 档「可滚动观看」+ HITL 应答语义分流（第 155 轮）

> 唯一最新版。前置：doc 05（可视化模式蒙层与人工操作拦截，第 141 轮）、doc 07（页面管控三档，第 143 轮）、doc 12（产物锚定与人工核验闭环，第 152 轮）。
> 实现：`src/agent/browser_overlay.rs`（DOM 输入闸）、`src/agent/tools/mcp_web_use/guard_ctl.rs`（让路分流）、`src/agent/browser_follow.rs`（诊断提示）、`src/agent/tools/mcp_web_use/hitl_hint.rs`（应答语义）。

## 1. 背景：本轮实测输入与核对结论

用户 2026-10-10 17:33 豆包实测（`laew --debug`，二进制编译于 2026-10-10 17:32:17）。**该二进制早于第 152~154 轮提交**（横幅无「产物目录：」行可证），报告中的多数问题已在第 152 轮根治，核对结论：

| 实测报告 | HEAD 现状 |
|---------|----------|
| Plan 文件在工作目录找不到（逃逸到 `/opt/homebrew/bin`） | 第 152 轮 `artifact_root.rs` 已根治（八类产物锚定工作目录，`safe_join` 防逃逸） |
| 弹出很多空/乱验证码 Chrome 标签页 | 第 152 轮 ⑧⑨ 已根治（「查看原图」改弹窗内 1:1 放大；噪声标签页不收编直接关） |
| 拖拽挑战被识别为「验证码」 | 第 152 轮 ② 已根治（`ChallengeType`：拖拽/滑块/点选 → `manual_verify` + 人工操作说明） |
| 人工核验弹窗阻碍人工输入 | 第 152 轮 ⑤⑥ 已根治（HITL 输入挂起闸 4003 + 挑战弹层放行白名单） |
| 页面不滚动，人工看不到下面信息 | 第 152 轮 ⑩ 半根治（auto_follow 让页面停在最新）；**locked 档人工自己滚不动** → 本轮根治 |
| 人工可经弹窗输入提示词与 Agent 继续交互 | 通道已存在（第 145 轮「提交 / 继续」双语义 + 自由文本回填）；但 `next_hint` 对自由文本一律当验证码 → 本轮分流 |

## 2. locked 档「可滚动观看」（R155-A）

### 2.1 问题本质

locked 档 L1 用 CDP `Input.setIgnoreInputEvents(true)` 拦人工输入。该 API 是**页面级一刀切**——鼠标、键盘、**滚轮**全拦。第 141 轮选它的论证是「CDP 合成输入与真实输入在 hit-test 层不可区分（isTrusted 均 true），DOM 蒙层（pointer-events:auto）会把 Agent 自己的点击一并吃掉」。该论证只否定 **pointer-events 蒙层**；**事件监听器 + `isTrusted` 判据**没有这个缺陷：

| 事件来源 | `isTrusted` | 第 141 轮 CDP 拦截下 | 第 155 轮 DOM 闸下 |
|---------|------------|---------------------|-------------------|
| 真实人工输入 | `true` | 拦 | 拦 |
| CDP 合成输入（`Input.dispatch*`） | `true` | 拦（Agent 动作先解后锁） | 拦（Agent 动作先挂起闸） |
| 页面自合成（`el.click()` / `dispatchEvent`，eval_js 驱动） | `false` | **放行**（不进输入管线） | **放行**（闸判 isTrusted） |

Agent 能力面与第 141 轮完全等价。

### 2.2 实现

1. **闸**（`AGENT_GUARD_JS_TEMPLATE`，烘焙 `{SCROLLWATCH}`，导航自动重挂）：
   - 拦截清单（仅 `isTrusted=true` 且 mode=locked 且未挂起）：`mousedown/mouseup/click/dblclick/auxclick/contextmenu/pointerdown/pointerup/dragstart/drop/keydown/keyup/keypress/touchstart/touchend` → `stopImmediatePropagation() + preventDefault()`（window 捕获阶段最先触发）；
   - **放行**：`wheel/scroll/mousemove/pointermove/touchmove` —— 人工可用滚轮/触摸板自由翻阅观看；
   - 状态条 locked 文案改为「🔒 …页面已锁定(仅观看·可滚动）/ 人工可滚动浏览;点击/键盘已拦截…」。
2. **让路**（`guard_ctl::guard_lift_for_input` 单一收口不变）：locked 档优先 `__laewGuardInputSuspend(true)`（毫秒级窗口），闸不在场（杀开关/注入失败）回退第 141 轮 CDP 解锁路径。`GuardLiftKind` 增 `GateSuspend`。
3. **决策序**（`apply_page_guard`）：JS 设置 → 探测 `__laewGuardGateReady` → 闸在场则 CDP 显式 `setIgnoreInputEvents(false)`（清残留锁）；闸不在场（含 hidden/connect 无脚本页面）→ CDP `setIgnoreInputEvents(true)` **fail-closed 回退**。响应新增 `scroll_watch`（bool）与 `input_gate`（`dom_scrollable`/`cdp`/`none`）对账字段。
4. **杀开关**：`LAEW_WEB_LOCKED_SCROLL=off`（默认开启）→ 烘焙 false → 闸不安装 → 完整回退第 141 轮行为。
5. **诊断联动**（`browser_follow::visibility_note` 增 `human_scrollable` 参数）：可滚动模式下「不在底部」不再是缺陷——人工可能正在上翻阅读历史，提示改为「人工可自行滚动查看；不要强制吸底」，仍给出 `scroll_to`/`auto_follow` 可执行动作；仅回退档保留「人工无法自行滚动」旧文案。

### 2.3 与三档 / HITL 的协同

- 提问期间切 partial/open：闸按 mode 判定自动失效，人工在放行区自由操作（含滚动）；收口回 locked 闸自动恢复——零额外接线。
- `auto_follow` 语义不变（仅用户本就在底部才吸底）：人工上翻 → 跟随自动停；回到底部 → 恢复跟随。滚轮放行后「人工翻阅」与「页面跟新」第一次真正共存。

### 2.4 边界（如实记录）

- **跨域 iframe（OOPIF）内部人工点击闸不住**：主文档捕获监听收不到 OOPIF 内事件，管控脚本注入通道也只覆盖 target 内 frame。与第 144 轮「跨域验证码 iframe 必须放行给人工」语义一致，风险可接受；
- `mousemove` 放行：人工 hover 可触发悬停菜单（观看语义下的可接受副作用）；
- 闸挂起窗口（毫秒级）内人工点击可穿透：与既有 CDP「先解后锁」窗口等价；
- 触摸屏 `touchstart/touchend` 被拦：触摸滚动发起也被拦（桌面工具优先滚轮/触摸板 = wheel 通路）。

## 3. HITL 应答语义分流（R155-B）

`hitl_hint::hitl_answer_hint`：captcha/sms/two_factor + 自由文本时先判 `looks_like_code_text`（≤16 字符、无 CJK、仅字母数字/空格/连字符/下划线/点，容忍 `993 041` 分段码）：

- **像码** → 维持第 145 轮「立即 input_text 填码提交」；
- **不像码**（整句指示，如「先别提交，改查北京天气」）→ 「人工输入的是一段文字指示而非验证码，按其内容调整流程，不要填入页面输入框」。

选项应答与其余 reason 分支不变。人工经弹窗「输入提示词与 Agent 继续交互 / 点继续不输入」两条路都有明确语义。

## 4. control.rs 拆分（R155-C，顺带）

control.rs 已越 1800 行硬线。滚动/视口/跟随一族（`act_scroll`/`act_scroll_to`/`act_set_viewport`/`act_sync_viewport`/`act_auto_follow`）机械搬移至 `follow_ctl.rs`（`pub(super)` + `use super::*`，`dispatch_mouse` 经 `super::control::` 引用）。拆后 control.rs ≈ 1796 行。

## 5. 测试

- 单测：`locked_scroll_watch_env_default_and_off` / `guard_script_gate_install_and_kill_switch`（GATE_EVTS 清单含交互事件、不含 wheel/scroll/mousemove；isTrusted 判据；烘焙开关；文案分流）/ `visibility_note_cases` 扩可滚动两态 / `hitl_answer_hint_directive_text_not_treated_as_code`；
- 真浏览器集成（`#[ignore]`，`cargo test --lib real_browser_overlay_lock_and_mask -- --ignored`）：locked+闸 → CDP 合成点击被拦 → 闸挂起后通过 → 复位再拦；`isTrusted=false` 页面自合成点击放行；wheel 事件放行；三档/盾区/挖洞行为不回归（计数基准随新步骤对齐）。

## 6. 环境变量

| 变量 | 取值 | 行为 |
| ---- | ---- | ---- |
| `LAEW_WEB_LOCKED_SCROLL` | `off`/`0`/`false`/`no` | 关闭 locked 档「可滚动观看」DOM 输入闸，回退第 141 轮 CDP 全量输入拦截（连滚轮一起拦） |
| | 未设置（默认） | 开启：人工可滚动浏览（滚轮/触摸板放行），点击/键盘/拖拽仍拦 |
