# 36 - 验证页布局弹层与 Canvas 截图保真修复

> 日期：2026-10-29
> 方案：`tmpPlan/20261029_101834_BOSS安全验证页白屏弹层与截图保真修复.md`
> 场景：BOSS 安全验证页在 Lsm 截图中大面积白屏，验证卡从应居中 520×515 变成近似全宽，GeeTest 显示「配置错误请点击重试116」。同页 Chrome 的卡片与点选弹层正常。
> 测试边界：用户要求停止真站测试。本轮根因来自既有日志/截图与代码审查，验收全部为本地离线回归；不宣称 BOSS 验证或岗位采集已经通过。

## 一、诊断证据与误区

- 本轮日志确认实际工具是 `MCP_Web_LsmAgentBrowser_Use`，首页重定向到验证页，200，但文本只有「配置错误请点击重试116」等残缺内容，按要求停止任务。
- Lsm 旧截图与白屏一致；Chrome 同页截图显示 520×515 居中卡片和真实点选弹层。因此截图 PNG 能生成，只能证明编码/落盘正常，不能证明布局与像素正确。
- 网络侧 `error_code=604` 只能说明第三方回调失败，不能直接推断为 WebGL。首屏 Chrome `canvas_count=0`，必须继续检查 JS 二进制语义、DOM 快照和 Canvas 2D，而不是做站点特判。

## 二、根因与修复

### 1. Flex column 把交叉轴宽度当成高度

`layout_content` 旧返回值不分主轴方向，column 最终返回 `container_cross`（内容宽）。1920×1080 下，验证页高度因此变成 1920，即使卡片已经按 1080 正确居中，整页仍被异常拉高。

现按方向返回：row 取交叉轴高度；column 取主轴高度。显式 `height` 保持确定值，只有 `min-height` 时允许内容超过地板继续撑开。

### 2. 定位 inset 百分比使用了错误基准

`top/right/bottom/left` 旧实现直接走通用长度近似，百分比可能按视口宽预解。现保留四个 inset 的百分比原值：

- left/right 相对包含块宽；
- top/bottom 相对包含块高；
- absolute/fixed/relative 共用同一求解入口；
- `transform:translate(-50%,-50%)` 仍相对元素自身盒，两个百分比阶段不再混用。

### 3. 高度约束与脱流盒模型不完整

- `max-height` 进入 `ComputedStyle`、计算样式反射和布局；统一先 max 后 min，冲突时 min 胜出。
- 块级、Flex、Grid 与 absolute/fixed 应用同一约束。
- 脱流盒支持 width/height 百分比、min/max 和显式 `content-box`，不再无条件把 width 当内容宽后重复加 padding。

### 4. Base64 把二进制字符串误当 UTF-8

旧 `btoa` 按 UTF-8 编码，旧 `atob` 用 `String::from_utf8_lossy`。任意非 UTF-8 图片/指纹字节都会被替换，导致页面脚本与真实浏览器得到不同结果。

现按浏览器语义实现：

- `btoa` 按 Latin-1 码元编码，>0xFF 显式抛错；
- `atob` 每个字节映射为同值 U+0000..U+00FF 码元；
- 支持 ASCII 空白与省略末尾 padding，非法长度报错。

### 5. 静态 DOM 快照漏掉固有尺寸

`common_attrs` 白名单没有 `width/height`，静态 `<canvas width=4 height=2>` 到 JS 包装层退化成默认 300×150。现保留这两个属性，静态 Canvas/图片包装可读到真实固有尺寸。

### 6. Canvas 不再以空实现伪装成功

新增 `script/canvas_js.rs`，静态与动态 Canvas 共用 RGBA8 像素上下文，按 `data-lsm-id`/稳定选择器复用状态：

- 已实现：宽高设置与缓冲重置、`fillRect/strokeRect/clearRect`、`createImageData/getImageData/putImageData`、Canvas→Canvas `drawImage`；
- `toDataURL` 通过宿主 `image::codecs::png::PngEncoder` 输出真实 PNG，不再固定返回空串；
- 每轮回写把 PNG 放入内部 `data-lsm-canvas`，复用既有 data: 图片解码缓存；缓存按 Canvas 稳定选择器逐帧覆盖并回收移除项，不按变化的 data URL 累积旧帧；显示列表在 CanvasNote 之上绘制真实图像，截图与 headed 查看器共用同一像素源；
- 路径、文字、渐变、远程图片入 Canvas 仍属未实现边界，WebGL 仍不可用，不把这些能力写成已支持。

### 7. 等待与截图前同步持久 JS 页面

纯 `wait` 旧实现只 sleep；无新脚本时 `run_pending_scripts` 直接返回，已有定时器/微任务不再推进。截图也只取既有布局帧。

新增轻量 `sync_page`：复用当前 JS Context 执行一次空调度，应用 DOM/Canvas 动作并按需重排。纯等待、条件等待轮询和截图前统一调用；不重建 Context，不另存第二份页面状态。

## 三、离线验证

- `layout/position_tests.rs` 4 项：全屏 Flex 验证卡、fixed 百分比居中与 transform、脱流 content-box/min/max、块级与 Grid 高度钳序。
- `script/render_compat_tests.rs` 5 项：全码元 Base64 往返、forgiving-base64、静态 Canvas 持久像素与固有尺寸、动态 Canvas 真 PNG 与贴图、尺寸重置。
- `tests/browser_canvas_screenshot.rs` 1 项：本地 mock 页面完整经过 open→脚本→图片缓存→截图落盘；断言 1920×1080 页面、520px 卡片、Canvas 中心像素 `[0,255,0,255]`。
- `tests/web_use_engine.rs` 23/23 通过（含既有富视觉渐变截图）。
- 全量单元测试 2638 通过、0 失败、7 忽略。
- 变基到最新 main 后上述新增 10 项与既有 23 项均通过；release 构建结果见当轮最终汇报。

## 四、后续边界

本轮解决的是通用布局、二进制 JS、Canvas 基础像素和截图帧同步，不是 BOSS 专用绕过。真实站点是否还会触发 604、IP 风控或服务端验证，必须在用户允许后另行真站复测。Canvas 路径/文字/渐变、远程图片入画布和 WebGL 仍需独立阶段实现。
