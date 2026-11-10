# 05-窗口交互与页面观察

**看得见、点得到、等得着**这一层：有头窗口、人工介入、页面就绪判定。

| 文件 | 一句话 |
| --- | --- |
| [14-有头帧查看器与真实站点JS启动链路修复.md](14-有头帧查看器与真实站点JS启动链路修复.md) | `laew __browser-view` 查看器子进程显示软件渲染帧；真实站点 JS 启动链七处根治 |
| [17-阶段五第一步页面观察与就绪判定设计.md](17-阶段五第一步页面观察与就绪判定设计.md) | `MutationObserver`/`IntersectionObserver`/`ResizeObserver` 真实派发；`control(wait)` 就绪判定原语（超时 2002） |
| [20-浏览器窗口交互化与地图页白屏根因.md](20-浏览器窗口交互化与地图页白屏根因.md) | 有头窗口升级为**真浏览器形态**（工具栏+地址栏+命令通道）；`location.protocol` 缺冒号导致协议相对地址 404 |
| [37-GeeTest自愈循环可观测与HITL可见性守卫.md](37-GeeTest自愈循环可观测与HITL可见性守卫.md) | GeeTest 吞回调异常的自愈循环可观测（`LAEW_JS_CB_TRACE`）+ 人工介入可见性守卫 |
| [38-查看器跟随页语义与天猫白板根因.md](38-查看器跟随页语义与天猫白板根因.md) | 查看器显示跟随「最近交互页」而非「前台页」；天猫一轮 HITL 白板三根因（前台抢占/心跳盲推/导航无加载帧） |
| [39-人类鼠标键盘交互与真浏览器输入链路设计.md](39-人类鼠标键盘交互与真浏览器输入链路设计.md) | 查看器页面区真实键鼠：双平台事件捕获 → `cmd:"input"` 协议 → 宿主 `human_input` 管线（控制权闸门/坐标命中/复用 Agent 交互语义）；P2c 页面查找/光标形状/模态焦点锁/侧键；控件全集×实现状态对照 |
| [54-滚轮回调选择器拼错unrecognized-selector查看器abort.md](54-滚轮回调选择器拼错unrecognized-selector查看器abort.md) | 滚轮回调发不存在选择器 `hasPreciseDeltas`（真名 `hasPreciseScrollingDeltas`）→ NSException 穿越 nounwind 回调 abort 查看器；修复=真名+`respondsToSelector:` 守卫+`ffi_guard` 回调防火墙+选择器单测+意外死亡自愈；「单份 CrashDump+asi 无异常回溯⇔foreign exception」取证口诀 |
| [56-人类键鼠P2d指针事件联想面板多选与浏览器缩放.md](56-人类键鼠P2d指针事件联想面板多选与浏览器缩放.md) | pointer/wheel/hover 兼容事件派发(成对事件**一次 JS 往返**)、datalist 联想面板、select[multiple] 多选、number 步进、双平台浏览器缩放(视口÷缩放的 Chrome 语义);顺带修 Windows 查看器编译断链、监听器 `this` 绑定、option 命中被 display:none 拦死 |
| [57-人类键鼠P2e边界事件指针捕获与选区整片高亮.md](57-人类键鼠P2e边界事件指针捕获与选区整片高亮.md) | hover 切换 Chrome 完整事件序(enter/leave **不冒泡**、链差集、`DispatchMulti` 单次往返)、pointer capture 真语义(setPointerCapture 重定向 + pointerup 隐式释放)、跨元素选区中间整片点亮、`select.value` 读值桥、F11 全屏、Shift+滚轮横滚、number 微调箭头点击 |
