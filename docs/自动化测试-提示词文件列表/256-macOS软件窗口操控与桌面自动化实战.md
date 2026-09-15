# 256 macOS 软件窗口操控与桌面自动化实战

> 编号段 MS01–MS20 · 聚焦「macOS 上常用软件的窗口读取、控件遍历、操作执行」：Chrome 浏览器 / Blender 建模工具 / 微信 IM / QQ 聊天 / Finder 文件管理 / 系统偏好设置 / 终端 Terminal / 活动监视器 / 截图与录屏 / 通知中心 / 启动台
>
> 与现有维度互补说明：
> - `230-WindowUse多轮连续性与Agent间通信实战`(HQ 维)测的是**跨轮状态持久化**;本文件测的是**单轮/多轮软件操控实操**。
> - `233-WindowUse桌面操控与多轮连续性实战`(HT 维)测的是**WindowUse 全能力**;本文件测的是**macOS 特有软件**的窗口结构与控件树。
> - `126-macOS桌面运维与Apple生态开发实战`(DR 维)测的是**系统运维命令**;本文件测的是**GUI 软件窗口操控**。
> - `03-电脑使用与系统管理`(C 维)测的是**通用系统管理**;本文件测的是**macOS 桌面软件**的窗口级交互。
>
> **本文件独特主题**：Chrome 浏览器标签页与地址栏读取 / Blender 3D 视口与属性面板 / 微信聊天列表与消息输入框 / QQ 聊天窗口与表情面板 / Finder 文件列表与预览 / 系统偏好设置面板 / 终端命令输出 / 活动监视器进程列表 / 截图与录屏控制 / 通知中心小组件 / 启动台应用图标

---

## MS01 Chrome 浏览器窗口读取与标签页枚举

- **测试状态**: ✅ 已测试(2026-09-15 第 53 轮 macOS 26.5 实测 WindowList 链路,Yolo→Main-Work→WindowUse→WindowList 真实返回 Chrome/Code/微信等窗口列表)
- **预期档位**: medium
- **考察维度**: WindowList 枚举 Chrome 窗口 / WindowInspect 读取标签页列表 / 地址栏 URL 获取
- **工具链**: WindowList → WindowInspect → WindowAction(get_text)
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出所有 Chrome 浏览器窗口(过滤 process_name 含 "Chrome"),获取窗口标题和进程信息。
  2. 用 WindowInspect 检视 Chrome 窗口的控件树,找到地址栏(AXTextField 角色)和标签页列表(AXTabGroup 角色)。
  3. 用 WindowAction 对地址栏执行 get_text,读取当前 URL。
  4. 断言:Agent 能正确识别 Chrome 窗口、找到地址栏控件、读取到 URL 文本(非空)。

## MS02 Chrome 浏览器标签页切换与导航

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: 标签页控件遍历 / 点击切换 / 地址栏写入导航
- **工具链**: WindowInspect → WindowAction(click) → WindowAction(set_text)
- **对话脚本**:
  1. [第 1 轮] 用 WindowInspect 读取 Chrome 标签页列表,记录当前激活标签页标题。
  2. 用 WindowAction 点击第二个标签页(通过控件路径定位),验证切换成功。
  3. 用 WindowAction 对地址栏执行 set_text 写入新 URL(如 "https://www.apple.com"),然后模拟回车键。
  4. 断言:标签页切换后标题变化、地址栏文本更新为新 URL。

## MS03 Blender 建模工具窗口结构读取

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: Blender 多窗口布局识别 / 3D 视口控件树 / 属性面板遍历
- **工具链**: WindowList → WindowInspect
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出所有 Blender 窗口(过滤 process_name 含 "Blender"),获取主窗口和子窗口。
  2. 用 WindowInspect 检视 Blender 主窗口,识别 3D 视口(AXView 角色)、顶部菜单栏、右侧属性面板。
  3. 用 WindowInspect 检视属性面板区域,找到变换(Transform)属性组(位置/旋转/缩放)。
  4. 断言:Agent 能区分 Blender 主窗口与子窗口、识别 3D 视口控件、找到变换属性组。

## MS04 Blender 3D 视口操作与对象选择

- **测试状态**: ⏳ 待测试
- **预期档位**: hard
- **考察维度**: 3D 视口点击选择 / 对象属性读取 / 视图切换
- **工具链**: WindowInspect → WindowAction(click) → WindowAction(get_text)
- **对话脚本**:
  1. [第 1 轮] 用 WindowInspect 读取 Blender 3D 视口的控件树,找到默认立方体对象。
  2. 用 WindowAction 点击立方体对象(通过控件路径定位),验证选中状态。
  3. 用 WindowAction 对属性面板执行 get_text,读取选中对象的变换属性(位置 X/Y/Z)。
  4. 断言:对象被选中(属性面板显示对象名称)、变换属性值非空。

## MS05 微信聊天窗口列表读取

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: 微信窗口识别 / 聊天列表控件树 / 消息预览读取
- **工具链**: WindowList → WindowInspect → WindowAction(get_text)
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出所有微信窗口(过滤 process_name 含 "WeChat" 或 "微信"),获取主窗口。
  2. 用 WindowInspect 检视微信主窗口,识别聊天列表(AXList 角色)、消息输入框(AXTextArea 角色)。
  3. 用 WindowAction 对聊天列表执行 get_text,读取最近几条聊天预览。
  4. 断言:Agent 能识别微信窗口、找到聊天列表控件、读取到聊天预览文本。

## MS06 微信消息输入与发送

- **测试状态**: ⏳ 待测试
- **预期档位**: hard
- **考察维度**: 消息输入框定位 / 文本写入 / 发送按钮点击
- **工具链**: WindowInspect → WindowAction(set_text) → WindowAction(click)
- **对话脚本**:
  1. [第 1 轮] 用 WindowInspect 读取微信主窗口,找到消息输入框(AXTextArea)和发送按钮(AXButton)。
  2. 用 WindowAction 对输入框执行 set_text 写入测试消息(如 "laew 测试消息")。
  3. 用 WindowAction 点击发送按钮。
  4. 断言:输入框文本更新、发送按钮可点击(不验证消息是否真实发送)。

## MS07 QQ 聊天窗口读取与表情面板

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: QQ 窗口识别 / 聊天区域控件树 / 表情面板遍历
- **工具链**: WindowList → WindowInspect
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出所有 QQ 窗口(过滤 process_name 含 "QQ"),获取主窗口。
  2. 用 WindowInspect 检视 QQ 主窗口,识别聊天消息区域(AXScrollView)、输入框、表情按钮(AXButton)。
  3. 用 WindowAction 点击表情按钮,验证表情面板弹出。
  4. 断言:Agent 能识别 QQ 窗口、找到表情按钮、点击后面板状态变化。

## MS08 QQ 聊天消息读取与回复

- **测试状态**: ⏳ 待测试
- **预期档位**: hard
- **考察维度**: 聊天消息读取 / 输入框定位 / 回复发送
- **工具链**: WindowInspect → WindowAction(get_text) → WindowAction(set_text) → WindowAction(click)
- **对话脚本**:
  1. [第 1 轮] 用 WindowInspect 读取 QQ 聊天窗口,找到消息列表区域。
  2. 用 WindowAction 对消息列表执行 get_text,读取最近几条消息。
  3. 用 WindowAction 对输入框执行 set_text 写入回复内容。
  4. 用 WindowAction 点击发送按钮。
  5. 断言:消息列表非空、输入框文本更新、发送按钮可点击。

## MS09 Finder 文件列表读取与文件操作

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: Finder 窗口识别 / 文件列表控件树 / 文件选中与预览
- **工具链**: WindowList → WindowInspect → WindowAction(click)
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出所有 Finder 窗口(过滤 process_name 含 "Finder"),获取文件管理器窗口。
  2. 用 WindowInspect 检视 Finder 窗口,识别文件列表(AXList 或 AXOutline)、侧边栏、预览面板。
  3. 用 WindowAction 点击文件列表中的某个文件,验证选中状态。
  4. 断言:Agent 能识别 Finder 窗口、找到文件列表控件、点击后文件被选中。

## MS10 Finder 文件搜索与排序

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: 搜索框定位 / 关键词输入 / 结果列表读取
- **工具链**: WindowInspect → WindowAction(set_text) → WindowAction(get_text)
- **对话脚本**:
  1. [第 1 轮] 用 WindowInspect 读取 Finder 窗口,找到搜索框(AXTextField 角色)。
  2. 用 WindowAction 对搜索框执行 set_text 写入搜索关键词(如 ".md")。
  3. 用 WindowAction 对文件列表执行 get_text,读取搜索结果。
  4. 断言:搜索框文本更新、文件列表内容变化(显示搜索结果)。

## MS11 系统偏好设置窗口读取

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: 系统偏好窗口识别 / 设置面板列表 / 面板切换
- **工具链**: WindowList → WindowInspect → WindowAction(click)
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出系统偏好设置窗口(过滤 process_name 含 "SystemPreferences" 或 "System Settings")。
  2. 用 WindowInspect 检视系统偏好窗口,识别设置面板列表(AXList 角色,如"通用"/"桌面与屏幕保护程序"/"网络"等)。
  3. 用 WindowAction 点击某个面板(如"通用"),验证面板切换。
  4. 断言:Agent 能识别系统偏好窗口、找到面板列表、点击后面板内容变化。

## MS12 终端窗口命令输出读取

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: 终端窗口识别 / 命令输出区域读取 / 滚动历史
- **工具链**: WindowList → WindowInspect → WindowAction(get_text)
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出终端窗口(过滤 process_name 含 "Terminal" 或 "iTerm")。
  2. 用 WindowInspect 检视终端窗口,识别输出区域(AXTextArea 或 AXScrollView)。
  3. 用 WindowAction 对输出区域执行 get_text,读取最近命令输出。
  4. 断言:Agent 能识别终端窗口、找到输出区域、读取到命令输出文本。

## MS13 活动监视器进程列表读取

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: 活动监视器窗口识别 / 进程列表控件树 / 进程信息读取
- **工具链**: WindowList → WindowInspect → WindowAction(get_text)
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出活动监视器窗口(过滤 process_name 含 "Activity Monitor")。
  2. 用 WindowInspect 检视活动监视器窗口,识别进程列表(AXTable 或 AXOutline)。
  3. 用 WindowAction 对进程列表执行 get_text,读取前几行进程信息(进程名/CPU/内存)。
  4. 断言:Agent 能识别活动监视器窗口、找到进程列表、读取到进程信息。

## MS14 截图与录屏控制

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: 截图工具窗口识别 / 截图按钮点击 / 录屏控制
- **工具链**: WindowList → WindowInspect → WindowAction(click)
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出截图工具窗口(过滤 process_name 含 "Screenshot" 或 "截屏")。
  2. 用 WindowInspect 检视截图工具窗口,识别截图模式按钮(AXButton 角色,如"截取整个屏幕"/"截取选定窗口")。
  3. 用 WindowAction 点击某个截图模式按钮。
  4. 断言:Agent 能识别截图工具窗口、找到截图按钮、按钮可点击。

## MS15 通知中心小组件读取

- **测试状态**: ⏳ 待测试
- **预期档位**: simple
- **考察维度**: 通知中心窗口识别 / 小组件列表 / 通知内容读取
- **工具链**: WindowList → WindowInspect → WindowAction(get_text)
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出通知中心窗口(过滤 process_name 含 "NotificationCenter" 或 "通知中心")。
  2. 用 WindowInspect 检视通知中心窗口,识别小组件列表(AXList 角色)。
  3. 用 WindowAction 对小组件区域执行 get_text,读取通知内容。
  4. 断言:Agent 能识别通知中心窗口、找到小组件列表、读取到通知文本。

## MS16 启动台应用图标枚举

- **测试状态**: ⏳ 待测试
- **预期档位**: simple
- **考察维度**: 启动台窗口识别 / 应用图标网格 / 应用名称读取
- **工具链**: WindowList → WindowInspect → WindowAction(get_text)
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出启动台窗口(过滤 process_name 含 "Launchpad" 或 "启动台")。
  2. 用 WindowInspect 检视启动台窗口,识别应用图标网格(AXGroup 或 AXList 角色)。
  3. 用 WindowAction 对应用图标区域执行 get_text,读取应用名称列表。
  4. 断言:Agent 能识别启动台窗口、找到应用图标区域、读取到应用名称。

## MS17 多窗口并发操作:Chrome + 微信

- **测试状态**: ⏳ 待测试
- **预期档位**: hard
- **考察维度**: 多窗口状态隔离 / 跨应用数据搬运 / 并发操作
- **工具链**: WindowList ×2 → WindowInspect ×2 → WindowAction(get_text) → WindowAction(set_text)
- **对话脚本**:
  1. [第 1 轮] 同时打开 Chrome 和微信,用 WindowList 列出两个窗口。
  2. 用 WindowInspect 分别检视 Chrome 地址栏和微信输入框。
  3. 用 WindowAction 对 Chrome 地址栏执行 get_text 读取 URL。
  4. 用 WindowAction 对微信输入框执行 set_text 写入从 Chrome 读取的 URL。
  5. 断言:两个窗口状态互不干扰、URL 正确传递到微信输入框。

## MS18 窗口状态跨轮持久化:Chrome 标签页

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: WindowState 跨轮注入 / 路径自动复用 / 操作历史回溯
- **工具链**: WindowAction(第 1 轮) → WindowAction(第 2 轮)
- **对话脚本**:
  1. [第 1 轮] 用 WindowList 列出 Chrome 窗口,用 WindowInspect 读取标签页列表,点击第二个标签页。
  2. [第 2 轮] 继续在那个 Chrome 窗口点击第三个标签页(验证:Agent 不应再要求用户描述「是哪个 Chrome 窗口」;系统提示词里的 `<<<LAEW:WINDOW_STATE>>>` 段应包含该 window_id)。
  3. 断言:第 2 轮操作复用第 1 轮的窗口上下文、WindowState 中 action_history 长度 ≥ 2。

## MS19 失败操作后重试:Blender 属性面板

- **测试状态**: ⏳ 待测试
- **预期档位**: medium
- **考察维度**: 路径失效检测 / 历史回溯 / 重新检视后重试
- **工具链**: WindowAction(失败) → WindowInspect(刷新) → WindowAction(重试)
- **对话脚本**:
  1. [第 1 轮] 用 WindowInspect 读取 Blender 属性面板,尝试点击一个不存在的控件路径(返回 NOT_FOUND)。
  2. [第 2 轮] 让 Agent 重试刚才的操作(验证:Agent 应读取 window_state 中的 action_history 发现上一轮失败,重新 WindowInspect 刷新 known_controls,再尝试相近路径)。
  3. 断言:第 2 轮 mock 接收序列为 `[WindowInspect(刷新), WindowAction(重试)]`,不盲目重复原 path。

## MS20 跨应用数据搬运:Chrome URL → 微信消息

- **测试状态**: ⏳ 待测试
- **预期档位**: hard
- **考察维度**: pending_data 跨应用传递 / 不需要用户复述 / 端到端数据搬运
- **工具链**: WindowList ×2 → WindowInspect ×2 → WindowAction(get_text) → WindowAction(set_text)
- **对话脚本**:
  1. [第 1 轮] 在 Chrome 地址栏输入一个 URL(如 "https://www.rust-lang.org"),用 WindowAction 读取地址栏文本。
  2. [第 2 轮] 「把刚才 Chrome 里的 URL 复制到微信输入框」(验证:第 2 轮应自动 `WindowInspect(Chrome)` + `get_text` 得到 URL 存入 `pending_data`,再 `WindowInspect(微信)` + `set_text(pending_data)`,全程不要求用户复述 URL)。
  3. 断言:微信输入框中的文本与 Chrome 地址栏中的 URL 字节一致。
