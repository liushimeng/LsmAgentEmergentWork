# MCP_Window_Use 横向对比与 Agent 窗口操控能力映射

> 2026-09-20 第 98 轮。本文是「MCP_Window_Use 与其他 Agent 项目的窗口操控能力横向对比」
> +「laew 可借鉴方向」+「laew 已实现的对照」清单,供知识库回填 `专题-laew实现进度对照表`
> 与后续 99+ 轮设计参考。
>
> 前置阅读:
> - `docs/Agent源码调研/` 下各 Agent 综合文档(atomcode/claudecode/openclaw/opencode/pi);
> - `docs/Agent源码调研/专题/专题-MCP架构深度分析.md` MCP 工具层对比;
> - `docs/MCP_Window_Use/01-设计与解决方案.md` laew MCP_Window_Use 工具架构。
>
> 编写依据:`docs/Agent源码调研/` 各 Agent 文档的窗口操控章节 +
> `docs/Agent源码调研/专题-MCP架构深度分析.md` + 知识库 gap(L2244-L2280 维度)。

---

## 1. 对比总表

| 项目 | 窗口操控能力入口 | 形态 | 平台门控 | 路线选择 | LLM 集成 |
|------|----------------|------|---------|---------|---------|
| **laew**(本项目) | `MCP_Window_Use` 单工具 | MCP 风格服务(in-proc) | macOS / Windows | **T1 无障碍 → T2 消息 → T3 物理** 四层链 | SubAgent-Work 工具集 |
| **atomcode**(Rust) | `atomcode-code/src/mcp/window.rs` + `ToolActionProvider` | MCP server(独立 stdio) | 三平台 | **T1 UIA → T2 物理** 两层 | MCP client 工具集 |
| **claudecode**(TS/Bun) | `computer-use` 工具 + Bash 兜底 | MCP server | macOS / Windows | **T1 截图识别 → T2 物理**(无系统 API) | Claude 工具集 |
| **openclaw**(TS) | `mcp-playwright` + `mcp-window`(自研双栈) | MCP server | macOS / Windows / Linux | **T1 AX/UIA → T2 CDP/Playwright** 双栈 | MCP gateway + adapter |
| **opencode**(TS/Bun) | 无窗口操控(纯命令行/浏览器) | — | — | — | — |
| **pi**(TS) | 无窗口操控(纯 TUI) | — | — | — | — |
| **deepseek-harness**(TS) | 无桌面 GUI(纯 CLI) | — | — | — | — |
| **claudecode computer-use** | Anthropic API 内置 `computer_use_20241022` 工具 | 远程 API | 跨平台(Linux VM 接管) | 截图 + 坐标(无系统 API) | Claude SDK 工具 |

## 2. 各项目窗口操控详解

### 2.1 laew MCP_Window_Use(本项目)

**架构**:13 个 action 单工具(open / list / find / inspect / control / ocr / screenshot
/ capability_probe / osascript_run / chat_send / chat_loop / input_batch / run_sequence),
按 MCP 服务调用语义设计,in-proc 实现,Agent 循环只见到一个 ToolDef。

**核心特色**:
- **四层优先级链**(第 90 轮):UIA Pattern / AX Action → Win32 消息 / 消息不可达 →
  物理鼠标键盘 / 全部失败;
- **复合 action**:chat_send(原子发送)、chat_loop(长时多轮)、input_batch(键盘+鼠标
  +控件树复合步骤批处理)、run_sequence(连续工作模式含 assert_text/wait_for_text
  /wait_front + 焦点守护 + 落盘记录);
- **平台门控**:仅 macOS/Windows 注册,Linux 不出现工具定义;
- **能力矩阵**:capability_probe 返回真实能力(accessibility/screen_recording/
  coordinate_input),LLM 据此选路线;
- **前台焦点守卫**:chat_send/chat_loop/run_sequence 三层渐进式焦点守护,
  连续 3 次焦点重夺失败止损(防止用户切窗后消息误发)。

### 2.2 atomcode `ToolActionProvider`

**架构**:`atomcode-code/src/mcp/window.rs` 独立 MCP server,stdio 协议;
`ToolActionProvider` trait 把窗口动作抽象为统一接口。

**核心特色**:
- **两层链**(无消息层):UIA Pattern → 物理 SendInput;
- **CodeIntel 七件套**(LSP/Tree-sitter/call hierarchy)与窗口操控解耦;
- **Bash 沙箱**(第 22 轮 landlock/seccomp/cgroups);
- **失败语义**:pattern 失败不降级,直接 Err(laew 是降级)。

### 2.3 claudecode `computer-use`

**架构**:Anthropic API 内置工具 `computer_use_20241022`,模型直接返回
`screenshot+action`(screenshot + left_click + type + key);截图 OCR 走云端(Vision API)。

**核心特色**:
- **零本地依赖**:截图发云端、动作由云端模型决策;适合 Linux VM 接管;
- **零本地 UI 自动化代码**:全部走 Anthropic API;
- **缺点**:每步截图耗时 1-3s、上下文占用大(token 与延迟双高);
- **不适合微信等自绘 UI**:截图盲点 + 时延累积。

### 2.4 openclaw `mcp-playwright` + `mcp-window` 双栈

**架构**:`openclaw/mcp-window/` 自研 6300+ 行双栈(CDP/Playwright + 平台原生),
MCP server stdio 协议。

**核心特色**:
- **CDP/Playwright 主路线**(类似 laew MCP_Web_Use):浏览器场景统一;
- **平台原生桥路线**(AX/UIA):桌面 GUI 场景;
- **openclaw Adapter 层**统一两栈调用,LLM 不知道差异;
- **mcp-windows-mcp** 项目化封装,1700+ 行;
- **缺点**:双栈维护成本高(laew 单栈 + 输入底座更轻)。

### 2.5 pi / opencode / deepseek-harness

均**无桌面 GUI 自动化能力**,定位为命令行/终端/服务器端 Agent。

## 3. laew vs 其他项目的关键差异

| 维度 | laew | atomcode | claudecode | openclaw |
|------|------|----------|------------|----------|
| **形态** | MCP 风格 in-proc 工具 | MCP server stdio | 云端 API 工具 | MCP server stdio |
| **链深度** | **4 层(T1 无障碍→T2 消息→T3 物理→T4 外部命令)** | 2 层 | 1 层(纯截图) | 2 栈(CDP/原生) |
| **失败策略** | **逐层降级 + 结构化 next_action** | 直接 Err | 云端重试 | Adapter 重试 |
| **复合 action** | ✅ 5 个(chat_send/loop/input_batch/run_sequence/osascript_run) | ❌ | ❌ | ❌ |
| **能力探测** | ✅ capability_probe(action=) | ❌ | ❌ | mcp_capability |
| **焦点守护** | ✅ 三层渐进 + 连续 3 次止损 | ❌ | ❌ | ❌ |
| **UI 装饰层过滤** | ✅ ControlViewWalker(98 轮 G9) | ❌ | N/A | ✅(CDP 天然无装饰层) |
| **OCR 失败兜底** | ✅ capability_probe + osascript_fallback | ✅(Bash 兜底) | 云端重试 | CDP 截图 |
| **中文 UI 同义词表** | ✅ Unicode Modifier Letter 归一 + 30+ 中文 UI 别名 | ❌ | N/A | 部分 |
| **平台门控** | ✅ cfg!(macos/windows) | ✅(三平台) | 跨平台(Linux VM) | ✅(三平台) |
| **Chat 原子发送** | ✅ chat_send(5 路线)+ chat_loop(N 轮) | ❌ | ❌ | ❌ |

## 4. laew 可借鉴方向(99+ 轮候选)

| # | 借鉴来源 | 借鉴内容 | 优先级 | laew 当前状态 |
|---|---------|---------|------|--------------|
| K1 | atomcode CodeIntel | 把窗口操控与代码图索引联动(操作后自动 inspect 关联文件) | P2 | 未实现 |
| K2 | openclaw 双栈 | 桌面 GUI + 浏览器场景 Adapter 统一,LLM 无感差异 | P2 | 部分(SubAgent-Work 自选) |
| K3 | claudecode computer-use | 截图 → 云端 Vision 决策(适合 Linux 平台) | P1 | 未实现,Linux 不在门控 |
| K4 | atomcode landlock 沙箱 | Bash 工具联动窗口操控的沙箱隔离 | P1 | 未实现(Bash 黑名单基础) |
| K5 | openclaw mcp_capability | 把 capability 探测做成 MCP 标准能力声明 | P2 | 部分(capability_probe) |
| K6 | opencode Durable Object | capability 缓存(已实现 L1884:OnceLock + TTL) | ✅ | 已实现 |
| K7 | pi Lane 三态 | 窗口操控操作排队 + 串行化 | P2 | run_sequence 已串行 |

## 5. laew 已实现的独特能力(优于其他项目)

| 能力 | 轮次 | 对比优势 |
|------|------|---------|
| **四层优先级链** | 第 90 轮 | atomcode 仅 2 层;claudecode 仅 1 层 |
| **复合 action chat_send/chat_loop/input_batch/run_sequence** | 第 85/88/90/91 轮 | 所有对比项目都无 |
| **前台焦点守卫 + 3 次止损** | 第 88 轮 | 其他项目无 |
| **macOS capability 真实矩阵**(TCC preflight) | 第 87 轮 | 其他项目简化探测 |
| **osascript_fallback 路线** | 第 86 轮 | 仅 laew |
| **中文 UI 同义词 + Unicode 归一** | 第 62 轮 | 仅 laew |
| **chat_log + sequence_log 落盘可追溯** | 第 86/91 轮 | claudecode 走云端无 |
| **run_sequence assert_text/wait_for_text 自绘 UI OCR 兜底** | 第 93 轮 | 仅 laew |
| **ControlViewWalker 装饰层过滤**(98 轮) | G9 | openclaw 通过 CDP 天然无,laew 通过 UIA 显式过滤 |

## 6. 回填知识库对照(状态标记)

| 维度 | 编号 | 状态 | 实现位置 |
|------|------|------|---------|
| 窗口操控 MCP 工具(单工具+action 枚举) | L2244 | ✅ 第 84 轮 | `tools/mcp_window_use/mod.rs` |
| 平台门控(macOS/Windows 注册) | L2245 | ✅ 第 84 轮 | `mcp_window_use_available()` |
| 优先级链(无障碍→消息→物理) | L2246 | ✅ 第 90 轮 | `windows.rs::act` 四层 |
| 复合 action(chat/sequence) | L2247 | ✅ 第 85/91 轮 | `chat.rs` + `sequence.rs` |
| capability_probe(action=) | L2248 | ✅ 第 86 轮 | `chat.rs::run_capability_probe` |
| 焦点守护 + 止损 | L2249 | ✅ 第 88 轮 | `ensure_frontmost` + 3 次止损 |
| 失败信息 next_action 引导 | **G12** | ✅ **第 98 轮** | `windows.rs::act(GetText)` |
| UIA TextPattern 读取 | **G5** | ✅ **第 98 轮** | `windows.rs::act(GetText)` T1c |
| UIA SelectionPattern 容器级 | **G6** | ✅ **第 98 轮** | `windows.rs::act(GetText)` T1d |
| 控件元信息(help/access/accelerator/is_selected) | **G7** | ✅ **第 98 轮** | `ControlNode` +4 字段 |
| ControlViewWalker 装饰层过滤 | **G9** | ✅ **第 98 轮** | `uia_build_tree` |
| Bash 沙箱联动 | — | ⏳ 未实现 | 候选 99+ 轮 K4 |
| Linux 平台 GUI 操控 | — | ⏳ 平台门控排除 | 候选 99+ 轮 K3 |

## 7. 结论

laew MCP_Window_Use 在窗口操控能力维度上,**已领先于 atomcode / claudecode / openclaw**,
特别在「复合 action」「焦点守护」「优先级链深度」「失败 next_action」四处独特。
后续可借鉴的主要是 **K1(CodeIntel 联动)/ K4(Bash 沙箱)/ K3(Linux 接管)** 三个方向,
与 laew 当前定位互补而非冲突。
