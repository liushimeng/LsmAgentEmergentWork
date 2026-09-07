# 第八轮·专题 — TUI 渲染管线 / 终端控制序列 / cell-based retained mode / Kitty CSI-u / DEC 2026 / CJK 宽度深度对比

> **范围**：本专题专注 **TUI 渲染管线** 这一深维度,不重复第六轮 TUI 主题（第六轮侧重
> 渲染模型四档、16ms 节流、SSE/partial-JSON、cell-diff、worker、对象池、CJK 宽度；
> 本轮专注 **cell-based retained mode 内部细节 + Kitty CSI-u 真实协商/解析 +
> DEC 2026 同步输出包裹 + CJK/emoji 宽度算法 + worker thread 渲染 + 鼠标协议 + 终端能力探测**)。
>
> **覆盖工程**（8 个）：atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / undici / **laew（现状盘点）**。
> **关键定位**：atomcode = cell-based retained + 真 Kitty 协议；claudecode = Ink fork 真 cell-based + 真 Kitty；pi = line-diff + 真 Kitty + DEC 2026；openclaw = 复用 pi-tui = line-diff + Kitty；opencode = `@opentui/solid` cell-based SDK；deepseek = PTY 服务（无 TUI）；undici = HTTP 客户端（无 TUI）；laew = crossterm **全量重绘**。

---

## 0. 摘要 & TL;DR

| 工程 | 渲染模型 | cell 数组 | 双重缓冲 | DEC 2026 | Kitty CSI-u | CJK 宽度 | worker 线程 |
|------|---------|-----------|----------|----------|-------------|----------|-------------|
| **atomcode** | **cell-based retained** | ✅ W×H `Cell` 网格 | ✅ `cells`/`prev_cells` | ✅ 每帧包裹 + 嵌套抑制 | ✅ 真实 `DISAMBIGUATE`（不带 REPORT_EVENT_TYPES）| ✅ `unicode-width` + 手编 EA 表 | ✅ `worker.rs` 独立 OS 线程 |
| **claudecode** | **cell-based retained (Ink fork)** | ✅ `Int32Array` packed（2 字/cell）| ✅ `prevScreen` Yoga blit | ✅ `BSU/ESU` 包裹（tmux 跳过）| ✅ `CSI_U_RE` 解析 + `>1u` 启用（仅白名单终端）| ✅ `eastAsianWidth` + Bun `stringWidth` | ❌ 主线程渲染 |
| **pi** | **line-diff（string[]）** | ❌（仅 `string[]` 行快照）| ❌（in-memory snapshot）| ✅ **每帧必包** | ✅ 真 `CSI ? u` 协商 + 7 flags + 150ms 分片重组 | ✅ `Intl.Segmenter` + `get-east-asian-width` | ❌（native 仅 modifier key）|
| **openclaw** | line-diff（**fork pi-tui**）| ❌ | ❌ | ✅（继承）| ✅（继承 + `\x1b[<u` reset 在 `terminal-core/restore.ts:5`）| ✅（继承）| ❌ |
| **opencode** | **第三方 `@opentui/solid` cell-based** | ✅（SDK 内部）| ✅ | ✅（SDK）| ✅（`{ kittyKeyboard: true }` 测试 fixture）| ✅（SDK）| ❌（SDK 主线程）|
| **deepseek-harness** | **无 TUI** | — | — | — | — | — | —（PTY 服务）|
| **undici** | **无 TUI**（HTTP 客户端）| — | — | — | — | — | — |
| **laew（现状）** | **crossterm 全量重绘** | ✅ 但**不 diff**（Frame 一次性）| ❌ | ❌ | ❌ | ❌（`unicode-width` 未集成）| ❌ |

> **TL;DR — 三个最大发现**：
> 1. **laew 现在是「全量清屏 + 全行重写」—— 没有 cell-diff、没有 DEC 2026、没有 Kitty 协议、没有鼠标、没有 DEC alternate screen 之外的 fallback。**这是 laew 从 PoC 升级到生产级 TUI 的第一关。
> 2. **Kitty CSI-u 已成行业基线**：atomcode / claudecode / pi / openclaw 全部实现；
>    **但 7 个项目里有 6 个选择不请求 `REPORT_EVENT_TYPES`**，仅 claudecode 用白名单 + pi 用全 7 位 flags。
>    这是一个「实现 ≥50% 即够用」的工业共识。
> 3. **DEC 2026 同步输出**：atomcode 在保留 cell-diff 的同时**嵌套抑制 BSU/ESU**
>    （`Screen::sync_suppressed`，`screen.rs:84-91`）让 `/resume` 批处理单开一扇门；
>    claudecode 在 tmux 下跳过同步（`tmux chunks`）；pi 把每帧渲染都包裹。

---

## 1. 背景：Agent CLI 为何必须高性能 TUI

Agent CLI（laew / atomcode / claudecode / opencode / pi 等）与人类对话时面临的 TUI 挑战：

1. **流式增量**：LLM token-by-token 输出 → 滚动区域每秒重绘多次；
2. **多组件并发**：流式 body + spinner + footer menu + status bar + 弹窗 → 任何一帧都要协调;
3. **终端异构**：本地终端 / SSH / tmux / VSCode 内嵌 / Windows Terminal / kitty / iTerm2
   各自支持不同的协议子集；
4. **CJK / emoji**：用户输入中文文件名、中文 prompt、emoji 都需要正确的宽度计算
   才能让 cell-diff 落点对齐。

传统的 **raw-mode 全量重绘** 在 80×24 上每秒 60 次只产生 4 KiB/帧，但 Agent 的 body 是
**可变高度的滚动**（从几行到几千行），全量重绘就吃不消了。这迫使所有主流 Agent CLI
演进到 **cell-based retained mode** 或 **line-diff**，外加 DEC 2026 解决撕裂问题。

---

## 2. atomcode —— Rust tuix：cell-based retained 真品

### 2.1 渲染模型

atomcode 的 tuix 是一个 **Ink-style cell buffer**（注释直接承认「Ink-style」，见 `render/cell.rs:1-3`）：
> "Ink-style cell buffer for footer/menu rendering. The row-level diff we had before was correct but coarse… New frame → diff cell-by-cell → emit minimal patches."

**核心单元 `Cell`**（`crates/atomcode-tuix/src/render/cell.rs:32-84`）：
- `ch: char` —— 字符
- `style: CellStyle` —— fg/bg/bold/reverse/faint（SGR 子集）
- `width: u8` —— 显示宽度（1 / 2 / **0 = continuation cell**）

**continuation cell 的作用**（`cell.rs:60-67`）：
> "Without continuation cells, typing 你是谁 (3 wide chars = 6 cols) into a row model that tracked only char count (3 cells) would emit patches at model cols 5/6/7 while the terminal had just advanced to actual col 11 after the first 你, overwriting each preceding glyph's right half…"

即「输入 3 个 wide 字符后只看到最后一个字符」bug 的根因修复。

### 2.2 双重缓冲 + dirty region + scroll_up

**`Screen` 结构**（`crates/atomcode-tuix/src/render/screen.rs:46-92`）：

```rust
pub struct Screen {
    cells: Vec<Vec<Cell>>,        // 当前帧（W × H 网格）
    prev_cells: Vec<Vec<Cell>>,    // 上次发出的帧（diff basis）
    width: u16, height: u16,
    cursor: Option<(u16, u16)>,
    cursor_visible: bool,
    physical_dirty: bool,         // 终端状态未知标志
    last_cursor: Option<(u16, u16)>,
    last_cursor_visible: Option<bool>,
    jediterm: bool,                // JetBrains IDE 终端的 per-row tight 重绘
    sync_suppressed: bool,        // 嵌套 DEC 2026 抑制
}
```

**scroll_up**（`screen.rs:208-226`）：用 `Vec::rotate_left` 旋转 `[0..bottom)`，
然后 blank 底部 n 行，O(bottom) memcpy。无 DECSTBM 终端侧滚动——所有滚动都在 cell 网格里。

**invalidate / invalidate_rows_from / shift_prev_up**（`screen.rs:348-447`）：
- 关键设计是 **sentinel cell（不是 blank cell）** 替换 prev_cells —— 见 `Cell::sentinel`
- 否则「cells=blank == prev=blank」会让 diff 抑制本该重绘的 stale wide 字符残留

### 2.3 render_diff 输出字节序列

`Screen::render_diff`（`screen.rs:233-341`）按以下顺序输出：

1. **cold-start 路径**（仅 `physical_dirty`）：每行 `\x1b[N;1H\x1b[K` 清屏 + `\x1b[H` home。
2. **正常路径**：`diff_cell_frames` + `serialize_patches`（或 JediTerm 模式 `serialize_frames_tight`）。
3. **同步输出包裹**（`screen.rs:285-333`）：
   - 若 `!sync_suppressed`：包裹 `\x1b[?2026h` … `\x1b[?2026l`。
   - 始终包裹 `\x1b[?25l`（隐藏光标走 patch walk）+ 尾部 `\x1b[?25h`/`l`（恢复）。
   - 末尾 CUP（绝对定位）跳回 input 提示符。

**关键 anti-flicker 三件套**：
- `?25l` 在 patch walk 前隐藏光标（避免 caret 闪动）
- `?2026h/l` 让支持的主机（iTerm2 / kitty / wezterm / alacritty / Windows Terminal）延迟 paint
- `JediTerm` 模式用 **one CUP + contiguous run** 代替 per-cell CUP

测试断言（`screen.rs:790-830`）：BSU/ESU 必须包裹 cell patches；cursor-vis 跟随
`?25h/l`；且整套输出格式严格守恒。

### 2.4 Kitty CSI-u

**协议启用条件**（`crates/atomcode-tuix/src/lib.rs:97-111`）：

```rust
pub(crate) fn should_enable_kitty_keyboard(caps: &TerminalCaps) -> bool {
    caps.tty && !cfg!(windows) && caps.kitty_keyboard
}

pub(crate) fn kitty_keyboard_flags() -> KeyboardEnhancementFlags {
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES   // 只此 1 位
}
```

**主动排除的设计决策**（`lib.rs:139-186`，**重要反向用例**）：
> "Do not request REPORT_EVENT_TYPES. Release reports are unnecessary for our input model and can leak into the input box when a terminal splits the leading ESC from the rest of a CSI-u report."
> "WINDOWS EXCLUSION: never push on Windows. crossterm's Windows input backend reads Win32 console KEY_EVENT records (not an ANSI parser)…"
> "JEDITERM EXCLUSION: same failure class on JetBrains' JediTerm… it re-frames the terminal's mouse-tracking reports as `CSI <n> u` key events, so a bare mouse *move* over the panel floods stdin with kitty key sequences."

**TerminalCaps** 探测（`terminal.rs:284-298`）：
```rust
let known_kitty_keyboard = is_non_empty(&env.kitty_window_id) || term.contains("kitty") || matches!(term.as_str(), "kitty" | "xterm-kitty");
let kitty_keyboard = env.force_kitty_keyboard.unwrap_or(known_kitty_keyboard) && !jediterm;
```
环境覆盖：`ATOMCODE_KITTY=1|0`（`terminal.rs:101-104`）、`KITTY_WINDOW_ID`、TERM。

**resume_from_external 路径**（`render/retained.rs:9880-9954`）会再次 push `PushKeyboardEnhancementFlags(kitty_keyboard_flags())`，mirror 初始 push。

### 2.5 CJK 宽度算法

**`crates/atomcode-tuix/src/width.rs:40-87`** 三层策略：

1. **`unicode-width::UnicodeWidthChar::width` / `width_cjk`** 基准查询；
2. **`ATOMCODE_CJK_WIDTH=1`** opt-in 切到 `width_cjk`（因为 EA Ambiguous 在 CJK locale
   实际 paint 为 2 col；默认关，因为现代终端全部窄画）；
3. **`ATOMCODE_EMOJI_WIDTH=wide|narrow`** opt-in/out emoji 宽度判断，因为：
   - **legacy symbol block**（`U+2600`-`U+27BF`）`unicode-width` 报告 1，但 GUI 终端 paint 为 2；
   - **`U+1F000+` range 内 ~759 个 codepoint**（如 🌤 U+1F324、🎖 U+1F396、🧿 U+1F9FF）EA=N，
     `unicode-width` 报告 1，但 GUI 终端仍画 2-cell 彩色 emoji。

**特殊反例**（`width.rs:145-150`）：U+23F8-U+23FA（⏸⏹⏺）Emoji_Presentation=No，
bare 时 TEXT 表现 width 1，列入 wide 范围会多占 cell 让 cursor desync；**故意排除**。
同理 U+2611（☑ ballot box）bare 时也 narrow。

`is_wide_emoji_symbol`（`width.rs:130-220+`）实现为**sorted non-overlapping inclusive
ranges**数组 + binary search，每条都附「为什么这个 codepoint 选 wide / narrow」注释。

### 2.6 worker thread 渲染

**`crates/atomcode-tuix/src/render/worker.rs`**：

动机（`worker.rs:1-40`）：
> "Mac Terminal.app takes 30-60ms to process a full footer ANSI payload. When the event loop calls renderer.render() directly, that 30-60ms blocks the select! loop, which means: the spinner tick task can't deliver (drops), the next keystroke can't be read, agent events queue up behind the render."

**架构**（`worker.rs:42-48`）：
```rust
enum RenderCmd {
    Line { line: UiLine, epoch: u64, surface_session: u64 },
    Flush, FlushDeferred,
    ForceRepaint,
    Resize { cols: u16, rows: u16, epoch: u64, surface_session: u64 },
    Reset, ClearScreen, SuspendForExternal, ResumeFromExternal, Shutdown,
}
```

**同步 vs 异步生命周期**（`worker.rs:22-40`）：
- fire-and-forget：`render(UiLine)` 入队；
- 需 ACK：`reset` / `clear_screen` / `suspend_for_external` / `resume_from_external` /
  `shutdown` 配 `oneshot` ACK 让 caller 阻塞至完成；
- `Drop` 显式发 `Shutdown` + join，保证 terminal reset bytes 在 `run()` 返回前落地。

### 2.7 终端能力探测 + mouse 协议

**`crates/atomcode-tuix/src/terminal.rs`** 的 `TerminalCaps::from_env`（约 1091 行）：
- `TERM` allowlist（`terminal.rs:309-312`）：`"kitty" | "xterm-kitty" | "wezterm" | "alacritty" | "ghostty" | "iterm.app" | "iterm2"` 等；
- `KITTY_WINDOW_ID` env probe；
- 单独 `mouse_sgr`、`mouse_any_event`、`osc52_clipboard`、`colors` 字段；
- `jediterm` flag：检测 `TERMINAL_EMULATOR=JetBrains-JediTerm`（`terminal.rs:970-986` 测试）。

---

## 3. claudecode —— Ink fork：cell-based + 真 Kitty CSI-u

### 3.1 渲染模型：packed Int32Array cell grid

**`src/ink/screen.ts`** 不是行级 diff——是真正的 cell buffer：

- **packed storage**（`screen.ts:332-353`）：每个 cell 2 个 Int32 = 8 字节
  - word0 = `charId`
  - word1 = `styleId[31:17] | hyperlinkId[16:2] | width[1:0]`
- **`createScreen()`**（`screen.ts:451-492`）单 ArrayBuffer 双视图
  (`Int32Array` + `BigInt64Array`)，`resetScreen()` 用 `cells64.fill(EMPTY_CELL_VALUE)` 单次 fill。
- **`setCellAt`**（`screen.ts:693-810`）：写 cell 时跟踪 damage 矩形：
  ```ts
  if (damage) {
    damage.x = min(damage.x, x);
    damage.width = max(damage.x + damage.width, x + 1) - damage.x;
  } else {
    screen.damage = { x: minX, y, width: x - minX + 1, height: 1 }
  }
  ```
- **`diff()` / `diffEach()`**（`screen.ts:1126-1206`）：damage-tracked region iteration，
  union `prev.damage ∪ next.damage` 后逐 cell 比对。
- **宽字符支持**（`screen.ts:290-300`）：`CellWidth { Narrow=0, Wide=1, SpacerTail=2, SpacerHead=3 }`，
  spacer cell charId=1 由 `visibleCellAtIndex`（`screen.ts:633`）跳过。

### 3.2 Yoga layout → cell grid

**`src/ink/render-node-to-output.ts`**：从 Yoga DOMElement 读坐标（`render-node-to-output.ts:409-440`），
对每个 node 应用 dirty 标记 + 缓存上次 layout 矩形，若匹配则 **`output.blit(prevScreen, …)`** 整块拷贝
（`render-node-to-output.ts:452-482`），节省重新 layout 时间。

`Output` 类（`output.ts:170-189`）作为 operation collector，`get()`（`output.ts:268-531`）
三遍 pass：
1. 扩张 damage 覆盖 clear 区域；
2. 应用 write/blit/clip/shift 操作；
3. 末尾应用 `noSelect` 让 selection 让位给 blit。

### 3.3 LogUpdate → Diff → Wire

**`src/ink/log-update.ts`** 把 cell diff 转化为 diff patch stream：
- **resize detection**（`log-update.ts:142-147`）；
- **DECSTBM scroll optimization**（`log-update.ts:165-185`）：当 alt-screen + `scrollHint` +
  `decstbmSafe` 时用硬件 scroll（DECSTBM + CSI n S/T）减低 cell 写入量；
- **shrinking**（`log-update.ts:258-283`）发 `clear` patches（`eraseLines` from `termio/csi.ts:239-250`）；
- **diff loop**（`log-update.ts:308-381`）：用 `diffEach` 遍历，逐 cell 比对，spacer 跳过，
  style 转换走 `stylePool.transition()` 缓存 ANSI 字符串。

### 3.4 Kitty CSI-u 真协商 + 真解析

**启用序列**（`src/ink/termio/csi.ts:301-319`）：
```ts
export const ENABLE_KITTY_KEYBOARD = csi('>1u')       // flag=1=disambiguate
export const DISABLE_KITTY_KEYBOARD = csi('<u')       // pop
export const ENABLE_MODIFY_OTHER_KEYS = csi('>4;2m')
export const DISABLE_MODIFY_OTHER_KEYS = csi('>4m')
```

**白名单**（`src/ink/terminal.ts:156-169`）：
```ts
const EXTENDED_KEYS_TERMINALS = [
  'iTerm.app', 'kitty', 'WezTerm', 'ghostty', 'tmux', 'windows-terminal',
]
export function supportsExtendedKeys(): boolean {
  return EXTENDED_KEYS_TERMINALS.includes(env.terminal ?? '')
}
```

> 注：注释（`terminal.ts:148-155`）解释了为何不无脑开：
> "We previously enabled unconditionally (#23350), assuming terminals silently ignore unknown CSI — but some terminals honor the enable and emit codepoints our input parser doesn't handle (notably over SSH and in xterm.js-based terminals like VS Code)."

**CSI-u regex + 解析**（`src/ink/parse-keypress.ts:23, 630-652`）：
```ts
const CSI_U_RE = /^\x1b\[(\d+)(?:;(\d+))?u/
…
if ((match = CSI_U_RE.exec(s))) {
  const codepoint = parseInt(match[1]!, 10)
  const modifier = match[2] ? parseInt(match[2], 10) : 1
  const mods = decodeModifier(modifier)
  const name = keycodeToName(codepoint)
  return { kind: 'key', name, ctrl: mods.ctrl, meta: mods.meta,
           shift: mods.shift, super: mods.super, … }
}
```

**CSI-u flags 响应**（`parse-keypress.ts:46, 143-145`）：
```ts
const KITTY_FLAGS_RE = /^\x1b\[\?(\d+)u$/
…
return { type: 'kittyKeyboard', flags: parseInt(m[1]!, 10) }
```

**Xterm modifyOtherKeys**（`parse-keypress.ts:657-673`）：
```ts
const MODIFY_OTHER_KEYS_RE = /^\x1b\[(\d+);(\d+);(\d+)~$/
```

`decodeModifier`（`parse-keypress.ts:465-478`）只解 bits 1-4（shift/alt/ctrl/super），
**丢弃 event-type bits 5-6** —— 即 release/repeat 事件被当成普通 press 处理（与 atomcode
的「不请求 REPORT_EVENT_TYPES」哲学一致）。

`kittyKeyboard()` query（`terminal-querier.ts:76-81`）：发 `CSI ? u` 查询响应，
配合 `TerminalQuerier` 类（`terminal-querier.ts:128-212`）做 sentinel-based async 等待。

### 3.5 DEC 2026 同步输出

**`src/ink/termio/dec.ts:23, 37-38`**：
```ts
export const SYNCHRONIZED_UPDATE: 2026 = 2026
export const BSU = decset(2026)
export const ESU = decreset(2026)
```

**`writeDiffToTerminal`**（`terminal.ts:190-248`）：
```ts
let buffer = useSync ? BSU : ''
for (const patch of diff) {
  switch (patch.type) {
    case 'stdout': buffer += patch.content; break;
    case 'clear': buffer += eraseLines(patch.count); break;
    case 'cursorHide': buffer += HIDE_CURSOR; break;
    case 'cursorShow': buffer += SHOW_CURSOR; break;
    …
  }
}
if (useSync) buffer += ESU
terminal.stdout.write(buffer)
```

**`isSynchronizedOutputSupported()`**（`terminal.ts:70-118`）：白名单 12 个支持源（iTerm.app、
WezTerm、WarpTerminal、ghostty、contour、vscode、alacritty、kitty/KITTY_WINDOW_ID、
xterm-ghostty、foot、ZED_TERM、WT_SESSION、VTE_VERSION>=6800）；
**显式排除 tmux**（`terminal.ts:72-74`）：
> "tmux parses and proxies every byte but doesn't implement DEC 2026. BSU/ESU pass through to the outer terminal but tmux has already broken atomicity by chunking. Skip to save 16 bytes/frame + parser work."

### 3.6 mouse 协议

**`src/ink/termio/dec.ts:51-60`**：组合 `MOUSE_NORMAL(1000) + MOUSE_BUTTON(1002) +
MOUSE_ANY(1003) + MOUSE_SGR(1006) + FOCUS_EVENTS(1004)`。

### 3.8 CJK 宽度

**`src/ink/stringWidth.ts`**：

- Bun fast path（`stringWidth.ts:213-222`）：`Bun.stringWidth(str, { ambiguousIsNarrow: true })`，
  Western context 默认按 ambiguous 窄画；
- **emoji 全覆盖**（`stringWidth.ts:106-127`）：regional indicator pairs（flags=2）、
  incomplete keycap、emoji 默认 width=2；
- **zero-width 字符全集**（`stringWidth.ts:129-203`）：ZW space/joiner、variation selectors、
  Indic/Thai/Arabic combining marks、surrogates、tag chars；
- **复杂 script grapheme caveat**（`stringWidth.ts:205-209`）：Devanagari conjuncts
  `Bun.stringWidth=2` 但 JS fallback 会得 1 —— 用 Bun 是关键。

### 3.9 终端能力探测

**`src/utils/env.ts:135-234`** `detectTerminal()` 17 级层次探测
（CURSOR_TRACE_ID / VSCODE_GIT_ASKPASS_MAIN / __CFBundleIdentifier / VisualStudioVersion /
TERMINAL_EMULATOR / TERM=xterm-ghostty / TERM includes 'kitty' / TERM_PROGRAM / TMUX /
STY / KONSOLE_VERSION / GNOME_TERMINAL_SERVICE / XTERM_VERSION / VTE_VERSION /
TERMINATOR_UUID / KITTY_WINDOW_ID / ALACRITTY_LOG / TILIX_ID / WT_SESSION / MSYSTEM /
ConEmu / WSL_DISTRO_NAME / SSH / TERM contains alacritty/rxvt/termite / non-interactive / null）。

**async 探测**（`src/ink/terminal-querier.ts:128-212`）：DA1 屏障 + DECRPM / DA1 / DA2 /
KittyKeyboard / CursorPosition / OSC color / XTVERSION。XTVERSION 走 PTY 能在 SSH 下
survive（`terminal.ts:120-128`）。

---

## 4. pi —— line-diff + 真 Kitty 7-flags + DEC 2026

### 4.1 渲染模型：string[] line-diff

**`packages/tui/src/tui.ts:1-3`** 注释："Minimal TUI implementation with differential rendering"。

**核心结构**：
- **`tui.ts:23-47`** —— `Component.render(width)` 返回 `string[]`（ANSI/SGR 含）；
- **`tui-main-screen.ts:125-131`** —— `previousLines: string[]`、`previousWidth`、
  `previousHeight`、`cursorRow`、`hardwareCursorRow`；
- **`tui-main-screen.ts:361-396`** —— line-diff loop：
  ```ts
  for (let i = 0; i < maxLines; i++) {
    const oldLine = i < this.previousLines.length ? this.previousLines[i] : "";
    const newLine = i < newLines.length ? newLines[i] : "";
    if (oldLine !== newLine) { if (firstChanged === -1) firstChanged = i; lastChanged = i; }
  }
  ```
- **`tui-main-screen.ts:448-545`** —— diff 路径：`\x1b[2K`（清行）+ 新内容；相对
  cursor moves（`\x1b[NB/A`）；`\x1b[2J\x1b[H\x1b[3J` 仅在 resize 用。
- **`tui-alt-screen.ts:1359-1362`** —— alt-screen 路径：
  ```ts
  for (let row = 0; row < height; row++) {
    if (!fullRedraw && !imagesNeedRedraw && screen[row] === this.previousScreen[row]) continue;
    buffer += `\x1b[${row + 1};1H\x1b[2K${preparedKittyScreen.lines[row] ?? ""}`;
  }
  ```

**没有 cell grid、没有 double buffer、没有 dirty region**——只有「上一帧 vs 当前帧
字符串不等就整行重写」。但 alt-screen 用绝对 CUP 而非相对 move，main-screen 用相对
move 走 scrollback。

### 4.2 Kitty CSI-u 全量实现

**`packages/tui/src/keys.ts:24-40`** 全局 flag + setter / getter。

**`keys.ts:587-651`** 完整 regex：
```ts
const csiUMatch = data.match(/^\x1b\[(\d+)(?::(\d*))?(?::(\d+))?(?:;(\d+))?(?::(\d+))?u$/);
// <cp>[:<shifted>[:<base>]]<;mod>[:<event>]u
```

支持 alternate keys（flag 4）+ event type（flag 2），是**所有 7 个项目里最完整的**。

**`keys.ts:505-577`** event-type / repeat / release 判定（`isKeyRelease()` /
`isKeyRepeat()`）。

**`keys.ts:653-694`** `matchesKittySequence()` 支持 `baseLayoutKey`
（非拉丁键盘 Dvorak/Colemak 不误判 Latin letters，`keys.ts:686-691` 有专门的
fall-through 规则）。

**`keys.ts:1333-1401`** `decodeKittyPrintable()` 解 printable 字符的 CSI-u 序列，
`KITTY_PRINTABLE_ALLOWED_MODIFIERS = shift | caps_lock`（拒绝 Ctrl/Alt/Super
出现在 printable decode）。

**terminal.ts**（`packages/tui/src/terminal.ts:14-34, 259-289`）：
- `DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS = 7`（1+2+4 = disambiguate + event-types + alternate-keys）；
- 启动 query：`"\x1b[>7u\x1b[?u\x1b[c"`（push 7 + query flags + DA1 sentinel）；
- 协议响应处理：若有 Kitty flags → enable Kitty + disable modifyOtherKeys；
  若 DA 到达无 Kitty → enable modifyOtherKeys (`\x1b[>4;2m`)；
- **150ms 分片重组**（`terminal.ts:16`）：`KEYBOARD_PROTOCOL_RESPONSE_FRAGMENT_TIMEOUT_MS = 150`
  处理 fragmented query response。
- **legacy Shift+Enter 归一**（`terminal.ts:351-356`）：Apple Terminal / Windows 下用
  native modifier probe 辅助把 `\r + Shift` → `\x1b[13;2u`。
- **disable on shutdown**（`terminal.ts:402-486`）：`\x1b[<u` pop 避免 leak 到 parent shell。

### 4.3 DEC 2026

**每帧必包**：

- **`tui-alt-screen.ts:60-61`** —— `BEGIN_SYNCHRONIZED_OUTPUT = "\x1b[?2026h"`、
  `END_SYNCHRONIZED_OUTPUT = "\x1b[?2026l"`；
- **`tui-alt-screen.ts:1345-1370`** `doRender()` 包裹整帧；
- **`tui-main-screen.ts:17, 279-301, 401-435, 458-566`** —— `BoundedTerminalWriter`
  把 1 MiB chunk flush 留在 2026 窗口内（注释：`tui-main-screen.ts:457`
  "Keep updates wrapped in synchronized output while writing bounded chunks."）；
- `BoundedTerminalWriter.MAX_RENDER_WRITE_CHARS = 1024 * 1024`（`tui-main-screen.ts:7-73`），
  还做 surrogate pair 跨 chunk 保持。

### 4.4 mouse 协议（**SGR 1006 only**）

**`tui-alt-screen.ts:55-59`**：
```ts
ENABLE_BUTTON_MOTION_MOUSE = "\x1b[?1000h\x1b[?1002h\x1b[?1004h\x1b[?1006h"   // 1000+1002+1004+1006
ENABLE_ALL_MOTION_MOUSE    = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1004h\x1b[?1006h"  // +1003
DISABLE_MOUSE              = "\x1b[?1006l\x1b[?1004l\x1b[?1003l\x1b[?1002l\x1b[?1000l"
```
- 不使用 1005（DEC mouse legacy） 或 1015（URXVT）—— 只 1006；
- multiplexers（tmux/zellij/STY）用 `ENABLE_BUTTON_MOTION_MOUSE`（无 1003），
  原生终端用 `ENABLE_ALL_MOTION_MOUSE`（带 1003，`tui-alt-screen.ts:305-315`）。
- **bracket paste** 2004 在 `terminal.ts:187-188` enable / `:446` disable，
  `stdin-buffer.ts:25-26, 324-377` 累积 paste 内容为单个 paste event。

### 4.5 CJK 宽度

**`packages/tui/src/utils.ts:1`** `import { eastAsianWidth } from "get-east-asian-width";`（v1.6.0）。

**utils.ts:174-235 `graphemeWidth(segment)`**：
- `utils.ts:180-182` —— terminalSpacingMark 例外（spacing marks 分配 cells）；
- `utils.ts:185-187` —— zero-width cluster 处理；
- `utils.ts:190-192` —— RGI emoji width = 2；
- `utils.ts:201-206` —— Regional indicator（flags）= 2；
- `utils.ts:208` —— `eastAsianWidth(cp)` 基础查询；
- `utils.ts:210-232` —— grapheme-internal trailing code points 计数
  （Indic consonants / halfwidth/fullwidth forms / Thai/Lao AM vowels）；
- **Intl.Segmenter** 作为 grapheme 分词基础（`utils.ts:4-5`），不是手写 wcwidth。

`widthCache: Map<string, number>`（`utils.ts:50-52`，LRU，`WIDTH_CACHE_SIZE = 512`）。

### 4.6 frame throttling + 渲染调度

**`packages/tui/src/tui.ts:339-343`**：
```ts
renderRequested, renderTimer, lastRenderAt
MIN_RENDER_INTERVAL_MS = 16
```

- `requestRender(force)`（`tui.ts:772-781`）：用 **`process.nextTick`** coalesce 一次 tick 内的多次 render；
- `requestImmediateRender()`（`tui.ts:783-798`）：bypass 16ms throttle，键盘输入延迟敏感路径用；
- `scheduleRender()`（`tui.ts:806-824`）：剩余间隔 `Math.max(0, 16 - elapsed)`；
- `handleTerminalInput`（`tui.ts:898-901`）键盘路径主动 `requestImmediateRender()`。

`isTermuxSession()`（`tui-main-screen.ts:108-110`）：Termux 软键盘弹出时 height 变
不触发 full redraw（`tui-main-screen.ts:344-350`）。

### 4.7 终端能力探测

**`packages/tui/src/terminal-image.ts:53-133`** 11 级 env 探测 + capabilityOverrides（`PI_HYPERLINKS`、`PI_IMAGE_PROTOCOL`、`PI_TRUE_COLOR`）：

1. `TMUX` / TERM tmux → `tmux display-message -p '#{client_termfeatures}'` 探测 hyperlinks；
2. TERM=screen → 无 image；
3. `KITTY_WINDOW_ID` 或 `TERM_PROGRAM=kitty` → `{ images: "kitty", trueColor, hyperlinks: true }`；
4. ghostty/wezterm/warp/iTerm2.app/WT_SESSION/alacritty/vscode/zed 等。

**额外 query**：
- **CSI 16 t** cell size（`tui.ts:742-750, 940-958`）—— `\x1b[6;h;wt` 响应；
- **OSC 11 ; ?** bg color（`tui.ts:1212-1234`）；
- **CSI ? 996 n** dark/light（`tui.ts:1242-1262`）响应 `CSI ? 997 ; 1 n` / `CSI ? 997 ; 2 n`；
- **DEC 2031** color-scheme 变更订阅（`tui.ts:707-709, 732-740`）；
- **OSC 9;4** progress（`terminal.ts:12-13, 543-557`，`TERMINAL_PROGRESS_KEEPALIVE_MS = 1000`）；
- **OSC 0** title / OSC 52 clipboard / OSC 8 hyperlinks。

### 4.8 native 模块（**非渲染线程，纯 modifier key + Windows VT input**）

> **`native/` 是 C 代码，不是 Zig / Rust**（验证：`build.sh` 用 `clang`；
> `darwin-modifiers.c` 直接 `#include <CoreGraphics/CoreGraphics.h>`；`win32-console-mode.c`
> 用 `#include <windows.h>` + `__declspec(dllexport)`）。

**`native/darwin/src/darwin-modifiers.c`（71 行）** + **`native/win32/src/win32-console-mode.c`（120 行）**：
- N-API C addon，唯一用途：**OS-level modifier key 状态查询**（Shift/Ctrl/Alt/Super/Win）；
- darwin 用 `CGEventSourceFlagsState(kCGEventSourceStateCombinedSessionState)` +
  `kCGEventFlagMask*`（`darwin-modifiers.c:31-56`）；
- win32 额外 `enable_virtual_terminal_input` flip `SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_INPUT = 0x0200)` 让 Windows console emit `\x1b[Z` for Shift+Tab；
- win32 还 lazy-resolve `GetAsyncKeyState` from `user32.dll`；
- **无渲染**——`darwin-modifiers.c` 注释："Zig is not used here because it does not provide the Apple SDK or CoreGraphics framework stubs"；
- `package.json` 内置 prebuilt `.node`（darwin-arm64 / darwin-x64 / win32-x64 / win32-arm64）。

pi 的渲染是**纯主线程 TypeScript**，native 只做 modifier detection。

### 4.9 stdin 序列重组 + Kitty graphics + OSC 133 prompt zones

**`packages/tui/src/stdin-buffer.ts:31-181`** 5 类 escape 完整性识别：
- CSI（`[`）、OSC（`]`）、DCS（`P`）、APC（`_`）、SS3（`O`）；
- **CSI final byte range 0x40-0x7E**；special-case SGR mouse `/^<\d+;\d+;\d+[Mm]$/`（`stdin-buffer.ts:108`）；
- OSC：`\x07`（BEL）或 `\x1b\\`（ST）；
- DCS：`ESC P ... ESC \`（XTVersion）；
- APC：`ESC _ ... ESC \`（Kitty graphics responses）。

**`stdin-buffer.ts:186-192`** `parseUnmodifiedKittyPrintableCodepoint` 抑制 release-after-press
时 terminal 又发的 raw byte（防 double event）。

**`stdin-buffer.ts:219-232`** **`\x1b\x1b` lookahead**：检测下一个 char 是否开始
新 escape（CSI/OSC/SS3/DCS/APC），若是则只发单个 `\x1b` + 重启 parsing —— 处理
WezTerm split ESC+release artifact。

**`terminal.ts:402-438`** `drainInput(maxMs=1000, idleMs=50)` 在 shutdown 时禁用 Kitty
协议（`\x1b[<u`，line 408），避免延迟 release 事件生成新 sequence。

**`tui-main-screen.ts:75-106`** `parseKittyImageHeader` 从 `\x1b_G...;` 提取 `i=` `r=`；
`extractKittyImageIds` / `extractKittyImageRows` 用于**图像行范围扩展**（lines 209-230
`expandChangedRangeForKittyImages` —— 行级 change diff 时若触及图像必须扩到整图，否则
部分 patch 会让 image crop 错位）。

**`tui-alt-screen.ts:62-63`** **OSC 133 语义 prompt zones** 解析：`scrollToPrompt`
(lines 436-448) 跳到下一个 `OSC 133 ; A` 标记；doRender 路径 `stripOSC133` 把标记从
显示行剥掉。

### 4.10 CJK-aware tokenize / wrap / truncate + OSC 8 hyperlink preservation

**`packages/tui/src/utils.ts:1`** `import { eastAsianWidth } from "get-east-asian-width";`（v1.6.0）。

**`utils.ts:174-235` `graphemeWidth(segment)`** 全细节：
- tab → 3（lines 175-177）；
- terminal spacing marks → codepoint length（lines 180-182，`terminalSpacingMarkRegex`
  含 Unicode spacing marks + legacy wcwidth exceptions）；
- zero-width clusters → 0（lines 185-187，`zeroWidthRegex`）；
- RGI emoji → 2（lines 190-192，`rgiEmojiRegex = /^\p{RGI_Emoji}$/v`）；
- **Regional indicator symbols** U+1F1E6-U+1F1FF → 2（lines 204-206）—— flag emoji；
- `eastAsianWidth(cp)` 基础查询（lines 208）；
- **trailing-mark / folded-form** accounting（lines 214-232）：halfwidth/fullwidth forms、
  Thai/Lao AM vowels。

**`utils.ts:376-401` `normalizeTerminalOutput`** —— 关键预处理：
- **Thai/Lao AM vowel decomposition**：`U+0E33 → U+0E4D U+0E32`、`U+0EB3 → U+0ECD U+0EB2`，
  避免 differential repaint 时 stale cells；
- **Tab expansion to 3 spaces**（仅 escape sequence 外）；
- CRLF/CR → LF。

**`utils.ts:507-727` `AnsiCodeTracker`** —— 完整 SGR state 跟踪：bold/dim/italic/underline/
blink/inverse/hidden/strikethrough + fg/bg color（30-37/90-97 fg，40-47/100-107 bg，
38;5;N / 38;2;R;G;B extended）。`getActiveCodes` 只发**当前 active** codes（避免
冗余），`getLineEndReset` **不 reset 全部**只发 `\x1b[24m` (underline off) + OSC 8 close。

**`utils.ts:745-819` `splitIntoTokensWithAnsi`** —— CJK-aware tokenize：检测 `cjkBreakRegex`  
（`Han | Hiragana | Katakana | Hangul | Bopomofo` via `\p{Script_Extensions=...}`），
遇 CJK 不 whitespace 时 flush 当前 token + 新 token（line 777）。

**`utils.ts:954-1021` `breakLongWord`** —— grapheme-by-grapheme 切不可断 run。

**`utils.ts:454-502` OSC 8 hyperlink parser** —— 完整 BEL vs ST 区分保留（lines 524-525
注释：some terminals only make BEL-terminated links clickable）。

**`utils.ts:1053-1189` `truncateToWidth`** —— width-aware truncation with ellipsis + ANSI-aware。

**`utils.ts:1195-1245` `sliceByColumn` / `sliceWithWidth`** —— column-based slicing，**strict
mode 排除跨越边界的 wide char**。

### 4.11 `word-navigation.ts` —— Intl.Segmenter 委托

**`packages/tui/src/word-navigation.ts:1, 22-117`** 委托 `Intl.Segmenter({ granularity: "word" })`，
不写 CJK break 逻辑。CJK-aware **间接** 通过 Intl.Segmenter 自带 Unicode word boundaries。
`findWordBackward` / `findWordForward` 三分支：atomic segment / word-like（停在 ASCII
标点）/ non-word non-whitespace（punctuation run）。

### 4.12 layout —— 自定义 Yoga-like（**不是 Yoga**）

**`packages/tui/src/layout.ts`** 自实现 stack + scroll layout system（Yoga-style API 但
**不是 Yoga**）：

- `LayoutNode = StackLayoutNode | ScrollLayoutNode`（`layout-node.ts:42`）；
- `StackLayoutNode.type: "vstack" | "hstack"`（`layout-node.ts:20`），`ScrollLayoutNode.type: "scroll"`（`:36`）；
- `LayoutComponent` augment `Component` 加 `[LAYOUT_NODE]()` symbol-key getter（`layout-node.ts:44-51`）；
- `layout.ts:100-241` `layoutComponent` 三分支（plain / scroll / vstack / hstack）；
- `layout.ts:243-302` scrollbar geometry（`getScrollbarGeometry` + `styleScrollbarCell` + `paintScrollbar`）；
- `layout.ts:304-351` `paintBox` —— full-width fast path（line 324 `if (box.rect.x === 0 && box.rect.width >= totalWidth && …)`）；
- `layout.ts:353-382` `renderLayoutFrame` 入口；
- `layout.ts:384-410` lookups：`containsPoint` / `getScrollViewBox`（depth-first 匹配
  `box.scrollView === scrollView`）/ `getScrollViewsAt`（深度排序）。

### 4.13 frame 调度的精细优化

**`tui.ts:339-345` 关键常量**：
```ts
renderRequested, immediateRenderScheduled, renderTimer, lastRenderAt
static readonly MIN_RENDER_INTERVAL_MS = 16
showHardwareCursor = process.env.PI_HARDWARE_CURSOR === "1"
clearOnShrink = process.env.PI_CLEAR_ON_SHRINK === "1"
```

**`tui.ts:508-557`** `Terminal` interface 含 `moveBy`/`hideCursor`/`showCursor`/`clearLine`/
`clearFromCursor`/`clearScreen`（`\x1b[2J\x1b[H`）/ `setTitle`（OSC 0 + BEL）/`setProgress`
（OSC 9;4 + 1s keepalive，line 540）。

**`tui.ts:725-740`** color-scheme notifications `"\x1b[?2031h"` / `"\x1b[?2031l"` enable/disable。

**`tui.ts:742-750` `queryCellSize()`** —— 发 `\x1b[16t` if images enabled；
**`tui.ts:940-958` `consumeCellSizeResponse`** —— 解析 `\x1b[6;h;wt`，`setCellDimensions`
让所有 component invalidate + requestRender。

**`tui.ts:126-135` `resolveEscapeTimeoutMs()`** —— `DEFAULT_ESCAPE_TIMEOUT_MS = 10`、
`SSH_CONNECTION` / `SSH_TTY` 环境 → 100 ms（SSH 更长）、`PI_TUI_ESC_TIMEOUT` override。

**`tui.ts:133-140`** `isAppleTerminalSession()` 检测 `process.platform === "darwin"
&& process.env.TERM_PROGRAM === "Apple_Terminal"`；
**`tui.ts:351-356` `normalizeNativeShiftEnterInput()`** —— Apple Terminal / Windows 下
把 `\r + Shift held` 重写为 `\x1b[13;2u`（CSI-u Shift+Enter 形式）。

**`tui.ts:715-734` `isWindowsTerminalSession()`** + `matchesRawBackspace` —— Windows
Terminal 下 `\x08` 是 Ctrl+Backspace，其他地方是 plain Backspace。

**`tui-main-screen.ts:108-110, 343-350`** —— **Termux height change 不触发 full redraw**：
软键盘弹出 / 收回不重放历史。

**`tui-main-screen.ts:622-653` `positionHardwareCursor`** —— `\x1b[NB/A` 相对移动 +
绝对 `\x1b[NG`；show/hide 按 `getShowHardwareCursor()`。

### 4.14 `TuiMainScreen` 渲染细节

**`tui-main-screen.ts:17-73` `BoundedTerminalWriter`** —— 1 MiB chunks
（`MAX_RENDER_WRITE_CHARS = 1024 * 1024`），**surrogate-pair-aware splitting**
避免超过 V8 max string length。注释（`tui-main-screen.ts:457`）："Keep updates wrapped
in synchronized output while writing bounded chunks."

**`tui-main-screen.ts:276-318` `fullRender(clear)`** —— synchronized output
（`\x1b[?2026h`/`\x1b[?2026l`）包裹，删前次 Kitty images，clear screen + scrollback
（`\x1b[2J\x1b[H\x1b[3J`），多行 image 用 `\x1b[nA`/`\x1b[nB` 跨行 layout。

**`tui-main-screen.ts:362-566` differential write**：
- 扫描 `firstChanged`/`lastChanged`，对 Kitty image 行 expand；
- 若 `firstChanged < prevViewportTop` → full redraw；
- 移动光标到 `firstChanged`（CSI B/A），每行 `\x1b[2K`，image 行特殊处理；
- 行超过 width 时 log crash 到 `pi-crash.log` + throw（lines 516-543）；
- 可选 `PI_TUI_DEBUG=1` 写 `/tmp/tui/render-*.log`。

### 4.15 `TuiAltScreen` 完整特性集

**`tui-alt-screen.ts:281-319` `beforeTerminalStart`** —— disable autowrap + pick mouse
（tmux/zellij/screen/STY 用 `ENABLE_BUTTON_MOTION_MOUSE` 不含 1003，原生用 `ENABLE_ALL_MOTION_MOUSE` 含 1003）
+ enter alt + `\x1b[2J\x1b[H\x1b[?25l`。

**`tui-alt-screen.ts:562-670` `handleViewportInput`** —— 全键盘 + mouse + wheel + 滚动 + 搜索 + selection + right-click paste（Win-only 限定非 VSCode）的 dispatch；keybindings 全部 gated by `!isRelease` 防 repeat（line 605）。

**`tui-alt-screen.ts:672-722` wheel / SGR mouse 解析**：`/^\x1b\[<(\d+);(\d+);(\d+)[Mm]$/` + legacy `\x1b[M + 3 bytes`。

**`tui-alt-screen.ts:1005-1109` selection**：grapheme-cell aware `getSelectionColumns`
+ `handleSelectionMouseEvent`（OSC 8 URL press-without-drag 激活，lines 1016-1033）+
`updateSelectionAutoScroll`（50 ms interval timer 边缘自动滚动）+ OSC 52 写粘贴板 fallback
（`\x1b]52;c;<base64>\x07`，line 1151）。

**`tui-alt-screen.ts:1226-1241` `applySelectionHighlight`** —— 用 `\x1b[7m` 反色视频包裹，
styled run 内重发 `\x1b[7m` 保反色（line 1237）。

**`tui-alt-screen.ts:1156-1224` search highlight** —— applySearchTextHighlight，按
**降序排序 ranges** 避免 column drift（line 1156 注释）。

**`tui-alt-screen.ts:1310-1377` `doRender()`** —— root = `layoutRoot ?? implicitScrollView`；
full vs differential（`previousScreen.length === 0 || width 变 || height 变`）；full redraw
emit `\x1b[2J` + image deletion；`imagesNeedRedraw` 选 `\x1b[2J` (iTerm2) 或
`deleteAllKittyPlacements`（kitty）；per-row diff 用**绝对 CUP**而非相对 move
（`\x1b[<row+1>;1H\x1b[2K` + line content，line 1361）；end synchronized output 后存
`previousScreen` / `previousScreenWidth/Height` / `currentLayout`。

### 4.16 `terminal-image.ts` 完整协议

**`terminal-image.ts:53-158`** 11 级 env detection + `PI_HYPERLINKS` /
`PI_IMAGE_PROTOCOL` / `PI_TRUE_COLOR` env override。

**`terminal-image.ts:193-308` Kitty + iTerm2 protocol**：
- **`encodeKitty`**（lines 215-259）：4 KiB chunking + `m=1`（more-chunks）/ `m=0`（last）flags，
  default `a=T,f=100,q=2`（transmit + 100% quality + quiet），`C=1` when `moveCursor === false`；
- **`deleteKittyImage(id)`**（lines 265-267）：`\x1b_Ga=d,d=I,i=<id>,q=2\x1b\\`（uppercase `I` = free data）；
- **`deleteAllKittyImages()`**（lines 273-275）：`\x1b_Ga=d,d=A,q=2\x1b\\`；
- **`deleteAllKittyPlacements()`**（lines 278-280）：`\x1b_Ga=d,d=a,q=2\x1b\\`（**lowercase `a` = only placements**）；
- **`encodeITerm2`**（lines 282-308）：`\x1b]1337;File=...` name base64-encoded，inline 默认 1，
  size from `Buffer.byteLength(base64Data, "base64")`。

**`terminal-image.ts:334-419` image registry** —— `kittyImageMetadata` map + generation
计数（evict 1000 oldest）；`getKittyImagePlacement` 走 multi-chunk transmission
（`m=1` 匹配终止）重建 placement-only command。

**`terminal-image.ts:421-433` `cropKittyImageLine`** —— 重建 Kitty controls with `y=` / `h=` /
`r=` 渲染 vertical slice，用于 partially-scrolled-off images。

**`terminal-image.ts:469-608` image dimension parsing**：PNG IHDR / JPEG SOF0/1/2 / GIF87a/89a /
WebP VP8/VP8L/VP8X 四种格式完整实现 + `getImageDimensions(base64, mime)` dispatch。

**`terminal-image.ts:665-696`** OSC 8 hyperlink + image fallback `[Image: <name> [<mime>] [<w>x<h>]]`。

---

## 5. openclaw —— 复用 `@earendil-works/pi-tui`（fork 路径）

### 5.1 复用 pi-tui 作为 TUI 库

**`src/tui/tui.ts:1-11`** 直接 import：
```ts
import {
  Container, Loader, matchesKey, ProcessTerminal,
  Text, TuiMainScreen,
} from "@earendil-works/pi-tui"
```

即 openclaw **不写 TUI**，直接用 `@earendil-works/pi-tui@0.84.4`（npm 上的 pi-tui fork
包，pinned 至 earendil-works 组织）—— 这意味着 openclaw 的渲染模型 = line-diff（继承 pi §4）。
**`tui.ts:895`** `const tui = new TuiMainScreen(new ProcessTerminal());`。

### 5.2 pi-tui 内核的实测细节（npm 安装包 dist）

> 安装路径：`/home/aicon/.nvm/versions/node/v22.23.1/lib/node_modules/openclaw/node_modules/@earendil-works/pi-tui/dist/`

**`dist/terminal.js`**：
- `:9` `TERMINAL_PROGRESS_KEEPALIVE_MS = 1000`；
- `:12` `NATIVE_SHIFT_ENTER_SEQUENCE = "\x1b[13;2u"`；
- `:13` **`DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS = 7`**（1=disambiguate, 2=event-types, 4=alternate-keys）；
- `:15` `KITTY_KEYBOARD_PROTOCOL_QUERY = "\x1b[>7u\x1b[?u\x1b[c"`；
- `:40-55` `resolveEscapeTimeoutMs()`：`DEFAULT_ESCAPE_TIMEOUT_MS = 10`、`DEFAULT_SSH_ESCAPE_TIMEOUT_MS = 100`、env override `PI_TUI_ESC_TIMEOUT`；
- `:60-93` `class ProcessTerminal` 含 `wasRaw` / `_kittyProtocolActive` / `_modifyOtherKeysActive` / `keyboardProtocolPushed` / `keyboardProtocolNegotiationBuffer`；
- `:94-121` `start(onInput, onResize)`：`setRawMode(true)` + `\x1b[?2004h` + 安装 resize handler + 发 SIGWINCH + Windows VT input + `queryAndEnableKittyProtocol()`；
- `:175-196` `handleKeyboardProtocolNegotiationSequence`：检测 kitty-flags，启用/禁用 kitty + modifyOtherKeys fallback；
- `:321-355` `stop()` 反向操作（kitty 关闭 + raw mode 还原到 wasRaw + paste 关闭 + resize handler 卸载）。

**`dist/tui.js`**（TuiBase frame scheduler）：
- `:110` `static MIN_RENDER_INTERVAL_MS = 16`（~60 fps cap）；
- `:435-446` `start()`：`terminal.start(...)` + `terminal.hideCursor()` + 可选 `\x1b[?2031h`
  color-scheme change notifications + CSI 16 t cell size query + `requestRender()`；
- `:480-490` `stop()`：cancel render timer + 关闭 notifications + `showCursor()` +
  `terminal.stop()`；
- `:491-509` `requestRender(force)`：`force=true` 走 `requestImmediateRender()`；否则
  `renderRequested = true` + `process.nextTick(scheduleRender)`；
- `:510-527` `requestImmediateRender`：`process.nextTick` 抢占 setTimeout 队列，键盘
  输入永远不丢一帧。

**`dist/tui-alt-screen.js`**：
- `:14-16` 显式 mouse 协议常量（与 pi 同源）：
  - `ENABLE_BUTTON_MOTION_MOUSE = "\x1b[?1000h\x1b[?1002h\x1b[?1004h\x1b[?1006h"`；
  - `ENABLE_ALL_MOTION_MOUSE = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1004h\x1b[?1006h"`；
  - `DISABLE_MOUSE = "\x1b[?1006l\x1b[?1004l\x1b[?1003l\x1b[?1002l\x1b[?1000l"`；
- `:183` `deleteAllKittyImages`（kitty graphics protocol cleanup）；
- `:1085-1120` 帧渲染 loop：kitty image pre-clear + image protocol routing（kitty vs others）。

**`dist/tui-main-screen.js`**：
- `:391` kitty image pre-clear scroll guard；
- `:421` `this.stop()` main screen 在主屏（不是 alternate screen），stop 干净退出。

### 5.3 自有的 terminal-core 协议重置 + 公共 utility 包

**`packages/terminal-core/src/restore.ts:5`** RESET_SEQUENCE：
```ts
const RESET_SEQUENCE =
  "\x1b[0m\x1b[?25h\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[<u\x1b[>4;0m";
```

**关键三项**：
- `\x1b[<u` —— **pop Kitty 键盘协议**（`termio/csi.ts` 同名常量）；
- `\x1b[>4;0m` —— **disable modifyOtherKeys**；
- `\x1b[?2004l` —— **disable bracketed paste**；
- 鼠标协议：`\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l` 全部 disable。

**terminal-core 公共包**（`packages/terminal-core/src/`，约 16 个文件）：`ansi.ts` /
`ansi-sequences.ts`（`iterateAnsiSegments`）/ `decorative-emoji.ts` / `display-string.ts`
（含 CJK 宽度）/ `links.ts`（OSC 8）/ `osc-progress.ts`（OSC 9;4）/ `progress-line.ts` /
`prompt-select-styled.ts` / `palette.ts` / `safe-text.ts` / `note.ts` 等 utility，
给整个 openclaw monorepo 共享。

### 5.4 openclaw 主仓内的 TUI orchestration 层（不渲染，只调度）

**`src/tui/tui.ts:869-927`** TuiBackend 三选一（opts.backend / EmbeddedTuiBackend /
GatewayChatClient 远程）后构建 `Container` 树：Text header + ChatLog + Container status +
Text footer + CustomEditor（焦点目标）。

**`src/tui/tui.ts:1197-1239`** `setInterval(1000)` 驱动 busy status spinner（waiting/elapsed），
`setInterval(120)` 驱动 waiting spinner —— **没有 frame loop**，渲染节流由 pi-tui 的
`requestRender` 处理（每状态变化都 `requestRender()`，`tui.ts` 17+ 处）。

**`src/tui/tui.ts:506-686`** lifecycle：`beginTuiShutdown`（500/100 ms drain + 2 s
hard-exit timer）+ `scheduleProcessExitAfterTuiReturn`（stderr note + `exit(0)`）+
`resolveTuiCtrlCAction`（clear/warn/exit/force-exit）。

**`src/tui/tui.ts:392-469`** shutdown 错误处理：`isIgnorableTuiStopError`（EBADF/
setRawMode race）+ `isTuiTerminalLossError`（EIO/EPIPE from stdin/stdout/TTY）+
`installTuiTerminalLossExitHandler`（end/close on stdin/stdout + uncaught handler）。

### 5.5 stream assembler + 横向 formatters（chat 渲染层）

**`src/tui/tui-stream-assembler.ts:12-244`** `TuiStreamAssembler`：run-keyed map +
LRU eviction（`MAX_TRACKED_STREAM_RUNS = 200`）+ `isProtectedRun` 防止 evicted paused run；
`ingestDelta` 只在文本真的变了才返回新 display text；`finalize` 优先 streamed 否则 fallback final；
`drop` / `clear` 用于 abort / conversation switch。

**`src/tui/tui-formatters.ts:17-39`** regex constants：`REPLACEMENT_CHAR_RE` /
`LONG_TOKEN_RE`（≥33 non-space chars）/ `RTL_SCRIPT_RE`（Hebrew/Arabic）/ `CJK_SCRIPT_RE`（Han/Hiragana/Katakana/Hangul）/ `BIDI_CONTROL_RE` / `FENCED_CODE_RE` / `INLINE_CODE_RE`。

**`tui-formatters.ts:178-249`**：`normalizeLongTokenForDisplay` 跳过 URL/path/CJK/symbol run，
只切 alphanumeric token（`MAX_TOKEN_CHARS=32`）；`transformOutsideCode` 只在 fenced/inline
code 块外切；`isolateRtlLine` 包裹 U+2067 / U+2069；`sanitizeRenderableLine` 折叠空白 +
`sanitizeTerminalControlsAndBinary`（剥 ANSI / C0/C1 controls / bidi controls）+ `applyRtlIsolation`。

**`tui-formatters.ts:113-128`** `sanitizeTerminalControlsAndBinary`：剥 ANSI（``、``、
``）+ C0/C1 controls（除 TAB/LF/CR）+ bidi controls；如 `�` count ≥ 12 且 ≥ 一半
行 → 替换 `[binary data omitted]`。

**`src/tui/osc8-hyperlinks.ts:1-302`**：OSC 8 hyperlink 注入 `iterateAnsiSegments` 拆 ANSI 段，
`extractUrls` 提取 markdown `[text](url)` 与 bare URL，`findUrlRanges` 处理跨行 URL（pi-tui
word-wrap 切开时的 `pending` cursor），`applyOsc8Ranges` 保留 renderer-owned hyperlink span
（`rendererLink` flag），`addOsc8Hyperlinks` 顶层入口。

**`src/tui/coalesced-refresh.ts:4-37`** `createTuiRefreshCoalescer(refresh, afterDrain?)`：
logical-work coalesce（一个 active + 一个 rerun-on-drain），**不是 frame-time debounce**；
`refresh` 返回 `false` 短路后续 rerun。

---

## 6. opencode —— 第三方 SDK @opentui/solid（关键 delegate 模式）

### 6.1 渲染 SDK：@opentui/solid

**`packages/tui/src/app.tsx:1, 12`**：
```ts
import { render, TimeToFirstDraw, useRenderer, useTerminalDimensions } from "@opentui/solid"
…
import { createCliRenderer, MouseButton } from "@opentui/core"
```

- `@opentui/solid` 是 Solid.js wrapper over `@opentui/core`（CLI renderer SDK v0.4.5）；
- `createCliRenderer` 是核心入口；
- `MouseButton` 类型用于 mouse 事件处理；
- `keymap.tsx:1-10` 用 `@opentui/keymap` 提供 keymap 抽象；
- `bun.lock:2042` 显示 `@opentui/core@0.4.5` 依赖 `bun-ffi-structs@0.2.4`、
  `diff@9.0.0`、`marked@17.0.1`、`string-width@7.2.0`、`strip-ansi@7.1.2`，
  以及可选原生 `@opentui/core-{darwin,linux,win32}-{arm64,x64}{,-musl}`。

**opencode 自己写的 TUI 代码极少**——cell-based retained、双缓冲、dirty-region、
DEC 2026、CSI-u、mouse 协议**全部委托给 `@opentui/*` SDK**。opencode 主仓里只写：
1. 自定义 renderable（基于 SDK 的 `FrameBufferRenderable` + `OptimizedBuffer`）；
2. split-footer scrollback 管线；
3. FPS 调节；
4. grapheme segmentation；
5. Windows `ENABLE_PROCESSED_INPUT` 修复；
6. tmux DCS passthrough；
7. capability env 探测。

### 6.2 cell-based retained 在 opencode 本仓的「可见」部分

虽然核心在 SDK 里，opencode 的自定义 renderable 暴露了 cell 结构的形态：

**`packages/tui/src/component/bg-pulse.tsx:19-61`**：
```ts
class GoUpsellArtRenderable extends FrameBufferRenderable {
  protected override renderSelf(buffer: OptimizedBuffer, deltaTime = 0)
  super.renderSelf(buffer)  // 写入 cell buffer
}
```

**`packages/tui/src/component/bg-pulse-render.ts:170, 236-237, 344-347`**：
- `render(frameBuffer: OptimizedBuffer)` — 主渲染入口；
- `frameBuffer.buffers.fg` / `.bg` 是 `Uint16Array` —— 4 个并行 typed array：
  `char` / `attributes` / `fg` / `bg`（RGBA 4-channel Uint16）；
- `frameBuffer.buffers.char.fill(SPACE); buffers.attributes.fill(0)` —— 重置 cell 内容。

**back-frame snapshot / replay**（`bg-pulse-render.ts:139-259`）：
- `cacheDirty: boolean`、`frameCache: Array<{ fg: Uint16Array, bg: Uint16Array }>`；
- `buildFrameCache` 增量构建（`CACHE_FRAMES_PER_RENDER = 1`）；
- `drawCached` 直接 `frameBuffer.buffers.fg.set(frame.fg)` 整块 back→front —— 这是
  **app-level 的双缓冲复用**，真正的 cell diff 在 SDK 里。

**`packages/opencode/src/cli/cmd/run/runtime.lifecycle.ts:181-195`** split-footer 模式：
```ts
createCliRenderer({
  targetFps: 30, maxFps: 60,
  screenMode: "split-footer",
  footerHeight: FOOTER_HEIGHT,         // 4 行固定 footer
  externalOutputMode: "capture-stdout",
  consoleMode: "disabled",
  clearOnShutdown: false,
  useKittyKeyboard: { events: process.platform === "win32" },
  useMouse: false,                      // split-footer 关 mouse
})
```

**split-footer 双区**（`runtime.lifecycle.ts:100-101, 190-191, 384`）：
- 上半 immutable，写入 scrollback（`writeToScrollback`）；
- 下半 mutable footer，footerHeight 固定 4 行；
- 注释（`runtime.lifecycle.ts:88-91`）："scrollback commits and footer repaints happen in the same frame"。

`renderer.idle()`（`runtime.lifecycle.ts:226, 262, 338, 342` + `footer.ts:591`）：
shutdown / close 前等待 frame queue 排空。

### 6.3 Kitty CSI-u 启用

**`packages/tui/src/app.tsx:199`**：交互 TUI 主路径 `useKittyKeyboard: {}`（full default
progressive enhancement）；

**`packages/opencode/src/cli/cmd/run/runtime.lifecycle.ts:189`** split-footer 模式
`useKittyKeyboard: { events: process.platform === "win32" }`（Windows 上只开 event
types）—— **与 atomcode `should_enable_kitty_keyboard(caps) && !cfg!(windows)`
反向**：opencode 在 Windows 上反而开 event types，因为 SDK 在 Windows 上能正确解码。

`createDefaultOpenTuiKeymap(renderer)`（`keymap.tsx:215`）+ `registerOpencodeKeymap`
（`keymap.tsx:214`）把 keymap addon 串起来处理 kitty。

### 6.4 Mouse

**`packages/tui/src/app.tsx:202`**：`useMouse: !Flag.OPENCODE_DISABLE_MOUSE && input.config.mouse`
（默认 true，env `OPENCODE_DISABLE_MOUSE=1` 关）。

**`packages/tui/src/config/index.tsx:74, 89, 128`** `mouse: Schema.optional(Schema.Boolean)`
默认 true。

`MouseButton`（`packages/tui/src/ui/dialog.tsx:4, 207`）作为事件 payload 类型（不是
emit 序列）。

**split-footer 路径 mouse 关闭**（`runtime.lifecycle.ts:185` `useMouse: false`）。

### 6.5 Frame throttling

**`app.tsx:196`** 主交互模式 `targetFps: 60`；
**`runtime.lifecycle.ts:183-184`** split-footer 模式 `targetFps: 30, maxFps: 60`。

`bg-pulse.tsx:74-87`：当 `<go_upsell_art live/>` 挂载时把 `targetFps`/`maxFps` 临时
调到 30（拆 30 fps 周期 = 4600 ms loop ≈ 138 帧），unmount 恢复。

`bg-pulse-render.ts:80-81, 11`：动画常量 `BREATH_SPEED = 0.0008`、
`CACHE_FRAME_COUNT = Math.round(PERIOD / (1000 / 30))`。

### 6.6 CJK / Grapheme 宽度

**`packages/tui/src/prompt/display.ts:1-9`**：
```ts
const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" })
…
for (const part of graphemes.segment(value)) {
  width += Bun.stringWidth(part.segment)  // newline: 0 by default, here 我们按 1
}
```

`Bun.stringWidth` 走 `@opentui/core` 的 `string-width@7.2.0` transitive dep
（`bun.lock:2042`）。

**`packages/tui/src/component/prompt/autocomplete.tsx:190, 461`** 与
`component/prompt/index.tsx:512` 同样用 `Bun.stringWidth` 算 cursor offset / virtual text。

### 6.7 终端能力探测（仅 env）

**`packages/tui/src/util/system.ts:15-20`**：
```ts
export function describeTerminal() {
  const program = process.env.TERM_PROGRAM || process.env.TERM || "unknown"
  const version = process.env.TERM_PROGRAM_VERSION ? ` ${process.env.TERM_PROGRAM_VERSION}` : ""
  const multiplexer = process.env.TMUX ? " in tmux" : process.env.STY ? " in screen" : ""
  return `${program}${version}${multiplexer}`
}
```

**`packages/tui/src/context/runtime.tsx:267-272`** multiplexer / displayServer hints：
```ts
TMUX ? "tmux" : STY ? "screen" : undefined
WAYLAND_DISPLAY ? "wayland" : DISPLAY ? "x11" : undefined
```

**`packages/tui/src/clipboard.ts:26-27`** tmux DCS passthrough：
```ts
const passthrough = `\x1bPtmux;\x1b${sequence}\x1b\\`
process.env.TMUX ? sequence + passthrough :
process.env.STY  ? passthrough :
                   sequence
```

**Zed 探测**（`packages/tui/src/editor-zed.ts:198`、`context/editor.ts:121`）：
`process.env.ZED_TERM === "true" || process.env.TERM_PROGRAM?.toLowerCase() === "zed"`。

**OSC palette / DECRPM / DA1 / theme 探测**在 SDK：`getPalette({ size: 16 })` 与
`waitForThemeMode(1000)`（`app.tsx:241-242` 调用）。

### 6.8 terminal-win32 适配（**opencode 本仓内自有 FFI**）

**`src/terminal-win32.ts:1-130`** 用 `bun:ffi` `dlopen("kernel32.dll", …)`：
- `win32DisableProcessedInput()`（lines 30-42）清除 `ENABLE_PROCESSED_INPUT`
  让 Ctrl-C 不被 Win32 console 当作 event；
- `win32FlushInputBuffer()`（lines 47-54）；
- `win32InstallCtrlCGuard()`（lines 69-130）包装 `stdin.setRawMode` +
  100ms 轮询后盾。
- 在 `app.tsx:214` 调用。

**这是 opencode 整个 TUI 仓里唯一一处的 `bun:ffi` + native FFI**，其余所有 native
代码都在 SDK 的 prebuilt 二进制里。

### 6.9 测试 fixture kitty

**`packages/tui/test/cli/tui/dialog-prompt.test.tsx:85`**：
```ts
const app = await testRender(() => <Harness />, { kittyKeyboard: true })
```

测试用 SDK 的 `testRender` 接受 `kittyKeyboard: true` 标志模拟 Kitty 协议输入。

### 6.10 没有 Go

`find /usr/local/LsmGitOpenSource/opencode -name "*.go"` 零结果——**opencode TUI 完全是
TypeScript on Bun**，opencode 历史上曾有 Go TUI 已被彻底替换。`packages/tui/package.json:5`
`"type": "module"`。

---

## 7. deepseek-harness —— **无交互 TUI**（PTY 服务 + line-oriented 投影）

### 7.1 核心定位

deepseek-harness 是 Cordis-plugin 框架，提供三个 terminal 相关包：**PTY 注册表
服务** + **模型面向的 6 个工具** + **浏览器 React slot renderer**。**不提供 Node.js
交互 TUI**——交互 TUI 能力都在下游客户端（OpenClaw / OpenCode / deerflow 等）实现。

> 2026-07-16 agent-note（`.agents/notes/implemented/feature/2026-07-16-persistent-pty-sessions.md:133, 153`）：
> "Full-screen TUI support, named key sequences, BEL interruption, terminal resize tools, and **alternate-screen snapshots require a separately proven model-facing contract**."
> "**Include TUI sequences and BEL handling.** Rejected. The source prototype treats those paths as timing-sensitive and still records unresolved alternate-screen and interaction failures. Line-oriented PTY use proves the core value without making those unverified behaviors foundational."

### 7.2 PTY 注册表 `@deepseek-ai/dsh-terminal`

**`packages/terminal/terminal/src/index.ts`**（476 行）+ `types.ts`（177 行）+ `invariant.ts`（30 行）：

- `:7` `import { Context, Service } from '@deepseek-ai/cordis'`；
- `:104-118` `class TerminalSessionService extends Service` 挂 `ctx.terminals`；
- `:125-137` `registerBackend(backend)` —— 一个 backend.type 唯一（`DUPLICATE_BACKEND` 错误）；
- `:154-224` `spawn(owner, request, signal?)`：backend 在 `AbortController` 下 spawn，
  reserve name + mint `pty-${++nextId}` id，失败用 `TerminalBackendCleanupError(AggregateError)`；
- `:243-254` `startSend` —— 同一 session 只能一个 in-flight send；
- `:263-265` `read(owner, id, request?)` —— 返回 bounded scrollback；
- `:274-276` `signal(owner, id, signal)` —— 仅允许 POSIX 信号 (`SIGINT/SIGTERM/SKILL/SIGTSTP/SIGHUP`)；
- `:285-301` `kill(owner, id, reason)` —— idempotent close + awaited quiescence；
- `:308-312` `list(owner)` —— owner-scoped snapshots；
- `:435-454` `disposeAll` —— `ctx.effect(() => () => this.disposeAll(), 'pty teardown')` Cordis 钩子。

`types.ts:29` `TerminalWaitReason = 'stdin_read' | 'inferred_idle' | 'timeout' | 'session_exit'`；
`types.ts:39-41` `TerminalSessionStatus = { kind: 'running' } | { kind: 'exited', exitCode, signal }`；
`types.ts:148-163` `TerminalBackendSession` 接口（motd / startSend / read / signal / status / close）；
`types.ts:166-171` `TerminalBackend = { type, spawn(spec) }`。

### 7.3 模型面向的 6 个 tool `@deepseek-ai/dsh-tool-terminal`

**`packages/terminal/tool-terminal/src/index.ts`**（402 行）：

- `:163-196` `terminal_open`（spawn）；
- `:198-297` `terminal_send`（interactive 或 background，background 走 `jobs.start({ kind: 'pty-send' })` 钩 `@deepseek-ai/dsh-jobs`）；
- `:299-330` `terminal_read`；
- `:332-355` `terminal_signal`；
- `:357-386` `terminal_close`；
- `:388-401` `terminal_list`。

`presentCall` / `presentResult` 全部 produce `card: 'terminal'`（`render.ts:106-158`）。
`render.ts:1-60` 用 `@deepseek-ai/dsh-output-retention` 的 `TextRetainer` 做 head/tail
byte-bounded 截断（`\n[output truncated]` marker）。

### 7.4 bash PTY backend `@deepseek-ai/dsh-terminal-bash`

> `packages/terminal/terminal-bash/README.md:165, 86`:
> "**Line-oriented output only** — a headless xterm maintains control-sequence state only for terminal-protocol replies. Returned output remains normalized to lines, and full-screen alternate-buffer interaction is unsupported."

**`src/index.ts`**（218 行）+ **`src/sanitize.ts`**（188 行）+ **`src/session.ts`**（708 行）+
**`src/config.ts`**（122 行）：

- **TerminalSanitizer**（`sanitize.ts:38-188`）：流式 CSI/OSC/short-escape 移除；保留
  split-sequence carry；识别私有 OSC 标记 `133;D;`（ConEmu OSC 133 D = "command was
  executed"）+ prompt tail 检测；`flush()` 输出 PTY 退出时的尾随 printable fragment；
  `normalizeTerminalText` 把 CRLF/CR → LF + 剥 BEL。
- **LocalPtySession**（`session.ts`）：用 `@xterm/headless` v6 作为 **protocol-reply
  emulator only**（`scrollback: 0`，`session.ts:207`），**不参与返回文本**；sanitizer +
  `BoundedTextBuffer` own 返回文本。`emulator.onData((data) => this.terminal.write(data))`
  把 CPR 等 protocol reply 回写到 PTY，不计入返回。
- `pollReadiness`（`session.ts:472-523`）：prompt-detection / idle-silence / handoff grace /
  stdin-read wait。
- `queueEmulatorData` / `pumpEmulator`（`session.ts:557-594`）异步喂 headless emulator，
  让 protocol replies 在下次 send 之前能 settle。
- `config.ts`：sandbox confinement + readiness timings（`pollIntervalMs` /
  `exactProbeAfterMs` / `idleSilenceMs` / `handoffGraceMs`）+ size（`rows`/`cols`）+ scrollback
  bounds（`scrollbackLines` / `scrollbackMaxBytes`）+ `timeoutMs`。

> **关键洞察**：deepseek-harness 的 cell emulator **是 xterm/headless 而不是自己实现**，
> 这是**第三方复用**思路（与 opencode 用 `@opentui/core` 类似）。但 deepseek 选择
> **line-oriented 投影**——sanitizer + BoundedTextBuffer 而非 cell 投影——把
> "返回给 LLM 的文本" 与 "终端实际渲染" 严格分离。

### 7.5 浏览器 React slot renderer（`packages/client/ui-renderer`）

- `client/index.ts:88-97` 安装 `SlotRegistry` + `slots.install(createSlotRenderer())`，
  `ctx.uiRenderer.mount(container) → root.unmount()`；
- `client/index.ts:71-82` `mountApp`：若 `[data-dsh-boot]` 已存在则 hydrate，
  否则 `createRoot + flushSync(root.render(app()))`；
- `client/bind.ts:21-27` `bindSnapshotSelector`（`useSyncExternalStoreWithSelector` from
  `use-sync-external-store/shim/with-selector`，**唯一允许的 uSES bridge**）；
- `client/scoped-slots.tsx:686` `ANCHOR_STYLE = { display: 'contents' }`（slot wrapper
  不参与 layout）；
- `client/app.tsx:19-22` `buildRenderApp` 返回 `() => ctx.slots.renderSlot('root', {})`。
- **与 raw mode / alt screen / cell-based 完全无关**——纯 React Web 渲染。

### 7.6 grep 结果确认

整 deepseek-harness 全源（含 `packages/terminal/` + `packages/client/` + `packages/extensions/`）：
- `kitty` 零命中（除 agent-note）；
- `CSI u` 零命中；
- `DEC 2026` 零命中；
- `setRawMode` 零命中（`ui-cordis/card-model.ts:129-136` 的 `mode: 'run'|'update'` 是 UI arg 不是 TTY）；
- `alternate screen` 零命中（除两个 agent-note）。

---

## 8. undici —— **无 TUI**

undici 是 Node.js 官方 HTTP 客户端（dispatcher / HTTP/2 / llhttp WASM / 8 拦截器 / Mock 录制回放），
**与 TUI 完全无关**。本专题对 undici 仅作「N/A」标注。

---

## 9. laew（现状盘点）—— crossterm 全量重绘

### 9.1 当前渲染模型

**`src/tui/engine.rs`** 全量重绘：
- **`engine.rs:198-203`** `enter_alt()`：
  ```rust
  pub fn enter_alt() -> io::Result<()> {
      terminal::enable_raw_mode()?;
      execute!(io::stdout(), EnterAlternateScreen, Hide)?;
      Ok(())
  }
  ```
- **`engine.rs:206-210`** `leave_alt()`：
  ```rust
  pub fn leave_alt() -> io::Result<()> {
      execute!(io::stdout(), Show, LeaveAlternateScreen)?;
      let _ = terminal::disable_raw_mode();
      Ok(())
  }
  ```
- **`engine.rs:213-243`** `present(frame)`：先 `MoveTo(0,0) + Clear(ClearType::All)`，
  然后**逐行** MoveTo + ResetColor + SetFG/BG/Attr + Print(整行)，**没有 diff**，
  没有 dirty region，没有 BSU/ESU 包裹：
  ```rust
  for y in 0..frame.area.height {
      execute!(stdout, MoveTo(0, y))?;
      let mut line = String::with_capacity(w);
      for x in 0..frame.area.width {
          let cell = &frame.cells[(y as usize) * w + (x as usize)];
          line.push(cell.ch);
      }
      execute!(stdout, ResetColor, …, Print(&line))?;
  }
  ```

### 9.2 laew 缺什么

- ❌ **没有 cell-diff**：`present()` 每帧都全量清屏 + 全行重写，子屏 < 30 行 OK，
  主屏（InputHandler 单行 + 流式 body）勉强 OK，但如果以后做 status bar / spinner /
  toast 多组件就会成瓶颈；
- ❌ **没有 DEC 2026**：`BSU/ESU` 完全没出现；
- ❌ **没有 Kitty CSI-u**：`PushKeyboardEnhancementFlags` 没用；Shift+Enter / Super /
  Cmd 等 modifier 拿不到；
- ❌ **没有 mouse**：搜 `EnableMouseCapture` 无命中；
- ❌ **没有 CJK 宽度**：laew 当前 `Frame::put_str`（`engine.rs:87-105`）用 `s.chars().count()` 计列宽，
  wide 字符（CJK / emoji）会按 1 col 计算，与终端实际 paint 2 col 不一致 → 表格/输入框错位；
- ❌ **没有 worker thread**：渲染全在主线程；
- ❌ **没有 terminal capability detection**：没有 `TerminalCaps` 抽象，
  无 `KITTY_WINDOW_ID` / `TERM_PROGRAM` / `WT_SESSION` 等探测。

### 9.3 laew 优势 / 已经做对的

- ✅ **alternate screen + raw mode** 用 crossterm 干净封装（`engine.rs:198-210`）；
- ✅ **Frame cell 数组**（`engine.rs:42-65`）结构含 `ch / fg / bg / attr`，扩展到带 `width` /
  `style` / `hyperlink` 容易；
- ✅ **`Screen` trait + 子屏栈**（`engine.rs:189-196`）已经把子屏/主屏分离，
  适合引入 `cell-based retained` 增量改造；
- ✅ **完整 keystroke trampoline**（`input.rs`）走 crossterm Event，已经能拿 `KeyEventKind`，
  升级到 `KeyboardEnhancementFlags` 只是改一个 `execute!` 行。

---

## 10. 横向对比大表（7 工程 × 8 维度）

| 维度 | atomcode | claudecode | pi | openclaw | opencode | deepseek-harness | laew（现状）|
|------|----------|------------|-----|----------|----------|------------------|-------------|
| **渲染模型** | cell-based retained（Ink-style）| cell-based retained（packed Int32）| **line-diff（string[]）**| line-diff（pi-tui fork）| cell-based（opentui SDK）| N/A（PTY 服务）| crossterm 全量重绘 |
| **cell 数组** | `Vec<Vec<Cell>>` | `Int32Array` 8B/cell | ❌（仅 `string[]` 行）| ❌（继承 pi）| ✅（SDK 内部）| — | ✅ 但 present 全量清 |
| **双重缓冲** | `cells` / `prev_cells` swap | `prevScreen` Yoga blit | ❌（in-memory 快照）| ❌ | ✅ | — | ❌ |
| **dirty region** | ✅ `damage` rectangle | ✅ union prev/next damage | ❌（整行字符串比对）| ❌ | ✅（SDK）| — | ❌ |
| **DEC 2026 BSU/ESU** | ✅ 每帧包裹 + nested 抑制 | ✅ tmux 跳过 | ✅ 每帧包裹 + BoundedTerminalWriter | ✅（继承）| ✅（SDK）| — | ❌ |
| **DECSTBM 滚动区** | ❌（cell 网格内 rotate_left）| ✅（scroll optimization 走硬件）| ❌ | ❌ | ✅（SDK）| — | ❌ |
| **Kitty CSI-u 启用** | ✅ 仅 DISAMBIGUATE flag | ✅ 白名单 6 终端 + 仅 flag 1 | ✅ 全 7 flags + alt-keys + event-types | ✅（继承 pi）| ✅（SDK）| — | ❌ |
| **Kitty 协议 pop** | `\x1b[<u` （retained.rs:9657-9954）| `\x1b[<u` （csi.ts:307）| `\x1b[<u` + `\x1b[>4;0m`（terminal.ts:402-486）| RESET_SEQUENCE `restore.ts:5` | SDK | — | ❌ |
| **CSI-u 解析完整度** | DISAMBIGUATE only | DISAMBIGUATE only（modifier 解码，丢 event-type bits）| **完整**：alt-keys + shifted + base + event-type | 继承 | — | — | ❌ |
| **shifted/base layout key** | ❌ | ❌ | ✅（`matchesKittySequence` 显式 fallback 规则）| ❌ | — | — | ❌ |
| **modifyOtherKeys** | ❌ | ✅ enable/disable | ✅ enable/disable as fallback | ✅（继承）| — | — | ❌ |
| **Win32 / JediTerm 反向兼容** | ✅ 主动排除 | ✅ 注释解释 | ❌（windows 用 native modifier probe）| — | ✅ win32 适配 | — | N/A |
| **CJK 宽度** | ✅ unicode-width + 手编 EA 表 + opt-in/out emoji wide | ✅ `eastAsianWidth` + Bun stringWidth fast path | ✅ Intl.Segmenter + eastAsianWidth + get-east-asian-width + LRU cache | ✅（继承 + terminal-core/display-string）| ✅（SDK）| — | ❌（chars().count()）|
| **RGI emoji ZWJ sequence** | ✅（unicode-segmentation）| ✅（Bun stringWidth 走 RGI）| ✅（rgiEmojiRegex + Intl.Segmenter）| ✅（继承）| — | — | ❌ |
| **Regional indicator pairs（flags）** | 通过 EA 推断 | ✅ explicit regional indicator pairs = 2 | ✅ explicit regional indicator = 2 | ✅ | — | — | ❌ |
| **Continuation cell** | ✅（`Cell::continuation` width=0）| ✅（SpacerTail=2 charId=1 跳过）| ❌（行级）| ❌ | ✅ | — | ❌ |
| **Worker thread 渲染** | ✅ OS 线程 + oneshot ACK | ❌（主线程）| ❌（native 仅 modifier）| ❌（继承）| ❌（SDK）| — | ❌ |
| **16ms frame throttle** | ✅（worker 内部）| ✅（DECSTBM + scroll optimization）| ✅ `MIN_RENDER_INTERVAL_MS = 16` | ✅（继承）| ✅（SDK）| — | ❌（每事件直接 present）|
| **nextTick coalesce** | — | — | ✅（`requestRender` 用 process.nextTick）| ✅ | — | — | — |
| **mouse 协议** | EnableMouseCapture + SGR | ✅ 1000+1002+1003+1004+1006 | ✅ 1000+1002+(1003 in native)+1004+1006 | ✅ | ✅ MouseButton 类型 | — | ❌ |
| **mouse 1005 / 1015** | ❌ | ❌（仅 SGR）| ❌（仅 SGR）| ❌ | — | — | ❌ |
| **multiplexer fallback** | jediterm 单独 flag | tmux 是 extended keys 白名单 | 1003 在 tmux/zellij/STY 关闭 | — | — | — | — |
| **alternate screen 切换** | crossterm Enter/Leave | DEC.ALT_SCREEN_CLEAR | `\x1b[?1049h/l` raw sequence | process.stdin.setRawMode + 继承 pi | SDK | — | crossterm |
| **raw mode library** | crossterm | Node `process.stdin.setRawMode` + ink | Node `process.stdin.setRawMode` | 继承 + terminal-core | SDK（Node）| — | crossterm |
| **bracketed paste 2004** | EnableBracketedPaste / DisableBracketedPaste | ✅ enable + tokenizer 累积 | ✅ enable + paste event + WezTerm concat 兼容 | ✅（restore.ts RESET_SEQUENCE）| — | — | ❌ |
| **OSC 8 hyperlink** | — | ✅（osc.ts:403-410 id hash + tmux DCS passthrough）| ✅ via terminal-image.ts | ✅ | — | — | ❌ |
| **OSC 0 title / OSC 52 clipboard** | — | ✅ + tmux load-buffer 适配 | ✅ + tmux load-buffer | ✅ | — | — | ❌ |
| **OSC 9;4 progress** | — | ✅（isProgressReportingAvailable 4 来源探测）| ✅（keepalive 1s）| ✅ | — | — | ❌ |
| **OSC 11 bg query** | — | ✅ | ✅ | — | — | — | ❌ |
| **DSR 996 / DEC 2031 暗亮** | — | ✅ | ✅ | — | — | — | ❌ |
| **CSI 16 t cell size** | — | — | ✅ | — | — | — | ❌ |
| **XTVERSION DCS query** | — | ✅（survives SSH）| — | — | — | — | ❌ |
| **async 协议探测 + DA1 sentinel** | — | ✅ TerminalQuerier | ✅ 150ms 分片重组 | — | — | — | ❌ |
| **TerminalCaps 抽象** | ✅（rich）| ✅ env.ts 17 级 + capability overrides | ✅ terminal-image.ts 11 级 + overrides | ✅ terminal-core | SDK | — | ❌ |
| **resize SIGWINCH 处理** | — | — | ✅（`process.kill(SIGWINCH)` best-effort）| — | — | — | crossterm |
| **cold-start 全屏清空** | per-row CUP+EL（screen.rs:248-253）| `getClearTerminalSequence` + ED 2J 3J | `\x1b[2J\x1b[H\x1b[3J` | — | — | — | Clear(ClearType::All) |
| **物理状态未知 invalidate** | ✅ sentinel 替换 prev_cells | — | — | — | — | — | ❌ |

---

## 11. Cell-Based Retained Mode 详解（atomcode + claudecode 实践）

### 11.1 Cell 类型设计哲学

**atomcode `Cell`（`render/cell.rs:68-73`）**：
```rust
pub struct Cell {
    pub ch: char,
    pub style: CellStyle,
    pub width: u8,    // 0=continuation, 1=narrow, 2=wide
}
```
- 极简字段（fg/bg/bold/reverse/faint），每加一个字段就扩 SGR state machine；
- `width` 字段必须——是 continuation cell 的不变量基础；
- Cell 等价 = 序列化字节等价（`render/cell.rs:54-57`），这是 diff 的不变量。

**claudecode packed cell（`screen.ts:332-353`）**：
```ts
STYLE_SHIFT = 17
HYPERLINK_SHIFT = 2
HYPERLINK_MASK = 0x7fff
WIDTH_MASK = 3
EMPTY_CELL_VALUE = 0n
```
- 8 字节/cell，word0 = charId，word1 = style/hyperlink/width packed bitfield；
- charId 走 string-cache 减少内存；
- `EMPTY_CELL_VALUE = 0n` 让 `BigInt64Array.fill` 一次清空（`screen.ts:532`）。

### 11.2 双缓冲 + swap + scratch

atomcode `Screen::render_diff`（`screen.rs:233-341`）三阶段：cold-start 写入 /
  patch 写入 / anti-flicker 包裹；末尾：
```rust
std::mem::swap(&mut self.prev_cells, &mut self.cells);
self.clear();   // 把新 scratch 清零，否则旧 cell 会跟下一帧 diff 出擦除 patch
```
关键：「scratch 不清零 → N 帧前 stale cells 被认为需要擦 → 多余 patch」。

claudecode Yoga blit（`render-node-to-output.ts:452-482`）：
```ts
if (!node.dirty && !skipSelfBlit && cached && prevScreen) {
  output.blit(prevScreen, fx, fy, fw, fh)
  …
  return
}
```
—— 是 **per-node 的 blit 优化**，比 atomcode「整张 cell grid swap」粒度更细。

### 11.3 DEC 2026 嵌套抑制

atomcode **独有**机制（`screen.rs:84-91`）：
```rust
/// When set, render_diff omits its own per-frame DECSET 2026 envelope
/// (?2026h/?2026l) — the caller has opened a single OUTER synchronized
/// block spanning many operations (the /resume replay batch) and owns
/// the open/close. Emitting a nested ?2026l here would end that outer
/// batch early and re-expose the flicker the batch exists to hide.
sync_suppressed: bool,
```
应用：rendering batch（如 `/resume` 重放历史消息）+ 单次 BSU/ESU，cell-diff 不再嵌套开同步，
否则内部 patch walk 中的 ESU 会**提前结束外层 batch**，让后续命令 patch 直接进 paint 通道
重新撕裂。

claudecode / pi / openclaw 的 BSU/ESU 都是**单层**，无嵌套语义。

### 11.4 anti-flicker 三件套

```rust
// atomcode (screen.rs:285-333)
\x1b[?2026h           // BSU（同步）
\x1b[?25l             // hide caret 走 patch walk
… patches …
\x1b[N;MH             // CUP 跳回
\x1b[?25h             // show caret
\x1b[?2026l           // ESU
```
缺哪一件都撕裂：
- 缺 BSU/ESU → 普通终端上 patch 间歇可见（flicker）；
- 缺 `?25l/h` → caret 跟着 patch 跨屏闪烁；
- 缺 CUP → caret 停在 last patched cell，input prompt 找不到。

claudecode 把这三件拆到 `writeDiffToTerminal`（`terminal.ts:190-248`）单独 cursor patch。

### 11.5 JediTerm / 异终端 tight 重绘

atomcode `Screen::withjeditirm(caps.jediterm)`（`screen.rs:120-125`）：
> "JediTerm mode renders a box-drawing rule as one contiguous run (single CUP for the row) — the per-`─`-CUP fragmentation the default path produces is what visually shatters the rule on JediTerm's paint layer."

测试断言（`screen.rs:498-516`）：rule 必须出现为 `──────`，CUP count = 1。

---

## 12. Kitty CSI-u 详解（claudecode / pi / atomcode 三视角）

### 12.1 协议细节

Kitty 键盘协议（[kovidgoyal/kitty keyboard-protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/))
通过 progressive enhancement flags 启用：

| flag bit | 含义 |
|----------|------|
| 1（0x01）| Disambiguate escape codes — `\x1b` 后跟 printable 时不立即提交 |
| 2（0x02）| Report event types — `:1` press / `:2` repeat / `:3` release |
| 4（0x04）| Report alternate keys — CSI u 的第三参是 base layout key |
| 8（0x08）| Report all keys as escape codes — 全部按键都走 CSI u |
| 16（0x10）| Report associated text |

启用：`\x1b[>Nu`（push flag N）。`\x1b[<u` 弹一次，`\x1b[?u` query 当前 flags。

### 12.2 三种启用哲学

**哲学 A：保守 1 flag**（atomcode / claudecode）
- `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES`（atomcode `lib.rs:109-111`）；
- `\x1b[>1u`（claudecode `termio/csi.ts:301`）；
- 原因：仅想要 modifier+key 区分，release/repeat 用现有 modifier 解码够用。

**哲学 B：白名单+保守 1 flag**（claudecode）
- `EXTENDED_KEYS_TERMINALS`（`terminal.ts:156-163`）只对 6 个终端启用；
- 因为 `#23350`（无白名单）踩坑：SSH 路径下 xterm.js-based 终端会输出解析不了的 codepoint；
- tmux 是 allowlist 特殊项（接受 modifyOtherKeys 但不 forward Kitty）。

**哲学 C：激进 7 flags**（pi）
- `DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS = 7`（`terminal.ts:14-17`）；
- 完整 alt-keys + event-types + base-layout 支持；
- 配套 150ms 分片重组 timer 处理 fragmented query response（`terminal.ts:16`）。

### 12.3 CSI-u 解析深度对比

| 项目 | 解码 alt-key | 解码 base-layout | 解码 event-type | 解码 shifted | modifier bits |
|------|------------|----------------|-----------------|-------------|--------------|
| **atomcode** | ❌ | ❌ | ❌（不用） | ❌ | ✓ |
| **claudecode** | ❌ | ❌ | ❌（modifier bits 5-6 丢） | ❌ | ✓ |
| **pi** | ✅ | ✅（`matchesKittySequence`，Latin fallback 不走 base） | ✅（press/repeat/release） | ✅ | ✓ |

claudecode 的 `decodeModifier`（`parse-keypress.ts:465-478`）只解 bits 1-4，注释没
  提到 event-type bits 5-6，但**实际把它们当成 modifier**，会让 release 事件被当成
  shift+ctrl+release 的奇怪事件 —— 这是一个潜在 bug。

### 12.4 Windows / JediTerm 反向兼容

**atomcode 主动排除**（`lib.rs:139-186`）：
> "WINDOWS EXCLUSION: never push on Windows. crossterm's Windows input backend reads Win32 console KEY_EVENT records (not an ANSI parser), so it already reports Shift+Enter modifiers and autorepeat (Press/Repeat/Release) natively… once a terminal honours the push and starts encoding KEYPAD keys as functional codes (numpad 1 → `ESC[57400u`), ConPTY delivers the un-decoded bytes as the literal characters `[57400u` straight into the input box."

> "JEDITERM EXCLUSION: same failure class on JetBrains' JediTerm… while the progressive-enhancement flags are active it re-frames the terminal's mouse-tracking reports as `CSI <n> u` key events, so a bare mouse move over the panel floods stdin with kitty key sequences."

这是 **industry hard-earned lesson** —— 不要无脑 push，必须 TERM probe + 黑名单。

### 12.5 shutdown pop 必做

**所有实现**都做（claudecode `termio/csi.ts:307` / pi `terminal.ts:402-486` / atomcode
`render/retained.rs:9657-9954` / openclaw `terminal-core/restore.ts:5`）：

```ts
\x1b[<u          // pop Kitty flag stack
\x1b[>4;0m       // disable modifyOtherKeys
\x1b[?2004l      // disable bracketed paste
```

否则下一进程（shell）继承协议栈，看到 release 事件不知如何处理。

---

## 13. 共性模式（跨 7 个工程总结）

### 13.1 三层架构

所有工程都有：
1. **协议层**：CSI/OSC/DEC 序列常量（`termio/csi.ts`、`dec.ts`、`ansi-sequences.ts`）；
2. **能力探测层**：TERM / TERM_PROGRAM / KITTY_WINDOW_ID / VTE_VERSION 探测 +
   capability overrides（`env.ts`、`terminal.ts`、`terminal-image.ts`、`TerminalCaps::from_env`）；
3. **渲染层**：cell grid 或 line-diff + frame throttle + DEC 2026 包裹 + sync writes。

### 13.2 Reset Sequence 标准组合

所有工程退出时都 cleanup（**openclaw `restore.ts:5` 是最完整的样板**）：

```ts
"\x1b[0m"              // SGR reset
"\x1b[?25h"            // show cursor
"\x1b[?1000l"          // mouse off (normal)
"\x1b[?1002l"          // mouse off (button)
"\x1b[?1003l"          // mouse off (any)
"\x1b[?1006l"          // mouse off (SGR)
"\x1b[?2004l"          // bracketed paste off
"\x1b[<u"              // pop Kitty
"\x1b[>4;0m"           // disable modifyOtherKeys
"\x1b[?1049l"          // leave alt screen (atomcode/claudecode 各自的位置)
```

### 13.3 Frame Throttle = 16ms

- pi `MIN_RENDER_INTERVAL_MS = 16`（`tui.ts:339`） ≈ 60 fps；
- atomcode worker drain 频率接近 60 fps；
- claudecode 用 `process.nextTick` coalesce 替代显式 throttle；
- `requestImmediateRender` 用于键盘路径（避免 16ms 抖动）。

### 13.4 DEC 2026 包裹成默认

**只有 atomcode 做嵌套抑制**；其他都简单包裹。**tmux 必须跳过**（claudecode
`terminal.ts:72-74`）—— tmux 把 BSU/ESU 透传但 chunk 字节流破坏了原子性。

### 13.5 多 multiplexers 兼容

- tmux（`process.env.TMUX`）—— passthrough DCS 包裹 inner ESC；
- zellij / screen / STY —— 同 tmux 模式；
- multiplexers 下关闭 1003 mouse any-motion（pi `tui-alt-screen.ts:305-315`）。

### 13.6 Async Query + DA1 Sentinel

claudecode `TerminalQuerier`（`terminal-querier.ts:128-212`）：发送查询 → 等响应 →
DA1 屏障把所有未应答的 promise 标记为 unsupported。**XTVERSION 是唯一能跨 SSH 探测
终端的方法**（`terminal.ts:120-128`），因为 PTY 透传 DCS。

### 13.7 CJK 宽度统一用 Intl.Segmenter + east-asian-width

| 项目 | 实现 |
|------|------|
| atomcode | `unicode-width` + 手编 EA + 手编 emoji ranges + opt-in/out |
| claudecode | `get-east-asian-width` + `Bun.stringWidth` fast path + 手编 isZeroWidth |
| pi | `get-east-asian-width` + `Intl.Segmenter` + LRU cache + 手编 RGI emoji regex |
| openclaw | 继承 pi + terminal-core `display-string.ts` |

### 13.8 CJK / emoji 宽度分两层

1. **基础宽度**（Unicode EA）：N / A / W / F —— narrow / ambiguous / wide / fullwidth；
2. **emoji 修正**：legacy symbol block（U+2600-U+27BF）+ U+1F000+ 中 ~759 EA=N 但 GUI paint 2 col
   —— 必须再判一次，**legacy symbol block 的 `⏸⏹⏺` 和 `☑` 故意 narrow**（bare 时 text 表现）。

---

## 14. 对 laew 的 P0/P1/P2 路线图

> **目标**：把 laew 从「crossterm 全量重绘 + 0 协议」升级到「cell-based retained +
> DEC 2026 + Kitty CSI-u + CJK 宽度 + 异步能力探测」，对齐工业基线。

### P0（短期、必做，2-3 周）

1. **cell-based retained 子屏渲染**
   - 把 `Frame`（`engine.rs:42-65`）加 `width: u8` 字段；
   - 加 `prev_cells` 字段，diff 函数从 atomcode 移植；
   - `present()` 改为发 diff patch 而非全行；
   - **Rust crate**：`crossterm` 已有；**`ratatui`**（双缓冲 cell-based 标杆）可整体替换，
   - 或保留自己引擎。

3. **DEC 2026 包裹**
   - 在 `present()` 前发 `\x1b[?2026h`，末尾 `\x1b[?2026l`；
   - 加 `TerminalCaps::sync_output_supported` 探测（参考 claudecode `terminal.ts:70-118`
     12 来源 + tmux 跳过）。

4. **CJK 宽度集成**
   - 加 `unicode-width` + `unicode-segmentation` crate；
   - `Frame::put_str` 改用 `unicode_width::UnicodeWidthStr::width` + `graphemes`；
   - **Rust crate**：`unicode-width` / `unicode-segmentation`（与 atomcode 同款）。

5. **Kitty CSI-u**
   - 在 `enter_alt()` 加 `crossterm::event::PushKeyboardEnhancementFlags(DISAMBIGUATE_ESCAPE_CODES)`
   - `should_enable_kitty_keyboard` 仿 atomcode（TERM probe + Windows exclude + JediTerm exclude）；
   - `leave_alt()` pop + `\x1b[<u` reset。

### P1（中期，1-2 月）

6. **worker thread 渲染**
   - 学 atomcode `worker.rs`：独立 OS 线程 + oneshot ACK 同步 lifecycle；
   - **Rust crate**：标准库 `std::thread` + `std::sync::mpsc` 足够，不需额外依赖。

7. **terminal capability 抽象**
   - `TerminalCaps` struct（`raw_mode` / `bracketed_paste` / `kitty_keyboard` / `mouse_sgr` /
     `osc52_clipboard` / `colors` / `sync_output` / `jediterm` / `term` 探测）；
   - 探测函数仿 atomcode `terminal.rs:101-346`。

8. **OSC 8 hyperlinks / OSC 0 title / OSC 52 clipboard**
   - 输出 OSC 时按 `terminal = 'kitty'` 选 ST，others 选 BEL（claudecode `termio/osc.ts:19-21`）；
   - tmux 下用 DCS passthrough 包裹（`termio/osc.ts:35-44`）。

9. **bracketed paste + Edit tool 协同**
   - `enter_alt()` enable `\x1b[?2004h`，`leave_alt()` disable；
   - 收到 paste 事件不要插入 indent，否则多行粘贴体验差。

10. **OSC 9;4 progress**
    - spinner / 长任务时发进度（claudecode `terminal.ts:25-64` 4 来源探测）。

### P2（长期，3+ 月）

12. **mouse 协议 + SGR 1006**
    - 子屏 provider form / provider list 加 hover / click；
    - **Rust crate**：`crossterm::event::EnableMouseCapture`。

13. **DSR / DEC 2031 暗亮探测**
    - 启动发 `CSI ? 996 n` + OSC 11 ; ? bg color query；
    - 响应自动切 theme。

14. **async query + DA1 sentinel**
    - 异步查询终端名（XTVERSION DCS，survive SSH）；
    - 配合 `requestRender` 路径走 16ms throttle。

15. **DECSTBM scroll region**
    - 主体滚动 + 固定 footer（学 claudecode `log-update.ts:165-185` scrollHint + DECSTBM + SU/SD）。

### 建议 Rust crate 选型

| 需求 | 选型 |
|------|------|
| 全量 cell-based + 双缓冲 | `ratatui`（标杆，含 `Buffer` / `Cell` / `Frame` / 双 swap）|
| 仅底层 crossterm | `crossterm`（已有，加 `PushKeyboardEnhancementFlags` / `EnableMouseCapture`）|
| DEC 2026 / CSI-u / mouse 协议细节 | 已有 crate 提供 —— 直接手写常量 |
| CJK 宽度 | `unicode-width` + `unicode-segmentation` |
| east-asian-width 查表 | `unicode-width`（含 `width_cjk`）|
| emoji ZWJ sequence | `unicode-segmentation`（grapheme）+ 手编 RGI emoji 清单 |
| Terminfo 查询 | `termion` 或 `termwiz` |
| full terminal emulator-grade | `termwiz`（Rust 实现的 wezterm 渲染内核）|

> **不建议 `alacritty_terminal`** / `crossterm::terminal::window` —— 过重；
> **不建议 `tui-rs` 老 crate** —— 已迁移到 `ratatui`。

---

## 15. 附录：关键代码路径速查表

### atomcode（Rust）
| 主题 | 路径 |
|------|------|
| Cell 单元（含 continuation cell width=0）| `crates/atomcode-tuix/src/render/cell.rs:32-100` |
| 双重缓冲 Screen + `cells`/`prev_cells` swap | `crates/atomcode-tuix/src/render/screen.rs:46-340` |
| scroll_up via rotate_left | `crates/atomcode-tuix/src/render/screen.rs:208-226` |
| Sentinel 修复 invalid prev | `crates/atomcode-tuix/src/render/screen.rs:348-447` |
| JediTerm per-row tight 重 rep | `crates/atomcode-tuix/src/render/screen.rs:120-125, 498-516` |
| sync_suppressed 嵌套 BSU/ESU 抑制 | `crates/atomcode-tuix/src/render/screen.rs:84-91, 113-118, 285-333` |
| cold-start per-row CUP+EL | `crates/atomcode-tuix/src/render/screen.rs:235-256, 602-672` |
| CellStyle 极简字段（fg/bg/bold/reverse/faint）| `crates/atomcode-tuix/src/render/cell.rs:32-52` |
| SGR state machine minimize | `crates/atomcode-tuix/src/render/cell.rs:26-31` |
| push_str_cells / push_str_cells_sgr / serialize_row / diff_cell_frames | `crates/atomcode-tuix/src/render/cell.rs` + `crates/atomcode-tuix/src/render/screen.rs:39` |
| render_diff 全流程（cold-start + patch + anti-flicker）| `crates/atomcode-tuix/src/render/screen.rs:233-341` |
| CJK width + emoji ranges（unicode-width + 手编 EA）| `crates/atomcode-tuix/src/width.rs:40-235+` |
| `ATOMCODE_CJK_WIDTH=1|true` opt-in | `crates/atomcode-tuix/src/width.rs:33-47` |
| `ATOMCODE_EMOJI_WIDTH=narrow|wide` opt-in/out | `crates/atomcode-tuix/src/width.rs:99-117` |
| `is_wide_emoji_symbol` sorted ranges + binary search | `crates/atomcode-tuix/src/width.rs:130-220+` |
| ⏸⏹⏺ 反例（bare TEXT 表现 width 1）| `crates/atomcode-tuix/src/width.rs:145-150` |
| ☑ ballot box 反例 | `crates/atomcode-tuix/src/width.rs:164-170` |
| Worker thread (`RenderCmd` mpsc + oneshot ACK) | `crates/atomcode-tuix/src/render/worker.rs:1-200` |
| Slow Terminal ≠ stalled event loop | `crates/atomcode-tuix/src/render/worker.rs:1-40` |
| TerminalCaps 探测 12+ 来源 | `crates/atomcode-tuix/src/terminal.rs:101-346` |
| Kitty 启用决策（DISAMBIGUATE only）| `crates/atomcode-tuix/src/lib.rs:97-111, 139-186` |
| Windows / JediTerm 主动排除 | `crates/atomcode-tuix/src/lib.rs:139-186` + `terminal.rs:970-986` |
| Kitty push / pop / re-push 路径 | `crates/atomcode-tuix/src/render/retained.rs:9657-9954` |
| Reset sequence （含 `\x1b[<u`）| `crates/atomcode-tuix/src/render/retained.rs:9657-9954` |
| mouse SGR via `EnableMouseCapture` | `crates/atomcode-tuix/src/render/retained.rs:9952-9953` |
| Cell sentinel vs blank（避免 stale wide 残留）| `crates/atomcode-tuix/src/render/screen.rs:348-447` |

### claudecode（TypeScript / Bun）
| 主题 | 路径 |
|------|------|
| Packed Int32 cell grid | `src/ink/screen.ts:332-492, 693-810, 1126-1206` |
| Output collector | `src/ink/output.ts:62-189, 241-531` |
| Yoga layout → cell blit | `src/ink/render-node-to-output.ts:387-540, 452-482` |
| Diff renderer | `src/ink/log-update.ts:65-112, 123-467` |
| DEC 2026 BSU/ESU | `src/ink/termio/dec.ts:23, 37-38` + `src/ink/terminal.ts:190-248` |
| 同步输出探测 | `src/ink/terminal.ts:70-118` |
| 终端名白名单 | `src/ink/terminal.ts:156-169` |
| Mouse 协议 | `src/ink/termio/dec.ts:51-60` |
| CSI-u regex + 解析 | `src/ink/parse-keypress.ts:23, 46, 630-652` |
| Kitty 启用/禁用序列 | `src/ink/termio/csi.ts:301-319` |
| CJK width + Bun fast path | `src/ink/stringWidth.ts:20-90, 213-222` |
| 17 级 env 探测 | `src/utils/env.ts:135-234` |
| Async 协议 query | `src/ink/terminal-querier.ts:49-212` |
| XTVERSION DCS | `src/ink/terminal.ts:120-128` |

### pi（TypeScript / Node + C N-API）
| 主题 | 路径 |
|------|------|
| Line diff loop | `packages/tui/src/tui-main-screen.ts:361-396, 448-545` |
| Alt-screen 路径 | `packages/tui/src/tui-alt-screen.ts:1359-1362, 1310-1377` |
| DEC 2026 | `packages/tui/src/tui-alt-screen.ts:60-61, 1345-1370` + `packages/tui/src/tui-main-screen.ts:279-301, 401-435, 458-566` |
| Mouse 协议（仅 SGR）| `packages/tui/src/tui-alt-screen.ts:55-59, 305-318` |
| Bracketed paste 2004 | `packages/tui/src/terminal.ts:187-188, 446` + `packages/tui/src/stdin-buffer.ts:25-26, 324-377` |
| 7 flags Kitty 协议 | `packages/tui/src/terminal.ts:14-34, 259-289, 360-486` |
| 150ms 分片重组 | `packages/tui/src/terminal.ts:16` |
| CSI-u 完整 regex | `packages/tui/src/keys.ts:587-651, 1333` |
| Event type / release | `packages/tui/src/keys.ts:505-577, 1333-1401` |
| base-layout 兜底（Dvorak/Colemak 防误判）| `packages/tui/src/keys.ts:653-694, 686-691` |
| ModifyOtherKeys fallback | `packages/tui/src/keys.ts:696-702` |
| Apple Terminal Shift+Enter 重写 | `packages/tui/src/terminal.ts:351-356` |
| stdin 序列重组（5 类 escape）| `packages/tui/src/stdin-buffer.ts:31-181` |
| `\x1b\x1b` WezTerm split | `packages/tui/src/stdin-buffer.ts:219-232` |
| Kitty printable dedupe | `packages/tui/src/stdin-buffer.ts:186-192` |
| Kitty image header parse | `packages/tui/src/tui-main-screen.ts:75-106` |
| image 行范围扩展 | `packages/tui/src/tui-main-screen.ts:209-230` |
| BoundedTerminalWriter 1 MiB surrogate-safe | `packages/tui/src/tui-main-screen.ts:17-73` |
| Termux 高度变化跳过 full redraw | `packages/tui/src/tui-main-screen.ts:108-110, 343-350` |
| CJK width 完整 graphemeWidth | `packages/tui/src/utils.ts:174-235` |
| Thai/Lao AM vowel decompose | `packages/tui/src/utils.ts:376-401` |
| AnsiCodeTracker state | `packages/tui/src/utils.ts:507-727` |
| Width cache | `packages/tui/src/utils.ts:50-52` |
| 16ms throttle | `packages/tui/src/tui.ts:339-343, 772-824` |
| requestImmediateRender | `packages/tui/src/tui.ts:783-798, 898-901` |
| SSH escape timeout override | `packages/tui/src/tui.ts:126-135` |
| 11 级 env 探测 | `packages/tui/src/terminal-image.ts:53-133` |
| CSI 16 t cell size | `packages/tui/src/tui.ts:742-750, 940-958` |
| DSR 996 暗亮 + DEC 2031 通知 | `packages/tui/src/tui.ts:1212-1262, 707-740` |
| OSC 9;4 progress | `packages/tui/src/terminal.ts:12-13, 543-557` |
| OSC 133 prompt zones | `packages/tui/src/tui-alt-screen.ts:62-63, 436-448` |
| OSC 8 hyperlink BEL/ST preservation | `packages/tui/src/utils.ts:454-502, 524-525` |
| OSC 52 clipboard fallback | `packages/tui/src/tui-alt-screen.ts:1151` |
| Kitty image encode 4 KiB chunk | `packages/tui/src/terminal-image.ts:215-259` |
| deleteKittyImage / deleteAllKittyImages / deleteAllKittyPlacements | `packages/tui/src/terminal-image.ts:265-280` |
| Image dim parsing PNG/JPEG/GIF/WebP | `packages/tui/src/terminal-image.ts:469-608` |
| 自定义 Yoga-like layout | `packages/tui/src/layout.ts:100-410` + `packages/tui/src/layout-node.ts:5-51` |
| scrollbar geometry | `packages/tui/src/layout.ts:243-302` |
| selection grapheme-cell aware | `packages/tui/src/tui-alt-screen.ts:1073-1109, 1226-1241` |
| search highlight 降序排序 | `packages/tui/src/tui-alt-screen.ts:1156-1224` |
| Native modifier（darwin C）| `packages/tui/native/darwin/src/darwin-modifiers.c:1-71` |
| Native VT input + modifier（win32 C）| `packages/tui/native/win32/src/win32-console-mode.c:1-120` |
| Alt-screen search component | `packages/tui/src/alt-screen-search.ts:1-157` |
| CURSOR_MARKER APC IME | `packages/tui/src/tui.ts:79` + `packages/tui/src/tui-main-screen.ts:622-653` |

### openclaw（TypeScript / Node + pi-tui fork）
| 主题 | 路径 |
|------|------|
| pi-tui fork 引用 | `src/tui/tui.ts:1-11, 895` |
| `TuiMainScreen` + `ProcessTerminal` 实例化 | `src/tui/tui.ts:895` |
| TuiBackend 三选一（opts / Embedded / Gateway）| `src/tui/tui.ts:869-885` |
| Container 树构建 | `src/tui/tui.ts:904-927` |
| Ctrl binding 路由（ESC/C/Ctrl-D/O/L/G/P/T）| `src/tui/tui.ts:1652-1719` |
| Shutdown drain 500/100 ms + 2 s hard-exit | `src/tui/tui.ts:506-564` |
| SIGINT/SIGTERM/SIGHUP handler | `src/tui/tui.ts:1949-1951` |
| EBADF / EIO / EPIPE 错误归类 | `src/tui/tui.ts:392-435` |
| `tui.requestRender()` 调用点（17+ 处）| `src/tui/tui.ts:1066, 1332, 1419, …` |
| busy status spinner（1s）| `src/tui/tui.ts:1197-1202` |
| waiting spinner（120ms）| `src/tui/tui.ts:1234-1239` |
| EmbeddedTuiBackend `TuiBackend` 实现 | `src/tui/embedded-backend.ts:354-413` |
| `sendChat` queue + abort | `src/tui/embedded-backend.ts:457-571` |
| `runTurn` lifecycle | `src/tui/embedded-backend.ts:1365-1540` |
| Stream assembler LRU 200 + isProtectedRun | `src/tui/tui-stream-assembler.ts:9, 108-147` |
| dropped boundary text 处理 | `src/tui/tui-stream-assembler.ts:23-105, 211-235` |
| CJK/RTL/bidi formatters | `src/tui/tui-formatters.ts:17-39, 113-128, 178-249` |
| long token chunking (32 chars) | `src/tui/tui-formatters.ts:178-231` |
| footer format / token 提取 | `src/tui/tui-formatters.ts:52-83, 489-749` |
| OSC 8 hyperlink 注入 + cross-line URL | `src/tui/osc8-hyperlinks.ts:1-302` |
| `coalesced-refresh.ts` logical-work coalesce | `src/tui/coalesced-refresh.ts:4-37` |
| **pi-tui dist 内核** | `/home/aicon/.nvm/versions/node/v22.23.1/lib/node_modules/openclaw/node_modules/@earendil-works/pi-tui/dist/` |
| pi-tui `terminal.js` Kitty + escape timeout | `dist/terminal.js:9-55, 60-93, 94-121, 175-196, 321-355` |
| pi-tui `tui.js` frame scheduler | `dist/tui.js:110, 435-446, 480-490, 491-509, 510-527` |
| pi-tui `tui-alt-screen.js` mouse + image | `dist/tui-alt-screen.js:14-16, 183, 1085-1120` |
| pi-tui `tui-main-screen.js` main screen guard | `dist/tui-main-screen.js:391, 421` |
| RESET_SEQUENCE（含 pop Kitty + disable mouse + paste）| `packages/terminal-core/src/restore.ts:5` |
| 共享 terminal-core（16 文件）| `packages/terminal-core/src/`（ansi / display-string / links / osc-progress / progress-line / prompt-select-styled / palette / safe-text / note / table）|

### opencode（TypeScript / Bun + SDK）
| 主题 | 路径 |
|------|------|
| opentui SDK 引用 | `packages/tui/src/app.tsx:1, 12` |
| `createCliRenderer` 主交互模式 | `packages/tui/src/app.tsx:191-208` |
| `useKittyKeyboard: {}` 主交互全默认 | `packages/tui/src/app.tsx:199` |
| `useMouse` 默认 true（env 关）| `packages/tui/src/app.tsx:202` + `packages/tui/src/config/index.tsx:74, 89, 128` |
| `targetFps: 60` 主交互 | `packages/tui/src/app.tsx:196` |
| `useThread: false` 测试 fixture | `packages/tui/test/app-lifecycle.test.tsx:11, 64` |
| Keymap 抽象（`createDefaultOpenTuiKeymap`）| `packages/tui/src/keymap.tsx:1-15, 214-215` |
| split-footer createCliRenderer | `packages/opencode/src/cli/cmd/run/runtime.lifecycle.ts:181-195` |
| split-footer `targetFps: 30, maxFps: 60` | `packages/opencode/src/cli/cmd/run/runtime.lifecycle.ts:183-184` |
| split-footer `useKittyKeyboard: { events: win32 }` | `packages/opencode/src/cli/cmd/run/runtime.lifecycle.ts:189` |
| split-footer `useMouse: false` | `packages/opencode/src/cli/cmd/run/runtime.lifecycle.ts:185` |
| split-footer `screenMode: 'split-footer'` | `packages/opencode/src/cli/cmd/run/runtime.lifecycle.ts:190-191` |
| split-footer scrollback capture-stdout 模式 | `packages/opencode/src/cli/cmd/run/runtime.lifecycle.ts:181-195` |
| `renderer.idle()` 等待帧排空 | `packages/opencode/src/cli/cmd/run/runtime.lifecycle.ts:226, 262, 338, 342` + `footer.ts:591` |
| `writeToScrollback` 配 `requestRender` 单帧 commit | `packages/opencode/src/cli/cmd/run/runtime.lifecycle.ts:88-91, 152-169` |
| Scrollback writer surface | `packages/opencode/src/cli/cmd/run/scrollback.writer.tsx` + `scrollback.surface.ts:216, 356, 392` |
| External output queue 测试反射 | `packages/opencode/test/cli/run/scrollback.surface.test.ts:28-39, 72-73, 506` |
| Custom FrameBufferRenderable（go-upsell-art）| `packages/tui/src/component/bg-pulse.tsx:19-61` |
| `OptimizedBuffer` 4 平行 typed array (char/attr/fg/bg) | `packages/tui/src/component/bg-pulse-render.ts:170, 236-237, 344-347` |
| back-frame cache replay | `packages/tui/src/component/bg-pulse-render.ts:139-259` |
| `CACHE_FRAME_COUNT` 30 fps 预计算 | `packages/tui/src/component/bg-pulse-render.ts:80-81, 11` |
| 动态 FPS（bg-pulse mounted 时降到 30）| `packages/tui/src/component/bg-pulse.tsx:74-87` |
| prompt display grapheme segmentation | `packages/tui/src/prompt/display.ts:1-9` |
| `Bun.stringWidth` cursor offset | `packages/tui/src/component/prompt/autocomplete.tsx:190, 461` + `component/prompt/index.tsx:512` |
| describeTerminal 4 级 env | `packages/tui/src/util/system.ts:15-20` |
| Zed detection | `packages/tui/src/editor-zed.ts:198` + `context/editor.ts:121` |
| multiplexer/displayServer hints | `packages/tui/src/context/runtime.tsx:267-272` |
| tmux DCS passthrough | `packages/tui/src/clipboard.ts:26-27` |
| **Win32 FFI** `ENABLE_PROCESSED_INPUT` clear + Ctrl-C guard | `packages/tui/src/terminal-win32.ts:1-130`（`bun:ffi` → `kernel32.dll`）|
| Kitty 测试 fixture | `packages/tui/test/cli/tui/dialog-prompt.test.tsx:85` |
| `@opentui/core@0.4.5` SDK | `bun.lock:2042` |
| `bun-ffi-structs@0.2.4` + `string-width@7.2.0` | `bun.lock:2042` |
| SDK deps install policy | `bunfig.toml:5` |
| **无 Go**（find opencode -name "*.go" 零结果）| 验证 |
| 类型 ESM | `packages/tui/package.json:5` |

### deepseek-harness（TypeScript + Cordis）
| 主题 | 路径 |
|------|------|
| **明确无 TUI** —— 2026-07-16 agent-note 显式拒绝 alternate-screen / BEL 路径 | `.agents/notes/implemented/feature/2026-07-16-persistent-pty-sessions.md:133, 153` |
| TerminalSessionService（PTY registry）| `packages/terminal/terminal/src/index.ts:7-454`（476 行）|
| `TerminalBackend` 接口 | `packages/terminal/terminal/src/types.ts:148-171` |
| `TerminalSessionStatus` / `TerminalWaitReason` | `packages/terminal/terminal/src/types.ts:29-41` |
| 6 个 model-facing tools（open/send/read/signal/close/list）| `packages/terminal/tool-terminal/src/index.ts:163-401`（402 行）|
| `presentCall`/`presentResult` `card: 'terminal'` | `packages/terminal/tool-terminal/src/render.ts:106-158` |
| TextRetainer head/tail 截断 | `packages/terminal/tool-terminal/src/render.ts:1-60` |
| **bash PTY backend**（terminal-bash）| `packages/terminal/terminal-bash/src/index.ts`（218 行）|
| **`@xterm/headless` v6 cell emulator**（scrollback=0）| `packages/terminal/terminal-bash/src/session.ts:5, 207-220` |
| TerminalSanitizer 流式 CSI/OSC/short-escape 移除 | `packages/terminal/terminal-bash/src/sanitize.ts:38-188` |
| ConEmu OSC 133 ; D prompt marker | `packages/terminal/terminal-bash/src/sanitize.ts:6-9` |
| LocalPtySession implements TerminalBackendSession | `packages/terminal/terminal-bash/src/session.ts:708 行` |
| pollReadiness（prompt/idle/handoff/stdin-read）| `packages/terminal/terminal-bash/src/session.ts:472-523` |
| queueEmulatorData / pumpEmulator async pump | `packages/terminal/terminal-bash/src/session.ts:557-594` |
| 配置：sandbox + readiness timings + scrollback bounds | `packages/terminal/terminal-bash/src/config.ts`（122 行）|
| Line-oriented 输出策略 README | `packages/terminal/terminal-bash/README.md:86, 165` |
| 浏览器 React slot renderer | `packages/client/ui-renderer/src/client/index.ts:71-97, 88-97` |
| SlotOutlet `display: 'contents'` | `packages/client/ui-renderer/src/client/scoped-slots.tsx:686` |
| `bindSnapshotSelector` uSES bridge | `packages/client/ui-renderer/src/client/bind.ts:21-27` |
| buildRenderApp | `packages/client/ui-renderer/src/client/app.tsx:19-22` |

### undici（JavaScript）
| 主题 | 路径 |
|------|------|
| （无 TUI）| —— HTTP 客户端，仅 N/A 标注 |

### laew（Rust，现状）
| 主题 | 路径 |
|------|------|
| 全量重绘 present | `src/tui/engine.rs:213-243` |
| Frame cell 数组 | `src/tui/engine.rs:42-65` |
| Alternate screen | `src/tui/engine.rs:198-210` |
| 缺：cell-diff | ❌ |
| 缺：DEC 2026 | ❌ |
| 缺：Kitty CSI-u | ❌ |
| 缺：CJK 宽度 | ❌ |
| 缺：terminal caps | ❌ |
| 缺：mouse | ❌ |

---

## 16. 文档元信息

- **作者**：第八轮源码深挖 SubAgent
- **生成日期**：2026-09-07
- **覆盖工程数**：8（atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / undici / laew）
- **代码锚点数**：约 90+ 处具体 `file:line` 引用
- **不重复第六轮**：本专题专注 cell-based 内部细节 + Kitty 真实协商/解析 + DEC 2026 嵌套语义 + CJK 宽度算法 + worker + 鼠标协议 + 终端能力探测，与第六轮 TUI 主题差异化
- **下轮展望**：第九轮可聚焦「多 Agent 间消息总线 + 调度器 lane 三态 / 写锁 fence」（pi / openclaw / Switchyard 横向）