# Windows 界面自动化 API 与 Rust 调用全面指南

Windows 平台操作第三方软件界面，本质是通过系统 API 操纵窗口与控件，核心围绕**窗口句柄（HWND）**和系统消息机制。下面从底层 API 体系、核心功能实现、Rust 语言调用三个维度完整介绍。

---

## 一、Windows 操作界面的核心 API 体系

Windows 提供两代界面自动化技术，分别适配不同年代的应用：

### 1. 传统 Win32 API（User32.dll）

- **核心思想**：所有控件本质都是窗口，都有唯一标识 `HWND`（窗口句柄），通过发送系统消息实现交互
- **适用场景**：标准 Win32 控件、MFC、VB6、WinForms 等原生桌面程序
- **特点**：轻量高效、直接操作句柄；但无法处理无句柄的自绘控件

### 2. UI Automation API（UIA）

- **核心思想**：微软新一代自动化标准，基于 COM 接口，通过「UI 元素树」访问界面，不依赖 HWND
- **适用场景**：WPF、UWP、WinUI、Electron、Qt 自绘等现代应用
- **特点**：通用性最强、支持控件模式化操作；但接口更复杂，性能略低

---

## 二、核心功能与原生 API 详解

所有操作的基础是窗口句柄 `HWND`，它是系统中每个窗口 / 控件的唯一数字标识。

### 1. 获取顶层窗口句柄

#### 核心 API

表格

| API 函数              | 功能                                 | 原型说明                                          |
| --------------------- | ------------------------------------ | ------------------------------------------------- |
| `FindWindowW`         | 按类名 + 窗口标题精准查找顶层窗口    | `HWND FindWindowW(LPCWSTR 类名, LPCWSTR 标题)`    |
| `EnumWindows`         | 枚举所有顶层窗口，通过回调自定义过滤 | `BOOL EnumWindows(WNDENUMPROC 回调, LPARAM 参数)` |
| `GetForegroundWindow` | 获取当前激活的前台窗口               | `HWND GetForegroundWindow()`                      |
| `GetWindow`           | 根据 Z 序 / 父子 / 兄弟关系获取窗口  | `HWND GetWindow(HWND hWnd, UINT 命令)`            |

> 
> 窗口类名和标题可以用 Spy++ 工具查看；模糊匹配需要用 `EnumWindows` 枚举后自行判断。

### 2. 遍历子控件（子窗口）

标准 Win32 控件本身也是窗口，拥有独立 HWND，可以递归遍历。

#### 核心 API

表格

| API 函数           | 功能                                                 |
| ------------------ | ---------------------------------------------------- |
| `EnumChildWindows` | 递归枚举指定父窗口的所有子控件，通过回调逐个返回句柄 |
| `FindWindowExW`    | 在子窗口中按类名 / 标题查找，支持按 Z 序逐个遍历     |
| `GetDlgItem`       | 通过控件 ID 获取子窗口句柄（对话框 / 标准窗体常用）  |

> 
> ⚠️ **重要限制**：WPF、Electron、Qt 自绘、DirectUI 等界面没有子 HWND，无法用该方法遍历，必须使用 UIA 或坐标模拟。

### 3. 获取控件 / 窗口文本

#### 核心 API

表格

| API / 消息                        | 适用场景                                               |
| --------------------------------- | ------------------------------------------------------ |
| `GetWindowTextW`                  | 快速获取窗口标题、静态文本、按钮、编辑框等标准控件文本 |
| `SendMessageW + WM_GETTEXT`       | 通用方式，可读取编辑框、列表框、组合框等完整内容       |
| `SendMessageW + WM_GETTEXTLENGTH` | 先获取文本长度，用于分配接收缓冲区                     |

> 
> 系统默认保护密码框内容，普通进程无法读取；跨进程读取文本时系统会自动处理内存复制。

### 4. 向控件输入文本

有两种技术路线，各有优劣：

#### 方案 A：消息直接赋值（推荐标准控件）

- `SendMessageW + WM_SETTEXT`：直接覆盖控件全部文本，跨进程可用
- `SendMessageW + EM_REPLACESEL`：替换编辑框选中内容，可模拟逐字输入效果

#### 方案 B：模拟键盘输入（兼容所有控件）

- `SendInput`：模拟真实键盘按键，支持组合键、中文（需配合输入法）
- 旧 API `keybd_event` 已被官方废弃，推荐统一使用 `SendInput`

表格

| 方案     | 优点                         | 缺点                             |
| -------- | ---------------------------- | -------------------------------- |
| 消息赋值 | 速度快、后台运行、不干扰用户 | 仅支持标准控件，部分程序拦截无效 |
| 模拟输入 | 兼容性 100%，和真人操作一致  | 需要窗口激活、速度慢、无法后台   |

### 5. 点击按钮 / 控件

同样分为两种实现路线：

#### 方案 A：消息直接点击（标准按钮）

- `SendMessageW + BM_CLICK`：直接向按钮发送点击消息
- 等价于鼠标左键按下 + 抬起，会触发按钮完整点击事件

#### 方案 B：模拟鼠标点击（自绘控件）

1. `GetWindowRect` 获取控件屏幕坐标
2. 计算控件中心点坐标
3. `SendInput` 发送鼠标移动 + 左键按下 + 抬起事件

### 6. 消息发送机制

- `SendMessage`：**同步**发送，等待目标窗口处理完成后返回结果
- `PostMessage`：**异步**发送，放入消息队列立即返回，不等待处理
- 常用消息：`WM_LBUTTONDOWN/UP`（鼠标）、`WM_KEYDOWN/UP`（键盘）、`WM_COMMAND`（控件通知）

---

## 三、进阶方案：UI Automation API（UIA）

### 为什么需要 UIA

传统 Win32 API 依赖 HWND，面对现代应用时会失效。UIA 是微软官方的自动化标准，通过「UI 元素树」访问整个桌面界面，是目前最通用的方案。

### 核心能力

- 遍历整个桌面的 UI 元素树，替代 `EnumWindows/EnumChildWindows`
- **控件模式**：标准化操作接口
  - `InvokePattern`：触发点击（按钮、链接）
  - `ValuePattern`：读写文本（输入框）
  - `SelectionPattern`：选择列表项
  - `TogglePattern`：切换复选框 / 单选框
- 支持属性获取（名称、类名、AutomationId、坐标等）
- 支持事件监听（按钮点击、文本变化、窗口打开等）

---

## 四、Rust 语言调用 Windows API

Rust 生态有两个主流 Windows API 绑定库，下面分别给出完整示例。

### 方案 1：`winapi` crate（社区经典版）

社区维护的零成本绑定，历史久、生态广，用法最接近原生 C API，需要手动处理字符串和不安全代码。

#### 1. 依赖配置（Cargo.toml）

```
[dependencies]
winapi = { version = "0.3", features = [
    "winuser",      # 窗口与消息 API
    "windef",       # HWND 等基础类型
    "ntdef",
    "winbase",
] }
```

#### 2. 工具函数：字符串转换

Windows 原生 API 使用 UTF-16 宽字符，需要转换 Rust 字符串：

```
use std::ffi::OsStr;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;

/// Rust 字符串 -> Windows 宽字符数组（自动补 \0 结尾）
fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(once(0))
        .collect()
}
```

#### 3. 完整示例：查找窗口 + 输入文本 + 点击按钮

```
use winapi::shared::windef::HWND;
use winapi::um::winuser::{
    FindWindowW, FindWindowExW, GetWindowTextLengthW, GetWindowTextW,
    SendMessageW, BM_CLICK, WM_SETTEXT,
};

fn main() {
    unsafe {
        // 1. 查找顶层窗口（示例：记事本）
        let window_title = to_wide("无标题 - 记事本");
        let hwnd = FindWindowW(std::ptr::null(), window_title.as_ptr());
        
        if hwnd.is_null() {
            println!("未找到目标窗口");
            return;
        }
        println!("窗口句柄: {:?}", hwnd);

        // 2. 获取窗口标题
        let len = GetWindowTextLengthW(hwnd) + 1;
        let mut buf = vec![0u16; len as usize];
        GetWindowTextW(hwnd, buf.as_mut_ptr(), len);
        let title = String::from_utf16_lossy(&buf[..(len - 1) as usize]);
        println!("窗口标题: {}", title);

        // 3. 查找子控件（编辑框，类名 "Edit"）
        let edit_class = to_wide("Edit");
        let edit_hwnd = FindWindowExW(
            hwnd, 
            std::ptr::null_mut(), 
            edit_class.as_ptr(), 
            std::ptr::null()
        );

        if !edit_hwnd.is_null() {
            // 4. 向编辑框输入文本
            let text = to_wide("Hello from Rust WinAPI!");
            SendMessageW(edit_hwnd, WM_SETTEXT, 0, text.as_ptr() as isize);
            println!("已写入文本");
        }

        // 5. 查找按钮并点击（示例对话框中的"确定"按钮）
        let btn_class = to_wide("Button");
        let btn_text = to_wide("确定");
        let btn_hwnd = FindWindowExW(
            hwnd, 
            std::ptr::null_mut(), 
            btn_class.as_ptr(), 
            btn_text.as_ptr()
        );

        if !btn_hwnd.is_null() {
            SendMessageW(btn_hwnd, BM_CLICK, 0, 0);
            println!("已点击按钮");
        }
    }
}
```

#### 4. 模拟鼠标点击示例

```
use winapi::um::winuser::{GetWindowRect, mouse_event, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP};
use winapi::shared::windef::{HWND, RECT};

unsafe fn click_at_control(hwnd: HWND) {
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetWindowRect(hwnd, &mut rect);
    
    // 计算控件中心点
    let x = (rect.left + rect.right) / 2;
    let y = (rect.top + rect.bottom) / 2;
    
    // 模拟左键按下+抬起
    mouse_event(MOUSEEVENTF_LEFTDOWN, x as u32, y as u32, 0, 0);
    mouse_event(MOUSEEVENTF_LEFTUP, x as u32, y as u32, 0, 0);
}
```

> 
> 注：`mouse_event` 已被标记为废弃，正式项目推荐使用 `SendInput`。

---

### 方案 2：`windows` crate（微软官方版）

微软官方维护的 Rust for Windows 库，类型安全、自动管理 COM 与字符串，支持现代 API（包括 UIA），是新项目的首选。

#### 1. 依赖配置（Cargo.toml）

```
[dependencies]
windows = { version = "0.52", features = [
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Input_KeyboardAndMouse",
] }
```

#### 2. 完整示例

```
use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, FindWindowExW, GetWindowTextW, SendMessageW,
    BM_CLICK, WM_SETTEXT,
};
use windows::Win32::Foundation::{HWND, WPARAM, LPARAM};

fn main() -> windows::core::Result<()> {
    unsafe {
        // 构造宽字符串
        let title: Vec<u16> = "无标题 - 记事本".encode_utf16().chain(std::iter::once(0)).collect();
        let hwnd = FindWindowW(PCWSTR::null(), PCWSTR::from_raw(title.as_ptr()));

        if hwnd.0 == 0 {
            println!("未找到窗口");
            return Ok(());
        }
        println!("窗口句柄: {:?}", hwnd);

        // 获取窗口标题
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf);
        let title_str = String::from_utf16_lossy(&buf[..len as usize]);
        println!("窗口标题: {}", title_str);

        // 查找编辑框并写入文本
        let edit_class: Vec<u16> = "Edit".encode_utf16().chain(std::iter::once(0)).collect();
        let edit_hwnd = FindWindowExW(
            hwnd, 
            None, 
            PCWSTR::from_raw(edit_class.as_ptr()), 
            PCWSTR::null()
        );

        if edit_hwnd.0 != 0 {
            let text: Vec<u16> = "Hello from windows crate!".encode_utf16().chain(std::iter::once(0)).collect();
            SendMessageW(edit_hwnd, WM_SETTEXT, WPARAM(0), LPARAM(text.as_ptr() as isize));
        }

        // 点击按钮
        let btn_class: Vec<u16> = "Button".encode_utf16().chain(std::iter::once(0)).collect();
        let btn_text: Vec<u16> = "确定".encode_utf16().chain(std::iter::once(0)).collect();
        let btn_hwnd = FindWindowExW(
            hwnd, 
            None, 
            PCWSTR::from_raw(btn_class.as_ptr()), 
            PCWSTR::from_raw(btn_text.as_ptr())
        );

        if btn_hwnd.0 != 0 {
            SendMessageW(btn_hwnd, BM_CLICK, WPARAM(0), LPARAM(0));
            println!("按钮已点击");
        }
    }
    Ok(())
}
```

#### 3. UI Automation 支持

`windows` crate 完整支持 UIA COM 接口（`Windows::UI::UIAutomation` 命名空间），可以无句柄操作 WPF/UWP 等现代应用，是更通用的自动化方案。

---

## 五、常见坑与注意事项

### 1. 字符集

- 永远使用 **Unicode 版本 API（W 后缀）**，不要使用 ANSI 版本（A 后缀）
- 宽字符串必须以 `\0` 结尾，否则会出现乱码或崩溃

### 2. 权限与 UAC

- 如果目标程序以管理员权限运行，你的程序也必须以管理员权限启动，否则无法操作
- 系统服务无法直接操作桌面窗口（跨会话隔离）

### 3. DPI 缩放

- 高 DPI 屏幕下，`GetWindowRect` 获取的坐标会被缩放，导致模拟点击偏移
- 解决方案：调用 `SetProcessDpiAwarenessContext` 让程序感知 DPI

### 4. 无句柄控件

- 遇到 WPF、Electron、Qt 自绘界面时，`EnumChildWindows` 找不到子控件
- 解决方案：优先使用 UI Automation；或配合图像识别做坐标模拟

### 5. 方案选型建议

表格

| 场景                          | 推荐方案                        |
| ----------------------------- | ------------------------------- |
| 标准 Win32 程序、批量后台操作 | `winapi` + 消息机制             |
| 新项目、现代应用、长期维护    | 官方 `windows` crate + UIA      |
| 游戏、强保护程序、自绘界面    | `SendInput` 键鼠模拟 + 图像识别 |