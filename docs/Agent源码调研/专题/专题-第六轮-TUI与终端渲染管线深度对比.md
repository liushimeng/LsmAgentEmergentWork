# 第六轮 · TUI 与终端渲染管线深度对比

> **调研范围**：6 个项目 `atomcode`（Rust）/ `claudecode`（TypeScript Fork Ink）/
> `opencode`（SolidJS + OpenTUI）/ `pi`（TypeScript 自研）/ `openclaw`（Node + Ink 上层）/
> `deepseek-harness`（Web UI, React + anser）。
>
> **对比目标**：渲染模型 / 框架 / 布局引擎 / 状态管理 / 主题 / 国际化 /
> 鼠标键盘协议 / 屏幕缓冲 / 对象池 / 异步渲染 / 选区复制 / 子屏 Modal /
> 进度条 Spinner / 流式 Markdown 等共 14 个维度。
>
> **特别聚焦**：laew 当前 `src/tui/`（自研 Screen trait + Frame + present 全量重绘）
> 的具体借鉴路线图（P0–P2）。

---

## 目录

1. [整体对比矩阵](#1-整体对比矩阵)
2. [渲染模型：全量重绘 vs Retained Cell-based](#2-渲染模型)
3. [框架与语言选择](#3-框架与语言选择)
4. [布局引擎](#4-布局引擎)
5. [屏幕缓冲：Cell-based 与对象池](#5-屏幕缓冲与对象池)
6. [Diff 算法与 ANSI 序列化](#6-diff-算法与-ansi-序列化)
7. [异步渲染：Worker Thread / 主线程分时](#7-异步渲染)
8. [鼠标协议与 SGR 模式选择](#8-鼠标协议)
9. [键盘协议：Kitty CSI-u / xterm modifyOtherKeys](#9-键盘协议)
10. [同步输出与防闪烁 DECSET 2026](#10-同步输出)
11. [对象池与 GC 优化](#11-对象池与-gc-优化)
12. [子屏 Modal / Alternate Screen](#12-子屏-modal)
13. [主题与国际化](#13-主题与国际化)
14. [进度条 / Spinner](#14-进度条与-spinner)
15. [选区与复制](#15-选区与复制)
16. [流式 Markdown 渲染](#16-流式-markdown-渲染)
17. [laew 借鉴路线图（P0/P1/P2）](#17-laew-借鉴路线图)
18. [附录：行号速查表](#18-附录行号速查)

---

## 1. 整体对比矩阵

| 维度 | atomcode | claudecode Ink Fork | opencode (OpenTUI) | pi (自研) | openclaw (terminal-core) | deepseek-harness (Web) | laew 现状 |
|------|----------|--------------------|--------------------|-----------|--------------------------|------------------------|-----------|
| **语言** | Rust | TypeScript | TypeScript | TypeScript | TypeScript | TypeScript + anser | Rust |
| **UI 框架** | 自研 Renderer trait | 自研 Ink Fork（React Reconciler + Yoga） | `@opentui/solid`（SolidJS） | 自研 Component/Container + LayoutBox | Ink（vadimdemedes/ink） + pi-tui | React + Web VDOM + anser ANSI 解析 | 自研 Screen trait + Frame |
| **渲染模型** | Retained cell-based + per-frame diff | Retained Int32Array cell-based + Yoga 全布局 | OpenTUI retained cell-based | Differential line-string diff | Ink 渲染（React → Yoga → cell-grid）+ 差异 line | DOM（React） + anser 解析 ANSI → `<span>` | **全量重绘**（alternate screen 内 `Clear(All)` 后重写） |
| **布局引擎** | 自研 (row-based menu math) | Yoga (Flexbox) WASM | Yoga via OpenTUI | 自研 `LayoutBox` 树 + `intersect` clip | Yoga via Ink | CSS Modules + Flexbox/Grid | 无布局引擎（手动坐标） |
| **屏幕缓冲** | `Vec<Vec<Cell>>` (W×H Cell grid) + prev_cells | Packed `Int32Array` (8 bytes/cell, char+style+hyperlink+width) | OpenTUI cell grid | 无 cell grid；按行 string + `CURSOR_MARKER` APC | Ink cell grid（vadimdemedes） | DOM `<span>` 序列 | `Vec<Cell>` 行优先 |
| **对象池** | 无（Cell 是 Rust struct,无 GC） | CharPool / StylePool / HyperlinkPool（interning） | 依赖 OpenTUI 实现 | 无（字符串拼接） | 无 | 无 | 无 |
| **异步渲染** | **专用 `tuix-render` worker thread**（30-60ms 解耦 event loop） | 主线程 React commit + log-update 节流 | 主线程 SolidJS + renderer frame loop | 主线程 + `requestRender` 16ms 节流 | 主线程 + coalesced-refresh（37 行） | 浏览器 RAF | 主线程同步（管道冒烟回退） |
| **状态管理** | `UiLine` enum + `StatusLine` struct | React `useState` + props | SolidJS `createStore` + Signals | 自定义 `TuiBase` + Container 树 | React state + Cordis plugin runtime | React state + Cordis `Fiber` epoch | 命令式 `pub struct` 字段 |
| **主题系统** | 16-color + 256-color（光感分 light/dark） | React `useTheme` + CSS variables | `@opentui/core` theme JSON + palette prewarm | 自定义 theme module（`theme.ts`） | `LOBSTER_PALETTE` 9 tokens + NO_COLOR | CSS variables + 8/16-color token map | `tui::theme::FG` 等常量 |
| **国际化** | `Msg<'a>` enum + `t(Msg::…)` lookup | 无 i18n（英文硬编码 + 命令本地化） | 无 i18n（界面英文） | 无 i18n（界面英文） | 部分本地化（CLI prompts via chalk） | i18n context（`locale/` 子包） | 中文文案硬编码 |
| **鼠标协议** | SGR 1006/1002/1003（auto-detect via env） | SGR 1006 + alt press detection（macOS） | OpenTUI `useMouse: true`（gate by config） | Kitty / SGR 自动协商 | Ink 默认 + override | DOM `onMouseDown` | crossterm 默认（SGR/X10 自动） |
| **键盘协议** | Kitty CSI-u + auto-detect `KITTY_WINDOW_ID` 等 | Kitty CSI-u + xterm modifyOtherKeys（白名单） | OpenTUI `useKittyKeyboard: {}` | Kitty 7-flag 协商（`CSI >7u`） | 标准 Kitty | DOM `onKeyDown` | crossterm 默认 |
| **同步输出 DEC 2026** | `?2026h/l` 包裹 | `BSU/ESU` 包裹（tmux 跳过） | OpenTUI 内置 | 无 | 无（Ink 内置） | 不适用（DOM） | 无 |
| **Alternate screen** | `RetainedRenderer` 全屏 + 90+ modals 走 alt | `<AlternateScreen>` 组件（Ink） | OpenTUI alt screen | `tui-alt-screen.ts` 全屏模式（搜索） | Ink 内置 | 不适用 | `engine::enter_alt/leave_alt`（provider_list/form/del） |
| **选区** | 鼠标 drag → `CopyRun[]` + soft-wrap + OSC 52 clipboard | Mouse drag → `SelectionState` + accumulator | 依赖 OpenTUI 选区机制 | 复制基于 `compositeTuiLine` 行内 extract | Ink 默认 | DOM `window.getSelection()` | 无（终端原生 select） |
| **Spinner** | Unicode glyph `◐` 或 ASCII `\|/-\`（按 caps） | OpenTUI `register-spinner.ts` | OpenTUI + frame | `loader.ts`（4-frame 全角符号） | chalk + ink-spinner | CSS animation | 无（仅 cursor blink） |
| **流式 Markdown** | 自研 `markdown.rs`（3115 行,line-oriented state machine） | anser 解析 + log-update 增量 | OpenTUI markdown | `markdown.ts` 自研（按行渲染） | marked-terminal + chalk | remark + mdast + anser | 无 |
| **总规模（TUI 部分）** | 70k+ 行 Rust（atomcode-tuix crate） | 13,306 行 TS（96 文件） | 依赖外部 OpenTUI 包 | ~4,600 行 TS（自研 tui 包） | terminal-core ~30 文件 + Ink | ui-* 30+ 包 | ~2,820 行 Rust |

---

## 2. 渲染模型

### 2.1 全量重绘（laew 现状）

laew 在子屏走的是**全量重绘**——alternate screen 内 `Clear(All)` 后逐行写：

`/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/tui/engine.rs:213-243`

```rust
pub fn present(frame: &Frame) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    execute!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;

    let w = frame.area.width as usize;
    let prev_fg = Color::Reset;     // ← 不做 diff,每行 reset
    let prev_bg = Color::Reset;
    let prev_attr = Attribute::Reset;

    for y in 0..frame.area.height {
        execute!(stdout, MoveTo(0, y))?;
        let mut line = String::with_capacity(w);
        for x in 0..frame.area.width {
            let cell = &frame.cells[(y as usize) * w + (x as usize)];
            line.push(cell.ch);
        }
        execute!(stdout, ResetColor,
            SetForegroundColor(prev_fg),
            SetBackgroundColor(prev_bg),
            SetAttribute(prev_attr),
            Print(&line))?;
    }
    execute!(stdout, ResetColor)?;
    stdout.flush()?;
    Ok(())
}
```

设计取舍（CLAUDE.md 注释）：**适合 < 30 行的子屏**。`Screen` 不直接写 stdout，只往 `Frame` 填充 Cell。

### 2.2 atomcode：Retained Cell-based + Diff

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/render/screen.rs:46-92` 定义双缓冲 cell grid：

```rust
pub struct Screen {
    cells: Vec<Vec<Cell>>,         // 当前正在构建的帧
    prev_cells: Vec<Vec<Cell>>,    // 上一帧发到 terminal 的内容（diff 基准）
    width: u16, height: u16,
    cursor: Option<(u16, u16)>,
    cursor_visible: bool,
    physical_dirty: bool,          // 物理 terminal 与 prev_cells 不同步时=true
    last_cursor: Option<(u16, u16)>,
    last_cursor_visible: Option<bool>,
    jediterm: bool,                // JediTerm 走 per-row tight repaint
    sync_suppressed: bool,         // 外层 DEC 2026 包络期间不嵌套
}
```

`render_diff()`（`screen.rs:233-266`）先把 `prev_cells → cells` 做 patch diff，
再触发 JediTerm 旁路（`serialize_frames_tight`）或标准路径（`serialize_patches`），
最后用 `?2026h` + `?25l` 包络整个 emit，footer 完成后再 `?2026l` + 恢复光标。

**关键设计注释**（`screen.rs:1-35`）：
- **无 DECSTBM scroll region**：footer + body 共用一个 grid，`scroll_up(bottom, n)` 是 O(bottom) 的内部 memcpy，terminal 滚动只通过 diff 表达。
- **`invalidate()`** 不做单独的 cache invalidation path，而是用 `prev_cells` 灌满 `sentinel` Cell（`Cell::sentinel` ch=`U+FFFF`），下一次 diff 自然认为"每格都变"。
- **光标 + 可见性是 frame-level state**：visibility 在 diff 头部隐藏，diff 尾部恢复，绝不与 cell 写入交错（避免 caret 在屏幕间跳来跳去）。

### 2.3 claudecode Ink Fork：Packed Int32Array + Yoga

`/usr/local/LsmGitOpenSource/claudecode/src/ink/screen.ts:332-348`（packed cell layout）：

```ts
// 每格 2 个 Int32: word0=charId, word1=styleId<<17 | hyperlinkId<<2 | width
const STYLE_SHIFT = 17
const HYPERLINK_SHIFT = 2
const HYPERLINK_MASK = 0x7fff
const WIDTH_MASK = 3
function packWord1(styleId, hyperlinkId, width) {
  return (styleId << STYLE_SHIFT) | (hyperlinkId << HYPERLINK_SHIFT) | width
}
```

整屏共 `width × height × 8 bytes`（`screen.ts:476`）：
```ts
const buf = new ArrayBuffer(size << 3) // 8 bytes per cell
const cells = new Int32Array(buf)
const cells64 = new BigInt64Array(buf)  // 共享 buffer,用于 BigInt64 fill
```

设计上写明（`screen.ts:333-365`）：
> "Screen uses a packed Int32Array instead of Cell objects to eliminate GC pressure. For a 200x120 screen, this avoids allocating 24,000 objects."

并且对**软换行 marker** 也单独分配 `Int32Array(height)`（`screen.ts:414, softWrap`）：
> "softWrap[r]=N>0 means row r is a word-wrap continuation of row r-1 (the `\n` before it was inserted by wrapAnsi, not in the source)"

`resetScreen`（`screen.ts:501-544`）走 `cells64.fill(EMPTY_CELL_VALUE)`，**一次 fill** 代替循环 zero——shared buffer + BigInt64 view 的核心收益。

### 2.4 pi：行级字符串 diff（无 cell grid）

`/usr/local/LsmGitOpenSource/pi/packages/tui/src/tui.ts:1-3`：

```ts
/**
 * Minimal TUI implementation with differential rendering
 */
```

pi 没有 cell grid，而是**按行 string diff**。`Component` 接口 (`tui.ts:23-46`)：

```ts
export interface Component {
    render(width: number): string[];            // 输出多行 string
    handleInput?(data: string): void;
    wantsKeyRelease?: boolean;
    invalidate(): void;
}
```

光标位置通过 **APC 零宽 escape** 在 string 中标记（`tui.ts:79, 1189-1207`）：

```ts
export const CURSOR_MARKER = "\x1b_pi:c\x07";
// TUI finds and strips this marker, then positions the hardware cursor there.
```

这是非常巧妙的设计：
- 避免把光标位置塞进 `Component` 协议（focused component emit → TUI base 在最后一行 strip + 还原）。
- 利用 APC（Application Program Command），终端会忽略。
- `requestRender` + `MIN_RENDER_INTERVAL_MS = 16`（`tui.ts:343`）做帧率节流。

### 2.5 openclaw：Ink + 自研 terminal-core

openclaw **复用 Ink 上层**（`src/tui/tui.ts:11`）：

```ts
import {
  Container, Loader, matchesKey, ProcessTerminal,
  Text, TuiMainScreen,
} from "@earendil-works/pi-tui";   // ← 直接用 pi 的 tui 包!
```

注意：**openclaw 与 pi 共享 `@earendil-works/pi-tui`**（两者 monorepo 关系见 openclaw `src/tui/tui.ts:11`）。`packages/terminal-core/src/`（30 文件）只提供 ANSI / 主题 / 选区原语（`palette.ts`, `theme.ts`, `restore.ts`, `ansi.ts`）。

这意味着 openclaw **没有自己的渲染管线**，复用 Ink + pi-tui；只是用 `terminal-core` 包了一层 ANSI helper。

### 2.6 opencode：OpenTUI 框架（外部依赖）

`/usr/local/LsmGitOpenSource/opencode/packages/tui/src/app.tsx:1, 12`：

```ts
import { render, TimeToFirstDraw, useRenderer, useTerminalDimensions } from "@opentui/solid"
import { createCliRenderer, MouseButton } from "@opentui/core"
```

OpenTUI 是 sst/opencode 团队 fork 的 SolidJS TUI 框架，对应 Inkreact-reconciler 设计但换成 Solid 响应式信号。`createCliRenderer` 配置（`app.tsx:194-206`）：

```ts
createCliRenderer({
  externalOutputMode: "passthrough",
  targetFps: 60,
  gatherStats: false,
  exitOnCtrlC: false,
  useKittyKeyboard: {},
  autoFocus: false,
  openConsoleOnError: false,
  useMouse: !Flag.OPENCODE_DISABLE_MOUSE && input.config.mouse,
  consoleOptions: {
    keyBindings: [{ name: "y", ctrl: true, action: "copy-selection" }],
  },
}),
```

> 注：opencode 的 `OpenTUI` 是 sst 团队自研并对外开源（`@opentui/solid` npm 包），其内部实现与 Ink 同源（都是 cell-based retained + Yoga），但用 SolidJS 替代 React。

### 2.7 deepseek-harness：Web DOM + anser ANSI 解析

**完全不走终端 path**——`@deepseek-ai/dsh-client-ui-renderer`（`package.json:3`）是 **Browser UI renderer: React slot bindings, ctx.uiRenderer, and the assembled application root**。

`/usr/local/LsmGitOpenSource/deepseek-harness/packages/client/ui-primitives/src/ansi.ts:1-100`：
- 用 `anser` npm 包**解析 ANSI 文本为 React `<span>` 序列**。
- 每个 anser chunk 的 `{ fg, bg, decorations }` 映射到 `style: CSSProperties`。
- 8/16 色通过 `TOKEN_BY_BASIC_RGB` 表映射到 `var(--dsw-alias-*)` CSS 变量。
- `STYLE_BY_DECORATION` 处理 bold/dim/italic/underline/strikethrough。

```ts
const STYLE_BY_DECORATION: Record<string, CSSProperties | undefined> = {
  bold: { fontWeight: 700 },
  dim: { opacity: 0.7 },
  italic: { fontStyle: 'italic' },
  underline: { textDecoration: 'underline' },
  strikethrough: { textDecoration: 'line-through' },
  hidden: { visibility: 'hidden' },
}
```

Web 渲染的优势：浏览器自动处理 reflow/IME/可访问性，但**对真实 terminal emulator 适配困难**——只能在浏览器控制的 Web shell 中运行。

### 2.8 渲染模型对比小结

| 方案 | 优点 | 代价 | 适合场景 |
|------|------|------|----------|
| 全量重绘（laew） | 实现极简；< 30 行子屏成本可接受 | 大屏/动效 flicker；不能做真正的 cell diff | 简单表单、modal |
| Retained cell-based（atomcode/claudecode/opencode） | 帧间 patch 可观；支持 partial paint；可叠加 selection overlay | 需要 prev/current 双缓冲；canvas 大时单 patch 多 | 长生命周期的复杂 UI（chat、tools） |
| 行级 string diff（pi） | 无 cell grid 内存；组件输出协议简单 | 不能精确到 cell；CJK 宽度模型需手工算 | 行式工具（editor、query） |
| Web DOM（deepseek-harness） | 浏览器原生 IME/accessibility；样式系统完整 | 必须跑在 web shell；不能用 OSC/原生协议 | Web IDE、IDE 内嵌 |

---

## 3. 框架与语言选择

### 3.1 选型矩阵

| 项目 | 渲染库 | 选择理由 |
|------|--------|----------|
| atomcode | 自研 Renderer trait + task worker | Rust 性能 + 与 kernel 同进程 + 跨 macOS/Windows/Linux terminal 兼容 |
| claudecode | Ink Fork（React + Yoga） | 96 文件/13k 行自研是必要的——vadimdemedes/ink 不支持 sub-cell diff、不支持 selection overlay、不支持 DEC 2026 |
| opencode | OpenTUI（SolidJS） | 自研并开源，比 React-Ink 更细粒度响应式（Solid 编译时优化） |
| pi | 完全自研（Container/Component + LayoutBox） | 不依赖任何渲染库；可直接输出到 raw terminal |
| openclaw | Ink + pi-tui + 自研 terminal-core | 复用 pi 的 tui 包 + 自己的 ANSI/选区原语 |
| deepseek-harness | React + anser | **Web 渲染**，与 terminal 协议无关 |
| laew | 自研 Screen trait + Frame + crossterm | 复用 crossterm，避免重新实现 terminal 协议层 |

### 3.2 claudecode 为什么 fork Ink（关键决策）

`claudecode/src/ink/reconciler.ts:1-512` 是一个完整的 React Reconciler 实现。注释（`reconciler.ts:31-36`）表明这是为了支持 React DevTools + 自定义 commit 路径。

更关键的是 `screen.ts` 的所有 packed-Int32Array 优化（`screen.ts:333-365` 的 "halves memory accesses"）、`selection.ts` 的软换行累积器、`terminal.ts` 的 `BSU/ESU` 同步输出——这些都是上游 Ink 不具备的能力。

`renderer.ts:31-37` 也写明：

```ts
// Reuse Output across frames so charCache (tokenize + grapheme clustering)
// persists — most lines don't change between renders.
let output: Output | undefined
```

即 charCache 跨帧持久化，是 fork 后做的二次优化。

### 3.3 opencode OpenTUI：SolidJS 的响应式优势

OpenTUI 选择 SolidJS（不是 React）的核心收益：`createSignal` + `createStore`（`context/theme.tsx:23, 92-98`）是编译时优化的细粒度反应——React 的 vDOM diff 是粗粒度组件级别，SolidJS 在 cell-level signal 直接同步。

`/usr/local/LsmGitOpenSource/opencode/packages/tui/src/context/theme.tsx:92-98`：
```ts
const [store, setStore] = createStore<State>({
  themes: allThemes(),
  mode: "dark",
  lock: undefined,
  active: "opencode",
  ready: false,
})
```

每个 theme 切换只触发订阅了该 signal 的 cell 重新生成 patch——这是 Ink React 在大屏下做不到的。

### 3.4 pi 自研：极简主义

`/usr/local/LsmGitOpenSource/pi/packages/tui/src/tui.ts:208-245`（Container）和 `tui.ts:331-1263`（TuiBase）总共不到 1300 行实现完整 differential rendering 引擎，对比 atomcode 31k 行的 event_loop + claudecode 13k 行的 ink 包，pi 是**最轻量**的方案。

关键简化：
- **不分配 cell grid**：直接 string concat + `compositeTuiLine` 叠加 overlay
- **不持久化 prev frame**：每帧 new string[]，靠 `compositeOverlays` 做行替换
- **光标走 APC 标记**：避免引入 cell-level 状态

---

## 4. 布局引擎

### 4.1 claudecode/opencode/openclaw：Yoga Flexbox

claudecode 通过 `react-reconciler` + Yoga WASM：
- `reconciler.ts:121-129`：`applyStyles(node.yogaNode, value as Styles)` 在 commit 时同步把 styles 推到 Yoga。
- 每个 `<Box>` / `<Text>` 节点挂一个 `yogaNode`（在 `dom.ts` 创建）。
- 整个 root 的 `onComputeLayout()` 在 `resetAfterCommit`（`reconciler.ts:277`）触发：
  ```ts
  if (typeof rootNode.onComputeLayout === 'function') {
      rootNode.onComputeLayout()
  }
  ```

`reconciler.ts:280-290` 还埋了性能探针：
```ts
if (COMMIT_LOG) {
  const layoutMs = performance.now() - _t0
  if (layoutMs > 20) {
    const c = getYogaCounters()
    appendFileSync(COMMIT_LOG, `${_t0.toFixed(1)} SLOW_YOGA ...`)
  }
}
```

opencode OpenTUI 同样走 Yoga（`@opentui/core` 提供 Yoga 绑定）。
openclaw 通过 Ink 的 `<Box>` 复用 Yoga。

### 4.2 pi：自研 LayoutBox 树

`/usr/local/LsmGitOpenSource/pi/packages/tui/src/layout.ts:10-100`：

```ts
export interface LayoutBox {
    component: Component;
    rect: LayoutRect;
    clip: LayoutRect;
    children: LayoutBox[];
    parent?: LayoutBox;
    lines?: readonly string[];
    lineOffset?: number;
    scrollView?: ScrollView;
    scrollContentLines?: readonly string[];
    layer: number;
}

export interface LayoutFrame {
    root: LayoutBox;
    width: number;
    height: number;
    lines: string[];
    primaryScrollView?: ScrollView;
}
```

`renderCache`（`layout.ts:62-75`）对 `(component, width)` 缓存 `string[]`：
```ts
function renderCached(context, component, width) {
    const safeWidth = Math.max(1, Math.floor(width));
    let widths = context.renderCache.get(component);
    if (!widths) {
        widths = new Map<number, string[]>();
        context.renderCache.set(component, widths);
    }
    let lines = widths.get(safeWidth);
    if (!lines) {
        lines = component.render(safeWidth);  // ← 真正调一次
        widths.set(safeWidth, lines);
    }
    return lines;
}
```

→ **组件渲染对宽度是 memoize 的**——`invalidate()` 才能失效。`intersect()`（`layout.ts:54-60`）做父/子 clip 求交。

### 4.3 atomcode：完全无布局引擎

atomcode-tuix **没有 Flexbox**，全是绝对坐标：
- Footer 的 `paint_footer` 直接计算 (row, col)。
- Modal 的 box 在 `modals/*.rs` 各自手算宽高。
- 多 MenuKind 通过 `MenuKind::max_visible_rows`（`render/mod.rs:528-553`）静态决策高度。

原因：atomcode 是 chat 工具，UI 是**线性流式**（消息堆叠），不是 web app——Flexbox 没有收益。

### 4.4 laew 现状

laew `src/tui/` 也无布局引擎：`Rect` + `put_str(area, …)` + `put_str_centered`。`engine.rs:116-145` 的 `border_box` 手画 `╭╮╰╯` 边框。

适用边界：当前 3 个子屏（ProviderList/Form/Del）< 30 行 → 无需布局引擎。

### 4.5 布局引擎对比

| 项目 | 引擎 | 复杂度 | 适用 |
|------|------|--------|------|
| atomcode | 无 | 0 | 单向流式 chat |
| laew | 无（手画 Rect） | 0 | 表单/列表 |
| pi | 自研 LayoutBox + clip intersect | 中 | editor + overlay |
| opencode | Yoga (OpenTUI) | 高 | 多 panel 应用 |
| claudecode | Yoga (Ink Fork) | 高 | 复杂 TUI |
| openclaw | Yoga (Ink) | 高 | 复杂 TUI |
| deepseek-harness | CSS Modules + Flex/Grid | 极高 | Web UI |

---

## 5. 屏幕缓冲与对象池

### 5.1 claudecode：Packed Int32Array + 3 个共享池

这是所有 6 个项目中最精密的 cell 模型。`screen.ts:20-260` 定义 3 个池：

#### 5.1.1 CharPool（`screen.ts:21-53`）

```ts
export class CharPool {
  private strings: string[] = [' ', ''] // Index 0 = space, 1 = empty (spacer)
  private stringMap = new Map<string, number>([
    [' ', 0], ['', 1],
  ])
  private ascii: Int32Array = initCharAscii() // charCode → index, -1 = not interned

  intern(char: string): number {
    if (char.length === 1) {
      const code = char.charCodeAt(0)
      if (code < 128) {
        const cached = this.ascii[code]!
        if (cached !== -1) return cached  // ← ASCII O(1) fast path
        const index = this.strings.length
        this.strings.push(char)
        this.ascii[code] = index
        return index
      }
    }
    const existing = this.stringMap.get(char)
    if (existing !== undefined) return existing
    const index = this.strings.length
    this.strings.push(char)
    this.stringMap.set(char, index)
    return index
  }
}
```

ASCII fast-path 用 `Int32Array(128)` 直接 index lookup，避免 Map.get 开销。

#### 5.1.2 StylePool（`screen.ts:112-260`）

```ts
export class StylePool {
  private ids = new Map<string, number>()
  private styles: AnsiCode[][] = []
  private transitionCache = new Map<number, string>()
  readonly none: number

  constructor() { this.none = this.intern([]) }

  intern(styles: AnsiCode[]): number {
    const key = styles.length === 0 ? '' : styles.map(s => s.code).join('\0')
    let id = this.ids.get(key)
    if (id === undefined) {
      const rawId = this.styles.length
      this.styles.push(styles.length === 0 ? [] : styles)
      // 位 0 编码"是否在空格上有可见效果",让 renderer 跳 invisible space
      id = (rawId << 1) | (styles.length > 0 && hasVisibleSpaceEffect(styles) ? 1 : 0)
      this.ids.set(key, id)
    }
    return id
  }
}
```

`(fromId, toId)` 之间的 transition ANSI 字符串**也缓存**（`screen.ts:153-162`）：
```ts
transition(fromId: number, toId: number): string {
  if (fromId === toId) return ''
  const key = fromId * 0x100000 + toId
  let str = this.transitionCache.get(key)
  if (str === undefined) {
    str = ansiCodesToString(diffAnsiCodes(this.get(fromId), this.get(toId)))
    this.transitionCache.set(key, str)
  }
  return str
}
```

`withCurrentMatch`（`screen.ts:189-220`）实现"当前搜索匹配"高亮：base + yellow-fg + inverse + bold + underline，**同时 strip 原 bg/inverse 避免冲突**。

#### 5.1.3 HyperlinkPool（`screen.ts:55-75`）

```ts
export class HyperlinkPool {
  private strings: string[] = [''] // Index 0 = no hyperlink
  private stringMap = new Map<string, number>()
  intern(hyperlink: string | undefined): number {
    if (!hyperlink) return 0
    let id = this.stringMap.get(hyperlink)
    if (id === undefined) {
      id = this.strings.length
      this.strings.push(hyperlink)
      this.stringMap.set(hyperlink, id)
    }
    return id
  }
}
```

→ 跨 Screen 共享相同 charId/styleId/hyperlinkId，**直接复制 Int32** 即可（无需 re-intern）。

#### 5.1.4 `migrateScreenPools`（`screen.ts:554-587`）

"generational pool reset" — 当 pools 累积太大时换新池：
```ts
export function migrateScreenPools(screen, charPool, hyperlinkPool): void {
  // Re-intern chars and hyperlinks in a single pass, stride by 2
  for (let ci = 0; ci < size << 1; ci += 2) {
    const oldCharId = cells[ci]!
    cells[ci] = charPool.intern(oldCharPool.get(oldCharId))
    // ... repack hyperlinkId
  }
}
```

### 5.2 atomcode：Cell struct + 双 Vec<Vec<Cell>>

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/render/cell.rs:32-52`：

```rust
pub struct CellStyle {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub reverse: bool,
    pub faint: bool,
}
```

注释（`cell.rs:26-31`）强调**字段最小化**：
> "footer uses fg color, bold, and reverse-video. Extending this to bg / underline / italic is a future concern — adding fields is the mechanical part, but every field widens the diff equality surface and the SGR state machine's emit path, so we don't preemptively carry what we don't use."

`Cell` 含 `width: u8`（0 = continuation cell, 1 = narrow, 2 = wide CJK/emoji）（`cell.rs:68-73`）：

> "Without continuation cells, typing "你是谁" (3 wide chars = 6 cols) into a row model that tracked only char count (3 cells) would emit patches at model cols 5/6/7 while the terminal had just advanced to actual col 11 after the first 你 — the 'you3-type-shows-only-last-char' bug."

`Cell::sentinel()`（`cell.rs:126-132`）的 `ch = '\u{FFFF}'` 用于 invalidate 后让 diff 认为"每格都变"。

`Vec<Vec<Cell>>` 的 `W × H` 在 Rust 是**栈外分配 + 移动友好**——`rotate_left` 直接内存块移动（`screen.rs:208-226`）。

### 5.3 pi/openclaw/opencode/openclaw：无对象池

pi 直接用 `string[]`（`tui.ts:236`）；openclaw 通过 Ink cell grid 自动有 cell pooling；opencode 依赖 OpenTUI。

deepseek-harness 走 React `<span>` 序列，DOM 自带 GC 压力但有 VDOM 复用。

### 5.4 laew 现状

`/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/tui/engine.rs:42-57`：

```rust
#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attr: Attribute,
}
```

`Frame.cells: Vec<Cell>` 行优先（`engine.rs:62-70`）：
```rust
pub struct Frame {
    pub area: Rect,
    cells: Vec<Cell>,
}
impl Frame {
    pub fn new(area: Rect) -> Self {
        let len = (area.width as usize) * (area.height as usize);
        Self { area, cells: (0..len).map(|_| Cell::blank()).collect() }
    }
}
```

**无 prev frame**——每次子屏重入都是全新 Frame → `present()` 全量 `Clear(All)`。

---

## 6. Diff 算法与 ANSI 序列化

### 6.1 atomcode 双策略：`serialize_patches` vs `serialize_frames_tight`

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/render/cell.rs:374-589`：

#### 6.1.1 标准 per-cell CUP（`serialize_patches`，行 426-514）

```rust
pub fn serialize_patches(patches: &[Patch]) -> Vec<u8> {
    let mut out = Vec::with_capacity(patches.len() * 8);
    let mut current_style: Option<CellStyle> = None;
    let mut expected_cursor: Option<(u16, u16)> = None;
    let mut emitted_any_sgr = false;

    for patch in patches {
        // Continuation cell: skip emit
        if patch.cell.width == 0 { continue; }

        if expected_cursor != Some((patch.row, patch.col)) {
            let _ = write!(out, "\x1b[{};{}H", patch.row, patch.col);
            expected_cursor = Some((patch.row, patch.col));
        }

        if current_style.as_ref() != Some(&patch.cell.style) {
            emit_sgr_transition(&mut out, current_style.as_ref(), &patch.cell.style);
            current_style = Some(patch.cell.style.clone());
        }

        let mut buf = [0u8; 4];
        out.extend_from_slice(patch.cell.ch.encode_utf8(&mut buf).as_bytes());

        // 只对 ASCII 预测 cursor 推进;非 ASCII 强制下一次 CUP
        if (patch.cell.ch as u32) < 0x80 {
            if let Some((r, c)) = expected_cursor {
                expected_cursor = Some((r, c + patch.cell.width as u16));
            }
        } else {
            expected_cursor = None;
        }
    }

    if emitted_any_sgr {
        out.extend_from_slice(b"\x1b[0m");
    }
    out
}
```

`emit_sgr_transition`（`cell.rs:646-714`）的 reset+reapply vs additive 路径切换：
> "If any attribute is being turned OFF — if so, cheapest path is reset everything and reapply the ON set."

注释（`cell.rs:463-500`）解释了为什么非 ASCII 要 force CUP——**East Asian Ambiguous** 字符宽度模型与 terminal 实际宽度可能错位：
> "predicted 2 cols, but some Windows fonts render at 1 col. When prediction is off by N, the next patch's expected_cursor comparison thinks no CUP is needed but the terminal cursor is actually N cols away from where the model says."

#### 6.1.2 JediTerm tight repaint（`serialize_frames_tight`，行 543-589）

为 IntelliJ 平台的 JediTerm 终端专门做了**per-row 一条完整 run**：

```rust
pub fn serialize_frames_tight(prev: &[Vec<Cell>], next: &[Vec<Cell>]) -> Vec<u8> {
    for r in 0..max_rows {
        // CUP to (row, col 1) then EL — erase the whole physical line
        let _ = write!(out, "\x1b[{};1H\x1b[K", r + 1);
        // 找到 last non-blank cell,从那里开始流式输出
        if let Some(last) = n.iter().rposition(|c| c != &blank) {
            let mut current_style: Option<CellStyle> = None;
            for cell in &n[..=last] {
                if cell.width == 0 { continue; }
                if current_style.as_ref() != Some(&cell.style) {
                    emit_sgr_transition(&mut out, current_style.as_ref(), &cell.style);
                    current_style = Some(cell.style.clone());
                }
                let mut buf = [0u8; 4];
                out.extend_from_slice(cell.ch.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out
}
```

注释（`cell.rs:516-542`）：
> "JediTerm (IntelliJ-terminal) repaint: serialise (prev → next) as a per-changed-row tight stream, instead of the per-cell-CUP patch stream. JediTerm's GRID agrees with our width model (CJK = 2 cells), so we can safely let the terminal advance its own cursor across a contiguous run."

EnvView（`terminal.rs:40-52`）专门有 `TERMINAL_EMULATOR == "JetBrains-JediTerm"` 检测：
```rust
pub terminal_emulator: Option<String>,
/// `ATOMCODE_JEDITERM` manual override for the JediTerm render quirk
pub force_jediterm: Option<bool>,
```

### 6.2 claudecode：log-update + diff via `@alcalzone/ansi-tokenize`

`screen.ts:1-5`：
```ts
import { type AnsiCode, ansiCodesToString, diffAnsiCodes } from '@alcalzone/ansi-tokenize'
```

`log-update.ts:773 行`实现 `eraseLines` + `clearTerminal` 序列。
`render-node-to-output.ts:1462 行`把 React 树 flatten 成 line-based output。

`frame.ts:124 行` 定义 `Diff` 类型，`terminal.ts:190-248` 的 `writeDiffToTerminal`：

```ts
export function writeDiffToTerminal(terminal, diff, skipSyncMarkers = false): void {
  if (diff.length === 0) return

  const useSync = !skipSyncMarkers
  let buffer = useSync ? BSU : ''   // ← BSU = "\x1b[?2026h"

  for (const patch of diff) {
    switch (patch.type) {
      case 'stdout': buffer += patch.content; break
      case 'clear': buffer += eraseLines(patch.count); break
      // ... cursor / hyperlink / styleStr
    }
  }

  if (useSync) buffer += ESU         // ← ESU = "\x1b[?2026l"
  terminal.stdout.write(buffer)
}
```

### 6.3 pi：行级 string diff（无 cell）

`/usr/local/LsmGitOpenSource/pi/packages/tui/src/tui-main-screen.ts` 是行 diff 主屏（654 行）。

pi 的 diff 算法在 `tui-main-screen.ts` 内：prev `string[]` → current `string[]`，逐行 `===` 比较，差异行用 `\x1b[2K\x1b[H` + `\r` 重写。比 cell-based 简单但精度低（同一行内变化会重写整行）。

### 6.4 关键差异

| 维度 | atomcode | claudecode | pi | laew |
|------|----------|------------|-----|------|
| Diff 粒度 | Cell | Cell (packed Int32) | Line string | None (full clear) |
| 续格 CJK 处理 | `Cell::continuation` width=0 | `CellWidth.SpacerTail` | `visibleWidth` 算宽 | 简化字符数（`s.chars().count()`）|
| East Asian Ambiguous 兜底 | 非 ASCII 强制 CUP | 通过 `stringWidth.ts:222` 工具 | 内部按 Unicode width | 未处理（中文可能被错位） |
| 性能 | microsecond 级 | microsecond 级（PackedInt32 → 2 loads） | millisecond 级 | millisecond 级（整屏） |
| 兼容性 fix | JediTerm tight repaint path | ink 自带 fallback | 浏览器色彩 profile auto | 无 |

---

## 7. 异步渲染

### 7.1 atomcode：`tuix-render` 专用 worker thread

这是 6 个项目里**唯一**把渲染 I/O 彻底移到独立 OS 线程的。

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/render/worker.rs:1-21`：

> "Mac Terminal.app takes 30-60ms to process a full footer ANSI payload. When the event loop calls `renderer.render()` directly, that 30-60ms blocks the select! loop, which means:
>   - the spinner tick task can't deliver (drops),
>   - the next keystroke can't be read,
>   - agent events queue up behind the render."

`TaskRenderer::new_inner`（`worker.rs:220-240`）：
```rust
let (cmd_tx, cmd_rx) = mpsc::channel::<RenderCmd>();
let flush_pending = Arc::new(AtomicBool::new(false));
let worker = thread::Builder::new()
    .name("tuix-render".to_string())
    .spawn(move || run_worker(inner, cmd_rx, worker_flag, worker_interactions))
    .expect("spawn render worker thread");
```

`run_worker`（`worker.rs:437-666`）的事件循环：
- `recv_timeout` 配合 `RESIZE_REFLOW_DEBOUNCE = 75ms`（行 444）做 resize 防抖。
- `flush_pending` AtomicBool（`worker.rs:199, 478-492`）合并冗余 `FlushDeferred`：
  > "Without this, when the worker's terminal write blocks — classically the Windows console pausing output in QuickEdit/mark-selection mode — the ~200/sec heartbeat piles unbounded FlushDeferreds into the channel until allocation fails and the panic = "abort" build fast-fails."
- ACK op（`worker.rs:615-647`）用 oneshot channel 同步等待生命周期命令（`Reset`, `ClearScreen`, `Shutdown`）。
- `Drop` 兜底（`worker.rs:425-435`）保证 worker 线程 join。

### 7.2 pi：主线程 + 16ms 节流

`/usr/local/LsmGitOpenSource/pi/packages/tui/src/tui.ts:343, 764-824`：

```ts
private static readonly MIN_RENDER_INTERVAL_MS = 16;

requestRender(force = false): void {
    if (force) { this.resetRenderState(); this.requestImmediateRender(); return; }
    if (this.renderRequested) return;
    this.renderRequested = true;
    process.nextTick(() => this.scheduleRender());
}

private scheduleRender(): void {
    if (this.stopped || this.renderTimer || !this.renderRequested) return;
    const elapsed = performance.now() - this.lastRenderAt;
    const delay = Math.max(0, TuiBase.MIN_RENDER_INTERVAL_MS - elapsed);
    this.renderTimer = setTimeout(() => {
        this.renderTimer = undefined;
        if (this.stopped || !this.renderRequested) return;
        this.renderRequested = false;
        this.lastRenderAt = performance.now();
        this.doRender();
        if (this.renderRequested) this.scheduleRender();   // ← 连续帧累积
    }, delay);
}
```

`requestImmediateRender`（`tui.ts:783-798`）绕过节流用于键盘输入：
```ts
// Keyboard input is latency-sensitive. Avoid the throttled timer path,
// where even setTimeout(0) can take a full 16 ms tick on Windows.
this.requestImmediateRender();
```

### 7.3 openclaw：coalesced-refresh

`/usr/local/LsmGitOpenSource/openclaw/src/tui/coalesced-refresh.ts`（37 行）——类似 pi 的 16ms 节流：

```ts
// 文件名暗示:coalesced(合并) + refresh(刷新)
```

测试文件 `coalesced-refresh.test.ts`（隐含）覆盖合并逻辑。

### 7.4 opencode：OpenTUI frame loop

`createCliRenderer({ targetFps: 60, ... })`（`app.tsx:197`）由 OpenTUI 内部 SolidJS 调度控制。

### 7.5 claudecode Ink：React commit + log-update 节流

`reconciler.ts:241-315` 的 `prepareForCommit` + `resetAfterCommit` 是 React Reconciler 的标准生命周期。log-update（`log-update.ts:773 行`）按行 erase。

### 7.6 laew：同步（阻塞）

`present()`（`engine.rs:213-243`）在主线程同步写，30 行子屏约 1-2ms——子屏交互频率低（按 Esc/Enter 才重绘），不是问题。

但若 laew 未来要做**长生命周期流式主屏**，应直接借鉴 atomcode 的 worker thread 方案。

---

## 8. 鼠标协议

### 8.1 自动检测矩阵（atomcode）

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/terminal.rs:147-216`（TerminalCaps）：

| Mode | 说明 | atomcode 支持 |
|------|------|---------------|
| DECSET 1000 | X11 mouse（legacy, 5 button） | YES |
| DECSET 1002 | Cell-motion drag（按下时报告所有 cell） | YES（pointer_select.rs） |
| DECSET 1003 | All motion（任何鼠标移动都报告） | 可选 |
| DECSET 1006 | SGR encoding（>223,>223,>223 格式） | YES（推荐） |
| DECSET 1015 | Urxvt encoding | 通常 disable |

`pointer_select.rs`（`crates/atomcode-tuix/src/event_loop/pointer_select.rs:339 行`）实现了：
- 单/双/三连击分类（`MULTI_CLICK_WINDOW = 400ms`）
- WORD 边界识别（`is_cjk` 行 77-86，CJK 是单独 class）
- LINE 软换行跨 run（`line_run_span` 行 144-161）

### 8.2 claudecode：`alt press` 探测 macOS 行为

`screen.ts:62`：
```ts
/** True if the mouse-down that started this selection had the alt
 *  modifier set (SGR button bit 0x08). On macOS xterm.js this is a
 *  signal that VS Code's macOptionClickForcesSelection is OFF — if it
 *  were on, xterm.js would have consumed the event for native selection
 *  and we'd never receive it. */
lastPressHadAlt: boolean
```

### 8.3 opencode：可配置 useMouse

`/usr/local/LsmGitOpenSource/opencode/packages/tui/src/app.tsx:202`：
```ts
useMouse: !Flag.OPENCODE_DISABLE_MOUSE && input.config.mouse,
```

→ 默认开，但 TUI config 可关闭。

### 8.4 pi：默认 SGR + Ink 内置

`/usr/local/LsmGitOpenSource/pi/packages/tui/src/terminal.ts` 实现 `Terminal` interface（565 行），通过 `node:readline` + 私有 stdin buffer (`stdin-buffer.ts`) 解析 SGR mouse。

### 8.5 laew：crossterm 默认

crossterm 的 `EnableMouseCapture` 默认走 SGR 1006；cross-platform 一致。

---

## 9. 键盘协议

### 9.1 Kitty CSI-u 三方对比

| 项目 | 检测方式 | 启用方式 |
|------|----------|----------|
| atomcode | `KITTY_WINDOW_ID` / `WEZTERM_VERSION` / `ALACRITTY_SOCKET` | `is_kitty_keyboard = force_kitty_keyboard \|\| known_kitty` |
| claudecode | `EXTENDED_KEYS_TERMINALS` 白名单（`terminal.ts:156-163`） | 显式检查 |
| pi | `DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS = 7` + 协商 query `CSI >7u` | `setKittyProtocolActive` 标志 |
| opencode | `useKittyKeyboard: {}`（OpenTUI 启用） | 直接开 |
| openclaw | Ink + pi-tui | 透传 |
| laew | crossterm 默认（无 Kitty 增强） | 不支持 |

### 9.2 claudecode 白名单（`terminal.ts:155-163`）

```ts
const EXTENDED_KEYS_TERMINALS = [
  'iTerm.app', 'kitty', 'WezTerm', 'ghostty', 'tmux', 'windows-terminal',
]

export function supportsExtendedKeys(): boolean {
  return EXTENDED_KEYS_TERMINALS.includes(env.terminal ?? '')
}
```

注释（`terminal.ts:148-156`）解释了**为什么不全开**：
> "We previously enabled unconditionally (#23350), assuming terminals silently ignore unknown CSI — but some terminals honor the enable and emit codepoints our input parser doesn't handle (notably over SSH and in xterm.js-based terminals like VS Code)."

→ SSH 透传 + VS Code xterm.js 实际**会乱码**，必须白名单。

### 9.3 pi 的 7-flag 协商（`terminal.ts:14-17`）

```ts
const DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS = 7
const KITTY_KEYBOARD_PROTOCOL_QUERY = `\x1b[>${DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS}u\x1b[?u\x1b[c`
```

flags 7 = `disambiguate_escape_codes | report_alternate_keys | report_all_keys_as_escape_codes` 的组合。

`/usr/local/LsmGitOpenSource/pi/packages/tui/src/keys.ts:1401 行` 完整实现 Kitty key parser（含 `kittyProtocolActive` 状态、release events、`wantsKeyRelease` 组件 opt-in）。

### 9.4 atomcode 渐进 enable（`terminal.rs:284-346`）

```rust
let known_kitty_keyboard = is_non_empty(&env.kitty_window_id)
    || is_non_empty(&env.wezterm_version)
    || is_non_empty(&env.alacritty_socket);

let kitty_keyboard = env.force_kitty_keyboard.unwrap_or(known_kitty_keyboard) && !jediterm;
```

`should_enable_kitty_keyboard()`（`terminal.rs:958-1009`）的测试矩阵：
- JediTerm TTY **不**开（`!crate::should_enable_kitty_keyboard(&jt)`，行 977）
- `force_kitty_keyboard: Some(true)` 强制开（行 1024）
- `force_kitty_keyboard: Some(false)` 强制关（行 1037）
- SSH 检测到时不盲信 env vars

### 9.5 laew 改进建议

`docs/Agent架构对比与参考.md` 暂未提及键盘协议。当前依赖 crossterm 的 Press/Release 区分（`engine.rs:248-253`）。

P1 路线图：
1. 加 `force_kitty_keyboard` env override
2. 检测 `KITTY_WINDOW_ID` 自动开
3. `MultiClick`-style 修饰键解析在 `completion.rs` 已有先例

---

## 10. 同步输出

### 10.1 atomcode：DECSET 2026 + 外层抑制

`render/mod.rs:354-367`：
```rust
/// Open a single DECSET 2026 synchronized-output envelope spanning the
/// burst of operations up to the matching `end_sync()`. Used by the
/// `/resume` replay so the screen wipe + full-transcript re-emit paint
/// as ONE atomic update on capable hosts instead of visibly blanking
/// and re-scrolling (the flicker).
fn begin_sync(&mut self) {}
fn end_sync(&mut self) {}
```

`worker.rs:594-598` 透传到 inner renderer。
`render_diff`（`screen.rs:286-291`）跳过 per-frame envelope 当 `sync_suppressed`：
```rust
// Skip the per-frame BSU when an outer synchronized batch owns the envelope
if !self.sync_suppressed {
    out.extend_from_slice(b"\x1b[?2026h");
}
```

### 10.2 claudecode：BSU/ESU 包裹

`terminal.ts:190-248` 的 `writeDiffToTerminal` 默认开 `BSU` = `"\x1b[?2026h"` 头、`ESU` = `"\x1b[?2026l"` 尾；tmux 跳过（`terminal.ts:71-74`）：
> "tmux parses and proxies every byte but doesn't implement DEC 2026. BSU/ESU pass through to the outer terminal but tmux has already broken atomicity by chunking."

### 10.3 pi：无（依赖 raw terminal 行为）

pi 不发 `BSU/ESU`——纯靠 `setCursor` + 行 `\r` 重写。如果 terminal 卡顿会有 flicker。

### 10.4 同步输出对比

| 项目 | 启用 | 跳过条件 |
|------|------|----------|
| atomcode | `?2026h/l` 包每帧 | 外层 sync_suppressed |
| claudecode | BSU/ESU 包每帧 | tmux / `skipSyncMarkers=true` |
| opencode | OpenTUI 内置 | （未读源码） |
| openclaw | Ink 内置 | （未读源码） |
| pi | 无 | — |
| deepseek-harness | 不适用（DOM） | — |
| laew | 无 | — |

laew 子屏全量重绘不需要 sync output（不是 partial paint）；主屏若未来走 incremental 渲染，应加 `?2026h/l` 包裹。

---

## 11. 对象池与 GC 优化

### 11.1 claudecode 三池 + CharCache

`renderer.ts:36-37` 注释：
> "Reuse Output across frames so charCache (tokenize + grapheme clustering) persists — most lines don't change between renders."

`output.ts:797 行` 实现 `Output` 类：
- `charCache: Map<string, GraphemeCluster[]>`——同一个 string 只 grapheme-cluster 一次。
- 缓存命中后直接复用，避免每次重新按 grapheme 拆。

跨帧 **Output 复用** + 跨屏 **3 个 Pool** + **transitionCache**（StylePool 内的 `Map<number, string>`）——总共 4 层 memoization。

### 11.2 atomcode：无池但有显式预分配

`cell.rs:431`：
```rust
let mut out = Vec::with_capacity(patches.len() * 8);
```

`worker.rs:225` 的 channel 与 AtomicBool 避免频繁分配。

Rust 的所有权模型让"对象池"意义较小——`Cell` 是栈分配 struct，`Vec<Vec<Cell>>` 通过 `rotate_left` 复用内存。

### 11.3 pi：`renderCache: Map<Component, Map<number, string[]>>`

`layout.ts:49, 62-75`：
```ts
renderCache: Map<Component, Map<number, string[]>>;
```

按 (component, width) 二维缓存 `string[]` 输出，避免组件重渲染。

### 11.4 laew：无任何缓存

`engine.rs:62-70` 的 Frame 每次 `new()` 全 alloc。子屏 30 行成本可忽略；若要做流式主屏应借鉴 pi 的 `renderCache`。

---

## 12. 子屏 Modal

### 12.1 laew：`engine::enter_alt/leave_alt` + Screen trait

`/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/tui/engine.rs:198-210`：

```rust
pub fn enter_alt() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    Ok(())
}

pub fn leave_alt() -> io::Result<()> {
    execute!(io::stdout(), Show, LeaveAlternateScreen)?;
    let _ = terminal::disable_raw_mode();
    Ok(())
}
```

`mod.rs:360-374` 路由 `/provider *` 系列：
```rust
use crate::tui::engine::{present, read_key, Frame, Outcome, Rect};
let mut frame = Frame::new(area);
top.render(&mut frame);
present(&frame).map_err(anyhow::Error::from)?;
```

### 12.2 claudecode：`<AlternateScreen>` 组件

`/usr/local/LsmGitOpenSource/claudecode/src/ink/components/AlternateScreen.tsx` 把子树渲染到 alternate screen buffer（cell grid 独立），退出时整个 cell grid 丢。

### 12.3 atomcode：90+ modal.rs 文件

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/modals/`：

```
modals/
├── mod.rs              # modal 框架 + ModalOverlayClear
├── config_panel.rs
├── diff_viewer.rs
├── dir_picker.rs
├── file_viewer.rs
├── language_picker.rs
├── model_picker.rs
├── onboarding_wizard.rs
├── password.rs
├── plugin_manager.rs
├── provider_panel.rs
├── proxy_picker.rs
├── qr.rs
├── rewind.rs
├── session_picker.rs
└── usage.rs
```

每个 modal 都是独立的 `Renderer` 实现，通过 `MenuPayload`（`render/mod.rs:557-564`）+ `MenuKind`（`render/mod.rs:489-522`，10 种 kind）复用底层渲染。

`menu_kind::max_visible_rows`（`render/mod.rs:528-553`）按屏幕高度 + item count 计算可见行数：
```rust
pub fn max_visible_rows(&self, screen_height: usize, item_count: usize) -> usize {
    match self {
        MenuKind::SlashCommand | MenuKind::AtMention => item_count.min(4),
        MenuKind::Skill | MenuKind::Action | MenuKind::TwoColumn { .. } => {
            item_count.min((screen_height / 2).max(4))
        }
        MenuKind::Plugin | MenuKind::SessionList => {
            let plugin_count = item_count.saturating_sub(3);
            let max_plugins = (screen_height / 4).max(2);
            let visible_plugins = plugin_count.min(max_plugins);
            3 + visible_plugins * 2
        }
        // ...
    }
}
```

### 12.4 pi：overlay stack + composite

`/usr/local/LsmGitOpenSource/pi/packages/tui/src/tui.ts:549-642`（`showOverlay`）：

```ts
showOverlay(component: Component, options?: OverlayOptions): OverlayHandle {
    const entry: OverlayStackEntry = {
        component,
        ...(options === undefined ? {} : { options }),
        preFocus: this.focusedComponent,
        hidden: false,
        focusOrder: ++this.focusOrderCounter,
    };
    this.overlayStack.push(entry);
    // Only focus if overlay is actually visible
    if (!options?.nonCapturing && this.isOverlayVisible(entry)) {
        this.setFocus(component);
    }
    this.terminal.hideCursor();
    this.requestRender();

    return {
        hide: () => { ... },
        setHidden: (hidden) => { ... },
        focus: () => { ... },
        unfocus: (unfocusOptions) => { ... },
        isFocused: () => this.focusedComponent === component,
    };
}
```

→ 完整 focus management，`preFocus` + `focusOrder` 决定 z-order。
`compositeOverlays`（`tui.ts:1099-1158`）把 overlay 行**叠加**到 base content 的 `(row, col)`：

```ts
private compositeLineAt(baseLine, overlayLine, startCol, overlayWidth, totalWidth): string {
    return compositeTuiLine(baseLine, overlayLine, startCol, overlayWidth, totalWidth);
}
```

`compositeTuiLine`（`tui.ts:253-282`）做精确列替换：
```ts
const result =
    base.before +
    " ".repeat(beforePad) +
    SEGMENT_RESET +
    overlay.text +
    " ".repeat(overlayPad) +
    SEGMENT_RESET +
    base.after +
    " ".repeat(afterPad);
```

### 12.5 opencode：route-based navigation + DialogProvider

`/usr/local/LsmGitOpenSource/opencode/packages/tui/src/app.tsx:300+` 用 `<Switch><Match>` SolidJS 模式：

```tsx
<Switch>
  <Match when={route.data.type === "home"}>
    <Home />
  </Match>
  <Match when={route.data.type === "session"}>
    <Session />
  </Match>
  // ...
</Switch>
```

Dialog 是 z-index 在 route 上层的 `<DialogProvider>` 包裹（`context/dialog.tsx`）——通过 SolidJS reactive context 显隐。

### 12.6 子屏策略对比

| 项目 | 实现 | 焦点管理 | z-order |
|------|------|----------|---------|
| laew | Screen trait + 全量 present | 无（子屏独占） | 单一 |
| atomcode | 90+ modals + UiLine::ModalOverlayClear | Footer + body 互动 | 单层 modal |
| pi | overlay stack + compositeOverlays | preFocus + focusOrder | 多层栈 |
| opencode | SolidJS `<Switch>` + `<DialogProvider>` | SolidJS reactive | 单层 |
| claudecode | Ink `<AlternateScreen>` 组件 | React tree | 单层 |
| openclaw | Ink + pi-tui 复用 | pi-tui focusOrder | 多层栈 |

---

## 13. 主题与国际化

### 13.1 主题策略矩阵

| 项目 | 主题来源 | light/dark 切换 | ANSI 调色板 |
|------|----------|-----------------|-------------|
| atomcode | 16-color + 256-color（按 `TerminalCaps` 决策） | `is_light_for_render()`（`highlight/theme.rs`） | `Palette::BRAND/MODE/SHELL_*` |
| claudecode | React `useTheme` + 配置文件 | 由 Ink 主题系统处理 | 由 React props 决定 |
| opencode | `theme.tsx` + `@opentui/core` JSON | `mode: "dark" \| "light"` + `lock` | JSON token 解析 |
| pi | 自定义 `theme.ts` | ENV: `PI_*` | 由 `terminal-colors.ts` 检测（OSC 11） |
| openclaw | `LOBSTER_PALETTE` 9 tokens + chalk | `NO_COLOR` / `FORCE_COLOR` | chalk |
| deepseek-harness | CSS variables (`--dsw-alias-*`) | 浏览器 `prefers-color-scheme` | CSS 变量 |
| laew | `tui::theme` 常量 | 无 | crossterm Color |

### 13.2 atomcode 调色板细节

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/render/theme.rs:7-141`：

```rust
/// Basic 16-color palette — SGR 30-37/90-97 only, no truecolor RGB.
///
/// **Why 16 colors:** truecolor RGB renders the same pixel regardless of
/// terminal theme. On Mac Terminal.app's default "Basic" (light) profile,
/// our old lavender/mint/grays landed on a light background and all but
/// disappeared. The 16-color SGR palette (30-37, 90-97) is interpreted by
/// the terminal's own theme engine.
```

**关键洞察**：**主题感知**通过 SGR 30-37/90-97 的**调色板索引**实现——terminal 自己把 `\x1b[33m` 映射到当前 theme 的暗黄/亮黄。

`muted_for_current_theme()`（`theme.rs:151-157`）：
```rust
pub fn muted_for_current_theme() -> Color {
    if md_theme::is_light_for_render() {
        Palette::MUTED_LIGHT  // SGR 90
    } else {
        Palette::MUTED_DARK   // SGR 37
    }
}
```

`selection_bg_for_current_theme()`（`theme.rs:225-231`）用 `AnsiValue(24)` (deep blue) / `AnsiValue(153)` (pale blue) 给文本选择背景。

### 13.3 国际化矩阵

| 项目 | i18n 范围 | 方案 |
|------|-----------|------|
| atomcode | 完整 UI | `Msg<'a>` enum + `t(Msg::…)` lookup，zh_CN / en 双语 |
| claudecode | 命令 / 错误信息 | React props 国际化 |
| opencode | CLI 输出 | 简单 i18n context（locale/） |
| pi | 部分 | 命令支持多语言（通过 chalk） |
| openclaw | 部分 | 通过 chalk + locale |
| deepseek-harness | 完整（i18n context） | `locale/` 包 + i18next 风格 |
| laew | 中文硬编码 | 无 |

### 13.4 atomcode i18n 实现

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/i18n/mod.rs`（2 行）：
```rust
// Driver-local import surface for the config-owned localization tables.
pub use atomcode_config::i18n::*;
```

实际 enum 在 `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-config/src/i18n/messages.rs:1-3`：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg<'a> {
    WelcomeBannerLine1,
    WelcomeBannerLine2,
    WelcomeOptionCodingPlan,
    // ... 100+ 变体
}
```

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-config/src/i18n/en.rs` 与 `zh_cn.rs` 分别实现 lookup。`t(Msg::…)` 在 crate 入口定义。

### 13.5 deepseek-harness i18n

`/usr/local/LsmGitOpenSource/deepseek-harness/packages/client/locale/` 整个子包处理 i18n。
`README.i18n.yaml` 描述 i18n 协议。

---

## 14. 进度条与 Spinner

### 14.1 atomcode：caps 驱动的 unicode/ascii 双模式

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/terminal.rs:167-173`：

```rust
/// Render decorative Unicode glyphs (`❯`, `◆`, box-drawing corners).
/// Off → use ASCII fallbacks (`>`, `*`, `+`) so minimal terminals
/// (Windows legacy console, Docker/CI, POSIX locale without a full
/// font) don't show `□` tofu.
pub unicode_symbols: bool,
```

ASCII fallback 在 `render/mod.rs:1013-1017`：
> "Wraps the spinner glyph: `◐` (unicode) ↔ `\|/-\` (ASCII fallback), the same `unicode_symbols` gate the ellipse `…`↔`...` uses."

### 14.2 claudecode：`register-spinner.ts` 注册自定义帧

`/usr/local/LsmGitOpenSource/opencode/packages/tui/src/component/register-spinner.ts`（opencode 用 OpenTUI 机制）——通过 OpenTUI 框架注册 spinner 帧序列。

### 14.3 OSC 9;4 进度条（claudecode）

`/usr/local/LsmGitOpenSource/claudecode/src/ink/terminal.ts:25-64`：

```ts
export function isProgressReportingAvailable(): boolean {
  if (!process.stdout.isTTY) return false
  if (process.env.WT_SESSION) return false  // Windows Terminal 不用 9;4
  if (process.env.ConEmuANSI || process.env.ConEmuPID || process.env.ConEmuTask) return true

  const version = coerce(process.env.TERM_PROGRAM_VERSION)
  if (!version) return false

  if (process.env.TERM_PROGRAM === 'ghostty') return gte(version.version, '1.2.0')
  if (process.env.TERM_PROGRAM === 'iTerm.app') return gte(version.version, '3.6.6')

  return false
}
```

支持列表：ConEmu (all)、Ghostty 1.2.0+、iTerm2 3.6.6+。

### 14.4 pi：`progress-line.ts` in openclaw terminal-core

`/usr/local/LsmGitOpenSource/openclaw/packages/terminal-core/src/progress-line.ts` + `osc-progress.ts` —— 独立的进度条原语（清屏 + OSC 9;4 + 重渲染）。

### 14.5 laew：无 spinner

主屏只有 `crossterm::cursor::Show/Hide`，无显式 spinner。流式 LLM 输出时仅靠"流式文本逐 token 出现"提供进度反馈。

P1 路线图：加 `◐|/-\` spinner（atomcode 风格），仅在 idle state 显示。

---

## 15. 选区与复制

### 15.1 atomcode：`pointer_select.rs` + `interaction.rs`

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/event_loop/pointer_select.rs:339 行`：

```rust
pub const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(400);

pub fn next_click(prev: Option<ClickRecord>, now: Instant, row: u16, col: u16, window: Duration) -> ClickRecord {
    let count = match prev {
        Some(p) if p.row == row && p.col == col && now.saturating_duration_since(p.at) <= window => {
            if p.count >= 3 { 1 } else { p.count + 1 }
        }
        _ => 1,
    };
    // ...
}
```

`word_bounds`（`pointer_select.rs:104-136`）CJK-aware：
```rust
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF        // Hiragana + Katakana
        | 0x3400..=0x4DBF      // CJK Ext-A
        | 0x4E00..=0x9FFF      // CJK Unified Ideographs
        | 0xAC00..=0xD7AF      // Hangul syllables
        | 0xF900..=0xFAFF      // CJK Compatibility Ideographs
        | 0x20000..=0x2FA1F    // CJK Ext-B..F
    )
}
```

`line_run_span`（行 144-161）跨 soft-wrap run 识别逻辑行。

### 15.2 claudecode：`selection.ts` 复杂状态机

`/usr/local/LsmGitOpenSource/claudecode/src/ink/selection.ts:917 行`：

```ts
export type SelectionState = {
  anchor: Point | null
  focus: Point | null
  isDragging: boolean
  anchorSpan: { lo: Point; hi: Point; kind: 'word' | 'line' } | null
  scrolledOffAbove: string[]      // 滚出可见区的累积
  scrolledOffAboveSW: boolean[]   // 平行 soft-wrap 标记
  scrolledOffBelow: string[]
  scrolledOffBelowSW: boolean[]
  virtualAnchorRow?: number       // PgDn 截断后的虚拟位置
  virtualFocusRow?: number
  lastPressHadAlt: boolean        // macOS alt press detection
}
```

`getSelectedText`（`selection.ts` 后续）必须用 `scrolledOffAbove/Below` + soft-wrap 重组逻辑行（注释：`screen.ts:403-413` softWrap 字段）。

**iTerm2 默认 word boundary**（`selection.ts:141-142`）：
```ts
const WORD_CHAR = /[\p{L}\p{N}_/.\-+~\\]/u
// iTerm2 default: /-+\~_.
```

→ 双击 `/usr/bin/bash` 选中整个路径——这是 macOS 用户肌肉记忆。

### 15.3 选区对比

| 项目 | 选区方案 | CJK word | 复制集成 |
|------|----------|----------|----------|
| atomcode | `pointer_select.rs` + `CopyRun[]` | `is_cjk` | OSC 52 clipboard |
| claudecode | `selection.ts` + 软换行累积 | `WORD_CHAR` 正则 | anser + OSC 52 |
| opencode | OpenTUI 内置 | — | — |
| pi | 行内 extract via `extractSegments` | 由组件决定 | — |
| openclaw | 复用 pi-tui | — | — |
| laew | 无（终端原生 select） | 终端原生 | 终端原生 |

### 15.4 laew 改进建议

主屏若做流式渲染，**选区是必须**的——用户拖选一段代码再粘贴是高频操作。
- 借鉴 atomcode 的 `pointer_select.rs` 思路：CJK word + soft-wrap。
- 借鉴 claudecode 的 `scrolledOffAbove` 累积：流式输出时锚点可能滚出 viewport。
- 复制走 OSC 52（xterm/iTerm2 都支持）。

---

## 16. 流式 Markdown 渲染

### 16.1 atomcode：`markdown.rs` 3115 行手写解析器

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/markdown.rs:1-150`：

```rust
/// Line-oriented markdown renderer. Handles:
///   **bold** / *italic* / `code` (inline)
///   # / ## / ### headings
///   - / * bullet lists
///   ```fenced code blocks``` (state-tracked)
///   --- horizontal rules
/// Tables are passed through as raw text (pipes show literally).
```

`MdState`（行 17-65）跨行 state：
```rust
pub struct MdState {
    pub in_code_block: bool,
    fence_char: char,
    fence_len: usize,
    pub table_buf: Vec<String>,
    pub code_buf: Vec<String>,
    pub last_code_block_source: Option<String>,  // 用于复制原文
    pub code_block_count: usize,
    pub last_heading: Option<String>,             // 防重复标题
}
```

**关键设计**（行 33-48）：
> "Set on close-fence and unclosed-finalize paths. Callers take the value via `Option::take()` and push it to the system clipboard via arboard / OSC 52 so the user can paste the unwrapped original instead of selecting wrapped display lines (issue #699)."

→ 自动复制最近一次闭合的代码块到剪贴板——issue #699 是关键 UX 需求。

**GFM table 智能检测**（行 141-147）：
```rust
if !state.in_code_block
    && split_table_row(trimmed).len() >= 2
    && parse_list_item(line).is_none()
{
    state.table_buf.push(trimmed.to_string());
    return None;  // 延迟 emit,等分隔行
}
```

**列表项误判防御**（行 135-144）：
> "EXCLUDE list items: a bullet/number line that merely mentions a pipe (`- option A | option B`) is a LIST item, not a table row — GFM only treats it as a table cell when a table is actually established. Without this guard the broadened detection stole such items from the list path and dropped their marker."

### 16.2 claudecode：anser 输出 → React

stream token 直接拼接，anser 在 React 端解析（`ui-primitives/src/ansi.ts`）：

```ts
const OSC_SEQUENCE = /\][^]*(?:|\\)?/g
const NON_CSI_ESCAPE = /(?!\[)[ -/]*[0-~]?/g
const INERT_CONTROL = /[ ---]/g
const NEEDS_REPLAY = /\r||\[[0-?]*[ -/]*K/
```

`NEEDS_REPLAY`（行 92）检测 `\r` / `\b` / `\x1b[K`，用 `replayLine` 重放光标轨迹——处理 stream 中的进度条更新。

### 16.3 opencode：OpenTUI markdown

OpenTUI 自带 markdown 支持（`@opentui/core` 提供）。

### 16.4 pi：`markdown.ts` 按行渲染

`/usr/local/LsmGitOpenSource/pi/packages/tui/src/components/markdown.ts`（组件级别）—— `Component` 实现，按 `render(width)` 输出 `string[]`。

### 16.5 deepseek-harness：完整 remark 工具链

`/usr/local/LsmGitOpenSource/deepseek-harness/packages/client/ui-primitives/package.json:30-40`：

```
"dependencies": {
  "@shikijs/langs": "^4.3.1",
  "anser": "^2.3.5",
  "katex": "^0.16.47",
  "mdast-util-from-markdown": "^2.0.3",
  "mdast-util-gfm": "^3.1.0",
  "micromark-core-commonmark": "^2.0.3",
  "micromark-extension-gfm": "^3.0.0",
  ...
}
```

→ **micromark** + **mdast-util-gfm** + **KaTeX** 数学公式 + **Shiki** 代码块高亮。

### 16.6 流式 Markdown 方案对比

| 项目 | 解析器 | 流式策略 | 代码块高亮 | 数学 |
|------|--------|----------|------------|------|
| atomcode | 手写 3115 行 | 按行 state | 仅 2-space indent（line.rs:24-37） | 无 |
| claudecode | anser + log-update | 增量 patch | React Prism 风格 | 无 |
| opencode | OpenTUI | 增量 | 框架自带 | 框架 |
| pi | `markdown.ts` | 按组件 render | 基础 | 无 |
| deepseek-harness | micromark + mdast + remark | VDOM diff | Shiki | KaTeX |
| laew | 无（纯文本输出） | — | — | — |

### 16.7 laew 现状与建议

laew 当前**完全无 markdown 渲染**——LLM 流式输出按 token 写入 `input.rs` 的输入行。

P2 路线图：
1. 引入 `pulldown-cmark` crate（增量解析 + event stream）。
2. 为 `Renderer` trait 新增 `render_markdown_line(line: &str, state: &mut MdState)`。
3. 代码块自动复制可借鉴 atomcode `last_code_block_source` 机制。

---

## 17. laew 借鉴路线图

### 17.1 现状评估（laew `src/tui/`）

| 模块 | 行数 | 评价 |
|------|------|------|
| `engine.rs` | 277 | Screen trait + Frame + present 全量重绘。`< 30 行子屏`够用 |
| `input.rs` | 415 | 单行输入 + crossterm 原始模式 + 补全 |
| `completion.rs` | 255 | 斜杠命令补全（行内提示） |
| `form.rs` | 371 | Tab 表单状态机 |
| `theme.rs` | 59 | 仅常量，无 light/dark 切换 |
| `mod.rs` | 627 | REPL 主循环 |
| `screen/provider_list.rs` | 233 | 5 只读字段 Tab |
| `screen/provider_form.rs` | 303 | 5+1 Tab 表单 |
| `screen/provider_del.rs` | 276 | Picker + 二次确认 |
| **总计** | **~2820** | — |

**关键缺陷**：
1. **无 markdown 渲染**——LLM 输出当 plain text。
2. **无流式主屏**——只有 `input.rs` 单行 + 子屏 modal。
3. **无 spinner / progress**——idle/streaming 视觉反馈缺失。
4. **全量重绘不友好大屏**——`present()` 在 100×30 屏下 3-5ms，慢终端 30-60ms。
5. **无光标控制**——`present()` 不 park cursor，光标位置不可预测。
6. **无 ANSI 状态机**——每行 `ResetColor` 兜底，色块叠加会丢 SGR。
7. **无 DECSET 2026**——主屏若有 incremental 渲染会 flicker。

### 17.2 P0（必做）

#### 17.2.1 Cell grid 引入（借鉴 atomcode `screen.rs`）

```rust
// src/tui/engine.rs (新增)
pub struct Cell {
    pub ch: char,
    pub style: CellStyle,  // fg/bg/bold/reverse/faint
    pub width: u8,         // 0 = continuation, 1 = narrow, 2 = wide
}

pub struct Screen {
    cells: Vec<Vec<Cell>>,
    prev_cells: Vec<Vec<Cell>>,
    width: u16,
    height: u16,
    cursor: Option<(u16, u16)>,
    cursor_visible: bool,
    physical_dirty: bool,
}

impl Screen {
    pub fn render_diff(&mut self) -> Vec<u8> { ... }
    pub fn draw_row(&mut self, row: usize, col: usize, cells: &[Cell]) { ... }
    pub fn scroll_up(&mut self, bottom: usize, n: usize) { ... }
}
```

P0 任务清单：
- [ ] 把 `engine.rs:42-57` 的 `Cell` 升级为 `Cell + CellStyle`（`atomcode cell.rs:32-52`）。
- [ ] 加 `Screen` 双缓冲（`atomcode screen.rs:46-92`）。
- [ ] 加 `serialize_patches` + diff 函数（`cell.rs:394-414`）。
- [ ] `present()` 改用 `render_diff` 输出。

预期收益：30 行子屏从 3-5ms 降到 < 1ms（仅 patch diff）。

#### 17.2.2 DECSET 2026 包裹（借鉴 claudecode `terminal.ts:190-248`）

```rust
// src/tui/engine.rs (新增)
pub fn present_with_sync(frame: &Frame) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(b"\x1b[?2026h")?;  // BSU
    stdout.write_all(b"\x1b[?25l")?;    // 隐藏光标
    // ... render_diff bytes
    stdout.write_all(b"\x1b[?25h")?;    // 恢复光标
    stdout.write_all(b"\x1b[?2026l")?;  // ESU
    stdout.flush()?;
    Ok(())
}
```

P0 任务清单：
- [ ] `tmux` 环境跳过（`atomcode terminal.ts:71-74`）。
- [ ] `mod.rs:360` 的子屏入口用 `present_with_sync`。

#### 17.2.3 光标 park + 可见性

`engine.rs:213` 的 `present()` 当前不控制光标。

```rust
// 借鉴 atomcode screen.rs:173-189
pub fn set_cursor(&mut self, row: u16, col: u16) {
    let row = row.clamp(1, self.height.max(1));
    let col = col.clamp(1, self.width.max(1));
    self.cursor = Some((row, col));
}

pub fn set_cursor_visible(&mut self, visible: bool) {
    self.cursor_visible = visible;
}
```

P0 任务清单：
- [ ] `Frame` 加 `cursor: Option<(u16, u16)>` + `cursor_visible: bool`。
- [ ] `present()` 在 frame 尾部 park cursor + set visibility。

#### 17.2.4 子屏结束时不光标丢失

`engine.rs:206-209` 的 `leave_alt` 后光标丢失到主屏——`input.rs` 接管后不会自动恢复。

```rust
pub fn leave_alt() -> io::Result<()> {
    execute!(io::stdout(), Show, LeaveAlternateScreen)?;
    let _ = terminal::disable_raw_mode();
    Ok(())
}
```

P0 任务清单：
- [ ] `leave_alt` 后 `input.rs` 自动 `\x1b[?25h\x1b[<u` 重置 input cursor。

### 17.3 P1（重要）

#### 17.3.1 流式 Markdown 渲染（借鉴 atomcode `markdown.rs:1-150`）

新增 `src/tui/markdown.rs`：

```rust
pub struct MdState {
    pub in_code_block: bool,
    fence_char: char,
    fence_len: usize,
    pub table_buf: Vec<String>,
    pub code_buf: Vec<String>,
    pub last_code_block_source: Option<String>,
}

pub fn render_line(line: &str, state: &mut MdState) -> Option<String> { ... }
```

P1 任务清单：
- [ ] 抄 atomcode 的 line-oriented state machine（`markdown.rs:106-200`）。
- [ ] GFM table detection（`markdown.rs:141-147`）。
- [ ] 代码块自动复制（`markdown.rs:33-48` 的 `last_code_block_source`）。

预期收益：LLM 流式输出可识别 `# 标题` / `**bold**` / ``` ```code``` ```，并自动复制代码块到剪贴板。

#### 17.3.2 CJK-aware 宽度模型（借鉴 atomcode `cell.rs:160-193`）

`engine.rs:108-112` 的 `put_str_centered` 当前用 `s.chars().count()`：

```rust
pub fn put_str_centered(&mut self, y: u16, s: &str, fg: Color, attr: Attribute) {
    let w = s.chars().count() as u16;  // ← CJK 错位!
    let x = self.area.width.saturating_sub(w) / 2;
    // ...
}
```

P1 任务清单：
- [ ] 引入 `unicode-width` crate。
- [ ] 把 `chars().count()` 替换为 `UnicodeWidthStr::width()`。
- [ ] 借鉴 atomcode `cell.rs:60-67` 的 `Cell::continuation()` 处理 wide char。

#### 17.3.3 异步渲染 worker（借鉴 atomcode `worker.rs:1-50`）

仅当主屏做长生命周期流式渲染时需要。

P1 任务清单：
- [ ] `tuix-render` 线程 + `mpsc::channel<RenderCmd>`（`worker.rs:220-240`）。
- [ ] `flush_pending: Arc<AtomicBool>` 合并冗余 FlushDeferred（`worker.rs:199`）。
- [ ] ACK op for `Reset`/`ClearScreen`/`Shutdown`（`worker.rs:165-171`）。

#### 17.3.4 Spinner + OSC 9;4 进度条（借鉴 claudecode `terminal.ts:25-64`）

P1 任务清单：
- [ ] `terminal.rs:167-173` 的 `unicode_symbols` + ASCII fallback。
- [ ] `◐|/-\` spinner 帧序列。
- [ ] `isProgressReportingAvailable()` 检查 ConEmu/Ghostty/iTerm2。

#### 17.3.5 主题 light/dark 切换（借鉴 atomcode `theme.rs:151-231`）

当前 `theme.rs` 只导出常量：

```rust
// src/tui/theme.rs
pub const FG: Color = Color::Reset;
pub const ACCENT: Color = Color::Cyan;
pub const ERROR: Color = Color::Red;
// ...
```

P1 任务清单：
- [ ] `is_light_for_render()` 检测 `WT_SESSION` / `TERM_PROGRAM`（`terminal.rs:33-39`）。
- [ ] `muted_for_current_theme()` light/dark 双套常量（`theme.rs:151-157`）。

### 17.4 P2（增强）

#### 17.4.1 鼠标选区 + CJK word + soft-wrap

完全借鉴 atomcode `pointer_select.rs:339 行`：

- [ ] `MULTI_CLICK_WINDOW = 400ms` + `next_click` 状态机（`pointer_select.rs:35-60`）。
- [ ] `is_cjk` 范围表（`pointer_select.rs:77-86`）。
- [ ] `word_bounds` + `line_run_span`（`pointer_select.rs:104-161`）。

#### 17.4.2 Kitty CSI-u 键盘增强

借鉴 atomcode `terminal.rs:284-346`：

- [ ] `force_kitty_keyboard: Option<bool>` + env override。
- [ ] 自动检测 `KITTY_WINDOW_ID` / `WEZTERM_VERSION` / `ALACRITTY_SOCKET`。

#### 17.4.3 i18n

借鉴 atomcode `i18n/mod.rs` + `crates/atomcode-config/src/i18n/messages.rs:1-3`：

- [ ] `pub enum Msg<'a> { ... }` + `t(Msg::…)` lookup。
- [ ] `zh_CN.rs` / `en.rs` 双套。
- [ ] P0 阶段仅处理 UI 文案（welcome / error / hint）。

#### 17.4.4 JediTerm tight repaint 兼容

`terminal.rs:40-52` 的 `TERMINAL_EMULATOR == "JetBrains-JediTerm"` 检测 + `screen.rs:543-589` 的 `serialize_frames_tight`——若 laew 用户的开发环境在 IntelliJ 平台终端跑，启用以避免 CJK 间隙。

#### 17.4.5 Retained-mode TUI 主屏

如果未来 laew 要做"主屏像 chat 流式 UI"（参考 CC / atomcode 的全屏 chat），需要：
- 引入 `tuix` crate 或自己写 Screen + CellGrid（`atomcode screen.rs:46-92`）。
- 主屏布局走 `prompt + body + footer` 三段式（`atomcode UiLine::*` 49+ 变体）。
- 借鉴 atomcode `worker.rs` 把渲染 I/O 移出 event loop。

### 17.5 借鉴优先级总表

| 阶段 | 来源 | 改动量 | 收益 |
|------|------|--------|------|
| **P0** | atomcode `screen.rs` + `cell.rs` | ~600 行 Rust | 大屏从 30ms 降到 < 1ms；flicker 消失 |
| **P0** | claudecode `terminal.ts:190-248` | ~50 行 | flicker 修复 |
| **P0** | atomcode `screen.rs:170-189` | ~30 行 | 光标可预测 |
| **P1** | atomcode `markdown.rs` 3115 行 | ~800 行 | LLM 输出可读 |
| **P1** | atomcode `cell.rs:160-193` | ~100 行 | CJK 中文不错位 |
| **P1** | atomcode `worker.rs` | ~400 行 | 慢终端不卡 event loop |
| **P1** | claudecode `terminal.ts:25-64` | ~50 行 | 进度条 UI |
| **P1** | atomcode `theme.rs:151-231` | ~100 行 | light/dark 适配 |
| **P2** | atomcode `pointer_select.rs` 339 行 | ~400 行 | 鼠标选区 |
| **P2** | atomcode `terminal.rs:284-346` | ~80 行 | Kitty CSI-u |
| **P2** | atomcode `i18n` | ~500 行 | 中英双语 |
| **P2** | atomcode `cell.rs:543-589` | ~80 行 | JediTerm 兼容 |

---

## 18. 附录：行号速查

### 18.1 atomcode

| 文件 | 关键行 | 内容 |
|------|--------|------|
| `render/mod.rs` | 1-1158 | `UiLine` enum（49 变体） + `MenuKind`（10 种） + `MenuPayload` + `Renderer` trait |
| `render/cell.rs` | 1-1476 | `Cell` / `CellStyle` / `push_str_cells` / `serialize_patches` / `serialize_frames_tight` |
| `render/screen.rs` | 1-988 | `Screen` 双缓冲 + `render_diff` + DEC 2026 + cursor |
| `render/retained.rs` | 1-26012 | RetainedRenderer（主实现） |
| `render/plain.rs` | 1-1220 | PlainRenderer（管道 / 非 TTY fallback） |
| `render/worker.rs` | 1-1175 | `TaskRenderer` + worker thread |
| `render/theme.rs` | 1-481 | `Palette` (16-color) + `Role` + light/dark |
| `event_loop/mod.rs` | 1-31321 | 主事件循环 + 多 Agent 编排 |
| `event_loop/pointer_select.rs` | 1-339 | `next_click` + `word_bounds` + `line_run_span` |
| `terminal.rs` | 1-1091 | `EnvView` + `TerminalCaps` + Kitty keyboard + JediTerm |
| `markdown.rs` | 1-3115 | line-oriented markdown 解析器 |
| `highlight/mod.rs` | 1-314 | 代码块格式化（2-space indent） |
| `highlight/theme.rs` | — | md theme |
| `width.rs` | 1-1466 | `cell_char_width` / `display_width` |
| `i18n/mod.rs` | 1-2 | re-export `atomcode_config::i18n::*` |
| `crates/atomcode-config/src/i18n/messages.rs` | 1-3 | `Msg<'a>` enum |
| `crates/atomcode-config/src/i18n/en.rs` | — | 英文 lookup |
| `crates/atomcode-config/src/i18n/zh_cn.rs` | — | 中文 lookup |
| `modals/mod.rs` | 1-291 | modal framework |
| `modals/*.rs` | 17 个文件 | 90+ modal 实现 |

### 18.2 claudecode Ink Fork

| 文件 | 关键行 | 内容 |
|------|--------|------|
| `ink.tsx` | 1-1722 | Ink 入口 |
| `screen.ts` | 1-1486 | `CharPool` / `StylePool` / `HyperlinkPool` / `Cell` / `Screen` packed Int32Array |
| `selection.ts` | 1-917 | `SelectionState` + `word_boundsAt` + `getSelectedText` |
| `parse-keypress.ts` | 1-801 | 键盘解析 |
| `output.ts` | 1-797 | `Output` 类 + charCache |
| `log-update.ts` | 1-773 | log-update 增量 |
| `styles.ts` | 1-771 | `Styles` + `TextStyles` |
| `reconciler.ts` | 1-512 | React Reconciler + Yoga commit |
| `dom.ts` | 1-484 | `DOMElement` / `TextNode` |
| `terminal.ts` | 1-248 | DEC 2026 + OSC 9;4 + extended keys 白名单 |
| `render-node-to-output.ts` | 1-1462 | React 树 → line output |
| `render-border.ts` | 1-231 | 边框绘制 |
| `render-to-screen.ts` | 1-231 | node → Screen |
| `hit-test.ts` | 1-130 | 鼠标点击测试 |
| `termio.ts` | 1-100+ | 终端 I/O 原语 |
| `components/AlternateScreen.tsx` | — | alt-screen 组件 |
| `components/Box.tsx` | — | Yoga Box |
| `components/Text.tsx` | — | Text |
| `components/ScrollBox.tsx` | — | ScrollBox |

### 18.3 opencode

| 文件 | 关键行 | 内容 |
|------|--------|------|
| `app.tsx` | 1-1134 | SolidJS App + `createCliRenderer` |
| `keymap.tsx` | 1-290 | keymap provider |
| `runtime.tsx` | 1-9 | runtime context |
| `context/theme.tsx` | 1-332 | `theme()` + `DEFAULT_THEMES` |
| `context/sync.tsx` | 1-673 | 数据同步 |
| `context/clipboard.tsx` | — | clipboard |
| `context/dialog.tsx` | — | Dialog Provider |
| `component/register-spinner.ts` | — | OpenTUI spinner 注册 |
| `component/spinner.tsx` | — | Spinner 组件 |
| `component/dialog-*.tsx` | 20+ | 各种 dialog |
| `ui/spinner.ts` | — | spinner 原语 |
| `ui/border.ts` | — | border 原语 |
| `util/selection.ts` | — | 选区 |
| `util/scroll.ts` | — | scroll |
| `routes/home.tsx` | — | Home route |
| `routes/session.tsx` | — | Session route |

### 18.4 pi

| 文件 | 关键行 | 内容 |
|------|--------|------|
| `tui.ts` | 1-1263 | `TuiBase` + `Container` + overlay stack + `compositeTuiLine` |
| `layout.ts` | 1-410 | `LayoutBox` + `LayoutFrame` + `intersect` |
| `layout-node.ts` | 1-51 | `getLayoutNode` |
| `terminal.ts` | 1-565 | `Terminal` interface + Kitty 协商 |
| `terminal-colors.ts` | — | OSC 11 / DSR color scheme |
| `terminal-image.ts` | — | Kitty image protocol |
| `keys.ts` | 1-1401 | Kitty key parser |
| `stdin-buffer.ts` | — | stdin raw buffer |
| `utils.ts` | — | `visibleWidth` / `sliceByColumn` |
| `tui-main-screen.ts` | 1-654 | 主屏行级 diff |
| `tui-alt-screen.ts` | 1-1378 | alt-screen 全屏模式 |
| `alt-screen-search.ts` | 1-157 | alt-screen 内搜索 |
| `editor-component.ts` | — | editor |
| `components/markdown.ts` | — | markdown component |
| `components/scroll-view.ts` | — | scroll view |
| `components/box.ts` | — | Box |
| `components/h-stack.ts` | — | horizontal stack |
| `components/v-stack.ts` | — | vertical stack |
| `components/input.ts` | — | Input |
| `components/loader.ts` | — | Loader (spinner) |
| `components/cancellable-loader.ts` | — | CancellableLoader |
| `components/select-list.ts` | — | SelectList |
| `components/settings-list.ts` | — | SettingsList |
| `components/spacer.ts` | — | Spacer |
| `components/text.ts` | — | Text |
| `components/truncated-text.ts` | — | TruncatedText |
| `components/editor.ts` | — | Editor |
| `components/image.ts` | — | Image |
| `components/stack.ts` | — | Stack |
| `components/alt-screen-flash.ts` | — | alt-screen flash |
| `keybindings.ts` | — | keybindings |
| `autocomplete.ts` | — | autocomplete |
| `fuzzy.ts` | — | fuzzy match |
| `word-navigation.ts` | — | word navigation |
| `kill-ring.ts` | — | kill ring |
| `latex.ts` | — | LaTeX |
| `native-modifiers.ts` | — | native modifier detection |
| `native-module-path.ts` | — | native module path |

### 18.5 openclaw terminal-core

| 文件 | 关键行 | 内容 |
|------|--------|------|
| `palette.ts` | 1-12 | `LOBSTER_PALETTE` 9 tokens |
| `theme.ts` | 1-36 | chalk-based theme + `isRich` |
| `ansi.ts` | — | ANSI helper |
| `ansi-sequences.ts` | — | ANSI sequence map |
| `restore.ts` | 1-80 | `restoreTerminalState` (`RESET_SEQUENCE`) |
| `safe-text.ts` | — | safe text |
| `progress-line.ts` | — | `clearActiveProgressLine` |
| `osc-progress.ts` | — | OSC 9;4 |
| `table.ts` | — | ASCII table |
| `links.ts` | — | OSC 8 links |
| `prompt-style.ts` | — | prompt style |
| `prompt-select-styled.ts` | — | styled prompt select |
| `prompt-select-styled-params.ts` | — | params |
| `display-string.ts` | — | display width |
| `stream-writer.ts` | — | stream writer |
| `note.ts` | — | note formatting |
| `health-style.ts` | — | health style |
| `decorative-emoji.ts` | — | decorative emoji |
| `index.ts` | — | barrel export |
| `terminal-link.ts` | — | OSC 8 helper |

### 18.6 openclaw src/tui/

| 文件 | 关键行 | 内容 |
|------|--------|------|
| `tui.ts` | — | `runTui` orchestrator |
| `coalesced-refresh.ts` | 1-37 | 渲染合并（类似 pi 的 16ms 节流） |
| `tui-overlays.ts` | — | overlay 集成 |
| `tui-launch.ts` | — | launch lifecycle |
| `embedded-backend.ts` | 1-1547 | embedded backend |
| `tui-formatters.ts` | — | line formatters |
| `tui-input-history.ts` | — | input history |
| `tui-local-shell.ts` | — | `/shell` escape hatch |
| `tui-picker-*.ts` | — | various pickers |
| `theme/theme.ts` | — | tuiTheme |
| `components/chat-log.ts` | — | ChatLog |
| `components/custom-editor.ts` | — | CustomEditor |

### 18.7 deepseek-harness

| 文件 | 包 | 内容 |
|------|----|------|
| `ui-renderer/src/index.ts` | @deepseek-ai/dsh-client-ui-renderer | React 入口 |
| `ui-primitives/src/ansi.ts` | @deepseek-ai/dsh-client-ui-primitives | anser ANSI → CSS |
| `ui-primitives/src/BrandWordmark.tsx` | — | brand wordmark |
| `ui-primitives/src/Button.tsx` | — | Button |
| `ui-primitives/src/ConnectionBanner.tsx` | — | banner |
| `ui-primitives/src/DiffBlock.tsx` | — | diff |
| `ui-primitives/src/DisclosureRow.tsx` | — | disclosure |
| `ui-primitives/src/FishLogo.tsx` | — | logo |
| `ui-primitives/src/FoldToggle.tsx` | — | toggle |
| `ui-primitives/src/HoverCard.tsx` | — | hover |
| `ui-primitives/src/Input.tsx` | — | input |
| `ui-primitives/src/JsonTree.tsx` | — | JSON tree |
| `ui-primitives/src/Menu.tsx` | — | menu |
| `ui-primitives/src/Modal.tsx` | — | modal |
| `ui-primitives/src/markdown/` | — | markdown |
| `locale/` | — | i18n |
| `ui-chat/` | — | chat-specific |
| `ui-agent-preset/` | — | agent preset |
| `ui-approval/` | — | approval |
| `ui-attachment/` | — | attachment |
| `ui-brand-official/` | — | brand |
| `ui-commands/` | — | commands |
| `ui-conversation/` | — | conversation |
| `ui-deliverables/` | — | deliverables |
| `ui-directory-picker-browse/` | — | directory picker |
| `ui-directory-picker-native/` | — | native picker |
| `ui-goal/` | — | goal |
| `ui-input-trigger/` | — | input trigger |
| `ui-jobs/` | — | jobs |
| `ui-layout/` | — | layout |
| `ui-message-feedback/` | — | feedback |
| `ui-model-selection/` | — | model select |
| `ui-permission-presets/` | — | permission |
| `ui-plan/` | — | plan |
| `ui-reference/` | — | reference |
| `ui-session/` | — | session |
| `ui-settings/` | — | settings |
| `ui-sidebar/` | — | sidebar |
| `ui-skill/` | — | skill |
| `ui-slots/` | — | slots |

### 18.8 laew `src/tui/`

| 文件 | 行数 | 内容 |
|------|------|------|
| `mod.rs` | 627 | REPL 主循环 + 路由 `/provider *` |
| `engine.rs` | 277 | Screen trait + Frame + present 全量重绘 |
| `input.rs` | 415 | 单行输入 + crossterm raw mode + 补全 |
| `completion.rs` | 255 | 斜杠命令补全（行内提示） |
| `form.rs` | 371 | Tab 表单状态机 |
| `theme.rs` | 59 | 仅常量（FG/ACCENT/ERROR） |
| `screen/mod.rs` | 4 | 子屏 barrel |
| `screen/provider_list.rs` | 233 | 5 只读字段 Tab + 操作按钮 |
| `screen/provider_form.rs` | 303 | 5+1 Tab 表单 |
| `screen/provider_del.rs` | 276 | Picker + 二次确认 |

---

## 19. 一句话总结

> **atomcode** 是**重工业级** retained cell-based TUI（Rust + 70k 行 + 31k 行 event_loop + JediTerm/legacy_conhost/Windows console 兼容性矩阵）。
> **claudecode Ink Fork** 是**极致优化**的代表（96 文件/13k 行 packed Int32Array + 3 个共享池 + transitionCache + log-update）。
> **opencode** 走 **SolidJS OpenTUI** 响应式路线。
> **pi** 是**极简主义**自研（无 cell grid + APC CURSOR_MARKER + 16ms 节流）。
> **openclaw** **复用** pi-tui + 自研 ANSI helper（LOBSTER_PALETTE）。
> **deepseek-harness** 是 **Web DOM + anser** 路径，与终端协议无关。
>
> **laew 现状** = atomcode 的小屏版（Cell struct + 全量重绘 + 无 worker thread + 无 CJK width model + 无 markdown）。
>
> **P0 必须做**：Cell grid + Screen 双缓冲 + DEC 2026 + cursor park（直接借鉴 atomcode `screen.rs`/`cell.rs`，约 600 行 Rust）。
>
> **P1 重要**：流式 Markdown + CJK 宽度 + worker thread + Spinner + light/dark（atomcode `markdown.rs` 800 行 + `cell.rs:160-193` 100 行 + `worker.rs` 400 行）。
>
> **P2 增强**：鼠标选区 + Kitty CSI-u + i18n + JediTerm tight repaint。
