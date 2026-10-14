# MCP_Web_Use Windows 弹窗 macOS 风格界面美化(主题模块)

> 版本:2026-10-14
> 适用平台:**Windows**(macOS 弹窗本就是 AppKit 原生控件,不在本文范围)
> 实现入口:`src/agent/human_ui/windows_theme.rs`(主题唯一真源)+ `windows_dialog.rs`(接入)
> 关联文档:`03-人工介入弹窗原生UI方案.md`(弹窗结构与契约,本文不改变其任何契约)、
> `01-设计与解决方案.md`(MCP_Web_Use 基础架构)

## 1. 背景与目标

第 137 轮弹窗原生化后,Windows 侧(`windows_dialog.rs`)一直使用**系统默认控件观感**:
`COLOR_WINDOW` 白底、方角系统按钮、原生 `BS_AUTOCHECKBOX` 复选框、系统配色标题栏。
与 macOS 侧 AppKit 控件(圆角按钮 / NSSwitch 开关 / 分层灰底)观感割裂,也缺乏
深色模式适配。

目标:

1. **macOS 控件风格**:主按钮 = 品牌蓝圆角填充、次按钮 = 白底描边圆角、多选项 =
   NSSwitch 形态开关(药丸轨道 + 圆形滑块)、说明区/输入框 = 卡片式、窗口底色 =
   macOS 窗口灰;浅色/深色双配色板**随系统外观自动切换**;
2. **自动遍历全部弹窗窗口**:本进程弹出的所有 UI 窗口统一走主题
   (`EnumWindows` 按进程 PID 过滤自遍历补挂 chrome),当前窗口清单
   = `LAEWHITLDIALOG`(人工介入弹窗)+ `LAEWHITLRELAYPAD`(侵入式触控板子窗),
   **日后新增窗口自动被遍历机制覆盖**;
3. **规则固化**:新增 Windows 弹窗一律复用主题模块(见 §7,已写入 CLAUDE.md /
   AGENTS.md 供 AI Agent Tools 自动加载);
4. **契约零变化**:payload/result JSON、四态应答、按钮语义、Tab/Enter/Esc 路由、
   WM_SIZE 全量重排布局纯函数——全部不动,只换「观感层」。

## 2. 总体设计

```text
windows_theme.rs(唯一真源)
  ├── WinTheme 配色板(浅/深双色板 + 刷子缓存,进程生命周期)
  │     检测: LAEW_WIN_THEME=off|light|dark|缺省auto(注册表 AppsUseLightTheme)
  ├── 控件自绘图元(BS_OWNERDRAW → WM_DRAWITEM)
  │     draw_button(Primary/Secondary) / draw_switch_row(macOS 开关)
  ├── ctl_color(WM_CTLCOLORSTATIC / WM_CTLCOLOREDIT → 文本色 + 卡片刷)
  ├── apply_window_chrome(DWM: 沉浸式深色标题栏 / 标题栏同色 / Win11 圆角)
  └── sweep_process_windows(EnumWindows 本进程 PID 过滤 → 逐窗补挂 chrome)
windows_dialog.rs(接入,不持有任何颜色常量)
  ├── 窗口类 hbrBackground = 主题底刷(触控板类 = 卡片刷)
  ├── style_button / style_check_option:主题启用时追加 BS_OWNERDRAW
  ├── wnd_proc 新增 WM_DRAWITEM / WM_CTLCOLOR* / WM_SETTINGCHANGE 分发
  ├── 多选项勾选台账 opt_checked(Vec<bool>,单一真源;收口只读台账)
  └── WM_TIMER 每秒 sweep(新窗口/主题广播后自动补挂)
```

**为什么做成独立模块而不是塞进 windows_dialog.rs**:

- `windows_dialog.rs` 已 1200+ 行(拆分红线 1800),主题属于「观感层」独立职责;
- 「日后新窗口都要美化」要求主题可复用——窗口代码只做**接入**(按控件 ID 决定
  「画什么」),图元绘制与配色全部在主题模块,新窗口接入成本 = 3 个消息分支。

## 3. 配色系统

数值取自 macOS 系统色(Apple HIG / Sonoma 系统色板),COLORREF 按 GDI `0x00BBGGRR` 打包。

| 槽位 | 浅色 | 深色 | 用途 |
| --- | --- | --- | --- |
| 窗口底 `bg` | `#F2F2F7` | `#1E1E20` | 窗口背景 / 标题栏同色 |
| 卡片 `card` | `#FFFFFF` | `#2C2C2E` | 说明区 / 输入框 / 触控板 |
| 正文 `text` | `#1D1D1F` | `#F5F5F7` | 标题 / 正文 / 按钮文字 |
| 次要文本 `text_secondary` | `#6E6E73` | `#98989D` | 时间轴 / 倒计时 / 触控板提示 |
| 强调 `accent` | `#007AFF` | `#0A84FF` | 主按钮 / 开关开启态 / 焦点环 |
| 强调悬停 `accent_hover` | `#2D8CFF` | `#2491FF` | 主按钮 hover |
| 强调按下 `accent_press` | `#0062D6` | `#0868C8` | 主按钮按下 |
| 次按钮底 `btn_bg` | `#FFFFFF` | `#3A3A3C` | 次按钮 / 单选选项行 |
| 次按钮边 `btn_border` | `#D2D2D7` | `#48484A` | 同上描边 |
| 次按钮悬停 `btn_hover` | `#F5F5F7` | `#454547` | |
| 次按钮按下 `btn_press` | `#E8E8ED` | `#515154` | |
| 开关关轨 `switch_track_off` | `#E9E9EA` | `#39393D` | 多选项开关未选中轨道 |
| 滑块 `knob` | `#FFFFFF` | `#F5F5F7` | 开关圆形滑块 |

**外观检测**(`system_prefers_dark`):读注册表
`HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme`
(DWORD,`0` = 深色)。读取失败/字段缺失 → 回退浅色(fail-open);进程启动时检测一次,
运行期不热切(弹窗是短生命周期子进程,生命周期内切外观的场景可忽略)。

**强制档位**:`LAEW_WIN_THEME=light|dark` 跳过检测直接用对应色板(观感验证/截图留档用)。

## 4. 控件样式规范

### 4.1 按钮(BS_OWNERDRAW → WM_DRAWITEM)

| 控件 | 样式 |
| --- | --- |
| 「继续」(IDOK,主按钮) | accent 圆角填充(radius 6px)、白字居中、悬停 accent_hover、按下 accent_press |
| 取消 / ⏱+2分钟 / 解除屏蔽 / 🎮侵入式 / 📋复制(次按钮) | btn_bg 圆角填充 + 1px btn_border 描边、text 居中 |
| 单选选项(非 multi_select) | 次按钮样式整行展开(点击即应答,行为不变) |

- 焦点环:ODS_FOCUS 时在控件内缩 2px 画 1px accent 圆角描边(macOS keyboard focus 形态);
- 状态位:`ODS_SELECTED`(按下)/ `ODS_HOTLIGHT`(悬停)来自系统 owner-draw 通知,
  不自行追踪鼠标;
- 文本用 `DrawTextW(DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS)`,
  与旧 BUTTON 控件同一条 GDI 渲染路径(emoji 按钮文案 📋⏱🔓🎮 的字体链接行为不变);
- `WM_SETTEXT`(延长 ×N / 管控互切标题 / ✓已复制反馈)自动触发重绘,无需额外接线。

### 4.2 多选项 = macOS 开关(NSSwitch 形态)

`multi_select=true` 的选项不再用 `BS_AUTOCHECKBOX` 原生复选框,而是整行 owner-draw:

```text
[ ●───────]  选项文案……            ← off:switch_track_off 轨道 + 白滑块靠左
[ ───────● ]  选项文案……            ← on:accent 轨道 + 白滑块靠右
```

- 轨道 38×22px(对齐 NSSwitch),滑块 = 轨道高 − 4px 圆;行高仍 BTN_H=30(布局零改动);
- **勾选态记账**:`BS_OWNERDRAW` 不持有系统勾选态,新增 `WinState::opt_checked: Vec<bool>`
  单一真源——点击在 `WM_COMMAND` 翻转台账 + `InvalidateRect`,收口(`finish`)只读台账
  (不再 `IsDlgButtonChecked`);
- **杀开关 off 时**回退 `BS_AUTOCHECKBOX`(系统自翻转),台账同步翻转保持单一真源不变
  ——两种模式收口路径完全一致,不会出现「台账与系统勾选态漂移」。

### 4.3 卡片与文本(WM_CTLCOLORSTATIC / WM_CTLCOLOREDIT)

| 控件 | 处理 |
| --- | --- |
| 标题(IDC_TITLE) | text 色 + bg 底 |
| 时间轴/倒计时(IDC_T1/T2) | text_secondary 色 + bg 底 |
| 说明区(IDC_MSG,只读 EDIT) | card 底卡片 + text 色(只读 EDIT 走 WM_CTLCOLORSTATIC) |
| 输入框(IDC_INPUT) | card 底卡片 + text 色(WM_CTLCOLOREDIT) |
| 触控板提示(触控板子窗 STATIC) | card 底 + text_secondary 色 |

用 `SetBkColor` 而非 `TRANSPARENT`:EDIT 拖选高亮与 STATIC 重绘(倒计时每秒刷新)
在透明模式下都易出残影,同色实底是零观感差异的稳妥解。

### 4.4 窗口 chrome(apply_window_chrome,DWM)

对每个窗口按序设置(全部 fail-open,Win10 不支持的属性静默失败):

| DWM 属性 | 值 | 效果 |
| --- | --- | --- |
| `DWMWA_USE_IMMERSIVE_DARK_MODE`(20) | 随主题 | 深色时标题栏文字/系统按钮变白 |
| `DWMWA_CAPTION_COLOR`(35) | = `bg` | 标题栏与客户区同色(macOS 统一标题栏观感,消除白条割裂) |
| `DWMWA_WINDOW_CORNER_PREFERENCE`(33) | `DWMWCP_ROUND` | Win11 圆角窗口(Win11 本就默认圆角,幂等保险) |

**保留系统标题栏**(不自绘、不 borderless):最大化/还原/系统菜单/拖拽缩放语义
与 `WM_SIZE` 全量重排链路零改动——第 137 轮根治点不回退。

## 5. 进程窗口自遍历(sweep_process_windows)

「所有弹出的 UI 窗口都要美化」的机械化实现:

```rust
EnumWindows(每顶层窗口)
  → GetWindowThreadProcessId == GetCurrentProcessId   // 只碰本进程窗口,绝不越权
  → apply_window_chrome(hwnd)                          // 幂等,重复调用无副作用
```

触发时机(三处,覆盖窗口生命周期全程):

1. **主弹窗创建后**(`CreateWindowExW` 成功即单点 chrome);
2. **WM_TIMER 每秒 sweep**(弹窗既有 1s 定时器复用,零新定时器)——**日后新增的
   任何窗口**即使创建路径忘了接入,最多 1s 后被遍历补挂;触控板子窗随互切显示,
   由本机制兜底;
3. **WM_SETTINGCHANGE**(系统外观/设置变更广播)重申 chrome。

遍历只做**窗口级** chrome(标题栏/圆角/深色位);控件级主题(owner-draw/ctlcolor)
在控件创建时由窗口代码接入(见 §7 规则)。代价:每秒 1 次 `EnumWindows`(回调
全系统顶层窗口、仅 PID 比对)+ 命中窗口 3 次 `DwmSetWindowAttribute`,纳秒级,
可忽略。

## 6. 杀开关与兼容性

| 环境变量 | 行为 |
| --- | --- |
| 未设置(默认) | 主题开启;浅/深随系统注册表自动 |
| `light` / `dark` | 强制浅色 / 深色色板(跳过检测) |
| `off` / `0` / `false` / `no` | **整体关闭**:控件不加 `BS_OWNERDRAW`(回退系统原生观感)、多选项回退 `BS_AUTOCHECKBOX`、类背景回退 `GetSysColorBrush(COLOR_WINDOW)`、chrome/sweep 短路 |

兼容性:

- **Win10**:DWM 属性 33/35 不受支持 → 调用失败静默忽略(标题栏保持系统观感),
  owner-draw 自绘照常(WM_DRAWITEM 自 Win95 可用);`EM_SETCUEBANNER` 等既有兼容
  行为不变;
- **无桌面会话**(SSH 等):弹窗进程本来就创建失败走降级链,主题不引入新路径;
- **字体不换**:继续 `Microsoft YaHei UI`(CJK 确定性渲染;macOS 观感由配色/形状/
 层级承载,不赌 Segoe UI Variable 的存在性)。

## 7. 新增 Windows 弹窗的强制规则(给后续开发/AI Agent Tools)

CLAUDE.md / AGENTS.md 已同步收录本条。**日后任何新增的 Windows 弹出窗口必须**:

1. 窗口类 `hbrBackground` 用 `windows_theme::theme()` 的底刷(或卡片刷);
2. `CreateWindowExW` 后调用 `windows_theme::apply_window_chrome(hwnd)`
   (漏了也会被每秒 sweep 补挂,但创建点接入是规范);
3. 按钮统一 `style_button`(主题自动加 `BS_OWNERDRAW`),`WM_DRAWITEM` 分发到
   `windows_theme::draw_button` / `draw_switch_row`;
4. 文本类控件 `WM_CTLCOLOR*` 分发到 `windows_theme::ctl_color`;
5. **禁止在窗口代码里硬编码颜色**——配色只存在于 `windows_theme.rs` 一处,
   改主题 = 改一个文件。

## 8. 实现分布与测试

| 文件 | 改动 |
| --- | --- |
| `src/agent/human_ui/windows_theme.rs` | **新增**:配色板 / 外观检测 / 自绘图元 / ctlcolor / DWM chrome / 进程窗口 sweep + 纯函数单测 |
| `src/agent/human_ui/windows_dialog.rs` | 接入:类背景刷、owner-draw 样式、WM_DRAWITEM/WM_CTLCOLOR*/WM_SETTINGCHANGE 分发、`opt_checked` 勾选台账、每秒 sweep、触控板类卡片化 |
| `src/agent/human_ui/mod.rs` | 模块声明 `windows_theme` |
| `Cargo.toml` | `windows` crate 追加 `Win32_Graphics_Dwm` / `Win32_System_Registry` feature(既有依赖,非新 crate) |
| `CLAUDE.md` / `AGENTS.md` | 环境变量表 `LAEW_WIN_THEME`、领域概念 bullet、架构树、文档地图 |

测试(`cargo test --lib`):

- 主题模块纯函数:`parse_theme_setting` 四档解析 / `system_prefers_dark_from_flag`
  容错 / `rgb` 打包序 / `radius_for` 钳制 / `switch_geometry` 滑块在轨道内且左右位正确 /
  浅深色板关键槽位互异(bg≠card、accent≠bg);
- `windows_dialog.rs` 布局单测原样通过(布局零改动);`opt_checked` 翻转/收口语义
  随 `join_multi_answer` 既有用例覆盖;
- 真机验证:手工构造 payload 运行 `laew __hitl-dialog payload.json`
  (`LAEW_WIN_THEME` 三档各跑一次,短 timeout 自灭收口),确认自绘路径无 panic、
  `{"status":"timeout"}` 正常输出。

## 9. 关联文档

- `03-人工介入弹窗原生UI方案.md` —— 弹窗结构 / 子进程契约 / WM_SIZE 全量重排(本文的宿主)
- `02-人工介入与窗口可视化方案.md` —— HITL 闭环基线
- `docs/TUI界面与CLI渲染引擎/` —— TUI 侧选中效果视觉规范(本主题与其同源的
  「形状 + 边框 + 颜色」分层思路,但实现载体不同:终端 ANSI vs Win32 GDI)
