# TUI 输入处理常见陷阱与修复记录

## 1. 退格键在某些终端下不生效（2026-09-03 修复）

### 症状

在 TUI 交互模式下，输入文本后按退格键，每按一次退格不是原地编辑当前行，而是换行重新显示 `>>` 提示符 + 剩余文本：

```
>> /provider lis
>> /provider li
>> /provider l
>> /provider
>> /provide
...
```

### 根因

`InputHandler::read_line_inner()` 的退格处理只匹配了 `KeyCode::Backspace`：

```rust
KeyCode::Backspace => {
    // 删除光标前字符
    ...
}
KeyCode::Char(c) => {
    // 可打印字符：插入到光标位置
    buffer.insert(cursor, c);
    ...
}
```

某些终端环境下（Docker、SSH、TERM 类型不标准等），退格键发送的字符码未被 crossterm 映射为 `KeyCode::Backspace`，而是作为 `Char('\x7f')` (DEL) 或 `Char('\x08')` (BS) 传递。此时退格字符落入 `Char(c)` 分支被当作普通字符插入缓冲区，导致显示异常。

### 修复

在 `KeyCode::Backspace` 之后、`KeyCode::Char(c)` 之前，添加退格字符变体的兜底处理：

```rust
// src/tui/input.rs
KeyCode::Char('\x7f') | KeyCode::Char('\x08') => {
    // 兜底：处理退格字符变体（DEL=0x7f / BS=0x08）
    if cursor > 0 {
        cursor -= 1;
        buffer.remove(cursor);
        self.redraw_line(&mut stdout, prompt, &buffer, cursor, prompt_width)?;
        self.update_completion(...)?;
    }
}
```

### 教训

1. **crossterm 的 `KeyCode::Backspace` 不是万能的**：不同终端发送不同的退格字符码，crossterm 可能无法全部映射。
2. **match 分支顺序很重要**：`Char('\x7f')` / `Char('\x08')` 必须在通用 `Char(c)` 之前匹配，否则退格字符会被当作普通字符插入。
3. **终端兼容性需要显式处理**：不能假设所有终端都正确支持 raw mode 的所有方面。

### 验证

- 单元测试：`test_complete_backspace_scenario` 验证补全引擎在退格场景下的行为
- E2E 测试：`run_e2e.sh` Section 8 步骤10 使用 tmux `C-h` 发送退格验证原地编辑

### 相关文件

- `src/tui/input.rs` — `InputHandler::read_line_inner()` 退格处理
- `testReport/run_e2e.sh` — Section 8 步骤10 退格键测试
- `tmpPlan/01-退格键bug分析与修复方案.md` — 详细分析文档

---

## 2. tmux send-keys 退格键名称（2026-09-03 记录）

### 陷阱

在 tmux 自动化测试中，使用 `tmux send-keys Backspace` 不会发送退格控制字符，而是发送字面量文本 `"Backspace"`。

### 正确做法

```bash
# ❌ 错误：发送字面量文本 "Backspace"
tmux send-keys -t session Backspace

# ✅ 正确：发送 Ctrl+H（即 \x08，标准退格字符之一）
tmux send-keys -t session C-h
```

### 原因

tmux `send-keys` 的特殊键名列表中没有 `Backspace`。`Backspace` 被视为普通字符串（类似 `-l` 模式）。正确的退格键名是 `C-h`（Ctrl+H = `\x08`）。

### 其他常用 tmux 键名参考

| 键名 | 含义 |
|------|------|
| `C-h` | 退格（\x08） |
| `Enter` | 回车 |
| `Escape` | Esc |
| `Tab` | Tab |
| `Up` / `Down` / `Left` / `Right` | 方向键 |
| `C-c` | Ctrl+C |
| `C-d` | Ctrl+D |
| `BSpace` | 退格（部分 tmux 版本） |

---

## 3. 新增 TUI 子屏的断言锚点（通用指引）

### 规则

新增子屏时，`Screen::title()` 返回的字符串本身就是 tmux 断言锚点：

```rust
// src/tui/screen/xxx.rs
impl Screen for XxxScreen {
    fn title(&self) -> &str { "xxx title" }
    ...
}
```

在 `run_e2e.sh` 中：

```bash
tsubmit "/xxx"
texpect "xxx title" "tmux: 进入 Xxx 子屏"
```

### 注意事项

1. 断言文本必须与 `title()` 返回值完全一致
2. 使用 `texpect` 而非简单的 `grep`，因为子屏渲染需要时间
3. 子屏退出后验证回到主屏（检查 `>>` 提示符且无子屏边框字符）

## 4. 显示宽度按「字符数」算，CJK 场景盒子参差 + 进度行折行残影（2026-09-24 修复）

### 症状

启动横幅右边框不成一条直线，被截断的行还会顶穿边框：

```
║  项目说明: 未找到                                        ║   ← 60 列
║  工作区 : [通用] 非 git                                 ║    ← 59 列（短 1）
║  当前模型: [anthropic] liusm191-laew-model/liusm191-laew… ║  ← 61 列（溢出 1）
╚══════════════════════════════════════════════════════════╝
```

`/help` 同理（实测行宽 62/63/64/65 混合）；`[waiting]` 心跳行在 80 列终端上偶发半行残影。

### 根因（四条，互相独立）

1. **盒宽与行预算两套常量不自洽**：边框写死 58 内宽，每行内容按 `fit_display(v, 45/46)`
   独立填充。只有 `标签显示宽 + 预算 == 55` 的行才对齐 58，`工作区 :`(9)+45=54 就短 1 列。
2. **`truncate()` 省略号预算越界**：先截到 `≤max` 再 `out + "…"`，返回值宽度 = `max+1`，
   `fit_display` 的补空格算式 `max - w` 因此得 0 —— 任何被截断的行都比盒宽多 1 列。
3. **宽度按字符数而非显示列**：`truncate_chars(s, 40)` / `short_stage_label(.. MAX_CHARS=40)`
   放过 40 个汉字 = 80 列，加前缀必然折行。而 `[waiting]` 行靠「回车回列 0 + `ESC[K`」
   **原地重写**，一旦折行，下一次回车回到的是物理行首而非屏幕行首 → 残影。
4. **盒宽与终端无关**：58 列内宽固定，100+ 列终端上 `当前模型`（含 endpoint）、长路径
   照样被砍尾 —— 用户主观感受即「信息显示不全」。

### 修复

显示宽度度量与盒线排版收敛到一个真源 `src/tui/textfit.rs`：

| 不变量 | 保证方式 |
| ------ | -------- |
| 截断结果宽度 `≤ max` | `clip` / `clip_mid`：省略号**计入预算**（不再追加） |
| 盒线严格等宽 | `InfoBox::render()`：内宽 = `min(内容自然宽, 终端可用宽)`，逐行 `pad` 到同一内宽 |
| 长信息不砍尾 | `RowKind::Kv` 超长**折行续排**（续行缩进到值列）；路径/URL 用 `KvKeepTail` 中间省略保尾 |
| 进度行不折行 | `stage_line_text()` / `short_stage_label()` 按显示列收敛，预留前缀+计时开销 |
| 三处度量不分叉 | `input::char_width`/`display_width` 转发 `textfit`，横幅与输入行同一套列数 |

新增度宽歧义开关：`LAEW_AMBIGUOUS_WIDE=1` 让 `✓ · … → ║` 等 East Asian Ambiguous 字符按
2 列参与**计算**（不改渲染），供把盒线渲染成双宽的老终端（部分 Windows conhost 中文字体 /
xterm `-ctwidth`）校正对齐；默认关闭 = 与修复前逐字节一致。

### 校验

- 单测：`textfit::infobox_各行严格等宽_多种终端宽`（40~200 列全等宽且不溢出）、
  `banner::窄终端折行而非砍尾_且一个字符都不丢`（折行后拼回原文零损失）。
- e2e：`run_e2e.sh` §7 抓真机横幅 `╔…╚` 区块，python3 按 `east_asian_width` 复算行宽，
  断言集合大小 == 1（`command -v python3` 缺失时 SKIP）。

### 注意事项

1. **新增任何盒线/信息块一律走 `textfit::InfoBox`**，不要手写 `"═".repeat(58)` 或逐行调空格。
2. 度量只用 `textfit::width`；按 `chars().count()` 估宽在 CJK 下必然低估一倍。
3. 原地重写类行（`\r` + `ESC[K`）必须先保证「整行不超终端宽」，否则 ANSI 纪律失效。
