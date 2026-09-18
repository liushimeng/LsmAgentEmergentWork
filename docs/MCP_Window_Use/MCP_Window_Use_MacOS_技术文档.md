## 一、macOS 无障碍 API 核心功能与架构

macOS 无障碍体系分为两大方向：**应用内无障碍支持**（为自身 App 集成辅助功能适配）和**无障碍客户端 API**（跨应用读取 / 操控 UI 界面，开发辅助工具、自动化工具），底层基于 Apple Accessibility 架构，从 OS X 10.2 开始引入，10.10 之后升级为方法化的现代 API。

### 1. 核心功能分类

#### （1）UI 元素树与属性查询

- 层级遍历：以系统根元素为起点，遍历所有运行中应用、窗口、控件的 UI 元素树，每个元素对应一个`AXUIElement`对象
- 属性读取：获取元素的角色（按钮、文本框、菜单等）、标签、值、位置尺寸、父元素、子元素、是否可聚焦、是否启用等数十种标准属性
- 元素搜索：按角色、标题、位置、值等条件筛选查找目标 UI 元素，支持递归搜索

#### （2）UI 元素交互控制

- 动作执行：对元素触发原生交互动作，如点击 (`AXPress`)、展开 / 收起、滚动、聚焦、选中文本、触发菜单命令等
- 值修改：直接修改可编辑元素的文本内容、滑块数值、复选框状态等
- 全局控制：切换前台应用、最小化 / 关闭窗口、触发系统全局快捷键等

#### （3）通知与事件监听

- 系统级事件：应用启动 / 退出、窗口创建 / 关闭、焦点元素变化、菜单弹出 / 关闭
- 元素级事件：文本内容变更、选中状态变化、控件值改变、布局变更
- 支持异步回调机制，实时响应 UI 状态变化，无需轮询

#### （4）系统辅助功能状态检测

- 查询系统辅助功能开关：VoiceOver（旁白）、全键盘访问、屏幕缩放、智能反转、减弱动态效果、降低透明度、粗体文本等
- 监听辅助功能开关变更通知，让 App 实时适配显示与交互逻辑

#### （5）输入模拟与事件拦截

基于 Quartz Event Services (CGEvent) 子系统：

- 全局模拟键盘按键、鼠标移动 / 点击、滚轮滚动等输入事件
- 拦截全局输入事件，可修改、丢弃事件（用于自定义输入设备、宏工具、无障碍输入设备）
- 事件合成无需特殊权限，事件拦截必须获得无障碍权限

#### （6）应用自身无障碍化适配

基于 AppKit 的 `NSAccessibility` 协议体系：

- 为自定义控件 / 视图添加无障碍角色、标签、操作提示、值描述
- 遵循标准无障碍协议（按钮、滑块、文本框、表格等协议），让 VoiceOver 等辅助技术自动识别
- 自定义无障碍元素层级，优化辅助技术的导航逻辑

#### （7）扩展能力

- 文本转语音（Speech Synthesis API）、语音识别集成
- 快捷指令、辅助触控的深度集成
- 全屏缩放、光标放大、悬停键入等系统级辅助功能的配置接口

### 2. 技术层级

表格

| 层级   | API 类型                    | 原生语言            | 所属框架                         | 适用场景                           |
| ------ | --------------------------- | ------------------- | -------------------------------- | ---------------------------------- |
| 高层   | SwiftUI / AppKit 无障碍属性 | Swift / Objective-C | SwiftUI / AppKit                 | 应用自身无障碍化适配               |
| 中层   | NSAccessibility 协议        | Objective-C / Swift | AppKit                           | 自定义控件无障碍化、高级无障碍功能 |
| 底层   | AXUIElement C API           | C / Objective-C     | ApplicationServices (HIServices) | 无障碍客户端开发、跨应用 UI 自动化 |
| 输入层 | CGEvent (Quartz)            | C                   | CoreGraphics                     | 全局输入模拟与拦截                 |

## 二、官方技术文档汇总

### 1. 总览与设计指南

- [Accessibility 官方总览（英文）](https://developer.apple.com/documentation/Accessibility)：全 Apple 平台无障碍 API 总览、新特性、样本代码，覆盖 macOS/iOS/visionOS 等Apple Deve...
- [macOS 无障碍编程指南（归档）](https://developer.apple.com/library/archive/documentation/Accessibility/Conceptual/AccessibilityMacOSX/)：OS X 无障碍架构完整原理讲解，适合理解底层设计思路Apple Deve...
- [人机界面指南 - 无障碍（中文）](https://developer.apple.com/cn/design/human-interface-guidelines/accessibility)：无障碍设计规范与产品最佳实践Apple Deve...

### 2. AppKit 无障碍 API（应用自身适配）

- [Accessibility for AppKit](https://developer.apple.com/documentation/appkit/accessibility-for-appkit)：AppKit 无障碍开发官方入口Apple Deve...
- [NSAccessibilityProtocol 协议参考](https://developer.apple.com/documentation/AppKit/NSAccessibilityProtocol)：完整的无障碍属性、方法协议参考手册Apple Deve...
- [集成无障碍到你的 App](https://developer.apple.com/documentation/accessibility/integrating-accessibility-into-your-app)：自定义控件无障碍化分步开发指南Apple Deve...

### 3. 底层 AXUIElement 客户端 API

- [HIServices 框架参考](https://developer.apple.com/documentation/coreservices/hiservices)：底层 C 语言 AXUIElement API 官方文档，包含所有核心函数定义
- 核心函数：`AXUIElementCreateSystemWide`、`AXUIElementCopyAttributeValue`、`AXUIElementPerformAction`、`AXUIElementAddNotification` 等

### 4. 输入事件 API

- [Quartz Event Services 参考](https://developer.apple.com/documentation/coregraphics/quartz_event_services)：CGEvent 事件合成与拦截官方文档

## 三、Rust 语言调用支持

Rust 生态已有多个成熟 crate 封装了 macOS 无障碍 API，分为**底层原生绑定**和**跨平台封装**两类。所有涉及跨应用 UI 访问、输入拦截的功能，都需要在「系统设置 → 隐私与安全性 → 无障碍」中授予对应程序权限。

### 1. 核心库推荐

#### （1）`axuielement` — 原生 AXUIElement 绑定

最接近原生 C API 的 Rust 绑定，直接封装 macOS HIServices 框架的 AXUIElement 系列函数，是 macOS 无障碍客户端开发的基础库。

- 完整支持元素遍历、属性读写、动作执行、通知监听
- Crate 地址：[https://crates.io/crates/axuielement](https://crates.io/crates/axuielement)

**基础示例：获取当前聚焦的应用**

```
use axuielement::AXElement;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 检查是否已获得无障碍权限
    if !axuielement::is_process_trusted() {
        eprintln!("请在系统设置中授予无障碍权限后重启程序");
        return Ok(());
    }

    // 获取系统级根元素
    let system = AXElement::system_wide()?;
    // 获取当前聚焦的应用元素
    let focused_app = system.element_attribute("AXFocusedApplication")??;
    // 读取应用名称属性
    let app_name: String = focused_app.attribute_value("AXTitle")??;
    
    println!("当前聚焦应用: {}", app_name);
    Ok(())
}
```

#### （2）`xa11y` — 跨平台无障碍客户端库

提供统一的跨平台 API，底层在 macOS 上基于 AXUIElement，Windows 上基于 UI Automation，Linux 上基于 AT-SPI2，适合开发跨平台桌面自动化 / 辅助工具。

- 支持 CSS 选择器定位元素、点击输入、滚动、输入模拟
- Crate 地址：[https://docs.rs/xa11y](https://docs.rs/xa11y)

**基础示例：定位并点击按钮**

```
use std::time::Duration;
use xa11y::*;

fn main() {
    // 按名称查找 Safari 应用
    let app = App::by_name("Safari", Duration::from_secs(5))
        .expect("未找到 Safari 应用");
    
    // 用选择器定位 OK 按钮并点击
    app.locator(r#"button[name="OK"]"#)
        .press()
        .expect("点击失败");
}
```

#### （3）`cgevents` — Quartz 事件绑定

专门封装 macOS Quartz Event Services，用于全局键盘鼠标输入模拟与事件拦截。

- 支持模拟按键、鼠标移动点击、滚轮事件；全局监听 / 拦截输入事件
- Crate 地址：[https://crates.io/crates/cgevents](https://crates.io/crates/cgevents)

**示例：全局监听键盘事件**

```
use cgevents::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 创建键盘事件监听器
    let tap = EventTap::keyboard(|event| {
        println!("按键码: {}, 标志位: {:?}", event.keycode(), event.flags());
        TapAction::Pass // 传递事件；返回 TapAction::Drop 则拦截事件
    })?;
    
    tap.run(); // 阻塞运行事件循环
    Ok(())
}
```

#### （4）其他相关库

- `axterminator`：基于无障碍 API 的 macOS GUI 测试框架，支持后台 UI 操作，元素访问延迟低于 1ms
- `agent-desktop`：跨平台桌面自动化 CLI 工具，底层基于原生无障碍 API，适合 AI Agent 调用
- `cocoa` + `objc`：原生 Objective-C 绑定，可直接调用 NSAccessibility 协议，适合应用自身无障碍化开发

### 2. 开发注意事项

1. **权限要求**：跨应用 UI 访问、输入事件拦截必须获得系统无障碍权限，首次运行会触发权限申请，授权后需重启程序
2. **架构兼容**：所有主流库均同时支持 Apple Silicon (arm64) 和 Intel (x86_64) 架构
3. **系统版本**：核心 API 兼容 macOS 10.10+，部分新特性需要 macOS 11.0+
4. **性能说明**：AXUIElement 基于进程间通信，频繁调用有开销，优先使用事件驱动而非轮询
5. **沙盒限制**：Mac App Store 沙盒应用无法使用无障碍客户端 API，仅可做自身应用无障碍适配