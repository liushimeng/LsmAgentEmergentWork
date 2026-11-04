# 54 - 滚轮回调选择器拼错 unrecognized selector 查看器 abort

> 2026-11-04 真机任务实录(MCP_Web_LsmAgentBrowser_Use_02,猎聘 AI 岗位采集,
> `laew --debug -f` + `LAEW_BROWSER_MODE=headed`)。
> 现象:猎聘首页刚显示进有头查看器窗口,数秒后窗口直接消失(查看器子进程崩溃),
> 任务现场只剩宿主继续空转。**站点无关**——是查看器自身 FFI 缺陷。

## 根因(一句话)

`view/macos.rs::on_pv_scroll` 发了**不存在的选择器** `hasPreciseDeltas`
(SDK 真名是 `hasPreciseScrollingDeltas`,少了 `Scrolling`),首个滚轮事件即
`unrecognized selector` → NSException(foreign exception)展开穿过 `extern "C"`
(nounwind)回调 → `panic_cannot_unwind` → 整个查看器 SIGABRT。

## 因果链

1. 人工滚动 → `-[NSWindow _reallySendEvent:]` 路由 `scrollWheel:` → `on_pv_scroll`;
2. `send_i64(event, sel(b"hasPreciseDeltas\0"))` → objc_msgSend 找不到 IMP
   → **NSException**(`unrecognized selector sent to instance`);
3. ObjC 异常是 **foreign exception**——不是 Rust panic,不经 `panic_with_hook`,
   也不被 `catch_unwind` 捕获;展开到 `extern "C"` 函数的 nounwind 落地垫时
   `panic_cannot_unwind` 接管 →「panic in a function that cannot unwind」→ abort;
4. 查看器死亡,窗口消失。宿主 1Hz 心跳只写帧不查活,窗口永不再现
   (直到下一次工具动作才可能复活)。

## 取证方法论(下次同类事故照此走)

| 证据 | 本例取值 | 结论指向 |
| ---- | ---- | ---- |
| CrashDump **份数** | 1 份,消息 =「panic in a function that cannot unwind」 | 文件名含纳秒哈希,两次 hook 必然两份文件 ⇒ 只有一次 hook ⇒ **无 Rust panic A** |
| `.ips` 的 `asi` | 只有 `abort() called`,**无 `lastExceptionBacktrace`** | Rust 在展开中途 abort,ObjC 顶层处理器没跑到 ⇒ NSException 形态 |
| CrashDump 栈顶 | `on_pv_scroll`(全文件唯一发该选择器的回调) | 罪犯选择器锁死 |
| `.ips` 栈 | `_reallySendEvent` 在下 | 真实系统事件路由(非合成) |
| 崩溃时刻 | `open` 完成后首帧显示数秒内 | 人工第一次滚轮 = 该选择器第一次执行 |

口诀:**单份 CrashDump + `asi` 无异常回溯 ⇔ foreign exception 穿越 nounwind 边界**;
两份 CrashDump(第一份带真消息)⇔ 普通 Rust panic 展开到边界。

注:查看器 stderr 曾默认 `Stdio::null()`,panic 默认文案全部丢失;现已默认转发
进宿主运行日志(`tracing` target=`viewer_stderr`,`LAEW_BROWSER_VIEW_DEBUG=1`
改直连宿主 stderr)。

## 已落地修复(通用,非本选择器点杀)

1. **选择器改真名**:`hasPreciseDeltas` → `hasPreciseScrollingDeltas`。
2. **`respondsToSelector:` 前置守卫**(`view/macos.rs::respond`):事件子类型相关
   选择器(`hasPreciseScrollingDeltas`/`deltaX`/`deltaY`/`buttonNumber`)先验后发,
   缺失优雅降级——「选择器拼错/SDK 差异」从进程 abort 降级为该特性失效。
   foreign exception 无法 `catch_unwind`,**不发不存在的消息**是唯一根治。
3. **FFI 回调 panic 防火墙**(`src/ffi_guard.rs`):全部 `extern "C"`/`extern "system"`
   回调体经 `guard`/`guard_ret` 包裹——Rust panic 类不再穿越 nounwind 边界 abort
   (panic hook 照常写 CrashDump)。覆盖面:查看器 macOS 全部回调 + Windows
   `wnd_proc`/`edit_subclass` + HITL 弹窗全部回调(两平台)。
4. **回归单测钉死拼写类 bug**:`cargo test --lib` 用
   `+[NSObject instancesRespondToSelector:]` 校验事件处理链全部选择器真实存在于
   NSEvent/NSView(无需 GUI;本测试在事故前就该存在)。
5. **查看器意外死亡自愈**:`push_frame` 查活(`is_alive()`),死亡且**非人工关闭**
   (`windowWillClose:` 先发 `{"cmd":"viewer_closed"}` 再退出,宿主据此置旗标)
   时自动重生(≤3 次/会话,`close_view` 清零)并补推当前帧;超限回退纯无头 +
   WARN 提示查 `CrashReport/`。人工关窗不重生,不与用户抢窗口。

## 同类风险清单(排查用)

- 所有 `msg!` 转写的 `objc_msgSend` 目标选择器拼写(72 个已逐一对照 SDK 头文件,
  仅此一处错);新选择器必须对照 SDK 头文件 + 进单测清单;
- 任何新增 `extern "C"`/`extern "system"` 回调必须进 `ffi_guard`(nounwind 边界);
- 事件子类型相关 API(delta/phase/buttonNumber 等)先 `respond` 再发。
